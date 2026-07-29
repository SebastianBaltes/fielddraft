/**
 * End-to-end acceptance of the P3 foundation (document VARIABLES + BOOLEAN)
 * and of P4 (THICKEN) against the REAL wasm kernel, on the #415 part studio.
 *
 * Ticket #415 asked for
 * `cad.onshape.com/documents/62f1ce7ad3c034605b717799/…/e/fec8d92b825fe449a8f783a8`.
 * Per the gap matrix it needed three translators: `assignVariable`,
 * `booleanBodies` (P3) and `thicken` (P4, this pass).
 *
 * GROUND TRUTH is Onshape's own answer for that document, never our own
 * resolution (the methodology lesson of #412):
 *
 *   GET /api/v6/parts/d/{did}/w/{wid}/e/{eid}           → ONE solid part, "Part 1" (JKD)
 *   GET …/partid/JKD/boundingboxes                      → x[−33, 33] · y[−63.55, 63.55]
 *                                                         · z[−3, 32.8103] mm
 *   GET …/partid/JKD/tessellatedfaces (322 074 vertices) → the SAME box in x and y,
 *                                                         but z[−3, 32.5]
 *
 * The two disagree in z by 0.31 mm, and the TESSELLATION is the tight one: the
 * bounding-box endpoint is documented as approximate for parts with blended /
 * spline faces, and this part carries seven fillets. Every number asserted here
 * is therefore read off Onshape's own tessellation — still Onshape's answer,
 * just the exact one. (Recorded because the 0.31 mm looked for a while like a
 * missing feature, and it is not one.)
 *
 * The unit tests prove the TRANSLATION; this file proves the kernel ACCEPTS
 * what we write — a `boolean` feature call that regenerates, document variables
 * that survive `setVariable`, and a `thicken` that grows the walls to exactly
 * the thickness Onshape reports.
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

const ticket415 = (): { features: any[] } =>
  JSON.parse(readFileSync(join(__dirname, "fixtures", "onshape-415-features.json"), "utf8"));

/**
 * Onshapes eigene 3D-Antwort für dieselben Sketches
 * (`GET …/sketches?includeGeometry=true&output3D=true`, jede Entity in
 * Weltkoordinaten). Zusammen mit der 2D-Geometrie aus dem Feature-Payload ist
 * Onshapes Sketch-FRAME damit überbestimmt und messbar — die Quelle, aus der
 * der Importer `B_fd⁻¹ ∘ B_os` bildet (#415). Als Fixture, damit der Test kein
 * Netz braucht; im Betrieb holt `onshapeProjectImport` sie über den Go-Proxy.
 */
const sketches3d = (): unknown =>
  JSON.parse(readFileSync(join(__dirname, "fixtures", "onshape-415-sketches3d.json"), "utf8"));

/** Onshape's own answer for the finished part, in mm (the API reports metres);
 *  x/y from `boundingboxes`, z from the tessellation (see the header). */
const GROUND_TRUTH = {
  parts: 1,
  x: [-33.0, 33.0],
  y: [-63.55, 63.55],
  z: [-3.0, 32.5],
};

/**
 * The same tessellation, sliced ABOVE the base plate (z > 10 mm), where the
 * part is nothing but Extrude 2's block plus the two thickened walls:
 *
 *   x ∈ [−25, 25]   — Extrude 2, symmetric 50 mm
 *   y ∈ [−48.5, 48.5] — Sketch 2's ±46.5 mm walls, THICKENED by 2 mm outward
 *
 * That ±48.5 is the thicken's entire geometric contribution to this document,
 * measured in Onshape. It is what {@link THICKEN_ONLY} below is checked against.
 */
const THICKEN_ONLY = {
  x: [-25.0, 25.0],
  y: [-48.5, 48.5],
  z: [-3.0, 32.5],
};

/** Replay of `mesher.worker::handleRebuild` — the exact call sequence the real
 *  worker runs, so an assertion here is an assertion about the viewport. */
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

const featureOfType = (doc: { features: any[] }, type: string): any =>
  doc.features.map((f: any) => f.message ?? f).find((f: any) => f.featureType === type);

const paramOf = (feat: any, id: string): any =>
  feat.parameters.map((p: any) => p.message ?? p).find((p: any) => p.parameterId === id);


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
    const kernel = wrapImportKernel(new WasmKernel());
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
    const kernel = wrapImportKernel(new WasmKernel());
    await importOnshapeFeatures(ticket415(), kernel);
    const bohrloch = variablesOf(kernel.raw).find((v) => v.name === "Bohrloch");
    expect(bohrloch, JSON.stringify(variablesOf(kernel.raw))).toBeTruthy();
    expect(bohrloch!.value).toBeCloseTo(3.1, 5); // 0.31 cm
    expect(bohrloch!.kind).toBe("length");
  });
});

describe("#415 — the Boolean the real kernel regenerates", () => {
  let kernel: SuiteKernel;
  let result: Awaited<ReturnType<typeof importOnshapeFeatures>>;

  beforeAll(async () => {
    kernel = wrapImportKernel(new WasmKernel());
    result = await importOnshapeFeatures(ticket415(), kernel, {
      sketches3d: sketches3d() as any,
    });
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

  it("translates `thicken` without a single complaint of its own", () => {
    // P4: the feature type is gone from the "not imported yet" list, and the
    // translation itself reports nothing — no dropped face, no guessed scope.
    expect(result.errors.join(" | ")).not.toContain("thicken");
    const mine = [...result.errors, ...result.warnings].filter((m) =>
      m.startsWith("Thicken 1:"),
    );
    expect(mine, mine.join(" | ")).toHaveLength(0);
  });

  it("creates one `thicken` on TWO id-hash faces, 2 mm outward, and regenerates", () => {
    const calls = JSON.parse(kernel.raw.toJson()).feature_calls as {
      id: string | number;
      type: string;
      params: Record<string, any>;
    }[];
    const thickens = calls.filter((c) => c.type === "thicken");
    expect(thickens.length, result.errors.join(" | ")).toBe(1);
    const p = thickens[0].params;
    // The kernel's thicken accepts only the fingerprint-carrying element form,
    // and Onshape's two walls must stay two.
    const faces = String(p.faces).split(", ");
    expect(faces).toHaveLength(2);
    for (const f of faces) expect(f).toMatch(/^qFace\(-?\d+ @ /);
    expect(p.thickness).toEqual({ value: 2, unit: "mm" });
    expect(p.thickness_back).toBeUndefined();
    expect(p.reverse).toBeUndefined();
    expect(p.operation).toBe("add");
    expect(String(p.targets)).toMatch(/^qCreatedBy\(\d+\)$/);

    expect(regenErrors(kernel.raw).get(String(thickens[0].id)), "thicken regen").toBeUndefined();
  });

  // The thicken's ENTIRE geometric contribution, checked against Onshape's own
  // measurement of it: the box walls stand at ±46.5 mm and the feature has to
  // move them to ±48.5 mm — outward, not inward, and on both walls.
  it("grows the two walls to Onshape's ±48.5 mm — and renders that way", async () => {
    const doc = ticket415();
    // Sketch 2 + Extrude 2 + Thicken 1, i.e. the block whose walls are
    // thickened and nothing else. The rest of the document only cuts into it
    // (and drags in the wall-frame gap below), which would hide the wall.
    const sub = wrapImportKernel(new WasmKernel());
    const feats = doc.features.map((f: any) => f.message ?? f);
    const subResult = await importOnshapeFeatures(
      {
        features: [
          feats.find((f: any) => f.name === "Sketch 2"),
          feats.find((f: any) => f.name === "Extrude 2"),
          feats.find((f: any) => f.featureType === "thicken"),
        ],
      },
      sub,
      { sketches3d: sketches3d() as any },
    );
    expect(subResult.errors, subResult.errors.join(" | ")).toHaveLength(0);
    expect(sub.raw.bodyCount()).toBe(1);

    // Through the app's real mesher-worker path, at the app's resolutions —
    // structure and SDF do not prove that a user sees anything (#412 lesson).
    const main = new WasmKernel();
    main.setSyncMeshing(false);
    main.setBrepSuppressed(true);
    main.loadJson(sub.raw.toJson());
    const documentJson = main.toJson();

    const FULL_RES = 64;
    const CONTEXT_RES = 32;
    for (const [pass, res] of [
      ["initial", Math.max(8, Math.floor(FULL_RES / 2))],
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
      expect(worker.bodyCount(), `${pass}: body count`).toBe(1);
      expect(
        worker.bodyTriangleCount(0),
        `${pass}: the worker shipped an EMPTY mesh — blank viewport`,
      ).toBeGreaterThan(0);

      const bb = meshAabb(worker, 0);
      const where = `${pass} pass: mesh ${JSON.stringify(bb)}`;
      expect(bb.min[0], where).toBeCloseTo(THICKEN_ONLY.x[0], 0);
      expect(bb.max[0], where).toBeCloseTo(THICKEN_ONLY.x[1], 0);
      // The assertion the whole feature exists for.
      expect(bb.min[1], where).toBeCloseTo(THICKEN_ONLY.y[0], 0);
      expect(bb.max[1], where).toBeCloseTo(THICKEN_ONLY.y[1], 0);
      expect(bb.min[2], where).toBeCloseTo(THICKEN_ONLY.z[0], 0);
      expect(bb.max[2], where).toBeCloseTo(THICKEN_ONLY.z[1], 0);
    }
  }, 180_000);

  /**
   * The whole document against Onshape's own measurement of it.
   *
   * This was pinned as a KNOWN FAILURE while a FRAME mismatch stood in the way:
   * FieldDraft derives a face-mounted sketch's in-plane basis from the face
   * NORMAL alone (`planeBasisFromNormal`: world axis least aligned with n,
   * crossed twice), Onshape from the face's own parametrisation. On this
   * document's bottom wall (n = −Z) FieldDraft has x = +Y / y = +X where Onshape
   * has x = +X, so Sketch 3 (the base plate with its mounting tabs) and Sketch 4
   * (the top opening) landed rotated by 90°, and the part measured
   * x ∈ [−63.54, 63.54] · y ∈ [−48.5, 48.5] — x and y swapped.
   *
   * The importer now measures Onshape's frame from its own 3D answer and
   * rotates the 2D coordinates into FieldDraft's basis before emitting them.
   *
   * MEASURED ON THE RENDER MESH, not on `sceneAabb`. `sceneAabb` is a
   * CONSERVATIVE bound over the CSG tree, and Extrude 1 profiles the WHOLE of
   * Sketch 1 — including the six coincident `…MirrorCS` construction segments
   * Onshape parks at x = −273 mm, which bound no region and cut no material.
   * The scene box therefore reads x ∈ [−273, 91] for a solid that ends at
   * ±33 mm. Onshape's ground truth is its TESSELLATION, so FieldDraft's
   * tessellation is the apples-to-apples answer — and it is also the only one
   * that proves the user sees the part (the #412 lesson).
   */
  it("matches the Onshape ground-truth bounding box", () => {
    expect(kernel.raw.bodyCount(), result.errors.join(" | ")).toBe(GROUND_TRUTH.parts);

    const main = new WasmKernel();
    main.setSyncMeshing(false);
    main.setBrepSuppressed(true);
    main.loadJson(kernel.raw.toJson());
    const worker = replayMesherWorker({
      type: "rebuild",
      requestId: 1,
      documentJson: main.toJson(),
      activeMeshResolution: 64,
      contextMeshResolution: 32,
      activeBodyId: 0,
      meshAlgorithm: "mdc",
    });
    expect(worker.bodyCount()).toBe(GROUND_TRUTH.parts);
    expect(
      worker.bodyTriangleCount(0),
      "the worker shipped an EMPTY mesh — blank viewport",
    ).toBeGreaterThan(0);

    const bb = meshAabb(worker, 0);
    const where = `mesh ${JSON.stringify(bb)}`;
    expect(bb.min[0], where).toBeCloseTo(GROUND_TRUTH.x[0], 1);
    expect(bb.max[0], where).toBeCloseTo(GROUND_TRUTH.x[1], 1);
    // The two assertions the frame transform exists for.
    expect(bb.min[1], where).toBeCloseTo(GROUND_TRUTH.y[0], 1);
    expect(bb.max[1], where).toBeCloseTo(GROUND_TRUTH.y[1], 1);
    expect(bb.min[2], where).toBeCloseTo(GROUND_TRUTH.z[0], 1);
    expect(bb.max[2], where).toBeCloseTo(GROUND_TRUTH.z[1], 1);
  }, 180_000);

  /** The counter-proof: WITHOUT Onshape's 3D answer the very same document
   *  still lands rotated. Keeps the fix from quietly becoming a no-op. */
  it("lands rotated without Onshape's 3D sketch answer", async () => {
    const bare = wrapImportKernel(new WasmKernel());
    await importOnshapeFeatures(ticket415(), bare);
    const main = new WasmKernel();
    main.setSyncMeshing(false);
    main.setBrepSuppressed(true);
    main.loadJson(bare.raw.toJson());
    const worker = replayMesherWorker({
      type: "rebuild",
      requestId: 1,
      documentJson: main.toJson(),
      activeMeshResolution: 64,
      contextMeshResolution: 32,
      activeBodyId: 0,
      meshAlgorithm: "mdc",
    });
    const bb = meshAabb(worker, 0);
    // x and y carry each other's extent — the defect this pass removed.
    expect(bb.max[0]).toBeCloseTo(GROUND_TRUTH.y[1], 1);
    expect(bb.max[1]).toBeCloseTo(48.5, 1);
  }, 180_000);
});
