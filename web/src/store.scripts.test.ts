/**
 * S4: store-level FieldSkript record CRUD + editor-state tests.
 *
 * Monaco can't run under jsdom, so all the testable logic lives in the
 * store and is exercised here against a fake kernel that stubs only the
 * methods `KernelStore` touches during construction, `commit()`, and
 * script CRUD. The record transpile → eval → schema pipeline runs for
 * real (sucrase), so a malformed `definition` genuinely throws.
 */

import { describe, expect, it, beforeEach } from "vitest";
import { KernelStore } from "./store";
import type { Kernel } from "./kernel";
import type { AutoSaveHandle } from "./persistence";
import type { FieldSkriptRecord } from "./fieldskript/record";

/** In-memory stand-in for the wasm kernel's script store + the handful
 *  of accessors the store's construct/commit path reads. */
class FakeKernel {
  scripts = new Map<string, FieldSkriptRecord>();
  upsertCalls: FieldSkriptRecord[] = [];
  removeCalls: string[] = [];
  featureCallArgs: { typeName: string; paramsJson: string }[] = [];
  nextFeatureId = 42;
  /** Last JSON handed to loadJson — what toJson echoes back. */
  loadedJson = '{"schema_version":1,"name":"Part","feature_calls":[],"next_feature_id":1}';
  loadJsonCalls: string[] = [];

  // ── script CRUD ──
  listScripts(): FieldSkriptRecord[] {
    return [...this.scripts.values()];
  }
  upsertScript(record: FieldSkriptRecord): boolean {
    const existed = this.scripts.has(record.id);
    this.scripts.set(record.id, record);
    this.upsertCalls.push(record);
    return existed;
  }
  removeScript(id: string): boolean {
    this.removeCalls.push(id);
    return this.scripts.delete(id);
  }
  addFeatureCall(typeName: string, paramsJson: string): number {
    this.featureCallArgs.push({ typeName, paramsJson });
    return this.nextFeatureId;
  }

  // ── document load/save ──
  loadJson(json: string) {
    this.loadJsonCalls.push(json);
    this.loadedJson = json;
    // Keep the script map in sync with any embedded scripts.
    this.scripts.clear();
    try {
      const doc = JSON.parse(json);
      if (Array.isArray(doc.scripts)) {
        for (const r of doc.scripts) this.scripts.set(r.id, r);
      }
    } catch {
      // ignore — fake kernel tolerates malformed input
    }
  }

  // ── accessors the constructor + commit path read ──
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
    // Echo the last-loaded document, minus any embedded scripts (the
    // store strips scripts before persisting). JSON.parse must succeed.
    try {
      const doc = JSON.parse(this.loadedJson);
      delete doc.scripts;
      return JSON.stringify(doc);
    } catch {
      return '{"schema_version":1,"name":"Part","feature_calls":[],"next_feature_id":1}';
    }
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

function makeStore(): { store: KernelStore; kernel: FakeKernel } {
  const kernel = new FakeKernel();
  // jsdom can't spawn the mesher Worker, so the constructor falls into
  // its sync-meshing fallback path — exactly the seam we want here.
  const store = new KernelStore(kernel as unknown as Kernel, autosave);
  return { store, kernel };
}

const VALID_SOURCE = `import { fd } from "fielddraft";
export const definition = fd.feature({
  name: "Widget",
  icon: "⬡",
  parameters: [fd.length("size", { default: 20 })],
});
export default function build() {}`;

/** Find the (single) script tab in the project, or undefined. */
function scriptTabId(store: KernelStore): string | undefined {
  return store
    .getState()
    .project.tabs.find((t) => t.kind === "script")?.id;
}

describe("KernelStore — FieldSkript tab CRUD", () => {
  let store: KernelStore;
  let kernel: FakeKernel;

  beforeEach(() => {
    ({ store, kernel } = makeStore());
  });

  it("createTab('script') appends a script tab, mirrors the record, activates it", () => {
    const id = store.createTab("script");
    const state = store.getState();
    expect(state.project.activeTabId).toBe(id);
    const tab = state.project.tabs.find((t) => t.id === id);
    expect(tab?.kind).toBe("script");
    // The starter record was mirrored into the kernel.
    expect(kernel.upsertCalls.length).toBeGreaterThanOrEqual(1);
    expect(store.listScripts()).toHaveLength(1);
  });

  it("saveScriptTab with valid source creates/updates the record", () => {
    const id = store.createTab("script");
    const result = store.saveScriptTab(id, VALID_SOURCE);
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.record.name).toBe("Widget");
    // reviseRecord keeps the id and bumps version (starter was v1).
    expect(result.record.version).toBe(2);
    // The tab label tracks the record name.
    const tab = store.getState().project.tabs.find((t) => t.id === id);
    expect(tab?.kind === "script" && tab.name).toBe("Widget");
  });

  it("saveScriptTab on malformed source returns an error and does NOT upsert", () => {
    const id = store.createTab("script");
    const upsertsBefore = kernel.upsertCalls.length;
    const result = store.saveScriptTab(id, "export const definition = 123;");
    expect(result.ok).toBe(false);
    if (result.ok) return;
    expect(result.error.length).toBeGreaterThan(0);
    expect(kernel.upsertCalls.length).toBe(upsertsBefore);
  });

  it("saveScriptTab revise bumps the version", () => {
    const id = store.createTab("script");
    const first = store.saveScriptTab(id, VALID_SOURCE);
    expect(first.ok).toBe(true);
    if (!first.ok) return;
    const revised = store.saveScriptTab(id, VALID_SOURCE.replace("size", "width"));
    expect(revised.ok).toBe(true);
    if (!revised.ok) return;
    expect(revised.record.id).toBe(first.record.id);
    expect(revised.record.version).toBe(first.record.version + 1);
    expect(revised.record.schema.params.map((p) => p.key)).toEqual(["width"]);
  });

  it("deleteTab on a script tab removes it via the kernel", () => {
    const id = store.createTab("script");
    const recordId = store.listScripts()[0].id;
    store.deleteTab(id);
    expect(kernel.removeCalls).toContain(recordId);
    expect(store.listScripts()).toHaveLength(0);
    expect(store.getState().project.tabs.some((t) => t.id === id)).toBe(false);
  });

  it("insertScriptInstance adds a feature call for a known script", () => {
    store.createTab("script");
    const recordId = store.listScripts()[0].id;
    const id = store.insertScriptInstance(recordId);
    expect(id).toBe(kernel.nextFeatureId);
    expect(kernel.featureCallArgs).toHaveLength(1);
    expect(kernel.featureCallArgs[0].typeName).toBe(recordId);
    const params = JSON.parse(kernel.featureCallArgs[0].paramsJson);
    expect(typeof params).toBe("object");
    expect(params).not.toBeNull();
  });

  it("insertScriptInstance on an unknown id returns null and does NOT call addFeatureCall", () => {
    const id = store.insertScriptInstance("nope");
    expect(id).toBeNull();
    expect(kernel.featureCallArgs).toHaveLength(0);
  });
});

describe("KernelStore — workspace tabs", () => {
  it("default project has one active part tab", () => {
    const { store } = makeStore();
    const { project } = store.getState();
    expect(project.tabs).toHaveLength(1);
    expect(project.tabs[0].kind).toBe("part");
    expect(project.activeTabId).toBe(project.tabs[0].id);
  });

  it("createTab('script') appends + activates; setActiveTab(partId) switches back", () => {
    const { store } = makeStore();
    const partId = store.getState().project.tabs[0].id;

    const scriptId = store.createTab("script");
    expect(store.getState().project.activeTabId).toBe(scriptId);

    store.setActiveTab(partId);
    expect(store.getState().project.activeTabId).toBe(partId);
  });

  it("setActiveTab with the active id is a no-op (no extra notify)", () => {
    const { store } = makeStore();
    const partId = store.getState().project.tabs[0].id;
    let notifications = 0;
    store.subscribe(() => {
      notifications += 1;
    });
    store.setActiveTab(partId);
    expect(notifications).toBe(0);
  });

  it("createTab('assembly') activates assembly mode", () => {
    const { store } = makeStore();
    const id = store.createTab("assembly");
    const state = store.getState();
    expect(state.project.activeTabId).toBe(id);
    expect(state.assembly.active).toBe(true);
  });

  it("switching to a second part tab reloads the kernel document", () => {
    const { store, kernel } = makeStore();
    const loadsBefore = kernel.loadJsonCalls.length;
    const secondId = store.createTab("part");
    // createTab flushes + setActiveTab(part) reloads the kernel.
    expect(kernel.loadJsonCalls.length).toBeGreaterThan(loadsBefore);
    expect(store.getState().project.activeTabId).toBe(secondId);
    expect(store.getState().project.tabs.filter((t) => t.kind === "part")).toHaveLength(2);
  });
});

describe("KernelStore — script-type toolbar meta", () => {
  it("saveScriptTab surfaces the new script in state.scriptTypes", () => {
    const { store } = makeStore();
    expect(store.getState().scriptTypes).toHaveLength(0);

    const id = store.createTab("script");
    const result = store.saveScriptTab(id, VALID_SOURCE);
    expect(result.ok).toBe(true);
    if (!result.ok) return;

    const types = store.getState().scriptTypes;
    expect(types).toEqual([{ id: result.record.id, name: "Widget", icon: "⬡" }]);
  });

  it("deleteTab removes the script from state.scriptTypes", () => {
    const { store } = makeStore();
    store.createTab("script");
    expect(store.getState().scriptTypes).toHaveLength(1);

    const id = scriptTabId(store);
    expect(id).toBeDefined();
    store.deleteTab(id!);
    expect(store.getState().scriptTypes).toHaveLength(0);
  });
});
