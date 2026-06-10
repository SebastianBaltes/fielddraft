import { describe, expect, it } from "vitest";
import {
  planeBasisFromNormal,
  planeToWorld,
  rayToPlaneUv,
  resolvePlaneFrame,
  type PlaneFrameKernel,
} from "./sketchPlaneFrame";
import type { BRepFace, Placement } from "./kernel";

const IDENTITY: Placement = {
  translate: [0, 0, 0],
  rotation: [0, 0, 0, 1],
  scale: [1, 1, 1],
};

function face(
  idHash: string,
  featureId: number,
  centroid: [number, number, number],
  normal: [number, number, number],
): BRepFace {
  return {
    faceIndex: 0,
    idHash,
    kind: "face",
    branchId: featureId << 8,
    featureId,
    surfaceKind: "plane",
    centroidLocal: centroid,
    normalLocal: normal,
  };
}

function kernelWith(faces: BRepFace[], placement = IDENTITY): PlaneFrameKernel {
  return {
    bodyCount: () => 1,
    bodyBrepFaces: () => faces,
    bodyPlacement: () => placement,
  };
}

describe("sketchPlaneFrame (tickets yx6a/bxoo)", () => {
  it("resolves the three datum planes with the kernel's frames", () => {
    const k = kernelWith([]);
    expect(resolvePlaneFrame("qCreatedBy(origin_plane_xy)", k)).toEqual({
      origin: [0, 0, 0],
      xAxis: [1, 0, 0],
      yAxis: [0, 1, 0],
      normal: [0, 0, 1],
    });
    expect(resolvePlaneFrame("qCreatedBy(origin_plane_xz)", k)?.normal).toEqual([0, 1, 0]);
    expect(resolvePlaneFrame("qCreatedBy(origin_plane_yz)", k)?.normal).toEqual([1, 0, 0]);
  });

  it("resolves qFace by stable hash with world centroid + normal", () => {
    const k = kernelWith([face("42", 7, [50, 0, 0], [1, 0, 0])]);
    const f = resolvePlaneFrame("qFace(42)", k);
    expect(f).not.toBeNull();
    expect(f!.origin).toEqual([50, 0, 0]);
    expect(f!.normal).toEqual([1, 0, 0]);
    // Kernel seed rule for +X normal: seed [0,1,0] → x=[0,0,-1], y=[0,1,0].
    expect(f!.xAxis.map(Math.fround)).toEqual([0, 0, -1].map(Math.fround));
    expect(f!.yAxis[1]).toBeCloseTo(1, 6);
    expect(Math.abs(f!.yAxis[0])).toBeCloseTo(0, 6);
    expect(Math.abs(f!.yAxis[2])).toBeCloseTo(0, 6);
  });

  it("applies the body placement to centroid and normal", () => {
    // 90° about Z: local +X normal becomes world +Y.
    const s = Math.SQRT1_2;
    const placement: Placement = {
      translate: [10, 0, 0],
      rotation: [0, 0, s, s],
      scale: [1, 1, 1],
    };
    const k = kernelWith([face("9", 3, [5, 0, 0], [1, 0, 0])], placement);
    const f = resolvePlaneFrame("qFace(9)", k)!;
    expect(f.origin[0]).toBeCloseTo(10, 5);
    expect(f.origin[1]).toBeCloseTo(5, 5);
    expect(f.normal[0]).toBeCloseTo(0, 5);
    expect(f.normal[1]).toBeCloseTo(1, 5);
  });

  it("resolves qCreatedBy(featureId) to that feature's first face", () => {
    const k = kernelWith([
      face("1", 2, [0, 0, 5], [0, 0, 1]),
      face("2", 8, [0, 5, 0], [0, 1, 0]),
    ]);
    expect(resolvePlaneFrame("qCreatedBy(8)", k)?.origin).toEqual([0, 5, 0]);
  });

  it("returns null for unknown queries and missing faces", () => {
    const k = kernelWith([]);
    expect(resolvePlaneFrame("qFace(123)", k)).toBeNull();
    expect(resolvePlaneFrame("qEdge(5)", k)).toBeNull();
  });

  it("planeToWorld and rayToPlaneUv round-trip", () => {
    const f = resolvePlaneFrame("qCreatedBy(origin_plane_xz)", kernelWith([]))!;
    const w = planeToWorld(7, -3, f);
    // Shoot a ray straight down onto the XZ plane through that point.
    const uv = rayToPlaneUv([w[0], 100, w[2]], [0, -1, 0], f);
    expect(uv).not.toBeNull();
    expect(uv![0]).toBeCloseTo(7, 5);
    expect(uv![1]).toBeCloseTo(-3, 5);
  });

  it("rayToPlaneUv rejects parallel rays and hits behind the origin", () => {
    const f = resolvePlaneFrame("qCreatedBy(origin_plane_xy)", kernelWith([]))!;
    expect(rayToPlaneUv([0, 0, 10], [1, 0, 0], f)).toBeNull();
    expect(rayToPlaneUv([0, 0, 10], [0, 0, 1], f)).toBeNull();
  });

  it("basis is orthonormal and right-handed for arbitrary normals", () => {
    const n: [number, number, number] = [0.3, -0.5, 0.81];
    const { xAxis, yAxis } = planeBasisFromNormal(n);
    const dot = (a: number[], b: number[]) =>
      a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    expect(dot(xAxis, yAxis)).toBeCloseTo(0, 5);
    expect(dot(xAxis, n) / Math.hypot(...n)).toBeCloseTo(0, 5);
    expect(Math.hypot(...xAxis)).toBeCloseTo(1, 5);
    expect(Math.hypot(...yAxis)).toBeCloseTo(1, 5);
  });
});
