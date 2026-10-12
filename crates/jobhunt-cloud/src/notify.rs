//! The daily digest email (notification policy v2, BRU-389 slice A; see
//! `docs/email-digest-policy-v2.md`): once a day, Narrow quietly sends at
//! most a few jobs worth reviewing, and nothing when nothing qualifies.
//!
//! * The candidates are the product's own ranking, unchanged
//!   ([`jobhunt_app::App::digest`]): strong fits only, recommended outright
//!   (verified recently at an authoritative source, eligible or
//!   conditionally eligible), never acted on, never already shown in the
//!   app or put aside, never notified before. No plausible fits.
//! * The digest is a selection over them, in rank order: one job per
//!   company, companies notified about in the last 7 days giving way when
//!   another can fill the slot, at most the person's digest size (3 by
//!   default, 5 at most). Never padded: one candidate makes a one-job
//!   digest, none makes no email (no "nothing new", no recaps).
//! * When: at most one digest per local calendar day, at the person's
//!   digest time on their days, never inside quiet hours or while paused,
//!   never late ([`crate::digest`]).
//! * Each job says how fresh it is, honestly
//!   ([`jobhunt_app::freshness`]): an old posting is labelled with its
//!   age, never "new".
//! * Emailing a job does not consume Today: nothing is recorded as shown.
//!
//! Delivery is an outbox in Postgres (`jobhunt_storage::postgres::notify`):
//! the rendered message and the opportunities it covers are committed
//! first, then sent with the delivery id as the provider's idempotency key,
//! then marked sent (moving the account's notification cursor). A provider
//! failure leaves it pending, retried with backoff by later runs; a retry
//! after a crash sends the same stored message with the same key, which the
//! provider deduplicates. A delivery still unsent after
//! [`STALE_AFTER_HOURS`] is abandoned rather than sent late (and possibly
//! twice, once the provider has forgotten its key).
//!
//! Workers coordinate through leases on accounts and deliveries (`FOR
//! UPDATE SKIP LOCKED`), like discovery and verification: two workers never
//! compose for the same account at once, and an opportunity can be put in a
//! person's notification only once (a primary key), whatever happens.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use jobhunt_app::feed::{
    DIGEST_COMPANY_SPACING_DAYS, DigestRequest, NotificationCandidate, Omitted, company_key,
};
use jobhunt_app::{App, AppError, DiscoveryMode};
use jobhunt_storage::postgres::{
    Delivery, DeliveryKind, NewDelivery, NotifiedItem, NotifyAccount, PgStore, WorkerKind,
    new_delivery_id,
};
use serde::Serialize;
use tracing::Instrument;
use url::Url;

use crate::config::CloudConfig;
use crate::digest::{Schedule, Slot};
use crate::email::{EmailMessage, EmailSender, SendError};

/// A pending delivery older than this is abandoned, not sent: the news is
/// stale, and the provider (Resend keeps idempotency keys 24 hours) may no
/// longer recognize a repeat.
pub const STALE_AFTER_HOURS: i64 = 20;
/// Attempts before a delivery is given up as failed.
pub const MAX_ATTEMPTS: u32 = 6;
const LEASE: Duration = Duration::from_secs(10 * 60);
const ABANDON_RUNS_AFTER: Duration = Duration::from_secs(3600);

/// What a notification run did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct NotifySummary {
    /// Accounts with notifications on that were examined.
    pub accounts: usize,
    /// Of those, with no digest due now (another time or day, quiet hours,
    /// today's already used or missed).
    pub not_due: usize,
    /// Of those, paused.
    pub paused: usize,
    /// Of those, due, with nothing worth an email (the day's slot is used:
    /// nothing is sent later that day).
    pub nothing_new: usize,
    /// Emails composed (written to the outbox).
    pub composed: usize,
    /// Opportunities in them.
    pub opportunities: usize,
    /// Emails the provider accepted.
    pub sent: usize,
    /// Attempts that failed and will be retried.
    pub retrying: usize,
    /// Emails given up after repeated or permanent failures.
    pub failed: usize,
    /// Emails too old to send safely.
    pub abandoned: usize,
}

/// What happened to one delivery attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Sent,
    Retrying,
    Failed,
    Abandoned,
    /// Another worker finished it first.
    Lost,
}

fn backoff(attempts: u32) -> chrono::Duration {
    // 5 minutes, 10, 20, 40, 80, capped at 3 hours.
    let minutes = 5i64.saturating_mul(1i64 << attempts.min(6));
    chrono::Duration::minutes(minutes.min(180))
}

/// Sends one claimed delivery and records the outcome.
pub async fn deliver(
    store: &PgStore,
    sender: &dyn EmailSender,
    owner: &str,
    delivery: &Delivery,
    now: DateTime<Utc>,
) -> Result<Outcome, AppError> {
    if now - delivery.created_at > chrono::Duration::hours(STALE_AFTER_HOURS)
        || delivery.expires_at.is_some_and(|by| now >= by)
    {
        store.delivery_abandoned(owner, &delivery.id).await?;
        tracing::warn!(delivery = %delivery.id, "notification abandoned: too old to send safely");
        return Ok(Outcome::Abandoned);
    }
    let message: EmailMessage = match serde_json::from_slice(&delivery.message) {
        Ok(m) => m,
        Err(e) => {
            store
                .delivery_failed(
                    owner,
                    &delivery.id,
                    &format!("unreadable message: {e}"),
                    None,
                )
                .await?;
            return Ok(Outcome::Failed);
        }
    };
    match sender.send(&message, &delivery.id).await {
        Ok(receipt) => {
            let recorded = store
                .delivery_sent(
                    owner,
                    &delivery.id,
                    sender.name(),
                    receipt.message_id.as_deref(),
                    Utc::now(),
                )
                .await?;
            tracing::info!(delivery = %delivery.id, kind = delivery.kind.as_str(), "notification sent");
            Ok(if recorded {
                Outcome::Sent
            } else {
                Outcome::Lost
            })
        }
        Err(SendError::Retryable(error)) if delivery.attempts + 1 < MAX_ATTEMPTS => {
            let retry_at = now + backoff(delivery.attempts);
            store
                .delivery_failed(owner, &delivery.id, &error, Some(retry_at))
                .await?;
            tracing::warn!(delivery = %delivery.id, %error, %retry_at, "notification not sent; will retry");
            Ok(Outcome::Retrying)
        }
        Err(e) => {
            store
                .delivery_failed(owner, &delivery.id, &e.to_string(), None)
                .await?;
            tracing::warn!(delivery = %delivery.id, error = %e, "notification failed");
            Ok(Outcome::Failed)
        }
    }
}

/// Sends every due delivery (of one account, or all).
pub async fn deliver_due(
    store: &PgStore,
    sender: &dyn EmailSender,
    owner: &str,
    only: Option<&jobhunt_storage::postgres::UserId>,
    summary: &mut NotifySummary,
) -> Result<(), AppError> {
    loop {
        let due = store
            .claim_deliveries(owner, 20, LEASE, Utc::now(), only)
            .await?;
        if due.is_empty() {
            return Ok(());
        }
        for d in &due {
            match deliver(store, sender, owner, d, Utc::now()).await? {
                Outcome::Sent => summary.sent += 1,
                Outcome::Retrying => summary.retrying += 1,
                Outcome::Failed => summary.failed += 1,
                Outcome::Abandoned => summary.abandoned += 1,
                Outcome::Lost => {}
            }
        }
    }
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

fn link(web: &Url, path: &str) -> String {
    format!("{}{path}", web.as_str().trim_end_matches('/'))
}

/// Where and how: "Remote (Americas)", "Lisbon, Portugal · hybrid".
fn place(c: &NotificationCandidate) -> Option<String> {
    let item = &c.item.item;
    let mut parts: Vec<String> = item.locations.iter().take(2).cloned().collect();
    if let Some(w) = &item.workplace
        && !parts.iter().any(|p| p.to_lowercase().contains(w.as_str()))
    {
        parts.push(w.clone());
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// Up to two of the ranking's own reasons the job may be worth the
/// person's time, said tersely: the brief's line without the posting
/// excerpt quoted after it, without the clause addressed to the person
/// ("…, the kind of company you want", "…, matching your latest title")
/// and without a leading article. "An early-stage company, the kind of
/// company you want: “…”" reads "Early-stage company". A line that still
/// speaks about the person is left out. Nothing is added or reinterpreted;
/// the full lines are on the job's page. They speak about the job (by
/// design never pay, remote work or freshness); eligibility details and
/// caveats stay in the app.
pub fn reasons(c: &NotificationCandidate) -> Vec<String> {
    c.item
        .item
        .why
        .iter()
        .filter_map(|reason| terse(reason))
        .take(2)
        .collect()
}

/// Whether `text` speaks to the person ("you", "your", "you're", …).
fn addresses_person(text: &str) -> bool {
    text.split(|c: char| !c.is_alphanumeric() && c != '\'' && c != '’')
        .any(|w| {
            let w = w.to_lowercase();
            w == "you"
                || w == "your"
                || w == "yours"
                || w.starts_with("you'")
                || w.starts_with("you’")
        })
}

/// One brief line, tersely (see [`reasons`]); `None` when what is left
/// still addresses the person.
fn terse(reason: &str) -> Option<String> {
    let mut text = match reason.split_once(": “") {
        Some((lead, quoted)) if quoted.ends_with('”') => lead,
        _ => reason,
    }
    .replace(" the way you have", "");
    // The clause after the last comma that speaks to the person.
    if let Some((evidence, tail)) = text.rsplit_once(", ")
        && addresses_person(tail)
    {
        text = evidence.to_owned();
    }
    let text = text
        .strip_prefix("A ")
        .or_else(|| text.strip_prefix("An "))
        .map_or(text.clone(), |rest| {
            let mut chars = rest.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect())
                .unwrap_or_default()
        });
    let text = text.trim().to_owned();
    (!text.is_empty() && !addresses_person(&text)).then_some(text)
}

/// "1 job", "3 jobs".
fn jobs(n: usize) -> String {
    if n == 1 {
        "1 job".to_owned()
    } else {
        format!("{n} jobs")
    }
}

/// The daily digest: the jobs in rank order, each with where, how fresh
/// and why, and a link to it in the app. Restrained on purpose: no match
/// percentages, no superlatives.
pub fn render_digest(
    from: &str,
    to: &str,
    web: &Url,
    items: &[NotificationCandidate],
    now: DateTime<Utc>,
) -> EmailMessage {
    let subject = format!("Narrow found {} worth a look", jobs(items.len()));
    let settings = link(web, "/settings#notifications");
    let intro = format!("Narrow found {} worth reviewing today.", jobs(items.len()));
    let closing = "Some days Narrow sends nothing. That's on purpose.";
    let muted = "color:#5f5a52;font-family:system-ui,sans-serif;font-size:14px";
    let mut text = format!("{intro}\n");
    let mut html = format!(
        "<div style=\"font-family:system-ui,sans-serif;max-width:560px;color:#1c1b19;line-height:1.5\">\
         <p style=\"font-size:16px\">{}</p>",
        escape(&intro)
    );
    for (i, c) in items.iter().enumerate() {
        let item = &c.item.item;
        let url = link(web, &format!("/opportunities/{}", item.id));
        let pay = c.pay.as_ref().map(|p| p.compact());
        let facts: Vec<String> = place(c).into_iter().chain(pay).collect();
        let fresh = c.freshness.label(&item.company, now);
        let why = reasons(c);
        text.push_str(&format!("\n{}. {}\n{}\n", i + 1, item.company, item.title));
        if !facts.is_empty() {
            text.push_str(&format!("{}\n", facts.join(" · ")));
        }
        text.push_str(&format!("{fresh}\n"));
        if !why.is_empty() {
            text.push_str("\nWhy Narrow picked it:\n");
            for reason in &why {
                text.push_str(&format!("{reason}\n"));
            }
        }
        text.push_str(&format!("\nView job: {url}\n"));

        html.push_str(&format!(
            "<div style=\"border-top:1px solid #e4e0d8;padding:14px 0\">\
             <p style=\"margin:0;{muted}\">{}. {}</p>\
             <p style=\"margin:0;font-size:17px\"><a href=\"{}\" style=\"color:#1c1b19\">{}</a></p>",
            i + 1,
            escape(&item.company),
            escape(&url),
            escape(&item.title),
        ));
        if !facts.is_empty() {
            html.push_str(&format!(
                "<p style=\"margin:2px 0 0;{muted}\">{}</p>",
                escape(&facts.join(" · "))
            ));
        }
        html.push_str(&format!(
            "<p style=\"margin:2px 0 8px;{muted}\">{}</p>",
            escape(&fresh)
        ));
        if !why.is_empty() {
            html.push_str(&format!(
                "<p style=\"margin:0 0 4px;font-family:system-ui,sans-serif;font-size:14px\">\
                 <strong>Why Narrow picked it:</strong> {}</p>",
                escape(&why.join(" · "))
            ));
        }
        html.push_str(&format!(
            "<p style=\"margin:6px 0 0;font-family:system-ui,sans-serif;font-size:14px\">\
             <a href=\"{}\">View job</a></p></div>",
            escape(&url)
        ));
    }
    let footer = format!(
        "You get this because the daily digest is on in Narrow. Change when it comes, pause it \
         or turn it off: {settings}"
    );
    text.push_str(&format!("\n{closing}\n\n—\n{footer}\n"));
    html.push_str(&format!(
        "<p style=\"border-top:1px solid #e4e0d8;padding-top:12px;{muted}\">{}</p>\
         <p style=\"color:#5f5a52;font-family:system-ui,sans-serif;font-size:12px\">You get this \
         because the daily digest is on in Narrow. <a href=\"{}\">Change when it comes, pause it or \
         turn it off</a>.</p></div>",
        escape(closing),
        escape(&settings)
    ));
    EmailMessage {
        from: from.to_owned(),
        to: to.to_owned(),
        subject,
        text,
        html,
        headers: vec![("List-Unsubscribe".into(), format!("<{settings}>"))],
    }
}

/// The email that confirms an address.
pub fn render_confirmation(from: &str, to: &str, web: &Url, token: &str) -> EmailMessage {
    let url = link(web, &format!("/settings/confirm?token={token}"));
    let text = format!(
        "Confirm this address to get Narrow's daily digest of jobs worth a look:\n\n\
         {url}\n\nThe link works for 48 hours, while you are signed in. If you did not ask for \
         this, ignore it: nothing will be sent.\n"
    );
    let html = format!(
        "<div style=\"font-family:system-ui,sans-serif;max-width:520px;color:#1c1b19;line-height:1.5\">\
         <p>Confirm this address to get Narrow's daily digest of jobs worth a look.</p>\
         <p><a href=\"{0}\">Confirm the address</a></p>\
         <p style=\"color:#5f5a52;font-size:13px\">The link works for 48 hours, while you are \
         signed in. If you did not ask for this, ignore it: nothing will be sent.</p></div>",
        escape(&url)
    );
    EmailMessage {
        from: from.to_owned(),
        to: to.to_owned(),
        subject: "Confirm your email for Narrow notifications".into(),
        text,
        html,
        headers: Vec::new(),
    }
}

/// Composes (at most) the day's digest for an account and sends it, when
/// its slot is due. Returns how many opportunities it covered (0: nothing
/// sent).
async fn compose(
    store: &PgStore,
    config: &CloudConfig,
    sender: &dyn EmailSender,
    owner: &str,
    account: &NotifyAccount,
    now: DateTime<Utc>,
    summary: &mut NotifySummary,
) -> Result<usize, AppError> {
    let schedule = Schedule::of(&account.digest);
    let (day, slot_at) = match schedule.check(account.last_digest_on, now) {
        Slot::Due { day, at } => (day, at),
        Slot::Paused => {
            summary.paused += 1;
            return Ok(0);
        }
        _ => {
            summary.not_due += 1;
            return Ok(0);
        }
    };
    let (Some(web), Some(email)) = (&config.web_url, &config.email) else {
        return Err(AppError::Config(
            "notifications need JOBHUNT_WEB_URL and an email provider".into(),
        ));
    };
    // The day's slot is used now, whatever is found: at most one digest
    // per local day, and nothing sent later because something appeared.
    if !store.claim_digest_day(&account.user, day).await? {
        summary.not_due += 1;
        return Ok(0);
    }
    // The shortlist is reviewed here too: after discovery, in the
    // background, so Today's requests find the reviews stored.
    let app = App::from_parts(
        Arc::clone(&config.app),
        Arc::new(store.for_user(account.user.clone())),
        DiscoveryMode::Background,
    )
    .with_fit_review(crate::config::fit_review(config));
    let already = store.notified_opportunities_all(&account.user).await?;
    let recent: std::collections::HashSet<String> = store
        .recently_notified_companies(
            &account.user,
            now - chrono::Duration::days(DIGEST_COMPANY_SPACING_DAYS),
        )
        .await?
        .iter()
        .map(|c| company_key(c))
        .collect();
    let size = usize::from(account.digest.size).clamp(1, config.notify.max_items);
    let digest = app
        .digest(
            &DigestRequest {
                exclude: &already,
                size,
                recent_companies: &recent,
            },
            now,
        )
        .await?;
    let omitted = |why: Omitted| {
        digest
            .pool
            .iter()
            .filter(|e| e.omitted == Some(why))
            .count()
    };
    tracing::info!(
        user = %account.user,
        candidates = digest.pool.len(),
        items = digest.items.len(),
        same_company = omitted(Omitted::SameCompany),
        recent_company = omitted(Omitted::RecentCompany),
        full = omitted(Omitted::Full),
        "digest composed"
    );
    if digest.items.is_empty() {
        summary.nothing_new += 1;
        return Ok(0);
    }
    let message = render_digest(
        crate::email::from_address(email),
        &account.email,
        web,
        &digest.items,
        now,
    );
    let delivery = NewDelivery {
        id: new_delivery_id(),
        user: account.user.clone(),
        kind: DeliveryKind::Recommendations,
        message: serde_json::to_vec(&message)
            .map_err(|e| AppError::Config(format!("encoding an email: {e}")))?,
        items: digest
            .items
            .iter()
            .enumerate()
            .map(|(i, c)| NotifiedItem {
                opportunity: c.item.item.id.clone(),
                tier: serde_json::to_value(c.item.item.tier)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_else(|| "strong_fit".into()),
                job: c.job.clone(),
                content_version: c.content_version.clone(),
                ranking_version: c.ranking_version.clone(),
                position: u16::try_from(i + 1).unwrap_or(u16::MAX),
                freshness: Some(c.freshness.class.as_str().to_owned()),
            })
            .collect(),
        expires_at: Some(schedule.deliver_by(day, slot_at)),
    };
    if !store.enqueue_delivery(&delivery, now).await? {
        // Another worker notified about one of them meanwhile.
        return Ok(0);
    }
    summary.composed += 1;
    summary.opportunities += digest.items.len();
    deliver_due(store, sender, owner, Some(&account.user), summary).await?;
    Ok(digest.items.len())
}

/// `narrow worker notify`: sends due retries, then composes and sends the
/// digest of every account with notifications on whose digest is due.
pub async fn notify(
    store: &PgStore,
    config: &CloudConfig,
    sender: &dyn EmailSender,
) -> Result<NotifySummary, AppError> {
    notify_at(store, config, sender, Utc::now()).await
}

/// [`notify`], deciding which digests are due as of `now` (tests).
pub async fn notify_at(
    store: &PgStore,
    config: &CloudConfig,
    sender: &dyn EmailSender,
    now: DateTime<Utc>,
) -> Result<NotifySummary, AppError> {
    let owner = format!("notification:{}", config.instance);
    let run = store
        .start_worker_run(
            WorkerKind::Notification,
            &owner,
            ABANDON_RUNS_AFTER,
            Utc::now(),
        )
        .await?;
    let span = tracing::info_span!("worker", kind = "notification", run = run.id, worker = %owner);
    let result = notify_inner(store, config, sender, &owner, now)
        .instrument(span)
        .await;
    let (summary_json, error) = match &result {
        Ok(summary) => (serde_json::to_value(summary).unwrap_or_default(), None),
        Err(e) => (serde_json::json!({}), Some(e.public_message())),
    };
    if let Err(e) = store
        .finish_worker_run(run, &summary_json, error.as_deref(), Utc::now())
        .await
    {
        tracing::warn!(error = %e, "could not record the end of the run");
    }
    result
}

async fn notify_inner(
    store: &PgStore,
    config: &CloudConfig,
    sender: &dyn EmailSender,
    owner: &str,
    now: DateTime<Utc>,
) -> Result<NotifySummary, AppError> {
    let mut summary = NotifySummary::default();
    // Retries first: an email that failed earlier goes before new ones.
    deliver_due(store, sender, owner, None, &mut summary).await?;
    let mut claimed = Vec::new();
    let result = async {
        loop {
            let accounts = store
                .claim_notification_accounts(owner, config.notify.batch, LEASE, Utc::now())
                .await?;
            if accounts.is_empty() {
                break;
            }
            for account in accounts {
                summary.accounts += 1;
                // One account's failure does not stop the others.
                if let Err(e) =
                    compose(store, config, sender, owner, &account, now, &mut summary).await
                {
                    tracing::warn!(user = %account.user, error = %e.public_message(), "could not notify an account");
                }
                claimed.push(account.user);
            }
        }
        Ok::<(), AppError>(())
    }
    .await;
    // Leases are held for the whole run so an account is looked at once.
    for user in &claimed {
        if let Err(e) = store.release_notification_account(owner, user).await {
            tracing::warn!(error = %e, "could not release an account");
        }
    }
    result?;
    tracing::info!(
        accounts = summary.accounts,
        not_due = summary.not_due,
        paused = summary.paused,
        nothing_new = summary.nothing_new,
        composed = summary.composed,
        sent = summary.sent,
        retrying = summary.retrying,
        failed = summary.failed,
        "notification run finished"
    );
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_and_is_capped() {
        assert_eq!(backoff(0), chrono::Duration::minutes(5));
        assert_eq!(backoff(1), chrono::Duration::minutes(10));
        assert_eq!(backoff(10), chrono::Duration::minutes(180));
    }

    #[test]
    fn html_is_escaped() {
        assert_eq!(
            escape("<a href=\"x\">&'"),
            "&lt;a href=&quot;x&quot;&gt;&amp;&#39;"
        );
    }

    use chrono::TimeZone;
    use jobhunt_app::freshness::{Freshness, FreshnessClass};

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 12, 12, 0, 0).unwrap()
    }

    fn candidate(company: &str, title: &str, freshness: Freshness) -> NotificationCandidate {
        let item = serde_json::json!({
            "id": format!("opp_{}", company.to_lowercase()),
            "short_id": "opp_1",
            "title": title,
            "company": company,
            "tier": "strong_fit",
            "recommendation": "recommended",
            "summary": "Backend · senior",
            "verification": {"state": "verified_active", "trusted": true},
            "eligibility": {
                "status": "conditional",
                "headline": "Needs authorization to work in Canada"
            },
            "compensation": {
                "status": "published",
                "ranges": ["USD 150,000 – 180,000 per year"],
                "verified": true
            },
            "locations": ["Remote · Latin America"],
            "workplace": "remote",
            "why": [
                "Senior backend work, the kind of engineering you want",
                "An early-stage company, the kind of company you want: “Ando is a Seed-stage \
                 company backed by Slow Ventures.”",
                "Senior level, as you're looking for"
            ],
            "consider": ["Your minimum is USD 140,000"],
            "sources": 1,
            "next_step": "narrow why opp_1",
            "reason": "new",
            "stage": "unseen"
        });
        NotificationCandidate {
            item: serde_json::from_value(item).unwrap(),
            job: "job_1".into(),
            content_version: "c1".into(),
            ranking_version: "r1".into(),
            freshness,
            pay: Some(jobhunt_jobs::verification::PayRange {
                kind: "salary".into(),
                label: None,
                currency: jobhunt_jobs::verification::CurrencyEvidence::Code { code: "USD".into() },
                min: Some(28.0),
                max: Some(58.0),
                interval: Some(jobhunt_jobs::PayInterval::Hour),
            }),
        }
    }

    fn posted(days: i64) -> Freshness {
        Freshness {
            class: FreshnessClass::Posted,
            at: Some(now() - chrono::Duration::days(days)),
            approximate: false,
        }
    }

    fn digest(items: &[NotificationCandidate]) -> EmailMessage {
        let web = Url::parse("https://app.jobhunt.test/").unwrap();
        render_digest("N <n@j.test>", "ana@example.com", &web, items, now())
    }

    #[test]
    fn a_digest_numbers_jobs_and_says_how_fresh_and_why() {
        let items = [
            candidate(
                "Ando Technologies",
                "Senior Backend Engineer (Contract to Hire)",
                posted(2),
            ),
            candidate(
                "Oneleet",
                "Fullstack Engineer, AI Implementation",
                Freshness {
                    class: FreshnessClass::Backfill,
                    at: Some(now() - chrono::Duration::days(342)),
                    approximate: false,
                },
            ),
            candidate(
                "Veda",
                "Backend Engineer - Solana",
                Freshness {
                    class: FreshnessClass::Unknown,
                    at: None,
                    approximate: false,
                },
            ),
        ];
        let m = digest(&items);
        assert_eq!(m.subject, "Narrow found 3 jobs worth a look");
        let text = &m.text;
        assert!(
            text.starts_with("Narrow found 3 jobs worth reviewing today.\n"),
            "{text}"
        );
        for expected in [
            "1. Ando Technologies\nSenior Backend Engineer (Contract to Hire)\nRemote · Latin America · USD 28–58/hr\n",
            "Posted 2 days ago",
            "2. Oneleet\nFullstack Engineer, AI Implementation",
            "Posted 342 days ago",
            "3. Veda",
            "Posted date unknown",
            "Why Narrow picked it:\nSenior backend work\nEarly-stage company\n",
            "View job: https://app.jobhunt.test/opportunities/opp_ando technologies",
            "Some days Narrow sends nothing. That's on purpose.",
            "https://app.jobhunt.test/settings#notifications",
        ] {
            assert!(text.contains(expected), "{expected:?} in {text}");
        }
        assert!(m.html.contains("Posted 342 days ago"));
        assert!(m.html.contains(">View job</a>"));
        // At most two reasons; eligibility details and the person's own
        // requirements stay in the app.
        for absent in [
            "Senior level",
            "kind of",
            "you want",
            "Slow Ventures",
            "per hour",
            "authorization",
            "Your minimum",
            "Consider",
        ] {
            assert!(!text.contains(absent), "{absent} in {text}");
            assert!(!m.html.contains(absent), "{absent} in html");
        }
        // An old job is never called new, and nothing oversells.
        let lower = format!("{} {} {}", m.subject, text, m.html).to_lowercase();
        for banned in [
            "new",
            "fresh",
            "just posted",
            "perfect",
            "best match",
            "% match",
            "unusually",
            "top ",
            "ai picked",
            "strong match",
        ] {
            assert!(!lower.contains(banned), "{banned:?} in {lower}");
        }
    }

    #[test]
    fn reasons_state_the_evidence_tersely() {
        for (line, terse_line) in [
            (
                "Senior backend work, the kind of engineering you want",
                Some("Senior backend work"),
            ),
            (
                "An early-stage company, the kind of company you want: “We just raised.”",
                Some("Early-stage company"),
            ),
            (
                "A small company of 35 people, the kind of company you're looking for: “…”",
                Some("Small company of 35 people"),
            ),
            (
                "Full-stack work that includes the backend, close to the work you want: “…”",
                Some("Full-stack work that includes the backend"),
            ),
            (
                "Touches infrastructure (the team: Platform), close to the backend work you've favored",
                Some("Touches infrastructure (the team: Platform)"),
            ),
            (
                "Senior level, matching your latest title",
                Some("Senior level"),
            ),
            ("Senior level, as you're looking for", Some("Senior level")),
            (
                "A founding role: senior scope, matching your latest title",
                Some("Founding role: senior scope"),
            ),
            (
                "Uses Kafka the way you have (as a tool), not stream processing internals",
                Some("Uses Kafka (as a tool), not stream processing internals"),
            ),
            (
                "Backend and platform work",
                Some("Backend and platform work"),
            ),
            ("Remote for Latin America", Some("Remote for Latin America")),
            // Still about the person once simplified: left out.
            ("Something you said you want", None),
        ] {
            assert_eq!(terse(line).as_deref(), terse_line, "{line}");
        }
        // At most two, skipping lines that can't be said tersely.
        let mut c = candidate("Acme", "Engineer", posted(1));
        c.item.item.why = vec![
            "Something you said you want".into(),
            "Infrastructure ownership, the kind of engineering you want".into(),
            "Small engineering team".into(),
            "Remote for Latin America".into(),
        ];
        assert_eq!(
            reasons(&c),
            ["Infrastructure ownership", "Small engineering team"]
        );
    }

    #[test]
    fn a_one_job_digest_and_fresh_labels() {
        let newly = Freshness {
            class: FreshnessClass::NewlyListed,
            at: Some(now() - chrono::Duration::hours(3)),
            approximate: false,
        };
        let m = digest(&[candidate("RevenueCat", "Backend Engineer", newly)]);
        assert_eq!(m.subject, "Narrow found 1 job worth a look");
        assert!(
            m.text
                .starts_with("Narrow found 1 job worth reviewing today.")
        );
        assert!(
            m.text
                .contains("New on RevenueCat's job board · first seen 3h ago"),
            "{}",
            m.text
        );
        let hours = Freshness {
            class: FreshnessClass::Posted,
            at: Some(now() - chrono::Duration::hours(4)),
            approximate: false,
        };
        assert!(
            digest(&[candidate("Acme", "Engineer", hours)])
                .text
                .contains("Posted 4h ago")
        );
    }

    #[test]
    fn confirmation_links_to_the_web_app() {
        let web = Url::parse("https://app.jobhunt.test/").unwrap();
        let m = render_confirmation("J <n@j.test>", "ana@example.com", &web, "abc");
        assert!(
            m.text
                .contains("https://app.jobhunt.test/settings/confirm?token=abc")
        );
        assert_eq!(m.to, "ana@example.com");
    }
}
