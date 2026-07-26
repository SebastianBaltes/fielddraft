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
interface MockVar {
  name: string;
  value: number;
  kind: string;
  formula?: string;
}

/** A stable stand-in for the kernel's B-Rep face id hash. */
function mockFaceHash(query: string): string {
  let h = 0;
  for (let i = 0; i < query.length; i += 1) h = (h * 31 + query.charCodeAt(i)) >>> 0;
  return String(h);
}

function mockKernel(): ImportKernel & {
  calls: { type: string; params: any }[];
  vars: MockVar[];
} {
  let next = 1;
  const calls: { type: string; params: any }[] = [];
  const vars: MockVar[] = [];
  return {
    calls,
    vars,
    setVariable(name, value, kind, formula) {
      vars.push({ name, value, kind, formula });
    },
    addSketch(planeQuery, subCalls) {
      calls.push({ type: "sketch", params: { planeQuery, subCalls } });
      return next++;
    },
    addFeatureCall(typeName, paramsJson) {
      calls.push({ type: typeName, params: JSON.parse(paramsJson) });
      return next++;
    },
    // ONE face per DISTINCT query. The identity matters for any translation
    // that has to turn N parametric queries into N id-hash face references
    // (thicken): a mock that answers every query with the same face would let
    // a collapse of all N into one pass unnoticed.
    resolveQuery(query) {
      const h = mockFaceHash(query);
      return [{ kind: "face", idHash: h, query: `qFace(${h})` }];
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

/**
 * Sketches on IMPLICIT mate connectors (#416).
 *
 * Three of the document's five sketches are placed on a connector the sketch
 * carries as its own sub-feature. The connector itself is a FieldDraft feature
 * — `mate_connector`, built on the same rules (edge tangent / face normal as
 * +Z, a position `t` along an edge, flip, 90° reorient, offset along its own
 * axes) — so it is imported. Placing a SKETCH on it is the part FieldDraft's
 * kernel cannot do yet, and that is reported in full rather than approximated.
 */
describe("implicit mate connectors (#416)", () => {
  const doc = ticket416();
  const featureById = (id: string) =>
    (doc.features as any[]).map((f) => f.message ?? f).find((f) => f.featureId === id);
  const paramOf = (f: any, id: string) =>
    f.parameters.map((p: any) => p.message ?? p).find((p: any) => p.parameterId === id);
  const rightPlane = () => structuredClone(paramOf(featureById("FeniPjG8I3vr6X2_0"), "sketchPlane").queries);

  const SKETCH_2 = "FfRxz4hmBrm96Lm_0";
  const SKETCH_4 = "FUYLAXOHGV4RBJ9_2";
  const SKETCH_6 = "F2BP3BGeuInpe6G_1";

  /** The chain that carries Sketch 4's connector: its anchor is the cap of
   *  Extrude 4, which is drawn on Sketch 3. Sketch 3 is REPLANTED on the
   *  document's own Right plane — in the document it hangs off a cap of an
   *  extrude that in turn needs Sketch 2, i.e. off this very gap. Everything
   *  the connector translation reads is the document verbatim. */
  const capChain = () => {
    const sketch3 = structuredClone(featureById("FiVOzOHdKGa6422_1"));
    paramOf(sketch3, "sketchPlane").queries = rightPlane();
    return [
      structuredClone(featureById("FeniPjG8I3vr6X2_0")),
      sketch3,
      structuredClone(featureById("FEQ7ajZembzIAXS_1")),
    ];
  };

  const mateCalls = (kernel: { calls: { type: string; params: any }[] }) =>
    kernel.calls.filter((c) => c.type === "mate_connector");

  it("imports the connector of Sketch 4 as a mate_connector on the extrude's CAP", async () => {
    const { kernel, result } = await translate([...capChain(), structuredClone(featureById(SKETCH_4))]);
    const calls = mateCalls(kernel);
    expect(calls.length, result.errors.join(" | ")).toBe(1);
    // CENTER inference on a circular cap EDGE is the cap FACE's own frame:
    // same origin (the centre), same +Z (the cap normal). Naming the face is
    // that coordinate system in the form FieldDraft resolves — not a stand-in.
    expect(calls[0].params.anchor).toMatch(/^qCreatedBy\(\d+, CAP_(TOP|BOTTOM)\)$/);
    expect(calls[0].params.flip).toBe(false);
    expect(calls[0].params.offset).toEqual([0, 0, 0]);
    expect(calls[0].params.reorient).toBe(0);
  });

  it("still SKIPS the sketch on it, naming the connector and the kernel gap", async () => {
    const { kernel, result } = await translate([...capChain(), structuredClone(featureById(SKETCH_4))]);
    // The connector landed; the sketch did not.
    expect(kernel.calls.filter((c) => c.type === "sketch").length).toBe(2); // Sketch 1 + 3
    const err = result.errors.find((e) => e.includes("Sketch 4"))!;
    expect(err).toContain("Mate-Connector");
    expect(err).toContain("resolve_plane_frame");
    expect(err).toContain("GAP-MATRIX");
    // It says the connector itself DID land — otherwise the message would send
    // the user looking for something that is right there in the tree.
    expect(err).toMatch(/als Feature \d+ importiert/);
  });

  it("puts a MID_POINT connector at t = 0.5 on the cap EDGE it names", async () => {
    const { kernel, result } = await translate([
      structuredClone(featureById("FeniPjG8I3vr6X2_0")),
      structuredClone(featureById("FUwE53MM0gG2CH2_0")), // Extrude 1
      structuredClone(featureById(SKETCH_6)),
    ]);
    const calls = mateCalls(kernel);
    expect(calls.length, result.errors.join(" | ")).toBe(1);
    // An edge, not a face: a midpoint is a position ALONG the cap's rim.
    expect(calls[0].params.anchor).toMatch(
      /^qCommonEdge\(qCreatedBy\(\d+, CAP_(TOP|BOTTOM)\), qWall\(\d+, \d+\)\)$/,
    );
    expect(calls[0].params.t).toBe(0.5);
  });

  it("SKIPS a connector on a raw sketch line and names the kernel limit", async () => {
    // Sketch 2's connector sits on an ENDPOINT of a construction line of
    // Sketch 1 — geometry that never enters the B-Rep.
    const { kernel, result } = await translate([
      structuredClone(featureById("FeniPjG8I3vr6X2_0")),
      structuredClone(featureById(SKETCH_2)),
    ]);
    expect(mateCalls(kernel).length).toBe(0);
    const err = result.errors.join(" | ");
    expect(err).toContain("rohen Sketch-Linie");
    expect(err).toContain("resolve_mate_anchor");
    // The query it WOULD use is named, so the gap is checkable, not folklore.
    expect(err).toMatch(/qSketchLine\(\d+, \d+\)/);
  });

  it("carries Onshape's realignment offset verbatim, in millimetres", async () => {
    const sketch4 = structuredClone(featureById(SKETCH_4));
    const p = (id: string) =>
      sketch4.subFeatures[0].parameters.find((q: any) => q.parameterId === id);
    p("translationZ").value = 0.0125; // Onshape reports SI
    p("flipPrimary").value = true;
    const { kernel, result } = await translate([...capChain(), sketch4]);
    const calls = mateCalls(kernel);
    expect(calls.length, result.errors.join(" | ")).toBe(1);
    expect(calls[0].params.offset).toEqual([0, 0, 12.5]);
    expect(calls[0].params.flip).toBe(true);
  });

  it("takes a 90° rotation as a reorient step", async () => {
    const sketch4 = structuredClone(featureById(SKETCH_4));
    sketch4.subFeatures[0].parameters.find((q: any) => q.parameterId === "rotation").value =
      Math.PI / 2;
    const { kernel } = await translate([...capChain(), sketch4]);
    expect(mateCalls(kernel)[0].params.reorient).toBe(1);
  });

  it("REPORTS a rotation it cannot express instead of rounding it off", async () => {
    const sketch4 = structuredClone(featureById(SKETCH_4));
    sketch4.subFeatures[0].parameters.find((q: any) => q.parameterId === "rotation").value =
      Math.PI / 6; // 30°
    const { kernel, result } = await translate([...capChain(), sketch4]);
    expect(result.warnings.join(" | ")).toContain("90°-Schritten");
    expect(mateCalls(kernel)[0].params.reorient).toBe(0);
  });

  it("imports a mate connector that stands in the feature list like any other", async () => {
    // The same feature, just not hidden inside a sketch — 22 of those across
    // the survey corpus. Lifting the sub-feature out is the whole difference.
    const standalone = structuredClone(featureById(SKETCH_4).subFeatures[0]);
    const { kernel, result } = await translate([...capChain(), standalone]);
    expect(mateCalls(kernel).length, result.errors.join(" | ")).toBe(1);
    expect(result.idMap[standalone.featureId]).toBeDefined();
  });
});

// ─── P3: Variablen + Boolean ────────────────────────────────────

/** The #415 document (Onshape `/features`): 20 features, and the smallest one
 *  in the corpus that carries BOTH an `assignVariable` and a `booleanBodies`. */
function ticket415(): { features: any[] } {
  return JSON.parse(
    readFileSync(join(__dirname, "fixtures", "onshape-415-features.json"), "utf8"),
  );
}

const params415 = (feat: any) => feat.parameters.map((p: any) => p.message ?? p);

function paramOf(feat: any, id: string): any {
  return params415(feat).find((p: any) => p.parameterId === id);
}

/** A clone of the document's real `assignVariable` feature, re-authored. */
function variableFeature(
  doc: { features: any[] },
  name: string,
  expression: string,
  variableType = "LENGTH",
  mode = "ASSIGNED",
): any {
  const f = structuredClone(featureOfType(doc, "assignVariable"));
  paramOf(f, "name").value = name;
  paramOf(f, "value").expression = expression;
  paramOf(f, "variableType").value = variableType;
  paramOf(f, "mode").value = mode;
  f.featureId = `FVAR_${name || "anon"}_${expression.length}`;
  return f;
}

describe("assignVariable → FieldDraft-Dokumentvariable (#409/#415/#417)", () => {
  const doc = ticket415();

  it("translates the document's own variable, unit and all", async () => {
    const { kernel, result } = await translate([
      structuredClone(featureOfType(doc, "assignVariable")),
    ]);
    expect(result.errors, result.errors.join(" | ")).toHaveLength(0);
    expect(kernel.vars).toHaveLength(1);
    // `#Bohrloch = 0.31 cm` — FieldDraft stores a length in MILLIMETRES.
    expect(kernel.vars[0].name).toBe("Bohrloch");
    expect(kernel.vars[0].value).toBeCloseTo(3.1, 10);
    expect(kernel.vars[0].kind).toBe("length");
  });

  it("declares variables IN DOCUMENT ORDER so a later one may use an earlier", async () => {
    const { kernel, result } = await translate([
      variableFeature(doc, "Klingenlaenge", "89 cm"),
      variableFeature(doc, "Grifflaenge", "295 mm"),
      variableFeature(doc, "Gesamtlaenge", "#Klingenlaenge+#Grifflaenge"),
    ]);
    expect(result.errors, result.errors.join(" | ")).toHaveLength(0);
    expect(kernel.vars.map((v) => [v.name, v.value])).toEqual([
      ["Klingenlaenge", 890],
      ["Grifflaenge", 295],
      ["Gesamtlaenge", 1185],
    ]);
  });

  it("keeps the dependency as a FieldDraft formula when it computes the same number", async () => {
    const { kernel } = await translate([
      variableFeature(doc, "a", "89 cm"),
      variableFeature(doc, "b", "295 mm"),
      variableFeature(doc, "c", "#a+#b"),
    ]);
    // Provenance the user can edit: the projection is stored ONLY because
    // FieldDraft's own evaluator reproduced 1185 mm from it.
    expect(kernel.vars[2].formula).toBe("(a + b)");
    // A plain literal keeps no formula.
    expect(kernel.vars[0].formula).toBeUndefined();
  });

  it("stores an angle variable in DEGREES and projects no formula for it", async () => {
    const { kernel } = await translate([
      variableFeature(doc, "Knaufflachwinkel", "45 deg", "ANGLE"),
      variableFeature(doc, "Doppelt", "#Knaufflachwinkel*2", "ANGLE"),
    ]);
    expect(kernel.vars[0]).toMatchObject({ name: "Knaufflachwinkel", kind: "angle" });
    expect(kernel.vars[0].value).toBeCloseTo(45, 10);
    expect(kernel.vars[1].value).toBeCloseTo(90, 10);
    // FieldDraft's formula language is dimensionless and mm/degree-based;
    // an angle formula would collide with its radian trigonometry.
    expect(kernel.vars[1].formula).toBeUndefined();
  });

  it("stores a NUMBER variable without a unit", async () => {
    const { kernel } = await translate([variableFeature(doc, "Faktor", "0.592561", "NUMBER")]);
    expect(kernel.vars[0]).toMatchObject({ name: "Faktor", kind: "number" });
    expect(kernel.vars[0].value).toBeCloseTo(0.592561, 10);
  });

  it("reports a MEASURED variable instead of inventing a value", async () => {
    const { kernel, result } = await translate([
      variableFeature(doc, "MirrorBoxLength", '"?"', "LENGTH", "MEASURED"),
    ]);
    expect(kernel.vars).toHaveLength(0);
    expect(result.warnings.join(" | ")).toContain("GEMESSEN");
    expect(result.warnings.join(" | ")).toContain("#MirrorBoxLength");
  });

  it("reports an unevaluable declaration instead of storing a wrong number", async () => {
    const { kernel, result } = await translate([
      variableFeature(doc, "Abgeleitet", "#GibtEsNicht*2"),
    ]);
    expect(kernel.vars).toHaveLength(0);
    expect(result.errors.join(" | ")).toContain('unbekannte Variable "#GibtEsNicht"');
  });

  it("makes an Onshape name legal for FieldDraft and says so", async () => {
    const { kernel, result } = await translate([variableFeature(doc, "Brett Stärke!", "15 mm")]);
    expect(kernel.vars[0].name).toBe("Brett_Stärke_");
    expect(result.warnings.join(" | ")).toContain("nicht zulässig");
  });

  it("skips a nameless declaration with an error", async () => {
    const { kernel, result } = await translate([variableFeature(doc, "", "15 mm")]);
    expect(kernel.vars).toHaveLength(0);
    expect(result.errors.join(" | ")).toContain("ohne Variablennamen");
  });
});

describe("variablen-fähige Maße — nie mehr still falsch (#409/#415/#417)", () => {
  const doc = ticket415();
  /** Sketch 1 + Extrude 1 of the #415 document: a BLIND extrude whose depth
   *  parameter carries the authored expression and NO evaluated SI value —
   *  precisely the shape that used to fall through to a default. */
  const sketchAndExtrude = () =>
    (doc.features as any[]).slice(1, 3).map((f) => structuredClone(f));

  function withDepth(expression: string): any[] {
    const feats = sketchAndExtrude();
    const extrude = feats[1].message ?? feats[1];
    paramOf(extrude, "depth").expression = expression;
    return feats;
  }

  it("takes the literal depth as before", async () => {
    const { kernel, result } = await translate(sketchAndExtrude());
    const extrude = kernel.calls.find((c) => c.type === "extrude");
    expect(extrude, result.errors.join(" | ")).toBeTruthy();
    expect(Math.abs(extrude!.params.depth.value)).toBeCloseTo(29.5, 6);
  });

  it("resolves a depth written as a VARIABLE expression", async () => {
    const feats = [
      structuredClone(featureOfType(doc, "assignVariable")), // #Bohrloch = 0.31 cm
      ...withDepth("#Bohrloch*2 + 1 mm"),
    ];
    const { kernel, result } = await translate(feats);
    const extrude = kernel.calls.find((c) => c.type === "extrude");
    expect(extrude, result.errors.join(" | ")).toBeTruthy();
    expect(Math.abs(extrude!.params.depth.value)).toBeCloseTo(7.2, 6);
    // Nothing to report: it evaluated.
    expect(result.warnings.filter((w) => w.includes("Ausdruck"))).toHaveLength(0);
  });

  it("SAYS SO when an expression cannot be evaluated, instead of using a default silently", async () => {
    const { kernel, result } = await translate(withDepth("#Brettstaerke"));
    const extrude = kernel.calls.find((c) => c.type === "extrude");
    // The default is still used — but it is now announced, with the cause,
    // the parameter, the substituted number and the honest verdict.
    expect(Math.abs(extrude!.params.depth.value)).toBe(25);
    const warning = result.warnings.find((w) => w.includes("#Brettstaerke"));
    expect(warning, result.warnings.join(" | ")).toBeTruthy();
    expect(warning).toContain("nicht auswertbar");
    expect(warning).toContain('unbekannte Variable "#Brettstaerke"');
    expect(warning).toContain("Parameter depth");
    expect(warning).toContain("Vorgabe 25 mm");
    expect(warning).toContain("FALSCH");
  });

  it("reports a dimension mismatch (an angle where a length belongs)", async () => {
    const { result } = await translate(withDepth("45 deg"));
    const warning = result.warnings.find((w) => w.includes("45 deg"));
    expect(warning, result.warnings.join(" | ")).toBeTruthy();
    expect(warning).toContain("erwartet war eine Länge");
  });

  it("says each broken expression once, not once per parameter read", async () => {
    const { result } = await translate(withDepth("#Brettstaerke"));
    expect(result.warnings.filter((w) => w.includes("#Brettstaerke"))).toHaveLength(1);
  });
});

describe("booleanBodies → FieldDraft-Boolean (#409/#415/#419/#420)", () => {
  const doc339 = ticket339();
  const doc415 = ticket415();

  /** Every feature of the #339 document up to and including its Boolean. */
  const upToBoolean = (doc: { features: any[] }) => {
    const feats = doc.features as any[];
    const idx = feats.findIndex((f) => (f.message ?? f).featureType === "booleanBodies");
    return feats.slice(0, idx + 1).map((f) => structuredClone(f));
  };

  it("translates the #339 SUBTRACTION into a scoped FieldDraft Boolean", async () => {
    const { kernel, result } = await translate(upToBoolean(doc339));
    const booleans = kernel.calls.filter((c) => c.type === "boolean");
    expect(booleans.length, result.errors.join(" | ")).toBe(1);
    expect(booleans[0].params.operation).toBe("Subtract");
    // Both operand lists are FieldDraft feature references — the only form the
    // kernel Boolean accepts — and the scope is EXPLICIT.
    expect(booleans[0].params.tools).toMatch(/^qCreatedBy\(\d+\)$/);
    expect(booleans[0].params.targets).toMatch(/^qCreatedBy\(\d+\)$/);
    expect(booleans[0].params.tools).not.toBe(booleans[0].params.targets);
  });

  it("translates the #415 SUBTRACTION the same way", async () => {
    const { kernel, result } = await translate(upToBoolean(doc415));
    const booleans = kernel.calls.filter((c) => c.type === "boolean");
    expect(booleans.length, result.errors.join(" | ")).toBe(1);
    expect(booleans[0].params.operation).toBe("Subtract");
    expect(booleans[0].params.targets).toMatch(/^qCreatedBy\(\d+\)$/);
  });

  it("maps a UNION onto one tool list and an INTERSECTION onto tools + scope", async () => {
    for (const [onshapeOp, fieldDraft] of [
      ["UNION", "Union"],
      ["INTERSECTION", "Intersect"],
    ] as const) {
      const feats = upToBoolean(doc339);
      const bool = feats[feats.length - 1].message ?? feats[feats.length - 1];
      paramOf(bool, "operationType").value = onshapeOp;
      const { kernel, result } = await translate(feats);
      const booleans = kernel.calls.filter((c) => c.type === "boolean");
      expect(booleans.length, result.errors.join(" | ")).toBe(1);
      expect(booleans[0].params.operation).toBe(fieldDraft);
      if (onshapeOp === "UNION") {
        // Onshape's union names every body in `tools`; a stray target joins it.
        expect(booleans[0].params.tools.split(", ")).toHaveLength(2);
        expect(booleans[0].params.targets).toBeUndefined();
      } else {
        expect(booleans[0].params.targets).toMatch(/^qCreatedBy\(\d+\)$/);
      }
    }
  });

  it("carries `keep tools` over", async () => {
    const feats = upToBoolean(doc339);
    const bool = feats[feats.length - 1].message ?? feats[feats.length - 1];
    paramOf(bool, "keepTools").value = true;
    const { kernel } = await translate(feats);
    expect(kernel.calls.find((c) => c.type === "boolean")!.params.keep_tools).toBe(true);
  });

  it("reports the tool clearance it cannot reproduce", async () => {
    const feats = upToBoolean(doc339);
    const bool = feats[feats.length - 1].message ?? feats[feats.length - 1];
    paramOf(bool, "offset").value = true;
    paramOf(bool, "offsetDistance").expression = "0.2 mm";
    const { result } = await translate(feats);
    expect(result.warnings.join(" | ")).toContain("Werkzeug-Aufmaß");
  });

  it("SKIPS a Boolean whose operand was never imported instead of guessing one", async () => {
    // Drop the loft the Boolean's tool query names: its operand is now
    // unresolvable, and a Boolean on the wrong body deletes material silently.
    const feats = upToBoolean(doc339).filter(
      (f) => ((f.message ?? f) as any).featureType !== "loft",
    );
    const { kernel, result } = await translate(feats);
    expect(kernel.calls.filter((c) => c.type === "boolean")).toHaveLength(0);
    expect(result.errors.join(" | ")).toMatch(/(Werkzeug|Ziel)-Körper nicht auflösbar/);
    expect(result.errors.join(" | ")).toContain("Feature übersprungen");
  });

  it("SKIPS a subtraction that has no target (no implicit merge scope)", async () => {
    const feats = upToBoolean(doc339);
    const bool = feats[feats.length - 1].message ?? feats[feats.length - 1];
    paramOf(bool, "targets").queries = [];
    const { kernel, result } = await translate(feats);
    expect(kernel.calls.filter((c) => c.type === "boolean")).toHaveLength(0);
    expect(result.errors.join(" | ")).toContain("ohne Ziel-Körper");
  });

  it("points the id map at the feature that owns the resulting body", async () => {
    const { result } = await translate(upToBoolean(doc339));
    const feats = doc339.features as any[];
    const bool = feats.map((f) => f.message ?? f).find((f) => f.featureType === "booleanBodies");
    const extrude1 = feats.map((f) => f.message ?? f).find((f) => f.name === "Extrude 1");
    // A Subtract leaves its geometry on the TARGET body, so a later Onshape
    // query naming "the boolean's body" must land on that feature.
    expect(result.idMap[bool.featureId]).toBe(result.idMap[extrude1.featureId]);
  });
});

// ─── P4: Thicken ────────────────────────────────────────────────

describe("thicken → FieldDraft-Thicken (#415/#417/#418/#419)", () => {
  const doc = ticket415();
  /** Sketch 2 + Extrude 2 — the sketch the thickened walls were swept from and
   *  the extrude that owns them; the thicken's `booleanScope` names the latter. */
  const prefix = () => (doc.features as any[]).slice(3, 5).map((f) => structuredClone(f));
  const thicken = () => structuredClone(featureOfType(doc, "thicken"));

  /** The prefix plus a (possibly re-authored) thicken. */
  const withThicken = (edit: (t: any) => void = () => {}) => {
    const t = thicken();
    edit(t);
    return [...prefix(), t];
  };

  const thickenCall = (kernel: { calls: { type: string; params: any }[] }) =>
    kernel.calls.find((c) => c.type === "thicken");

  it("translates the document's thicken: two walls, 2 mm outward, scoped ADD", async () => {
    const { kernel, result } = await translate(withThicken());
    expect(result.errors, result.errors.join(" | ")).toHaveLength(0);
    const call = thickenCall(kernel);
    expect(call, "no thicken feature was created").toBeTruthy();
    expect(call!.params.display_name).toBe("Thicken 1");
    // Onshape names TWO faces; the kernel's thicken takes only the id-hash
    // form, and the two must stay two.
    const faces = String(call!.params.faces).split(", ");
    expect(faces).toHaveLength(2);
    for (const f of faces) expect(f).toMatch(/^qFace\(\d+\)$/);
    expect(new Set(faces).size).toBe(2);
    // 0.2 cm, grown outward — no second direction, no flip.
    expect(call!.params.thickness).toEqual({ value: 2, unit: "mm" });
    expect(call!.params.thickness_back).toBeUndefined();
    expect(call!.params.reverse).toBeUndefined();
    // ADD with an EXPLICIT scope (Feature-Scope-Direktive).
    expect(call!.params.operation).toBe("add");
    expect(call!.params.targets).toMatch(/^qCreatedBy\(\d+\)$/);
  });

  it("registers the thicken in the id map so later queries can name its body", async () => {
    const { result } = await translate(withThicken());
    expect(result.idMap[thicken().featureId]).toBeGreaterThan(0);
  });

  it("carries Onshape's second thickness over as the inward wall", async () => {
    const { kernel, result } = await translate(
      withThicken((t) => {
        paramOf(t, "thickness2").expression = "0.5 cm";
      }),
    );
    expect(result.errors, result.errors.join(" | ")).toHaveLength(0);
    expect(thickenCall(kernel)!.params.thickness).toEqual({ value: 2, unit: "mm" });
    expect(thickenCall(kernel)!.params.thickness_back).toEqual({ value: 5, unit: "mm" });
  });

  it("maps the direction arrow onto the reverse flag", async () => {
    const { kernel } = await translate(
      withThicken((t) => {
        paramOf(t, "oppositeDirection").value = true;
      }),
    );
    expect(thickenCall(kernel)!.params.reverse).toBe(true);
  });

  it("rewrites an empty first direction as a flipped one-sided wall", async () => {
    // Onshape: nothing outward, 3 mm inward. FieldDraft has no zero primary
    // thickness, but the SAME solid is a 3 mm wall grown the other way.
    const { kernel, result } = await translate(
      withThicken((t) => {
        paramOf(t, "thickness1").expression = "0 cm";
        paramOf(t, "thickness2").expression = "0.3 cm";
      }),
    );
    expect(result.errors, result.errors.join(" | ")).toHaveLength(0);
    const call = thickenCall(kernel)!;
    expect(call.params.thickness).toEqual({ value: 3, unit: "mm" });
    expect(call.params.thickness_back).toBeUndefined();
    expect(call.params.reverse).toBe(true);
  });

  it("evaluates a variable-driven wall thickness instead of guessing one", async () => {
    const feats = withThicken((t) => {
      paramOf(t, "thickness1").expression = "#Bohrloch";
    });
    // The document's own variable (0.31 cm) declared ahead of the thicken.
    const { kernel, result } = await translate([
      structuredClone(featureOfType(doc, "assignVariable")),
      ...feats,
    ]);
    expect(result.warnings.join(" | ")).not.toContain("Bohrloch");
    expect(thickenCall(kernel)!.params.thickness.value).toBeCloseTo(3.1, 6);
  });

  it("SKIPS a thicken whose wall thickness is zero", async () => {
    const { kernel, result } = await translate(
      withThicken((t) => {
        paramOf(t, "thickness1").expression = "0 cm";
      }),
    );
    expect(thickenCall(kernel)).toBeUndefined();
    expect(result.errors.join(" | ")).toContain("Wandstärke ist 0");
  });

  it("SKIPS a thicken whose faces bind to nothing instead of thickening a guess", async () => {
    const kernel = mockKernel();
    kernel.resolveQuery = () => [];
    const result = await importOnshapeFeatures({ features: withThicken() }, kernel);
    expect(thickenCall(kernel)).toBeUndefined();
    expect(result.errors.join(" | ")).toContain("löst auf 0 Flächen auf (erwartet 1)");
    expect(result.errors.join(" | ")).toContain("keine auflösbare Fläche");
  });

  it("drops an AMBIGUOUS face — thickening the wrong wall adds phantom material", async () => {
    const kernel = mockKernel();
    let call = 0;
    const real = kernel.resolveQuery.bind(kernel);
    kernel.resolveQuery = (q: string) => {
      call += 1;
      return call === 1 ? [...real(q), ...real(`${q} sibling`)] : real(q);
    };
    const result = await importOnshapeFeatures({ features: withThicken() }, kernel);
    expect(result.errors.join(" | ")).toContain("löst auf 2 Flächen auf (erwartet 1)");
    // The unambiguous one still lands, and the shortfall is named.
    expect(String(thickenCall(kernel)!.params.faces).split(", ")).toHaveLength(1);
    expect(result.warnings.join(" | ")).toContain(
      "Onshape dickt 2 Flächen auf, FieldDraft 1",
    );
  });

  it("makes a NEW body without a merge scope", async () => {
    const { kernel } = await translate(
      withThicken((t) => {
        paramOf(t, "operationType").value = "NEW";
      }),
    );
    expect(thickenCall(kernel)!.params.operation).toBe("new");
    expect(thickenCall(kernel)!.params.targets).toBeUndefined();
  });

  it("reports `keep tools`, which only means something for Onshape surfaces", async () => {
    const { result } = await translate(
      withThicken((t) => {
        paramOf(t, "keepTools").value = true;
      }),
    );
    expect(result.warnings.join(" | ")).toContain("Werkzeuge behalten");
  });
});
