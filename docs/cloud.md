# JobHunt Cloud

JobHunt Cloud is the local product, hosted. It keeps discovering and
verifying opportunities while your laptop is closed, gives you an account
your laptop syncs with, and serves the same use cases over an HTTP API and
a hosted MCP endpoint. It is not a second product: discovery, verification,
eligibility, ranking, taste, the profile rules and the evidence policy are
the same code the `narrow` command runs, reached through the same
application layer.

- [Architecture](#architecture)
- [Processes](#processes)
- [Data: shared corpus and private data](#data-shared-corpus-and-private-data)
- [Accounts and authentication](#accounts-and-authentication)
- [Encryption](#encryption)
- [HTTP API](#http-api)
- [Hosted MCP](#hosted-mcp)
- [The web app](#the-web-app)
- [The Today feed](#the-today-feed)
- [Email notifications](#email-notifications)
- [Sync](#sync)
- [Scheduled discovery](#scheduled-discovery)
- [Scheduled verification](#scheduled-verification)
- [Coordination and locking](#coordination-and-locking)
- [Observability and usage events](#observability-and-usage-events)
- [Deploying on Railway](#deploying-on-railway)
- [Configuration reference](#configuration-reference)
- [Validating a deployment](#validating-a-deployment)
- [Known limitations](#known-limitations)

## Architecture

```text
 narrow (CLI) ───────┐                       ┌── domain crates ─────────────────────┐
 narrow mcp (stdio)─┤                       │ jobs: lifecycle, identity, discovery,│
                     ├── jobhunt-app ────────┤       verification                   │
 HTTP API /api/v1 ───┤   (use cases, views)  │ eligibility · profile · ranking      │
   └ web (Next.js) ──┘                       │                                      │
 hosted MCP /mcp ────┤          │            └──────────────────────────────────────┘
 workers (cron) ─────┘          │
                         Store (repository traits)
                          /                  \
              SqliteJobStore               PgUserStore ── PgStore (shared corpus)
              (one person, local)          (one account's view of Postgres)
```

- `jobhunt-app`'s `App` holds an `Arc<dyn Store>`. Locally that is the
  SQLite file; in the cloud each request gets an `App` over
  `PgStore::for_user(account)`, a view of Postgres bound to one account.
  Use cases do not know which one they run on.
- `jobhunt-storage::postgres` implements every repository trait
  (`JobRepository`, `VerificationRepository`, `EligibilityRepository`,
  `FeedbackRepository`, `RankingRepository`, `ProfileRepository`). A shared
  contract suite runs the same behavior against SQLite and a real Postgres,
  including a whole scenario whose rankings must come out identical.
- `jobhunt-cloud` adds only what hosting needs: configuration, auth, the
  API, hosted MCP, workers, usage events and request tracing.
- Nothing requires an LLM: discovery, verification, eligibility and
  ranking are deterministic, and no AI API key is used.

## Processes

One binary (`narrow`), one Docker image, several modes:

| Command | Railway service | What it does |
| --- | --- | --- |
| `narrow server` | `api` | HTTP API, hosted MCP, `/health`, `/ready` |
| `narrow migrate` | `api` pre-deploy | applies pending migrations, exits |
| `narrow worker discovery` | `worker-discovery` (cron) | reads the sources that are due, exits |
| `narrow worker verification` | `worker-verification` (cron) | re-verifies jobs that matter, exits |
| `narrow worker notify` | `worker-notify` (cron) | emails strong new recommendations (and retries failed emails), exits |
| `node server.js` (apps/web) | `web` | the web app; calls `api` over the private network |
| `narrow admin status` / `reencrypt` | (run by an operator) | configuration report, schedule, usage; key rotation |

The cloud modes read their configuration from environment variables
([reference](#configuration-reference)); they never read the local config
file or database. They log JSON to stderr.

No Kubernetes, message queue, Redis or browser worker: Postgres is the
database and the only coordination mechanism, and Railway's cron runs the
workers.

## Data: shared corpus and private data

| Shared (one copy for everyone) | Private (per account) |
| --- | --- |
| `jobs` (canonical postings, lifecycle status, opportunity) | `profiles`, `profile_entities` (every profile record, encrypted) |
| `job_events` (NEW / UPDATED / CLOSED / REOPENED history) | `profile_events` (history; details encrypted) |
| `job_evidence` (cross-source identity) | `feedback` (reasons encrypted) |
| `discovery_runs`, `source_scans` (validators, counts) | `eligibility_decisions` (encrypted; they depend on the profile) |
| `job_verifications` (listing facts do not depend on who asked) | `rankings` (encrypted) |
| `source_schedule`, `verification_leases`, `worker_runs` | `user_opportunities` (what shortlists showed), `user_state` |
| | `users`, `user_identities`, `api_tokens` (hashed) |
| | `usage_events` (analytics, kept apart) |
| | `notification_settings` (the address sealed), `notification_deliveries` (the outbox; rendered messages sealed), `notification_items` |

- A Greenhouse job is discovered, stored and verified once, however many
  people it concerns. Each person's eligibility and ranking of it are
  computed against their own profile and stored under their account, so
  the same shared job can be eligible for one person and not another
  (tested).
- Every private query is bound to an account: `PgUserStore` has no method
  that reads private data without its account id. Deleting an account
  (`DELETE /api/v1/account`) deletes all of it; shared jobs stay.
- Identifiers keep the application's formats (`job_…`, `opp_…`, `clm_…`,
  `fb_…`, `usr_…`) with the `"C"` collation, so ordering matches SQLite.
  Every person's profile keeps the local profile id: records have the same
  ids on the laptop and in the cloud, which is what makes sync exact.
- Timestamps are `timestamptz` (microseconds, like the local store);
  structured values that are read whole are `jsonb` (locations,
  compensation, job history snapshots, verification records); private
  values are `bytea` ciphertext.

## Accounts and authentication

JobHunt keeps no passwords and no personal details of an account. People
sign in with an **OpenID Connect provider**. The deployment uses **WorkOS
AuthKit**: it has the device authorization grant the CLI uses, JWT
access tokens bound to a resource (RFC 8707), refresh tokens, and OAuth
for MCP clients with dynamic client registration. Nothing in the code is
specific to it; Auth0, Okta, Zitadel and Keycloak work through the same
settings. An account is an internal id (`usr_<32 hex>`) linked to the
`(issuer, subject)` of the provider's tokens.

- **Access tokens** (JWTs) are verified against the issuer's published
  keys (OpenID discovery, else RFC 8414 authorization server metadata,
  else `JOBHUNT_OIDC_JWKS_URL`; keys cached 10 minutes, and an unknown key
  id triggers one rate-limited refresh, for key rotation). Only asymmetric
  algorithms are accepted; the issuer (exactly as configured: a trailing
  slash matters), the audience (one of `JOBHUNT_OIDC_AUDIENCE`) and the
  expiry are checked (60 s leeway). The first valid token of an identity
  creates its account.
- **Personal access tokens** (`jh_pat_…`, 256 random bits) are for MCP
  clients and scripts without OAuth. They expire (default 90 days, at most
  365), are listed without their secret, and are revocable. Only their
  SHA-256 digest is stored.
- **Log out everywhere** (`narrow logout --everywhere`,
  `POST /api/v1/account/logout`) refuses every token issued before that
  moment and revokes every personal access token; it takes effect within
  30 seconds on every replica.
- A **development mode** (`JOBHUNT_AUTH_MODE=dev`, HS256 tokens minted by
  `POST /api/v1/auth/dev-token`) exists for local development and tests;
  the server refuses to start with it in production.

Internally a request becomes a `Principal { user, method }`; domain code
never sees tokens or the provider.

### The CLI

```bash
narrow login --server https://jobhunt.example.com   # opens a device code sign-in
narrow login --token < token.txt                    # a personal access token (or JOBHUNT_TOKEN)
narrow account                                      # account and sync state
narrow logout [--everywhere]
narrow token create "Claude Desktop" --days 30      # prints the secret once
narrow token list | narrow token revoke tok_…
```

`login` uses the OAuth 2.0 device authorization grant (RFC 8628): it
prints a URL and a code, you confirm in the browser, and the CLI receives
tokens. You never paste a long-lived secret into a config file. The
session is stored in the system keychain on macOS and Windows, and in an
owner-only file (`0600` in a `0700` directory under the data directory)
elsewhere, or where `JOBHUNT_CREDENTIALS_FILE` says. Access tokens are
refreshed with the refresh token when they are about to expire; `logout`
revokes the refresh token at the provider (when it has a revocation
endpoint) and deletes the session. `narrow doctor` says where the session
is stored and whether there is one, never its tokens.

## Encryption

| Layer | What | How |
| --- | --- | --- |
| Transport | clients ↔ API | HTTPS at Railway's edge; the CLI refuses plain `http` except to localhost |
| Transport | services ↔ Postgres | Railway's private network (WireGuard-encrypted) |
| Application | every profile record (resume text, experiences, claims, contacts, preferences, statements), profile history details, feedback reasons, eligibility decisions, rankings | AES-256-GCM (RustCrypto `aes-gcm`) before the value reaches Postgres |
| Infrastructure | the Postgres volume, backups | as provided by Railway; Railway's documentation does not state volume encryption at rest, which is why private data is encrypted by the application |
| Hashing | personal access tokens | SHA-256 digest only |

Application encryption (`jobhunt_storage::postgres::crypto`):

- Keys come from `JOBHUNT_ENCRYPTION_KEYS`, never from code:
  `<key id>:<base64 of 32 random bytes>`, comma-separated. Generate one with
  `openssl rand -base64 32`. The first key encrypts; every listed key
  decrypts.
- A sealed value is `version | key id | 96-bit random nonce | ciphertext
  and tag`, so it names the key it needs.
- The associated data binds each value to its table, account and record
  (`profile_entities|usr_…|prof_…|claim|clm_…`): a ciphertext copied into
  another account's row, or onto another record, fails to decrypt instead
  of being read (tested).
- **Rotation**: add a new key *first* (`k2:…,k1:…`), redeploy, run
  `narrow admin reencrypt` (idempotent; safe while serving), then remove
  the old key.
- Plain columns are only what queries need: ids, states, timestamps, a
  job's title and company on feedback (public facts about the job), and
  digests.
- Lose the keys and the private data is unreadable: keep them in a
  password manager as well as in Railway's sealed variables.

## HTTP API

Versioned under `/api/v1`. Every endpoint is one use case for the
authenticated account and answers with the same view the MCP tool of the
same name returns (`SearchResults`, `JobDetail`, `VerificationReport`,
`FeedbackResult`, `ProfileView`, `PreferenceUpdateResult`, `PipelineView`,
`ApplicationContext`, `StateExport`, and for BRU-295 `FeedView`,
`TasteView`, `ClaimReview`, `ResumeImportResult`). Request bodies reuse
the MCP tools' argument types, and reject unknown fields. The JSON Schema
of every body is generated from these Rust types into
[`apps/web/src/lib/api-schema.json`](../apps/web/src/lib/api-schema.json)
(a test fails when it is stale), and the web app's TypeScript types are
generated from it, so the web cannot drift from the server.

Clients may name themselves with `x-jobhunt-client: web | cli | mcp`; it
only tags usage events.

| Method | Path | |
| --- | --- | --- |
| GET | `/health` | liveness (no dependencies) |
| GET | `/ready` | Postgres reachable and every migration applied (503 otherwise); never checks job boards |
| GET | `/.well-known/oauth-protected-resource[/mcp]` | RFC 9728 metadata for MCP clients |
| GET | `/api/v1/auth/config` | how to sign in (issuer, CLI client id, device endpoints) |
| GET / DELETE | `/api/v1/account` | the account and its cloud state / delete it and all its data |
| POST | `/api/v1/account/logout` | revoke every session and token |
| GET, POST / DELETE | `/api/v1/tokens`, `/api/v1/tokens/{id}` | personal access tokens |
| POST | `/api/v1/sync/pull`, `/api/v1/sync/push` | [sync](#sync) |
| POST | `/api/v1/search` | the shortlist (`search_jobs` arguments) |
| GET | `/api/v1/opportunities/{id}` | details (`?include_sources`, `?full_description`) |
| POST | `/api/v1/opportunities/{id}/verify` | ask the sources now (`{"force": bool}`) |
| POST | `/api/v1/opportunities/{id}/feedback` | `{"action": "save" \| "reject" \| "applied" \| …, "reason"}` |
| GET | `/api/v1/opportunities/{id}/application-context` | usable evidence (`?include_contact_details`) |
| GET | `/api/v1/profile` | the profile, never contact details |
| POST | `/api/v1/preferences` | `update_preferences` arguments |
| GET | `/api/v1/pipeline` | `?include_rejected` |
| GET | `/api/v1/export` | the portable `jobhunt.state` file |
| GET | `/api/v1/feed` | the Today feed (`get_feed`; `?limit` 1–10, default 5): `FeedView` |
| POST | `/api/v1/opportunities/{id}/dismiss` | "not now": off the feed, recorded as looked at (no taste signal) |
| GET | `/api/v1/taste` | stated preferences and learned taste (`get_taste`): `TasteView` |
| GET, POST | `/api/v1/taste/profile` | the candidate taste profile (`get_taste_profile`): `TasteProfileView`; a `TasteAction` (describe, reinterpret, confirm, correct, neutral, remove, add) answered with `TasteUpdateResult` (`update_taste_profile`); see [taste-profile.md](taste-profile.md) |
| GET / POST | `/api/v1/profile/claims` | claims needing review, with their source words / `{"ids", "decision": "confirm" \| "reject" \| "reset", "note"}` |
| PUT | `/api/v1/profile/resume` | import or re-import a resume: the body is the file (`?file_name=resume.pdf`; PDF, text or Markdown, 10 MB) |
| GET / PUT | `/api/v1/notifications` | email notification settings (`email_enabled`, `cadence`, `email`, `resend_confirmation`) |
| POST | `/api/v1/notifications/confirm` | `{"token"}` from the confirmation link |

Errors are always `{"error": {"code", "message", "hint"}}` with the stable
codes the CLI and MCP use (`unknown_opportunity` 404, `ambiguous_id` 409,
`no_profile` 409, `conflict` 409, `invalid_arguments` 400,
`invalid_preference` 422, `source_unavailable` 503, …) plus
`unauthenticated` (401, with a `WWW-Authenticate` challenge naming the
resource metadata), `invalid_request` (malformed body or query),
`invalid_sync` (422), `not_found` and `internal_error`. Internal causes are
logged, never returned. Every answer has an `x-request-id`.

Searches in the cloud never read job boards (discovery is the workers'
job); they rank the shared corpus for the account and verify their best
candidates when needed. Verifications are shared, so a job verified for
one person minutes ago is reused for the next.

## Hosted MCP

`/mcp` serves the local MCP server's tools (same names, descriptions,
input and output schemas, structured content and error codes) over the
Streamable HTTP transport of the official Rust SDK (`rmcp` 3.4), for
remote clients such as ChatGPT or Claude's connectors.

- Every request must carry a bearer token (OAuth access token or personal
  access token). Without one the server answers 401 with
  `WWW-Authenticate: Bearer resource_metadata=".../.well-known/oauth-protected-resource/mcp"`,
  which MCP clients follow to the authorization server (RFC 9728).
- Each tool call builds the application for the authenticated account;
  there is no tool, argument or session through which another account's
  data can be reached (tested with two accounts).
- The transport is stateless (no in-memory MCP sessions; the `2026-07-28`
  protocol has none), so any replica answers any request.
- `Host` is checked against `JOBHUNT_PUBLIC_URL`, and `Origin` against
  `JOBHUNT_ALLOWED_ORIGINS` when set.
- The evidence policy is unchanged: `get_profile` never includes name or
  contacts, `prepare_application_context` only usable evidence, contacts
  only when asked.
- BRU-295 added two tools, over the same use cases as the web:
  `get_feed` (what's new: the Today feed, and it records what it showed,
  exactly like the web) and `get_taste` (stated vs learned). A web
  account, a CLI login and an MCP client signing in with the same identity
  are one account (`usr_…`), so "find me new jobs" in an assistant and
  Today in the browser show the same state.

### Connecting an assistant (for people using JobHunt)

1. In the assistant (Claude: Settings → Connectors → Add custom connector;
   ChatGPT: a custom connector in developer mode), add the remote MCP
   server `https://<api domain>/mcp`.
2. Sign in when asked: it is the same sign-in as the web app (OAuth with
   dynamic client registration). Assistants that can't do OAuth take a
   personal access token instead (Settings → "Use JobHunt from your AI
   assistant" → Create token, sent as `Authorization: Bearer jh_pat_…`).
3. Ask in plain words. How the requests map to tools:

| You say | The assistant uses |
| --- | --- |
| "Find me new jobs" | `get_feed` |
| "Show only small teams" | `update_preferences` with your words, then `get_feed` / `search_jobs` |
| "Don't show jobs like this again" | `reject_job` with your reason, verbatim (learned from) |
| "Why is this worth my time?" | `get_job` (the decision brief) |
| "Help me apply to the second one" | `prepare_application_context`: only evidence you confirmed or that is quoted from your resume; nothing is invented, no letter is written by JobHunt |

## The web app

[`apps/web`](../apps/web) is a Next.js 16 app (React 19, TypeScript,
Tailwind 4) that renders on the server and calls the API from the server
only: the browser never sees an access token, and there is no
browser-to-API traffic (so CORS is not needed for it). Details, local
development and tests: [apps/web/README.md](../apps/web/README.md).

- **Sign-in** is the API's identity provider (from
  `/api/v1/auth/config`): the OAuth 2.0 authorization code flow with
  PKCE, from the web server, asking for the API's audience. The tokens go
  into one encrypted, HttpOnly, `SameSite=Lax` cookie (AES-256-GCM JWE);
  a Next.js proxy renews the access token with the refresh token before
  it expires; a 401 from the API clears the session and asks to sign in
  again. Sign-out revokes the refresh token at the provider (and ends the
  provider's session when it has an end-session endpoint); "sign out
  everywhere" is the API's. There is no web-side user database: the
  account is the API's.
- **Mutations** are server actions that call the API and return its view
  or a stable error code, which the UI turns into words (a missing
  profile, JobHunt unreachable, a conflict, an expired session).
- **Development sign-in** (`JOBHUNT_WEB_AUTH_MODE=dev`) works only against
  an API in development auth mode, which refuses to start in production.

## The Today feed

`GET /api/v1/feed` (the `get_feed` tool) answers "what is worth my time
that I haven't dealt with?", so a visit can end. It is built from the
product's ranking (fit, practicality, gates, briefs; see
[fit-and-practicality.md](fit-and-practicality.md)); only the selection is
its own (`jobhunt_app::feed`):

| The person's state | On the feed |
| --- | --- |
| never acted on it, never shown it | **new** |
| never acted on it, first shown less than 24 hours ago | **new** (reloading doesn't make it vanish) |
| never acted on it, shown longer ago | passed over: counted, not shown |
| looked at, put aside ("not now") or saved | only if it **changed** materially since |
| rejected, applied, interviewing, offer, closed, ineligible | never (the ranking's gate) |

- Only strong fits are candidates (since BRU-322; jobs worth reviewing stay
  in search). With nothing new, the feed is **caught up**: empty, with when
  job boards were last read and when the next scheduled read is due, never
  padded with weaker matches. There is no quota: one strong fit is a feed
  of one.
- **One role per company.** Today is a few distinct decisions: each
  company's best-ranked candidate, in rank order, until the feed is full.
  The company's other strong fits go with it (`also_at_company`, "+3 more
  roles at Supabase") instead of taking slots, and are not recorded as
  shown, so they stay new; a company's weaker roles never ride along. Fewer companies means a shorter feed, never a
  padded one.
- Preparing the feed ranks every open opportunity for the person: records,
  verification state and eligibility come in a fixed number of batched
  queries whatever the corpus size, and what is read from each posting is
  remembered per posting version in the process. Each feed logs where its
  time went (`today prepared`: person, load, eligibility, ranking, review,
  verify, classify, select) and what the semantic review did (reviews from
  cache, calls, failures).
- A **material change** is one the job's history records (UPDATED /
  REOPENED) in a field that changes whether someone would want or could
  take the job: pay published or changed, location or remote policy, work
  authorization, employment type, a closed job reopening. Title,
  description or link edits never bring a job back. A resurfaced job
  stays 24 hours and is not brought back twice for the same change.
- **"Not now"** (`POST …/dismiss`) records the opportunity as looked at
  (which carries no weight in learned taste) and puts it aside; **"Not for
  me"** is a rejection, with an optional reason in the person's words,
  which teaches. The two are kept apart on purpose.
- The summary counts are the ranking's own (open jobs checked, left after
  eligibility, worth reviewing, strong fits, new, on the feed now); nothing
  is estimated.
- The feed verifies its best candidates whose listings aren't trusted yet
  (at most 12), like a search; it never reads job boards (the workers do).
- What it showed is recorded per account (`user_opportunities`:
  first/last shown, resurfaced, put aside). Locally (SQLite) there is no
  such state, so "new" means "not acted on yet".

## Email notifications

`narrow worker notify` (every 15 minutes) sends each subscriber a **daily
digest**: once a day at most, a few jobs worth reviewing, nothing when
nothing qualifies (notification policy v2, BRU-389 slice A; the full
policy is in [`email-digest-policy-v2.md`](email-digest-policy-v2.md)).

- **Who**: accounts with email notifications on and a confirmed address.
  An address is confirmed by following a link sent to it (48 hours); until
  then nothing else is sent to it. The address is stored sealed and
  deleted with the account.
- **What** (`App::digest`): from the product's ranking, unchanged, the
  strong fits recommended outright (verified recently at an authoritative
  source, eligible or conditionally eligible), never acted on, never shown
  in the app or put aside, never in an earlier email; then, in rank order,
  one job per company, preferring companies not emailed about in the last
  7 days when another can fill the slot, at most the person's digest size
  (3 by default; at most `JOBHUNT_NOTIFY_MAX_ITEMS`, 5). Never padded:
  one candidate makes a one-job digest; none, no email. Each job shows
  where, pay when published, an honest freshness line ("Posted 342 days
  ago", "New on Acme's job board · first seen 3h ago", "Posted date
  unknown"), up to two reasons from its brief, and a link.
- **When**: at the person's digest time (09:00) on their days (Monday to
  Friday), in their timezone (UTC until set), never inside quiet hours
  (21:00–08:00) or while paused. At most one digest per local calendar day
  (`notification_settings.last_digest_on` is claimed whether anything is
  found or not); a slot missed by more than an hour is not sent later, and
  a digest the provider hasn't accepted by quiet hours, the end of the day
  or 6 hours is abandoned (`expires_at`). The old `cadence` column
  (`immediate`/`daily`) is kept but no longer read.
- **Today**: emailing a job doesn't consume it; it stays new on Today.
- **Idempotency**: an outbox. The rendered message and its opportunities
  are committed first (`notification_deliveries`,
  `notification_items`, whose primary key `(account, opportunity)` makes a
  second email about the same opportunity impossible), then sent with the
  delivery id as the provider's `Idempotency-Key`, then marked `sent`
  (only by the worker holding its lease), which moves the account's
  `notification_cursor` to it in the same transaction.
- **Failures**: a retryable failure (network, 429, 5xx) leaves it pending,
  retried with backoff (5 min, doubling, capped at 3 h) up to 6 attempts;
  a permanent refusal marks it `failed`. Never `sent` unless the provider
  accepted it. A crash after the provider accepted it is retried with the
  same stored message and key, which Resend answers with the original
  result instead of sending again (it keeps keys 24 hours); a delivery
  still pending after 20 hours is `abandoned` rather than sent late.
- **Coordination**: accounts and deliveries are claimed with leases
  (`FOR UPDATE SKIP LOCKED`), as for discovery and verification; runs are
  recorded in `worker_runs` (`kind = 'notification'`).
- **Provider**: behind `jobhunt_cloud::email::EmailSender`. Production
  uses [Resend](https://resend.com) (`JOBHUNT_EMAIL_PROVIDER=resend`): a
  plain HTTPS API, idempotency keys, and sender-domain verification. The
  file provider (`file`, refused in production) appends JSON lines for
  development and the end-to-end tests; tests use an in-memory sender. No
  required test sends real email.

## Sync

`narrow sync` merges this machine's state with the account's.

What syncs: the **profile** (every record: basics, resume documents,
experiences, projects, education, skills, claims with their confirm/reject
decisions, preferences, statements) and **feedback** (save, reject,
applied, interview, offer, like, dislike, unsave, with reasons). The
pipeline and learned taste are folded from feedback, so they follow.
"Seen" marks stay local. Jobs are not copied wholesale: a pull brings the
source records your feedback and cloud recommendations refer to, with
their latest verification, so they work offline; pushing feedback about a
job the cloud has never seen brings that job along (stored as
`origin = 'sync'` and verified first by the verification worker before it
can be recommended to anyone).

### Protocol

- The cloud stores each profile record as an entity with a **version**
  (incremented by every change, deletions are tombstones) and every
  private change takes the next value of the account's **change
  sequence** (under the account row's lock, so it is gap-free in commit
  order).
- **Pull** (`POST /api/v1/sync/pull {protocol, cursor}`) returns, from one
  consistent snapshot, every entity and feedback event changed after the
  cursor, the jobs and verifications they need, and the new cursor.
- **Push** (`POST /api/v1/sync/push`) sends entity mutations, each "based
  on version N", plus feedback and jobs. It is **compare-and-set, all or
  nothing**: if any entity changed since its base, nothing is applied and
  the current versions come back; the resulting profile must be
  consistent (every reference resolves) or the push is refused. A push
  that already landed (a retry after a lost answer) is recognized by its
  content and changes nothing: sync is idempotent.
- The client keeps, per entity, the last cloud version it agreed with (the
  *base*), the cursor, and which feedback the cloud has, in its own SQLite
  tables. Nothing else about the local product changes.

### Conflict semantics

Each record is decided on its own, three ways (base, here, cloud) — never
"last writer wins" for the whole profile:

| Here | Cloud | Result |
| --- | --- | --- |
| unchanged | changed | the cloud's version is applied here |
| changed | unchanged | this version is pushed |
| changed | changed identically | nothing to do |
| changed | changed, different fields | merged field by field and pushed (timestamps take the later one; edited-field lists and parser notes are united) |
| changed | changed, **same field differently** | **conflict** |
| edited | deleted (or the reverse) | **conflict** |

A conflict overwrites nothing: this machine keeps its version and does not
push it, the cloud keeps its own, and `narrow sync` lists it with both
sides ("the claim was decided differently here and in the cloud (confirmed
vs rejected)"). It stays until you choose:

```bash
narrow sync --keep local                  # every conflict: this machine's version wins
narrow sync --keep cloud --record clm_…   # one record: the cloud's version wins
narrow sync --status                      # where sync stands (offline)
```

Confirmed or rejected claims, explicit preferences and statements, and
feedback therefore never disappear silently. Feedback cannot conflict:
events are immutable with stable ids and merge by union. If merging would
leave the profile inconsistent (a claim about an experience deleted
elsewhere), the sync stops with a message and changes nothing.

### Offline first

Only `login`, `logout`, `account`, `sync` and `token` use the network to
reach the cloud. `find --offline`, `show`, `profile`, `claims`,
`preferences`, feedback and everything else work on the local database
whether the cloud is reachable or not. When it is not, `sync` fails with
`cloud_unavailable` ("could not reach JobHunt Cloud at …") and changes
nothing; changes made meanwhile sync next time (tested). The state file
(`narrow export`) remains the portable backup, locally and via
`GET /api/v1/export`.

## Scheduled discovery

`narrow worker discovery` (a Railway cron, every 30 minutes by default)
reads the corpus configured in `deploy/cloud.toml` (`JOBHUNT_CONFIG`):

1. registers the configured sources in `source_schedule` (new ones are due
   now; removed ones are disabled, their jobs and history kept);
2. recomputes tiers: a source is **active** when someone gave feedback on,
   or was recommended, one of its jobs in the last 14 days;
3. claims due sources (leases, see below) in batches and runs the
   **same discovery pipeline** as `narrow find`: conditional requests
   (ETags) so unchanged boards answer "not modified", the lifecycle
   (NEW / UPDATED / CLOSED / REOPENED, with history), completeness rules
   before closing, cross-source grouping;
4. records each source's outcome and next due time.

Cadence (configurable): active sources every 3 hours
(`JOBHUNT_DISCOVERY_ACTIVE_HOURS`), others every 12 hours
(`JOBHUNT_DISCOVERY_NORMAL_HOURS`); a failing source is retried after
1 hour, then 2, 4, 8, … hours, capped at 48 hours
(`JOBHUNT_DISCOVERY_MAX_BACKOFF_HOURS`), and returns to its tier's
interval after its next success. A run stops claiming after 20
minutes (`--budget-minutes`). Each source is read once for everyone: no
per-user crawling, no LLM, no browser.

## Scheduled verification

`narrow worker verification` (hourly by default) re-verifies up to
`JOBHUNT_VERIFY_BATCH` (100) open jobs whose last attempt is older than
the freshness window (`[verification] fresh_hours`, 24 h), most important
first:

1. in someone's pipeline (saved, applied, interviewing, offer);
2. recommended to someone in the last 7 days;
3. brought by a person's sync and never seen by cloud discovery;
4. discovered in the last 48 hours.

Other jobs are not verified in the background (they are verified when a
search makes them a candidate). It uses the product's verification
service and freshness policy, so a changed listing is UPDATED and a
reappearing one REOPENED exactly as locally.

Rankings are not precomputed: a search ranks for the person on request,
reusing stored eligibility decisions and rankings whose inputs (profile
revision, taste digest, record content, verification, rules versions) are
unchanged; any shared job change is a new cache key, so nothing stale is
served. Per-person search state (`user_opportunities`: first and last
shown, tier, times shown; `user_state`: last sync, last shortlist, the
profile revision it used, a notification cursor) is what "new since you
last looked" and notifications (BRU-295) build on.

## Coordination and locking

Postgres is the only coordination mechanism; nothing relies on
process-local state across instances.

| Concern | Mechanism |
| --- | --- |
| Migrations from several instances | sqlx's migration advisory lock (tested with concurrent `migrate`) |
| Two discovery workers | `UPDATE … FROM (… FOR UPDATE SKIP LOCKED)` leases per source; a dead worker's lease expires (15 min) and is taken over; outcomes are only recorded by the lease holder |
| Two verification workers | `verification_leases` claimed with one upsert that only takes free or expired leases |
| A scan and a verification of the same source | a transaction-scoped advisory lock per source (SQLite's `BEGIN IMMEDIATE` serialized all writers; Postgres serializes only what conflicts) |
| Opportunity regrouping | a transaction-scoped advisory lock |
| A person's read-modify-write use cases (feedback, preferences, imports) | a per-account advisory lock (the local store uses a process mutex) |
| Profile writers | the same optimistic revision as locally, checked under the profile row's lock (`Conflict` on a lost race) |
| Private change order | the account row's lock and change sequence |
| Worker runs | `worker_runs` rows (start, end, outcome, counts); runs left `running` by a dead worker are closed as abandoned |

## Observability and usage events

- **Logs**: JSON lines on stderr (`JOBHUNT_LOG` for the filter). Every
  request has a span with `request_id`, method, the route *template*,
  the internal account id once authenticated, status and duration.
  Bodies, query strings, `Authorization` and cookies are never logged;
  neither are resumes, contacts, reasons, preference text or tokens.
  Workers log counts per run and per source.
- **Usage events** (`usage_events`, separate from product data): `login`,
  `logout`, `sync_pull`, `sync_push`, `find`, `verify`, `feedback`,
  `preferences`, `token_created`, `mcp_tool`, and for the web
  `feed_opened`, `opportunity_viewed`, `dismiss`, `claims_decided`,
  `resume_imported`, `notifications_updated`, `email_confirmed` — with the account id and
  small safe metadata (counts, the tool name, the client, the action,
  whether a reason was given — never the reason, a preference, a file
  name or anything from the resume). They are written in batches off the request path
  (dropped rather than slowing a request when the writer is behind), can
  be turned off (`JOBHUNT_USAGE_EVENTS=false`), and are deleted with the
  account. `narrow admin status` summarizes the last 7 days.

## Deploying on Railway

Railway's `railway.json` / `railway.toml` config-as-code is deprecated
(new services cannot use it); the deployment is described as
Infrastructure as Code in [`.railway/railway.ts`](../.railway/railway.ts):
a Postgres database, the `api` service (`narrow server`, pre-deploy
`narrow migrate`, healthcheck `/ready`, 30 s draining on SIGTERM), the
`web` service (the Next.js app from
[`apps/web/Dockerfile`](../apps/web/Dockerfile), healthcheck `/healthz`,
calling `api` at `http://${{api.RAILWAY_PRIVATE_DOMAIN}}:8080`), and three
cron services for the workers (discovery, verification, notify). All services build the repository's
[`Dockerfile`](../Dockerfile) (a multi-stage build; Railpack's Rust
provider assumes the binary is named like its package, which is not the
case here).

First deployment:

1. Install the tools: Node 22.6+ and the Railway CLI (5.42+), then
   `npm install` at the repository root (it only installs the `railway`
   IaC package).
2. `railway login`, `railway init` (or `railway link` to an existing
   project and environment).
3. Review and apply: `railway config plan`, then `railway config apply`.
4. Set the hand-managed variables on the `api` service (then seal the
   secret ones in the dashboard):

   ```bash
   railway variable set -s api \
     JOBHUNT_ENCRYPTION_KEYS="k1:$(openssl rand -base64 32)" \
     JOBHUNT_OIDC_ISSUER=https://<subdomain>.authkit.app \
     JOBHUNT_OIDC_AUDIENCE="https://<api domain>,https://<api domain>/mcp,<environment client id>" \
     JOBHUNT_OIDC_CLI_CLIENT_ID=<the CLI application's client id> \
     JOBHUNT_PUBLIC_URL=https://<api domain>
   ```

5. Give `api` a public domain (`railway domain -s api`, or a custom
   domain); generated domains are not managed by the IaC file.
6. In the identity provider. For WorkOS AuthKit:
   - enable at least one sign-in method (Magic Auth, Google, GitHub, …);
   - under the OAuth resources (resource indicators), add
     `https://<api domain>` (the CLI's, as default) and
     `https://<api domain>/mcp` (MCP clients');
   - optionally enable dynamic client registration, so MCP clients can
     sign in with OAuth instead of a personal access token.

   - create a **public OAuth application** for the CLI (Applications →
     OAuth, public client, no secret) and use its client id as
     `JOBHUNT_OIDC_CLI_CLIENT_ID`; AuthKit's device endpoint refuses the
     environment's own client id (`invalid_client`);
   - add the **environment's client id** to `JOBHUNT_OIDC_AUDIENCE`:
     AuthKit's device flow ignores the `resource` parameter and issues
     tokens whose `aud` is the environment's client id (the CLI's own
     client id is in the `client_id` claim). Keep one WorkOS environment
     for JobHunt only, since every token of that environment then
     reaches the API.

   For Auth0 instead: an API
   whose identifier is the audience, a native application with the device
   code grant and refresh token rotation, and
   `JOBHUNT_OIDC_AUDIENCE_PARAMETER=audience`.
7. The web app (BRU-295):
   - give `web` a public domain (`railway domain -s web`);
   - in WorkOS, create an **OAuth application for the web app**
     (confidential, with a secret, or public with PKCE), with the redirect
     URI `https://<web domain>/auth/callback` and the sign-out redirect
     `https://<web domain>/signin`; add its client id to
     `JOBHUNT_OIDC_AUDIENCE` on `api` when AuthKit issues it as the
     audience (as for the CLI);
   - set on `web`: `JOBHUNT_WEB_URL=https://<web domain>`,
     `JOBHUNT_API_PUBLIC_URL=https://<api domain>`,
     `JOBHUNT_WEB_SESSION_SECRET` (`openssl rand -base64 48`, sealed),
     `JOBHUNT_WEB_OIDC_CLIENT_ID` and, for a confidential client,
     `JOBHUNT_WEB_OIDC_CLIENT_SECRET` (sealed);
   - set on `api`: `JOBHUNT_WEB_URL=https://<web domain>` (links in the
     confirmation email). `JOBHUNT_ALLOWED_ORIGINS` is not needed for the
     web app (it calls the API from its server).
8. Email (BRU-295): in Resend, verify a sending domain and create an API
   key restricted to sending; set on both `api` and `worker-notify`:
   `JOBHUNT_EMAIL_PROVIDER=resend`, `JOBHUNT_RESEND_API_KEY` (sealed),
   `JOBHUNT_EMAIL_FROM="JobHunt <notifications@<your domain>>"`,
   `JOBHUNT_WEB_URL`; and on `worker-notify` the same
   `JOBHUNT_ENCRYPTION_KEYS` as `api` (it reads addresses and rankings).
9. Deploy (pushes to `main`, or `railway up`), then
   [validate](#validating-a-deployment).

Applying the IaC file deletes variables it does not list; every
hand-managed variable is listed with `preserve()` so it survives.

Cost: one small always-on `api` service, two cron services that run for
minutes per hour and exit, and one Postgres. Pool sizes are small
(`api` 10 connections, workers 4) to stay far below Postgres's default
100 connections.

## Configuration reference

Cloud processes read environment variables only (plus `JOBHUNT_CONFIG`).
`narrow admin status` shows each as set, missing or invalid — never a
secret's value. Invalid or missing required values stop the process at
startup with every problem listed.

| Variable | Used by | Required | Default / notes |
| --- | --- | --- | --- |
| `DATABASE_URL` | all | yes | `${{Postgres.DATABASE_URL}}` (private network). Secret. |
| `JOBHUNT_ENCRYPTION_KEYS` | server, `admin reencrypt` | yes | `<id>:<base64 32 bytes>[,…]`; first encrypts. Secret. |
| `JOBHUNT_OIDC_ISSUER` | server | yes (OIDC) | the provider's issuer URL, exactly as in its tokens |
| `JOBHUNT_OIDC_AUDIENCE` | server | yes (OIDC) | the accepted `aud` values, comma separated; the first is the one the CLI asks for |
| `JOBHUNT_OIDC_AUDIENCE_PARAMETER` | server | no | how the CLI asks for it: `resource` (RFC 8707, default), `audience` (Auth0) or `none` |
| `JOBHUNT_OIDC_CLI_CLIENT_ID` | server | for CLI login | the public client of the device flow |
| `JOBHUNT_OIDC_SCOPES` | server | no | `openid offline_access` |
| `JOBHUNT_OIDC_JWKS_URL` | server | no | the provider's keys, for providers without metadata (skips discovery) |
| `JOBHUNT_OIDC_DEVICE_AUTHORIZATION_URL`, `_TOKEN_URL`, `_REVOCATION_URL` | server | no | endpoints the provider's metadata lacks (each overrides it) |
| `JOBHUNT_AUTH_MODE` | server | no | `oidc`; `dev` for development only (refused in production) |
| `JOBHUNT_AUTH_DEV_SECRET` | server | with `dev` | 32+ characters |
| `JOBHUNT_PUBLIC_URL` | server | in production | the https URL clients use; falls back to `https://$RAILWAY_PUBLIC_DOMAIN` |
| `JOBHUNT_ALLOWED_ORIGINS` | server | no | comma-separated browser origins (BRU-295's web app) |
| `JOBHUNT_ENV` | all | no | `RAILWAY_ENVIRONMENT_NAME`, else `development` |
| `PORT` / `JOBHUNT_BIND` | server | no | `[::]:$PORT` (8080), dual stack for the private network |
| `JOBHUNT_MIGRATE_ON_START` | server | no | `true` (migrations are also the pre-deploy command) |
| `JOBHUNT_DB_MAX_CONNECTIONS` | all | no | 10 |
| `JOBHUNT_DB_ACQUIRE_TIMEOUT_SECS` | all | no | 10 |
| `JOBHUNT_DB_STATEMENT_TIMEOUT_SECS` | all | no | 60 |
| `JOBHUNT_CONFIG` | all | no | the corpus and product settings (`/app/cloud.toml` in the image) |
| `JOBHUNT_LOG`, `JOBHUNT_LOG_FORMAT` | all | no | from the config file (`info`, `json`) |
| `JOBHUNT_DISCOVERY_ACTIVE_HOURS` / `_NORMAL_HOURS` / `_MAX_BACKOFF_HOURS` | discovery | no | 3 / 12 / 48 |
| `JOBHUNT_DISCOVERY_BATCH` | discovery | no | 25 sources per claim |
| `JOBHUNT_DISCOVERY_CONCURRENCY` | discovery | no | from the config file |
| `JOBHUNT_VERIFY_BATCH` | verification | no | 100 jobs per run |
| `JOBHUNT_VERIFICATION_FRESH_HOURS` / `_STALE_HOURS` | all | no | from the config file (24 / 72) |
| `JOBHUNT_USAGE_EVENTS` | server | no | `true` |
| `JOBHUNT_AI_PROVIDER` | server | no | `anthropic` or `openai` (any OpenAI-compatible server): a model that reads people's descriptions of what they want into their taste profile; unset or `none` uses the built-in reader. Also `JOBHUNT_AI_MODEL` (Anthropic default `claude-opus-5-5`), `JOBHUNT_AI_BASE_URL`, `JOBHUNT_AI_API_KEY` (secret), `JOBHUNT_AI_TIMEOUT_SECS`. An unusable setting falls back to the built-in reader and never stops the server. What is sent: [taste-profile.md](taste-profile.md#exactly-what-is-sent) |
| `JOBHUNT_AI_REVIEW_FIT` | server, worker | no | `true` to review the fit of each ranking's shortlist with the `JOBHUNT_AI_*` model (optionally `JOBHUNT_AI_REVIEW_MODEL`); off by default. A dozen new reviews per ranking at most, each (person, posting version, model) once, stored sealed; failures leave the rules' assessment. What is sent and the cost: [fit-and-practicality.md](fit-and-practicality.md#semantic-review) |
| `JOBHUNT_GITHUB_TOKEN` | server | no | a GitHub token without scopes: GitHub evidence imports at the authenticated rate limit, with language statistics (secret; without it, imports share GitHub's 60 requests/hour and use each repository's primary language) |
| `JOBHUNT_WEB_URL` | server, notify | for email | the web app's https URL (links in emails) |
| `JOBHUNT_EMAIL_PROVIDER` | server, notify | for email | `resend`, `file` (development; refused in production) or `none` (default) |
| `JOBHUNT_RESEND_API_KEY` | server, notify | with `resend` | Secret |
| `JOBHUNT_EMAIL_FROM` | server, notify | with `resend` | `JobHunt <notifications@example.com>`, on a domain verified in Resend |
| `JOBHUNT_RESEND_API_URL` | server, notify | no | `https://api.resend.com` (tests point it at a mock) |
| `JOBHUNT_EMAIL_FILE` | server, notify | with `file` | where the file provider appends messages |
| `JOBHUNT_NOTIFY_MAX_ITEMS` | server, notify | no | 5 (1–5): the largest digest a person can choose |
| `JOBHUNT_NOTIFY_BATCH` | notify | no | 50 accounts per claim |

The web app (`web` service) reads its own variables:

| Variable | Required | Default / notes |
| --- | --- | --- |
| `JOBHUNT_API_URL` | yes | where the web server reaches the API (`http://${{api.RAILWAY_PRIVATE_DOMAIN}}:8080`) |
| `JOBHUNT_API_PUBLIC_URL` | no | the API's public URL, shown in the MCP connection instructions; defaults to `JOBHUNT_API_URL` |
| `JOBHUNT_WEB_URL` | yes | this app's https URL (OAuth redirect URIs); `http://localhost:3000` by default |
| `JOBHUNT_WEB_SESSION_SECRET` | yes | 32+ characters; encrypts the session cookie. Secret |
| `JOBHUNT_WEB_AUTH_MODE` | no | `oidc` (default) or `dev` (development sign-in against a dev-mode API) |
| `JOBHUNT_WEB_OIDC_CLIENT_ID` | with `oidc` | the web app's OAuth client at the identity provider |
| `JOBHUNT_WEB_OIDC_CLIENT_SECRET` | no | for a confidential client (else PKCE only). Secret |
| `JOBHUNT_WEB_OIDC_SCOPES` | no | `openid profile email offline_access` |

On the laptop: `[cloud] server = "https://…"` in the config file (or
`JOBHUNT_CLOUD_URL`), `JOBHUNT_CREDENTIALS_FILE` to choose where the
session is stored, `JOBHUNT_TOKEN` for `login --token`.

## Validating a deployment

After the first deploy (the commands a person with access to the project
runs; nothing here was run against Railway from this repository):

```bash
API=https://<api domain>
curl -fsS $API/health                          # {"status":"ok",…}
curl -fsS $API/ready                           # {"status":"ready","database":"ok","schema":"current"}
curl -fsS $API/.well-known/oauth-protected-resource/mcp
railway logs -s api --since 10m                # "JobHunt Cloud listening", migrations applied

# An account, and sync
narrow login --server $API                    # device sign-in in the browser
narrow account
narrow init resume.pdf && narrow sync        # the profile goes up
narrow sync --status

# Workers, on demand
railway run -s worker-discovery narrow worker discovery      # or trigger the cron in the dashboard
railway run -s worker-verification narrow worker verification
railway ssh -s api -- narrow admin status     # schedule, schema, usage

# Stored jobs and an authenticated request
TOKEN=$(narrow token create validation --days 1 | tail -1)
curl -fsS -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' \
  -d '{"limit":3}' $API/api/v1/search
# MCP: point an MCP client at $API/mcp with the token (or OAuth)
narrow token revoke <tok_…>

# The web app (BRU-295)
WEB=https://<web domain>
curl -fsS $WEB/healthz                         # {"status":"ok"}
# In a browser: sign in, upload a resume, add a preference, open Today,
# save / reject (with a reason) / mark applied, check Applications,
# Preferences (stated vs learned) and Profile (claims to review).
curl -fsS -H "Authorization: Bearer $TOKEN" $API/api/v1/feed | jq '.summary, .caught_up'

# Notifications to a controlled test recipient
#   Settings → Email notifications → your address → follow the link → on
railway run -s worker-notify narrow worker notify   # or trigger the cron
curl -fsS -H "Authorization: Bearer $TOKEN" $API/api/v1/notifications | jq .recent
```

(`railway run` runs locally with the service's variables; the private
`DATABASE_URL` does not resolve outside Railway, so use
`DATABASE_PUBLIC_URL` locally or `railway ssh` into the service.)

## Known limitations

- Sync is explicit (`narrow sync`); mutations are not pushed
  automatically, and there are no server-pushed updates to the CLI.
- A pull brings the jobs your feedback and recent cloud recommendations
  refer to; the rest of the corpus is searched in the cloud, not copied.
  A cloud-closed job already stored on the laptop is updated by the
  laptop's own discovery or verification, not by sync.
- A conflict that would leave the profile inconsistent stops the whole
  profile part of a sync (feedback still syncs) until resolved with
  `--keep`.
- Ranking runs per request over every open opportunity in the shared
  corpus (batched reads); with a much larger corpus it will need
  precomputation for active users.
- The OS keychain is used on macOS and Windows; on Linux the session is an
  owner-only file (Secret Service support would need D-Bus).
- MCP clients that cannot do OAuth need a personal access token.
- There is no per-account rate limiting yet (request bodies are capped at
  16 MB and requests time out after 120 s); a search's verification is
  bounded (at most 12 candidates) and verifications are shared.
- "Log out everywhere" takes effect within 30 seconds on other replicas
  (identities are cached that long); personal access tokens record their
  last use on every request.
- Usage events are basic counts for operating the beta, not an analytics
  product; there is no dashboard.
- The feed's "shown" state lives in the cloud only; the local product
  (SQLite) treats "new" as "not acted on yet".
- Notifications are email only, with an on/off switch and two cadences;
  the unsubscribe link goes to Settings (signed in), not a one-click
  token. A delivery that stays pending for 20 hours is abandoned (its
  opportunities are not emailed later; they are on Today).
- An address is confirmed by a link, not by the identity provider's
  verified email claim.
- Not built here (BRU-296): billing and plans, application assistance
  (answers, tailored resumes, letters).
