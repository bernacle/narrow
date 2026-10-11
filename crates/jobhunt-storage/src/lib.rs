//! Storage backends for JobHunt.
//!
//! Each backend implements [`jobhunt_jobs::JobRepository`],
//! [`jobhunt_profile::ProfileRepository`],
//! [`jobhunt_jobs::verification::VerificationRepository`],
//! [`jobhunt_eligibility::EligibilityRepository`],
//! [`jobhunt_ranking::FeedbackRepository`] and
//! [`jobhunt_ranking::RankingRepository`], bundled as [`Store`], and owns its
//! own schema and migrations:
//!
//! * [`SqliteJobStore`]: the local store. One SQLite file holds jobs and the
//!   career profile of the one person using this machine
//!   (`migrations/sqlite`).
//! * [`postgres`]: the cloud store (`migrations/postgres`). Jobs, their
//!   history and their verification are one shared corpus; everything that
//!   belongs to a person is keyed by their account, encrypted where it is
//!   private, and only reachable through [`postgres::PgUserStore`], a view
//!   bound to one account.
//!
//! Nothing in the jobs, profile, eligibility or ranking domains, or the
//! discovery pipeline, knows which backend is in use.

pub mod postgres;
mod profile;
mod ranking;
mod sqlite;
mod sqlite_store;
mod state;
mod store;
pub mod sync;
mod verification;

pub use sqlite::{SqliteJobStore, StoreStats};
pub use state::{ProfileWrite, StateImport, StateImported};
pub use store::{FeedMark, ScanRecord, Shown, Store, WriteGuard};
