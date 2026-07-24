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
import { resolvePlaneFrame } from "../sketchPlaneFrame";

const wasmDir = join(__dirname, "..", "wasm");

beforeAll(async () => {
  await initWasm({
    module_or_path: readFileSync(join(wasmDir, "fielddraft_kernel_bg.wasm")),
  });
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
  // The extrude selects FOUR imprinted sub-faces. FieldDraft's SDF-union region
  // model merges the nested rectangles into ONE region, so the selection cannot
  // be honoured 1:1 — the import must SAY SO (not silently extrude a block) and
  // fall back to that one region rather than crashing.
  it("resolves the profile to a qRegion and warns about the region reduction", async () => {
    const kernel = wrap(new WasmKernel());
    const result = await importOnshapeFeatures(ticket412(), kernel);

    const extrude = (JSON.parse(kernel.raw.toJson()).feature_calls as FeatureCall[]).find(
      (c) => c.type === "extrude",
    )!;
    // A concrete region query, NOT the whole-sketch profile — and NOT a silent
    // block. (One FieldDraft region here, so a single qRegion term.)
    expect(String(extrude.params.profiles)).toMatch(/^qRegion\(\d+, \d+\)$/);

    // The mismatch is reported, never hidden.
    expect(
      result.warnings.some((w) => w.includes("Onshape wählte 4 Regionen")),
      result.warnings.join(" | "),
    ).toBe(true);
  }, 120_000);
});
