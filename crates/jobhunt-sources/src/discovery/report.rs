//! What each discovery strategy is worth, from the candidate store.
//!
//! Every number is a count over candidates that a strategy's sightings
//! touch; there is no combined score. A board counts toward every strategy
//! that saw it (directly, through one of its jobs, or through a company's
//! careers page), and `unique_boards` says how many no other strategy saw.
//!
//! * **raw hits**: sightings of the strategy's own (not derived);
//! * **duplicate rate**: 1 − unique candidates / raw hits, within the
//!   strategy;
//! * **stale rate**: discovered jobs no longer on their board's current
//!   listing (or whose board is gone), of those checked;
//! * **first-party rate**: boards whose ownership is verified by the
//!   company's own site or corroborated, of boards that could be read;
//! * **useful**: engineering postings open to (or unclear for) the
//!   reference profile, a Brazil-based remote candidate, or with a global,
//!   Brazil or Americas remote scope.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde::Serialize;

use super::{Candidate, CandidateKind, CandidateStatus, DiscoveryStore, Ownership, Sighting};
use crate::registry::{GeoBucket, PersonYield, SourceRegistry};

/// One strategy's (or query family's, query's, host's) yield.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Row {
    pub key: String,
    pub raw_hits: usize,
    pub unique_urls: usize,
    pub jobs: usize,
    pub live_jobs: usize,
    pub closed_jobs: usize,
    /// Live and posted under 30 days ago (by the source's date).
    pub fresh_jobs: usize,
    pub engineering_jobs: usize,
    /// Live engineering jobs open to Brazil or with a global / Brazil /
    /// Americas remote scope.
    pub useful_jobs: usize,
    pub boards: usize,
    /// Boards no other strategy found.
    pub unique_boards: usize,
    pub boards_read: usize,
    pub validated_boards: usize,
    pub inconclusive_boards: usize,
    pub rejected_boards: usize,
    /// Validated boards with an engineering posting open to (or unclear
    /// for) a Brazil-based remote candidate.
    pub useful_boards: usize,
    /// Validated boards meeting the activation bar.
    pub ready_boards: usize,
    pub companies: usize,
    pub new_companies: usize,
    pub unsupported_ats: usize,
    /// Open postings on the validated boards.
    pub board_postings: usize,
    pub board_engineering: usize,
    pub board_engineering_brazil: usize,
    /// The person's ranking on the validated boards, when one was given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub person: Option<PersonYield>,
    pub duplicate_rate: Option<f64>,
    pub stale_rate: Option<f64>,
    pub first_party_rate: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ProviderRow {
    pub provider: String,
    pub boards: usize,
    pub boards_read: usize,
    pub validated: usize,
    pub useful: usize,
    pub ready: usize,
    pub postings: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct UnsupportedRow {
    pub provider: String,
    /// Distinct companies (tenants or domains).
    pub companies: usize,
    /// URLs pointing at it (a rough volume signal: no adapter reads them).
    pub urls: usize,
    /// Companies whose discovery context names remote work in Brazil,
    /// Latin America, the Americas or anywhere.
    pub brazil_latam_global: usize,
    /// Companies that also have a supported board (no adapter needed to
    /// reach them).
    pub also_supported: usize,
    pub examples: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BoardLine {
    pub source: String,
    pub company: Option<String>,
    pub domain: Option<String>,
    pub ownership: Ownership,
    pub status: CandidateStatus,
    pub new_to_narrow: bool,
    pub open: usize,
    pub engineering: usize,
    pub engineering_brazil_open: usize,
    pub engineering_brazil_unclear: usize,
    pub main_geo: Option<GeoBucket>,
    pub stale_share: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub person: Option<PersonYield>,
    pub strategies: Vec<String>,
    pub why: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Report {
    pub now: Option<DateTime<Utc>>,
    pub candidates: BTreeMap<String, BTreeMap<String, usize>>,
    pub total: Row,
    pub strategies: Vec<Row>,
    pub families: Vec<Row>,
    pub queries: Vec<Row>,
    pub hosts: Vec<Row>,
    pub geo: Vec<Row>,
    pub providers: Vec<ProviderRow>,
    pub unsupported: Vec<UnsupportedRow>,
    pub ownership: BTreeMap<String, usize>,
    /// Recommended to activate: validated, ready, ranked by the person's
    /// strong and plausible, then engineering open to Brazil.
    pub activate: Vec<BoardLine>,
    /// Validated, below the activation bar.
    pub hold: usize,
    /// Useful but ownership not established: a person (or a later run)
    /// should look again.
    pub recheck: Vec<BoardLine>,
    pub rejected: usize,
}

/// Facts about candidates computed once.
struct Facts<'a> {
    store: &'a DiscoveryStore,
    known_sources: HashSet<String>,
    known_domains: HashSet<String>,
    person: &'a HashMap<String, PersonYield>,
    /// Board key → strategies that touched it.
    board_strategies: HashMap<String, BTreeSet<String>>,
}

fn rate(part: usize, whole: usize) -> Option<f64> {
    (whole > 0).then(|| part as f64 / whole as f64)
}

fn useful_board(c: &Candidate) -> bool {
    c.status == CandidateStatus::Validated
        && c.board_result
            .as_ref()
            .and_then(|b| b.yields.as_ref())
            .is_some_and(|y| y.engineering_brazil_open + y.engineering_brazil_unclear > 0)
}

fn ready_board(c: &Candidate) -> bool {
    c.status == CandidateStatus::Validated
        && c.board_result
            .as_ref()
            .is_some_and(|b| b.activation.is_empty())
}

fn useful_job(c: &Candidate) -> bool {
    c.status == CandidateStatus::Live
        && c.job_result.as_ref().is_some_and(|j| {
            j.engineering
                && (matches!(j.brazil.as_deref(), Some("open" | "unclear"))
                    && matches!(
                        j.geo,
                        Some(GeoBucket::Global | GeoBucket::Brazil | GeoBucket::Americas)
                    )
                    || j.brazil.as_deref() == Some("open"))
        })
}

impl<'a> Facts<'a> {
    fn company_of(&self, c: &Candidate) -> String {
        c.board_result
            .as_ref()
            .and_then(|b| b.domain.clone())
            .or_else(|| c.domain_hint.clone())
            .or_else(|| c.board.as_ref().map(|b| b.key()))
            .unwrap_or_else(|| c.key.clone())
    }

    fn new_to_narrow(&self, c: &Candidate) -> bool {
        let known_source = c
            .source
            .as_ref()
            .is_some_and(|s| self.known_sources.contains(s));
        let domain = c
            .board_result
            .as_ref()
            .and_then(|b| b.domain.as_ref())
            .or(c.domain_hint.as_ref());
        let known_domain = domain.is_some_and(|d| self.known_domains.contains(d));
        !(known_source || known_domain)
    }

    /// The row for every candidate `touched` says the group covers.
    fn row(&self, key: String, touched: impl Fn(&Sighting) -> bool) -> Row {
        let mut row = Row {
            key,
            ..Row::default()
        };
        let mut urls = HashSet::new();
        let mut companies = HashSet::new();
        let mut new_companies = HashSet::new();
        let mut person = PersonYield::default();
        let mut any_person = false;
        let mut unique_candidates = 0usize;
        let mut boards_ok = 0usize;
        let mut first_party = 0usize;
        for c in &self.store.candidates {
            let hits: Vec<&Sighting> = c.hits().filter(|s| touched(s)).collect();
            let any = c.sightings.iter().any(&touched);
            if !any {
                continue;
            }
            row.raw_hits += hits.len();
            for s in &hits {
                urls.insert(s.discovered_url.as_str());
            }
            if !hits.is_empty() {
                unique_candidates += 1;
            }
            match c.kind {
                CandidateKind::Job if !hits.is_empty() => {
                    row.jobs += 1;
                    match c.status {
                        CandidateStatus::Live => {
                            row.live_jobs += 1;
                            if let Some(j) = &c.job_result {
                                row.engineering_jobs += usize::from(j.engineering);
                                row.fresh_jobs += usize::from(
                                    self.store_now()
                                        .zip(j.posted_at)
                                        .is_some_and(|(now, p)| (now - p).num_days() < 30),
                                );
                            }
                            row.useful_jobs += usize::from(useful_job(c));
                        }
                        CandidateStatus::Closed => row.closed_jobs += 1,
                        _ => {}
                    }
                }
                CandidateKind::Board => {
                    row.boards += 1;
                    if self
                        .board_strategies
                        .get(&c.key)
                        .is_some_and(|s| s.len() == 1)
                    {
                        row.unique_boards += 1;
                    }
                    if let Some(b) = &c.board_result
                        && b.listing == "ok"
                    {
                        row.boards_read += 1;
                        if b.ownership_checked {
                            boards_ok += 1;
                            first_party += usize::from(b.ownership.established());
                        }
                    }
                    match c.status {
                        CandidateStatus::Validated => {
                            row.validated_boards += 1;
                            if let Some(y) = c.board_result.as_ref().and_then(|b| b.yields.as_ref())
                            {
                                row.board_postings += y.open;
                                row.board_engineering += y.engineering;
                                row.board_engineering_brazil +=
                                    y.engineering_brazil_open + y.engineering_brazil_unclear;
                            }
                            if let Some(p) = c.source.as_ref().and_then(|s| self.person.get(s)) {
                                any_person = true;
                                person.actionable += p.actionable;
                                person.plausible += p.plausible;
                                person.strong += p.strong;
                            }
                        }
                        CandidateStatus::Inconclusive => row.inconclusive_boards += 1,
                        CandidateStatus::Rejected => row.rejected_boards += 1,
                        _ => {}
                    }
                    row.useful_boards += usize::from(useful_board(c));
                    row.ready_boards += usize::from(ready_board(c));
                    let company = self.company_of(c);
                    if self.new_to_narrow(c) {
                        new_companies.insert(company.clone());
                    }
                    companies.insert(company);
                }
                CandidateKind::Company => {
                    let company = self.company_of(c);
                    if self.new_to_narrow(c) {
                        new_companies.insert(company.clone());
                    }
                    companies.insert(company);
                }
                CandidateKind::UnsupportedAts => row.unsupported_ats += 1,
                _ => {}
            }
        }
        row.unique_urls = urls.len();
        row.companies = companies.len();
        row.new_companies = new_companies.len();
        row.person = any_person.then_some(person);
        row.duplicate_rate =
            (row.raw_hits > 0).then(|| 1.0 - unique_candidates as f64 / row.raw_hits as f64);
        row.stale_rate = rate(row.closed_jobs, row.live_jobs + row.closed_jobs);
        row.first_party_rate = rate(first_party, boards_ok);
        row
    }

    fn store_now(&self) -> Option<DateTime<Utc>> {
        self.store
            .candidates
            .iter()
            .filter_map(|c| c.last_checked_at)
            .max()
    }

    fn line(&self, c: &Candidate) -> BoardLine {
        let b = c.board_result.as_ref();
        let y = b.and_then(|b| b.yields.as_ref());
        BoardLine {
            source: c.source.clone().unwrap_or_else(|| c.key.clone()),
            company: b
                .and_then(|b| b.company.clone())
                .or_else(|| c.company_hint.clone()),
            domain: b
                .and_then(|b| b.domain.clone())
                .or_else(|| c.domain_hint.clone()),
            ownership: b.map_or(Ownership::Unknown, |b| b.ownership),
            status: c.status,
            new_to_narrow: self.new_to_narrow(c),
            open: y.map_or(0, |y| y.open),
            engineering: y.map_or(0, |y| y.engineering),
            engineering_brazil_open: y.map_or(0, |y| y.engineering_brazil_open),
            engineering_brazil_unclear: y.map_or(0, |y| y.engineering_brazil_unclear),
            main_geo: y.and_then(crate::registry::SourceYield::main_geo),
            stale_share: y.and_then(|y| y.freshness.stale_share()),
            person: c.source.as_ref().and_then(|s| self.person.get(s)).copied(),
            strategies: self
                .board_strategies
                .get(&c.key)
                .map(|s| s.iter().cloned().collect())
                .unwrap_or_default(),
            why: b
                .map(|b| {
                    let mut why = b
                        .validation
                        .reasons
                        .iter()
                        .rev()
                        .take(1)
                        .cloned()
                        .collect::<Vec<_>>();
                    why.extend(b.activation.iter().map(|a| format!("not ready: {a}")));
                    why
                })
                .unwrap_or_default(),
        }
    }
}

/// Whether discovery context names remote work in Brazil, Latin America,
/// the Americas or anywhere (a post's header, a directory's regions).
fn context_useful(c: &Candidate) -> bool {
    c.sightings.iter().any(|s| {
        s.metadata.iter().any(|(k, v)| {
            let v = v.to_ascii_lowercase();
            matches!(k.as_str(), "geo" | "regions" | "region")
                && [
                    "latam",
                    "latin america",
                    "brazil",
                    "global",
                    "worldwide",
                    "americas",
                    "anywhere",
                ]
                .iter()
                .any(|w| v.contains(w))
        })
    })
}

/// Per unsupported provider: companies, URLs, companies with a useful
/// geography, companies also on a supported board.
type UnsupportedTally = (BTreeSet<String>, usize, BTreeSet<String>, BTreeSet<String>);

/// Builds the report. `person` maps source keys to the person's ranking
/// counts (empty when no profile was ranked).
pub fn build(
    store: &DiscoveryStore,
    registry: Option<&SourceRegistry>,
    person: &HashMap<String, PersonYield>,
) -> Report {
    let mut board_strategies: HashMap<String, BTreeSet<String>> = HashMap::new();
    for c in store.of_kind(CandidateKind::Board) {
        board_strategies.insert(
            c.key.clone(),
            c.sightings.iter().map(Sighting::strategy).collect(),
        );
    }
    let facts = Facts {
        store,
        known_sources: registry
            .map(|r| r.sources.iter().map(|e| e.source.to_string()).collect())
            .unwrap_or_default(),
        known_domains: registry
            .map(|r| r.sources.iter().filter_map(|e| e.domain.clone()).collect())
            .unwrap_or_default(),
        person,
        board_strategies,
    };
    let mut report = Report {
        now: facts.store_now(),
        ..Report::default()
    };
    for c in &store.candidates {
        *report
            .candidates
            .entry(c.kind.as_str().to_owned())
            .or_default()
            .entry(c.status.as_str().to_owned())
            .or_default() += 1;
    }
    report.total = facts.row("total".into(), |_| true);

    let mut strategies = BTreeSet::new();
    let mut families = BTreeSet::new();
    let mut queries = BTreeSet::new();
    let mut hosts = BTreeSet::new();
    let mut geos = BTreeSet::new();
    for c in &store.candidates {
        for s in &c.sightings {
            strategies.insert(s.strategy());
            if let Some(f) = &s.family {
                families.insert(f.clone());
            }
            if let Some(q) = &s.query {
                queries.insert(q.clone());
            }
            if let Some(h) = s.metadata.get("host") {
                hosts.insert(h.clone());
            }
            if s.method == super::Method::AtsSearch
                && let Some(g) = s.metadata.get("geo")
            {
                geos.insert(g.clone());
            }
        }
    }
    report.strategies = strategies
        .into_iter()
        .map(|k| facts.row(k.clone(), |s| s.strategy() == k))
        .collect();
    report.families = families
        .into_iter()
        .map(|k| facts.row(k.clone(), |s| s.family.as_deref() == Some(&k)))
        .collect();
    report.queries = queries
        .into_iter()
        .map(|k| facts.row(k.clone(), |s| s.query.as_deref() == Some(&k)))
        .collect();
    report.hosts = hosts
        .into_iter()
        .map(|k| facts.row(k.clone(), |s| s.metadata.get("host") == Some(&k)))
        .collect();
    report.geo = geos
        .into_iter()
        .map(|k| {
            facts.row(k.clone(), |s| {
                s.method == super::Method::AtsSearch && s.metadata.get("geo") == Some(&k)
            })
        })
        .collect();

    let mut providers: BTreeMap<String, ProviderRow> = BTreeMap::new();
    for c in store.of_kind(CandidateKind::Board) {
        let p = c.provider.clone().unwrap_or_default();
        let row = providers.entry(p.clone()).or_insert_with(|| ProviderRow {
            provider: p,
            ..ProviderRow::default()
        });
        row.boards += 1;
        row.boards_read += usize::from(c.board_result.as_ref().is_some_and(|b| b.listing == "ok"));
        row.validated += usize::from(c.status == CandidateStatus::Validated);
        row.useful += usize::from(useful_board(c));
        row.ready += usize::from(ready_board(c));
        row.postings += c.board_result.as_ref().map_or(0, |b| b.jobs);
    }
    report.providers = providers.into_values().collect();

    // Unsupported ATS: direct URLs, and companies whose site names one.
    let supported_domains: HashSet<String> = store
        .of_kind(CandidateKind::Board)
        .filter(|c| c.status == CandidateStatus::Validated)
        .filter_map(|c| c.board_result.as_ref().and_then(|b| b.domain.clone()))
        .collect();
    let mut unsupported: BTreeMap<String, UnsupportedTally> = BTreeMap::new();
    for c in &store.candidates {
        let mut add = |provider: &str, who: String, urls: usize| {
            let e = unsupported.entry(provider.to_owned()).or_default();
            e.1 += urls;
            if context_useful(c) {
                e.2.insert(who.clone());
            }
            if c.domain_hint
                .as_ref()
                .is_some_and(|d| supported_domains.contains(d))
            {
                e.3.insert(who.clone());
            }
            e.0.insert(who);
        };
        match c.kind {
            CandidateKind::UnsupportedAts => {
                let who = c
                    .domain_hint
                    .clone()
                    .or_else(|| c.tenant.clone())
                    .unwrap_or_else(|| c.key.clone());
                add(
                    c.provider.as_deref().unwrap_or_default(),
                    who,
                    c.hits().count().max(1),
                );
            }
            CandidateKind::Company => {
                if let Some(r) = &c.company_result {
                    for p in &r.other_ats {
                        add(p, c.domain_hint.clone().unwrap_or_else(|| c.key.clone()), 1);
                    }
                }
            }
            _ => {}
        }
    }
    let mut rows: Vec<UnsupportedRow> = unsupported
        .into_iter()
        .map(|(provider, (who, urls, useful, also))| UnsupportedRow {
            provider,
            companies: who.len(),
            urls,
            brazil_latam_global: useful.len(),
            also_supported: also.len(),
            examples: who.into_iter().take(8).collect(),
        })
        .collect();
    rows.sort_by(|a, b| {
        b.companies
            .cmp(&a.companies)
            .then(a.provider.cmp(&b.provider))
    });
    report.unsupported = rows;

    for c in store.of_kind(CandidateKind::Board) {
        if let Some(b) = &c.board_result {
            *report
                .ownership
                .entry(b.ownership.as_str().to_owned())
                .or_default() += 1;
        }
    }
    let mut activate: Vec<BoardLine> = store
        .of_kind(CandidateKind::Board)
        .filter(|c| ready_board(c) && useful_board(c))
        .map(|c| facts.line(c))
        .collect();
    activate.sort_by(|a, b| {
        let key = |l: &BoardLine| {
            let p = l.person.unwrap_or_default();
            (
                p.strong,
                p.plausible,
                l.engineering_brazil_open,
                l.engineering_brazil_unclear,
            )
        };
        key(b).cmp(&key(a)).then(a.source.cmp(&b.source))
    });
    report.activate = activate;
    report.hold = store
        .of_kind(CandidateKind::Board)
        .filter(|c| c.status == CandidateStatus::Validated && !(ready_board(c) && useful_board(c)))
        .count();
    let mut recheck: Vec<BoardLine> = store
        .of_kind(CandidateKind::Board)
        .filter(|c| {
            c.status == CandidateStatus::Inconclusive
                && c.board_result
                    .as_ref()
                    .and_then(|b| b.yields.as_ref())
                    .is_some_and(|y| y.engineering_brazil_open + y.engineering_brazil_unclear > 0)
        })
        .map(|c| facts.line(c))
        .collect();
    recheck.sort_by_key(|l| {
        std::cmp::Reverse((l.engineering_brazil_open, l.engineering_brazil_unclear))
    });
    report.recheck = recheck;
    report.rejected = store
        .of_kind(CandidateKind::Board)
        .filter(|c| c.status == CandidateStatus::Rejected)
        .count();
    report
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::discovery::{BoardResult, JobResult, Lead, Method};
    use crate::registry::{SourceYield, Validation, Verdict};

    fn at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 5, 12, 0, 0).unwrap()
    }

    fn hit(query: &str, url: &str) -> Lead {
        let mut s = Sighting::new(Method::AtsSearch, "websearch", url, at());
        s.query = Some(query.into());
        s.family = Some(format!("{query} family"));
        Lead::url(url, s)
    }

    #[test]
    fn strategy_rows_count_duplicates_stale_jobs_and_validated_boards() {
        let mut store = DiscoveryStore::new();
        let live = "https://jobs.ashbyhq.com/acme/11111111-1111-1111-1111-111111111111";
        let dead = "https://jobs.ashbyhq.com/acme/22222222-2222-2222-2222-222222222222";
        store.add(hit("a", live));
        store.add(hit("a", &format!("{live}/application")));
        store.add(hit("a", dead));
        store.add(hit("b", "https://jobs.lever.co/gone"));
        let mut hn = Sighting::new(Method::HiringThread, "hn:1", live, at());
        hn.metadata.insert("geo".into(), "latam".into());
        store.add(Lead::url(live, hn));

        let job = |c: &mut Candidate, status, eng| {
            c.status = status;
            c.last_checked_at = Some(at());
            c.job_result = Some(JobResult {
                title: None,
                posted_at: Some(at() - chrono::Duration::days(3)),
                first_seen_at: at(),
                engineering: eng,
                geo: Some(GeoBucket::Global),
                brazil: Some("unclear".into()),
            });
        };
        job(
            store
                .get_mut(&format!(
                    "job:ashby:acme:{}",
                    "11111111-1111-1111-1111-111111111111"
                ))
                .unwrap(),
            CandidateStatus::Live,
            true,
        );
        job(
            store
                .get_mut(&format!(
                    "job:ashby:acme:{}",
                    "22222222-2222-2222-2222-222222222222"
                ))
                .unwrap(),
            CandidateStatus::Closed,
            false,
        );
        let validated = Validation {
            verdict: Verdict::Validated,
            reasons: vec!["ok".into()],
        };
        let acme = store.get_mut("board:ashby:acme").unwrap();
        acme.status = CandidateStatus::Validated;
        acme.board_result = Some(BoardResult {
            listing: "ok".into(),
            jobs: 5,
            error: None,
            company: Some("Acme".into()),
            domain: Some("acme.com".into()),
            careers_url: None,
            hints: vec![],
            ownership: Ownership::Verified,
            points_at: vec![],
            validation: validated,
            yields: Some(SourceYield {
                open: 5,
                engineering: 3,
                engineering_brazil_unclear: 1,
                ..SourceYield::default()
            }),
            activation: vec![],
            ownership_checked: true,
        });
        let gone = store.get_mut("board:lever:gone").unwrap();
        gone.status = CandidateStatus::Rejected;

        let person = HashMap::from([(
            "ashby:acme".to_owned(),
            PersonYield {
                actionable: 4,
                plausible: 2,
                strong: 1,
            },
        )]);
        let report = build(&store, None, &person);
        let a = report.queries.iter().find(|r| r.key == "a").unwrap();
        assert_eq!(a.raw_hits, 3);
        assert_eq!(a.jobs, 2);
        assert_eq!((a.live_jobs, a.closed_jobs), (1, 1));
        assert_eq!(a.stale_rate, Some(0.5));
        assert_eq!(a.useful_jobs, 1);
        assert_eq!(a.fresh_jobs, 1);
        assert!((a.duplicate_rate.unwrap() - 1.0 / 3.0).abs() < 1e-9);
        assert_eq!((a.boards, a.validated_boards, a.useful_boards), (1, 1, 1));
        assert_eq!(a.person.unwrap().strong, 1);
        assert_eq!(a.first_party_rate, Some(1.0));
        let b = report.queries.iter().find(|r| r.key == "b").unwrap();
        assert_eq!((b.boards, b.rejected_boards, b.unique_boards), (1, 1, 1));
        // The HN post saw the same job: acme is not unique to search.
        let search = report
            .strategies
            .iter()
            .find(|r| r.key == "ats_search")
            .unwrap();
        assert_eq!(search.unique_boards, 1);
        assert_eq!(report.activate.len(), 1);
        assert_eq!(
            report.activate[0].strategies,
            ["ats_search", "hiring_thread:hn"]
        );
        assert_eq!(report.rejected, 1);
    }

    #[test]
    fn unsupported_ats_are_counted_by_company() {
        let mut store = DiscoveryStore::new();
        for (tenant, job) in [("acme", "1"), ("acme", "2"), ("beta", "3")] {
            let url = format!("https://jobs.smartrecruiters.com/{tenant}/{job}");
            let mut s = Sighting::new(Method::HiringThread, "hn:1", &url, at());
            s.metadata.insert("geo".into(), "remote,latam".into());
            store.add(Lead {
                url: Some(url),
                domain: None,
                company: None,
                title: None,
                sighting: s,
            });
        }
        let report = build(&store, None, &HashMap::new());
        let row = &report.unsupported[0];
        assert_eq!(
            (
                row.provider.as_str(),
                row.companies,
                row.urls,
                row.brazil_latam_global
            ),
            ("smartrecruiters", 2, 3, 2)
        );
    }
}
