//! Evaluating Narrow's recommendations against a fixed, reviewable
//! benchmark before ranking changes.
//!
//! The recommendation benchmark asks, for synthetic candidates and
//! realistic postings, whether a job that Today labels "worth your
//! attention" is one a reasonable candidate with that profile would
//! genuinely consider applying to. Each (candidate, job) pair carries a
//! judgment made of two separate answers, fit and practicality
//! ([`taxonomy`]), with structured reasons; the current ranker runs on the
//! same inputs production gives it ([`build`], [`run`](mod@run)); and the report
//! shows each measurement on its own ([`metrics`], [`report`]).
//!
//! * [`fixture`]: the fixture files and their integrity checks.
//! * [`taxonomy`]: fit, practicality, labels, reasons, contradictions.
//! * [`build`]: fixtures into canonical profiles, job records and
//!   verifications.
//! * [`run`](mod@run): the production ranker and Today's selection on each pool.
//! * [`metrics`]: precision, false positives, contradiction misses,
//!   density.
//! * [`report`]: the Markdown report and recorded baseline.
//! * [`job_class`]: the BRU-330 semantic job-classifier experiment (the
//!   job only, never a candidate), scored against real postings.
//! * [`job_function`]: its function-only follow-up, measured on a fresh
//!   real-posting snapshot.
//! * [`breadth`]: how concentrated a role's own work is (broad, focused,
//!   deep specialist), read without a specialty taxonomy.
//! * [`job_ic`]: a binary "software engineering IC or not" classifier with
//!   three votes per posting, its follow-up.
//!
//! Everything is deterministic and offline: no network, no database, no
//! model calls, a fixed clock. `docs/recommendation-quality.md` says what
//! future ranking work must preserve.

pub mod breadth;
pub mod build;
pub mod fixture;
pub mod job_class;
pub mod job_function;
pub mod job_ic;
pub mod metrics;
pub mod report;
pub mod run;
pub mod taxonomy;

pub use fixture::{FixtureError, Fixtures, default_dir};
pub use metrics::{Metrics, Ratio};
pub use run::{CandidateRun, Case, Run, VariantRun, Verdict, run};
pub use taxonomy::{Contradiction, Fit, Label, Practicality, Reason, TodayExpectation};

/// Where the recorded baseline report lives.
pub fn baseline_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("baseline/recommendation.md")
}
