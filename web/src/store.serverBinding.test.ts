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

  it("loadServerProject persists the binding", () => {
    const store = makeStore();
    expect(store.loadServerProject("srv42", "{}")).toBe(true);
    expect(loadStoredServerBinding()).toBe("srv42");
    expect(store.getState().projectId).toBe("srv42");
    expect(store.getState().syncState).toBe("clean");
  });

  it("detachServerProject clears the binding (also when already offline)", () => {
    const store = makeStore();
    store.loadServerProject("srv42", "{}");
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
