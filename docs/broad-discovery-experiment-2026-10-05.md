# Broad discovery experiment, 2026-10-05 (BRU-360)

The scale experiment for [`broad-discovery.md`](broad-discovery.md): can
Narrow find companies and boards it has never heard of, resolve them to
first-party boards, and say which discovery strategies are worth running?

No ranking rule, Fit semantics, Today threshold, eligibility rule,
application workflow or monetization changed. `deploy/cloud.toml` is
unchanged (21 active sources). Nothing was activated or deployed. The
registry gains 134 of the 144 recommended boards as `validated` (the other
10 were already listed); existing entries are untouched.

**Result in one paragraph.**
- **Discovered without names:** five repeatable strategies produced 64,998
  unique job URLs, 7,968 unique boards and about 9,800 companies. None of
  their names was supplied by a person.
- **First-party resolution:** 2,261 boards validated as the company's own.
  Ownership was established for 71% of the boards whose ownership was
  checked. On the 60 validated boards Narrow already knew, it found the
  registry's company every time; the two mismatches are registry entries
  that are out of date.
- **Usefulness:** 417 validated boards have engineering postings open to,
  or unclear for, a Brazil-based remote candidate; 397 are new to Narrow.
  144 meet the activation bar.
- **Dogfood profile:**
  - **Recommended set added:** strong fits go from 4 to 14, plausible from
    45 to 491, and Today from 1 company to 5 (still capped at 5).
  - **All 411 useful boards added:** strong fits go from 4 to 37.
- **Noise:**
  - 62% of the job URLs the indexes still list are closed. Read through
    the first-party board, they are caught as closed rather than shown as
    open.
  - One query family in three found no useful board.

---

## 1. Setup

| | |
|---|---|
| Date | 2026-10-05 (UTC) |
| Code | branch `t3code/broad-job-discovery` (this PR) |
| Production before | 21 active sources (`deploy/cloud.toml`) |
| Store | one `discovery.json`, 75,795 candidates; private, in `/tmp/bru360` |
| HTTP | the shared client; 3 requests at a time per host (production's setting); company pages 15 s timeout, 1 retry; ownership check capped at 60 s per board |
| Reference profile | lives in São Paulo, works remotely only, no relocation (BRU-343 §7) |
| Dogfood profile | the private profile copied from production for Batch 2A (`/tmp/bru344/base.db`, jobs deleted); nothing from it is committed |

**Discovery inputs.** No company was named by a person:

| Strategy | Provider | Input |
|---|---|---|
| `ats_search` | a web-search tool with the ATS host as a domain filter | the 65 pairwise queries + 44 specialty queries + the "applied ai" example: 110 queries, `docs/broad-discovery/queries-2026-10-05.txt`; 1,085 results, `search-results-2026-10-05.json` |
| `web_index` | Common Crawl CDX, crawl CC-MAIN-2026-39 (Sept 2026) | every indexed URL of `jobs.ashbyhq.com`, `job-boards.greenhouse.io`, `boards.greenhouse.io`, `job-boards.eu.greenhouse.io`, `jobs.lever.co`, `jobs.eu.lever.co` (two Greenhouse pages were cut short by the index server; their valid lines were kept) |
| `hiring_thread` | HN Algolia API | "Ask HN: Who is hiring?" for August, September and October 2026 |
| `company_directory` | yc-oss | 1,476 hiring, active YC companies |
| `company_directory` | remoteintech | 872 remote-friendly companies |
| `sitemap`, `jsonld` | the company's own site | fallback for companies whose careers page shows no board |

**Not completed:**
- **Common Crawl boards:** 2,988 of the 7,390 boards it found were never
  read. The run was stopped to keep the session bounded. Every other
  strategy's boards were read in full. Common Crawl's numbers below are
  over the 4,402 boards it did read.
- **Ownership of Common Crawl boards:** after the first ~2,000, it was
  checked only for useful boards (`--ownership useful`), as a cost
  control. The first-party rate counts checked boards only.
- **Unsupported-ATS volume:** a Common Crawl count of unsupported ATS hosts
  could not be measured; the index stopped answering.

## 2. Totals

| | Count |
|---|---:|
| Raw hits (sightings of their own) | 94,929 |
| Unique URLs | 94,656 |
| **Unique candidate job URLs** | **64,998** |
| Unique boards | 7,968 (Ashby 3,025 · Greenhouse 4,615 · Lever 298 · YC 30) |
| Boards read through their adapter | 4,980: 4,183 listing, 544 gone (404), 253 empty |
| **Validated boards** | **2,261** (2,254 distinct company domains) |
| Inconclusive (ownership not established, or not checked) | 1,873 |
| Rejected (gone, empty, stale-only, unstable ids) | 846 |
| Companies (by domain, or by board when no domain was found) | 9,826 |
| **Companies new to Narrow** (validated boards whose domain and board the registry doesn't list) | **2,199 of 2,254** |
| Company candidates resolved through BRU-343's careers-page path | 5,039: 2,482 with a board, 223 unsupported ATS, 1,184 custom page, 956 no careers page, 111 blocked, 83 unreachable |
| Unsupported-ATS companies | 400+ (§7) |
| Aggregator pages ignored (never a source) | 595 |
| **Duplicate rate** (1 − unique candidates ÷ raw hits) | 23% overall; 11% search, 38% HN, 23% crawl |
| **Stale rate** (discovered job URLs no longer on their board ÷ checked) | 62% (19,521 closed: 17,565 not on the current listing, 1,240 board gone, 716 board empty) |
| Live discovered jobs | 11,835: 646 posted in the last 30 days, 912 over a year old |

**Ownership of the 3,181 readable boards whose ownership was checked:**

| Ownership | Boards | Share |
|---|---:|---:|
| verified (the company's site points at the board) | 2,051 | 64% |
| corroborated (an independent source names the domain; postings tie to it) | 210 | 7% |
| claimed (only the board names its website) | 798 | 25% |
| elsewhere (the company's site points at another board) | 53 | 2% |
| unknown (no domain found) | 69 | 2% |

**First-party resolution rate: 71%** (verified + corroborated).

**No false ownership validation.** On the 60 validated boards the registry
already lists, discovery found the registry's company domain 58 times. The
other two are registry entries that are out of date:
- **`ashby:neon`:** the board itself names neonpay.com as its website, and
  neonpay.com's site links it. The registry's neon.com now redirects to
  Databricks (BRU-343 §12).
- **`lever:contentsquare`:** discovery attributes it to contentsquare.com.
  BRU-343 recorded it as Hotjar's parent's board.

Spot-check: 40 validated boards drawn at random all belong to the domain
attributed to them.

One false validation was caught and fixed during the run. An HN post linked
an `archive.ph` copy of a careers page, and its matching-slug board
validated for `archive.ph`. Archives and link shorteners are no longer
company domains. A second bug was caught by the known-answer check:
- **The bug:** boards linked from another company's careers page (a
  portfolio page, a job-board site such as getcargo.io) inherited that
  page's domain and were filed as inconclusive. That kept langchain,
  elevenlabs, warp and others from their own ownership check.
- **Not a false validation:** none of those boards was validated.
- **The fix:** such boards are now resolved on their own (`add_linked_board`).
  The 294 affected boards were reset and resolved again.

## 3. Per-strategy yield

`useful`: validated with an engineering posting open to (or unclear for)
the reference profile. `ready`: also meets the activation bar. `only`:
boards no other strategy found. Person counts are the dogfood profile's
ranking on the validated useful boards (§6).

| Strategy | Raw hits | Unique job URLs | Live · closed (stale) | Boards | Read | Validated | 1st-party | Useful | Ready | Only this strategy | Companies (new) | Dup | Person act · plaus · strong |
|---|---:|---:|---|---:|---:|---:|---:|---:|---:|---:|---|---:|---|
| ats_search | 1,085 | 800 | 223 · 430 (66%) | 469 | 433 | 246 | 58% | 81 | 50 | 116 | 508 (482) | 11% | 1,728 · 438 · 9 |
| hiring_thread:hn | 880 | 127 | 80 · 21 (21%) | 257 | 249 | 189 | 77% | 50 | 18 | 65 | 492 (468) | 38% | 569 · 92 · 12 |
| company_directory:yc-oss | 1,476 | — | — | 818 | 771 | 628 | 82% | 92 | 34 | 288 | 1,722 (1,698) | 0% | 921 · 173 · 13 |
| company_directory:remoteintech | 872 | 2 | — | 242 | 223 | 179 | 80% | 53 | 29 | 81 | 884 (844) | 0% | 1,139 · 178 · 5 |
| web_index:commoncrawl | 90,616 | 64,157 | 11,580 · 19,088 (62%) | 7,390 | 3,667 | 1,901 | 71% | 364 | 125 | 6,367 | 7,514 (7,427) | 23% | 3,795 · 658 · 34 |
| jsonld (careers surface) | — | — | — | 6 | 6 | 6 | 100% | 1 | 0 | 0 | 6 | — | 188 · 36 · 0 |
| sitemap (careers surface) | — | — | — | 3 | 2 | 0 | 0% | 0 | 0 | 0 | 3 | — | — |

Reading it:
- **Common Crawl is the coverage engine.** It gives 6,367 boards no other
  strategy found and 364 useful boards. It is free, but it is a
  snapshot: 62% of its job URLs had closed by today.
- **ATS search is the precision instrument per request.**
  - 110 queries gave 81 useful and 50 ready boards: one useful board per
    1.4 queries.
  - 116 of its boards appear nowhere else (fresh, keyword-steered
    postings the crawl lacks).
  - Its stale rate is the worst (66%): search indexes keep closed pages.
- **HN "Who is hiring?" is the freshest.**
  - Stale rate 21%, and 77% of its boards are first-party verified.
  - It has the most strong fits per board (12 strong from 50 useful
    boards).
  - Small: about 290 posts a month.
- **Directories (yc-oss, remoteintech)** give the cleanest ownership
  (80–82% first-party) and a steady stream of boards. They are not
  steered to engineering or geography, so their yield per board is lower.
  remoteintech's `region` field makes it the better directory for
  Brazil/LATAM.
- **Sitemap and JSON-LD** rescued 6 validated boards from companies whose
  careers page showed none (1 useful). That is cheap to run, but its yield
  is small.

## 4. Query families

97 families (role or specialty × place group) from 110 queries. Full
per-query and per-family rows are in `report-2026-10-05.json`.

**Top families** (useful validated boards, from ~10–20 results each):

| Family | Results | Boards | Validated | Useful | Ready | Stale | New companies |
|---|---:|---:|---:|---:|---:|---:|---:|
| backend engineer · brazil | 20 | 15 | 8 | 6 | 3 | 62% | 15 |
| infrastructure engineer · global | 20 | 18 | 10 | 6 | 1 | 42% | 17 |
| cloud engineer · brazil | 20 | 17 | 12 | 5 | 4 | 53% | 17 |
| developer infrastructure · brazil | 10 | 8 | 7 | 5 | 4 | 57% | 9 |
| developer tools · brazil | 10 | 9 | 8 | 5 | 4 | 50% | 8 |
| backend engineer · americas | 10 | 9 | 6 | 4 | 3 | 43% | 8 |
| infrastructure · americas | 10 | 7 | 5 | 4 | 3 | 0% | 6 |
| distributed systems engineer · latam | 20 | 18 | 11 | 4 | 1 | 44% | 19 |
| applied AI · latam | 10 | 9 | 4 | 3 | 3 | 43% | 8 |
| site reliability engineer · americas | 10 | 10 | 3 | 3 | 3 | 71% | 8 |

**Noisy families:** no useful board, and most results closed.

| Family | Results | Boards | Validated | Useful | Stale |
|---|---:|---:|---:|---:|---:|
| platform engineer · global | 10 | 9 | 0 | 0 | 100% |
| software engineer · brazil | 10 | 6 | 3 | 0 | 100% |
| security engineer · brazil / americas | 20 | 8 | 2 | 0 | 100% |
| systems engineer · global | 10 | 7 | 1 | 0 | 100% |
| backend software engineer · global | 10 | 10 | 4 | 0 | 89% |
| developer tools · global | 10 | 10 | 3 | 0 | 100% |
| observability · brazil, fintech · latam, distributed systems · latam, data infrastructure · americas | 40 | 6 | 1 | 0 | 100% |

32 of 97 families found no useful board.

**By place searched:**

| Place group | Results | Boards | Validated | Useful | Ready | Stale | 1st-party |
|---|---:|---:|---:|---:|---:|---:|---:|
| brazil | 268 | 120 | 65 | 30 | 20 | 73% | 59% |
| latam | 258 | 133 | 66 | 28 | 21 | 64% | 55% |
| americas | 270 | 165 | 96 | 29 | 17 | 62% | 64% |
| global (remote, global, worldwide) | 279 | 150 | 68 | 22 | 9 | 60% | 50% |

"Brazil" and "LATAM" find ready boards at twice the rate of
"remote/global/worldwide". The generic words mostly surface US-remote
roles.

**By ATS host searched:**

| Host | Results | Boards | Validated | Useful | Ready | Stale |
|---|---:|---:|---:|---:|---:|---:|
| jobs.ashbyhq.com | 227 | 150 | 116 | 34 | 19 | 42% |
| job-boards.greenhouse.io | 220 | 135 | 57 | 27 | 17 | 56% |
| boards.greenhouse.io | 210 | 94 | 37 | 14 | 9 | **97%** |
| jobs.lever.co | 220 | 75 | 34 | 12 | 10 | 76% |
| jobs.eu.lever.co | 208 | 42 | 16 | 2 | 0 | 57% |

`boards.greenhouse.io` is the legacy host. Its indexed job pages are 97%
dead (Greenhouse moved boards to `job-boards.`), but its board slugs are
still good leads. Ashby's index is the freshest and its boards the most
often validated.

## 5. Brazil / LATAM / global yield

| | Count |
|---|---:|
| Validated boards with an engineering posting open to (or unclear for) a Brazil-based remote candidate | **417** (397 new to Narrow) |
| …with a posting whose remote scope names Brazil | 60 |
| …scoped to the Americas / Latin America | 45 |
| …scoped worldwide | 36 |
| Engineering postings on validated boards | 19,568 |
| …open to (or unclear for) the reference profile | 1,722 |
| Meeting the activation bar (BRU-343's rule, unchanged) | 144 |

## 6. Dogfood-profile impact

**Method.**
- **Scan:** one scan of the superset (A + C′: 432 sources, 17,566 open
  jobs; 140 s, 0 source failures) into a copy of the dogfood profile's
  database.
- **Cells:** each cell's database is that snapshot with every other
  source's jobs deleted, so cells differ only by sources.
- **Ranking:** `real_posting_probe` at `2026-10-05T20:00:00Z`, offline,
  rules only, no reviewer, no verification. Today is computed exactly as
  the feed selects it.

Cells:
- **A:** production's 21 sources.
- **B:** every readable discovered board. Not run: about 4,000 boards and
  over 100,000 jobs, beyond this session's time.
- **C′:** A + the 411 validated boards that are useful for the reference
  profile. This is the first-party-resolved corpus, restricted to boards
  that can matter to a Brazil-remote candidate.
- **D:** A + the 140 recommended boards not already active (144 ready,
  minus 4 already in production or non-adapter).

| | A: production (21) | C′: + useful validated (432) | D: + recommended (161) |
|---|---:|---:|---:|
| Open jobs | 2,937 | 17,566 | 10,585 |
| Excluded: ineligible · unmet requirement | 575 · 2,139 | 5,424 · 7,829 | 4,205 · 4,272 |
| **Actionable** | **223** | **4,313** (+4,090) | **2,108** (+1,885) |
| **Plausible** | **45** | **848** (+803) | **491** (+446) |
| **Strong** | **4** | **37** (+33) | **14** (+10) |
| **Today** (companies · jobs) | **1 · 4** | **5 · 10** | **5 · 13** |

**Today in D:**
- **Ando Technologies:** *Senior Backend Engineer (Contract to Hire)*.
- **Oneleet:** *Backend Engineer*, plus 2 more.
- **Nango:** *Staff Engineer, Platform & Infrastructure*, plus 3 more.
- **Veda:** *Backend Engineer – Solana*.
- **Railway:** *Senior Infra Engineer: Baremetal Orchestration*, plus 3
  more.

Today in C′ has the same first four and Scoreplay's *Senior Platform
Engineer (SRE)*, with Railway pushed off by the 5-company limit.

**Where the strong fits are:**
- **In D (14):** Nango 4, Railway 4, Oneleet 3, Ando 1, Veda 1, g2i 1.
- **In C′ (37):** the above plus Runlayer 3, Atticus 2, Fin 2, AllSpice 2,
  Wisdom 2, Fathom 2, Scoreplay, Pigment, Oddball, Escape, Centralize,
  ApartmentIQ, Neuroscale, Bayesian Health, Spur and Tread. Those boards
  fail the activation bar today (no posting open to Brazil, or no Brazil
  scope), so they are held, not recommended. The person still ranks some
  of their roles strong; deciding whether to activate them is a decision
  about the bar itself, not this task's.

Ranking, Fit and Today's thresholds are untouched: the difference between
cells is only sources. Today stays capped at 5 companies.

## 7. Supported and unsupported ATS

| Supported | Boards | Read | Validated | Useful | Ready | Postings read |
|---|---:|---:|---:|---:|---:|---:|
| Ashby | 3,025 | 2,604 | 1,851 | 292 | 96 | 54,327 |
| Greenhouse | 4,615 | 1,345 | 273 | 98 | 36 | 62,274 |
| Lever | 298 | 204 | 110 | 26 | 12 | 14,436 |
| YC | 30 | 30 | 27 | 1 | 0 | 116 |

Greenhouse's read share is low: most unread boards are Greenhouse, and
many crawled Greenhouse slugs are gone.

**Unsupported ATS map.** Companies counted once per provider, from direct
URLs (search, HN, crawl) and from careers pages that name the ATS:

| Provider | Companies | URLs | Brazil/LATAM/global context | Also on a supported board | Examples |
|---|---:|---:|---:|---:|---|
| Workable | 55 | 78 | 27 | 3 | balena.io, auror.co, atria.org, bandlab.com |
| Workday | 39 | 43 | 24 | 5 | cloudera.com, 8vc.com, bloc.io |
| Dover | 34 | 39 | 0 | 4 | activeloop.ai, castle.io, autostep.ai |
| Gem | 30 | 40 | 3 | 3 | biorender.com, allia.health |
| Teamtailor | 26 | 28 | 12 | 0 | cast.ai, arsenal.com, balena.io |
| Wellfound | 24 | 25 | 5 | 10 | baremetrics.com, axlehealth.com |
| BambooHR | 23 | 24 | 7 | 2 | dribbble.com, clutch.co |
| Rippling | 22 | 27 | 4 | 3 | argyle.com, boomsupersonic.com |
| Recruitee | 20 | 22 | 5 | 4 | astronomer.io, channable.com |
| JazzHR | 18 | 27 | 8 | 2 | bitovi.com, chargify.com |
| Breezy | 13 | 13 | 4 | 1 | boldare.com, embraer.com |
| Personio | 12 | 14 | 1 | 2 | cratedb.com, capmo.com |
| Pinpoint | 12 | 17 | 7 | 1 | cartodb.com, freeagent.com |
| SmartRecruiters | 12 | 12 | 8 | 2 | enpal.com, edify.cr |
| iCIMS | 11 | 11 | 6 | 1 | booking.com, ebsco.com |
| Jobvite | 5 | 5 | 3 | 0 | crowdstrike.com, leidos.com |

**Recommended next adapter: Workable** (`apply.workable.com/<account>`).
- **Opportunity:** it leads on companies (55) and on Brazil/LATAM/global
  context (27). Few of those companies also have a supported board (3).
- **Effort:** it has a public, unauthenticated per-account jobs endpoint
  (`apply.workable.com/api/v1/widget/accounts/<account>` answers JSON with
  the account's `jobs`). It would follow the Ashby/Greenhouse/Lever adapter
  pattern.
- **Runner-up: Teamtailor** (26 companies, 12 Brazil/LATAM/global). Its
  public per-account feed still needs checking: the obvious
  `<account>.teamtailor.com/jobs.json` answered 404.
- **Not first:**
  - Workday is second by count but enterprise-heavy, per-tenant and harder
    to read.
  - Dover's companies are US startups with no Brazil context.
- This is a recommendation for a follow-up, not built here.

The counts include one aggregator careers page (getcargo.io) that names
several ATS, which adds at most 1 to each provider.

## 8. Source activation candidates

From `narrow discovery report` (full list in `report-2026-10-05.json`,
`activate[]`):

| Bucket | Boards | Rule |
|---|---:|---|
| **Recommended activate** | **144** | validated; meets the activation bar; an engineering posting open to (or unclear for) Brazil |
| Hold | 2,117 | validated, below the bar (mostly no Brazil scope) |
| Recheck | 227 | useful postings, ownership not established (`claimed`, `elsewhere`, timed out) |
| Reject | 846 | gone, empty, stale-only, unstable ids |

**First batch suggested to a person**: verified ownership, strong or
plausible fits for the dogfood profile, or many engineering postings open
to Brazil.

| Board | Company domain | Ownership | Eng · open BR · unclear | Dogfood act · plaus · strong | Found by |
|---|---|---|---|---|---|
| ashby:nango | nango.dev | verified | 5 · 5 · 0 | 5 · 0 · 4 | search, YC, HN, crawl |
| ashby:oneleet | oneleet.com | verified | 7 · 7 · 0 | 12 · 4 · 3 | YC, HN, crawl |
| ashby:g2i | g2i.co | verified | 23 · 1 · 9 | 11 · 3 · 1 | search, crawl |
| ashby:andotechnologies | ando.work | verified | 2 · 2 · 0 | 2 · 0 · 1 | crawl |
| ashby:veda | veda.tech | verified | 2 · 1 · 0 | 1 · 0 · 1 | crawl |
| greenhouse:sezzle | sezzle.com | verified | 128 · 104 · 0 | 144 · 75 · 0 | search, crawl |
| greenhouse:coderoad | coderoad.com | verified | 19 · 18 · 1 | 22 · 11 · 0 | search, crawl |
| ashby:camunda | camunda.com | verified | 13 · 8 · 5 | 38 · 9 · 0 | search, remoteintech, crawl |
| greenhouse:customerio | customer.io | verified | 12 · 12 · 0 | 24 · 8 · 0 | search, directories, crawl |
| greenhouse:alpaca | alpaca.markets | verified | 30 · 13 · 0 | 25 · 7 · 0 | search, YC, crawl |
| ashby:revenuecat | revenuecat.com | verified | 8 · 8 · 0 | 19 · 4 · 0 | directories, HN, crawl |
| ashby:truelogic | truelogicsoftware.com | verified | 94 · 67 · 24 | — | search, remoteintech, crawl |

**Volume boards to judge separately:**
- **lever:bluelightconsulting:** 1,319 engineering postings, 156 open to
  Brazil. A nearshore staffing firm; its 120 plausible roles would flood
  a Today that is ranked by company.
- **greenhouse:coinbase:** 183 actionable for the dogfood profile, 0 open
  to Brazil.

The BRU-343 / Batch 2A candidates come back on their own:
- greenhouse:elastic (corroborated, 60 engineering open to Brazil);
- ashby:supabase and ashby:railway (already active, rediscovered by three
  strategies each);
- ashby:clickhouse (validated).

## 9. Timing and first-party findings

**Coverage:**
- 6,917 boards were found by exactly one strategy.
- 2,199 of 2,254 validated company domains are new to Narrow's registry.
- Absence from Narrow's own registry is the comparison here. No external
  aggregator was re-measured. BRU-343's Himalayas result (23 of 24 fresh
  first-party postings absent) still stands.

**Freshness: not measured, by design.**
- Every discovered job was first seen today. `first_seen_at` therefore says
  when discovery ran, not when Narrow could have known.
- Monitoring must start first. The cohort is ready:
  - **Boards:** D's 161 sources (`D.toml`, built by
    `narrow discovery export --select ready --format config`), scanned at
    2026-10-05T19:47Z.
  - **Tool:** `crates/jobhunt-cli/examples/first_party_timing.rs`.
- Procedure:
  1. Rescan the cohort on a schedule.
  2. At +0, +1, +3 and +7 days, run
     `first_party_timing <config> <db> 2026-10-05T19:47:00Z >> timing.jsonl`.
  3. It lists jobs first seen after the start, with a publish date after
     it too, and checks Himalayas' public search for each: same company,
     compatible title.

**First-party durability:**
- 19,521 discovered job URLs were already closed on their board (62%).
- 1,240 point at boards that no longer exist.
- 53 companies' sites point at a different board than the indexed one
  (ATS migrations). Discovery followed them to the current board.
- This is the Amplitude case from BRU-343, at scale: search engines and
  crawls keep dead job pages, and the first-party board is what says a job
  is gone.

## 10. Continuous discovery: cadence and cost

Design in `broad-discovery.md` §14–16.

**Expected cadence:**
- ATS search: ~15 rotated queries a day.
- HN: monthly, with daily rereads in the first week.
- yc-oss and remoteintech: weekly diffs.
- Common Crawl: each new crawl (~monthly), new boards only.
- Resolution: hourly over pending candidates.

**Operational cost.** This experiment issued roughly 70–90k HTTP
requests:
- about 5,000 board reads;
- about 3,200 ownership checks of up to 6 pages each;
- about 5,000 company probes of up to 8 pages each;
- sitemap and JSON-LD looks;
- a few hundred Common Crawl and API calls.

Done once, that took about 1.5 hours of the run's wall time at production's
per-host limits. The rest of the run was the throughput problems in §11.

Steady state handles only what is new:
- tens to low hundreds of new boards a day;
- one adapter read each;
- one ownership check of a few pages each.

| Item | Estimate |
|---|---|
| CPU and network | negligible on `worker-discovery` |
| Postgres | two tables, low-MB per month (the crawl import is the bulk: ~100 MB of candidates per crawl if kept whole; keep boards, drop settled job URLs after 30 days) |
| Search API | 450 queries a month (15 a day) fits Brave's free tier (2,000 a month); other providers are similar or file-based |
| Common Crawl, HN, yc-oss, GitHub | free |
| People | the activation review queue: ~5–20 boards a week at the bar used here |

## 11. What took time, and limits

**Throughput, found and fixed during the run:**
- `discover_companies` (BRU-343) ran companies with an ordered stream. One
  slow site held back new starts, so effective concurrency collapsed. It
  is now unordered, and results keep input order.
- The sitemap and JSON-LD fallback ran one company at a time. It is now
  concurrent.
- Ownership checks have a 60 s deadline.

**Common Crawl's index server** answered 502/504 for long stretches.
Requests went one at a time, with pauses.

**Limits of the numbers:**
- 2,988 crawl boards were not read.
- B was not run.
- The person counts come from the C′ ranking.
- "Companies" counts boards without a domain by board.
- Search results came from one search tool (10 per query). Other providers
  would give different, overlapping sets.

## 12. Recommended next work

1. **Activate a first batch** from §8 after a person's look: Nango,
   Oneleet, g2i, Ando, Veda (strong fits), then Sezzle, Coderoad, Camunda,
   Customer.io, Alpaca, RevenueCat. Measure it like Batch 2A.
2. **Fix stale registry entries:**
   - `ashby:neon` belongs to Neon Pay;
   - `lever:contentsquare` to Contentsquare.
3. **Run discovery continuously** (design §14) with the query feedback
   loop:
   - **Keep:** the Brazil, LATAM and Americas families for backend,
     cloud, infrastructure, developer infrastructure and developer tools.
   - **Rotate:** the global families.
   - **Disable:** the noisy ones in §4.
   - Search `job-boards.greenhouse.io`, not `boards.greenhouse.io`, for
     job pages.
4. **Write the Workable adapter**, then Teamtailor.
5. **Start the timing cohort** and measure +0/+1/+3/+7.
6. **Recheck the 227 useful-but-unconfirmed boards** with a person's look
   or a second ownership pass.

## Reproduce

```bash
cargo build --release -p jobhunt-cli --bin narrow --example real_posting_probe
N="target/release/narrow --config disc.toml --database disc.db"
$N discovery queries --specialties > queries.txt           # run through any search provider, save results JSON
$N discovery import search-results.json
$N discovery run hn --months 3
$N discovery run yc
$N discovery run remoteintech
$N discovery import cc/*.jsonl --method web_index --provider commoncrawl:CC-MAIN-2026-39
$N discovery resolve --registry deploy/sources.toml --chunk 1000            # companies, then boards
$N discovery resolve --registry deploy/sources.toml --ownership useful      # remaining crawl boards
$N discovery report --registry deploy/sources.toml --json > report.json
$N discovery export --select ready --format config > D-extra.toml
```

The private store, databases, cells and scripts are in `/tmp/bru360/` on
the machine that ran this. No profile data is committed.
