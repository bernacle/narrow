//! schema.org `JobPosting` structured data.
//!
//! Many careers pages and job pages carry `<script type="application/ld+json">`
//! with a `JobPosting` (search engines ask for it). [`job_postings`] reads
//! every one on a page, whether the block is a single object, an array or
//! a `@graph`. A posting found this way is discovery input, not a source:
//! its URLs and its `hiringOrganization` lead back to the company and,
//! whenever possible, to the ATS board behind the page
//! ([`JsonLdPosting::ats_urls`]).

use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;
use serde_json::Value;
use url::Url;

use super::ats::{self, UrlTarget};

/// One `JobPosting`, with the fields discovery uses.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct JsonLdPosting {
    pub title: Option<String>,
    pub hiring_organization: Option<String>,
    /// The organization's website (`hiringOrganization.sameAs` or `.url`).
    pub organization_url: Option<String>,
    pub date_posted: Option<DateTime<Utc>>,
    pub valid_through: Option<DateTime<Utc>>,
    pub employment_type: Vec<String>,
    /// `addressLocality, addressRegion, addressCountry` of each location.
    pub job_location: Vec<String>,
    /// Countries or regions a remote applicant may be in.
    pub applicant_location_requirements: Vec<String>,
    /// `TELECOMMUTE` for remote postings.
    pub job_location_type: Option<String>,
    /// Plain text, shortened.
    pub description: Option<String>,
    pub direct_apply: Option<bool>,
    /// The posting's own URL, or the page's canonical one.
    pub url: Option<String>,
    pub identifier: Option<String>,
}

impl JsonLdPosting {
    /// Remote, by `jobLocationType`.
    pub fn remote(&self) -> bool {
        self.job_location_type
            .as_deref()
            .is_some_and(|t| t.eq_ignore_ascii_case("telecommute"))
    }

    /// Expired by `validThrough`.
    pub fn expired(&self, now: DateTime<Utc>) -> bool {
        self.valid_through.is_some_and(|v| v < now)
    }

    /// URLs in the posting that point at an ATS (supported or not).
    pub fn ats_urls(&self) -> Vec<Url> {
        let mut text = String::new();
        for u in [&self.url, &self.organization_url].into_iter().flatten() {
            text.push_str(u);
            text.push(' ');
        }
        if let Some(d) = &self.description {
            text.push_str(d);
        }
        crate::careers::absolute_urls(&text)
            .into_iter()
            .map(|(_, u)| u)
            .filter(|u| !matches!(ats::classify(u), UrlTarget::Page))
            .collect()
    }
}

/// Every `JobPosting` in a page's JSON-LD blocks, in page order. Blocks
/// that don't parse are skipped.
pub fn job_postings(html: &str) -> Vec<JsonLdPosting> {
    let lower = html.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(pos) = lower[from..].find("<script") {
        let start = from + pos;
        let Some(open_end) = lower[start..].find('>').map(|e| start + e + 1) else {
            break;
        };
        from = open_end;
        let tag = &lower[start..open_end];
        if !tag.contains("application/ld+json") {
            continue;
        }
        let Some(close) = lower[open_end..].find("</script").map(|e| open_end + e) else {
            break;
        };
        from = close;
        let body = html[open_end..close].trim();
        let Ok(value) = serde_json::from_str::<Value>(body) else {
            continue;
        };
        collect(&value, &mut out, 0);
    }
    out
}

fn collect(value: &Value, out: &mut Vec<JsonLdPosting>, depth: usize) {
    if depth > 4 {
        return;
    }
    match value {
        Value::Array(items) => items.iter().for_each(|v| collect(v, out, depth + 1)),
        Value::Object(map) => {
            if is_type(map.get("@type"), "JobPosting") {
                out.push(posting(value));
            } else if let Some(graph) = map.get("@graph") {
                collect(graph, out, depth + 1);
            }
        }
        _ => {}
    }
}

fn is_type(t: Option<&Value>, name: &str) -> bool {
    match t {
        Some(Value::String(s)) => s == name || s.ends_with(&format!("/{name}")),
        Some(Value::Array(a)) => a.iter().any(|v| is_type(Some(v), name)),
        _ => false,
    }
}

fn text(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::String(s) => {
            let s = jobhunt_core::html::decode_entities(s.trim())
                .trim()
                .to_owned();
            (!s.is_empty()).then_some(s)
        }
        Value::Number(n) => Some(n.to_string()),
        Value::Object(m) => text(
            m.get("name")
                .or_else(|| m.get("@value"))
                .or_else(|| m.get("value")),
        ),
        Value::Array(a) => a.iter().find_map(|v| text(Some(v))),
        _ => None,
    }
}

fn list(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Array(a)) => a.iter().filter_map(|v| text(Some(v))).collect(),
        Some(other) => text(Some(other)).into_iter().collect(),
        None => Vec::new(),
    }
}

fn date(v: Option<&Value>) -> Option<DateTime<Utc>> {
    let s = text(v)?;
    DateTime::parse_from_rfc3339(&s)
        .map(|d| d.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            NaiveDate::parse_from_str(s.get(..10)?, "%Y-%m-%d")
                .ok()?
                .and_hms_opt(0, 0, 0)
                .map(|d| d.and_utc())
        })
}

fn place(v: &Value) -> Option<String> {
    let address = v.get("address").unwrap_or(v);
    if let Value::String(s) = address {
        return Some(s.trim().to_owned()).filter(|s| !s.is_empty());
    }
    let parts: Vec<String> = ["addressLocality", "addressRegion", "addressCountry"]
        .iter()
        .filter_map(|k| text(address.get(*k)))
        .collect();
    if parts.is_empty() {
        text(v.get("name"))
    } else {
        Some(parts.join(", "))
    }
}

fn places(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Array(a)) => a.iter().filter_map(place).collect(),
        Some(other) => place(other).into_iter().collect(),
        None => Vec::new(),
    }
}

fn posting(v: &Value) -> JsonLdPosting {
    let org = v.get("hiringOrganization");
    let description = text(v.get("description")).map(|d| {
        let plain = jobhunt_core::html::html_to_text(&d).unwrap_or(d);
        plain.chars().take(4000).collect::<String>()
    });
    JsonLdPosting {
        title: text(v.get("title")),
        hiring_organization: org.and_then(|o| text(Some(o))),
        organization_url: org.and_then(|o| text(o.get("sameAs")).or_else(|| text(o.get("url")))),
        date_posted: date(v.get("datePosted")),
        valid_through: date(v.get("validThrough")),
        employment_type: list(v.get("employmentType")),
        job_location: places(v.get("jobLocation")),
        applicant_location_requirements: match v.get("applicantLocationRequirements") {
            Some(Value::Array(a)) => a
                .iter()
                .filter_map(|x| text(x.get("name")).or_else(|| text(Some(x))))
                .collect(),
            Some(x) => text(x.get("name"))
                .or_else(|| text(Some(x)))
                .into_iter()
                .collect(),
            None => Vec::new(),
        },
        job_location_type: text(v.get("jobLocationType")),
        description,
        direct_apply: v.get("directApply").and_then(|d| match d {
            Value::Bool(b) => Some(*b),
            Value::String(s) => s.parse().ok(),
            _ => None,
        }),
        url: text(v.get("url"))
            .or_else(|| text(v.get("mainEntityOfPage")).filter(|s| s.starts_with("http"))),
        identifier: v
            .get("identifier")
            .and_then(|i| text(i.get("value")).or_else(|| text(Some(i)))),
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    const PAGE: &str = r#"
        <html><head>
        <script type="application/ld+json">{"@context":"https://schema.org","@type":"Organization","name":"Acme"}</script>
        <script type='application/ld+json'>
        {
          "@context": "https://schema.org/",
          "@type": "JobPosting",
          "title": "Senior Platform Engineer",
          "datePosted": "2026-09-30",
          "validThrough": "2026-11-30T00:00:00Z",
          "employmentType": ["FULL_TIME"],
          "hiringOrganization": {"@type": "Organization", "name": "Acme", "sameAs": "https://acme.com"},
          "jobLocationType": "TELECOMMUTE",
          "applicantLocationRequirements": [{"@type": "Country", "name": "Brazil"}, {"@type": "Country", "name": "Mexico"}],
          "jobLocation": {"@type": "Place", "address": {"addressLocality": "São Paulo", "addressCountry": "BR"}},
          "description": "&lt;p&gt;Build our platform. Apply at https://jobs.ashbyhq.com/acme/11111111-1111-1111-1111-111111111111&lt;/p&gt;",
          "directApply": true,
          "identifier": {"@type": "PropertyValue", "name": "Acme", "value": "PE-1"},
          "url": "https://acme.com/careers/senior-platform-engineer"
        }
        </script>
        <script type="application/ld+json">{"@graph":[{"@type":"WebPage"},{"@type":["JobPosting"],"title":"SRE","hiringOrganization":"Acme"}]}</script>
        <script type="application/ld+json">{not json</script>
        </head></html>
    "#;

    #[test]
    fn reads_job_postings_from_json_ld() {
        let found = job_postings(PAGE);
        assert_eq!(found.len(), 2);
        let p = &found[0];
        assert_eq!(p.title.as_deref(), Some("Senior Platform Engineer"));
        assert_eq!(p.hiring_organization.as_deref(), Some("Acme"));
        assert_eq!(p.organization_url.as_deref(), Some("https://acme.com"));
        assert_eq!(
            p.date_posted,
            Some(Utc.with_ymd_and_hms(2026, 9, 30, 0, 0, 0).unwrap())
        );
        assert!(p.valid_through.is_some());
        assert_eq!(p.employment_type, ["FULL_TIME"]);
        assert_eq!(p.job_location, ["São Paulo, BR"]);
        assert_eq!(p.applicant_location_requirements, ["Brazil", "Mexico"]);
        assert!(p.remote());
        assert_eq!(p.direct_apply, Some(true));
        assert_eq!(p.identifier.as_deref(), Some("PE-1"));
        assert!(
            p.description
                .as_deref()
                .unwrap()
                .contains("Build our platform")
        );
        assert!(!p.expired(Utc.with_ymd_and_hms(2026, 10, 5, 0, 0, 0).unwrap()));
        assert!(p.expired(Utc.with_ymd_and_hms(2026, 12, 1, 0, 0, 0).unwrap()));
        // The ATS link in the description leads back to the board.
        let ats: Vec<String> = p.ats_urls().iter().map(Url::to_string).collect();
        assert_eq!(
            ats,
            ["https://jobs.ashbyhq.com/acme/11111111-1111-1111-1111-111111111111"]
        );
        // @graph, array @type and a plain-string organization.
        assert_eq!(found[1].title.as_deref(), Some("SRE"));
        assert_eq!(found[1].hiring_organization.as_deref(), Some("Acme"));
        assert!(job_postings("<p>no data</p>").is_empty());
    }
}
