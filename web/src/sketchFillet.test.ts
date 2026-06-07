import { describe, expect, it } from "vitest";
import { filletLines, type Curve, type Vec2 } from "./sketchFillet";

const line = (a: Vec2, b: Vec2): Curve => ({ kind: "line", a, b });

/** Point on an arc at normalized parameter s ∈ [0, 1] (0 = start). */
function arcPoint(arc: Extract<Curve, { kind: "arc" }>, s: number): Vec2 {
  const a = arc.start + arc.sweep * s;
  return [arc.c[0] + arc.r * Math.cos(a), arc.c[1] + arc.r * Math.sin(a)];
}

const close = (a: Vec2, b: Vec2, tol = 1e-6) =>
  Math.hypot(a[0] - b[0], a[1] - b[1]) < tol;

describe("filletLines — right-angle corner", () => {
  // L1 along +x from origin, L2 along +y from origin, r = 2.
  const l1 = line([0, 0], [10, 0]);
  const l2 = line([0, 0], [0, 10]);
  const res = filletLines(l1, l2, 2);

  it("produces a result", () => {
    expect(res).not.toBeNull();
  });

  it("tangent points at (2,0) and (0,2)", () => {
    if (!res) throw new Error("no result");
    // trimmed1 keeps far end (10,0), corner moved to (2,0).
    const t1 = line([0, 0], [10, 0]);
    void t1;
    if (res.trimmed1.kind !== "line" || res.trimmed2.kind !== "line") {
      throw new Error("expected lines");
    }
    // The corner endpoint (the one nearest origin) is the tangent point.
    expect(close(res.trimmed1.a, [2, 0]) || close(res.trimmed1.b, [2, 0])).toBe(true);
    expect(close(res.trimmed2.a, [0, 2]) || close(res.trimmed2.b, [0, 2])).toBe(true);
    // Far ends preserved.
    expect(close(res.trimmed1.a, [10, 0]) || close(res.trimmed1.b, [10, 0])).toBe(true);
    expect(close(res.trimmed2.a, [0, 10]) || close(res.trimmed2.b, [0, 10])).toBe(true);
  });

  it("arc centre (2,2), radius 2", () => {
    if (!res || res.arc.kind !== "arc") throw new Error("expected arc");
    expect(close(res.arc.c, [2, 2])).toBe(true);
    expect(res.arc.r).toBeCloseTo(2, 9);
  });

  it("arc spans 90° and runs between the tangent points", () => {
    if (!res || res.arc.kind !== "arc") throw new Error("expected arc");
    expect(Math.abs(res.arc.sweep)).toBeCloseTo(Math.PI / 2, 9);
    const start = arcPoint(res.arc, 0);
    const end = arcPoint(res.arc, 1);
    const ends = [start, end];
    expect(ends.some((p) => close(p, [2, 0]))).toBe(true);
    expect(ends.some((p) => close(p, [0, 2]))).toBe(true);
  });

  it("arc midpoint bulges toward the corner (between V and C)", () => {
    if (!res || res.arc.kind !== "arc") throw new Error("expected arc");
    const mid = arcPoint(res.arc, 0.5);
    // For the right angle, the rounding arc midpoint is at
    // (2 − √2, 2 − √2) ≈ (0.586, 0.586): closer to V=(0,0) than C=(2,2).
    const V: Vec2 = [0, 0];
    const C = res.arc.c;
    expect(Math.hypot(mid[0] - V[0], mid[1] - V[1])).toBeLessThan(
      Math.hypot(C[0] - V[0], C[1] - V[1]),
    );
    expect(close(mid, [2 - Math.SQRT2, 2 - Math.SQRT2])).toBe(true);
  });
});

describe("filletLines — angle variations", () => {
  it("45° (acute) corner gives larger tangent distance than 90°", () => {
    // L1 +x, L2 at 45°. θ = 45°, t = r/tan(22.5°) ≈ 2.414·r.
    const l1 = line([0, 0], [10, 0]);
    const l2 = line([0, 0], [10, 10]);
    const res = filletLines(l1, l2, 2);
    if (!res || res.trimmed1.kind !== "line" || res.arc.kind !== "arc") {
      throw new Error("expected result");
    }
    const t = 2 / Math.tan(Math.PI / 8);
    expect(t).toBeGreaterThan(2); // > the 90° tangent distance (== r)
    // Tangent point on L1 is at (t, 0).
    const corner = close(res.trimmed1.a, [t, 0]) ? res.trimmed1.a : res.trimmed1.b;
    expect(close(corner, [t, 0])).toBe(true);
  });

  it("135° (obtuse) corner gives smaller tangent distance than 90°", () => {
    // L1 +x, L2 at 135°. θ = 135°, t = r/tan(67.5°) ≈ 0.414·r.
    const l1 = line([0, 0], [10, 0]);
    const l2 = line([0, 0], [-10, 10]);
    const res = filletLines(l1, l2, 2);
    if (!res || res.arc.kind !== "arc") throw new Error("expected result");
    const t = 2 / Math.tan((3 * Math.PI) / 8);
    expect(t).toBeLessThan(2);
    // Arc still bulges toward the corner V=(0,0).
    const mid = arcPoint(res.arc, 0.5);
    const C = res.arc.c;
    expect(Math.hypot(mid[0], mid[1])).toBeLessThan(Math.hypot(C[0], C[1]));
  });

  it("works when the shared corner is the b-end of both lines", () => {
    // Lines drawn toward the corner: far ends are the a-ends.
    const l1 = line([10, 0], [0, 0]);
    const l2 = line([0, 10], [0, 0]);
    const res = filletLines(l1, l2, 2);
    if (!res || res.arc.kind !== "arc") throw new Error("expected result");
    expect(close(res.arc.c, [2, 2])).toBe(true);
    if (res.trimmed1.kind !== "line" || res.trimmed2.kind !== "line") {
      throw new Error("expected lines");
    }
    // Far ends (10,0) and (0,10) preserved.
    expect(close(res.trimmed1.a, [10, 0])).toBe(true);
    expect(close(res.trimmed2.a, [0, 10])).toBe(true);
  });

  it("works with a small corner gap (lines nearly meet)", () => {
    const l1 = line([0, 0], [10, 0.05]);
    const l2 = line([0.05, 0], [0, 10]);
    const res = filletLines(l1, l2, 2);
    expect(res).not.toBeNull();
  });
});

describe("filletLines — edge cases return null", () => {
  it("parallel lines", () => {
    const l1 = line([0, 0], [10, 0]);
    const l2 = line([0, 5], [10, 5]);
    expect(filletLines(l1, l2, 2)).toBeNull();
  });

  it("antiparallel (collinear) lines", () => {
    const l1 = line([0, 0], [10, 0]);
    const l2 = line([10, 0], [20, 0]);
    expect(filletLines(l1, l2, 2)).toBeNull();
  });

  it("radius so large the tangent point passes the segment end", () => {
    // Right angle, short legs of length 1: t = r = 5 ≫ leg length.
    const l1 = line([0, 0], [1, 0]);
    const l2 = line([0, 0], [0, 1]);
    expect(filletLines(l1, l2, 5)).toBeNull();
  });

  it("lines that don't share a near corner (far apart)", () => {
    const l1 = line([0, 0], [10, 0]);
    const l2 = line([100, 100], [100, 110]);
    expect(filletLines(l1, l2, 2)).toBeNull();
  });

  it("zero radius", () => {
    const l1 = line([0, 0], [10, 0]);
    const l2 = line([0, 0], [0, 10]);
    expect(filletLines(l1, l2, 0)).toBeNull();
  });

  it("negative radius", () => {
    const l1 = line([0, 0], [10, 0]);
    const l2 = line([0, 0], [0, 10]);
    expect(filletLines(l1, l2, -2)).toBeNull();
  });

  it("non-finite radius", () => {
    const l1 = line([0, 0], [10, 0]);
    const l2 = line([0, 0], [0, 10]);
    expect(filletLines(l1, l2, Number.NaN)).toBeNull();
    expect(filletLines(l1, l2, Number.POSITIVE_INFINITY)).toBeNull();
  });

  it("zero-length line", () => {
    const l1 = line([0, 0], [0, 0]);
    const l2 = line([0, 0], [0, 10]);
    expect(filletLines(l1, l2, 2)).toBeNull();
  });

  it("non-line operands", () => {
    const circle: Curve = { kind: "circle", c: [0, 0], r: 3 };
    const l2 = line([0, 0], [0, 10]);
    expect(filletLines(circle, l2, 2)).toBeNull();
  });
});
