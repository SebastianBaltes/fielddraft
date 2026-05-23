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
import {
  INACTIVE_ASSEMBLY,
  allocateAssemblyId,
  IDENTITY_TRANSFORM,
  solveAssembly,
  solveIK,
  type AssemblyPartInstance,
  type AssemblyMeshedBody,
  type Mate,
  type MateType,
  type IKTarget,
} from "./assembly";
import { executeScript } from "./fieldscript";
import { import3mf, export3mfRoundtrip, extractModelXml, type ThreeMfStash } from "./threemf";

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
  undoDepth: number;
  canRedo: boolean;
  redoDepth: number;
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
  /** K8.10: document-level scalar variables, sorted by name
   *  (the kernel's BTreeMap ordering). K8.11 will teach
   *  Length fields to reference these by name. */
  variables: readonly { name: string; value: number }[];
  /** K9.0: imported STL reference mesh. Ephemeral — not
   *  persisted to the document. `null` when no import is
   *  loaded. */
  referenceMesh: {
    positions: Float32Array;
    normals: Float32Array;
    indices: Uint32Array;
  } | null;
  /** K9.4: measure mode active — clicks place measure points
   *  instead of selecting features. */
  measureMode: boolean;
  /** K10.6: fillet mode — clicks on edges create fillet_edge features. */
  filletMode: boolean;
  /** M12: sync state machine for server-backed projects.
   *  "clean" = in sync with server, "dirty" = local changes
   *  pending, "syncing" = upload in flight, "error" = last
   *  sync failed. */
  syncState: "clean" | "dirty" | "syncing" | "error" | "offline";
  /** M12: server-side project ID, null in guest/offline mode. */
  projectId: string | null;
  /** K9.1: overall scene extents in mm, refreshed on commit. */
  sceneSize: [number, number, number];
  /** K7.22 UX-fix (2): display-only feature folders. The feature
   *  spine stays linear top-down; folders are a UI grouping layer
   *  that lets the user collapse clumps of related features.
   *  Persisted to localStorage, never touched by the kernel. */
  folders: readonly FeatureFolder[];
  /** K7.22 UX-fix (5): sketch feature ids that are rendered in
   *  the 3D viewport as line overlays. Defaults to empty: a
   *  freshly-created sketch is invisible in 3D. The Outliner's
   *  per-row eye icon toggles membership. Persisted to
   *  localStorage so the visibility survives reloads. */
  visibleSketches: readonly number[];
  /** K7.22 UX-fix (7): latest user-visible transient error message
   *  from a kernel mutation (e.g. "sketch has no closed regions to
   *  extrude"). Cleared automatically on the next successful
   *  mutation. The App renders it as a toast in the topbar. */
  transientError: string | null;
  /** K7.21e: face-pick mode — the next viewport click selects a
   *  face to use as the sketch plane instead of defaulting to XY. */
  facePickMode: boolean;
  /** K9: mate connector placement mode — the next viewport click
   *  places a mate connector at the surface hit point. */
  mateMode: boolean;
  /** P1: assembly state — manages multi-part scene graph. */
  assembly: import("./assembly").AssemblyState;
  /** P2: IK mode — clicking in viewport sets the IK target. */
  ikMode: boolean;
  /** K7.21h: transform gizmo mode. */
  gizmoMode: "translate" | "rotate" | "scale";
  /** 3MF roundtrip: stashed slicer metadata from import, used on re-export. */
  threemfStash: ThreeMfStash | null;
  /** Loading overlay: non-null while an import or heavy operation is
   *  in progress. The `phase` string is shown to the user. */
  loading: { phase: string } | null;
}

/** K7.22 UX-fix (2): one folder entry in the outliner grouping
 *  layer. Folders are stable across reorder: `memberIds` is a
 *  set of feature ids, and features that disappear from the
 *  document (remove/undo) are filtered out lazily at render time
 *  rather than eagerly pruned. */
export interface FeatureFolder {
  id: string;
  name: string;
  memberIds: readonly number[];
  collapsed: boolean;
}

const FOLDERS_STORAGE_KEY = "fielddraft.folders";

function loadFoldersFromStorage(): FeatureFolder[] {
  try {
    const raw = localStorage.getItem(FOLDERS_STORAGE_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed
      .filter(
        (f): f is FeatureFolder =>
          typeof f === "object" &&
          f !== null &&
          typeof f.id === "string" &&
          typeof f.name === "string" &&
          Array.isArray(f.memberIds),
      )
      .map((f) => ({
        id: f.id,
        name: f.name,
        memberIds: f.memberIds.filter((n: unknown) => typeof n === "number"),
        collapsed: Boolean((f as { collapsed?: boolean }).collapsed),
      }));
  } catch {
    return [];
  }
}

function persistFoldersToStorage(folders: readonly FeatureFolder[]): void {
  try {
    localStorage.setItem(FOLDERS_STORAGE_KEY, JSON.stringify(folders));
  } catch {
    // localStorage quota exceeded or disabled — silently skip.
  }
}

const VISIBLE_SKETCHES_STORAGE_KEY = "fielddraft.visibleSketches";

function loadVisibleSketchesFromStorage(): number[] {
  try {
    const raw = localStorage.getItem(VISIBLE_SKETCHES_STORAGE_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed.filter((n): n is number => typeof n === "number");
  } catch {
    return [];
  }
}

function persistVisibleSketchesToStorage(ids: readonly number[]): void {
  try {
    localStorage.setItem(VISIBLE_SKETCHES_STORAGE_KEY, JSON.stringify(ids));
  } catch {
    // ignore
  }
}

/** Pull a user-readable message out of a wasm-bindgen-thrown error.
 *  The kernel emits strings like
 *  `"extrude regeneration failed: feature ‘extrude’: parameter
 *  ‘profiles’ invalid: sketch has no closed regions to extrude — …"`
 *  — we strip the machine-friendly prefix and keep the human tail. */
function normalToPlaneQuery(normal: [number, number, number]): string {
  const [nx, ny, nz] = normal;
  const ax = Math.abs(nx), ay = Math.abs(ny), az = Math.abs(nz);
  if (ay >= ax && ay >= az) return "qCreatedBy(origin_plane_xz)";
  if (az >= ax && az >= ay) return "qCreatedBy(origin_plane_xy)";
  return "qCreatedBy(origin_plane_yz)";
}

function formatKernelError(err: unknown, fallback: string): string {
  if (err instanceof Error) {
    const raw = err.message ?? fallback;
    // Trim the most common kernel prefixes to surface the actual
    // cause first. Ordering matters — match the longest prefix
    // first so shorter ones don't shadow.
    const stripped = raw
      .replace(/^.*?regeneration failed:\s*/, "")
      .replace(/^feature\s*[`'"][^`'"]+[`'"]:\s*/, "")
      .replace(/^parameter\s*[`'"][^`'"]+[`'"]\s*invalid:\s*/, "");
    return stripped.length > 0 ? stripped : fallback;
  }
  return typeof err === "string" && err.length > 0 ? err : fallback;
}

const INACTIVE_SKETCH_SESSION: SketchSessionState = _INACTIVE;

export type MeshOverlayData = {
  positions: Float32Array;
  normals: Float32Array;
  indices: Uint32Array;
};

export class KernelStore {
  readonly kernel: Kernel;
  private readonly autosave: AutoSaveHandle;
  private listeners = new Set<() => void>();
  private state: StoreState;

  private meshDataCache = new Map<number, MeshOverlayData>();
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
  private dcMeshingEnabled = true;

  constructor(kernel: Kernel, autosave: AutoSaveHandle) {
    this.kernel = kernel;
    this.autosave = autosave;
    this.state = {
      features: kernel.listFeatures(),
      selection: new Set(),
      selectionAnchor: null,
      canUndo: kernel.canUndo(),
      undoDepth: kernel.undoDepth(),
      canRedo: kernel.canRedo(),
      redoDepth: kernel.redoDepth(),
      nextOp: BoolOpTag.Union,
      meshedBodies: [],
      measureMode: false,
      filletMode: false,
      syncState: "offline",
      projectId: null,
      sketchSession: INACTIVE_SKETCH_SESSION,
      variables: kernel.listVariables(),
      referenceMesh: null,
      sceneSize: [0, 0, 0],
      folders: loadFoldersFromStorage(),
      visibleSketches: loadVisibleSketchesFromStorage(),
      transientError: null,
      facePickMode: false,
      mateMode: false,
      assembly: INACTIVE_ASSEMBLY,
      ikMode: false,
      gizmoMode: "translate" as const,
      threemfStash: null,
      loading: null,
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
        sceneSize: this.kernel.sceneAabb().size as [number, number, number],
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
      undoDepth: this.kernel.undoDepth(),
      canRedo: this.kernel.canRedo(),
      redoDepth: this.kernel.redoDepth(),
      nextOp: this.state.nextOp,
      meshedBodies: this.state.meshedBodies,
      measureMode: this.state.measureMode,
      filletMode: this.state.filletMode,
      syncState: this.state.projectId ? "dirty" : "offline",
      projectId: this.state.projectId,
      sketchSession: this.state.sketchSession,
      variables: this.kernel.listVariables(),
      referenceMesh: this.state.referenceMesh,
      sceneSize: this.kernel.sceneAabb().size as [number, number, number],
      folders: this.state.folders,
      visibleSketches: this.state.visibleSketches,
      transientError: null, // successful commit clears any transient error
      facePickMode: false,
      mateMode: false,
      assembly: this.state.assembly,
      ikMode: false,
      gizmoMode: this.state.gizmoMode,
      threemfStash: this.state.threemfStash,
      loading: this.state.loading,
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
    if (!this.dcMeshingEnabled) return;
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
  /** M13: progressive refinement — after a coarse pass, auto-
   *  schedule a fine pass at full resolution. */
  private pendingRefine = false;

  setDcMeshingEnabled(enabled: boolean): void {
    if (this.dcMeshingEnabled === enabled) return;
    this.dcMeshingEnabled = enabled;
    if (enabled) {
      this.scheduleWorkerRebuild();
    }
  }

  private scheduleWorkerRebuild(): void {
    if (!this.worker || !this.workerAvailable) return;
    if (!this.dcMeshingEnabled) return;
    if (this.workerInflight) {
      this.workerRebuildQueued = true;
      return;
    }
    // M13: start with a fast coarse pass (half resolution),
    // then refine to full resolution on completion.
    this.pendingRefine = true;
    this.postWorkerRebuild(true);
  }

  private postWorkerRebuild(coarse = false): void {
    if (!this.worker) return;
    this.workerRequestCounter += 1;
    this.latestWorkerRequestId = this.workerRequestCounter;
    this.workerInflight = true;
    this.workerRebuildQueued = false;
    const fullRes = this.kernel.meshResolution();
    const coarseRes = Math.max(8, Math.floor(fullRes / 2));
    const request: MesherRebuildRequest = {
      type: "rebuild",
      requestId: this.workerRequestCounter,
      documentJson: this.kernel.toJson(),
      activeMeshResolution: coarse ? coarseRes : fullRes,
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
      this.pendingRefine = false;
      if (this.workerRebuildQueued) {
        this.postWorkerRebuild();
      }
      return;
    }
    this.state = {
      ...this.state,
      meshedBodies: data.bodies,
      sceneSize: this.kernel.sceneAabb().size as [number, number, number],
    };
    for (const listener of this.listeners) listener();
    if (this.workerRebuildQueued) {
      this.pendingRefine = false;
      this.postWorkerRebuild();
    } else if (this.pendingRefine) {
      this.pendingRefine = false;
      this.postWorkerRebuild(false);
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
      let transformDelta: [number, number, number] | null = null;
      if (this.kernel.bodyHasTransformDelta(i)) {
        const d = this.kernel.bodyTransformDelta(i);
        transformDelta = [d[0], d[1], d[2]];
      }
      out.push({
        id: this.kernel.bodyId(i),
        seedId: this.kernel.bodySeedFeatureId(i),
        version: this.kernel.bodyVersion(i),
        positions: this.kernel.bodyPositionsView(i).slice(),
        normals: this.kernel.bodyNormalsView(i).slice(),
        indices: this.kernel.bodyIndicesView(i).slice(),
        classifications: this.kernel.bodyClassificationsView(i).slice(),
        contributors: this.kernel.bodyContributorsView(i).slice(),
        transformDelta,
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

  // ── K9.0: STL reference import ──────────────────────────────

  /** K9.0: parse a binary STL file into positions/normals/indices. */
  private parseStl(buffer: ArrayBuffer): MeshOverlayData | null {
    console.log(`fielddraft: importing STL — ${(buffer.byteLength / 1024).toFixed(0)} KB`);

    const view = new DataView(buffer);
    if (buffer.byteLength < 84) {
      console.error("fielddraft: STL import failed — file too small (< 84 bytes), possibly not a valid STL");
      return null;
    }
    const triCount = view.getUint32(80, true);
    const expected = 84 + triCount * 50;
    if (buffer.byteLength < expected) {
      console.error(
        `fielddraft: STL import failed — header says ${triCount} triangles (needs ${expected} bytes) but file is ${buffer.byteLength} bytes. Possibly ASCII STL (not supported) or corrupt file.`,
      );
      return null;
    }

    console.log(`fielddraft: parsing ${triCount} triangles…`);
    const positions = new Float32Array(triCount * 9);
    const normals = new Float32Array(triCount * 9);
    const indices = new Uint32Array(triCount * 3);

    let offset = 84;
    for (let t = 0; t < triCount; t++) {
      const nx = view.getFloat32(offset, true);
      const ny = view.getFloat32(offset + 4, true);
      const nz = view.getFloat32(offset + 8, true);
      offset += 12;
      for (let v = 0; v < 3; v++) {
        const base = t * 9 + v * 3;
        positions[base] = view.getFloat32(offset, true);
        positions[base + 1] = view.getFloat32(offset + 4, true);
        positions[base + 2] = view.getFloat32(offset + 8, true);
        normals[base] = nx;
        normals[base + 1] = ny;
        normals[base + 2] = nz;
        indices[t * 3 + v] = t * 3 + v;
        offset += 12;
      }
      offset += 2;
    }

    return { positions, normals, indices };
  }

  loadReferenceMesh(buffer: ArrayBuffer): boolean {
    this.setLoading("Parsing STL…");
    const mesh = this.parseStl(buffer);
    if (!mesh) {
      this.setLoading(null);
      return false;
    }
    this.setLoading("Building SDF from mesh…");
    const id = this.importMeshAsFeature(mesh.positions, mesh.normals, mesh.indices);
    this.setLoading(null);
    return id !== null;
  }

  /** K9.2: load a GLB/glTF file as a visual reference mesh.
   *  Uses Three.js GLTFLoader to parse, merges all mesh
   *  geometries into one combined referenceMesh. */
  async loadReferenceGlb(buffer: ArrayBuffer): Promise<boolean> {
    const t0 = performance.now();
    console.log(`fielddraft: importing GLB reference — ${(buffer.byteLength / 1024).toFixed(0)} KB`);
    this.setLoading("Loading GLB file…");
    try {
      const { GLTFLoader } = await import("three/examples/jsm/loaders/GLTFLoader.js");
      const loader = new GLTFLoader();
      const gltf = await new Promise<{ scene: { traverse: (cb: (child: unknown) => void) => void } }>(
        (resolve, reject) => {
          loader.parse(buffer, "", resolve as never, reject);
        },
      );

      type GeoLike = { getAttribute: (n: string) => { array: ArrayLike<number>; count: number } | null; getIndex: () => { array: ArrayLike<number> } | null };
      const allPos: Float32Array[] = [];
      const allNor: Float32Array[] = [];
      const allIdx: Uint32Array[] = [];
      let vertexOffset = 0;

      gltf.scene.traverse((child: unknown) => {
        const mesh = child as { isMesh?: boolean; geometry?: GeoLike };
        if (!mesh.isMesh || !mesh.geometry) return;
        const posAttr = mesh.geometry.getAttribute("position");
        if (!posAttr || posAttr.count === 0) return;

        const pos = new Float32Array(posAttr.array as ArrayLike<number>);
        const normalAttr = mesh.geometry.getAttribute("normal");
        const nor = normalAttr
          ? new Float32Array(normalAttr.array as ArrayLike<number>)
          : new Float32Array(pos.length);
        const indexAttr = mesh.geometry.getIndex();
        const rawIdx = indexAttr
          ? new Uint32Array(indexAttr.array as ArrayLike<number>)
          : new Uint32Array(Array.from({ length: posAttr.count }, (_, i) => i));

        if (vertexOffset > 0) {
          const offset = new Uint32Array(rawIdx.length);
          for (let i = 0; i < rawIdx.length; i++) offset[i] = rawIdx[i] + vertexOffset;
          allIdx.push(offset);
        } else {
          allIdx.push(rawIdx);
        }
        allPos.push(pos);
        allNor.push(nor);
        vertexOffset += posAttr.count;
      });

      if (allPos.length === 0) return false;

      const totalPosLen = allPos.reduce((s, a) => s + a.length, 0);
      const totalIdxLen = allIdx.reduce((s, a) => s + a.length, 0);
      const positions = new Float32Array(totalPosLen);
      const normals = new Float32Array(totalPosLen);
      const indices = new Uint32Array(totalIdxLen);
      let pOff = 0;
      for (let i = 0; i < allPos.length; i++) {
        positions.set(allPos[i], pOff);
        normals.set(allNor[i], pOff);
        pOff += allPos[i].length;
      }
      let iOff = 0;
      for (const idx of allIdx) {
        indices.set(idx, iOff);
        iOff += idx.length;
      }

      console.log(`fielddraft: GLB parsed — ${allPos.length} mesh(es), ${vertexOffset} vertices in ${(performance.now() - t0).toFixed(0)} ms`);
      this.setLoading("Building SDF from mesh…");
      const id = this.importMeshAsFeature(positions, normals, indices);
      this.setLoading(null);
      return id !== null;
    } catch (err) {
      this.setLoading(null);
      console.error("fielddraft: GLB import failed", err);
      return false;
    }
  }

  /** K9.3: load a Wavefront OBJ file as visual reference mesh.
   *  Parses the text directly — no external loader needed. */
  loadReferenceObj(text: string): boolean {
    const t0 = performance.now();
    console.log(`fielddraft: importing OBJ reference — ${(text.length / 1024).toFixed(0)} KB`);
    const verts: number[] = [];
    const positions: number[] = [];
    const normals: number[] = [];

    for (const raw of text.split("\n")) {
      const line = raw.trim();
      if (line.startsWith("v ")) {
        const parts = line.split(/\s+/);
        verts.push(
          parseFloat(parts[1]),
          parseFloat(parts[2]),
          parseFloat(parts[3]),
        );
      } else if (line.startsWith("f ")) {
        const parts = line.split(/\s+/).slice(1);
        // Triangulate fan-style for quads/n-gons.
        const idxs = parts.map((p) => parseInt(p.split("/")[0], 10) - 1);
        for (let i = 1; i < idxs.length - 1; i++) {
          for (const idx of [idxs[0], idxs[i], idxs[i + 1]]) {
            positions.push(
              verts[idx * 3] ?? 0,
              verts[idx * 3 + 1] ?? 0,
              verts[idx * 3 + 2] ?? 0,
            );
            normals.push(0, 0, 0);
          }
        }
      }
    }

    if (positions.length === 0) {
      console.error("fielddraft: OBJ import failed — no geometry found");
      return false;
    }

    console.log(`fielddraft: OBJ parsed — ${verts.length / 3} vertices, ${positions.length / 9} triangles`);
    // Compute flat normals per triangle.
    for (let i = 0; i < positions.length; i += 9) {
      const ax = positions[i + 3] - positions[i];
      const ay = positions[i + 4] - positions[i + 1];
      const az = positions[i + 5] - positions[i + 2];
      const bx = positions[i + 6] - positions[i];
      const by = positions[i + 7] - positions[i + 1];
      const bz = positions[i + 8] - positions[i + 2];
      let nx = ay * bz - az * by;
      let ny = az * bx - ax * bz;
      let nz = ax * by - ay * bx;
      const len = Math.hypot(nx, ny, nz) || 1;
      nx /= len;
      ny /= len;
      nz /= len;
      for (let v = 0; v < 3; v++) {
        normals[i + v * 3] = nx;
        normals[i + v * 3 + 1] = ny;
        normals[i + v * 3 + 2] = nz;
      }
    }

    const posArr = new Float32Array(positions);
    const normArr = new Float32Array(normals);
    const idxArr = new Uint32Array(
      Array.from({ length: positions.length / 3 }, (_, i) => i),
    );

    console.log(`fielddraft: OBJ parsed — ${positions.length / 9} triangles in ${(performance.now() - t0).toFixed(0)} ms`);
    this.setLoading("Building SDF from mesh…");
    const id = this.importMeshAsFeature(posArr, normArr, idxArr);
    this.setLoading(null);
    return id !== null;
  }

  /** K13.1: convert the current reference mesh into a CSG solid
   *  that participates in Booleans. The reference mesh is
   *  consumed (cleared) and replaced by a feature. */
  convertReferenceToSolid(): number | null {
    const ref_ = this.state.referenceMesh;
    if (!ref_ || ref_.positions.length === 0) return null;
    const t0 = performance.now();
    const triCount = ref_.indices.length / 3;
    console.log(`fielddraft: converting reference mesh to solid — ${triCount} triangles, building BVH…`);
    let id: number;
    try {
      id = this.kernel.addImportedMesh(
        ref_.positions,
        ref_.indices,
        BoolOpTag.Union,
      );
    } catch (err) {
      console.error("fielddraft: convertReferenceToSolid failed", err);
      return null;
    }
    this.state = { ...this.state, referenceMesh: null };
    this.commit(new Set([id]), id);
    console.log(`fielddraft: reference → solid complete — feature #${id} in ${(performance.now() - t0).toFixed(0)} ms`);
    return id;
  }

  clearReferenceMesh(): void {
    this.state = { ...this.state, referenceMesh: null };
    this.notify();
  }

  importMeshAsFeature(
    positions: Float32Array,
    normals: Float32Array,
    indices: Uint32Array,
  ): number | null {
    const t0 = performance.now();
    const triCount = indices.length / 3;
    console.log(`fielddraft: importing mesh as SDF feature — ${triCount} triangles, building BVH…`);
    let id: number;
    try {
      id = this.kernel.addImportedMesh(positions, indices, BoolOpTag.Union);
    } catch (err) {
      console.error("fielddraft: importMeshAsFeature failed", err);
      return null;
    }
    this.meshDataCache.set(id, { positions, normals, indices });
    this.commit(new Set([id]), id);
    console.log(`fielddraft: mesh imported as feature #${id} (solid) in ${(performance.now() - t0).toFixed(0)} ms`);
    return id;
  }

  getMeshOverlayData(id: number): MeshOverlayData | undefined {
    return this.meshDataCache.get(id);
  }

  getMeshOverlayFeatures(): { id: number; data: MeshOverlayData }[] {
    const result: { id: number; data: MeshOverlayData }[] = [];
    for (const f of this.state.features) {
      if (f.kind === "imported_mesh" && f.suppressed) {
        const data = this.meshDataCache.get(f.id);
        if (data) result.push({ id: f.id, data });
      }
    }
    return result;
  }

  // ── K8.10: document-level variables ──────────────────────────

  /** K8.10: upsert a variable and commit. Returns `true` on
   *  success, `false` when the kernel rejected the call
   *  (empty name, NaN, Infinity). */
  setVariable(name: string, value: number): boolean {
    try {
      this.kernel.setVariable(name, value);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: setVariable failed", err);
      return false;
    }
    this.commit();
    return true;
  }

  /** K8.10: remove a variable and commit. */
  removeVariable(name: string): boolean {
    if (!this.kernel.removeVariable(name)) return false;
    this.commit();
    return true;
  }

  /** K12: add a thread with specific metric preset params. */
  addThreadPreset(radius: number, pitch: number, depth: number): number {
    const offset: [number, number, number] = [
      this.state.features.length * 80,
      0,
      0,
    ];
    let id: number;
    try {
      id = this.kernel.addThread(this.state.nextOp, offset, radius, pitch, depth);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: addThreadPreset failed", err);
      return -1;
    }
    this.commit(new Set([id]), id);
    return id;
  }

  /** K11.0: add a gyroid TPMS infill. Default: 10 mm cells,
   *  1 mm wall thickness, op=Intersect. */
  addGyroid(cellSizeMm = 10, thicknessMm = 1): number {
    let id: number;
    try {
      id = this.kernel.addGyroid(cellSizeMm, thicknessMm, BoolOpTag.Intersect);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: addGyroid failed", err);
      return -1;
    }
    this.commit(new Set([id]), id);
    return id;
  }

  addHole(diameterMm = 6, depthMm = 40): number {
    const offset: [number, number, number] = [0, 0, 0];
    let id: number;
    try {
      id = this.kernel.addHole(
        diameterMm,
        depthMm,
        offset[0],
        offset[1],
        offset[2],
      );
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: addHole failed", err);
      return -1;
    }
    this.commit(new Set([id]), id);
    return id;
  }

  addPrimitive(
    kind: PrimitiveKindName,
    op: BoolOpTagValue = this.state.nextOp,
    translate?: [number, number, number],
  ): number {
    const offset: [number, number, number] = translate ?? [0, 0, 0];
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
      case "torus":
        id = this.kernel.addTorus(op, offset, 30, 10);
        break;
      case "thread":
        id = this.kernel.addThread(op, offset, 3, 0.5, 0.3);
        break;
      case "cone":
        id = this.kernel.addCone(op, offset, 40, 10, 60);
        break;
      case "capsule":
        id = this.kernel.addCapsule(op, offset, 20, 40);
        break;
      case "hex_prism":
        id = this.kernel.addHexPrism(op, offset, 25, 30);
        break;
      case "ellipsoid":
        id = this.kernel.addEllipsoid(op, offset, 40, 25, 30);
        break;
      case "hole":
        return this.addHole();
      case "linear_pattern": {
        const pid = this.addLinearPatternOfLatest();
        if (pid == null) throw new Error("linear_pattern: no valid source feature");
        return pid;
      }
      case "circular_pattern": {
        const pid = this.addCircularPatternOfLatest();
        if (pid == null) throw new Error("circular_pattern: no valid source feature");
        return pid;
      }
      default: {
        const _exhaustive: never = kind;
        throw new Error(`addPrimitive: unknown kind "${_exhaustive}"`);
      }
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
      if (this.kernel.removeFeature(id)) {
        this.meshDataCache.delete(id);
        removed = true;
      }
    }
    if (removed) this.commit(new Set(), null);
    return removed;
  }

  removeFeature(id: number): boolean {
    if (!this.kernel.removeFeature(id)) return false;
    this.meshDataCache.delete(id);
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

  /** Save the current project as a downloadable .fdraft file. */
  saveProjectToFile(filename = "project.fdraft"): void {
    if (typeof document === "undefined") return;
    const json = this.kernel.toJson();
    const blob = new Blob([json], { type: "application/json" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = filename;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    URL.revokeObjectURL(url);
  }

  /** Load a project from a .fdraft JSON string. */
  loadProjectFromJson(json: string): boolean {
    try {
      this.kernel.loadJson(json);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: loadProjectFromJson failed", err);
      return false;
    }
    this.commit(new Set(), null);
    this.autosave.flush();
    return true;
  }

  /** M12: set the server-side project ID (activates sync). */
  setProjectId(id: string | null): void {
    this.state = {
      ...this.state,
      projectId: id,
      syncState: id ? "dirty" : "offline",
    };
    for (const listener of this.listeners) listener();
  }

  /** M12: sync the current document to the server. No-op in
   *  offline mode. */
  async syncToServer(): Promise<boolean> {
    if (!this.state.projectId || this.state.syncState === "offline") {
      return false;
    }
    this.state = { ...this.state, syncState: "syncing" };
    for (const listener of this.listeners) listener();
    try {
      const res = await fetch(
        `/api/projects/${this.state.projectId}`,
        {
          method: "PATCH",
          credentials: "include",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({
            document: JSON.parse(this.kernel.toJson()),
          }),
        },
      );
      if (!res.ok) throw new Error(`sync failed: ${res.status}`);
      this.state = { ...this.state, syncState: "clean" };
      for (const listener of this.listeners) listener();
      return true;
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: sync failed", err);
      this.state = { ...this.state, syncState: "error" };
      for (const listener of this.listeners) listener();
      return false;
    }
  }

  /** Clear the document back to a fresh empty state. */
  newProject(): void {
    this.kernel.loadJson('{"schema_version":1,"name":"Untitled","feature_calls":[],"next_feature_id":1}');
    this.commit(new Set(), null);
    this.autosave.flush();
  }

  /** K9.6: add a mate connector at the current pick point. */
  addMateConnectorAtPick(
    point: [number, number, number],
    normal: [number, number, number],
  ): number | null {
    let id: number;
    try {
      id = this.kernel.addMateConnector(
        point[0], point[1], point[2],
        normal[0], normal[1], normal[2],
      );
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: addMateConnector failed", err);
      return null;
    }
    this.commit(new Set([id]), id);
    return id;
  }

  /** K10.6: add a fillet_edge feature at a detected edge. */
  addFilletAtEdge(
    featureA: number,
    featureB: number,
    p0: [number, number, number],
    p1: [number, number, number],
    radius = 5,
  ): number | null {
    let id: number;
    try {
      id = this.kernel.addFilletEdge(
        featureA,
        featureB,
        p0[0], p0[1], p0[2],
        p1[0], p1[1], p1[2],
        radius * 2,
        radius,
      );
    } catch (err) {
      console.error("fielddraft: addFilletAtEdge failed", err);
      this.setTransientError(formatKernelError(err, "Fillet failed"));
      return null;
    }
    this.commit(new Set([id]), id);

    // K10 ghost-edge rollback: if there are older fillet_edge features
    // that touch the same contributors, reorder this fillet before the
    // earliest one. This ensures the new fillet applies to the original
    // edge geometry rather than the already-smoothed surface.
    try {
      const features = this.state.features;
      const newIdx = features.findIndex((f) => f.id === id);
      if (newIdx >= 0) {
        let earliestFilletIdx = newIdx;
        for (let i = 0; i < newIdx; i++) {
          const f = features[i];
          if (f.kind === "fillet_edge") {
            const params = this.kernel.getFeatureCallParams(f.id);
            const fa = params?.feature_a as number | undefined;
            const fb = params?.feature_b as number | undefined;
            if ((fa === featureA || fa === featureB) && (fb === featureA || fb === featureB)) {
              earliestFilletIdx = i;
              break;
            }
          }
        }
        if (earliestFilletIdx < newIdx) {
          this.kernel.reorderFeature(id, earliestFilletIdx);
          this.commit(new Set([id]), id);
        }
      }
    } catch {
      // Rollback is best-effort; don't break the fillet creation.
    }

    return id;
  }

  toggleMeasureMode(): void {
    this.state = { ...this.state, measureMode: !this.state.measureMode, filletMode: false, mateMode: false };
    for (const listener of this.listeners) listener();
  }

  toggleFilletMode(): void {
    this.state = { ...this.state, filletMode: !this.state.filletMode, measureMode: false, mateMode: false };
    for (const listener of this.listeners) listener();
  }

  toggleMateMode(): void {
    this.state = { ...this.state, mateMode: !this.state.mateMode, filletMode: false, measureMode: false };
    for (const listener of this.listeners) listener();
  }

  // ── P1: Assembly management ────────────────────────────────────

  enterAssembly(): void {
    this.state = {
      ...this.state,
      assembly: {
        active: true,
        document: { ...this.state.assembly.document },
        selectedPartId: null,
      },
    };
    this.notify();
  }

  exitAssembly(): void {
    this.state = { ...this.state, assembly: INACTIVE_ASSEMBLY };
    this.notify();
  }

  addPartToAssembly(documentJson: string, name?: string): void {
    const asm = this.state.assembly;
    if (!asm.active) return;

    const id = allocateAssemblyId();
    const partName = name ?? `Part ${asm.document.parts.length + 1}`;

    let meshBodies: AssemblyMeshedBody[] = [];
    let mateConnectors: import("./assembly").MateConnectorDef[] = [];
    const savedJson = this.kernel.toJson();
    const wasSyncOff = this.kernel.setSyncMeshing(true);
    try {
      this.kernel.loadJson(documentJson);
      const bodyCount = this.kernel.bodyCount();
      for (let i = 0; i < bodyCount; i++) {
        meshBodies.push({
          positions: this.kernel.bodyPositionsView(i).slice(),
          normals: this.kernel.bodyNormalsView(i).slice(),
          indices: this.kernel.bodyIndicesView(i).slice(),
        });
      }
      const features = JSON.parse(this.kernel.toJson()).feature_calls ?? [];
      for (const fc of features) {
        if (fc.type === "mate_connector" && fc.params) {
          const origin = fc.params.origin ?? [0, 0, 0];
          const normal = fc.params.normal ?? [0, 1, 0];
          mateConnectors.push({
            featureId: fc.id ?? 0,
            origin: [origin[0], origin[1], origin[2]],
            normal: [normal[0], normal[1], normal[2]],
          });
        }
      }
    } catch {
      // Meshing failed — add the part with empty meshes
    } finally {
      this.kernel.loadJson(savedJson);
      if (wasSyncOff) this.kernel.setSyncMeshing(false);
    }

    const newPart: AssemblyPartInstance = {
      id,
      name: partName,
      documentJson,
      transform: IDENTITY_TRANSFORM,
      meshBodies,
      mateConnectors,
    };

    this.state = {
      ...this.state,
      assembly: {
        ...asm,
        document: {
          ...asm.document,
          parts: [...asm.document.parts, newPart],
        },
      },
    };
    this.notify();
  }

  removePartFromAssembly(partId: string): void {
    const asm = this.state.assembly;
    if (!asm.active) return;
    this.state = {
      ...this.state,
      assembly: {
        ...asm,
        document: {
          ...asm.document,
          parts: asm.document.parts.filter((p) => p.id !== partId),
          mates: asm.document.mates.filter(
            (m) => m.connectorA.partId !== partId && m.connectorB.partId !== partId,
          ),
        },
        selectedPartId: asm.selectedPartId === partId ? null : asm.selectedPartId,
      },
    };
    this.notify();
  }

  selectAssemblyPart(partId: string | null): void {
    const asm = this.state.assembly;
    if (!asm.active) return;
    this.state = {
      ...this.state,
      assembly: { ...asm, selectedPartId: partId },
    };
    this.notify();
  }

  setPartTransform(
    partId: string,
    translate: [number, number, number],
    rotate: [number, number, number],
  ): void {
    const asm = this.state.assembly;
    if (!asm.active) return;
    this.state = {
      ...this.state,
      assembly: {
        ...asm,
        document: {
          ...asm.document,
          parts: asm.document.parts.map((p) =>
            p.id === partId ? { ...p, transform: { translate, rotate } } : p,
          ),
        },
      },
    };
    this.notify();
  }

  addMate(
    type: MateType,
    partAId: string,
    connAFeatureId: number,
    partBId: string,
    connBFeatureId: number,
  ): void {
    const asm = this.state.assembly;
    if (!asm.active) return;
    const mate: Mate = {
      id: allocateAssemblyId(),
      name: `${type} mate`,
      type,
      connectorA: { partId: partAId, connectorFeatureId: connAFeatureId },
      connectorB: { partId: partBId, connectorFeatureId: connBFeatureId },
      angle: type === "revolute" ? 0 : undefined,
      offset: type === "prismatic" ? 0 : undefined,
    };
    const newDoc = {
      ...asm.document,
      mates: [...asm.document.mates, mate],
    };
    const solved = solveAssembly({ ...newDoc, parts: asm.document.parts });
    this.state = {
      ...this.state,
      assembly: { ...asm, document: { ...newDoc, parts: solved } },
    };
    this.notify();
  }

  removeMate(mateId: string): void {
    const asm = this.state.assembly;
    if (!asm.active) return;
    this.state = {
      ...this.state,
      assembly: {
        ...asm,
        document: {
          ...asm.document,
          mates: asm.document.mates.filter((m) => m.id !== mateId),
        },
      },
    };
    this.notify();
  }

  setMateAngle(mateId: string, angleDeg: number): void {
    const asm = this.state.assembly;
    if (!asm.active) return;
    const newMates = asm.document.mates.map((m) =>
      m.id === mateId ? { ...m, angle: angleDeg } : m,
    );
    const newDoc = { ...asm.document, mates: newMates };
    const solved = solveAssembly({ ...newDoc, parts: asm.document.parts });
    this.state = {
      ...this.state,
      assembly: { ...asm, document: { ...newDoc, parts: solved } },
    };
    this.notify();
  }

  setAssemblyScript(source: string): void {
    const asm = this.state.assembly;
    if (!asm.active) return;
    this.state = {
      ...this.state,
      assembly: {
        ...asm,
        document: { ...asm.document, script: source },
      },
    };
    this.notify();
  }

  runFieldScript(
    params: Record<string, number | boolean>,
    time: number,
    dt: number,
  ): string | null {
    const asm = this.state.assembly;
    if (!asm.active || !asm.document.script) return null;

    const currentAngles = new Map<string, number>();
    for (const mate of asm.document.mates) {
      if (mate.type === "revolute") {
        currentAngles.set(mate.id, mate.angle ?? 0);
      }
    }

    const result = executeScript(asm.document.script, params, time, dt, currentAngles);

    if (!result.success) return result.error ?? "script error";

    if (result.jointUpdates.size > 0) {
      let newMates = [...asm.document.mates];
      for (const [mateId, angle] of result.jointUpdates) {
        newMates = newMates.map((m) =>
          m.id === mateId ? { ...m, angle } : m,
        );
      }
      const newDoc = { ...asm.document, mates: newMates };
      const solved = solveAssembly({ ...newDoc, parts: asm.document.parts });
      this.state = {
        ...this.state,
        assembly: { ...asm, document: { ...newDoc, parts: solved } },
      };
      this.notify();
    }

    return null;
  }

  exportR3F(): void {
    if (!this.state.assembly.active) return;
    import("./exportR3F").then(({ downloadR3FExport }) => {
      downloadR3FExport(this.state.assembly.document);
    });
  }

  async publishProject(): Promise<string | null> {
    if (!this.state.projectId) return null;
    try {
      const resp = await fetch(`/api/projects/${this.state.projectId}/publish`, {
        method: "POST",
        credentials: "include",
      });
      if (!resp.ok) return null;
      const data = await resp.json();
      const viewUrl = data.view_url ?? `/api/v/${data.snapshot_id}`;
      const embed = `<iframe src="${window.location.origin}${viewUrl}" width="640" height="480" frameborder="0" allow="webgpu"></iframe>`;
      try {
        await navigator.clipboard.writeText(embed);
      } catch {
        prompt("Share embed code:", embed);
      }
      return data.snapshot_id;
    } catch {
      return null;
    }
  }

  toggleIKMode(): void {
    this.state = { ...this.state, ikMode: !this.state.ikMode };
    this.notify();
  }

  runIK(target: IKTarget): void {
    const asm = this.state.assembly;
    if (!asm.active) return;
    const solved = solveIK(asm.document, target);
    if (!solved) return;
    const newDoc = { ...asm.document, mates: solved };
    const parts = solveAssembly({ ...newDoc, parts: asm.document.parts });
    this.state = {
      ...this.state,
      assembly: { ...asm, document: { ...newDoc, parts } },
    };
    this.notify();
  }

  setMateOffset(mateId: string, offsetMm: number): void {
    const asm = this.state.assembly;
    if (!asm.active) return;
    const newMates = asm.document.mates.map((m) =>
      m.id === mateId ? { ...m, offset: offsetMm } : m,
    );
    const newDoc = { ...asm.document, mates: newMates };
    const solved = solveAssembly({ ...newDoc, parts: asm.document.parts });
    this.state = {
      ...this.state,
      assembly: { ...asm, document: { ...newDoc, parts: solved } },
    };
    this.notify();
  }

  setMateJointLimits(mateId: string, minAngle: number, maxAngle: number): void {
    const asm = this.state.assembly;
    if (!asm.active) return;
    this.state = {
      ...this.state,
      assembly: {
        ...asm,
        document: {
          ...asm.document,
          mates: asm.document.mates.map((m) =>
            m.id === mateId ? { ...m, minAngle, maxAngle } : m,
          ),
        },
      },
    };
    this.notify();
  }

  selectAll(): void {
    const all = new Set(this.state.features.map((f) => f.id));
    this.state = { ...this.state, selection: all, selectionAnchor: null };
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

  setLoading(phase: string | null): void {
    this.state = { ...this.state, loading: phase ? { phase } : null };
    this.notify();
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

  enterFacePickMode(): void {
    this.state = { ...this.state, facePickMode: true };
    this.notify();
  }

  cancelFacePickMode(): void {
    this.state = { ...this.state, facePickMode: false };
    this.notify();
  }

  enterSketchFromFacePick(normal: [number, number, number]): void {
    this.state = { ...this.state, facePickMode: false };
    const planeQuery = normalToPlaneQuery(normal);
    this.updateSketch(reduceEnter(this.state.sketchSession, planeQuery));
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

  toggleConstraintDriven(index: number): void {
    const session = this.state.sketchSession;
    const call = session.subCalls[index];
    if (!call) return;
    const isDimensional =
      call.type === "sketch_constraint_distance" ||
      call.type === "sketch_constraint_angle";
    if (!isDimensional) return;
    const next = [...session.subCalls];
    next[index] = {
      ...call,
      params: { ...call.params, driven: !call.params.driven },
    };
    this.updateSketch({ ...session, subCalls: next });
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

  /** K7.22 UX-fix (1): public reorder that drives the Outliner's
   *  drag/drop handler. `targetIndex` is the destination slot in
   *  the features list (clamped by the kernel). No-op if the
   *  feature is already at that slot. */
  reorderFeature(id: number, targetIndex: number): boolean {
    const idx = this.state.features.findIndex((f) => f.id === id);
    if (idx < 0 || idx === targetIndex) return false;
    if (!this.kernel.reorderFeature(id, targetIndex)) return false;
    this.commit();
    return true;
  }

  /** K7.22 UX-fix (2): commit-style setter for a feature's world-
   *  space translate. Used by the viewport gizmo drag handler —
   *  it keeps the store in sync with the kernel, triggers a worker
   *  re-mesh and refreshes the features list so the outliner
   *  highlight and the gizmo's own placeholder position both
   *  follow the new translate. Returns `true` if anything changed. */
  extrudeFace(
    point: [number, number, number],
    normal: [number, number, number],
    depth = 20,
  ): void {
    const halfDepth = depth / 2;
    const translate: [number, number, number] = [
      point[0] + normal[0] * halfDepth,
      point[1] + normal[1] * halfDepth,
      point[2] + normal[2] * halfDepth,
    ];
    this.addPrimitive("box", undefined, translate);
  }

  setGizmoMode(mode: "translate" | "rotate" | "scale"): void {
    this.state = { ...this.state, gizmoMode: mode };
    this.notify();
  }

  setFeatureTranslate(id: number, x: number, y: number, z: number): boolean {
    if (!this.kernel.setFeatureTranslate(id, x, y, z)) return false;
    this.commit();
    return true;
  }

  setFeatureRotation(id: number, rx: number, ry: number, rz: number, rw: number): boolean {
    if (!this.kernel.setFeatureRotation(id, rx, ry, rz, rw)) return false;
    this.commit();
    return true;
  }

  setFeatureScale(id: number, scale: number): boolean {
    if (!this.kernel.setFeatureScale(id, scale)) return false;
    this.commit();
    return true;
  }

  /** K7.22 UX-fix (5): flip a sketch's 3D-visible state. When on,
   *  the Viewport renders the sketch's geometry as line overlays
   *  on the sketch plane. Default off. Not a kernel mutation —
   *  lives entirely in the store + localStorage. */
  toggleSketchVisible(sketchId: number): void {
    const current = this.state.visibleSketches;
    const next = current.includes(sketchId)
      ? current.filter((id) => id !== sketchId)
      : [...current, sketchId];
    this.state = { ...this.state, visibleSketches: next };
    persistVisibleSketchesToStorage(next);
    for (const listener of this.listeners) listener();
  }

  // ── K7.22 UX-fix (2): feature folders (display-only grouping) ──

  /** Update the `folders` slice of state, persist to localStorage,
   *  notify React subscribers. Kernel is not touched. */
  private setFolders(next: readonly FeatureFolder[]): void {
    this.state = { ...this.state, folders: next };
    persistFoldersToStorage(next);
    for (const listener of this.listeners) listener();
  }

  /** Create a new empty folder with a default name. Returns the
   *  new folder id so UI code can immediately open it for rename.
   *  The default name is suffixed with the first free integer so
   *  repeated clicks create `Folder 1`, `Folder 2`, … without
   *  collisions. */
  createFolder(): string {
    const existingNames = new Set(this.state.folders.map((f) => f.name));
    let n = 1;
    while (existingNames.has(`Folder ${n}`)) n += 1;
    const id = `folder_${Date.now()}_${Math.floor(Math.random() * 1e6)}`;
    const folder: FeatureFolder = {
      id,
      name: `Folder ${n}`,
      memberIds: [],
      collapsed: false,
    };
    this.setFolders([...this.state.folders, folder]);
    return id;
  }

  /** Rename a folder in place. No-op for unknown ids. */
  renameFolder(folderId: string, name: string): void {
    const trimmed = name.trim();
    if (trimmed.length === 0) return;
    this.setFolders(
      this.state.folders.map((f) =>
        f.id === folderId ? { ...f, name: trimmed } : f,
      ),
    );
  }

  /** Toggle a folder's collapsed flag. The actual render decision
   *  happens in the outliner JSX — this just flips the state bit. */
  toggleFolderCollapsed(folderId: string): void {
    this.setFolders(
      this.state.folders.map((f) =>
        f.id === folderId ? { ...f, collapsed: !f.collapsed } : f,
      ),
    );
  }

  /** Move a feature into a folder. If the feature was previously
   *  a member of another folder, it is removed from there first.
   *  Dropping onto the same folder is a no-op. */
  addFeatureToFolder(featureId: number, folderId: string): void {
    this.setFolders(
      this.state.folders.map((f) => {
        const inFolder = f.memberIds.includes(featureId);
        if (f.id === folderId) {
          if (inFolder) return f;
          return { ...f, memberIds: [...f.memberIds, featureId] };
        }
        if (inFolder) {
          return {
            ...f,
            memberIds: f.memberIds.filter((id) => id !== featureId),
          };
        }
        return f;
      }),
    );
  }

  /** Remove a feature from any folder it belongs to. The feature
   *  itself stays in the kernel. */
  removeFeatureFromFolder(featureId: number): void {
    this.setFolders(
      this.state.folders.map((f) =>
        f.memberIds.includes(featureId)
          ? {
              ...f,
              memberIds: f.memberIds.filter((id) => id !== featureId),
            }
          : f,
      ),
    );
  }

  /** Delete a folder. Its members are preserved as ungrouped
   *  features. */
  deleteFolder(folderId: string): void {
    this.setFolders(this.state.folders.filter((f) => f.id !== folderId));
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
      this.setTransientError(formatKernelError(err, "Parameter update failed"));
      return false;
    }
    this.commit();
    return true;
  }

  // ── K7.4: extrude convenience ─────────────────────────────────

  latestSketchId(): number | null {
    const sketches = this.state.features.filter((f) => f.kind === "sketch");
    if (sketches.length === 0) return null;
    return sketches.reduce((acc, f) => (f.id > acc ? f.id : acc), sketches[0].id);
  }

  private static NON_SPINE_KINDS = new Set([
    "sketch", "linear_pattern", "circular_pattern", "mirror",
    "mate_connector", "transform", "hole", "fillet_edge",
  ]);

  private latestSpineFeature(): FeatureSummary | null {
    for (let i = this.state.features.length - 1; i >= 0; i--) {
      const f = this.state.features[i];
      if (!KernelStore.NON_SPINE_KINDS.has(f.kind) && !f.suppressed) return f;
    }
    return null;
  }

  addCircularPatternOfLatest(): number | null {
    const source = this.latestSpineFeature();
    if (!source) {
      this.setTransientError("Circular Pattern: no valid source feature");
      return null;
    }
    let id: number;
    try {
      id = this.kernel.addCircularPattern(source.id, 0, 1, 0, 0, 0, 0, 6, 360);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: addCircularPattern failed", err);
      this.setTransientError(formatKernelError(err, "Circular Pattern failed"));
      return null;
    }
    this.commit(new Set([id]), id);
    return id;
  }

  addLinearPatternOfLatest(): number | null {
    const source = this.latestSpineFeature();
    if (!source) {
      this.setTransientError("Linear Pattern: no valid source feature");
      return null;
    }
    let id: number;
    try {
      id = this.kernel.addLinearPattern(source.id, 1, 0, 0, 3, 60);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: addLinearPattern failed", err);
      this.setTransientError(formatKernelError(err, "Linear Pattern failed"));
      return null;
    }
    this.commit(new Set([id]), id);
    return id;
  }

  addMirrorOfLatest(): number | null {
    const source = this.latestSpineFeature();
    if (!source) {
      this.setTransientError("Mirror: no valid source feature");
      return null;
    }
    let id: number;
    try {
      id = this.kernel.addMirror(source.id, 1, 0, 0, 0, 0, 0);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: addMirror failed", err);
      this.setTransientError(formatKernelError(err, "Mirror failed"));
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
   *  id, or `null` when there's no sketch to consume or the
   *  kernel rejected the regeneration (e.g. the sketch has no
   *  closed regions). Kernel errors are surfaced via the
   *  `transientError` state slot so the App can render a toast. */
  addExtrudeFromLatestSketch(depthMm: number): number | null {
    const sketchId = this.latestSketchId();
    if (sketchId === null) return null;
    let id: number;
    try {
      id = this.kernel.addExtrude(sketchId, depthMm, this.state.nextOp, 0);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: addExtrude failed", err);
      this.setTransientError(formatKernelError(err, "Extrude failed"));
      return null;
    }
    this.commit(new Set([id]), id);
    return id;
  }

  private transientErrorTimer: ReturnType<typeof setTimeout> | null = null;

  showTransientError(message: string): void {
    this.setTransientError(message);
  }

  private setTransientError(message: string): void {
    if (this.transientErrorTimer) clearTimeout(this.transientErrorTimer);
    this.state = { ...this.state, transientError: message };
    for (const listener of this.listeners) listener();
    this.transientErrorTimer = setTimeout(() => {
      this.transientErrorTimer = null;
      this.state = { ...this.state, transientError: null };
      for (const listener of this.listeners) listener();
    }, 5000);
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

  /** M14a: export the current scene as a 3MF archive with
   *  per-body colors. Same WASM→Blob→download pattern as STL. */
  download3mf(filename = "fielddraft.3mf"): void {
    if (
      typeof window === "undefined" ||
      typeof document === "undefined" ||
      typeof URL === "undefined" ||
      typeof URL.createObjectURL !== "function"
    ) {
      return;
    }
    const bytes = this.kernel.export3mf();
    const copy = new Uint8Array(bytes.length);
    copy.set(bytes);
    const blob = new Blob([copy], { type: "application/vnd.ms-package.3dmanufacturing-3dmodel+xml" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = filename;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    URL.revokeObjectURL(url);
  }

  /** Import a .3mf file: extract meshes as CSG solids, stash
   *  slicer metadata for roundtrip re-export. */
  import3mfFile(buffer: ArrayBuffer): number[] {
    const t0 = performance.now();
    console.log(`fielddraft: importing 3MF — ${(buffer.byteLength / 1024).toFixed(0)} KB`);
    this.setLoading("Extracting 3MF archive…");
    try {
      const result = import3mf(buffer);
      console.log(`fielddraft: 3MF extracted — ${result.meshes.length} mesh(es)`);
      const ids: number[] = [];
      for (let i = 0; i < result.meshes.length; i++) {
        const mesh = result.meshes[i];
        this.setLoading(`Building SDF ${i + 1}/${result.meshes.length}…`);
        console.log(`fielddraft: 3MF mesh ${i + 1} — ${mesh.indices.length / 3} triangles, building BVH…`);
        const id = this.kernel.addImportedMesh(
          mesh.positions,
          mesh.indices,
          BoolOpTag.Union,
        );
        ids.push(id);
      }
      this.state = { ...this.state, threemfStash: result.stash, loading: null };
      if (ids.length > 0) {
        this.commit(new Set(ids), ids[0]);
      } else {
        this.notify();
      }
      console.log(`fielddraft: 3MF import complete — ${ids.length} body(ies) in ${(performance.now() - t0).toFixed(0)} ms`);
      return ids;
    } catch (e) {
      this.setLoading(null);
      console.error("fielddraft: 3MF import failed", e);
      alert(`3MF import failed: ${e instanceof Error ? e.message : e}`);
      return [];
    }
  }

  /** Re-export as 3MF, preserving stashed slicer metadata from the
   *  last import. Falls back to normal 3MF export if no stash exists. */
  download3mfRoundtrip(filename = "fielddraft.3mf"): void {
    if (
      typeof window === "undefined" ||
      typeof document === "undefined" ||
      typeof URL === "undefined" ||
      typeof URL.createObjectURL !== "function"
    ) {
      return;
    }
    const stash = this.state.threemfStash;
    if (!stash) {
      this.download3mf(filename);
      return;
    }
    const kernelZip = this.kernel.export3mf();
    const copy = new Uint8Array(kernelZip.length);
    copy.set(kernelZip);
    const modelXmlOnly = extractModelXml(copy);
    const bytes = export3mfRoundtrip(modelXmlOnly, stash);
    const blobData = new Uint8Array(bytes.length);
    blobData.set(bytes);
    const blob = new Blob([blobData as unknown as BlobPart], {
      type: "application/vnd.ms-package.3dmanufacturing-3dmodel+xml",
    });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = filename;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    URL.revokeObjectURL(url);
  }


  /** M14c: export the mechanical core of the scene as STEP via
   *  the OCCT Web Worker. Lazy-loads opencascade.js (~25 MB)
   *  on first call. */
  async downloadStep(filename = "fielddraft.step"): Promise<void> {
    if (typeof window === "undefined" || typeof document === "undefined") return;
    try {
      const classJson = this.kernel.classifyForStep();
      const report = JSON.parse(classJson) as {
        hard: number;
        degraded: number;
        excluded: number;
        total_geometric: number;
        features: Array<{ id: string; name: string; status: string }>;
      };

      if (report.total_geometric === 0) {
        alert("No geometric features to export.");
        return;
      }

      if (report.degraded > 0 || report.excluded > 0) {
        const lines: string[] = ["STEP Export — Feature Report\n"];
        if (report.excluded > 0) {
          lines.push(`${report.excluded} feature(s) will be EXCLUDED (not exportable):`);
          for (const f of report.features) {
            if (f.status.startsWith("excluded:")) {
              lines.push(`  • ${f.name}: ${f.status.slice(10)}`);
            }
          }
          lines.push("");
        }
        if (report.degraded > 0) {
          lines.push(`${report.degraded} feature(s) will be DEGRADED (sharp edges instead of fillets/chamfers):`);
          for (const f of report.features) {
            if (f.status.startsWith("degraded:")) {
              lines.push(`  • ${f.name}: ${f.status.slice(10)}`);
            }
          }
          lines.push("");
        }
        lines.push(`${report.hard} feature(s) will export exactly.`);
        lines.push("\nProceed with export?");

        if (!confirm(lines.join("\n"))) return;
      }

      const { exportStep } = await import("./stepExport");
      const commandsJson = this.kernel.stepCommands();
      const commands = JSON.parse(commandsJson);
      if (!Array.isArray(commands) || commands.length === 0) {
        alert("No STEP-exportable features found.");
        return;
      }
      const stepBytes = await exportStep(commands);
      const copy = new Uint8Array(stepBytes.length);
      copy.set(stepBytes);
      const blob = new Blob([copy], { type: "application/step" });
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = filename;
      document.body.appendChild(a);
      a.click();
      document.body.removeChild(a);
      URL.revokeObjectURL(url);
    } catch (e) {
      console.error("STEP export failed:", e);
      alert(`STEP export failed: ${e instanceof Error ? e.message : e}`);
    }
  }

  /** M14i: import a STEP file via the OCCT worker. Tessellates
   *  the B-Rep geometry and adds it as an ImportedMesh feature. */
  async importStepFile(buffer: ArrayBuffer): Promise<number | null> {
    if (typeof window === "undefined") return null;
    const t0 = performance.now();
    console.log(`fielddraft: importing STEP — ${(buffer.byteLength / 1024).toFixed(0)} KB`);
    this.setLoading("Loading STEP importer…");
    try {
      const { importStep } = await import("./stepImport");
      const fileBytes = new Uint8Array(buffer);
      const { positions, indices } = await importStep(fileBytes, (phase) => {
        console.log(`fielddraft: STEP phase — ${phase}`);
        this.setLoading(phase);
      });

      const posF32 = positions instanceof Float32Array ? positions : new Float32Array(positions);
      const idxU32 = indices instanceof Uint32Array ? indices : new Uint32Array(indices);

      console.log(`fielddraft: STEP tessellated — ${idxU32.length / 3} triangles, building BVH…`);
      this.setLoading("Building SDF from mesh…");
      const id = this.kernel.addImportedMesh(
        posF32,
        idxU32,
        BoolOpTag.Union,
      );
      this.setLoading(null);
      this.commit(new Set([id]), id);
      console.log(`fielddraft: STEP import complete — feature #${id} in ${(performance.now() - t0).toFixed(0)} ms`);
      return id;
    } catch (e) {
      this.setLoading(null);
      console.error("fielddraft: STEP import failed", e);
      alert(`STEP import failed: ${e instanceof Error ? e.message : e}`);
      return null;
    }
  }

  /** K12.1: export the current scene as a binary GLB file
   *  (glTF 2.0). Builds a temporary Three.js scene from the
   *  body mesh cache and runs the GLTFExporter. */
  async downloadGlb(filename = "fielddraft.glb"): Promise<void> {
    if (typeof window === "undefined" || typeof document === "undefined") {
      return;
    }
    const THREE = await import("three");
    const { GLTFExporter } = await import("three/examples/jsm/exporters/GLTFExporter.js");

    const scene = new THREE.Scene();
    const mat = new THREE.MeshStandardMaterial({ color: 0x8ca6d9 });
    for (const body of this.state.meshedBodies) {
      if (body.positions.length === 0) continue;
      const geo = new THREE.BufferGeometry();
      geo.setAttribute("position", new THREE.BufferAttribute(body.positions, 3));
      geo.setAttribute("normal", new THREE.BufferAttribute(body.normals, 3));
      geo.setIndex(new THREE.BufferAttribute(body.indices, 1));
      scene.add(new THREE.Mesh(geo, mat));
    }

    const exporter = new GLTFExporter();
    const glb = await exporter.parseAsync(scene, { binary: true }) as ArrayBuffer;
    const blob = new Blob([glb], { type: "model/gltf-binary" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = filename;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    URL.revokeObjectURL(url);
  }

  /** Export the current scene as a Wavefront OBJ text file. */
  downloadObj(filename = "fielddraft.obj"): void {
    if (typeof document === "undefined") return;
    const lines: string[] = ["# FieldDraft OBJ export"];
    let vertexOffset = 0;
    for (const body of this.state.meshedBodies) {
      if (body.positions.length === 0) continue;
      const vertCount = body.positions.length / 3;
      for (let i = 0; i < body.positions.length; i += 3) {
        lines.push(
          `v ${body.positions[i].toFixed(6)} ${body.positions[i + 1].toFixed(6)} ${body.positions[i + 2].toFixed(6)}`,
        );
      }
      for (let i = 0; i < body.normals.length; i += 3) {
        lines.push(
          `vn ${body.normals[i].toFixed(6)} ${body.normals[i + 1].toFixed(6)} ${body.normals[i + 2].toFixed(6)}`,
        );
      }
      for (let i = 0; i < body.indices.length; i += 3) {
        const a = body.indices[i] + 1 + vertexOffset;
        const b = body.indices[i + 1] + 1 + vertexOffset;
        const c = body.indices[i + 2] + 1 + vertexOffset;
        lines.push(`f ${a}//${a} ${b}//${b} ${c}//${c}`);
      }
      vertexOffset += vertCount;
    }
    const text = lines.join("\n") + "\n";
    const blob = new Blob([text], { type: "text/plain" });
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
      this.setTransientError("Sketch discarded — add geometry before finishing.");
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
      this.setTransientError(formatKernelError(err, "Sketch commit failed"));
      this.exitSketch();
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
