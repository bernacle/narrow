//! The source registry: what JobHunt knows about each job board beyond
//! "it is configured".
//!
//! The configuration (`[[sources.*]]`) stays the one list discovery reads.
//! The registry sits next to it as a static file of entries
//! ([`SourceRegistry`], e.g. `deploy/sources.toml`): which company a board
//! belongs to, its careers page, where it came from, and its place in a
//! small lifecycle ([`SourceStatus`]):
//!
//! ```text
//! candidate ──validate──▶ validated ──a person adds it to the config──▶ active
//!     │                                                                 │  ▲
//!     └──▶ rejected                              repeated failures ──▶ unhealthy
//!                                                 a person removes it ──▶ disabled
//! ```
//!
//! Everything that changes with each scan (last scan, last success, open
//! jobs, the last error) is derived from the database when a report is
//! made, never written here, so the file only changes when a decision
//! does. The rules are plain functions: [`validate`] (candidate →
//! validated or rejected), [`activation`] (is a validated board worth
//! reading in production), [`health`] (from recent scans) and
//! [`next_status`].

use chrono::{DateTime, Utc};
use jobhunt_core::SourceKey;
use serde::{Deserialize, Serialize};

use crate::company::{BoardCheck, BoardEvidence};

/// Where a source stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceStatus {
    /// Found (by discovery or by hand), not yet checked.
    Candidate,
    /// Passed [`validate`]; not read in production.
    Validated,
    /// In the production configuration.
    Active,
    /// Active, but its recent scans fail ([`Health::Unhealthy`]).
    Unhealthy,
    /// Removed from the configuration by a person; kept for the record.
    Disabled,
    /// Failed [`validate`]; kept as research data with its reasons.
    Rejected,
}

impl SourceStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Candidate => "candidate",
            Self::Validated => "validated",
            Self::Active => "active",
            Self::Unhealthy => "unhealthy",
            Self::Disabled => "disabled",
            Self::Rejected => "rejected",
        }
    }

    /// Statuses whose source is in the production configuration.
    pub fn configured(self) -> bool {
        matches!(self, Self::Active | Self::Unhealthy)
    }
}

/// One source in the registry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistryEntry {
    /// Canonical source id, `<provider>:<board>` (`ashby:railway`).
    pub source: SourceKey,
    pub company: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub careers_url: Option<String>,
    pub status: SourceStatus,
    /// Since when (a date, `YYYY-MM-DD`).
    pub since: String,
    /// Where it came from: an experiment, a careers page, a person.
    pub provenance: String,
    /// Why it has its status, one short line each.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

impl RegistryEntry {
    /// The ATS (`ashby`, `greenhouse`, …).
    pub fn provider(&self) -> &str {
        self.source.kind()
    }

    /// The board, site or company slug on the ATS.
    pub fn board(&self) -> &str {
        self.source.instance()
    }
}

/// The registry file: `[[sources]]` entries.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceRegistry {
    #[serde(default)]
    pub sources: Vec<RegistryEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegistryError {
    #[error("invalid source registry: {0}")]
    Parse(String),
    #[error("source {0} is in the registry more than once")]
    Duplicate(SourceKey),
}

impl SourceRegistry {
    pub fn parse(text: &str) -> Result<Self, RegistryError> {
        let registry: Self =
            toml::from_str(text).map_err(|e| RegistryError::Parse(e.to_string()))?;
        let mut seen = std::collections::HashSet::new();
        for entry in &registry.sources {
            if !seen.insert(&entry.source) {
                return Err(RegistryError::Duplicate(entry.source.clone()));
            }
        }
        Ok(registry)
    }

    pub fn to_toml(&self) -> String {
        toml::to_string(self).unwrap_or_default()
    }

    pub fn get(&self, source: &SourceKey) -> Option<&RegistryEntry> {
        self.sources.iter().find(|e| &e.source == source)
    }

    /// Sources a production configuration should read.
    pub fn configured(&self) -> impl Iterator<Item = &RegistryEntry> {
        self.sources.iter().filter(|e| e.status.configured())
    }

    /// Adds an entry, or replaces the one for the same source unless that
    /// one records a person's decision (active, unhealthy, disabled).
    /// Returns whether the registry changed.
    pub fn upsert(&mut self, entry: RegistryEntry) -> bool {
        match self.sources.iter_mut().find(|e| e.source == entry.source) {
            Some(existing)
                if matches!(
                    existing.status,
                    SourceStatus::Active | SourceStatus::Unhealthy | SourceStatus::Disabled
                ) =>
            {
                false
            }
            Some(existing) if *existing == entry => false,
            Some(existing) => {
                *existing = entry;
                true
            }
            None => {
                self.sources.push(entry);
                true
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Freshness.

/// How old a source's postings are, by the source's own publish date.
///
/// `first_seen_at` is when JobHunt first read a posting, not when it was
/// posted: a 2022 posting read for the first time today is not new. A
/// posting without a publish date is counted as unknown, never as fresh.
/// The day counts are cumulative (`under_30_days` includes
/// `under_7_days`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Freshness {
    pub total: usize,
    pub under_7_days: usize,
    pub under_30_days: usize,
    pub over_90_days: usize,
    pub over_180_days: usize,
    pub over_365_days: usize,
    /// No publish date from the source.
    pub unknown: usize,
}

impl Freshness {
    pub fn of(posted: impl IntoIterator<Item = Option<DateTime<Utc>>>, now: DateTime<Utc>) -> Self {
        let mut f = Self::default();
        for date in posted {
            f.add(date, now);
        }
        f
    }

    pub fn add(&mut self, posted: Option<DateTime<Utc>>, now: DateTime<Utc>) {
        self.total += 1;
        let Some(posted) = posted else {
            self.unknown += 1;
            return;
        };
        let days = (now - posted).num_days();
        self.under_7_days += usize::from(days < 7);
        self.under_30_days += usize::from(days < 30);
        self.over_90_days += usize::from(days > 90);
        self.over_180_days += usize::from(days > 180);
        self.over_365_days += usize::from(days > 365);
    }

    /// Postings with a publish date.
    pub fn dated(&self) -> usize {
        self.total - self.unknown
    }

    /// Share of dated postings older than a year (`None` when none is
    /// dated).
    pub fn stale_share(&self) -> Option<f64> {
        let dated = self.dated();
        (dated > 0).then(|| self.over_365_days as f64 / dated as f64)
    }
}

// ---------------------------------------------------------------------------
// Yield.

/// Where a posting's remote work can be done, coarsely, from the
/// posting's own location data (not from the company's reputation).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GeoBucket {
    /// Remote anywhere.
    Global,
    /// Remote with Brazil named (alone or in a list).
    Brazil,
    /// Remote in Latin America or the Americas.
    Americas,
    /// Remote only in the US / North America.
    NorthAmerica,
    /// Remote only in Europe (EU, EMEA without Brazil, single countries).
    Europe,
    /// Remote in other or mixed places.
    Other,
    /// "Remote" with no place.
    UnknownScope,
    /// No remote option (on-site or hybrid).
    Office,
}

impl GeoBucket {
    pub const ALL: [Self; 8] = [
        Self::Global,
        Self::Brazil,
        Self::Americas,
        Self::NorthAmerica,
        Self::Europe,
        Self::Other,
        Self::UnknownScope,
        Self::Office,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Brazil => "brazil",
            Self::Americas => "americas",
            Self::NorthAmerica => "north_america",
            Self::Europe => "europe",
            Self::Other => "other",
            Self::UnknownScope => "unknown_scope",
            Self::Office => "office",
        }
    }
}

/// What a source yields, as transparent counts (no single score).
///
/// The reference-profile counts read each posting's eligibility for a
/// person living in Brazil who works remotely only and does not relocate:
/// Narrow's core audience, without anyone's private profile.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SourceYield {
    pub open: usize,
    /// Engineering by title (Narrow's job-function reading).
    pub engineering: usize,
    /// Reference profile: eligible or conditional.
    pub brazil_open: usize,
    /// Reference profile: uncertain (nothing rules Brazil out or in).
    pub brazil_unclear: usize,
    /// Engineering and eligible or conditional for the reference profile.
    pub engineering_brazil_open: usize,
    pub engineering_brazil_unclear: usize,
    /// Postings per [`GeoBucket`], in [`GeoBucket::ALL`] order.
    pub geo: Vec<(GeoBucket, usize)>,
    pub freshness: Freshness,
    /// A person's own ranking, when one was run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person: Option<PersonYield>,
}

/// A source's yield for one person's profile and ranking.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersonYield {
    /// Not excluded by eligibility or the person's requirements.
    pub actionable: usize,
    pub plausible: usize,
    /// Strong fits: Today's candidates (Today shows the best of them, a
    /// few companies at a time).
    pub strong: usize,
}

impl SourceYield {
    pub fn geo_count(&self, bucket: GeoBucket) -> usize {
        self.geo
            .iter()
            .find(|(b, _)| *b == bucket)
            .map_or(0, |(_, n)| *n)
    }

    /// The bucket most postings fall in (ties: the first in order).
    pub fn main_geo(&self) -> Option<GeoBucket> {
        self.geo
            .iter()
            .filter(|(_, n)| *n > 0)
            .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
            .map(|(b, _)| *b)
    }
}

// ---------------------------------------------------------------------------
// Validation and activation.

/// The outcome of [`validate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Validated,
    Rejected,
    /// Nothing conclusive (a transient failure, or a board nothing ties
    /// to the company): stays a candidate for a person to confirm.
    Inconclusive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Validation {
    pub verdict: Verdict,
    /// Every rule's finding, in order.
    pub reasons: Vec<String>,
}

/// Postings older than a year above this share make a board stale-only.
pub const STALE_ONLY_SHARE: f64 = 1.0;
/// Below this share of postings with provider ids, identity is unstable.
pub const STABLE_ID_SHARE: f64 = 0.9;

/// Candidate → validated, rejected or still a candidate:
///
/// 1. the adapter reads the board (a transient failure is inconclusive);
/// 2. it lists at least one posting;
/// 3. the board is the company's: the company's own site points at a board
///    whose slug matches the company; otherwise (a guessed slug, or a
///    linked board named after someone else: a parent company, a portfolio
///    company) something ties the board to the company's domain: the
///    website the board names, or a posting on or mentioning the domain.
///    The company's name in postings is not enough ("Ghost" is several
///    companies). Without a tie the board stays a candidate: nothing shows
///    it is wrong either, and a person can confirm it;
/// 4. postings have stable identities (the provider's own ids);
/// 5. not every dated posting is over a year old.
pub fn validate(check: &BoardCheck) -> Validation {
    let mut reasons = Vec::new();
    let reject = |mut reasons: Vec<String>, why: String| {
        reasons.push(why);
        Validation {
            verdict: Verdict::Rejected,
            reasons,
        }
    };
    if let Some(error) = &check.error {
        if check.transient {
            return Validation {
                verdict: Verdict::Inconclusive,
                reasons: vec![format!("transient failure: {error}")],
            };
        }
        return reject(reasons, format!("adapter failed: {error}"));
    }
    if check.jobs == 0 {
        return reject(reasons, "lists no postings".into());
    }
    reasons.push(format!(
        "adapter read {} posting{}",
        check.jobs,
        if check.jobs == 1 { "" } else { "s" }
    ));
    let tie = match (check.website_matches, check.naming_domain) {
        (Some(true), _) => Some(format!(
            "the board names the company's website ({})",
            check.website.as_deref().unwrap_or_default()
        )),
        (_, n) if n > 0 => Some(format!(
            "{n} of {} postings are on or mention the company's domain",
            check.jobs
        )),
        _ => None,
    };
    let site = match check.evidence {
        BoardEvidence::Redirect => "redirect",
        BoardEvidence::CareersPage => "careers page",
        BoardEvidence::Homepage => "homepage",
        BoardEvidence::SlugGuess | BoardEvidence::Discovered => "",
    };
    let how = if check.evidence == BoardEvidence::Discovered {
        "found by broad discovery"
    } else {
        "guessed slug"
    };
    match (check.evidence.first_party(), check.matches_company, tie) {
        (true, true, _) => reasons.push(format!("the company's site points at it ({site})")),
        (true, false, Some(tie)) => reasons.push(format!(
            "the company's site points at it ({site}); the slug differs from the company's name, but {tie}"
        )),
        (true, false, None) => {
            reasons.push(
                "the company's site points at it, but the slug doesn't match the company and \
                 nothing ties it to the company's domain (another company's board?)"
                    .into(),
            );
            return Validation {
                verdict: Verdict::Inconclusive,
                reasons,
            };
        }
        (false, _, Some(tie)) => reasons.push(format!("{how}; {tie}")),
        (false, _, None) => {
            reasons.push(format!(
                "{how}; nothing ties the board to the company's domain ({} of {} \
                 postings name the company, which is not enough)",
                check.naming_company, check.jobs
            ));
            return Validation {
                verdict: Verdict::Inconclusive,
                reasons,
            };
        }
    }
    if (check.with_provider_id as f64 / check.jobs as f64) < STABLE_ID_SHARE {
        return reject(
            reasons,
            format!(
                "only {} of {} postings have a provider id",
                check.with_provider_id, check.jobs
            ),
        );
    }
    if check
        .freshness
        .stale_share()
        .is_some_and(|s| s >= STALE_ONLY_SHARE)
    {
        return reject(reasons, "every dated posting is over a year old".into());
    }
    Validation {
        verdict: Verdict::Validated,
        reasons,
    }
}

/// Validated → worth activating? Each unmet condition is returned; empty
/// means ready. Activation stays a person's decision (a configuration
/// change); this only says whether the evidence supports it:
///
/// * at least one engineering posting that the reference profile (Brazil,
///   remote) can take or that doesn't rule Brazil out;
/// * at least one posting that is eligible or conditional for the
///   reference profile, or whose published remote scope includes Brazil
///   (global, Brazil, the Americas): a board whose only hope is "Remote"
///   with no place, or vague time-zone wording, is not enough;
/// * fewer than three quarters of dated postings over a year old (an
///   evergreen posting is not a reason to drop a board by itself).
pub fn activation(y: &SourceYield) -> Vec<String> {
    let mut unmet = Vec::new();
    if y.engineering_brazil_open + y.engineering_brazil_unclear == 0 {
        unmet.push(format!(
            "no engineering posting open to Brazil ({} engineering of {} open)",
            y.engineering, y.open
        ));
    }
    let scoped = y.geo_count(GeoBucket::Global)
        + y.geo_count(GeoBucket::Brazil)
        + y.geo_count(GeoBucket::Americas);
    if y.brazil_open + scoped == 0 {
        unmet.push("no posting open to Brazil or with a remote scope that includes it".into());
    }
    if let Some(share) = y.freshness.stale_share()
        && share >= 0.75
    {
        unmet.push(format!(
            "{:.0}% of dated postings are over a year old",
            share * 100.0
        ));
    }
    unmet
}

// ---------------------------------------------------------------------------
// Health.

/// One scan of a source, as stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScanSummary {
    pub finished_at: DateTime<Utc>,
    /// `listing`, `not_modified` or `failed`.
    pub status: String,
    pub received: usize,
    pub error: Option<String>,
}

impl ScanSummary {
    pub fn succeeded(&self) -> bool {
        self.status != "failed"
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Health {
    /// The latest scan succeeded.
    Healthy,
    /// A transient-looking failure, or an unusual empty listing.
    Degraded,
    /// Repeated failures, or the board is gone.
    Unhealthy,
    /// Never scanned.
    Unknown,
}

impl Health {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Degraded => "degraded",
            Self::Unhealthy => "unhealthy",
            Self::Unknown => "unknown",
        }
    }
}

/// Consecutive failures that make a source unhealthy.
pub const UNHEALTHY_AFTER: usize = 3;

/// A source's health from its scans, newest first, with why. One failure
/// is never enough to call a source unhealthy, unless it is a second
/// "not found" in a row (the board moved or closed).
pub fn health(scans: &[ScanSummary]) -> (Health, String) {
    let Some(latest) = scans.first() else {
        return (Health::Unknown, "never scanned".into());
    };
    if latest.succeeded() {
        let previous_listing = scans
            .iter()
            .skip(1)
            .find(|s| s.status == "listing")
            .map(|s| s.received);
        return match previous_listing {
            Some(before) if latest.status == "listing" && latest.received == 0 && before > 0 => (
                Health::Degraded,
                format!("empty listing after {before} postings"),
            ),
            _ => (Health::Healthy, "latest scan succeeded".into()),
        };
    }
    let failures = scans.iter().take_while(|s| !s.succeeded()).count();
    let not_found = scans
        .iter()
        .take_while(|s| !s.succeeded())
        .filter(|s| {
            s.error
                .as_deref()
                .is_some_and(|e| e.contains("404") || e.contains("not found"))
        })
        .count();
    let error = latest.error.clone().unwrap_or_default();
    if not_found >= 2 {
        (
            Health::Unhealthy,
            format!("not found {not_found} times in a row: {error}"),
        )
    } else if failures >= UNHEALTHY_AFTER {
        (
            Health::Unhealthy,
            format!("{failures} failures in a row: {error}"),
        )
    } else {
        (
            Health::Degraded,
            format!("{failures} failure(s) in a row: {error}"),
        )
    }
}

/// The status a source moves to, given its health and (for a candidate)
/// its validation. Transitions a person must decide (validated → active,
/// anything → disabled) are not made here.
pub fn next_status(
    current: SourceStatus,
    health: Health,
    validation: Option<&Validation>,
) -> SourceStatus {
    match (current, health) {
        (SourceStatus::Candidate, _) => match validation.map(|v| v.verdict) {
            Some(Verdict::Validated) => SourceStatus::Validated,
            Some(Verdict::Rejected) => SourceStatus::Rejected,
            Some(Verdict::Inconclusive) | None => SourceStatus::Candidate,
        },
        (SourceStatus::Active, Health::Unhealthy) => SourceStatus::Unhealthy,
        (SourceStatus::Unhealthy, Health::Healthy) => SourceStatus::Active,
        (status, _) => status,
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, TimeZone};

    use super::*;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 5, 0, 0, 0).unwrap()
    }

    fn check(evidence: BoardEvidence) -> BoardCheck {
        BoardCheck {
            source: "ashby:acme".into(),
            evidence,
            error: None,
            transient: false,
            jobs: 10,
            rejected: 0,
            complete: true,
            with_provider_id: 10,
            matches_company: true,
            naming_company: 2,
            naming_domain: 0,
            website: None,
            website_matches: None,
            company_names: vec!["acme".into()],
            freshness: Freshness::of((0..10).map(|d| Some(now() - Duration::days(d * 20))), now()),
            postings: Vec::new(),
        }
    }

    fn scan(status: &str, received: usize, error: Option<&str>) -> ScanSummary {
        ScanSummary {
            finished_at: now(),
            status: status.into(),
            received,
            error: error.map(str::to_owned),
        }
    }

    #[test]
    fn freshness_never_treats_unknown_dates_as_fresh() {
        let f = Freshness::of(
            [
                Some(now() - Duration::days(2)),
                Some(now() - Duration::days(40)),
                Some(now() - Duration::days(200)),
                Some(now() - Duration::days(1000)),
                None,
            ],
            now(),
        );
        assert_eq!(
            (f.total, f.under_7_days, f.under_30_days, f.unknown),
            (5, 1, 1, 1)
        );
        assert_eq!(
            (f.over_90_days, f.over_180_days, f.over_365_days),
            (2, 2, 1)
        );
        assert_eq!(f.stale_share(), Some(0.25));
        assert_eq!(Freshness::of([None], now()).stale_share(), None);
    }

    #[test]
    fn first_party_boards_validate() {
        let v = validate(&check(BoardEvidence::CareersPage));
        assert_eq!(v.verdict, Verdict::Validated, "{:?}", v.reasons);
        assert!(v.reasons.iter().any(|r| r.contains("careers page")));
    }

    #[test]
    fn guessed_boards_need_a_tie_to_the_company_domain() {
        // The name in every posting is not enough…
        let mut named = check(BoardEvidence::SlugGuess);
        named.naming_company = 10;
        let v = validate(&named);
        assert_eq!(v.verdict, Verdict::Inconclusive);
        assert!(v.reasons.last().unwrap().contains("10 of 10"), "{v:?}");
        // …a posting mentioning the domain is…
        let mut tied = named.clone();
        tied.naming_domain = 1;
        assert_eq!(validate(&tied).verdict, Verdict::Validated);
        // …and so is the board naming the company's website.
        let mut site = named.clone();
        site.website = Some("https://acme.app/".into());
        site.website_matches = Some(true);
        let v = validate(&site);
        assert_eq!(v.verdict, Verdict::Validated);
        assert!(v.reasons.last().unwrap().contains("acme.app"));
        site.website_matches = Some(false);
        assert_eq!(validate(&site).verdict, Verdict::Inconclusive);
    }

    #[test]
    fn a_linked_board_named_after_another_company_needs_a_tie_too() {
        let mut other = check(BoardEvidence::CareersPage);
        other.matches_company = false;
        let v = validate(&other);
        assert_eq!(v.verdict, Verdict::Inconclusive);
        assert!(v.reasons.last().unwrap().contains("another company"));
        other.naming_domain = 3;
        assert_eq!(validate(&other).verdict, Verdict::Validated);
    }

    #[test]
    fn validation_rejects_empty_broken_unstable_and_stale_boards() {
        let mut empty = check(BoardEvidence::Homepage);
        empty.jobs = 0;
        assert_eq!(validate(&empty).verdict, Verdict::Rejected);

        let mut gone = check(BoardEvidence::Homepage);
        gone.error = Some("board was not found (HTTP 404)".into());
        assert_eq!(validate(&gone).verdict, Verdict::Rejected);

        let mut flaky = gone.clone();
        flaky.transient = true;
        assert_eq!(validate(&flaky).verdict, Verdict::Inconclusive);

        let mut unstable = check(BoardEvidence::Homepage);
        unstable.with_provider_id = 5;
        assert_eq!(validate(&unstable).verdict, Verdict::Rejected);

        let mut stale = check(BoardEvidence::Homepage);
        stale.freshness = Freshness::of(
            [
                Some(now() - Duration::days(800)),
                None,
                Some(now() - Duration::days(400)),
            ],
            now(),
        );
        let v = validate(&stale);
        assert_eq!(v.verdict, Verdict::Rejected);
        assert!(v.reasons.last().unwrap().contains("over a year"));
    }

    #[test]
    fn activation_wants_brazil_open_engineering() {
        let mut y = SourceYield {
            open: 20,
            engineering: 5,
            ..SourceYield::default()
        };
        let unmet = activation(&y);
        assert_eq!(unmet.len(), 2, "{unmet:?}");
        assert!(unmet[0].contains("5 engineering of 20 open"));
        y.engineering_brazil_unclear = 1;
        // "Remote" with no place is not enough…
        y.geo = vec![(GeoBucket::UnknownScope, 20)];
        assert_eq!(activation(&y).len(), 1);
        // …a global scope is, even when eligibility stays unclear (vague
        // time-zone wording)…
        y.geo = vec![(GeoBucket::Global, 1), (GeoBucket::UnknownScope, 19)];
        assert!(activation(&y).is_empty());
        // …and so is a posting confirmed open to Brazil.
        y.geo = vec![(GeoBucket::UnknownScope, 20)];
        y.brazil_open = 3;
        assert!(activation(&y).is_empty());
        y.freshness = Freshness::of([Some(now() - Duration::days(500))], now());
        assert_eq!(activation(&y).len(), 1);
    }

    #[test]
    fn health_from_recent_scans() {
        assert_eq!(health(&[]).0, Health::Unknown);
        assert_eq!(health(&[scan("listing", 8, None)]).0, Health::Healthy);
        assert_eq!(
            health(&[scan("not_modified", 0, None), scan("listing", 8, None)]).0,
            Health::Healthy
        );
        let (h, why) = health(&[scan("listing", 0, None), scan("listing", 8, None)]);
        assert_eq!(h, Health::Degraded);
        assert_eq!(why, "empty listing after 8 postings");
        // One failure is never enough.
        let timeout = scan("failed", 0, Some("request timed out"));
        let ok = scan("listing", 8, None);
        assert_eq!(health(&[timeout.clone(), ok.clone()]).0, Health::Degraded);
        assert_eq!(
            health(&[timeout.clone(), timeout.clone(), timeout.clone(), ok]).0,
            Health::Unhealthy
        );
        let gone = scan("failed", 0, Some("board was not found (HTTP 404)"));
        assert_eq!(health(std::slice::from_ref(&gone)).0, Health::Degraded);
        assert_eq!(health(&[gone.clone(), gone]).0, Health::Unhealthy);
    }

    #[test]
    fn lifecycle_transitions() {
        use SourceStatus::*;
        let validated = Validation {
            verdict: Verdict::Validated,
            reasons: vec![],
        };
        let rejected = Validation {
            verdict: Verdict::Rejected,
            reasons: vec![],
        };
        assert_eq!(
            next_status(Candidate, Health::Unknown, Some(&validated)),
            Validated
        );
        assert_eq!(
            next_status(Candidate, Health::Unknown, Some(&rejected)),
            Rejected
        );
        assert_eq!(next_status(Candidate, Health::Unknown, None), Candidate);
        // Activation is a person's decision.
        assert_eq!(next_status(Validated, Health::Healthy, None), Validated);
        assert_eq!(next_status(Active, Health::Degraded, None), Active);
        assert_eq!(next_status(Active, Health::Unhealthy, None), Unhealthy);
        assert_eq!(next_status(Unhealthy, Health::Degraded, None), Unhealthy);
        assert_eq!(next_status(Unhealthy, Health::Healthy, None), Active);
        assert_eq!(next_status(Disabled, Health::Healthy, None), Disabled);
        assert_eq!(
            next_status(Rejected, Health::Healthy, Some(&validated)),
            Rejected
        );
    }

    #[test]
    fn registry_round_trips_and_keeps_decisions() {
        let text = r#"
            [[sources]]
            source = "ashby:railway"
            company = "Railway"
            domain = "railway.com"
            status = "active"
            since = "2026-10-05"
            provenance = "BRU-325 board set"
            reasons = ["3 practical strong yes in BRU-325"]
        "#;
        let mut registry = SourceRegistry::parse(text).unwrap();
        let entry = registry.sources[0].clone();
        assert_eq!((entry.provider(), entry.board()), ("ashby", "railway"));
        assert_eq!(registry.configured().count(), 1);
        assert_eq!(
            SourceRegistry::parse(&registry.to_toml()).unwrap(),
            registry
        );

        // A discovery run doesn't overwrite a person's decision…
        let mut found = entry.clone();
        found.status = SourceStatus::Validated;
        assert!(!registry.upsert(found.clone()));
        // …but adds and updates what discovery owns.
        found.source = "greenhouse:acme".parse().unwrap();
        assert!(registry.upsert(found.clone()));
        found.status = SourceStatus::Rejected;
        assert!(registry.upsert(found));
        assert_eq!(registry.sources.len(), 2);

        let twice = format!("{text}\n{text}");
        assert!(matches!(
            SourceRegistry::parse(&twice),
            Err(RegistryError::Duplicate(_))
        ));
        assert!(SourceRegistry::parse("[[sources]]\nsource = \"ashby:x\"\nbogus = 1").is_err());
    }
}
