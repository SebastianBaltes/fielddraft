/**
 * End-to-end acceptance of the #409 part studio against the REAL wasm kernel.
 *
 * Ticket #409 asked for the document
 * `cad.onshape.com/documents/b55673157df9a3c81b074d80/…/e/34f53155f6984084e526a35a`
 * — a shelving unit — to import completely. It does NOT yet, and this file is
 * the honest measurement of how far it gets. Every feature type in the document
 * has a translator; that turned out not to be the same thing as importing, and
 * the difference is what the ticket is actually about.
 *
 * GROUND TRUTH comes from the Onshape REST API of that very document, never
 * from our own resolution (the methodology lesson of #412):
 *
 *   GET /api/v6/parts/d/{did}/w/{wid}/e/{eid}       → 33 solid parts
 *   GET …/partid/{id}/boundingboxes                 → the AABBs, unioned below
 *
 * Needs a current `just build-wasm` (the stale-wasm trap).
 */
import { beforeAll, describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import initWasm, { Kernel as WasmKernel } from "../wasm/fielddraft_kernel";
import { importOnshapeFeatures, type ImportKernel } from "./onshapeImport";
import type { SketchSubCall } from "./onshapeSketch";

const wasmDir = join(__dirname, "..", "wasm");

beforeAll(async () => {
  await initWasm({ module_or_path: readFileSync(join(wasmDir, "fielddraft_kernel_bg.wasm")) });
});

const fixture = (name: string): any =>
  JSON.parse(readFileSync(join(__dirname, "fixtures", name), "utf8"));

/**
 * Onshape's own answer for this document, in mm (the API reports metres): 33
 * solid parts, and the union of their bounding boxes.
 */
const GROUND_TRUTH = {
  parts: 33,
  x: [0, 960],
  y: [-550, 40],
  z: [0, 1840],
};

/**
 * The Onshape features that still do NOT import, by name, with the gap each
 * one hangs on. Keyed by NAME rather than by message because the messages carry
 * FieldDraft feature ids, which shift the moment anything upstream starts
 * importing — the name is what the user sees in Onshape's tree either way.
 *
 * This list is an assertion in both directions. A new name appearing is a
 * REGRESSION. A name disappearing means a gap was closed and the line belongs
 * deleted — which is the point: the file has to be edited when the import gets
 * better, so the record cannot quietly go stale.
 *
 * The gaps themselves, grouped (see docs/ONSHAPE-IMPORT-GAP-MATRIX.md):
 *
 *  A. `qWall(op, ord)` binds NO face — FieldDraft splits faces only at creases,
 *     so a rounded rectangle has ONE mantle face where Onshape has eight.
 *     Sketch 4, 5, 13 sit on such a wall.
 *  B. The resolved plane is the WRONG one: same normal, different offset
 *     (Sketch 6, 7, 11, 12). The importer refuses to rewrite coordinates onto
 *     a plane it did not really find, which is why these are errors and not
 *     silently misplaced geometry.
 *  C. End conditions UP_TO_NEXT / UP_TO_SURFACE (Extrude 8, 12).
 *  D. Hole positions that are not plain sketch points (both `Bohrung`s).
 *  E. A cap-edge mate-connector anchor that binds nothing because its extrude
 *     is missing (Mirror 2). CENTROID inference itself is closed since v0.38.1:
 *     on a straight edge the centroid IS the midpoint, so Sketch 8 and its
 *     connector now import.
 *  H. `startOffset` — Onshape starts the extrusion on a plane offset from the
 *     sketch (Extrude 7: 100 mm). FieldDraft's extrude always begins on the
 *     profile plane, so this is refused rather than silently misplaced.
 *  F. A chamfer edge in EDGE/SWEPT_EDGE form (Chamfer 1).
 *  G. A plane query shape the decoder does not know (Sketch 14).
 *
 * Everything else is cascade: an extrude whose profile sketch is gone, a mirror
 * whose source body was never built.
 */
const KNOWN_GAPS = [
  "Bohrung Ø5 mm",
  "Bohrung Ø5.1 mm",
  "Chamfer 1",
  "Extrude 12",
  "Extrude 13",
  "Extrude 14",
  "Extrude 15",
  "Extrude 7",
  "Extrude 8",
  "Extrude 9",
  "Mirror 1",
  "Mirror 2",
  "Mirror 2 · Mate connector",
  "Mirror 3",
  "Sketch 10",
  "Sketch 11",
  "Sketch 12",
  "Sketch 13",
  "Sketch 14",
  "Sketch 4",
  "Sketch 5",
  "Sketch 6",
  "Sketch 7",
];

function wrap(k: WasmKernel): ImportKernel & { raw: WasmKernel } {
  return {
    raw: k,
    addSketch: (planeQuery: string, subCalls: SketchSubCall[]) =>
      k.addSketch(
        planeQuery,
        JSON.stringify(
          subCalls.map((c, i) => ({ type: c.type, id: String(i + 1), params: c.params })),
        ),
      ),
    addFeatureCall: (type: string, paramsJson: string) => k.addFeatureCall(type, paramsJson),
    resolveQuery: (q: string) => {
      const parsed = JSON.parse(k.resolveQuery(q)) as unknown;
      return Array.isArray(parsed) ? (parsed as any[]) : [];
    },
    sceneAabb: () => JSON.parse(k.sceneAabb()),
    bodyCount: () => k.bodyCount(),
    bodyBrepFaces: (i: number) => JSON.parse(k.bodyBrepFaces(i)),
    bodyPlacement: (i: number) => JSON.parse(k.bodyPlacement(i)),
    regionQueryAtPoint: (sketchId: number, u: number, v: number) =>
      k.regionQueryAtPoint(sketchId, u, v),
    listSketchRegions: (sketchId: number, resolution: number) =>
      k.listSketchRegions(sketchId, resolution),
    setSyncMeshing: (enabled: boolean) => k.setSyncMeshing(enabled),
    setVariable: (n: string, v: number, kind: string, formula?: string) =>
      k.setVariable(n, v, kind, formula),
  };
}

async function importFixture() {
  const kernel = wrap(new WasmKernel());
  kernel.raw.setSyncMeshing(true);
  const result = await importOnshapeFeatures(fixture("onshape-409-features.json"), kernel, {
    sketches3d: fixture("onshape-409-sketches3d.json"),
  });
  return { kernel, result };
}

describe("#409 shelving unit", () => {
  it("fails only on the gaps that are still open, and on nothing else", async () => {
    const { result } = await importFixture();
    // The FEATURE NAME each error reports under, deduplicated: several errors
    // per feature (a hole with three unresolvable positions) are one gap.
    const failed = [...new Set(result.errors.map((e) => e.split(":")[0]))].sort();
    expect(failed, `import errors:\n${result.errors.join("\n")}`).toEqual(KNOWN_GAPS);
  }, 900_000);

  it("builds the spine it can, replication features included", async () => {
    const { kernel } = await importFixture();
    expect(kernel.raw.bodyCount()).toBeGreaterThan(0);

    // Without these the geometry would be a fraction of the shelf and still
    // look plausible — which is exactly how a broken pattern import hides.
    const kinds = (JSON.parse(kernel.raw.toJson()).feature_calls as { type: string }[]).map(
      (c) => c.type,
    );
    expect(kinds).toContain("linear_pattern");
    expect(kinds).toContain("mate_connector");
  }, 900_000);

  it("stands where Onshape puts it along the shelf's height", async () => {
    const { kernel } = await importFixture();
    const bb = JSON.parse(kernel.raw.sceneAabb()) as { min: number[]; max: number[] };

    // Z is the axis the whole part hangs from: its first sketch sits on a mate
    // connector offset `#Hoehe-3cm` along the world +Z, and that offset is an
    // EXPRESSION whose `value` field Onshape reports as 0. Reading the field
    // instead of the text put the entire shelf between z = −1617 and z = +15,
    // and nothing complained. So this is the assertion that catches it: the
    // scene must lie inside Onshape's own z range.
    const slack = 1.0;
    expect(bb.min[2], `min of ${JSON.stringify(bb)}`).toBeGreaterThan(GROUND_TRUTH.z[0] - slack);
    expect(bb.max[2], `max of ${JSON.stringify(bb)}`).toBeLessThan(GROUND_TRUTH.z[1] + slack);
    // …and it must actually REACH up there, rather than collapsing to a slab.
    expect(bb.max[2] - bb.min[2]).toBeGreaterThan(1500);

    // X and Y are deliberately NOT asserted against the ground truth yet: 20 of
    // Onshape's 33 parts are missing (see KNOWN_GAPS), and the ones that ARE
    // there include mirror sources whose partners never got built, so the union
    // box is both too small in places and too large in others. Claiming a match
    // here would be the kind of green test #412 warned about.
  }, 900_000);
});
