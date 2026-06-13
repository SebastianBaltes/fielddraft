import { describe, it, expect } from "vitest";
import {
  tessellatePlanarBody,
  collectPlanarBodyMeshes,
  type BrepMeshKernel,
  type BrepMeshFace,
  type BrepMeshEdge,
} from "./brepMesh";

type V3 = [number, number, number];

type Quat = [number, number, number, number];
interface Placement { translate: V3; rotation: Quat; scale: V3 }
const IDENTITY: Placement = { translate: [0, 0, 0], rotation: [0, 0, 0, 1], scale: [1, 1, 1] };

/** Same transform the kernel applies — used to put edge points into WORLD
 *  space (as real `BRepEdge.points` already are), while faces stay local. */
function toWorld(p: V3, pl: Placement): V3 {
  const sx = p[0] * pl.scale[0], sy = p[1] * pl.scale[1], sz = p[2] * pl.scale[2];
  const [qx, qy, qz, qw] = pl.rotation;
  const tx = 2 * (qy * sz - qz * sy);
  const ty = 2 * (qz * sx - qx * sz);
  const tz = 2 * (qx * sy - qy * sx);
  return [
    sx + qw * tx + qy * tz - qz * ty + pl.translate[0],
    sy + qw * ty + qz * tx - qx * tz + pl.translate[1],
    sz + qw * tz + qx * ty - qy * tx + pl.translate[2],
  ];
}

/**
 * Synthetic axis-aligned cube kernel: 6 planar faces + 12 edges derived from
 * the face boundaries (each edge shared by exactly two faces). Faces carry
 * LOCAL centroid/normal; edge points are lifted to WORLD by `placement` —
 * mirroring the real kernel. `overrideFaceKind` poisons one face's surfaceKind.
 */
function makeCubeKernel(
  half = 1,
  overrideFaceKind?: { index: number; kind: string },
  placement: Placement = IDENTITY,
): BrepMeshKernel {
  const c = (x: number, y: number, z: number): V3 => [x * half, y * half, z * half];
  const faceDefs = [
    { faceIndex: 0, normal: [1, 0, 0] as V3, centroid: [half, 0, 0] as V3, corners: [c(1, -1, -1), c(1, 1, -1), c(1, 1, 1), c(1, -1, 1)] },
    { faceIndex: 1, normal: [-1, 0, 0] as V3, centroid: [-half, 0, 0] as V3, corners: [c(-1, -1, -1), c(-1, -1, 1), c(-1, 1, 1), c(-1, 1, -1)] },
    { faceIndex: 2, normal: [0, 1, 0] as V3, centroid: [0, half, 0] as V3, corners: [c(-1, 1, -1), c(-1, 1, 1), c(1, 1, 1), c(1, 1, -1)] },
    { faceIndex: 3, normal: [0, -1, 0] as V3, centroid: [0, -half, 0] as V3, corners: [c(-1, -1, -1), c(1, -1, -1), c(1, -1, 1), c(-1, -1, 1)] },
    { faceIndex: 4, normal: [0, 0, 1] as V3, centroid: [0, 0, half] as V3, corners: [c(-1, -1, 1), c(1, -1, 1), c(1, 1, 1), c(-1, 1, 1)] },
    { faceIndex: 5, normal: [0, 0, -1] as V3, centroid: [0, 0, -half] as V3, corners: [c(-1, -1, -1), c(-1, 1, -1), c(1, 1, -1), c(1, -1, -1)] },
  ];
  const faces: BrepMeshFace[] = faceDefs.map((f) => ({
    faceIndex: f.faceIndex,
    surfaceKind:
      overrideFaceKind && overrideFaceKind.index === f.faceIndex ? overrideFaceKind.kind : "plane",
    centroidLocal: f.centroid,
    normalLocal: f.normal,
  }));

  const key = (p: V3) => p.map((n) => n.toFixed(3)).join(",");
  const edgeMap = new Map<string, { points: number[]; faces: Set<number> }>();
  for (const f of faceDefs) {
    for (let i = 0; i < 4; i++) {
      const a = f.corners[i];
      const b = f.corners[(i + 1) % 4];
      const ka = key(a);
      const kb = key(b);
      const pk = ka < kb ? `${ka}|${kb}` : `${kb}|${ka}`;
      let e = edgeMap.get(pk);
      if (!e) {
        e = { points: [...toWorld(a, placement), ...toWorld(b, placement)], faces: new Set() };
        edgeMap.set(pk, e);
      }
      e.faces.add(f.faceIndex);
    }
  }
  const edges: BrepMeshEdge[] = [...edgeMap.values()].map((e) => ({
    points: e.points,
    faces: [...e.faces],
  }));

  return {
    bodyCount: () => 1,
    bodyPlacement: () => placement,
    bodyBrepFaces: () => faces,
    bodyBrepEdges: () => edges,
  };
}

describe("tessellatePlanarBody", () => {
  it("derives 12 edges shared pairwise (sanity of the test fixture)", () => {
    const k = makeCubeKernel();
    const edges = k.bodyBrepEdges(0);
    expect(edges.length).toBe(12);
    expect(edges.every((e) => e.faces.length === 2)).toBe(true);
  });

  it("tessellates an all-planar cube into 12 triangles (6 quads)", () => {
    const k = makeCubeKernel();
    const mesh = tessellatePlanarBody(k, 0);
    expect(mesh).not.toBeNull();
    // 6 faces × 2 triangles × 3 verts × 3 floats = 108.
    expect(mesh!.positions.length).toBe(108);
    expect(mesh!.normals.length).toBe(108);
    expect(mesh!.bodyIndex).toBe(0);
  });

  it("emits unit normals — the 6 distinct cube-face directions", () => {
    const mesh = tessellatePlanarBody(makeCubeKernel(), 0)!;
    const seen = new Set<string>();
    for (let i = 0; i < mesh.normals.length; i += 3) {
      const n: V3 = [mesh.normals[i], mesh.normals[i + 1], mesh.normals[i + 2]];
      expect(Math.hypot(...n)).toBeCloseTo(1, 6);
      seen.add(n.map((x) => Math.round(x)).join(","));
    }
    expect(seen).toEqual(
      new Set(["1,0,0", "-1,0,0", "0,1,0", "0,-1,0", "0,0,1", "0,0,-1"]),
    );
  });

  it("respects a translated placement (origin lifted to the world plane)", () => {
    // World edges shifted +10 in X, centroid stays local + placement.translate
    // = +10 → originW must land on the shifted plane, else triangulation fails.
    const k = makeCubeKernel(1, undefined, {
      translate: [10, 0, 0],
      rotation: [0, 0, 0, 1],
      scale: [1, 1, 1],
    });
    const mesh = tessellatePlanarBody(k, 0);
    expect(mesh).not.toBeNull();
    let minX = Infinity;
    for (let i = 0; i < mesh!.positions.length; i += 3) minX = Math.min(minX, mesh!.positions[i]);
    expect(minX).toBeCloseTo(9, 5); // cube [-1,1] shifted by +10 → [9,11]
  });

  it("rotates the face normal by the placement (45° about Z)", () => {
    const s = Math.SQRT1_2; // sin/cos 45°
    const k = makeCubeKernel(1, undefined, {
      translate: [0, 0, 0],
      rotation: [0, 0, Math.sin(Math.PI / 8), Math.cos(Math.PI / 8)], // 45° about Z
      scale: [1, 1, 1],
    });
    const mesh = tessellatePlanarBody(k, 0)!;
    // The local +X face normal [1,0,0] must appear rotated to ≈ [√½, √½, 0].
    let found = false;
    for (let i = 0; i < mesh.normals.length; i += 3) {
      if (
        Math.abs(mesh.normals[i] - s) < 1e-6 &&
        Math.abs(mesh.normals[i + 1] - s) < 1e-6 &&
        Math.abs(mesh.normals[i + 2]) < 1e-6
      ) {
        found = true;
        break;
      }
    }
    expect(found).toBe(true);
  });

  it("returns null when any face is non-planar (→ caller raymarches)", () => {
    const k = makeCubeKernel(1, { index: 2, kind: "cylinder" });
    expect(tessellatePlanarBody(k, 0)).toBeNull();
  });

  it("returns null for a body with no faces yet (mid-regen)", () => {
    const k: BrepMeshKernel = {
      bodyCount: () => 1,
      bodyPlacement: () => ({ translate: [0, 0, 0], rotation: [0, 0, 0, 1], scale: [1, 1, 1] }),
      bodyBrepFaces: () => [],
      bodyBrepEdges: () => [],
    };
    expect(tessellatePlanarBody(k, 0)).toBeNull();
  });
});

describe("collectPlanarBodyMeshes", () => {
  it("returns the planar body and omits a curved one", () => {
    const planar = makeCubeKernel();
    expect(collectPlanarBodyMeshes(planar)).toHaveLength(1);

    const curved = makeCubeKernel(1, { index: 0, kind: "sphere" });
    expect(collectPlanarBodyMeshes(curved)).toHaveLength(0);
  });
});
