/**
 * Ticket 6398… — the server-project binding survives a page reload.
 *
 * The localStorage autosave restores the document; `fielddraft.serverProjectId`
 * restores the sync binding. Without it, a reload silently degrades a
 * server-saved project to guest mode and the CGAL server export claims
 * "save to server first". Reuses the FakeKernel seam from
 * store.renameProject.test.ts.
 */

import { describe, expect, it, beforeEach, afterEach, vi } from "vitest";
import { KernelStore } from "./store";
import type { Kernel } from "./kernel";
import type { AutoSaveHandle } from "./persistence";
import {
  loadStoredServerBinding,
  clearStoredServerBinding,
} from "./projectStore";

class FakeKernel {
  loadedJson =
    '{"schema_version":1,"name":"Part","feature_calls":[],"next_feature_id":1}';
  listScripts() {
    return [];
  }
  upsertScript() {
    return false;
  }
  removeScript() {
    return false;
  }
  addFeatureCall() {
    return 42;
  }
  loadJson(json: string) {
    this.loadedJson = json;
  }
  listFeatures() {
    return [];
  }
  listVariables() {
    return [];
  }
  canUndo() {
    return false;
  }
  undoDepth() {
    return 0;
  }
  canRedo() {
    return false;
  }
  redoDepth() {
    return 0;
  }
  getRollbackIndex() {
    return -1;
  }
  toJson() {
    return this.loadedJson;
  }
  sceneAabb() {
    return { size: [0, 0, 0] };
  }
  setSyncMeshing() {
    return false;
  }
  activeBodyId() {
    return 0;
  }
  clearActiveBodyId() {}
  setActiveBodyByFeature() {}
  bodyCount() {
    return 0;
  }
}

const autosave: AutoSaveHandle = {
  schedule: () => {},
  flush: () => {},
  cancel: () => {},
};

function makeStore(): KernelStore {
  return new KernelStore(new FakeKernel() as unknown as Kernel, autosave);
}

beforeEach(() => {
  clearStoredServerBinding();
  globalThis.fetch = vi.fn(
    async () => new Response("{}", { status: 200 }),
  ) as unknown as typeof fetch;
});

afterEach(() => {
  clearStoredServerBinding();
  vi.restoreAllMocks();
});

describe("server-project binding persistence", () => {
  it("setProjectId persists the binding; null clears it", () => {
    const store = makeStore();
    store.setProjectId("abc123");
    expect(loadStoredServerBinding()).toBe("abc123");
    expect(store.getState().projectId).toBe("abc123");
    expect(store.getState().syncState).toBe("dirty");

    store.setProjectId(null);
    expect(loadStoredServerBinding()).toBeNull();
  });

  it("loadServerProject persists the binding", async () => {
    const store = makeStore();
    expect(await store.loadServerProject("srv42", "{}")).toBe(true);
    expect(loadStoredServerBinding()).toBe("srv42");
    expect(store.getState().projectId).toBe("srv42");
    expect(store.getState().syncState).toBe("clean");
  });

  it("detachServerProject clears the binding (also when already offline)", async () => {
    const store = makeStore();
    await store.loadServerProject("srv42", "{}");
    store.detachServerProject();
    expect(loadStoredServerBinding()).toBeNull();
    expect(store.getState().projectId).toBeNull();
    expect(store.getState().syncState).toBe("offline");

    // Early-return path (already detached) must still clear a stray key —
    // the App-level rebind failure calls clearStoredServerBinding, but a
    // direct detach on a fresh store must not leave stale storage behind.
    store.setProjectId("stray");
    store.setProjectId(null); // state now offline/null, storage cleared
    localStorage.setItem("fielddraft.serverProjectId", "stale");
    store.detachServerProject();
    expect(loadStoredServerBinding()).toBeNull();
  });

  it("a fresh store starts unbound regardless of stored binding (rebind is App-level)", () => {
    localStorage.setItem("fielddraft.serverProjectId", "srv42");
    const store = makeStore();
    // The store itself never auto-adopts the stored id — App.tsx verifies it
    // against the server first (deleted/foreign projects must not bind).
    expect(store.getState().projectId).toBeNull();
    expect(store.getState().syncState).toBe("offline");
    expect(loadStoredServerBinding()).toBe("srv42");
  });
});

/** A Project blob whose single part tab embeds an `imported_mesh` — drives the
 *  asset-bearing sync path (externalize → blob upload → envelope). */
function meshProjectBlob(): string {
  const documentJson = JSON.stringify({
    schema_version: 1,
    name: "Part",
    feature_calls: [
      {
        type: "imported_mesh",
        id: "1",
        params: {
          op: "New",
          __positions: [0, 0, 0, 1, 0, 0, 0, 1, 0],
          __indices: [0, 1, 2],
          __watertight: true,
        },
      },
    ],
    next_feature_id: 2,
  });
  return JSON.stringify({
    schemaVersion: 1,
    name: "Untitled",
    tabs: [{ id: "tab_1", kind: "part", name: "Part 1", documentJson }],
    activeTabId: "tab_1",
  });
}

describe("syncToServer — lean cloud save (P2)", () => {
  it("sends the plain Project blob for an asset-free workspace", async () => {
    const store = makeStore();
    store.setProjectId("srv1");
    const fetchMock = vi.fn(
      async (_url: string | URL | Request, _init?: RequestInit) =>
        new Response("{}", { status: 200 }),
    );
    globalThis.fetch = fetchMock as unknown as typeof fetch;

    expect(await store.syncToServer()).toBe(true);
    // The only request is the project PATCH (no blobs to probe/upload).
    const patch = fetchMock.mock.calls.find(
      (c) => (c[1] as RequestInit).method === "PATCH",
    );
    expect(patch).toBeDefined();
    expect(String(patch![0])).toBe("/api/projects/srv1");
    const body = JSON.parse((patch![1] as RequestInit).body as string) as {
      document: { tabs?: unknown; v?: unknown };
    };
    // Asset-free ⇒ document is the full Project blob (has tabs), NOT an envelope.
    expect(body.document.tabs).toBeDefined();
    expect(body.document.v).toBeUndefined();
  });

  it("externalizes the mesh to a blob and sends the envelope for an asset workspace", async () => {
    const store = makeStore();
    expect(store.loadProjectFromJson(meshProjectBlob())).toBe(true);
    store.setProjectId("srv2");

    const fetchMock = vi.fn(async (url: string | URL | Request, init?: RequestInit) => {
      const u = String(url);
      if (u === "/api/blobs/missing") {
        // Echo the requested hashes back as missing ⇒ the client uploads them.
        const reqBody = JSON.parse((init?.body as string) ?? "{}") as { hashes: string[] };
        return new Response(JSON.stringify({ missing: reqBody.hashes }), { status: 200 });
      }
      if (u.startsWith("/api/blobs/")) {
        return new Response(JSON.stringify({ stored: true }), { status: 200 });
      }
      return new Response("{}", { status: 200 }); // project PATCH
    });
    globalThis.fetch = fetchMock as unknown as typeof fetch;

    expect(await store.syncToServer()).toBe(true);

    // A blob was uploaded via PUT /api/blobs/{hash}.
    const put = fetchMock.mock.calls.find(
      (c) =>
        (c[1] as RequestInit)?.method === "PUT" &&
        String(c[0]).startsWith("/api/blobs/"),
    );
    expect(put).toBeDefined();

    // The PATCH document is the lean envelope (v:2) — no inline mesh payload.
    const patch = fetchMock.mock.calls.find(
      (c) => (c[1] as RequestInit)?.method === "PATCH",
    );
    expect(patch).toBeDefined();
    const body = JSON.parse((patch![1] as RequestInit).body as string) as {
      document: { v?: number; doc?: string };
    };
    expect(body.document.v).toBe(2);
    expect(body.document.doc).not.toContain("__positions");
    expect(body.document.doc).toContain("__asset");
  });
});
