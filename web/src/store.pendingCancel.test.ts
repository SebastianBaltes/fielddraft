/**
 * #400 — create-mode vs edit-mode Cancel semantics.
 *
 * A feature freshly created by a tool click opens its detail dialog in
 * "create mode": Cancel there must DELETE the feature (document restored to
 * before the tool click). Re-entering an EXISTING feature's dialog from the
 * feature list opens it in "edit mode": Cancel there must NOT delete anything.
 *
 * The store draws this line with the `pendingFeature` flag — every "add
 * feature" path funnels through `commitNewFeature`, which flags the new id;
 * selecting an existing feature never does. These tests pin that contract at
 * the store level (the Outliner's ✗ / Escape both dispatch off
 * `isPendingFeature`).
 */

import { describe, expect, it, beforeEach } from "vitest";
import { KernelStore } from "./store";
import type { Kernel } from "./kernel";
import type { AutoSaveHandle } from "./persistence";

/** Minimal in-memory kernel: enough of the add / list / remove surface for the
 *  pending-feature lifecycle. `addLoft` and `addBox` both append a feature and
 *  return its id, so we can exercise more than one creation path. */
class FakeKernel {
  features: { id: number; name: string; op: string; kind: string }[] = [];
  private nextId = 1;
  loadedJson =
    '{"schema_version":1,"name":"Part","feature_calls":[],"next_feature_id":1}';
  loadJson(json: string) {
    this.loadedJson = json;
  }
  toJson() {
    return this.loadedJson;
  }
  listScripts() {
    return [];
  }
  listVariables() {
    return [];
  }
  listFeatures() {
    return this.features.map((f) => ({
      ...f,
      translate: [0, 0, 0],
      rotation: [0, 0, 0, 1],
      scale: [1, 1, 1],
    }));
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
  removeFeature(id: number) {
    const before = this.features.length;
    this.features = this.features.filter((f) => f.id !== id);
    return this.features.length < before;
  }
  addLoft() {
    const id = this.nextId++;
    this.features.push({ id, name: `Loft ${id}`, op: "new", kind: "loft" });
    return id;
  }
  addBox() {
    const id = this.nextId++;
    this.features.push({ id, name: `Box ${id}`, op: "new", kind: "box" });
    return id;
  }
}

const autosave: AutoSaveHandle = {
  schedule: () => {},
  flush: () => {},
  cancel: () => {},
};

function makeStore(): { store: KernelStore; kernel: FakeKernel } {
  const kernel = new FakeKernel();
  const store = new KernelStore(kernel as unknown as Kernel, autosave);
  return { store, kernel };
}

describe("KernelStore — create-mode Cancel deletes the feature (#400)", () => {
  let store: KernelStore;
  let kernel: FakeKernel;
  beforeEach(() => {
    ({ store, kernel } = makeStore());
  });

  it("flags a freshly-created feature as pending (create-mode)", () => {
    const id = store.addLoft([]);
    expect(id).not.toBeNull();
    expect(store.isPendingFeature(id!)).toBe(true);
  });

  it("flags a primitive tool-add as pending too, not just extrude", () => {
    const id = store.addPrimitive("box");
    expect(store.isPendingFeature(id)).toBe(true);
  });

  it("cancelPendingFeature removes the just-created feature entirely", () => {
    const id = store.addLoft([]);
    expect(kernel.features.some((f) => f.id === id)).toBe(true);
    store.cancelPendingFeature();
    expect(kernel.features.some((f) => f.id === id)).toBe(false);
    expect(store.isPendingFeature(id!)).toBe(false);
    expect(store.getState().features).toHaveLength(0);
  });

  it("creating a second feature CONFIRMS (keeps) the first, only the second stays pending", () => {
    const first = store.addLoft([]);
    const second = store.addPrimitive("box");
    expect(store.isPendingFeature(first!)).toBe(false);
    expect(store.isPendingFeature(second)).toBe(true);
    // The first feature survives — confirming a pending feature never deletes.
    expect(kernel.features.some((f) => f.id === first)).toBe(true);
  });
});

describe("KernelStore — edit-mode Cancel keeps the feature (#400)", () => {
  let store: KernelStore;
  let kernel: FakeKernel;
  beforeEach(() => {
    ({ store, kernel } = makeStore());
  });

  it("re-selecting an existing feature is NOT pending (edit-mode)", () => {
    const id = store.addLoft([]);
    // Move selection away (confirms the create), then re-enter from the list.
    store.clearSelection();
    expect(store.isPendingFeature(id!)).toBe(false);
    store.selectOnly(id!);
    expect(store.isPendingFeature(id!)).toBe(false);
  });

  it("cancelPendingFeature is a no-op once the feature was confirmed", () => {
    const id = store.addLoft([]);
    store.clearSelection(); // confirm (OK path)
    store.selectOnly(id!); // re-open in edit-mode
    store.cancelPendingFeature(); // Outliner would take the undo path instead
    // Nothing deleted: the existing feature is untouched.
    expect(kernel.features.some((f) => f.id === id)).toBe(true);
    expect(store.getState().features).toHaveLength(1);
  });
});
