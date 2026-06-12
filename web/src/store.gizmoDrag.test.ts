/**
 * Drag B-Rep suppression (setting "B-Rep rebuild while dragging", default
 * off): a transform-gizmo drag must flip `state.gizmoDragSuppressed` so the
 * viewport hides every B-Rep overlay for the drag's duration — cached edges
 * only refresh at the 50ms commit cadence and would trail the live mesh —
 * and mirror the suppression into the kernel (`setBrepSuppressed`).
 */

import { describe, expect, it, beforeEach } from "vitest";
import { KernelStore } from "./store";
import type { Kernel } from "./kernel";
import type { AutoSaveHandle } from "./persistence";

/** Minimal in-memory kernel covering only the construct/commit path plus
 *  the suppression flag KernelStore touches in these tests. */
class FakeKernel {
  brepSuppressed = false;
  loadedJson = '{"schema_version":1,"name":"Part","feature_calls":[],"next_feature_id":1}';
  loadJson(json: string) {
    this.loadedJson = json;
  }
  listScripts() {
    return [];
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
  setBrepSuppressed(on: boolean): boolean {
    if (this.brepSuppressed === on) return false;
    this.brepSuppressed = on;
    return true;
  }
}

const autosave: AutoSaveHandle = {
  schedule: () => {},
  flush: () => {},
  cancel: () => {},
};

describe("KernelStore — gizmo-drag B-Rep suppression", () => {
  let kernel: FakeKernel;
  let store: KernelStore;
  beforeEach(() => {
    kernel = new FakeKernel();
    store = new KernelStore(kernel as unknown as Kernel, autosave);
  });

  it("starts unsuppressed", () => {
    expect(store.getState().gizmoDragSuppressed).toBe(false);
    expect(kernel.brepSuppressed).toBe(false);
  });

  it("begin/end drag toggles state + kernel flag and notifies (default off)", () => {
    let notifications = 0;
    store.subscribe(() => {
      notifications += 1;
    });
    store.beginGizmoDrag();
    expect(store.getState().gizmoDragSuppressed).toBe(true);
    expect(kernel.brepSuppressed).toBe(true);
    expect(notifications).toBe(1);
    store.endGizmoDrag();
    expect(store.getState().gizmoDragSuppressed).toBe(false);
    expect(kernel.brepSuppressed).toBe(false);
    expect(notifications).toBe(2);
  });

  it("endGizmoDrag without a drag is a no-op", () => {
    let notifications = 0;
    store.subscribe(() => {
      notifications += 1;
    });
    store.endGizmoDrag();
    expect(notifications).toBe(0);
  });

  it("setting dragBrepRecompute=true keeps drags live (no suppression)", () => {
    store.setDragBrepRecompute(true);
    store.beginGizmoDrag();
    expect(store.getState().gizmoDragSuppressed).toBe(false);
    expect(kernel.brepSuppressed).toBe(false);
    store.endGizmoDrag();
    expect(store.getState().gizmoDragSuppressed).toBe(false);
  });

  it("flipping the setting ON mid-drag lifts the active suppression", () => {
    store.beginGizmoDrag();
    expect(store.getState().gizmoDragSuppressed).toBe(true);
    store.setDragBrepRecompute(true);
    expect(store.getState().gizmoDragSuppressed).toBe(false);
    expect(kernel.brepSuppressed).toBe(false);
    store.endGizmoDrag();
    expect(store.getState().gizmoDragSuppressed).toBe(false);
  });
});
