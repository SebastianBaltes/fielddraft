import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { decodeRegionQuery } from "./onshapeRegion";
import { regionFaceIndex, regionInteriorPoint, sketchEntityVertices } from "./onshapeImport";

/** The #412 part studio: one sketch (many imprinted rectangles) + one extrude
 *  whose profile selects FOUR sketch regions via compressed SKETCH_ENTITY
 *  queries. The real reverse-engineering corpus for region resolution. */
function ticket412(): { features: any[] } {
  return JSON.parse(
    readFileSync(join(__dirname, "fixtures", "onshape-412-features.json"), "utf8"),
  );
}

const msg = (x: any) => x?.message ?? x;

function extrudeEntityQueries(doc: { features: any[] }): string[] {
  const ext = doc.features.map(msg).find((f) => f.featureType === "extrude");
  const entities = ext.parameters
    .map(msg)
    .find((p: any) => p.parameterId === "entities");
  return entities.queries.map((q: any) => msg(q).queryString as string);
}

function sketchFeature(doc: { features: any[] }): any {
  return doc.features.map(msg).find((f) => f.featureType === "newSketch");
}

describe("decodeRegionQuery — #412 profile queries", () => {
  it("decodes each region query to its owning sketch and referenced entities", async () => {
    const qs = extrudeEntityQueries(ticket412());
    expect(qs.length).toBe(4);

    const decoded = await Promise.all(qs.map((q) => decodeRegionQuery(q)));
    for (const d of decoded) {
      expect(d).not.toBeNull();
      expect(d!.isRegionQuery).toBe(true);
      // All four regions belong to the ONE sketch the extrude draws from.
      expect(d!.sketchFeatureId).toBe("FK2BpslRgEVXcqd_0");
      expect(d!.entityIds.length).toBeGreaterThan(0);
    }

    // Each region is discriminated by a DIFFERENT bounding rectangle — the
    // decode must surface that, or the four regions collapse to one.
    const secondBases = decoded.map((d) => d!.entityIds.map((e) => e.split(".")[0]));
    expect(secondBases[0]).toContain("oPgfZ6ZnMtct");
    expect(secondBases[1]).toContain("VIuVSdLYde01");
    expect(secondBases[2]).toContain("tVU4dFWkoozg");
    expect(secondBases[3]).toContain("dsTR38wnDTF3");
  });

  it("returns null for a non-compressed (plain) query string", async () => {
    expect(await decodeRegionQuery('query=qSketchRegion(id + "F0", 0);')).toBeNull();
    expect(await decodeRegionQuery("not a query at all")).toBeNull();
  });
});

describe("regionInteriorPoint", () => {
  it("returns the centre of the rectangle whose four sides are referenced", () => {
    // A 100×40 axis-aligned rectangle at (10,20)..(110,60).
    const geom = new Map<string, [number, number][]>([
      ["R.bottom", [[10, 20], [110, 20]]],
      ["R.top", [[10, 60], [110, 60]]],
      ["R.left", [[10, 20], [10, 60]]],
      ["R.right", [[110, 20], [110, 60]]],
    ]);
    const p = regionInteriorPoint(["R.bottom", "R.top", "R.left", "R.right"], geom)!;
    expect(p[0]).toBeCloseTo(60);
    expect(p[1]).toBeCloseTo(40);
  });

  it("falls back to a base rectangle's real segments for an imprint-derived id", () => {
    // `R.top` is not a raw segment; only R.left / R.right exist. The base
    // fallback must still yield a point inside the strip.
    const geom = new Map<string, [number, number][]>([
      ["R.left", [[0, 0], [0, 10]]],
      ["R.right", [[20, 0], [20, 10]]],
    ]);
    const p = regionInteriorPoint(["R.top"], geom)!;
    expect(p[0]).toBeCloseTo(10);
    expect(p[1]).toBeCloseTo(5);
  });

  it("returns null when no referenced entity has geometry", () => {
    expect(regionInteriorPoint(["ghost"], new Map())).toBeNull();
  });

  it("reads real #412 sketch geometry in millimetres", () => {
    const geom = sketchEntityVertices(sketchFeature(ticket412()));
    // dsTR38 is a strip near y≈1711 mm, x≈[615..2115] — metres ×1000.
    const p = regionInteriorPoint(["dsTR38wnDTF3.bottom", "dsTR38wnDTF3.top"], geom)!;
    expect(p[0]).toBeGreaterThan(600);
    expect(p[0]).toBeLessThan(2200);
    expect(p[1]).toBeGreaterThan(1690);
    expect(p[1]).toBeLessThan(1730);
  });
});

describe("regionFaceIndex", () => {
  // A 100x40 plate (face 2) sitting inside a much larger ambient face (0).
  // Probing off ANY of the plate's edges finds both: the plate and the ambient
  // face. The plate is the specific one, so the smallest area must win.
  const geom = new Map<string, [number, number][]>([
    ["P.bottom", [[10, 20], [110, 20]]],
    ["P.top", [[10, 60], [110, 60]]],
  ]);
  const areas: Record<number, number> = { 0: 1_000_000, 1: 900_000, 2: 4_000 };
  const faceArea = (i: number) => areas[i] ?? Infinity;
  /** Inside the plate -> 2; anywhere else in the sheet -> the ambient face 0. */
  const faceAt = (u: number, v: number): number | null =>
    v > 20 && v < 60 && u > 10 && u < 110 ? 2 : 0;

  it("picks the specific (smallest) face adjacent to a named edge", () => {
    expect(regionFaceIndex(["P.bottom"], geom, faceAt, faceArea)).toBe(2);
  });

  it("still picks the plate when a second, unrelated edge is named too", () => {
    // The ambient face borders BOTH named edges, so an intersection rule would
    // wrongly return it; the specificity rule keeps the plate.
    const g2 = new Map(geom);
    g2.set("Other.right", [
      [500, 0],
      [500, 400],
    ]);
    expect(regionFaceIndex(["Other.right", "P.top"], g2, faceAt, faceArea)).toBe(2);
  });

  it("returns null when no named entity has geometry", () => {
    expect(regionFaceIndex(["ghost"], new Map(), faceAt, faceArea)).toBeNull();
  });

  it("returns null when every probe lands outside all regions", () => {
    expect(regionFaceIndex(["P.bottom"], geom, () => null, faceArea)).toBeNull();
  });
});
