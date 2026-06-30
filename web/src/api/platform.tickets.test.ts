import { describe, expect, it, beforeEach, afterEach, vi } from "vitest";
import { ApiError, artifactUrl, platformApi } from "./platform";

/**
 * Ticket API client tests (WP4). Stub `globalThis.fetch` and assert the request
 * shape (method, path, query string, JSON body, cookie credentials) and the
 * response handling, mirroring platform.test.ts's approach.
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

describe("platformApi tickets — list", () => {
  it("lists with no filter and hits the bare endpoint with credentials", async () => {
    mockFetch(() => json([]));
    const list = await platformApi.listTickets();
    expect(list).toEqual([]);
    expect(calls[0].url).toBe("/api/tickets");
    expect(calls[0].init.method).toBe("GET");
    expect(calls[0].init.credentials).toBe("include");
  });

  it("encodes only the non-empty filters into the query string", async () => {
    mockFetch(() => json([]));
    await platformApi.listTickets({
      author: "me",
      state: "triage",
      category: "bug",
      q: "  crash  ",
    });
    const url = calls[0].url;
    expect(url.startsWith("/api/tickets?")).toBe(true);
    const params = new URLSearchParams(url.split("?")[1]);
    expect(params.get("author")).toBe("me");
    expect(params.get("state")).toBe("triage");
    expect(params.get("category")).toBe("bug");
    // free-text is trimmed.
    expect(params.get("q")).toBe("crash");
  });

  it("omits empty/whitespace filters", async () => {
    mockFetch(() => json([]));
    await platformApi.listTickets({ author: undefined, state: "", q: "   " });
    expect(calls[0].url).toBe("/api/tickets");
  });
});

describe("platformApi tickets — create / get / patch / delete", () => {
  it("createTicket POSTs title + body + category", async () => {
    mockFetch(() => json({ id: "t1", title: "X", body: "y" }, 201));
    const t = await platformApi.createTicket({
      title: "X",
      body: "y",
      category: "bug",
    });
    expect(t.id).toBe("t1");
    expect(calls[0].url).toBe("/api/tickets");
    expect(calls[0].init.method).toBe("POST");
    expect(JSON.parse(calls[0].init.body as string)).toEqual({
      title: "X",
      body: "y",
      category: "bug",
    });
  });

  it("createTicket defaults the category to empty", async () => {
    mockFetch(() => json({ id: "t1" }, 201));
    await platformApi.createTicket({ title: "X", body: "y" });
    expect(JSON.parse(calls[0].init.body as string).category).toBe("");
  });

  it("getTicket GETs the detail by id", async () => {
    mockFetch(() => json({ id: "t1", comments: [], artifacts: [] }));
    const t = await platformApi.getTicket("t1");
    expect(t.id).toBe("t1");
    expect(calls[0].url).toBe("/api/tickets/t1");
    expect(calls[0].init.method).toBe("GET");
  });

  it("updateTicket PATCHes the provided fields", async () => {
    mockFetch(() => json({ id: "t1", state: "closed" }));
    await platformApi.updateTicket("t1", { state: "closed" });
    expect(calls[0].url).toBe("/api/tickets/t1");
    expect(calls[0].init.method).toBe("PATCH");
    expect(JSON.parse(calls[0].init.body as string)).toEqual({
      state: "closed",
    });
  });

  it("deleteTicket DELETEs by id and tolerates an empty body", async () => {
    mockFetch(() => new Response(`{"ok":true}`, { status: 200 }));
    await expect(platformApi.deleteTicket("t1")).resolves.toBeDefined();
    expect(calls[0].init.method).toBe("DELETE");
    expect(calls[0].url).toBe("/api/tickets/t1");
  });

  it("surfaces a 401 as an ApiError (auth required)", async () => {
    mockFetch(() => json({ error: "not authenticated" }, 401));
    await expect(
      platformApi.createTicket({ title: "X", body: "y" }),
    ).rejects.toBeInstanceOf(ApiError);
  });
});

describe("platformApi tickets — comments + artifacts", () => {
  it("listComments GETs the comment thread", async () => {
    mockFetch(() => json([]));
    await platformApi.listComments("t1");
    expect(calls[0].url).toBe("/api/tickets/t1/comments");
    expect(calls[0].init.method).toBe("GET");
  });

  it("addComment POSTs the body", async () => {
    mockFetch(() => json({ id: "c1", body: "hi" }, 201));
    const c = await platformApi.addComment("t1", "hi");
    expect(c.id).toBe("c1");
    expect(calls[0].url).toBe("/api/tickets/t1/comments");
    expect(calls[0].init.method).toBe("POST");
    expect(JSON.parse(calls[0].init.body as string)).toEqual({ body: "hi" });
  });

  it("fetchArtifact returns the raw bytes with credentials", async () => {
    const bytes = new Uint8Array([1, 2, 3, 4]);
    mockFetch(() => new Response(bytes, { status: 200 }));
    const buf = await platformApi.fetchArtifact("t1", "a1");
    expect(new Uint8Array(buf)).toEqual(bytes);
    expect(calls[0].url).toBe("/api/tickets/t1/artifacts/a1");
    expect(calls[0].init.credentials).toBe("include");
  });

  it("fetchArtifact throws an ApiError on a non-2xx", async () => {
    mockFetch(() => json({ error: "not found" }, 404));
    await expect(
      platformApi.fetchArtifact("t1", "missing"),
    ).rejects.toMatchObject({
      status: 404,
    });
  });

  it("artifactUrl builds the download path", () => {
    expect(artifactUrl("t 1", "a/1")).toBe(
      "/api/tickets/t%201/artifacts/a%2F1",
    );
  });
});
