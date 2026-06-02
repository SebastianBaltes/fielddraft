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
    return "{}";
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

describe("KernelStore — FieldSkript CRUD", () => {
  let store: KernelStore;
  let kernel: FakeKernel;

  beforeEach(() => {
    ({ store, kernel } = makeStore());
  });

  it("saveScript creates a new record from valid source", () => {
    const result = store.saveScript({ source: VALID_SOURCE });
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.record.name).toBe("Widget");
    expect(result.record.version).toBe(1);
    expect(result.record.id).toBe("widget"); // slugged from the name
    expect(kernel.upsertCalls).toHaveLength(1);
    expect(kernel.upsertCalls[0].id).toBe("widget");
    expect(store.listScripts().map((r) => r.id)).toEqual(["widget"]);
  });

  it("saveScript on malformed source returns an error and does NOT upsert", () => {
    const result = store.saveScript({ source: "export const definition = 123;" });
    expect(result.ok).toBe(false);
    if (result.ok) return;
    expect(result.error.length).toBeGreaterThan(0);
    expect(kernel.upsertCalls).toHaveLength(0);
    expect(store.listScripts()).toHaveLength(0);
  });

  it("saveScript with an existing id revises and bumps the version", () => {
    const first = store.saveScript({ source: VALID_SOURCE });
    expect(first.ok).toBe(true);
    if (!first.ok) return;

    const revised = store.saveScript({
      id: first.record.id,
      source: VALID_SOURCE.replace("size", "width"),
    });
    expect(revised.ok).toBe(true);
    if (!revised.ok) return;
    expect(revised.record.id).toBe(first.record.id);
    expect(revised.record.version).toBe(2);
    expect(revised.record.schema.params.map((p) => p.key)).toEqual(["width"]);
    // Still a single record under that id.
    expect(store.listScripts()).toHaveLength(1);
  });

  it("dedupes derived ids for distinct same-named scripts", () => {
    const a = store.saveScript({ source: VALID_SOURCE });
    const b = store.saveScript({ source: VALID_SOURCE });
    expect(a.ok && b.ok).toBe(true);
    if (!a.ok || !b.ok) return;
    expect(a.record.id).toBe("widget");
    expect(b.record.id).toBe("widget_2");
    expect(store.listScripts()).toHaveLength(2);
  });

  it("deleteScript removes via the kernel", () => {
    const saved = store.saveScript({ source: VALID_SOURCE });
    expect(saved.ok).toBe(true);
    if (!saved.ok) return;
    store.deleteScript(saved.record.id);
    expect(kernel.removeCalls).toEqual([saved.record.id]);
    expect(store.listScripts()).toHaveLength(0);
  });

  it("insertScriptInstance adds a feature call for a known script", () => {
    const saved = store.saveScript({ source: VALID_SOURCE });
    expect(saved.ok).toBe(true);
    if (!saved.ok) return;

    const id = store.insertScriptInstance(saved.record.id);
    expect(id).toBe(kernel.nextFeatureId);
    expect(kernel.featureCallArgs).toHaveLength(1);
    expect(kernel.featureCallArgs[0].typeName).toBe(saved.record.id);
    // Default params are a JSON object derived from the script's schema.
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
  it("setActiveTab flips activeTab part↔script and notifies", () => {
    const { store } = makeStore();
    let notifications = 0;
    store.subscribe(() => {
      notifications += 1;
    });

    expect(store.getState().activeTab).toBe("part");

    store.setActiveTab("script");
    expect(store.getState().activeTab).toBe("script");
    expect(notifications).toBe(1);

    // Idempotent: setting the same tab is a no-op (no extra notify).
    store.setActiveTab("script");
    expect(notifications).toBe(1);

    store.setActiveTab("part");
    expect(store.getState().activeTab).toBe("part");
    expect(notifications).toBe(2);
  });
});

describe("KernelStore — script-type toolbar meta", () => {
  it("saveScript surfaces the new script in state.scriptTypes", () => {
    const { store } = makeStore();
    expect(store.getState().scriptTypes).toHaveLength(0);

    const result = store.saveScript({ source: VALID_SOURCE });
    expect(result.ok).toBe(true);
    if (!result.ok) return;

    const types = store.getState().scriptTypes;
    expect(types).toEqual([{ id: "widget", name: "Widget", icon: "⬡" }]);
  });

  it("deleteScript removes the script from state.scriptTypes", () => {
    const { store } = makeStore();
    const saved = store.saveScript({ source: VALID_SOURCE });
    expect(saved.ok).toBe(true);
    if (!saved.ok) return;
    expect(store.getState().scriptTypes).toHaveLength(1);

    store.deleteScript(saved.record.id);
    expect(store.getState().scriptTypes).toHaveLength(0);
  });
});
