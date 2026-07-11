/**
 * Settings domain module — the persistence layer behind the Settings dialog.
 *
 * Owns the localStorage keys, the `SettingsState` shape (with validation on
 * load), the workspace-unit types/options and the ClaudeDraft deep-link boot
 * override. `SettingsMenu.tsx` is a pure view on top of this module; nothing
 * here imports React components, so consumers (App, ReportDialog, workers,
 * boot code) can depend on it without pulling UI in.
 */

import { createContext, useContext } from "react";
import {
  DEFAULT_CONTROLS_PRESET,
  isControlsPresetId,
  type ControlsPresetId,
} from "./controlsPreset";

/* ── localStorage keys ──────────────────────────────────────── */
const LS_COMPACT = "fielddraft.compact";
const LS_UNITS = "fielddraft.units";
const LS_THEME = "fielddraft.theme";
const LS_RENDER_MODE = "fielddraft.renderMode";
const LS_BREP_EDGE_MODE = "fielddraft.brepEdgeMode";
const LS_OVERLAY_DEPTH_BIAS = "fielddraft.overlayDepthBias";
const LS_MESH_ALGORITHM = "fielddraft.meshAlgorithm";
const LS_FRAME_LOOP = "fielddraft.frameLoop";
const LS_PARAMETRIC_WEAVE = "fielddraft.parametricWeave";
const LS_DRAG_BREP_RECOMPUTE = "fielddraft.dragBrepRecompute";
const LS_LIVE_EXTRUDE_DRAG = "fielddraft.liveExtrudeDrag";
const LS_SHOW_EDGE_POINTS = "fielddraft.showEdgePoints";
const LS_SHOW_OCTREE_CELLS = "fielddraft.showOctreeCells";
const LS_SCREEN_SPACE_CSG = "fielddraft.screenSpaceCsg";
const LS_PERSPECTIVE = "fielddraft.perspective";
const LS_CONTROLS_PRESET = "fielddraft.controlsPreset";
const LS_AUTOSAVE_DEBOUNCE = "fielddraft.autosaveDebounceSec";
const LS_CLOUD_SYNC_INTERVAL = "fielddraft.cloudSyncIntervalMin";
/** Shared with main.tsx's HMR guard — kept here so all localStorage keys live
 *  in one place. */
export const LS_AUTO_RELOAD = "fielddraft.autoReload";

/* ── Workspace units ────────────────────────────────────────── */

export type LengthUnitKey = "mm" | "cm" | "m" | "in" | "ft";
export type LinearAccelerationUnitKey = "m/s2" | "mm/s2" | "ft/s2" | "in/s2";
export type AngleUnitKey = "deg" | "rad";
export type AngularVelocityUnitKey = "deg/s" | "rad/s" | "rpm";
export type MassUnitKey = "g" | "kg" | "lb" | "oz";
export type DensityUnitKey = "g/cm3" | "kg/m3" | "lb/ft3" | "lb/in3";
export type ForceUnitKey = "N" | "kN" | "MN" | "lbf" | "kgf";
export type MomentUnitKey = "N*m" | "N*mm" | "kN*m" | "lbf*ft" | "lbf*in";
export type PressureUnitKey = "Pa" | "kPa" | "MPa" | "GPa" | "psi" | "bar" | "atm";
export type EnergyUnitKey = "J" | "kJ" | "cal" | "kcal" | "BTU" | "ft*lbf";
export type DecimalFormat = "1" | "0.1" | "0.12" | "0.123" | "0.1234" | "0.12345";

export interface WorkspaceUnits {
  lengthUnit: LengthUnitKey;
  lengthDecimals: DecimalFormat;
  linearAccelerationUnit: LinearAccelerationUnitKey;
  linearAccelerationDecimals: DecimalFormat;
  angleUnit: AngleUnitKey;
  angleDecimals: DecimalFormat;
  angularVelocityUnit: AngularVelocityUnitKey;
  angularVelocityDecimals: DecimalFormat;
  massUnit: MassUnitKey;
  massDecimals: DecimalFormat;
  densityUnit: DensityUnitKey;
  densityDecimals: DecimalFormat;
  forceUnit: ForceUnitKey;
  forceDecimals: DecimalFormat;
  momentUnit: MomentUnitKey;
  momentDecimals: DecimalFormat;
  pressureUnit: PressureUnitKey;
  pressureDecimals: DecimalFormat;
  energyUnit: EnergyUnitKey;
  energyDecimals: DecimalFormat;
}

const DEFAULT_UNITS: WorkspaceUnits = {
  lengthUnit: "mm",
  lengthDecimals: "0.123",
  linearAccelerationUnit: "m/s2",
  linearAccelerationDecimals: "0.123",
  angleUnit: "deg",
  angleDecimals: "0.123",
  angularVelocityUnit: "deg/s",
  angularVelocityDecimals: "0.123",
  massUnit: "g",
  massDecimals: "0.123",
  densityUnit: "g/cm3",
  densityDecimals: "0.123",
  forceUnit: "N",
  forceDecimals: "0.123",
  momentUnit: "N*m",
  momentDecimals: "0.123",
  pressureUnit: "Pa",
  pressureDecimals: "0.123",
  energyUnit: "J",
  energyDecimals: "0.123",
};

export const LENGTH_UNIT_OPTIONS: { key: LengthUnitKey; label: string }[] = [
  { key: "mm", label: "Millimeter" },
  { key: "cm", label: "Centimeter" },
  { key: "m", label: "Meter" },
  { key: "in", label: "Inch" },
  { key: "ft", label: "Foot" },
];

export const LINEAR_ACCELERATION_UNIT_OPTIONS: { key: LinearAccelerationUnitKey; label: string }[] = [
  { key: "m/s2", label: "Meter per second squared" },
  { key: "mm/s2", label: "Millimeter per second squared" },
  { key: "ft/s2", label: "Foot per second squared" },
  { key: "in/s2", label: "Inch per second squared" },
];

export const ANGLE_UNIT_OPTIONS: { key: AngleUnitKey; label: string }[] = [
  { key: "deg", label: "Degree" },
  { key: "rad", label: "Radian" },
];

export const ANGULAR_VELOCITY_UNIT_OPTIONS: { key: AngularVelocityUnitKey; label: string }[] = [
  { key: "deg/s", label: "Degree per second" },
  { key: "rad/s", label: "Radian per second" },
  { key: "rpm", label: "Revolution per minute" },
];

export const MASS_UNIT_OPTIONS: { key: MassUnitKey; label: string }[] = [
  { key: "g", label: "Gram" },
  { key: "kg", label: "Kilogram" },
  { key: "lb", label: "Pound" },
  { key: "oz", label: "Ounce" },
];

export const DENSITY_UNIT_OPTIONS: { key: DensityUnitKey; label: string }[] = [
  { key: "g/cm3", label: "Gram per cubic centimeter" },
  { key: "kg/m3", label: "Kilogram per cubic meter" },
  { key: "lb/ft3", label: "Pound per cubic foot" },
  { key: "lb/in3", label: "Pound per cubic inch" },
];

export const FORCE_UNIT_OPTIONS: { key: ForceUnitKey; label: string }[] = [
  { key: "N", label: "Newton" },
  { key: "kN", label: "Kilonewton" },
  { key: "MN", label: "Meganewton" },
  { key: "lbf", label: "Pound-force" },
  { key: "kgf", label: "Kilogram-force" },
];

export const MOMENT_UNIT_OPTIONS: { key: MomentUnitKey; label: string }[] = [
  { key: "N*m", label: "Newton-meter" },
  { key: "N*mm", label: "Newton-millimeter" },
  { key: "kN*m", label: "Kilonewton-meter" },
  { key: "lbf*ft", label: "Pound-force foot" },
  { key: "lbf*in", label: "Pound-force inch" },
];

export const PRESSURE_UNIT_OPTIONS: { key: PressureUnitKey; label: string }[] = [
  { key: "Pa", label: "Pascal" },
  { key: "kPa", label: "Kilopascal" },
  { key: "MPa", label: "Megapascal" },
  { key: "GPa", label: "Gigapascal" },
  { key: "psi", label: "Pound per square inch" },
  { key: "bar", label: "Bar" },
  { key: "atm", label: "Atmosphere" },
];

export const ENERGY_UNIT_OPTIONS: { key: EnergyUnitKey; label: string }[] = [
  { key: "J", label: "Joule" },
  { key: "kJ", label: "Kilojoule" },
  { key: "cal", label: "Calorie" },
  { key: "kcal", label: "Kilocalorie" },
  { key: "BTU", label: "British thermal unit" },
  { key: "ft*lbf", label: "Foot-pound" },
];

export const DECIMAL_OPTIONS: { key: DecimalFormat; label: string }[] = [
  { key: "1", label: "1" },
  { key: "0.1", label: "0.1" },
  { key: "0.12", label: "0.12" },
  { key: "0.123", label: "0.123" },
  { key: "0.1234", label: "0.1234" },
  { key: "0.12345", label: "0.12345" },
];

export function decimalPlaces(fmt: DecimalFormat): number {
  if (!fmt || fmt === "1") return 0;
  return fmt.length - 2;
}

export const WorkspaceUnitsContext = createContext<WorkspaceUnits>(DEFAULT_UNITS);
export function useWorkspaceUnits(): WorkspaceUnits {
  return useContext(WorkspaceUnitsContext);
}

/* ── Setting types ──────────────────────────────────────────── */

export type ThemeChoice = "light" | "dark";
export type RenderMode = "sdf-vm" | "sdf-deferred" | "mesh" | "brep" | "webgl";
export type MeshAlgorithm = "mdc" | "mt";
/** R3F render scheduling:
 *   - "demand": render only when something invalidates the canvas (default,
 *               idle GPU when the scene is static)
 *   - "always": render every animation frame regardless of changes — a
 *               diagnostic crutch: if a view only updates correctly in
 *               "always", its "demand" path is missing an invalidate trigger. */
export type FrameLoopMode = "demand" | "always";
/** How the analytic B-Rep edge wireframe is drawn:
 *   - "xray":    all edges, never occluded by faces (current default)
 *   - "visible": only edges that are not hidden behind a face
 *   - "none":    don't draw the B-Rep wireframe at all */
export type BRepEdgeMode = "xray" | "visible" | "none";

const VALID_RENDER_MODES: RenderMode[] = ["sdf-vm", "sdf-deferred", "mesh", "brep", "webgl"];
const VALID_MESH_ALGORITHMS: MeshAlgorithm[] = ["mdc", "mt"];
const VALID_BREP_EDGE_MODES: BRepEdgeMode[] = ["xray", "visible", "none"];

export interface SettingsState {
  compact: boolean;
  units: WorkspaceUnits;
  theme: ThemeChoice;
  renderMode: RenderMode;
  meshAlgorithm: MeshAlgorithm;
  brepEdgeMode: BRepEdgeMode;
  /** Depth-Bias-Stärke im "visible"-Modus: zieht Kanten/Overlay-Fragmente im
   * Z-Buffer zur Kamera, damit Geometrie exakt AUF einer Fläche nicht
   * z-fightet (polygonOffsetFactor = −bias, Units = −2·bias; WebGPU:
   * depthBias). 0 = aus; wirkungslos außerhalb von "visible". */
  overlayDepthBias: number;
  frameLoop: FrameLoopMode;
  /** Milestone-1 parametric weaving: exact analytic B-Rep for pure grundforms
   * via CSG branch-pair seeds, skipping the octree. On by default — resolves
   * sub-cell features the octree misses (grazing near-tangent intersections)
   * and falls back to the octree weave per-body for anything not yet handled,
   * so the result is never worse. */
  parametricWeave: boolean;
  /** Re-weave and display the exact B-Rep live during gizmo drags. Off by
   * default: the throttled drag commits then only update the render mesh,
   * B-Rep edges of geometry-changed bodies hide for the drag's duration,
   * and the weave runs exactly once on release — keeps weave-heavy drags
   * (booleans against spline-revolves) fluid. */
  dragBrepRecompute: boolean;
  /** Commit the extrude depth live WHILE the gizmo drag runs (throttled),
   * so the renderer shows the real body instead of an approximate preview
   * prism. On by default — the heavy B-Rep weave stays deferred to the
   * drag's end via the same suppression the transform gizmo uses. */
  liveExtrudeDrag: boolean;
  /** Debug overlay: render each B-Rep edge's individual sample points as
   * markers, so a NURBS edge's tessellation density (many points vs. few) is
   * visible at a glance. */
  showEdgePoints: boolean;
  /** Debug overlay: render the topology octree's leaf cells as a wireframe
   * grid, colour-coded by class (face / edge / vertex), to inspect the
   * adaptive resolution the B-Rep weave actually used. Session-only, never
   * persisted: the dump rebuilds the octree synchronously on the main thread
   * and can take minutes on complex bodies — persisting it would re-freeze the
   * tab on every reload with no way to recover. */
  showOctreeCells: boolean;
  /** EXPERIMENTELL & OPT-IN (default AUS): Screen-Space-CSG Stage 1
   * (docs/ADAPTIVE-RENDERING-PLAN.md, Achse 3, Phase 3). Nur im WebGL-Renderpfad
   * wirksam; rendert einen einstufigen A−B-Composite-Pass über die
   * parametrischen Hüllmeshes. Visuell unverifiziertes Experiment — der normale
   * WebGL-/WebGPU-Pfad bleibt bei AUS unverändert. */
  screenSpaceCsg: boolean;
  /** Camera projection. Default OFF → an orthographic (isometric-style)
   *  viewport: parallel projection, no foreshortening — the CAD default, where
   *  equal lengths read as equal at every depth. ON → a perspective camera
   *  (fov 45) for a more photographic look. */
  perspective: boolean;
  /** Mouse/keyboard control scheme (docs/UX-PRESETS.md) — which CAD
   * package's habits the camera, selection and view keys follow. */
  controlsPreset: ControlsPresetId;
  /** Dev-only: auto-reload the page when Vite pushes a code change it can't
   * hot-swap. On by default. Off keeps the running session (and in-progress
   * project) alive across edits — main.tsx's `vite:beforeFullReload` guard
   * reads this. No effect in a production build (no HMR). */
  autoReload: boolean;
  /** Project autosave debounce, in seconds. Every dirty marker (document
   * edit, camera move, debug-dialog change) restarts this timer; the project
   * is written once the state has been quiet for this long (plus a flush on
   * unload). */
  autosaveDebounceSec: number;
  /** Cloud auto-sync interval, in minutes (P3b, docs/PROJECT-ASSET-STORAGE-PLAN
   * §5.7). A background timer pushes a server-bound project to the cloud this
   * often — but only when it is `dirty`, so a quiet project costs nothing.
   * `0` disables the periodic timer entirely (the exit-editor and beforeunload
   * triggers still fire). Because P1/P2 externalize meshes to blobs, the doc
   * PATCH is small, so frequent saves are cheap. */
  cloudSyncIntervalMin: number;
}

export const DEFAULT_OVERLAY_DEPTH_BIAS = 2;

function clampOverlayDepthBias(v: unknown): number {
  const n = Number(v);
  if (!Number.isFinite(n)) return DEFAULT_OVERLAY_DEPTH_BIAS;
  return Math.min(100, Math.max(0, n));
}

export const DEFAULT_AUTOSAVE_DEBOUNCE_SEC = 5;
export const DEFAULT_CLOUD_SYNC_INTERVAL_MIN = 1;

/** Module-level mirror of the debounce so non-React code (the autosave
 *  handle created at store bootstrap) can read the CURRENT value without a
 *  settings re-load. Kept in sync by loadSettings/updateSetting. */
let autosaveDebounceSecCache = DEFAULT_AUTOSAVE_DEBOUNCE_SEC;

export function getAutosaveDebounceMs(): number {
  return autosaveDebounceSecCache * 1000;
}

function clampAutosaveDebounceSec(v: unknown): number {
  return typeof v === "number" && Number.isFinite(v)
    ? Math.min(60, Math.max(0.5, v))
    : DEFAULT_AUTOSAVE_DEBOUNCE_SEC;
}

/** Module-level mirror of the cloud-sync interval so non-React code can read
 *  the CURRENT value; kept in sync by loadSettings/updateSetting. */
let cloudSyncIntervalMinCache = DEFAULT_CLOUD_SYNC_INTERVAL_MIN;

/** Current cloud auto-sync interval in milliseconds (`0` = disabled). */
export function getCloudSyncIntervalMs(): number {
  return cloudSyncIntervalMinCache * 60_000;
}

/** Clamp to [0, 60] minutes; `0` (disabled) is a valid value, unlike the
 *  autosave debounce which has a floor. */
function clampCloudSyncIntervalMin(v: unknown): number {
  return typeof v === "number" && Number.isFinite(v)
    ? Math.min(60, Math.max(0, v))
    : DEFAULT_CLOUD_SYNC_INTERVAL_MIN;
}

function validOr<K extends string>(opts: { key: K }[], val: unknown, fallback: K): K {
  return opts.some((o) => o.key === val) ? (val as K) : fallback;
}
function validDec(val: unknown): DecimalFormat {
  return DECIMAL_OPTIONS.some((o) => o.key === val) ? (val as DecimalFormat) : DEFAULT_UNITS.lengthDecimals;
}

export function parseUnits(raw: string | null): WorkspaceUnits {
  if (!raw) return DEFAULT_UNITS;
  try {
    const o = JSON.parse(raw);
    return {
      lengthUnit: validOr(LENGTH_UNIT_OPTIONS, o.lengthUnit, DEFAULT_UNITS.lengthUnit),
      lengthDecimals: validDec(o.lengthDecimals),
      linearAccelerationUnit: validOr(LINEAR_ACCELERATION_UNIT_OPTIONS, o.linearAccelerationUnit, DEFAULT_UNITS.linearAccelerationUnit),
      linearAccelerationDecimals: validDec(o.linearAccelerationDecimals),
      angleUnit: validOr(ANGLE_UNIT_OPTIONS, o.angleUnit, DEFAULT_UNITS.angleUnit),
      angleDecimals: validDec(o.angleDecimals),
      angularVelocityUnit: validOr(ANGULAR_VELOCITY_UNIT_OPTIONS, o.angularVelocityUnit, DEFAULT_UNITS.angularVelocityUnit),
      angularVelocityDecimals: validDec(o.angularVelocityDecimals),
      massUnit: validOr(MASS_UNIT_OPTIONS, o.massUnit, DEFAULT_UNITS.massUnit),
      massDecimals: validDec(o.massDecimals),
      densityUnit: validOr(DENSITY_UNIT_OPTIONS, o.densityUnit, DEFAULT_UNITS.densityUnit),
      densityDecimals: validDec(o.densityDecimals),
      forceUnit: validOr(FORCE_UNIT_OPTIONS, o.forceUnit, DEFAULT_UNITS.forceUnit),
      forceDecimals: validDec(o.forceDecimals),
      momentUnit: validOr(MOMENT_UNIT_OPTIONS, o.momentUnit, DEFAULT_UNITS.momentUnit),
      momentDecimals: validDec(o.momentDecimals),
      pressureUnit: validOr(PRESSURE_UNIT_OPTIONS, o.pressureUnit, DEFAULT_UNITS.pressureUnit),
      pressureDecimals: validDec(o.pressureDecimals),
      energyUnit: validOr(ENERGY_UNIT_OPTIONS, o.energyUnit, DEFAULT_UNITS.energyUnit),
      energyDecimals: validDec(o.energyDecimals),
    };
  } catch {
    return DEFAULT_UNITS;
  }
}

/* ── Deep-link boot override ────────────────────────────────── */

// ClaudeDraft "open ticket" deep-link (`?project=`): the ticket's settings.json
// is applied as a ONE-SHOT, in-memory override of the first loadSettings() call
// (React mount) — never written to localStorage. localStorage is origin-wide,
// so persisting a ticket's per-device settings would clobber the user's real
// settings in their other tabs; an in-memory override keeps the repro tab
// isolated. bootProject.ts sets this before the app mounts.
let bootOverride: Partial<SettingsState> | null = null;
const BOOT_SETTING_KEYS: (keyof SettingsState)[] = [
  "compact",
  "units",
  "theme",
  "renderMode",
  "meshAlgorithm",
  "brepEdgeMode",
  "overlayDepthBias",
  "frameLoop",
  "parametricWeave",
  "dragBrepRecompute",
  "liveExtrudeDrag",
  "showEdgePoints",
  "screenSpaceCsg",
  "perspective",
  "controlsPreset",
  "autoReload",
  "autosaveDebounceSec",
  "cloudSyncIntervalMin",
];

/** Stage a settings.json snapshot as the next loadSettings() result (one-shot,
 *  in-memory; see {@link bootOverride}). Unknown keys are ignored;
 *  `showOctreeCells` is excluded by design (session-only, never restored). */
export function setBootSettings(raw: Record<string, unknown>): void {
  const picked: Partial<SettingsState> = {};
  for (const k of BOOT_SETTING_KEYS) {
    if (raw[k] !== undefined) {
      (picked as Record<string, unknown>)[k] = raw[k];
    }
  }
  bootOverride = picked;
}

/* ── Load / save ────────────────────────────────────────────── */

/** Load all settings from localStorage with safe fallbacks. */
export function loadSettings(): SettingsState {
  let compact = false;
  let units: WorkspaceUnits = DEFAULT_UNITS;
  let theme: ThemeChoice = "light";
  let renderMode: RenderMode = "sdf-deferred";
  let meshAlgorithm: MeshAlgorithm = "mdc";
  let brepEdgeMode: BRepEdgeMode = "xray";
  let overlayDepthBias = DEFAULT_OVERLAY_DEPTH_BIAS;
  let frameLoop: FrameLoopMode = "demand";
  let parametricWeave = true;
  let dragBrepRecompute = false;
  let liveExtrudeDrag = true;
  let showEdgePoints = false;
  const showOctreeCells = false;
  let screenSpaceCsg = false;
  let perspective = false;
  let controlsPreset: ControlsPresetId = DEFAULT_CONTROLS_PRESET;
  let autoReload = true;
  let autosaveDebounceSec = DEFAULT_AUTOSAVE_DEBOUNCE_SEC;
  let cloudSyncIntervalMin = DEFAULT_CLOUD_SYNC_INTERVAL_MIN;
  try {
    compact = localStorage.getItem(LS_COMPACT) === "true";
    units = parseUnits(localStorage.getItem(LS_UNITS));
    const rawTheme = localStorage.getItem(LS_THEME);
    if (rawTheme === "light" || rawTheme === "dark") theme = rawTheme;
    const rawMode = localStorage.getItem(LS_RENDER_MODE);
    if (rawMode && VALID_RENDER_MODES.includes(rawMode as RenderMode))
      renderMode = rawMode as RenderMode;
    const rawAlgo = localStorage.getItem(LS_MESH_ALGORITHM);
    if (rawAlgo && VALID_MESH_ALGORITHMS.includes(rawAlgo as MeshAlgorithm))
      meshAlgorithm = rawAlgo as MeshAlgorithm;
    const rawBrepEdge = localStorage.getItem(LS_BREP_EDGE_MODE);
    if (rawBrepEdge && VALID_BREP_EDGE_MODES.includes(rawBrepEdge as BRepEdgeMode))
      brepEdgeMode = rawBrepEdge as BRepEdgeMode;
    const rawBias = localStorage.getItem(LS_OVERLAY_DEPTH_BIAS);
    if (rawBias !== null && Number.isFinite(Number(rawBias)))
      overlayDepthBias = clampOverlayDepthBias(Number(rawBias));
    const rawFrameLoop = localStorage.getItem(LS_FRAME_LOOP);
    if (rawFrameLoop === "always" || rawFrameLoop === "demand")
      frameLoop = rawFrameLoop;
    const rawParametric = localStorage.getItem(LS_PARAMETRIC_WEAVE);
    if (rawParametric !== null) parametricWeave = rawParametric === "true";
    dragBrepRecompute = localStorage.getItem(LS_DRAG_BREP_RECOMPUTE) === "true";
    liveExtrudeDrag = localStorage.getItem(LS_LIVE_EXTRUDE_DRAG) !== "false";
    showEdgePoints = localStorage.getItem(LS_SHOW_EDGE_POINTS) === "true";
    // Opt-in experiment, default off — only "true" turns it on.
    screenSpaceCsg = localStorage.getItem(LS_SCREEN_SPACE_CSG) === "true";
    // Default off → orthographic; only an explicit "true" enables perspective.
    perspective = localStorage.getItem(LS_PERSPECTIVE) === "true";
    // showOctreeCells is deliberately NOT read back — drop any stale key so a
    // tab frozen by the overlay recovers on reload.
    localStorage.removeItem(LS_SHOW_OCTREE_CELLS);
    const rawPreset = localStorage.getItem(LS_CONTROLS_PRESET);
    if (isControlsPresetId(rawPreset)) controlsPreset = rawPreset;
    autoReload = localStorage.getItem(LS_AUTO_RELOAD) !== "false";
    const rawDebounce = Number(localStorage.getItem(LS_AUTOSAVE_DEBOUNCE));
    if (Number.isFinite(rawDebounce) && rawDebounce > 0)
      autosaveDebounceSec = clampAutosaveDebounceSec(rawDebounce);
    const rawCloud = localStorage.getItem(LS_CLOUD_SYNC_INTERVAL);
    if (rawCloud !== null && Number.isFinite(Number(rawCloud)))
      cloudSyncIntervalMin = clampCloudSyncIntervalMin(Number(rawCloud));
  } catch {
    // localStorage may be unavailable
  }
  const loaded: SettingsState = { compact, units, theme, renderMode, meshAlgorithm, brepEdgeMode, overlayDepthBias, frameLoop, parametricWeave, dragBrepRecompute, liveExtrudeDrag, showEdgePoints, showOctreeCells, screenSpaceCsg, perspective, controlsPreset, autoReload, autosaveDebounceSec, cloudSyncIntervalMin };
  if (bootOverride) {
    const merged = { ...loaded, ...bootOverride };
    bootOverride = null; // one-shot: only the first read (React mount) honors it
    autosaveDebounceSecCache = clampAutosaveDebounceSec(merged.autosaveDebounceSec);
    cloudSyncIntervalMinCache = clampCloudSyncIntervalMin(merged.cloudSyncIntervalMin);
    return merged;
  }
  autosaveDebounceSecCache = autosaveDebounceSec;
  cloudSyncIntervalMinCache = cloudSyncIntervalMin;
  return loaded;
}

function saveSetting(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    // ignore
  }
}

/** Which localStorage key persists each setting. `showOctreeCells` is absent
 *  by design: session-only, never written (see its doc on SettingsState). */
const LS_KEY_BY_SETTING: Partial<Record<keyof SettingsState, string>> = {
  compact: LS_COMPACT,
  units: LS_UNITS,
  theme: LS_THEME,
  renderMode: LS_RENDER_MODE,
  meshAlgorithm: LS_MESH_ALGORITHM,
  brepEdgeMode: LS_BREP_EDGE_MODE,
  overlayDepthBias: LS_OVERLAY_DEPTH_BIAS,
  frameLoop: LS_FRAME_LOOP,
  parametricWeave: LS_PARAMETRIC_WEAVE,
  dragBrepRecompute: LS_DRAG_BREP_RECOMPUTE,
  liveExtrudeDrag: LS_LIVE_EXTRUDE_DRAG,
  showEdgePoints: LS_SHOW_EDGE_POINTS,
  screenSpaceCsg: LS_SCREEN_SPACE_CSG,
  perspective: LS_PERSPECTIVE,
  controlsPreset: LS_CONTROLS_PRESET,
  autoReload: LS_AUTO_RELOAD,
  autosaveDebounceSec: LS_AUTOSAVE_DEBOUNCE,
  cloudSyncIntervalMin: LS_CLOUD_SYNC_INTERVAL,
};

/** Set one setting: persists it to localStorage (unless it's session-only)
 *  and returns the next immutable SettingsState. The view layer calls this
 *  and never touches localStorage keys itself. */
export function updateSetting<K extends keyof SettingsState>(
  settings: SettingsState,
  key: K,
  value: SettingsState[K],
): SettingsState {
  if (key === "overlayDepthBias") {
    value = clampOverlayDepthBias(value) as SettingsState[K];
  }
  if (key === "autosaveDebounceSec") {
    value = clampAutosaveDebounceSec(value) as SettingsState[K];
    autosaveDebounceSecCache = value as number;
  }
  if (key === "cloudSyncIntervalMin") {
    value = clampCloudSyncIntervalMin(value) as SettingsState[K];
    cloudSyncIntervalMinCache = value as number;
  }
  const lsKey = LS_KEY_BY_SETTING[key];
  if (lsKey !== undefined) {
    saveSetting(lsKey, key === "units" ? JSON.stringify(value) : String(value));
  }
  // Ticket dc02f591: the HMR mute handshake (main.tsx) must follow a LIVE
  // toggle, not only the next boot.
  if (key === "autoReload" && typeof window !== "undefined") {
    window.dispatchEvent(new Event("fd:autoreload-changed"));
  }
  return { ...settings, [key]: value };
}
