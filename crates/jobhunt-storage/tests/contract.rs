//! The storage contract: the same behavior from every backend.
//!
//! Each case is written once against `&dyn Store` and runs against the local
//! SQLite store and against a real Postgres database (a user's view of the
//! cloud store). The Postgres half needs `JOBHUNT_TEST_DATABASE_URL`; without
//! it that half is skipped (and fails when `JOBHUNT_REQUIRE_POSTGRES` is set,
//! as in CI's cloud job).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::sync::Arc;
use std::time::Duration as StdDuration;

use chrono::Duration;
use common::*;
use jobhunt_core::{IngestCounts, SourceKey, UpsertOutcome};
use jobhunt_eligibility::profile::ProfileFacts;
use jobhunt_eligibility::{CacheKey, cached_assess, cached_assess_many};
use jobhunt_jobs::verification::{
    FreshnessPolicy, ListingStatus, VerificationService, VerifyMode, cached,
};
use jobhunt_jobs::{JobEventKind, JobQuery, JobStatus, OpportunityId, ScanBody, ScanWrite};
use jobhunt_profile::{
    ClaimQuery, ProfileEventKind, ProfileService, StorageError as ProfileStorageError,
};
use jobhunt_ranking::{FeedbackAction, RankQuery, RankingService, RuleReader, Stage, Tier};
use jobhunt_storage::{StateImport, Store};

macro_rules! contract {
    ($($name:ident),* $(,)?) => {$(
        mod $name {
            #[tokio::test]
            async fn sqlite() {
                super::$name(super::sqlite().await.as_ref()).await;
            }

            #[tokio::test]
            async fn postgres() {
                let Some(fixture) = super::PgFixture::new().await else { return };
                let store = fixture.user("contract").await;
                super::$name(&store).await;
                fixture.finish().await;
            }
        }
    )*};
}

contract!(
    round_trips_and_classifies_the_lifecycle,
    not_modified_and_failed_scans_change_nothing,
    search_filters_orders_and_resolves_prefixes,
    identity_and_opportunities,
    verification_history_and_observations,
    profile_round_trip_revisions_and_claims,
    feedback_rankings_and_eligibility_cache,
    state_import_is_atomic_and_idempotent,
    write_lock_serializes_writers,
    structured_preference_values_round_trip,
    evidence_sources_round_trip,
    taste_profile_round_trips,
    fit_reviews_round_trip,
);

async fn round_trips_and_classifies_the_lifecycle(store: &dyn Store) {
    let a = posting("ramp", "1", "Engineer");
    let b = posting("ramp", "2", "Designer");
    let c = posting("ramp", "3", "Writer");
    let first = scan(
        store,
        "ramp",
        &[a.clone(), b.clone(), c.clone()],
        at(0),
        true,
    )
    .await;
    assert_eq!(first.outcomes, vec![UpsertOutcome::Inserted; 3]);

    let record = store.get(a.id()).await.unwrap().unwrap();
    assert_eq!(record.posting, a, "every field round-trips");
    assert_eq!(record.first_seen_at, at(0));
    assert_eq!(record.status, JobStatus::Open);
    assert_eq!(record.opportunity_id, OpportunityId::founded_by(a.id()));

    let mut a2 = a.clone();
    a2.title = "Senior Engineer".into();
    let mut b_cosmetic = b.clone();
    b_cosmetic.description_html = Some("<p class=\"x\">Build things.</p>".into());
    let second = scan(
        store,
        "ramp",
        &[a2.clone(), b_cosmetic.clone()],
        at(5),
        true,
    )
    .await;
    assert_eq!(
        second.outcomes,
        vec![UpsertOutcome::Updated, UpsertOutcome::Unchanged]
    );
    assert_eq!(second.closed, vec![c.id()]);
    let a_record = store.get(a.id()).await.unwrap().unwrap();
    assert_eq!(
        (
            a_record.first_seen_at,
            a_record.last_seen_at,
            a_record.content_updated_at
        ),
        (at(0), at(5), at(5))
    );
    let b_record = store.get(b.id()).await.unwrap().unwrap();
    assert_eq!(
        b_record.posting.description_html,
        b_cosmetic.description_html
    );
    assert_eq!(b_record.content_updated_at, at(0), "cosmetic change");
    let c_record = store.get(c.id()).await.unwrap().unwrap();
    assert_eq!(c_record.status, JobStatus::Closed);
    assert_eq!(c_record.closed_at, Some(at(5)));
    assert_eq!(c_record.last_seen_at, at(0));

    let mut c2 = c.clone();
    c2.compensation = None;
    let third = scan(store, "ramp", &[a2, b_cosmetic, c2], at(9), true).await;
    assert_eq!(
        third.outcomes,
        vec![
            UpsertOutcome::Unchanged,
            UpsertOutcome::Unchanged,
            UpsertOutcome::Reopened
        ]
    );
    let history = store.history(c.id()).await.unwrap();
    let kinds: Vec<_> = history.iter().map(|e| e.kind).collect();
    assert_eq!(
        kinds,
        vec![
            JobEventKind::New,
            JobEventKind::Closed,
            JobEventKind::Reopened
        ]
    );
    assert_eq!(history[2].changed_fields, vec!["compensation"]);
    assert_eq!(
        history[2].previous.as_ref().unwrap().compensation,
        c.compensation
    );
    let a_history = store.history(a.id()).await.unwrap();
    assert_eq!(a_history[1].kind, JobEventKind::Updated);
    assert_eq!(a_history[1].changed_fields, vec!["title"]);
    assert_eq!(a_history[1].previous.as_ref().unwrap().title, "Engineer");
    assert!(a_history[1].run.is_some());
    assert_eq!(store.count(&JobQuery::default()).await.unwrap(), 3);

    // A partial listing closes nothing.
    let partial = scan(store, "ramp", std::slice::from_ref(&a), at(12), false).await;
    assert!(partial.closed.is_empty());
    assert_eq!(
        store.get(b.id()).await.unwrap().unwrap().status,
        JobStatus::Open
    );
}

async fn not_modified_and_failed_scans_change_nothing(store: &dyn Store) {
    let source = SourceKey::new("ashby", "ramp").unwrap();
    let open = posting("ramp", "1", "Engineer");
    let closed = posting("ramp", "2", "Designer");
    let elsewhere = posting("linear", "1", "Engineer");
    scan(store, "ramp", &[open.clone(), closed.clone()], at(0), true).await;
    scan(store, "ramp", std::slice::from_ref(&open), at(1), true).await;
    scan(
        store,
        "linear",
        std::slice::from_ref(&elsewhere),
        at(1),
        true,
    )
    .await;
    let history_len = |id| async move { store.history(id).await.unwrap().len() };
    let before = (history_len(open.id()).await, history_len(closed.id()).await);
    let closed_before = store.get(closed.id()).await.unwrap().unwrap();

    let last = store.last_listing(&source).await.unwrap().unwrap();
    assert_eq!(last.finished_at, at(1));
    assert!(last.complete);
    assert_eq!(last.received, 1);
    assert_eq!(last.validator.as_deref(), Some("\"etag\""));

    let run = store.begin_run(at(5)).await.unwrap();
    for (minute, body) in [
        (5, ScanBody::NotModified),
        (6, ScanBody::Failed { error: "HTTP 503" }),
    ] {
        store
            .apply_scan(&ScanWrite {
                run,
                source: &source,
                started_at: at(minute),
                observed_at: at(minute),
                counts: IngestCounts::default(),
                body,
            })
            .await
            .unwrap();
    }
    let open_after = store.get(open.id()).await.unwrap().unwrap();
    assert_eq!(open_after.last_seen_at, at(5), "a 304 marks open jobs seen");
    assert_eq!(
        store.get(closed.id()).await.unwrap().unwrap(),
        closed_before
    );
    assert_eq!(
        store
            .get(elsewhere.id())
            .await
            .unwrap()
            .unwrap()
            .last_seen_at,
        at(1)
    );
    assert_eq!(
        (history_len(open.id()).await, history_len(closed.id()).await),
        before,
        "no history for unchanged observations"
    );
    // The last *listing* is still the full one; the source counts as read
    // at the 304.
    assert_eq!(store.last_listing(&source).await.unwrap().unwrap(), last);
    let checked = store.last_checked().await.unwrap();
    assert_eq!(checked.get(&source), Some(&at(5)));
    assert_eq!(
        checked.get(&SourceKey::new("ashby", "linear").unwrap()),
        Some(&at(1))
    );

    // Recent scans (source health), newest first, failures included.
    let recent = store.recent_scans(2).await.unwrap();
    let scans = &recent[&source];
    let seen: Vec<_> = scans
        .iter()
        .map(|s| (s.finished_at, s.status.as_str(), s.error.as_deref()))
        .collect();
    assert_eq!(
        seen,
        vec![
            (at(6), "failed", Some("HTTP 503")),
            (at(5), "not_modified", None)
        ]
    );
    let all = store.recent_scans(10).await.unwrap();
    let received: Vec<_> = all[&source]
        .iter()
        .map(|s| (s.status.as_str(), s.received))
        .collect();
    assert_eq!(
        received,
        vec![
            ("failed", 0),
            ("not_modified", 0),
            ("listing", 1),
            ("listing", 2)
        ]
    );
    assert_eq!(all[&SourceKey::new("ashby", "linear").unwrap()].len(), 1);
}

async fn search_filters_orders_and_resolves_prefixes(store: &dyn Store) {
    let mut old = posting("ramp", "1", "Backend Engineer");
    old.posted_at = Some(at(-600));
    let mut new = posting("ramp", "2", "Frontend Engineer");
    new.posted_at = Some(at(-60));
    let mut undated = posting("ramp", "3", "Node.js Developer");
    undated.posted_at = None;
    let other = posting("linear", "1", "Backend Engineer");
    scan(
        store,
        "ramp",
        &[old.clone(), new.clone(), undated.clone()],
        at(0),
        true,
    )
    .await;
    scan(store, "linear", std::slice::from_ref(&other), at(10), true).await;

    let ids = |records: Vec<jobhunt_jobs::JobRecord>| -> Vec<_> {
        records.into_iter().map(|r| r.id).collect()
    };
    // Newest posted first, undated last; ties by first seen (newest), id.
    let all = store.search(&JobQuery::default()).await.unwrap();
    let order = ids(all);
    assert_eq!(order.last(), Some(&undated.id()));
    assert_eq!(order[0], new.id());

    let backend = store
        .search(&JobQuery::default().with_text("backend"))
        .await
        .unwrap();
    assert_eq!(ids(backend).len(), 2);
    let word_start = store
        .search(&JobQuery::default().with_text("end"))
        .await
        .unwrap();
    assert!(word_start.is_empty(), "terms match word starts only");
    let node = store
        .search(&JobQuery::default().with_text("node.js"))
        .await
        .unwrap();
    assert_eq!(ids(node), vec![undated.id()]);

    let only_linear = JobQuery {
        sources: vec![SourceKey::new("ashby", "linear").unwrap()],
        ..JobQuery::default()
    };
    assert_eq!(
        ids(store.search(&only_linear).await.unwrap()),
        vec![other.id()]
    );
    let seen_late = JobQuery {
        seen_since: Some(at(5)),
        ..JobQuery::default()
    };
    assert_eq!(store.count(&seen_late).await.unwrap(), 1);
    let limited = JobQuery {
        limit: Some(2),
        ..JobQuery::default()
    };
    assert_eq!(store.search(&limited).await.unwrap().len(), 2);
    assert_eq!(
        store.count(&limited).await.unwrap(),
        4,
        "count ignores limit"
    );

    // Merge two into one opportunity: distinct search returns it once.
    store
        .assign_opportunities(&[(other.id(), OpportunityId::founded_by(old.id()))])
        .await
        .unwrap();
    let distinct = JobQuery {
        distinct_opportunities: true,
        status: Some(JobStatus::Open),
        ..JobQuery::default()
    };
    assert_eq!(store.search(&distinct).await.unwrap().len(), 3);
    assert_eq!(store.count(&distinct).await.unwrap(), 3);

    // Prefix lookups.
    let opp = OpportunityId::founded_by(new.id()).to_string();
    let found = store
        .opportunities_with_prefix(&opp[..12], 5)
        .await
        .unwrap();
    assert_eq!(found, vec![OpportunityId::founded_by(new.id())]);
    let job = new.id().to_string();
    assert_eq!(
        store.jobs_with_prefix(&job[..12], 5).await.unwrap(),
        vec![new.id()]
    );
    assert!(store.jobs_with_prefix("job_%", 5).await.unwrap().is_empty());
}

async fn identity_and_opportunities(store: &dyn Store) {
    let a = posting("ramp", "1", "Engineer");
    let b = posting("linear", "9", "Engineer");
    scan(store, "ramp", std::slice::from_ref(&a), at(0), true).await;
    scan(store, "linear", std::slice::from_ref(&b), at(1), true).await;
    let mut index = store.identity_index().await.unwrap();
    index.sort_by_key(|e| e.job);
    assert_eq!(index.len(), 2);
    let entry = index.iter().find(|e| e.job == a.id()).unwrap();
    assert!(entry.evidence.iter().any(|k| k.starts_with("url:")));
    let target = OpportunityId::founded_by(a.id());
    store
        .assign_opportunities(&[(b.id(), target)])
        .await
        .unwrap();
    let records = store.opportunity_records(target).await.unwrap();
    assert_eq!(
        records.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![a.id(), b.id()],
        "earliest seen first"
    );
}

async fn verification_history_and_observations(store: &dyn Store) {
    let a = posting("ramp", "1", "Engineer");
    scan(store, "ramp", std::slice::from_ref(&a), at(0), true).await;
    let record = store.get(a.id()).await.unwrap().unwrap();
    let mut changed = a.clone();
    changed.title = "Staff Engineer".into();
    let live = Live::new(std::slice::from_ref(&changed));
    let policy = FreshnessPolicy::default();
    let service = VerificationService::new(store, &live).with_policy(policy);
    let first = service
        .verify(std::slice::from_ref(&record), VerifyMode::Force, at(10))
        .await
        .unwrap();
    assert_eq!(first[0].record.posting.title, "Staff Engineer");
    let history = store.history(a.id()).await.unwrap();
    assert_eq!(history.last().unwrap().kind, JobEventKind::Updated);
    assert!(history.last().unwrap().run.is_none(), "not a discovery run");

    // Reused within the window.
    let reused = service
        .verify(std::slice::from_ref(&record), VerifyMode::IfDue, at(11))
        .await
        .unwrap();
    assert!(reused[0].reused);

    // Gone at the source: a definitive "closed" answer.
    *live.0.lock().unwrap() = Vec::new();
    service
        .verify(std::slice::from_ref(&record), VerifyMode::Force, at(100))
        .await
        .unwrap();
    let all = store.verification_history(a.id()).await.unwrap();
    assert_eq!(all.len(), 2);
    assert!(all[0].attempted_at > all[1].attempted_at, "newest first");
    let latest = store.latest_verification(a.id()).await.unwrap().unwrap();
    assert_eq!(latest.listing, ListingStatus::Closed);
    let success = store
        .latest_successful_verification(a.id())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(success.id, latest.id);

    let verified = cached(store, std::slice::from_ref(&record)).await.unwrap();
    assert_eq!(verified[0].latest.as_ref().map(|v| v.id), Some(latest.id));

    // Importing an attempt made elsewhere is idempotent.
    let mut elsewhere = latest.clone();
    elsewhere.attempted_at = at(200);
    elsewhere.id = jobhunt_jobs::verification::VerificationId::derive(a.id(), at(200));
    assert!(store.import_verification(&elsewhere).await.unwrap());
    assert!(!store.import_verification(&elsewhere).await.unwrap());
    assert_eq!(
        store.latest_verification(a.id()).await.unwrap().unwrap().id,
        elsewhere.id
    );
}

async fn profile_round_trip_revisions_and_claims(store: &dyn Store) {
    let now = at(0);
    import_resume(store, "ana_lima.md", now).await;
    let service = ProfileService::new(store);
    let data = service.require().await.unwrap();
    assert_eq!(data.profile.revision, 1);
    assert!(!data.experiences.is_empty());
    assert!(!data.claims.is_empty());
    assert!(!data.documents[0].text.is_empty());

    // Decide a claim, set a preference: both survive a reload.
    let claim = data.claims[0].id;
    service
        .decide_claims(
            &[claim.to_string()],
            jobhunt_profile::Verification::Rejected,
            Some("not accurate".into()),
            at(1),
        )
        .await
        .unwrap();
    service
        .add_statement(
            "I want remote backend roles, at least USD 150k",
            &jobhunt_profile::RuleParser,
            at(2),
        )
        .await
        .unwrap();
    let reloaded = service.require().await.unwrap();
    assert_eq!(reloaded.profile.revision, 3);
    assert_eq!(
        reloaded.claim(claim).unwrap().verification,
        jobhunt_profile::Verification::Rejected
    );
    assert_eq!(reloaded.statements.len(), 1);
    assert!(reloaded.preferences.iter().any(|p| p.active));

    // A writer holding an old revision conflicts, and nothing is written.
    let mut stale = data.clone();
    stale.profile.headline = Some("Changed".into());
    let conflict = store.save_profile(&stale, 1, &[]).await.unwrap_err();
    assert!(matches!(
        conflict,
        ProfileStorageError::Conflict {
            expected: 1,
            found: 3
        }
    ));
    assert_eq!(service.require().await.unwrap(), reloaded);

    // Claims by policy: a rejected claim is never usable.
    let usable = store
        .find_claims(
            reloaded.id(),
            &ClaimQuery {
                usable_only: true,
                ..ClaimQuery::default()
            },
        )
        .await
        .unwrap();
    assert!(usable.iter().all(|c| c.id != claim));
    let all = store
        .find_claims(reloaded.id(), &ClaimQuery::default())
        .await
        .unwrap();
    assert_eq!(all.len(), reloaded.claims.len());

    let events = store.profile_events(reloaded.id(), 10).await.unwrap();
    assert_eq!(
        events[0].kind,
        ProfileEventKind::StatementAdded,
        "newest first"
    );
    assert_eq!(
        events.last().unwrap().kind,
        ProfileEventKind::ResumeImported
    );
    assert!(events.last().unwrap().detail.contains("ana_lima.md"));
}

/// A profile fed by a resume, a LinkedIn export and a GitHub account
/// (BRU-309): origins, document kinds and corroborations survive a reload
/// on every backend, and so does taking a source out. Fictional data.
async fn evidence_sources_round_trip(store: &dyn Store) {
    use jobhunt_profile::{
        DocumentId, DocumentKind, GithubAccount, GithubRepo, GithubSnapshot, Origin, ParsedBasics,
        ParsedExperience, ParsedResume, ParsedSkillLine, SourceDocument,
    };
    let document = |kind: DocumentKind, seed: &str| SourceDocument {
        id: DocumentId::derive(&[seed]),
        kind,
        file_name: Some(format!("{seed}.txt")),
        sha256: format!("{:0>64}", seed.len()),
        pages: None,
        text: format!("text of {seed}"),
        parser: "test".into(),
        first_imported_at: at(0),
        last_imported_at: at(0),
    };
    let position = |header: &str| ParsedExperience {
        company: Some("Northwind Labs".into()),
        title: Some("Senior Software Engineer".into()),
        start: Some("2021-03".parse().unwrap()),
        current: true,
        header: header.into(),
        bullets: vec!["Built the settlement pipeline in Rust.".into()],
        ..ParsedExperience::default()
    };
    // Backends list records in their own order; compare them sorted.
    let sorted = |mut d: jobhunt_profile::ProfileData| {
        d.documents.sort_by_key(|x| x.id);
        d.experiences.sort_by_key(|x| x.id);
        d.projects.sort_by_key(|x| x.id);
        d.education.sort_by_key(|x| x.id);
        d.skills.sort_by_key(|x| x.id);
        d.claims.sort_by_key(|x| x.id);
        d
    };
    let service = ProfileService::new(store);
    service
        .import_resume(
            document(DocumentKind::Text, "resume"),
            &ParsedResume {
                experiences: vec![position("Northwind Labs — Senior Software Engineer")],
                basics: ParsedBasics {
                    headline: Some("Resume headline".into()),
                    ..ParsedBasics::default()
                },
                ..ParsedResume::default()
            },
            at(1),
        )
        .await
        .unwrap();
    service
        .import_linkedin(
            document(DocumentKind::Linkedin, "linkedin"),
            &ParsedResume {
                experiences: vec![ParsedExperience {
                    location: Some("Lisbon".into()),
                    ..position("Senior Software Engineer · Northwind Labs · Mar 2021 – Present")
                }],
                basics: ParsedBasics {
                    headline: Some("LinkedIn headline".into()),
                    location: Some("Lisbon".into()),
                    ..ParsedBasics::default()
                },
                skills: vec![ParsedSkillLine {
                    category: None,
                    skills: vec!["Kafka".into()],
                    line: "Skills: Kafka".into(),
                }],
                ..ParsedResume::default()
            },
            at(2),
        )
        .await
        .unwrap();
    let snapshot = GithubSnapshot {
        account: GithubAccount {
            login: "rileyx".into(),
            html_url: "https://github.com/rileyx".into(),
            kind: "User".into(),
            public_repos: 1,
            created_at: None,
        },
        repos: vec![GithubRepo {
            id: 7,
            name: "lox".into(),
            full_name: "rileyx/lox".into(),
            owner: "rileyx".into(),
            html_url: "https://github.com/rileyx/lox".into(),
            description: Some("A tiny interpreter".into()),
            fork: false,
            archived: false,
            is_template: false,
            private: false,
            size: 10,
            stars: 1,
            forks: 0,
            language: Some("Rust".into()),
            languages: Some(vec![("Rust".into(), 100)]),
            topics: vec![],
            created_at: Some(at(0)),
            pushed_at: Some(at(0)),
        }],
        orgs: vec![],
        fetched_at: at(3),
        problems: vec![],
    };
    let (_, data) = service.import_github(&snapshot, at(3)).await.unwrap();
    assert_eq!(
        sorted(service.require().await.unwrap()),
        sorted(data.clone()),
        "everything round-trips"
    );
    assert_eq!(data.profile.basic_sources.len(), 2);
    assert_eq!(data.experiences[0].meta.source_snapshots.len(), 2);
    let kinds: Vec<DocumentKind> = data.documents.iter().map(|d| d.kind).collect();
    assert!(kinds.contains(&DocumentKind::Linkedin) && kinds.contains(&DocumentKind::Github));
    let job = data
        .claims
        .iter()
        .find(|c| c.kind == jobhunt_profile::ClaimKind::Employment)
        .unwrap();
    assert_eq!(
        data.claim_sources(job),
        vec![Origin::Resume, Origin::Linkedin]
    );
    assert!(
        data.projects
            .iter()
            .all(|p| p.meta.origin == Origin::Github)
    );
    assert!(
        data.skills
            .iter()
            .any(|s| s.meta.origin == Origin::Linkedin)
    );

    let (_, removed) = service
        .remove_source(Origin::Linkedin, at(4))
        .await
        .unwrap();
    assert_eq!(
        sorted(service.require().await.unwrap()),
        sorted(removed.clone()),
        "removal round-trips"
    );
    assert!(
        removed
            .documents
            .iter()
            .all(|d| d.kind != DocumentKind::Linkedin)
    );
    assert_eq!(removed.profile.location, None);
    assert_eq!(removed.profile.headline.as_deref(), Some("Resume headline"));
    assert_eq!(removed.experiences[0].location, None);
    let events = store.profile_events(removed.id(), 3).await.unwrap();
    assert_eq!(events[0].kind, ProfileEventKind::SourceRemoved);
    assert_eq!(events[1].kind, ProfileEventKind::GithubImported);
    assert_eq!(events[2].kind, ProfileEventKind::LinkedinImported);
}

/// The values the structured Preferences controls write (BRU-308) survive
/// a reload on every backend, beside the older ones.
async fn structured_preference_values_round_trip(store: &dyn Store) {
    use jobhunt_profile::{PreferenceValue, Stance};
    let service = ProfileService::new(store);
    let values = [
        PreferenceValue::Relocation {
            willing: true,
            only_to: vec!["Portugal".into(), "the EU".into()],
        },
        PreferenceValue::UnknownPay { show: false },
        PreferenceValue::UnclearEligibility { show: true },
        PreferenceValue::Region {
            region: "Latin America".into(),
        },
    ];
    for (i, value) in values.iter().enumerate() {
        service
            .set_preference(value.clone(), Stance::Required, at(i as i64))
            .await
            .unwrap();
    }
    let data = service.require().await.unwrap();
    for value in &values {
        assert!(
            data.preferences
                .iter()
                .any(|p| p.active && &p.value == value),
            "{value:?}"
        );
    }
    let view = data.preferences();
    assert!(!view.shows_unknown_pay());
    assert!(view.shows_unclear_eligibility());
    // A newer answer replaces the older one (same key), which is kept.
    service
        .set_preference(
            PreferenceValue::Relocation {
                willing: false,
                only_to: Vec::new(),
            },
            Stance::Required,
            at(10),
        )
        .await
        .unwrap();
    let data = service.require().await.unwrap();
    let relocation: Vec<_> = data
        .preferences
        .iter()
        .filter(|p| p.value.key() == "relocation")
        .collect();
    assert_eq!(relocation.len(), 2);
    assert_eq!(relocation.iter().filter(|p| p.active).count(), 1);
}

async fn taste_profile_round_trips(store: &dyn Store) {
    use jobhunt_profile::taste::edit::{apply_reading, correct, remove, set_brief};
    use jobhunt_profile::taste::reading::ReadAssertion;
    use jobhunt_profile::taste::{
        InterpretationOutcome, Polarity, TasteConfidence, TasteDimension, TasteOrigin,
        TasteReading, TasteReview, TasteSource, compose,
    };
    let service = ProfileService::new(store);
    let read = |d, v: &str, p| ReadAssertion {
        dimension: d,
        value: v.into(),
        polarity: p,
        confidence: TasteConfidence::High,
        text: v.into(),
        explanation: Some("Read from your words.".into()),
        origin: TasteOrigin::Interpreted,
        sources: vec![TasteSource::Words {
            quote: "small teams".into(),
            statement: None,
        }],
    };
    let reading = TasteReading {
        interpreter: "rules/1".into(),
        assertions: vec![
            read(TasteDimension::Team, "small_team", Polarity::Prefer),
            read(TasteDimension::Company, "early_stage", Polarity::Prefer),
            read(TasteDimension::WorkShape, "ml_research", Polarity::Avoid),
        ],
        ambiguities: vec!["Which team size?".into()],
        ..TasteReading::default()
    };
    let (_, data) = service
        .change_taste(ProfileEventKind::TasteInterpreted, "read", at(1), |d| {
            set_brief(d, "small teams, early-stage, no ML research", None, at(1));
            apply_reading(
                d,
                &reading,
                InterpretationOutcome::Read,
                None,
                "digest".into(),
                at(1),
            );
            Ok(())
        })
        .await
        .unwrap();
    let p = compose(&data, &[]);
    let id = |key: &str| p.assertions.iter().find(|a| a.key() == key).unwrap().id;
    let (early, research) = (id("company:early_stage"), id("work_shape:ml_research"));
    service
        .change_taste(ProfileEventKind::TasteReviewed, "reviewed", at(2), |d| {
            correct(d, &p, early, Some(Polarity::Open), None, None, at(2))
                .map_err(|e| jobhunt_profile::ProfileError::Invalid(e.to_string()))?;
            remove(d, &p, research, at(2))
                .map_err(|e| jobhunt_profile::ProfileError::Invalid(e.to_string()))?;
            Ok(())
        })
        .await
        .unwrap();
    let stored = service.require().await.unwrap();
    assert_eq!(stored.taste.len(), 3);
    assert_eq!(
        stored.taste_brief.as_ref().map(|b| b.text.as_str()),
        Some("small teams, early-stage, no ML research")
    );
    let p = compose(&stored, &[]);
    let early = p.find(early).unwrap();
    assert_eq!(
        (early.polarity, early.review),
        (Polarity::Open, TasteReview::Corrected)
    );
    assert_eq!(p.removed.len(), 1);
    let history = service.history(10).await.unwrap();
    assert!(
        history
            .iter()
            .any(|e| e.kind == ProfileEventKind::TasteReviewed)
    );
    // Everything is kept on a plain save of the same aggregate.
    let (_, again) = service
        .change_taste(ProfileEventKind::TasteReviewed, "noop", at(3), |_| Ok(()))
        .await
        .unwrap();
    assert_eq!(again.taste, stored.taste);
    assert_eq!(again.taste_brief, stored.taste_brief);
}

async fn feedback_rankings_and_eligibility_cache(store: &dyn Store) {
    import_resume(store, "ana_lima.md", at(0)).await;
    let rust = posting("ramp", "1", "Senior Rust Engineer");
    let web = posting("ramp", "2", "Frontend Engineer");
    scan(store, "ramp", &[rust.clone(), web.clone()], at(1), true).await;
    let live = Live::new(&[rust.clone(), web.clone()]);
    let records = vec![
        store.get(rust.id()).await.unwrap().unwrap(),
        store.get(web.id()).await.unwrap().unwrap(),
    ];
    VerificationService::new(store, &live)
        .verify(&records, VerifyMode::Force, at(2))
        .await
        .unwrap();

    // Eligibility decisions are stored and reused while inputs are equal.
    let data = ProfileService::new(store).require().await.unwrap();
    let facts = ProfileFacts::from_profile(&data);
    let verified = cached(store, &records[..1]).await.unwrap();
    let policy = FreshnessPolicy::default();
    let (first, reused) = cached_assess(store, &verified, &facts, &policy, at(3))
        .await
        .unwrap();
    assert!(!reused);
    let (second, reused) = cached_assess(store, &verified, &facts, &policy, at(4))
        .await
        .unwrap();
    assert!(reused);
    assert_eq!(first.decision, second.decision);
    let key = CacheKey::of(&verified, &facts).unwrap();
    assert_eq!(
        store.cached_decision(&key).await.unwrap(),
        Some(first.decision)
    );

    // Many at once (what a search does): stored decisions are reused, new
    // ones computed and written together, and the answers are the ones the
    // one-at-a-time path gives.
    let both = vec![
        verified.clone(),
        cached(store, &records[1..]).await.unwrap(),
    ];
    let many = cached_assess_many(store, &both, &facts, &policy, at(4))
        .await
        .unwrap();
    assert_eq!(many.len(), 2);
    assert_eq!(many[0].decision, second.decision);
    let keys: Vec<CacheKey> = both
        .iter()
        .map(|v| CacheKey::of(v, &facts).unwrap())
        .collect();
    let stored = store.cached_decisions(&keys).await.unwrap();
    assert_eq!(stored.len(), 2);
    assert_eq!(stored.get(&keys[1].key), Some(&many[1].decision));
    let (single, reused) = cached_assess(store, &both[1], &facts, &policy, at(4))
        .await
        .unwrap();
    assert!(reused, "the batch stored what it computed");
    assert_eq!(single.decision, many[1].decision);
    // Writing the same key twice in one batch is an upsert, not an error.
    let twice = [
        (keys[1].clone(), many[1].decision.clone()),
        (keys[1].clone(), many[1].decision.clone()),
    ];
    store.store_decisions(&twice, at(5)).await.unwrap();
    assert!(store.cached_decisions(&[]).await.unwrap().is_empty());

    // Feedback: stored verbatim, found by record, folded into state.
    let ranking = RankingService::new(store, &RuleReader).with_policy(policy);
    let rejected = ranking
        .record(
            &records[1..],
            FeedbackAction::Reject,
            Some("too frontend-heavy"),
            at(5),
        )
        .await
        .unwrap();
    assert!(rejected.recorded);
    let again = ranking
        .record(
            &records[1..],
            FeedbackAction::Reject,
            Some("too frontend-heavy"),
            at(6),
        )
        .await
        .unwrap();
    assert!(!again.recorded, "a repeat is not stored twice");
    ranking
        .record(&records[..1], FeedbackAction::Save, None, at(7))
        .await
        .unwrap();
    let all = ranking.feedback().await.unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].reason.as_deref(), Some("too frontend-heavy"));
    assert_eq!(
        ranking.state(&records[1..]).await.unwrap().stage,
        Stage::Rejected
    );
    let pipeline = ranking.pipeline(false).await.unwrap();
    assert_eq!(pipeline.len(), 1);
    assert_eq!(pipeline[0].state.stage, Stage::Saved);

    // Ranking: the rejected job is excluded, the rest ranked and stored.
    let report = ranking
        .rank(
            &RankQuery {
                store_top: 5,
                all: true,
                ..RankQuery::default()
            },
            at(8),
        )
        .await
        .unwrap();
    assert_eq!(report.considered, 2);
    assert_eq!(report.excluded.rejected, 1);
    assert_eq!(report.rankings.len() + report.excluded.total(), 2);
    let explained = ranking.explain(&records[..1], at(8)).await.unwrap();
    assert!(matches!(
        explained.ranking.tier,
        Tier::StrongFit | Tier::WorthReviewing | Tier::Maybe | Tier::LowPriority
    ));
    let again = ranking.explain(&records[..1], at(8)).await.unwrap();
    assert!(again.reused, "a stored ranking is reused for equal inputs");
}

async fn fit_reviews_round_trip(store: &dyn Store) {
    use jobhunt_ranking::FitLevel;
    use jobhunt_ranking::review::{FitReview, ReviewPoint};
    let review = FitReview {
        fit: FitLevel::Poor,
        role_fit: "mismatch".into(),
        company_fit: "unknown".into(),
        seniority: "match".into(),
        specialization: "mismatch".into(),
        affirmative: Vec::new(),
        contradictions: vec![ReviewPoint {
            aspect: "specialization".into(),
            reason: "Storage-engine internals, beyond their work".into(),
            quote: "Develop the storage engine".into(),
        }],
        uncertainties: vec!["The team's size isn't stated".into()],
        reviewer: "model/anthropic:claude-opus-5-5".into(),
        rejected: 1,
        input_tokens: 2_400,
        output_tokens: 350,
    };
    assert_eq!(
        store.cached_fit_review("prof_a", "rev_1").await.unwrap(),
        None
    );
    store
        .store_fit_review("prof_a", "rev_1", &review, at(1))
        .await
        .unwrap();
    assert_eq!(
        store.cached_fit_review("prof_a", "rev_1").await.unwrap(),
        Some(review.clone())
    );
    assert_eq!(
        store.cached_fit_review("prof_b", "rev_1").await.unwrap(),
        None,
        "a review is one profile's"
    );
    // Stored again under the same key: replaced, not duplicated.
    let mut better = review.clone();
    better.fit = FitLevel::Plausible;
    store
        .store_fit_review("prof_a", "rev_1", &better, at(2))
        .await
        .unwrap();
    assert_eq!(
        store.cached_fit_review("prof_a", "rev_1").await.unwrap(),
        Some(better)
    );
}

async fn state_import_is_atomic_and_idempotent(store: &dyn Store) {
    import_resume(store, "ana_lima.md", at(0)).await;
    let a = posting("ramp", "1", "Engineer");
    scan(store, "ramp", std::slice::from_ref(&a), at(1), true).await;
    let record = store.get(a.id()).await.unwrap().unwrap();
    let ranking = RankingService::new(store, &RuleReader);
    ranking
        .record(
            std::slice::from_ref(&record),
            FeedbackAction::Applied,
            None,
            at(2),
        )
        .await
        .unwrap();
    let feedback = ranking.feedback().await.unwrap();
    let other = posting("elsewhere", "7", "Imported job");
    let mut imported = record.clone();
    imported.id = other.id();
    imported.posting = other;
    imported.opportunity_id = OpportunityId::founded_by(imported.id);
    let jobs = vec![record.clone(), imported.clone()];
    let first = store
        .import_state(StateImport {
            profile: None,
            jobs: &jobs,
            feedback: &feedback,
        })
        .await
        .unwrap();
    assert_eq!((first.jobs_added, first.jobs_present), (1, 1));
    assert_eq!((first.feedback_added, first.feedback_present), (0, 1));
    let second = store
        .import_state(StateImport {
            profile: None,
            jobs: &jobs,
            feedback: &feedback,
        })
        .await
        .unwrap();
    assert_eq!((second.jobs_added, second.feedback_added), (0, 0));
    assert_eq!(
        store.get(imported.id).await.unwrap().unwrap().posting,
        imported.posting
    );

    // A stale profile write fails the whole import: nothing is written.
    let data = ProfileService::new(store).require().await.unwrap();
    let third_job = {
        let p = posting("elsewhere", "8", "Never stored");
        let mut r = imported.clone();
        r.id = p.id();
        r.posting = p;
        r.opportunity_id = OpportunityId::founded_by(r.id);
        r
    };
    let failed = store
        .import_state(StateImport {
            profile: Some(jobhunt_storage::ProfileWrite {
                data: &data,
                expected_revision: 99,
                events: &[],
            }),
            jobs: std::slice::from_ref(&third_job),
            feedback: &[],
        })
        .await;
    assert!(failed.is_err());
    assert!(store.get(third_job.id).await.unwrap().is_none());
}

async fn write_lock_serializes_writers(store: &dyn Store) {
    let guard = store.write_lock().await.unwrap();
    let acquired = tokio::time::timeout(StdDuration::from_millis(300), store.write_lock()).await;
    assert!(acquired.is_err(), "a second writer waits");
    drop(guard);
    let again = tokio::time::timeout(StdDuration::from_secs(10), store.write_lock()).await;
    assert!(again.is_ok(), "the lock is released on drop");
}

/// The same scenario gives the same answers on both backends.
#[tokio::test]
async fn backends_agree_on_a_whole_scenario() {
    async fn run(store: &dyn Store) -> Vec<String> {
        import_resume(store, "ana_lima.md", at(0)).await;
        let mut postings = vec![
            posting("ramp", "1", "Senior Rust Engineer"),
            posting("ramp", "2", "Frontend Engineer"),
            posting("ramp", "3", "Backend Engineer, Payments"),
        ];
        postings[2].location = Some("San Francisco, CA (on-site)".into());
        postings[2].workplace_type = Some(jobhunt_jobs::WorkplaceType::OnSite);
        postings[2].is_remote = Some(false);
        scan(store, "ramp", &postings, at(1), true).await;
        let live = Live::new(&postings);
        let mut records = Vec::new();
        for p in &postings {
            records.push(store.get(p.id()).await.unwrap().unwrap());
        }
        VerificationService::new(store, &live)
            .verify(&records, VerifyMode::Force, at(2))
            .await
            .unwrap();
        let ranking = RankingService::new(store, &RuleReader);
        ranking
            .record(
                &records[1..2],
                FeedbackAction::Reject,
                Some("frontend"),
                at(3),
            )
            .await
            .unwrap();
        let report = ranking
            .rank(
                &RankQuery {
                    store_top: 5,
                    all: true,
                    ..RankQuery::default()
                },
                at(4) + Duration::hours(1),
            )
            .await
            .unwrap();
        let mut out: Vec<String> = report
            .rankings
            .iter()
            .map(|r| format!("{} {:?} {:?}", r.opportunity, r.tier, r.gate))
            .collect();
        out.push(format!("{:?}", report.excluded));
        out
    }
    let local = run(sqlite().await.as_ref()).await;
    assert!(!local.is_empty());
    let Some(fixture) = PgFixture::new().await else {
        return;
    };
    let store: Arc<dyn Store> = Arc::new(fixture.user("scenario").await);
    let cloud = run(store.as_ref()).await;
    assert_eq!(local, cloud);
    drop(store);
    fixture.finish().await;
}
