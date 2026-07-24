import { describe, it, expect } from "vitest";
import {
  collectSketchSessionGeometry,
  collectSketchRegionFill,
  collectSketchLineSegments,
  collectSketchPointPickables,
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
    const { solid, legs, guide, points, controlHandles } =
      collectSketchSessionGeometry(calls, XY);
    expect(solid.length).toBeGreaterThan(0); // sampled curve
    expect(legs.length).toBe(2 * 6); // start + end tangent legs
    expect(guide.length).toBeGreaterThan(0); // interior control polygon edges
    // Ticket 7ad7d649: the two INTERIOR poles are tangent control points
    // (white ring markers); only the endpoints stay filled dots.
    expect(points.length).toBe(2 * 3);
    expect(controlHandles.length).toBe(2 * 3);
  });
});

describe("collectSketchLineSegments (committed overlay outline)", () => {
  it("renders a sketch_line as one world segment", () => {
    const out = collectSketchLineSegments(
      [{ type: "sketch_line", params: { start: [0, 0], end: [10, 0] } }],
      XY,
    );
    expect(out).toEqual([0, 0, 0, 10, 0, 0]);
  });

  it("flattens a sketch_spline into segments (ticket sc8g — was empty)", () => {
    // An interpolate spline closing a loop: the committed overlay used to
    // skip splines entirely, leaving a visible gap. Now it samples them.
    const out = collectSketchLineSegments(
      [
        {
          type: "sketch_spline",
          params: {
            mode: "interpolate",
            closed: false,
            points: [0, 0, 5, 5, 10, 0],
          },
        },
      ],
      XY,
    );
    expect(out.length).toBeGreaterThan(0);
    expect(out.length % 6).toBe(0); // whole [ax,ay,az, bx,by,bz] segments
    // First sample sits at the first control point on the XY plane.
    expect(out.slice(0, 3)).toEqual([0, 0, 0]);
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

describe("collectSketchPointPickables (#404 Hole locations)", () => {
  it("emits each free sketch point with its shared entity id and world pos", () => {
    // Two free points → solver entity ids 1 and 2 (origin is 0).
    const calls: SketchSubCall[] = [
      { type: "sketch_point", params: { at: [2, 3] }, id: 100 },
      { type: "sketch_point", params: { at: [-4, 5] }, id: 101 },
    ];
    const pts = collectSketchPointPickables(calls, XY);
    // origin (id 0) + the two free points.
    const byId = new Map(pts.map((p) => [p.pointId, p.p]));
    expect(byId.get(1)).toEqual([2, 3, 0]);
    expect(byId.get(2)).toEqual([-4, 5, 0]);
    expect(byId.get(0)).toEqual([0, 0, 0]);
  });

  it("maps points through a tilted plane frame", () => {
    const calls: SketchSubCall[] = [
      { type: "sketch_point", params: { at: [2, 3] }, id: 100 },
    ];
    const pts = collectSketchPointPickables(calls, XZ);
    // origin (1,2,3) + point: origin + 2*xAxis + 3*yAxis = (1+2, 2, 3+3).
    const p1 = pts.find((p) => p.pointId === 1);
    expect(p1?.p).toEqual([3, 2, 6]);
  });
});
