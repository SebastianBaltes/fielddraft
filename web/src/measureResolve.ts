/**
 * Resolve a picked B-Rep element to a world-space measurement carrier
 * (`MeasureGeom`) — ticket me9w.
 *
 * The pure math in `measure.ts` never touches the kernel; this is the one
 * kernel-bound seam that turns a `PickedElement` (stable `idHash`) into the
 * normalized geometry the panel measures:
 *   - vertex → its world point (body-local → world via `bodyPlacement`);
 *   - edge   → its world-space sample polyline (`BRepEdge.points`, already
 *              world space);
 *   - face   → a world-space TRIANGLE SOUP plus the analytic area / centroid.
 *
 * Face triangles are built MESH-INDEPENDENTLY (the live main-thread kernel does
 * not mesh while the worker is up, so `bodyPositionsView` is empty): planar
 * faces are triangulated from their boundary edge loop (`triangulatePlanarFace`,
 * the same robust profile assembly the face highlight uses); analytic curved
 * faces (cylinder / sphere / cone / torus) are tessellated parametrically,
 * clipped to the boundary's parameter extent. The triangle soup doubles as the
 * area integrand (`triangleSoupArea`) and the distance geometry, so both numbers
 * come from one self-consistent surface sampling.
 */

import {
  localToWorld,
  type BRepEdge,
  type BRepFace,
  type BRepVertex,
  type Placement,
  type SurfaceParams,
} from "./kernel";
import { triangulatePlanarFace } from "./faceFill";
import { tessellateCylinderFace } from "./brepMesh";
import {
  triangleSoupArea,
  triangleSoupCentroid,
  type MeasureGeom,
  type Vec3,
} from "./measure";
import type { PickedElement } from "./elementQuery";

/** German label for each analytic surface kind (mirrors `ElementInfo`). */
const SURFACE_LABEL: Record<BRepFace["surfaceKind"], string> = {
  plane: "Ebene",
  cylinder: "Zylinder",
  sphere: "Kugel",
  cone: "Kegel",
  torus: "Torus",
  tessellated: "Tesselliert",
};

/** The slice of the kernel this resolver reads. */
export interface MeasureKernel {
  bodyCount(): number;
  bodyBrepFaces(i: number): BRepFace[];
  bodyBrepEdges(i: number): BRepEdge[];
  bodyBrepVertices(i: number): BRepVertex[];
  bodyPlacement(i: number): Placement;
}

type V3 = [number, number, number];

const cross = (a: V3, b: V3): V3 => [
  a[1] * b[2] - a[2] * b[1],
  a[2] * b[0] - a[0] * b[2],
  a[0] * b[1] - a[1] * b[0],
];
const norm = (a: V3): V3 => {
  const l = Math.hypot(a[0], a[1], a[2]);
  return l > 1e-12 ? [a[0] / l, a[1] / l, a[2] / l] : [0, 0, 1];
};
function rotateByPlacement(d: V3, pl: Placement): V3 {
  const [qx, qy, qz, qw] = pl.rotation;
  const tx = 2 * (qy * d[2] - qz * d[1]);
  const ty = 2 * (qz * d[0] - qx * d[2]);
  const tz = 2 * (qx * d[1] - qy * d[0]);
  return [
    d[0] + qw * tx + qy * tz - qz * ty,
    d[1] + qw * ty + qz * tx - qx * tz,
    d[2] + qw * tz + qx * ty - qy * tx,
  ];
}
/** Any unit vector orthogonal to `n` (seed = world axis least aligned). */
function orthonormal(n: V3): V3 {
  const ax = Math.abs(n[0]),
    ay = Math.abs(n[1]),
    az = Math.abs(n[2]);
  const seed: V3 = ax <= ay && ax <= az ? [1, 0, 0] : ay <= az ? [0, 1, 0] : [0, 0, 1];
  return norm(cross(seed, n));
}

const ANGULAR_SEGMENTS = 48;

/** Fold a body-local analytic surface param (origin/axis) to world space. */
function paramFrameWorld(
  params: SurfaceParams,
  pl: Placement,
): { originW: V3; axisW: V3 } {
  const originW = localToWorld(params.origin, pl);
  const axisW =
    "axis" in params ? rotateByPlacement(params.axis, pl) : [0, 1, 0];
  return { originW, axisW: norm(axisW as V3) };
}

/** Axial [min,max] extent + mean radius of a boundary loop about a world axis
 *  through `originW` (self-consistent with the edges, immune to placement
 *  scale). */
function axialExtent(
  boundary: number[][],
  originW: V3,
  axisW: V3,
): { tMin: number; tMax: number; radius: number } | null {
  let tMin = Infinity,
    tMax = -Infinity,
    rSum = 0,
    rN = 0;
  for (const pl of boundary) {
    for (let i = 0; i + 2 < pl.length; i += 3) {
      const px = pl[i] - originW[0],
        py = pl[i + 1] - originW[1],
        pz = pl[i + 2] - originW[2];
      const t = px * axisW[0] + py * axisW[1] + pz * axisW[2];
      if (t < tMin) tMin = t;
      if (t > tMax) tMax = t;
      const rx = px - t * axisW[0],
        ry = py - t * axisW[1],
        rz = pz - t * axisW[2];
      rSum += Math.hypot(rx, ry, rz);
      rN++;
    }
  }
  if (rN === 0) return null;
  return { tMin, tMax, radius: rSum / rN };
}

/** Two quads → six-vertex triangle soup for a parametric cell. */
function pushQuad(out: number[], p00: V3, p01: V3, p11: V3, p10: V3): void {
  out.push(...p00, ...p01, ...p11, ...p00, ...p11, ...p10);
}

/** Tessellate a sphere cap/band bounded by the boundary's latitude extent. */
function tessellateSphere(boundary: number[][], originW: V3, radius: number): number[] {
  // Latitude band: measure the boundary's angular spread along an arbitrary
  // axis through the centre; default to the full sphere when there is no clean
  // bound (a lone sphere has no edges at all).
  const out: number[] = [];
  const axis: V3 = [0, 1, 0];
  const u = orthonormal(axis);
  const v = cross(axis, u);
  let phiMin = -Math.PI / 2,
    phiMax = Math.PI / 2;
  if (boundary.length > 0) {
    let lo = Infinity,
      hi = -Infinity;
    for (const pl of boundary) {
      for (let i = 0; i + 2 < pl.length; i += 3) {
        const d: V3 = [pl[i] - originW[0], pl[i + 1] - originW[1], pl[i + 2] - originW[2]];
        const t = Math.max(-1, Math.min(1, (d[0] * axis[0] + d[1] * axis[1] + d[2] * axis[2]) / radius));
        const phi = Math.asin(t);
        if (phi < lo) lo = phi;
        if (phi > hi) hi = phi;
      }
    }
    if (hi > lo) {
      phiMin = lo;
      phiMax = hi;
    }
  }
  const LAT = 24;
  const pt = (phi: number, theta: number): V3 => {
    const cp = Math.cos(phi),
      sp = Math.sin(phi),
      ct = Math.cos(theta),
      st = Math.sin(theta);
    return [
      originW[0] + radius * (sp * axis[0] + cp * (ct * u[0] + st * v[0])),
      originW[1] + radius * (sp * axis[1] + cp * (ct * u[1] + st * v[1])),
      originW[2] + radius * (sp * axis[2] + cp * (ct * u[2] + st * v[2])),
    ];
  };
  for (let i = 0; i < LAT; i++) {
    const ph0 = phiMin + ((phiMax - phiMin) * i) / LAT;
    const ph1 = phiMin + ((phiMax - phiMin) * (i + 1)) / LAT;
    for (let j = 0; j < ANGULAR_SEGMENTS; j++) {
      const th0 = (2 * Math.PI * j) / ANGULAR_SEGMENTS;
      const th1 = (2 * Math.PI * (j + 1)) / ANGULAR_SEGMENTS;
      pushQuad(out, pt(ph0, th0), pt(ph1, th0), pt(ph1, th1), pt(ph0, th1));
    }
  }
  return out;
}

/** Tessellate a cone band bounded by the boundary's axial extent. */
function tessellateCone(
  boundary: number[][],
  originW: V3,
  axisW: V3,
  halfAngle: number,
): number[] {
  const ext = axialExtent(boundary, originW, axisW);
  if (!ext || !(ext.tMax > ext.tMin)) return [];
  const u = orthonormal(axisW);
  const v = cross(axisW, u);
  const tan = Math.tan(halfAngle);
  const pt = (t: number, a: number): V3 => {
    const r = Math.max(0, t * tan);
    const ca = Math.cos(a),
      sa = Math.sin(a);
    return [
      originW[0] + axisW[0] * t + r * (ca * u[0] + sa * v[0]),
      originW[1] + axisW[1] * t + r * (ca * u[1] + sa * v[1]),
      originW[2] + axisW[2] * t + r * (ca * u[2] + sa * v[2]),
    ];
  };
  const out: number[] = [];
  for (let j = 0; j < ANGULAR_SEGMENTS; j++) {
    const a0 = (2 * Math.PI * j) / ANGULAR_SEGMENTS;
    const a1 = (2 * Math.PI * (j + 1)) / ANGULAR_SEGMENTS;
    pushQuad(out, pt(ext.tMin, a0), pt(ext.tMax, a0), pt(ext.tMax, a1), pt(ext.tMin, a1));
  }
  return out;
}

/** Tessellate a full torus (analytic major/minor radius). */
function tessellateTorus(
  originW: V3,
  axisW: V3,
  majorRadius: number,
  minorRadius: number,
): number[] {
  const u = orthonormal(axisW);
  const v = cross(axisW, u);
  const pt = (theta: number, phi: number): V3 => {
    const ct = Math.cos(theta),
      st = Math.sin(theta);
    const ringCenter: V3 = [
      originW[0] + majorRadius * (ct * u[0] + st * v[0]),
      originW[1] + majorRadius * (ct * u[1] + st * v[1]),
      originW[2] + majorRadius * (ct * u[2] + st * v[2]),
    ];
    const radial: V3 = [ct * u[0] + st * v[0], ct * u[1] + st * v[1], ct * u[2] + st * v[2]];
    const cp = Math.cos(phi),
      sp = Math.sin(phi);
    return [
      ringCenter[0] + minorRadius * (cp * radial[0] + sp * axisW[0]),
      ringCenter[1] + minorRadius * (cp * radial[1] + sp * axisW[1]),
      ringCenter[2] + minorRadius * (cp * radial[2] + sp * axisW[2]),
    ];
  };
  const out: number[] = [];
  for (let i = 0; i < ANGULAR_SEGMENTS; i++) {
    const t0 = (2 * Math.PI * i) / ANGULAR_SEGMENTS;
    const t1 = (2 * Math.PI * (i + 1)) / ANGULAR_SEGMENTS;
    for (let j = 0; j < ANGULAR_SEGMENTS; j++) {
      const p0 = (2 * Math.PI * j) / ANGULAR_SEGMENTS;
      const p1 = (2 * Math.PI * (j + 1)) / ANGULAR_SEGMENTS;
      pushQuad(out, pt(t0, p0), pt(t0, p1), pt(t1, p1), pt(t1, p0));
    }
  }
  return out;
}

/** Best-effort triangle soup for a face whose surface we cannot tessellate
 *  analytically (tessellated/gyroid faces, or an analytic face missing its
 *  params): triangulate the boundary loop as a planar projection through the
 *  face centroid/normal. Area is then approximate, but distance probing still
 *  has geometry to work with. */
function fallbackPlanarSoup(boundary: number[][], originW: V3, normalW: V3): number[] {
  const tri = triangulatePlanarFace(boundary, originW, normalW);
  return tri;
}

/** Build a face's world-space triangle soup from its analytic kind + boundary. */
function faceTriangleSoup(
  face: BRepFace,
  boundary: number[][],
  pl: Placement,
): number[] {
  const originW = localToWorld(face.centroidLocal, pl);
  const normalW = norm(rotateByPlacement(face.normalLocal, pl));
  const params = face.surfaceParams ?? null;

  if (face.surfaceKind === "plane") {
    return triangulatePlanarFace(boundary, originW, normalW);
  }
  if (face.surfaceKind === "cylinder" && params?.kind === "cylinder") {
    const { originW: oW, axisW } = paramFrameWorld(params, pl);
    const tess = tessellateCylinderFace(boundary, oW, axisW);
    return tess ? tess.positions : fallbackPlanarSoup(boundary, originW, normalW);
  }
  if (face.surfaceKind === "sphere" && params?.kind === "sphere") {
    const oW = localToWorld(params.origin, pl);
    const radius = params.radius * (pl.scale[0] || 1);
    return tessellateSphere(boundary, oW, radius);
  }
  if (face.surfaceKind === "cone" && params?.kind === "cone") {
    const { originW: oW, axisW } = paramFrameWorld(params, pl);
    return tessellateCone(boundary, oW, axisW, params.halfAngle);
  }
  if (face.surfaceKind === "torus" && params?.kind === "torus") {
    const { originW: oW, axisW } = paramFrameWorld(params, pl);
    const s = pl.scale[0] || 1;
    return tessellateTorus(oW, axisW, params.majorRadius * s, params.minorRadius * s);
  }
  return fallbackPlanarSoup(boundary, originW, normalW);
}

/**
 * Resolve a picked element to its measurement geometry, or `null` when no body
 * carries the `idHash` (the element was edited away). Scans every body's B-Rep.
 */
export function resolveMeasureGeom(
  kernel: MeasureKernel,
  el: PickedElement,
): MeasureGeom | null {
  const count = kernel.bodyCount();

  if (el.kind === "vertex") {
    for (let i = 0; i < count; i++) {
      let verts: BRepVertex[];
      let pl: Placement;
      try {
        verts = kernel.bodyBrepVertices(i);
        pl = kernel.bodyPlacement(i);
      } catch {
        continue;
      }
      const v = verts.find((vx) => vx.idHash === el.idHash);
      if (!v) continue;
      const p = localToWorld(v.positionLocal, pl) as Vec3;
      return { kind: "vertex", point: p };
    }
    return null;
  }

  if (el.kind === "edge") {
    for (let i = 0; i < count; i++) {
      let edges: BRepEdge[];
      try {
        edges = kernel.bodyBrepEdges(i);
      } catch {
        continue;
      }
      const e = edges.find((x) => x.idHash === el.idHash);
      if (!e || e.points.length < 6) continue;
      // `points` are already world space — consume raw.
      return { kind: "edge", points: e.points };
    }
    return null;
  }

  // face
  for (let i = 0; i < count; i++) {
    let faces: BRepFace[];
    let edges: BRepEdge[];
    let pl: Placement;
    try {
      faces = kernel.bodyBrepFaces(i);
      edges = kernel.bodyBrepEdges(i);
      pl = kernel.bodyPlacement(i);
    } catch {
      continue;
    }
    const face = faces.find((f) => f.idHash === el.idHash);
    if (!face) continue;
    const boundary = edges
      .filter((edge) => edge.faces.includes(face.faceIndex))
      .map((edge) => edge.points);
    const triangles = faceTriangleSoup(face, boundary, pl);
    const area = triangleSoupArea(triangles);
    const centroid =
      triangles.length >= 9
        ? triangleSoupCentroid(triangles)
        : (localToWorld(face.centroidLocal, pl) as Vec3);
    return {
      kind: "face",
      triangles,
      area,
      centroid,
      surfaceLabel: SURFACE_LABEL[face.surfaceKind] ?? face.surfaceKind,
    };
  }
  return null;
}
