/**
 * Chunk 3a: persistent, tool-free B-Rep element-selection store tests.
 *
 * Exercises the mutual exclusivity between the feature-level `selection` set
 * and the ephemeral `selectedElement`, plus notify behaviour. Element
 * selection must NOT persist, so we never assert autosave here.
 */

import { describe, expect, it, beforeEach } from "vitest";
import { KernelStore } from "./store";
import type { Kernel } from "./kernel";
import type { AutoSaveHandle } from "./persistence";
import type { PickedElement } from "./elementQuery";

/** Minimal in-memory kernel covering only the construct/commit/selection
 *  path KernelStore touches in these tests. */
class FakeKernel {
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

const FACE: PickedElement = { kind: "face", idHash: "12345" };

describe("KernelStore — element selection", () => {
  let store: KernelStore;
  beforeEach(() => {
    store = makeStore();
  });

  it("starts with no element selected", () => {
    expect(store.getState().selectedElement).toBeNull();
  });

  it("selectElement sets selectedElement and notifies", () => {
    let notifications = 0;
    store.subscribe(() => {
      notifications += 1;
    });
    store.selectElement(FACE);
    expect(store.getState().selectedElement).toEqual(FACE);
    expect(notifications).toBe(1);
  });

  it("clearElementSelection resets to null and notifies", () => {
    store.selectElement(FACE);
    let notifications = 0;
    store.subscribe(() => {
      notifications += 1;
    });
    store.clearElementSelection();
    expect(store.getState().selectedElement).toBeNull();
    expect(notifications).toBe(1);
  });

  it("clearElementSelection is a no-op when nothing is selected", () => {
    let notifications = 0;
    store.subscribe(() => {
      notifications += 1;
    });
    store.clearElementSelection();
    expect(notifications).toBe(0);
  });

  it("selecting an element clears the feature selection set", () => {
    store.selectOnly(7);
    expect(store.getState().selection.has(7)).toBe(true);
    store.selectElement(FACE);
    expect(store.getState().selection.size).toBe(0);
    expect(store.getState().selectionAnchor).toBeNull();
    expect(store.getState().selectedElement).toEqual(FACE);
  });

  it("selecting a feature clears the element selection", () => {
    store.selectElement(FACE);
    store.selectOnly(3);
    expect(store.getState().selectedElement).toBeNull();
    expect(store.getState().selection.has(3)).toBe(true);
  });

  it("clearSelection clears both notions", () => {
    store.selectElement(FACE);
    store.clearSelection();
    expect(store.getState().selectedElement).toBeNull();
    expect(store.getState().selection.size).toBe(0);
  });
});
