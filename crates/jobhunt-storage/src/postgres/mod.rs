//! The cloud store: PostgreSQL.
//!
//! * [`PgStore`] is the shared part: the job corpus, its history and
//!   verification ([`jobhunt_jobs::JobRepository`] and
//!   [`jobhunt_jobs::verification::VerificationRepository`]), accounts,
//!   scheduling and usage. Workers use it directly.
//! * [`PgUserStore`] is one person's view ([`PgStore::for_user`]): the shared
//!   corpus plus their own profile, feedback, eligibility decisions,
//!   rankings and search state, and nobody else's. Every private query is
//!   bound to its account; there is no method that reads private data
//!   without one. It implements [`crate::Store`], so the application's use
//!   cases run on it unchanged.
//!
//! Private values are encrypted by the application ([`crypto`]) before
//! they are written.
//!
//! Migrations (`migrations/postgres`) are applied by [`PgStore::migrate`];
//! sqlx holds a Postgres advisory lock while migrating, so several
//! instances starting at once apply each migration exactly once.

pub mod accounts;
mod cache;
pub mod crypto;
mod jobs;
pub mod notify;
mod profile;
mod rotate;
pub mod schedule;
mod sync;
#[doc(hidden)]
pub mod testing;
pub mod usage;
mod user;
mod verification;

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use jobhunt_jobs::StorageError;
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};

pub use accounts::{
    Account, ApiToken, IdentityLink, NewApiToken, TOKEN_PREFIX, TokenCheck, UserId, hash_token,
};
pub use crypto::{CryptoError, Keyring};
pub use notify::{
    Cadence, Delivery, DeliveryKind, DeliverySummary, DigestSettings, NewDelivery,
    NotificationSettings, NotifiedItem, NotifyAccount, SettingsChange, WEEKDAYS,
    new_confirmation_token, new_delivery_id,
};
pub use rotate::Reencrypted;
pub use schedule::{
    ClaimedSource, ScheduleRow, ScheduleSettings, ScheduledSource, SourceOutcome, SourceTier,
    VerificationCandidate, VerificationPick, VerifyReason, WorkerKind, WorkerRun,
};
pub use sync::{CloudSyncState, MAX_PUSH_ENTITIES, MAX_PUSH_FEEDBACK, MAX_PUSH_JOBS};
pub use usage::UsageEvent;
pub use user::PgUserStore;

pub(crate) static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations/postgres");

pub(crate) fn query_error(operation: &'static str) -> impl FnOnce(sqlx::Error) -> StorageError {
    move |source| StorageError::Query {
        operation,
        source: Box::new(source),
    }
}

pub(crate) fn corrupt(id: &str, detail: impl ToString) -> StorageError {
    StorageError::Corrupt {
        id: id.to_owned(),
        detail: detail.to_string(),
    }
}

/// Connection pool settings. Every process keeps its pool small: Railway's
/// Postgres allows about 100 connections in total, shared by the API
/// replicas and the workers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PgSettings {
    pub max_connections: u32,
    pub min_connections: u32,
    /// How long a request waits for a free connection before failing.
    pub acquire_timeout: Duration,
    /// Idle connections are closed after this long.
    pub idle_timeout: Duration,
    /// Statements running longer than this are cancelled by the server.
    pub statement_timeout: Duration,
}

impl Default for PgSettings {
    fn default() -> Self {
        Self {
            max_connections: 10,
            min_connections: 0,
            acquire_timeout: Duration::from_secs(10),
            idle_timeout: Duration::from_secs(300),
            statement_timeout: Duration::from_secs(60),
        }
    }
}

/// The schema state, for readiness checks and diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaStatus {
    pub applied: u64,
    pub known: u64,
    pub latest: Option<i64>,
}

impl SchemaStatus {
    /// Every migration this build knows is applied.
    pub fn is_current(&self) -> bool {
        self.applied >= self.known
    }
}

pub(crate) struct Shared {
    pub(crate) pool: PgPool,
    pub(crate) keys: Keyring,
    location: String,
}

/// The shared cloud store (see the module docs).
#[derive(Clone)]
pub struct PgStore {
    pub(crate) shared: Arc<Shared>,
}

impl std::fmt::Debug for PgStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgStore")
            .field("location", &self.shared.location)
            .field("keys", &self.shared.keys)
            .finish()
    }
}

impl PgStore {
    /// Connects to `url` (`postgres://…`). Nothing is migrated: call
    /// [`PgStore::migrate`].
    pub async fn connect(
        url: &str,
        settings: &PgSettings,
        keys: Keyring,
    ) -> Result<Self, StorageError> {
        let options = PgConnectOptions::from_str(url).map_err(|e| StorageError::Open {
            location: "the configured Postgres URL".into(),
            source: Box::new(e),
        })?;
        Self::connect_with(options, settings, keys).await
    }

    /// Connects with parsed options.
    pub async fn connect_with(
        options: PgConnectOptions,
        settings: &PgSettings,
        keys: Keyring,
    ) -> Result<Self, StorageError> {
        let options = options.application_name("jobhunt").options([(
            "statement_timeout",
            format!("{}ms", settings.statement_timeout.as_millis()),
        )]);
        let location = format!(
            "postgres://{}:{}/{}",
            options.get_host(),
            options.get_port(),
            options.get_database().unwrap_or("")
        );
        let pool = PgPoolOptions::new()
            .max_connections(settings.max_connections.max(1))
            .min_connections(settings.min_connections)
            .acquire_timeout(settings.acquire_timeout)
            .idle_timeout(Some(settings.idle_timeout))
            .connect_with(options)
            .await
            .map_err(|e| StorageError::Open {
                location: location.clone(),
                source: Box::new(e),
            })?;
        Ok(Self {
            shared: Arc::new(Shared {
                pool,
                keys,
                location,
            }),
        })
    }

    pub(crate) fn pool(&self) -> &PgPool {
        &self.shared.pool
    }

    pub(crate) fn keys(&self) -> &Keyring {
        &self.shared.keys
    }

    /// Host, port and database (never credentials).
    pub fn location(&self) -> &str {
        &self.shared.location
    }

    /// Applies pending migrations. Safe to run from several processes at
    /// once: sqlx serializes them with an advisory lock.
    pub async fn migrate(&self) -> Result<SchemaStatus, StorageError> {
        MIGRATOR
            .run(self.pool())
            .await
            .map_err(|e| StorageError::Migration(Box::new(e)))?;
        self.schema().await
    }

    /// Which migrations are applied.
    pub async fn schema(&self) -> Result<SchemaStatus, StorageError> {
        let exists: bool = sqlx::query_scalar("SELECT to_regclass('_sqlx_migrations') IS NOT NULL")
            .fetch_one(self.pool())
            .await
            .map_err(query_error("reading the schema version"))?;
        let known = MIGRATOR.iter().count() as u64;
        if !exists {
            return Ok(SchemaStatus {
                applied: 0,
                known,
                latest: None,
            });
        }
        let (applied, latest): (i64, Option<i64>) =
            sqlx::query_as("SELECT COUNT(*), MAX(version) FROM _sqlx_migrations WHERE success")
                .fetch_one(self.pool())
                .await
                .map_err(query_error("reading the schema version"))?;
        Ok(SchemaStatus {
            applied: u64::try_from(applied).unwrap_or(0),
            known,
            latest,
        })
    }

    /// A round trip to the database.
    pub async fn ping(&self) -> Result<(), StorageError> {
        sqlx::query("SELECT 1")
            .execute(self.pool())
            .await
            .map_err(query_error("checking the database connection"))?;
        Ok(())
    }

    /// One person's view of the store.
    pub fn for_user(&self, user: UserId) -> PgUserStore {
        PgUserStore::new(self.clone(), user)
    }

    /// Closes every connection (graceful shutdown).
    pub async fn close(&self) {
        self.pool().close().await;
    }
}
