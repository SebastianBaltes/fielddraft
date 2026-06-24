import { createContext, useContext, useEffect, useRef, useState, useCallback } from "react";
import type { KernelStore, StoreState } from "./store";
import { ReportDialog } from "./ReportDialog";
import {
  CONTROLS_PRESETS,
  CONTROLS_PRESET_OPTIONS,
  DEFAULT_CONTROLS_PRESET,
  isControlsPresetId,
  viewShortcutRow,
  type ControlsPresetId,
} from "./controlsPreset";

/* ── localStorage keys ──────────────────────────────────────── */
const LS_COMPACT = "fielddraft.compact";
const LS_UNITS = "fielddraft.units";
const LS_THEME = "fielddraft.theme";
const LS_RENDER_MODE = "fielddraft.renderMode";
const LS_BREP_EDGE_MODE = "fielddraft.brepEdgeMode";
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
/** Shared with main.tsx's HMR guard — kept here so all localStorage keys live
 *  in one place. */
export const LS_AUTO_RELOAD = "fielddraft.autoReload";

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

export type ThemeChoice = "light" | "dark";
export type RenderMode = "sdf-vm" | "sdf-deferred" | "mesh" | "brep" | "webgl";
export type MeshAlgorithm = "mdc" | "rfta" | "mt";
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
const VALID_MESH_ALGORITHMS: MeshAlgorithm[] = ["mdc", "rfta", "mt"];
const VALID_BREP_EDGE_MODES: BRepEdgeMode[] = ["xray", "visible", "none"];

export interface SettingsState {
  compact: boolean;
  units: WorkspaceUnits;
  theme: ThemeChoice;
  renderMode: RenderMode;
  meshAlgorithm: MeshAlgorithm;
  brepEdgeMode: BRepEdgeMode;
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
}

function validOr<K extends string>(opts: { key: K }[], val: unknown, fallback: K): K {
  return opts.some((o) => o.key === val) ? (val as K) : fallback;
}
function validDec(val: unknown): DecimalFormat {
  return DECIMAL_OPTIONS.some((o) => o.key === val) ? (val as DecimalFormat) : DEFAULT_UNITS.lengthDecimals;
}

function parseUnits(raw: string | null): WorkspaceUnits {
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
  "frameLoop",
  "parametricWeave",
  "dragBrepRecompute",
  "liveExtrudeDrag",
  "showEdgePoints",
  "screenSpaceCsg",
  "perspective",
  "controlsPreset",
  "autoReload",
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

/** Load all settings from localStorage with safe fallbacks. */
export function loadSettings(): SettingsState {
  let compact = false;
  let units: WorkspaceUnits = DEFAULT_UNITS;
  let theme: ThemeChoice = "light";
  let renderMode: RenderMode = "sdf-deferred";
  let meshAlgorithm: MeshAlgorithm = "mdc";
  let brepEdgeMode: BRepEdgeMode = "xray";
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
  } catch {
    // localStorage may be unavailable
  }
  const loaded: SettingsState = { compact, units, theme, renderMode, meshAlgorithm, brepEdgeMode, frameLoop, parametricWeave, dragBrepRecompute, liveExtrudeDrag, showEdgePoints, showOctreeCells, screenSpaceCsg, perspective, controlsPreset, autoReload };
  if (bootOverride) {
    const merged = { ...loaded, ...bootOverride };
    bootOverride = null; // one-shot: only the first read (React mount) honors it
    return merged;
  }
  return loaded;
}

function saveSetting(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    // ignore
  }
}

/* ── Keyboard shortcuts table ──────────────────────────────── */
function shortcutRows(preset: ControlsPresetId): { keys: string; action: string }[] {
  return [
    { keys: "B", action: "Add Box" },
    { keys: "S", action: "Add Sphere" },
    { keys: "C", action: "Add Cylinder" },
    { keys: "T", action: "Add Torus" },
    { keys: "H", action: "Add Hole" },
    { keys: "G", action: "Move gizmo" },
    { keys: "R", action: "Rotate gizmo" },
    { keys: "Del", action: "Delete selection" },
    { keys: "Esc", action: "Clear selection" },
    { keys: "Ctrl+Z", action: "Undo" },
    { keys: "Ctrl+Shift+Z", action: "Redo" },
    { keys: "Ctrl+A", action: "Select all" },
    { keys: "Ctrl+D", action: "Duplicate" },
    { keys: "Ctrl+S", action: "Save project" },
    viewShortcutRow(CONTROLS_PRESETS[preset]),
    { keys: "Arrows", action: "Nudge selection" },
    { keys: "F1", action: "Toggle help overlay" },
    { keys: "F2", action: "Report an issue" },
  ];
}

/* ── Component ──────────────────────────────────────────────── */

interface SettingsMenuProps {
  store: KernelStore | null;
  state: StoreState | null;
  settings: SettingsState;
  onSettingsChange: (next: SettingsState) => void;
}

export function SettingsMenu({
  store,
  state,
  settings,
  onSettingsChange,
}: SettingsMenuProps) {
  const [open, setOpen] = useState(false);
  const [settingsDialogOpen, setSettingsDialogOpen] = useState(false);
  const [reportDialogOpen, setReportDialogOpen] = useState(false);
  const popoverRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);

  const close = useCallback(() => setOpen(false), []);

  // F2 (handled globally in App) opens the Report Issue dialog from anywhere.
  useEffect(() => {
    const openReport = () => setReportDialogOpen(true);
    window.addEventListener("fd:report-issue", openReport);
    return () => window.removeEventListener("fd:report-issue", openReport);
  }, []);

  useEffect(() => {
    if (!open) return;
    const handleClick = (e: MouseEvent) => {
      const target = e.target as Node | null;
      if (
        popoverRef.current &&
        !popoverRef.current.contains(target) &&
        buttonRef.current &&
        !buttonRef.current.contains(target)
      ) {
        close();
      }
    };
    const timer = setTimeout(() => {
      window.addEventListener("click", handleClick);
    }, 0);
    return () => {
      clearTimeout(timer);
      window.removeEventListener("click", handleClick);
    };
  }, [open, close]);

  useEffect(() => {
    if (!open) return;
    const handleKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        close();
        e.stopPropagation();
      }
    };
    window.addEventListener("keydown", handleKey, true);
    return () => window.removeEventListener("keydown", handleKey, true);
  }, [open, close]);

  const hasFeatures = (state?.features.length ?? 0) > 0;

  const menuAction = (fn: () => void) => {
    fn();
    close();
  };

  return (
    <div className="fd-settings-menu" data-testid="settings-menu">
      <button
        ref={buttonRef}
        type="button"
        className="burger"
        title="Menu"
        data-testid="settings-menu-trigger"
        aria-expanded={open}
        aria-haspopup="menu"
        onClick={() => setOpen((v) => !v)}
      >
        &#9776;
      </button>

      {open && (
        <div
          ref={popoverRef}
          className="popover"
          role="menu"
          aria-label="Application menu"
          data-testid="settings-menu-popover"
        >
          <button
            type="button"
            role="menuitem"
            className="menu-item"
            data-testid="menu-new-project"
            onClick={() =>
              menuAction(() => {
                if (
                  !state ||
                  state.features.length === 0 ||
                  confirm("Discard current project?")
                ) {
                  store?.newProject();
                }
              })
            }
          >
            New
          </button>
          <button
            type="button"
            role="menuitem"
            className="menu-item"
            data-testid="menu-open"
            onClick={() =>
              menuAction(() =>
                document
                  .querySelector<HTMLInputElement>(
                    '[data-testid="load-project-input"]',
                  )
                  ?.click(),
              )
            }
          >
            Open…
          </button>
          <button
            type="button"
            role="menuitem"
            className="menu-item"
            data-testid="menu-save"
            onClick={() => menuAction(() => store?.saveProjectToFile())}
          >
            <span>Save</span>
            <span className="menu-shortcut">Ctrl+S</span>
          </button>

          <div className="menu-separator" role="separator" />

          <button
            type="button"
            role="menuitem"
            className="menu-item"
            data-testid="menu-import"
            onClick={() =>
              menuAction(() =>
                document
                  .querySelector<HTMLInputElement>(
                    '[data-testid="import-stl-input"]',
                  )
                  ?.click(),
              )
            }
          >
            Import…
          </button>

          <ExportSubmenu
            store={store}
            state={state}
            disabled={!hasFeatures}
            onClose={close}
          />

          <div className="menu-separator" role="separator" />

          <button
            type="button"
            role="menuitem"
            className="menu-item"
            data-testid="menu-share"
            disabled={!hasFeatures || !state?.projectId}
            onClick={() => menuAction(() => store?.publishProject())}
          >
            Share
          </button>

          <div className="menu-separator" role="separator" />

          <button
            type="button"
            role="menuitem"
            className="menu-item"
            data-testid="menu-undo"
            disabled={!state?.canUndo}
            onClick={() => menuAction(() => { store?.undo(); })}
          >
            <span>
              Undo
              {(state?.undoDepth ?? 0) > 0 ? ` (${state!.undoDepth})` : ""}
            </span>
            <span className="menu-shortcut">Ctrl+Z</span>
          </button>
          <button
            type="button"
            role="menuitem"
            className="menu-item"
            data-testid="menu-redo"
            disabled={!state?.canRedo}
            onClick={() => menuAction(() => { store?.redo(); })}
          >
            <span>
              Redo
              {(state?.redoDepth ?? 0) > 0 ? ` (${state!.redoDepth})` : ""}
            </span>
            <span className="menu-shortcut">Ctrl+Shift+Z</span>
          </button>

          <div className="menu-separator" role="separator" />

          <button
            type="button"
            role="menuitem"
            className="menu-item"
            data-testid="menu-settings"
            onClick={() => menuAction(() => setSettingsDialogOpen(true))}
          >
            Settings…
          </button>

          <div className="menu-separator" role="separator" />

          <button
            type="button"
            role="menuitem"
            className="menu-item"
            data-testid="menu-report"
            onClick={() => menuAction(() => setReportDialogOpen(true))}
          >
            <span>Report Issue…</span>
            <span className="menu-shortcut">F2</span>
          </button>
        </div>
      )}

      {/* Hidden file inputs (always in DOM) */}
      <input
        type="file"
        accept=".fdraft,.json"
        className="hidden-file-input"
        data-testid="load-project-input"
        onChange={(e) => {
          const file = e.currentTarget.files?.[0];
          if (!file) return;
          // wu2v: read bytes so ZIP `.fdraft` containers load; the byte
          // loader falls back to JSON for legacy plain-text files.
          file.arrayBuffer().then((buf) => store?.loadProjectFromBytes(buf));
          e.currentTarget.value = "";
        }}
      />
      <input
        type="file"
        accept=".stl,.glb,.gltf,.obj,.step,.stp,.3mf"
        className="hidden-file-input"
        data-testid="import-stl-input"
        onChange={(e) => {
          const file = e.currentTarget.files?.[0];
          if (!file || !store) return;
          const name = file.name.toLowerCase();
          const sizeMB = (file.size / (1024 * 1024)).toFixed(1);
          console.log(`fielddraft: file selected — "${file.name}" (${sizeMB} MB)`);
          if (name.endsWith(".3mf")) {
            file.arrayBuffer().then((buf) => store.import3mfFile(buf));
          } else if (name.endsWith(".step") || name.endsWith(".stp")) {
            file.arrayBuffer().then((buf) => store.importStepFile(buf));
          } else if (name.endsWith(".glb") || name.endsWith(".gltf")) {
            file.arrayBuffer().then((buf) => store.loadReferenceGlb(buf));
          } else if (name.endsWith(".obj")) {
            file.text().then(async (text) => {
              if (!(await store.loadReferenceObj(text))) {
                alert("OBJ import failed — no geometry found in file or SDF conversion failed.");
              }
            });
          } else {
            file.arrayBuffer().then(async (buf) => {
              if (!(await store.loadReferenceMesh(buf))) {
                alert("STL import failed — file may be ASCII STL (only binary STL is supported), corrupt, or SDF conversion failed.");
              }
            });
          }
          e.currentTarget.value = "";
        }}
      />

      {settingsDialogOpen && (
        <SettingsDialog
          settings={settings}
          onSettingsChange={onSettingsChange}
          onClose={() => setSettingsDialogOpen(false)}
        />
      )}

      {reportDialogOpen && (
        <ReportDialog
          store={store}
          onClose={() => setReportDialogOpen(false)}
        />
      )}
    </div>
  );
}

/* ── Export submenu ─────────────────────────────────────────── */

function ExportSubmenu({
  store,
  state,
  disabled,
  onClose,
}: {
  store: KernelStore | null;
  state: StoreState | null;
  disabled: boolean;
  onClose: () => void;
}) {
  const action = (fn: () => void) => {
    fn();
    onClose();
  };

  const r3fDisabled =
    !state?.assembly.active ||
    state.assembly.document.parts.length === 0;

  return (
    <div className="submenu-wrapper">
      <button
        type="button"
        role="menuitem"
        className="menu-item submenu-trigger"
        disabled={disabled}
        aria-haspopup="menu"
        data-testid="menu-export"
      >
        <span>Export</span>
        <span className="menu-chevron">&#9656;</span>
      </button>
      <div className="submenu" role="menu">
        <button
          type="button"
          role="menuitem"
          className="menu-item"
          data-testid="menu-export-stl"
          onClick={() => action(() => store?.downloadStl())}
        >
          STL
        </button>
        <button
          type="button"
          role="menuitem"
          className="menu-item"
          data-testid="menu-export-glb"
          onClick={() => action(() => store?.downloadGlb())}
        >
          GLB
        </button>
        <button
          type="button"
          role="menuitem"
          className="menu-item"
          data-testid="menu-export-obj"
          onClick={() => action(() => store?.downloadObj())}
        >
          OBJ
        </button>
        <button
          type="button"
          role="menuitem"
          className="menu-item"
          data-testid="menu-export-3mf"
          onClick={() => action(() => store?.download3mfRoundtrip())}
        >
          3MF
        </button>
        <button
          type="button"
          role="menuitem"
          className="menu-item"
          data-testid="menu-export-step"
          onClick={() => action(() => store?.downloadStep())}
        >
          STEP
        </button>
        <div className="menu-separator" role="separator" />
        <button
          type="button"
          role="menuitem"
          className="menu-item"
          data-testid="menu-export-r3f"
          disabled={r3fDisabled}
          onClick={() => action(() => store?.exportR3F())}
        >
          R3F
        </button>
      </div>
    </div>
  );
}

/* ── Unit row helper ───────────────────────────────────────── */

function UnitRow<K extends string>({
  label, value, options, decimals, onUnitChange, onDecimalsChange, testId,
}: {
  label: string;
  value: K;
  options: { key: K; label: string }[];
  decimals: DecimalFormat;
  onUnitChange: (v: K) => void;
  onDecimalsChange: (v: DecimalFormat) => void;
  testId: string;
}) {
  return (
    <div className="unit-row">
      <div className="unit-field">
        <span className="unit-field-label">{label}</span>
        <select
          className="mode-select"
          value={value}
          data-testid={`setting-${testId}-unit`}
          onChange={(e) => onUnitChange(e.target.value as K)}
        >
          {options.map((o) => (
            <option key={o.key} value={o.key}>{o.label}</option>
          ))}
        </select>
      </div>
      <div className="unit-field">
        <span className="unit-field-label">Display decimals</span>
        <select
          className="mode-select"
          value={decimals}
          data-testid={`setting-${testId}-decimals`}
          onChange={(e) => onDecimalsChange(e.target.value as DecimalFormat)}
        >
          {DECIMAL_OPTIONS.map((o) => (
            <option key={o.key} value={o.key}>{o.label}</option>
          ))}
        </select>
      </div>
    </div>
  );
}

/* ── Workspace unit descriptors (data-driven) ─────────────── */

interface UnitDescriptor {
  label: string;
  unitKey: keyof WorkspaceUnits;
  decKey: keyof WorkspaceUnits;
  options: { key: string; label: string }[];
  testId: string;
}

const UNIT_GROUPS: { title: string; items: UnitDescriptor[] }[] = [
  {
    title: "Length",
    items: [
      { label: "Length", unitKey: "lengthUnit", decKey: "lengthDecimals", options: LENGTH_UNIT_OPTIONS, testId: "length" },
      { label: "Linear acceleration", unitKey: "linearAccelerationUnit", decKey: "linearAccelerationDecimals", options: LINEAR_ACCELERATION_UNIT_OPTIONS, testId: "linear-acceleration" },
    ],
  },
  {
    title: "Angle",
    items: [
      { label: "Angle", unitKey: "angleUnit", decKey: "angleDecimals", options: ANGLE_UNIT_OPTIONS, testId: "angle" },
      { label: "Angular velocity", unitKey: "angularVelocityUnit", decKey: "angularVelocityDecimals", options: ANGULAR_VELOCITY_UNIT_OPTIONS, testId: "angular-velocity" },
    ],
  },
  {
    title: "Mechanical",
    items: [
      { label: "Mass", unitKey: "massUnit", decKey: "massDecimals", options: MASS_UNIT_OPTIONS, testId: "mass" },
      { label: "Density", unitKey: "densityUnit", decKey: "densityDecimals", options: DENSITY_UNIT_OPTIONS, testId: "density" },
      { label: "Force", unitKey: "forceUnit", decKey: "forceDecimals", options: FORCE_UNIT_OPTIONS, testId: "force" },
      { label: "Moment", unitKey: "momentUnit", decKey: "momentDecimals", options: MOMENT_UNIT_OPTIONS, testId: "moment" },
      { label: "Pressure", unitKey: "pressureUnit", decKey: "pressureDecimals", options: PRESSURE_UNIT_OPTIONS, testId: "pressure" },
      { label: "Energy", unitKey: "energyUnit", decKey: "energyDecimals", options: ENERGY_UNIT_OPTIONS, testId: "energy" },
    ],
  },
];

/* ── Settings dialog ───────────────────────────────────────── */

type SettingsTab = "ui" | "units" | "shortcuts";

function SettingsDialog({
  settings,
  onSettingsChange,
  onClose,
}: {
  settings: SettingsState;
  onSettingsChange: (next: SettingsState) => void;
  onClose: () => void;
}) {
  const [activeTab, setActiveTab] = useState<SettingsTab>("ui");
  const dialogRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const handleKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        onClose();
        e.stopPropagation();
      }
    };
    window.addEventListener("keydown", handleKey, true);
    return () => window.removeEventListener("keydown", handleKey, true);
  }, [onClose]);

  const toggleCompact = () => {
    const next = { ...settings, compact: !settings.compact };
    saveSetting(LS_COMPACT, String(next.compact));
    onSettingsChange(next);
  };

  const updateUnits = (patch: Partial<WorkspaceUnits>) => {
    const units = { ...settings.units, ...patch };
    const next = { ...settings, units };
    saveSetting(LS_UNITS, JSON.stringify(units));
    onSettingsChange(next);
  };

  const setTheme = (theme: ThemeChoice) => {
    const next = { ...settings, theme };
    saveSetting(LS_THEME, theme);
    onSettingsChange(next);
  };

  const setRenderMode = (mode: RenderMode) => {
    const next = { ...settings, renderMode: mode };
    saveSetting(LS_RENDER_MODE, mode);
    onSettingsChange(next);
  };

  const setMeshAlgorithm = (algorithm: MeshAlgorithm) => {
    const next = { ...settings, meshAlgorithm: algorithm };
    saveSetting(LS_MESH_ALGORITHM, algorithm);
    onSettingsChange(next);
  };

  const setBRepEdgeMode = (mode: BRepEdgeMode) => {
    const next = { ...settings, brepEdgeMode: mode };
    saveSetting(LS_BREP_EDGE_MODE, mode);
    onSettingsChange(next);
  };

  const setFrameLoop = (mode: FrameLoopMode) => {
    const next = { ...settings, frameLoop: mode };
    saveSetting(LS_FRAME_LOOP, mode);
    onSettingsChange(next);
  };

  const toggleParametricWeave = () => {
    const next = { ...settings, parametricWeave: !settings.parametricWeave };
    saveSetting(LS_PARAMETRIC_WEAVE, String(next.parametricWeave));
    onSettingsChange(next);
  };

  const toggleDragBrepRecompute = () => {
    const next = { ...settings, dragBrepRecompute: !settings.dragBrepRecompute };
    saveSetting(LS_DRAG_BREP_RECOMPUTE, String(next.dragBrepRecompute));
    onSettingsChange(next);
  };

  const toggleLiveExtrudeDrag = () => {
    const next = { ...settings, liveExtrudeDrag: !settings.liveExtrudeDrag };
    saveSetting(LS_LIVE_EXTRUDE_DRAG, String(next.liveExtrudeDrag));
    onSettingsChange(next);
  };

  const toggleShowEdgePoints = () => {
    const next = { ...settings, showEdgePoints: !settings.showEdgePoints };
    saveSetting(LS_SHOW_EDGE_POINTS, String(next.showEdgePoints));
    onSettingsChange(next);
  };

  const toggleScreenSpaceCsg = () => {
    const next = { ...settings, screenSpaceCsg: !settings.screenSpaceCsg };
    saveSetting(LS_SCREEN_SPACE_CSG, String(next.screenSpaceCsg));
    onSettingsChange(next);
  };

  const togglePerspective = () => {
    const next = { ...settings, perspective: !settings.perspective };
    saveSetting(LS_PERSPECTIVE, String(next.perspective));
    onSettingsChange(next);
  };

  const toggleAutoReload = () => {
    const next = { ...settings, autoReload: !settings.autoReload };
    saveSetting(LS_AUTO_RELOAD, String(next.autoReload));
    onSettingsChange(next);
  };

  const setControlsPreset = (preset: ControlsPresetId) => {
    const next = { ...settings, controlsPreset: preset };
    saveSetting(LS_CONTROLS_PRESET, preset);
    onSettingsChange(next);
  };

  const toggleShowOctreeCells = () => {
    const next = { ...settings, showOctreeCells: !settings.showOctreeCells };
    onSettingsChange(next);
  };

  const TABS: { key: SettingsTab; label: string }[] = [
    { key: "ui", label: "UI" },
    { key: "units", label: "Workspace Units" },
    { key: "shortcuts", label: "Shortcuts" },
  ];

  return (
    <div
      className="fd-settings-overlay"
      data-testid="settings-dialog-overlay"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div
        ref={dialogRef}
        className="fd-settings-dialog"
        role="dialog"
        aria-label="Settings"
        data-testid="settings-dialog"
      >
        <div className="dialog-header">
          <h3 className="dialog-title">Settings</h3>
          <button
            type="button"
            className="dialog-close"
            data-testid="settings-dialog-close"
            onClick={onClose}
          >
            &times;
          </button>
        </div>

        <div className="settings-tab-bar" role="tablist">
          {TABS.map((tab) => (
            <button
              key={tab.key}
              type="button"
              role="tab"
              className={"settings-tab" + (activeTab === tab.key ? " is-active" : "")}
              aria-selected={activeTab === tab.key}
              data-testid={`settings-tab-${tab.key}`}
              onClick={() => setActiveTab(tab.key)}
            >
              {tab.label}
            </button>
          ))}
        </div>

        <div className="dialog-body">
          {activeTab === "ui" && (
            <>
              <div className="setting-group">
                <div className="setting-group-title">Interface</div>
                <div className="setting-row" data-testid="setting-compact">
                  <span className="setting-label">Compact toolbar</span>
                  <button
                    type="button"
                    className={"toggle" + (settings.compact ? " is-on" : "")}
                    role="switch"
                    aria-checked={settings.compact}
                    data-testid="setting-compact-toggle"
                    onClick={toggleCompact}
                  >
                    {settings.compact ? "On" : "Off"}
                  </button>
                </div>

                <div className="setting-row" data-testid="setting-theme">
                  <span className="setting-label">Theme</span>
                  <div className="segmented">
                    <button
                      type="button"
                      className={"seg" + (settings.theme === "light" ? " is-active" : "")}
                      data-testid="setting-theme-light"
                      onClick={() => setTheme("light")}
                    >
                      Light
                    </button>
                    <button
                      type="button"
                      className={"seg" + (settings.theme === "dark" ? " is-active" : "")}
                      data-testid="setting-theme-dark"
                      onClick={() => setTheme("dark")}
                    >
                      Dark
                    </button>
                  </div>
                </div>
              </div>

              <div className="setting-group">
                <div className="setting-group-title">Camera</div>
                <div className="setting-row" data-testid="setting-controls-preset">
                  <span
                    className="setting-label"
                    title="Mouse and keyboard scheme of another CAD package — orbit/pan buttons, multi-select modifiers and view keys follow that program's habits."
                  >
                    Controls preset
                  </span>
                  <select
                    className="mode-select"
                    value={settings.controlsPreset}
                    data-testid="setting-controls-preset-select"
                    onChange={(e) => setControlsPreset(e.target.value as ControlsPresetId)}
                  >
                    {CONTROLS_PRESET_OPTIONS.map((o) => (
                      <option key={o.key} value={o.key}>{o.label}</option>
                    ))}
                  </select>
                </div>

                <div className="setting-row" data-testid="setting-perspective">
                  <span
                    className="setting-label"
                    title="Perspective projection (fov 45) instead of the default orthographic view. Off (default) keeps an isometric-style parallel projection where equal lengths read as equal at every depth — the CAD default."
                  >
                    Perspective projection
                  </span>
                  <button
                    type="button"
                    className={"toggle" + (settings.perspective ? " is-on" : "")}
                    role="switch"
                    aria-checked={settings.perspective}
                    data-testid="setting-perspective-toggle"
                    onClick={togglePerspective}
                  >
                    {settings.perspective ? "On" : "Off"}
                  </button>
                </div>
              </div>

              <div className="setting-group">
                <div className="setting-group-title">Rendering</div>
                <div className="setting-row" data-testid="setting-render-mode">
                  <span className="setting-label">Render</span>
                  <select
                    className="mode-select"
                    value={settings.renderMode}
                    data-testid="setting-render-mode-select"
                    onChange={(e) => setRenderMode(e.target.value as RenderMode)}
                  >
                    <option value="sdf-vm">SDF VM</option>
                    <option value="sdf-deferred">SDF Deferred</option>
                    <option value="mesh">Mesh</option>
                    <option value="brep">B-Rep</option>
                    <option value="webgl">WebGL</option>
                  </select>
                </div>

                {settings.renderMode === "mesh" && (
                  <div className="setting-row" data-testid="setting-mesh-algorithm">
                    <span className="setting-label">Mesh algorithm</span>
                    <select
                      className="mode-select"
                      value={settings.meshAlgorithm}
                      data-testid="setting-mesh-algorithm-select"
                      onChange={(e) => setMeshAlgorithm(e.target.value as MeshAlgorithm)}
                    >
                      <option value="mdc">MDC</option>
                      <option value="rfta">RFTA</option>
                      <option value="mt">Marching Tetrahedra</option>
                    </select>
                  </div>
                )}

                <div className="setting-row" data-testid="setting-brep-edges">
                  <span className="setting-label">B-Rep edges</span>
                  <select
                    className="mode-select"
                    value={settings.brepEdgeMode}
                    data-testid="setting-brep-edges-select"
                    onChange={(e) => setBRepEdgeMode(e.target.value as BRepEdgeMode)}
                  >
                    <option value="xray">X-Ray</option>
                    <option value="visible">Visible only</option>
                    <option value="none">Hidden</option>
                  </select>
                </div>

                <div className="setting-row" data-testid="setting-frame-loop">
                  <span className="setting-label">Render loop</span>
                  <div className="segmented">
                    <button
                      type="button"
                      className={"seg" + (settings.frameLoop === "demand" ? " is-active" : "")}
                      data-testid="setting-frame-loop-demand"
                      title="Render only when the scene changes (idle GPU when static)"
                      onClick={() => setFrameLoop("demand")}
                    >
                      On demand
                    </button>
                    <button
                      type="button"
                      className={"seg" + (settings.frameLoop === "always" ? " is-active" : "")}
                      data-testid="setting-frame-loop-always"
                      title="Render every frame — diagnostic: if a view only updates here, its on-demand invalidate is missing"
                      onClick={() => setFrameLoop("always")}
                    >
                      Always
                    </button>
                  </div>
                </div>

                {settings.renderMode === "webgl" && (
                  <div className="setting-row" data-testid="setting-screen-space-csg">
                    <span className="setting-label" title="Experiment (nur WebGL-Renderpfad): einstufiger Screen-Space-CSG-Subtraktionspass (A−B) über die parametrischen Hüllmeshes, statt den Subtrahenden wegzulassen. Visuell unverifiziertes Stage-1-Experiment — Default aus; mit Aus bleibt der Renderpfad unverändert.">
                      Screen-Space-CSG (experimentell)
                    </span>
                    <button
                      type="button"
                      className={"toggle" + (settings.screenSpaceCsg ? " is-on" : "")}
                      role="switch"
                      aria-checked={settings.screenSpaceCsg}
                      data-testid="setting-screen-space-csg-toggle"
                      onClick={toggleScreenSpaceCsg}
                    >
                      {settings.screenSpaceCsg ? "On" : "Off"}
                    </button>
                  </div>
                )}
              </div>

              <div className="setting-group">
                <div className="setting-group-title">Modeling</div>
                <div className="setting-row" data-testid="setting-parametric-weave">
                  <span className="setting-label" title="Exact analytic B-Rep for pure grundforms (Box) — skips the octree. Falls back automatically for anything not yet supported.">
                    Parametric weaving
                  </span>
                  <button
                    type="button"
                    className={"toggle" + (settings.parametricWeave ? " is-on" : "")}
                    role="switch"
                    aria-checked={settings.parametricWeave}
                    data-testid="setting-parametric-weave-toggle"
                    onClick={toggleParametricWeave}
                  >
                    {settings.parametricWeave ? "On" : "Off"}
                  </button>
                </div>

                <div className="setting-row" data-testid="setting-drag-brep">
                  <span className="setting-label" title="Re-weave and display the exact B-Rep live during gizmo drags. Off: the drag updates only the render mesh — affected B-Rep edges hide while dragging and re-weave once on release. Keeps drags fluid on weave-heavy models.">
                    B-Rep rebuild while dragging
                  </span>
                  <button
                    type="button"
                    className={"toggle" + (settings.dragBrepRecompute ? " is-on" : "")}
                    role="switch"
                    aria-checked={settings.dragBrepRecompute}
                    data-testid="setting-drag-brep-toggle"
                    onClick={toggleDragBrepRecompute}
                  >
                    {settings.dragBrepRecompute ? "On" : "Off"}
                  </button>
                </div>

                <div className="setting-row" data-testid="setting-live-extrude">
                  <span className="setting-label" title="Commit the extrude depth live while the arrow drag runs — the renderer shows the real body instead of an approximate preview prism. The exact B-Rep still weaves once on release (see 'B-Rep rebuild while dragging').">
                    Live extrude drag
                  </span>
                  <button
                    type="button"
                    className={"toggle" + (settings.liveExtrudeDrag ? " is-on" : "")}
                    role="switch"
                    aria-checked={settings.liveExtrudeDrag}
                    data-testid="setting-live-extrude-toggle"
                    onClick={toggleLiveExtrudeDrag}
                  >
                    {settings.liveExtrudeDrag ? "On" : "Off"}
                  </button>
                </div>
              </div>

              <div className="setting-group">
                <div className="setting-group-title">Debug</div>
                <div className="setting-row" data-testid="setting-edge-points">
                  <span className="setting-label" title="Highlight each B-Rep edge's individual sample points — reveals whether a NURBS edge is tessellated into many points or few.">
                    Edge sample points
                  </span>
                  <button
                    type="button"
                    className={"toggle" + (settings.showEdgePoints ? " is-on" : "")}
                    role="switch"
                    aria-checked={settings.showEdgePoints}
                    data-testid="setting-edge-points-toggle"
                    onClick={toggleShowEdgePoints}
                  >
                    {settings.showEdgePoints ? "On" : "Off"}
                  </button>
                </div>

                <div className="setting-row" data-testid="setting-octree-cells">
                  <span className="setting-label" title="Draw the topology octree's leaf cells as a wireframe grid, colour-coded by class (face / edge / vertex) — inspect the adaptive resolution the B-Rep weave used. Rebuilds the octree on the spot and can stall for minutes on complex bodies; resets to Off on reload.">
                    Octree cells
                  </span>
                  <button
                    type="button"
                    className={"toggle" + (settings.showOctreeCells ? " is-on" : "")}
                    role="switch"
                    aria-checked={settings.showOctreeCells}
                    data-testid="setting-octree-cells-toggle"
                    onClick={toggleShowOctreeCells}
                  >
                    {settings.showOctreeCells ? "On" : "Off"}
                  </button>
                </div>
              </div>

              {import.meta.hot && (
                <div className="setting-group">
                  <div className="setting-group-title">Developer</div>
                  <div className="setting-row" data-testid="setting-auto-reload">
                    <span className="setting-label" title="Dev only: when off, a code change that Vite can't hot-swap no longer reloads the page — your running session and in-progress project stay alive. Reload manually (F5) to pick the change up.">
                      Auto-reload on code change
                    </span>
                    <button
                      type="button"
                      className={"toggle" + (settings.autoReload ? " is-on" : "")}
                      role="switch"
                      aria-checked={settings.autoReload}
                      data-testid="setting-auto-reload-toggle"
                      onClick={toggleAutoReload}
                    >
                      {settings.autoReload ? "On" : "Off"}
                    </button>
                  </div>
                </div>
              )}
            </>
          )}

          {activeTab === "units" && (
            <div data-testid="setting-units">
              {UNIT_GROUPS.map((group) => (
                <div className="unit-group" key={group.title}>
                  <div className="unit-group-title">{group.title}</div>
                  {group.items.map((desc) => (
                    <UnitRow
                      key={desc.testId}
                      label={desc.label}
                      value={settings.units[desc.unitKey] as string}
                      options={desc.options}
                      decimals={settings.units[desc.decKey] as DecimalFormat}
                      onUnitChange={(v) => updateUnits({ [desc.unitKey]: v })}
                      onDecimalsChange={(v) => updateUnits({ [desc.decKey]: v })}
                      testId={desc.testId}
                    />
                  ))}
                </div>
              ))}
            </div>
          )}

          {activeTab === "shortcuts" && (
            <table
              className="shortcuts-table"
              data-testid="shortcuts-table"
            >
              <tbody>
                {shortcutRows(settings.controlsPreset).map((s) => (
                  <tr key={s.keys}>
                    <td className="shortcut-key">
                      {s.keys.split(" / ").map((k, i) => (
                        <span key={k}>
                          {i > 0 && " / "}
                          <kbd>{k.trim()}</kbd>
                        </span>
                      ))}
                    </td>
                    <td className="shortcut-action">{s.action}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>
      </div>
    </div>
  );
}
