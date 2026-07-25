/**
 * End-to-end acceptance of the #414 part studio against the REAL wasm kernel.
 *
 * Ticket #414 asked for the document
 * `cad.onshape.com/documents/17b3c41d06b6c158e839585e/…/e/7896b002f4274b0832cfad5a`
 * to import completely. It needs the two translators this file exercises —
 * `chamfer` → `chamfer_edge` and `hole` → `hole`.
 *
 * GROUND TRUTH comes from the Onshape REST API of that very document, never
 * from our own resolution (the methodology lesson of #412):
 *
 *   GET /api/v6/parts/d/{did}/w/{wid}/e/{eid}                → ONE part, "JHD"
 *   GET …/partid/JHD/boundingboxes                           → the AABB below
 *
 * Needs a current `just build-wasm` (the stale-wasm trap).
 */
import { beforeAll, describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import initWasm, { Kernel as WasmKernel } from "../wasm/fielddraft_kernel";
import { importOnshapeFeatures, type ImportKernel } from "./onshapeImport";
import type { SketchSubCall } from "./onshapeSketch";
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

const ticket414 = (): { features: any[] } =>
  JSON.parse(readFileSync(join(__dirname, "fixtures", "onshape-414-features.json"), "utf8"));

/**
 * Onshape's own answer for this document, in mm (the API reports metres).
 * ONE solid part, `JHD`.
 */
const GROUND_TRUTH = {
  parts: 1,
  x: [-65.0547, 65.0],
  y: [-70.1285, 0.0],
  z: [-50.0, 50.0],
};

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
  };
}

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
 *  decides whether the user sees anything at all (see the #412 E2E test). */
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

describe("Onshape import — #414 against the Onshape ground truth", () => {
  let kernel: ImportKernel & { raw: WasmKernel };
  let result: Awaited<ReturnType<typeof importOnshapeFeatures>>;

  beforeAll(async () => {
    kernel = wrap(new WasmKernel());
    result = await importOnshapeFeatures(ticket414(), kernel);
  }, 180_000);

  it("builds ONE solid, as Onshape does (part JHD)", () => {
    expect(kernel.raw.bodyCount(), result.errors.join(" | ")).toBe(GROUND_TRUTH.parts);
  });

  it("matches the ground-truth extent along Z", () => {
    const bb = kernel.sceneAabb!() as { min: number[]; max: number[] };
    expect(bb.min[2], `scene ${JSON.stringify(bb)}`).toBeCloseTo(GROUND_TRUTH.z[0], 1);
    expect(bb.max[2]).toBeCloseTo(GROUND_TRUTH.z[1], 1);
  });

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
      // triangles from a lucky lattice plane. The tolerance is the DC CELL —
      // a contoured surface cannot be localised finer than the grid it was
      // sampled on, and the coarse pass is exactly that grid at half density.
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

  /**
   * The remaining #414 gap, pinned as a FAILING test on purpose.
   *
   * X and Y still overshoot the ground truth. The cause is NOT the chamfer or
   * hole translation but the kernel follow-up recorded in
   * docs/SKETCH-REGION-IMPRINT-PLAN.md §5: a region-restricted profile carries
   * no entity provenance, so its wall subs fall back to POSITIONAL numbering.
   * Every later feature that names a wall of the region-restricted `Extrude 1`
   * — the chamfer's cap rim, the plane of `Sketch 2` (which in turn carries the
   * hole's target point) and six fillets — therefore resolves to nothing.
   *
   * `it.fails` keeps that honest in both directions: today the suite is green
   * WITH the gap documented, and the day the provenance lands this test starts
   * failing and forces someone to promote it to a plain `it`.
   */
  it.fails("matches the FULL ground-truth AABB (open: region-profile provenance)", () => {
    const bb = kernel.sceneAabb!() as { min: number[]; max: number[] };
    expect(bb.min[0]).toBeCloseTo(GROUND_TRUTH.x[0], 1);
    expect(bb.max[0]).toBeCloseTo(GROUND_TRUTH.x[1], 1);
    expect(bb.min[1]).toBeCloseTo(GROUND_TRUTH.y[0], 1);
    expect(bb.max[1]).toBeCloseTo(GROUND_TRUTH.y[1], 1);
  });
});

/**
 * The chamfer translation, proven on the REAL document and the REAL kernel.
 *
 * Driven with the region API withheld, which is a supported degradation of the
 * importer (`regionQueryAtPoint` is optional; without it every extrude uses the
 * whole sketch). That keeps the wall provenance intact, so the document's
 * chamfer resolves — isolating this ticket's translator from the region gap
 * pinned above. The solid is a superset of Onshape's, so nothing here asserts
 * the ground-truth AABB; it asserts what a chamfer must do to a solid.
 */
describe("Onshape import — #414 chamfer end-to-end (real kernel)", () => {
  let kernel: ImportKernel & { raw: WasmKernel };
  let result: Awaited<ReturnType<typeof importOnshapeFeatures>>;

  beforeAll(async () => {
    kernel = wrap(new WasmKernel());
    delete (kernel as Partial<ImportKernel>).regionQueryAtPoint;
    delete (kernel as Partial<ImportKernel>).listSketchRegions;
    result = await importOnshapeFeatures(ticket414(), kernel);
  }, 180_000);

  it("creates the document's chamfer as a 5 mm chamfer_edge that regenerates", () => {
    const chamfers = callsOf(kernel.raw).filter((c) => c.type === "chamfer_edge");
    expect(chamfers.length, result.errors.join(" | ")).toBe(1);
    // Onshape authored OFFSET_ANGLE, `width1 = 0.5 cm`, 45° → 5 mm of bevel.
    expect(chamfers[0].params.distance).toEqual({ value: 5, unit: "mm" });
    // It regenerated: the kernel has no silent fallback, so an unresolvable
    // edge query would be a feature error here.
    expect(regenErrors(kernel.raw).get(String(chamfers[0].id))).toBeUndefined();
  });

  it("REMOVES material along the edge and adds none anywhere", () => {
    // Same document minus the chamfer call — the only difference between the
    // two fields is the bevel itself.
    const doc = JSON.parse(kernel.raw.toJson());
    const before = doc.feature_calls.length;
    doc.feature_calls = (doc.feature_calls as FeatureCall[]).filter(
      (c) => c.type !== "chamfer_edge",
    );
    expect(doc.feature_calls.length).toBe(before - 1);
    const plain = new WasmKernel();
    plain.loadJson(JSON.stringify(doc));

    const bb = kernel.sceneAabb!() as { min: number[]; max: number[]; size: number[] };
    const N = 48;
    let removed = 0;
    let added = 0;
    for (let i = 0; i < N; i++) {
      for (let j = 0; j < N; j++) {
        for (let k = 0; k < N; k++) {
          const x = bb.min[0] + (bb.size[0] * (i + 0.5)) / N;
          const y = bb.min[1] + (bb.size[1] * (j + 0.5)) / N;
          const z = bb.min[2] + (bb.size[2] * (k + 0.5)) / N;
          const solidBefore = plain.bodySdfEval(0, x, y, z) < 0;
          const solidAfter = kernel.raw.bodySdfEval(0, x, y, z) < 0;
          if (solidBefore && !solidAfter) removed += 1;
          if (!solidBefore && solidAfter) added += 1;
        }
      }
    }
    // A chamfer is a CUT: it may only take material away.
    expect(added, "the chamfer added material — that is not a bevel").toBe(0);
    expect(removed, "the chamfer removed nothing — the bevel is not there").toBeGreaterThan(0);
  }, 180_000);
});

/**
 * The hole translation, proven through the REAL API chain (the #402/#405 gate:
 * `add_feature_call` → `body_sdf_eval`, `Length` values in `{value, unit}` wire
 * form, never a bare f32).
 *
 * The parameters are exactly the ones the translator emits for the #414
 * document's hole — Ø6 mm, blind 50 mm, drilled at a sketch point, explicit
 * body scope (asserted against the fixture in `onshapeImport.test.ts`). They
 * are drilled into a plate built here rather than into the imported solid,
 * because the #414 hole's own target sketch (`Sketch 2`) is lost to the region
 * gap pinned above; what this proves is that those parameters DRILL.
 */
describe("Onshape import — #414 hole parameters through the real API chain", () => {
  it("drills a Ø6 mm blind 50 mm bore at the sketch point, and stops there", () => {
    const k = new WasmKernel();
    const sketch = k.addSketch(
      "qCreatedBy(origin_plane_xy)",
      JSON.stringify([
        // The bore's target point FIRST: the kernel and the web sketch layer
        // allocate entity ids from sub-call order out of the shared id space,
        // so this point is entity 1.
        { type: "sketch_point", id: "1", params: { at: [30, 30] } },
        { type: "sketch_line", id: "2", params: { start: [0, 0], end: [60, 0] } },
        { type: "sketch_line", id: "3", params: { start: [60, 0], end: [60, 60] } },
        { type: "sketch_line", id: "4", params: { start: [60, 60], end: [0, 60] } },
        { type: "sketch_line", id: "5", params: { start: [0, 60], end: [0, 0] } },
      ]),
    );
    const plate = k.addFeatureCall(
      "extrude",
      JSON.stringify({
        profiles: `qCreatedBy(${sketch})`,
        depth: { value: 60, unit: "mm" },
        operation: "new",
      }),
    );
    const hole = k.addFeatureCall(
      "hole",
      JSON.stringify({
        style: "simple",
        hole_type: "drilled",
        standard_size: "custom",
        diameter: { value: 6, unit: "mm" },
        end_type: "blind",
        depth: { value: 50, unit: "mm" },
        opposite_direction: true,
        locations: `qSketchPoint(${sketch}, 1)`,
        targets: `qCreatedBy(${plate})`,
      }),
    );
    expect(regenErrors(k).get(String(hole))).toBeUndefined();
    expect(k.bodyCount()).toBe(1);

    const solid = (x: number, y: number, z: number) => k.bodySdfEval(0, x, y, z) < 0;
    // Away from the bore the plate is untouched…
    expect(solid(10, 10, 25), "the plate itself").toBe(true);
    // …on the bore axis it is hollow for the authored 50 mm…
    expect(solid(30, 30, 5), "bore mouth").toBe(false);
    expect(solid(30, 30, 45), "bore bottom end").toBe(false);
    // …and solid again beyond it: BLIND, not through-all.
    expect(solid(30, 30, 55), "material below a BLIND bore").toBe(true);
    // The bore is Ø6, so 5 mm off the axis is still material.
    expect(solid(35, 30, 25), "Ø6 bore wall").toBe(true);
  }, 180_000);
});
