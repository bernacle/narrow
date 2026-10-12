import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import type { NotificationSettingsView } from "@/lib/api-types";

import { violations } from "../../test/axe";
import { NotificationSettings, inQuietHours } from "./notification-settings";

function view(overrides: Partial<NotificationSettingsView> = {}): NotificationSettingsView {
  return {
    available: true,
    email_enabled: true,
    cadence: "immediate",
    email: "ana@example.com",
    email_status: "confirmed",
    digest_time: "09:00",
    digest_days: ["mon", "tue", "wed", "thu", "fri"],
    digest_size: 3,
    quiet_start: "21:00",
    quiet_end: "08:00",
    next_digest_at: "2026-10-13T12:00:00Z",
    timezone: "America/Sao_Paulo",
    max_items: 5,
    recent: [
      { id: "ntf_1", kind: "recommendations", status: "sent", opportunities: 2, created_at: "2026-10-12T12:00:00Z", sent_at: "2026-10-12T12:00:05Z" },
    ],
    ...overrides,
  };
}

describe("NotificationSettings", () => {
  it("describes the daily digest, its next slot and the change for existing subscribers", async () => {
    const save = vi.fn();
    const { container } = render(<NotificationSettings initial={view()} save={save} />);
    expect(screen.getByLabelText("Email me a daily digest")).toBeChecked();
    expect(screen.getByText(/At most 3 jobs worth reviewing, once a day, one per company/)).toBeInTheDocument();
    expect(screen.getByText(/one digest a day instead of an email per match/)).toBeInTheDocument();
    // 12:00 UTC is 09:00 in São Paulo.
    expect(screen.getByText(/Next digest Tue 13 Oct, 09:00, if anything qualifies/)).toBeInTheDocument();
    expect(screen.getByLabelText("Time")).toHaveValue("09:00");
    expect(screen.getByLabelText("Mon")).toBeChecked();
    expect(screen.getByLabelText("Sat")).not.toBeChecked();
    await userEvent.click(screen.getByText("Recent emails"));
    expect(screen.getByText(/Digest · 2 jobs · sent/)).toBeInTheDocument();
    expect(await violations(container)).toEqual([]);
  });

  it("saves the whole schedule, and refuses a digest time inside quiet hours", async () => {
    const save = vi.fn().mockResolvedValue({ ok: true, data: view({ digest_time: "10:30" }) });
    render(<NotificationSettings initial={view()} save={save} />);
    const time = screen.getByLabelText("Time");
    await userEvent.clear(time);
    await userEvent.type(time, "07:00");
    expect(screen.getByText(/inside your quiet hours/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Save schedule" })).toBeDisabled();
    await userEvent.clear(time);
    await userEvent.type(time, "10:30");
    await userEvent.click(screen.getByLabelText("Sat"));
    await userEvent.selectOptions(screen.getByLabelText("Jobs per digest, at most"), "5");
    await userEvent.click(screen.getByRole("button", { name: "Save schedule" }));
    await waitFor(() => expect(save).toHaveBeenCalled());
    expect(save.mock.calls[0]![0]).toEqual({
      timezone: "America/Sao_Paulo",
      digest_time: "10:30",
      digest_days: ["mon", "tue", "wed", "thu", "fri", "sat"],
      digest_size: 5,
      quiet_start: "21:00",
      quiet_end: "08:00",
    });
  });

  it("pauses for a week and resumes", async () => {
    const until = new Date(Date.now() + 7 * 24 * 3600 * 1000).toISOString();
    const save = vi.fn().mockResolvedValue({ ok: true, data: view({ paused_until: until, next_digest_at: null }) });
    render(<NotificationSettings initial={view()} save={save} />);
    await userEvent.click(screen.getByRole("button", { name: "Pause for a week" }));
    await waitFor(() => expect(screen.getByText(/Paused until/)).toBeInTheDocument());
    const paused = save.mock.calls[0]![0].paused_until as string;
    expect(new Date(paused).getTime()).toBeGreaterThan(Date.now() + 6 * 24 * 3600 * 1000);
    save.mockResolvedValue({ ok: true, data: view() });
    await userEvent.click(screen.getByRole("button", { name: "Resume" }));
    await waitFor(() => expect(save).toHaveBeenLastCalledWith({ paused_until: null }));
  });

  it("turning it on saves the browser's timezone when none is set", async () => {
    const save = vi.fn().mockResolvedValue({ ok: true, data: view() });
    render(<NotificationSettings initial={view({ email_enabled: false, timezone: null })} save={save} />);
    expect(screen.getByText(/Narrow uses UTC until you save/)).toBeInTheDocument();
    await userEvent.click(screen.getByLabelText("Email me a daily digest"));
    await waitFor(() => expect(save).toHaveBeenCalled());
    const request = save.mock.calls[0]![0];
    expect(request.email_enabled).toBe(true);
    expect(request.timezone).toBe(Intl.DateTimeFormat().resolvedOptions().timeZone);
  });
});

describe("inQuietHours", () => {
  it("wraps past midnight and treats equal bounds as none", () => {
    expect(inQuietHours("22:00", "21:00", "08:00")).toBe(true);
    expect(inQuietHours("07:59", "21:00", "08:00")).toBe(true);
    expect(inQuietHours("08:00", "21:00", "08:00")).toBe(false);
    expect(inQuietHours("13:00", "12:00", "14:00")).toBe(true);
    expect(inQuietHours("03:00", "00:00", "00:00")).toBe(false);
  });
});
