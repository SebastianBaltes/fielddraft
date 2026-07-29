import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { decodeRegionQuery } from "./onshapeRegion";
import { sketchEntityVertices } from "./onshapeImport";
import { resolveRegionFaces, type FaceMetrics } from "./onshapeRegionFaces";

/** The #412 part studio: one sketch (many imprinted rectangles) + one extrude
 *  whose profile selects FOUR sketch regions via compressed SKETCH_ENTITY
 *  queries. The real reverse-engineering corpus for region resolution. */
function ticket412(): { features: any[] } {
  return JSON.parse(
    readFileSync(join(__dirname, "fixtures", "onshape-412-features.json"), "utf8"),
  );
}

const msg = (x: any) => x?.message ?? x;

function entityQueries(doc: { features: any[] }, name?: string): string[] {
  const feat = doc.features
    .map(msg)
    .find((f) => (name ? f.name === name : f.featureType === "extrude"));
  const entities = feat.parameters.map(msg).find((p: any) => p.parameterId === "entities");
  return entities.queries.map((q: any) => msg(q).queryString as string);
}

function sketchFeature(doc: { features: any[] }): any {
  return doc.features.map(msg).find((f) => f.featureType === "newSketch");
}

describe("decodeRegionQuery — #412 profile queries", () => {
  it("decodes each region query to its owning sketch and bounding edges", async () => {
    const qs = entityQueries(ticket412());
    expect(qs.length).toBe(4);

    const decoded = await Promise.all(qs.map((q) => decodeRegionQuery(q)));
    for (const d of decoded) {
      expect(d).not.toBeNull();
      expect(d!.isRegionQuery).toBe(true);
      // All four regions belong to the ONE sketch the extrude draws from.
      expect(d!.sketchFeatureId).toBe("FK2BpslRgEVXcqd_0");
      expect(d!.edges.length).toBeGreaterThan(0);
    }

    // Each region is discriminated by a DIFFERENT bounding rectangle — the
    // decode must surface that, or the four regions collapse to one.
    const bases = decoded.map((d) => d!.edges.map((e) => e.entityId.split(".")[0]));
    expect(bases[0]).toContain("oPgfZ6ZnMtct");
    expect(bases[1]).toContain("VIuVSdLYde01");
    expect(bases[2]).toContain("tVU4dFWkoozg");
    expect(bases[3]).toContain("dsTR38wnDTF3");
  });

  it("keeps regions apart that name the very same curves (#440)", async () => {
    // `Extrude 4` of the #419 document selects THREE strips of one rectangle.
    // Two of them are fenced by literally the same three curves and differ only
    // in where those curves are TRIMMED — a decode that reports ids alone
    // cannot tell them apart, and #412's did not: the extrude came out with two
    // regions instead of three.
    const doc = JSON.parse(
      readFileSync(join(__dirname, "fixtures", "onshape-419-features.json"), "utf8"),
    );
    const decoded = await Promise.all(
      entityQueries(doc, "Extrude 4").map((q) => decodeRegionQuery(q)),
    );
    expect(decoded.length).toBe(3);
    const signature = decoded.map((d) =>
      JSON.stringify(d!.edges.map((e) => [e.entityId, e.ends]).sort()),
    );
    expect(new Set(signature).size).toBe(3);
    // What keeps them apart is the TRIM: all three run along the same top edge
    // of the same rectangle, each between a different pair of imprint vertices.
    const tops = decoded.map((d) =>
      JSON.stringify(d!.edges.find((e) => e.entityId === "q0PrimhP9B2a.top")!.ends),
    );
    expect(new Set(tops).size).toBe(3);
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

  it("runs a full circle in ONSHAPE's direction, clockwise ones included (#446)", () => {
    // The rim's DIRECTION is what „the region lies left of this edge" is
    // measured against, so a circle Onshape parametrises clockwise — every
    // mirrored copy is one — must come out clockwise. Read as a fixed
    // counter-clockwise rim, the two mirrored bores of `Sketch 4` in the #419
    // document elected the material AROUND them instead of their own inside.
    const circle = (clockwise: boolean) =>
      sketchEntityVertices({
        entities: [
          {
            btType: "BTMSketchCurve-4",
            entityId: "C",
            geometry: {
              btType: "BTCurveGeometryCircle-115",
              xCenter: 0,
              yCenter: 0,
              radius: 0.05,
              xDir: 1,
              yDir: 0,
              clockwise,
            },
          },
        ],
      }).get("C")!;
    /** Twice the signed area — positive counter-clockwise. */
    const orientation = (pts: [number, number][]) =>
      pts.reduce((a, p, i) => {
        const q = pts[(i + 1) % pts.length];
        return a + (p[0] * q[1] - q[0] * p[1]);
      }, 0);
    expect(orientation(circle(false))).toBeGreaterThan(0);
    expect(orientation(circle(true))).toBeLessThan(0);
  });
});

describe("resolveRegionFaces", () => {
  // A 100×40 plate inside a 1000×1000 sheet (the ambient face 0). The plate is
  // split across the middle in OUR arrangement (face 2 below, face 3 above) but
  // is ONE region in Onshape's — the #440 mismatch in miniature.
  const geom = new Map<string, [number, number][]>([
    ["P.bottom", [[10, 20], [110, 20]]],
    ["P.top", [[10, 60], [110, 60]]],
    ["P.left", [[10, 20], [10, 60]]],
    ["P.right", [[110, 20], [110, 60]]],
    ["P.mid", [[10, 40], [110, 40]]],
    ["Other.right", [[500, 0], [500, 400]]],
  ]);
  const metricTable: Record<number, FaceMetrics> = {
    0: { area: 1_000_000 - 4_000, perimeter: 4_000 + 280 },
    2: { area: 2_000, perimeter: 240 },
    3: { area: 2_000, perimeter: 240 },
  };
  const metrics = (i: number): FaceMetrics | null => metricTable[i] ?? null;
  /** Lower half of the plate -> 2, upper half -> 3, everything else -> 0. */
  const faceAt = (u: number, v: number): number | null =>
    u > 10 && u < 110 && v > 20 && v < 60 ? (v < 40 ? 2 : 3) : 0;
  const whole = (...ids: string[]) => ids.map((entityId) => ({ entityId, ends: [] }));

  it("covers EVERY face of a region our arrangement splits (#440)", () => {
    // `P.left` and `P.right` each run along BOTH halves without a break: one
    // Onshape imprint edge has one face per side, so both halves are inside
    // that one Onshape region.
    const r = resolveRegionFaces(
      whole("P.bottom", "P.top", "P.left", "P.right"),
      geom,
      faceAt,
      metrics,
    );
    expect(r.faces).toEqual([2, 3]);
    expect(r.conflict).toBe(false);
  });

  it("picks the face whose boundary the named edges account for", () => {
    // The ambient face borders the named edge too — an adjacency count would
    // return it. Coverage does not: the edge is a rounding error of ITS
    // perimeter and half of the plate half's.
    const r = resolveRegionFaces(whole("P.bottom"), geom, faceAt, metrics);
    expect(r.seed).toBe(2);
    expect(r.faces).toEqual([2]);
  });

  it("keeps the plate when a second, unrelated edge is named too", () => {
    const r = resolveRegionFaces(whole("Other.right", "P.top"), geom, faceAt, metrics);
    expect(r.seed).toBe(3);
  });

  it("does not merge across an edge that is part of the boundary", () => {
    // Same four sides, plus the divider itself: now Onshape says the two halves
    // are separate regions, and the merge evidence must yield to that.
    const r = resolveRegionFaces(
      whole("P.bottom", "P.top", "P.left", "P.right", "P.mid"),
      geom,
      faceAt,
      metrics,
    );
    expect(r.conflict).toBe(true);
    expect(r.faces).toEqual([r.seed]);
  });

  it("trims a boundary edge to the piece between its imprint vertices", () => {
    // `P.left` fenced by `P.bottom` and `P.mid` is the LOWER half of that side
    // only — it never touches face 3, so nothing merges.
    const r = resolveRegionFaces(
      [
        { entityId: "P.bottom", ends: [] },
        { entityId: "P.left", ends: [["P.left", "P.bottom"], ["P.left", "P.mid"]] },
      ],
      geom,
      faceAt,
      metrics,
    );
    expect(r.faces).toEqual([2]);
  });

  it("returns nothing instead of substituting a sibling for an unknown id", () => {
    // `P.ghost` does not exist here. Falling back to "any entity sharing the
    // prefix" is what made #412 resolve to a block Onshape never extruded.
    expect(resolveRegionFaces(whole("P.ghost"), geom, faceAt, metrics)).toMatchObject({
      seed: null,
      unknownCurve: true,
    });
    expect(resolveRegionFaces(whole("P.bottom", "P.ghost"), geom, faceAt, metrics).seed).toBeNull();
    expect(resolveRegionFaces([], geom, faceAt, metrics).seed).toBeNull();
  });

  it("ignores a curve that runs THROUGH a face instead of bounding it", () => {
    // Both sides of a dangling trim line land in the same face; it says nothing
    // about that face, so no coverage is credited and nothing resolves.
    const g = new Map<string, [number, number][]>([["D", [[200, 200], [300, 200]]]]);
    expect(resolveRegionFaces(whole("D"), g, faceAt, metrics).seed).toBeNull();
  });

  it("returns nothing when every probe lands outside all regions", () => {
    expect(resolveRegionFaces(whole("P.bottom"), geom, () => null, metrics).seed).toBeNull();
  });

  it("returns nothing when the kernel reports no metrics for the candidate", () => {
    expect(resolveRegionFaces(whole("P.bottom"), geom, faceAt, () => null).seed).toBeNull();
  });

  it("lets Onshape's flank OVERRULE the coverage ranking (#442/#446)", () => {
    // `P.bottom` runs +x, so its LEFT is the plate half (face 2) and its RIGHT
    // is the sheet around it (face 0). Coverage elects the plate either way —
    // the edge is a rounding error of the sheet's perimeter. Onshape saying
    // „the region is on the RIGHT" is the only thing that can say otherwise,
    // and it must, or an extrude fills the hole it was fenced around.
    const edge = (side: number) => [{ entityId: "P.bottom", ends: [], side }];
    expect(resolveRegionFaces(edge(1), geom, faceAt, metrics).seed).toBe(2);
    expect(resolveRegionFaces(edge(-1), geom, faceAt, metrics).seed).toBe(0);
  });

  it("reads the flank through the sketch's handedness (#446)", () => {
    // In a MIRRORED basis left and right have traded places, and the sign is
    // known for every sketch — either the in-plane map the importer applied or,
    // for a payload taken literally, the identity's `+1` (the default here).
    const edge = [{ entityId: "P.bottom", ends: [], side: -1 }];
    expect(resolveRegionFaces(edge, geom, faceAt, metrics, -1).seed).toBe(2);
    expect(resolveRegionFaces(edge, geom, faceAt, metrics, 1).seed).toBe(0);
  });
});
