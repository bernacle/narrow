# Source batch: BRU-360 first batch, 2026-10-11 (BRU-389 slice 0)

The inventory precondition of
[`proactive-agent-experience.md`](proactive-agent-experience.md): activate
the first batch of boards that broad discovery
([`broad-discovery-experiment-2026-10-05.md`](broad-discovery-experiment-2026-10-05.md) §8)
recommended, and measure what changes for the dogfood profile.

No ranking rule, Fit semantics, Today rule or eligibility rule changed.
The change is configuration only: 10 entries added to `deploy/cloud.toml`
(21 → 31 sources), and the same 10 registry entries in
`deploy/sources.toml` moved from `validated` to `active`. Other registry
entries are untouched. No migration is needed.

**Nango is not activated**, although it was in the BRU-360 batch. The
dogfood user is already known to be uninterested in Nango, and company
hiding isn't on `main` yet. This choice is about this dogfood rollout, not
a judgment on the board; it stays `validated`.

## Method

- **Profile:** the dogfood profile, synced from production into a private
  database (no `/feed` or `/search` calls).
- **Scan:** one fresh scan of A + the 10 candidates (31 sources) on
  2026-10-11 at 22:05Z: 0 failures, 3,434 open jobs.
- **Cells:** each cell is that snapshot with every other candidate's jobs
  deleted, so cells differ only by sources.
- **Ranking:** `real_posting_probe` at `2026-10-11T22:10:00Z`, offline,
  rules only, no reviewer, no verification. Today is computed exactly as
  the feed selects it.
- **Validation:** `narrow sources discover` re-run on the 10 company
  domains, and a second scan of all 31 boards.

## Validation (re-checked 2026-10-11)

All 10 boards still pass:
- **Resolution:** the company's homepage or careers page links the board.
- **Ownership:** verified.
- **Adapter:** the scan reads every posting, all with provider ids. The
  rescan matched every posting (0 new, 0 closed).
- **Activation bar:** met.
- **Staleness:** none is stale-only.

Oneleet shrank since 2026-10-05: open postings went from 12 to 8,
engineering from 7 to 3, and strong fits from 3 to 1. Its BRU-360 Today
job (*Backend Engineer*) has closed. The board is healthy and still meets
the bar, so it is activated, but its yield is small.

## Per-source marginal yield (A + one board)

Columns:
- **Eng BR / unclear:** engineering postings open to Brazil, or whose
  scope is unclear for it.
- **Fresh:** posted in the last 30 days.
- **Act / Plaus / Strong:** change against A.

| Board | Open | Eng | Eng BR | Unclear | Fresh | >90d | Act | Plaus | Strong | New Today entry |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| `ashby:oneleet` | 8 | 3 | 3 | 0 | 1 | 62% | +8 | +2 | +1 | Oneleet: *Fullstack Engineer, AI Implementation* |
| `ashby:g2i` | 23 | 22 | 2 | 8 | 20 | 9% | +10 | +4 | +0 | — |
| `ashby:andotechnologies` | 2 | 2 | 2 | 0 | 0 | 0% | +2 | +0 | +1 | Ando Technologies: *Senior Backend Engineer (Contract to Hire)* |
| `ashby:veda` | 3 | 2 | 1 | 0 | 0 | 0% | +1 | +0 | +1 | Veda: *Backend Engineer - Solana* |
| `greenhouse:sezzle` | 232 | 121 | 97 | 0 | 56 | 46% | +143 | +75 | +0 | — |
| `greenhouse:coderoad` | 26 | 22 | 20 | 1 | 10 | 38% | +24 | +11 | +0 | — |
| `ashby:camunda` | 43 | 13 | 8 | 5 | 25 | 12% | +42 | +8 | +0 | — |
| `greenhouse:customerio` | 30 | 12 | 12 | 0 | 10 | 33% | +24 | +6 | +0 | — |
| `greenhouse:alpaca` | 78 | 33 | 15 | 0 | 30 | 18% | +28 | +7 | +0 | — |
| `ashby:revenuecat` | 16 | 7 | 7 | 0 | 4 | 0% | +16 | +4 | +0 | — |

**What each board adds:**
- **Ando, Veda and Oneleet:** each adds one strong fit and a new Today
  company.
  - Ando: a LATAM-remote senior backend role.
  - Veda: a worldwide backend role.
  - Oneleet: the role is US/EU remote and was posted 342 days ago (shown
    with "it may be filled").
- **Camunda:** 8 plausible remote backend, data-layer, Kubernetes and SRE
  roles, 4 of them posted in the last 30 days.
  - Most of its other actionable jobs are low priority.
  - The top reason is customer-facing engineering (10 jobs).
- **RevenueCat:** 4 plausible Americas-scoped senior backend, SDK, agents
  and product roles.
- **Alpaca:** 7 plausible remote engineering roles for the Americas, EU or
  worldwide, among them core trading, crypto and ledger. Two of them are
  over a year old.
- **Customer.io:** 6 plausible backend roles for the Americas.
  - 4 of them are over 90 days old.
  - Its low-priority jobs are mostly sales, finance and product roles.
- **CodeRoad:** 11 plausible LATAM roles: Java, .NET, React, QA and
  DevOps. It is a nearshore firm, and 5 of these 11 roles are over
  90 days old.
- **g2i:** 4 plausible roles, among them a fresh LATAM *Software
  Engineer* and a *DevOps Engineer*. Most of its other jobs are
  "maybe" (unclear scope).
- **Sezzle:** the largest Brazil inventory, with 97 engineering postings
  open to Brazil, and real backend, payments, identity and infrastructure
  roles. Read its counts with care:
  - Each role is posted once per country. 232 postings are 102 distinct
    titles.
  - Its 75 plausible jobs are 11 distinct roles.
  - It adds no strong fit and no Today entry. Today shows one entry per
    company, so the duplicates can't crowd Today.

## Combined cell

A is production's 21 sources; B is A plus the 10 boards.

| | A: production (21) | B: + batch (31) | Δ |
|---|---:|---:|---:|
| Sources scanned · failed | 21 · 0 | 31 · 0 | +10 · 0 |
| Open jobs | 2,973 | 3,434 | +461 |
| Engineering jobs | 1,089 | 1,326 | +237 |
| Engineering open to Brazil · unclear | 33 · 28 | 200 · 42 | +167 · +14 |
| Boards with engineering open to (or unclear for) Brazil | 8 | 18 | +10 |
| **Actionable** | **137** | **435** | **+298** |
| **Plausible** | **32** | **149** | **+117** (11 distinct Sezzle roles account for 75) |
| **Strong** | **4** | **7** | **+3** |
| **Today** (companies · jobs) | **1 · 4** | **4 · 7** | **+3 · +3** |
| Scan (wall clock) | 6.8 s | 6.2 s | within noise; slowest new board 1.7 s (Sezzle) |

**Today in B:**
- **Ando Technologies:** *Senior Backend Engineer (Contract to Hire)*.
  New.
- **Oneleet:** *Fullstack Engineer, AI Implementation*. New.
- **Veda:** *Backend Engineer - Solana*. New.
- **Railway:** *Senior Infra Engineer: Baremetal Orchestration*, plus
  *Observability*, *Storage* and *Infrastructure Engineer*. Unchanged.

## Decision

All 10 boards are activated. Each adds a new company with
engineering inventory that a Brazil-based remote candidate can take:
- three new Today companies;
- the rest add plausible backend, platform and infrastructure roles;
- none fails a scan or adds a duplicate company.

Sezzle and CodeRoad are the noisiest. Sezzle adds per-country variants,
and CodeRoad's roles are nearshore and often older than 90 days. Neither
reaches Today. The change is easy to reverse: delete a board's entry from
`deploy/cloud.toml` and set its registry status back to `validated`.

## Post-merge checks

- **Deploys:** `api`, `worker-discovery` and `worker-verification`
  deployments reach SUCCESS for the merge commit, checked on the new
  deployment, not `/ready`.
- **Workers:** the discovery worker claims the 10 new sources, and their
  first scans succeed with no new failures.
- **Funnel:** re-measure it through logs and the mirror, and compare
  with B above.
