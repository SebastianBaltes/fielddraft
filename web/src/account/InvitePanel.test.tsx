import { describe, expect, it, beforeEach, afterEach, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { InvitePanel } from "./InvitePanel";
import * as platform from "../api/platform";
import type { Invite } from "../api/platform";

(
  globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

function invite(over: Partial<Invite> = {}): Invite {
  return {
    token: "T1",
    created_by: "admin",
    invitee_name: "Ada Lovelace",
    invitee_email: "ada@x.io",
    message: "join us",
    expires_at: "2026-07-01T00:00:00Z",
    used_by: null,
    used_at: null,
    created_at: "2026-06-01T00:00:00Z",
    status: "open",
    register_url: "/register?invite=T1",
    ...over,
  };
}

let container: HTMLDivElement;
let root: Root;

function mount() {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => {
    root.render(<InvitePanel />);
  });
}

async function flush() {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
  });
}

function byTestId<T extends HTMLElement = HTMLElement>(id: string): T | null {
  return container.querySelector<T>(`[data-testid="${id}"]`);
}

function setInput(id: string, value: string) {
  const el = byTestId<HTMLInputElement | HTMLTextAreaElement>(id)!;
  const proto =
    el instanceof HTMLTextAreaElement
      ? HTMLTextAreaElement.prototype
      : HTMLInputElement.prototype;
  const setter = Object.getOwnPropertyDescriptor(proto, "value")!.set!;
  act(() => {
    setter.call(el, value);
    el.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

beforeEach(() => {
  vi.restoreAllMocks();
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

describe("InvitePanel", () => {
  it("has invitee name + email + invitation text fields (not a Note field)", async () => {
    vi.spyOn(platform.platformApi, "listInvites").mockResolvedValue([]);
    mount();
    await flush();
    expect(byTestId("invite-name")).toBeTruthy();
    expect(byTestId("invite-email")).toBeTruthy();
    expect(byTestId("invite-message")).toBeTruthy();
    expect(byTestId("invite-note")).toBeNull();
  });

  it("creates an invite from the addressee fields and shows the register link", async () => {
    vi.spyOn(platform.platformApi, "listInvites").mockResolvedValue([]);
    const created = invite({
      token: "NEW",
      register_url: "/register?invite=NEW",
    });
    const create = vi
      .spyOn(platform.platformApi, "createInvite")
      .mockResolvedValue(created);
    mount();
    await flush();
    setInput("invite-name", "Ada Lovelace");
    setInput("invite-email", "ada@x.io");
    setInput("invite-message", "please join");
    act(() => byTestId<HTMLButtonElement>("invite-create")!.click());
    await flush();
    expect(create).toHaveBeenCalledWith({
      invitee_name: "Ada Lovelace",
      invitee_email: "ada@x.io",
      message: "please join",
    });
    // The shareable register_url is surfaced after creation.
    expect(byTestId("invite-created-link")?.textContent).toContain(
      "/register?invite=NEW",
    );
  });

  it("create is disabled until name + email are filled", async () => {
    vi.spyOn(platform.platformApi, "listInvites").mockResolvedValue([]);
    mount();
    await flush();
    const btn = byTestId<HTMLButtonElement>("invite-create")!;
    expect(btn.disabled).toBe(true);
    setInput("invite-name", "Ada");
    expect(byTestId<HTMLButtonElement>("invite-create")!.disabled).toBe(true);
    setInput("invite-email", "ada@x.io");
    expect(byTestId<HTMLButtonElement>("invite-create")!.disabled).toBe(false);
  });

  it("lists invites with their addressee", async () => {
    vi.spyOn(platform.platformApi, "listInvites").mockResolvedValue([
      invite({ token: "T1", invitee_name: "Ada", invitee_email: "ada@x.io" }),
    ]);
    mount();
    await flush();
    const row = byTestId("invite-row-T1")!;
    expect(row.textContent).toContain("Ada");
    expect(row.textContent).toContain("ada@x.io");
  });

  it("has no × close button (it is a tab)", async () => {
    vi.spyOn(platform.platformApi, "listInvites").mockResolvedValue([]);
    mount();
    await flush();
    expect(byTestId("invite-close")).toBeNull();
  });
});
