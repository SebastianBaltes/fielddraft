// Phase 1b: the async persistence sink + load boundary. Uses an in-memory fake
// AssetStore (jsdom ships no IndexedDB) to verify that a serialized Project's
// embedded mesh is externalized into a content-addressed blob, that the stored
// document is lean, that load re-inlines it (round-trip), and the one-way
// localStorage → store migration.
import { describe, expect, it, beforeEach } from "vitest";
import type { AssetStore } from "./assetStore";
import {
  persistProjectDoc,
  loadProjectDoc,
  migrateFromLocalStorage,
  __setAssetStoreForTest,
} from "./projectPersistence";
import { STORAGE_KEY } from "./persistence";

/** In-memory {@link AssetStore} over plain Maps — the sink/boundary under test
 *  is storage-agnostic, so this stands in for IndexedDB. */
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
    if (!this.blobs.has(hash)) this.blobs.set(hash, bytes); // immutable
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

/** A serialized Project blob whose single part tab embeds an `imported_mesh`.
 *  Mirrors production: `PartTab.documentJson` is a nested JSON *string*, so the
 *  `__positions`/`__indices` live INSIDE that string (all f32-exact so the
 *  round-trip is lossless). */
function projectWithMesh(): string {
  const documentJson = JSON.stringify({
    schema_version: 1,
    name: "Part",
    feature_calls: [
      {
        type: "imported_mesh",
        id: "1",
        params: {
          op: "New",
          __positions: [0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1],
          __indices: [0, 1, 2, 0, 1, 3],
          __watertight: true,
        },
      },
    ],
    next_feature_id: 2,
  });
  return JSON.stringify({
    schemaVersion: 1,
    name: "Untitled",
    tabs: [{ id: "tab_1", kind: "part", name: "Part 1", documentJson }],
    activeTabId: "tab_1",
  });
}

/** Parse a Project and its part tabs' inner docs so a deep comparison is
 *  key-order-independent (the externalize→inline round-trip re-appends payload
 *  keys, so the inner string is equal only after re-parsing). */
function normalized(projectJson: string): unknown {
  const p = JSON.parse(projectJson) as { tabs: { kind: string; documentJson?: string }[] };
  for (const tab of p.tabs) {
    if (tab.kind === "part" && typeof tab.documentJson === "string") {
      (tab as { documentJson: unknown }).documentJson = JSON.parse(tab.documentJson);
    }
  }
  return p;
}

describe("projectPersistence", () => {
  let fake: FakeAssetStore;

  beforeEach(() => {
    fake = new FakeAssetStore();
    __setAssetStoreForTest(fake);
    try {
      localStorage.clear();
    } catch {
      /* jsdom without localStorage — ignore */
    }
  });

  it("externalizes a part tab's embedded mesh and stores a lean document", async () => {
    await persistProjectDoc(projectWithMesh());

    expect(fake.doc).not.toBeNull();
    // The stored document no longer carries the raw payload anywhere…
    expect(fake.doc!).not.toContain("__positions");
    expect(fake.doc!).not.toContain("__indices");
    expect(fake.doc!).toContain("__asset");
    // …and exactly one blob was written.
    expect(fake.blobs.size).toBe(1);

    // The inner (nested) documentJson references the asset by hash; sibling
    // metadata survives.
    const inner = JSON.parse(JSON.parse(fake.doc!).tabs[0].documentJson);
    const params = inner.feature_calls[0].params;
    expect(params.__asset).toMatch(/^[0-9a-f]{64}$/);
    expect(params.__assetKind).toBe("mesh");
    expect(params.__positions).toBeUndefined();
    expect(params.op).toBe("New");
    expect(params.__watertight).toBe(true);
    expect(fake.blobs.has(params.__asset)).toBe(true);
  });

  it("round-trips the full project through persist → load", async () => {
    const original = projectWithMesh();
    await persistProjectDoc(original);

    const loaded = await loadProjectDoc();
    expect(loaded).not.toBeNull();
    // The re-inlined document carries the mesh again…
    expect(loaded!).toContain("__positions");
    expect(loaded!).not.toContain("__asset");
    // …and structurally matches the original (key order aside).
    expect(normalized(loaded!)).toEqual(normalized(original));
  });

  it("returns null from load when nothing is stored", async () => {
    expect(await loadProjectDoc()).toBeNull();
  });

  it("leaves a mesh-free project untouched (no blobs)", async () => {
    const doc = JSON.stringify({
      schemaVersion: 1,
      name: "Untitled",
      tabs: [
        {
          id: "tab_1",
          kind: "part",
          name: "Part 1",
          documentJson: JSON.stringify({
            schema_version: 1,
            name: "Part",
            feature_calls: [{ type: "box", id: "1", params: { size: 5 } }],
            next_feature_id: 2,
          }),
        },
      ],
      activeTabId: "tab_1",
    });
    await persistProjectDoc(doc);
    expect(fake.blobs.size).toBe(0);
    expect(normalized(fake.doc!)).toEqual(normalized(doc));
  });

  describe("migrateFromLocalStorage", () => {
    it("moves the legacy localStorage row into the store and clears it", async () => {
      const legacy = projectWithMesh();
      localStorage.setItem(STORAGE_KEY, legacy);

      await migrateFromLocalStorage();

      // localStorage row consumed; store now holds the externalized document.
      expect(localStorage.getItem(STORAGE_KEY)).toBeNull();
      expect(fake.doc).not.toBeNull();
      expect(fake.doc!).not.toContain("__positions");
      expect(fake.blobs.size).toBe(1);

      // Round-trips back to the original on load.
      const loaded = await loadProjectDoc();
      expect(normalized(loaded!)).toEqual(normalized(legacy));
    });

    it("is idempotent / a no-op with no legacy row", async () => {
      await migrateFromLocalStorage();
      expect(fake.doc).toBeNull();
      expect(fake.blobs.size).toBe(0);
    });

    it("never overwrites an existing store document, but drops the stale row", async () => {
      fake.doc = '{"already":"migrated"}';
      localStorage.setItem(STORAGE_KEY, projectWithMesh());

      await migrateFromLocalStorage();

      expect(fake.doc).toBe('{"already":"migrated"}');
      expect(localStorage.getItem(STORAGE_KEY)).toBeNull();
    });
  });
});
