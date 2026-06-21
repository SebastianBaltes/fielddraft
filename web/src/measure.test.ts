import { describe, it, expect } from "vitest";
import {
  polylineLength,
  analyzeEdge,
  minDistanceBetweenPolylines,
  triangleSoupArea,
  triangleSoupCentroid,
  closestPointOnTriangle,
  minDistancePointToTriangles,
  minDistancePolylineToTriangles,
  minDistanceBetweenTriangleSoups,
  measureSingle,
  measureDistance,
  type MeasureGeom,
} from "./measure";

/** Flat-xyz samples of a circle of `r` about (cx,cy,z0) in the z-plane. */
function circle(r: number, cx: number, cy: number, z0: number, n = 48): number[] {
  const pts: number[] = [];
  for (let i = 0; i <= n; i++) {
    const a = (i / n) * Math.PI * 2;
    pts.push(cx + r * Math.cos(a), cy + r * Math.sin(a), z0);
  }
  return pts; // closed: last sample == first
}

describe("polylineLength", () => {
  it("sums segment lengths", () => {
    expect(polylineLength([0, 0, 0, 3, 0, 0, 3, 4, 0])).toBeCloseTo(7, 9);
  });
  it("is zero for a single point", () => {
    expect(polylineLength([1, 2, 3])).toBe(0);
  });
});

describe("analyzeEdge", () => {
  it("measures a straight edge's length, not closed, no circle", () => {
    const m = analyzeEdge([0, 0, 0, 0, 0, 110]);
    expect(m).not.toBeNull();
    expect(m!.length).toBeCloseTo(110, 6);
    expect(m!.closed).toBe(false);
    expect(m!.circle).toBeNull();
  });

  it("detects a circular rim and its radius/centre", () => {
    const m = analyzeEdge(circle(25, 10, -5, 40));
    expect(m).not.toBeNull();
    expect(m!.closed).toBe(true);
    expect(m!.circle).not.toBeNull();
    expect(m!.circle!.radius).toBeCloseTo(25, 1);
    expect(m!.circle!.center[0]).toBeCloseTo(10, 3);
    expect(m!.circle!.center[1]).toBeCloseTo(-5, 3);
    expect(m!.circle!.center[2]).toBeCloseTo(40, 3);
    // Circle lies in the z-plane → normal is ±z.
    expect(Math.abs(m!.circle!.normal[2])).toBeCloseTo(1, 2);
    // Circumference sanity.
    expect(m!.length).toBeCloseTo(2 * Math.PI * 25, 0);
  });

  it("does not call a closed square a circle", () => {
    const sq = [0, 0, 0, 10, 0, 0, 10, 10, 0, 0, 10, 0, 0, 0, 0];
    const m = analyzeEdge(sq);
    expect(m!.closed).toBe(true);
    expect(m!.circle).toBeNull();
  });
});

describe("minDistanceBetweenPolylines", () => {
  it("measures the axial gap between two coaxial rims", () => {
    // Two identical circles 76 mm apart in z → closest approach is the gap.
    const top = circle(25, 0, 0, 76);
    const bot = circle(25, 0, 0, 0);
    const d = minDistanceBetweenPolylines(top, bot);
    expect(d).not.toBeNull();
    expect(d!.min).toBeCloseTo(76, 1);
    expect(Math.abs(d!.delta[2])).toBeCloseTo(76, 1);
  });

  it("measures point-to-segment distance for crossing edges", () => {
    const a = [0, 0, 0, 0, 0, 10]; // along +z
    const b = [5, 0, 5, -5, 0, 5]; // crosses 5mm away in x at z=5
    const d = minDistanceBetweenPolylines(a, b);
    expect(d!.min).toBeCloseTo(0, 6); // segment b passes through (0,0,5)
  });

  it("zero distance when edges share a point", () => {
    const a = [0, 0, 0, 1, 0, 0];
    const b = [1, 0, 0, 1, 1, 0];
    expect(minDistanceBetweenPolylines(a, b)!.min).toBeCloseTo(0, 9);
  });
});

// ── ticket me9w: faces (triangle soups) and vertices ────────────────────

/** A unit-spaced grid quad in the z=z0 plane spanning [0,w]×[0,h] as two
 *  triangles (flat xyz, 9/tri). */
function quad(w: number, h: number, z0: number): number[] {
  // (0,0)-(w,0)-(w,h) and (0,0)-(w,h)-(0,h)
  return [
    0, 0, z0, w, 0, z0, w, h, z0,
    0, 0, z0, w, h, z0, 0, h, z0,
  ];
}

describe("triangleSoupArea / centroid", () => {
  it("sums triangle areas (a 3×4 rectangle = 12 mm²)", () => {
    expect(triangleSoupArea(quad(3, 4, 0))).toBeCloseTo(12, 9);
  });
  it("area-weighted centroid of a centred rectangle is its centre", () => {
    const tris = quad(10, 6, 2);
    const c = triangleSoupCentroid(tris);
    expect(c[0]).toBeCloseTo(5, 6);
    expect(c[1]).toBeCloseTo(3, 6);
    expect(c[2]).toBeCloseTo(2, 6);
  });
  it("returns 0 area for an empty soup", () => {
    expect(triangleSoupArea([])).toBe(0);
  });
});

describe("closestPointOnTriangle", () => {
  const A: [number, number, number] = [0, 0, 0];
  const B: [number, number, number] = [4, 0, 0];
  const C: [number, number, number] = [0, 4, 0];
  it("projects an interior point straight down to the plane", () => {
    const p = closestPointOnTriangle([1, 1, 5], A, B, C);
    expect(p).toEqual([1, 1, 0]);
  });
  it("clamps to the nearest vertex outside a corner", () => {
    const p = closestPointOnTriangle([-2, -3, 0], A, B, C);
    expect(p).toEqual([0, 0, 0]);
  });
  it("clamps to an edge for a point beyond it", () => {
    const p = closestPointOnTriangle([2, -1, 0], A, B, C);
    expect(p[0]).toBeCloseTo(2, 6);
    expect(p[1]).toBeCloseTo(0, 6);
  });
});

describe("minDistancePointToTriangles", () => {
  it("distance from a point above a face is the plane gap", () => {
    const d = minDistancePointToTriangles([5, 3, 7], quad(10, 6, 0));
    expect(d!.min).toBeCloseTo(7, 6);
    expect(Math.abs(d!.delta[2])).toBeCloseTo(7, 6);
  });
  it("distance to a face edge when the point is off to the side", () => {
    // point at (15, 3, 0) — beyond the x=10 edge of a 10×6 quad → gap = 5.
    const d = minDistancePointToTriangles([15, 3, 0], quad(10, 6, 0));
    expect(d!.min).toBeCloseTo(5, 6);
  });
});

describe("minDistancePolylineToTriangles", () => {
  it("edge parallel above a face → perpendicular gap", () => {
    const edge = [2, 2, 4, 8, 2, 4]; // along +x at height 4
    const d = minDistancePolylineToTriangles(edge, quad(10, 6, 0));
    expect(d!.min).toBeCloseTo(4, 4);
  });
});

describe("minDistanceBetweenTriangleSoups", () => {
  it("two parallel quads 9 mm apart in z", () => {
    const d = minDistanceBetweenTriangleSoups(quad(10, 6, 0), quad(10, 6, 9));
    expect(d!.min).toBeCloseTo(9, 6);
    expect(Math.abs(d!.delta[2])).toBeCloseTo(9, 6);
  });
  it("touching quads → zero distance", () => {
    const d = minDistanceBetweenTriangleSoups(quad(10, 6, 0), quad(10, 6, 0));
    expect(d!.min).toBeCloseTo(0, 6);
  });
});

describe("measureSingle", () => {
  it("vertex → its coordinates", () => {
    const m = measureSingle({ kind: "vertex", point: [1, 2, 3] });
    expect(m.kind).toBe("vertex");
    expect(m.position).toEqual([1, 2, 3]);
  });
  it("face → area + centroid + label", () => {
    const g: MeasureGeom = {
      kind: "face",
      triangles: quad(3, 4, 0),
      area: 12,
      centroid: [1.5, 2, 0],
      surfaceLabel: "Ebene",
    };
    const m = measureSingle(g);
    expect(m.area).toBeCloseTo(12, 9);
    expect(m.surfaceLabel).toBe("Ebene");
  });
});

describe("measureDistance — all kind combinations", () => {
  const vertex = (p: [number, number, number]): MeasureGeom => ({
    kind: "vertex",
    point: p,
  });
  const edge = (pts: number[]): MeasureGeom => ({ kind: "edge", points: pts });
  const face = (tris: number[]): MeasureGeom => ({
    kind: "face",
    triangles: tris,
    area: triangleSoupArea(tris),
    centroid: triangleSoupCentroid(tris),
    surfaceLabel: "Ebene",
  });

  it("point–point", () => {
    const d = measureDistance(vertex([0, 0, 0]), vertex([3, 4, 0]));
    expect(d!.min).toBeCloseTo(5, 9);
  });
  it("point–edge", () => {
    const d = measureDistance(vertex([5, 5, 0]), edge([0, 0, 0, 10, 0, 0]));
    expect(d!.min).toBeCloseTo(5, 6);
  });
  it("point–face", () => {
    const d = measureDistance(vertex([5, 3, 8]), face(quad(10, 6, 0)));
    expect(d!.min).toBeCloseTo(8, 6);
  });
  it("edge–edge", () => {
    const d = measureDistance(
      edge([0, 0, 0, 10, 0, 0]),
      edge([0, 7, 0, 10, 7, 0]),
    );
    expect(d!.min).toBeCloseTo(7, 6);
  });
  it("edge–face", () => {
    const d = measureDistance(edge([2, 2, 6, 8, 2, 6]), face(quad(10, 6, 0)));
    expect(d!.min).toBeCloseTo(6, 4);
  });
  it("face–face", () => {
    const d = measureDistance(face(quad(10, 6, 0)), face(quad(10, 6, 11)));
    expect(d!.min).toBeCloseTo(11, 6);
  });
  it("orientation: face on side A reports A→B delta sign consistently", () => {
    // vertex 8mm above a face, face passed as A.
    const d = measureDistance(face(quad(10, 6, 0)), vertex([5, 3, 8]));
    expect(d!.min).toBeCloseTo(8, 6);
    // a = closest point on face (z≈0), b = vertex (z=8) → delta.z ≈ +8
    expect(d!.delta[2]).toBeCloseTo(8, 6);
  });
});
