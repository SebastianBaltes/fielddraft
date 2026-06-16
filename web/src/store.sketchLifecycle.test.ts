/**
 * Tickets 7hgr / vev1 / 5zd3 — sketch-session lifecycle store invariants.
 *
 *  - 7hgr (a): a SCENE-origin selection change must NEVER end an active
 *    sketch session (stickiness); a LIST-origin change keeps the pqoe
 *    behaviour of committing on switch-away.
 *  - vev1: deleting the sketch currently open for editing exits the session.
 *  - 5zd3: New Project fully clears any lingering session.
 *
 * Exercised at the store level against a minimal in-memory kernel that
 * covers only the sketch construct / commit / remove path these flows touch.
 */

import { describe, expect, it, beforeEach } from "vitest";
import { KernelStore } from "./store";
import type { Kernel } from "./kernel";
import type { AutoSaveHandle } from "./persistence";

/** A fake kernel carrying a single committed `sketch` feature (id 1) so the
 *  edit-session round-trip (`getSketchSubCalls` / `getFeatureCallParams`)
 *  and `removeFeature` work without the real WASM kernel. */
class FakeKernel {
  features: { id: number; name: string; op: string; kind: string }[] = [];
  loadedJson = '{"schema_version":1,"name":"Part","feature_calls":[],"next_feature_id":1}';
  loadJson(json: string) {
    this.loadedJson = json;
  }
  listScripts() {
    return [];
  }
  upsertScript() {
    return false;
  }
  removeScript() {
    return false;
  }
  listFeatures() {
    return this.features.map((f) => ({
      ...f,
      translate: [0, 0, 0],
      rotation: [0, 0, 0, 1],
      scale: [1, 1, 1],
    }));
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
  // Sketch-edit round-trip.
  getSketchSubCalls() {
    return [{ type: "sketch_line", params: {} }];
  }
  getFeatureCallParams() {
    return { plane: "qCreatedBy(origin_plane_xy)" };
  }
  removeFeature(id: number) {
    const before = this.features.length;
    this.features = this.features.filter((f) => f.id !== id);
    return this.features.length < before;
  }
  replaceSketch() {
    return true;
  }
  addSketch() {
    const id = this.features.length + 1;
    this.features.push({ id, name: `Sketch ${id}`, op: "new", kind: "sketch" });
    return id;
  }
}

const autosave: AutoSaveHandle = {
  schedule: () => {},
  flush: () => {},
  cancel: () => {},
};

function makeStore(seedSketch = false): { store: KernelStore; kernel: FakeKernel } {
  const kernel = new FakeKernel();
  if (seedSketch) {
    kernel.features.push({ id: 1, name: "Sketch 1", op: "new", kind: "sketch" });
  }
  const store = new KernelStore(kernel as unknown as Kernel, autosave);
  return { store, kernel };
}

describe("KernelStore — sketch-session lifecycle", () => {
  let store: KernelStore;
  let kernel: FakeKernel;
  beforeEach(() => {
    ({ store, kernel } = makeStore(true));
    // The store committed an empty doc on construction; re-sync features.
    store.selectOnly(0); // harmless reset; commit() below re-reads features.
  });

  // ── 7hgr (a): scene clicks never end an active session ──────────────
  it("scene-origin selection change does NOT exit an active session", () => {
    store.enterSketch("qCreatedBy(origin_plane_xy)");
    expect(store.getState().sketchSession.active).toBe(true);

    // A 3D-scene click selecting some other feature/body.
    store.selectOnly(1, "scene");
    expect(store.getState().sketchSession.active).toBe(true);

    store.clearSelection("scene");
    expect(store.getState().sketchSession.active).toBe(true);

    store.toggleSelect(1, "scene");
    expect(store.getState().sketchSession.active).toBe(true);
  });

  it("list-origin selection of a non-sketch still commits an editing session", () => {
    // Editing the seeded sketch (id 1).
    store.editSketch(1);
    expect(store.getState().sketchSession.active).toBe(true);
    expect(store.getState().sketchSession.editingFeatureId).toBe(1);

    // A deliberate LIST click on another feature commits the sketch.
    store.selectOnly(2 /* default origin = "list" */);
    expect(store.getState().sketchSession.active).toBe(false);
  });

  // ── vev1: deleting the open sketch closes the view ──────────────────
  it("deleting the sketch being edited exits the session", () => {
    store.editSketch(1);
    expect(store.getState().sketchSession.active).toBe(true);
    expect(store.getState().sketchSession.editingFeatureId).toBe(1);

    store.removeFeature(1);

    const session = store.getState().sketchSession;
    expect(session.active).toBe(false);
    expect(session.editingFeatureId).toBeNull();
    expect(session.subCalls).toEqual([]);
  });

  it("deleting a DIFFERENT feature leaves the edit session open", () => {
    kernel.features.push({ id: 2, name: "Box", op: "new", kind: "box" });
    store.editSketch(1);
    store.removeFeature(2);
    expect(store.getState().sketchSession.active).toBe(true);
    expect(store.getState().sketchSession.editingFeatureId).toBe(1);
  });

  // ── 5zd3: New Project clears any lingering session ──────────────────
  it("newProject fully clears an active session", () => {
    store.enterSketch("qCreatedBy(origin_plane_xy)");
    expect(store.getState().sketchSession.active).toBe(true);

    store.newProject();

    const session = store.getState().sketchSession;
    expect(session.active).toBe(false);
    expect(session.editingFeatureId).toBeNull();
    expect(session.subCalls).toEqual([]);
  });

  // ── r3ci: sketch-local undo/redo of drawn geometry ──────────────────
  const line = (i: number) => ({
    type: "sketch_line",
    params: { start: [0, 0], end: [i, i] },
  });

  it("undo removes the last drawn sub-call; redo restores it", () => {
    store.enterSketch("qCreatedBy(origin_plane_xy)");
    store.pushSketchSubCalls([line(1)]);
    store.pushSketchSubCalls([line(2)]);
    expect(store.getState().sketchSession.subCalls).toHaveLength(2);

    expect(store.sketchUndo()).toBe(true);
    expect(store.getState().sketchSession.subCalls).toHaveLength(1);

    expect(store.sketchUndo()).toBe(true);
    expect(store.getState().sketchSession.subCalls).toHaveLength(0);

    // Nothing left to undo.
    expect(store.sketchUndo()).toBe(false);

    expect(store.sketchRedo()).toBe(true);
    expect(store.getState().sketchSession.subCalls).toHaveLength(1);
    expect(store.sketchRedo()).toBe(true);
    expect(store.getState().sketchSession.subCalls).toHaveLength(2);
    expect(store.sketchRedo()).toBe(false);
  });

  it("a fresh draw after undo clears the redo branch", () => {
    store.enterSketch("qCreatedBy(origin_plane_xy)");
    store.pushSketchSubCalls([line(1)]);
    store.pushSketchSubCalls([line(2)]);
    store.sketchUndo(); // back to 1
    store.pushSketchSubCalls([line(3)]); // new branch — redo of line(2) gone
    expect(store.getState().sketchSession.subCalls).toHaveLength(2);
    expect(store.sketchRedo()).toBe(false);
  });

  it("undo is scoped to the session and resets on enter", () => {
    store.enterSketch("qCreatedBy(origin_plane_xy)");
    store.pushSketchSubCalls([line(1)]);
    store.exitSketch();
    // Re-entering starts fresh: no cross-session undo.
    store.enterSketch("qCreatedBy(origin_plane_xy)");
    expect(store.sketchUndo()).toBe(false);
  });

  it("sketchUndo/Redo are no-ops outside a session", () => {
    expect(store.sketchUndo()).toBe(false);
    expect(store.sketchRedo()).toBe(false);
  });
});
