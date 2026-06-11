import { describe, expect, it } from "vitest";
import {
  bodiesInRect,
  elementsInRect,
  type BoxSelectKernel,
  type NdcRect,
  type ProjectFn,
} from "./boxSelect";
import type { BRepEdge, BRepVertex, Placement } from "./kernel";

const IDENTITY: Placement = {
  translate: [0, 0, 0],
  rotation: [0, 0, 0, 1],
  scale: [1, 1, 1],
};

/** Orthographic-style projection: world x/y map straight to NDC, z ignored. */
const projectXY: ProjectFn = (x, y) => ({ x, y });

function edge(idHash: string, pts: number[][]): BRepEdge {
  return {
    a: 1 << 12,
    b: 2 << 12,
    points: pts.flat(),
    controlPoints: [],
    faces: [],
    degree: 1,
    idHash,
  };
}

function vertex(idHash: string, p: [number, number, number]): BRepVertex {
  return { vertexIndex: 0, idHash, kind: "vertex", positionLocal: p, branches: [] };
}

function kernelWith(bodies: {
  edges: BRepEdge[];
  vertices: BRepVertex[];
  aabb?: number[];
}[]): BoxSelectKernel {
  return {
    bodyCount: () => bodies.length,
    bodyBrepEdges: (i) => bodies[i].edges,
    bodyBrepVertices: (i) => bodies[i].vertices,
    bodyPlacement: () => IDENTITY,
    bodySdfAabb: (i) => new Float32Array(bodies[i].aabb ?? []),
  };
}

const RECT: NdcRect = { minX: -0.5, minY: -0.5, maxX: 0.5, maxY: 0.5 };

describe("boxSelect", () => {
  const inEdge = edge("e-in", [[0, 0, 0], [0.2, 0.1, 0]]);
  const crossingEdge = edge("e-cross", [[0, 0, 0], [0.9, 0, 0]]);
  const outEdge = edge("e-out", [[0.8, 0.8, 0], [0.9, 0.9, 0]]);

  it("window mode keeps only fully-contained edges and vertices", () => {
    const k = kernelWith([
      {
        edges: [inEdge, crossingEdge, outEdge],
        vertices: [vertex("v-in", [0.1, 0.1, 0]), vertex("v-out", [0.9, 0, 0])],
      },
    ]);
    const els = elementsInRect(k, projectXY, RECT, "window");
    expect(els).toEqual([
      { kind: "edge", idHash: "e-in" },
      { kind: "vertex", idHash: "v-in" },
    ]);
  });

  it("crossing mode additionally selects partially covered edges", () => {
    const k = kernelWith([
      { edges: [inEdge, crossingEdge, outEdge], vertices: [] },
    ]);
    const els = elementsInRect(k, projectXY, RECT, "crossing");
    expect(els.map((e) => e.idHash).sort()).toEqual(["e-cross", "e-in"]);
  });

  it("points behind the camera count as outside", () => {
    const projectClip: ProjectFn = (x, y, z) => (z > 0 ? null : { x, y });
    const k = kernelWith([
      { edges: [edge("e-half", [[0, 0, -1], [0, 0.1, 1]])], vertices: [] },
    ]);
    expect(elementsInRect(k, projectClip, RECT, "window")).toEqual([]);
    expect(elementsInRect(k, projectClip, RECT, "crossing")).toEqual([
      { kind: "edge", idHash: "e-half" },
    ]);
  });

  it("selects whole bodies by their edge set in object mode", () => {
    const k = kernelWith([
      { edges: [inEdge], vertices: [] },
      { edges: [outEdge], vertices: [] },
    ]);
    expect(bodiesInRect(k, projectXY, RECT, "window")).toEqual([0]);
  });

  it("falls back to the AABB corners for edge-free bodies (sphere)", () => {
    const sphereInside = { edges: [], vertices: [], aabb: [-0.2, -0.2, -0.2, 0.2, 0.2, 0.2] };
    const sphereOutside = { edges: [], vertices: [], aabb: [0.4, 0.4, 0.4, 2, 2, 2] };
    const k = kernelWith([sphereInside, sphereOutside]);
    expect(bodiesInRect(k, projectXY, RECT, "window")).toEqual([0]);
    // crossing also catches the half-covered sphere
    expect(bodiesInRect(k, projectXY, RECT, "crossing")).toEqual([0, 1]);
  });
});
