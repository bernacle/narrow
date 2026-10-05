//! `narrow sources`: what each job board yields, and finding the boards
//! behind company careers pages.

use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Context;
use clap::Subcommand;
use jobhunt_app::sources::{CompanyDiscovery, SourceReport};
use jobhunt_sources::company::{CompanyTarget, ProbeSettings, parse_targets};
use jobhunt_sources::registry::{GeoBucket, SourceRegistry, SourceYield};

use crate::config::LoadedConfig;
use crate::local::{finish, with_app};

#[derive(Debug, clap::Args)]
pub struct SourcesArgs {
    #[command(subcommand)]
    pub command: SourcesCommand,
}

#[derive(Debug, Subcommand)]
pub enum SourcesCommand {
    /// Every source's yield (open, engineering, open to Brazil, freshness,
    /// and your own actionable/plausible/strong counts) and health, from
    /// stored jobs. Reads no network and records nothing.
    Report {
        /// A source registry file (e.g. deploy/sources.toml), for each
        /// source's status and provenance.
        #[arg(long, value_name = "FILE")]
        registry: Option<PathBuf>,
        /// JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Find the careers page and job board of companies from their domains,
    /// read each board with its adapter, and validate it. Adds nothing to
    /// your configuration.
    Discover {
        /// Company domains: `railway.com`, or `railway.com,Railway` with a
        /// display name.
        #[arg(value_name = "DOMAIN")]
        domains: Vec<String>,
        /// A file of companies, one per line (`domain` or `domain, Name`;
        /// `#` comments).
        #[arg(long, value_name = "FILE")]
        file: Option<PathBuf>,
        /// Don't try the company's name as a board slug when no page names
        /// a board.
        #[arg(long)]
        no_guess: bool,
        /// Write validated and rejected boards into this registry file
        /// (entries recording a person's decision are kept as they are).
        #[arg(long, value_name = "FILE")]
        registry: Option<PathBuf>,
        /// JSON instead of a summary.
        #[arg(long)]
        json: bool,
    },
}

pub async fn run(args: SourcesArgs, loaded: &LoadedConfig) -> anyhow::Result<ExitCode> {
    match args.command {
        SourcesCommand::Report { registry, json } => {
            let registry = registry.map(|p| read_registry(&p)).transpose()?;
            with_app!(loaded, |app| {
                let report = app
                    .source_report(registry.as_ref(), jobhunt_app::now())
                    .await?;
                let mut out = anstream::stdout().lock();
                let result = if json {
                    serde_json::to_writer_pretty(&mut out, &report)
                        .map_err(io::Error::from)
                        .and_then(|()| writeln!(out))
                } else {
                    write_report(&mut out, &report)
                };
                finish(result, "the report")
            })
        }
        SourcesCommand::Discover {
            domains,
            file,
            no_guess,
            registry,
            json,
        } => {
            let mut targets: Vec<CompanyTarget> = Vec::new();
            if let Some(file) = &file {
                let text = std::fs::read_to_string(file)
                    .with_context(|| format!("could not read {}", file.display()))?;
                targets.extend(
                    parse_targets(&text).map_err(|e| anyhow::anyhow!("{}: {e}", file.display()))?,
                );
            }
            for domain in &domains {
                let target: CompanyTarget = domain.parse()?;
                if !targets.iter().any(|t| t.domain == target.domain) {
                    targets.push(target);
                }
            }
            if targets.is_empty() {
                anyhow::bail!("name at least one company domain, or a file with --file");
            }
            let settings = ProbeSettings {
                guess_slugs: !no_guess,
                ..ProbeSettings::default()
            };
            with_app!(loaded, |app| {
                let now = jobhunt_app::now();
                let found = app.discover_companies(&targets, &settings, now).await?;
                if let Some(path) = &registry {
                    let mut current = if path.exists() {
                        read_registry(path)?
                    } else {
                        SourceRegistry::default()
                    };
                    let since = now.format("%Y-%m-%d").to_string();
                    let changed = found
                        .iter()
                        .flat_map(|d| d.registry_entries(&since))
                        .filter(|entry| current.upsert(entry.clone()))
                        .count();
                    std::fs::write(path, current.to_toml())
                        .with_context(|| format!("could not write {}", path.display()))?;
                    eprintln!(
                        "{changed} registry entries added or updated in {}",
                        path.display()
                    );
                }
                let mut out = anstream::stdout().lock();
                let result = if json {
                    serde_json::to_writer_pretty(&mut out, &found)
                        .map_err(io::Error::from)
                        .and_then(|()| writeln!(out))
                } else {
                    write_discovery(&mut out, &found)
                };
                finish(result, "the discovery")
            })
        }
    }
}

fn read_registry(path: &std::path::Path) -> anyhow::Result<SourceRegistry> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("could not read {}", path.display()))?;
    Ok(SourceRegistry::parse(&text)?)
}

/// `"12 eng (3 BR / 2 ?)"`-style counts.
fn yield_cells(y: &SourceYield) -> String {
    let f = &y.freshness;
    format!(
        "{:>5} {:>4} {:>4} {:>4} {:>4} {:>4} {:>13} {:>4} {:>4} {:>4} {:>4}",
        y.open,
        y.engineering,
        y.engineering_brazil_open,
        y.engineering_brazil_unclear,
        y.brazil_open,
        y.brazil_unclear,
        y.main_geo().map_or("-", GeoBucket::as_str),
        f.under_30_days,
        f.over_180_days,
        f.over_365_days,
        f.unknown,
    )
}

fn person_cells(y: &SourceYield) -> String {
    y.person.map_or_else(String::new, |p| {
        format!("{:>5} {:>5} {:>5}", p.actionable, p.plausible, p.strong)
    })
}

fn write_report(out: &mut impl Write, report: &SourceReport) -> io::Result<()> {
    writeln!(
        out,
        "{:<28} {:<10} {:<9} {:>5} {:>4} {:>4} {:>4} {:>4} {:>4} {:>13} {:>4} {:>4} {:>4} {:>4}{}",
        "source",
        "status",
        "health",
        "open",
        "eng",
        "eBR",
        "eBR?",
        "BR",
        "BR?",
        "main geo",
        "<30d",
        ">180",
        ">1y",
        "?d",
        if report.ranked {
            "  act plaus strong"
        } else {
            ""
        }
    )?;
    for row in &report.rows {
        let status = row
            .registry
            .as_ref()
            .map_or(if row.configured { "configured" } else { "-" }, |e| {
                e.status.as_str()
            });
        writeln!(
            out,
            "{:<28} {:<10} {:<9} {} {}",
            row.source.to_string(),
            status,
            row.health.as_str(),
            yield_cells(&row.yields),
            person_cells(&row.yields),
        )?;
        if let Some(next) = row.suggested_status {
            writeln!(out, "    → {} ({})", next.as_str(), row.health_reason)?;
        } else if let Some(error) = &row.last_error {
            writeln!(out, "    last error: {error}")?;
        }
    }
    writeln!(
        out,
        "{:<48} {} {}",
        "total",
        yield_cells(&report.totals),
        person_cells(&report.totals)
    )?;
    writeln!(out)?;
    writeln!(
        out,
        "eng: engineering · eBR/eBR?: engineering open to / unclear for a Brazil-based remote \
         candidate · BR/BR?: any posting · <30d, >180, >1y: by the source's publish date · ?d: \
         no publish date{}",
        if report.ranked {
            " · act/plaus/strong: your ranking (rules only)"
        } else {
            ""
        }
    )
}

fn write_discovery(out: &mut impl Write, found: &[CompanyDiscovery]) -> io::Result<()> {
    for d in found {
        let p = &d.probe;
        writeln!(
            out,
            "{} ({}): {}{}",
            p.company,
            p.domain,
            p.outcome.as_str(),
            p.careers_url
                .as_deref()
                .map_or_else(String::new, |u| format!(" · {u}"))
        )?;
        for other in &p.other_ats {
            writeln!(
                out,
                "    unsupported ATS: {} ({})",
                other.provider, other.url
            )?;
        }
        for (check, board) in p.checks.iter().zip(&d.assessments) {
            let y = &board.yields;
            writeln!(
                out,
                "    {} [{}] {:?}{}: {} jobs, {} engineering, {} open to Brazil ({} engineering), main geo {}",
                check.source,
                check.evidence.as_str(),
                board.validation.verdict,
                if board.configured { ", configured" } else { "" },
                check.jobs,
                y.engineering,
                y.brazil_open,
                y.engineering_brazil_open,
                y.main_geo().map_or("-", GeoBucket::as_str),
            )?;
            let why = board.validation.reasons.last().cloned().unwrap_or_default();
            writeln!(out, "        {why}")?;
            for unmet in &board.activation {
                writeln!(out, "        not ready: {unmet}")?;
            }
        }
    }
    let boards = found.iter().filter(|d| !d.probe.boards.is_empty()).count();
    writeln!(
        out,
        "\n{} companies · {} with a supported board · {} boards validated",
        found.len(),
        boards,
        found
            .iter()
            .flat_map(|d| &d.assessments)
            .filter(|b| b.validation.verdict == jobhunt_sources::registry::Verdict::Validated)
            .count()
    )
}
