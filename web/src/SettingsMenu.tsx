/**
 * K7.21a: Burger-Menu + Settings-Panel.
 *
 * Hamburger icon in the topbar opens a popover with:
 * - Compact-Mode toggle (migrated from the old inline toggle)
 * - Workspace-Units slot (mm/inch, placeholder for future)
 * - Keyboard-Shortcuts overview
 * - Theme slot (light/dark, placeholder for future)
 *
 * All settings persist to localStorage under the `fielddraft.*` namespace.
 * The popover closes on click-outside or Escape.
 */

import { useEffect, useRef, useState, useCallback } from "react";

/* ── localStorage keys ──────────────────────────────────────── */
const LS_COMPACT = "fielddraft.compact";
const LS_UNITS = "fielddraft.units";
const LS_THEME = "fielddraft.theme";
const LS_RENDER_MODE = "fielddraft.renderMode";

export type WorkspaceUnits = "mm" | "inch";
export type ThemeChoice = "light" | "dark";
export type RenderMode = "dc" | "sdf" | "sdf-vm" | "sdf-baked" | "sdf-batch" | "sdf-hybrid";

const VALID_RENDER_MODES: RenderMode[] = ["dc", "sdf", "sdf-vm", "sdf-baked", "sdf-batch", "sdf-hybrid"];

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
  let renderMode: RenderMode = "dc";
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
    // localStorage may be unavailable (private browsing, etc.)
  }
  return { compact, units, theme, renderMode };
}

/** Persist a single setting to localStorage. */
function saveSetting(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    // ignore
  }
}

/* ── Keyboard shortcuts table data ──────────────────────────── */
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
  settings: SettingsState;
  onSettingsChange: (next: SettingsState) => void;
}

export function SettingsMenu({ settings, onSettingsChange }: SettingsMenuProps) {
  const [open, setOpen] = useState(false);
  const [shortcutsExpanded, setShortcutsExpanded] = useState(false);
  const popoverRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);

  const close = useCallback(() => setOpen(false), []);

  // Close on click outside
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
    // Use setTimeout to avoid the same click that opened the menu
    // from immediately closing it.
    const timer = setTimeout(() => {
      window.addEventListener("click", handleClick);
    }, 0);
    return () => {
      clearTimeout(timer);
      window.removeEventListener("click", handleClick);
    };
  }, [open, close]);

  // Close on Escape
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
    <div className="fd-settings-menu" data-testid="settings-menu">
      <button
        ref={buttonRef}
        type="button"
        className="burger"
        title="Settings"
        data-testid="settings-menu-trigger"
        aria-expanded={open}
        aria-haspopup="true"
        onClick={() => setOpen((v) => !v)}
      >
        &#9776;
      </button>

      {open && (
        <div
          ref={popoverRef}
          className="popover"
          role="dialog"
          aria-label="Settings"
          data-testid="settings-menu-popover"
        >
          <h3 className="popover-title">Settings</h3>

          {/* ── Compact Mode ──────────────────────────────── */}
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

          {/* ── Workspace Units ───────────────────────────── */}
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

          {/* ── Theme ─────────────────────────────────────── */}
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

          {/* ── Render Mode ─────────────────────────────── */}
          <div className="setting-row" data-testid="setting-render-mode">
            <span className="setting-label">Render</span>
            <select
              className="mode-select"
              value={settings.renderMode}
              data-testid="setting-render-mode-select"
              onChange={(e) =>
                setRenderMode(e.target.value as RenderMode)
              }
            >
              <option value="dc">DC Mesh</option>
              <option value="sdf">SDF Transpiled</option>
              <option value="sdf-vm">SDF VM</option>
              <option value="sdf-baked">SDF Baked</option>
              <option value="sdf-batch">Batchmarch</option>
              <option value="sdf-hybrid">Hybrid</option>
            </select>
          </div>

          {/* ── Keyboard Shortcuts ────────────────────────── */}
          <div className="setting-section" data-testid="setting-shortcuts">
            <button
              type="button"
              className="section-toggle"
              data-testid="setting-shortcuts-toggle"
              aria-expanded={shortcutsExpanded}
              onClick={() => setShortcutsExpanded((v) => !v)}
            >
              <span className="chevron">{shortcutsExpanded ? "▾" : "▸"}</span>
              Keyboard Shortcuts
            </button>
            {shortcutsExpanded && (
              <table className="shortcuts-table" data-testid="shortcuts-table">
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
      )}
    </div>
  );
}
