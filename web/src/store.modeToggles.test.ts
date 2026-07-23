/**
 * Mode-toggle mutual-exclusion tests for the viewport interaction modes
 * (fillet / mate / measure). These tools are mode toggles on the main toolbar:
 * activating one clears the others so the next viewport click is unambiguous.
 * Thicken is NOT a mode anymore (#401): the toolbar button creates a thicken
 * feature and opens its dialog, exactly like Extrude/Loft.
 */

import { describe, expect, it, beforeEach } from "vitest";
import { KernelStore } from "./store";
import type { Kernel } from "./kernel";
import type { AutoSaveHandle } from "./persistence";

/** Minimal in-memory kernel covering only the construct path KernelStore
 *  touches before any feature edit. */
class FakeKernel {
  loadedJson =
    '{"schema_version":1,"name":"Part","feature_calls":[],"next_feature_id":1}';
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
  const kernel = new FakeKernel();
  return new KernelStore(kernel as unknown as Kernel, autosave);
}

describe("KernelStore — interaction-mode toggles", () => {
  let store: KernelStore;
  beforeEach(() => {
    store = makeStore();
  });

  it("all interaction modes start off", () => {
    const s = store.getState();
    expect(s.filletMode).toBe(false);
    expect(s.mateMode).toBe(false);
    expect(s.measureMode).toBe(false);
  });

  it("toggleFilletMode flips the flag and notifies", () => {
    let notifications = 0;
    store.subscribe(() => {
      notifications += 1;
    });
    store.toggleFilletMode();
    expect(store.getState().filletMode).toBe(true);
    expect(notifications).toBe(1);
    store.toggleFilletMode();
    expect(store.getState().filletMode).toBe(false);
    expect(notifications).toBe(2);
  });

  it("activating one mode clears the others (mutual exclusion)", () => {
    store.toggleFilletMode();
    store.toggleMateMode();
    let s = store.getState();
    expect(s.mateMode).toBe(true);
    expect(s.filletMode).toBe(false);

    store.toggleMeasureMode();
    s = store.getState();
    expect(s.measureMode).toBe(true);
    expect(s.mateMode).toBe(false);
    expect(s.filletMode).toBe(false);
  });

  it("at most one interaction mode is active at a time", () => {
    const activeCount = () => {
      const s = store.getState();
      return [s.filletMode, s.mateMode, s.measureMode].filter(Boolean).length;
    };
    store.toggleFilletMode();
    expect(activeCount()).toBe(1);
    store.toggleMateMode();
    expect(activeCount()).toBe(1);
    store.toggleMeasureMode();
    expect(activeCount()).toBe(1);
  });
});
