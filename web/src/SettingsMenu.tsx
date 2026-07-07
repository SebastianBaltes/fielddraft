import {
  lazy,
  Suspense,
  useEffect,
  useRef,
  useState,
  useCallback,
  useSyncExternalStore,
} from "react";
import type { KernelStore, StoreState } from "./store";
import {
  getPartAabbVisible,
  subscribePartAabb,
  togglePartAabbVisible,
} from "./partAabbStore";
import { ReportDialog } from "./ReportDialog";
import { ExportDialog } from "./ExportDialog";
import { NumberInput } from "./NumberInput";
import { SdfPlaneDialog } from "./SdfPlaneDialog";
import { MesherDialog } from "./MesherDialog";
import {
  getDebugDialogs,
  setMesherDialogOpen,
  setSdfPlaneDialogOpen,
  subscribeDebugDialogs,
} from "./projectDebug";

// Lazy: the dialog pulls the full release-note history (releaseNotes.ts,
// thousands of lines of data) — that belongs in an on-demand chunk, not in
// the eager app bundle. The chunk loads on the first open (menu entry or
// version-badge click).
const ReleaseNotesDialog = lazy(() =>
  import("./ReleaseNotesDialog").then((m) => ({
    default: m.ReleaseNotesDialog,
  })),
);
import {
  CONTROLS_PRESETS,
  CONTROLS_PRESET_OPTIONS,
  viewShortcutRow,
  type ControlsPresetId,
} from "./controlsPreset";
import {
  ANGLE_UNIT_OPTIONS,
  ANGULAR_VELOCITY_UNIT_OPTIONS,
  DECIMAL_OPTIONS,
  DENSITY_UNIT_OPTIONS,
  ENERGY_UNIT_OPTIONS,
  FORCE_UNIT_OPTIONS,
  LENGTH_UNIT_OPTIONS,
  LINEAR_ACCELERATION_UNIT_OPTIONS,
  MASS_UNIT_OPTIONS,
  MOMENT_UNIT_OPTIONS,
  PRESSURE_UNIT_OPTIONS,
  updateSetting,
  type BRepEdgeMode,
  type DecimalFormat,
  type FrameLoopMode,
  type MeshAlgorithm,
  type RenderMode,
  type SettingsState,
  type ThemeChoice,
  type WorkspaceUnits,
} from "./settings";

// Transitional re-export for the renderer files (Viewport.tsx,
// viewportOverlays.tsx, viewportSdfOverlays.tsx) which are owned by a parallel
// work stream and still import these types from here. Everything else imports
// from ./settings directly.
export type { BRepEdgeMode, FrameLoopMode } from "./settings";

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
  /** Ticket 376bf4: document-scoped actions (New/Save/Import/Export/
   *  Share/Undo/Redo/Debug) only make sense in the modeling view — on the
   *  dashboard they'd operate on the invisible background workspace. */
  inProject: boolean;
  /** Ticket 3d0f20: "Open…" DOES work from the dashboard — it loads the
   *  local `.fdraft` as an unbound project and this callback switches the
   *  app into the modeling view (otherwise the load happens invisibly). */
  onEnterProject: () => void;
  /** Ticket 14661fc4: open a local .fdraft — App routes this to the File
   *  System Access picker (remember + disk autosave) or falls back to the
   *  hidden file input below on unsupported browsers. */
  onOpenLocal: () => void;
  /** Ticket 6c09c11f: turn the open LOCAL project into a cloud project.
   *  `null` when unavailable (guest, or already bound) — the menu item
   *  renders disabled with a hint then. */
  onSaveToCloud: (() => void) | null;
}

export function SettingsMenu({
  store,
  state,
  settings,
  onSettingsChange,
  inProject,
  onEnterProject,
  onOpenLocal,
  onSaveToCloud,
}: SettingsMenuProps) {
  const [open, setOpen] = useState(false);
  const [settingsDialogOpen, setSettingsDialogOpen] = useState(false);
  const [reportDialogOpen, setReportDialogOpen] = useState(false);
  const [releaseNotesOpen, setReleaseNotesOpen] = useState(false);
  const [exportDialog, setExportDialog] = useState<"stl" | "3mf" | null>(null);
  // Debug-dialog open flags live in projectDebug (module store), not local
  // state: a project load restores them (Project.debug), and the revision
  // keys remount open dialogs onto freshly-restored settings.
  const debugDialogs = useSyncExternalStore(subscribeDebugDialogs, getDebugDialogs);
  const partAabbVisible = useSyncExternalStore(
    subscribePartAabb,
    getPartAabbVisible,
  );
  const popoverRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);

  const close = useCallback(() => setOpen(false), []);

  // F2 (handled globally in App) opens the Report Issue dialog from anywhere.
  useEffect(() => {
    const openReport = () => setReportDialogOpen(true);
    window.addEventListener("fd:report-issue", openReport);
    return () => window.removeEventListener("fd:report-issue", openReport);
  }, []);

  // The version badge in the topbar dispatches this to open the release notes.
  useEffect(() => {
    const openNotes = () => setReleaseNotesOpen(true);
    window.addEventListener("fd:release-notes", openNotes);
    return () => window.removeEventListener("fd:release-notes", openNotes);
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
            disabled={!inProject}
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
            onClick={() => menuAction(onOpenLocal)}
          >
            Open local project…
          </button>
          <button
            type="button"
            role="menuitem"
            className="menu-item"
            data-testid="menu-save"
            disabled={!inProject}
            onClick={() => menuAction(() => store?.saveProjectToFile())}
          >
            <span>Download project</span>
            <span className="menu-shortcut">Ctrl+S</span>
          </button>
          {/* Ticket 6c09c11f: local vs cloud must be explicit. "Download
              project" writes the local .fdraft; this turns the open LOCAL
              project into a cloud project (creates + binds a server project;
              autosave keeps it in sync from then on). Bound projects are
              already in the cloud — disabled with a hint. */}
          <button
            type="button"
            role="menuitem"
            className="menu-item"
            data-testid="menu-save-to-cloud"
            disabled={!inProject || !onSaveToCloud}
            title={
              !inProject
                ? undefined
                : onSaveToCloud
                  ? "Create a cloud project from the current workspace"
                  : state?.projectId
                    ? "Already a cloud project — changes autosave"
                    : "Log in to save to the cloud"
            }
            onClick={() => menuAction(() => onSaveToCloud?.())}
          >
            Save to cloud
          </button>

          <div className="menu-separator" role="separator" />

          <button
            type="button"
            role="menuitem"
            className="menu-item"
            data-testid="menu-import"
            disabled={!inProject}
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
            disabled={!hasFeatures || !inProject}
            onClose={close}
            onExportDialog={setExportDialog}
          />

          <div className="menu-separator" role="separator" />

          <button
            type="button"
            role="menuitem"
            className="menu-item"
            data-testid="menu-share"
            disabled={!hasFeatures || !state?.projectId || !inProject}
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
            disabled={!state?.canUndo || !inProject}
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
            disabled={!state?.canRedo || !inProject}
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

          {/* Ticket e752b6 removed this; ticket b5a60122 brought it back with
              the F2 shortcut shown (and dropped the UserMenu copy instead).
              "Release Notes…" stays out — reachable via the version badge. */}
          <button
            type="button"
            role="menuitem"
            className="menu-item"
            data-testid="menu-report-issue"
            onClick={() => menuAction(() => setReportDialogOpen(true))}
          >
            <span>Report Issue…</span>
            <span className="menu-shortcut">F2</span>
          </button>

          <div className="menu-separator" role="separator" />

          <div className="submenu-wrapper">
            <button
              type="button"
              role="menuitem"
              className="menu-item submenu-trigger"
              aria-haspopup="menu"
              data-testid="menu-debug"
              disabled={!inProject}
            >
              <span>Debug</span>
              <span className="menu-chevron">&#9656;</span>
            </button>
            <div className="submenu" role="menu">
              <button
                type="button"
                role="menuitem"
                className="menu-item"
                data-testid="menu-debug-sdf-plane"
                onClick={() => menuAction(() => setSdfPlaneDialogOpen(true))}
              >
                SDF-Plane…
              </button>
              <button
                type="button"
                role="menuitem"
                className="menu-item"
                data-testid="menu-debug-mesher"
                onClick={() => menuAction(() => setMesherDialogOpen(true))}
              >
                Mesher…
              </button>
              <button
                type="button"
                role="menuitemcheckbox"
                aria-checked={partAabbVisible}
                className="menu-item"
                data-testid="menu-debug-part-aabb"
                onClick={() => menuAction(togglePartAabbVisible)}
              >
                {/* Ticket cvii: left-aligned with the sibling items (no leading
                    char/space); the on-state shows a TRAILING check, never indents. */}
                Part-AABB{partAabbVisible ? " ✓" : ""}
              </button>
            </div>
          </div>
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
          // Ticket 4z37: the explicit-open path persists + graduates a
          // deep-link repro tab so a reload restores the opened project.
          // Ticket 3d0f20: from the dashboard, detach the (invisibly
          // rebound) server project BEFORE loading — the local file is not
          // that project, and the load-time autosave flush would otherwise
          // PATCH the file's content over it. Then enter the modeling view;
          // previously the load happened without a view switch ("nothing
          // happens").
          file.arrayBuffer().then((buf) => {
            if (!store) return;
            if (!inProject) store.detachServerProject();
            if (store.openUserProjectFromBytes(buf) && !inProject) {
              onEnterProject();
            }
          });
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

      {releaseNotesOpen && (
        <Suspense fallback={null}>
          <ReleaseNotesDialog onClose={() => setReleaseNotesOpen(false)} />
        </Suspense>
      )}

      {exportDialog && store && (
        <ExportDialog
          store={store}
          format={exportDialog}
          onClose={() => setExportDialog(null)}
        />
      )}

      {debugDialogs.sdfPlaneOpen && (
        <SdfPlaneDialog
          key={`sdf-plane-${debugDialogs.revision}`}
          onClose={() => setSdfPlaneDialogOpen(false)}
        />
      )}
      {debugDialogs.mesherOpen && (
        <MesherDialog
          key={`mesher-${debugDialogs.revision}`}
          onClose={() => setMesherDialogOpen(false)}
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
  onExportDialog,
}: {
  store: KernelStore | null;
  state: StoreState | null;
  disabled: boolean;
  onClose: () => void;
  onExportDialog: (format: "stl" | "3mf") => void;
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
          onClick={() => action(() => onExportDialog("stl"))}
        >
          STL…
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
          onClick={() => action(() => onExportDialog("3mf"))}
        >
          3MF…
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

  // All persistence goes through settings.ts — the dialog only says WHICH
  // setting changes; updateSetting persists it and returns the next state.
  const set = <K extends keyof SettingsState>(key: K, value: SettingsState[K]) =>
    onSettingsChange(updateSetting(settings, key, value));

  const toggle = (key: keyof SettingsState & ("compact" | "parametricWeave" | "dragBrepRecompute" | "liveExtrudeDrag" | "showEdgePoints" | "showOctreeCells" | "screenSpaceCsg" | "perspective" | "autoReload")) =>
    set(key, !settings[key]);

  const updateUnits = (patch: Partial<WorkspaceUnits>) =>
    set("units", { ...settings.units, ...patch });

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
                    onClick={() => toggle("compact")}
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
                      onClick={() => set("theme", "light" as ThemeChoice)}
                    >
                      Light
                    </button>
                    <button
                      type="button"
                      className={"seg" + (settings.theme === "dark" ? " is-active" : "")}
                      data-testid="setting-theme-dark"
                      onClick={() => set("theme", "dark" as ThemeChoice)}
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
                    onChange={(e) => set("controlsPreset", e.target.value as ControlsPresetId)}
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
                    onClick={() => toggle("perspective")}
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
                    onChange={(e) => set("renderMode", e.target.value as RenderMode)}
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
                      onChange={(e) => set("meshAlgorithm", e.target.value as MeshAlgorithm)}
                    >
                      <option value="mdc">MDC</option>
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
                    onChange={(e) => set("brepEdgeMode", e.target.value as BRepEdgeMode)}
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
                      onClick={() => set("frameLoop", "demand" as FrameLoopMode)}
                    >
                      On demand
                    </button>
                    <button
                      type="button"
                      className={"seg" + (settings.frameLoop === "always" ? " is-active" : "")}
                      data-testid="setting-frame-loop-always"
                      title="Render every frame — diagnostic: if a view only updates here, its on-demand invalidate is missing"
                      onClick={() => set("frameLoop", "always" as FrameLoopMode)}
                    >
                      Always
                    </button>
                  </div>
                </div>

                {settings.renderMode === "webgl" && (
                  <div className="setting-row" data-testid="setting-screen-space-csg">
                    <span className="setting-label" title="Experiment (nur WebGL-Renderpfad): einstufiger Screen-Space-CSG-Subtraktionspass (A−B) über die parametrischen Hüllmeshes, statt den Subtrahenden wegzulassen. Visuell unverifiziertes Stage-1-Experiment — Default aus; mit Aus bleibt der Renderpfad unverändert.">
                      Screen-Space-CSG
                    </span>
                    <button
                      type="button"
                      className={"toggle" + (settings.screenSpaceCsg ? " is-on" : "")}
                      role="switch"
                      aria-checked={settings.screenSpaceCsg}
                      data-testid="setting-screen-space-csg-toggle"
                      onClick={() => toggle("screenSpaceCsg")}
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
                    onClick={() => toggle("parametricWeave")}
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
                    onClick={() => toggle("dragBrepRecompute")}
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
                    onClick={() => toggle("liveExtrudeDrag")}
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
                    onClick={() => toggle("showEdgePoints")}
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
                    onClick={() => toggle("showOctreeCells")}
                  >
                    {settings.showOctreeCells ? "On" : "Off"}
                  </button>
                </div>
              </div>

              {import.meta.hot && (
                <div className="setting-group">
                  <div className="setting-group-title">Developer</div>
                  <div className="setting-row" data-testid="setting-autosave-debounce">
                    <span className="setting-label" title="Änderungen (Dokument, Kamera, Debug-Dialoge) markieren das Projekt dirty; gespeichert wird, sobald so lange Ruhe war. Beim Schließen des Tabs wird immer sofort gespeichert.">
                      Autosave-Debounce (s)
                    </span>
                    <NumberInput
                      className="setting-number"
                      value={settings.autosaveDebounceSec}
                      min={0.5}
                      max={60}
                      onChange={(v) => set("autosaveDebounceSec", v)}
                      data-testid="setting-autosave-debounce-input"
                    />
                  </div>
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
                      onClick={() => toggle("autoReload")}
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
