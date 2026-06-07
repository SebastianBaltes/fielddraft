import { describe, expect, it } from "vitest";
import { mirrorCurve, reflectPoint } from "./sketchMirror";
import type { Curve, Vec2 } from "./sketchTrim";

const close = (a: number, b: number, eps = 1e-9) => Math.abs(a - b) < eps;
const closeV = (a: Vec2, b: Vec2, eps = 1e-9) =>
  close(a[0], b[0], eps) && close(a[1], b[1], eps);

/** Point on a circle/arc at angle theta. */
const ptAt = (c: Vec2, r: number, theta: number): Vec2 => [
  c[0] + r * Math.cos(theta),
  c[1] + r * Math.sin(theta),
];

describe("reflectPoint", () => {
  it("reflects across the Y axis (A=(0,0), B=(0,1))", () => {
    expect(reflectPoint([3, 2], [0, 0], [0, 1])).toEqual([-3, 2]);
  });

  it("reflects across the X axis (A=(0,0), B=(1,0))", () => {
    expect(reflectPoint([3, 2], [0, 0], [1, 0])).toEqual([3, -2]);
  });

  it("reflects across the 45° line y = x (A=(0,0), B=(1,1))", () => {
    const r = reflectPoint([3, 2], [0, 0], [1, 1]);
    expect(closeV(r, [2, 3])).toBe(true);
  });

  it("reflects across an offset vertical axis x = 5", () => {
    // mirror of x=2 across x=5 is x=8, y unchanged.
    const r = reflectPoint([2, 7], [5, 0], [5, -3]);
    expect(closeV(r, [8, 7])).toBe(true);
  });

  it("leaves a point ON the axis approximately in place", () => {
    // (2,2) lies on y = x.
    const r = reflectPoint([2, 2], [0, 0], [1, 1]);
    expect(closeV(r, [2, 2])).toBe(true);
  });

  it("reflecting twice returns the original point (involution)", () => {
    const p: Vec2 = [4, -1];
    const a: Vec2 = [1, 2];
    const b: Vec2 = [3, 5];
    const once = reflectPoint(p, a, b);
    const twice = reflectPoint(once, a, b);
    expect(closeV(twice, p)).toBe(true);
  });

  it("throws on a degenerate axis (A == B)", () => {
    expect(() => reflectPoint([1, 1], [2, 2], [2, 2])).toThrow();
  });
});

describe("mirrorCurve", () => {
  it("line: reflects both endpoints across the Y axis", () => {
    const cv: Curve = { kind: "line", a: [1, 2], b: [4, 5] };
    const m = mirrorCurve(cv, [0, 0], [0, 1]);
    expect(m.kind).toBe("line");
    if (m.kind !== "line") throw new Error("kind");
    expect(closeV(m.a, [-1, 2])).toBe(true);
    expect(closeV(m.b, [-4, 5])).toBe(true);
  });

  it("circle: reflects centre, radius unchanged", () => {
    const cv: Curve = { kind: "circle", c: [3, 4], r: 2.5 };
    const m = mirrorCurve(cv, [0, 0], [0, 1]);
    expect(m.kind).toBe("circle");
    if (m.kind !== "circle") throw new Error("kind");
    expect(closeV(m.c, [-3, 4])).toBe(true);
    expect(close(m.r, 2.5)).toBe(true);
  });

  it("arc: passes through reflected start/end points, sweep sign flips", () => {
    const axisA: Vec2 = [0, 0];
    const axisB: Vec2 = [0, 1]; // Y axis
    const cv: Curve & { kind: "arc" } = {
      kind: "arc",
      c: [2, 1],
      r: 3,
      start: 0.3,
      sweep: 1.2, // CCW
    };
    const origStart = ptAt(cv.c, cv.r, cv.start);
    const origEnd = ptAt(cv.c, cv.r, cv.start + cv.sweep);

    const m = mirrorCurve(cv, axisA, axisB);
    expect(m.kind).toBe("arc");
    if (m.kind !== "arc") throw new Error("kind");

    // sweep sign flipped, magnitude preserved
    expect(close(Math.abs(m.sweep), Math.abs(cv.sweep))).toBe(true);
    expect(Math.sign(m.sweep)).toBe(-Math.sign(cv.sweep));

    // radius invariant
    expect(close(m.r, cv.r)).toBe(true);

    // reflected arc's sampled start/end equal reflections of orig start/end
    const mStart = ptAt(m.c, m.r, m.start);
    const mEnd = ptAt(m.c, m.r, m.start + m.sweep);
    expect(closeV(mStart, reflectPoint(origStart, axisA, axisB), 1e-7)).toBe(true);
    expect(closeV(mEnd, reflectPoint(origEnd, axisA, axisB), 1e-7)).toBe(true);
  });

  it("arc: works across a 45° axis (sampled midpoint reflects correctly)", () => {
    const axisA: Vec2 = [0, 0];
    const axisB: Vec2 = [1, 1];
    const cv: Curve & { kind: "arc" } = {
      kind: "arc",
      c: [4, 1],
      r: 2,
      start: -0.5,
      sweep: -0.9, // CW
    };
    const m = mirrorCurve(cv, axisA, axisB);
    if (m.kind !== "arc") throw new Error("kind");
    expect(Math.sign(m.sweep)).toBe(-Math.sign(cv.sweep));
    // sample a few interior parameters and verify they reflect onto each other
    for (const s of [0, 0.25, 0.5, 0.75, 1]) {
      const orig = ptAt(cv.c, cv.r, cv.start + s * cv.sweep);
      const refl = reflectPoint(orig, axisA, axisB);
      // the reflected arc, traversed at the SAME fraction s, must hit refl
      const got = ptAt(m.c, m.r, m.start + s * m.sweep);
      expect(closeV(got, refl, 1e-7)).toBe(true);
    }
  });

  it("throws (via reflectPoint) on a degenerate axis", () => {
    const cv: Curve = { kind: "line", a: [1, 2], b: [4, 5] };
    expect(() => mirrorCurve(cv, [0, 0], [0, 0])).toThrow();
  });
});
