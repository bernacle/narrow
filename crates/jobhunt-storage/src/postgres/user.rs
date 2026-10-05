//! [`PgUserStore`]: one person's view of the cloud store.

use std::collections::HashMap;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use jobhunt_core::{SourceKey, UpsertOutcome};
use jobhunt_eligibility::{CacheKey, EligibilityDecision, EligibilityRepository};
use jobhunt_jobs::verification::{LatestVerifications, VerificationRecord, VerificationRepository};
use jobhunt_jobs::{
    IdentityEntry, JobEvent, JobId, JobPosting, JobQuery, JobRecord, JobRepository, LastListing,
    OpportunityId, RunId, RunSummary, ScanResult, ScanWrite, StorageError,
};
use jobhunt_ranking::{
    FeedbackAction, FeedbackEvent, FeedbackRepository, FitReview, RankKey, Ranking,
    RankingRepository,
};
use sqlx::{PgConnection, Postgres, Row};

use super::accounts::UserId;
use super::cache::Cache;
use super::{PgStore, corrupt, query_error};
use crate::sqlite::StoreStats;
use crate::state::{StateImport, StateImported};
use crate::store::{FeedMark, ScanRecord, Shown, Store, WriteGuard};

/// One person's view of the cloud store: the shared corpus, and their own
/// private data only (see [`super`]).
pub struct PgUserStore {
    pub(crate) shared: PgStore,
    pub(crate) user: UserId,
    pub(crate) cache: Cache,
}

impl std::fmt::Debug for PgUserStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgUserStore")
            .field("user", &self.user)
            .finish_non_exhaustive()
    }
}

/// Takes the next value of the person's change counter, locking their
/// account row until the transaction ends. Every private write does this
/// first, so a person's writes are serialized and sync cursors see them in
/// order.
pub(crate) async fn next_seq(tx: &mut PgConnection, user: &UserId) -> Result<i64, StorageError> {
    sqlx::query_scalar(
        "UPDATE users SET change_seq = change_seq + 1 WHERE id = $1 RETURNING change_seq",
    )
    .bind(user.as_str())
    .fetch_optional(&mut *tx)
    .await
    .map_err(query_error("recording a change"))?
    .ok_or_else(|| corrupt(user.as_str(), "the account does not exist"))
}

impl PgUserStore {
    pub(crate) fn new(shared: PgStore, user: UserId) -> Self {
        Self {
            shared,
            user,
            cache: Cache::default(),
        }
    }

    pub fn user(&self) -> &UserId {
        &self.user
    }

    pub fn shared(&self) -> &PgStore {
        &self.shared
    }

    pub(crate) fn seal(&self, context: &str, plaintext: &[u8]) -> Result<Vec<u8>, StorageError> {
        self.shared
            .keys()
            .seal(context, plaintext)
            .map_err(|e| StorageError::Query {
                operation: "encrypting private data",
                source: Box::new(e),
            })
    }

    pub(crate) fn open(&self, context: &str, sealed: &[u8]) -> Result<Vec<u8>, StorageError> {
        self.shared
            .keys()
            .open(context, sealed)
            .map_err(|e| corrupt(context, e))
    }

    fn open_text(&self, context: &str, sealed: &[u8]) -> Result<String, StorageError> {
        String::from_utf8(self.open(context, sealed)?).map_err(|e| corrupt(context, e))
    }

    fn reason_context(&self, feedback: &str) -> String {
        format!("feedback|{}|{feedback}", self.user)
    }

    pub(crate) fn decode_feedback(
        &self,
        row: &sqlx::postgres::PgRow,
    ) -> Result<FeedbackEvent, StorageError> {
        let id: String = row.try_get("id").map_err(|e| corrupt("<feedback>", e))?;
        let bad = |e: &dyn std::fmt::Display| corrupt(&id, e);
        let text = |column: &str| -> Result<String, StorageError> {
            row.try_get(column).map_err(|e| bad(&e))
        };
        let action = text("action")?;
        let reason: Option<Vec<u8>> = row.try_get("reason").map_err(|e| bad(&e))?;
        Ok(FeedbackEvent {
            id: id.parse().map_err(|e| bad(&e))?,
            profile_id: text("profile_id")?,
            opportunity: text("opportunity_id")?.parse().map_err(|e| bad(&e))?,
            job: text("job_id")?.parse().map_err(|e| bad(&e))?,
            action: FeedbackAction::from_canonical(&action)
                .ok_or_else(|| bad(&format!("unknown action {action:?}")))?,
            reason: reason
                .map(|sealed| self.open_text(&self.reason_context(&id), &sealed))
                .transpose()?,
            title: text("title")?,
            company: text("company")?,
            at: row.try_get("recorded_at").map_err(|e| bad(&e))?,
        })
    }

    /// Inserts a feedback event inside a transaction. With `if_absent`, an
    /// event whose id is stored is left alone and `false` returned.
    pub(crate) async fn insert_feedback(
        &self,
        tx: &mut PgConnection,
        event: &FeedbackEvent,
        seq: i64,
        if_absent: bool,
    ) -> Result<bool, StorageError> {
        let id = event.id.to_string();
        let reason = event
            .reason
            .as_deref()
            .map(|r| self.seal(&self.reason_context(&id), r.as_bytes()))
            .transpose()?;
        let result = sqlx::query(&format!(
            "INSERT INTO feedback (user_id, id, profile_id, opportunity_id, job_id, action, \
             reason, title, company, recorded_at, seq) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11){}",
            if if_absent {
                " ON CONFLICT (user_id, id) DO NOTHING"
            } else {
                ""
            }
        ))
        .bind(self.user.as_str())
        .bind(&id)
        .bind(&event.profile_id)
        .bind(event.opportunity.to_string())
        .bind(event.job.to_string())
        .bind(event.action.as_str())
        .bind(reason)
        .bind(&event.title)
        .bind(&event.company)
        .bind(event.at)
        .bind(seq)
        .execute(&mut *tx)
        .await
        .map_err(query_error("saving feedback"))?;
        Ok(result.rows_affected() > 0)
    }

    /// Takes the person's write lock (see [`Store::write_lock`]).
    async fn lock(&self) -> Result<sqlx::Transaction<'static, Postgres>, StorageError> {
        let mut tx = self
            .shared
            .pool()
            .begin()
            .await
            .map_err(query_error("starting a transaction"))?;
        sqlx::query("SET LOCAL lock_timeout = '20s'")
            .execute(&mut *tx)
            .await
            .map_err(query_error("taking the write lock"))?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(format!("jobhunt.user:{}", self.user))
            .execute(&mut *tx)
            .await
            .map_err(query_error("taking the write lock"))?;
        Ok(tx)
    }
}

/// Holds a person's advisory lock; dropping it ends the transaction, which
/// releases the lock.
struct AdvisoryGuard(Option<sqlx::Transaction<'static, Postgres>>);

impl Drop for AdvisoryGuard {
    fn drop(&mut self) {
        if let Some(tx) = self.0.take()
            && let Ok(handle) = tokio::runtime::Handle::try_current()
        {
            // Roll back promptly instead of waiting for the connection to
            // be reused.
            handle.spawn(async move {
                let _ = tx.rollback().await;
            });
        }
    }
}

#[async_trait]
impl JobRepository for PgUserStore {
    async fn begin_run(&self, started_at: DateTime<Utc>) -> Result<RunId, StorageError> {
        self.shared.begin_run(started_at).await
    }

    async fn finish_run(&self, run: RunId, summary: &RunSummary) -> Result<(), StorageError> {
        self.shared.finish_run(run, summary).await
    }

    async fn last_listing(&self, source: &SourceKey) -> Result<Option<LastListing>, StorageError> {
        self.shared.last_listing(source).await
    }

    async fn apply_scan(&self, scan: &ScanWrite<'_>) -> Result<ScanResult, StorageError> {
        self.cache.clear();
        self.shared.apply_scan(scan).await
    }

    async fn identity_index(&self) -> Result<Vec<IdentityEntry>, StorageError> {
        self.shared.identity_index().await
    }

    async fn assign_opportunities(
        &self,
        assignments: &[(JobId, OpportunityId)],
    ) -> Result<(), StorageError> {
        self.cache.clear();
        self.shared.assign_opportunities(assignments).await
    }

    async fn get(&self, id: JobId) -> Result<Option<JobRecord>, StorageError> {
        self.shared.get(id).await
    }

    async fn opportunity_records(&self, id: OpportunityId) -> Result<Vec<JobRecord>, StorageError> {
        if let Some(records) = self.cache.lock().records.get(&id) {
            return Ok(records.clone());
        }
        self.shared.opportunity_records(id).await
    }

    async fn history(&self, id: JobId) -> Result<Vec<JobEvent>, StorageError> {
        self.shared.history(id).await
    }

    async fn histories(
        &self,
        ids: &[JobId],
    ) -> Result<HashMap<JobId, Vec<JobEvent>>, StorageError> {
        self.shared.histories(ids).await
    }

    async fn get_many(&self, ids: &[JobId]) -> Result<HashMap<JobId, JobRecord>, StorageError> {
        self.shared.get_many(ids).await
    }

    /// Also kept for the rest of the request: the single lookups that
    /// follow (classifying, checking what is shown) are answered from it.
    async fn opportunity_records_many(
        &self,
        ids: &[OpportunityId],
    ) -> Result<HashMap<OpportunityId, Vec<JobRecord>>, StorageError> {
        let records = self.shared.opportunity_records_many(ids).await?;
        self.cache
            .lock()
            .records
            .extend(records.iter().map(|(id, r)| (*id, r.clone())));
        Ok(records)
    }

    async fn search(&self, query: &JobQuery) -> Result<Vec<JobRecord>, StorageError> {
        self.shared.search(query).await
    }

    async fn count(&self, query: &JobQuery) -> Result<u64, StorageError> {
        self.shared.count(query).await
    }
}

#[async_trait]
impl VerificationRepository for PgUserStore {
    async fn save_verification(&self, record: &VerificationRecord) -> Result<(), StorageError> {
        self.cache.clear();
        self.shared.save_verification(record).await
    }

    async fn verification_history(
        &self,
        job: JobId,
    ) -> Result<Vec<VerificationRecord>, StorageError> {
        self.shared.verification_history(job).await
    }

    async fn latest_verification(
        &self,
        job: JobId,
    ) -> Result<Option<VerificationRecord>, StorageError> {
        if let Some(v) = self.cache.lock().latest.get(&job) {
            return Ok(v.clone());
        }
        self.shared.latest_verification(job).await
    }

    async fn latest_successful_verification(
        &self,
        job: JobId,
    ) -> Result<Option<VerificationRecord>, StorageError> {
        if let Some(v) = self.cache.lock().latest_success.get(&job) {
            return Ok(v.clone());
        }
        self.shared.latest_successful_verification(job).await
    }

    /// Also kept for the rest of the request (see
    /// [`PgUserStore::opportunity_records_many`]).
    async fn latest_verifications_of(
        &self,
        jobs: &[JobId],
    ) -> Result<HashMap<JobId, LatestVerifications>, StorageError> {
        let found = self.shared.latest_verifications_of(jobs).await?;
        let mut cache = self.cache.lock();
        for job in jobs {
            let known = found.get(job).cloned().unwrap_or_default();
            cache.latest.insert(*job, known.latest);
            cache.latest_success.insert(*job, known.last_success);
        }
        Ok(found)
    }

    async fn record_observation(
        &self,
        posting: &JobPosting,
        observed_at: DateTime<Utc>,
    ) -> Result<UpsertOutcome, StorageError> {
        self.cache.clear();
        self.shared.record_observation(posting, observed_at).await
    }
}

#[async_trait]
impl EligibilityRepository for PgUserStore {
    async fn cached_decision(
        &self,
        key: &CacheKey,
    ) -> Result<Option<EligibilityDecision>, StorageError> {
        let row: Option<Vec<u8>> = sqlx::query_scalar(
            "SELECT decision FROM eligibility_decisions WHERE user_id = $1 \
             AND opportunity_id = $2 AND profile_id = $3 AND cache_key = $4",
        )
        .bind(self.user.as_str())
        .bind(key.opportunity.to_string())
        .bind(&key.profile_id)
        .bind(&key.key)
        .fetch_optional(self.shared.pool())
        .await
        .map_err(query_error("loading an eligibility decision"))?;
        row.map(|sealed| {
            let json = self.open(&eligibility_context(&self.user, key), &sealed)?;
            serde_json::from_slice(&json)
                .map_err(|e| corrupt(&key.key, format!("eligibility decision: {e}")))
        })
        .transpose()
    }

    async fn store_decision(
        &self,
        key: &CacheKey,
        decision: &EligibilityDecision,
        at: DateTime<Utc>,
    ) -> Result<(), StorageError> {
        let json = serde_json::to_vec(decision).map_err(|e| StorageError::Query {
            operation: "encoding an eligibility decision",
            source: Box::new(e),
        })?;
        let sealed = self.seal(&eligibility_context(&self.user, key), &json)?;
        sqlx::query(
            "INSERT INTO eligibility_decisions (user_id, opportunity_id, profile_id, cache_key, \
             profile_revision, rules_version, status, decided_at, decision) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) \
             ON CONFLICT (user_id, opportunity_id, profile_id, cache_key) DO UPDATE SET \
             status = excluded.status, decided_at = excluded.decided_at, \
             decision = excluded.decision",
        )
        .bind(self.user.as_str())
        .bind(key.opportunity.to_string())
        .bind(&key.profile_id)
        .bind(&key.key)
        .bind(i64::try_from(key.profile_revision).unwrap_or(i64::MAX))
        .bind(&decision.rules_version)
        .bind(decision.status.as_str())
        .bind(at)
        .bind(sealed)
        .execute(self.shared.pool())
        .await
        .map_err(query_error("storing an eligibility decision"))?;
        Ok(())
    }

    /// One query for a whole search (a lookup per opportunity is a network
    /// round trip each).
    async fn cached_decisions(
        &self,
        keys: &[CacheKey],
    ) -> Result<HashMap<String, EligibilityDecision>, StorageError> {
        let by_key: HashMap<&str, &CacheKey> = keys.iter().map(|k| (k.key.as_str(), k)).collect();
        let opportunities: Vec<String> = keys.iter().map(|k| k.opportunity.to_string()).collect();
        let profiles: Vec<&str> = keys.iter().map(|k| k.profile_id.as_str()).collect();
        let digests: Vec<&str> = keys.iter().map(|k| k.key.as_str()).collect();
        let rows: Vec<(String, Vec<u8>)> = sqlx::query_as(
            "SELECT d.cache_key, d.decision FROM eligibility_decisions d \
             JOIN unnest($2::text[], $3::text[], $4::text[]) AS k(opportunity_id, profile_id, cache_key) \
             ON d.opportunity_id = k.opportunity_id AND d.profile_id = k.profile_id \
             AND d.cache_key = k.cache_key \
             WHERE d.user_id = $1",
        )
        .bind(self.user.as_str())
        .bind(&opportunities)
        .bind(&profiles)
        .bind(&digests)
        .fetch_all(self.shared.pool())
        .await
        .map_err(query_error("loading eligibility decisions"))?;
        let mut out = HashMap::with_capacity(rows.len());
        for (digest, sealed) in rows {
            let Some(key) = by_key.get(digest.as_str()) else {
                continue;
            };
            let json = self.open(&eligibility_context(&self.user, key), &sealed)?;
            let decision = serde_json::from_slice(&json)
                .map_err(|e| corrupt(&digest, format!("eligibility decision: {e}")))?;
            out.insert(digest, decision);
        }
        Ok(out)
    }

    /// One statement for every new decision of a search.
    async fn store_decisions(
        &self,
        decisions: &[(CacheKey, EligibilityDecision)],
        at: DateTime<Utc>,
    ) -> Result<(), StorageError> {
        let mut opportunities = Vec::with_capacity(decisions.len());
        let mut profiles = Vec::with_capacity(decisions.len());
        let mut digests = Vec::with_capacity(decisions.len());
        let mut revisions = Vec::with_capacity(decisions.len());
        let mut rules = Vec::with_capacity(decisions.len());
        let mut statuses = Vec::with_capacity(decisions.len());
        let mut sealed = Vec::with_capacity(decisions.len());
        let mut seen = std::collections::HashSet::new();
        for (key, decision) in decisions {
            // One row per key: a statement cannot upsert the same row twice.
            if !seen.insert((key.opportunity, key.profile_id.as_str(), key.key.as_str())) {
                continue;
            }
            let json = serde_json::to_vec(decision).map_err(|e| StorageError::Query {
                operation: "encoding an eligibility decision",
                source: Box::new(e),
            })?;
            sealed.push(self.seal(&eligibility_context(&self.user, key), &json)?);
            opportunities.push(key.opportunity.to_string());
            profiles.push(key.profile_id.clone());
            digests.push(key.key.clone());
            revisions.push(i64::try_from(key.profile_revision).unwrap_or(i64::MAX));
            rules.push(decision.rules_version.clone());
            statuses.push(decision.status.as_str().to_owned());
        }
        if sealed.is_empty() {
            return Ok(());
        }
        sqlx::query(
            "INSERT INTO eligibility_decisions (user_id, opportunity_id, profile_id, cache_key, \
             profile_revision, rules_version, status, decided_at, decision) \
             SELECT $1, o, p, k, r, v, s, $8, d \
             FROM unnest($2::text[], $3::text[], $4::text[], $5::bigint[], $6::text[], \
             $7::text[], $9::bytea[]) AS t(o, p, k, r, v, s, d) \
             ON CONFLICT (user_id, opportunity_id, profile_id, cache_key) DO UPDATE SET \
             status = excluded.status, decided_at = excluded.decided_at, \
             decision = excluded.decision",
        )
        .bind(self.user.as_str())
        .bind(&opportunities)
        .bind(&profiles)
        .bind(&digests)
        .bind(&revisions)
        .bind(&rules)
        .bind(&statuses)
        .bind(at)
        .bind(&sealed)
        .execute(self.shared.pool())
        .await
        .map_err(query_error("storing eligibility decisions"))?;
        Ok(())
    }
}

fn eligibility_context(user: &UserId, key: &CacheKey) -> String {
    format!("eligibility|{user}|{}|{}", key.opportunity, key.key)
}

fn ranking_context(user: &UserId, key: &RankKey) -> String {
    format!("rankings|{user}|{}|{}", key.opportunity, key.key)
}

fn review_context(user: &UserId, profile_id: &str, key: &str) -> String {
    format!("fit_reviews|{user}|{profile_id}|{key}")
}

const FEEDBACK_COLUMNS: &str =
    "id, profile_id, opportunity_id, job_id, action, reason, title, company, recorded_at";

#[async_trait]
impl FeedbackRepository for PgUserStore {
    async fn record_feedback(&self, event: &FeedbackEvent) -> Result<(), StorageError> {
        let mut tx = self
            .shared
            .pool()
            .begin()
            .await
            .map_err(query_error("starting a transaction"))?;
        let seq = next_seq(&mut tx, &self.user).await?;
        self.insert_feedback(&mut tx, event, seq, false).await?;
        tx.commit()
            .await
            .map_err(query_error("committing feedback"))?;
        Ok(())
    }

    async fn feedback(&self, profile_id: &str) -> Result<Vec<FeedbackEvent>, StorageError> {
        let rows = sqlx::query(&format!(
            "SELECT {FEEDBACK_COLUMNS} FROM feedback WHERE user_id = $1 AND profile_id = $2 \
             ORDER BY recorded_at, id"
        ))
        .bind(self.user.as_str())
        .bind(profile_id)
        .fetch_all(self.shared.pool())
        .await
        .map_err(query_error("loading feedback"))?;
        rows.iter().map(|r| self.decode_feedback(r)).collect()
    }

    async fn feedback_for_jobs(
        &self,
        profile_id: &str,
        jobs: &[JobId],
    ) -> Result<Vec<FeedbackEvent>, StorageError> {
        if jobs.is_empty() {
            return Ok(Vec::new());
        }
        let ids: Vec<String> = jobs.iter().map(ToString::to_string).collect();
        let rows = sqlx::query(&format!(
            "SELECT {FEEDBACK_COLUMNS} FROM feedback WHERE user_id = $1 AND profile_id = $2 \
             AND job_id = ANY($3) ORDER BY recorded_at, id"
        ))
        .bind(self.user.as_str())
        .bind(profile_id)
        .bind(&ids)
        .fetch_all(self.shared.pool())
        .await
        .map_err(query_error("loading an opportunity's feedback"))?;
        rows.iter().map(|r| self.decode_feedback(r)).collect()
    }
}

#[async_trait]
impl RankingRepository for PgUserStore {
    async fn cached_ranking(&self, key: &RankKey) -> Result<Option<Ranking>, StorageError> {
        let row: Option<Vec<u8>> = sqlx::query_scalar(
            "SELECT ranking FROM rankings WHERE user_id = $1 AND opportunity_id = $2 \
             AND profile_id = $3 AND rank_key = $4",
        )
        .bind(self.user.as_str())
        .bind(key.opportunity.to_string())
        .bind(&key.profile_id)
        .bind(&key.key)
        .fetch_optional(self.shared.pool())
        .await
        .map_err(query_error("loading a ranking"))?;
        row.map(|sealed| {
            let json = self.open(&ranking_context(&self.user, key), &sealed)?;
            serde_json::from_slice(&json).map_err(|e| corrupt(&key.key, format!("ranking: {e}")))
        })
        .transpose()
    }

    async fn store_ranking(
        &self,
        key: &RankKey,
        ranking: &Ranking,
        at: DateTime<Utc>,
    ) -> Result<(), StorageError> {
        let json = serde_json::to_vec(ranking).map_err(|e| StorageError::Query {
            operation: "encoding a ranking",
            source: Box::new(e),
        })?;
        let sealed = self.seal(&ranking_context(&self.user, key), &json)?;
        sqlx::query(
            "INSERT INTO rankings (user_id, opportunity_id, profile_id, rank_key, \
             ranking_version, tier, ranked_at, ranking) VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
             ON CONFLICT (user_id, opportunity_id, profile_id, rank_key) DO UPDATE SET \
             tier = excluded.tier, ranked_at = excluded.ranked_at, ranking = excluded.ranking",
        )
        .bind(self.user.as_str())
        .bind(key.opportunity.to_string())
        .bind(&key.profile_id)
        .bind(&key.key)
        .bind(&ranking.ranking_version)
        .bind(ranking.tier.as_str())
        .bind(at)
        .bind(sealed)
        .execute(self.shared.pool())
        .await
        .map_err(query_error("storing a ranking"))?;
        Ok(())
    }

    async fn cached_fit_review(
        &self,
        profile_id: &str,
        key: &str,
    ) -> Result<Option<FitReview>, StorageError> {
        let row: Option<Vec<u8>> = sqlx::query_scalar(
            "SELECT review FROM fit_reviews WHERE user_id = $1 AND profile_id = $2 \
             AND review_key = $3",
        )
        .bind(self.user.as_str())
        .bind(profile_id)
        .bind(key)
        .fetch_optional(self.shared.pool())
        .await
        .map_err(query_error("loading a fit review"))?;
        row.map(|sealed| {
            let json = self.open(&review_context(&self.user, profile_id, key), &sealed)?;
            serde_json::from_slice(&json).map_err(|e| corrupt(key, format!("fit review: {e}")))
        })
        .transpose()
    }

    async fn store_fit_review(
        &self,
        profile_id: &str,
        key: &str,
        review: &FitReview,
        at: DateTime<Utc>,
    ) -> Result<(), StorageError> {
        let json = serde_json::to_vec(review).map_err(|e| StorageError::Query {
            operation: "encoding a fit review",
            source: Box::new(e),
        })?;
        let sealed = self.seal(&review_context(&self.user, profile_id, key), &json)?;
        sqlx::query(
            "INSERT INTO fit_reviews (user_id, profile_id, review_key, reviewer, fit, \
             reviewed_at, review) VALUES ($1, $2, $3, $4, $5, $6, $7) \
             ON CONFLICT (user_id, profile_id, review_key) DO UPDATE SET \
             reviewer = excluded.reviewer, fit = excluded.fit, \
             reviewed_at = excluded.reviewed_at, review = excluded.review",
        )
        .bind(self.user.as_str())
        .bind(profile_id)
        .bind(key)
        .bind(&review.reviewer)
        .bind(review.fit.as_str())
        .bind(at)
        .bind(sealed)
        .execute(self.shared.pool())
        .await
        .map_err(query_error("storing a fit review"))?;
        Ok(())
    }
}

#[async_trait]
impl Store for PgUserStore {
    fn location(&self) -> String {
        self.shared.location().to_owned()
    }

    async fn opportunities_with_prefix(
        &self,
        prefix: &str,
        limit: usize,
    ) -> Result<Vec<OpportunityId>, StorageError> {
        self.shared.opportunities_with_prefix(prefix, limit).await
    }

    async fn jobs_with_prefix(
        &self,
        prefix: &str,
        limit: usize,
    ) -> Result<Vec<JobId>, StorageError> {
        self.shared.jobs_with_prefix(prefix, limit).await
    }

    async fn last_checked(&self) -> Result<HashMap<SourceKey, DateTime<Utc>>, StorageError> {
        self.shared.last_checked().await
    }

    async fn recent_scans(
        &self,
        per_source: usize,
    ) -> Result<HashMap<SourceKey, Vec<ScanRecord>>, StorageError> {
        self.shared.recent_scans(per_source).await
    }

    async fn stats(&self) -> Result<StoreStats, StorageError> {
        let (jobs, open_jobs, open_opportunities, verifications): (i64, i64, i64, i64) =
            sqlx::query_as(
                "SELECT (SELECT COUNT(*) FROM jobs), \
                 (SELECT COUNT(*) FROM jobs WHERE status = 'open'), \
                 (SELECT COUNT(DISTINCT opportunity_id) FROM jobs WHERE status = 'open'), \
                 (SELECT COUNT(*) FROM job_verifications)",
            )
            .fetch_one(self.shared.pool())
            .await
            .map_err(query_error("counting stored records"))?;
        let feedback: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM feedback WHERE user_id = $1 AND action <> 'seen'",
        )
        .bind(self.user.as_str())
        .fetch_one(self.shared.pool())
        .await
        .map_err(query_error("counting feedback"))?;
        let schema = self.shared.schema().await?;
        let n = |v: i64| u64::try_from(v).unwrap_or(0);
        Ok(StoreStats {
            jobs: n(jobs),
            open_jobs: n(open_jobs),
            open_opportunities: n(open_opportunities),
            verifications: n(verifications),
            feedback: n(feedback),
            migrations_applied: schema.applied,
            migrations_known: schema.known,
            latest_migration: schema.latest,
        })
    }

    async fn import_state(
        &self,
        state: StateImport<'_>,
    ) -> Result<StateImported, jobhunt_profile::StorageError> {
        self.import_state_tx(state).await
    }

    async fn import_verification(&self, record: &VerificationRecord) -> Result<bool, StorageError> {
        self.cache.clear();
        self.shared.insert_verification(record, true).await
    }

    async fn write_lock(&self) -> Result<WriteGuard, StorageError> {
        Ok(WriteGuard::new(AdvisoryGuard(Some(self.lock().await?))))
    }

    async fn record_shown(&self, shown: &[Shown], at: DateTime<Utc>) -> Result<(), StorageError> {
        let mut tx = self
            .shared
            .pool()
            .begin()
            .await
            .map_err(query_error("starting a transaction"))?;
        let seq = next_seq(&mut tx, &self.user).await?;
        for s in shown {
            sqlx::query(
                "INSERT INTO user_opportunities (user_id, opportunity_id, first_shown_at, \
                 last_shown_at, last_tier, times_shown, seq) VALUES ($1, $2, $3, $3, $4, 1, $5) \
                 ON CONFLICT (user_id, opportunity_id) DO UPDATE SET \
                 first_shown_at = CASE WHEN user_opportunities.times_shown = 0 \
                 THEN excluded.first_shown_at ELSE user_opportunities.first_shown_at END, \
                 last_shown_at = excluded.last_shown_at, last_tier = excluded.last_tier, \
                 times_shown = user_opportunities.times_shown + 1, seq = excluded.seq",
            )
            .bind(self.user.as_str())
            .bind(s.opportunity.to_string())
            .bind(at)
            .bind(s.tier.as_str())
            .bind(seq)
            .execute(&mut *tx)
            .await
            .map_err(query_error("recording a shortlist"))?;
        }
        let revision: Option<i64> =
            sqlx::query_scalar("SELECT MAX(revision) FROM profiles WHERE user_id = $1")
                .bind(self.user.as_str())
                .fetch_one(&mut *tx)
                .await
                .map_err(query_error("recording a shortlist"))?;
        sqlx::query(
            "INSERT INTO user_state (user_id, last_shortlist_at, shortlist_profile_revision) \
             VALUES ($1, $2, $3) ON CONFLICT (user_id) DO UPDATE SET \
             last_shortlist_at = excluded.last_shortlist_at, \
             shortlist_profile_revision = excluded.shortlist_profile_revision",
        )
        .bind(self.user.as_str())
        .bind(at)
        .bind(revision)
        .execute(&mut *tx)
        .await
        .map_err(query_error("recording a shortlist"))?;
        tx.commit()
            .await
            .map_err(query_error("committing a shortlist"))?;
        Ok(())
    }

    async fn feed_marks(
        &self,
        ids: &[OpportunityId],
    ) -> Result<HashMap<OpportunityId, FeedMark>, StorageError> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let wanted: Vec<String> = ids.iter().map(ToString::to_string).collect();
        type Row = (
            String,
            Option<DateTime<Utc>>,
            Option<DateTime<Utc>>,
            i32,
            Option<DateTime<Utc>>,
            Option<DateTime<Utc>>,
        );
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT opportunity_id, \
             CASE WHEN times_shown > 0 THEN first_shown_at END, \
             CASE WHEN times_shown > 0 THEN last_shown_at END, \
             times_shown, resurfaced_at, dismissed_at \
             FROM user_opportunities WHERE user_id = $1 AND opportunity_id = ANY($2)",
        )
        .bind(self.user.as_str())
        .bind(&wanted)
        .fetch_all(self.shared.pool())
        .await
        .map_err(query_error("loading the feed's state"))?;
        rows.into_iter()
            .map(|(id, first, last, times, resurfaced, dismissed)| {
                let opportunity = id.parse().map_err(|e| corrupt(&id, e))?;
                Ok((
                    opportunity,
                    FeedMark {
                        first_shown_at: first,
                        last_shown_at: last,
                        times_shown: u32::try_from(times).unwrap_or(0),
                        resurfaced_at: resurfaced,
                        dismissed_at: dismissed,
                    },
                ))
            })
            .collect()
    }

    async fn record_resurfaced(
        &self,
        ids: &[OpportunityId],
        at: DateTime<Utc>,
    ) -> Result<(), StorageError> {
        if ids.is_empty() {
            return Ok(());
        }
        let wanted: Vec<String> = ids.iter().map(ToString::to_string).collect();
        sqlx::query(
            "UPDATE user_opportunities SET resurfaced_at = $3 \
             WHERE user_id = $1 AND opportunity_id = ANY($2)",
        )
        .bind(self.user.as_str())
        .bind(&wanted)
        .bind(at)
        .execute(self.shared.pool())
        .await
        .map_err(query_error("recording resurfaced opportunities"))?;
        Ok(())
    }

    async fn record_dismissed(
        &self,
        id: OpportunityId,
        at: DateTime<Utc>,
    ) -> Result<(), StorageError> {
        let mut tx = self
            .shared
            .pool()
            .begin()
            .await
            .map_err(query_error("starting a transaction"))?;
        let seq = next_seq(&mut tx, &self.user).await?;
        sqlx::query(
            "INSERT INTO user_opportunities (user_id, opportunity_id, first_shown_at, \
             last_shown_at, last_tier, times_shown, seq, dismissed_at) \
             VALUES ($1, $2, $3, $3, NULL, 0, $4, $3) \
             ON CONFLICT (user_id, opportunity_id) DO UPDATE SET dismissed_at = excluded.dismissed_at",
        )
        .bind(self.user.as_str())
        .bind(id.to_string())
        .bind(at)
        .bind(seq)
        .execute(&mut *tx)
        .await
        .map_err(query_error("putting an opportunity aside"))?;
        tx.commit()
            .await
            .map_err(query_error("putting an opportunity aside"))?;
        Ok(())
    }

    async fn next_discovery_due(&self) -> Result<Option<DateTime<Utc>>, StorageError> {
        sqlx::query_scalar("SELECT MIN(next_due_at) FROM source_schedule WHERE enabled")
            .fetch_one(self.shared.pool())
            .await
            .map_err(query_error("reading the discovery schedule"))
    }

    async fn shutdown(&self) {
        // The pool is shared by every request; the server closes it once.
    }
}
