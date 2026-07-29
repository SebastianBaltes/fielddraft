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
import { importOnshapeFeatures } from "./onshapeImport";
import { wrapImportKernel } from "./importKernelDouble";

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
 *  A. CLOSED in #428 — kept here because the diagnosis took three tickets.
 *     `qWall(op, ord)` bound no face for Sketch 4, 5 and 13. The first cause
 *     on record (a tangentially merged mantle) was refuted by measurement in
 *     #424; the second (walls interior to the union of four disjoint
 *     rectangles) was the right shape of answer but the wrong culprit. The
 *     actual reason was that `Boolean 1` CONSUMED Extrude 2: the posts were
 *     folded into the plates' body, so their walls really were interior
 *     faces — of a body they should never have joined. With `keep_tools`
 *     honoured they are a body again and all three sketches resolve.
 *  B. The resolved plane is PARALLEL to Onshape's but offset (Sketch 11, 12 —
 *     Sketch 6 and 7 LEFT this list in #442, see below). Not a decoder defect —
 *     measured in #427, the decoder names exactly the entity Onshape names. The
 *     offset is inherited: Sketch 2's stored 2D geometry in the `features`
 *     payload disagrees with Onshape's own 3D answer by 50 mm for two of its
 *     four rectangles (external references are only re-solved at REGENERATION,
 *     so the payload carries the authoring seed), which puts every wall of
 *     Extrude 2 — and thus Sketch 11 and everything behind it — 50 mm off.
 *     FieldDraft now rotates the 2D coordinates into its own basis anyway and
 *     reports the residual offset, so these sketches sit in the right
 *     ORIENTATION at roughly the right place instead of 1072.5 mm away and
 *     turned 90°.
 *     Sketch 6 and 7 were never in that class at all, and #442 measured why:
 *     the offset the importer compared against was its OWN. `B_fd` came from
 *     the web's `resolvePlaneFrame`, whose face origin is the mean of the outer
 *     loop's edge samples, while the kernel builds the sketch frame on the
 *     fingerprint centroid projected onto the carrier plane — two different
 *     points on any face without symmetry. The kernel now reports its own
 *     anchor (`planeOriginLocal`) and the web uses it, and these two sketches
 *     place without a residual.
 *     `Sketch 8` JOINED this class in #448 and LEFT it again in #447, and both
 *     moves are measurements rather than geometry: its body count, feature
 *     count and scene AABB have been bit-identical throughout.
 *     #448 made it visible — this file's kernel double used to omit
 *     `resolvedMateConnectors`, so the web could not resolve the plane of the
 *     mate connector `qCreatedBy(26)` AT ALL and only warned „Frame nicht
 *     auflösbar". Resolved, it stood 168.96 mm from Onshape's own 3D answer.
 *     #447 found why, and it was not an offset at all: Onshape anchors that
 *     connector on an IMPRINT FACE — the filled inside of a sketch region —
 *     and the anchor decoder read the first `entityType` in the token stream,
 *     which belongs to one of the region's bounding EDGES. The connector's +Z
 *     therefore became that edge's tangent, and its origin slid 168.96 mm
 *     along it. Reading the type from the query DOCUMENT puts the plane on
 *     Onshape's to under a micron.
 *  C. End conditions UP_TO_NEXT / UP_TO_SURFACE (Extrude 8, 12).
 *  D. Hole positions that are not plain sketch points (both `Bohrung`s).
 *  E. CLOSED — twice, and the second time is the interesting one. A cap-edge
 *     mate-connector anchor (`Mirror 2`) bound nothing. Closed in #424,
 *     REOPENED in #440, closed again in #424; the paragraph stays because it
 *     looks like a regression and was the shadow of a correction.
 *     `Extrude 4` selects two regions of Sketch 3, a 920×500 frame. Its first
 *     region query is bounded by `jTxVth8TCEaF.left`/`.right` and by the pieces
 *     of the long top/bottom lines BETWEEN them — i.e. the 20×460 left rail.
 *     #412 read the query as a flat list of curve ids, so the two long lines
 *     went in UNTRIMMED (920 mm each), and the face they accounted for best was
 *     the 880×460 MIDDLE PANEL — a body Onshape never extrudes. #440 trims each
 *     boundary edge to its imprint vertices and the profile is the frame's two
 *     rails, as Onshape has it.
 *     The rails are DISJOINT, though, and both are fenced by the same two long
 *     lines. `wall_sub_table` refused a table whose ids repeat (two walls with
 *     one sub would collapse into one face), so ALL eight walls fell back to
 *     POSITIONAL subs — including the six whose curve is unique — and
 *     `qWall(8, 45)` stopped binding. A wall name now carries the WALL
 *     OCCURRENCE beside the sketch entity (`BranchId::occurrence`), so the two
 *     lines name two walls each and the six others keep theirs. The profile was
 *     deliberately NOT re-widened: the alternative was a mate connector that
 *     works because the body under it is 44× too big.
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
  "Extrude 14",
  "Extrude 15",
  "Extrude 7",
  "Extrude 8",
  "Extrude 9",
  "Mirror 1",
  "Mirror 3",
  "Sketch 10",
  "Sketch 11",
  "Sketch 12",
  "Sketch 14",
];

async function importFixture() {
  const kernel = wrapImportKernel(new WasmKernel());
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
    expect(bb.max[2], `max of ${JSON.stringify(bb)}`).toBeLessThan(GROUND_TRUTH.z[1] + slack);
    // Nothing may hang BELOW Onshape's floor. This bound was widened to 80 mm
    // once (#427): the class-B chain Sketch 6 → Extrude 5 → Sketch 7 swung
    // ~75 mm under it because its sketches were built on a plane perpendicular
    // to Onshape's and 1072.5 mm away. That is fixed at the source — a plane
    // that is merely PARALLEL-OFFSET no longer forfeits the in-plane rotation —
    // so the bound is back on the ground truth.
    expect(bb.min[2], `min of ${JSON.stringify(bb)}`).toBeGreaterThan(GROUND_TRUTH.z[0] - slack);
    // …and it must actually REACH up, rather than collapsing to a slab. This
    // bound was down at 880 mm for one release (#428): `Boolean 1` subtracts
    // the four vertical posts (Extrude 2) from the top plates with Onshape's
    // „Werkzeuge behalten" set, and FieldDraft's Boolean parsed `keep_tools`
    // without ever honouring it — the posts were consumed and the shelf lost
    // its lower structure, leaving the top assembly and the middle shelf,
    // 887 mm apart. With the flag honoured the posts stand again and the scene
    // spans Onshape's own z range.
    expect(bb.max[2] - bb.min[2]).toBeGreaterThan(1500);

    // X and Y are deliberately NOT asserted against the ground truth yet: 20 of
    // Onshape's 33 parts are missing (see KNOWN_GAPS), and the ones that ARE
    // there include mirror sources whose partners never got built, so the union
    // box is both too small in places and too large in others. Claiming a match
    // here would be the kind of green test #412 warned about.
  }, 900_000);
});
