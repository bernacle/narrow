//! Cloud configuration: environment variables (Railway variables in
//! production), validated at startup.
//!
//! The job-board sources and the product settings (verification freshness,
//! discovery concurrency, …) use the same TOML format as the local product
//! (`JOBHUNT_CONFIG`, e.g. `deploy/cloud.toml`); environment variables
//! override the file. Everything specific to running a service (database,
//! keys, identity provider, schedule, pool) comes from the environment
//! only. Secrets are never printed: [`CloudConfig::report`] says whether
//! each one is set, not what it is.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use jobhunt_app::config::{AppConfig, LogFormat};
use jobhunt_app::{LoadedConfig, config};
use jobhunt_storage::postgres::{CryptoError, Keyring, PgSettings, ScheduleSettings};
use url::Url;

/// Reads one variable (the process environment, or a map in tests).
pub type Env<'a> = &'a dyn Fn(&str) -> Option<String>;

/// The process environment.
pub fn process_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

/// How the service authenticates people.
#[derive(Clone, PartialEq, Eq)]
pub enum AuthConfig {
    /// A standards-based OpenID Connect / OAuth 2 provider (WorkOS AuthKit,
    /// Auth0, Okta, Zitadel, Keycloak, …): access tokens are JWTs verified
    /// against the issuer's published keys; the CLI signs in with the
    /// device authorization grant.
    Oidc(OidcSettings),
    /// Local development and tests only: HS256 tokens minted by the server
    /// itself. Refused in production.
    Dev { secret: String },
}

/// The identity provider, and what its tokens must say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OidcSettings {
    /// The issuer identifier, exactly as in the tokens' `iss` (no
    /// normalization: `https://x.authkit.app` and `https://x.authkit.app/`
    /// are different issuers).
    pub issuer: String,
    /// The accepted `aud` values (`JOBHUNT_OIDC_AUDIENCE`, comma separated);
    /// the first is the one the CLI asks for.
    pub audiences: Vec<String>,
    /// The public (no secret) client the CLI uses for the device flow.
    pub cli_client_id: Option<String>,
    pub scopes: String,
    /// How the CLI names the audience it asks for.
    pub audience_parameter: AudienceParameter,
    /// Endpoints set by hand, for providers that do not publish them in
    /// their metadata. With a JWKS URL set, discovery is skipped.
    pub endpoints: ProviderEndpoints,
}

/// The request parameter that names the audience a token is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudienceParameter {
    /// `resource` (RFC 8707 resource indicators: WorkOS AuthKit, Okta, …).
    Resource,
    /// `audience` (Auth0).
    Audience,
    /// Not sent: the provider decides (e.g. from a token template).
    None,
}

impl AudienceParameter {
    /// The form field, if any.
    pub fn field(self) -> Option<&'static str> {
        match self {
            Self::Resource => Some("resource"),
            Self::Audience => Some("audience"),
            Self::None => None,
        }
    }
}

/// Provider endpoints configured by hand (each overrides the metadata).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderEndpoints {
    pub jwks_uri: Option<String>,
    pub device_authorization: Option<String>,
    pub token: Option<String>,
    pub revocation: Option<String>,
}

impl std::fmt::Debug for AuthConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Oidc(settings) => f
                .debug_struct("Oidc")
                .field("issuer", &settings.issuer)
                .field("audiences", &settings.audiences)
                .finish_non_exhaustive(),
            Self::Dev { .. } => f.write_str("Dev"),
        }
    }
}

/// Where transactional email goes.
#[derive(Clone, PartialEq, Eq)]
pub enum EmailConfig {
    /// Resend's HTTP API (<https://resend.com/docs/api-reference>).
    Resend {
        api_key: String,
        /// `JobHunt <notifications@example.com>`: a sender on a domain
        /// verified in Resend.
        from: String,
        /// The API's base URL (tests point it at a local server).
        api_url: String,
    },
    /// Every message appended as a JSON line to a file: development and
    /// end-to-end tests only (refused in production).
    File { path: PathBuf, from: String },
}

impl std::fmt::Debug for EmailConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Resend { from, .. } => f
                .debug_struct("Resend")
                .field("from", from)
                .finish_non_exhaustive(),
            Self::File { path, .. } => f.debug_struct("File").field("path", path).finish(),
        }
    }
}

impl EmailConfig {
    /// The provider's name, for reports and delivery records.
    pub fn provider(&self) -> &'static str {
        match self {
            Self::Resend { .. } => "resend",
            Self::File { .. } => "file",
        }
    }
}

/// How the notification worker behaves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotifySettings {
    /// With the `immediate` cadence, at most one recommendations email per
    /// account this often.
    pub min_interval: Duration,
    /// Opportunities in one email at most.
    pub max_items: usize,
    /// Accounts a worker claims at a time.
    pub batch: usize,
}

impl Default for NotifySettings {
    fn default() -> Self {
        Self {
            min_interval: Duration::from_secs(4 * 3600),
            max_items: 3,
            batch: 50,
        }
    }
}

/// Everything the cloud processes need.
#[derive(Clone)]
pub struct CloudConfig {
    /// `production`, `staging`, `development`, … (`JOBHUNT_ENV`, else
    /// Railway's `RAILWAY_ENVIRONMENT_NAME`).
    pub environment: String,
    database_url: Option<String>,
    encryption_keys: Option<String>,
    /// Where clients reach the service (`https://api.example.com`).
    pub public_url: Option<Url>,
    pub bind: SocketAddr,
    pub auth: Option<AuthConfig>,
    pub db: PgSettings,
    /// Apply migrations when the server starts (they are also applied by
    /// `narrow migrate`, Railway's pre-deploy command).
    pub migrate_on_start: bool,
    /// Browser origins allowed to call the API (BRU-295's web app).
    pub allowed_origins: Vec<String>,
    /// Where people use JobHunt in a browser (`https://app.example.com`):
    /// the links in emails.
    pub web_url: Option<Url>,
    /// Transactional email (notifications, address confirmation); `None`
    /// means email is not available.
    pub email: Option<EmailConfig>,
    pub notify: NotifySettings,
    pub schedule: ScheduleSettings,
    /// Sources a discovery worker claims per run.
    pub discovery_batch: usize,
    /// Jobs a verification worker verifies per run.
    pub verification_batch: usize,
    /// This process, in leases and worker runs.
    pub instance: String,
    /// Record usage events.
    pub usage_events: bool,
    /// A GitHub token (no scopes) for importing people's public GitHub
    /// evidence at the authenticated rate limit (`JOBHUNT_GITHUB_TOKEN`);
    /// without it imports share GitHub's unauthenticated limit. Secret.
    pub github_token: Option<String>,
    /// Where GitHub's API is (`JOBHUNT_GITHUB_ENDPOINT`): a test hook for a
    /// local server, not a deployment setting.
    pub github_api: Option<String>,
    /// The product configuration (sources, verification policy, …).
    pub app: Arc<LoadedConfig>,
    /// The model that reads people's descriptions of what they want into
    /// their taste profile (`JOBHUNT_AI_PROVIDER`, `_MODEL`, `_BASE_URL`,
    /// `_API_KEY`, `_TIMEOUT_SECS`): `Ok(None)` when none is configured,
    /// `Err` when the configuration can't be used. Either way the built-in
    /// reader works; this never stops the server.
    pub ai: Result<Option<jobhunt_ai::ModelConfig>, String>,
    /// The model that reviews the fit of each ranking's shortlist
    /// (`JOBHUNT_AI_REVIEW_FIT=true`, optionally `JOBHUNT_AI_REVIEW_MODEL`;
    /// the rest as `JOBHUNT_AI_*`): `Ok(None)` when off (the default). A
    /// configuration that can't be used never stops the server: ranking
    /// relies on its rules.
    pub fit_review: Result<Option<jobhunt_ai::ModelConfig>, String>,
    problems: Vec<String>,
}

/// The fit reviewer a configuration names, ready to share between
/// requests (see [`CloudConfig::fit_review`]).
pub fn fit_review(config: &CloudConfig) -> jobhunt_app::FitReview {
    match &config.fit_review {
        Ok(Some(model)) => match jobhunt_ai::ModelFitReviewer::new(model.clone()) {
            Ok(reviewer) => jobhunt_app::FitReview {
                reviewer: Some(std::sync::Arc::new(reviewer)),
                ..jobhunt_app::FitReview::default()
            },
            Err(e) => jobhunt_app::FitReview {
                problem: Some(e.to_string()),
                ..jobhunt_app::FitReview::default()
            },
        },
        Ok(None) => jobhunt_app::FitReview::default(),
        Err(problem) => {
            tracing::warn!(%problem, "JOBHUNT_AI_REVIEW_FIT can't be used; ranking relies on its rules");
            jobhunt_app::FitReview {
                problem: Some(problem.clone()),
                ..jobhunt_app::FitReview::default()
            }
        }
    }
}

/// `JOBHUNT_AI_REVIEW_FIT` and `JOBHUNT_AI_REVIEW_MODEL`, over
/// `JOBHUNT_AI_*`.
fn fit_review_settings(env: Env<'_>) -> Result<Option<jobhunt_ai::ModelConfig>, String> {
    let on = match env("JOBHUNT_AI_REVIEW_FIT").map(|v| v.trim().to_ascii_lowercase()) {
        None => false,
        Some(v) if matches!(v.as_str(), "" | "0" | "false" | "no" | "off") => false,
        Some(v) if matches!(v.as_str(), "1" | "true" | "yes" | "on") => true,
        Some(v) => return Err(format!("JOBHUNT_AI_REVIEW_FIT is {v:?} (true or false)")),
    };
    if !on {
        return Ok(None);
    }
    let model = env("JOBHUNT_AI_REVIEW_MODEL").or_else(|| env("JOBHUNT_AI_MODEL"));
    match model_settings(env, model.as_deref())? {
        Some(config) => Ok(Some(config)),
        None => Err("JOBHUNT_AI_REVIEW_FIT needs JOBHUNT_AI_PROVIDER".into()),
    }
}

/// `JOBHUNT_AI_*`.
fn ai_settings(env: Env<'_>) -> Result<Option<jobhunt_ai::ModelConfig>, String> {
    model_settings(env, env("JOBHUNT_AI_MODEL").as_deref())
}

fn model_settings(
    env: Env<'_>,
    model: Option<&str>,
) -> Result<Option<jobhunt_ai::ModelConfig>, String> {
    let Some(provider) = env("JOBHUNT_AI_PROVIDER").filter(|p| !p.eq_ignore_ascii_case("none"))
    else {
        return Ok(None);
    };
    let mut config = jobhunt_ai::ModelConfig::new(
        &provider,
        model,
        env("JOBHUNT_AI_BASE_URL").as_deref(),
        env("JOBHUNT_AI_API_KEY"),
        "JOBHUNT_AI_API_KEY",
    )
    .map_err(|e| e.to_string())?;
    if let Some(raw) = env("JOBHUNT_AI_TIMEOUT_SECS") {
        let secs: u64 = raw
            .trim()
            .parse()
            .map_err(|_| "JOBHUNT_AI_TIMEOUT_SECS is not a number of seconds".to_owned())?;
        config.timeout = std::time::Duration::from_secs(secs.max(1));
    }
    Ok(Some(config))
}

impl std::fmt::Debug for CloudConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CloudConfig")
            .field("environment", &self.environment)
            .field("public_url", &self.public_url.as_ref().map(Url::as_str))
            .field("bind", &self.bind)
            .field("auth", &self.auth)
            .field("github_token", &self.github_token.as_ref().map(|_| "set"))
            .field("ai", &self.ai)
            .finish_non_exhaustive()
    }
}

/// Which process is starting (each needs different settings).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Server,
    Worker,
    /// The notification worker: reads private data and sends email.
    Notifier,
    Migrate,
}

/// One line of `narrow doctor`'s cloud section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setting {
    pub name: &'static str,
    /// `set`, `missing`, `invalid: …` or a non-secret value.
    pub status: String,
    pub required: bool,
}

fn parse<T: std::str::FromStr>(
    env: Env<'_>,
    name: &str,
    default: T,
    problems: &mut Vec<String>,
) -> T {
    match env(name) {
        None => default,
        Some(raw) => raw.trim().parse().unwrap_or_else(|_| {
            problems.push(format!("{name} is not a valid value"));
            default
        }),
    }
}

/// The identity provider settings (`JOBHUNT_OIDC_*`).
fn oidc_settings(
    env: Env<'_>,
    issuer: String,
    audience: &str,
    problems: &mut Vec<String>,
) -> Option<AuthConfig> {
    let issuer = issuer.trim().to_owned();
    if Url::parse(&issuer).is_err() {
        problems.push("JOBHUNT_OIDC_ISSUER is not a URL".into());
        return None;
    }
    let audiences: Vec<String> = audience
        .split(',')
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .map(str::to_owned)
        .collect();
    if audiences.is_empty() {
        problems.push("JOBHUNT_OIDC_AUDIENCE is empty".into());
        return None;
    }
    let audience_parameter = match env("JOBHUNT_OIDC_AUDIENCE_PARAMETER")
        .as_deref()
        .map(str::trim)
    {
        None | Some("resource") => AudienceParameter::Resource,
        Some("audience") => AudienceParameter::Audience,
        Some("none") => AudienceParameter::None,
        Some(other) => {
            problems.push(format!(
                "JOBHUNT_OIDC_AUDIENCE_PARAMETER must be \"resource\", \"audience\" or \
                 \"none\", not {other:?}"
            ));
            AudienceParameter::Resource
        }
    };
    let mut url = |name: &str| {
        let value = env(name)?.trim().to_owned();
        if Url::parse(&value).is_err() {
            problems.push(format!("{name} is not a URL"));
            return None;
        }
        Some(value)
    };
    let endpoints = ProviderEndpoints {
        jwks_uri: url("JOBHUNT_OIDC_JWKS_URL"),
        device_authorization: url("JOBHUNT_OIDC_DEVICE_AUTHORIZATION_URL"),
        token: url("JOBHUNT_OIDC_TOKEN_URL"),
        revocation: url("JOBHUNT_OIDC_REVOCATION_URL"),
    };
    Some(AuthConfig::Oidc(OidcSettings {
        issuer,
        audiences,
        cli_client_id: env("JOBHUNT_OIDC_CLI_CLIENT_ID"),
        scopes: env("JOBHUNT_OIDC_SCOPES").unwrap_or_else(|| "openid offline_access".into()),
        audience_parameter,
        endpoints,
    }))
}

fn hours(value: u64) -> Duration {
    Duration::from_secs(value.saturating_mul(3600))
}

impl CloudConfig {
    /// Reads the configuration. Problems are collected, not fatal here:
    /// [`CloudConfig::require`] checks what a role needs.
    pub fn from_env(env: Env<'_>) -> Self {
        let mut problems = Vec::new();
        let environment = env("JOBHUNT_ENV")
            .or_else(|| env("RAILWAY_ENVIRONMENT_NAME"))
            .unwrap_or_else(|| "development".to_owned());
        let public_url = match env("JOBHUNT_PUBLIC_URL")
            .or_else(|| env("RAILWAY_PUBLIC_DOMAIN").map(|d| format!("https://{d}")))
        {
            Some(raw) => match Url::parse(raw.trim()) {
                Ok(url) if matches!(url.scheme(), "http" | "https") => Some(url),
                _ => {
                    problems.push("JOBHUNT_PUBLIC_URL is not an http(s) URL".into());
                    None
                }
            },
            None => None,
        };
        let port: u16 = parse(env, "PORT", 8080, &mut problems);
        let bind = match env("JOBHUNT_BIND") {
            Some(raw) => raw.parse().unwrap_or_else(|_| {
                problems.push("JOBHUNT_BIND is not an address (e.g. [::]:8080)".into());
                SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 0], port))
            }),
            // Dual stack: Railway's private network may be IPv6 only.
            None => SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 0], port)),
        };
        let auth = match env("JOBHUNT_AUTH_MODE").as_deref().unwrap_or("oidc") {
            "dev" => match env("JOBHUNT_AUTH_DEV_SECRET") {
                Some(secret) if secret.len() >= 32 => Some(AuthConfig::Dev { secret }),
                _ => {
                    problems.push(
                        "JOBHUNT_AUTH_MODE=dev needs JOBHUNT_AUTH_DEV_SECRET (32+ characters)"
                            .into(),
                    );
                    None
                }
            },
            "oidc" => match (env("JOBHUNT_OIDC_ISSUER"), env("JOBHUNT_OIDC_AUDIENCE")) {
                (Some(issuer), Some(audience)) => {
                    oidc_settings(env, issuer, &audience, &mut problems)
                }
                _ => None,
            },
            other => {
                problems.push(format!(
                    "JOBHUNT_AUTH_MODE must be \"oidc\" or \"dev\", not {other:?}"
                ));
                None
            }
        };
        let defaults = PgSettings::default();
        let db = PgSettings {
            max_connections: parse(
                env,
                "JOBHUNT_DB_MAX_CONNECTIONS",
                defaults.max_connections,
                &mut problems,
            ),
            min_connections: 0,
            acquire_timeout: Duration::from_secs(parse(
                env,
                "JOBHUNT_DB_ACQUIRE_TIMEOUT_SECS",
                defaults.acquire_timeout.as_secs(),
                &mut problems,
            )),
            idle_timeout: defaults.idle_timeout,
            statement_timeout: Duration::from_secs(parse(
                env,
                "JOBHUNT_DB_STATEMENT_TIMEOUT_SECS",
                defaults.statement_timeout.as_secs(),
                &mut problems,
            )),
        };
        let schedule_defaults = ScheduleSettings::default();
        let schedule = ScheduleSettings {
            active_every: hours(parse(
                env,
                "JOBHUNT_DISCOVERY_ACTIVE_HOURS",
                schedule_defaults.active_every.as_secs() / 3600,
                &mut problems,
            )),
            normal_every: hours(parse(
                env,
                "JOBHUNT_DISCOVERY_NORMAL_HOURS",
                schedule_defaults.normal_every.as_secs() / 3600,
                &mut problems,
            )),
            max_backoff: hours(parse(
                env,
                "JOBHUNT_DISCOVERY_MAX_BACKOFF_HOURS",
                schedule_defaults.max_backoff.as_secs() / 3600,
                &mut problems,
            )),
            ..schedule_defaults
        };
        let web_url = match env("JOBHUNT_WEB_URL") {
            Some(raw) => match Url::parse(raw.trim()) {
                Ok(url) if matches!(url.scheme(), "http" | "https") => Some(url),
                _ => {
                    problems.push("JOBHUNT_WEB_URL is not an http(s) URL".into());
                    None
                }
            },
            None => None,
        };
        let from = env("JOBHUNT_EMAIL_FROM");
        let email = match env("JOBHUNT_EMAIL_PROVIDER").as_deref().map(str::trim) {
            None | Some("none") => None,
            Some("resend") => match (env("JOBHUNT_RESEND_API_KEY"), from.clone()) {
                (Some(api_key), Some(from)) => Some(EmailConfig::Resend {
                    api_key,
                    from,
                    api_url: env("JOBHUNT_RESEND_API_URL")
                        .unwrap_or_else(|| "https://api.resend.com".into()),
                }),
                _ => {
                    problems.push(
                        "JOBHUNT_EMAIL_PROVIDER=resend needs JOBHUNT_RESEND_API_KEY and \
                         JOBHUNT_EMAIL_FROM"
                            .into(),
                    );
                    None
                }
            },
            Some("file") => match env("JOBHUNT_EMAIL_FILE") {
                Some(path) => Some(EmailConfig::File {
                    path: PathBuf::from(path),
                    from: from.unwrap_or_else(|| "JobHunt <notifications@jobhunt.test>".into()),
                }),
                None => {
                    problems.push("JOBHUNT_EMAIL_PROVIDER=file needs JOBHUNT_EMAIL_FILE".into());
                    None
                }
            },
            Some(other) => {
                problems.push(format!(
                    "JOBHUNT_EMAIL_PROVIDER must be \"resend\", \"file\" or \"none\", not {other:?}"
                ));
                None
            }
        };
        let notify_defaults = NotifySettings::default();
        let notify = NotifySettings {
            min_interval: hours(parse(
                env,
                "JOBHUNT_NOTIFY_MIN_INTERVAL_HOURS",
                notify_defaults.min_interval.as_secs() / 3600,
                &mut problems,
            )),
            max_items: parse(
                env,
                "JOBHUNT_NOTIFY_MAX_ITEMS",
                notify_defaults.max_items,
                &mut problems,
            )
            .clamp(1, 5),
            batch: parse(
                env,
                "JOBHUNT_NOTIFY_BATCH",
                notify_defaults.batch,
                &mut problems,
            )
            .max(1),
        };
        let instance = env("RAILWAY_REPLICA_ID")
            .or_else(|| env("HOSTNAME"))
            .map_or_else(
                || format!("pid-{}", std::process::id()),
                |host| format!("{host}-{}", std::process::id()),
            );
        let app = match load_app(env, &mut problems) {
            Some(app) => app,
            None => LoadedConfig {
                config: AppConfig::default(),
                file: None,
                default_file: None,
                database: PathBuf::from("(cloud)"),
            },
        };
        Self {
            environment,
            database_url: env("DATABASE_URL"),
            encryption_keys: env("JOBHUNT_ENCRYPTION_KEYS"),
            public_url,
            bind,
            auth,
            db,
            migrate_on_start: parse(env, "JOBHUNT_MIGRATE_ON_START", true, &mut problems),
            allowed_origins: env("JOBHUNT_ALLOWED_ORIGINS")
                .map(|v| {
                    v.split(',')
                        .map(|o| o.trim().trim_end_matches('/').to_owned())
                        .filter(|o| !o.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
            web_url,
            email,
            notify,
            schedule,
            discovery_batch: parse(env, "JOBHUNT_DISCOVERY_BATCH", 25, &mut problems),
            verification_batch: parse(env, "JOBHUNT_VERIFY_BATCH", 100, &mut problems),
            instance,
            usage_events: parse(env, "JOBHUNT_USAGE_EVENTS", true, &mut problems),
            github_token: env("JOBHUNT_GITHUB_TOKEN"),
            github_api: env("JOBHUNT_GITHUB_ENDPOINT"),
            app: Arc::new(app),
            ai: ai_settings(env),
            fit_review: fit_review_settings(env),
            problems,
        }
    }

    /// Whether this is a production environment.
    pub fn is_production(&self) -> bool {
        self.environment.eq_ignore_ascii_case("production")
    }

    /// Development fixture endpoint; production always uses GitHub's API.
    pub fn github_endpoint(&self) -> Option<&str> {
        (!self.is_production())
            .then_some(self.github_api.as_deref())
            .flatten()
    }

    /// The database URL (a secret: never log it).
    pub fn database_url(&self) -> Option<&str> {
        self.database_url.as_deref()
    }

    /// The encryption keys, parsed. Workers and migrations that never touch
    /// private data may run without them (a random key is used).
    pub fn keyring(&self) -> Result<Keyring, CryptoError> {
        match &self.encryption_keys {
            Some(spec) => Keyring::parse(spec),
            None => Err(CryptoError::NoKeys),
        }
    }

    /// Everything wrong for `role`, as sentences. Empty means ready.
    pub fn problems(&self, role: Role) -> Vec<String> {
        let mut out = self.problems.clone();
        if self.is_production() && self.github_api.is_some() {
            out.push("JOBHUNT_GITHUB_ENDPOINT is refused in production; GitHub imports use the official HTTPS API".into());
        }
        if self.database_url.is_none() {
            out.push("DATABASE_URL is not set (use ${{Postgres.DATABASE_URL}} on Railway)".into());
        }
        if role == Role::Server {
            match self.keyring() {
                Ok(_) => {}
                Err(CryptoError::NoKeys) => out.push(
                    "JOBHUNT_ENCRYPTION_KEYS is not set (<id>:<base64 of 32 random bytes>)".into(),
                ),
                Err(e) => out.push(format!("JOBHUNT_ENCRYPTION_KEYS: {e}")),
            }
            match &self.auth {
                None if !self.problems.iter().any(|p| p.contains("JOBHUNT_AUTH")) => out.push(
                    "authentication is not configured: set JOBHUNT_OIDC_ISSUER and \
                     JOBHUNT_OIDC_AUDIENCE (or JOBHUNT_AUTH_MODE=dev outside production)"
                        .into(),
                ),
                Some(AuthConfig::Dev { .. }) if self.is_production() => out.push(
                    "JOBHUNT_AUTH_MODE=dev is refused in production: configure an identity \
                     provider"
                        .into(),
                ),
                _ => {}
            }
            if self.public_url.is_none() && self.is_production() {
                out.push("JOBHUNT_PUBLIC_URL is not set (the service's https URL)".into());
            }
        }
        if role == Role::Notifier {
            match self.keyring() {
                Ok(_) => {}
                Err(CryptoError::NoKeys) => out.push(
                    "JOBHUNT_ENCRYPTION_KEYS is not set (the notification worker reads private \
                     data)"
                        .into(),
                ),
                Err(e) => out.push(format!("JOBHUNT_ENCRYPTION_KEYS: {e}")),
            }
            if self.email.is_none() && !self.problems.iter().any(|p| p.contains("EMAIL")) {
                out.push(
                    "email is not configured: set JOBHUNT_EMAIL_PROVIDER (resend) with \
                     JOBHUNT_RESEND_API_KEY and JOBHUNT_EMAIL_FROM"
                        .into(),
                );
            }
            if self.web_url.is_none() {
                out.push("JOBHUNT_WEB_URL is not set (the links in emails)".into());
            }
        }
        if matches!(self.email, Some(EmailConfig::File { .. })) && self.is_production() {
            out.push("JOBHUNT_EMAIL_PROVIDER=file is refused in production".into());
        }
        out
    }

    /// Fails with every problem for `role`, or returns the configuration.
    pub fn require(self, role: Role) -> Result<Self, ConfigProblems> {
        let problems = self.problems(role);
        if problems.is_empty() {
            Ok(self)
        } else {
            Err(ConfigProblems(problems))
        }
    }

    /// What is configured, for `narrow doctor`, without secret values.
    pub fn report(&self) -> Vec<Setting> {
        let secret = |present: bool| if present { "set" } else { "missing" }.to_owned();
        let mut out = vec![
            Setting {
                name: "JOBHUNT_ENV",
                status: self.environment.clone(),
                required: false,
            },
            Setting {
                name: "DATABASE_URL",
                status: secret(self.database_url.is_some()),
                required: true,
            },
            Setting {
                name: "JOBHUNT_ENCRYPTION_KEYS",
                status: match self.keyring() {
                    Ok(k) => format!(
                        "set ({} keys, active {:?})",
                        k.key_ids().len(),
                        k.active_key()
                    ),
                    Err(CryptoError::NoKeys) => "missing".into(),
                    Err(e) => format!("invalid: {e}"),
                },
                required: true,
            },
            Setting {
                name: "JOBHUNT_PUBLIC_URL",
                status: self
                    .public_url
                    .as_ref()
                    .map_or_else(|| "missing".into(), ToString::to_string),
                required: self.is_production(),
            },
        ];
        out.push(match &self.auth {
            Some(AuthConfig::Oidc(settings)) => Setting {
                name: "JOBHUNT_OIDC_ISSUER / _AUDIENCE / _CLI_CLIENT_ID",
                status: format!(
                    "{} / {} / {}",
                    settings.issuer,
                    settings.audiences.join(","),
                    if settings.cli_client_id.is_some() {
                        "set"
                    } else {
                        "missing (CLI login unavailable)"
                    }
                ),
                required: true,
            },
            Some(AuthConfig::Dev { .. }) => Setting {
                name: "JOBHUNT_AUTH_MODE",
                status: "dev (development only)".into(),
                required: true,
            },
            None => Setting {
                name: "JOBHUNT_OIDC_ISSUER / _AUDIENCE",
                status: "missing".into(),
                required: true,
            },
        });
        out.push(Setting {
            name: "JOBHUNT_WEB_URL",
            status: self
                .web_url
                .as_ref()
                .map_or_else(|| "missing".into(), ToString::to_string),
            required: false,
        });
        out.push(Setting {
            name: "JOBHUNT_EMAIL_PROVIDER",
            status: match &self.email {
                Some(e @ EmailConfig::Resend { from, .. }) => {
                    format!("{} (from {from}, API key set)", e.provider())
                }
                Some(e @ EmailConfig::File { path, .. }) => {
                    format!("{} ({})", e.provider(), path.display())
                }
                None => "none (email notifications unavailable)".into(),
            },
            required: false,
        });
        out.push(Setting {
            name: "JOBHUNT_AI_PROVIDER",
            status: match &self.ai {
                Ok(Some(c)) => format!(
                    "{} ({}{})",
                    c.provider.as_str(),
                    c.model,
                    if c.api_key.is_some() {
                        ", API key set"
                    } else {
                        ""
                    }
                ),
                Ok(None) => "none (the built-in taste reader)".into(),
                Err(problem) => format!("invalid: {problem} (the built-in taste reader is used)"),
            },
            required: false,
        });
        out.push(Setting {
            name: "JOBHUNT_AI_REVIEW_FIT",
            status: match &self.fit_review {
                Ok(Some(c)) => format!("on ({}:{})", c.provider.as_str(), c.model),
                Ok(None) => "off (ranking relies on its rules)".into(),
                Err(problem) => format!("invalid: {problem} (ranking relies on its rules)"),
            },
            required: false,
        });
        out
    }
}

/// Configuration problems, one per line.
#[derive(Debug, thiserror::Error)]
#[error("the cloud configuration is incomplete:\n  - {}", .0.join("\n  - "))]
pub struct ConfigProblems(pub Vec<String>);

/// The product configuration: `JOBHUNT_CONFIG` (if set), then variables.
fn load_app(env: Env<'_>, problems: &mut Vec<String>) -> Option<LoadedConfig> {
    let file = env("JOBHUNT_CONFIG").map(PathBuf::from);
    let mut loaded = match config::load(file.as_deref(), Some(Path::new("(cloud)")), None) {
        Ok(loaded) => loaded,
        Err(e) => {
            problems.push(format!("JOBHUNT_CONFIG: {}", jobhunt_core::ErrorChain(&e)));
            return None;
        }
    };
    let c = &mut loaded.config;
    if let Some(level) = env("JOBHUNT_LOG") {
        c.logging.level = level;
    }
    match env("JOBHUNT_LOG_FORMAT").as_deref() {
        Some("json") => c.logging.format = LogFormat::Json,
        Some("text") => c.logging.format = LogFormat::Text,
        Some(_) => problems.push("JOBHUNT_LOG_FORMAT must be \"json\" or \"text\"".into()),
        None => {}
    }
    c.discovery.concurrency = parse(
        env,
        "JOBHUNT_DISCOVERY_CONCURRENCY",
        c.discovery.concurrency,
        problems,
    );
    c.verification.fresh_hours = parse(
        env,
        "JOBHUNT_VERIFICATION_FRESH_HOURS",
        c.verification.fresh_hours,
        problems,
    );
    c.verification.stale_hours = parse(
        env,
        "JOBHUNT_VERIFICATION_STALE_HOURS",
        c.verification.stale_hours,
        problems,
    );
    if let Err(e) = c.sources.specs() {
        problems.push(format!("invalid source in the configuration: {e}"));
    }
    Some(loaded)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn config(vars: &[(&str, &str)]) -> CloudConfig {
        let map: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        CloudConfig::from_env(&move |name: &str| map.get(name).cloned())
    }

    const KEY: &str = "k1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

    #[test]
    fn a_complete_server_configuration_is_ready() {
        let c = config(&[
            ("DATABASE_URL", "postgres://u:secret@db:5432/jobhunt"),
            ("JOBHUNT_ENCRYPTION_KEYS", KEY),
            ("JOBHUNT_OIDC_ISSUER", "https://example.auth0.com/"),
            ("JOBHUNT_OIDC_AUDIENCE", "https://api.jobhunt.test"),
            ("RAILWAY_ENVIRONMENT_NAME", "production"),
            ("RAILWAY_PUBLIC_DOMAIN", "api.jobhunt.test"),
            ("PORT", "9000"),
        ]);
        assert!(
            c.problems(Role::Server).is_empty(),
            "{:?}",
            c.problems(Role::Server)
        );
        assert!(c.is_production());
        assert_eq!(c.bind.port(), 9000);
        assert_eq!(
            c.public_url.as_ref().map(Url::as_str),
            Some("https://api.jobhunt.test/")
        );
        // Secrets never appear in reports or debug output.
        let report = format!("{:?} {:?}", c.report(), c);
        assert!(!report.contains("secret"));
        assert!(!report.contains("AAAAAAAA"));
    }

    #[test]
    fn missing_pieces_are_named() {
        let problems = config(&[]).problems(Role::Server);
        let text = problems.join("\n");
        assert!(text.contains("DATABASE_URL"));
        assert!(text.contains("JOBHUNT_ENCRYPTION_KEYS"));
        assert!(text.contains("authentication is not configured"));
        // A worker needs only the database.
        assert_eq!(
            config(&[("DATABASE_URL", "postgres://x")]).problems(Role::Worker),
            Vec::<String>::new()
        );
    }

    #[test]
    fn dev_auth_is_refused_in_production() {
        let c = config(&[
            ("DATABASE_URL", "postgres://x"),
            ("JOBHUNT_ENCRYPTION_KEYS", KEY),
            ("JOBHUNT_AUTH_MODE", "dev"),
            (
                "JOBHUNT_AUTH_DEV_SECRET",
                "0123456789abcdef0123456789abcdef",
            ),
            ("JOBHUNT_ENV", "production"),
            ("JOBHUNT_PUBLIC_URL", "https://x.test"),
        ]);
        assert!(
            c.problems(Role::Server)
                .iter()
                .any(|p| p.contains("refused in production"))
        );
        let staging = config(&[
            ("DATABASE_URL", "postgres://x"),
            ("JOBHUNT_ENCRYPTION_KEYS", KEY),
            ("JOBHUNT_AUTH_MODE", "dev"),
            (
                "JOBHUNT_AUTH_DEV_SECRET",
                "0123456789abcdef0123456789abcdef",
            ),
        ]);
        assert!(staging.problems(Role::Server).is_empty());
    }

    #[test]
    fn production_refuses_github_endpoint_override_before_a_token_can_be_used() {
        let production = config(&[
            ("JOBHUNT_ENV", "production"),
            ("JOBHUNT_GITHUB_TOKEN", "secret-test-token"),
            ("JOBHUNT_GITHUB_ENDPOINT", "http://127.0.0.1:9876"),
        ]);
        assert!(
            production
                .problems(Role::Server)
                .iter()
                .any(|p| p.contains("JOBHUNT_GITHUB_ENDPOINT") && p.contains("production"))
        );
        assert_eq!(
            production.github_endpoint(),
            None,
            "a token can only go to the official endpoint"
        );
        let development = config(&[
            ("JOBHUNT_ENV", "development"),
            ("JOBHUNT_GITHUB_ENDPOINT", "http://127.0.0.1:9876"),
        ]);
        assert!(
            !development
                .problems(Role::Server)
                .iter()
                .any(|p| p.contains("JOBHUNT_GITHUB_ENDPOINT"))
        );
        assert_eq!(development.github_endpoint(), Some("http://127.0.0.1:9876"));
    }

    #[test]
    fn invalid_values_are_reported() {
        let c = config(&[
            ("JOBHUNT_DB_MAX_CONNECTIONS", "many"),
            ("JOBHUNT_ENCRYPTION_KEYS", "k1:short"),
            ("JOBHUNT_LOG_FORMAT", "xml"),
        ]);
        let text = c.problems(Role::Server).join("\n");
        assert!(text.contains("JOBHUNT_DB_MAX_CONNECTIONS"));
        assert!(text.contains("JOBHUNT_ENCRYPTION_KEYS: invalid"));
        assert!(text.contains("JOBHUNT_LOG_FORMAT"));
    }

    #[test]
    fn the_cloud_source_list_is_valid() {
        let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy/cloud.toml");
        let c = config(&[("JOBHUNT_CONFIG", file.to_str().unwrap_or_default())]);
        assert!(
            c.problems(Role::Worker)
                .iter()
                .all(|p| p.contains("DATABASE_URL"))
        );
        assert!(c.app.config.sources.specs().unwrap().len() >= 10);
    }

    /// The registry (`deploy/sources.toml`) records the production sources
    /// as active, and nothing else: activating a board means adding it to
    /// both files.
    #[test]
    fn the_source_registry_matches_the_cloud_source_list() {
        let deploy = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy");
        let c = config(&[(
            "JOBHUNT_CONFIG",
            deploy.join("cloud.toml").to_str().unwrap_or_default(),
        )]);
        let mut configured: Vec<String> = c
            .app
            .config
            .sources
            .specs()
            .unwrap()
            .iter()
            .map(|s| s.key().to_string())
            .collect();
        let text = std::fs::read_to_string(deploy.join("sources.toml")).unwrap();
        let registry = jobhunt_sources::registry::SourceRegistry::parse(&text).unwrap();
        let mut active: Vec<String> = registry
            .configured()
            .map(|e| e.source.to_string())
            .collect();
        configured.sort();
        active.sort();
        assert_eq!(active, configured);
    }

    #[test]
    fn email_and_notification_settings() {
        let base = [
            ("DATABASE_URL", "postgres://x"),
            ("JOBHUNT_ENCRYPTION_KEYS", KEY),
        ];
        // Without email, the notifier names what is missing.
        let text = config(&base).problems(Role::Notifier).join("\n");
        assert!(text.contains("email is not configured"), "{text}");
        assert!(text.contains("JOBHUNT_WEB_URL"), "{text}");
        let mut vars = base.to_vec();
        vars.extend([
            ("JOBHUNT_EMAIL_PROVIDER", "resend"),
            ("JOBHUNT_RESEND_API_KEY", "re_secret_value"),
            ("JOBHUNT_EMAIL_FROM", "JobHunt <n@jobhunt.test>"),
            ("JOBHUNT_WEB_URL", "https://app.jobhunt.test"),
            ("JOBHUNT_NOTIFY_MAX_ITEMS", "40"),
        ]);
        let c = config(&vars);
        assert!(
            c.problems(Role::Notifier).is_empty(),
            "{:?}",
            c.problems(Role::Notifier)
        );
        assert_eq!(c.notify.max_items, 5, "an email stays short");
        assert!(matches!(c.email, Some(EmailConfig::Resend { .. })));
        let report = format!("{:?} {:?}", c.report(), c);
        assert!(!report.contains("re_secret_value"), "{report}");
        // Half-configured providers and the file provider in production.
        let text = config(&[("JOBHUNT_EMAIL_PROVIDER", "resend")])
            .problems(Role::Worker)
            .join("\n");
        assert!(text.contains("JOBHUNT_RESEND_API_KEY"), "{text}");
        let text = config(&[
            ("JOBHUNT_EMAIL_PROVIDER", "file"),
            ("JOBHUNT_EMAIL_FILE", "/tmp/mail.jsonl"),
            ("JOBHUNT_ENV", "production"),
        ])
        .problems(Role::Worker)
        .join("\n");
        assert!(text.contains("refused in production"), "{text}");
    }

    #[test]
    fn identity_provider_settings_are_read_verbatim() {
        let c = config(&[
            ("JOBHUNT_OIDC_ISSUER", " https://acme.authkit.app "),
            (
                "JOBHUNT_OIDC_AUDIENCE",
                "https://api.jobhunt.test, https://api.jobhunt.test/mcp,",
            ),
            ("JOBHUNT_OIDC_CLI_CLIENT_ID", "client_01"),
        ]);
        let Some(AuthConfig::Oidc(settings)) = &c.auth else {
            panic!("{:?}", c.auth);
        };
        // No normalization: tokens say exactly this.
        assert_eq!(settings.issuer, "https://acme.authkit.app");
        assert_eq!(
            settings.audiences,
            ["https://api.jobhunt.test", "https://api.jobhunt.test/mcp"]
        );
        assert_eq!(settings.audience_parameter, AudienceParameter::Resource);
        assert_eq!(settings.endpoints, ProviderEndpoints::default());

        let c = config(&[
            ("JOBHUNT_OIDC_ISSUER", "https://example.auth0.com/"),
            ("JOBHUNT_OIDC_AUDIENCE", "https://api.jobhunt.test"),
            ("JOBHUNT_OIDC_AUDIENCE_PARAMETER", "audience"),
            ("JOBHUNT_OIDC_JWKS_URL", "https://example.auth0.com/jwks"),
        ]);
        let Some(AuthConfig::Oidc(settings)) = &c.auth else {
            panic!("{:?}", c.auth);
        };
        assert_eq!(settings.audience_parameter, AudienceParameter::Audience);
        assert_eq!(
            settings.endpoints.jwks_uri.as_deref(),
            Some("https://example.auth0.com/jwks")
        );

        let c = config(&[
            ("JOBHUNT_OIDC_ISSUER", "https://acme.authkit.app"),
            ("JOBHUNT_OIDC_AUDIENCE", " , "),
            ("JOBHUNT_OIDC_AUDIENCE_PARAMETER", "aud"),
            ("JOBHUNT_OIDC_TOKEN_URL", "not a url"),
        ]);
        let text = c.problems(Role::Server).join("\n");
        assert!(text.contains("JOBHUNT_OIDC_AUDIENCE is empty"), "{text}");
        let c = config(&[
            ("JOBHUNT_OIDC_ISSUER", "https://acme.authkit.app"),
            ("JOBHUNT_OIDC_AUDIENCE", "x"),
            ("JOBHUNT_OIDC_AUDIENCE_PARAMETER", "aud"),
            ("JOBHUNT_OIDC_TOKEN_URL", "not a url"),
        ]);
        let text = c.problems(Role::Server).join("\n");
        assert!(text.contains("JOBHUNT_OIDC_AUDIENCE_PARAMETER"), "{text}");
        assert!(
            text.contains("JOBHUNT_OIDC_TOKEN_URL is not a URL"),
            "{text}"
        );
    }
}
