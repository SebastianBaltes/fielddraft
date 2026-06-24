/**
 * Async-weave import → fresh snapshot (tickets 1do9 / zhfy).
 *
 * The B-Rep edge overlay reads `kernel.bodyBrepEdges(i)` through a memo keyed
 * on the store snapshot. The weaver worker imports the freshly-woven B-Rep
 * AFTER the document is already loaded (on the LOAD path it lands after the
 * first surface-only frame). For the overlay to pick the edges up, that import
 * MUST publish a NEW `getState()` reference (a bumped `brepVersion`) — a bare
 * `notify()` leaves the reference unchanged and `useSyncExternalStore` bails,
 * so the loaded body would render its faces but no edges until an unrelated
 * state change (see `reference_store_notify_needs_new_snapshot`).
 *
 * This pins the store contract that the overlay's load-path repaint depends on.
 */

import { describe, expect, it, beforeEach, afterEach } from "vitest";
import { KernelStore } from "./store";
import type { Kernel } from "./kernel";
import type { AutoSaveHandle } from "./persistence";

/** A Worker stub that records every posted message and exposes a hook to
 *  push a crafted response back to the listener the store registered. */
class FakeWorker {
  static instances: FakeWorker[] = [];
  posted: unknown[] = [];
  private listener: ((event: MessageEvent) => void) | null = null;
  constructor() {
    FakeWorker.instances.push(this);
  }
  addEventListener(_type: string, cb: (event: MessageEvent) => void) {
    this.listener = cb;
  }
  postMessage(msg: unknown) {
    this.posted.push(msg);
  }
  terminate() {}
  /** Deliver a message to the store's handler synchronously. */
  emit(data: unknown) {
    this.listener?.({ data } as MessageEvent);
  }
}

/** Minimal kernel covering the construct/commit + weave-import path. One body
 *  whose B-Rep starts stale and becomes current once `importBodyBrep` runs;
 *  the body version bumps on import exactly like the real kernel. */
class FakeKernel {
  loadedJson =
    '{"schema_version":1,"name":"Part","feature_calls":[],"next_feature_id":1}';
  private brepCurrent = false;
  bodyVer = 1;
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
    return { size: [0, 0, 0], min: [0, 0, 0], max: [0, 0, 0] };
  }
  setSyncMeshing() {
    return false;
  }
  setBrepSuppressed() {
    return false;
  }
  meshResolution() {
    return 32;
  }
  contextMeshResolution() {
    return 16;
  }
  activeBodyId() {
    return 0;
  }
  clearActiveBodyId() {}
  setActiveBodyByFeature() {}
  bodyCount() {
    return 1;
  }
  bodyId() {
    return 1;
  }
  bodyVersion() {
    return this.bodyVer;
  }
  /** Body 1's B-Rep is stale until imported. The `Kernel` wrapper parses the
   *  kernel's JSON to a number[] before the store sees it. */
  bodyBrepStaleIds(): number[] {
    return this.brepCurrent ? [] : [1];
  }
  /** Install the woven B-Rep — marks current + bumps the version, like the
   *  real `import_body_brep`. */
  importBodyBrep() {
    if (this.brepCurrent) return false;
    this.brepCurrent = true;
    this.bodyVer += 1;
    return true;
  }
}

const autosave: AutoSaveHandle = {
  schedule: () => {},
  flush: () => {},
  cancel: () => {},
};

describe("KernelStore — async-weave import publishes a fresh snapshot", () => {
  let kernel: FakeKernel;
  let store: KernelStore;
  const OrigWorker = (globalThis as { Worker?: unknown }).Worker;

  beforeEach(() => {
    FakeWorker.instances = [];
    (globalThis as { Worker?: unknown }).Worker =
      FakeWorker as unknown as typeof Worker;
    kernel = new FakeKernel();
    store = new KernelStore(kernel as unknown as Kernel, autosave);
  });
  afterEach(() => {
    (globalThis as { Worker?: unknown }).Worker = OrigWorker;
  });

  it("brings up the weaver and posts the initial (load-path) weave", () => {
    // Two workers come up: mesher then weaver. The weaver got the load weave.
    expect(FakeWorker.instances.length).toBe(2);
    const weaver = FakeWorker.instances[1];
    const weaveMsgs = weaver.posted.filter(
      (m): m is { type: string; bodyIds: number[] } =>
        typeof m === "object" && m !== null && (m as { type?: string }).type === "weave",
    );
    expect(weaveMsgs.length).toBeGreaterThanOrEqual(1);
    expect(weaveMsgs[weaveMsgs.length - 1].bodyIds).toEqual([1]);
  });

  it("import bumps brepVersion AND swaps the getState() reference", () => {
    const weaver = FakeWorker.instances[1];
    const before = store.getState();
    let notifications = 0;
    store.subscribe(() => {
      notifications += 1;
    });

    // The worker replies for the latest weave request id with one body.
    const lastReq = weaver.posted
      .filter(
        (m): m is { type: string; requestId: number } =>
          typeof m === "object" &&
          m !== null &&
          (m as { type?: string }).type === "weave",
      )
      .pop()!;
    weaver.emit({
      type: "breps",
      requestId: lastReq.requestId,
      items: [{ id: 1, bytes: new Uint8Array([1]) }],
    });

    const after = store.getState();
    // A bare notify() would leave `after === before`; the overlay's memo would
    // never recompute and edges would stay hidden until an unrelated change.
    expect(after).not.toBe(before);
    expect(after.brepVersion).toBe(before.brepVersion + 1);
    expect(notifications).toBe(1);
  });

  it("a no-op import (nothing newly installed) does not churn the snapshot", () => {
    const weaver = FakeWorker.instances[1];
    // First import installs the B-Rep.
    const lastReq = weaver.posted
      .filter(
        (m): m is { type: string; requestId: number } =>
          typeof m === "object" &&
          m !== null &&
          (m as { type?: string }).type === "weave",
      )
      .pop()!;
    weaver.emit({
      type: "breps",
      requestId: lastReq.requestId,
      items: [{ id: 1, bytes: new Uint8Array([1]) }],
    });
    const afterFirst = store.getState();

    // A second delivery for the same request installs nothing new.
    let notifications = 0;
    store.subscribe(() => {
      notifications += 1;
    });
    weaver.emit({
      type: "breps",
      requestId: lastReq.requestId,
      items: [{ id: 1, bytes: new Uint8Array([1]) }],
    });
    expect(store.getState()).toBe(afterFirst);
    expect(notifications).toBe(0);
  });
});
