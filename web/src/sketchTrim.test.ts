import { describe, expect, it } from "vitest";
import {
  computeTrim,
  curveOfSubCall,
  distToCurve,
  intersectCurves,
  nearestTrimTarget,
  trimCurveAt,
  type SubCall,
  type Vec2,
} from "./sketchTrim";

const line = (a: Vec2, b: Vec2, extra: Record<string, unknown> = {}): SubCall => ({
  type: "sketch_line",
  params: { start: a, end: b, ...extra },
});
const circle = (c: Vec2, r: number, extra: Record<string, unknown> = {}): SubCall => ({
  type: "sketch_circle",
  params: { center: c, radius: r, ...extra },
});
const arc = (
  c: Vec2,
  r: number,
  start: number,
  sweep: number,
  extra: Record<string, unknown> = {},
): SubCall => ({
  type: "sketch_arc",
  params: { center: c, radius: r, start_angle: start, sweep_angle: sweep, ...extra },
});

const near = (a: number, b: number, tol = 1e-6): boolean => Math.abs(a - b) <= tol;
const pt = (call: SubCall, key: string): Vec2 => call.params[key] as Vec2;

describe("curveOfSubCall", () => {
  it("parses line / circle / arc and rejects non-trimmable types", () => {
    expect(curveOfSubCall(line([0, 0], [1, 1]))?.kind).toBe("line");
    expect(curveOfSubCall(circle([0, 0], 5))?.kind).toBe("circle");
    expect(curveOfSubCall(arc([0, 0], 5, 0, Math.PI))?.kind).toBe("arc");
    expect(curveOfSubCall({ type: "sketch_rectangle", params: {} })).toBeNull();
    expect(curveOfSubCall({ type: "sketch_point", params: { at: [0, 0] } })).toBeNull();
  });
});

describe("intersectCurves", () => {
  it("crossing segments meet at one point", () => {
    const pts = intersectCurves(
      curveOfSubCall(line([-10, 0], [10, 0]))!,
      curveOfSubCall(line([0, -10], [0, 10]))!,
    );
    expect(pts).toHaveLength(1);
    expect(near(pts[0][0], 0)).toBe(true);
    expect(near(pts[0][1], 0)).toBe(true);
  });

  it("parallel segments do not meet", () => {
    const pts = intersectCurves(
      curveOfSubCall(line([0, 0], [10, 0]))!,
      curveOfSubCall(line([0, 5], [10, 5]))!,
    );
    expect(pts).toHaveLength(0);
  });

  it("segments whose infinite lines cross but extents don't, miss", () => {
    const pts = intersectCurves(
      curveOfSubCall(line([-10, 0], [-5, 0]))!,
      curveOfSubCall(line([0, -10], [0, 10]))!,
    );
    expect(pts).toHaveLength(0);
  });

  it("line through a circle yields two points", () => {
    const pts = intersectCurves(
      curveOfSubCall(line([-20, 0], [20, 0]))!,
      curveOfSubCall(circle([0, 0], 10))!,
    );
    expect(pts).toHaveLength(2);
    const xs = pts.map((p) => p[0]).sort((a, b) => a - b);
    expect(near(xs[0], -10)).toBe(true);
    expect(near(xs[1], 10)).toBe(true);
  });

  it("line tangent to a circle yields one point", () => {
    const pts = intersectCurves(
      curveOfSubCall(line([-20, 10], [20, 10]))!,
      curveOfSubCall(circle([0, 0], 10))!,
    );
    expect(pts).toHaveLength(1);
    expect(near(pts[0][1], 10)).toBe(true);
  });

  it("two overlapping circles meet at two points", () => {
    const pts = intersectCurves(
      curveOfSubCall(circle([0, 0], 10))!,
      curveOfSubCall(circle([10, 0], 10))!,
    );
    expect(pts).toHaveLength(2);
    expect(near(pts[0][0], 5)).toBe(true);
    expect(near(pts[1][0], 5)).toBe(true);
  });

  it("disjoint circles do not meet", () => {
    const pts = intersectCurves(
      curveOfSubCall(circle([0, 0], 5))!,
      curveOfSubCall(circle([100, 0], 5))!,
    );
    expect(pts).toHaveLength(0);
  });

  it("a line crossing a circle outside the arc span is clamped away", () => {
    // Top-half arc (0 → π). The x-axis crosses the full circle at
    // (±10, 0); only points on the arc survive — here, neither (both
    // sit on the arc's endpoints, filtered as the seam) so use a line
    // that only crosses the top.
    const pts = intersectCurves(
      curveOfSubCall(arc([0, 0], 10, 0, Math.PI))!,
      curveOfSubCall(line([0, -20], [0, 20]))!,
    );
    // Vertical line crosses circle at (0, ±10); only (0, 10) is on the
    // top arc.
    expect(pts).toHaveLength(1);
    expect(near(pts[0][1], 10)).toBe(true);
  });
});

describe("distToCurve / nearestTrimTarget", () => {
  it("measures perpendicular distance to a segment", () => {
    expect(near(distToCurve([5, 3], curveOfSubCall(line([0, 0], [10, 0]))!), 3)).toBe(true);
  });
  it("measures radial distance to a circle", () => {
    expect(near(distToCurve([0, 12], curveOfSubCall(circle([0, 0], 10))!), 2)).toBe(true);
  });
  it("an arc's empty side is not near", () => {
    const a = curveOfSubCall(arc([0, 0], 10, 0, Math.PI))!; // top half
    // A click just outside the bottom (empty) side is far (≈ endpoint dist),
    // not ≈0.
    expect(distToCurve([0, -10], a)).toBeGreaterThan(5);
    // A click just outside the top (filled) side is radial-near.
    expect(near(distToCurve([0, 11], a), 1)).toBe(true);
  });
  it("picks the nearest trimmable sub-call, ignoring others", () => {
    const calls = [
      line([-10, 0], [10, 0]),
      circle([0, 50], 5),
      { type: "sketch_point", params: { at: [0, 0] } },
    ];
    expect(nearestTrimTarget(calls, [0, 0.5], 2)).toBe(0);
    expect(nearestTrimTarget(calls, [0, 46], 3)).toBe(1);
    expect(nearestTrimTarget(calls, [100, 100], 2)).toBeNull();
  });
});

describe("trimCurveAt — lines", () => {
  it("deletes an isolated line (no crossings)", () => {
    const calls = [line([-10, 0], [10, 0])];
    expect(trimCurveAt(calls, 0, [0, 0])).toEqual([]);
  });

  it("shortens a line to the crossing when an overhang is clicked", () => {
    const calls = [
      line([-20, 0], [20, 0]),
      line([-10, -5], [-10, 5]), // crosses target at x = -10 (t = 0.25)
    ];
    // Click on the right overhang.
    const out = trimCurveAt(calls, 0, [10, 0])!;
    expect(out).toHaveLength(1);
    expect(near(pt(out[0], "start")[0], -20)).toBe(true);
    expect(near(pt(out[0], "end")[0], -10)).toBe(true);
  });

  it("splits a line into two when the middle is clicked", () => {
    const calls = [
      line([-20, 0], [20, 0]),
      line([-10, -5], [-10, 5]), // t = 0.25
      line([10, -5], [10, 5]), // t = 0.75
    ];
    const out = trimCurveAt(calls, 0, [0, 0])!;
    expect(out).toHaveLength(2);
    expect(near(pt(out[0], "start")[0], -20)).toBe(true);
    expect(near(pt(out[0], "end")[0], -10)).toBe(true);
    expect(near(pt(out[1], "start")[0], 10)).toBe(true);
    expect(near(pt(out[1], "end")[0], 20)).toBe(true);
  });

  it("carries the construction flag onto the trimmed pieces", () => {
    const calls = [
      line([-20, 0], [20, 0], { construction: true }),
      line([0, -5], [0, 5]),
    ];
    const out = trimCurveAt(calls, 0, [10, 0])!;
    expect(out).toHaveLength(1);
    expect(out[0].params.construction).toBe(true);
  });
});

describe("trimCurveAt — circles", () => {
  it("deletes a circle with no crossings", () => {
    expect(trimCurveAt([circle([0, 0], 10)], 0, [10, 0])).toEqual([]);
  });

  it("turns a circle into the complementary arc (click top → keep bottom)", () => {
    const calls = [
      circle([0, 0], 10),
      line([-20, 0], [20, 0]), // crosses at (±10, 0): angles 0 and π
    ];
    const out = trimCurveAt(calls, 0, [0, 10])!; // click the top
    expect(out).toHaveLength(1);
    expect(out[0].type).toBe("sketch_arc");
    // Kept arc must contain the bottom point (0, -10) and not the top.
    const a = curveOfSubCall(out[0])!;
    expect(distToCurve([0, -10], a)).toBeLessThan(1e-6);
    expect(distToCurve([0, 10], a)).toBeGreaterThan(5);
  });

  it("click bottom → keep top", () => {
    const calls = [circle([0, 0], 10), line([-20, 0], [20, 0])];
    const out = trimCurveAt(calls, 0, [0, -10])!;
    const a = curveOfSubCall(out[0])!;
    expect(distToCurve([0, 10], a)).toBeLessThan(1e-6);
    expect(distToCurve([0, -10], a)).toBeGreaterThan(5);
  });
});

describe("trimCurveAt — arcs", () => {
  it("shortens an arc at an interior crossing", () => {
    // Top-half arc 0→π, crossed at the apex (0, 10) by a vertical line.
    const calls = [
      arc([0, 0], 10, 0, Math.PI),
      line([0, -20], [0, 20]), // crosses arc at (0, 10), s = 0.5
    ];
    // Click the right part of the arc (near (10, 0)).
    const out = trimCurveAt(calls, 0, [10, 0])!;
    expect(out).toHaveLength(1);
    const a = curveOfSubCall(out[0])!;
    // Right part removed → left part (around (-10,0)) kept.
    expect(distToCurve([-10, 0.0001], a)).toBeLessThan(1e-2);
    expect(distToCurve([10, 0.0001], a)).toBeGreaterThan(5);
  });

  it("deletes an arc with no interior crossings", () => {
    expect(trimCurveAt([arc([0, 0], 10, 0, Math.PI)], 0, [10, 0])).toEqual([]);
  });
});

describe("computeTrim", () => {
  it("returns the target index and replacement, or null when off-curve", () => {
    const calls = [line([-20, 0], [20, 0]), line([0, -5], [0, 5])];
    const res = computeTrim(calls, [10, 0], 1)!;
    expect(res.index).toBe(0);
    expect(res.newCalls).toHaveLength(1);
    expect(computeTrim(calls, [200, 200], 1)).toBeNull();
  });
});
