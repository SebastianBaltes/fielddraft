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
  removeSketchSubCallAt as reduceRemoveAt,
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
  /** K6b.3b: feature id of the sketch being edited, or `null`
   *  when the session is a fresh draft. When set, Finish goes
   *  through `replaceSketch` instead of `addSketch` so the
   *  outliner row keeps its id. */
  editingFeatureId: number | null;
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
   * K6b.3b: reopen a previously committed sketch for editing.
   * Pulls the sketch's sub_calls back out of the kernel and
   * seeds the session with them, remembering the feature id
   * so Finish takes the `replaceSketch` path instead of
   * creating a new outliner row.
   */
  editSketch(featureId: number): void {
    if (this.state.sketchSession.active) return;
    let subCalls: SketchSessionState["subCalls"] = [];
    try {
      const raw = this.kernel.getSketchSubCalls(featureId);
      subCalls = raw.map((c) => ({ type: c.type, params: c.params }));
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn("fielddraft: editSketch() failed to load sub-calls", err);
      return;
    }
    // K6b.3b uses the same placeholder plane query that
    // `enterSketch` seeds new sessions with, because the real
    // plane lives inside the committed FeatureCall params and
    // we don't round-trip it yet — K6b.3 follow-up adds the
    // face-lock resolution path.
    this.updateSketch(
      reduceEnter(this.state.sketchSession, "qCreatedBy(origin_plane_xy)", {
        initialSubCalls: subCalls,
        editingFeatureId: featureId,
      }),
    );
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
   * K6b.3d: append multiple sub-spine calls atomically. Used by
   * the 2D canvas commit path when a line drag needs to land
   * both the `sketch_line` call and a constraint call in a
   * single step so the session snapshot is always consistent.
   */
  pushSketchSubCalls(
    calls: ReadonlyArray<{ type: string; params: Record<string, unknown> }>,
  ): void {
    if (!this.state.sketchSession.active) return;
    if (calls.length === 0) return;
    this.state = {
      ...this.state,
      sketchSession: {
        ...this.state.sketchSession,
        subCalls: [...this.state.sketchSession.subCalls, ...calls],
      },
    };
    this.notify();
  }

  /**
   * K6b.3d: wholesale replace the session's sub-spine. After a
   * commit-time solver pass rewrites the coordinates of every
   * call, the canvas overlay calls this to swap the old
   * sub_calls for the solved ones without going through a
   * delta diff.
   */
  replaceSketchSubCalls(
    calls: ReadonlyArray<{ type: string; params: Record<string, unknown> }>,
  ): void {
    if (!this.state.sketchSession.active) return;
    this.state = {
      ...this.state,
      sketchSession: {
        ...this.state.sketchSession,
        subCalls: [...calls],
      },
    };
    this.notify();
  }

  /**
   * K6b.3g: drop a single sub-spine entry by index. Downstream
   * constraint calls whose references would dangle after the
   * removal are rewritten by the
   * [`removeSketchSubCallAt`](./sketchSession) reducer — ones
   * with refs into the removed range are also dropped, ones
   * above the range are shifted down by the range size so
   * they still point at the same post-remove entity.
   */
  removeSketchSubCall(index: number): void {
    this.updateSketch(reduceRemoveAt(this.state.sketchSession, index));
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
  // ── K7.5: editable feature params ─────────────────────────────

  /** Read the current wire-form params for a feature. */
  getFeatureParams(id: number): Record<string, unknown> {
    return this.kernel.getFeatureCallParams(id);
  }

  /** K8.8: duplicate a feature call. Returns the new feature
   *  id, or `null` when the kernel rejected the call. */
  duplicateFeature(id: number): number | null {
    let newId: number;
    try {
      newId = this.kernel.duplicateFeature(id);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: duplicateFeature failed", err);
      return null;
    }
    this.commit(new Set([newId]), newId);
    return newId;
  }

  /** K8.7: flip a feature's suppressed flag. Returns `true` on
   *  success, `false` when the toggle rolls back (e.g. an
   *  upstream dependency broke and regeneration failed). */
  toggleFeatureSuppressed(id: number): boolean {
    const current = this.state.features.find((f) => f.id === id);
    if (!current) return false;
    const next = !current.suppressed;
    try {
      this.kernel.setFeatureSuppressed(id, next);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: setFeatureSuppressed failed", err);
      return false;
    }
    this.commit();
    return true;
  }

  /** K8.6: move a feature one slot up in the outliner. No-op
   *  when the feature is already at the top or doesn't exist.
   *  Returns `true` on success. Reorder is a kernel mutation,
   *  so it goes through snapshot/commit and participates in
   *  undo/redo. */
  moveFeatureUp(id: number): boolean {
    const idx = this.state.features.findIndex((f) => f.id === id);
    if (idx <= 0) return false;
    if (!this.kernel.reorderFeature(id, idx - 1)) return false;
    this.commit();
    return true;
  }

  /** K8.6: twin of [`moveFeatureUp`] — move a feature one slot
   *  down. No-op at the bottom. */
  moveFeatureDown(id: number): boolean {
    const idx = this.state.features.findIndex((f) => f.id === id);
    if (idx < 0 || idx >= this.state.features.length - 1) return false;
    if (!this.kernel.reorderFeature(id, idx + 1)) return false;
    this.commit();
    return true;
  }

  /** K8.5: rename a feature by patching the `name` key in its
   *  wire-form params and commiting. Works for every feature
   *  kind — primitives and Extrude/Revolve honour `"name"` via
   *  `optional_display_name`, patterns and sketches pick it up
   *  via `synth_name_for_call`'s params short-circuit. Returns
   *  `true` on success, `false` if the kernel rejected the
   *  update. */
  renameFeature(id: number, newName: string): boolean {
    let current: Record<string, unknown>;
    try {
      current = this.kernel.getFeatureCallParams(id);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: getFeatureCallParams failed", err);
      return false;
    }
    return this.updateFeatureParams(id, { ...current, name: newName });
  }

  /** Replace a feature's wire-form params and commit. Used
   *  by the Outliner FeatureDialog's onChange to round-trip
   *  edits back into the kernel. */
  updateFeatureParams(id: number, params: Record<string, unknown>): boolean {
    try {
      this.kernel.replaceFeatureCallParams(id, params);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: replaceFeatureCallParams failed", err);
      return false;
    }
    this.commit();
    return true;
  }

  // ── K7.4: extrude convenience ─────────────────────────────────

  /** Find the most recently committed sketch feature (by id)
   *  or `null` if there isn't one. Used by the toolbar Extrude
   *  button to pick a default profile. */
  latestSketchId(): number | null {
    const sketches = this.state.features.filter((f) => f.kind === "sketch");
    if (sketches.length === 0) return null;
    // Ids are monotonic within a document — the highest id is
    // the most recently added.
    return sketches.reduce((acc, f) => (f.id > acc ? f.id : acc), sketches[0].id);
  }

  /** K8.1: circular-pattern the most recent spine feature
   *  around the world Y axis at (0, 0, 0) with 6 instances
   *  over a full 360° sweep. */
  addCircularPatternOfLatest(): number | null {
    if (this.state.features.length === 0) return null;
    const source = this.state.features[this.state.features.length - 1];
    let id: number;
    try {
      id = this.kernel.addCircularPattern(source.id, 0, 1, 0, 0, 0, 0, 6, 360);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: addCircularPattern failed", err);
      return null;
    }
    this.commit(new Set([id]), id);
    return id;
  }

  /** K8.0: pattern the most recent spine feature along +X
   *  with default count 3 and spacing 60 mm. MVP — a future
   *  slice grows a pattern-parameter dialog so the user can
   *  pick direction / count / spacing per call. */
  addLinearPatternOfLatest(): number | null {
    if (this.state.features.length === 0) return null;
    // Pick the *last* spine entry as source. Same heuristic
    // as addExtrudeFromLatestSketch but doesn't filter by
    // kind — box/sphere/cylinder/extrude all pattern.
    const source = this.state.features[this.state.features.length - 1];
    let id: number;
    try {
      id = this.kernel.addLinearPattern(source.id, 1, 0, 0, 3, 60);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: addLinearPattern failed", err);
      return null;
    }
    this.commit(new Set([id]), id);
    return id;
  }

  /** K8.2: mirror the most recent spine feature through the
   *  world YZ plane (normal `+X`, point at origin). MVP — a
   *  future slice pops a dialog so the user can pick the
   *  mirror plane. */
  addMirrorOfLatest(): number | null {
    if (this.state.features.length === 0) return null;
    const source = this.state.features[this.state.features.length - 1];
    let id: number;
    try {
      id = this.kernel.addMirror(source.id, 1, 0, 0, 0, 0, 0);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: addMirror failed", err);
      return null;
    }
    this.commit(new Set([id]), id);
    return id;
  }

  /** K8.4: revolve the latest committed sketch around the world
   *  Y axis at the origin, using the store's current `nextOp`.
   *  Returns the new feature id, or `null` when there's no
   *  sketch to consume. */
  addRevolveFromLatestSketch(): number | null {
    const sketchId = this.latestSketchId();
    if (sketchId === null) return null;
    let id: number;
    try {
      id = this.kernel.addRevolve(sketchId, this.state.nextOp, 0, 0, 0);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: addRevolve failed", err);
      return null;
    }
    this.commit(new Set([id]), id);
    return id;
  }

  /** Extrude the latest committed sketch with the given depth
   *  and the store's current `nextOp`. Returns the new feature
   *  id, or `null` when there's no sketch to consume. */
  addExtrudeFromLatestSketch(depthMm: number): number | null {
    const sketchId = this.latestSketchId();
    if (sketchId === null) return null;
    let id: number;
    try {
      id = this.kernel.addExtrude(sketchId, depthMm, this.state.nextOp, 0);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: addExtrude failed", err);
      return null;
    }
    this.commit(new Set([id]), id);
    return id;
  }

  // ── K7.2.1: STL export ────────────────────────────────────────

  /** Serialise the current scene as ASCII STL. Mostly useful
   *  for debug / diffing — the Download button uses the binary
   *  path below. */
  exportStlAscii(): string {
    return this.kernel.exportStlAscii();
  }

  /** Serialise the current scene as binary STL bytes. */
  exportStlBinary(): Uint8Array {
    return this.kernel.exportStlBinary();
  }

  /** Fire a browser download of the current scene as a binary
   *  STL file. No-op outside the browser (the store is also
   *  used in Vitest where `document`/`URL` may not exist). */
  downloadStl(filename = "fielddraft.stl"): void {
    if (
      typeof window === "undefined" ||
      typeof document === "undefined" ||
      typeof URL === "undefined" ||
      typeof URL.createObjectURL !== "function"
    ) {
      return;
    }
    const bytes = this.kernel.exportStlBinary();
    // Copy the view into a dedicated backing buffer so the
    // Blob constructor sees a plain ArrayBuffer — passing a
    // Uint8Array that aliases wasm memory can hand the Blob
    // a buffer that gets invalidated by a subsequent kernel
    // mutation.
    const copy = new Uint8Array(bytes.length);
    copy.set(bytes);
    const blob = new Blob([copy], { type: "model/stl" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = filename;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    URL.revokeObjectURL(url);
  }

  commitSketch(): number | null {
    const session = this.state.sketchSession;
    if (!session.active || session.planeQuery === null) {
      this.exitSketch();
      return null;
    }
    // Edit reentry path: if a sketch feature id is attached to
    // the session we always replace in place — even when the
    // sub-spine is empty — so the user can intentionally clear
    // the sketch by opening it and hitting Finish with no
    // further edits.
    if (session.editingFeatureId !== null) {
      try {
        this.kernel.replaceSketch(
          session.editingFeatureId,
          session.planeQuery,
          session.subCalls.map((c) => ({ type: c.type, params: c.params })),
        );
      } catch (err) {
        // eslint-disable-next-line no-console
        console.error("fielddraft: replaceSketch failed", err);
        return null;
      }
      const editedId = session.editingFeatureId;
      this.updateSketch(reduceExit(this.state.sketchSession));
      this.commit(new Set([editedId]), editedId);
      return editedId;
    }
    // Fresh sketch draft: drop on cancel / finish-with-empty.
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
