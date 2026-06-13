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

/** Body-local analytic surface parameters (only the kinds this module
 *  tessellates so far). The axis is the primitive's local +Y. */
export interface CylinderSurfaceParams {
  kind: "cylinder";
  origin: [number, number, number];
  axis: [number, number, number];
  radius: number;
}

export interface BrepMeshFace {
  faceIndex: number;
  surfaceKind: string;
  centroidLocal: [number, number, number];
  normalLocal: [number, number, number];
  /** Present for curved analytic faces (cylinder/sphere/…); null/absent for
   *  planar and tessellated faces. */
  surfaceParams?: { kind: string } | null;
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

type V3 = [number, number, number];

const cross = (a: V3, b: V3): V3 => [
  a[1] * b[2] - a[2] * b[1],
  a[2] * b[0] - a[0] * b[2],
  a[0] * b[1] - a[1] * b[0],
];
const normalize = (v: V3): V3 => {
  const len = Math.hypot(v[0], v[1], v[2]);
  return len > 1e-12 ? [v[0] / len, v[1] / len, v[2] / len] : [0, 0, 1];
};
/** Any unit vector orthogonal to `n`: cross `n` with the world axis least
 *  aligned with it (the same seed rule as `faceFill`/the kernel). */
const orthonormal = (n: V3): V3 => {
  const ax = Math.abs(n[0]), ay = Math.abs(n[1]), az = Math.abs(n[2]);
  const seed: V3 = ax <= ay && ax <= az ? [1, 0, 0] : ay <= az ? [0, 1, 0] : [0, 0, 1];
  return normalize(cross(seed, n));
};

/** Angular tessellation of a cylinder band (Phase-2 LOD is later — fixed for now). */
const CYLINDER_SEGMENTS = 48;

/**
 * Tessellate a cylindrical face as a band around its axis. Phase 2 targets the
 * common FULL-360° band (a lone cylinder's side, a through-hole's wall): the
 * axial extent + radius are read from the boundary cap circles (self-consistent
 * with the edges, immune to placement scale); the axis/origin come from the
 * kernel's analytic `surfaceParams` (folded to world by the caller). Normals are
 * radial; outward-vs-inward (hole) is left to the renderer's double-sided
 * shading (the symmetric band gives no reliable sign on its own). Returns `null`
 * for a degenerate band.
 */
export function tessellateCylinderFace(
  boundary: number[][],
  originW: V3,
  axisW: V3,
): { positions: number[]; normals: number[] } | null {
  const ax = normalize(axisW);
  const u = orthonormal(ax);
  const v = cross(ax, u);

  let tMin = Infinity, tMax = -Infinity;
  let radiusSum = 0, radiusN = 0;
  for (const pl of boundary) {
    for (let i = 0; i + 2 < pl.length; i += 3) {
      const px = pl[i] - originW[0], py = pl[i + 1] - originW[1], pz = pl[i + 2] - originW[2];
      const t = px * ax[0] + py * ax[1] + pz * ax[2];
      if (t < tMin) tMin = t;
      if (t > tMax) tMax = t;
      const rx = px - t * ax[0], ry = py - t * ax[1], rz = pz - t * ax[2];
      radiusSum += Math.hypot(rx, ry, rz);
      radiusN++;
    }
  }
  if (radiusN === 0 || !(tMax > tMin)) return null;
  const radius = radiusSum / radiusN;
  if (radius < 1e-6) return null;

  const ringPt = (t: number, a: number): V3 => {
    const ca = Math.cos(a), sa = Math.sin(a);
    return [
      originW[0] + ax[0] * t + radius * (ca * u[0] + sa * v[0]),
      originW[1] + ax[1] * t + radius * (ca * u[1] + sa * v[1]),
      originW[2] + ax[2] * t + radius * (ca * u[2] + sa * v[2]),
    ];
  };
  const ringN = (a: number): V3 => {
    const ca = Math.cos(a), sa = Math.sin(a);
    return [ca * u[0] + sa * v[0], ca * u[1] + sa * v[1], ca * u[2] + sa * v[2]];
  };

  const positions: number[] = [];
  const normals: number[] = [];
  for (let i = 0; i < CYLINDER_SEGMENTS; i++) {
    const a0 = (2 * Math.PI * i) / CYLINDER_SEGMENTS;
    const a1 = (2 * Math.PI * (i + 1)) / CYLINDER_SEGMENTS;
    const p00 = ringPt(tMin, a0), p01 = ringPt(tMax, a0);
    const p10 = ringPt(tMin, a1), p11 = ringPt(tMax, a1);
    const n0 = ringN(a0), n1 = ringN(a1);
    positions.push(...p00, ...p01, ...p11, ...p00, ...p11, ...p10);
    normals.push(...n0, ...n0, ...n1, ...n0, ...n1, ...n1);
  }
  return { positions, normals };
}

/**
 * Tessellate `bodyIndex` if its whole surface is analytic and supported
 * (PLANAR faces + CYLINDER faces in Phase 2), else `null`. Returns `null` (→
 * caller raymarches) when: no faces yet (mid-regen), ANY face is an unsupported
 * kind, a face has no bounding edges, or a face fails to assemble — i.e. only a
 * fully, cleanly meshable body is taken.
 */
export function tessellateAnalyticBody(
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
  // Gate: every face must be a kind we can tessellate exactly. A cylinder face
  // needs its analytic params (a wasm too old to export them → bail to raymarch).
  const supported = (f: BrepMeshFace) =>
    f.surfaceKind === "plane" ||
    (f.surfaceKind === "cylinder" && f.surfaceParams?.kind === "cylinder");
  if (!faces.every(supported)) return null;

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
    const normalW = localNormalToWorld(f.normalLocal, placement);

    if (f.surfaceKind === "plane") {
      const originW = localPointToWorld(f.centroidLocal, placement);
      const tri = triangulatePlanarFace(boundary, originW, normalW);
      if (tri.length === 0) return null;
      for (const c of tri) positions.push(c);
      for (let v = 0; v < tri.length / 3; v++) {
        normals.push(normalW[0], normalW[1], normalW[2]);
      }
    } else {
      // cylinder (params validated by the gate above)
      const sp = f.surfaceParams as unknown as CylinderSurfaceParams;
      const originW = localPointToWorld(sp.origin, placement);
      const axisW = localNormalToWorld(sp.axis, placement);
      const band = tessellateCylinderFace(boundary, originW, axisW);
      if (!band) return null;
      for (const c of band.positions) positions.push(c);
      for (const c of band.normals) normals.push(c);
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
 * Tessellate every supported analytic body in the scene. Bodies with curved
 * (beyond cylinder) or implicit faces are omitted — the caller renders those
 * via the SDF path (their analytic wireframe edges stay visible regardless).
 */
export function collectAnalyticBodyMeshes(kernel: BrepMeshKernel): BodyMesh[] {
  const out: BodyMesh[] = [];
  let count: number;
  try {
    count = kernel.bodyCount();
  } catch {
    return out;
  }
  for (let i = 0; i < count; i++) {
    const mesh = tessellateAnalyticBody(kernel, i);
    if (mesh) out.push(mesh);
  }
  return out;
}
