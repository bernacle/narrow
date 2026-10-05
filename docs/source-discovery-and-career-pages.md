# Source discovery and career pages (BRU-343)

Two goals, kept separate:

- **A. Production inventory now:** add a small, proven batch of boards to
  `deploy/cloud.toml`.
- **B. A reusable path from companies to their boards:** company domain →
  careers page → first-party ATS board → adapter check → candidate in a
  registry. Nothing it finds is activated automatically.

No ranking rule, Fit semantics, Today threshold, eligibility rule, scoring,
application workflow or monetization changed. Every number below is from
2026-10-05 (UTC) unless dated otherwise.

**Result in one paragraph.** Four boards (Railway, Oyster, Zapier,
Wikimedia) take the real dogfood profile from an empty Today and 0 strong
fits to 4 strong fits and a Today of 1 company and 4 jobs. Actionable jobs
go from 183 to 222 and plausible from 39 to 45, with 0 scan failures. Socket
was inspected and held back.

`narrow sources discover` turns a company domain into a validated board. On
59 companies whose boards are known, it found the right board for 54 and
validated 49, and it validated no wrong board. On 85 new candidate
companies it found boards for 43 and validated 38; three are ready to
activate (Elastic, Fivetran, ElevenLabs).

First-party advantage was only partly measurable. 23 of 24 fresh
first-party postings were absent from Himalayas at measurement time, but
Himalayas does not list 11 of those 17 companies at all. HiringCafe could
not be measured. No discovery-time advantage could be measured yet.

---

## 1. Current production source architecture

- **Config is the list.** `deploy/cloud.toml` is baked into the image
  (`COPY deploy/cloud.toml /app/cloud.toml`, `JOBHUNT_CONFIG=/app/cloud.toml`
  in the `Dockerfile`). Railway's `worker-discovery` sets no override (its
  only variables are `DATABASE_URL` and `JOBHUNT_DB_MAX_CONNECTIONS`). It
  runs `narrow worker discovery` on cron `7,37 * * * *`.
- **Adapters** (`crates/jobhunt-sources`): `ashby`, `greenhouse`, `lever`
  and `yc`. Each is generic over a board, site or slug; no company-specific
  code. `careers` resolves a careers page to the board it links to or
  embeds.
- **Scheduling and health in production.** The worker registers every
  configured source in `source_schedule` (Postgres): tier, `enabled`,
  `next_due_at`, `consecutive_failures` with exponential backoff, and
  `last_status`/`last_error`. Each read is a row in `source_scans` (status,
  received, normalized, rejected, new, updated, closed, error).
- **Identity.** `JobId` = source kind + instance + the provider's job id (or
  the canonical URL). Opportunities group records across sources only on
  ATS ids found in URLs or identical canonical URLs (`jobs/identity.rs`).
- **Freshness fields.** A posting carries `posted_at` (the source's own
  publish date) and `source_updated_at`. A record carries `first_seen_at`,
  `last_seen_at`, `content_updated_at` (last material change), `status` and
  `closed_at`. Verifications record `checked_at`.

## 2. Current production source list (before)

17 sources, 4 families:

| Family | Sources |
| --- | --- |
| Ashby (8) | linear, ramp, notion, supabase, posthog, modal, replit, vanta |
| Greenhouse (4) | anthropic, stripe, figma, airbnb |
| Lever (3) | spotify, palantir, zoox |
| YC (2) | posthog, doordash |

All 43 BRU-325 boards (Appendix A of
`competitive-recommendation-teardown.md`) still exist as reusable config in
`/tmp/bru325/B-off.toml`. All four ATS families already had generic
adapters and careers-page board detection.

## 3. Immediate proven batch

Inspected with the exact BRU-325 identifiers: `ashby:railway`,
`ashby:oyster`, `ashby:socket`, `ashby:zapier`, `greenhouse:wikimedia`.

- **Health:** all five endpoints answered 200, and every adapter scan
  succeeded with 0 rejected postings.
- **Stable identity:** every posting carries a provider id.

| Board | Open | Engineering | Open to BR (eng) | Unclear for BR (eng) | Geography (postings) | >1y old | Decision |
| --- | ---: | ---: | ---: | ---: | --- | ---: | --- |
| ashby:railway | 8 | 8 | 0 (0) | 8 (8) | 6 global, 2 US | 4 of 8 | **add** |
| ashby:oyster | 26 | 2 | 10 (1) | 8 (0) | 5 name Brazil, 7 Europe, 7 other, 4 no scope | 1 | **add** |
| ashby:zapier | 11 | 1 | 3 (1) | 0 | 7 North America, 1 Americas, 3 other | 0 | **add** |
| greenhouse:wikimedia | 11 | 5 | 10 (5) | 1 | "Remote" with no scope; the text opens it worldwide | 0 | **add** |
| ashby:socket | 26 | 5 | 0 (0) | 2 (2) | 17 US, 5 Europe, 2 other, 2 no scope | 1 | **hold (validated)** |

"Open to BR" means eligible or conditional for the **reference profile**:
lives in São Paulo, may work in Brazil, remote only, no relocation (§7).

**Railway.** Its postings are "Global" but mention time-zone overlap
without saying which, so eligibility stays *unclear* rather than open. Its
old postings are evergreen and kept on purpose. In BRU-325 it supplied 3 of
the 5 practical strong yeses.

**Socket fails the activation rule** (§7): no posting is open to Brazil or
has a remote scope that includes it. Its BRU-325 strong yes (Senior Platform
Engineer) is still listed but publishes no geographic scope, and 17 of 26
postings are US-only. It stays `validated` in the registry; adding it
would add 2 actionable jobs, 1 plausible and 0 strong (§5).

## 4. Before/after inventory (real dogfood profile)

**Method.**
- **Profile:** the private dogfood profile synced from production for
  BRU-328 (`/tmp/bru328/dogfood`). It was copied with its jobs deleted;
  nothing from it is committed.
- **Schema:** the paused BRU-328 branch's empty `profile_hide_rules` table
  and its migration row were dropped from the copy, so `main` opens it.
- **Scans:** one fresh full scan per config with `main`'s `narrow find
  --raw --refresh`.
  - A = the 17 production sources.
  - B = A + the 5 inspected boards.
  - C = A + the 4 added (the proposed config). C is B's snapshot without
    Socket's rows, so A/B/C differ only by sources.
- **Ranking:** `real_posting_probe` at `2026-10-05T01:10:00Z`, offline, no
  reviewer, `verify` off. Today is computed exactly as the feed selects it.

| | A: before (17) | C: proposed (21) | Δ | B: all 5 (22) |
| --- | ---: | ---: | ---: | ---: |
| Sources configured · succeeded · failed | 17 · 17 · 0 | 21 · 21 · 0 | +4 · +4 · 0 | 22 · 22 · 0 |
| Open jobs | 2,892 | 2,948 | +56 | 2,974 |
| Engineering jobs | 1,063 | 1,079 | +16 | 1,084 |
| Excluded: ineligible · unmet requirement | 560 · 2,149 | 576 · 2,150 | +16 · +1 | 600 · 2,150 |
| **Actionable** | **183** | **222** | **+39** | 224 |
| Plausible | 39 | 45 | +6 | 46 |
| **Strong** | **0** | **4** | **+4** | 4 |
| **Today** (companies · jobs) | **0 · 0** (caught up) | **1 · 4** | +1 · +4 | 1 · 4 |
| Reference profile: open to Brazil (engineering) | 49 (25) | 72 (32) | +23 (+7) | 72 (32) |
| Reference profile: unclear (engineering) | 44 (17) | 61 (25) | +17 (+8) | 63 (27) |

**Today in C:** Railway, *Senior Infra Engineer: Baremetal Orchestration*,
with *Senior Infra Engineer: Observability*, *Senior Platform Engineer:
Storage* and *Infrastructure Engineer*.

**Scan duration** (3 timed full scans each, back to back):
- A: 9.5, 10.4, 7.7 s.
- C: 13.1, 15.4, 22.3 s; a single earlier run took 8.5 s.
- The difference is not the new boards, which took 0.3–2.5 s each. It is
  `lever:palantir`, which took 6.3–19.6 s depending on the run. The added
  boards also queue ahead of the Lever sources (concurrency 6), so Lever
  starts about a second later.
- In production each source is claimed and read on its own schedule, so
  four small boards add four requests per cycle.

**How the dogfood profile differs from the reference profile.** It states
no current location, *wants* (does not require) remote, does not relocate,
and is authorized in Brazil. Its "actionable" therefore includes US-remote
and office jobs the reference profile excludes (75 Vercel and 34 EBANX jobs,
for example). The reference columns are the stricter Brazil-remote reading.

## 5. Per-source yield

Each new source's contribution to the person's ranking (from B, rules only):

| New source | Actionable | Plausible | Strong | On Today |
| --- | ---: | ---: | ---: | ---: |
| ashby:railway | 8 | 3 | 4 | 4 (all of Today) |
| greenhouse:wikimedia | 11 | 2 | 0 | 0 |
| ashby:oyster | 17 | 0 | 0 | 0 |
| ashby:zapier | 3 | 1 | 0 | 0 |
| *ashby:socket (held)* | 2 | 1 | 0 | 0 |

The production sources themselves, measured the same way with `narrow
sources report` (eBR = engineering open to Brazil, eBR? = engineering
unclear, BR = any posting open to Brazil):

| Source | Open | Eng | eBR | eBR? | BR | Main geo | Person: act · plaus · strong |
| --- | ---: | ---: | ---: | ---: | ---: | --- | --- |
| ashby:supabase | 48 | 20 | 19 | 0 | 38 | global | 44 · 13 · 0 |
| greenhouse:airbnb | 152 | 49 | 6 | 1 | 10 | north_america | 11 · 3 · 0 |
| greenhouse:anthropic | 640 | 274 | 0 | 12 | 0 | office | 18 · 4 · 0 |
| greenhouse:stripe | 716 | 182 | 0 | 0 | 1 | office | 95 · 14 · 0 |
| ashby:posthog | 8 | 5 | 0 | 2 | 0 | no scope | 3 · 2 · 0 |
| yc:doordash | 9 | 9 | 0 | 2 | 0 | office | 5 · 3 · 0 |
| ashby:notion | 136 | 40 | 0 | 0 | 0 | office | 1 · 0 · 0 |
| greenhouse:figma | 162 | 35 | 0 | 0 | 0 | no scope | 6 · 0 · 0 |
| ashby:linear, ramp, modal, replit, vanta; lever:spotify, palantir, zoox; yc:posthog | 1,045 | 444 | 0 | 0 | 0 | office / north_america | 0 · 0 · 0 |

**13 of the 17 production boards fail today's activation bar.** Only
supabase, airbnb and the two YC/PostHog duplicates come close. Nothing was
removed in this task; the registry records each board's measurement.

## 6. Source registry

`deploy/sources.toml`: 104 entries, one per board Narrow knows about.

- **Fields:**
  - `source`: the canonical id `<provider>:<board>`, which gives the
    provider and the board;
  - `company`, `domain`, `careers_url`;
  - `status`, `since`;
  - `provenance`: where the entry came from;
  - `reasons`: the measurement behind the status;
  - `notes`.
- **Static file, derived facts.** Last scan, last success, last job seen,
  open-job count, last error and health change with every scan, so they are
  derived from the database by `narrow sources report --registry
  deploy/sources.toml` and never stored. The file changes only when a
  decision does.
- **No schema change.** Production already keeps per-scan rows
  (`source_scans`) and schedule health (`source_schedule`). The report adds
  one read path, `Store::recent_scans`, for SQLite and Postgres.
- **The config stays the switch.** The entries marked `active` are exactly
  `cloud.toml`'s sources; a test (`jobhunt-cloud`,
  `the_source_registry_matches_the_cloud_source_list`) fails otherwise.
  Rollback is one config edit.
- **Contents today:** 21 active, 75 validated, 5 candidates, 3 rejected.

## 7. Candidate → validated → active lifecycle

```text
candidate ──validate──▶ validated ──a person adds it to cloud.toml──▶ active
    │                                                                │  ▲
    └──▶ rejected                             repeated failures ──▶ unhealthy
                                               a person removes it ──▶ disabled
```

These are plain functions in `jobhunt_sources::registry`, unit-tested.

**`validate`: candidate → validated, rejected or still candidate.**
1. The adapter reads the board. A transient failure (timeout, 429, 5xx) is
   inconclusive; anything else rejects.
2. It lists at least one posting; otherwise rejected.
3. The board is the company's:
   - the company's own site links, embeds or redirects to a board whose slug
     matches the company; or
   - something ties the board to the company's domain: the website an Ashby
     board names (`publicWebsite`), or a posting whose URL is on the domain
     or whose text mentions it.

   The company's name in the postings is **not** enough. `greenhouse:ghost`
   reports the company "Ghost" but belongs to ghst.io, not ghost.org. Without
   a tie the board stays a **candidate**: nothing shows it is wrong either,
   and a person can confirm it.
4. At least 90% of postings have the provider's own id; otherwise rejected.
5. Not every dated posting is over a year old; otherwise rejected.

**`activation`: is a validated board worth reading in production?** It
returns the unmet conditions, and activation stays a person's config change.
- At least one engineering posting open to, or unclear for, the reference
  profile.
- At least one posting open to Brazil, or whose published remote scope
  includes Brazil (global, Brazil, the Americas). "Remote" with no place, or
  vague time-zone wording, is not enough on its own. Railway passes on its
  "Global" scope.
- Fewer than 75% of dated postings over a year old. An evergreen board is
  never dropped for age alone: this only gates activation.

**`next_status`:**
- candidate → validated or rejected by `validate`;
- active → unhealthy when health is unhealthy, and back to active when
  healthy;
- validated → active and anything → disabled are left to a person.

## 8. Company-domain discovery

`narrow sources discover <domain>… [--file companies.txt] [--no-guess]
[--registry FILE] [--json]` runs `jobhunt_sources::company::discover` for
each company. It reads public pages only, at most 8 per company, through
the shared HTTP client (per-host limits). Company pages get a 15 s timeout
and one retry, and run 6 companies at a time.

1. **Homepage.** Find board links and embeds, known ATS without an adapter,
   and links to a careers page. A link counts when it is on the same site
   and has a careers word in its path or text (`careers`, `jobs`,
   `join-us`, "we're hiring", …; a bare `/join` is not one:
   `huggingface.co/join/discord`).
2. **Careers links, then the usual places:** `/careers`, `/jobs`,
   `/company/careers`, `/about/careers`, `/join-us`, `careers.<domain>`,
   `jobs.<domain>`. It stops at the first board whose slug matches the
   company.
3. **Slug guess,** only when no page names a board: the domain's first
   label and the name, on Ashby, Greenhouse and Lever. The guess must pass
   rule 3 above.
4. **Adapter check:** every board found is read with its regular adapter.
   This records postings, provider ids, ownership evidence, freshness, and
   (in the app layer) job function, geography and reference eligibility.

**Input.** The input is a list of domains (`domain` or `domain, Name`,
`#` comments). No directory or aggregator is a runtime dependency.
`--registry` writes validated, rejected and candidate entries and never
overwrites an active, unhealthy or disabled one.

## 9. Careers-page detection

- **Which page counts:** the page a board or ATS was found through, else
  the first candidate page whose *final* URL looks like a careers page.
- **Redirects are judged by where they land.** A careers URL that redirects
  to the homepage is not a careers page (Turso). One that redirects to
  another company's careers page is recorded as that page:
  - `dagster.io/careers` → `prefect.io/careers`;
  - `stytch.com/careers` → `jobs.twilio.com/careers`;
  - `hotjar.com` → `contentsquare.com`.
- **Never followed:** a careers page's own job links. Those are the
  board's business.

## 10. ATS extraction

Supported boards are recognized in links, iframes, embed scripts and inline
JSON (escaped slashes included):
- `jobs.ashbyhq.com/<board>` and the Ashby posting API;
- `boards.greenhouse.io` and `job-boards.greenhouse.io` (EU hosts too),
  `…/embed/job_board?for=<board>`, and the Greenhouse boards API;
- `jobs.lever.co` / `jobs.eu.lever.co` and the Lever API;
- YC company pages.

Lookalike hosts (`myjobs.lever.co`, `notgreenhouse.io`), ATS root pages and
invalid slugs are not boards. When a page links several boards, all are
reported. A slug that doesn't resemble the company is marked, and must then
be tied to the company like a guess. Remote.com's `/jobs` page links
Jobgether, AIR, Veeam and JumpCloud boards; none validates.

**Tests:**
- `careers::tests`: Ashby, Greenhouse and Lever detection, several boards,
  invalid or unrelated links, custom pages, unsupported ATS, careers links;
- `tests/company_http.rs`: end to end against a mock site and ATS;
- `registry::tests`: validation, activation, health, lifecycle and the
  registry file.

## 11. Custom and unsupported pages

- **Custom page:** a careers page with no recognizable board. Usually the
  jobs load with JavaScript, or the site is first-party only. The URL is
  recorded and nothing is scraped.
- **Unsupported ATS:** a page links a known ATS that has no adapter
  (Workday, SmartRecruiters, Workable, BambooHR, Recruitee, Personio,
  Teamtailor, JazzHR, Breezy, Rippling, Dover, Gem, Pinpoint, iCIMS, Jobvite,
  Comeet, Homerun, Polymer, Wellfound, JOIN, Factorial). The provider is
  recorded.
- **Not done:** no JavaScript rendering, no CAPTCHA or bot-challenge
  handling, no stealth. A 403/429 homepage is recorded as `blocked`
  (DoorDash).

In the candidate run, 29 companies had custom pages and 8 used an
unsupported ATS: Gem ×2 (Retool, Hasura), Teamtailor, Homerun, Wellfound,
JazzHR, Personio, Workable (Hugging Face) and BambooHR (Bitso). Across
both runs Gem, Personio and Wellfound appear twice each, the rest once: no
provider is common enough yet to justify a new adapter on this evidence.

## 12. Candidate-company experiment

**Input:** `docs/source-discovery/candidates-2026-10-05.txt`, 85
companies chosen before probing. They are developer tools and
infrastructure, open-source companies, AI infrastructure, remote-first
international employers and Latin American tech companies, none of them
already known. Full results are in `discovery-2026-10-05.json` (no posting
text).

| | Count |
| --- | ---: |
| Companies attempted | 85 |
| Careers page found | 74 |
| Supported ATS board found | 43 companies, 44 boards |
| &nbsp;&nbsp;via the company's pages (careers page, homepage, redirect) | 20 |
| &nbsp;&nbsp;via slug guess | 24 |
| Adapter scan succeeded | 43 of 44 (Deno's board is not public: 404) |
| Unsupported ATS · custom page · no careers page · blocked | 8 · 29 · 5 · 0 |
| **Validated** | **38** |
| Candidate (ownership unproven) | 5: Metabase and Toptal (Lever postings never name the domain), Ghost ×2 (other companies' boards), Contentsquare (Hotjar's parent) |
| Rejected | 1: Deno (adapter 404) |
| **Ready to activate** | **3: Elastic, Fivetran, ElevenLabs** |

Ready boards:

| Board | Open | Eng | Open to BR (eng) | Unclear (eng) | <30 d | >1 y |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| greenhouse:elastic | 396 | 196 | 63 (60) | 0 | 127 | 0 |
| ashby:elevenlabs | 169 | 24 | 14 (2) | 1 | 33 | 12 |
| greenhouse:fivetran | 181 | 69 | 1 (0) | 32 (11) | 50 | 1 |

The other 35 validated boards are real but not useful for Brazil-remote
today. For example: MongoDB (390 postings, 0 open to Brazil), Docker,
1Password, Teleport, Coder (North America), LangChain (102, 3 open to
Brazil, 0 engineering), Baseten, Together AI (offices). They are recorded
with the unmet condition, so a later run can promote them when that
changes.

**Known-answer check.** The same tool was run on the domains of the 59
companies whose boards Narrow already reads or read in BRU-325
(`known-boards-2026-10-05.txt`, results in `known-boards-2026-10-05.json`):

| | Count |
| --- | ---: |
| Known board found | **54 / 59** |
| &nbsp;&nbsp;via the company's pages · via slug guess | 26 · 28 |
| Known board validated | 49 |
| Known board left as candidate (postings never name the domain) | 4: wikimedia, neon (now redirects to Databricks), wildlifestudios, pipedrive |
| Known board rejected | 1: inngest (its only posting is over a year old) |
| Not found | 5 |
| **Wrong boards validated** | **0** (8 extra boards found: 7 left inconclusive, 1 rejected) |

The 5 not found:
- DoorDash: 403, blocked;
- Automattic: custom JavaScript page;
- DuckDuckGo: no careers page at the usual paths;
- Axiom: its careers page links `ashby:axiom-co`, a stale board, while the
  live one is `ashby:axiom`;
- Remote.com: now on Personio; `greenhouse:remotecom` lists 0 postings.

Careers-page HTML alone finds about half of the known boards. The slug
guess, gated by the domain tie, roughly doubles recall without a false
validation.

## 13. First-party advantage experiment

**Sample.** 24 postings from the new production boards and discovered
boards, published 2026-09-25 → 2026-10-05, mostly the freshest and
engineering-weighted. Each row records company, title, first-party URL,
ATS, `posted_at` and Narrow's `first_seen_at`. Data:
`docs/source-discovery/first-party-sample-2026-10-05.json`. Measured at
about 2026-10-05T02:15Z.

**Controls:**
- **HiringCafe: not measurable.** hiring.cafe redirects to
  hiringcafe.com, which answers automated clients with a Cloudflare
  challenge (403 "Just a moment…"), API included. It was not bypassed.
- **Remotive: not usable.** Its public API ignores the `company_name` and
  `search` filters and returns the same 18 recent jobs.
- **Himalayas:** a public, documented search API (`/jobs/api/search?q=`)
  with its own publication timestamps. Each posting was searched by
  "title company" and by title; a hit needs a matching company and title.
- **General web search:** 6 postings.

| Result | Postings |
| --- | ---: |
| Absent from Himalayas at measurement time | **23 of 24** |
| &nbsp;&nbsp;…where Himalayas lists other jobs from the company | 6 (Elastic ×2, MongoDB, Mixpanel, Netlify, PagerDuty) |
| &nbsp;&nbsp;…where Himalayas' search shows no job from the company | 17 (coverage gap, not timing) |
| Present on Himalayas | 1: Wikimedia *Senior Software Engineer, Core Experiences (Contract)*: ATS `posted_at` 2026-09-25 23:29Z, Himalayas published 2026-10-01 05:44Z (**5.3 days later**) |
| Web search: not found | 4 of 6 (Elastic *Streams*, Teleport *Performance*, MongoDB legal, Mixpanel finance; older similar roles are indexed) |
| Web search: found | 2 of 6 (Amplitude's *Analytics Compute Platform* role; ElevenLabs' earlier LATAM posting of the per-country FDE role) |

**One qualitative finding.** Search engines and aggregators carry
Amplitude's role at a Greenhouse URL that now returns 404. Amplitude's
Greenhouse board is empty; the live posting is only on the Ashby board that
discovery found from `amplitude.com/careers`.

**Honest reading:**
- "First-party found 23 of 24 sampled postings absent from Himalayas at
  measurement time; 6 of them at companies Himalayas does index."
- The one posting Himalayas carried appeared there 5.3 days after the ATS
  publish date.
- **No discovery-time advantage is measured.** For a board added today,
  `first_seen_at` is when Narrow started reading it, so it says nothing
  about speed. A timing measurement needs postings published after
  monitoring starts. The 59-board monitoring set was scanned at
  02:09:52Z (5,063 open) and again at 02:31:49Z, and 0 postings appeared in
  between (late Sunday, US time). Nothing could be timed in this session
  (§20).
- The web-search sample is small, and search engines lag by design.

## 14. Freshness

The semantics were already distinct and stay so:
- `posted_at`: the source's publish date;
- `first_seen_at`: when Narrow first read the posting;
- `last_seen_at`: last seen listed;
- `content_updated_at`: last material change;
- verification `checked_at`.

`registry::Freshness` buckets postings by `posted_at` only. A posting
without one is **unknown**, never fresh. A 2022 posting read today counts
as over 365 days.

| Corpus (C, 21 sources) | Postings |
| --- | ---: |
| < 7 days | 185 |
| < 30 days | 771 |
| > 90 days | 1,097 |
| > 180 days | 642 |
| > 365 days | 303 |
| Publish date unknown | 15 (all YC) |

**Evergreen-heavy boards:**
- `lever:palantir`: 182 of 319 over a year old;
- `greenhouse:canonical`: 234 of 310 (75%), which is why Canonical, despite
  71 engineering postings open to Brazil, does not pass activation;
- Railway: 4 of 8 over a year old. These are kept, because they are its
  best roles.

**Found, not changed (ranking is frozen in this task).** Ranking's
freshness signal (`ranking/signals.rs`) falls back to `first_seen_at` when
a posting has no `posted_at`, and gives a +0.25 "First seen N days ago"
boost in the first week. Today only YC postings (15) lack `posted_at`, so
a newly added YC company's old postings would look fresh for a week. Every
Ashby, Greenhouse and Lever posting carries `posted_at`. Recommended
follow-up: treat an undated posting's freshness as unknown in ranking.

## 15. Dedupe

Identity was not changed, and no duplicate was introduced.

- **Same posting, two paths:** a board configured directly and found again
  through a careers page is one `SourceKey`, read once, so each job keeps
  one `JobId` and one opportunity. The test
  `a_careers_page_resolving_to_a_configured_board_is_read_once` (app,
  `discover.rs`) covers it.
- **New sources:** in C, 2,948 open records form 2,948 opportunities. The
  new boards have no look-alike in other sources.
- **Pre-existing duplicate:** the only cross-source look-alikes are the 6
  PostHog roles listed by both `ashby:posthog` and `yc:posthog`. The YC
  records carry no ATS id or shared URL, so they stay separate (documented
  in the README). `yc:posthog` adds no unique posting; the registry notes it
  as a candidate for disabling.
- **Not merged, correctly:** Elastic lists the same title several times
  with different Greenhouse ids (location variants). Those are separate
  postings, and title matching is never evidence.
- **ATS migrations** (Amplitude: Greenhouse → Ashby) would produce look-
  alikes if both boards were configured. Discovery reports the board the
  company's site points at now.
- **Provenance:** a job's provenance is its board. How the board was found
  (BRU-325 list, careers page, slug guess) is the registry's `provenance`.

## 16. Source health

`registry::health` reads the latest 5 scans of a source:

| Health | When |
| --- | --- |
| Healthy | the latest scan succeeded (listing or not-modified) |
| Degraded | 1–2 failures in a row, or an empty listing after a non-empty one |
| Unhealthy | 3 failures in a row, or "not found" twice in a row |
| Unknown | never scanned |

One transient error never makes a source unhealthy. All 21 proposed sources
are healthy. Production's own backoff (`source_schedule`) is unchanged. No
alerting was added. `narrow sources report --registry` shows a suggested
move (active → unhealthy) and never applies it.

## 17. Brazil/international usefulness

Buckets come from each posting's own location data
(`app::sources::geo_bucket`) and eligibility for the reference profile, not
from company marketing.

| Board | Bucket | Evidence |
| --- | --- | --- |
| ashby:railway | global / worldwide | 6 of 8 "Global"; eligibility unclear on time-zone wording |
| greenhouse:wikimedia | global (stated in the text) | 11 "Remote", 10 eligible for Brazil |
| ashby:oyster | Brazil explicitly supported, mixed | 5 name Brazil, 10 open to Brazil; mostly non-engineering |
| ashby:zapier | mixed, mostly North America | 7 NAMER, 1 South America, 3 open to Brazil |
| ashby:socket (held) | mostly US-only | 17 of 26 US, 0 open to Brazil |
| greenhouse:elastic (ready) | Brazil supported | 63 open to Brazil, 60 engineering |
| ashby:elevenlabs (ready) | Americas / LATAM, mixed | 14 open to Brazil; per-country LATAM postings |
| greenhouse:fivetran (ready) | mixed / unknown | 1 open, 32 unclear |

"Remote-first" again does not mean "hires in Brazil". Most validated
devtools boards (Teleport, 1Password, Docker, Coder, Help Scout) are North
America only.

## 18. Rejected sources

| Board | Why |
| --- | --- |
| ashby:socket | Held, not rejected: validated, fails activation (§3) |
| greenhouse:remotecom | Lists 0 postings; Remote.com now uses Personio |
| ashby:inngest | Its only posting is over a year old |
| ashby:deno | `deno.com/jobs` redirects to it, but Ashby answers 404 (not public) |
| ashby:ghost, greenhouse:ghost | Left as candidates: boards of other "Ghost" companies (ghst.io); Ghost uses Homerun |
| lever:contentsquare | Left as a candidate for Hotjar: its parent's board |
| Remote.com's linked boards (Jobgether, AIR, Veeam, JumpCloud) | Another company's boards on an aggregator page |
| ashby:axiom-co | Linked from axiom.co but stale-only; the live board is ashby:axiom |

Unsupported or custom pages are listed in §11 and §12.

## 19. Recommended next expansion batch

Validated and ready by the rule. Each is one config line plus a status
change in the registry:

1. **greenhouse:elastic**: 60 engineering postings open to Brazil.
2. **ashby:clickhouse**: 2 engineering open to Brazil, 5 open overall.
3. **greenhouse:gitlab**, **greenhouse:sourcegraph91**,
   **greenhouse:automatticcareers**: BRU-325 boards that pass today.
4. **ashby:resend**, **ashby:elevenlabs**, **greenhouse:fivetran**: small
   or mixed, but they pass.

Hold:
- **Canonical:** large, evergreen-heavy, and BRU-325 judged it noise.
- **Socket.**

Re-check:
- the 4 known boards left as candidates (one look by a person confirms
  them);
- disabling `yc:posthog`.

Measure each batch with the same before/after (`narrow sources report` plus
the probe), as here.

## 20. Rollout and deploy notes

- **Production config changed:** yes. `deploy/cloud.toml` goes from 17 to
  21 sources (Railway, Oyster, Zapier, Wikimedia).
- **Railway deploy required:** yes, and merging **is** the deploy.
  - The config is baked into the image, and `worker-discovery` (like `api`
    and `worker-verification`) auto-deploys `main` with `checkSuites: false`.
  - Production currently runs `82b690d` (SUCCESS).
  - On merge, confirm the *new* deployment for the merge commit reaches
    SUCCESS, then that the next cron run registers the 4 sources in
    `source_schedule` (`narrow admin status` lists the schedule).
  - The worker registers new configured sources itself, and `next_status`
    leaves the rest alone. No new Railway service.
- **DB migration:** none. The registry is a file, and health is read from
  existing tables.
- **Vercel:** no change.
- **Rollback:** remove the 4 entries from `cloud.toml` (and mark them
  `disabled` in the registry). Their jobs and history stay; the worker stops
  reading them.
- **Not deployed by this task.**

**Production rollout, verified 2026-10-05.** PR #47 merged as `278883a`.
- **Deployments:** `api`, `worker-discovery` and `worker-verification`
  each reached SUCCESS for `278883a` (14:06–14:11 UTC). The worker's
  deploy manifest is `narrow worker discovery` on cron `7,37 * * * *`,
  built from the `Dockerfile` (which bakes in the 21-source `cloud.toml`).
- **Scans:** the first run on the new image (14:11 UTC) read 13 due
  sources with 0 failures, including the 4 new ones, which all came back
  complete with 0 rejected: Railway 8 new, Oyster 23, Zapier 11,
  Wikimedia 11. The next run (14:38) read 0 sources.
- **Schedule:** the 4 sources were claimed as due, so they are in
  `source_schedule`, and they were not re-read at 14:38, so their
  `next_due_at` advanced. This was read from the worker logs, not from a
  direct query: Postgres has no public proxy.
- **Live funnel (real profile):**
  - Production's own feed log at 14:28 UTC: 2,941 considered, 4 strong
    fits, 1 company shown. It was 2,894 considered, 0 strong and 0 shown
    before the deploy.
  - A mirror of production (the 21 sources scanned at 14:37, with the
    real profile synced from production at 14:35) gives 2,939 open, 221
    actionable, 45 plausible, 4 strong, and Today of 1 company and 4 jobs
    (Railway).
  - Expected: 2,948 · 222 · 45 · 4 · 1/4. The differences are board
    churn: Oyster lists 23 postings, not 26, and has 16 actionable, not
    17.
- **Status:** BRU-343 closed. The next batch is in
  `source-expansion-batch-2a.md`.

**Timing follow-up.** To measure a discovery-time advantage, keep a
monitoring set (the 59 boards of this run), rescan on a schedule, and check
each newly appearing posting against Himalayas and web search right away,
then again after 1, 3 and 7 days. `first-party-sample-2026-10-05.json`
holds the first presence check.

## Reproduce

```bash
cargo build --release -p jobhunt-cli --bin narrow --example real_posting_probe
N=target/release/narrow
# configs: [storage]+[discovery] and the sources of A/B/C (see §4); DB: a
# copy of a profile-only database (jobs, events, evidence, verifications,
# eligibility decisions, rankings, scans and runs deleted)
$N --config C.toml --database c.db find --raw --refresh -n 1     # scan
target/release/examples/real_posting_probe C.toml c.db 2026-10-05T01:10:00Z > cell.json
$N --config C.toml --database c.db sources report --registry deploy/sources.toml
$N --config C.toml --database c.db sources discover \
  --file docs/source-discovery/candidates-2026-10-05.txt --json > discovery.json
```

The private databases, cell outputs and analysis scripts are in
`/tmp/bru343/` on the machine that ran this. No profile data is committed.
