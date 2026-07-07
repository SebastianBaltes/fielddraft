/**
 * Local-file binding (ticket 14661fc4) — autosave for LOCAL projects.
 *
 * While a `.fdraft` opened via the File System Access API is bound here,
 * every localStorage autosave write also mirrors the canonical ZIP bytes
 * back into the file on disk (the same bytes "Download project" produces).
 * A module singleton like the other bridges; App.tsx binds on local open
 * and clears when different content replaces the workspace.
 *
 * Writes are serialized: while one is in flight, further autosaves collapse
 * into a single trailing write. A revoked permission (NotAllowedError)
 * silently unbinds — the project keeps living in localStorage as before.
 */

import { setAutosaveMirror } from "./persistence";
import type { FsaFileHandle } from "./localProjects";

interface Binding {
  handle: FsaFileHandle;
  /** Produces the canonical `.fdraft` ZIP bytes (store.exportProjectFdraft). */
  exportBytes: () => Uint8Array;
}

let binding: Binding | null = null;
let writing = false;
let writeAgain = false;

async function writeNow(): Promise<void> {
  if (!binding || writing) {
    writeAgain = writing;
    return;
  }
  writing = true;
  try {
    do {
      writeAgain = false;
      const { handle, exportBytes } = binding;
      const bytes = exportBytes();
      const w = await handle.createWritable();
      // Copy — the export may be a WASM-heap view.
      await w.write(bytes.slice().buffer as ArrayBuffer);
      await w.close();
    } while (writeAgain && binding);
  } catch (err) {
    console.warn(
      "fielddraft: local file autosave failed — unbinding the file",
      err,
    );
    clearLocalFileBinding();
  } finally {
    writing = false;
  }
}

export function bindLocalFile(
  handle: FsaFileHandle,
  exportBytes: () => Uint8Array,
): void {
  binding = { handle, exportBytes };
  setAutosaveMirror(() => void writeNow());
}

export function clearLocalFileBinding(): void {
  binding = null;
  setAutosaveMirror(null);
}

/** The bound file's name, or null — UI hint ("autosaves to <name>"). */
export function localFileName(): string | null {
  return binding?.handle.name ?? null;
}
