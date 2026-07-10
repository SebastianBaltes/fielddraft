// Phase 1b: the async persistence sink + load boundary. Uses an in-memory fake
// AssetStore (jsdom ships no IndexedDB) to verify that a serialized Project's
// embedded mesh is externalized into a content-addressed blob, that the stored
// document is lean, that load re-inlines it (round-trip), and the one-way
// localStorage → store migration.
import { describe, expect, it, beforeEach } from "vitest";
import { sha256Hex, type AssetStore } from "./assetStore";
import {
  persistProjectDoc,
  loadProjectDoc,
  migrateFromLocalStorage,
  gcLocalBlobs,
  __setAssetStoreForTest,
  __setFontCacheForTest,
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

/** In-memory uploaded-font cache: the persist path reads bytes out of it, the
 *  load path writes them back. Stands in for the real (opentype-parsing) cache,
 *  which would reject the synthetic font bytes a unit test uses. */
class FakeFontCache {
  bytes = new Map<string, Uint8Array>();
  get(id: string): Uint8Array | undefined {
    return this.bytes.get(id);
  }
  set(id: string, b: Uint8Array): void {
    this.bytes.set(id, b);
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

/** A serialized Project whose single part tab references an uploaded font via a
 *  `sketch_path` `text.fontId` (mirrors production: fonts are referenced by id,
 *  their bytes live only in the font cache). */
function projectWithFont(fontId: string): string {
  const documentJson = JSON.stringify({
    schema_version: 1,
    name: "Part",
    feature_calls: [
      {
        type: "sketch_path",
        id: "1",
        params: { text: { group: "g1", value: "Hi", height: 10, fontId, origin: [0, 0] } },
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

/** Unwrap the persisted v2 envelope (`{ v, doc, fonts }`) to its inner
 *  project-json string. Tolerates a legacy raw slot (returns it verbatim). */
function docSlot(slot: string): string {
  try {
    const env = JSON.parse(slot) as { v?: unknown; doc?: unknown };
    if (typeof env.v === "number" && typeof env.doc === "string") return env.doc;
  } catch {
    /* not JSON — fall through */
  }
  return slot;
}

/** The font manifest recorded in the persisted envelope. */
function envelopeFonts(slot: string): Record<string, string> {
  const env = JSON.parse(slot) as { fonts?: Record<string, string> };
  return env.fonts ?? {};
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
  let fonts: FakeFontCache;

  beforeEach(() => {
    fake = new FakeAssetStore();
    __setAssetStoreForTest(fake);
    fonts = new FakeFontCache();
    __setFontCacheForTest(fonts);
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
    const inner = JSON.parse(JSON.parse(docSlot(fake.doc!)).tabs[0].documentJson);
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
    expect(normalized(docSlot(fake.doc!))).toEqual(normalized(doc));
  });

  describe("fonts (P1c)", () => {
    const FONT_ID = "uploaded:MyFont.ttf";
    const FONT_BYTES = new Uint8Array([0x00, 0x01, 0x00, 0x00, 0x42, 0x99]);

    it("externalizes a referenced uploaded font to a blob + envelope manifest", async () => {
      fonts.set(FONT_ID, FONT_BYTES);
      await persistProjectDoc(projectWithFont(FONT_ID));

      const hash = await sha256Hex(FONT_BYTES);
      // Font bytes landed in a content-addressed blob…
      expect(fake.blobs.get(hash)).toEqual(FONT_BYTES);
      // …and the envelope records fontId → hash.
      expect(envelopeFonts(fake.doc!)).toEqual({ [FONT_ID]: hash });
      // The document itself still only carries the id, never the bytes.
      expect(docSlot(fake.doc!)).toContain(FONT_ID);
      expect(docSlot(fake.doc!)).not.toContain("__fontData");
    });

    it("restores the font bytes into the cache on load", async () => {
      fonts.set(FONT_ID, FONT_BYTES);
      await persistProjectDoc(projectWithFont(FONT_ID));

      // Simulate a reload: the in-memory font cache is empty again.
      fonts.bytes.clear();
      expect(fonts.get(FONT_ID)).toBeUndefined();

      const loaded = await loadProjectDoc();
      expect(loaded).not.toBeNull();
      // The load boundary pushed the exact bytes back into the cache.
      expect(fonts.get(FONT_ID)).toEqual(FONT_BYTES);
      // The re-inlined doc still references the font by id.
      expect(loaded!).toContain(FONT_ID);
    });

    it("skips a referenced font with no retained bytes (no blob, empty manifest)", async () => {
      // Cache has no bytes for the id (e.g. opened from a legacy JSON).
      await persistProjectDoc(projectWithFont(FONT_ID));
      expect(fake.blobs.size).toBe(0);
      expect(envelopeFonts(fake.doc!)).toEqual({});
    });

    it("warns (no crash) when a manifested font blob is missing on load", async () => {
      fonts.set(FONT_ID, FONT_BYTES);
      await persistProjectDoc(projectWithFont(FONT_ID));

      // Drop the blob but keep the manifest → the load must degrade gracefully.
      fake.blobs.clear();
      fonts.bytes.clear();
      const loaded = await loadProjectDoc();
      expect(loaded).not.toBeNull();
      expect(fonts.get(FONT_ID)).toBeUndefined();
    });

    it("tolerates a legacy raw-string slot (no envelope) on load", async () => {
      // P1b wrote the raw project-json string directly, with no envelope.
      fake.doc = projectWithFont(FONT_ID);
      const loaded = await loadProjectDoc();
      expect(loaded).not.toBeNull();
      // No fonts restored (legacy slot has no manifest), and no crash.
      expect(fonts.get(FONT_ID)).toBeUndefined();
      expect(JSON.parse(loaded!).tabs[0].kind).toBe("part");
    });
  });

  describe("gcLocalBlobs (P3e)", () => {
    it("deletes an orphaned blob but keeps referenced mesh + font blobs", async () => {
      // Persist a project with a mesh + a referenced font → 2 referenced blobs.
      fonts.set("uploaded:MyFont.ttf", new Uint8Array([1, 2, 3, 4]));
      await persistProjectDoc(
        JSON.stringify({
          schemaVersion: 1,
          name: "Untitled",
          tabs: [
            {
              id: "tab_1",
              kind: "part",
              name: "Part 1",
              documentJson: JSON.stringify({
                schema_version: 1,
                feature_calls: [
                  {
                    type: "imported_mesh",
                    id: "1",
                    params: {
                      op: "New",
                      __positions: [0, 0, 0, 1, 0, 0, 0, 1, 0],
                      __indices: [0, 1, 2],
                    },
                  },
                  {
                    type: "sketch_path",
                    id: "2",
                    params: {
                      text: { value: "Hi", height: 10, fontId: "uploaded:MyFont.ttf", origin: [0, 0] },
                    },
                  },
                ],
                next_feature_id: 3,
              }),
            },
          ],
          activeTabId: "tab_1",
        }),
      );

      const referenced = [...fake.blobs.keys()];
      expect(referenced.length).toBe(2); // mesh + font

      // Inject an orphan blob that no document references.
      const orphan = await sha256Hex(new Uint8Array([9, 9, 9]));
      fake.blobs.set(orphan, new Uint8Array([9, 9, 9]));
      expect(fake.blobs.size).toBe(3);

      const res = await gcLocalBlobs();
      expect(res.deleted).toBe(1);
      expect(res.kept).toBe(2);
      // Orphan gone, both referenced blobs survive.
      expect(fake.blobs.has(orphan)).toBe(false);
      for (const h of referenced) expect(fake.blobs.has(h)).toBe(true);
    });

    it("is a no-op on an empty store (never deletes when nothing is loaded)", async () => {
      // No document stored, but a stray blob exists — must NOT be deleted.
      fake.blobs.set("deadbeef", new Uint8Array([0]));
      const res = await gcLocalBlobs();
      expect(res).toEqual({ deleted: 0, kept: 0 });
      expect(fake.blobs.has("deadbeef")).toBe(true);
    });
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
