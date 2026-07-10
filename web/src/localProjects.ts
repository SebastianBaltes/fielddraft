/**
 * Local-project registry (ticket 14661fc4).
 *
 * Remembers `FileSystemFileHandle`s of locally opened `.fdraft` files in
 * IndexedDB (handles are structured-cloneable), so the Projects overview can
 * list local projects and reopen them without a file picker. Chromium-only —
 * {@link isLocalProjectsSupported} gates every UI surface; on other browsers
 * the classic `<input type=file>` open path remains and nothing is listed.
 *
 * Reopening may require re-granting permission; `openLocalProject` runs the
 * `queryPermission`/`requestPermission` dance and must therefore be called
 * from a user gesture (a click).
 */

/* Minimal File System Access typings — `showOpenFilePicker` and the
 * permission methods are not in lib.dom yet. */
export interface FsaFileHandle {
  readonly kind: "file";
  readonly name: string;
  getFile(): Promise<File>;
  createWritable(): Promise<{
    write(data: BufferSource | Blob | string): Promise<void>;
    close(): Promise<void>;
  }>;
  /** Absent on some handle kinds (e.g. OPFS) — treated as granted then. */
  queryPermission?(desc: { mode: "read" | "readwrite" }): Promise<string>;
  requestPermission?(desc: { mode: "read" | "readwrite" }): Promise<string>;
  isSameEntry(other: FsaFileHandle): Promise<boolean>;
}

export interface LocalProjectEntry {
  id: string;
  name: string;
  lastOpened: number;
  handle: FsaFileHandle;
}

/** Listing shape for the UI (no live handle). */
export interface LocalProjectSummary {
  id: string;
  name: string;
  lastOpened: number;
}

export function isLocalProjectsSupported(): boolean {
  return (
    typeof window !== "undefined" &&
    "showOpenFilePicker" in window &&
    typeof indexedDB !== "undefined"
  );
}

const DB_NAME = "fielddraft-local";
const DB_VERSION = 2;
const STORE = "projects";
/** Per-project thumbnail PNGs (Phase 3c). Out-of-line, keyed by the local
 *  project id — mutable/overwritable, so NOT content-addressed like the
 *  cloud/asset blobs. Bumping to v2 adds this store for existing users. */
const THUMB_STORE = "thumbnails";

function openDb(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const req = indexedDB.open(DB_NAME, DB_VERSION);
    req.onupgradeneeded = () => {
      const db = req.result;
      if (!db.objectStoreNames.contains(STORE)) {
        db.createObjectStore(STORE, { keyPath: "id" });
      }
      if (!db.objectStoreNames.contains(THUMB_STORE)) {
        db.createObjectStore(THUMB_STORE);
      }
    };
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
}

function tx<T>(
  store: string,
  mode: IDBTransactionMode,
  run: (store: IDBObjectStore) => IDBRequest<T>,
): Promise<T> {
  return openDb().then(
    (db) =>
      new Promise<T>((resolve, reject) => {
        const t = db.transaction(store, mode);
        const req = run(t.objectStore(store));
        req.onsuccess = () => resolve(req.result);
        req.onerror = () => reject(req.error);
        t.oncomplete = () => db.close();
        t.onabort = () => db.close();
      }),
  );
}

export async function listLocalProjects(): Promise<LocalProjectSummary[]> {
  if (!isLocalProjectsSupported()) return [];
  try {
    const all = await tx<LocalProjectEntry[]>(STORE, "readonly", (s) =>
      s.getAll(),
    );
    return all
      .map(({ id, name, lastOpened }) => ({ id, name, lastOpened }))
      .sort((a, b) => b.lastOpened - a.lastOpened);
  } catch {
    return [];
  }
}

/** Remember (or refresh) a handle. Dedupes against existing entries via
 *  `isSameEntry`, so reopening the same file updates its timestamp instead
 *  of growing the list. Returns the entry id. */
export async function rememberLocalProject(
  handle: FsaFileHandle,
): Promise<string> {
  const all = await tx<LocalProjectEntry[]>(STORE, "readonly", (s) =>
    s.getAll(),
  );
  let id: string = crypto.randomUUID();
  for (const e of all) {
    try {
      if (await e.handle.isSameEntry(handle)) {
        id = e.id;
        break;
      }
    } catch {
      // Foreign/stale handle — ignore.
    }
  }
  const entry: LocalProjectEntry = {
    id,
    name: handle.name.replace(/\.(fdraft|json)$/i, ""),
    lastOpened: Date.now(),
    handle,
  };
  await tx(STORE, "readwrite", (s) => s.put(entry));
  return id;
}

export async function removeLocalProject(id: string): Promise<void> {
  await tx(STORE, "readwrite", (s) => s.delete(id));
  // Forget the preview too, so a later same-id entry never inherits it.
  await removeLocalThumbnail(id);
}

/** Store (overwrite) a local project's preview PNG. Best-effort; no-op where
 *  IndexedDB / the File System Access API is unavailable. */
export async function putLocalThumbnail(id: string, png: Blob): Promise<void> {
  if (!isLocalProjectsSupported()) return;
  try {
    await tx(THUMB_STORE, "readwrite", (s) => s.put(png, id));
  } catch {
    // A missing thumbnail is harmless.
  }
}

/** The stored preview PNG for a local project, or null. */
export async function getLocalThumbnail(id: string): Promise<Blob | null> {
  if (!isLocalProjectsSupported()) return null;
  try {
    const v = await tx<Blob | undefined>(THUMB_STORE, "readonly", (s) =>
      s.get(id),
    );
    return v instanceof Blob ? v : null;
  } catch {
    return null;
  }
}

async function removeLocalThumbnail(id: string): Promise<void> {
  if (!isLocalProjectsSupported()) return;
  try {
    await tx(THUMB_STORE, "readwrite", (s) => s.delete(id));
  } catch {
    // best-effort
  }
}

/** Fetch the stored handle and ensure readwrite permission (prompts if
 *  needed — call from a user gesture). Returns null when the entry is gone
 *  or the user denied access. */
export async function localProjectHandle(
  id: string,
): Promise<FsaFileHandle | null> {
  const entry = await tx<LocalProjectEntry | undefined>(STORE, "readonly", (s) =>
    s.get(id),
  );
  if (!entry) return null;
  const handle = entry.handle;
  try {
    // Handle kinds without the permission API (OPFS) are always granted.
    if (
      typeof handle.queryPermission === "function" &&
      (await handle.queryPermission({ mode: "readwrite" })) !== "granted"
    ) {
      if (
        typeof handle.requestPermission !== "function" ||
        (await handle.requestPermission({ mode: "readwrite" })) !== "granted"
      ) {
        return null;
      }
    }
  } catch {
    return null;
  }
  await tx(STORE, "readwrite", (s) =>
    s.put({ ...entry, lastOpened: Date.now() }),
  );
  return handle;
}
