import { describe, it, expect } from "vitest";
import { buildSweepGeometry, __test } from "./sweepMesh";
import type { ContourData } from "./kernel";

type V3 = [number, number, number];

/** Square profile, radial extent ±2 (CCW). */
function square(): ContourData {
  return { outer: [[-2, -2], [2, -2], [2, 2], [-2, 2]], holes: [] };
}

const straightX: V3[] = [
  [0, 0, 0],
  [10, 0, 0],
  [20, 0, 0],
  [30, 0, 0],
];

describe("sweepMesh framing (RMF)", () => {
  it("keeps a constant frame along a straight spine (no spurious twist)", () => {
    const T = __test.computeTangents(straightX, false);
    const { N, B } = __test.rmFrames(straightX, T);
    for (let i = 0; i < N.length; i++) {
      expect(N[i][0]).toBeCloseTo(N[0][0], 6);
      expect(N[i][1]).toBeCloseTo(N[0][1], 6);
      expect(N[i][2]).toBeCloseTo(N[0][2], 6);
      expect(B[i][2]).toBeCloseTo(B[0][2], 6);
    }
    // Tangent +X → seed normal +Y, binormal +Z.
    expect(N[0]).toEqual([0, 1, 0]);
    expect(B[0][0]).toBeCloseTo(0, 6);
    expect(B[0][2]).toBeCloseTo(1, 6);
  });

  it("rotates the frame to follow a turning spine, staying orthonormal", () => {
    const path: V3[] = [
      [0, 0, 0],
      [10, 0, 0],
      [20, 0, 0],
      [20, 10, 0], // a turn into +Y
      [20, 20, 0],
    ];
    const T = __test.computeTangents(path, false);
    const { N, B } = __test.rmFrames(path, T);
    for (let i = 0; i < N.length; i++) {
      // Orthonormal: |N|=|B|=1 and N⊥B.
      const nl = Math.hypot(...N[i]);
      const bl = Math.hypot(...B[i]);
      const ndotb = N[i][0] * B[i][0] + N[i][1] * B[i][1] + N[i][2] * B[i][2];
      expect(nl).toBeCloseTo(1, 5);
      expect(bl).toBeCloseTo(1, 5);
      expect(ndotb).toBeCloseTo(0, 5);
    }
  });
});

describe("buildSweepGeometry", () => {
  it("returns null for degenerate input", () => {
    expect(buildSweepGeometry([[0, 0, 0]], square())).toBeNull();
    expect(
      buildSweepGeometry(straightX, { outer: [[0, 0], [1, 0]], holes: [] }),
    ).toBeNull();
  });

  it("sweeps a square along +X into a prism with the right extents", () => {
    const geo = buildSweepGeometry(straightX, square())!;
    expect(geo).not.toBeNull();
    const pos = geo.getAttribute("position");
    expect(pos.count).toBe(4 * 4); // 4 rings × 4 profile points
    // Ring 0 at origin, frame N=+Y B=+Z → profile (u,v) → (0, u, v).
    expect([pos.getX(0), pos.getY(0), pos.getZ(0)]).toEqual([0, -2, -2]);
    geo.computeBoundingBox();
    const bb = geo.boundingBox!;
    expect(bb.min.x).toBeCloseTo(0, 5);
    expect(bb.max.x).toBeCloseTo(30, 5);
    expect(bb.min.y).toBeCloseTo(-2, 5);
    expect(bb.max.y).toBeCloseTo(2, 5);
    expect(bb.min.z).toBeCloseTo(-2, 5);
    expect(bb.max.z).toBeCloseTo(2, 5);
  });

  it("caps an open sweep but not a closed loop", () => {
    const open = buildSweepGeometry(straightX, square())!;
    const openTris = open.getIndex()!.count / 3;
    // 3 ring spans × 4 edges × 2 + two square caps (2 tris each).
    expect(openTris).toBe(3 * 4 * 2 + 4);

    // Closed loop: a square-ish circuit of 4 samples, wrap, no caps.
    const loop: V3[] = [[0, 0, 0], [10, 0, 0], [10, 10, 0], [0, 10, 0]];
    const closed = buildSweepGeometry(loop, square(), { closed: true })!;
    const closedTris = closed.getIndex()!.count / 3;
    expect(closedTris).toBe(4 * 4 * 2); // 4 ring spans (wrap), no caps
  });

  it("produces finite vertex normals", () => {
    const geo = buildSweepGeometry(
      [
        [0, 0, 0],
        [10, 0, 0],
        [15, 8, 0],
        [15, 18, 0],
      ],
      square(),
    )!;
    const nrm = geo.getAttribute("normal");
    for (let i = 0; i < nrm.count; i++) {
      expect(Number.isFinite(nrm.getX(i))).toBe(true);
      expect(Number.isFinite(nrm.getY(i))).toBe(true);
      expect(Number.isFinite(nrm.getZ(i))).toBe(true);
    }
  });
});
