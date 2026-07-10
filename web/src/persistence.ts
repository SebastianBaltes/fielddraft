/**
 * Debounced auto-save.
 *
 * A short debounce coalesces bursts of edits (dragging a slider, typing in a
 * field) into a single write. The persisted payload is the whole serialized
 * Project (one JSON string). The write itself goes to a pluggable {@link
 * AutoSaveSink}: since P1b the real sink is `persistProjectDoc` (async,
 * IndexedDB — see projectPersistence.ts), which this module does NOT import so
 * it stays a dependency-free leaf; the sink is injected by the store. Reads
 * happen exactly once at startup; everything else is in-memory.
 *
 * `STORAGE_KEY` / `loadPersisted` / `clearPersisted` remain the legacy
 * localStorage helpers — still used by the one-way P1b migration
 * (`migrateFromLocalStorage`) and as the default sink for tests.
 */

export const STORAGE_KEY = "fielddraft.project.v2";
export const DEFAULT_DEBOUNCE_MS = 500;

export interface StorageLike {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}

/** Where a debounced autosave write lands. Sync (localStorage / a test fake)
 *  or async (IndexedDB via `persistProjectDoc`). A rejected async write is
 *  swallowed to the console — a reload reads back from the store, so the loss
 *  window is at most one debounce. */
export type AutoSaveSink = (json: string) => void | Promise<void>;

export interface AutoSaveHandle {
  /** Schedule a save; coalesces with any pending save on the same handle.
   *  A function is resolved lazily at WRITE time — dirty markers that fire
   *  often (camera drags mark once per changed frame) pass a producer so the
   *  project is serialized once per actual write, not once per marker. */
  schedule(json: string | (() => string)): void;
  /** Flush a pending save immediately. No-op if nothing is pending. */
  flush(): void;
  /** Cancel any pending save without writing. */
  cancel(): void;
}

export interface CreateAutoSaveOptions {
  /** Explicit write sink. Overrides `storage`. Production passes
   *  `persistProjectDoc` (async, IndexedDB); tests inject a capturing fake. */
  sink?: AutoSaveSink;
  storage?: StorageLike;
  key?: string;
  /** Debounce; a getter is re-read on every schedule so a live settings
   *  change (autosaveDebounceSec) takes effect without recreating the handle. */
  debounceMs?: number | (() => number);
  /** Clock override, primarily for tests. */
  now?: () => number;
  /** Scheduler override, primarily for tests. */
  setTimeoutImpl?: typeof setTimeout;
  clearTimeoutImpl?: typeof clearTimeout;
}

/** Ticket 14661fc4: a secondary autosave sink. While a LOCAL project file
 *  is bound (localFileBridge), every debounced autosave write also triggers
 *  a mirror write of the `.fdraft` bytes back to disk. Module-scoped so the
 *  bridge can attach without threading through every AutoSaveHandle. */
let autosaveMirror: (() => void) | null = null;

export function setAutosaveMirror(fn: (() => void) | null): void {
  autosaveMirror = fn;
}

function safeStorage(): StorageLike | null {
  try {
    if (typeof globalThis.localStorage === "undefined") return null;
    return globalThis.localStorage;
  } catch {
    return null;
  }
}

/** Load a previously-persisted document JSON, or `null` if none exists. */
export function loadPersisted(
  storage: StorageLike | null = safeStorage(),
  key: string = STORAGE_KEY,
): string | null {
  if (!storage) return null;
  try {
    return storage.getItem(key);
  } catch {
    return null;
  }
}

/** Erase any persisted document. */
export function clearPersisted(
  storage: StorageLike | null = safeStorage(),
  key: string = STORAGE_KEY,
): void {
  if (!storage) return;
  try {
    storage.removeItem(key);
  } catch {
    // Ignore: storage may be full or disabled (private mode, quota).
  }
}

/**
 * Create a debounced auto-save handle.
 *
 * Typical wiring:
 *
 *     const autosave = createAutoSave();
 *     store.subscribe(() => autosave.schedule(store.toJson()));
 *     window.addEventListener("beforeunload", () => autosave.flush());
 */
export function createAutoSave(options: CreateAutoSaveOptions = {}): AutoSaveHandle {
  const storage = options.storage ?? safeStorage();
  const key = options.key ?? STORAGE_KEY;
  const debounce = options.debounceMs ?? DEFAULT_DEBOUNCE_MS;
  const debounceNow = () => (typeof debounce === "function" ? debounce() : debounce);
  const setTimeoutFn = options.setTimeoutImpl ?? setTimeout;
  const clearTimeoutFn = options.clearTimeoutImpl ?? clearTimeout;
  // Resolve the write sink once: an explicit sink (production: persistProjectDoc)
  // wins; otherwise fall back to the legacy synchronous localStorage write
  // (the default sink used by the persistence tests via `storage`).
  const sink: AutoSaveSink =
    options.sink ??
    ((json: string) => {
      if (!storage) return;
      storage.setItem(key, json);
    });

  let pendingJson: string | (() => string) | null = null;
  let timer: ReturnType<typeof setTimeout> | null = null;

  const writeNow = () => {
    if (pendingJson === null) return;
    const pending = pendingJson;
    pendingJson = null;
    let result: void | Promise<void>;
    try {
      const json = typeof pending === "function" ? pending() : pending;
      result = sink(json);
    } catch (err) {
      // Sync sink failed (quota exceeded, private mode, disk full) — swallow
      // but surface on the console so a hook can pick it up later.
      console.warn("fielddraft: auto-save failed", err);
      return;
    }
    if (result && typeof (result as Promise<void>).then === "function") {
      // Async sink (IndexedDB): fire-and-forget. Mirror after it lands; a
      // rejection is logged, never thrown (the reload reads back from the
      // store, so the loss window is at most one debounce).
      (result as Promise<void>).then(
        () => autosaveMirror?.(),
        (err) => console.warn("fielddraft: auto-save failed", err),
      );
    } else {
      autosaveMirror?.();
    }
  };

  return {
    schedule(json: string | (() => string)) {
      pendingJson = json;
      if (timer !== null) clearTimeoutFn(timer);
      timer = setTimeoutFn(() => {
        timer = null;
        writeNow();
      }, debounceNow());
    },
    flush() {
      if (timer !== null) {
        clearTimeoutFn(timer);
        timer = null;
      }
      writeNow();
    },
    cancel() {
      if (timer !== null) {
        clearTimeoutFn(timer);
        timer = null;
      }
      pendingJson = null;
    },
  };
}

/**
 * An inert auto-save handle — every method is a no-op. Used by the ClaudeDraft
 * "open ticket" deep-link tab (`?project=`), which must never write to the
 * origin-wide persisted project: that localStorage slot is shared across all
 * tabs, so a repro tab autosaving the ticket would clobber the user's real
 * work-in-progress in their other tabs.
 */
export function createNoopAutoSave(): AutoSaveHandle {
  return {
    schedule() {},
    flush() {},
    cancel() {},
  };
}
