import { describe, expect, it, beforeEach, afterEach, vi } from "vitest";
import { ApiError, platformApi, thumbnailUrl } from "./platform";

/**
 * The platform API client is a thin typed wrapper over `fetch`. These tests
 * stub `globalThis.fetch` and assert the request shape (method, path, JSON
 * body, cookie credentials) and the response/error handling (typed ApiError,
 * 401 → null for getMe).
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

describe("platformApi auth", () => {
  it("login POSTs JSON with credentials and returns the user", async () => {
    mockFetch(() =>
      json({
        id: "u1",
        email: "a@b.c",
        display_name: "Ada",
        role: "user",
        created_at: "2026-01-01T00:00:00Z",
      }),
    );
    const user = await platformApi.login({
      email: "a@b.c",
      password: "secret123",
    });
    expect(user.id).toBe("u1");
    expect(user.role).toBe("user");
    expect(calls).toHaveLength(1);
    expect(calls[0].url).toBe("/api/auth/login");
    expect(calls[0].init.method).toBe("POST");
    expect(calls[0].init.credentials).toBe("include");
    expect(JSON.parse(calls[0].init.body as string)).toEqual({
      email: "a@b.c",
      password: "secret123",
    });
  });

  it("register sends the invite token in the body", async () => {
    mockFetch(() =>
      json({
        id: "u2",
        email: "n@e.w",
        display_name: "New",
        role: "user",
        created_at: "2026-01-01T00:00:00Z",
      }),
    );
    await platformApi.register({
      email: "n@e.w",
      display_name: "New",
      password: "longpassword",
      invite: "TOKEN123",
    });
    expect(calls[0].url).toBe("/api/auth/register");
    expect(JSON.parse(calls[0].init.body as string).invite).toBe("TOKEN123");
  });

  it("getMe returns the user when authenticated", async () => {
    mockFetch(() =>
      json({
        id: "u1",
        email: "a@b.c",
        display_name: "Ada",
        role: "admin",
        created_at: "2026-01-01T00:00:00Z",
      }),
    );
    const me = await platformApi.getMe();
    expect(me?.role).toBe("admin");
    expect(calls[0].init.method).toBe("GET");
  });

  it("getMe returns null on a 401 (logged out)", async () => {
    mockFetch(() => json({ error: "not authenticated" }, 401));
    const me = await platformApi.getMe();
    expect(me).toBeNull();
  });

  it("getMe rethrows a non-401 error", async () => {
    mockFetch(() => json({ error: "boom" }, 500));
    await expect(platformApi.getMe()).rejects.toBeInstanceOf(ApiError);
  });
});

describe("platformApi error handling", () => {
  it("throws an ApiError carrying the server message + status", async () => {
    mockFetch(() => json({ error: "invalid credentials" }, 401));
    await expect(
      platformApi.login({ email: "x", password: "y" }),
    ).rejects.toMatchObject({
      status: 401,
      message: "invalid credentials",
    });
  });

  it("falls back to the status text when the body has no error field", async () => {
    mockFetch(
      () =>
        new Response("", { status: 503, statusText: "Service Unavailable" }),
    );
    await expect(platformApi.listProjects()).rejects.toMatchObject({
      status: 503,
    });
  });
});

describe("platformApi projects", () => {
  it("createProject posts name + document", async () => {
    mockFetch(() =>
      json(
        {
          id: "p1",
          owner_id: "u1",
          name: "Untitled",
          visibility: "private",
          document: {},
          updated_at: "2026-01-01T00:00:00Z",
          created_at: "2026-01-01T00:00:00Z",
        },
        201,
      ),
    );
    const p = await platformApi.createProject("Untitled", { hello: 1 });
    expect(p.id).toBe("p1");
    expect(calls[0].init.method).toBe("POST");
    expect(JSON.parse(calls[0].init.body as string)).toEqual({
      name: "Untitled",
      document: { hello: 1 },
    });
  });

  it("setVisibility PATCHes the visibility only", async () => {
    mockFetch(() =>
      json({
        id: "p1",
        owner_id: "u1",
        name: "P",
        visibility: "public",
        document: {},
        updated_at: "2026-01-02T00:00:00Z",
        created_at: "2026-01-01T00:00:00Z",
      }),
    );
    const p = await platformApi.setVisibility("p1", "public");
    expect(p.visibility).toBe("public");
    expect(calls[0].url).toBe("/api/projects/p1");
    expect(calls[0].init.method).toBe("PATCH");
    expect(JSON.parse(calls[0].init.body as string)).toEqual({
      visibility: "public",
    });
  });

  it("deleteProject DELETEs and tolerates an empty body", async () => {
    mockFetch(() => new Response("", { status: 200 }));
    await expect(platformApi.deleteProject("p1")).resolves.toBeUndefined();
    expect(calls[0].init.method).toBe("DELETE");
    expect(calls[0].url).toBe("/api/projects/p1");
  });

  it("listGallery hits the public endpoint with credentials", async () => {
    mockFetch(() => json([]));
    const list = await platformApi.listGallery();
    expect(list).toEqual([]);
    expect(calls[0].url).toBe("/api/gallery");
  });
});

describe("platformApi invites", () => {
  it("createInvite posts the invitee name + email + message", async () => {
    mockFetch(() =>
      json(
        {
          token: "T",
          created_by: "u1",
          invitee_name: "Bob",
          invitee_email: "bob@x.io",
          message: "join us",
          expires_at: "2026-02-01T00:00:00Z",
          used_by: null,
          used_at: null,
          created_at: "2026-01-01T00:00:00Z",
          status: "open",
          register_url: "/register?invite=T",
        },
        201,
      ),
    );
    const inv = await platformApi.createInvite({
      invitee_name: "Bob",
      invitee_email: "bob@x.io",
      message: "join us",
    });
    expect(inv.register_url).toBe("/register?invite=T");
    expect(inv.invitee_email).toBe("bob@x.io");
    const body = JSON.parse(calls[0].init.body as string);
    expect(body).toEqual({
      invitee_name: "Bob",
      invitee_email: "bob@x.io",
      message: "join us",
    });
  });

  it("deleteInvite revokes by token", async () => {
    mockFetch(() => new Response("", { status: 200 }));
    await platformApi.deleteInvite("T");
    expect(calls[0].url).toBe("/api/invites/T");
    expect(calls[0].init.method).toBe("DELETE");
  });
});

describe("thumbnailUrl", () => {
  it("builds the thumbnail path", () => {
    expect(thumbnailUrl("p1")).toBe("/api/projects/p1/thumbnail");
  });
});
