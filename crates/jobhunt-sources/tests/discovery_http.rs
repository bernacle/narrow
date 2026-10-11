//! Broad discovery end to end over HTTP: a discovered ATS URL → its board,
//! read through the adapter → the company's domain from the board → the
//! company's own site, which must point back. One local mock server plays
//! the ATS APIs, the hosted board pages and the company's site.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use chrono::{TimeZone, Utc};
use jobhunt_sources::discovery::resolve::{
    BoardRead, OwnershipResult, ResolveSettings, establish_ownership, read_board,
};
use jobhunt_sources::discovery::{
    CandidateKind, DiscoveryStore, Lead, Method, Ownership, Sighting,
};
use jobhunt_sources::registry::Verdict;
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

fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 5, 0, 0, 0).unwrap()
}

fn settings(server: &MockServer) -> ResolveSettings {
    ResolveSettings {
        api_base: Some(server.uri()),
        board_page_base: Some(server.uri()),
        site_base: Some(Url::parse(&format!("{}/", server.uri())).unwrap()),
        ..ResolveSettings::new()
    }
}

async fn serve(server: &MockServer, at: &str, status: u16, body: Vec<u8>) {
    Mock::given(method("GET"))
        .and(path(at))
        .respond_with(ResponseTemplate::new(status).set_body_bytes(body))
        .mount(server)
        .await;
}

async fn linear_board(server: &MockServer) {
    serve(
        server,
        "/posting-api/job-board/linear",
        200,
        fixture("ashby", "linear.json"),
    )
    .await;
    // The hosted board page names the company's website.
    serve(
        server,
        "/linear",
        200,
        br#"<script>window.__appData = {"organization":{"name":"Linear","publicWebsite":"https:\/\/linear.app"}}</script>"#.to_vec(),
    )
    .await;
}

const LIVE: &str =
    "https://jobs.ashbyhq.com/linear/f04f398b-6320-499d-8a60-290239d62da8?utm_source=search";
const DEAD: &str = "https://jobs.ashbyhq.com/linear/00000000-0000-4000-8000-000000000000";

async fn resolve(
    server: &MockServer,
    board: &str,
    independent: &[String],
) -> (BoardRead, OwnershipResult) {
    let mut store = DiscoveryStore::new();
    let mut s = Sighting::new(Method::AtsSearch, "websearch", board, now());
    s.query = Some("site:jobs.ashbyhq.com \"platform engineer\" remote".into());
    store.add(Lead::url(board, s)).unwrap();
    let candidate = store.of_kind(CandidateKind::Board).next().unwrap().clone();
    let board = candidate.board.unwrap().to_ref().unwrap();
    let settings = settings(server);
    let read = read_board(&http(), &board, &settings, now()).await;
    let owner = establish_ownership(&http(), &read, independent, None, &settings, now()).await;
    (read, owner)
}

#[tokio::test]
async fn a_search_hit_resolves_to_a_board_the_company_site_points_at() {
    let server = MockServer::start().await;
    linear_board(&server).await;
    serve(
        &server,
        "/",
        200,
        br#"<a href="/careers">Careers</a>"#.to_vec(),
    )
    .await;
    serve(
        &server,
        "/careers",
        200,
        br#"<script src="https://jobs.ashbyhq.com/linear/embed?version=2"></script>"#.to_vec(),
    )
    .await;

    // One job URL found twice and a stale one: one board, two jobs.
    let mut store = DiscoveryStore::new();
    for url in [LIVE, LIVE.split('?').next().unwrap(), DEAD] {
        store
            .add(Lead::url(
                url,
                Sighting::new(Method::AtsSearch, "websearch", url, now()),
            ))
            .unwrap();
    }
    assert_eq!(store.of_kind(CandidateKind::Job).count(), 2);
    assert_eq!(store.of_kind(CandidateKind::Board).count(), 1);

    let (read, owner) = resolve(&server, LIVE, &[]).await;
    assert_eq!(read.listing, "ok");
    assert_eq!(read.check.jobs, 3);
    // Stale index entries are found by the board's current listing.
    assert!(
        read.posting("f04f398b-6320-499d-8a60-290239d62da8")
            .is_some()
    );
    assert!(
        read.posting("00000000-0000-4000-8000-000000000000")
            .is_none()
    );

    assert_eq!(
        owner.ownership,
        Ownership::Verified,
        "{:?}",
        owner.validation
    );
    assert_eq!(owner.validation.verdict, Verdict::Validated);
    assert_eq!(owner.domain.as_deref(), Some("linear.app"));
    assert!(owner.careers_url.as_deref().unwrap().ends_with("/careers"));
    assert!(
        owner
            .validation
            .reasons
            .iter()
            .any(|r| r.contains("the company's site points at it"))
    );
}

#[tokio::test]
async fn a_board_naming_a_website_that_does_not_point_back_stays_a_candidate() {
    let server = MockServer::start().await;
    linear_board(&server).await;
    serve(
        &server,
        "/",
        200,
        br#"<p>Welcome. No careers link here.</p>"#.to_vec(),
    )
    .await;

    let (_, owner) = resolve(&server, LIVE, &[]).await;
    assert_eq!(owner.ownership, Ownership::Claimed);
    assert_eq!(
        owner.validation.verdict,
        Verdict::Inconclusive,
        "{:?}",
        owner.validation
    );
    assert!(
        owner
            .validation
            .reasons
            .last()
            .unwrap()
            .contains("doesn't point back")
    );
}

#[tokio::test]
async fn a_company_site_pointing_at_another_board_is_followed() {
    let server = MockServer::start().await;
    linear_board(&server).await;
    serve(
        &server,
        "/",
        200,
        br#"<a href="https://jobs.lever.co/linear">Open roles</a>"#.to_vec(),
    )
    .await;
    serve(
        &server,
        "/v0/postings/linear",
        200,
        fixture("lever", "spotify.json"),
    )
    .await;

    let (_, owner) = resolve(&server, LIVE, &[]).await;
    assert_eq!(owner.ownership, Ownership::Elsewhere);
    assert_eq!(owner.validation.verdict, Verdict::Inconclusive);
    let points: Vec<String> = owner
        .points_at
        .iter()
        .map(|b| format!("{}:{}", b.kind, b.name))
        .collect();
    assert_eq!(points, ["lever:linear"]);
    assert!(
        owner
            .validation
            .reasons
            .last()
            .unwrap()
            .contains("lever:linear instead")
    );
}

#[tokio::test]
async fn a_gone_board_is_rejected_whatever_the_index_says() {
    let server = MockServer::start().await;
    serve(&server, "/v1/boards/acme/jobs", 404, b"{}".to_vec()).await;
    let url = "https://boards.greenhouse.io/acme/jobs/4400001";
    let (read, _) = resolve(&server, url, &[]).await;
    assert_eq!(read.listing, "not_found");
    let v = jobhunt_sources::discovery::resolve::validate_read(&read);
    assert_eq!(v.verdict, Verdict::Rejected);
}

#[tokio::test]
async fn ownership_needs_the_company_site_or_an_independent_source() {
    let server = MockServer::start().await;
    // Stripe's postings live on stripe.com (an embedded board), so the
    // board itself suggests stripe.com.
    serve(
        &server,
        "/v1/boards/stripe/jobs",
        200,
        fixture("greenhouse", "stripe.json"),
    )
    .await;
    serve(&server, "/", 200, br#"<p>No careers link.</p>"#.to_vec()).await;
    let url = "https://job-boards.greenhouse.io/stripe/jobs/7789539";

    // The board's own word is not enough…
    let (_, owner) = resolve(&server, url, &[]).await;
    assert_eq!(owner.domain.as_deref(), Some("stripe.com"));
    assert_eq!(owner.ownership, Ownership::Claimed);
    assert_eq!(owner.validation.verdict, Verdict::Inconclusive);

    // …a hiring post naming stripe.com, with postings tied to it, is.
    let (_, owner) = resolve(&server, url, &["stripe.com".to_owned()]).await;
    assert_eq!(
        owner.ownership,
        Ownership::Corroborated,
        "{:?}",
        owner.validation
    );
    assert_eq!(owner.validation.verdict, Verdict::Validated);

    // An independent source naming an unrelated domain ties to nothing.
    let (_, owner) = resolve(&server, url, &["unrelated.dev".to_owned()]).await;
    assert_ne!(owner.validation.verdict, Verdict::Validated, "{:?}", owner);
}
