import { describe, it, expect } from "vitest";
import * as THREE from "three";
import { buildHelixGeometry, decodeHelixParams } from "./helixMesh";
import type { ContourData } from "./kernel";

/** Square cross-section: radial 9..11, axial −1..1 (CCW outer). */
function squareProfile(): ContourData {
  return {
    outer: [
      [9, -1],
      [11, -1],
      [11, 1],
      [9, 1],
    ],
    holes: [],
  };
}

describe("decodeHelixParams", () => {
  it("maps the flat kernel param array to named fields", () => {
    const p = decodeHelixParams([4, 2, 1, 0.1, 0.5, 3]);
    expect(p).toEqual({
      pitch: 4,
      turns: 2,
      leftHanded: true,
      taper: 0.1,
      startAngle: 0.5,
      yAnchor: 3,
    });
    // leftHanded is false for a 0 flag.
    expect(decodeHelixParams([4, 2, 0, 0, 0, 0]).leftHanded).toBe(false);
  });
});

describe("buildHelixGeometry", () => {
  it("returns null for degenerate input", () => {
    expect(
      buildHelixGeometry({ outer: [[0, 0], [1, 0]], holes: [] }, {
        pitch: 4, turns: 2, leftHanded: false, taper: 0, startAngle: 0, yAnchor: 0,
      }),
    ).toBeNull();
    // Zero turns / zero pitch produce no sweep.
    expect(
      buildHelixGeometry(squareProfile(), {
        pitch: 4, turns: 0, leftHanded: false, taper: 0, startAngle: 0, yAnchor: 0,
      }),
    ).toBeNull();
    expect(
      buildHelixGeometry(squareProfile(), {
        pitch: 0, turns: 2, leftHanded: false, taper: 0, startAngle: 0, yAnchor: 0,
      }),
    ).toBeNull();
  });

  it("builds one full cross-section per ring", () => {
    const geo = buildHelixGeometry(squareProfile(), {
      pitch: 4, turns: 2, leftHanded: false, taper: 0, startAngle: 0, yAnchor: 0,
    })!;
    expect(geo).not.toBeNull();
    const segs = Math.ceil(2 * 48); // SEGMENTS_PER_TURN = 48
    const nRings = segs + 1;
    const pos = geo.getAttribute("position");
    expect(pos.count).toBe(nRings * 4); // 4 profile points per ring
  });

  it("places the first ring via the forward helix map (t=0)", () => {
    const geo = buildHelixGeometry(squareProfile(), {
      pitch: 4, turns: 2, leftHanded: false, taper: 0, startAngle: 0, yAnchor: 0,
    })!;
    const pos = geo.getAttribute("position");
    // Ring 0, point 0 = (rp=9, w=−1) at t=0 → (9, −1, 0).
    expect(pos.getX(0)).toBeCloseTo(9, 5);
    expect(pos.getY(0)).toBeCloseTo(-1, 5);
    expect(pos.getZ(0)).toBeCloseTo(0, 5);
    // Point 1 = (rp=11, w=−1) → (11, −1, 0).
    expect(pos.getX(1)).toBeCloseTo(11, 5);
    expect(pos.getZ(1)).toBeCloseTo(0, 5);
  });

  it("advances Y by pitch·turns over the full sweep", () => {
    const geo = buildHelixGeometry(squareProfile(), {
      pitch: 4, turns: 2, leftHanded: false, taper: 0, startAngle: 0, yAnchor: 0,
    })!;
    geo.computeBoundingBox();
    const bb = geo.boundingBox!;
    // Axial: min w = −1 at t=0; max = w(+1) + pitch·turns (4·2=8) = 9.
    expect(bb.min.y).toBeCloseTo(-1, 4);
    expect(bb.max.y).toBeCloseTo(9, 4);
    // Radial extent stays within the profile radii (no taper).
    expect(bb.max.x).toBeLessThanOrEqual(11.0001);
    expect(bb.min.x).toBeGreaterThanOrEqual(-11.0001);
  });

  it("winds the opposite way when left-handed", () => {
    const right = buildHelixGeometry(squareProfile(), {
      pitch: 4, turns: 1, leftHanded: false, taper: 0, startAngle: 0, yAnchor: 0,
    })!;
    const left = buildHelixGeometry(squareProfile(), {
      pitch: 4, turns: 1, leftHanded: true, taper: 0, startAngle: 0, yAnchor: 0,
    })!;
    // Quarter turn in: θ = ±π/2 → Z flips sign between the two windings.
    const segs = Math.ceil(1 * 48);
    const quarter = Math.round(segs / 4);
    const idx = quarter * 4; // point 0 of that ring
    const zr = right.getAttribute("position").getZ(idx);
    const zl = left.getAttribute("position").getZ(idx);
    expect(Math.sign(zr)).toBe(-Math.sign(zl));
    expect(zr).toBeCloseTo(-zl, 4);
  });

  it("shrinks the radius along +y under taper", () => {
    const taper = 0.1;
    const geo = buildHelixGeometry(squareProfile(), {
      pitch: 4, turns: 2, leftHanded: false, taper, startAngle: 0, yAnchor: 0,
    })!;
    const pos = geo.getAttribute("position");
    const segs = Math.ceil(2 * 48);
    const lastBase = segs * 4; // last ring, point 0
    // Last ring point 0: w=−1, Y=−1+8=7, r = 9 + taper·(7−0) = 9.7.
    const x = pos.getX(lastBase);
    const z = pos.getZ(lastBase);
    const r = Math.hypot(x, z);
    expect(r).toBeCloseTo(9 + taper * 7, 4);
  });

  it("closes the solid with end caps (more tris than walls alone)", () => {
    const geo = buildHelixGeometry(squareProfile(), {
      pitch: 4, turns: 2, leftHanded: false, taper: 0, startAngle: 0, yAnchor: 0,
    })!;
    const segs = Math.ceil(2 * 48);
    const wallTris = segs * 4 * 2; // 4 edges · 2 tris per quad
    const totalTris = geo.getIndex()!.count / 3;
    expect(totalTris).toBeGreaterThan(wallTris);
    // A square cap triangulates into 2 triangles, two caps = 4 extra.
    expect(totalTris).toBe(wallTris + 4);
  });

  it("has finite vertex normals", () => {
    const geo = buildHelixGeometry(squareProfile(), {
      pitch: 4, turns: 1, leftHanded: false, taper: 0, startAngle: 0, yAnchor: 0,
    })!;
    const n = geo.getAttribute("normal");
    expect(n).toBeTruthy();
    for (let i = 0; i < n.count; i++) {
      expect(Number.isFinite(n.getX(i))).toBe(true);
      expect(Number.isFinite(n.getY(i))).toBe(true);
      expect(Number.isFinite(n.getZ(i))).toBe(true);
    }
  });
});

describe("buildHelixGeometry with holes", () => {
  it("sweeps hole loops as inner walls and caps around them", () => {
    const contour: ContourData = {
      outer: [
        [8, -2],
        [12, -2],
        [12, 2],
        [8, 2],
      ],
      // CW inner loop (a bore through the profile).
      holes: [[[9, -1], [9, 1], [11, 1], [11, -1]]],
    };
    const geo = buildHelixGeometry(contour, {
      pitch: 6, turns: 1, leftHanded: false, taper: 0, startAngle: 0, yAnchor: 0,
    })!;
    expect(geo).not.toBeNull();
    const segs = Math.ceil(1 * 48);
    const nRings = segs + 1;
    // 8 profile points per ring (4 outer + 4 hole).
    expect(geo.getAttribute("position").count).toBe(nRings * 8);
    // Walls: both loops (8 edges) swept; caps: an annulus triangulates
    // into 8 triangles → indices present and divisible by 3.
    expect(geo.getIndex()!.count % 3).toBe(0);
    void THREE; // ensure three is loaded in this test context
  });
});
