/**
 * Cloud auto-sync scheduler (P3b, docs/PROJECT-ASSET-STORAGE-PLAN §5.7).
 *
 * A server-bound project is pushed to the cloud on three triggers, each gated
 * so it only fires when there is something to save — the session is `dirty`
 * AND a `projectId` is bound:
 *
 *   1. periodic   — a background timer (interval from settings; 0 = off),
 *   2. exit editor — when the user navigates from the editor back to the
 *                    overview (awaited, reliable — the tab stays alive),
 *   3. beforeunload — best-effort on tab close (keepalive PATCH / fire-and-
 *                     forget; see App.tsx for the documented limit).
 *
 * After a successful sync `syncState` flips to `clean`, so any redundant
 * trigger that follows is a no-op via the same dirty gate — no extra
 * de-duplication is needed.
 *
 * This module is deliberately free of React and store imports: its deps are
 * injected (getSyncState / getProjectId / sync + optional timer overrides), so
 * the trigger logic is unit-testable and App only has to wire it up.
 */

/** The sync lifecycle states relevant to a cloud-bound project. Mirrors
 *  `StoreState.syncState`; only `"dirty"` means "there are unsynced edits". */
export type CloudSyncState =
  | "clean"
  | "dirty"
  | "syncing"
  | "error"
  | "offline";

export interface CloudSyncDeps {
  /** Read the CURRENT sync state (live — never a stale snapshot). */
  getSyncState: () => CloudSyncState;
  /** Read the CURRENT bound server project id (`null` = guest/offline). */
  getProjectId: () => string | null;
  /** Kick off a sync. Its promise is intentionally ignored here — callers
   *  that need to await do so themselves. */
  sync: () => void;
}

/** A cloud sync is worthwhile only when a project is bound AND has unsynced
 *  edits. `syncing`/`clean`/`error`/`offline` all short-circuit. */
export function shouldCloudSync(
  syncState: CloudSyncState,
  projectId: string | null,
): boolean {
  return projectId != null && syncState === "dirty";
}

/**
 * Fire a sync iff the project is dirty and bound; returns whether it triggered.
 * Shared by the exit-editor and beforeunload triggers (and the periodic tick).
 */
export function cloudSyncIfDirty(deps: CloudSyncDeps): boolean {
  if (!shouldCloudSync(deps.getSyncState(), deps.getProjectId())) return false;
  deps.sync();
  return true;
}

export interface CloudSyncTimerOptions extends CloudSyncDeps {
  /** Tick period in milliseconds. `<= 0` disables the timer (no-op stop). */
  intervalMs: number;
  /** Scheduler overrides, primarily for tests. */
  setIntervalImpl?: typeof setInterval;
  clearIntervalImpl?: typeof clearInterval;
}

/**
 * Start the periodic dirty-gated cloud sync. Returns a `stop()` that clears the
 * timer (call it on unmount / interval change). `intervalMs <= 0` starts no
 * timer and returns a no-op stop — so "0 = off" needs no special-casing at the
 * call site.
 */
export function startCloudSyncTimer(opts: CloudSyncTimerOptions): () => void {
  if (!(opts.intervalMs > 0)) return () => {};
  const setIntervalFn = opts.setIntervalImpl ?? setInterval;
  const clearIntervalFn = opts.clearIntervalImpl ?? clearInterval;
  const handle = setIntervalFn(() => {
    cloudSyncIfDirty(opts);
  }, opts.intervalMs);
  return () => clearIntervalFn(handle);
}
