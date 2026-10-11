//! First-party timing probe (BRU-360; see
//! `docs/broad-discovery-experiment-2026-10-05.md`, "Timing"). Not part of
//! the product.
//!
//! A freshness advantage can only be measured for postings published
//! after monitoring began. Monitor a cohort of boards (scan it with
//! `narrow find --raw --refresh` on a schedule), then run this probe at
//! +0, +1, +3 and +7 days with the same `since`: it lists every open job
//! first seen after `since` whose publish date (when known) is after it
//! too, and checks a control (Himalayas' public search API) for each,
//! appending one JSON line per job and check.
//!
//! ```text
//! cargo run --release -p jobhunt-cli --example first_party_timing -- \
//!     <config.toml> <jobhunt.db> <since: RFC 3339> >> timing.jsonl
//! ```
//!
//! A control hit needs the same company and a compatible title. "Absent"
//! means absent at the time of the check, nothing more.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stdout)]

use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Utc};
use jobhunt_app::LocalApp;
use jobhunt_jobs::{JobQuery, JobStatus};
use jobhunt_sources::{HttpClient, HttpSettings};
use serde_json::{Value, json};
use url::Url;

fn squash(s: &str) -> String {
    s.chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase()
}

/// The control's listing of this job, if any: same company, compatible
/// title (one title's words contain the other's).
async fn himalayas(http: &HttpClient, company: &str, title: &str) -> Value {
    let mut found = None;
    let mut company_listed = false;
    for q in [format!("{title} {company}"), title.to_owned()] {
        let mut url = Url::parse("https://himalayas.app/jobs/api/search").unwrap();
        url.query_pairs_mut().append_pair("q", &q);
        let Ok(answer) = http.probe(&url).await else {
            return json!({"error": "unreachable"});
        };
        if answer.status != 200 {
            return json!({"error": format!("HTTP {}", answer.status)});
        }
        let body: Value = serde_json::from_slice(&answer.body).unwrap_or(Value::Null);
        for job in body["jobs"].as_array().into_iter().flatten() {
            let same_company =
                squash(job["companyName"].as_str().unwrap_or_default()) == squash(company);
            company_listed |= same_company;
            let (a, b) = (
                squash(job["title"].as_str().unwrap_or_default()),
                squash(title),
            );
            if same_company && !a.is_empty() && (a.contains(&b) || b.contains(&a)) {
                found = Some(json!({
                    "title": job["title"],
                    "published": job["pubDate"].as_str().and_then(|s| s.parse::<i64>().ok())
                        .and_then(|t| DateTime::from_timestamp(t, 0)),
                    "link": job["applicationLink"],
                }));
            }
        }
        if found.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    json!({"present": found.is_some(), "company_listed": company_listed, "listing": found})
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let since: DateTime<Utc> = args[3].parse()?;
    let loaded =
        jobhunt_app::config::load(Some(Path::new(&args[1])), Some(Path::new(&args[2])), None)?;
    let app = LocalApp::open(loaded).await?;
    let records = app
        .store()
        .search(&JobQuery {
            status: Some(JobStatus::Open),
            ..JobQuery::default()
        })
        .await?;
    let http = HttpClient::new(HttpSettings::default())?;
    let now = Utc::now();
    for r in records.iter().filter(|r| {
        r.first_seen_at > since
            && r.posting
                .posted_at
                .is_none_or(|p| p > since - chrono::Duration::hours(1))
    }) {
        let control = himalayas(&http, &r.posting.company, &r.posting.title).await;
        println!(
            "{}",
            json!({
                "job": r.id.to_string(),
                "source": r.posting.provenance.source.to_string(),
                "company": r.posting.company,
                "title": r.posting.title,
                "url": r.posting.url.as_str(),
                "posted_at": r.posting.posted_at,
                "first_seen_at": r.first_seen_at,
                "checked_at": now,
                "hours_since_first_seen": (now - r.first_seen_at).num_minutes() as f64 / 60.0,
                "himalayas": control,
            })
        );
    }
    app.close().await;
    Ok(())
}
