//! `narrow discovery`: broad discovery of companies and boards Narrow
//! doesn't know yet (docs/broad-discovery.md).
//!
//! ```text
//! narrow discovery queries                 # the ATS search queries to run
//! narrow discovery import results.json     # search results, crawl lines, URL/domain lists
//! narrow discovery run hn --months 3       # a public provider
//! narrow discovery resolve                 # companies → boards; boards → owner; jobs → live/closed
//! narrow discovery report                  # yield per strategy, family, query, host, ATS
//! narrow discovery export --format config  # the boards worth activating, for a person to review
//! ```
//!
//! Candidates live in one file (`--store`, `discovery.json` by default),
//! saved after every step. Nothing here changes the configuration or the
//! registry unless `export --registry` is asked to.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Context;
use clap::{Subcommand, ValueEnum};
use jobhunt_app::broad_discovery::{OwnershipScope, ResolveOptions};
use jobhunt_sources::discovery::import::{self, ImportOptions};
use jobhunt_sources::discovery::queries::{Plan, QueryMatrix, SearchQuery};
use jobhunt_sources::discovery::report::{Report, Row};
use jobhunt_sources::discovery::resolve::ResolveSettings;
use jobhunt_sources::discovery::{
    CandidateKind, CandidateStatus, DiscoveryStore, Lead, Method, providers,
};
use jobhunt_sources::registry::{RegistryEntry, SourceRegistry, SourceStatus};

use crate::config::LoadedConfig;
use crate::local::{finish, with_app};

#[derive(Debug, clap::Args)]
pub struct DiscoveryArgs {
    /// The candidate store.
    #[arg(
        long,
        global = true,
        value_name = "FILE",
        default_value = "discovery.json"
    )]
    store: PathBuf,
    #[command(subcommand)]
    command: DiscoveryCommand,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum PlanArg {
    Pairwise,
    Full,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum OwnershipArg {
    All,
    Useful,
    Off,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Select {
    /// Validated and meeting the activation bar, with an engineering
    /// posting open to Brazil.
    Ready,
    /// Validated with an engineering posting open to (or unclear for) Brazil.
    Useful,
    /// Every validated board.
    Validated,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ExportFormat {
    /// `[[sources.*]]` entries for a config file.
    Config,
    /// Source registry entries.
    Registry,
}

#[derive(Debug, Subcommand)]
enum DiscoveryCommand {
    /// The ATS-index search queries to run: `site:<ATS host> "<role>"
    /// <place>`, from the query matrix (pairwise by default).
    Queries {
        /// A query matrix file instead of the built-in one.
        #[arg(long, value_name = "FILE")]
        matrix: Option<PathBuf>,
        #[arg(long, value_enum, default_value = "pairwise")]
        plan: PlanArg,
        /// Add specialty queries (a specialty with each place group).
        #[arg(long)]
        specialties: bool,
        /// Shift which place term and host each query uses (for rotating
        /// sweeps).
        #[arg(long, default_value_t = 0)]
        rotate: usize,
        /// At most this many queries.
        #[arg(long)]
        max: Option<usize>,
        #[arg(long)]
        json: bool,
    },
    /// Add leads from files: search results (JSON, JSON lines, CSV),
    /// crawl-index lines, or lists of URLs or domains.
    Import {
        #[arg(value_name = "FILE", required = true)]
        files: Vec<PathBuf>,
        /// The discovery method (inferred per result otherwise).
        #[arg(long)]
        method: Option<String>,
        /// The provider name (the file name otherwise).
        #[arg(long)]
        provider: Option<String>,
        /// The query matrix used to read query families back.
        #[arg(long, value_name = "FILE")]
        matrix: Option<PathBuf>,
    },
    /// Collect leads from a public provider.
    Run {
        #[command(subcommand)]
        provider: RunProvider,
    },
    /// Resolve pending candidates: companies to their boards, boards to
    /// their owner (the company's site must point back), discovered jobs to
    /// live or closed. Saves after every chunk.
    Resolve {
        /// A source registry, to note boards Narrow already knows.
        #[arg(long, value_name = "FILE")]
        registry: Option<PathBuf>,
        /// Resolve at most this many candidates in total.
        #[arg(long)]
        limit: Option<usize>,
        /// Candidates per chunk (saved after each).
        #[arg(long, default_value_t = 100)]
        chunk: usize,
        /// Which read boards get their ownership checked.
        #[arg(long, value_enum, default_value = "all")]
        ownership: OwnershipArg,
        #[arg(long)]
        no_companies: bool,
        #[arg(long)]
        no_boards: bool,
        /// Don't look at sitemaps and JSON-LD for companies without a board.
        #[arg(long)]
        no_surface: bool,
        /// Resolve inconclusive candidates again.
        #[arg(long)]
        recheck: bool,
        /// Only candidates found by this strategy (prefix: `ats_search`,
        /// `web_index`, `hiring_thread:hn`, …).
        #[arg(long)]
        strategy: Option<String>,
        /// Take pending candidates in a seeded random order (a sample).
        #[arg(long)]
        shuffle_seed: Option<u64>,
        /// Concurrent board reads (ownership checks run twice as many).
        #[arg(long)]
        concurrency: Option<usize>,
    },
    /// Yield per strategy, query family, query, ATS host and place;
    /// supported and unsupported ATS; boards to activate or recheck. With
    /// a profile and the boards' jobs stored, the person's ranking counts.
    Report {
        #[arg(long, value_name = "FILE")]
        registry: Option<PathBuf>,
        #[arg(long)]
        json: bool,
        /// Rows per table.
        #[arg(long, default_value_t = 15)]
        top: usize,
    },
    /// Write selected boards as config or registry entries, for a person to
    /// review. Nothing is activated.
    Export {
        #[arg(long, value_enum, default_value = "ready")]
        select: Select,
        #[arg(long, value_enum, default_value = "config")]
        format: ExportFormat,
        /// With `--format registry`: add the entries to this registry file
        /// (entries recording a person's decision are kept as they are).
        #[arg(long, value_name = "FILE")]
        registry: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
enum RunProvider {
    /// Run the generated queries through a search API (Brave Search;
    /// the key in BRAVE_SEARCH_API_KEY).
    Search {
        #[arg(long, value_name = "FILE")]
        matrix: Option<PathBuf>,
        #[arg(long, value_enum, default_value = "pairwise")]
        plan: PlanArg,
        #[arg(long)]
        specialties: bool,
        #[arg(long, default_value_t = 0)]
        rotate: usize,
        #[arg(long)]
        max: Option<usize>,
    },
    /// Hacker News "Who is hiring?" threads.
    Hn {
        /// The latest N monthly threads.
        #[arg(long, default_value_t = 1)]
        months: usize,
    },
    /// Hiring YC companies (the yc-oss open dataset).
    Yc,
    /// The remoteintech directory of remote-friendly companies.
    Remoteintech,
    /// Common Crawl's URL index of ATS hosts.
    WebIndex {
        /// The crawl (`CC-MAIN-2026-39`).
        #[arg(long)]
        crawl: String,
        #[arg(long = "host", value_name = "HOST", required = true)]
        hosts: Vec<String>,
        /// At most this many index pages per host.
        #[arg(long)]
        pages: Option<usize>,
    },
}

fn read_store(path: &Path) -> anyhow::Result<DiscoveryStore> {
    if !path.exists() {
        return Ok(DiscoveryStore::new());
    }
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("could not read {}", path.display()))?;
    Ok(DiscoveryStore::parse(&text)?)
}

fn write_store(path: &Path, store: &DiscoveryStore) -> anyhow::Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, store.to_json())
        .with_context(|| format!("could not write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("could not write {}", path.display()))
}

fn read_matrix(path: Option<&Path>) -> anyhow::Result<QueryMatrix> {
    match path {
        Some(p) => {
            let text = std::fs::read_to_string(p)
                .with_context(|| format!("could not read {}", p.display()))?;
            Ok(QueryMatrix::parse(&text)?)
        }
        None => Ok(QueryMatrix::builtin()),
    }
}

fn read_registry(path: &Path) -> anyhow::Result<SourceRegistry> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("could not read {}", path.display()))?;
    Ok(SourceRegistry::parse(&text)?)
}

fn plan_queries(
    matrix: &QueryMatrix,
    plan: PlanArg,
    specialties: bool,
    rotate: usize,
    max: Option<usize>,
) -> Vec<SearchQuery> {
    let plan = match plan {
        PlanArg::Pairwise => Plan::Pairwise,
        PlanArg::Full => Plan::Full,
    };
    let mut queries = matrix.plan(plan, rotate);
    if specialties {
        queries.extend(matrix.specialty_queries(rotate));
    }
    if let Some(max) = max {
        queries.truncate(max);
    }
    queries
}

/// Adds leads; returns (leads, new candidates, new sightings).
fn add_leads(store: &mut DiscoveryStore, leads: Vec<Lead>) -> (usize, usize, usize) {
    let n = leads.len();
    let (mut new, mut seen) = (0, 0);
    for lead in leads {
        if let Some(a) = store.add(lead) {
            new += usize::from(a.new);
            seen += usize::from(a.new_sighting);
        }
    }
    (n, new, seen)
}

pub async fn run(args: DiscoveryArgs, loaded: &LoadedConfig) -> anyhow::Result<ExitCode> {
    let now = jobhunt_app::now();
    match args.command {
        DiscoveryCommand::Queries {
            matrix,
            plan,
            specialties,
            rotate,
            max,
            json,
        } => {
            let matrix = read_matrix(matrix.as_deref())?;
            let queries = plan_queries(&matrix, plan, specialties, rotate, max);
            let mut out = anstream::stdout().lock();
            let result = if json {
                serde_json::to_writer_pretty(&mut out, &queries)
                    .map_err(io::Error::from)
                    .and_then(|()| writeln!(out))
            } else {
                queries
                    .iter()
                    .try_for_each(|q| writeln!(out, "{}", q.text()))
            };
            eprintln!("{} queries", queries.len());
            finish(result, "the queries")
        }
        DiscoveryCommand::Import {
            files,
            method,
            provider,
            matrix,
        } => {
            let method: Option<Method> = method
                .map(|m| m.parse())
                .transpose()
                .map_err(|e: String| anyhow::anyhow!(e))?;
            let matrix = read_matrix(matrix.as_deref())?;
            let mut store = read_store(&args.store)?;
            for file in &files {
                let text = std::fs::read_to_string(file)
                    .with_context(|| format!("could not read {}", file.display()))?;
                let opts = ImportOptions {
                    method,
                    provider: provider.clone().unwrap_or_else(|| {
                        file.file_name()
                            .map_or_else(|| "import".into(), |n| n.to_string_lossy().into_owned())
                    }),
                    at: now,
                    matrix: Some(&matrix),
                };
                let leads = import::parse(&text, &opts)
                    .map_err(|e| anyhow::anyhow!("{}: {e}", file.display()))?;
                let (n, new, seen) = add_leads(&mut store, leads);
                eprintln!(
                    "{}: {n} leads, {new} new candidates, {seen} new sightings",
                    file.display()
                );
            }
            write_store(&args.store, &store)?;
            eprintln!("{} candidates in {}", store.len(), args.store.display());
            Ok(ExitCode::SUCCESS)
        }
        DiscoveryCommand::Run { provider } => {
            let mut store = read_store(&args.store)?;
            with_app!(loaded, |app| {
                let http = app.discovery_http()?;
                match provider {
                    RunProvider::Search {
                        matrix,
                        plan,
                        specialties,
                        rotate,
                        max,
                    } => {
                        let key = std::env::var("BRAVE_SEARCH_API_KEY")
                            .context("set BRAVE_SEARCH_API_KEY, or export results from any search provider and use `narrow discovery import`")?;
                        let matrix = read_matrix(matrix.as_deref())?;
                        for q in plan_queries(&matrix, plan, specialties, rotate, max) {
                            match providers::brave_search(&http, None, &key, &q, 20, now).await {
                                Ok(leads) => {
                                    let (n, new, _) = add_leads(&mut store, leads);
                                    eprintln!("{n:>3} results, {new:>3} new · {}", q.text());
                                }
                                Err(e) => eprintln!("failed: {} ({e})", q.text()),
                            }
                            // One query a second: the API's free-tier rate.
                            tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
                        }
                    }
                    RunProvider::Hn { months } => {
                        for thread in providers::hn_threads(&http, None, months).await? {
                            let leads = providers::hn_thread(&http, None, &thread.id, now).await?;
                            let (n, new, _) = add_leads(&mut store, leads);
                            eprintln!("{}: {n} leads, {new} new candidates", thread.title);
                        }
                    }
                    RunProvider::Yc => {
                        let leads = providers::yc_companies(&http, None, now).await?;
                        let (n, new, _) = add_leads(&mut store, leads);
                        eprintln!("yc-oss: {n} hiring companies, {new} new candidates");
                    }
                    RunProvider::Remoteintech => {
                        let leads = providers::remoteintech(&http, 4, now).await?;
                        let (n, new, _) = add_leads(&mut store, leads);
                        eprintln!("remoteintech: {n} companies, {new} new candidates");
                    }
                    RunProvider::WebIndex {
                        crawl,
                        hosts,
                        pages,
                    } => {
                        for host in hosts {
                            let total =
                                providers::common_crawl_pages(&http, None, &crawl, &host).await?;
                            for page in 0..pages.map_or(total, |p| p.min(total)) {
                                match providers::common_crawl(&http, None, &crawl, &host, page, now)
                                    .await
                                {
                                    Ok(leads) => {
                                        let (n, new, _) = add_leads(&mut store, leads);
                                        eprintln!(
                                            "{host} page {page}/{total}: {n} URLs, {new} new candidates"
                                        );
                                    }
                                    Err(e) => eprintln!(
                                        "{host} page {page}: {e} (skipped; run again later)"
                                    ),
                                }
                                write_store(&args.store, &store)?;
                            }
                        }
                    }
                }
                write_store(&args.store, &store)?;
                eprintln!("{} candidates in {}", store.len(), args.store.display());
                Ok(ExitCode::SUCCESS)
            })
        }
        DiscoveryCommand::Resolve {
            registry,
            limit,
            chunk,
            ownership,
            no_companies,
            no_boards,
            no_surface,
            recheck,
            strategy,
            shuffle_seed,
            concurrency,
        } => {
            let registry = registry.map(|p| read_registry(&p)).transpose()?;
            let mut store = read_store(&args.store)?;
            with_app!(loaded, |app| {
                let mut opts = ResolveOptions {
                    limit: chunk.max(1),
                    ownership: match ownership {
                        OwnershipArg::All => OwnershipScope::All,
                        OwnershipArg::Useful => OwnershipScope::Useful,
                        OwnershipArg::Off => OwnershipScope::Off,
                    },
                    companies: !no_companies,
                    boards: !no_boards,
                    surface: !no_surface,
                    recheck,
                    strategy,
                    shuffle_seed,
                    concurrency: concurrency
                        .unwrap_or(app.config().discovery.concurrency)
                        .max(1),
                };
                let settings = ResolveSettings::new();
                let mut done = 0usize;
                // A recheck takes each inconclusive candidate once.
                let mut rechecked = std::collections::HashSet::new();
                loop {
                    if let Some(limit) = limit {
                        if done >= limit {
                            break;
                        }
                        opts.limit = chunk.min(limit - done).max(1);
                    }
                    let before: Vec<String> = store
                        .candidates
                        .iter()
                        .filter(|c| c.status == CandidateStatus::Inconclusive)
                        .map(|c| c.key.clone())
                        .collect();
                    let summary = app
                        .resolve_discovery(&mut store, registry.as_ref(), &opts, &settings, now)
                        .await?;
                    if summary.is_empty() {
                        break;
                    }
                    done += summary.companies + summary.boards_read;
                    write_store(&args.store, &store)?;
                    eprintln!(
                        "{} companies, {} boards read ({} ownership checks, {} jobs settled, {} boards added) · {done} so far",
                        summary.companies,
                        summary.boards_read,
                        summary.ownership_checked,
                        summary.jobs_settled,
                        summary.boards_added
                    );
                    if recheck {
                        rechecked.extend(before);
                        // Inconclusive candidates already rechecked are
                        // left for another run.
                        if store
                            .candidates
                            .iter()
                            .filter(|c| c.status == CandidateStatus::Inconclusive)
                            .all(|c| rechecked.contains(&c.key))
                        {
                            opts.recheck = false;
                        }
                    }
                }
                write_store(&args.store, &store)?;
                Ok(ExitCode::SUCCESS)
            })
        }
        DiscoveryCommand::Report {
            registry,
            json,
            top,
        } => {
            let registry = registry.map(|p| read_registry(&p)).transpose()?;
            let store = read_store(&args.store)?;
            with_app!(loaded, |app| {
                let report = app.discovery_report(&store, registry.as_ref(), now).await?;
                let mut out = anstream::stdout().lock();
                let result = if json {
                    serde_json::to_writer_pretty(&mut out, &report)
                        .map_err(io::Error::from)
                        .and_then(|()| writeln!(out))
                } else {
                    write_report(&mut out, &report, top)
                };
                finish(result, "the report")
            })
        }
        DiscoveryCommand::Export {
            select,
            format,
            registry,
        } => {
            let store = read_store(&args.store)?;
            let report =
                jobhunt_sources::discovery::report::build(&store, None, &Default::default());
            let chosen: Vec<_> = store
                .of_kind(CandidateKind::Board)
                .filter(|c| c.status == CandidateStatus::Validated)
                .filter(|c| {
                    let y = c.board_result.as_ref().and_then(|b| b.yields.as_ref());
                    let useful = y.is_some_and(|y| {
                        y.engineering_brazil_open + y.engineering_brazil_unclear > 0
                    });
                    match select {
                        Select::Validated => true,
                        Select::Useful => useful,
                        Select::Ready => report
                            .activate
                            .iter()
                            .any(|l| Some(&l.source) == c.source.as_ref()),
                    }
                })
                .collect();
            let date = now.format("%Y-%m-%d").to_string();
            match format {
                ExportFormat::Config => {
                    let mut out = anstream::stdout().lock();
                    let mut text = String::new();
                    for c in &chosen {
                        let (Some(board), Some(result)) = (&c.board, &c.board_result) else {
                            continue;
                        };
                        let company = result.company.clone().or_else(|| c.company_hint.clone());
                        text.push_str(&format!("[[sources.{}]]\n", board.provider));
                        let field = if board.provider == "lever" {
                            "site"
                        } else if board.provider == "yc" {
                            "slug"
                        } else {
                            "board"
                        };
                        text.push_str(&format!("{field} = {}\n", toml_str(&board.board)));
                        if let Some(company) = company {
                            text.push_str(&format!("company = {}\n", toml_str(&company)));
                        }
                        if board.eu {
                            text.push_str("region = \"eu\"\n");
                        }
                        text.push('\n');
                    }
                    eprintln!("{} boards", chosen.len());
                    finish(write!(out, "{text}"), "the config")
                }
                ExportFormat::Registry => {
                    let entries: Vec<RegistryEntry> = chosen
                        .iter()
                        .filter_map(|c| {
                            let result = c.board_result.as_ref()?;
                            let strategies: std::collections::BTreeSet<String> = c.sightings.iter().map(|s| s.strategy()).collect();
                            let mut reasons: Vec<String> = result.validation.reasons.clone();
                            if let Some(y) = &result.yields {
                                reasons.push(format!(
                                    "{date}: {} open, {} engineering, {} engineering open to Brazil and {} unclear",
                                    y.open, y.engineering, y.engineering_brazil_open, y.engineering_brazil_unclear
                                ));
                            }
                            reasons.extend(result.activation.iter().map(|a| format!("not yet: {a}")));
                            Some(RegistryEntry {
                                source: c.source.as_deref()?.parse().ok()?,
                                company: result.company.clone().or_else(|| c.company_hint.clone()).unwrap_or_else(|| c.board.as_ref().map(|b| b.board.clone()).unwrap_or_default()),
                                domain: result.domain.clone(),
                                careers_url: result.careers_url.clone(),
                                status: SourceStatus::Validated,
                                since: date.clone(),
                                provenance: format!(
                                    "broad discovery (BRU-360): {}; ownership {}",
                                    strategies.into_iter().collect::<Vec<_>>().join(", "),
                                    result.ownership.as_str()
                                ),
                                reasons,
                                notes: None,
                            })
                        })
                        .collect();
                    match registry {
                        Some(path) => {
                            let mut current = if path.exists() {
                                read_registry(&path)?
                            } else {
                                SourceRegistry::default()
                            };
                            let changed = entries
                                .into_iter()
                                .filter(|e| current.upsert(e.clone()))
                                .count();
                            std::fs::write(&path, current.to_toml())
                                .with_context(|| format!("could not write {}", path.display()))?;
                            eprintln!(
                                "{changed} registry entries added or updated in {}",
                                path.display()
                            );
                            Ok(ExitCode::SUCCESS)
                        }
                        None => {
                            let r = SourceRegistry { sources: entries };
                            let mut out = anstream::stdout().lock();
                            finish(write!(out, "{}", r.to_toml()), "the registry entries")
                        }
                    }
                }
            }
        }
    }
}

fn toml_str(s: &str) -> String {
    toml::Value::String(s.to_owned()).to_string()
}

fn pct(r: Option<f64>) -> String {
    r.map_or_else(|| "-".into(), |r| format!("{:.0}%", r * 100.0))
}

fn row_line(r: &Row, width: usize) -> String {
    let p = r.person.map_or_else(String::new, |p| {
        format!(" {:>5} {:>5} {:>4}", p.actionable, p.plausible, p.strong)
    });
    let key: String = r.key.chars().take(width).collect();
    format!(
        "{key:<width$} {:>6} {:>6} {:>5} {:>5} {:>5} {:>4} {:>5} {:>4} {:>4} {:>4} {:>4} {:>5} {:>4} {:>5} {:>5}{p}",
        r.raw_hits,
        r.jobs,
        r.live_jobs,
        r.closed_jobs,
        r.useful_jobs,
        r.boards,
        r.validated_boards,
        r.useful_boards,
        r.ready_boards,
        r.companies,
        r.new_companies,
        pct(r.duplicate_rate),
        pct(r.stale_rate),
        pct(r.first_party_rate),
        r.unique_boards,
    )
}

fn header(width: usize, person: bool) -> String {
    format!(
        "{:<width$} {:>6} {:>6} {:>5} {:>5} {:>5} {:>4} {:>5} {:>4} {:>4} {:>4} {:>4} {:>5} {:>4} {:>5} {:>5}{}",
        "",
        "hits",
        "jobs",
        "live",
        "dead",
        "useJ",
        "brds",
        "valid",
        "useB",
        "rdy",
        "cos",
        "new",
        "dup",
        "stale",
        "1stP",
        "only",
        if person { "   act plaus strg" } else { "" }
    )
}

fn table(
    out: &mut impl Write,
    title: &str,
    rows: &[Row],
    top: usize,
    width: usize,
) -> io::Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    writeln!(out, "\n{title}")?;
    writeln!(
        out,
        "{}",
        header(width, rows.iter().any(|r| r.person.is_some()))
    )?;
    for r in rows.iter().take(top) {
        writeln!(out, "{}", row_line(r, width))?;
    }
    if rows.len() > top {
        writeln!(out, "… {} more", rows.len() - top)?;
    }
    Ok(())
}

fn write_report(out: &mut impl Write, r: &Report, top: usize) -> io::Result<()> {
    writeln!(out, "Candidates by kind and status:")?;
    for (kind, statuses) in &r.candidates {
        let list: Vec<String> = statuses.iter().map(|(s, n)| format!("{s} {n}")).collect();
        writeln!(out, "  {kind:<16} {}", list.join(" · "))?;
    }
    let own: Vec<String> = r
        .ownership
        .iter()
        .map(|(o, n)| format!("{o} {n}"))
        .collect();
    writeln!(out, "  board ownership  {}", own.join(" · "))?;
    table(out, "Total", std::slice::from_ref(&r.total), 1, 28)?;
    table(out, "By strategy", &r.strategies, top, 28)?;
    let mut families = r.families.clone();
    families.sort_by_key(|r| std::cmp::Reverse((r.useful_boards, r.validated_boards)));
    table(
        out,
        "Query families, most useful boards first",
        &families,
        top,
        40,
    )?;
    let mut queries = r.queries.clone();
    queries.sort_by_key(|r| std::cmp::Reverse((r.useful_boards, r.useful_jobs)));
    table(out, "Queries, most useful boards first", &queries, top, 64)?;
    table(out, "ATS host searched", &r.hosts, top, 28)?;
    table(out, "Place searched", &r.geo, top, 28)?;
    writeln!(out, "\nSupported ATS")?;
    writeln!(
        out,
        "{:<12} {:>6} {:>6} {:>6} {:>6} {:>6} {:>8}",
        "", "boards", "read", "valid", "useful", "ready", "postings"
    )?;
    for p in &r.providers {
        writeln!(
            out,
            "{:<12} {:>6} {:>6} {:>6} {:>6} {:>6} {:>8}",
            p.provider, p.boards, p.boards_read, p.validated, p.useful, p.ready, p.postings
        )?;
    }
    writeln!(
        out,
        "\nUnsupported ATS (companies · URLs · Brazil/LATAM/global context · also on a supported board)"
    )?;
    for u in r.unsupported.iter().take(top) {
        writeln!(
            out,
            "  {:<16} {:>5} {:>6} {:>5} {:>5}  {}",
            u.provider,
            u.companies,
            u.urls,
            u.brazil_latam_global,
            u.also_supported,
            u.examples.join(", ")
        )?;
    }
    writeln!(
        out,
        "\nRecommended to activate ({}), hold {}, recheck {}, rejected {}",
        r.activate.len(),
        r.hold,
        r.recheck.len(),
        r.rejected
    )?;
    for l in r.activate.iter().take(top) {
        let p = l.person.map_or_else(String::new, |p| {
            format!(
                " · you: {} act, {} plaus, {} strong",
                p.actionable, p.plausible, p.strong
            )
        });
        writeln!(
            out,
            "  {:<36} {:<24} {:>4} open {:>3} eng {:>3} eBR {:>3} eBR?{}{} [{}]",
            l.source,
            l.domain.as_deref().unwrap_or("-"),
            l.open,
            l.engineering,
            l.engineering_brazil_open,
            l.engineering_brazil_unclear,
            p,
            if l.new_to_narrow { " · new" } else { "" },
            l.strategies.join(", ")
        )?;
    }
    if !r.recheck.is_empty() {
        writeln!(out, "\nRecheck (useful, ownership not established):")?;
        for l in r.recheck.iter().take(top) {
            writeln!(
                out,
                "  {:<36} {:<10} {:>3} eBR {:>3} eBR? · {}",
                l.source,
                l.ownership.as_str(),
                l.engineering_brazil_open,
                l.engineering_brazil_unclear,
                l.why.first().map_or("", String::as_str)
            )?;
        }
    }
    writeln!(
        out,
        "\nhits: raw hits · useJ: live engineering jobs open to Brazil or with a global/Brazil/Americas scope · brds: boards · valid: validated · \
         useB: validated with engineering open to (or unclear for) Brazil · rdy: meets the activation bar · cos/new: companies, new to Narrow · \
         dup: duplicate rate · stale: closed share of checked jobs · 1stP: ownership established, of boards read · only: boards no other strategy found"
    )
}
