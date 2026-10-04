//! Role-breadth probe (see `docs/role-breadth-fit-experiment.md`). Not
//! part of the product: it samples a fresh snapshot's software-engineering
//! IC postings and dumps exactly the job-only text they were annotated
//! from.
//!
//! ```text
//! # a seeded, stratified sample of software-engineering IC postings,
//! # leaving out the ids in exclude.txt
//! breadth_probe sample <config.toml> <corpus.db> <seed> <ids.txt> <selection.json> <exclude.txt>
//! # the job-only text of each sampled posting (no network)
//! breadth_probe dump   <config.toml> <corpus.db> <ids.txt> <out dir>
//! # the deterministic breadth reading of each posting in ids.txt
//! breadth_probe run    <config.toml> <corpus.db> <ids.txt> <readings.json>
//! # readings scored against the annotations (Phase 1 gate)
//! breadth_probe score  <annotations.json> <readings.json>
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stdout)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use jobhunt_app::LocalApp;
use jobhunt_core::StableId;
use jobhunt_eval::job_function::{self, JobInput, PostingFields};
use jobhunt_jobs::{JobQuery, JobRecord, JobStatus};
use jobhunt_ranking::facets::work::split_title;
use jobhunt_ranking::facets::{JobFunction, facets, title_roles};
use serde_json::json;

/// Title role families, first match wins (the title's own role words,
/// before any qualifier).
const FAMILIES: &[(&str, &[&str])] = &[
    ("sre", &["sre"]),
    ("security", &["security"]),
    ("data_ml", &["data", "machine learning"]),
    ("platform", &["platform"]),
    ("infrastructure", &["infrastructure"]),
    ("backend", &["backend"]),
    ("full_stack", &["full stack"]),
    ("mobile", &["mobile", "embedded"]),
    ("frontend", &["frontend"]),
];

/// Families the title vocabulary has no role for, by title words.
const TITLE_WORD_FAMILIES: &[(&str, &[&str])] = &[
    (
        "developer_tooling",
        &[
            "tooling",
            "developer experience",
            "developer productivity",
            "build system",
        ],
    ),
    ("product", &["product engineer", "product software"]),
];

/// How many to draw per family, with and without a title qualifier.
const QUOTA_QUALIFIED: usize = 5;
const QUOTA_PLAIN: usize = 3;
/// Postings whose title names no family.
const QUOTA_OTHER: usize = 8;
const COMPANY_CAP: usize = 4;

fn family(title: &str) -> &'static str {
    let (primary, area) = split_title(title);
    let roles: Vec<&str> = title_roles(primary)
        .into_iter()
        .chain(title_roles(area))
        .map(|(r, _)| r)
        .collect();
    let lower = title.to_lowercase();
    FAMILIES
        .iter()
        .find(|(_, values)| roles.iter().any(|r| values.contains(r)))
        .or_else(|| {
            TITLE_WORD_FAMILIES
                .iter()
                .find(|(_, words)| words.iter().any(|w| lower.contains(w)))
        })
        .map_or("other_engineering", |(name, _)| name)
}

/// A software-engineering IC posting, as Narrow's deterministic facets
/// already read it.
fn engineering_ic(record: &JobRecord) -> bool {
    let f = facets(record);
    f.function == JobFunction::Engineering && !f.manages_people()
}

async fn open(config: &str, db: &str) -> LocalApp {
    let loaded =
        jobhunt_app::config::load(Some(Path::new(config)), Some(Path::new(db)), None).unwrap();
    LocalApp::open(loaded).await.unwrap()
}

async fn open_records(app: &LocalApp) -> Vec<JobRecord> {
    app.store()
        .search(&JobQuery {
            status: Some(JobStatus::Open),
            ..JobQuery::default()
        })
        .await
        .unwrap()
}

fn lines(path: &str) -> BTreeSet<String> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_owned)
        .collect()
}

async fn sample(args: &[String]) {
    let (config, db, seed, ids_out, selection_out, exclude) =
        (&args[2], &args[3], &args[4], &args[5], &args[6], &args[7]);
    let excluded = lines(exclude);
    let app = open(config, db).await;
    let records = open_records(&app).await;
    let open_count = records.len();
    // stratum -> (hash, id, company, title)
    let mut pools: BTreeMap<String, Vec<(String, String, String, String)>> = BTreeMap::new();
    let mut population = 0;
    for r in &records {
        let id = r.id.to_string();
        if excluded.contains(&id) || !engineering_ic(r) {
            continue;
        }
        population += 1;
        let title = r.posting.title.trim().to_owned();
        let fam = family(&title);
        let qualified = !split_title(&title).1.is_empty();
        let stratum = if fam == "other_engineering" {
            fam.to_owned()
        } else {
            format!("{fam}/{}", if qualified { "qualified" } else { "plain" })
        };
        let key = StableId::derive("narrow.eval.breadth.sample", &[seed, &id]).to_string();
        pools
            .entry(stratum)
            .or_default()
            .push((key, id, r.posting.company.clone(), title));
    }
    let mut per_company: BTreeMap<String, usize> = BTreeMap::new();
    let mut titles: BTreeSet<(String, String)> = BTreeSet::new();
    let mut picked = Vec::new();
    let mut summary = Vec::new();
    for (stratum, mut pool) in pools {
        pool.sort();
        let quota = if stratum == "other_engineering" {
            QUOTA_OTHER
        } else if stratum.ends_with("/qualified") {
            QUOTA_QUALIFIED
        } else {
            QUOTA_PLAIN
        };
        let mut drawn = 0;
        for (_, id, company, title) in &pool {
            if drawn >= quota {
                break;
            }
            let title_key = (company.clone(), title.to_lowercase());
            if per_company.get(company).copied().unwrap_or(0) >= COMPANY_CAP
                || titles.contains(&title_key)
            {
                continue;
            }
            titles.insert(title_key);
            *per_company.entry(company.clone()).or_default() += 1;
            drawn += 1;
            picked.push(json!({"job": id, "company": company, "title": title, "stratum": stratum}));
        }
        summary
            .push(json!({"stratum": stratum, "pool": pool.len(), "quota": quota, "drawn": drawn}));
    }
    let ids: Vec<&str> = picked.iter().map(|p| p["job"].as_str().unwrap()).collect();
    std::fs::write(ids_out, ids.join("\n") + "\n").unwrap();
    let out = json!({
        "seed": seed,
        "open_postings": open_count,
        "engineering_ic_population": population,
        "excluded": excluded.len(),
        "company_cap": COMPANY_CAP,
        "strata": summary,
        "sampled": picked.len(),
        "jobs": picked,
    });
    std::fs::write(selection_out, serde_json::to_string_pretty(&out).unwrap()).unwrap();
    println!(
        "sampled {} of {population} engineering IC postings ({open_count} open)",
        picked.len()
    );
    app.close().await;
}

fn input(record: &JobRecord) -> JobInput {
    let p = &record.posting;
    JobInput::build(&PostingFields {
        title: &p.title,
        department: p.department.as_deref(),
        team: p.team.as_deref(),
        location: None,
        workplace: None,
        description: p.description_text.as_deref(),
    })
}

async fn dump(args: &[String]) {
    let (config, db, ids, dir) = (&args[2], &args[3], &args[4], &args[5]);
    let wanted = lines(ids);
    let app = open(config, db).await;
    std::fs::create_dir_all(dir).unwrap();
    for r in open_records(&app).await {
        let id = r.id.to_string();
        if !wanted.contains(&id) {
            continue;
        }
        let i = input(&r);
        let text = format!(
            "# {} · {id} · omitted {}\n{}",
            r.posting.company,
            i.omitted,
            job_function::render(&i)
        );
        std::fs::write(format!("{dir}/{id}.txt"), text).unwrap();
    }
    app.close().await;
}

async fn run(args: &[String]) {
    let (config, db, ids, out) = (&args[2], &args[3], &args[4], &args[5]);
    let wanted = lines(ids);
    let app = open(config, db).await;
    let mut rows = Vec::new();
    for r in open_records(&app).await {
        let id = r.id.to_string();
        if !wanted.contains(&id) {
            continue;
        }
        let p = &r.posting;
        let reading = jobhunt_eval::breadth::read(
            &p.title,
            p.description_text.as_deref().unwrap_or_default(),
            &p.company,
        );
        rows.push(json!({"job": id, "company": p.company, "title": p.title, "reading": reading}));
    }
    std::fs::write(
        out,
        serde_json::to_string_pretty(&json!({"jobs": rows})).unwrap(),
    )
    .unwrap();
    app.close().await;
}

fn score(args: &[String]) {
    use jobhunt_eval::breadth::{Annotation, Breadth, BreadthReading, evaluate};
    let annotations: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&args[2]).unwrap()).unwrap();
    let readings: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&args[3]).unwrap()).unwrap();
    let by_job: BTreeMap<String, BreadthReading> = readings["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|j| {
            (
                j["job"].as_str().unwrap().to_owned(),
                serde_json::from_value(j["reading"].clone()).unwrap(),
            )
        })
        .collect();
    let mut rows = Vec::new();
    let mut table = Vec::new();
    for j in annotations["jobs"].as_array().unwrap() {
        let a: Annotation = serde_json::from_value(j["annotation"].clone()).unwrap();
        let id = j["job"].as_str().unwrap();
        let got = by_job.get(id).map_or(Breadth::Unclear, |r| r.breadth);
        if a.breadth != "out_of_scope" {
            table.push(json!({
                "job": id, "company": j["company"], "title": j["title"],
                "annotation": a.breadth, "also": a.also, "reading": by_job.get(id),
            }));
        }
        rows.push((a, got));
    }
    let m = evaluate(&rows);
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({"metrics": m, "jobs": table})).unwrap()
    );
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("sample") => sample(&args).await,
        Some("dump") => dump(&args).await,
        Some("run") => run(&args).await,
        Some("score") => score(&args),
        _ => eprintln!("usage: breadth_probe sample|dump|run|score …"),
    }
}
