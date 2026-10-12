# Email digest: notification policy v2 (BRU-389 slice A)

Email is the first channel of Narrow's proactive experience
([`proactive-agent-experience.md`](proactive-agent-experience.md), §12–§14,
§27 A). This slice changes **how existing recommendations are selected and
delivered** by email. It does not change ranking, Fit, eligibility, Today,
the sources, onboarding or applications, and adds no channel.

> Once a day, Narrow quietly sends at most a few jobs worth reviewing.
> Nothing when nothing qualifies.

## 1. Old policy (BRU-295)

- Candidates: `App::notification_candidates`: Strong fit, `Gate::Recommended`,
  unseen, no feed mark (never shown, never put aside), never notified.
- Up to `JOBHUNT_NOTIFY_MAX_ITEMS` (3) per email, in rank order, **no
  company rule** (three Railway roles could fill one email).
- When: `immediate` = at most one email every
  `JOBHUNT_NOTIFY_MIN_INTERVAL_HOURS` (4) whenever something new appeared;
  `daily` = at most one per 24 hours (a fixed interval, not a calendar day).
  No timezone, quiet hours or pause.
- Copy: "A strong new match: … at …" / "3 new jobs worth your time", with
  a "Consider:" caveat that could carry eligibility details.
- No freshness: a job was "new" because Narrow had just read it.

## 2. New policy

| | |
|---|---|
| Pool | unchanged (§3) |
| Selection | rank order, one job per company, 7-day company spacing as a preference (§4) |
| Size | the person's digest size: 3 by default, 1–5 |
| When | at most one digest per **local calendar day**, at the digest time on the digest days (§5) |
| Never | inside quiet hours (§6), while paused (§7), late, or to catch up |
| Empty | nothing is sent; no "nothing today", recap or reminder |
| Copy | numbered jobs, an honest freshness line, at most two reasons (§9) |

Code: `jobhunt_app::feed::{LocalApp::digest, select_digest}`,
`jobhunt_app::freshness`, `jobhunt_cloud::digest` (schedule),
`jobhunt_cloud::notify::{compose, render_digest}`.

## 3. Candidate pool

Exactly the pool BRU-295 already used, now `LocalApp::digest`:

- `Tier::StrongFit` and `Gate::Recommended` (trusted verification at an
  authoritative source within the freshness policy; eligible or
  conditionally eligible);
- `Stage::Unseen` (never saved, rejected, applied…);
- no feed mark: never shown on Today or in a shortlist, never put aside;
- not in `notification_items` (once, ever).

**No Plausible** ("Possible fit") jobs: BRU-328 has not passed. Nothing is
re-scored; the ranking's order is kept.

## 4. Company diversity

`select_digest` walks the pool in rank order:

1. the first job of each company is its candidate; the company's other
   jobs are left out (`same_company`);
2. companies with a job in a notification **sent in the last 7 days**
   (`notification_items` ⋈ `notification_deliveries` ⋈ `jobs.company`) go
   after the others: they get a slot only if no other company can fill it
   (`recent_company` when they lose it);
3. at most `digest_size` companies (`full` for the rest); the picks keep
   rank order.

Companies are compared like Today does (`search_key` of the company).
A recent company is never blocked: with nobody else qualifying it is sent.
Example (size 3): Railway #1, Railway #2, Ando, Railway #3, Veda, Oneleet →
**Railway #1, Ando, Veda**.

## 5. Schedule

Settings (all local to the person):

| Field | Default | Range |
|---|---|---|
| `timezone` | the browser's, saved when the digest is turned on; **UTC** until set | IANA name |
| `digest_time` | 09:00 | outside quiet hours |
| `digest_days` | Mon–Fri | at least one day |
| `digest_size` | 3 | 1–5 (`JOBHUNT_NOTIFY_MAX_ITEMS` caps it, default 5) |
| `quiet_start`–`quiet_end` | 21:00–08:00 | equal = none |
| `paused_until` | none | ≤ 1 year ahead |

The invariant is **at most one digest per local calendar day**, not "every
24 hours" (`jobhunt_cloud::digest::Schedule::check`):

- The worker still runs every 15 minutes. A slot is due from the digest
  time to 60 minutes after it (`GRACE_MINUTES`).
- When due, the worker **claims the local date** (`UPDATE … SET
  last_digest_on = $day WHERE last_digest_on < $day`) *before* composing,
  and keeps it whether or not anything qualifies. A second worker, a
  second run, a 23- or 25-hour day, or a job appearing at 09:20 cannot make
  a second digest that day.
- **DST**: the slot is the digest time on that local date; when the time
  happens twice (fall back) the earlier one, when it is skipped (spring
  forward) the first valid time after it (02:30 → 03:00). Tested for New
  York, London, Berlin, Sydney, Kolkata, Chatham and São Paulo by
  simulating a run every 15 minutes across the changes.
- **No catch-up**: a slot not used within the grace window is gone; the
  next digest is the next slot.
- **Delivery deadline**: a composed digest carries `expires_at` = the
  earliest of slot + 6 h, the start of quiet hours and the local midnight.
  Provider retries after it are `abandoned` (the existing 20 h staleness
  rule still applies to everything).

## 6. Quiet hours

- Nothing is composed while the local time is inside quiet hours
  (`Slot::Quiet`), and no retry delivers after quiet hours start
  (`expires_at`).
- A digest time inside quiet hours is **refused** by the API (400, "the
  digest time 07:00 is inside your quiet hours (21:00–08:00); choose a time
  outside them"), and the Settings form says the same and disables Save.
  Nothing is silently moved to another hour. The check runs on the merged
  settings, so changing only the quiet hours is validated too.

## 7. Pause

`paused_until`: no slot is used while `now < paused_until`. When it ends,
the schedule simply resumes; slots that fell inside the pause are never
sent. Settings offer "Pause for a week" and "Resume"; the API accepts any
instant up to a year ahead, or `null` to resume.

## 8. Freshness classes

`jobhunt_app::freshness::Freshness::classify(record, history,
previous_complete_listing)`, from existing data only (`jobs.posted_at`,
`jobs.first_seen_at`, `job_events`, `source_scans`); one new store query,
`Store::complete_listing_before(source, t)`: the last complete read of the
board (a complete listing, or a 304 confirming one) that finished before
`t`.

| Class | Evidence | Label |
|---|---|---|
| `reopened` | the latest New/Reopened event is Reopened | "Reopened 2 days ago" |
| `backfill` | no complete read of the board before the job was first seen: it was in the board's first listing | its board date if any ("Posted 342 days ago", Lever: "Listed …"), else "Posted date unknown" |
| `posted` | Greenhouse/Ashby `posted_at` | "Posted 4h ago", "Posted 9 days ago" |
| `posted_approx` | another source's `posted_at` (Lever's `createdAt`) | "Listed 4h ago" |
| `newly_listed` | no `posted_at`, and the previous complete read was ≤ 12 h before it was first seen | "New on {company}'s job board · first seen 3h ago" |
| `unknown` | no `posted_at`, previous complete read too old (a gap) | "Posted date unknown" |

Precedence is top to bottom. A backfill is **never** called new, fresh or
just posted, whatever its date; the class is kept so a future alert can
refuse it. Old jobs are **not** hidden: Oneleet's "Fullstack Engineer, AI
Implementation" is labelled "Posted 342 days ago" (or whatever its age is
on the day), which is what the experiment should learn from.

## 9. Rendering

Subject: "Narrow found 3 jobs worth a look" ("1 job").

```text
Narrow found 3 jobs worth reviewing today.

1. Ando Technologies
Senior Backend Engineer (Contract to Hire)
Latin & South America | Remote · USD 28–58/hr
Posted 56 days ago

Why Narrow picked it:
Senior backend work
Early-stage company

View job: https://narrow.fyi/opportunities/opp_…

2. …

Some days Narrow sends nothing. That's on purpose.
```

- Where (locations, workplace) and the job's **published** pay range,
  when there is one, compact (`PayRange::compact`, presentation only):
  "USD 28–58/hr", "USD 120k–160k/yr", "BRL 25k–32k/mo". The range is the
  one the job's page shows (verified facts, else the listing).
- Reasons: at most two lines of the ranking's own brief (`worth`), by
  construction fit reasons about the job (never pay, remote work or
  freshness), said tersely (`notify::reasons`): the quoted posting
  excerpt, the clause addressed to the person ("…, the kind of company
  you want", "…, matching your latest title", "…, as you're looking for")
  and a leading article are dropped, so "An early-stage company, the kind
  of company you want: “…”" reads "Early-stage company". A line that
  would still speak about the person is skipped. No new reasons, no
  reinterpretation; the full lines are on the job's page.
- Removed: the "Consider:" line (it could carry eligibility or
  authorization details and the person's own requirements).
- Never: match percentages, "perfect/best match", "unusually strong", "AI
  picked this", "top X%" (tested).
- The footer links to `/settings#notifications`, also the
  `List-Unsubscribe` target as before.

## 10. Existing subscribers

Additive migration `20261012000000_digest_policy.sql`; no data rewritten.

| Before | After |
|---|---|
| `cadence = 'daily'` (≤ 1 email / 24 h) | daily digest, defaults |
| `cadence = 'immediate'` (≤ 1 email / 4 h, any new strong fit) | **daily digest**, defaults. Instant alerts are a separate gated feature (BRU-389 §13.1), not built |
| no timezone | UTC until set (the Settings page saves the browser's when the digest is turned on, and offers it otherwise) |
| address, confirmation, on/off | unchanged |
| `notification_items` (once ever) | unchanged |

`cadence` is kept (still accepted by the API, shown as legacy) so the
"immediate" choice is not lost; nothing reads it. Settings show
"Narrow now sends one digest a day instead of an email per match" to
subscribers with `immediate`. `JOBHUNT_NOTIFY_MIN_INTERVAL_HOURS` is no
longer read.

**In production today there is no `worker-notify` service** (the IaC
declares one; it was never created), so no recommendation email is sent
now and this change affects no one until that cron exists.

## 11. Dogfood dry run

Run 2026-10-12T00:22Z, and again at 01:33Z after the copy polish (same
pool and digest; 3 jobs had closed in between, 3,431 open, still 7
strong), with
`cargo run --release -p jobhunt-cloud --example digest_dry_run`, which
calls the production selection (`LocalApp::digest`) and renderer
(`render_digest`) over a private SQLite mirror: the dogfood profile synced
from the cloud plus the 31 production boards (`deploy/cloud.toml`). No
email was sent and nothing was written to the cloud.

**Inventory** (rescanned just before the run): 31 sources (29 answered
"not modified", 2 re-listed unchanged; 0 new, 0 closed since the Slice 0
snapshot), 3,434 open, 435 actionable, 149 plausible, **7 strong**. Today:
4 companies / 7 jobs (Ando, Oneleet, Veda, Railway + 3). Production's own
Today at 23:44Z counted 3,433 considered, 7 strong, 4 shown.

**Strong jobs and the pool.** The 7 Strong jobs were verified at their
Ashby boards first (all active), as the verification worker does:

| Rank | Company | Title | Gate | In pool? |
|---:|---|---|---|---|
| 1 | Ando Technologies | Senior Backend Engineer (Contract to Hire) | recommended | yes |
| 2 | Oneleet | Fullstack Engineer, AI Implementation | recommended | yes |
| 3 | Veda | Backend Engineer - Solana | recommended | yes |
| 340–343 | Railway | Baremetal Orchestration; Observability; Storage; Infrastructure Engineer | eligibility unclear: "the posting mentions time-zone overlap without saying which zones" | **no**: not `Recommended` (Today shows them; email does not) |

Candidates before diversity: Ando, Oneleet, Veda. Removed by
one-per-company: none (the four Railway roles never reached the pool). By
7-day spacing: none (no earlier digests).

**The digest (3 by default; 5 gives the same 3):**

| # | Company | Title | Eligibility | Freshness | Label | Reasons shown |
|---|---|---|---|---|---|---|
| 1 | Ando Technologies | Senior Backend Engineer (Contract to Hire) | eligible ("Brazil is within the listed South America region") | backfill | Posted 56 days ago | Senior backend work · Early-stage company |
| 2 | Oneleet | Fullstack Engineer, AI Implementation | eligible ("hires employer of records from anywhere") | backfill | **Posted 342 days ago** | Full-stack work that includes the backend · Early-stage company |
| 3 | Veda | Backend Engineer - Solana | eligible ("remote from anywhere") | backfill | Posted 58 days ago | Backend work · Small company of 35 people |

All three are `backfill` (first seen in their boards' first listing,
2026-10-11 slice 0), labelled with Ashby's real publish date; none is
called new. Not shown in the email: the "pay can't be checked against
your minimum" caveat on Ando and Veda (the person's own floor), Oneleet's
third reason, and every eligibility headline.

**But in production this digest is empty.** The dogfood user opened Today
at 23:44Z, which recorded Ando, Oneleet, Veda and Railway's first role as
shown. Seen in the app means not emailed (one canonical state), so with
those four marks the pool is empty and **no email would be sent**
(`dry run … no 3 <the four opp ids>`). The next digest will hold only
Strong, Recommended jobs that appear and are not opened on Today before
09:00.

The email of the mirror run, verbatim:

```text
Subject: Narrow found 3 jobs worth a look

Narrow found 3 jobs worth reviewing today.

1. Ando Technologies
Senior Backend Engineer (Contract to Hire)
Latin & South America | Remote · USD 28–58/hr
Posted 56 days ago

Why Narrow picked it:
Senior backend work
Early-stage company

View job: https://narrow.fyi/opportunities/opp_d0fec14378806a6e6cfd6931a0e53e70

2. Oneleet
Fullstack Engineer, AI Implementation
US/EU - Remote · USD 110k–180k/yr
Posted 342 days ago

Why Narrow picked it:
Full-stack work that includes the backend
Early-stage company

View job: https://narrow.fyi/opportunities/opp_234250376fc406874a13f77256aa2283

3. Veda
Backend Engineer - Solana
Worldwide · remote
Posted 58 days ago

Why Narrow picked it:
Backend work
Small company of 35 people

View job: https://narrow.fyi/opportunities/opp_9d88a119134f70d3eb9c16d042d8398a

Some days Narrow sends nothing. That's on purpose.

—
You get this because the daily digest is on in Narrow. Change when it comes, pause it or turn it off: https://narrow.fyi/settings#notifications
```

An observation for the experiment, not changed here: Railway's four roles
are on Today but can never be emailed while their eligibility is unclear.

## 12. Rollout and rollback

Rollout:

1. Merge. Railway deploys `api` (the migration runs as its pre-deploy
   `narrow migrate`) and the two workers; Vercel deploys the web app.
   During the few minutes the new web app may talk to the old API, saving
   the schedule fails with a 400 (unknown fields); the page renders
   without the new fields.
2. Check the new `api` deployment for the commit is SUCCESS (not just
   `/ready`), and `GET /api/v1/notifications` returns `digest_time`.
3. Open Settings once as the dogfood user: turn the digest on (saves the
   browser timezone) and check "Next digest …".
4. Only then, if wanted, create `worker-notify` (cron `12,27,42,57 * * *
   *`, `narrow worker notify`, the env in `.railway/railway.ts`).
5. After the first digest: `worker_runs` (`kind='notification'`) summary
   and the `digest composed` log line.

Rollback:

- Fastest: delete/disable the `worker-notify` cron (or turn the digest off
  in Settings). Nothing else sends recommendation email.
- Code: revert the PR. The migration is additive; the old code ignores the
  new columns (its `INSERT`s name their columns, `items`/`deliveries` new
  columns are nullable or defaulted), so no down-migration is needed.
  Reverting brings back the 4-hour `immediate` behavior.

## 13. Metrics available for slice I

Already stored, per delivery and item, without new analytics:

| Need | Where |
|---|---|
| delivery id, user, kind | `notification_deliveries.id`, `user_id`, `kind` |
| sent time, status, attempts, provider id | `sent_at`, `status`, `attempts`, `provider_message_id` |
| items per digest (1/2/3…) | `notification_deliveries.items` |
| opportunities, job record, versions | `notification_items.opportunity_id`, `job_id`, `content_version`, `ranking_version` |
| company | `notification_items.job_id` → `jobs.company` |
| tier | `notification_items.tier` |
| position in the email | `notification_items.position` (new) |
| freshness class | `notification_items.freshness` (new) |
| deadline | `notification_deliveries.expires_at` (new) |
| candidates, omissions by reason | log line `digest composed` (`candidates`, `items`, `same_company`, `recent_company`, `full`) |
| runs | `worker_runs.summary` (`accounts`, `not_due`, `paused`, `nothing_new`, `composed`, `sent`, …) |

Outcomes (save/reject/applied after delivery) are in the `feedback` log,
joinable on opportunity and time. Rank position within the ranking is not
stored (it is not stable across runs); `position` is the email order.
