import { describe, expect, it, vi, beforeEach } from "vitest";
import {
  clearPersisted,
  createAutoSave,
  loadPersisted,
  STORAGE_KEY,
  type StorageLike,
} from "./persistence";

function fakeStorage(): StorageLike & { map: Map<string, string> } {
  const map = new Map<string, string>();
  return {
    map,
    getItem: (k) => map.get(k) ?? null,
    setItem: (k, v) => {
      map.set(k, v);
    },
    removeItem: (k) => {
      map.delete(k);
    },
  };
}

describe("persistence", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  it("writes after the debounce window elapses", () => {
    const storage = fakeStorage();
    const autosave = createAutoSave({ storage, debounceMs: 500 });

    autosave.schedule('{"a":1}');
    expect(storage.map.has(STORAGE_KEY)).toBe(false);

    vi.advanceTimersByTime(499);
    expect(storage.map.has(STORAGE_KEY)).toBe(false);

    vi.advanceTimersByTime(1);
    expect(storage.map.get(STORAGE_KEY)).toBe('{"a":1}');
  });

  it("coalesces bursts into a single write with the last value", () => {
    const storage = fakeStorage();
    const autosave = createAutoSave({ storage, debounceMs: 100 });

    autosave.schedule('{"v":1}');
    vi.advanceTimersByTime(50);
    autosave.schedule('{"v":2}');
    vi.advanceTimersByTime(50);
    autosave.schedule('{"v":3}');
    vi.advanceTimersByTime(99);
    expect(storage.map.has(STORAGE_KEY)).toBe(false);
    vi.advanceTimersByTime(1);

    expect(storage.map.get(STORAGE_KEY)).toBe('{"v":3}');
  });

  it("flush() writes immediately and cancels the timer", () => {
    const storage = fakeStorage();
    const autosave = createAutoSave({ storage, debounceMs: 1000 });

    autosave.schedule('{"k":"v"}');
    autosave.flush();
    expect(storage.map.get(STORAGE_KEY)).toBe('{"k":"v"}');

    // A second flush without new data is a no-op, not a duplicate write.
    storage.map.clear();
    autosave.flush();
    expect(storage.map.size).toBe(0);
  });

  it("cancel() drops a pending save", () => {
    const storage = fakeStorage();
    const autosave = createAutoSave({ storage, debounceMs: 500 });

    autosave.schedule('{"keep":false}');
    autosave.cancel();
    vi.advanceTimersByTime(10_000);

    expect(storage.map.size).toBe(0);
  });

  it("loadPersisted round-trips with the schedule path", () => {
    const storage = fakeStorage();
    const autosave = createAutoSave({ storage, debounceMs: 0 });

    autosave.schedule('{"schema_version":1,"name":"x","primitives":[]}');
    vi.advanceTimersByTime(0);

    expect(loadPersisted(storage)).toBe('{"schema_version":1,"name":"x","primitives":[]}');
  });

  it("clearPersisted removes the stored value", () => {
    const storage = fakeStorage();
    storage.setItem(STORAGE_KEY, "stale");
    clearPersisted(storage);
    expect(storage.getItem(STORAGE_KEY)).toBeNull();
  });

  it("gracefully handles a throwing storage backend", () => {
    const throwing: StorageLike = {
      getItem: () => {
        throw new Error("denied");
      },
      setItem: () => {
        throw new Error("denied");
      },
      removeItem: () => {
        throw new Error("denied");
      },
    };
    // The autosave path logs a warning on write failure; silence it here
    // so the expected error doesn't pollute the test output.
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    try {
      const autosave = createAutoSave({ storage: throwing, debounceMs: 0 });
      expect(() => autosave.schedule("{}")).not.toThrow();
      vi.advanceTimersByTime(0);
      expect(() => loadPersisted(throwing)).not.toThrow();
      expect(loadPersisted(throwing)).toBeNull();
      expect(warn).toHaveBeenCalled();
    } finally {
      warn.mockRestore();
    }
  });
  it("resolves a lazy producer once, at write time", () => {
    const storage = fakeStorage();
    let calls = 0;
    let tick: () => void = () => {};
    const autosave = createAutoSave({
      storage,
      debounceMs: 100,
      setTimeoutImpl: ((fn: () => void) => {
        tick = fn;
        return 1 as unknown as ReturnType<typeof setTimeout>;
      }) as typeof setTimeout,
      clearTimeoutImpl: (() => {}) as typeof clearTimeout,
    });
    const producer = () => {
      calls++;
      return `state-${calls}`;
    };
    autosave.schedule(producer);
    autosave.schedule(producer);
    autosave.schedule(producer);
    expect(calls).toBe(0); // never serialized during the burst
    tick();
    expect(calls).toBe(1); // exactly once, at the debounced write
    expect(storage.map.get(STORAGE_KEY)).toBe("state-1");
  });

  it("re-reads a debounce getter on every schedule (live setting change)", () => {
    const storage = fakeStorage();
    const delays: number[] = [];
    let debounce = 500;
    const autosave = createAutoSave({
      storage,
      debounceMs: () => debounce,
      setTimeoutImpl: ((_fn: () => void, ms?: number) => {
        delays.push(ms ?? 0);
        return 1 as unknown as ReturnType<typeof setTimeout>;
      }) as typeof setTimeout,
      clearTimeoutImpl: (() => {}) as typeof clearTimeout,
    });
    autosave.schedule("a");
    debounce = 5000;
    autosave.schedule("b");
    expect(delays).toEqual([500, 5000]);
  });
});
