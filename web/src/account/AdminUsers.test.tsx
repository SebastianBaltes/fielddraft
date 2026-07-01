import { describe, expect, it, beforeEach, afterEach, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { AdminUsers } from "./AdminUsers";
import * as platform from "../api/platform";
import type { AdminUser } from "../api/platform";

(
  globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

function user(over: Partial<AdminUser> = {}): AdminUser {
  return {
    id: "u1",
    email: "a@test.com",
    display_name: "Alice",
    role: "user",
    disabled: false,
    created_at: "2026-06-01T00:00:00Z",
    ...over,
  };
}

let container: HTMLDivElement;
let root: Root;

function mount(selfId: string | null = null) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => {
    root.render(<AdminUsers selfId={selfId} />);
  });
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

describe("AdminUsers", () => {
  it("lists users sorted by email and reflects status", async () => {
    vi.spyOn(platform.platformApi, "listAdminUsers").mockResolvedValue([
      user({ id: "b", email: "b@test.com" }),
      user({ id: "a", email: "a@test.com", disabled: true }),
    ]);
    mount();
    await flush();
    const rows = Array.from(
      container.querySelectorAll('tr[data-testid^="admin-user-"]'),
    );
    // sorted: a@ before b@.
    expect(rows).toHaveLength(2);
    expect(rows[0].getAttribute("data-testid")).toBe("admin-user-a");
    expect(
      container.querySelector('[data-testid="admin-user-status-a"]')
        ?.textContent,
    ).toBe("Banned");
  });

  it("bans a user after confirmation and reflects the disabled state", async () => {
    vi.spyOn(platform.platformApi, "listAdminUsers").mockResolvedValue([
      user({ id: "u1", disabled: false }),
    ]);
    const ban = vi
      .spyOn(platform.platformApi, "banUser")
      .mockResolvedValue(user({ id: "u1", disabled: true }));
    vi.spyOn(window, "confirm").mockReturnValue(true);
    mount();
    await flush();
    const btn = container.querySelector<HTMLButtonElement>(
      '[data-testid="admin-user-ban-u1"]',
    )!;
    act(() => btn.click());
    await flush();
    expect(ban).toHaveBeenCalledWith("u1");
    // Row now shows the unban action + Banned status.
    expect(
      container.querySelector('[data-testid="admin-user-unban-u1"]'),
    ).toBeTruthy();
    expect(
      container.querySelector('[data-testid="admin-user-status-u1"]')
        ?.textContent,
    ).toBe("Banned");
  });

  it("does not ban when the confirm is declined", async () => {
    vi.spyOn(platform.platformApi, "listAdminUsers").mockResolvedValue([
      user({ id: "u1" }),
    ]);
    const ban = vi.spyOn(platform.platformApi, "banUser");
    vi.spyOn(window, "confirm").mockReturnValue(false);
    mount();
    await flush();
    const btn = container.querySelector<HTMLButtonElement>(
      '[data-testid="admin-user-ban-u1"]',
    )!;
    act(() => btn.click());
    await flush();
    expect(ban).not.toHaveBeenCalled();
  });

  it("disables the ban button on the admin's own row", async () => {
    vi.spyOn(platform.platformApi, "listAdminUsers").mockResolvedValue([
      user({ id: "me", role: "admin" }),
    ]);
    mount("me");
    await flush();
    const btn = container.querySelector<HTMLButtonElement>(
      '[data-testid="admin-user-ban-me"]',
    )!;
    expect(btn.disabled).toBe(true);
  });

  it("unbans a banned user", async () => {
    vi.spyOn(platform.platformApi, "listAdminUsers").mockResolvedValue([
      user({ id: "u1", disabled: true }),
    ]);
    const unban = vi
      .spyOn(platform.platformApi, "unbanUser")
      .mockResolvedValue(user({ id: "u1", disabled: false }));
    mount();
    await flush();
    const btn = container.querySelector<HTMLButtonElement>(
      '[data-testid="admin-user-unban-u1"]',
    )!;
    act(() => btn.click());
    await flush();
    expect(unban).toHaveBeenCalledWith("u1");
    expect(
      container.querySelector('[data-testid="admin-user-status-u1"]')
        ?.textContent,
    ).toBe("Active");
  });

  it("surfaces an error when the list fails", async () => {
    vi.spyOn(platform.platformApi, "listAdminUsers").mockRejectedValue(
      new platform.ApiError(403, "admin only"),
    );
    mount();
    await flush();
    expect(
      container.querySelector('[data-testid="admin-users-error"]')?.textContent,
    ).toContain("admin only");
  });
});
