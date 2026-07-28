import { describe, expect, it } from "vitest";
import {
  SUB_BITS,
  FEATURE_SHIFT,
  bodyFeatureIds,
  branchFeatureId,
  resolveSelectionLinks,
  type SelectionLinkKernel,
} from "./selectionLinks";
import type { BRepEdge, BRepFace, BRepVertex } from "./kernel";

const pack = (feature: number, sub = 0, instance = 0) =>
  feature * 2 ** FEATURE_SHIFT + instance * 2 ** SUB_BITS + sub;

function face(idHash: string, featureId: number): BRepFace {
  return {
    faceIndex: 0,
    idHash,
    kind: "face",
    branchId: pack(featureId, 3),
    featureId,
    surfaceKind: "plane",
    centroidLocal: [0, 0, 0],
    normalLocal: [0, 0, 1],
  };
}

function edge(idHash: string, featA: number, featB: number): BRepEdge {
  return {
    a: pack(featA, 1),
    b: pack(featB, 2),
    points: [],
    controlPoints: [],
    faces: [],
    degree: 1,
    idHash,
  };
}

function vertex(idHash: string, feats: number[]): BRepVertex {
  return {
    vertexIndex: 0,
    idHash,
    kind: "vertex",
    positionLocal: [0, 0, 0],
    branches: feats.map((f) => pack(f, 7)),
  };
}

/** Two bodies: body 0 owned by features 1+2, body 1 by feature 5. */
function stubKernel(): SelectionLinkKernel {
  const bodies = [
    {
      faces: [face("f-box", 1), face("f-boss", 2)],
      edges: [edge("e-seam", 1, 2)],
      vertices: [vertex("v-corner", [1, 2])],
    },
    {
      faces: [face("f-other", 5)],
      edges: [edge("e-other", 5, 5)],
      vertices: [vertex("v-other", [5])],
    },
  ];
  return {
    bodyCount: () => bodies.length,
    bodyBrepFaces: (i) => bodies[i].faces,
    bodyBrepEdges: (i) => bodies[i].edges,
    bodyBrepVertices: (i) => bodies[i].vertices,
  };
}

describe("selectionLinks", () => {
  it("strips the sub-index bits off a packed branch id", () => {
    expect(branchFeatureId(pack(7, 99))).toBe(7);
    expect(branchFeatureId(pack(0, 1))).toBe(0);
  });

  it("maps a selected face to its owning feature and body", () => {
    const { featureIds, bodyIndex } = resolveSelectionLinks(stubKernel(), [
      { kind: "face", idHash: "f-boss" },
    ]);
    expect([...featureIds]).toEqual([2]);
    expect(bodyIndex).toBe(0);
  });

  it("maps a selected edge to BOTH adjacent features", () => {
    const { featureIds, bodyIndex } = resolveSelectionLinks(stubKernel(), [
      { kind: "edge", idHash: "e-seam" },
    ]);
    expect([...featureIds].sort()).toEqual([1, 2]);
    expect(bodyIndex).toBe(0);
  });

  it("maps a selected vertex through all colliding branches", () => {
    const { featureIds } = resolveSelectionLinks(stubKernel(), [
      { kind: "vertex", idHash: "v-corner" },
    ]);
    expect([...featureIds].sort()).toEqual([1, 2]);
  });

  it("finds elements on later bodies and accumulates multi-selects", () => {
    const { featureIds, bodyIndex } = resolveSelectionLinks(stubKernel(), [
      { kind: "face", idHash: "f-other" },
      { kind: "face", idHash: "f-box" },
    ]);
    expect([...featureIds].sort()).toEqual([1, 5]);
    // Primary body = first body that resolved any selected element.
    expect(bodyIndex).toBe(0);
  });

  it("ignores unknown idHashes (mid-regen) without throwing", () => {
    const { featureIds, bodyIndex } = resolveSelectionLinks(stubKernel(), [
      { kind: "face", idHash: "gone" },
    ]);
    expect(featureIds.size).toBe(0);
    expect(bodyIndex).toBeNull();
  });

  it("lists every feature contributing to a body", () => {
    expect([...bodyFeatureIds(stubKernel(), 0)].sort()).toEqual([1, 2]);
    expect([...bodyFeatureIds(stubKernel(), 1)]).toEqual([5]);
    expect(bodyFeatureIds(stubKernel(), 99).size).toBe(0);
  });
});
