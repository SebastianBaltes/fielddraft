/**
 * Project-persisted debug state.
 *
 * The debug tools (Debug ▸ SDF-Plane…, Debug ▸ Mesher…, Debug ▸ Part-AABB) are
 * part of a project's reproducible state during development: a ticket or a
 * saved project should re-open with the same debug dialogs, parameters and
 * overlays it was captured with. This module is the single funnel between
 * those tools and the project file:
 *
 *  - a module-scoped store for the dialog OPEN flags (lifted out of
 *    SettingsMenu's local state so a project load can open/close them),
 *  - `collectProjectDebug()` → the `Project.debug` blob at save time,
 *  - `applyProjectDebug()`  ← restore on project load: writes the dialog
 *    settings into their localStorage slots, sets the open flags and bumps a
 *    revision counter that remounts open dialogs so they re-read the settings.
 *
 * The dialogs keep owning their settings persistence (localStorage) — the
 * project carries a snapshot of those slots, not a second source of truth.
 */

import { getPartAabbVisible, setPartAabbVisible } from "./partAabbStore";

/** localStorage slots the dialogs persist their settings under. */
const SDF_PLANE_SETTINGS_KEY = "fielddraft.debug.sdfPlane";
const MESHER_SETTINGS_KEY = "fielddraft.debug.mesher";
const MESHER_PASM_SETTINGS_KEY = "fielddraft.debug.mesherPasm";

export interface ProjectDebug {
  /** Debug ▸ SDF-Plane dialog open? */
  sdfPlaneOpen?: boolean;
  /** Snapshot of the SDF-Plane dialog settings (shape owned by the dialog). */
  sdfPlane?: Record<string, unknown>;
  /** Debug ▸ Mesher dialog open? */
  mesherOpen?: boolean;
  /** Snapshot of the Mesher dialog settings (shape owned by the dialog). */
  mesher?: Record<string, unknown>;
  /** Debug ▸ Mesher PASM dialog open? */
  mesherPasmOpen?: boolean;
  /** Snapshot of the Mesher PASM dialog settings (shape owned by the dialog). */
  mesherPasm?: Record<string, unknown>;
  /** Debug ▸ Part-AABB overlay visible? */
  partAabbVisible?: boolean;
}

// ── Dialog open flags (+ remount revision) ───────────────────────────

export interface DebugDialogs {
  sdfPlaneOpen: boolean;
  mesherOpen: boolean;
  mesherPasmOpen: boolean;
  /** Bumped by applyProjectDebug — keys the dialog elements so an already-
   *  open dialog remounts and re-reads its (just overwritten) settings. */
  revision: number;
}

let dialogs: DebugDialogs = {
  sdfPlaneOpen: false,
  mesherOpen: false,
  mesherPasmOpen: false,
  revision: 0,
};
const listeners = new Set<() => void>();

function notify(): void {
  for (const l of listeners) l();
}

export function getDebugDialogs(): DebugDialogs {
  return dialogs;
}

export function setSdfPlaneDialogOpen(open: boolean): void {
  if (dialogs.sdfPlaneOpen === open) return;
  dialogs = { ...dialogs, sdfPlaneOpen: open };
  notify();
}

// Both Mesher dialogs publish into the SAME `mesherDebugStore` singleton
// (one overlay), so they are mutually exclusive: opening one closes the
// other — the closing dialog's unmount would otherwise clear the store the
// surviving dialog just wrote.
export function setMesherDialogOpen(open: boolean): void {
  if (dialogs.mesherOpen === open) return;
  dialogs = {
    ...dialogs,
    mesherOpen: open,
    mesherPasmOpen: open ? false : dialogs.mesherPasmOpen,
  };
  notify();
}

export function setMesherPasmDialogOpen(open: boolean): void {
  if (dialogs.mesherPasmOpen === open) return;
  dialogs = {
    ...dialogs,
    mesherPasmOpen: open,
    mesherOpen: open ? false : dialogs.mesherOpen,
  };
  notify();
}

export function subscribeDebugDialogs(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

// ── Project (de)serialisation ────────────────────────────────────────

function readSettingsSlot(key: string): Record<string, unknown> | undefined {
  try {
    const raw = localStorage.getItem(key);
    if (!raw) return undefined;
    const parsed = JSON.parse(raw);
    return typeof parsed === "object" && parsed !== null
      ? (parsed as Record<string, unknown>)
      : undefined;
  } catch {
    return undefined;
  }
}

function writeSettingsSlot(key: string, value: Record<string, unknown>): void {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    // Storage unavailable — the dialog falls back to its defaults.
  }
}

/** The `Project.debug` blob for a save, or `undefined` when no debug tool is
 *  active (keeps ordinary project files free of debug noise — settings alone
 *  are meaningless without an open dialog/overlay to show them). */
export function collectProjectDebug(): ProjectDebug | undefined {
  const partAabb = getPartAabbVisible();
  if (
    !dialogs.sdfPlaneOpen &&
    !dialogs.mesherOpen &&
    !dialogs.mesherPasmOpen &&
    !partAabb
  )
    return undefined;
  return {
    ...(dialogs.sdfPlaneOpen ? { sdfPlaneOpen: true } : {}),
    ...(dialogs.sdfPlaneOpen
      ? { sdfPlane: readSettingsSlot(SDF_PLANE_SETTINGS_KEY) }
      : {}),
    ...(dialogs.mesherOpen ? { mesherOpen: true } : {}),
    ...(dialogs.mesherOpen ? { mesher: readSettingsSlot(MESHER_SETTINGS_KEY) } : {}),
    ...(dialogs.mesherPasmOpen ? { mesherPasmOpen: true } : {}),
    ...(dialogs.mesherPasmOpen
      ? { mesherPasm: readSettingsSlot(MESHER_PASM_SETTINGS_KEY) }
      : {}),
    ...(partAabb ? { partAabbVisible: true } : {}),
  };
}

/** Restore a loaded project's debug state: overwrite the dialog settings
 *  slots, set the overlay flag and the open flags, and bump the revision so
 *  open dialogs remount onto the restored settings. `undefined` (project
 *  without debug state) closes everything — the state must be reproducible
 *  in BOTH directions. */
export function applyProjectDebug(debug: ProjectDebug | undefined): void {
  if (debug?.sdfPlane) writeSettingsSlot(SDF_PLANE_SETTINGS_KEY, debug.sdfPlane);
  if (debug?.mesher) writeSettingsSlot(MESHER_SETTINGS_KEY, debug.mesher);
  if (debug?.mesherPasm)
    writeSettingsSlot(MESHER_PASM_SETTINGS_KEY, debug.mesherPasm);
  setPartAabbVisible(debug?.partAabbVisible === true);
  // Mesher and Mesher PASM are mutually exclusive (shared store); a blob
  // claiming both keeps only the PASM dialog.
  const pasmOpen = debug?.mesherPasmOpen === true;
  dialogs = {
    sdfPlaneOpen: debug?.sdfPlaneOpen === true,
    mesherOpen: debug?.mesherOpen === true && !pasmOpen,
    mesherPasmOpen: pasmOpen,
    revision: dialogs.revision + 1,
  };
  notify();
}

/** Validate an untrusted `Project.debug` blob (project file load). Returns
 *  `undefined` for anything malformed — a bad debug blob must never make an
 *  otherwise-valid project unloadable. */
export function parseProjectDebug(raw: unknown): ProjectDebug | undefined {
  if (typeof raw !== "object" || raw === null) return undefined;
  const p = raw as Record<string, unknown>;
  const rec = (v: unknown): Record<string, unknown> | undefined =>
    typeof v === "object" && v !== null && !Array.isArray(v)
      ? (v as Record<string, unknown>)
      : undefined;
  const out: ProjectDebug = {
    ...(p.sdfPlaneOpen === true ? { sdfPlaneOpen: true } : {}),
    ...(rec(p.sdfPlane) ? { sdfPlane: rec(p.sdfPlane) } : {}),
    ...(p.mesherOpen === true ? { mesherOpen: true } : {}),
    ...(rec(p.mesher) ? { mesher: rec(p.mesher) } : {}),
    ...(p.mesherPasmOpen === true ? { mesherPasmOpen: true } : {}),
    ...(rec(p.mesherPasm) ? { mesherPasm: rec(p.mesherPasm) } : {}),
    ...(p.partAabbVisible === true ? { partAabbVisible: true } : {}),
  };
  return Object.keys(out).length > 0 ? out : undefined;
}
