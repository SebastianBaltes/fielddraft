// Phase 1a: pure document ⇄ blob transforms. Covers mesh (v2 + legacy) and
// font externalization, the round-trip guarantee, deterministic hashing,
// dedup, and the missing-blob (leave-ref) path.
import { describe, expect, it } from "vitest";
import {
  base64ToBytes,
  bytesToBase64,
  externalizeAssets,
  inlineAssets,
} from "./assetExternalize";

/** Resolver over an in-memory blob map (what an AssetStore would provide). */
function resolverFrom(blobs: Map<string, Uint8Array>) {
  return (hash: string): Promise<Uint8Array | null> =>
    Promise.resolve(blobs.get(hash) ?? null);
}

/** A ~600-byte synthetic "font" as canonical base64. Bytes are f32-agnostic
 *  raw data; the size guarantees the document shrinks once externalized. */
const FONT_BYTES = Uint8Array.from({ length: 600 }, (_, i) => (i * 37 + 11) & 0xff);
const FONT_B64 = bytesToBase64(FONT_BYTES);

/** A document mirroring the real kernel shape: `imported_mesh` calls carry
 *  `__positions`/`__indices` inside `params`; a text call carries an inline
 *  `__fontData`. All mesh floats are f32-exact so the round-trip is lossless. */
function sampleDoc(): unknown {
  return {
    schema_version: 1,
    name: "Part",
    feature_calls: [
      {
        type: "box",
        id: "1",
        params: { half_extents: [10, 10, 10], center: [0, 0, 0] },
      },
      {
        type: "imported_mesh",
        id: "2",
        params: {
          op: "New",
          __watertight: true,
          __positions: [0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1],
          __indices: [0, 1, 2, 0, 1, 3],
        },
      },
      {
        type: "sketch_text",
        id: "3",
        params: {
          text: "Hi",
          fontId: "uploaded:Foo.ttf",
          __fontData: FONT_B64,
        },
      },
    ],
    next_feature_id: 4,
  };
}

describe("base64 helpers", () => {
  it("round-trip is canonical and exact", () => {
    expect(base64ToBytes(bytesToBase64(FONT_BYTES))).toEqual(FONT_BYTES);
    expect(bytesToBase64(base64ToBytes(FONT_B64))).toBe(FONT_B64);
  });
});

describe("externalizeAssets / inlineAssets", () => {
  it("shrinks the document and replaces payloads with asset refs", async () => {
    const docJson = JSON.stringify(sampleDoc());
    const { doc, blobs } = await externalizeAssets(docJson);

    expect(doc.length).toBeLessThan(docJson.length);
    expect(doc).not.toContain("__positions");
    expect(doc).not.toContain("__indices");
    expect(doc).not.toContain("__fontData");
    // One mesh blob + one font blob.
    expect(blobs.size).toBe(2);

    const parsed = JSON.parse(doc);
    const mesh = parsed.feature_calls[1].params;
    expect(mesh.__assetKind).toBe("mesh");
    expect(mesh.__asset).toMatch(/^[0-9a-f]{64}$/);
    expect(mesh.__positions).toBeUndefined();
    // Sibling metadata survives.
    expect(mesh.op).toBe("New");
    expect(mesh.__watertight).toBe(true);

    const font = parsed.feature_calls[2].params;
    expect(font.__assetKind).toBe("font");
    expect(font.fontId).toBe("uploaded:Foo.ttf");
    expect(font.__fontData).toBeUndefined();

    // Every referenced hash is present in the blob map.
    for (const kind of [mesh, font]) {
      expect(blobs.has(kind.__asset)).toBe(true);
    }
  });

  it("round-trips back to the original document", async () => {
    const original = sampleDoc();
    const { doc, blobs } = await externalizeAssets(JSON.stringify(original));
    const inlined = await inlineAssets(doc, resolverFrom(blobs));
    expect(JSON.parse(inlined)).toEqual(original);
  });

  it("hashes deterministically (same doc ⇒ same hashes)", async () => {
    const docJson = JSON.stringify(sampleDoc());
    const a = await externalizeAssets(docJson);
    const b = await externalizeAssets(docJson);
    expect([...a.blobs.keys()].sort()).toEqual([...b.blobs.keys()].sort());
    expect(a.doc).toBe(b.doc);
  });

  it("dedups identical meshes to a single blob", async () => {
    const mk = () => ({
      op: "New",
      __positions: [0, 0, 0, 2, 0, 0, 0, 2, 0],
      __indices: [0, 1, 2],
    });
    const docJson = JSON.stringify({
      schema_version: 1,
      name: "Part",
      feature_calls: [
        { type: "imported_mesh", id: "1", params: mk() },
        { type: "imported_mesh", id: "2", params: mk() },
      ],
      next_feature_id: 3,
    });
    const { doc, blobs } = await externalizeAssets(docJson);
    expect(blobs.size).toBe(1);
    const parsed = JSON.parse(doc);
    expect(parsed.feature_calls[0].params.__asset).toBe(
      parsed.feature_calls[1].params.__asset,
    );
  });

  it("leaves the ref (and does not crash) when a blob is missing", async () => {
    const { doc } = await externalizeAssets(JSON.stringify(sampleDoc()));
    // Resolver that never finds anything.
    const inlined = await inlineAssets(doc, () => Promise.resolve(null));
    const parsed = JSON.parse(inlined);
    const mesh = parsed.feature_calls[1].params;
    expect(mesh.__asset).toMatch(/^[0-9a-f]{64}$/);
    expect(mesh.__positions).toBeUndefined();
  });

  it("handles the legacy __mesh (v1 MeshSdf) format", async () => {
    const original = {
      schema_version: 1,
      name: "Part",
      feature_calls: [
        {
          type: "imported_mesh",
          id: "1",
          params: {
            op: "New",
            __mesh: {
              tris: [0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 2, 0, 0, 0, 2, 0],
              bboxes: [0, 0, 0, 1, 1, 0, 0, 0, 0, 2, 2, 0],
              aabb_min: [0, 0, 0],
              aabb_max: [2, 2, 0],
            },
          },
        },
      ],
      next_feature_id: 2,
    };
    const { doc, blobs } = await externalizeAssets(JSON.stringify(original));
    expect(blobs.size).toBe(1);
    expect(JSON.parse(doc).feature_calls[0].params.__assetKind).toBe(
      "meshLegacy",
    );
    const inlined = await inlineAssets(doc, resolverFrom(blobs));
    expect(JSON.parse(inlined)).toEqual(original);
  });

  it("is a no-op for documents without inline assets", async () => {
    const docJson = JSON.stringify({
      schema_version: 1,
      name: "Part",
      feature_calls: [{ type: "box", id: "1", params: { size: 5 } }],
      next_feature_id: 2,
    });
    const { doc, blobs } = await externalizeAssets(docJson);
    expect(blobs.size).toBe(0);
    expect(JSON.parse(doc)).toEqual(JSON.parse(docJson));
  });
});
