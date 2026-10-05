//! First-party careers pages.
//!
//! JobHunt does not scrape arbitrary career sites: every site is different,
//! and a generic scraper would produce exactly the kind of noisy, fragile
//! data discovery must avoid. Most company careers pages, however, are a
//! thin shell around a hosted ATS board: they link to
//! `jobs.lever.co/<site>`, embed `boards.greenhouse.io/embed/job_board?for=…`,
//! or load `jobs.ashbyhq.com/<board>/embed`. Reading that board through its
//! adapter gives complete, structured data.
//!
//! This module provides that bridge:
//!
//! * [`board_for_url`] recognizes a supported board from its URL, without
//!   any network access (`https://jobs.lever.co/spotify` → `lever:spotify`);
//! * [`find_boards`] finds board references inside a page's HTML;
//! * [`detect`] fetches a careers page and returns the boards it uses.
//!
//! A future first-party adapter (for example one reading schema.org
//! `JobPosting` data) would be a regular [`jobhunt_core::Source`]; nothing
//! here would need to change.

use jobhunt_core::SourceError;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::http::HttpClient;
use crate::lever::LeverRegion;
use crate::{ashby, greenhouse, lever, yc};

/// A company careers page to read through the board it embeds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CareersPage {
    /// The page's URL, e.g. `https://www.example.com/careers`.
    pub url: String,
    /// Company display name for the detected board.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub company: Option<String>,
}

/// A supported board, as referenced by a URL.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BoardRef {
    /// Source kind: `ashby`, `greenhouse`, `lever` or `yc`.
    pub kind: &'static str,
    /// Board, site or company slug.
    pub name: String,
    pub lever_region: LeverRegion,
}

impl BoardRef {
    fn new(kind: &'static str, name: &str) -> Option<Self> {
        let valid = !name.is_empty()
            && name.len() <= 100
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'));
        valid.then(|| Self {
            kind,
            name: name.to_ascii_lowercase(),
            lever_region: LeverRegion::Global,
        })
    }
}

/// Path segments that are part of a host's own routes, never board names.
const RESERVED: &[&str] = &[
    "embed",
    "api",
    "v0",
    "v1",
    "static",
    "assets",
    "images",
    "favicon.ico",
    "robots.txt",
    "industry",
    "location",
    "batch",
    "tags",
];

/// Recognizes a supported board from a URL (a board page, a job page, an
/// embed script or an API endpoint).
pub fn board_for_url(url: &Url) -> Option<BoardRef> {
    let host = url.host_str()?.to_ascii_lowercase();
    let segments: Vec<&str> = url
        .path_segments()
        .map(|s| s.filter(|seg| !seg.is_empty()).collect())
        .unwrap_or_default();
    let query = |name: &str| {
        url.query_pairs()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.into_owned())
    };
    let first = segments.first().copied().filter(|s| !RESERVED.contains(s));

    match host.as_str() {
        "jobs.ashbyhq.com" => BoardRef::new(ashby::KIND, first?),
        "api.ashbyhq.com" => match segments.as_slice() {
            ["posting-api", "job-board", board, ..] => BoardRef::new(ashby::KIND, board),
            _ => None,
        },
        "boards.greenhouse.io"
        | "job-boards.greenhouse.io"
        | "boards.eu.greenhouse.io"
        | "job-boards.eu.greenhouse.io" => match segments.as_slice() {
            ["embed", ..] => BoardRef::new(greenhouse::KIND, &query("for")?),
            _ => BoardRef::new(greenhouse::KIND, first?),
        },
        "boards-api.greenhouse.io" => match segments.as_slice() {
            ["v1", "boards", board, ..] => BoardRef::new(greenhouse::KIND, board),
            _ => None,
        },
        "jobs.lever.co" | "jobs.eu.lever.co" => {
            let mut board = BoardRef::new(lever::KIND, first?)?;
            if host == "jobs.eu.lever.co" {
                board.lever_region = LeverRegion::Eu;
            }
            Some(board)
        }
        "api.lever.co" | "api.eu.lever.co" => match segments.as_slice() {
            ["v0", "postings", site, ..] => {
                let mut board = BoardRef::new(lever::KIND, site)?;
                if host == "api.eu.lever.co" {
                    board.lever_region = LeverRegion::Eu;
                }
                Some(board)
            }
            _ => None,
        },
        "www.ycombinator.com" | "ycombinator.com" => match segments.as_slice() {
            ["companies", slug, ..] if !RESERVED.contains(slug) => BoardRef::new(yc::KIND, slug),
            _ => None,
        },
        _ => None,
    }
}

/// Hosts whose URLs can reference a supported board.
const BOARD_HOSTS: &[&str] = &[
    "jobs.ashbyhq.com/",
    "api.ashbyhq.com/",
    "boards.greenhouse.io/",
    "job-boards.greenhouse.io/",
    "boards.eu.greenhouse.io/",
    "job-boards.eu.greenhouse.io/",
    "boards-api.greenhouse.io/",
    "jobs.lever.co/",
    "jobs.eu.lever.co/",
    "api.lever.co/",
    "api.eu.lever.co/",
];

/// Finds references to supported boards in HTML (links, iframes, embed
/// scripts, inline JSON). Each board is returned once, in page order.
pub fn find_boards(html: &str) -> Vec<BoardRef> {
    // Inline JSON often escapes slashes ("jobs.lever.co\/spotify").
    let html = html.replace("\\/", "/");
    let mut found: Vec<(usize, BoardRef)> = Vec::new();
    for host in BOARD_HOSTS {
        let mut from = 0;
        while let Some(pos) = html[from..].find(host) {
            let start = from + pos;
            from = start + host.len();
            // Skip matches that are part of a longer host name
            // ("myjobs.lever.co" is not Lever's host).
            let preceded_by_host_char = html[..start]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.');
            if preceded_by_host_char {
                continue;
            }
            let end = html[start..]
                .find(|c: char| {
                    c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | ')' | '\\' | '`')
                })
                .map_or(html.len(), |e| start + e);
            let candidate = format!("https://{}", html[start..end].replace("&amp;", "&"));
            if let Some(board) = Url::parse(&candidate).ok().as_ref().and_then(board_for_url)
                && !found.iter().any(|(_, b)| *b == board)
            {
                found.push((start, board));
            }
        }
    }
    found.sort_by_key(|(pos, _)| *pos);
    found.into_iter().map(|(_, board)| board).collect()
}

/// Fetches a careers page and returns the supported boards it references.
/// A URL that already is a board URL is recognized without fetching.
pub async fn detect(http: &HttpClient, page: &Url) -> Result<Vec<BoardRef>, SourceError> {
    if let Some(board) = board_for_url(page) {
        return Ok(vec![board]);
    }
    let body = http.get_bytes(page).await?;
    Ok(find_boards(&String::from_utf8_lossy(&body)))
}

/// A hosted applicant tracking system JobHunt has no adapter for, as
/// referenced by a page. Recorded so a careers page behind one is
/// classified as such (and counted when choosing the next adapter to
/// write), never scraped.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct OtherAts {
    /// `workday`, `smartrecruiters`, `workable`, …
    pub provider: &'static str,
    /// The first URL referencing it.
    pub url: String,
}

/// Host suffixes of known ATS without an adapter, and their names. A host
/// matches when it equals the suffix or ends with `.` + suffix.
const OTHER_ATS_HOSTS: &[(&str, &str)] = &[
    ("myworkdayjobs.com", "workday"),
    ("myworkdaysite.com", "workday"),
    ("smartrecruiters.com", "smartrecruiters"),
    ("workable.com", "workable"),
    ("bamboohr.com", "bamboohr"),
    ("recruitee.com", "recruitee"),
    ("personio.de", "personio"),
    ("personio.com", "personio"),
    ("teamtailor.com", "teamtailor"),
    ("applytojob.com", "jazzhr"),
    ("breezy.hr", "breezy"),
    ("ats.rippling.com", "rippling"),
    ("app.dover.com", "dover"),
    ("jobs.gem.com", "gem"),
    ("pinpointhq.com", "pinpoint"),
    ("icims.com", "icims"),
    ("jobvite.com", "jobvite"),
    ("comeet.com", "comeet"),
    ("homerun.co", "homerun"),
    ("jobs.polymer.co", "polymer"),
    ("wellfound.com", "wellfound"),
    ("join.com", "join"),
    ("factorialhr.com", "factorial"),
];

/// Every absolute `http(s)` URL in `html` (links, scripts, inline JSON),
/// with its byte offset, in page order.
pub(crate) fn absolute_urls(html: &str) -> Vec<(usize, Url)> {
    let mut out = Vec::new();
    for scheme in ["https://", "http://"] {
        let mut from = 0;
        while let Some(pos) = html[from..].find(scheme) {
            let start = from + pos;
            let end = html[start..]
                .find(|c: char| {
                    c.is_whitespace()
                        || matches!(c, '"' | '\'' | '<' | '>' | ')' | '\\' | '`' | ',' | ';')
                })
                .map_or(html.len(), |e| start + e);
            from = end.max(start + scheme.len());
            if let Ok(url) = Url::parse(&html[start..end].replace("&amp;", "&")) {
                out.push((start, url));
            }
        }
    }
    out.sort_by_key(|(pos, _)| *pos);
    out
}

/// Whether `host` is `suffix` or one of its subdomains.
fn host_within(host: &str, suffix: &str) -> bool {
    host == suffix
        || host
            .strip_suffix(suffix)
            .is_some_and(|rest| rest.ends_with('.'))
}

/// The unsupported ATS a URL belongs to, if any.
pub fn other_ats_for_url(url: &Url) -> Option<&'static str> {
    let host = url.host_str()?.to_ascii_lowercase();
    OTHER_ATS_HOSTS
        .iter()
        .find(|(suffix, _)| host_within(&host, suffix))
        .map(|(_, provider)| *provider)
}

/// Finds references to known ATS that have no adapter. Each provider is
/// returned once, in page order.
pub fn find_other_ats(html: &str) -> Vec<OtherAts> {
    let html = html.replace("\\/", "/");
    let mut found: Vec<OtherAts> = Vec::new();
    for (_, url) in absolute_urls(&html) {
        if let Some(provider) = other_ats_for_url(&url)
            && !found.iter().any(|o| o.provider == provider)
        {
            found.push(OtherAts {
                provider,
                url: url.to_string(),
            });
        }
    }
    found
}

/// Words that mark a link to a careers page, in its path or its text.
const CAREERS_PATH_WORDS: &[&str] = &[
    "careers",
    "career",
    "jobs",
    "join-us",
    "joinus",
    "work-with-us",
    "hiring",
    "open-positions",
    "open-roles",
    "openings",
];
const CAREERS_TEXT_WORDS: &[&str] = &[
    "careers",
    "jobs",
    "join us",
    "join the team",
    "we're hiring",
    "we’re hiring",
    "open roles",
    "open positions",
    "work with us",
    "hiring",
];

/// Links on a page that lead to the company's careers page: same site as
/// `site` (the company's domain, subdomains included), with a careers word
/// in the path or the link text. Resolved against `base`, deduplicated,
/// in page order. Links straight to an ATS are found by [`find_boards`]
/// and [`find_other_ats`] instead.
pub fn careers_links(html: &str, base: &Url, site: &str) -> Vec<Url> {
    let lower = html.to_ascii_lowercase();
    let mut out: Vec<Url> = Vec::new();
    let mut from = 0;
    while let Some(pos) = lower[from..].find("<a") {
        let start = from + pos;
        from = start + 2;
        // `<a` followed by whitespace: an anchor, not `<abbr>` or `<aside>`.
        if !lower[from..].starts_with(|c: char| c.is_ascii_whitespace()) {
            continue;
        }
        let Some(tag_end) = lower[start..].find('>').map(|e| start + e) else {
            break;
        };
        let Some(href) = attribute(&html[start..tag_end], &lower[start..tag_end], "href") else {
            continue;
        };
        let text_end = lower[tag_end..]
            .find("</a")
            .map_or(lower.len().min(tag_end + 300), |e| tag_end + e);
        let text = strip_tags(&lower[tag_end + 1..text_end]);
        let href = jobhunt_core::html::decode_entities(&href).into_owned();
        let Ok(url) = base.join(href.trim()) else {
            continue;
        };
        let same_site = url
            .host_str()
            .is_some_and(|h| host_within(&h.to_ascii_lowercase(), site));
        if !matches!(url.scheme(), "http" | "https") || !same_site {
            continue;
        }
        let path = url.path().to_ascii_lowercase();
        let by_path = path
            .split('/')
            .any(|segment| CAREERS_PATH_WORDS.contains(&segment));
        let by_text = CAREERS_TEXT_WORDS.iter().any(|w| text.contains(w));
        let by_host = url
            .host_str()
            .is_some_and(|h| h.starts_with("careers.") || h.starts_with("jobs."));
        if !(by_path || by_text || by_host) {
            continue;
        }
        let mut url = url;
        url.set_fragment(None);
        if !out.contains(&url) {
            out.push(url);
        }
    }
    out
}

/// The value of attribute `name` in one tag (`lower` is the tag
/// lowercased, for matching; the value is taken from `tag`).
fn attribute(tag: &str, lower: &str, name: &str) -> Option<String> {
    let mut from = 0;
    while let Some(pos) = lower[from..].find(name) {
        let start = from + pos;
        from = start + name.len();
        let preceded = lower[..start]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_whitespace());
        let rest = lower[from..].trim_start();
        if !preceded || !rest.starts_with('=') {
            continue;
        }
        let offset = lower.len() - rest.len() + 1;
        let value = tag[offset..].trim_start();
        return match value.chars().next()? {
            q @ ('"' | '\'') => value[1..].find(q).map(|end| value[1..=end].to_owned()),
            _ => Some(
                value
                    .split(|c: char| c.is_ascii_whitespace() || c == '>')
                    .next()
                    .unwrap_or_default()
                    .to_owned(),
            ),
        };
    }
    None
}

fn strip_tags(fragment: &str) -> String {
    let mut out = String::with_capacity(fragment.len());
    let mut in_tag = false;
    for c in fragment.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board(url: &str) -> Option<(String, String)> {
        board_for_url(&Url::parse(url).unwrap()).map(|b| (b.kind.to_owned(), b.name))
    }

    fn pair(kind: &str, name: &str) -> Option<(String, String)> {
        Some((kind.to_owned(), name.to_owned()))
    }

    #[test]
    fn recognizes_board_urls() {
        assert_eq!(
            board("https://jobs.ashbyhq.com/Linear"),
            pair("ashby", "linear")
        );
        assert_eq!(
            board(
                "https://jobs.ashbyhq.com/linear/d3bc1ced-3ce4-4086-a050-555055dbb1ff/application"
            ),
            pair("ashby", "linear")
        );
        assert_eq!(
            board("https://jobs.ashbyhq.com/linear/embed?version=2"),
            pair("ashby", "linear")
        );
        assert_eq!(
            board("https://job-boards.greenhouse.io/anthropic"),
            pair("greenhouse", "anthropic")
        );
        assert_eq!(
            board("https://boards.greenhouse.io/figma/jobs/5426468004?gh_jid=5426468004"),
            pair("greenhouse", "figma")
        );
        assert_eq!(
            board("https://boards.greenhouse.io/embed/job_board/js?for=stripe"),
            pair("greenhouse", "stripe")
        );
        assert_eq!(
            board("https://boards-api.greenhouse.io/v1/boards/airbnb/jobs"),
            pair("greenhouse", "airbnb")
        );
        assert_eq!(
            board("https://jobs.lever.co/spotify/abc"),
            pair("lever", "spotify")
        );
        assert_eq!(
            board("https://www.ycombinator.com/companies/posthog/jobs"),
            pair("yc", "posthog")
        );

        let eu = board_for_url(&Url::parse("https://jobs.eu.lever.co/acme").unwrap()).unwrap();
        assert_eq!(eu.lever_region, LeverRegion::Eu);

        assert_eq!(board("https://boards.greenhouse.io/embed/job_board"), None);
        assert_eq!(
            board("https://www.ycombinator.com/companies/industry/fintech"),
            None
        );
        assert_eq!(board("https://jobs.lever.co/"), None);
        assert_eq!(board("https://stripe.com/jobs"), None);
    }

    #[test]
    fn finds_embedded_boards_in_html() {
        let html = r#"
            <a href="https://jobs.lever.co/spotify/abc?lever-origin=applied">Apply</a>
            <script src="https://boards.greenhouse.io/embed/job_board/js?for=stripe&amp;b=x"></script>
            <script>window.__DATA__ = {"jobs":"https:\/\/jobs.ashbyhq.com\/linear"};</script>
            <a href="https://jobs.lever.co/spotify">All jobs</a>
            <a href="https://myjobs.lever.co/other">Not Lever</a>
        "#;
        let found: Vec<(String, String)> = find_boards(html)
            .into_iter()
            .map(|b| (b.kind.to_owned(), b.name))
            .collect();
        assert_eq!(
            found,
            vec![
                ("lever".to_owned(), "spotify".to_owned()),
                ("greenhouse".to_owned(), "stripe".to_owned()),
                ("ashby".to_owned(), "linear".to_owned()),
            ]
        );
        assert!(find_boards("<p>No boards here</p>").is_empty());
    }

    fn kinds(html: &str) -> Vec<(String, String)> {
        find_boards(html)
            .into_iter()
            .map(|b| (b.kind.to_owned(), b.name))
            .collect()
    }

    #[test]
    fn detects_each_supported_ats_on_a_careers_page() {
        // Ashby: a link to the board and the embed script.
        assert_eq!(
            kinds(r#"<a href="https://jobs.ashbyhq.com/railway">Open roles</a>"#),
            vec![("ashby".to_owned(), "railway".to_owned())]
        );
        assert_eq!(
            kinds(r#"<script src="https://jobs.ashbyhq.com/oyster/embed?version=2"></script>"#),
            vec![("ashby".to_owned(), "oyster".to_owned())]
        );
        // Greenhouse: both board hosts, and a job link.
        assert_eq!(
            kinds(
                r#"<iframe src="https://job-boards.greenhouse.io/embed/job_board?for=wikimedia"></iframe>"#
            ),
            vec![("greenhouse".to_owned(), "wikimedia".to_owned())]
        );
        assert_eq!(
            kinds(r#"<a href="https://boards.greenhouse.io/gitlab/jobs/8123456002">Engineer</a>"#),
            vec![("greenhouse".to_owned(), "gitlab".to_owned())]
        );
        // Lever, global and EU.
        assert_eq!(
            kinds(r#"<a href='https://jobs.lever.co/pipedrive/2f3c1d7e'>Apply</a>"#),
            vec![("lever".to_owned(), "pipedrive".to_owned())]
        );
        let eu = find_boards(r#"<a href="https://jobs.eu.lever.co/acme">Jobs</a>"#);
        assert_eq!(eu[0].lever_region, LeverRegion::Eu);
    }

    #[test]
    fn several_boards_are_all_reported_in_page_order() {
        let html = r#"
            <a href="https://jobs.lever.co/acme">Engineering</a>
            <a href="https://boards.greenhouse.io/acme-sales">Sales</a>
            <a href="https://jobs.ashbyhq.com/acme">Everything else</a>
        "#;
        assert_eq!(
            kinds(html),
            vec![
                ("lever".to_owned(), "acme".to_owned()),
                ("greenhouse".to_owned(), "acme-sales".to_owned()),
                ("ashby".to_owned(), "acme".to_owned()),
            ]
        );
    }

    #[test]
    fn invalid_or_unrelated_ats_links_are_not_boards() {
        let html = r#"
            <a href="https://jobs.ashbyhq.com/">Ashby home</a>
            <a href="https://jobs.ashbyhq.com/embed">embed root</a>
            <a href="https://boards.greenhouse.io/embed/job_board">no board</a>
            <a href="https://jobs.lever.co/a%20b">bad slug</a>
            <a href="https://www.ashbyhq.com/customers">marketing</a>
            <a href="https://notgreenhouse.io/acme">lookalike</a>
            <a href="https://evil.jobs.lever.co.example.com/acme">lookalike</a>
        "#;
        assert_eq!(kinds(html), vec![]);
    }

    #[test]
    fn custom_careers_pages_have_no_board_but_known_ats_are_named() {
        let custom = r#"
            <h1>Careers at Acme</h1>
            <ul><li><a href="/careers/senior-engineer">Senior Engineer</a></li></ul>
            <form action="/careers/apply" method="post"></form>
        "#;
        assert!(find_boards(custom).is_empty());
        assert!(find_other_ats(custom).is_empty());

        let workday = r#"
            <a href="https://acme.wd5.myworkdayjobs.com/en-US/External">See jobs</a>
            <script>{"url":"https:\/\/apply.workable.com\/acme\/"}</script>
            <a href="https://acme.wd5.myworkdayjobs.com/en-US/External/job/1">Job</a>
        "#;
        let found = find_other_ats(workday);
        assert_eq!(
            found.iter().map(|o| o.provider).collect::<Vec<_>>(),
            vec!["workday", "workable"]
        );
        assert_eq!(
            found[0].url,
            "https://acme.wd5.myworkdayjobs.com/en-US/External"
        );
        // A host that merely ends with the same letters is not the ATS.
        assert!(find_other_ats(r#"<a href="https://notworkable.com/x">x</a>"#).is_empty());
    }

    #[test]
    fn finds_links_to_the_careers_page() {
        let base = Url::parse("https://www.acme.com/").unwrap();
        let html = r#"
            <nav>
              <a href="/about">About</a>
              <a class="nav" href="/careers#top">Careers</a>
              <a href="https://www.acme.com/company"><span>We&#39;re hiring!</span></a>
              <a href="https://careers.acme.com/">Work here</a>
              <a href="https://twitter.com/acme/jobs">Jobs on Twitter</a>
              <a href=/jobs>Jobs</a>
              <abbr href="/careers">not a link</abbr>
            </nav>
        "#;
        let links: Vec<String> = careers_links(html, &base, "acme.com")
            .into_iter()
            .map(String::from)
            .collect();
        assert_eq!(
            links,
            vec![
                "https://www.acme.com/careers",
                "https://www.acme.com/company",
                "https://careers.acme.com/",
                "https://www.acme.com/jobs",
            ]
        );
    }
}
