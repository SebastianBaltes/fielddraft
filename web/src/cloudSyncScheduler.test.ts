import { describe, expect, it, vi } from "vitest";
import {
  cloudSyncIfDirty,
  shouldCloudSync,
  startCloudSyncTimer,
  type CloudSyncDeps,
  type CloudSyncState,
} from "./cloudSyncScheduler";

/** Build injectable deps with a spy `sync`. */
function makeDeps(
  syncState: CloudSyncState,
  projectId: string | null,
): CloudSyncDeps & { sync: ReturnType<typeof vi.fn> } {
  const sync = vi.fn();
  return {
    getSyncState: () => syncState,
    getProjectId: () => projectId,
    sync,
  };
}

describe("shouldCloudSync", () => {
  it("is true only when dirty AND a project is bound", () => {
    expect(shouldCloudSync("dirty", "p1")).toBe(true);
  });

  it("is false when clean/syncing/error/offline regardless of binding", () => {
    for (const s of ["clean", "syncing", "error", "offline"] as const) {
      expect(shouldCloudSync(s, "p1")).toBe(false);
    }
  });

  it("is false when no project is bound, even if dirty", () => {
    expect(shouldCloudSync("dirty", null)).toBe(false);
  });
});

describe("cloudSyncIfDirty (exit / beforeunload trigger)", () => {
  it("syncs when dirty + bound and reports it fired", () => {
    const deps = makeDeps("dirty", "p1");
    expect(cloudSyncIfDirty(deps)).toBe(true);
    expect(deps.sync).toHaveBeenCalledOnce();
  });

  it("does NOT sync when clean", () => {
    const deps = makeDeps("clean", "p1");
    expect(cloudSyncIfDirty(deps)).toBe(false);
    expect(deps.sync).not.toHaveBeenCalled();
  });

  it("does NOT sync when unbound (guest), even if dirty", () => {
    const deps = makeDeps("dirty", null);
    expect(cloudSyncIfDirty(deps)).toBe(false);
    expect(deps.sync).not.toHaveBeenCalled();
  });
});

describe("startCloudSyncTimer (periodic trigger)", () => {
  it("interval 0 → no timer is started, stop is a safe no-op", () => {
    const setIntervalImpl = vi.fn();
    const clearIntervalImpl = vi.fn();
    const deps = makeDeps("dirty", "p1");
    const stop = startCloudSyncTimer({
      ...deps,
      intervalMs: 0,
      setIntervalImpl: setIntervalImpl as unknown as typeof setInterval,
      clearIntervalImpl: clearIntervalImpl as unknown as typeof clearInterval,
    });
    expect(setIntervalImpl).not.toHaveBeenCalled();
    stop(); // must not throw / must not clear a phantom handle
    expect(clearIntervalImpl).not.toHaveBeenCalled();
    expect(deps.sync).not.toHaveBeenCalled();
  });

  it("fires the tick only when dirty", () => {
    let tick: (() => void) | null = null;
    const setIntervalImpl = vi.fn((fn: () => void) => {
      tick = fn;
      return 1 as unknown as ReturnType<typeof setInterval>;
    });
    const deps = makeDeps("dirty", "p1");
    startCloudSyncTimer({
      ...deps,
      intervalMs: 60_000,
      setIntervalImpl: setIntervalImpl as unknown as typeof setInterval,
    });
    expect(setIntervalImpl).toHaveBeenCalledWith(expect.any(Function), 60_000);
    tick!();
    expect(deps.sync).toHaveBeenCalledOnce();
  });

  it("clean tick → no-op (dirty gate)", () => {
    let tick: (() => void) | null = null;
    const setIntervalImpl = vi.fn((fn: () => void) => {
      tick = fn;
      return 1 as unknown as ReturnType<typeof setInterval>;
    });
    const deps = makeDeps("clean", "p1");
    startCloudSyncTimer({
      ...deps,
      intervalMs: 60_000,
      setIntervalImpl: setIntervalImpl as unknown as typeof setInterval,
    });
    tick!();
    expect(deps.sync).not.toHaveBeenCalled();
  });

  it("re-reads live state each tick (dirty→clean stops syncing)", () => {
    let tick: (() => void) | null = null;
    const setIntervalImpl = vi.fn((fn: () => void) => {
      tick = fn;
      return 1 as unknown as ReturnType<typeof setInterval>;
    });
    let syncState: CloudSyncState = "dirty";
    const sync = vi.fn();
    startCloudSyncTimer({
      intervalMs: 1000,
      getSyncState: () => syncState,
      getProjectId: () => "p1",
      sync,
      setIntervalImpl: setIntervalImpl as unknown as typeof setInterval,
    });
    tick!();
    expect(sync).toHaveBeenCalledTimes(1);
    syncState = "clean"; // a successful sync flipped it
    tick!();
    expect(sync).toHaveBeenCalledTimes(1); // no second call
  });

  it("stop() clears the timer handle", () => {
    const clearIntervalImpl = vi.fn();
    const deps = makeDeps("dirty", "p1");
    const stop = startCloudSyncTimer({
      ...deps,
      intervalMs: 1000,
      setIntervalImpl: (() => 42 as unknown as ReturnType<typeof setInterval>) as unknown as typeof setInterval,
      clearIntervalImpl: clearIntervalImpl as unknown as typeof clearInterval,
    });
    stop();
    expect(clearIntervalImpl).toHaveBeenCalledWith(42);
  });
});
