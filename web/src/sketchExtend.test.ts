import { describe, expect, it } from "vitest";
import { extendLine, type Curve, type Vec2 } from "./sketchExtend";

/** Assert two points are equal within a hair. */
function expectVec(p: Vec2, q: Vec2, tol = 1e-6): void {
  expect(Math.hypot(p[0] - q[0], p[1] - q[1])).toBeLessThan(tol);
}

const line = (a: Vec2, b: Vec2): Curve => ({ kind: "line", a, b });

describe("extendLine", () => {
  it("extends the right end to a vertical line on the right", () => {
    // Short horizontal segment (0,0)→(5,0); a vertical line at x=10.
    const horiz = line([0, 0], [5, 0]);
    const vert = line([10, -5], [10, 5]);
    // Click near the RIGHT end (5,0) → move that end out to (10,0).
    const out = extendLine(horiz, [4.9, 0], [vert]);
    expect(out).not.toBeNull();
    expect(out!.kind).toBe("line");
    if (out!.kind !== "line") return;
    expectVec(out!.a, [0, 0]); // anchor preserved
    expectVec(out!.b, [10, 0]); // grabbed end pushed to the crossing
  });

  it("extends the left end leftward to a curve on the left", () => {
    const horiz = line([0, 0], [5, 0]);
    const leftVert = line([-7, -5], [-7, 5]);
    // Click near the LEFT end (0,0) → move it out to (-7,0).
    const out = extendLine(horiz, [0.1, 0], [leftVert]);
    expect(out).not.toBeNull();
    if (out!.kind !== "line") return;
    expectVec(out!.a, [-7, 0]); // grabbed (left) end moved
    expectVec(out!.b, [5, 0]); // right end preserved
  });

  it("returns null when grabbing the left end but the only curve is on the right", () => {
    const horiz = line([0, 0], [5, 0]);
    const vert = line([10, -5], [10, 5]); // to the RIGHT
    // Grab the LEFT end → extension points left, the vertical is behind it.
    const out = extendLine(horiz, [0.1, 0], [vert]);
    expect(out).toBeNull();
  });

  it("returns null when no curve lies in the extension direction", () => {
    const horiz = line([0, 0], [5, 0]);
    const far = line([0, 20], [5, 20]); // parallel, never crossed
    const out = extendLine(horiz, [4.9, 0], [far]);
    expect(out).toBeNull();
  });

  it("returns null when the only intersection is behind the moving end", () => {
    // Grab right end; the crossing sits to the LEFT of it (wrong direction).
    const horiz = line([0, 0], [5, 0]);
    const behind = line([2, -5], [2, 5]); // crosses the segment itself at x=2
    const out = extendLine(horiz, [4.9, 0], [behind]);
    expect(out).toBeNull();
  });

  it("returns null when the crossing is beyond the other curve's bounds", () => {
    // The infinite support would cross the vertical LINE at (10,0), but the
    // vertical segment only spans y∈[5,8] — it never reaches y=0, so a miss.
    const horiz = line([0, 0], [5, 0]);
    const shortVert = line([10, 5], [10, 8]);
    const out = extendLine(horiz, [4.9, 0], [shortVert]);
    expect(out).toBeNull();
  });

  it("returns null for a non-line target", () => {
    const circle: Curve = { kind: "circle", c: [0, 0], r: 3 };
    const vert = line([10, -5], [10, 5]);
    const out = extendLine(circle, [3, 0], [vert]);
    expect(out).toBeNull();
  });

  it("extends to the NEAR crossing of a circle", () => {
    // Horizontal seg (0,0)→(5,0); circle centred (10,0) r=2 → crossings at
    // x=8 and x=12. Extending the right end forward stops at the nearer, 8.
    const horiz = line([0, 0], [5, 0]);
    const circle: Curve = { kind: "circle", c: [10, 0], r: 2 };
    const out = extendLine(horiz, [4.9, 0], [circle]);
    expect(out).not.toBeNull();
    if (out!.kind !== "line") return;
    expectVec(out!.b, [8, 0]);
    expectVec(out!.a, [0, 0]);
  });

  it("picks the NEAREST of several forward crossings", () => {
    const horiz = line([0, 0], [5, 0]);
    const near = line([8, -3], [8, 3]);
    const far = line([20, -3], [20, 3]);
    const out = extendLine(horiz, [4.9, 0], [far, near]);
    expect(out).not.toBeNull();
    if (out!.kind !== "line") return;
    expectVec(out!.b, [8, 0]); // stops at the nearer crossing
  });

  it("clamps the crossing onto an arc's drawn span", () => {
    // Circle (10,0) r=2 would cross at x=8 and x=12. An arc covering only
    // the RIGHT side (around angle 0, i.e. the x=12 point) is reached at 12,
    // because the x=8 point (angle π) is off the arc span.
    const horiz = line([0, 0], [5, 0]);
    // Arc centred (10,0) r=2 spanning −π/4 .. +π/4 (right cap only).
    const arc: Curve = {
      kind: "arc",
      c: [10, 0],
      r: 2,
      start: -Math.PI / 4,
      sweep: Math.PI / 2,
    };
    const out = extendLine(horiz, [4.9, 0], [arc]);
    expect(out).not.toBeNull();
    if (out!.kind !== "line") return;
    expectVec(out!.b, [12, 0]); // x=8 is off the arc, so the far cap wins
  });

  it("returns null for a zero-length line", () => {
    const degenerate = line([3, 3], [3, 3]);
    const vert = line([10, -5], [10, 5]);
    const out = extendLine(degenerate, [3, 3], [vert]);
    expect(out).toBeNull();
  });
});
