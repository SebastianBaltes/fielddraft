import { describe, expect, it } from "vitest";
import {
  nearestProjectedEdge,
  pointToSegmentDist,
  type ProjectedEdgeSeg,
} from "./sketchProject";

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
