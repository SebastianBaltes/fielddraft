import { describe, expect, it } from "vitest";
import type { Curve } from "./sketchTrim";
import {
  patternCircular,
  patternLinear,
  rotateCurve,
  translateCurve,
} from "./sketchPattern";

const TAU = Math.PI * 2;
const near = (a: number, b: number, eps = 1e-9): boolean => Math.abs(a - b) < eps;

describe("translateCurve", () => {
  it("shifts both line endpoints", () => {
    const cv: Curve = { kind: "line", a: [0, 0], b: [10, 0] };
    const out = translateCurve(cv, 5, 3);
    expect(out).toEqual({ kind: "line", a: [5, 3], b: [15, 3] });
  });

  it("shifts a circle centre, radius unchanged", () => {
    const cv: Curve = { kind: "circle", c: [1, 2], r: 7 };
    expect(translateCurve(cv, 10, 0)).toEqual({ kind: "circle", c: [11, 2], r: 7 });
  });

  it("shifts an arc centre WITHOUT touching its angles", () => {
    const cv: Curve = { kind: "arc", c: [0, 0], r: 4, start: 0.5, sweep: 1.2 };
    const out = translateCurve(cv, -2, 6);
    expect(out).toEqual({ kind: "arc", c: [-2, 6], r: 4, start: 0.5, sweep: 1.2 });
  });
});

describe("rotateCurve", () => {
  it("rotates both line endpoints about the centre", () => {
    const cv: Curve = { kind: "line", a: [1, 0], b: [2, 0] };
    const out = rotateCurve(cv, [0, 0], Math.PI / 2);
    if (out.kind !== "line") throw new Error("kind");
    expect(near(out.a[0], 0)).toBe(true);
    expect(near(out.a[1], 1)).toBe(true);
    expect(near(out.b[0], 0)).toBe(true);
    expect(near(out.b[1], 2)).toBe(true);
  });

  it("rotates a circle centre, radius unchanged", () => {
    const cv: Curve = { kind: "circle", c: [10, 0], r: 3 };
    const out = rotateCurve(cv, [0, 0], Math.PI / 2);
    if (out.kind !== "circle") throw new Error("kind");
    expect(near(out.c[0], 0)).toBe(true);
    expect(near(out.c[1], 10)).toBe(true);
    expect(out.r).toBe(3);
  });

  it("rotates an arc centre AND adds the angle to start, sweep unchanged", () => {
    const cv: Curve = { kind: "arc", c: [10, 0], r: 2, start: 0.3, sweep: 1.0 };
    const out = rotateCurve(cv, [0, 0], Math.PI / 2);
    if (out.kind !== "arc") throw new Error("kind");
    expect(near(out.c[0], 0)).toBe(true);
    expect(near(out.c[1], 10)).toBe(true);
    expect(near(out.start, 0.3 + Math.PI / 2)).toBe(true);
    expect(out.sweep).toBe(1.0);
  });
});

describe("patternLinear", () => {
  it("a circle at origin, count 3, spacing (20,0) → copies at (20,0) and (40,0)", () => {
    const cv: Curve = { kind: "circle", c: [0, 0], r: 5 };
    const out = patternLinear([cv], 3, 20, 0);
    expect(out).toHaveLength(2);
    expect(out[0]).toEqual({ kind: "circle", c: [20, 0], r: 5 });
    expect(out[1]).toEqual({ kind: "circle", c: [40, 0], r: 5 });
  });

  it("count 1 → []", () => {
    const cv: Curve = { kind: "circle", c: [0, 0], r: 5 };
    expect(patternLinear([cv], 1, 20, 0)).toEqual([]);
  });

  it("count < 2 (zero / negative) → []", () => {
    const cv: Curve = { kind: "line", a: [0, 0], b: [1, 0] };
    expect(patternLinear([cv], 0, 10, 0)).toEqual([]);
    expect(patternLinear([cv], -3, 10, 0)).toEqual([]);
  });

  it("negative spacing steps in the opposite direction", () => {
    const cv: Curve = { kind: "circle", c: [0, 0], r: 1 };
    const out = patternLinear([cv], 3, 0, -20);
    expect(out).toHaveLength(2);
    expect(out[0]).toEqual({ kind: "circle", c: [0, -20], r: 1 });
    expect(out[1]).toEqual({ kind: "circle", c: [0, -40], r: 1 });
  });

  it("emits all copies of each curve in input order", () => {
    const a: Curve = { kind: "circle", c: [0, 0], r: 1 };
    const b: Curve = { kind: "circle", c: [0, 100], r: 1 };
    const out = patternLinear([a, b], 2, 10, 0);
    expect(out).toHaveLength(2);
    expect(out[0]).toEqual({ kind: "circle", c: [10, 0], r: 1 });
    expect(out[1]).toEqual({ kind: "circle", c: [10, 100], r: 1 });
  });
});

describe("patternCircular", () => {
  it("a circle at (10,0), count 4, full 360° → 3 copies at 90/180/270°", () => {
    const cv: Curve = { kind: "circle", c: [10, 0], r: 2 };
    const out = patternCircular([cv], 4, [0, 0], TAU);
    expect(out).toHaveLength(3);
    const expected = [
      [0, 10],
      [-10, 0],
      [0, -10],
    ];
    out.forEach((c, i) => {
      if (c.kind !== "circle") throw new Error("kind");
      expect(near(c.c[0], expected[i][0])).toBe(true);
      expect(near(c.c[1], expected[i][1])).toBe(true);
      expect(c.r).toBe(2);
    });
  });

  it("full circle step = 2π/count (last copy does NOT overlap the original)", () => {
    const cv: Curve = { kind: "circle", c: [10, 0], r: 1 };
    const out = patternCircular([cv], 4, [0, 0], TAU);
    // last copy is at 270°, not back at (10,0)
    const last = out[out.length - 1];
    if (last.kind !== "circle") throw new Error("kind");
    expect(near(last.c[0], 0)).toBe(true);
    expect(near(last.c[1], -10)).toBe(true);
  });

  it("count 3, 180° span → step 90° (inclusive: last copy at 180°)", () => {
    const cv: Curve = { kind: "circle", c: [10, 0], r: 1 };
    const out = patternCircular([cv], 3, [0, 0], Math.PI);
    expect(out).toHaveLength(2);
    // step = 180/(3-1) = 90°
    const c0 = out[0];
    const c1 = out[1];
    if (c0.kind !== "circle" || c1.kind !== "circle") throw new Error("kind");
    expect(near(c0.c[0], 0)).toBe(true);
    expect(near(c0.c[1], 10)).toBe(true); // 90°
    expect(near(c1.c[0], -10)).toBe(true);
    expect(near(c1.c[1], 0)).toBe(true); // 180°
  });

  it("count < 2 → []", () => {
    const cv: Curve = { kind: "circle", c: [10, 0], r: 1 };
    expect(patternCircular([cv], 1, [0, 0], TAU)).toEqual([]);
    expect(patternCircular([cv], 0, [0, 0], Math.PI)).toEqual([]);
  });

  it("rotates arcs by adding to start_angle", () => {
    const cv: Curve = { kind: "arc", c: [10, 0], r: 2, start: 0, sweep: 0.5 };
    const out = patternCircular([cv], 3, [0, 0], Math.PI);
    expect(out).toHaveLength(2);
    const a0 = out[0];
    if (a0.kind !== "arc") throw new Error("kind");
    expect(near(a0.start, Math.PI / 2)).toBe(true);
    expect(a0.sweep).toBe(0.5);
  });
});
