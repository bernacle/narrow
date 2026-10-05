//! Broad discovery: finding companies and boards Narrow has not heard of.
//!
//! [`crate::company`] goes from a company Narrow already knows to its
//! board. Broad discovery goes the other way: from places where hiring
//! surfaces appear (search engines' index of ATS pages, "Who is hiring"
//! threads, company directories, crawls, sitemaps, `JobPosting` markup) to
//! candidates, and from each candidate back to a first-party board:
//!
//! ```text
//! provider ──▶ Lead ──▶ Candidate (deduped, every sighting kept)
//!                         │ job ─────────▶ its board ─┐
//!                         │ board ────────────────────┤ read through the adapter,
//!                         │ company domain ─▶ careers page / sitemap / JSON-LD ─┤ tied to the
//!                         │ unsupported ATS ─▶ counted                          │ company's site
//!                         ▼                                                     ▼
//!                       DiscoveryStore  ◀── resolution, validation, liveness ───┘
//! ```
//!
//! * A provider ([`providers`], [`import`]) yields [`Lead`]s: a URL and/or a
//!   domain with hints, plus a [`Sighting`] saying where it came from (the
//!   method, the provider, the query, the URL as found, when).
//! * [`DiscoveryStore::add`] classifies the URL ([`ats::classify`]) and
//!   files it under one dedupe key: `job:<provider>:<board>:<job id>`,
//!   `board:<provider>:<board>`, `company:<domain>`, `ats:<provider>:<tenant>`
//!   or `page:<url>`. The same job found by twelve queries is one candidate
//!   with twelve sightings; a job also files a derived sighting on its
//!   board, so "same board from many jobs" is one board. Titles never
//!   identify anything.
//! * [`resolve`] reads each board through its adapter, finds the company's
//!   domain from the board itself, and checks that the company's own site
//!   points back at the board, with [`crate::company::discover`] and the
//!   unchanged [`crate::registry::validate`]. Each job is checked against
//!   its board's current listing, so a stale index entry is a closed job,
//!   not an opening.
//!
//! Nothing is thrown away: a rejected or dead candidate keeps its reason,
//! because measuring which strategies produce noise needs the noise.
//! Nothing is activated either: the store proposes, the registry and the
//! configuration decide.

pub mod ats;
pub mod import;
pub mod jsonld;
pub mod providers;
pub mod queries;
pub mod report;
pub mod resolve;
pub mod sitemap;

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::registry::{GeoBucket, SourceYield, Validation};
use ats::{BoardId, UrlTarget};

/// How a lead was found. Yield is reported per method, provider and query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    /// A web search of an ATS host (`site:jobs.ashbyhq.com …`).
    AtsSearch,
    /// A web crawl's URL index of an ATS host (Common Crawl).
    WebIndex,
    /// A public hiring thread (Hacker News "Who is hiring?").
    HiringThread,
    /// A company directory (YC, remote-company lists).
    CompanyDirectory,
    /// A public job API or feed.
    PublicApi,
    /// A company's sitemap.
    Sitemap,
    /// `JobPosting` structured data on a page.
    Jsonld,
    /// A list given by hand (URLs, domains, CSV).
    ManualImport,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AtsSearch => "ats_search",
            Self::WebIndex => "web_index",
            Self::HiringThread => "hiring_thread",
            Self::CompanyDirectory => "company_directory",
            Self::PublicApi => "public_api",
            Self::Sitemap => "sitemap",
            Self::Jsonld => "jsonld",
            Self::ManualImport => "manual_import",
        }
    }
}

impl std::str::FromStr for Method {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_value(serde_json::Value::String(s.to_owned()))
            .map_err(|_| format!("unknown discovery method {s:?}"))
    }
}

/// Where and how a candidate was seen. One per (method, provider, query,
/// URL); a candidate keeps all of them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sighting {
    pub method: Method,
    /// Which provider: `websearch`, `brave`, `hn:49922569`, `yc-oss`,
    /// `commoncrawl:CC-MAIN-2026-39`, a file name.
    pub provider: String,
    /// The query, for search.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// The query family (`platform engineer · latam`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    /// The URL as it was found.
    pub discovered_url: String,
    pub discovered_at: DateTime<Utc>,
    /// Position in the provider's results.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rank: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Seen through another candidate (a board through one of its jobs, a
    /// board through a company's careers page): not a hit of its own.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub derived: bool,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
}

impl Sighting {
    pub fn new(method: Method, provider: &str, url: &str, at: DateTime<Utc>) -> Self {
        Self {
            method,
            provider: provider.to_owned(),
            query: None,
            family: None,
            discovered_url: url.to_owned(),
            discovered_at: at,
            rank: None,
            title: None,
            derived: false,
            metadata: BTreeMap::new(),
        }
    }

    /// The strategy this sighting counts toward: `ats_search`,
    /// `hiring_thread:hn`, `company_directory:yc-oss`.
    pub fn strategy(&self) -> String {
        let provider = self.provider.split(':').next().unwrap_or(&self.provider);
        if self.method == Method::AtsSearch {
            self.method.as_str().to_owned()
        } else {
            format!("{}:{provider}", self.method.as_str())
        }
    }

    fn same_as(&self, other: &Self) -> bool {
        self.method == other.method
            && self.provider == other.provider
            && self.query == other.query
            && self.discovered_url == other.discovered_url
            && self.derived == other.derived
    }
}

/// What a provider found: a URL and/or a company domain, with hints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lead {
    pub url: Option<String>,
    pub domain: Option<String>,
    pub company: Option<String>,
    pub title: Option<String>,
    pub sighting: Sighting,
}

impl Lead {
    pub fn url(url: &str, sighting: Sighting) -> Self {
        Self {
            url: Some(url.to_owned()),
            domain: None,
            company: None,
            title: sighting.title.clone(),
            sighting,
        }
    }

    pub fn domain(domain: &str, sighting: Sighting) -> Self {
        Self {
            url: None,
            domain: Some(domain.to_owned()),
            company: None,
            title: None,
            sighting,
        }
    }

    pub fn with_company(mut self, company: Option<String>) -> Self {
        self.company = company.filter(|c| !c.trim().is_empty());
        self
    }

    pub fn with_domain(mut self, domain: Option<String>) -> Self {
        self.domain = domain;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateKind {
    /// A posting on a supported board.
    Job,
    /// A supported board.
    Board,
    /// A company, by domain.
    Company,
    /// A company on a known ATS without an adapter.
    UnsupportedAts,
    /// A page that is neither (an aggregator's copy of a job).
    Page,
}

impl CandidateKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Job => "job",
            Self::Board => "board",
            Self::Company => "company",
            Self::UnsupportedAts => "unsupported_ats",
            Self::Page => "page",
        }
    }
}

/// Where a candidate stands. Which statuses apply depends on the kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateStatus {
    /// Not looked at yet.
    New,
    /// Board: read, owned by the company it names, worth registering.
    Validated,
    /// Board: nothing conclusive yet (ownership unproven, a transient
    /// failure). Kept for a recheck.
    Inconclusive,
    /// Board: read and rejected (gone, empty, stale-only, unstable ids).
    Rejected,
    /// Job: on its board's current listing.
    Live,
    /// Job: not on its board's current listing (closed, or a stale index
    /// entry), or its board is gone.
    Closed,
    /// Job: its board could not be read.
    Unknown,
    /// Company: at least one supported board found.
    Resolved,
    /// Company: uses a known ATS without an adapter.
    UnsupportedAts,
    /// Company: careers page without a recognizable board.
    CustomPage,
    /// Company: no careers page found.
    NoCareersPage,
    /// Company: its site refused automated requests.
    Blocked,
    /// Company: its site could not be reached.
    Unreachable,
    /// Page: not a company's page or an ATS (an aggregator).
    Ignored,
}

impl CandidateStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Validated => "validated",
            Self::Inconclusive => "inconclusive",
            Self::Rejected => "rejected",
            Self::Live => "live",
            Self::Closed => "closed",
            Self::Unknown => "unknown",
            Self::Resolved => "resolved",
            Self::UnsupportedAts => "unsupported_ats",
            Self::CustomPage => "custom_page",
            Self::NoCareersPage => "no_careers_page",
            Self::Blocked => "blocked",
            Self::Unreachable => "unreachable",
            Self::Ignored => "ignored",
        }
    }
}

/// How sure Narrow is that a board belongs to the company it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ownership {
    /// The company's own site links, embeds or redirects to the board.
    Verified,
    /// A source independent of the board (a hiring post, a directory) names
    /// the company's domain, and the board's postings tie to it.
    Corroborated,
    /// Only the board itself names the company's website; the company's
    /// site does not point back (a JavaScript careers page, a blocked site).
    Claimed,
    /// The company's site points at another board.
    Elsewhere,
    /// No company domain could be found.
    Unknown,
}

impl Ownership {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::Corroborated => "corroborated",
            Self::Claimed => "claimed",
            Self::Elsewhere => "elsewhere",
            Self::Unknown => "unknown",
        }
    }

    /// Strong enough to validate a board.
    pub fn established(self) -> bool {
        matches!(self, Self::Verified | Self::Corroborated)
    }
}

/// What reading a board found.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoardResult {
    /// `ok`, `not_found`, `empty`, `failed` or `transient`.
    pub listing: String,
    pub jobs: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub company: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub careers_url: Option<String>,
    /// Every domain the board suggested, best first, and where from.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hints: Vec<resolve::DomainHint>,
    pub ownership: Ownership,
    /// Boards the company's site points at instead of this one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub points_at: Vec<String>,
    pub validation: Validation,
    /// Measured by the app layer (job function, geography, eligibility).
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "yield")]
    pub yields: Option<SourceYield>,
    /// Unmet activation conditions, for a validated board.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub activation: Vec<String>,
    /// Whether ownership was looked into (the company's site read), as
    /// opposed to skipped or timed out.
    #[serde(default = "yes")]
    pub ownership_checked: bool,
}

fn yes() -> bool {
    true
}

/// What a job's board says about the job now.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The source's own publish date.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub posted_at: Option<DateTime<Utc>>,
    /// When discovery first saw it (not when it was posted).
    pub first_seen_at: DateTime<Utc>,
    #[serde(default)]
    pub engineering: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geo: Option<GeoBucket>,
    /// The reference profile's eligibility: `open`, `unclear` or `closed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brazil: Option<String>,
}

/// What looking at a company found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompanyResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub careers_url: Option<String>,
    /// Boards found, as source keys.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub boards: Vec<String>,
    /// Known ATS without an adapter, by provider.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub other_ats: Vec<String>,
    /// The sitemap / JSON-LD look, when it ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface: Option<sitemap::SurfaceSummary>,
}

/// One deduplicated thing discovery found.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    /// Stable id derived from the key.
    pub id: String,
    /// The dedupe key (`job:ashby:moxie:<uuid>`).
    pub key: String,
    pub kind: CandidateKind,
    /// The canonical URL (the board's or the job's own).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board: Option<BoardId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    /// An unsupported ATS's account name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub company_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_hint: Option<String>,
    pub first_discovered_at: DateTime<Utc>,
    pub sightings: Vec<Sighting>,
    pub status: CandidateStatus,
    /// Why it has its status (a rejection reason, a closed job).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_checked_at: Option<DateTime<Utc>>,
    /// The source key this resolved to, and its registry status when the
    /// registry already lists it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board_result: Option<BoardResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_result: Option<JobResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub company_result: Option<CompanyResult>,
}

impl Candidate {
    fn new(key: String, kind: CandidateKind, at: DateTime<Utc>) -> Self {
        Self {
            id: queries::stable_id(&key)[..12].to_owned(),
            key,
            kind,
            url: None,
            provider: None,
            board: None,
            job_id: None,
            tenant: None,
            company_hint: None,
            domain_hint: None,
            title_hint: None,
            first_discovered_at: at,
            sightings: Vec::new(),
            status: if kind == CandidateKind::Page {
                CandidateStatus::Ignored
            } else {
                CandidateStatus::New
            },
            reason: None,
            last_checked_at: None,
            source: None,
            registry_status: None,
            board_result: None,
            job_result: None,
            company_result: None,
        }
    }

    /// The board's key (`board:ashby:moxie`) for a job or board.
    pub fn board_key(&self) -> Option<String> {
        self.board.as_ref().map(|b| format!("board:{}", b.key()))
    }

    /// Hits of its own (not derived sightings).
    pub fn hits(&self) -> impl Iterator<Item = &Sighting> {
        self.sightings.iter().filter(|s| !s.derived)
    }

    fn sight(&mut self, sighting: Sighting) -> bool {
        if self.sightings.iter().any(|s| s.same_as(&sighting)) {
            return false;
        }
        if sighting.discovered_at < self.first_discovered_at {
            self.first_discovered_at = sighting.discovered_at;
        }
        self.sightings.push(sighting);
        true
    }

    fn hint(&mut self, company: Option<&str>, domain: Option<&str>, title: Option<&str>) {
        fill(&mut self.company_hint, company);
        fill(&mut self.domain_hint, domain);
        fill(&mut self.title_hint, title);
    }
}

fn fill(slot: &mut Option<String>, value: Option<&str>) {
    if slot.is_none()
        && let Some(v) = value.map(str::trim).filter(|v| !v.is_empty())
    {
        *slot = Some(v.chars().take(200).collect());
    }
}

/// What [`DiscoveryStore::add`] did with a lead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Added {
    pub key: String,
    /// A candidate that did not exist before.
    pub new: bool,
    /// A sighting not already recorded.
    pub new_sighting: bool,
}

/// The candidate store: every candidate with its sightings and results.
/// A file (`discovery.json`) for now; see docs/broad-discovery.md for the
/// database design of the continuous version.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DiscoveryStore {
    #[serde(default = "version")]
    pub version: u32,
    pub candidates: Vec<Candidate>,
    #[serde(skip)]
    index: HashMap<String, usize>,
}

fn version() -> u32 {
    1
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid discovery store: {0}")]
pub struct StoreError(pub String);

impl DiscoveryStore {
    pub fn new() -> Self {
        Self {
            version: version(),
            ..Self::default()
        }
    }

    pub fn parse(text: &str) -> Result<Self, StoreError> {
        let mut store: Self = serde_json::from_str(text).map_err(|e| StoreError(e.to_string()))?;
        store.reindex();
        Ok(store)
    }

    /// One candidate per line: compact, and still easy to grep and diff.
    pub fn to_json(&self) -> String {
        let mut out = format!("{{\"version\": {}, \"candidates\": [\n", self.version);
        for (i, c) in self.candidates.iter().enumerate() {
            if i > 0 {
                out.push_str(",\n");
            }
            out.push_str(&serde_json::to_string(c).unwrap_or_default());
        }
        out.push_str("\n]}\n");
        out
    }

    fn reindex(&mut self) {
        self.index = self
            .candidates
            .iter()
            .enumerate()
            .map(|(i, c)| (c.key.clone(), i))
            .collect();
    }

    pub fn get(&self, key: &str) -> Option<&Candidate> {
        self.index.get(key).map(|&i| &self.candidates[i])
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Candidate> {
        self.index.get(key).map(|&i| &mut self.candidates[i])
    }

    pub fn len(&self) -> usize {
        self.candidates.len()
    }

    pub fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }

    fn entry(
        &mut self,
        key: String,
        kind: CandidateKind,
        at: DateTime<Utc>,
    ) -> (&mut Candidate, bool) {
        if let Some(&i) = self.index.get(&key) {
            return (&mut self.candidates[i], false);
        }
        self.index.insert(key.clone(), self.candidates.len());
        self.candidates.push(Candidate::new(key, kind, at));
        let last = self.candidates.len() - 1;
        (&mut self.candidates[last], true)
    }

    /// Files a lead under its dedupe key, merging it into an existing
    /// candidate when there is one. A job also files a derived sighting on
    /// its board. A URL that is neither an ATS nor a company's own page
    /// (an aggregator's copy) is kept as an ignored page, unless the lead
    /// names the company's domain, which then becomes the candidate.
    pub fn add(&mut self, lead: Lead) -> Option<Added> {
        let at = lead.sighting.discovered_at;
        let url = lead.url.as_deref().and_then(|u| Url::parse(u.trim()).ok());
        let target = url.as_ref().map(ats::classify);
        let lead_domain = lead
            .domain
            .as_deref()
            .and_then(ats::registrable_domain)
            .filter(|d| ats::is_company_domain(d));
        match target {
            Some(UrlTarget::Job { board, job_id }) => {
                let id = BoardId::of(&board);
                let key = format!("job:{}:{job_id}", id.key());
                let (c, new) = self.entry(key.clone(), CandidateKind::Job, at);
                c.url = Some(ats::job_url(&board, &job_id));
                c.provider = Some(id.provider.clone());
                c.board = Some(id.clone());
                c.job_id = Some(job_id);
                c.hint(
                    lead.company.as_deref(),
                    lead_domain.as_deref(),
                    lead.title.as_deref(),
                );
                let new_sighting = c.sight(lead.sighting.clone());
                self.add_board(&board, &lead, lead_domain.as_deref(), true);
                Some(Added {
                    key,
                    new,
                    new_sighting,
                })
            }
            Some(UrlTarget::Board(board)) => {
                let key = self.add_board(&board, &lead, lead_domain.as_deref(), false);
                Some(key)
            }
            Some(UrlTarget::Unsupported { provider, tenant }) => {
                let who = tenant
                    .clone()
                    .or_else(|| lead_domain.clone())
                    .or_else(|| {
                        url.as_ref()
                            .map(|u| u.host_str().unwrap_or_default().to_owned())
                    })
                    .unwrap_or_default();
                let key = format!("ats:{provider}:{who}");
                let (c, new) = self.entry(key.clone(), CandidateKind::UnsupportedAts, at);
                c.url = url
                    .as_ref()
                    .map(|u| format!("{}://{}/", u.scheme(), u.host_str().unwrap_or_default()));
                c.provider = Some(provider.to_owned());
                c.tenant = tenant;
                c.status = CandidateStatus::UnsupportedAts;
                c.hint(
                    lead.company.as_deref(),
                    lead_domain.as_deref(),
                    lead.title.as_deref(),
                );
                let new_sighting = c.sight(lead.sighting.clone());
                // The company itself is still worth a look: its careers
                // page may also link a supported board.
                if let Some(domain) = &lead_domain {
                    self.add_company(domain, &lead, true);
                }
                Some(Added {
                    key,
                    new,
                    new_sighting,
                })
            }
            Some(UrlTarget::Page) => {
                let domain = lead_domain.clone().or_else(|| {
                    url.as_ref()
                        .and_then(ats::company_domain_of)
                        .filter(|_| lead.sighting.method != Method::AtsSearch)
                });
                match domain {
                    Some(domain) => Some(self.add_company(&domain, &lead, false)),
                    None => {
                        let canonical = jobhunt_core::CanonicalUrl::parse(url.as_ref()?.as_str())
                            .ok()?
                            .into_string();
                        let key = format!("page:{canonical}");
                        let (c, new) = self.entry(key.clone(), CandidateKind::Page, at);
                        c.url = Some(canonical);
                        c.reason = Some("neither an ATS nor a company's own page".into());
                        c.hint(lead.company.as_deref(), None, lead.title.as_deref());
                        let new_sighting = c.sight(lead.sighting);
                        Some(Added {
                            key,
                            new,
                            new_sighting,
                        })
                    }
                }
            }
            None => {
                let domain = lead_domain?;
                Some(self.add_company(&domain, &lead, false))
            }
        }
    }

    fn add_board(
        &mut self,
        board: &crate::careers::BoardRef,
        lead: &Lead,
        domain: Option<&str>,
        derived: bool,
    ) -> Added {
        let id = BoardId::of(board);
        let key = format!("board:{}", id.key());
        let (c, new) = self.entry(
            key.clone(),
            CandidateKind::Board,
            lead.sighting.discovered_at,
        );
        c.url = Some(ats::board_url(board));
        c.provider = Some(id.provider.clone());
        c.board = Some(id);
        c.source = ats::source_key(board);
        c.hint(lead.company.as_deref(), domain, None);
        let mut sighting = lead.sighting.clone();
        sighting.derived |= derived;
        let new_sighting = c.sight(sighting);
        Added {
            key,
            new,
            new_sighting,
        }
    }

    fn add_company(&mut self, domain: &str, lead: &Lead, derived: bool) -> Added {
        let key = format!("company:{domain}");
        let (c, new) = self.entry(
            key.clone(),
            CandidateKind::Company,
            lead.sighting.discovered_at,
        );
        c.url = Some(format!("https://{domain}/"));
        c.domain_hint = Some(domain.to_owned());
        c.hint(lead.company.as_deref(), None, None);
        let mut sighting = lead.sighting.clone();
        sighting.derived |= derived;
        let new_sighting = c.sight(sighting);
        Added {
            key,
            new,
            new_sighting,
        }
    }

    /// Files a board found while resolving another candidate (a company's
    /// careers page, a sitemap): it inherits the sightings of `from`, marked
    /// derived, so its yield counts toward the strategies that led to it.
    pub fn add_derived_board(
        &mut self,
        board: &crate::careers::BoardRef,
        from: &str,
        via: &str,
        at: DateTime<Utc>,
    ) -> Option<Added> {
        let parent = self.get(from)?.clone();
        let mut out = None;
        // The parent's own hits; a parent that is itself derived passes on
        // what it was derived from.
        let inherited: Vec<&Sighting> = if parent.hits().next().is_some() {
            parent.hits().collect()
        } else {
            parent.sightings.iter().collect()
        };
        for s in inherited {
            let mut sighting = s.clone();
            sighting.metadata.insert("via".into(), via.to_owned());
            let lead = Lead {
                url: None,
                domain: parent.domain_hint.clone(),
                company: parent.company_hint.clone(),
                title: None,
                sighting: Sighting {
                    discovered_at: s.discovered_at.min(at),
                    ..sighting
                },
            };
            out = Some(self.add_board(board, &lead, parent.domain_hint.as_deref(), true));
        }
        out
    }

    /// Candidates of a kind, in store order.
    pub fn of_kind(&self, kind: CandidateKind) -> impl Iterator<Item = &Candidate> {
        self.candidates.iter().filter(move |c| c.kind == kind)
    }

    /// The jobs filed under a board key.
    pub fn jobs_of(&self, board_key: &str) -> Vec<String> {
        self.of_kind(CandidateKind::Job)
            .filter(|c| c.board_key().as_deref() == Some(board_key))
            .map(|c| c.key.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn at(minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 5, 12, minute, 0).unwrap()
    }

    fn search(query: &str, url: &str, minute: u32) -> Lead {
        let mut s = Sighting::new(Method::AtsSearch, "websearch", url, at(minute));
        s.query = Some(query.to_owned());
        Lead::url(url, s)
    }

    const JOB: &str = "https://jobs.ashbyhq.com/moxie/36c5bcce-5a3d-4394-9eeb-68d812f9e753";

    #[test]
    fn the_same_job_from_many_queries_is_one_candidate() {
        let mut store = DiscoveryStore::new();
        let queries = (0..12).map(|i| format!("site:jobs.ashbyhq.com \"engineer {i}\" remote"));
        for (i, q) in queries.enumerate() {
            let url = if i % 2 == 0 {
                JOB.to_owned()
            } else {
                format!("{JOB}/application?utm_source=search")
            };
            store.add(search(&q, &url, 10 + i as u32)).unwrap();
        }
        let jobs: Vec<_> = store.of_kind(CandidateKind::Job).collect();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].sightings.len(), 12);
        assert_eq!(jobs[0].url.as_deref(), Some(JOB));
        assert_eq!(jobs[0].first_discovered_at, at(10));
        // One board, every sighting derived from the job.
        let boards: Vec<_> = store.of_kind(CandidateKind::Board).collect();
        assert_eq!(boards.len(), 1);
        assert_eq!(boards[0].source.as_deref(), Some("ashby:moxie"));
        assert_eq!(boards[0].hits().count(), 0);
        assert_eq!(store.len(), 2);

        // The same query and URL again is not a new sighting.
        let again = store
            .add(search(
                "site:jobs.ashbyhq.com \"engineer 0\" remote",
                JOB,
                50,
            ))
            .unwrap();
        assert!(!again.new && !again.new_sighting);
    }

    #[test]
    fn many_jobs_of_one_board_are_one_board() {
        let mut store = DiscoveryStore::new();
        for (i, id) in [
            "11111111-1111-1111-1111-111111111111",
            "22222222-2222-2222-2222-222222222222",
            "33333333-3333-3333-3333-333333333333",
        ]
        .iter()
        .enumerate()
        {
            store
                .add(search(
                    "q",
                    &format!("https://jobs.lever.co/acme/{id}"),
                    i as u32,
                ))
                .unwrap();
        }
        store
            .add(search("q2", "https://jobs.lever.co/acme", 9))
            .unwrap();
        assert_eq!(store.of_kind(CandidateKind::Job).count(), 3);
        let board = store.get("board:lever:acme").unwrap();
        assert_eq!(board.sightings.len(), 4);
        assert_eq!(board.hits().count(), 1, "the board URL itself is a hit");
        assert_eq!(store.jobs_of("board:lever:acme").len(), 3);
    }

    #[test]
    fn one_company_from_several_methods_keeps_every_provenance() {
        let mut store = DiscoveryStore::new();
        let hn = Lead::url(
            "https://www.acme.com/careers",
            Sighting::new(
                Method::HiringThread,
                "hn:1",
                "https://www.acme.com/careers",
                at(1),
            ),
        );
        store.add(hn).unwrap();
        let yc = Lead::domain(
            "acme.com",
            Sighting::new(
                Method::CompanyDirectory,
                "yc-oss",
                "https://acme.com",
                at(2),
            ),
        )
        .with_company(Some("Acme".into()));
        store.add(yc).unwrap();
        let company = store.get("company:acme.com").unwrap();
        assert_eq!(company.sightings.len(), 2);
        assert_eq!(company.company_hint.as_deref(), Some("Acme"));
        let strategies: Vec<String> = company.sightings.iter().map(Sighting::strategy).collect();
        assert_eq!(strategies, ["hiring_thread:hn", "company_directory:yc-oss"]);
    }

    #[test]
    fn provenance_is_retained() {
        let mut store = DiscoveryStore::new();
        let mut lead = search("site:jobs.ashbyhq.com \"platform engineer\" LATAM", JOB, 3);
        lead.sighting.family = Some("platform engineer · latam".into());
        lead.sighting.rank = Some(4);
        store.add(lead).unwrap();
        let text = store.to_json();
        let back = DiscoveryStore::parse(&text).unwrap();
        let job = back
            .get(&format!(
                "job:ashby:moxie:{}",
                "36c5bcce-5a3d-4394-9eeb-68d812f9e753"
            ))
            .unwrap();
        let s = &job.sightings[0];
        assert_eq!(s.method, Method::AtsSearch);
        assert_eq!(
            s.query.as_deref(),
            Some("site:jobs.ashbyhq.com \"platform engineer\" LATAM")
        );
        assert_eq!(s.discovered_url, JOB);
        assert_eq!(s.discovered_at, at(3));
        assert_eq!(s.rank, Some(4));
        assert_eq!(back, store);
    }

    #[test]
    fn aggregator_copies_and_unsupported_ats_are_kept_apart() {
        let mut store = DiscoveryStore::new();
        // A search hit on an aggregator: kept, ignored, never a company.
        store
            .add(search(
                "q",
                "https://arc.dev/remote-jobs/j/acme-staff-platform-engineer",
                1,
            ))
            .unwrap();
        let page = store.of_kind(CandidateKind::Page).next().unwrap();
        assert_eq!(page.status, CandidateStatus::Ignored);
        assert_eq!(store.of_kind(CandidateKind::Company).count(), 0);
        // Two Workday URLs of one tenant: one unsupported-ATS candidate.
        for path in ["en-US/External/job/A_1", "en-US/External/job/B_2"] {
            store
                .add(search(
                    "q",
                    &format!("https://acme.wd5.myworkdayjobs.com/{path}"),
                    2,
                ))
                .unwrap();
        }
        let ats: Vec<_> = store.of_kind(CandidateKind::UnsupportedAts).collect();
        assert_eq!(ats.len(), 1);
        assert_eq!(ats[0].key, "ats:workday:acme");
        assert_eq!(ats[0].sightings.len(), 2);
    }

    #[test]
    fn derived_boards_inherit_the_strategies_that_led_to_them() {
        let mut store = DiscoveryStore::new();
        let mut s = Sighting::new(
            Method::CompanyDirectory,
            "yc-oss",
            "https://acme.com",
            at(1),
        );
        s.metadata.insert("regions".into(), "Remote".into());
        store.add(Lead::domain("acme.com", s)).unwrap();
        let board = crate::careers::BoardRef {
            kind: "greenhouse",
            name: "acme".into(),
            lever_region: crate::lever::LeverRegion::Global,
        };
        store
            .add_derived_board(&board, "company:acme.com", "careers page", at(5))
            .unwrap();
        let b = store.get("board:greenhouse:acme").unwrap();
        assert_eq!(b.sightings.len(), 1);
        assert!(b.sightings[0].derived);
        assert_eq!(b.sightings[0].strategy(), "company_directory:yc-oss");
        assert_eq!(b.domain_hint.as_deref(), Some("acme.com"));
        assert_eq!(
            b.sightings[0].metadata.get("via").map(String::as_str),
            Some("careers page")
        );

        // A company that is itself derived (named next to an unsupported
        // ATS link) still passes its provenance on.
        let mut s = Sighting::new(
            Method::HiringThread,
            "hn:1",
            "https://apply.workable.com/beta/j/1",
            at(2),
        );
        s.title = Some("Beta | Engineer".into());
        store
            .add(
                Lead::url("https://apply.workable.com/beta/j/1", s)
                    .with_domain(Some("beta.io".into())),
            )
            .unwrap();
        assert_eq!(store.get("company:beta.io").unwrap().hits().count(), 0);
        let lever = crate::careers::BoardRef {
            kind: "lever",
            name: "beta".into(),
            lever_region: crate::lever::LeverRegion::Global,
        };
        store
            .add_derived_board(&lever, "company:beta.io", "careers page", at(6))
            .unwrap();
        assert_eq!(
            store.get("board:lever:beta").unwrap().sightings[0].strategy(),
            "hiring_thread:hn"
        );
    }
}
