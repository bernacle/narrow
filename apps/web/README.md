# Narrow web

The web app of **Narrow** ([narrow.fyi](https://narrow.fyi)), served by JobHunt
Cloud: **Today**, **Applications**, **Preferences**,
**Profile** and Settings. It is deliberately small. A good visit is short:
open Today, see the two to five opportunities that are new and worth your
time, understand why each one is there and what to watch out for, decide,
and leave. When nothing new is worth your time, it says you're caught up.

It decides nothing about jobs. Ranking, eligibility, verification, taste,
preference parsing and the evidence policy are the Rust application's
(`crates/jobhunt-app` and the domain crates), reached through the HTTP API
(`/api/v1`). This app renders their answers and forwards the person's
actions.

## Architecture

```text
browser ──(HTML, server actions; one encrypted HttpOnly cookie)──▶ Next.js server
                                                                     │  Bearer <access token>
                                                                     ▼
                                                        JobHunt API (/api/v1) ── Postgres
```

- **Next.js 16 App Router, React 19, TypeScript, Tailwind 4.** Pages are
  server components; the few interactive pieces (card actions, the reject
  dialog, forms) are small client components. No client-side data store:
  the API is the state.
- **Server-only API access** ([`src/lib/api.ts`](src/lib/api.ts)): pages and
  server actions call the API with the session's access token. The browser
  never sees a token or the API's private address.
- **Types from the server.** [`src/lib/api-schema.json`](src/lib/api-schema.json)
  is generated from the Rust response and request types
  (`crates/jobhunt-cloud/tests/api_schema.rs`, which fails when it is
  stale), and [`src/lib/api-types.ts`](src/lib/api-types.ts) from it
  (`npm run types`; `npm run types:check` in CI). A view changed in Rust
  that the web doesn't handle fails the type-check.
- **Auth** ([`src/lib/oidc.ts`](src/lib/oidc.ts),
  [`src/lib/session.ts`](src/lib/session.ts), [`src/proxy.ts`](src/proxy.ts),
  [`src/app/auth`](src/app/auth)): the API's identity provider (read from
  `/api/v1/auth/config`), authorization code + PKCE with `openid-client`,
  the API's audience requested, tokens sealed into one AES-256-GCM JWE
  cookie (HttpOnly, `SameSite=Lax`, `Secure` over https). The proxy renews
  the access token with the refresh token before it expires; a 401 clears
  the session. Development sign-in (`JOBHUNT_WEB_AUTH_MODE=dev`) mints a
  token from an API in development auth mode.
- **Mutations** are server actions ([`src/app/actions.ts`](src/app/actions.ts))
  that return the API's view or a stable error code;
  [`src/lib/errors.ts`](src/lib/errors.ts) turns codes into words. Raw
  server messages are not shown.
- **Freshness without real-time machinery:** Today renders on each visit,
  refreshes when you come back to the tab after five minutes, every 15
  minutes while visible, and on "Check again". Nothing polls in a hidden
  tab.

## Design: Quiet Material v2

Narrow should feel precise, calm and selective: premium through restraint.
Hierarchy comes from type size, ink, spacing and hairline rules, not
containers. At most one raised surface per screen (Today's lead, the
decision brief); everything else sits on the ground. Tiers are words
("Strong fit", "Worth reviewing"), never percentages or scores.

- **Tokens** ([`src/app/globals.css`](src/app/globals.css)): every colour,
  type size, radius, shadow and easing, each role defined once as
  `light-dark(light, dark)` and mapped into Tailwind's theme. Components use
  only the semantic names (`bg-ground`, `bg-raised`, `text-fg-muted`,
  `border-line-subtle`, `text-title-l`, `text-mono-s`, …); the default
  palette is switched off. Dark is the design's first theme; the theme
  follows the system, and Settings → Appearance can pin dark or light (a
  `narrow_theme` cookie read by the server, so there is no flash).
- **Colour is information.** Primary actions are ink, not accent. Mint
  (`accent`, `verified`) marks first-party verification, focus and live
  status only; sand (`warning`) marks a real caution. Radii stay at 4, 6 and
  8px; borders are 1px.
- **Type**: Instrument Sans for everything a person reads
  (`@fontsource-variable/instrument-sans`, width axis for display), Geist
  Mono (`geist`) only for machine-measured facts: times, ages, counts, ids.
  Both are self-hosted from npm; the build downloads nothing.
- **Trust and uncertainty** ([`src/components/trust.tsx`](src/components/trust.tsx)):
  a verified fact gets the check; an inferred or unresolved one (conditional
  eligibility, "Remote" without a region) less ink and a dotted underline;
  an unknown is said plainly in muted ink; a caution gets a sand square,
  an unknown a hollow one. Meaning never rests on colour alone.
- **Components** are Narrow concepts: `Wordmark`/`BrandMark`, the shell
  (`nav.tsx`), `OpportunityLead`/`OpportunityPeer`, `FactRow`,
  `VerificationStatus`, `Consideration`, `EligibilityDetail`, and the
  primitives in `ui.tsx` (`Button`, `Raised`, `Notice`, `Skeleton`, …).
- **Default view = decision; detail = evidence.** `DecisionSummary` picks
  the strongest distinct reasons and the most material concern from the
  API's ordered lists ([`src/lib/decision.ts`](src/lib/decision.ts)); it
  never renders every line, never counts what it leaves out, and never
  drops an unknown (the rest is in the evidence). Detail is reached through
  a few patterns only: `SummaryRow` (setting or fact · value · one action;
  a row never changes shape), `Sheet` (one focused surface: a 440px panel
  at the side, a sheet from the bottom on phones) used by `EvidencePanel`
  for evidence and by each preference's editor (one decision at a time, a
  draft with Save and Cancel), `SummarySection` (conclusion first, then a
  labelled disclosure), `ClaimReviewRow`, and `StateMessage` (one state,
  one next step, details on demand). The opportunity page is a decision
  brief: the verdict, two reasons and the most material concern, then its
  key facts once each (`SummaryRow`s, their evidence behind each row's
  action), then the actions. Repeated items share tracks
  (`--nr-track-label`, `--nr-row-min`); Today's peers share rows with
  CSS subgrid.

Accessibility is part of the components: landmarks and a skip link, one
`h1` per page, labelled forms, `aria-live` status for every action's
outcome, a native modal `<dialog>` for "Not for me" (a bottom sheet on
phones), real tabs with arrow keys on Applications, a 1.5px mint focus ring
(an outline, so it survives forced colours), 44px targets on phones,
reduced motion respected, and AA contrast in both themes. Automated checks
(axe) run in the component tests and in the browser tests, in both themes;
they don't replace a manual review.

Internal names stay `jobhunt` (the crates, the API, environment
variables, the `jh_session` cookie, `jh_pat_` tokens): the
rename is of the product people see, not of protocols.

## Configuration

[`.env.example`](.env.example) lists these; copy it to `.env.local`
(ignored by git) for local work.

| Variable | |
| --- | --- |
| `JOBHUNT_API_URL` | where the server reaches the API (required) |
| `JOBHUNT_API_PUBLIC_URL` | the API's public URL, for the MCP connection instructions |
| `JOBHUNT_WEB_URL` | this app's URL (OAuth redirect: `<url>/auth/callback`) |
| `JOBHUNT_WEB_SESSION_SECRET` | 32+ characters (required) |
| `JOBHUNT_WEB_AUTH_MODE` | `oidc` (default) or `dev` |
| `JOBHUNT_WEB_OIDC_CLIENT_ID` / `_SECRET` | the web app's OAuth client (secret only for a confidential client) |
| `JOBHUNT_WEB_OIDC_SCOPES` | default `openid profile email offline_access` |

## Running it locally

Needs Node 24, a Postgres server (and `psql`), and the `narrow` binary
(`cargo build -p jobhunt-cli` at the repository root).

```bash
npm install
JOBHUNT_E2E_DATABASE_URL=postgres://user:pass@127.0.0.1:5432/postgres npm run stack
```

`npm run stack` ([`e2e/stack.mjs`](e2e/stack.mjs)) creates a fresh
`jobhunt_e2e` database, serves the fixture job boards
([`e2e/fixtures/boards.mjs`](e2e/fixtures/boards.mjs) and one recorded real
board) in Greenhouse's API format, runs `narrow migrate` and the real
discovery worker over them, starts `narrow server` (development auth,
email written to `.e2e/mail.jsonl`, verification against the fixtures),
and `next dev` on <http://127.0.0.1:3100>. Sign in with any name; import
`crates/jobhunt-resume/tests/fixtures/ana_lima.md` as the resume. Nothing
reaches a real job board or sends real email.

## Tests

```bash
npm run types:check   # the API types match the committed schema
npm run typecheck
npm run lint
npm test              # Vitest + Testing Library + axe (jsdom)
npm run build
npm run e2e           # Playwright against the real stack (needs `npm run build` and the binary)
```

- **Component tests** (`src/**/*.test.tsx`): the recommendation card (why,
  what to consider, verified vs listed vs unknown pay, eligibility
  uncertainty, verify-first notes, changed items), save / applied / not
  now / reject with a free-text reason and suggestions, rollback when the
  API refuses, the decision brief, the caught-up and "still gathering"
  states, the summary using only API counts, preference interpretation
  (understood, uncertain, not interpreted), learned vs stated taste,
  claim review, and axe checks on each.
- **End-to-end** (`e2e/*.spec.ts`, Chromium, desktop and a phone):
  sign in, upload a resume, state preferences, Today (a lead and peers, only
  strong fits and jobs worth reviewing, verified pay and listing,
  stable across reloads, no infinite list), the full brief, reject with a
  reason from the keyboard, save, applied, Applications (stage changes),
  Preferences (stated vs learned), Profile (claim review), the daily
  digest email (confirmation link, nothing when nothing is new, a digest
  for a strong job published later and found by the real discovery and
  verification workers, one digest a day, never twice), sign out and back in with everything
  kept, finishing Today to "caught up", signed-out redirects, a tampered
  cookie, no horizontal scrolling and 44px actions on a phone, Narrow (never
  JobHunt) on every page, both themes and the theme setting, and axe on
  every page.
