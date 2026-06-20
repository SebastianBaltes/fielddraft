import { describe, it, expect } from "vitest";
import {
  polylineLength,
  analyzeEdge,
  minDistanceBetweenPolylines,
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
