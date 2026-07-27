/**
 * End-to-end acceptance of the #419 part studio against the REAL wasm kernel.
 *
 * Ticket #419 asked for the document
 * `cad.onshape.com/documents/…/e/…` — a large multi-part assembly — to import.
 * It does NOT, and this file is the honest measurement of how far it gets.
 * Until now #419 had NO test at all: the numbers in the gap matrix came from a
 * throwaway probe that was deleted afterwards, so nothing in the repo could
 * reproduce them and nothing would have noticed them changing.
 *
 * GROUND TRUTH comes from the Onshape REST API of that very document, never
 * from our own resolution (the methodology lesson of #412):
 *
 *   GET /api/v6/parts/d/{did}/w/{wid}/e/{eid}       → 17 solid parts
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
 * Onshape's own answer for this document, in mm (the API reports metres): 17
 * solid parts, and the union of their bounding boxes. The x extent is ~3.1 m —
 * this is a long assembly, and that single number is what makes the axis
 * mix-up below unmistakable rather than a rounding argument.
 */
const GROUND_TRUTH = {
  parts: 17,
  x: [-3015.4, 85.6],
  y: [-157, 163],
  z: [-151, 169],
};

/**
 * The Onshape features that still do NOT import, by name, with the gap each one
 * hangs on. Keyed by NAME rather than by message because the messages carry
 * FieldDraft feature ids, which shift the moment anything upstream starts
 * importing — the name is what the user sees in Onshape's tree either way.
 *
 * This list is an assertion in both directions. A new name appearing is a
 * REGRESSION. A name disappearing means a gap was closed and the line belongs
 * deleted — which is the point: the file has to be edited when the import gets
 * better, so the record cannot quietly go stale.
 *
 * 192 messages fall on these 107 features, and MOST of them are cascade — a
 * feature failing only because something it refers to never got built. Measured
 * after #424 (was 216 on 118 features):
 *
 *    44  chamfer/fillet edge shape with no FieldDraft counterpart    → #429
 *    31  cascade: profile sketch missing
 *    27  cascade: wall plane names a feature that was not imported
 *    25  cascade: EDGE query names a sweep that was not imported     (see #425)
 *     5  `qWall(…)` binds 0 faces                                    → #424 rest
 *     5  Onshape's sketch plane stands SKEW to the resolved wall
 *     4  `startOffset` (refused, not misplaced)
 *     3  end condition UP_TO_NEXT / UP_TO_SURFACE
 *     3  no resolvable face / target or tool body for a boolean
 *     2  pattern axis not resolvable in world coordinates
 *     2  hole position is not a plain sketch point
 *     2  feature type `printedHole` has no translator
 *     2  unknown plane-query form
 *    37  the rest: cap planes naming unimported features, undecodable
 *        edge/face query types, `qCreatedBy(op, CAP_TOP)` binding SEVERAL
 *        faces (Sketch 11 — a new shape, exposed only once the extrudes
 *        around it started importing)
 *
 * The edge count moved 42+23 → 29+61 in #425 without a single message
 * disappearing, and that IS the finding: the 65 edge queries that used to fail
 * as „unknown form" / „origin unresolvable" now decode — 29 of them all the way
 * to a NAMED sweep that this document never imports (cascade), the other 61 to a
 * shape (boolean MERGE/SPLIT, pattern COPY, earlier BLEND) FieldDraft cannot
 * name. #419's blend features therefore hang on the extrudes, not on the
 * decoder.
 */
const KNOWN_GAPS = [
  "3D Printed Hole 2",
  "3D Printed Hole 3",
  "Bohrung Ø5.6 mm",
  "Boolean 3",
  "Boolean 5",
  "Boolean 7",
  "Boolean 8",
  "Chamfer 1",
  "Chamfer 10",
  "Chamfer 11",
  "Chamfer 12",
  "Chamfer 2",
  "Chamfer 4",
  "Chamfer 6",
  "Chamfer 8",
  "Chamfer 9",
  "Circular pattern 1",
  "Extrude 11",
  "Extrude 12",
  "Extrude 14",
  "Extrude 15",
  "Extrude 17",
  "Extrude 18",
  "Extrude 19",
  "Extrude 21",
  "Extrude 22",
  "Extrude 24",
  "Extrude 26",
  "Extrude 27",
  "Extrude 28",
  "Extrude 29",
  "Extrude 30",
  "Extrude 31",
  "Extrude 32",
  "Extrude 33",
  "Extrude 34",
  "Extrude 35",
  "Extrude 36",
  "Extrude 37",
  "Extrude 38",
  "Extrude 40",
  "Extrude 41",
  "Extrude 42",
  "Extrude 44",
  "Extrude 45",
  "Extrude 46",
  "Extrude 47",
  "Extrude 5",
  "Extrude 50",
  "Extrude 51",
  "Extrude 52",
  "Extrude 53",
  "Extrude 54",
  "Extrude 8",
  "Extrude 9",
  "Fillet 12",
  "Fillet 13",
  "Fillet 14",
  "Fillet 15",
  "Fillet 16",
  "Fillet 2",
  "Fillet 3",
  "Fillet 4",
  "Fillet 5",
  "Fillet 6",
  "Fillet 7",
  "Fillet 8",
  "Fillet 9",
  "Linear pattern 1",
  "Shell 1",
  "Sketch 10",
  "Sketch 11",
  "Sketch 12",
  "Sketch 14",
  "Sketch 15",
  "Sketch 16",
  "Sketch 17",
  "Sketch 18",
  "Sketch 19",
  "Sketch 20",
  "Sketch 21",
  "Sketch 21 · Mate connector",
  "Sketch 22",
  "Sketch 23",
  "Sketch 24",
  "Sketch 25",
  "Sketch 26",
  "Sketch 27",
  "Sketch 28",
  "Sketch 29",
  "Sketch 30",
  "Sketch 31",
  "Sketch 32",
  "Sketch 33",
  "Sketch 34",
  "Sketch 35",
  "Sketch 36",
  "Sketch 37",
  "Sketch 4",
  "Sketch 5",
  "Sketch 8",
  "Thicken 1",
  "Thicken 2",
  "Thicken 3",
  "Thicken 4",
  "Thicken 5",
  "Thicken 7",
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
  const result = await importOnshapeFeatures(fixture("onshape-419-features.json"), kernel, {
    sketches3d: fixture("onshape-419-sketches3d.json"),
  });
  return { kernel, result };
}

describe("#419 multi-part assembly", () => {
  it("fails only on the gaps that are still open, and on nothing else", async () => {
    const { result } = await importFixture();
    // The FEATURE NAME each error reports under, deduplicated: several errors
    // per feature (a hole with three unresolvable positions) are one gap.
    const failed = [...new Set(result.errors.map((e) => e.split(":")[0]))].sort();
    expect(failed, `import errors:\n${result.errors.join("\n")}`).toEqual(KNOWN_GAPS);
  }, 900_000);

  it("builds the spine it can, every translator represented", async () => {
    const { kernel } = await importFixture();

    // Pinned EXACTLY, and deliberately not as `toBeGreaterThan`, because on
    // this document the count is NOT monotone in import quality: v0.38.3 fixed
    // seven features and the body count FELL from 9 to 7, because `Boolean 2`
    // and `Boolean 4` began to resolve and merged bodies that had been lying
    // around separately. A bound in either direction would have called that
    // either a regression or a win; it is neither. Onshape's own answer is 17
    // parts (GROUND_TRUTH.parts) and that stays the target.
    //
    // 7 → 10 in v0.38.5 (#428): eight of this document's booleans carry
    // Onshape's „Werkzeuge behalten", and FieldDraft parsed `keep_tools`
    // without ever honouring it — the tools were consumed. They stand as their
    // own bodies now. The error list did NOT move (212 messages, same names),
    // so these three are bodies that were being swallowed, not features that
    // newly import.
    //
    // 10 → 12 in #424, and this time it IS new features: `qRegions(...)` over
    // several regions used to union the regions' closed CONTOURS, which put a
    // shared boundary line into the profile twice and made the wall-sub table
    // collapse to positional subs. With the union's own boundary the walls
    // carry their sketch entity again, `qWall(24, 121/133/136)` binds, and
    // eleven features (Boolean 1, Boolean 6, eight extrudes, one hole) left
    // KNOWN_GAPS above. 216 → 192 messages.
    expect(kernel.raw.bodyCount()).toBe(12);

    // Without these the geometry would be a fraction of the assembly and still
    // look plausible — which is exactly how a broken import hides.
    //
    // `fillet_edge` left this list in #425 and came BACK in #424 — which is
    // exactly what #425 predicted would happen and why it was written down.
    //
    // In #425 the one fillet on the list was fabricated: the edge query named a
    // face of a PATTERN COPY, and the marker-based decoder silently filleted
    // the corresponding edge of the pattern's ORIGINAL. The line was then
    // pinned as `not.toContain` with the note that a fillet reappearing means
    // an EXTRUDE started importing. It did: `Extrude 24` now builds, and with
    // it `Fillet 16` resolves — as FOUR distinct calls, one per corner of that
    // extrude, `qCommonEdge(qWall(27, a), qWall(27, b))` over the wall pairs
    // 7/10, 4/10, 4/13, 7/13. Four different edges of one rectangular boss, no
    // two queries alike: the failure mode #425 guarded against was one query
    // appearing twice.
    const kinds = new Set(
      (JSON.parse(kernel.raw.toJson()).feature_calls as { type: string }[]).map((c) => c.type),
    );
    for (const k of [
      "extrude",
      "revolve",
      "thicken",
      "boolean",
      "mate_connector",
      "fillet_edge",
    ]) {
      expect([...kinds], `feature kinds: ${[...kinds].join(", ")}`).toContain(k);
    }
    // No two blends may carry the SAME resolved edge — that is what a
    // fabricated one looks like (#414: `Fillet 1` and `Fillet 2` identical).
    const blendEdges = (JSON.parse(kernel.raw.toJson()).feature_calls as any[])
      .filter((c) => c.type === "fillet_edge" || c.type === "chamfer_edge")
      .map((c) => c.params.edge as string);
    expect(new Set(blendEdges).size, `blend edges: ${blendEdges.join(" | ")}`).toBe(
      blendEdges.length,
    );
  }, 900_000);

  it("stands 90° rotated about Z — pinned as the measured defect it is", async () => {
    const { kernel } = await importFixture();
    const bb = JSON.parse(kernel.raw.sceneAabb()) as { min: number[]; max: number[] };

    // Z is right to within 0.06 mm, so the SIZE of the import is not in doubt.
    const slack = 1.0;
    expect(bb.min[2], `scene ${JSON.stringify(bb)}`).toBeCloseTo(GROUND_TRUTH.z[0], 0);
    expect(bb.max[2]).toBeCloseTo(GROUND_TRUTH.z[1], 0);

    // X and Y are NOT. The whole document comes in turned by +90° about Z:
    // ours(x, y, z) = Onshape(−y, x, z). Three of the four horizontal bounds
    // match that mapping EXACTLY — our x spans [−163, 157] where Onshape's y
    // spans [−157, 163], and our y bottoms out at −3015.4 where Onshape's x
    // does. (Our y top is short of Onshape's 85.6 because 10 of 17 parts are
    // missing, so that one bound is not evidence either way.)
    //
    // #414, #415 and #416 assert x/y/z DIRECTLY against the same API and pass,
    // so this is not a bounding-box frame convention — it is our import. The
    // suspect is the known one: FieldDraft derives a face-bound sketch plane's
    // u/v basis from the face NORMAL, Onshape from the face PARAMETRISATION
    // (gap matrix, #415). Both rules are deterministic and different, and a
    // root sketch that sits on a face hands the rotation to everything after
    // it. The rotation predates v0.38.3 — the AABB is bit-identical across the
    // #424 fix, measured.
    //
    // So this is pinned as the mapping it actually is, NOT as a loose bound:
    // the day the frame is right, these four lines fail and belong replaced by
    // the direct assertion against GROUND_TRUTH.x / .y.
    expect(bb.min[0], `scene ${JSON.stringify(bb)}`).toBeCloseTo(-GROUND_TRUTH.y[1], 0);
    expect(bb.max[0]).toBeCloseTo(-GROUND_TRUTH.y[0], 0);
    expect(bb.min[1]).toBeCloseTo(GROUND_TRUTH.x[0], 0);
    expect(bb.max[1]).toBeLessThan(GROUND_TRUTH.x[1] + slack);
  }, 900_000);
});
