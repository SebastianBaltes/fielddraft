/**
 * End-to-end acceptance of the P3 foundation (document VARIABLES + BOOLEAN)
 * against the REAL wasm kernel, on the #415 part studio.
 *
 * Ticket #415 asked for
 * `cad.onshape.com/documents/62f1ce7ad3c034605b717799/…/e/fec8d92b825fe449a8f783a8`.
 * Per the gap matrix it needs three translators: `assignVariable` and
 * `booleanBodies` (this pass) and `thicken` (P4, still missing).
 *
 * GROUND TRUTH is Onshape's own answer for that document, never our own
 * resolution (the methodology lesson of #412):
 *
 *   GET /api/v6/parts/d/{did}/w/{wid}/e/{eid}     → ONE solid part, "Part 1" (JKD)
 *   GET …/partid/JKD/boundingboxes                → the AABB below
 *
 * The unit tests prove the TRANSLATION; this file proves the kernel ACCEPTS
 * what we write — a `boolean` feature call that regenerates, and document
 * variables (name, unit, formula text) that survive `setVariable`.
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
  await initWasm({
    module_or_path: readFileSync(join(wasmDir, "fielddraft_kernel_bg.wasm")),
  });
});

const ticket415 = (): { features: any[] } =>
  JSON.parse(readFileSync(join(__dirname, "fixtures", "onshape-415-features.json"), "utf8"));

/** Onshape's own answer for this document, in mm (the API reports metres). */
const GROUND_TRUTH = {
  parts: 1,
  x: [-33.0, 33.0],
  y: [-63.55, 63.55],
  z: [-3.0, 32.8103],
};

const featureOfType = (doc: { features: any[] }, type: string): any =>
  doc.features.map((f: any) => f.message ?? f).find((f: any) => f.featureType === type);

const paramOf = (feat: any, id: string): any =>
  feat.parameters.map((p: any) => p.message ?? p).find((p: any) => p.parameterId === id);

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
    setVariable: (name: string, value: number, kind: string, formula?: string) =>
      k.setVariable(name, value, kind, formula),
  };
}

/** Per-feature regeneration errors, keyed by feature id. */
function regenErrors(k: WasmKernel): Map<string, string> {
  const rows = JSON.parse(k.listFeatures()) as { id: string | number; error?: string | null }[];
  const out = new Map<string, string>();
  for (const r of rows) if (r.error) out.set(String(r.id), r.error);
  return out;
}

interface KernelVar {
  name: string;
  value: number;
  kind: string;
  formula?: string;
}

const variablesOf = (k: WasmKernel): KernelVar[] => JSON.parse(k.listVariables());

/** A clone of the document's real `assignVariable` feature, re-authored. */
function variableFeature(
  doc: { features: any[] },
  name: string,
  expression: string,
  variableType = "LENGTH",
): any {
  const f = structuredClone(featureOfType(doc, "assignVariable"));
  paramOf(f, "name").value = name;
  paramOf(f, "value").expression = expression;
  paramOf(f, "variableType").value = variableType;
  f.featureId = `FVAR_${name}`;
  return f;
}

describe("#415 — document variables reach the real kernel", () => {
  it("stores a variable chain with its value, kind and formula", async () => {
    const doc = ticket415();
    const kernel = wrap(new WasmKernel());
    const result = await importOnshapeFeatures(
      {
        features: [
          variableFeature(doc, "Klingenlaenge", "89 cm"),
          variableFeature(doc, "Grifflaenge", "295 mm"),
          variableFeature(doc, "Gesamtlaenge", "#Klingenlaenge+#Grifflaenge"),
          variableFeature(doc, "Knaufwinkel", "45 deg", "ANGLE"),
        ],
      },
      kernel,
    );
    expect(result.errors, result.errors.join(" | ")).toHaveLength(0);

    const vars = new Map(variablesOf(kernel.raw).map((v) => [v.name, v]));
    // Lengths are stored in MILLIMETRES, angles in DEGREES — FieldDraft's base.
    expect(vars.get("Klingenlaenge")).toMatchObject({ kind: "length" });
    expect(vars.get("Klingenlaenge")!.value).toBeCloseTo(890, 4);
    expect(vars.get("Gesamtlaenge")!.value).toBeCloseTo(1185, 4);
    expect(vars.get("Knaufwinkel")).toMatchObject({ kind: "angle" });
    expect(vars.get("Knaufwinkel")!.value).toBeCloseTo(45, 6);
    // The dependency survives as an editable FieldDraft formula.
    expect(vars.get("Gesamtlaenge")!.formula).toBe("(Klingenlaenge + Grifflaenge)");
  });

  it("imports the document's own variable", async () => {
    const kernel = wrap(new WasmKernel());
    await importOnshapeFeatures(ticket415(), kernel);
    const bohrloch = variablesOf(kernel.raw).find((v) => v.name === "Bohrloch");
    expect(bohrloch, JSON.stringify(variablesOf(kernel.raw))).toBeTruthy();
    expect(bohrloch!.value).toBeCloseTo(3.1, 5); // 0.31 cm
    expect(bohrloch!.kind).toBe("length");
  });
});

describe("#415 — the Boolean the real kernel regenerates", () => {
  let kernel: ImportKernel & { raw: WasmKernel };
  let result: Awaited<ReturnType<typeof importOnshapeFeatures>>;

  beforeAll(async () => {
    kernel = wrap(new WasmKernel());
    result = await importOnshapeFeatures(ticket415(), kernel);
  }, 180_000);

  it("creates exactly one `boolean` feature, and it regenerates cleanly", () => {
    const calls = JSON.parse(kernel.raw.toJson()).feature_calls as {
      id: string | number;
      type: string;
      params: Record<string, any>;
    }[];
    const booleans = calls.filter((c) => c.type === "boolean");
    expect(booleans.length, result.errors.join(" | ")).toBe(1);
    expect(booleans[0].params.operation).toBe("Subtract");
    // Explicit scope on both sides (no implicit merge), in the only operand
    // form the kernel Boolean accepts.
    expect(String(booleans[0].params.tools)).toMatch(/^qCreatedBy\(\d+\)$/);
    expect(String(booleans[0].params.targets)).toMatch(/^qCreatedBy\(\d+\)$/);

    const errors = regenErrors(kernel.raw);
    expect(errors.get(String(booleans[0].id)), "boolean regen").toBeUndefined();
  });

  it("still misses `thicken`, and says so", () => {
    // P4 territory; pinned here so closing that gap shows up as a failure.
    expect(result.errors.join(" | ")).toContain("thicken");
  });

  // The document is not importable yet: `thicken` (P4) contributes a whole wall
  // of the part. Pinned as a KNOWN FAILURE against the Onshape ground truth so
  // that closing P4 makes this test go green instead of silently staying red.
  it.fails("matches the Onshape ground-truth bounding box", () => {
    expect(kernel.raw.bodyCount()).toBe(GROUND_TRUTH.parts);
    const bb = JSON.parse(kernel.raw.sceneAabb()) as { min: number[]; max: number[] };
    expect(bb.min[0]).toBeCloseTo(GROUND_TRUTH.x[0], 1);
    expect(bb.max[0]).toBeCloseTo(GROUND_TRUTH.x[1], 1);
    expect(bb.min[1]).toBeCloseTo(GROUND_TRUTH.y[0], 1);
    expect(bb.max[1]).toBeCloseTo(GROUND_TRUTH.y[1], 1);
    expect(bb.min[2]).toBeCloseTo(GROUND_TRUTH.z[0], 1);
    expect(bb.max[2]).toBeCloseTo(GROUND_TRUTH.z[1], 1);
  });
});
