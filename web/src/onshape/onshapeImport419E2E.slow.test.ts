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
 * 177 messages fall on these 97 features, and MOST of them are cascade — a
 * feature failing only because something it refers to never got built. Measured
 * after #444: 179 on 98 features beforehand, 190 on 107 before #443 (that
 * comment claimed 192, which was two stale), 216 on 118 before #424. The
 * buckets below are cut freshly from the current messages, so a single line is
 * not comparable one-to-one with the older list; the two TOTALS are:
 *
 *    45  cascade: EDGE query names a feature that was not imported   (see #425)
 *    38  chamfer/fillet edge shape with no FieldDraft counterpart    → #429
 *    24  cascade: wall plane names a feature that was not imported
 *    23  cascade: profile sketch missing
 *     8  Onshape's sketch plane stands SKEW to the resolved wall — SEVEN of
 *        them are an artefact of `wrap()` below, which omits
 *        `resolvedMateConnectors` and so cannot place a sketch on a mate
 *        connector at all; the app has ONE (measured, #446 → ticket #448)
 *     8  no resolvable face / target or tool body
 *     7  cascade: cap plane names a feature that was not imported
 *     6  unknown plane-query form
 *     5  end condition UP_TO_NEXT / UP_TO_SURFACE
 *     4  `startOffset` (refused, not misplaced)
 *     1  `qWall(…)` binds 0 faces (`Sketch 30`, still open)
 *     2  pattern axis not resolvable in world coordinates
 *     2  feature type `printedHole` has no translator
 *     4  the rest: two hole positions that are not sketch points, one
 *        `CAP_EDGE` over three sketch entities, one sketch on a mate
 *        connector that was itself not imported
 *
 * Nine names left the list in #443, and only ONE of them is the fix itself:
 * `qCreatedBy(op, CAP_TOP)` binding four COPLANAR cap faces is not an
 * ambiguity for a PLANE, so `Sketch 11` imports — and with it `Sketch 12`,
 * `Boolean 3` and the extrudes 5, 11, 14, 15, 45, 47 that hung on the two.
 *
 * `Thicken 7` left in #444: the two walls it names exist now that `Sketch 4`'s
 * three MIRRORED keyhole slots close (their arcs were read as sketch angles
 * instead of angles of the arc's own, reflected frame). Nothing else moved —
 * the other 97 names and the body count are bit-identical, which is the honest
 * reading of a fix that only repairs geometry three copies of one slot had.
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
  "Extrude 12",
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
  "Extrude 46",
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
  "Sketch 13",
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
  "Sketch 6",
  "Sketch 8",
  "Thicken 1",
  "Thicken 2",
  "Thicken 3",
  "Thicken 4",
  "Thicken 5",
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
    //
    // 12 → 15 in #443: `Sketch 11` sits on a plane Onshape names as a MERGE of
    // coplanar cap faces, so our gate saw four faces and skipped the sketch —
    // and with it everything downstream. Nine names left KNOWN_GAPS (190 → 179
    // messages), six of them extrudes and one a boolean — and the NET effect on
    // the body partition is +3, which is the honest reading: how many of the
    // six extrudes make their own body and how many merge into one is not
    // measured here, only the total is.
    // 15 → 14 in #442, and the cause is a translation that got RIGHT rather
    // than wrong: `Extrude 22` and `Extrude 25` select a CAP FACE of an earlier
    // extrude (Onshape `CAP_FACE`), not a sketch region. Read as an
    // unresolvable region they used to fall back to the WHOLE sketch behind the
    // query — the failure mode that made the #416 document extrude a 32 mm
    // block. They now extrude the one cap they name, which is a smaller solid
    // in a different place, and the partition folds one body into another.
    // The count is a coarse proxy for „the assembly is still there"; which of
    // the extrudes make their own body is not measured here.
    expect(kernel.raw.bodyCount()).toBe(14);

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

  it("gives `Revolve 1` its whole profile, and `Thicken 6` a real face (#440)", async () => {
    const { kernel } = await importFixture();
    const calls = JSON.parse(kernel.raw.toJson()).feature_calls as any[];
    const named = (n: string) => calls.find((c) => c.params?.display_name === n)!;

    // Onshape's `Revolve 1` selects two regions of Sketch 3; the lower one is
    // the rectangle x ∈ [9.25; 16.99] × y ∈ [−60.93; −46.68], which OUR
    // arrangement splits at y = −52.83 (a neighbouring rectangle's top edge
    // runs through it). #412 mapped a region to its single best face and lost
    // the upper half in silence, so the revolve came out with a 6.15 mm GAP in
    // its radial profile — 46.68 … 52.83 local had no material at all. Face 6
    // is that band.
    expect(named("Revolve 1").params.profiles).toBe("qRegions(6, [4, 6, 7])");
    // Same document, same defect, other feature: Onshape's `Extrude 4` selects
    // the THREE strips one rectangle is cut into. Two of them name literally
    // the same curves and differ only in where those curves are trimmed.
    expect(named("Extrude 4").params.profiles).toBe("qRegions(6, [3, 4, 5])");

    // And the point of the exercise: `Thicken 6` names the cylinder wall the
    // revolve sweeps at r = 7.735 mm. With the gap, that wall was buried under
    // `Extrude 4`'s block along its whole length — the face did not exist and
    // the query bound the wrong one. Now it resolves to exactly one face, on
    // the revolve (feature 23), and it is that cylinder.
    const faces = String(named("Thicken 6").params.faces);
    const hits = kernel.resolveQuery(faces);
    expect(hits.length, `Thicken 6 faces = ${faces}`).toBe(1);
    const body9 = JSON.parse(kernel.raw.bodyBrepFaces(9)) as any[];
    const carrier = body9.find((f) => String(f.idHash) === String(hits[0].idHash));
    expect(carrier, `body 9 faces: ${body9.map((f) => f.idHash).join(",")}`).toBeTruthy();
    expect(carrier.featureId).toBe(23);
    expect(carrier.surfaceParams?.kind).toBe("cylinder");
    expect(carrier.surfaceParams?.radius).toBeCloseTo(7.735, 3);

    // `Extrude 16` — the body BOTH of them work on — ends UP_TO_SURFACE with
    // `oppositeDirection: true`, and until v0.42.2 that combination put it on
    // the wrong side: the measured signed distance was flipped a second time
    // by the flag, so the body grew 15.85 mm PAST its own sketch plane instead
    // of up to the target, and neither `Boolean 1` nor `Thicken 6` touched
    // anything. The depth is the measurement itself now, sign included.
    expect(named("Extrude 16").params.depth.value).toBeCloseTo(-15.85, 2);

    // Onshape's own answer, via `evBox3d` on `qCreatedBy(FuEM5hJm4ZF6lHu_11,
    // BODY)` in the source document: x ∈ [−5.011; 38.989], y ∈ [46.677;
    // 64.527], z ∈ [−12.95; 31.05] mm. Only the extrude's OWN axis is asserted
    // here — the document's 90° rotation (see the test below) maps Onshape's y
    // onto our −x, and the other two axes additionally carry the frame offsets
    // that are still open elsewhere in this file. Our x bounds reproduce
    // Onshape's y bounds to 0.01 mm; before the fix the body sat at
    // [−78.38; −62.53], a whole depth away.
    const box = Array.from(kernel.raw.bodySdfAabb(10));
    expect(box[0], `Extrude 16 body aabb ${box.join(",")}`).toBeCloseTo(-64.527, 1);
    expect(box[3]).toBeCloseTo(-46.677, 1);

    // And now both features BITE. Suppressing either one changes the B-Rep of
    // that body, which is the whole point of #440 — measured, not argued.
    for (const name of ["Boolean 1", "Thicken 6"]) {
      const before = kernel.raw.bodyBrepCounts(10);
      kernel.raw.setFeatureSuppressed(named(name).id, true);
      expect(kernel.raw.bodyBrepCounts(10), `${name} moves nothing`).not.toBe(before);
      kernel.raw.setFeatureSuppressed(named(name).id, false);
    }

    // The one thing `Thicken 6` still does NOT reproduce is WHERE its 0.1 mm
    // shell goes. Onshape grows it OUTWARD (the cavity in `Extrude 16`'s body
    // widens from r = 7.735 to r = 7.835 — measured in the source document),
    // we grow it inward, so our cut lands in material `Boolean 1` already took
    // away and only grazes the seam. The cause is face PROVENANCE, not
    // direction: Onshape's `thickness1` is 0.1 mm and its `oppositeDirection`
    // does mean „inward" (verified by running Onshape's own `thicken` on that
    // very face both ways), but the face Onshape names is the one `Boolean 1`
    // COPIED onto the cavity — same surface, opposite normal — while we
    // resolve the revolve's own outer wall. Flipping `reverse` would cancel two
    // errors instead of fixing one; the repair is naming the cavity wall
    // (#429's derived-naming ground). See docs/research/440-…-provenance.md.
    expect(named("Thicken 6").params.thickness.value).toBeCloseTo(0.1, 6);
  }, 900_000);

  it("closes `Sketch 4`'s mirrored slots, so `Extrude 20` is a frame (#444)", async () => {
    const { kernel } = await importFixture();
    const calls = JSON.parse(kernel.raw.toJson()).feature_calls as any[];
    const named = (n: string) => calls.find((c) => c.params?.display_name === n)!;

    // `Sketch 4` draws a 44 × 44 mm plate with a 41 × 41 and a 38 × 38 rectangle
    // inside it and FOUR identical keyhole slots — one drawn, three mirrored.
    // Onshape selects 14 regions for `Extrude 20`: both 1.5 mm rings and all
    // twelve faces the slots cut, but NOT the 1110 mm² plate in the middle.
    //
    // Until #444 the three mirrored slots' arcs were read as SKETCH angles,
    // while Onshape stores them as angles of the arc's OWN frame — which a
    // mirror REFLECTS. The copies' arcs therefore landed mirrored, their
    // tangent lines no longer met them, and the arrangement had 14 faces
    // instead of 15 (two of them 0.03 mm² slivers cut by an arc crossing a line
    // it should have ended on). The 14 regions collapsed onto 11 of them, three
    // pairs colliding, and the region that should have been a slot fell on the
    // big plate — so the extrude covered the plate and its frame TOGETHER.
    expect(named("Extrude 20").params.profiles).toBe(
      "qRegions(30, [0, 1, 2, 3, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14])",
    );

    // The consequence, and the reason this has a ticket: with the plate in the
    // profile the inner rectangle lay INSIDE the extruded area and bounded
    // nothing, so none of its four sides minted a wall. `Thicken 7` names two
    // of them and bound nothing at all. All four resolve now, and those faces
    // are the ones `Thicken 7` asks for.
    //
    // Wall 26 binds TWO faces since #442, and that is a consequence of MORE
    // geometry, not of a worse name: `Extrude 25` selects this very extrude's
    // CAP FACE, which used to fall back to a whole sketch elsewhere and now
    // grows the boss Onshape has there — and that boss cuts wall 26 in two.
    // Onshape reports its own wall as ONE face because it merges coplanar
    // faces and we do not (the standing gap of `docs/DERIVED-NAMING-PLAN.md`
    // §4/E3); the pieces are both right and both named, they just need a
    // `qUnion` the translator does not build yet.
    const walls = [29, 26, 17, 11].map((e) => kernel.resolveQuery(`qWall(31, ${e})`));
    for (const [i, hits] of walls.entries()) {
      const entity = [29, 26, 17, 11][i];
      expect(hits.length, `qWall(31, ${entity})`).toBe(entity === 26 ? 2 : 1);
    }
    // Wall 26 is the one that got cut in two, and an AMBIGUOUS name is dropped
    // rather than guessed — thickening the wrong half adds phantom material.
    // So `Thicken 7` asks for three of the four. Closing that needs the
    // `MERGE → qUnion` translator of `docs/DERIVED-NAMING-PLAN.md` §4/E3: the
    // two pieces together ARE Onshape's one face, and naming their union is the
    // honest answer, not picking one.
    const asked = String(named("Thicken 7").params.faces);
    for (const [i, hits] of walls.entries()) {
      if ([29, 26, 17, 11][i] === 26) continue;
      expect(asked, `Thicken 7 faces = ${asked}`).toContain(String(hits[0].idHash));
    }

    // And it BITES: suppressing it moves the B-Rep of the body it thickens.
    const before = kernel.raw.bodyBrepCounts(11);
    kernel.raw.setFeatureSuppressed(named("Thicken 7").id, true);
    expect(kernel.raw.bodyBrepCounts(11), "Thicken 7 moves nothing").not.toBe(before);
    kernel.raw.setFeatureSuppressed(named("Thicken 7").id, false);
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

  it("gives `Sketch 4`'s mirrored bores their own inside — in the APP's kernel (#446)", async () => {
    // This one test drives the kernel THE APP DRIVES. Every other case in this
    // file (and in the five sibling suites) builds `wrap()` by hand and leaves
    // `resolvedMateConnectors` off it, so a sketch placed on a mate connector
    // cannot resolve its plane at all — three of them here, and with them the
    // features that stand on those. Measured on this fixture, same code, only
    // the double differing: 14 bodies against 18, 99 failing features against
    // 89, and EIGHT sketches whose plane resolves 90° skew against one. Closing
    // that gap means re-measuring every pin in six files and has its own
    // ticket (#448); until then this test is the one place where the numbers
    // are the user's.
    //
    // And it is not academic. `Sketch 4`'s four keyhole slots are one drawing
    // plus three MIRRORED copies, and a mirror flips a circle's parametrisation
    // to CLOCKWISE. Onshape states its region flank against that direction; we
    // flattened every full circle counter-clockwise regardless, so for the two
    // mirrored Ø 1.9 mm bores „left of the edge" pointed outward and the veto
    // elected the plate around them. In the app — where the flank was readable
    // — three extrudes therefore LOST those two regions; in the double above,
    // where no plane resolved, nobody noticed. Both configurations now name the
    // same fourteen faces, which is the reading #444 established.
    const kernel = wrap(new WasmKernel());
    (kernel as any).resolvedMateConnectors = () =>
      JSON.parse(kernel.raw.resolvedMateConnectors());
    kernel.raw.setSyncMeshing(true);
    const result = await importOnshapeFeatures(fixture("onshape-419-features.json"), kernel, {
      sketches3d: fixture("onshape-419-sketches3d.json"),
    });
    const calls = JSON.parse(kernel.raw.toJson()).feature_calls as any[];
    const profiles = (n: string) =>
      String(calls.find((c) => c.params?.display_name === n)!.params.profiles);
    for (const n of ["Extrude 20", "Extrude 23", "Extrude 49"]) {
      expect(profiles(n), n).toContain(", 7, ");
      expect(profiles(n), n).toContain(", 14]");
    }
    expect(profiles("Extrude 20")).toBe(
      "qRegions(32, [0, 1, 2, 3, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14])",
    );
    // The size of #448, pinned so it cannot grow unnoticed: ONE skew plane here
    // against the eight the poorer double produces.
    expect(result.errors.filter((e) => e.includes("schief")).length).toBe(1);
  }, 900_000);
});
