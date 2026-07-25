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

describe("importOnshapeFeatures — batch meshing deferral (#410)", () => {
  it("suppresses the display mesh for the whole walk and rebuilds it once", async () => {
    // Records the interleaving of feature-building calls and mesh toggles.
    const log: string[] = [];
    let next = 1;
    const kernel: ImportKernel = {
      addSketch() {
        log.push("build");
        return next++;
      },
      addFeatureCall() {
        log.push("build");
        return next++;
      },
      resolveQuery() {
        return [{ kind: "face", idHash: "1", query: "qFace(1)" }];
      },
      sceneAabb() {
        return { min: [0, 0, 0], max: [200, 200, 200], size: [200, 200, 200] };
      },
      setSyncMeshing(enabled: boolean) {
        log.push(enabled ? "mesh:on" : "mesh:off");
      },
    };

    await importOnshapeFeatures(kabelbox(), kernel);

    // Meshing is switched OFF before the first feature is built and back ON only
    // after the last — never toggled per feature (the O(n²) main-thread stall).
    expect(log[0]).toBe("mesh:off");
    expect(log[log.length - 1]).toBe("mesh:on");
    expect(log.filter((e) => e === "mesh:off")).toHaveLength(1);
    expect(log.filter((e) => e === "mesh:on")).toHaveLength(1);
    const firstBuild = log.indexOf("build");
    const lastBuild = log.lastIndexOf("build");
    expect(firstBuild).toBeGreaterThan(log.indexOf("mesh:off"));
    expect(lastBuild).toBeLessThan(log.indexOf("mesh:on"));
  });

  it("re-enables meshing even when a feature throws", async () => {
    let on: boolean | null = null;
    const kernel: ImportKernel = {
      addSketch() {
        throw new Error("boom");
      },
      addFeatureCall() {
        return 1;
      },
      resolveQuery() {
        return [{ kind: "face", idHash: "1", query: "qFace(1)" }];
      },
      setSyncMeshing(enabled: boolean) {
        on = enabled;
      },
    };
    await expect(importOnshapeFeatures(kabelbox(), kernel)).rejects.toThrow("boom");
    expect(on).toBe(true); // finally re-enabled the mesh pass
  });
});

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

/** Mock kernel whose imprint arrangement is supplied per test — the region
 *  resolution runs against the REAL decoded #412 queries and the REAL sketch
 *  geometry, only the arrangement itself is stubbed. */
function regionMockKernel(
  regionAt: (sketchId: number, u: number, v: number) => string,
  faces: { index: number; area: number; perimeter: number }[] = [],
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
    listSketchRegions: () => JSON.stringify(faces),
  };
}

/** The imprint arrangement of the #412 sketch, as the real kernel reports it:
 *  the sheet (0), a 604×1500 block the extrude does NOT select (1) and the
 *  three plates Onshape turned into parts (2, 3, 4). Rectangles in sketch mm. */
const PLATES: [number, number, number, number][] = [
  [-2488.69, 800, -1885.09, 2300], // 1 — block, never extruded
  [-1885.09, 800.17, -885.09, 822.17], // 2 — JHH
  [627.63, 1700.0, 2114.91, 1722.24], // 3 — JHL
  [-1885.09, 1700.24, 614.91, 1722.24], // 4 — JHD
];
const SHEET: [number, number, number, number] = [-2488.69, 0, 2511.31, 2300];

const rectPerimeter = (r: [number, number, number, number]) =>
  2 * (r[2] - r[0]) + 2 * (r[3] - r[1]);
const rectArea = (r: [number, number, number, number]) => (r[2] - r[0]) * (r[3] - r[1]);

/** `regionQueryAtPoint` over that arrangement. `merge` folds plate indices
 *  together to emulate an arrangement that did NOT separate them. */
function arrangementAt(merge: Record<number, number> = {}) {
  return (sid: number, u: number, v: number): string => {
    for (let i = 0; i < PLATES.length; i++) {
      const [x0, y0, x1, y1] = PLATES[i];
      if (u > x0 && u < x1 && v > y0 && v < y1) return `qRegion(${sid}, ${merge[i + 1] ?? i + 1})`;
    }
    const inSheet = u > SHEET[0] && u < SHEET[2] && v > SHEET[1] && v < SHEET[3];
    return inSheet ? `qRegion(${sid}, 0)` : "";
  };
}

/** The face metrics that go with it — the sheet's boundary is its own loop plus
 *  every plate's, which is precisely what keeps it from winning. */
function arrangementFaces(merge: Record<number, number> = {}) {
  const rows = [
    {
      index: 0,
      area: rectArea(SHEET) - PLATES.reduce((s, p) => s + rectArea(p), 0),
      perimeter: rectPerimeter(SHEET) + PLATES.reduce((s, p) => s + rectPerimeter(p), 0),
    },
  ];
  for (let i = 0; i < PLATES.length; i++) {
    const index = merge[i + 1] ?? i + 1;
    const row = rows.find((r) => r.index === index);
    if (row) {
      row.area += rectArea(PLATES[i]);
      row.perimeter += rectPerimeter(PLATES[i]);
    } else {
      rows.push({ index, area: rectArea(PLATES[i]), perimeter: rectPerimeter(PLATES[i]) });
    }
  }
  return rows;
}

describe("extrude region selection (#412)", () => {
  it("resolves each region query to the plate its named curves bound", async () => {
    const kernel = regionMockKernel(arrangementAt(), arrangementFaces());
    await importOnshapeFeatures(ticket412(), kernel);
    const extrude = kernel.calls.find((c) => c.type === "extrude")!;
    const profiles = extrude.params.profiles as string;
    // The three plates Onshape really turned into parts — collapsed into ONE
    // `qRegions(sketch, [i,j,k])` expression. Face 1 (the 604×1500 block the
    // long left edge also runs along) must NOT be among them: the query that
    // named it names a curve this sketch does not have.
    expect(profiles).toBe("qRegions(1, [2, 3, 4])");
  });

  it("warns that a selection naming absent curves was dropped", async () => {
    const kernel = regionMockKernel(arrangementAt(), arrangementFaces());
    const result = await importOnshapeFeatures(ticket412(), kernel);
    const warn = result.warnings.find((w) => w.includes("Onshape wählte"));
    expect(warn, result.warnings.join(" | ")).toBeTruthy();
    expect(warn).toContain("Onshape wählte 4 Regionen, FieldDraft findet dafür nur 3");
    expect(warn).toContain("1 nennen Sketch-Kurven, die dieser Sketch nicht enthält");
  });

  it("reports a collapse when two queries land on the same atomic face", async () => {
    // An arrangement that did NOT separate the two upper plates: faces 3 and 4
    // are one. Two of the resolvable queries then name the same face.
    const merge = { 4: 3 };
    const kernel = regionMockKernel(arrangementAt(merge), arrangementFaces(merge));
    const result = await importOnshapeFeatures(ticket412(), kernel);
    const extrude = kernel.calls.find((c) => c.type === "extrude")!;
    expect(extrude.params.profiles).toBe("qRegions(1, [2, 3])");
    const warn = result.warnings.find((w) => w.includes("Onshape wählte"))!;
    expect(warn, result.warnings.join(" | ")).toContain(
      "1 fielen mit einer anderen Auswahl auf dieselbe atomare Face",
    );
  });

  it("keeps the whole-sketch profile when the sketch has only ONE face", async () => {
    // An arrangement that did not subdivide at all: picking its single face is
    // the whole sketch, and `qCreatedBy` is the form that keeps the entity
    // provenance later `qWall(...)` plane queries depend on.
    const only = PLATES[1];
    const kernel = regionMockKernel(
      (sid, u, v) =>
        u > only[0] && u < only[2] && v > only[1] && v < only[3] ? `qRegion(${sid}, 0)` : "",
      [{ index: 0, area: rectArea(only), perimeter: rectPerimeter(only) }],
    );
    await importOnshapeFeatures(ticket412(), kernel);
    const extrude = kernel.calls.find((c) => c.type === "extrude")!;
    expect(extrude.params.profiles).toBe("qCreatedBy(1)");
  });

  it("warns and extrudes the whole sketch when no region resolves", async () => {
    const kernel = regionMockKernel(() => "", arrangementFaces()); // outside everything
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

  it("extrudes the whole sketch when the kernel cannot rank the faces", async () => {
    // Without `listSketchRegions` there is no perimeter to normalise coverage
    // against — every candidate would look equally good, so don't pick one.
    const kernel = regionMockKernel(arrangementAt());
    delete (kernel as { listSketchRegions?: unknown }).listSketchRegions;
    await importOnshapeFeatures(ticket412(), kernel);
    const extrude = kernel.calls.find((c) => c.type === "extrude")!;
    expect(extrude.params.profiles).toBe("qCreatedBy(1)");
  });
});

/** The #414 fixture (Onshape doc 17b3c41d…): 27 features, among them the one
 *  `chamfer` and the one `hole` this ticket added translators for. */
function ticket414(): { features: any[] } {
  return JSON.parse(
    readFileSync(join(__dirname, "fixtures", "onshape-414-features.json"), "utf8"),
  );
}

/** A deep-ish clone of one feature message, by Onshape feature type. */
function featureOfType(doc: { features: any[] }, type: string): any {
  return (doc.features as any[]).map((f) => f.message ?? f).find((f) => f.featureType === type);
}

/** Run the importer over ONLY the given features (plus everything they depend
 *  on) against the mock kernel — the translation contract, not the geometry. */
async function translate(features: any[]) {
  const kernel = mockKernel();
  const result = await importOnshapeFeatures({ features }, kernel);
  return { kernel, result };
}

describe("chamfer import (#414)", () => {
  const doc = ticket414();
  // Sketch 1 + Extrude 1 are what the chamfer's edge query names.
  const prefix = () => (doc.features as any[]).slice(0, 2);
  const chamfer = () => structuredClone(featureOfType(doc, "chamfer"));

  it("translates the document's 45° chamfer into a chamfer_edge on a cap rim", async () => {
    const { kernel, result } = await translate([...prefix(), chamfer()]);
    const calls = kernel.calls.filter((c) => c.type === "chamfer_edge");
    expect(calls.length, result.errors.join(" | ")).toBe(1);
    // Onshape authored `width1 = 0.5 cm` at 45° — 5 mm of bevel, in `Length`
    // wire form (a bare f32 would fail schema parsing, #405).
    expect(calls[0].params.distance).toEqual({ value: 5, unit: "mm" });
    // The edge is the rim where the extrude's cap meets the wall swept from a
    // named sketch entity — Onshape's `CAP_EDGE` form.
    expect(calls[0].params.edge).toMatch(
      /^qCommonEdge\(qCreatedBy\(\d+, CAP_(TOP|BOTTOM)\), qWall\(\d+, \d+\)\)$/,
    );
  });

  it("says out loud that OFFSET_ANGLE 45° is only exact on square corners", async () => {
    const { result } = await translate([...prefix(), chamfer()]);
    expect(result.warnings.join(" | ")).toContain("OFFSET_ANGLE 45°");
    expect(result.warnings.join(" | ")).toContain("rechtwinklig");
  });

  it("takes EQUAL_OFFSETS verbatim from `width`", async () => {
    const c = chamfer();
    const p = c.parameters.map((x: any) => x.message ?? x);
    p.find((x: any) => x.parameterId === "chamferType").value = "EQUAL_OFFSETS";
    // `width` is 4.5 cm in the document — the EQUAL_OFFSETS slot.
    const { kernel } = await translate([...prefix(), c]);
    const calls = kernel.calls.filter((x) => x.type === "chamfer_edge");
    expect(calls[0].params.distance).toEqual({ value: 45, unit: "mm" });
  });

  it("SKIPS an unequal-offset chamfer with a warning instead of squaring it off", async () => {
    for (const type of ["TWO_OFFSETS", "OPPOSITE_OFFSETS"]) {
      const c = chamfer();
      const p = c.parameters.map((x: any) => x.message ?? x);
      p.find((x: any) => x.parameterId === "chamferType").value = type;
      const { kernel, result } = await translate([...prefix(), c]);
      expect(kernel.calls.filter((x) => x.type === "chamfer_edge").length, type).toBe(0);
      expect(result.warnings.join(" | "), type).toContain(type);
    }
  });

  it("SKIPS an OFFSET_ANGLE that is not 45°", async () => {
    const c = chamfer();
    const p = c.parameters.map((x: any) => x.message ?? x);
    p.find((x: any) => x.parameterId === "angle").expression = "30 deg";
    const { kernel, result } = await translate([...prefix(), c]);
    expect(kernel.calls.filter((x) => x.type === "chamfer_edge").length).toBe(0);
    expect(result.warnings.join(" | ")).toContain("30°");
  });
});

describe("hole import (#414)", () => {
  const doc = ticket414();
  /** Everything the hole depends on: the base sketch/extrudes plus Sketch 2,
   *  which carries the point the hole is drilled at. */
  const prefix = () =>
    (doc.features as any[]).filter((f) => {
      const t = (f.message ?? f).featureType;
      return t === "newSketch" || t === "extrude";
    });
  const hole = () => structuredClone(featureOfType(doc, "hole"));

  it("drills the document's hole at its SKETCH POINT, Ø6 mm, 50 mm blind", async () => {
    const { kernel, result } = await translate([...prefix(), hole()]);
    const calls = kernel.calls.filter((c) => c.type === "hole");
    expect(calls.length, result.errors.join(" | ")).toBe(1);
    const p = calls[0].params;
    // Diameter taken VERBATIM from the document (`holeDiameterV2 = 6 mm`),
    // not round-tripped through FieldDraft's ISO table.
    expect(p.standard_size).toBe("custom");
    expect(p.diameter).toEqual({ value: 6, unit: "mm" });
    expect(p.end_type).toBe("blind");
    expect(p.depth).toEqual({ value: 50, unit: "mm" });
    expect(p.style).toBe("simple");
    // The location is the sketch point Onshape placed it on…
    expect(p.locations).toMatch(/^qSketchPoint\(\d+, \d+\)$/);
    // …and the merge scope became an explicit body target (Scope-Direktive).
    expect(p.targets).toMatch(/^qCreatedBy\(\d+\)$/);
  });

  it("reports the countersink it cannot model instead of drilling it silently", async () => {
    const { result } = await translate([...prefix(), hole()]);
    const warn = result.warnings.join(" | ");
    // The document's hole is C_SINK Ø10 mm × 90° with a 118° drill tip.
    expect(warn).toContain("Countersink");
    expect(warn).toContain("Bohrspitze");
  });

  it("approximates an up-to-entity end condition by through-all, and says so", async () => {
    const h = hole();
    const p = h.parameters.map((x: any) => x.message ?? x);
    p.find((x: any) => x.parameterId === "endStyleV2").value = "UP_TO_ENTITY";
    const { kernel, result } = await translate([...prefix(), h]);
    const call = kernel.calls.find((c) => c.type === "hole")!;
    expect(call.params.end_type).toBe("through_all");
    expect(result.warnings.join(" | ")).toContain("UP_TO_ENTITY");
  });

  it("skips the hole when no location resolves, instead of drilling nowhere", async () => {
    // Without Sketch 2 the point the hole sits on does not exist.
    const noPointPrefix = prefix().filter(
      (f: any) => (f.message ?? f).featureId !== "FW7hNi1ppStDFOU_1",
    );
    const { kernel, result } = await translate([...noPointPrefix, hole()]);
    expect(kernel.calls.filter((c) => c.type === "hole").length).toBe(0);
    expect(result.errors.join(" | ")).toContain("keine auflösbare Loch-Position");
  });
});

/** The #416 fixture (Onshape doc 204cc669…): 19 features, among them the one
 *  `revolve` this ticket added a translator for, and two 45° chamfers. */
function ticket416(): { features: any[] } {
  return JSON.parse(
    readFileSync(join(__dirname, "fixtures", "onshape-416-features.json"), "utf8"),
  );
}

describe("revolve import (#416)", () => {
  const doc = ticket416();
  const featureById = (id: string) =>
    (doc.features as any[]).map((f) => f.message ?? f).find((f) => f.featureId === id);
  const paramOf = (f: any, id: string) =>
    f.parameters.map((p: any) => p.message ?? p).find((p: any) => p.parameterId === id);

  /** Everything the revolve depends on: Sketch 1 + Extrude 2 (the body its ADD
   *  scope names) and Sketch 4, which carries both its profile and its axis.
   *
   *  Sketch 4 is REPLANTED on the document's own Right plane. In the document
   *  it sits on an implicit MATE CONNECTOR, which FieldDraft has no counterpart
   *  for (see the gap matrix) — a plane gap that would swallow the revolve
   *  before its own translation ever runs. Substituting a plane query from the
   *  SAME document isolates what this ticket is about; the untouched document
   *  is what `onshapeImport416E2E.slow.test.ts` runs. */
  const prefix = () => {
    const sketch1 = structuredClone(featureById("FeniPjG8I3vr6X2_0"));
    const sketch4 = structuredClone(featureById("FUYLAXOHGV4RBJ9_2"));
    paramOf(sketch4, "sketchPlane").queries = structuredClone(
      paramOf(sketch1, "sketchPlane").queries,
    );
    return [sketch1, structuredClone(featureById("FjQX2yN4ORMPFe0_0")), sketch4];
  };
  const revolve = () => structuredClone(featureOfType(doc, "revolve"));

  it("spins the document's profile around its CONSTRUCTION sketch line", async () => {
    const { kernel, result } = await translate([...prefix(), revolve()]);
    const calls = kernel.calls.filter((c) => c.type === "revolve");
    expect(calls.length, result.errors.join(" | ")).toBe(1);
    const p = calls[0].params;
    // The axis is a sketch LINE query — the only axis form `RevolveDef` takes
    // besides a B-Rep edge; the document draws it as a construction line.
    expect(p.axis).toMatch(/^qSketchLine\(\d+, \d+\)$/);
    expect(p.profiles).toMatch(/^qCreatedBy\(\d+\)|^qRegions?\(/);
    // Onshape's ADD with an explicit boolean scope → explicit targets.
    expect(p.operation).toBe("add");
    expect(p.targets).toMatch(/^qCreatedBy\(\d+\)$/);
  });

  it("imports the FULL turn without any angle warning", async () => {
    const { result } = await translate([...prefix(), revolve()]);
    expect(result.warnings.join(" | ")).not.toContain("Umdrehung");
  });

  it("SKIPS a partial revolve instead of closing it into a full ring", async () => {
    const r = revolve();
    paramOf(r, "revolveType").value = "ONE_SIDED";
    paramOf(r, "angle").expression = "90 deg";
    const { kernel, result } = await translate([...prefix(), r]);
    expect(kernel.calls.filter((c) => c.type === "revolve").length).toBe(0);
    expect(result.warnings.join(" | ")).toContain("Teil-Umdrehung");
    expect(result.warnings.join(" | ")).toContain("90°");
  });

  it("accepts a TWO_SIDED pair that adds up to a full turn, and says so", async () => {
    const r = revolve();
    // The document already carries 30°/330° in the (inactive) angle slots.
    paramOf(r, "revolveType").value = "TWO_SIDED";
    const { kernel, result } = await translate([...prefix(), r]);
    expect(kernel.calls.filter((c) => c.type === "revolve").length).toBe(1);
    expect(result.warnings.join(" | ")).toContain("TWO_SIDED");
  });

  it("SKIPS a SURFACE revolve", async () => {
    const r = revolve();
    paramOf(r, "bodyType").value = "SURFACE";
    const { kernel, result } = await translate([...prefix(), r]);
    expect(kernel.calls.filter((c) => c.type === "revolve").length).toBe(0);
    expect(result.warnings.join(" | ")).toContain("SURFACE");
  });

  it("reports an axis it cannot translate instead of guessing one", async () => {
    // Without Sketch 4 neither the profile nor the axis line exists.
    const noSketch = prefix().filter((f: any) => f.featureId !== "FUYLAXOHGV4RBJ9_2");
    const { kernel, result } = await translate([...noSketch, revolve()]);
    expect(kernel.calls.filter((c) => c.type === "revolve").length).toBe(0);
    expect(result.errors.join(" | ")).toContain("Profil-Sketch nicht auflösbar");
  });
});

describe("mate-connector sketch planes (#416)", () => {
  const doc = ticket416();

  it("names the missing Onshape concept instead of reporting a decode failure", async () => {
    // Sketch 4 of the document sits on an implicit mate connector — a
    // sub-feature of the sketch itself, invisible in the feature-type census.
    const sketch4 = (doc.features as any[])
      .map((f) => f.message ?? f)
      .find((f) => f.featureId === "FUYLAXOHGV4RBJ9_2");
    const { result } = await translate([structuredClone(sketch4)]);
    expect(result.errors.join(" | ")).toContain("Mate-Connector");
    expect(result.errors.join(" | ")).toContain("GAP-MATRIX");
  });
});
