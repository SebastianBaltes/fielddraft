/**
 * assetStore — content-addressed asset persistence (Phase 1a).
 *
 * The keystone of the project-asset refactor (docs/PROJECT-ASSET-STORAGE-PLAN
 * §5.1/§5.3): large binary payloads (imported meshes, fonts, later PNGs and
 * thumbnails) become immutable, content-addressed blobs (sha256), and the
 * persisted document only references them by hash. This module owns:
 *
 *  - {@link sha256Hex}: the canonical hash of raw bytes (Web Crypto).
 *  - {@link AssetStore}: the storage abstraction the rest of the app talks to,
 *    keeping it backend-agnostic.
 *  - {@link IndexedDbAssetStore}: the browser baseline backend (its own
 *    IndexedDB database, separate from `fielddraft-local`). Degrades to a
 *    quiet no-op where `indexedDB` is absent (SSR / vitest jsdom), mirroring
 *    `persistence.ts`' `safeStorage`.
 *
 * Framework-free by design — no three.js / React / kernel imports — so it can
 * be unit-tested and reused from workers.
 *
 * P1a is the self-contained foundation. Wiring this into serialize/load/
 * autosave (and migrating the old `fielddraft.project.v2` localStorage slot
 * into it) is P1b — see the plan §5.3 and the P1a report.
 */

/** Lowercase hex sha256 of `bytes` via Web Crypto. Deterministic across
 *  runs/platforms, so identical bytes always yield the same blob key (the
 *  basis for dedup). Throws if `crypto.subtle` is unavailable. */
export async function sha256Hex(bytes: Uint8Array): Promise<string> {
  const subtle = globalThis.crypto?.subtle;
  if (!subtle) {
    throw new Error("fielddraft: Web Crypto (crypto.subtle) unavailable");
  }
  // `digest` wants a plain-`ArrayBuffer`-backed BufferSource. In the normal
  // case reinterpret the exact byte range without copying; a SharedArrayBuffer
  // view (never produced here) falls back to a copy.
  const data: BufferSource =
    bytes.buffer instanceof ArrayBuffer
      ? new Uint8Array(bytes.buffer, bytes.byteOffset, bytes.byteLength)
      : bytes.slice();
  const digest = await subtle.digest("SHA-256", data);
  const view = new Uint8Array(digest);
  let hex = "";
  for (let i = 0; i < view.length; i++) {
    hex += view[i].toString(16).padStart(2, "0");
  }
  return hex;
}

/**
 * The persistence surface the app codes against. One document slot plus a set
 * of immutable, hash-keyed blobs. `writeBlob` is idempotent: a blob whose hash
 * already exists is never rewritten (content-addressed ⇒ immutable).
 */
export interface AssetStore {
  /** The single persisted document JSON, or `null` when none is stored. */
  readDoc(): Promise<string | null>;
  /** Persist the document JSON, replacing any previous one. */
  writeDoc(json: string): Promise<void>;
  /** Bytes of a blob by hash, or `null` when it is not stored. */
  readBlob(hash: string): Promise<Uint8Array | null>;
  /** Store a blob. No-op when the hash already exists (blobs are immutable). */
  writeBlob(hash: string, bytes: Uint8Array): Promise<void>;
  /** Whether a blob with this hash is stored. */
  hasBlob(hash: string): Promise<boolean>;
  /** Every stored blob hash — the input to garbage collection. */
  listBlobs(): Promise<string[]>;
  /** Remove a blob (GC). No-op when absent. */
  deleteBlob(hash: string): Promise<void>;
}

const DB_NAME = "fielddraft-assets";
const DB_VERSION = 1;
const DOC_STORE = "doc";
const BLOB_STORE = "blobs";
/** Fixed single-slot key for the document (the IndexedDB analogue of the
 *  localStorage row). */
const DOC_KEY = "current";

function idbAvailable(): boolean {
  try {
    return typeof indexedDB !== "undefined" && indexedDB !== null;
  } catch {
    return false;
  }
}

/**
 * IndexedDB-backed {@link AssetStore}. Uses its own database (`fielddraft-
 * assets`) with two object stores: `doc` (a single out-of-line slot) and
 * `blobs` (out-of-line, keyed by hash). Where `indexedDB` is missing every
 * method degrades quietly — reads resolve to `null`/empty, writes are no-ops —
 * so the same code runs under SSR / vitest without special-casing.
 */
export class IndexedDbAssetStore implements AssetStore {
  /** Override the DB name (tests / isolated instances). */
  constructor(private readonly dbName: string = DB_NAME) {}

  private openDb(): Promise<IDBDatabase> {
    return new Promise((resolve, reject) => {
      const req = indexedDB.open(this.dbName, DB_VERSION);
      req.onupgradeneeded = () => {
        const db = req.result;
        if (!db.objectStoreNames.contains(DOC_STORE)) {
          db.createObjectStore(DOC_STORE);
        }
        if (!db.objectStoreNames.contains(BLOB_STORE)) {
          db.createObjectStore(BLOB_STORE);
        }
      };
      req.onsuccess = () => resolve(req.result);
      req.onerror = () => reject(req.error);
    });
  }

  /** Run one request inside a fresh transaction, closing the db after. */
  private run<T>(
    store: string,
    mode: IDBTransactionMode,
    fn: (s: IDBObjectStore) => IDBRequest<T>,
  ): Promise<T> {
    return this.openDb().then(
      (db) =>
        new Promise<T>((resolve, reject) => {
          const t = db.transaction(store, mode);
          const req = fn(t.objectStore(store));
          req.onsuccess = () => resolve(req.result);
          req.onerror = () => reject(req.error);
          t.oncomplete = () => db.close();
          t.onabort = () => db.close();
        }),
    );
  }

  async readDoc(): Promise<string | null> {
    if (!idbAvailable()) return null;
    try {
      const v = await this.run<string | undefined>(DOC_STORE, "readonly", (s) =>
        s.get(DOC_KEY),
      );
      return typeof v === "string" ? v : null;
    } catch {
      return null;
    }
  }

  async writeDoc(json: string): Promise<void> {
    if (!idbAvailable()) return;
    try {
      await this.run(DOC_STORE, "readwrite", (s) => s.put(json, DOC_KEY));
    } catch (err) {
      console.warn("fielddraft: asset doc write failed", err);
    }
  }

  async readBlob(hash: string): Promise<Uint8Array | null> {
    if (!idbAvailable()) return null;
    try {
      const v = await this.run<unknown>(BLOB_STORE, "readonly", (s) =>
        s.get(hash),
      );
      return v == null ? null : toU8(v);
    } catch {
      return null;
    }
  }

  async writeBlob(hash: string, bytes: Uint8Array): Promise<void> {
    if (!idbAvailable()) return;
    try {
      // Immutable: never rewrite an existing hash.
      if (await this.hasBlob(hash)) return;
      await this.run(BLOB_STORE, "readwrite", (s) => s.put(bytes, hash));
    } catch (err) {
      console.warn("fielddraft: asset blob write failed", err);
    }
  }

  async hasBlob(hash: string): Promise<boolean> {
    if (!idbAvailable()) return false;
    try {
      const key = await this.run<IDBValidKey | undefined>(
        BLOB_STORE,
        "readonly",
        (s) => s.getKey(hash),
      );
      return key !== undefined;
    } catch {
      return false;
    }
  }

  async listBlobs(): Promise<string[]> {
    if (!idbAvailable()) return [];
    try {
      const keys = await this.run<IDBValidKey[]>(BLOB_STORE, "readonly", (s) =>
        s.getAllKeys(),
      );
      return keys.map((k) => String(k));
    } catch {
      return [];
    }
  }

  async deleteBlob(hash: string): Promise<void> {
    if (!idbAvailable()) return;
    try {
      await this.run(BLOB_STORE, "readwrite", (s) => s.delete(hash));
    } catch (err) {
      console.warn("fielddraft: asset blob delete failed", err);
    }
  }
}

/** Normalize an IndexedDB-returned value (Uint8Array or ArrayBuffer, per
 *  structured clone) into a Uint8Array. */
function toU8(v: unknown): Uint8Array {
  if (v instanceof Uint8Array) return v;
  if (v instanceof ArrayBuffer) return new Uint8Array(v);
  // ArrayBufferView of another kind — reinterpret its byte range.
  if (ArrayBuffer.isView(v)) {
    return new Uint8Array(v.buffer, v.byteOffset, v.byteLength);
  }
  throw new Error("fielddraft: stored blob is not binary");
}
