import { describe, it, expect } from "vitest";
import { bsplinePoints, catmullRomPoints, type Vec2 } from "./sketchSpline";

const near = (a: Vec2, b: Vec2, eps = 1e-6) =>
  Math.hypot(a[0] - b[0], a[1] - b[1]) < eps;

/** Min distance from a point to the sampled polyline. */
const distToPolyline = (poly: Vec2[], q: Vec2): number => {
  let best = Infinity;
  for (let i = 0; i + 1 < poly.length; i++) {
    const a = poly[i];
    const b = poly[i + 1];
    const dx = b[0] - a[0];
    const dy = b[1] - a[1];
    const len2 = dx * dx + dy * dy;
    const t =
      len2 < 1e-12
        ? 0
        : Math.max(0, Math.min(1, ((q[0] - a[0]) * dx + (q[1] - a[1]) * dy) / len2));
    const px = a[0] + t * dx;
    const py = a[1] + t * dy;
    best = Math.min(best, Math.hypot(q[0] - px, q[1] - py));
  }
  return best;
};

describe("catmullRomPoints", () => {
  it("interpolates (passes through) every control point — open", () => {
    const pts: Vec2[] = [
      [0, 0],
      [10, 5],
      [20, 0],
      [30, 5],
    ];
    const steps = 8;
    const sampled = catmullRomPoints(pts, false, steps);
    // For each control point there must be a sample lying on it.
    for (const cp of pts) {
      expect(sampled.some((s) => near(s, cp, 1e-4))).toBe(true);
    }
    // First and last samples are the endpoints.
    expect(near(sampled[0], pts[0])).toBe(true);
    expect(near(sampled[sampled.length - 1], pts[pts.length - 1])).toBe(true);
  });

  it("produces stepsPerSpan·spans + 1 points for an open spline", () => {
    const pts: Vec2[] = [
      [0, 0],
      [10, 0],
      [20, 0],
      [30, 0],
    ];
    const steps = 12;
    const spans = pts.length - 1; // 3
    const sampled = catmullRomPoints(pts, false, steps);
    expect(sampled.length).toBe(steps * spans + 1);
  });

  it("wraps and produces stepsPerSpan·N + 1 points for a closed spline", () => {
    const pts: Vec2[] = [
      [0, 10],
      [10, 0],
      [0, -10],
      [-10, 0],
    ];
    const steps = 10;
    const sampled = catmullRomPoints(pts, true, steps);
    expect(sampled.length).toBe(steps * pts.length + 1);
    // The closed curve still interpolates every control point.
    for (const cp of pts) {
      expect(sampled.some((s) => near(s, cp, 1e-4))).toBe(true);
    }
    // The wrap returns to the first control point.
    expect(near(sampled[sampled.length - 1], pts[0], 1e-4)).toBe(true);
  });

  it("behaves like a straight line for exactly 2 control points", () => {
    const a: Vec2 = [0, 0];
    const b: Vec2 = [10, 4];
    const sampled = catmullRomPoints([a, b], false, 8);
    expect(sampled.length).toBe(9);
    // Every sample lies on the segment a→b (zero perpendicular dist).
    for (const s of sampled) {
      const t = (s[0] - a[0]) / (b[0] - a[0]);
      const expectedY = a[1] + t * (b[1] - a[1]);
      expect(Math.abs(s[1] - expectedY)).toBeLessThan(1e-6);
    }
  });

  it("keeps collinear control points ~straight", () => {
    const pts: Vec2[] = [
      [0, 0],
      [10, 0],
      [20, 0],
      [30, 0],
    ];
    const sampled = catmullRomPoints(pts, false, 8);
    for (const s of sampled) {
      expect(Math.abs(s[1])).toBeLessThan(1e-6);
    }
  });

  it("returns [] for fewer than 2 control points (edge)", () => {
    expect(catmullRomPoints([], false, 8)).toEqual([]);
    expect(catmullRomPoints([[1, 2]], false, 8)).toEqual([]);
  });

  it("closed with 3 control points wraps into a loop", () => {
    const pts: Vec2[] = [
      [0, 0],
      [10, 0],
      [5, 8],
    ];
    const steps = 6;
    const sampled = catmullRomPoints(pts, true, steps);
    expect(sampled.length).toBe(steps * 3 + 1);
    for (const cp of pts) {
      expect(sampled.some((s) => near(s, cp, 1e-4))).toBe(true);
    }
    expect(near(sampled[sampled.length - 1], pts[0], 1e-4)).toBe(true);
  });

  it("clamps stepsPerSpan to at least 1", () => {
    const pts: Vec2[] = [
      [0, 0],
      [10, 0],
    ];
    const sampled = catmullRomPoints(pts, false, 0);
    expect(sampled.length).toBe(2);
  });
});

describe("bsplinePoints (de Boor, control-point cubic B-spline)", () => {
  it("clamped open spline starts/ends at the first/last control point", () => {
    const cp: Vec2[] = [
      [0, 0],
      [10, 20],
      [20, -20],
      [30, 0],
      [40, 10],
    ];
    const out = bsplinePoints(cp, 3, false, 64);
    expect(near(out[0], cp[0], 1e-6)).toBe(true);
    expect(near(out[out.length - 1], cp[cp.length - 1], 1e-6)).toBe(true);
  });

  it("produces samples + 1 points", () => {
    const cp: Vec2[] = [
      [0, 0],
      [10, 10],
      [20, 0],
      [30, 10],
    ];
    expect(bsplinePoints(cp, 3, false, 50).length).toBe(51);
    expect(bsplinePoints(cp, 3, true, 80).length).toBe(81);
  });

  it("mid value matches an independent de Boor evaluation", () => {
    // Hand-reference: a uniform clamped cubic B-spline with control
    // points P0..P4. At u = 0.5 (mid of the [0,2] knot domain) the de
    // Boor value is computed below directly; the sampler must hit it.
    const cp: Vec2[] = [
      [0, 0],
      [10, 30],
      [30, 30],
      [40, 0],
      [50, 20],
    ];
    // Reference de Boor at u=0.5 over clamped knots [0,0,0,0,1,2,2,2,2].
    const deBoorRef = (u: number): Vec2 => {
      const knots = [0, 0, 0, 0, 1, 2, 2, 2, 2];
      const degree = 3;
      const n = cp.length;
      let k = degree;
      while (k + 1 <= n - 1 && knots[k + 1] <= u) k++;
      const d: Vec2[] = [];
      for (let j = 0; j <= degree; j++) d.push(cp[Math.min(k - degree + j, n - 1)]);
      for (let r = 1; r <= degree; r++) {
        for (let j = degree; j >= r; j--) {
          const i = k - degree + j;
          const denom = knots[i + degree - r + 1] - knots[i];
          const a = denom === 0 ? 0 : (u - knots[i]) / denom;
          d[j] = [(1 - a) * d[j - 1][0] + a * d[j][0], (1 - a) * d[j - 1][1] + a * d[j][1]];
        }
      }
      return d[degree];
    };
    const ref = deBoorRef(1.0); // u=1.0 is the midpoint of [0,2]
    const out = bsplinePoints(cp, 3, false, 200);
    expect(distToPolyline(out, ref)).toBeLessThan(0.05);
  });

  it("closed spline is periodic (forms a loop, first ≈ last sample)", () => {
    const cp: Vec2[] = [
      [-10, -10],
      [10, -10],
      [10, 10],
      [-10, 10],
    ];
    const out = bsplinePoints(cp, 3, true, 120);
    expect(out.length).toBe(121);
    // The loop closes: the last sample returns near the first.
    expect(Math.hypot(out[0][0] - out[out.length - 1][0], out[0][1] - out[out.length - 1][1])).toBeLessThan(0.2);
    // Centroid (0,0) is roughly enclosed: samples span all quadrants.
    const hasPos = out.some((p) => p[0] > 0 && p[1] > 0);
    const hasNeg = out.some((p) => p[0] < 0 && p[1] < 0);
    expect(hasPos && hasNeg).toBe(true);
  });

  it("two control points → a straight line", () => {
    const a: Vec2 = [0, 0];
    const b: Vec2 = [10, 4];
    const out = bsplinePoints([a, b], 3, false, 8);
    expect(out.length).toBe(9);
    for (const s of out) {
      const t = (s[0] - a[0]) / (b[0] - a[0]);
      expect(Math.abs(s[1] - (a[1] + t * (b[1] - a[1])))).toBeLessThan(1e-9);
    }
  });

  it("three control points → degree-2, still clamped to endpoints", () => {
    const cp: Vec2[] = [
      [0, 0],
      [5, 10],
      [10, 0],
    ];
    const out = bsplinePoints(cp, 3, false, 32);
    expect(near(out[0], cp[0], 1e-6)).toBe(true);
    expect(near(out[out.length - 1], cp[2], 1e-6)).toBe(true);
    // Bows up toward the middle control point.
    const mid = out[Math.floor(out.length / 2)];
    expect(mid[1]).toBeGreaterThan(1);
  });

  it("collinear control points stay straight (no NaN)", () => {
    const cp: Vec2[] = [
      [0, 0],
      [10, 0],
      [20, 0],
      [30, 0],
    ];
    const out = bsplinePoints(cp, 3, false, 40);
    for (const s of out) {
      expect(Math.abs(s[1])).toBeLessThan(1e-9);
      expect(Number.isFinite(s[0]) && Number.isFinite(s[1])).toBe(true);
    }
  });

  it("returns [] for fewer than 2 controls (edge)", () => {
    expect(bsplinePoints([], 3, false, 8)).toEqual([]);
    expect(bsplinePoints([[1, 2]], 3, false, 8)).toEqual([]);
  });
});
