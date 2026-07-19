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
  type AutoSaveHandle,
} from "./persistence";
import {
  persistProjectDoc,
  loadProjectDoc,
  migrateFromLocalStorage,
  gcLocalBlobs,
} from "./projectPersistence";
import { bootProjectPath } from "./bootParams";
import { getAutosaveDebounceMs } from "./settings";
import { subscribeCameraPoseChanged } from "./cameraPoseBridge";
import { subscribeDebugDialogs } from "./projectDebug";
import { subscribePartAabb } from "./partAabbStore";
import { subscribeSdfPlane } from "./sdfPlaneStore";
import { subscribeMesherDebug } from "./mesherDebugStore";
import {
  diffSnapshots,
  type VoxelSnapshot,
  type VoxelDiff,
} from "./voxelSnapshot";
import { getVoxelResolution } from "./voxelDebugStore";
import { subscribeLichenDebug } from "./lichenDebugStore";
import {
  parseVariableInput,
  resolveVariableTable,
  type RawVariable,
} from "./variableInput";
import type { PickedElement } from "./elementQuery";
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
import type { MeshOverlayData } from "./meshIO";
import type {
  ExportEstimate,
  ExportFormat,
  ExportMethodOptions,
  ExportOutcome,
  ExportProgress,
  MeshMethod,
} from "./exportTypes";

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
  type SelectionOrigin,
  type StoreState,
} from "./storeCore";
import { SketchStore, initialSketchToolParams } from "./sketchStore";
import { AssemblyStore } from "./assemblyStore";
import { ProjectStore } from "./projectStore";
import { IoStore } from "./ioStore";
import { SelectionStore } from "./selectionStore";
import { WorkerOrchestrator } from "./workerOrchestration";

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
  /** Import/export domain controller — see ioStore.ts. */
  private readonly io: IoStore;
  /** Selection domain controller — see selectionStore.ts. */
  private readonly sel: SelectionStore;

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

  /** Worker orchestration (mesher + weaver channels, drag/mesh
   *  toggles) — see workerOrchestration.ts. */
  private readonly workers: WorkerOrchestrator;

  /** True in a deep-link repro tab (`?project=`), which boots with an inert
   *  autosave so it can't clobber another tab's persisted project. An
   *  explicit user Open clears this via {@link graduateFromDeepLink}. */
  private deepLink = false;

  constructor(
    kernel: Kernel,
    autosave: AutoSaveHandle,
    persistedProjectJson: string | null = null,
    deepLink = false,
  ) {
    this.kernel = kernel;
    this.deepLink = deepLink;
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
      syncActiveBody: () => this.sel.syncActiveBody(),
      serializeProject: () => this.project.serializeProject(),
      refreshMeshCache: () => this.workers.refreshMeshCache(),
      scheduleWeave: () => this.workers.scheduleWeave(),
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
    this.io = new IoStore(this.core);
    // Selection reconciles against the pending feature, the sketch
    // session and the mesher — all injected as deps.
    this.sel = new SelectionStore(this.core, {
      isPendingFeature: (id) => this.isPendingFeature(id),
      confirmPendingFeature: () => this.confirmPendingFeature(),
      refreshMeshCache: () => this.workers.refreshMeshCache(),
      commitSketch: () => this.commitSketch(),
      editSketch: (featureId) => this.editSketch(featureId),
    });

    // Bring up the mesher + weaver workers BEFORE the initial document
    // load (see WorkerOrchestrator): with the mesher available, sync
    // meshing is off, so the bootstrap loadJson below neither runs the
    // Dual-Contouring pass on the main thread nor weaves synchronously.
    this.workers = new WorkerOrchestrator(this.core);

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

    if (this.workers.workerAvailable) {
      // Kick off the first worker rebuild so the initial document
      // (possibly rehydrated from localStorage) becomes visible.
      this.workers.scheduleWorkerRebuild();
    } else {
      // Sync-meshing fallback: read body buffers from the main-thread
      // kernel directly (the loadJson above already meshed them).
      this.state = {
        ...this.state,
        meshedBodies: this.workers.snapshotMainThreadBodies(),
        sceneSize: this.kernel.sceneAabb().size as [number, number, number],
      };
    }
    // First async weave for the freshly loaded document — the overlays
    // mount against empty exports and refetch when the imports land.
    this.workers.scheduleWeave();
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

  // ── worker orchestration — implementation in WorkerOrchestrator ───

  setDcMeshingEnabled(enabled: boolean, algorithm?: string): void {
    this.workers.setDcMeshingEnabled(enabled, algorithm);
  }

  setParametricWeave(enabled: boolean): void {
    this.workers.setParametricWeave(enabled);
  }

  setDragBrepRecompute(enabled: boolean): void {
    this.workers.setDragBrepRecompute(enabled);
  }

  setLiveExtrudeDrag(enabled: boolean): void {
    this.workers.setLiveExtrudeDrag(enabled);
  }

  isLiveExtrudeDrag(): boolean {
    return this.workers.isLiveExtrudeDrag();
  }

  beginGizmoDrag(): void {
    this.workers.beginGizmoDrag();
  }

  endGizmoDrag(): void {
    this.workers.endGizmoDrag();
  }

  /** Public accessor for the live project as a `.fdraft` ZIP — see
   *  {@link ProjectStore.exportProjectFdraft} (bug-report flow, on3k). */
  exportProjectFdraft(): Promise<Uint8Array> {
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

  // ── mutations ─────────────────────────────────────────────────────

  setNextOp(op: BoolOpTagValue): void {
    if (this.state.nextOp === op) return;
    this.patch({ nextOp: op });
    this.core.emit();
  }

  // ── K9.0: reference-mesh import — implementation in IoStore ───────

  loadReferenceMesh(buffer: ArrayBuffer): Promise<boolean> {
    return this.io.loadReferenceMesh(buffer);
  }

  loadReferenceGlb(buffer: ArrayBuffer): Promise<boolean> {
    return this.io.loadReferenceGlb(buffer);
  }

  loadReferenceObj(text: string): Promise<boolean> {
    return this.io.loadReferenceObj(text);
  }

  convertReferenceToSolid(): number | null {
    return this.io.convertReferenceToSolid();
  }

  clearReferenceMesh(): void {
    this.io.clearReferenceMesh();
  }

  importMeshAsFeature(
    positions: Float32Array,
    normals: Float32Array,
    indices: Uint32Array,
  ): number | null {
    return this.io.importMeshAsFeature(positions, normals, indices);
  }

  getMeshOverlayData(id: number): MeshOverlayData | undefined {
    return this.io.getMeshOverlayData(id);
  }

  getMeshOverlayFeatures(): { id: number; data: MeshOverlayData }[] {
    return this.io.getMeshOverlayFeatures();
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

  /** Add a custom-SDF body (default: 15 mm sphere expression in a
   *  20 mm box — edit the expression in the feature dialog). */
  addCustomSdf(halfExtentMm = 20): number {
    let id: number;
    try {
      id = this.kernel.addCustomSdf(halfExtentMm);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: addCustomSdf failed", err);
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

  /** #384 offset_tree: add an `offset` feature. Bodies (scope) and the signed
   *  offset are picked in the dialog (schema-driven); the feature emits no
   *  geometry until at least one body is scoped. */
  addOffset(deltaMm = 1): number {
    let id: number;
    try {
      id = this.kernel.addOffset("", deltaMm);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: addOffset failed", err);
      return -1;
    }
    this.commit(new Set([id]), id);
    return id;
  }

  /** d: round every Boolean-junction edge globally with `radiusMm`. */
  addRoundAll(radiusMm = 2): number {
    let id: number;
    try {
      id = this.kernel.addRoundAll(radiusMm);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: addRoundAll failed", err);
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
        this.io.dropMeshData(id);
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
    this.io.dropMeshData(id);
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
    this.sel.selectOnly(id, origin);
  }

  toggleSelect(id: number, origin: SelectionOrigin = "list"): void {
    this.sel.toggleSelect(id, origin);
  }

  selectRangeTo(id: number, origin: SelectionOrigin = "list"): void {
    this.sel.selectRangeTo(id, origin);
  }

  highlightBody(index: number | null): void {
    this.sel.highlightBody(index);
  }

  // ── project persistence & server sync — see ProjectStore ──────────

  saveProjectToFile(filename = "project.fdraft"): Promise<void> {
    return this.project.saveProjectToFile(filename);
  }

  loadProjectFromJson(json: string): Promise<boolean> {
    return this.project.loadProjectFromJson(json);
  }

  loadProjectFromBytes(bytes: Uint8Array | ArrayBuffer): Promise<boolean> {
    return this.project.loadProjectFromBytes(bytes);
  }

  /** Explicit user "Open project" (File → Open, drag-drop): loads the bytes
   *  AND makes the result durable across a reload. Ticket 4z37: a deep-link
   *  repro tab boots with an inert autosave and a `?project=` URL that
   *  re-fetches the ticket on reload — so a disk project opened there
   *  vanished on reload. Opening a project is a deliberate "work on THIS"
   *  action, so it graduates the tab out of repro mode (real autosave, URL
   *  param stripped) and force-writes the loaded project now. The automated
   *  ticket-repro boot never calls this, so it can't clobber another tab. */
  async openUserProjectFromBytes(bytes: Uint8Array | ArrayBuffer): Promise<boolean> {
    const ok = await this.project.loadProjectFromBytes(bytes);
    if (!ok) return false;
    this.graduateFromDeepLink();
    // Persist immediately (deep-link tabs' load-time flush hit the noop).
    this.core.autosave.schedule(this.project.serializeProject());
    this.core.autosave.flush();
    return true;
  }

  /** Flush any pending autosave immediately (beforeunload). Reads the LIVE
   *  core handle so a graduated deep-link tab flushes its real autosave. */
  flushAutosave(): void {
    this.core.autosave.flush();
  }

  /** Turn a deep-link repro tab into a normal working tab: swap in a real
   *  debounced autosave and drop the `?project=` param so a reload restores
   *  the now-persisted project instead of re-fetching the ticket. No-op in a
   *  tab that was already a normal one. */
  private graduateFromDeepLink(): void {
    if (!this.deepLink) return;
    this.deepLink = false;
    this.core.replaceAutosave(
      createAutoSave({ debounceMs: getAutosaveDebounceMs, sink: persistProjectDoc }),
    );
    if (typeof window !== "undefined") {
      try {
        const url = new URL(window.location.href);
        if (url.searchParams.has("project")) {
          url.searchParams.delete("project");
          window.history.replaceState(null, "", url.toString());
        }
      } catch {
        // history/URL unavailable (tests) — the autosave swap alone suffices.
      }
    }
  }

  setProjectId(id: string | null): void {
    this.project.setProjectId(id);
  }

  serializeProjectJson(): string {
    return this.project.serializeProjectJson();
  }

  loadServerProject(id: string, documentJson: string): Promise<boolean> {
    return this.project.loadServerProject(id, documentJson);
  }

  /** P2: re-inline a server document's externalized asset blobs into a full
   *  project JSON (for a gallery-copy open). Legacy full documents are a no-op. */
  inlineServerDocument(documentJson: string): Promise<string> {
    return this.project.inlineServerDocument(documentJson);
  }

  detachServerProject(): void {
    this.project.detachServerProject();
  }

  syncToServer(opts?: { keepalive?: boolean }): Promise<boolean> {
    return this.project.syncToServer(opts);
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
  ): Promise<void> {
    return this.assembly.addPartToAssembly(source, name);
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
    this.assembly.addMate(
      type,
      partAId,
      connAFeatureId,
      partBId,
      connBFeatureId,
    );
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
    this.sel.selectAll();
  }

  clearSelection(origin: SelectionOrigin = "list"): void {
    this.sel.clearSelection(origin);
  }

  // ── Chunk 3a: persistent B-Rep element selection — SelectionStore ─

  selectElement(el: PickedElement): void {
    this.sel.selectElement(el);
  }

  toggleElement(el: PickedElement): void {
    this.sel.toggleElement(el);
  }

  applyBoxSelection(els: PickedElement[], additive: boolean): void {
    this.sel.applyBoxSelection(els, additive);
  }

  selectFeatureSet(ids: number[], additive: boolean): void {
    this.sel.selectFeatureSet(ids, additive);
  }

  clearElementSelection(): void {
    this.sel.clearElementSelection();
  }

  selectDatum(datumId: string | null): void {
    this.sel.selectDatum(datumId);
  }

  // ── selectors (Review-Befund 8) ────────────────────────────────────

  /** The partitioned body list for the Parts tree — id / seed feature /
   *  triangle count. Sourced from the kernel body list (present in every
   *  render mode, unlike `meshedBodies`) with triangle counts looked up
   *  in the worker mesh cache (0 in SDF modes). */
  listBodies(): { id: number; seedId: number; triangleCount: number }[] {
    const k = this.kernel;
    const n = k.bodyCount();
    const triById = new Map<number, number>(
      this.state.meshedBodies.map((b) => [
        b.id,
        Math.round(b.indices.length / 3),
      ]),
    );
    const out: { id: number; seedId: number; triangleCount: number }[] = [];
    for (let i = 0; i < n; i++) {
      const id = k.bodyId(i);
      out.push({
        id,
        seedId: k.bodySeedFeatureId(i),
        triangleCount: triById.get(id) ?? 0,
      });
    }
    return out;
  }

  relatedFeatureIds(): ReadonlySet<number> {
    return this.sel.relatedFeatureIds();
  }

  relatedBodyIndex(): number | null {
    return this.sel.relatedBodyIndex();
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

  filletSketchAt(
    firstIndex: number,
    secondIndex: number,
    radius: number,
  ): void {
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

  offsetSketchAt(
    targetIndex: number,
    clickPoint: Vec2,
    distance: number,
  ): void {
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

  /**
   * Create a feature of ANY type the kernel knows (`listFeatureTypes`) from its
   * wire-form params (`schemaForType`) — the generic counterpart to the typed
   * `addPrimitive` / `addHole` / … helpers the toolbar uses.
   *
   * Added for the AI chat (ticket #347), whose whole point is to drive features
   * it was never hardcoded for — including the user's own FieldSkript features,
   * which only exist at runtime. It commits like every other mutation, so undo,
   * regeneration and autosave behave exactly as they do for a toolbar click.
   * Throws (kernel error) on an unknown type or malformed params.
   */
  addFeatureByType(type: string, params: Record<string, unknown> = {}): number {
    const id = this.kernel.addFeatureCall(type, JSON.stringify(params));
    this.commit(new Set([id]), id);
    return id;
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

  /** Mark the persisted project dirty WITHOUT a document commit: camera
   *  moves and debug-dialog changes route here. Serialisation is lazy (the
   *  producer runs once at write time), so high-frequency markers (a camera
   *  drag marks once per changed frame) cost nothing until the debounce
   *  fires. */
  markProjectDirty(): void {
    this.core.autosave.schedule(() => this.project.serializeProject());
  }

  private setTransientError(message: string): void {
    this.core.setTransientError(message);
  }

  // ── K7.2.1: import/export — implementation in IoStore ─────────────

  /** Meshing Phase 0: run the tolerance-driven export pass (worker
   *  when available) and get bytes + validation report back. `options`
   *  carries the method-specific knobs (Lichen: the curvature angle). */
  exportMeshed(
    format: ExportFormat,
    cellSizeMm: number,
    method: MeshMethod,
    options: ExportMethodOptions,
    onProgress?: (p: ExportProgress) => void,
  ): Promise<ExportOutcome> {
    return this.workers.exportMeshed(
      format,
      cellSizeMm,
      method,
      options,
      onProgress,
    );
  }

  /** Cheap, synchronous pre-mesh estimate for the export dialog (dense-grid
   *  methods only): the finest achievable cell size and a rough triangle
   *  count at `cellSizeMm`. Runs on the main-thread kernel (it only needs the
   *  decoded features + AABBs), so the dialog can call it live. Returns null
   *  on a parse failure or when no session-capable kernel is present. */
  exportEstimate(cellSizeMm: number, method: MeshMethod): ExportEstimate | null {
    try {
      const json = this.kernel.exportEstimate(cellSizeMm, method);
      if (!json) return null;
      return JSON.parse(json) as ExportEstimate;
    } catch {
      return null;
    }
  }

  /** 3MF archive from raw triangle buffers — format conversion for the
   *  server meshing route (the mesher returns binary STL). */
  mesh3mfFromBuffers(positions: Float32Array, indices: Uint32Array): Uint8Array {
    return this.kernel.mesh3mfFromBuffers(positions, indices);
  }

  /** Debug SDF-plane sampler: the scene SDF on a `resolution`² grid over the
   *  plane at `origin`/`normal` spanning ±`halfExtent`. Synchronous main-thread
   *  eval of the exact mesher field. */
  sdfPlaneSample(
    origin: [number, number, number],
    normal: [number, number, number],
    halfExtent: number,
    resolution: number,
  ): Float32Array {
    return this.kernel.sdfPlaneSample(origin, normal, halfExtent, resolution);
  }

  /** Debug ▸ Mesher (step 1): narrow-band adaptive octree leaf centres, packed
   *  `[x,y,z, sdf, nx,ny,nz, cellSize]` per leaf. Synchronous main-thread eval. */
  mesherPoints(optionsJson: string): Float32Array {
    return this.kernel.mesherPoints(optionsJson);
  }

  /** Voxel debug (#369): sample the scene solid on a regular cube grid of
   *  `resolution` voxels along the longest scene-AABB axis. Returns the grid
   *  geometry + a row-major occupancy array (1 inside / 0 outside) + the
   *  occupied count, or `null` for an empty scene. Both wasm arrays are copies
   *  owned by JS, so a snapshot stays valid after further edits — that is what
   *  lets the dialog/console diff a before/after pair. Synchronous main-thread
   *  eval of the exact mesher field. */
  voxelize(resolution: number): VoxelSnapshot | null {
    const info = this.kernel.voxelGridInfo(resolution);
    if (info.length < 7) return null;
    const dims: [number, number, number] = [info[0], info[1], info[2]];
    const origin: [number, number, number] = [info[3], info[4], info[5]];
    const spacing = info[6];
    const occupancy = this.kernel.voxelizeOccupancy(resolution);
    let count = 0;
    for (let i = 0; i < occupancy.length; i++) if (occupancy[i]) count++;
    return { dims, origin, spacing, occupancy, count };
  }

  /** Debug ▸ Mesher Lichen: the flat overlay format the PASM mesher emits
   *  (`[nPoints, nTris, nFallbackTris, points×9, indices]`). Synchronous
   *  main-thread eval. */
  lichenMesh(json: string): Float32Array {
    return this.kernel.lichenMesh(json);
  }

  /** Download an already-built export (3MF re-wraps stashed slicer
   *  metadata from the last import). */
  downloadExport(
    bytes: Uint8Array,
    format: ExportFormat,
    filename?: string,
  ): void {
    this.io.downloadExport(bytes, format, filename);
  }

  import3mfFile(buffer: ArrayBuffer): number[] {
    return this.io.import3mfFile(buffer);
  }

  downloadStep(filename = "fielddraft.step"): Promise<void> {
    return this.io.downloadStep(filename);
  }

  importStepFile(buffer: ArrayBuffer): Promise<number | null> {
    return this.io.importStepFile(buffer);
  }

  downloadGlb(filename = "fielddraft.glb"): Promise<void> {
    return this.io.downloadGlb(filename);
  }

  downloadObj(filename = "fielddraft.obj"): void {
    this.io.downloadObj(filename);
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

/** Synchronous, non-subscribing peek at the loaded store singleton (or `null`
 *  before it boots). For imperative call-sites that must not re-render on state
 *  changes — e.g. the camera controller's cursor-pick pivot. */
export function peekStore(): KernelStore | null {
  return singleton;
}

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
    const autosave = deepLink
      ? createNoopAutoSave()
      : createAutoSave({ debounceMs: getAutosaveDebounceMs, sink: persistProjectDoc });
    // P1b: the browser-local working store moved from localStorage to IndexedDB
    // (content-addressed assets, docs/PROJECT-ASSET-STORAGE-PLAN §5.3). Migrate
    // any legacy localStorage row once, then load the (mesh-externalized) doc
    // and re-inline its blobs. Both awaits are safe here — getStore is already
    // async (it awaits the WASM kernel). A deep-link repro tab keeps its clean
    // start (it must never touch the origin-wide working store).
    let persisted: string | null = null;
    if (!deepLink) {
      await migrateFromLocalStorage();
      persisted = await loadProjectDoc();
      // P3e: reclaim orphaned local blobs left by earlier saves (mesh/font a
      // newer save replaced). Fire-and-forget so it never delays boot; it
      // no-ops on an empty store, so it can only run once a doc has loaded.
      void gcLocalBlobs().catch(() => {});
    }
    const store = new KernelStore(kernel, autosave, persisted, deepLink);

    // Dirty tracking beyond document commits: camera moves and debug-tool
    // changes also persist into the project (Project.view / Project.debug),
    // debounced through the same autosave handle. The camera source fires
    // only on REAL pose changes (cameraPoseBridge deduplicates per frame).
    const markDirty = () => store.markProjectDirty();
    subscribeCameraPoseChanged(markDirty);
    subscribeDebugDialogs(markDirty);
    subscribePartAabb(markDirty);
    subscribeSdfPlane(markDirty);
    subscribeMesherDebug(markDirty);
    subscribeLichenDebug(markDirty);

    // Stale-WASM guard (UI side): `Kernel.create` already logs a console
    // warning on a version mismatch; also surface it as a toast so it's
    // not missed. A stale kernel means new mesher code never
    // loaded, so the viewport silently shows old output.
    if (kernel.version() !== __KERNEL_EXPECTED__) {
      store.showTransientError(
        `Stale WASM (loaded kernel v${kernel.version()} ≠ on-disk v${__KERNEL_EXPECTED__}) — reload (Ctrl+Shift+R)`,
      );
    }

    if (typeof window !== "undefined") {
      // Flush the LIVE autosave (a deep-link tab graduates its handle on an
      // explicit Open, ticket 4z37) — not the captured boot handle, or a
      // pending edit after graduation would be lost on unload.
      window.addEventListener("beforeunload", () => store.flushAutosave());
      // Voxel-Debug console API (#369): pull a voxel snapshot of the current
      // scene and diff two snapshots straight from DevTools — the workflow the
      // operator asked for (snapshot → add a feature → snapshot again → diff).
      // Exposed unconditionally (not DEV-gated like __fdStore): it is a
      // read-only measurement tool the operator explicitly requested.
      (
        window as unknown as {
          __fdVoxel?: {
            snapshot: (resolution?: number) => VoxelSnapshot | null;
            diff: (
              a: VoxelSnapshot,
              b: VoxelSnapshot,
            ) => VoxelDiff | { error: string };
          };
        }
      ).__fdVoxel = {
        snapshot: (resolution?: number) =>
          store.voxelize(resolution ?? getVoxelResolution()),
        diff: (a: VoxelSnapshot, b: VoxelSnapshot) => diffSnapshots(a, b),
      };
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
