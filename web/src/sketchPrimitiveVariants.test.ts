import { describe, it, expect } from "vitest";
import {
  circleThrough3,
  rectCenterCorners,
  rect3pCorners,
  type Vec2,
} from "./sketchPrimitiveVariants";

describe("circleThrough3", () => {
  it("recovers a known unit circle centred at the origin", () => {
    const res = circleThrough3([1, 0], [0, 1], [-1, 0]);
    expect(res).not.toBeNull();
    expect(res!.center[0]).toBeCloseTo(0, 10);
    expect(res!.center[1]).toBeCloseTo(0, 10);
    expect(res!.radius).toBeCloseTo(1, 10);
  });

  it("recovers an off-origin circle", () => {
    // Circle centre (2, -3), radius 5 → points on it.
    const res = circleThrough3([7, -3], [2, 2], [-3, -3]);
    expect(res).not.toBeNull();
    expect(res!.center[0]).toBeCloseTo(2, 8);
    expect(res!.center[1]).toBeCloseTo(-3, 8);
    expect(res!.radius).toBeCloseTo(5, 8);
  });

  it("returns null for collinear points (no finite circumcircle)", () => {
    expect(circleThrough3([0, 0], [1, 1], [2, 2])).toBeNull();
    expect(circleThrough3([0, 0], [5, 0], [10, 0])).toBeNull();
  });

  it("returns null for coincident points", () => {
    expect(circleThrough3([1, 1], [1, 1], [3, 4])).toBeNull();
  });
});

describe("rectCenterCorners", () => {
  it("center (0,0) corner (5,3) → corners (-5,-3) and (5,3)", () => {
    const res = rectCenterCorners([0, 0], [5, 3]);
    expect(res).not.toBeNull();
    expect(res!.cornerA).toEqual([-5, -3]);
    expect(res!.cornerB).toEqual([5, 3]);
  });

  it("handles negative deltas (drag into the lower-left quadrant)", () => {
    const res = rectCenterCorners([2, 2], [-3, -1]);
    expect(res).not.toBeNull();
    // |dx| = 5, |dy| = 3
    expect(res!.cornerA).toEqual([-3, -1]);
    expect(res!.cornerB).toEqual([7, 5]);
  });

  it("returns null for a zero-area rectangle", () => {
    expect(rectCenterCorners([0, 0], [0, 4])).toBeNull();
    expect(rectCenterCorners([0, 0], [4, 0])).toBeNull();
    expect(rectCenterCorners([1, 1], [1, 1])).toBeNull();
  });
});

describe("rect3pCorners", () => {
  it("axis-aligned case yields the expected 4 corners", () => {
    // Edge p1→p2 along +x, width 2 upward.
    const corners = rect3pCorners([0, 0], [4, 0], [4, 2]);
    expect(corners).not.toBeNull();
    const [a, b, c, d] = corners as [Vec2, Vec2, Vec2, Vec2];
    expect(a).toEqual([0, 0]);
    expect(b).toEqual([4, 0]);
    expect(c[0]).toBeCloseTo(4, 10);
    expect(c[1]).toBeCloseTo(2, 10);
    expect(d[0]).toBeCloseTo(0, 10);
    expect(d[1]).toBeCloseTo(2, 10);
  });

  it("uses only the perpendicular component of the 3rd point", () => {
    // p3 slid along the edge direction should not change the width.
    const corners = rect3pCorners([0, 0], [4, 0], [10, 2]);
    expect(corners).not.toBeNull();
    const [, , c, d] = corners as [Vec2, Vec2, Vec2, Vec2];
    expect(c[1]).toBeCloseTo(2, 10);
    expect(d[1]).toBeCloseTo(2, 10);
  });

  it("45°-rotated case yields the expected 4 corners", () => {
    // Edge along (1,1)/√2, length √2; p3 sets width 1 to the left normal.
    const s = Math.SQRT2;
    const corners = rect3pCorners([0, 0], [1, 1], [0, 1]);
    expect(corners).not.toBeNull();
    const [a, b, c, d] = corners as [Vec2, Vec2, Vec2, Vec2];
    // Unit normal of (1,1) is (-1,1)/√2; w = dot(p3-p2, n) = dot((-1,0),n)=1/√2.
    const w = 1 / s;
    expect(a).toEqual([0, 0]);
    expect(b).toEqual([1, 1]);
    expect(c[0]).toBeCloseTo(1 + w * (-1 / s), 10);
    expect(c[1]).toBeCloseTo(1 + w * (1 / s), 10);
    expect(d[0]).toBeCloseTo(0 + w * (-1 / s), 10);
    expect(d[1]).toBeCloseTo(0 + w * (1 / s), 10);
    // Right angles: edge·width-edge ≈ 0.
    const e1 = [b[0] - a[0], b[1] - a[1]];
    const e2 = [d[0] - a[0], d[1] - a[1]];
    expect(e1[0] * e2[0] + e1[1] * e2[1]).toBeCloseTo(0, 10);
  });

  it("returns null for a zero-length edge", () => {
    expect(rect3pCorners([1, 1], [1, 1], [3, 4])).toBeNull();
  });

  it("returns null for zero width (3rd point on the edge line)", () => {
    expect(rect3pCorners([0, 0], [4, 0], [2, 0])).toBeNull();
  });
});
