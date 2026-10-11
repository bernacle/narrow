//! Refreshing discovery: which sources to read, whether stored jobs are
//! fresh enough to skip it, and running the pipeline.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use jobhunt_core::{ErrorChain, SourceKey};
use jobhunt_jobs::{Discovery, DiscoveryReport, JobRepository};
use jobhunt_sources::{CareersPage, HttpClient, SourceSpec, careers};
use url::Url;

use crate::config::AppConfig;
use crate::error::AppError;
use crate::{LocalApp, Progress, ProgressEvent};

/// Sends every discovery request to this base URL instead of the real
/// hosts. For offline tests against a local server; not a user setting.
pub const DISCOVERY_ENDPOINT_OVERRIDE: &str = "JOBHUNT_DISCOVERY_ENDPOINT";

/// Whether to read sources before answering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RefreshMode {
    /// Refresh only when a configured source was never read, or was read
    /// longer ago than `discovery.refresh_after_hours`.
    #[default]
    Auto,
    /// Always refresh.
    Always,
    /// Never touch the network; use what is stored.
    Never,
}

/// Why a refresh happened, or didn't.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshReason {
    /// Asked for explicitly.
    Requested,
    /// Some configured sources were never read.
    NeverRead { sources: usize },
    /// Some configured sources were read too long ago.
    Stale { oldest: DateTime<Utc> },
    /// Everything was read recently enough.
    Fresh { oldest: DateTime<Utc> },
    /// Refreshing was turned off.
    Disabled,
    /// Job boards are read by scheduled workers, not by searches
    /// ([`crate::DiscoveryMode::Background`]); `oldest` is when the
    /// least recently read source was read.
    Background { oldest: Option<DateTime<Utc>> },
}

impl RefreshReason {
    pub fn describe(&self, now: DateTime<Utc>) -> String {
        use jobhunt_jobs::verification::ago;
        match self {
            Self::Requested => "refresh requested".into(),
            Self::NeverRead { sources } if *sources == 1 => "1 source was never read".into(),
            Self::NeverRead { sources } => format!("{sources} sources were never read"),
            Self::Stale { oldest } => format!("sources last read {}", ago(now - *oldest)),
            Self::Fresh { oldest } => format!("every source read {}", ago(now - *oldest)),
            Self::Disabled => "working offline".into(),
            Self::Background {
                oldest: Some(oldest),
            } => format!(
                "job boards are read in the background; the oldest was read {}",
                ago(now - *oldest)
            ),
            Self::Background { oldest: None } => {
                "job boards are read in the background; none has been read yet".into()
            }
        }
    }
}

/// What a refresh did.
#[derive(Debug)]
pub struct Refresh {
    pub reason: RefreshReason,
    /// `None` when nothing was fetched.
    pub report: Option<DiscoveryReport>,
    /// Problems that did not stop the refresh (a careers page that could
    /// not be read, for example).
    pub warnings: Vec<String>,
}

/// Which sources a refresh reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceArg {
    Key(SourceKey),
    Url(Url),
}

impl std::str::FromStr for SourceArg {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.contains("://") {
            let url = Url::parse(s).map_err(|e| format!("invalid URL {s:?}: {e}"))?;
            if !matches!(url.scheme(), "http" | "https") {
                return Err(format!("{s:?} is not an http(s) URL"));
            }
            return Ok(Self::Url(url));
        }
        s.parse().map(Self::Key).map_err(|e| e.to_string())
    }
}

/// The HTTP client discovery uses, from configuration.
pub fn http_client(config: &AppConfig) -> Result<HttpClient, AppError> {
    HttpClient::new(config.discovery.http_settings())
        .map_err(|e| AppError::Config(format!("HTTP client: {e}")))
}

/// Every configured source, including boards found on configured careers
/// pages (pages that cannot be read are reported in `warnings`).
pub async fn configured_sources(
    config: &AppConfig,
    http: &HttpClient,
    warnings: &mut Vec<String>,
) -> Result<Vec<SourceSpec>, AppError> {
    select_sources(&[], config, http, false, warnings).await
}

/// Runs the discovery pipeline over `specs`, storing into `store`: the one
/// implementation of discovery, used by `find` and by the cloud's
/// scheduled workers alike (conditional requests, lifecycle, history and
/// grouping included).
pub async fn run_discovery(
    store: &dyn JobRepository,
    config: &AppConfig,
    http: &HttpClient,
    specs: &[SourceSpec],
) -> Result<DiscoveryReport, AppError> {
    let base = std::env::var(DISCOVERY_ENDPOINT_OVERRIDE)
        .ok()
        .filter(|b| !b.trim().is_empty());
    let sources = specs
        .iter()
        .map(|spec| spec.build_at(http, base.as_deref()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| AppError::InvalidArguments(e.to_string()))?;
    let settings = &config.discovery;
    Discovery::new(store)
        .with_concurrency(settings.concurrency)
        .with_validator_max_age(settings.validator_max_age())
        .run(&sources)
        .await
        .map_err(|e| AppError::storage("saving discovered jobs", e))
}

impl LocalApp {
    fn http(&self) -> Result<HttpClient, AppError> {
        http_client(self.config())
    }

    /// Whether any open job is stored.
    pub async fn has_open_jobs(&self) -> Result<bool, AppError> {
        use jobhunt_jobs::{JobQuery, JobStatus};
        let query = JobQuery {
            status: Some(JobStatus::Open),
            limit: Some(1),
            ..JobQuery::default()
        };
        Ok(self.store().count(&query).await? > 0)
    }

    /// Whether stored jobs are fresh enough to answer from, per
    /// configured source (careers pages are resolved only when fetching,
    /// so they don't count here).
    pub async fn freshness(&self, now: DateTime<Utc>) -> Result<RefreshReason, AppError> {
        let configured = self
            .config()
            .sources
            .specs()
            .map_err(|e| AppError::Config(e.to_string()))?;
        let checked: HashMap<SourceKey, DateTime<Utc>> = self.store().last_checked().await?;
        if self.discovery_mode() == crate::DiscoveryMode::Background {
            let oldest = configured
                .iter()
                .filter_map(|s| checked.get(s.key()))
                .min()
                .copied()
                .or_else(|| checked.values().min().copied());
            return Ok(RefreshReason::Background { oldest });
        }
        let never = configured
            .iter()
            .filter(|s| !checked.contains_key(s.key()))
            .count();
        if never > 0 {
            return Ok(RefreshReason::NeverRead { sources: never });
        }
        let oldest = configured
            .iter()
            .filter_map(|s| checked.get(s.key()))
            .min()
            .copied()
            // Only careers pages configured: nothing to judge by.
            .or_else(|| checked.values().min().copied());
        match oldest {
            None => Ok(RefreshReason::NeverRead { sources: 0 }),
            Some(oldest) if now - oldest >= self.config().discovery.refresh_after() => {
                Ok(RefreshReason::Stale { oldest })
            }
            Some(oldest) => Ok(RefreshReason::Fresh { oldest }),
        }
    }

    /// Reads sources when `mode` says so: every configured source, or the
    /// requested ones. In [`crate::DiscoveryMode::Background`] nothing is
    /// read: scheduled workers keep the shared corpus fresh.
    pub async fn refresh(
        &self,
        mode: RefreshMode,
        requested: &[SourceArg],
        progress: &dyn Progress,
        now: DateTime<Utc>,
    ) -> Result<Refresh, AppError> {
        if self.discovery_mode() == crate::DiscoveryMode::Background {
            return Ok(Refresh {
                reason: self.freshness(now).await?,
                report: None,
                warnings: Vec::new(),
            });
        }
        let reason = match mode {
            RefreshMode::Never => RefreshReason::Disabled,
            RefreshMode::Always => RefreshReason::Requested,
            RefreshMode::Auto if !requested.is_empty() => RefreshReason::Requested,
            RefreshMode::Auto => self.freshness(now).await?,
        };
        if matches!(
            reason,
            RefreshReason::Fresh { .. } | RefreshReason::Disabled
        ) {
            return Ok(Refresh {
                reason,
                report: None,
                warnings: Vec::new(),
            });
        }
        let http = self.http()?;
        let mut warnings = Vec::new();
        let specs = select_sources(requested, self.config(), &http, false, &mut warnings).await?;
        progress.note(ProgressEvent::Refreshing {
            sources: specs.len(),
            reason: reason.clone(),
        });
        let report = run_discovery(self.store(), self.config(), &http, &specs).await?;
        if report.succeeded() == 0 && !report.sources.is_empty() {
            let detail = report
                .failures()
                .next()
                .map(|(source, error)| format!("{source}: {}", ErrorChain(error)))
                .unwrap_or_default();
            return Err(AppError::SourceUnavailable {
                failed: report.sources.len(),
                detail,
            });
        }
        Ok(Refresh {
            reason,
            report: Some(report),
            warnings,
        })
    }

    /// The sources a search reads, without fetching anything (careers
    /// pages that need resolving are skipped).
    pub async fn selected_sources(
        &self,
        requested: &[SourceArg],
    ) -> Result<Vec<SourceSpec>, AppError> {
        let http = self.http()?;
        select_sources(requested, self.config(), &http, true, &mut Vec::new()).await
    }
}

/// The sources to read: the ones named (using configured details such as
/// the company name when available), else all configured ones, including
/// boards found on configured careers pages.
async fn select_sources(
    requested: &[SourceArg],
    config: &AppConfig,
    http: &HttpClient,
    offline: bool,
    warnings: &mut Vec<String>,
) -> Result<Vec<SourceSpec>, AppError> {
    let configured = config
        .sources
        .specs()
        .map_err(|e| AppError::Config(format!("invalid source in configuration: {e}")))?;
    let mut selected: Vec<SourceSpec> = Vec::new();
    let add = |spec: SourceSpec, selected: &mut Vec<SourceSpec>| {
        if !selected.iter().any(|s| s.key() == spec.key()) {
            let spec = configured
                .iter()
                .find(|c| c.key() == spec.key())
                .cloned()
                .unwrap_or(spec);
            selected.push(spec);
        }
    };

    if requested.is_empty() {
        for spec in configured.iter().cloned() {
            add(spec, &mut selected);
        }
        if !offline {
            let resolved = futures::future::join_all(
                config
                    .sources
                    .careers
                    .iter()
                    .map(|page| async move { (page, resolve_page(http, page).await) }),
            )
            .await;
            for (page, result) in resolved {
                match result {
                    Ok(specs) => specs.into_iter().for_each(|s| add(s, &mut selected)),
                    Err(error) => {
                        warnings.push(format!("skipped careers page {}: {error}", page.url))
                    }
                }
            }
        }
        return Ok(selected);
    }

    for arg in requested {
        match arg {
            SourceArg::Key(key) => add(
                SourceSpec::from_key(key).map_err(|e| AppError::InvalidArguments(e.to_string()))?,
                &mut selected,
            ),
            SourceArg::Url(url) => {
                let specs = match careers::board_for_url(url) {
                    Some(board) => vec![
                        SourceSpec::from_board(&board, None)
                            .map_err(|e| AppError::InvalidArguments(e.to_string()))?,
                    ],
                    None if offline => {
                        return Err(AppError::InvalidArguments(format!(
                            "can't find the job board behind {url} while offline"
                        )));
                    }
                    None => {
                        let page = CareersPage {
                            url: url.to_string(),
                            company: None,
                        };
                        resolve_page(http, &page)
                            .await
                            .map_err(AppError::InvalidArguments)?
                    }
                };
                specs.into_iter().for_each(|s| add(s, &mut selected));
            }
        }
    }
    Ok(selected)
}

/// The supported boards a careers page uses.
async fn resolve_page(http: &HttpClient, page: &CareersPage) -> Result<Vec<SourceSpec>, String> {
    let url = Url::parse(&page.url).map_err(|e| format!("invalid URL {:?}: {e}", page.url))?;
    let boards = careers::detect(http, &url)
        .await
        .map_err(|e| format!("could not read {url}: {}", ErrorChain(&e)))?;
    if boards.is_empty() {
        return Err(format!(
            "found no Ashby, Greenhouse, Lever or YC job board on {url} \
             (pages that load jobs with JavaScript can't be read; configure the board instead)"
        ));
    }
    let specs = boards
        .iter()
        .map(|board| SourceSpec::from_board(board, page.company.clone()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    tracing::info!(
        page = %url,
        boards = ?specs.iter().map(|s| s.key().to_string()).collect::<Vec<_>>(),
        "careers page resolved"
    );
    Ok(specs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn http() -> HttpClient {
        HttpClient::new(jobhunt_sources::HttpSettings::default()).unwrap_or_else(|e| panic!("{e}"))
    }

    fn keys(specs: &[SourceSpec]) -> Vec<String> {
        specs.iter().map(|s| s.key().to_string()).collect()
    }

    #[tokio::test]
    async fn selects_configured_sources_by_default() {
        let config = AppConfig::default();
        let selected = select_sources(&[], &config, &http(), true, &mut Vec::new())
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            selected.len(),
            config.sources.specs().map(|s| s.len()).unwrap_or(0)
        );
    }

    #[tokio::test]
    async fn requested_sources_reuse_configured_details() {
        let config = AppConfig::default();
        let requested: Vec<SourceArg> = [
            "ashby:linear",
            "ashby:posthog",
            "ashby:linear",
            "greenhouse:somethingelse",
            "https://jobs.lever.co/spotify/2193db3f-77c5-43b8-b030-8f92c9882bf1",
            "https://www.ycombinator.com/companies/posthog/jobs",
        ]
        .iter()
        .filter_map(|s| s.parse().ok())
        .collect();
        let selected = select_sources(&requested, &config, &http(), true, &mut Vec::new())
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            keys(&selected),
            [
                "ashby:linear",
                "ashby:posthog",
                "greenhouse:somethingelse",
                "lever:spotify",
                "yc:posthog"
            ]
        );
        let SourceSpec::Ashby { board, .. } = &selected[0] else {
            panic!("expected ashby");
        };
        assert_eq!(board.company.as_deref(), Some("Linear"));
        let SourceSpec::Lever { site, .. } = &selected[3] else {
            panic!("expected lever");
        };
        assert_eq!(
            site.company.as_deref(),
            Some("Spotify"),
            "configured name reused"
        );
    }

    #[tokio::test]
    async fn unknown_kinds_and_unrecognized_urls_offline_are_rejected() {
        let config = AppConfig::default();
        let unknown: Vec<SourceArg> =
            vec!["workday:acme".parse().unwrap_or_else(|e| panic!("{e}"))];
        assert!(
            select_sources(&unknown, &config, &http(), true, &mut Vec::new())
                .await
                .is_err()
        );
        let page: Vec<SourceArg> = vec![
            "https://example.com/careers"
                .parse()
                .unwrap_or_else(|e| panic!("{e}")),
        ];
        let error = select_sources(&page, &config, &http(), true, &mut Vec::new())
            .await
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(error.contains("offline"));
    }

    /// Two paths to one board (configured directly, and through a careers
    /// page that resolves to it) read it once, so its jobs keep one
    /// identity and one opportunity each.
    #[tokio::test]
    async fn a_careers_page_resolving_to_a_configured_board_is_read_once() {
        let config: AppConfig = toml::from_str(
            r#"
                [[sources.ashby]]
                board = "railway"
                company = "Railway"

                [[sources.careers]]
                url = "https://jobs.ashbyhq.com/Railway"
                company = "Railway (careers page)"
            "#,
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let mut warnings = Vec::new();
        let selected = select_sources(&[], &config, &http(), false, &mut warnings)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(keys(&selected), vec!["ashby:railway"]);
        assert!(warnings.is_empty(), "{warnings:?}");
        let SourceSpec::Ashby { board, .. } = &selected[0] else {
            panic!("expected ashby");
        };
        assert_eq!(
            board.company.as_deref(),
            Some("Railway"),
            "configured details win"
        );
    }

    #[test]
    fn parses_source_arguments() {
        assert_eq!(
            "lever:spotify".parse::<SourceArg>(),
            Ok(SourceArg::Key(
                "lever:spotify".parse().unwrap_or_else(|e| panic!("{e}"))
            ))
        );
        assert!(matches!(
            "https://jobs.lever.co/spotify".parse::<SourceArg>(),
            Ok(SourceArg::Url(_))
        ));
        assert!("ftp://example.com".parse::<SourceArg>().is_err());
        assert!("spotify".parse::<SourceArg>().is_err());
    }
}
