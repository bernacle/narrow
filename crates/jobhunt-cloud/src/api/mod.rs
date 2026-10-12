//! The HTTP API: `/api/v1/…`, health checks, OAuth protected-resource
//! metadata, and the hosted MCP endpoint (`/mcp`).
//!
//! Every endpoint is one application use case for the authenticated
//! account: the handler parses the request, builds the account's
//! application (a user-scoped view of the store, see
//! [`ApiState::app_for`]), calls the use case and returns the view the
//! MCP tool of the same name returns. Nothing here decides anything about
//! jobs, profiles, eligibility or ranking.
//!
//! | Method | Path | Answer |
//! | --- | --- | --- |
//! | GET | `/health` | the process is up |
//! | GET | `/ready` | Postgres answers and the schema is current |
//! | GET | `/.well-known/oauth-protected-resource[/mcp]` | RFC 9728 metadata |
//! | GET | `/api/v1/auth/config` | how to sign in |
//! | POST | `/api/v1/auth/dev-token` | development servers only |
//! | GET, DELETE | `/api/v1/account` | the account; delete it and its data |
//! | POST | `/api/v1/account/logout` | revoke every session and token |
//! | GET, POST | `/api/v1/tokens` | personal access tokens |
//! | DELETE | `/api/v1/tokens/{id}` | revoke one |
//! | POST | `/api/v1/sync/pull`, `/api/v1/sync/push` | sync (see `jobhunt_storage::sync`) |
//! | POST | `/api/v1/search` | the shortlist (`search_jobs`) |
//! | GET | `/api/v1/opportunities/{id}` | details (`get_job`) |
//! | POST | `/api/v1/opportunities/{id}/verify` | `verify_job` |
//! | POST | `/api/v1/opportunities/{id}/feedback` | save, reject, applied, … |
//! | GET | `/api/v1/opportunities/{id}/application-context` | `prepare_application_context` |
//! | GET | `/api/v1/profile` | `get_profile` |
//! | POST | `/api/v1/preferences` | `update_preferences` |
//! | GET | `/api/v1/pipeline` | `get_pipeline` |
//! | GET | `/api/v1/export` | the portable state file |
//! | GET | `/api/v1/feed` | the Today feed (`get_feed`) |
//! | POST | `/api/v1/opportunities/{id}/dismiss` | put it aside ("not now") |
//! | GET | `/api/v1/taste` | stated preferences and learned taste (`get_taste`) |
//! | GET, POST | `/api/v1/taste/profile` | the candidate taste profile; describe, confirm, correct (`get_taste_profile`, `update_taste_profile`) |
//! | GET, POST | `/api/v1/profile/claims` | claims needing review; confirm / reject them |
//! | PUT | `/api/v1/profile/resume` | import or re-import a resume (the body is the file) |
//! | PUT | `/api/v1/profile/linkedin` | import a LinkedIn data export (the body is the file) |
//! | POST | `/api/v1/profile/github` | import a public GitHub account |
//! | DELETE | `/api/v1/profile/sources/{source}` | take a LinkedIn or GitHub source out |
//! | GET, PUT | `/api/v1/notifications` | email notification settings |
//! | POST | `/api/v1/notifications/confirm` | confirm the address |
//! | * | `/mcp` | hosted MCP (Streamable HTTP) |

pub mod error;
pub mod types;

use std::sync::Arc;
use std::time::Duration;

use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{FromRequest, FromRequestParts, Path, Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use chrono::{NaiveTime, Utc};
use http::StatusCode;
use jobhunt_app::context::ApplicationContext;
use jobhunt_app::feed::{FeedRequest, FeedView, MAX_FEED_LIMIT};
use jobhunt_app::feedback::{FeedbackResult, PipelineView};
use jobhunt_app::inspect::{JobDetail, VerificationReport};
use jobhunt_app::preferences::{PreferenceUpdate, PreferenceUpdateResult};
use jobhunt_app::profile_edit::{ClaimDecisionResult, ClaimReview, ResumeImportResult};
use jobhunt_app::profile_sources::{
    EvidenceSource, GithubAccess, SourceImportResult, SourceRemovalResult,
};
use jobhunt_app::profile_view::ProfileView;
use jobhunt_app::shortlist::MAX_LIMIT;
use jobhunt_app::state::StateExport;
use jobhunt_app::taste_profile::{TasteAction, TasteProfileView, TasteUpdateResult};
use jobhunt_app::taste_view::TasteView;
use jobhunt_app::{App, AppError, DiscoveryMode, FindRequest, Quiet, RefreshMode, SearchResults};
use jobhunt_jobs::verification::VerifyMode;
use jobhunt_mcp::{SearchJobsParams, UpdatePreferencesParams};
use jobhunt_ranking::FeedbackAction;
use jobhunt_storage::postgres::{
    Cadence, DeliveryKind, DigestSettings, NewDelivery, PgStore, PgUserStore, SettingsChange,
    new_confirmation_token, new_delivery_id,
};
use jobhunt_storage::sync::{PullRequest, PullResponse, PushError, PushRequest, PushResponse};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::request_id::{PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::sensitive_headers::SetSensitiveRequestHeadersLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

use self::error::ApiError;
use self::types::*;
use crate::auth::{Authenticator, DevVerifier, Method, OidcVerifier, Principal};
use crate::config::{AuthConfig, CloudConfig};
use crate::email::EmailSender;
use crate::observability::{REQUEST_ID, RandomRequestId, make_span, on_response};
use crate::usage::UsageLog;

/// Everything the handlers share.
#[derive(Clone)]
pub struct ApiState {
    inner: Arc<Inner>,
}

struct Inner {
    store: PgStore,
    config: Arc<CloudConfig>,
    auth: Authenticator,
    oidc: Option<Arc<OidcVerifier>>,
    dev: Option<DevVerifier>,
    usage: UsageLog,
    email: Option<Arc<dyn EmailSender>>,
    /// Shared by every request: one HTTP client, one configuration.
    taste_model: jobhunt_app::TasteModel,
    fit_review: jobhunt_app::FitReview,
}

impl std::fmt::Debug for ApiState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiState").finish_non_exhaustive()
    }
}

impl ApiState {
    /// The shared state: the authenticator for the configured mode, and
    /// the configured email provider.
    pub fn new(
        store: PgStore,
        config: Arc<CloudConfig>,
        usage: UsageLog,
    ) -> Result<Self, crate::config::ConfigProblems> {
        let email = crate::email::sender(config.email.as_ref())
            .map_err(|e| crate::config::ConfigProblems(vec![format!("email: {e}")]))?;
        Self::with_email(store, config, usage, email)
    }

    /// Like [`ApiState::new`], with this email sender (tests use an
    /// in-memory one).
    pub fn with_email(
        store: PgStore,
        config: Arc<CloudConfig>,
        usage: UsageLog,
        email: Option<Arc<dyn EmailSender>>,
    ) -> Result<Self, crate::config::ConfigProblems> {
        let (auth, oidc, dev) = match &config.auth {
            Some(AuthConfig::Oidc(settings)) => {
                let verifier = Arc::new(
                    OidcVerifier::new(settings.clone())
                        .map_err(|e| crate::config::ConfigProblems(vec![e.to_string()]))?,
                );
                (
                    Authenticator::new(verifier.clone(), Method::Oidc, store.clone()),
                    Some(verifier),
                    None,
                )
            }
            Some(AuthConfig::Dev { secret }) => {
                let dev = DevVerifier::new(secret.clone());
                (
                    Authenticator::new(Arc::new(dev.clone()), Method::Dev, store.clone()),
                    None,
                    Some(dev),
                )
            }
            None => {
                return Err(crate::config::ConfigProblems(vec![
                    "authentication is not configured".into(),
                ]));
            }
        };
        let taste_model = match &config.ai {
            Ok(Some(model)) => match jobhunt_ai::ModelInterpreter::new(model.clone()) {
                Ok(m) => jobhunt_app::TasteModel {
                    model: Some(Arc::new(m)),
                    problem: None,
                },
                Err(e) => jobhunt_app::TasteModel {
                    model: None,
                    problem: Some(e.to_string()),
                },
            },
            Ok(None) => jobhunt_app::TasteModel::default(),
            Err(problem) => {
                tracing::warn!(%problem, "JOBHUNT_AI_* can't be used; the built-in taste reader is");
                jobhunt_app::TasteModel {
                    model: None,
                    problem: Some(problem.clone()),
                }
            }
        };
        let fit_review = crate::config::fit_review(&config);
        Ok(Self {
            inner: Arc::new(Inner {
                store,
                config,
                auth,
                oidc,
                dev,
                usage,
                email,
                taste_model,
                fit_review,
            }),
        })
    }

    pub fn store(&self) -> &PgStore {
        &self.inner.store
    }

    pub fn usage(&self) -> &UsageLog {
        &self.inner.usage
    }

    pub fn config(&self) -> &CloudConfig {
        &self.inner.config
    }

    /// The account's application: the shared configuration over a view of
    /// the store bound to this account. Built per request; cheap.
    pub fn app_for(&self, principal: &Principal) -> Arc<App> {
        Arc::new(
            App::from_parts(
                Arc::clone(&self.inner.config.app),
                Arc::new(self.user_store(principal)),
                DiscoveryMode::Background,
            )
            .with_taste_model(self.inner.taste_model.clone())
            .with_fit_review(self.inner.fit_review.clone()),
        )
    }

    fn user_store(&self, principal: &Principal) -> PgUserStore {
        self.inner.store.for_user(principal.user.clone())
    }

    /// Where MCP clients read how to authenticate (RFC 9728).
    pub fn resource_metadata_url(&self) -> Option<String> {
        self.inner.config.public_url.as_ref().map(|u| {
            format!(
                "{}/.well-known/oauth-protected-resource/mcp",
                u.as_str().trim_end_matches('/')
            )
        })
    }
}

/// A JSON body whose rejection is a structured error.
pub struct Body<T>(pub T);

impl<S, T> FromRequest<S> for Body<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
    Json<T>: FromRequest<S, Rejection = JsonRejection>,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        Json::<T>::from_request(req, state)
            .await
            .map(|Json(v)| Self(v))
            .map_err(|e| ApiError::invalid(e.body_text()))
    }
}

/// Query parameters whose rejection is a structured error.
pub struct Params<T>(pub T);

impl<S, T> FromRequestParts<S> for Params<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut http::request::Parts,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        axum::extract::Query::<T>::from_request_parts(parts, state)
            .await
            .map(|axum::extract::Query(v)| Self(v))
            .map_err(|e: QueryRejection| ApiError::invalid(e.body_text()))
    }
}

/// Runs a use case on the account's application.
///
/// Like the MCP server, the use case runs on a blocking thread driven by
/// the runtime: discovery and verification drive stream combinators over
/// borrowed data whose futures cannot be proven `Send` in a generic
/// context, which axum handlers require.
async fn run<T, F, Fut>(app: Arc<App>, work: F) -> Result<T, ApiError>
where
    T: Send + 'static,
    F: FnOnce(Arc<App>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<T, AppError>>,
{
    let handle = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || handle.block_on(work(app)))
        .await
        .map_err(|error| {
            tracing::error!(%error, "a use case panicked or was cancelled");
            ApiError::internal()
        })?
        .map_err(ApiError::from)
}

/// Resolves the bearer token; the principal goes into the request's
/// extensions (the hosted MCP server reads it from there).
async fn authenticate(State(state): State<ApiState>, mut request: Request, next: Next) -> Response {
    let bearer = request
        .headers()
        .get(http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            v.strip_prefix("Bearer ")
                .or_else(|| v.strip_prefix("bearer "))
        })
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_owned);
    let result = match bearer {
        Some(token) => state.inner.auth.authenticate(&token).await,
        None => Err(crate::auth::AuthError::Missing),
    };
    match result {
        Ok(principal) => {
            tracing::Span::current().record("user", principal.user.as_str());
            request.extensions_mut().insert(principal);
            next.run(request).await
        }
        Err(error) => {
            tracing::info!(reason = %error, "unauthenticated request");
            ApiError::unauthenticated(&error, state.resource_metadata_url().as_deref())
                .into_response()
        }
    }
}

type Auth = axum::Extension<Principal>;

/// Which kind of client made a request (`x-jobhunt-client`), for usage
/// events: `web`, `cli`, `mcp` or `other`. Never anything identifying.
fn client_of(headers: &http::HeaderMap) -> &'static str {
    match headers
        .get("x-jobhunt-client")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
    {
        Some("web") => "web",
        Some("cli") => "cli",
        Some("mcp") => "mcp",
        _ => "other",
    }
}

async fn health() -> Json<Health> {
    Json(Health {
        status: "ok".into(),
        version: env!("CARGO_PKG_VERSION").into(),
    })
}

async fn ready(State(state): State<ApiState>) -> Response {
    let database = state.store().ping().await;
    let schema = state.store().schema().await;
    let (db, schema_text, ok) = match (&database, &schema) {
        (Ok(()), Ok(s)) if s.is_current() => ("ok".to_owned(), "current".to_owned(), true),
        (Ok(()), Ok(s)) => (
            "ok".to_owned(),
            format!("{} of {} migrations applied", s.applied, s.known),
            false,
        ),
        _ => ("unreachable".to_owned(), "unknown".to_owned(), false),
    };
    let body = Json(Ready {
        status: if ok { "ready" } else { "not_ready" }.into(),
        database: db,
        schema: schema_text,
    });
    if ok {
        body.into_response()
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, body).into_response()
    }
}

/// OAuth 2.0 Protected Resource Metadata (RFC 9728), which MCP clients
/// read to find the authorization server.
async fn protected_resource(State(state): State<ApiState>) -> Result<Response, ApiError> {
    let Some(base) = state.config().public_url.as_ref() else {
        return Err(ApiError::not_found(
            "resource metadata (JOBHUNT_PUBLIC_URL is not set)",
        ));
    };
    let issuer = match &state.config().auth {
        Some(AuthConfig::Oidc(settings)) => vec![settings.issuer.clone()],
        _ => Vec::new(),
    };
    let scopes = match &state.config().auth {
        Some(AuthConfig::Oidc(settings)) => settings
            .scopes
            .split_whitespace()
            .map(str::to_owned)
            .collect(),
        _ => Vec::<String>::new(),
    };
    Ok(Json(serde_json::json!({
        "resource": format!("{}/mcp", base.as_str().trim_end_matches('/')),
        "authorization_servers": issuer,
        "bearer_methods_supported": ["header"],
        "scopes_supported": scopes,
        // Shown to people by MCP clients when they sign in.
        "resource_name": "Narrow",
    }))
    .into_response())
}

async fn auth_config(State(state): State<ApiState>) -> Result<Json<AuthConfigView>, ApiError> {
    Ok(Json(match &state.config().auth {
        Some(AuthConfig::Oidc(settings)) => {
            let metadata = match &state.inner.oidc {
                Some(v) => v.metadata().await.ok(),
                None => None,
            };
            AuthConfigView {
                mode: "oidc".into(),
                issuer: Some(settings.issuer.clone()),
                audience: settings.audiences.first().cloned(),
                audience_parameter: settings.audience_parameter.field().map(str::to_owned),
                cli_client_id: settings.cli_client_id.clone(),
                scopes: Some(settings.scopes.clone()),
                device_authorization_endpoint: metadata
                    .as_ref()
                    .and_then(|m| m.device_authorization_endpoint.clone()),
                token_endpoint: metadata.as_ref().and_then(|m| m.token_endpoint.clone()),
                revocation_endpoint: metadata.and_then(|m| m.revocation_endpoint),
            }
        }
        _ => AuthConfigView {
            mode: "dev".into(),
            issuer: None,
            audience: None,
            audience_parameter: None,
            cli_client_id: None,
            scopes: None,
            device_authorization_endpoint: None,
            token_endpoint: None,
            revocation_endpoint: None,
        },
    }))
}

async fn dev_token(
    State(state): State<ApiState>,
    Body(request): Body<DevTokenRequest>,
) -> Result<Json<DevTokenResponse>, ApiError> {
    let Some(dev) = &state.inner.dev else {
        return Err(ApiError::not_found("development sign-in"));
    };
    let subject = request.subject.trim();
    if subject.is_empty() || subject.len() > 64 {
        return Err(ApiError::invalid("subject must be 1 to 64 characters"));
    }
    let ttl = chrono::Duration::hours(12);
    let token = dev.mint(subject, ttl).map_err(|_| ApiError::internal())?;
    state
        .usage()
        .record(None, "login", serde_json::json!({"method": "dev"}));
    Ok(Json(DevTokenResponse {
        access_token: token,
        expires_in: 12 * 3600,
    }))
}

async fn account(
    State(state): State<ApiState>,
    axum::Extension(principal): Auth,
) -> Result<Json<AccountView>, ApiError> {
    let account = state
        .store()
        .account(&principal.user)
        .await?
        .ok_or_else(|| ApiError::not_found("account"))?;
    let cloud = state.user_store(&principal).sync_state().await?;
    Ok(Json(AccountView {
        id: account.id.to_string(),
        created_at: account.created_at,
        authenticated_with: match principal.method {
            Method::Oidc => "oidc",
            Method::Token => "token",
            Method::Dev => "dev",
        }
        .into(),
        identities: account
            .identities
            .into_iter()
            .map(|i| IdentityView {
                issuer: i.issuer,
                created_at: i.created_at,
                last_login_at: i.last_login_at,
            })
            .collect(),
        cloud: CloudStateView {
            change_seq: cloud.change_seq,
            profile_revision: cloud.profile_revision,
            last_sync_at: cloud.last_sync_at,
            last_shortlist_at: cloud.last_shortlist_at,
            feedback: cloud.feedback,
            notification_cursor: cloud.notification_cursor,
        },
    }))
}

async fn logout(
    State(state): State<ApiState>,
    axum::Extension(principal): Auth,
) -> Result<StatusCode, ApiError> {
    state
        .store()
        .revoke_sessions(&principal.user, Utc::now())
        .await?;
    state.inner.auth.forget(&principal.user).await;
    state
        .usage()
        .record(Some(&principal.user), "logout", serde_json::json!({}));
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_account(
    State(state): State<ApiState>,
    axum::Extension(principal): Auth,
) -> Result<StatusCode, ApiError> {
    state.store().delete_account(&principal.user).await?;
    state.inner.auth.forget(&principal.user).await;
    tracing::info!(user = %principal.user, "account deleted");
    Ok(StatusCode::NO_CONTENT)
}

fn token_view(t: jobhunt_storage::postgres::ApiToken) -> TokenView {
    TokenView {
        id: t.id,
        name: t.name,
        created_at: t.created_at,
        last_used_at: t.last_used_at,
        expires_at: t.expires_at,
        revoked_at: t.revoked_at,
    }
}

async fn list_tokens(
    State(state): State<ApiState>,
    axum::Extension(principal): Auth,
) -> Result<Json<TokenList>, ApiError> {
    let tokens = state.store().api_tokens(&principal.user).await?;
    Ok(Json(TokenList {
        tokens: tokens.into_iter().map(token_view).collect(),
    }))
}

async fn create_token(
    State(state): State<ApiState>,
    axum::Extension(principal): Auth,
    Body(request): Body<CreateTokenRequest>,
) -> Result<(StatusCode, Json<CreatedToken>), ApiError> {
    let name = request.name.trim();
    if name.is_empty() || name.chars().count() > 100 {
        return Err(ApiError::invalid("name must be 1 to 100 characters"));
    }
    let days = request.expires_in_days.unwrap_or(90);
    if !(1..=365).contains(&days) {
        return Err(ApiError::invalid(
            "expires_in_days must be between 1 and 365",
        ));
    }
    let now = Utc::now();
    let created = state
        .store()
        .create_api_token(
            &principal.user,
            name,
            Some(now + chrono::Duration::days(i64::from(days))),
            now,
        )
        .await?;
    state.usage().record(
        Some(&principal.user),
        "token_created",
        serde_json::json!({"days": days}),
    );
    Ok((
        StatusCode::CREATED,
        Json(CreatedToken {
            token: token_view(created.token),
            secret: created.secret,
        }),
    ))
}

async fn revoke_token(
    State(state): State<ApiState>,
    axum::Extension(principal): Auth,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    if state
        .store()
        .revoke_api_token(&principal.user, &id, Utc::now())
        .await?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found("token"))
    }
}

async fn sync_pull(
    State(state): State<ApiState>,
    axum::Extension(principal): Auth,
    Body(request): Body<PullRequest>,
) -> Result<Json<PullResponse>, ApiError> {
    if request.protocol != jobhunt_storage::sync::SYNC_PROTOCOL {
        return Err(ApiError::invalid(format!(
            "sync protocol {} is not supported (this server speaks {}); update JobHunt",
            request.protocol,
            jobhunt_storage::sync::SYNC_PROTOCOL
        )));
    }
    let pulled = state
        .user_store(&principal)
        .sync_pull(&request, Utc::now())
        .await?;
    state.usage().record(
        Some(&principal.user),
        "sync_pull",
        serde_json::json!({
            "entities": pulled.entities.len(),
            "feedback": pulled.feedback.len(),
            "jobs": pulled.jobs.len(),
        }),
    );
    Ok(Json(pulled))
}

async fn sync_push(
    State(state): State<ApiState>,
    axum::Extension(principal): Auth,
    Body(request): Body<PushRequest>,
) -> Result<Json<PushResponse>, ApiError> {
    let pushed = state
        .user_store(&principal)
        .sync_push(&request, Utc::now())
        .await
        .map_err(|e| match e {
            PushError::Invalid(message) => {
                ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, "invalid_sync", message)
            }
            PushError::Storage(e) => e.into(),
        })?;
    state.usage().record(
        Some(&principal.user),
        "sync_push",
        serde_json::json!({
            "entities": request.entities.len(),
            "feedback": request.feedback.len(),
            "conflicts": pushed.conflicts.len(),
        }),
    );
    Ok(Json(pushed))
}

async fn search(
    State(state): State<ApiState>,
    axum::Extension(principal): Auth,
    Body(p): Body<SearchJobsParams>,
) -> Result<Json<SearchResults>, ApiError> {
    let limit = usize::from(p.limit.unwrap_or(5));
    if limit == 0 || limit > MAX_LIMIT {
        return Err(ApiError::invalid(format!(
            "limit must be between 1 and {MAX_LIMIT}"
        )));
    }
    let request = FindRequest {
        text: p.query.unwrap_or_default(),
        limit,
        all_tiers: p.include_lower_tiers,
        // Discovery runs in the background; `refresh: never` still means
        // "don't verify either".
        refresh: RefreshMode::Never,
        verify: p.refresh != jobhunt_mcp::RefreshInput::Never && p.verify.unwrap_or(true),
    };
    let now = Utc::now();
    let results = run(state.app_for(&principal), move |app| async move {
        let found = app.find(&request, &Quiet, now).await?;
        Ok(SearchResults::of(&found, now))
    })
    .await?;
    state.usage().record(
        Some(&principal.user),
        "find",
        serde_json::json!({"limit": limit, "results": results.results.len()}),
    );
    Ok(Json(results))
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct DetailQuery {
    #[serde(default)]
    include_sources: bool,
    #[serde(default)]
    full_description: bool,
}

async fn opportunity(
    State(state): State<ApiState>,
    axum::Extension(principal): Auth,
    headers: http::HeaderMap,
    Path(id): Path<String>,
    Params(q): Params<DetailQuery>,
) -> Result<Json<JobDetail>, ApiError> {
    let client = client_of(&headers);
    let detail = run(state.app_for(&principal), move |app| async move {
        let opportunity = app.resolve(&id).await?;
        let inspection = app.inspect(&opportunity, false, Utc::now()).await?;
        Ok(JobDetail::of(
            &inspection,
            q.include_sources,
            q.full_description,
        ))
    })
    .await?;
    state.usage().record(
        Some(&principal.user),
        "opportunity_viewed",
        serde_json::json!({"client": client, "tier": detail.decision.as_ref().map(|d| d.tier)}),
    );
    Ok(Json(detail))
}

async fn verify(
    State(state): State<ApiState>,
    axum::Extension(principal): Auth,
    Path(id): Path<String>,
    Body(request): Body<VerifyRequest>,
) -> Result<Json<VerificationReport>, ApiError> {
    let mode = if request.force {
        VerifyMode::Force
    } else {
        VerifyMode::IfDue
    };
    let report = run(state.app_for(&principal), move |app| async move {
        let opportunity = app.resolve(&id).await?;
        let checked = app.verify(&opportunity, mode, &Quiet, Utc::now()).await?;
        Ok(VerificationReport::of(&opportunity, &checked))
    })
    .await?;
    state.usage().record(
        Some(&principal.user),
        "verify",
        serde_json::json!({"force": request.force}),
    );
    Ok(Json(report))
}

async fn feedback(
    State(state): State<ApiState>,
    axum::Extension(principal): Auth,
    headers: http::HeaderMap,
    Path(id): Path<String>,
    Body(request): Body<FeedbackRequest>,
) -> Result<Json<FeedbackResult>, ApiError> {
    let client = client_of(&headers);
    let action = match request.action {
        FeedbackInput::Seen => FeedbackAction::Seen,
        FeedbackInput::Save => FeedbackAction::Save,
        FeedbackInput::Unsave => FeedbackAction::Unsave,
        FeedbackInput::Reject => FeedbackAction::Reject,
        FeedbackInput::Applied => FeedbackAction::Applied,
        FeedbackInput::Interview => FeedbackAction::Interview,
        FeedbackInput::Offer => FeedbackAction::Offer,
        FeedbackInput::Like => FeedbackAction::Like,
        FeedbackInput::Dislike => FeedbackAction::Dislike,
    };
    let reason = request.reason;
    let with_reason = reason.is_some();
    let result = run(state.app_for(&principal), move |app| async move {
        let opportunity = app.resolve(&id).await?;
        let outcome = app
            .record_feedback(&opportunity, action, reason.as_deref(), Utc::now())
            .await?;
        Ok(FeedbackResult::of(&outcome))
    })
    .await?;
    state.usage().record(
        Some(&principal.user),
        "feedback",
        serde_json::json!({
            "action": action.as_str(),
            "with_reason": with_reason,
            "recorded": result.recorded,
            "client": client,
        }),
    );
    Ok(Json(result))
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ContextQuery {
    #[serde(default)]
    include_contact_details: bool,
}

async fn application_context(
    State(state): State<ApiState>,
    axum::Extension(principal): Auth,
    Path(id): Path<String>,
    Params(q): Params<ContextQuery>,
) -> Result<Json<ApplicationContext>, ApiError> {
    let context = run(state.app_for(&principal), move |app| async move {
        let opportunity = app.resolve(&id).await?;
        app.application_context(&opportunity, q.include_contact_details, Utc::now())
            .await
    })
    .await?;
    Ok(Json(context))
}

async fn profile(
    State(state): State<ApiState>,
    axum::Extension(principal): Auth,
) -> Result<Json<ProfileView>, ApiError> {
    let view = run(state.app_for(&principal), |app| async move {
        app.profile_view().await
    })
    .await?;
    Ok(Json(view))
}

async fn preferences(
    State(state): State<ApiState>,
    axum::Extension(principal): Auth,
    Body(p): Body<UpdatePreferencesParams>,
) -> Result<Json<PreferenceUpdateResult>, ApiError> {
    let update = PreferenceUpdate {
        statement: p.statement,
        set: p.set,
        remove: p.remove,
    };
    let counts = serde_json::json!({
        "statement": update.statement.is_some(),
        "set": update.set.len(),
        "remove": update.remove.len(),
    });
    let result = run(state.app_for(&principal), move |app| async move {
        let changes = app.update_preferences(&update, Utc::now()).await?;
        Ok(PreferenceUpdateResult::of(&changes))
    })
    .await?;
    state
        .usage()
        .record(Some(&principal.user), "preferences", counts);
    Ok(Json(result))
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PipelineQuery {
    #[serde(default)]
    include_rejected: bool,
}

async fn pipeline(
    State(state): State<ApiState>,
    axum::Extension(principal): Auth,
    Params(q): Params<PipelineQuery>,
) -> Result<Json<PipelineView>, ApiError> {
    let view = run(state.app_for(&principal), move |app| async move {
        Ok(PipelineView::of(&app.pipeline(q.include_rejected).await?))
    })
    .await?;
    Ok(Json(view))
}

async fn export(
    State(state): State<ApiState>,
    axum::Extension(principal): Auth,
) -> Result<Json<StateExport>, ApiError> {
    let export = run(state.app_for(&principal), |app| async move {
        app.export_state(
            Utc::now(),
            Some(format!("jobhunt-cloud {}", env!("CARGO_PKG_VERSION"))),
        )
        .await
    })
    .await?;
    Ok(Json(export))
}

async fn feed(
    State(state): State<ApiState>,
    axum::Extension(principal): Auth,
    headers: http::HeaderMap,
    Params(q): Params<FeedQuery>,
) -> Result<Json<FeedView>, ApiError> {
    let limit = q.limit.unwrap_or(jobhunt_app::feed::DEFAULT_FEED_LIMIT);
    if limit == 0 || limit > MAX_FEED_LIMIT {
        return Err(ApiError::invalid(format!(
            "limit must be between 1 and {MAX_FEED_LIMIT}"
        )));
    }
    let request = FeedRequest {
        limit,
        verify: true,
        // Discovery runs in the background.
        refresh: RefreshMode::Never,
    };
    let now = Utc::now();
    let view = run(state.app_for(&principal), move |app| async move {
        let feed = app.feed(&request, &Quiet, now).await?;
        Ok(FeedView::of(&feed, now))
    })
    .await?;
    state.usage().record(
        Some(&principal.user),
        "feed_opened",
        serde_json::json!({
            "client": client_of(&headers),
            "items": view.items.len(),
            "new": view.summary.new,
            "changed": view.summary.changed,
            "caught_up": view.caught_up,
        }),
    );
    Ok(Json(view))
}

async fn dismiss(
    State(state): State<ApiState>,
    axum::Extension(principal): Auth,
    headers: http::HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<FeedbackResult>, ApiError> {
    let result = run(state.app_for(&principal), move |app| async move {
        let opportunity = app.resolve(&id).await?;
        let outcome = app.dismiss(&opportunity, Utc::now()).await?;
        Ok(FeedbackResult::of(&outcome))
    })
    .await?;
    state.usage().record(
        Some(&principal.user),
        "dismiss",
        serde_json::json!({"client": client_of(&headers)}),
    );
    Ok(Json(result))
}

async fn taste(
    State(state): State<ApiState>,
    axum::Extension(principal): Auth,
) -> Result<Json<TasteView>, ApiError> {
    let view = run(state.app_for(&principal), |app| async move {
        app.taste_view().await
    })
    .await?;
    Ok(Json(view))
}

async fn taste_profile(
    State(state): State<ApiState>,
    axum::Extension(principal): Auth,
) -> Result<Json<TasteProfileView>, ApiError> {
    let view = run(state.app_for(&principal), |app| async move {
        app.taste_profile().await
    })
    .await?;
    Ok(Json(view))
}

async fn update_taste_profile(
    State(state): State<ApiState>,
    axum::Extension(principal): Auth,
    Body(action): Body<TasteAction>,
) -> Result<Json<TasteUpdateResult>, ApiError> {
    let name = action.name();
    let result = run(state.app_for(&principal), move |app| async move {
        app.review_taste(&action, Utc::now()).await
    })
    .await?;
    // Counts only: never the person's words.
    state.usage().record(
        Some(&principal.user),
        "taste_profile",
        serde_json::json!({
            "action": name,
            "interpreted": result.interpreted,
            "reader": result.profile.reader.kind,
            "outcome": result.profile.interpretation.as_ref().map(|i| i.outcome.clone()),
        }),
    );
    Ok(Json(result))
}

async fn claims(
    State(state): State<ApiState>,
    axum::Extension(principal): Auth,
    Params(q): Params<ClaimsQuery>,
) -> Result<Json<ClaimReview>, ApiError> {
    let review = run(state.app_for(&principal), move |app| async move {
        app.claims_for_review(q.limit).await
    })
    .await?;
    Ok(Json(review))
}

async fn decide_claims(
    State(state): State<ApiState>,
    axum::Extension(principal): Auth,
    headers: http::HeaderMap,
    Body(request): Body<DecideClaimsRequest>,
) -> Result<Json<ClaimDecisionResult>, ApiError> {
    let count = request.ids.len();
    let decision = request.decision;
    let with_note = request.note.is_some();
    let result = run(state.app_for(&principal), move |app| async move {
        app.decide_claims(&request.ids, request.decision, request.note, Utc::now())
            .await
    })
    .await?;
    state.usage().record(
        Some(&principal.user),
        "claims_decided",
        serde_json::json!({
            "client": client_of(&headers),
            "decision": decision,
            "count": count,
            "with_note": with_note,
        }),
    );
    Ok(Json(result))
}

/// The most a resume upload may be (the extractor refuses larger files).
const MAX_RESUME_BYTES: usize = 10 * 1024 * 1024;

async fn import_resume(
    State(state): State<ApiState>,
    axum::Extension(principal): Auth,
    headers: http::HeaderMap,
    Params(q): Params<ResumeQuery>,
    body: axum::body::Bytes,
) -> Result<Json<ResumeImportResult>, ApiError> {
    if body.is_empty() {
        return Err(ApiError::invalid("the body must be the resume file"));
    }
    if body.len() > MAX_RESUME_BYTES {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "invalid_request",
            "the resume is larger than 10 MB",
        ));
    }
    let content_type = headers
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let file_name = q
        .file_name
        .map(|n| n.trim().to_owned())
        .filter(|n| !n.is_empty() && n.len() <= 200)
        .or_else(|| {
            let default = if content_type.starts_with("application/pdf") {
                "resume.pdf"
            } else if content_type.starts_with("text/markdown") {
                "resume.md"
            } else if content_type.starts_with("text/plain") {
                "resume.txt"
            } else {
                return None;
            };
            Some(default.to_owned())
        })
        .ok_or_else(|| {
            ApiError::invalid(
                "name the file (?file_name=resume.pdf) or send a PDF, text or Markdown content type",
            )
        })?;
    let result = run(state.app_for(&principal), move |app| async move {
        app.import_resume(&body, Some(file_name), Utc::now()).await
    })
    .await?;
    // Counts only: never the resume's content or its file name.
    state.usage().record(
        Some(&principal.user),
        "resume_imported",
        serde_json::json!({
            "client": client_of(&headers),
            "first_import": result.first_import,
            "same_file": result.same_file,
            "experiences": result.experiences.added + result.experiences.updated + result.experiences.unchanged,
            "claims_added": result.claims.added,
            "needs_review": result.needs_review,
        }),
    );
    Ok(Json(result))
}

/// The most a LinkedIn export upload may be (the service's body limit).
const MAX_LINKEDIN_BYTES: usize = 16 * 1024 * 1024;

async fn import_linkedin(
    State(state): State<ApiState>,
    axum::Extension(principal): Auth,
    headers: http::HeaderMap,
    Params(q): Params<LinkedinQuery>,
    body: axum::body::Bytes,
) -> Result<Json<SourceImportResult>, ApiError> {
    if body.is_empty() {
        return Err(ApiError::invalid(
            "the body must be the LinkedIn export (.zip or .csv)",
        ));
    }
    if body.len() > MAX_LINKEDIN_BYTES {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "invalid_request",
            "the export is larger than 16 MB; ask LinkedIn for just the files you need (profile, positions, skills, …)",
        ));
    }
    let file_name = q
        .file_name
        .map(|n| n.trim().to_owned())
        .filter(|n| !n.is_empty() && n.len() <= 200);
    let result = run(state.app_for(&principal), move |app| async move {
        app.import_linkedin(&body, file_name, Utc::now()).await
    })
    .await?;
    // Counts only: never the export's content or its file name.
    state.usage().record(
        Some(&principal.user),
        "linkedin_imported",
        serde_json::json!({
            "client": client_of(&headers),
            "first_import": result.first_import,
            "unchanged": result.unchanged,
            "experiences_added": result.experiences.added,
            "corroborated": result.experiences.corroborated + result.claims.corroborated,
            "claims_added": result.claims.added,
            "conflicts": result.conflicts,
            "needs_review": result.needs_review,
        }),
    );
    Ok(Json(result))
}

async fn import_github(
    State(state): State<ApiState>,
    axum::Extension(principal): Auth,
    headers: http::HeaderMap,
    Body(request): Body<GithubImportRequest>,
) -> Result<Json<SourceImportResult>, ApiError> {
    let access = GithubAccess::at(
        state.config().github_endpoint().map(str::to_owned),
        state.config().github_token.clone(),
    );
    let result = run(state.app_for(&principal), move |app| async move {
        app.import_github(request.username.as_deref(), &access, Utc::now())
            .await
    })
    .await?;
    // Counts only: never the account name.
    state.usage().record(
        Some(&principal.user),
        "github_imported",
        serde_json::json!({
            "client": client_of(&headers),
            "first_import": result.first_import,
            "unchanged": result.unchanged,
            "projects_added": result.projects.added,
            "corroborated": result.projects.corroborated + result.claims.corroborated,
            "claims_added": result.claims.added,
            "problems": result.problems.len(),
            "needs_review": result.needs_review,
        }),
    );
    Ok(Json(result))
}

async fn remove_source(
    State(state): State<ApiState>,
    axum::Extension(principal): Auth,
    Path(source): Path<String>,
) -> Result<Json<SourceRemovalResult>, ApiError> {
    let source = EvidenceSource::parse(&source)
        .ok_or_else(|| ApiError::invalid("the source is `linkedin` or `github`"))?;
    let result = run(state.app_for(&principal), move |app| async move {
        app.remove_source(source, Utc::now()).await
    })
    .await?;
    state.usage().record(
        Some(&principal.user),
        "source_removed",
        serde_json::json!({
            "source": result.source,
            "records_deleted": result.records_deleted,
            "claims_deleted": result.claims_deleted,
            "decisions_kept": result.decisions_kept,
        }),
    );
    Ok(Json(result))
}

/// A plausible email address (the confirmation link proves it works).
fn valid_email(email: &str) -> bool {
    let Some((local, domain)) = email.split_once('@') else {
        return false;
    };
    (3..=254).contains(&email.len())
        && !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !domain.contains('@')
        && !email.chars().any(|c| c.is_whitespace() || c.is_control())
}

async fn notification_view(
    state: &ApiState,
    principal: &Principal,
) -> Result<NotificationSettingsView, ApiError> {
    let settings = state.store().notification_settings(&principal.user).await?;
    let recent = state.store().recent_deliveries(&principal.user, 5).await?;
    let digest = &settings.digest;
    let next_digest_at = (settings.email_enabled && settings.email_confirmed_at.is_some())
        .then(|| crate::digest::Schedule::of(digest).next(settings.last_digest_on, Utc::now()))
        .flatten();
    Ok(NotificationSettingsView {
        available: state.inner.email.is_some() && state.config().web_url.is_some(),
        email_enabled: settings.email_enabled,
        cadence: settings.cadence.as_str().to_owned(),
        timezone: digest.timezone.clone(),
        digest_time: digest.time.format("%H:%M").to_string(),
        digest_days: crate::digest::day_names(digest.days),
        digest_size: digest.size,
        quiet_start: digest.quiet_start.format("%H:%M").to_string(),
        quiet_end: digest.quiet_end.format("%H:%M").to_string(),
        paused_until: digest.paused_until.filter(|until| *until > Utc::now()),
        next_digest_at,
        email_status: match (&settings.email, settings.email_confirmed_at) {
            (None, _) => "none",
            (Some(_), None) => "unconfirmed",
            (Some(_), Some(_)) => "confirmed",
        }
        .to_owned(),
        email: settings.email,
        confirmation_sent_at: settings.confirmation_sent_at,
        max_items: state.config().notify.max_items,
        recent: recent
            .into_iter()
            .map(|d| DeliveryView {
                id: d.id,
                kind: d.kind.as_str().to_owned(),
                status: d.status,
                opportunities: d.items,
                created_at: d.created_at,
                sent_at: d.sent_at,
            })
            .collect(),
    })
}

async fn notifications(
    State(state): State<ApiState>,
    axum::Extension(principal): Auth,
) -> Result<Json<NotificationSettingsView>, ApiError> {
    Ok(Json(notification_view(&state, &principal).await?))
}

/// Sends a confirmation link to the account's unconfirmed address.
async fn send_confirmation(
    state: &ApiState,
    principal: &Principal,
    email: &str,
) -> Result<(), ApiError> {
    let (Some(sender), Some(config), Some(web)) = (
        state.inner.email.as_ref(),
        state.config().email.as_ref(),
        state.config().web_url.as_ref(),
    ) else {
        return Err(unavailable_email());
    };
    let now = Utc::now();
    let token = new_confirmation_token();
    state
        .store()
        .set_confirmation_token(&principal.user, &token, now)
        .await?;
    let message =
        crate::notify::render_confirmation(crate::email::from_address(config), email, web, &token);
    let delivery = NewDelivery {
        id: new_delivery_id(),
        user: principal.user.clone(),
        kind: DeliveryKind::Confirmation,
        message: serde_json::to_vec(&message).map_err(|_| ApiError::internal())?,
        items: Vec::new(),
        expires_at: None,
    };
    state.store().enqueue_delivery(&delivery, now).await?;
    // Sent now; if the provider fails, the notification worker retries it.
    let owner = format!("api:{}", state.config().instance);
    let mut summary = crate::notify::NotifySummary::default();
    if let Err(e) = crate::notify::deliver_due(
        state.store(),
        sender.as_ref(),
        &owner,
        Some(&principal.user),
        &mut summary,
    )
    .await
    {
        tracing::warn!(error = %e.public_message(), "could not send a confirmation now");
    }
    Ok(())
}

fn unavailable_email() -> ApiError {
    ApiError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "email_unavailable",
        "This Narrow server cannot send email.",
    )
}

async fn update_notifications(
    State(state): State<ApiState>,
    axum::Extension(principal): Auth,
    headers: http::HeaderMap,
    Body(request): Body<UpdateNotificationsRequest>,
) -> Result<Json<NotificationSettingsView>, ApiError> {
    let cadence = match request.cadence.as_deref() {
        None => None,
        Some(c) => Some(
            Cadence::parse(c)
                .ok_or_else(|| ApiError::invalid("cadence must be \"immediate\" or \"daily\""))?,
        ),
    };
    let schedule = digest_change(&state, &principal, &request).await?;
    let email = match request.email.as_deref().map(str::trim) {
        None => None,
        Some(e) if valid_email(e) => Some(e.to_owned()),
        Some(_) => return Err(ApiError::invalid("that is not an email address")),
    };
    let available = state.inner.email.is_some() && state.config().web_url.is_some();
    if !available && (email.is_some() || request.email_enabled == Some(true)) {
        return Err(unavailable_email());
    }
    let change = SettingsChange {
        email_enabled: request.email_enabled,
        cadence,
        email,
        ..schedule
    };
    let (settings, email_changed) = state
        .store()
        .update_notification_settings(&principal.user, &change, Utc::now())
        .await?;
    let unconfirmed = settings.email_confirmed_at.is_none();
    if let Some(address) = settings.email.as_deref()
        && unconfirmed
        && (email_changed || request.resend_confirmation)
    {
        let recently = settings
            .confirmation_sent_at
            .is_some_and(|at| Utc::now() - at < chrono::Duration::seconds(60));
        if email_changed || !recently {
            send_confirmation(&state, &principal, address).await?;
        }
    }
    state.usage().record(
        Some(&principal.user),
        "notifications_updated",
        serde_json::json!({
            "client": client_of(&headers),
            "email_enabled": settings.email_enabled,
            "cadence": settings.cadence.as_str(),
            "email_changed": email_changed,
            "digest_size": settings.digest.size,
            "digest_days": settings.digest.days.count_ones(),
            "timezone_set": settings.digest.timezone.is_some(),
            "paused": settings.digest.paused_until.is_some_and(|p| p > Utc::now()),
        }),
    );
    Ok(Json(notification_view(&state, &principal).await?))
}

/// The digest schedule a request changes, validated as a whole against the
/// current settings (a digest time inside quiet hours is refused, not
/// moved).
async fn digest_change(
    state: &ApiState,
    principal: &Principal,
    request: &UpdateNotificationsRequest,
) -> Result<SettingsChange, ApiError> {
    let time = |field: &str, value: &Option<String>| -> Result<Option<NaiveTime>, ApiError> {
        value
            .as_deref()
            .map(|v| {
                NaiveTime::parse_from_str(v.trim(), "%H:%M")
                    .map_err(|_| ApiError::invalid(format!("{field} must be a time like 09:00")))
            })
            .transpose()
    };
    let timezone = match request.timezone.as_deref().map(str::trim) {
        None => None,
        Some(name) => {
            name.parse::<chrono_tz::Tz>().map_err(|_| {
                ApiError::invalid("timezone must be an IANA name like America/Sao_Paulo")
            })?;
            Some(name.to_owned())
        }
    };
    let digest_days = request
        .digest_days
        .as_deref()
        .map(crate::digest::days_of)
        .transpose()
        .map_err(|day| ApiError::invalid(format!("{day:?} is not a day (mon … sun)")))?;
    let max = state.config().notify.max_items;
    if let Some(size) = request.digest_size
        && !(1..=max).contains(&usize::from(size))
    {
        return Err(ApiError::invalid(format!(
            "digest_size must be between 1 and {max}"
        )));
    }
    let now = Utc::now();
    if let Some(Some(until)) = request.paused_until
        && until > now + chrono::Duration::days(366)
    {
        return Err(ApiError::invalid("a pause can last a year at most"));
    }
    let change = SettingsChange {
        timezone,
        digest_time: time("digest_time", &request.digest_time)?,
        digest_days,
        digest_size: request.digest_size,
        quiet_start: time("quiet_start", &request.quiet_start)?,
        quiet_end: time("quiet_end", &request.quiet_end)?,
        paused_until: request.paused_until,
        ..SettingsChange::default()
    };
    let current = state
        .store()
        .notification_settings(&principal.user)
        .await?
        .digest;
    let merged = DigestSettings {
        timezone: change.timezone.clone().or(current.timezone),
        time: change.digest_time.unwrap_or(current.time),
        days: change.digest_days.unwrap_or(current.days),
        size: change.digest_size.unwrap_or(current.size),
        quiet_start: change.quiet_start.unwrap_or(current.quiet_start),
        quiet_end: change.quiet_end.unwrap_or(current.quiet_end),
        paused_until: change.paused_until.unwrap_or(current.paused_until),
    };
    crate::digest::Schedule::validate(&merged).map_err(ApiError::invalid)?;
    Ok(change)
}

async fn confirm_notifications(
    State(state): State<ApiState>,
    axum::Extension(principal): Auth,
    Body(request): Body<ConfirmEmailRequest>,
) -> Result<Json<NotificationSettingsView>, ApiError> {
    let now = Utc::now();
    let confirmed = state
        .store()
        .confirm_email(
            &principal.user,
            request.token.trim(),
            now - chrono::Duration::hours(48),
            now,
        )
        .await?;
    if !confirmed {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_confirmation",
            "This confirmation link is not valid for this account, or it expired.",
        )
        .with_hint("Send a new link from the notification settings."));
    }
    state.usage().record(
        Some(&principal.user),
        "email_confirmed",
        serde_json::json!({}),
    );
    Ok(Json(notification_view(&state, &principal).await?))
}

async fn not_found() -> ApiError {
    ApiError::not_found("route")
}

/// The whole service.
pub fn router(state: ApiState) -> Router {
    let protected = Router::new()
        .route("/api/v1/account", get(account).delete(delete_account))
        .route("/api/v1/account/logout", post(logout))
        .route("/api/v1/tokens", get(list_tokens).post(create_token))
        .route("/api/v1/tokens/{id}", delete(revoke_token))
        .route("/api/v1/sync/pull", post(sync_pull))
        .route("/api/v1/sync/push", post(sync_push))
        .route("/api/v1/search", post(search))
        .route("/api/v1/opportunities/{id}", get(opportunity))
        .route("/api/v1/opportunities/{id}/verify", post(verify))
        .route("/api/v1/opportunities/{id}/feedback", post(feedback))
        .route(
            "/api/v1/opportunities/{id}/application-context",
            get(application_context),
        )
        .route("/api/v1/profile", get(profile))
        .route("/api/v1/preferences", post(preferences))
        .route("/api/v1/pipeline", get(pipeline))
        .route("/api/v1/export", get(export))
        .route("/api/v1/feed", get(feed))
        .route("/api/v1/opportunities/{id}/dismiss", post(dismiss))
        .route("/api/v1/taste", get(taste))
        .route(
            "/api/v1/taste/profile",
            get(taste_profile).post(update_taste_profile),
        )
        .route("/api/v1/profile/claims", get(claims).post(decide_claims))
        .route("/api/v1/profile/resume", axum::routing::put(import_resume))
        .route(
            "/api/v1/profile/linkedin",
            axum::routing::put(import_linkedin),
        )
        .route("/api/v1/profile/github", post(import_github))
        .route("/api/v1/profile/sources/{source}", delete(remove_source))
        .route(
            "/api/v1/notifications",
            get(notifications).put(update_notifications),
        )
        .route("/api/v1/notifications/confirm", post(confirm_notifications))
        .nest_service("/mcp", crate::mcp::service(state.clone()))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            authenticate,
        ));
    let public = Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready))
        .route(
            "/.well-known/oauth-protected-resource",
            get(protected_resource),
        )
        .route(
            "/.well-known/oauth-protected-resource/mcp",
            get(protected_resource),
        )
        .route("/api/v1/auth/config", get(auth_config))
        .route("/api/v1/auth/dev-token", post(dev_token));
    let cors = if state.config().allowed_origins.is_empty() {
        CorsLayer::new()
    } else {
        let origins: Vec<http::HeaderValue> = state
            .config()
            .allowed_origins
            .iter()
            .filter_map(|o| o.parse().ok())
            .collect();
        CorsLayer::new()
            .allow_origin(AllowOrigin::list(origins))
            .allow_methods([
                http::Method::GET,
                http::Method::POST,
                http::Method::PUT,
                http::Method::DELETE,
            ])
            .allow_headers([
                http::header::AUTHORIZATION,
                http::header::CONTENT_TYPE,
                http::HeaderName::from_static("x-jobhunt-client"),
            ])
    };
    public
        .merge(protected)
        .fallback(not_found)
        .with_state(state)
        .layer(RequestBodyLimitLayer::new(16 * 1024 * 1024))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::GATEWAY_TIMEOUT,
            Duration::from_secs(120),
        ))
        .layer(cors)
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(make_span)
                .on_request(())
                .on_response(on_response),
        )
        .layer(PropagateRequestIdLayer::new(REQUEST_ID))
        .layer(SetRequestIdLayer::new(REQUEST_ID, RandomRequestId))
        .layer(SetSensitiveRequestHeadersLayer::new([
            http::header::AUTHORIZATION,
            http::header::COOKIE,
        ]))
}
