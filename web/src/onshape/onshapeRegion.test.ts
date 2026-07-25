import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { decodeRegionQuery } from "./onshapeRegion";
import { regionFaceIndex, sketchEntityVertices, type FaceMetrics } from "./onshapeImport";

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

describe("sketchEntityVertices", () => {
  it("reads real #412 sketch geometry in millimetres", () => {
    const geom = sketchEntityVertices(sketchFeature(ticket412()));
    // dsTR38 is a strip near y≈1711 mm, x≈[628..2115] — metres ×1000.
    const bottom = geom.get("dsTR38wnDTF3.bottom")!;
    expect(bottom.length).toBe(2);
    expect(bottom[0][0]).toBeCloseTo(627.63, 1);
    expect(bottom[1][0]).toBeCloseTo(2114.91, 1);
    expect(bottom[0][1]).toBeCloseTo(1722.24, 1);
  });

  it("flattens a circle into its rim, not a bounding box", () => {
    // The region resolution probes just OFF a named curve for the face it
    // borders; a box diagonal borders nothing, so the rim is the only usable
    // form. Every emitted point must sit on the circle.
    const geom = sketchEntityVertices({
      entities: [
        {
          btType: "BTMSketchCurve-4",
          entityId: "C",
          geometry: { btType: "BTCurveGeometryCircle-115", xCenter: 0.01, yCenter: 0, radius: 0.05 },
        },
      ],
    });
    const pts = geom.get("C")!;
    expect(pts.length).toBeGreaterThan(8);
    for (const [x, y] of pts) expect(Math.hypot(x - 10, y)).toBeCloseTo(50, 6);
  });
});

describe("regionFaceIndex", () => {
  // A 100×40 plate (face 2) inside a 1000×1000 sheet (the ambient face 0).
  // Every one of the plate's edges borders BOTH faces, so adjacency alone never
  // decides; what decides is that the named edges account for half the plate's
  // boundary and ~2 % of the sheet's.
  const geom = new Map<string, [number, number][]>([
    ["P.bottom", [[10, 20], [110, 20]]],
    ["P.top", [[10, 60], [110, 60]]],
    ["Other.right", [[500, 0], [500, 400]]],
  ]);
  const metricTable: Record<number, FaceMetrics> = {
    0: { area: 1_000_000 - 4_000, perimeter: 4_000 + 280 },
    1: { area: 900_000, perimeter: 3_800 },
    2: { area: 4_000, perimeter: 280 },
  };
  const metrics = (i: number): FaceMetrics | null => metricTable[i] ?? null;
  /** Inside the plate -> 2; elsewhere in the sheet -> the ambient face 0. */
  const faceAt = (u: number, v: number): number | null =>
    v > 20 && v < 60 && u > 10 && u < 110 ? 2 : 0;

  it("picks the face whose boundary the named edges account for", () => {
    expect(regionFaceIndex(["P.bottom"], geom, faceAt, metrics)).toBe(2);
  });

  it("keeps the plate when a second, unrelated edge is named too", () => {
    // The ambient face borders BOTH named edges — an adjacency count would
    // return it. Coverage does not: those edges are a rounding error of its
    // perimeter and half of the plate's.
    expect(regionFaceIndex(["Other.right", "P.top"], geom, faceAt, metrics)).toBe(2);
  });

  it("returns null instead of substituting a sibling for an unknown id", () => {
    // `P.left` does not exist here. Falling back to "any entity sharing the
    // prefix" is what made #412 resolve to a block Onshape never extruded.
    expect(regionFaceIndex(["P.left"], geom, faceAt, metrics)).toBeNull();
    expect(regionFaceIndex(["P.bottom", "P.left"], geom, faceAt, metrics)).toBeNull();
    expect(regionFaceIndex(["ghost"], new Map(), faceAt, metrics)).toBeNull();
    expect(regionFaceIndex([], geom, faceAt, metrics)).toBeNull();
  });

  it("ignores a curve that runs THROUGH a face instead of bounding it", () => {
    // Both sides of a dangling trim line land in the same face; it says nothing
    // about that face, so no coverage is credited and nothing resolves.
    const g = new Map<string, [number, number][]>([["D", [[200, 200], [300, 200]]]]);
    expect(regionFaceIndex(["D"], g, faceAt, metrics)).toBeNull();
  });

  it("returns null when every probe lands outside all regions", () => {
    expect(regionFaceIndex(["P.bottom"], geom, () => null, metrics)).toBeNull();
  });

  it("returns null when the kernel reports no metrics for the candidate", () => {
    expect(regionFaceIndex(["P.bottom"], geom, faceAt, () => null)).toBeNull();
  });
});
