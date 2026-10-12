//! What the next daily digest would contain (notification policy v2,
//! BRU-389 slice A), over a local mirror of a person's data. Sends nothing.
//! Not part of the product.
//!
//! It runs the production selection ([`jobhunt_app::LocalApp::digest`]) and
//! renderer ([`jobhunt_cloud::notify::render_digest`]) over a SQLite mirror
//! (the person's synced profile plus a scan of the cloud's sources; see
//! `docs/email-digest-policy-v2.md`), and prints every Strong job, why each
//! one is in the digest or not, and the email as text.
//!
//! ```text
//! cargo run --release -p jobhunt-cloud --example digest_dry_run -- \
//!     <config.toml> <mirror.db> <now: RFC 3339> [verify] [size] [shown opp ids] > digest.json
//! ```
//!
//! `verify` first verifies the Strong jobs at their sources, as the cloud's
//! verification worker does for recommendations (network; it changes the
//! mirror's verification state, never the cloud's). A mirror has no
//! per-person feed marks: jobs the person already saw on Today in the
//! cloud would be left out there; the output lists Today's picks so they
//! can be told apart, and `shown` (comma-separated `opp_…`) leaves out the
//! ones the cloud recorded as shown, as its feed marks would.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stdout)]

use std::collections::HashSet;
use std::path::Path;

use chrono::{DateTime, Utc};
use jobhunt_app::feed::{DigestRequest, FeedRequest};
use jobhunt_app::resolve::Opportunity;
use jobhunt_app::{LocalApp, Quiet, RefreshMode};
use jobhunt_jobs::verification::VerifyMode;
use jobhunt_ranking::{Gate, RankQuery, Tier};
use serde_json::{Value, json};

fn gate(g: &Gate) -> String {
    match g {
        Gate::Recommended => "recommended".into(),
        Gate::VerifyFirst { why } => format!("verify first: {why}"),
        Gate::EligibilityUnclear { why } => format!("eligibility unclear: {why}"),
        Gate::Excluded { exclusion } => format!("excluded: {}", exclusion.label()),
    }
}

async fn strong(app: &LocalApp, now: DateTime<Utc>) -> Vec<Value> {
    let query = RankQuery {
        text: String::new(),
        store_top: 0,
        all: false,
    };
    let report = app.ranking().rank(&query, now).await.unwrap();
    let mut out = Vec::new();
    for (i, r) in report.rankings.iter().enumerate() {
        if r.tier != Tier::StrongFit {
            continue;
        }
        let record = app.store().get(r.job).await.unwrap();
        out.push(json!({
            "rank": i + 1,
            "opportunity": r.opportunity.to_string(),
            "company": r.company,
            "title": r.title,
            "gate": gate(&r.gate),
            "source": record.as_ref().map(|r| r.posting.provenance.source.to_string()),
            "posted_at": record.as_ref().and_then(|r| r.posting.posted_at).map(|d| d.to_rfc3339()),
            "first_seen_at": record.as_ref().map(|r| r.first_seen_at.to_rfc3339()),
        }));
    }
    out
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let now: DateTime<Utc> = args[3].parse()?;
    let verify = args.get(4).is_some_and(|a| a == "verify");
    let size: usize = args.get(5).map_or(Ok(3), |a| a.parse())?;
    let loaded =
        jobhunt_app::config::load(Some(Path::new(&args[1])), Some(Path::new(&args[2])), None)?;
    let app = LocalApp::open(loaded).await?;

    let before = strong(&app, now).await;
    let mut verified = Vec::new();
    if verify {
        for s in &before {
            let id = s["opportunity"].as_str().unwrap().parse()?;
            let records = app.store().opportunity_records(id).await?;
            let checked = app
                .verify(&Opportunity { id, records }, VerifyMode::Force, &Quiet, now)
                .await?;
            verified.push(json!({
                "opportunity": s["opportunity"],
                "company": s["company"],
                "state": format!("{:?}", checked.trust.state),
            }));
        }
    }
    let after = strong(&app, now).await;

    // Today, as the feed selects it (locally nothing is recorded as shown).
    let feed = app
        .feed(
            &FeedRequest {
                limit: jobhunt_app::feed::DEFAULT_FEED_LIMIT,
                verify: false,
                refresh: RefreshMode::Never,
            },
            &Quiet,
            now,
        )
        .await?;
    let today: Vec<Value> = feed
        .entries
        .iter()
        .map(|e| {
            json!({
                "company": e.entry.ranking.company,
                "title": e.entry.ranking.title,
                "also": e.also_at_company.iter().map(|s| s.title.clone()).collect::<Vec<_>>(),
            })
        })
        .collect();

    let none = HashSet::new();
    let shown: HashSet<String> = args
        .get(6)
        .map(|a| {
            a.split(',')
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let digest = app
        .digest(
            &DigestRequest {
                exclude: &shown,
                size,
                recent_companies: &none,
            },
            now,
        )
        .await?;
    let items: Vec<Value> = digest
        .items
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let item = &c.item.item;
            json!({
                "position": i + 1,
                "company": item.company,
                "title": item.title,
                "eligibility": item.eligibility.status,
                "eligibility_headline": item.eligibility.headline,
                "freshness": c.freshness.class.as_str(),
                "freshness_label": c.freshness.label(&item.company, now),
                "reasons_shown": jobhunt_cloud::notify::reasons(c),
                "brief_reasons": item.why,
                "pay_shown": c.pay.as_ref().map(|p| p.compact()),
                "consider_not_shown": item.consider,
                "locations": item.locations,
                "workplace": item.workplace,
            })
        })
        .collect();
    let web = url::Url::parse("https://narrow.fyi/")?;
    let email = jobhunt_cloud::notify::render_digest(
        "Narrow <notifications@narrow.fyi>",
        "you@example.com",
        &web,
        &digest.items,
        now,
    );
    let out = json!({
        "now": now.to_rfc3339(),
        "size": size,
        "shown_in_cloud": shown,
        "verified": verified,
        "strong_before_verification": before,
        "strong": after,
        "today": today,
        "pool": digest.pool,
        "digest": items,
        "email": {"subject": email.subject, "text": email.text},
    });
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}
