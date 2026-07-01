/**
 * Ticket C — store-level project rename.
 *
 * `renameProject` always updates local `project.name` (trim; ignore empty /
 * unchanged) and, when the project is server-bound (`projectId` set), persists
 * via `PATCH /api/projects/{id}` with `{name}`. An unbound guest project renames
 * locally only. Reuses the same FakeKernel seam as store.scripts.test.ts so a
 * real KernelStore can be driven under jsdom (sync-meshing fallback).
 */

import { describe, expect, it, beforeEach, afterEach, vi } from "vitest";
import { KernelStore } from "./store";
import type { Kernel } from "./kernel";
import type { AutoSaveHandle } from "./persistence";

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

interface Captured {
  url: string;
  init: RequestInit;
}

let calls: Captured[];

beforeEach(() => {
  calls = [];
  globalThis.fetch = vi.fn(
    async (input: RequestInfo | URL, init: RequestInit = {}) => {
      calls.push({ url: String(input), init });
      return new Response("{}", { status: 200 });
    },
  ) as unknown as typeof fetch;
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("KernelStore.renameProject", () => {
  it("renames the project locally (unbound → no server PATCH)", () => {
    const store = makeStore();
    store.renameProject("  My Widget  ");
    expect(store.getState().project.name).toBe("My Widget");
    expect(store.projectName()).toBe("My Widget");
    // Guest project: no server id, so no fetch.
    expect(calls).toHaveLength(0);
  });

  it("ignores an empty / whitespace-only name", () => {
    const store = makeStore();
    const before = store.getState().project.name;
    store.renameProject("   ");
    expect(store.getState().project.name).toBe(before);
    expect(calls).toHaveLength(0);
  });

  it("is a no-op when the name is unchanged", () => {
    const store = makeStore();
    const name = store.getState().project.name;
    store.renameProject(name);
    expect(calls).toHaveLength(0);
  });

  it("PATCHes /api/projects/{id} with {name} when server-bound", () => {
    const store = makeStore();
    store.setProjectId("proj-9");
    store.renameProject("Renamed On Server");
    expect(store.getState().project.name).toBe("Renamed On Server");
    expect(store.getState().syncState).toBe("dirty");
    expect(calls).toHaveLength(1);
    expect(calls[0].url).toBe("/api/projects/proj-9");
    expect(calls[0].init.method).toBe("PATCH");
    expect(calls[0].init.credentials).toBe("include");
    expect(JSON.parse(calls[0].init.body as string)).toEqual({
      name: "Renamed On Server",
    });
  });
});
