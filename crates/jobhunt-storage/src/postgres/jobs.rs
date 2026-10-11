//! [`JobRepository`] for Postgres: the shared job corpus.
//!
//! The lifecycle rules are the domain's ([`plan_scan`]); this module applies
//! the plan exactly as the SQLite store does (the same inserts, rewrites,
//! history events and closings), in one transaction per scan.
//!
//! SQLite serializes every writer (`BEGIN IMMEDIATE`). Postgres runs writers
//! concurrently, so what must not interleave is serialized explicitly: a
//! scan of a source and a verification of one of its jobs both plan from
//! what is stored, so each takes a transaction-scoped advisory lock on the
//! source ([`lock_source`]) before reading it. Different sources proceed in
//! parallel.

use std::collections::{HashMap, HashSet};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use jobhunt_core::text::search_key;
use jobhunt_core::{CanonicalUrl, IngestCounts, Provenance, SourceKey};
use jobhunt_jobs::lifecycle::{Action, Closing, StoredJob, plan_scan};
use jobhunt_jobs::{
    CANONICAL_REVISION, Compensation, EmploymentType, IdentityEntry, JobEvent, JobEventKind, JobId,
    JobPosting, JobQuery, JobRecord, JobRepository, JobSnapshot, JobStatus, LastListing,
    OpportunityId, RunId, RunSummary, ScanBody, ScanResult, ScanWrite, SourceLocation,
    StorageError, WorkplaceType, evidence_keys,
};
use sqlx::postgres::PgRow;
use sqlx::types::Json;
use sqlx::{PgConnection, Postgres, QueryBuilder, Row};
use tracing::debug;

use super::{PgStore, corrupt, query_error};
use crate::store::ScanRecord;

/// Takes the transaction-scoped lock of a source: scans and single-job
/// observations of the same source run one at a time.
pub(crate) async fn lock_source(
    tx: &mut PgConnection,
    source: &SourceKey,
) -> Result<(), StorageError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("jobhunt.source:{source}"))
        .execute(&mut *tx)
        .await
        .map_err(query_error("locking a source"))?;
    Ok(())
}

fn count(value: usize) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
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
    locations: serde_json::Value,
    employment_type: Option<String>,
    workplace_type: Option<String>,
    is_remote: Option<bool>,
    compensation: Option<serde_json::Value>,
    work_authorization: Option<String>,
    description_text: Option<String>,
    description_html: Option<String>,
    posted_at: Option<DateTime<Utc>>,
    source_updated_at: Option<DateTime<Utc>>,
    search_text: String,
    fingerprint: String,
    content_fingerprint: String,
}

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
    "work_authorization",
    "description_text",
    "description_html",
    "posted_at",
    "source_updated_at",
    "search_text",
    "fingerprint",
    "content_fingerprint",
];

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
            locations: serde_json::to_value(&p.locations).map_err(encode_error)?,
            employment_type: p.employment_type.as_ref().map(|t| t.as_str().to_owned()),
            workplace_type: p.workplace_type.as_ref().map(|t| t.as_str().to_owned()),
            is_remote: p.is_remote,
            compensation: p
                .compensation
                .as_ref()
                .map(serde_json::to_value)
                .transpose()
                .map_err(encode_error)?,
            work_authorization: p.work_authorization.clone(),
            description_text: p.description_text.clone(),
            description_html: p.description_html.clone(),
            posted_at: p.posted_at,
            source_updated_at: p.source_updated_at,
            search_text: p.search_document(),
            fingerprint: fingerprint.to_owned(),
            content_fingerprint: content_fingerprint.to_owned(),
        })
    }

    /// Pushes the values in [`CONTENT_COLUMNS`] order, comma separated.
    fn push(&self, b: &mut QueryBuilder<'_, Postgres>) {
        let mut s = b.separated(", ");
        s.push_bind(self.source_kind.clone())
            .push_bind(self.source_instance.clone())
            .push_bind(self.source_job_id.clone())
            .push_bind(self.fetched_from.clone())
            .push_bind(self.url.clone())
            .push_bind(self.apply_url.clone())
            .push_bind(self.company.clone())
            .push_bind(self.title.clone())
            .push_bind(self.department.clone())
            .push_bind(self.team.clone())
            .push_bind(self.location.clone())
            .push_bind(Json(self.locations.clone()))
            .push_bind(self.employment_type.clone())
            .push_bind(self.workplace_type.clone())
            .push_bind(self.is_remote)
            .push_bind(self.compensation.clone().map(Json))
            .push_bind(self.work_authorization.clone())
            .push_bind(self.description_text.clone())
            .push_bind(self.description_html.clone())
            .push_bind(self.posted_at)
            .push_bind(self.source_updated_at)
            .push_bind(self.search_text.clone())
            .push_bind(self.fingerprint.clone())
            .push_bind(self.content_fingerprint.clone());
    }
}

#[allow(clippy::too_many_arguments)]
async fn insert_job(
    tx: &mut PgConnection,
    id: &str,
    values: &ContentValues,
    first_seen: DateTime<Utc>,
    last_seen: DateTime<Utc>,
    content_updated: DateTime<Utc>,
    status: JobStatus,
    closed_at: Option<DateTime<Utc>>,
    opportunity: &str,
    origin: &str,
) -> Result<(), StorageError> {
    let mut b = QueryBuilder::<Postgres>::new(format!(
        "INSERT INTO jobs (id, {}, first_seen_at, last_seen_at, content_updated_at, status, \
         closed_at, opportunity_id, origin) VALUES (",
        CONTENT_COLUMNS.join(", ")
    ));
    b.push_bind(id.to_owned()).push(", ");
    values.push(&mut b);
    b.push(", ")
        .push_bind(first_seen)
        .push(", ")
        .push_bind(last_seen)
        .push(", ")
        .push_bind(content_updated)
        .push(", ")
        .push_bind(status.as_str())
        .push(", ")
        .push_bind(closed_at)
        .push(", ")
        .push_bind(opportunity.to_owned())
        .push(", ")
        .push_bind(origin.to_owned())
        .push(")");
    b.build()
        .execute(&mut *tx)
        .await
        .map_err(query_error("inserting a job"))?;
    Ok(())
}

/// Rewrites content and marks the job seen and open. `None` keeps the
/// stored `content_updated_at` (non-material refreshes).
async fn rewrite(
    tx: &mut PgConnection,
    values: &ContentValues,
    id: &str,
    observed: DateTime<Utc>,
    content_updated_at: Option<DateTime<Utc>>,
) -> Result<(), StorageError> {
    let mut b = QueryBuilder::<Postgres>::new(format!(
        "UPDATE jobs SET ({}) = (",
        CONTENT_COLUMNS.join(", ")
    ));
    values.push(&mut b);
    b.push("), last_seen_at = ")
        .push_bind(observed)
        .push(", content_updated_at = COALESCE(")
        .push_bind(content_updated_at)
        .push(", content_updated_at), status = 'open', closed_at = NULL, origin = 'discovery' WHERE id = ")
        .push_bind(id.to_owned());
    b.build()
        .execute(&mut *tx)
        .await
        .map_err(query_error("updating a job"))?;
    Ok(())
}

async fn replace_evidence(
    tx: &mut PgConnection,
    id: &str,
    posting: &JobPosting,
) -> Result<(), StorageError> {
    sqlx::query("DELETE FROM job_evidence WHERE job_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(query_error("replacing identity evidence"))?;
    let keys: Vec<String> = evidence_keys(posting).into_iter().collect();
    if !keys.is_empty() {
        sqlx::query(
            "INSERT INTO job_evidence (job_id, key) SELECT $1, k FROM unnest($2::text[]) AS k \
             ON CONFLICT DO NOTHING",
        )
        .bind(id)
        .bind(&keys)
        .execute(&mut *tx)
        .await
        .map_err(query_error("storing identity evidence"))?;
    }
    Ok(())
}

/// Who saw postings, when, and in which discovery run (none for a
/// verification).
pub(crate) struct Observation<'a> {
    pub source: &'a SourceKey,
    pub observed_at: DateTime<Utc>,
    pub run: Option<RunId>,
}

async fn insert_event(
    tx: &mut PgConnection,
    scan: &Observation<'_>,
    job_id: &str,
    kind: JobEventKind,
    changed_fields: &[&str],
    previous: Option<&JobSnapshot>,
) -> Result<(), StorageError> {
    let previous = previous
        .map(serde_json::to_value)
        .transpose()
        .map_err(|e| StorageError::Query {
            operation: "encoding job history",
            source: Box::new(e),
        })?;
    sqlx::query(
        "INSERT INTO job_events (job_id, run_id, kind, at, changed_fields, previous) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(job_id)
    .bind(scan.run.map(|r| r.0))
    .bind(kind.as_str())
    .bind(scan.observed_at)
    .bind(Json(changed_fields))
    .bind(previous.map(Json))
    .execute(&mut *tx)
    .await
    .map_err(query_error("recording job history"))?;
    Ok(())
}

async fn load_stored(
    tx: &mut PgConnection,
    source: &SourceKey,
) -> Result<HashMap<JobId, StoredJob>, StorageError> {
    let rows = sqlx::query(
        "SELECT id, status, fingerprint, content_fingerprint FROM jobs \
         WHERE source_kind = $1 AND source_instance = $2",
    )
    .bind(source.kind())
    .bind(source.instance())
    .fetch_all(&mut *tx)
    .await
    .map_err(query_error("loading stored jobs of a source"))?;
    rows.iter()
        .map(|row| {
            let raw_id: String = row.try_get("id").map_err(|e| corrupt("<jobs row>", e))?;
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

async fn load_snapshot(tx: &mut PgConnection, id: JobId) -> Result<JobSnapshot, StorageError> {
    let row = sqlx::query("SELECT * FROM jobs WHERE id = $1")
        .bind(id.to_string())
        .fetch_one(&mut *tx)
        .await
        .map_err(query_error("loading a job's previous version"))?;
    Ok(decode_record(&row)?.posting.snapshot())
}

/// Applies a listing inside a transaction that holds the source's lock.
pub(crate) async fn apply_listing(
    tx: &mut PgConnection,
    scan: &Observation<'_>,
    postings: &[JobPosting],
    close_missing: bool,
    retain: &[JobId],
    result: &mut ScanResult,
) -> Result<(), StorageError> {
    let observed = scan.observed_at;
    let stored = load_stored(&mut *tx, scan.source).await?;
    let retain: HashSet<JobId> = retain.iter().copied().collect();
    let closing = if close_missing {
        Closing::CloseMissing { retain: &retain }
    } else {
        Closing::Skip
    };
    let plan = plan_scan(&stored, postings, closing);

    let mut touched: Vec<String> = Vec::new();
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
                insert_job(
                    &mut *tx,
                    &id,
                    &values,
                    observed,
                    observed,
                    observed,
                    JobStatus::Open,
                    None,
                    &OpportunityId::founded_by(planned.id).to_string(),
                    "discovery",
                )
                .await?;
                insert_event(&mut *tx, scan, &id, JobEventKind::New, &[], None).await?;
            }
            Action::Refresh => {
                rewrite(&mut *tx, &values, &id, observed, None).await?;
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
                    observed,
                    content_changed.then_some(observed),
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

    if !touched.is_empty() {
        sqlx::query("UPDATE jobs SET last_seen_at = $1 WHERE id = ANY($2)")
            .bind(observed)
            .bind(&touched)
            .execute(&mut *tx)
            .await
            .map_err(query_error("marking jobs as seen"))?;
    }

    for id in &plan.close {
        let id = id.to_string();
        sqlx::query("UPDATE jobs SET status = 'closed', closed_at = $1 WHERE id = $2")
            .bind(observed)
            .bind(&id)
            .execute(&mut *tx)
            .await
            .map_err(query_error("closing a job"))?;
        insert_event(&mut *tx, scan, &id, JobEventKind::Closed, &[], None).await?;
    }
    result.closed = plan.close;
    Ok(())
}

/// Inserts a job exactly as it was stored elsewhere (a person's synced
/// feedback refers to it), unless a job with its id already exists.
/// Returns whether it was inserted. It is marked `origin = 'sync'` until
/// cloud discovery or verification sees it at its source.
pub(crate) async fn import_job(
    tx: &mut PgConnection,
    record: &JobRecord,
) -> Result<bool, StorageError> {
    let id = record.id.to_string();
    if record.posting.id() != record.id {
        return Err(corrupt(
            &id,
            "the id does not match the posting's source and source id",
        ));
    }
    lock_source(&mut *tx, &record.posting.provenance.source).await?;
    let exists: Option<i32> = sqlx::query_scalar("SELECT 1 FROM jobs WHERE id = $1")
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
    insert_job(
        &mut *tx,
        &id,
        &values,
        record.first_seen_at,
        record.last_seen_at,
        record.content_updated_at,
        record.status,
        record.closed_at,
        &record.opportunity_id.to_string(),
        "sync",
    )
    .await?;
    replace_evidence(&mut *tx, &id, &record.posting).await?;
    Ok(true)
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
    tx: &mut PgConnection,
    scan: &ScanWrite<'_>,
    row: &ScanRow<'_>,
    c: &IngestCounts,
) -> Result<(), StorageError> {
    sqlx::query(
        "INSERT INTO source_scans (run_id, source_kind, source_instance, status, complete, \
         closing_applied, closing_withheld, started_at, finished_at, received, normalized, \
         rejected, skipped, duplicates, new, updated, unchanged, reopened, closed, validator, \
         revision, error) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, \
         $15, $16, $17, $18, $19, $20, $21, $22)",
    )
    .bind(scan.run.0)
    .bind(scan.source.kind())
    .bind(scan.source.instance())
    .bind(row.status)
    .bind(row.complete)
    .bind(row.closing_applied)
    .bind(row.closing_withheld)
    .bind(scan.started_at)
    .bind(scan.observed_at)
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

/// A `LIKE` pattern matching values that start with `prefix` literally.
pub(crate) fn like_prefix(prefix: &str) -> String {
    let mut pattern = escape_like(prefix);
    pattern.push('%');
    pattern
}

fn push_filters(builder: &mut QueryBuilder<'_, Postgres>, query: &JobQuery) {
    builder.push(" WHERE TRUE");
    for term in &query.terms {
        // search_text is " word word ... ", so " term%" matches the start
        // of a word, exactly as in the local store.
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
        builder.push(" AND last_seen_at >= ").push_bind(since);
    }
}

pub(crate) fn decode_record(row: &PgRow) -> Result<JobRecord, StorageError> {
    let raw_id: String = row
        .try_get("id")
        .map_err(|e| corrupt("<unknown>", format!("id: {e}")))?;
    let bad = |column: &str, e: &dyn std::fmt::Display| corrupt(&raw_id, format!("{column}: {e}"));
    let text = |column: &'static str| -> Result<String, StorageError> {
        row.try_get(column).map_err(|e| bad(column, &e))
    };
    let opt = |column: &'static str| -> Result<Option<String>, StorageError> {
        row.try_get(column).map_err(|e| bad(column, &e))
    };
    let time = |column: &'static str| -> Result<DateTime<Utc>, StorageError> {
        row.try_get(column).map_err(|e| bad(column, &e))
    };
    let opt_time = |column: &'static str| -> Result<Option<DateTime<Utc>>, StorageError> {
        row.try_get(column).map_err(|e| bad(column, &e))
    };
    let url =
        |column: &'static str, value: &str| CanonicalUrl::parse(value).map_err(|e| bad(column, &e));

    let id: JobId = raw_id.parse().map_err(|e| bad("id", &e))?;
    let source = SourceKey::new(&text("source_kind")?, &text("source_instance")?)
        .map_err(|e| bad("source", &e))?;
    let Json(locations): Json<Vec<SourceLocation>> =
        row.try_get("locations").map_err(|e| bad("locations", &e))?;
    let compensation: Option<Json<Compensation>> = row
        .try_get("compensation")
        .map_err(|e| bad("compensation", &e))?;
    let status = text("status")?;

    let posting = JobPosting {
        provenance: Provenance {
            source,
            source_record_id: opt("source_job_id")?,
            fetched_from: opt("fetched_from")?
                .map(|v| url("fetched_from", &v))
                .transpose()?,
        },
        url: url("url", &text("url")?)?,
        apply_url: opt("apply_url")?
            .map(|v| url("apply_url", &v))
            .transpose()?,
        company: text("company")?,
        title: text("title")?,
        department: opt("department")?,
        team: opt("team")?,
        location: opt("location")?,
        locations,
        employment_type: opt("employment_type")?
            .as_deref()
            .map(EmploymentType::from_canonical),
        workplace_type: opt("workplace_type")?
            .as_deref()
            .map(WorkplaceType::from_canonical),
        is_remote: row.try_get("is_remote").map_err(|e| bad("is_remote", &e))?,
        compensation: compensation.map(|Json(c)| c),
        work_authorization: opt("work_authorization")?,
        description_text: opt("description_text")?,
        description_html: opt("description_html")?,
        posted_at: opt_time("posted_at")?,
        source_updated_at: opt_time("source_updated_at")?,
    };
    Ok(JobRecord {
        id,
        posting,
        first_seen_at: time("first_seen_at")?,
        last_seen_at: time("last_seen_at")?,
        content_updated_at: time("content_updated_at")?,
        status: JobStatus::from_canonical(&status)
            .ok_or_else(|| bad("status", &format!("unknown status {status:?}")))?,
        closed_at: opt_time("closed_at")?,
        opportunity_id: text("opportunity_id")?
            .parse()
            .map_err(|e| bad("opportunity_id", &e))?,
    })
}

impl PgStore {
    /// Opportunity ids that start with `prefix`, at most `limit`.
    pub async fn opportunities_with_prefix(
        &self,
        prefix: &str,
        limit: usize,
    ) -> Result<Vec<OpportunityId>, StorageError> {
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT opportunity_id FROM jobs WHERE opportunity_id LIKE $1 ESCAPE '\\' \
             ORDER BY opportunity_id LIMIT $2",
        )
        .bind(like_prefix(prefix))
        .bind(i64::try_from(limit).unwrap_or(i64::MAX))
        .fetch_all(self.pool())
        .await
        .map_err(query_error("resolving an opportunity id"))?;
        rows.iter()
            .map(|id| {
                id.parse()
                    .map_err(|e| corrupt(id, format!("opportunity_id: {e}")))
            })
            .collect()
    }

    /// Job ids that start with `prefix`, at most `limit`.
    pub async fn jobs_with_prefix(
        &self,
        prefix: &str,
        limit: usize,
    ) -> Result<Vec<JobId>, StorageError> {
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM jobs WHERE id LIKE $1 ESCAPE '\\' ORDER BY id LIMIT $2",
        )
        .bind(like_prefix(prefix))
        .bind(i64::try_from(limit).unwrap_or(i64::MAX))
        .fetch_all(self.pool())
        .await
        .map_err(query_error("resolving a job id"))?;
        rows.iter()
            .map(|id| id.parse().map_err(|e| corrupt(id, format!("id: {e}"))))
            .collect()
    }

    /// When each source was last read successfully.
    pub async fn last_checked(&self) -> Result<HashMap<SourceKey, DateTime<Utc>>, StorageError> {
        let rows = sqlx::query(
            "SELECT source_kind, source_instance, MAX(finished_at) AS at FROM source_scans \
             WHERE status IN ('listing', 'not_modified') GROUP BY source_kind, source_instance",
        )
        .fetch_all(self.pool())
        .await
        .map_err(query_error("loading when sources were last read"))?;
        let mut out = HashMap::with_capacity(rows.len());
        for row in rows {
            let kind: String = row
                .try_get("source_kind")
                .map_err(|e| corrupt("<source_scans>", e))?;
            let instance: String = row
                .try_get("source_instance")
                .map_err(|e| corrupt("<source_scans>", e))?;
            let at: DateTime<Utc> = row
                .try_get("at")
                .map_err(|e| corrupt("<source_scans>", e))?;
            if let Ok(key) = SourceKey::new(&kind, &instance) {
                out.insert(key, at);
            }
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
             ) s WHERE n <= $1 ORDER BY source_kind, source_instance, n",
        )
        .bind(i64::try_from(per_source).unwrap_or(i64::MAX))
        .fetch_all(self.pool())
        .await
        .map_err(query_error("loading recent source scans"))?;
        let mut out: HashMap<SourceKey, Vec<ScanRecord>> = HashMap::new();
        for row in rows {
            let bad = |e| corrupt("<source_scans>", e);
            let kind: String = row.try_get("source_kind").map_err(bad)?;
            let instance: String = row.try_get("source_instance").map_err(bad)?;
            let Ok(key) = SourceKey::new(&kind, &instance) else {
                continue;
            };
            let received: i32 = row.try_get("received").map_err(bad)?;
            out.entry(key).or_default().push(ScanRecord {
                finished_at: row.try_get("finished_at").map_err(bad)?,
                status: row.try_get("status").map_err(bad)?,
                received: u64::try_from(received).unwrap_or(0),
                error: row.try_get("error").map_err(bad)?,
            });
        }
        Ok(out)
    }

    /// Records of many opportunities at once, grouped, earliest seen first.
    pub(crate) async fn records_of(
        &self,
        opportunities: &[String],
    ) -> Result<HashMap<OpportunityId, Vec<JobRecord>>, StorageError> {
        let rows = sqlx::query(
            "SELECT * FROM jobs WHERE opportunity_id = ANY($1) \
             ORDER BY opportunity_id, first_seen_at, id",
        )
        .bind(opportunities)
        .fetch_all(self.pool())
        .await
        .map_err(query_error("loading opportunities"))?;
        let mut out: HashMap<OpportunityId, Vec<JobRecord>> = HashMap::new();
        for row in &rows {
            let record = decode_record(row)?;
            out.entry(record.opportunity_id).or_default().push(record);
        }
        Ok(out)
    }
}

#[async_trait]
impl JobRepository for PgStore {
    async fn begin_run(&self, started_at: DateTime<Utc>) -> Result<RunId, StorageError> {
        let id: i64 =
            sqlx::query_scalar("INSERT INTO discovery_runs (started_at) VALUES ($1) RETURNING id")
                .bind(started_at)
                .fetch_one(self.pool())
                .await
                .map_err(query_error("recording a discovery run"))?;
        Ok(RunId(id))
    }

    async fn finish_run(&self, run: RunId, summary: &RunSummary) -> Result<(), StorageError> {
        let c = &summary.counts;
        sqlx::query(
            "UPDATE discovery_runs SET finished_at = $1, sources = $2, failed = $3, received = $4, \
             normalized = $5, rejected = $6, new = $7, updated = $8, unchanged = $9, \
             reopened = $10, closed = $11, multi_source_opportunities = $12 WHERE id = $13",
        )
        .bind(summary.finished_at)
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
        .execute(self.pool())
        .await
        .map_err(query_error("finishing a discovery run"))?;
        Ok(())
    }

    async fn last_listing(&self, source: &SourceKey) -> Result<Option<LastListing>, StorageError> {
        let row = sqlx::query(
            "SELECT finished_at, complete, received, validator, revision FROM source_scans \
             WHERE source_kind = $1 AND source_instance = $2 AND status = 'listing' \
             ORDER BY id DESC LIMIT 1",
        )
        .bind(source.kind())
        .bind(source.instance())
        .fetch_optional(self.pool())
        .await
        .map_err(query_error("loading the last scan of a source"))?;
        let Some(row) = row else {
            return Ok(None);
        };
        let scan_error = |e: sqlx::Error| corrupt(&format!("scan of {source}"), e);
        let received: i32 = row.try_get("received").map_err(scan_error)?;
        Ok(Some(LastListing {
            finished_at: row.try_get("finished_at").map_err(scan_error)?,
            complete: row.try_get("complete").map_err(scan_error)?,
            received: usize::try_from(received).unwrap_or(0),
            validator: row.try_get("validator").map_err(scan_error)?,
            revision: row.try_get("revision").map_err(scan_error)?,
        }))
    }

    async fn apply_scan(&self, scan: &ScanWrite<'_>) -> Result<ScanResult, StorageError> {
        let mut tx = self
            .pool()
            .begin()
            .await
            .map_err(query_error("starting a transaction"))?;
        lock_source(&mut tx, scan.source).await?;
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
                    "UPDATE jobs SET last_seen_at = $1 WHERE source_kind = $2 \
                     AND source_instance = $3 AND status = 'open'",
                )
                .bind(scan.observed_at)
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
        .fetch_all(self.pool())
        .await
        .map_err(query_error("loading job identities"))?;
        let evidence_rows = sqlx::query("SELECT job_id, key FROM job_evidence")
            .fetch_all(self.pool())
            .await
            .map_err(query_error("loading identity evidence"))?;
        let mut evidence: HashMap<String, Vec<String>> = HashMap::new();
        for row in &evidence_rows {
            let job: String = row
                .try_get("job_id")
                .map_err(|e| corrupt("<job_evidence>", e))?;
            let key: String = row
                .try_get("key")
                .map_err(|e| corrupt("<job_evidence>", e))?;
            evidence.entry(job).or_default().push(key);
        }
        rows.iter()
            .map(|row| {
                let raw_id: String = row.try_get("id").map_err(|e| corrupt("<jobs>", e))?;
                let text = |column: &'static str| -> Result<String, StorageError> {
                    row.try_get(column)
                        .map_err(|e| corrupt(&raw_id, format!("{column}: {e}")))
                };
                Ok(IdentityEntry {
                    job: raw_id
                        .parse()
                        .map_err(|e| corrupt(&raw_id, format!("id: {e}")))?,
                    source: SourceKey::new(&text("source_kind")?, &text("source_instance")?)
                        .map_err(|e| corrupt(&raw_id, format!("source: {e}")))?,
                    company: text("company")?,
                    title: text("title")?,
                    first_seen_at: row
                        .try_get("first_seen_at")
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
        let jobs: Vec<String> = assignments.iter().map(|(j, _)| j.to_string()).collect();
        let opportunities: Vec<String> = assignments.iter().map(|(_, o)| o.to_string()).collect();
        let mut tx = self
            .pool()
            .begin()
            .await
            .map_err(query_error("starting a transaction"))?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('jobhunt.opportunities', 0))")
            .execute(&mut *tx)
            .await
            .map_err(query_error("locking opportunities"))?;
        sqlx::query(
            "UPDATE jobs SET opportunity_id = a.opportunity \
             FROM unnest($1::text[], $2::text[]) AS a (job, opportunity) WHERE jobs.id = a.job",
        )
        .bind(&jobs)
        .bind(&opportunities)
        .execute(&mut *tx)
        .await
        .map_err(query_error("assigning opportunities"))?;
        tx.commit()
            .await
            .map_err(query_error("committing opportunities"))?;
        Ok(())
    }

    async fn get(&self, id: JobId) -> Result<Option<JobRecord>, StorageError> {
        let row = sqlx::query("SELECT * FROM jobs WHERE id = $1")
            .bind(id.to_string())
            .fetch_optional(self.pool())
            .await
            .map_err(query_error("loading a job"))?;
        row.as_ref().map(decode_record).transpose()
    }

    async fn opportunity_records(&self, id: OpportunityId) -> Result<Vec<JobRecord>, StorageError> {
        let rows =
            sqlx::query("SELECT * FROM jobs WHERE opportunity_id = $1 ORDER BY first_seen_at, id")
                .bind(id.to_string())
                .fetch_all(self.pool())
                .await
                .map_err(query_error("loading an opportunity"))?;
        rows.iter().map(decode_record).collect()
    }

    async fn history(&self, id: JobId) -> Result<Vec<JobEvent>, StorageError> {
        let raw_id = id.to_string();
        let rows = sqlx::query(
            "SELECT job_id, kind, at, run_id, changed_fields, previous FROM job_events \
             WHERE job_id = $1 ORDER BY id",
        )
        .bind(&raw_id)
        .fetch_all(self.pool())
        .await
        .map_err(query_error("loading job history"))?;
        rows.iter()
            .map(|row| decode_event(row).map(|(_, e)| e))
            .collect()
    }

    async fn histories(
        &self,
        ids: &[JobId],
    ) -> Result<HashMap<JobId, Vec<JobEvent>>, StorageError> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let raw: Vec<String> = ids.iter().map(ToString::to_string).collect();
        let rows = sqlx::query(
            "SELECT job_id, kind, at, run_id, changed_fields, previous FROM job_events \
             WHERE job_id = ANY($1) ORDER BY id",
        )
        .bind(&raw)
        .fetch_all(self.pool())
        .await
        .map_err(query_error("loading job history"))?;
        let mut out: HashMap<JobId, Vec<JobEvent>> = HashMap::new();
        for row in &rows {
            let (job, event) = decode_event(row)?;
            out.entry(job).or_default().push(event);
        }
        Ok(out)
    }

    async fn get_many(&self, ids: &[JobId]) -> Result<HashMap<JobId, JobRecord>, StorageError> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let raw: Vec<String> = ids.iter().map(ToString::to_string).collect();
        let rows = sqlx::query("SELECT * FROM jobs WHERE id = ANY($1)")
            .bind(&raw)
            .fetch_all(self.pool())
            .await
            .map_err(query_error("loading jobs"))?;
        rows.iter()
            .map(|row| decode_record(row).map(|r| (r.id, r)))
            .collect()
    }

    async fn opportunity_records_many(
        &self,
        ids: &[OpportunityId],
    ) -> Result<HashMap<OpportunityId, Vec<JobRecord>>, StorageError> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let raw: Vec<String> = ids.iter().map(ToString::to_string).collect();
        self.records_of(&raw).await
    }

    async fn search(&self, query: &JobQuery) -> Result<Vec<JobRecord>, StorageError> {
        let mut builder = if query.distinct_opportunities {
            // One row per opportunity: an open record if there is one, then
            // the earliest seen.
            let mut b = QueryBuilder::<Postgres>::new(
                "SELECT * FROM (SELECT jobs.*, ROW_NUMBER() OVER (PARTITION BY opportunity_id \
                 ORDER BY status = 'open' DESC, first_seen_at, id) AS opportunity_rank \
                 FROM jobs",
            );
            push_filters(&mut b, query);
            b.push(") AS ranked WHERE opportunity_rank = 1");
            b
        } else {
            let mut b = QueryBuilder::<Postgres>::new("SELECT * FROM jobs");
            push_filters(&mut b, query);
            b
        };
        builder.push(" ORDER BY posted_at DESC NULLS LAST, first_seen_at DESC, id");
        if let Some(limit) = query.limit {
            builder
                .push(" LIMIT ")
                .push_bind(i64::try_from(limit).unwrap_or(i64::MAX));
        }
        let rows = builder
            .build()
            .fetch_all(self.pool())
            .await
            .map_err(query_error("searching jobs"))?;
        rows.iter().map(decode_record).collect()
    }

    async fn count(&self, query: &JobQuery) -> Result<u64, StorageError> {
        let mut builder = QueryBuilder::<Postgres>::new(if query.distinct_opportunities {
            "SELECT COUNT(DISTINCT opportunity_id) FROM jobs"
        } else {
            "SELECT COUNT(*) FROM jobs"
        });
        push_filters(&mut builder, query);
        let count: i64 = builder
            .build_query_scalar()
            .fetch_one(self.pool())
            .await
            .map_err(query_error("counting jobs"))?;
        Ok(u64::try_from(count).unwrap_or(0))
    }
}

/// One `job_events` row (with its `job_id`).
fn decode_event(row: &PgRow) -> Result<(JobId, JobEvent), StorageError> {
    let raw_id: String = row
        .try_get("job_id")
        .map_err(|e| corrupt("<job event>", e))?;
    let bad = |e: String| corrupt(&raw_id, format!("history: {e}"));
    let job: JobId = raw_id
        .parse()
        .map_err(|e: jobhunt_core::ParseIdError| bad(e.to_string()))?;
    let kind: String = row.try_get("kind").map_err(|e| bad(e.to_string()))?;
    let run: Option<i64> = row.try_get("run_id").map_err(|e| bad(e.to_string()))?;
    let Json(changed): Json<Vec<String>> = row
        .try_get("changed_fields")
        .map_err(|e| bad(e.to_string()))?;
    let previous: Option<Json<JobSnapshot>> =
        row.try_get("previous").map_err(|e| bad(e.to_string()))?;
    Ok((
        job,
        JobEvent {
            kind: JobEventKind::from_canonical(&kind)
                .ok_or_else(|| bad(format!("unknown kind {kind:?}")))?,
            at: row.try_get("at").map_err(|e| bad(e.to_string()))?,
            run: run.map(RunId),
            changed_fields: changed,
            previous: previous.map(|Json(p)| p),
        },
    ))
}
