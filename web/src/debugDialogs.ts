/**
 * Registry of the Debug ▸ … dialogs — the single source of truth that drives
 * the whole "debug dialog" machinery. Adding a dialog is ONE entry here; the
 * project-persisted open/settings state (`projectDebug`) and the menu +
 * rendering (`SettingsMenu`) are both generic over this list, so a new dialog
 * inherits all of it for free:
 *
 *   - a module-store OPEN flag a project load can flip,
 *   - a snapshot of the dialog's localStorage settings in the project file,
 *   - a remount-on-restore so an already-open dialog re-reads restored settings,
 *   - mutual exclusion for dialogs that share one overlay/store.
 *
 * To add a dialog: append a spec here, then supply its UI descriptor in
 * `SettingsMenu` (TypeScript's `Record<DebugDialogId, …>` makes that mandatory)
 * and write the dialog component + its own overlay/store. No edits to
 * `projectDebug` are needed.
 */

export interface DebugDialogSpec {
  /** Stable id. Drives the open-flag key AND the persisted wire format: a
   *  dialog with id `foo` serialises as `fooOpen` (bool) + `foo` (a snapshot
   *  of its settings slot). */
  readonly id: string;
  /** localStorage slot the dialog owns its settings under. The project carries
   *  a snapshot of this slot, never a second source of truth. */
  readonly settingsKey: string;
  /** Dialogs that publish into the SAME overlay/store are mutually exclusive:
   *  opening one closes the others in its group. Without this the closing
   *  dialog's unmount would clear the store the survivor just wrote. */
  readonly exclusiveGroup?: string;
}

// `exclusiveGroup` is spelled out (even as `undefined`) on every entry so the
// `as const` union carries the property uniformly — otherwise it would be
// absent from the type of the entries that omit it.
export const DEBUG_DIALOG_SPECS = [
  { id: "sdfPlane", settingsKey: "fielddraft.debug.sdfPlane", exclusiveGroup: undefined },
  { id: "mesher", settingsKey: "fielddraft.debug.mesher", exclusiveGroup: "mesher" },
  { id: "mesherPasm", settingsKey: "fielddraft.debug.mesherPasm", exclusiveGroup: "mesher" },
  { id: "lichen", settingsKey: "fielddraft.debug.lichen", exclusiveGroup: undefined },
  { id: "voxel", settingsKey: "fielddraft.debug.voxel", exclusiveGroup: undefined },
] as const satisfies readonly DebugDialogSpec[];

/** Union of the registered dialog ids ("sdfPlane" | "mesher" | …). */
export type DebugDialogId = (typeof DEBUG_DIALOG_SPECS)[number]["id"];
