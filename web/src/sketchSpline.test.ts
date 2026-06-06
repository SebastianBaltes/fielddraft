import { describe, it, expect } from "vitest";
import { catmullRomPoints, type Vec2 } from "./sketchSpline";

const near = (a: Vec2, b: Vec2, eps = 1e-6) =>
  Math.hypot(a[0] - b[0], a[1] - b[1]) < eps;

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
