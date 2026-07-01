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
  type VariableEntry,
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
import type { WeaveBrepsResponse, WeaveRequest } from "./weaver.worker";
import {
  INACTIVE_SKETCH_SESSION as _INACTIVE,
  applySketchToolDraft as reduceApply,
  enterSketchSession as reduceEnter,
  exitSketchSession as reduceExit,
  setSketchSessionPlane as reduceSetPlane,
  selectSketchTool as reduceSelect,
  setSketchToolDraft as reduceSetDraft,
  removeSketchSubCallAt as reduceRemoveAt,
  replaceSketchSubCallAt as reduceReplaceAt,
  toggleSketchSelection as reduceToggleSelection,
  setSketchSelection as reduceSetSelection,
  clearSketchSelection as reduceClearSelection,
  buildDraftSketchState,
  subCallsForSelection,
  entityOwnerSubCalls,
} from "./sketchSession";
import {
  buildConstraintCall,
  applicableConstraints,
  isConstraintTool,
  resolveCoincidentConstraint,
  CONSTRAINT_DIMENSION_TOOL,
  CONSTRAINT_COINCIDENT_TOOL,
  DIMENSION_TYPES_BY_SPECIFICITY,
} from "./sketchConstraints";
import { logUserAction } from "./userActionLog";
import { resolveCornerFillet } from "./sketchCorner";
import { buildLineLengthEdit } from "./sketchLiveInput";
import {
  computeTrim,
  curveOfSubCall,
  edgesOfSubCall,
  subCallOfCurve,
} from "./sketchTrim";
import type { Vec2 } from "./sketchTrim";
import { mirrorSubCall, isMirrorableSubCall } from "./sketchMirror";
import { offsetCurve, offsetChain, buildChain } from "./sketchOffset";
import { patternLinear, patternCircular, rotateCurve } from "./sketchPattern";
import {
  translateSubCall,
  isTranslatableSubCall,
} from "./sketchSubCallTransform";
import {
  filletLines,
  filletRecipe,
  reeditFilletRadius,
  nextFdId,
  fdIdOf,
  FD_ID_PARAM,
} from "./sketchFillet";
import {
  chamferLines,
  chamferRecipe,
  reeditChamferDistance,
} from "./sketchChamfer";
import { extendLine } from "./sketchExtend";
import { splitCurveAt } from "./sketchSplit";
import { getPublishedPickCamera } from "./pickCameraBridge";
import { publishSceneBounds } from "./sceneBoundsBridge";
import { importVectorToSubCalls } from "./importVector";
import { textToSubCalls, reeditTextGroup, removeTextGroup } from "./sketchText";
import {
  DEFAULT_FONT_ID,
  cachedFont,
  loadBundledFont,
  parseFontBuffer,
  registerUploadedFont,
  getUploadedFontBytes,
  registerUploadedFontFromBytes,
  UPLOADED_FONT_PREFIX,
} from "./sketchTextFont";
import { solveAndReplace } from "./solver/replaySolve";
import type { SketchDiagnostics } from "./solver/types";
import {
  INACTIVE_ASSEMBLY,
  allocateAssemblyId,
  IDENTITY_TRANSFORM,
  solveAssembly,
  solveIK,
  type AssemblyDocument,
  type AssemblyPartInstance,
  type AssemblyMeshedBody,
  type Mate,
  type MateType,
  type IKTarget,
} from "./assembly";
import { executeScript } from "./fieldscript";
import {
  createRecord,
  reviseRecord,
  type FieldSkriptRecord,
} from "./fieldskript/record";
import {
  DEFAULT_SCRIPT_SOURCE,
  allocateTabId,
  createDefaultProject,
  createPartTab,
  createAssemblyTab,
  injectScripts,
  parseProject,
  projectScripts,
  reseedTabIds,
  serializeProject as serializeProjectJson,
  serializeProjectToZip,
  parseProjectBytes,
  emptyProjectAssets,
  type ProjectAssets,
  stripScripts,
  type Project,
  type ProjectTab,
  type ScriptTab,
  type TabKind,
} from "./project";
import { parseFeatureSchema, buildDefaultDefinition } from "./widgets/schema";
import {
  import3mf,
  export3mfRoundtrip,
  extractModelXml,
  type ThreeMfStash,
} from "./threemf";
import {
  type MeshOverlayData,
  canTriggerDownload,
  triggerDownload,
  parseStlBinary,
  parseObjText,
  buildObjExportText,
} from "./meshIO";

/**
 * One mesh-ready body as cached by the store after a worker response.
 * The TypedArrays are owned by the main thread (they were transferred
 * from the worker), so they're safe to retain across future kernel
 * mutations — unlike the WASM-aliased views you'd get from the
 * kernel's own `body*View` accessors.
 */
export type MeshedBody = MesherBodyPayload;

/**
 * Ticket 7hgr (a): where a feature-selection change originated.
 *  - "list": a deliberate outliner row action (default). Keeps the pqoe
 *    behaviour of selecting a sketch ⇒ editing it / switching commits the
 *    previous one.
 *  - "scene": a 3D viewport click (body / datum / sketch entity / empty
 *    space). These must NEVER end an active sketch session — the session is
 *    sticky and only the ✓/✗ buttons, the Extrude/Revolve sketch-toolbar
 *    buttons, delete (vev1) or New Project (5zd3) exit it.
 */
export type SelectionOrigin = "list" | "scene";

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
  /** Ticket d9o9: true when this session MATERIALISED its sketch
   *  feature on entry (Onshape style — the sketch shows in the tree
   *  immediately). Cancel (✗) then deletes that fresh feature; an
   *  edit-reentry of an existing sketch leaves this false so Cancel
   *  only discards the in-session edits. */
  createdThisSession: boolean;
  /** Solver-entity ids of the committed geometry currently
   *  selected in the canvas (select mode — no drawing tool
   *  active). Drives the context-sensitive constraint palette
   *  and, later, drag-edit. Ids are resolved against
   *  `buildDraftSketchState(subCalls)`. Empty when nothing is
   *  picked. */
  selection: ReadonlyArray<number>;
  /** F20: latest solve diagnostics for the session geometry —
   *  which committed constraints the solver flagged as conflicting
   *  or (partially) redundant. Written by the centralized solve
   *  pass (`solveAndReplace`) after every successful solve; reset
   *  to empty on session enter. Constraint ids live in the same id
   *  space as `buildDraftSketchState(subCalls)`, so the Outliner can
   *  map them back to constraint sub-call rows. */
  diagnostics: SketchDiagnostics;
}

export interface StoreState {
  features: FeatureSummary[];
  selection: ReadonlySet<number>;
  /** Anchor for shift-click range selection — id of the last single-clicked
   *  feature. `null` while nothing has been clicked yet. */
  selectionAnchor: number | null;
  /** Chunk 3a: persistent, tool-free B-Rep element selection (a single
   *  face / edge / vertex picked in the viewport with no active tool).
   *  Ephemeral UI state — NOT persisted to the project. Mutually exclusive
   *  with the feature-level `selection` set: setting one clears the other. */
  selectedElement: PickedElement | null;
  /** The full multi-select set (Shift/Ctrl-click accumulates faces for a
   *  coplanar face-extrude). `selectedElement` is the primary (last-picked)
   *  member; an empty array means nothing is element-selected. */
  selectedElements: PickedElement[];
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
  /** Monotonic counter bumped whenever the async weaver installs freshly
   *  woven B-Reps into the main-thread kernel (`importBodyBrep`). The mesher
   *  publishes a fresh `meshedBodies` array to force a re-render; the weaver
   *  produces no mesh, so it bumps this instead — overlays/picking read the
   *  analytic B-Rep straight from the kernel and must refetch when it lands.
   *  Without it `notify()` after a weave never changes the `getState()`
   *  snapshot, so `useSyncExternalStore` bails and freshly-extruded bodies
   *  render without their edges until some unrelated state change. */
  brepVersion: number;
  /** K6b.2: active sketch session, or inactive-default. */
  sketchSession: SketchSessionState;
  /** K8.10 / 8ar9: document-level scalar variables, sorted by name
   *  (the kernel's BTreeMap ordering). Each carries a kind
   *  (number/length/angle) that drives its display unit. */
  variables: readonly VariableEntry[];
  /** K9.0: imported STL reference mesh. Ephemeral — not
   *  persisted to the document. `null` when no import is
   *  loaded. */
  referenceMesh: {
    positions: Float32Array;
    normals: Float32Array;
    indices: Uint32Array;
  } | null;
  /** K9.4: measure mode active — clicks pick B-Rep elements to measure
   *  instead of selecting features. */
  measureMode: boolean;
  /** stv8: B-Rep elements picked for measurement (max 2: one → length/radius,
   *  two → distance). Cleared when measure mode toggles. */
  measureSelection: readonly PickedElement[];
  /** K10.6: fillet mode — clicks on edges create fillet_edge features. */
  filletMode: boolean;
  /** Thicken mode — clicks on curved faces create thicken features (one per
   *  click), mirroring the edge-click fillet flow. */
  thickenMode: boolean;
  /** F10: sketch-fillet radius (mm) used by the two-click `sketch_fillet`
   *  tool to round the corner between two picked lines. */
  sketchFilletRadius: number;
  /** F11: sketch-chamfer setback distance (mm) used by the two-click
   *  `sketch_chamfer` tool to bevel the corner between two picked lines. */
  sketchChamferDistance: number;
  /** F15: sketch-offset distance (mm) used by the single-click
   *  `sketch_offset` tool to place a parallel copy of the picked curve. */
  sketchOffsetDistance: number;
  /** F16: number of instances (original + copies) for a sketch pattern. */
  sketchPatternCount: number;
  /** F16: per-step spacing (mm) for a linear sketch pattern. */
  sketchPatternSpacing: number;
  /** F16: total span (degrees) for a circular sketch pattern. */
  sketchPatternAngle: number;
  /** F17: rotation angle (degrees) used by the `rotateSketchSelection`
   *  transform (rotates the selection about the sketch origin). */
  sketchRotateAngle: number;
  /** F23-full: which spline family the `sketch_spline` tool draws —
   *  `"interpolate"` (Catmull-Rom through the points) or `"control"`
   *  (real cubic B-spline with the points as control polygon). */
  sketchSplineMode: "interpolate" | "control";
  /** F24: the string the single-click `sketch_text` tool emits as
   *  filled glyph geometry. */
  sketchTextValue: string;
  /** F24: cap/em height (mm) for the `sketch_text` tool. */
  sketchTextHeight: number;
  /** F24-full: id of the font the `sketch_text` tool renders with —
   *  a bundled-font id (see sketchTextFont.BUNDLED_FONTS) or a synthetic
   *  `uploaded:…` id for a user-supplied .ttf/.otf. */
  sketchTextFontId: string;
  /** F24-full: true once the currently-selected text font has finished
   *  loading + parsing (drives the UI's "loading font…" hint and gates
   *  placement). */
  sketchTextFontReady: boolean;
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
  /** K9: mate connector placement mode — the next viewport click
   *  places a mate connector at the surface hit point. */
  mateMode: boolean;
  /** Hide all committed mate-connector triads in the viewport (toggled by
   *  the `K` shortcut, Onshape parity). */
  mateConnectorsHidden: boolean;
  /** P1: assembly state — manages multi-part scene graph. */
  assembly: import("./assembly").AssemblyState;
  /** P2: IK mode — clicking in viewport sets the IK target. */
  ikMode: boolean;
  /** K7.21h: transform gizmo mode. */
  gizmoMode: "translate" | "rotate" | "scale";
  /** Ticket D6/320t: "Move mode" — armed by the transform toolbar button. While
   *  true, the next scene click on a body creates (or re-selects the most
   *  recent) transform feature for that body and shows the gizmo, instead of a
   *  plain feature select. Consumed (disarmed) by that click. */
  gizmoArmed: boolean;
  /** True while a transform-gizmo drag runs WITH B-Rep suppression
   *  (setting "B-Rep rebuild while dragging" off). The viewport hides
   *  every B-Rep overlay for the drag's duration — cached edges would
   *  otherwise trail the live mesh at the 50ms commit cadence. */
  gizmoDragSuppressed: boolean;
  /** 3MF roundtrip: stashed slicer metadata from import, used on re-export. */
  threemfStash: ThreeMfStash | null;
  /** Rollback bar position: features at index >= this value are
   *  skipped during regeneration. Equals features.length when the
   *  bar sits at the bottom (all features active). */
  rollbackIndex: number;
  /** Loading overlay: non-null while an import or heavy operation is
   *  in progress. The `phase` string is shown to the user. */
  loading: { phase: string } | null;
  /** Default datum geometry hidden set. Stores ids of datums the user
   *  has toggled off ("origin", "top", "front", "right"). Datums not
   *  in this set are visible. Persisted to localStorage. */
  hiddenDatums: ReadonlySet<string>;
  /** Selected default-geometry datum ("origin", "top", "front", "right"),
   *  or null. Onshape-style tree/viewport selection of the default planes
   *  (ticket r36f). Mutually exclusive with feature/element selection. */
  selectedDatum: string | null;
  highlightedBodyIndex: number | null;
  /** MT-A: the multi-tab project model (part / script / assembly
   *  tabs). Replaces the old two-way `activeTab` toggle — the active
   *  tab is `project.tabs` filtered by `project.activeTabId`.
   *  Persisted (via the store's serializeProject funnel) to
   *  localStorage. */
  project: Project;
  /** FT-W: lightweight per-script-type meta the Part toolbar renders
   *  as one button each (glyph = icon, label = name). Recomputed from
   *  the kernel's `listScripts()` on every commit so newly saved /
   *  deleted scripts appear or vanish immediately. */
  scriptTypes: { id: string; name: string; icon: string }[];
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

const HIDDEN_DATUMS_STORAGE_KEY = "fielddraft.hiddenDatums";

function loadHiddenDatumsFromStorage(): Set<string> {
  try {
    const raw = localStorage.getItem(HIDDEN_DATUMS_STORAGE_KEY);
    if (!raw) return new Set();
    const parsed = JSON.parse(raw);
    if (!Array.isArray(parsed)) return new Set();
    return new Set(parsed.filter((s): s is string => typeof s === "string"));
  } catch {
    return new Set();
  }
}

function persistHiddenDatumsToStorage(ids: ReadonlySet<string>): void {
  try {
    localStorage.setItem(HIDDEN_DATUMS_STORAGE_KEY, JSON.stringify([...ids]));
  } catch {
    // ignore
  }
}

/** Pull a user-readable message out of a wasm-bindgen-thrown error.
 *  The kernel emits strings like
 *  `"extrude regeneration failed: feature ‘extrude’: parameter
 *  ‘profiles’ invalid: sketch has no closed regions to extrude — …"`
 *  — we strip the machine-friendly prefix and keep the human tail. */
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

export type { MeshOverlayData } from "./meshIO";

export class KernelStore {
  readonly kernel: Kernel;
  private readonly autosave: AutoSaveHandle;
  private listeners = new Set<() => void>();
  private state: StoreState;

  private patch(partial: Partial<StoreState>): void {
    this.state = { ...this.state, ...partial };
  }

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

  /** B-Rep weaver worker (third kernel instance). While it's up, the
   *  main kernel's B-Rep exports stay PERMANENTLY suppressed
   *  (cache-or-empty, never a synchronous main-thread weave); after
   *  every commit the stale bodies are farmed out here and the woven
   *  results come back as MessagePack buffers for `importBodyBrep`. */
  private weaver: Worker | null = null;
  private weaverAvailable = false;
  private weaverRequestCounter = 0;
  private latestWeaveRequestId = 0;
  private weaverInflight = false;
  private weaveQueued = false;
  /** Mirror of the parametric-weave setting for weave requests. */
  private parametricWeaveEnabled = true;

  /** MT-A: which part tab's document is currently loaded into the live
   *  kernel. `null` only transiently during construction. */
  private loadedPartTabId: string | null = null;

  constructor(
    kernel: Kernel,
    autosave: AutoSaveHandle,
    persistedProjectJson: string | null = null,
  ) {
    this.kernel = kernel;
    this.autosave = autosave;

    // Bring up the mesher worker BEFORE the initial document load: with the
    // worker available, sync meshing is disabled first, so `loadJson` below
    // neither runs the Dual-Contouring pass on the main thread nor enters
    // the kernel's mesh face-tagging loop — which would weave every body's
    // B-Rep synchronously and freeze the tab for minutes on weave-heavy
    // projects (spline-revolve + shell). On failure, sync meshing stays on
    // (functionally correct, just blocks the UI on every mesh rebuild).
    try {
      this.worker = new Worker(new URL("./mesher.worker.ts", import.meta.url), {
        type: "module",
      });
      this.worker.addEventListener("message", (event) => {
        this.handleWorkerMessage(event);
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
      this.weaver = new Worker(new URL("./weaver.worker.ts", import.meta.url), {
        type: "module",
      });
      this.weaver.addEventListener("message", (event) => {
        this.handleWeaverMessage(event);
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
    const project =
      parseProject(persistedProjectJson ?? "") ?? createDefaultProject();
    reseedTabIds(project.tabs);

    // Mirror every project script into the live kernel so the loaded
    // part regenerates against them.
    for (const record of projectScripts(project)) {
      kernel.upsertScript(record);
    }

    // Determine the part tab to load: the active tab if it's a part,
    // else the first part tab. A default project always has ≥1 part
    // tab; if a loaded project somehow has none, create + append one.
    const activeTab = project.tabs.find((t) => t.id === project.activeTabId);
    let partTab =
      activeTab && activeTab.kind === "part"
        ? activeTab
        : project.tabs.find(
            (t): t is import("./project").PartTab => t.kind === "part",
          );
    if (!partTab) {
      partTab = createPartTab("Part 1");
      project.tabs.push(partTab);
    }
    kernel.loadJson(
      injectScripts(partTab.documentJson, projectScripts(project)),
    );
    this.loadedPartTabId = partTab.id;

    const initialAssembly =
      activeTab && activeTab.kind === "assembly"
        ? { active: true, document: activeTab.document, selectedPartId: null }
        : INACTIVE_ASSEMBLY;

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
      sketchFilletRadius: 5,
      sketchChamferDistance: 5,
      sketchOffsetDistance: 5,
      sketchPatternCount: 3,
      sketchPatternSpacing: 20,
      sketchPatternAngle: 360,
      sketchRotateAngle: 90,
      sketchSplineMode: "interpolate",
      sketchTextValue: "Text",
      sketchTextHeight: 10,
      sketchTextFontId: DEFAULT_FONT_ID,
      sketchTextFontReady: cachedFont(DEFAULT_FONT_ID) !== undefined,
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
      scriptTypes: this.snapshotScriptTypes(),
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

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  getState = (): StoreState => this.state;

  private commit(
    nextSelection?: ReadonlySet<number>,
    nextAnchor?: number | null,
  ): void {
    // A mutation that moves the selection away from the pending extrude
    // confirms it first (e.g. creating the next feature while its dialog is
    // still open) — see the pending-feature section.
    if (
      this.pendingFeature !== null &&
      nextSelection !== undefined &&
      !nextSelection.has(this.pendingFeature)
    ) {
      this.confirmPendingInline();
    }
    const features = this.kernel.listFeatures();
    const rbRaw = this.kernel.getRollbackIndex();
    this.state = {
      features,
      selection: nextSelection ?? this.state.selection,
      selectionAnchor:
        nextAnchor !== undefined ? nextAnchor : this.state.selectionAnchor,
      // A document mutation invalidates any picked element (its idHash may no
      // longer resolve), and the two selection notions are mutually exclusive.
      selectedElement: null,
      selectedElements: [],
      canUndo: this.kernel.canUndo(),
      undoDepth: this.kernel.undoDepth(),
      canRedo: this.kernel.canRedo(),
      redoDepth: this.kernel.redoDepth(),
      nextOp: this.state.nextOp,
      // An empty document must show NO meshes immediately (ticket vy3t): the
      // mesher worker skips posting for a body-less doc, so the stale meshes
      // would otherwise linger (e.g. after Menu → New). Clear synchronously
      // when the kernel has no bodies; otherwise the worker rebuild updates.
      meshedBodies:
        this.kernel.bodyCount() === 0 ? [] : this.state.meshedBodies,
      brepVersion: this.state.brepVersion,
      measureMode: this.state.measureMode,
      measureSelection: this.state.measureSelection,
      filletMode: this.state.filletMode,
      thickenMode: this.state.thickenMode,
      sketchFilletRadius: this.state.sketchFilletRadius,
      sketchChamferDistance: this.state.sketchChamferDistance,
      sketchOffsetDistance: this.state.sketchOffsetDistance,
      sketchPatternCount: this.state.sketchPatternCount,
      sketchPatternSpacing: this.state.sketchPatternSpacing,
      sketchPatternAngle: this.state.sketchPatternAngle,
      sketchRotateAngle: this.state.sketchRotateAngle,
      sketchSplineMode: this.state.sketchSplineMode,
      sketchTextValue: this.state.sketchTextValue,
      sketchTextHeight: this.state.sketchTextHeight,
      sketchTextFontId: this.state.sketchTextFontId,
      sketchTextFontReady: this.state.sketchTextFontReady,
      syncState: this.state.projectId ? "dirty" : "offline",
      projectId: this.state.projectId,
      sketchSession: this.state.sketchSession,
      variables: this.kernel.listVariables(),
      referenceMesh: this.state.referenceMesh,
      sceneSize: this.kernel.sceneAabb().size as [number, number, number],
      folders: this.state.folders,
      visibleSketches: this.state.visibleSketches,
      transientError: null, // successful commit clears any transient error
      mateMode: false,
      mateConnectorsHidden: this.state.mateConnectorsHidden,
      assembly: this.state.assembly,
      ikMode: false,
      gizmoMode: this.state.gizmoMode,
      gizmoArmed: this.state.gizmoArmed,
      gizmoDragSuppressed: this.state.gizmoDragSuppressed,
      threemfStash: this.state.threemfStash,
      loading: this.state.loading,
      hiddenDatums: this.state.hiddenDatums,
      selectedDatum: this.state.selectedDatum,
      highlightedBodyIndex: this.state.highlightedBodyIndex,
      rollbackIndex: rbRaw < 0 ? features.length : rbRaw,
      project: this.state.project,
      scriptTypes: this.snapshotScriptTypes(),
    };
    // Drop selection entries pointing at deleted features.
    if (this.state.selection.size > 0) {
      const live = new Set(this.state.features.map((f) => f.id));
      const filtered = new Set<number>();
      for (const id of this.state.selection) {
        if (live.has(id)) filtered.add(id);
      }
      if (filtered.size !== this.state.selection.size) {
        this.patch({ selection: filtered });
      }
    }
    // Anchor must point at a still-living feature, or be cleared.
    if (
      this.state.selectionAnchor !== null &&
      !this.state.features.some((f) => f.id === this.state.selectionAnchor)
    ) {
      this.patch({ selectionAnchor: null });
    }
    this.syncActiveBody();
    this.autosave.schedule(this.serializeProject());
    this.refreshMeshCache();
    this.scheduleWeave();
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
      meshAlgorithm: this.meshAlgorithm,
    };
    this.worker.postMessage(request);
  }

  private handleWorkerMessage(event: MessageEvent): void {
    const data = event.data as MesherBodiesResponse | undefined;
    if (!data || data.type !== "bodies") return;
    if (data.requestId < this.latestWorkerRequestId) {
      // Stale response — a newer rebuild is already in flight or queued. Drop
      // it; we'll render the fresh one shortly. Crucially we keep
      // workerInflight=true here: the newer request is still outstanding, so
      // clearing the flag would let the next scheduleWorkerRebuild post a
      // second concurrent job whose result could clobber the newer one.
      this.pendingRefine = false;
      if (this.workerRebuildQueued) {
        this.postWorkerRebuild();
      }
      return;
    }
    this.workerInflight = false;
    this.state = {
      ...this.state,
      meshedBodies: data.bodies,
      sceneSize: this.kernel.sceneAabb().size as [number, number, number],
    };
    this.updateSceneBounds();
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
   * Farm the stale B-Rep weaves out to the weaver worker. Rapid-fire
   * commits coalesce exactly like mesh rebuilds: while a weave is in
   * flight, subsequent calls flip `weaveQueued`; the response handler
   * posts one new request against the LATEST document. During a
   * suppressed gizmo drag nothing is scheduled — `endGizmoDrag` runs
   * the catch-up.
   */
  private scheduleWeave(): void {
    if (!this.weaver || !this.weaverAvailable) return;
    if (this.gizmoDragActive && !this.dragBrepRecompute) return;
    if (this.weaverInflight) {
      this.weaveQueued = true;
      return;
    }
    const stale = this.kernel.bodyBrepStaleIds();
    if (stale.length === 0) return;
    this.weaverRequestCounter += 1;
    this.latestWeaveRequestId = this.weaverRequestCounter;
    this.weaverInflight = true;
    this.weaveQueued = false;
    const request: WeaveRequest = {
      type: "weave",
      requestId: this.weaverRequestCounter,
      documentJson: this.kernel.toJson(),
      parametricWeave: this.parametricWeaveEnabled,
      bodyIds: stale,
    };
    this.weaver.postMessage(request);
  }

  private handleWeaverMessage(event: MessageEvent): void {
    const data = event.data as WeaveBrepsResponse | undefined;
    if (!data || data.type !== "breps") return;
    this.weaverInflight = false;
    let imported = false;
    if (data.requestId === this.latestWeaveRequestId) {
      for (const item of data.items) {
        // Validated against the CURRENT placement-invariant key — a
        // result that raced a newer edit misses here; the queued
        // re-schedule below covers it against the latest document.
        if (this.kernel.importBodyBrep(item.id, item.bytes)) imported = true;
      }
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
    if (this.weaveQueued) {
      this.weaveQueued = false;
      this.scheduleWeave();
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
   * FT-W: derive the Part toolbar's script-button meta from the
   * kernel's FieldSkript records. Lightweight {id,name,icon} only —
   * recomputed on every commit so saves/deletes surface immediately.
   */
  private snapshotScriptTypes(): { id: string; name: string; icon: string }[] {
    return this.kernel
      .listScripts()
      .map((r) => ({ id: r.id, name: r.name, icon: r.icon }));
  }

  /**
   * MT-A: the single persistence funnel. Builds a Project mirroring
   * `state.project` but with the loaded part tab's `documentJson`
   * replaced by the live kernel doc (scripts stripped — they live in
   * script tabs). The active assembly tab is always current (assembly
   * edits write through immediately), so it needs no special-casing.
   * Everything that persists routes here.
   */
  private serializeProject(): string {
    return serializeProjectJson(this.buildPersistableProject());
  }

  /** MT-A: build the persistable {@link Project} (live kernel doc folded
   *  back into the loaded part tab, scripts stripped). The JSON funnel and
   *  the wu2v ZIP save both consume this. */
  private buildPersistableProject(): Project {
    const tabs = this.state.project.tabs.map((t) => {
      if (t.id === this.loadedPartTabId && t.kind === "part") {
        return { ...t, documentJson: stripScripts(this.kernel.toJson()) };
      }
      return t;
    });
    return { ...this.state.project, tabs };
  }

  /**
   * wu2v: collect the uploaded fonts actually referenced by a project.
   * Scans every part tab's document JSON for `sketch_path` sub-calls whose
   * `text.fontId` is an `uploaded:` id, then pulls the retained raw bytes.
   * Fonts with no retained bytes (e.g. opened from a legacy JSON that never
   * carried them) are skipped.
   */
  private collectProjectAssets(project: Project): ProjectAssets {
    const assets = emptyProjectAssets();
    const ids = new Set<string>();
    for (const tab of project.tabs) {
      if (tab.kind !== "part") continue;
      let doc: unknown;
      try {
        doc = JSON.parse(tab.documentJson);
      } catch {
        continue;
      }
      // Walk the doc collecting any `fontId` field that names an upload.
      const walk = (value: unknown): void => {
        if (Array.isArray(value)) {
          for (const v of value) walk(v);
          return;
        }
        if (value && typeof value === "object") {
          const obj = value as Record<string, unknown>;
          const fontId = obj.fontId;
          if (
            typeof fontId === "string" &&
            fontId.startsWith(UPLOADED_FONT_PREFIX)
          ) {
            ids.add(fontId);
          }
          for (const v of Object.values(obj)) walk(v);
        }
      };
      walk(doc);
    }
    for (const id of ids) {
      const bytes = getUploadedFontBytes(id);
      if (!bytes) continue;
      // Extension from the id tail (`uploaded:Foo.ttf` → `ttf`), default ttf.
      const dot = id.lastIndexOf(".");
      const ext = dot >= 0 ? id.slice(dot + 1).toLowerCase() : "ttf";
      assets.fonts.set(id, { bytes, ext });
    }
    return assets;
  }

  /** Public accessor for the live project as a `.fdraft` ZIP (document
   *  JSON + uploaded font assets), byte-identical to what Save writes.
   *  Used by the in-app bug-report flow (on3k) so a ticket carries the
   *  exact, openable project file — assets included — not a stripped
   *  JSON blob. */
  exportProjectFdraft(): Uint8Array {
    const project = this.buildPersistableProject();
    const assets = this.collectProjectAssets(project);
    return serializeProjectToZip(project, assets);
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
    for (const listener of this.listeners) listener();
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

  // ── sketch-session undo/redo (ticket r3ci) ───────────────────────
  //
  // Sketch geometry lives entirely on the UI side as the session's
  // `subCalls` (it isn't pushed to the kernel until Finish), so the
  // kernel undo stack — what `undo()`/`redo()` drive — knows nothing
  // about a line the user just drew. These two stacks give the active
  // session its own history: each entry is a full `subCalls` snapshot
  // (the arrays are always replaced immutably, so storing the reference
  // is a sound snapshot). Checkpoints are captured at the two structural
  // chokepoints — `updateSketch` (active→active, subCalls changed) and
  // `pushSketchSubCalls`. The solver's re-coordinate pass goes through
  // `replaceSketchSubCalls` (NOT these), so a solve never lands as its
  // own undo step.
  private sketchUndoStack: SketchSessionState["subCalls"][] = [];
  private sketchRedoStack: SketchSessionState["subCalls"][] = [];

  /** Snapshot the current sub-spine onto the undo stack and start a fresh
   *  redo branch. Called just before a user-initiated structural mutation. */
  private pushSketchUndoCheckpoint(): void {
    this.sketchUndoStack.push(this.state.sketchSession.subCalls);
    // Bound the history so a long session can't grow it without limit.
    if (this.sketchUndoStack.length > 200) this.sketchUndoStack.shift();
    this.sketchRedoStack = [];
  }

  /** Clear both sketch history stacks — on session enter/exit, where a
   *  cross-session undo would make no sense. */
  private resetSketchHistory(): void {
    this.sketchUndoStack = [];
    this.sketchRedoStack = [];
  }

  /** Undo the last structural sketch edit (drawn geometry, deletes,
   *  dimension edits). No-op outside a session or with empty history.
   *  Re-solves so the overlay / inference / DoF reflect the restored
   *  spine, exactly like every other mutation. */
  sketchUndo(): boolean {
    if (!this.state.sketchSession.active) return false;
    const prev = this.sketchUndoStack.pop();
    if (prev === undefined) return false;
    this.sketchRedoStack.push(this.state.sketchSession.subCalls);
    this.state = {
      ...this.state,
      sketchSession: {
        ...this.state.sketchSession,
        subCalls: prev,
        selection: [],
      },
    };
    this.notify();
    void solveAndReplace(this);
    return true;
  }

  /** Redo the last undone sketch edit. */
  sketchRedo(): boolean {
    if (!this.state.sketchSession.active) return false;
    const next = this.sketchRedoStack.pop();
    if (next === undefined) return false;
    this.sketchUndoStack.push(this.state.sketchSession.subCalls);
    this.state = {
      ...this.state,
      sketchSession: {
        ...this.state.sketchSession,
        subCalls: next,
        selection: [],
      },
    };
    this.notify();
    void solveAndReplace(this);
    return true;
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
    for (const listener of this.listeners) listener();
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
    for (const listener of this.listeners) listener();
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
    for (const listener of this.listeners) listener();
  }

  highlightBody(index: number | null): void {
    this.patch({ highlightedBodyIndex: index });
    for (const listener of this.listeners) listener();
  }

  /** Save the current project as a downloadable .fdraft file.
   *
   *  wu2v: the file is a ZIP container (document.json + embedded uploaded
   *  font bytes + manifest) so binary assets survive a reload. Legacy JSON
   *  `.fdraft` files still open (the loader sniffs the ZIP magic bytes).
   *  The bug-report path uses the same ZIP — see exportProjectFdraft. */
  saveProjectToFile(filename = "project.fdraft"): void {
    if (typeof document === "undefined") return;
    const project = this.buildPersistableProject();
    const assets = this.collectProjectAssets(project);
    const zip = serializeProjectToZip(project, assets);
    // Copy into a fresh ArrayBuffer so Blob doesn't retain the WASM/heap view.
    const blob = new Blob([zip.slice()], { type: "application/zip" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = filename;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    URL.revokeObjectURL(url);
  }

  /** MT-A: load a project from a .fdraft JSON string. Kept for the
   *  report/tests/legacy callers; routes through {@link loadProjectFromBytes}
   *  so a JSON string and a ZIP file share one load path (the byte loader
   *  sniffs the magic bytes and treats non-ZIP bytes as JSON text).
   *  Returns `false` on malformed input. */
  loadProjectFromJson(json: string): boolean {
    return this.loadProjectFromBytes(new TextEncoder().encode(json));
  }

  /** wu2v: load a project from raw `.fdraft` bytes — a ZIP container or
   *  legacy JSON (auto-detected). Re-registers any embedded uploaded fonts
   *  so they render again, then applies the project. Returns `false` on
   *  malformed input. */
  loadProjectFromBytes(bytes: Uint8Array | ArrayBuffer): boolean {
    const u8 = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
    const parsed = parseProjectBytes(u8);
    if (!parsed) return false;
    logUserAction("doc", "load-project", { bytes: u8.length });
    // Re-register embedded uploaded fonts before applying the project so the
    // font cache is warm when the scene regenerates its text geometry.
    for (const [id, { bytes: fontBytes }] of parsed.assets.fonts) {
      try {
        registerUploadedFontFromBytes(id, fontBytes);
      } catch (err) {
        // eslint-disable-next-line no-console
        console.error("fielddraft: embedded font re-register failed", id, err);
      }
    }
    return this.applyLoadedProject(parsed.project);
  }

  /** Common tail of the load paths: reseed ids, rebuild scripts, load the
   *  active/first part doc, replace project state. Returns `false` only if
   *  the project shape is somehow unusable. */
  private applyLoadedProject(project: Project): boolean {
    reseedTabIds(project.tabs);

    // Rebuild the kernel's scripts from scratch.
    for (const r of this.kernel.listScripts()) this.kernel.removeScript(r.id);
    for (const r of projectScripts(project)) this.kernel.upsertScript(r);

    const activeTab = project.tabs.find((t) => t.id === project.activeTabId);
    let partTab =
      activeTab && activeTab.kind === "part"
        ? activeTab
        : project.tabs.find(
            (t): t is import("./project").PartTab => t.kind === "part",
          );
    if (!partTab) {
      partTab = createPartTab("Part 1");
      project.tabs.push(partTab);
    }
    this.kernel.loadJson(
      injectScripts(partTab.documentJson, projectScripts(project)),
    );
    this.loadedPartTabId = partTab.id;

    this.state = {
      ...this.state,
      project,
      assembly:
        activeTab && activeTab.kind === "assembly"
          ? { active: true, document: activeTab.document, selectedPartId: null }
          : INACTIVE_ASSEMBLY,
      // Ticket 5zd3: loading a project replaces the scene wholesale — drop
      // any active sketch session so a stale sketch view can't survive.
      sketchSession: INACTIVE_SKETCH_SESSION,
    };
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

  /** WP3: the live workspace serialized as the full multi-tab Project blob —
   *  the same payload localStorage and `.fdraft` carry. This is what a
   *  server-side project's `document` field stores, so opening a server
   *  project restores the WHOLE workspace (all tabs), not just one part. */
  serializeProjectJson(): string {
    return this.serializeProject();
  }

  /** WP3: load a server project into the live workspace and bind it for sync.
   *  `documentJson` is the Project blob previously saved via
   *  {@link serializeProjectJson}. A freshly-created server project carries an
   *  empty document (`{}` / `""`) — that's not a valid Project blob, so we
   *  start a fresh default workspace and bind the id instead of failing. Marks
   *  the session clean (just-loaded), so the next edit flips it to "dirty".
   *  Returns `false` only on a genuinely malformed (non-empty) blob. */
  loadServerProject(id: string, documentJson: string): boolean {
    const trimmed = documentJson.trim();
    const isEmpty = trimmed === "" || trimmed === "{}" || trimmed === "null";
    if (isEmpty) {
      this.newProject();
    } else if (!this.loadProjectFromJson(documentJson)) {
      return false;
    }
    this.state = {
      ...this.state,
      projectId: id,
      syncState: "clean",
    };
    for (const listener of this.listeners) listener();
    return true;
  }

  /** WP3: detach from any server project, returning to guest/offline mode
   *  (e.g. after the bound project is deleted, or on logout). The live
   *  workspace document is left untouched. */
  detachServerProject(): void {
    if (this.state.projectId === null && this.state.syncState === "offline") {
      return;
    }
    this.state = {
      ...this.state,
      projectId: null,
      syncState: "offline",
    };
    for (const listener of this.listeners) listener();
  }

  /** M12: sync the current workspace to the bound server project. No-op in
   *  offline mode. Sends the full Project blob (see
   *  {@link serializeProjectJson}). */
  async syncToServer(): Promise<boolean> {
    if (!this.state.projectId || this.state.syncState === "offline") {
      return false;
    }
    this.patch({ syncState: "syncing" });
    for (const listener of this.listeners) listener();
    try {
      const res = await fetch(`/api/projects/${this.state.projectId}`, {
        method: "PATCH",
        credentials: "include",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          document: JSON.parse(this.serializeProject()),
        }),
      });
      if (!res.ok) throw new Error(`sync failed: ${res.status}`);
      this.patch({ syncState: "clean" });
      for (const listener of this.listeners) listener();
      return true;
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: sync failed", err);
      this.patch({ syncState: "error" });
      for (const listener of this.listeners) listener();
      return false;
    }
  }

  /** MT-A: reset to a fresh default project (one empty part tab). */
  newProject(): void {
    const project = createDefaultProject();
    reseedTabIds(project.tabs);
    for (const r of this.kernel.listScripts()) this.kernel.removeScript(r.id);
    const partTab = project.tabs[0] as import("./project").PartTab;
    this.kernel.loadJson(injectScripts(partTab.documentJson, []));
    this.loadedPartTabId = partTab.id;
    this.state = {
      ...this.state,
      project,
      assembly: INACTIVE_ASSEMBLY,
      // Ticket 5zd3: a fresh project must show no sketch view — drop any
      // lingering session (e.g. after deleting the open sketch the session
      // could otherwise survive into the new empty project).
      sketchSession: INACTIVE_SKETCH_SESSION,
    };
    this.commit(new Set(), null);
    this.autosave.flush();
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
    for (const listener of this.listeners) listener();
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
    for (const listener of this.listeners) listener();
  }

  /** stv8: drop one measured element (panel "×" button). */
  removeMeasureEntity(el: PickedElement): void {
    this.patch({
      measureSelection: this.state.measureSelection.filter(
        (e) => !(e.kind === el.kind && e.idHash === el.idHash),
      ),
    });
    for (const listener of this.listeners) listener();
  }

  clearMeasureSelection(): void {
    this.patch({ measureSelection: [] });
    for (const listener of this.listeners) listener();
  }

  toggleFilletMode(): void {
    this.patch({
      filletMode: !this.state.filletMode,
      thickenMode: false,
      measureMode: false,
      measureSelection: [],
      mateMode: false,
    });
    for (const listener of this.listeners) listener();
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
    for (const listener of this.listeners) listener();
  }

  toggleMateMode(): void {
    this.patch({
      mateMode: !this.state.mateMode,
      filletMode: false,
      thickenMode: false,
      measureMode: false,
      measureSelection: [],
    });
    for (const listener of this.listeners) listener();
  }

  /** Toggle visibility of all committed mate-connector triads (Onshape's `K`
   *  shortcut). Pure UI state — does not touch the document. */
  toggleMateConnectorVisibility(): void {
    this.patch({ mateConnectorsHidden: !this.state.mateConnectorsHidden });
    for (const listener of this.listeners) listener();
  }

  // ── P1: Assembly management ────────────────────────────────────

  /** Single write path for assembly-document edits. Writes `next` into the
   *  active assembly tab (the source of truth) AND mirrors it into the
   *  derived `state.assembly` read-view (same object reference), then
   *  notifies + schedules autosave. No-op if the active tab isn't an
   *  assembly tab. */
  private updateAssemblyDocument(next: AssemblyDocument): void {
    const activeId = this.state.project.activeTabId;
    const tabs = this.state.project.tabs.map((t) =>
      t.id === activeId && t.kind === "assembly" ? { ...t, document: next } : t,
    );
    this.state = {
      ...this.state,
      project: { ...this.state.project, tabs },
      assembly: { ...this.state.assembly, document: next },
    };
    this.notify();
    this.autosave.schedule(this.serializeProject());
  }

  /** MT-A: leave the active assembly tab by switching to the first
   *  part tab. Drives the Outliner's "exit assembly" button. */
  exitAssembly(): void {
    const partTab = this.state.project.tabs.find((t) => t.kind === "part");
    if (partTab) this.setActiveTab(partTab.id);
  }

  /** Import a `.fdraft` part into the active assembly. Accepts a document
   *  JSON string or raw `.fdraft` bytes (wu2v: ZIP container → its
   *  `document.json` is extracted; legacy JSON bytes pass through). */
  addPartToAssembly(
    source: string | Uint8Array | ArrayBuffer,
    name?: string,
  ): void {
    const asm = this.state.assembly;
    if (!asm.active) return;

    let documentJson: string;
    if (typeof source === "string") {
      documentJson = source;
    } else {
      const u8 = source instanceof Uint8Array ? source : new Uint8Array(source);
      const isZip = u8.length >= 2 && u8[0] === 0x50 && u8[1] === 0x4b;
      if (isZip) {
        const parsed = parseProjectBytes(u8);
        if (!parsed) return;
        documentJson = serializeProjectJson(parsed.project);
      } else {
        documentJson = new TextDecoder().decode(u8);
      }
    }

    const id = allocateAssemblyId();
    // Derive the default name from the highest existing "Part N" so removing a
    // part and adding another doesn't reuse a name that's still in use.
    const maxPartNumber = asm.document.parts.reduce((max, p) => {
      const m = /^Part (\d+)$/.exec(p.name);
      return m ? Math.max(max, Number(m[1])) : max;
    }, 0);
    const partName = name ?? `Part ${maxPartNumber + 1}`;

    const meshBodies: AssemblyMeshedBody[] = [];
    const mateConnectors: import("./assembly").MateConnectorDef[] = [];
    const savedJson = this.kernel.toJson();
    const syncChanged = this.kernel.setSyncMeshing(true);
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
      // C2: mate-connector CSYS is resolved by the kernel from the
      // face query during the regeneration triggered by loadJson above.
      for (const c of this.kernel.resolvedMateConnectors()) {
        mateConnectors.push({
          featureId: c.id ?? 0,
          origin: [c.origin[0], c.origin[1], c.origin[2]],
          normal: [c.normal[0], c.normal[1], c.normal[2]],
        });
      }
    } catch {
      // Meshing failed — add the part with empty meshes
    } finally {
      // Restore the live document. If this ever throws the main kernel state
      // would be lost, so surface it loudly rather than swallowing it.
      try {
        this.kernel.loadJson(savedJson);
      } catch (err) {
        console.error(
          "fielddraft: failed to restore document after addPartToAssembly",
          err,
        );
      }
      if (syncChanged) this.kernel.setSyncMeshing(false);
    }

    const newPart: AssemblyPartInstance = {
      id,
      name: partName,
      documentJson,
      transform: IDENTITY_TRANSFORM,
      meshBodies,
      mateConnectors,
    };

    this.updateAssemblyDocument({
      ...asm.document,
      parts: [...asm.document.parts, newPart],
    });
  }

  removePartFromAssembly(partId: string): void {
    const asm = this.state.assembly;
    if (!asm.active) return;
    this.updateAssemblyDocument({
      ...asm.document,
      parts: asm.document.parts.filter((p) => p.id !== partId),
      mates: asm.document.mates.filter(
        (m) => m.connectorA.partId !== partId && m.connectorB.partId !== partId,
      ),
    });
    if (asm.selectedPartId === partId) this.selectAssemblyPart(null);
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
    this.updateAssemblyDocument({
      ...asm.document,
      parts: asm.document.parts.map((p) =>
        p.id === partId ? { ...p, transform: { translate, rotate } } : p,
      ),
    });
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
    this.updateAssemblyDocument({ ...newDoc, parts: solved });
  }

  removeMate(mateId: string): void {
    const asm = this.state.assembly;
    if (!asm.active) return;
    this.updateAssemblyDocument({
      ...asm.document,
      mates: asm.document.mates.filter((m) => m.id !== mateId),
    });
  }

  setMateAngle(mateId: string, angleDeg: number): void {
    const asm = this.state.assembly;
    if (!asm.active) return;
    const newMates = asm.document.mates.map((m) =>
      m.id === mateId ? { ...m, angle: angleDeg } : m,
    );
    const newDoc = { ...asm.document, mates: newMates };
    const solved = solveAssembly({ ...newDoc, parts: asm.document.parts });
    this.updateAssemblyDocument({ ...newDoc, parts: solved });
  }

  setAssemblyScript(source: string): void {
    const asm = this.state.assembly;
    if (!asm.active) return;
    this.updateAssemblyDocument({ ...asm.document, script: source });
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

    const result = executeScript(
      asm.document.script,
      params,
      time,
      dt,
      currentAngles,
    );

    if (!result.success) return result.error ?? "script error";

    if (result.jointUpdates.size > 0) {
      let newMates = [...asm.document.mates];
      for (const [mateId, angle] of result.jointUpdates) {
        newMates = newMates.map((m) => (m.id === mateId ? { ...m, angle } : m));
      }
      const newDoc = { ...asm.document, mates: newMates };
      const solved = solveAssembly({ ...newDoc, parts: asm.document.parts });
      this.updateAssemblyDocument({ ...newDoc, parts: solved });
    }

    return null;
  }

  exportR3F(): void {
    if (!this.state.assembly.active) return;
    import("./exportR3F")
      .then(({ downloadR3FExport }) => {
        downloadR3FExport(this.state.assembly.document);
      })
      .catch((err) => {
        console.error("fielddraft: R3F export failed", err);
      });
  }

  async publishProject(): Promise<string | null> {
    if (!this.state.projectId) return null;
    try {
      const resp = await fetch(
        `/api/projects/${this.state.projectId}/publish`,
        {
          method: "POST",
          credentials: "include",
        },
      );
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
    this.patch({ ikMode: !this.state.ikMode });
    this.notify();
  }

  runIK(target: IKTarget): void {
    const asm = this.state.assembly;
    if (!asm.active) return;
    const solved = solveIK(asm.document, target);
    if (!solved) return;
    const newDoc = { ...asm.document, mates: solved };
    const parts = solveAssembly({ ...newDoc, parts: asm.document.parts });
    this.updateAssemblyDocument({ ...newDoc, parts });
  }

  setMateOffset(mateId: string, offsetMm: number): void {
    const asm = this.state.assembly;
    if (!asm.active) return;
    const newMates = asm.document.mates.map((m) =>
      m.id === mateId ? { ...m, offset: offsetMm } : m,
    );
    const newDoc = { ...asm.document, mates: newMates };
    const solved = solveAssembly({ ...newDoc, parts: asm.document.parts });
    this.updateAssemblyDocument({ ...newDoc, parts: solved });
  }

  setMateJointLimits(mateId: string, minAngle: number, maxAngle: number): void {
    const asm = this.state.assembly;
    if (!asm.active) return;
    this.updateAssemblyDocument({
      ...asm.document,
      mates: asm.document.mates.map((m) =>
        m.id === mateId ? { ...m, minAngle, maxAngle } : m,
      ),
    });
  }

  selectAll(): void {
    const all = new Set(this.state.features.map((f) => f.id));
    this.patch({ selection: all, selectionAnchor: null });
    for (const listener of this.listeners) listener();
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
    for (const listener of this.listeners) listener();
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
    this.updateSceneBounds();
    for (const listener of this.listeners) listener();
  }

  /** Publish the scene's world AABB centre + bounding radius so the viewport
   *  can adapt the camera near/far planes to it (ticket lbzq — bodies vanished
   *  past a fixed far plane on zoom-out). Cheap (9 floats from the kernel) and
   *  only runs on state changes, never per frame. */
  private updateSceneBounds(): void {
    try {
      const a = this.kernel.sceneAabb();
      const radius = 0.5 * Math.hypot(a.size[0], a.size[1], a.size[2]);
      publishSceneBounds(
        radius > 0
          ? {
              center: [
                (a.min[0] + a.max[0]) / 2,
                (a.min[1] + a.max[1]) / 2,
                (a.min[2] + a.max[2]) / 2,
              ],
              radius,
            }
          : null,
      );
    } catch {
      // Kernel not ready (mid-load) — keep the previous bounds.
    }
  }

  setLoading(phase: string | null): void {
    this.patch({ loading: phase ? { phase } : null });
    this.notify();
  }

  /**
   * Enter sketch mode against a given plane query. K6b.2 accepts
   * a plane query as a plain string (placeholder until K6b.3
   * wires the face-pick path); calling this while a session is
   * already active is a no-op so the viewport can't stack
   * sessions on top of each other.
   */
  private updateSketch(next: SketchSessionState): void {
    if (next === this.state.sketchSession) return;
    // r3ci: a structural sub-spine change within an ongoing session is a
    // user edit — checkpoint it for sketch undo. Enter/exit transitions
    // (active flips) reset history separately and must not checkpoint.
    const cur = this.state.sketchSession;
    if (cur.active && next.active && next.subCalls !== cur.subCalls) {
      this.pushSketchUndoCheckpoint();
    }
    this.patch({ sketchSession: next });
    this.notify();
  }

  enterSketch(planeQuery: string): void {
    if (this.state.sketchSession.active) return;
    logUserAction("sketch", "enter", { plane: planeQuery });
    // Ticket d9o9 (Onshape style): materialise the sketch as a real feature
    // immediately so it shows up in the tree the moment you start drawing —
    // not only after Finish. We then edit it in place (the editingFeatureId
    // path); Finish keeps it (even empty) and Cancel deletes it.
    let id: number;
    try {
      id = this.kernel.addSketch(planeQuery, []);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: enterSketch failed to create sketch", err);
      this.setTransientError(formatKernelError(err, "Could not start sketch"));
      return;
    }
    this.resetSketchHistory(); // r3ci: fresh session, fresh history
    this.commit(new Set([id]), id); // surface + select the new sketch row
    this.updateSketch(
      reduceEnter(this.state.sketchSession, planeQuery, {
        editingFeatureId: id,
        createdThisSession: true,
      }),
    );
  }

  /**
   * Ticket d9o9: Cancel (✗) a sketch session. When the session created its
   * feature on entry, the cancel removes that fresh sketch entirely (it was
   * never confirmed); re-editing an existing sketch just discards the
   * in-session edits and leaves the committed feature untouched.
   */
  cancelSketch(): void {
    const session = this.state.sketchSession;
    const createdId = session.createdThisSession
      ? session.editingFeatureId
      : null;
    this.exitSketch();
    if (createdId !== null) {
      this.removeFeature(createdId);
    }
  }

  /**
   * Ticket yx6a: re-anchor the ACTIVE session's sketch plane. The
   * plane is an ordinary feature property now — the session panel's
   * QueryWidget calls this when the user picks a face (or types a
   * datum query). Committed (u,v) sub-calls keep their coordinates
   * and re-project onto the new plane.
   */
  setSketchPlane(planeQuery: string): void {
    this.updateSketch(reduceSetPlane(this.state.sketchSession, planeQuery));
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
    // Round-trip the committed plane query (ticket yx6a): the real plane
    // lives in the FeatureCall params; falling back to the XY placeholder
    // silently re-planed every edited sketch.
    let planeQuery = "qCreatedBy(origin_plane_xy)";
    try {
      const params = this.kernel.getFeatureCallParams(featureId);
      if (typeof params.plane === "string" && params.plane.length > 0) {
        planeQuery = params.plane;
      }
    } catch {
      // params unavailable — keep the XY fallback
    }
    this.resetSketchHistory(); // r3ci: re-opened sketch starts fresh history
    this.updateSketch(
      reduceEnter(this.state.sketchSession, planeQuery, {
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
    logUserAction("sketch", "exit");
    this.resetSketchHistory(); // r3ci
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
    logUserAction("sketch", "tool", { tool: typeName });
    this.updateSketch(
      reduceSelect(this.state.sketchSession, typeName, initialDraft),
    );
    // F24-full: preload the text font on Text-tool activation so the first
    // placement click has a parsed font ready (load is async).
    if (typeName === "sketch_text") {
      void this.ensureTextFontLoaded();
    }
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
    logUserAction("sketch", "draw", { calls: calls.map((c) => c.type) });
    this.pushSketchUndoCheckpoint(); // r3ci
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
   * F22: parse an SVG / DXF file's text into sketch sub-calls and append
   * them to the active session, then re-solve. Returns the number of
   * sub-calls imported (0 when nothing parsed or no session is active),
   * so the caller can surface a hint on an empty result. SVG coordinates
   * are y-flipped (y-down → y-up); DXF passes through unflipped. Curves
   * are tessellated into `sketch_line` chains.
   */
  importVectorSubCalls(text: string, filename: string): number {
    if (!this.state.sketchSession.active) return 0;
    let calls: ReadonlyArray<{ type: string; params: Record<string, unknown> }>;
    try {
      calls = importVectorToSubCalls(text, filename);
    } catch {
      return 0;
    }
    if (calls.length === 0) return 0;
    this.pushSketchSubCalls(calls);
    void solveAndReplace(this);
    return calls.length;
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
   * F18 (click-to-edit dimension): set the committed line at spine
   * `index` to exactly `lengthMm`. The line keeps its start point and
   * direction; the typed value also lands in a DRIVEN distance
   * constraint between the line's endpoints (replacing an existing one
   * from a prior edit) so the dimension stays attached. Re-solves.
   * Returns false when the edit is degenerate (see buildLineLengthEdit).
   */
  setSketchLineLength(index: number, lengthMm: number): boolean {
    const session = this.state.sketchSession;
    if (!session.active) return false;
    const edit = buildLineLengthEdit(session.subCalls, index, lengthMm);
    if (!edit) return false;
    let next = reduceReplaceAt(session, index, [edit.line]);
    if (edit.existingConstraintIndex !== null) {
      next = reduceReplaceAt(next, edit.existingConstraintIndex, [
        edit.constraint,
      ]);
    } else {
      next = { ...next, subCalls: [...next.subCalls, edit.constraint] };
    }
    this.updateSketch(next);
    void solveAndReplace(this);
    return true;
  }

  /**
   * F20: stash the latest solve diagnostics on the active session so
   * the Outliner constraints panel can flag conflicting / redundant
   * constraint rows. No-op when no session is active. Called by
   * `solveAndReplace` after a successful solve; the snapshot-identity
   * guard there ensures a stale solve can't write outdated flags.
   */
  setSketchDiagnostics(diagnostics: SketchDiagnostics): void {
    if (!this.state.sketchSession.active) return;
    this.state = {
      ...this.state,
      sketchSession: {
        ...this.state.sketchSession,
        diagnostics,
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
   * Delete the sketch elements currently selected in select mode
   * (Delete/Backspace in a sketch session). Maps the selected entity
   * ids to their owning geometry sub-calls and drops those rows
   * (descending, so earlier indices stay valid); constraints that
   * reference the removed entities are cascaded by the reducer.
   * Returns true when something was deleted.
   */
  deleteSketchSelection(): boolean {
    const session = this.state.sketchSession;
    if (!session.active || session.selection.length === 0) return false;
    const owner = entityOwnerSubCalls(session.subCalls);
    const indices = [
      ...new Set(
        session.selection
          .map((id) => owner.get(id))
          .filter((i): i is number => i !== undefined),
      ),
    ].sort((a, b) => b - a);
    if (indices.length === 0) return false;
    let next = session;
    for (const i of indices) next = reduceRemoveAt(next, i);
    this.updateSketch(next);
    void solveAndReplace(this);
    return true;
  }

  /**
   * Trim: cut the curve under `click` at its nearest intersections with
   * the other sketch geometry and drop the picked piece. The clicked
   * line/arc is replaced in place by the 0–2 surviving pieces (an
   * intersection-free curve is deleted outright); downstream constraint
   * refs are rewritten by [`replaceSketchSubCallAt`] and the sketch is
   * re-solved. `tolMm` is the pick radius in sketch millimetres. No-op
   * when nothing trimmable is under the cursor.
   */
  trimSketchAt(click: [number, number], tolMm: number): void {
    const session = this.state.sketchSession;
    if (!session.active) return;
    const result = computeTrim(session.subCalls, click, tolMm);
    if (!result) return;
    this.updateSketch(reduceReplaceAt(session, result.index, result.newCalls));
    void solveAndReplace(this);
  }

  /**
   * F13: Split — the keep-both sibling of Trim. The curve at
   * `targetIndex` is cut at `click` into two sub-curves that meet at the
   * click point; nothing is deleted. The clicked line/arc is replaced in
   * place by its two pieces (each carrying the target's construction
   * flag); downstream constraint refs are rewritten by
   * [`replaceSketchSubCallAt`] and the sketch is re-solved. No-op when
   * the target isn't a simple line/arc, or the click is at an endpoint /
   * on a full circle (see sketchSplit.ts). `click` is in sketch mm.
   */
  splitSketchAt(
    targetIndex: number,
    click: [number, number],
    _tolMm: number,
  ): void {
    const session = this.state.sketchSession;
    if (!session.active) return;
    const target = session.subCalls[targetIndex];
    if (!target) return;
    const cv = curveOfSubCall(target);
    if (!cv) return; // composites / unsupported kinds: keep v1 simple

    const pieces = splitCurveAt(cv, click);
    if (!pieces) return;

    const carry: Record<string, unknown> =
      target.params.construction !== undefined
        ? { construction: target.params.construction }
        : {};
    this.updateSketch(
      reduceReplaceAt(session, targetIndex, [
        subCallOfCurve(pieces[0], carry),
        subCallOfCurve(pieces[1], carry),
      ]),
    );
    void solveAndReplace(this);
  }

  /**
   * F12: Extend (complement of Trim). Lengthen the line sub-call at
   * `targetIndex` along its own direction until it meets the nearest other
   * sketch curve. The endpoint nearer `click` is the grabbed end that
   * moves outward; the far endpoint is preserved. The lengthened line
   * replaces the target in place (carrying its construction flag);
   * downstream constraint refs are rewritten by [`replaceSketchSubCallAt`]
   * and the sketch is re-solved. No-op when the target isn't a line or
   * nothing lies in the forward extension. `click` is in sketch mm.
   */
  extendSketchAt(
    targetIndex: number,
    click: [number, number],
    _tolMm: number,
  ): void {
    const session = this.state.sketchSession;
    if (!session.active) return;
    const target = session.subCalls[targetIndex];
    if (!target) return;
    const line = curveOfSubCall(target);
    if (!line || line.kind !== "line") return;

    // Every edge of every OTHER sub-call is a curve to extend into.
    const others = session.subCalls.flatMap((call, i) =>
      i === targetIndex ? [] : edgesOfSubCall(call),
    );

    const extended = extendLine(line, click, others);
    if (!extended) return;

    const carry: Record<string, unknown> =
      target.params.construction !== undefined
        ? { construction: target.params.construction }
        : {};
    this.updateSketch(
      reduceReplaceAt(session, targetIndex, [subCallOfCurve(extended, carry)]),
    );
    void solveAndReplace(this);
  }

  /**
   * F14: interactive Mirror. Reflect the currently-selected source
   * geometry across the axis line at sub-call index `axisLineIndex` and
   * append the mirrored copies as ordinary `sketch_line` /
   * `sketch_circle` / `sketch_arc` sub-calls (so they stay editable —
   * this is a UI-side rewrite, no kernel mirror tool).
   *
   * Selection → sub-call rule: mirror every SIMPLE geometry sub-call
   * (line / circle / arc, i.e. `curveOfSubCall` non-null) that owns at
   * least one selected entity, EXCLUDING the axis line itself. The axis
   * line's two solved endpoints (read from `buildDraftSketchState`)
   * define the infinite reflection axis. Each mirrored copy carries the
   * source's construction flag. After appending, the sketch is re-solved
   * and the selection is cleared.
   *
   * No-op when the axis isn't a line, the selection is empty, or nothing
   * mirrorable remains after excluding the axis.
   */
  mirrorSketchSelection(axisLineIndex: number): void {
    const session = this.state.sketchSession;
    if (!session.active) return;
    if (session.selection.length === 0) return;

    // Map the entity-id selection back to the simple-geometry sub-calls
    // that produced it, dropping the axis line and any non-simple kinds.
    const sources = subCallsForSelection(
      session.subCalls,
      session.selection,
    ).filter(
      (i) => i !== axisLineIndex && isMirrorableSubCall(session.subCalls[i]),
    );
    const appended = this.mirrorSubCallsAcrossAxis(axisLineIndex, sources);
    if (!appended) return;
    this.updateSketch(reduceClearSelection(this.state.sketchSession));
    void solveAndReplace(this);
  }

  /**
   * Ticket ps21: live "pick-to-apply" Mirror. Reflect the single sketch
   * entity `entityId` across the axis line at `axisLineIndex` and append
   * the mirrored copy immediately — no Enter, no selection step. Each
   * click in the viewport while the Mirror tool's axis is set calls this,
   * so the result appears the instant the user clicks the source object
   * (Onshape style). No-op when the entity maps to no simple curve, is the
   * axis itself, or the axis isn't a line.
   */
  mirrorSketchEntityAt(axisLineIndex: number, entityId: number): void {
    const session = this.state.sketchSession;
    if (!session.active) return;
    const sources = subCallsForSelection(session.subCalls, [entityId]).filter(
      (i) => i !== axisLineIndex && isMirrorableSubCall(session.subCalls[i]),
    );
    if (sources.length === 0) return;
    if (this.mirrorSubCallsAcrossAxis(axisLineIndex, sources)) {
      void solveAndReplace(this);
    }
  }

  /**
   * Shared core for both Mirror entry points: reflect every sub-call in
   * `sourceIndices` across the line at `axisLineIndex` and append the
   * mirrored copies (carrying each source's construction flag). Returns
   * `true` when at least one mirrored copy was pushed, `false` on a no-op
   * (axis not a line, no mirrorable sources). Does NOT touch the selection
   * or trigger a re-solve — callers decide.
   */
  private mirrorSubCallsAcrossAxis(
    axisLineIndex: number,
    sourceIndices: ReadonlyArray<number>,
  ): boolean {
    const session = this.state.sketchSession;
    const axisCall = session.subCalls[axisLineIndex];
    if (!axisCall) return false;
    const axisCurve = curveOfSubCall(axisCall);
    if (!axisCurve || axisCurve.kind !== "line") return false;
    const axisA = axisCurve.a;
    const axisB = axisCurve.b;

    const mirrored: Array<{ type: string; params: Record<string, unknown> }> =
      [];
    for (const i of sourceIndices) {
      // Ticket o6hx: points, simple curves and composites (rectangle /
      // polygon) all reflect via the pure `mirrorSubCall` SSOT.
      mirrored.push(...mirrorSubCall(session.subCalls[i], axisA, axisB));
    }
    if (mirrored.length === 0) return false;
    this.pushSketchSubCalls(mirrored);
    return true;
  }

  /** F10: set the radius (mm) the two-click `sketch_fillet` tool uses to
   *  round the next corner. No-op for a non-finite / non-positive value. */
  setSketchFilletRadius(radius: number): void {
    if (!Number.isFinite(radius) || radius <= 0) return;
    this.patch({ sketchFilletRadius: radius });
    for (const listener of this.listeners) listener();
  }

  /**
   * F10: round the corner between the two line sub-calls at
   * `firstIndex` and `secondIndex` with a tangent arc of `radius` mm.
   * Reads both lines' solved geometry, computes the fillet via
   * [`filletLines`], then rewrites the spine: the first line is replaced
   * by its trimmed remainder in place, the second line is replaced by
   * its trimmed remainder in place, and the rounding arc is appended.
   * Downstream constraint refs are rewritten by [`replaceSketchSubCallAt`]
   * and the sketch is re-solved. No-op when either pick isn't a line,
   * they're the same call, or no fillet is geometrically possible
   * (parallel, radius too large, no shared corner).
   */
  filletSketchAt(
    firstIndex: number,
    secondIndex: number,
    radius: number,
  ): void {
    const session = this.state.sketchSession;
    if (!session.active) return;
    if (firstIndex === secondIndex) return;
    const c1 = session.subCalls[firstIndex];
    const c2 = session.subCalls[secondIndex];
    if (!c1 || !c2) return;
    const cv1 = curveOfSubCall(c1);
    const cv2 = curveOfSubCall(c2);
    if (!cv1 || !cv2 || cv1.kind !== "line" || cv2.kind !== "line") return;

    const fillet = filletLines(cv1, cv2, radius);
    if (!fillet) return;

    // Carry each line's construction flag onto its trimmed remainder; the
    // arc inherits the first line's flag (a fillet of two construction
    // lines stays construction).
    const carry = (params: Record<string, unknown>): Record<string, unknown> =>
      params.construction !== undefined
        ? { construction: params.construction }
        : {};

    // Plan item 4: tag the two source lines with a STABLE id (re-use one
    // already present, else mint a fresh `fd_id`) so the re-edit recipe can
    // resolve them by id after reorders. The arc carries the same two ids
    // plus the ORIGINAL (pre-trim) geometry, so a radius change re-derives
    // from `cv1`/`cv2` rather than the already-trimmed remainders.
    const idA = fdIdOf(c1) ?? nextFdId();
    const idB = fdIdOf(c2) ?? nextFdId();

    // Replace the two lines in place (each a single trimmed line), then
    // append the arc. Apply the higher index first so the lower index
    // stays valid across the first rewrite.
    const lo = Math.min(firstIndex, secondIndex);
    const hi = Math.max(firstIndex, secondIndex);
    const loCurve = lo === firstIndex ? fillet.trimmed1 : fillet.trimmed2;
    const hiCurve = hi === firstIndex ? fillet.trimmed1 : fillet.trimmed2;
    const loParams = lo === firstIndex ? c1.params : c2.params;
    const hiParams = hi === firstIndex ? c1.params : c2.params;
    const loId = lo === firstIndex ? idA : idB;
    const hiId = hi === firstIndex ? idA : idB;

    let next = reduceReplaceAt(session, hi, [
      subCallOfCurve(hiCurve, { ...carry(hiParams), [FD_ID_PARAM]: hiId }),
    ]);
    next = reduceReplaceAt(next, lo, [
      subCallOfCurve(loCurve, { ...carry(loParams), [FD_ID_PARAM]: loId }),
    ]);
    this.updateSketch(next);
    this.pushSketchSubCalls([
      subCallOfCurve(fillet.arc, {
        ...carry(c1.params),
        [FD_ID_PARAM]: nextFdId(),
        fillet: filletRecipe(idA, idB, cv1, cv2, radius),
      }),
    ]);
    void solveAndReplace(this);
  }

  /**
   * Plan item 4: re-edit a previously baked fillet's radius in place.
   * `arcSubCallId` is the stable `fd_id` of the recipe-tagged `sketch_arc`
   * (surfaced by the in-scene corner editor). Re-derives the rounding arc
   * + the two trimmed source lines from the recipe via [`reeditFilletRadius`]
   * and re-solves. No-op (returns false) when the arc isn't a tagged
   * fillet, a source line is gone, or the new radius is impossible.
   *
   * Known limitation (chained fillets): the recipe snapshots the source
   * lines at first creation, so re-editing a corner whose lines a later
   * corner also trimmed restores their first-creation length — see
   * `reeditFilletRadius`.
   */
  setFilletRadiusAt(arcSubCallId: string, newRadius: number): boolean {
    const session = this.state.sketchSession;
    if (!session.active) return false;
    if (!Number.isFinite(newRadius) || newRadius <= 0) return false;
    const rewritten = reeditFilletRadius(
      session.subCalls,
      arcSubCallId,
      newRadius,
    );
    if (!rewritten) return false;
    this.replaceSketchSubCalls(rewritten);
    void solveAndReplace(this);
    return true;
  }

  /** F11: set the setback distance (mm) the two-click `sketch_chamfer`
   *  tool uses to bevel the next corner. No-op for a non-finite /
   *  non-positive value. */
  setSketchChamferDistance(dist: number): void {
    if (!Number.isFinite(dist) || dist <= 0) return;
    this.patch({ sketchChamferDistance: dist });
    for (const listener of this.listeners) listener();
  }

  /**
   * F11: bevel the corner between the two line sub-calls at
   * `firstIndex` and `secondIndex` with a straight chamfer at setback
   * `dist` mm. Reads both lines' solved geometry, computes the chamfer
   * via [`chamferLines`], then rewrites the spine: the first line is
   * replaced by its trimmed remainder in place, the second line is
   * replaced by its trimmed remainder in place, and the connecting bevel
   * line is appended. Downstream constraint refs are rewritten by
   * [`replaceSketchSubCallAt`] and the sketch is re-solved. No-op when
   * either pick isn't a line, they're the same call, or no chamfer is
   * geometrically possible (parallel, setback too large, no shared
   * corner).
   */
  chamferSketchAt(firstIndex: number, secondIndex: number, dist: number): void {
    const session = this.state.sketchSession;
    if (!session.active) return;
    if (firstIndex === secondIndex) return;
    const c1 = session.subCalls[firstIndex];
    const c2 = session.subCalls[secondIndex];
    if (!c1 || !c2) return;
    const cv1 = curveOfSubCall(c1);
    const cv2 = curveOfSubCall(c2);
    if (!cv1 || !cv2 || cv1.kind !== "line" || cv2.kind !== "line") return;

    const chamfer = chamferLines(cv1, cv2, dist);
    if (!chamfer) return;

    // Carry each line's construction flag onto its trimmed remainder; the
    // bevel inherits the first line's flag (a chamfer of two construction
    // lines stays construction).
    const carry = (params: Record<string, unknown>): Record<string, unknown> =>
      params.construction !== undefined
        ? { construction: params.construction }
        : {};

    // Plan item 4: tag the two source lines with a STABLE id and the bevel
    // with the same two ids + ORIGINAL geometry, so the setback distance is
    // re-editable in-scene. Same scheme as `filletSketchAt`.
    const idA = fdIdOf(c1) ?? nextFdId();
    const idB = fdIdOf(c2) ?? nextFdId();

    // Replace the two lines in place (each a single trimmed line), then
    // append the bevel. Apply the higher index first so the lower index
    // stays valid across the first rewrite.
    const lo = Math.min(firstIndex, secondIndex);
    const hi = Math.max(firstIndex, secondIndex);
    const loCurve = lo === firstIndex ? chamfer.trimmed1 : chamfer.trimmed2;
    const hiCurve = hi === firstIndex ? chamfer.trimmed1 : chamfer.trimmed2;
    const loParams = lo === firstIndex ? c1.params : c2.params;
    const hiParams = hi === firstIndex ? c1.params : c2.params;
    const loId = lo === firstIndex ? idA : idB;
    const hiId = hi === firstIndex ? idA : idB;

    let next = reduceReplaceAt(session, hi, [
      subCallOfCurve(hiCurve, { ...carry(hiParams), [FD_ID_PARAM]: hiId }),
    ]);
    next = reduceReplaceAt(next, lo, [
      subCallOfCurve(loCurve, { ...carry(loParams), [FD_ID_PARAM]: loId }),
    ]);
    this.updateSketch(next);
    this.pushSketchSubCalls([
      subCallOfCurve(chamfer.line, {
        ...carry(c1.params),
        [FD_ID_PARAM]: nextFdId(),
        chamfer: chamferRecipe(idA, idB, cv1, cv2, dist),
      }),
    ]);
    void solveAndReplace(this);
  }

  /**
   * Plan item 4: re-edit a previously baked chamfer's setback distance in
   * place. `bevelSubCallId` is the stable `fd_id` of the recipe-tagged
   * `sketch_line` bevel. Re-derives the bevel + the two trimmed source
   * lines via [`reeditChamferDistance`] and re-solves. No-op (returns
   * false) when the line isn't a tagged chamfer, a source line is gone, or
   * the new distance is impossible. Same chained-corner limitation as the
   * fillet path.
   */
  setChamferDistanceAt(bevelSubCallId: string, newDistance: number): boolean {
    const session = this.state.sketchSession;
    if (!session.active) return false;
    if (!Number.isFinite(newDistance) || newDistance <= 0) return false;
    const rewritten = reeditChamferDistance(
      session.subCalls,
      bevelSubCallId,
      newDistance,
    );
    if (!rewritten) return false;
    this.replaceSketchSubCalls(rewritten);
    void solveAndReplace(this);
    return true;
  }

  /**
   * Tickets tqi4 / chfp: single-CLICK corner fillet. Resolve the corner under
   * the cursor — exploding a Polygon / N-gon / Rectangle into lines if needed —
   * then fillet the two line edges that meet there. No-op when no corner is
   * close enough.
   */
  filletCornerAt(point: [number, number], radius: number, tolMm: number): void {
    const session = this.state.sketchSession;
    if (!session.active) return;
    const r = resolveCornerFillet(session.subCalls, point, tolMm);
    if (!r) return;
    if (r.exploded) this.replaceSketchSubCalls(r.subCalls);
    this.filletSketchAt(r.i1, r.i2, radius);
  }

  /** Tickets tqi4 / chfp: single-CLICK corner chamfer (see filletCornerAt). */
  chamferCornerAt(point: [number, number], dist: number, tolMm: number): void {
    const session = this.state.sketchSession;
    if (!session.active) return;
    const r = resolveCornerFillet(session.subCalls, point, tolMm);
    if (!r) return;
    if (r.exploded) this.replaceSketchSubCalls(r.subCalls);
    this.chamferSketchAt(r.i1, r.i2, dist);
  }

  /** F15: set the distance (mm) the single-click `sketch_offset` tool
   *  uses to place a parallel copy. No-op for a non-finite / non-positive
   *  value. */
  setSketchOffsetDistance(dist: number): void {
    if (!Number.isFinite(dist) || dist <= 0) return;
    this.patch({ sketchOffsetDistance: dist });
    for (const listener of this.listeners) listener();
  }

  /**
   * F15 / F15-full: offset the whole connected CHAIN the clicked curve
   * belongs to, toward the side the click `clickPoint` lies on.
   *
   * From `targetIndex` the connected component of line/arc curves whose
   * endpoints coincide is walked ([`buildChain`]); every segment is
   * offset on a globally consistent side and the joints are reconciled —
   * concave corners trim to the support intersection, convex corners get
   * a rounding fillet arc of radius = distance, a closed loop also closes
   * the wrap-around joint ([`offsetChain`]). The result is appended as
   * ordinary `sketch_line` / `sketch_arc` sub-calls (the originals stay
   * put), each carrying its source's construction flag, then the sketch
   * is re-solved.
   *
   * A full circle (no endpoints) is never part of a chain and falls
   * through to the concentric single-curve [`offsetCurve`]; a lone
   * line/arc with no coincident neighbour is the degenerate length-1
   * chain. No-op when the target isn't a simple curve or no valid offset
   * exists (collapsed offset, degenerate geometry, bad distance).
   */
  offsetSketchAt(
    targetIndex: number,
    clickPoint: Vec2,
    distance: number,
  ): void {
    const session = this.state.sketchSession;
    if (!session.active) return;
    const target = session.subCalls[targetIndex];
    if (!target) return;
    const cv = curveOfSubCall(target);
    if (!cv) return;

    const carry = (params: Record<string, unknown>): Record<string, unknown> =>
      params.construction !== undefined
        ? { construction: params.construction }
        : {};

    // A full circle has no endpoints — offset it concentrically.
    if (cv.kind === "circle") {
      const offset = offsetCurve(cv, distance, clickPoint);
      if (!offset) return;
      this.pushSketchSubCalls([subCallOfCurve(offset, carry(target.params))]);
      void solveAndReplace(this);
      return;
    }

    // Build the connected chain of line/arc curves from the sub-spine,
    // keeping each sub-call's curve at its own index so the seed maps in.
    const curves = session.subCalls.map((c) => curveOfSubCall(c));
    const chain = buildChain(curves, targetIndex);
    if (!chain) return;
    const clickedPos = chain.indices.indexOf(targetIndex);
    if (clickedPos < 0) return;

    const offsets = offsetChain(chain, clickedPos, distance, clickPoint);
    if (!offsets || offsets.length === 0) return;

    // The offset copies inherit the clicked source's construction flag.
    const flags = carry(target.params);
    this.pushSketchSubCalls(offsets.map((o) => subCallOfCurve(o, flags)));
    void solveAndReplace(this);
  }

  /** F23-full: choose the spline family the `sketch_spline` tool draws. */
  setSketchSplineMode(mode: "interpolate" | "control"): void {
    this.patch({ sketchSplineMode: mode });
    for (const listener of this.listeners) listener();
  }

  /** F24: set the string the `sketch_text` tool emits. */
  setSketchTextValue(value: string): void {
    this.patch({ sketchTextValue: value });
    for (const listener of this.listeners) listener();
  }

  /** F24: set the cap height (mm) for the `sketch_text` tool. No-op for a
   *  non-finite / non-positive value. */
  setSketchTextHeight(height: number): void {
    if (!Number.isFinite(height) || height <= 0) return;
    this.patch({ sketchTextHeight: height });
    for (const listener of this.listeners) listener();
  }

  /** F24-full: select the font the `sketch_text` tool renders with and
   *  kick off its async load (no-op when already the current font). */
  setSketchTextFontId(id: string): void {
    if (this.state.sketchTextFontId === id) return;
    this.patch({
      sketchTextFontId: id,
      sketchTextFontReady: cachedFont(id) !== undefined,
    });
    for (const listener of this.listeners) listener();
    void this.ensureTextFontLoaded(id);
  }

  /**
   * F24-full: ensure the currently-selected (or given) text font is
   * fetched + parsed into the cache. Idempotent and safe to call on tool
   * activation / font change. Flips `sketchTextFontReady` when the load
   * settles. Uploaded fonts are already cached, so this returns fast.
   */
  async ensureTextFontLoaded(id = this.state.sketchTextFontId): Promise<void> {
    if (cachedFont(id) !== undefined) {
      if (
        this.state.sketchTextFontId === id &&
        !this.state.sketchTextFontReady
      ) {
        this.patch({ sketchTextFontReady: true });
        for (const listener of this.listeners) listener();
      }
      return;
    }
    try {
      await loadBundledFont(id);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: text font load failed", err);
      return;
    }
    // Only flip ready if the user hasn't switched fonts meanwhile.
    if (this.state.sketchTextFontId === id) {
      this.patch({ sketchTextFontReady: true });
      for (const listener of this.listeners) listener();
    }
  }

  /**
   * F24-full: parse an uploaded .ttf/.otf ArrayBuffer, register it under a
   * synthetic id and make it the active text font. Returns the new id, or
   * `null` when the buffer isn't a parseable font.
   */
  loadUploadedTextFont(name: string, buffer: ArrayBuffer): string | null {
    let id: string;
    try {
      const font = parseFontBuffer(buffer);
      // wu2v: retain the raw bytes so this font persists into saved
      // `.fdraft` ZIPs (previously the bytes were lost on reload).
      id = registerUploadedFont(name, font, new Uint8Array(buffer.slice(0)));
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: uploaded font parse failed", err);
      return null;
    }
    this.patch({ sketchTextFontId: id, sketchTextFontReady: true });
    for (const listener of this.listeners) listener();
    return id;
  }

  /**
   * F24-full: place the current text value as FILLED glyph sketch geometry
   * with its baseline-left at `origin`, using the selected font. Each glyph
   * becomes one `sketch_path` sub-call carrying its outer ring + holes (the
   * kernel fills with even-odd holes), then the sketch is re-solved. No-op
   * when no session is active, the font isn't loaded yet, or the text
   * produces no geometry (e.g. an empty / all-space value).
   */
  placeSketchText(origin: [number, number]): void {
    if (!this.state.sketchSession.active) return;
    const font = cachedFont(this.state.sketchTextFontId);
    if (!font) {
      // Font not ready — kick a load so a follow-up click works.
      void this.ensureTextFontLoaded();
      return;
    }
    // Plan item 5: tag every glyph with a `text` recipe (shared group id +
    // value/height/fontId/origin) so the placement is re-editable in-scene.
    // The kernel ignores the param and round-trips it (see sketchText.ts).
    const calls = textToSubCalls(font, this.state.sketchTextValue, {
      origin,
      height: this.state.sketchTextHeight,
      fontId: this.state.sketchTextFontId,
    });
    if (calls.length === 0) return;
    this.pushSketchSubCalls(calls);
    void solveAndReplace(this);
  }

  /**
   * Plan item 5: re-edit a previously committed text group in place.
   * `groupId` is the shared `text.group` id of the placement's glyph
   * sub-calls (surfaced by the in-scene text editor reopened over a baked
   * text). Re-derives the glyphs at the new value / height / font via
   * [`reeditTextGroup`] from the loaded font and re-solves, replacing the
   * old group's glyphs in place. Returns false (a graceful no-op) when no
   * session is active, the requested font isn't loaded yet, no glyph
   * carries the group, or the new text produces no geometry.
   *
   * Font availability: re-deriving needs the parsed font in the cache. If
   * it isn't ready we kick a load (same as `placeSketchText`) and bail —
   * the old geometry is kept; the caller can retry once the font settles.
   */
  updateSketchTextGroup(
    groupId: string,
    next: { value: string; height: number; fontId: string },
  ): boolean {
    const session = this.state.sketchSession;
    if (!session.active) return false;
    if (!Number.isFinite(next.height) || next.height <= 0) return false;
    const font = cachedFont(next.fontId);
    if (!font) {
      // Font not ready — kick a load so a follow-up edit works. Keep the
      // old geometry for now.
      void this.ensureTextFontLoaded(next.fontId);
      return false;
    }
    const rewritten = reeditTextGroup(session.subCalls, font, groupId, next);
    if (!rewritten) return false;
    this.replaceSketchSubCalls(rewritten);
    void solveAndReplace(this);
    return true;
  }

  /**
   * Ticket g91j: delete a previously committed text group in place.
   * `groupId` is the shared `text.group` id of the placement's glyph
   * sub-calls (surfaced by the in-scene text editor reopened over a baked
   * text). Drops every glyph carrying the group via [`removeTextGroup`] and
   * re-solves, replacing the spine in place. Returns false (a graceful no-op)
   * when no session is active or no glyph carries the group. Unlike
   * `updateSketchTextGroup` this needs no font — it only removes geometry — so
   * it deletes even while a font is mid-load. Called when the editor commits
   * an empty value over an existing text (edit mode → delete).
   */
  deleteSketchTextGroup(groupId: string): boolean {
    const session = this.state.sketchSession;
    if (!session.active) return false;
    const rewritten = removeTextGroup(session.subCalls, groupId);
    if (!rewritten) return false;
    this.replaceSketchSubCalls(rewritten);
    void solveAndReplace(this);
    return true;
  }

  /** F16: set the number of pattern instances (original + copies). Must be
   *  an integer ≥ 1; no-op for anything else. A value < 2 makes the
   *  pattern actions no-ops (no copies). */
  setSketchPatternCount(count: number): void {
    if (!Number.isFinite(count) || count < 1) return;
    this.patch({ sketchPatternCount: Math.round(count) });
    for (const listener of this.listeners) listener();
  }

  /** F16: set the per-step spacing (mm) for a linear pattern. May be
   *  negative (steps the opposite way); no-op for non-finite values. */
  setSketchPatternSpacing(spacing: number): void {
    if (!Number.isFinite(spacing)) return;
    this.patch({ sketchPatternSpacing: spacing });
    for (const listener of this.listeners) listener();
  }

  /** F16: set the total span (degrees) for a circular pattern. No-op for
   *  non-finite values. */
  setSketchPatternAngle(angleDeg: number): void {
    if (!Number.isFinite(angleDeg)) return;
    this.patch({ sketchPatternAngle: angleDeg });
    for (const listener of this.listeners) listener();
  }

  /**
   * F16: linear pattern of the current selection along `axis`.
   *
   * Maps the entity-id selection back to its simple-geometry sub-calls
   * (line / circle / arc, i.e. `curveOfSubCall` non-null), produces the
   * `count − 1` translated copies of each via [`patternLinear`] (per-step
   * spacing `(spacing, 0)` for X, `(0, spacing)` for Y), and appends them
   * as ordinary `sketch_line` / `sketch_circle` / `sketch_arc` sub-calls.
   * Each copy carries its source's construction flag. The originals stay
   * in place; the sketch is re-solved and the selection is kept.
   *
   * No-op when the selection is empty, nothing simple maps, or
   * `count < 2`.
   */
  linearPatternSketch(axis: "x" | "y"): void {
    const session = this.state.sketchSession;
    if (!session.active) return;
    if (session.selection.length === 0) return;

    const count = this.state.sketchPatternCount;
    const spacing = this.state.sketchPatternSpacing;
    const dx = axis === "x" ? spacing : 0;
    const dy = axis === "x" ? 0 : spacing;

    const sources = subCallsForSelection(
      session.subCalls,
      session.selection,
    ).filter((i) => curveOfSubCall(session.subCalls[i]) !== null);
    if (sources.length === 0) return;

    const carry = (params: Record<string, unknown>): Record<string, unknown> =>
      params.construction !== undefined
        ? { construction: params.construction }
        : {};

    const copies: Array<{ type: string; params: Record<string, unknown> }> = [];
    for (const i of sources) {
      const call = session.subCalls[i];
      const cv = curveOfSubCall(call);
      if (!cv) continue;
      for (const out of patternLinear([cv], count, dx, dy)) {
        copies.push(subCallOfCurve(out, carry(call.params)));
      }
    }
    if (copies.length === 0) return;

    this.pushSketchSubCalls(copies);
    void solveAndReplace(this);
  }

  /**
   * F16: circular pattern of the current selection about the sketch
   * origin `[0, 0]`, spanning `sketchPatternAngle` degrees.
   *
   * Maps the entity-id selection back to its simple-geometry sub-calls and
   * produces the `count − 1` rotated copies of each via
   * [`patternCircular`] — see that helper for the full-vs-partial step
   * rule (full revolution → 2π/count so copies don't overlap the
   * original; partial → totalAngle/(count−1), inclusive span). Each copy
   * is appended as an ordinary `sketch_line` / `sketch_circle` /
   * `sketch_arc` carrying its source's construction flag. The originals
   * stay in place; the sketch is re-solved and the selection is kept.
   *
   * No-op when the selection is empty, nothing simple maps, or
   * `count < 2`.
   */
  circularPatternSketch(): void {
    const session = this.state.sketchSession;
    if (!session.active) return;
    if (session.selection.length === 0) return;

    const count = this.state.sketchPatternCount;
    const angleRad = (this.state.sketchPatternAngle * Math.PI) / 180;

    const sources = subCallsForSelection(
      session.subCalls,
      session.selection,
    ).filter((i) => curveOfSubCall(session.subCalls[i]) !== null);
    if (sources.length === 0) return;

    const carry = (params: Record<string, unknown>): Record<string, unknown> =>
      params.construction !== undefined
        ? { construction: params.construction }
        : {};

    const copies: Array<{ type: string; params: Record<string, unknown> }> = [];
    for (const i of sources) {
      const call = session.subCalls[i];
      const cv = curveOfSubCall(call);
      if (!cv) continue;
      for (const out of patternCircular([cv], count, [0, 0], angleRad)) {
        copies.push(subCallOfCurve(out, carry(call.params)));
      }
    }
    if (copies.length === 0) return;

    this.pushSketchSubCalls(copies);
    void solveAndReplace(this);
  }

  /** F17: set the rotation angle (degrees) the `rotateSketchSelection`
   *  transform applies. No-op for non-finite values. */
  setSketchRotateAngle(angleDeg: number): void {
    if (!Number.isFinite(angleDeg)) return;
    this.patch({ sketchRotateAngle: angleDeg });
    for (const listener of this.listeners) listener();
  }

  /**
   * F17: translate the current selection by `(dx, dy)`.
   *
   * Maps the entity-id selection back to its simple-geometry sub-calls
   * (line / circle / arc, i.e. `curveOfSubCall` non-null) and translates
   * each via [`translateCurve`].
   *
   *  - `copy === false` (Move): each selected geometry sub-call is replaced
   *    IN PLACE by its translated version. A translation never changes a
   *    sub-call's entity COUNT (a line stays a line, a circle a circle), so
   *    the replacement allocates exactly as many ids as it freed — all
   *    downstream entity ids stay put and no constraint refs shift. The
   *    target indices are SNAPSHOTTED first and the replacements applied
   *    highest-index-first so each index stays valid across the loop. The
   *    selection is preserved (the same entity ids still own the moved
   *    geometry).
   *  - `copy === true` (Copy): the originals stay; one translated copy of
   *    each selected geometry sub-call is appended as an ordinary
   *    `sketch_line` / `sketch_circle` / `sketch_arc` carrying its source's
   *    construction flag. The selection is preserved (still points at the
   *    originals).
   *
   * Re-solves afterwards. No-op when the session is inactive, the
   * selection is empty, or nothing simple maps.
   */
  moveSketchSelection(dx: number, dy: number, copy: boolean): void {
    const session = this.state.sketchSession;
    if (!session.active) return;
    if (session.selection.length === 0) return;
    this.translateSketchSubCalls(
      subCallsForSelection(session.subCalls, session.selection),
      dx,
      dy,
      copy,
      session.selection,
    );
  }

  /**
   * Ticket matf: rigidly translate the geometry of an explicit set of
   * committed ENTITY ids by `(dx, dy)`. Backs the Select-mode click-drag (a
   * single entity body, or a whole multi-selection grabbed and dragged). The
   * dragged ids map back to their owning geometry sub-calls and translate in
   * place; the solver then settles every constraint (a move forbidden by a
   * constraint is pulled back). No-op when nothing translatable maps.
   */
  translateSketchEntities(
    entityIds: ReadonlyArray<number>,
    dx: number,
    dy: number,
  ): void {
    const session = this.state.sketchSession;
    if (!session.active) return;
    if (entityIds.length === 0) return;
    this.translateSketchSubCalls(
      subCallsForSelection(session.subCalls, entityIds),
      dx,
      dy,
      false,
      entityIds,
    );
  }

  /**
   * Shared core for Move / Copy / Select-drag translation. Translates every
   * TRANSLATABLE geometry sub-call in `sourceIndices` by `(dx, dy)` — lines,
   * circles, arcs, rectangles, polygons, ellipses, splines, paths and free
   * points all move as one rigid body (ticket matf widened this past the
   * line/circle/arc-only `curveOfSubCall` path). `copy` appends translated
   * copies instead of moving in place. `restoreSelection` is re-applied after
   * an in-place move (an equal-count replace keeps entity ids stable).
   */
  private translateSketchSubCalls(
    sourceIndices: ReadonlyArray<number>,
    dx: number,
    dy: number,
    copy: boolean,
    restoreSelection: ReadonlyArray<number>,
  ): void {
    const session = this.state.sketchSession;
    if (!session.active) return;
    const sources = sourceIndices.filter((i) =>
      isTranslatableSubCall(session.subCalls[i]),
    );
    if (sources.length === 0) return;

    if (copy) {
      const copies: Array<{ type: string; params: Record<string, unknown> }> =
        [];
      for (const i of sources) {
        copies.push(translateSubCall(session.subCalls[i], dx, dy));
      }
      if (copies.length === 0) return;
      this.pushSketchSubCalls(copies);
      void solveAndReplace(this);
      return;
    }

    // In-place: replace each selected geometry sub-call with its translated
    // self. A translation preserves each call's entity count, so ids stay
    // stable. Highest index first keeps the lower indices valid across each
    // rewrite (replaceSketchSubCallAt clears the selection; restored below).
    let next = session;
    for (const i of [...sources].sort((a, b) => b - a)) {
      next = reduceReplaceAt(next, i, [
        translateSubCall(session.subCalls[i], dx, dy),
      ]);
    }
    // Equal-count replace keeps entity ids, so the original selection still
    // points at the moved geometry — restore it.
    this.updateSketch(reduceSetSelection(next, [...restoreSelection]));
    void solveAndReplace(this);
  }

  /**
   * F17: rotate the current selection about the sketch origin `[0, 0]` by
   * `angleDeg` degrees (CCW). Same selection → simple-geometry mapping and
   * id-stability reasoning as [`moveSketchSelection`]: a rotation preserves
   * each sub-call's entity count, so an in-place replace leaves all
   * downstream ids and constraint refs intact.
   *
   *  - `copy === false`: replace each selected geometry sub-call in place
   *    by its rotated version (selection preserved).
   *  - `copy === true`: append one rotated copy of each (originals kept).
   *
   * Re-solves afterwards. No-op when the session is inactive, the
   * selection is empty, or nothing simple maps.
   */
  rotateSketchSelection(angleDeg: number, copy: boolean): void {
    const session = this.state.sketchSession;
    if (!session.active) return;
    if (session.selection.length === 0) return;

    const angleRad = (angleDeg * Math.PI) / 180;
    const sources = subCallsForSelection(
      session.subCalls,
      session.selection,
    ).filter((i) => curveOfSubCall(session.subCalls[i]) !== null);
    if (sources.length === 0) return;

    const carry = (params: Record<string, unknown>): Record<string, unknown> =>
      params.construction !== undefined
        ? { construction: params.construction }
        : {};

    if (copy) {
      const copies: Array<{ type: string; params: Record<string, unknown> }> =
        [];
      for (const i of sources) {
        const call = session.subCalls[i];
        const cv = curveOfSubCall(call);
        if (!cv) continue;
        copies.push(
          subCallOfCurve(rotateCurve(cv, [0, 0], angleRad), carry(call.params)),
        );
      }
      if (copies.length === 0) return;
      this.pushSketchSubCalls(copies);
      void solveAndReplace(this);
      return;
    }

    let next = session;
    for (const i of [...sources].sort((a, b) => b - a)) {
      const call = session.subCalls[i];
      const cv = curveOfSubCall(call);
      if (!cv) continue;
      next = reduceReplaceAt(next, i, [
        subCallOfCurve(rotateCurve(cv, [0, 0], angleRad), carry(call.params)),
      ]);
    }
    this.updateSketch(reduceSetSelection(next, session.selection));
    void solveAndReplace(this);
  }

  /** Select mode: toggle a committed-entity id in the sketch
   *  selection set. Ids are resolved against
   *  `buildDraftSketchState(subCalls)` by the canvas. */
  toggleSketchSelection(entityId: number): void {
    this.updateSketch(
      reduceToggleSelection(this.state.sketchSession, entityId),
    );
  }

  /** Replace the sketch selection wholesale. */
  setSketchSelection(ids: ReadonlyArray<number>): void {
    this.updateSketch(reduceSetSelection(this.state.sketchSession, ids));
  }

  /** Clear the sketch selection. */
  clearSketchSelection(): void {
    this.updateSketch(reduceClearSelection(this.state.sketchSession));
  }

  /**
   * P0.1: apply an explicit geometric constraint to the current
   * sketch selection. Maps the selected entity ids onto the
   * constraint's kernel param names, appends the `sketch_constraint_*`
   * sub-call, clears the selection and re-solves so the geometry
   * snaps to satisfy the new relation. No-op when the selection
   * doesn't match the constraint (the palette only offers matching
   * ones, but the guard keeps the call safe).
   */
  applySketchConstraint(type: string): void {
    const session = this.state.sketchSession;
    if (!session.active) return;
    const state = buildDraftSketchState(session.subCalls);
    const call = buildConstraintCall(type, session.selection, state);
    if (!call) return;
    logUserAction("sketch", "constraint-apply", {
      type,
      entities: session.selection.length,
    });
    this.pushSketchSubCalls([call]);
    this.clearSketchSelection();
    void solveAndReplace(this);
  }

  /**
   * zz1r: enter a constraint TOOL (the tool-first constrain workflow).
   * The constraint buttons are sketch tools now: pick the tool, then
   * click the entities one after another and the constraint applies the
   * moment the running selection satisfies it. Clicking the active
   * constraint tool again toggles back to plain select mode. The current
   * selection is kept on entry so a selection-first pick (select the
   * lines, then click the constraint) still applies at once.
   */
  selectSketchConstraintTool(type: string): void {
    const session = this.state.sketchSession;
    if (!session.active) return;
    logUserAction("sketch", "constraint-tool", { type });
    if (session.selectedTool === type) {
      this.selectSketchTool(null);
      return;
    }
    // Keep the current selection (unlike a drawing tool) so an existing
    // pick can apply immediately and partial picks accumulate.
    this.updateSketch({ ...session, selectedTool: type, toolDraft: {} });
    this.tryApplyConstraintTool();
  }

  /**
   * zz1r: route a committed-entity pick while a constraint tool is active
   * — toggle it into the running selection, then apply the constraint the
   * instant the selection satisfies it (tool stays active for the next
   * application).
   */
  pickSketchConstraintEntity(entityId: number): void {
    const session = this.state.sketchSession;
    if (!session.active || !isConstraintTool(session.selectedTool)) return;
    this.toggleSketchSelection(entityId);
    this.tryApplyConstraintTool();
  }

  /**
   * zz1r: apply the active constraint tool if the running selection now
   * satisfies it. The Dimension tool resolves the most-specific
   * dimensional type that fits; the others apply their own type. No-op
   * until the selection composition matches (so partial picks accumulate).
   */
  private tryApplyConstraintTool(): void {
    const session = this.state.sketchSession;
    const tool = session.selectedTool;
    if (!session.active || !isConstraintTool(tool) || tool === null) return;
    if (session.selection.length === 0) return;
    const state = buildDraftSketchState(session.subCalls);
    const options = applicableConstraints(state, session.selection);
    // uf62: the Coincident tool, like Dimension, is a type-dependent
    // dispatcher — it resolves the fitting coincidence constraint
    // (coincident / point-on-object / collinear / concentric) from the
    // two picked entities rather than emitting a fixed type.
    const resolved =
      tool === CONSTRAINT_DIMENSION_TOOL
        ? (DIMENSION_TYPES_BY_SPECIFICITY.find((t) =>
            options.some((o) => o.type === t),
          ) ?? null)
        : tool === CONSTRAINT_COINCIDENT_TOOL
          ? resolveCoincidentConstraint(state, session.selection)
          : options.some((o) => o.type === tool)
            ? tool
            : null;
    if (!resolved) return;
    // Ticket 32az: for a BINARY constraint applied via the tool, keep the
    // FIRST-picked entity as a persistent anchor. The user can then keep
    // clicking entities and they all chain into the constraint — every line
    // parallel to the reference, every point coincident with it, etc. — instead
    // of having to re-pick in pairs (which left consecutive lines unrelated).
    // Dimensional tools open a value editor on apply, so they don't chain.
    const anchor =
      session.selection.length === 2 && tool !== CONSTRAINT_DIMENSION_TOOL
        ? session.selection[0]
        : null;
    this.applySketchConstraint(resolved);
    if (anchor !== null) this.toggleSketchSelection(anchor);
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
   * P0.2: change the numeric value of a dimensional constraint
   * (distance/angle) sub-call, then re-solve so the geometry snaps
   * to the new measurement. The param key depends on the kind:
   * `value_mm` for distance, `value_deg` for angle. No-op for
   * non-dimensional calls or a non-finite value.
   */
  setSketchConstraintValue(index: number, value: number): void {
    const session = this.state.sketchSession;
    if (!session.active) return;
    const call = session.subCalls[index];
    if (!call || !Number.isFinite(value)) return;
    const key =
      call.type === "sketch_constraint_distance" ||
      call.type === "sketch_constraint_radius" ||
      call.type === "sketch_constraint_diameter"
        ? "value_mm"
        : call.type === "sketch_constraint_angle"
          ? "value_deg"
          : null;
    if (key === null) return;
    if (call.params[key] === value) return;
    const next = [...session.subCalls];
    next[index] = { ...call, params: { ...call.params, [key]: value } };
    this.updateSketch({ ...session, subCalls: next });
    void solveAndReplace(this);
  }

  /**
   * Toggle the construction flag on a drawing sub-call. Construction
   * geometry still solves and can be referenced by constraints but is
   * dropped from the extrudable region (kernel `build_sketch2d`) and
   * drawn dashed. Constraint sub-calls have no construction state, so
   * the toggle is a no-op on them. Re-solves because the region — and
   * thus the DoF colour — changes.
   */
  toggleSketchConstructionAt(index: number): void {
    const session = this.state.sketchSession;
    if (!session.active) return;
    const call = session.subCalls[index];
    if (!call || call.type.startsWith("sketch_constraint")) return;
    const wasConstruction = Boolean(call.params.construction);
    const next = [...session.subCalls];
    next[index] = {
      ...call,
      params: { ...call.params, construction: !wasConstruction },
    };
    this.updateSketch({ ...session, subCalls: next });
    void solveAndReplace(this);
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
    for (const listener of this.listeners) listener();
  }

  toggleDatumVisible(datumId: string): void {
    const next = new Set(this.state.hiddenDatums);
    if (next.has(datumId)) next.delete(datumId);
    else next.add(datumId);
    this.patch({ hiddenDatums: next });
    persistHiddenDatumsToStorage(next);
    for (const listener of this.listeners) listener();
  }

  // ── K7.22 UX-fix (2): feature folders (display-only grouping) ──

  /** Update the `folders` slice of state, persist to localStorage,
   *  notify React subscribers. Kernel is not touched. */
  private setFolders(next: readonly FeatureFolder[]): void {
    this.patch({ folders: next });
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

  private transientErrorTimer: ReturnType<typeof setTimeout> | null = null;

  showTransientError(message: string): void {
    this.setTransientError(message);
  }

  private setTransientError(message: string): void {
    if (this.transientErrorTimer) clearTimeout(this.transientErrorTimer);
    this.patch({ transientError: message });
    for (const listener of this.listeners) listener();
    this.transientErrorTimer = setTimeout(() => {
      this.transientErrorTimer = null;
      this.patch({ transientError: null });
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

  // ── MT-A: project / multi-tab management ──────────────────────

  /** Flush the live kernel document back into the loaded part tab. The
   *  active assembly tab is always current (assembly edits write through
   *  immediately), so there is nothing to flush for it.
   *  Pure state shuffle — does NOT notify. */
  private flushLoadedTab(): void {
    const tabs = this.state.project.tabs.map((t) => {
      if (t.id === this.loadedPartTabId && t.kind === "part") {
        return { ...t, documentJson: stripScripts(this.kernel.toJson()) };
      }
      return t;
    });
    this.state = {
      ...this.state,
      project: { ...this.state.project, tabs },
    };
  }

  /** MT-A: switch the active tab by id. Flushes the currently-loaded
   *  part (and active assembly) first, then loads / activates the
   *  target. Switching to a part tab resets undo (intended). */
  setActiveTab(id: string): void {
    const project = this.state.project;
    if (id === project.activeTabId) return;
    const target = project.tabs.find((t) => t.id === id);
    if (!target) return;

    this.flushLoadedTab();

    if (target.kind === "part") {
      this.kernel.loadJson(
        injectScripts(target.documentJson, projectScripts(this.state.project)),
      );
      this.loadedPartTabId = target.id;
      this.state = {
        ...this.state,
        assembly: INACTIVE_ASSEMBLY,
        project: { ...this.state.project, activeTabId: id },
      };
      // Standard commit path refreshes features / meshes / selection.
      this.commit(new Set(), null);
      return;
    }

    if (target.kind === "script") {
      this.state = {
        ...this.state,
        assembly: INACTIVE_ASSEMBLY,
        project: { ...this.state.project, activeTabId: id },
      };
      this.notify();
      this.autosave.schedule(this.serializeProject());
      return;
    }

    // assembly
    this.state = {
      ...this.state,
      assembly: {
        active: true,
        document: target.document,
        selectedPartId: null,
      },
      project: { ...this.state.project, activeTabId: id },
    };
    this.notify();
    this.autosave.schedule(this.serializeProject());
  }

  /** MT-A: create a new tab of the given kind, activate it, return its
   *  id. Flushes the currently-loaded part first. */
  createTab(kind: TabKind): string {
    this.flushLoadedTab();
    const project = this.state.project;
    let newTab: ProjectTab;

    if (kind === "part") {
      const maxN = project.tabs.reduce((max, t) => {
        const m = /^Part (\d+)$/.exec(t.name);
        return m ? Math.max(max, Number(m[1])) : max;
      }, 0);
      newTab = createPartTab(`Part ${maxN + 1}`);
    } else if (kind === "assembly") {
      const maxN = project.tabs.reduce((max, t) => {
        const m = /^Assembly (\d+)$/.exec(t.name);
        return m ? Math.max(max, Number(m[1])) : max;
      }, 0);
      newTab = createAssemblyTab(`Assembly ${maxN + 1}`);
    } else {
      const id = this.deriveScriptId(undefined, projectScripts(project));
      const record = createRecord({ id, source: DEFAULT_SCRIPT_SOURCE });
      newTab = {
        id: allocateTabId(),
        kind: "script",
        name: record.name,
        record,
      };
      this.kernel.upsertScript(record);
    }

    this.state = {
      ...this.state,
      project: { ...project, tabs: [...project.tabs, newTab] },
      // A script tab mirrors a new record into the kernel; refresh the
      // toolbar meta now since the script-tab activation path below does
      // not run commit().
      scriptTypes: this.snapshotScriptTypes(),
    };
    this.setActiveTab(newTab.id);
    return newTab.id;
  }

  /** The current project's display name (shown next to the logo). */
  projectName(): string {
    return this.state.project.name;
  }

  /** Rename the whole project (Ticket C: editable title next to the logo).
   *  Trims; ignores an empty or unchanged name. Always updates local store
   *  state (and schedules the autosave). When the project is server-bound
   *  (`projectId` set) it ALSO persists via `PATCH /api/projects/{id}` with
   *  `{name}`; an unbound guest project renames locally only (the next sync
   *  carries the new name). A failed PATCH is swallowed — the local rename
   *  stands and the pending change syncs later. */
  renameProject(name: string): void {
    const trimmed = name.trim();
    if (trimmed.length === 0 || trimmed === this.state.project.name) return;
    this.state = {
      ...this.state,
      project: { ...this.state.project, name: trimmed },
      // A bound project now has an unsynced local change.
      syncState: this.state.projectId ? "dirty" : this.state.syncState,
    };
    this.notify();
    this.autosave.schedule(this.serializeProject());

    const id = this.state.projectId;
    if (id) {
      void fetch(`/api/projects/${id}`, {
        method: "PATCH",
        credentials: "include",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ name: trimmed }),
      }).catch((err) => {
        // eslint-disable-next-line no-console
        console.error("fielddraft: project rename sync failed", err);
      });
    }
  }

  /** MT-A: rename a tab (trim; ignore empty). For a script tab the
   *  record id is unchanged — only the tab label moves. */
  renameTab(id: string, name: string): void {
    const trimmed = name.trim();
    if (trimmed.length === 0) return;
    const tabs = this.state.project.tabs.map((t) =>
      t.id === id ? ({ ...t, name: trimmed } as ProjectTab) : t,
    );
    this.state = {
      ...this.state,
      project: { ...this.state.project, tabs },
    };
    this.notify();
    this.autosave.schedule(this.serializeProject());
  }

  /** MT-A: delete a tab. Mirrors the kernel for script tabs, picks a
   *  fresh part to load when the loaded part is removed, and never
   *  leaves the tab list empty. */
  deleteTab(id: string): void {
    const project = this.state.project;
    const target = project.tabs.find((t) => t.id === id);
    if (!target) return;

    if (target.kind === "script") {
      this.kernel.removeScript(target.record.id);
    }

    let tabs = project.tabs.filter((t) => t.id !== id);
    const deletedIndex = project.tabs.findIndex((t) => t.id === id);

    // Ensure a part tab still exists to load against.
    const removedLoadedPart =
      target.kind === "part" && target.id === this.loadedPartTabId;
    if (removedLoadedPart) {
      let fallback = tabs.find(
        (t): t is import("./project").PartTab => t.kind === "part",
      );
      if (!fallback) {
        fallback = createPartTab("Part 1");
        tabs = [...tabs, fallback];
      }
      this.kernel.loadJson(
        injectScripts(
          fallback.documentJson,
          projectScripts({ ...project, tabs }),
        ),
      );
      this.loadedPartTabId = fallback.id;
    }

    // Pick the next active tab if we deleted the active one.
    let activeTabId = project.activeTabId;
    if (id === project.activeTabId) {
      const neighbor = tabs[deletedIndex - 1] ?? tabs[deletedIndex] ?? tabs[0];
      activeTabId = neighbor.id;
    }

    this.state = {
      ...this.state,
      project: { ...this.state.project, tabs, activeTabId },
    };

    // If the active tab changed, route through setActiveTab semantics by
    // re-loading. Simplest: reset activeTabId then reuse setActiveTab when
    // the active tab actually moved. Here we instead resolve inline.
    const newActive = tabs.find((t) => t.id === activeTabId);
    if (newActive?.kind === "assembly") {
      this.state = {
        ...this.state,
        assembly: {
          active: true,
          document: newActive.document,
          selectedPartId: null,
        },
      };
    } else if (newActive?.kind === "part") {
      // Load the active part if it isn't already loaded.
      if (this.loadedPartTabId !== newActive.id) {
        this.kernel.loadJson(
          injectScripts(
            newActive.documentJson,
            projectScripts(this.state.project),
          ),
        );
        this.loadedPartTabId = newActive.id;
      }
      this.state = { ...this.state, assembly: INACTIVE_ASSEMBLY };
    } else {
      this.state = { ...this.state, assembly: INACTIVE_ASSEMBLY };
    }

    // A loaded-part regenerate happened; surface it through commit so
    // features/meshes/scriptTypes refresh and autosave persists.
    this.commit();
  }

  /** S4: the document's FieldSkript records (ground truth in the
   *  kernel's `Document.scripts`). */
  listScripts(): FieldSkriptRecord[] {
    return this.kernel.listScripts();
  }

  /**
   * MT-A: save the source of a script tab. Revises the tab's record
   * (keeps id, bumps version), updates tab name + label, mirrors into
   * the kernel, and commits so the live part regenerates + autosave
   * persists. Returns the new record or an error on malformed source.
   */
  saveScriptTab(
    tabId: string,
    source: string,
  ): { ok: true; record: FieldSkriptRecord } | { ok: false; error: string } {
    const tab = this.state.project.tabs.find(
      (t): t is ScriptTab => t.id === tabId && t.kind === "script",
    );
    if (!tab) return { ok: false, error: `Unknown script tab "${tabId}".` };

    let record: FieldSkriptRecord;
    try {
      record = reviseRecord(tab.record, { source });
    } catch (e) {
      return { ok: false, error: String(e instanceof Error ? e.message : e) };
    }

    const tabs = this.state.project.tabs.map((t) =>
      t.id === tabId && t.kind === "script"
        ? ({ ...t, record, name: record.name } as ProjectTab)
        : t,
    );
    this.state = {
      ...this.state,
      project: { ...this.state.project, tabs },
    };
    this.kernel.upsertScript(record);
    this.commit();
    return { ok: true, record };
  }

  /**
   * SI-W: instantiate a saved FieldSkript as a feature in the document.
   * Builds the script's default parameter definition from its schema and
   * adds a feature call keyed by the script id (its FeatureCall `type`).
   * On success commits + selects the new feature, mirroring `commitSketch`.
   * Returns the new feature id, or `null` on a missing record / kernel error.
   */
  insertScriptInstance(scriptId: string): number | null {
    const record = this.kernel.listScripts().find((r) => r.id === scriptId);
    if (!record) {
      this.setTransientError(`Unknown script "${scriptId}".`);
      return null;
    }
    const paramsJson = JSON.stringify(
      buildDefaultDefinition(parseFeatureSchema(record.schema)),
    );
    let id: number;
    try {
      id = this.kernel.addFeatureCall(scriptId, paramsJson);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: addFeatureCall failed", err);
      this.setTransientError(formatKernelError(err, "Script insert failed"));
      return null;
    }
    this.commit(new Set([id]), id);
    return id;
  }

  /** Derive a stable, deterministic script id from a name (or a
   *  positional fallback), slugged and de-duped against the existing
   *  records. No Date.now/Math.random — repeated saves are stable. */
  private deriveScriptId(
    name: string | undefined,
    existing: readonly FieldSkriptRecord[],
  ): string {
    const taken = new Set(existing.map((r) => r.id));
    const slug = (name ?? "")
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "_")
      .replace(/^_+|_+$/g, "");
    const base = slug.length > 0 ? slug : `script_${existing.length + 1}`;
    if (!taken.has(base)) return base;
    let n = 2;
    while (taken.has(`${base}_${n}`)) n += 1;
    return `${base}_${n}`;
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
    logUserAction("sketch", "commit", {
      subCalls: this.state.sketchSession.subCalls.length,
    });
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
      this.setTransientError(
        "Sketch discarded — add geometry before finishing.",
      );
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
