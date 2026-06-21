import { describe, expect, it } from "vitest";
import { snapToCircles, type SnapCircle, type Vec2 } from "./sketchCircleSnap";

const circle = (
  center: Vec2,
  radius: number,
  extra: Partial<SnapCircle> = {},
): SnapCircle => ({ id: 10, centerId: 11, center, radius, ...extra });

const near = (a: number, b: number, tol = 1e-6): boolean => Math.abs(a - b) <= tol;

describe("snapToCircles — rim projection", () => {
  it("projects a cursor just outside the rim onto the rim", () => {
    const snap = snapToCircles([circle([0, 0], 10)], [12, 0], 5)!;
    expect(snap.kind).toBe("quadrant"); // (10,0) is a quadrant, wins over rim
    expect(near(snap.at[0], 10)).toBe(true);
    expect(near(snap.at[1], 0)).toBe(true);
  });

  it("projects an off-axis cursor radially onto the rim", () => {
    // 45°-ish direction, away from any quadrant.
    const snap = snapToCircles([circle([0, 0], 10)], [8, 8], 5)!;
    expect(snap.kind).toBe("rim");
    // Lands on the rim: |at| == radius, same direction as the cursor.
    expect(near(Math.hypot(snap.at[0], snap.at[1]), 10)).toBe(true);
    expect(near(snap.at[0], snap.at[1])).toBe(true); // 45° → x == y
  });

  it("returns null when the cursor is far from every circle", () => {
    expect(snapToCircles([circle([0, 0], 10)], [50, 50], 5)).toBeNull();
  });
});

describe("snapToCircles — feature points", () => {
  it("snaps to the centre when near it (beats a far rim)", () => {
    const snap = snapToCircles([circle([0, 0], 10)], [1, 0.5], 5)!;
    expect(snap.kind).toBe("center");
    expect(snap.at).toEqual([0, 0]);
    expect(snap.curve.centerId).toBe(11);
  });

  it("snaps to a quadrant in preference to a plain rim point", () => {
    // Cursor near (0,10) — the top quadrant — but slightly off-axis.
    const snap = snapToCircles([circle([0, 0], 10)], [0.4, 11], 5)!;
    expect(snap.kind).toBe("quadrant");
    expect(near(snap.at[0], 0)).toBe(true);
    expect(near(snap.at[1], 10)).toBe(true);
  });
});

describe("snapToCircles — arcs", () => {
  const topArc = (extra: Partial<SnapCircle> = {}) =>
    circle([0, 0], 10, { start: 0, sweep: Math.PI, ...extra }); // 0 → π (top)

  it("snaps onto the live span of an arc", () => {
    const snap = snapToCircles([topArc()], [7, 7.5], 5)!;
    expect(snap.kind).toBe("rim");
    expect(near(Math.hypot(snap.at[0], snap.at[1]), 10)).toBe(true);
    expect(snap.at[1]).toBeGreaterThan(0); // on the top half
  });

  it("does not rim-snap onto the empty side of an arc", () => {
    // Cursor below the centre: angle ≈ -90°, off the top arc. The bottom
    // quadrant is also off-span, so no snap within a tight radius.
    expect(snapToCircles([topArc()], [0, -11], 3)).toBeNull();
  });

  it("still offers the centre snap for an arc", () => {
    const snap = snapToCircles([topArc()], [0.5, 0.5], 5)!;
    expect(snap.kind).toBe("center");
  });

  it("offers only the quadrants that lie on the arc span", () => {
    // Top arc 0→π contains quadrants 0°, 90°, 180°, but not 270°.
    const bottom = snapToCircles([topArc()], [0.3, -10], 5);
    // Nearest off-span point is the bottom quadrant (0,-10) which is excluded;
    // rim is off-span too → no snap.
    expect(bottom).toBeNull();
    const left = snapToCircles([topArc()], [-10, 0.3], 5)!;
    expect(left.kind).toBe("quadrant");
    expect(near(left.at[0], -10)).toBe(true);
  });
});

describe("snapToCircles — multiple circles", () => {
  it("picks the nearest snap across several circles", () => {
    const snap = snapToCircles(
      [circle([0, 0], 10, { id: 1 }), circle([100, 0], 10, { id: 2 })],
      [108, 0],
      5,
    )!;
    expect(snap.curve.id).toBe(2);
  });

  it("ignores degenerate (zero-radius) circles", () => {
    expect(snapToCircles([circle([0, 0], 0)], [0.001, 0], 5)).toBeNull();
  });
});
