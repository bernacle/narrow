//! Local SQLite job store.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::str::FromStr;
use std::sync::LazyLock;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use jobhunt_core::text::search_key;
use jobhunt_core::{BoxError, CanonicalUrl, IngestCounts, Provenance, SourceKey};
use jobhunt_jobs::lifecycle::{Action, Closing, StoredJob, plan_scan};
use jobhunt_jobs::{
    CANONICAL_REVISION, Compensation, EmploymentType, IdentityEntry, JobEvent, JobEventKind, JobId,
    JobPosting, JobQuery, JobRecord, JobRepository, JobSnapshot, JobStatus, LastListing,
    OpportunityId, RunId, RunSummary, ScanBody, ScanResult, ScanWrite, SourceLocation,
    StorageError, WorkplaceType, evidence_keys,
};
use sqlx::sqlite::{
    SqliteArguments, SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions,
    SqliteRow, SqliteSynchronous,
};
use sqlx::{QueryBuilder, Row, Sqlite, SqliteConnection};
use tracing::{debug, info};

use crate::store::ScanRecord;

pub(crate) static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations/sqlite");

/// Ids bound per statement in batched updates (well under SQLite's limit).
const BATCH: usize = 500;

/// How many times opening a database retries migrations that raced
/// another process.
const MIGRATION_ATTEMPTS: u32 = 5;

/// Content columns written on insert and on content rewrite, in bind order.
/// Must match [`ContentValues::bind`].
const CONTENT_COLUMNS: [&str; 24] = [
    "source_kind",
    "source_instance",
    "source_job_id",
    "fetched_from",
    "url",
    "apply_url",
    "company",
    "title",
    "department",
    "team",
    "location",
    "locations",
    "employment_type",
    "workplace_type",
    "is_remote",
    "compensation",
    "description_text",
    "description_html",
    "posted_at",
    "source_updated_at",
    "search_text",
    "fingerprint",
    "content_fingerprint",
    "work_authorization",
];

static INSERT_SQL: LazyLock<String> = LazyLock::new(|| {
    let columns = CONTENT_COLUMNS.join(", ");
    // id, content columns, 3 timestamps, status, opportunity_id.
    let placeholders = vec!["?"; CONTENT_COLUMNS.len() + 6].join(", ");
    format!(
        "INSERT INTO jobs (id, {columns}, first_seen_at, last_seen_at, content_updated_at, \
         status, opportunity_id) VALUES ({placeholders})"
    )
});

/// Rewrites content and marks the job seen and open. A NULL
/// `content_updated_at` keeps the stored value (non-material refreshes).
static REWRITE_SQL: LazyLock<String> = LazyLock::new(|| {
    let assignments: Vec<String> = CONTENT_COLUMNS.iter().map(|c| format!("{c} = ?")).collect();
    format!(
        "UPDATE jobs SET {}, last_seen_at = ?, \
         content_updated_at = COALESCE(?, content_updated_at), status = 'open', \
         closed_at = NULL WHERE id = ?",
        assignments.join(", ")
    )
});

/// SQLite implementation of [`JobRepository`].
///
/// Opening a store creates the database file (and its directory) if needed
/// and applies pending migrations, so callers never see an uninitialized
/// schema.
#[derive(Debug, Clone)]
pub struct SqliteJobStore {
    pub(crate) pool: SqlitePool,
    location: String,
    /// Serializes read-modify-write use cases within this process (see
    /// [`crate::Store::write_lock`]); other processes are covered by
    /// SQLite's own locking and the profile's revisions.
    pub(crate) writes: std::sync::Arc<tokio::sync::Mutex<()>>,
}

impl SqliteJobStore {
    /// Opens (creating if necessary) the database at `path` and migrates it.
    pub async fn open(path: &Path) -> Result<Self, StorageError> {
        let location = path.display().to_string();
        let open_error = |source: BoxError| StorageError::Open {
            location: location.clone(),
            source,
        };

        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|e| open_error(Box::new(e)))?;
        }
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(5));
        // Journal setup can race another process opening this database for
        // the first time. The busy timeout does not cover every PRAGMA lock,
        // so retry only SQLite lock errors before running migrations.
        let mut attempt = 0;
        let pool = loop {
            match SqlitePoolOptions::new()
                .max_connections(4)
                .connect_with(options.clone())
                .await
            {
                Ok(pool) => break pool,
                Err(error)
                    if attempt < MIGRATION_ATTEMPTS
                        && matches!(&error, sqlx::Error::Database(db) if matches!(db.code().as_deref(), Some("5" | "6"))) =>
                {
                    attempt += 1;
                    debug!(%error, attempt, "database opening raced another process; retrying");
                    tokio::time::sleep(Duration::from_millis(50 * u64::from(attempt))).await;
                }
                Err(error) => return Err(open_error(Box::new(error))),
            }
        };
        Self::initialize(pool, location).await
    }

    /// Opens a private in-memory database, mainly for tests.
    pub async fn open_in_memory() -> Result<Self, StorageError> {
        let location = ":memory:".to_owned();
        let open_error = |source: sqlx::Error| StorageError::Open {
            location: location.clone(),
            source: Box::new(source),
        };
        let options = SqliteConnectOptions::from_str("sqlite::memory:")
            .map_err(open_error)?
            .foreign_keys(true);
        // Every in-memory connection is a separate database, so keep exactly
        // one connection alive for the lifetime of the pool.
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .min_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
            .connect_with(options)
            .await
            .map_err(open_error)?;
        Self::initialize(pool, location).await
    }

    async fn initialize(pool: SqlitePool, location: String) -> Result<Self, StorageError> {
        // Two processes opening a new database at the same moment (the CLI
        // and an MCP server, say) can both see a migration as pending; the
        // second one's attempt then fails because the first applied it.
        // Running the migrator again is safe: it re-reads what is applied
        // (and verifies checksums), so it either finishes or reports a real
        // problem.
        let mut attempt = 0;
        loop {
            match MIGRATOR.run(&pool).await {
                Ok(()) => break,
                Err(error) if attempt < MIGRATION_ATTEMPTS => {
                    attempt += 1;
                    debug!(%error, attempt, "migration raced another process; retrying");
                    tokio::time::sleep(Duration::from_millis(50 * u64::from(attempt))).await;
                }
                Err(error) => return Err(StorageError::Migration(Box::new(error))),
            }
        }
        info!(database = %location, "job store ready");
        Ok(Self {
            pool,
            location,
            writes: std::sync::Arc::default(),
        })
    }

    /// Human-readable location of the database (a path or `:memory:`).
    pub fn location(&self) -> &str {
        &self.location
    }

    /// Starts a transaction that writes. It takes SQLite's write lock at
    /// `BEGIN IMMEDIATE`, where a busy database is waited for (the busy
    /// timeout), instead of upgrading a read transaction later, which fails
    /// at once (`SQLITE_BUSY_SNAPSHOT`) if another connection wrote in
    /// between. Every read-then-write transaction must start here.
    pub(crate) async fn begin_write(
        &self,
    ) -> Result<sqlx::Transaction<'static, Sqlite>, sqlx::Error> {
        self.pool.begin_with("BEGIN IMMEDIATE").await
    }

    /// Closes all connections, flushing the write-ahead log.
    pub async fn close(self) {
        self.pool.close().await;
    }

    /// Opportunity ids that start with `prefix` (`opp_1a2b…`), at most
    /// `limit`, so short ids typed by people can be resolved.
    pub async fn opportunities_with_prefix(
        &self,
        prefix: &str,
        limit: usize,
    ) -> Result<Vec<OpportunityId>, StorageError> {
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT opportunity_id FROM jobs WHERE opportunity_id LIKE ? ESCAPE '\\' \
             ORDER BY opportunity_id LIMIT ?",
        )
        .bind(like_prefix(prefix))
        .bind(i64::try_from(limit).unwrap_or(i64::MAX))
        .fetch_all(&self.pool)
        .await
        .map_err(query_error("resolving an opportunity id"))?;
        rows.iter()
            .map(|id| {
                id.parse()
                    .map_err(|e| corrupt(id, format!("opportunity_id: {e}")))
            })
            .collect()
    }

    /// Job ids that start with `prefix` (`job_1a2b…`), at most `limit`.
    pub async fn jobs_with_prefix(
        &self,
        prefix: &str,
        limit: usize,
    ) -> Result<Vec<JobId>, StorageError> {
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM jobs WHERE id LIKE ? ESCAPE '\\' ORDER BY id LIMIT ?",
        )
        .bind(like_prefix(prefix))
        .bind(i64::try_from(limit).unwrap_or(i64::MAX))
        .fetch_all(&self.pool)
        .await
        .map_err(query_error("resolving a job id"))?;
        rows.iter()
            .map(|id| id.parse().map_err(|e| corrupt(id, format!("id: {e}"))))
            .collect()
    }

    /// When each source was last read successfully (a full listing or a
    /// "not modified" answer). Sources never read are absent.
    pub async fn last_checked(&self) -> Result<HashMap<SourceKey, DateTime<Utc>>, StorageError> {
        let rows = sqlx::query(
            "SELECT source_kind, source_instance, MAX(finished_at) AS at FROM source_scans \
             WHERE status IN ('listing', 'not_modified') GROUP BY source_kind, source_instance",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(query_error("loading when sources were last read"))?;
        let mut out = HashMap::with_capacity(rows.len());
        for row in rows {
            let kind: String = row
                .try_get("source_kind")
                .map_err(column_error("source_scans"))?;
            let instance: String = row
                .try_get("source_instance")
                .map_err(column_error("source_scans"))?;
            let at: String = row.try_get("at").map_err(column_error("source_scans"))?;
            let Ok(key) = SourceKey::new(&kind, &instance) else {
                continue;
            };
            let at = decode_timestamp(&at)
                .map_err(|e| corrupt(&format!("scan of {key}"), e.to_string()))?;
            out.insert(key, at);
        }
        Ok(out)
    }

    /// The latest `per_source` scans of every source, newest first.
    pub async fn recent_scans(
        &self,
        per_source: usize,
    ) -> Result<HashMap<SourceKey, Vec<ScanRecord>>, StorageError> {
        let rows = sqlx::query(
            "SELECT source_kind, source_instance, finished_at, status, received, error FROM ( \
               SELECT *, ROW_NUMBER() OVER ( \
                 PARTITION BY source_kind, source_instance ORDER BY finished_at DESC, id DESC \
               ) AS n FROM source_scans \
             ) WHERE n <= ? ORDER BY source_kind, source_instance, n",
        )
        .bind(i64::try_from(per_source).unwrap_or(i64::MAX))
        .fetch_all(&self.pool)
        .await
        .map_err(query_error("loading recent source scans"))?;
        let mut out: HashMap<SourceKey, Vec<ScanRecord>> = HashMap::new();
        for row in rows {
            let get = |name: &str| -> Result<String, StorageError> {
                row.try_get(name).map_err(column_error("source_scans"))
            };
            let Ok(key) = SourceKey::new(&get("source_kind")?, &get("source_instance")?) else {
                continue;
            };
            let at = get("finished_at")?;
            let finished_at = decode_timestamp(&at)
                .map_err(|e| corrupt(&format!("scan of {key}"), e.to_string()))?;
            let received: i64 = row
                .try_get("received")
                .map_err(column_error("source_scans"))?;
            let error: Option<String> =
                row.try_get("error").map_err(column_error("source_scans"))?;
            out.entry(key).or_default().push(ScanRecord {
                finished_at,
                status: get("status")?,
                received: u64::try_from(received).unwrap_or(0),
                error,
            });
        }
        Ok(out)
    }

    /// Counts for diagnostics (`narrow doctor`).
    pub async fn stats(&self) -> Result<StoreStats, StorageError> {
        let count = |sql: &'static str| {
            let pool = self.pool.clone();
            async move {
                sqlx::query_scalar::<_, i64>(sql)
                    .fetch_one(&pool)
                    .await
                    .map(|n| u64::try_from(n).unwrap_or(0))
                    .map_err(query_error("counting stored records"))
            }
        };
        let latest_migration: Option<i64> =
            sqlx::query_scalar("SELECT MAX(version) FROM _sqlx_migrations WHERE success = 1")
                .fetch_one(&self.pool)
                .await
                .map_err(query_error("reading the schema version"))?;
        Ok(StoreStats {
            jobs: count("SELECT COUNT(*) FROM jobs").await?,
            open_jobs: count("SELECT COUNT(*) FROM jobs WHERE status = 'open'").await?,
            open_opportunities: count(
                "SELECT COUNT(DISTINCT opportunity_id) FROM jobs WHERE status = 'open'",
            )
            .await?,
            verifications: count("SELECT COUNT(*) FROM job_verifications").await?,
            feedback: count("SELECT COUNT(*) FROM opportunity_feedback WHERE action != 'seen'")
                .await?,
            migrations_applied: count("SELECT COUNT(*) FROM _sqlx_migrations WHERE success = 1")
                .await?,
            migrations_known: MIGRATOR.iter().count() as u64,
            latest_migration,
        })
    }
}

/// What a store holds, for diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreStats {
    pub jobs: u64,
    pub open_jobs: u64,
    pub open_opportunities: u64,
    pub verifications: u64,
    /// Feedback events other than "looked at".
    pub feedback: u64,
    pub migrations_applied: u64,
    /// Migrations this build knows about.
    pub migrations_known: u64,
    pub latest_migration: Option<i64>,
}

/// A `LIKE` pattern matching values that start with `prefix` literally.
fn like_prefix(prefix: &str) -> String {
    let mut pattern = String::with_capacity(prefix.len() + 1);
    for c in prefix.chars() {
        if matches!(c, '%' | '_' | '\\') {
            pattern.push('\\');
        }
        pattern.push(c);
    }
    pattern.push('%');
    pattern
}

/// Inserts a job exactly as it was stored elsewhere (an imported state
/// file), unless a job with its id already exists. Returns whether it was
/// inserted. The record keeps its own timestamps and status; the next scan
/// of its source reconciles it through the normal lifecycle.
pub(crate) async fn import_job(
    tx: &mut SqliteConnection,
    record: &JobRecord,
) -> Result<bool, StorageError> {
    let id = record.id.to_string();
    if record.posting.id() != record.id {
        return Err(corrupt(
            &id,
            "the id does not match the posting's source and source id".into(),
        ));
    }
    let exists: Option<i64> = sqlx::query_scalar("SELECT 1 FROM jobs WHERE id = ?")
        .bind(&id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(query_error("checking for an imported job"))?;
    if exists.is_some() {
        return Ok(false);
    }
    let values = ContentValues::from_posting(
        &record.posting,
        &record.posting.fingerprint().to_hex(),
        &record.posting.content_fingerprint().to_hex(),
    )?;
    values
        .bind(sqlx::query(&INSERT_SQL).bind(&id))
        .bind(encode_timestamp(record.first_seen_at))
        .bind(encode_timestamp(record.last_seen_at))
        .bind(encode_timestamp(record.content_updated_at))
        .bind(record.status.as_str())
        .bind(record.opportunity_id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(query_error("inserting an imported job"))?;
    if let Some(closed_at) = record.closed_at {
        sqlx::query("UPDATE jobs SET closed_at = ? WHERE id = ?")
            .bind(encode_timestamp(closed_at))
            .bind(&id)
            .execute(&mut *tx)
            .await
            .map_err(query_error("inserting an imported job"))?;
    }
    replace_evidence(&mut *tx, &id, &record.posting).await?;
    Ok(true)
}

#[async_trait]
impl JobRepository for SqliteJobStore {
    async fn begin_run(&self, started_at: DateTime<Utc>) -> Result<RunId, StorageError> {
        let result = sqlx::query("INSERT INTO discovery_runs (started_at) VALUES (?)")
            .bind(encode_timestamp(started_at))
            .execute(&self.pool)
            .await
            .map_err(query_error("recording a discovery run"))?;
        Ok(RunId(result.last_insert_rowid()))
    }

    async fn finish_run(&self, run: RunId, summary: &RunSummary) -> Result<(), StorageError> {
        let c = &summary.counts;
        sqlx::query(
            "UPDATE discovery_runs SET finished_at = ?, sources = ?, failed = ?, received = ?, \
             normalized = ?, rejected = ?, new = ?, updated = ?, unchanged = ?, reopened = ?, \
             closed = ?, multi_source_opportunities = ? WHERE id = ?",
        )
        .bind(encode_timestamp(summary.finished_at))
        .bind(count(summary.sources))
        .bind(count(summary.failed))
        .bind(count(c.received))
        .bind(count(c.normalized))
        .bind(count(c.rejected))
        .bind(count(c.inserted))
        .bind(count(c.updated))
        .bind(count(c.unchanged))
        .bind(count(c.reopened))
        .bind(count(c.closed))
        .bind(count(summary.multi_source_opportunities))
        .bind(run.0)
        .execute(&self.pool)
        .await
        .map_err(query_error("finishing a discovery run"))?;
        Ok(())
    }

    async fn last_listing(&self, source: &SourceKey) -> Result<Option<LastListing>, StorageError> {
        let row = sqlx::query(
            "SELECT finished_at, complete, received, validator, revision FROM source_scans \
             WHERE source_kind = ? AND source_instance = ? AND status = 'listing' \
             ORDER BY id DESC LIMIT 1",
        )
        .bind(source.kind())
        .bind(source.instance())
        .fetch_optional(&self.pool)
        .await
        .map_err(query_error("loading the last scan of a source"))?;
        let Some(row) = row else {
            return Ok(None);
        };
        let scan_error = |e: String| corrupt(&format!("scan of {source}"), e);
        let finished_at: String = row
            .try_get("finished_at")
            .map_err(|e| scan_error(e.to_string()))?;
        let received: i64 = row
            .try_get("received")
            .map_err(|e| scan_error(e.to_string()))?;
        Ok(Some(LastListing {
            finished_at: decode_timestamp(&finished_at).map_err(|e| scan_error(e.to_string()))?,
            complete: row
                .try_get("complete")
                .map_err(|e| scan_error(e.to_string()))?,
            received: usize::try_from(received).unwrap_or(0),
            validator: row
                .try_get("validator")
                .map_err(|e| scan_error(e.to_string()))?,
            revision: row
                .try_get("revision")
                .map_err(|e| scan_error(e.to_string()))?,
        }))
    }

    async fn apply_scan(&self, scan: &ScanWrite<'_>) -> Result<ScanResult, StorageError> {
        let mut tx = self
            .begin_write()
            .await
            .map_err(query_error("starting a transaction"))?;
        let observed = encode_timestamp(scan.observed_at);
        let mut counts = scan.counts;
        let mut result = ScanResult::default();

        let row = match &scan.body {
            ScanBody::Failed { error } => ScanRow {
                status: "failed",
                error: Some(error),
                ..ScanRow::default()
            },
            ScanBody::NotModified => {
                let touched = sqlx::query(
                    "UPDATE jobs SET last_seen_at = ? WHERE source_kind = ? \
                     AND source_instance = ? AND status = 'open'",
                )
                .bind(&observed)
                .bind(scan.source.kind())
                .bind(scan.source.instance())
                .execute(&mut *tx)
                .await
                .map_err(query_error("marking jobs as seen"))?
                .rows_affected();
                result.touched = usize::try_from(touched).unwrap_or(usize::MAX);
                counts.unchanged = result.touched;
                ScanRow {
                    status: "not_modified",
                    complete: true,
                    ..ScanRow::default()
                }
            }
            ScanBody::Listing {
                postings,
                complete,
                close_missing,
                closing_withheld,
                retain,
                validator,
            } => {
                let observation = Observation {
                    source: scan.source,
                    observed_at: scan.observed_at,
                    run: Some(scan.run),
                };
                apply_listing(
                    &mut tx,
                    &observation,
                    postings,
                    *close_missing,
                    retain,
                    &mut result,
                )
                .await?;
                for outcome in &result.outcomes {
                    counts.record(*outcome);
                }
                counts.closed = result.closed.len();
                ScanRow {
                    status: "listing",
                    complete: *complete,
                    closing_applied: *close_missing,
                    closing_withheld: *closing_withheld,
                    validator: *validator,
                    error: None,
                }
            }
        };
        insert_scan(&mut tx, scan, &row, &counts).await?;

        tx.commit()
            .await
            .map_err(query_error("committing a source scan"))?;
        debug!(source = %scan.source, status = row.status, "scan stored");
        Ok(result)
    }

    async fn identity_index(&self) -> Result<Vec<IdentityEntry>, StorageError> {
        let rows = sqlx::query(
            "SELECT id, source_kind, source_instance, company, title, first_seen_at, \
             opportunity_id FROM jobs",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(query_error("loading job identities"))?;
        let evidence_rows = sqlx::query("SELECT job_id, key FROM job_evidence")
            .fetch_all(&self.pool)
            .await
            .map_err(query_error("loading identity evidence"))?;

        let mut evidence: HashMap<String, Vec<String>> = HashMap::new();
        for row in &evidence_rows {
            let job: String = row
                .try_get("job_id")
                .map_err(column_error("job_evidence"))?;
            let key: String = row.try_get("key").map_err(column_error("job_evidence"))?;
            evidence.entry(job).or_default().push(key);
        }

        rows.iter()
            .map(|row| {
                let raw_id: String = row.try_get("id").map_err(column_error("jobs"))?;
                let text = |column: &'static str| -> Result<String, StorageError> {
                    row.try_get(column)
                        .map_err(|e| corrupt(&raw_id, format!("{column}: {e}")))
                };
                let id: JobId = raw_id
                    .parse()
                    .map_err(|e| corrupt(&raw_id, format!("id: {e}")))?;
                Ok(IdentityEntry {
                    job: id,
                    source: SourceKey::new(&text("source_kind")?, &text("source_instance")?)
                        .map_err(|e| corrupt(&raw_id, format!("source: {e}")))?,
                    company: text("company")?,
                    title: text("title")?,
                    first_seen_at: decode_timestamp(&text("first_seen_at")?)
                        .map_err(|e| corrupt(&raw_id, format!("first_seen_at: {e}")))?,
                    opportunity: text("opportunity_id")?
                        .parse()
                        .map_err(|e| corrupt(&raw_id, format!("opportunity_id: {e}")))?,
                    evidence: evidence.remove(&raw_id).unwrap_or_default(),
                })
            })
            .collect()
    }

    async fn assign_opportunities(
        &self,
        assignments: &[(JobId, OpportunityId)],
    ) -> Result<(), StorageError> {
        if assignments.is_empty() {
            return Ok(());
        }
        let mut tx = self
            .begin_write()
            .await
            .map_err(query_error("starting a transaction"))?;
        for (job, opportunity) in assignments {
            sqlx::query("UPDATE jobs SET opportunity_id = ? WHERE id = ?")
                .bind(opportunity.to_string())
                .bind(job.to_string())
                .execute(&mut *tx)
                .await
                .map_err(query_error("assigning opportunities"))?;
        }
        tx.commit()
            .await
            .map_err(query_error("committing opportunities"))?;
        Ok(())
    }

    async fn get(&self, id: JobId) -> Result<Option<JobRecord>, StorageError> {
        let row = sqlx::query("SELECT * FROM jobs WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(query_error("loading a job"))?;
        row.as_ref().map(decode_record).transpose()
    }

    async fn opportunity_records(&self, id: OpportunityId) -> Result<Vec<JobRecord>, StorageError> {
        let rows =
            sqlx::query("SELECT * FROM jobs WHERE opportunity_id = ? ORDER BY first_seen_at, id")
                .bind(id.to_string())
                .fetch_all(&self.pool)
                .await
                .map_err(query_error("loading an opportunity"))?;
        rows.iter().map(decode_record).collect()
    }

    async fn history(&self, id: JobId) -> Result<Vec<JobEvent>, StorageError> {
        let raw_id = id.to_string();
        let rows = sqlx::query(
            "SELECT kind, at, run_id, changed_fields, previous FROM job_events \
             WHERE job_id = ? ORDER BY id",
        )
        .bind(&raw_id)
        .fetch_all(&self.pool)
        .await
        .map_err(query_error("loading job history"))?;
        rows.iter()
            .map(|row| {
                let bad = |e: String| corrupt(&raw_id, format!("history: {e}"));
                let kind: String = row.try_get("kind").map_err(|e| bad(e.to_string()))?;
                let at: String = row.try_get("at").map_err(|e| bad(e.to_string()))?;
                let run: Option<i64> = row.try_get("run_id").map_err(|e| bad(e.to_string()))?;
                let changed: String = row
                    .try_get("changed_fields")
                    .map_err(|e| bad(e.to_string()))?;
                let previous: Option<String> =
                    row.try_get("previous").map_err(|e| bad(e.to_string()))?;
                Ok(JobEvent {
                    kind: JobEventKind::from_canonical(&kind)
                        .ok_or_else(|| bad(format!("unknown kind {kind:?}")))?,
                    at: decode_timestamp(&at).map_err(|e| bad(e.to_string()))?,
                    run: run.map(RunId),
                    changed_fields: serde_json::from_str(&changed)
                        .map_err(|e| bad(e.to_string()))?,
                    previous: previous
                        .map(|json| serde_json::from_str::<JobSnapshot>(&json))
                        .transpose()
                        .map_err(|e| bad(e.to_string()))?,
                })
            })
            .collect()
    }

    async fn search(&self, query: &JobQuery) -> Result<Vec<JobRecord>, StorageError> {
        let mut builder = if query.distinct_opportunities {
            // One row per opportunity: an open record if there is one, then
            // the earliest seen.
            let mut b = QueryBuilder::<Sqlite>::new(
                "SELECT * FROM (SELECT jobs.*, ROW_NUMBER() OVER (PARTITION BY opportunity_id \
                 ORDER BY status = 'open' DESC, first_seen_at, id) AS opportunity_rank \
                 FROM jobs",
            );
            push_filters(&mut b, query);
            b.push(") WHERE opportunity_rank = 1");
            b
        } else {
            let mut b = QueryBuilder::<Sqlite>::new("SELECT * FROM jobs");
            push_filters(&mut b, query);
            b
        };
        builder.push(" ORDER BY posted_at IS NULL, posted_at DESC, first_seen_at DESC, id");
        if let Some(limit) = query.limit {
            builder
                .push(" LIMIT ")
                .push_bind(i64::try_from(limit).unwrap_or(i64::MAX));
        }
        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(query_error("searching jobs"))?;
        rows.iter().map(decode_record).collect()
    }

    async fn count(&self, query: &JobQuery) -> Result<u64, StorageError> {
        let mut builder = QueryBuilder::<Sqlite>::new(if query.distinct_opportunities {
            "SELECT COUNT(DISTINCT opportunity_id) FROM jobs"
        } else {
            "SELECT COUNT(*) FROM jobs"
        });
        push_filters(&mut builder, query);
        let count: i64 = builder
            .build_query_scalar()
            .fetch_one(&self.pool)
            .await
            .map_err(query_error("counting jobs"))?;
        Ok(u64::try_from(count).unwrap_or(0))
    }
}

/// Who saw postings, when, and in which discovery run (none for a
/// verification).
pub(crate) struct Observation<'a> {
    pub source: &'a SourceKey,
    pub observed_at: DateTime<Utc>,
    pub run: Option<RunId>,
}

/// Applies a listing inside the scan's transaction.
pub(crate) async fn apply_listing(
    tx: &mut SqliteConnection,
    scan: &Observation<'_>,
    postings: &[JobPosting],
    close_missing: bool,
    retain: &[JobId],
    result: &mut ScanResult,
) -> Result<(), StorageError> {
    let observed = encode_timestamp(scan.observed_at);
    let stored = load_stored(&mut *tx, scan.source).await?;
    let retain: HashSet<JobId> = retain.iter().copied().collect();
    let closing = if close_missing {
        Closing::CloseMissing { retain: &retain }
    } else {
        Closing::Skip
    };
    let plan = plan_scan(&stored, postings, closing);

    let mut touched = Vec::new();
    for (posting, planned) in postings.iter().zip(&plan.postings) {
        let id = planned.id.to_string();
        result.outcomes.push(planned.action.outcome());
        if planned.action == Action::Touch {
            touched.push(id);
            continue;
        }
        let values = ContentValues::from_posting(
            posting,
            &planned.fingerprint,
            &planned.content_fingerprint,
        )?;
        match planned.action {
            Action::Touch => {}
            Action::Insert => {
                values
                    .bind(sqlx::query(&INSERT_SQL).bind(&id))
                    .bind(&observed)
                    .bind(&observed)
                    .bind(&observed)
                    .bind(JobStatus::Open.as_str())
                    .bind(OpportunityId::founded_by(planned.id).to_string())
                    .execute(&mut *tx)
                    .await
                    .map_err(query_error("inserting a job"))?;
                insert_event(&mut *tx, scan, &id, JobEventKind::New, &[], None).await?;
            }
            Action::Refresh => {
                rewrite(&mut *tx, &values, &id, &observed, None).await?;
            }
            Action::Update | Action::Reopen { .. } => {
                let content_changed = matches!(
                    planned.action,
                    Action::Update
                        | Action::Reopen {
                            content_changed: true
                        }
                );
                let (changed, previous) = if content_changed {
                    let before = load_snapshot(&mut *tx, planned.id).await?;
                    let fields: Vec<&str> = before.changed_fields(&posting.snapshot());
                    (fields, Some(before))
                } else {
                    (Vec::new(), None)
                };
                rewrite(
                    &mut *tx,
                    &values,
                    &id,
                    &observed,
                    content_changed.then_some(observed.as_str()),
                )
                .await?;
                let kind = if planned.action == Action::Update {
                    JobEventKind::Updated
                } else {
                    JobEventKind::Reopened
                };
                insert_event(&mut *tx, scan, &id, kind, &changed, previous.as_ref()).await?;
            }
        }
        if planned.action != Action::Touch {
            replace_evidence(&mut *tx, &id, posting).await?;
        }
    }

    for chunk in touched.chunks(BATCH) {
        let mut builder = QueryBuilder::<Sqlite>::new("UPDATE jobs SET last_seen_at = ");
        builder.push_bind(&observed).push(" WHERE id IN (");
        let mut ids = builder.separated(", ");
        for id in chunk {
            ids.push_bind(id);
        }
        builder.push(")");
        builder
            .build()
            .execute(&mut *tx)
            .await
            .map_err(query_error("marking jobs as seen"))?;
    }

    for id in &plan.close {
        let id = id.to_string();
        sqlx::query("UPDATE jobs SET status = 'closed', closed_at = ? WHERE id = ?")
            .bind(&observed)
            .bind(&id)
            .execute(&mut *tx)
            .await
            .map_err(query_error("closing a job"))?;
        insert_event(&mut *tx, scan, &id, JobEventKind::Closed, &[], None).await?;
    }
    result.closed = plan.close;
    Ok(())
}

async fn load_stored(
    tx: &mut SqliteConnection,
    source: &SourceKey,
) -> Result<HashMap<JobId, StoredJob>, StorageError> {
    let rows = sqlx::query(
        "SELECT id, status, fingerprint, content_fingerprint FROM jobs \
         WHERE source_kind = ? AND source_instance = ?",
    )
    .bind(source.kind())
    .bind(source.instance())
    .fetch_all(&mut *tx)
    .await
    .map_err(query_error("loading stored jobs of a source"))?;
    rows.iter()
        .map(|row| {
            let raw_id: String = row.try_get("id").map_err(column_error("jobs"))?;
            let bad = |e: String| corrupt(&raw_id, e);
            let id: JobId = raw_id.parse().map_err(|e| bad(format!("id: {e}")))?;
            let status: String = row.try_get("status").map_err(|e| bad(e.to_string()))?;
            Ok((
                id,
                StoredJob {
                    status: JobStatus::from_canonical(&status)
                        .ok_or_else(|| bad(format!("unknown status {status:?}")))?,
                    fingerprint: row.try_get("fingerprint").map_err(|e| bad(e.to_string()))?,
                    content_fingerprint: row
                        .try_get("content_fingerprint")
                        .map_err(|e| bad(e.to_string()))?,
                },
            ))
        })
        .collect()
}

async fn load_snapshot(tx: &mut SqliteConnection, id: JobId) -> Result<JobSnapshot, StorageError> {
    let row = sqlx::query("SELECT * FROM jobs WHERE id = ?")
        .bind(id.to_string())
        .fetch_one(&mut *tx)
        .await
        .map_err(query_error("loading a job's previous version"))?;
    Ok(decode_record(&row)?.posting.snapshot())
}

async fn rewrite(
    tx: &mut SqliteConnection,
    values: &ContentValues,
    id: &str,
    observed: &str,
    content_updated_at: Option<&str>,
) -> Result<(), StorageError> {
    values
        .bind(sqlx::query(&REWRITE_SQL))
        .bind(observed)
        .bind(content_updated_at)
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(query_error("updating a job"))?;
    Ok(())
}

async fn replace_evidence(
    tx: &mut SqliteConnection,
    id: &str,
    posting: &JobPosting,
) -> Result<(), StorageError> {
    sqlx::query("DELETE FROM job_evidence WHERE job_id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(query_error("replacing identity evidence"))?;
    for key in evidence_keys(posting) {
        sqlx::query("INSERT INTO job_evidence (job_id, key) VALUES (?, ?)")
            .bind(id)
            .bind(key)
            .execute(&mut *tx)
            .await
            .map_err(query_error("storing identity evidence"))?;
    }
    Ok(())
}

async fn insert_event(
    tx: &mut SqliteConnection,
    scan: &Observation<'_>,
    job_id: &str,
    kind: JobEventKind,
    changed_fields: &[&str],
    previous: Option<&JobSnapshot>,
) -> Result<(), StorageError> {
    let encode_error = |e: serde_json::Error| StorageError::Query {
        operation: "encoding job history",
        source: Box::new(e),
    };
    let changed = serde_json::to_string(changed_fields).map_err(encode_error)?;
    let previous = previous
        .map(serde_json::to_string)
        .transpose()
        .map_err(encode_error)?;
    sqlx::query(
        "INSERT INTO job_events (job_id, run_id, kind, at, changed_fields, previous) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(job_id)
    .bind(scan.run.map(|r| r.0))
    .bind(kind.as_str())
    .bind(encode_timestamp(scan.observed_at))
    .bind(changed)
    .bind(previous)
    .execute(&mut *tx)
    .await
    .map_err(query_error("recording job history"))?;
    Ok(())
}

#[derive(Default)]
struct ScanRow<'a> {
    status: &'static str,
    complete: bool,
    closing_applied: bool,
    closing_withheld: Option<&'static str>,
    validator: Option<&'a str>,
    error: Option<&'a str>,
}

async fn insert_scan(
    tx: &mut SqliteConnection,
    scan: &ScanWrite<'_>,
    row: &ScanRow<'_>,
    c: &IngestCounts,
) -> Result<(), StorageError> {
    sqlx::query(
        "INSERT INTO source_scans (run_id, source_kind, source_instance, status, complete, \
         closing_applied, closing_withheld, started_at, finished_at, received, normalized, \
         rejected, skipped, duplicates, new, updated, unchanged, reopened, closed, validator, \
         revision, error) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(scan.run.0)
    .bind(scan.source.kind())
    .bind(scan.source.instance())
    .bind(row.status)
    .bind(row.complete)
    .bind(row.closing_applied)
    .bind(row.closing_withheld)
    .bind(encode_timestamp(scan.started_at))
    .bind(encode_timestamp(scan.observed_at))
    .bind(count(c.received))
    .bind(count(c.normalized))
    .bind(count(c.rejected))
    .bind(count(c.skipped))
    .bind(count(c.duplicates))
    .bind(count(c.inserted))
    .bind(count(c.updated))
    .bind(count(c.unchanged))
    .bind(count(c.reopened))
    .bind(count(c.closed))
    .bind(row.validator)
    .bind(CANONICAL_REVISION)
    .bind(row.error)
    .execute(&mut *tx)
    .await
    .map_err(query_error("recording a source scan"))?;
    Ok(())
}

fn count(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn push_filters(builder: &mut QueryBuilder<'_, Sqlite>, query: &JobQuery) {
    builder.push(" WHERE 1 = 1");
    for term in &query.terms {
        // search_text is " word word ... " (normalized in Rust), so matching
        // " term" finds terms at the start of a word. Terms are normalized
        // the same way, which makes case handling Unicode-aware; escaping is
        // defensive since normalized terms contain no LIKE wildcards.
        let pattern = format!("% {}%", escape_like(&search_key(term)));
        builder
            .push(" AND search_text LIKE ")
            .push_bind(pattern)
            .push(" ESCAPE '\\'");
    }
    if !query.sources.is_empty() {
        builder.push(" AND (");
        for (i, source) in query.sources.iter().enumerate() {
            if i > 0 {
                builder.push(" OR ");
            }
            builder
                .push("(source_kind = ")
                .push_bind(source.kind().to_owned())
                .push(" AND source_instance = ")
                .push_bind(source.instance().to_owned())
                .push(")");
        }
        builder.push(")");
    }
    if let Some(status) = query.status {
        builder.push(" AND status = ").push_bind(status.as_str());
    }
    if let Some(since) = query.seen_since {
        builder
            .push(" AND last_seen_at >= ")
            .push_bind(encode_timestamp(since));
    }
}

fn escape_like(term: &str) -> String {
    let mut out = String::with_capacity(term.len());
    for c in term.chars() {
        if matches!(c, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// A posting flattened into column values.
struct ContentValues {
    source_kind: String,
    source_instance: String,
    source_job_id: Option<String>,
    fetched_from: Option<String>,
    url: String,
    apply_url: Option<String>,
    company: String,
    title: String,
    department: Option<String>,
    team: Option<String>,
    location: Option<String>,
    locations: String,
    employment_type: Option<String>,
    workplace_type: Option<String>,
    is_remote: Option<bool>,
    compensation: Option<String>,
    work_authorization: Option<String>,
    description_text: Option<String>,
    description_html: Option<String>,
    posted_at: Option<String>,
    source_updated_at: Option<String>,
    search_text: String,
    fingerprint: String,
    content_fingerprint: String,
}

impl ContentValues {
    fn from_posting(
        p: &JobPosting,
        fingerprint: &str,
        content_fingerprint: &str,
    ) -> Result<Self, StorageError> {
        let encode_error = |e: serde_json::Error| StorageError::Query {
            operation: "encoding a job for storage",
            source: Box::new(e),
        };
        Ok(Self {
            source_kind: p.provenance.source.kind().to_owned(),
            source_instance: p.provenance.source.instance().to_owned(),
            source_job_id: p.provenance.source_record_id.clone(),
            fetched_from: p.provenance.fetched_from.as_ref().map(|u| u.to_string()),
            url: p.url.to_string(),
            apply_url: p.apply_url.as_ref().map(|u| u.to_string()),
            company: p.company.clone(),
            title: p.title.clone(),
            department: p.department.clone(),
            team: p.team.clone(),
            location: p.location.clone(),
            locations: serde_json::to_string(&p.locations).map_err(encode_error)?,
            employment_type: p.employment_type.as_ref().map(|t| t.as_str().to_owned()),
            workplace_type: p.workplace_type.as_ref().map(|t| t.as_str().to_owned()),
            is_remote: p.is_remote,
            compensation: p
                .compensation
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map_err(encode_error)?,
            work_authorization: p.work_authorization.clone(),
            description_text: p.description_text.clone(),
            description_html: p.description_html.clone(),
            posted_at: p.posted_at.map(encode_timestamp),
            source_updated_at: p.source_updated_at.map(encode_timestamp),
            search_text: p.search_document(),
            fingerprint: fingerprint.to_owned(),
            content_fingerprint: content_fingerprint.to_owned(),
        })
    }

    /// Binds the values in [`CONTENT_COLUMNS`] order.
    fn bind<'q>(
        &'q self,
        query: sqlx::query::Query<'q, Sqlite, SqliteArguments<'q>>,
    ) -> sqlx::query::Query<'q, Sqlite, SqliteArguments<'q>> {
        query
            .bind(&self.source_kind)
            .bind(&self.source_instance)
            .bind(&self.source_job_id)
            .bind(&self.fetched_from)
            .bind(&self.url)
            .bind(&self.apply_url)
            .bind(&self.company)
            .bind(&self.title)
            .bind(&self.department)
            .bind(&self.team)
            .bind(&self.location)
            .bind(&self.locations)
            .bind(&self.employment_type)
            .bind(&self.workplace_type)
            .bind(self.is_remote)
            .bind(&self.compensation)
            .bind(&self.description_text)
            .bind(&self.description_html)
            .bind(&self.posted_at)
            .bind(&self.source_updated_at)
            .bind(&self.search_text)
            .bind(&self.fingerprint)
            .bind(&self.content_fingerprint)
            .bind(&self.work_authorization)
    }
}

fn decode_record(row: &SqliteRow) -> Result<JobRecord, StorageError> {
    let raw_id: String = row
        .try_get("id")
        .map_err(|e| corrupt("<unknown>", format!("id: {e}")))?;
    let get_text = |column: &'static str| -> Result<String, StorageError> {
        row.try_get(column)
            .map_err(|e| corrupt(&raw_id, format!("{column}: {e}")))
    };
    let get_opt = |column: &'static str| -> Result<Option<String>, StorageError> {
        row.try_get(column)
            .map_err(|e| corrupt(&raw_id, format!("{column}: {e}")))
    };
    let url = |column: &'static str, value: &str| {
        CanonicalUrl::parse(value).map_err(|e| corrupt(&raw_id, format!("{column}: {e}")))
    };
    let timestamp = |column: &'static str, value: &str| {
        decode_timestamp(value).map_err(|e| corrupt(&raw_id, format!("{column}: {e}")))
    };

    let id: JobId = raw_id
        .parse()
        .map_err(|e| corrupt(&raw_id, format!("id: {e}")))?;
    let source = SourceKey::new(&get_text("source_kind")?, &get_text("source_instance")?)
        .map_err(|e| corrupt(&raw_id, format!("source: {e}")))?;
    let locations: Vec<SourceLocation> = serde_json::from_str(&get_text("locations")?)
        .map_err(|e| corrupt(&raw_id, format!("locations: {e}")))?;
    let compensation: Option<Compensation> = get_opt("compensation")?
        .map(|json| serde_json::from_str(&json))
        .transpose()
        .map_err(|e| corrupt(&raw_id, format!("compensation: {e}")))?;
    let is_remote: Option<bool> = row
        .try_get("is_remote")
        .map_err(|e| corrupt(&raw_id, format!("is_remote: {e}")))?;
    let status = get_text("status")?;

    let posting = JobPosting {
        provenance: Provenance {
            source,
            source_record_id: get_opt("source_job_id")?,
            fetched_from: get_opt("fetched_from")?
                .map(|v| url("fetched_from", &v))
                .transpose()?,
        },
        url: url("url", &get_text("url")?)?,
        apply_url: get_opt("apply_url")?
            .map(|v| url("apply_url", &v))
            .transpose()?,
        company: get_text("company")?,
        title: get_text("title")?,
        department: get_opt("department")?,
        team: get_opt("team")?,
        location: get_opt("location")?,
        locations,
        employment_type: get_opt("employment_type")?
            .as_deref()
            .map(EmploymentType::from_canonical),
        workplace_type: get_opt("workplace_type")?
            .as_deref()
            .map(WorkplaceType::from_canonical),
        is_remote,
        compensation,
        work_authorization: get_opt("work_authorization")?,
        description_text: get_opt("description_text")?,
        description_html: get_opt("description_html")?,
        posted_at: get_opt("posted_at")?
            .map(|v| timestamp("posted_at", &v))
            .transpose()?,
        source_updated_at: get_opt("source_updated_at")?
            .map(|v| timestamp("source_updated_at", &v))
            .transpose()?,
    };

    Ok(JobRecord {
        id,
        posting,
        first_seen_at: timestamp("first_seen_at", &get_text("first_seen_at")?)?,
        last_seen_at: timestamp("last_seen_at", &get_text("last_seen_at")?)?,
        content_updated_at: timestamp("content_updated_at", &get_text("content_updated_at")?)?,
        status: JobStatus::from_canonical(&status)
            .ok_or_else(|| corrupt(&raw_id, format!("unknown status {status:?}")))?,
        closed_at: get_opt("closed_at")?
            .map(|v| timestamp("closed_at", &v))
            .transpose()?,
        opportunity_id: get_text("opportunity_id")?
            .parse()
            .map_err(|e| corrupt(&raw_id, format!("opportunity_id: {e}")))?,
    })
}

/// Fixed-width UTC RFC 3339 with microseconds, so values sort as text.
/// Sub-microsecond precision is dropped.
pub(crate) fn encode_timestamp(value: DateTime<Utc>) -> String {
    value.format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string()
}

pub(crate) fn decode_timestamp(value: &str) -> Result<DateTime<Utc>, chrono::ParseError> {
    DateTime::parse_from_rfc3339(value).map(|t| t.with_timezone(&Utc))
}

fn corrupt(id: &str, detail: String) -> StorageError {
    StorageError::Corrupt {
        id: id.to_owned(),
        detail,
    }
}

fn column_error(table: &'static str) -> impl Fn(sqlx::Error) -> StorageError {
    move |e| corrupt(&format!("<{table} row>"), e.to_string())
}

fn query_error(operation: &'static str) -> impl FnOnce(sqlx::Error) -> StorageError {
    move |source| StorageError::Query {
        operation,
        source: Box::new(source),
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use chrono::TimeZone;
    use jobhunt_core::UpsertOutcome;
    use jobhunt_jobs::{CompensationComponent, CompensationKind, PayInterval};

    use super::*;

    fn at(minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 25, 12, minute, 0).unwrap()
    }

    fn posting(instance: &str, native_id: &str, title: &str) -> JobPosting {
        JobPosting {
            provenance: Provenance {
                source: SourceKey::new("ashby", instance).unwrap(),
                source_record_id: Some(native_id.to_owned()),
                fetched_from: Some(
                    CanonicalUrl::parse(&format!(
                        "https://api.ashbyhq.com/posting-api/job-board/{instance}?includeCompensation=true"
                    ))
                    .unwrap(),
                ),
            },
            url: CanonicalUrl::parse(&format!("https://jobs.ashbyhq.com/{instance}/{native_id}"))
                .unwrap(),
            apply_url: Some(
                CanonicalUrl::parse(&format!(
                    "https://jobs.ashbyhq.com/{instance}/{native_id}/application"
                ))
                .unwrap(),
            ),
            company: instance.to_uppercase(),
            title: title.to_owned(),
            department: Some("Engineering".into()),
            team: Some("Platform".into()),
            location: Some("New York, NY (HQ)".into()),
            locations: vec![
                SourceLocation {
                    name: Some("New York, NY (HQ)".into()),
                    locality: Some("New York City".into()),
                    region: Some("NY".into()),
                    country: Some("USA".into()),
                },
                SourceLocation {
                    name: Some("Remote (Canada)".into()),
                    country: Some("Canada".into()),
                    ..Default::default()
                },
            ],
            employment_type: Some(EmploymentType::FullTime),
            workplace_type: Some(WorkplaceType::Hybrid),
            is_remote: Some(true),
            compensation: Some(Compensation {
                summary: Some("$211.4K – $290.6K • Offers Equity".into()),
                components: vec![
                    CompensationComponent {
                        kind: CompensationKind::Salary,
                        label: None,
                        currency: Some("USD".into()),
                        min: Some(211_400.0),
                        max: Some(290_600.0),
                        interval: Some(PayInterval::Year),
                    },
                    CompensationComponent {
                        kind: CompensationKind::EquityPercentage,
                        label: None,
                        currency: None,
                        min: None,
                        max: None,
                        interval: None,
                    },
                ],
            }),
            work_authorization: None,
            description_text: Some("Build things.\n\nWith care.".into()),
            description_html: Some("<p>Build things.</p><p>With care.</p>".into()),
            posted_at: Some(Utc.with_ymd_and_hms(2026, 4, 7, 17, 12, 35).unwrap()),
            source_updated_at: None,
        }
    }

    async fn store() -> SqliteJobStore {
        SqliteJobStore::open_in_memory().await.unwrap()
    }

    /// Persists one listing of `ashby:<instance>` in its own run.
    async fn scan(
        store: &SqliteJobStore,
        instance: &str,
        postings: &[JobPosting],
        observed_at: DateTime<Utc>,
        close_missing: bool,
    ) -> ScanResult {
        let source = SourceKey::new("ashby", instance).unwrap();
        let run = store.begin_run(observed_at).await.unwrap();
        store
            .apply_scan(&ScanWrite {
                run,
                source: &source,
                started_at: observed_at,
                observed_at,
                counts: IngestCounts {
                    received: postings.len(),
                    normalized: postings.len(),
                    ..IngestCounts::default()
                },
                body: ScanBody::Listing {
                    postings,
                    complete: close_missing,
                    close_missing,
                    closing_withheld: None,
                    retain: &[],
                    validator: Some("\"etag\""),
                },
            })
            .await
            .unwrap()
    }

    async fn tables(store: &SqliteJobStore) -> Vec<String> {
        sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE '\\_%' ESCAPE '\\' ORDER BY name",
        )
        .fetch_all(&store.pool)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn migrations_create_the_schema() {
        let store = store().await;
        let applied: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations")
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert_eq!(applied, MIGRATOR.iter().count() as i64);
        assert_eq!(
            tables(&store).await,
            vec![
                "discovery_runs",
                "eligibility_decisions",
                "fit_reviews",
                "job_events",
                "job_evidence",
                "job_verifications",
                "jobs",
                "opportunity_feedback",
                "opportunity_rankings",
                "profile_claims",
                "profile_documents",
                "profile_education",
                "profile_events",
                "profile_experiences",
                "profile_preference_statements",
                "profile_preferences",
                "profile_projects",
                "profile_skill_evidence",
                "profile_skills",
                "profile_taste",
                "profile_taste_briefs",
                "profiles",
                "source_scans",
                "sync_account",
                "sync_conflicts",
                "sync_entities",
                "sync_feedback"
            ]
        );
    }

    #[tokio::test]
    async fn upgrades_a_database_written_before_lifecycle_tracking() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobhunt.db");
        let legacy = posting("ramp", "1", "Engineer");

        // Build the database exactly as the first release left it: only the
        // first migration, with a row written by that release's code.
        {
            let pool = SqlitePool::connect_with(
                SqliteConnectOptions::new()
                    .filename(&path)
                    .create_if_missing(true),
            )
            .await
            .unwrap();
            let first_only = sqlx::migrate::Migrator {
                migrations: Cow::Owned(MIGRATOR.iter().take(1).cloned().collect()),
                ..sqlx::migrate::Migrator::DEFAULT
            };
            first_only.run(&pool).await.unwrap();
            let values = ContentValues::from_posting(&legacy, "legacy-fp", "unused").unwrap();
            let columns = CONTENT_COLUMNS[..22].join(", ");
            let placeholders = vec!["?"; 22 + 4].join(", ");
            let sql = format!(
                "INSERT INTO jobs (id, {columns}, first_seen_at, last_seen_at, content_updated_at) \
                 VALUES ({placeholders})"
            );
            let stamp = encode_timestamp(at(0));
            let mut query = sqlx::query(&sql).bind(legacy.id().to_string());
            // Bind the 22 legacy content columns (everything but
            // content_fingerprint), then the timestamps.
            query = query
                .bind(&values.source_kind)
                .bind(&values.source_instance)
                .bind(&values.source_job_id)
                .bind(&values.fetched_from)
                .bind(&values.url)
                .bind(&values.apply_url)
                .bind(&values.company)
                .bind(&values.title)
                .bind(&values.department)
                .bind(&values.team)
                .bind(&values.location)
                .bind(&values.locations)
                .bind(&values.employment_type)
                .bind(&values.workplace_type)
                .bind(values.is_remote)
                .bind(&values.compensation)
                .bind(&values.description_text)
                .bind(&values.description_html)
                .bind(&values.posted_at)
                .bind(&values.source_updated_at)
                .bind(&values.search_text)
                .bind(&values.fingerprint);
            query
                .bind(&stamp)
                .bind(&stamp)
                .bind(&stamp)
                .execute(&pool)
                .await
                .unwrap();
            pool.close().await;
        }

        let store = SqliteJobStore::open(&path).await.unwrap();
        let record = store.get(legacy.id()).await.unwrap().unwrap();
        assert_eq!(record.status, JobStatus::Open);
        assert_eq!(
            record.opportunity_id,
            OpportunityId::founded_by(legacy.id())
        );
        let history = store.history(legacy.id()).await.unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].kind, JobEventKind::New);
        assert_eq!(history[0].at, at(0));

        // Seeing the same posting again baselines it: UNCHANGED, no event.
        let result = scan(&store, "ramp", std::slice::from_ref(&legacy), at(5), true).await;
        assert_eq!(result.outcomes, vec![UpsertOutcome::Unchanged]);
        assert_eq!(store.history(legacy.id()).await.unwrap().len(), 1);
        let result = scan(&store, "ramp", std::slice::from_ref(&legacy), at(6), true).await;
        assert_eq!(result.outcomes, vec![UpsertOutcome::Unchanged]);
    }

    #[tokio::test]
    async fn reopening_a_database_file_keeps_data_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("jobhunt.db");

        let first = SqliteJobStore::open(&path).await.unwrap();
        scan(
            &first,
            "ramp",
            &[posting("ramp", "1", "Engineer")],
            at(0),
            true,
        )
        .await;
        first.close().await;

        let second = SqliteJobStore::open(&path).await.unwrap();
        assert_eq!(second.count(&JobQuery::default()).await.unwrap(), 1);
        let applied: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations")
            .fetch_one(&second.pool)
            .await
            .unwrap();
        assert_eq!(applied, MIGRATOR.iter().count() as i64);
    }

    #[tokio::test]
    async fn round_trips_every_field() {
        let store = store().await;
        let original = posting("ramp", "34413f8d", "Security Engineer, Cloud");
        scan(&store, "ramp", std::slice::from_ref(&original), at(0), true).await;

        let record = store.get(original.id()).await.unwrap().unwrap();
        assert_eq!(record.id, original.id());
        assert_eq!(record.posting, original);
        assert_eq!(record.first_seen_at, at(0));
        assert_eq!(record.last_seen_at, at(0));
        assert_eq!(record.content_updated_at, at(0));
        assert_eq!(record.status, JobStatus::Open);
        assert_eq!(record.closed_at, None);
        assert_eq!(
            record.opportunity_id,
            OpportunityId::founded_by(original.id())
        );
        assert_eq!(record.posting.fingerprint(), original.fingerprint());
    }

    #[tokio::test]
    async fn scans_classify_new_unchanged_updated_closed_and_reopened() {
        let store = store().await;
        let a = posting("ramp", "1", "Engineer");
        let b = posting("ramp", "2", "Designer");
        let c = posting("ramp", "3", "Writer");

        let first = scan(
            &store,
            "ramp",
            &[a.clone(), b.clone(), c.clone()],
            at(0),
            true,
        )
        .await;
        assert_eq!(first.outcomes, vec![UpsertOutcome::Inserted; 3]);

        let mut a2 = a.clone();
        a2.title = "Senior Engineer".into();
        let mut b_cosmetic = b.clone();
        b_cosmetic.description_html = Some("<p class=\"x\">Build things.</p>".into());
        let second = scan(
            &store,
            "ramp",
            &[a2.clone(), b_cosmetic.clone()],
            at(5),
            true,
        )
        .await;
        assert_eq!(
            second.outcomes,
            vec![UpsertOutcome::Updated, UpsertOutcome::Unchanged]
        );
        assert_eq!(second.closed, vec![c.id()]);

        let a_record = store.get(a.id()).await.unwrap().unwrap();
        assert_eq!(a_record.posting.title, "Senior Engineer");
        assert_eq!(a_record.first_seen_at, at(0));
        assert_eq!(a_record.last_seen_at, at(5));
        assert_eq!(a_record.content_updated_at, at(5));

        // The cosmetic change was stored but is not a content update.
        let b_record = store.get(b.id()).await.unwrap().unwrap();
        assert_eq!(
            b_record.posting.description_html,
            b_cosmetic.description_html
        );
        assert_eq!(b_record.content_updated_at, at(0));
        assert_eq!(b_record.last_seen_at, at(5));

        let c_record = store.get(c.id()).await.unwrap().unwrap();
        assert_eq!(c_record.status, JobStatus::Closed);
        assert_eq!(c_record.closed_at, Some(at(5)));
        assert_eq!(
            c_record.last_seen_at,
            at(0),
            "closing does not touch last_seen_at"
        );

        // c comes back with a raise.
        let mut c2 = c.clone();
        c2.compensation = None;
        let third = scan(&store, "ramp", &[a2.clone(), b_cosmetic, c2], at(9), true).await;
        assert_eq!(
            third.outcomes,
            vec![
                UpsertOutcome::Unchanged,
                UpsertOutcome::Unchanged,
                UpsertOutcome::Reopened
            ]
        );
        let c_record = store.get(c.id()).await.unwrap().unwrap();
        assert_eq!(c_record.status, JobStatus::Open);
        assert_eq!(c_record.closed_at, None);
        assert_eq!(c_record.content_updated_at, at(9));

        let history = store.history(c.id()).await.unwrap();
        let kinds: Vec<_> = history.iter().map(|e| e.kind).collect();
        assert_eq!(
            kinds,
            vec![
                JobEventKind::New,
                JobEventKind::Closed,
                JobEventKind::Reopened
            ]
        );
        assert_eq!(history[2].changed_fields, vec!["compensation"]);
        assert_eq!(
            history[2].previous.as_ref().unwrap().compensation,
            c.compensation,
            "the replaced version is kept"
        );

        let a_history = store.history(a.id()).await.unwrap();
        assert_eq!(a_history[1].kind, JobEventKind::Updated);
        assert_eq!(a_history[1].changed_fields, vec!["title"]);
        assert_eq!(a_history[1].previous.as_ref().unwrap().title, "Engineer");
        assert_eq!(store.count(&JobQuery::default()).await.unwrap(), 3);
    }

    #[tokio::test]
    async fn partial_scans_close_nothing() {
        let store = store().await;
        let a = posting("ramp", "1", "Engineer");
        let b = posting("ramp", "2", "Designer");
        scan(&store, "ramp", &[a.clone(), b.clone()], at(0), true).await;
        let partial = scan(&store, "ramp", std::slice::from_ref(&a), at(1), false).await;
        assert!(partial.closed.is_empty());
        assert_eq!(
            store.get(b.id()).await.unwrap().unwrap().status,
            JobStatus::Open
        );
    }

    #[tokio::test]
    async fn not_modified_and_failed_scans_change_no_state_or_history() {
        let store = store().await;
        let source = SourceKey::new("ashby", "ramp").unwrap();
        let open = posting("ramp", "1", "Engineer");
        let closed = posting("ramp", "2", "Designer");
        let elsewhere = posting("linear", "1", "Engineer");
        scan(&store, "ramp", &[open.clone(), closed.clone()], at(0), true).await;
        scan(&store, "ramp", std::slice::from_ref(&open), at(1), true).await;
        scan(
            &store,
            "linear",
            std::slice::from_ref(&elsewhere),
            at(1),
            true,
        )
        .await;
        let events = || async {
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM job_events")
                .fetch_one(&store.pool)
                .await
                .unwrap()
        };
        let events_before = events().await;
        let open_before = store.get(open.id()).await.unwrap().unwrap();
        let closed_before = store.get(closed.id()).await.unwrap().unwrap();
        assert_eq!(closed_before.status, JobStatus::Closed);

        let run = store.begin_run(at(5)).await.unwrap();
        for (minute, body) in [
            (5, ScanBody::NotModified),
            (6, ScanBody::Failed { error: "HTTP 503" }),
        ] {
            store
                .apply_scan(&ScanWrite {
                    run,
                    source: &source,
                    started_at: at(minute),
                    observed_at: at(minute),
                    counts: IngestCounts::default(),
                    body,
                })
                .await
                .unwrap();
        }

        // Only the open job of this source is marked seen, by the 304.
        let open_after = store.get(open.id()).await.unwrap().unwrap();
        assert_eq!(open_after.status, JobStatus::Open);
        assert_eq!(open_after.last_seen_at, at(5));
        assert_eq!(open_after.posting, open_before.posting);
        assert_eq!(
            open_after.content_updated_at,
            open_before.content_updated_at
        );
        // A closed job is not reopened or touched by a 304.
        assert_eq!(
            store.get(closed.id()).await.unwrap().unwrap(),
            closed_before
        );
        // Other sources are untouched.
        assert_eq!(
            store
                .get(elsewhere.id())
                .await
                .unwrap()
                .unwrap()
                .last_seen_at,
            at(1)
        );
        assert_eq!(events().await, events_before, "no history rows added");
    }

    #[tokio::test]
    async fn scans_are_recorded_and_feed_the_next_fetch() {
        let store = store().await;
        let source = SourceKey::new("ashby", "ramp").unwrap();
        assert_eq!(store.last_listing(&source).await.unwrap(), None);

        let a = posting("ramp", "1", "Engineer");
        scan(&store, "ramp", std::slice::from_ref(&a), at(0), true).await;
        let last = store.last_listing(&source).await.unwrap().unwrap();
        assert!(last.complete);
        assert_eq!(last.received, 1);
        assert_eq!(last.validator.as_deref(), Some("\"etag\""));
        assert_eq!(last.revision, CANONICAL_REVISION);
        assert_eq!(last.finished_at, at(0));

        // Not modified: open jobs are marked seen; the listing stays the
        // reference for the next conditional fetch.
        let run = store.begin_run(at(3)).await.unwrap();
        let result = store
            .apply_scan(&ScanWrite {
                run,
                source: &source,
                started_at: at(3),
                observed_at: at(3),
                counts: IngestCounts::default(),
                body: ScanBody::NotModified,
            })
            .await
            .unwrap();
        assert_eq!(result.touched, 1);
        assert_eq!(
            store.get(a.id()).await.unwrap().unwrap().last_seen_at,
            at(3)
        );
        assert_eq!(store.last_listing(&source).await.unwrap(), Some(last));

        // A failed scan changes nothing about jobs.
        store
            .apply_scan(&ScanWrite {
                run,
                source: &source,
                started_at: at(4),
                observed_at: at(4),
                counts: IngestCounts::default(),
                body: ScanBody::Failed { error: "HTTP 503" },
            })
            .await
            .unwrap();
        assert_eq!(
            store.get(a.id()).await.unwrap().unwrap().last_seen_at,
            at(3)
        );
        let statuses: Vec<String> =
            sqlx::query_scalar("SELECT status FROM source_scans ORDER BY id")
                .fetch_all(&store.pool)
                .await
                .unwrap();
        assert_eq!(statuses, ["listing", "not_modified", "failed"]);

        store
            .finish_run(
                run,
                &RunSummary {
                    finished_at: at(5),
                    sources: 1,
                    failed: 1,
                    ..RunSummary::default()
                },
            )
            .await
            .unwrap();
        let finished: Option<String> =
            sqlx::query_scalar("SELECT finished_at FROM discovery_runs WHERE id = ?")
                .bind(run.0)
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert!(finished.is_some());
    }

    #[tokio::test]
    async fn schema_rejects_two_rows_for_one_source_job() {
        let store = store().await;
        let p = posting("ramp", "1", "Engineer");
        scan(&store, "ramp", std::slice::from_ref(&p), at(0), true).await;
        let values = ContentValues::from_posting(&p, "a", "b").unwrap();
        let stamp = encode_timestamp(at(1));
        let result = values
            .bind(sqlx::query(&INSERT_SQL).bind("job_ffffffffffffffffffffffffffffffff"))
            .bind(&stamp)
            .bind(&stamp)
            .bind(&stamp)
            .bind("open")
            .bind("opp_ffffffffffffffffffffffffffffffff")
            .execute(&store.pool)
            .await;
        assert!(
            result.is_err(),
            "duplicate source identity must be rejected"
        );
    }

    #[tokio::test]
    async fn identity_evidence_and_opportunities_round_trip() {
        let store = store().await;
        let a = posting("ramp", "1", "Engineer");
        let b = posting("linear", "2", "Engineer");
        scan(&store, "ramp", std::slice::from_ref(&a), at(0), true).await;
        scan(&store, "linear", std::slice::from_ref(&b), at(1), true).await;

        let mut index = store.identity_index().await.unwrap();
        index.sort_by_key(|e| e.first_seen_at);
        assert_eq!(index.len(), 2);
        assert_eq!(index[0].job, a.id());
        assert_eq!(index[0].evidence, evidence_keys(&a));

        let opp = OpportunityId::founded_by(a.id());
        store.assign_opportunities(&[(b.id(), opp)]).await.unwrap();
        let members = store.opportunity_records(opp).await.unwrap();
        let ids: Vec<_> = members.iter().map(|r| r.id).collect();
        assert_eq!(ids, vec![a.id(), b.id()]);

        let distinct = JobQuery {
            distinct_opportunities: true,
            ..JobQuery::default()
        };
        assert_eq!(store.count(&distinct).await.unwrap(), 1);
        let found = store.search(&distinct).await.unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].id,
            a.id(),
            "the earliest record represents the group"
        );

        // With the representative filtered out, the other member stands in.
        let linear_only = JobQuery {
            sources: vec!["ashby:linear".parse().unwrap()],
            ..distinct
        };
        let found = store.search(&linear_only).await.unwrap();
        assert_eq!(found[0].id, b.id());
    }

    #[tokio::test]
    async fn search_filters_orders_and_limits() {
        let store = store().await;
        let mut old = posting("ramp", "1", "Backend Engineer");
        old.posted_at = Some(at(1));
        let mut new = posting("ramp", "2", "Frontend Engineer");
        new.posted_at = Some(at(2));
        let mut undated = posting("linear", "3", "Product Designer");
        undated.posted_at = None;
        undated.department = Some("Design".into());
        undated.team = None;
        scan(&store, "ramp", &[old, new], at(10), true).await;
        scan(&store, "linear", &[undated], at(10), true).await;

        let titles = |records: Vec<JobRecord>| -> Vec<String> {
            records.into_iter().map(|r| r.posting.title).collect()
        };

        let all = store.search(&JobQuery::default()).await.unwrap();
        assert_eq!(
            titles(all),
            vec!["Frontend Engineer", "Backend Engineer", "Product Designer"]
        );

        let engineers = JobQuery::default().with_text("ENGINEER");
        assert_eq!(store.count(&engineers).await.unwrap(), 2);

        let multi = JobQuery::default().with_text("engineer backend");
        assert_eq!(
            titles(store.search(&multi).await.unwrap()),
            vec!["Backend Engineer"]
        );

        let by_company = JobQuery::default().with_text("linear");
        assert_eq!(
            titles(store.search(&by_company).await.unwrap()),
            vec!["Product Designer"]
        );

        let by_source = JobQuery {
            sources: vec!["ashby:ramp".parse().unwrap()],
            limit: Some(1),
            ..Default::default()
        };
        assert_eq!(
            titles(store.search(&by_source).await.unwrap()),
            vec!["Frontend Engineer"]
        );
        assert_eq!(
            store.count(&by_source).await.unwrap(),
            2,
            "count ignores limit"
        );
    }

    #[tokio::test]
    async fn search_by_status_and_seen_since() {
        let store = store().await;
        let stale = posting("ramp", "1", "Engineer");
        let fresh = posting("ramp", "2", "Designer");
        scan(&store, "ramp", &[stale, fresh.clone()], at(0), true).await;
        scan(&store, "ramp", &[fresh], at(30), true).await;

        let seen = JobQuery {
            seen_since: Some(at(30)),
            ..Default::default()
        };
        let found = store.search(&seen).await.unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].posting.title, "Designer");

        let open = JobQuery {
            status: Some(JobStatus::Open),
            ..Default::default()
        };
        assert_eq!(store.count(&open).await.unwrap(), 1);
        let closed = JobQuery {
            status: Some(JobStatus::Closed),
            ..Default::default()
        };
        assert_eq!(
            store.search(&closed).await.unwrap()[0].posting.title,
            "Engineer"
        );
    }

    #[tokio::test]
    async fn terms_match_word_starts_not_arbitrary_substrings() {
        let store = store().await;
        let mut trust = posting("ramp", "1", "Program Manager");
        trust.department = Some("Trust & Safety".into());
        trust.team = None;
        let mut rust = posting("ramp", "2", "Backend Engineer (Rust)");
        rust.department = None;
        rust.team = None;
        let mut munich = posting("ramp", "3", "Account Executive");
        munich.location = Some("MÜNCHEN".into());
        munich.department = Some("Sales".into());
        munich.team = None;
        scan(&store, "ramp", &[trust, rust, munich], at(0), true).await;

        let titles = |q: &str| {
            let store = store.clone();
            let query = JobQuery::default().with_text(q);
            async move {
                store
                    .search(&query)
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|r| r.posting.title)
                    .collect::<Vec<_>>()
            }
        };
        assert_eq!(titles("rust").await, vec!["Backend Engineer (Rust)"]);
        assert_eq!(titles("eng").await, vec!["Backend Engineer (Rust)"]);
        assert_eq!(titles("trust safety").await, vec!["Program Manager"]);
        assert_eq!(titles("münchen").await, vec!["Account Executive"]);
        assert_eq!(titles("100%").await, Vec::<String>::new());
    }

    #[tokio::test]
    async fn large_unchanged_scans_take_the_cheap_path() {
        let store = store().await;
        let postings: Vec<JobPosting> = (0..1_200)
            .map(|i| posting("ramp", &i.to_string(), &format!("Engineer {i}")))
            .collect();
        scan(&store, "ramp", &postings, at(0), true).await;
        let result = scan(&store, "ramp", &postings, at(1), true).await;
        assert!(
            result
                .outcomes
                .iter()
                .all(|o| *o == UpsertOutcome::Unchanged)
        );
        let seen = JobQuery {
            seen_since: Some(at(1)),
            ..Default::default()
        };
        assert_eq!(store.count(&seen).await.unwrap(), 1_200);
        let events: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM job_events")
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert_eq!(events, 1_200, "unchanged observations add no history");
    }

    #[test]
    fn escapes_like_wildcards() {
        assert_eq!(escape_like(r"50%_off\"), r"50\%\_off\\");
    }

    #[test]
    fn timestamps_are_fixed_width_and_round_trip() {
        let t = Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap();
        let encoded = encode_timestamp(t);
        assert_eq!(encoded, "2026-01-02T03:04:05.000000Z");
        assert_eq!(decode_timestamp(&encoded).unwrap(), t);
    }
}
