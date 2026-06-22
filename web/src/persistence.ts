/**
 * Auto-save to `localStorage` with debouncing.
 *
 * The MVP persists the entire project document as a single JSON string under
 * a stable key. A short debounce coalesces bursts of edits (dragging a
 * slider, typing in a field) into a single write. Reads happen exactly once
 * at startup; everything else is in-memory.
 */

export const STORAGE_KEY = "fielddraft.project.v2";
export const DEFAULT_DEBOUNCE_MS = 500;

export interface StorageLike {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}

export interface AutoSaveHandle {
  /** Schedule a save; coalesces with any pending save on the same handle. */
  schedule(json: string): void;
  /** Flush a pending save immediately. No-op if nothing is pending. */
  flush(): void;
  /** Cancel any pending save without writing. */
  cancel(): void;
}

export interface CreateAutoSaveOptions {
  storage?: StorageLike;
  key?: string;
  debounceMs?: number;
  /** Clock override, primarily for tests. */
  now?: () => number;
  /** Scheduler override, primarily for tests. */
  setTimeoutImpl?: typeof setTimeout;
  clearTimeoutImpl?: typeof clearTimeout;
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
  const debounceMs = options.debounceMs ?? DEFAULT_DEBOUNCE_MS;
  const setTimeoutFn = options.setTimeoutImpl ?? setTimeout;
  const clearTimeoutFn = options.clearTimeoutImpl ?? clearTimeout;

  let pendingJson: string | null = null;
  let timer: ReturnType<typeof setTimeout> | null = null;

  const writeNow = () => {
    if (pendingJson === null) return;
    const json = pendingJson;
    pendingJson = null;
    if (!storage) return;
    try {
      storage.setItem(key, json);
    } catch (err) {
      // Quota exceeded, private mode, disk full — swallow but surface on the
      // console so the UI can pick it up from a hook later if needed.
      console.warn("fielddraft: auto-save failed", err);
    }
  };

  return {
    schedule(json: string) {
      pendingJson = json;
      if (timer !== null) clearTimeoutFn(timer);
      timer = setTimeoutFn(() => {
        timer = null;
        writeNow();
      }, debounceMs);
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
