import { describe, expect, it, beforeEach, afterEach, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { AdminAlerts } from "./AdminAlerts";
import * as platform from "../api/platform";
import type { AdminAlert } from "../api/platform";

(
  globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

function alert(over: Partial<AdminAlert> = {}): AdminAlert {
  return {
    id: "a1",
    kind: "malicious_ticket",
    ticket_id: null,
    detail: "flagged",
    created_at: "2026-06-01T00:00:00Z",
    seen: false,
    ...over,
  };
}

let container: HTMLDivElement;
let root: Root;

function mount(props: Partial<Parameters<typeof AdminAlerts>[0]> = {}) {
  const onOpenTicket = vi.fn();
  const onSeenChange = vi.fn();
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => {
    root.render(
      <AdminAlerts
        onClose={() => {}}
        onOpenTicket={onOpenTicket}
        onSeenChange={onSeenChange}
        {...props}
      />,
    );
  });
  return { onOpenTicket, onSeenChange };
}

async function flush() {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
  });
}

beforeEach(() => {
  vi.restoreAllMocks();
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

describe("AdminAlerts", () => {
  it("lists alerts newest-first with a kind badge", async () => {
    vi.spyOn(platform.platformApi, "listAdminAlerts").mockResolvedValue([
      alert({ id: "old", created_at: "2026-01-01T00:00:00Z" }),
      alert({
        id: "new",
        kind: "user_banned",
        created_at: "2026-06-01T00:00:00Z",
      }),
    ]);
    mount();
    await flush();
    const rows = container.querySelectorAll('[data-testid^="admin-alert-"]');
    // Two rows (excludes the mark/ticket buttons via the prefix match? no —
    // those share the prefix). Filter to the row li elements only.
    const items = container.querySelectorAll(".alert-row");
    expect(items).toHaveLength(2);
    expect(rows.length).toBeGreaterThanOrEqual(2);
    expect(items[0].getAttribute("data-testid")).toBe("admin-alert-new");
  });

  it("shows an empty state when there are none", async () => {
    vi.spyOn(platform.platformApi, "listAdminAlerts").mockResolvedValue([]);
    mount();
    await flush();
    expect(
      container.querySelector('[data-testid="admin-alerts-empty"]'),
    ).toBeTruthy();
  });

  it("links into the ticket when ticket_id is set", async () => {
    vi.spyOn(platform.platformApi, "listAdminAlerts").mockResolvedValue([
      alert({ id: "a1", ticket_id: "t9" }),
    ]);
    const { onOpenTicket } = mount();
    await flush();
    const link = container.querySelector<HTMLButtonElement>(
      '[data-testid="admin-alert-ticket-a1"]',
    )!;
    act(() => link.click());
    expect(onOpenTicket).toHaveBeenCalledWith("t9");
  });

  it("marks an alert seen and reports the new unseen count", async () => {
    const list = vi
      .spyOn(platform.platformApi, "listAdminAlerts")
      .mockResolvedValueOnce([alert({ id: "a1", seen: false })]) // initial load
      .mockResolvedValueOnce([]) // unseen recount after marking
      .mockResolvedValueOnce([alert({ id: "a1", seen: true })]); // reload
    const markSeen = vi
      .spyOn(platform.platformApi, "markAdminAlertSeen")
      .mockResolvedValue(undefined);
    const { onSeenChange } = mount();
    await flush();
    const btn = container.querySelector<HTMLButtonElement>(
      '[data-testid="admin-alert-mark-a1"]',
    )!;
    act(() => btn.click());
    await flush();
    expect(markSeen).toHaveBeenCalledWith("a1");
    expect(onSeenChange).toHaveBeenCalledWith(0);
    expect(list).toHaveBeenCalled();
  });

  it("filters unseen when the toggle is checked", async () => {
    const list = vi
      .spyOn(platform.platformApi, "listAdminAlerts")
      .mockResolvedValue([]);
    mount();
    await flush();
    const toggle = container.querySelector<HTMLInputElement>(
      '[data-testid="admin-alerts-unseen-toggle"] input',
    )!;
    // A native click toggles `checked` and fires React's onChange.
    act(() => toggle.click());
    await flush();
    expect(toggle.checked).toBe(true);
    expect(list).toHaveBeenLastCalledWith(true);
  });

  it("surfaces an error when the list fails", async () => {
    vi.spyOn(platform.platformApi, "listAdminAlerts").mockRejectedValue(
      new platform.ApiError(403, "admin only"),
    );
    mount();
    await flush();
    expect(
      container.querySelector('[data-testid="admin-alerts-error"]')
        ?.textContent,
    ).toContain("admin only");
  });
});
