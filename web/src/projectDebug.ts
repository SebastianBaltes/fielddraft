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
 *
 * Everything here is GENERIC over the `debugDialogs` registry: the per-dialog
 * open flag, (de)serialisation and mutual exclusion all iterate that list, so a
 * new dialog needs no edit in this file. `Part-AABB` is the one non-dialog
 * exception (a bare overlay toggle with its own store, no settings snapshot).
 */

import { getPartAabbVisible, setPartAabbVisible } from "./partAabbStore";
import { DEBUG_DIALOG_SPECS, type DebugDialogId } from "./debugDialogs";

/** The `Project.debug` blob: for each dialog a `${id}Open` flag and an `${id}`
 *  settings snapshot, plus the Part-AABB overlay flag. All optional — an absent
 *  key means off/closed. */
export type ProjectDebug = Partial<
  Record<`${DebugDialogId}Open`, boolean> &
    Record<DebugDialogId, Record<string, unknown>> & {
      partAabbVisible: boolean;
    }
>;

// ── Dialog open flags (+ remount revision) ───────────────────────────

export interface DebugDialogsState {
  /** id → is the dialog open. */
  open: Record<DebugDialogId, boolean>;
  /** Bumped by applyProjectDebug — keys the dialog elements so an already-
   *  open dialog remounts and re-reads its (just overwritten) settings. */
  revision: number;
}

const allClosed = (): Record<DebugDialogId, boolean> =>
  Object.fromEntries(DEBUG_DIALOG_SPECS.map((d) => [d.id, false])) as Record<
    DebugDialogId,
    boolean
  >;

let state: DebugDialogsState = { open: allClosed(), revision: 0 };
const listeners = new Set<() => void>();

function notify(): void {
  for (const l of listeners) l();
}

export function getDebugDialogs(): DebugDialogsState {
  return state;
}

export function subscribeDebugDialogs(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** Within each exclusiveGroup keep only the LAST requested-open dialog in
 *  registry order — group-mates share one overlay store, so at most one may be
 *  open. Used on restore (`applyProjectDebug`), where several may be requested. */
function resolveExclusiveGroups(
  open: Record<DebugDialogId, boolean>,
): Record<DebugDialogId, boolean> {
  const out = { ...open };
  const winner = new Map<string, DebugDialogId>();
  for (const d of DEBUG_DIALOG_SPECS) {
    if (!d.exclusiveGroup || !out[d.id]) continue;
    const prev = winner.get(d.exclusiveGroup);
    if (prev !== undefined) out[prev] = false;
    winner.set(d.exclusiveGroup, d.id);
  }
  return out;
}

/** Open or close a debug dialog. Opening one closes any dialog sharing its
 *  overlay/store (its exclusiveGroup) — the just-opened dialog always wins. */
export function setDebugDialogOpen(id: DebugDialogId, open: boolean): void {
  if (state.open[id] === open) return;
  const next = { ...state.open, [id]: open };
  if (open) {
    const { exclusiveGroup } = DEBUG_DIALOG_SPECS.find((d) => d.id === id)!;
    if (exclusiveGroup) {
      for (const d of DEBUG_DIALOG_SPECS) {
        if (d.id !== id && d.exclusiveGroup === exclusiveGroup) next[d.id] = false;
      }
    }
  }
  state = { open: next, revision: state.revision };
  notify();
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
  const anyOpen = DEBUG_DIALOG_SPECS.some((d) => state.open[d.id]);
  if (!anyOpen && !partAabb) return undefined;
  const out: Record<string, unknown> = {};
  for (const d of DEBUG_DIALOG_SPECS) {
    if (!state.open[d.id]) continue;
    out[`${d.id}Open`] = true;
    const slot = readSettingsSlot(d.settingsKey);
    if (slot) out[d.id] = slot;
  }
  if (partAabb) out.partAabbVisible = true;
  return out as ProjectDebug;
}

/** Restore a loaded project's debug state: overwrite the dialog settings
 *  slots, set the overlay flag and the open flags, and bump the revision so
 *  open dialogs remount onto the restored settings. `undefined` (project
 *  without debug state) closes everything — the state must be reproducible
 *  in BOTH directions. */
export function applyProjectDebug(debug: ProjectDebug | undefined): void {
  for (const d of DEBUG_DIALOG_SPECS) {
    const slot = debug?.[d.id];
    if (slot) writeSettingsSlot(d.settingsKey, slot);
  }
  setPartAabbVisible(debug?.partAabbVisible === true);

  const requested = allClosed();
  for (const d of DEBUG_DIALOG_SPECS) {
    requested[d.id] = debug?.[`${d.id}Open`] === true;
  }
  state = {
    open: resolveExclusiveGroups(requested),
    revision: state.revision + 1,
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
  const out: Record<string, unknown> = {};
  for (const d of DEBUG_DIALOG_SPECS) {
    if (p[`${d.id}Open`] === true) out[`${d.id}Open`] = true;
    const settings = rec(p[d.id]);
    if (settings) out[d.id] = settings;
  }
  if (p.partAabbVisible === true) out.partAabbVisible = true;
  return Object.keys(out).length > 0 ? (out as ProjectDebug) : undefined;
}
