/**
 * End-to-end acceptance of the Onshape import against the REAL wasm
 * kernel (Onshape-Import Schritt 5) — the Kabelbox part studio.
 *
 * THE acceptance criterion of the whole import project: all SEVEN sketches
 * land, each on the face Onshape drew it on, and the references are
 * PARAMETRIC — change the base body and the planes travel with it.
 *
 * Three of them sit on a SIDE of the rounded-rectangle base, which only
 * became possible with the constructive tangential seams
 * (`brep::tangential_seams`): before those, a line and its fillet arc fused
 * into ONE lateral face and "the left side" did not exist as a face at all.
 *
 * Semantics note: a sketch's plane query is resolved MID-HISTORY (against
 * the body as of that feature), exactly like Onshape's rollback state — so
 * the acceptance signal is "the sketch feature regenerates without an
 * error", not "its query still binds at the end of the document" (a later
 * cut may legitimately consume the face it was drawn on). The kernel has NO
 * silent fallback: an unresolvable plane is a hard feature error.
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
import { resolvePlaneFrame } from "../sketchPlaneFrame";

const wasmDir = join(__dirname, "..", "wasm");

/** WASM linear memory — needed to read the worker's mesh buffers by pointer. */
let wasmMemory: WebAssembly.Memory;

beforeAll(async () => {
  const exports = (await initWasm({
    module_or_path: readFileSync(join(wasmDir, "fielddraft_kernel_bg.wasm")),
  })) as unknown as { memory: WebAssembly.Memory };
  wasmMemory = exports.memory;
});

const kabelbox = () =>
  JSON.parse(readFileSync(join(__dirname, "fixtures", "kabelbox-features.json"), "utf8"));

const ticket412 = () =>
  JSON.parse(readFileSync(join(__dirname, "fixtures", "onshape-412-features.json"), "utf8"));

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
    // The geometry readers the end-condition translation measures against —
    // the same ones the app hands the importer.
    sceneAabb: () => JSON.parse(k.sceneAabb()),
    bodyCount: () => k.bodyCount(),
    bodyBrepFaces: (i: number) => JSON.parse(k.bodyBrepFaces(i)),
    bodyPlacement: (i: number) => JSON.parse(k.bodyPlacement(i)),
    regionQueryAtPoint: (sketchId: number, u: number, v: number) =>
      k.regionQueryAtPoint(sketchId, u, v),
    // The imprint-face list (#412) — ranks candidate faces by area when an
    // Onshape region selection is resolved to an atomic face.
    listSketchRegions: (sketchId: number, resolution: number) =>
      k.listSketchRegions(sketchId, resolution),
    // Exercise the #410 batch meshing deferral end-to-end: the SDF verification
    // below then proves the deferred-mesh result is geometrically identical.
    setSyncMeshing: (enabled: boolean) => k.setSyncMeshing(enabled),
  };
}

interface FeatureCall {
  id: string | number;
  type: string;
  params: Record<string, any>;
}

const sketchCalls = (k: WasmKernel): FeatureCall[] =>
  (JSON.parse(k.toJson()).feature_calls as FeatureCall[]).filter((c) => c.type === "sketch");

/** Per-feature regeneration errors, keyed by feature id. */
function regenErrors(k: WasmKernel): Map<string, string> {
  const rows = JSON.parse(k.listFeatures()) as { id: string | number; error?: string | null }[];
  const out = new Map<string, string>();
  for (const r of rows) if (r.error) out.set(String(r.id), r.error);
  return out;
}

/**
 * Replay of `mesher.worker.ts`'s `handleRebuild` on a fresh kernel: the
 * SAME call sequence, in the same order, driven by the same request
 * object the store posts. Anything the app's render mesh depends on that
 * a bare `loadJson` does not (meshing/B-Rep flags, LOD resolutions, mesh
 * algorithm) is therefore exercised here — the store→worker path is the
 * one that showed an empty viewport while `loadJson` alone looked fine.
 */
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

/** The plane-frame view the sketch overlay uses (kernel-backed). */
function frameKernel(k: WasmKernel) {
  return {
    bodyCount: () => k.bodyCount(),
    bodyBrepFaces: (i: number) => JSON.parse(k.bodyBrepFaces(i)),
    bodyPlacement: (i: number) => JSON.parse(k.bodyPlacement(i)),
    resolveQuery: (q: string) => {
      const parsed = JSON.parse(k.resolveQuery(q)) as unknown;
      return Array.isArray(parsed) ? (parsed as { kind: string; idHash: string }[]) : [];
    },
  };
}

describe("Onshape import — Kabelbox end-to-end (real kernel)", () => {
  // The import weaves the body after every feature (the uniqueness gate),
  // so it is the expensive part — run it ONCE for all three assertions.
  let kernel: ImportKernel & { raw: WasmKernel };
  let result: Awaited<ReturnType<typeof importOnshapeFeatures>>;

  beforeAll(async () => {
    kernel = wrap(new WasmKernel());
    result = await importOnshapeFeatures(kabelbox(), kernel);
  }, 180_000);

  it("imports ALL SEVEN sketches, each on its own resolved face", () => {
    const sketches = sketchCalls(kernel.raw);
    expect(sketches.length, `errors: ${result.errors.join(" | ")}`).toBe(7);

    // Every one regenerates — i.e. its plane query resolved to a real face
    // mid-history. The kernel has no silent fallback, so this IS the proof.
    const errors = regenErrors(kernel.raw);
    for (const s of sketches) {
      expect(errors.get(String(s.id)), `sketch #${s.id} regen`).toBeUndefined();
    }

    const planes = sketches.map((s) => s.params.plane as string);
    // Two datums (Onshape's default planes), two extrude caps, and three on
    // the SIDES of the rounded base — the ones the tangential seams unlocked.
    expect(planes.filter((p) => p.startsWith("qCreatedBy(origin_plane_")).length).toBe(2);
    expect(planes.filter((p) => /qCreatedBy\(\d+, CAP_/.test(p)).length).toBe(2);
    expect(planes.filter((p) => p.includes("qWall(")).length).toBe(3);
  });

  it("places a sketch on a SIDE of the rounded base — the tangential-seam payoff", () => {
    // Sketch 5 sits on the wall swept from the base profile's left LINE. That
    // face only exists because the line's plane and its fillet cylinder are
    // separate faces now, joined by a constructive tangential seam.
    const planes = sketchCalls(kernel.raw).map((s) => s.params.plane as string);
    const wall = planes.find((p) => /^qWall\(\d+, \d+\)$/.test(p));
    expect(wall, `no plain wall plane among ${planes.join(", ")}`).toBeTruthy();
    const hits = kernel.resolveQuery(wall!);
    expect(hits.length, `${wall} must bind exactly one face`).toBe(1);
    expect(hits[0].kind).toBe("face");

    // And the disambiguated ones (boolean SPLIT of a wall) bind uniquely too.
    for (const p of planes.filter((q) => q.startsWith("qAdjacentTo("))) {
      expect(kernel.resolveQuery(p).length, p).toBe(1);
    }
  });

  it("sketch plane FRAMES follow the base body when its depth changes", { timeout: 180_000 }, () => {
    const facePlanes = sketchCalls(kernel.raw)
      .map((s) => s.params.plane as string)
      .filter((p) => !p.startsWith("qCreatedBy(origin_plane_"));
    expect(facePlanes.length).toBeGreaterThan(0);

    const before = facePlanes.map((p) => resolvePlaneFrame(p, frameKernel(kernel.raw)));
    for (const f of before) expect(f, "plane frame resolves").not.toBeNull();

    // Change the BASE extrude's depth; reload into a fresh kernel.
    const baseOnshapeId = Object.keys(result.idMap).find((k) => k.startsWith("Fz9Ll1"))!;
    const baseFd = String(result.idMap[baseOnshapeId]);
    const doc = JSON.parse(kernel.raw.toJson());
    let touched = false;
    for (const call of doc.feature_calls as FeatureCall[]) {
      if (String(call.id) === baseFd && call.params?.depth) {
        call.params.depth.value = (call.params.depth.value as number) * 0.6;
        touched = true;
      }
    }
    expect(touched, "base extrude carries a depth").toBe(true);

    const k2 = new WasmKernel();
    k2.loadJson(JSON.stringify(doc));

    // Nothing broke, and at least one plane MOVED with the resized body —
    // the reference is parametric, not baked geometry.
    const errors = regenErrors(k2);
    for (const s of sketchCalls(k2)) {
      expect(errors.get(String(s.id)), `sketch #${s.id} after the change`).toBeUndefined();
    }
    let moved = 0;
    facePlanes.forEach((plane, i) => {
      const f = resolvePlaneFrame(plane, frameKernel(k2));
      expect(f, `plane ${plane} after the change`).not.toBeNull();
      const a = before[i]!.origin;
      const b = f!.origin;
      if (Math.hypot(a[0] - b[0], a[1] - b[1], a[2] - b[2]) > 1e-3) moved += 1;
    });
    expect(moved, "at least one plane travels with the resized body").toBeGreaterThan(0);
  });
});

describe("Onshape import — #412 imprint region selection (real kernel)", () => {
  // GROUND TRUTH, read off the Onshape REST API of the source document
  // ("Garagenregal_rechts", parts + bounding boxes, mm). Three parts, all thin
  // plates; the sketch plane maps u→world y, v→world z:
  //   JHD  y −1885.1…614.9   z 1700.2…1722.2
  //   JHH  y −1885.1…−885.1  z  800.2… 822.2
  //   JHL  y   627.6…2114.9  z 1700.0…1722.2
  // The extrude carries FOUR region queries, but one of them names a rectangle
  // side (`oPgfZ6ZnMtct.bottom`) that no longer exists in the sketch — which is
  // why Onshape itself builds three parts, not four. The importer must resolve
  // the three real ones, drop the stale one WITH a warning, and above all not
  // invent the 604×1500 block that the shared left edge also runs along.
  const GROUND_TRUTH = {
    y: [-1885.1, 2114.9],
    z: [800.2, 1722.2],
    /** Inside the block that the old resolver wrongly picked (world mm). */
    emptySpot: [250, -2200, 1500] as [number, number, number],
  };

  let kernel: ImportKernel & { raw: WasmKernel };
  let result: Awaited<ReturnType<typeof importOnshapeFeatures>>;

  beforeAll(async () => {
    kernel = wrap(new WasmKernel());
    result = await importOnshapeFeatures(ticket412(), kernel);
  }, 120_000);

  it("resolves the profile to the three plate faces Onshape really extrudes", () => {
    const extrude = (JSON.parse(kernel.raw.toJson()).feature_calls as FeatureCall[]).find(
      (c) => c.type === "extrude",
    )!;
    const profiles = String(extrude.params.profiles);
    // A concrete multi-region query, NOT the whole-sketch profile.
    const m = profiles.match(/^qRegions\(\d+,\s*\[([\d,\s]+)\]\)$/);
    expect(m, `profiles = ${profiles}`).not.toBeNull();
    const indices = m![1].split(",").map((s) => Number(s.trim()));
    expect(indices.length, `three plate faces: ${profiles}`).toBe(3);

    // Each selected face IS one of the three plates: a thin (≈22 mm) strip
    // whose extent matches a part's bounding box.
    const faces = JSON.parse(kernel.raw.listSketchRegions(1, 128)) as {
      index: number;
      area: number;
      aabb: { min: [number, number]; max: [number, number] };
    }[];
    for (const i of indices) {
      const f = faces.find((r) => r.index === i)!;
      const w = f.aabb.max[0] - f.aabb.min[0];
      const h = f.aabb.max[1] - f.aabb.min[1];
      expect(Math.min(w, h), `face ${i} is a plate, not a block`).toBeLessThan(25);
      expect(Math.max(w, h)).toBeGreaterThan(900);
    }
  });

  it("warns that the stale fourth selection was dropped", () => {
    const warn = result.warnings.find((w) => w.includes("Onshape wählte"));
    expect(warn, result.warnings.join(" | ")).toContain(
      "Onshape wählte 4 Regionen, FieldDraft findet dafür nur 3",
    );
  });

  it("builds the ground-truth geometry — and nothing where Onshape has nothing", () => {
    // The query string is only the recipe; this is the SOLID. The imported
    // scene must span exactly the three parts' bounding boxes…
    const bb = kernel.sceneAabb!() as { min: number[]; max: number[] };
    expect(bb.min[1], `scene ${JSON.stringify(bb)}`).toBeCloseTo(GROUND_TRUTH.y[0], 0);
    expect(bb.max[1]).toBeCloseTo(GROUND_TRUTH.y[1], 0);
    expect(bb.min[2]).toBeCloseTo(GROUND_TRUTH.z[0], 0);
    expect(bb.max[2]).toBeCloseTo(GROUND_TRUTH.z[1], 0);

    // …and there must be NO material inside the 604×1500 block: that face is a
    // real region of the sketch, but nobody selected it. Extruding it was the
    // silent-wrong-geometry bug this test exists for.
    const [x, y, z] = GROUND_TRUTH.emptySpot;
    const n = kernel.bodyCount!();
    expect(n).toBeGreaterThan(0);
    for (let i = 0; i < n; i++) {
      expect(
        kernel.raw.bodySdfEval(i, x, y, z),
        `body ${i} has material at the unselected block`,
      ).toBeGreaterThan(0);
    }
  });

  /**
   * …and the viewport must actually SHOW it. The shelf is 4 m of 22 mm
   * boards: at the app's mesh resolution the uniform DC grid has 62 mm
   * cells, so every lattice plane steps clean over every board — the
   * mesher worker answered with a body whose positions/indices were
   * EMPTY, the viewport stayed blank, and nothing anywhere logged a
   * word (the kernel's SDF, scene AABB and B-Rep were all correct,
   * which is why the import tests above passed throughout).
   *
   * `dc::resolution_for_thin_features` closes that: the grid refines
   * until samples land inside the thinnest feature of the body.
   */
  it("renders through the app's mesher-worker path, at the app's resolutions", () => {
    // The document the store hands the worker: the imported kernel's JSON,
    // round-tripped through the main-thread kernel exactly like the app does
    // (open project → `loadJson` → `toJson()` into the rebuild request).
    const main = new WasmKernel();
    main.setSyncMeshing(false); // the mesher worker owns the DC pass
    main.setBrepSuppressed(true); // …and the weaver worker the B-Rep
    main.loadJson(kernel.raw.toJson());
    const documentJson = main.toJson();

    // Resolutions as `Kernel.create` (kernel.ts) sets them, with the
    // coarse→refine ladder from `WorkerOrchestrator.buildMesherRequest`.
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

      // Non-empty is not enough: the mesh must be the three plates, not a
      // few stray triangles from a lucky lattice plane.
      const bb = meshAabb(worker, 0);
      expect(bb.min[1], `${pass} pass: mesh ${JSON.stringify(bb)}`).toBeCloseTo(
        GROUND_TRUTH.y[0],
        0,
      );
      expect(bb.max[1]).toBeCloseTo(GROUND_TRUTH.y[1], 0);
      expect(bb.min[2]).toBeCloseTo(GROUND_TRUTH.z[0], 0);
      expect(bb.max[2]).toBeCloseTo(GROUND_TRUTH.z[1], 0);
    }
  }, 180_000);
});
