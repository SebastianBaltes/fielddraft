/**
 * `KernelStore.transformBody` — the Move-tool core both entry points share:
 * a scene click on a body (Viewport host handler) and a Parts-list row click
 * (Outliner, ticket 7hk3). When the gizmo is armed it materialises (or reuses)
 * a transform feature of the active `gizmoMode` targeting the clicked body and
 * selects it, so the transform gizmo appears. Consuming the arm and reusing the
 * last matching transform keeps repeated moves from stacking features.
 */

import { describe, expect, it, beforeEach } from "vitest";
import { KernelStore } from "./store";
import type { Kernel } from "./kernel";
import type { AutoSaveHandle } from "./persistence";

interface FakeCall {
  id: number;
  kind: string;
  params: Record<string, unknown>;
}

/** In-memory kernel covering only the feature-add/list/params path that
 *  `transformBody` + `commit` touch. */
class FakeKernel {
  calls: FakeCall[] = [{ id: 6, kind: "box", params: {} }];
  nextId = 7;

  addFeatureCall(kind: string, paramsJson: string): number {
    const id = this.nextId++;
    this.calls.push({ id, kind, params: JSON.parse(paramsJson) });
    return id;
  }
  getFeatureCallParams(id: number): Record<string, unknown> {
    const c = this.calls.find((x) => x.id === id);
    if (!c) throw new Error(`no call ${id}`);
    return c.params;
  }
  listFeatures() {
    return this.calls.map((c) => ({
      id: c.id,
      name: "",
      op: "add",
      kind: c.kind,
      translate: [0, 0, 0],
      rotation: [0, 0, 0, 1],
      scale: [1, 1, 1],
    }));
  }
  // Plumbing commit()/buildState read.
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
  setBrepSuppressed() {
    return false;
  }
}

const autosave: AutoSaveHandle = {
  schedule: () => {},
  flush: () => {},
  cancel: () => {},
};

describe("KernelStore.transformBody", () => {
  let kernel: FakeKernel;
  let store: KernelStore;
  beforeEach(() => {
    kernel = new FakeKernel();
    store = new KernelStore(kernel as unknown as Kernel, autosave);
  });

  it("materialises a translate feature targeting the body and selects it", () => {
    store.setGizmoMode("translate");
    const id = store.transformBody(6);
    expect(id).not.toBeNull();
    const created = kernel.calls.find((c) => c.id === id);
    expect(created?.kind).toBe("translate");
    expect(created?.params.target_id).toBe(6);
    // The new transform is selected so the gizmo mounts.
    expect([...store.getState().selection]).toEqual([id]);
    // The arm is consumed (single-shot Move).
    expect(store.getState().gizmoArmed).toBe(false);
  });

  it("reuses the last matching transform instead of stacking", () => {
    store.setGizmoMode("translate");
    const first = store.transformBody(6);
    store.setGizmoMode("translate");
    const second = store.transformBody(6);
    expect(second).toBe(first);
    const translates = kernel.calls.filter((c) => c.kind === "translate");
    expect(translates.length).toBe(1);
  });

  it("creates the feature for the active gizmoMode (rotate)", () => {
    store.setGizmoMode("rotate");
    const id = store.transformBody(6);
    expect(kernel.calls.find((c) => c.id === id)?.kind).toBe("rotate");
  });

  it("rejects an invalid body feature id", () => {
    store.setGizmoMode("translate");
    expect(store.transformBody(0)).toBeNull();
  });
});
