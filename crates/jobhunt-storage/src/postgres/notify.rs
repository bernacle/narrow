//! Email notifications: settings, an outbox of deliveries, and the record
//! of every opportunity a person was notified about.
//!
//! The rules the schema and these methods enforce:
//!
//! * An address is used only after it was confirmed through a link sent to
//!   it; it is stored sealed, like every private value, and deleted with
//!   the account.
//! * A delivery and its items are written in one transaction *before* the
//!   provider is asked to send anything. `notification_items` has one row
//!   per (account, opportunity), so an opportunity can be put in a
//!   person's notification once, whatever retries or concurrent workers
//!   do: a second delivery claiming it fails to commit.
//! * A delivery is marked `sent` only after the provider accepted it, and
//!   only by the worker holding its lease. The account's notification
//!   cursor moves in the same statement. A provider failure leaves it
//!   `pending` for a later retry, which sends the same stored message with
//!   the same idempotency key (the delivery id), so a crash between the
//!   provider accepting it and the database recording it does not send it
//!   twice while the provider remembers the key.
//! * A digest slot is claimed once per local calendar day
//!   ([`PgStore::claim_digest_day`]), whether it finds anything to send or
//!   not, so a person gets at most one digest a day whatever the clock,
//!   the daylight-saving changes or concurrent workers do.

use std::collections::HashSet;
use std::time::Duration;

use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use jobhunt_jobs::StorageError;
use sqlx::Row;

use super::accounts::{UserId, hash_token, random_hex};
use super::{PgStore, corrupt, query_error};

/// How often a person asked to be emailed (BRU-295). Legacy: since
/// notification policy v2 both values mean the daily digest
/// ([`DigestSettings`]); it is kept, and still accepted, so nothing a
/// person chose is lost (instant alerts may read it later).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Cadence {
    /// Formerly: soon after strong new matches appear.
    #[default]
    Immediate,
    /// Formerly: at most one email a day.
    Daily,
}

impl Cadence {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Immediate => "immediate",
            Self::Daily => "daily",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "immediate" => Some(Self::Immediate),
            "daily" => Some(Self::Daily),
            _ => None,
        }
    }
}

/// When and how much the daily digest sends (local to the person).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DigestSettings {
    /// IANA name; `None`: not set yet (UTC is used).
    pub timezone: Option<String>,
    pub time: NaiveTime,
    /// Bitmask from Monday (1) to Sunday (64).
    pub days: u8,
    /// Jobs in one digest at most (1 to 5).
    pub size: u8,
    pub quiet_start: NaiveTime,
    pub quiet_end: NaiveTime,
    pub paused_until: Option<DateTime<Utc>>,
}

/// Monday to Friday.
pub const WEEKDAYS: u8 = 0b001_1111;

fn hm(hour: u32, minute: u32) -> NaiveTime {
    NaiveTime::from_hms_opt(hour, minute, 0).unwrap_or(NaiveTime::MIN)
}

impl Default for DigestSettings {
    fn default() -> Self {
        Self {
            timezone: None,
            time: hm(9, 0),
            days: WEEKDAYS,
            size: 3,
            quiet_start: hm(21, 0),
            quiet_end: hm(8, 0),
            paused_until: None,
        }
    }
}

/// A person's notification settings.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NotificationSettings {
    pub email_enabled: bool,
    /// Legacy (see [`Cadence`]).
    pub cadence: Cadence,
    pub digest: DigestSettings,
    /// The local date of the last digest slot used.
    pub last_digest_on: Option<NaiveDate>,
    pub email: Option<String>,
    /// When the address was confirmed (`None`: not yet).
    pub email_confirmed_at: Option<DateTime<Utc>>,
    /// When the last confirmation link was sent.
    pub confirmation_sent_at: Option<DateTime<Utc>>,
}

/// What to change (absent fields stay as they are). Values are validated
/// by the caller.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SettingsChange {
    pub email_enabled: Option<bool>,
    pub cadence: Option<Cadence>,
    /// A new address (normalized by the caller). A different address needs
    /// confirming again.
    pub email: Option<String>,
    pub timezone: Option<String>,
    pub digest_time: Option<NaiveTime>,
    pub digest_days: Option<u8>,
    pub digest_size: Option<u8>,
    pub quiet_start: Option<NaiveTime>,
    pub quiet_end: Option<NaiveTime>,
    /// `Some(None)` resumes; `Some(Some(t))` pauses until `t`.
    pub paused_until: Option<Option<DateTime<Utc>>>,
}

/// What a delivery is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryKind {
    /// New strong recommendations.
    Recommendations,
    /// The link that confirms an address.
    Confirmation,
}

impl DeliveryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Recommendations => "recommendations",
            Self::Confirmation => "confirmation",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "recommendations" => Some(Self::Recommendations),
            "confirmation" => Some(Self::Confirmation),
            _ => None,
        }
    }
}

/// One opportunity put in a notification, with the ranking that justified
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotifiedItem {
    /// Where it is in the message (1 = first).
    pub position: u16,
    /// How fresh the message said it was (`posted`, `backfill`, …).
    pub freshness: Option<String>,
    /// `opp_…`.
    pub opportunity: String,
    /// `strong_fit`, …
    pub tier: String,
    /// `job_…` the recommendation rested on.
    pub job: String,
    /// The content version of that record (its material fingerprint).
    pub content_version: String,
    pub ranking_version: String,
}

/// A delivery to write to the outbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewDelivery {
    /// `ntf_…` ([`new_delivery_id`]); also the provider's idempotency key.
    pub id: String,
    pub user: UserId,
    pub kind: DeliveryKind,
    /// The rendered message (sealed before it is stored).
    pub message: Vec<u8>,
    pub items: Vec<NotifiedItem>,
    /// Abandoned instead of sent after this (a digest is never sent
    /// inside quiet hours or on another day).
    pub expires_at: Option<DateTime<Utc>>,
}

/// A delivery a worker holds and must send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivery {
    pub id: String,
    pub user: UserId,
    pub kind: DeliveryKind,
    /// Attempts made before this one.
    pub attempts: u32,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    /// The rendered message, exactly as first written.
    pub message: Vec<u8>,
}

/// A delivery, for the person's settings page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliverySummary {
    pub id: String,
    pub kind: DeliveryKind,
    /// `pending`, `sent`, `failed` or `abandoned`.
    pub status: String,
    pub items: u32,
    pub created_at: DateTime<Utc>,
    pub sent_at: Option<DateTime<Utc>>,
}

/// An account whose digest a worker may compose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotifyAccount {
    pub user: UserId,
    pub email: String,
    pub digest: DigestSettings,
    /// The local date of the last digest slot used.
    pub last_digest_on: Option<NaiveDate>,
}

/// A fresh delivery id, `ntf_<32 hex>`.
pub fn new_delivery_id() -> String {
    format!("ntf_{}", random_hex(16))
}

/// A fresh email confirmation token (what the link carries; only its
/// digest is stored).
pub fn new_confirmation_token() -> String {
    random_hex(32)
}

fn email_context(user: &UserId) -> String {
    format!("notification_settings|{user}")
}

fn message_context(user: &UserId, id: &str) -> String {
    format!("notification_deliveries|{user}|{id}")
}

/// The digest columns of a `notification_settings` row.
fn digest_of(row: &sqlx::postgres::PgRow) -> Result<DigestSettings, sqlx::Error> {
    let days: i16 = row.try_get("digest_days")?;
    let size: i16 = row.try_get("digest_size")?;
    let d = DigestSettings::default();
    Ok(DigestSettings {
        timezone: row.try_get("timezone")?,
        time: row.try_get("digest_time")?,
        days: u8::try_from(days).unwrap_or(d.days),
        size: u8::try_from(size).unwrap_or(d.size),
        quiet_start: row.try_get("quiet_start")?,
        quiet_end: row.try_get("quiet_end")?,
        paused_until: row.try_get("paused_until")?,
    })
}

fn chrono_of(d: Duration) -> chrono::Duration {
    chrono::Duration::from_std(d).unwrap_or(chrono::Duration::MAX)
}

impl PgStore {
    fn seal_for(&self, context: &str, plain: &[u8]) -> Result<Vec<u8>, StorageError> {
        self.keys()
            .seal(context, plain)
            .map_err(|e| StorageError::Query {
                operation: "encrypting private data",
                source: Box::new(e),
            })
    }

    fn open_for(&self, context: &str, sealed: &[u8]) -> Result<Vec<u8>, StorageError> {
        self.keys()
            .open(context, sealed)
            .map_err(|e| corrupt(context, e))
    }

    /// The account's settings (defaults when never set).
    pub async fn notification_settings(
        &self,
        user: &UserId,
    ) -> Result<NotificationSettings, StorageError> {
        let row = sqlx::query(
            "SELECT email_enabled, cadence, email, email_confirmed_at, confirm_sent_at, \
             timezone, digest_time, digest_days, digest_size, quiet_start, quiet_end, \
             paused_until, last_digest_on FROM notification_settings WHERE user_id = $1",
        )
        .bind(user.as_str())
        .fetch_optional(self.pool())
        .await
        .map_err(query_error("loading notification settings"))?;
        let Some(row) = row else {
            return Ok(NotificationSettings::default());
        };
        let bad = |e: sqlx::Error| corrupt(user.as_str(), e);
        let cadence: String = row.try_get("cadence").map_err(bad)?;
        let sealed: Option<Vec<u8>> = row.try_get("email").map_err(bad)?;
        let email = sealed
            .map(|s| {
                let plain = self.open_for(&email_context(user), &s)?;
                String::from_utf8(plain).map_err(|e| corrupt(user.as_str(), e))
            })
            .transpose()?;
        Ok(NotificationSettings {
            email_enabled: row.try_get("email_enabled").map_err(bad)?,
            cadence: Cadence::parse(&cadence).unwrap_or_default(),
            digest: digest_of(&row).map_err(bad)?,
            last_digest_on: row.try_get("last_digest_on").map_err(bad)?,
            email,
            email_confirmed_at: row.try_get("email_confirmed_at").map_err(bad)?,
            confirmation_sent_at: row.try_get("confirm_sent_at").map_err(bad)?,
        })
    }

    /// Changes the settings. Returns them, and whether the address changed
    /// (it then needs confirming).
    pub async fn update_notification_settings(
        &self,
        user: &UserId,
        change: &SettingsChange,
        now: DateTime<Utc>,
    ) -> Result<(NotificationSettings, bool), StorageError> {
        let current = self.notification_settings(user).await?;
        let email_changed = change
            .email
            .as_ref()
            .is_some_and(|e| current.email.as_deref() != Some(e.as_str()));
        let sealed = match (&change.email, email_changed) {
            (Some(email), true) => Some(self.seal_for(&email_context(user), email.as_bytes())?),
            _ => None,
        };
        let d = DigestSettings::default();
        sqlx::query(
            "INSERT INTO notification_settings (user_id, email_enabled, cadence, email, \
             created_at, updated_at, timezone, digest_time, digest_days, digest_size, \
             quiet_start, quiet_end, paused_until) \
             VALUES ($1, COALESCE($2, false), COALESCE($3, 'immediate'), $4, $5, $5, $6, \
             COALESCE($7, $14), COALESCE($8, $15), COALESCE($9, $16), COALESCE($10, $17), \
             COALESCE($11, $18), $13) \
             ON CONFLICT (user_id) DO UPDATE SET \
             email_enabled = COALESCE($2, notification_settings.email_enabled), \
             cadence = COALESCE($3, notification_settings.cadence), \
             email = COALESCE($4, notification_settings.email), \
             timezone = COALESCE($6, notification_settings.timezone), \
             digest_time = COALESCE($7, notification_settings.digest_time), \
             digest_days = COALESCE($8, notification_settings.digest_days), \
             digest_size = COALESCE($9, notification_settings.digest_size), \
             quiet_start = COALESCE($10, notification_settings.quiet_start), \
             quiet_end = COALESCE($11, notification_settings.quiet_end), \
             paused_until = CASE WHEN $12 THEN $13 ELSE notification_settings.paused_until END, \
             email_confirmed_at = CASE WHEN $4 IS NULL \
                 THEN notification_settings.email_confirmed_at END, \
             confirm_token_hash = CASE WHEN $4 IS NULL \
                 THEN notification_settings.confirm_token_hash END, \
             confirm_sent_at = CASE WHEN $4 IS NULL \
                 THEN notification_settings.confirm_sent_at END, \
             updated_at = $5",
        )
        .bind(user.as_str())
        .bind(change.email_enabled)
        .bind(change.cadence.map(Cadence::as_str))
        .bind(sealed)
        .bind(now)
        .bind(change.timezone.as_deref())
        .bind(change.digest_time)
        .bind(change.digest_days.map(i16::from))
        .bind(change.digest_size.map(i16::from))
        .bind(change.quiet_start)
        .bind(change.quiet_end)
        .bind(change.paused_until.is_some())
        .bind(change.paused_until.flatten())
        .bind(d.time)
        .bind(i16::from(d.days))
        .bind(i16::from(d.size))
        .bind(d.quiet_start)
        .bind(d.quiet_end)
        .execute(self.pool())
        .await
        .map_err(query_error("saving notification settings"))?;
        Ok((self.notification_settings(user).await?, email_changed))
    }

    /// Remembers the digest of a confirmation token just sent.
    pub async fn set_confirmation_token(
        &self,
        user: &UserId,
        token: &str,
        now: DateTime<Utc>,
    ) -> Result<(), StorageError> {
        sqlx::query(
            "UPDATE notification_settings SET confirm_token_hash = $2, confirm_sent_at = $3 \
             WHERE user_id = $1",
        )
        .bind(user.as_str())
        .bind(hash_token(token))
        .bind(now)
        .execute(self.pool())
        .await
        .map_err(query_error("saving a confirmation token"))?;
        Ok(())
    }

    /// Confirms the address when `token` is the one last sent to it (and
    /// was sent after `sent_after`). Returns whether it was confirmed.
    pub async fn confirm_email(
        &self,
        user: &UserId,
        token: &str,
        sent_after: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<bool, StorageError> {
        let changed = sqlx::query(
            "UPDATE notification_settings SET email_confirmed_at = $4, \
             confirm_token_hash = NULL, updated_at = $4 \
             WHERE user_id = $1 AND confirm_token_hash = $2 AND confirm_sent_at > $3 \
             AND email IS NOT NULL",
        )
        .bind(user.as_str())
        .bind(hash_token(token))
        .bind(sent_after)
        .bind(now)
        .execute(self.pool())
        .await
        .map_err(query_error("confirming an address"))?
        .rows_affected();
        Ok(changed > 0)
    }

    /// Claims up to `limit` accounts with notifications on and a confirmed
    /// address, that no other worker holds and that have no delivery
    /// waiting to be sent.
    pub async fn claim_notification_accounts(
        &self,
        owner: &str,
        limit: usize,
        lease: Duration,
        now: DateTime<Utc>,
    ) -> Result<Vec<NotifyAccount>, StorageError> {
        let rows = sqlx::query(
            "UPDATE notification_settings s SET lease_owner = $1, lease_expires_at = $2 \
             FROM (SELECT user_id FROM notification_settings \
                   WHERE email_enabled AND email_confirmed_at IS NOT NULL AND email IS NOT NULL \
                   AND (lease_expires_at IS NULL OR lease_expires_at < $3) \
                   AND NOT EXISTS (SELECT 1 FROM notification_deliveries d \
                       WHERE d.user_id = notification_settings.user_id \
                       AND d.status = 'pending' AND d.kind = 'recommendations') \
                   ORDER BY user_id LIMIT $4 FOR UPDATE SKIP LOCKED) due \
             WHERE s.user_id = due.user_id \
             RETURNING s.user_id, s.email, s.timezone, s.digest_time, s.digest_days, \
             s.digest_size, s.quiet_start, s.quiet_end, s.paused_until, s.last_digest_on",
        )
        .bind(owner)
        .bind(now + chrono_of(lease))
        .bind(now)
        .bind(i64::try_from(limit).unwrap_or(i64::MAX))
        .fetch_all(self.pool())
        .await
        .map_err(query_error("claiming accounts to notify"))?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let id: String = row.try_get("user_id").map_err(|e| corrupt("<notify>", e))?;
            let user: UserId = id.parse().map_err(|e| corrupt(&id, e))?;
            let bad = |e: sqlx::Error| corrupt(user.as_str(), e);
            let sealed: Vec<u8> = row.try_get("email").map_err(bad)?;
            let email = String::from_utf8(self.open_for(&email_context(&user), &sealed)?)
                .map_err(|e| corrupt(user.as_str(), e))?;
            out.push(NotifyAccount {
                email,
                digest: digest_of(&row).map_err(bad)?,
                last_digest_on: row.try_get("last_digest_on").map_err(bad)?,
                user,
            });
        }
        Ok(out)
    }

    /// Gives an account back.
    pub async fn release_notification_account(
        &self,
        owner: &str,
        user: &UserId,
    ) -> Result<(), StorageError> {
        sqlx::query(
            "UPDATE notification_settings SET lease_owner = NULL, lease_expires_at = NULL \
             WHERE user_id = $1 AND lease_owner = $2",
        )
        .bind(user.as_str())
        .bind(owner)
        .execute(self.pool())
        .await
        .map_err(query_error("releasing an account"))?;
        Ok(())
    }

    /// Uses the person's digest slot for the local date `day`. Returns
    /// `false` when it was already used (by this or another worker): at
    /// most one digest per local calendar day.
    pub async fn claim_digest_day(
        &self,
        user: &UserId,
        day: NaiveDate,
    ) -> Result<bool, StorageError> {
        let changed = sqlx::query(
            "UPDATE notification_settings SET last_digest_on = $2 \
             WHERE user_id = $1 AND (last_digest_on IS NULL OR last_digest_on < $2)",
        )
        .bind(user.as_str())
        .bind(day)
        .execute(self.pool())
        .await
        .map_err(query_error("claiming a digest"))?
        .rows_affected();
        Ok(changed > 0)
    }

    /// The companies of the jobs in the person's notifications accepted by
    /// the provider since `since` (as the job records name them).
    pub async fn recently_notified_companies(
        &self,
        user: &UserId,
        since: DateTime<Utc>,
    ) -> Result<HashSet<String>, StorageError> {
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT j.company FROM notification_items i \
             JOIN notification_deliveries d ON d.id = i.delivery_id \
             JOIN jobs j ON j.id = i.job_id \
             WHERE i.user_id = $1 AND d.status = 'sent' AND d.sent_at >= $2",
        )
        .bind(user.as_str())
        .bind(since)
        .fetch_all(self.pool())
        .await
        .map_err(query_error("loading recently notified companies"))?;
        Ok(rows.into_iter().collect())
    }

    /// Which of these opportunities the person was already notified about.
    pub async fn notified_opportunities(
        &self,
        user: &UserId,
        opportunities: &[String],
    ) -> Result<HashSet<String>, StorageError> {
        if opportunities.is_empty() {
            return Ok(HashSet::new());
        }
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT opportunity_id FROM notification_items \
             WHERE user_id = $1 AND opportunity_id = ANY($2)",
        )
        .bind(user.as_str())
        .bind(opportunities)
        .fetch_all(self.pool())
        .await
        .map_err(query_error("loading notified opportunities"))?;
        Ok(rows.into_iter().collect())
    }

    /// Every opportunity the person was ever notified about.
    pub async fn notified_opportunities_all(
        &self,
        user: &UserId,
    ) -> Result<HashSet<String>, StorageError> {
        let rows: Vec<String> =
            sqlx::query_scalar("SELECT opportunity_id FROM notification_items WHERE user_id = $1")
                .bind(user.as_str())
                .fetch_all(self.pool())
                .await
                .map_err(query_error("loading notified opportunities"))?;
        Ok(rows.into_iter().collect())
    }

    /// Writes a delivery and its items to the outbox, in one transaction.
    /// Returns `false` (and writes nothing) when one of its opportunities
    /// was already put in another notification for this person.
    pub async fn enqueue_delivery(
        &self,
        delivery: &NewDelivery,
        now: DateTime<Utc>,
    ) -> Result<bool, StorageError> {
        let sealed = self.seal_for(
            &message_context(&delivery.user, &delivery.id),
            &delivery.message,
        )?;
        let mut tx = self
            .pool()
            .begin()
            .await
            .map_err(query_error("starting a transaction"))?;
        sqlx::query(
            "INSERT INTO notification_deliveries (id, user_id, kind, status, attempts, \
             next_attempt_at, created_at, items, message, expires_at) \
             VALUES ($1, $2, $3, 'pending', 0, $4, $4, $5, $6, $7)",
        )
        .bind(&delivery.id)
        .bind(delivery.user.as_str())
        .bind(delivery.kind.as_str())
        .bind(now)
        .bind(i32::try_from(delivery.items.len()).unwrap_or(i32::MAX))
        .bind(sealed)
        .bind(delivery.expires_at)
        .execute(&mut *tx)
        .await
        .map_err(query_error("writing a notification"))?;
        for item in &delivery.items {
            let inserted = sqlx::query(
                "INSERT INTO notification_items (user_id, opportunity_id, delivery_id, tier, \
                 job_id, content_version, ranking_version, notified_at, position, freshness) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) \
                 ON CONFLICT (user_id, opportunity_id) DO NOTHING",
            )
            .bind(delivery.user.as_str())
            .bind(&item.opportunity)
            .bind(&delivery.id)
            .bind(&item.tier)
            .bind(&item.job)
            .bind(&item.content_version)
            .bind(&item.ranking_version)
            .bind(now)
            .bind(i16::try_from(item.position).unwrap_or(i16::MAX))
            .bind(item.freshness.as_deref())
            .execute(&mut *tx)
            .await
            .map_err(query_error("writing a notification"))?
            .rows_affected();
            if inserted == 0 {
                // Someone else notified about it meanwhile: send nothing.
                tx.rollback()
                    .await
                    .map_err(query_error("abandoning a notification"))?;
                return Ok(false);
            }
        }
        tx.commit()
            .await
            .map_err(query_error("writing a notification"))?;
        Ok(true)
    }

    /// Claims pending deliveries that are due (all accounts, or one).
    pub async fn claim_deliveries(
        &self,
        owner: &str,
        limit: usize,
        lease: Duration,
        now: DateTime<Utc>,
        only: Option<&UserId>,
    ) -> Result<Vec<Delivery>, StorageError> {
        let rows = sqlx::query(
            "UPDATE notification_deliveries d SET lease_owner = $1, lease_expires_at = $2 \
             FROM (SELECT id FROM notification_deliveries \
                   WHERE status = 'pending' AND next_attempt_at <= $3 \
                   AND (lease_expires_at IS NULL OR lease_expires_at < $3) \
                   AND ($5::text IS NULL OR user_id = $5) \
                   ORDER BY next_attempt_at, id LIMIT $4 FOR UPDATE SKIP LOCKED) due \
             WHERE d.id = due.id \
             RETURNING d.id, d.user_id, d.kind, d.attempts, d.created_at, d.expires_at, \
             d.message",
        )
        .bind(owner)
        .bind(now + chrono_of(lease))
        .bind(now)
        .bind(i64::try_from(limit).unwrap_or(i64::MAX))
        .bind(only.map(UserId::as_str))
        .fetch_all(self.pool())
        .await
        .map_err(query_error("claiming notifications to send"))?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let id: String = row.try_get("id").map_err(|e| corrupt("<delivery>", e))?;
            let bad = |e: sqlx::Error| corrupt(&id, e);
            let user_id: String = row.try_get("user_id").map_err(bad)?;
            let user: UserId = user_id.parse().map_err(|e| corrupt(&id, e))?;
            let kind: String = row.try_get("kind").map_err(bad)?;
            let sealed: Vec<u8> = row.try_get("message").map_err(bad)?;
            let attempts: i32 = row.try_get("attempts").map_err(bad)?;
            out.push(Delivery {
                message: self.open_for(&message_context(&user, &id), &sealed)?,
                kind: DeliveryKind::parse(&kind)
                    .ok_or_else(|| corrupt(&id, format!("unknown kind {kind:?}")))?,
                attempts: u32::try_from(attempts).unwrap_or(0),
                created_at: row.try_get("created_at").map_err(bad)?,
                expires_at: row.try_get("expires_at").map_err(bad)?,
                user,
                id,
            });
        }
        Ok(out)
    }

    /// The provider accepted a delivery: it is sent, and the account's
    /// notification cursor moves to it. Only the lease holder can do this.
    pub async fn delivery_sent(
        &self,
        owner: &str,
        id: &str,
        provider: &str,
        message_id: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<bool, StorageError> {
        let mut tx = self
            .pool()
            .begin()
            .await
            .map_err(query_error("starting a transaction"))?;
        let row: Option<(String, String, i64)> = sqlx::query_as(
            "UPDATE notification_deliveries SET status = 'sent', sent_at = $3, \
             attempts = attempts + 1, provider = $4, provider_message_id = $5, \
             last_error = NULL, lease_owner = NULL, lease_expires_at = NULL \
             WHERE id = $1 AND lease_owner = $2 AND status = 'pending' \
             RETURNING user_id, kind, seq",
        )
        .bind(id)
        .bind(owner)
        .bind(now)
        .bind(provider)
        .bind(message_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(query_error("recording a sent notification"))?;
        let Some((user, kind, seq)) = row else {
            return Ok(false);
        };
        if kind == DeliveryKind::Recommendations.as_str() {
            sqlx::query(
                "INSERT INTO user_state (user_id, notification_cursor) VALUES ($1, $2) \
                 ON CONFLICT (user_id) DO UPDATE SET notification_cursor = \
                 GREATEST(user_state.notification_cursor, excluded.notification_cursor)",
            )
            .bind(&user)
            .bind(seq)
            .execute(&mut *tx)
            .await
            .map_err(query_error("moving the notification cursor"))?;
        }
        tx.commit()
            .await
            .map_err(query_error("recording a sent notification"))?;
        Ok(true)
    }

    /// The provider refused or could not be reached: try again at
    /// `retry_at`, or give up (`None`: the delivery is `failed`).
    pub async fn delivery_failed(
        &self,
        owner: &str,
        id: &str,
        error: &str,
        retry_at: Option<DateTime<Utc>>,
    ) -> Result<(), StorageError> {
        sqlx::query(
            "UPDATE notification_deliveries SET attempts = attempts + 1, last_error = $3, \
             status = CASE WHEN $4::timestamptz IS NULL THEN 'failed' ELSE 'pending' END, \
             next_attempt_at = COALESCE($4, next_attempt_at), \
             lease_owner = NULL, lease_expires_at = NULL \
             WHERE id = $1 AND lease_owner = $2 AND status = 'pending'",
        )
        .bind(id)
        .bind(owner)
        .bind(error)
        .bind(retry_at)
        .execute(self.pool())
        .await
        .map_err(query_error("recording a failed notification"))?;
        Ok(())
    }

    /// A delivery whose outcome is unknown and can no longer be retried
    /// safely (the provider no longer remembers its idempotency key).
    pub async fn delivery_abandoned(&self, owner: &str, id: &str) -> Result<(), StorageError> {
        sqlx::query(
            "UPDATE notification_deliveries SET status = 'abandoned', \
             lease_owner = NULL, lease_expires_at = NULL \
             WHERE id = $1 AND lease_owner = $2 AND status = 'pending'",
        )
        .bind(id)
        .bind(owner)
        .execute(self.pool())
        .await
        .map_err(query_error("abandoning a notification"))?;
        Ok(())
    }

    /// The person's latest deliveries, newest first.
    pub async fn recent_deliveries(
        &self,
        user: &UserId,
        limit: usize,
    ) -> Result<Vec<DeliverySummary>, StorageError> {
        let rows = sqlx::query(
            "SELECT id, kind, status, items, created_at, sent_at FROM notification_deliveries \
             WHERE user_id = $1 ORDER BY created_at DESC, seq DESC LIMIT $2",
        )
        .bind(user.as_str())
        .bind(i64::try_from(limit).unwrap_or(i64::MAX))
        .fetch_all(self.pool())
        .await
        .map_err(query_error("loading notifications"))?;
        rows.iter()
            .map(|row| {
                let id: String = row.try_get("id").map_err(|e| corrupt("<delivery>", e))?;
                let bad = |e: sqlx::Error| corrupt(&id, e);
                let kind: String = row.try_get("kind").map_err(bad)?;
                let items: i32 = row.try_get("items").map_err(bad)?;
                Ok(DeliverySummary {
                    kind: DeliveryKind::parse(&kind)
                        .ok_or_else(|| corrupt(&id, format!("unknown kind {kind:?}")))?,
                    status: row.try_get("status").map_err(bad)?,
                    items: u32::try_from(items).unwrap_or(0),
                    created_at: row.try_get("created_at").map_err(bad)?,
                    sent_at: row.try_get("sent_at").map_err(bad)?,
                    id,
                })
            })
            .collect()
    }
}
