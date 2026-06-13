import { describe, it, expect } from "vitest";
import {
  collectSketchSessionGeometry,
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
    const { outline, points } = collectSketchSessionGeometry(calls, XY);
    expect(outline).toEqual([0, 0, 0, 10, 0, 0]);
    expect(points).toEqual([0, 0, 0, 10, 0, 0]);
  });

  it("maps (u,v) through the plane frame (tilted plane)", () => {
    const calls: SketchSubCall[] = [
      { type: "sketch_line", params: { start: [0, 0], end: [4, 5] } },
    ];
    const { outline } = collectSketchSessionGeometry(calls, XZ);
    // origin (1,2,3); u along +x, v along +z.
    expect(outline).toEqual([1, 2, 3, 5, 2, 8]);
  });

  it("skips construction geometry (stays on the overlay, dashed)", () => {
    const calls: SketchSubCall[] = [
      {
        type: "sketch_line",
        params: { start: [0, 0], end: [1, 0], construction: true },
      },
    ];
    const { outline, points } = collectSketchSessionGeometry(calls, XY);
    expect(outline).toHaveLength(0);
    expect(points).toHaveLength(0);
  });

  it("skips types not handled in 3D (spline/path/point)", () => {
    const calls: SketchSubCall[] = [
      { type: "sketch_point", params: { at: [1, 1] } },
      { type: "sketch_spline", params: { points: [0, 0, 1, 1, 2, 0] } },
    ];
    const { outline, points } = collectSketchSessionGeometry(calls, XY);
    expect(outline).toHaveLength(0);
    expect(points).toHaveLength(0);
    expect(SESSION_3D_TYPES.has("sketch_spline")).toBe(false);
    expect(SESSION_3D_TYPES.has("sketch_point")).toBe(false);
  });

  it("closes a rectangle into four segments and no vertex dots", () => {
    const calls: SketchSubCall[] = [
      { type: "sketch_rectangle", params: { corner_a: [0, 0], corner_b: [2, 1] } },
    ];
    const { outline, points } = collectSketchSessionGeometry(calls, XY);
    expect(outline).toHaveLength(4 * 6); // 4 segments × 2 points × 3 coords
    expect(points).toHaveLength(0);
  });

  it("tessellates a circle into a closed loop with a centre marker", () => {
    const calls: SketchSubCall[] = [
      { type: "sketch_circle", params: { center: [0, 0], radius: 5 } },
    ];
    const { outline, points } = collectSketchSessionGeometry(calls, XY);
    expect(outline).toHaveLength(64 * 6); // 64 steps, segment pairs
    expect(points).toEqual([0, 0, 0]); // centre dot
    // First sampled vertex sits on +x at the radius.
    expect(outline.slice(0, 3)).toEqual([5, 0, 0]);
  });
});
