//! The `narrow` command.
//!
//! Every command is a thin interface over [`jobhunt_app::LocalApp`], the
//! same application the MCP server (`narrow mcp`) runs: arguments in, a
//! use case, output on stdout (progress and logs on stderr).

mod check;
mod claims;
mod cloud;
mod config;
mod context;
mod credentials;
mod doctor;
mod eligibility;
mod find;
mod init;
mod local;
mod logging;
mod preferences;
mod profile;
mod profile_args;
mod profile_render;
mod rank;
mod rank_render;
mod render;
mod serve;
mod show;
mod sources;
mod state;
mod verify;

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Context;
use clap::{Parser, Subcommand};
use jobhunt_ranking::FeedbackAction;

use crate::config::{LoadedConfig, LogFormatArg, Paths};

/// High-signal job discovery: a tiny, verified, personalized shortlist
/// from a large universe of jobs.
///
/// Start with `narrow init resume.pdf`, say what you want with
/// `narrow preferences add "…"`, then run `narrow find`.
#[derive(Debug, Parser)]
#[command(name = "narrow", version, about, propagate_version = true)]
struct Cli {
    /// Config file to use instead of the default location.
    #[arg(long, global = true, env = "JOBHUNT_CONFIG", value_name = "PATH")]
    config: Option<PathBuf>,

    /// SQLite database file to use instead of the configured one.
    #[arg(long, global = true, env = "JOBHUNT_DATABASE", value_name = "PATH")]
    database: Option<PathBuf>,

    /// Show more logs on stderr (-v: progress, -vv: debug, -vvv: trace).
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    verbose: u8,

    /// Log format on stderr.
    #[arg(long, global = true, value_enum, value_name = "FORMAT")]
    log_format: Option<LogFormatArg>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Import your resume (PDF, .txt or .md) into your career profile. Run it
    /// again after updating the resume; your edits and decisions are kept.
    Init(init::InitArgs),
    /// What you want next: roles, pay, location, companies, domains, work style.
    #[command(alias = "prefs", alias = "preference")]
    Preferences(preferences::PreferencesArgs),
    /// The few opportunities worth your time: personalized, verified, with
    /// why. Refreshes job boards when stored jobs are stale (--refresh,
    /// --offline); --raw lists every matching stored job instead.
    Find(find::FindArgs),
    /// Everything about one opportunity: every source listing it, its
    /// verification, eligibility, fit, your status and history.
    Show(show::ShowArgs),
    /// The decision brief of one opportunity: why it may be worth your
    /// time, caveats, unknowns, your history with it, and (--details) every
    /// signal with its evidence.
    Why(rank::WhyArgs),
    /// Verify one opportunity at its authoritative sources (still open? can
    /// you apply? who publishes it? what does it say now?), save the
    /// result, and check it against your profile.
    Verify(verify::VerifyArgs),
    /// The full verification and eligibility report of one opportunity,
    /// with the evidence behind every reason, from what is stored
    /// (`--refresh` verifies first).
    Check(check::CheckArgs),
    /// Save an opportunity for later.
    Save(rank::FeedbackArgs),
    /// Not interested. Say why with --reason ("too corporate", "pure SRE"):
    /// Narrow learns from your words and keeps them verbatim.
    Reject(rank::FeedbackArgs),
    /// You like this opportunity (independent of saving or applying).
    Like(rank::FeedbackArgs),
    /// You dislike this opportunity (independent of rejecting it).
    Dislike(rank::FeedbackArgs),
    /// Take an opportunity off your saved list (without rejecting it).
    Unsave(rank::FeedbackArgs),
    /// You applied.
    Applied(rank::FeedbackArgs),
    /// You're interviewing.
    Interview(rank::FeedbackArgs),
    /// You got an offer.
    Offer(rank::FeedbackArgs),
    /// Opportunities you saved, applied to, are interviewing for or got an
    /// offer from.
    Pipeline(rank::PipelineArgs),
    /// Your feedback, verbatim, for every opportunity or one.
    Feedback(rank::LogArgs),
    /// What Narrow learned from your feedback, with the evidence behind
    /// every pattern, next to what you told it.
    Taste(rank::TasteArgs),
    /// Show, correct, export or import your career profile; add evidence
    /// from your LinkedIn export or public GitHub repositories.
    Profile(profile::ProfileArgs),
    /// Review the evidence behind your profile: list, confirm or reject claims.
    #[command(alias = "claim")]
    Claims(claims::ClaimsArgs),
    /// The evidence you approved that bears on one opportunity, as JSON, for
    /// an assistant helping with an application (writes nothing itself).
    Context(context::ContextArgs),
    /// Write everything that is yours (profile, evidence decisions,
    /// preferences, feedback and pipeline) as one versioned JSON file.
    Export(state::ExportArgs),
    /// Restore a file written by `narrow export`, atomically.
    Import(state::ImportArgs),
    /// Serve the Model Context Protocol on stdin/stdout, so an MCP client
    /// (Claude, ChatGPT, Codex, …) can use this same profile, jobs and
    /// feedback.
    Mcp,
    /// Check the setup: config and database paths, schema, profile, stored
    /// jobs, sources, and the MCP command for your client.
    Doctor,
    /// Show where Narrow keeps its files and the effective configuration.
    Config,
    /// What each job board yields and how healthy it is; find the job
    /// boards behind company careers pages.
    Sources(sources::SourcesArgs),
    /// Sign in to Narrow Cloud (in the browser, or with --token).
    Login(cloud::LoginArgs),
    /// Sign out of Narrow Cloud on this machine (--everywhere: on every
    /// device). Local data is kept.
    Logout(cloud::LogoutArgs),
    /// Your Narrow Cloud account and where this machine's sync stands.
    Account(cloud::AccountArgs),
    /// Sync your profile, evidence decisions, preferences and feedback with
    /// Narrow Cloud, and bring back what changed there. Conflicting
    /// changes are shown, never overwritten.
    Sync(cloud::SyncArgs),
    /// Personal access tokens for MCP clients and scripts.
    Token(cloud::TokenArgs),
    /// Narrow Cloud: serve the HTTP API and hosted MCP (configured by
    /// environment variables; see the README).
    #[command(hide = true)]
    Server,
    /// Narrow Cloud: run one scheduled job (a cron run), then exit.
    #[command(hide = true)]
    Worker(serve::WorkerArgs),
    /// Narrow Cloud: apply database migrations (the pre-deploy command).
    #[command(hide = true)]
    Migrate,
    /// Narrow Cloud: operator commands.
    #[command(hide = true)]
    Admin(serve::AdminArgs),
    /// The shortlist from stored jobs only (`narrow find --offline`); kept
    /// for scripts written before `find` became personalized.
    #[command(hide = true)]
    Rank(find::RankArgs),
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli).await {
        Ok(code) => code,
        Err(error) => {
            let mut stderr = anstream::stderr().lock();
            let red = anstyle::Style::new()
                .bold()
                .fg_color(Some(anstyle::AnsiColor::Red.into()));
            let _ = writeln!(stderr, "{red}error:{red:#} {error}");
            for cause in error.chain().skip(1) {
                let _ = writeln!(stderr, "  caused by: {cause}");
            }
            if let Some(hint) = error
                .downcast_ref::<jobhunt_app::AppError>()
                .and_then(|e| e.hint())
                .filter(|_| {
                    !matches!(
                        error.downcast_ref::<jobhunt_app::AppError>(),
                        Some(jobhunt_app::AppError::NoProfile | jobhunt_app::AppError::NoJobs)
                    )
                })
            {
                let _ = writeln!(stderr, "hint: {hint}");
            }
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> anyhow::Result<ExitCode> {
    // The cloud process modes are configured by their environment, and log
    // JSON by default (Railway collects stderr).
    if matches!(
        cli.command,
        Command::Server | Command::Worker(_) | Command::Migrate | Command::Admin(_)
    ) {
        let cloud = serve::cloud_config();
        let format = cli
            .log_format
            .map(Into::into)
            .unwrap_or(cloud.app.config.logging.format);
        logging::init(cli.verbose, &cloud.app.config.logging, format)?;
        return match cli.command {
            Command::Server => serve::serve(cloud).await,
            Command::Worker(args) => serve::worker(args, cloud).await,
            Command::Migrate => serve::migrate(cloud).await,
            Command::Admin(args) => serve::admin(args, cloud).await,
            _ => unreachable!("matched above"),
        };
    }
    let paths = Paths::platform();
    let loaded = config::load(
        cli.config.as_deref(),
        cli.database.as_deref(),
        paths.as_ref(),
    )?;
    let format = cli
        .log_format
        .map(Into::into)
        .unwrap_or(loaded.config.logging.format);
    logging::init(cli.verbose, &loaded.config.logging, format)?;
    tracing::debug!(
        config_file = ?loaded.file,
        database = %loaded.database.display(),
        "configuration loaded"
    );

    match cli.command {
        Command::Init(args) => init::run(args, &loaded).await,
        Command::Preferences(args) => preferences::run(args, &loaded).await,
        Command::Find(args) => find::run(args, &loaded, cli.verbose).await,
        Command::Show(args) => show::run(args, &loaded).await,
        Command::Why(args) => rank::why(args, &loaded).await,
        Command::Verify(args) => verify::run(args, &loaded).await,
        Command::Check(args) => check::run(args, &loaded).await,
        Command::Save(args) => rank::feedback(FeedbackAction::Save, args, &loaded).await,
        Command::Reject(args) => rank::feedback(FeedbackAction::Reject, args, &loaded).await,
        Command::Like(args) => rank::feedback(FeedbackAction::Like, args, &loaded).await,
        Command::Dislike(args) => rank::feedback(FeedbackAction::Dislike, args, &loaded).await,
        Command::Unsave(args) => rank::feedback(FeedbackAction::Unsave, args, &loaded).await,
        Command::Applied(args) => rank::feedback(FeedbackAction::Applied, args, &loaded).await,
        Command::Interview(args) => rank::feedback(FeedbackAction::Interview, args, &loaded).await,
        Command::Offer(args) => rank::feedback(FeedbackAction::Offer, args, &loaded).await,
        Command::Pipeline(args) => rank::pipeline(args, &loaded).await,
        Command::Feedback(args) => rank::log(args, &loaded).await,
        Command::Taste(args) => rank::taste(args, &loaded).await,
        Command::Profile(args) => profile::run(args, &loaded).await,
        Command::Claims(args) => claims::run(args, &loaded).await,
        Command::Context(args) => context::run(args, &loaded).await,
        Command::Export(args) => state::export(args, &loaded).await,
        Command::Import(args) => state::import(args, &loaded).await,
        Command::Mcp => mcp(&loaded).await,
        Command::Doctor => doctor::run(&loaded).await,
        Command::Config => show_config(&loaded),
        Command::Sources(args) => sources::run(args, &loaded).await,
        Command::Login(args) => cloud::login(args, &loaded).await,
        Command::Logout(args) => cloud::logout(args).await,
        Command::Account(args) => cloud::account(args, &loaded).await,
        Command::Sync(args) => cloud::sync(args, &loaded).await,
        Command::Token(args) => cloud::token(args).await,
        Command::Server | Command::Worker(_) | Command::Migrate | Command::Admin(_) => {
            unreachable!("handled before loading the local configuration")
        }
        Command::Rank(args) => find::rank(args, &loaded).await,
    }
}

/// `narrow mcp`: the same configuration and database, served over stdio.
/// Nothing but protocol messages is written to stdout; logs go to stderr.
async fn mcp(loaded: &LoadedConfig) -> anyhow::Result<ExitCode> {
    let app = local::open(loaded).await?;
    jobhunt_mcp::serve_stdio(app).await?;
    Ok(ExitCode::SUCCESS)
}

fn show_config(loaded: &LoadedConfig) -> anyhow::Result<ExitCode> {
    let mut out = anstream::stdout().lock();
    let config_file = match (&loaded.file, &loaded.default_file) {
        (Some(file), _) => file.display().to_string(),
        (None, Some(default)) => format!("{} (not found, using defaults)", default.display()),
        (None, None) => "(none)".to_owned(),
    };
    let effective =
        toml::to_string_pretty(&loaded.config).context("could not render the configuration")?;
    writeln!(out, "Config file: {config_file}")?;
    writeln!(out, "Database:    {}", loaded.database.display())?;
    writeln!(out)?;
    writeln!(out, "# Effective configuration")?;
    write!(out, "{effective}")?;
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn parses_find_arguments() {
        let cli = Cli::try_parse_from([
            "narrow",
            "find",
            "rust",
            "backend",
            "-n",
            "5",
            "--source",
            "ashby:linear",
            "-vv",
        ])
        .unwrap();
        assert_eq!(cli.verbose, 2);
        let Command::Find(args) = cli.command else {
            panic!("expected find");
        };
        assert_eq!(args.query, ["rust", "backend"]);
        assert_eq!(args.limit, Some(5));
        assert_eq!(
            args.sources[0],
            jobhunt_app::SourceArg::Key("ashby:linear".parse().unwrap())
        );
        assert!(!args.offline);
    }

    #[test]
    fn parses_show_and_url_sources() {
        let cli = Cli::try_parse_from(["narrow", "show", "job_02e51190085f8a9a0772e845ddd9f329"])
            .unwrap();
        assert!(matches!(cli.command, Command::Show(_)));
        let cli = Cli::try_parse_from([
            "narrow",
            "find",
            "--source",
            "https://www.notion.com/careers",
        ])
        .unwrap();
        let Command::Find(args) = cli.command else {
            panic!("expected find");
        };
        assert!(matches!(args.sources[0], jobhunt_app::SourceArg::Url(_)));
    }

    #[test]
    fn parses_profile_commands() {
        let cli = Cli::try_parse_from(["narrow", "init", "resume.pdf"]).unwrap();
        assert!(matches!(cli.command, Command::Init(_)));
        let cli = Cli::try_parse_from(["narrow", "profile"]).unwrap();
        let Command::Profile(args) = cli.command else {
            panic!("expected profile");
        };
        assert!(args.command.is_none());
        let cli = Cli::try_parse_from([
            "narrow",
            "profile",
            "edit",
            "experience",
            "exp_12",
            "--title",
            "Staff Engineer",
            "--start",
            "2021-03",
            "--end",
            "none",
        ])
        .unwrap();
        assert!(matches!(cli.command, Command::Profile(_)));
        for command in [
            vec![
                "narrow",
                "profile",
                "import-linkedin",
                "Basic_LinkedInDataExport.zip",
            ],
            vec!["narrow", "profile", "import-github", "octocat"],
            vec!["narrow", "profile", "import-github"],
            vec!["narrow", "profile", "remove-source", "github"],
        ] {
            assert!(Cli::try_parse_from(&command).is_ok(), "{command:?}");
        }
        assert!(Cli::try_parse_from(["narrow", "profile", "import-linkedin"]).is_err());
        assert!(Cli::try_parse_from(["narrow", "profile", "remove-source", "resume"]).is_err());
        let cli = Cli::try_parse_from(["narrow", "claim", "confirm", "clm_1", "clm_2"]).unwrap();
        assert!(matches!(cli.command, Command::Claims(_)));
        let cli = Cli::try_parse_from(["narrow", "claims", "--state", "review"]).unwrap();
        assert!(matches!(cli.command, Command::Claims(_)));
        let cli =
            Cli::try_parse_from(["narrow", "preferences", "add", "I want small product teams"])
                .unwrap();
        assert!(matches!(cli.command, Command::Preferences(_)));
        let cli = Cli::try_parse_from([
            "narrow",
            "prefs",
            "set",
            "compensation",
            "--minimum",
            "120k",
            "--currency",
            "USD",
        ])
        .unwrap();
        assert!(matches!(cli.command, Command::Preferences(_)));
        assert!(
            Cli::try_parse_from([
                "narrow",
                "profile",
                "edit",
                "experience",
                "exp_1",
                "--start",
                "soon"
            ])
            .is_err()
        );
    }

    #[test]
    fn parses_verify() {
        let cli = Cli::try_parse_from([
            "narrow",
            "verify",
            "opp_02e51190085f8a9a0772e845ddd9f329",
            "--force",
            "-d",
        ])
        .unwrap();
        let Command::Verify(args) = cli.command else {
            panic!("expected verify");
        };
        assert!(args.force && args.details);
    }

    #[test]
    fn parses_ranking_and_feedback() {
        let cli = Cli::try_parse_from([
            "narrow",
            "reject",
            "opp_02e51190085f8a9a0772e845ddd9f329",
            "--reason",
            "too corporate",
        ])
        .unwrap();
        let Command::Reject(args) = cli.command else {
            panic!("expected reject");
        };
        assert_eq!(args.reason.as_deref(), Some("too corporate"));
        for verb in [
            "save",
            "unsave",
            "like",
            "dislike",
            "applied",
            "interview",
            "offer",
            "why",
        ] {
            assert!(
                Cli::try_parse_from(["narrow", verb, "job_1"]).is_ok(),
                "{verb}"
            );
        }
        let cli = Cli::try_parse_from(["narrow", "rank", "rust", "-n", "3", "--all"]).unwrap();
        let Command::Rank(args) = cli.command else {
            panic!("expected rank");
        };
        assert_eq!((args.query.len(), args.limit, args.all), (1, 3, true));
        let cli = Cli::try_parse_from(["narrow", "find", "--refresh", "--all", "-n", "3"]).unwrap();
        let Command::Find(args) = cli.command else {
            panic!("expected find");
        };
        assert!(args.refresh && args.all && !args.raw);
        assert!(Cli::try_parse_from(["narrow", "find", "--refresh", "--offline"]).is_err());
        assert!(Cli::try_parse_from(["narrow", "find", "--raw", "--json"]).is_err());
        for command in [
            vec!["narrow", "mcp"],
            vec!["narrow", "doctor"],
            vec!["narrow", "export", "-o", "state.json"],
            vec!["narrow", "import", "state.json", "--replace"],
            vec!["narrow", "context", "opp_1234", "--contact"],
            vec!["narrow", "show", "opp_1234", "--json"],
            vec!["narrow", "verify", "opp_1234", "--json"],
            vec!["narrow", "pipeline", "--json"],
        ] {
            assert!(Cli::try_parse_from(&command).is_ok(), "{command:?}");
        }
        assert!(Cli::try_parse_from(["narrow", "taste", "--all"]).is_ok());
        assert!(Cli::try_parse_from(["narrow", "pipeline"]).is_ok());
        assert!(Cli::try_parse_from(["narrow", "feedback"]).is_ok());
        assert!(Cli::try_parse_from(["narrow", "reject"]).is_err());
    }

    #[test]
    fn parses_cloud_commands() {
        for command in [
            vec!["narrow", "login", "--server", "https://api.example.com"],
            vec!["narrow", "login", "--token"],
            vec!["narrow", "logout", "--everywhere"],
            vec!["narrow", "account", "--json"],
            vec!["narrow", "sync"],
            vec!["narrow", "sync", "--status"],
            vec!["narrow", "sync", "--keep", "local", "--record", "clm_1"],
            vec![
                "narrow",
                "token",
                "create",
                "Claude Desktop",
                "--days",
                "30",
            ],
            vec!["narrow", "token", "list"],
            vec!["narrow", "token", "revoke", "tok_1"],
            vec!["narrow", "server"],
            vec!["narrow", "worker", "discovery", "--budget-minutes", "5"],
            vec!["narrow", "worker", "verification"],
            vec!["narrow", "worker", "notify"],
            vec!["narrow", "migrate"],
            vec!["narrow", "admin", "status", "--json"],
            vec!["narrow", "admin", "reencrypt"],
        ] {
            assert!(Cli::try_parse_from(&command).is_ok(), "{command:?}");
        }
        assert!(Cli::try_parse_from(["narrow", "sync", "--record", "clm_1"]).is_err());
        assert!(Cli::try_parse_from(["narrow", "sync", "--status", "--keep", "cloud"]).is_err());
    }

    #[test]
    fn rejects_malformed_source_keys() {
        assert!(Cli::try_parse_from(["narrow", "find", "--source", "linear"]).is_err());
    }
}
