//! Search queries for indexed ATS pages.
//!
//! Search engines index hosted job pages (`jobs.ashbyhq.com/<board>/<job>`),
//! so `site:jobs.ashbyhq.com "platform engineer" LATAM` lists companies
//! hiring platform engineers in Latin America, whether or not Narrow has
//! heard of them. The query dimensions are data ([`QueryMatrix`], the
//! built-in one is `queries.toml` next to this file): ATS hosts, role
//! phrases, seniority, places in groups, and optional specialty terms.
//!
//! The full cross product is hundreds of near-identical queries, so the
//! default plan is pairwise: every pair of values across host, role,
//! seniority and place group appears in at least one query
//! ([`QueryMatrix::plan`]). Yield is then measured per query and per
//! family (role or specialty × place group), and the families that
//! produce useful boards are the ones worth running again.
//!
//! Queries are provider-neutral: [`SearchQuery::text`] is the classic
//! `site:` form, [`SearchQuery::terms`] the same words without it, for
//! providers that take the site as a separate domain filter.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

/// The built-in matrix.
pub const DEFAULT_MATRIX: &str = include_str!("queries.toml");

/// Query dimensions.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct QueryMatrix {
    pub hosts: Vec<String>,
    pub roles: Vec<String>,
    /// `""` is "no seniority".
    #[serde(default)]
    pub seniority: Vec<String>,
    pub geo: Vec<GeoGroup>,
    #[serde(default)]
    pub specialties: Specialties,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GeoGroup {
    /// What yield is reported by: `global`, `brazil`, `latam`, …
    pub group: String,
    /// The words searched for (`LATAM`, `Latin America`).
    pub terms: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Specialties {
    #[serde(default)]
    pub terms: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MatrixError {
    #[error("invalid query matrix: {0}")]
    Parse(String),
    #[error("the query matrix needs at least one {0}")]
    Empty(&'static str),
}

/// How queries are chosen from the matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Plan {
    /// Every pair of values (host, role, seniority, place group) at least
    /// once.
    #[default]
    Pairwise,
    /// The whole cross product (host × role × seniority × place term).
    Full,
}

/// One query.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SearchQuery {
    /// Stable id of the query text.
    pub id: String,
    /// What yield is grouped by: role (or specialty) and place group,
    /// `platform engineer · latam`.
    pub family: String,
    /// The ATS host searched (`jobs.ashbyhq.com`).
    pub host: String,
    /// The quoted phrase: seniority and role, or a specialty.
    pub phrase: String,
    /// The place searched for (`LATAM`).
    pub place: String,
    pub role: Option<String>,
    pub seniority: Option<String>,
    pub specialty: Option<String>,
    pub geo_group: String,
}

impl SearchQuery {
    fn new(
        host: &str,
        role: Option<&str>,
        seniority: Option<&str>,
        specialty: Option<&str>,
        geo_group: &str,
        place: &str,
    ) -> Self {
        let seniority = seniority.filter(|s| !s.is_empty());
        let phrase = match (seniority, role, specialty) {
            (Some(s), Some(r), _) => format!("{s} {r}"),
            (None, Some(r), _) => r.to_owned(),
            (_, None, Some(sp)) => sp.to_owned(),
            _ => String::new(),
        };
        let family = format!("{} · {geo_group}", role.or(specialty).unwrap_or_default());
        let mut q = Self {
            id: String::new(),
            family,
            host: host.to_owned(),
            phrase,
            place: place.to_owned(),
            role: role.map(str::to_owned),
            seniority: seniority.map(str::to_owned),
            specialty: specialty.map(str::to_owned),
            geo_group: geo_group.to_owned(),
        };
        q.id = stable_id(&q.text());
        q
    }

    /// `site:jobs.ashbyhq.com "staff platform engineer" LATAM`; places of
    /// more than one word are quoted.
    pub fn text(&self) -> String {
        format!("site:{} {}", self.host, self.terms())
    }

    /// The query without `site:`, for providers that filter by domain
    /// separately.
    pub fn terms(&self) -> String {
        let place = if self.place.contains(' ') {
            format!("\"{}\"", self.place)
        } else {
            self.place.clone()
        };
        format!("\"{}\" {place}", self.phrase)
    }
}

/// A short stable id (FNV-1a, hex) for a string.
pub fn stable_id(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.bytes() {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

impl QueryMatrix {
    pub fn parse(text: &str) -> Result<Self, MatrixError> {
        let m: Self = toml::from_str(text).map_err(|e| MatrixError::Parse(e.to_string()))?;
        if m.hosts.is_empty() {
            return Err(MatrixError::Empty("host"));
        }
        if m.roles.is_empty() {
            return Err(MatrixError::Empty("role"));
        }
        if m.geo.iter().all(|g| g.terms.is_empty()) {
            return Err(MatrixError::Empty("place"));
        }
        Ok(m)
    }

    pub fn builtin() -> Self {
        // The built-in matrix is parsed by a test; an empty matrix would
        // only generate no queries.
        Self::parse(DEFAULT_MATRIX).unwrap_or(Self {
            hosts: Vec::new(),
            roles: Vec::new(),
            seniority: Vec::new(),
            geo: Vec::new(),
            specialties: Specialties::default(),
        })
    }

    fn seniorities(&self) -> Vec<&str> {
        if self.seniority.is_empty() {
            vec![""]
        } else {
            self.seniority.iter().map(String::as_str).collect()
        }
    }

    fn groups(&self) -> Vec<&GeoGroup> {
        self.geo.iter().filter(|g| !g.terms.is_empty()).collect()
    }

    /// The role queries of a plan. `rotate` shifts which place term of a
    /// group (and which host, for specialties) each query uses, so
    /// successive runs of a continuous sweep search different words
    /// without changing what is covered.
    pub fn plan(&self, plan: Plan, rotate: usize) -> Vec<SearchQuery> {
        let mut out = match plan {
            Plan::Full => self.full(),
            Plan::Pairwise => self.pairwise(rotate),
        };
        dedupe(&mut out);
        out
    }

    fn full(&self) -> Vec<SearchQuery> {
        let mut out = Vec::new();
        for host in &self.hosts {
            for role in &self.roles {
                for seniority in self.seniorities() {
                    for group in self.groups() {
                        for term in &group.terms {
                            out.push(SearchQuery::new(
                                host,
                                Some(role),
                                Some(seniority),
                                None,
                                &group.group,
                                term,
                            ));
                        }
                    }
                }
            }
        }
        out
    }

    /// Greedy pairwise covering over (host, role, seniority, place group):
    /// each new query starts from the first uncovered pair and fills the
    /// other dimensions with the values covering the most uncovered pairs,
    /// least-used first. Deterministic.
    fn pairwise(&self, rotate: usize) -> Vec<SearchQuery> {
        let seniorities = self.seniorities();
        let groups = self.groups();
        let sizes = [
            self.hosts.len(),
            self.roles.len(),
            seniorities.len(),
            groups.len(),
        ];
        if sizes.contains(&0) {
            return Vec::new();
        }
        let mut uncovered: HashSet<(usize, usize, usize, usize)> = HashSet::new();
        for a in 0..4 {
            for b in a + 1..4 {
                for va in 0..sizes[a] {
                    for vb in 0..sizes[b] {
                        uncovered.insert((a, va, b, vb));
                    }
                }
            }
        }
        let mut used = [
            vec![0usize; sizes[0]],
            vec![0usize; sizes[1]],
            vec![0usize; sizes[2]],
            vec![0usize; sizes[3]],
        ];
        let mut rows: Vec<[usize; 4]> = Vec::new();
        while let Some(&seed) = {
            let mut pending: Vec<_> = uncovered.iter().collect();
            pending.sort();
            pending.first().copied()
        } {
            let (a, va, b, vb) = seed;
            let mut row: [Option<usize>; 4] = [None; 4];
            row[a] = Some(va);
            row[b] = Some(vb);
            for dim in 0..4 {
                if row[dim].is_some() {
                    continue;
                }
                let best = (0..sizes[dim])
                    .max_by_key(|&v| {
                        let gain = (0..4)
                            .filter_map(|other| row[other].map(|ov| (other, ov)))
                            .filter(|&(other, ov)| {
                                let key = if other < dim {
                                    (other, ov, dim, v)
                                } else {
                                    (dim, v, other, ov)
                                };
                                uncovered.contains(&key)
                            })
                            .count();
                        (gain, std::cmp::Reverse(used[dim][v]), std::cmp::Reverse(v))
                    })
                    .unwrap_or(0);
                row[dim] = Some(best);
            }
            let row = row.map(|v| v.unwrap_or(0));
            for a in 0..4 {
                used[a][row[a]] += 1;
                for b in a + 1..4 {
                    uncovered.remove(&(a, row[a], b, row[b]));
                }
            }
            rows.push(row);
        }
        rows.iter()
            .enumerate()
            .map(|(i, [h, r, s, g])| {
                let group = groups[*g];
                let term = &group.terms[(i + rotate) % group.terms.len()];
                SearchQuery::new(
                    &self.hosts[*h],
                    Some(&self.roles[*r]),
                    Some(seniorities[*s]),
                    None,
                    &group.group,
                    term,
                )
            })
            .collect()
    }

    /// Specialty queries: each specialty with each place group, the host
    /// rotating.
    pub fn specialty_queries(&self, rotate: usize) -> Vec<SearchQuery> {
        let groups = self.groups();
        let mut out = Vec::new();
        let mut i = rotate;
        for specialty in &self.specialties.terms {
            for group in &groups {
                let host = &self.hosts[i % self.hosts.len()];
                let term = &group.terms[i % group.terms.len()];
                out.push(SearchQuery::new(
                    host,
                    None,
                    None,
                    Some(specialty),
                    &group.group,
                    term,
                ));
                i += 1;
            }
        }
        dedupe(&mut out);
        out
    }

    /// Recovers a query's dimensions from its text, for search results
    /// exported by hand (`site:jobs.lever.co "staff sre" "South America"`).
    /// Words not in the matrix are kept as they are.
    pub fn read(&self, text: &str) -> Option<SearchQuery> {
        let (host, rest) = text.trim().strip_prefix("site:")?.split_once(' ')?;
        let rest = rest.trim();
        let (phrase, place) = rest.strip_prefix('"')?.split_once('"')?;
        let phrase = phrase.trim();
        // Further quoted phrases are part of the place ("applied ai" remote).
        let place = place.replace('"', "");
        let place = place.split_whitespace().collect::<Vec<_>>().join(" ");
        let place = place.as_str();
        let lower = phrase.to_ascii_lowercase();
        let seniority = self
            .seniority
            .iter()
            .filter(|s| !s.is_empty())
            .find(|s| lower.starts_with(&format!("{} ", s.to_ascii_lowercase())));
        let bare = seniority.map_or(phrase, |s| phrase[s.len()..].trim());
        let role = self.roles.iter().find(|r| r.eq_ignore_ascii_case(bare));
        let specialty = role
            .is_none()
            .then(|| {
                self.specialties
                    .terms
                    .iter()
                    .find(|s| s.eq_ignore_ascii_case(bare))
            })
            .flatten();
        let group = self
            .geo
            .iter()
            .find(|g| g.terms.iter().any(|t| t.eq_ignore_ascii_case(place)))
            .map_or_else(|| place.to_ascii_lowercase(), |g| g.group.clone());
        let mut q = SearchQuery::new(
            host,
            specialty
                .is_none()
                .then_some(role.map_or(bare, String::as_str)),
            seniority.map(String::as_str),
            specialty.map(String::as_str),
            &group,
            place,
        );
        // Keep the text exactly as it was searched.
        q.id = stable_id(text.trim());
        Some(q)
    }
}

fn dedupe(queries: &mut Vec<SearchQuery>) {
    let mut seen = HashSet::new();
    queries.retain(|q| seen.insert(q.text()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_builtin_matrix_parses() {
        let m = QueryMatrix::builtin();
        assert_eq!(m.hosts.len(), 5);
        assert!(m.roles.contains(&"platform engineer".to_owned()));
        assert_eq!(m.geo.len(), 4);
        assert!(QueryMatrix::parse("hosts = []\nroles = [\"x\"]\ngeo = []").is_err());
    }

    #[test]
    fn pairwise_covers_every_pair_with_far_fewer_queries() {
        let m = QueryMatrix::builtin();
        let full = m.plan(Plan::Full, 0);
        let pairwise = m.plan(Plan::Pairwise, 0);
        assert_eq!(full.len(), 5 * 13 * 5 * 8);
        assert!(pairwise.len() <= 80, "{}", pairwise.len());
        // Every (host, role), (role, place group), (seniority, group) pair.
        for host in &m.hosts {
            for role in &m.roles {
                assert!(
                    pairwise
                        .iter()
                        .any(|q| &q.host == host && q.role.as_ref() == Some(role))
                );
            }
        }
        for role in &m.roles {
            for g in &m.geo {
                assert!(
                    pairwise
                        .iter()
                        .any(|q| q.role.as_ref() == Some(role) && q.geo_group == g.group),
                    "{role} × {}",
                    g.group
                );
            }
        }
        for s in &m.seniority {
            for g in &m.geo {
                let want = Some(s.as_str()).filter(|s| !s.is_empty());
                assert!(
                    pairwise
                        .iter()
                        .any(|q| q.seniority.as_deref() == want && q.geo_group == g.group)
                );
            }
        }
        // Deterministic, and rotation changes words, not coverage.
        assert_eq!(pairwise, m.plan(Plan::Pairwise, 0));
        let rotated = m.plan(Plan::Pairwise, 1);
        assert_eq!(rotated.len(), pairwise.len());
        assert_ne!(rotated, pairwise);
    }

    #[test]
    fn query_text_and_family() {
        let q = SearchQuery::new(
            "jobs.lever.co",
            Some("distributed systems engineer"),
            Some("staff"),
            None,
            "latam",
            "South America",
        );
        assert_eq!(
            q.text(),
            "site:jobs.lever.co \"staff distributed systems engineer\" \"South America\""
        );
        assert_eq!(
            q.terms(),
            "\"staff distributed systems engineer\" \"South America\""
        );
        assert_eq!(q.family, "distributed systems engineer · latam");
        let none = SearchQuery::new(
            "jobs.ashbyhq.com",
            Some("SRE"),
            Some(""),
            None,
            "global",
            "remote",
        );
        assert_eq!(none.text(), "site:jobs.ashbyhq.com \"SRE\" remote");
        assert_eq!(none.seniority, None);
    }

    #[test]
    fn exported_query_text_is_read_back_into_its_family() {
        let m = QueryMatrix::builtin();
        let q = m
            .read("site:jobs.ashbyhq.com \"staff platform engineer\" LATAM")
            .unwrap();
        assert_eq!(q.family, "platform engineer · latam");
        assert_eq!(q.seniority.as_deref(), Some("staff"));
        assert_eq!(q.host, "jobs.ashbyhq.com");
        let sp = m
            .read("site:boards.greenhouse.io \"payments\" \"Latin America\"")
            .unwrap();
        assert_eq!(sp.specialty.as_deref(), Some("payments"));
        assert_eq!(sp.family, "payments · latam");
        let other = m
            .read("site:jobs.lever.co \"rust engineer\" Chile")
            .unwrap();
        assert_eq!(other.family, "rust engineer · chile");
        assert!(m.read("platform engineer jobs").is_none());
    }

    #[test]
    fn specialty_queries_cover_each_specialty_and_place() {
        let m = QueryMatrix::builtin();
        let s = m.specialty_queries(0);
        assert_eq!(s.len(), 11 * 4);
        assert!(s.iter().all(|q| q.role.is_none() && q.specialty.is_some()));
        let hosts: HashSet<&str> = s.iter().map(|q| q.host.as_str()).collect();
        assert_eq!(hosts.len(), 5);
    }
}
