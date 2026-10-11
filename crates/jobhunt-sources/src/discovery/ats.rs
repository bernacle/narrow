//! What a discovered URL points at, from the URL alone.
//!
//! A search hit, a link in a "Who is hiring" post or a crawled URL is only
//! a lead. [`classify`] turns it into the most specific thing it names:
//!
//! * a job on a supported board (`jobs.ashbyhq.com/<board>/<uuid>` →
//!   `ashby`, board, job id), from [`careers::board_for_url`] and
//!   [`jobhunt_jobs::ats_job_ref`];
//! * a supported board without a job (`jobs.lever.co/<site>`);
//! * a known ATS without an adapter, with its tenant when the URL shows one
//!   (`acme.wd5.myworkdayjobs.com/External` → `workday`, `acme`), so the
//!   unsupported-ATS map counts companies, not URLs;
//! * anything else: a company page, judged later by reading it.
//!
//! Nothing here touches the network, and a URL's shape never validates a
//! board: the board is read through its adapter before anything is
//! believed (see [`super::resolve`]).

use jobhunt_core::CanonicalUrl;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::careers::{self, BoardRef};
use crate::lever::LeverRegion;
use crate::{SourceSpec, ashby, greenhouse, lever, yc};

/// What a URL names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrlTarget {
    /// A posting on a supported board.
    Job {
        board: BoardRef,
        /// The provider's own job id (a UUID for Ashby and Lever, a number
        /// for Greenhouse).
        job_id: String,
    },
    /// A supported board, no particular posting.
    Board(BoardRef),
    /// A known ATS without an adapter.
    Unsupported {
        provider: &'static str,
        /// The company's account on the ATS, when the URL shows it.
        tenant: Option<String>,
    },
    /// Not an ATS URL: a company site, a careers page, an aggregator.
    Page,
}

/// Recognizes what `url` points at. See the module docs.
pub fn classify(url: &Url) -> UrlTarget {
    if let Some(board) = careers::board_for_url(url) {
        // YC company pages are boards; their job ids are not in the URL.
        if board.kind == yc::KIND {
            return UrlTarget::Board(board);
        }
        let job = CanonicalUrl::parse(url.as_str())
            .ok()
            .and_then(|c| jobhunt_jobs::ats_job_ref(&c))
            .filter(|r| r.system == board.kind);
        return match job {
            Some(job) => UrlTarget::Job {
                board,
                job_id: job.id,
            },
            None => UrlTarget::Board(board),
        };
    }
    if let Some(provider) = careers::other_ats_for_url(url) {
        return UrlTarget::Unsupported {
            provider,
            tenant: tenant(provider, url),
        };
    }
    UrlTarget::Page
}

/// Hosts that serve every tenant on one name, whose first path segment is
/// the tenant (`jobs.smartrecruiters.com/<Company>/…`).
const PATH_TENANT_HOSTS: &[(&str, &[&str])] = &[
    (
        "smartrecruiters",
        &["jobs.smartrecruiters.com", "careers.smartrecruiters.com"],
    ),
    ("workable", &["apply.workable.com", "jobs.workable.com"]),
    ("gem", &["jobs.gem.com"]),
    ("rippling", &["ats.rippling.com"]),
    ("jobvite", &["jobs.jobvite.com"]),
    ("polymer", &["jobs.polymer.co"]),
];

/// Words that are a host's own routes, never a tenant.
const NOT_TENANTS: &[&str] = &[
    "www",
    "jobs",
    "careers",
    "apply",
    "app",
    "api",
    "en",
    "en-us",
    "j",
    "o",
    "p",
    "search",
    "companies",
    "company",
    "embed",
];

/// The company's account on an unsupported ATS, from the URL.
fn tenant(provider: &'static str, url: &Url) -> Option<String> {
    let host = url.host_str()?.to_ascii_lowercase();
    let segments: Vec<&str> = url
        .path_segments()
        .map(|s| s.filter(|seg| !seg.is_empty()).collect())
        .unwrap_or_default();
    let clean = |s: &str| {
        let s = s.to_ascii_lowercase();
        let ok = !s.is_empty()
            && s.len() <= 80
            && !NOT_TENANTS.contains(&s.as_str())
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'));
        ok.then_some(s)
    };
    if let Some((_, hosts)) = PATH_TENANT_HOSTS.iter().find(|(p, _)| *p == provider)
        && hosts.contains(&host.as_str())
    {
        return segments.first().and_then(|s| clean(s));
    }
    match provider {
        // app.dover.com/apply/<tenant>/<id>, app.dover.com/jobs/<tenant>
        "dover" => match segments.as_slice() {
            ["apply" | "jobs", tenant, ..] => clean(tenant),
            _ => None,
        },
        // wellfound.com/company/<tenant>/jobs
        "wellfound" => match segments.as_slice() {
            ["company", tenant, ..] => clean(tenant),
            _ => None,
        },
        // join.com/companies/<tenant>
        "join" => match segments.as_slice() {
            ["companies", tenant, ..] => clean(tenant),
            _ => None,
        },
        // www.comeet.com/jobs/<tenant>/<uid>
        "comeet" => match segments.as_slice() {
            ["jobs", tenant, ..] => clean(tenant),
            _ => None,
        },
        // <tenant>.wd5.myworkdayjobs.com, careers-<tenant>.icims.com,
        // <tenant>.jobs.personio.de, <tenant>.recruitee.com, …: the first
        // label of the host.
        _ => {
            let first = host.split('.').next()?;
            let first = first.strip_prefix("careers-").unwrap_or(first);
            clean(first)
        }
    }
}

/// A supported board's canonical public URL, the same for every spelling
/// (`boards.greenhouse.io/x` and `job-boards.greenhouse.io/x` are one
/// board).
pub fn board_url(board: &BoardRef) -> String {
    match board.kind {
        ashby::KIND => format!("https://jobs.ashbyhq.com/{}", board.name),
        greenhouse::KIND => format!("https://job-boards.greenhouse.io/{}", board.name),
        lever::KIND => match board.lever_region {
            LeverRegion::Eu => format!("https://jobs.eu.lever.co/{}", board.name),
            LeverRegion::Global => format!("https://jobs.lever.co/{}", board.name),
        },
        _ => format!("https://www.ycombinator.com/companies/{}/jobs", board.name),
    }
}

/// A posting's canonical public URL on its board.
pub fn job_url(board: &BoardRef, job_id: &str) -> String {
    match board.kind {
        greenhouse::KIND => format!("{}/jobs/{job_id}", board_url(board)),
        _ => format!("{}/{job_id}", board_url(board)),
    }
}

/// The source key of a board (`ashby:railway`), as the registry and the
/// configuration spell it.
pub fn source_key(board: &BoardRef) -> Option<String> {
    SourceSpec::from_board(board, None)
        .ok()
        .map(|s| s.key().to_string())
}

/// A company's domain from a URL or host: lowercase, without `www.`, and
/// reduced to the registrable part (`careers.acme.com` → `acme.com`,
/// `jobs.acme.com.br` → `acme.com.br`). A heuristic, not the public suffix
/// list: two labels, or three when the second-to-last is a generic
/// second-level label under a country code (`co.uk`, `com.br`).
pub fn registrable_domain(host: &str) -> Option<String> {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    let labels: Vec<&str> = host.split('.').filter(|l| !l.is_empty()).collect();
    let valid = labels.len() >= 2
        && labels
            .iter()
            .all(|l| l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
        && labels
            .last()
            .is_some_and(|tld| tld.len() >= 2 && tld.bytes().all(|b| b.is_ascii_alphabetic()));
    if !valid {
        return None;
    }
    let n = labels.len();
    let generic_second = ["co", "com", "org", "net", "ac", "gov", "edu", "ltd", "plc"];
    let keep = if n >= 3 && labels[n - 1].len() == 2 && generic_second.contains(&labels[n - 2]) {
        3
    } else {
        2
    };
    Some(labels[n.saturating_sub(keep)..].join("."))
}

/// Hosts that never name a company: ATS infrastructure, CDNs, social
/// networks, search engines, job aggregators, standards bodies.
const NOT_COMPANY_DOMAINS: &[&str] = &[
    "ashbyhq.com",
    "greenhouse.io",
    "greenhouse.com",
    "lever.co",
    "ycombinator.com",
    "workatastartup.com",
    "w3.org",
    "schema.org",
    "google.com",
    "googleapis.com",
    "gstatic.com",
    "googletagmanager.com",
    "google-analytics.com",
    "recaptcha.net",
    "cloudflare.com",
    "cloudfront.net",
    "amazonaws.com",
    "jsdelivr.net",
    "unpkg.com",
    "facebook.com",
    "fb.com",
    "twitter.com",
    "x.com",
    "t.co",
    "linkedin.com",
    "lnkd.in",
    "instagram.com",
    "youtube.com",
    "youtu.be",
    "tiktok.com",
    "glassdoor.com",
    "indeed.com",
    "github.com",
    "githubusercontent.com",
    "github.io",
    "medium.com",
    "wikipedia.org",
    "apple.com",
    "microsoft.com",
    "bit.ly",
    "notion.site",
    "typeform.com",
    "calendly.com",
    "hsforms.com",
    "hubspot.com",
    "sentry.io",
    "segment.com",
    "segment.io",
    "intercom.io",
    "mailchimp.com",
    "vimeo.com",
    "wistia.com",
    "news.ycombinator.com",
    "himalayas.app",
    "remoteok.com",
    "weworkremotely.com",
    "wellfound.com",
    "angel.co",
    "builtin.com",
    "otta.com",
    "welcometothejungle.com",
    "crunchbase.com",
    "discord.gg",
    "discord.com",
    "slack.com",
    "zoom.us",
    "gmail.com",
    "example.com",
    "loom.com",
    "figma.com",
    "docs.google.com",
    "forms.gle",
    "goo.gl",
    "imgur.com",
    // Archives and shorteners show other sites' pages under their own name.
    "archive.ph",
    "archive.today",
    "archive.is",
    "archive.org",
    "web.archive.org",
    "tinyurl.com",
    "rebrand.ly",
    "buff.ly",
    "ow.ly",
    "lnk.to",
    "linktr.ee",
];

/// Whether a domain can be a company's: not infrastructure, a social
/// network, an aggregator, or a known ATS.
pub fn is_company_domain(domain: &str) -> bool {
    let domain = domain.to_ascii_lowercase();
    !NOT_COMPANY_DOMAINS
        .iter()
        .any(|d| domain == *d || domain.ends_with(&format!(".{d}")))
        && Url::parse(&format!("https://{domain}/"))
            .ok()
            .is_some_and(|u| careers::other_ats_for_url(&u).is_none())
}

/// A company domain named by a URL, if the URL can be a company's.
pub fn company_domain_of(url: &Url) -> Option<String> {
    registrable_domain(url.host_str()?).filter(|d| is_company_domain(d))
}

/// A serializable view of a board reference.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct BoardId {
    pub provider: String,
    pub board: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub eu: bool,
}

impl BoardId {
    pub fn of(board: &BoardRef) -> Self {
        Self {
            provider: board.kind.to_owned(),
            board: board.name.clone(),
            eu: board.lever_region == LeverRegion::Eu,
        }
    }

    /// The board reference, when the provider is supported.
    pub fn to_ref(&self) -> Option<BoardRef> {
        let kind = crate::SUPPORTED_KINDS
            .iter()
            .copied()
            .find(|k| *k == self.provider)?;
        Some(BoardRef {
            kind,
            name: self.board.clone(),
            lever_region: if self.eu {
                LeverRegion::Eu
            } else {
                LeverRegion::Global
            },
        })
    }

    /// `ashby:railway`.
    pub fn key(&self) -> String {
        format!("{}:{}", self.provider, self.board)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(url: &str) -> UrlTarget {
        classify(&Url::parse(url).unwrap())
    }

    fn job(kind: &str, board: &str, id: &str) -> (String, String, String) {
        (kind.to_owned(), board.to_owned(), id.to_owned())
    }

    fn as_job(t: UrlTarget) -> Option<(String, String, String)> {
        match t {
            UrlTarget::Job { board, job_id } => Some((board.kind.to_owned(), board.name, job_id)),
            _ => None,
        }
    }

    #[test]
    fn ashby_job_urls_name_their_board_and_job() {
        assert_eq!(
            as_job(target(
                "https://jobs.ashbyhq.com/Moxie/36c5bcce-5a3d-4394-9eeb-68d812f9e753?utm_source=x"
            )),
            Some(job(
                "ashby",
                "moxie",
                "36c5bcce-5a3d-4394-9eeb-68d812f9e753"
            ))
        );
        assert_eq!(
            as_job(target(
                "https://jobs.ashbyhq.com/readyon.ai/244a03f4-ce9b-4d42-926c-bf6be001b1c6/application"
            )),
            Some(job(
                "ashby",
                "readyon.ai",
                "244a03f4-ce9b-4d42-926c-bf6be001b1c6"
            ))
        );
        // A board page is a board, not a job.
        let UrlTarget::Board(b) = target("https://jobs.ashbyhq.com/moxie") else {
            panic!("expected a board");
        };
        assert_eq!((b.kind, b.name.as_str()), ("ashby", "moxie"));
    }

    #[test]
    fn greenhouse_job_urls_on_both_hosts_are_the_same_job() {
        let a = as_job(target(
            "https://boards.greenhouse.io/wikimedia/jobs/7012345?gh_src=abc",
        ));
        let b = as_job(target(
            "https://job-boards.greenhouse.io/wikimedia/jobs/7012345",
        ));
        assert_eq!(a, Some(job("greenhouse", "wikimedia", "7012345")));
        assert_eq!(a, b);
        let UrlTarget::Job { board, job_id } =
            target("https://job-boards.eu.greenhouse.io/acme/jobs/4400001")
        else {
            panic!("expected a job");
        };
        assert_eq!(
            job_url(&board, &job_id),
            "https://job-boards.greenhouse.io/acme/jobs/4400001"
        );
    }

    #[test]
    fn lever_job_urls_keep_the_region() {
        assert_eq!(
            as_job(target(
                "https://jobs.lever.co/pipedrive/bfaffe50-ecc7-42c6-8c88-996d920d60df/apply"
            )),
            Some(job(
                "lever",
                "pipedrive",
                "bfaffe50-ecc7-42c6-8c88-996d920d60df"
            ))
        );
        let UrlTarget::Job { board, .. } =
            target("https://jobs.eu.lever.co/acme/bfaffe50-ecc7-42c6-8c88-996d920d60df")
        else {
            panic!("expected a job");
        };
        assert_eq!(board.lever_region, LeverRegion::Eu);
        assert_eq!(board_url(&board), "https://jobs.eu.lever.co/acme");
        assert_eq!(BoardId::of(&board).to_ref(), Some(board));
    }

    #[test]
    fn unsupported_ats_are_named_with_their_tenant() {
        let cases = [
            (
                "https://acme.wd5.myworkdayjobs.com/en-US/External/job/X_123",
                "workday",
                Some("acme"),
            ),
            (
                "https://jobs.smartrecruiters.com/Acme1/744000012345-engineer",
                "smartrecruiters",
                Some("acme1"),
            ),
            (
                "https://apply.workable.com/huggingface/j/ABC123/",
                "workable",
                Some("huggingface"),
            ),
            (
                "https://acme.recruitee.com/o/backend-engineer",
                "recruitee",
                Some("acme"),
            ),
            (
                "https://acme.jobs.personio.de/job/123",
                "personio",
                Some("acme"),
            ),
            (
                "https://careers-acme.icims.com/jobs/1/job",
                "icims",
                Some("acme"),
            ),
            ("https://jobs.gem.com/retool/am9i", "gem", Some("retool")),
            (
                "https://ats.rippling.com/acme/jobs/1",
                "rippling",
                Some("acme"),
            ),
            ("https://app.dover.com/apply/acme/1", "dover", Some("acme")),
            (
                "https://wellfound.com/company/acme/jobs",
                "wellfound",
                Some("acme"),
            ),
            ("https://apply.workable.com/", "workable", None),
        ];
        for (url, provider, tenant) in cases {
            assert_eq!(
                target(url),
                UrlTarget::Unsupported {
                    provider,
                    tenant: tenant.map(str::to_owned)
                },
                "{url}"
            );
        }
        assert_eq!(target("https://acme.com/careers"), UrlTarget::Page);
        assert_eq!(target("https://myjobs.lever.co/acme/x"), UrlTarget::Page);
    }

    #[test]
    fn company_domains() {
        assert_eq!(
            registrable_domain("careers.Acme.com").as_deref(),
            Some("acme.com")
        );
        assert_eq!(
            registrable_domain("jobs.acme.com.br").as_deref(),
            Some("acme.com.br")
        );
        assert_eq!(
            registrable_domain("www.acme.co.uk").as_deref(),
            Some("acme.co.uk")
        );
        assert_eq!(registrable_domain("acme.io").as_deref(), Some("acme.io"));
        assert_eq!(registrable_domain("localhost"), None);
        assert_eq!(registrable_domain("10.0.0.1"), None);
        let domain = |u: &str| company_domain_of(&Url::parse(u).unwrap());
        assert_eq!(
            domain("https://www.joinmoxie.com/about").as_deref(),
            Some("joinmoxie.com")
        );
        assert_eq!(domain("https://www.linkedin.com/company/acme"), None);
        assert_eq!(domain("https://cdn.jsdelivr.net/x.js"), None);
        assert_eq!(domain("https://acme.bamboohr.com/careers"), None);
        assert_eq!(domain("https://jobs.ashbyhq.com/acme"), None);
        assert_eq!(domain("https://archive.ph/company/careers"), None);
    }
}
