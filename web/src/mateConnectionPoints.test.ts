import { describe, it, expect } from "vitest";
import {
  enumerateConnectionPoints,
  seedCrossX,
  mateFrameFor,
  reorientSecondary,
  type ConnectionKernel,
  type MateConnectionPoint,
} from "./mateConnectionPoints";
import type { BRepFace, BRepVertex, BRepEdge, Placement } from "./kernel";

const IDENTITY: Placement = {
  translate: [0, 0, 0],
  rotation: [0, 0, 0, 1],
  scale: [1, 1, 1],
};

/** A toy single-body kernel: one +Z planar face (centroid 0,0,10), one top
 *  edge along +Y at x=10 (faces=[0]), one top corner vertex at (10,10,10). */
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
      points: [10, -10, 10, 10, 10, 10], // tangent +Y, midpoint (10,0,10)
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
  const byKind = (k: MateConnectionPoint["kind"]) =>
    pts.find((p) => p.kind === k)!;

  it("face-centre: +Z = face normal, no secondary refs", () => {
    const fc = byKind("face-center");
    expect(fc.world).toEqual([0, 0, 10]);
    expect(fc.anchorQuery).toBe("qFace(100)");
    expect(fc.primaryAxis).toEqual([0, 0, 1]);
    expect(fc.secondaryRefs).toHaveLength(0);
  });

  it("edge midpoint: +Z = edge tangent, adjacent face as secondary ref", () => {
    const em = byKind("edge-mid");
    expect(em.anchorQuery).toBe("qEdge(200)");
    expect(em.t).toBe(0.5);
    expect(em.world[0]).toBeCloseTo(10);
    expect(em.world[1]).toBeCloseTo(0);
    expect(em.world[2]).toBeCloseTo(10);
    // Primary +Z is the edge direction (+Y), NOT the face normal (ticket o3er).
    expect(em.primaryAxis![1]).toBeCloseTo(1);
    expect(Math.abs(em.primaryAxis![0])).toBeCloseTo(0);
    expect(Math.abs(em.primaryAxis![2])).toBeCloseTo(0);
    expect(em.secondaryRefs).toHaveLength(1);
    expect(em.secondaryRefs[0].faceQuery).toBe("qFace(100)");
    expect(em.secondaryRefs[0].normal).toEqual([0, 0, 1]);
  });

  it("vertex: no intrinsic axis, adjacent face as secondary ref", () => {
    const v = byKind("vertex");
    expect(v.anchorQuery).toBe("qVertex(300)");
    expect(v.world).toEqual([10, 10, 10]);
    expect(v.primaryAxis).toBeNull();
    expect(v.secondaryRefs).toHaveLength(1);
    expect(v.secondaryRefs[0].faceQuery).toBe("qFace(100)");
  });
});

describe("mateFrameFor", () => {
  const pts = enumerateConnectionPoints(toyKernel());
  const edge = pts.find((p) => p.kind === "edge-mid")!;

  it("edge: +Z = tangent, X ⟂ Z; reorient rotates X 90°", () => {
    const f0 = mateFrameFor(edge, edge.secondaryRefs[0].normal, 0, false);
    // Z = +Y (tangent), X = the face normal projected ⟂ Z = +Z.
    expect(f0.z[1]).toBeCloseTo(1);
    expect(Math.abs(f0.z[2])).toBeCloseTo(0);
    expect(f0.x[0] * f0.z[0] + f0.x[1] * f0.z[1] + f0.x[2] * f0.z[2]).toBeCloseTo(0);
    expect(f0.x[2]).toBeCloseTo(1);

    const f1 = mateFrameFor(edge, edge.secondaryRefs[0].normal, 1, false);
    // 90°: X' = Z × X = +Y × +Z = +X.
    expect(f1.x[0]).toBeCloseTo(1);
  });

  it("flip reverses +Z", () => {
    const f = mateFrameFor(edge, edge.secondaryRefs[0].normal, 0, true);
    expect(f.z[1]).toBeCloseTo(-1);
  });
});

describe("reorientSecondary", () => {
  const z: [number, number, number] = [0, 0, 1];
  const x: [number, number, number] = [1, 0, 0];
  it("cycles back after 4 steps", () => {
    expect(reorientSecondary(x, z, 0)).toEqual(x);
    expect(reorientSecondary(x, z, 4)).toEqual(x);
    expect(reorientSecondary(x, z, 2)).toEqual([-1, -0, -0]);
    // 90°: Z × X = +Y.
    expect(reorientSecondary(x, z, 1)).toEqual([0, 1, 0]);
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
