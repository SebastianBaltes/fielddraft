import { describe, it, expect } from "vitest";
import { offsetCurve } from "./sketchOffset";
import type { Curve } from "./sketchTrim";

describe("offsetCurve — line", () => {
  const line: Curve = { kind: "line", a: [0, 0], b: [10, 0] };

  it("offsets toward a click above the line (+y side)", () => {
    const out = offsetCurve(line, 2, [5, 5]);
    expect(out).not.toBeNull();
    expect(out!.kind).toBe("line");
    if (out!.kind !== "line") throw new Error("expected line");
    expect(out!.a[1]).toBeCloseTo(2, 9);
    expect(out!.b[1]).toBeCloseTo(2, 9);
    expect(out!.a[0]).toBeCloseTo(0, 9);
    expect(out!.b[0]).toBeCloseTo(10, 9);
  });

  it("offsets toward a click below the line (−y side)", () => {
    const out = offsetCurve(line, 2, [5, -5]);
    expect(out).not.toBeNull();
    if (out!.kind !== "line") throw new Error("expected line");
    expect(out!.a[1]).toBeCloseTo(-2, 9);
    expect(out!.b[1]).toBeCloseTo(-2, 9);
  });

  it("a click exactly on the line picks the deterministic +n side", () => {
    // +n side of a→b = (10,0) is +y (n = (−dy,dx)/len = (0,1)).
    const out = offsetCurve(line, 2, [5, 0]);
    expect(out).not.toBeNull();
    if (out!.kind !== "line") throw new Error("expected line");
    expect(out!.a[1]).toBeCloseTo(2, 9);
    expect(out!.b[1]).toBeCloseTo(2, 9);
  });

  it("respects the segment direction for the normal sign", () => {
    // Reversed line b→a: n flips, so a click above now maps to −n side.
    const rev: Curve = { kind: "line", a: [10, 0], b: [0, 0] };
    const out = offsetCurve(rev, 2, [5, 5]);
    expect(out).not.toBeNull();
    if (out!.kind !== "line") throw new Error("expected line");
    expect(out!.a[1]).toBeCloseTo(2, 9);
    expect(out!.b[1]).toBeCloseTo(2, 9);
  });
});

describe("offsetCurve — circle", () => {
  const circle: Curve = { kind: "circle", c: [0, 0], r: 5 };

  it("grows the radius when the click is outside", () => {
    const out = offsetCurve(circle, 2, [10, 0]);
    expect(out).not.toBeNull();
    if (out!.kind !== "circle") throw new Error("expected circle");
    expect(out!.r).toBeCloseTo(7, 9);
    expect(out!.c).toEqual([0, 0]);
  });

  it("shrinks the radius when the click is inside", () => {
    const out = offsetCurve(circle, 2, [1, 0]);
    expect(out).not.toBeNull();
    if (out!.kind !== "circle") throw new Error("expected circle");
    expect(out!.r).toBeCloseTo(3, 9);
    expect(out!.c).toEqual([0, 0]);
  });

  it("returns null when an inner offset collapses the circle", () => {
    expect(offsetCurve(circle, 6, [1, 0])).toBeNull();
  });
});

describe("offsetCurve — arc", () => {
  const arc: Curve = {
    kind: "arc",
    c: [0, 0],
    r: 5,
    start: 0,
    sweep: Math.PI / 2,
  };

  it("grows the radius when the click is outside, keeping centre/angles", () => {
    const out = offsetCurve(arc, 2, [10, 0]);
    expect(out).not.toBeNull();
    if (out!.kind !== "arc") throw new Error("expected arc");
    expect(out!.r).toBeCloseTo(7, 9);
    expect(out!.c).toEqual([0, 0]);
    expect(out!.start).toBeCloseTo(0, 9);
    expect(out!.sweep).toBeCloseTo(Math.PI / 2, 9);
  });

  it("shrinks the radius when the click is inside, keeping centre/angles", () => {
    const out = offsetCurve(arc, 2, [1, 1]);
    expect(out).not.toBeNull();
    if (out!.kind !== "arc") throw new Error("expected arc");
    expect(out!.r).toBeCloseTo(3, 9);
    expect(out!.c).toEqual([0, 0]);
    expect(out!.start).toBeCloseTo(0, 9);
    expect(out!.sweep).toBeCloseTo(Math.PI / 2, 9);
  });

  it("returns null when an inner offset collapses the arc", () => {
    expect(offsetCurve(arc, 6, [1, 1])).toBeNull();
  });
});

describe("offsetCurve — edge cases", () => {
  const line: Curve = { kind: "line", a: [0, 0], b: [10, 0] };

  it("returns null for distance <= 0", () => {
    expect(offsetCurve(line, 0, [5, 5])).toBeNull();
    expect(offsetCurve(line, -2, [5, 5])).toBeNull();
  });

  it("returns null for a non-finite distance", () => {
    expect(offsetCurve(line, NaN, [5, 5])).toBeNull();
    expect(offsetCurve(line, Infinity, [5, 5])).toBeNull();
  });

  it("returns null for a zero-length line", () => {
    const dot: Curve = { kind: "line", a: [3, 3], b: [3, 3] };
    expect(offsetCurve(dot, 2, [5, 5])).toBeNull();
  });
});
