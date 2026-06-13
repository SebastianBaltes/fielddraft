/**
 * Hybrid-Renderer, Phase 1 (siehe docs/ADAPTIVE-RENDERING-PLAN.md, Achse 2):
 * tessellate a body whose entire analytic B-Rep surface is PLANAR — boxes,
 * prisms, planar boolean results — into an exact raster mesh. Sharp edges, exact
 * per-face normals, ZERO raymarching. Bodies with any curved or implicit face
 * (cylinder/sphere/cone/torus/`tessellated`) return `null`, so the caller falls
 * back to the SDF raymarcher for those.
 *
 * DOM-free and kernel-binding-free (the placement transforms are inlined) so the
 * geometry can be unit-tested without a GPU or the wasm kernel. Per planar face
 * it gathers the bounding edges (`edge.faces` carries the face arena indices),
 * lifts the centroid + normal to world space, and reuses the robust planar
 * triangulator in `faceFill.ts` — the same path the face-highlight already uses,
 * generalised from "the selected face" to "every face of an all-planar body".
 */

import { triangulatePlanarFace } from "./faceFill";

export interface BrepMeshPlacement {
  translate: [number, number, number];
  /** Unit quaternion (x, y, z, w). */
  rotation: [number, number, number, number];
  scale: [number, number, number];
}

export interface BrepMeshFace {
  faceIndex: number;
  surfaceKind: string;
  centroidLocal: [number, number, number];
  normalLocal: [number, number, number];
}

export interface BrepMeshEdge {
  /** Dense world-space polyline, flat `[x,y,z, …]` (== `BRepEdge.points`). */
  points: number[];
  /** Arena indices of the faces this edge bounds (== `BRepFace.faceIndex`). */
  faces: number[];
}

/** The subset of the kernel this module reads. */
export interface BrepMeshKernel {
  bodyCount(): number;
  bodyPlacement(i: number): BrepMeshPlacement;
  bodyBrepFaces(i: number): BrepMeshFace[];
  bodyBrepEdges(i: number): BrepMeshEdge[];
}

export interface BodyMesh {
  bodyIndex: number;
  /** Flat world-space triangle soup `[x,y,z, …]`. */
  positions: Float32Array;
  /** One unit normal per position (flat-shaded per face). */
  normals: Float32Array;
}

/** Body-local point → world: scale, then quaternion-rotate, then translate. */
function localPointToWorld(
  p: [number, number, number],
  pl: BrepMeshPlacement,
): [number, number, number] {
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

/** Body-local direction (normal) → world: quaternion rotation only, normalised.
 *  (Non-uniform scale would need the inverse-transpose; Phase 1 targets the
 *  unit-scale primitive/boolean bodies where rotation suffices.) */
function localNormalToWorld(
  d: [number, number, number],
  pl: BrepMeshPlacement,
): [number, number, number] {
  const [qx, qy, qz, qw] = pl.rotation;
  const tx = 2 * (qy * d[2] - qz * d[1]);
  const ty = 2 * (qz * d[0] - qx * d[2]);
  const tz = 2 * (qx * d[1] - qy * d[0]);
  const r: [number, number, number] = [
    d[0] + qw * tx + qy * tz - qz * ty,
    d[1] + qw * ty + qz * tx - qx * tz,
    d[2] + qw * tz + qx * ty - qy * tx,
  ];
  const len = Math.hypot(r[0], r[1], r[2]);
  return len > 1e-12 ? [r[0] / len, r[1] / len, r[2] / len] : r;
}

/**
 * Tessellate `bodyIndex` if its whole surface is analytic-planar, else `null`.
 * Returns `null` (→ caller raymarches) when: no faces yet (mid-regen), ANY face
 * is non-planar, a planar face has no bounding edges, or a face fails to
 * assemble a clean profile — i.e. only a fully, cleanly meshable body is taken.
 */
export function tessellatePlanarBody(
  kernel: BrepMeshKernel,
  bodyIndex: number,
): BodyMesh | null {
  let faces: BrepMeshFace[];
  try {
    faces = kernel.bodyBrepFaces(bodyIndex);
  } catch {
    return null;
  }
  if (faces.length === 0) return null;
  // The Phase-1 gate: a single non-planar face disqualifies the whole body.
  if (!faces.every((f) => f.surfaceKind === "plane")) return null;

  let edges: BrepMeshEdge[];
  let placement: BrepMeshPlacement;
  try {
    edges = kernel.bodyBrepEdges(bodyIndex);
    placement = kernel.bodyPlacement(bodyIndex);
  } catch {
    return null;
  }

  const positions: number[] = [];
  const normals: number[] = [];
  for (const f of faces) {
    const boundary = edges
      .filter((e) => e.faces.includes(f.faceIndex))
      .map((e) => e.points);
    if (boundary.length === 0) return null;
    const originW = localPointToWorld(f.centroidLocal, placement);
    const normalW = localNormalToWorld(f.normalLocal, placement);
    const tri = triangulatePlanarFace(boundary, originW, normalW);
    if (tri.length === 0) return null;
    for (const c of tri) positions.push(c);
    const vertCount = tri.length / 3;
    for (let v = 0; v < vertCount; v++) {
      normals.push(normalW[0], normalW[1], normalW[2]);
    }
  }
  if (positions.length === 0) return null;
  return {
    bodyIndex,
    positions: new Float32Array(positions),
    normals: new Float32Array(normals),
  };
}

/**
 * Tessellate every planar-analytic body in the scene. Bodies with curved or
 * implicit faces are simply omitted — the caller renders those via the SDF path
 * (and their analytic wireframe edges stay visible regardless).
 */
export function collectPlanarBodyMeshes(kernel: BrepMeshKernel): BodyMesh[] {
  const out: BodyMesh[] = [];
  let count: number;
  try {
    count = kernel.bodyCount();
  } catch {
    return out;
  }
  for (let i = 0; i < count; i++) {
    const mesh = tessellatePlanarBody(kernel, i);
    if (mesh) out.push(mesh);
  }
  return out;
}
