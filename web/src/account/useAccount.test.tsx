import { describe, expect, it, beforeEach, afterEach, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { useAccount, type UseAccount } from "./useAccount";
import type { PlatformApi, User } from "../api/platform";

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

/** A fake PlatformApi that records calls; only the methods useAccount touches
 *  are meaningful, the rest throw so an accidental call is caught. */
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
    listProjects: notImpl,
    createProject: notImpl,
    getProject: notImpl,
    updateProject: notImpl,
    deleteProject: notImpl,
    setVisibility: notImpl,
    publishProject: notImpl,
    listGallery: notImpl,
    listInvites: notImpl,
    createInvite: notImpl,
    deleteInvite: notImpl,
    ...overrides,
  } as PlatformApi;
}

let container: HTMLDivElement;
let root: Root;
let latest: UseAccount;

function Probe({ api, search }: { api: PlatformApi; search: string }) {
  latest = useAccount({ api, search });
  return null;
}

async function mount(api: PlatformApi, search = "") {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  await act(async () => {
    root.render(<Probe api={api} search={search} />);
  });
  // Let the boot getMe() effect resolve.
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

describe("useAccount", () => {
  it("resolves to guest when getMe returns null", async () => {
    await mount(fakeApi({ getMe: async () => null }));
    expect(latest.state.status).toBe("guest");
    expect(latest.state.user).toBeNull();
  });

  it("resolves to authed when getMe returns a user", async () => {
    await mount(fakeApi({ getMe: async () => user }));
    expect(latest.state.status).toBe("authed");
    expect(latest.state.user?.id).toBe("u1");
  });

  it("treats a getMe failure as guest (offline-safe)", async () => {
    await mount(
      fakeApi({
        getMe: async () => {
          throw new Error("network down");
        },
      }),
    );
    expect(latest.state.status).toBe("guest");
  });

  it("login transitions to authed and opens the dashboard", async () => {
    const login = vi.fn(async () => user);
    await mount(fakeApi({ getMe: async () => null, login }));
    await act(async () => {
      await latest.login("a@b.c", "pw");
    });
    expect(login).toHaveBeenCalledWith({ email: "a@b.c", password: "pw" });
    expect(latest.state.status).toBe("authed");
    expect(latest.state.overlay).toBe("dashboard");
  });

  it("logout returns to guest even if the server call fails", async () => {
    await mount(
      fakeApi({
        getMe: async () => user,
        logout: async () => {
          throw new Error("already gone");
        },
      }),
    );
    expect(latest.state.status).toBe("authed");
    await act(async () => {
      // The server logout rejects, but the hook still drops the local session
      // (and swallows the server error). Assert the resulting guest state.
      await latest.logout();
    });
    expect(latest.state.status).toBe("guest");
    expect(latest.state.user).toBeNull();
  });

  it("exposes isAdmin true for an admin user", async () => {
    await mount(fakeApi({ getMe: async () => ({ ...user, role: "admin" }) }));
    expect(latest.isAdmin).toBe(true);
  });

  it("auto-opens register from a ?invite= deep-link", async () => {
    await mount(fakeApi({ getMe: async () => null }), "?invite=ABC");
    expect(latest.state.overlay).toBe("register");
    expect(latest.state.invitePrefill).toBe("ABC");
  });
});
