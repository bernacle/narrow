//! The local JobHunt application: one composition of configuration,
//! storage and the domain services, shared by every front-end.
//!
//! `narrow` commands and the MCP server (`narrow mcp`) are both thin
//! interfaces over [`LocalApp`]: they parse arguments, call a use case
//! here, and present the answer. So both read the same configuration, open
//! the same SQLite database, and reach the same decisions: a job rejected
//! from the terminal is rejected for an MCP client too, with the same
//! reason, and the next search reflects it either way.
//!
//! The use cases:
//!
//! * [`LocalApp::find`] ([`shortlist`]): refresh when stale, rank, verify
//!   the best candidates, and return the few worth the person's time;
//! * [`LocalApp::resolve`] ([`resolve`]): `opp_…` / `job_…` / short ids to
//!   a logical opportunity and its source records;
//! * [`LocalApp::inspect`] ([`inspect`]): one opportunity, with its
//!   verification, eligibility, decision brief and pipeline state;
//! * [`LocalApp::verify`] ([`verify`]): ask the authoritative sources;
//! * [`LocalApp::record_feedback`] ([`feedback`]): save, reject, applied, …
//!   (repeats are not recorded twice);
//! * [`LocalApp::update_preferences`] ([`preferences`]);
//! * [`LocalApp::taste_profile`], [`LocalApp::describe_taste`] and
//!   [`LocalApp::review_taste`] ([`taste_profile`]): the candidate taste
//!   profile, read from the person's words (by a configured model or the
//!   built-in rules), with their confirmations and corrections;
//! * [`LocalApp::profile_view`] ([`profile_view`]): the profile, without
//!   contact details;
//! * [`LocalApp::import_linkedin`], [`LocalApp::import_github`] and
//!   [`LocalApp::remove_source`] ([`profile_sources`]): evidence from a
//!   LinkedIn export or a public GitHub account, in the same graph;
//! * [`LocalApp::application_context`] ([`context`]): evidence a client may
//!   use to help with an application, only what the evidence policy allows;
//! * [`LocalApp::export_state`] / [`LocalApp::import_state`] ([`state`]);
//! * [`LocalApp::doctor`] ([`doctor`]);
//! * [`LocalApp::sync`] ([`sync`]): merge the person's state with JobHunt
//!   Cloud.
//!
//! [`views`] holds the typed, serializable answers (the MCP tools'
//! structured content and output schemas).
//!
//! Nothing here prints: progress is reported through [`Progress`], and
//! results are returned. That keeps an MCP server's stdout for the
//! protocol alone.

pub mod broad_discovery;
pub mod config;
pub mod context;
pub mod controls;
pub mod discover;
pub mod doctor;
pub mod error;
pub mod feed;
pub mod feedback;
pub mod inspect;
pub mod preferences;
pub mod profile_edit;
pub mod profile_sources;
pub mod profile_view;
pub mod resolve;
pub mod shortlist;
pub mod sources;
pub mod state;
pub mod sync;
pub mod taste_profile;
pub mod taste_view;
pub mod verify;
pub mod views;

use chrono::{DateTime, Utc};
use jobhunt_jobs::verification::FreshnessPolicy;
use jobhunt_profile::ProfileService;
use jobhunt_ranking::{RankingService, RuleReader};
use std::sync::Arc;

use jobhunt_storage::{SqliteJobStore, Store};

pub use config::{AppConfig, LoadedConfig, Paths};
pub use discover::{Refresh, RefreshMode, RefreshReason, SourceArg};
pub use error::{AppError, ErrorKind};
pub use resolve::{Opportunity, short_id};
pub use shortlist::{FindRequest, Found, SearchResults};
pub use sync::{Remote, Side, SyncReport, SyncTransport};

/// Something a long use case is doing, for front-ends that show progress
/// (the CLI prints it on stderr; the MCP server logs it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProgressEvent {
    /// Reading sources.
    Refreshing {
        sources: usize,
        reason: RefreshReason,
    },
    /// Asking authoritative sources about listings.
    Verifying { records: usize, sources: usize },
}

/// Receives [`ProgressEvent`]s.
pub trait Progress: Send + Sync {
    fn note(&self, event: ProgressEvent);
}

/// Ignores progress.
#[derive(Debug, Clone, Copy, Default)]
pub struct Quiet;

impl Progress for Quiet {
    fn note(&self, _event: ProgressEvent) {}
}

/// Where discovery happens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DiscoveryMode {
    /// Searches read the configured job boards themselves when stored jobs
    /// are stale (the local product).
    #[default]
    OnDemand,
    /// Scheduled workers keep one shared corpus fresh (JobHunt Cloud);
    /// searches never read job boards. They still verify their best
    /// candidates, and verifications are shared by everyone.
    Background,
}

/// The application: configuration plus one person's view of a store.
///
/// Locally that is the SQLite file ([`App::open`]); in the cloud it is a
/// user-scoped view of Postgres ([`App::from_parts`]), one per request. The
/// use cases are the same either way.
///
/// Safe to share between concurrent requests (`Arc<App>`).
/// Read-modify-write use cases (feedback, preferences, imports) run under
/// the store's write lock ([`Store::write_lock`]: a process-wide mutex for
/// SQLite, a per-person advisory lock in Postgres), so concurrent requests
/// cannot interleave their checks and writes. Other writers are covered by
/// the profile's optimistic revisions: a lost race is reported as
/// [`ErrorKind::Conflict`], never silently merged.
pub struct App {
    loaded: Arc<LoadedConfig>,
    store: Arc<dyn Store>,
    discovery: DiscoveryMode,
    taste_model: TasteModel,
    fit_review: FitReview,
}

/// The semantic fit reviewer of the ranking's shortlist, if one is
/// configured (see `jobhunt_ranking::review`). Without it, the rules decide
/// fit on their own.
#[derive(Clone, Default)]
pub struct FitReview {
    pub reviewer: Option<Arc<dyn jobhunt_ranking::FitReviewer>>,
    pub budget: jobhunt_ranking::ReviewBudget,
    /// Why a configured reviewer can't be used.
    pub problem: Option<String>,
}

impl std::fmt::Debug for FitReview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FitReview")
            .field("reviewer", &self.reviewer.as_ref().map(|r| r.name()))
            .field("budget", &self.budget)
            .field("problem", &self.problem)
            .finish()
    }
}

impl FitReview {
    /// The reviewer a configuration names (see [`config::AiConfig`]).
    pub fn from_config(config: &config::AiConfig) -> Self {
        match config.fit_reviewer() {
            Ok(Some(reviewer)) => Self {
                reviewer: Some(Arc::new(reviewer)),
                ..Self::default()
            },
            Ok(None) => Self::default(),
            Err(problem) => {
                tracing::warn!(%problem, "the fit reviewer is not usable; ranking relies on its rules");
                Self {
                    problem: Some(problem),
                    ..Self::default()
                }
            }
        }
    }
}

/// The model that reads taste, if one is configured.
#[derive(Clone, Default)]
pub struct TasteModel {
    /// Usable.
    pub model: Option<Arc<dyn jobhunt_profile::taste::TasteInterpreter>>,
    /// Why a configured model can't be used (the built-in rules read
    /// instead).
    pub problem: Option<String>,
}

impl std::fmt::Debug for TasteModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TasteModel")
            .field("model", &self.model.as_ref().map(|m| m.name()))
            .field("problem", &self.problem)
            .finish()
    }
}

impl TasteModel {
    /// The model a configuration names (see [`config::AiConfig`]).
    pub fn from_config(config: &config::AiConfig) -> Self {
        match config.model() {
            Ok(Some(model)) => Self {
                model: Some(Arc::new(model)),
                problem: None,
            },
            Ok(None) => Self::default(),
            Err(problem) => {
                tracing::warn!(%problem, "the taste model is not usable; using the built-in reader");
                Self {
                    model: None,
                    problem: Some(problem),
                }
            }
        }
    }
}

/// The local application (the name the CLI and the stdio MCP server use).
pub type LocalApp = App;

impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App")
            .field("store", &self.store.location())
            .field("discovery", &self.discovery)
            .finish_non_exhaustive()
    }
}

impl App {
    /// Opens (creating and migrating if needed) the configured local
    /// database.
    pub async fn open(loaded: LoadedConfig) -> Result<Self, AppError> {
        let store = SqliteJobStore::open(&loaded.database)
            .await
            .map_err(|e| AppError::storage("opening it", e))?;
        let model = TasteModel::from_config(&loaded.config.ai);
        let review = FitReview::from_config(&loaded.config.ai);
        Ok(Self::with_store(loaded, store)
            .with_taste_model(model)
            .with_fit_review(review))
    }

    /// An application over an already open store, discovering on demand.
    pub fn with_store(loaded: LoadedConfig, store: impl Store + 'static) -> Self {
        Self::from_parts(Arc::new(loaded), Arc::new(store), DiscoveryMode::OnDemand)
    }

    /// An application over a shared configuration and store (the cloud
    /// builds one per request).
    pub fn from_parts(
        loaded: Arc<LoadedConfig>,
        store: Arc<dyn Store>,
        discovery: DiscoveryMode,
    ) -> Self {
        Self {
            loaded,
            store,
            discovery,
            taste_model: TasteModel::default(),
            fit_review: FitReview::default(),
        }
    }

    /// Reviews the ranking's shortlist with this reviewer (the cloud shares
    /// one between requests; tests give a fake).
    pub fn with_fit_review(mut self, review: FitReview) -> Self {
        self.fit_review = review;
        self
    }

    pub fn fit_review(&self) -> &FitReview {
        &self.fit_review
    }

    /// Reads taste with this model (the cloud shares one between
    /// requests; tests give a fake).
    pub fn with_taste_model(mut self, model: TasteModel) -> Self {
        self.taste_model = model;
        self
    }

    pub fn taste_model(&self) -> &TasteModel {
        &self.taste_model
    }

    /// Closes the store (for SQLite, flushing the write-ahead log).
    pub async fn close(self) {
        self.store.shutdown().await;
    }

    pub fn loaded(&self) -> &LoadedConfig {
        &self.loaded
    }

    pub fn config(&self) -> &AppConfig {
        &self.loaded.config
    }

    pub fn store(&self) -> &(dyn Store + 'static) {
        self.store.as_ref()
    }

    pub fn discovery_mode(&self) -> DiscoveryMode {
        self.discovery
    }

    /// When a verification is fresh, stale, or reused.
    pub fn policy(&self) -> FreshnessPolicy {
        self.config().verification.policy()
    }

    /// The ranking use cases, with the configured freshness policy.
    pub fn ranking(&self) -> RankingService<'_, dyn Store> {
        let service = RankingService::new(self.store(), &RuleReader).with_policy(self.policy());
        match &self.fit_review.reviewer {
            Some(reviewer) => service.with_reviewer(reviewer.as_ref(), self.fit_review.budget),
            None => service,
        }
    }

    /// The profile use cases for the person's profile.
    pub fn profiles(&self) -> ProfileService<'_, dyn Store> {
        ProfileService::new(self.store())
    }

    /// The opportunity an id (`opp_…`, `job_…`, or a unique prefix of
    /// either) refers to, with every source record.
    pub async fn resolve(&self, id: &str) -> Result<Opportunity, AppError> {
        resolve::resolve(self.store(), id).await
    }

    /// Runs a read-modify-write use case under the store's write lock.
    pub(crate) async fn exclusive<T, F>(&self, f: F) -> Result<T, AppError>
    where
        F: std::future::Future<Output = Result<T, AppError>>,
    {
        let _guard = self.store.write_lock().await?;
        f.await
    }
}

/// The current time. One place, so every use case in a request agrees.
pub fn now() -> DateTime<Utc> {
    Utc::now()
}
