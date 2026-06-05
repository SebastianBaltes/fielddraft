import { describe, expect, it } from "vitest";
import {
  angleAbout,
  buildArcSubCall,
  MIN_ARC_SWEEP_RAD,
  sampleArc,
  unwrapSweep,
  type Vec2,
} from "./sketchArc";

const HALF_PI = Math.PI / 2;
const PI = Math.PI;
const TAU = 2 * Math.PI;

describe("angleAbout", () => {
  it("measures CCW from +x in sketch space (Y-up)", () => {
    const c: Vec2 = [0, 0];
    expect(angleAbout(c, [1, 0])).toBeCloseTo(0);
    expect(angleAbout(c, [0, 1])).toBeCloseTo(HALF_PI);
    expect(angleAbout(c, [-1, 0])).toBeCloseTo(PI);
    expect(angleAbout(c, [0, -1])).toBeCloseTo(-HALF_PI);
  });
});

describe("unwrapSweep", () => {
  it("returns the short signed delta when prevSweep is 0", () => {
    expect(unwrapSweep(0, HALF_PI, 0)).toBeCloseTo(HALF_PI);
    expect(unwrapSweep(0, -HALF_PI, 0)).toBeCloseTo(-HALF_PI);
  });

  it("stays continuous past the +180° seam instead of flipping sign", () => {
    // Was sweeping CCW near +π; the next sample wraps just past π. Naively
    // that normalizes to ≈ -π (a sign flip); unwrap keeps it positive.
    const startAngle = 0;
    const prev = PI - 0.1; // almost half a turn, CCW
    const endAngle = -(PI - 0.05); // ≈ +π + 0.05 expressed in (-π,π]
    const sweep = unwrapSweep(startAngle, endAngle, prev);
    expect(sweep).toBeGreaterThan(prev);
    expect(sweep).toBeCloseTo(PI + 0.05);
  });

  it("clamps to a single full revolution", () => {
    expect(unwrapSweep(0, 0.0, TAU * 5)).toBeLessThanOrEqual(TAU + 1e-9);
    expect(unwrapSweep(0, 0.0, -TAU * 5)).toBeGreaterThanOrEqual(-TAU - 1e-9);
  });
});

describe("buildArcSubCall", () => {
  it("derives centre, radius, start angle and sweep from three points", () => {
    const call = buildArcSubCall([0, 0], [10, 0], [0, 10], 0);
    expect(call).not.toBeNull();
    expect(call!.type).toBe("sketch_arc");
    expect(call!.params.center).toEqual([0, 0]);
    expect(call!.params.radius).toBeCloseTo(10);
    expect(call!.params.start_angle).toBeCloseTo(0);
    // end at +y → quarter turn CCW
    expect(call!.params.sweep_angle).toBeCloseTo(HALF_PI);
  });

  it("keeps radius from the start point, ignoring the end point's distance", () => {
    // end point is far off the circle — only its angle matters.
    const call = buildArcSubCall([0, 0], [5, 0], [0, 99], 0);
    expect(call!.params.radius).toBeCloseTo(5);
    expect(call!.params.sweep_angle).toBeCloseTo(HALF_PI);
  });

  it("rejects a zero-radius arc", () => {
    expect(buildArcSubCall([3, 3], [3, 3], [9, 9], 0)).toBeNull();
  });

  it("rejects a zero-sweep arc", () => {
    // end collinear with start (same angle) → no sweep
    const call = buildArcSubCall([0, 0], [10, 0], [20, 0], 0);
    expect(call).toBeNull();
  });

  it("honours an accumulated sweep past 180°", () => {
    const call = buildArcSubCall([0, 0], [10, 0], [10, 0], PI + 1);
    // start and end at the same angle, but prevSweep carries the winding
    expect(call).not.toBeNull();
    expect(call!.params.sweep_angle).toBeGreaterThan(PI);
  });
});

describe("sampleArc", () => {
  it("returns endpoints that match the start/end angles", () => {
    const pts = sampleArc([0, 0], 10, 0, HALF_PI);
    expect(pts.length).toBeGreaterThanOrEqual(3);
    expect(pts[0][0]).toBeCloseTo(10);
    expect(pts[0][1]).toBeCloseTo(0);
    const last = pts[pts.length - 1];
    expect(last[0]).toBeCloseTo(0);
    expect(last[1]).toBeCloseTo(10);
  });

  it("subdivides a large sweep into many smooth segments", () => {
    const few = sampleArc([0, 0], 10, 0, MIN_ARC_SWEEP_RAD);
    const many = sampleArc([0, 0], 10, 0, TAU);
    expect(many.length).toBeGreaterThan(few.length);
  });
});
