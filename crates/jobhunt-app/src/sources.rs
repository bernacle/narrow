//! Source quality: what each job board yields, its health, and finding
//! the boards behind companies' careers pages.
//!
//! A board with 500 postings is not better than one with 12: what matters
//! is how many of them someone can actually take. Every number here is a
//! count (see [`SourceYield`]); there is no combined score.
//!
//! * [`yield_of`] measures postings: job function, geography and freshness
//!   for everyone, and eligibility for a reference profile (living in
//!   Brazil, remote only, no relocation), Narrow's core audience, without
//!   reading anyone's private profile.
//! * [`LocalApp::source_report`] adds, per stored source, its health from
//!   recent scans, its registry entry, and (when a profile exists) the
//!   person's own ranking counts, from the rules alone: no model calls and
//!   nothing recorded as shown.
//! * [`LocalApp::discover_companies`] runs [`company::discover`] over a
//!   list of companies and measures every board it finds, without adding
//!   anything to the configuration.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use futures::StreamExt;
use jobhunt_core::SourceKey;
use jobhunt_eligibility::geo::{self, Area, Membership, Region};
use jobhunt_eligibility::job::RemoteScope;
use jobhunt_eligibility::profile::ProfileLocation;
use jobhunt_eligibility::{Eligibility, ProfileFacts};
use jobhunt_jobs::{JobPosting, JobQuery, JobRecord, JobStatus, OpportunityId};
use jobhunt_profile::{Stance, WorkMode};
use jobhunt_ranking::facets::{self, JobFunction};
use jobhunt_ranking::{RankQuery, RankingService, RuleReader, Tier};
use jobhunt_sources::HttpClient;
use jobhunt_sources::company::{self, CompanyProbe, CompanyTarget, ProbeSettings};
use jobhunt_sources::registry::{
    self, Freshness, GeoBucket, Health, PersonYield, RegistryEntry, SourceRegistry, SourceStatus,
    SourceYield, Validation,
};
use serde::Serialize;

use crate::LocalApp;
use crate::error::AppError;

/// Scans per source read to judge its health.
const HEALTH_SCANS: usize = 5;

/// Longest wait for one page of a company's site during discovery.
const DISCOVERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// The reference profile: lives in São Paulo, Brazil, may work there,
/// works remotely only and does not relocate.
pub fn reference_profile() -> ProfileFacts {
    let brazil = geo::country("BR").map(Area::Country);
    ProfileFacts {
        location: Some(ProfileLocation::read(
            "São Paulo, Brazil",
            jobhunt_eligibility::profile::FactBasis::Preference,
        )),
        work_modes: vec![(WorkMode::Remote, Stance::Required)],
        relocation: Some(false),
        authorized_in: vec![(brazil, "Brazil".to_owned())],
        ..ProfileFacts::default()
    }
}

/// Where a posting's remote work can be done, from its own location data.
pub fn geo_bucket(record: &JobRecord) -> GeoBucket {
    let requirements = jobhunt_eligibility::requirements(record);
    let Some(scope) = requirements.remote_option() else {
        return GeoBucket::Office;
    };
    let areas = match scope {
        RemoteScope::Global(_) => return GeoBucket::Global,
        RemoteScope::Unknown => return GeoBucket::UnknownScope,
        RemoteScope::Areas(areas) => areas,
    };
    let Some(brazil) = geo::country("BR") else {
        return GeoBucket::Other;
    };
    let mut buckets: Vec<GeoBucket> = Vec::new();
    for scoped in areas {
        let area = &scoped.area;
        let bucket = match area {
            Area::Worldwide => GeoBucket::Global,
            _ if area.country().is_some_and(|c| c.code == "BR") => GeoBucket::Brazil,
            _ if area.contains(brazil) == Membership::Yes => GeoBucket::Americas,
            Area::Region(Region::NorthAmerica) => GeoBucket::NorthAmerica,
            Area::Region(
                Region::Europe
                | Region::EuropeanUnion
                | Region::Eea
                | Region::Nordics
                | Region::Dach,
            ) => GeoBucket::Europe,
            _ => match area.country().map(|c| c.continent) {
                Some("NA")
                    if area
                        .country()
                        .is_some_and(|c| matches!(c.code, "US" | "CA")) =>
                {
                    GeoBucket::NorthAmerica
                }
                Some("EU") => GeoBucket::Europe,
                _ => GeoBucket::Other,
            },
        };
        buckets.push(bucket);
    }
    for preferred in [GeoBucket::Brazil, GeoBucket::Global, GeoBucket::Americas] {
        if buckets.contains(&preferred) {
            return preferred;
        }
    }
    match buckets.first() {
        Some(first) if buckets.iter().all(|b| b == first) => *first,
        _ => GeoBucket::Other,
    }
}

/// Measures postings (one source's, usually). `person` is added by the
/// caller when a ranking was run.
pub fn yield_of<'a>(
    records: impl IntoIterator<Item = &'a JobRecord>,
    reference: &ProfileFacts,
    now: DateTime<Utc>,
) -> SourceYield {
    let mut y = SourceYield::default();
    let mut geo: HashMap<GeoBucket, usize> = HashMap::new();
    let mut freshness = Freshness::default();
    for record in records {
        y.open += 1;
        let engineering = facets::facets(record).function == JobFunction::Engineering;
        y.engineering += usize::from(engineering);
        let status = jobhunt_eligibility::evaluate_record(record, reference).status;
        let open = matches!(status, Eligibility::Eligible | Eligibility::Conditional);
        let unclear = status == Eligibility::Uncertain;
        y.brazil_open += usize::from(open);
        y.brazil_unclear += usize::from(unclear);
        y.engineering_brazil_open += usize::from(engineering && open);
        y.engineering_brazil_unclear += usize::from(engineering && unclear);
        *geo.entry(geo_bucket(record)).or_default() += 1;
        freshness.add(record.posting.posted_at, now);
    }
    y.geo = GeoBucket::ALL
        .iter()
        .map(|b| (*b, geo.get(b).copied().unwrap_or(0)))
        .collect();
    y.freshness = freshness;
    y
}

/// A posting fetched straight from a board, as an unsaved record (for
/// measuring a candidate board before anything is stored).
pub fn unsaved_record(posting: JobPosting, now: DateTime<Utc>) -> JobRecord {
    let id = posting.id();
    JobRecord {
        id,
        posting,
        first_seen_at: now,
        last_seen_at: now,
        content_updated_at: now,
        status: JobStatus::Open,
        closed_at: None,
        opportunity_id: OpportunityId::founded_by(id),
    }
}

/// One source in a [`SourceReport`].
#[derive(Debug, Clone, Serialize)]
pub struct SourceRow {
    pub source: SourceKey,
    pub company: Option<String>,
    /// In the configuration this report was made with.
    pub configured: bool,
    /// Its registry entry, when a registry was given.
    pub registry: Option<RegistryEntry>,
    pub health: Health,
    pub health_reason: String,
    pub last_scan_at: Option<DateTime<Utc>>,
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    /// The most recent time any of its postings was seen listed.
    pub last_job_seen_at: Option<DateTime<Utc>>,
    #[serde(rename = "yield")]
    pub yields: SourceYield,
    /// For a registry entry: the status its health (and nothing else)
    /// moves it to, when that differs.
    pub suggested_status: Option<SourceStatus>,
    /// Unmet conditions for activation ([`registry::activation`]; empty:
    /// the evidence supports reading it in production).
    pub activation: Vec<String>,
}

/// Every source's yield and health.
#[derive(Debug, Clone, Serialize)]
pub struct SourceReport {
    pub now: DateTime<Utc>,
    pub rows: Vec<SourceRow>,
    /// All open postings together.
    pub totals: SourceYield,
    /// Whether the person's ranking was run (a profile exists).
    pub ranked: bool,
}

/// Discovery's result for one company, with each board's validation and
/// yield.
#[derive(Debug, Clone, Serialize)]
pub struct CompanyDiscovery {
    #[serde(flatten)]
    pub probe: CompanyProbe,
    /// Per checked board, in `probe.checks` order.
    pub assessments: Vec<BoardAssessment>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BoardAssessment {
    pub source: String,
    pub validation: Validation,
    #[serde(rename = "yield")]
    pub yields: SourceYield,
    /// Unmet conditions for activation (empty: ready to activate).
    pub activation: Vec<String>,
    /// Already in this configuration.
    pub configured: bool,
}

impl CompanyDiscovery {
    /// The registry entries discovery proposes, one per checked board:
    /// validated, rejected, or still a candidate when inconclusive.
    pub fn registry_entries(&self, since: &str) -> Vec<RegistryEntry> {
        let mut out = Vec::new();
        for (check, board) in self.probe.checks.iter().zip(&self.assessments) {
            let status = match board.validation.verdict {
                registry::Verdict::Validated => SourceStatus::Validated,
                registry::Verdict::Rejected => SourceStatus::Rejected,
                registry::Verdict::Inconclusive => SourceStatus::Candidate,
            };
            let Ok(source) = check.source.parse::<SourceKey>() else {
                continue;
            };
            let mut reasons = board.validation.reasons.clone();
            if status == SourceStatus::Validated {
                if board.activation.is_empty() {
                    reasons.push("ready to activate".into());
                } else {
                    reasons.extend(board.activation.iter().map(|u| format!("not yet: {u}")));
                }
            }
            out.push(RegistryEntry {
                source,
                company: self.probe.company.clone(),
                domain: Some(self.probe.domain.clone()),
                careers_url: self.probe.careers_url.clone(),
                status,
                since: since.to_owned(),
                provenance: format!("company discovery ({})", check.evidence.as_str()),
                reasons,
                notes: None,
            });
        }
        out
    }
}

impl LocalApp {
    /// Every source's yield and health, from what is stored: configured
    /// sources and any other source with stored jobs. With a profile, the
    /// person's ranking counts are added (rules only; no model calls,
    /// nothing recorded).
    pub async fn source_report(
        &self,
        registry: Option<&SourceRegistry>,
        now: DateTime<Utc>,
    ) -> Result<SourceReport, AppError> {
        let configured = self
            .config()
            .sources
            .specs()
            .map_err(|e| AppError::Config(e.to_string()))?;
        let query = JobQuery {
            status: Some(JobStatus::Open),
            ..JobQuery::default()
        };
        let records = self.store().search(&query).await?;
        let scans = self.store().recent_scans(HEALTH_SCANS).await?;

        let person = self.person_yields(&records, now).await?;
        let ranked = person.is_some();
        let person = person.unwrap_or_default();

        let mut by_source: HashMap<SourceKey, Vec<&JobRecord>> = HashMap::new();
        for r in &records {
            by_source
                .entry(r.posting.provenance.source.clone())
                .or_default()
                .push(r);
        }
        let mut keys: Vec<SourceKey> = configured.iter().map(|s| s.key().clone()).collect();
        for key in by_source.keys().chain(scans.keys()) {
            if !keys.contains(key) {
                keys.push(key.clone());
            }
        }
        let reference = reference_profile();
        let mut rows = Vec::new();
        for key in keys {
            let jobs = by_source.get(&key).map(Vec::as_slice).unwrap_or_default();
            let mut yields = yield_of(jobs.iter().copied(), &reference, now);
            if ranked {
                yields.person = Some(person.get(&key).copied().unwrap_or_default());
            }
            let history: Vec<registry::ScanSummary> = scans
                .get(&key)
                .map(|list| {
                    list.iter()
                        .map(|s| registry::ScanSummary {
                            finished_at: s.finished_at,
                            status: s.status.clone(),
                            received: usize::try_from(s.received).unwrap_or(usize::MAX),
                            error: s.error.clone(),
                        })
                        .collect()
                })
                .unwrap_or_default();
            let (health, health_reason) = registry::health(&history);
            let entry = registry.and_then(|r| r.get(&key)).cloned();
            let suggested_status = entry.as_ref().and_then(|e| {
                let next = registry::next_status(e.status, health, None);
                (next != e.status).then_some(next)
            });
            let spec = configured.iter().find(|s| s.key() == &key);
            rows.push(SourceRow {
                company: spec
                    .and_then(|s| s.company().map(str::to_owned))
                    .or_else(|| entry.as_ref().map(|e| e.company.clone()))
                    .or_else(|| jobs.first().map(|r| r.posting.company.clone())),
                configured: spec.is_some(),
                registry: entry,
                health,
                health_reason,
                last_scan_at: history.first().map(|s| s.finished_at),
                last_success_at: history
                    .iter()
                    .find(|s| s.succeeded())
                    .map(|s| s.finished_at),
                last_error: history
                    .first()
                    .filter(|s| !s.succeeded())
                    .and_then(|s| s.error.clone()),
                last_job_seen_at: jobs.iter().map(|r| r.last_seen_at).max(),
                activation: registry::activation(&yields),
                yields,
                suggested_status,
                source: key,
            });
        }
        let mut totals = yield_of(records.iter(), &reference, now);
        if ranked {
            totals.person = Some(rows.iter().filter_map(|r| r.yields.person).fold(
                PersonYield::default(),
                |a, p| PersonYield {
                    actionable: a.actionable + p.actionable,
                    plausible: a.plausible + p.plausible,
                    strong: a.strong + p.strong,
                },
            ));
        }
        Ok(SourceReport {
            now,
            rows,
            totals,
            ranked,
        })
    }

    /// The person's ranking counts per source of each ranked job (rules
    /// only: no model calls, nothing recorded as shown), or `None` without
    /// a profile. `records` are the stored open jobs.
    pub async fn person_yields(
        &self,
        records: &[JobRecord],
        now: DateTime<Utc>,
    ) -> Result<Option<HashMap<SourceKey, PersonYield>>, AppError> {
        if self.profile_facts().await?.is_none() {
            return Ok(None);
        }
        let service = RankingService::new(self.store(), &RuleReader).with_policy(self.policy());
        let query = RankQuery {
            text: String::new(),
            store_top: 0,
            all: true,
        };
        let report = service.rank(&query, now).await?;
        let source_of: HashMap<_, _> = records
            .iter()
            .map(|r| (r.id, r.posting.provenance.source.clone()))
            .collect();
        let mut person: HashMap<SourceKey, PersonYield> = HashMap::new();
        for r in &report.rankings {
            let Some(source) = source_of.get(&r.job) else {
                continue;
            };
            let p = person.entry(source.clone()).or_default();
            p.actionable += 1;
            p.plausible += usize::from(r.tier == Tier::WorthReviewing);
            p.strong += usize::from(r.tier == Tier::StrongFit);
        }
        Ok(Some(person))
    }

    /// Finds and checks the boards of each company, measuring every board
    /// found. Reads the network; stores nothing and changes no
    /// configuration.
    pub async fn discover_companies(
        &self,
        targets: &[CompanyTarget],
        settings: &ProbeSettings,
        now: DateTime<Utc>,
    ) -> Result<Vec<CompanyDiscovery>, AppError> {
        // Company sites are not job boards: a slow or unresponsive one is
        // a finding, not something to wait out.
        let mut settings_http = self.config().discovery.http_settings();
        settings_http.timeout = settings_http.timeout.min(DISCOVERY_TIMEOUT);
        settings_http.max_retries = settings_http.max_retries.min(1);
        let http = HttpClient::new(settings_http)
            .map_err(|e| AppError::Config(format!("HTTP client: {e}")))?;
        let configured: Vec<String> = self
            .config()
            .sources
            .specs()
            .map_err(|e| AppError::Config(e.to_string()))?
            .iter()
            .map(|s| s.key().to_string())
            .collect();
        let reference = reference_profile();
        let concurrency = self.config().discovery.concurrency.max(1);
        let mut out = Vec::with_capacity(targets.len());
        // A few companies at a time: each company's pages are read one by
        // one, and the client limits requests per host. Unordered, so one
        // slow site doesn't hold back the others; reported in input order.
        let mut probes: Vec<(usize, CompanyProbe)> =
            futures::stream::iter(targets.iter().enumerate())
                .map(|(i, t)| {
                    let http = &http;
                    async move { (i, company::discover(http, t, settings, now).await) }
                })
                .buffer_unordered(concurrency)
                .collect()
                .await;
        probes.sort_by_key(|(i, _)| *i);
        let probes = probes.into_iter().map(|(_, p)| p);
        {
            for mut probe in probes {
                tracing::info!(
                    domain = %probe.domain,
                    outcome = probe.outcome.as_str(),
                    boards = probe.boards.len(),
                    "company probed"
                );
                let assessments = probe
                    .checks
                    .iter_mut()
                    .map(|check| {
                        let postings = std::mem::take(&mut check.postings);
                        let records: Vec<JobRecord> = postings
                            .into_iter()
                            .map(|p| unsaved_record(p, now))
                            .collect();
                        let yields = yield_of(records.iter(), &reference, now);
                        let validation = registry::validate(check);
                        let activation = if validation.verdict == registry::Verdict::Validated {
                            registry::activation(&yields)
                        } else {
                            Vec::new()
                        };
                        BoardAssessment {
                            source: check.source.clone(),
                            configured: configured.contains(&check.source),
                            validation,
                            yields,
                            activation,
                        }
                    })
                    .collect();
                out.push(CompanyDiscovery { probe, assessments });
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use jobhunt_core::{CanonicalUrl, Provenance};
    use jobhunt_jobs::WorkplaceType;

    use super::*;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 5, 0, 0, 0)
            .single()
            .unwrap_or_default()
    }

    fn record(title: &str, location: &str, remote: bool) -> JobRecord {
        let posting = JobPosting {
            provenance: Provenance {
                source: SourceKey::new("ashby", "acme").unwrap_or_else(|e| panic!("{e}")),
                source_record_id: Some(title.to_owned()),
                fetched_from: None,
            },
            url: CanonicalUrl::parse(&format!("https://jobs.ashbyhq.com/acme/{}", title.len()))
                .unwrap_or_else(|e| panic!("{e}")),
            apply_url: None,
            company: "Acme".into(),
            title: title.into(),
            department: None,
            team: None,
            location: Some(location.into()),
            locations: Vec::new(),
            employment_type: None,
            workplace_type: Some(if remote {
                WorkplaceType::Remote
            } else {
                WorkplaceType::OnSite
            }),
            is_remote: Some(remote),
            compensation: None,
            work_authorization: None,
            description_text: None,
            description_html: None,
            posted_at: None,
            source_updated_at: None,
        };
        unsaved_record(posting, now())
    }

    #[test]
    fn buckets_postings_by_their_own_geography() {
        let cases = [
            ("Remote - Worldwide", true, GeoBucket::Global),
            ("Remote, Brazil", true, GeoBucket::Brazil),
            ("Remote - LATAM", true, GeoBucket::Americas),
            ("Remote (United States)", true, GeoBucket::NorthAmerica),
            ("Remote - Germany", true, GeoBucket::Europe),
            ("Remote", true, GeoBucket::UnknownScope),
            ("New York, NY", false, GeoBucket::Office),
        ];
        for (location, remote, expected) in cases {
            assert_eq!(
                geo_bucket(&record("Senior Backend Engineer", location, remote)),
                expected,
                "{location}"
            );
        }
    }

    #[test]
    fn yield_counts_engineering_open_to_brazil() {
        let records = [
            record("Senior Backend Engineer", "Remote - Worldwide", true),
            record("Senior Backend Engineer II", "Remote (United States)", true),
            record("Account Executive", "Remote, Brazil", true),
        ];
        let y = yield_of(records.iter(), &reference_profile(), now());
        assert_eq!((y.open, y.engineering), (3, 2));
        assert_eq!(y.engineering_brazil_open, 1, "{y:?}");
        assert_eq!(y.brazil_open, 2, "{y:?}");
        assert_eq!(y.geo_count(GeoBucket::NorthAmerica), 1);
        // No source publish date: unknown, never fresh.
        assert_eq!((y.freshness.unknown, y.freshness.under_7_days), (3, 0));
        assert!(registry::activation(&y).is_empty());
    }
}
