import { describe, expect, it, beforeEach, afterEach, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { UserMenu } from "./UserMenu";
import { initialAccountState, type AccountState } from "./accountState";
import type { UseAccount } from "./useAccount";
import type { User } from "../api/platform";

(
  globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

const USER: User = {
  id: "u1",
  email: "ada@b.c",
  display_name: "Ada",
  role: "user",
  created_at: "2026-01-01T00:00:00Z",
};

function fakeAccount(over: Partial<AccountState> = {}): {
  account: UseAccount;
  openAuth: ReturnType<typeof vi.fn>;
  logout: ReturnType<typeof vi.fn>;
} {
  const state: AccountState = {
    ...initialAccountState(""),
    status: "authed",
    user: USER,
    ...over,
  };
  const openAuth = vi.fn();
  const logout = vi.fn(async () => {});
  const account = {
    state,
    isAdmin: state.user?.role === "admin",
    showOverview: vi.fn(),
    selectTab: vi.fn(),
    enterProject: vi.fn(),
    openTicket: vi.fn(),
    openAuth,
    closeAuth: vi.fn(),
    setBell: vi.fn(),
    login: vi.fn(),
    register: vi.fn(),
    logout,
    refresh: vi.fn(),
    setUnseenAlerts: vi.fn(),
    refreshAlertCount: vi.fn(),
  } as unknown as UseAccount;
  return { account, openAuth, logout };
}

let container: HTMLDivElement;
let root: Root;

function mount(account: UseAccount, onLoggedOut = vi.fn()) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => {
    root.render(<UserMenu account={account} onLoggedOut={onLoggedOut} />);
  });
  return onLoggedOut;
}

function byTestId<T extends HTMLElement = HTMLElement>(id: string): T | null {
  return container.querySelector<T>(`[data-testid="${id}"]`);
}

function click(el: Element) {
  act(() => el.dispatchEvent(new MouseEvent("click", { bubbles: true })));
}

beforeEach(() => {
  vi.restoreAllMocks();
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

describe("UserMenu", () => {
  it("guest shows a Log in button that opens the login auth screen", () => {
    const { account, openAuth } = fakeAccount({ status: "guest", user: null });
    mount(account);
    expect(byTestId("user-login")).toBeTruthy();
    click(byTestId("user-login")!);
    expect(openAuth).toHaveBeenCalledWith("login");
  });

  it("authed shows a person glyph (no name) with the identity line in the menu", () => {
    const { account } = fakeAccount();
    mount(account);
    const trigger = byTestId("user-menu-trigger")!;
    expect(trigger.textContent).toContain("👤");
    // No display name / email leaks onto the trigger itself.
    expect(trigger.textContent).not.toContain("Ada");
    click(trigger);
    expect(byTestId("user-identity")?.textContent).toContain("ada@b.c");
    expect(byTestId("user-identity")?.textContent).toContain("user");
  });

  it("has no Report Issue entry (lives in the settings burger, b5a60122)", () => {
    const { account } = fakeAccount();
    mount(account);
    click(byTestId("user-menu-trigger")!);
    expect(byTestId("user-report")).toBeNull();
    expect(byTestId("user-logout")).toBeTruthy();
  });

  it("Log out calls logout then onLoggedOut", async () => {
    const { account, logout } = fakeAccount();
    const onLoggedOut = mount(account);
    click(byTestId("user-menu-trigger")!);
    click(byTestId("user-logout")!);
    await act(async () => {
      await Promise.resolve();
    });
    expect(logout).toHaveBeenCalled();
    expect(onLoggedOut).toHaveBeenCalled();
  });

  it("no longer has Projects / Gallery / Issues / Invites / Users entries", () => {
    const { account } = fakeAccount({ user: { ...USER, role: "admin" } });
    mount(account);
    click(byTestId("user-menu-trigger")!);
    expect(byTestId("account-dashboard")).toBeNull();
    expect(byTestId("account-gallery")).toBeNull();
    expect(byTestId("account-tickets")).toBeNull();
    expect(byTestId("account-invites")).toBeNull();
    expect(byTestId("account-users")).toBeNull();
  });
});
