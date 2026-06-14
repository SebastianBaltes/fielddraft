import { describe, expect, it } from "vitest";
import { catmullRomSpline, polylineToSegments, type V3 } from "./sweepGizmo";

const approx = (a: V3, b: V3, eps = 1e-4) =>
  Math.abs(a[0] - b[0]) < eps && Math.abs(a[1] - b[1]) < eps && Math.abs(a[2] - b[2]) < eps;

describe("catmullRomSpline (1:1 port of crate::path_provider::catmull_rom)", () => {
  it("two open nodes degenerate to a straight segment", () => {
    const out = catmullRomSpline([[0, 0, 0], [10, 0, 0]], false, 8);
    expect(out).toHaveLength(2);
  });

  it("passes through every control node (open), final point is the last node", () => {
    const nodes: V3[] = [
      [0, 0, 0],
      [10, 5, 0],
      [20, -3, 4],
      [30, 0, 0],
    ];
    const sps = 10;
    const out = catmullRomSpline(nodes, false, sps);
    for (let s = 0; s < nodes.length - 1; s++) {
      expect(approx(out[s * sps], nodes[s])).toBe(true);
    }
    expect(approx(out[out.length - 1], nodes[nodes.length - 1])).toBe(true);
  });

  it("closed spline wraps without an appended endpoint", () => {
    const nodes: V3[] = [
      [0, 0, 0],
      [10, 0, 0],
      [10, 10, 0],
      [0, 10, 0],
    ];
    const sps = 6;
    const out = catmullRomSpline(nodes, true, sps);
    expect(out).toHaveLength(nodes.length * sps); // no separate endpoint
    for (let s = 0; s < nodes.length; s++) {
      expect(approx(out[s * sps], nodes[s])).toBe(true);
    }
  });
});

describe("polylineToSegments", () => {
  it("flattens a polyline into endpoint pairs (6 floats per segment)", () => {
    const seg = polylineToSegments([[0, 0, 0], [1, 0, 0], [1, 1, 0]], false);
    expect(seg).toEqual([0, 0, 0, 1, 0, 0, 1, 0, 0, 1, 1, 0]);
  });

  it("appends the wrap segment when closed", () => {
    const seg = polylineToSegments([[0, 0, 0], [1, 0, 0]], true);
    // segment 0→1 plus wrap 1→0
    expect(seg).toEqual([0, 0, 0, 1, 0, 0, 1, 0, 0, 0, 0, 0]);
  });
});
