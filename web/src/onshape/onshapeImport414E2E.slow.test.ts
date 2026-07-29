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
 *   POST …/featurescript, evBox3d(tight: true)               → the AABB below
 *
 * NOT `GET …/partid/JHD/boundingboxes`: that endpoint answers Onshape's LOOSE
 * box, which on this document is 0.0547 mm too wide in `x min` and 0.1285 mm
 * in `y min` (#449 — see `onshapeImport416E2E.slow.test.ts`, where the same
 * mistake was worth 0.63 mm and three tickets of hunting).
 *
 * Needs a current `just build-wasm` (the stale-wasm trap).
 */
import { beforeAll, describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import initWasm, { Kernel as WasmKernel } from "../wasm/fielddraft_kernel";
import { importOnshapeFeatures, type ImportKernel } from "./onshapeImport";
import {
  wrapImportKernel,
  regenErrors,
  regenCensus,
  type SuiteKernel,
} from "./importKernelDouble";
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

/** Onshapes eigene 3D-Antwort für dieselben Sketches — die Quelle, aus der der
 *  Importer Onshapes Sketch-Frame misst (#415). Der Live-Import zieht sie über
 *  den Go-Proxy mit; hier kommt sie aus der Fixture, damit der Test kein Netz
 *  braucht. Ohne sie landet Sketch 3 dieses Dokuments (flächengebunden) um 90°
 *  verdreht und y reichte bis −104 statt −70,1 mm. */
const sketches3d = (): unknown =>
  JSON.parse(readFileSync(join(__dirname, "fixtures", "onshape-414-sketches3d.json"), "utf8"));

/**
 * Onshape's own answer for this document, in mm (the API reports metres).
 * ONE solid part, `JHD`. This is the TIGHT box; both were read in one call:
 *
 *   loose  min (−65.0547, −70.1285, −50)  max (65, 0, 50)
 *   TIGHT  min (−65.0000, −70.0000, −50)  max (65, 0, 50)
 *
 * Only the two bounds that touch this part's blends carry any slack, and both
 * of them are `min`s — which is why the full-AABB pin below used to fail while
 * the `z` pin passed. Nothing about the import changed with #449.
 */
const GROUND_TRUTH = {
  parts: 1,
  x: [-65.0, 65.0],
  y: [-70.0, 0.0],
  z: [-50.0, 50.0],
};


interface FeatureCall {
  id: string | number;
  type: string;
  params: Record<string, any>;
}

const callsOf = (k: WasmKernel): FeatureCall[] =>
  JSON.parse(k.toJson()).feature_calls as FeatureCall[];

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
  let kernel: SuiteKernel;
  let result: Awaited<ReturnType<typeof importOnshapeFeatures>>;

  beforeAll(async () => {
    kernel = wrapImportKernel(new WasmKernel());
    result = await importOnshapeFeatures(ticket414(), kernel, {
      sketches3d: sketches3d() as any,
    });
  }, 180_000);

  it("builds ONE solid, as Onshape does (part JHD)", () => {
    expect(kernel.raw.bodyCount(), result.errors.join(" | ")).toBe(GROUND_TRUTH.parts);
  });

  /**
   * The SECOND failure surface, pinned in both directions (#449).
   *
   * `result.errors` says what the importer refused to WRITE; this says what
   * the KERNEL refused to BUILD out of what was written, and until #449 no
   * suite asked for the whole list — only ever for one named feature at a
   * time. The two surfaces are disjoint, which is how #416's two rejected
   * chamfers stayed invisible for three tickets.
   *
   * EMPTY here, measured.
   */
  it("regenerates EVERY feature it wrote (#449)", () => {
    expect(regenCensus(kernel.raw, result, ticket414())).toEqual([]);
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
   * The FULL extent, and it is EXACT — all six bounds to better than 5 µm.
   * This was an `it.fails` until #449, and no geometry moved to green it.
   *
   * The history is worth keeping, because the comment on the failing pin was
   * wrong in a way that took a measurement to see. It read: „X and Y still
   * overshoot… the residual ~2.8 mm in x and y", and blamed the region-profile
   * provenance gap of docs/SKETCH-REGION-IMPRINT-PLAN.md §5. The 2.8 mm was
   * real once — it is not now, and nobody noticed it go, because the pin kept
   * failing for a completely different reason: the ground truth was Onshape's
   * LOOSE box (x min −65.0547, y min −70.1285) and the tolerance here is
   * 0.05 mm. The import had been landing on Onshape's tight −65.0000 /
   * −70.0000 exactly, and the suite called that a failure.
   *
   * The provenance gap itself is NOT closed — it is what the twenty-odd
   * failing edge names below are — it just costs this document no extent.
   *
   * Asserted at 5 µm rather than the old 0.05 mm: the numbers are exact, and a
   * gate two orders of magnitude wider than the quantity it guards cannot tell
   * exact from nearly. That is the whole lesson of #449 in one line.
   */
  it("matches the FULL ground-truth AABB, exactly", () => {
    const bb = kernel.sceneAabb!() as { min: number[]; max: number[] };
    const where = `scene ${JSON.stringify(bb)}`;
    expect(bb.min[0], where).toBeCloseTo(GROUND_TRUTH.x[0], 2);
    expect(bb.max[0], where).toBeCloseTo(GROUND_TRUTH.x[1], 2);
    expect(bb.min[1], where).toBeCloseTo(GROUND_TRUTH.y[0], 2);
    expect(bb.max[1], where).toBeCloseTo(GROUND_TRUTH.y[1], 2);
    expect(bb.min[2], where).toBeCloseTo(GROUND_TRUTH.z[0], 2);
    expect(bb.max[2], where).toBeCloseTo(GROUND_TRUTH.z[1], 2);
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
  let kernel: SuiteKernel;
  let result: Awaited<ReturnType<typeof importOnshapeFeatures>>;

  beforeAll(async () => {
    kernel = wrapImportKernel(new WasmKernel());
    delete (kernel as Partial<ImportKernel>).regionQueryAtPoint;
    delete (kernel as Partial<ImportKernel>).listSketchRegions;
    result = await importOnshapeFeatures(ticket414(), kernel, {
      sketches3d: sketches3d() as any,
    });
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
