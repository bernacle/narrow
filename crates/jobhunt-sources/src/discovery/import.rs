//! Leads from files: search results exported from any search provider, a
//! crawl index, a list of URLs or domains, a CSV.
//!
//! The architecture does not depend on one search vendor: any provider's
//! results can be saved and imported. Recognized shapes:
//!
//! * JSON: `{"provider": "…", "results": [{"query": "…", "url": "…",
//!   "title": "…", "rank": 1}, …]}` or a bare array of such results;
//! * JSON lines of the same objects, including Common Crawl's CDX index
//!   output (`{"url": "…", "timestamp": "20260910…", "status": "200"}`);
//! * CSV with a header naming `url` and/or `domain` (and optionally
//!   `company`, `query`, `title`, `rank`);
//! * plain text: one URL or domain per line, `#` comments.
//!
//! The method is `ats_search` for results that carry a query,
//! `web_index` for crawl-index lines, and `manual_import` otherwise, unless
//! one is given. A query's family is recovered from its text with the
//! query matrix ([`super::queries::QueryMatrix::read`]).

use chrono::{DateTime, NaiveDateTime, Utc};
use serde_json::Value;

use super::queries::QueryMatrix;
use super::{Lead, Method, Sighting};

/// How to read a file.
#[derive(Debug, Clone)]
pub struct ImportOptions<'a> {
    /// Overrides the method inferred from each result.
    pub method: Option<Method>,
    /// The provider name when the file doesn't give one (usually the file
    /// name).
    pub provider: String,
    /// When the leads were found (now, for a file of fresh results).
    pub at: DateTime<Utc>,
    pub matrix: Option<&'a QueryMatrix>,
}

/// Reads leads from a file's text. Errors name the line or the problem.
pub fn parse(text: &str, opts: &ImportOptions<'_>) -> Result<Vec<Lead>, String> {
    let trimmed = text.trim_start();
    if trimmed.starts_with('[')
        || (trimmed.starts_with('{') && serde_json::from_str::<Value>(trimmed).is_ok())
    {
        let value: Value =
            serde_json::from_str(trimmed).map_err(|e| format!("invalid JSON: {e}"))?;
        let (provider, results) = match &value {
            Value::Array(items) => (None, items.clone()),
            Value::Object(map) => (
                map.get("provider")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                map.get("results")
                    .and_then(Value::as_array)
                    .cloned()
                    .ok_or("a JSON object needs a \"results\" array")?,
            ),
            _ => return Err("expected a JSON array or object".into()),
        };
        let provider = provider.unwrap_or_else(|| opts.provider.clone());
        return Ok(results
            .iter()
            .filter_map(|r| lead_from_json(r, &provider, opts))
            .collect());
    }
    if trimmed.starts_with('{') {
        let mut out = Vec::new();
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let value: Value =
                serde_json::from_str(line).map_err(|e| format!("line {}: {e}", n + 1))?;
            if let Some(lead) = lead_from_json(&value, &opts.provider, opts) {
                out.push(lead);
            }
        }
        return Ok(out);
    }
    let first = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .unwrap_or_default();
    let header = first.to_ascii_lowercase();
    if header.contains(',')
        && header
            .split(',')
            .any(|c| matches!(c.trim().trim_matches('"'), "url" | "domain"))
    {
        return parse_csv(text, opts);
    }
    Ok(text
        .lines()
        .map(|l| l.split('#').next().unwrap_or_default().trim())
        .filter(|l| !l.is_empty())
        .map(|line| plain_lead(line, None, None, opts))
        .collect())
}

fn plain_lead(
    value: &str,
    company: Option<String>,
    query: Option<&str>,
    opts: &ImportOptions<'_>,
) -> Lead {
    let is_url = value.starts_with("http://") || value.starts_with("https://");
    let method = opts.method.unwrap_or(if query.is_some() {
        Method::AtsSearch
    } else {
        Method::ManualImport
    });
    let mut s = Sighting::new(method, &opts.provider, value, opts.at);
    set_query(&mut s, query, None, opts);
    let lead = if is_url {
        Lead::url(value, s)
    } else {
        Lead::domain(value, s)
    };
    lead.with_company(company)
}

fn set_query(
    s: &mut Sighting,
    query: Option<&str>,
    family: Option<&str>,
    opts: &ImportOptions<'_>,
) {
    let Some(query) = query.map(str::trim).filter(|q| !q.is_empty()) else {
        return;
    };
    s.query = Some(query.to_owned());
    s.family = family
        .map(str::to_owned)
        .or_else(|| opts.matrix.and_then(|m| m.read(query)).map(|q| q.family));
    if let Some(q) = opts.matrix.and_then(|m| m.read(query)) {
        s.metadata.insert("host".into(), q.host);
        s.metadata.insert("geo".into(), q.geo_group);
        if let Some(sen) = q.seniority {
            s.metadata.insert("seniority".into(), sen);
        }
    }
}

fn str_field<'a>(v: &'a Value, names: &[&str]) -> Option<&'a str> {
    names
        .iter()
        .find_map(|n| v.get(*n).and_then(Value::as_str))
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn lead_from_json(v: &Value, provider: &str, opts: &ImportOptions<'_>) -> Option<Lead> {
    let url = str_field(v, &["url", "link", "href"]);
    let domain = str_field(v, &["domain", "website"]);
    if url.is_none() && domain.is_none() {
        return None;
    }
    let query = str_field(v, &["query", "q"]);
    let crawl = str_field(v, &["timestamp"]);
    let method = opts.method.unwrap_or(if query.is_some() {
        Method::AtsSearch
    } else if crawl.is_some() {
        Method::WebIndex
    } else {
        Method::ManualImport
    });
    let provider = str_field(v, &["provider"]).unwrap_or(provider);
    let at = str_field(v, &["discovered_at", "executed_at"])
        .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
        .map_or(opts.at, |t| t.with_timezone(&Utc));
    let mut s = Sighting::new(method, provider, url.or(domain).unwrap_or_default(), at);
    set_query(&mut s, query, str_field(v, &["family"]), opts);
    s.title = str_field(v, &["title"]).map(str::to_owned);
    s.rank = v
        .get("rank")
        .and_then(|r| {
            r.as_u64()
                .or_else(|| r.as_str().and_then(|s| s.parse().ok()))
        })
        .and_then(|r| u32::try_from(r).ok());
    if let Some(ts) = crawl {
        if let Ok(t) = NaiveDateTime::parse_from_str(ts, "%Y%m%d%H%M%S") {
            s.metadata
                .insert("crawled_at".into(), t.and_utc().to_rfc3339());
        }
        if let Some(status) = str_field(v, &["status"]) {
            s.metadata.insert("crawl_status".into(), status.to_owned());
        }
    }
    if let Some(Value::Object(meta)) = v.get("metadata") {
        for (k, val) in meta {
            if let Some(text) = val.as_str() {
                s.metadata.insert(k.clone(), text.to_owned());
            }
        }
    }
    let company = str_field(v, &["company", "name"]).map(str::to_owned);
    let mut lead = match url {
        Some(u) => Lead::url(u, s).with_domain(domain.map(str::to_owned)),
        None => Lead::domain(domain.unwrap_or_default(), s),
    };
    lead.company = company;
    Some(lead)
}

fn parse_csv(text: &str, opts: &ImportOptions<'_>) -> Result<Vec<Lead>, String> {
    let body: String = text
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .trim(csv::Trim::All)
        .from_reader(body.as_bytes());
    let headers: Vec<String> = reader
        .headers()
        .map_err(|e| format!("CSV header: {e}"))?
        .iter()
        .map(str::to_ascii_lowercase)
        .collect();
    let col = |name: &str| headers.iter().position(|h| h == name);
    let (url, domain, company, query, title, rank) = (
        col("url"),
        col("domain").or(col("website")),
        col("company").or(col("name")),
        col("query"),
        col("title"),
        col("rank"),
    );
    let mut out = Vec::new();
    for (n, record) in reader.records().enumerate() {
        let record = record.map_err(|e| format!("CSV line {}: {e}", n + 2))?;
        let get = |i: Option<usize>| {
            i.and_then(|i| record.get(i))
                .map(str::trim)
                .filter(|s| !s.is_empty())
        };
        let value = get(url).or(get(domain));
        let Some(value) = value else { continue };
        let mut lead = plain_lead(value, get(company).map(str::to_owned), get(query), opts);
        if get(url).is_some() {
            lead.domain = get(domain).map(str::to_owned);
        }
        lead.sighting.title = get(title).map(str::to_owned);
        lead.title = lead.sighting.title.clone();
        lead.sighting.rank = get(rank).and_then(|r| r.parse().ok());
        out.push(lead);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::discovery::{CandidateKind, DiscoveryStore};

    fn opts(matrix: Option<&QueryMatrix>) -> ImportOptions<'_> {
        ImportOptions {
            method: None,
            provider: "results.json".into(),
            at: Utc.with_ymd_and_hms(2026, 10, 5, 12, 0, 0).unwrap(),
            matrix,
        }
    }

    #[test]
    fn search_results_keep_query_family_and_rank() {
        let m = QueryMatrix::builtin();
        let text = r#"{"provider": "websearch", "results": [
            {"query": "site:jobs.ashbyhq.com \"staff platform engineer\" LATAM", "rank": 1,
             "url": "https://jobs.ashbyhq.com/moxie/36c5bcce-5a3d-4394-9eeb-68d812f9e753", "title": "Staff Platform Engineer @ Moxie"},
            {"query": "site:jobs.ashbyhq.com \"staff platform engineer\" LATAM", "rank": 2,
             "url": "https://jobs.ashbyhq.com/moxie/36c5bcce-5a3d-4394-9eeb-68d812f9e753"}
        ]}"#;
        let leads = parse(text, &opts(Some(&m))).unwrap();
        assert_eq!(leads.len(), 2);
        let s = &leads[0].sighting;
        assert_eq!(s.method, Method::AtsSearch);
        assert_eq!(s.provider, "websearch");
        assert_eq!(s.family.as_deref(), Some("platform engineer · latam"));
        assert_eq!(s.rank, Some(1));
        assert_eq!(
            s.metadata.get("seniority").map(String::as_str),
            Some("staff")
        );
        // Duplicate results of one query collapse into one job, one board.
        let mut store = DiscoveryStore::new();
        for lead in leads {
            store.add(lead);
        }
        assert_eq!(store.of_kind(CandidateKind::Job).count(), 1);
        assert_eq!(store.of_kind(CandidateKind::Board).count(), 1);
    }

    #[test]
    fn crawl_index_lines_are_web_index_leads() {
        let text = "{\"url\": \"https://jobs.ashbyhq.com/acme/11111111-1111-1111-1111-111111111111\", \"timestamp\": \"20260910120000\", \"status\": \"200\"}\n\
                    {\"url\": \"https://job-boards.greenhouse.io/acme\", \"timestamp\": \"20260911000000\", \"status\": \"302\"}\n";
        let leads = parse(text, &opts(None)).unwrap();
        assert_eq!(leads.len(), 2);
        assert_eq!(leads[0].sighting.method, Method::WebIndex);
        assert_eq!(
            leads[0]
                .sighting
                .metadata
                .get("crawled_at")
                .map(String::as_str),
            Some("2026-09-10T12:00:00+00:00")
        );
        assert_eq!(
            leads[1]
                .sighting
                .metadata
                .get("crawl_status")
                .map(String::as_str),
            Some("302")
        );
    }

    #[test]
    fn csv_and_plain_lists() {
        let csv = "# companies\ndomain,company\nacme.com,Acme\n\"beta.io\",Beta Inc\n";
        let leads = parse(csv, &opts(None)).unwrap();
        assert_eq!(leads.len(), 2);
        assert_eq!(leads[1].domain.as_deref(), Some("beta.io"));
        assert_eq!(leads[1].company.as_deref(), Some("Beta Inc"));
        assert_eq!(leads[0].sighting.method, Method::ManualImport);

        let plain = "https://jobs.lever.co/acme\n# a comment\nacme.com\n";
        let leads = parse(plain, &opts(None)).unwrap();
        assert_eq!(leads.len(), 2);
        assert!(leads[0].url.is_some() && leads[1].domain.is_some());

        let csv_urls =
            "query,url,title\nsite:jobs.lever.co \"sre\" remote,https://jobs.lever.co/acme,Acme\n";
        let leads = parse(csv_urls, &opts(None)).unwrap();
        assert_eq!(leads[0].sighting.method, Method::AtsSearch);
        assert_eq!(leads[0].title.as_deref(), Some("Acme"));
    }
}
