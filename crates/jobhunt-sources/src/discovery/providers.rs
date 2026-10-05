//! Broad discovery providers: public, documented sources of hiring leads.
//!
//! | provider | method | reads | yields |
//! |---|---|---|---|
//! | [`brave_search`] | `ats_search` | Brave Search API (needs a key) | ATS URLs for a generated query |
//! | [`common_crawl`] | `web_index` | Common Crawl's CDX index of an ATS host | every crawled board and job URL |
//! | [`hn_threads`], [`hn_thread`] | `hiring_thread` | Hacker News "Who is hiring?" via the HN Algolia API | ATS links and company domains per post |
//! | [`yc_companies`] | `company_directory` | the yc-oss open dataset of YC companies | hiring companies' domains |
//! | [`remoteintech`] | `company_directory` | the remoteintech directory on GitHub | remote-friendly companies' domains and careers pages |
//!
//! Every request goes through the shared [`HttpClient`] (per-host limits,
//! bounded retries honoring `Retry-After`). None of them is scraped from a
//! page meant for people: each is an API or a published dataset. Search
//! engines' own result pages are never read; a search provider without an
//! API is used by exporting its results and importing them
//! ([`super::import`]).
//!
//! Each fetcher takes a `base` so tests can point it at a local server;
//! `None` is the real host.

use chrono::{DateTime, Utc};
use reqwest::header::{ACCEPT, HeaderMap, HeaderName, HeaderValue};
use serde::Serialize;
use serde_json::Value;
use url::Url;

use super::import::{self, ImportOptions};
use super::queries::SearchQuery;
use super::{Lead, Method, Sighting, ats};
use crate::http::HttpClient;

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("{0}")]
    Http(String),
    #[error("{what}: unexpected response ({detail})")]
    Payload { what: &'static str, detail: String },
}

fn http_error(e: impl std::fmt::Display) -> ProviderError {
    ProviderError::Http(e.to_string())
}

async fn get_json(
    http: &HttpClient,
    url: &Url,
    headers: &HeaderMap,
    what: &'static str,
) -> Result<Value, ProviderError> {
    let answer = http
        .probe_with(url, headers)
        .await
        .map_err(|e| http_error(jobhunt_core::ErrorChain(&e)))?;
    if !(200..300).contains(&answer.status) {
        return Err(ProviderError::Http(format!(
            "{what}: HTTP {} from {url}",
            answer.status
        )));
    }
    serde_json::from_slice(&answer.body).map_err(|e| ProviderError::Payload {
        what,
        detail: e.to_string(),
    })
}

fn join(base: Option<&str>, default: &str, path: &str) -> Result<Url, ProviderError> {
    let base = base.unwrap_or(default).trim_end_matches('/');
    Url::parse(&format!("{base}{path}")).map_err(http_error)
}

// ---------------------------------------------------------------------------
// Search API.

/// Brave Search's web search API. `X-Subscription-Token` carries the key;
/// the key is never logged.
pub async fn brave_search(
    http: &HttpClient,
    base: Option<&str>,
    key: &str,
    query: &SearchQuery,
    count: u32,
    at: DateTime<Utc>,
) -> Result<Vec<Lead>, ProviderError> {
    let mut url = join(base, "https://api.search.brave.com", "/res/v1/web/search")?;
    url.query_pairs_mut()
        .append_pair("q", &query.text())
        .append_pair("count", &count.min(20).to_string());
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
    let token = HeaderValue::from_str(key)
        .map_err(|_| ProviderError::Http("invalid search API key".into()))?;
    headers.insert(HeaderName::from_static("x-subscription-token"), token);
    let body = get_json(http, &url, &headers, "Brave Search").await?;
    Ok(search_results(&body, "brave", query, at))
}

/// Leads from a search API's `web.results[]` (`url`, `title`).
pub fn search_results(
    body: &Value,
    provider: &str,
    query: &SearchQuery,
    at: DateTime<Utc>,
) -> Vec<Lead> {
    let results = body
        .pointer("/web/results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    results
        .iter()
        .enumerate()
        .filter_map(|(i, r)| {
            let url = r.get("url").and_then(Value::as_str)?;
            let mut s = Sighting::new(Method::AtsSearch, provider, url, at);
            s.query = Some(query.text());
            s.family = Some(query.family.clone());
            s.rank = u32::try_from(i + 1).ok();
            s.title = r.get("title").and_then(Value::as_str).map(str::to_owned);
            s.metadata.insert("host".into(), query.host.clone());
            s.metadata.insert("geo".into(), query.geo_group.clone());
            if let Some(sen) = &query.seniority {
                s.metadata.insert("seniority".into(), sen.clone());
            }
            Some(Lead::url(url, s))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Common Crawl.

pub const COMMON_CRAWL: &str = "https://index.commoncrawl.org";

/// How many index pages (one block of about 3,000 URLs each) a crawl has
/// for a host.
pub async fn common_crawl_pages(
    http: &HttpClient,
    base: Option<&str>,
    crawl: &str,
    host: &str,
) -> Result<usize, ProviderError> {
    let mut url = join(base, COMMON_CRAWL, &format!("/{crawl}-index"))?;
    url.query_pairs_mut()
        .append_pair("url", &format!("{host}/*"))
        .append_pair("output", "json")
        .append_pair("showNumPages", "true")
        .append_pair("pageSize", "1");
    let v = get_json(http, &url, &HeaderMap::new(), "Common Crawl index").await?;
    v.get("pages")
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .ok_or(ProviderError::Payload {
            what: "Common Crawl index",
            detail: "no page count".into(),
        })
}

/// One index page of a crawl's URLs for a host, as `web_index` leads.
pub async fn common_crawl(
    http: &HttpClient,
    base: Option<&str>,
    crawl: &str,
    host: &str,
    page: usize,
    at: DateTime<Utc>,
) -> Result<Vec<Lead>, ProviderError> {
    let mut url = join(base, COMMON_CRAWL, &format!("/{crawl}-index"))?;
    url.query_pairs_mut()
        .append_pair("url", &format!("{host}/*"))
        .append_pair("output", "json")
        .append_pair("fl", "url,timestamp,status")
        .append_pair("pageSize", "1")
        .append_pair("page", &page.to_string());
    let answer = http
        .probe(&url)
        .await
        .map_err(|e| http_error(jobhunt_core::ErrorChain(&e)))?;
    if !(200..300).contains(&answer.status) {
        return Err(ProviderError::Http(format!(
            "Common Crawl index: HTTP {}",
            answer.status
        )));
    }
    let text = String::from_utf8_lossy(&answer.body);
    let opts = ImportOptions {
        method: Some(Method::WebIndex),
        provider: format!("commoncrawl:{crawl}"),
        at,
        matrix: None,
    };
    import::parse(&text, &opts).map_err(|detail| ProviderError::Payload {
        what: "Common Crawl index",
        detail,
    })
}

// ---------------------------------------------------------------------------
// Hacker News "Who is hiring?".

pub const HN_ALGOLIA: &str = "https://hn.algolia.com";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HnThread {
    pub id: String,
    pub title: String,
    pub created_at: Option<DateTime<Utc>>,
}

/// The latest monthly "Who is hiring?" threads, newest first.
pub async fn hn_threads(
    http: &HttpClient,
    base: Option<&str>,
    months: usize,
) -> Result<Vec<HnThread>, ProviderError> {
    let mut url = join(base, HN_ALGOLIA, "/api/v1/search_by_date")?;
    url.query_pairs_mut()
        .append_pair("query", "who is hiring")
        .append_pair("tags", "story,author_whoishiring")
        .append_pair("hitsPerPage", &(months * 3 + 3).to_string());
    let v = get_json(http, &url, &HeaderMap::new(), "HN search").await?;
    let hits = v
        .get("hits")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(hits
        .iter()
        .filter_map(|h| {
            let title = h.get("title").and_then(Value::as_str)?;
            title
                .starts_with("Ask HN: Who is hiring?")
                .then(|| HnThread {
                    id: h
                        .get("objectID")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    title: title.to_owned(),
                    created_at: h
                        .get("created_at")
                        .and_then(Value::as_str)
                        .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
                        .map(|t| t.with_timezone(&Utc)),
                })
        })
        .take(months)
        .collect())
}

/// Every top-level post of a thread, as leads.
pub async fn hn_thread(
    http: &HttpClient,
    base: Option<&str>,
    id: &str,
    at: DateTime<Utc>,
) -> Result<Vec<Lead>, ProviderError> {
    let url = join(base, HN_ALGOLIA, &format!("/api/v1/items/{id}"))?;
    let v = get_json(http, &url, &HeaderMap::new(), "HN item").await?;
    Ok(hn_thread_leads(&v, at))
}

/// One "Who is hiring?" post.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HnPost {
    /// The first line: `Company | Role | Location | …`.
    pub header: String,
    pub company: Option<String>,
    /// Every absolute URL in the post, in order.
    pub urls: Vec<Url>,
}

pub fn parse_hn_post(html: &str) -> HnPost {
    let decoded = jobhunt_core::html::decode_entities(html).into_owned();
    let first = decoded.split("<p>").next().unwrap_or_default();
    let header = strip_tags(first);
    let company = header
        .split('|')
        .next()
        .map(|c| c.split(" (").next().unwrap_or(c).trim().to_owned())
        .filter(|c| !c.is_empty() && c.len() <= 80);
    let mut urls: Vec<Url> = Vec::new();
    for (_, u) in crate::careers::absolute_urls(&decoded) {
        if !urls.contains(&u) {
            urls.push(u);
        }
    }
    HnPost {
        header,
        company,
        urls,
    }
}

fn strip_tags(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The company's own domain among a post's links: the one named like the
/// company, else the first that can be a company's.
fn post_domain(post: &HnPost) -> Option<String> {
    let squash = |s: &str| {
        s.chars()
            .filter(char::is_ascii_alphanumeric)
            .collect::<String>()
            .to_ascii_lowercase()
    };
    let domains: Vec<String> = post
        .urls
        .iter()
        .filter_map(ats::company_domain_of)
        .collect();
    let name = post.company.as_deref().map(squash).unwrap_or_default();
    domains
        .iter()
        .find(|d| {
            let stem = squash(d.split('.').next().unwrap_or_default());
            stem.len() >= 3 && name.len() >= 3 && (name.contains(&stem) || stem.contains(&name))
        })
        .or_else(|| domains.first())
        .cloned()
}

/// Place words in a post's header, for the per-strategy geography split.
fn header_geo(header: &str) -> Vec<&'static str> {
    let lower = header.to_ascii_lowercase();
    let mut out = Vec::new();
    for (word, tag) in [
        ("remote", "remote"),
        ("worldwide", "global"),
        ("global", "global"),
        ("anywhere", "global"),
        ("latam", "latam"),
        ("latin america", "latam"),
        ("south america", "latam"),
        ("brazil", "brazil"),
        ("americas", "americas"),
    ] {
        if lower.contains(word) && !out.contains(&tag) {
            out.push(tag);
        }
    }
    out
}

/// Leads from a thread's JSON (the HN Algolia `items` shape): each ATS
/// link of a post, with the post's company and the first company domain it
/// links as hints; a post without an ATS link gives its company domain.
pub fn hn_thread_leads(thread: &Value, at: DateTime<Utc>) -> Vec<Lead> {
    let story = thread
        .get("id")
        .map(|v| v.to_string().trim_matches('"').to_owned())
        .unwrap_or_default();
    let provider = format!("hn:{story}");
    let mut out = Vec::new();
    for post in thread
        .get("children")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(text) = post.get("text").and_then(Value::as_str) else {
            continue;
        };
        let item = post
            .get("id")
            .map(|v| v.to_string().trim_matches('"').to_owned())
            .unwrap_or_default();
        let parsed = parse_hn_post(text);
        let domain = post_domain(&parsed);
        let sighting = |url: &str| {
            let mut s = Sighting::new(Method::HiringThread, &provider, url, at);
            s.title = Some(parsed.header.chars().take(200).collect());
            s.metadata.insert("hn_item".into(), item.clone());
            let geo = header_geo(&parsed.header);
            if !geo.is_empty() {
                s.metadata.insert("geo".into(), geo.join(","));
            }
            s
        };
        let mut any_ats = false;
        for url in &parsed.urls {
            if matches!(ats::classify(url), ats::UrlTarget::Page) {
                continue;
            }
            any_ats = true;
            out.push(
                Lead::url(url.as_str(), sighting(url.as_str()))
                    .with_company(parsed.company.clone())
                    .with_domain(domain.clone()),
            );
        }
        if !any_ats && let Some(d) = &domain {
            let link = format!("https://news.ycombinator.com/item?id={item}");
            out.push(Lead::domain(d, sighting(&link)).with_company(parsed.company.clone()));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Company directories.

pub const YC_OSS: &str = "https://yc-oss.github.io";

/// Hiring, active YC companies from the yc-oss dataset
/// (`/api/companies/hiring.json`).
pub async fn yc_companies(
    http: &HttpClient,
    base: Option<&str>,
    at: DateTime<Utc>,
) -> Result<Vec<Lead>, ProviderError> {
    let url = join(base, YC_OSS, "/api/companies/hiring.json")?;
    let v = get_json(http, &url, &HeaderMap::new(), "yc-oss").await?;
    Ok(yc_leads(&v, at))
}

pub fn yc_leads(companies: &Value, at: DateTime<Utc>) -> Vec<Lead> {
    companies
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c.get("isHiring").and_then(Value::as_bool).unwrap_or(true))
        .filter(|c| {
            c.get("status")
                .and_then(Value::as_str)
                .is_none_or(|s| s == "Active")
        })
        .filter_map(|c| {
            let website = c.get("website").and_then(Value::as_str)?;
            let domain = Url::parse(website)
                .ok()
                .as_ref()
                .and_then(ats::company_domain_of)?;
            let mut s = Sighting::new(Method::CompanyDirectory, "yc-oss", website, at);
            let text = |k: &str| c.get(k).and_then(Value::as_str).map(str::to_owned);
            if let Some(regions) = c.get("regions").and_then(Value::as_array) {
                let r: Vec<&str> = regions.iter().filter_map(Value::as_str).collect();
                s.metadata.insert("regions".into(), r.join(", "));
            }
            for k in ["batch", "slug", "industry"] {
                if let Some(v) = text(k) {
                    s.metadata.insert(k.into(), v);
                }
            }
            if let Some(n) = c.get("team_size").and_then(Value::as_u64) {
                s.metadata.insert("team_size".into(), n.to_string());
            }
            Some(Lead::domain(&domain, s).with_company(text("name")))
        })
        .collect()
}

/// A remoteintech company profile's front matter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DirectoryEntry {
    pub name: Option<String>,
    pub website: Option<String>,
    pub careers_url: Option<String>,
    pub region: Option<String>,
    pub remote_policy: Option<String>,
}

/// Reads `key: value` lines between the leading `---` fences.
pub fn parse_front_matter(md: &str) -> DirectoryEntry {
    let mut entry = DirectoryEntry::default();
    let mut lines = md.lines();
    if lines.next().map(str::trim) != Some("---") {
        return entry;
    }
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let v = v.trim().trim_matches('"').trim_matches('\'').trim();
        if v.is_empty() {
            continue;
        }
        let v = Some(v.to_owned());
        match k.trim() {
            "title" => entry.name = v,
            "website" => entry.website = v,
            "careers_url" => entry.careers_url = v,
            "region" => entry.region = v,
            "remote_policy" => entry.remote_policy = v,
            _ => {}
        }
    }
    entry
}

pub fn directory_lead(entry: &DirectoryEntry, provider: &str, at: DateTime<Utc>) -> Option<Lead> {
    let website = entry.website.as_deref()?;
    let domain = Url::parse(website)
        .ok()
        .as_ref()
        .and_then(ats::company_domain_of)?;
    let found = entry.careers_url.as_deref().unwrap_or(website);
    let mut s = Sighting::new(Method::CompanyDirectory, provider, found, at);
    for (k, v) in [
        ("region", &entry.region),
        ("remote_policy", &entry.remote_policy),
    ] {
        if let Some(v) = v {
            s.metadata.insert(k.into(), v.clone());
        }
    }
    let lead = match entry
        .careers_url
        .as_deref()
        .filter(|u| u.starts_with("http"))
    {
        Some(careers) => Lead::url(careers, s).with_domain(Some(domain)),
        None => Lead::domain(&domain, s),
    };
    Some(lead.with_company(entry.name.clone()))
}

const REMOTEINTECH_TREE: &str =
    "https://api.github.com/repos/remoteintech/remote-jobs/git/trees/main?recursive=1";
const REMOTEINTECH_RAW: &str = "https://raw.githubusercontent.com/remoteintech/remote-jobs/main/";

/// Every company profile of the remoteintech directory (one GitHub API
/// request for the file list, then one raw file per company).
pub async fn remoteintech(
    http: &HttpClient,
    concurrency: usize,
    at: DateTime<Utc>,
) -> Result<Vec<Lead>, ProviderError> {
    use futures::StreamExt;
    let tree = get_json(
        http,
        &Url::parse(REMOTEINTECH_TREE).map_err(http_error)?,
        &HeaderMap::new(),
        "GitHub tree",
    )
    .await?;
    let paths: Vec<String> = tree
        .get("tree")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|t| t.get("path").and_then(Value::as_str))
        .filter(|p| p.starts_with("src/companies/") && p.ends_with(".md"))
        .map(str::to_owned)
        .collect();
    let leads: Vec<Option<Lead>> = futures::stream::iter(paths)
        .map(|path| async move {
            let url = Url::parse(&format!("{REMOTEINTECH_RAW}{path}")).ok()?;
            let body = http.get_bytes(&url).await.ok()?;
            directory_lead(
                &parse_front_matter(&String::from_utf8_lossy(&body)),
                "remoteintech",
                at,
            )
        })
        .buffer_unordered(concurrency.max(1))
        .collect()
        .await;
    Ok(leads.into_iter().flatten().collect())
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use serde_json::json;

    use super::*;
    use crate::discovery::{CandidateKind, DiscoveryStore};

    fn at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 5, 12, 0, 0).unwrap()
    }

    #[test]
    fn hiring_posts_give_ats_links_with_the_company_domain() {
        let thread = json!({"id": 49922569, "children": [
            {"id": 1, "text": "Moxie | Staff Platform Engineer | REMOTE (LATAM) | Full-time<p>We build. <a href=\"https:&#x2F;&#x2F;www.joinmoxie.com\">https:&#x2F;&#x2F;www.joinmoxie.com</a> Apply: <a href=\"https:&#x2F;&#x2F;jobs.ashbyhq.com&#x2F;moxie&#x2F;36c5bcce-5a3d-4394-9eeb-68d812f9e753\">link</a>"},
            {"id": 2, "text": "Acme (YC W24) | Backend | Remote, Worldwide<p>Our paper: https:&#x2F;&#x2F;example-journal.org&#x2F;x See https:&#x2F;&#x2F;acme.dev&#x2F;careers"},
            {"id": 3, "text": "Nothing to link | SF onsite"},
            {"id": 4}
        ]});
        let leads = hn_thread_leads(&thread, at());
        assert_eq!(leads.len(), 2);
        assert_eq!(leads[0].domain.as_deref(), Some("joinmoxie.com"));
        assert_eq!(leads[0].company.as_deref(), Some("Moxie"));
        assert_eq!(leads[0].sighting.provider, "hn:49922569");
        assert_eq!(
            leads[0].sighting.metadata.get("geo").map(String::as_str),
            Some("remote,latam")
        );
        assert_eq!(leads[1].domain.as_deref(), Some("acme.dev"));
        assert_eq!(leads[1].company.as_deref(), Some("Acme"));
        assert_eq!(
            leads[1].sighting.metadata.get("geo").map(String::as_str),
            Some("remote,global")
        );

        let mut store = DiscoveryStore::new();
        for l in leads {
            store.add(l);
        }
        let board = store.get("board:ashby:moxie").unwrap();
        assert_eq!(board.domain_hint.as_deref(), Some("joinmoxie.com"));
        assert_eq!(store.of_kind(CandidateKind::Company).count(), 1);
    }

    #[test]
    fn yc_directory_gives_hiring_company_domains() {
        let data = json!([
            {"name": "Acme", "website": "https://www.acme.com", "isHiring": true, "status": "Active", "regions": ["Remote", "Latin America"], "batch": "Winter 2024", "team_size": 12},
            {"name": "Gone", "website": "https://gone.com", "isHiring": true, "status": "Inactive"},
            {"name": "Full", "website": "https://full.com", "isHiring": false, "status": "Active"},
            {"name": "NoSite", "isHiring": true}
        ]);
        let leads = yc_leads(&data, at());
        assert_eq!(leads.len(), 1);
        assert_eq!(leads[0].domain.as_deref(), Some("acme.com"));
        assert_eq!(
            leads[0]
                .sighting
                .metadata
                .get("regions")
                .map(String::as_str),
            Some("Remote, Latin America")
        );
        assert_eq!(leads[0].sighting.method, Method::CompanyDirectory);
    }

    #[test]
    fn directory_front_matter() {
        let md = "---\ntitle: \"GitLab\"\nslug: gitlab\nwebsite: https://about.gitlab.com/\ncareers_url: https://about.gitlab.com/jobs/\nregion: worldwide\nremote_policy: fully-remote\n---\n\n## Company blurb\n";
        let e = parse_front_matter(md);
        assert_eq!(e.name.as_deref(), Some("GitLab"));
        assert_eq!(e.region.as_deref(), Some("worldwide"));
        let lead = directory_lead(&e, "remoteintech", at()).unwrap();
        assert_eq!(lead.url.as_deref(), Some("https://about.gitlab.com/jobs/"));
        assert_eq!(lead.domain.as_deref(), Some("gitlab.com"));
        assert_eq!(
            parse_front_matter("no front matter"),
            DirectoryEntry::default()
        );
    }

    #[test]
    fn search_api_results_are_ranked_leads() {
        let q = crate::discovery::queries::QueryMatrix::builtin()
            .plan(crate::discovery::queries::Plan::Pairwise, 0)
            .remove(0);
        let body = json!({"web": {"results": [
            {"url": "https://jobs.ashbyhq.com/acme/11111111-1111-1111-1111-111111111111", "title": "Engineer @ Acme"},
            {"url": "https://jobs.ashbyhq.com/beta"}
        ]}});
        let leads = search_results(&body, "brave", &q, at());
        assert_eq!(leads.len(), 2);
        assert_eq!(leads[1].sighting.rank, Some(2));
        assert_eq!(leads[0].sighting.query.as_deref(), Some(q.text().as_str()));
        assert_eq!(leads[0].sighting.family.as_deref(), Some(q.family.as_str()));
    }
}
