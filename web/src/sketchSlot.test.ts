import { describe, expect, it } from "vitest";
import { slotOutline } from "./sketchSlot";
import type { Vec2 } from "./sketchArc";

const PI = Math.PI;
const close = (a: number, b: number, eps = 1e-6) => Math.abs(a - b) < eps;
const closeVec = (a: Vec2, b: Vec2, eps = 1e-6) =>
  close(a[0], b[0], eps) && close(a[1], b[1], eps);

describe("slotOutline", () => {
  it("horizontal slot: corners flank the axis, caps bulge along ±x", () => {
    // A=(0,0) B=(20,0) r=5 → x∈[−5,25], y∈[−5,5].
    const o = slotOutline([0, 0], [20, 0], 5)!;
    expect(o).not.toBeNull();
    // n = (0,1): +n side is +y, −n side is −y.
    expect(closeVec(o.aPlus, [0, 5])).toBe(true);
    expect(closeVec(o.bPlus, [20, 5])).toBe(true);
    expect(closeVec(o.bMinus, [20, -5])).toBe(true);
    expect(closeVec(o.aMinus, [0, -5])).toBe(true);
    // Cap B starts at +n (∠ +π/2), sweep −π → midpoint at +x (bulge to 25).
    expect(close(o.capBStart, PI / 2)).toBe(true);
    const bMidAngle = o.capBStart - PI / 2; // t=0.5 of sweep −π
    const bMid: Vec2 = [
      o.centerB[0] + o.radius * Math.cos(bMidAngle),
      o.centerB[1] + o.radius * Math.sin(bMidAngle),
    ];
    expect(closeVec(bMid, [25, 0])).toBe(true);
    // Cap A starts at −n (∠ −π/2), sweep −π → midpoint at −x (bulge to −5).
    expect(close(o.capAStart, -PI / 2)).toBe(true);
    const aMidAngle = o.capAStart - PI / 2;
    const aMid: Vec2 = [
      o.centerA[0] + o.radius * Math.cos(aMidAngle),
      o.centerA[1] + o.radius * Math.sin(aMidAngle),
    ];
    expect(closeVec(aMid, [-5, 0])).toBe(true);
  });

  it("vertical slot: corners flank the axis along ±x", () => {
    // A=(0,0) B=(0,20) r=4. u=(0,1), n=(−1,0).
    const o = slotOutline([0, 0], [0, 20], 4)!;
    expect(closeVec(o.aPlus, [-4, 0])).toBe(true);
    expect(closeVec(o.bPlus, [-4, 20])).toBe(true);
    expect(closeVec(o.bMinus, [4, 20])).toBe(true);
    expect(closeVec(o.aMinus, [4, 0])).toBe(true);
    // Cap B bulges to +y (25... here 24), cap A to −y (−4).
    const bMid: Vec2 = [
      o.centerB[0] + o.radius * Math.cos(o.capBStart - PI / 2),
      o.centerB[1] + o.radius * Math.sin(o.capBStart - PI / 2),
    ];
    expect(closeVec(bMid, [0, 24])).toBe(true);
    const aMid: Vec2 = [
      o.centerA[0] + o.radius * Math.cos(o.capAStart - PI / 2),
      o.centerA[1] + o.radius * Math.sin(o.capAStart - PI / 2),
    ];
    expect(closeVec(aMid, [0, -4])).toBe(true);
  });

  it("45° slot: corners are perpendicular to the axis by r", () => {
    // A=(0,0) B=(10,10) r = √2 → offset along n=(−1,1)/√2 gives ±(−1,1).
    const r = Math.SQRT2;
    const o = slotOutline([0, 0], [10, 10], r)!;
    expect(closeVec(o.aPlus, [-1, 1])).toBe(true);
    expect(closeVec(o.bPlus, [9, 11])).toBe(true);
    expect(closeVec(o.bMinus, [11, 9])).toBe(true);
    expect(closeVec(o.aMinus, [1, -1])).toBe(true);
  });

  it("zero-length slot (A==B) is rejected", () => {
    expect(slotOutline([3, 3], [3, 3], 5)).toBeNull();
  });

  it("zero radius is rejected", () => {
    expect(slotOutline([0, 0], [20, 0], 0)).toBeNull();
  });
});
