import { describe, expect, it } from "vitest";
import {
  closedRegionPolygons,
  loopsFromEdges,
  signedArea,
} from "./sketchRegions";
import type { SubCall } from "./sketchTrim";

const line = (a: [number, number], b: [number, number]): SubCall => ({
  type: "sketch_line",
  params: { start: a, end: b },
});

/** Total absolute area of all returned polygons. */
const totalArea = (polys: [number, number][][]): number =>
  polys.reduce((acc, p) => acc + Math.abs(signedArea(p)), 0);

describe("sketchRegions", () => {
  it("detects a closed loop of four lines as one region", () => {
    const regions = closedRegionPolygons([
      line([0, 0], [10, 0]),
      line([10, 0], [10, 10]),
      line([10, 10], [0, 10]),
      line([0, 10], [0, 0]),
    ]);
    expect(regions).toHaveLength(1);
    expect(signedArea(regions[0])).toBeCloseTo(100, 3);
  });

  it("loop direction does not matter (mixed edge orientations)", () => {
    const regions = closedRegionPolygons([
      line([0, 0], [10, 0]),
      line([10, 10], [10, 0]), // reversed
      line([10, 10], [0, 10]),
      line([0, 0], [0, 10]), // reversed
    ]);
    expect(regions).toHaveLength(1);
    expect(Math.abs(signedArea(regions[0]))).toBeCloseTo(100, 3);
  });

  it("an open chain produces no region", () => {
    const regions = closedRegionPolygons([
      line([0, 0], [10, 0]),
      line([10, 0], [10, 10]),
      line([10, 10], [0, 10]),
    ]);
    expect(regions).toHaveLength(0);
  });

  it("two squares sharing an edge yield two faces, not the union", () => {
    const regions = closedRegionPolygons([
      line([0, 0], [10, 0]),
      line([10, 0], [10, 10]),
      line([10, 10], [0, 10]),
      line([0, 10], [0, 0]),
      // Second square to the right, sharing the x=10 edge.
      line([10, 0], [20, 0]),
      line([20, 0], [20, 10]),
      line([20, 10], [10, 10]),
    ]);
    expect(regions).toHaveLength(2);
    expect(totalArea(regions)).toBeCloseTo(200, 2);
  });

  it("a dangling stub on a loop does not break the region", () => {
    const regions = closedRegionPolygons([
      line([0, 0], [10, 0]),
      line([10, 0], [10, 10]),
      line([10, 10], [0, 10]),
      line([0, 10], [0, 0]),
      line([10, 10], [15, 15]), // whisker
    ]);
    expect(regions).toHaveLength(1);
    expect(Math.abs(signedArea(regions[0]))).toBeCloseTo(100, 2);
  });

  it("an overshooting edge (T-junction) still forms the region (ticket mdye)", () => {
    // The left edge runs PAST the bottom-left corner; the bottom edge meets it
    // partway along (a T-junction, not a shared endpoint). The overhang must not
    // stop the rectangle from shading — it is split at the junction.
    const regions = closedRegionPolygons([
      line([0, 0], [10, 0]), // bottom — its (0,0) sits on the left line's interior
      line([10, 0], [10, 8]),
      line([10, 8], [0, 8]),
      line([0, 8], [0, -10]), // left edge overshoots below the corner
    ]);
    expect(regions).toHaveLength(1);
    expect(Math.abs(signedArea(regions[0]))).toBeCloseTo(80, 2);
  });

  it("welds endpoints within tolerance only", () => {
    const closeEnough = closedRegionPolygons([
      line([0, 0], [10, 0]),
      line([10, 0.0004], [10, 10]),
      line([10, 10], [0, 10]),
      line([0, 10], [0, 0]),
    ]);
    expect(closeEnough).toHaveLength(1);

    const gap = closedRegionPolygons([
      line([0, 0], [10, 0]),
      line([10, 0.5], [10, 10]), // visible gap
      line([10, 10], [0, 10]),
      line([0, 10], [0, 0]),
    ]);
    expect(gap).toHaveLength(0);
  });

  it("circles, rectangles and polygons are regions by themselves", () => {
    const regions = closedRegionPolygons([
      { type: "sketch_circle", params: { center: [0, 0], radius: 5 } },
      {
        type: "sketch_rectangle",
        params: { corner_a: [20, 0], corner_b: [30, 10] },
      },
      { type: "sketch_polygon", params: { vertices: [50, 0, 60, 0, 55, 10] } },
    ]);
    expect(regions).toHaveLength(3);
    // All normalized CCW for the single nonzero fill.
    for (const r of regions) expect(signedArea(r)).toBeGreaterThan(0);
    // Triangle (50,0)(60,0)(55,10): ½·10·10 = 50.
    expect(totalArea(regions)).toBeCloseTo(Math.PI * 25 + 100 + 50, 0);
  });

  it("a line–arc loop (D-shape) closes through the arc samples", () => {
    // Semicircle radius 5 around (0,0) from (5,0) to (-5,0), closed by a line.
    const regions = closedRegionPolygons([
      {
        type: "sketch_arc",
        params: {
          center: [0, 0],
          radius: 5,
          start_angle: 0,
          sweep_angle: Math.PI,
        },
      },
      line([-5, 0], [5, 0]),
    ]);
    expect(regions).toHaveLength(1);
    expect(Math.abs(signedArea(regions[0]))).toBeCloseTo((Math.PI * 25) / 2, 0);
  });

  it("construction geometry never forms regions", () => {
    const regions = closedRegionPolygons([
      line([0, 0], [10, 0]),
      line([10, 0], [10, 10]),
      line([10, 10], [0, 10]),
      {
        type: "sketch_line",
        params: { start: [0, 10], end: [0, 0], construction: true },
      },
      {
        type: "sketch_circle",
        params: { center: [0, 0], radius: 5, construction: true },
      },
    ]);
    expect(regions).toHaveLength(0);
  });

  it("closed splines fill, open splines do not", () => {
    const closed = closedRegionPolygons([
      {
        type: "sketch_spline",
        params: { points: [0, 0, 10, 0, 10, 10, 0, 10], closed: true },
      },
    ]);
    expect(closed).toHaveLength(1);
    const open = closedRegionPolygons([
      {
        type: "sketch_spline",
        params: { points: [0, 0, 10, 0, 10, 10, 0, 10], closed: false },
      },
    ]);
    expect(open).toHaveLength(0);
  });

  it("slot produces a stadium region", () => {
    const regions = closedRegionPolygons([
      {
        type: "sketch_slot",
        params: { center_a: [0, 0], center_b: [10, 0], radius: 2 },
      },
    ]);
    expect(regions).toHaveLength(1);
    // Stadium area = 2r·L + πr² = 40 + 4π.
    expect(Math.abs(signedArea(regions[0]))).toBeCloseTo(40 + Math.PI * 4, 0);
  });

  it("loopsFromEdges treats a self-closing polyline as a loop", () => {
    const square: [number, number][] = [
      [0, 0],
      [10, 0],
      [10, 10],
      [0, 10],
      [0, 0],
    ];
    const loops = loopsFromEdges([square]);
    expect(loops).toHaveLength(1);
    expect(Math.abs(signedArea(loops[0]))).toBeCloseTo(100, 6);
  });
});
