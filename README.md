# Narrow

Narrow is an open-source job-search agent that searches the market for you
and surfaces only the opportunities worth your attention. It is not another
job board: the goal is a short list you can act on, not an endless one to
scroll.

This repository is Narrow's code. The command is `narrow`. Its internal name
is JobHunt: the crates, the API, the environment variables and the file
locations keep that name, and so does the rest of this README's prose.
Narrow is licensed under [Apache-2.0](#license).

High-signal job discovery, as a local product. JobHunt reads a large
universe of jobs from company job boards, verifies them at the employers'
own sources, checks whether you can take them, learns what you want from
what you say and do, and shows you the few worth your time, with why.

Everything lives on your machine, in one SQLite database. You use it from
the terminal (`narrow`), or from an AI assistant that speaks the Model
Context Protocol (`narrow mcp`): both are interfaces to the same
application, the same profile, jobs, rankings and feedback.

Optionally, [JobHunt Cloud](#jobhunt-cloud) keeps discovering and
verifying jobs while your laptop is closed: `narrow login`, `narrow
sync`, and the same use cases over an HTTP API and a hosted MCP endpoint.
The local product never needs it.

```text
$ narrow find
Checked 11 open jobs
11 passed basic eligibility
6 looked plausible
3 are worth reviewing

  1. Member of Technical Staff - Systems — Modal   Worth reviewing
     backend, infrastructure · Linux · infrastructure, ai · USD 220,000 – 300,000 per year · on-site
     ✓ Verified open just now · ✓ Eligible: The on-site office is in New York, where you live
     Why this may be worth your time
       + Senior backend work (from what the description asks for), the kind of engineering you want
     opp_c22d474a · narrow why opp_c22d474a

  2. Security Engineer, Cloud — Ramp   Worth reviewing
     infrastructure, security · AWS, Terraform · security, infrastructure · USD 211,400 – 290,600 per year · hybrid
     ✓ Verified open just now · ✓ Eligible: The listing allows remote work from the United States
     Why this may be worth your time
       + Touches infrastructure (the team: “Cloud”), close to the infrastructure work you want
       + Senior level, matching your latest title
     opp_55a62bd9 · narrow why opp_55a62bd9

  3. …

Not shown: 8 maybe or low priority (--all).
Used stored jobs (every source read 2 hours ago; --refresh reads the job boards now).
Ranked against your profile. Tiers are coarse on purpose; `narrow why <id>` shows every reason.
```

Tiers are deliberately coarse (strong fit, worth reviewing, maybe, low
priority) and say how well each job fits what you want; there is no match
percentage. Here only the work matches what this person asked for
("backend or infrastructure roles"): worth reviewing, not strong fits,
however well paid. Say more (the kind of company, team, level, depth) and
the jobs that match it all become strong fits. Verification and eligibility are
on every line, and every reason can be traced to its evidence
(`narrow why <id> --details`).

## Install

You need a stable Rust toolchain (1.88 or newer, the declared minimum
supported version) and a C compiler (for the bundled SQLite and TLS
libraries). No database server, account or API key is needed.

```bash
cargo install --path crates/jobhunt-cli   # installs `narrow`
# or, from a checkout:
cargo build --release && ./target/release/narrow --help
```

## First run

```bash
narrow init resume.pdf
narrow preferences describe "Small product teams, backend/platform work, startups. Remote from Brazil, at least USD 120k. No early-career roles."
narrow find
```

1. `init` builds your career profile from your resume (PDF, `.txt` or
   `.md`): experiences, projects, skills, domains, and an evidence graph in
   which every claim keeps the resume text it came from (see
   [Career profile](#career-profile)).
2. `preferences add` records what you want, in your own words; what
   JobHunt understood is printed, and whatever it didn't understand is kept
   and shown, never dropped (see [Preferences](#preferences)).
3. `find` reads the configured job boards (the first time, or when the
   stored jobs are stale), ranks every open opportunity against your
   profile, verifies the best candidates at their sources, and shows the
   few worth reviewing.

Optionally, add evidence the resume may not carry: your LinkedIn data
export (`narrow profile import-linkedin export.zip`) and your public GitHub
repositories (`narrow profile import-github <user>`). Both feed the same
profile; what Narrow concludes from them waits for your review (see
[LinkedIn and GitHub evidence](#linkedin-and-github-evidence)).

Without a profile, `find` lists what it found and says how to start;
without preferences nothing is a strong fit (a job earns attention on what
you want, never on eligibility or freshness alone), and it says so.

## The loop

```bash
narrow find                     # the shortlist (fast: works from stored jobs while they are fresh)
narrow show <id>                # everything about one opportunity, every source, its history
narrow why <id>                 # the decision brief: why, caveats, unknowns (--details: every signal)
narrow verify <id>              # still open? can you apply? can you take it? (asks the source now)
narrow save <id>
narrow reject <id> --reason "too corporate"
narrow like <id> --reason "tiny team and strong ownership"
narrow applied <id>
narrow pipeline                 # what you saved, applied to, interview for
narrow find                     # reflects all of it
```

Ids are opportunity ids (`opp_…`): one job, however many boards list it.
Feedback, pipeline state, rankings and decision briefs belong to the
opportunity; provenance and verification stay per source record (`show`
and `verify --details` list every one). Commands take the short form `find`
prints (`opp_c22d474a`), any unique prefix, a full `opp_…` id, or a
source record's `job_…` id.

Doing something twice is safe: saving a saved job, applying twice, or the
same rejection with the same reason changes nothing (`Already saved …:
nothing changed.`); a rejection with a new reason is new information and is
recorded.

### `find`: the shortlist, and when it reads the network

`find` is the one command for finding opportunities worth your time.

| Situation | What `find` does |
| --- | --- |
| every configured source read within `discovery.refresh_after_hours` (default 12) | works from stored jobs: no discovery requests |
| a source never read, or read longer ago | refreshes first (`Refreshing 17 sources (sources last read 2 days ago)…`) |
| `--refresh` | always refreshes |
| `--offline` | no network at all: no refresh, no verification |
| a refresh nobody asked for can't reach any source | warns, and answers from stored jobs |
| the best candidates aren't verified recently | verifies them (a few requests; attempts from the last 15 minutes are reused) |

So a second `find` right after the first makes no request at all. Other
options: `WORDS` (every word must match), `-n N` (default 5, at most 25),
`--all` (also maybe and low-priority opportunities), `--json` (the same
structure the MCP `search_jobs` tool returns).

`find --raw` is the inventory instead: every matching stored job, unranked,
one per opportunity, with its source, URL and (with a profile) a one-line
eligibility verdict. `--source KIND:NAME|URL` (read only that source),
`--eligible` and `--possible` imply it:

```bash
narrow find --raw rust backend -n 50
narrow find --source greenhouse:stripe
narrow find --source https://www.notion.com/careers
narrow find --raw --offline --eligible
```

`narrow rank` (the shortlist from stored jobs only, like `find
--offline`) still works for scripts written before `find` was
personalized; it is no longer listed in `--help`.

### Everything else

```bash
narrow check <id>               # the full verification and eligibility report, from what is stored
narrow taste                    # what JobHunt learned from your feedback, with evidence
narrow feedback [<id>]          # your feedback, verbatim
narrow profile                  # your profile (edit, add, remove, export, import, history)
narrow claims review            # evidence waiting for your decision
narrow context <id>             # the evidence an assistant may use for an application (JSON)
narrow export -o narrow.json   # everything that is yours, one file
narrow import narrow.json      # …restored, atomically
narrow mcp                      # serve all of this to an MCP client
narrow doctor                   # check the setup; prints the MCP command for your client
narrow config                   # file locations and the effective configuration
```

`show`, `verify`, `pipeline` and `find` take `--json` and print the same
structures the MCP tools return. Global options: `--config PATH`,
`--database PATH` (or `JOBHUNT_CONFIG`, `JOBHUNT_DATABASE`), `-v`,
`--log-format json`.

### How search works

Each search word must match the start of a word in the job's title, company,
department, team, locations or workplace type, ignoring case and punctuation.
`rust` matches "Backend Engineer (Rust)" but not "Trust & Safety", and
`node.js` matches "Node.js".

The shortlist considers every open stored opportunity matching the words.
`find --raw`, after a refresh, shows open jobs that were listed during that
refresh, one per opportunity; jobs of a source that failed are not shown for
that run (a warning names the source) and stay open in the database.
Without a refresh (`--offline`, or stored jobs still fresh) it shows every
open stored job.

## Sources

| Kind | Instance | Reads | Complete listing? |
| --- | --- | --- | --- |
| `ashby` | board (`jobs.ashbyhq.com/<board>`) | `api.ashbyhq.com/posting-api/job-board/<board>` | yes, one response |
| `greenhouse` | board token (`job-boards.greenhouse.io/<board>`) | `boards-api.greenhouse.io/v1/boards/<board>/jobs?content=true&pay_transparency=true` | yes, when the job count matches `meta.total` |
| `lever` | site (`jobs.lever.co/<site>`) | `api.lever.co/v0/postings/<site>?mode=json` (or `api.eu.lever.co`) | yes, one response |
| `yc` | company slug (`ycombinator.com/companies/<slug>`) | `/companies/<slug>/jobs` plus each job's page | yes, per company |

All sources use plain HTTP through one shared client: a `jobhunt/<version>`
User-Agent, timeouts, bounded retries with exponential backoff for timeouts,
connection errors, 429 and 5xx (honoring `Retry-After`), at most 4 requests
in flight per host, and connection reuse. Nothing uses a browser or an LLM.

Source notes, from real data:

- **Ashby** does not return the company name; it comes from config.
- **Greenhouse** job content is HTML escaped once more; it is unescaped into
  `description_html` and converted to readable `description_text`.
  `absolute_url` is often the company's own page
  (`stripe.com/jobs/search?gh_jid=…`). Workplace and employment type exist
  only as company-specific custom fields ("Location Type", "Workplace Type",
  "Employment Type"), used when present. Pay ranges keep their labels
  ("Canada Annual Pay Range").
- **Lever** splits the description into opening, titled lists, "additional"
  and salary text; they are joined in page order. Commitments are free text:
  only unambiguous ones ("Full-time", "Contractor", "Internship") map, others
  ("Permanent", "Fixed-Term") are kept verbatim. `createdAt` is used as the
  posting date; Lever has no update time. Some sites return `[]` instead of
  404 when they have (or moved) no postings.
- **YC / Work at a Startup** has no jobs API. Its public pages are Inertia.js
  pages that embed their data as JSON in a `data-page` attribute; the adapter
  reads that JSON over HTTP (no HTML scraping, no browser). Descriptions are
  Markdown and only on each job's page, so a company with N jobs costs N+1
  requests. Posting dates are only published as relative text ("5 months"),
  so they stay unknown. Pay is published as text ("$190K - $215K"); a bare
  `$` does not say which dollar, so its currency stays unknown. The
  company's own visa field ("US citizen/visa only", "Will sponsor") is kept
  as `work_authorization`. The site-wide `/jobs` pages show a sample, not a
  complete list, and are not used.
- **Careers pages** are not scraped. JobHunt fetches the page, looks for a
  supported board it links to or embeds (`jobs.lever.co/<site>`,
  `boards.greenhouse.io/embed/job_board?for=<board>`,
  `jobs.ashbyhq.com/<board>`, …) and reads that board. Pages that only load
  their jobs with JavaScript (Stripe's, Anthropic's) can't be detected;
  configure their board instead.

To find the board of a company you care about, give its domain to
`narrow sources discover railway.com` (or a file of domains with `--file`):
it looks for the careers page from the homepage and the usual paths, finds
the board it links to or embeds, and, when no page names one, tries the
company's name as a board slug, accepting it only when something ties the
board to the company's domain. Every board found is read with its adapter
and validated; nothing is added to the configuration. `narrow sources
report` shows what each stored source yields (open, engineering, open to a
Brazil-based remote candidate, freshness by publish date, and your own
ranking counts) and its health from recent scans. See
[docs/source-discovery-and-career-pages.md](docs/source-discovery-and-career-pages.md).

Across the source families, unknown stays unknown: a field a source does not
publish is `None`, never guessed.

## Configuration

Everything has a default, so no config file is needed. To customize, copy
[`config.example.toml`](config.example.toml) to the config path shown by
`narrow config`. Settings are applied in this order, later winning: built-in
defaults, then the config file, then environment variables, then
command-line flags.

Sources are listed per family:

```toml
[[sources.ashby]]
board = "linear"
company = "Linear"        # display name; Ashby's API has none

[[sources.greenhouse]]
board = "stripe"          # company name comes from Greenhouse

[[sources.lever]]
site = "spotify"
company = "Spotify"
# region = "eu"           # for jobs.eu.lever.co sites

[[sources.yc]]
slug = "posthog"

[[sources.careers]]
url = "https://www.notion.com/careers"
```

With no `[sources]` at all, a built-in selection from every family is used.
Listing any source replaces all of the defaults, so the file fully controls
what is searched. Invalid names, unknown fields, duplicated sources and
non-http careers URLs are rejected with a message naming the problem.

`[discovery]` controls fetch concurrency (`concurrency`, default 8 sources;
`max_requests_per_host`, default 4), timeouts and retries,
`revalidate_after_hours` (default 24, see below), and
`refresh_after_hours` (default 12): how long stored jobs count as fresh, so
`find` and the MCP `search_jobs` tool answer from them without reading the
boards (see [`find`](#find-the-shortlist-and-when-it-reads-the-network)).

`narrow mcp` reads exactly the same file and database as every other
command; there is no MCP-specific configuration.

## Where data lives

| What | Default location |
| --- | --- |
| Database (jobs and your profile) | macOS: `~/Library/Application Support/jobhunt/jobhunt.db`<br>Linux: `~/.local/share/jobhunt/jobhunt.db` |
| Config file (optional) | macOS: `~/Library/Application Support/jobhunt/config.toml`<br>Linux: `~/.config/jobhunt/config.toml` |

`narrow config` prints the exact paths on your machine. Use a different
database with `--database <PATH>` or `JOBHUNT_DATABASE`, and a different
config file with `--config <PATH>` or `JOBHUNT_CONFIG`. The database is
created and migrated automatically on first use; delete the file to start
over.

## Discovery lifecycle

```text
begin run
  │
Source::fetch(request)     adapter: HTTP fetch (conditional when possible), parse,
  │                        convert to canonical JobPostings; bad records become
  │                        per-record errors, the rest continue
  ▼
validate                   canonical invariants, drop in-batch duplicates,
  │                        decide whether this listing may close missing jobs
  ▼
JobRepository::apply_scan  lifecycle plan → one transaction per source:
  │                        rows, history, identity evidence, scan record
  ▼
identity::group            cross-source equivalence → opportunities
  ▼
finish run                 run statistics
```

Sources are fetched concurrently (bounded). A failing source is recorded and
reported; the others complete. Only a storage failure stops the run, so data
is never silently lost.

### NEW / UNCHANGED / UPDATED / CLOSED

Each source record (one job at one source) is classified on every scan of
its source ([`jobhunt_jobs::lifecycle`](crates/jobhunt-jobs/src/lifecycle.rs)):

| Stored state | In this scan | Result |
| --- | --- | --- |
| never seen for this source identity | listed | **NEW** |
| open, same material content | listed | **UNCHANGED** (only `last_seen_at` moves) |
| open, material content changed | listed | **UPDATED** |
| open | missing from a *trustworthy complete* listing | **CLOSED** |
| open | missing from a failed or partial scan | stays open |
| closed | listed again | **REOPENED** (open again; history shows both) |

"Material content" is title, company, URLs, department, team, locations,
employment and workplace type, remote flag, compensation, description text
and posting date. Markup-only changes to the description HTML and the
source's own update timestamp rewrite the stored row but are not UPDATED.

A listing can close jobs only when all of these hold:

1. the fetch succeeded and the adapter vouched the listing is complete
   (Greenhouse: job count equals `meta.total`; YC: the company page loaded);
2. every rejected record carried its source id (a record the source lists
   but that failed to convert is kept open, never closed; one without an id
   could be any job, so nothing closes);
3. it is not a sudden empty listing: an empty listing after a non-empty one
   closes nothing until a second consecutive empty listing confirms it.

When a source answers "not modified" (HTTP 304 to `If-None-Match`), its open
jobs are marked seen and nothing is re-read. That validator is only reused
from a listing that was fully applied, recorded under the current conversion
revision (`CANONICAL_REVISION`), and at most `revalidate_after_hours` old;
after that the listing is read in full again.

Closed jobs are never deleted. `find` hides them; `show` shows them.

## Identity and deduplication

There are two kinds of duplicates.

**The same job at the same source** is solved by stable ids. A job id
(`job_<32 hex>`) is a hash of the source (kind and instance) and the
source's own id for the posting (or the canonical URL when it has none). The
same posting always gets the same id on every machine, so repeated scans
update rows instead of adding them.

**The same job at different sources** is handled by grouping source records
into *opportunities* (`opp_<32 hex>`), without merging or changing the
records themselves
([`jobhunt_jobs::identity`](crates/jobhunt-jobs/src/identity.rs)). Two
records are linked only by deterministic evidence:

- the same ATS job recovered from their URLs: a Greenhouse job id
  (`boards.greenhouse.io/<board>/jobs/<id>`, `job-boards…`, embeds, or a
  first-party page's `?gh_jid=<id>`), a Lever posting id, an Ashby job id
  (`jobs.ashbyhq.com/<board>/<uuid>` or `?ashby_jid=`), or a Work at a
  Startup job id;
- an identical canonical posting URL or application URL.

False merges are worse than duplicates, so there are safeguards: a key two
records *of the same source* share (a generic "apply here" URL) is ignored;
linked records must have compatible titles (equal, or one a whole-word part
of the other); and two groups that each contain a record of one source are
never merged. Similar titles, companies or locations are never evidence on
their own. Records from different sources with the same company and title
but no shared evidence are counted as *look-alikes* in the statistics
(`find -v`) and left apart. PostHog's jobs on both Ashby and YC are the live
example: YC exposes no link to the Ashby posting, so they appear twice.

An opportunity is named after its earliest-seen record, so a job listed by
one source has `opp_` + its own job id's hex, and the id stays put as other
sources join. `find` shows one record per opportunity; `show` lists them all
with their own provenance.

**Canonical URLs** (`jobhunt_core::CanonicalUrl`): scheme and host are
lowercased, and default ports, credentials, trailing slashes, anchor
fragments and tracking parameters (`utm_*`, `gclid`, `gh_src`,
`lever-source`, `lever-via`, …) are removed; remaining parameters are sorted.
`www.`, `http`, path case and parameters that identify a job (`gh_jid`) are
kept. Host or path variants of one ATS posting are related by the identity
layer above, not folded into one URL.

## Database and history

SQLite, with migrations in
[`crates/jobhunt-storage/migrations/sqlite/`](crates/jobhunt-storage/migrations/sqlite/).
Migrations are additive; a database written by an earlier version is
upgraded in place (its jobs become open, get a `new` history entry, and are
baselined without being reported as UPDATED).
`20260929000000_job_work_authorization.sql` adds the `work_authorization`
column (Work at a Startup's visa field); the canonical revision was bumped
so every source is read again and fills it.
`20260930000000_verification_eligibility.sql` adds the verification and
eligibility tables below without touching any existing table, and
`20261001000000_feedback_ranking.sql` the feedback and ranking tables.

| Table | Holds |
| --- | --- |
| `jobs` | one row per source record: canonical content, status (`open`/`closed`), `first_seen_at`, `last_seen_at`, `content_updated_at`, `closed_at`, fingerprints, `opportunity_id` |
| `job_events` | history, one row per *change*: `new`, `updated` (with changed fields and the replaced version as a JSON snapshot), `closed`, `reopened` |
| `job_evidence` | identity keys (`url:…`, `ats:<system>:<id>`) used for grouping |
| `source_scans` | one row per source per run: listing / not modified / failed, complete or not, whether closing was applied or withheld and why, all counts, the validator |
| `discovery_runs` | one row per run with totals |
| `job_verifications` | one row per verification attempt: listing, application and authority states, whether it succeeded, the method, URL and failure kind as columns, and the whole record (authority chain, compensation facts, published places, unknowns) as JSON |
| `eligibility_decisions` | stored eligibility decisions, keyed by opportunity, profile and a digest of every input (see [Caching and revisions](#caching-and-revisions)) |
| `opportunity_feedback` | one row per action you took on an opportunity (save, reject, applied, …), with your reason verbatim; never updated or deleted (see [Ranking, feedback and taste](#ranking-feedback-and-taste)) |
| `opportunity_rankings` | rankings that were shown to you, keyed by a digest of every input |

Unchanged observations add no history, so history grows with changes, not
with scans. The current row plus the chain of replaced snapshots answers:
when did this job first appear (`first_seen_at`), when was it last verified
(`last_seen_at`), did its pay or description change (`updated` events with
`compensation` / `description`), and was it closed and reopened.

The domain talks to storage only through `JobRepository`, which reports
`StorageError`, never driver errors; lifecycle decisions are made in the
domain, so a Postgres backend (its own module and `migrations/postgres/`)
would only persist them. Timestamps are fixed-width UTC text and structured
values JSON, both mapping directly to Postgres (`timestamptz`, `jsonb`). The
`jobs` table holds no per-user data: in a hosted setup a job is fetched once
for everyone, and per-user state belongs in separate tables keyed by job or
opportunity id.

## Career profile

```text
$ narrow init resume.pdf
Imported resume.pdf (2 pages)
ignored 3 lines repeated on every page (header, footer, page numbers)
Marina Costa — Senior Software Engineer · Backend & Platform

Experience
  Ledgerly — Senior Software Engineer · Mar 2022 – Present · Remote
  Banco Horizonte — Software Engineer II · Jan 2020 – Apr 2022 · São Paulo, Brazil
  Banco Horizonte — Software Engineer · Jun 2018 – Dec 2019 · São Paulo, Brazil
  Full Stack Developer (freelance) · dates unknown

Skills
  PostgreSQL, AWS, Rust, Docker, Kafka, Kubernetes, React, Terraform, … (+9 more)

Preferences
  none yet

Evidence
  77 claims extracted
  56 directly supported
  21 need review

Uncertain
  - Full Stack Developer: no dates found

Next
  narrow profile
  narrow claims review
  narrow preferences add "I want … and at least …; avoid …"
```

Everything JobHunt believes about you can be inspected (`narrow profile`,
`narrow claims`), traced to its source (`narrow claims show <id>` prints
the resume sentence behind a claim), corrected (`narrow profile edit`,
`narrow claims reject`), and exported. Nothing needs an API key or the
network.

### Commands

| Command | Does |
| --- | --- |
| `narrow init <resume>` | Import a resume (PDF, `.txt` or `.md`), or re-import an updated one |
| `narrow profile [--all]` | Experience, projects, education, skills (used vs only listed), domains, role signals, preferences, evidence counts, what is missing or uncertain |
| `narrow profile edit basics\|experience\|project\|education …` | Correct a value (`--title`, `--start 2021-03`, `--end none`, `--current`, `--tech Rust,Go`, …) |
| `narrow profile add experience\|project\|education\|skill …` | Add what the resume does not say |
| `narrow profile remove <id>` | Delete what you added; reject (hide) what was imported |
| `narrow profile export [-o file]` / `import <file> [--replace]` | Versioned JSON, all or nothing |
| `narrow profile history` | Every import, decision and edit |
| `narrow profile import-linkedin <export.zip\|folder\|file.csv>` | Add evidence from your LinkedIn data export (career files only) |
| `narrow profile import-github [user]` | Add evidence from your public GitHub repositories (default: the GitHub link on your profile) |
| `narrow profile remove-source linkedin\|github` | Take that source out: what only it supported goes, your decisions stay |
| `narrow claims [--kind K] [--state S] [--for <id>] [--all]` | List claims, marked ✓ usable, ? needs review, ✗ rejected |
| `narrow claims review [--all]` | Claims needing review, each with why JobHunt believes it and the resume text |
| `narrow claims show\|confirm\|reject\|reset <id>…` | Inspect or decide (`reject --reason …`) |
| `narrow claims add "…" [--for <id>] [--kind K]` / `edit <id> "…"` | State or reword a claim yourself |
| `narrow preferences` | What Narrow understands you want (your taste profile) and your practical constraints; `show --all` adds every structured preference and statement |
| `narrow preferences describe "…"` | What kind of job you're looking for, in a few words: read into a short summary to confirm or correct |
| `narrow preferences confirm [taste_…]` | "Looks right": the whole summary, or some statements |
| `narrow preferences correct <taste_…> ["…"] [--polarity prefer\|open\|avoid\|neutral]` | Correct one statement; `neutral` is "doesn't matter". Your corrections always win |
| `narrow preferences reinterpret` | Read your description again, keeping every correction |
| `narrow preferences add "…"` | A preference statement in your own words |
| `narrow preferences set role\|compensation\|work-mode\|work-setup\|location\|region\|timezone\|relocation\|sponsorship\|company\|domain\|work-style\|unknown-pay\|unclear-eligibility …` | One structured preference |
| `narrow preferences remove <pref_…\|stmt_…\|taste_…>` | Remove a preference, a statement and what was read from it, or a statement of the summary (never read again) |

Ids are printed short (`clm_3fa2b1c4`); any unique prefix works. `claim`
and `prefs` are accepted as aliases.

### Architecture

```text
jobhunt-cli ──► jobhunt-resume ──► jobhunt-profile ──► jobhunt-core
     │                                   ▲
     └──────► jobhunt-storage ───────────┘  (ProfileRepository for SQLite)
```

- **`jobhunt-profile`** is the domain: the model, the evidence policy,
  preferences and their parser, the re-import rules, the export format, and
  `ProfileService` (the use cases the CLI calls, and MCP and web will). It
  knows nothing about SQL or PDFs; storage is the `ProfileRepository` trait.
- **`jobhunt-resume`** reads files and parses them into
  `jobhunt_profile::ParsedResume`, the contract any resume parser fills in.
  It only structures the document and keeps its words; deciding what those
  words claim, and how far to trust them, is the domain's job.
- **`jobhunt-storage`** implements `ProfileRepository` on the same SQLite
  file as the jobs, in its own tables.
- A LinkedIn data export is read by `jobhunt_resume::linkedin` into the
  same `ParsedResume` contract; a public GitHub account by
  `jobhunt_sources::github` into `jobhunt_profile::GithubSnapshot`. The
  domain folds both in with the resume's rules
  ([docs/evidence-imports.md](docs/evidence-imports.md)).

### Reading resumes

PDFs are read locally with `pdf-extract` (pure Rust): no browser, no OCR, no
LLM. JobHunt lays the text out itself from glyph positions: lines by
baseline in content-stream order, spaces from real gaps (so kerning does not
split words, whether the PDF writes space characters or, like TeX, places
every word separately), wide gaps as column breaks (right-aligned dates),
paragraph breaks from vertical gaps, ligatures normalized, and lines that
repeat at the top or bottom of pages (running headers, "Page 2 of 3")
removed. The PDF reader can panic on damaged files; it runs on its own
thread, so that becomes an error. Empty or image-only PDFs, damaged files,
password-protected files and unsupported formats (`.docx`, …) are refused
with a message saying what to do instead.

The deterministic parser (`jobhunt_resume::DeterministicParser`) finds
sections by their headings (English and Portuguese), the name, headline,
contacts and location in the header, and entries by their header lines
(dates, column gaps, separators such as `—`, `|`, " at "). A company line
followed by several titled roles is one company with several positions.
Titles and companies are told apart by title words; when neither side has
one the entry is flagged ambiguous rather than guessed. Bullets are list
items when the document marks them, otherwise sentences rebuilt from
wrapped lines. Missing dates stay missing, with a note; lines it does not
understand are reported, not dropped.

`ResumeParser` (resume structure) and `StatementParser` (preference
statements) are traits. An AI-assisted parser can be plugged in later
behind them; nothing in the profile depends on one, and `narrow init`
never needs an API key.

### The evidence graph

Every professional statement is a **claim** (`clm_…`) about the profile or
one experience, project or education entry:

| Kind | Example | Provenance |
| --- | --- | --- |
| employment, education, project | "Senior Software Engineer at Ledgerly (Mar 2022 – Present)" | extracted |
| accomplishment, responsibility | each resume bullet, verbatim | extracted |
| technology | "Used Kafka at Ledgerly" (a "Tech:" line or a bullet naming it) | extracted |
| skill | "Lists Go as a skill (Languages)" | extracted |
| domain | "Worked in payments at Ledgerly" | inferred |
| role | "Backend engineering experience at Ledgerly" | inferred |
| ownership | "Staff-level role at …", "Mentored or hired engineers at …" | inferred |
| other | certifications, awards, anything you add | extracted / user_entered |

Each claim records its **source** (the imported document and the resume's
own words, never a paraphrase), **provenance** (`extracted`, `inferred`,
`user_entered`), **confidence** (`high`/`medium`/`low`, with the **basis**
of an inference: "mentions “payment providers”, “PIX”"), and your
**verification** (`unverified`, `confirmed`, `rejected`).

The evidence policy (`Claim::standing`) decides what may later be used on
your behalf (application answers, tailoring):

1. rejected claims, and claims about records you rejected, are never used;
2. a claim whose source left the resume needs review, even if confirmed,
   until you confirm it again;
3. confirmed and user-entered claims are usable;
4. extracted claims are usable when quoted from the resume with high
   confidence ("directly supported");
5. everything else, including every inference, needs review.

Inference is never promoted to truth on its own. Skills are records with
their evidence: *used in your work* (a technology claim in an experience or
project), *added by you*, or *listed only*, plus when they were last used;
there are no proficiency scores. Domains and role signals (backend,
platform, full stack, …; senior, staff, technical leadership, mentorship,
…) are inferred claims, each with the evidence it rests on.

### Preferences

Preferences start with one question: what kind of job are you looking
for? `narrow preferences describe "…"` (or the web's Preferences and
onboarding) keeps your words verbatim and reads them into a short **taste
profile**: the level, kind of engineering work, specialization, ownership,
company, team and culture you want, and what you avoid, each statement
saying where it comes from (your words, your profile, earlier settings,
feedback). You confirm it or correct it, and your corrections always win.
Practical constraints (work setup, where you live and may work,
relocation, time zones, a pay floor) are kept apart, deterministic and
explicit. By default Narrow reads your words with built-in rules, offline;
an optional model (`[ai]` in the config: the Anthropic API or any
OpenAI-compatible server, Ollama included) reads more. See
[docs/taste-profile.md](docs/taste-profile.md) for the model, what is sent
to a model and when, and how it will feed ranking (BRU-322; ranking doesn't
read it yet).

Under the taste profile, preferences are structured values with a stance (`required`, `wanted`,
`acceptable`, `unwanted`): roles; compensation (minimum and target, amount,
ISO currency — never assumed from your country — period, employment or
contract); location (where you live, remote/hybrid/on-site, regions, time
zones, relocation, visa sponsorship); company and team kinds (startup,
early-stage, founder-led, product company, agency, consulting, small team,
…); domains you like or avoid; and work style (ownership, IC vs
management, greenfield vs maintenance, async, meetings, closeness to
product, on-call).

`narrow preferences add "I want small product teams and at least $120k.
Avoid pure SRE roles."` stores the statement verbatim, then reads it with
deterministic rules: clauses, their polarity ("avoid", "no", "open to",
"at least", …), and known values. Each preference read from it links back
to the statement and the clause it came from. Hedged or cue-less readings
are marked uncertain, and parts that could not be read are kept and shown.

Currencies are never assumed. A code or a symbol only one currency uses
(`USD 120k`, `$120k USD`, `US$`, `CA$`, `R$`, `€`, `£`) settles it. A bare
`$` (or `¥`) does not: USD, CAD, AUD, NZD, SGD, MXN and others all write
`$`. If the rest of the statement points to exactly one of them ("I live
in Toronto. At least $150k." → CAD), that reading is kept but marked
uncertain, with a note naming the words it rests on; otherwise the
currency stays unknown, the note says so, and `narrow profile` lists it
until you set it (`narrow preferences set compensation --minimum 120k
--currency USD`). Compensation will be a hard constraint later, so a
guessed currency could wrongly exclude or favor jobs.
A newer preference with the same key (say, a new minimum salary) replaces
the older one, which is kept as history. `narrow check` and `find` read
the location, work-mode, time-zone, relocation, sponsorship,
authorization and engagement preferences (see
[Verification and eligibility](#verification-and-eligibility)); `narrow
rank` reads the rest (pay, roles, companies, domains, work style; see
[Ranking, feedback and taste](#ranking-feedback-and-taste)).

The web app's Preferences page shows the same records as structured
settings, each in one of three layers: **requirements** (a posting that
states the opposite is left out as a stated conflict; one that doesn't say
is unresolved, never a strong fit, and never counted as meeting it),
**preferences** (they only change the order), and **learned** taste (from
decisions, ranking only). A statement fills the settings in; a setting
changed directly replaces what the statement set, and the statement stays
as written. Ambiguous words are asked about, never guessed: a missing
currency, a floor or a target, the team or the company, a must or a
nice-to-have.

| Setting | CLI | Stored as |
| --- | --- | --- |
| Work setup | `preferences set work-setup remote-only\|prefer-remote\|hybrid-okay\|onsite-okay\|no-preference` | remote required; remote wanted (ranking only); remote or hybrid required; on-site acceptable; nothing. Replaces every work-mode preference |
| Relocation | `preferences set relocation yes\|no [--only-to PLACE …]` | separate from the work setup; an office outside the named places is a conflict |
| Remote roles open to | `preferences set region Worldwide\|Americas\|"Latin America"\|<place> --stance require\|want` | a required geography the published remote scope is entirely outside of is a stated conflict; "Remote" with no scope is unresolved |
| Unclear eligibility | `preferences set unclear-eligibility show\|hide` | `hide` leaves out jobs whose eligibility isn't confirmed |
| Unknown pay | `preferences set unknown-pay show\|hide` | `hide` leaves out jobs without comparable published pay; either way unknown pay never meets a minimum |

"Remote from Brazil" reads as living in Brazil and requiring remote work.
A job ruled out only by the work setup or relocation is counted as a
stated conflict (`not_shown.unmet_requirement`), not as "can't take it".

### Re-importing a resume

Run `narrow init` again after changing your resume. It never deletes and
re-inserts:

- **Identity.** Experiences are matched by company and title (and, when the
  title changed, by company and start date, so a corrected title updates
  the record), projects by name, education by institution, skills by
  normalized name, and claims by their subject plus what they say. The same
  file imported twice changes nothing and duplicates nothing.
- **Source facts update**, except fields you edited by hand, which always
  win.
- **Decisions survive.** Confirmed claims stay confirmed; rejected claims
  stay rejected and never come back as trusted. If the statement of a
  one-per-record claim changes (a title or dates), your confirmation of the
  old statement does not carry over.
- **Nothing is deleted.** Records and claims the new resume no longer
  contains are marked stale; stale claims need review before they are used
  again. A reworded bullet is a new claim that points to the one it
  replaces. What you entered yourself (records, claims, preferences) is
  never touched by an import.

`init` prints what changed: new, updated, unchanged and stale counts, the
decisions and edits it kept, and anything you need to confirm again.

### LinkedIn and GitHub evidence

Two optional sources feed the **same** profile and evidence graph as the
resume. Imported data is evidence, not truth: facts keep their source's
words, conclusions wait for your review, and your decisions win.

```text
$ narrow profile import-linkedin ~/Downloads/Basic_LinkedInDataExport_09-01-2026.zip
Imported LinkedIn export Basic_LinkedInDataExport_09-01-2026.zip
Read: profile (1 row), positions (4 rows), education (1 row), skills (18 rows)
Not used: 38 other files in the export, never opened (messages, connections, contacts, …)

Your profile
  experiences: 1 new, 3 already in your profile, now also backed by this source
  skills: 6 new
  claims: 21 new, 14 already in your profile, now also backed by this source

5 claims need your review (not used until you confirm): narrow claims review
```

- **LinkedIn** is read from the file you download yourself (*Settings →
  Data privacy → Get a copy of your data*): the `.zip`, its folder, or one
  of its CSVs. No scraping, no login, no LinkedIn API. Only the career
  files are opened (profile headline/summary/location/websites, positions,
  education, skills, certifications, projects, languages); messages,
  connections, contact details, ads and searches never are. A file Narrow
  cannot read truthfully fails the import and changes nothing.
- **GitHub** is read through GitHub's official REST API, public data only.
  The repositories you own become projects (forks, empty repositories and
  others' repositories are skipped); their main languages are facts about
  that code; "recent hands-on Rust work" is an inference for you to
  confirm. Stars are not quality, organizations are not employers, and a
  language share is not a skill level. `GITHUB_TOKEN` (no scopes) is
  optional: it raises the rate limit and adds per-repository language
  statistics, and is never stored.
- **One graph.** A position both your resume and LinkedIn list is one
  experience and one claim with two sources (`narrow claims show <id>`
  prints both). Matching is conservative (same company and title,
  overlapping dates); ambiguous matches stay separate, and a source that
  dates a shared position differently gets its own claim to review.
- **Re-import** any time: nothing is duplicated, decisions and edits are
  kept, and what a source dropped becomes stale only when no other source
  still supports it.

The rules, the storage and the limits are in
[docs/evidence-imports.md](docs/evidence-imports.md).

### Export format

`narrow profile export` writes the profile alone (for everything that is
yours, feedback and pipeline included, use `narrow export`; see
[Your data: export and import](#your-data-export-and-import)). It is one
JSON document:

```json
{
  "format": "jobhunt.profile",
  "version": 1,
  "exported_at": "2026-09-25T12:00:00Z",
  "generator": "narrow 0.1.0",
  "profile": { "id": "prof_…", "name": "…", "revision": 7, … },
  "documents": [ { "id": "doc_…", "sha256": "…", "text": "…", … } ],
  "experiences": [ { "id": "exp_…", "company": "…", "meta": { "origin": "resume", "verification": "unverified", "edited_fields": [], … } } ],
  "projects": [ … ], "education": [ … ], "skills": [ … ],
  "claims": [ { "id": "clm_…", "kind": "accomplishment", "subject": { "type": "experience", "id": "exp_…" }, "provenance": "extracted", "verification": "confirmed", "source": { "document": "doc_…", "snippet": "…" }, … } ],
  "preferences": [ { "id": "pref_…", "value": { "type": "compensation", "bound": "minimum", "amount": 120000, "currency": "USD", "period": "year" }, "stance": "required", … } ],
  "statements": [ { "id": "stmt_…", "text": "I want small product teams …", "reading": "understood", … } ]
}
```

The file contains your resume's text and contact details. `narrow profile
import` checks the format name and version first, parses strictly
(unknown fields are errors), validates every reference (claim subjects,
sources, superseded claims, project experiences, preference statements),
and only then replaces the stored profile in one transaction; an invalid
file changes nothing. Replacing an existing profile needs `--replace`.

### Storage

The migration `20260927000000_career_profile.sql` adds, without touching
the jobs tables: `profiles`, `profile_documents` (with the extracted text),
`profile_experiences`, `profile_projects`, `profile_education`,
`profile_skills`, `profile_claims`, `profile_skill_evidence` (which claims
back which skill), `profile_preference_statements`, `profile_preferences`
and `profile_events` (history); `20260928000000_preference_notes.sql` adds
the note explaining how an ambiguous preference was read. Every table is keyed by `profile_id`; a
local install has one profile, but nothing prevents more. Records are
rows, with JSON only for small values read whole (contact lists, a
preference's typed value, lists of edited fields). Saving writes the whole
profile in one transaction (upsert by id, delete what is gone, rebuild the
derived skill evidence) and checks a revision number, so two concurrent
writers cannot silently overwrite each other. The rules live in the
domain, so a Postgres backend would only persist them.

## Verification and eligibility

Two separate questions, answered separately:

- **Verification**: is this job still open at a source that speaks for the
  employer, can it be applied to, and what does that source publish right
  now?
- **Eligibility**: can *you* work it, from where you are, on the terms the
  posting states?

A job can be verified active and ineligible ("US residents only"),
verified active and uncertain ("Remote", no scope), or eligible on paper
but not verified (and then not recommended). The two answers are never
collapsed into one state, and neither is a percentage.

```text
$ narrow verify job_02e51190085f8a9a0772e845ddd9f329
Verifying 1 source record at 1 source…
Senior / Staff Fullstack Engineer — Linear
job_02e51190085f8a9a0772e845ddd9f329 · opp_02e51190085f8a9a0772e845ddd9f329

Verification
  ✓ First-party Ashby listing active
  ✓ Application path active (published by the board's API)
  Verified 12 seconds ago

Location
  Remote: Europe
  Limited to: North America, Europe
  You: Berlin, Germany

Employment
  Employment: full time
  Contractor / EOR: not stated
  Visa sponsorship: not stated
  Time zone: flexible hours

Compensation
  Not published
  Verified from the first-party listing · first verification

Eligibility
  ELIGIBLE

Why
  ✓ Germany is within the listed Europe region
  ✓ Germany is within Europe, which the description allows
  • The posting says working hours are flexible

Conflicting information
  ! The location fields say remote in Europe, but the description says North America, Europe
```

For someone in Toronto the same job is `UNCLEAR`: the listing says Europe,
the description includes North America, and JobHunt can't tell which one
applies to this role, so it says so and shows both statements. For
someone in São Paulo it is `INELIGIBLE`: both exclude Brazil.

This is a compatibility signal computed from what the posting publishes
and what you told JobHunt. It is not legal advice and does not determine
work authorization.

### Commands

| Command | Does |
| --- | --- |
| `narrow verify <job_…\|opp_…>` | Verify every source record of the job now (reusing an attempt from the last 15 minutes), save the result, and check it against your profile |
| `narrow verify --force <id>` | Ask the sources even if a recent attempt exists |
| `narrow verify --details <id>` | Plus provenance: every record, the method and URLs checked, the authority chain, what changed, and the posting's words and your profile facts under every reason |
| `narrow check <id>` | The detailed report from what is stored, without fetching (`--refresh` verifies first) |
| `narrow show <id>` | Everything stored, with the last verification and the eligibility verdict (no fetch) |
| `narrow find --eligible` / `--possible` | The inventory (`--raw`) filtered to eligible or conditional jobs / everything not ruled out. The shortlist (`find`) never recommends ineligible jobs |
| `narrow preferences set authorized-in <country>` | A country (or "the EU") you may already work in |
| `narrow preferences set engagement contractor\|employee --stance require\|want\|accept\|avoid` | How you can be hired |

### What "verified" means

A verification asks the job's authoritative source directly, through the
narrowest public endpoint each family has, using the same HTTP client
(timeouts, bounded retries honoring `Retry-After`, at most 4 requests per
host) and the same conversion code as discovery:

| Family | Listing | Application path |
| --- | --- | --- |
| Greenhouse | `boards-api.greenhouse.io/v1/boards/<board>/jobs/<id>`; 404 is closed | the board's hosted job page (it holds the form); a redirect to the board with `error=true` is closed |
| Lever | `api.lever.co/v0/postings/<site>/<id>` (or `api.eu.lever.co`); 404 is closed | the posting's `/apply` page |
| Ashby | the board (Ashby has no single-job endpoint; one request, the whole board); a job missing from it is closed | the `applyUrl` the board API publishes (the page renders in the browser, so it is not requested) |
| YC / Work at a Startup | the job's page (its embedded page data); 404, or the company's job list instead, is closed | the Work at a Startup application URL the page names |

Verifying one job never re-reads more than that job's own endpoint (or one
Ashby board). Records of one opportunity are verified concurrently
(bounded, `[verification] concurrency`).

Each attempt is stored as a **verification record**
(`jobhunt_jobs::verification::VerificationRecord`), never updated or
deleted:

| Field | Holds |
| --- | --- |
| `listing` | `active`, `closed` (the source said so), `unreachable` (timeout, 5xx, connection), `ambiguous` (an unreadable answer, or a different record than the job), `unknown` (no verifier for the source) |
| `application` | `active`, `closed`, `unavailable`, `unknown`, and *how* it is known: `probed` (requested, with the HTTP status), `published` (the source publishes the route), `listing_page`, `not_checked` |
| `authority`, `authority_chain` | see below; each link records whether JobHunt requested it or the source only named it |
| `checked_url`, `listing_url`, `method` | what was asked, and how |
| `changed_fields`, `changed_since_last_verification`, `lifecycle` | what differs from the stored record and from the previous successful verification, and the lifecycle outcome |
| `compensation` | see [Compensation](#compensation-verification) |
| `published` | the location, workplace, remote, employment and work-authorization text the source publishes now, verbatim |
| `unknowns`, `failure` | what could not be established; a typed failure (`timeout`, `unavailable`, `malformed`, `request`, `mismatch`, `not_supported`) with the URL and HTTP status |
| `revision` | the verification rules revision (`VERIFICATION_REVISION`) |

An attempt **succeeds** when the source gives a definitive answer (active
or closed). The latest attempt and the latest successful verification are
kept apart, so a failure never erases an earlier success: "Last attempt 2
minutes ago failed; last successfully verified 3 days ago".

**Lifecycle.** A live posting is fed back through the existing discovery
lifecycle exactly as a scan listing only that job would be: an edited job
is UPDATED (with the changed fields and the replaced version in its
history), a closed job that is live again is REOPENED with its history
kept, and `last_seen_at` moves. No scan is recorded and nothing else is
closed. A job verified closed is not closed in the jobs table (closing is
discovery's decision, with its safeguards); the trust view shows it as
closed. A job discovery closed after its last successful verification is
never shown as verified active.

**Freshness** (`[verification]` in the config, one policy in
`FreshnessPolicy`): an attempt within `reuse_minutes` (15) is reused
instead of asking again (`--force` always asks), a success within
`fresh_hours` (24) is fresh, and older than `stale_hours` (72) it is stale
and the job is not trusted enough to recommend until verified again.
`show`, `check` and `find` only read what is stored.

### Authority

Who stands behind a listing, strongest first:

| Level | Means | Sources |
| --- | --- | --- |
| `employer_first_party` | a page on the employer's own domain, checked by JobHunt, that publishes the job or embeds its board | (no verifier checks employer pages yet) |
| `employer_configured_ats` | the employer's own applicant-tracking board, which it configures and publishes | Ashby, Greenhouse, Lever |
| `trusted_source` | a platform the employer posts to itself, but not its own system | Y Combinator's Work at a Startup |
| `secondary_source` | a reposting by someone else | none today |
| `unknown` | a source JobHunt doesn't know | |

A Greenhouse board that publishes the employer's own page as the job's URL
(`stripe.com/jobs/search?gh_jid=…`) adds that page to the chain, marked as
named by the board, not checked; the authority stays the board's. Work at
a Startup is never called first-party.

**Opportunities.** Every source record of an opportunity is verified and
kept inspectable. The opportunity's status rests on the strongest live
record (authority, then the most recent success); records that disagree
(one live, one gone) are listed as conflicts. It is **trusted enough to
recommend** only when that record is verified active, not stale, at a
source of at least `trusted_source` authority.

### Location normalization

Places are normalized from the job's structured location fields (and the
source's structured country), sentences of its description, and your
`current_location` preference (or, failing that, your resume's header,
which is then named in every reason that uses it). A place is never
guessed from the machine's locale or IP.

A normalized place is an area of
[`geo`](crates/jobhunt-eligibility/src/geo.rs): anywhere (a scope, not a
place), a business region, a country (ISO 3166-1 alpha-2 codes
internally), a first-level subdivision (state, province, region) or a
city, each with its IANA time zones. Two sources:

- **Narrow's own tables**: the regions and their membership (below), the
  countries postings name with their aliases and demonyms ("USA",
  "British"), state and province codes ("CA", "ON", "NSW", "RS", "MS"
  after a Brazilian city) and the nicknames of tech hubs ("NYC", "Bay
  Area"). Sentences of a description are only ever read against these, so
  an ordinary word that happens to be a town somewhere is not taken for a
  place.
- **A GeoNames subset** compiled into the binary
  ([`gazetteer`](crates/jobhunt-eligibility/src/gazetteer.rs), data under
  [`crates/jobhunt-eligibility/data/geonames`](crates/jobhunt-eligibility/data/geonames/README.md)):
  every country, every first-level region and every place of more than
  15,000 people (or a capital), with names in English and in the country's
  own languages, and each place's IANA zone ("Dourados" is
  `America/Campo_Grande`, UTC-4, not Brasília time). Location fields and
  the places you state are resolved against it too; diacritics don't
  matter ("Sao Paulo", "São Paulo", "SÃO PAULO"). Nothing is looked up
  over the network: the subset is read once per process, into hash
  indexes.

**Ambiguous names stay ambiguous.** A name resolves to one place only when
every other place it could mean has less than a tenth of that one's
population, or lies inside it (the city of New York, not the state; the
country of Singapore, not the city). So "London" is London, England, but
"Cambridge" (England, Ontario, Massachusetts, New Zealand), "Santiago"
(Chile or the Dominican Republic), "San José" and "Georgia" (the country
or the US state) are unresolved until something around them chooses: a
qualifier ("Cambridge, MA", "London, Ontario", "Atlanta, Georgia"), or the
source's structured country field. An unresolved job location is kept as
written (an office there is uncertain); an unresolved home location is
reported with what it could be ("Your location “Cambridge” could be …; say
which"), and counts only for what the readings share (both Portlands are
in the United States). A stated relocation destination, authorization or
remote-geography preference that is ambiguous is not recognized at all:
one reading never stands for another.

The raw text is always kept next to the normalized place, and text neither
source knows stays unrecognized (`Narrow doesn't recognize your location
“…”`), never approximated. ISO codes are used as identifiers, not as
political statements.

### Region definitions

Membership lives in one tested table (`Region::members`). "Maybe" means
usage disagrees; a decision built on it is uncertain, never eligible or
ineligible. A country the table doesn't list (Andorra, Kazakhstan) is
placed by its GeoNames continent: certainly in the continent's own region
(Europe, Africa, Asia, Oceania, the Americas, South America), maybe in a
business region on it (the EU, North or Latin America, EMEA, APAC, the
Middle East), and never in a formal list (the EEA, DACH).

| Region | Includes | Maybe |
| --- | --- | --- |
| North America | US, Canada | Mexico |
| Central America | Belize, Costa Rica, El Salvador, Guatemala, Honduras, Nicaragua, Panama | Mexico |
| Caribbean | Cuba, Dominican Republic, Jamaica, Trinidad and Tobago, Puerto Rico | |
| South America | Argentina, Bolivia, Brazil, Chile, Colombia, Ecuador, Guyana, Paraguay, Peru, Suriname, Uruguay, Venezuela | |
| Latin America (LATAM) | Mexico, Central and South America, the Spanish-speaking Caribbean | Belize, Guyana, Suriname, Jamaica, Trinidad and Tobago |
| Americas | all of the above | |
| Europe | the EU, the EEA, the UK, Switzerland, the Western Balkans, Ukraine, Moldova | Turkey, Russia, Belarus, Georgia, Armenia, Azerbaijan |
| EU | the 27 member states | other European countries (postings often write "EU" for Europe) |
| EEA | the EU plus Iceland, Liechtenstein, Norway | (formal: nothing else) |
| Nordics / DACH | Sweden, Norway, Denmark, Finland, Iceland / Germany, Austria, Switzerland | Faroe Islands, Åland, Greenland, Svalbard / Liechtenstein |
| Middle East | UAE, Saudi Arabia, Israel, Qatar, Kuwait, Bahrain, Oman, Jordan, Lebanon, Syria, Iran, Iraq, Yemen, Palestine | Turkey, Egypt |
| Africa | the African countries in the table | |
| EMEA | Europe (including its maybes), the Middle East (including its maybes), Africa | |
| Asia | East, Southeast and South Asia | the Middle East, Turkey, Georgia, Armenia, Azerbaijan |
| APAC | East and Southeast Asia, Oceania, India | Pakistan, Bangladesh, Sri Lanka, Nepal |
| Oceania | Australia, New Zealand | |
| Global / anywhere | every country | |

### Remote scope

Remote availability is separate from workplace type. A job offers one or
more **work options**:

- **remote**, with a scope: explicitly global ("Remote - Worldwide",
  "Anywhere"), a list of areas, or **unknown** ("Remote" alone). Areas are
  *stated* or *listed*:
  - **stated**: "Remote (US)", "Remote - LATAM", "Remote: Brazil,
    Argentina, Chile", and the same in a list of offices ("US-Remote,
    Chicago, Seattle, San Francisco"), or a bare country beside a bare
    "Remote" (Stripe's "Remote" with the location "US");
  - **listed**: a remote job whose location fields list only places, with
    nothing unscoped beside them, is remote in their countries (remote,
    with Seattle, Austin and San Francisco: the United States; with
    London and Manchester: the United Kingdom). A finite list is a scope.
    An explicit statement in the description outranks it.

  A city listed beside an unscoped "Remote" ("Remote, San Francisco, CA")
  or a place Narrow can't read only *suggests* its country: that is never
  enough for a definite answer. A country attribute on a "Remote" entry
  (PostHog's "Remote"/"USA") is the source's default, not a scope;
- **office** (on-site, hybrid, or office-based when the source doesn't
  say how often), at a place;
- **engagement**: a remote path through a named mechanism in named places
  ("we hire contractors in Brazil, Argentina and Mexico through Deel").

"Remote" is never "anywhere", "Remote — US" is not "Remote", and
"Americas" is not "Global". An explicit hybrid or on-site workplace type
outranks a remote flag (Ashby sets `isRemote` on some hybrid jobs; the
disagreement is recorded).

### Restrictions

The description is read with fixed English cues, sentence by sentence,
keeping each sentence as evidence:

- **allowed places** ("must be based in Canada", "US only", "open to
  candidates in Brazil", "we're hiring in LATAM", "Remote in the US",
  "applicants must live in NYC"), each *required* or only *preferred*
  ("candidates in Europe are preferred" is not a rule);
- **excluded places** ("we are unable to hire in Cuba, Iran, North Korea
  or Syria");
- **anywhere** ("work from anywhere", "anywhere in the world"), but not
  descriptions of the team ("a globally distributed team" is marketing);
- **scope labels**: a "Location:", "Countries:" or "Region:" line is read
  as where this role is open ("Location: Americas - North, Central and
  South America, EMEA, APAC"; "Countries: Brazil, Canada, Colombia, …"),
  clause by clause when it has several ("San Francisco (strongly
  preferred); remote (US) considered"). "Location: Fully remote" makes the
  job remote even when the fields list only places;
- **offices** ("hybrid in London", "onsite in New York", "based in our
  Toronto office");
- **work authorization** ("must be authorized to work in the United
  States"), and Work at a Startup's first-party visa field ("US
  citizen/visa only");
- **sponsorship** (offered; offered "but not for every role";
  unavailable), **relocation** (help offered, or required);
- **engagement** (contractors, B2B, employer of record such as Deel or
  Oyster, where; "we don't work with contractors"). An employer of
  record's own postings name it everywhere; there the name is the
  company, not a way the job is offered;
- **time zones** (below).

Each place statement records whom it is about: **this role** ("this role
requires you to be based within EMEA", "This is a remote position
available anywhere in the world", a scope label) or **hiring in
general** ("we are open to candidates across the Americas", "Work from
anywhere: we have no HQ").

**Hiring scope is not pay scope.** A sentence about pay, salary,
compensation, a base or annual range, or benefits says what is paid where,
not where people may be: "The anticipated annual pay range … for
applicants based within the United States is …" limits nothing. Neither
do terms for some of the people hired ("For US-based applicants: this
position is part of a bargaining unit"). Country names elsewhere in the
description are still read.

### Time zones

A time-zone requirement is kept separate from geography, with its kind:
**within** a range ("You must be located between UTC-5 and UTC+1", "based
in a European time zone"), **hours** of a zone with an optional tolerance
("EST ±3 hours", "Pacific time"), or **overlap** of so many hours
("4 hours overlap with EST", counted against an 8-hour day). Your zones
are the ones you stated ("UTC-3", "US hours", "America/Sao_Paulo"), else
your city's IANA zone, else your state's or country's zones. Where you
live gives your time zone only: it is never read as work authorization,
and a time zone that fits never makes a remote scope that doesn't include
you eligible (or the other way round); each is its own reason.

**Daylight saving time is applied, not averaged away.** Zones are IANA
zones (from the IANA database compiled into `chrono-tz`; the version is
`chrono_tz::IANA_TZDB_VERSION`), and named zones mean what postings mean
by them: "EST" and "Pacific time" are US Eastern and Pacific time,
daylight saving included; "UTC-3" is exactly UTC-3. Every requirement is
judged on each day of a **reference year** (currently 2026,
`zones::REFERENCE_YEAR`, each day at 12:00 UTC):

- if the answer is the same every day, that is the answer;
- if daylight saving time changes it (São Paulo is 2 hours from New York
  from November to March and 1 hour the rest of the year; London and New
  York are 4 hours apart for the weeks only one has moved its clocks),
  the requirement is **uncertain**, and the reason says when it fits and
  when it doesn't ("1h away Mar 8–Oct 31 and 2h away Nov 1–Mar 7").

A fixed reference year keeps decisions deterministic and cacheable; the
IANA database version and the reference year are part of every stored
decision's cache key, so updating either re-evaluates. `Zone::at` and
`Clock::offset_at` answer for a specific instant, for anything that needs
one date.

- Within: inside (every day of the year) passes; outside fails for a
  requirement; partly inside (a country spanning zones) is uncertain.
- Hours: within the tolerance (the stated one, else 3 hours) passes;
  outside a stated tolerance, or with no working-day overlap at all,
  fails; otherwise uncertain ("5h from your time zone; the posting doesn't
  say how much shift is acceptable").
- No time-zone language is "no requirement published", not "unrestricted";
  vague language ("some overlap with the team") is uncertain; "flexible
  hours" says hours are flexible. Nothing is inferred from where the
  company's headquarters are.

### Visa, work authorization, contractors and EOR

- **Authorization** is taken only from what you stated: `authorized-in`
  preferences, and "no sponsorship needed", which is read as authorized
  *where you live* (and nowhere else). Living somewhere is not assumed to
  mean you may work there.
- An explicit requirement ("authorized to work in the US", the visa field,
  or an office abroad) passes when you hold it; with sponsorship needed it
  is conditional if the posting offers sponsorship, uncertain if it offers
  it "not for every role" or doesn't say, and ineligible if it doesn't
  sponsor; otherwise it is uncertain and says how to settle it.
- **"No visa sponsorship" is not "no international applicants."** For a
  remote option with no authorization requirement it restricts nothing and
  is reported as such ("doesn't restrict remote work from Brazil by
  itself").
- **"US only, no sponsorship"** is ineligible from Brazil because of "US
  only", not because of "no sponsorship", and it says nothing about a
  contractor path. A contractor or EOR path exists only where the posting
  names one (then it is its own work option), or for "remote worldwide,
  contractor", which is strong evidence. When the scope is unknown, the
  missing contractor information is named as a reason.
- Your `engagement` preferences rule paths out (contractor `avoid`) or
  require them (contractor `require`: a job that doesn't say whether it
  hires contractors is uncertain).

### Eligibility decisions and rules

Each work option goes through these rules, in order (each a function in
[`rules`](crates/jobhunt-eligibility/src/rules.rs)):

1. **Listing** (a gate on top of the decision): verified active,
   recently, at an authoritative source; otherwise "not trusted enough to
   recommend".
2. **Work mode and presence**: a work mode you require; being at an
   office (same city passes; elsewhere in your country is uncertain, or
   conditional if you'd relocate; abroad is ineligible if you won't
   relocate, conditional if you would, uncertain if your profile doesn't
   say).
3. **Countries** the job allows or rules out (including cities).
4. **Regions** and remote scope.
5. **Work authorization and sponsorship.**
6. **Contractor, B2B and EOR** engagement.
7. **Time zones.**
8. **Ambiguity**: what could not be read.

An option is **ineligible** if any rule fails, else **uncertain** if any
rule can't tell, else **conditional** if it holds only on a condition you
haven't ruled out (you relocate; the company grants the sponsorship it
offers), else **eligible**. The job's decision is its best option's; the
other options are listed with theirs. Every reason records the rule, its
conclusion, the posting's words (source and field) and the profile fact it
used, so "why am I eligible?" is answered from the stored decision itself.

**Unknowns stay unknown**: no location in your profile, an unrecognized
place, a remote scope that isn't published, a membership usage disagrees
on, an authorization your profile doesn't state: each is uncertain, with
the reason and, where there is one, the command that settles it.

### Hiring-scope precedence and conflicting evidence

Where a remote job can be done comes from, strongest first:

1. an explicit hiring statement in the description (limits, "anywhere",
   scope labels);
2. a stated scope in the location fields (country, region, "anywhere");
3. a finite list of places in the location fields;
4. the workplace type or a remote flag alone: scope unknown;
5. nothing: unknown.

Pay and terms-for-some-hires sentences are never hiring evidence. When
statements disagree, both are kept and the disagreement is recorded:

- The description is narrower than the fields (listed "Remote -
  Worldwide", description "US only"): the narrower, explicit restriction
  applies, whoever it is about, and the conflict is shown.
- The description is broader than a stated scope **and is about this
  role** (listed "NAMER; APAC; EMEA", "Location: Americas - North, Central
  and South America, EMEA, APAC"; listed "Remote (United States)", "This
  is a remote position available anywhere in the world"): the
  description's statement applies, and the resolution is shown.
- The description is broader **about hiring in general** (listed "Remote
  (US)", "we are open to candidates across the Americas"): for someone the
  fields exclude but the description includes, it is uncertain, with both
  statements: the description may describe the company, not the role.
- A listed or suggested scope gives way to any explicit statement of the
  description.
- "Remote", but "applicants must live in NYC": the city requirement
  applies and the conflict is shown.
- Several source records of one opportunity: the live records decide; a
  record that says nothing defers to one that answers; records that
  contradict each other (one eligible, one ineligible) make the answer
  uncertain, with each record's answer kept.

### Compensation verification

Compensation is verified as facts, not scored: whether it is published,
each range with its source label ("Canada Annual Pay Range"), minimum and
maximum where given, pay period, and its **currency evidence**: an ISO code
the source gives, or an ambiguous symbol (`$`, `¥`) kept as such with the
currency unknown. A bare `$` is never read as USD, whatever country the
job is in. Each verification compares with the previous successful one:
first verification, unchanged, changed (with the previous version),
newly published, removed. Currencies are not converted. Whether pay meets
your minimum is a ranking question and is not part of eligibility.

### Caching and revisions

Eligibility decisions are stored in `eligibility_decisions` under a key
that is a digest of everything they depend on: the profile id and
revision, every source record's material content and lifecycle status, the
verification each record rests on, and the rules revision
(`RULES_VERSION`, bumped with any rule or normalization change). A changed
preference, an UPDATED job, a new verification or a rule change is a new
key, so a stored decision can never outlive its inputs; older rows remain
as an audit trail. Trust is always recomputed, since it depends on the
clock.

### Browser automation

Not used. Every supported family publishes what verification needs as
plain HTTP responses (JSON APIs, or server-rendered page data), so no
browser runs anywhere. A browser-backed verifier would be warranted for an
important first-party page that shows its listing only after JavaScript
runs (Ashby's hosted application form is one, but its API publishes the
same route, so it is not needed). It would be another implementation of
the `ListingVerifier` trait, isolated from the domain.

### Architecture

```text
jobhunt-cli ──► jobhunt-sources::verify (HttpVerifier) ──► jobhunt-jobs::verification (ListingVerifier)
     │                                                         ▲
     ├──► jobhunt-eligibility (rules, decisions, cache) ───────┤ ──► jobhunt-profile
     └──► jobhunt-storage (VerificationRepository, EligibilityRepository)
```

- `jobhunt-jobs::verification` is the verification domain: records,
  states, authority, freshness, trust, compensation facts, the
  `ListingVerifier` fetch contract, `VerificationRepository` and
  `VerificationService`. No HTTP or SQL.
- `jobhunt-sources::verify` implements the contract over HTTP.
- `jobhunt-eligibility` reads jobs and the profile domain (never profile
  tables), and owns normalization, extraction, rules, decisions and the
  `EligibilityRepository` boundary. No HTTP or SQL.
- `jobhunt-storage` implements both repositories on SQLite.

## Ranking, feedback and taste

Eligibility answers *can this person plausibly work this job?* Ranking
answers *would they want it?* They are kept apart: ranking sits on top of
the eligibility decision and never feeds pay, roles, seniority, domains or
companies back into it.

A resume says what someone can do. JobHunt learns what they actually want
from three things, kept separate and always inspectable:

- **what you told it**: preferences you set or stated (roles, pay,
  company and team kinds, domains, work style, work modes). These always
  win;
- **what you did**: feedback on jobs (save, reject, applied, interview,
  offer, like, dislike), with your reasons in your own words;
- **what it inferred**: patterns across that feedback, each with the
  evidence behind it.

`narrow find` (see [The loop](#the-loop)) is where this shows: the
rejected and applied opportunities have left the list, learned patterns
appear, attributed, among the reasons, and what you said always outranks
them:

```text
$ narrow reject opp_011ee5ec --reason "customer-facing, too corporate"
Rejected Forward Deployed Engineer - ML — Modal
opp_011ee5ec62edd51f11dc0fd4b3edd532 · fb_a2b6d47142c401ba85154fade545fd65
Reason: “customer-facing, too corporate”
Read as: avoid role: solutions / forward-deployed engineering; avoid company: large companies
Status: rejected (was unseen)
Learned: avoid company: large companies (tentative)
Learned: avoid role: solutions / forward-deployed engineering (tentative)
It won't be recommended again. Changed your mind: narrow save opp_011ee5ec62edd51f11dc0fd4b3edd532
```

### Commands

| Command | Does |
| --- | --- |
| `narrow find [WORDS] [-n N] [--all]` | The few open opportunities most worth your time (default 5, strong fit and worth reviewing only), best first, each with its verification, eligibility and short brief (`--all`: maybe and low priority too). Refreshes and verifies as described in [`find`](#find-the-shortlist-and-when-it-reads-the-network); `--offline` (or the hidden `rank`) reads only what is stored |
| `narrow why <id> [--details]` | One opportunity's decision brief; `--details` lists every signal with its group, basis, weight and evidence |
| `narrow save\|unsave\|reject\|like\|dislike\|applied\|interview\|offer <id> [--reason "…"]` | Feedback on an opportunity (`opp_…`, a short id, or a `job_…` id), with how the reason was read and what was learned; a repeat already in effect changes nothing |
| `narrow pipeline [--all]` | Jobs you saved, applied to, are interviewing for or got an offer from (`--all`: and rejected ones) |
| `narrow taste [--all]` | What you told JobHunt, what it learned from your feedback (with evidence), what contradicts itself, and reasons it couldn't read |
| `narrow feedback [<id>]` | Your feedback, verbatim |
| `narrow show <id>` | Now also shows the fit verdict and your status on the job |

### The gate: eligibility and verification first

| Situation | Where it goes |
| --- | --- |
| eligible or conditional, verified recently at an authoritative source (`Assessment::recommendable`) | **Worth your attention** |
| eligible or conditional, but never verified, stale, or the last check failed | **Promising, but verify first** (`narrow verify <id>`) |
| eligibility uncertain | **Could be worth it, if you can take it**, with why (`narrow check <id>`) |
| ineligible, closed, rejected by you, already in your pipeline, or verified pay below a *required* minimum | not recommended; counted under "Not shown" |

A conditional job (you would relocate; the company grants the sponsorship
it offers) is ranked normally, with its condition as a caveat.
Uncertainty is never hidden, and a job is never ranked on eligibility it
doesn't have.

### Fit first, practicality second

Every job is assessed on two separate questions (the whole design is in
[docs/fit-and-practicality.md](docs/fit-and-practicality.md)):

- **Fit**: would you genuinely want this company and role? Read against
  your taste profile (what you said, confirmed or corrected counts more
  than Narrow's readings and inferences, which count more than patterns
  learned from feedback): the shape of the work (the title's own role
  words first), its depth (building a storage engine is not using
  PostgreSQL), the level, the company, team, ownership and culture.
- **Practicality**: can you pursue it, and what still needs checking?
  Eligibility, verification, pay, the work setup. Practicality never
  creates fit: pay, remote work, verification and freshness add nothing,
  and what a posting doesn't say (pay, team size) never counts against it.

What you see is a coarse tier, the fit: **Strong fit** (the work and at
least one more aspect affirmatively fit what you want, and nothing
contradicts it), **Worth reviewing** (something points to it, not enough),
**Maybe** (little does), **Low priority** (something you said you don't
want, a level two steps from yours, a specialization you neither want nor
have shown). Today shows strong fits only; `find` also lists jobs worth
reviewing. `why --details` shows how the fit was assessed: every reason and
contradiction, whose statement it rests on, and the practicality.

### Signals

Every signal is independently inspectable (`narrow why --details`): its
group, its basis (your preference, learned from your feedback, your
resume, the posting, your feedback on this job, verification,
eligibility), a one-line summary, and its evidence.

| Group | Reads |
| --- | --- |
| eligibility, verification | the stored assessment: eligible, conditional (a thing to check), uncertain; verified fresh, aging, or not trusted yet |
| role | the job's role shape from its title (backend, frontend, full stack, platform, infrastructure, SRE / DevOps, data, mobile, machine learning, security, embedded, solutions / forward-deployed, sales, product management, design), or its description when the title says only "Software Engineer"; against roles you want, accept, require or avoid; a non-engineering job when your resume is engineering; your role experience (context) |
| seniority | the title's level against your latest title (junior below a senior is a caveat; a step up is a stretch, not a penalty) |
| stack | technologies the job *requires* (its title, requirements lists, "must"/"strong experience" sentences) against those your resume shows you used; "nice to have" and passing mentions don't count; one missing technology is never a reason to skip |
| domain | the job's domains (title, department, or named more than once in its description) against domains you want or avoid; having worked in a domain is shown as experience, never as wanting it |
| pay | see below |
| company | company and team kinds the posting states (startup, early-stage, scale-up, founder-led, small team, small company, large company, public company, consulting, agency, open source, remote-first, …) against yours. Team size and company size are different facts: a stated opposite of the *same* kind (a large team against small teams; a large or public company against small companies) counts against a want and rules out a requirement, and a company's size never decides a team's. Unstated ones you care about are unknowns; a required one left unstated is unresolved, never taken as met, and never a strong fit |
| work style | ownership, management, on-call, greenfield, async, … against yours; people management when you want individual-contributor work |
| work mode | remote / hybrid / on-site against modes you want or avoid (a *required* mode is eligibility's) |
| feedback | what you did with this job (saved, liked, disliked), and notes about it only ("great product") |
| freshness | posted (or first seen) in the last week +; over 60 days a caveat, unless verification found it still listed |

Everything is read deterministically from the posting with fixed
vocabularies, and every fact keeps the words it came from. Benefits and
policy sections ("medical, dental & vision insurance") are skipped, so
they don't make a company look like a healthcare one. Nothing is guessed:
a posting that doesn't say how big the company is has no size.

### Compensation

Pay is compared with your minimum and target using the latest successful
verification's compensation facts (or, until verified, what discovery
stored, which says so):

- only a salary range in your currency (an ISO code) and your period is
  compared. Currencies are never converted and periods never translated;
  a bare `$` is never USD. Anything else is an **unknown**, with why ("Pay
  is in EUR; your minimum is in USD");
- **not published is unknown, not low**;
- a range labeled for another location ("US base salary range" when you
  live in Brazil) is not your pay: never compared, an unknown with why;
- a range topping out below a **required** minimum rules the job out when
  that pay is verified, and is a practical concern until then; below a
  preferred minimum or your target it is a concern. None of it changes
  the fit;
- reaching your target is a practical fact, never a reason for fit;
- your minimum or target without a currency is never compared (set one
  with `narrow preferences set compensation --currency …`);
- if you've turned jobs down over pay before, unknown or low pay gets a
  caveat.

### Feedback and state

Feedback is per **opportunity** (`opp_…`): a job listed on two boards is
saved, rejected or applied to once, and every source record keeps its own
provenance. Each action is an event that is never changed; state is folded
from events, so the history behind "rejected" is always there. Events keep
the record you acted on, so if identity grouping later merges two
opportunities, feedback follows its record.

State has two independent parts:

- the **pipeline stage**: unseen, seen (`show` and `why` record that you
  looked), saved, rejected, applied, interviewing, offer. Saving a
  rejected job brings it back; rejecting after applying withdraws, and
  keeps how far it went;
- **like / dislike**, which is orthogonal: liking a job is not applying to
  it, and disliking it is not rejecting it.

### Reasons

`--reason` is the most useful feedback there is. Reasons are kept exactly as
written, and read deterministically (`RuleReader`, revision `rules/1`)
into structured signals:

| Reason | Read as |
| --- | --- |
| "too corporate" | avoid company: large companies |
| "pure SRE" | avoid role: SRE / DevOps |
| "too frontend-heavy" | avoid role: frontend |
| "too much consulting" / "too much management" | avoid company: consulting / avoid work style: managing people |
| "salary too local" / "compensation too low" | avoid pay pegged to a local market / the pay level |
| "fintech" (rejecting) | avoid domain: fintech |
| "already worked with this domain" | avoid the job's own domains (resolved from the job) |
| "love tiny founder-led teams" | prefer founder-led companies, small teams |
| "interesting infra problem" | prefer domain: infrastructure; the problem (this job only) |
| "no ownership" / "not enough autonomy" | prefer work style: ownership (something missing is something wanted) |
| "great product but too corporate" | prefer the product (this job only); avoid large companies |
| "boring product" / "unclear remote policy" | about this job only: kept as a note, not generalized |
| "meh vibes" | nothing recognized: kept as written and listed by `taste` |

Clauses are read separately; "too", "boring", "hate" point away, "love",
"great", "interesting" toward, and "no", "lack of", "not enough" toward the
missing thing. Without a cue, the action decides (a reason given while
rejecting describes what was wrong). Interviews and offers carry no
direction of their own. The reading is recomputed whenever taste is
derived, so a better reader improves old feedback too. `ReasonReader` is
the seam for other readers (an AI-assisted one could augment it); nothing
requires one.

### Learned taste

Each learned pattern answers: what was inferred, from which feedback, how
many signals, whether they were your words or only behavior, when it was
last reinforced, and what contradicts it.

| Evidence | Weight |
| --- | --- |
| a reason naming something ("pure SRE") | 1.0 |
| a reason pointing at the job ("this domain") | 0.75 per fact it resolves to |
| liked / disliked | ±0.6 |
| applied (interviewing 0.6, offer 0.7) | +0.5 |
| saved | +0.25 |
| rejected without a reason (with one, the reason carries it: −0.1) | −0.2 |
| only looked at | 0 |

Behavior is spread over the job's facets (role, level, domains, company
kind, work style, required technologies, employer); reasons count for what
they name. A pattern is used when enough agrees: one reason makes it
*tentative*, two *established*, three *strong*; behavior alone needs three
jobs (tentative) or five (established) and is never strong, so passive
behavior can't create a strong preference, and one rejection without a
reason teaches next to nothing. When evidence on both sides is comparable
the pattern is **contradictory** and not used: rejecting one fintech job
"because fintech", then applying to two fintech infrastructure roles, does
not blacklist fintech, and the role (infrastructure) is its own pattern.
A stated preference about the same thing always wins; the learned pattern
stays visible next to it ("your feedback leans the other way").

### Caching and revisions

Rankings are recomputed from their inputs, and the ones shown to you
(`find`'s list, `why`) are stored in `opportunity_rankings` under a digest
of everything they depend on: the ranking rules (`RANKING_VERSION`), the
taste digest (every feedback event, the reader's revision and
`TASTE_VERSION`), the profile and its revision, every record's content and
status, the verification and eligibility answers, and the day. A new
input is a new key, so a stored ranking never outlives what it was
computed from; older rows are a record of what was shown and why. Bump
`RANKING_VERSION`, `TASTE_VERSION` or `RULE_READER_REVISION` with any
change that can rank, learn or read differently.

### Architecture

```text
jobhunt-cli ──► jobhunt-ranking ──► jobhunt-eligibility ──► jobhunt-jobs, jobhunt-profile
     │               ▲
     └──► jobhunt-storage (FeedbackRepository, RankingRepository)
```

`jobhunt-ranking` holds the feedback model, reason reading, job facets,
the person's side, learned taste, signals, gates, tiers, briefs, the rank
cache key and the `FeedbackRepository` / `RankingRepository` boundaries,
and `RankingService` (the use cases). It reads jobs, verification,
eligibility and the profile only through their domain types and
repository traits: no SQL, HTTP, CLI formatting or AI vendor code. The CLI
is thin.

## Using JobHunt from an AI assistant (MCP)

`narrow mcp` serves JobHunt over the [Model Context
Protocol](https://modelcontextprotocol.io) on stdio, so an MCP client
(Claude Code, Claude Desktop, Codex, or any other client that starts local
stdio servers) can search, inspect, verify and record feedback for you.

It is the same product, not a second one:

- **same configuration**: the normal config file, `--config` /
  `JOBHUNT_CONFIG`, `--database` / `JOBHUNT_DATABASE`; no MCP settings;
- **same database and profile**: the server opens the SQLite file the CLI
  uses. A job you reject in the terminal is gone from the assistant's next
  search, and the other way round, even while the server is running;
- **same behavior**: every tool calls the same use case as the matching
  command (`search_jobs` is `find`, `reject_job` is `reject`, …), so both
  interfaces reach the same decisions. `find --json`, `show --json`,
  `verify --json` and `pipeline --json` print exactly what the tools
  return;
- **no account and no API key**: the assistant provides the intelligence
  on its side; JobHunt's core stays deterministic and local.

The server speaks newline-delimited JSON-RPC 2.0 on stdin/stdout using the
official Rust SDK ([`rmcp`](https://github.com/modelcontextprotocol/rust-sdk)),
negotiating protocol versions from 2024-11-05 to 2025-11-25. Stdout carries
protocol messages only; logs go to stderr (`-v`, `--log-format json`,
`JOBHUNT_LOG` work as for every command, quiet by default). The server
exits when the client disconnects (stdin closes).

### Tools

| Tool | Kind | Does |
| --- | --- | --- |
| `search_jobs` | refreshes caches, network | The shortlist (`find`): `query`, `limit` (1–25, default 5), `refresh` (`auto` \| `always` \| `never`), `verify` (default true), `include_lower_tiers`. Returns the funnel, and per opportunity: `id`, `title`, `company`, `tier`, `recommendation`, `verification` (state, trusted, verified_at, authority), `eligibility` (status, headline), `why`, `consider`, `next_step`; plus what was not shown and why |
| `get_feed` | refreshes caches, network | What's new since the person last looked (the web's Today): up to `limit` (1–10, default 5) strong fits they haven't dealt with, plus reviewed ones that changed materially (pay published or changed, remote policy, work authorization, reopened), each with why and what to consider; `caught_up: true` and an empty list when nothing new is worth their time. Never padded with weaker matches |
| `get_taste` | read | What JobHunt believes the person wants, kept apart: stated preferences and statements (which always win), and patterns learned from feedback with their confidence and evidence (contradictory and weak ones listed, not used) |
| `get_job` | read | One opportunity: locations, workplace, compensation facts, description summary (`full_description` for all of it), verification, eligibility with reasons, the decision brief, pipeline state; `include_sources` adds every source record with its provenance and latest attempt. Does not mark it seen |
| `verify_job` | network | Asks the authoritative sources now (`force`, or reuse an attempt from the last few minutes): listing and application state, authority, last attempt and success, compensation facts, eligibility, what remains uncertain, per source |
| `get_profile` | read | Professional profile: headline, location, experiences, technologies with evidence strength, domains, role and ownership signals, preferences and statements, claims awaiting review, gaps. Never names or contact details |
| `get_taste_profile` | read | The taste profile: what the person wants and avoids (level, kind of work, specialization, ownership, company, team, culture, domains), each statement with its provenance and review; their practical constraints, apart; what was learned |
| `update_taste_profile` | **writes**, may call a configured model | `action`: `describe` (their words about the job they want), `reinterpret`, `confirm`, `correct` (new words and/or polarity), `neutral`, `remove`, `add`. Their decisions always win over any later reading |
| `update_preferences` | **writes** | `statement` (your words, kept verbatim), `set` (typed values: `role`, `compensation`, `work_mode`, `work_setup`, `location`, `region`, `timezone`, `relocation` (with `only_to`), `sponsorship`, `authorized_in`, `engagement`, `company`, `domain`, `work_style`, `unknown_pay`, `unclear_eligibility`), `remove` (`pref_…`/`stmt_…`). Returns what was understood, what is uncertain, what was not understood (verbatim), what was replaced, and every preference in effect |
| `save_job` | **writes** | Save (a rejected opportunity comes back) |
| `reject_job` | **writes** | Not interested, with the person's `reason` verbatim; returns how it was read and whether learned taste changed |
| `mark_applied` | **writes** | The person applied; nothing else about the application is stored |
| `record_feedback` | **writes** | `like`, `dislike`, `unsave`, `interview`, `offer` |
| `get_pipeline` | read | Saved, applied, interviewing, offers (`include_rejected`) |
| `prepare_application_context` | read | Evidence for helping with an application (below); `include_contact_details` adds name and contacts |

Tools declare this in their annotations (`readOnlyHint`,
`destructiveHint: false`, `idempotentHint`, `openWorldHint` for the two
that reach job boards, and `get_feed`, which may verify its candidates). Every tool has an input schema (unknown arguments
are rejected) and an output schema; results come as structured content,
with the same JSON as text for clients that read only text.

**Retries are safe.** A mutation that repeats one already in effect
(saving a saved job, marking applied twice, the same rejection with the
same reason, the same preference statement or value) changes nothing and
says so (`"recorded": false`, `"unchanged": true`), so a client retrying a
request can't pile up duplicate feedback. Concurrent identical requests
are serialized in the server, so they record once too.

**Errors are actionable.** A tool failure is a result with `isError: true`
and a JSON body `{"error": {"code", "message", "hint"}}`. Codes:
`unknown_opportunity`, `ambiguous_id`, `no_profile`, `no_jobs`,
`invalid_preference`, `invalid_arguments`, `source_unavailable`,
`verification_unavailable`, `conflict`, `storage`, `config`,
`cancelled`. Arguments that don't match a tool's schema are rejected the
same way before anything runs. Storage and configuration failures are
reported without local paths or internals (the details go to the server's
stderr; `narrow doctor` shows them). A cancelled `search_jobs` or
`verify_job` stops; what it already stored stays consistent (each source
scan and verification attempt is its own transaction).

### Application context

`prepare_application_context` (and `narrow context <id>`) prepares
evidence; it writes nothing (no cover letter, no answers, no tailored
resume). It returns the job and its decision brief, what the job asks for
(roles, level, technologies with their requirement, domains), your
relevant experience and projects with their facts, technologies matched to
the job, what the job asks for that no usable evidence covers, and
caveats.

Only claims the evidence policy marks usable are included
([`Claim::standing`](crates/jobhunt-profile/src/evidence.rs) is usable:
you confirmed or entered them, or they are quoted directly from your
current resume with high confidence), each with its resume snippet and
why it may be used. Inferences, uncertain readings, claims that left your
resume, and rejected claims are withheld (not even their text is sent) and
only counted, with how to review them (`narrow claims review`). An
experience appears only if the claim that you held it is itself usable.
The answer tells the client to use the facts as written, without adding
metrics, responsibilities or accomplishments.

### Connecting a client

Run `narrow doctor` for the exact command and arguments on your machine
(it prints the absolute path of the binary and your database). Use the
absolute path when the client does not share your shell's `PATH`.

**Claude Code** (tested: `claude mcp list` reports it connected):

```bash
claude mcp add --transport stdio narrow -- narrow mcp
# a specific database or config:
claude mcp add --transport stdio narrow -- /usr/local/bin/narrow --database ~/jobhunt/jobhunt.db mcp
```

or, for a project, `.mcp.json`:

```json
{
  "mcpServers": {
    "narrow": {
      "type": "stdio",
      "command": "/usr/local/bin/narrow",
      "args": ["mcp"],
      "env": {}
    }
  }
}
```

**Claude Desktop**: the same entry under `mcpServers` in
`claude_desktop_config.json` (Settings → Developer → Edit Config), with the
binary's absolute path.

**Codex CLI**:

```bash
codex mcp add narrow -- narrow mcp
```

or in `~/.codex/config.toml`:

```toml
[mcp_servers.narrow]
command = "/usr/local/bin/narrow"
args = ["mcp"]
# search_jobs may read many job boards the first time
tool_timeout_sec = 120
```

**Any stdio MCP client**: start `narrow mcp` (plus `--config` /
`--database` if you don't use the defaults) and speak MCP on its
stdin/stdout. The official Python and TypeScript SDK clients were used to
run the whole loop against it.

**ChatGPT** connects to remote MCP servers over HTTPS only (developer
mode); it cannot start a local stdio server. JobHunt does not ship an HTTP
transport (it would expose your local profile on the network), so ChatGPT
is not supported directly today.

## JobHunt Cloud

The same application, hosted: an account your machines sync with,
scheduled discovery and re-verification of one shared job corpus, a web
app, email notifications about strong new matches, an HTTP API
(`/api/v1`) and a hosted MCP endpoint (`/mcp`) for remote assistants. The
full design, deployment and configuration reference is in
[docs/cloud.md](docs/cloud.md); the web app is described in
[apps/web/README.md](apps/web/README.md).

### The web app

A small product on purpose: **Today**, **Applications**, **Preferences**,
**Profile** (and Settings). A visit should be short: open Today, see the
two to five opportunities that are new and worth your time, each with why
it may matter and what to consider (verification, eligibility and pay in
plain words, never a match percentage), save, reject with a reason, mark
applied or put aside, and leave. When nothing new is worth your time,
Today says you're caught up instead of showing weaker jobs. Everything the
web shows comes from the same use cases and views as the CLI and MCP
tools; nothing about ranking, eligibility, verification or taste is
decided in the browser.

### Email notifications

Off by default; on in Settings for a confirmed address. An email means
"probably worth interrupting you for": only strong fits, verified at the
employer, that you haven't seen, acted on or been emailed about, at most
three per email and at most one email every few hours (or a day). Nothing
found means no email.

```bash
narrow login --server https://jobhunt.example.com   # sign in in the browser (device code)
narrow sync                                         # merge profile, decisions, preferences, feedback
narrow sync --status                                # offline: where sync stands, conflicts
narrow sync --keep local                            # resolve conflicts (or --keep cloud, --record <id>)
narrow account                                      # the account and this machine's sync state
narrow token create "Claude Desktop"                # a personal access token for an MCP client
narrow logout [--everywhere]
```

- **Offline first.** Only `login`, `logout`, `account`, `sync` and `token`
  reach the cloud; every other command works on the local database,
  online or not. When the cloud is unreachable, `sync` says so and changes
  nothing.
- **Sync** merges per record, never "last writer wins" for the whole
  profile: a change on one side wins, different fields changed on both
  sides merge, and the same field changed differently (a claim confirmed
  here and rejected there) is a conflict that is shown and kept until you
  choose. Feedback merges by union and is never lost.
- **Shared corpus.** Job boards are read once for everyone by a scheduled
  worker (conditional requests, the same lifecycle and history), and the
  jobs that matter to someone are re-verified in the background.
  Eligibility and ranking stay per person.
- **Private data** (profile, resume text, claims, preferences, feedback
  reasons, eligibility decisions, rankings) is isolated per account and
  encrypted by the application (AES-256-GCM) before it reaches Postgres.
- **Deployment**: one binary with process modes (`narrow server`,
  `narrow migrate`, `narrow worker discovery|verification|notify`), the
  web app ([apps/web](apps/web)), two Dockerfiles, and Railway
  Infrastructure as Code in [.railway/railway.ts](.railway/railway.ts).

## Your data: export and import

`narrow export` writes everything that is yours and can't be rebuilt, as
one versioned JSON document (`"format": "jobhunt.state"`, `"version": 1`):

| Included | Why |
| --- | --- |
| your profile, as the profile export format (`jobhunt.profile`): experiences, projects, education, skills, every claim with its evidence **and your decision about it**, preferences and statements in your words | it is you |
| every piece of feedback, verbatim (save, reject, applied, interview, offer, like, dislike, unsave, with reasons) | your pipeline stage and learned taste are folded from it, so both come back exactly; neither is stored separately |
| the source records that feedback is about (postings and lifecycle dates) | so your pipeline survives on a machine that hasn't discovered those jobs; job ids are stable, so the next discovery updates them in place |

Left out, because they are rebuilt: the rest of the discovered jobs,
discovery runs and HTTP validators, verification history, eligibility
decisions, stored rankings, and "looked at" marks.

```bash
narrow export -o narrow.json      # or: narrow export > narrow.json
narrow import narrow.json         # --replace to replace an existing profile
```

`import` checks the format and version first, parses strictly, validates
every reference (the embedded profile, every feedback event naming a job
in the file), then writes everything in one transaction: an invalid file,
or a failure halfway, changes nothing. Feedback and jobs already present
are left alone, so importing the same file twice changes nothing; an
existing profile is replaced only with `--replace`. `narrow profile
export` / `import` remain for the profile alone.

## Privacy

Your profile, resume text, preferences, feedback and every job stay in the
local SQLite database; JobHunt itself sends nothing anywhere except
requests to the job boards and employer pages it reads.

When you connect an MCP client, what a tool returns is sent to that client,
and through it to whichever model provider the client uses, only when a
tool is called:

- `get_profile` never includes your name or contact details, nor your
  resume's raw text;
- `prepare_application_context` includes only usable evidence (with its
  resume snippets), and your name and contacts only when
  `include_contact_details` is set;
- search results, job details and verification describe jobs, not you
  (eligibility headlines can mention where you live, e.g. "in New York,
  where you live").

Errors sent to clients never include local file paths. Nothing is sent to
a model provider by JobHunt itself; no AI API key is used or needed.

With JobHunt Cloud, only what `narrow sync` sends leaves your machine
(your profile records and feedback, and the jobs that feedback is about),
to your own account. In the cloud it is isolated per account and
encrypted by the application; logs carry ids, never your text; usage
events are counts. See [docs/cloud.md](docs/cloud.md#encryption).

## Logging

Logs are structured (`tracing`) and go to stderr, so they never mix with
results on stdout. By default only errors are logged.

```bash
cargo run -- find -v                     # per-source table and progress logs
cargo run -- find -vv                    # plus debug detail (HTTP, rejected records, links)
cargo run -- find -v --log-format json   # JSON lines
JOBHUNT_LOG="warn,jobhunt_jobs=debug" cargo run -- find   # any tracing filter
```

`-v` prints a table with, per source, the time taken and how many postings
were received, rejected, new, updated, unchanged, reopened and closed, and
whether the scan was a full listing, "not modified", partial, or had closing
withheld. The logs carry the same as events: `discovery run started`,
`source started`, `fetch completed`, `source not modified`,
`source completed`, `not closing missing jobs`, retries,
`duplicate detected`, `discovery run completed`.

The same applies to `narrow mcp`, where it is a protocol requirement:
stdout carries MCP messages only, and everything else (logs at any
verbosity, progress, errors) goes to stderr, which MCP clients capture in
their log files. Colors are used only when stderr is a terminal.

## Workspace layout

```text
crates/
  jobhunt-core      Domain-agnostic primitives: canonical URLs, stable IDs,
                    fingerprints, HTML-to-text, source keys/provenance, the
                    Source trait (conditional fetch, completeness), counters.
  jobhunt-profile   The profile domain: career profile, evidence claims and
                    policy, preferences and their statement parser, resume
                    re-import rules, export format, ProfileRepository,
                    ProfileService.
  jobhunt-resume    Resume files (PDF, text, Markdown) into text, and the
                    deterministic parser into the profile's ParsedResume.
  jobhunt-jobs      The jobs domain: canonical model, lifecycle rules,
                    cross-source identity, the JobRepository boundary, the
                    Discovery pipeline, and verification (records, states,
                    authority, freshness, trust, the ListingVerifier contract,
                    VerificationRepository, VerificationService).
  jobhunt-eligibility Location normalization (geography and time-zone
                    tables, region definitions), restriction extraction with
                    evidence, profile facts, the rules, decisions and reasons,
                    opportunity aggregation, and the decision cache boundary.
  jobhunt-ranking   Feedback on opportunities, reason reading, job facets,
                    learned taste with its evidence, ranking signals, gates,
                    tiers, decision briefs, the rank cache, RankingService.
  jobhunt-eval      The recommendation-quality benchmark: synthetic
                    candidates, postings and expected judgments run against
                    the production ranker (`narrow-eval`); see
                    docs/recommendation-benchmark.md.
  jobhunt-sources   Adapters (Ashby, Greenhouse, Lever, YC), careers-page board
                    detection, the HTTP verifiers, and the shared HTTP client.
  jobhunt-storage   Storage backends behind the repository traits (bundled
                    as `Store`): local SQLite, and Postgres for the cloud
                    (shared corpus, per-account encrypted private data,
                    sync, leases, accounts, usage).
  jobhunt-app       The application every front-end shares (over any
                    Store): config loading, opening the database, sync,
                    and the use cases (find:
                    refresh + rank + verify; resolve ids; inspect; verify;
                    feedback; preferences; profile view; application
                    context; export/import; doctor) with their typed,
                    serializable answers. No printing, no protocol code.
  jobhunt-mcp       The MCP server (rmcp): each tool is a thin adapter over
                    one jobhunt-app use case; served over stdio locally,
                    over Streamable HTTP by the cloud.
  jobhunt-cloud     JobHunt Cloud: environment configuration, OIDC and
                    token authentication, the HTTP API, hosted MCP,
                    scheduled workers, usage events, the cloud client.
  jobhunt-cli       The `narrow` binary: arguments, logging, human output,
                    `narrow mcp`, login/sync, and the cloud process modes
                    (server, migrate, workers); every command calls
                    jobhunt-app.
```

Dependencies only point downward: `cli → cloud → mcp → app`, `app → sources,
storage → jobs → core`, `app, storage → profile → core`, `app →
eligibility → jobs, profile`, and `app, storage → ranking → eligibility`
(the CLI also uses `resume` to read resume files). `jobhunt-jobs` and `jobhunt-profile` do not depend on any HTTP,
SQL or PDF crate, nor on each other; `jobhunt-eligibility` is the only
place where they meet.

## Tests

```bash
./scripts/check.sh                  # the full local quality gate (what CI requires)
./scripts/check.sh --cloud --e2e    # also Postgres, the web app and the browser tests
cargo test                          # everything offline
JOBHUNT_TEST_DATABASE_URL=postgres://postgres:postgres@127.0.0.1/postgres \
  cargo test                        # also the cloud tests against a real Postgres
cargo test -p jobhunt-sources --test ashby_live -- --ignored --nocapture
cargo test -p jobhunt-sources --test greenhouse_live -- --ignored --nocapture
cargo test -p jobhunt-sources --test lever_live -- --ignored --nocapture
cargo test -p jobhunt-sources --test yc_live -- --ignored --nocapture
cargo test -p jobhunt-eligibility --test verification_live -- --ignored --nocapture --test-threads 1
JOBHUNT_LIVE_GREENHOUSE_BOARDS=gitlab,databricks cargo test -p jobhunt-sources --test greenhouse_live -- --ignored --nocapture
```

The live tests read several real boards per family (override them with
`JOBHUNT_LIVE_ASHBY_BOARD`, `JOBHUNT_LIVE_GREENHOUSE_BOARDS`,
`JOBHUNT_LIVE_LEVER_SITES`, `JOBHUNT_LIVE_YC_COMPANIES`), print counts and
timings, and fail if a listing is incomplete, more than 5% of postings are
rejected, or any converted posting is invalid.

Tests that need Postgres (the storage contract's Postgres half, isolation,
encryption at rest, sync, leases, the HTTP API, hosted MCP, the binary's
cloud modes) create a fresh database per test on the server named by
`JOBHUNT_TEST_DATABASE_URL` (any user allowed to create databases), and
are skipped without it; CI's `cloud` job runs them against Postgres 18
with `JOBHUNT_REQUIRE_POSTGRES=1`, which turns a skip into a failure.

- `jobhunt-storage` (both backends): one repository contract run against
  SQLite and Postgres (lifecycle and history, not-modified and failed
  scans, search and prefixes, identity, verification, profiles and
  revisions, feedback, rankings, eligibility cache, atomic imports, the
  write lock) plus a whole scenario whose rankings must match; Postgres
  only: per-account isolation and deletion, shared jobs with per-person
  eligibility, encryption at rest and ciphertexts bound to their owner,
  key rotation, sync compare-and-set and idempotency, concurrent writers,
  source and verification leases (exclusive, expiring, backing off),
  accounts and tokens, concurrent migrations, usage events.
- `jobhunt-app` (sync): first sync, repeats, a second machine, changes both
  ways, field merges, conflicting decisions surfaced and resolved,
  offline changes synced later, feedback by union.
- `jobhunt-cloud`: configuration validation, OIDC tokens against a mock
  provider (audience, issuer, expiry, unknown key, algorithm confusion),
  personal tokens and logout, every API endpoint's view and error codes,
  account isolation, sync over HTTP, the device flow, and hosted MCP
  through the official MCP client (same tools, per-account data,
  401 challenge). BRU-295 (`web.rs`, `email.rs`, `api_schema.rs`): the
  Today feed (a small set of new recommendations, reloads stable, acted-on
  jobs gone, caught up without padding, passed-over items leaving,
  material changes resurfacing and cosmetic ones not, cross-source
  duplicates once), resume upload and claim review over the API, taste,
  expired sessions, email notifications through the outbox (only strong,
  verified, unseen, unnotified matches; grouped; sent once across retries,
  crashes after the provider accepted, and concurrent workers; provider
  failures retryable, permanent ones failed, stale ones abandoned; the
  cursor moving only after a send; nothing crossing accounts), the Resend
  sender against a mock, and the web app's API schema being current.
- `apps/web`: component tests (Vitest, Testing Library, axe) for the
  recommendation card, decision brief, eligibility and pay wording,
  actions with rollback, the reject dialog, preference interpretation,
  learned vs stated taste, claim review and the caught-up state; and
  Playwright end-to-end tests of the whole loop against the real stack
  (see [apps/web/README.md](apps/web/README.md)).
- `jobhunt-cli` (`cloud_e2e`): the binary's `migrate`, `server`, two
  concurrent discovery workers against recorded boards, verification,
  `login`, `sync` from two machines, `token`, offline behavior, `logout`.
- `jobhunt-core`: URL normalization against real ATS URL variants (and URLs
  that must stay distinct), stable IDs, fingerprints, HTML-to-text.
- `jobhunt-jobs`: lifecycle rules, closing safeguards, conditional fetches,
  cross-source grouping and its safeguards, pipeline behavior against an
  in-memory repository.
- `jobhunt-storage`: migrations (including upgrading a first-release
  database, and a jobs-only database to profiles), lifecycle persistence
  and history, scans, opportunities, search; profiles stored and loaded
  exactly, revision conflicts, deletions, skill evidence, claim queries,
  profile history.
- `jobhunt-profile`: dates, ids, the evidence policy, vocabularies
  (technologies, domains, roles, ownership), the preference statement
  parser, and use cases against an in-memory repository: first import,
  identical re-import, a changed resume (decisions, edits, stale and
  superseded claims, corrected titles), rejection across re-imports,
  manual claims, preferences and supersession, export round trip and
  all-or-nothing import, concurrent writers.
- `jobhunt-resume`: fixtures under `tests/fixtures/` (see its README): a
  two-page Chromium PDF, a TeX-style PDF without space characters,
  Markdown and plain-text resumes, and blank, truncated and fake PDFs.
- `jobhunt-jobs` (verification): active, changed, reopened and deleted
  listings through the lifecycle, timeouts, 5xx and unreadable answers
  keeping the last success, reuse and forced verification, mismatched
  records, unsupported sources, compensation changes across
  verifications, a bare `$` staying ambiguous, the employer page in the
  authority chain, and opportunity trust (one source dead and one live,
  two live, stale, failed, never verified, secondary only, closed by
  discovery after a verification).
- `jobhunt-eligibility`: the rule matrix on synthetic postings (remote
  scopes for Brazil and other countries, disputed memberships, description
  restrictions and preferences, exclusions, on-site and hybrid with and
  without relocation, offices named in prose, mixed options, time zones of
  every kind, work authorization and sponsorship, "no sponsorship" on
  remote jobs, contractors and EOR, relocation, conflicting evidence,
  missing profile facts, office-city scopes); the region definitions at
  their boundaries; every adapter's saved real postings (Linear's Europe
  listing with a broader description, Figma, Spotify, Work at a Startup's
  visa field, Pacific hours, Anthropic's Sydney office, a hybrid job with
  a remote flag); opportunity aggregation and the decision cache
  (profile, job, verification and rules changes each invalidate it).
- `jobhunt-ranking`: job facets (titles to role shapes and levels,
  generic titles read from the description, required vs preferred vs
  mentioned technologies, domains needing more than a stray mention,
  company and work-style statements with their sentence, benefits
  boilerplate skipped); the reason reader on every documented phrasing
  (cues, absence, clauses, job references, unread reasons, scope);
  feedback state (pipeline folding, withdrawals, like/dislike apart from
  it); learned taste (strong patterns from repeated reasons with their
  evidence, reasonless rejections staying weak, contradictions not used,
  explicit preferences winning, references resolved to the job, single-job
  notes, digests); and ranking (a strong fit and its brief, the
  eligibility and verification gates, conditional jobs, feedback on the
  job, unwanted roles and non-engineering jobs, pay below a required
  minimum verified or not, unknown, ambiguous, foreign-currency and
  currency-less pay never read as low, learned taste moving rankings with
  attribution, stated preferences outranking it, one rejection not
  blacklisting a domain, work mode and style, ordering and round trips).
- `jobhunt-eval`: the recommendation benchmark's fixture integrity
  (judgments consistent with their reasons, every job judged, pairs
  complete, the golden, archetype, contrastive and compensation cases
  encoded), the evaluator's verdicts and metrics, determinism, and the
  recorded baseline being current (`JOBHUNT_UPDATE_BENCHMARK=1` regenerates
  it). Cases the current ranker gets wrong are recorded, not failed.
- `jobhunt-storage` (ranking): feedback round trips and per-record
  lookups, duplicates sharing state and feedback following a merge,
  ranking through `RankingService` over SQLite (gates, exclusions,
  learning from a reason, what was shown stored and reused, a new event
  being a new key, verified pay below a minimum).
- `jobhunt-sources` (verification): each family's verifier against a local
  mock serving saved real responses: active, closed and redirected
  listings, working and broken application paths, 5xx, garbage, timeouts,
  one request per Ashby board, unsupported sources.
- `jobhunt-sources`: per adapter, conversion of saved real responses
  (`tests/fixtures/<family>/`, including files with deliberately broken
  records) and HTTP behavior against a local mock server (conditional
  requests, 404s, retries, truncated listings, garbage bodies, failed YC
  detail pages).
- `jobhunt-cli`: config, output, and end-to-end tests: every family served
  from one mock server through one pipeline into a SQLite file, then mutated
  across runs to prove NEW / UNCHANGED / UPDATED / CLOSED / REOPENED, that
  failed and partial scans close nothing, and cross-source grouping; and the
  `narrow` binary through the profile flow (`init`, `profile`,
  `preferences`, `claims`, edits, `export`, re-import of a changed PDF,
  `import` into a fresh database, unreadable files) with a temporary
  database; eligibility through the binary (`find` verdicts and
  `--eligible`, `check` with evidence, `show`) over real Ashby and
  Greenhouse responses discovered into a temporary database; and the whole
  verification flow offline: `init` a resume PDF, discover Ashby,
  Greenhouse and Lever fixtures, then `verify` against mock authoritative
  endpoints (`JOBHUNT_VERIFY_ENDPOINT`), `show` and `check` without
  fetching, reuse within the window, `--force`, `--details`, a profile
  change, 503s keeping the last success, jobs taken down, and `find`
  filtering; and ranking through the binary (`rank` / `find --offline`
  with and without `--all`, `why --details`, every feedback command with reasons read or
  kept as written, `taste`, `pipeline`, `feedback`, `show`'s fit) over
  real Ashby and Greenhouse responses, a resume and verified jobs.

- `jobhunt-app`: id resolution and short ids, source selection, the
  structured preference inputs, description summaries.
- The local product, through the binary (`crates/jobhunt-cli/tests`, all
  offline, against mock job boards serving saved real responses for both
  discovery, `JOBHUNT_DISCOVERY_ENDPOINT`, and verification,
  `JOBHUNT_VERIFY_ENDPOINT`):
  - `product_e2e`: first run without a profile, the first-use flow, the
    fast path (a repeated `find` makes no request), `--refresh`,
    `--offline`, `--raw`, `--json`, short ids, falling back to stored jobs
    when an automatic refresh fails, "no jobs yet", `export` / `import`
    into a fresh database (pipeline and taste identical, re-import changes
    nothing, `--replace`, wrong format and version), `doctor`;
  - `mcp_protocol`: `narrow mcp` as a child process spoken to over
    stdin/stdout: handshake, version negotiation, `ping`, every tool's
    schemas and annotations, every error code, schema violations, a server
    that can't start writing nothing to stdout, ambiguous and unique short
    ids, clean exit when the client disconnects, and, at `-vvv` with JSON
    logs, every stdout line a JSON-RPC message;
  - `mcp_workflow`: the whole loop through MCP (profile, preferences,
    refresh and verify through `search_jobs`, `get_job`, `verify_job`,
    save, reject, applied, the next search reflecting it, application
    context checked claim by claim against the evidence policy in the
    database), confirmed / rejected / stale evidence, and the ranking rules
    (ineligible and rejected jobs stay out, applied ones join the
    pipeline, what you said beats learned taste, unpublished pay is unknown
    not low, no percentages);
  - `shared_state`: the CLI and a running MCP server on one database (a
    save through MCP in the CLI's pipeline, a CLI rejection in the next
    MCP search), identical answers from `find --json` / `search_jobs`,
    `show --json` / `get_job`, `pipeline --json` / `get_pipeline`,
    equivalent state from `reject` and `reject_job`, retries of every
    mutation through both interfaces, concurrent MCP requests next to CLI
    processes, and processes creating one new database at once.

Only the offline suite runs in required CI. The live tests run separately,
three times a week and on demand, in the "Live sources" workflow, so a
third-party outage never blocks a merge.

Before sending changes, run `./scripts/check.sh`. It runs formatting,
clippy (warnings denied), the build, every offline test and rustdoc
(warnings denied), exactly as required CI does, and `--msrv` adds the
minimum-Rust check. [CONTRIBUTING.md](CONTRIBUTING.md) describes the CI
checks, the live validation workflow, the PR workflow and branch
protection.

## Adding a source

1. Add a module in `crates/jobhunt-sources/src/` implementing
   `jobhunt_core::Source<Record = JobPosting>`: fetch with the shared
   `HttpClient` (use `get_conditional` if the source sends ETags), parse,
   convert. Follow `greenhouse.rs`/`lever.rs`: raw payload types with
   optional fields, per-record `RecordError`s, no invented values. Set
   `SourceBatch::complete` only when the batch is provably the whole
   listing, and `validator` to the response's ETag.
2. Add a `SourceSpec` variant (in `from_key`, `from_board`, `key`, `build`),
   a list on `SourcesConfig`, and the kind to `SUPPORTED_KINDS`. If its URLs
   carry a global job id, teach `identity::ats_job_ref` to read it, and
   `careers::board_for_url` to recognize its boards.
3. Save real responses under `tests/fixtures/<source>/`, test the conversion
   and HTTP behavior against them, and add an `#[ignore]` live test.
4. If the change alters what existing adapters produce, bump
   `CANONICAL_REVISION` so stored "not modified" validators are not reused.

Nothing in the pipeline, lifecycle, storage or CLI output changes.

## Known limitations

Local product and MCP:

- Not built yet: application assistance (writing answers, cover letters,
  tailored resumes; `prepare_application_context` only prepares evidence),
  notifications, a web interface, billing. JobHunt Cloud's own
  limitations are listed in [docs/cloud.md](docs/cloud.md#known-limitations).
- `narrow mcp` speaks stdio; remote clients that need HTTPS use JobHunt
  Cloud's hosted `/mcp` endpoint.
- The MCP server has no resources or prompts; the tools cover the product.
  It reports progress on stderr, not as MCP progress notifications.
- Discovery and verification run with a process-wide lock only around
  read-modify-write use cases (feedback, preferences, imports); across
  processes, SQLite serializes writes (`BEGIN IMMEDIATE`, 5 s busy
  timeout) and profile changes retry on a lost optimistic-revision race.
  A very long discovery run in one process can make another process's
  write wait for up to the busy timeout.
- The statement parser reads English with fixed vocabularies; parts it
  can't read are kept and reported, never used.

Profile:

- Resume parsing is rule-based. Unusual layouts (two-column designs whose
  columns interleave in the PDF, tables, headings JobHunt does not know)
  can be misread; `init` reports what it did not understand, and anything
  can be corrected. Scanned (image-only) PDFs need OCR, which JobHunt does
  not do; `.docx` must be saved as PDF or text first.
- Domain, role and seniority inferences come from fixed vocabularies
  (English, with some Portuguese). They are always marked inferred and
  need your confirmation.
- The preference parser understands common phrasings in English;
  everything else is kept as written and flagged.

Eligibility:

- Places are read from a table of about 75 countries, their business
  regions, subdivisions used in postings, and major tech cities; anything
  else stays unrecognized (and the answer `unknown`). Region membership is
  a judgment ("North America" includes Mexico only *maybe*).
- Description sentences are read with fixed English cues ("based in",
  "authorized to work", "sponsor", time zones); other phrasings and
  languages are missed, so a missing restriction is not proof there is
  none. Each answer shows the sentences it used.
- A remote job in a city ("Remote (San Francisco; Oakland)") is treated as
  commuting distance, and whether elsewhere in the country works is
  uncertain. Commuting distance between two cities is never estimated.
- Work authorization is only what you stated; JobHunt does not know
  citizenship rules, and the answer is a compatibility signal, not legal
  advice.
- No currency conversion, and pay is not part of eligibility (a pay
  minimum is a ranking question, answered by `narrow find`).
- Verification is plain HTTP. Employer careers pages are not checked
  (so no listing reaches `employer_first_party` yet); Ashby application
  pages render in a browser and are known from the API, not requested; a
  Work at a Startup application page may need an account.
- A job verified closed stays open in the jobs table until discovery
  closes it; meanwhile `show`, `check` and `verify` show it closed, `find`
  marks it ("Verified closed at its source") and `--eligible` /
  `--possible` never offer it.

Ranking:

- Job facets, levels and reasons are read with fixed English
  vocabularies. A posting that describes itself in other words (company
  size, stage, work style) simply has no such fact, and a reason with
  nothing recognizable is kept as written and listed by `narrow taste`.
- Learned patterns are about single facets (a role, a domain, a company
  kind). "Fintech is fine, fintech sales isn't" is learned as two patterns
  (the domain is contradictory; sales is avoided), not as a combination.
- Learned taste doesn't decay with time; `taste` shows when each pattern
  was last reinforced.
- `find` (and `search_jobs`) reads every open stored opportunity and its
  verification and eligibility from the database each time (seconds for
  thousands of jobs); only the network is skipped while stored jobs are
  fresh.

Jobs:

- YC companies must be listed individually. The directory of ~1,500 hiring
  companies is available (through the site's public search index), but
  scanning all of them costs thousands of page loads per run, so it is not
  done by default. YC pages carry per-request tokens, so they can't answer
  "not modified" and are re-read every run (listing + one page per job).
- Careers pages that render their jobs with JavaScript can't be resolved to
  a board; arbitrary first-party pages without an ATS behind them are not
  read.
- Cross-source grouping only uses URL and ATS-id evidence. Real duplicates
  without shared evidence (PostHog on Ashby and YC) are shown twice and
  counted as look-alikes.
- Grouping is recomputed over every stored record each run (fast for tens of
  thousands of records; a much larger corpus would need an incremental
  version).
- Greenhouse boards hosted in its EU region (`job-boards.eu.greenhouse.io`)
  are recognized in URLs but read through the global API, which does not
  serve them. Lever's EU region is supported (`region = "eu"`) but was not
  verified against a live EU site. Only public, unauthenticated endpoints
  are used.

## Feedback

Trying Narrow against a real job search and telling us where it fails is
more useful than praise. Open a [feedback
issue](https://github.com/bernacle/narrow/issues/new?template=experiment-feedback.yml)
once you've run `init`, `preferences add` and `find` at least once: what
got in the way, whether the shortlist felt better than browsing job boards
yourself, what you did with the results, what you wished it did. Issues
are public, so keep it to the product experience — no resume contents,
recruiter names, salary tied to your identity, or application details.
See [docs/oss-adoption-experiment.md](docs/oss-adoption-experiment.md) for
what this feedback is used for. Security vulnerabilities go through
[SECURITY.md](SECURITY.md) instead, never a public issue.

## License

Narrow is licensed under the [Apache License, Version 2.0](LICENSE).

It ships geographic data derived from [GeoNames](https://www.geonames.org),
licensed under [Creative Commons Attribution
4.0](https://creativecommons.org/licenses/by/4.0/): see
[`crates/jobhunt-eligibility/data/geonames`](crates/jobhunt-eligibility/data/geonames/README.md)
for what is included, how it was made and how to refresh it. Time-zone
rules come from the IANA time zone database (public domain) through the
`chrono-tz` crate (MIT or Apache-2.0).

Contributions are welcome: see [CONTRIBUTING.md](CONTRIBUTING.md). Please
report security vulnerabilities privately, as described in
[SECURITY.md](SECURITY.md).
