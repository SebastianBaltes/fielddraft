import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { importOnshapeFeatures, type ImportKernel } from "./onshapeImport";

function kabelbox(): { features: any[] } {
  return JSON.parse(
    readFileSync(join(__dirname, "fixtures", "kabelbox-features.json"), "utf8"),
  );
}

/** The #339 C-channel fixture: has a real `loft` (Loft 1) fed by two sketch
 *  cross-sections (Sketch 2, Sketch 3). Used for the #376 loft-import path. */
function ticket339(): { features: any[] } {
  return JSON.parse(
    readFileSync(join(__dirname, "fixtures", "ticket339-features.json"), "utf8"),
  );
}

/** Grab a deep clone of the loft feature's message body. */
function loftFeature(doc: { features: any[] }): any {
  return (doc.features as any[])
    .map((f) => f.message ?? f)
    .find((f) => f.featureType === "loft");
}

function loftParam(loft: any, id: string): any {
  return loft.parameters.map((p: any) => p.message ?? p).find((p: any) => p.parameterId === id);
}

/** Mock kernel: allocates sequential ids and reports every face-level
 *  query as uniquely resolving (the REAL uniqueness check runs against
 *  the wasm kernel in the app; here we exercise the translation flow). */
function mockKernel(): ImportKernel & { calls: { type: string; params: any }[] } {
  let next = 1;
  const calls: { type: string; params: any }[] = [];
  return {
    calls,
    addSketch(planeQuery, subCalls) {
      calls.push({ type: "sketch", params: { planeQuery, subCalls } });
      return next++;
    },
    addFeatureCall(typeName, paramsJson) {
      calls.push({ type: typeName, params: JSON.parse(paramsJson) });
      return next++;
    },
    resolveQuery() {
      return [{ kind: "face", idHash: "1", query: "qFace(1)" }];
    },
    // A 200 mm cube of "scene": the extent a THROUGH_ALL cut must clear.
    sceneAabb() {
      return { min: [0, 0, 0], max: [200, 200, 200], size: [200, 200, 200] };
    },
  };
}

describe("importOnshapeFeatures — Kabelbox translation flow", () => {
  it("imports sketches with parametric plane queries", async () => {
    const kernel = mockKernel();
    const result = await importOnshapeFeatures(kabelbox(), kernel);

    const sketches = kernel.calls.filter((c) => c.type === "sketch");
    expect(sketches.length).toBe(7);

    const planes = sketches.map((s) => s.params.planeQuery as string);
    // Sketch 1 + 3: default plane.
    expect(planes[0]).toBe("qCreatedBy(origin_plane_xy)");
    expect(planes[2]).toBe("qCreatedBy(origin_plane_xy)");
    // Sketch 2: extrude-1 start cap (fd id 2 — sketch1=1, extrude1=2).
    expect(planes[1]).toBe("qCreatedBy(2, CAP_BOTTOM)");
    // Sketch 5: the wall swept from Sketch 1's left line (entity 12:
    // rectangle lines bottom=3, top=6, left=9, right=12 by emission
    // order? — assert the FORM here; ordinals are covered below).
    expect(planes[4]).toMatch(/^qWall\(2, \d+\)$/);
    // Sketch 4: split-disambiguated wall.
    expect(planes[3]).toMatch(/^qAdjacentTo\(qWall\(2, \d+\), qWall\(\d+, \d+\)\)$/);

    // Extrudes carry operation + profile of the right sketch.
    const extrudes = kernel.calls.filter((c) => c.type === "extrude");
    expect(extrudes.length).toBeGreaterThanOrEqual(5);
    expect(extrudes[0].params.operation).toBe("new");
    expect(extrudes[0].params.profiles).toBe("qCreatedBy(1)");
    expect(extrudes[0].params.depth.value).toBeCloseTo(250); // 25 cm
    expect(extrudes[1].params.operation).toBe("remove");
    // Extrude 2 is THROUGH_ALL: the depth is MEASURED from the scene (twice
    // its diagonal), not a hardcoded constant.
    expect(extrudes[1].params.depth.value).toBeCloseTo(2 * Math.hypot(200, 200, 200));

    // Shell opens extrude 1's END cap.
    const shell = kernel.calls.find((c) => c.type === "shell");
    expect(shell).toBeTruthy();
    expect(shell!.params.open_faces).toBe("qCreatedBy(2, CAP_TOP)");
    expect(shell!.params.thickness.value).toBeCloseTo(2.5);

    // No silent failures: whatever couldn't translate is REPORTED.
    for (const err of result.errors) expect(typeof err).toBe("string");
  });

  it("skips a sketch whose plane resolves ambiguously — visibly", async () => {
    const kernel = mockKernel();
    kernel.resolveQuery = () => []; // nothing resolves
    const result = await importOnshapeFeatures(kabelbox(), kernel);
    // Default-plane sketches still import; face-level planes error out.
    const sketches = kernel.calls.filter((c) => c.type === "sketch");
    expect(sketches.length).toBe(2);
    expect(result.errors.some((e) => e.includes("löst auf 0"))).toBe(true);
  });
});

describe("loft import (#385 / #376 C-channel)", () => {
  it("translates an Onshape loft into ONE loft call with its cross-sections in order", async () => {
    const kernel = mockKernel();
    await importOnshapeFeatures(ticket339(), kernel);

    const lofts = kernel.calls.filter((c) => c.type === "loft");
    expect(lofts.length).toBe(1);
    // Two cross-sections: Sketch 2 (fd id 4) then Sketch 3 (fd id 5), in the
    // order they appear in sheetProfilesArray.
    expect(lofts[0].params.profiles).toBe("qCreatedBy(4), qCreatedBy(5)");
    expect(lofts[0].params.operation).toBe("new");
    expect(lofts[0].params.display_name).toBe("Loft 1");
  });

  it("does NOT degrade the loft to extrudes or skip it (#376 regression)", async () => {
    const kernel = mockKernel();
    const result = await importOnshapeFeatures(ticket339(), kernel);

    // A real loft call exists — the loft is no longer an unknown feature type.
    expect(kernel.calls.some((c) => c.type === "loft")).toBe(true);
    expect(
      result.errors.some((e) => e.includes('Feature-Typ "loft"')),
    ).toBe(false);
    // Exactly the three genuine document extrudes (Extrude 1/2 before, Extrude 3
    // after the loft) — the loft did not spawn extra extrudes standing in for it.
    expect(kernel.calls.filter((c) => c.type === "extrude").length).toBe(3);
  });

  it("skips a SURFACE loft with a warning (no fake solid)", async () => {
    const doc = ticket339();
    loftParam(loftFeature(doc), "bodyType").value = "SURFACE";
    const kernel = mockKernel();
    const result = await importOnshapeFeatures(doc, kernel);

    expect(kernel.calls.some((c) => c.type === "loft")).toBe(false);
    expect(result.warnings.some((w) => w.includes("SOLID") && w.includes("Loft"))).toBe(true);
  });

  it("skips a loft with fewer than 2 resolvable profiles, with a warning", async () => {
    const doc = ticket339();
    // Drop one cross-section: only a single profile remains.
    loftParam(loftFeature(doc), "sheetProfilesArray").items.length = 1;
    const kernel = mockKernel();
    const result = await importOnshapeFeatures(doc, kernel);

    expect(kernel.calls.some((c) => c.type === "loft")).toBe(false);
    expect(
      result.warnings.some((w) => w.includes("weniger als 2") && w.includes("Querschnitt")),
    ).toBe(true);
  });
});

describe("cap side follows the extrude direction (Ticket #339)", () => {
  /** The REAL Kabelbox base: Sketch 1 → Extrude 1 → Sketch 2 (on the extrude's
   *  START cap), with the extrude's direction flipped on demand. Hand-writing
   *  BTON payloads is a trap; these are the live-captured ones. */
  function baseDoc(oppositeDirection: boolean) {
    const feats = (kabelbox().features as any[]).map((f) => f.message ?? f);
    const pick = (name: string) => JSON.parse(JSON.stringify(feats.find((f) => f.name === name)));
    const extrude = pick("Extrude 1");
    for (const p of extrude.parameters) {
      const pm = p.message ?? p;
      if (pm.parameterId === "oppositeDirection") pm.value = oppositeDirection;
    }
    return { features: [pick("Sketch 1"), extrude, pick("Sketch 2")] };
  }

  it("a positive extrude puts the START cap at CAP_BOTTOM", async () => {
    const kernel = mockKernel();
    await importOnshapeFeatures(baseDoc(false), kernel);
    const sketches = kernel.calls.filter((c) => c.type === "sketch");
    expect(sketches.length).toBe(2);
    expect(sketches[1].params.planeQuery).toBe("qCreatedBy(2, CAP_BOTTOM)");
  });

  it("an extrude AGAINST the normal puts the START cap at CAP_TOP", async () => {
    // FieldDraft centres the slab on depth/2, so a NEGATIVE depth lays CAP_TOP
    // — not CAP_BOTTOM — flush with the sketch plane. Getting this backwards
    // made the sketch's plane query resolve to nothing (ticket #339).
    const kernel = mockKernel();
    await importOnshapeFeatures(baseDoc(true), kernel);
    const extrude = kernel.calls.find((c) => c.type === "extrude")!;
    expect(extrude.params.depth.value).toBeLessThan(0);
    const sketches = kernel.calls.filter((c) => c.type === "sketch");
    expect(sketches[1].params.planeQuery).toBe("qCreatedBy(2, CAP_TOP)");
  });
});

/** The #412 part studio: one sketch, one extrude selecting FOUR sketch
 *  regions via compressed SKETCH_ENTITY profile queries. */
function ticket412(): { features: any[] } {
  return JSON.parse(
    readFileSync(join(__dirname, "fixtures", "onshape-412-features.json"), "utf8"),
  );
}

/** Mock kernel whose `regionQueryAtPoint` is supplied per test — the region
 *  resolution runs against the REAL decoded #412 queries and REAL interior
 *  points, only the flood-fill lookup is stubbed. */
function regionMockKernel(
  regionAt: (sketchId: number, u: number, v: number) => string,
): ImportKernel & { calls: { type: string; params: any }[] } {
  let next = 1;
  const calls: { type: string; params: any }[] = [];
  return {
    calls,
    addSketch(planeQuery, subCalls) {
      calls.push({ type: "sketch", params: { planeQuery, subCalls } });
      return next++;
    },
    addFeatureCall(typeName, paramsJson) {
      calls.push({ type: typeName, params: JSON.parse(paramsJson) });
      return next++;
    },
    resolveQuery() {
      return [{ kind: "face", idHash: "1", query: "qFace(1)" }];
    },
    sceneAabb() {
      return { min: [0, 0, 0], max: [200, 200, 200], size: [200, 200, 200] };
    },
    regionQueryAtPoint: regionAt,
  };
}

describe("extrude region selection (#412)", () => {
  it("maps each Onshape region to its own qRegion → a MultiQuery of distinct regions", async () => {
    // A kernel that DID subdivide: hand each distinct interior point its own
    // region index (first-seen order), like a real per-region flood-fill.
    const seen = new Map<string, number>();
    const kernel = regionMockKernel((sid, u, v) => {
      const key = `${Math.round(u)},${Math.round(v)}`;
      if (!seen.has(key)) seen.set(key, seen.size);
      return `qRegion(${sid}, ${seen.get(key)})`;
    });
    const result = await importOnshapeFeatures(ticket412(), kernel);
    const extrude = kernel.calls.find((c) => c.type === "extrude")!;
    // MultiQuery joins top-level with ", "; qRegion's own args also use ", ",
    // so count the qRegion terms rather than a naive split (parser is
    // paren-aware, see protocol/src/lib.rs).
    const profiles = extrude.params.profiles as string;
    const terms = profiles.match(/qRegion\(1, \d+\)/g) ?? [];
    expect(terms.length).toBe(4); // four distinct regions, not the whole sketch
    expect(new Set(terms).size).toBe(4);
    expect(profiles).not.toContain("qCreatedBy");
    // A faithful 1:1 selection raises no reduction warning.
    expect(result.warnings.some((w) => w.includes("FieldDraft findet dafür nur"))).toBe(false);
  });

  it("warns and falls back to the whole sketch when regions collapse to fewer (imprint mismatch)", async () => {
    // A kernel like the real one: all four points land in the ONE union region.
    const kernel = regionMockKernel((sid) => `qRegion(${sid}, 0)`);
    const result = await importOnshapeFeatures(ticket412(), kernel);
    const extrude = kernel.calls.find((c) => c.type === "extrude")!;
    expect(extrude.params.profiles).toBe("qRegion(1, 0)");
    expect(
      result.warnings.some((w) => w.includes("Onshape wählte 4 Regionen")),
      result.warnings.join(" | "),
    ).toBe(true);
  });

  it("warns and extrudes the whole sketch when no region resolves", async () => {
    const kernel = regionMockKernel(() => ""); // every point outside every region
    const result = await importOnshapeFeatures(ticket412(), kernel);
    const extrude = kernel.calls.find((c) => c.type === "extrude")!;
    expect(extrude.params.profiles).toBe("qCreatedBy(1)");
    expect(result.warnings.some((w) => w.includes("nicht auflösbar"))).toBe(true);
  });

  it("extrudes the whole sketch (no region resolution) when the kernel lacks regionQueryAtPoint", async () => {
    const kernel = mockKernel(); // no regionQueryAtPoint
    await importOnshapeFeatures(ticket412(), kernel);
    const extrude = kernel.calls.find((c) => c.type === "extrude")!;
    expect(extrude.params.profiles).toBe("qCreatedBy(1)");
  });
});
