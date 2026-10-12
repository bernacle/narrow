"use client";

import { useEffect, useId, useState, useTransition } from "react";

import type { ActionResult } from "@/app/actions";
import type { NotificationSettingsView, UpdateNotificationsRequest } from "@/lib/api-types";
import { ago } from "@/lib/format";

import { Button, Notice, helpClass, inlineActionClass, inputClass, labelClass, selectClass } from "./ui";

type Save = (request: UpdateNotificationsRequest) => Promise<ActionResult<NotificationSettingsView>>;

const choice =
  "size-4 shrink-0 cursor-pointer accent-[var(--nr-fg-primary)] disabled:cursor-default";

const DAYS: [string, string][] = [
  ["mon", "Mon"],
  ["tue", "Tue"],
  ["wed", "Wed"],
  ["thu", "Thu"],
  ["fri", "Fri"],
  ["sat", "Sat"],
  ["sun", "Sun"],
];

const WEEK = 7 * 24 * 3600 * 1000;

/** The browser's IANA timezone, if it knows one. */
function browserTimezone(): string | undefined {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || undefined;
  } catch {
    return undefined;
  }
}

function timezones(current: string): string[] {
  let all: string[] = [];
  try {
    all = Intl.supportedValuesOf("timeZone");
  } catch {
    all = [];
  }
  return Array.from(new Set([current, "UTC", ...all])).filter(Boolean);
}

/** "09:00" as minutes after midnight. */
function minutes(time: string): number {
  const [h, m] = time.split(":").map(Number);
  return (h ?? 0) * 60 + (m ?? 0);
}

/** Whether `time` falls in quiet hours `start`–`end` (wrapping past midnight; none when equal). */
export function inQuietHours(time: string, start: string, end: string): boolean {
  const [t, s, e] = [minutes(time), minutes(start), minutes(end)];
  if (s === e) return false;
  return s < e ? t >= s && t < e : t >= s || t < e;
}

/** "Tue 13 Oct, 09:00" in the schedule's timezone. */
function when(iso: string, timeZone: string): string {
  try {
    return new Date(iso).toLocaleString("en-GB", {
      weekday: "short",
      day: "numeric",
      month: "short",
      hour: "2-digit",
      minute: "2-digit",
      timeZone,
    });
  } catch {
    return new Date(iso).toUTCString();
  }
}

/**
 * The daily digest: once a day at most, a few jobs worth reviewing, and
 * nothing at all when nothing qualifies. No instant alerts, no recaps, no
 * reminders to come back.
 */
export function NotificationSettings({
  initial,
  suggestedEmail,
  save,
}: {
  initial: NotificationSettingsView;
  suggestedEmail?: string;
  save: Save;
}) {
  const [settings, setSettings] = useState(initial);
  const [email, setEmail] = useState(initial.email ?? suggestedEmail ?? "");
  const [schedule, setSchedule] = useState(() => ({
    timezone: initial.timezone ?? "UTC",
    digest_time: initial.digest_time ?? "09:00",
    digest_days: initial.digest_days ?? ["mon", "tue", "wed", "thu", "fri"],
    digest_size: initial.digest_size ?? 3,
    quiet_start: initial.quiet_start ?? "21:00",
    quiet_end: initial.quiet_end ?? "08:00",
  }));
  const [pending, startTransition] = useTransition();
  const [message, setMessage] = useState<{ tone: "error" | "success"; text: string } | null>(null);
  const ids = {
    toggle: useId(),
    toggleHelp: useId(),
    email: useId(),
    help: useId(),
    time: useId(),
    timezone: useId(),
    timezoneHelp: useId(),
    size: useId(),
    quietStart: useId(),
    quietEnd: useId(),
    quietHelp: useId(),
  };

  // Until a timezone is saved, suggest the browser's (after hydration: the
  // server can't know it).
  useEffect(() => {
    const detected = browserTimezone();
    // eslint-disable-next-line react-hooks/set-state-in-effect -- a browser-only value, read once
    if (!initial.timezone && detected) setSchedule((s) => ({ ...s, timezone: detected }));
  }, [initial.timezone]);

  const apply = (request: UpdateNotificationsRequest, success?: string) => {
    // Toggles show at once; the API's answer replaces them, and a refusal
    // puts them back.
    const before = settings;
    if (typeof request.email_enabled === "boolean") setSettings({ ...settings, email_enabled: request.email_enabled });
    startTransition(async () => {
      setMessage(null);
      const r = await save(request);
      if (r.ok) {
        setSettings(r.data);
        if (success) setMessage({ tone: "success", text: success });
      } else {
        setSettings(before);
        setMessage({ tone: "error", text: `${r.title}. ${r.message}` });
      }
    });
  };

  if (!settings.available) {
    return <Notice title="Email isn't available on this Narrow service yet.">Everything still shows up on Today.</Notice>;
  }

  const addressChanged = email.trim() !== (settings.email ?? "");
  const timeZone = settings.timezone ?? "UTC";
  const paused = settings.paused_until && new Date(settings.paused_until) > new Date() ? settings.paused_until : null;
  const quietClash = inQuietHours(schedule.digest_time, schedule.quiet_start, schedule.quiet_end);
  const size = settings.digest_size ?? 3;
  return (
    <div>
      <div className="flex items-start gap-3 border-t border-line-subtle py-3.5">
        <input
          id={ids.toggle}
          type="checkbox"
          className={`${choice} mt-0.5`}
          checked={settings.email_enabled}
          disabled={pending}
          aria-describedby={ids.toggleHelp}
          onChange={(e) => {
            const on = e.target.checked;
            const request: UpdateNotificationsRequest = { email_enabled: on };
            if (on && addressChanged && email.trim()) request.email = email.trim();
            // The schedule needs a timezone: the browser's, until one is chosen.
            if (on && !settings.timezone) request.timezone = schedule.timezone;
            apply(request, on ? "Saved. The daily digest is on." : "Saved. Emails are off.");
          }}
        />
        <div>
          <label htmlFor={ids.toggle} className="cursor-pointer text-[14px] font-medium">
            Email me a daily digest
          </label>
          <p id={ids.toggleHelp} className="mt-0.5 text-[13px] leading-normal text-fg-secondary">
            At most {size} {size === 1 ? "job" : "jobs"} worth reviewing, once a day, one per company. Some days nothing: Narrow sends only
            what qualifies.
          </p>
          {settings.email_enabled && settings.cadence === "immediate" && (
            <p className="mt-1 text-[13px] leading-normal text-fg-muted">
              Narrow now sends one digest a day instead of an email per match.
            </p>
          )}
          {settings.email_enabled && (
            <p className="mt-1 text-[13px] leading-normal text-fg-secondary" aria-live="polite">
              {paused ? (
                <>
                  Paused until {when(paused, timeZone)}.{" "}
                  <button
                    type="button"
                    className={`${inlineActionClass} text-[13px] underline underline-offset-2`}
                    disabled={pending}
                    onClick={() => apply({ paused_until: null }, "Resumed.")}
                  >
                    Resume
                  </button>
                </>
              ) : (
                <>
                  {settings.next_digest_at && `Next digest ${when(settings.next_digest_at, timeZone)}, if anything qualifies. `}
                  <button
                    type="button"
                    className={`${inlineActionClass} text-[13px] underline underline-offset-2`}
                    disabled={pending}
                    onClick={() => apply({ paused_until: new Date(Date.now() + WEEK).toISOString() }, "Paused for a week.")}
                  >
                    Pause for a week
                  </button>
                </>
              )}
            </p>
          )}
        </div>
      </div>

      <form
        className="border-t border-line-subtle py-3.5"
        aria-label="Digest schedule"
        onSubmit={(e) => {
          e.preventDefault();
          if (quietClash || schedule.digest_days.length === 0) return;
          apply({ ...schedule }, "Saved.");
        }}
      >
        <fieldset disabled={pending} className="grid gap-3 sm:grid-cols-2">
          <legend className="mb-2 text-[13px] text-fg-muted">When it comes</legend>
          <div>
            <label htmlFor={ids.time} className={labelClass}>
              Time
            </label>
            <input
              id={ids.time}
              type="time"
              step={900}
              required
              value={schedule.digest_time}
              onChange={(e) => setSchedule({ ...schedule, digest_time: e.target.value })}
              className={inputClass}
            />
          </div>
          <div>
            <label htmlFor={ids.timezone} className={labelClass}>
              Timezone
            </label>
            <select
              id={ids.timezone}
              value={schedule.timezone}
              onChange={(e) => setSchedule({ ...schedule, timezone: e.target.value })}
              aria-describedby={ids.timezoneHelp}
              className={selectClass}
            >
              {timezones(schedule.timezone).map((tz) => (
                <option key={tz} value={tz}>
                  {tz.replaceAll("_", " ")}
                </option>
              ))}
            </select>
            <p id={ids.timezoneHelp} className={helpClass}>
              {settings.timezone ? "Times are in this timezone." : "Not saved yet: Narrow uses UTC until you save."}
            </p>
          </div>
          <fieldset className="sm:col-span-2">
            <legend className={labelClass}>Days</legend>
            <div className="flex flex-wrap gap-x-4 gap-y-1">
              {DAYS.map(([value, label]) => (
                <label key={value} className="flex min-h-8 cursor-pointer items-center gap-2 text-[14px] max-sm:min-h-11">
                  <input
                    type="checkbox"
                    className={choice}
                    checked={schedule.digest_days.includes(value)}
                    onChange={(e) =>
                      setSchedule({
                        ...schedule,
                        digest_days: e.target.checked
                          ? DAYS.map(([d]) => d).filter((d) => d === value || schedule.digest_days.includes(d))
                          : schedule.digest_days.filter((d) => d !== value),
                      })
                    }
                  />
                  {label}
                </label>
              ))}
            </div>
          </fieldset>
          <div>
            <label htmlFor={ids.size} className={labelClass}>
              Jobs per digest, at most
            </label>
            <select
              id={ids.size}
              value={schedule.digest_size}
              onChange={(e) => setSchedule({ ...schedule, digest_size: Number(e.target.value) })}
              className={selectClass}
            >
              {Array.from({ length: settings.max_items }, (_, i) => i + 1).map((n) => (
                <option key={n} value={n}>
                  {n}
                </option>
              ))}
            </select>
          </div>
          <div>
            <span className={labelClass}>Quiet hours</span>
            <div className="flex items-center gap-2">
              <label htmlFor={ids.quietStart} className="sr-only">
                Quiet from
              </label>
              <input
                id={ids.quietStart}
                type="time"
                step={900}
                required
                value={schedule.quiet_start}
                aria-describedby={ids.quietHelp}
                onChange={(e) => setSchedule({ ...schedule, quiet_start: e.target.value })}
                className={inputClass}
              />
              <span className="text-[13px] text-fg-muted">to</span>
              <label htmlFor={ids.quietEnd} className="sr-only">
                Quiet until
              </label>
              <input
                id={ids.quietEnd}
                type="time"
                step={900}
                required
                value={schedule.quiet_end}
                aria-describedby={ids.quietHelp}
                onChange={(e) => setSchedule({ ...schedule, quiet_end: e.target.value })}
                className={inputClass}
              />
            </div>
            <p id={ids.quietHelp} className={helpClass}>
              Nothing is sent in these hours.
            </p>
          </div>
        </fieldset>
        {quietClash && <p className="mt-2 text-[13px] text-danger">The digest time is inside your quiet hours. Choose a time outside them.</p>}
        {schedule.digest_days.length === 0 && <p className="mt-2 text-[13px] text-danger">Choose at least one day.</p>}
        <Button type="submit" disabled={pending || quietClash || schedule.digest_days.length === 0} className="mt-3 h-9 max-sm:h-11">
          Save schedule
        </Button>
      </form>

      <form
        className="border-t border-line-subtle pt-3.5"
        onSubmit={(e) => {
          e.preventDefault();
          if (email.trim()) apply({ email: email.trim() }, "We sent a confirmation link. Nothing else is sent until you follow it.");
        }}
      >
        <label htmlFor={ids.email} className={labelClass}>
          Send to
        </label>
        <div className="flex flex-col gap-2 sm:flex-row">
          <input
            id={ids.email}
            type="email"
            autoComplete="email"
            required
            value={email}
            onChange={(e) => setEmail(e.target.value)}
            aria-describedby={ids.help}
            className={inputClass}
          />
          <Button type="submit" disabled={pending || !addressChanged} className="h-9 shrink-0 max-sm:h-11">
            {settings.email ? "Change address" : "Use this address"}
          </Button>
        </div>
        <p id={ids.help} className="mt-1.5 text-caption text-fg-muted">
          {settings.email_status === "confirmed" && "Confirmed."}
          {settings.email_status === "unconfirmed" && (
            <>
              Waiting for you to follow the link we sent
              {settings.confirmation_sent_at && ` ${ago(settings.confirmation_sent_at)}`}.{" "}
              <button
                type="button"
                className={`${inlineActionClass} text-[12px] underline underline-offset-2`}
                disabled={pending}
                onClick={() => apply({ resend_confirmation: true }, "Sent again.")}
              >
                Send it again
              </button>
            </>
          )}
          {settings.email_status === "none" && "We'll send a link to confirm the address first."}
        </p>
      </form>

      <p aria-live="polite" className={`mt-3 text-[13px] empty:mt-0 ${message?.tone === "error" ? "text-danger" : "text-fg-secondary"}`}>
        {message?.text}
      </p>

      {settings.recent.length > 0 && (
        <details className="group mt-3 text-[13px]">
          <summary className="inline-flex cursor-pointer list-none items-center gap-1.5 text-fg-secondary hover:text-fg max-sm:min-h-11">
            <span aria-hidden="true" className="text-fg-muted transition-transform duration-[120ms] group-open:rotate-90">
              ›
            </span>
            Recent emails
          </summary>
          <ul className="mt-2 space-y-1 font-mono text-mono-s text-fg-muted">
            {settings.recent.map((d) => (
              <li key={d.id}>
                {d.kind === "confirmation" ? "Address confirmation" : `Digest · ${d.opportunities} ${d.opportunities === 1 ? "job" : "jobs"}`} ·{" "}
                {d.status} · {ago(d.sent_at ?? d.created_at)}
              </li>
            ))}
          </ul>
        </details>
      )}
    </div>
  );
}
