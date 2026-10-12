import { spawn } from "node:child_process";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import AxeBuilder from "@axe-core/playwright";
import { type Page, expect } from "@playwright/test";

import { API_URL, FIXTURES_URL, JOBHUNT_BIN, MAIL_FILE, cloudEnv, databaseUrl } from "./env.mjs";

export const RESUME = fileURLToPath(new URL("../../../crates/jobhunt-resume/tests/fixtures/ana_lima.md", import.meta.url));
export const STATEMENT =
  "I want backend and platform roles at small product teams, remote, at least USD 120k. No pure SRE roles. Something about good vibes.";

/** Signs in with the development sign-in (the stack's API runs in dev auth mode). */
export async function signIn(page: Page, name: string, next = "/today") {
  await page.goto(`/signin?next=${encodeURIComponent(next)}`);
  await page.getByLabel("Your name").fill(name);
  await page.getByRole("button", { name: "Continue" }).click();
}

/** Onboards an account through the API (for tests about other screens). */
export async function onboardViaApi(name: string) {
  const token = ((await (
    await fetch(`${API_URL}/api/v1/auth/dev-token`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ subject: name }),
    })
  ).json()) as { access_token: string }).access_token;
  const auth = { authorization: `Bearer ${token}` };
  const upload = await fetch(`${API_URL}/api/v1/profile/resume?file_name=ana_lima.md`, {
    method: "PUT",
    headers: auth,
    body: readFileSync(RESUME),
  });
  expect(upload.ok).toBe(true);
  const prefs = await fetch(`${API_URL}/api/v1/preferences`, {
    method: "POST",
    headers: { ...auth, "content-type": "application/json" },
    body: JSON.stringify({ statement: STATEMENT }),
  });
  expect(prefs.ok).toBe(true);
}

/** Runs one scheduled job of the stack (`narrow worker notify`, …). */
export function runWorker(kind: "notify" | "verification" | "discovery"): Promise<Record<string, unknown>> {
  return new Promise((resolve, reject) => {
    const child = spawn(JOBHUNT_BIN, ["worker", kind], { env: cloudEnv() });
    let out = "";
    let err = "";
    child.stdout.on("data", (d) => (out += d));
    child.stderr.on("data", (d) => (err += d));
    child.on("close", (code) => (code === 0 ? resolve(JSON.parse(out)) : reject(new Error(`worker ${kind} failed: ${err}`))));
  });
}

export interface SentEmail {
  idempotency_key: string;
  message: { to: string; subject: string; text: string; html: string };
}

/** Everything the stack "sent" (the file email provider). */
export function sentEmails(): SentEmail[] {
  try {
    return readFileSync(MAIL_FILE, "utf8")
      .split("\n")
      .filter(Boolean)
      .map((line) => JSON.parse(line) as SentEmail);
  } catch {
    return [];
  }
}

/**
 * Automated accessibility checks (WCAG A/AA rules), in a real browser.
 * Once any entrance animation has finished: a sheet fading in is measured
 * at the colors it settles on, not halfway through.
 */
export async function expectAccessible(page: Page) {
  // (A spinner's endless turn is not waited for.)
  await page.waitForFunction(() =>
    document.getAnimations().every((a) => a.playState !== "running" || a.effect?.getTiming().iterations === Infinity),
  );
  const results = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"]).analyze();
  expect(results.violations.map((v) => `${v.id}: ${v.help} — ${v.nodes.map((n) => n.target.join(" ")).join(", ")}`)).toEqual([]);
}

/**
 * Makes every subscriber's daily digest due now (as if the clock had just
 * reached their digest time on a new day): UTC, every day, no quiet hours,
 * the digest time a few minutes ago, today's slot unused.
 */
export async function makeDigestDue() {
  await new Promise<void>((resolve, reject) => {
    const psql = spawn("psql", [
      databaseUrl(),
      "-q",
      "-c",
      `UPDATE notification_settings SET timezone = 'UTC', digest_days = 127,
         quiet_start = '00:00', quiet_end = '00:00', last_digest_on = NULL,
         digest_time = (now() AT TIME ZONE 'UTC')::time
           - least(interval '20 minutes', (now() AT TIME ZONE 'UTC')::time - time '00:00')`,
    ]);
    psql.on("close", (code) => (code === 0 ? resolve() : reject(new Error("psql failed"))));
  });
}

/**
 * A new opening appears on a fixture board, and the next scheduled/**
 * A new opening appears on a fixture board, and the next scheduled
 * discovery reads that board (as the cron would once it is due).
 */
export async function publishLaterJobs(board: string) {
  const answer = await fetch(`${FIXTURES_URL}/__fixtures/publish/${board}`, { method: "POST" });
  expect(answer.ok).toBe(true);
  await new Promise<void>((resolve, reject) => {
    const psql = spawn("psql", [
      databaseUrl(),
      "-q",
      "-c",
      `UPDATE source_schedule SET next_due_at = now() WHERE source_instance = '${board.replace(/'/g, "")}'`,
    ]);
    psql.on("close", (code) => (code === 0 ? resolve() : reject(new Error("psql failed"))));
  });
}
