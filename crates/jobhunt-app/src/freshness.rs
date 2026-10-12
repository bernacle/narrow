//! How fresh a job is, said honestly (BRU-389 §13.3).
//!
//! Narrow learns about a job in one of two ways: the board says when it
//! was published (`posted_at`), or Narrow notices it appear. Neither means
//! "new" by itself:
//!
//! * a board's `posted_at` is reliable from Greenhouse (`first_published`)
//!   and Ashby (`publishedAt`); Lever's is when the posting was created,
//!   not published, so it is only approximate; other sources have none;
//! * `first_seen_at` is when Narrow first read the job. When a board is
//!   added, every job on it is "first seen" that day, however old: that is
//!   a **backfill**, never news. A job is **newly listed** only when it
//!   was missing from a complete listing of a board Narrow was already
//!   reading, taken recently enough ([`NOVELTY_WINDOW_HOURS`]) to say when
//!   it appeared;
//! * a job that closed and came back is **reopened**, not new.
//!
//! "New" in any copy means `posted` (recently) or `newly_listed`. Old jobs
//! are not hidden: they are labelled with their age.

use chrono::{DateTime, Duration, Utc};
use jobhunt_jobs::{JobEvent, JobEventKind, JobRecord};
use serde::{Deserialize, Serialize};

/// The previous complete listing must be at most this old for a job
/// missing from it to count as newly listed.
pub const NOVELTY_WINDOW_HOURS: i64 = 12;

/// What the evidence says about a job's age.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FreshnessClass {
    /// The board's reliable publish date.
    Posted,
    /// The board's approximate date (Lever: created, not published).
    PostedApprox,
    /// Missing from the previous complete listing of a board Narrow was
    /// already reading, then present.
    NewlyListed,
    /// Present in the first complete listing Narrow took of its board:
    /// its age is the board's date, if it has one.
    Backfill,
    /// No reliable date.
    Unknown,
    /// Closed, then listed again.
    Reopened,
}

impl FreshnessClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Posted => "posted",
            Self::PostedApprox => "posted_approx",
            Self::NewlyListed => "newly_listed",
            Self::Backfill => "backfill",
            Self::Unknown => "unknown",
            Self::Reopened => "reopened",
        }
    }
}

/// A job's freshness: its class and the moment the label speaks of.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Freshness {
    pub class: FreshnessClass,
    /// Posted, listed, first seen or reopened at (by class); for a
    /// backfill, the board's date if it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<DateTime<Utc>>,
    /// `at` is the board's approximate date (Lever).
    #[serde(default)]
    pub approximate: bool,
}

/// How much a source's `posted_at` can be trusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dated {
    Reliable,
    Approximate,
}

fn dated(source_kind: &str) -> Dated {
    match source_kind {
        "greenhouse" | "ashby" => Dated::Reliable,
        // Lever's `createdAt`, and any other source's date until it is
        // known to be a publish date.
        _ => Dated::Approximate,
    }
}

impl Freshness {
    /// Classifies `record` from its `history` (oldest first) and when the
    /// last complete listing of its board before it was first seen
    /// finished (`None`: it was in the board's first complete listing).
    pub fn classify(
        record: &JobRecord,
        history: &[JobEvent],
        previous_listing: Option<DateTime<Utc>>,
    ) -> Self {
        let appeared = history
            .iter()
            .rev()
            .find(|e| matches!(e.kind, JobEventKind::New | JobEventKind::Reopened));
        if let Some(event) = appeared
            && event.kind == JobEventKind::Reopened
        {
            return Self {
                class: FreshnessClass::Reopened,
                at: Some(event.at),
                approximate: false,
            };
        }
        let posted = record
            .posting
            .posted_at
            .map(|at| (at, dated(record.posting.provenance.source.kind())));
        let first_seen = record.first_seen_at;
        let Some(previous) = previous_listing else {
            return Self {
                class: FreshnessClass::Backfill,
                at: posted.map(|(at, _)| at),
                approximate: posted.is_some_and(|(_, d)| d == Dated::Approximate),
            };
        };
        match posted {
            Some((at, Dated::Reliable)) => Self {
                class: FreshnessClass::Posted,
                at: Some(at),
                approximate: false,
            },
            Some((at, Dated::Approximate)) => Self {
                class: FreshnessClass::PostedApprox,
                at: Some(at),
                approximate: true,
            },
            None if first_seen - previous <= Duration::hours(NOVELTY_WINDOW_HOURS) => Self {
                class: FreshnessClass::NewlyListed,
                at: Some(first_seen),
                approximate: false,
            },
            None => Self {
                class: FreshnessClass::Unknown,
                at: None,
                approximate: false,
            },
        }
    }

    /// One line for a person: "Posted 4h ago", "New on Acme's job board ·
    /// first seen 3h ago", "Posted date unknown". Never calls a backfill or
    /// a reopened job new.
    pub fn label(&self, company: &str, now: DateTime<Utc>) -> String {
        let dated = |verb: &str| match self.at {
            Some(at) => format!("{verb} {}", age(now - at)),
            None => "Posted date unknown".to_owned(),
        };
        match self.class {
            FreshnessClass::Posted => dated("Posted"),
            FreshnessClass::PostedApprox => dated("Listed"),
            FreshnessClass::Backfill if self.approximate => dated("Listed"),
            FreshnessClass::Backfill => dated("Posted"),
            FreshnessClass::NewlyListed => match self.at {
                Some(at) => format!(
                    "New on {company}'s job board · first seen {}",
                    age(now - at)
                ),
                None => "Posted date unknown".to_owned(),
            },
            FreshnessClass::Reopened => dated("Reopened"),
            FreshnessClass::Unknown => "Posted date unknown".to_owned(),
        }
    }
}

/// "less than an hour ago", "4h ago", "1 day ago", "342 days ago".
pub fn age(elapsed: Duration) -> String {
    let elapsed = elapsed.max(Duration::zero());
    if elapsed < Duration::hours(1) {
        "less than an hour ago".to_owned()
    } else if elapsed < Duration::hours(24) {
        format!("{}h ago", elapsed.num_hours())
    } else if elapsed < Duration::hours(48) {
        "1 day ago".to_owned()
    } else {
        format!("{} days ago", elapsed.num_days())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use jobhunt_core::{CanonicalUrl, Provenance, SourceKey};
    use jobhunt_jobs::{JobPosting, JobStatus, OpportunityId};

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 12, 12, 0, 0).unwrap()
    }

    fn record(
        kind: &str,
        posted_at: Option<DateTime<Utc>>,
        first_seen: DateTime<Utc>,
    ) -> JobRecord {
        let posting = JobPosting {
            provenance: Provenance {
                source: SourceKey::new(kind, "acme").unwrap(),
                source_record_id: Some("1".into()),
                fetched_from: None,
            },
            url: CanonicalUrl::parse("https://jobs.example.com/acme/1").unwrap(),
            apply_url: None,
            company: "Acme".into(),
            title: "Backend Engineer".into(),
            department: None,
            team: None,
            location: None,
            locations: Vec::new(),
            employment_type: None,
            workplace_type: None,
            is_remote: None,
            compensation: None,
            work_authorization: None,
            description_text: None,
            description_html: None,
            posted_at,
            source_updated_at: None,
        };
        JobRecord {
            id: posting.id(),
            opportunity_id: OpportunityId::founded_by(posting.id()),
            posting,
            first_seen_at: first_seen,
            last_seen_at: first_seen,
            content_updated_at: first_seen,
            status: JobStatus::Open,
            closed_at: None,
        }
    }

    fn event(kind: JobEventKind, at: DateTime<Utc>) -> JobEvent {
        JobEvent {
            kind,
            at,
            run: None,
            changed_fields: Vec::new(),
            previous: None,
        }
    }

    fn hours(h: i64) -> Duration {
        Duration::hours(h)
    }

    #[test]
    fn reliable_board_dates_are_posted() {
        for kind in ["greenhouse", "ashby"] {
            let seen = now() - hours(3);
            let r = record(kind, Some(now() - hours(4)), seen);
            let f =
                Freshness::classify(&r, &[event(JobEventKind::New, seen)], Some(seen - hours(1)));
            assert_eq!(f.class, FreshnessClass::Posted, "{kind}");
            assert_eq!(f.label("Acme", now()), "Posted 4h ago");
        }
        let r = record("ashby", Some(now() - Duration::days(9)), now() - hours(1));
        let f = Freshness::classify(&r, &[], Some(now() - hours(2)));
        assert_eq!(f.label("Acme", now()), "Posted 9 days ago");
    }

    #[test]
    fn lever_dates_are_approximate() {
        let seen = now() - hours(2);
        let r = record("lever", Some(now() - hours(4)), seen);
        let f = Freshness::classify(&r, &[event(JobEventKind::New, seen)], Some(seen - hours(1)));
        assert_eq!(f.class, FreshnessClass::PostedApprox);
        assert_eq!(f.label("Acme", now()), "Listed 4h ago");
    }

    #[test]
    fn the_first_listing_of_a_board_is_a_backfill_never_new() {
        let seen = now() - hours(2);
        // No date: unknown age.
        let r = record("yc", None, seen);
        let f = Freshness::classify(&r, &[event(JobEventKind::New, seen)], None);
        assert_eq!(f.class, FreshnessClass::Backfill);
        assert_eq!(f.label("Acme", now()), "Posted date unknown");
        // With the board's date: its real age, even when Narrow found it
        // two hours ago (the Oneleet case).
        let r = record("ashby", Some(now() - Duration::days(342)), seen);
        let f = Freshness::classify(&r, &[event(JobEventKind::New, seen)], None);
        assert_eq!(f.class, FreshnessClass::Backfill);
        let label = f.label("Oneleet", now());
        assert_eq!(label, "Posted 342 days ago");
        assert!(!label.to_lowercase().contains("new"));
        let r = record("lever", Some(now() - Duration::days(30)), seen);
        let f = Freshness::classify(&r, &[], None);
        assert_eq!(f.label("Acme", now()), "Listed 30 days ago");
    }

    #[test]
    fn a_job_missing_from_a_recent_complete_listing_is_newly_listed() {
        let seen = now() - hours(3);
        let r = record("yc", None, seen);
        let f = Freshness::classify(&r, &[event(JobEventKind::New, seen)], Some(seen - hours(1)));
        assert_eq!(f.class, FreshnessClass::NewlyListed);
        assert_eq!(
            f.label("RevenueCat", now()),
            "New on RevenueCat's job board · first seen 3h ago"
        );
        // The previous listing is too old to say when it appeared.
        let f = Freshness::classify(&r, &[], Some(seen - hours(30)));
        assert_eq!(f.class, FreshnessClass::Unknown);
        assert_eq!(f.label("Acme", now()), "Posted date unknown");
    }

    #[test]
    fn reopened_jobs_say_so() {
        let seen = now() - Duration::days(40);
        let r = record("greenhouse", Some(now() - Duration::days(41)), seen);
        let history = [
            event(JobEventKind::New, seen),
            event(JobEventKind::Closed, now() - Duration::days(10)),
            event(JobEventKind::Reopened, now() - Duration::days(2)),
            event(JobEventKind::Updated, now() - Duration::days(1)),
        ];
        let f = Freshness::classify(&r, &history, None);
        assert_eq!(f.class, FreshnessClass::Reopened);
        assert_eq!(f.label("Acme", now()), "Reopened 2 days ago");
    }

    #[test]
    fn ages_read_naturally() {
        assert_eq!(age(Duration::minutes(10)), "less than an hour ago");
        assert_eq!(age(hours(1)), "1h ago");
        assert_eq!(age(hours(23)), "23h ago");
        assert_eq!(age(hours(30)), "1 day ago");
        assert_eq!(age(Duration::days(342)), "342 days ago");
        assert_eq!(age(-hours(2)), "less than an hour ago", "clock skew");
    }
}
