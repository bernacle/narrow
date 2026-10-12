import { expect, test } from "@playwright/test";

import { RESUME, STATEMENT, expectAccessible, makeDigestDue, publishLaterJobs, runWorker, sentEmails, signIn } from "./helpers";

/**
 * The main loop, as a person does it: sign in, upload a resume, choose the
 * kinds of role they want, say what else they care about, open Today, review, reject one with a reason, save another,
 * mark one applied, check Applications and Preferences, get an email about
 * a strong match they haven't seen, sign out and back in, and find
 * everything as they left it.
 */
test.describe.serial("the product loop", () => {
  const name = "e2e-ana";
  let rejected = "";
  let saved = "";
  let applied = "";

  test("sign in and onboard", async ({ page }) => {
    await signIn(page, name);
    await expect(page).toHaveURL(/\/welcome$/);
    await page.getByLabel("Your resume", { exact: true }).setInputFiles(RESUME);
    await page.getByRole("button", { name: "Import" }).click();
    // The step folds into one line once it's done.
    await expect(page.getByText(/ana_lima\.md · \d+ roles? found/)).toBeVisible();

    // The one structured question: the kinds of role, a few chips.
    const roles = page.getByRole("form", { name: "What kind of role are you looking for?" });
    await roles.getByRole("checkbox", { name: "Backend" }).check();
    await roles.getByRole("checkbox", { name: "Platform" }).check();
    await roles.getByRole("button", { name: "Continue" }).click();
    await expect(page.getByText("Backend · Platform", { exact: true })).toBeVisible();

    // Anything else, in their words; Narrow reads it into a short summary.
    await page.getByRole("textbox", { name: "Anything else you care about?" }).fill(STATEMENT);
    await page.getByRole("button", { name: "Continue" }).click();
    const understood = page.getByRole("region", { name: "What Narrow understands" });
    // The kinds of role they chose aren't repeated as Narrow's reading.
    await expect(understood.getByRole("list", { name: "What you want" }).getByText(/Backend engineering/)).toHaveCount(0);
    await expect(understood.getByRole("list", { name: "What you avoid" }).getByText(/SRE/)).toBeVisible();
    // What it couldn't place is said, never dropped.
    await expect(understood.getByText(/Something about good vibes/)).toBeVisible();
    await expect(page.getByRole("list", { name: "Your practical constraints" }).getByText(/USD 120,000/)).toBeVisible();
    await understood.getByRole("button", { name: "Looks right" }).click();
    await expect(understood.getByText("You confirmed this")).toBeVisible();
    await page.getByRole("link", { name: "Go to Today" }).click();
    await expect(page).toHaveURL(/\/today$/);
  });

  test("Today is a short list of verified, explained recommendations", async ({ page }) => {
    await signIn(page, name);
    const cards = page.getByRole("list", { name: "Recommendations" }).getByRole("article");
    await expect(cards.first()).toBeVisible();
    const count = await cards.count();
    expect(count).toBeGreaterThanOrEqual(3);
    expect(count).toBeLessThanOrEqual(5);
    await expect(page.getByText(/of the \d+ open jobs Narrow checked/)).toBeVisible();
    // Only strong fits: no maybes, never the unwanted SRE role or the
    // marketing job.
    await expect(page.getByText("Maybe", { exact: true })).toHaveCount(0);
    await expect(page.getByText("Site Reliability Engineer")).toHaveCount(0);
    await expect(page.getByText("Marketing Manager")).toHaveCount(0);
    // One raised lead, then peers side by side.
    await expect(page.getByText("Also worth a look")).toBeVisible();
    // Facts, a reason, verification and the four decisions on every card;
    // everything else behind See evidence.
    const decisions = /^(Not now|Not for me|I applied|Save)$/;
    for (let i = 0; i < count; i++) {
      const card = cards.nth(i);
      await expect(card.getByText(/USD [0-9,]+ – [0-9,]+ per year/)).toBeVisible();
      await expect(card.getByText(/Verified (just now|\d+ (min|h) ago)/)).toBeVisible();
      await expect(card.getByRole("button", { name: /^See evidence/ })).toBeVisible();
      await expect(card.getByRole("button", { name: decisions })).toHaveText(["Not now", "Not for me", "I applied", "Save"]);
    }
    await expect(cards.first().getByText(/on the employer's job board/)).toBeVisible();
    // Peers in one row share their geometry: titles and actions line up.
    const peers = page.getByRole("list", { name: "Also worth a look" }).getByRole("article");
    const [a, b] = [peers.nth(0), peers.nth(1)];
    for (const part of [(c: typeof a) => c.getByRole("heading", { level: 2 }), (c: typeof a) => c.getByRole("button", { name: "Save" })]) {
      const [ya, yb] = [(await part(a).boundingBox())!.y, (await part(b).boundingBox())!.y];
      expect(Math.abs(ya - yb)).toBeLessThan(1);
    }
    // The evidence: every reason and concern, on demand, and Escape closes it.
    await cards.first().getByRole("button", { name: /^See evidence/ }).click();
    const evidence = page.getByRole("dialog", { name: "Why Narrow surfaced it" });
    await expect(evidence.getByText("Things to consider")).toBeVisible();
    await expect(evidence.getByRole("link", { name: "Open the full brief" })).toBeVisible();
    await expectAccessible(page);
    await page.keyboard.press("Escape");
    await expect(evidence).toBeHidden();
    await expect(cards.first().getByRole("button", { name: /^See evidence/ })).toBeFocused();
    // Tiers are words, never scores.
    await expect(page.locator("main")).not.toContainText(/\d+\s?%/);
    // No infinite scroll: the page ends.
    await expect(page.getByText("That's everything new worth your time.")).toBeVisible();
    await expectAccessible(page);

    // Reloading keeps the same recommendations.
    const titles = await cards.getByRole("heading", { level: 2 }).allTextContents();
    await page.reload();
    await expect(cards.getByRole("heading", { level: 2 })).toHaveText(titles);
  });

  test("the full brief explains one opportunity", async ({ page }) => {
    await signIn(page, name);
    const first = page.getByRole("list", { name: "Recommendations" }).getByRole("article").first();
    const title = (await first.getByRole("heading", { level: 2 }).textContent()) ?? "";
    await first.getByRole("link", { name: title, exact: true }).click();
    await expect(page.getByRole("heading", { level: 1, name: title })).toBeVisible();
    // The conclusion first: the verdict, then each key fact once, then the description.
    await expect(page.getByText(/^Looks unusually aligned|^Worth a look/).first()).toBeVisible();
    for (const fact of ["Pay", "Location", "Eligibility", "Listing"]) {
      await expect(page.getByRole("group", { name: fact, exact: true })).toBeVisible();
    }
    await expect(page.getByRole("region", { name: "Description" })).toBeVisible();
    await expect(page.getByText(/^Verified .* on the employer's job board/)).toHaveCount(1);
    // The reasoning, the checks and the provenance are one labelled action away.
    await expect(page.getByText(/A compatibility signal/)).toBeHidden();
    await page.getByRole("button", { name: "Checks for eligibility" }).click();
    await expect(page.getByRole("dialog", { name: "Eligibility checks" }).getByText(/A compatibility signal/)).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.getByRole("button", { name: "Checks for eligibility" })).toBeFocused();
    await page.getByRole("button", { name: "Sources of the listing" }).click();
    await expect(page.getByRole("dialog", { name: "Where it's listed" }).getByRole("link", { name: /^Greenhouse · / })).toBeVisible();
    await page.keyboard.press("Escape");
    await page.getByRole("button", { name: "Full reasoning" }).click();
    await expect(page.getByRole("dialog", { name: "Why Narrow surfaced it" }).getByText(/Narrow read it as/)).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.getByRole("navigation", { name: "Breadcrumb" }).getByRole("link", { name: "Today" })).toBeVisible();
    await expectAccessible(page);
  });

  test("reject with a reason, save, mark applied", async ({ page }) => {
    await signIn(page, name);
    const cards = page.getByRole("list", { name: "Recommendations" }).getByRole("article");
    await expect(cards.nth(2)).toBeVisible();
    const titles = await cards.getByRole("heading", { level: 2 }).allTextContents();
    [rejected, saved, applied] = titles as [string, string, string];

    // Keyboard only, for the rejection.
    const firstCard = cards.nth(0);
    await firstCard.getByRole("button", { name: "Not for me" }).focus();
    await page.keyboard.press("Enter");
    const dialog = page.getByRole("dialog", { name: "Not for me" });
    await expect(dialog).toBeVisible();
    await expect(dialog.getByLabel(/What didn't fit/)).toBeFocused();
    await page.keyboard.press("Escape");
    await expect(dialog).toBeHidden();
    await firstCard.getByRole("button", { name: "Not for me" }).click();
    await dialog.getByLabel(/What didn't fit/).fill("On-call heavy");
    await dialog.getByRole("button", { name: "Company too big" }).click();
    await dialog.getByRole("button", { name: "Mark not for me" }).click();
    await expect(cards.nth(0).getByRole("status")).toContainText("Won't be recommended again");

    await cards.nth(1).getByRole("button", { name: "Save" }).click();
    await expect(cards.nth(1).getByRole("status")).toContainText("Saved");
    await cards.nth(2).getByRole("button", { name: "I applied" }).click();
    await expect(cards.nth(2).getByRole("status")).toContainText("Marked as applied");

    // A fresh Today no longer shows any of the three.
    await page.reload();
    await expect(page.getByRole("list", { name: "Recommendations" }).getByRole("article").first()).toBeVisible();
    const remaining = await page.getByRole("list", { name: "Recommendations" }).getByRole("heading", { level: 2 }).allTextContents();
    for (const title of [rejected, saved, applied]) expect(remaining).not.toContain(title);
  });

  test("Applications tracks the pipeline", async ({ page }) => {
    await signIn(page, name, "/applications");
    const appliedSection = page.getByRole("region", { name: /Applied/ });
    await expect(appliedSection.getByRole("link", { name: applied, exact: true })).toBeVisible();
    await expect(page.getByRole("region", { name: /Saved/ }).getByRole("link", { name: saved, exact: true })).toBeVisible();
    await expect(page.getByRole("link", { name: rejected, exact: true })).toBeHidden();
    await appliedSection.getByRole("button", { name: new RegExp(`^Interviewing — ${applied.replace(/[()]/g, "\\$&")}`) }).click();
    await expect(page.getByRole("region", { name: /Interview/ }).getByRole("link", { name: applied, exact: true })).toBeVisible();
    // The stage tabs narrow the list to one stage.
    await page.getByRole("tab", { name: /^Saved/ }).click();
    await expect(page.getByRole("tabpanel").getByRole("link", { name: saved, exact: true })).toBeVisible();
    await expect(page.getByRole("tabpanel").getByRole("link", { name: applied, exact: true })).toBeHidden();
    await page.getByRole("tab", { name: /^All/ }).click();
    await page.getByText(/Not for me \(1\)/).click();
    await expect(page.getByRole("link", { name: rejected, exact: true })).toBeVisible();
    await expect(page.getByText("On-call heavy; company too big")).toBeVisible();
    await expectAccessible(page);
  });

  test("Preferences keep what was said apart from what was learned", async ({ page }) => {
    await signIn(page, name, "/preferences");
    await expect(page.getByRole("region", { name: "What you're looking for" }).getByText("Backend · Platform")).toBeVisible();
    await expect(page.getByRole("region", { name: "Anything else you care about?" }).getByText(STATEMENT)).toBeVisible();
    // What was learned from decisions is its own section, never "you said".
    await expect(page.getByRole("region", { name: "Learned over time" }).getByText(/You tend to pass on large companies/)).toBeVisible();
    await page.locator("summary").filter({ hasText: "Edit constraints" }).click();
    await page.locator("summary").filter({ hasText: "Fine-tune" }).click();
    await expect(page.getByRole("group", { name: "Minimum", exact: true }).getByText("At least USD 120,000 per year")).toBeVisible();
    const more = page.getByRole("region", { name: "Roles, domains and more" });
    await expect(more.getByRole("group", { name: "Role" }).filter({ hasText: "backend roles" })).toContainText("Want");
    // A structured preference, the same model the API and assistants use.
    await more.getByText("Add a preference").click();
    await page.getByLabel("About", { exact: true }).selectOption("domain");
    await page.getByLabel("Rule").selectOption("avoid");
    await page.getByLabel("Value").fill("adtech");
    await page.getByRole("button", { name: "Add preference" }).click();
    await expect(page.getByText("Saved.")).toBeVisible();
    await expect(more.getByRole("group", { name: "Domain" }).filter({ hasText: "adtech" })).toContainText("Avoid");
    await expect(page.getByRole("region", { name: "In your words" }).getByText(STATEMENT)).toBeVisible();
    // Everything Narrow uses, in its layers: what was learned stays apart.
    await page.getByRole("button", { name: "Review all" }).click();
    const review = page.getByRole("dialog", { name: "What Narrow uses" });
    await expect(review.getByRole("region", { name: "Preferences" }).getByText("Want: backend roles")).toBeVisible();
    await expect(review.getByRole("region", { name: "Learned" }).getByText(/You tend to pass on large companies/)).toBeVisible();
    await expectAccessible(page);
  });

  test("Profile shows the model and takes claim decisions", async ({ page }) => {
    await signIn(page, name, "/profile");
    await expect(page.getByRole("region", { name: "Sources" }).getByText(/^ana_lima\.md\s*current$/)).toBeVisible();
    await expect(page.getByRole("heading", { name: "Experience" })).toBeVisible();
    await expectAccessible(page);
    // Review is one row on Profile, and its own page.
    await page.getByRole("link", { name: "Review", exact: true }).click();
    await expect(page.getByRole("heading", { level: 1, name: "Review claims" })).toBeVisible();
    const review = page.getByRole("region", { name: "Needs your review" });
    // The evidence behind a claim is on demand, beside its actions.
    await review.getByRole("button", { name: /^See evidence/ }).first().click();
    await expect(page.getByRole("dialog").getByText("What your answer does")).toBeVisible();
    await page.keyboard.press("Escape");
    const before = await review.getByText(/claims? needs? your review/).textContent();
    await review.getByRole("button", { name: /^Confirm/ }).first().click();
    await expect(review.getByText(/Confirmed: it can be used as evidence/)).toBeVisible();
    expect(await review.getByText(/claims? needs? your review|Nothing left/).textContent()).not.toBe(before);
    await expectAccessible(page);
  });

  test("a daily digest of a strong match not seen yet, once", async ({ page }) => {
    await signIn(page, name, "/settings");
    await page.getByLabel("Send to").fill("e2e-ana@example.com");
    await page.getByRole("button", { name: "Use this address" }).click();
    await expect(page.getByText(/sent a confirmation link/)).toBeVisible();
    const confirmation = sentEmails().find((m) => m.message.to === "e2e-ana@example.com");
    expect(confirmation?.message.subject).toBe("Confirm your email for Narrow notifications");
    const link = /http:\/\/127\.0\.0\.1:3100\/settings\/confirm\?token=\w+/.exec(confirmation!.message.text)![0];
    await page.goto(link);
    await expect(page.getByRole("heading", { name: "Email confirmed" })).toBeVisible();
    await page.goto("/settings");
    await page.getByLabel("Email me a daily digest").check();
    await expect(page.getByText("Saved. The daily digest is on.")).toBeVisible();
    // Saved on the server, not just on screen.
    await page.reload();
    await expect(page.getByLabel("Email me a daily digest")).toBeChecked();
    await expect(page.getByText(/Next digest .*, if anything qualifies/)).toBeVisible();
    await expectAccessible(page);

    // Everything strong was already seen on Today: nothing to email.
    await makeDigestDue();
    const quiet = await runWorker("notify");
    expect(quiet.sent, JSON.stringify(quiet)).toBe(0);
    expect(quiet.nothing_new).toBe(1);

    // A new opening appears; the scheduled workers find and verify it.
    await publishLaterJobs("latecomer");
    const discovered = await runWorker("discovery");
    expect(discovered.new).toBe(1);
    await runWorker("verification");
    // The next day's digest.
    await makeDigestDue();
    const first = await runWorker("notify");
    expect(first.sent, JSON.stringify(first)).toBe(1);
    const emails = sentEmails().filter((m) => m.message.to === "e2e-ana@example.com" && !m.message.subject.startsWith("Confirm"));
    expect(emails).toHaveLength(1);
    const email = emails[0]!.message;
    // A strong fit the person hasn't seen on Today; nothing already dealt with.
    expect(email.subject).toBe("Narrow found 1 job worth a look");
    expect(email.text).toContain("1. Latecomer\nStaff Backend Engineer, Payments Platform");
    expect(email.text).toContain("Some days Narrow sends nothing. That's on purpose.");
    for (const title of [rejected, saved, applied]) expect(email.text).not.toContain(title);
    expect(email.text).toContain("http://127.0.0.1:3100/opportunities/opp_");
    // One digest a day: nothing more today.
    const second = await runWorker("notify");
    expect(second.sent).toBe(0);
    expect(sentEmails().filter((m) => m.message.to === "e2e-ana@example.com")).toHaveLength(2);
  });

  test("signing out and back in keeps everything", async ({ page }) => {
    await signIn(page, name, "/settings");
    await page.getByRole("button", { name: "Sign out", exact: true }).first().click();
    await expect(page).toHaveURL(/\/signin\?signed_out=1/);
    await page.goto("/today");
    await expect(page).toHaveURL(/\/signin\?next=%2Ftoday/);
    await signIn(page, name);
    await expect(page.getByRole("heading", { level: 1, name: "Today" })).toBeVisible();
    // Past the loading skeleton: the feed itself is there.
    await expect(page.getByRole("list", { name: "Recommendations" }).getByRole("article").first()).toBeVisible();
    const titles = await page.getByRole("list", { name: "Recommendations" }).getByRole("heading", { level: 2 }).allTextContents();
    for (const title of [rejected, saved, applied]) expect(titles).not.toContain(title);
    await page.goto("/applications");
    await expect(page.getByRole("region", { name: /Interview/ }).getByRole("link", { name: applied, exact: true })).toBeVisible();
    await expect(page.getByRole("region", { name: /Saved/ }).getByRole("link", { name: saved, exact: true })).toBeVisible();
  });

  test("dealing with everything leaves you caught up", async ({ page }) => {
    await signIn(page, name);
    const caughtUp = page.getByRole("heading", { name: "You're caught up." });
    const list = page.getByRole("list", { name: "Recommendations" });
    for (let round = 0; round < 5; round++) {
      // The loading skeleton has the "Today" heading too: wait for the feed
      // itself (a list, or caught up) before counting what's on it.
      await expect(list.or(caughtUp)).toBeVisible();
      if (await caughtUp.isVisible()) break;
      const cards = list.getByRole("article");
      const count = await cards.count();
      for (let i = 0; i < count; i++) {
        await cards.nth(i).getByRole("button", { name: "Not now" }).click();
        await expect(cards.nth(i).getByRole("status")).toContainText("Put aside");
      }
      await page.reload();
    }
    await expect(page.getByRole("heading", { name: "You're caught up." })).toBeVisible();
    await expect(page.getByText(/0 jobs/)).toHaveCount(0);
    await expect(page.getByRole("link", { name: /1 saved/ })).toBeVisible();
    await expectAccessible(page);
  });
});
