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
import { importOnshapeFeatures } from "./onshapeImport";
import { wrapImportKernel, regenCensus } from "./importKernelDouble";

const wasmDir = join(__dirname, "..", "wasm");

beforeAll(async () => {
  await initWasm({ module_or_path: readFileSync(join(wasmDir, "fielddraft_kernel_bg.wasm")) });
});

const fixture = (name: string): any =>
  JSON.parse(readFileSync(join(__dirname, "fixtures", name), "utf8"));

/**
 * Onshape's own answer for this document, in mm (the API reports metres): 17
 * solid parts, and the union of their bounding boxes. The x extent is ~3.1 m —
 * this is a long assembly, and that one enormous number is why an axis mix-up
 * shows up here as a fact rather than as a rounding argument. It is what
 * exposed one, and then what proved it gone in #448.
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
 * 169 messages fall on these 88 features, and MOST of them are cascade — a
 * feature failing only because something it refers to never got built. Measured
 * after #447: 170 on 89 before it, 180 on 99 before #448, 190 on 107 before
 * #443 (that comment claimed 192, which was two stale), 216 on 118 before #424.
 * The
 * buckets below are cut freshly from the current messages, so a single line is
 * not comparable one-to-one with the older list; the two TOTALS are:
 *
 *    45  cascade: EDGE query names a feature that was not imported   (see #425)
 *    37  chamfer/fillet edge shape with no FieldDraft counterpart    → #429
 *    24  cascade: wall plane names a feature that was not imported
 *    23  cascade: profile sketch missing
 *     8  no resolvable face / target or tool body
 *     7  cascade: cap plane names a feature that was not imported
 *     6  unknown plane-query form
 *     4  end condition UP_TO_NEXT / UP_TO_SURFACE
 *     4  `startOffset` (refused, not misplaced)
 *     2  the resolved plane is PARALLEL to Onshape's but OFFSET
 *     2  feature type `printedHole` has no translator
 *     2  hole position that is not a sketch point
 *     1  `qWall(…)` binds 0 faces (`Sketch 30`, still open)
 *     1  face profile in COPY form
 *     1  `CAP_EDGE` over three sketch entities
 *     1  `SWEPT_EDGE` over three sketch entities
 *     1  sketch on a mate connector that was itself not imported
 *
 * #448 took ten names off this list and put one back, and NO import code
 * changed for it — only the kernel double this file drives. It used to be built
 * by hand here without `resolvedMateConnectors`, so `resolvePlaneFrame` could
 * not place a sketch on a mate connector AT ALL; the app has always passed the
 * whole `Kernel`. `Sketch 4`, `5`, `6`, `10`, `13`, `14`, `28` and `29` place on
 * their connectors now, `Circular pattern 1` and `Linear pattern 1` find their
 * axis in world coordinates (their sketch line exists), and `Extrude 28`'s
 * UP_TO_SURFACE target resolves. Seven of the eight SKEW messages were the same
 * artefact. `Sketch 9` was what remained, and #447 closed it: its connector
 * `qCreatedBy(23)` resolved to a frame 90.0° off Onshape's because the anchor
 * decoder read the first `entityType` in the token stream — one of the bounding
 * EDGES of the IMPRINT FACE Onshape actually names — and made that edge's
 * tangent the connector's +Z. Read from the query DOCUMENT instead, the anchor
 * is a sketch REGION, its +Z is the sketch normal, and this document has NO
 * skew plane left. Body count 14 → 18 (Onshape: 17), unchanged by #447.
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
  "Extrude 12",
  "Extrude 17",
  "Extrude 18",
  "Extrude 19",
  "Extrude 21",
  "Extrude 22",
  "Extrude 24",
  "Extrude 26",
  "Extrude 27",
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
  "Shell 1",
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
  "Sketch 30",
  "Sketch 31",
  "Sketch 32",
  "Sketch 33",
  "Sketch 34",
  "Sketch 35",
  "Sketch 36",
  "Sketch 37",
  "Sketch 8",
  "Thicken 1",
  "Thicken 2",
  "Thicken 3",
  "Thicken 4",
  "Thicken 5",
];


async function importFixture() {
  const kernel = wrapImportKernel(new WasmKernel());
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

  /**
   * The SECOND failure surface, pinned in both directions (#449).
   *
   * The census above says what the importer refused to WRITE. This says what
   * the KERNEL refused to BUILD out of what was written, and until #449 no
   * suite asked. The two are disjoint — every name here passed the import
   * clean — which is how #416's two rejected chamfers stayed invisible for
   * three tickets.
   *
   * TWO features on this document, and neither appears in `KNOWN_GAPS`:
   * `Extrude 2` and `Extrude 15` are written, accepted, and then refused by
   * the kernel because the sketch they name carries no closed region. Both
   * were found by introducing this pin, both are labelled from `result.idMap`
   * directly (not inferred), and neither is diagnosed yet — see
   * docs/ONSHAPE-IMPORT-GAP-MATRIX.md, Update (10).
   */
  it("loses exactly two features at REGENERATION, both to empty profiles (#449)", async () => {
    const { kernel, result } = await importFixture();
    const census = regenCensus(kernel.raw, result, fixture("onshape-419-features.json"));
    expect(census.map((c) => c.name)).toEqual(["Extrude 2", "Extrude 15"]);
    for (const c of census) expect(c.error).toContain("no closed regions to extrude");
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
    //
    // 14 → 18 in #448, and no import code changed: this suite's kernel double
    // grew the `resolvedMateConnectors` the app always had, eight sketches that
    // stand on mate connectors place, and two patterns and an extrude follow.
    // That is 18 against Onshape's 17 — the closest this document has come, and
    // still not the same partition: our 18 include bodies Onshape merges and
    // exclude parts we never build, so the number agreeing to ±1 is arithmetic,
    // not a match. It stays pinned exactly, in both directions.
    expect(kernel.raw.bodyCount()).toBe(18);

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
    //
    // `circular_pattern` and `linear_pattern` joined the list in #448. Both
    // take their axis from a sketch LINE (`b6zz5LCAEe08`) of a sketch that sits
    // on a mate connector; with the plane unresolvable that line had no world
    // coordinates and both features were skipped. Pinning them here is what
    // makes the replication of this assembly a regression when it stops.
    for (const k of [
      "extrude",
      "revolve",
      "thicken",
      "boolean",
      "mate_connector",
      "fillet_edge",
      "circular_pattern",
      "linear_pattern",
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
    // its radial profile — 46.68 … 52.83 local had no material at all. Faces 8
    // and 9 are the two halves of that band.
    //
    // The INDICES moved in #448 (`[4, 6, 7]` → `[6, 8, 9]`) and the faces did
    // not. Sketch 3 sits on the mate connector `qCreatedBy(5)`; with the
    // connector resolvable its 2D coordinates are rotated into the connector's
    // own frame instead of being taken literally, and the kernel's arrangement
    // of the SAME curves comes out in a different order. So the index list is
    // pinned together with the AREAS it has to select, which is the invariant
    // the prose above actually claims: 7.74 × 6.15 = 47.6 mm² and
    // 7.74 × 8.10 = 62.7 mm² for the split band.
    expect(named("Revolve 1").params.profiles).toBe("qRegions(6, [6, 8, 9])");
    const regions3 = JSON.parse(kernel.listSketchRegions(6, 128)) as {
      index: number;
      area: number;
    }[];
    const areaOf = (i: number) => regions3.find((r) => r.index === i)!.area;
    expect(areaOf(8), "lower half of the split band").toBeCloseTo(47.57, 1);
    expect(areaOf(9), "upper half of the split band").toBeCloseTo(62.65, 1);
    // Same document, same defect, other feature: Onshape's `Extrude 4` selects
    // the THREE strips one rectangle is cut into. Two of them name literally
    // the same curves and differ only in where those curves are trimmed.
    expect(named("Extrude 4").params.profiles).toBe("qRegions(6, [7, 9, 12])");

    // And the point of the exercise: `Thicken 6` names the cylinder wall the
    // revolve sweeps at r = 7.735 mm. With the gap, that wall was buried under
    // `Extrude 4`'s block along its whole length — the face did not exist and
    // the query bound the wrong one. Now it resolves to exactly one face, on
    // the revolve (feature 25), and it is that cylinder. Body index and feature
    // id both moved by the two pattern features #448 let in ahead of them
    // (`Circular pattern 1` = 21, `Linear pattern 1` = 22); the face itself is
    // the same cylinder at the same radius.
    const faces = String(named("Thicken 6").params.faces);
    const hits = kernel.resolveQuery(faces);
    expect(hits.length, `Thicken 6 faces = ${faces}`).toBe(1);
    const body13 = JSON.parse(kernel.raw.bodyBrepFaces(13)) as any[];
    const carrier = body13.find((f) => String(f.idHash) === String(hits[0].idHash));
    expect(carrier, `body 13 faces: ${body13.map((f) => f.idHash).join(",")}`).toBeTruthy();
    expect(carrier.featureId).toBe(25);
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
    // 64.527], z ∈ [−12.95; 31.05] mm.
    //
    // ALL SIX bounds are asserted since #448, where until then only the
    // extrude's own axis was — and rotated at that, because the whole document
    // used to come in turned 90° about Z (Onshape's y onto our −x). That
    // rotation was an artefact of this file's kernel double, not of the import:
    // it came from the root sketches whose mate-connector planes the double
    // could not resolve, so the web synthesised a basis from the face beneath
    // them. With the app's surface the body lands on Onshape's own numbers to
    // the last measured digit — this is the single sharpest evidence in the
    // repo that #419's placement is right, and it is worth six lines.
    const box = Array.from(kernel.raw.bodySdfAabb(14));
    const label = `Extrude 16 body aabb ${box.join(",")}`;
    expect(box[0], label).toBeCloseTo(-5.011, 1);
    expect(box[1], label).toBeCloseTo(46.677, 1);
    expect(box[2], label).toBeCloseTo(-12.95, 1);
    expect(box[3], label).toBeCloseTo(38.989, 1);
    expect(box[4], label).toBeCloseTo(64.527, 1);
    expect(box[5], label).toBeCloseTo(31.05, 1);

    // And now both features BITE. Suppressing either one changes the B-Rep of
    // that body, which is the whole point of #440 — measured, not argued.
    for (const name of ["Boolean 1", "Thicken 6"]) {
      const before = kernel.raw.bodyBrepCounts(14);
      kernel.raw.setFeatureSuppressed(named(name).id, true);
      expect(kernel.raw.bodyBrepCounts(14), `${name} moves nothing`).not.toBe(before);
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
      "qRegions(32, [0, 1, 2, 3, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14])",
    );

    // The consequence, and the reason this has a ticket: with the plate in the
    // profile the inner rectangle lay INSIDE the extruded area and bounded
    // nothing, so none of its four sides minted a wall. `Thicken 7` names two
    // of them and bound nothing at all. All four resolve now, and those faces
    // are the ones `Thicken 7` asks for.
    //
    // Wall 26 bound TWO faces between #442 and #448, because `Extrude 25`
    // selects this extrude's CAP FACE and the boss it grows there cut wall 26
    // in two — an AMBIGUOUS name, which is dropped rather than guessed, so
    // `Thicken 7` could only ask for three of the four. In #448 it binds ONE
    // again and all four are asked for. Nothing about walls or `qUnion`
    // changed: `Extrude 25` sits behind `Sketch 13`, one of the eight sketches
    // whose mate-connector plane this suite's double could not resolve, so the
    // boss it grows now lands where Onshape has it instead of across that wall.
    // The `MERGE → qUnion` gap of `docs/DERIVED-NAMING-PLAN.md` §4/E3 is
    // untouched by this — it is simply no longer reached HERE.
    const walls = [29, 26, 17, 11].map((e) => kernel.resolveQuery(`qWall(33, ${e})`));
    for (const [i, hits] of walls.entries()) {
      expect(hits.length, `qWall(33, ${[29, 26, 17, 11][i]})`).toBe(1);
    }
    const asked = String(named("Thicken 7").params.faces);
    for (const [i, hits] of walls.entries()) {
      expect(asked, `Thicken 7 misses wall ${[29, 26, 17, 11][i]}: ${asked}`).toContain(
        String(hits[0].idHash),
      );
    }

    // And it BITES: suppressing it moves the B-Rep of the body it thickens.
    const before = kernel.raw.bodyBrepCounts(15);
    kernel.raw.setFeatureSuppressed(named("Thicken 7").id, true);
    expect(kernel.raw.bodyBrepCounts(15), "Thicken 7 moves nothing").not.toBe(before);
    kernel.raw.setFeatureSuppressed(named("Thicken 7").id, false);
  }, 900_000);

  it("stands on Onshape's own axes, five bounds of six", async () => {
    const { kernel } = await importFixture();
    const bb = JSON.parse(kernel.raw.sceneAabb()) as { min: number[]; max: number[] };
    const where = `scene ${JSON.stringify(bb)}`;

    // This test used to be called „stands 90° rotated about Z" and pinned the
    // mapping ours(x, y, z) = Onshape(−y, x, z) as a measured defect of the
    // import, with the face-normal-versus-parametrisation basis rule (#415) as
    // the suspect. That was WRONG, and #448 says how: the rotation lived in
    // this file's kernel double, which had no `resolvedMateConnectors`. The
    // document's root sketches sit on mate connectors; unable to resolve one,
    // `resolvePlaneFrame` fell through to the FACE under it and synthesised a
    // basis from that face's normal — and a root sketch hands its rotation to
    // everything after it, which is exactly the mechanism the old comment
    // described, applied to the wrong actor. The app passes the whole `Kernel`
    // and always did, so the app never had this rotation.
    //
    // Five of the six bounds now land on Onshape's own numbers to under
    // 0.06 mm, ON THE SAME AXIS. #414, #415 and #416 assert x/y/z directly
    // against the same API and pass; #419 does too now.
    expect(bb.min[0], where).toBeCloseTo(GROUND_TRUTH.x[0], 0);
    expect(bb.min[1], where).toBeCloseTo(GROUND_TRUTH.y[0], 0);
    expect(bb.max[1], where).toBeCloseTo(GROUND_TRUTH.y[1], 0);
    expect(bb.min[2], where).toBeCloseTo(GROUND_TRUTH.z[0], 0);
    expect(bb.max[2], where).toBeCloseTo(GROUND_TRUTH.z[1], 0);

    // `x max` is the sixth, and it is short — 71.48 against Onshape's 85.6 —
    // for the honest reason: 89 features do not import (KNOWN_GAPS above), so
    // the union box cannot reach everything Onshape's does. It is asserted as
    // an upper bound plus the distance it currently falls short, so that both
    // directions are pinned: growing past Onshape is a bug, and closing the
    // 14 mm is a win that has to be recorded here rather than passing silently.
    const slack = 1.0;
    expect(bb.max[0], where).toBeLessThan(GROUND_TRUTH.x[1] + slack);
    expect(bb.max[0], where).toBeCloseTo(71.48, 0);
  }, 900_000);

  it("gives `Sketch 4`'s mirrored bores their own inside (#446)", async () => {
    const { kernel, result } = await importFixture();

    // `Sketch 4`'s four keyhole slots are one drawing plus three MIRRORED
    // copies, and a mirror flips a circle's parametrisation to CLOCKWISE.
    // Onshape states its region flank against that direction; we flattened
    // every full circle counter-clockwise regardless, so for the two mirrored
    // Ø 1.9 mm bores „left of the edge" pointed outward and the veto elected
    // the plate around them. Three extrudes therefore LOST those two regions.
    //
    // This case was added in #446 as the ONE test in the file that drove the
    // app's kernel surface, because the hand-built double could not resolve
    // `Sketch 4`'s plane and so could not see the bug at all. In #448 the whole
    // file drives that surface, so it shares `importFixture()` with everything
    // else — one import less, and no second definition of what „the app" means.
    const calls = JSON.parse(kernel.raw.toJson()).feature_calls as any[];
    const profiles = (n: string) =>
      String(calls.find((c) => c.params?.display_name === n)!.params.profiles);
    for (const n of ["Extrude 20", "Extrude 23", "Extrude 49"]) {
      expect(profiles(n), n).toContain(", 7, ");
      expect(profiles(n), n).toContain(", 14]");
    }

    // NO skew plane left in this document. Eight sketches reported one before
    // #448, seven of them only because their plane never resolved at all in
    // this file's kernel double; the eighth was `Sketch 9` on the mate
    // connector `qCreatedBy(23)`, and #447 closed it — Onshape anchors that
    // connector on an IMPRINT FACE, and reading the anchor's type from the
    // query document instead of from the first token in its stream makes its
    // +Z the sketch normal rather than one bounding edge's tangent.
    //
    // Kept as an assertion on the COUNT rather than deleted: a skew plane is
    // the loudest symptom a wrong frame has, and this is the one document with
    // enough mate connectors to produce one again.
    const skew = result.errors.filter((e) => e.includes("schief"));
    expect(skew, skew.join(" | ")).toHaveLength(0);
  }, 900_000);
});
