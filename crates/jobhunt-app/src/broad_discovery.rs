//! Broad discovery: resolving the candidates in a
//! [`DiscoveryStore`] back to first-party boards, and measuring what each
//! discovery strategy yields.
//!
//! * Companies (a domain from a directory, a hiring post, a careers link)
//!   go through [`LocalApp::discover_companies`], the same company →
//!   careers page → board path as `narrow sources discover`. A company
//!   whose careers page shows no board gets a bounded look at its sitemap
//!   and `JobPosting` data ([`sitemap::surface`]).
//! * Boards are read once through their adapter ([`resolve::read_board`]),
//!   which settles every discovered job on them (live or closed). Boards
//!   worth it then get their owner checked ([`resolve::establish_ownership`]):
//!   the company's own site must point back at the board.
//! * Every board is measured like a registry source ([`yield_of`]): job
//!   function, geography, freshness and the reference profile's
//!   eligibility. The person's own ranking is only read, rules only, from
//!   what is stored.
//!
//! Nothing here changes the configuration, the registry, ranking or Today.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use futures::StreamExt;
use jobhunt_eligibility::Eligibility;
use jobhunt_jobs::{JobQuery, JobRecord, JobStatus};
use jobhunt_ranking::facets::{self, JobFunction};
use jobhunt_sources::HttpClient;
use jobhunt_sources::careers::BoardRef;
use jobhunt_sources::company::{
    self, BoardEvidence, CompanyTarget, FoundBoard, ProbeOutcome, ProbeSettings,
};
use jobhunt_sources::discovery::resolve::{self, DomainHint, HintSource, ResolveSettings};
use jobhunt_sources::discovery::{
    BoardResult, CandidateKind, CandidateStatus, CompanyResult, DiscoveryStore, JobResult, Method,
    Ownership, Sighting, report, sitemap,
};
use jobhunt_sources::registry::{self, SourceRegistry, SourceYield, Verdict};
use serde::Serialize;

use crate::LocalApp;
use crate::error::AppError;
use crate::sources::{geo_bucket, reference_profile, unsaved_record, yield_of};

/// Which read boards get their ownership checked (the expensive stage:
/// several pages of the company's site).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OwnershipScope {
    /// Every board that lists postings.
    #[default]
    All,
    /// Boards with an engineering posting open to (or unclear for) a
    /// Brazil-based remote candidate.
    Useful,
    /// None: read only.
    Off,
}

#[derive(Debug, Clone, Default)]
pub struct ResolveOptions {
    /// Candidates to resolve at most in this call.
    pub limit: usize,
    pub ownership: OwnershipScope,
    pub companies: bool,
    pub boards: bool,
    /// Look at sitemaps and JSON-LD for companies without a board.
    pub surface: bool,
    /// Resolve inconclusive candidates again.
    pub recheck: bool,
    /// Only candidates one of whose sightings has this strategy prefix.
    pub strategy: Option<String>,
    /// Order pending candidates by a seeded hash instead of store order
    /// (a reproducible random sample).
    pub shuffle_seed: Option<u64>,
    /// Concurrent board reads, and concurrent ownership checks.
    pub concurrency: usize,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ResolveSummary {
    pub companies: usize,
    pub boards_read: usize,
    pub ownership_checked: usize,
    pub jobs_settled: usize,
    pub boards_added: usize,
}

impl ResolveSummary {
    pub fn is_empty(&self) -> bool {
        self.companies + self.boards_read == 0
    }
}

fn status_of(verdict: Verdict) -> CandidateStatus {
    match verdict {
        Verdict::Validated => CandidateStatus::Validated,
        Verdict::Rejected => CandidateStatus::Rejected,
        Verdict::Inconclusive => CandidateStatus::Inconclusive,
    }
}

fn company_status(outcome: ProbeOutcome) -> CandidateStatus {
    match outcome {
        ProbeOutcome::Board => CandidateStatus::Resolved,
        ProbeOutcome::UnsupportedAts => CandidateStatus::UnsupportedAts,
        ProbeOutcome::CustomPage => CandidateStatus::CustomPage,
        ProbeOutcome::NoCareersPage => CandidateStatus::NoCareersPage,
        ProbeOutcome::Blocked => CandidateStatus::Blocked,
        ProbeOutcome::Unreachable => CandidateStatus::Unreachable,
    }
}

fn listing_of(check: &company::BoardCheck) -> &'static str {
    match &check.error {
        Some(e) if e.contains("not found") || e.contains("404") => "not_found",
        Some(_) if check.transient => "transient",
        Some(_) => "failed",
        None if check.jobs == 0 => "empty",
        None => "ok",
    }
}

fn useful(y: &SourceYield) -> bool {
    y.engineering_brazil_open + y.engineering_brazil_unclear > 0
}

/// A board result replaces an older one unless it is weaker.
fn replaces(current: CandidateStatus, new: CandidateStatus) -> bool {
    matches!(
        current,
        CandidateStatus::New | CandidateStatus::Inconclusive
    ) || new == CandidateStatus::Validated
}

fn order_key(seed: Option<u64>, key: &str) -> String {
    match seed {
        Some(seed) => jobhunt_sources::discovery::queries::stable_id(&format!("{seed}:{key}")),
        None => String::new(),
    }
}

impl LocalApp {
    /// The HTTP client discovery uses: company sites are not job boards, so
    /// a slow one is a finding, not something to wait out.
    pub fn discovery_http(&self) -> Result<HttpClient, AppError> {
        let mut settings = self.config().discovery.http_settings();
        settings.timeout = settings.timeout.min(std::time::Duration::from_secs(15));
        settings.max_retries = settings.max_retries.min(1);
        HttpClient::new(settings).map_err(|e| AppError::Config(format!("HTTP client: {e}")))
    }

    /// Resolves up to `opts.limit` pending candidates: companies first,
    /// then boards (the jobs on them are settled by the board's read).
    /// Call again until the summary is empty; the store can be saved in
    /// between.
    pub async fn resolve_discovery(
        &self,
        store: &mut DiscoveryStore,
        registry: Option<&SourceRegistry>,
        opts: &ResolveOptions,
        settings: &ResolveSettings,
        now: DateTime<Utc>,
    ) -> Result<ResolveSummary, AppError> {
        let http = self.discovery_http()?;
        let mut summary = ResolveSummary::default();
        let pending = |store: &DiscoveryStore, kind: CandidateKind| -> Vec<String> {
            let mut keys: Vec<(String, String)> = store
                .of_kind(kind)
                .filter(|c| {
                    c.status == CandidateStatus::New
                        || (opts.recheck && c.status == CandidateStatus::Inconclusive)
                })
                .filter(|c| {
                    opts.strategy
                        .as_deref()
                        .is_none_or(|p| c.sightings.iter().any(|s| s.strategy().starts_with(p)))
                })
                .map(|c| (order_key(opts.shuffle_seed, &c.key), c.key.clone()))
                .collect();
            keys.sort();
            keys.into_iter()
                .map(|(_, k)| k)
                .take(opts.limit.max(1))
                .collect()
        };
        let registry_status = |source: &str| {
            registry.and_then(|r| {
                r.sources
                    .iter()
                    .find(|e| e.source.to_string() == source)
                    .map(|e| e.status.as_str().to_owned())
            })
        };

        if opts.companies {
            let keys = pending(store, CandidateKind::Company);
            if !keys.is_empty() {
                summary.companies = keys.len();
                self.resolve_companies(
                    store,
                    &http,
                    &keys,
                    opts,
                    settings,
                    now,
                    &mut summary,
                    &registry_status,
                )
                .await?;
                return Ok(summary);
            }
        }
        if !opts.boards {
            return Ok(summary);
        }
        let keys = pending(store, CandidateKind::Board);
        if keys.is_empty() {
            return Ok(summary);
        }
        let refs: Vec<(String, BoardRef)> = keys
            .iter()
            .filter_map(|k| {
                let c = store.get(k)?;
                Some((k.clone(), c.board.as_ref()?.to_ref()?))
            })
            .collect();
        let concurrency = opts.concurrency.max(1);
        let reads: Vec<(String, resolve::BoardRead)> = futures::stream::iter(refs)
            .map(|(key, board)| {
                let http = &http;
                async move { (key, resolve::read_board(http, &board, settings, now).await) }
            })
            .buffer_unordered(concurrency)
            .collect()
            .await;
        summary.boards_read = reads.len();
        let reference = reference_profile();
        let mut measured: Vec<(String, resolve::BoardRead, SourceYield, Vec<JobRecord>)> = reads
            .into_iter()
            .map(|(key, mut read)| {
                let postings = std::mem::take(&mut read.check.postings);
                let records: Vec<JobRecord> = postings
                    .into_iter()
                    .map(|p| unsaved_record(p, now))
                    .collect();
                let y = yield_of(records.iter(), &reference, now);
                read.check.postings = records.iter().map(|r| r.posting.clone()).collect();
                (key, read, y, records)
            })
            .collect();
        tracing::info!(boards = measured.len(), "boards read");

        // Ownership, for the boards that get it.
        let wanted: Vec<usize> = measured
            .iter()
            .enumerate()
            .filter(|(_, (_, read, y, _))| {
                read.listing == "ok"
                    && match opts.ownership {
                        OwnershipScope::All => true,
                        OwnershipScope::Useful => useful(y),
                        OwnershipScope::Off => false,
                    }
            })
            .map(|(i, _)| i)
            .collect();
        let inputs: Vec<(usize, Vec<String>, Option<String>)> = wanted
            .iter()
            .map(|&i| {
                let c = store.get(&measured[i].0);
                let independent = c
                    .filter(|c| {
                        c.sightings
                            .iter()
                            .any(|s| s.method != Method::AtsSearch && s.method != Method::WebIndex)
                    })
                    .and_then(|c| c.domain_hint.clone())
                    .into_iter()
                    .collect();
                (i, independent, c.and_then(|c| c.company_hint.clone()))
            })
            .collect();
        let owners: Vec<(usize, resolve::OwnershipResult)> = {
            let measured = &measured;
            futures::stream::iter(inputs)
                .map(|(i, independent, company)| {
                    let http = &http;
                    async move {
                        let read = &measured[i].1;
                        (
                            i,
                            resolve::establish_ownership(
                                http,
                                read,
                                &independent,
                                company.as_deref(),
                                settings,
                                now,
                            )
                            .await,
                        )
                    }
                })
                .buffer_unordered(concurrency * 2)
                .collect()
                .await
        };
        summary.ownership_checked = owners.len();
        let mut owners: HashMap<usize, resolve::OwnershipResult> = owners.into_iter().collect();

        for (i, (key, read, y, records)) in measured.drain(..).enumerate() {
            // Settle every discovered job on the board.
            for job_key in store.jobs_of(&key) {
                let Some(job) = store.get_mut(&job_key) else {
                    continue;
                };
                let job_id = job.job_id.clone().unwrap_or_default();
                job.last_checked_at = Some(now);
                summary.jobs_settled += 1;
                match read.listing {
                    "ok" => match records.iter().find(|r| {
                        r.posting
                            .provenance
                            .source_record_id
                            .as_deref()
                            .is_some_and(|id| id.eq_ignore_ascii_case(&job_id))
                    }) {
                        Some(record) => {
                            job.status = CandidateStatus::Live;
                            job.reason = None;
                            let status =
                                jobhunt_eligibility::evaluate_record(record, &reference).status;
                            job.job_result = Some(JobResult {
                                title: Some(record.posting.title.clone()),
                                posted_at: record.posting.posted_at,
                                first_seen_at: job.first_discovered_at,
                                engineering: facets::facets(record).function
                                    == JobFunction::Engineering,
                                geo: Some(geo_bucket(record)),
                                brazil: Some(
                                    match status {
                                        Eligibility::Eligible | Eligibility::Conditional => "open",
                                        Eligibility::Uncertain => "unclear",
                                        _ => "closed",
                                    }
                                    .to_owned(),
                                ),
                            });
                        }
                        None => {
                            job.status = CandidateStatus::Closed;
                            job.reason = Some("not on the board's current listing".into());
                        }
                    },
                    "not_found" | "empty" => {
                        job.status = CandidateStatus::Closed;
                        job.reason = Some(format!(
                            "the board is {}",
                            if read.listing == "empty" {
                                "empty"
                            } else {
                                "gone"
                            }
                        ));
                    }
                    _ => {
                        job.status = CandidateStatus::Unknown;
                        job.reason = read.check.error.clone();
                    }
                }
            }

            let owner = owners.remove(&i);
            let (result, status) = match owner {
                Some(o) => {
                    let status = status_of(o.validation.verdict);
                    let activation = if status == CandidateStatus::Validated {
                        registry::activation(&y)
                    } else {
                        Vec::new()
                    };
                    for other in &o.points_at {
                        if store
                            .add_derived_board(other, &key, "the company's site points here", now)
                            .is_some_and(|a| a.new)
                        {
                            summary.boards_added += 1;
                        }
                    }
                    for probe in &o.probes {
                        self.file_company_probe(store, &key, probe, now);
                    }
                    (
                        BoardResult {
                            listing: read.listing.into(),
                            jobs: read.check.jobs,
                            error: read.check.error.clone(),
                            company: o.company,
                            domain: o.domain,
                            careers_url: o.careers_url,
                            hints: o.hints,
                            ownership: o.ownership,
                            points_at: o
                                .points_at
                                .iter()
                                .filter_map(jobhunt_sources::discovery::ats::source_key)
                                .collect(),
                            validation: o.validation,
                            yields: Some(y),
                            activation,
                        },
                        status,
                    )
                }
                None => {
                    let mut validation = resolve::validate_read(&read);
                    if validation.verdict != Verdict::Rejected {
                        validation.verdict = Verdict::Inconclusive;
                        validation.reasons.push(if read.listing == "ok" {
                            "ownership not checked (no engineering posting open to Brazil)".into()
                        } else {
                            format!("board could not be read ({})", read.listing)
                        });
                    }
                    let status = status_of(validation.verdict);
                    (
                        BoardResult {
                            listing: read.listing.into(),
                            jobs: read.check.jobs,
                            error: read.check.error.clone(),
                            company: None,
                            domain: None,
                            careers_url: None,
                            hints: Vec::new(),
                            ownership: Ownership::Unknown,
                            points_at: Vec::new(),
                            validation,
                            yields: (read.listing == "ok").then_some(y),
                            activation: Vec::new(),
                        },
                        status,
                    )
                }
            };
            let Some(c) = store.get_mut(&key) else {
                continue;
            };
            c.last_checked_at = Some(now);
            if replaces(c.status, status) {
                c.reason = result.validation.reasons.last().cloned();
                c.status = status;
                c.board_result = Some(result);
                c.registry_status = c.source.as_deref().and_then(registry_status);
            }
        }
        Ok(summary)
    }

    /// Records what a company probe found as a (derived) company candidate:
    /// its careers page and the unsupported ATS it names.
    fn file_company_probe(
        &self,
        store: &mut DiscoveryStore,
        from: &str,
        probe: &company::CompanyProbe,
        now: DateTime<Utc>,
    ) {
        let key = format!("company:{}", probe.domain);
        if store.get(&key).is_none() {
            let Some(parent) = store.get(from).cloned() else {
                return;
            };
            for s in &parent.sightings {
                let mut sighting = s.clone();
                sighting.derived = true;
                sighting
                    .metadata
                    .insert("via".into(), "board ownership check".into());
                store.add(jobhunt_sources::discovery::Lead::domain(
                    &probe.domain,
                    sighting,
                ));
            }
        }
        let Some(c) = store.get_mut(&key) else { return };
        if c.status == CandidateStatus::New {
            c.status = company_status(probe.outcome);
            c.last_checked_at = Some(now);
            c.company_result = Some(CompanyResult {
                careers_url: probe.careers_url.clone(),
                boards: probe.boards.iter().map(|b| b.source.clone()).collect(),
                other_ats: probe
                    .other_ats
                    .iter()
                    .map(|o| o.provider.to_owned())
                    .collect(),
                surface: None,
            });
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn resolve_companies(
        &self,
        store: &mut DiscoveryStore,
        http: &HttpClient,
        keys: &[String],
        opts: &ResolveOptions,
        settings: &ResolveSettings,
        now: DateTime<Utc>,
        summary: &mut ResolveSummary,
        registry_status: &dyn Fn(&str) -> Option<String>,
    ) -> Result<(), AppError> {
        let targets: Vec<CompanyTarget> = keys
            .iter()
            .filter_map(|k| {
                let c = store.get(k)?;
                Some(CompanyTarget {
                    domain: c.domain_hint.clone()?,
                    name: c.company_hint.clone(),
                })
            })
            .collect();
        let probe_settings = ProbeSettings {
            api_base: settings.api_base.clone(),
            site_base: settings.site_base.clone(),
            ..ProbeSettings::default()
        };
        let found = self
            .discover_companies(&targets, &probe_settings, now)
            .await?;
        let reference = reference_profile();
        // Sitemaps and JSON-LD for companies without a board, many at a
        // time (each company's own pages are read one by one).
        let mut surfaces: HashMap<String, (sitemap::Surface, Vec<company::BoardCheck>)> =
            if opts.surface {
                futures::stream::iter(found.iter().filter(|d| {
                    matches!(
                        d.probe.outcome,
                        ProbeOutcome::CustomPage | ProbeOutcome::NoCareersPage
                    )
                }))
                .map(|d| async move {
                    let careers = d
                        .probe
                        .careers_url
                        .as_deref()
                        .and_then(|u| url::Url::parse(u).ok());
                    let surface = sitemap::surface(
                        http,
                        &d.probe.domain,
                        careers.as_ref(),
                        sitemap::SurfaceLimits::default(),
                        settings.site_base.as_ref(),
                    )
                    .await;
                    let target = CompanyTarget {
                        domain: d.probe.domain.clone(),
                        name: Some(d.probe.company.clone()),
                    };
                    let mut checks = Vec::new();
                    for board in &surface.boards {
                        let found = FoundBoard {
                            source: jobhunt_sources::discovery::ats::source_key(board)
                                .unwrap_or_default(),
                            board: board.clone(),
                            evidence: BoardEvidence::CareersPage,
                            page: None,
                            matches_company: company::slug_matches(&board.name, &target),
                        };
                        checks.push(
                            company::check_board(
                                http,
                                &target,
                                &found,
                                settings.api_base.as_deref(),
                                now,
                            )
                            .await,
                        );
                    }
                    (d.probe.domain.clone(), (surface, checks))
                })
                .buffer_unordered(opts.concurrency.max(1))
                .collect()
                .await
            } else {
                HashMap::new()
            };
        for d in found {
            let key = format!("company:{}", d.probe.domain);
            let mut result = CompanyResult {
                careers_url: d.probe.careers_url.clone(),
                boards: d.probe.boards.iter().map(|b| b.source.clone()).collect(),
                other_ats: d
                    .probe
                    .other_ats
                    .iter()
                    .map(|o| o.provider.to_owned())
                    .collect(),
                surface: None,
            };
            let mut status = company_status(d.probe.outcome);
            for (check, assessment) in d.probe.checks.iter().zip(&d.assessments) {
                let Some(found) = d.probe.boards.iter().find(|b| b.source == check.source) else {
                    continue;
                };
                let ownership = match (assessment.validation.verdict, check.evidence.first_party())
                {
                    (Verdict::Validated, true) => Ownership::Verified,
                    (Verdict::Validated, false) => Ownership::Corroborated,
                    _ => Ownership::Claimed,
                };
                let board = BoardResult {
                    listing: listing_of(check).into(),
                    jobs: check.jobs,
                    error: check.error.clone(),
                    company: Some(d.probe.company.clone()),
                    domain: Some(d.probe.domain.clone()),
                    careers_url: d.probe.careers_url.clone(),
                    hints: vec![DomainHint {
                        domain: d.probe.domain.clone(),
                        from: HintSource::Discovery,
                        count: 1,
                    }],
                    ownership,
                    points_at: Vec::new(),
                    validation: assessment.validation.clone(),
                    yields: Some(assessment.yields.clone()),
                    activation: assessment.activation.clone(),
                };
                self.file_board(
                    store,
                    &key,
                    &found.board,
                    check.evidence.as_str(),
                    board,
                    now,
                    summary,
                    registry_status,
                );
            }
            if let Some((surface, checks)) = surfaces.remove(&d.probe.domain) {
                for (board, mut check) in surface.boards.iter().zip(checks) {
                    let records: Vec<JobRecord> = std::mem::take(&mut check.postings)
                        .into_iter()
                        .map(|p| unsaved_record(p, now))
                        .collect();
                    let y = yield_of(records.iter(), &reference, now);
                    let validation = registry::validate(&check);
                    let verdict = validation.verdict;
                    let via = if surface.postings.is_empty() {
                        "sitemap"
                    } else {
                        "jsonld"
                    };
                    let board_result = BoardResult {
                        listing: listing_of(&check).into(),
                        jobs: check.jobs,
                        error: check.error.clone(),
                        company: Some(d.probe.company.clone()),
                        domain: Some(d.probe.domain.clone()),
                        careers_url: d.probe.careers_url.clone(),
                        hints: vec![DomainHint {
                            domain: d.probe.domain.clone(),
                            from: HintSource::Discovery,
                            count: 1,
                        }],
                        ownership: if verdict == Verdict::Validated {
                            Ownership::Verified
                        } else {
                            Ownership::Claimed
                        },
                        points_at: Vec::new(),
                        activation: if verdict == Verdict::Validated {
                            registry::activation(&y)
                        } else {
                            Vec::new()
                        },
                        validation,
                        yields: Some(y),
                    };
                    self.file_board(
                        store,
                        &key,
                        board,
                        via,
                        board_result,
                        now,
                        summary,
                        registry_status,
                    );
                    // The sitemap / JSON-LD route as a strategy of its own.
                    if let Some(c) = store.get_mut(&format!(
                        "board:{}",
                        jobhunt_sources::discovery::ats::BoardId::of(board).key()
                    )) {
                        let method = if via == "jsonld" {
                            Method::Jsonld
                        } else {
                            Method::Sitemap
                        };
                        let mut s = Sighting::new(
                            method,
                            "surface",
                            &format!("https://{}/", d.probe.domain),
                            now,
                        );
                        s.derived = true;
                        if !c
                            .sightings
                            .iter()
                            .any(|x| x.method == method && x.provider == "surface")
                        {
                            c.sightings.push(s);
                        }
                    }
                }
                if !surface.boards.is_empty() {
                    status = CandidateStatus::Resolved;
                }
                for p in &surface.summary.other_ats {
                    if !result.other_ats.contains(p) {
                        result.other_ats.push(p.clone());
                    }
                }
                result.surface = Some(surface.summary);
            }
            if let Some(c) = store.get_mut(&key) {
                c.status = status;
                c.reason = Some(d.probe.outcome.as_str().to_owned());
                c.last_checked_at = Some(now);
                if c.company_hint.is_none() {
                    c.company_hint = Some(d.probe.company.clone());
                }
                c.company_result = Some(result);
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn file_board(
        &self,
        store: &mut DiscoveryStore,
        from: &str,
        board: &BoardRef,
        via: &str,
        result: BoardResult,
        now: DateTime<Utc>,
        summary: &mut ResolveSummary,
        registry_status: &dyn Fn(&str) -> Option<String>,
    ) {
        let Some(added) = store.add_derived_board(board, from, via, now) else {
            return;
        };
        summary.boards_added += usize::from(added.new);
        let status = status_of(result.validation.verdict);
        let Some(c) = store.get_mut(&added.key) else {
            return;
        };
        if replaces(c.status, status) {
            c.status = status;
            c.reason = result.validation.reasons.last().cloned();
            c.last_checked_at = Some(now);
            c.registry_status = c.source.as_deref().and_then(registry_status);
            c.board_result = Some(result);
        }
    }

    /// The yield report, with the person's ranking counts per board when a
    /// profile exists and the boards' jobs are stored (scan them first).
    pub async fn discovery_report(
        &self,
        store: &DiscoveryStore,
        registry: Option<&SourceRegistry>,
        now: DateTime<Utc>,
    ) -> Result<report::Report, AppError> {
        let query = JobQuery {
            status: Some(JobStatus::Open),
            ..JobQuery::default()
        };
        let records = self.store().search(&query).await?;
        let person = if records.is_empty() {
            HashMap::new()
        } else {
            self.person_yields(&records, now)
                .await?
                .unwrap_or_default()
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect()
        };
        let mut r = report::build(store, registry, &person);
        r.now = Some(now);
        Ok(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weaker_results_never_replace_a_validation() {
        assert!(replaces(CandidateStatus::New, CandidateStatus::Rejected));
        assert!(replaces(
            CandidateStatus::Inconclusive,
            CandidateStatus::Validated
        ));
        assert!(!replaces(
            CandidateStatus::Validated,
            CandidateStatus::Inconclusive
        ));
        assert!(replaces(
            CandidateStatus::Rejected,
            CandidateStatus::Validated
        ));
        assert!(!replaces(
            CandidateStatus::Rejected,
            CandidateStatus::Inconclusive
        ));
    }
}
