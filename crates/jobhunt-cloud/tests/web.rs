//! BRU-295 over real HTTP and a real Postgres: the Today feed, feedback
//! from the web, the pipeline, preferences and learned taste, the profile
//! (resume upload, claim review, LinkedIn and GitHub evidence), email
//! notifications through the outbox
//! (with an in-memory provider: no test sends real email), and isolation
//! between accounts.
//!
//! Needs `JOBHUNT_TEST_DATABASE_URL` (skipped without it).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use chrono::{DateTime, Duration, TimeZone, Utc};
use jobhunt_app::feed::FeedView;
use jobhunt_cloud::api::{ApiState, router};
use jobhunt_cloud::auth::DevVerifier;
use jobhunt_cloud::config::CloudConfig;
use jobhunt_cloud::email::{EmailSender, MemoryMode, MemorySender};
use jobhunt_cloud::notify;
use jobhunt_cloud::usage::UsageLog;
use jobhunt_core::{CanonicalUrl, IngestCounts, Provenance, SourceKey};
use jobhunt_jobs::verification::{
    ApplicationCheck, Authority, CompensationCheck, ListingStatus, PublishedPlace,
    VERIFICATION_REVISION, VerificationId, VerificationMethod, VerificationRecord,
    VerificationRepository,
};
use jobhunt_jobs::{
    Compensation, CompensationComponent, CompensationKind, JobPosting, JobRepository,
    OpportunityId, PayInterval, ScanBody, ScanWrite, WorkplaceType,
};
use jobhunt_storage::postgres::testing::TestDatabase;
use jobhunt_storage::postgres::{Keyring, PgSettings, PgStore};
use reqwest::StatusCode;
use serde_json::{Value, json};

const KEY: &str = "k1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
const DEV_SECRET: &str = "0123456789abcdef0123456789abcdef";
const WEB: &str = "https://app.jobhunt.test";

struct Server {
    base: String,
    db: TestDatabase,
    store: PgStore,
    config: Arc<CloudConfig>,
    mail: MemorySender,
    http: reqwest::Client,
    task: tokio::task::JoinHandle<()>,
    _dir: tempfile::TempDir,
}

impl Server {
    async fn start() -> Option<Self> {
        Self::start_with(&[]).await
    }

    /// With extra environment (`JOBHUNT_GITHUB_ENDPOINT` for a mock).
    async fn start_with(extra: &[(&str, String)]) -> Option<Self> {
        let db = TestDatabase::create().await?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let dir = tempfile::tempdir().unwrap();
        let vars: HashMap<String, String> = [
            ("DATABASE_URL", db.url()),
            ("JOBHUNT_ENCRYPTION_KEYS", KEY.to_owned()),
            ("JOBHUNT_PUBLIC_URL", base.clone()),
            ("JOBHUNT_ENV", "test".to_owned()),
            ("JOBHUNT_AUTH_MODE", "dev".to_owned()),
            ("JOBHUNT_AUTH_DEV_SECRET", DEV_SECRET.to_owned()),
            ("JOBHUNT_WEB_URL", WEB.to_owned()),
            // The address emails come from; the sender itself is replaced by
            // an in-memory one below.
            ("JOBHUNT_EMAIL_PROVIDER", "file".to_owned()),
            (
                "JOBHUNT_EMAIL_FILE",
                dir.path().join("unused.jsonl").display().to_string(),
            ),
            (
                "JOBHUNT_EMAIL_FROM",
                "JobHunt <notify@jobhunt.test>".to_owned(),
            ),
        ]
        .into_iter()
        .chain(extra.iter().cloned())
        .map(|(k, v)| (k.to_owned(), v))
        .collect();
        let config = Arc::new(CloudConfig::from_env(&move |name: &str| {
            vars.get(name).cloned()
        }));
        assert!(
            config.problems(jobhunt_cloud::Role::Server).is_empty(),
            "{:?}",
            config.problems(jobhunt_cloud::Role::Server)
        );
        assert!(
            config.problems(jobhunt_cloud::Role::Notifier).is_empty(),
            "{:?}",
            config.problems(jobhunt_cloud::Role::Notifier)
        );
        let store = PgStore::connect(
            &db.url(),
            &PgSettings::default(),
            Keyring::parse(KEY).unwrap(),
        )
        .await
        .unwrap();
        store.migrate().await.unwrap();
        let mail = MemorySender::new();
        let (usage, _) = UsageLog::start(store.clone());
        let state = ApiState::with_email(
            store.clone(),
            Arc::clone(&config),
            usage,
            Some(Arc::new(mail.clone()) as Arc<dyn EmailSender>),
        )
        .unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, router(state)).await.unwrap();
        });
        Some(Self {
            base,
            db,
            store,
            config,
            mail,
            http: reqwest::Client::new(),
            task,
            _dir: dir,
        })
    }

    async fn token(&self, subject: &str) -> String {
        let r: Value = self
            .http
            .post(format!("{}/api/v1/auth/dev-token", self.base))
            .json(&json!({ "subject": subject }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        r["access_token"].as_str().unwrap().to_owned()
    }

    async fn call(
        &self,
        method: reqwest::Method,
        route: &str,
        token: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut request = self
            .http
            .request(method, format!("{}{route}", self.base))
            .bearer_auth(token)
            .header("x-jobhunt-client", "web");
        if let Some(b) = body {
            request = request.json(&b);
        }
        let response = request.send().await.unwrap();
        let status = response.status();
        let text = response.text().await.unwrap();
        let value = if text.is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text).unwrap_or(Value::String(text))
        };
        (status, value)
    }

    async fn get(&self, route: &str, token: &str) -> (StatusCode, Value) {
        self.call(reqwest::Method::GET, route, token, None).await
    }

    async fn post(&self, route: &str, token: &str, body: Value) -> (StatusCode, Value) {
        self.call(reqwest::Method::POST, route, token, Some(body))
            .await
    }

    async fn put(&self, route: &str, token: &str, body: Value) -> (StatusCode, Value) {
        self.call(reqwest::Method::PUT, route, token, Some(body))
            .await
    }

    async fn upload(&self, token: &str, file: &str, content_type: &str) -> (StatusCode, Value) {
        let bytes = std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../jobhunt-resume/tests/fixtures")
                .join(file),
        )
        .unwrap();
        let response = self
            .http
            .put(format!(
                "{}/api/v1/profile/resume?file_name={file}",
                self.base
            ))
            .bearer_auth(token)
            .header("content-type", content_type)
            .body(bytes)
            .send()
            .await
            .unwrap();
        let status = response.status();
        (status, response.json().await.unwrap())
    }

    async fn feed(&self, token: &str, limit: usize) -> FeedView {
        let (status, body) = self
            .get(&format!("/api/v1/feed?limit={limit}"), token)
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        serde_json::from_value(body).unwrap()
    }

    /// Resume and preferences, through the API (the web's onboarding).
    async fn onboard(&self, token: &str) {
        let (status, body) = self.upload(token, "ana_lima.md", "text/markdown").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let (status, body) = self
            .post(
                "/api/v1/preferences",
                token,
                json!({"statement": "I want backend and platform roles at small product \
                    teams, remote, at least USD 120k. No pure SRE roles."}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }

    /// Discovers postings from `source` into the shared corpus (as the
    /// worker would), each verified active at its employer's ATS `hours`
    /// ago when given. Returns their opportunity ids.
    async fn seed(
        &self,
        source: &str,
        postings: &[JobPosting],
        verified_hours: Option<i64>,
    ) -> Vec<OpportunityId> {
        self.seed_at(
            source,
            postings,
            verified_hours,
            Utc::now() - Duration::hours(2),
        )
        .await
    }

    /// Like [`Server::seed`], observed at `when` (a later scan of the same
    /// postings is a change, recorded in their history at `when`).
    async fn seed_at(
        &self,
        source: &str,
        postings: &[JobPosting],
        verified_hours: Option<i64>,
        when: DateTime<Utc>,
    ) -> Vec<OpportunityId> {
        let source: SourceKey = source.parse().unwrap();
        let run = self.store.begin_run(when).await.unwrap();
        self.store
            .apply_scan(&ScanWrite {
                run,
                source: &source,
                started_at: when,
                observed_at: when,
                counts: IngestCounts::default(),
                body: ScanBody::Listing {
                    postings,
                    complete: false,
                    close_missing: false,
                    closing_withheld: None,
                    retain: &[],
                    validator: None,
                },
            })
            .await
            .unwrap();
        let mut ids = Vec::new();
        for p in postings {
            let record = self.store.get(p.id()).await.unwrap().unwrap();
            if let Some(hours) = verified_hours {
                self.verify(&record, hours).await;
            }
            ids.push(record.opportunity_id);
        }
        ids
    }

    async fn verify(&self, record: &jobhunt_jobs::JobRecord, hours: i64) {
        let at = Utc::now() - Duration::hours(hours);
        self.store
            .save_verification(&VerificationRecord {
                id: VerificationId::derive(record.id, at),
                job_id: record.id,
                opportunity_id: record.opportunity_id,
                source: record.posting.provenance.source.clone(),
                source_record_id: record.posting.provenance.source_record_id.clone(),
                attempted_at: at,
                method: VerificationMethod::GreenhouseJobApi,
                listing: ListingStatus::Active,
                application: ApplicationCheck::unknown("not checked in tests"),
                authority: Authority::EmployerConfiguredAts,
                authority_chain: Vec::new(),
                checked_url: None,
                listing_url: None,
                content_fingerprint: None,
                changed_fields: Vec::new(),
                changed_since_last_verification: None,
                lifecycle: None,
                compensation: CompensationCheck::observe(
                    record.posting.compensation.as_ref(),
                    None,
                ),
                published: PublishedPlace::default(),
                unknowns: Vec::new(),
                failure: None,
                revision: VERIFICATION_REVISION.to_owned(),
            })
            .await
            .unwrap();
    }

    async fn pool(&self) -> sqlx::PgPool {
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&self.db.url())
            .await
            .unwrap()
    }

    /// Time passing: what the feed showed, the jobs' history and the
    /// person's feedback all look `hours` older.
    async fn age_feed(&self, hours: i64) {
        let pool = self.pool().await;
        for sql in [
            "UPDATE user_opportunities SET first_shown_at = first_shown_at - make_interval(hours => $1), \
             last_shown_at = last_shown_at - make_interval(hours => $1), \
             resurfaced_at = resurfaced_at - make_interval(hours => $1), \
             dismissed_at = dismissed_at - make_interval(hours => $1)",
            "UPDATE job_events SET at = at - make_interval(hours => $1)",
            "UPDATE feedback SET recorded_at = recorded_at - make_interval(hours => $1)",
        ] {
            sqlx::query(sql)
                .bind(i32::try_from(hours).unwrap())
                .execute(&pool)
                .await
                .unwrap();
        }
        pool.close().await;
    }

    async fn notify(&self) -> notify::NotifySummary {
        notify::notify(&self.store, &self.config, &self.mail)
            .await
            .unwrap()
    }

    async fn finish(self) {
        self.task.abort();
        self.store.close().await;
        self.db.drop_database().await;
    }
}

fn salary(min: f64, max: f64) -> Compensation {
    Compensation {
        summary: None,
        components: vec![CompensationComponent {
            kind: CompensationKind::Salary,
            label: None,
            currency: Some("USD".into()),
            min: Some(min),
            max: Some(max),
            interval: Some(PayInterval::Year),
        }],
    }
}

fn posting(
    source: &str,
    id: &str,
    company: &str,
    title: &str,
    description: &str,
    pay: Option<Compensation>,
) -> JobPosting {
    let key: SourceKey = source.parse().unwrap();
    JobPosting {
        url: CanonicalUrl::parse(&format!(
            "https://job-boards.greenhouse.io/{}/jobs/{id}",
            key.instance()
        ))
        .unwrap(),
        provenance: Provenance {
            source: key,
            source_record_id: Some(id.into()),
            fetched_from: None,
        },
        apply_url: None,
        company: company.into(),
        title: title.into(),
        department: Some("Engineering".into()),
        team: None,
        location: Some("Remote - Worldwide".into()),
        locations: Vec::new(),
        employment_type: None,
        workplace_type: Some(WorkplaceType::Remote),
        is_remote: Some(true),
        compensation: pay,
        work_authorization: None,
        description_text: Some(description.into()),
        description_html: None,
        posted_at: Some(Utc.with_ymd_and_hms(2026, 9, 20, 0, 0, 0).unwrap()),
        source_updated_at: None,
    }
}

const BACKEND: &str = "We are a small product team of 12 engineers building developer tools \
    for payments. You will own backend services in Go and PostgreSQL, with Kafka. Fully remote.";
const PLATFORM: &str = "Small team building the platform for developer tooling: Go, \
    Kubernetes, PostgreSQL. You will own our internal CLI and developer platform. Remote.";
const SRE: &str = "Site reliability engineering: on-call rotation, incident response, \
    Kubernetes and Terraform operations. Pure SRE role, 24/7 on-call.";
const MARKETING: &str = "Own our brand campaigns and marketing calendar. Content, SEO and \
    events. No engineering.";

/// Jobs for Ana (a Go backend engineer who wants small product teams,
/// remote, at least USD 120k): three strong fits and a developer-tools role
/// only worth reviewing (`strong[3]`), and jobs that are not worth her time.
fn corpus() -> (Vec<JobPosting>, Vec<JobPosting>) {
    let strong = vec![
        posting(
            "greenhouse:ledgerly",
            "101",
            "Ledgerly",
            "Senior Backend Engineer (Go)",
            BACKEND,
            Some(salary(150_000.0, 190_000.0)),
        ),
        posting(
            "greenhouse:toolbox",
            "201",
            "Toolbox",
            "Staff Platform Engineer",
            PLATFORM,
            Some(salary(170_000.0, 210_000.0)),
        ),
        posting(
            "greenhouse:paydev",
            "301",
            "PayDev",
            "Backend Engineer, Payments APIs",
            BACKEND,
            Some(salary(140_000.0, 175_000.0)),
        ),
        posting(
            "greenhouse:cliworks",
            "401",
            "CLI Works",
            "Senior Software Engineer, Developer Tools",
            PLATFORM,
            Some(salary(160_000.0, 200_000.0)),
        ),
    ];
    let weak = vec![
        posting(
            "greenhouse:opsco",
            "501",
            "OpsCo",
            "Site Reliability Engineer",
            SRE,
            Some(salary(150_000.0, 180_000.0)),
        ),
        posting(
            "greenhouse:brandly",
            "601",
            "Brandly",
            "Marketing Manager",
            MARKETING,
            None,
        ),
    ];
    (strong, weak)
}

fn ids(feed: &FeedView) -> Vec<String> {
    feed.items.iter().map(|i| i.item.id.clone()).collect()
}

fn tier(item: &jobhunt_app::feed::FeedItem) -> String {
    serde_json::to_value(item.item.tier)
        .unwrap()
        .as_str()
        .unwrap()
        .to_owned()
}

async fn seed_each(server: &Server, postings: &[JobPosting], verified: Option<i64>) -> Vec<String> {
    let mut out = Vec::new();
    for p in postings {
        let ids = server
            .seed(
                &p.provenance.source.to_string(),
                std::slice::from_ref(p),
                verified,
            )
            .await;
        out.push(ids[0].to_string());
    }
    out
}

#[tokio::test]
async fn an_ambiguous_onboarding_answer_stays_unresolved_until_confirmed() {
    let Some(server) = Server::start().await else {
        return;
    };
    let token = server.token("bruno").await;
    let (status, body) = server.upload(&token, "ana_lima.md", "text/markdown").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = server
        .post(
            "/api/v1/preferences",
            &token,
            json!({"statement": "Small teams and the sallary of 140k"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let questions = |profile: &Value| -> Vec<Value> {
        profile["preferences"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|p| p["active"] == true && !p["clarify"].is_null())
            .cloned()
            .collect()
    };
    let (_, profile) = server.get("/api/v1/profile", &token).await;
    let open = questions(&profile);
    assert_eq!(open.len(), 2, "{profile}");
    let pay = open.iter().find(|p| p["clarify"]["kind"] == "pay").unwrap();
    assert_eq!(pay["clarify"]["amount"], 140_000);
    assert_eq!(pay["clarify"]["bound"], "target");
    assert!(pay["clarify"]["currency"].is_null(), "never assumed");
    let size = open
        .iter()
        .find(|p| p["clarify"]["kind"] == "size")
        .unwrap();
    assert_eq!(size["clarify"]["value"], "small_team");

    // The answers replace the readings: a USD floor, and small teams as a
    // requirement.
    let (status, body) = server
        .post(
            "/api/v1/preferences",
            &token,
            json!({
                "set": [
                    {"kind": "compensation", "minimum": 140_000, "currency": "USD", "period": "year"},
                    {"kind": "company", "company": "small_team", "stance": "require"},
                ],
                "remove": [pay["id"], size["id"]],
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, profile) = server.get("/api/v1/profile", &token).await;
    assert!(questions(&profile).is_empty(), "{profile}");
    let active: Vec<(String, String)> = profile["preferences"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["active"] == true)
        .map(|p| {
            (
                p["stance"].as_str().unwrap().to_owned(),
                p["value"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert!(
        active
            .iter()
            .any(|(stance, value)| stance == "required" && value.contains("USD 140,000")),
        "{active:?}"
    );
    assert!(
        active
            .iter()
            .any(|(stance, value)| stance == "required" && value == "small teams"),
        "{active:?}"
    );
    server.finish().await;
}

#[tokio::test]
async fn the_taste_profile_reads_once_with_the_configured_model() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let model = MockServer::start().await;
    let reading = json!({
        "summary": "Small teams.",
        "preferences": [
            {"dimension": "team", "value": "small_team", "polarity": "prefer", "confidence": "high",
             "statement": "Small technical teams", "explanation": "Said so.",
             "sources": [{"kind": "words", "ref": "small technical teams"}]},
            {"dimension": "seniority", "value": "staff_plus", "polarity": "prefer", "confidence": "high",
             "statement": "Staff-plus roles", "explanation": "Latest title.",
             "sources": [{"kind": "profile", "ref": "e1"}]}
        ],
        "ambiguities": [],
        "constraints_noted": ["remote"]
    });
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"index": 0, "finish_reason": "stop",
                         "message": {"role": "assistant", "content": reading.to_string()}}]
        })))
        .mount(&model)
        .await;
    let Some(server) = Server::start_with(&[
        ("JOBHUNT_AI_PROVIDER", "openai".to_owned()),
        ("JOBHUNT_AI_MODEL", "test-model".to_owned()),
        ("JOBHUNT_AI_BASE_URL", format!("{}/v1", model.uri())),
    ])
    .await
    else {
        return;
    };
    let token = server.token("taste").await;
    let (status, body) = server.upload(&token, "ana_lima.md", "text/markdown").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let words = "Small technical teams, backend work. Remote.";
    let (status, body) = server
        .post(
            "/api/v1/taste/profile",
            &token,
            json!({"action": "describe", "text": words}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["interpreted"], true);
    let profile = &body["profile"];
    assert_eq!(profile["reader"]["name"], "model/openai:test-model");
    assert_eq!(
        profile["interpretation"]["interpreter"],
        "model/openai:test-model"
    );
    let understood = profile["understood"].to_string();
    assert!(understood.contains("Small technical teams"), "{profile}");
    assert!(
        understood.contains("Inferred from your profile"),
        "{profile}"
    );
    // Remote is a constraint, read deterministically from the same words.
    assert!(
        profile["constraints"].to_string().contains("remote")
            || profile["constraints"].to_string().contains("Remote"),
        "{profile}"
    );
    // What the model was sent: the words and a normalized career, nothing
    // personal.
    let sent = model.received_requests().await.unwrap();
    assert_eq!(sent.len(), 1);
    let prompt = String::from_utf8_lossy(&sent[0].body).to_string();
    assert!(prompt.contains("Small technical teams, backend work"));
    for private in [
        "Ana Lima",
        "ana@example.org",
        "Acme Payments",
        "Lisbon",
        "analima",
    ] {
        assert!(!prompt.contains(private), "{private} was sent");
    }
    // Reading it never calls the model; confirming doesn't either.
    let (status, view) = server.get("/api/v1/taste/profile", &token).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    let (status, confirmed) = server
        .post(
            "/api/v1/taste/profile",
            &token,
            json!({"action": "confirm"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{confirmed}");
    assert_eq!(confirmed["profile"]["needs_confirmation"], false);
    assert_eq!(model.received_requests().await.unwrap().len(), 1);
    // Invalid actions are refused, with a reason the person can act on.
    let (status, error) = server
        .post(
            "/api/v1/taste/profile",
            &token,
            json!({"action": "describe", "text": "  "}),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
    assert_eq!(error["error"]["code"], "invalid_arguments");
    // Another account sees nothing of it.
    let other = server.token("someone-else").await;
    let (status, _) = server.get("/api/v1/taste/profile", &other).await;
    assert_ne!(status, StatusCode::OK);
    server.finish().await;
}

#[tokio::test]
async fn today_shows_one_role_per_company() {
    let Some(server) = Server::start().await else {
        return;
    };
    let token = server.token("ana").await;
    server.onboard(&token).await;
    // Four roles at one company (three strong fits, and a developer-tools
    // role only worth reviewing), and two strong ones elsewhere.
    let supa: Vec<JobPosting> = [
        ("701", "Senior Backend Engineer (Go)"),
        ("702", "Backend Engineer, Payments APIs"),
        ("703", "Staff Platform Engineer"),
        ("704", "Senior Software Engineer, Developer Tools"),
    ]
    .iter()
    .map(|(id, title)| {
        posting(
            "greenhouse:supa",
            id,
            "Supa",
            title,
            BACKEND,
            Some(salary(150_000.0, 190_000.0)),
        )
    })
    .collect();
    server.seed("greenhouse:supa", &supa, Some(1)).await;
    let (strong, _) = corpus();
    seed_each(&server, &strong[..2], Some(1)).await;

    let feed = server.feed(&token, 5).await;
    let companies: Vec<&str> = feed.items.iter().map(|i| i.item.company.as_str()).collect();
    assert_eq!(
        companies.iter().filter(|c| **c == "Supa").count(),
        1,
        "{companies:?}"
    );
    assert_eq!(
        feed.items.len(),
        3,
        "three companies, not padded: {companies:?}"
    );
    let first = feed
        .items
        .iter()
        .find(|i| i.item.company == "Supa")
        .unwrap();
    assert_eq!(
        first.also_at_company.len(),
        2,
        "the other strong Supa roles go with it, not the weaker one"
    );
    assert!(
        first
            .also_at_company
            .iter()
            .all(|o| o.id != first.item.id && !ids(&feed).contains(&o.id))
    );
    assert_eq!(feed.summary.new, 5, "held back is still new");

    // Held back is not shown: a day later the Supa role on the feed has
    // passed, and one of the others takes its place, new.
    server.age_feed(25).await;
    let later = server.feed(&token, 5).await;
    let next = later
        .items
        .iter()
        .find(|i| i.item.company == "Supa")
        .expect("another Supa role, never shown before");
    assert_ne!(next.item.id, first.item.id);
    assert!(first.also_at_company.iter().any(|o| o.id == next.item.id));
    assert_eq!(next.also_at_company.len(), 1);
    assert!(
        next.first_shown_at.is_none(),
        "first shown by this feed, not before"
    );
    server.finish().await;
}

#[tokio::test]
async fn today_is_a_small_feed_of_new_recommendations_that_can_be_finished() {
    let Some(server) = Server::start().await else {
        return;
    };
    let token = server.token("ana").await;
    // Before onboarding: a stable code the web turns into onboarding.
    let (status, body) = server.get("/api/v1/feed", &token).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], "no_profile");
    server.onboard(&token).await;
    let (strong, weak) = corpus();
    let strong_ids = seed_each(&server, &strong, Some(1)).await;
    let weak_ids = seed_each(&server, &weak, Some(1)).await;

    // Three strong fits; the developer-tools role is only worth reviewing
    // (close to the platform work she wants), so it stays in search.
    let feed = server.feed(&token, 3).await;
    assert_eq!(feed.items.len(), 3, "a small shortlist");
    assert!(!feed.caught_up);
    assert_eq!(feed.summary.checked, 6);
    assert_eq!(feed.summary.worth_reviewing, 4);
    assert_eq!(feed.summary.strong_fits, 3);
    assert_eq!(feed.summary.new, 3);
    assert_eq!(feed.summary.shown, 3);
    assert!(!ids(&feed).contains(&strong_ids[3]), "no maybes on Today");
    for item in &feed.items {
        assert_eq!(tier(item), "strong_fit", "{}", item.item.title);
        assert!(!weak_ids.contains(&item.item.id), "no maybes on Today");
        assert!(!item.item.why.is_empty(), "every item says why");
        assert_eq!(item.item.compensation.status, "published");
        assert!(
            item.item.compensation.verified,
            "pay comes from the verification"
        );
        assert!(item.item.verification.trusted);
        assert!(!item.item.locations.is_empty());
    }
    // Reloading does not make them vanish or change.
    let again = server.feed(&token, 3).await;
    assert_eq!(ids(&again), ids(&feed));

    // Deal with them: reject one with a reason, save one, and apply to the
    // job worth reviewing she found in search.
    let [rejected, saved] = [&feed.items[0], &feed.items[1]].map(|i| i.item.id.clone());
    let applied = strong_ids[3].clone();
    let (status, fb) = server
        .post(
            &format!("/api/v1/opportunities/{rejected}/feedback"),
            &token,
            json!({"action": "reject", "reason": "too corporate for me"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{fb}");
    assert_eq!(fb["state"]["stage"], "rejected");
    assert_eq!(fb["interpretation"]["reason"], "too corporate for me");
    for (id, action) in [(&saved, "save"), (&applied, "applied")] {
        let (status, fb) = server
            .post(
                &format!("/api/v1/opportunities/{id}/feedback"),
                &token,
                json!({ "action": action }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{fb}");
    }

    // What is left: the one recommendation not dealt with yet.
    let feed = server.feed(&token, 3).await;
    assert_eq!(feed.items.len(), 1, "{:?}", ids(&feed));
    let last = feed.items[0].item.id.clone();
    assert!(strong_ids.contains(&last));
    assert!(![&rejected, &saved, &applied].contains(&&last));
    assert_eq!(feed.pipeline.saved, 1);
    assert_eq!(feed.pipeline.applied, 1);

    // "Not now" puts it aside without teaching anything.
    let (status, fb) = server
        .post(
            &format!("/api/v1/opportunities/{last}/dismiss"),
            &token,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{fb}");
    assert_eq!(fb["taste_changed"], false);
    let feed = server.feed(&token, 3).await;
    assert!(feed.caught_up, "caught up: {:?}", ids(&feed));
    assert!(feed.items.is_empty(), "never padded with maybes");
    assert_eq!(feed.summary.checked, 6, "the corpus is still there");
    assert_eq!(feed.summary.new, 0);

    // The pipeline: saved and applied; rejected only when asked.
    let (_, pipeline) = server.get("/api/v1/pipeline", &token).await;
    let stages: Vec<&str> = pipeline["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["stage"].as_str().unwrap())
        .collect();
    assert_eq!(stages, ["applied", "saved"]);
    let (_, all) = server
        .get("/api/v1/pipeline?include_rejected=true", &token)
        .await;
    assert_eq!(all["entries"].as_array().unwrap().len(), 3);
    // Moving through the pipeline: interview, then offer.
    for action in ["interview", "offer"] {
        let (status, fb) = server
            .post(
                &format!("/api/v1/opportunities/{applied}/feedback"),
                &token,
                json!({ "action": action }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{fb}");
    }
    let (_, pipeline) = server.get("/api/v1/pipeline", &token).await;
    assert_eq!(pipeline["entries"][0]["stage"], "offer");

    // Applied and rejected jobs are never recommended as new again, by the
    // feed or by search.
    let (_, found) = server
        .post(
            "/api/v1/search",
            &token,
            json!({"limit": 10, "verify": false}),
        )
        .await;
    let found: Vec<&str> = found["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert!(!found.contains(&applied.as_str()) && !found.contains(&rejected.as_str()));
    assert!(found.contains(&saved.as_str()), "saved jobs stay findable");

    // What JobHunt believes: stated preferences apart from what it learned,
    // and the reason kept verbatim.
    let (status, taste) = server.get("/api/v1/taste", &token).await;
    assert_eq!(status, StatusCode::OK, "{taste}");
    assert!(!taste["stated"].as_array().unwrap().is_empty());
    assert_eq!(taste["feedback_events"], 5);
    assert!(taste.to_string().contains("too corporate for me"));
    for list in ["learned", "contradictory", "covered_by_stated", "emerging"] {
        for l in taste[list].as_array().unwrap() {
            assert!(!l["support"].as_array().unwrap().is_empty(), "{l}");
        }
    }

    // Usage: product actions, tagged with the client, never the words.
    tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
    let pool = server.pool().await;
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT event, metadata::text FROM usage_events ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
    let events: Vec<&str> = rows.iter().map(|(e, _)| e.as_str()).collect();
    for expected in [
        "feed_opened",
        "feedback",
        "dismiss",
        "resume_imported",
        "preferences",
    ] {
        assert!(events.contains(&expected), "{expected} in {events:?}");
    }
    for (_, metadata) in &rows {
        assert!(!metadata.contains("too corporate"), "{metadata}");
        assert!(!metadata.contains("ana_lima"), "{metadata}");
    }
    assert!(
        rows.iter()
            .any(|(e, m)| e == "feed_opened" && m.contains("\"client\": \"web\""))
    );
    pool.close().await;
    server.finish().await;
}

#[tokio::test]
async fn passed_over_items_leave_and_material_changes_bring_them_back() {
    let Some(server) = Server::start().await else {
        return;
    };
    let token = server.token("ana").await;
    server.onboard(&token).await;
    let (strong, _) = corpus();
    let ids_all = seed_each(&server, &strong[..3], Some(1)).await;

    let first = server.feed(&token, 1).await;
    assert_eq!(first.items.len(), 1);
    let a = first.items[0].item.id.clone();
    let a_posting = strong[ids_all.iter().position(|i| *i == a).unwrap()].clone();

    // A day later, unacted: it was passed over; the next one shows.
    server.age_feed(25).await;
    let next = server.feed(&token, 1).await;
    assert_eq!(next.passed_over, 1);
    let b = next.items[0].item.id.clone();
    assert_ne!(a, b);

    // Pay published higher on A: a material change brings it back.
    let mut raised = a_posting.clone();
    raised.compensation = Some(salary(180_000.0, 230_000.0));
    server
        .seed_at(
            &raised.provenance.source.to_string(),
            std::slice::from_ref(&raised),
            None,
            Utc::now(),
        )
        .await;
    let feed = server.feed(&token, 3).await;
    let back = feed
        .items
        .iter()
        .find(|i| i.item.id == a)
        .expect("the changed job is back");
    assert_eq!(serde_json::to_value(back.reason).unwrap(), json!("changed"));
    assert!(
        back.changes.iter().any(|c| c.starts_with("Pay changed")),
        "{:?}",
        back.changes
    );
    assert_eq!(feed.summary.changed, 1);

    // Saved B: saving takes it off Today (it lives in Applications).
    let (status, _) = server
        .post(
            &format!("/api/v1/opportunities/{b}/feedback"),
            &token,
            json!({"action": "save"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let feed = server.feed(&token, 3).await;
    assert!(!ids(&feed).contains(&b));
    assert_eq!(feed.pipeline.saved, 1);

    // A cosmetic edit (the description) does not bring B back...
    let b_posting = strong[ids_all.iter().position(|i| *i == b).unwrap()].clone();
    let mut edited = b_posting.clone();
    edited.description_text = Some(format!("{BACKEND} We also have a nice office dog."));
    server
        .seed_at(
            &edited.provenance.source.to_string(),
            std::slice::from_ref(&edited),
            None,
            Utc::now() + Duration::seconds(1),
        )
        .await;
    let feed = server.feed(&token, 3).await;
    assert!(!ids(&feed).contains(&b), "cosmetic changes don't resurface");
    // ...its pay being published higher does, even though it is saved.
    let mut moved = edited.clone();
    moved.compensation = Some(salary(190_000.0, 240_000.0));
    server
        .seed_at(
            &moved.provenance.source.to_string(),
            std::slice::from_ref(&moved),
            None,
            Utc::now() + Duration::seconds(2),
        )
        .await;
    let feed = server.feed(&token, 3).await;
    let b_back = feed.items.iter().find(|i| i.item.id == b);
    assert!(b_back.is_some(), "{:?}", ids(&feed));
    assert_eq!(
        b_back.unwrap().stage,
        jobhunt_app::views::PipelineStage::Saved
    );

    // Resurfaced and let pass for a day: not shown again for that change.
    server.age_feed(25).await;
    let feed = server.feed(&token, 3).await;
    assert!(!ids(&feed).contains(&a), "{:?}", ids(&feed));
    server.finish().await;
}

#[tokio::test]
async fn a_job_listed_by_two_sources_appears_once() {
    let Some(server) = Server::start().await else {
        return;
    };
    let token = server.token("ana").await;
    server.onboard(&token).await;
    let (strong, _) = corpus();
    let original = &strong[0];
    let opps = server
        .seed(
            "greenhouse:ledgerly",
            std::slice::from_ref(original),
            Some(1),
        )
        .await;
    let mut copy = original.clone();
    copy.provenance.source = "ashby:ledgerly".parse().unwrap();
    copy.url = CanonicalUrl::parse("https://jobs.ashbyhq.com/ledgerly/abc-101").unwrap();
    copy.apply_url = Some(original.url.clone());
    server
        .seed("ashby:ledgerly", std::slice::from_ref(&copy), Some(1))
        .await;
    // Identity grouping (discovery's job) puts both records in one
    // opportunity.
    server
        .store
        .assign_opportunities(&[(copy.id(), opps[0])])
        .await
        .unwrap();
    let feed = server.feed(&token, 5).await;
    let matching: Vec<_> = feed
        .items
        .iter()
        .filter(|i| i.item.title == original.title)
        .collect();
    assert_eq!(matching.len(), 1, "{:?}", ids(&feed));
    assert_eq!(matching[0].item.sources, 2);
    assert_eq!(feed.summary.checked, 1);
    // Details list both sources as provenance.
    let (_, detail) = server
        .get(
            &format!("/api/v1/opportunities/{}?include_sources=true", opps[0]),
            &token,
        )
        .await;
    assert_eq!(detail["sources"].as_array().unwrap().len(), 2);
    server.finish().await;
}

#[tokio::test]
async fn profile_resume_upload_and_claim_review() {
    let Some(server) = Server::start().await else {
        return;
    };
    let token = server.token("ana").await;
    let (status, body) = server.get("/api/v1/profile/claims", &token).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], "no_profile");

    let (status, imported) = server.upload(&token, "ana_lima.md", "text/markdown").await;
    assert_eq!(status, StatusCode::OK, "{imported}");
    assert_eq!(imported["first_import"], true);
    assert!(imported["experiences"]["added"].as_u64().unwrap() >= 3);
    assert_eq!(imported["profile"]["documents"][0]["current"], true);
    assert!(
        !imported["profile"]["projects"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        !imported["profile"]["education"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        !imported.to_string().contains("ana@example.org"),
        "contact details never leave through the profile"
    );

    let (status, review) = server.get("/api/v1/profile/claims", &token).await;
    assert_eq!(status, StatusCode::OK, "{review}");
    let total = review["total"].as_u64().unwrap();
    assert!(total >= 2, "{review}");
    let claims = review["claims"].as_array().unwrap();
    for c in claims {
        assert!(!c["why"].as_str().unwrap().is_empty());
        assert!(!c["provenance"].as_str().unwrap().is_empty());
    }
    let confirm = claims[0]["id"].as_str().unwrap().to_owned();
    let reject = claims[1]["id"].as_str().unwrap().to_owned();
    let (status, decided) = server
        .post(
            "/api/v1/profile/claims",
            &token,
            json!({"ids": [confirm], "decision": "confirm"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{decided}");
    assert_eq!(decided["decided"][0]["usable"], true);
    let (_, decided) = server
        .post(
            "/api/v1/profile/claims",
            &token,
            json!({"ids": [reject], "decision": "reject", "note": "never did that"}),
        )
        .await;
    assert_eq!(decided["decided"][0]["state"], "rejected");
    assert_eq!(decided["needs_review_total"].as_u64().unwrap(), total - 2);

    // Re-importing keeps the person's decisions.
    let (_, again) = server.upload(&token, "ana_lima.md", "text/markdown").await;
    assert_eq!(again["same_file"], true);
    let (_, v2) = server
        .upload(&token, "ana_lima_v2.md", "text/markdown")
        .await;
    assert_eq!(v2["first_import"], false);
    let (_, review) = server.get("/api/v1/profile/claims", &token).await;
    assert!(
        !review["claims"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["id"] == reject.as_str()),
        "a rejected claim stays rejected across re-imports"
    );
    let (_, profile) = server.get("/api/v1/profile", &token).await;
    assert_eq!(profile["documents"].as_array().unwrap().len(), 2);

    // Garbage is refused with a stable code, not a server error.
    let response = server
        .http
        .put(format!(
            "{}/api/v1/profile/resume?file_name=resume.pdf",
            server.base
        ))
        .bearer_auth(&token)
        .body("not a pdf at all")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], "invalid_arguments");

    // An expired session is a 401 the web turns into "sign in again".
    let expired = DevVerifier::new(DEV_SECRET.into())
        .mint("ana", chrono::Duration::minutes(-10))
        .unwrap();
    let (status, body) = server.get("/api/v1/feed", &expired).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["code"], "unauthenticated");
    server.finish().await;
}

/// LinkedIn and GitHub evidence through the API (BRU-309): a LinkedIn CSV
/// upload beside the resume, a GitHub import against a mock of GitHub's
/// API, and taking both out again. Fictional data only.
#[tokio::test]
async fn profile_linkedin_and_github_evidence() {
    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let github = MockServer::start().await;
    Mock::given(path("/users/analima"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "login": "analima", "html_url": "https://github.com/analima",
            "type": "User", "public_repos": 1
        })))
        .mount(&github)
        .await;
    Mock::given(path("/users/analima/repos"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
            "id": 7, "name": "ledgerlint", "full_name": "analima/ledgerlint",
            "owner": {"login": "analima"}, "html_url": "https://github.com/analima/ledgerlint",
            "description": "A linter for double-entry ledgers", "fork": false,
            "archived": false, "private": false, "size": 10, "stargazers_count": 3,
            "forks_count": 0, "language": "Go", "topics": [],
            "created_at": "2022-01-01T00:00:00Z", "pushed_at": "2026-09-01T00:00:00Z"
        }])))
        .mount(&github)
        .await;
    Mock::given(path("/users/analima/orgs"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&github)
        .await;
    let Some(server) = Server::start_with(&[("JOBHUNT_GITHUB_ENDPOINT", github.uri())]).await
    else {
        return;
    };
    let token = server.token("ana").await;
    let (status, _) = server.upload(&token, "ana_lima.md", "text/markdown").await;
    assert_eq!(status, StatusCode::OK);

    // LinkedIn: one CSV of the export is enough.
    let positions = "Company Name,Title,Description,Location,Started On,Finished On
        Acme Payments,Staff Software Engineer,,Remote,Jan 2021,
        Initech,Backend Engineer,,,Jan 2015,May 2016
";
    let upload = |bytes: &'static str, name: &'static str| {
        server
            .http
            .put(format!(
                "{}/api/v1/profile/linkedin?file_name={name}",
                server.base
            ))
            .bearer_auth(&token)
            .header("x-jobhunt-client", "web")
            .body(bytes)
            .send()
    };
    let response = upload(positions, "Positions.csv").await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let imported: Value = response.json().await.unwrap();
    assert_eq!(imported["source"], "linkedin");
    assert_eq!(imported["first_import"], true);
    assert_eq!(imported["experiences"]["added"], 1);
    assert_eq!(imported["experiences"]["corroborated"], 1);
    let acme = imported["profile"]["experiences"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["title"] == "Staff Software Engineer")
        .unwrap();
    assert_eq!(acme["sources"], json!(["resume", "linkedin"]));
    let sources: Vec<&str> = imported["profile"]["documents"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["current"] == true)
        .map(|d| d["source"].as_str().unwrap())
        .collect();
    assert_eq!(sources.len(), 2, "the current resume and the export");
    let again: Value = upload(positions, "Positions.csv")
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(again["unchanged"], true);
    assert_eq!(again["experiences"]["added"], 0);
    // Not an export: a stable code, and nothing changes.
    let response = upload("hello", "notes.txt").await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], "invalid_arguments");

    // GitHub: the account the resume links, through the mock.
    let (status, gh) = server
        .post("/api/v1/profile/github", &token, json!({}))
        .await;
    assert_eq!(status, StatusCode::OK, "{gh}");
    assert_eq!(gh["label"], "github.com/analima");
    assert_eq!(gh["projects"]["corroborated"], 1, "the resume's ledgerlint");
    let (_, review) = server.get("/api/v1/profile/claims", &token).await;
    let recent = review["claims"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["text"] == "Recent hands-on Go work in public GitHub repositories")
        .expect("the inference waits for review");
    assert_eq!(recent["provenance"], "inferred");
    assert_eq!(recent["evidence"][0]["source"], "github");
    let (status, err) = server
        .post(
            "/api/v1/profile/github",
            &token,
            json!({"username": "not a login"}),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{err}");

    // Removal.
    let (status, removed) = server
        .call(
            reqwest::Method::DELETE,
            "/api/v1/profile/sources/linkedin",
            &token,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{removed}");
    assert_eq!(removed["records_deleted"], 1, "Initech");
    let (_, profile) = server.get("/api/v1/profile", &token).await;
    assert!(
        profile["experiences"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["company"] != "Initech")
    );
    let (status, _) = server
        .call(
            reqwest::Method::DELETE,
            "/api/v1/profile/sources/resume",
            &token,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = server
        .call(
            reqwest::Method::DELETE,
            "/api/v1/profile/sources/github",
            &token,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    // Another account sees none of it.
    let other = server.token("bruno").await;
    let (status, _) = server.get("/api/v1/profile", &other).await;
    assert_eq!(status, StatusCode::CONFLICT);
    server.finish().await;
}

/// Turns email notifications on for `address` and follows the link.
async fn subscribe(server: &Server, token: &str, address: &str) {
    let before = server.mail.sent().len();
    let (status, settings) = server
        .put(
            "/api/v1/notifications",
            token,
            json!({"email_enabled": true, "email": address}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{settings}");
    assert_eq!(settings["email_status"], "unconfirmed");
    let sent = server.mail.sent();
    assert_eq!(sent.len(), before + 1, "a confirmation link");
    let link = &sent[before];
    assert_eq!(link.to, address);
    let token_param = link
        .text
        .split("token=")
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_owned();
    let (status, settings) = server
        .post(
            "/api/v1/notifications/confirm",
            token,
            json!({ "token": token_param }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{settings}");
    assert_eq!(settings["email_status"], "confirmed");
}

/// Makes the person's daily digest due now (UTC, every day, no quiet
/// hours, its time a few minutes ago), through the API.
async fn digest_due_now(server: &Server, token: &str) {
    let now = Utc::now();
    let midnight = now.date_naive().and_time(chrono::NaiveTime::MIN).and_utc();
    let at = (now - Duration::minutes(20)).max(midnight);
    let (status, body) = server
        .put(
            "/api/v1/notifications",
            token,
            json!({
                "timezone": "UTC",
                "digest_time": at.format("%H:%M").to_string(),
                "digest_days": ["mon", "tue", "wed", "thu", "fri", "sat", "sun"],
                "quiet_start": "00:00",
                "quiet_end": "00:00",
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// A day passes for every account: today's digest slot is free again.
async fn next_day(server: &Server) {
    let pool = server.pool().await;
    sqlx::query("UPDATE notification_settings SET last_digest_on = last_digest_on - 1")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
}

fn recommendation_emails(server: &Server) -> Vec<jobhunt_cloud::email::EmailMessage> {
    server
        .mail
        .sent()
        .into_iter()
        .filter(|m| !m.subject.starts_with("Confirm"))
        .collect()
}

#[tokio::test]
async fn strong_new_recommendations_are_emailed_once_and_nothing_else_is() {
    let Some(server) = Server::start().await else {
        return;
    };
    let token = server.token("ana").await;
    server.onboard(&token).await;

    // Not confirmed yet: nothing is sent to the address.
    let (_, settings) = server
        .put(
            "/api/v1/notifications",
            &token,
            json!({"email_enabled": true, "email": "ana@old.example.com"}),
        )
        .await;
    assert_eq!(settings["email_status"], "unconfirmed");
    let (status, body) = server
        .post(
            "/api/v1/notifications/confirm",
            &token,
            json!({"token": "not-the-token"}),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "invalid_confirmation");
    let (strong, weak) = corpus();
    seed_each(&server, &weak, Some(1)).await;
    seed_each(&server, &strong, Some(1)).await;
    let summary = server.notify().await;
    assert_eq!(
        summary.accounts, 0,
        "unconfirmed addresses are not notified"
    );
    assert!(recommendation_emails(&server).is_empty());

    // Another address needs its own confirmation; then it is confirmed.
    subscribe(&server, &token, "ana@example.com").await;
    digest_due_now(&server, &token).await;

    // The person already rejected one strong fit and applied to another.
    let ids = seed_each(&server, &strong[..2], Some(1)).await;
    let (_, fb) = server
        .post(
            &format!("/api/v1/opportunities/{}/feedback", ids[0]),
            &token,
            json!({"action": "reject", "reason": "not for me"}),
        )
        .await;
    assert_eq!(fb["state"]["stage"], "rejected");
    server
        .post(
            &format!("/api/v1/opportunities/{}/feedback", ids[1]),
            &token,
            json!({"action": "applied"}),
        )
        .await;
    // A strong fit whose listing was never verified is not "verified
    // enough to recommend".
    let unverified = posting(
        "greenhouse:unverified",
        "901",
        "Unverified Inc",
        "Senior Backend Engineer",
        BACKEND,
        Some(salary(160_000.0, 200_000.0)),
    );
    seed_each(&server, std::slice::from_ref(&unverified), None).await;

    let summary = server.notify().await;
    assert_eq!(
        (summary.accounts, summary.composed, summary.sent),
        (1, 1, 1),
        "{summary:?}"
    );
    let emails = recommendation_emails(&server);
    assert_eq!(emails.len(), 1);
    let email = &emails[0];
    assert_eq!(email.to, "ana@example.com");
    // Only the strong fits left: Payments APIs and Staff Platform... minus
    // the rejected and applied ones.
    let expected: Vec<&str> = strong[2..3].iter().map(|p| p.title.as_str()).collect();
    for title in &expected {
        assert!(email.text.contains(title), "{title} in {}", email.text);
    }
    for absent in [
        strong[0].title.as_str(),
        strong[1].title.as_str(),
        strong[3].title.as_str(), // worth reviewing: not strong
        weak[0].title.as_str(),   // maybe
        weak[1].title.as_str(),
        "Unverified Inc",
    ] {
        assert!(!email.text.contains(absent), "{absent} in {}", email.text);
    }
    assert_eq!(email.subject, "Narrow found 1 job worth a look");
    assert!(email.text.contains(&format!("{WEB}/opportunities/")));
    assert!(email.text.contains("Why Narrow picked it:"));
    assert!(email.html.contains("Why Narrow picked it:"));
    assert!(email.text.contains("USD"), "pay when known");
    // Found when its board was first read: its age is the board's date,
    // never "new".
    assert!(email.text.contains("Posted ") && email.text.contains(" days ago"));
    assert!(!email.text.to_lowercase().contains("new"), "{}", email.text);
    assert!(email.headers.iter().any(|(k, _)| k == "List-Unsubscribe"));

    // The cursor moved. Today's digest was sent: whatever appears now
    // waits for tomorrow's.
    let (_, account) = server.get("/api/v1/account", &token).await;
    assert!(account["cloud"]["notification_cursor"].as_u64().unwrap() > 0);
    let summary = server.notify().await;
    assert_eq!((summary.composed, summary.not_due), (0, 1), "{summary:?}");
    // Nothing new the next day means no email (no "nothing found").
    next_day(&server).await;
    let summary = server.notify().await;
    assert_eq!(
        (summary.composed, summary.nothing_new),
        (0, 1),
        "{summary:?}"
    );
    // Found nothing, the day's slot is still used: nothing later today.
    let summary = server.notify().await;
    assert_eq!((summary.composed, summary.not_due), (0, 1), "{summary:?}");
    assert_eq!(recommendation_emails(&server).len(), 1);

    // Several new strong fits at once are grouped into one email.
    let more = vec![
        posting(
            "greenhouse:alpha",
            "1001",
            "Alpha",
            "Senior Backend Engineer",
            BACKEND,
            Some(salary(150_000.0, 190_000.0)),
        ),
        posting(
            "greenhouse:beta",
            "1002",
            "Beta",
            "Backend Engineer, Ledger",
            BACKEND,
            Some(salary(150_000.0, 190_000.0)),
        ),
        posting(
            "greenhouse:gamma",
            "1003",
            "Gamma",
            "Platform Engineer",
            PLATFORM,
            Some(salary(150_000.0, 190_000.0)),
        ),
        posting(
            "greenhouse:delta",
            "1004",
            "Delta",
            "Backend Engineer, Payouts",
            BACKEND,
            Some(salary(150_000.0, 190_000.0)),
        ),
    ];
    seed_each(&server, &more, Some(1)).await;
    let summary = server.notify().await;
    assert_eq!(summary.composed, 0, "{summary:?}");
    next_day(&server).await;
    let summary = server.notify().await;
    assert_eq!(summary.sent, 1, "{summary:?}");
    let emails = recommendation_emails(&server);
    assert_eq!(emails.len(), 2);
    assert_eq!(emails[1].subject, "Narrow found 3 jobs worth a look");
    assert!(emails[1].text.contains("\n1. ") && emails[1].text.contains("\n3. "));
    assert_eq!(summary.opportunities, 3, "at most three per digest");
    // The fourth waits for the next digest.
    next_day(&server).await;
    let summary = server.notify().await;
    assert_eq!(summary.opportunities, 1, "{summary:?}");

    // A strong fit the person already saw in the app is not emailed.
    let seen = posting(
        "greenhouse:omega",
        "1100",
        "Omega",
        "Senior Backend Engineer",
        BACKEND,
        Some(salary(150_000.0, 190_000.0)),
    );
    seed_each(&server, std::slice::from_ref(&seen), Some(1)).await;
    let feed = server.feed(&token, 10).await;
    assert!(feed.items.iter().any(|i| i.item.company == "Omega"));
    next_day(&server).await;
    let summary = server.notify().await;
    assert_eq!(
        (summary.composed, summary.nothing_new),
        (0, 1),
        "{summary:?}"
    );

    // Turned off: nothing more, whatever appears.
    server
        .put(
            "/api/v1/notifications",
            &token,
            json!({"email_enabled": false}),
        )
        .await;
    seed_each(
        &server,
        &[posting(
            "greenhouse:zeta",
            "1200",
            "Zeta",
            "Senior Backend Engineer",
            BACKEND,
            Some(salary(150_000.0, 190_000.0)),
        )],
        Some(1),
    )
    .await;
    let summary = server.notify().await;
    assert_eq!(summary.accounts, 0);
    let (_, settings) = server.get("/api/v1/notifications", &token).await;
    assert!(settings["recent"].as_array().unwrap().len() >= 3);
    server.finish().await;
}

#[tokio::test]
async fn the_digest_keeps_its_schedule_one_company_at_a_time_and_leaves_today_alone() {
    let Some(server) = Server::start().await else {
        return;
    };
    let token = server.token("ana").await;
    server.onboard(&token).await;
    subscribe(&server, &token, "ana@example.com").await;

    // Existing subscribers get the defaults: 3 jobs at 09:00 on weekdays,
    // quiet from 21:00 to 08:00, in UTC until a timezone is set.
    let (_, settings) = server.get("/api/v1/notifications", &token).await;
    assert_eq!(settings["digest_time"], "09:00");
    assert_eq!(
        settings["digest_days"],
        json!(["mon", "tue", "wed", "thu", "fri"])
    );
    assert_eq!(settings["digest_size"], 3);
    assert_eq!(
        (
            settings["quiet_start"].as_str(),
            settings["quiet_end"].as_str()
        ),
        (Some("21:00"), Some("08:00"))
    );
    assert!(settings.get("timezone").is_none());
    assert_eq!(settings["max_items"], 5);
    assert!(settings["next_digest_at"].is_string(), "{settings}");
    assert!(settings.get("min_interval_hours").is_none());

    // Settings that could never be honored are refused, not adjusted.
    for (body, error) in [
        (json!({"digest_time": "07:00"}), "inside your quiet hours"),
        (
            json!({"quiet_start": "08:30", "quiet_end": "10:00"}),
            "inside your quiet hours",
        ),
        (json!({"timezone": "Mars/Olympus"}), "IANA"),
        (json!({"digest_size": 6}), "between 1 and 5"),
        (json!({"digest_days": []}), "at least one day"),
        (json!({"digest_days": ["someday"]}), "not a day"),
        (json!({"digest_time": "9am"}), "a time like 09:00"),
    ] {
        let (status, answer) = server
            .put("/api/v1/notifications", &token, body.clone())
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body} {answer}");
        assert!(answer.to_string().contains(error), "{body}: {answer}");
    }
    let (_, unchanged) = server.get("/api/v1/notifications", &token).await;
    assert_eq!(unchanged["digest_time"], "09:00");

    digest_due_now(&server, &token).await;
    let (_, settings) = server.get("/api/v1/notifications", &token).await;
    assert_eq!(settings["timezone"], "UTC");
    let railway: Vec<JobPosting> = (1..=3)
        .map(|n| {
            posting(
                "greenhouse:railway",
                &format!("70{n}"),
                "Railway",
                &format!("Backend Engineer {n}"),
                BACKEND,
                Some(salary(150_000.0, 190_000.0)),
            )
        })
        .collect();
    let ando = posting(
        "greenhouse:ando",
        "801",
        "Ando Technologies",
        "Senior Backend Engineer",
        BACKEND,
        Some(salary(150_000.0, 190_000.0)),
    );
    server.seed("greenhouse:railway", &railway, Some(1)).await;
    seed_each(&server, std::slice::from_ref(&ando), Some(1)).await;

    // One job per company: three Railway roles make one item.
    let summary = server.notify().await;
    assert_eq!((summary.sent, summary.opportunities), (1, 2), "{summary:?}");
    let email = recommendation_emails(&server).pop().unwrap();
    assert_eq!(email.subject, "Narrow found 2 jobs worth a look");
    assert_eq!(
        email.text.matches(". Railway\n").count(),
        1,
        "{}",
        email.text
    );
    assert!(email.text.contains(". Ando Technologies\n"));
    let pool = server.pool().await;
    let items: Vec<(i16, String)> =
        sqlx::query_as("SELECT position, freshness FROM notification_items ORDER BY position")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        items,
        [(1, "backfill".to_owned()), (2, "backfill".to_owned())]
    );

    // Emailing them did not consume Today: both are still new there.
    let feed = server.feed(&token, 10).await;
    for company in ["Railway", "Ando Technologies"] {
        let item = feed
            .items
            .iter()
            .find(|i| i.item.company == company)
            .unwrap();
        assert_eq!(item.reason, jobhunt_app::feed::FeedReason::New, "{company}");
    }

    // The next days, one job each: Railway, in last week's digest, gives
    // way to a company that wasn't; then it is sent again, not blocked.
    let (status, _) = server
        .put("/api/v1/notifications", &token, json!({"digest_size": 1}))
        .await;
    assert_eq!(status, StatusCode::OK);
    let veda = posting(
        "greenhouse:veda",
        "901",
        "Veda",
        "Backend Engineer",
        BACKEND,
        Some(salary(150_000.0, 190_000.0)),
    );
    seed_each(&server, std::slice::from_ref(&veda), Some(1)).await;
    next_day(&server).await;
    server.notify().await;
    let email = recommendation_emails(&server).pop().unwrap();
    assert_eq!(email.subject, "Narrow found 1 job worth a look");
    assert!(email.text.contains("1. Veda\n"), "{}", email.text);
    next_day(&server).await;
    server.notify().await;
    let email = recommendation_emails(&server).pop().unwrap();
    assert!(email.text.contains("1. Railway\n"), "{}", email.text);
    assert_eq!(recommendation_emails(&server).len(), 3);

    // Paused: nothing; resumed: the schedule goes on, nothing caught up.
    let until = Utc::now() + Duration::days(7);
    let (status, settings) = server
        .put(
            "/api/v1/notifications",
            &token,
            json!({"paused_until": until}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{settings}");
    assert!(settings["paused_until"].is_string());
    let next: DateTime<Utc> = settings["next_digest_at"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!(next >= until - Duration::seconds(1), "{settings}");
    next_day(&server).await;
    let summary = server.notify().await;
    assert_eq!((summary.paused, summary.composed), (1, 0), "{summary:?}");
    let (status, settings) = server
        .put(
            "/api/v1/notifications",
            &token,
            json!({"paused_until": null}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{settings}");
    assert!(settings.get("paused_until").is_none(), "{settings}");
    let summary = server.notify().await;
    assert_eq!(summary.paused, 0, "{summary:?}");
    assert_eq!(summary.sent, 1, "the last Railway role: {summary:?}");
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notification_items")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(total, 5, "each opportunity once, ever");
    pool.close().await;
    server.finish().await;
}

#[tokio::test]
async fn provider_failures_are_retried_without_sending_twice() {
    let Some(server) = Server::start().await else {
        return;
    };
    let token = server.token("ana").await;
    server.onboard(&token).await;
    subscribe(&server, &token, "ana@example.com").await;
    digest_due_now(&server, &token).await;
    let (strong, _) = corpus();
    seed_each(&server, &strong[..1], Some(1)).await;
    let pool = server.pool().await;
    let make_due = || async {
        sqlx::query(
            "UPDATE notification_deliveries SET next_attempt_at = now() - interval '1 minute'",
        )
        .execute(&pool)
        .await
        .unwrap();
    };
    let cursor = || async {
        let (_, account) = server.get("/api/v1/account", &token).await;
        account["cloud"]["notification_cursor"].as_u64().unwrap()
    };

    // The provider is down: nothing is marked sent, the cursor stays.
    server.mail.set_mode(MemoryMode::FailRetryable);
    let summary = server.notify().await;
    assert_eq!(
        (summary.composed, summary.sent, summary.retrying),
        (1, 0, 1),
        "{summary:?}"
    );
    assert!(recommendation_emails(&server).is_empty());
    assert_eq!(cursor().await, 0);
    // A run before the retry is due neither sends nor composes another.
    server.mail.set_mode(MemoryMode::Accept);
    let summary = server.notify().await;
    assert_eq!((summary.composed, summary.sent), (0, 0), "{summary:?}");
    // Due: the same email goes out once.
    make_due().await;
    let summary = server.notify().await;
    assert_eq!(summary.sent, 1, "{summary:?}");
    assert_eq!(recommendation_emails(&server).len(), 1);
    assert!(cursor().await > 0);

    // The provider accepts but the answer is lost (a crash, a dropped
    // connection): the retry reuses the idempotency key, so the person gets
    // one email.
    seed_each(&server, &strong[1..2], Some(1)).await;
    next_day(&server).await;
    server.mail.set_mode(MemoryMode::AcceptThenLoseAnswer);
    let summary = server.notify().await;
    assert_eq!(summary.retrying, 1, "{summary:?}");
    assert_eq!(
        recommendation_emails(&server).len(),
        2,
        "the provider has it"
    );
    server.mail.set_mode(MemoryMode::Accept);
    make_due().await;
    let summary = server.notify().await;
    assert_eq!(summary.sent, 1, "{summary:?}");
    assert_eq!(recommendation_emails(&server).len(), 2, "not sent twice");
    // Retries never duplicate what a digest covered.
    let items: Vec<(i64, i64)> =
        sqlx::query_as("SELECT COUNT(*), COUNT(DISTINCT opportunity_id) FROM notification_items")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(items, [(2, 2)]);

    // A permanent refusal fails the delivery (never "sent").
    seed_each(&server, &strong[2..3], Some(1)).await;
    next_day(&server).await;
    server.mail.set_mode(MemoryMode::FailPermanent);
    let summary = server.notify().await;
    assert_eq!(summary.failed, 1, "{summary:?}");
    let status: String =
        sqlx::query_scalar("SELECT status FROM notification_deliveries ORDER BY seq DESC LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "failed");

    // Retries give up after too long rather than sending stale news late.
    server.mail.set_mode(MemoryMode::FailRetryable);
    next_day(&server).await;
    seed_each(
        &server,
        &[posting(
            "greenhouse:late",
            "1300",
            "Late",
            "Senior Backend Engineer",
            BACKEND,
            Some(salary(150_000.0, 190_000.0)),
        )],
        Some(1),
    )
    .await;
    server.notify().await;
    sqlx::query(
        "UPDATE notification_deliveries SET created_at = now() - interval '21 hours', \
         next_attempt_at = now() - interval '1 minute' WHERE status = 'pending'",
    )
    .execute(&pool)
    .await
    .unwrap();
    server.mail.set_mode(MemoryMode::Accept);
    let summary = server.notify().await;
    assert_eq!(summary.abandoned, 1, "{summary:?}");
    assert!(
        !recommendation_emails(&server)
            .iter()
            .any(|m| m.text.contains("Late"))
    );

    // A digest the provider has not accepted by its deadline (quiet hours,
    // the end of the day) is abandoned, not sent late.
    seed_each(
        &server,
        &[posting(
            "greenhouse:quiet",
            "1400",
            "Quiet",
            "Senior Backend Engineer",
            BACKEND,
            Some(salary(150_000.0, 190_000.0)),
        )],
        Some(1),
    )
    .await;
    next_day(&server).await;
    server.mail.set_mode(MemoryMode::FailRetryable);
    server.notify().await;
    let expires: Option<DateTime<Utc>> = sqlx::query_scalar(
        "SELECT expires_at FROM notification_deliveries WHERE status = 'pending'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        expires.is_some_and(|e| e > Utc::now()),
        "a digest has a deadline"
    );
    sqlx::query(
        "UPDATE notification_deliveries SET expires_at = now() - interval '1 minute', \
         next_attempt_at = now() - interval '1 minute' WHERE status = 'pending'",
    )
    .execute(&pool)
    .await
    .unwrap();
    server.mail.set_mode(MemoryMode::Accept);
    let summary = server.notify().await;
    assert_eq!(summary.abandoned, 1, "{summary:?}");
    assert!(
        !recommendation_emails(&server)
            .iter()
            .any(|m| m.text.contains("Quiet"))
    );
    pool.close().await;
    server.finish().await;
}

#[tokio::test]
async fn concurrent_workers_send_one_email() {
    let Some(server) = Server::start().await else {
        return;
    };
    let token = server.token("ana").await;
    server.onboard(&token).await;
    subscribe(&server, &token, "ana@example.com").await;
    digest_due_now(&server, &token).await;
    let (strong, _) = corpus();
    seed_each(&server, &strong, Some(1)).await;
    let (a, b, c) = tokio::join!(server.notify(), server.notify(), server.notify());
    assert_eq!(a.sent + b.sent + c.sent, 1, "{a:?} {b:?} {c:?}");
    assert_eq!(recommendation_emails(&server).len(), 1);
    let pool = server.pool().await;
    let items: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notification_items")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(items, 3);
    pool.close().await;
    server.finish().await;
}

#[tokio::test]
async fn notifications_and_feeds_never_cross_accounts() {
    let Some(server) = Server::start().await else {
        return;
    };
    let ana = server.token("ana").await;
    let bob = server.token("bob").await;
    server.onboard(&ana).await;
    server.onboard(&bob).await;
    subscribe(&server, &ana, "ana@example.com").await;
    subscribe(&server, &bob, "bob@example.com").await;
    digest_due_now(&server, &ana).await;
    digest_due_now(&server, &bob).await;
    let (strong, _) = corpus();
    let ids = seed_each(&server, &strong[..3], Some(1)).await;
    // Ana rejects the platform job and saves another; Bob does nothing.
    let platform = ids[1].clone();
    server
        .post(
            &format!("/api/v1/opportunities/{platform}/feedback"),
            &ana,
            json!({"action": "reject", "reason": "ana's private reason"}),
        )
        .await;
    server
        .post(
            &format!("/api/v1/opportunities/{}/feedback", ids[0]),
            &ana,
            json!({"action": "save"}),
        )
        .await;

    let summary = server.notify().await;
    assert_eq!(summary.sent, 2, "{summary:?}");
    let emails = recommendation_emails(&server);
    let to_ana: Vec<_> = emails
        .iter()
        .filter(|m| m.to == "ana@example.com")
        .collect();
    let to_bob: Vec<_> = emails
        .iter()
        .filter(|m| m.to == "bob@example.com")
        .collect();
    assert_eq!((to_ana.len(), to_bob.len()), (1, 1));
    assert!(
        !to_ana[0].text.contains(&strong[1].title),
        "ana rejected it"
    );
    assert!(
        !to_ana[0].text.contains(&strong[0].title),
        "ana saved it already"
    );
    assert!(to_bob[0].text.contains(&strong[1].title), "bob didn't");
    assert!(to_bob[0].text.contains(&strong[0].title));
    for m in &emails {
        assert!(!m.text.contains("private reason"));
    }

    // Feeds, settings and deliveries are per account.
    let bob_feed = server.feed(&bob, 5).await;
    assert_eq!(bob_feed.pipeline.saved, 0);
    assert!(
        bob_feed
            .items
            .iter()
            .all(|i| i.stage == jobhunt_app::views::PipelineStage::Unseen)
    );
    let (_, bob_settings) = server.get("/api/v1/notifications", &bob).await;
    assert_eq!(bob_settings["email"], "bob@example.com");
    let (_, ana_settings) = server.get("/api/v1/notifications", &ana).await;
    let ana_deliveries: Vec<&str> = ana_settings["recent"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["id"].as_str().unwrap())
        .collect();
    for d in bob_settings["recent"].as_array().unwrap() {
        assert!(!ana_deliveries.contains(&d["id"].as_str().unwrap()));
    }
    let (_, bob_taste) = server.get("/api/v1/taste", &bob).await;
    assert!(!bob_taste.to_string().contains("private reason"));
    // Bob cannot put Ana's email on his account's confirmation either: a
    // token only confirms the account it was sent for.
    let pool = server.pool().await;
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(DISTINCT user_id) FROM notification_items")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows, 2);
    pool.close().await;
    server.finish().await;
}
