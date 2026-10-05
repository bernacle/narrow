//! Company discovery end to end over HTTP: one local mock server plays
//! the company's site and the ATS APIs, which serve the saved fixtures.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use chrono::{TimeZone, Utc};
use jobhunt_sources::company::{
    BoardEvidence, CompanyProbe, CompanyTarget, ProbeOutcome, ProbeSettings, discover,
};
use jobhunt_sources::registry::{Verdict, validate};
use jobhunt_sources::{HttpClient, HttpSettings};
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn fixture(kind: &str, name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/fixtures/{kind}/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

fn http() -> HttpClient {
    HttpClient::new(HttpSettings {
        timeout: Duration::from_secs(5),
        connect_timeout: Duration::from_secs(5),
        max_retries: 0,
        retry_base_delay: Duration::from_millis(1),
        max_per_host: 4,
    })
    .unwrap()
}

async fn page(server: &MockServer, at: &str, status: u16, html: &str) {
    Mock::given(method("GET"))
        .and(path(at))
        .respond_with(ResponseTemplate::new(status).set_body_string(html))
        .mount(server)
        .await;
}

async fn run(server: &MockServer, target: &str, guess_slugs: bool) -> CompanyProbe {
    let settings = ProbeSettings {
        guess_slugs,
        api_base: Some(server.uri()),
        site_base: Some(Url::parse(&format!("{}/", server.uri())).unwrap()),
        ..ProbeSettings::default()
    };
    let target: CompanyTarget = target.parse().unwrap();
    let now = Utc.with_ymd_and_hms(2026, 10, 5, 0, 0, 0).unwrap();
    discover(&http(), &target, &settings, now).await
}

#[tokio::test]
async fn follows_the_homepage_to_the_careers_page_and_checks_its_board() {
    let server = MockServer::start().await;
    page(
        &server,
        "/",
        200,
        r#"<nav><a href="/about">About</a> <a href="/careers">Careers</a></nav>"#,
    )
    .await;
    page(
        &server,
        "/careers",
        200,
        r#"<h1>Join Linear</h1>
           <a href="https://jobs.lever.co/portfolio-co">A company we invested in</a>
           <script src="https://jobs.ashbyhq.com/linear/embed?version=2"></script>"#,
    )
    .await;
    Mock::given(method("GET"))
        .and(path("/posting-api/job-board/linear"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(fixture("ashby", "linear.json")))
        .mount(&server)
        .await;

    let probe = run(&server, "linear.app, Linear", false).await;
    assert_eq!(probe.outcome, ProbeOutcome::Board);
    assert!(probe.careers_url.as_deref().unwrap().ends_with("/careers"));
    let sources: Vec<(&str, bool)> = probe
        .boards
        .iter()
        .map(|b| (b.source.as_str(), b.matches_company))
        .collect();
    assert_eq!(
        sources,
        vec![("lever:portfolio-co", false), ("ashby:linear", true)]
    );
    assert!(
        probe
            .boards
            .iter()
            .all(|b| b.evidence == BoardEvidence::CareersPage)
    );

    // Every board is read through its adapter: the unrelated one fails…
    let other = &probe.checks[0];
    assert!(other.error.as_deref().unwrap().contains("404"), "{other:?}");
    assert_eq!(validate(other).verdict, Verdict::Rejected);
    // …the company's own validates.
    let linear = &probe.checks[1];
    assert_eq!(linear.jobs, 3);
    assert_eq!(linear.with_provider_id, 3);
    assert_eq!(linear.freshness.total, 3);
    let v = validate(linear);
    assert_eq!(v.verdict, Verdict::Validated, "{:?}", v.reasons);
}

#[tokio::test]
async fn a_custom_careers_page_is_recorded_not_scraped() {
    let server = MockServer::start().await;
    page(&server, "/", 200, r#"<a href="/jobs">Jobs</a>"#).await;
    page(
        &server,
        "/jobs",
        200,
        r#"<h1>Open roles</h1><a href="/jobs/senior-engineer">Senior Engineer</a>"#,
    )
    .await;

    let probe = run(&server, "acme.com, Acme", false).await;
    assert_eq!(probe.outcome, ProbeOutcome::CustomPage);
    assert!(probe.careers_url.as_deref().unwrap().ends_with("/jobs"));
    assert!(probe.boards.is_empty() && probe.checks.is_empty());
    // The page's own job links are never followed.
    assert!(
        probe
            .pages
            .iter()
            .all(|p| !p.url.ends_with("/senior-engineer"))
    );
}

#[tokio::test]
async fn an_unsupported_ats_is_named() {
    let server = MockServer::start().await;
    page(&server, "/", 200, "<h1>Acme</h1>").await;
    page(
        &server,
        "/careers",
        200,
        r#"<a href="https://acme.wd1.myworkdayjobs.com/External">See open roles</a>"#,
    )
    .await;

    let probe = run(&server, "acme.com", false).await;
    assert_eq!(probe.outcome, ProbeOutcome::UnsupportedAts);
    assert_eq!(probe.other_ats[0].provider, "workday");
    assert!(probe.careers_url.as_deref().unwrap().ends_with("/careers"));
}

#[tokio::test]
async fn guesses_the_slug_only_when_no_page_names_a_board() {
    let server = MockServer::start().await;
    page(&server, "/", 200, "<h1>Figma</h1>").await;
    Mock::given(method("GET"))
        .and(path("/v1/boards/figma/jobs"))
        .respond_with(
            ResponseTemplate::new(200).set_body_bytes(fixture("greenhouse", "figma.json")),
        )
        .mount(&server)
        .await;

    let probe = run(&server, "figma.com, Figma", true).await;
    assert_eq!(probe.outcome, ProbeOutcome::Board);
    assert_eq!(probe.boards.len(), 1, "{:?}", probe.boards);
    let board = &probe.boards[0];
    assert_eq!(board.source, "greenhouse:figma");
    assert_eq!(board.evidence, BoardEvidence::SlugGuess);
    // Greenhouse reports the employer's name: evidence the board is Figma's.
    let check = &probe.checks[0];
    assert_eq!(check.company_names, vec!["Figma".to_owned()]);
    assert_eq!(check.naming_company, check.jobs);
    assert_eq!(validate(check).verdict, Verdict::Validated);

    // The same slug guessed for another company is not theirs: its
    // postings name neither the company nor its domain.
    let wrong = run(&server, "figma.io, Figment", true).await;
    assert_eq!(wrong.checks[0].source, "greenhouse:figma");
    let v = validate(&wrong.checks[0]);
    assert_eq!(v.verdict, Verdict::Inconclusive, "{:?}", v.reasons);
}

#[tokio::test]
async fn blocked_and_missing_sites_are_told_apart() {
    let blocked = MockServer::start().await;
    page(&blocked, "/", 403, "Forbidden").await;
    assert_eq!(
        run(&blocked, "acme.com", false).await.outcome,
        ProbeOutcome::Blocked
    );

    let plain = MockServer::start().await;
    page(&plain, "/", 200, "<h1>Just a homepage</h1>").await;
    let probe = run(&plain, "acme.com", false).await;
    assert_eq!(probe.outcome, ProbeOutcome::NoCareersPage);
    // The homepage plus the usual careers paths, within the page budget.
    assert!(probe.pages.len() <= ProbeSettings::default().max_pages);
    assert!(probe.pages.iter().any(|p| p.url.ends_with("/careers")));
}

#[tokio::test]
async fn redirects_home_and_community_links_are_not_careers_pages() {
    let server = MockServer::start().await;
    page(
        &server,
        "/",
        200,
        r#"<a href="/join/discord">Join our Discord</a> <a href="/pricing">Pricing</a>"#,
    )
    .await;
    // `/careers` and `/jobs` send people back to the homepage.
    for at in ["/careers", "/jobs"] {
        Mock::given(method("GET"))
            .and(path(at))
            .respond_with(ResponseTemplate::new(301).insert_header("location", "/"))
            .mount(&server)
            .await;
    }

    let probe = run(&server, "acme.com", false).await;
    assert_eq!(probe.outcome, ProbeOutcome::NoCareersPage, "{probe:?}");
    assert_eq!(probe.careers_url, None);
    assert!(probe.pages.iter().all(|p| !p.url.contains("discord")));
}
