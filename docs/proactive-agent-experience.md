# The proactive Narrow agent: lightweight onboarding and WhatsApp delivery (BRU-389)

Design and research, 2026-10-11, against `main` at `176c84b` (BRU-360
merged). Nothing here is built. No ranking rule, Fit semantics, Today
threshold, eligibility rule or deployment changes. There is no WhatsApp
code, no auto-apply, and no generic chat.

The question is what Narrow becomes next:

> **From** a dashboard you open to look at jobs,
> **to** an agent that keeps watching first-party job boards and
> interrupts you only when a job is worth it.

Every claim is labelled with what it rests on:

| Label | Meaning |
|---|---|
| **Code** | Read in this repository, with a `path:line` reference. |
| **Doc** | Narrow's own measured documents (experiments, design docs). |
| **Source** | A current public page, cited in [Sources](#sources), fetched 2026-10-11. |
| **Inference** | My reading of the above. Not verified. |
| **Recommendation** | The proposal. |

---

## 1. Executive decision

1. **Yes, become proactive.** But the proactive channel is a delivery
   channel *into* the canonical Today queue, not a second product.
   - One ranking, one recommendation state, one set of actions.
   - A message is a pointer to a recommendation that exists in Narrow.
2. **Build on what already exists.** Narrow already has a proactive
   channel: email notifications (BRU-295, `crates/jobhunt-cloud/src/notify.rs`).
   - It has an outbox, idempotent sends, leases and per-opportunity
     dedupe.
   - Generalize it to channels. Don't build a second notification system.
3. **WhatsApp is the first new channel, through Meta's Cloud API directly,
   not Twilio.**
   - Twilio adds $0.005 per message in each direction and says it requires
     Meta business verification before production. It buys nothing the
     existing outbox doesn't already do.
4. **The notification policy goes live on email first.**
   - Email already works and costs nothing per message.
   - WhatsApp job alerts are *Marketing* messages: about $0.0625 each in
     Brazil, under per-user marketing caps. That makes WhatsApp the wrong
     place to debug a selection policy.
5. **The default is one digest a day, not alerts.**
   - At most 3 jobs (5 if the person asks), one per company.
   - At the person's local time, on weekdays, never in quiet hours.
   - **Nothing is sent when nothing qualifies.**
6. **Immediate alerts are built but off.**
   - On real postings, today's "Strong fit" is strictly right only about
     23% of the time (Doc: BRU-325).
   - "Worth a look" precision has never been measured on real Today jobs,
     because BRU-328 is paused.
   - That doesn't justify interrupting someone the moment a job appears.
   - Alerts turn on for a person only after delivered digests reach about
     80% "worth the interruption" over at least 30 judged jobs, the
     BRU-328 bar.
7. **"New" needs evidence.**
   - A job counts as fresh only from a dated source (`posted_at`) or
     because Narrow saw it appear between two complete scans of a board it
     was already monitoring.
   - Jobs read when a board is first added are a backfill, never "new".
8. **Onboarding becomes about 3–4 minutes with 5 required screens:**
   resume (with one seniority confirmation), kind of role, where you can
   work, pay floor, and how to reach you. One optional screen asks what to
   avoid.
   - The Taste Profile, fine-tune controls and free-text description leave
     onboarding.
   - The internal model stays the same.
9. **Company preferences are asked only as avoids.**
   - In the current ranking, any firm or soft *prefer* on company, team,
     ownership or culture requires company evidence for a Strong fit
     (`fit.rs:765-776`).
   - A one-tap "I prefer startups" would therefore quietly shrink
     inventory, so preferences are learned instead.
10. **Add company hiding.** "Never show me Nango" becomes a visible,
    reversible, per-person suppression.
    - It doesn't remove Nango as a source for anyone else.
    - It doesn't feed learned taste.
11. **Learned rules stay explicit.**
    - Narrow suggests a hide rule ("Hide roles mentioning “Storage”?")
      only after an explicit reason or a repeated pattern.
    - It never creates one silently (BRU-328 §10).
12. **Application capability is a first-class, honest property.**
    - Every verified job has **External** ("Apply on company site").
    - **Prepare** comes next: a kit of standard answers plus, for
      Greenhouse, the job's real questions.
    - No ATS offers a documented candidate-side submission path.
      **"Apply with Narrow" must not appear** until a partner agreement
      makes it true.
13. **Inventory is a precondition, not part of this work.**
    - Production reads 21 boards (`deploy/cloud.toml`).
    - BRU-360 showed recommended boards taking strong fits from 4 to 14 and
      Today from 1 to 5 companies.
    - Without activating those boards, the digest will mostly be empty.
      That is correct behavior, but nothing would be learned from it.

---

## 2. Product thesis

**Broad discovery changed which problem is hard.**
- Narrow can now find thousands of first-party boards. BRU-360 found
  7,968 and validated 2,261.
- The scarce resource is no longer jobs. It is the person's attention.
- So the product is a *filter with a voice*:
  - it searches continuously;
  - it stays quiet by default;
  - it speaks when something is worth about 30 seconds.

**The thesis, stated so it can fail:**

> A person who configures Narrow once, and mostly never opens the
> website, receives a few jobs a week. Most of them are worth the
> interruption. They act on them (save, dismiss, apply) from the message
> itself. Their actions make later messages better.

**It fails if any of these happen:**
- Delivered jobs are mostly not worth the interruption. Measure it (§24).
- Inventory is too thin to deliver anything. Measure it: digests sent per
  active user-week.
- People mute the channel. Measure it: opt-outs, blocks.

**What doesn't change:**
- Narrow is high-signal, finite and honest. "Caught up" is a feature.
- The local CLI product keeps working without the cloud. Proactive
  delivery is cloud-only, as email notifications already are.

---

## 3. Current Narrow flow

### What a person does today

```text
sign in ─▶ /welcome (5 steps) ─▶ /today (strong fits, one per company, ≤5)
                                   │ Not now · Not for me · I applied · Save
                                   ▼
                     /applications (saved → applied → interviewing → offer)
optional: Settings → "Email me about strong new matches" (immediate ≤ every 4h | daily)
```

### Onboarding

**Code:** `apps/web/src/app/(app)/welcome/page.tsx:54-194`.

1. **Your career**: upload a resume.
   - Parsed deterministically, never by an AI service.
   - Shows "N roles found".
2. **"What kind of role are you looking for?"**
   - Up to 3 of 12 role families plus an optional title (BRU-324).
   - Stored as stated, confirmed `work_shape` taste assertions.
3. **"Anything else you care about?"**
   - Optional free text, read into Taste Profile assertions with "Looks
     right / Edit".
   - The skip is only a URL parameter and is not persisted.
4. **Practical constraints**:
   - Work setup (5 options) and Relocation.
   - Where you live, Authorized to work in, Remote roles open to.
   - When eligibility is unclear.
   - Clarify questions for location and pay.
   - **Pay is not on this step.** Pay controls exist only in
     `/preferences`.
5. **Go to Today.**

### Today

**Code:** `crates/jobhunt-app/src/feed.rs`.

**Who qualifies:**
- Only strong fits, and only if not excluded (`qualifies_for_today`,
  `:138`).
- `VerifyFirst` and `EligibilityUnclear` strong fits *can* appear.
- Plausible fits ("worth reviewing") never do.

**How it is selected:**
- The best candidate per company, until 5 companies (`one_per_company`,
  `:149`). The maximum is 10.
- A job is "new" until 24h after it was first shown (`HOLD_HOURS`).
- After that it is "passed over".
- It comes back only on a material change: pay, location, authorization,
  employment type, or a reopening.
- `record_shown` writes what was shown to `user_opportunities`.

### Actions

**Code:** `crates/jobhunt-ranking/src/feedback.rs:102-341`.

**The state model:**
- An append-only `feedback` log with 9 actions.
- State (`unseen, seen, saved, rejected, applied, interviewing, offer`)
  is replayed from it.
- A repeat is a no-op (`service.rs:266-317`).

**"Not now":**
- It is `dismiss`: a weightless `seen` event plus
  `user_opportunities.dismissed_at`.
- It lasts until a material change, not for a set time.

**"Not for me":**
- The reason is free text, stored verbatim and encrypted.
- The web suggests 8 chips that append text.

### Email notifications

These already exist (BRU-295): `crates/jobhunt-cloud/src/notify.rs`,
`crates/jobhunt-storage/src/postgres/notify.rs`, migration
`20261003000000_feed_notifications.sql`.

**Candidates** (`App::notification_candidates`, `feed.rs:614-693`):
- `StrongFit` and `Gate::Recommended`, meaning verified recently at an
  authoritative source, and eligible or conditionally eligible.
- `Stage::Unseen`.
- Never shown in the app or dismissed (no `FeedMark`).
- Never notified before.
- Rank order. **No one-per-company rule.**

**Limits:**
- At most 3 items per email (`JOBHUNT_NOTIFY_MAX_ITEMS`, capped at 5).
- At most one email per 4h with `immediate`, or per day with `daily`.
- Cron runs every 15 minutes (`.railway/railway.ts:122`).

**Delivery:**
- An outbox: `notification_deliveries` holds the rendered message, sealed.
- `notification_items` has primary key `(user_id, opportunity_id)`, so an
  opportunity is notified **at most once, ever**.
- The delivery id is the provider's idempotency key. Retries back off;
  a delivery is abandoned after 20h.

**What it lacks:**
- Timezone and quiet hours.
- Channels other than email.
- Per-item actions (links go to the signed-in web app).
- Delivered/read status.
- An unsubscribe token (`List-Unsubscribe` points at `/settings`).

**It doesn't consume Today.** Notifying does not call `record_shown`, so a
job that was emailed is still "new" on Today.

### Inventory and quality

**Inventory:**
- Production reads **21** boards (`deploy/cloud.toml`).
- BRU-360's dogfood with the 144 recommended boards: strong fits 4 → 14,
  Today 1 → 5 companies (Doc: `broad-discovery-experiment-2026-10-05.md`
  §6). None of the 144 is activated yet.

**Quality** (Doc: `real-posting-precision-experiment.md` §1,
`role-vs-company-fit-experiment.md`, `role-breadth-fit-experiment.md`):
- Strict precision of "Strong fit" on real postings with the rules
  `main` runs (fit-rules/2): **3/13 (23%)** and 8/33 (24%). Today: 2/10.
- With the unmerged fit-rules/3, "Strong yes + Maybe" reached 11/14
  (79%).
- The Fit-improvement line is closed. Today's promise is being reframed
  as "worth reviewing" (BRU-328, paused before its first judgment).

---

## 4. Proposed agent-first flow

```text
configure once (≈3–4 min) ─▶ Narrow watches first-party boards (existing workers)
        │
        ▼
 ranking (unchanged) ─▶ Today = the canonical review queue (unchanged selection)
        │                     ▲                    ▲
        ▼                     │ same state         │ same state
 notification policy ─▶ digest (daily, ≤3) ──┐     │
 (selection only)     └▶ alert (rare, flag) ─┤     │
                                             ▼     │
                         channel: email │ WhatsApp │ (later: push, Telegram…)
                                             │
                     person taps Save / Not for me / Not now / opens job
                                             │
                                             ▼
                existing feedback use cases (same log, same repeat rule)
                                             │
                                             ▼
              learning: existing taste patterns + explicit hide rules + hidden companies
                                             │
                                             ▼
                 application: External (always) ─▶ Prepare (next) ─▶ Assisted (only via partners)
```

**What the person experiences:**
- **Day 0:** onboarding ends with "Narrow will message you on WhatsApp on
  weekdays at 9:00 when there's something worth your time".
- **Most weekdays:** one message with 1–3 jobs, or nothing.
- Each job can be saved, dismissed or opened from the message.
- The website is for reviewing (Today), keeping track (Applications), and
  steering (Preferences).

---

## 5. Onboarding audit

**Code** sources for every row: `crates/jobhunt-profile/src/preferences.rs`
(`PreferenceValue`, `:341-417`), `taste/*`, `model.rs`, the web
components named.

### How the columns are scored

| Column | Meaning |
|---|---|
| E/I | Explicit or inferred today |
| Onboarding? | Is it actually necessary to produce a useful first feed? |
| Resume? | Can it be inferred from the resume? |
| Later? | Can it be learned later? |

| Input / signal | Current source | E/I | Onboarding? | Resume? | Later? | Risk if inferred wrongly |
|---|---|---|---|---|---|---|
| Resume (experiences, skills, claims) | `/welcome` step 1 upload; deterministic parser (`jobhunt-resume`) | extracted | **Yes**: career evidence for fit and seniority | n/a | LinkedIn/GitHub imports add more | Low: claims are not used until usable (`context.rs` policy) |
| Seniority (taste `seniority`) | words, or latest title at medium confidence (`taste/rules.rs`) | inferred | **Confirm only**: one prefilled choice | **Yes**, from the latest title | yes (seniority rejections) | **High**: staff vs mid rejects whole classes of jobs |
| Kind of role (`work_shape`, BRU-324) | `/welcome` step 2 chips (`roles.rs:38-51`, max 3) | explicit | **Yes**: without it nothing is Strong (Doc: BRU-323/324) | partly (`roles::demonstrated`, what was done, not what's wanted) | refined by feedback | **High**: wrong family means every result is wrong |
| Target title (`other:target_role`) | step 2, optional | explicit | No | no | — | None: ranking doesn't read it |
| Current location | step 4 "Where you live"; resume header fallback | explicit (+ resume) | **Yes** (prefilled) | **Yes**: resume header | no | **High**: eligibility (`rules::presence`, `geography`) |
| Work authorization | step 4 "Authorized to work in" | explicit | **Yes** (prefilled with country of residence, confirmed) | weakly (residence) | no | **High**: impossible jobs |
| Sponsorship needed | free text / CLI only | explicit | No | **derive**: needed outside authorized places | no | Medium; authorization already covers the common case |
| Work mode | step 4 "Work setup" (5 options, `WorkSetupInput`) | explicit | **Yes** (3 options) | no | yes for soft preference | **High** if remote-only is missed |
| Relocation | step 4 | explicit | **Yes** (one tap) | no | no | **High**: on-site abroad is impossible without it |
| Remote regions ("Remote roles open to") | step 4 | explicit | **No** | **derive**: eligibility already computes "remote roles open to you" from residence and authorization | yes | Low: eligibility decides the hiring scope |
| When eligibility is unclear (show/hide) | step 4 | explicit | **No**: default "show, marked" | — | — | Low |
| Pay minimum | `/preferences` only (`preference-controls.tsx:1004`) | explicit | **Yes** (optional "no minimum") | no | no | **High**: below-floor jobs waste interruptions; currency is never assumed |
| Pay target | `/preferences` | explicit | No | no | — | Low: ordering only |
| Unknown pay (show/hide) | `/preferences` | explicit | **Yes**, as one sub-choice of pay | — | — | Medium: hiding unknown pay removes much of the inventory |
| Company stage/size (`CompanyTrait`) | free text; Fine-tune "Company & team" | explicit | **Only as avoids** (§6) | no | **yes** | **Prefer: high.** A firm or soft prefer makes company evidence mandatory for Strong (`fit.rs:765-776`). Avoid: low, acts only on evidence |
| Team size / ownership | words; Fine-tune | explicit or interpreted | No | weakly (ownership claims) | **yes** | Same as company: a prefer gates Strong |
| Work style (IC/management, on-call, greenfield…) | words; "Add a preference" | explicit or interpreted | **Only "avoid management", "avoid heavy on-call"** as optional chips | partly (title) | yes | Medium |
| Intensity / pace (`fast_paced`, `process_heavy`) | words only | interpreted | **Only as an optional avoid** | no | yes | Medium |
| Technology preferences | words only (taste `technology`); no control | interpreted | No | **yes** (skills are evidence, not wants) | yes | Low: Fit treats technology as soft |
| Domain preferences / avoids | words; "Add a preference" | explicit or interpreted | **Only avoids**, as optional chips (crypto, gambling…) | weakly | yes | Low/Medium |
| Taste Profile assertions | `/welcome` step 3 words → interpreter; `/preferences` | interpreted, confirmed | **No**: the internal model stays, the UI leaves onboarding | some (`origin=profile`) | yes (`learned`) | Varies; unreviewed readings are weaker |
| Free-text "what you want" (`TasteBrief`) | step 3 | explicit words | **No** (moved to Preferences) | — | — | None; it was optional |
| Clarify questions (pay currency, must or nice) | steps 3 and 4 | explicit | Only when a typed value is ambiguous, which structured inputs avoid | — | — | — |
| Claim review | `/profile/review` | explicit | No | — | — | None: unconfirmed claims are withheld |
| Timezone (job overlap) | Fine-tune "Time zone" | explicit | No | **derive** from residence (`ProfileFacts::zone`) | — | Low |
| Engagement (employee/contract) | CLI/API only | explicit | No | — | yes | Low |
| Start date / notice | **not modelled** | — | **No**: ask when preparing an application | no | — | None for discovery |
| Company blocklist | **not modelled** | — | **Optional** ("specific companies to skip") | no | **yes**, from "Not for me → This company" | Low; explicit |
| Phone / channel address | resume contacts only, unused (`model.rs:259`) | — | **Yes**, for the agent: the channel step (§6) | the resume's phone is never used to message | — | **High** (consent): never message an unconfirmed number |
| Timezone (for notifications) | **not modelled** | — | **Yes**, prefilled from the browser | — | — | Medium: a 3 a.m. message |
| Feedback actions and reasons | Today / Applications / MCP | explicit | — | — | **this is the learning** | — |
| Learned taste patterns | `ranking/src/taste.rs` (3–5 behaviors or 1–2 reasons) | learned | — | — | yes | Soft: they only reorder within a tier |

**What the audit says:**
- The internal model is rich and correct to keep.
- The *onboarding* problem is not too many fields. It is:
  1. a free-text step whose value overlaps the role chips;
  2. a location step with six controls, two of which eligibility can
     derive;
  3. pay missing entirely;
  4. no notion of how or when to reach the person.

Two smaller defects were found along the way:
- **The "Tell Narrow what you want" notice:**
  - `Person::has_preferences` (`person.rs:387`) counts only structured
    preferences.
  - Chosen roles are taste assertions.
  - So someone who chose roles but set no structured constraint (pay,
    work mode, remote regions) still sees the notice.
- **"Based in" on `/profile`** shows the resume header rather than the
  stated location (`profile_view.rs:372`).

---

## 6. Proposed minimal onboarding

**Design rules:**
- Prefill everything that can be prefilled.
- Ask only hard constraints and the one thing ranking cannot work
  without (the kind of role).
- Keep each screen to one question group.
- Answers are stored in the **existing** records, so ranking and
  eligibility are unchanged.

### Screen 1: "Start with your resume" (about 60 s)

- Upload (PDF, .txt, .md). The help text stays: "Read on Narrow's
  server, never by an AI service."
- After parsing, one confirmation and one read-only line:

  ```text
  Your next role's level:   [ Mid ] [● Senior ] [ Staff+ ]      ← prefilled from "Senior Backend Engineer"
  We read: 6 roles · 2016–now · Go, PostgreSQL, Kubernetes      (Fix later in Profile)
  ```

- **Stored as:** taste `seniority` prefer, `origin=stated`,
  `review=confirmed`. It is the person's choice, not an inference. The
  default is the inferred level.
- **Why only seniority:**
  - It is the one resume inference whose error makes jobs materially
    undesirable.
  - It is also the one the resume supports well.
  - Everything else read from the resume stays evidence, not
    preference.

### Screen 2: "What kind of role are you looking for?" (about 20 s)

- Unchanged from BRU-324:
  - up to 3 of Backend, Platform, Infrastructure, Product engineering,
    Full stack, Frontend, Mobile, Developer tooling, SRE, Security,
    Data, ML product;
  - plus "A title in your words (optional)".
- Hint: "Your experience shows Backend, Platform: that's what you've
  done; choose what you want next."
- **Stored as:** `TasteAction::SetRoles` (existing).

### Screen 3: "Where can you work?" (about 45 s)

```text
Where do you live?                 [ São Paulo, Brazil        ]   (from your resume)
Where are you authorized to work?  [ Brazil ✕ ] [ + add ]          (we assumed where you live — check it)
How do you want to work?           (● Remote only) ( Remote or hybrid ) ( On-site is fine too )
Would you relocate for a job?      (● No) ( Yes ) ( Only to… [          ] )
```

**Stored as** existing records:
- `CurrentLocation`.
- `WorkAuthorization` per place.
- `WorkSetupInput::{RemoteOnly, HybridOkay, OnsiteOkay}`.
- `Relocation`.

**Dropped from this screen, still in Preferences:**
- "Prefer remote" and "No preference" setups.
- "Remote roles open to": eligibility already derives which remote
  hiring scopes include the person.
- "When eligibility is unclear": the default is to show, marked.

**The four concepts stay distinct:**
- residence;
- authorization;
- mode;
- relocation.

No single "Location: Remote" field.

### Screen 4: "What's the least you'd accept?" (about 30 s)

```text
Minimum pay   [ 120,000 ]  [ USD ▾ ]  per [ year ▾ ]          ( No minimum )
Currency is required: Narrow never converts or guesses one.

Jobs that don't publish pay:
  (● Show them, marked "pay not published")   ( Hide them )
```

**Stored as:**
- `PreferenceValue::Compensation { bound: Minimum, amount, currency, period }`.
- `UnknownPay { show }`.

**How a minimum behaves** (existing semantics, stated on the screen as
help text):
- Verified pay below the minimum: the job is excluded (`BelowMinimum`).
- Unknown pay never counts as meeting the minimum: it is shown with a
  check, or hidden if chosen.
- Different currencies or periods are not compared:
  - "pay in a different currency isn't compared";
  - no conversion exists (`signals.rs:1465`);
  - this is a known gap, worth a separate task.

**Hourly or monthly** is allowed through the period selector, which also
covers contractors paid per hour.

### Screen 5 (optional): "Anything Narrow should avoid?" (about 30 s, skippable)

```text
[ Management roles ] [ Consulting or agencies ] [ Large companies ] [ Very early-stage startups ]
[ Heavy on-call ] [ Crypto / Web3 ] [ Very intense pace ]
Specific companies to skip:  [ Nango, …            ]
Something else:              [ e.g. "no adtech, no test automation" ]
```

| Choice | Stored as (existing unless noted) |
|---|---|
| Management roles | taste `work_style:management` avoid, `stated`, firm |
| Consulting or agencies | taste `company:consulting` and `company:agency` avoid |
| Large companies | taste `company:large_company` avoid |
| Very early-stage startups | taste `company:early_stage` avoid |
| Heavy on-call | taste `work_style:on_call` avoid |
| Crypto / Web3 | `PreferenceValue::Domain` `Unwanted` (crypto) |
| Very intense pace | taste `culture:fast_paced` avoid |
| Specific companies | **new:** hidden companies (§17) |
| Something else | the `TasteBrief` words, read by the existing interpreter (`describe_taste`) |

**Why avoids and not preferences:**
- An avoid acts only when the posting carries evidence: a material
  contradiction leads to Poor.
- A firm or soft *prefer* on company, team, ownership or culture sets
  `needs_company_fit`. Every Strong then needs company support ≥ 0.6
  (`fit.rs:381-400`, `765-776`).
- Postings often lack company evidence. So a one-tap "Early stage"
  preference would silently remove Strong fits at companies that never
  stated their stage.

So the competitor's "company stage" and "startup intensity" questions
become optional avoids now, and preferences learned later (§18). If the
product wants prefers at onboarding, that is a ranking decision, outside
this design.

### Screen 6: "How should Narrow reach you?" (about 60 s)

```text
When there are jobs worth your time, Narrow sends them:
  (● WhatsApp — recommended)  ( Email )  ( Only here, in the app )

Weekdays at [ 09:00 ] · America/Sao_Paulo (detected) · [ ] weekends too
At most one message a day, with 1–3 jobs. Nothing is sent on days with nothing worth it.

[ Connect WhatsApp ]   → opens WhatsApp with a code to send (§9.1)
```

- Email reuses the existing confirm-by-link flow.
- "Only here" sets no channel. Today still works.
- **Stored as:** notification settings (§23).

### Then: Today

- Today may be empty: "Nothing worth your time yet. Narrow keeps
  watching and will message you when there is."
- No onboarding step promises a number of jobs.

### Totals and what was removed

**Totals:**
- 5 required screens and 1 optional.
- Required inputs beyond the upload: about 8 taps plus one number.
- **About 3–4 minutes.**

**Removed from onboarding, kept in Preferences or Profile:**
- the free-text description and its Taste Profile summary;
- Fine-tune (company, team, work style, time zone, domain);
- pay target;
- remote regions;
- the eligibility-unclear policy;
- claim review;
- LinkedIn and GitHub imports.

**Also fix:**
- the `has_preferences` notice;
- persisting the skip.

---

## 7. Ask vs infer vs learn

**The rule:**

| Do this | When |
|---|---|
| **ASK** | Being wrong makes a job impossible or materially undesirable, **and** it cannot be safely inferred. Hard constraints. |
| **INFER (and show)** | The resume strongly supports it, it is a soft preference, or it is easy to correct. One-tap confirmation is shown when an error would be costly. |
| **LEARN** | One action is insufficient evidence; behavior across several jobs is more reliable. Always visible, editable and attributable (§18). |
| **REMOVE** | Not needed to produce or deliver a recommendation. |

**Current profile fields, categorized:**

| ASK (onboarding) | INFER (prefilled or derived, shown) | LEARN (from feedback) | REMOVE from onboarding (kept in the model and in Preferences) |
|---|---|---|---|
| Kind of role (≤3) | Seniority (latest title, one-tap confirm) | Specialties to hide ("storage", "test automation") | Free-text description / Taste Profile review |
| Where you live (prefilled from resume) | Authorization = residence (prefilled, confirmed) | Company traits preferred (stage, size, team) | Pay target |
| Where you're authorized | Sponsorship needed (outside authorized places) | Ownership / pace / work-style preferences | Remote regions ("open to") |
| Work mode (3 options) | Remote hiring scopes that include you (eligibility) | Domain preferences | Eligibility-unclear policy |
| Relocation | Time zone overlap (from residence) | Technology preferences | Fine-tune controls |
| Pay floor + currency + period, unknown-pay policy | Notification timezone (browser) | Seniority drift (stretch roles saved or applied) | Claim review, LinkedIn, GitHub |
| Channel + time (for the agent) | Technologies and domains as **evidence** (not wants) | Specific companies to hide (via "Not for me → This company") | Engagement (employee/contract) unless stated |
| *(optional)* Avoids, specific companies | | | Start date / notice (asked at Prepare, §19) |

---

## 8. Proactive delivery model

### Domain

These are names for the design, not new crates.

```text
Recommendation  (computed by ranking — exists; never stored as a separate "notification ranking")
   │ selected by
NotificationPolicy  (new: digest | alert | none; pure function of rankings + state + settings + clock)
   │ produces
Delivery  (exists: notification_deliveries — one message "envelope" to one channel)
   └─ DeliveryItem (exists: notification_items — one opportunity in it, once per person ever)
   │ sent through
Channel  (new trait; email exists behind EmailSender; WhatsApp next)
   │ yields
DeliveryEvent  (new: sent / delivered / read / failed, from provider callbacks)
InboundAction  (new: a tap or link action, deduplicated, applied through the existing feedback use cases)
```

### Recommendations

- **Policy is channel-independent.**
  - The policy decides *which* opportunities and *what kind* of delivery
    (digest or alert).
  - The channel decides only *how* to render and send.
  - This is today's split between `notification_candidates`
    (`jobhunt-app`) and the email rendering in `jobhunt-cloud`.
- **A channel trait** generalizes `EmailSender` and has three parts:
  - `render(delivery, items, locale) → ChannelMessage(s)`;
  - `send(message, idempotency_key) → Receipt`;
  - `parse_callback(request) → DeliveryEvent | InboundAction`.
- **Email:**
  - It keeps the delivery id as the idempotency key.
  - It renders one email per delivery.
- **WhatsApp:**
  - It renders a delivery as one template message, then per-item
    messages only after the person asks for them (§9). That keeps paid
    sends to one per delivery.
  - Meta has no idempotency key on send, so the outbox must guard
    against duplicates.
  - Rule: the delivery is marked sent from the API response. A crash
    between the provider accepting it and the database recording it can
    resend once.
  - The 20h abandonment rule stays.
  - This weaker guarantee is documented in §28.
- **One primary proactive channel per person.**
  - A person with both email and WhatsApp gets recommendations on one
    of them. The other is only for account messages.
  - `notification_items`' primary key `(user, opportunity)` already
    makes "once, across all channels" true.
- **Future channels** (push, Slack, Telegram) implement the same trait.
  None changes the policy, the state or the actions.

---

## 9. WhatsApp UX

All copy is concise and structured, with no chat persona. A free-text
message from the person gets at most one fixed reply a day:
"Narrow only sends job picks here. To change anything, open
narrow.fyi/settings."

What WhatsApp allows, per Meta's documentation (§10):
- **Templates.** Their category decides the price.
  - Body up to 1024 characters.
  - Up to 10 quick-reply buttons whose payload comes back to us.
  - Up to 2 URL buttons, each with one dynamic suffix.
  - Quick replies and URL buttons must be grouped, not interleaved.
- **Free-form messages.** These are only allowed inside the 24-hour
  customer service window that starts when the person messages or taps.
  - Reply buttons: up to 3, labels up to 20 characters.
  - Lists: up to 10 rows.
  - A single "CTA URL" button.
- **Not available:** per-item buttons inside one template, except the
  Marketing-only media carousel, which needs an image on every card.
  Nothing in this design needs a capability that doesn't exist.

### 9.1 Opt-in and identity (one flow does both)

1. In Settings or onboarding, the signed-in person ticks a consent line
   that names the business and the message kind:
   > "Narrow will send me job recommendations on WhatsApp: up to one
   > message a day, and rarely a single new-job alert. I can stop
   > anytime by replying STOP."
2. **Connect WhatsApp** opens
   `https://wa.me/<Narrow number>?text=NARROW%20K7F3-9QX2`. On a desktop
   it shows a QR code.
   - The code is single-use, valid for 15 minutes, and only its hash is
     stored.
3. The person sends it. The inbound webhook carries their WhatsApp id
   (the phone number). Narrow binds that number to the account that
   issued the code.
   - Ownership is proven without an OTP template.
   - Narrow never types or guesses a number. The resume's phone is never
     used.
4. Sending the code opens a service window, so the confirmation is a
   free-form message:

   ```text
   Connected. Narrow will send jobs worth your time here on weekdays at 9:00 (São Paulo).
   Some days there will be nothing — that's on purpose.
   Reply STOP anytime to stop.
   ```

**Opting out:**
- Replying `STOP`, `PARAR`, `SAIR` or `CANCELAR` disables the channel and
  sends one confirmation.
- Meta's marketing opt-out ("Offers and announcements") arrives as a
  `user_preferences` webhook. It pauses the channel, and the web says so.
- If the person has email confirmed, Narrow offers it, but never switches
  silently.

### 9.2 Daily digest

The digest is one **Marketing** template (§10) with one variant per
item count, `digest_1` to `digest_3`. `digest_4` and `digest_5` are added
only if anyone chooses 5. Languages are `pt_BR` and `en`.

```text
┌──────────────────────────────────────────────────────────┐
│ Narrow · 3 jobs worth a look                              │  header (≤60)
│                                                           │
│ 1. Oneleet — Senior Backend Engineer                       │
│    Remote (Americas) · USD 140k–180k · posted 4h ago       │
│ 2. RevenueCat — Senior Backend Engineer                    │
│    Remote (Americas) · pay not published · new on board    │
│ 3. Camunda — Platform Engineer                             │
│    Remote (Brazil) · BRL 25k–32k/mo · posted 2 days ago    │
│                                                           │
│ Tap "Review here" to save or dismiss each one.            │
│ Not interested anymore? Reply STOP.                        │  footer (≤60)
│ [ Review here ]  [ Pause 1 week ]  [ Open in Narrow ↗ ]    │  QR, QR, URL (grouped)
└──────────────────────────────────────────────────────────┘
```

**Template parameters:**
- One parameter per job line.
- *Inference, to verify at template review:* Meta rejects newlines inside
  parameter values. That is why each count is its own template rather
  than one template with a variable-length list.

**"Review here"** opens the service window. Narrow then sends one
interactive reply-button message per job:

```text
1/3 · Oneleet
Senior Backend Engineer
Remote (Americas) · ✓ eligible · USD 140k–180k/yr
Posted 4h ago · verified open 1h ago
Why: backend and platform work · small engineering team
narrow.fyi/j/7Qm2…   ← item page (§22)
[ Save ]  [ Not for me ]  [ Not now ]
```

Each of these is a service message: $0.0068 in Brazil since 2026-10-01
(§10).

**"Open in Narrow"** opens `narrow.fyi/d/<delivery ref>`. That is the
same three jobs on the web, then the rest of Today, if signed in.

**"Pause 1 week"** sets `paused_until` and replies "Paused until Tue 21
Oct. Resume anytime in Settings."

**Window reuse:**
- If the person tapped anything in the last 24 hours, the next digest is
  sent as a free-form interactive message instead of a template.
- That is billed as a service message ($0.0068) instead of Marketing
  ($0.0625).
- Meta documents that a promotional free-form message inside the window
  is billed once, as service (Source: non-template pricing).

### 9.3 After "Not for me"

1. The rejection is recorded at once, with no reason, through the
   existing use case.
2. Narrow asks for the reason in an optional list message:

   ```text
   Got it — it won't come back. What didn't fit? (optional)
   [ Choose a reason ▾ ]
     Wrong kind of work
     Too specialized
     Wrong seniority
     Location
     Compensation
     Customer facing
     This company
     Something else (tell us in Narrow)
   ```

3. A chosen row is recorded as the reject reason text, word for word as
   the label.
   - The existing model stores reasons as text, and the existing reason
     reader interprets them.
   - The labels are the web dialog's existing chips. The reader
     (`ranking/src/reason.rs:361-363`, `536-543`) already maps
     "Wrong seniority", "Too specialized", "Wrong kind of work",
     "Location", "Compensation" and "customer facing".
   - Re-recording a reject with a new reason is a new event under the
     current repeat rule, which is correct.
4. **"This company"** asks for confirmation:

   ```text
   Hide every job at Nango from now on? You can undo this in Preferences.
   [ Hide Nango ]  [ Just this job ]
   ```

5. **"Too specialized"**, when the title has a qualifier, offers a hide
   rule (BRU-328 §10; §18):

   ```text
   Hide roles mentioning “Storage”? (title, team and department)
   [ Hide “Storage” ]  [ No ]
   ```

### 9.4 Immediate alert (off by default; §13)

The alert is one **Marketing** template, `alert_1`:

```text
┌──────────────────────────────────────────────────────────┐
│ New job worth a quick look                                │
│ RevenueCat — Senior Backend Engineer                      │
│ Remote (Americas) · USD 150k–190k                         │
│ Posted 37 minutes ago on RevenueCat's own job board.      │
│ It's the kind of role you asked for, and it's open now.   │
│ [ Save ]  [ Not for me ]  [ View job ↗ ]                  │
└──────────────────────────────────────────────────────────┘
```

**Copy rules:**
- No "perfect match", "unusually close" or "% match". Narrow's tiers
  aren't calibrated, and the copy must not claim they are.
- "Posted …" is used only with `posted_at` evidence.
- "New on RevenueCat's job board" is used when Narrow saw the job appear
  (§13.3).

### 9.5 Other messages

**Nothing qualifies:**
- Nothing is sent.
- No "no jobs today", no "Narrow is still looking", no weekly summary.

**A late tap** on a job that is no longer open:
- Narrow replies "This job closed on 12 Oct." Save is not recorded.
- Not for me and I applied are still recorded, since they are true about
  the person.

**A tap that changes nothing:**
- Example: Save after Applied.
- Reply: "Already marked as applied." This is the existing repeat or
  transition rule.

**Applied:**
- It is not a WhatsApp button, because of the 3-button limit and because
  Save, Not for me and Not now are the decisions a list asks for.
- "I applied" is on the item page and the web.

---

## 10. WhatsApp provider research

Research date: 2026-10-11. Sources are listed at the end ([W1]–[W26]).
Where Meta's pages disagree, both are quoted.

### 10.1 Documented facts

| Topic | Meta WhatsApp Cloud API (direct) | Twilio WhatsApp |
|---|---|---|
| Accounts | Meta developer app, business portfolio, WhatsApp Business Account (WABA), which can be auto-created [W1] | Twilio account plus your own Meta business portfolio via Self Sign-up [W14] |
| Business verification | **Not required to start.** Unverified portfolios are limited to 250 unique recipients per 24h; verification (or 2,000 delivered business-initiated messages in 30 days) lifts it to 2,000 [W2] | "You must complete business verification on Meta before you can move into production"; "can take several weeks" [W14] |
| Test setup | Test WABA and test number created automatically; no payment method needed [W3] | Sandbox; recipients must join it first [W15] |
| Phone number | Must be owned by you, have a country code, and receive SMS or a voice call. Can't be used in the WhatsApp app at the same time unless onboarded with coexistence. New portfolios get up to 2 numbers [W4][W5] | Twilio or own number; must not already be registered on WhatsApp; Brazil local number $4.25/month [W14][W16] |
| Messaging limits | Per business portfolio (since 2025-10-07): 250 → 2,000 → 10,000 → 100,000 → unlimited, auto-scaling with quality, effective within 6 h [W2][W6] | Same Meta limits |
| Template categories | Marketing, Utility, Authentication. Utility must be non-promotional **and** specific to or requested by the user (order, account, transaction) or essential. Meta's Marketing examples include *"We found a {{car}} that meets your saved search"*, under "these are marketing even if requested by users". Since 2025-04-09 a misfiled utility template "is approved as MARKETING"; repeated misclassification brings escalating restrictions [W7] | Same Meta rules (Content API) |
| Template review | Up to 24 h; pt_BR supported; no auto-translation; variables can't start or end a template [W8][W9] | "Usually within one business day" [W17] |
| Template buttons | Up to 10 total. Quick replies up to 10 (25 chars), payload returned on webhook. URL buttons ≤ 2, one dynamic suffix each. Quick replies and others must be grouped. Mixed templates don't render on WhatsApp desktop [W10] | Same, via Content API (quick-reply payload as `ButtonPayload`) [W18] |
| 24-hour window | Opens when the user messages (a button tap is an inbound message). Inside it any message type; outside only templates [W11] | Same |
| Interactive (in-window) | Reply buttons ≤ 3 (20 chars); list ≤ 10 rows; CTA URL ≤ 1 button; media carousel 2–10 cards [W12] | Same, via Content API |
| Webhooks | Inbound messages, `button` payloads, statuses `sent/delivered/read/failed` with a `pricing` object; `X-Hub-Signature-256` (HMAC-SHA256, app secret); retries up to 7 days with possible duplicates; keep median latency ≤ 250 ms [W13][W19][W20] | `StatusCallback` (queued/sent/delivered/undelivered/read/failed); `X-Twilio-Signature` (HMAC-SHA1 with auth token) [W21][W22] |
| Pricing model | Per delivered message since 2025-07-01 [W23]. **Since 2026-10-01 Meta charges for service and in-window utility messages too, at utility rates** [W24]; Meta's main pricing page still says non-template messages are free [W23] | Meta fees passed through, **plus $0.005 per message inbound or outbound, plus $0.001 per failed message** [W25] |
| Brazil rates (per delivered message, rate card effective 2026-10-01) | Marketing **$0.0625** (R$0.3217); Utility $0.0068 (R$0.035); Authentication $0.0068; Service $0.0068 [W23] | Same + $0.005 |
| Marketing limits in Brazil | Per-user marketing caps apply (failure 131049); users can turn off "Offers and announcements" (webhook `user_preferences`) [W26] | Same |
| Billing currency | BRL WABAs for Brazil Sold-To customers since 2026-07-01; non-BRL WABAs of eligible customers stop delivering on 2027-07-01 [W23] | Twilio is the billing party (currency not verified) |
| Portability | Registered numbers can be migrated between WABAs, keeping display name, quality and limits [W5] | Documents migration *into* Twilio; Content API template IDs (ContentSid) |
| Operations | You host the webhook; long-lived system-user token; On-Premises API sunset (Cloud API only); 1 message per 6 s per recipient; throughput 80 → 1,000 mps [W3][W19] | Twilio queues (up to 4 h); you still host webhooks |
| AI provider charge | Since 2026-03-11 Meta charges "AI Providers" (general-purpose AI assistants) for non-template messages to +55 numbers [W24] | Same |

### 10.2 Inference

**Narrow's digest and alert are Marketing.** Meta's own "saved search"
example is the same shape: "a job that matches what you asked for".
Filing them as Utility would invite recategorization and penalties
across the business portfolio.

**Cost per person per month** at Brazil rates:

| Scenario | Cost | Notes |
|---|---|---|
| Base (weekday digests) | **≈ $1.38** | 22 digests × $0.0625 |
| Digest reviewed inside the window | +$0.02 per day | 3 item messages × $0.0068 |
| Digests reusing an open window | ≈ $0.15 | 22 × $0.0068 |
| Each alert | $0.0625 | Rare by design |
| Through Twilio | +$0.005 per message both ways | Adds ~8% on Marketing and ~74% on service messages |

**Narrow is probably not a Meta "AI Provider".** Narrow is not a
general-purpose assistant. Check this against the WhatsApp Business
Solution Terms before launch.

**Per-user marketing caps mean a digest can fail to deliver.**
- Today stays canonical, so a lost message loses nothing but the
  interruption.
- Error 131049 must not be retried in a loop.

### 10.3 Not verified

- Whether Meta offers a free monthly allowance of service messages
  (third-party claims only).
- Coexistence for direct (non-partner) developers.
- Test-number recipient count.
- Whether Meta retains message content and for how long.
  - Statements I recall from Meta's data-privacy page could not be
    confirmed in this research.
  - Check that page before writing the privacy notice (§22).

---

## 11. Provider recommendation

**Recommendation: Meta WhatsApp Cloud API, directly.**

### Why direct

| Factor | Meta direct |
|---|---|
| Start | Test number in minutes; real users with no business verification (250 recipients/day covers the MVP); first pt_BR template within 24 h |
| Cost | No provider markup. On service messages Twilio's markup is 74%; on Marketing it is about 8% |
| Billing | BRL billing; becomes mandatory for Brazilian entities on direct WABAs from 2027-07 |
| Portability | The WABA and number sit in Narrow's own business portfolio |

### Why not Twilio

Twilio's value is queueing, SDKs and support. Narrow already has:
- an outbox;
- leases;
- retries;
- a provider-trait pattern (`EmailSender`).

Twilio's documented requirement of Meta business verification "before
production" would also delay the MVP.

### Other options considered

- **A flat-fee BSP such as 360dialog** (€49/month, no markup [W27]) does
  not change the decision at MVP scale.

### What it costs Narrow

| Item | Detail |
|---|---|
| Webhook endpoint | Hosted on the `api` service: signature verification, ≤ 250 ms median (persist and acknowledge) |
| Secret | A sealed system-user token |
| Templates | Template management |
| Business verification | Before passing 250 recipients a day |

That is roughly the operational shape of the Resend integration plus a
webhook.

### What would change the decision

- Meta forcing a partner-only onboarding for new direct developers.
- Narrow needing 24/7 vendor support before it has an on-call.

---

## 12. Notification selection policy

The policy is a **selection over existing rankings**. It never re-scores.

### The pool

**Today's ranking**, unchanged:
- `RankingService::rank`;
- the same tiers, gates, briefs and sort (gate, tier, score, listing
  date).

**The candidate pool `C`** is what `notification_candidates` already
selects:

| Condition | Code |
|---|---|
| `tier == StrongFit` | `rank.rs:436` |
| `gate == Gate::Recommended` | trusted verification ≤ 72 h at an authoritative source, and eligible or conditionally eligible |
| `Stage::Unseen` | |
| No `FeedMark` | never shown in Today/search, never put aside |
| Not already in `notification_items` | |

**New filters, applied by selection, not by ranking:**
- the company is not hidden by the person (§17);
- no hide rule of the person's matches (when BRU-328's hide rules land);
- one opportunity per company per delivery. This closes today's gap
  (§3).

### The tiers

| Tier | Gets | Rule |
|---|---|---|
| **Immediate alert** | at most 1/day, 3/week, **off by default** | §13.1 |
| **Digest** | at most 1/day at the chosen local time, 3 items (≤ 5 if chosen) | §13.2 |
| **No notification** | everything else | Stays where it already is: Today (strong fits including `VerifyFirst`/`EligibilityUnclear`, changed items) and search (worth reviewing, maybe) |

### Deliberately not done

- **No plausible fits ("worth reviewing") in any notification** until
  BRU-328 passes its gate. Today doesn't show them either.
- **No resurfacing by message** of changed jobs (pay published on a saved
  job). Today handles those. The `(user, opportunity)` primary key keeps
  "once, ever".
- **No engagement filler:**
  - no "nothing new";
  - no weekly recap;
  - no "we miss you".

---

## 13. Immediate vs digest

### 13.0 Is "Strong" good enough to interrupt for?

**Honest answer: for a daily digest the person can ignore, yes, as an
experiment. For an immediate interruption, not yet.**

**What has been measured (Doc):**
- Strict precision of Strong on real postings with today's rules is about
  23%.
- Company boilerplate made every peer at Supabase and Railway "Strong"
  (`real-posting-precision-experiment.md` §18).
- The "worth 30 seconds" question, which is the right one for a digest,
  has never been answered on real Today jobs.

**Why the digest is still fine:**
- It is cheap in attention: one message, read when convenient, one per
  company.
- Delivered digests are the first real instrument to answer that
  question (§24).

**Why an alert is not:**
- It claims urgency. Its cost is higher, and its precision would need to
  be higher.

**This is not papered over.** Alerts ship disabled and are enabled per
person only after the gate in §13.1.

### 13.1 Immediate alert rule

An opportunity in `C` is an **alert candidate** only if *all* hold:

**About the job:**

| Rule | Code |
|---|---|
| Eligibility `Eligible`, not `Conditional` | nothing to check first |
| `brief.caveats` is empty: no fit contradiction of any severity, no practical concern, no blocker | `brief.rs:179-192` |
| **Fresh by evidence** (§13.3): posted ≤ 24 h ago by `posted_at` from a dated adapter, or *newly listed* ≤ 24 h ago. **Never a backfill.** | |
| Verified open after it was first seen, and within the last 12 h | the verification worker already prioritizes jobs first seen < 2 days, `schedule.rs:424-460` |
| Not at a company that had an alert in the last 14 days | |

**About the person:**
- `alerts_enabled` for this person, and the global switch on.
- Outside quiet hours and on an allowed day; otherwise it waits for the
  next digest as an ordinary item.
- Fewer than 1 alert today and 3 this week.

**Ordering and spillover:**
- Several candidates at once: the best-ranked one, and the rest go to the
  digest.
- No score threshold such as "meaningfully better than usual". Narrow's
  fit score orders jobs within a tier but is not calibrated across days,
  and inventing a threshold would be a ranking change.
- Immediacy is justified by *freshness*, which matters only when it is
  real, not by claimed superiority.

**Gate for enabling alerts** (per person, first the dogfood user):
- Delivered digest jobs judged worth the interruption ≥ **80%**, on ≥
  **30** judged deliveries.
- **0** impossible.
- At most 1 "obvious avoidable No" per week.
- These are the BRU-328 criteria applied to deliveries (§24).

### 13.2 Digest rule

**When it is composed:**
- At the person's `digest_time` (default 09:00) in their timezone.
- On allowed days (default Mon–Fri).
- Not while paused.

**What goes in:**
- `C` in rank order, one per company.
- At most `digest_size` items (default 3, maximum 5).
- Companies that had an item in a digest or alert in the last 7 days are
  skipped *if* another company can fill the slot. This keeps variety
  without padding.

**Fewer is fine:**
- With 1 or 2 qualifying items, the digest has 1 or 2 items.
- **With 0, nothing is sent.**

**Items marked fresh show their freshness label.** Older items show
"posted 9 days ago". An old posting is not hidden: two year-old Railway
jobs were Strong yes in BRU-325. It is labelled honestly.

**When BRU-328 passes:**
- A later version may let "Possible fit" fill empty digest slots,
  labelled as such.
- Never in alerts.

### 13.3 Freshness

| Timestamp | Meaning | Where (Code) |
|---|---|---|
| `posted_at` | the source's publish date | `JobPosting.posted_at` (`model.rs:408`): Greenhouse `first_published`, Ashby `publishedAt` (reliable); Lever `createdAt` (approximate: creation, not publication); YC and career pages: none |
| `first_seen_at` | when Narrow first read it | `JobRecord.first_seen_at` (`model.rs:680`) |
| `last_seen_at` | last scan that listed it | `model.rs:682` |
| `verified_at` | last successful verification | `VerificationBrief.verified_at` (`views.rs:261-299`), from `job_verifications.attempted_at`; no column of that name |

**The trap.** When a board is added (BRU-343/360 activation), every job
on it is inserted with `first_seen_at = now` and a `New` event. Nothing in
feed or notification logic distinguishes this (Code: audit;
`broad-discovery.md:476`). Today that would make a backfill look new. It
doesn't matter for Today, but it would for alerts.

**The derived freshness class**, computed at selection time:

| Class | Evidence | Label | Alert-eligible |
|---|---|---|---|
| `posted` | `posted_at` from Greenhouse/Ashby | "Posted 4h ago" | if ≤ 24 h |
| `posted_approx` | Lever `createdAt` | "Listed 4h ago" | no (digest only) |
| `newly_listed` | `first_seen_at` is after the board's first *complete* listing scan **and** the previous complete scan of that board was ≤ 12 h earlier | "New on {company}'s board · first seen 3h ago" | if ≤ 24 h |
| `backfill` | first seen in the board's first complete listing | "Posted date unknown" or the `posted_at` label | **never** |
| `unknown` | none of the above (e.g. YC, career pages, gaps in scanning) | "Posted date unknown" | **never** |
| `reopened` | `Reopened` event | "Reopened 2 days ago" | no |

**Derivation:**
- All of this comes from existing tables:
  - `source_scans` (`complete`, `status='listing'`, `finished_at` per
    source);
  - `job_events` (`run_id` of the `New` event).
- No schema change is required.
- A `first_complete_listing_at` on `source_schedule` would make it a
  join instead of an aggregate.

**Rules:**
- "New" in any copy means `posted` or `newly_listed`.
- Narrow never says "new" because *it* just discovered something.
- Unknown freshness never blocks the digest. It just isn't an alert.

**Latency to measure** (§24):
- `first_seen_at − posted_at`, excluding backfill.
- `sent_at − first_seen_at`.

The BRU-360 timing cohort (`first_party_timing.rs`) already measures the
first.

---

## 14. Quiet hours and frequency

**Recommended initial limits:**

| Limit | Default | Person can change |
|---|---|---|
| Digests | ≤ 1 per local day | time, days, size 3 or 5, pause 1 week, off |
| Items per digest | 3 | 1–5 |
| Digest days | Mon–Fri | every day |
| Digest time | 09:00 local | any hour outside quiet hours |
| Alerts | **off**; when on: ≤ 1/day, ≤ 3/week, ≥ 14 days between alerts for the same company | on/off (after the gate) |
| Total proactive messages | ≤ 2/day (1 digest + 1 alert); ≤ 8/week | — |
| Quiet hours | 21:00–08:00 local | yes |
| Timezone | browser, confirmed on the channel step | yes |
| Same opportunity | once, ever, across channels (existing primary key) | — |
| Replies to taps | unlimited inside the window; not counted as interruptions (the person asked) | — |

**What happens to a digest held back by quiet hours or limits:**
- It is never sent late.
- Its items stay in `C` for the next digest, if they still qualify.

**An unanswered digest is not a signal to send more:**
- Nothing escalates.
- **Recommendation:** after 10 consecutive digests with no interaction,
  the next digest's footer says once:
  > "Not useful? Reply STOP or change how often in Settings."
- There is no automatic throttling the person can't see.

---

## 15. Canonical recommendation state

### One state per opportunity

There is **one state per opportunity**: the existing append-only
`feedback` log, replayed into `OpportunityState` (`feedback.rs:284-341`).
- Every channel action goes through the **same use cases**:
  - `record_feedback`;
  - `dismiss`;
  - plus `hide_company` and hide rules.
- They take the same per-account advisory lock (`docs/cloud.md`) and
  follow the same repeat rule.
- No channel stores its own state.
- The channel adds only **attribution**: `via` (web, cli, mcp, email,
  whatsapp) on the delivery item and in usage events.

### Mapping

| Person does | Where | Recorded as (existing) | Today | Future notifications |
|---|---|---|---|---|
| Save | web / WhatsApp button / item page | `FeedbackAction::Save` | leaves Today ("Saved", in Applications) | excluded (stage ≠ unseen) |
| Not for me (+ reason) | web / WhatsApp + list / item page | `Reject` (+ reason text) | gone (gate `Rejected`) | excluded |
| Not now | web / WhatsApp / item page | `dismiss` (`Seen` + `dismissed_at`) | gone until a material change | excluded (has a feed mark) |
| I applied | web / item page / MCP | `Applied` | gone (`InPipeline`) | excluded |
| Hide company | WhatsApp confirm / web / Preferences | **new** hidden-company entity | that company's jobs gone | excluded |
| Opens the job | WhatsApp link / email link | `notification_items.opened_at` (new); **not** `Seen` | unchanged | — |
| Nothing | — | — | still "new" on Today until shown there and its 24 h hold passes (existing) | not re-sent (primary key) |

### How Today and delivery relate

**Delivery does not consume Today.** This is unchanged from today's email
behavior.
- A delivered job is still new on Today, with a "Sent on WhatsApp · Tue
  9:00" marker.
- Today's `HOLD_HOURS` and "passed over" logic are untouched.
- If the person opened Today first, the job has a feed mark and is not
  delivered. They already saw it.

**Today stays the superset:**
- The digest is "the verified, eligible, new part of Today, capped, one
  per company".
- Not a different list.

### Idempotency and conflicts

**Every inbound action carries a server-side item reference**
(`notification_items.ref`, random 128-bit):
- Tap payload: `v1:<ref>:save`.
- Link: `/j/<ref>`.
- The action is resolved to the person through that reference **and**
  checked against the sender (§22).

**WhatsApp webhook duplicates:**
- Meta retries for up to 7 days.
- They are dropped by the inbound message id (`notification_inbound`
  primary key, §23).

**Actions are idempotent by construction:**
- A repeat returns `recorded: false` (`service.rs:286-291`).
- An impossible transition is a no-op that returns the current stage.
- The reply says so ("Already saved").

**Conflicting actions on two channels resolve by time order**, as the
replay already does:
- Not for me on the web, then Save on WhatsApp: saved. The transition
  `Rejected → Saved` exists.
- Applied, then Not for me: rejected, with `furthest = applied` kept.

**Closed jobs:** the action handler re-reads the opportunity's status
(§9.5).

**MCP parity:** add "Not now" (`dismiss`) to MCP. It's the only action
missing there (Code: audit).

---

## 16. Feedback actions

| Action | Meaning | Learning effect (existing, `ranking/src/taste.rs`) | Notes |
|---|---|---|---|
| **Save** | "I want to revisit this." Explicit positive for *this job* | +0.25 behavior weight, spread over the job's facets; a pattern needs 3 jobs (tentative) or 5 (established) | One save never creates a preference. Unchanged |
| **Applied** | Strong positive event | +0.5 behavior weight; same thresholds | Not overgeneralized. Unchanged |
| **Not for me** | Remove this job; optional reason | −0.2 without a readable reason, −0.1 with one; the *reason* counts 1.0 (or 0.75 per fact for "this domain"); a pattern needs 1 (tentative) to 3 (strong) reasons | Reasons on WhatsApp are the web's existing chip labels, which the reader already maps (`reason.rs:536-543`) |
| **Not now** | Suppress this job only | none (`Seen` carries no weight) | Unchanged |
| **Hide company** | Never show this employer to me | **none**: a filter, not taste | §17 |
| **Hide rule** | Never show jobs mentioning X | none: a filter, not taste | §18 |

**Reason labels, the same on every channel.** These are the web dialog's
existing chips (`reject-dialog.tsx:12-21`), so the reader's mapping is
unchanged:

| Label | Effect |
|---|---|
| Wrong kind of work | read as "avoid job:Work" |
| Too specialized | read as "avoid job:Specialty"; may offer a hide rule (§18) |
| Wrong seniority | read as "avoid job:Level" |
| Location | read as "avoid information:location"; **also** counted as a quality defect in metrics (an eligibility miss reached the person, §24) |
| Compensation | read as "avoid compensation:pay_level" |
| Customer facing | read as a role signal (`reason.rs:229`) |
| This company | **new:** offers hiding (§17); also recorded as text "this company", which the reader resolves to the employer (`reason.rs:355-357`) |
| Something else | free text, on the web only |

**Dropped from WhatsApp:** the web's "Company too big", "Company stage"
and "Domain" chips. A list has a 10-row limit and the rows above are the
frequent ones. These three stay on the web.

**Recommendation:** also store a reason **code** next to the verbatim
text.
- It is a nullable column on `feedback`, additive.
- Metrics (§24) can then count reasons without re-reading encrypted text.
- The text stays the source of truth for learning.

---

## 17. Company suppression

### What the person sees

- "Hidden companies" in Preferences: "Nango · hidden on 9 Oct from
  'Senior Backend Engineer' · Undo".
- Today notes "1 job hidden by your company list", as BRU-328 does for
  rules.

### What it does

- Removes that company's opportunities from:
  - Today;
  - digests;
  - alerts.
- **Search still shows them**, labelled "Hidden company". The person can
  still find them on purpose, and nothing is silently missing from an
  explicit query.
- Saved or applied jobs at the company stay in Applications.

### What it doesn't do

- Change fit, taste, learned patterns, or ranking.
- Remove the board from monitoring, or affect any other person.
  Discovery is shared; suppression is private.

### Matching

There is no company entity (Code: companies are free text; Today groups
by `search_key(company)`). A hidden company stores:
- the display name;
- the normalized company key (`search_key`);
- the **source boards** of the opportunity it came from,
  `(source_kind, source_instance)`, e.g. `ashby:nango`.

A job matches if its company key **or** any of its records' boards match.
- That covers name variants ("Nango" and "Nango, Inc.") through the
  board.
- It covers a second board for the same company through the name.
- Domain matching can be added when a company entity exists.

### Storage

- A private profile entity: encrypted in the cloud, synced, exported, and
  deleted with the account, like BRU-328's `hide_rule`.
- **Recommendation:** implement it as the same "hide" entity family with
  a `company` target. A separate kind would duplicate sync and export
  code.

### Creation

Only explicit:
- WhatsApp "This company → Hide Nango";
- the web "Not for me" dialog's "Hide all jobs at Nango";
- Preferences "Hide a company";
- onboarding "Specific companies to skip", matched by name until a job
  provides boards.

Two rejections at the same company may *suggest* hiding it (§18), but
never hide it automatically.

---

## 18. Learned rules

BRU-328's mechanism is reused unchanged (branch
`t3code/worth-reviewing-blind-gate`, `jobhunt-profile/src/hide.rs`,
`jobhunt-app/src/hide_rules.rs`; not merged):
- A hide rule is the person's phrase, matched as whole words in the
  title, team and department, or anywhere.
- It is visible, editable and removable.
- It never changes fit, search or taste.
- Narrow never creates one by itself.

### When Narrow offers a rule

It *offers*, and the person confirms.

| Trigger | Offer |
|---|---|
| One "Not for me" with reason **Too specialized**, and the title has a qualifier ("Software Engineer, Storage") | "Hide roles mentioning “Storage”?" |
| **Three** rejections in 30 days whose titles share a qualifier or specialty word, with reasons Too specialized / Wrong kind of work | "You've passed on 3 storage-focused roles. Hide roles mentioning “storage”?" |
| **Two** rejections at the same company | "Hide every job at Nango?" |
| An existing learned pattern (`taste.rs`) reaching "established" | Shown in Preferences → "Learned over time" as today, with "Make it a rule / Not right" (BRU-321 confirm/correct) |

The one-rejection offer is the person's explicit reason plus an explicit
confirmation. That is not an inference from an ambiguous click.

### Where offers appear

- **Inside an open WhatsApp window**, right after the triggering
  rejection. They are free-form and cost nothing extra in attention.
- **On the web:** Today's outcome line and Preferences.
- **Never as a new outbound message** by themselves.

### Attribution

Each rule keeps `origin`: the job, the reason, the date. Preferences
shows "From 'Senior Software Engineer, Storage' (Railway), 9 Oct".

### Measured

BRU-328 §10 defines how rules are measured:
- hits;
- false suppressions;
- escapes.

Apply them to delivered jobs.

---

## 19. Application capability model

Applying differs per job, and the product says so instead of offering one
"Apply" button.

### The capability

The capability is a property of the **posting's application endpoint**
(source kind + apply URL host), derived at read time.

| Capability | Label | Available when | What Narrow does |
|---|---|---|---|
| `external` | **Apply on company site** | every opportunity verified open, with `url` / `apply_url` (Code: `JobPosting.apply_url`, `model.rs:386`) | opens the canonical first-party application URL; offers "I applied" afterwards |
| `prepare` | **Prepare application** | any open job (standard kit); **with the job's own questions** only where the ATS publishes them (§20: Greenhouse; Recruitee once an adapter exists) | builds a kit for the person to review and copy; then opens the external flow |
| `assisted_submit` | **Apply with Narrow** | **never today.** Only for an endpoint covered by a signed partner agreement *and* a successful end-to-end test on that ATS | submits after explicit review; anything uncertain falls back to `external` |
| `unavailable` | "No longer open" / "Verify first" | closed, or not trusted (`Gate::VerifyFirst`) | no application call to action |

**Rules:**
- Capability is shown, never implied.
- An ATS that changes (Greenhouse → Ashby) changes the capability
  automatically because it is derived from the record, not stored.
- **"Never fake support":** if any `prepare` input fails to load (schema
  unreachable, unknown question types), the kit degrades to the standard
  kit and says what's missing.

### Prepare MVP: what the kit contains

**From existing data:**
- **The resume:** the current uploaded document, as a download.
- **Facts:** `GET /api/v1/opportunities/{id}/application-context`
  (Code: `jobhunt-app/src/context.rs`). These are facts usable under the
  evidence policy:
  - confirmed;
  - entered;
  - or quoted from the resume with high confidence.
  - Withheld claims are counted, never shown.

**Standard answers**, each with a copy button. The person decides what to
disclose:

| Answer | Source |
|---|---|
| Where you live | the location preference |
| Authorization | the authorization preference |
| Work mode | the work mode preference |
| Links | profile contacts |
| Salary expectation | **left blank by default**. The person's floor is private (§22); Narrow may *show* the posted range and the floor side by side, but never pre-fills it |
| Start date / notice | **asked here the first time** and remembered: the only new field, and asked when it matters |

**The job's questions** (Greenhouse, via the public `?questions=true`
schema [A1]):
- required flags;
- standard fields mapped to the kit;
- custom questions listed for the person to answer.

**Never in the kit:**
- **Voluntary EEO / demographic questions** are never pre-filled, and
  "answer them on the company's form" is said explicitly.
- **GDPR / data-retention consent** is never ticked on the person's
  behalf.

**Later, separately:** screening-answer **drafts** from usable facts only
(the context `POLICY`), clearly marked as drafts. That needs a model call
and an evidence check of its own, and is not in the Prepare MVP.

**Not built:** tailored resumes, cover letters, or any submission.

---

## 20. ATS application research

Research date: 2026-10-11; sources [A1]–[A37].

### 20.1 Documented facts

| ATS | Public question schema | Submission API & who holds the key | Login | Anti-bot (documented) |
|---|---|---|---|---|
| **Greenhouse** | **Yes**: `GET boards-api.greenhouse.io/v1/boards/{token}/jobs/{id}?questions=true`, no auth; includes EEOC, demographic and GDPR (`data_compliance`) blocks [A1] | `POST …/jobs/{id}` with the **employer's** Job Board API key ("proxy form posts through your own servers") [A1]; partner **Candidate Ingestion API** (OAuth or the customer's partner key; defaults to prospects) [A3][A4] | no | invisible reCAPTCHA, per-board sensitivity, email code challenge [A2] |
| **Lever** | **Partial**: postings public; custom questions "not exposed" | `POST /v0/postings/{site}/{id}?key=` with a key made by the **employer's** Super Admin; partner OAuth separate [A5][A7] | no | hCaptcha for suspicious users (help-center snippet) [A6] |
| **Ashby** | **No** (public posting API has no form; `jobPosting.info` needs a `jobsRead` key) | `applicationForm.submit`, **employer** key with `candidatesWrite` [A8]–[A11] | no | "Application Spam Protection" (Strict→None), fraud checks, application limits [A12][A13] |
| **Workable** | **No** (questions endpoint needs a token) | `POST /spi/v3/jobs/{shortcode}/candidates`, **employer** token; partner tokens "only… official third party integrations" [A14]–[A16] | no | firewall, IP reputation, browser integrity, CAPTCHA; explicitly targets AI auto-apply tools and tags AI-assisted applications [A37] |
| **Workday** | none found | none found | **employer can require a Candidate Home account**, per employer [A18] | not documented |
| **SmartRecruiters** | **No** (posting API public; `/configuration` protected) | `POST /postings/{uuid}/candidates`, built "for customers and partners"; customer key or partner OAuth [A20]–[A22] | not found | not found |
| **Recruitee** | **Yes**: keyless `GET /api/offers/` with `open_questions` [A23] | **keyless** `POST /api/offers/{slug}/candidates` (CAPTCHA not triggered on this path) [A24][A25]; no third-party permission documented | no | invisible CAPTCHA on hosted forms [A25] |
| **Personio** | No (XML feed is jobs only) | employer recruiting token, backend only [A27][A28] | not found | not found |
| **BambooHR** | No | caller needs ATS settings access [A29] | not found | not found |
| **Teamtailor** | No | Admin-scope key by a Company Admin; separate Partner API [A30][A31] | not found | not found |
| **JazzHR / Breezy** | No | account key / account credentials [A32][A33] | not found | not found |
| **Rippling** | No | job-board API publishes jobs; no apply endpoint [A34] | not found | not found |

**LinkedIn Apply Connect and Indeed Apply** are employer-enabled
distribution channels, not open to other parties [A35].

### 20.2 Inference

**Employer-held keys don't extend to candidate-side tools.** Keys are
admin-created, the docs say to proxy them server-side, and partner tokens
are reserved for official integrations. So a candidate-side tool can't
legitimately use them without an agreement.

**Recruitee's keyless POST works technically**, but nothing grants
third-party use. Using it for Narrow without permission would be a
policy risk. **Don't.**

**Ashby and Lever load their full forms through undocumented browser
endpoints.** Reading those is scraping, not a schema source. **Don't
build on it.**

**No ATS has a documented candidate-side submission path.** Every
"Assisted Apply" route is a partnership:
- Greenhouse Candidate Ingestion;
- the SmartRecruiters Application API;
- Lever partner OAuth.

Most also need each employer to opt in.

### 20.3 Classification

**Recommendation:**

| ATS | Narrow reads it today | Classification |
|---|---|---|
| Greenhouse | yes | **Potential Prepare (with questions)**; Assisted only via partnership plus per-employer opt-in |
| Lever | yes | **Prepare (standard kit only)**; Assisted only via partner OAuth |
| Ashby | yes | **External only** (standard kit available, no job questions) |
| Workable | next adapter (BRU-360) | **External only; poor target** for any form automation |
| Recruitee | no adapter | **Potential Prepare (with questions)**; not Assisted without permission |
| SmartRecruiters | no adapter | External; **best formal partner route** if Assisted is ever pursued |
| Teamtailor, Personio, BambooHR, JazzHR, Breezy, Rippling, Gem, Dover, Pinpoint, iCIMS, Jobvite | no | **External only** |
| Workday | no | **Poor automation target** |

**Volume.** The ATSs Narrow reads are Ashby (54k postings in BRU-360),
Greenhouse (62k) and Lever (14k).
- `prepare` with real questions is available on Greenhouse, the largest.
- Everything else gets the standard kit.

---

## 21. Website's role

The website stops being where discovery happens. It becomes where the
person **reviews, tracks and steers**.

| Surface | Role | Change |
|---|---|---|
| **Today** | The canonical, finite review queue: everything worth a decision now, including what was delivered | Add "Sent on WhatsApp · Tue 9:00" markers and the hidden-company/rule notes; the empty state says Narrow will message when there's something |
| **Applications** (Saved / Applied / Interviewing / Offer) | Application history and follow-up | Unchanged; add the capability label and "Prepare application" when it exists |
| **Preferences** | Simple controls, then "Hidden companies", "Hide rules", "Learned over time" | Reorder around the onboarding questions; the Taste Profile and Fine-tune move below |
| **Settings → Notifications** | Channel, schedule, quiet hours, pause; **"What Narrow sent you"**: the last 30 deliveries with each item's state | Extends the existing notification settings, which already list the last 5 deliveries |
| **Item page `/j/<ref>`** | What a message links to: the job, Apply on company site, actions (§22) | New, minimal |
| **Profile** | Career evidence and claim review | Unchanged |

**"Activity": recommended against as a page.**
- "12,431 jobs checked, 165 companies monitored" is vanity. The person
  can't act on it, and it invites comparisons with job boards.
- The one useful part already exists as Today's status line:
  - "Watching N company job boards · last checked 12 min ago · next
    digest 9:00" (Code: `DiscoveryStatus`, `FeedSummary`).
- The other useful part is the "What Narrow sent you" history above.
  - It answers "where's that job from Tuesday's message?"

---

## 22. Privacy and security

### 22.1 What Meta receives

**Provider-side** (Meta as the WhatsApp Cloud API provider):
- the person's WhatsApp number (their id);
- the content of every message Narrow sends;
- the button and list choices the person taps (including reason labels);
- timestamps;
- delivery and read status.

**What messages contain:**
- company, title;
- the **job's** location and workplace;
- **published** pay only;
- freshness;
- at most 2 short reasons phrased about the **job** ("backend and
  platform work · small engineering team");
- "✓ eligible" without saying why;
- a Narrow link carrying an opaque item reference.

**Messages never contain:**
- the resume, employment history, or any profile claim;
- where the person lives;
- their authorization;
- their pay floor or target, or anything that reveals them ("below your
  minimum");
- Taste Profile assertions, or reasons phrased about the person ("you
  said you avoid big companies");
- their free-text rejection reasons (free text is collected only on the
  web);
- email address, name, or account ids.

**Meta-side retention** of message content is *not verified* in this
research (§10.3). The privacy notice must state it from Meta's
current terms before launch.

### 22.2 What Narrow stores

**The channel address:**
- The WhatsApp number is sealed, like the email address today
  (`notification_settings.email` is sealed per account).
- A keyed hash of the number is kept for routing inbound messages.
- Both are deleted with the account.
- Numbers are never logged. Logs use the account id.

**Inbound content:**
- Stored are the inbound message id, the button payload (an opaque ref
  plus an action) and the outcome.
- **The text of free-form messages is not stored.** It is checked only
  for STOP words and the binding code, then discarded.

**Usage events:** gain `client = whatsapp | email`, with the same rules
as today (counts and actions, never content).

### 22.3 Links and actions

**No long-lived credential ever appears in a URL.**

| Flow | Design |
|---|---|
| **WhatsApp buttons** (Save, Not for me, Not now, reasons, hide) | No URL at all: the action arrives on the webhook. Authenticated by: (1) a valid `X-Hub-Signature-256` over the raw body with the app secret, constant-time compare, otherwise 401; (2) the sender's number being the **bound, active** channel of the account that owns the item `ref` in the payload. A payload for someone else's item, or from an unbound number, is ignored. |
| **View / item page** `narrow.fyi/j/<ref>` | `ref` is a random 128-bit id of one delivered item. It is a **capability for that one item only**: public job facts, "Apply on company site", and the four actions (Save, Not now, Not for me, I applied), valid **7 days** after delivery. It never shows personal reasons, other jobs or account data; "See why Narrow picked it" requires sign-in. |
| **Actions on the item page** | Performed by **POST** with a per-page anti-CSRF token. **GET never changes state**, so WhatsApp's and mail scanners' link previews can't act. |
| **Email** | Uses the same item pages, so email gains one-click actions without session links. "List-Unsubscribe" gets a real one-click endpoint (RFC 8058) with a per-delivery token. |
| **Full brief, Today, Settings** | Normal OIDC session, as today. |

**Item page hardening:**
- `Referrer-Policy: no-referrer`, so the ref doesn't leak to the
  company's ATS when the person clicks Apply.
- No third-party scripts.
- Open-graph tags carry only public job facts.

**Replay and staleness:**
- Actions are idempotent (§15).
- An expired `ref` redirects to sign-in.
- Inbound message ids are deduplicated for at least 30 days, which
  covers Meta's 7-day retries.
- A tap on an item older than 30 days answers "This pick is old: open
  Narrow" and records nothing.

**Forwarding.** A forwarded `/j/<ref>` link lets the recipient act on
that one item for up to 7 days.
- Every action is reversible on the web (Save ↔ Unsave, a rejected job
  can be saved again).
- The page says "This link is personal".
- *Inference:* this is acceptable for the convenience it buys. If not, require sign-in for
  Not for me and hiding on the page.

**Binding codes:**
- single-use;
- 15 minutes;
- only the hash is stored;
- rate-limited per account.

**Meta secrets:**
- The system-user token and app secret are sealed environment secrets on
  `api` and `worker-notify` only.
- Rotation is documented next to the Resend key.

---

## 23. Data-model implications

**Inspected first** (Code: `20261003000000_feed_notifications.sql`,
`postgres/notify.rs`, `postgres/user.rs`). Most of the domain already
exists:

| Need | Existing | Gap |
|---|---|---|
| Preferences per person | `notification_settings` (email, enabled, cadence, confirmation, lease) | channel, timezone, schedule, quiet hours, pause, alerts flag |
| Channel address | `notification_settings.email` (sealed, confirmed) | a WhatsApp address and its consent; more than one channel |
| Outbox | `notification_deliveries` (sealed message, status, attempts, provider id, kind) | channel, template, delivered/read times, error code, billed category |
| What was notified | `notification_items` (`PRIMARY KEY (user_id, opportunity_id)`, tier, job, versions) | item ref, position, class (digest/alert), freshness class, per-item provider message id, opened, action, via |
| What was shown on the web | `user_opportunities` (first/last shown, dismissed, resurfaced) | none |
| Actions | `feedback` log (append-only, encrypted reason) | optional reason code |
| Inbound dedupe | — | new table |
| Company hiding | — | a profile entity (sealed, synced, exported) |
| Usage | `usage_events` | client values `whatsapp`, `email` |

**Recommended changes.** All are additive, in one migration. Email keeps
working throughout.

```sql
-- One row per (person, channel). Email moves here later; until then the
-- existing email columns stay authoritative for email.
CREATE TABLE notification_channels (
    user_id          text COLLATE "C" NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    channel          text NOT NULL CHECK (channel IN ('email', 'whatsapp')),
    address          bytea NOT NULL,             -- sealed
    address_key      bytea NOT NULL,             -- keyed hash, for inbound routing
    status           text NOT NULL CHECK (status IN ('pending', 'active', 'paused', 'opted_out', 'failing')),
    consent_version  text,                       -- the consent copy shown
    opted_in_at      timestamptz,
    opted_out_at     timestamptz,
    opt_out_source   text,                       -- stop_word | meta_preference | settings | blocked
    last_inbound_at  timestamptz,                -- service window = last_inbound_at + 24h
    created_at       timestamptz NOT NULL,
    updated_at       timestamptz NOT NULL,
    PRIMARY KEY (user_id, channel),
    UNIQUE (channel, address_key)                -- one account per number
);

ALTER TABLE notification_settings
    ADD COLUMN primary_channel   text,           -- NULL = only in the app
    ADD COLUMN timezone          text,           -- IANA
    ADD COLUMN digest_time       time NOT NULL DEFAULT '09:00',
    ADD COLUMN digest_days       smallint NOT NULL DEFAULT 31,   -- bitmask Mon..Sun, 31 = Mon–Fri
    ADD COLUMN digest_size       smallint NOT NULL DEFAULT 3 CHECK (digest_size BETWEEN 1 AND 5),
    ADD COLUMN quiet_start       time NOT NULL DEFAULT '21:00',
    ADD COLUMN quiet_end         time NOT NULL DEFAULT '08:00',
    ADD COLUMN paused_until      timestamptz,
    ADD COLUMN alerts_enabled    boolean NOT NULL DEFAULT false;

ALTER TABLE notification_deliveries
    ADD COLUMN channel           text NOT NULL DEFAULT 'email',
    ADD COLUMN template          text,           -- e.g. digest_3@v1/pt_BR
    ADD COLUMN delivered_at      timestamptz,
    ADD COLUMN read_at           timestamptz,
    ADD COLUMN error_code        text,           -- e.g. 131049
    ADD COLUMN billed_category   text;           -- from the status webhook's pricing object
-- kind gains 'digest', 'alert', 'channel_reply' (the CHECK is replaced).

ALTER TABLE notification_items
    ADD COLUMN ref               bytea UNIQUE,   -- random 16 bytes; payloads and /j/<ref>
    ADD COLUMN position          smallint,
    ADD COLUMN class             text,           -- digest | alert
    ADD COLUMN freshness         text,           -- posted | posted_approx | newly_listed | backfill | unknown | reopened
    ADD COLUMN provider_message_id text,         -- the per-item interactive message, if sent
    ADD COLUMN opened_at         timestamptz,
    ADD COLUMN action            text,           -- save | reject | dismiss | applied | hide_company
    ADD COLUMN action_at         timestamptz,
    ADD COLUMN action_via        text;           -- whatsapp | email | item_page | web | mcp

CREATE TABLE notification_inbound (
    provider             text NOT NULL,
    provider_message_id  text NOT NULL,
    user_id              text COLLATE "C" REFERENCES users (id) ON DELETE CASCADE,
    received_at          timestamptz NOT NULL,
    kind                 text NOT NULL,       -- button | list | text | status | preference
    outcome              text NOT NULL,       -- applied | repeat | ignored | stop | bound | rejected_signature
    PRIMARY KEY (provider, provider_message_id)
);

ALTER TABLE feedback ADD COLUMN reason_code text;   -- optional; the text stays authoritative
```

**Things to note:**
- The `notification_items` **primary key stays** `(user_id,
  opportunity_id)`: once ever, across channels.
- **The existing `cadence` column maps to the new policy:**
  - `daily` → digest.
  - `immediate` → digest plus `alerts_enabled`, but alerts stay gated
    (§13.1). Today's `immediate` emails *any* recommended strong fit
    every 4 h; under this design that becomes the digest until the gate
    passes.
  - This is a notification-policy change that affects existing
    subscribers. Ship it in slice A with a note in Settings.
- **Hidden companies:** a profile entity (`profile_entities` kind
  `hide_rule` with a `company` target, or kind `hidden_company`), sealed,
  synced and exported, not a cloud-only table, so the local product can
  use it too.
- **Freshness** needs no column. Optionally add
  `source_schedule.first_complete_listing_at`.
- **Not needed:**
  - a recommendations table;
  - a separate "WhatsApp state";
  - a per-channel feedback table.

---

## 24. Metrics

**We are not optimizing for:**
- messages sent;
- job count;
- application count;
- open rates.

### North star

**Worth-the-interruption rate (WIR):**
> the share of proactively delivered jobs the person considers worth
> about 30 seconds of attention.

**How it is measured:**
- **Unit:** a delivered item (`notification_items` row whose delivery
  is `sent`).
- **Ground truth** (dogfood, and later a sampled panel):
  - the BRU-328 question asked blind: "Was this job worth about 30
    seconds to decide whether to pursue it?" (Yes/No), plus `impossible`
    and `obvious_avoidable_no`;
  - every delivered item in the week goes into a blind packet with a
    random control drawn from the same day's `C`, reusing
    `worth_review_gate.rs` (on the BRU-328 branch, not `main`);
  - **Judgments are only ever the person's.**
- **Bar:**
  - ≥ 80% WIR;
  - 0 impossible;
  - ≤ 10% obvious avoidable No;
  - ≥ 30 judged items.

**Production proxy** (no extra questions to anyone):
- **Outcome per delivered item within 7 days:**

  | Outcome | Actions |
  |---|---|
  | **positive** | Save / Applied |
  | **negative** | Not for me / hid company |
  | **neutral** | Not now / opened only |
  | **silent** | nothing |

- **Proxy precision** = positive / (positive + negative), reported with
  its **coverage** (positive + negative) / delivered.
- The proxy is calibrated against the ground-truth judgments. It is
  never reported alone.

**Paired outcome metric:** useful deliveries per active person-week
(positive outcomes), so precision can't be "won" by sending nothing.

### Quality and safety

| Metric | Definition |
|---|---|
| Save rate / Apply rate / Not-for-me rate | per delivered item, 7-day window, by class (digest/alert) |
| Not-for-me by reason | reason codes; **"Location"** is counted as a *defect* (an impossible job reached the person), not only a preference |
| Company-hide rate | hides per 100 delivered items |
| Mute / opt-out rate | STOP, Meta marketing opt-outs, pauses, blocks; per 100 digests |
| Repeat rate | same opportunity delivered twice (must be 0; primary key) and same company within 7 days |
| Closed-at-delivery rate | delivered items that were closed by the time of the first tap |
| Empty-day rate | scheduled digests with 0 items (inventory health, not a failure) |
| Delivery failures | by error code (131049 marketing cap, blocked, invalid) |

### Speed

All of these exclude backfill.

| Metric | Definition |
|---|---|
| Posting → discovery | `first_seen_at − posted_at` for `posted` items |
| Discovery → notification | `sent_at − first_seen_at` |
| Delivery → action | `action_at − sent_at` |

### Reach

**% of positive actions taken without a web session that day.**
- `action_via` in (`whatsapp`, `email`, `item_page`).
- It shows whether the product delivers value without the website.

### Cost

Meta spend per useful delivery, from `billed_category` × the rate card.

---

## 25. Product positioning

### Candidates

| Candidate | Assessment |
|---|---|
| "Narrow searches the job market for you." | True but passive; says nothing about the interruption contract |
| "Narrow continuously searches thousands of companies and sends you the few jobs worth your time." | **Not true yet.** Production reads 21 boards; "thousands" is the discovered universe, not what's monitored |
| "Stop checking job boards. Narrow checks them for you." | Strong hook, but positions against job boards on their terms and promises coverage first |
| **"Narrow watches companies' own job boards and messages you only when a job is worth your time."** | Concrete about the mechanism (first-party boards, Narrow's measured moat: 23/24 fresh first-party postings absent from an aggregator, BRU-343), and about the restraint |

### Recommendation

> **Narrow watches companies' own job boards and messages you only when
> a job is worth your time.**

**Supporting line, on the channel step and in the WhatsApp profile:**

> "Some days that's nothing. That's on purpose."

**Why:**
- It states the product's two differentiators, first-party freshness and
  restraint, without "AI" or numbers that aren't true yet.
- It sets the expectation that silence is normal. Without that, an empty
  digest day reads as broken.
- It survives scale: when monitored boards really are in the thousands, a
  number can be added.

It is consistent with the README's "searches the market for you and
surfaces only the opportunities worth your attention".

---

## 26. MVP

### The MVP

**1. Onboarding cleanup** (§6):
- the pay floor step;
- the optional avoid step;
- the channel step;
- the location step trimmed to 4 inputs;
- seniority confirmation;
- the free-text step moved out;
- `has_preferences` fixed.

**2. Notification policy v2 on the existing email channel:**
- daily digest at the local time, weekdays, quiet hours, pause;
- one per company;
- freshness labels.

**3. Item pages and one-click actions:**
- `/j/<ref>` with Save / Not now / Not for me / I applied;
- Apply on company site;
- attribution.

**4. Company hiding:** web, API, MCP; filters in Today and notifications.

**5. WhatsApp via Meta Cloud API:**
- `wa.me` code binding and opt-in;
- STOP and opt-out handling;
- the digest template;
- "Review here" per-item messages with Save / Not for me / Not now;
- reason list;
- hide-company confirmation;
- status webhooks.

**6. Canonical sync:** WhatsApp and email actions through the existing use
cases, and "Sent on …" markers on Today.

**7. External application label** on every item and page ("Apply on
company site").

**8. Delivery analytics:**
- per-item outcomes;
- the §24 metrics in `narrow admin status`;
- the weekly blind judgment packet for dogfood.

### How this differs from the proposed shape

**Immediate alerts:**
- Built in slice J, behind a flag.
- **Not part of the MVP's promise**, for the reason in §13.0.

**Email first:**
- Items 2–4 ship on **email first**: no vendor wait, no per-message cost.
- They give a week or two of real WIR data before WhatsApp traffic
  starts.
- Meta setup starts on day 1 in parallel: number, display name,
  templates, verification.

**"Applied" via WhatsApp:**
- Through the item page, not a button.
- WhatsApp's 3 reply buttons go to the decisions a list asks for.

**Learned-rule suggestions:**
- Need BRU-328's hide rules on `main` first.
- The MVP includes only the company case (§17). Phrase rules follow as
  slice N.

### Precondition (outside BRU-389)

Activate the first BRU-360 board batch. Otherwise the digest will be
honest and mostly empty, and the WIR measurement will have too few items.

### Explicitly not in the MVP

- Prepare application.
- Assisted submit.
- Push, Slack or Telegram.
- Multiple simultaneous recommendation channels.
- Plausible fits in digests.
- Resurfacing changed jobs by message.
- Weekly summaries.
- Any chat.

---

## 27. Implementation sequence

Ordered. Each slice is shippable and reversible on its own.

### Slice 0 (precondition, separate issue): activate the first board batch

| | |
|---|---|
| **Purpose** | Enough inventory to deliver and measure |
| **Scope** | BRU-360's recommended first batch (Nango, Oneleet, g2i, Ando, Veda, then Sezzle, Coderoad, Camunda, Customer.io, Alpaca, RevenueCat), with a rollout check |
| **Dependencies** | — |
| **Risk** | Low |
| **Acceptance** | The new boards are read in production; Today's distinct companies on the dogfood profile are measured before and after |

### A. Notification policy v2 (email)

| | |
|---|---|
| **Purpose** | The digest/alert/none policy with limits, on the channel that exists |
| **Scope** | Settings columns (§23); digest composer in `worker notify` (local time, days, quiet hours, pause, one per company, 7-day company spacing); freshness classifier and labels; `cadence` migration; Settings UI |
| **Dependencies** | — |
| **Risk** | Low/medium: timezone and DST bugs |
| **Acceptance (tests)** | No delivery when 0 candidates; ≤ 1 digest per local day across DST changes; nothing inside quiet hours; never two items from one company; never an item twice; backfill items never labelled "posted"/"new"; existing email tests unchanged |

### B. Channel-agnostic notification domain

| | |
|---|---|
| **Purpose** | Channels behind one trait; per-item refs, events, outcomes |
| **Scope** | `notification_channels`; `channel`/`template`/status columns; item `ref`, `class`, `freshness`, outcome columns; `ChannelSender` generalizing `EmailSender`; delivery events; usage `client` values |
| **Dependencies** | A |
| **Risk** | Medium: migration on live tables (all additive) |
| **Acceptance** | A fake channel passes the same outbox contract tests as email; every delivered item has a ref and an outcome slot; email behavior byte-identical |

### C. Item pages and actions

| | |
|---|---|
| **Purpose** | One-tap actions from any message without session links |
| **Scope** | `/j/<ref>` (public facts, Apply on company site, POST actions, CSRF token, `no-referrer`, 7-day validity); RFC 8058 one-click unsubscribe; `action_via` attribution |
| **Dependencies** | B |
| **Risk** | Medium: security surface |
| **Acceptance** | GET never changes state (link-preview test); expired ref redirects to sign-in; actions produce the same feedback events as the web; replay is a no-op; a security review of the page passes |

### D. Company hiding

| | |
|---|---|
| **Purpose** | Visible, reversible company suppression |
| **Scope** | Profile entity (sealed, synced, exported); `GET/POST /api/v1/hidden-companies`; MCP tool; Preferences list with undo; "Hide all jobs at X" in the Not-for-me dialog; filters in feed and notification candidates; label in search |
| **Dependencies** | — (parallel to A) |
| **Risk** | Low |
| **Acceptance** | Hidden company's jobs never in Today, digests or alerts; matched by name *or* board; undo restores on the next feed; other accounts and source monitoring unaffected |

### E. Meta Cloud API integration

| | |
|---|---|
| **Purpose** | Send and receive on WhatsApp |
| **Scope** | Meta app, WABA (BRL), number, display name; templates `digest_1..3`, `alert_1` in `pt_BR` and `en`; sender (Graph API); `POST/GET /webhooks/whatsapp` (verify token, `X-Hub-Signature-256`, persist and acknowledge ≤ 250 ms); status events → items/deliveries; `billed_category` |
| **Dependencies** | B; Meta setup can start on day 1 |
| **Risk** | Medium: template categorization, review delays |
| **Acceptance** | Bad signature → 401 and nothing recorded; a duplicated webhook is a no-op; delivered/read/failed update the item; an end-to-end digest to the test number |

### F. WhatsApp opt-in and opt-out

| | |
|---|---|
| **Purpose** | Consent-correct identity binding |
| **Scope** | Consent copy and versioning; binding code (`wa.me` link + QR, 15 min, single-use, hashed); STOP words; `user_preferences` opt-out; error 131049, blocked and invalid handling; Settings and onboarding channel step |
| **Dependencies** | E |
| **Risk** | Medium: policy compliance |
| **Acceptance** | No message ever to an unbound, paused or opted-out number (property test over the sender); STOP takes effect before the next send; one number ↔ one account |

### G. WhatsApp digest and inbound actions

| | |
|---|---|
| **Purpose** | The product loop on WhatsApp |
| **Scope** | Digest rendering (template, or free-form inside an open window); "Review here" → per-item interactive messages; Save / Not for me / Not now; reason list; hide-company confirmation; late, closed and repeat replies; one daily fixed reply to free text |
| **Dependencies** | C, D, F |
| **Risk** | Medium |
| **Acceptance** | A tap is visible on the web on the next request; actions equal the web's; repeats answered "Already …"; no outbound message that isn't a digest, alert or a reply to a tap |

### H. Onboarding simplification

| | |
|---|---|
| **Purpose** | About 3–4 minutes to a useful profile |
| **Scope** | Screens in §6 on existing records; pay step; avoid step; channel step; seniority confirm; `has_preferences` fix; persisted skip |
| **Dependencies** | — (parallel; the channel step links to A, or to F once WhatsApp exists) |
| **Risk** | Low/medium: changing who gets which preferences |
| **Acceptance** | ≤ 6 screens; required inputs as listed; the same `Preference`/taste records as today's controls produce (contract test); median completion ≤ 5 min in a 5-person hallway test |

### I. Delivery analytics and dogfood judgments

| | |
|---|---|
| **Purpose** | Measure WIR honestly |
| **Scope** | §24 metrics in `narrow admin status`; weekly blind packet of delivered items + control (reuse `worth_review_gate.rs`); outcome report |
| **Dependencies** | B (A is enough for email) |
| **Risk** | Low |
| **Acceptance** | One week's report with every metric; judgments only from the person, never inferred |

### J. Immediate alerts (flagged)

| | |
|---|---|
| **Purpose** | Fresh, unusually actionable jobs, early |
| **Scope** | §13.1 rule; `alert_1` template; caps; per-person flag; global switch |
| **Dependencies** | A, B, I; for WhatsApp, G |
| **Risk** | High (attention) — hence the gate |
| **Acceptance** | Rule tests (backfill never alerts; caveats block; caps hold); **enabled for a person only after I shows ≥ 80% WIR on ≥ 30 judged deliveries with 0 impossible** |

### K. Application capability model

| | |
|---|---|
| **Purpose** | Honest application calls to action |
| **Scope** | Derived capability per opportunity; "Apply on company site" everywhere; Greenhouse question-schema reader (public `?questions=true`) |
| **Dependencies** | — |
| **Risk** | Low |
| **Acceptance** | Every verified open job shows External; no "Apply with Narrow" string exists; capability changes with the record's ATS |

### L. Prepare application MVP

| | |
|---|---|
| **Purpose** | Reduce application effort without submitting |
| **Scope** | The kit in §19 (resume, usable facts, standard answers, start date asked here, Greenhouse questions with required flags) |
| **Dependencies** | K |
| **Risk** | Medium: evidence-policy correctness |
| **Acceptance** | Only usable facts appear (existing `POLICY`); EEO/demographic and consent never pre-filled; salary blank by default; no network call to any ATS other than reading public schemas |

### M. Assisted-apply research

| | |
|---|---|
| **Purpose** | Decide whether submission is ever legitimate |
| **Scope** | Partner-program conversations (Greenhouse Candidate Ingestion, SmartRecruiters Application API, Lever partner OAuth); terms review; a decision memo. **No code.** |
| **Dependencies** | — |
| **Risk** | Low |
| **Acceptance** | A memo per program: requirements, employer opt-in, timeline, go/no-go |

### N. Learned hide-rule suggestions

| | |
|---|---|
| **Purpose** | Feedback that visibly changes later deliveries |
| **Scope** | Merge BRU-328's hide rules; offers in §18 (one explicit reason, or a pattern of 3; company after 2); offers inside open WhatsApp windows and on the web |
| **Dependencies** | BRU-328 code on `main`; G |
| **Risk** | Medium: false suppressions |
| **Acceptance** | No rule without a confirmation; every rule shows its origin; BRU-328 §10's hits, false suppressions and escapes reported on delivered items |

### Order and parallelism

```text
0 ─┐
   ├─ A ─ B ─┬─ C ─┐
   │          ├─ E ─ F ─┴─ G ─ N
   ├─ D ─────────────────┘
   ├─ H (anytime)
   └─ I (after A/B) ─ J ;  K ─ L ;  M (anytime)
```

---

## 28. Open risks

| Risk | Detail |
|---|---|
| **Inventory** | With 21 boards, most days will have nothing to send. The design is right to send nothing, but the product feels dead and WIR can't be measured. Slice 0 is a hard precondition for learning anything. |
| **Strong-fit quality** | The digest exposes the ~23% strict precision directly to the person, without Today's context. If WIR comes in well under 80%, the answer is *not* new Fit rules (that line is closed). It is fewer items, BRU-328's "worth reviewing" framing, and faster feedback (hide rules, company hiding). The risk is churn before that loop works. |
| **Meta categorization and cost** | Digests are Marketing at $0.0625 in Brazil, with per-user marketing caps that can silently drop a digest (131049). The person can opt out of marketing in WhatsApp without telling Narrow (handled via webhook). Mitigated by Today being canonical, window reuse, and one paid message per day at most. |
| **Meta pricing in flux** | Meta's pages disagree about in-window charges after 2026-10-01. Confirm from the first `pricing` webhooks before committing to a cost model. |
| **Duplicate sends on WhatsApp** | There is no provider idempotency key, so a crash between Meta accepting a message and Narrow recording it can resend once. Accept this for the MVP and record `provider_message_id` before marking sent; email keeps its stronger guarantee. |
| **Freshness evidence** | `posted_at` is absent for YC and career pages and approximate for Lever. Scanning gaps make `newly_listed` unknowable. Alerts may then rarely qualify, which is the correct failure. |
| **Link capability forwarding** | A forwarded `/j/<ref>` lets someone act on one item for 7 days. Every action is reversible; tighten if it is ever abused. |
| **WhatsApp in-app browser** | The browser has no Narrow session, so "See why" means signing in on a phone. The item page is designed to be useful without signing in. |
| **Policy drift** | Meta's WhatsApp policy, opt-in rules and "AI Provider" definitions changed several times in 2025–2026. Keep the consent copy versioned (`consent_version`) and re-check before launch. |
| **Onboarding regressions** | Moving the free-text step out of onboarding loses some company/team taste at the start. That is acceptable by design (company prefers gate Strong), but measure Strong count before and after on new profiles. |
| **Local product** | Proactive delivery is cloud-only. The local CLI keeps working without it. The README's "local product" framing needs an honest note that the agent features need JobHunt Cloud. |

---

## 29. Final recommendation

**1. Should Narrow become proactive/agent-first?**
- **Yes.**
- Proactive delivery is a channel into the canonical Today queue, built
  by generalizing the existing email notification outbox.
- One ranking, one state, one set of actions.

**2. Should WhatsApp be the first proactive channel?**
- **Yes, as the first new channel and the primary one for Brazilian
  users.**
- But the new policy goes live on the existing email channel first:
  - email is free per message and already built;
  - WhatsApp job alerts are paid Marketing messages with per-user caps;
  - so it's the wrong place to debug selection.

**3. Which provider?**
- **Meta WhatsApp Cloud API, directly.**
- No business verification needed to start (250 recipients/day).
- No markup. Twilio adds $0.005 per message each way, 74% on service
  messages.
- BRL billing.
- A portable number.
- The existing outbox already does what Twilio would add.

**4. Exact onboarding questions:**

| # | Screen | Question | Answer type |
|---|---|---|---|
| 1 | Resume | Upload your resume, then "Your next role's level" | Mid / Senior / Staff+, prefilled |
| 2 | Role | "What kind of role are you looking for?" | up to 3 role families, plus an optional title |
| 3 | Where | "Where do you live?" | prefilled |
| 3 | Where | "Where are you authorized to work?" | prefilled with residence |
| 3 | Where | "How do you want to work?" | Remote only / Remote or hybrid / On-site is fine too |
| 3 | Where | "Would you relocate for a job?" | No / Yes / Only to… |
| 4 | Pay | "What's the least you'd accept?" | amount, currency (required), per year/month/hour, or No minimum |
| 4 | Pay | "Jobs that don't publish pay" | show (default) / hide |
| 5 | Avoid *(optional)* | "Anything Narrow should avoid?" | chips (management, consulting/agencies, large companies, very early-stage, heavy on-call, crypto, very intense pace) + specific companies + something else |
| 6 | Reach | "How should Narrow reach you?" | WhatsApp / Email / Only in the app, plus time and days, with timezone detected |

**5. What should be inferred instead?**
- Seniority, from the latest title (confirmed in one tap).
- Authorization defaulting to residence (confirmed).
- Sponsorship need.
- Remote hiring scopes that include the person.
- Time-zone overlap.
- Notification timezone.
- Technologies and domains, as evidence only.

**6. What should Narrow learn from behavior?**
- Specialties to hide, offered as explicit rules.
- Companies to hide, offered after 2 rejections.
- Company/team/ownership/pace preferences.
- Domain and technology preferences.
- Seniority drift.

All through the existing learned-taste patterns (soft, visible) plus
confirmed hide rules. Never a permanent rule from one ambiguous click.

**7. Maximum reasonable notification volume:**

| Limit | Value |
|---|---|
| Digests | 1 per day, weekdays by default |
| Jobs per digest | 3 (5 if chosen) |
| Alerts (when enabled) | ≤ 1/day, ≤ 3/week |
| All proactive messages | ≤ 2/day and ≤ 8/week |
| Quiet hours | 21:00–08:00 local |

**8. What qualifies for an immediate alert?**

All of these must hold:
- a recommended strong fit (verified, eligible) that is new to the
  person;
- **Eligible** (not conditional);
- **no caveats**;
- **fresh by evidence:** posted ≤ 24 h ago from a dated source, or seen
  appearing on an already-monitored board ≤ 24 h ago, never a backfill;
- verified within 12 h;
- no alert for that company in 14 days;
- inside the caps, outside quiet hours.

Off by default. Enabled per person only after delivered digests reach ≥
80% worth-the-interruption on ≥ 30 judged items with 0 impossible.

**9. What belongs in the digest?**
- The verified, eligible, new part of Today: strong fits with
  `Gate::Recommended`, never shown or acted on, never notified.
- One per company.
- Spaced 7 days per company when possible.
- Up to 3, in rank order.
- No plausible fits until BRU-328 passes.

**10. What happens when there are zero good jobs?**
- **Nothing is sent.**
- No "nothing today", no recap, no filler.
- Today says "caught up".

**11. Save / Not for me / Not now / Applied across web and WhatsApp:**
- **Same use cases, same append-only feedback log, same repeat rule.**

| Action | Effect |
|---|---|
| Save | saved everywhere |
| Not for me | removed from Today and all future notifications, with an optional reason |
| Not now | the existing dismiss: gone until a material change, nothing learned |
| Applied | the pipeline; on WhatsApp via the item page |

- Taps are idempotent (by inbound id and state), conflicts resolve by
  time order, and the channel is attribution only.

**12. How should company-level suppression work?**
- A private, visible, reversible "hidden company": matched by normalized
  name **or** source board.
- **Removes from:** Today and all notifications.
- **Labelled in:** search.
- **Not touched:** the pipeline.
- **Never affects:** ranking, taste, monitoring, or other people.
- **Created only explicitly:** "Not for me → This company → Hide Nango",
  Preferences, or onboarding.

**13. What are the application capability states?**
- `external` ("Apply on company site").
- `prepare` ("Prepare application").
- `assisted_submit` ("Apply with Narrow"): reserved; never shown without
  a partner agreement and a tested endpoint.
- `unavailable`.

They are derived per posting from its ATS and verification.

**14. What can realistically be built for application assistance next?**
- A **Prepare** kit:
  - the resume;
  - evidence-policy facts (`application-context` exists);
  - standard answers, with salary blank by default;
  - start date asked once;
  - for Greenhouse, the job's real questions from its public schema.
- The person copies and applies on the company site.
- No ATS documents a candidate-side submission path. Assisted submit is a
  partnership question (Greenhouse, SmartRecruiters, Lever), not an
  engineering one.

**15. What is the smallest MVP?**
- Onboarding cleanup.
- Digest policy v2 on email.
- Item pages with one-click actions.
- Company hiding.
- WhatsApp:
  - opt-in;
  - digest;
  - Save / Not for me / Not now with reasons and hide-company.
- Canonical sync.
- "Apply on company site".
- Delivery analytics with weekly blind judgments.

It depends on activating the first BRU-360 board batch.

**16. What should we explicitly NOT build yet?**
- Immediate alerts as default.
- Prepare or assisted submission.
- Auto-apply.
- Any chat or free-text agent.
- Push, Slack or Telegram.
- More than one recommendation channel at a time.
- Plausible fits in messages.
- Resurfacing by message.
- Weekly recaps or engagement nudges.
- An "Activity" vanity page.
- New ranking rules or Today thresholds.

**17. Next implementation slices, in order:**
- 0: activate the first board batch.
- **A:** digest policy on email.
- **B:** channel domain.
- **C:** item pages and actions.
- **D:** company hiding (parallel with A).
- **E:** Meta integration (setup from day 1).
- **F:** WhatsApp opt-in.
- **G:** WhatsApp digest and actions.
- **H:** onboarding (parallel).
- **I:** analytics and judgments.
- **J:** alerts (flagged, gated on I).
- **K:** capability model.
- **L:** Prepare MVP.
- **M:** assisted-apply partner research.
- **N:** learned hide-rule suggestions.

---

## Sources

Fetched 2026-10-11. For Meta pages, `[M]` stands for
`https://developers.facebook.com/documentation/business-messaging/whatsapp`.

### WhatsApp

- [W1] [M]/get-started
- [W2] [M]/messaging-limits
- [W3] [M]/about-the-platform
- [W4] [M]/business-phone-numbers/phone-numbers
- [W5] [M]/solution-providers/support/migrating-phone-numbers-among-solution-partners-programmatically
- [W6] [M]/upcoming-messaging-limits-changes
- [W7] [M]/templates/template-categorization
- [W8] [M]/templates/template-review
- [W9] [M]/templates/supported-languages
- [W10] [M]/templates/components
- [W11] [M]/messages/send-messages
- [W12] [M]/messages/interactive-reply-buttons-messages, [M]/messages/interactive-list-messages, [M]/messages/interactive-cta-url-messages, [M]/messages/interactive-media-carousel-messages
- [W13] [M]/webhooks/create-webhook-endpoint, [M]/webhooks/overview
- [W14] https://www.twilio.com/docs/whatsapp/self-sign-up
- [W15] https://www.twilio.com/docs/whatsapp/best-practices-and-faqs
- [W16] https://www.twilio.com/en-us/voice/pricing/br
- [W17] https://www.twilio.com/docs/content/content-api-resources
- [W18] https://www.twilio.com/docs/content/twilio-quick-reply
- [W19] [M]/throughput
- [W20] [M]/webhooks/reference/messages/status, [M]/webhooks/reference/messages/button
- [W21] https://www.twilio.com/docs/messaging/guides/outbound-message-status-in-status-callbacks
- [W22] https://www.twilio.com/docs/usage/webhooks/webhooks-security
- [W23] [M]/pricing (and its USD/BRL rate cards "effective October 1, 2026"); https://whatsappbusiness.com/products/platform-pricing/
- [W24] [M]/pricing/non-template-messages, [M]/pricing/ai-providers
- [W25] https://www.twilio.com/en-us/whatsapp/pricing
- [W26] [M]/templates/marketing-templates/per-user-limits, [M]/templates/marketing-templates; opt-in: [M]/getting-opt-in, https://whatsappbusiness.com/policy/
- [W27] https://360dialog.com/pricing

### ATS

- [A1] https://docs.greenhouse.io/job-board.html
- [A2] https://support.greenhouse.io/hc/en-us/articles/115005448066
- [A3] https://docs.greenhouse.io/candidate-ingestion.html
- [A4] https://www.greenhouse.com/become-a-greenhouse-partner-integration-request-form
- [A5] https://github.com/lever/postings-api
- [A6] https://help.lever.co/hc/en-us/articles/20087243347741 (search snippet; page renders in JavaScript)
- [A7] https://hire.sandbox.lever.co/developer/oauth
- [A8] https://developers.ashbyhq.com/docs/public-job-posting-api
- [A9] https://developers.ashbyhq.com/docs/creating-a-custom-careers-page
- [A10] https://developers.ashbyhq.com/reference/applicationformsubmit
- [A11] https://developers.ashbyhq.com/docs/authentication
- [A12] https://docs.ashbyhq.com/job-board-application-spam-protection
- [A13] https://docs.ashbyhq.com/application-limits
- [A14] https://workable.readme.io/reference/job-candidates-create
- [A15] https://help.workable.com/hc/en-us/articles/115012771647-Using-the-Workable-API-to-create-a-careers-page
- [A16] https://workable.readme.io/docs/partner-token (from search results; 404 when fetched)
- [A18] https://doc.workday.com/admin-guide/en-us/human-capital-management/recruiting/career-sites/gtv1538650489786.html
- [A20] https://developers.smartrecruiters.com/docs/application-api
- [A21] https://developers.smartrecruiters.com/docs/authentication
- [A22] https://developers.smartrecruiters.com/docs/the-smartrecruiters-platform
- [A23] https://docs.recruitee.com/reference/offers
- [A24] https://docs.recruitee.com/reference/offersoffer_idcandidates
- [A25] https://support.recruitee.com/en/articles/6929909-add-captcha-to-your-application-forms
- [A27] https://support.personio.de/hc/en-us/articles/360000314338 (403; search-result content)
- [A28] https://developer.personio.de/v1.0/reference/post_v1-recruiting-applications
- [A29] https://documentation.bamboohr.com/reference/create-candidate
- [A30] https://support.teamtailor.com/en/articles/5963369-use-our-teamtailor-api
- [A31] https://teamtailor.notion.site/Integrate-with-Teamtailor-3919690568754aaaa7ffd681afe955f2
- [A32] https://www.resumatorapi.com/v1/
- [A33] https://developer.breezy.hr/
- [A34] https://developer.rippling.com/documentation/job-board-api
- [A35] https://support.greenhouse.io/hc/en-us/articles/17073067102747-LinkedIn-Apply-Connect-integration, https://learn.microsoft.com/en-us/linkedin/talent/apply-connect, https://docs.indeed.com/indeed-apply/ats
- [A37] https://help.workable.com/hc/en-us/articles/35293126257815-Managing-AI-generated-and-automated-job-applications

### Narrow (this repository)

- `docs/cloud.md` (email notifications, schema, workers)
- `docs/taste-profile.md` (BRU-321/324)
- `docs/fit-and-practicality.md`, `docs/ranking-invariants.md`
- `docs/real-posting-precision-experiment.md` (BRU-325)
- `docs/role-vs-company-fit-experiment.md`, `docs/role-breadth-fit-experiment.md`
- `docs/source-discovery-and-career-pages.md` (BRU-343)
- `docs/broad-discovery.md`, `docs/broad-discovery-experiment-2026-10-05.md` (BRU-360)
- BRU-328 pre-registration and hide rules: branch `t3code/worth-reviewing-blind-gate` (`docs/today-worth-reviewing-dogfood.md`), not merged
