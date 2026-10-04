# Experiment: role breadth as a fit concept, without a specialty taxonomy

Run date: **2026-10-03**. *Pre-registration, committed before the fresh
snapshot was fetched, the sample drawn or any breadth logic written.
Results are added below it without editing this part.*

## 1. Starting point

**PR #45** (`docs/role-vs-company-fit-experiment.md`) separated role fit
from company fit. Its reference implementation is
`t3code/role-vs-company-fit-impl`, not merged.

Reproduced today with that branch's binary on BRU-325's frozen B1 corpus
(PR #43 eligibility, reviewer off):

- Today: 5 companies · 16 jobs;
- strict precision on judged Today jobs: **5/14**;
- 3 judged No on Today;
- 8 practical Strong yes jobs, in 5 companies;
- 2 practical Strong-yes companies on Today.

It matches the report exactly.

**Remaining failure:** Narrow reads the role family (platform,
infrastructure, backend) correctly, but not whether the work is the
broad family or one narrow slice of it.

## 2. Phase 0 findings

- **Sound in the reference branch:**
  - the evidence `Scope` (role vs company);
  - the staged `classify` (role from role evidence, then company, then
    level);
  - `support` and `company_support` kept apart;
  - the `RoleBasis` of the role evidence;
  - the narrow readings ("II"/"IC3", duty bullets, a title's
    responsibility sentence).
- **Not to reuse as is:** the cross-posting `CompanyBook`. The feed and
  a single job's page read a posting differently, and it is a
  company-fit mechanism this experiment doesn't need.
- **Existing depth signals:** `facets::work::specialties_in`.
  - It is already a named-specialty taxonomy: 6 specialties (database
    internals, Kubernetes control-plane internals, model training,
    low-latency trading, privacy, security research) and about 100 cue
    phrases.
  - A specialty needs one title cue or two description cues.
- **Why it misses the remaining cases:**
  - its cues are exact phrases ("kubernetes operators", "custom
    controllers"), and Multigres says "the Multigres Operator", "custom
    resources", "Kubernetes internals": one cue;
  - storage, networking, release, cost, observability and test
    automation aren't specialties at all;
  - extending the list is what this experiment must not do.

## 3. Hypothesis

> A candidate-independent reading of how **concentrated** the actual work
> is (broad, focused, deep specialist, or unclear) can be identified
> generically, without naming specialties. Used as one role-local fact,
> it lets a person who wants broad roles keep focused and deep-specialist
> jobs off Today.

## 4. Fresh sample (Phase 1)

- **Snapshot:** one discovery-only fetch of the same 60 boards, into a
  fresh copy of the candidate database, frozen and hashed.
- **Population:** open postings that Narrow's existing deterministic
  facets read as software engineering IC work:
  - function is engineering;
  - not people management;
  - not customer-facing engineering.
- **Excluded:** the 239 postings judged in earlier experiments (BRU-325's
  50, the 2026-10-02 sample's 95, the 2026-10-03 sample's 94).
- **Strata,** first match wins, by the title's own role words:
  - backend, platform, infrastructure, SRE, security, data/ML, developer
    tooling, product, full stack, mobile, frontend, other engineering;
  - crossed with whether the title carries a qualifier after its role
    (", Storage", "- Edge & Networking", ": Observability") or none.

  Qualifier-bearing titles are oversampled, so narrower roles are
  present, but they are not selected by any specialty word.
- **Size:** about 100.
- **Order:** seeded hash of the job id (seed `role-breadth/1 2026-10-03`);
  at most 4 per company; a repeated company + title is drawn once.

## 5. Annotation protocol

- **Who:** one annotator (Claude).
- **What is read:** exactly the job-only text the probe dumps: title,
  department/team, content sentences.
- **When:** before any breadth logic exists. The annotations are
  committed before the first evaluation and never changed afterwards.
- **Labels,** describing the job only, never a candidate:

| Label | Meaning |
| --- | --- |
| `broad` | The responsibilities span several distinct areas of the role family: several systems or services, product areas, end-to-end ownership across a stack or platform. |
| `focused` | Legitimate engineering in the family, but most of the responsibilities center on one narrower technical concern. |
| `deep_specialist` | The work is centered in a narrow subsystem, implementation layer or technical primitive, and needs unusually deep expertise in it. |
| `unclear` | The text doesn't show the breadth of the responsibilities, or it is genuinely mixed. |

- Each label has a short reason and one or two short excerpts.
- Strict agreement uses the label alone; `also` lists an acceptable
  second label for a genuinely borderline job.

## 6. Evidence model (to be implemented after annotation)

Generic properties only. Named specialties can appear as evidence words,
never as categories.

- **Responsibility concentration:** whether the duty sentences (what the
  person will do, not the requirements) keep returning to one concern,
  or span several distinct ones.
- **Title scope:** a qualifier naming a part of the role family. Never
  decisive alone.
- **Implementation layer:** existing depth signals whose work is at a
  lower layer (internals, primitives), from responsibilities rather than
  qualifications.
- **Qualifications are weaker than responsibilities.**
- **Uncertain cases:** prefer `unclear` over narrowing a broad job.

## 7. Phase 1 gate (from the task, unchanged)

`narrow` means `focused` or `deep_specialist`.

| Metric | Definition | Bar |
| --- | --- | --- |
| Narrowness precision | of jobs Narrow marks narrow, share the annotation marks narrow | ≥ 90% |
| Narrowness recall | of jobs the annotation marks narrow, share Narrow marks narrow | ≥ 70% |
| Broad preservation | of jobs annotated `broad`, share Narrow marks `broad` or `unclear` | ≥ 95% |
| Deep cases | every annotated `deep_specialist` | never `broad` |
| Generality | no growing named-specialty list; no systematic family- or company-specific errors | required |

Also reported: the confusion matrix and the `unclear` rate.

**If the gate fails:** stop, report FAIL, integrate nothing, and don't
run Phase 2.

## 8. Phase 2 (only if Phase 1 passes)

- **Setup:** the same frozen corpus and judgments, and the reference
  role/company architecture. Breadth is added as one role-local fact.
- **For a person who prefers broad roles:**
  - `broad` supports the role;
  - `focused` holds a role back from Strong;
  - `deep_specialist` is a material role contradiction;
  - `unclear` is neutral.
- **Company fit can't compensate.**
- **Bar:**
  - Today strict precision ≥ 60%;
  - 0 deterministically rejectable judged No on Today;
  - all 8 practical Strong yes jobs kept, none poor;
  - all 5 companies with practical Strong yes jobs still in the strong
    pool;
  - the improvement comes from the separation and generic breadth
    semantics only.

---

# Results (added after the pre-registration)

Numbered R1–R18 to keep them apart from the pre-registration above.

## R1. Executive summary

**Outcome: FAIL.** Phase 1 failed, so **Phase 2 was not run** and
nothing is integrated. No production behavior, `FIT_RULES` or
`RANKING_VERSION` changes.

- **Phase 1 gate**, 74 scored postings out of 90 sampled:
  - narrowness precision: 4/5 (80%), bar ≥ 90%;
  - narrowness recall: 4/24 (17%), bar ≥ 70%;
  - broad preservation: 49/50 (98%), passes;
  - deep specialists read `broad`: 3/3, bar 0.
- **Why it fails:**
  - A focus is usually written in *varied* vocabulary, so no single term
    recurs: detection and response, identity, an SDK, i18n, performance,
    an inference launch.
  - Deep work is written in domain words, not generic depth words:
    learned trajectory models, a statistics engine.
  - Telling these apart generically would take semantic grouping of
    terms, which is a classifier, or a vocabulary of concerns, which is
    the specialty taxonomy. Both are out of bounds by design.
- **Today:** unchanged from the baseline, 5/14 (36%) strict, 3 judged
  No.
- **Recommendation:** stop Fit-rule iteration and change what Today
  promises (R18).

## R2. Previous result (role fit vs company fit)

From PR #45 (`docs/role-vs-company-fit-experiment.md`), reproduced
exactly in §1 above:

- Today: 5/14 strict;
- 3 judged No: Supabase *Multigres Deployment*, Railway *Storage*,
  Airbnb *Quality Engineering*;
- all 8 practical Strong yes jobs and 5 companies kept.

Every remaining No was the wanted family at a narrow slice. That one
mechanism is what this experiment tested.

PR #45 was still open when this task started. It was merged (7189db4)
and this branch starts from it. The implementation branch
`t3code/role-vs-company-fit-impl` was inspected (§2), not merged.

## R3. Breadth hypothesis

As registered in §3. It is **rejected for generic deterministic
evidence.** The text often does say how concentrated the work is, but
only in domain words.

## R4. Why no named-specialty taxonomy

- Narrow already has one (`specialties_in`: 6 specialties, about 100
  cues), and it misses every case that matters here (§2).
- Each miss would need new entries:
  - "detection and response", "SDK", "identity", "i18n", "learned
    trajectory", "experimentation statistics";
  - and from the frozen corpus, "operator", "block storage", "test
    automation".

  That is a list that grows with every sample, which the task forbids.
- The reader uses only language vocabularies:
  - function words and generic work words (`STOP`);
  - role-family names, so "platform" never counts as a concern;
  - 12 breadth phrases ("generalist", "across the stack");
  - 7 depth phrases ("internals", "low level", "primitives").
- It also reuses the existing `specialties_in` as one depth input, as
  pre-registered ("existing depth signals"). That does not help any of
  the misses.

## R5. Fresh sample methodology

As registered (§4), with these details:

- **Snapshot:** fetched 2026-10-03T02:58:55Z–02:59:06Z.
  - 60 sources, 0 failures, 4,643 open postings.
  - Corpus sha256 `fb25765e…4aabfb`; id/fingerprint manifest
    `2733c94e…1f02`.
  - Both are recorded in the fixture.
- **Population:** 1,442 engineering IC postings, after excluding the 239
  judged earlier.
- **Draw:** 23 strata. Quotas: 5 qualified, 3 plain, 8 other; 4 per
  company. The draw gave 90 postings.
  - Thin strata were drawn short: SRE qualified 1 of 5, developer
    tooling plain 2, mobile plain 2.
  - The size is under the pre-registered "about 100", but inside the
    task's 80–120.
- **Tooling:** `breadth_probe sample`, then `dump` for the job-only
  text.

## R6. Breadth annotations

Fixture: `crates/jobhunt-eval/fixtures/real-postings/fresh-2026-10-03-role-breadth.json`.

- **Committed in bbfae1a, before any breadth logic existed.** Never
  edited afterwards.
- **What each job carries:** the label, an optional `also`, a reason,
  and an excerpt of at most 120 characters. No full descriptions.

| Label | Jobs |
| --- | ---: |
| broad | 50 |
| focused | 21 |
| deep_specialist | 3 |
| out_of_scope (not SWE IC work the filter let through; unscored) | 16 |

**Convention:** breadth is judged within the job's own family. For
example, a detection-and-response role is focused security work, and a
security role spanning product, cloud and corporate security is broad.

## R7. Deterministic evidence model

`crates/jobhunt-eval/src/breadth.rs` (`jobhunt_eval::breadth::read`). Not
wired into ranking.

1. **Duties.**
   - Duties are the lines under a responsibilities heading or lead:
     "What you'll do", "In this role, you will:", "What will you work
     on?".
   - The section ends only at a requirements, about-us or benefits
     heading, so subheadings stay inside it.
   - Without such a section, the duties are the sentences addressed to
     the person ("you'll …").
   - Requirements are never duties.
2. **Concentration.** One content term in at least 50% of the duties,
   and in at least 3 of them.
   - Function words, generic work words, role-family names and the
     company's name never count.
   - A plural counts as its singular.
3. **Labels, in order:**
   - concentrated, with a depth phrase or an existing specialty in the
     duties: `deep_specialist`;
   - a breadth phrase: `broad`;
   - concentrated: `focused`;
   - 4 or more duties, not concentrated: `broad`;
   - otherwise: `unclear`.
4. **Title scope** is not used. Alone it reads the wrong thing (R17).

**Unit tests (3):**

- the duties come from the responsibilities section;
- a recurring concern is `focused`, and variety is `broad`;
- depth with concentration is `deep_specialist`.

## R8. Phase 1 metrics

- **Run 1** was the first implementation.
- **Run 2** fixed implementation bugs only, with no threshold or
  vocabulary-of-concern change:
  - curly apostrophes ("you’ll") were not recognized;
  - "?" headings were not read as headings;
  - any subheading closed the duty section;
  - "wide range of" was matched anywhere in the text, not only in the
    duties' context.
- **Run 2 is the result.** Run 1 is shown so the fixes are visible.
- **Readings:** run 2's readings are in
  `fresh-2026-10-03-role-breadth-readings.json`, next to the annotations.
  Reproduce with `breadth_probe run`, then `breadth_probe score`.

| Metric | Run 1 | Run 2 | Bar |
| --- | ---: | ---: | --- |
| Narrowness precision | 6/8 (75%) | 4/5 (80%) | ≥ 90% |
| Narrowness recall | 4/24 (17%) | **4/24 (17%)** | ≥ 70% |
| Broad preservation | 46/50 (92%) | 49/50 (98%) | ≥ 95% |
| Deep read as `broad` | 3/3 | **3/3** | 0 |
| `unclear` | 9/74 | 5/74 | — |
| Strict agreement | 47/74 | 50/74 | — |

**Confusion, run 2** (rows: annotation; columns: reading):

| | broad | focused | deep | unclear |
| --- | ---: | ---: | ---: | ---: |
| broad (50) | 46 | 1 | 0 | 3 |
| focused (21) | 15 | 4 | 0 | 2 |
| deep_specialist (3) | 3 | 0 | 0 | 0 |

**Caught:**

- ClickHouse *Efficiency Engine*;
- Chainguard *Sustaining Automation*;
- Chainguard *Platform Database*;
- Spotify *Content Platform* (annotation allows focused).

**Missed narrow jobs**, with the reading and duty share. The share is the
duties carrying the most frequent term, out of all duties.

| Job | Annotation | Reading | Why missed |
| --- | --- | --- | --- |
| Zoox *Learned Trajectory ML* ×2 | deep | broad (2/4) | depth is in domain words; 4 duties, none concentrated |
| LaunchDarkly *Staff Engineer, Experimentation* | deep | broad (3/7) | statistics-engine depth, no generic depth phrase |
| Modal / Vercel / Notion *Detection and Response* | focused | broad | one concern written as detection, alerting, incidents, investigations, threat hunting |
| Chainguard *Product Security* | focused | broad | as above |
| DuckDuckGo *Web Security, Browser Platform* | focused | broad | "any part of" read as breadth |
| Vanta *Identity* | focused | broad (3/41) | the section over-captures a long posting |
| Grafana *Loki*, *DataViz*, *iOS SDK* ×2 | focused | broad (3/21–24) | one product area, varied wording; long sections |
| Figma *Developer Experience* | focused | broad | breadth phrase plus 36 captured lines |
| Spotify *Fullstack* (i18n) | focused | broad (2/7) | i18n written as translations, locales, languages |
| ClickHouse *Cloud Performance* | focused | broad | "performance" is a generic word, rightly never a concern |
| Anthropic *Cloud Inference Launch* | focused | broad (6/37) | over-captured section |
| Zoox *Mapping Web* | focused | broad | "wide range of" |
| Canonical *Python Cloud graduate*, *Embedded IoT* | focused | unclear | 2 addressed sentences only |

**Broad job narrowed:** Spotify *Backend, Subscriptions* was read
focused on "squad" (3/4). That is a team-structure word, not a concern.

## R9. Phase 1 gate

| Criterion | Result |
| --- | --- |
| Narrowness precision ≥ 90% | **FAIL**: 80% |
| Narrowness recall ≥ 70% | **FAIL**: 17% |
| Broad preservation ≥ 95% | PASS: 98% |
| No deep specialist read `broad` | **FAIL**: 3/3 |
| No named-specialty list | held, so the reader is generic and doesn't work |

**Gate: FAIL.** Per §7: stop, integrate nothing, don't run Phase 2.

The failure is not one of tuning. A post-hoc sweep of the concentration
threshold uses the same readings, ignoring the breadth and depth phrases.
It was not used for any decision.

| Share ≥ | Min duties | Precision | Recall | Broad narrowed |
| --- | ---: | ---: | ---: | ---: |
| 50% | 3 | 6/7 | 6/24 | 1/50 |
| 40% | 3 | 10/13 | 9/24 | 4/50 |
| 30% | 3 | 12/16 | 11/24 | 5/50 |
| 25% | 2 | 20/39 | 14/24 | 25/50 |
| 15% | 2 | 24/50 | 16/24 | 34/50 |

- **No setting reaches 70% recall.** Long before recall gets near it,
  half the broad jobs are narrowed.
- **Collapsing synonyms** ("detection", "alerting", "incident") into one
  concern is the semantic grouping the task excludes.

## R10. Phase 2 methodology

**Not run.** The gate failed. The registered plan stays in §8 for
reference.

## R11. Today before and after

**No change**, since nothing was integrated:

- 5 companies, 16 jobs;
- strict precision 5/14 (36%);
- judged No on Today: 3 (Supabase *Multigres Deployment*, Railway
  *Storage*, Airbnb *Quality Engineering*);
- 2 practical Strong-yes companies on Today.

## R12. Known Strong yes guards

Unchanged:

- all 8 practical Strong yes jobs are kept, in 5 companies (§9 of PR
  #45's report);
- none is poor;
- Supabase *Branching* stays reported separately.

**Diagnostic only.** The reader's output did not feed ranking. Reading
the frozen corpus's 8 Strong yes postings:

| Strong yes | Reading |
| --- | --- |
| Supabase *Compute Capacity* | broad (8/19) |
| Supabase *Supalite* | broad ("generalist") |
| Railway *Full-Stack - Product* | broad |
| Railway *Infrastructure Engineer* | broad |
| Railway *Product Engineer, Scalability* | broad |
| Socket *Senior Platform Engineer* | broad |
| Oyster *Senior Engineer (Platform)* | broad |
| PostHog *Product Engineer* | **focused**, on "handbook" (3/5) |

If breadth had been integrated, one Strong yes would have been held
back by a word about the company's handbook, not about the work.

## R13. Supabase breakdown

The ranking is unchanged from PR #45 (its §10):

- 8 Supabase jobs on Today: 2 Strong yes, 5 Maybe, 1 No;
- *Multigres Deployment* is still the No.

**Diagnostic reading of Supabase and the other Today jobs:**

| Today job | Judged | Reading |
| --- | --- | --- |
| Supabase *Multigres Deployment* | **No** | focused, "deployment" (5/6). Read focused, not deep, even though its depth is operator internals. |
| Supabase *FinOps* | Maybe | focused, "cost" (9/14) |
| Supabase *Edge & Networking* | Maybe | broad (3/11) |
| Supabase *Platform Security* | Maybe | broad (4/17) |
| Supabase *Branching* (reported separately) | Strong yes | broad |
| Railway *Storage* | **No** | broad (3/8) |
| Railway *Baremetal Orchestration*, *Observability* | Maybe | broad |
| Airbnb *Quality Engineering* | **No** | broad (8/31) |
| Airbnb *Service Tools* | Maybe | broad |
| Zapier *Commerce* | post-hoc Maybe | broad |
| Wikimedia *MediaWiki Content Platform* | not in set | unclear |

Of the 3 No, it catches 1 (Multigres), and as `focused`, not deep. It
also misses 2 and, as R12 shows, holds back one Strong yes. That is the
fresh sample's pattern again, on the frozen corpus.

## R14. Remaining false positives

The same 3 judged No as before: Multigres (Kubernetes operator work),
Railway *Storage* and Airbnb *Quality Engineering* (test automation).

- **Most Maybes are narrow slices of the wanted family:**
  - Supabase networking, platform security and FinOps;
  - Railway bare-metal orchestration and observability.
- A breadth reading would have had to separate them. The diagnostic
  above shows it does not.

## R15. Benchmark results

- **BRU-320** (`recommendation_benchmark`), regenerated with
  `JOBHUNT_UPDATE_BENCHMARK=1`: 21 passed, and the baseline is
  byte-identical.
- **Fresh breadth eval:** R8.
- **Frozen BRU-325 + #43 eval:** run as the baseline reproduction (§1),
  5/14. No variant to compare, because Phase 2 did not run.

## R16. Test expectation changes

**None.**

- **Added:** the 3 unit tests of the offline reader (R7).
- **Unchanged:** no production test, fixture expectation or benchmark
  baseline.

## R17. Generalization sanity check

**Errors by family** (narrow annotated → found; broad annotated →
narrowed):

| Family | Narrow found | Broad narrowed |
| --- | ---: | ---: |
| security | 0/4 | 0/3 |
| infrastructure | 1/4 | 0/4 |
| mobile | 0/3 | 0/3 |
| other engineering | 1/3 | 0/3 |
| platform | 2/2 | 0/6 |
| backend | 0/2 | 1/6 |
| data/ML, frontend | 0/2 each | 0 |
| full stack, developer tooling | 0/1 each | 0 |
| product, SRE | — | 0 |

- **The failure is systematic, not sampling noise.** Recall is near zero
  in every family except platform.
- **By company:** Grafana (4 misses) and Zoox (3) write long or
  domain-worded duty lists.
- **Title scope alone is no substitute.** Reading a title qualifier as
  "narrow" (", Identity", " - Loki") gives:
  - precision 16/46 (35%) and recall 16/24 (67%);
  - 30/50 (60%) broad jobs narrowed.

  Qualifiers name teams and products far more often than slices.
- **The excluded fix fits these misses only.** A named-specialty list
  could close them, but it would only fit this sample. The next sample
  brings new names, as the frozen corpus already does (operator, block
  storage, test automation).

## R18. Conclusion

## FAIL

Breadth can't be identified generically. With the specialty vocabulary
held out, as required:

- recall is 17%;
- every deep specialist reads broad;
- precision is below the bar.

Nothing was integrated, and Today is unchanged at 5/14.

**Recommendation: stop Fit-rule iteration.** This is the eighth line
investigated:

- candidate intent;
- source coverage;
- eligibility;
- semantic candidate fit;
- semantic job function;
- binary engineering gating;
- role vs company fit;
- breadth.

Each fixed what it targeted. None moves strict Today precision to 60%,
because the remaining errors are distinctions inside the wanted family
(generalist infrastructure vs one infrastructure slice). A reader of the
text can't make that distinction without knowing the specialties.

**Reconsider the Today promise, not the rules.** This is an analysis, not
a proposal to build anything here:

- **The current promise** is "these are strong fits". At ≥ 60% strict it
  needs that distinction, and Narrow can't make it reliably.
- **What Narrow can already show reliably:**
  - Practicality (eligibility);
  - the role family;
  - real role evidence vs company evidence (PR #45's architecture);
  - recall of the good jobs: all 8 practical Strong yes jobs, in 5
    companies.
- **A Today that keeps its promise:** "jobs worth reviewing today", with
  the reasons visible:
  - why the role matches;
  - what the company evidence is;
  - why it is practical.

  Precision is then judged as "worth a look". Counting Maybe as worth a
  look, today's judged Today jobs are 11/14 (5 Strong yes, 6 Maybe).
  The 3 No are the slices above.
- **Where the specialty distinction belongs:** with the person, as a fast
  dismissal ("not storage", "not test automation") that shapes later
  days. It does not belong in more deterministic rules.
- **What would need deciding:**
  - whether Today's metric becomes "worth reviewing" plus "no rejectable
    No", rather than strict Strong-yes precision;
  - whether the dismissal feedback Narrow already records (opportunity
    feedback) is the intended way to learn a person's narrower wants.
