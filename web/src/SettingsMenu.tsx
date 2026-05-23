import { useEffect, useRef, useState, useCallback } from "react";
import type { KernelStore, StoreState } from "./store";

/* ── localStorage keys ──────────────────────────────────────── */
const LS_COMPACT = "fielddraft.compact";
const LS_UNITS = "fielddraft.units";
const LS_THEME = "fielddraft.theme";
const LS_RENDER_MODE = "fielddraft.renderMode";

export type WorkspaceUnits = "mm" | "inch";
export type ThemeChoice = "light" | "dark";
export type RenderMode = "sdf-vm" | "sdf-deferred";

const VALID_RENDER_MODES: RenderMode[] = ["sdf-vm", "sdf-deferred"];

export interface SettingsState {
  compact: boolean;
  units: WorkspaceUnits;
  theme: ThemeChoice;
  renderMode: RenderMode;
}

/** Load all settings from localStorage with safe fallbacks. */
export function loadSettings(): SettingsState {
  let compact = false;
  let units: WorkspaceUnits = "mm";
  let theme: ThemeChoice = "light";
  let renderMode: RenderMode = "sdf-deferred";
  try {
    compact = localStorage.getItem(LS_COMPACT) === "true";
    const rawUnits = localStorage.getItem(LS_UNITS);
    if (rawUnits === "mm" || rawUnits === "inch") units = rawUnits;
    const rawTheme = localStorage.getItem(LS_THEME);
    if (rawTheme === "light" || rawTheme === "dark") theme = rawTheme;
    const rawMode = localStorage.getItem(LS_RENDER_MODE);
    if (rawMode && VALID_RENDER_MODES.includes(rawMode as RenderMode))
      renderMode = rawMode as RenderMode;
  } catch {
    // localStorage may be unavailable
  }
  return { compact, units, theme, renderMode };
}

function saveSetting(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    // ignore
  }
}

/* ── Keyboard shortcuts table ──────────────────────────────── */
const SHORTCUTS: { keys: string; action: string }[] = [
  { keys: "B", action: "Add Box" },
  { keys: "S", action: "Add Sphere" },
  { keys: "C", action: "Add Cylinder" },
  { keys: "T", action: "Add Torus" },
  { keys: "H", action: "Add Hole" },
  { keys: "Del", action: "Delete selection" },
  { keys: "Esc", action: "Clear selection" },
  { keys: "Ctrl+Z", action: "Undo" },
  { keys: "Ctrl+Shift+Z", action: "Redo" },
  { keys: "Ctrl+A", action: "Select all" },
  { keys: "Ctrl+D", action: "Duplicate" },
  { keys: "Ctrl+S", action: "Save project" },
  { keys: "1 / 2 / 3", action: "Front / Top / Right view" },
  { keys: "0", action: "Isometric view" },
  { keys: "Arrows", action: "Nudge selection" },
  { keys: "F1", action: "Toggle help overlay" },
];

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
  const popoverRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);

  const close = useCallback(() => setOpen(false), []);

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
            className={"menu-item" + (state?.assembly.active ? " is-active" : "")}
            data-testid="menu-assembly"
            onClick={() =>
              menuAction(() => {
                if (state?.assembly.active) store?.exitAssembly();
                else store?.enterAssembly();
              })
            }
          >
            Assembly
          </button>
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
          file.text().then((json) => store?.loadProjectFromJson(json));
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

/* ── Settings dialog ───────────────────────────────────────── */

function SettingsDialog({
  settings,
  onSettingsChange,
  onClose,
}: {
  settings: SettingsState;
  onSettingsChange: (next: SettingsState) => void;
  onClose: () => void;
}) {
  const [shortcutsExpanded, setShortcutsExpanded] = useState(false);
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

  const setUnits = (units: WorkspaceUnits) => {
    const next = { ...settings, units };
    saveSetting(LS_UNITS, units);
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

        <div className="dialog-body">
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

          <div className="setting-row" data-testid="setting-units">
            <span className="setting-label">Units</span>
            <div className="segmented">
              <button
                type="button"
                className={"seg" + (settings.units === "mm" ? " is-active" : "")}
                data-testid="setting-units-mm"
                onClick={() => setUnits("mm")}
              >
                mm
              </button>
              <button
                type="button"
                className={"seg" + (settings.units === "inch" ? " is-active" : "")}
                data-testid="setting-units-inch"
                onClick={() => setUnits("inch")}
              >
                inch
              </button>
            </div>
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
            </select>
          </div>

          <div className="setting-section" data-testid="setting-shortcuts">
            <button
              type="button"
              className="section-toggle"
              data-testid="setting-shortcuts-toggle"
              aria-expanded={shortcutsExpanded}
              onClick={() => setShortcutsExpanded((v) => !v)}
            >
              <span className="chevron">
                {shortcutsExpanded ? "▾" : "▸"}
              </span>
              Keyboard Shortcuts
            </button>
            {shortcutsExpanded && (
              <table
                className="shortcuts-table"
                data-testid="shortcuts-table"
              >
                <tbody>
                  {SHORTCUTS.map((s) => (
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
    </div>
  );
}
