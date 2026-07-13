/**
 * End-to-end acceptance of the Onshape import against the REAL wasm
 * kernel (Onshape-Import Schritt 5) — the Kabelbox part studio.
 *
 * Semantics note: a sketch's plane query is resolved MID-HISTORY (against
 * the body as of that feature), exactly like Onshape's rollback state —
 * so the acceptance signal is "the sketch feature regenerates without an
 * error", not "its query still binds at the end of the document" (a later
 * cut may legitimately consume the face it was drawn on). The kernel has
 * NO silent fallback: an unresolvable plane is a hard feature error.
 *
 * What this pins down (see docs/ONSHAPE-IMPORT-STATE.md):
 *  - Every imported sketch regenerates cleanly on a resolved face, and
 *    its plane FRAME travels when the base body's depth changes.
 *  - Sketches whose Onshape plane has no FieldDraft counterpart fail
 *    VISIBLY, never silently on XY. That is currently the case for the
 *    three sketches on a SIDE of the rounded-rectangle base: FieldDraft
 *    separates faces at CREASES, so a rounded rectangle has ONE
 *    tangentially fused lateral face, not four sides + four rounds.
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

  it("regenerates every imported sketch on a resolved face — none on a guessed plane", () => {
    const sketches = sketchCalls(kernel.raw);
    const errors = regenErrors(kernel.raw);
    for (const s of sketches) {
      expect(errors.get(String(s.id)), `sketch #${s.id} regen`).toBeUndefined();
    }

    // Exactly the two Onshape default-plane sketches use a datum; every
    // other imported sketch sits on a RESOLVED face of a prior feature.
    const planes = sketches.map((s) => s.params.plane as string);
    const datums = planes.filter((p) => p.startsWith("qCreatedBy(origin_plane_"));
    expect(datums.length).toBe(2);
    expect(planes.length).toBeGreaterThan(datums.length);
  });

  it("names the tangential-fusion limit instead of silently defaulting to XY", () => {
    // The known gap: sketches on a SIDE of the rounded-rectangle base.
    const wallErrors = result.errors.filter((e) => e.includes("qWall(") && e.includes("Ebenen-Query"));
    expect(wallErrors.length).toBeGreaterThan(0);
    for (const e of wallErrors) {
      expect(e).toMatch(/tangential verschmolzene Mantelfläche/);
    }
    // And none of them quietly became an XY sketch.
    const planes = sketchCalls(kernel.raw).map((s) => s.params.plane as string);
    expect(planes.filter((p) => p === "qCreatedBy(origin_plane_xy)").length).toBe(2);
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
