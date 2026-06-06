import { describe, it, expect } from "vitest";
import { splitCurveAt } from "./sketchSplit";
import type { Curve, Vec2 } from "./sketchTrim";

const close = (a: number, b: number, eps = 1e-9) => Math.abs(a - b) < eps;
const vecClose = (p: Vec2, q: Vec2, eps = 1e-9) =>
  close(p[0], q[0], eps) && close(p[1], q[1], eps);

describe("splitCurveAt — line", () => {
  const line: Curve = { kind: "line", a: [0, 0], b: [10, 0] };

  it("splits a horizontal line at a mid point into two abutting lines", () => {
    const res = splitCurveAt(line, [4, 0]);
    expect(res).not.toBeNull();
    const [p1, p2] = res!;
    expect(p1.kind).toBe("line");
    expect(p2.kind).toBe("line");
    if (p1.kind !== "line" || p2.kind !== "line") throw new Error("kind");
    expect(vecClose(p1.a, [0, 0])).toBe(true);
    expect(vecClose(p1.b, [4, 0])).toBe(true);
    // pieces meet at the split point
    expect(vecClose(p1.b, p2.a)).toBe(true);
    expect(vecClose(p2.a, [4, 0])).toBe(true);
    expect(vecClose(p2.b, [10, 0])).toBe(true);
  });

  it("splits at the projection when the click is off the line", () => {
    // click above the segment at x=6 projects onto (6,0)
    const res = splitCurveAt(line, [6, 3]);
    expect(res).not.toBeNull();
    const [p1, p2] = res!;
    if (p1.kind !== "line" || p2.kind !== "line") throw new Error("kind");
    expect(vecClose(p1.b, [6, 0])).toBe(true);
    expect(vecClose(p2.a, [6, 0])).toBe(true);
  });

  it("returns null for a click at / near the start endpoint", () => {
    expect(splitCurveAt(line, [0, 0])).toBeNull();
    expect(splitCurveAt(line, [1e-9, 0])).toBeNull();
  });

  it("returns null for a click at / near the end endpoint", () => {
    expect(splitCurveAt(line, [10, 0])).toBeNull();
    expect(splitCurveAt(line, [10 - 1e-9, 0])).toBeNull();
  });

  it("returns null for a degenerate zero-length line", () => {
    const deg: Curve = { kind: "line", a: [5, 5], b: [5, 5] };
    expect(splitCurveAt(deg, [5, 5])).toBeNull();
  });
});

describe("splitCurveAt — arc", () => {
  // Quarter arc on the unit circle: 0 → π/2, CCW.
  const arc: Curve = {
    kind: "arc",
    c: [0, 0],
    r: 1,
    start: 0,
    sweep: Math.PI / 2,
  };

  it("splits a quarter arc at its midpoint angle into two arcs", () => {
    // midpoint of the sweep is at θ = π/4 → click on the arc there
    const mid: Vec2 = [Math.cos(Math.PI / 4), Math.sin(Math.PI / 4)];
    const res = splitCurveAt(arc, mid);
    expect(res).not.toBeNull();
    const [a1, a2] = res!;
    if (a1.kind !== "arc" || a2.kind !== "arc") throw new Error("kind");

    // same centre / radius
    expect(vecClose(a1.c, [0, 0])).toBe(true);
    expect(vecClose(a2.c, [0, 0])).toBe(true);
    expect(close(a1.r, 1)).toBe(true);
    expect(close(a2.r, 1)).toBe(true);

    // sub-sweeps sum to the original and share its sign
    expect(close(a1.sweep + a2.sweep, arc.sweep)).toBe(true);
    expect(Math.sign(a1.sweep)).toBe(Math.sign(arc.sweep));
    expect(Math.sign(a2.sweep)).toBe(Math.sign(arc.sweep));

    // they meet at the midpoint angle
    expect(close(a1.start, 0)).toBe(true);
    expect(close(a1.start + a1.sweep, Math.PI / 4)).toBe(true);
    expect(close(a2.start, Math.PI / 4)).toBe(true);
    expect(close(a2.start + a2.sweep, Math.PI / 2)).toBe(true);
  });

  it("preserves sweep sign for a clockwise (negative-sweep) arc", () => {
    const cw: Curve = { kind: "arc", c: [0, 0], r: 1, start: 0, sweep: -Math.PI / 2 };
    // midpoint is at θ = -π/4
    const mid: Vec2 = [Math.cos(-Math.PI / 4), Math.sin(-Math.PI / 4)];
    const res = splitCurveAt(cw, mid);
    expect(res).not.toBeNull();
    const [a1, a2] = res!;
    if (a1.kind !== "arc" || a2.kind !== "arc") throw new Error("kind");
    expect(close(a1.sweep + a2.sweep, cw.sweep)).toBe(true);
    expect(a1.sweep).toBeLessThan(0);
    expect(a2.sweep).toBeLessThan(0);
  });

  it("returns null when the click is at the arc start endpoint", () => {
    const startPt: Vec2 = [1, 0];
    expect(splitCurveAt(arc, startPt)).toBeNull();
  });

  it("returns null when the click is at the arc end endpoint", () => {
    const endPt: Vec2 = [0, 1];
    expect(splitCurveAt(arc, endPt)).toBeNull();
  });

  it("returns null when the click angle is outside the arc span", () => {
    // θ = π (opposite side) is not on a 0→π/2 quarter arc
    expect(splitCurveAt(arc, [-1, 0])).toBeNull();
  });
});

describe("splitCurveAt — unsupported", () => {
  it("returns null for a full circle", () => {
    const circle: Curve = { kind: "circle", c: [0, 0], r: 5 };
    expect(splitCurveAt(circle, [5, 0])).toBeNull();
  });
});
