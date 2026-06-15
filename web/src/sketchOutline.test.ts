import { describe, it, expect } from "vitest";
import {
  collectSketchSessionGeometry,
  collectSketchRegionFill,
  SESSION_3D_TYPES,
  type SketchSubCall,
} from "./sketchOutline";
import type { PlaneFrame } from "./sketchPlaneFrame";

// XY datum: sketch (u, v) maps straight to world (x, y, 0).
const XY: PlaneFrame = {
  origin: [0, 0, 0],
  xAxis: [1, 0, 0],
  yAxis: [0, 1, 0],
  normal: [0, 0, 1],
};

// A tilted frame (sketch +v along world +z) to confirm (u,v) really go
// through the plane basis, not a flat passthrough.
const XZ: PlaneFrame = {
  origin: [1, 2, 3],
  xAxis: [1, 0, 0],
  yAxis: [0, 0, 1],
  normal: [0, -1, 0],
};

describe("collectSketchSessionGeometry", () => {
  it("emits a line as one world segment plus its two endpoints", () => {
    const calls: SketchSubCall[] = [
      { type: "sketch_line", params: { start: [0, 0], end: [10, 0] } },
    ];
    const { solid, points } = collectSketchSessionGeometry(calls, XY);
    expect(solid).toEqual([0, 0, 0, 10, 0, 0]);
    expect(points).toEqual([0, 0, 0, 10, 0, 0]);
  });

  it("maps (u,v) through the plane frame (tilted plane)", () => {
    const calls: SketchSubCall[] = [
      { type: "sketch_line", params: { start: [0, 0], end: [4, 5] } },
    ];
    const { solid } = collectSketchSessionGeometry(calls, XZ);
    // origin (1,2,3); u along +x, v along +z.
    expect(solid).toEqual([1, 2, 3, 5, 2, 8]);
  });

  it("routes construction geometry to its own (dashed) bucket, no points", () => {
    const calls: SketchSubCall[] = [
      {
        type: "sketch_line",
        params: { start: [0, 0], end: [1, 0], construction: true },
      },
    ];
    const { solid, construction, points } = collectSketchSessionGeometry(calls, XY);
    expect(solid).toHaveLength(0);
    expect(construction).toEqual([0, 0, 0, 1, 0, 0]);
    expect(points).toHaveLength(0);
  });

  it("covers every drawable entity type", () => {
    for (const t of [
      "sketch_line",
      "sketch_point",
      "sketch_rectangle",
      "sketch_circle",
      "sketch_polygon",
      "sketch_arc",
      "sketch_ellipse",
      "sketch_slot",
      "sketch_spline",
      "sketch_path",
    ]) {
      expect(SESSION_3D_TYPES.has(t)).toBe(true);
    }
  });

  it("closes a rectangle into four segments and no vertex dots", () => {
    const calls: SketchSubCall[] = [
      { type: "sketch_rectangle", params: { corner_a: [0, 0], corner_b: [2, 1] } },
    ];
    const { solid, points } = collectSketchSessionGeometry(calls, XY);
    expect(solid).toHaveLength(4 * 6); // 4 segments × 2 points × 3 coords
    expect(points).toHaveLength(0);
  });

  it("tessellates a circle into a closed loop with a centre marker", () => {
    const calls: SketchSubCall[] = [
      { type: "sketch_circle", params: { center: [0, 0], radius: 5 } },
    ];
    const { solid, points } = collectSketchSessionGeometry(calls, XY);
    expect(solid).toHaveLength(64 * 6); // 64 steps, segment pairs
    expect(points).toEqual([0, 0, 0]); // centre dot
    expect(solid.slice(0, 3)).toEqual([5, 0, 0]); // first vertex on +x
  });

  it("splits a control-mode spline into curve, legs and guide polygon", () => {
    const calls: SketchSubCall[] = [
      {
        type: "sketch_spline",
        params: { points: [0, 0, 5, 5, 10, 0, 15, 5], mode: "control", closed: false },
      },
    ];
    const { solid, legs, guide, points } = collectSketchSessionGeometry(calls, XY);
    expect(solid.length).toBeGreaterThan(0); // sampled curve
    expect(legs.length).toBe(2 * 6); // start + end tangent legs
    expect(guide.length).toBeGreaterThan(0); // interior control polygon edges
    expect(points.length).toBe(4 * 3); // 4 control points
  });
});

describe("collectSketchRegionFill", () => {
  it("triangulates a closed rectangle region into world-space triangles", () => {
    const calls: SketchSubCall[] = [
      { type: "sketch_rectangle", params: { corner_a: [0, 0], corner_b: [2, 2] } },
    ];
    // Fan triangulation of the polygon (indices into the region's points).
    const tri = (poly: [number, number][]): number[][] => {
      const out: number[][] = [];
      for (let i = 1; i + 1 < poly.length; i++) out.push([0, i, i + 1]);
      return out;
    };
    const pos = collectSketchRegionFill(calls, XY, tri);
    expect(pos.length).toBeGreaterThan(0);
    expect(pos.length % 9).toBe(0); // whole triangles (3 verts × 3 coords)
  });

  it("fills a committed circle region (ticket jjmv)", () => {
    // The committed-sketch sub_call shape (center array, numeric radius) — the
    // same fields the outline renderer reads — must produce a filled region so
    // a committed circle reads as a pickable face, not just an outline.
    const calls: SketchSubCall[] = [
      { type: "sketch_circle", params: { center: [0, 0], radius: 5 } },
    ];
    const tri = (poly: [number, number][]): number[][] => {
      const out: number[][] = [];
      for (let i = 1; i + 1 < poly.length; i++) out.push([0, i, i + 1]);
      return out;
    };
    const pos = collectSketchRegionFill(calls, XY, tri);
    expect(pos.length).toBeGreaterThan(0);
    expect(pos.length % 9).toBe(0);
  });
});
