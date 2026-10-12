"use server";

import { refresh } from "next/cache";
import { redirect } from "next/navigation";

import { ApiError, api } from "@/lib/api";
import type {
  ClaimDecisionResult,
  CreatedToken,
  FeedbackResult,
  NotificationSettingsView,
  PreferenceInput,
  PreferenceUpdateResult,
  ResumeImportResult,
  SourceImportResult,
  SourceRemovalResult,
  TasteAction,
  TasteUpdateResult,
  UpdateNotificationsRequest,
} from "@/lib/api-types";
import { describeError } from "@/lib/errors";
import { clearSession } from "@/lib/session";

/**
 * Every mutation the web makes, as server actions over the JobHunt API.
 * Nothing here decides anything: it forwards the person's action and
 * returns the API's answer, or a stable error code.
 */
export type ActionResult<T> = { ok: true; data: T } | { ok: false; code: string; title: string; message: string };

async function attempt<T>(work: () => Promise<T>, { explain = false }: { explain?: boolean } = {}): Promise<ActionResult<T>> {
  try {
    return { ok: true, data: await work() };
  } catch (error) {
    const code = error instanceof ApiError ? error.code : "internal_error";
    if (!(error instanceof ApiError)) console.error("action failed", error);
    const described = describeError(code);
    // A preference the domain refused says why in terms the person can act
    // on ("use one of: startup, …"); other messages are never shown raw.
    if (error instanceof ApiError && (code === "invalid_preference" || (explain && code === "invalid_arguments"))) {
      described.message = error.message;
    }
    return { ok: false, ...described };
  }
}

export type FeedbackKind = "save" | "unsave" | "reject" | "applied" | "interview" | "offer" | "like" | "dislike";

export async function recordFeedback(
  id: string,
  action: FeedbackKind,
  reason?: string,
): Promise<ActionResult<FeedbackResult>> {
  const trimmed = reason?.trim();
  return attempt(() => api.feedback(id, { action, reason: trimmed ? trimmed : null }));
}

/** "Not now": off Today, nothing learned. */
export async function putAside(id: string): Promise<ActionResult<FeedbackResult>> {
  return attempt(() => api.dismiss(id));
}

/** Stage changes on the Applications page, then a fresh page. */
export async function changeStage(id: string, action: FeedbackKind, reason?: string): Promise<ActionResult<FeedbackResult>> {
  const result = await recordFeedback(id, action, reason);
  if (result.ok) refresh();
  return result;
}

export interface StatementState {
  result?: PreferenceUpdateResult;
  error?: { title: string; message: string };
  submitted?: string;
}

export async function tellPreferences(_: StatementState, form: FormData): Promise<StatementState> {
  const statement = String(form.get("statement") ?? "").trim();
  if (!statement) return { error: { title: "Say what you want", message: "Write a sentence or two first." } };
  const result = await attempt(() => api.preferences({ statement }));
  if (!result.ok) return { error: result, submitted: statement };
  refresh();
  return { result: result.data, submitted: statement };
}

export interface DescribeState {
  result?: TasteUpdateResult;
  error?: { title: string; message: string };
  submitted?: string;
}

/**
 * "What kind of job are you looking for?": the person's words, kept as
 * written and read into a short summary (by the API: a model when one is
 * configured, Narrow's built-in reader otherwise).
 */
export async function describeTaste(_: DescribeState, form: FormData): Promise<DescribeState> {
  const text = String(form.get("text") ?? "").trim();
  if (!text) return { error: { title: "Say what you're looking for", message: "A sentence or two is enough." } };
  const result = await attempt(() => api.updateTasteProfile({ action: "describe", text }), { explain: true });
  if (!result.ok) return { error: result, submitted: text };
  refresh();
  return { result: result.data, submitted: text };
}

/** Confirm, correct, "doesn't matter", remove, add, or read again. */
export async function reviewTaste(action: TasteAction): Promise<ActionResult<TasteUpdateResult>> {
  const result = await attempt(() => api.updateTasteProfile(action), { explain: true });
  if (result.ok) refresh();
  return result;
}

export async function setPreference(input: PreferenceInput): Promise<ActionResult<PreferenceUpdateResult>> {
  const result = await attempt(() => api.preferences({ set: [input] }));
  if (result.ok) refresh();
  return result;
}

/**
 * Settles a preference Narrow read with an open question: the person's
 * explicit answer replaces the reading, in one update.
 */
export async function clarifyPreference(id: string, set: PreferenceInput[]): Promise<ActionResult<PreferenceUpdateResult>> {
  const result = await attempt(() => api.preferences({ set, remove: [id] }));
  if (result.ok) refresh();
  return result;
}

/**
 * A change from the structured controls: the preferences to set and the
 * ones they replace (ids), in one update. The API decides what each value
 * means; this only forwards the person's choice.
 */
export async function updatePreferences(set: PreferenceInput[], remove: string[] = []): Promise<ActionResult<PreferenceUpdateResult>> {
  const result = await attempt(() => api.preferences({ set, remove }));
  if (result.ok) refresh();
  return result;
}

export async function removePreference(id: string): Promise<ActionResult<PreferenceUpdateResult>> {
  const result = await attempt(() => api.preferences({ remove: [id] }));
  if (result.ok) refresh();
  return result;
}

export async function decideClaim(
  id: string,
  decision: "confirm" | "reject" | "reset",
  note?: string,
): Promise<ActionResult<ClaimDecisionResult>> {
  return attempt(() => api.decideClaims([id], decision, note?.trim() || undefined));
}

export interface ResumeState {
  result?: ResumeImportResult;
  error?: { title: string; message: string };
}

const MAX_RESUME = 10 * 1024 * 1024;

export async function uploadResume(_: ResumeState, form: FormData): Promise<ResumeState> {
  const file = form.get("resume");
  if (!(file instanceof File) || file.size === 0) {
    return { error: { title: "Choose a file", message: "A PDF, .txt or .md resume." } };
  }
  if (file.size > MAX_RESUME) {
    return { error: { title: "That file is too large", message: "Resumes up to 10 MB." } };
  }
  const result = await attempt(async () => api.uploadResume(await file.arrayBuffer(), file.name, file.type));
  if (!result.ok) {
    return {
      error:
        result.code === "invalid_arguments"
          ? { title: "That file couldn't be read", message: "Use a text-based PDF (not a scan), or a .txt or .md file." }
          : result,
    };
  }
  refresh();
  return { result: result.data };
}

export interface SourceState {
  result?: SourceImportResult;
  error?: { title: string; message: string };
}

const MAX_LINKEDIN = 16 * 1024 * 1024;

/** A LinkedIn data export (the .zip, or one of its CSV files). */
export async function uploadLinkedin(_: SourceState, form: FormData): Promise<SourceState> {
  const file = form.get("export");
  if (!(file instanceof File) || file.size === 0) {
    return { error: { title: "Choose a file", message: "The .zip LinkedIn sends, or one of its CSV files." } };
  }
  if (file.size > MAX_LINKEDIN) {
    return {
      error: {
        title: "That file is too large",
        message: "Exports up to 16 MB. Ask LinkedIn for just the files you need (profile, positions, skills…).",
      },
    };
  }
  const result = await attempt(async () => api.uploadLinkedin(await file.arrayBuffer(), file.name));
  if (!result.ok) {
    return {
      error:
        result.code === "invalid_arguments"
          ? {
              title: "That file couldn't be read",
              message: "Use the .zip from LinkedIn (Settings → Data privacy → Get a copy of your data), or one of its CSV files like Positions.csv. Nothing was imported.",
            }
          : result,
    };
  }
  refresh();
  return { result: result.data };
}

/** A public GitHub account. */
export async function importGithub(_: SourceState, form: FormData): Promise<SourceState> {
  const username = String(form.get("username") ?? "").trim();
  const result = await attempt(() => api.importGithub(username || undefined));
  if (!result.ok) {
    return {
      error:
        result.code === "invalid_arguments"
          ? {
              title: "That account couldn't be imported",
              message: "Check the username: a person's public GitHub account. If another account is already imported, remove it first.",
            }
          : result.code === "source_unavailable"
            ? { title: "GitHub couldn't be reached", message: "GitHub didn't answer, or its rate limit is used up. Try again in a while." }
            : result,
    };
  }
  refresh();
  return { result: result.data };
}

/** Takes a LinkedIn export or a GitHub account out of the profile. */
export async function removeSource(source: "linkedin" | "github"): Promise<ActionResult<SourceRemovalResult>> {
  const result = await attempt(() => api.removeSource(source));
  if (result.ok) refresh();
  return result;
}

export async function saveNotifications(request: UpdateNotificationsRequest): Promise<ActionResult<NotificationSettingsView>> {
  return attempt(() => api.updateNotifications(request));
}

export async function createToken(name: string, days: number): Promise<ActionResult<CreatedToken>> {
  const result = await attempt(() => api.createToken(name.trim() || "AI assistant", days));
  if (result.ok) refresh();
  return result;
}

export async function revokeToken(id: string): Promise<ActionResult<void>> {
  const result = await attempt(() => api.revokeToken(id));
  if (result.ok) refresh();
  return result;
}

export async function signOutEverywhere(): Promise<void> {
  await attempt(() => api.logoutEverywhere());
  await clearSession();
  redirect("/signin?signed_out=everywhere");
}

export async function refreshPage(): Promise<void> {
  refresh();
}
