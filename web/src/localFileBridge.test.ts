/**
 * Phase 4 — local-file bridge as an explicit Save target + dirty tracker.
 * No continuous mirror any more: `writeBoundFile` writes on demand and
 * `markLocalDirty`/`setLocalClean` track divergence from the file on disk.
 */

import { describe, expect, it, afterEach, vi } from "vitest";
import {
  bindLocalFile,
  clearLocalFileBinding,
  getLocalFileState,
  isLocalDirty,
  isLocalFileBound,
  localFileName,
  markLocalDirty,
  setLocalClean,
  subscribeLocalFile,
  writeBoundFile,
} from "./localFileBridge";
import type { FsaFileHandle } from "./localProjects";

function fakeHandle(opts: { failWrite?: boolean } = {}) {
  const writes: ArrayBuffer[] = [];
  const handle = {
    kind: "file",
    name: "part.fdraft",
    getFile: async () => new File([], "part.fdraft"),
    createWritable: async () => ({
      write: async (data: ArrayBuffer) => {
        if (opts.failWrite) throw new Error("NotAllowedError");
        writes.push(data);
      },
      close: async () => {},
    }),
    queryPermission: async () => "granted",
    requestPermission: async () => "granted",
    isSameEntry: async () => false,
  } as unknown as FsaFileHandle;
  return { handle, writes };
}

afterEach(() => {
  clearLocalFileBinding();
});

describe("localFileBridge", () => {
  it("binds a file as the save target, starting clean", () => {
    const { handle } = fakeHandle();
    expect(isLocalFileBound()).toBe(false);
    bindLocalFile(handle);
    expect(isLocalFileBound()).toBe(true);
    expect(localFileName()).toBe("part.fdraft");
    expect(isLocalDirty()).toBe(false);
    expect(getLocalFileState()).toEqual({ name: "part.fdraft", dirty: false });
  });

  it("marks dirty only while a file is bound, and notifies subscribers", () => {
    const listener = vi.fn();
    const unsub = subscribeLocalFile(listener);

    // No binding → mark is a no-op.
    markLocalDirty();
    expect(isLocalDirty()).toBe(false);

    const { handle } = fakeHandle();
    bindLocalFile(handle);
    listener.mockClear();

    markLocalDirty();
    expect(isLocalDirty()).toBe(true);
    expect(getLocalFileState().dirty).toBe(true);
    expect(listener).toHaveBeenCalledTimes(1);

    // A second mark while already dirty must not re-notify.
    markLocalDirty();
    expect(listener).toHaveBeenCalledTimes(1);

    setLocalClean();
    expect(isLocalDirty()).toBe(false);
    expect(listener).toHaveBeenCalledTimes(2);

    unsub();
  });

  it("returns a referentially-stable snapshot between changes", () => {
    const { handle } = fakeHandle();
    bindLocalFile(handle);
    const a = getLocalFileState();
    expect(getLocalFileState()).toBe(a);
    markLocalDirty();
    expect(getLocalFileState()).not.toBe(a);
  });

  it("writes the bytes into the bound handle", async () => {
    const { handle, writes } = fakeHandle();
    bindLocalFile(handle);
    const bytes = new Uint8Array([1, 2, 3]);
    const ok = await writeBoundFile(bytes);
    expect(ok).toBe(true);
    expect(writes).toHaveLength(1);
    expect(new Uint8Array(writes[0])).toEqual(bytes);
  });

  it("reports a failed write without throwing", async () => {
    const { handle } = fakeHandle({ failWrite: true });
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    bindLocalFile(handle);
    const ok = await writeBoundFile(new Uint8Array([9]));
    expect(ok).toBe(false);
    expect(warn).toHaveBeenCalled();
    warn.mockRestore();
  });

  it("writeBoundFile is a no-op when nothing is bound", async () => {
    expect(await writeBoundFile(new Uint8Array([1]))).toBe(false);
  });

  it("clearing the binding resets name and dirty", () => {
    const { handle } = fakeHandle();
    bindLocalFile(handle);
    markLocalDirty();
    expect(isLocalDirty()).toBe(true);
    clearLocalFileBinding();
    expect(isLocalFileBound()).toBe(false);
    expect(localFileName()).toBeNull();
    expect(isLocalDirty()).toBe(false);
    expect(getLocalFileState()).toEqual({ name: null, dirty: false });
  });
});
