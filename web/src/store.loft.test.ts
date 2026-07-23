/**
 * #385 P4 — `KernelStore.addLoft` wiring.
 *
 * Exercised at the store level against a minimal in-memory kernel that records
 * `addLoft` calls (the real MultiQuery resolve runs against the wasm kernel in
 * the app; here we assert the store → kernel translation flow, mirroring the
 * existing sketch-lifecycle store tests).
 */

import { describe, expect, it, beforeEach } from "vitest";
import { KernelStore } from "./store";
import type { Kernel } from "./kernel";
import type { AutoSaveHandle } from "./persistence";
import { BoolOpTag } from "./kernel";

/** A fake kernel that records every `addLoft(profiles, op)` and appends a loft
 *  feature so `commit()` re-reads a stable feature list. */
class FakeKernel {
  features: { id: number; name: string; op: string; kind: string }[] = [];
  loftCalls: { profiles: string; op: number }[] = [];
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
  removeFeature(id: number) {
    const before = this.features.length;
    this.features = this.features.filter((f) => f.id !== id);
    return this.features.length < before;
  }
  addLoft(profiles: string, op: number) {
    this.loftCalls.push({ profiles, op });
    const id = this.features.length + 1;
    this.features.push({ id, name: `Loft ${id}`, op: "new", kind: "loft" });
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

describe("KernelStore — addLoft (#385 P4)", () => {
  let store: KernelStore;
  let kernel: FakeKernel;
  beforeEach(() => {
    ({ store, kernel } = makeStore());
  });

  it("joins N profile queries into a single comma-separated MultiQuery", () => {
    const id = store.addLoft([
      "qCreatedBy(1)",
      "qCreatedBy(2)",
      "qFace(abc)",
    ]);
    expect(id).not.toBeNull();
    expect(kernel.loftCalls).toHaveLength(1);
    expect(kernel.loftCalls[0].profiles).toBe(
      "qCreatedBy(1),qCreatedBy(2),qFace(abc)",
    );
    // Uses the store's current boolean op (default New).
    expect(kernel.loftCalls[0].op).toBe(BoolOpTag.New);
  });

  it("creates an EMPTY loft when no profiles are picked (dialog fills them)", () => {
    const id = store.addLoft([]);
    expect(id).not.toBeNull();
    expect(kernel.loftCalls).toHaveLength(1);
    expect(kernel.loftCalls[0].profiles).toBe("");
  });

  it("defaults the argument to an empty selection", () => {
    const id = store.addLoft();
    expect(id).not.toBeNull();
    expect(kernel.loftCalls[0].profiles).toBe("");
  });

  it("selects the new loft feature after creation", () => {
    const id = store.addLoft(["qCreatedBy(1)", "qCreatedBy(2)"]);
    expect(store.getState().selection.has(id!)).toBe(true);
  });

  it("passes the store's current boolean op through to the kernel", () => {
    store.setNextOp(BoolOpTag.Add);
    store.addLoft(["qCreatedBy(1)", "qCreatedBy(2)"]);
    expect(kernel.loftCalls[0].op).toBe(BoolOpTag.Add);
  });

  it("returns null and surfaces a transient error on a kernel throw", () => {
    kernel.addLoft = () => {
      throw new Error("not coplanar");
    };
    const id = store.addLoft(["qCreatedBy(1)", "qCreatedBy(2)"]);
    expect(id).toBeNull();
    expect(store.getState().transientError).toBeTruthy();
  });
});
