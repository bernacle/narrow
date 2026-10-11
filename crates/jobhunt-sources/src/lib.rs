//! Source adapters.
//!
//! Each adapter implements [`jobhunt_core::Source`] with
//! `Record = JobPosting` and owns everything specific to its source: the
//! endpoint, the payload format, and the mapping into the canonical model.
//! Supported families:
//!
//! | kind         | instance        | reads                                              |
//! |--------------|-----------------|----------------------------------------------------|
//! | `ashby`      | board name      | `api.ashbyhq.com/posting-api/job-board/<board>`    |
//! | `greenhouse` | board token     | `boards-api.greenhouse.io/v1/boards/<board>/jobs`  |
//! | `lever`      | site name       | `api.lever.co/v0/postings/<site>`                  |
//! | `yc`         | company slug    | `www.ycombinator.com/companies/<slug>/jobs`        |
//!
//! [`careers`] maps company careers pages onto these boards, [`company`]
//! finds a company's careers page and board from its domain, [`registry`]
//! keeps what is known about each board (status, provenance, health and
//! yield rules), and [`verify`] checks single jobs against the same
//! sources. [`github`] is
//! not a job source: it reads a public GitHub account as evidence for the
//! profile, over the same HTTP client.
//!
//! Adding a source family means:
//! 1. a module with the adapter, its raw payload types and conversion;
//! 2. a variant on [`SourceSpec`] plus its arms in [`SourceSpec::from_key`],
//!    [`SourceSpec::key`] and [`SourceSpec::build`];
//! 3. a config list on [`SourcesConfig`];
//! 4. fixture tests under `tests/fixtures/<source>/`, HTTP tests against a
//!    local mock, and an `#[ignore]` live test.
//!
//! Nothing outside this crate (pipeline, storage, CLI output) changes.

pub mod ashby;
pub mod careers;
mod common;
pub mod company;
pub mod discovery;
pub mod github;
pub mod greenhouse;
pub mod http;
pub mod lever;
pub mod registry;
pub mod verify;
pub mod yc;

use std::collections::HashSet;

use jobhunt_core::{SourceError, SourceKey, SourceKeyError};
use jobhunt_jobs::JobSource;
use serde::{Deserialize, Deserializer, Serialize};

pub use ashby::{AshbyBoard, AshbySource};
pub use careers::{BoardRef, CareersPage};
pub use github::{GithubError, GithubReader};
pub use greenhouse::{GreenhouseBoard, GreenhouseSource};
pub use http::{HttpClient, HttpClientError, HttpSettings, Probe};
pub use lever::{LeverRegion, LeverSite, LeverSource};
pub use verify::{HttpVerifier, VerifierHosts};
pub use yc::{YcCompany, YcSource};

/// Which sources discovery reads by default.
///
/// With no `[sources]` configuration at all, a built-in selection from every
/// family is used. As soon as any family is configured, only the configured
/// sources are read (families left out are empty), so a config file fully
/// controls what is searched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourcesConfig {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ashby: Vec<AshbyBoard>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub greenhouse: Vec<GreenhouseBoard>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub lever: Vec<LeverSite>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub yc: Vec<YcCompany>,
    /// Company careers pages; each is resolved to the board it embeds.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub careers: Vec<CareersPage>,
}

impl Default for SourcesConfig {
    fn default() -> Self {
        Self {
            ashby: ashby::default_boards(),
            greenhouse: greenhouse::default_boards(),
            lever: lever::default_sites(),
            yc: yc::default_companies(),
            careers: Vec::new(),
        }
    }
}

impl<'de> Deserialize<'de> for SourcesConfig {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            ashby: Option<Vec<AshbyBoard>>,
            greenhouse: Option<Vec<GreenhouseBoard>>,
            lever: Option<Vec<LeverSite>>,
            yc: Option<Vec<YcCompany>>,
            careers: Option<Vec<CareersPage>>,
        }
        let raw = Raw::deserialize(deserializer)?;
        let nothing_configured = raw.ashby.is_none()
            && raw.greenhouse.is_none()
            && raw.lever.is_none()
            && raw.yc.is_none()
            && raw.careers.is_none();
        if nothing_configured {
            return Ok(Self::default());
        }
        Ok(Self {
            ashby: raw.ashby.unwrap_or_default(),
            greenhouse: raw.greenhouse.unwrap_or_default(),
            lever: raw.lever.unwrap_or_default(),
            yc: raw.yc.unwrap_or_default(),
            careers: raw.careers.unwrap_or_default(),
        })
    }
}

impl SourcesConfig {
    /// Every configured board-backed source, validated. Careers pages are
    /// resolved separately (that needs the network), see [`careers::detect`].
    pub fn specs(&self) -> Result<Vec<SourceSpec>, SourcesConfigError> {
        let specs = self
            .ashby
            .iter()
            .cloned()
            .map(SourceSpec::ashby)
            .chain(self.greenhouse.iter().cloned().map(SourceSpec::greenhouse))
            .chain(self.lever.iter().cloned().map(SourceSpec::lever))
            .chain(self.yc.iter().cloned().map(SourceSpec::yc))
            .collect::<Result<Vec<_>, _>>()?;
        let mut seen = HashSet::new();
        for spec in &specs {
            if !seen.insert(spec.key().clone()) {
                return Err(SourcesConfigError::Duplicate(spec.key().clone()));
            }
        }
        for page in &self.careers {
            url::Url::parse(&page.url)
                .ok()
                .filter(|u| matches!(u.scheme(), "http" | "https"))
                .ok_or_else(|| SourcesConfigError::InvalidUrl(page.url.clone()))?;
        }
        Ok(specs)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SourcesConfigError {
    #[error(transparent)]
    InvalidName(#[from] SourceKeyError),
    #[error("source {0} is configured more than once")]
    Duplicate(SourceKey),
    #[error("careers page {0:?} is not an http(s) URL")]
    InvalidUrl(String),
}

/// A fully described source instance that can be turned into an adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceSpec {
    Ashby {
        key: SourceKey,
        board: AshbyBoard,
    },
    Greenhouse {
        key: SourceKey,
        board: GreenhouseBoard,
    },
    Lever {
        key: SourceKey,
        site: LeverSite,
    },
    Yc {
        key: SourceKey,
        company: YcCompany,
    },
}

impl SourceSpec {
    pub fn ashby(board: AshbyBoard) -> Result<Self, SourceKeyError> {
        let key = SourceKey::new(ashby::KIND, &board.board)?;
        Ok(Self::Ashby { key, board })
    }

    pub fn greenhouse(board: GreenhouseBoard) -> Result<Self, SourceKeyError> {
        let key = SourceKey::new(greenhouse::KIND, &board.board)?;
        Ok(Self::Greenhouse { key, board })
    }

    pub fn lever(site: LeverSite) -> Result<Self, SourceKeyError> {
        let key = SourceKey::new(lever::KIND, &site.site)?;
        Ok(Self::Lever { key, site })
    }

    pub fn yc(company: YcCompany) -> Result<Self, SourceKeyError> {
        let key = SourceKey::new(yc::KIND, &company.slug)?;
        Ok(Self::Yc { key, company })
    }

    /// Describes an unconfigured source from its key alone (for example
    /// `greenhouse:stripe` given on the command line).
    pub fn from_key(key: &SourceKey) -> Result<Self, UnknownSourceKind> {
        let name = key.instance().to_owned();
        let key = key.clone();
        match key.kind() {
            ashby::KIND => Ok(Self::Ashby {
                key,
                board: AshbyBoard {
                    board: name,
                    company: None,
                },
            }),
            greenhouse::KIND => Ok(Self::Greenhouse {
                key,
                board: GreenhouseBoard {
                    board: name,
                    company: None,
                },
            }),
            lever::KIND => Ok(Self::Lever {
                key,
                site: LeverSite {
                    site: name,
                    company: None,
                    region: LeverRegion::Global,
                },
            }),
            yc::KIND => Ok(Self::Yc {
                key,
                company: YcCompany {
                    slug: name,
                    company: None,
                },
            }),
            other => Err(UnknownSourceKind(other.to_owned())),
        }
    }

    /// Describes the source for a board found by [`careers`], with an
    /// optional company display name.
    pub fn from_board(board: &BoardRef, company: Option<String>) -> Result<Self, SourceKeyError> {
        match board.kind {
            greenhouse::KIND => Self::greenhouse(GreenhouseBoard {
                board: board.name.clone(),
                company,
            }),
            lever::KIND => Self::lever(LeverSite {
                site: board.name.clone(),
                company,
                region: board.lever_region,
            }),
            yc::KIND => Self::yc(YcCompany {
                slug: board.name.clone(),
                company,
            }),
            _ => Self::ashby(AshbyBoard {
                board: board.name.clone(),
                company,
            }),
        }
    }

    pub fn key(&self) -> &SourceKey {
        match self {
            Self::Ashby { key, .. }
            | Self::Greenhouse { key, .. }
            | Self::Lever { key, .. }
            | Self::Yc { key, .. } => key,
        }
    }

    /// The configured display name of the company, if any.
    pub fn company(&self) -> Option<&str> {
        match self {
            Self::Ashby { board, .. } => board.company.as_deref(),
            Self::Greenhouse { board, .. } => board.company.as_deref(),
            Self::Lever { site, .. } => site.company.as_deref(),
            Self::Yc { company, .. } => company.company.as_deref(),
        }
    }

    pub fn build(&self, http: &HttpClient) -> Result<Box<JobSource>, SourceError> {
        self.build_at(http, None)
    }

    /// Like [`SourceSpec::build`], but with every API request sent to
    /// `base` instead of the source's real host when it is given. For
    /// offline tests against a local server; not a user setting.
    pub fn build_at(
        &self,
        http: &HttpClient,
        base: Option<&str>,
    ) -> Result<Box<JobSource>, SourceError> {
        let http = http.clone();
        if let Some(base) = base {
            return Ok(match self {
                Self::Ashby { key, board } => Box::new(AshbySource::with_api_base(
                    key.clone(),
                    board.clone(),
                    http,
                    base,
                )?),
                Self::Greenhouse { key, board } => Box::new(GreenhouseSource::with_api_base(
                    key.clone(),
                    board.clone(),
                    http,
                    base,
                )?),
                Self::Lever { key, site } => Box::new(LeverSource::with_api_base(
                    key.clone(),
                    site.clone(),
                    http,
                    base,
                )?),
                Self::Yc { key, company } => Box::new(YcSource::with_base(
                    key.clone(),
                    company.clone(),
                    http,
                    base,
                )?),
            });
        }
        Ok(match self {
            Self::Ashby { key, board } => {
                Box::new(AshbySource::new(key.clone(), board.clone(), http)?)
            }
            Self::Greenhouse { key, board } => {
                Box::new(GreenhouseSource::new(key.clone(), board.clone(), http)?)
            }
            Self::Lever { key, site } => {
                Box::new(LeverSource::new(key.clone(), site.clone(), http)?)
            }
            Self::Yc { key, company } => {
                Box::new(YcSource::new(key.clone(), company.clone(), http)?)
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown source kind {0:?} (supported: {kinds})", kinds = SUPPORTED_KINDS.join(", "))]
pub struct UnknownSourceKind(pub String);

/// Source kinds this build can read.
pub const SUPPORTED_KINDS: &[&str] = &[ashby::KIND, greenhouse::KIND, lever::KIND, yc::KIND];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_covers_every_family() {
        let specs = SourcesConfig::default().specs().unwrap();
        let kinds: HashSet<&str> = specs.iter().map(|s| s.key().kind()).collect();
        assert_eq!(kinds, SUPPORTED_KINDS.iter().copied().collect());
    }

    #[test]
    fn configuring_one_family_replaces_all_defaults() {
        let config: SourcesConfig = toml::from_str(
            r#"
                [[lever]]
                site = "spotify"
                region = "eu"
            "#,
        )
        .unwrap();
        let specs = config.specs().unwrap();
        assert_eq!(specs.len(), 1);
        let SourceSpec::Lever { site, .. } = &specs[0] else {
            panic!("expected lever");
        };
        assert_eq!(site.region, LeverRegion::Eu);

        let empty: SourcesConfig = toml::from_str("").unwrap();
        assert_eq!(empty, SourcesConfig::default());
    }

    #[test]
    fn specs_from_keys() {
        for kind in SUPPORTED_KINDS {
            let key: SourceKey = format!("{kind}:acme").parse().unwrap();
            assert_eq!(SourceSpec::from_key(&key).unwrap().key(), &key);
        }
        let unknown: SourceKey = "workday:acme".parse().unwrap();
        let err = SourceSpec::from_key(&unknown).unwrap_err();
        assert_eq!(
            err.to_string(),
            "unknown source kind \"workday\" (supported: ashby, greenhouse, lever, yc)"
        );
    }

    #[test]
    fn config_rejects_invalid_and_duplicate_sources() {
        let config = SourcesConfig {
            ashby: vec![AshbyBoard {
                board: "has space".into(),
                company: None,
            }],
            ..SourcesConfig::default()
        };
        assert!(matches!(
            config.specs(),
            Err(SourcesConfigError::InvalidName(_))
        ));

        let twice = SourcesConfig {
            greenhouse: vec![
                GreenhouseBoard {
                    board: "Stripe".into(),
                    company: None,
                },
                GreenhouseBoard {
                    board: "stripe".into(),
                    company: Some("Stripe".into()),
                },
            ],
            ..SourcesConfig::default()
        };
        assert_eq!(
            twice.specs().unwrap_err().to_string(),
            "source greenhouse:stripe is configured more than once"
        );

        let bad_page = SourcesConfig {
            careers: vec![CareersPage {
                url: "ftp://example.com".into(),
                company: None,
            }],
            ..SourcesConfig::default()
        };
        assert!(matches!(
            bad_page.specs(),
            Err(SourcesConfigError::InvalidUrl(_))
        ));
    }

    #[test]
    fn unknown_fields_are_rejected() {
        assert!(toml::from_str::<SourcesConfig>("[[workday]]\nboard = \"x\"").is_err());
        assert!(toml::from_str::<SourcesConfig>("[[lever]]\nsite = \"x\"\nsiet = \"y\"").is_err());
    }
}
