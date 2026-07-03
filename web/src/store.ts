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
  type VarKind,
} from "./kernel";
import {
  createAutoSave,
  createNoopAutoSave,
  loadPersisted,
  type AutoSaveHandle,
} from "./persistence";
import { bootProjectPath } from "./bootParams";
import {
  parseVariableInput,
  resolveVariableTable,
  type RawVariable,
} from "./variableInput";
import type { PickedElement } from "./elementQuery";
import type {
  MesherBodiesResponse,
  MesherBodyPayload,
  MesherRebuildRequest,
} from "./mesher.worker";
import type {
  WeaveBrepPayload,
  WeaveBrepsResponse,
  WeaveRequest,
} from "./weaver.worker";
import {
  CoalescingWorkerChannel,
  type ChannelEvent,
  type OutboundRequest,
} from "./workerChannel";
import {
  INACTIVE_SKETCH_SESSION as _INACTIVE,
  type SketchSessionState,
} from "./sketchSession";
import { logUserAction } from "./userActionLog";
import type { Vec2 } from "./sketchTrim";
import { getPublishedPickCamera } from "./pickCameraBridge";
import type { SketchDiagnostics } from "./solver/types";
import type { MateType, IKTarget } from "./assembly";
import type { FieldSkriptRecord } from "./fieldskript/record";
import type { TabKind } from "./project";
import {
  import3mf,
  export3mfRoundtrip,
  extractModelXml,
} from "./threemf";
import {
  type MeshOverlayData,
  canTriggerDownload,
  triggerDownload,
  parseStlBinary,
  parseObjText,
  buildObjExportText,
} from "./meshIO";

import {
  StoreCore,
  formatKernelError,
  loadFoldersFromStorage,
  persistFoldersToStorage,
  loadVisibleSketchesFromStorage,
  persistVisibleSketchesToStorage,
  loadHiddenDatumsFromStorage,
  persistHiddenDatumsToStorage,
  type FeatureFolder,
  type MeshedBody,
  type SelectionOrigin,
  type StoreState,
} from "./storeCore";
import {
  SketchStore,
  initialSketchToolParams,
} from "./sketchStore";
import { AssemblyStore } from "./assemblyStore";
import { ProjectStore } from "./projectStore";

// Re-export the shared store types from their new home so the many
// existing `import { StoreState } from "./store"` sites keep working.
export type {
  FeatureFolder,
  MeshedBody,
  SelectionOrigin,
  StoreState,
} from "./storeCore";
export type { SketchToolParams } from "./sketchStore";

const INACTIVE_SKETCH_SESSION: SketchSessionState = _INACTIVE;

export type { MeshOverlayData } from "./meshIO";

/** Map mesher-worker messages onto channel events. Other traffic on the
 *  shared worker (`fd_result` responses for the Outer API) is ignored. */
function parseMesherResponse(
  data: unknown,
): ChannelEvent<MesherBodyPayload[]> | null {
  const msg = data as MesherBodiesResponse | null | undefined;
  if (!msg || typeof msg !== "object" || msg.type !== "bodies") return null;
  return msg.ok
    ? { kind: "result", requestId: msg.requestId, value: msg.bodies }
    : { kind: "error", requestId: msg.requestId, error: msg.error };
}

/** Map weaver-worker messages onto channel events. */
function parseWeaverResponse(
  data: unknown,
): ChannelEvent<WeaveBrepPayload[]> | null {
  const msg = data as WeaveBrepsResponse | null | undefined;
  if (!msg || typeof msg !== "object" || msg.type !== "breps") return null;
  return msg.ok
    ? { kind: "result", requestId: msg.requestId, value: msg.items }
    : { kind: "error", requestId: msg.requestId, error: msg.error };
}

export class KernelStore {
  readonly kernel: Kernel;
  /** Shared foundation: state snapshot, pub-sub, commit() tail, autosave.
   *  The facade (and every domain controller) mutates state through it. */
  private readonly core: StoreCore;
  /** Sketch domain controller — see sketchStore.ts. */
  private readonly sketch: SketchStore;
  /** Assembly domain controller — see assemblyStore.ts. */
  private readonly assembly: AssemblyStore;
  /** Project domain controller — see projectStore.ts. */
  private readonly project: ProjectStore;

  /** The current immutable state snapshot lives in the core. */
  private get state(): StoreState {
    return this.core.state;
  }
  private set state(next: StoreState) {
    this.core.state = next;
  }
  private patch(partial: Partial<StoreState>): void {
    this.core.patch(partial);
  }

  private meshDataCache = new Map<number, MeshOverlayData>();
  /** Latest-wins channel to the mesher worker: rapid-fire mutations
   *  coalesce into one follow-up rebuild built against the freshest
   *  document; stale responses are dropped; worker errors surface via
   *  `reportWorkerError` instead of masking as an empty mesh set. */
  private mesherChannel: CoalescingWorkerChannel<MesherBodyPayload[]> | null =
    null;
  /** Fallback mode: when the Worker constructor throws (e.g. the
   *  bundler didn't emit the worker chunk), we keep sync meshing on
   *  the main-thread kernel and drive the meshedBodies cache from
   *  its own `body*` views. Loses the off-thread benefit but keeps
   *  the app rendering. */
  private workerAvailable = false;
  private dcMeshingEnabled = true;

  /** B-Rep weaver worker (third kernel instance). While it's up, the
   *  main kernel's B-Rep exports stay PERMANENTLY suppressed
   *  (cache-or-empty, never a synchronous main-thread weave); after
   *  every commit the stale bodies are farmed out here and the woven
   *  results come back as MessagePack buffers for `importBodyBrep`.
   *  Same latest-wins channel semantics as the mesher. */
  private weaverChannel: CoalescingWorkerChannel<WeaveBrepPayload[]> | null =
    null;
  private weaverAvailable = false;
  /** Mirror of the parametric-weave setting for weave requests. */
  private parametricWeaveEnabled = true;

  constructor(
    kernel: Kernel,
    autosave: AutoSaveHandle,
    persistedProjectJson: string | null = null,
  ) {
    this.kernel = kernel;
    this.core = new StoreCore(kernel, autosave);
    // Late-bound commit hooks: the domain-specific pieces of the shared
    // commit() tail (see StoreCoreHooks). Bound as closures so they always
    // dispatch against the live facade methods.
    this.core.setHooks({
      beforeCommit: (nextSelection) => {
        if (
          this.pendingFeature !== null &&
          nextSelection !== undefined &&
          !nextSelection.has(this.pendingFeature)
        ) {
          this.confirmPendingInline();
        }
      },
      syncActiveBody: () => this.syncActiveBody(),
      serializeProject: () => this.project.serializeProject(),
      refreshMeshCache: () => this.refreshMeshCache(),
      scheduleWeave: () => this.scheduleWeave(),
      snapshotScriptTypes: () => this.project.snapshotScriptTypes(),
    });
    // cancelSketch drops the sketch feature created on session entry —
    // feature removal stays a facade concern, injected as a dep.
    this.sketch = new SketchStore(this.core, {
      removeFeature: (id) => this.removeFeature(id),
    });
    // exitAssembly switches back to the first part tab — tab management
    // stays a project-domain concern, injected as a dep.
    this.assembly = new AssemblyStore(this.core, {
      setActiveTab: (id) => this.setActiveTab(id),
    });
    this.project = new ProjectStore(this.core);

    // Bring up the mesher worker BEFORE the initial document load: with the
    // worker available, sync meshing is disabled first, so `loadJson` below
    // neither runs the Dual-Contouring pass on the main thread nor enters
    // the kernel's mesh face-tagging loop — which would weave every body's
    // B-Rep synchronously and freeze the tab for minutes on weave-heavy
    // projects (spline-revolve + shell). On failure, sync meshing stays on
    // (functionally correct, just blocks the UI on every mesh rebuild).
    try {
      const mesher = new Worker(
        new URL("./mesher.worker.ts", import.meta.url),
        { type: "module" },
      );
      this.mesherChannel = new CoalescingWorkerChannel<MesherBodyPayload[]>({
        worker: mesher,
        parse: parseMesherResponse,
        buildRequest: (requestId, pass) =>
          this.buildMesherRequest(requestId, pass),
        onResult: (bodies) => this.applyMesherBodies(bodies),
        onError: (error) => this.reportWorkerError("Mesh rebuild", error),
        // M13: progressive refinement — the initial pass runs at coarse
        // resolution; its response auto-posts a full-resolution follow-up.
        followUpAfterInitial: true,
      });
      this.workerAvailable = true;
      this.kernel.setSyncMeshing(false);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn(
        "fielddraft: could not start mesher worker, falling back to sync meshing on the main thread",
        err,
      );
      this.workerAvailable = false;
    }

    // Bring up the B-Rep weaver worker. With it available, the main
    // kernel never weaves synchronously for an export again: exports are
    // permanently suppressed to cache-or-empty, and `scheduleWeave`
    // (called after every commit) keeps the caches fresh off-thread.
    // On failure the legacy behaviour remains: overlays weave
    // synchronously on first fetch.
    try {
      const weaver = new Worker(
        new URL("./weaver.worker.ts", import.meta.url),
        { type: "module" },
      );
      this.weaverChannel = new CoalescingWorkerChannel<WeaveBrepPayload[]>({
        worker: weaver,
        parse: parseWeaverResponse,
        buildRequest: (requestId) => this.buildWeaveRequest(requestId),
        onResult: (items) => this.applyWovenBreps(items),
        onError: (error) => this.reportWorkerError("B-Rep weave", error),
      });
      this.weaverAvailable = true;
      this.kernel.setBrepSuppressed(true);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn(
        "fielddraft: could not start weaver worker, falling back to synchronous B-Rep weaving",
        err,
      );
      this.weaverAvailable = false;
    }

    // ── MT-A: build the project + load the active/first part into the
    //   kernel before reading the initial feature list. ──────────────
    const { project, initialAssembly } =
      this.project.bootstrap(persistedProjectJson);

    const initFeatures = kernel.listFeatures();
    this.state = {
      features: initFeatures,
      selection: new Set(),
      selectionAnchor: null,
      selectedElement: null,
      selectedElements: [],
      canUndo: kernel.canUndo(),
      undoDepth: kernel.undoDepth(),
      canRedo: kernel.canRedo(),
      redoDepth: kernel.redoDepth(),
      nextOp: BoolOpTag.New,
      meshedBodies: [],
      brepVersion: 0,
      measureMode: false,
      measureSelection: [],
      filletMode: false,
      thickenMode: false,
      sketchToolParams: initialSketchToolParams(),
      syncState: "offline",
      projectId: null,
      sketchSession: INACTIVE_SKETCH_SESSION,
      variables: kernel.listVariables(),
      referenceMesh: null,
      sceneSize: [0, 0, 0],
      folders: loadFoldersFromStorage(),
      visibleSketches: loadVisibleSketchesFromStorage(),
      transientError: null,
      mateMode: false,
      mateConnectorsHidden: false,
      assembly: initialAssembly,
      ikMode: false,
      gizmoMode: "translate" as const,
      gizmoArmed: false,
      gizmoDragSuppressed: false,
      threemfStash: null,
      loading: null,
      hiddenDatums: loadHiddenDatumsFromStorage(),
      selectedDatum: null,
      highlightedBodyIndex: null,
      rollbackIndex: initFeatures.length,
      project,
      scriptTypes: this.project.snapshotScriptTypes(),
    };

    if (this.workerAvailable) {
      // Kick off the first worker rebuild so the initial document
      // (possibly rehydrated from localStorage) becomes visible.
      this.scheduleWorkerRebuild();
    } else {
      // Sync-meshing fallback: read body buffers from the main-thread
      // kernel directly (the loadJson above already meshed them).
      this.state = {
        ...this.state,
        meshedBodies: this.snapshotMainThreadBodies(),
        sceneSize: this.kernel.sceneAabb().size as [number, number, number],
      };
    }
    // First async weave for the freshly loaded document — the overlays
    // mount against empty exports and refetch when the imports land.
    this.scheduleWeave();
  }

  // ── pub-sub ───────────────────────────────────────────────────────

  subscribe = (listener: () => void): (() => void) =>
    this.core.subscribe(listener);

  getState = (): StoreState => this.core.state;

  /** Shared post-mutation tail — see {@link StoreCore.commit}. */
  private commit(
    nextSelection?: ReadonlySet<number>,
    nextAnchor?: number | null,
  ): void {
    this.core.commit(nextSelection, nextAnchor);
  }


  /**
   * Pull the latest meshes into `state.meshedBodies`. In worker mode
   * this posts a rebuild request and waits for the response
   * asynchronously; in fallback mode (no worker) it snapshots the
   * main-thread kernel's already-synced body buffers right now.
   * Callers are responsible for notifying listeners after — the
   * worker path notifies from inside `applyMesherBodies`, the
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

  private meshAlgorithm: string = "mdc";

  setDcMeshingEnabled(enabled: boolean, algorithm?: string): void {
    const algoChanged =
      algorithm !== undefined && algorithm !== this.meshAlgorithm;
    if (algorithm !== undefined) this.meshAlgorithm = algorithm;
    if (this.dcMeshingEnabled === enabled && !algoChanged) return;
    this.dcMeshingEnabled = enabled;
    if (enabled) {
      this.scheduleWorkerRebuild();
    }
  }

  /** Milestone-1 parametric weaving toggle. Only the woven B-Rep changes (the
   * DC render mesh is identical), so we just flip the kernel flag and notify —
   * the kernel bumps each body's version, which refetches the B-Rep edge /
   * face overlays. In weaver mode the cleared caches re-weave off-thread. */
  setParametricWeave(enabled: boolean): void {
    this.parametricWeaveEnabled = enabled;
    if (this.kernel.setParametricWeave(enabled)) {
      this.scheduleWeave();
      this.notify();
    }
  }

  /** Setting "B-Rep rebuild while dragging" (default false): when off, a
   * gizmo drag suppresses every B-Rep weave for its whole duration — the
   * throttled drag commits only re-mesh, and the exact B-Rep weaves once
   * on release (`endGizmoDrag`). When on, drags keep the historical
   * live-weave behaviour. */
  private dragBrepRecompute = false;
  private gizmoDragActive = false;

  setDragBrepRecompute(enabled: boolean): void {
    this.dragBrepRecompute = enabled;
    // Flipping the setting ON mid-drag lifts an active suppression.
    if (enabled && this.gizmoDragActive && this.state.gizmoDragSuppressed) {
      if (this.weaverAvailable) {
        this.scheduleWeave();
      } else {
        this.kernel.setBrepSuppressed(false);
      }
      this.patch({ gizmoDragSuppressed: false });
      this.notify();
    }
  }

  /** Setting "Live extrude drag" (default true): the extrude depth gizmo
   * commits the depth throttled WHILE the pointer is down — the renderer
   * shows the real body live instead of an approximate preview prism.
   * The B-Rep weave stays governed by `dragBrepRecompute` via
   * begin/endGizmoDrag, exactly like the transform gizmo. */
  private liveExtrudeDrag = true;

  setLiveExtrudeDrag(enabled: boolean): void {
    this.liveExtrudeDrag = enabled;
  }

  isLiveExtrudeDrag(): boolean {
    return this.liveExtrudeDrag;
  }

  /** Called by the transform gizmo when a drag starts (dragging-changed).
   * `gizmoDragSuppressed` hides all B-Rep overlays — even cached
   * (pure-move) edges only refresh at the 50ms commit cadence and would
   * visibly trail the drag. With the weaver worker up the main kernel is
   * permanently suppressed anyway and `scheduleWeave` skips scheduling
   * for the drag's duration; the legacy (no-weaver) path additionally
   * toggles the kernel suppression flag. */
  beginGizmoDrag(): void {
    this.gizmoDragActive = true;
    if (!this.dragBrepRecompute) {
      if (!this.weaverAvailable) this.kernel.setBrepSuppressed(true);
      this.patch({ gizmoDragSuppressed: true });
      this.notify();
    }
  }

  /** Called when the drag ends (or the gizmo unmounts mid-drag). Weaver
   * mode schedules the off-thread catch-up weave; the legacy path
   * un-suppresses the kernel, which weaves each stale body synchronously
   * once and bumps its version so overlays refetch. Idempotent. */
  endGizmoDrag(): void {
    if (!this.gizmoDragActive) return;
    this.gizmoDragActive = false;
    if (this.weaverAvailable) {
      this.scheduleWeave();
      if (this.state.gizmoDragSuppressed) {
        this.patch({ gizmoDragSuppressed: false });
        this.notify();
      }
      return;
    }
    const kernelChanged = this.kernel.setBrepSuppressed(false);
    if (this.state.gizmoDragSuppressed) {
      this.patch({ gizmoDragSuppressed: false });
      this.notify();
    } else if (kernelChanged) {
      this.notify();
    }
  }

  /** Hand off the current document to the mesher worker. Coalescing,
   *  latest-wins and the coarse→refine ladder live in the channel;
   *  this is just the trigger. */
  private scheduleWorkerRebuild(): void {
    if (!this.dcMeshingEnabled) return;
    this.mesherChannel?.schedule();
  }

  /** Channel callback: build the freshest rebuild request. The initial
   *  pass of a burst runs at coarse (half) resolution for a fast first
   *  paint (M13); the automatic follow-up refines at full resolution. */
  private buildMesherRequest(
    requestId: number,
    pass: "initial" | "followup",
  ): OutboundRequest | null {
    if (!this.dcMeshingEnabled) return null;
    const fullRes = this.kernel.meshResolution();
    const coarseRes = Math.max(8, Math.floor(fullRes / 2));
    const request: MesherRebuildRequest = {
      type: "rebuild",
      requestId,
      documentJson: this.kernel.toJson(),
      activeMeshResolution: pass === "initial" ? coarseRes : fullRes,
      contextMeshResolution: this.kernel.contextMeshResolution(),
      activeBodyId: this.kernel.activeBodyId(),
      meshAlgorithm: this.meshAlgorithm,
    };
    return { message: request };
  }

  /** Channel callback: publish the latest mesh set from the worker. */
  private applyMesherBodies(bodies: MesherBodyPayload[]): void {
    this.state = {
      ...this.state,
      meshedBodies: bodies,
      sceneSize: this.kernel.sceneAabb().size as [number, number, number],
    };
    this.core.updateSceneBounds();
    this.core.emit();
  }

  /** Channel callback: a worker job failed. Surface it — log + transient
   *  error banner — instead of masking it as an empty result (which used
   *  to silently blank the viewport / drop the weave). */
  private reportWorkerError(job: string, error: string): void {
    // eslint-disable-next-line no-console
    console.error(`fielddraft: ${job} failed in worker: ${error}`);
    this.patch({ transientError: `${job} failed: ${error}` });
    this.notify();
  }

  /**
   * Farm the stale B-Rep weaves out to the weaver worker. Rapid-fire
   * commits coalesce exactly like mesh rebuilds (latest-wins channel).
   * During a suppressed gizmo drag `buildWeaveRequest` returns null, so
   * nothing is posted — `endGizmoDrag` runs the catch-up schedule.
   */
  private scheduleWeave(): void {
    this.weaverChannel?.schedule();
  }

  /** Channel callback: build the freshest weave request, or null when
   *  there is nothing to weave right now (no stale bodies, or a
   *  suppressed gizmo drag is running). */
  private buildWeaveRequest(requestId: number): OutboundRequest | null {
    if (this.gizmoDragActive && !this.dragBrepRecompute) return null;
    const stale = this.kernel.bodyBrepStaleIds();
    if (stale.length === 0) return null;
    const request: WeaveRequest = {
      type: "weave",
      requestId,
      documentJson: this.kernel.toJson(),
      parametricWeave: this.parametricWeaveEnabled,
      bodyIds: stale,
    };
    return { message: request };
  }

  /** Channel callback: install freshly woven B-Reps into the main kernel. */
  private applyWovenBreps(items: WeaveBrepPayload[]): void {
    let imported = false;
    for (const item of items) {
      // Validated against the CURRENT placement-invariant key — a
      // result that raced a newer edit misses here; the channel's
      // queued re-schedule covers it against the latest document.
      if (this.kernel.importBodyBrep(item.id, item.bytes)) imported = true;
    }
    if (imported) {
      // Publish a fresh snapshot so `useSyncExternalStore` actually re-renders
      // — `notify()` alone leaves the `getState()` reference unchanged, and the
      // viewport's edge overlay would keep its stale empty memo (the freshly
      // extruded body would render without its B-Rep edges until an unrelated
      // state change). The kernel already bumped each body's version on import;
      // a new state reference is all the overlays need to refetch.
      this.patch({ brepVersion: this.state.brepVersion + 1 });
      this.notify();
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

  /** Public accessor for the live project as a `.fdraft` ZIP — see
   *  {@link ProjectStore.exportProjectFdraft} (bug-report flow, on3k). */
  exportProjectFdraft(): Uint8Array {
    return this.project.exportProjectFdraft();
  }

  /**
   * A JSON-serializable snapshot of the *ephemeral UI state* — what the user
   * is looking at and interacting with, which the project file (`.fdraft`)
   * deliberately does NOT carry: viewport camera, selection, active tool /
   * mode, sketch-session context, view toggles. Attached to bug reports
   * (ticket uz5q) as `ui_state.json` so a report reproduces the exact view,
   * not just the document.
   *
   * Curated on purpose: kernel-derived data (features, meshes, reference
   * mesh) is redundant with the project and huge, so it's excluded. Sets are
   * flattened to arrays for JSON.
   */
  uiStateSnapshot(): Record<string, unknown> {
    const s = this.state;
    const cam = getPublishedPickCamera() as unknown as {
      type: string;
      position: { toArray(): number[] };
      quaternion: { toArray(): number[] };
      fov?: number;
      zoom?: number;
      near?: number;
      far?: number;
    } | null;
    const camera = cam
      ? {
          type: cam.type,
          position: cam.position.toArray(),
          quaternion: cam.quaternion.toArray(),
          fov: cam.fov ?? null,
          zoom: cam.zoom ?? null,
          near: cam.near ?? null,
          far: cam.far ?? null,
        }
      : null;
    return {
      camera,
      selection: [...s.selection],
      selectionAnchor: s.selectionAnchor,
      selectedElement: s.selectedElement,
      selectedElements: s.selectedElements,
      selectedDatum: s.selectedDatum,
      highlightedBodyIndex: s.highlightedBodyIndex,
      nextOp: s.nextOp,
      modes: {
        measure: s.measureMode,
        fillet: s.filletMode,
        thicken: s.thickenMode,
        mate: s.mateMode,
        ik: s.ikMode,
      },
      sketchSession: {
        active: s.sketchSession.active,
        editingFeatureId: s.sketchSession.editingFeatureId,
        selectedTool: s.sketchSession.selectedTool,
        selection: [...s.sketchSession.selection],
      },
      visibleSketches: [...s.visibleSketches],
      folders: s.folders,
      rollbackIndex: s.rollbackIndex,
      variables: s.variables,
      sceneSize: s.sceneSize,
      syncState: s.syncState,
      projectId: s.projectId,
      activeTabId: this.state.project.activeTabId,
      transientError: s.transientError,
    };
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
    this.patch({ nextOp: op });
    this.core.emit();
  }

  // ── K9.0: STL reference import ──────────────────────────────

  async loadReferenceMesh(buffer: ArrayBuffer): Promise<boolean> {
    this.setLoading("Parsing STL…");
    await new Promise((r) => setTimeout(r, 0));
    const mesh = parseStlBinary(buffer);
    if (!mesh) {
      this.setLoading(null);
      return false;
    }
    this.setLoading("Building SDF from mesh…");
    await new Promise((r) => setTimeout(r, 0));
    const id = this.importMeshAsFeature(
      mesh.positions,
      mesh.normals,
      mesh.indices,
    );
    this.setLoading(null);
    return id !== null;
  }

  /** K9.2: load a GLB/glTF file as a visual reference mesh.
   *  Uses Three.js GLTFLoader to parse, merges all mesh
   *  geometries into one combined referenceMesh. */
  async loadReferenceGlb(buffer: ArrayBuffer): Promise<boolean> {
    const t0 = performance.now();
    console.log(
      `fielddraft: importing GLB reference — ${(buffer.byteLength / 1024).toFixed(0)} KB`,
    );
    this.setLoading("Loading GLB file…");
    try {
      const { GLTFLoader } =
        await import("three/examples/jsm/loaders/GLTFLoader.js");
      const loader = new GLTFLoader();
      const gltf = await new Promise<{
        scene: { traverse: (cb: (child: unknown) => void) => void };
      }>((resolve, reject) => {
        loader.parse(buffer, "", resolve as never, reject);
      });

      type GeoLike = {
        getAttribute: (
          n: string,
        ) => { array: ArrayLike<number>; count: number } | null;
        getIndex: () => { array: ArrayLike<number> } | null;
      };
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
          for (let i = 0; i < rawIdx.length; i++)
            offset[i] = rawIdx[i] + vertexOffset;
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

      console.log(
        `fielddraft: GLB parsed — ${allPos.length} mesh(es), ${vertexOffset} vertices in ${(performance.now() - t0).toFixed(0)} ms`,
      );
      this.setLoading("Building SDF from mesh…");
      await new Promise((r) => setTimeout(r, 0));
      const id = this.importMeshAsFeature(positions, normals, indices);
      this.setLoading(null);
      return id !== null;
    } catch (err) {
      this.setLoading(null);
      console.error("fielddraft: GLB import failed", err);
      return false;
    }
  }

  async loadReferenceObj(text: string): Promise<boolean> {
    const mesh = parseObjText(text);
    if (!mesh) return false;
    this.setLoading("Building SDF from mesh…");
    await new Promise((r) => setTimeout(r, 0));
    const id = this.importMeshAsFeature(
      mesh.positions,
      mesh.normals,
      mesh.indices,
    );
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
    console.log(
      `fielddraft: converting reference mesh to solid — ${triCount} triangles, building BVH…`,
    );
    let id: number;
    try {
      id = this.kernel.addImportedMesh(
        ref_.positions,
        ref_.indices,
        BoolOpTag.New,
      );
    } catch (err) {
      console.error("fielddraft: convertReferenceToSolid failed", err);
      return null;
    }
    this.patch({ referenceMesh: null });
    this.commit(new Set([id]), id);
    console.log(
      `fielddraft: reference → solid complete — feature #${id} in ${(performance.now() - t0).toFixed(0)} ms`,
    );
    return id;
  }

  clearReferenceMesh(): void {
    this.patch({ referenceMesh: null });
    this.notify();
  }

  importMeshAsFeature(
    positions: Float32Array,
    normals: Float32Array,
    indices: Uint32Array,
  ): number | null {
    const t0 = performance.now();
    const triCount = indices.length / 3;
    console.log(
      `fielddraft: importing mesh as SDF feature — ${triCount} triangles, building BVH…`,
    );
    let id: number;
    try {
      id = this.kernel.addImportedMesh(positions, indices, BoolOpTag.New);
    } catch (err) {
      console.error("fielddraft: importMeshAsFeature failed", err);
      return null;
    }
    this.meshDataCache.set(id, { positions, normals, indices });
    this.commit(new Set([id]), id);
    console.log(
      `fielddraft: mesh imported as feature #${id} (solid) in ${(performance.now() - t0).toFixed(0)} ms`,
    );
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

  /** K8.10 / 8ar9: upsert a variable and commit. Returns `true` on
   *  success, `false` when the kernel rejected the call (empty name,
   *  NaN, Infinity). `kind` defaults to "" = keep existing / number. */
  setVariable(name: string, value: number, kind: VarKind | "" = ""): boolean {
    try {
      this.kernel.setVariable(name, value, kind);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: setVariable failed", err);
      return false;
    }
    this.commit();
    return true;
  }

  /**
   * Tickets buy9 + nlij: upsert a variable from the single smart value field.
   * The raw text may be a plain number (`42`), a number with a unit (`10 cm`,
   * inferred as Length), an angle (`90 deg`), or a formula referencing other
   * variables and math functions (`sin(Breite) * 2`). The dimension is inferred
   * from the unit / referenced variables — there is no kind selector and no
   * `ƒx` toggle.
   *
   * The whole variable table (with this edit applied) is resolved web-side
   * with cycle detection; the resolved numeric value + raw source text are then
   * pushed to the kernel, so a `=name` feature formula keeps reading a plain
   * number. Dependents are re-resolved too. Returns an error string on a parse
   * / resolution / cycle failure (which is surfaced inline and leaves the table
   * untouched), or `null` on success.
   */
  setVariableFromInput(name: string, raw: string): string | null {
    const trimmed = name.trim();
    if (trimmed.length === 0) return "name must not be empty";

    // Build the prospective table: existing variables + this edit. A bare
    // identifier can reference any *other* declared name (plus the one being
    // edited is allowed to be referenced by others, but not itself).
    const existing = this.kernel.listVariables();
    const known = new Set(existing.map((v) => v.name));
    known.add(trimmed);

    const parse = parseVariableInput(raw, known);
    if (!parse.ok) return parse.error;

    const draft: RawVariable[] = existing.map((v) => ({
      name: v.name,
      value: v.value,
      kind: v.kind,
      formula: v.formula,
    }));
    const next: RawVariable = parse.isFormula
      ? { name: trimmed, value: NaN, kind: parse.kind, formula: parse.source }
      : {
          name: trimmed,
          value: parse.value,
          kind: parse.kind,
          formula: parse.source,
        };
    const idx = draft.findIndex((v) => v.name === trimmed);
    if (idx >= 0) draft[idx] = next;
    else draft.push(next);

    const { resolved, errors } = resolveVariableTable(draft);
    const ownErr = errors.get(trimmed);
    if (ownErr) return ownErr;
    // A cycle / break introduced elsewhere by this edit also blocks the commit.
    if (errors.size > 0) {
      const [first] = [...errors.entries()];
      return `${first[0]}: ${first[1]}`;
    }

    // Push every (possibly re-resolved) variable into the kernel. Pushing the
    // full set keeps dependents current after a referenced variable changes.
    try {
      for (const r of resolved.values()) {
        this.kernel.setVariable(r.name, r.value, r.kind, r.formula);
      }
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: setVariableFromInput failed", err);
      return formatKernelError(err, "variable update failed");
    }
    this.commit();
    return null;
  }

  /** K8.10: remove a variable and commit. */
  removeVariable(name: string): boolean {
    if (!this.kernel.removeVariable(name)) return false;
    this.commit();
    return true;
  }

  /** Ticket 8ar9: rename a variable, keeping its value, and commit.
   *  Returns `true` on success, `false` when the kernel rejected the
   *  rename (empty new name or a collision). A no-op rename succeeds. */
  renameVariable(oldName: string, newName: string): boolean {
    if (oldName === newName) return true;
    try {
      this.kernel.renameVariable(oldName, newName);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: renameVariable failed", err);
      this.setTransientError(formatKernelError(err, "Variable rename failed"));
      return false;
    }
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
      id = this.kernel.addThread(BoolOpTag.New, offset, radius, pitch, depth);
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

  /** Add a `shell` feature and select it so its dialog opens. Faces to open
   *  and the wall thickness are picked in the dialog (schema-driven); the
   *  feature emits no geometry until at least one face (planar or curved)
   *  is chosen. */
  addShell(thicknessMm = 2): number {
    let id: number;
    try {
      id = this.kernel.addShell("", thicknessMm);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: addShell failed", err);
      return -1;
    }
    this.commit(new Set([id]), id);
    return id;
  }

  addBoolean(): number {
    let id: number;
    try {
      id = this.kernel.addBoolean();
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: addBoolean failed", err);
      return -1;
    }
    this.commit(new Set([id]), id);
    return id;
  }

  addMaterial(): number {
    let id: number;
    try {
      id = this.kernel.addMaterial();
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: addMaterial failed", err);
      return -1;
    }
    this.commit(new Set([id]), id);
    return id;
  }

  addHole(diameterMm = 6, depthMm = 40): number {
    logUserAction("feature", "add", { kind: "hole" });
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
    op: BoolOpTagValue = BoolOpTag.New,
    translate?: [number, number, number],
  ): number {
    logUserAction("feature", "add", { kind });
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
        if (pid == null)
          throw new Error("linear_pattern: no valid source feature");
        return pid;
      }
      case "circular_pattern": {
        const pid = this.addCircularPatternOfLatest();
        if (pid == null)
          throw new Error("circular_pattern: no valid source feature");
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
        // Ticket vev1: also close the sketch view when the edited sketch is
        // among the deleted selection.
        if (
          this.state.sketchSession.active &&
          this.state.sketchSession.editingFeatureId === id
        ) {
          this.exitSketch();
        }
        if (this.pendingFeature === id) this.pendingFeature = null;
        this.meshDataCache.delete(id);
        removed = true;
      }
    }
    if (removed) this.commit(new Set(), null);
    return removed;
  }

  removeFeature(id: number): boolean {
    if (!this.kernel.removeFeature(id)) return false;
    // Ticket vev1: deleting the sketch that is currently open for editing
    // must close the sketch view — otherwise the session lingers, showing a
    // now-deleted sketch. exitSketch() fully clears the session (active=false,
    // subCalls empty, editingFeatureId null) — see reduceExit (ticket 5zd3).
    if (
      this.state.sketchSession.active &&
      this.state.sketchSession.editingFeatureId === id
    ) {
      this.exitSketch();
    }
    if (this.pendingFeature === id) this.pendingFeature = null;
    this.meshDataCache.delete(id);
    const next = new Set(this.state.selection);
    next.delete(id);
    const nextAnchor =
      this.state.selectionAnchor === id ? null : this.state.selectionAnchor;
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

  // ── sketch-session undo/redo (ticket r3ci) — see SketchStore ─────

  sketchUndo(): boolean {
    return this.sketch.sketchUndo();
  }

  sketchRedo(): boolean {
    return this.sketch.sketchRedo();
  }

  // ── pending feature (ticket x7af) ─────────────────────────────────
  //
  // A freshly-created extrude that still has its detail dialog open
  // (ticket x7af). Since yx5h the feature is UNSUPPRESSED — the real body
  // is woven and rendered solid right away, no translucent ghost. "Pending"
  // is now purely a UI lifecycle marker: Cancel drops the feature, and
  // confirmation (dialog OK, a click elsewhere, or the creation of another
  // feature) just clears this flag.

  private pendingFeature: number | null = null;

  /** Is `id` the freshly-created, dialog-open extrude? The extrude gizmo
   *  uses this to know it is still in the create/edit lifecycle. */
  isPendingFeature(id: number): boolean {
    return this.pendingFeature === id;
  }

  /** Clear the pending flag WITHOUT committing — shared by
   *  {@link confirmPendingFeature} and `commit()` (which refreshes state
   *  itself right after). The feature is already unsuppressed and rendered
   *  (ticket yx5h), so confirming just ends its edit lifecycle. */
  private confirmPendingInline(): void {
    this.pendingFeature = null;
  }

  /** Confirm the pending extrude: end its edit lifecycle and refresh. */
  confirmPendingFeature(): void {
    if (this.pendingFeature === null) return;
    this.confirmPendingInline();
    this.commit();
  }

  /** Cancel the pending extrude: remove the feature it created. */
  cancelPendingFeature(): void {
    const id = this.pendingFeature;
    if (id === null) return;
    this.pendingFeature = null;
    this.removeFeature(id);
  }

  // ── selection ─────────────────────────────────────────────────────

  selectOnly(id: number, origin: SelectionOrigin = "list"): void {
    logUserAction("feature", "select", { id, origin });
    if (id !== this.pendingFeature) this.confirmPendingFeature();
    this.state = {
      ...this.state,
      selection: new Set([id]),
      selectionAnchor: id,
      // Feature selection and element selection are mutually exclusive.
      selectedElement: null,
      selectedElements: [],
      selectedDatum: null,
    };
    this.syncActiveBody();
    this.refreshMeshCache();
    this.syncSketchSessionToSelection(origin);
    this.core.emit();
  }

  toggleSelect(id: number, origin: SelectionOrigin = "list"): void {
    if (id !== this.pendingFeature) this.confirmPendingFeature();
    const next = new Set(this.state.selection);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    this.state = {
      ...this.state,
      selection: next,
      selectionAnchor: id,
      selectedElement: null,
      selectedElements: [],
      selectedDatum: null,
    };
    this.syncActiveBody();
    this.refreshMeshCache();
    this.syncSketchSessionToSelection(origin);
    this.core.emit();
  }

  /**
   * Shift+Click range selection: select every feature between the
   * current anchor (inclusive) and the clicked id (inclusive). If no
   * anchor exists yet this falls back to a single selection.
   */
  selectRangeTo(id: number, origin: SelectionOrigin = "list"): void {
    if (id !== this.pendingFeature) this.confirmPendingFeature();
    const features = this.state.features;
    const targetIdx = features.findIndex((f) => f.id === id);
    if (targetIdx === -1) return;

    const anchor = this.state.selectionAnchor;
    if (anchor === null) {
      this.selectOnly(id, origin);
      return;
    }
    const anchorIdx = features.findIndex((f) => f.id === anchor);
    if (anchorIdx === -1) {
      // Anchor stale; reset.
      this.selectOnly(id, origin);
      return;
    }
    const [lo, hi] =
      anchorIdx <= targetIdx ? [anchorIdx, targetIdx] : [targetIdx, anchorIdx];
    const next = new Set<number>();
    for (let i = lo; i <= hi; i++) next.add(features[i].id);
    this.state = {
      ...this.state,
      selection: next,
      // Anchor stays put — that's the standard finder/Onshape behaviour.
      selectionAnchor: anchor,
      selectedElement: null,
      selectedElements: [],
      selectedDatum: null,
    };
    this.syncActiveBody();
    this.refreshMeshCache();
    this.syncSketchSessionToSelection(origin);
    this.core.emit();
  }

  highlightBody(index: number | null): void {
    this.patch({ highlightedBodyIndex: index });
    this.core.emit();
  }

  // ── project persistence & server sync — see ProjectStore ──────────

  saveProjectToFile(filename = "project.fdraft"): void {
    this.project.saveProjectToFile(filename);
  }

  loadProjectFromJson(json: string): boolean {
    return this.project.loadProjectFromJson(json);
  }

  loadProjectFromBytes(bytes: Uint8Array | ArrayBuffer): boolean {
    return this.project.loadProjectFromBytes(bytes);
  }

  setProjectId(id: string | null): void {
    this.project.setProjectId(id);
  }

  serializeProjectJson(): string {
    return this.project.serializeProjectJson();
  }

  loadServerProject(id: string, documentJson: string): boolean {
    return this.project.loadServerProject(id, documentJson);
  }

  detachServerProject(): void {
    this.project.detachServerProject();
  }

  syncToServer(): Promise<boolean> {
    return this.project.syncToServer();
  }

  newProject(): void {
    this.project.newProject();
  }

  /**
   * Add a mate connector anchored to a picked B-Rep element. `anchor` is an
   * element query (`qFace`/`qEdge`/`qVertex`) and defines +Z. `opts.t` is the
   * edge position (midpoint = 0.5); `opts.secondary` is the reference FACE
   * query orienting the secondary axes / defining +Z for a vertex (empty →
   * auto); `opts.reorient` rotates the secondary axes ×90° about +Z (0..3);
   * `opts.flip` reverses +Z. The kernel resolves the CSYS during regen (C2).
   */
  addMateConnectorAt(
    anchor: string,
    opts: {
      t?: number;
      secondary?: string;
      reorient?: number;
      flip?: boolean;
    } = {},
  ): number | null {
    let id: number;
    try {
      id = this.kernel.addMateConnector(
        anchor,
        opts.t ?? 0.5,
        opts.secondary ?? "",
        opts.reorient ?? 0,
        opts.flip ?? false,
      );
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: addMateConnector failed", err);
      return null;
    }
    this.commit(new Set([id]), id);
    return id;
  }

  /**
   * DL-61: add a `fillet_edge` feature selecting an edge by query
   * (e.g. `qEdge(<idHash>)`). The feature pair and the SMin mask tube are
   * derived from the resolved edge during regeneration.
   *
   * No ghost-edge reorder workaround is needed any more: fillets emit no
   * DecodedFeatures, so `brep_cache.ensure` always weaves the UN-filleted
   * geometry — every fillet's edge query therefore resolves against the
   * original edges regardless of fillet order.
   */
  addFilletAtEdge(edgeQuery: string, radius = 3): number | null {
    let id: number;
    try {
      id = this.kernel.addFilletEdge(edgeQuery, radius);
    } catch (err) {
      console.error("fielddraft: addFilletAtEdge failed", err);
      this.setTransientError(formatKernelError(err, "Fillet failed"));
      return null;
    }
    this.commit(new Set([id]), id);
    return id;
  }

  /** Extrude one or more coplanar planar faces (P1). `faceQueries` are
   *  `qFace(<idHash>)` strings (one per selected face); they are joined into a
   *  single multi-query and swept `depthMm` blind (outward from the face, the
   *  Onshape default; 25 mm per ticket jn3t). Depth/mode stay editable
   *  afterwards via the feature dialog. Returns the new feature id, or `null`
   *  on a kernel error (e.g. the faces are not coplanar). */
  addExtrudeFaces(faceQueries: string[], depthMm = 25): number | null {
    // An EMPTY selection is allowed (ticket 1rax): the feature is created
    // with no profile and the dialog's query picker fills it afterwards.
    let id: number;
    try {
      // Created as a PENDING feature (ticket x7af) but UNSUPPRESSED
      // (ticket yx5h): the real extruded body weaves and renders solid
      // straight away instead of a translucent preview prism. The feature
      // stays "pending" only for the dialog lifecycle (gizmo edits it,
      // Cancel drops it, OK / deselect clears the flag).
      id = this.kernel.addExtrudeFaces(
        faceQueries.join(","),
        depthMm,
        false,
        0,
        false,
      );
    } catch (err) {
      console.error("fielddraft: addExtrudeFaces failed", err);
      this.setTransientError(formatKernelError(err, "Extrude failed"));
      return null;
    }
    this.pendingFeature = id;
    this.commit(new Set([id]), id);
    return id;
  }

  /** Thicken curved face(s) into a thin wall. Closed non-analytic faces
   *  (revolved / swept blobs) use the general SDF field-offset band; analytic
   *  faces (sphere / cylinder / torus / cone) fold to exact surfaces. Outward
   *  (Onshape default) by `thicknessMm`; the kernel rejects planar faces and
   *  composite (boolean) non-analytic carriers with a helpful message. Passing
   *  `upToFaceQuery` (a `qFace(<idHash>)` target) switches to up-to-face mode:
   *  the wall grows up to that coaxial face and `thicknessMm` is ignored. */
  addThicken(
    faceQueries: string[],
    thicknessMm = 2,
    upToFaceQuery = "",
  ): number | null {
    if (faceQueries.length === 0) return null;
    let id: number;
    try {
      id = this.kernel.addThicken(
        faceQueries.join(","),
        thicknessMm,
        false,
        0,
        upToFaceQuery,
      );
    } catch (err) {
      console.error("fielddraft: addThicken failed", err);
      this.setTransientError(formatKernelError(err, "Thicken failed"));
      return null;
    }
    this.commit(new Set([id]), id);
    return id;
  }

  toggleMeasureMode(): void {
    this.patch({
      measureMode: !this.state.measureMode,
      measureSelection: [],
      filletMode: false,
      thickenMode: false,
      mateMode: false,
    });
    this.core.emit();
  }

  /** stv8: add a picked B-Rep element to the measure set. A repeated pick of
   *  the same element removes it (toggle). The set holds at most two entities
   *  — a third pick starts a fresh pair from the new element. */
  addMeasureEntity(el: PickedElement): void {
    const sel = this.state.measureSelection;
    const idx = sel.findIndex(
      (e) => e.kind === el.kind && e.idHash === el.idHash,
    );
    let next: PickedElement[];
    if (idx >= 0) {
      next = sel.filter((_, i) => i !== idx);
    } else if (sel.length >= 2) {
      next = [el];
    } else {
      next = [...sel, el];
    }
    this.patch({ measureSelection: next });
    this.core.emit();
  }

  /** stv8: drop one measured element (panel "×" button). */
  removeMeasureEntity(el: PickedElement): void {
    this.patch({
      measureSelection: this.state.measureSelection.filter(
        (e) => !(e.kind === el.kind && e.idHash === el.idHash),
      ),
    });
    this.core.emit();
  }

  clearMeasureSelection(): void {
    this.patch({ measureSelection: [] });
    this.core.emit();
  }

  toggleFilletMode(): void {
    this.patch({
      filletMode: !this.state.filletMode,
      thickenMode: false,
      measureMode: false,
      measureSelection: [],
      mateMode: false,
    });
    this.core.emit();
  }

  /** Thicken mode toggle — mirrors `toggleFilletMode`. While active, a
   *  viewport click on a curved face creates one thicken feature. Mutually
   *  exclusive with the fillet / mate / measure modes. */
  toggleThickenMode(): void {
    this.patch({
      thickenMode: !this.state.thickenMode,
      filletMode: false,
      measureMode: false,
      measureSelection: [],
      mateMode: false,
    });
    this.core.emit();
  }

  toggleMateMode(): void {
    this.patch({
      mateMode: !this.state.mateMode,
      filletMode: false,
      thickenMode: false,
      measureMode: false,
      measureSelection: [],
    });
    this.core.emit();
  }

  /** Toggle visibility of all committed mate-connector triads (Onshape's `K`
   *  shortcut). Pure UI state — does not touch the document. */
  toggleMateConnectorVisibility(): void {
    this.patch({ mateConnectorsHidden: !this.state.mateConnectorsHidden });
    this.core.emit();
  }

  // ── P1: Assembly management — implementation in AssemblyStore ─────

  exitAssembly(): void {
    this.assembly.exitAssembly();
  }

  addPartToAssembly(
    source: string | Uint8Array | ArrayBuffer,
    name?: string,
  ): void {
    this.assembly.addPartToAssembly(source, name);
  }

  removePartFromAssembly(partId: string): void {
    this.assembly.removePartFromAssembly(partId);
  }

  selectAssemblyPart(partId: string | null): void {
    this.assembly.selectAssemblyPart(partId);
  }

  setPartTransform(
    partId: string,
    translate: [number, number, number],
    rotate: [number, number, number],
  ): void {
    this.assembly.setPartTransform(partId, translate, rotate);
  }

  addMate(
    type: MateType,
    partAId: string,
    connAFeatureId: number,
    partBId: string,
    connBFeatureId: number,
  ): void {
    this.assembly.addMate(type, partAId, connAFeatureId, partBId, connBFeatureId);
  }

  removeMate(mateId: string): void {
    this.assembly.removeMate(mateId);
  }

  setMateAngle(mateId: string, angleDeg: number): void {
    this.assembly.setMateAngle(mateId, angleDeg);
  }

  setAssemblyScript(source: string): void {
    this.assembly.setAssemblyScript(source);
  }

  runFieldScript(
    params: Record<string, number | boolean>,
    time: number,
    dt: number,
  ): string | null {
    return this.assembly.runFieldScript(params, time, dt);
  }

  exportR3F(): void {
    this.assembly.exportR3F();
  }

  publishProject(): Promise<string | null> {
    return this.project.publishProject();
  }

  toggleIKMode(): void {
    this.assembly.toggleIKMode();
  }

  runIK(target: IKTarget): void {
    this.assembly.runIK(target);
  }

  setMateOffset(mateId: string, offsetMm: number): void {
    this.assembly.setMateOffset(mateId, offsetMm);
  }

  setMateJointLimits(mateId: string, minAngle: number, maxAngle: number): void {
    this.assembly.setMateJointLimits(mateId, minAngle, maxAngle);
  }

  selectAll(): void {
    const all = new Set(this.state.features.map((f) => f.id));
    this.patch({ selection: all, selectionAnchor: null });
    this.core.emit();
  }

  clearSelection(origin: SelectionOrigin = "list"): void {
    // Deselecting the pending ghost confirms it (the dialog's OK path).
    this.confirmPendingFeature();
    if (
      this.state.selection.size === 0 &&
      this.state.selectionAnchor === null &&
      this.state.selectedElement === null &&
      this.state.selectedDatum === null
    )
      return;
    this.patch({
      selection: new Set(),
      selectionAnchor: null,
      selectedElement: null,
      selectedElements: [],
      selectedDatum: null,
    });
    this.syncActiveBody();
    this.refreshMeshCache();
    this.syncSketchSessionToSelection(origin);
    this.core.emit();
  }

  /**
   * Ticket pqoe: a sketch behaves like any other feature — selecting it
   * IS editing it. Reconcile the active sketch session with the current
   * feature selection:
   *  - exactly one sketch feature selected → enter its edit session
   *    (unless we are already editing that same id);
   *  - selection moved away → commit the active session so live edits
   *    persist (the ✗ Cancel button is the explicit discard path).
   * Idempotent and called only from the explicit selection mutators, so
   * the selection change that `commitSketch` performs internally cannot
   * re-trigger it (no loop).
   *
   * Ticket 7hgr (a): the sketch session is STICKY against the 3D scene. A
   * scene-origin selection change (clicking a body / datum / sketch entity /
   * empty space in the viewport) must NEVER commit or exit the active
   * session — only the ✓/✗ buttons, the Extrude/Revolve sketch-toolbar
   * buttons, delete (vev1) and New Project (5zd3) end it. So while a session
   * is active we ignore scene-origin changes entirely. List-origin changes
   * (clicking a feature row in the outliner — the default) keep the pqoe
   * behaviour: selecting another sketch enters its edit session, selecting
   * a non-sketch feature commits the current one.
   */
  private syncSketchSessionToSelection(origin: SelectionOrigin = "list"): void {
    const session = this.state.sketchSession;
    // Scene clicks never touch an active session (ticket 7hgr a).
    if (origin === "scene" && session.active) return;
    const editingId = session.active ? session.editingFeatureId : null;
    const sole =
      this.state.selection.size === 1
        ? (this.state.features.find((f) => this.state.selection.has(f.id)) ??
          null)
        : null;
    const targetSketchId = sole && sole.kind === "sketch" ? sole.id : null;

    // Already editing exactly the selected sketch → nothing to do. (A fresh
    // draft session has editingFeatureId === null but is NOT "no session",
    // so it must still be reconciled below when the selection moves.)
    if (session.active && targetSketchId === editingId && editingId !== null)
      return;
    if (!session.active && targetSketchId === null) return;

    // Selection no longer matches the active session → commit it so live
    // edits persist (the ✗ button is the explicit discard). commitSketch()
    // routes through `commit()`, which resets the selection to the committed
    // sketch id; restore the caller's intended selection afterwards so a
    // switch A→B (or sketch→other feature) doesn't leave the old id selected.
    if (session.active) {
      const intended = this.state.selection;
      const intendedAnchor = this.state.selectionAnchor;
      this.commitSketch();
      this.state = {
        ...this.state,
        selection: intended,
        selectionAnchor: intendedAnchor,
      };
      this.syncActiveBody();
    }
    // A sketch is now the sole selection → enter its edit session.
    if (targetSketchId !== null) this.editSketch(targetSketchId);
  }

  // ── Chunk 3a: persistent B-Rep element selection ──────────────────

  /** Select a single B-Rep element (face / edge / vertex) picked tool-free
   *  in the viewport. Clears the feature-level selection so the two notions
   *  never both show. Ephemeral UI state — not persisted. */
  selectElement(el: PickedElement): void {
    this.confirmPendingFeature();
    this.state = {
      ...this.state,
      selection: new Set(),
      selectionAnchor: null,
      selectedElement: el,
      selectedElements: [el],
      selectedDatum: null,
    };
    this.syncActiveBody();
    this.notify();
  }

  /** Add or remove `el` from the multi-select set (Shift/Ctrl-click). The
   *  primary `selectedElement` becomes the toggled element when added, or the
   *  new last member when removed. Used to gather coplanar faces for a single
   *  face-extrude. Clears the feature-level selection like {@link selectElement}. */
  toggleElement(el: PickedElement): void {
    this.confirmPendingFeature();
    const same = (a: PickedElement, b: PickedElement) =>
      a.kind === b.kind && a.idHash === b.idHash;
    const present = this.state.selectedElements.some((e) => same(e, el));
    const next = present
      ? this.state.selectedElements.filter((e) => !same(e, el))
      : [...this.state.selectedElements, el];
    this.state = {
      ...this.state,
      selection: new Set(),
      selectionAnchor: null,
      selectedElement: next.length > 0 ? next[next.length - 1] : null,
      selectedElements: next,
      selectedDatum: null,
    };
    this.syncActiveBody();
    this.notify();
  }

  /** Replace (or, additive, extend) the element multi-select set with a
   *  whole batch at once — the box-select path (rectangle drag). Dedupes by
   *  kind+idHash; the last element becomes the primary. An empty batch with
   *  additive=false clears the element selection. */
  applyBoxSelection(els: PickedElement[], additive: boolean): void {
    const base = additive ? [...this.state.selectedElements] : [];
    const seen = new Set(base.map((e) => `${e.kind}:${e.idHash}`));
    for (const el of els) {
      const key = `${el.kind}:${el.idHash}`;
      if (seen.has(key)) continue;
      seen.add(key);
      base.push(el);
    }
    this.state = {
      ...this.state,
      selection: new Set(),
      selectionAnchor: null,
      selectedElement: base.length > 0 ? base[base.length - 1] : null,
      selectedElements: base,
      selectedDatum: null,
    };
    this.syncActiveBody();
    this.notify();
  }

  /** Replace (or extend) the feature-level selection with a set of ids —
   *  the object-mode box-select path (whole bodies → their seed features). */
  selectFeatureSet(ids: number[], additive: boolean): void {
    logUserAction("feature", "select-set", { ids, additive });
    const next = additive ? new Set(this.state.selection) : new Set<number>();
    for (const id of ids) next.add(id);
    this.state = {
      ...this.state,
      selection: next,
      selectionAnchor: null,
      selectedElement: null,
      selectedElements: [],
      selectedDatum: null,
    };
    this.syncActiveBody();
    this.refreshMeshCache();
    this.notify();
  }

  /** Clear the persistent element selection. No-op when nothing is selected. */
  clearElementSelection(): void {
    if (
      this.state.selectedElement === null &&
      this.state.selectedElements.length === 0 &&
      this.state.selectedDatum === null
    )
      return;
    this.patch({
      selectedElement: null,
      selectedElements: [],
      selectedDatum: null,
    });
    this.notify();
  }

  /** Select a default-geometry datum ("origin", "top", "front", "right"),
   *  or clear with null (ticket r36f). Clears the feature- and element-
   *  level selections — only one selection notion shows at a time. */
  selectDatum(datumId: string | null): void {
    if (this.state.selectedDatum === datumId) {
      if (datumId === null) return;
    }
    this.state = {
      ...this.state,
      selection: new Set(),
      selectionAnchor: null,
      selectedElement: null,
      selectedElements: [],
      selectedDatum: datumId,
      highlightedBodyIndex: null,
    };
    this.notify();
  }

  // ── K6b.2: sketch session ────────────────────────────────────────

  private notify(): void {
    this.core.notify();
  }

  setLoading(phase: string | null): void {
    this.core.setLoading(phase);
  }

  // ── K6b.2: sketch session — implementation in SketchStore ─────────
  //
  // The whole sketch domain (session lifecycle, drawing/modify tools,
  // constraints, the JS-side sketch undo stack and the solve pipeline)
  // lives in [`sketchStore.ts`](./sketchStore). The facade delegates
  // 1:1 so the public API (and every existing caller/test) stays put.

  enterSketch(planeQuery: string): void {
    this.sketch.enterSketch(planeQuery);
  }

  cancelSketch(): void {
    this.sketch.cancelSketch();
  }

  setSketchPlane(planeQuery: string): void {
    this.sketch.setSketchPlane(planeQuery);
  }

  editSketch(featureId: number): void {
    this.sketch.editSketch(featureId);
  }

  exitSketch(): void {
    this.sketch.exitSketch();
  }

  selectSketchTool(
    typeName: string | null,
    initialDraft: Record<string, unknown> = {},
  ): void {
    this.sketch.selectSketchTool(typeName, initialDraft);
  }

  setSketchToolDraft(draft: Record<string, unknown>): void {
    this.sketch.setSketchToolDraft(draft);
  }

  applySketchToolDraft(): void {
    this.sketch.applySketchToolDraft();
  }

  pushSketchSubCalls(
    calls: ReadonlyArray<{ type: string; params: Record<string, unknown> }>,
  ): void {
    this.sketch.pushSketchSubCalls(calls);
  }

  importVectorSubCalls(text: string, filename: string): number {
    return this.sketch.importVectorSubCalls(text, filename);
  }

  replaceSketchSubCalls(
    calls: ReadonlyArray<{ type: string; params: Record<string, unknown> }>,
  ): void {
    this.sketch.replaceSketchSubCalls(calls);
  }

  setSketchLineLength(index: number, lengthMm: number): boolean {
    return this.sketch.setSketchLineLength(index, lengthMm);
  }

  setSketchDiagnostics(diagnostics: SketchDiagnostics): void {
    this.sketch.setSketchDiagnostics(diagnostics);
  }

  removeSketchSubCall(index: number): void {
    this.sketch.removeSketchSubCall(index);
  }

  deleteSketchSelection(): boolean {
    return this.sketch.deleteSketchSelection();
  }

  trimSketchAt(click: [number, number], tolMm: number): void {
    this.sketch.trimSketchAt(click, tolMm);
  }

  splitSketchAt(
    targetIndex: number,
    click: [number, number],
    tolMm: number,
  ): void {
    this.sketch.splitSketchAt(targetIndex, click, tolMm);
  }

  extendSketchAt(
    targetIndex: number,
    click: [number, number],
    tolMm: number,
  ): void {
    this.sketch.extendSketchAt(targetIndex, click, tolMm);
  }

  mirrorSketchSelection(axisLineIndex: number): void {
    this.sketch.mirrorSketchSelection(axisLineIndex);
  }

  mirrorSketchEntityAt(axisLineIndex: number, entityId: number): void {
    this.sketch.mirrorSketchEntityAt(axisLineIndex, entityId);
  }

  setSketchFilletRadius(radius: number): void {
    this.sketch.setSketchFilletRadius(radius);
  }

  filletSketchAt(firstIndex: number, secondIndex: number, radius: number): void {
    this.sketch.filletSketchAt(firstIndex, secondIndex, radius);
  }

  setFilletRadiusAt(arcSubCallId: string, newRadius: number): boolean {
    return this.sketch.setFilletRadiusAt(arcSubCallId, newRadius);
  }

  setSketchChamferDistance(dist: number): void {
    this.sketch.setSketchChamferDistance(dist);
  }

  chamferSketchAt(firstIndex: number, secondIndex: number, dist: number): void {
    this.sketch.chamferSketchAt(firstIndex, secondIndex, dist);
  }

  setChamferDistanceAt(bevelSubCallId: string, newDistance: number): boolean {
    return this.sketch.setChamferDistanceAt(bevelSubCallId, newDistance);
  }

  filletCornerAt(point: [number, number], radius: number, tolMm: number): void {
    this.sketch.filletCornerAt(point, radius, tolMm);
  }

  chamferCornerAt(point: [number, number], dist: number, tolMm: number): void {
    this.sketch.chamferCornerAt(point, dist, tolMm);
  }

  setSketchOffsetDistance(dist: number): void {
    this.sketch.setSketchOffsetDistance(dist);
  }

  offsetSketchAt(targetIndex: number, clickPoint: Vec2, distance: number): void {
    this.sketch.offsetSketchAt(targetIndex, clickPoint, distance);
  }

  setSketchSplineMode(mode: "interpolate" | "control"): void {
    this.sketch.setSketchSplineMode(mode);
  }

  setSketchTextValue(value: string): void {
    this.sketch.setSketchTextValue(value);
  }

  setSketchTextHeight(height: number): void {
    this.sketch.setSketchTextHeight(height);
  }

  setSketchTextFontId(id: string): void {
    this.sketch.setSketchTextFontId(id);
  }

  ensureTextFontLoaded(id?: string): Promise<void> {
    return this.sketch.ensureTextFontLoaded(id);
  }

  loadUploadedTextFont(name: string, buffer: ArrayBuffer): string | null {
    return this.sketch.loadUploadedTextFont(name, buffer);
  }

  placeSketchText(origin: [number, number]): void {
    this.sketch.placeSketchText(origin);
  }

  updateSketchTextGroup(
    groupId: string,
    next: { value: string; height: number; fontId: string },
  ): boolean {
    return this.sketch.updateSketchTextGroup(groupId, next);
  }

  deleteSketchTextGroup(groupId: string): boolean {
    return this.sketch.deleteSketchTextGroup(groupId);
  }

  setSketchPatternCount(count: number): void {
    this.sketch.setSketchPatternCount(count);
  }

  setSketchPatternSpacing(spacing: number): void {
    this.sketch.setSketchPatternSpacing(spacing);
  }

  setSketchPatternAngle(angleDeg: number): void {
    this.sketch.setSketchPatternAngle(angleDeg);
  }

  linearPatternSketch(axis: "x" | "y"): void {
    this.sketch.linearPatternSketch(axis);
  }

  circularPatternSketch(): void {
    this.sketch.circularPatternSketch();
  }

  setSketchRotateAngle(angleDeg: number): void {
    this.sketch.setSketchRotateAngle(angleDeg);
  }

  moveSketchSelection(dx: number, dy: number, copy: boolean): void {
    this.sketch.moveSketchSelection(dx, dy, copy);
  }

  translateSketchEntities(
    entityIds: ReadonlyArray<number>,
    dx: number,
    dy: number,
  ): void {
    this.sketch.translateSketchEntities(entityIds, dx, dy);
  }

  rotateSketchSelection(angleDeg: number, copy: boolean): void {
    this.sketch.rotateSketchSelection(angleDeg, copy);
  }

  toggleSketchSelection(entityId: number): void {
    this.sketch.toggleSketchSelection(entityId);
  }

  setSketchSelection(ids: ReadonlyArray<number>): void {
    this.sketch.setSketchSelection(ids);
  }

  clearSketchSelection(): void {
    this.sketch.clearSketchSelection();
  }

  applySketchConstraint(type: string): void {
    this.sketch.applySketchConstraint(type);
  }

  selectSketchConstraintTool(type: string): void {
    this.sketch.selectSketchConstraintTool(type);
  }

  pickSketchConstraintEntity(entityId: number): void {
    this.sketch.pickSketchConstraintEntity(entityId);
  }

  toggleConstraintDriven(index: number): void {
    this.sketch.toggleConstraintDriven(index);
  }

  setSketchConstraintValue(index: number, value: number): void {
    this.sketch.setSketchConstraintValue(index, value);
  }

  toggleSketchConstructionAt(index: number): void {
    this.sketch.toggleSketchConstructionAt(index);
  }
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

  setRollbackIndex(index: number): void {
    const n = this.state.features.length;
    this.kernel.setRollbackIndex(index >= n ? -1 : index);
    this.commit();
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

  /** Add a box centred half a depth out along a picked face's normal —
   *  the "pull a face out" shortcut wired to the viewport. */
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
    // Ticket D6: picking a transform mode also ARMS it — the next scene body
    // click materialises/edits the matching transform feature (see
    // transformBody), Onshape "Move tool" style.
    this.patch({ gizmoMode: mode, gizmoArmed: true });
    this.notify();
  }

  /** Ticket D6/320t: a scene click on a body while Move mode is armed creates
   *  (or re-selects the most recent) transform feature of the active gizmoMode
   *  targeting that body, then selects it so the gizmo appears. Reusing the
   *  last matching transform keeps repeated moves from stacking features.
   *  `bodyFeatureId` is the body's owning feature (click.contributors[0]).
   *  Returns the transform feature id, or null on failure. */
  transformBody(bodyFeatureId: number): number | null {
    if (bodyFeatureId <= 0) return null;
    const kind = this.state.gizmoMode; // translate | rotate | scale
    // Reuse the most recent transform of this kind already targeting the body.
    let existing: number | null = null;
    for (const f of this.state.features) {
      if (f.kind !== kind) continue;
      try {
        const p = this.kernel.getFeatureCallParams(f.id);
        if (Number(p.target_id) === bodyFeatureId) existing = f.id;
      } catch {
        // unreadable params — skip
      }
    }
    this.patch({ gizmoArmed: false }); // consume the arm
    if (existing !== null) {
      this.selectOnly(existing, "scene");
      return existing;
    }
    let id: number;
    try {
      id = this.kernel.addFeatureCall(
        kind,
        JSON.stringify({ target_id: bodyFeatureId }),
      );
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: transformBody failed", err);
      this.setTransientError(formatKernelError(err, "Move failed"));
      return null;
    }
    this.commit(new Set([id]), id);
    return id;
  }

  /** Ticket D6: leave Move mode without acting (Esc, or a list selection). */
  disarmGizmo(): void {
    if (!this.state.gizmoArmed) return;
    this.patch({ gizmoArmed: false });
    this.notify();
  }

  /** K7.22 UX-fix (2): commit-style setter for a feature's world-
   *  space translate. Used by the viewport gizmo drag handler —
   *  it keeps the store in sync with the kernel, triggers a worker
   *  re-mesh and refreshes the features list so the outliner
   *  highlight and the gizmo's own placeholder position both
   *  follow the new translate. Returns `true` if anything changed. */
  setFeatureTranslate(id: number, x: number, y: number, z: number): boolean {
    if (!this.kernel.setFeatureTranslate(id, x, y, z)) return false;
    this.commit();
    return true;
  }

  setFeatureRotation(
    id: number,
    rx: number,
    ry: number,
    rz: number,
    rw: number,
  ): boolean {
    if (!this.kernel.setFeatureRotation(id, rx, ry, rz, rw)) return false;
    this.commit();
    return true;
  }

  setFeatureScale(id: number, sx: number, sy: number, sz: number): boolean {
    if (!this.kernel.setFeatureScale(id, sx, sy, sz)) return false;
    this.commit();
    return true;
  }

  applyGizmoTranslate(id: number, tx: number, ty: number, tz: number): boolean {
    if (!this.kernel.applyGizmoTranslate(id, tx, ty, tz)) return false;
    this.commit();
    return true;
  }

  applyGizmoRotate(
    id: number,
    rx: number,
    ry: number,
    rz: number,
    rw: number,
  ): boolean {
    if (!this.kernel.applyGizmoRotate(id, rx, ry, rz, rw)) return false;
    this.commit();
    return true;
  }

  applyGizmoScale(id: number, sx: number, sy: number, sz: number): boolean {
    if (!this.kernel.applyGizmoScale(id, sx, sy, sz)) return false;
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
    this.patch({ visibleSketches: next });
    persistVisibleSketchesToStorage(next);
    this.core.emit();
  }

  toggleDatumVisible(datumId: string): void {
    const next = new Set(this.state.hiddenDatums);
    if (next.has(datumId)) next.delete(datumId);
    else next.add(datumId);
    this.patch({ hiddenDatums: next });
    persistHiddenDatumsToStorage(next);
    this.core.emit();
  }

  // ── K7.22 UX-fix (2): feature folders (display-only grouping) ──

  /** Update the `folders` slice of state, persist to localStorage,
   *  notify React subscribers. Kernel is not touched. */
  private setFolders(next: readonly FeatureFolder[]): void {
    this.patch({ folders: next });
    persistFoldersToStorage(next);
    this.core.emit();
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
   *  edits back into the kernel. `coalesce` folds the replace
   *  into the previous undo step (throttled live drags). */
  updateFeatureParams(
    id: number,
    params: Record<string, unknown>,
    coalesce = false,
  ): boolean {
    try {
      this.kernel.replaceFeatureCallParams(id, params, coalesce);
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
    return sketches.reduce(
      (acc, f) => (f.id > acc ? f.id : acc),
      sketches[0].id,
    );
  }

  private static NON_SPINE_KINDS = new Set([
    "sketch",
    "linear_pattern",
    "circular_pattern",
    "mirror",
    "mate_connector",
    "translate",
    "rotate",
    "scale",
    "hole",
    "fillet_edge",
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
      id = this.kernel.addMirror(source.id, "origin_plane_yz");
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
    // No sketch yet → create an EMPTY revolve (ticket 1rax): the dialog's
    // query picker fills `profiles` afterwards; until then the feature
    // reports its regen error Onshape-style (red title + tooltip).
    const sketchId = this.latestSketchId() ?? 0;
    let id: number;
    try {
      id = this.kernel.addRevolve(sketchId, this.state.nextOp);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: addRevolve failed", err);
      return null;
    }
    this.commit(new Set([id]), id);
    return id;
  }

  /** Helix-sweep the latest committed sketch (screw threads) with
   *  default pitch/turns — the detail dialog tunes the rest. Without a
   *  sketch an EMPTY helix is created and the dialog's query picker
   *  fills `profiles` afterwards (same flow as revolve). */
  addHelixFromLatestSketch(): number | null {
    const sketchId = this.latestSketchId() ?? 0;
    let id: number;
    try {
      id = this.kernel.addHelix(sketchId, this.state.nextOp);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: addHelix failed", err);
      return null;
    }
    this.commit(new Set([id]), id);
    return id;
  }

  /** Sweep the second-latest sketch (profile) along the latest sketch
   *  (path). Convention: draw the profile first, then the path, then Sweep.
   *  Returns null if fewer than two sketches exist. */
  addSweepFromLatestSketches(): number | null {
    const sketchIds = this.state.features
      .filter((f) => f.kind === "sketch")
      .map((f) => f.id)
      .sort((a, b) => b - a);
    if (sketchIds.length < 2) return null;
    const pathId = sketchIds[0];
    const profileId = sketchIds[1];
    let id: number;
    try {
      id = this.kernel.addSweep(profileId, pathId, this.state.nextOp);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: addSweep failed", err);
      return null;
    }
    this.commit(new Set([id]), id);
    return id;
  }

  /** Sweep the latest sketch (profile) along a default 3D control-point path
   *  (Gizmo3d); the user then routes the spine by editing the control points
   *  in the Detail panel. Returns null without a sketch. */
  addSweepGizmoFromLatestSketch(): number | null {
    const sketchId = this.latestSketchId();
    if (sketchId === null) return null;
    let id: number;
    try {
      id = this.kernel.addSweepGizmo(sketchId, this.state.nextOp);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: addSweepGizmo failed", err);
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
      // Created as a PENDING feature (ticket x7af), but UNSUPPRESSED
      // (ticket yx5h): the real body is woven and rendered solid/opaque
      // straight away — no light-blue translucent preview prism. The
      // feature stays "pending" only for the dialog lifecycle: the depth
      // gizmo keeps editing it and Cancel drops it; confirming (OK /
      // deselect) just clears the pending flag.
      id = this.kernel.addExtrude(
        sketchId,
        depthMm,
        this.state.nextOp,
        0,
        false,
      );
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: addExtrude failed", err);
      this.setTransientError(formatKernelError(err, "Extrude failed"));
      return null;
    }
    this.pendingFeature = id;
    this.commit(new Set([id]), id);
    return id;
  }

  showTransientError(message: string): void {
    this.core.setTransientError(message);
  }

  private setTransientError(message: string): void {
    this.core.setTransientError(message);
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
    if (!canTriggerDownload()) return;
    const bytes = this.kernel.exportStlBinary();
    const copy = new Uint8Array(bytes.length);
    copy.set(bytes);
    triggerDownload(copy, "model/stl", filename);
  }

  /** M14a: export the current scene as a 3MF archive with
   *  per-body colors. Same WASM→Blob→download pattern as STL. */
  download3mf(filename = "fielddraft.3mf"): void {
    if (!canTriggerDownload()) return;
    const bytes = this.kernel.export3mf();
    const copy = new Uint8Array(bytes.length);
    copy.set(bytes);
    triggerDownload(
      copy,
      "application/vnd.ms-package.3dmanufacturing-3dmodel+xml",
      filename,
    );
  }

  /** Import a .3mf file: extract meshes as CSG solids, stash
   *  slicer metadata for roundtrip re-export. */
  import3mfFile(buffer: ArrayBuffer): number[] {
    const t0 = performance.now();
    console.log(
      `fielddraft: importing 3MF — ${(buffer.byteLength / 1024).toFixed(0)} KB`,
    );
    this.setLoading("Extracting 3MF archive…");
    try {
      const result = import3mf(buffer);
      console.log(
        `fielddraft: 3MF extracted — ${result.meshes.length} mesh(es)`,
      );
      const ids: number[] = [];
      for (let i = 0; i < result.meshes.length; i++) {
        const mesh = result.meshes[i];
        this.setLoading(`Building SDF ${i + 1}/${result.meshes.length}…`);
        console.log(
          `fielddraft: 3MF mesh ${i + 1} — ${mesh.indices.length / 3} triangles, building BVH…`,
        );
        const id = this.kernel.addImportedMesh(
          mesh.positions,
          mesh.indices,
          BoolOpTag.New,
        );
        ids.push(id);
      }
      this.patch({ threemfStash: result.stash, loading: null });
      if (ids.length > 0) {
        this.commit(new Set(ids), ids[0]);
      } else {
        this.notify();
      }
      console.log(
        `fielddraft: 3MF import complete — ${ids.length} body(ies) in ${(performance.now() - t0).toFixed(0)} ms`,
      );
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
    if (!canTriggerDownload()) return;
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
    triggerDownload(
      blobData,
      "application/vnd.ms-package.3dmanufacturing-3dmodel+xml",
      filename,
    );
  }

  /** M14c: export the mechanical core of the scene as STEP via
   *  the OCCT Web Worker. Lazy-loads opencascade.js (~25 MB)
   *  on first call. */
  async downloadStep(filename = "fielddraft.step"): Promise<void> {
    if (!canTriggerDownload()) return;
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
          lines.push(
            `${report.excluded} feature(s) will be EXCLUDED (not exportable):`,
          );
          for (const f of report.features) {
            if (f.status.startsWith("excluded:")) {
              lines.push(`  • ${f.name}: ${f.status.slice(10)}`);
            }
          }
          lines.push("");
        }
        if (report.degraded > 0) {
          lines.push(
            `${report.degraded} feature(s) will be DEGRADED (sharp edges instead of fillets/chamfers):`,
          );
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
      triggerDownload(copy, "application/step", filename);
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
    console.log(
      `fielddraft: importing STEP — ${(buffer.byteLength / 1024).toFixed(0)} KB`,
    );
    this.setLoading("Loading STEP importer…");
    try {
      const { importStep } = await import("./stepImport");
      const fileBytes = new Uint8Array(buffer);
      const { positions, indices } = await importStep(fileBytes, (phase) => {
        console.log(`fielddraft: STEP phase — ${phase}`);
        this.setLoading(phase);
      });

      const posF32 =
        positions instanceof Float32Array
          ? positions
          : new Float32Array(positions);
      const idxU32 =
        indices instanceof Uint32Array ? indices : new Uint32Array(indices);

      console.log(
        `fielddraft: STEP tessellated — ${idxU32.length / 3} triangles, building BVH…`,
      );
      this.setLoading("Building SDF from mesh…");
      const id = this.kernel.addImportedMesh(posF32, idxU32, BoolOpTag.New);
      this.setLoading(null);
      this.commit(new Set([id]), id);
      console.log(
        `fielddraft: STEP import complete — feature #${id} in ${(performance.now() - t0).toFixed(0)} ms`,
      );
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
    if (!canTriggerDownload()) return;
    const THREE = await import("three");
    const { GLTFExporter } =
      await import("three/examples/jsm/exporters/GLTFExporter.js");

    const scene = new THREE.Scene();
    const mat = new THREE.MeshStandardMaterial({ color: 0x8ca6d9 });
    for (const body of this.state.meshedBodies) {
      if (body.positions.length === 0) continue;
      const geo = new THREE.BufferGeometry();
      geo.setAttribute(
        "position",
        new THREE.BufferAttribute(body.positions, 3),
      );
      geo.setAttribute("normal", new THREE.BufferAttribute(body.normals, 3));
      geo.setIndex(new THREE.BufferAttribute(body.indices, 1));
      scene.add(new THREE.Mesh(geo, mat));
    }

    const exporter = new GLTFExporter();
    const glb = (await exporter.parseAsync(scene, {
      binary: true,
    })) as ArrayBuffer;
    triggerDownload(glb, "model/gltf-binary", filename);
  }

  downloadObj(filename = "fielddraft.obj"): void {
    if (!canTriggerDownload()) return;
    triggerDownload(
      buildObjExportText(this.state.meshedBodies),
      "text/plain",
      filename,
    );
  }

  // ── MT-A: project / multi-tab management — see ProjectStore ───────

  setActiveTab(id: string): void {
    this.project.setActiveTab(id);
  }

  createTab(kind: TabKind): string {
    return this.project.createTab(kind);
  }

  projectName(): string {
    return this.project.projectName();
  }

  renameProject(name: string): void {
    this.project.renameProject(name);
  }

  renameTab(id: string, name: string): void {
    this.project.renameTab(id, name);
  }

  deleteTab(id: string): void {
    this.project.deleteTab(id);
  }

  listScripts(): FieldSkriptRecord[] {
    return this.project.listScripts();
  }

  saveScriptTab(
    tabId: string,
    source: string,
  ): { ok: true; record: FieldSkriptRecord } | { ok: false; error: string } {
    return this.project.saveScriptTab(tabId, source);
  }

  insertScriptInstance(scriptId: string): number | null {
    return this.project.insertScriptInstance(scriptId);
  }

  /**
   * Ticket 7hgr (b): commit the active sketch, exit sketch mode, then start
   * an Extrude referencing the just-committed sketch as its profile. Mirrors
   * the main toolbar's Extrude path — `commitSketch()` selects the committed
   * sketch (making it the `latestSketchId`), and `addExtrudeFromLatestSketch`
   * creates the feature + selects it, which opens its Detail view. No-op when
   * the sketch has no committable geometry (commitSketch returns null).
   */
  extrudeFromSketchSession(depthMm = 25): number | null {
    if (!this.state.sketchSession.active) return null;
    if (this.commitSketch() === null) return null;
    return this.addExtrudeFromLatestSketch(depthMm);
  }

  /**
   * Ticket 7hgr (b): commit the active sketch, exit sketch mode, then start
   * a Revolve referencing the just-committed sketch (same flow as
   * {@link extrudeFromSketchSession}).
   */
  revolveFromSketchSession(): number | null {
    if (!this.state.sketchSession.active) return null;
    if (this.commitSketch() === null) return null;
    return this.addRevolveFromLatestSketch();
  }

  commitSketch(): number | null {
    return this.sketch.commitSketch();
  }
}

let singleton: KernelStore | null = null;
let storePromise: Promise<KernelStore> | null = null;

export async function getStore(): Promise<KernelStore> {
  if (singleton) return singleton;
  if (storePromise) return storePromise;

  storePromise = (async () => {
    const kernel = await Kernel.create();

    // The persisted blob is a serialized *Project* (multi-tab model),
    // not a kernel Document — the constructor rehydrates it
    // (parseProject → inject scripts → load the active part into the
    // kernel). Feeding it straight to `kernel.loadJson` here was the
    // MT-A leftover that threw "missing field schema_version" and, worse,
    // dropped the persisted project on every reload.
    // ClaudeDraft "open ticket" deep-link (`?project=`): a repro tab must not
    // touch the user's shared (origin-wide) persisted project — bootProject.ts
    // replaces the doc with the ticket's. Use an inert autosave and a clean
    // start so opening a ticket can never clobber a live work session in
    // another tab.
    const deepLink = bootProjectPath() !== null;
    const autosave = deepLink ? createNoopAutoSave() : createAutoSave();
    const store = new KernelStore(
      kernel,
      autosave,
      deepLink ? null : loadPersisted(),
    );

    // Stale-WASM guard (UI side): `Kernel.create` already logs a console
    // warning on a version mismatch; also surface it as a toast so it's
    // not missed. A stale kernel means new mesher code (e.g. RFTA) never
    // loaded, so the viewport silently shows old output.
    if (kernel.version() !== __KERNEL_EXPECTED__) {
      store.showTransientError(
        `Stale WASM (loaded kernel v${kernel.version()} ≠ on-disk v${__KERNEL_EXPECTED__}) — reload (Ctrl+Shift+R)`,
      );
    }

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
