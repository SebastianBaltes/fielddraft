/**
 * Minimal in-memory store wrapping the wasm kernel, with autosave + a
 * tiny pub-sub for React re-renders.
 *
 * The kernel is the ground truth. The store's job is to:
 *  - own the singleton kernel handle,
 *  - bump a `version` counter on every mutation so React subscribers
 *    re-render,
 *  - hold ephemeral UI state (current selection),
 *  - schedule debounced `localStorage` autosaves on every mutation.
 */

import { useEffect, useState, useSyncExternalStore } from "react";
import {
  BoolOpTag,
  Kernel,
  type BoolOpTagValue,
  type FeatureSummary,
  type PrimitiveKindName,
} from "./kernel";
import { createAutoSave, loadPersisted, type AutoSaveHandle } from "./persistence";
import type {
  MesherBodiesResponse,
  MesherBodyPayload,
  MesherRebuildRequest,
} from "./mesher.worker";
import {
  INACTIVE_SKETCH_SESSION as _INACTIVE,
  applySketchToolDraft as reduceApply,
  enterSketchSession as reduceEnter,
  exitSketchSession as reduceExit,
  selectSketchTool as reduceSelect,
  setSketchToolDraft as reduceSetDraft,
} from "./sketchSession";

/**
 * One mesh-ready body as cached by the store after a worker response.
 * The TypedArrays are owned by the main thread (they were transferred
 * from the worker), so they're safe to retain across future kernel
 * mutations — unlike the WASM-aliased views you'd get from the
 * kernel's own `body*View` accessors.
 */
export type MeshedBody = MesherBodyPayload;

/**
 * K6b.2: ephemeral per-sketch-session state. A sketch session is the
 * period between the user clicking the "Sketch" toolbar button (and
 * locking a plane) and clicking Finish/Cancel. While the session is
 * active the UI switches into sketch mode: the top toolbar shows
 * Finish/Cancel + a sketch-tool row instead of primitive-add buttons,
 * the outliner shows the active sketch tool's FeatureDialog, and the
 * viewport frames the locked plane.
 *
 * K6b.2 stores the session's draft geometry as a local `sub_calls`
 * buffer — nothing is committed to the kernel yet. Finish in K6b.2
 * simply drops the draft and exits sketch mode; K6b.3 will wire the
 * commit path that assembles a `sketch` FeatureCall from the draft
 * and regenerates the document.
 */
export interface SketchSessionState {
  /** True while the user is inside sketch mode. */
  active: boolean;
  /** The query string locking the sketch plane. K6b.2 uses a
   *  placeholder; K6b.3 will populate it from a face-click pick. */
  planeQuery: string | null;
  /** Currently selected sketch-tool type name, or `null` when the
   *  user has entered sketch mode but not yet picked a tool. */
  selectedTool: string | null;
  /** Controlled-input working copy for the selected tool's
   *  FeatureDialog. Resets when the tool changes. */
  toolDraft: Record<string, unknown>;
  /** Committed sub-spine calls for this session. K6b.2 appends to
   *  this in-memory on Apply and drops it on Cancel/Finish; K6b.3
   *  will flush it through the kernel. */
  subCalls: ReadonlyArray<{
    type: string;
    params: Record<string, unknown>;
  }>;
}

export interface StoreState {
  features: FeatureSummary[];
  selection: ReadonlySet<number>;
  /** Anchor for shift-click range selection — id of the last single-clicked
   *  feature. `null` while nothing has been clicked yet. */
  selectionAnchor: number | null;
  canUndo: boolean;
  canRedo: boolean;
  /** Default op applied when adding the next primitive. */
  nextOp: BoolOpTagValue;
  /** Latest per-body mesh cache streamed in from the mesher worker.
   *  A fresh array object is written on every worker response so
   *  React's useSyncExternalStore detects the change and re-renders
   *  the viewport — even though the contents only care about the
   *  delta per body id. */
  meshedBodies: readonly MeshedBody[];
  /** K6b.2: active sketch session, or inactive-default. */
  sketchSession: SketchSessionState;
}

const INACTIVE_SKETCH_SESSION: SketchSessionState = _INACTIVE;

export class KernelStore {
  readonly kernel: Kernel;
  private readonly autosave: AutoSaveHandle;
  private listeners = new Set<() => void>();
  private state: StoreState;
  private worker: Worker | null = null;
  private workerRequestCounter = 0;
  /** Latest requestId handed to the worker. Responses with a lower
   *  id are stale (the user mutated faster than the worker could
   *  keep up) and get dropped on arrival. */
  private latestWorkerRequestId = 0;
  /** True while a worker request is in flight; set back to false on
   *  response. Used to coalesce rapid-fire mutations into a single
   *  worker job: we always post the *most recent* document, never a
   *  stale one, by tracking whether another rebuild is queued while
   *  the current one is still meshing. */
  private workerInflight = false;
  private workerRebuildQueued = false;
  /** Fallback mode: when the Worker constructor throws (e.g. the
   *  bundler didn't emit the worker chunk), we keep sync meshing on
   *  the main-thread kernel and drive the meshedBodies cache from
   *  its own `body*` views. Loses the off-thread benefit but keeps
   *  the app rendering. */
  private workerAvailable = false;

  constructor(kernel: Kernel, autosave: AutoSaveHandle) {
    this.kernel = kernel;
    this.autosave = autosave;
    this.state = {
      features: kernel.listFeatures(),
      selection: new Set(),
      selectionAnchor: null,
      canUndo: kernel.canUndo(),
      canRedo: kernel.canRedo(),
      nextOp: BoolOpTag.Union,
      meshedBodies: [],
      sketchSession: INACTIVE_SKETCH_SESSION,
    };

    // Bring up the mesher worker. On success, disable sync meshing
    // on the main-thread kernel so every mutation goes straight
    // through to document + partition without the DC pass. On
    // failure (Vite didn't emit the worker, CSP blocked it,
    // whatever), fall back to sync meshing and read body buffers
    // from the main-thread kernel directly — functionally correct,
    // just blocks the UI on every mesh rebuild.
    try {
      this.worker = new Worker(
        new URL("./mesher.worker.ts", import.meta.url),
        { type: "module" },
      );
      this.worker.addEventListener("message", (event) => {
        this.handleWorkerMessage(event);
      });
      this.workerAvailable = true;
      this.kernel.setSyncMeshing(false);
      // Kick off the first worker rebuild so the initial document
      // (possibly rehydrated from localStorage) becomes visible.
      this.scheduleWorkerRebuild();
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn(
        "fielddraft: could not start mesher worker, falling back to sync meshing on the main thread",
        err,
      );
      this.workerAvailable = false;
      this.state = {
        ...this.state,
        meshedBodies: this.snapshotMainThreadBodies(),
      };
    }
  }

  // ── pub-sub ───────────────────────────────────────────────────────

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  getState = (): StoreState => this.state;

  primitivesView(): Float32Array {
    return this.kernel.primitivesView();
  }

  private commit(nextSelection?: ReadonlySet<number>, nextAnchor?: number | null): void {
    this.state = {
      features: this.kernel.listFeatures(),
      selection: nextSelection ?? this.state.selection,
      selectionAnchor: nextAnchor !== undefined ? nextAnchor : this.state.selectionAnchor,
      canUndo: this.kernel.canUndo(),
      canRedo: this.kernel.canRedo(),
      nextOp: this.state.nextOp,
      meshedBodies: this.state.meshedBodies,
      sketchSession: this.state.sketchSession,
    };
    // Drop selection entries pointing at deleted features.
    if (this.state.selection.size > 0) {
      const live = new Set(this.state.features.map((f) => f.id));
      const filtered = new Set<number>();
      for (const id of this.state.selection) {
        if (live.has(id)) filtered.add(id);
      }
      if (filtered.size !== this.state.selection.size) {
        this.state = { ...this.state, selection: filtered };
      }
    }
    // Anchor must point at a still-living feature, or be cleared.
    if (
      this.state.selectionAnchor !== null &&
      !this.state.features.some((f) => f.id === this.state.selectionAnchor)
    ) {
      this.state = { ...this.state, selectionAnchor: null };
    }
    this.syncActiveBody();
    this.autosave.schedule(this.kernel.toJson());
    this.refreshMeshCache();
    for (const listener of this.listeners) listener();
  }

  /**
   * Pull the latest meshes into `state.meshedBodies`. In worker mode
   * this posts a rebuild request and waits for the response
   * asynchronously; in fallback mode (no worker) it snapshots the
   * main-thread kernel's already-synced body buffers right now.
   * Callers are responsible for notifying listeners after — the
   * worker path notifies from inside `handleWorkerMessage`, the
   * fallback path mutates state inline so the caller's post-call
   * listener sweep picks it up.
   */
  private refreshMeshCache(): void {
    if (this.workerAvailable) {
      this.scheduleWorkerRebuild();
      return;
    }
    this.state = {
      ...this.state,
      meshedBodies: this.snapshotMainThreadBodies(),
    };
  }

  /**
   * Hand off the current document to the mesher worker. Rapid-fire
   * mutations coalesce: while a rebuild is in flight, subsequent
   * calls just flip the `workerRebuildQueued` flag; when the
   * current rebuild's response lands, we post one new rebuild with
   * the *latest* document instead of a backlog of stale ones.
   */
  private scheduleWorkerRebuild(): void {
    if (!this.worker || !this.workerAvailable) return;
    if (this.workerInflight) {
      this.workerRebuildQueued = true;
      return;
    }
    this.postWorkerRebuild();
  }

  private postWorkerRebuild(): void {
    if (!this.worker) return;
    this.workerRequestCounter += 1;
    this.latestWorkerRequestId = this.workerRequestCounter;
    this.workerInflight = true;
    this.workerRebuildQueued = false;
    const request: MesherRebuildRequest = {
      type: "rebuild",
      requestId: this.workerRequestCounter,
      documentJson: this.kernel.toJson(),
      activeMeshResolution: this.kernel.meshResolution(),
      contextMeshResolution: this.kernel.contextMeshResolution(),
      activeBodyId: this.kernel.activeBodyId(),
    };
    this.worker.postMessage(request);
  }

  private handleWorkerMessage(event: MessageEvent): void {
    const data = event.data as MesherBodiesResponse | undefined;
    if (!data || data.type !== "bodies") return;
    this.workerInflight = false;
    if (data.requestId < this.latestWorkerRequestId) {
      // Stale response — a newer rebuild is already in flight or
      // queued. Drop it; we'll render the fresh one shortly.
      if (this.workerRebuildQueued) {
        this.postWorkerRebuild();
      }
      return;
    }
    this.state = {
      ...this.state,
      meshedBodies: data.bodies,
    };
    for (const listener of this.listeners) listener();
    if (this.workerRebuildQueued) {
      this.postWorkerRebuild();
    }
  }

  /**
   * Fallback path for when the worker couldn't start: read per-body
   * meshes from the main-thread kernel's sync-meshing output
   * directly, copying each TypedArray out of WASM linear memory so
   * the store can retain it across future mutations.
   */
  private snapshotMainThreadBodies(): MeshedBody[] {
    const count = this.kernel.bodyCount();
    const out: MeshedBody[] = [];
    for (let i = 0; i < count; i++) {
      out.push({
        id: this.kernel.bodyId(i),
        version: this.kernel.bodyVersion(i),
        positions: this.kernel.bodyPositionsView(i).slice(),
        normals: this.kernel.bodyNormalsView(i).slice(),
        indices: this.kernel.bodyIndicesView(i).slice(),
        classifications: this.kernel.bodyClassificationsView(i).slice(),
        contributors: this.kernel.bodyContributorsView(i).slice(),
      });
    }
    return out;
  }

  /**
   * K3 part 4 LOD hook: push the selection anchor's body to the
   * kernel as the active body, or clear the active body when the
   * anchor is gone. Called from `commit()` after every mutation and
   * from every selection helper. The kernel's
   * `setActiveBodyByFeature` takes care of looking up which body
   * owns the feature and triggers a re-mesh only when the active
   * body actually changes.
   */
  private syncActiveBody(): void {
    const anchor = this.state.selectionAnchor;
    if (anchor === null) {
      if (this.kernel.activeBodyId() !== 0) {
        this.kernel.clearActiveBodyId();
      }
      return;
    }
    this.kernel.setActiveBodyByFeature(anchor);
  }

  // ── mutations ─────────────────────────────────────────────────────

  setNextOp(op: BoolOpTagValue): void {
    if (this.state.nextOp === op) return;
    this.state = { ...this.state, nextOp: op };
    for (const listener of this.listeners) listener();
  }

  addPrimitive(kind: PrimitiveKindName, op: BoolOpTagValue = this.state.nextOp): number {
    // Default sizes are tuned to be clearly readable inside the
    // ~600 mm build volume, and the per-add stagger pulls each new
    // feature off the previous one so freshly clicked primitives
    // aren't hidden inside their predecessors.
    const offset: [number, number, number] = [
      this.state.features.length * 80,
      0,
      0,
    ];
    let id: number;
    switch (kind) {
      case "box":
        id = this.kernel.addBox(op, offset, [50, 50, 50]);
        break;
      case "sphere":
        id = this.kernel.addSphere(op, offset, 50);
        break;
      case "cylinder":
        id = this.kernel.addCylinder(op, offset, 40, 60);
        break;
    }
    this.commit(new Set([id]), id);
    return id;
  }

  /** Move every selected feature by `(dx, dy, dz)`. */
  nudgeSelected(dx: number, dy: number, dz: number): boolean {
    if (this.state.selection.size === 0) return false;
    let any = false;
    for (const id of this.state.selection) {
      if (this.kernel.nudgeFeature(id, dx, dy, dz)) any = true;
    }
    if (any) this.commit();
    return any;
  }

  removeSelected(): boolean {
    if (this.state.selection.size === 0) return false;
    let removed = false;
    for (const id of this.state.selection) {
      if (this.kernel.removeFeature(id)) removed = true;
    }
    if (removed) this.commit(new Set(), null);
    return removed;
  }

  removeFeature(id: number): boolean {
    if (!this.kernel.removeFeature(id)) return false;
    const next = new Set(this.state.selection);
    next.delete(id);
    const nextAnchor = this.state.selectionAnchor === id ? null : this.state.selectionAnchor;
    this.commit(next, nextAnchor);
    return true;
  }

  setFeatureOp(id: number, op: BoolOpTagValue): boolean {
    if (!this.kernel.setFeatureOp(id, op)) return false;
    this.commit();
    return true;
  }

  undo(): boolean {
    if (!this.kernel.undo()) return false;
    this.commit();
    return true;
  }

  redo(): boolean {
    if (!this.kernel.redo()) return false;
    this.commit();
    return true;
  }

  // ── selection ─────────────────────────────────────────────────────

  selectOnly(id: number): void {
    this.state = {
      ...this.state,
      selection: new Set([id]),
      selectionAnchor: id,
    };
    this.syncActiveBody();
    this.refreshMeshCache();
    for (const listener of this.listeners) listener();
  }

  toggleSelect(id: number): void {
    const next = new Set(this.state.selection);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    this.state = {
      ...this.state,
      selection: next,
      selectionAnchor: id,
    };
    this.syncActiveBody();
    this.refreshMeshCache();
    for (const listener of this.listeners) listener();
  }

  /**
   * Shift+Click range selection: select every feature between the
   * current anchor (inclusive) and the clicked id (inclusive). If no
   * anchor exists yet this falls back to a single selection.
   */
  selectRangeTo(id: number): void {
    const features = this.state.features;
    const targetIdx = features.findIndex((f) => f.id === id);
    if (targetIdx === -1) return;

    const anchor = this.state.selectionAnchor;
    if (anchor === null) {
      this.selectOnly(id);
      return;
    }
    const anchorIdx = features.findIndex((f) => f.id === anchor);
    if (anchorIdx === -1) {
      // Anchor stale; reset.
      this.selectOnly(id);
      return;
    }
    const [lo, hi] = anchorIdx <= targetIdx ? [anchorIdx, targetIdx] : [targetIdx, anchorIdx];
    const next = new Set<number>();
    for (let i = lo; i <= hi; i++) next.add(features[i].id);
    this.state = {
      ...this.state,
      selection: next,
      // Anchor stays put — that's the standard finder/Onshape behaviour.
      selectionAnchor: anchor,
    };
    this.syncActiveBody();
    this.refreshMeshCache();
    for (const listener of this.listeners) listener();
  }

  clearSelection(): void {
    if (this.state.selection.size === 0 && this.state.selectionAnchor === null) return;
    this.state = { ...this.state, selection: new Set(), selectionAnchor: null };
    this.syncActiveBody();
    this.refreshMeshCache();
    for (const listener of this.listeners) listener();
  }

  // ── K6b.2: sketch session ────────────────────────────────────────

  private notify(): void {
    for (const listener of this.listeners) listener();
  }

  /**
   * Enter sketch mode against a given plane query. K6b.2 accepts
   * a plane query as a plain string (placeholder until K6b.3
   * wires the face-pick path); calling this while a session is
   * already active is a no-op so the viewport can't stack
   * sessions on top of each other.
   */
  private updateSketch(
    next: SketchSessionState,
  ): void {
    if (next === this.state.sketchSession) return;
    this.state = { ...this.state, sketchSession: next };
    this.notify();
  }

  enterSketch(planeQuery: string): void {
    this.updateSketch(reduceEnter(this.state.sketchSession, planeQuery));
  }

  /**
   * Exit sketch mode, discarding the draft. K6b.2 does not yet
   * persist sub_calls back into the kernel — that's the commit
   * path K6b.3 will implement. Both Finish and Cancel route
   * here for now.
   */
  exitSketch(): void {
    this.updateSketch(reduceExit(this.state.sketchSession));
  }

  /**
   * Select a sketch tool by type name. Passing `null` clears
   * the tool selection. Delegates shape changes to the pure
   * reducer in `sketchSession.ts`.
   */
  selectSketchTool(
    typeName: string | null,
    initialDraft: Record<string, unknown> = {},
  ): void {
    this.updateSketch(
      reduceSelect(this.state.sketchSession, typeName, initialDraft),
    );
  }

  /** Update the active sketch tool's FeatureDialog working copy. */
  setSketchToolDraft(draft: Record<string, unknown>): void {
    this.updateSketch(reduceSetDraft(this.state.sketchSession, draft));
  }

  /**
   * K6b.2: append the current tool draft to the session's
   * in-memory sub_calls buffer. No kernel interaction — K6b.3
   * replaces this with a regeneration path that rebuilds the
   * full `sketch` FeatureCall after every apply.
   */
  applySketchToolDraft(): void {
    this.updateSketch(reduceApply(this.state.sketchSession));
  }

  /**
   * K6b.3a: commit the current sketch session to the kernel as
   * a real `sketch` FeatureCall. Called from the "Finish"
   * button. Pushes the session's accumulated sub_calls through
   * `kernel.addSketch`, lets the kernel regenerate, then exits
   * sketch mode. If the session has no sub_calls yet, or no
   * plane is set, the session is simply dropped — same as a
   * cancel.
   */
  commitSketch(): number | null {
    const session = this.state.sketchSession;
    if (!session.active || session.planeQuery === null) {
      this.exitSketch();
      return null;
    }
    if (session.subCalls.length === 0) {
      this.exitSketch();
      return null;
    }
    let id: number | null = null;
    try {
      id = this.kernel.addSketch(
        session.planeQuery,
        session.subCalls.map((c) => ({ type: c.type, params: c.params })),
      );
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: addSketch failed", err);
      return null;
    }
    // Commit goes through the standard store `commit` path so
    // features, selection, meshes and autosave all refresh.
    this.updateSketch(reduceExit(this.state.sketchSession));
    this.commit(id !== null ? new Set([id]) : undefined, id);
    return id;
  }
}

let singleton: KernelStore | null = null;
let storePromise: Promise<KernelStore> | null = null;

export async function getStore(): Promise<KernelStore> {
  if (singleton) return singleton;
  if (storePromise) return storePromise;

  storePromise = (async () => {
    const kernel = await Kernel.create();

    const persisted = loadPersisted();
    if (persisted) {
      try {
        kernel.loadJson(persisted);
      } catch (err) {
        console.warn("fielddraft: could not rehydrate persisted document", err);
      }
    }

    const autosave = createAutoSave();
    const store = new KernelStore(kernel, autosave);

    if (typeof window !== "undefined") {
      window.addEventListener("beforeunload", () => autosave.flush());
      // Dev-mode test hook: expose the kernel + store handles on
      // window so Playwright specs can peek non-UI state (current
      // sub-spine, diagnostics, raw feature calls) without
      // plumbing a wire API. Kept dev-only to avoid shipping a
      // public API surface we'd have to support in production.
      if (import.meta.env.DEV) {
        (
          window as unknown as {
            __fdKernel?: Kernel;
            __fdStore?: KernelStore;
          }
        ).__fdKernel = kernel;
        (
          window as unknown as {
            __fdKernel?: Kernel;
            __fdStore?: KernelStore;
          }
        ).__fdStore = store;
      }
    }

    singleton = store;
    return store;
  })();

  return storePromise;
}

/** React hook that boots the store and re-renders on every mutation. */
export function useKernelStore(): {
  store: KernelStore | null;
  state: StoreState | null;
  error: Error | null;
} {
  const [store, setStore] = useState<KernelStore | null>(null);
  const [error, setError] = useState<Error | null>(null);

  useEffect(() => {
    let cancelled = false;
    getStore()
      .then((s) => {
        if (!cancelled) setStore(s);
      })
      .catch((e) => {
        if (!cancelled) setError(e instanceof Error ? e : new Error(String(e)));
      });
    return () => {
      cancelled = true;
    };
  }, []);

  // Subscribe to the live state when the store is ready.
  const state = useSyncExternalStore(
    (onChange) => (store ? store.subscribe(onChange) : () => {}),
    () => (store ? store.getState() : null),
    () => (store ? store.getState() : null),
  );

  return { store, state, error };
}
