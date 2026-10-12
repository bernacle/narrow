//! Request and response bodies of the HTTP API that are not already the
//! application's views. The CLI uses these same types, so client and
//! server cannot drift apart. Product answers (search results, job details,
//! verification reports, feedback results, the profile, preferences, the
//! pipeline, application context) are the exact views the MCP tools
//! return (`jobhunt_app::views` and friends).

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `GET /api/v1/auth/config`: how to sign in (public).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AuthConfigView {
    /// `oidc` or `dev`.
    pub mode: String,
    /// The identity provider (OIDC issuer URL), when `oidc`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issuer: Option<String>,
    /// The audience to request tokens for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audience: Option<String>,
    /// The request parameter that carries it (`resource`, RFC 8707, or
    /// `audience`); absent means none is sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audience_parameter: Option<String>,
    /// The public client the CLI signs in with (device authorization).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli_client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scopes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_authorization_endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revocation_endpoint: Option<String>,
}

/// `POST /api/v1/auth/dev-token` (development servers only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DevTokenRequest {
    /// Who to sign in as (any name; it becomes the identity's subject).
    pub subject: String,
}

/// A token issued by a development server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DevTokenResponse {
    pub access_token: String,
    pub expires_in: u64,
}

/// One identity-provider identity linked to the account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct IdentityView {
    pub issuer: String,
    pub created_at: DateTime<Utc>,
    pub last_login_at: DateTime<Utc>,
}

/// Where the account's data stands in the cloud.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CloudStateView {
    /// Changes recorded so far (the sync cursor's upper bound).
    pub change_seq: u64,
    pub profile_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sync_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_shortlist_at: Option<DateTime<Utc>>,
    /// Feedback events (not counting "seen" marks).
    pub feedback: u64,
    /// For notifications (BRU-295): the last recommendation change covered.
    pub notification_cursor: u64,
}

/// `GET /api/v1/account`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AccountView {
    /// `usr_…`: the internal account id.
    pub id: String,
    pub created_at: DateTime<Utc>,
    /// How this request authenticated: `oidc`, `token` or `dev`.
    pub authenticated_with: String,
    pub identities: Vec<IdentityView>,
    pub cloud: CloudStateView,
}

/// A personal access token (never its secret).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TokenView {
    /// `tok_…`.
    pub id: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<DateTime<Utc>>,
}

/// `POST /api/v1/tokens`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateTokenRequest {
    /// What the token is for ("Claude Desktop").
    pub name: String,
    /// Days until it expires (default 90, at most 365).
    #[serde(default)]
    pub expires_in_days: Option<u32>,
}

/// A token just created: the only time its secret is shown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CreatedToken {
    pub token: TokenView,
    /// `jh_pat_…`: send it as `Authorization: Bearer <secret>`.
    pub secret: String,
}

/// `GET /api/v1/tokens`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TokenList {
    pub tokens: Vec<TokenView>,
}

/// Feedback actions accepted by the API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackInput {
    /// Looked at it, no opinion. Carries no weight in learned taste.
    Seen,
    Save,
    Unsave,
    Reject,
    Applied,
    Interview,
    Offer,
    Like,
    Dislike,
}

/// `POST /api/v1/opportunities/{id}/feedback`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FeedbackRequest {
    pub action: FeedbackInput,
    /// Why, in the person's own words. Stored verbatim (encrypted).
    #[serde(default)]
    pub reason: Option<String>,
}

/// `POST /api/v1/opportunities/{id}/verify`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VerifyRequest {
    /// Ask the sources even if they were asked in the last few minutes.
    #[serde(default)]
    pub force: bool,
}

/// `GET /health`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Health {
    pub status: String,
    pub version: String,
}

/// `GET /ready`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Ready {
    /// `ready` or `not_ready`.
    pub status: String,
    pub database: String,
    pub schema: String,
}

/// Every error: `{"error": {"code", "message", "hint", "request_id"}}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ErrorBody {
    pub error: ErrorDetail,
}

/// An error, with a stable `code` (the same codes as the CLI and the MCP
/// tools: `unknown_opportunity`, `no_profile`, `conflict`, …, plus
/// `unauthenticated`, `forbidden`, `not_found`, `invalid_request`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ErrorDetail {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// `GET /api/v1/feed`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FeedQuery {
    /// Items at most, 1 to 10 (default 5).
    #[serde(default)]
    pub limit: Option<usize>,
}

/// `GET /api/v1/profile/claims`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClaimsQuery {
    /// Claims at most (default: all of them).
    #[serde(default)]
    pub limit: Option<usize>,
}

/// `POST /api/v1/profile/claims`: confirm, reject or reset claims.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DecideClaimsRequest {
    /// `clm_…` ids.
    pub ids: Vec<String>,
    pub decision: jobhunt_app::profile_edit::ClaimDecision,
    /// Why (kept on the claims verbatim).
    #[serde(default)]
    pub note: Option<String>,
}

/// `PUT /api/v1/profile/resume?file_name=…` (the body is the file).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResumeQuery {
    /// The file's name; its extension decides how it is read (`.pdf`,
    /// `.txt`, `.md`). Defaults from the content type.
    #[serde(default)]
    pub file_name: Option<String>,
}

/// `PUT /api/v1/profile/linkedin?file_name=…` (the body is the export:
/// the `.zip` LinkedIn sends, or one of its CSV files).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LinkedinQuery {
    /// The file's name (`Basic_LinkedInDataExport_09-01-2026.zip`,
    /// `Positions.csv`); a CSV is recognized by it.
    #[serde(default)]
    pub file_name: Option<String>,
}

/// `POST /api/v1/profile/github`: import a public GitHub account.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GithubImportRequest {
    /// A GitHub username or profile URL. Defaults to the account imported
    /// before, or the GitHub link on the profile.
    #[serde(default)]
    pub username: Option<String>,
}

/// One notification email, for the settings page (never its content).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeliveryView {
    pub id: String,
    /// `recommendations` (a digest) or `confirmation`.
    pub kind: String,
    /// `pending`, `sent`, `failed` or `abandoned`.
    pub status: String,
    /// Opportunities in it.
    pub opportunities: u32,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_at: Option<DateTime<Utc>>,
}

/// `GET /api/v1/notifications`: the daily digest email (notification
/// policy v2). At most one digest per local day, at `digest_time` on
/// `digest_days`, never inside quiet hours or while paused; nothing is
/// sent when nothing qualifies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NotificationSettingsView {
    /// Whether this service can send email at all.
    pub available: bool,
    /// The daily digest is on.
    pub email_enabled: bool,
    /// Legacy (BRU-295): `immediate` or `daily`. Both now mean the daily
    /// digest.
    pub cadence: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// `none`, `unconfirmed` (a link was sent; nothing else is sent until
    /// it is followed) or `confirmed`.
    pub email_status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmation_sent_at: Option<DateTime<Utc>>,
    /// The IANA timezone the schedule is read in; absent: not set yet
    /// (UTC is used until it is).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    /// Local time the digest is composed, `HH:MM`.
    pub digest_time: String,
    /// Days a digest may come (`mon` … `sun`).
    pub digest_days: Vec<String>,
    /// Jobs in one digest at most (1 to `max_items`).
    pub digest_size: u8,
    /// Nothing is sent from `quiet_start` to `quiet_end` (local, `HH:MM`;
    /// equal: no quiet hours).
    pub quiet_start: String,
    pub quiet_end: String,
    /// Paused until then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused_until: Option<DateTime<Utc>>,
    /// When the next digest may go out (it goes out only if something
    /// qualifies); absent while off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_digest_at: Option<DateTime<Utc>>,
    /// The largest digest size.
    pub max_items: usize,
    /// The latest emails, newest first.
    pub recent: Vec<DeliveryView>,
}

/// `PUT /api/v1/notifications`. Absent fields are left as they are.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateNotificationsRequest {
    #[serde(default)]
    pub email_enabled: Option<bool>,
    /// Legacy: `immediate` or `daily` (both mean the daily digest).
    #[serde(default)]
    pub cadence: Option<String>,
    /// IANA timezone (`America/Sao_Paulo`).
    #[serde(default)]
    pub timezone: Option<String>,
    /// `HH:MM`, outside quiet hours.
    #[serde(default)]
    pub digest_time: Option<String>,
    /// At least one of `mon` … `sun`.
    #[serde(default)]
    pub digest_days: Option<Vec<String>>,
    /// 1 to `max_items`.
    #[serde(default)]
    pub digest_size: Option<u8>,
    /// `HH:MM`; equal start and end: no quiet hours.
    #[serde(default)]
    pub quiet_start: Option<String>,
    #[serde(default)]
    pub quiet_end: Option<String>,
    /// Pause until then (at most a year ahead); `null` resumes.
    #[serde(default, deserialize_with = "present")]
    pub paused_until: Option<Option<DateTime<Utc>>>,
    /// The address to notify. A new address is sent a confirmation link;
    /// nothing else is sent to it until it is confirmed.
    #[serde(default)]
    pub email: Option<String>,
    /// Send the confirmation link again.
    #[serde(default)]
    pub resend_confirmation: bool,
}

/// A field that may be absent (`None`), `null` (`Some(None)`) or a value.
fn present<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

/// `POST /api/v1/notifications/confirm`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfirmEmailRequest {
    /// The token from the confirmation link.
    pub token: String,
}
