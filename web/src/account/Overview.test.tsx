import { describe, expect, it, beforeEach, afterEach, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { Overview } from "./Overview";
import { initialAccountState, type AccountState } from "./accountState";
import type { UseAccount } from "./useAccount";
import * as platform from "../api/platform";
import type { User } from "../api/platform";

(
  globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

const USER: User = {
  id: "u1",
  email: "a@b.c",
  display_name: "Ada",
  role: "user",
  created_at: "2026-01-01T00:00:00Z",
};

/** A fake UseAccount driven by a fixed state + a spy selectTab. */
function fakeAccount(over: Partial<AccountState> = {}): {
  account: UseAccount;
  selectTab: ReturnType<typeof vi.fn>;
} {
  const state: AccountState = {
    ...initialAccountState(""),
    status: "authed",
    user: USER,
    ...over,
  };
  const selectTab = vi.fn();
  const account = {
    state,
    isAdmin: state.user?.role === "admin",
    selectTab,
    showOverview: vi.fn(),
    enterProject: vi.fn(),
    openTicket: vi.fn(),
    openAuth: vi.fn(),
    closeAuth: vi.fn(),
    setBell: vi.fn(),
    login: vi.fn(),
    register: vi.fn(),
    logout: vi.fn(),
    refresh: vi.fn(),
    setUnseenAlerts: vi.fn(),
    refreshAlertCount: vi.fn(),
  } as unknown as UseAccount;
  return { account, selectTab };
}

let container: HTMLDivElement;
let root: Root;

function mount(account: UseAccount) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => {
    root.render(
      <Overview
        account={account}
        onOpenProject={() => {}}
        onOpenCopy={() => {}}
        onOpenLocalProject={() => {}}
        onLoadFdraft={() => {}}
        boundProjectId={null}
      />,
    );
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

beforeEach(() => {
  vi.restoreAllMocks();
  vi.spyOn(platform.platformApi, "listProjects").mockResolvedValue([]);
  vi.spyOn(platform.platformApi, "listGallery").mockResolvedValue([]);
  vi.spyOn(platform.platformApi, "listTickets").mockResolvedValue([]);
  vi.spyOn(platform.platformApi, "listInvites").mockResolvedValue([]);
  vi.spyOn(platform.platformApi, "listAdminUsers").mockResolvedValue([]);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

describe("Overview", () => {
  it("shows only Projects + Issues tabs for a non-admin", async () => {
    const { account } = fakeAccount();
    mount(account);
    await flush();
    expect(byTestId("overview-tab-projects")).toBeTruthy();
    expect(byTestId("overview-tab-issues")).toBeTruthy();
    expect(byTestId("overview-tab-invites")).toBeNull();
    expect(byTestId("overview-tab-users")).toBeNull();
  });

  it("shows all four tabs for an admin", async () => {
    const { account } = fakeAccount({ user: { ...USER, role: "admin" } });
    mount(account);
    await flush();
    expect(byTestId("overview-tab-invites")).toBeTruthy();
    expect(byTestId("overview-tab-users")).toBeTruthy();
  });

  it("renders the projects browser on the Projects tab", async () => {
    const { account } = fakeAccount({ overviewTab: "projects" });
    mount(account);
    await flush();
    expect(byTestId("projects-browser")).toBeTruthy();
  });

  it("renders the issues list on the Issues tab", async () => {
    const { account } = fakeAccount({ overviewTab: "issues" });
    mount(account);
    await flush();
    expect(byTestId("tickets")).toBeTruthy();
  });

  it("renders the invite form on the Invites tab (admin)", async () => {
    const { account } = fakeAccount({
      user: { ...USER, role: "admin" },
      overviewTab: "invites",
    });
    mount(account);
    await flush();
    expect(byTestId("invite-panel")).toBeTruthy();
  });

  it("renders the users table on the Users tab (admin)", async () => {
    const { account } = fakeAccount({
      user: { ...USER, role: "admin" },
      overviewTab: "users",
    });
    mount(account);
    await flush();
    expect(byTestId("admin-users")).toBeTruthy();
  });

  it("clicking a tab dispatches selectTab", async () => {
    const { account, selectTab } = fakeAccount();
    mount(account);
    await flush();
    act(() => byTestId<HTMLButtonElement>("overview-tab-issues")!.click());
    expect(selectTab).toHaveBeenCalledWith("issues");
  });
});
