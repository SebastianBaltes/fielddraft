/**
 * Phase 4 — explicit Save routing.
 *
 * `saveProjectToFile` writes straight into a bound local `.fdraft` (and clears
 * the local dirty flag) when a File System Access handle is bound; otherwise it
 * falls back to a browser download. Reuses the FakeKernel seam from the other
 * store tests to drive a real KernelStore under jsdom.
 */

import { describe, expect, it, beforeEach, afterEach, vi } from "vitest";
import { KernelStore } from "./store";
import type { Kernel } from "./kernel";
import { createNoopAutoSave } from "./persistence";
import { __setAssetStoreForTest } from "./projectPersistence";
import type { AssetStore } from "./assetStore";
import {
  bindLocalFile,
  clearLocalFileBinding,
  isLocalDirty,
  markLocalDirty,
} from "./localFileBridge";
import type { FsaFileHandle } from "./localProjects";

class FakeAssetStore implements AssetStore {
  doc: string | null = null;
  blobs = new Map<string, Uint8Array>();
  async readDoc() {
    return this.doc;
  }
  async writeDoc(json: string) {
    this.doc = json;
  }
  async readBlob(hash: string) {
    return this.blobs.get(hash) ?? null;
  }
  async writeBlob(hash: string, bytes: Uint8Array) {
    if (!this.blobs.has(hash)) this.blobs.set(hash, bytes);
  }
  async hasBlob(hash: string) {
    return this.blobs.has(hash);
  }
  async listBlobs() {
    return [...this.blobs.keys()];
  }
  async deleteBlob(hash: string) {
    this.blobs.delete(hash);
  }
}

class FakeKernel {
  loadedJson =
    '{"schema_version":1,"name":"Part","feature_calls":[],"next_feature_id":1}';
  listScripts() {
    return [];
  }
  upsertScript() {
    return false;
  }
  removeScript() {
    return false;
  }
  loadJson(json: string) {
    this.loadedJson = json;
  }
  listFeatures() {
    return [];
  }
  listVariables() {
    return [];
  }
  canUndo() {
    return false;
  }
  undoDepth() {
    return 0;
  }
  canRedo() {
    return false;
  }
  redoDepth() {
    return 0;
  }
  getRollbackIndex() {
    return -1;
  }
  toJson() {
    return this.loadedJson;
  }
  sceneAabb() {
    return { size: [0, 0, 0] };
  }
  setSyncMeshing() {
    return false;
  }
  activeBodyId() {
    return 0;
  }
  clearActiveBodyId() {}
  setActiveBodyByFeature() {}
  bodyCount() {
    return 0;
  }
}

function fakeHandle() {
  const writes: ArrayBuffer[] = [];
  const handle = {
    kind: "file",
    name: "part.fdraft",
    getFile: async () => new File([], "part.fdraft"),
    createWritable: async () => ({
      write: async (data: ArrayBuffer) => void writes.push(data),
      close: async () => {},
    }),
    queryPermission: async () => "granted",
    requestPermission: async () => "granted",
    isSameEntry: async () => false,
  } as unknown as FsaFileHandle;
  return { handle, writes };
}

function makeStore() {
  __setAssetStoreForTest(new FakeAssetStore());
  return new KernelStore(new FakeKernel() as unknown as Kernel, createNoopAutoSave());
}

describe("KernelStore.saveProjectToFile (Phase 4)", () => {
  beforeEach(() => {
    localStorage.clear();
    clearLocalFileBinding();
  });
  afterEach(() => {
    clearLocalFileBinding();
    vi.restoreAllMocks();
  });

  it("writes into the bound local file and clears the dirty flag", async () => {
    const store = makeStore();
    const { handle, writes } = fakeHandle();
    bindLocalFile(handle);
    markLocalDirty();
    expect(isLocalDirty()).toBe(true);

    await store.saveProjectToFile();

    expect(writes).toHaveLength(1);
    expect(new Uint8Array(writes[0]).byteLength).toBeGreaterThan(0);
    expect(isLocalDirty()).toBe(false);
  });

  it("falls back to a download when no local file is bound", async () => {
    const store = makeStore();
    const createUrl = vi.fn(() => "blob:fake");
    const revokeUrl = vi.fn();
    (globalThis.URL as unknown as { createObjectURL: unknown }).createObjectURL =
      createUrl;
    (globalThis.URL as unknown as { revokeObjectURL: unknown }).revokeObjectURL =
      revokeUrl;
    const click = vi
      .spyOn(HTMLAnchorElement.prototype, "click")
      .mockImplementation(() => {});

    await store.saveProjectToFile("out.fdraft");

    expect(createUrl).toHaveBeenCalledTimes(1);
    expect(click).toHaveBeenCalledTimes(1);
    expect(revokeUrl).toHaveBeenCalledTimes(1);
  });

  it("falls back to a download when the handle write fails", async () => {
    const store = makeStore();
    vi.spyOn(console, "warn").mockImplementation(() => {});
    const failing = {
      kind: "file",
      name: "part.fdraft",
      getFile: async () => new File([], "part.fdraft"),
      createWritable: async () => {
        throw new Error("NotAllowedError");
      },
      isSameEntry: async () => false,
    } as unknown as FsaFileHandle;
    bindLocalFile(failing);
    const createUrl = vi.fn(() => "blob:fake");
    (globalThis.URL as unknown as { createObjectURL: unknown }).createObjectURL =
      createUrl;
    (globalThis.URL as unknown as { revokeObjectURL: unknown }).revokeObjectURL =
      vi.fn();
    vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => {});

    await store.saveProjectToFile("out.fdraft");

    // Handle write threw → download path taken instead.
    expect(createUrl).toHaveBeenCalledTimes(1);
  });
});
