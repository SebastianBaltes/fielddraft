import { describe, expect, it, beforeEach, afterEach, vi } from "vitest";
import { ApiError, platformApi } from "./platform";

/**
 * Admin moderation API client tests (WP9). Stub `globalThis.fetch` and assert
 * the request shape (method, path, query string, cookie credentials) and the
 * response handling, mirroring platform.tickets.test.ts.
 */

interface CapturedCall {
  url: string;
  init: RequestInit;
}

let calls: CapturedCall[] = [];

function mockFetch(
  responder: (url: string, init: RequestInit) => Response | Promise<Response>,
) {
  globalThis.fetch = vi.fn(
    async (input: RequestInfo | URL, init: RequestInit = {}) => {
      const url = typeof input === "string" ? input : String(input);
      calls.push({ url, init });
      return responder(url, init);
    },
  ) as unknown as typeof fetch;
}

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

beforeEach(() => {
  calls = [];
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("platformApi admin alerts", () => {
  it("listAdminAlerts GETs the bare endpoint with credentials", async () => {
    mockFetch(() => json([]));
    const list = await platformApi.listAdminAlerts();
    expect(list).toEqual([]);
    expect(calls[0].url).toBe("/api/admin/alerts");
    expect(calls[0].init.method).toBe("GET");
    expect(calls[0].init.credentials).toBe("include");
  });

  it("listAdminAlerts adds ?unseen=1 when filtering unseen", async () => {
    mockFetch(() =>
      json([
        {
          id: "a1",
          kind: "malicious_ticket",
          ticket_id: "t1",
          detail: "spam",
          created_at: "2026-06-01T00:00:00Z",
          seen: false,
        },
      ]),
    );
    const list = await platformApi.listAdminAlerts(true);
    expect(list).toHaveLength(1);
    expect(list[0].kind).toBe("malicious_ticket");
    expect(calls[0].url).toBe("/api/admin/alerts?unseen=1");
    expect(calls[0].init.method).toBe("GET");
  });

  it("markAdminAlertSeen POSTs to the seen endpoint, encoding the id", async () => {
    mockFetch(() => new Response(`{"ok":true}`, { status: 200 }));
    await platformApi.markAdminAlertSeen("a/1");
    expect(calls[0].url).toBe("/api/admin/alerts/a%2F1/seen");
    expect(calls[0].init.method).toBe("POST");
    expect(calls[0].init.credentials).toBe("include");
  });

  it("surfaces a 403 (non-admin) as an ApiError", async () => {
    mockFetch(() => json({ error: "admin only" }, 403));
    await expect(platformApi.listAdminAlerts()).rejects.toBeInstanceOf(
      ApiError,
    );
  });
});

describe("platformApi admin users", () => {
  it("listAdminUsers GETs the users endpoint", async () => {
    mockFetch(() =>
      json([
        {
          id: "u1",
          email: "a@test.com",
          display_name: "A",
          role: "user",
          disabled: false,
          created_at: "2026-06-01T00:00:00Z",
        },
      ]),
    );
    const list = await platformApi.listAdminUsers();
    expect(list).toHaveLength(1);
    expect(list[0].disabled).toBe(false);
    expect(calls[0].url).toBe("/api/admin/users");
    expect(calls[0].init.method).toBe("GET");
  });

  it("banUser POSTs to the ban endpoint and returns the updated user", async () => {
    mockFetch(() =>
      json({
        id: "u1",
        email: "a@test.com",
        display_name: "A",
        role: "user",
        disabled: true,
        created_at: "2026-06-01T00:00:00Z",
      }),
    );
    const u = await platformApi.banUser("u1");
    expect(u.disabled).toBe(true);
    expect(calls[0].url).toBe("/api/admin/users/u1/ban");
    expect(calls[0].init.method).toBe("POST");
  });

  it("unbanUser POSTs to the unban endpoint and returns the updated user", async () => {
    mockFetch(() =>
      json({
        id: "u1",
        email: "a@test.com",
        display_name: "A",
        role: "user",
        disabled: false,
        created_at: "2026-06-01T00:00:00Z",
      }),
    );
    const u = await platformApi.unbanUser("u1");
    expect(u.disabled).toBe(false);
    expect(calls[0].url).toBe("/api/admin/users/u1/unban");
    expect(calls[0].init.method).toBe("POST");
  });

  it("encodes the user id in ban/unban paths", async () => {
    mockFetch(() => json({ id: "u 1", disabled: true }));
    await platformApi.banUser("u 1");
    expect(calls[0].url).toBe("/api/admin/users/u%201/ban");
  });

  it("surfaces a 403 (non-admin) as an ApiError", async () => {
    mockFetch(() => json({ error: "admin only" }, 403));
    await expect(platformApi.listAdminUsers()).rejects.toMatchObject({
      status: 403,
    });
  });
});
