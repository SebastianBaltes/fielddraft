import { describe, it, expect } from "vitest";
import {
  enumerateConnectionPoints,
  seedCrossX,
  type ConnectionKernel,
} from "./mateConnectionPoints";
import type { BRepFace, BRepVertex, BRepEdge, Placement } from "./kernel";

const IDENTITY: Placement = {
  translate: [0, 0, 0],
  rotation: [0, 0, 0, 1],
  scale: [1, 1, 1],
};

/** A toy single-body kernel: one +Z planar face (centroid 0,0,10), one top
 *  edge along Y at x=10 (faces=[0]), one top corner vertex at (10,10,10). */
function toyKernel(): ConnectionKernel {
  const faces: BRepFace[] = [
    {
      faceIndex: 0,
      idHash: "100",
      kind: "face",
      branchId: 0,
      featureId: 1,
      surfaceKind: "plane",
      centroidLocal: [0, 0, 10],
      normalLocal: [0, 0, 1],
    },
  ];
  const edges: BRepEdge[] = [
    {
      a: 0,
      b: 0,
      // (10,-10,10) → (10,10,10): tangent +Y, midpoint (10,0,10).
      points: [10, -10, 10, 10, 10, 10],
      controlPoints: [],
      degree: 1,
      idHash: "200",
      faces: [0],
    },
  ];
  const vertices: BRepVertex[] = [
    {
      vertexIndex: 0,
      idHash: "300",
      kind: "vertex",
      positionLocal: [10, 10, 10],
      branches: [0],
    },
  ];
  return {
    bodyCount: () => 1,
    bodyBrepFaces: () => faces,
    bodyBrepVertices: () => vertices,
    bodyBrepEdges: () => edges,
    bodyPlacement: () => IDENTITY,
  };
}

describe("enumerateConnectionPoints", () => {
  const pts = enumerateConnectionPoints(toyKernel());

  it("emits a face-centre with a null primary (uses the anchor face's normal)", () => {
    const fc = pts.find((p) => p.kind === "face-center")!;
    expect(fc).toBeTruthy();
    expect(fc.world).toEqual([0, 0, 10]);
    expect(fc.anchorQuery).toBe("qFace(100)");
    expect(fc.orientations).toHaveLength(1);
    expect(fc.orientations[0].primaryQuery).toBeNull();
    expect(fc.orientations[0].z).toEqual([0, 0, 1]);
  });

  it("emits an edge midpoint with a +Z-oriented variant and X along the edge", () => {
    const em = pts.find((p) => p.kind === "edge-mid")!;
    expect(em).toBeTruthy();
    expect(em.anchorQuery).toBe("qEdge(200)");
    expect(em.t).toBe(0.5);
    expect(em.world[0]).toBeCloseTo(10);
    expect(em.world[1]).toBeCloseTo(0);
    expect(em.world[2]).toBeCloseTo(10);
    expect(em.orientations).toHaveLength(1);
    const o = em.orientations[0];
    expect(o.primaryQuery).toBe("qFace(100)");
    expect(o.z).toEqual([0, 0, 1]);
    // Tangent +Y projected ⟂ +Z stays +Y.
    expect(o.x[0]).toBeCloseTo(0);
    expect(Math.abs(o.x[1])).toBeCloseTo(1);
    expect(o.x[2]).toBeCloseTo(0);
    // X ⟂ Z.
    expect(o.x[0] * o.z[0] + o.x[1] * o.z[1] + o.x[2] * o.z[2]).toBeCloseTo(0);
  });

  it("emits a vertex with the adjacent face's orientation", () => {
    const v = pts.find((p) => p.kind === "vertex")!;
    expect(v).toBeTruthy();
    expect(v.anchorQuery).toBe("qVertex(300)");
    expect(v.world).toEqual([10, 10, 10]);
    expect(v.orientations).toHaveLength(1);
    expect(v.orientations[0].primaryQuery).toBe("qFace(100)");
    expect(v.orientations[0].z).toEqual([0, 0, 1]);
  });
});

describe("seedCrossX", () => {
  it("returns a unit vector perpendicular to the normal", () => {
    for (const n of [
      [0, 0, 1],
      [1, 0, 0],
      [0, 1, 0],
      [0.3, 0.4, 0.5],
    ] as [number, number, number][]) {
      const x = seedCrossX(n);
      expect(Math.hypot(...x)).toBeCloseTo(1);
      expect(x[0] * n[0] + x[1] * n[1] + x[2] * n[2]).toBeCloseTo(0);
    }
  });
});
