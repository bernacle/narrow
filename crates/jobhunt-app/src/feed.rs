//! The Today feed: the few recommendations that are *new to the person*,
//! and the ones they already looked at that changed in a way that matters.
//!
//! The shortlist ([`crate::shortlist`]) answers "what is worth my time?"
//! every time it is asked, so the same jobs come back until the person acts
//! on them. The feed answers "what is worth my time *that I haven't dealt
//! with*?", so a visit can end: once the few items are saved, rejected,
//! marked applied or put aside, the person is caught up, and nothing
//! mediocre takes their place.
//!
//! It is built from the same ranking (fit, practicality, gates, briefs:
//! [`jobhunt_ranking`]); only the selection is its own. Today is the small
//! set Narrow puts its name behind: **strong fits** only
//! ([`qualifies_for_today`]), whatever their practical unknowns (pay not
//! published, remote scope unclear: shown as things to check). A plausible
//! fit ("worth reviewing") stays in search and never fills Today; Today may
//! hold one job, or none.
//!
//! | The person's state | On the feed |
//! | --- | --- |
//! | never acted on it, never shown it | **new** |
//! | never acted on it, first shown less than [`HOLD_HOURS`] ago | **new** (still; reloading does not make it vanish) |
//! | never acted on it, shown longer ago | passed over (counted, not shown) |
//! | looked at / put aside ("not now") / saved | only if it **changed** materially since |
//! | rejected, applied, interviewing, offer, closed, ineligible | never (the ranking gate excludes it) |
//!
//! Only strong fits are candidates: a feed with nothing new is *caught up*,
//! not padded with weaker jobs, and there is no quota.
//!
//! A material change is one the job's history records (discovery's
//! UPDATED / REOPENED events, see [`jobhunt_jobs::lifecycle`]) in a field
//! that changes whether someone would want or could take the job: pay
//! published or changed, location or remote policy, work authorization,
//! employment type, or a closed job reopening. Edits to the title, the
//! description or links do not bring a job back. A resurfaced job stays for
//! [`HOLD_HOURS`] too, and is never brought back twice for the same change.
//!
//! Where the store keeps per-person search state (the cloud), what the
//! feed showed is remembered; locally it is not, so there "new" simply
//! means not acted on yet.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use chrono::{DateTime, Duration, Utc};
use jobhunt_jobs::verification::CompensationCheck;
use jobhunt_jobs::{JobEventKind, JobId, JobRecord, OpportunityId};
use jobhunt_ranking::{
    FeedbackAction, FeedbackEvent, Gate, OpportunityState, RankQuery, RankReport, Ranking, Stage,
    Tier,
};
use jobhunt_storage::{FeedMark, Shown};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::discover::{Refresh, RefreshMode, RefreshReason};
use crate::error::AppError;
use crate::feedback::FeedbackOutcome;
use crate::freshness::Freshness;
use crate::resolve::Opportunity;
use crate::shortlist::{Entry, Learning, RefreshSummary, ShortlistItem};
use crate::views::{FitTier, PipelineStage, time};
use crate::{DiscoveryMode, LocalApp, Progress};

/// Items a feed shows by default.
pub const DEFAULT_FEED_LIMIT: usize = 5;
/// The most a feed shows: it is meant to be finished.
pub const MAX_FEED_LIMIT: usize = 10;
/// How long a recommendation stays on the feed after it was first shown
/// (or resurfaced) without the person acting on it.
pub const HOLD_HOURS: i64 = 24;
/// Recommendations (best first) whose history is checked for material
/// changes. Beyond these, a change does not bring a job back.
const CHANGE_POOL: usize = 40;

fn hold() -> Duration {
    Duration::hours(HOLD_HOURS)
}

/// What to put on the feed.
#[derive(Debug, Clone)]
pub struct FeedRequest {
    /// At most this many items (1 to [`MAX_FEED_LIMIT`]).
    pub limit: usize,
    /// Verify candidates whose listings aren't trusted yet before showing
    /// them (as searches do).
    pub verify: bool,
    /// Whether to read job boards first (the local product; the cloud never
    /// does in a request).
    pub refresh: RefreshMode,
}

impl Default for FeedRequest {
    fn default() -> Self {
        Self {
            limit: DEFAULT_FEED_LIMIT,
            verify: true,
            refresh: RefreshMode::Auto,
        }
    }
}

/// Why an item is on the feed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FeedReason {
    /// The person has not dealt with it yet.
    New,
    /// The person looked at it, put it aside or saved it, and it changed
    /// materially since (see `changes`).
    Changed,
}

/// One item selected for the feed.
#[derive(Debug, Clone)]
pub struct FeedEntry {
    pub entry: Entry,
    pub reason: FeedReason,
    /// What changed (for [`FeedReason::Changed`]).
    pub changes: Vec<String>,
    pub stage: Stage,
    pub first_shown_at: Option<DateTime<Utc>>,
    /// The company's other recommendations, held off the feed (best first).
    pub also_at_company: Vec<SameCompany>,
}

/// Another recommendation at the same company as a feed item, not on the
/// feed itself (and not recorded as shown).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SameCompany {
    /// `opp_…`.
    pub id: String,
    pub title: String,
}

/// Whether a ranking earns a place on Today ("worth your attention"): a
/// strong fit, not excluded. Plausible fits never do, however few strong
/// ones there are.
pub fn qualifies_for_today(r: &Ranking) -> bool {
    !r.gate.is_excluded() && r.tier == Tier::StrongFit
}

/// Today's picks, in rank order: each company's best-ranked candidate,
/// until `limit` companies are on it. A company's other candidates go
/// with its pick instead of taking a slot; they are strong fits on their
/// own (only strong fits are candidates), never roles carried by the
/// company's best one. Nothing pads the feed when fewer companies
/// qualify. One rule, no score: Today is a few distinct decisions, and "do
/// I want this company?" is one of them.
fn one_per_company<T>(
    candidates: Vec<T>,
    limit: usize,
    company: impl Fn(&T) -> String,
) -> Vec<(T, Vec<T>)> {
    let mut picks: Vec<(T, Vec<T>)> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    for candidate in candidates {
        let key = company(&candidate);
        match at.get(&key) {
            Some(i) => picks[*i].1.push(candidate),
            None if picks.len() < limit => {
                at.insert(key, picks.len());
                picks.push((candidate, Vec::new()));
            }
            None => {}
        }
    }
    picks
}

/// Opportunities in the person's pipeline, by stage.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PipelineCounts {
    pub saved: usize,
    pub applied: usize,
    pub interviewing: usize,
    pub offer: usize,
}

/// The feed, as computed.
#[derive(Debug)]
pub struct Feed {
    pub refresh: Refresh,
    pub report: RankReport,
    pub entries: Vec<FeedEntry>,
    /// Recommendations new to the person (shown or not).
    pub new_total: usize,
    /// Reviewed recommendations that changed materially.
    pub changed_total: usize,
    /// Recommendations shown earlier that the person let pass.
    pub passed_over: usize,
    pub pipeline: PipelineCounts,
    pub verified_now: usize,
    /// When a job board was last read.
    pub last_read_at: Option<DateTime<Utc>>,
    /// When the background discovery next reads one (the cloud).
    pub next_read_at: Option<DateTime<Utc>>,
    pub mode: DiscoveryMode,
}

/// A recommendation the feed could show, before any is chosen.
struct Candidate<'r> {
    ranking: &'r Ranking,
    records: Vec<JobRecord>,
    state: OpportunityState,
    reason: FeedReason,
    changes: Vec<String>,
    /// The material change it resurfaces for.
    change_at: Option<DateTime<Utc>>,
    mark: Option<FeedMark>,
}

struct Classified<'r> {
    candidates: Vec<Candidate<'r>>,
    new_total: usize,
    changed_total: usize,
    passed_over: usize,
}

/// A material change found in a job's history.
fn describe_change(
    record: &JobRecord,
    kind: JobEventKind,
    fields: &[String],
    previous: Option<&jobhunt_jobs::JobSnapshot>,
) -> Vec<String> {
    let mut out = Vec::new();
    match kind {
        JobEventKind::Reopened => out.push("It reopened after being closed".to_owned()),
        JobEventKind::Updated => {
            for field in fields {
                match field.as_str() {
                    "compensation" => {
                        let before = previous.and_then(|p| p.compensation.as_ref());
                        let now =
                            CompensationCheck::observe(record.posting.compensation.as_ref(), None);
                        let range = now.ranges.first().map(|r| r.describe());
                        match (before, range) {
                            // Pay withdrawn is not a reason to come back.
                            (_, None) => {}
                            (None, Some(range)) => {
                                out.push(format!("Pay is now published: {range}"))
                            }
                            (Some(_), Some(range)) => out.push(format!("Pay changed: {range}")),
                        }
                    }
                    "location" | "workplace" => {
                        out.push("Its location or remote policy changed".to_owned());
                    }
                    "work_authorization" => {
                        out.push("Its work authorization terms changed".to_owned());
                    }
                    "employment_type" => out.push("Its employment type changed".to_owned()),
                    // Title, description, links, dates: not a reason.
                    _ => {}
                }
            }
        }
        JobEventKind::New | JobEventKind::Closed => {}
    }
    out
}

/// Where preparing Today took its time (milliseconds), logged with every
/// feed; ranking's own parts come from [`jobhunt_ranking::RankTimings`].
#[derive(Debug, Clone, Copy, Default)]
struct FeedTimings {
    /// Profile check and feedback, outside the ranking.
    prepare_ms: u64,
    verify_ms: u64,
    rerank_ms: u64,
    classify_ms: u64,
    /// Checking and recording what is shown.
    select_ms: u64,
}

fn ms_since(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn report_ms(t: &jobhunt_ranking::RankTimings) -> u64 {
    t.person_ms + t.load_ms + t.eligibility_ms + t.ranking_ms + t.review_ms
}

impl LocalApp {
    /// Material changes to an opportunity's records after `since`, and when
    /// the latest happened, from their `histories`.
    fn material_changes(
        records: &[JobRecord],
        since: DateTime<Utc>,
        histories: &HashMap<JobId, Vec<jobhunt_jobs::JobEvent>>,
    ) -> (Vec<String>, Option<DateTime<Utc>>) {
        let mut changes: Vec<String> = Vec::new();
        let mut latest: Option<DateTime<Utc>> = None;
        for record in records {
            for event in histories.get(&record.id).into_iter().flatten() {
                if event.at <= since {
                    continue;
                }
                let found = describe_change(
                    record,
                    event.kind,
                    &event.changed_fields,
                    event.previous.as_ref(),
                );
                if found.is_empty() {
                    continue;
                }
                latest = latest.max(Some(event.at));
                for change in found {
                    if !changes.contains(&change) {
                        changes.push(change);
                    }
                }
            }
        }
        (changes, latest)
    }

    /// Sorts every strong fit into new, changed and passed over (see the
    /// module docs).
    async fn classify<'r>(
        &self,
        report: &'r RankReport,
        feedback: &HashMap<JobId, Vec<FeedbackEvent>>,
        now: DateTime<Utc>,
    ) -> Result<Classified<'r>, AppError> {
        let worth: Vec<&Ranking> = report
            .rankings
            .iter()
            .filter(|r| qualifies_for_today(r))
            .collect();
        let ids: Vec<OpportunityId> = worth.iter().map(|r| r.opportunity).collect();
        let marks = self.store().feed_marks(&ids).await?;
        // Records of every recommendation, and the history of those whose
        // changes can bring them back: one repository call each.
        let mut records_of = self.store().opportunity_records_many(&ids).await?;
        let pool: Vec<JobId> = ids
            .iter()
            .take(CHANGE_POOL)
            .filter_map(|id| records_of.get(id))
            .flatten()
            .map(|r| r.id)
            .collect();
        let histories = if pool.is_empty() {
            HashMap::new()
        } else {
            self.store().histories(&pool).await?
        };
        let mut out = Classified {
            candidates: Vec::new(),
            new_total: 0,
            changed_total: 0,
            passed_over: 0,
        };
        for (position, ranking) in worth.into_iter().enumerate() {
            let records = records_of.remove(&ranking.opportunity).unwrap_or_default();
            if records.is_empty() {
                continue;
            }
            let state = OpportunityState::of(
                records
                    .iter()
                    .flat_map(|r| feedback.get(&r.id).cloned().unwrap_or_default()),
            );
            let mark = marks.get(&ranking.opportunity).cloned();
            let first_shown = mark.as_ref().and_then(|m| m.first_shown_at);
            // When the person last dealt with it: their latest action, or,
            // for one they let pass, when it was first put in front of them.
            let reference = match state.stage {
                Stage::Unseen => match first_shown {
                    Some(first) if now - first >= hold() => Some(first),
                    _ => {
                        out.new_total += 1;
                        out.candidates.push(Candidate {
                            ranking,
                            records,
                            state,
                            reason: FeedReason::New,
                            changes: Vec::new(),
                            change_at: None,
                            mark,
                        });
                        continue;
                    }
                },
                Stage::Seen | Stage::Saved => state.events.last().map(|e| e.at),
                // The gate excludes the rest; nothing to do.
                _ => continue,
            };
            let Some(reference) = reference else { continue };
            let dismissed = mark.as_ref().and_then(|m| m.dismissed_at);
            let reference = reference.max(dismissed.unwrap_or(reference));
            let (changes, change_at) = if position < CHANGE_POOL {
                Self::material_changes(&records, reference, &histories)
            } else {
                (Vec::new(), None)
            };
            let resurfaced = mark.as_ref().and_then(|m| m.resurfaced_at);
            let changed = match change_at {
                None => false,
                // Already brought back for this change and let pass again.
                Some(at) => !resurfaced.is_some_and(|r| r >= at && now - r >= hold()),
            };
            if changed {
                out.changed_total += 1;
                out.candidates.push(Candidate {
                    ranking,
                    records,
                    state,
                    reason: FeedReason::Changed,
                    changes,
                    change_at,
                    mark,
                });
            } else if state.stage == Stage::Unseen {
                out.passed_over += 1;
            }
        }
        Ok(out)
    }

    /// The Today feed (see the module docs). Fails with
    /// [`AppError::NoProfile`] without a profile; with nothing discovered
    /// yet it is simply empty.
    pub async fn feed(
        &self,
        request: &FeedRequest,
        progress: &dyn Progress,
        now: DateTime<Utc>,
    ) -> Result<Feed, AppError> {
        if request.limit == 0 || request.limit > MAX_FEED_LIMIT {
            return Err(AppError::InvalidArguments(format!(
                "limit must be between 1 and {MAX_FEED_LIMIT}"
            )));
        }
        let started = Instant::now();
        if self.profile_facts().await?.is_none() {
            return Err(AppError::NoProfile);
        }
        let refresh = self
            .refresh_for_search(request.refresh, progress, now)
            .await?;
        let ranking = self.ranking();
        let query = RankQuery {
            text: String::new(),
            store_top: 0,
            all: false,
        };
        let feedback = self.feedback_by_job().await?;
        let mut report = ranking.rank(&query, now).await?;
        let first_rank = report.timings;
        let mut timings = FeedTimings {
            prepare_ms: ms_since(started).saturating_sub(report_ms(&first_rank)),
            ..FeedTimings::default()
        };
        let mut verified_now = 0;
        if request.verify {
            let phase = Instant::now();
            let pool = request.limit * 2;
            let classified = self.classify(&report, &feedback, now).await?;
            let chosen: Vec<&Ranking> = classified
                .candidates
                .iter()
                .take(pool)
                .map(|c| c.ranking)
                .collect();
            verified_now = self.verify_candidates(&chosen, pool, progress, now).await?;
            drop(classified);
            timings.verify_ms = ms_since(phase);
            if verified_now > 0 {
                let phase = Instant::now();
                report = ranking.rank(&query, now).await?;
                timings.rerank_ms = ms_since(phase);
            }
        }
        let phase = Instant::now();
        let Classified {
            candidates,
            new_total,
            changed_total,
            passed_over,
        } = self.classify(&report, &feedback, now).await?;
        timings.classify_ms = ms_since(phase);
        let phase = Instant::now();
        let mut entries = Vec::new();
        let mut shown = Vec::new();
        let mut resurfaced = Vec::new();
        let picks = one_per_company(candidates, request.limit, |c| {
            jobhunt_core::text::search_key(&c.ranking.facets.company)
        });
        for (c, others) in picks {
            let also_at_company = others
                .iter()
                .map(|o| SameCompany {
                    id: o.ranking.opportunity.to_string(),
                    title: o.ranking.title.clone(),
                })
                .collect();
            let opportunity = Opportunity {
                id: c.ranking.opportunity,
                records: c.records,
            };
            let checked = self.check(&opportunity, now).await?;
            shown.push(Shown {
                opportunity: c.ranking.opportunity,
                tier: c.ranking.tier,
            });
            if c.reason == FeedReason::Changed
                && c.change_at
                    .is_some_and(|at| c.mark.as_ref().and_then(|m| m.resurfaced_at) < Some(at))
            {
                resurfaced.push(c.ranking.opportunity);
            }
            entries.push(FeedEntry {
                entry: Entry {
                    ranking: c.ranking.clone(),
                    checked,
                },
                reason: c.reason,
                changes: c.changes,
                stage: c.state.stage,
                first_shown_at: c.mark.and_then(|m| m.first_shown_at),
                also_at_company,
            });
        }
        // Per-person search state: best effort, it never fails the feed.
        if let Err(error) = self.store().record_shown(&shown, now).await {
            tracing::warn!(%error, "could not record the feed");
        }
        if let Err(error) = self.store().record_resurfaced(&resurfaced, now).await {
            tracing::warn!(%error, "could not record resurfaced opportunities");
        }
        timings.select_ms = ms_since(phase);
        let mut pipeline = PipelineCounts::default();
        for entry in ranking.pipeline(false).await? {
            match entry.state.stage {
                Stage::Saved => pipeline.saved += 1,
                Stage::Applied => pipeline.applied += 1,
                Stage::Interviewing => pipeline.interviewing += 1,
                Stage::Offer => pipeline.offer += 1,
                _ => {}
            }
        }
        tracing::info!(
            total_ms = ms_since(started),
            prepare_ms = timings.prepare_ms,
            person_ms = first_rank.person_ms,
            load_ms = first_rank.load_ms,
            eligibility_ms = first_rank.eligibility_ms,
            ranking_ms = first_rank.ranking_ms,
            verify_ms = timings.verify_ms,
            verified_now,
            rerank_ms = timings.rerank_ms,
            classify_ms = timings.classify_ms,
            select_ms = timings.select_ms,
            considered = report.considered,
            strong_fits = report
                .rankings
                .iter()
                .filter(|r| qualifies_for_today(r))
                .count(),
            review_ms = report.timings.review_ms,
            reviewed_from_cache = report.review.cache_hits,
            review_calls = report.review.calls,
            review_failures = report.review.failures,
            shown = entries.len(),
            "today prepared"
        );
        Ok(Feed {
            refresh,
            report,
            entries,
            new_total,
            changed_total,
            passed_over,
            pipeline,
            verified_now,
            last_read_at: self.store().last_checked().await?.into_values().max(),
            next_read_at: self.store().next_discovery_due().await?,
            mode: self.discovery_mode(),
        })
    }

    /// Every feedback event, by the record it was about.
    async fn feedback_by_job(&self) -> Result<HashMap<JobId, Vec<FeedbackEvent>>, AppError> {
        let mut by_job: HashMap<JobId, Vec<FeedbackEvent>> = HashMap::new();
        for event in self.ranking().feedback().await? {
            by_job.entry(event.job).or_default().push(event);
        }
        Ok(by_job)
    }

    /// Puts an opportunity aside for now: it leaves the feed, and nothing
    /// is learned from it (it is recorded as looked at, which carries no
    /// weight in taste). It comes back only if it changes materially.
    pub async fn dismiss(
        &self,
        opportunity: &Opportunity,
        now: DateTime<Utc>,
    ) -> Result<FeedbackOutcome, AppError> {
        let outcome = self
            .record_feedback(opportunity, FeedbackAction::Seen, None, now)
            .await?;
        self.store().record_dismissed(opportunity.id, now).await?;
        Ok(outcome)
    }

    /// The digest Narrow would send now (BRU-389 slice A): from the
    /// recommendations worth interrupting the person for (a strong fit,
    /// recommended outright: verified recently at an authoritative source,
    /// and eligible or conditionally eligible; new to them: never acted
    /// on, never shown in a feed or shortlist, never put aside; not in
    /// `request.exclude`, already notified), in rank order, one per
    /// company, preferring companies not notified about recently (see
    /// [`select_digest`]). Nothing is verified, nothing is recorded as
    /// shown: a notification never consumes Today.
    pub async fn digest(
        &self,
        request: &DigestRequest<'_>,
        now: DateTime<Utc>,
    ) -> Result<Digest, AppError> {
        if request.size == 0 || self.profile_facts().await?.is_none() {
            return Ok(Digest::default());
        }
        let query = RankQuery {
            text: String::new(),
            store_top: 0,
            all: false,
        };
        let report = self.ranking().rank(&query, now).await?;
        let feedback = self.feedback_by_job().await?;
        // With its position in the ranking (1 = best).
        let strong: Vec<(usize, &Ranking)> = report
            .rankings
            .iter()
            .enumerate()
            .map(|(i, r)| (i + 1, r))
            .filter(|(_, r)| {
                r.tier == Tier::StrongFit
                    && r.gate == Gate::Recommended
                    && !request.exclude.contains(&r.opportunity.to_string())
            })
            .collect();
        let ids: Vec<OpportunityId> = strong.iter().map(|(_, r)| r.opportunity).collect();
        let marks = self.store().feed_marks(&ids).await?;
        let mut records_of = self.store().opportunity_records_many(&ids).await?;
        let mut pool: Vec<(usize, &Ranking, Vec<JobRecord>)> = Vec::new();
        for (rank, ranking) in strong {
            if marks.contains_key(&ranking.opportunity) {
                continue;
            }
            let records = records_of.remove(&ranking.opportunity).unwrap_or_default();
            let state = OpportunityState::of(
                records
                    .iter()
                    .flat_map(|r| feedback.get(&r.id).cloned().unwrap_or_default()),
            );
            if state.stage != Stage::Unseen || records.is_empty() {
                continue;
            }
            pool.push((rank, ranking, records));
        }
        let (picked, omitted) = select_digest(pool, request.size, request.recent_companies, |c| {
            company_key(&c.1.facets.company)
        });
        let mut out = Digest::default();
        for ((rank, ranking, _), why) in &omitted {
            out.pool.push(DigestEntry::of(*rank, ranking, Some(*why)));
        }
        for (rank, ranking, records) in picked {
            out.pool.push(DigestEntry::of(rank, ranking, None));
            let Some(record) = records
                .iter()
                .find(|r| r.id == ranking.job)
                .or_else(|| records.first())
                .cloned()
            else {
                continue;
            };
            let history = self.store().history(record.id).await?;
            let previous = self
                .store()
                .complete_listing_before(&record.posting.provenance.source, record.first_seen_at)
                .await?;
            let freshness = Freshness::classify(&record, &history, previous);
            let opportunity = Opportunity {
                id: ranking.opportunity,
                records,
            };
            let checked = self.check(&opportunity, now).await?;
            out.items.push(NotificationCandidate {
                item: FeedItem {
                    item: ShortlistItem::of(&Entry {
                        ranking: ranking.clone(),
                        checked,
                    }),
                    reason: FeedReason::New,
                    changes: Vec::new(),
                    stage: PipelineStage::Unseen,
                    first_shown_at: None,
                    also_at_company: Vec::new(),
                },
                job: record.id.to_string(),
                content_version: record.posting.content_fingerprint().to_hex(),
                ranking_version: ranking.ranking_version.clone(),
                freshness,
            });
        }
        out.pool.sort_by_key(|e| e.rank);
        Ok(out)
    }
}

/// How a company is compared (a digest holds one job per company).
pub fn company_key(company: &str) -> String {
    jobhunt_core::text::search_key(company)
}

/// The most jobs a digest holds.
pub const MAX_DIGEST_SIZE: usize = 5;
/// A company with a job in a notification sent this many days ago or less
/// gives its digest slot to another company when one can fill it.
pub const DIGEST_COMPANY_SPACING_DAYS: i64 = 7;

/// What a digest is composed from.
#[derive(Debug, Clone, Copy)]
pub struct DigestRequest<'a> {
    /// Opportunities already in one of the person's notifications (`opp_…`):
    /// an opportunity is notified once, ever.
    pub exclude: &'a HashSet<String>,
    /// Jobs in the digest at most (1 to [`MAX_DIGEST_SIZE`]).
    pub size: usize,
    /// Companies ([`company_key`]) with a job in a notification sent in the
    /// last [`DIGEST_COMPANY_SPACING_DAYS`].
    pub recent_companies: &'a HashSet<String>,
}

/// Why a candidate was left out of the digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Omitted {
    /// A better-ranked job at the same company is in it.
    SameCompany,
    /// Its company was in a recent notification, and other companies
    /// filled the digest.
    RecentCompany,
    /// The digest was full.
    Full,
}

/// One candidate of a digest, and what became of it (for logs and dry
/// runs).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DigestEntry {
    /// Position in the ranking (1 = best).
    pub rank: usize,
    pub opportunity: String,
    pub company: String,
    pub title: String,
    /// `None`: in the digest.
    pub omitted: Option<Omitted>,
}

impl DigestEntry {
    fn of(rank: usize, ranking: &Ranking, omitted: Option<Omitted>) -> Self {
        Self {
            rank,
            opportunity: ranking.opportunity.to_string(),
            company: ranking.company.clone(),
            title: ranking.title.clone(),
            omitted,
        }
    }
}

/// A digest: what goes in it (rank order), and every candidate considered.
#[derive(Debug, Clone, Default)]
pub struct Digest {
    pub items: Vec<NotificationCandidate>,
    pub pool: Vec<DigestEntry>,
}

/// A digest's composition, in rank order: each company's best-ranked
/// candidate, at most `size` of them, preferring companies that are not in
/// `recent` (notified about lately) when other companies can fill the
/// slots, never padding. Returns the picks (rank order) and the rest with
/// why. A selection, not a ranking: nothing is re-scored or re-ordered.
pub fn select_digest<T>(
    candidates: Vec<T>,
    size: usize,
    recent: &HashSet<String>,
    company: impl Fn(&T) -> String,
) -> (Vec<T>, Vec<(T, Omitted)>) {
    let mut seen: HashSet<String> = HashSet::new();
    let mut leaders: Vec<(T, bool)> = Vec::new();
    let mut omitted: Vec<(T, Omitted)> = Vec::new();
    for candidate in candidates {
        let key = company(&candidate);
        if seen.insert(key.clone()) {
            leaders.push((candidate, recent.contains(&key)));
        } else {
            omitted.push((candidate, Omitted::SameCompany));
        }
    }
    let mut chosen = vec![false; leaders.len()];
    let mut taken = 0;
    for pass_recent in [false, true] {
        for (i, (_, is_recent)) in leaders.iter().enumerate() {
            if *is_recent == pass_recent && taken < size {
                chosen[i] = true;
                taken += 1;
            }
        }
    }
    let mut picked = Vec::new();
    for ((candidate, is_recent), chosen) in leaders.into_iter().zip(chosen) {
        if chosen {
            picked.push(candidate);
        } else if is_recent {
            omitted.push((candidate, Omitted::RecentCompany));
        } else {
            omitted.push((candidate, Omitted::Full));
        }
    }
    (picked, omitted)
}

/// A recommendation chosen for a notification, with what it rested on.
#[derive(Debug, Clone, PartialEq)]
pub struct NotificationCandidate {
    pub item: FeedItem,
    /// `job_…` of the record the ranking was read from.
    pub job: String,
    /// That record's material fingerprint (its content version).
    pub content_version: String,
    pub ranking_version: String,
    /// How fresh that record is, said honestly.
    pub freshness: Freshness,
}

/// One item on the feed: a shortlist item, with why it is on the feed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FeedItem {
    #[serde(flatten)]
    pub item: ShortlistItem,
    /// `new`, or `changed` (reviewed before, changed materially since).
    pub reason: FeedReason,
    /// What changed, for `changed` items.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<String>,
    /// Where it is in the person's pipeline (`unseen`, `seen`, `saved`).
    pub stage: PipelineStage,
    /// When a feed or shortlist first showed it to the person.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_shown_at: Option<String>,
    /// Other recommendations at the same company, held off the feed so
    /// that one company doesn't fill it (best first).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub also_at_company: Vec<SameCompany>,
}

/// From every open job to what is on the feed. Every number is counted,
/// none estimated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FeedSummary {
    /// Open opportunities checked.
    pub checked: usize,
    /// Left after eligibility, closed listings, pay minimums and what the
    /// person already decided (rejected, applied).
    pub passed_eligibility: usize,
    /// Of those, strong fits and jobs worth reviewing.
    pub worth_reviewing: usize,
    /// Of those, strong fits: what Today chooses from.
    #[serde(default)]
    pub strong_fits: usize,
    /// Of those, new to the person.
    pub new: usize,
    /// Reviewed before and changed materially since.
    pub changed: usize,
    /// On the feed now.
    pub shown: usize,
}

/// Where background discovery stands.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DiscoveryStatus {
    /// `background` (JobHunt Cloud reads job boards on a schedule) or
    /// `on_demand` (searches read them when stale).
    pub mode: String,
    /// When a job board was last read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_read_at: Option<String>,
    /// When the next scheduled read is due (background only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_read_at: Option<String>,
}

/// The answer of `get_feed` (and `GET /api/v1/feed`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FeedView {
    pub generated_at: String,
    /// Best first. Few on purpose; empty when there is nothing new worth
    /// the person's time.
    pub items: Vec<FeedItem>,
    /// Nothing new is worth the person's time right now.
    pub caught_up: bool,
    pub summary: FeedSummary,
    /// Recommendations shown earlier that the person let pass (not shown
    /// again unless they change).
    pub passed_over: usize,
    pub pipeline: PipelineCounts,
    pub discovery: DiscoveryStatus,
    pub refresh: RefreshSummary,
    pub learning: Learning,
    /// Listings verified while preparing the feed.
    pub verified_now: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

impl FeedView {
    pub fn of(feed: &Feed, now: DateTime<Utc>) -> Self {
        let report = &feed.report;
        let items: Vec<FeedItem> = feed
            .entries
            .iter()
            .map(|e| FeedItem {
                item: ShortlistItem::of(&e.entry),
                reason: e.reason,
                changes: e.changes.clone(),
                stage: e.stage.into(),
                first_shown_at: e.first_shown_at.map(time),
                also_at_company: e.also_at_company.clone(),
            })
            .collect();
        let mut notes = Vec::new();
        if report.considered == 0 {
            notes.push(match feed.mode {
                DiscoveryMode::Background => {
                    "Narrow has not read any job board yet; its scheduled discovery will.".into()
                }
                DiscoveryMode::OnDemand => {
                    "No jobs discovered yet: configure sources and refresh.".into()
                }
            });
        }
        if !report.person.has_preferences() {
            notes.push(
                "Say what you want (update_preferences) and give reasons when you reject \
                 jobs: recommendations improve with both."
                    .to_owned(),
            );
        }
        if matches!(feed.refresh.reason, RefreshReason::Disabled) {
            notes.push("Working offline: stored jobs were not refreshed or verified.".into());
        }
        Self {
            generated_at: time(now),
            caught_up: items.is_empty(),
            summary: FeedSummary {
                checked: report.considered,
                passed_eligibility: report.rankings.len(),
                worth_reviewing: report
                    .rankings
                    .iter()
                    .filter(|r| r.tier >= Tier::WorthReviewing)
                    .count(),
                strong_fits: report
                    .rankings
                    .iter()
                    .filter(|r| qualifies_for_today(r))
                    .count(),
                new: feed.new_total,
                changed: feed.changed_total,
                shown: items.len(),
            },
            items,
            passed_over: feed.passed_over,
            pipeline: feed.pipeline,
            discovery: DiscoveryStatus {
                mode: match feed.mode {
                    DiscoveryMode::Background => "background",
                    DiscoveryMode::OnDemand => "on_demand",
                }
                .into(),
                last_read_at: feed.last_read_at.map(time),
                next_read_at: feed.next_read_at.map(time),
            },
            refresh: RefreshSummary::of(&feed.refresh, now),
            learning: Learning {
                feedback: report.taste.events,
                active_patterns: report.taste.active().count(),
                has_preferences: report.person.has_preferences(),
            },
            verified_now: feed.verified_now,
            notes,
        }
    }
}

impl FeedItem {
    /// The coarse tier (for notifications' wording).
    pub fn tier(&self) -> FitTier {
        self.item.tier
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jobhunt_core::{CanonicalUrl, Provenance, SourceKey};
    use jobhunt_jobs::{Compensation, CompensationComponent, CompensationKind, JobPosting};

    fn record(compensation: Option<Compensation>) -> JobRecord {
        let posting = JobPosting {
            provenance: Provenance {
                source: SourceKey::new("greenhouse", "acme").unwrap(),
                source_record_id: Some("1".into()),
                fetched_from: None,
            },
            url: CanonicalUrl::parse("https://job-boards.greenhouse.io/acme/jobs/1").unwrap(),
            apply_url: None,
            company: "Acme".into(),
            title: "Engineer".into(),
            department: None,
            team: None,
            location: None,
            locations: Vec::new(),
            employment_type: None,
            workplace_type: None,
            is_remote: None,
            compensation,
            work_authorization: None,
            description_text: None,
            description_html: None,
            posted_at: None,
            source_updated_at: None,
        };
        let now = Utc::now();
        JobRecord {
            id: posting.id(),
            opportunity_id: OpportunityId::founded_by(posting.id()),
            posting,
            first_seen_at: now,
            last_seen_at: now,
            content_updated_at: now,
            status: jobhunt_jobs::JobStatus::Open,
            closed_at: None,
        }
    }

    fn pay(min: f64, max: f64) -> Compensation {
        Compensation {
            summary: None,
            components: vec![CompensationComponent {
                kind: CompensationKind::Salary,
                label: None,
                currency: Some("USD".into()),
                min: Some(min),
                max: Some(max),
                interval: Some(jobhunt_jobs::PayInterval::Year),
            }],
        }
    }

    #[test]
    fn one_role_per_company_in_rank_order_never_padded() {
        let ranked = [
            ("Supabase", "a"),
            ("Supabase", "b"),
            ("Airbnb", "c"),
            ("supabase", "d"),
            ("Supabase", "e"),
            ("Linear", "f"),
            ("Modal", "g"),
        ];
        let key = |c: &(&str, &str)| jobhunt_core::text::search_key(c.0);
        let picks = one_per_company(ranked.to_vec(), 5, key);
        let chosen: Vec<&str> = picks.iter().map(|(c, _)| c.1).collect();
        assert_eq!(
            chosen,
            ["a", "c", "f", "g"],
            "best of each company, then the rest"
        );
        let held: Vec<&str> = picks[0].1.iter().map(|c| c.1).collect();
        assert_eq!(
            held,
            ["b", "d", "e"],
            "the company's other roles, best first"
        );
        assert!(picks[1..].iter().all(|(_, others)| others.is_empty()));
        // A full feed leaves later companies out; their roles are not
        // attached anywhere.
        let two = one_per_company(ranked.to_vec(), 2, key);
        assert_eq!(two.iter().map(|(c, _)| c.1).collect::<Vec<_>>(), ["a", "c"]);
        assert_eq!(two[0].1.len(), 3);
    }

    fn digest_of(
        ranked: &[(&'static str, &'static str)],
        size: usize,
        recent: &[&str],
    ) -> (Vec<&'static str>, Vec<(&'static str, Omitted)>) {
        let recent: HashSet<String> = recent.iter().map(|c| company_key(c)).collect();
        let (picked, omitted) = select_digest(ranked.to_vec(), size, &recent, |c| company_key(c.0));
        (
            picked.iter().map(|c| c.1).collect(),
            omitted.iter().map(|(c, why)| (c.1, *why)).collect(),
        )
    }

    #[test]
    fn a_digest_is_one_job_per_company_in_rank_order_never_padded() {
        // The brief's example: three Railway roles among the candidates.
        let ranked = [
            ("Railway", "railway-1"),
            ("Railway", "railway-2"),
            ("Ando Technologies", "ando"),
            ("railway", "railway-3"),
            ("Veda", "veda"),
            ("Oneleet", "oneleet"),
        ];
        let (picked, omitted) = digest_of(&ranked, 3, &[]);
        assert_eq!(picked, ["railway-1", "ando", "veda"]);
        assert!(omitted.contains(&("railway-2", Omitted::SameCompany)));
        assert!(omitted.contains(&("railway-3", Omitted::SameCompany)));
        assert!(omitted.contains(&("oneleet", Omitted::Full)));
        // Up to five when the person asks for it; still one per company.
        let (picked, _) = digest_of(&ranked, 5, &[]);
        assert_eq!(picked, ["railway-1", "ando", "veda", "oneleet"]);
        // Nothing qualifies: nothing. One qualifies: one.
        assert!(digest_of(&[], 3, &[]).0.is_empty());
        assert_eq!(digest_of(&ranked[..1], 3, &[]).0, ["railway-1"]);
        // Many jobs at one company: one item.
        assert_eq!(digest_of(&ranked[..2], 3, &[]).0, ["railway-1"]);
    }

    #[test]
    fn recently_notified_companies_give_way_but_are_never_blocked() {
        let ranked = [
            ("Railway", "railway-1"),
            ("Ando Technologies", "ando"),
            ("Veda", "veda"),
            ("Oneleet", "oneleet"),
        ];
        // Railway was in last week's digest; Oneleet can fill its slot.
        let (picked, omitted) = digest_of(&ranked, 3, &["Railway"]);
        assert_eq!(picked, ["ando", "veda", "oneleet"], "still in rank order");
        assert_eq!(omitted, [("railway-1", Omitted::RecentCompany)]);
        // Nobody else can: Railway is sent anyway.
        let (picked, _) = digest_of(&ranked[..2], 3, &["Railway"]);
        assert_eq!(picked, ["railway-1", "ando"]);
        let (picked, _) = digest_of(&ranked, 3, &["Railway", "Ando Technologies", "Veda"]);
        assert_eq!(picked, ["railway-1", "ando", "oneleet"]);
    }

    #[test]
    fn only_material_fields_count_as_changes() {
        let r = record(Some(pay(150_000.0, 190_000.0)));
        let fields = |f: &[&str]| f.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        let cosmetic = describe_change(
            &r,
            JobEventKind::Updated,
            &fields(&["title", "description", "url", "posted_at"]),
            None,
        );
        assert!(cosmetic.is_empty(), "{cosmetic:?}");
        let added = describe_change(&r, JobEventKind::Updated, &fields(&["compensation"]), None);
        assert_eq!(added.len(), 1);
        assert!(added[0].starts_with("Pay is now published"), "{added:?}");
        let before = record(Some(pay(120_000.0, 140_000.0))).posting.snapshot();
        let changed = describe_change(
            &r,
            JobEventKind::Updated,
            &fields(&["compensation"]),
            Some(&before),
        );
        assert!(changed[0].starts_with("Pay changed"), "{changed:?}");
        // Pay withdrawn is not a reason to bring a job back.
        let withdrawn = describe_change(
            &record(None),
            JobEventKind::Updated,
            &fields(&["compensation"]),
            Some(&before),
        );
        assert!(withdrawn.is_empty());
        assert_eq!(
            describe_change(&r, JobEventKind::Reopened, &[], None),
            ["It reopened after being closed"]
        );
        assert_eq!(
            describe_change(&r, JobEventKind::Updated, &fields(&["workplace"]), None).len(),
            1
        );
        assert!(describe_change(&r, JobEventKind::Closed, &[], None).is_empty());
    }
}
