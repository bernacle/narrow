//! From a discovered board back to the company that owns it.
//!
//! [`crate::company::discover`] starts from a company and looks for its
//! board. A discovered board comes the other way, so resolution runs in two
//! stages:
//!
//! 1. **Read** ([`read_board`]): the board through its regular adapter,
//!    once. This alone says whether the board still exists, what it lists,
//!    and which discovered jobs are still on it (a job missing from the
//!    current listing is closed, whatever the search index says). One API
//!    request per board, so it can run on every board found.
//! 2. **Ownership** ([`establish_ownership`]): the company's domain, from
//!    what the board says about itself ([`DomainHint`]: the website an
//!    Ashby board names, where the hosted board redirects, the company
//!    links on the hosted page, postings hosted on the company's site,
//!    domains the postings mention) and from what the discovery source
//!    said (a hiring post or a directory naming the domain). Then the
//!    company's own site is read with [`crate::company::discover`] (no slug
//!    guessing): when it links, embeds or redirects to this board, the
//!    board is verified first-party, and [`registry::validate`] judges it
//!    exactly as it judges a board found from the company's side.
//!
//! A board's word about itself is not proof: a board naming a website
//! whose site does not point back stays [`Ownership::Claimed`] (a
//! candidate), unless a source independent of the board names the same
//! domain and the postings tie to it ([`Ownership::Corroborated`]). When
//! the company's site points at a *different* board (the company moved
//! ATS, the indexed board is a leftover), the board is
//! [`Ownership::Elsewhere`] and the other board is reported, so discovery
//! follows the company to its current board.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use url::Url;

use super::Ownership;
use super::ats;
use crate::careers::{self, BoardRef};
use crate::company::{
    self, BoardCheck, BoardEvidence, CompanyProbe, CompanyTarget, FoundBoard, ProbeSettings,
};
use crate::http::HttpClient;
use crate::registry::{self, Validation, Verdict};
use crate::{ashby, greenhouse, lever};

/// Where a company domain came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HintSource {
    /// The discovery source itself (a hiring post, a directory entry):
    /// independent of the board.
    Discovery,
    /// The hosted board page redirects to the company's site.
    BoardRedirect,
    /// The website the board names (Ashby's `publicWebsite`).
    BoardWebsite,
    /// Postings whose public URL is on the company's site (an embedded
    /// board).
    PostingUrl,
    /// Links on the hosted board page.
    BoardPage,
    /// Domains the postings' text mentions.
    PostingText,
}

impl HintSource {
    fn weight(self) -> usize {
        match self {
            Self::Discovery => 6,
            Self::BoardRedirect | Self::BoardWebsite => 5,
            Self::PostingUrl => 4,
            Self::BoardPage => 3,
            Self::PostingText => 1,
        }
    }

    /// Independent of the board's own description of itself.
    pub fn independent(self) -> bool {
        self == Self::Discovery
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomainHint {
    pub domain: String,
    pub from: HintSource,
    /// How many times it was seen (links, postings).
    pub count: usize,
}

/// Where requests go; the defaults are the real hosts (tests point them at
/// a local server).
#[derive(Debug, Clone, Default)]
pub struct ResolveSettings {
    /// ATS API requests go here instead of the real hosts.
    pub api_base: Option<String>,
    /// Hosted board pages are read here (`<base>/<board>`) instead of the
    /// ATS's host.
    pub board_page_base: Option<String>,
    /// The company's site is read here instead of `https://<domain>/`.
    pub site_base: Option<Url>,
    /// Pages of a company's site to read at most when checking ownership.
    pub max_pages: usize,
    /// Domain hints to try at most.
    pub max_hints: usize,
}

impl ResolveSettings {
    pub fn new() -> Self {
        Self {
            max_pages: 6,
            max_hints: 2,
            ..Self::default()
        }
    }
}

/// What reading a board found (stage 1).
#[derive(Debug, Clone)]
pub struct BoardRead {
    pub board: BoardRef,
    /// The adapter's read, postings included.
    pub check: BoardCheck,
    /// `ok`, `not_found`, `empty`, `failed` or `transient`.
    pub listing: &'static str,
}

impl BoardRead {
    /// The posting with this provider job id, if the board lists it now.
    pub fn posting(&self, job_id: &str) -> Option<&jobhunt_jobs::JobPosting> {
        self.check.postings.iter().find(|p| {
            p.provenance
                .source_record_id
                .as_deref()
                .is_some_and(|id| id.eq_ignore_ascii_case(job_id))
        })
    }
}

fn placeholder(board: &BoardRef) -> CompanyTarget {
    CompanyTarget {
        domain: format!("{}.invalid", board.name),
        name: None,
    }
}

/// Stage 1: reads a board through its adapter.
pub async fn read_board(
    http: &HttpClient,
    board: &BoardRef,
    settings: &ResolveSettings,
    now: DateTime<Utc>,
) -> BoardRead {
    let found = FoundBoard {
        source: ats::source_key(board).unwrap_or_default(),
        board: board.clone(),
        evidence: BoardEvidence::Discovered,
        page: None,
        matches_company: false,
    };
    let check = company::check_board(
        http,
        &placeholder(board),
        &found,
        settings.api_base.as_deref(),
        now,
    )
    .await;
    let listing = match &check.error {
        Some(e) if e.contains("not found") || e.contains("404") => "not_found",
        Some(_) if check.transient => "transient",
        Some(_) => "failed",
        None if check.jobs == 0 => "empty",
        None => "ok",
    };
    BoardRead {
        board: board.clone(),
        check,
        listing,
    }
}

/// What stage 2 concluded.
#[derive(Debug, Clone)]
pub struct OwnershipResult {
    pub ownership: Ownership,
    pub domain: Option<String>,
    pub company: Option<String>,
    pub careers_url: Option<String>,
    pub hints: Vec<DomainHint>,
    /// Boards the company's site points at instead.
    pub points_at: Vec<BoardRef>,
    pub validation: Validation,
    /// The company probes run, for the unsupported-ATS map.
    pub probes: Vec<CompanyProbe>,
}

/// The validation of a board that could not be read or lists nothing:
/// [`registry::validate`] on the read alone.
pub fn validate_read(read: &BoardRead) -> Validation {
    registry::validate(&read.check)
}

/// Stage 2: finds the company behind a board and checks that the company's
/// site points back at it. `independent` are domains the discovery source
/// named (a hiring post's link, a directory's website).
pub async fn establish_ownership(
    http: &HttpClient,
    read: &BoardRead,
    independent: &[String],
    company_hint: Option<&str>,
    settings: &ResolveSettings,
    now: DateTime<Utc>,
) -> OwnershipResult {
    let mut hints: Vec<DomainHint> = independent
        .iter()
        .filter_map(|d| ats::registrable_domain(d))
        .filter(|d| ats::is_company_domain(d))
        .map(|domain| DomainHint {
            domain,
            from: HintSource::Discovery,
            count: 1,
        })
        .collect();
    // An Ashby board names its website in the page already read.
    let page = if read.board.kind == ashby::KIND && read.check.website.is_some() {
        None
    } else {
        board_page(http, &read.board, settings).await
    };
    hints.extend(board_hints(&read.check, page.as_ref()));
    let hints = rank_hints(hints);
    let company = company_hint
        .map(str::to_owned)
        .or_else(|| page.as_ref().and_then(|p| p.company.clone()))
        .or_else(|| {
            read.check
                .company_names
                .iter()
                .find(|n| !n.eq_ignore_ascii_case(&read.board.name))
                .cloned()
        });
    let key = read.check.source.clone();
    let mut probes = Vec::new();
    let mut points_at: Vec<BoardRef> = Vec::new();

    for hint in hints.iter().take(settings.max_hints.max(1)) {
        let target = CompanyTarget {
            domain: hint.domain.clone(),
            name: company.clone(),
        };
        let probe_settings = ProbeSettings {
            max_pages: settings.max_pages.max(1),
            guess_slugs: false,
            api_base: settings.api_base.clone(),
            site_base: settings.site_base.clone(),
        };
        let probe = company::discover(http, &target, &probe_settings, now).await;
        let linked = probe
            .checks
            .iter()
            .find(|c| c.source == key && c.evidence.first_party())
            .cloned();
        if let Some(check) = linked {
            let validation = registry::validate(&check);
            let ownership = if validation.verdict == Verdict::Validated {
                Ownership::Verified
            } else {
                Ownership::Claimed
            };
            let careers_url = probe.careers_url.clone();
            probes.push(probe);
            return OwnershipResult {
                ownership,
                domain: Some(hint.domain.clone()),
                company,
                careers_url,
                hints,
                points_at,
                validation,
                probes,
            };
        }
        let others: Vec<BoardRef> = probe
            .boards
            .iter()
            .filter(|b| b.source != key && b.evidence.first_party() && b.matches_company)
            .map(|b| b.board.clone())
            .collect();
        let elsewhere = !others.is_empty();
        points_at.extend(others);
        probes.push(probe);
        if elsewhere {
            break;
        }
    }

    // Not linked from the company's site: the board's own word, unless an
    // independent source names the same domain.
    let best = hints.first().cloned();
    let mut check = read.check.clone();
    check.evidence = BoardEvidence::Discovered;
    if let Some(best) = &best {
        company::retarget(
            &mut check,
            &CompanyTarget {
                domain: best.domain.clone(),
                name: company.clone(),
            },
        );
    }
    let mut validation = registry::validate(&check);
    let ownership = if !points_at.is_empty() {
        Ownership::Elsewhere
    } else if best.is_none() {
        Ownership::Unknown
    } else if validation.verdict == Verdict::Validated
        && best.as_ref().is_some_and(|h| h.from.independent())
    {
        Ownership::Corroborated
    } else {
        Ownership::Claimed
    };
    if validation.verdict != Verdict::Rejected && !ownership.established() {
        validation.verdict = Verdict::Inconclusive;
        validation.reasons.push(match ownership {
            Ownership::Elsewhere => format!(
                "the company's site points at {} instead",
                points_at
                    .iter()
                    .filter_map(ats::source_key)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Ownership::Unknown => "no company domain found for the board".into(),
            _ => format!(
                "only the board names {} ({}); the company's site doesn't point back at it",
                best.as_ref().map_or("", |h| h.domain.as_str()),
                best.as_ref().map_or("", |h| hint_label(h.from)),
            ),
        });
    }
    let careers_url = probes.iter().find_map(|p| p.careers_url.clone());
    OwnershipResult {
        ownership,
        domain: best.map(|h| h.domain),
        company,
        careers_url,
        hints,
        points_at,
        validation,
        probes,
    }
}

fn hint_label(from: HintSource) -> &'static str {
    match from {
        HintSource::Discovery => "discovery source",
        HintSource::BoardRedirect => "board redirect",
        HintSource::BoardWebsite => "board website",
        HintSource::PostingUrl => "posting URLs",
        HintSource::BoardPage => "board page links",
        HintSource::PostingText => "posting text",
    }
}

/// What the hosted board page says.
#[derive(Debug, Clone, Default)]
struct BoardPage {
    redirect: Option<String>,
    domains: Vec<(String, usize)>,
    company: Option<String>,
}

async fn board_page(
    http: &HttpClient,
    board: &BoardRef,
    settings: &ResolveSettings,
) -> Option<BoardPage> {
    let url = match &settings.board_page_base {
        Some(base) => Url::parse(&format!("{}/{}", base.trim_end_matches('/'), board.name)).ok()?,
        None => Url::parse(&ats::board_url(board)).ok()?,
    };
    let answer = http.probe(&url).await.ok()?;
    let mut page = BoardPage::default();
    if answer.final_url.host_str() != url.host_str()
        && careers::board_for_url(&answer.final_url).is_none()
    {
        page.redirect = ats::company_domain_of(&answer.final_url);
    }
    if (200..300).contains(&answer.status) {
        let html = String::from_utf8_lossy(&answer.body).replace("\\/", "/");
        page.domains = count_domains(&html);
        page.company = page_company(&html);
    }
    Some(page)
}

/// Company domains linked from a page, most linked first.
fn count_domains(html: &str) -> Vec<(String, usize)> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for (_, url) in careers::absolute_urls(html) {
        if let Some(d) = ats::company_domain_of(&url) {
            *counts.entry(d).or_default() += 1;
        }
    }
    let mut out: Vec<_> = counts.into_iter().collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    out
}

/// The company's name from the hosted page's title (`Jobs at Moxie`,
/// `Pipedrive jobs`, `Careers at Acme`).
fn page_company(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let open = start + lower[start..].find('>')? + 1;
    let end = open + lower[open..].find("</title")?;
    let title = jobhunt_core::html::decode_entities(html[open..end].trim()).into_owned();
    let mut name = title.as_str();
    for prefix in [
        "Jobs at ",
        "Careers at ",
        "Job openings at ",
        "Current openings at ",
    ] {
        if let Some(rest) = name.strip_prefix(prefix) {
            name = rest;
        }
    }
    for suffix in [" jobs", " Jobs", " careers", " Careers", " - Careers"] {
        if let Some(rest) = name.strip_suffix(suffix) {
            name = rest;
        }
    }
    let name = name.trim();
    (!name.is_empty() && name.len() <= 80).then(|| name.to_owned())
}

/// Domain hints from the board's own data.
fn board_hints(check: &BoardCheck, page: Option<&BoardPage>) -> Vec<DomainHint> {
    let mut out = Vec::new();
    if let Some(page) = page {
        if let Some(d) = &page.redirect {
            out.push(DomainHint {
                domain: d.clone(),
                from: HintSource::BoardRedirect,
                count: 1,
            });
        }
        for (d, n) in page.domains.iter().take(3) {
            out.push(DomainHint {
                domain: d.clone(),
                from: HintSource::BoardPage,
                count: *n,
            });
        }
    }
    if let Some(d) = check
        .website
        .as_deref()
        .and_then(|w| Url::parse(w).ok())
        .and_then(|u| ats::company_domain_of(&u))
    {
        out.push(DomainHint {
            domain: d,
            from: HintSource::BoardWebsite,
            count: 1,
        });
    }
    let mut on_site: HashMap<String, usize> = HashMap::new();
    let mut mentioned: HashMap<String, usize> = HashMap::new();
    for p in &check.postings {
        for url in std::iter::once(&p.url).chain(p.apply_url.as_ref()) {
            if let Some(d) = Url::parse(url.as_str())
                .ok()
                .and_then(|u| ats::company_domain_of(&u))
            {
                *on_site.entry(d).or_default() += 1;
                break;
            }
        }
        let text = p
            .description_html
            .as_deref()
            .or(p.description_text.as_deref())
            .unwrap_or_default();
        let mut seen = std::collections::HashSet::new();
        for (_, url) in careers::absolute_urls(text) {
            if let Some(d) = ats::company_domain_of(&url)
                && seen.insert(d.clone())
            {
                *mentioned.entry(d).or_default() += 1;
            }
        }
    }
    for (map, from) in [
        (on_site, HintSource::PostingUrl),
        (mentioned, HintSource::PostingText),
    ] {
        let mut list: Vec<_> = map.into_iter().collect();
        list.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        for (domain, count) in list.into_iter().take(2) {
            out.push(DomainHint {
                domain,
                from,
                count,
            });
        }
    }
    out
}

/// One hint per domain (its strongest source, the counts added up), best
/// first: by source, then by how often it was seen.
pub fn rank_hints(hints: Vec<DomainHint>) -> Vec<DomainHint> {
    let mut by_domain: Vec<DomainHint> = Vec::new();
    for h in hints {
        match by_domain.iter_mut().find(|e| e.domain == h.domain) {
            Some(e) => {
                e.count += h.count;
                if h.from.weight() > e.from.weight() {
                    e.from = h.from;
                }
            }
            None => by_domain.push(h),
        }
    }
    let score = |h: &DomainHint| h.from.weight() * 10 + h.count.min(9);
    by_domain.sort_by(|a, b| score(b).cmp(&score(a)).then(a.domain.cmp(&b.domain)));
    by_domain
}

/// Which supported kinds have a hosted page worth reading for hints.
pub fn has_board_page(kind: &str) -> bool {
    matches!(kind, ashby::KIND | greenhouse::KIND | lever::KIND)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hints_rank_by_source_then_frequency() {
        let ranked = rank_hints(vec![
            DomainHint {
                domain: "mentioned.com".into(),
                from: HintSource::PostingText,
                count: 9,
            },
            DomainHint {
                domain: "acme.com".into(),
                from: HintSource::BoardPage,
                count: 2,
            },
            DomainHint {
                domain: "acme.com".into(),
                from: HintSource::BoardWebsite,
                count: 1,
            },
            DomainHint {
                domain: "parent.com".into(),
                from: HintSource::BoardPage,
                count: 1,
            },
        ]);
        let domains: Vec<&str> = ranked.iter().map(|h| h.domain.as_str()).collect();
        assert_eq!(domains, ["acme.com", "parent.com", "mentioned.com"]);
        assert_eq!(ranked[0].from, HintSource::BoardWebsite);
        assert_eq!(ranked[0].count, 3);
    }

    #[test]
    fn hosted_page_titles_name_the_company() {
        assert_eq!(
            page_company("<html><title>Jobs at Moxie</title>").as_deref(),
            Some("Moxie")
        );
        assert_eq!(
            page_company("<TITLE>Pipedrive jobs</TITLE>").as_deref(),
            Some("Pipedrive")
        );
        assert_eq!(page_company("<title> </title>"), None);
    }

    #[test]
    fn hosted_page_links_suggest_the_company_domain() {
        let html = r#"
            <a href="https://jobs.lever.co/pipedrive">logo</a>
            <a href="https://www.pipedrive.com/en/jobs">Company website</a>
            <a href="https://www.pipedrive.com/en/jobs/privacy-policy">Privacy</a>
            <a href="https://www.linkedin.com/company/pipedrive">LinkedIn</a>
            <script src="https://cdn.jsdelivr.net/x.js"></script>
        "#;
        assert_eq!(count_domains(html), vec![("pipedrive.com".to_owned(), 2)]);
    }
}
