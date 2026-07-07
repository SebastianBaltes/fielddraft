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
const STORE = "projects";

function openDb(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const req = indexedDB.open(DB_NAME, 1);
    req.onupgradeneeded = () => {
      if (!req.result.objectStoreNames.contains(STORE)) {
        req.result.createObjectStore(STORE, { keyPath: "id" });
      }
    };
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
}

function tx<T>(
  mode: IDBTransactionMode,
  run: (store: IDBObjectStore) => IDBRequest<T>,
): Promise<T> {
  return openDb().then(
    (db) =>
      new Promise<T>((resolve, reject) => {
        const t = db.transaction(STORE, mode);
        const req = run(t.objectStore(STORE));
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
    const all = await tx<LocalProjectEntry[]>("readonly", (s) => s.getAll());
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
  const all = await tx<LocalProjectEntry[]>("readonly", (s) => s.getAll());
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
  await tx("readwrite", (s) => s.put(entry));
  return id;
}

export async function removeLocalProject(id: string): Promise<void> {
  await tx("readwrite", (s) => s.delete(id));
}

/** Fetch the stored handle and ensure readwrite permission (prompts if
 *  needed — call from a user gesture). Returns null when the entry is gone
 *  or the user denied access. */
export async function localProjectHandle(
  id: string,
): Promise<FsaFileHandle | null> {
  const entry = await tx<LocalProjectEntry | undefined>("readonly", (s) =>
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
  await tx("readwrite", (s) =>
    s.put({ ...entry, lastOpened: Date.now() }),
  );
  return handle;
}
