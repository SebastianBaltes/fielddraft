import { describe, it, expect } from "vitest";
import {
  tessellateAnalyticBody,
  tessellateCylinderFace,
  collectAnalyticBodyMeshes,
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

describe("tessellateAnalyticBody", () => {
  it("derives 12 edges shared pairwise (sanity of the test fixture)", () => {
    const k = makeCubeKernel();
    const edges = k.bodyBrepEdges(0);
    expect(edges.length).toBe(12);
    expect(edges.every((e) => e.faces.length === 2)).toBe(true);
  });

  it("tessellates an all-planar cube into 12 triangles (6 quads)", () => {
    const k = makeCubeKernel();
    const mesh = tessellateAnalyticBody(k, 0);
    expect(mesh).not.toBeNull();
    // 6 faces × 2 triangles × 3 verts × 3 floats = 108.
    expect(mesh!.positions.length).toBe(108);
    expect(mesh!.normals.length).toBe(108);
    expect(mesh!.bodyIndex).toBe(0);
  });

  it("emits unit normals — the 6 distinct cube-face directions", () => {
    const mesh = tessellateAnalyticBody(makeCubeKernel(), 0)!;
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
    const mesh = tessellateAnalyticBody(k, 0);
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
    const mesh = tessellateAnalyticBody(k, 0)!;
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
    expect(tessellateAnalyticBody(k, 0)).toBeNull();
  });

  it("returns null for a body with no faces yet (mid-regen)", () => {
    const k: BrepMeshKernel = {
      bodyCount: () => 1,
      bodyPlacement: () => ({ translate: [0, 0, 0], rotation: [0, 0, 0, 1], scale: [1, 1, 1] }),
      bodyBrepFaces: () => [],
      bodyBrepEdges: () => [],
    };
    expect(tessellateAnalyticBody(k, 0)).toBeNull();
  });
});

/** A circle of `n` points (flat [x,y,z,…]) at height `y`, radius `r`, on the
 *  Y axis — a synthetic cap-circle edge polyline. */
function circle(r: number, y: number, n = 64): number[] {
  const pts: number[] = [];
  for (let i = 0; i <= n; i++) {
    const a = (2 * Math.PI * i) / n;
    pts.push(r * Math.cos(a), y, r * Math.sin(a));
  }
  return pts;
}

describe("tessellateCylinderFace", () => {
  it("tessellates a 360° band at the given radius and axial extent", () => {
    const r = 5;
    const boundary = [circle(r, -10), circle(r, 10)];
    const band = tessellateCylinderFace(boundary, [0, 0, 0], [0, 1, 0]);
    expect(band).not.toBeNull();
    // 48 segments × 2 triangles × 3 verts × 3 floats.
    expect(band!.positions.length).toBe(48 * 2 * 3 * 3);
    expect(band!.normals.length).toBe(band!.positions.length);

    for (let i = 0; i < band!.positions.length; i += 3) {
      const x = band!.positions[i], y = band!.positions[i + 1], z = band!.positions[i + 2];
      expect(Math.hypot(x, z)).toBeCloseTo(r, 4); // on the cylinder of radius r
      expect(y).toBeGreaterThanOrEqual(-10 - 1e-6);
      expect(y).toBeLessThanOrEqual(10 + 1e-6);
      const nx = band!.normals[i], ny = band!.normals[i + 1], nz = band!.normals[i + 2];
      expect(Math.hypot(nx, ny, nz)).toBeCloseTo(1, 5); // unit
      expect(Math.abs(ny)).toBeLessThan(1e-6); // radial → ⟂ axis (no Y component)
    }
  });

  it("returns null for a degenerate (zero-height) band", () => {
    const flat = [circle(5, 0), circle(5, 0)];
    expect(tessellateCylinderFace(flat, [0, 0, 0], [0, 1, 0])).toBeNull();
  });
});

/** Synthetic cylinder kernel: 2 planar caps + 1 cylindrical side (with analytic
 *  surfaceParams), sharing the two cap-circle edges. Axis = +Y. */
function makeCylinderKernel(r = 5, halfH = 10): BrepMeshKernel {
  const faces: BrepMeshFace[] = [
    { faceIndex: 0, surfaceKind: "plane", centroidLocal: [0, halfH, 0], normalLocal: [0, 1, 0] },
    { faceIndex: 1, surfaceKind: "plane", centroidLocal: [0, -halfH, 0], normalLocal: [0, -1, 0] },
    {
      faceIndex: 2,
      surfaceKind: "cylinder",
      centroidLocal: [r, 0, 0],
      normalLocal: [1, 0, 0],
      surfaceParams: { kind: "cylinder", origin: [0, 0, 0], axis: [0, 1, 0], radius: r } as { kind: string },
    },
  ];
  const edges: BrepMeshEdge[] = [
    { points: circle(r, halfH), faces: [0, 2] }, // top circle: cap-top + side
    { points: circle(r, -halfH), faces: [1, 2] }, // bottom circle: cap-bottom + side
  ];
  return {
    bodyCount: () => 1,
    bodyPlacement: () => ({ translate: [0, 0, 0], rotation: [0, 0, 0, 1], scale: [1, 1, 1] }),
    bodyBrepFaces: () => faces,
    bodyBrepEdges: () => edges,
  };
}

describe("tessellateAnalyticBody — cylinder (Phase 2)", () => {
  it("tessellates a lone cylinder (2 planar caps + 1 cylindrical side)", () => {
    const r = 5, halfH = 10;
    const mesh = tessellateAnalyticBody(makeCylinderKernel(r, halfH), 0)!;
    expect(mesh).not.toBeNull();
    expect(mesh.positions.length).toBeGreaterThan(48 * 2 * 3 * 3); // band + two discs
    expect(mesh.normals.length).toBe(mesh.positions.length);
    for (let i = 0; i < mesh.positions.length; i += 3) {
      const x = mesh.positions[i], y = mesh.positions[i + 1], z = mesh.positions[i + 2];
      expect(Math.hypot(x, z)).toBeLessThanOrEqual(r + 1e-4); // within the cylinder
      expect(Math.abs(y)).toBeLessThanOrEqual(halfH + 1e-4); // within the height
    }
  });

  it("falls back (null) when a cylinder face lacks analytic params (old wasm)", () => {
    const k = makeCylinderKernel();
    const faces = k.bodyBrepFaces(0).map((f) =>
      f.surfaceKind === "cylinder" ? { ...f, surfaceParams: null } : f,
    );
    const stale: BrepMeshKernel = { ...k, bodyBrepFaces: () => faces };
    expect(tessellateAnalyticBody(stale, 0)).toBeNull();
  });
});

describe("collectAnalyticBodyMeshes", () => {
  it("returns the planar body and omits a curved one", () => {
    const planar = makeCubeKernel();
    expect(collectAnalyticBodyMeshes(planar)).toHaveLength(1);

    const curved = makeCubeKernel(1, { index: 0, kind: "sphere" });
    expect(collectAnalyticBodyMeshes(curved)).toHaveLength(0);
  });

  it("includes a cylinder body now that curved faces tessellate", () => {
    expect(collectAnalyticBodyMeshes(makeCylinderKernel())).toHaveLength(1);
  });
});
