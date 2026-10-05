# Source expansion, Batch 2A (prepared, not deployed)

Batch 2A is the first four boards of the next expansion batch
(`source-discovery-and-career-pages.md` §19):

- `greenhouse:elastic`
- `ashby:clickhouse`
- `greenhouse:gitlab`
- `greenhouse:sourcegraph91`

Each was revalidated today and measured against production's current 21
sources, alone and together, for the real dogfood profile. Nothing was
deployed: `deploy/cloud.toml` and the registry statuses are unchanged.

No ranking, Fit, eligibility, Today selection or source-discovery change.
All numbers are from 2026-10-05 (UTC).

**Result in one paragraph.**
- **Elastic dominates the volume:** 395 of the 809 added open postings,
  67 of the 88 added actionable jobs, and 60 of the 65 added engineering
  postings open to Brazil.
- **Elastic adds nothing above low priority for this profile:** 0
  plausible, 0 strong. 61 of its 67 actionable jobs are held down by the
  profile's own "small companies" preference, and the other 6 are sales
  or management.
- **Small but real gains elsewhere:** Sourcegraph adds the one plausible
  platform role. ClickHouse adds two plausible jobs that are
  customer-facing or teaching. GitLab adds nothing above low priority.
- **Today doesn't move in any cell:** it stays Railway, 1 company and 4
  jobs.

## 1. Production baseline (A): BRU-343 rollout

See `source-discovery-and-career-pages.md` §20 for the deploy check.

| | BRU-343 expected | Production (measured) |
| --- | ---: | ---: |
| Sources configured | 21 | 21 (image `278883a`) |
| Open jobs | ~2,948 | 2,941 (feed log) · 2,939 (mirror) |
| Actionable | 222 | 221 (mirror) |
| Plausible | 45 | 45 (mirror) |
| Strong | 4 | 4 (feed log and mirror) |
| Today | Railway, 1 company · 4 jobs | 1 company shown (feed log) · Railway, 4 jobs (mirror) |

**How each number was measured.**
- **Feed log:** production's own `today prepared` line, logged when the
  person opened Today at 14:28 UTC.
- **Mirror:**
  - the real profile, synced from production at 14:35 UTC;
  - jobs from a fresh scan of exactly the 21 production sources at
    14:37;
  - ranked offline with `real_posting_probe` at 14:40.
- **Mirror vs. production's worker:** every source's open count in the
  mirror equals what the worker received at 14:11 (Stripe 718, Anthropic
  638, Airbnb 149, Railway 8, Oyster 23, …).
- **Mirror vs. expected:** the mirror is 1 actionable job short of
  expected. Oyster now lists 23 postings, not 26.

## 2. Revalidation today

`narrow sources discover` from each company's domain, plus two full scans
through the adapters (the second scan got `not_modified` from all four).

| Board | Found via | Validated | Provider ids | Rejected | Ownership evidence |
| --- | --- | --- | ---: | ---: | --- |
| greenhouse:elastic | slug guess | yes | 395/395 | 0 | 395 of 395 postings on or naming elastic.co |
| ashby:clickhouse | slug guess | yes | 195/195 | 0 | board names clickhouse.com |
| greenhouse:gitlab | slug guess | yes | 209/209 | 0 | 40 of 209 postings name gitlab.com |
| greenhouse:sourcegraph91 | careers page | yes | 10/10 | 0 | sourcegraph.com/jobs points at it |

All four have health `healthy` and no unmet activation condition.

## 3. Per-candidate yield

Eligibility counts are for the reference profile (São Paulo, remote only,
no relocation):
- **eBR:** engineering postings open to Brazil;
- **eBR?:** engineering postings where eligibility is unclear.

Freshness uses each posting's own `posted_at`.

| Board | Open | Eng | eBR | eBR? | Main geography | <7 d | <30 d | >90 d (stale ratio) | >365 d | Scan |
| --- | ---: | ---: | ---: | ---: | --- | ---: | ---: | ---: | ---: | --- |
| greenhouse:elastic | 395 | 199 | **60** | 0 | 331 office, 60 global, 4 Americas | 34 | 131 | 53 (13%) | 0 | ok, conditional |
| ashby:clickhouse | 195 | 107 | 2 | 0 | 71 office, 63 N. America, 32 other, 21 Europe | 15 | 62 | 72 (**37%**) | 2 | ok, conditional |
| greenhouse:gitlab | 209 | 93 | 1 | 1 | 99 N. America, 41 Europe, 41 other | 32 | 92 | 18 (9%) | 0 | ok, conditional |
| greenhouse:sourcegraph91 | 10 | 2 | 2 | 0 | 10 "Remote", no scope | 2 | 5 | 1 (10%) | 1 | ok, conditional |

**Duplicates.** Elastic posts one role once per location, so its 395 open
postings carry only 201 distinct titles. Its 67 actionable postings carry
21 distinct titles; for example, *Senior Software Engineer – Search
Algorithms* appears 9 times. They are separate Greenhouse ids, and
identity is unchanged (BRU-343 §15).

## 4. Marginal value for the real profile

**Rules only:**
- no reviewer;
- `verify` off;
- the same instant, `2026-10-05T14:40:00Z`, for every cell;
- every cell's 21 production sources returned identical counts.

| Cell | Sources | Open | Eng | Actionable | Plausible | Strong | Today | eBR | eBR? |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- | ---: | ---: |
| **A** production | 21 | 2,939 | 1,077 | 221 | 45 | 4 | Railway · 4 | 32 | 25 |
| A + elastic | 22 | 3,334 | 1,276 | 288 (+67) | 45 (+0) | 4 | unchanged | 92 (+60) | 25 |
| A + clickhouse | 22 | 3,134 | 1,184 | 228 (+7) | 47 (+2) | 4 | unchanged | 34 (+2) | 25 |
| A + gitlab | 22 | 3,148 | 1,170 | 228 (+7) | 45 (+0) | 4 | unchanged | 33 (+1) | 26 (+1) |
| A + sourcegraph91 | 22 | 2,949 | 1,079 | 228 (+7) | 46 (+1) | 4 | unchanged | 34 (+2) | 25 |
| **C** A + Batch 2A | 25 | 3,748 | 1,478 | **309 (+88)** | **48 (+3)** | **4 (+0)** | **unchanged** | 97 (+65) | 26 (+1) |

The per-source gains add up exactly: the candidates don't interact.

**What each candidate adds.** No candidate adds a Today job.

| Board | Actionable | Plausible | Strong | Today candidates |
| --- | ---: | ---: | ---: | ---: |
| greenhouse:elastic | 67 (all low priority) | 0 | 0 | 0 |
| ashby:clickhouse | 7 | 2: *Senior Consulting Engineer – AMER*, *Senior Curriculum Developer & Instructor* | 0 | 0 |
| greenhouse:gitlab | 7 (all low priority) | 0 | 0 | 0 |
| greenhouse:sourcegraph91 | 7 | 1: *Software Engineer – Platform [IC3]* (Remote, no scope: eligibility unclear) | 0 | 0 |

**Why Elastic is all low priority.**
- **61 of its 67** have the material contradiction "a large company, while
  you want small companies". That is the profile's own stated preference.
- **The other 6** are sales or management roles.
- **Its engineering roles are specialist:** Search Algorithms, Vector
  Search, Query Engine/Database Internals, and Java distributed systems
  for Elasticsearch Serverless.

That reading was not changed: ranking is frozen here. It explains why the
largest Brazil-eligible source in the batch adds no plausible job for
this person.

**What ClickHouse and GitLab add.**
- ClickHouse's two plausible jobs are customer-facing (consulting) and
  teaching (curriculum). Neither is backend, platform or infrastructure
  IC work.
- GitLab's 7 actionable jobs are sales, analyst, support, programme
  management, and a UK-scoped SRE role, all low priority.

**Scan time.** Full scans took 6–29 s per cell. As in BRU-343,
`lever:palantir` dominates that variance; C took 7.4 s against A's 7.8 s.
In production each board is one conditional request per schedule tier,
and all four supported conditional reads (`not_modified`).

## 5. Recommendation

**Activate `greenhouse:elastic` and `greenhouse:sourcegraph91`. Hold
`ashby:clickhouse` and `greenhouse:gitlab`.**

- **Elastic: activate,** for Brazil-eligible inventory, not for this
  profile's Today.
  - It nearly triples the corpus's engineering postings open to Brazil
    (32 → 92).
  - It is fresh (none over a year old, 13% over 90 days), healthy, and
    validated by its domain on every posting.
  - For the dogfood profile it adds only low-priority rows, mostly
    duplicated titles, held down by the person's own company-size
    preference.
  - Expect actionable to jump by 67 with nothing new above low priority.
    Don't read that as a quality gain.
- **Sourcegraph: activate.**
  - It is the only candidate whose plausible job is platform engineering.
  - It is tiny (10 postings), and its careers page points at the board.
- **ClickHouse: hold.**
  - Its plausible gains are customer-facing and teaching roles.
  - It is the stalest candidate (37% over 90 days).
  - Its geography is office and North America first, with 2 engineering
    postings open to Brazil.
- **GitLab: hold.**
  - Nothing above low priority.
  - 1 engineering posting open to Brazil.
  - Its weakest ownership tie: 40 of 209 postings name the domain.

**Applying it later** takes one change and one deploy:
- add two entries to `deploy/cloud.toml`;
- set their registry status to `active`, which the
  `the_source_registry_matches_the_cloud_source_list` test enforces;
- merge, which deploys;
- then verify the rollout as in BRU-343 §20.

**After Batch 2A, measure next:** the rest of §19 (automatticcareers,
resend, elevenlabs, fivetran). ElevenLabs had 14 postings open to Brazil
in BRU-343.

## Reproduce

```bash
# configs: deploy/cloud.toml's [discovery]/[verification] and sources (A),
# plus one candidate (B-*) or all four (C); DB: the real profile synced
# from production with `narrow login --token` + `narrow sync`, jobs deleted
N=target/release/narrow
$N --config C.toml --database C.db find --raw --refresh -n 1
target/release/examples/real_posting_probe C.toml C.db 2026-10-05T14:40:00Z > C.cell.json
$N --config C.toml --database C.db sources report --registry deploy/sources.toml --json
$N --config A.toml --database disc.db sources discover \
  elastic.co,Elastic clickhouse.com,ClickHouse gitlab.com,GitLab sourcegraph.com,Sourcegraph --json
```

The private databases and cell outputs are in `/tmp/bru344/` on the
machine that ran this. No profile data is committed. The token used for
the sync was deleted after it, and the session was signed out.
