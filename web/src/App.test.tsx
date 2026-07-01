/**
 * App login-gate tests (WP11).
 *
 * The gate is a build flag: `import.meta.env.VITE_REQUIRE_LOGIN === "1"` turns
 * it on. The base/dev/e2e build leaves it unset, so the default (flag OFF) path
 * is what every other test and the local dev server exercise — these tests stub
 * the env per-case with `vi.stubEnv` and restore it after, so turning the gate
 * on here never leaks into the rest of the suite.
 *
 * The heavy modeling subtree is not under test here, only the gate boundary, so
 * `Viewport` (an R3F Canvas that jsdom can't host) is mocked to an inert node.
 * The kernel store loads asynchronously and stays null in jsdom — AppShell
 * renders its topbar regardless, which is the signal we assert "the app
 * mounted". The AuthScreen carries `data-testid="auth-screen"`.
 */

import { describe, expect, it, beforeEach, afterEach, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { App } from "./App";
import type { PlatformApi, User } from "./api/platform";

// The viewport is an R3F <Canvas> needing WebGL — unavailable under jsdom.
// Replace it with an inert stub so AppShell can render in the test.
vi.mock("./Viewport", () => ({
  Viewport: () => null,
}));

(
  globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

const user: User = {
  id: "u1",
  email: "a@b.c",
  display_name: "Ada",
  role: "user",
  created_at: "2026-01-01T00:00:00Z",
};

/** A fake PlatformApi. The boot + gate + Overview data fetches (projects /
 *  gallery / alerts) resolve to empty; the rest throw so an accidental call is
 *  caught. */
function fakeApi(overrides: Partial<PlatformApi> = {}): PlatformApi {
  const notImpl = () => {
    throw new Error("not implemented in fake");
  };
  return {
    getMe: async () => null,
    login: async () => user,
    register: async () => user,
    logout: async () => {},
    requestPasswordReset: notImpl,
    resetPassword: notImpl,
    listProjects: async () => [],
    createProject: notImpl,
    getProject: notImpl,
    updateProject: notImpl,
    deleteProject: notImpl,
    setVisibility: notImpl,
    publishProject: notImpl,
    listGallery: async () => [],
    listInvites: async () => [],
    createInvite: notImpl,
    deleteInvite: notImpl,
    listAdminAlerts: async () => [],
    ...overrides,
  } as PlatformApi;
}

let container: HTMLDivElement;
let root: Root;

async function mount(api: PlatformApi) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  await act(async () => {
    root.render(<App accountOptions={{ api, search: "" }} />);
  });
  // Let the boot getMe() effect resolve (two microtask turns, as in
  // useAccount.test.tsx).
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
  });
}

function has(testid: string): boolean {
  return container.querySelector(`[data-testid="${testid}"]`) !== null;
}

/** AppShell renders the modeling topbar even before the kernel store loads. */
function appShellMounted(): boolean {
  return container.querySelector(".fd-shell") !== null;
}

beforeEach(() => {
  vi.restoreAllMocks();
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
  vi.unstubAllEnvs();
});

describe("App login gate", () => {
  it("flag ON + no session: renders the AuthScreen, NOT the app", async () => {
    vi.stubEnv("VITE_REQUIRE_LOGIN", "1");
    await mount(fakeApi({ getMe: async () => null }));
    expect(has("auth-screen")).toBe(true);
    expect(appShellMounted()).toBe(false);
    // The full-page variant suppresses the dismiss control.
    expect(has("auth-close")).toBe(false);
  });

  it("flag ON + network error is treated as not authed (gate stays up)", async () => {
    vi.stubEnv("VITE_REQUIRE_LOGIN", "1");
    await mount(
      fakeApi({
        getMe: async () => {
          throw new Error("network down");
        },
      }),
    );
    expect(has("auth-screen")).toBe(true);
    expect(appShellMounted()).toBe(false);
  });

  it("flag ON + a session: renders the app, NOT the AuthScreen", async () => {
    vi.stubEnv("VITE_REQUIRE_LOGIN", "1");
    await mount(fakeApi({ getMe: async () => user }));
    expect(appShellMounted()).toBe(true);
    expect(has("auth-screen")).toBe(false);
  });

  it("flag OFF (default): renders the app without any auth, even with no session", async () => {
    vi.stubEnv("VITE_REQUIRE_LOGIN", "0");
    await mount(fakeApi({ getMe: async () => null }));
    expect(appShellMounted()).toBe(true);
    expect(has("auth-screen")).toBe(false);
  });

  it("flag UNSET (default): renders the app without any auth", async () => {
    // No stubEnv — VITE_REQUIRE_LOGIN is undefined, the dev/e2e default.
    await mount(fakeApi({ getMe: async () => null }));
    expect(appShellMounted()).toBe(true);
    expect(has("auth-screen")).toBe(false);
  });
});
