import { describe, expect, it } from "vitest";
import {
  makeLiveEdgeLookup,
  nearestProjectedEdge,
  parseQEdgeIdHash,
  pointToSegmentDist,
  resolveProjectedSubCalls,
  type ProjectedEdgeSeg,
} from "./sketchProject";
import type { PlaneFrame } from "./sketchPlaneFrame";
import type { SketchSubCall } from "./sketchOutline";

const XY: PlaneFrame = {
  origin: [0, 0, 0],
  xAxis: [1, 0, 0],
  yAxis: [0, 1, 0],
  normal: [0, 0, 1],
};

/** A frame on the +Z face of a box (origin lifted), still XY-aligned. */
const TOP: PlaneFrame = {
  origin: [0, 0, 5],
  xAxis: [1, 0, 0],
  yAxis: [0, 1, 0],
  normal: [0, 0, 1],
};

describe("pointToSegmentDist", () => {
  it("measures perpendicular distance to the segment interior", () => {
    expect(pointToSegmentDist([5, 3], [0, 0], [10, 0])).toBeCloseTo(3);
  });

  it("clamps to the nearest endpoint past the segment ends", () => {
    expect(pointToSegmentDist([-4, 0], [0, 0], [10, 0])).toBeCloseTo(4);
    expect(pointToSegmentDist([14, 0], [0, 0], [10, 0])).toBeCloseTo(4);
  });

  it("collapses a zero-length segment to point distance", () => {
    expect(pointToSegmentDist([3, 4], [0, 0], [0, 0])).toBeCloseTo(5);
  });
});

describe("nearestProjectedEdge", () => {
  const edges: ProjectedEdgeSeg[] = [
    { p0: [0, 0], p1: [10, 0] }, // bottom
    { p0: [10, 0], p1: [10, 10] }, // right
    { p0: [10, 10], p1: [0, 10] }, // top
    { p0: [0, 10], p1: [0, 0] }, // left
  ];

  it("picks the closest segment", () => {
    // Just below the top edge midpoint.
    expect(nearestProjectedEdge(edges, [5, 9.5], 1)).toBe(2);
    // Just right of the right edge midpoint.
    expect(nearestProjectedEdge(edges, [10.4, 5], 1)).toBe(1);
  });

  it("returns null when the click is far from every segment", () => {
    expect(nearestProjectedEdge(edges, [100, 100], 1)).toBeNull();
    // Centre of the square: 5 mm from each edge, tol too small.
    expect(nearestProjectedEdge(edges, [5, 5], 1)).toBeNull();
  });

  it("resolves an exact tie to the first segment in order", () => {
    // Equidistant (1 mm) from both the bottom and a duplicated bottom edge.
    const tie: ProjectedEdgeSeg[] = [
      { p0: [0, 0], p1: [10, 0] },
      { p0: [0, 0], p1: [10, 0] },
    ];
    expect(nearestProjectedEdge(tie, [5, 1], 5)).toBe(0);
  });

  it("returns null for an empty edge list", () => {
    expect(nearestProjectedEdge([], [0, 0], 100)).toBeNull();
  });

  it("picks the edge when the click lands exactly on an endpoint", () => {
    // Shared corner (10,0) belongs to edges 0 and 1; first wins the tie.
    expect(nearestProjectedEdge(edges, [10, 0], 0.001)).toBe(0);
  });

  it("honours the tolerance boundary inclusively", () => {
    // 2 mm from the bottom edge; tol exactly 2 → hit, tol just under → miss.
    expect(nearestProjectedEdge(edges, [5, 2], 2)).toBe(0);
    expect(nearestProjectedEdge(edges, [5, 2], 1.999)).toBeNull();
  });
});

describe("parseQEdgeIdHash", () => {
  it("extracts the decimal id-hash", () => {
    expect(parseQEdgeIdHash("qEdge(12345)")).toBe("12345");
    expect(parseQEdgeIdHash("  qEdge( 42 ) ")).toBe("42");
  });

  it("rejects non-edge or malformed queries", () => {
    expect(parseQEdgeIdHash("qFace(1)")).toBeNull();
    expect(parseQEdgeIdHash("qEdge(abc)")).toBeNull();
    expect(parseQEdgeIdHash("qEdgeBetween(1,2)")).toBeNull();
    expect(parseQEdgeIdHash(undefined)).toBeNull();
    expect(parseQEdgeIdHash(123)).toBeNull();
  });
});

describe("resolveProjectedSubCalls", () => {
  // A live horizontal edge from (1,2,5)→(4,2,5), keyed by id-hash "7".
  const lookup = (idHash: string): number[] | null =>
    idHash === "7" ? [1, 2, 5, 4, 2, 5] : null;

  const projectCall = (over: Partial<Record<string, unknown>> = {}): SketchSubCall => ({
    type: "sketch_project",
    id: 200,
    params: {
      edge: "qEdge(7)",
      start: [0, 0],
      end: [0, 0],
      construction: true,
      ...over,
    },
  });

  it("rewrites a sketch_project to a live sketch_line on its frame", () => {
    const [out] = resolveProjectedSubCalls([projectCall()], TOP, lookup);
    expect(out.type).toBe("sketch_line");
    expect(out.id).toBe(200);
    // On the TOP frame (origin z=5) the world z drops out → 2D = world xy.
    expect(out.params.start).toEqual([1, 2]);
    expect(out.params.end).toEqual([4, 2]);
    expect(out.params.construction).toBe(true);
  });

  it("FOLLOWS the body: a moved edge yields moved endpoints", () => {
    const moved = (idHash: string): number[] | null =>
      idHash === "7" ? [1, 2, 5, 9, 2, 5] : null; // end x: 4 → 9
    const [out] = resolveProjectedSubCalls([projectCall()], TOP, moved);
    expect(out.params.end).toEqual([9, 2]);
  });

  it("projects through a non-identity origin (relative to frame)", () => {
    // Same world edge, but the frame is the box-top frame: the projection is
    // relative to the origin only in xy here (origin xy = 0), so unchanged.
    // Shift the origin in x to prove relativity.
    const shifted: PlaneFrame = { ...XY, origin: [1, 0, 0] };
    const [out] = resolveProjectedSubCalls(
      [projectCall()],
      shifted,
      lookup,
    );
    expect(out.params.start).toEqual([0, 2]); // (1-1, 2-0)
    expect(out.params.end).toEqual([3, 2]); // (4-1, 2-0)
  });

  it("falls back to cached endpoints when the edge no longer resolves", () => {
    const gone = (): number[] | null => null;
    const [out] = resolveProjectedSubCalls(
      [projectCall({ start: [11, 12], end: [13, 14] })],
      TOP,
      gone,
    );
    expect(out.type).toBe("sketch_line");
    expect(out.params.start).toEqual([11, 12]);
    expect(out.params.end).toEqual([13, 14]);
  });

  it("drops a projection with neither a live edge nor cached endpoints", () => {
    const gone = (): number[] | null => null;
    const out = resolveProjectedSubCalls(
      [
        {
          type: "sketch_project",
          id: 1,
          params: { edge: "qEdge(99)" },
        },
      ],
      TOP,
      gone,
    );
    expect(out).toHaveLength(0);
  });

  it("defaults construction to true when the flag is absent", () => {
    const [out] = resolveProjectedSubCalls(
      [{ type: "sketch_project", id: 1, params: { edge: "qEdge(7)" } }],
      TOP,
      lookup,
    );
    expect(out.params.construction).toBe(true);
  });

  it("preserves an explicit construction=false", () => {
    const [out] = resolveProjectedSubCalls(
      [projectCall({ construction: false })],
      TOP,
      lookup,
    );
    expect(out.params.construction).toBe(false);
  });

  it("passes non-projection sub-calls through untouched", () => {
    const line: SketchSubCall = {
      type: "sketch_line",
      id: 5,
      params: { start: [0, 0], end: [1, 1] },
    };
    const circle: SketchSubCall = {
      type: "sketch_circle",
      id: 6,
      params: { center: [2, 2], radius: 3 },
    };
    const out = resolveProjectedSubCalls([line, circle, projectCall()], TOP, lookup);
    expect(out[0]).toBe(line);
    expect(out[1]).toBe(circle);
    expect(out[2].type).toBe("sketch_line");
  });
});

describe("makeLiveEdgeLookup", () => {
  it("indexes edges across all bodies by id-hash", () => {
    const kernel = {
      bodyCount: () => 2,
      bodyBrepEdges: (i: number) =>
        i === 0
          ? [{ points: [0, 0, 0, 1, 0, 0], idHash: "10" }]
          : [{ points: [2, 2, 2, 3, 2, 2], idHash: "20" }],
    };
    const lookup = makeLiveEdgeLookup(kernel);
    expect(lookup("10")).toEqual([0, 0, 0, 1, 0, 0]);
    expect(lookup("20")).toEqual([2, 2, 2, 3, 2, 2]);
    expect(lookup("missing")).toBeNull();
  });

  it("tolerates a throwing bodyBrepEdges and a missing idHash", () => {
    const kernel = {
      bodyCount: () => 2,
      bodyBrepEdges: (i: number) => {
        if (i === 0) throw new Error("not woven yet");
        return [{ points: [0, 0, 0, 1, 0, 0] }]; // no idHash
      },
    };
    const lookup = makeLiveEdgeLookup(kernel);
    expect(lookup("anything")).toBeNull();
  });
});
