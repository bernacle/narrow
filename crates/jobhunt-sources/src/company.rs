//! From a company to its job board.
//!
//! The source list grows from companies, not from boards: given a
//! company's domain, [`discover`] looks for its careers page the way a
//! person would, finds the hosted ATS board behind it, and reads that
//! board through its regular adapter to check it:
//!
//! 1. the homepage, for board links and links to a careers page;
//! 2. those careers links, then the usual places (`/careers`, `/jobs`,
//!    `careers.<domain>`, …) until a board is found;
//! 3. on each page, references to a supported board
//!    ([`careers::find_boards`]) and to known ATS without an adapter
//!    ([`careers::find_other_ats`]);
//! 4. optionally, when no page names a board, the company's own name as a
//!    board slug on each supported ATS ([`BoardEvidence::SlugGuess`]):
//!    only accepted when the board's postings name the company;
//! 5. every board found is read with its adapter ([`BoardCheck`]): a URL
//!    pattern alone is never trusted.
//!
//! Discovery reads public pages only, at most [`ProbeSettings::max_pages`]
//! per company, through the shared HTTP client (its per-host limits and
//! retry policy). It runs no JavaScript and does not work around anything:
//! a page that answers 403 or 429 is recorded as blocked and left alone. A
//! careers page whose jobs only appear after JavaScript runs looks like a
//! custom page here; the slug guess is what can still find its board.

use std::collections::HashSet;
use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use jobhunt_core::{ErrorChain, FetchRequest, Fetched, SourceError};
use jobhunt_jobs::JobPosting;
use serde::Serialize;
use url::Url;

use crate::careers::{self, BoardRef, OtherAts};
use crate::http::HttpClient;
use crate::lever::LeverRegion;
use crate::registry::Freshness;
use crate::{SourceSpec, ashby, greenhouse, lever};

/// A company to look for: its domain, and optionally its display name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompanyTarget {
    /// Lowercase host without `www.`, e.g. `railway.com`.
    pub domain: String,
    pub name: Option<String>,
}

impl CompanyTarget {
    /// The display name: the given one, else the domain's first label.
    pub fn display_name(&self) -> String {
        self.name.clone().unwrap_or_else(|| self.stem().to_owned())
    }

    /// The domain's first label (`railway` for `railway.com`).
    pub fn stem(&self) -> &str {
        self.domain.split('.').next().unwrap_or(&self.domain)
    }

    fn homepage(&self) -> Option<Url> {
        Url::parse(&format!("https://{}/", self.domain)).ok()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "{0:?} is not a company domain (expected e.g. \"railway.com\" or \"railway.com, Railway\")"
)]
pub struct InvalidTarget(pub String);

impl FromStr for CompanyTarget {
    type Err = InvalidTarget;

    /// `domain`, `domain, Name` or a URL on the company's site.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let invalid = || InvalidTarget(s.to_owned());
        let (domain, name) = match s.split_once(',') {
            Some((d, n)) => (d.trim(), Some(n.trim()).filter(|n| !n.is_empty())),
            None => (s.trim(), None),
        };
        let without_scheme = domain
            .split_once("://")
            .map_or(domain, |(_, rest)| rest)
            .to_ascii_lowercase();
        let host = without_scheme
            .split(['/', '?', '#'])
            .next()
            .unwrap_or_default();
        let host = host.split(':').next().unwrap_or_default();
        let host = host.strip_prefix("www.").unwrap_or(host);
        let valid = host.contains('.')
            && !host.starts_with('.')
            && !host.ends_with('.')
            && host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.'));
        if !valid {
            return Err(invalid());
        }
        Ok(Self {
            domain: host.to_owned(),
            name: name.map(str::to_owned),
        })
    }
}

impl fmt::Display for CompanyTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.name {
            Some(name) => write!(f, "{} ({name})", self.domain),
            None => f.write_str(&self.domain),
        }
    }
}

/// Reads a list of companies: one per line, `domain` or `domain, Name`;
/// blank lines and `#` comments are skipped. Errors name the line.
pub fn parse_targets(text: &str) -> Result<Vec<CompanyTarget>, String> {
    let mut out: Vec<CompanyTarget> = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        let target: CompanyTarget = line
            .parse()
            .map_err(|e: InvalidTarget| format!("line {}: {e}", n + 1))?;
        if !out.iter().any(|t| t.domain == target.domain) {
            out.push(target);
        }
    }
    Ok(out)
}

/// How a board was tied to the company.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BoardEvidence {
    /// A page of the company's site redirected to the board itself.
    Redirect,
    /// The company's careers page links to or embeds the board.
    CareersPage,
    /// The company's homepage links to or embeds the board.
    Homepage,
    /// No page named a board; the company's name is a board slug on the
    /// ATS. Weak: only the board's own postings can tie it to the company.
    SlugGuess,
    /// Found by broad discovery (a search hit, a hiring post, a crawl),
    /// not through the company's site. As weak as a guess until the
    /// company's site is shown to point at it.
    Discovered,
}

impl BoardEvidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Redirect => "redirect",
            Self::CareersPage => "careers_page",
            Self::Homepage => "homepage",
            Self::SlugGuess => "slug_guess",
            Self::Discovered => "discovered",
        }
    }

    /// The company's own site points at the board.
    pub fn first_party(self) -> bool {
        !matches!(self, Self::SlugGuess | Self::Discovered)
    }
}

/// A supported board found for a company.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FoundBoard {
    /// Source key, e.g. `ashby:railway`.
    pub source: String,
    #[serde(skip)]
    pub board: BoardRef,
    pub evidence: BoardEvidence,
    /// The page it was found on.
    pub page: Option<String>,
    /// The slug resembles the company's name or domain. A careers page
    /// that links several boards (a portfolio page, a parent company)
    /// makes the others suspect.
    pub matches_company: bool,
}

/// One page discovery read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PageVisit {
    pub url: String,
    /// HTTP status, `None` when the request failed.
    pub status: Option<u16>,
    /// Where redirects ended, when elsewhere.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// What discovery concluded about a company.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeOutcome {
    /// At least one supported board was found.
    Board,
    /// The careers page uses a known ATS without an adapter.
    UnsupportedAts,
    /// A careers page exists, with no recognizable board (a custom site, or
    /// one that loads its jobs with JavaScript).
    CustomPage,
    /// The site answered, but no careers page was found.
    NoCareersPage,
    /// The site refused automated requests (403/429).
    Blocked,
    /// The site could not be reached.
    Unreachable,
}

impl ProbeOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Board => "board",
            Self::UnsupportedAts => "unsupported_ats",
            Self::CustomPage => "custom_page",
            Self::NoCareersPage => "no_careers_page",
            Self::Blocked => "blocked",
            Self::Unreachable => "unreachable",
        }
    }
}

/// How far discovery goes for one company.
#[derive(Debug, Clone)]
pub struct ProbeSettings {
    /// Pages of the company's site to read at most.
    pub max_pages: usize,
    /// Try the company's name as a board slug when no page names a board.
    pub guess_slugs: bool,
    /// Sends ATS API requests here instead of the real hosts (tests).
    pub api_base: Option<String>,
    /// Reads the company's site here instead of `https://<domain>/`
    /// (tests).
    pub site_base: Option<Url>,
}

impl Default for ProbeSettings {
    fn default() -> Self {
        Self {
            max_pages: 8,
            guess_slugs: true,
            api_base: None,
            site_base: None,
        }
    }
}

/// Everything discovery found for one company.
#[derive(Debug, Clone, Serialize)]
pub struct CompanyProbe {
    pub company: String,
    pub domain: String,
    pub outcome: ProbeOutcome,
    /// The company's careers page, when one was found.
    pub careers_url: Option<String>,
    pub boards: Vec<FoundBoard>,
    /// Known ATS without an adapter referenced by the pages.
    pub other_ats: Vec<OtherAts>,
    pub pages: Vec<PageVisit>,
    /// Every board found, read through its adapter.
    pub checks: Vec<BoardCheck>,
}

/// The paths a careers page usually lives at, tried in order after the
/// homepage's own careers links.
const CAREERS_PATHS: &[&str] = &[
    "careers",
    "jobs",
    "company/careers",
    "about/careers",
    "join-us",
];

/// Finds the careers page and boards of a company and checks each board
/// with its adapter. Never fails: what went wrong is part of the result.
pub async fn discover(
    http: &HttpClient,
    target: &CompanyTarget,
    settings: &ProbeSettings,
    now: DateTime<Utc>,
) -> CompanyProbe {
    let mut probe = CompanyProbe {
        company: target.display_name(),
        domain: target.domain.clone(),
        outcome: ProbeOutcome::Unreachable,
        careers_url: None,
        boards: Vec::new(),
        other_ats: Vec::new(),
        pages: Vec::new(),
        checks: Vec::new(),
    };
    let Some(home) = settings.site_base.clone().or_else(|| target.homepage()) else {
        return probe;
    };
    let site = home.host_str().unwrap_or(&target.domain).to_owned();
    let site = site.strip_prefix("www.").unwrap_or(&site).to_owned();

    // Pages to read, in order, with whether each is a careers page.
    let mut queue: Vec<(Url, bool)> = vec![(home.clone(), false)];
    let mut visited: HashSet<String> = HashSet::new();
    let mut home_status = None;
    let mut conventional_added = false;
    // The careers page is the one a board or ATS was found through, else
    // the first page that looks like one.
    let mut careers_confirmed = false;
    let mut i = 0;
    while i < queue.len() && probe.pages.len() < settings.max_pages {
        let (url, careers_candidate) = queue[i].clone();
        i += 1;
        if !visited.insert(url.to_string()) {
            continue;
        }
        let visit = http.probe(&url).await;
        let is_home = probe.pages.is_empty();
        let mut page = PageVisit {
            url: url.to_string(),
            status: None,
            final_url: None,
            error: None,
        };
        match visit {
            Err(e) => page.error = Some(ErrorChain(&e).to_string()),
            Ok(answer) => {
                page.status = Some(answer.status);
                if answer.final_url != url {
                    page.final_url = Some(answer.final_url.to_string());
                    visited.insert(answer.final_url.to_string());
                }
                let ok = (200..300).contains(&answer.status);
                let html = String::from_utf8_lossy(&answer.body);
                let evidence = if careers_candidate {
                    BoardEvidence::CareersPage
                } else {
                    BoardEvidence::Homepage
                };
                let mut confirm = |probe: &mut CompanyProbe, at: &Url| {
                    if !careers_confirmed {
                        careers_confirmed = true;
                        probe.careers_url = Some(at.to_string());
                    }
                };
                // A careers URL that lands on the board itself.
                if let Some(board) = careers::board_for_url(&answer.final_url) {
                    add_board(&mut probe, target, board, BoardEvidence::Redirect, &url);
                    confirm(&mut probe, &url);
                } else if let Some(provider) = careers::other_ats_for_url(&answer.final_url) {
                    push_other(&mut probe.other_ats, provider, &answer.final_url);
                    confirm(&mut probe, &url);
                } else if ok {
                    let boards_before = probe.boards.len();
                    let others_before = probe.other_ats.len();
                    for board in careers::find_boards(&html) {
                        add_board(&mut probe, target, board, evidence, &answer.final_url);
                    }
                    for other in careers::find_other_ats(&html) {
                        if !probe.other_ats.iter().any(|o| o.provider == other.provider) {
                            probe.other_ats.push(other);
                        }
                    }
                    let found_here =
                        probe.boards.len() > boards_before || probe.other_ats.len() > others_before;
                    // Judged by where the page ended up: a careers URL that
                    // redirects to the homepage is not a careers page (one
                    // that redirects to another company's careers page is
                    // recorded as such).
                    if careers_candidate && found_here {
                        confirm(&mut probe, &answer.final_url);
                    } else if careers_candidate && is_careers_url(&answer.final_url) {
                        probe
                            .careers_url
                            .get_or_insert_with(|| answer.final_url.to_string());
                    }
                    // Only the homepage's links: a careers page's own links
                    // lead to its job pages, which are the board's business.
                    if is_home {
                        for link in careers::careers_links(&html, &answer.final_url, &site)
                            .into_iter()
                            .take(3)
                        {
                            if !queue.iter().any(|(u, _)| *u == link) {
                                queue.push((link, true));
                            }
                        }
                    }
                }
                if is_home {
                    home_status = Some(answer.status);
                }
            }
        }
        probe.pages.push(page);
        if probe.boards.iter().any(|b| b.matches_company) {
            break;
        }
        // Once the homepage's own links are read, try the usual places.
        if i == queue.len() && !conventional_added && probe.boards.is_empty() {
            conventional_added = true;
            for path in CAREERS_PATHS {
                if let Ok(url) = home.join(path) {
                    queue.push((url, true));
                }
            }
            if settings.site_base.is_none() {
                for sub in ["careers", "jobs"] {
                    if let Ok(url) = Url::parse(&format!("https://{sub}.{site}/")) {
                        queue.push((url, true));
                    }
                }
            }
        }
    }

    if probe.boards.is_empty() && settings.guess_slugs {
        guess_boards(http, target, settings, now, &mut probe).await;
    }
    for found in probe.boards.clone() {
        if found.evidence == BoardEvidence::SlugGuess {
            continue; // checked while guessing
        }
        let check = check_board(http, target, &found, settings.api_base.as_deref(), now).await;
        probe.checks.push(check);
    }

    probe.outcome = if !probe.boards.is_empty() {
        ProbeOutcome::Board
    } else if !probe.other_ats.is_empty() {
        ProbeOutcome::UnsupportedAts
    } else if probe.careers_url.is_some() {
        ProbeOutcome::CustomPage
    } else {
        match home_status {
            Some(403 | 429) => ProbeOutcome::Blocked,
            Some(_) => ProbeOutcome::NoCareersPage,
            None if probe.pages.iter().any(|p| p.status.is_some()) => ProbeOutcome::NoCareersPage,
            None => ProbeOutcome::Unreachable,
        }
    };
    probe
}

fn is_careers_url(url: &Url) -> bool {
    let host = url.host_str().unwrap_or_default();
    host.starts_with("careers.")
        || host.starts_with("jobs.")
        || url.path().to_ascii_lowercase().split('/').any(|s| {
            matches!(
                s,
                "careers" | "career" | "jobs" | "join-us" | "open-positions" | "open-roles"
            )
        })
}

fn push_other(found: &mut Vec<OtherAts>, provider: &'static str, url: &Url) {
    if !found.iter().any(|o| o.provider == provider) {
        found.push(OtherAts {
            provider,
            url: url.to_string(),
        });
    }
}

fn add_board(
    probe: &mut CompanyProbe,
    target: &CompanyTarget,
    board: BoardRef,
    evidence: BoardEvidence,
    page: &Url,
) {
    let Ok(spec) = SourceSpec::from_board(&board, None) else {
        return;
    };
    let source = spec.key().to_string();
    if probe.boards.iter().any(|b| b.source == source) {
        return;
    }
    probe.boards.push(FoundBoard {
        source,
        matches_company: slug_matches(&board.name, target),
        board,
        evidence,
        page: Some(page.to_string()),
    });
}

/// Lowercase ASCII letters and digits only.
fn squash(s: &str) -> String {
    s.chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase()
}

/// Whether a board slug resembles the company: one contains the other,
/// ignoring punctuation (`sourcegraph91` ~ `sourcegraph`, `convex-dev` ~
/// `convex`).
pub fn slug_matches(slug: &str, target: &CompanyTarget) -> bool {
    let slug = squash(slug);
    let names = [squash(target.stem()), squash(&target.display_name())];
    names
        .iter()
        .filter(|n| n.len() >= 3)
        .any(|n| slug.contains(n.as_str()) || (slug.len() >= 4 && n.contains(slug.as_str())))
}

/// Board slugs to guess: the domain's first label and the name, squashed
/// and hyphenated.
fn guess_slugs(target: &CompanyTarget) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let name = target.display_name().to_ascii_lowercase();
    let hyphenated = name
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    for slug in [target.stem().to_owned(), squash(&name), hyphenated] {
        if slug.len() >= 2 && !out.contains(&slug) {
            out.push(slug);
        }
    }
    out.truncate(2);
    out
}

async fn guess_boards(
    http: &HttpClient,
    target: &CompanyTarget,
    settings: &ProbeSettings,
    now: DateTime<Utc>,
    probe: &mut CompanyProbe,
) {
    for slug in guess_slugs(target) {
        for kind in [ashby::KIND, greenhouse::KIND, lever::KIND] {
            let board = BoardRef {
                kind,
                name: slug.clone(),
                lever_region: LeverRegion::Global,
            };
            let Ok(spec) = SourceSpec::from_board(&board, None) else {
                continue;
            };
            let found = FoundBoard {
                source: spec.key().to_string(),
                matches_company: true,
                board,
                evidence: BoardEvidence::SlugGuess,
                page: None,
            };
            let check = check_board(http, target, &found, settings.api_base.as_deref(), now).await;
            // A guess that doesn't exist or lists nothing is not a finding.
            if check.error.is_none() && check.jobs > 0 {
                probe.boards.push(found);
                probe.checks.push(check);
            }
        }
    }
}

/// A board read through its adapter, with what ties it to the company.
#[derive(Debug, Clone, Serialize)]
pub struct BoardCheck {
    pub source: String,
    pub evidence: BoardEvidence,
    /// Why the read failed, when it did.
    pub error: Option<String>,
    /// The failure is transient (timeout, 5xx, 429): try again later.
    pub transient: bool,
    pub jobs: usize,
    /// Postings the adapter could not convert.
    pub rejected: usize,
    /// The adapter vouched for the whole listing.
    pub complete: bool,
    /// Postings with the provider's own id (stable identity).
    pub with_provider_id: usize,
    /// The board's slug resembles the company's name or domain.
    pub matches_company: bool,
    /// Postings that name the company (in their company field or text).
    /// Weak evidence: a generic name ("Ghost") is also another company's.
    pub naming_company: usize,
    /// Postings tied to the company's domain: their URL is on it, or their
    /// text mentions it. Strong evidence.
    pub naming_domain: usize,
    /// The website the board itself names (Ashby boards publish one).
    pub website: Option<String>,
    /// That website is the company's (same domain or same first label:
    /// `railway.app` for `railway.com`). `None` when the board names none.
    pub website_matches: Option<bool>,
    /// The company names the postings carry (at most three).
    pub company_names: Vec<String>,
    pub freshness: Freshness,
    /// The postings themselves, for the caller to measure (eligibility,
    /// job function). Not serialized.
    #[serde(skip)]
    pub postings: Vec<JobPosting>,
}

/// Reads a found board through its adapter.
pub async fn check_board(
    http: &HttpClient,
    target: &CompanyTarget,
    found: &FoundBoard,
    api_base: Option<&str>,
    now: DateTime<Utc>,
) -> BoardCheck {
    let mut check = BoardCheck {
        source: found.source.clone(),
        evidence: found.evidence,
        error: None,
        transient: false,
        jobs: 0,
        rejected: 0,
        complete: false,
        with_provider_id: 0,
        matches_company: found.matches_company,
        naming_company: 0,
        naming_domain: 0,
        website: None,
        website_matches: None,
        company_names: Vec::new(),
        freshness: Freshness::default(),
        postings: Vec::new(),
    };
    // No configured company name: Greenhouse then reports its own, which
    // is evidence of ownership.
    let source = match SourceSpec::from_board(&found.board, None)
        .map_err(|e| e.to_string())
        .and_then(|spec| spec.build_at(http, api_base).map_err(|e| e.to_string()))
    {
        Ok(source) => source,
        Err(e) => {
            check.error = Some(e);
            return check;
        }
    };
    match source.fetch(&FetchRequest::default()).await {
        Err(e) => {
            check.transient = matches!(
                e,
                SourceError::Timeout { .. }
                    | SourceError::Request { .. }
                    | SourceError::Status {
                        status: 429 | 500..=599,
                        ..
                    }
            );
            check.error = Some(ErrorChain(&e).to_string());
        }
        Ok(Fetched::NotModified) => check.error = Some("unexpected 304 Not Modified".into()),
        Ok(Fetched::Batch(batch)) => {
            check.jobs = batch.records.len();
            check.rejected = batch.rejected.len();
            check.complete = batch.complete;
            check.with_provider_id = batch
                .records
                .iter()
                .filter(|p| p.provenance.source_record_id.is_some())
                .count();
            check.naming_company = batch
                .records
                .iter()
                .filter(|p| names_company(p, target))
                .count();
            check.naming_domain = batch
                .records
                .iter()
                .filter(|p| ties_to_domain(p, target))
                .count();
            for p in &batch.records {
                if check.company_names.len() < 3 && !check.company_names.contains(&p.company) {
                    check.company_names.push(p.company.clone());
                }
            }
            check.freshness = Freshness::of(batch.records.iter().map(|p| p.posted_at), now);
            check.postings = batch.records;
        }
    }
    if check.error.is_none() && found.board.kind == ashby::KIND {
        check.website = ashby_website(http, &found.board.name, api_base).await;
        check.website_matches = check
            .website
            .as_deref()
            .and_then(|w| Url::parse(w).ok())
            .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
            .map(|host| {
                let host = host.strip_prefix("www.").unwrap_or(&host).to_owned();
                host == target.domain || host.split('.').next() == Some(target.stem())
            });
    }
    check
}

/// Recomputes what ties a checked board to `target` (a company found after
/// the board was read): the postings naming the company or its domain,
/// and whether the website the board names is the company's.
pub fn retarget(check: &mut BoardCheck, target: &CompanyTarget) {
    check.matches_company =
        slug_matches(check.source.split(':').nth(1).unwrap_or_default(), target);
    check.naming_company = check
        .postings
        .iter()
        .filter(|p| names_company(p, target))
        .count();
    check.naming_domain = check
        .postings
        .iter()
        .filter(|p| ties_to_domain(p, target))
        .count();
    check.website_matches = check
        .website
        .as_deref()
        .and_then(|w| Url::parse(w).ok())
        .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
        .map(|host| website_is(&host, target));
}

fn website_is(host: &str, target: &CompanyTarget) -> bool {
    let host = host.strip_prefix("www.").unwrap_or(host);
    host == target.domain
        || host.ends_with(&format!(".{}", target.domain))
        || host.split('.').next() == Some(target.stem())
}

/// The website an Ashby board names (`"publicWebsite"` in its page data).
async fn ashby_website(http: &HttpClient, board: &str, base: Option<&str>) -> Option<String> {
    let base = base.unwrap_or("https://jobs.ashbyhq.com");
    let url = Url::parse(&format!("{}/{board}", base.trim_end_matches('/'))).ok()?;
    let answer = http.probe(&url).await.ok()?;
    if !(200..300).contains(&answer.status) {
        return None;
    }
    let html = String::from_utf8_lossy(&answer.body).replace("\\/", "/");
    let start = html.find("\"publicWebsite\":\"")? + "\"publicWebsite\":\"".len();
    let value = &html[start..];
    let end = value.find('"')?;
    Some(value[..end].to_owned()).filter(|w| w.starts_with("http"))
}

/// Whether a posting names the company: its company field (Greenhouse
/// reports the employer's own name) or its text mentions the company's
/// name or domain. Board slugs are not evidence (Ashby and Lever report
/// the slug as the company).
fn names_company(posting: &JobPosting, target: &CompanyTarget) -> bool {
    let name = target.display_name().to_ascii_lowercase();
    let squashed = squash(&name);
    let company = posting.company.to_ascii_lowercase();
    let reported_by_ats = company != posting.provenance.source.instance();
    if reported_by_ats && squashed.len() >= 3 && squash(&company).contains(&squashed) {
        return true;
    }
    let text = posting
        .description_text
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    text.contains(&target.domain) || (name.len() >= 3 && contains_word(&text, &name))
}

/// Whether a posting is tied to the company's domain: its URL or apply URL
/// is on it, or its text mentions it (`railway.com`).
fn ties_to_domain(posting: &JobPosting, target: &CompanyTarget) -> bool {
    let on_domain = |url: &jobhunt_core::CanonicalUrl| {
        let host = url.host();
        host == target.domain || host.ends_with(&format!(".{}", target.domain))
    };
    if on_domain(&posting.url) || posting.apply_url.as_ref().is_some_and(on_domain) {
        return true;
    }
    let text = posting
        .description_text
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    contains_word(&text, &target.domain)
}

/// `needle` in `haystack` with no letter or digit right before or after.
fn contains_word(haystack: &str, needle: &str) -> bool {
    let mut from = 0;
    while let Some(pos) = haystack[from..].find(needle) {
        let start = from + pos;
        let end = start + needle.len();
        let before = haystack[..start].chars().next_back();
        let after = haystack[end..].chars().next();
        let boundary = |c: Option<char>| !c.is_some_and(char::is_alphanumeric);
        if boundary(before) && boundary(after) {
            return true;
        }
        from = start + needle.len().max(1);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(s: &str) -> CompanyTarget {
        s.parse().unwrap()
    }

    #[test]
    fn parses_company_targets() {
        assert_eq!(
            target("https://www.Railway.com/careers?x=1"),
            CompanyTarget {
                domain: "railway.com".into(),
                name: None
            }
        );
        let t = target("posthog.com, PostHog");
        assert_eq!(t.domain, "posthog.com");
        assert_eq!(t.display_name(), "PostHog");
        assert_eq!(target("linear.app").display_name(), "linear");
        assert!("not a domain".parse::<CompanyTarget>().is_err());
        assert!("localhost".parse::<CompanyTarget>().is_err());

        let list = parse_targets(
            "# devtools\nrailway.com, Railway\n\nposthog.com  # analytics\nrailway.com\n",
        )
        .unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[1].domain, "posthog.com");
        assert_eq!(
            parse_targets("ok.com\nbad domain").unwrap_err(),
            "line 2: \"bad domain\" is not a company domain (expected e.g. \"railway.com\" or \"railway.com, Railway\")"
        );
    }

    #[test]
    fn slugs_match_their_company() {
        let t = target("sourcegraph.com, Sourcegraph");
        assert!(slug_matches("sourcegraph91", &t));
        assert!(slug_matches("convex-dev", &target("convex.dev")));
        assert!(!slug_matches("stripe", &t));
        // Too short to compare: never a match by containment.
        assert!(!slug_matches("a", &t));
        assert_eq!(
            guess_slugs(&target("duckduckgo.com, Duck Duck Go")),
            vec!["duckduckgo", "duck-duck-go"]
        );
    }

    #[test]
    fn words_match_on_boundaries() {
        assert!(contains_word("join railway today", "railway"));
        assert!(!contains_word("railways of europe", "railway"));
        assert!(contains_word("(oyster)", "oyster"));
    }
}
