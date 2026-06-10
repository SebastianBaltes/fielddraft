import { describe, expect, it } from "vitest";
import {
  axisDepthParam,
  lengthParamToMm,
  lengthParamWithMm,
  pickTipFace,
  resolveExtrudeAnchor,
  type ExtrudeAnchorKernel,
  type TipFaceCandidate,
  type Vec3,
} from "./extrudeGizmo";
import type { BRepEdge, BRepFace, Placement } from "./kernel";

const IDENTITY: Placement = {
  translate: [0, 0, 0],
  rotation: [0, 0, 0, 1],
  scale: [1, 1, 1],
};

const cand = (normal: Vec3, centroid: Vec3, index: number): TipFaceCandidate => ({
  normal,
  centroid,
  index,
});

describe("pickTipFace", () => {
  it("picks the cap among prism walls (max perpendicular siblings)", () => {
    // Cap at +X plus 4 side walls of a prism grown in +X.
    const picked = pickTipFace([
      cand([0, 1, 0], [55, 50, 0], 0),
      cand([0, -1, 0], [55, -50, 0], 1),
      cand([1, 0, 0], [60, 0, 0], 2), // cap
      cand([0, 0, 1], [55, 0, 50], 3),
      cand([0, 0, -1], [55, 0, -50], 4),
    ]);
    expect(picked?.index).toBe(2);
  });

  it("breaks cap/base ties toward the outward-most face", () => {
    // An unmerged "New" prism: cap at +X 60, base at +X 40 (normal −X).
    const picked = pickTipFace([
      cand([1, 0, 0], [60, 0, 0], 0),
      cand([-1, 0, 0], [40, 0, 0], 1),
      cand([0, 1, 0], [50, 10, 0], 2),
      cand([0, -1, 0], [50, -10, 0], 3),
    ]);
    expect(picked?.index).toBe(0);
  });

  it("handles empty and single-candidate lists", () => {
    expect(pickTipFace([])).toBeNull();
    expect(pickTipFace([cand([0, 0, 1], [0, 0, 5], 7)])?.index).toBe(7);
  });
});

describe("axisDepthParam", () => {
  it("recovers the param of a point on the axis", () => {
    // Axis +Z from origin; ray from the side aiming at (0,0,30).
    const t = axisDepthParam(
      [0, 0, 0],
      [0, 0, 1],
      [100, 0, 30],
      [-1, 0, 0],
    );
    expect(t).toBeCloseTo(30, 6);
  });

  it("is null for a ray parallel to the axis", () => {
    expect(
      axisDepthParam([0, 0, 0], [0, 0, 1], [5, 0, 0], [0, 0, 1]),
    ).toBeNull();
  });
});

describe("length params", () => {
  it("reads wire lengths in any unit as mm", () => {
    expect(lengthParamToMm({ value: 10, unit: "mm" })).toBe(10);
    expect(lengthParamToMm({ value: 2.5, unit: "cm" })).toBe(25);
    expect(lengthParamToMm({ value: 1, unit: "in" })).toBeCloseTo(25.4);
    expect(lengthParamToMm(12)).toBe(12);
    expect(lengthParamToMm("7.5")).toBe(7.5);
    expect(lengthParamToMm({ value: "x", unit: "mm" })).toBeNull();
  });

  it("re-encodes mm preserving the original unit", () => {
    expect(lengthParamWithMm({ value: 1, unit: "cm" }, 25)).toEqual({
      value: 2.5,
      unit: "cm",
    });
    expect(lengthParamWithMm(undefined, 25)).toEqual({ value: 25, unit: "mm" });
  });
});

describe("resolveExtrudeAnchor", () => {
  const face = (
    idHash: string,
    featureId: number,
    branchId: number,
    centroid: Vec3,
    normal: Vec3,
    surfaceKind: BRepFace["surfaceKind"] = "plane",
  ): BRepFace => ({
    faceIndex: 0,
    idHash,
    kind: "face",
    branchId,
    featureId,
    surfaceKind,
    centroidLocal: centroid,
    normalLocal: normal,
  });

  const edge = (a: number, b: number, points: number[]): BRepEdge => ({
    a,
    b,
    points,
    controlPoints: [],
    degree: 1,
    idHash: "e",
  });

  function kernelWith(faces: BRepFace[], edges: BRepEdge[]): ExtrudeAnchorKernel {
    return {
      bodyCount: () => 1,
      bodyBrepFaces: () => faces,
      bodyBrepEdges: () => edges,
      bodyPlacement: () => IDENTITY,
    };
  }

  it("finds the tip face and its outline for the extrude's branch", () => {
    const EXT = 7;
    const capBranch = (EXT << 12) | 1;
    const k = kernelWith(
      [
        face("box", 1, 1 << 12, [0, 0, 0], [0, 0, -1]),
        face("cap", EXT, capBranch, [55, 0, 0], [1, 0, 0]),
        face("side1", EXT, (EXT << 12) | 2, [52, 5, 0], [0, 1, 0]),
        face("side2", EXT, (EXT << 12) | 3, [52, -5, 0], [0, -1, 0]),
      ],
      [
        edge(capBranch, (EXT << 12) | 2, [55, 5, -5, 55, 5, 5]),
        edge((EXT << 12) | 3, capBranch, [55, -5, -5, 55, -5, 5]),
        edge(1 << 12, 1 << 12, [0, 0, 0, 1, 1, 1]), // unrelated
      ],
    );
    const anchor = resolveExtrudeAnchor(k, EXT);
    expect(anchor).not.toBeNull();
    expect(anchor!.origin).toEqual([55, 0, 0]);
    expect(anchor!.normal[0]).toBeCloseTo(1);
    expect(anchor!.outline).toHaveLength(2);
  });

  it("returns null when the feature owns no planar face", () => {
    const k = kernelWith([face("f", 3, 3 << 12, [0, 0, 0], [0, 0, 1])], []);
    expect(resolveExtrudeAnchor(k, 99)).toBeNull();
  });
});
