/**
 * Local-file binding (Phase 4 — durable-local as explicit Save).
 *
 * A `.fdraft` opened via the File System Access API is bound here as the
 * project's SAVE TARGET. The working state lives in IndexedDB (continuous,
 * crash-safe autosave); this module does NOT mirror every autosave to disk
 * any more. Instead it tracks a local dirty flag — the working state has
 * diverged from what was last written to the file — and writes the file only
 * on an explicit Save (Ctrl+S / menu). See docs/PROJECT-ASSET-STORAGE-PLAN §4.
 *
 * A module singleton like the other bridges, exposed as a React external
 * store (`subscribeLocalFile` / `getLocalFileState`) so the menu can show the
 * bound filename and the dirty indicator.
 */

import type { FsaFileHandle } from "./localProjects";

/** UI-facing snapshot. A fresh object is produced only when something changes,
 *  so `useSyncExternalStore` can compare by reference. */
export interface LocalFileState {
  /** The bound file's name, or null when no local file is the save target. */
  name: string | null;
  /** Working state differs from what was last written to the file. */
  dirty: boolean;
}

let handle: FsaFileHandle | null = null;
let dirty = false;
let writing = false;
const listeners = new Set<() => void>();
let snapshot: LocalFileState = { name: null, dirty: false };

function rebuildSnapshot(): void {
  snapshot = { name: handle?.name ?? null, dirty };
}

function emit(): void {
  for (const listener of listeners) listener();
}

export function subscribeLocalFile(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function getLocalFileState(): LocalFileState {
  return snapshot;
}

/** Bind a `.fdraft` file as the explicit Save target. The freshly-opened
 *  document matches the file on disk, so this starts clean. */
export function bindLocalFile(fileHandle: FsaFileHandle): void {
  handle = fileHandle;
  dirty = false;
  rebuildSnapshot();
  emit();
}

/** Drop the save-target binding (opening a cloud/guest project, New, or a
 *  revoked permission). */
export function clearLocalFileBinding(): void {
  if (handle === null && !dirty) return;
  handle = null;
  dirty = false;
  rebuildSnapshot();
  emit();
}

export function isLocalFileBound(): boolean {
  return handle !== null;
}

/** The bound file's name, or null. */
export function localFileName(): string | null {
  return handle?.name ?? null;
}

export function isLocalDirty(): boolean {
  return dirty;
}

/** Mark the bound project as having unsaved changes. No-op when no local file
 *  is bound (guest/cloud projects track dirtiness elsewhere) or already dirty. */
export function markLocalDirty(): void {
  if (handle === null || dirty) return;
  dirty = true;
  rebuildSnapshot();
  emit();
}

/** Mark the bound project as saved (working state == file on disk). */
export function setLocalClean(): void {
  if (!dirty) return;
  dirty = false;
  rebuildSnapshot();
  emit();
}

/**
 * Write `bytes` into the bound file via `handle.createWritable()`. Returns
 * `true` on success, `false` when nothing is bound, a write is already in
 * flight, or the write failed (e.g. permission revoked). The caller
 * (`saveProjectToFile`) falls back to a download on `false`.
 */
export async function writeBoundFile(bytes: Uint8Array): Promise<boolean> {
  if (handle === null || writing) return false;
  writing = true;
  try {
    const w = await handle.createWritable();
    // Copy — the export may be a WASM-heap view.
    await w.write(bytes.slice().buffer as ArrayBuffer);
    await w.close();
    return true;
  } catch (err) {
    console.warn("fielddraft: writing the local project file failed", err);
    return false;
  } finally {
    writing = false;
  }
}
