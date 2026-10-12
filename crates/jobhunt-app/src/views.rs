//! The structured answers of the use cases: what the MCP tools return as
//! structured content (and their output schemas), and what `--json` CLI
//! output prints. They are the product's API, so they are explicit,
//! typed and stable: enums instead of free text where the values are
//! known, ids as strings (`opp_…`), times as RFC 3339 strings.
//!
//! Nothing here is computed: every value is read from the domain types
//! the use cases return (rankings, assessments, verification records,
//! feedback state).

use chrono::{DateTime, SecondsFormat, Utc};
use jobhunt_eligibility::{Eligibility, EligibilityDecision};
use jobhunt_jobs::JobRecord;
use jobhunt_jobs::verification::{
    ApplicationStatus, CompensationCheck, CompensationStatus, ListingStatus, OpportunityTrust,
    RecordTrust, Standing, TrustState, VerificationAge, VerificationRecord,
};
use jobhunt_ranking::{Gate, OpportunityState, Ranking, Sentiment, Stage, Tier};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub(crate) fn time(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Every location a posting lists, primary first, without repeats.
pub(crate) fn locations(record: &JobRecord) -> Vec<String> {
    let p = &record.posting;
    let mut out: Vec<String> = p.location.iter().cloned().collect();
    for l in &p.locations {
        if let Some(name) = &l.name
            && !out.contains(name)
        {
            out.push(name.clone());
        }
    }
    out
}

/// The record an opportunity's state rests on: the most trusted one, else
/// the first.
pub(crate) fn best_record<'a>(
    verified: &'a [jobhunt_jobs::verification::RecordVerification],
    trust: &OpportunityTrust,
    fallback: Option<&'a JobRecord>,
) -> Option<&'a JobRecord> {
    trust
        .best
        .and_then(|b| verified.get(b))
        .map(|v| &v.record)
        .or(fallback)
        .or_else(|| verified.first().map(|v| &v.record))
}

/// How worth the person's time a job looks. Deliberately coarse: there is
/// no percentage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FitTier {
    StrongFit,
    WorthReviewing,
    Maybe,
    LowPriority,
}

impl From<Tier> for FitTier {
    fn from(tier: Tier) -> Self {
        match tier {
            Tier::StrongFit => Self::StrongFit,
            Tier::WorthReviewing => Self::WorthReviewing,
            Tier::Maybe => Self::Maybe,
            Tier::LowPriority => Self::LowPriority,
        }
    }
}

/// Whether JobHunt recommends spending time on it now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Recommendation {
    /// Verified recently at an authoritative source, and the person appears
    /// able to take it.
    Recommended,
    /// Looks eligible, but the listing is not verified recently enough:
    /// verify it before spending time on it.
    VerifyFirst,
    /// Nothing rules the person out, but nothing confirms they can take it.
    EligibilityUnclear,
    /// Rejected, already in the pipeline, closed, ineligible or below a
    /// required pay minimum.
    NotRecommended,
}

impl From<&Gate> for Recommendation {
    fn from(gate: &Gate) -> Self {
        match gate {
            Gate::Recommended => Self::Recommended,
            Gate::VerifyFirst { .. } => Self::VerifyFirst,
            Gate::EligibilityUnclear { .. } => Self::EligibilityUnclear,
            Gate::Excluded { .. } => Self::NotRecommended,
        }
    }
}

/// Whether the person appears able to work the job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EligibilityStatus {
    Eligible,
    /// Eligible if a condition holds (relocating, sponsorship offered).
    Conditional,
    Uncertain,
    Ineligible,
    /// There is no profile to check against.
    NotChecked,
}

impl From<Eligibility> for EligibilityStatus {
    fn from(status: Eligibility) -> Self {
        match status {
            Eligibility::Eligible => Self::Eligible,
            Eligibility::Conditional => Self::Conditional,
            Eligibility::Uncertain => Self::Uncertain,
            Eligibility::Ineligible => Self::Ineligible,
        }
    }
}

/// What JobHunt believes about the listing, from verification and
/// discovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VerificationState {
    VerifiedActive,
    VerifiedClosed,
    ClosedByDiscovery,
    CouldNotVerify,
    NotVerified,
}

impl From<TrustState> for VerificationState {
    fn from(state: TrustState) -> Self {
        match state {
            TrustState::VerifiedActive => Self::VerifiedActive,
            TrustState::VerifiedClosed => Self::VerifiedClosed,
            TrustState::ClosedByDiscovery => Self::ClosedByDiscovery,
            TrustState::CouldNotVerify => Self::CouldNotVerify,
            TrustState::NotVerified => Self::NotVerified,
        }
    }
}

/// How recent the last successful verification is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Freshness {
    Fresh,
    Aging,
    Stale,
}

impl From<VerificationAge> for Freshness {
    fn from(age: VerificationAge) -> Self {
        match age {
            VerificationAge::Fresh => Self::Fresh,
            VerificationAge::Aging => Self::Aging,
            VerificationAge::Stale => Self::Stale,
        }
    }
}

/// The listing state one verification attempt found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ListingState {
    Active,
    Closed,
    Unreachable,
    Ambiguous,
    Unknown,
}

impl From<ListingStatus> for ListingState {
    fn from(status: ListingStatus) -> Self {
        match status {
            ListingStatus::Active => Self::Active,
            ListingStatus::Closed => Self::Closed,
            ListingStatus::Unreachable => Self::Unreachable,
            ListingStatus::Ambiguous => Self::Ambiguous,
            ListingStatus::Unknown => Self::Unknown,
        }
    }
}

/// Whether the job can be applied to, as last checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApplicationState {
    Active,
    Closed,
    Unavailable,
    Unknown,
}

impl From<ApplicationStatus> for ApplicationState {
    fn from(status: ApplicationStatus) -> Self {
        match status {
            ApplicationStatus::Active => Self::Active,
            ApplicationStatus::Closed => Self::Closed,
            ApplicationStatus::Unavailable => Self::Unavailable,
            ApplicationStatus::Unknown => Self::Unknown,
        }
    }
}

/// Where the opportunity is in the person's pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PipelineStage {
    Unseen,
    Seen,
    Saved,
    Rejected,
    Applied,
    Interviewing,
    Offer,
}

impl From<Stage> for PipelineStage {
    fn from(stage: Stage) -> Self {
        match stage {
            Stage::Unseen => Self::Unseen,
            Stage::Seen => Self::Seen,
            Stage::Saved => Self::Saved,
            Stage::Rejected => Self::Rejected,
            Stage::Applied => Self::Applied,
            Stage::Interviewing => Self::Interviewing,
            Stage::Offer => Self::Offer,
        }
    }
}

/// Liked or disliked, independent of the pipeline stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SentimentView {
    Liked,
    Disliked,
}

impl From<Sentiment> for SentimentView {
    fn from(s: Sentiment) -> Self {
        match s {
            Sentiment::Liked => Self::Liked,
            Sentiment::Disliked => Self::Disliked,
        }
    }
}

/// Verification at a glance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct VerificationBrief {
    pub state: VerificationState,
    /// Recent enough, from an authoritative source, to recommend on.
    pub trusted: bool,
    /// Why it is not trusted, when it isn't.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_trusted_because: Option<String>,
    /// When the last successful verification happened (RFC 3339).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub freshness: Option<Freshness>,
    /// Who publishes the listing the state rests on
    /// (`employer_configured_ats`, `employer_first_party`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authority: Option<String>,
    /// The source the state rests on (`greenhouse:stripe`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

impl VerificationBrief {
    pub fn of(trust: &OpportunityTrust) -> Self {
        let best = trust.best();
        Self {
            state: trust.state.into(),
            trusted: trust.standing == Standing::Trusted,
            not_trusted_because: match &trust.standing {
                Standing::Trusted => None,
                Standing::NotTrusted(why) => Some(why.clone()),
            },
            verified_at: best
                .and_then(|b| b.last_success.as_ref())
                .map(|v| time(v.attempted_at)),
            freshness: best.and_then(|b| b.age).map(Into::into),
            authority: best.map(|b| b.authority.as_str().to_owned()),
            source: best.map(|b| b.source.to_string()),
        }
    }
}

/// Eligibility at a glance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EligibilityBrief {
    pub status: EligibilityStatus,
    /// One line: why.
    pub headline: String,
}

impl EligibilityBrief {
    pub fn of(decision: Option<&EligibilityDecision>) -> Self {
        match decision {
            Some(d) => Self {
                status: d.status.into(),
                headline: d.headline.clone(),
            },
            None => Self {
                status: EligibilityStatus::NotChecked,
                headline: "No career profile to check against".into(),
            },
        }
    }
}

/// One step of an eligibility decision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EligibilityReason {
    /// The rule (`country_constraint`, `authorization`, `timezone`, …).
    pub rule: String,
    /// `pass`, `conditional`, `unknown`, `fail` or `not_applicable`.
    pub verdict: String,
    pub conclusion: String,
    /// The posting's own words behind it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
}

/// An eligibility decision with its reasons.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EligibilityDetail {
    pub status: EligibilityStatus,
    pub headline: String,
    /// The way of doing the job the status is about ("Remote (Americas)").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option: Option<String>,
    pub reasons: Vec<EligibilityReason>,
    /// Contradictions between statements, kept visible.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conflicts: Vec<String>,
    /// A compatibility signal, not legal advice about work authorization.
    pub disclaimer: String,
}

impl EligibilityDetail {
    pub fn of(d: &EligibilityDecision) -> Self {
        Self {
            status: d.status.into(),
            headline: d.headline.clone(),
            option: d.option.clone(),
            reasons: d
                .reasons
                .iter()
                .map(|r| EligibilityReason {
                    rule: r.rule.as_str().to_owned(),
                    verdict: serde_json::to_value(r.verdict)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_owned))
                        .unwrap_or_default(),
                    conclusion: r.conclusion.clone(),
                    evidence: r
                        .evidence
                        .iter()
                        .map(|e| format!("{} ({}): {}", e.source, e.field, e.text))
                        .collect(),
                })
                .collect(),
            conflicts: d.conflicts.iter().map(|c| c.summary.clone()).collect(),
            disclaimer: "A compatibility signal from the posting and your profile, not legal \
                         advice about work authorization."
                .into(),
        }
    }
}

/// The decision brief: what someone needs to decide whether a job is
/// worth their time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DecisionView {
    pub tier: FitTier,
    pub recommendation: Recommendation,
    /// Why it is not recommended, or what to check first, when it applies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommendation_note: Option<String>,
    /// One sentence.
    pub verdict: String,
    /// Role · level · stack · domain · pay · where.
    pub summary: String,
    /// Why it may be worth the person's time, strongest first.
    pub worth: Vec<String>,
    /// What counts against it, and conditions.
    pub caveats: Vec<String>,
    /// What the posting doesn't say that matters to the person: the
    /// things to check.
    pub unknowns: Vec<String>,
    /// The person's history with it.
    pub history: Vec<String>,
    /// How well it fits what they want: `strong`, `plausible`,
    /// `insufficient` or `poor`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fit: Option<String>,
    /// What goes against the fit, most material first (why a job that
    /// looks relevant is held back).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub against: Vec<String>,
    /// Useful practical facts (pay reaching the person's target). Never
    /// reasons for fit.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub facts: Vec<String>,
}

impl DecisionView {
    pub fn of(r: &Ranking) -> Self {
        Self {
            fit: Some(r.fit.level.as_str().to_owned()),
            against: r
                .fit
                .contradictions
                .iter()
                .map(|c| c.text.clone())
                .collect(),
            facts: r.practicality.facts.clone(),
            tier: r.tier.into(),
            recommendation: (&r.gate).into(),
            recommendation_note: gate_note(&r.gate),
            verdict: r.brief.verdict.clone(),
            summary: r.brief.summary.clone(),
            worth: r.brief.worth.clone(),
            caveats: r.brief.caveats.clone(),
            unknowns: r.brief.unknowns.clone(),
            history: r.brief.history.clone(),
        }
    }
}

pub(crate) fn gate_note(gate: &Gate) -> Option<String> {
    match gate {
        Gate::Recommended => None,
        Gate::VerifyFirst { why } | Gate::EligibilityUnclear { why } => Some(why.clone()),
        Gate::Excluded { exclusion } => Some(exclusion.label()),
    }
}

/// A past action, verbatim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FeedbackView {
    /// `fb_…`.
    pub id: String,
    /// `save`, `reject`, `applied`, `like`, …
    pub action: String,
    /// The person's words, exactly as given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub at: String,
}

/// The person's state on an opportunity, folded from their feedback.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PipelineStateView {
    pub stage: PipelineStage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sentiment: Option<SentimentView>,
    /// The furthest application stage reached (kept after withdrawing).
    pub furthest: PipelineStage,
    /// When the current stage was reached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    /// Every action other than looking at it, oldest first.
    pub feedback: Vec<FeedbackView>,
}

impl PipelineStateView {
    pub fn of(state: &OpportunityState) -> Self {
        Self {
            stage: state.stage.into(),
            sentiment: state.sentiment.map(Into::into),
            furthest: state.furthest.into(),
            since: state.since().map(time),
            feedback: state
                .events
                .iter()
                .filter(|e| e.action != jobhunt_ranking::FeedbackAction::Seen)
                .map(|e| FeedbackView {
                    id: e.id.to_string(),
                    action: e.action.as_str().to_owned(),
                    reason: e.reason.clone(),
                    at: time(e.at),
                })
                .collect(),
        }
    }
}

/// Published pay, as facts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CompensationView {
    /// `published`, `not_published` or `not_observed`.
    pub status: String,
    /// Each published range in words ("USD 140,000 – 180,000 per year").
    /// A range whose currency is only an ambiguous symbol says so; it is
    /// never read as a low number.
    pub ranges: Vec<String>,
    /// The source's own summary text, verbatim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Whether the facts come from a verification (`true`) or from
    /// discovery (`false`).
    pub verified: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_at: Option<String>,
}

impl CompensationView {
    pub fn of(check: &CompensationCheck, verified_at: Option<DateTime<Utc>>) -> Self {
        Self {
            status: match check.status {
                CompensationStatus::Published => "published",
                CompensationStatus::NotPublished => "not_published",
                CompensationStatus::NotObserved => "not_observed",
            }
            .into(),
            ranges: check.ranges.iter().map(|r| r.describe()).collect(),
            summary: check.summary.clone(),
            verified: verified_at.is_some(),
            verified_at: verified_at.map(time),
        }
    }

    /// Verified facts when there are any, else what discovery stored.
    pub fn best(record: &JobRecord, trust: &OpportunityTrust) -> Self {
        let (check, verified_at) = Self::best_check(record, trust);
        Self::of(&check, verified_at)
    }

    /// What [`CompensationView::best`] shows, as data: the check, and when
    /// it was verified (`None`: as discovered).
    pub fn best_check(
        record: &JobRecord,
        trust: &OpportunityTrust,
    ) -> (CompensationCheck, Option<DateTime<Utc>>) {
        let verified = trust
            .best()
            .and_then(|b| b.last_success.as_ref())
            .filter(|v| v.compensation.status != CompensationStatus::NotObserved);
        match verified {
            Some(v) => (v.compensation.clone(), Some(v.attempted_at)),
            None => (
                CompensationCheck::observe(record.posting.compensation.as_ref(), None),
                None,
            ),
        }
    }
}

/// One verification attempt of one source record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AttemptView {
    pub at: String,
    pub listing: ListingState,
    pub application: ApplicationState,
    /// `probed`, `published`, `listing_page` or `not_checked`.
    pub application_basis: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub application_url: Option<String>,
    pub authority: String,
    /// How it was checked (`greenhouse_job_api`, `ashby_board_api`, …).
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked_url: Option<String>,
    /// Why it could not reach an answer, when it couldn't.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
    /// What this attempt could not establish.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unknowns: Vec<String>,
}

impl AttemptView {
    pub fn of(v: &VerificationRecord) -> Self {
        Self {
            at: time(v.attempted_at),
            listing: v.listing.into(),
            application: v.application.status.into(),
            application_basis: v.application.basis.as_str().to_owned(),
            application_url: v.application.url.clone(),
            authority: v.authority.as_str().to_owned(),
            method: v.method.as_str().to_owned(),
            checked_url: v.checked_url.clone(),
            failure: v.failure.as_ref().map(|f| f.detail.clone()),
            unknowns: v.unknowns.clone(),
        }
    }
}

/// One source record of an opportunity (provenance).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SourceRecordView {
    /// `job_…`: this source's record.
    pub job_id: String,
    /// `greenhouse:stripe`.
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_record_id: Option<String>,
    pub url: String,
    /// `open` or `closed` (as discovery last saw it).
    pub status: String,
    pub first_seen_at: String,
    pub last_seen_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<String>,
    pub verification: VerificationState,
    pub authority: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_attempt: Option<AttemptView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_success_at: Option<String>,
}

impl SourceRecordView {
    pub fn of(record: &JobRecord, trust: &RecordTrust) -> Self {
        Self {
            job_id: record.id.to_string(),
            source: record.posting.provenance.source.to_string(),
            source_record_id: record.posting.provenance.source_record_id.clone(),
            url: record.posting.url.to_string(),
            status: record.status.as_str().to_owned(),
            first_seen_at: time(record.first_seen_at),
            last_seen_at: time(record.last_seen_at),
            closed_at: record.closed_at.map(time),
            verification: trust.state.into(),
            authority: trust.authority.as_str().to_owned(),
            last_attempt: trust.latest.as_ref().map(AttemptView::of),
            last_success_at: trust.last_success.as_ref().map(|v| time(v.attempted_at)),
        }
    }
}
