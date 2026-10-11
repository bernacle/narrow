//! A company's careers surface beyond its careers page: `robots.txt`, its
//! sitemaps, and `JobPosting` data on a few job pages.
//!
//! Used when [`crate::company::discover`] finds a careers page with no
//! recognizable board, or none at all: a site whose jobs load with
//! JavaScript often still lists each job page in its sitemap, and those
//! pages often carry `JobPosting` JSON-LD naming the ATS behind them.
//!
//! It never crawls the site. [`SurfaceLimits`] caps everything per company
//! (sitemap files, URLs scanned, job pages read), the site's `robots.txt`
//! `Disallow` rules for `*` are honored, and only URLs on the company's own
//! site (or an ATS) are kept.

use serde::{Deserialize, Serialize};
use url::Url;

use super::ats;
use super::jsonld::{self, JsonLdPosting};
use crate::careers::{self, BoardRef};
use crate::http::HttpClient;

/// Hard limits per company.
#[derive(Debug, Clone, Copy)]
pub struct SurfaceLimits {
    /// Sitemap files read (an index counts as one).
    pub max_sitemaps: usize,
    /// URLs looked at across those sitemaps.
    pub max_urls: usize,
    /// Job pages read for JSON-LD.
    pub max_job_pages: usize,
}

impl Default for SurfaceLimits {
    fn default() -> Self {
        Self {
            max_sitemaps: 3,
            max_urls: 20_000,
            max_job_pages: 3,
        }
    }
}

/// What the look found, without postings' text.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceSummary {
    /// Whether `robots.txt` answered.
    pub robots: bool,
    /// Sitemap files read.
    pub sitemaps: Vec<String>,
    /// Job-like URLs the sitemaps list.
    pub job_urls: usize,
    /// Pages read for JSON-LD and board links.
    pub pages_read: usize,
    /// `JobPosting` blocks found on them.
    pub jsonld_postings: usize,
    /// Boards found (source keys).
    pub boards: Vec<String>,
    /// Unsupported ATS named.
    pub other_ats: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Surface {
    pub summary: SurfaceSummary,
    pub boards: Vec<BoardRef>,
    pub postings: Vec<JsonLdPosting>,
    pub job_urls: Vec<Url>,
}

/// `Sitemap:` lines and the `Disallow:` rules that apply to every agent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Robots {
    pub sitemaps: Vec<String>,
    pub disallow: Vec<String>,
}

impl Robots {
    pub fn parse(text: &str) -> Self {
        let mut out = Self::default();
        let mut applies = false;
        let mut in_agents = false;
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or_default().trim();
            let Some((field, value)) = line.split_once(':') else {
                continue;
            };
            let field = field.trim().to_ascii_lowercase();
            let value = value.trim();
            match field.as_str() {
                "sitemap" => out.sitemaps.push(value.to_owned()),
                "user-agent" => {
                    if !in_agents {
                        applies = false;
                    }
                    in_agents = true;
                    applies |= value == "*";
                }
                "disallow" => {
                    in_agents = false;
                    if applies && !value.is_empty() {
                        out.disallow.push(value.to_owned());
                    }
                }
                _ => in_agents = false,
            }
        }
        out
    }

    /// Whether the path may be read.
    pub fn allows(&self, url: &Url) -> bool {
        let path = url.path();
        !self.disallow.iter().any(|rule| {
            let rule = rule.trim_end_matches('*');
            !rule.is_empty() && path.starts_with(rule)
        })
    }
}

/// A sitemap file: an index of other sitemaps, or a list of pages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sitemap {
    Index(Vec<String>),
    Urls(Vec<String>),
}

/// Parses a sitemap's `<loc>` entries.
pub fn parse_sitemap(xml: &str) -> Sitemap {
    let mut locs = Vec::new();
    let mut from = 0;
    while let Some(pos) = xml[from..].find("<loc>") {
        let start = from + pos + "<loc>".len();
        let Some(end) = xml[start..].find("</loc>").map(|e| start + e) else {
            break;
        };
        from = end;
        let loc = xml[start..end].trim();
        let loc = loc
            .strip_prefix("<![CDATA[")
            .and_then(|l| l.strip_suffix("]]>"))
            .unwrap_or(loc);
        locs.push(jobhunt_core::html::decode_entities(loc.trim()).into_owned());
    }
    if xml.contains("<sitemapindex") {
        Sitemap::Index(locs)
    } else {
        Sitemap::Urls(locs)
    }
}

const JOB_WORDS: &[&str] = &[
    "careers",
    "career",
    "jobs",
    "job",
    "positions",
    "position",
    "openings",
    "opening",
    "vacancies",
    "vacancy",
    "roles",
    "join-us",
    "work-with-us",
    "open-roles",
];

/// A URL that looks like a job page: on the company's site, with a careers
/// word in its path followed by something more specific
/// (`/careers/senior-engineer`), or any ATS URL.
pub fn is_job_url(url: &Url, site: &str) -> bool {
    if !matches!(ats::classify(url), ats::UrlTarget::Page) {
        return true;
    }
    let on_site = url
        .host_str()
        .and_then(ats::registrable_domain)
        .is_some_and(|d| d == site);
    if !on_site {
        return false;
    }
    let segments: Vec<String> = url
        .path_segments()
        .map(|s| {
            s.filter(|x| !x.is_empty())
                .map(str::to_ascii_lowercase)
                .collect()
        })
        .unwrap_or_default();
    segments
        .iter()
        .position(|s| JOB_WORDS.contains(&s.as_str()))
        .is_some_and(|i| i + 1 < segments.len())
}

/// Whether a sitemap's name suggests it lists jobs.
fn jobby(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    ["career", "job", "position", "opening", "vacanc"]
        .iter()
        .any(|w| lower.contains(w))
}

/// Looks at a company's sitemaps and a few job pages. `careers_url` is read
/// too when given. Never fails: what could not be read is left out.
pub async fn surface(
    http: &HttpClient,
    domain: &str,
    careers_url: Option<&Url>,
    limits: SurfaceLimits,
    site_base: Option<&Url>,
) -> Surface {
    let mut out = Surface::default();
    let Some(home) = site_base
        .cloned()
        .or_else(|| Url::parse(&format!("https://{domain}/")).ok())
    else {
        return out;
    };
    let site = home
        .host_str()
        .and_then(ats::registrable_domain)
        .unwrap_or_else(|| domain.to_owned());
    let robots = match home.join("/robots.txt") {
        Ok(url) => match http.probe(&url).await {
            Ok(p) if (200..300).contains(&p.status) => {
                out.summary.robots = true;
                Robots::parse(&String::from_utf8_lossy(&p.body))
            }
            _ => Robots::default(),
        },
        Err(_) => Robots::default(),
    };
    let mut queue: Vec<String> = robots
        .sitemaps
        .iter()
        .filter(|s| {
            Url::parse(s)
                .ok()
                .and_then(|u| u.host_str().and_then(ats::registrable_domain))
                .as_deref()
                == Some(site.as_str())
                || site_base.is_some()
        })
        .cloned()
        .collect();
    if queue.is_empty()
        && let Ok(url) = home.join("/sitemap.xml")
    {
        queue.push(url.to_string());
    }
    queue.sort_by_key(|s| !jobby(s));
    let mut scanned = 0usize;
    let mut job_urls: Vec<Url> = Vec::new();
    while let Some(next) = (!queue.is_empty()).then(|| queue.remove(0)) {
        if out.summary.sitemaps.len() >= limits.max_sitemaps || scanned >= limits.max_urls {
            break;
        }
        // Compressed sitemaps would need a decoder; skipped.
        if next.ends_with(".gz") || out.summary.sitemaps.contains(&next) {
            continue;
        }
        let Some(url) = Url::parse(&next).ok().filter(|u| robots.allows(u)) else {
            continue;
        };
        let Ok(answer) = http.probe(&url).await else {
            continue;
        };
        out.summary.sitemaps.push(next.clone());
        if !(200..300).contains(&answer.status) {
            continue;
        }
        match parse_sitemap(&String::from_utf8_lossy(&answer.body)) {
            Sitemap::Index(children) => {
                let mut children = children;
                children.sort_by_key(|c| !jobby(c));
                queue.splice(0..0, children);
            }
            Sitemap::Urls(urls) => {
                for loc in urls {
                    scanned += 1;
                    if scanned > limits.max_urls {
                        break;
                    }
                    if let Ok(u) = Url::parse(&loc)
                        && is_job_url(&u, &site)
                        && !job_urls.contains(&u)
                    {
                        job_urls.push(u);
                    }
                }
            }
        }
    }
    out.summary.job_urls = job_urls.len();

    // Boards named directly by sitemap URLs.
    for u in &job_urls {
        match ats::classify(u) {
            ats::UrlTarget::Job { board, .. } | ats::UrlTarget::Board(board) => {
                push_board(&mut out, board);
            }
            ats::UrlTarget::Unsupported { provider, .. } => push_other(&mut out, provider),
            ats::UrlTarget::Page => {}
        }
    }
    // A few pages: the careers page first, then job pages.
    let pages: Vec<Url> = careers_url
        .into_iter()
        .cloned()
        .chain(
            job_urls
                .iter()
                .filter(|u| matches!(ats::classify(u), ats::UrlTarget::Page))
                .cloned(),
        )
        .filter(|u| robots.allows(u))
        .take(limits.max_job_pages + usize::from(careers_url.is_some()))
        .collect();
    for page in pages {
        let Ok(answer) = http.probe(&page).await else {
            continue;
        };
        out.summary.pages_read += 1;
        if !(200..300).contains(&answer.status) {
            continue;
        }
        let html = String::from_utf8_lossy(&answer.body);
        for board in careers::find_boards(&html) {
            push_board(&mut out, board);
        }
        for other in careers::find_other_ats(&html) {
            push_other(&mut out, other.provider);
        }
        for posting in jsonld::job_postings(&html) {
            for u in posting.ats_urls() {
                match ats::classify(&u) {
                    ats::UrlTarget::Job { board, .. } | ats::UrlTarget::Board(board) => {
                        push_board(&mut out, board)
                    }
                    ats::UrlTarget::Unsupported { provider, .. } => push_other(&mut out, provider),
                    ats::UrlTarget::Page => {}
                }
            }
            out.postings.push(posting);
        }
    }
    out.summary.jsonld_postings = out.postings.len();
    out.job_urls = job_urls;
    out
}

fn push_board(out: &mut Surface, board: BoardRef) {
    if !out.boards.contains(&board) {
        if let Some(key) = ats::source_key(&board) {
            out.summary.boards.push(key);
        }
        out.boards.push(board);
    }
}

fn push_other(out: &mut Surface, provider: &str) {
    if !out.summary.other_ats.iter().any(|p| p == provider) {
        out.summary.other_ats.push(provider.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn robots_lists_sitemaps_and_rules_for_everyone() {
        let r = Robots::parse(
            "User-agent: Googlebot\nDisallow: /only-google\n\nUser-agent: *\nDisallow: /admin\nDisallow: /search*\nAllow: /\n\nSitemap: https://acme.com/sitemap_index.xml\nsitemap: https://acme.com/careers-sitemap.xml # jobs\n",
        );
        assert_eq!(
            r.sitemaps,
            [
                "https://acme.com/sitemap_index.xml",
                "https://acme.com/careers-sitemap.xml"
            ]
        );
        assert_eq!(r.disallow, ["/admin", "/search*"]);
        assert!(!r.allows(&Url::parse("https://acme.com/search?q=x").unwrap()));
        assert!(r.allows(&Url::parse("https://acme.com/only-google").unwrap()));
        assert!(r.allows(&Url::parse("https://acme.com/careers/x").unwrap()));
    }

    #[test]
    fn sitemaps_and_indexes() {
        let index = r#"<?xml version="1.0"?><sitemapindex xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
            <sitemap><loc>https://acme.com/post-sitemap.xml</loc></sitemap>
            <sitemap><loc><![CDATA[https://acme.com/job-sitemap.xml]]></loc></sitemap></sitemapindex>"#;
        assert_eq!(
            parse_sitemap(index),
            Sitemap::Index(vec![
                "https://acme.com/post-sitemap.xml".into(),
                "https://acme.com/job-sitemap.xml".into()
            ])
        );
        let urls = "<urlset><url><loc>https://acme.com/careers/senior-engineer?a=1&amp;b=2</loc></url></urlset>";
        assert_eq!(
            parse_sitemap(urls),
            Sitemap::Urls(vec![
                "https://acme.com/careers/senior-engineer?a=1&b=2".into()
            ])
        );
    }

    #[test]
    fn job_urls_are_specific_pages_on_the_site_or_ats_links() {
        let job = |u: &str| is_job_url(&Url::parse(u).unwrap(), "acme.com");
        assert!(job("https://www.acme.com/careers/senior-platform-engineer"));
        assert!(job("https://acme.com/en/jobs/123"));
        assert!(job(
            "https://jobs.lever.co/acme/bfaffe50-ecc7-42c6-8c88-996d920d60df"
        ));
        assert!(job("https://acme.recruitee.com/o/backend"));
        assert!(!job("https://acme.com/careers"), "the careers index itself");
        assert!(!job("https://acme.com/blog/how-we-hire"));
        assert!(!job("https://other.com/careers/x"));
    }
}
