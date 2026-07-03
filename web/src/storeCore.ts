/**
 * StoreCore — the minimal shared foundation every store domain
 * controller builds on.
 *
 * The kernel is the ground truth. The core's job is to:
 *  - own the singleton kernel handle and the {@link StoreState} snapshot,
 *  - provide the pub-sub surface (`subscribe` / `getState` / `emit` /
 *    `notify`) that drives React's `useSyncExternalStore`,
 *  - run the shared `commit()` tail after every kernel mutation
 *    (derived fields, selection pruning, autosave, mesh/weave refresh),
 *  - host the small cross-domain UI helpers (`setLoading`,
 *    `setTransientError`).
 *
 * Domain-specific behaviour that `commit()` needs (pending-feature
 * confirmation, project serialisation, worker scheduling, …) is injected
 * via {@link StoreCoreHooks} — the KernelStore facade wires those up so
 * the core stays free of domain imports.
 */

import type { FeatureSummary, Kernel, BoolOpTagValue } from "./kernel";
import type { VariableEntry } from "./kernel";
import type { AutoSaveHandle } from "./persistence";
import type { PickedElement } from "./elementQuery";
import type { MesherBodyPayload } from "./mesher.worker";
import type { SketchSessionState } from "./sketchSession";
import type { SketchToolParams } from "./sketchStore";
import type { ThreeMfStash } from "./threemf";
import type { Project } from "./project";
import type { AssemblyState } from "./assembly";
import { publishSceneBounds } from "./sceneBoundsBridge";

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
  /** Parameters of the interactive sketch tools (fillet radius, pattern
   *  counts, text value/font, …) consolidated into one object — every
   *  setter in the sketch domain is a one-line patch of this slice.
   *  See {@link SketchToolParams}. */
  sketchToolParams: SketchToolParams;
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
  assembly: AssemblyState;
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

export function loadFoldersFromStorage(): FeatureFolder[] {
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

export function persistFoldersToStorage(
  folders: readonly FeatureFolder[],
): void {
  try {
    localStorage.setItem(FOLDERS_STORAGE_KEY, JSON.stringify(folders));
  } catch {
    // localStorage quota exceeded or disabled — silently skip.
  }
}

const VISIBLE_SKETCHES_STORAGE_KEY = "fielddraft.visibleSketches";

export function loadVisibleSketchesFromStorage(): number[] {
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

export function persistVisibleSketchesToStorage(
  ids: readonly number[],
): void {
  try {
    localStorage.setItem(VISIBLE_SKETCHES_STORAGE_KEY, JSON.stringify(ids));
  } catch {
    // ignore
  }
}

const HIDDEN_DATUMS_STORAGE_KEY = "fielddraft.hiddenDatums";

export function loadHiddenDatumsFromStorage(): Set<string> {
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

export function persistHiddenDatumsToStorage(ids: ReadonlySet<string>): void {
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
export function formatKernelError(err: unknown, fallback: string): string {
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

/**
 * The domain-specific pieces `commit()` needs but the core must not
 * know about. Wired up once by the KernelStore facade after all domain
 * controllers exist.
 */
export interface StoreCoreHooks {
  /** Pending-feature reconciliation: a mutation that moves the selection
   *  away from the pending extrude confirms it first. */
  beforeCommit(nextSelection: ReadonlySet<number> | undefined): void;
  /** Push the selection anchor's body to the kernel as the active body. */
  syncActiveBody(): void;
  /** The single persistence funnel (full multi-tab Project blob). */
  serializeProject(): string;
  /** Pull the latest meshes into `state.meshedBodies` (worker or sync). */
  refreshMeshCache(): void;
  /** Farm stale B-Rep weaves out to the weaver worker. */
  scheduleWeave(): void;
  /** Derive the Part toolbar's script-button meta from the kernel. */
  snapshotScriptTypes(): { id: string; name: string; icon: string }[];
}

export class StoreCore {
  readonly kernel: Kernel;
  readonly autosave: AutoSaveHandle;
  readonly listeners = new Set<() => void>();
  /** The current immutable state snapshot. Assigned by the facade
   *  constructor once the initial document is loaded; every mutation
   *  replaces the whole object so `useSyncExternalStore` re-renders. */
  state!: StoreState;
  private hooks!: StoreCoreHooks;

  constructor(kernel: Kernel, autosave: AutoSaveHandle) {
    this.kernel = kernel;
    this.autosave = autosave;
  }

  /** Late-bind the domain hooks (the facade wires them after the domain
   *  controllers exist). Must be called before the first `commit()`. */
  setHooks(hooks: StoreCoreHooks): void {
    this.hooks = hooks;
  }

  // ── pub-sub ───────────────────────────────────────────────────────

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  getState = (): StoreState => this.state;

  /** Shallow-merge a partial into the state snapshot (no notify). */
  patch(partial: Partial<StoreState>): void {
    this.state = { ...this.state, ...partial };
  }

  /** Bare listener sweep — callers that already refreshed whatever
   *  derived data they own use this directly. */
  emit(): void {
    for (const listener of this.listeners) listener();
  }

  /** Standard notify: refresh the published scene bounds, then sweep. */
  notify(): void {
    this.updateSceneBounds();
    this.emit();
  }

  /** Publish the scene's world AABB centre + bounding radius so the viewport
   *  can adapt the camera near/far planes to it (ticket lbzq — bodies vanished
   *  past a fixed far plane on zoom-out). Cheap (9 floats from the kernel) and
   *  only runs on state changes, never per frame. */
  updateSceneBounds(): void {
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

  /** The shared post-mutation tail: refresh every kernel-derived state
   *  field, prune dead selection entries, schedule autosave + the worker
   *  rebuild/weave, then notify subscribers. */
  commit(
    nextSelection?: ReadonlySet<number>,
    nextAnchor?: number | null,
  ): void {
    // A mutation that moves the selection away from the pending extrude
    // confirms it first (e.g. creating the next feature while its dialog is
    // still open) — see the pending-feature section in the facade.
    this.hooks.beforeCommit(nextSelection);
    const features = this.kernel.listFeatures();
    const rbRaw = this.kernel.getRollbackIndex();
    // Spread the previous snapshot and overwrite ONLY the fields commit()
    // actually derives/resets. Everything else carries over automatically —
    // a newly added StoreState field can no longer be silently reset to
    // `undefined` by a forgotten line here.
    this.state = {
      ...this.state,
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
      // An empty document must show NO meshes immediately (ticket vy3t): the
      // mesher worker skips posting for a body-less doc, so the stale meshes
      // would otherwise linger (e.g. after Menu → New). Clear synchronously
      // when the kernel has no bodies; otherwise the worker rebuild updates.
      meshedBodies:
        this.kernel.bodyCount() === 0 ? [] : this.state.meshedBodies,
      syncState: this.state.projectId ? "dirty" : "offline",
      variables: this.kernel.listVariables(),
      sceneSize: this.kernel.sceneAabb().size as [number, number, number],
      transientError: null, // successful commit clears any transient error
      mateMode: false,
      ikMode: false,
      rollbackIndex: rbRaw < 0 ? features.length : rbRaw,
      scriptTypes: this.hooks.snapshotScriptTypes(),
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
    this.hooks.syncActiveBody();
    this.autosave.schedule(this.hooks.serializeProject());
    this.hooks.refreshMeshCache();
    this.hooks.scheduleWeave();
    this.emit();
  }

  // ── small cross-domain UI helpers ─────────────────────────────────

  setLoading(phase: string | null): void {
    this.patch({ loading: phase ? { phase } : null });
    this.notify();
  }

  private transientErrorTimer: ReturnType<typeof setTimeout> | null = null;

  /** Show a user-visible transient error toast for 5 s. */
  setTransientError(message: string): void {
    if (this.transientErrorTimer) clearTimeout(this.transientErrorTimer);
    this.patch({ transientError: message });
    this.emit();
    this.transientErrorTimer = setTimeout(() => {
      this.transientErrorTimer = null;
      this.patch({ transientError: null });
      this.emit();
    }, 5000);
  }
}
