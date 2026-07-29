/**
 * End-to-end acceptance of the #416 part studio against the REAL wasm kernel.
 *
 * Ticket #416 asked for the document
 * `cad.onshape.com/documents/204cc6691c558887e7c25a87/…/e/a24d92526c6c8b6a71401989`
 * to import completely. Per the gap matrix it needed two translators: `chamfer`
 * (landed in v0.32.0) and `revolve` (this ticket).
 *
 * GROUND TRUTH comes from the Onshape REST API of that very document, never
 * from our own resolution (the methodology lesson of #412):
 *
 *   GET /api/v6/parts/d/{did}/w/{wid}/e/{eid}       → ONE solid part, "Part 1"
 *   GET …/partid/JHD/boundingboxes                  → the AABB below
 *
 * Needs a current `just build-wasm` (the stale-wasm trap).
 */
import { beforeAll, describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import initWasm, { Kernel as WasmKernel } from "../wasm/fielddraft_kernel";
import { importOnshapeFeatures } from "./onshapeImport";
import { wrapImportKernel, type SuiteKernel } from "./importKernelDouble";
import type { MesherRebuildRequest } from "../mesher.worker";

const wasmDir = join(__dirname, "..", "wasm");

/** WASM linear memory — needed to read the worker's mesh buffers by pointer. */
let wasmMemory: WebAssembly.Memory;

beforeAll(async () => {
  const exports = (await initWasm({
    module_or_path: readFileSync(join(wasmDir, "fielddraft_kernel_bg.wasm")),
  })) as unknown as { memory: WebAssembly.Memory };
  wasmMemory = exports.memory;
});

const ticket416 = (): { features: any[] } =>
  JSON.parse(readFileSync(join(__dirname, "fixtures", "onshape-416-features.json"), "utf8"));

/** Onshape's own 3D answer for the document's sketches — the only exact source
 *  for its sketch FRAME on a body face (#415). Without it a face-bound sketch is
 *  taken literally and lands wherever our own basis puts it. */
const sketches3d = (): unknown =>
  JSON.parse(readFileSync(join(__dirname, "fixtures", "onshape-416-sketches3d.json"), "utf8"));

/**
 * Onshape's own answer for this document, in mm (the API reports metres).
 * ONE solid part, `JHD` / „Part 1“.
 */
const GROUND_TRUTH = {
  parts: 1,
  x: [-70.0501, 3.074],
  y: [-26.774, 7.1292],
  z: [-73.0819, 35.0],
};

/** The document's revolve: a half-disc of R = 4.94 mm (arc `2hz9RlIR7uPq`,
 *  centred on the sketch origin) spun a full turn around the construction line
 *  `nsFmMv4kYM6f` that runs through that origin — i.e. a SPHERE of that radius,
 *  centred where the sketch origin sits. Read off the fixture geometry, not off
 *  our import. */
const SPHERE_R_MM = 4.94;

const featureById = (doc: { features: any[] }, id: string): any =>
  doc.features.map((f: any) => f.message ?? f).find((f: any) => f.featureId === id);

const paramOf = (feat: any, id: string): any =>
  feat.parameters.map((p: any) => p.message ?? p).find((p: any) => p.parameterId === id);


interface FeatureCall {
  id: string | number;
  type: string;
  params: Record<string, any>;
}

const callsOf = (k: WasmKernel): FeatureCall[] =>
  JSON.parse(k.toJson()).feature_calls as FeatureCall[];

/** Per-feature regeneration errors, keyed by feature id. */
function regenErrors(k: WasmKernel): Map<string, string> {
  const rows = JSON.parse(k.listFeatures()) as { id: string | number; error?: string | null }[];
  const out = new Map<string, string>();
  for (const r of rows) if (r.error) out.set(String(r.id), r.error);
  return out;
}

/** Replay of `mesher.worker.ts`'s `handleRebuild` — the store→worker path that
 *  decides whether the user sees anything at all (the #412 lesson: structure
 *  and SDF can be right while the viewport stays empty). */
function replayMesherWorker(msg: MesherRebuildRequest): WasmKernel {
  const kernel = new WasmKernel();
  kernel.setSyncMeshing(true);
  kernel.setBrepSuppressed(true);
  kernel.loadJson(msg.documentJson);
  kernel.setMeshAlgorithm(msg.meshAlgorithm ?? "mdc");
  kernel.setMeshResolution(msg.activeMeshResolution);
  kernel.setContextMeshResolution(msg.contextMeshResolution);
  kernel.setActiveBodyId(msg.activeBodyId);
  return kernel;
}

/** World-space AABB of a body's render mesh, read like the worker reads it. */
function meshAabb(k: WasmKernel, i: number): { min: number[]; max: number[] } {
  const n = k.bodyVertexCount(i);
  const p = new Float32Array(wasmMemory.buffer, k.bodyPositionsPtr(i), n * 3);
  const min = [Infinity, Infinity, Infinity];
  const max = [-Infinity, -Infinity, -Infinity];
  for (let v = 0; v < p.length; v += 3) {
    for (let c = 0; c < 3; c++) {
      min[c] = Math.min(min[c], p[v + c]);
      max[c] = Math.max(max[c], p[v + c]);
    }
  }
  return { min, max };
}

type SceneAabb = { min: number[]; max: number[]; size: number[] };

/** Sketch 4 of the document, REPLANTED on the document's own Right plane.
 *  In the document it sits on an implicit mate connector, a plane form
 *  FieldDraft has no counterpart for; without the substitution the sketch is
 *  never built and this ticket's translator never runs. Everything else — the
 *  profile, the construction axis, the constraints — is the document verbatim. */
function replantedSketch4(doc: { features: any[] }): any {
  const sketch4 = structuredClone(featureById(doc, "FUYLAXOHGV4RBJ9_2"));
  paramOf(sketch4, "sketchPlane").queries = structuredClone(
    paramOf(featureById(doc, "FeniPjG8I3vr6X2_0"), "sketchPlane").queries,
  );
  return sketch4;
}

/** The revolved body must be a SPHERE of the fixture's radius: 2·R across on
 *  every axis, solid at the centre, and hollow just outside R in all six
 *  directions — which a ring, a half turn or a lathe around the wrong line all
 *  fail. The centre comes from the AABB, since the substituted plane decides
 *  where the sketch origin lands. */
function assertSphere(k: WasmKernel, bb: SceneAabb): void {
  const r = SPHERE_R_MM;
  for (let c = 0; c < 3; c++) {
    expect(bb.size[c], `axis ${c} of ${JSON.stringify(bb)}`).toBeCloseTo(2 * r, 1);
  }
  const centre = [0, 1, 2].map((c) => (bb.min[c] + bb.max[c]) / 2);
  const solid = (p: number[]) => k.bodySdfEval(0, p[0], p[1], p[2]) < 0;
  expect(solid(centre), "sphere centre").toBe(true);
  for (const d of [
    [1, 0, 0],
    [-1, 0, 0],
    [0, 1, 0],
    [0, -1, 0],
    [0, 0, 1],
    [0, 0, -1],
  ]) {
    const at = (f: number) => centre.map((v, c) => v + d[c] * r * f);
    expect(solid(at(0.8)), `inside ${d}`).toBe(true);
    expect(solid(at(1.2)), `outside ${d}`).toBe(false);
  }
}

describe("Onshape import — #416 against the Onshape ground truth", () => {
  let kernel: SuiteKernel;
  let result: Awaited<ReturnType<typeof importOnshapeFeatures>>;

  beforeAll(async () => {
    kernel = wrapImportKernel(new WasmKernel());
    result = await importOnshapeFeatures(ticket416(), kernel, {
      sketches3d: sketches3d() as any,
    });
  }, 180_000);

  it("places EVERY sketch that sits on an implicit mate connector (#409/#442)", () => {
    // Three of the document's five sketches (2, 4, 6) sit on IMPLICIT MATE
    // CONNECTORS — sub-features of the sketches themselves, which is why the
    // feature-type census of the gap matrix never saw them, and why all three
    // used to fall together with everything drawn on them.
    //
    // All three land now. Sketch 2 hangs on a raw construction line of Sketch 1,
    // Sketch 4 on the cap of Extrude 4, and Sketch 6 — the last one — on the
    // EDGE where a cap meets a wall, which the kernel names
    // `qCommonEdge(a, b)`. `resolve_mate_anchor` had a match arm for `qEdge` and
    // `qEdgeBetween` and fell through to the FACE ladder for everything else, so
    // that connector resolved to nothing and took Sketch 6, `Extrude 9` and
    // `Fillet 3` with it (#442).
    const errors = result.errors.join(" | ");
    const planted = result.errors.filter((e) => e.includes("Sketch liegt auf dem Mate-Connector"));
    expect(planted, errors).toEqual([]);
  });

  /**
   * The whole failure surface of this document, by FEATURE NAME — the same
   * both-directions pin the #409 and #419 suites carry, and the one this file
   * was missing (#448). Without it an improvement is as invisible as a
   * regression: `Fillet 3` started resolving the moment this suite got the
   * app's kernel surface and NOTHING here would have said so.
   *
   * `Fillet 3` used to fail on `qCommonEdge(qUnion(qWall(3, 13),
   * qCreatedBy(6, CAP_TOP)), qCreatedBy(12, CAP_BOTTOM/TOP))` — two messages,
   * both gone. It hangs off `Sketch 6`, which sits on the mate connector
   * `qCommonEdge(qCreatedBy(3, CAP_TOP), qWall(3, 13))`; this suite's old
   * hand-built double omitted `resolvedMateConnectors`, so that plane never
   * resolved on the web side and the two features under it came out in the
   * wrong place — near enough to build, far enough for the blend edge to miss.
   *
   * The three that remain are all edge NAMING, none of them geometry:
   *  - `Chamfer 2`: a fragment of an edge a boolean SPLIT.
   *  - `Fillet 2`: an edge a boolean MERGE created, plus two references to a
   *    feature this document never imports.
   *  - `Fillet 5`: `qCommonEdge(qWall(19, 4), qWall(22, 4))` binds nothing.
   */
  it("fails only on the three edge names that are still open (#448)", () => {
    const failed = [...new Set(result.errors.map((e) => e.split(":")[0]))].sort();
    expect(failed, `import errors:\n${result.errors.join("\n")}`).toEqual([
      "Chamfer 2",
      "Fillet 2",
      "Fillet 5",
    ]);
  });

  /**
   * Sketch 2 is the ROOT of the gap, and its connector needs a SECOND kernel
   * capability beyond the plane itself: it hangs off an endpoint of a
   * construction line of Sketch 1, i.e. raw sketch geometry that never enters
   * the B-Rep. `resolve_mate_anchor` takes `qFace`/`qEdge`/`qVertex`/
   * `qSketchPoint` but no `qSketchLine`, so even the CONNECTOR cannot be placed
   * — and with Sketch 2 the whole chain (Extrude 3 → Sketch 3 → Extrude 4 →
   * Sketch 4 → Revolve 1) falls, which is why the document ends at 4 of 19
   * features no matter what the web side does.
   */
  it("anchors Sketch 2's connector on the raw construction line it names", () => {
    const calls = callsOf(kernel.raw).filter((c) => c.type === "mate_connector");
    const onLine = calls.filter((c) => /^qSketchLine\(\d+, \d+\)$/.test(c.params.anchor));
    expect(onLine.length, result.errors.join(" | ")).toBe(1);
    // POINT inference on an ENDPOINT: `t` says which end, and a silent
    // fallback to the midpoint would move the whole chain half a line along.
    expect([0, 1]).toContain(onLine[0].params.t);
  });

  /**
   * ONE part, and the two axes the document is now exact on.
   *
   * The document used to import as FOUR bodies spanning 102 × 67 × 105 mm
   * against Onshape's 73 × 34 × 108 (#442). Three translations were wrong and
   * each was measured before it was touched:
   *
   *  - `Extrude 5` extrudes a WALL of `Extrude 2` (an Onshape `SWEPT_FACE`
   *    query), not a sketch region. Read as an unresolvable region it fell back
   *    to the WHOLE of Sketch 1, 32 mm deep — a block that also swallowed
   *    `Extrude 1`'s top cap, which is why `qCreatedBy(3, CAP_TOP)` bound
   *    nothing for the mate connector and three fillets.
   *  - three of `Extrude 1`'s four region queries name two curves the sketch no
   *    longer has, and the whole query was refused over them.
   *  - `Sketch 3` (two concentric circles on a trapezoidal cap) landed 60 mm
   *    off, so `Extrude 4` and the revolve on it became bodies of their own and
   *    `Fillet 4` never found the edge between them.
   *
   * `Extrude 1`'s walls carry their SKETCH ENTITIES since #445: the union of
   * its three regions runs along two cut circles, the arrangement kept those
   * exact instead of polygonising them, and the sweep's 59 positional subs
   * became eleven entity-named walls (2, 7, 10, 13, 16, 19×2, 21, 23, 50, 53).
   * `qWall(3, 13)` therefore binds the wall of sketch curve 13 — the profile's
   * lower long line at v = −65 — where it used to bind the 13th wall in contour
   * order, a facet of the r = 4.5 mm circle. That is what makes the Y axis
   * exact below.
   */
  it("builds ONE part, where it used to build four", () => {
    expect(kernel.raw.bodyCount()).toBe(GROUND_TRUTH.parts);
    // FIVE of the six bounds, to better than 0.09 mm. Three of them are the
    // ones no feature of the document overshoots: the far end of `Extrude 3`,
    // the top of Sketch 1, and — since #445 — the bottom of its profile
    // rectangle. `y min` was −33.17 while `Extrude 9` hung off a mis-named
    // wall; it is −26.70 against Onshape's −26.77 now.
    //
    // `x max` and `z min` JOINED them in #448, and no code changed for it —
    // only this suite's kernel double did. `Extrude 9`'s 8.17 mm wedge grows
    // out of `Extrude 4`'s cap, and its sketch sits on a mate connector. The
    // old hand-built double had no `resolvedMateConnectors`, so
    // `resolvePlaneFrame` fell through to the FACE under that connector and
    // synthesised a basis from its normal — a frame turned 90° inside its own
    // plane. The wedge therefore pointed +x (x max 11.17 against Onshape's
    // 3.07) instead of −z (z min −65 against −73.08). With the connector's own
    // CSYS, which is what the app has always passed, it points where Onshape
    // has it: x max 3.000, z min −73.169. Suppressing `Extrude 9` is what
    // moves `z min` back to −65 — measured, so the attribution is not a story.
    const bb = kernel.sceneAabb!() as { min: number[]; max: number[] };
    expect(bb.min[0], "x min").toBeCloseTo(GROUND_TRUTH.x[0], 0);
    expect(bb.max[0], "x max").toBeCloseTo(GROUND_TRUTH.x[1], 0);
    expect(bb.min[1], "y min").toBeCloseTo(GROUND_TRUTH.y[0], 0);
    expect(bb.min[2], "z min").toBeCloseTo(GROUND_TRUTH.z[0], 0);
    expect(bb.max[2], "z max").toBeCloseTo(GROUND_TRUTH.z[1], 0);
  });

  /**
   * The full extent, pinned as a FAILING test on purpose. ONE bound is left
   * over, and #448 changed WHICH one — the answer this comment gave until now
   * was measured on a kernel double the app never had.
   *
   * What it used to say: the mate connector under `Sketch 6` was turned 90°
   * inside its own plane, because an edge anchor takes its secondary axis from
   * whichever adjacent face has the smaller branch word. That reading came
   * from a double WITHOUT `resolvedMateConnectors`, where the sketch plane was
   * never the connector's CSYS at all but the face beneath it, with a basis
   * synthesised from the face normal. Give this suite the surface the app
   * passes and the rotation is simply gone: `x max` 11.17 → 3.000 (Onshape
   * 3.07) and `z min` −65 → −73.169 (Onshape −73.08), both folded into the
   * test above. Anyone reading this to work on #447 should start from that:
   * the 90° in-plane skew reproduced HERE was the measurement, not the kernel.
   *
   * What is actually left is `y max`: 6.500 against Onshape's 7.1292, short by
   * 0.63 mm. It is NOT one missing solid — suppressing any single feature of
   * the import leaves `y max` at 6.500, so several bodies share that plane and
   * nothing we build reaches over it. The three features that still fail are
   * all edge names (`Chamfer 2`, `Fillet 2`, `Fillet 5`, see the census
   * above), and a blend on a concave edge is the one thing that ADDS material;
   * that is the open lead, unproven. The shortfall predates #448 — the poorer
   * double measured the same 6.500.
   *
   * `it.fails` keeps that honest in both directions: the day the last 0.63 mm
   * arrives, this starts failing and forces someone to fold it into the test
   * above, as x max and z min just were.
   */
  it.fails("matches the ground-truth AABB (open: y max, 0.63 mm short)", () => {
    const bb = kernel.sceneAabb!() as { min: number[]; max: number[] };
    for (const [c, [lo, hi]] of [GROUND_TRUTH.x, GROUND_TRUTH.y, GROUND_TRUTH.z].entries()) {
      expect(bb.min[c], `axis ${c} min`).toBeCloseTo(lo, 0);
      expect(bb.max[c], `axis ${c} max`).toBeCloseTo(hi, 0);
    }
  });
});

/**
 * The revolve translation, proven on the REAL document and the REAL kernel.
 *
 * Sketch 4 — which carries both the revolve's profile and its axis — is
 * REPLANTED on the document's own Right plane, by copying the plane query of
 * Sketch 1. In the document it sits on an implicit mate connector, a plane form
 * FieldDraft has no counterpart for; without the substitution the sketch never
 * gets built and this ticket's translator never runs. Everything else is the
 * document verbatim: the same profile region query, the same construction-line
 * axis, the same FULL revolve type.
 *
 * What that must produce is fixed by the fixture GEOMETRY, not by our import: a
 * half-disc of R = 4.94 mm spun around a line through its centre is a sphere of
 * R = 4.94 mm.
 */
describe("Onshape import — #416 revolve end-to-end (real kernel)", () => {
  let kernel: SuiteKernel;
  let result: Awaited<ReturnType<typeof importOnshapeFeatures>>;

  beforeAll(async () => {
    const doc = ticket416();
    const sketch4 = replantedSketch4(doc);
    const revolve = structuredClone(featureById(doc, "F0K5aKkQUQzivLM_2"));
    kernel = wrapImportKernel(new WasmKernel());
    result = await importOnshapeFeatures({ features: [sketch4, revolve] }, kernel);
  }, 180_000);

  it("translates the document's revolve into a full turn around its construction line", () => {
    const revolves = callsOf(kernel.raw).filter((c) => c.type === "revolve");
    expect(revolves.length, result.errors.join(" | ")).toBe(1);
    // The axis is the sketch LINE the document drew as construction geometry —
    // the only axis form `RevolveDef` accepts besides a B-Rep edge.
    expect(revolves[0].params.axis).toMatch(/^qSketchLine\(\d+, \d+\)$/);
    // It regenerated: an unresolvable axis or profile makes the kernel drop the
    // body with a warning, so nothing here may be papered over.
    expect(regenErrors(kernel.raw).get(String(revolves[0].id))).toBeUndefined();
    expect(kernel.raw.bodyCount()).toBe(1);
  });

  /**
   * The revolve's GEOMETRY, pinned as a FAILING test on purpose — the cause is
   * a KERNEL bug in the axis handling, not the translation above.
   *
   * `revolve.rs` aligns the picked axis onto the local `x = 0` line and then
   * checks whether the profile landed on the NEGATIVE side, because the revolve
   * SDF folds every query to a radius ≥ 0 and would never reach it there. That
   * check is an exact `profile.aabb().max[0] <= 0.0`. This document draws its
   * construction axis DOWNWARDS, so the alignment rotates the profile by 180°
   * onto −x — where its maximum is not 0 but +2.2e-7 mm, the numerical spill of
   * the arc endpoint sitting on the axis. The exact test misses by that hair,
   * the profile is never flipped back, and the revolve yields a body whose AABB
   * is right (the placement is) but whose field is empty: `bodySdfEval` is
   * POSITIVE everywhere, and the mesher ships no triangles.
   *
   * The control below proves the diagnosis: the SAME segment, drawn upwards
   * (start/end swapped, geometry untouched), gives the correct sphere. Nothing
   * about that is fixable from the importer — a `qSketchLine` query carries no
   * direction, so the web side cannot choose one.
   */
  it.fails("sweeps the half-disc into a SPHERE (open: kernel axis-direction flip)", () => {
    assertSphere(kernel.raw, kernel.sceneAabb!() as SceneAabb);
  }, 180_000);

});

/**
 * Control for the pinned kernel bug — and the render-path proof for this
 * ticket. Identical to the run above except for ONE thing: the axis entity's
 * `startParam` and `endParam` are swapped, so the construction line describes
 * the very same segment drawn in the opposite direction. Onshape's own document
 * draws it downwards; upwards is what the kernel's flip check needs.
 */
describe("Onshape import — #416 revolve with the axis line drawn upwards", () => {
  let kernel: SuiteKernel;
  let result: Awaited<ReturnType<typeof importOnshapeFeatures>>;

  beforeAll(async () => {
    const doc = ticket416();
    const sketch4 = replantedSketch4(doc);
    const axis = sketch4.entities
      .map((e: any) => e.message ?? e)
      .find((e: any) => e.entityId === "nsFmMv4kYM6f");
    [axis.startParam, axis.endParam] = [axis.endParam, axis.startParam];
    const revolve = structuredClone(featureById(doc, "F0K5aKkQUQzivLM_2"));
    kernel = wrapImportKernel(new WasmKernel());
    result = await importOnshapeFeatures({ features: [sketch4, revolve] }, kernel);
  }, 180_000);

  it("sweeps the half-disc into a SPHERE of the fixture's radius", () => {
    expect(result.errors, result.errors.join(" | ")).toEqual([]);
    assertSphere(kernel.raw, kernel.sceneAabb!() as SceneAabb);
  }, 180_000);

  it("renders through the app's mesher-worker path, at the app's resolutions", () => {
    const main = new WasmKernel();
    main.setSyncMeshing(false);
    main.setBrepSuppressed(true);
    main.loadJson(kernel.raw.toJson());
    const documentJson = main.toJson();

    const FULL_RES = 64;
    const CONTEXT_RES = 32;
    const COARSE_RES = Math.max(8, Math.floor(FULL_RES / 2));

    for (const [pass, res] of [
      ["initial", COARSE_RES],
      ["followup", FULL_RES],
    ] as const) {
      const worker = replayMesherWorker({
        type: "rebuild",
        requestId: 1,
        documentJson,
        activeMeshResolution: res,
        contextMeshResolution: CONTEXT_RES,
        activeBodyId: 0,
        meshAlgorithm: "mdc",
      });
      expect(worker.bodyCount(), `${pass} pass: body count`).toBe(1);
      expect(
        worker.bodyTriangleCount(0),
        `${pass} pass: the worker shipped an EMPTY mesh — blank viewport`,
      ).toBeGreaterThan(0);

      // Non-empty is not enough: the mesh must span the solid, not a few stray
      // triangles from a lucky lattice plane. The tolerance is the DC CELL — a
      // contoured surface cannot be localised finer than the grid it was
      // sampled on.
      const bb = meshAabb(worker, 0);
      const scene = kernel.sceneAabb!() as { min: number[]; max: number[]; size: number[] };
      const tol = (2 * Math.max(...scene.size)) / res;
      for (let c = 0; c < 3; c++) {
        expect(
          Math.abs(bb.min[c] - scene.min[c]),
          `${pass} pass: mesh ${JSON.stringify(bb)} vs scene ${JSON.stringify(scene)}`,
        ).toBeLessThan(tol);
        expect(Math.abs(bb.max[c] - scene.max[c])).toBeLessThan(tol);
      }
    }
  }, 180_000);
});
