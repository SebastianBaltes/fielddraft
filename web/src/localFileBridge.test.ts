/**
 * Ticket 14661fc4 — local-file autosave mirror: bound handle receives the
 * exported ZIP bytes on each autosave write; a failing write unbinds.
 */

import { describe, expect, it, afterEach, vi } from "vitest";
import {
  bindLocalFile,
  clearLocalFileBinding,
  localFileName,
} from "./localFileBridge";
import { createAutoSave, type StorageLike } from "./persistence";
import type { FsaFileHandle } from "./localProjects";

function fakeStorage(): StorageLike {
  const map = new Map<string, string>();
  return {
    getItem: (k) => map.get(k) ?? null,
    setItem: (k, v) => void map.set(k, v),
    removeItem: (k) => void map.delete(k),
  };
}

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

const flush = () => new Promise((r) => setTimeout(r, 0));

afterEach(() => {
  clearLocalFileBinding();
  vi.useRealTimers();
});

describe("localFileBridge", () => {
  it("mirrors autosave writes to the bound handle", async () => {
    const { handle, writes } = fakeHandle();
    const bytes = new Uint8Array([1, 2, 3]);
    bindLocalFile(handle, () => bytes);
    expect(localFileName()).toBe("part.fdraft");

    const autosave = createAutoSave({
      storage: fakeStorage(),
      debounceMs: 0,
    });
    autosave.schedule("{}");
    autosave.flush();
    await flush();
    await flush();

    expect(writes).toHaveLength(1);
    expect(new Uint8Array(writes[0])).toEqual(bytes);
  });

  it("a failing write unbinds instead of looping", async () => {
    const { handle } = fakeHandle({ failWrite: true });
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    bindLocalFile(handle, () => new Uint8Array([9]));

    const autosave = createAutoSave({
      storage: fakeStorage(),
      debounceMs: 0,
    });
    autosave.schedule("{}");
    autosave.flush();
    await flush();
    await flush();

    expect(localFileName()).toBeNull();
    expect(warn).toHaveBeenCalled();
  });

  it("no binding → autosave writes stay storage-only", async () => {
    const storage = fakeStorage();
    const autosave = createAutoSave({ storage, debounceMs: 0 });
    autosave.schedule("{}");
    autosave.flush();
    await flush();
    expect(storage.getItem("fielddraft.project.v2")).toBe("{}");
  });
});
