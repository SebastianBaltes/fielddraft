/**
 * Ticket 4z37 — explicit "Open project" must survive a reload, even in a
 * deep-link repro tab.
 *
 * A `?project=` deep-link tab boots with an inert autosave (so it can't
 * clobber another tab's persisted project) and re-fetches the ticket on
 * reload. Opening a project from disk there is a deliberate "work on THIS"
 * action: `openUserProjectFromBytes` must persist it AND graduate the tab
 * (real autosave, `?project=` stripped) so a reload restores it.
 *
 * Reuses the FakeKernel seam from the other store tests to drive a real
 * KernelStore under jsdom.
 */

import { describe, expect, it, beforeEach } from "vitest";
import { KernelStore } from "./store";
import type { Kernel } from "./kernel";
import { createNoopAutoSave } from "./persistence";
import type { AssetStore } from "./assetStore";
import { loadProjectDoc, __setAssetStoreForTest } from "./projectPersistence";

/** In-memory AssetStore standing in for IndexedDB (jsdom has none). P1b moved
 *  the working store off localStorage, so this is where a graduated deep-link
 *  tab's autosave now lands (`persistProjectDoc`). */
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

/** Let the fire-and-forget async autosave sink settle. */
const tick = () => new Promise((r) => setTimeout(r, 0));

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

function projectBytes(name: string): Uint8Array {
  const doc =
    '{"schema_version":1,"name":"Part","feature_calls":[],"next_feature_id":1}';
  const project = {
    schemaVersion: 1,
    name,
    tabs: [{ id: "tab_1", kind: "part", name: "Part 1", documentJson: doc }],
    activeTabId: "tab_1",
  };
  return new TextEncoder().encode(JSON.stringify(project));
}

describe("KernelStore.openUserProjectFromBytes (ticket 4z37)", () => {
  let assets: FakeAssetStore;

  beforeEach(() => {
    localStorage.clear();
    assets = new FakeAssetStore();
    __setAssetStoreForTest(assets);
    window.history.replaceState(null, "", "/?project=tickets/open/x/project.fdraft");
  });

  it("persists an opened project and graduates the deep-link tab", async () => {
    // Deep-link repro tab: inert autosave, `?project=` in the URL.
    const store = new KernelStore(
      new FakeKernel() as unknown as Kernel,
      createNoopAutoSave(),
      null,
      true, // deepLink
    );

    // Before opening, nothing is persisted (inert autosave).
    expect(await loadProjectDoc()).toBeNull();

    const ok = await store.openUserProjectFromBytes(projectBytes("Opened4z37"));
    expect(ok).toBe(true);
    await tick(); // let the graduated (async, IndexedDB) autosave flush settle

    // The opened project is now in the working store — a reload would restore
    // it. (The only way this lands despite the inert BOOT handle is the
    // graduate step swapping in a real autosave, so this proves graduation
    // happened.)
    const persisted = await loadProjectDoc();
    expect(persisted).not.toBeNull();
    expect(JSON.parse(persisted!).name).toBe("Opened4z37");

    // The `?project=` deep-link is stripped so reload uses the working store.
    expect(new URLSearchParams(window.location.search).has("project")).toBe(false);

    // Graduated for good: a subsequent open still persists through the now-real
    // autosave (not the inert boot handle).
    assets.doc = null;
    await store.openUserProjectFromBytes(projectBytes("Edited4z37"));
    await tick();
    expect(JSON.parse((await loadProjectDoc())!).name).toBe("Edited4z37");
  });
});
