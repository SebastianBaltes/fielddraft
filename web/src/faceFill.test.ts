import { describe, expect, it } from "vitest";
import { assembleFaceRings, triangulatePlanarFace, type Pt2 } from "./faceFill";

/** Total area of a world-space triangle soup (flat xyz). */
function soupArea(tris: number[]): number {
  let area = 0;
  for (let i = 0; i + 8 < tris.length; i += 9) {
    const ax = tris[i], ay = tris[i + 1], az = tris[i + 2];
    const ux = tris[i + 3] - ax, uy = tris[i + 4] - ay, uz = tris[i + 5] - az;
    const vx = tris[i + 6] - ax, vy = tris[i + 7] - ay, vz = tris[i + 8] - az;
    const cx = uy * vz - uz * vy;
    const cy = uz * vx - ux * vz;
    const cz = ux * vy - uy * vx;
    area += Math.hypot(cx, cy, cz) / 2;
  }
  return area;
}

const square = (s: number, rev = false): Pt2[][] => {
  const edges: Pt2[][] = [
    [[-s, -s], [s, -s]],
    [[s, -s], [s, s]],
    [[s, s], [-s, s]],
    [[-s, s], [-s, -s]],
  ];
  return rev ? edges.map((e) => [...e].reverse()) : edges;
};

describe("assembleFaceRings (ticket por8)", () => {
  it("assembles an ordered square boundary into one ring", () => {
    const rings = assembleFaceRings(square(10));
    expect(rings).toHaveLength(1);
    expect(rings[0].length).toBeGreaterThanOrEqual(4);
  });

  it("assembles shuffled, partially reversed edges", () => {
    const e = square(10, false);
    const shuffled = [e[2], [...e[0]].reverse(), e[3], [...e[1]].reverse()];
    const rings = assembleFaceRings(shuffled);
    expect(rings).toHaveLength(1);
  });

  it("drops duplicate seam traces (whole-edge copies)", () => {
    const e = square(10);
    const offset: Pt2[][] = [[[-10.0005, 10.0004], [10.0006, 10.0004]]]; // dup top
    const rings = assembleFaceRings([...e, ...offset]);
    expect(rings).toHaveLength(1);
  });

  it("drops a duplicate split differently than the original", () => {
    const e = square(10);
    // The top edge again, but split into two halves.
    const half1: Pt2[] = [[-10.0003, 10.0004], [0, 10.0004]];
    const half2: Pt2[] = [[0, 10.0004], [10.0003, 10.0004]];
    const rings = assembleFaceRings([...e, half1, half2]);
    expect(rings).toHaveLength(1);
  });

  it("keeps a genuine interior hole", () => {
    const rings = assembleFaceRings([...square(10), ...square(2)]);
    expect(rings).toHaveLength(2);
  });

  it("drops a micro squiggle ring touching the outer boundary", () => {
    const squiggle: Pt2[][] = [[
      [10, 10], [10.0005, 10.0004], [10.0002, 10.0007], [10, 10],
    ]];
    const rings = assembleFaceRings([...square(10), ...squiggle]);
    expect(rings).toHaveLength(1);
  });
});

describe("triangulatePlanarFace (ticket por8)", () => {
  const liftZ = (edges: Pt2[][], z: number): number[][] =>
    edges.map((e) => e.flatMap(([x, y]) => [x, y, z]));

  it("fills a 20×20 square face on the z-plane", () => {
    const tris = triangulatePlanarFace(liftZ(square(10), 5), [0, 0, 5], [0, 0, 1]);
    expect(tris.length).toBeGreaterThan(0);
    expect(soupArea(tris)).toBeCloseTo(400, 1);
    for (let i = 2; i < tris.length; i += 3) expect(tris[i]).toBeCloseTo(5, 5);
  });

  it("subtracts holes from the filled area", () => {
    const tris = triangulatePlanarFace(
      liftZ([...square(10), ...square(2)], 0),
      [0, 0, 0],
      [0, 0, 1],
    );
    expect(soupArea(tris)).toBeCloseTo(400 - 16, 1);
  });

  it("fills a face on a tilted plane (+X normal)", () => {
    const edges: number[][] = [
      [[5, -10, -10], [5, 10, -10]].flat(),
      [[5, 10, -10], [5, 10, 10]].flat(),
      [[5, 10, 10], [5, -10, 10]].flat(),
      [[5, -10, 10], [5, -10, -10]].flat(),
    ];
    const tris = triangulatePlanarFace(edges, [5, 0, 0], [1, 0, 0]);
    expect(soupArea(tris)).toBeCloseTo(400, 1);
    for (let i = 0; i < tris.length; i += 3) expect(tris[i]).toBeCloseTo(5, 5);
  });

  it("returns [] for garbage (a single dangling edge)", () => {
    const tris = triangulatePlanarFace([[0, 0, 0, 10, 0, 0]], [0, 0, 0], [0, 0, 1]);
    expect(tris).toEqual([]);
  });
});
