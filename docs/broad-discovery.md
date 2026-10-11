# Broad discovery (BRU-360)

How Narrow finds companies and job boards it has never heard of, and
turns them into first-party sources a person can activate.

```text
broad discovery ─▶ candidate job / board / company ─▶ normalize + dedupe
  ─▶ read the board through its adapter ─▶ find the company's domain
  ─▶ the company's own site must point back (ownership)
  ─▶ validate (registry::validate, unchanged) ─▶ measure yield
  ─▶ activation candidate ─▶ a person adds it to cloud.toml ─▶ monitored
```

Broad discovery is for **coverage**. The first-party board is for
**truth, freshness and durability**: once a board is known, Narrow reads
the ATS directly, and a search engine or aggregator is never a source of
jobs. Today stays small and high-signal. No ranking rule, Fit semantics,
Today threshold, eligibility rule, application workflow or monetization
changed. Nothing is activated automatically and nothing is deployed.

The measured results are in
[`broad-discovery-experiment-2026-10-05.md`](broad-discovery-experiment-2026-10-05.md).
This document is the design.

---

## 1. Architecture

| Piece | Where | What it does |
|---|---|---|
| Lead, Sighting, Method | `jobhunt_sources::discovery` | What a provider found and where from. |
| Providers | `discovery::providers`, `discovery::import` | Search API, Common Crawl index, HN "Who is hiring?", yc-oss, remoteintech, and files: exported search results, CDX lines, URL/domain/CSV lists. |
| Query generator | `discovery::queries` (+ `queries.toml`) | The ATS search matrix, pairwise plan, rotation, reading families back from query text. |
| URL classifier | `discovery::ats` | A URL → job, board, unsupported ATS (+ tenant) or page; canonical board/job URLs; company domains. |
| Candidate store | `discovery::DiscoveryStore` | Deduped candidates, every sighting, results, statuses. A file (`discovery.json`). |
| Resolution | `discovery::resolve` (board → owner), `company::discover` (BRU-343, company → board), `discovery::sitemap` + `discovery::jsonld` (careers surface) | Reads boards, settles jobs, establishes ownership. |
| Orchestration and measurement | `jobhunt_app::broad_discovery` | Runs resolution in chunks, measures yield with `sources::yield_of` (job function, geography, freshness, reference eligibility), reads the person's ranking (rules only). |
| Report | `discovery::report` | Yield per strategy, family, query, host, place; ATS breakdown; unsupported-ATS map; activate / hold / recheck. |
| CLI | `narrow discovery …` | `queries`, `import`, `run`, `resolve`, `report`, `export`. |

**Reused from BRU-343, unchanged in behavior:** the adapters, the shared
HTTP client (per-host limits, retries, `Retry-After`), `careers::board_for_url`,
`careers::find_boards`, `careers::find_other_ats`, `company::discover`,
`company::check_board`, `registry::validate`, `registry::activation`,
`registry::Freshness`, `sources::yield_of`, `sources::geo_bucket`, the
reference profile, and the person's ranking from `source_report` (moved into
`LocalApp::person_yields`, same code). `jobhunt_jobs::ats_job_ref` gives job
ids from URLs.

**Small additions to BRU-343 code:**
- `BoardEvidence::Discovered`: a board found by broad discovery. It
  validates exactly like a slug guess (never as first-party); only the
  reason text differs.
- `company::retarget`: recomputes a checked board's ties (postings naming
  the company or its domain, the board's website) for a domain found after
  the board was read.
- `Deserialize` on `SourceYield`, `Freshness`, `GeoBucket`, `PersonYield`,
  `Validation` and `Verdict`, so results round-trip through the store.

## 2. The discovery gap

BRU-343 answers "given a company, where is its board?" (54 of 59 known
boards found, 0 wrong validations). Every company still had to be named by
a person: the 85 candidates were a hand-made list. Broad discovery answers
"which companies are hiring that Narrow doesn't know?", from sources that
list hiring surfaces themselves, so no company name is supplied.

## 3. Discovery-provider abstraction

A provider is anything that yields `Lead`s:

```rust
pub struct Lead {
    pub url: Option<String>,     // a job, a board, a careers page…
    pub domain: Option<String>,  // the company's domain, when the source names it
    pub company: Option<String>, // a name hint
    pub title: Option<String>,   // a title hint (never identity)
    pub sighting: Sighting,      // provenance
}

pub struct Sighting {
    pub method: Method,          // ats_search, web_index, hiring_thread, company_directory,
                                 // public_api, sitemap, jsonld, manual_import
    pub provider: String,        // websearch, brave, hn:<thread>, yc-oss, commoncrawl:<crawl>, …
    pub query: Option<String>,
    pub family: Option<String>,  // "platform engineer · latam"
    pub discovered_url: String,  // the URL as found
    pub discovered_at: DateTime<Utc>,
    pub rank: Option<u32>,
    pub title: Option<String>,
    pub derived: bool,           // seen through another candidate, not a hit of its own
    pub metadata: BTreeMap<String, String>, // host, geo group, seniority, crawled_at, regions…
}
```

Providers are not tied to a search engine. A provider can be:
- an API (`providers::brave_search`, `hn_thread`, `yc_companies`,
  `remoteintech`, `common_crawl`);
- or a file of results exported from anywhere (`narrow discovery import`):
  - JSON `{"provider", "results": [{"query", "url", "title", "rank"}]}`;
  - JSON lines (including Common Crawl CDX output);
  - CSV with `url`/`domain` columns;
  - one URL or domain per line.

Search is a lead generator, never a source.

## 4. ATS-index search strategy

Search engines index hosted job pages. A query restricted to an ATS host
lists companies hiring for a role in a place, whether or not Narrow knows
them:

```text
site:jobs.ashbyhq.com "staff platform engineer" LATAM
```

The hosts searched are `jobs.ashbyhq.com`, `boards.greenhouse.io`,
`job-boards.greenhouse.io`, `jobs.lever.co` and `jobs.eu.lever.co`. A hit
is only a URL; what it is worth is decided by reading the board (§12).

## 5. Query generation

The dimensions are data (`crates/jobhunt-sources/src/discovery/queries.toml`,
overridable with `--matrix FILE`):

- 13 role phrases;
- 5 seniorities (none, senior, staff, principal, lead);
- 4 place groups:
  - `global`: remote, global, worldwide;
  - `brazil`: Brazil;
  - `latam`: LATAM, Latin America, South America;
  - `americas`: Americas;
- 11 optional specialty terms.

How queries are chosen:
- **Full matrix:** 5 hosts × 13 roles × 5 seniorities × 8 place terms =
  2,600 queries, most of them near-duplicates.
- **Pairwise (the default):** every pair of values across host, role,
  seniority and place group appears in at least one query. It uses a greedy
  covering, is deterministic, and makes **65 queries**, the minimum possible
  (13 roles × 5 hosts).
- **Specialties (`--specialties`):** each specialty with each place group,
  the host rotating. 44 queries.
- **Rotation (`--rotate N`):** shifts which place term of a group and which
  host each query uses. Successive sweeps therefore search different words
  with the same coverage.
- **Families:** role (or specialty) × place group, e.g.
  `platform engineer · latam`. Yield is reported per query, family, host,
  place group and seniority.
- **Exported queries:** `QueryMatrix::read` recovers the family from the
  text of a query someone ran by hand.

`narrow discovery queries [--plan pairwise|full] [--specialties] [--rotate N] [--max N] [--json]`

## 6. Search execution and public discovery sources

**Search execution.**
- No search API key is configured for the project, and scraping a search
  engine's result pages is out of scope.
- The query matrix therefore runs through any provider that exports
  results. In the experiment, the generated queries were run with a normal
  web-search tool (the host as a domain filter) and the results were
  imported.
- `narrow discovery run search` calls the Brave Search API when
  `BRAVE_SEARCH_API_KEY` is set (tested against a mock). Any other API is
  one function returning `Lead`s.

Sources evaluated (measured numbers are in the experiment write-up):

| Source | Access | Coverage | Freshness | First-party resolution | Duplication | Limits | Complexity | Brazil/LATAM use |
|---|---|---|---|---|---|---|---|---|
| ATS-index web search | any search provider; exported results | 10 results per query, unknown companies | index lag; dead pages are common | direct: the hit is the ATS | high across queries | provider quotas | low | geo words in the query steer it |
| Common Crawl CDX (`index.commoncrawl.org`) | public, documented, no key | every crawled board/job URL of a host: thousands of boards | crawl snapshot (weeks old) | direct | low (URL-level) | slow and flaky; one request at a time | low | none (no keywords) |
| HN "Who is hiring?" (HN Algolia API) | public API | ~200–300 posts a month | monthly | ATS links or company domains | across months | generous | low | headers name REMOTE/LATAM/… |
| yc-oss dataset | public JSON on GitHub Pages | ~1,480 hiring YC companies | daily rebuild | domain → BRU-343 | none | none | low | `regions` field |
| remoteintech directory | public GitHub repo | ~880 remote-friendly companies | community-maintained, uneven | domain or careers URL | none | GitHub raw limits | low | `region` field (worldwide…) |
| Himalayas API | public, documented | 20 results/search | good | aggregator links | — | 20/page | low | some |
| Remotive API | public | ignores filters; ~18 jobs | — | — | — | — | — | not usable (BRU-343) |
| RemoteOK API | public | latest ~100 jobs | good | aggregator links | — | attribution | low | thin |
| HiringCafe | Cloudflare challenge | — | — | — | — | blocked | — | not measurable; not bypassed |
| Greenhouse/Lever/Ashby | no public directory endpoint | — | — | — | — | — | — | — |
| Search-engine result pages | scraping | — | — | — | — | ToS | — | **not used** |

**Integrated:**
- ATS search (importer plus a search API client);
- Common Crawl;
- HN;
- yc-oss;
- remoteintech;
- sitemap and JSON-LD as a careers-surface fallback.

**Not integrated:**
- Himalayas and RemoteOK: they point at their own pages. First-party
  resolution would mean matching the company name to a domain, which is a
  guess.
- Remotive: not usable.
- HiringCafe: blocked.

## 7. JobPosting JSON-LD

`discovery::jsonld::job_postings(html)` reads every schema.org
`JobPosting` on a page. A block may be one object, an array or an
`@graph`, and `@type` may be an array. Fields:
- title, hiringOrganization (name and `sameAs`/`url`);
- datePosted, validThrough, employmentType;
- jobLocation (address parts), applicantLocationRequirements,
  jobLocationType (`TELECOMMUTE`);
- description (to text), directApply, url, identifier.

`JsonLdPosting::ats_urls` returns the ATS links a posting names, in its
URL, its organization or its description. A posting found this way is
discovery input. Narrow follows it back to the board and never makes the
page itself a source.

## 8. Sitemap / careers-surface discovery

`discovery::sitemap::surface(domain, careers_url, limits)` runs when BRU-343
finds a careers page with no recognizable board (custom page) or no careers
page at all:
1. `robots.txt`: its `Sitemap:` lines and its `Disallow` rules for `*`.
   Those rules are honored.
2. At most 3 sitemap files, jobby ones first (`careers`, `jobs`,
   `positions` in the name). Sitemap indexes are followed and `.gz` files
   skipped. At most 20,000 URLs are looked at.
3. Job-like URLs:
   - on the company's site, a careers word followed by something more
     specific (`/careers/senior-engineer`);
   - or any ATS URL.
4. At most 3 job pages, plus the careers page, are read for board links,
   unsupported ATS and `JobPosting` JSON-LD.

A board found this way is on the company's own site, so it counts as
first-party evidence (`careers_page`) and is read and validated like any
other. Nothing is crawled beyond these limits, and nothing runs
JavaScript.

## 9. Candidate store

`DiscoveryStore`: a JSON file, one candidate per line. Each candidate keeps:

| Field | Meaning |
|---|---|
| `id` | stable id from the key |
| `key` | the dedupe key (§10) |
| `kind` | `job`, `board`, `company`, `unsupported_ats`, `page` |
| `url` | canonical candidate URL (the board's or the job's own) |
| `provider`, `board`, `job_id`, `tenant` | ATS, board slug, job id, unsupported tenant |
| `company_hint`, `domain_hint`, `title_hint` | hints from the sources |
| `first_discovered_at` | earliest sighting |
| `sightings[]` | every provenance record (method, provider, query, family, discovered URL, discovered at, rank, metadata) |
| `status` | `new`, `validated`, `inconclusive`, `rejected` (boards); `live`, `closed`, `unknown` (jobs); `resolved`, `unsupported_ats`, `custom_page`, `no_careers_page`, `blocked`, `unreachable` (companies); `ignored` (pages) |
| `reason` | why: the rejection reason, "not on the board's current listing", … |
| `last_checked_at` | last resolution |
| `source`, `registry_status` | the source key, and its registry status when Narrow already knows it |
| `board_result` | listing state, jobs, company, domain, careers page, domain hints, ownership, boards pointed at, validation, yield, unmet activation conditions |
| `job_result` | title, `posted_at`, `first_seen_at`, engineering, geography bucket, reference eligibility |
| `company_result` | careers page, boards, unsupported ATS, sitemap/JSON-LD summary |

Failed and rejected candidates are kept with their reasons. That is what
measures noise.

## 10. Dedupe

Keys, in order of preference:

| Candidate | Key | Example |
|---|---|---|
| job | provider + board + job id | `job:ashby:moxie:36c5bcce-…` |
| board | provider + board slug | `board:greenhouse:wikimedia` |
| company | registrable domain | `company:joinmoxie.com` |
| unsupported ATS | provider + tenant (or domain) | `ats:workday:acme` |
| anything else | canonical URL | `page:https://arc.dev/…` |

- **Same job from 12 queries:** one job candidate with 12 sightings.
- **Same board from many job URLs:** each job files a *derived* sighting on
  its board. That gives one board, which knows every query that surfaced
  it without counting them as hits of its own.
- **Same company from search + sitemap + directory:** one company
  candidate, every sighting kept.
- **Host spellings:**
  - `boards.greenhouse.io` and `job-boards.greenhouse.io` (EU too) are one
    board;
  - tracking parameters and `/application` suffixes are one job.
- **Never merged:**
  - jobs with the same title (title is never identity);
  - two boards of one company (Greenhouse for engineering, Lever for
    sales). They stay two boards, tied to the same company domain.

## 11. Stale search-index detection

Every job candidate is settled by its board's **current** listing (one
adapter read per board, however many jobs were found on it):
- **Live:** the job id is on the listing. The result records the
  provider's `posted_at`, separate from `first_seen_at` (when discovery
  first saw it).
- **Closed:** not on the listing ("not on the board's current listing"),
  or the board is gone (404) or empty.
- **Unknown:** the board could not be read (transient).

The stale rate (closed ÷ checked) is reported per strategy, family, query
and host, so a family that returns mostly dead jobs shows up as such.
Freshness uses `posted_at` only (`registry::Freshness`). A job without a
publish date is unknown, never fresh.

## 12. First-party resolution

**Companies → boards** reuse BRU-343 exactly (`company::discover`): the
homepage, careers links, the usual paths, the slug guess gated by a domain
tie, and an adapter check of every board. Then the sitemap and JSON-LD
fallback (§8).

**Boards → owners** run in two stages:
1. **Read:** one adapter request per board. This gives the listing state
   (`ok`, `not_found`, `empty`, `failed`, `transient`), the postings, and
   the settling of every discovered job. It is cheap, so it runs on every
   board.
2. **Ownership:** the company's domain from what the board says about
   itself (`DomainHint`):
   - where the hosted board redirects;
   - the website an Ashby board names;
   - postings hosted on the company's site (embedded Greenhouse boards);
   - company links on the hosted page (Lever's footer, Greenhouse's
     logo);
   - domains the postings mention;
   - and, ranked first, a domain the discovery source itself named (an HN
     post's link, a directory's website).

   The company's own site is then read with `company::discover` (no slug
   guessing):

| Ownership | When | Verdict |
|---|---|---|
| `verified` | the company's site links, embeds or redirects to this board, and `registry::validate` passes | validated |
| `corroborated` | a source independent of the board names the domain, and the postings tie to it (BRU-343's rule for guessed boards) | validated |
| `claimed` | only the board names its website; the company's site doesn't point back (a JavaScript careers page, a blocked site) | inconclusive: kept as a candidate |
| `elsewhere` | the company's site points at another board (the company moved ATS; the indexed board is a leftover) | inconclusive; the other board is filed as a new candidate |
| `unknown` | no domain found | inconclusive |

A board's word about itself is never proof of ownership, and neither is a
URL's shape. `registry::validate` runs unchanged on the evidence: listing,
postings, provider ids, and not stale-only. `narrow discovery resolve
--ownership useful` limits stage 2 to boards with an engineering posting
open to (or unclear for) a Brazil-based remote candidate, as a cost control.

## 13. Activation signals

For each validated board the report shows:
- ownership level;
- listing health;
- open postings;
- engineering postings;
- engineering open to / unclear for Brazil;
- the main geography bucket;
- the share over a year old;
- the person's actionable / plausible / strong counts (when scanned with a
  profile);
- the strategies that found it;
- `registry::activation`'s unmet conditions.

Buckets:
- **activate:** validated, meets the activation bar, has an engineering
  posting open to (or unclear for) Brazil. Ranked by the person's strong,
  then plausible, then Brazil-open engineering.
- **hold:** validated, below the bar.
- **recheck:** useful postings, but ownership not established (`claimed`,
  `elsewhere`, `unknown`). A person can confirm with one look.
- **reject:** gone, empty, stale-only, unstable ids.

`narrow discovery export --select ready --format config|registry` writes
them for a person to review. With `--registry FILE` it appends the boards
the registry doesn't list yet and leaves every existing entry as it is.

## 14. Continuous cloud discovery (design, not built)

```text
cron: discovery-sweep (daily)        cron: discovery-resolve (hourly)        worker-discovery (existing)
  rotate query slice ──┐               pending boards (read) ──┐               active sources only
  new HN thread (monthly)              boards worth it (owner) │               (cloud.toml, unchanged)
  directory diffs (weekly) ─▶ discovery_candidates ─▶ validated ─▶ activation_candidates ─▶ person ─▶ cloud.toml
  CC crawl (per release, ~monthly)       + sightings              (registry)         (review queue)
```

- **Storage:** Postgres tables replace the file:
  - `discovery_candidates` (key, kind, status, hints, results as JSONB,
    `first_discovered_at`, `last_checked_at`, `next_check_at`);
  - `discovery_sightings` (candidate key, method, provider, query, family,
    URL, `discovered_at`, rank, metadata).

  One migration, additive, no change to existing tables.
- **Cadence:**

  | Input | When |
  |---|---|
  | ATS search | one rotation slice per day (≈15 queries; a full pairwise pass every ~4–5 days) |
  | HN | each new monthly thread, re-read daily for its first week |
  | yc-oss and remoteintech | weekly diff (new or changed entries only) |
  | Common Crawl | each new crawl (~monthly), only URLs whose board isn't already known |
  | Board reads (stage 1) | for new boards only |
  | Ownership (stage 2) | once per new board; again only when it was `claimed`/`elsewhere` and ≥14 days have passed |
- **Dedupe window:**
  - a sighting identical to one already stored (method, provider, query,
    URL) is a no-op;
  - a board already validated, rejected or active is not re-read by
    discovery: the production worker monitors active boards, and the
    others are rechecked on a schedule;
  - a job candidate is settled once, and again only while its board is
    pending.
- **Rate limits:**
  - the existing per-host limits (3 per ATS host in production);
  - one Common Crawl request at a time, with pauses;
  - search provider quotas (Brave's free tier is 1 query/s, 2,000 a
    month);
  - company sites at most 6–8 pages each.
- **Retry and backoff:**
  - transient failures leave a candidate `inconclusive`, with
    `next_check_at` doubling from 1 day up to 30;
  - three conclusive failures reject it, with the reason kept.
- **Aging:**
  - a `claimed`/`unknown` board not confirmed in 90 days is archived;
  - a closed job candidate is final;
  - rejected boards are rechecked every 90 days, in case a company returns
    to an ATS.
- **Health metrics:**
  - candidates per day, by strategy;
  - stale rate and first-party rate per strategy;
  - validated per day;
  - activation-queue size;
  - provider errors (Common Crawl 5xx rate, search API errors).
- **Cost controls:**
  - stage 2 only for boards that list postings, and with
    `--ownership useful` only for useful ones;
  - query families below a yield floor are disabled (§16);
  - no full-crawl of anything.

## 15. Automatic promotion (proposal, not enabled)

`validated → activation_candidate → active` could become automatic when
**all** hold:
1. ownership `verified` (the company's site points at the board), not
   only corroborated;
2. at least 3 successful scans over at least 7 days, with stable provider
   ids (≥ 90%) and no unhealthy period;
3. at least one engineering posting open to Brazil, or a remote scope
   that includes it (global, Brazil, the Americas), posted in the last 60
   days;
4. fewer than 50% of dated postings over a year old;
5. no `elsewhere` signal, no second board of the same company already
   active, and no duplicate-rate warning (postings matching an active
   board's ids or URLs);
6. a weekly cap (e.g. 10 boards), and every promotion logged with its
   evidence, revertible by one config change.

Until a measured period shows these boards behave like the hand-activated
ones, activation stays a person's decision. What this task removes is
manual company *discovery*.

## 16. Query feedback loop

Per family (role/specialty × place group) and host, from the report:
- **high yield:** useful validated boards per query ≥ 1, and a stale rate
  below 50%. These run every rotation.
- **medium:** some useful boards, or mostly new companies. These rotate,
  running every 2–3 sweeps.
- **low/noisy:** no useful board in two sweeps, or a stale rate of 80% or
  more, or a duplicate rate of 90% or more. These are disabled and retried
  quarterly.

This is a counter per family, not a model. Each sweep's sightings carry
their query and family, so the counts come straight from the store.

## 17. First-party moat measurement

Three claims stay separate:
- **Coverage advantage:** the job is absent from the comparison source at
  measurement time.
- **Freshness advantage:** Narrow saw the job *before* the comparison
  source. This is only measurable for jobs first published after
  monitoring began. Historical ingestion proves nothing; `first_seen_at` of
  a newly added board is when Narrow started reading it.
- **First-party durability:** Narrow follows the authoritative board when
  aggregators show stale links (Amplitude's Greenhouse URL, BRU-343).

The cohort and procedure are in the experiment write-up (§ timing).

## 18. CLI

```bash
narrow discovery queries [--specialties] [--rotate N] [--plan full --max N] [--json]
narrow discovery import results.json crawl.jsonl companies.csv urls.txt [--method M] [--provider P]
narrow discovery run hn --months 3
narrow discovery run yc
narrow discovery run remoteintech
narrow discovery run web-index --crawl CC-MAIN-2026-39 --host jobs.ashbyhq.com [--pages N]
narrow discovery run search                  # BRAVE_SEARCH_API_KEY
narrow discovery resolve [--registry deploy/sources.toml] [--ownership all|useful|off] [--strategy S] [--limit N] [--shuffle-seed N]
narrow discovery report [--registry deploy/sources.toml] [--json] [--top N]
narrow discovery export --select ready|useful|validated --format config|registry [--registry FILE]
narrow sources discover <domain>              # unchanged: company → board
```

Every command takes `--store FILE` (default `discovery.json`). `resolve`
saves after every chunk, so it can be stopped and resumed.

## 19. Deploy impact

- **Migrations:** none. The store is a file, and production tables are
  untouched.
- **Railway:** none. No service, cron or config change. `cloud.toml` is
  unchanged (21 sources). The registry gains 134 `validated` entries,
  appended; no existing entry changes.
- **Vercel:** none.
- **The continuous version** (§14) would need one additive migration
  (two tables) and two crons on `worker-discovery`'s image.
