/**
 * Resolve a sketch plane query to its world-space frame on the WEB side
 * (Phase 2 of the Sketch-in-3D rework, docs/SKETCH-IN-3D-PLAN.md, tickets
 * yx6a/bxoo). The kernel resolves the same queries during regeneration
 * (`resolve_plane_frame`); this mirror exists so the viewport can place the
 * session's 3D sketch layer and intersect input rays WITHOUT a kernel
 * round-trip per frame.
 *
 * CONVENTION LOCK: the in-plane basis derivation must match the kernel's
 * `face_plane_frame` (sketch_features.rs) exactly — world axis least aligned
 * with the normal as seed, crossed twice — otherwise drawn (u,v) coordinates
 * would land rotated/mirrored after commit. The datum frames mirror
 * `datum_plane_frame`.
 */

import type { BRepFace, Placement } from "./kernel";
import { localToWorld } from "./kernel";

export interface PlaneFrame {
  origin: [number, number, number];
  xAxis: [number, number, number];
  yAxis: [number, number, number];
  normal: [number, number, number];
}

type Vec3 = [number, number, number];

const cross = (a: Vec3, b: Vec3): Vec3 => [
  a[1] * b[2] - a[2] * b[1],
  a[2] * b[0] - a[0] * b[2],
  a[0] * b[1] - a[1] * b[0],
];

const normalize = (v: Vec3): Vec3 => {
  const len = Math.hypot(v[0], v[1], v[2]);
  return len > 1e-12 ? [v[0] / len, v[1] / len, v[2] / len] : [0, 0, 1];
};

/** Rotate a body-local direction into world space (placement rotation only). */
function rotateByPlacement(v: Vec3, pl: Placement): Vec3 {
  const [qx, qy, qz, qw] = pl.rotation;
  const tx = 2 * (qy * v[2] - qz * v[1]);
  const ty = 2 * (qz * v[0] - qx * v[2]);
  const tz = 2 * (qx * v[1] - qy * v[0]);
  return [
    v[0] + qw * tx + qy * tz - qz * ty,
    v[1] + qw * ty + qz * tx - qx * tz,
    v[2] + qw * tz + qx * ty - qy * tx,
  ];
}

/** In-plane basis from a unit normal — the kernel's seed rule, verbatim:
 *  pick the world axis least aligned with the normal, cross twice. */
export function planeBasisFromNormal(normal: Vec3): { xAxis: Vec3; yAxis: Vec3 } {
  const n = normalize(normal);
  const ax = Math.abs(n[0]);
  const ay = Math.abs(n[1]);
  const az = Math.abs(n[2]);
  const seed: Vec3 =
    ax <= ay && ax <= az ? [1, 0, 0] : ay <= az ? [0, 1, 0] : [0, 0, 1];
  const xAxis = normalize(cross(seed, n));
  const yAxis = normalize(cross(n, xAxis));
  return { xAxis, yAxis };
}

/** The three origin datum frames (mirrors `datum_plane_frame`). */
const DATUM_FRAMES: Record<string, PlaneFrame> = {
  origin_plane_xy: {
    origin: [0, 0, 0],
    xAxis: [1, 0, 0],
    yAxis: [0, 1, 0],
    normal: [0, 0, 1],
  },
  origin_plane_xz: {
    origin: [0, 0, 0],
    xAxis: [1, 0, 0],
    yAxis: [0, 0, 1],
    normal: [0, 1, 0],
  },
  origin_plane_yz: {
    origin: [0, 0, 0],
    xAxis: [0, 1, 0],
    yAxis: [0, 0, 1],
    normal: [1, 0, 0],
  },
};

/** Frame from a CSYS given as origin + Z + X — mirrors the kernel's
 *  `SketchFrame::from_csys`, including its Gram-Schmidt re-orthogonalisation
 *  of X against the normal and the `y = n × x` right-handed closure. */
export function frameFromCsys(origin: Vec3, normal: Vec3, xAxis: Vec3): PlaneFrame {
  const n = normalize(normal);
  const dot = xAxis[0] * n[0] + xAxis[1] * n[1] + xAxis[2] * n[2];
  const rejected: Vec3 = [
    xAxis[0] - dot * n[0],
    xAxis[1] - dot * n[1],
    xAxis[2] - dot * n[2],
  ];
  const x =
    Math.hypot(rejected[0], rejected[1], rejected[2]) > 1e-6
      ? normalize(rejected)
      : planeBasisFromNormal(n).xAxis;
  return { origin, xAxis: x, yAxis: cross(n, x), normal: n };
}

/** Minimal kernel surface this resolver needs (subset of `Kernel`). */
export interface PlaneFrameKernel {
  bodyCount(): number;
  bodyBrepFaces(i: number): BRepFace[];
  bodyPlacement(i: number): Placement;
  /** Kernel-side query resolution — the authority for every query form
   *  beyond the two regex fast paths below. Optional so the existing
   *  test doubles stay valid. */
  resolveQuery?(query: string): { kind: string; idHash: string }[];
  /** Placed mate connectors, whose CSYS is a plane frame in its own right
   *  (#409). Optional for the same reason as `resolveQuery`. */
  resolvedMateConnectors?(): {
    id: number;
    origin: [number, number, number];
    normal: [number, number, number];
    x_axis: [number, number, number];
  }[];
}

/**
 * Resolve `planeQuery` to a world frame, or `null` when the query matches
 * no datum plane, no mate connector and no face of the current bodies.
 * Face matching mirrors the kernel: `qFace(hash)` by stable id,
 * `qCreatedBy(id)` / `qFaceOf(id)` by owning feature (first hit in body
 * order).
 */
export function resolvePlaneFrame(
  planeQuery: string,
  kernel: PlaneFrameKernel,
): PlaneFrame | null {
  const datum = planeQuery.match(/^qCreatedBy\((origin_plane_(?:xy|xz|yz))\)$/);
  if (datum) return DATUM_FRAMES[datum[1]];

  let byHash = planeQuery.match(/^qFace\((\d+)\)$/);
  const byFeature = planeQuery.match(/^q(?:CreatedBy|FaceOf)\((\d+)\)$/);
  // A placed mate connector's CSYS is a plane frame (#409). This is checked
  // BEFORE the face lookup, in the kernel's order and for the kernel's
  // reason: a connector is usually anchored to a face, so falling through
  // would silently yield that face's centroid and synthesised basis instead
  // of the connector's own origin and rotation.
  if (byFeature) {
    const id = Number(byFeature[1]);
    const mc = kernel.resolvedMateConnectors?.().find((c) => c.id === id);
    if (mc) return frameFromCsys(mc.origin, mc.normal, mc.x_axis);
  }
  if (!byHash && !byFeature) {
    // Any richer form (`qCreatedBy(op, ROLE)`, `qWall`, `qAdjacentTo`,
    // fingerprint-carrying `qFace(h @ fp)` …) is resolved by the KERNEL —
    // the single authority — and comes back as a concrete face id_hash we
    // then look up below. Keeping the two fast paths above avoids changing
    // their established first-hit semantics.
    const resolved = kernel.resolveQuery?.(planeQuery) ?? [];
    const face = resolved.find((r) => r.kind === "face");
    if (!face) return null;
    byHash = [planeQuery, face.idHash] as unknown as RegExpMatchArray;
  }

  let count = 0;
  try {
    count = kernel.bodyCount();
  } catch {
    return null;
  }
  for (let i = 0; i < count; i++) {
    let faces: BRepFace[];
    let placement: Placement;
    try {
      faces = kernel.bodyBrepFaces(i);
      placement = kernel.bodyPlacement(i);
    } catch {
      continue;
    }
    const face = byHash
      ? faces.find((f) => f.idHash === byHash[1])
      : faces.find((f) => f.featureId === Number(byFeature![1]));
    if (!face) continue;
    const origin = localToWorld(face.centroidLocal, placement);
    const normal = normalize(rotateByPlacement(face.normalLocal, placement));
    const { xAxis, yAxis } = planeBasisFromNormal(normal);
    return { origin, xAxis, yAxis, normal };
  }
  return null;
}

/** The world-space PLANE a face lies in: its unit normal and the signed
 *  offset `d = n · p` of any of its points. Everything a caller needs to ask
 *  whether two faces span the SAME plane — which is not the same question as
 *  whether they are the same face (Onshape merges coplanar cap faces into one,
 *  we do not; see the Onshape importer's plane gate).
 *
 *  One pass over the bodies for the whole list, so asking about four faces
 *  costs one scan and not four. Faces the current bodies do not carry are
 *  absent from the result — the caller sees the shortfall in the map size. */
export function faceWorldPlanes(
  idHashes: readonly string[],
  kernel: PlaneFrameKernel,
): Map<string, { normal: Vec3; offset: number }> {
  const out = new Map<string, { normal: Vec3; offset: number }>();
  const want = new Set(idHashes);
  if (want.size === 0) return out;
  let count = 0;
  try {
    count = kernel.bodyCount();
  } catch {
    return out;
  }
  for (let i = 0; i < count && out.size < want.size; i++) {
    let faces: BRepFace[];
    let placement: Placement;
    try {
      faces = kernel.bodyBrepFaces(i);
      placement = kernel.bodyPlacement(i);
    } catch {
      continue;
    }
    for (const face of faces) {
      if (!want.has(face.idHash) || out.has(face.idHash)) continue;
      const normal = normalize(rotateByPlacement(face.normalLocal, placement));
      const p = localToWorld(face.centroidLocal, placement);
      out.set(face.idHash, {
        normal,
        offset: normal[0] * p[0] + normal[1] * p[1] + normal[2] * p[2],
      });
    }
  }
  return out;
}

/** Lift sketch-plane (u, v) coordinates into world space. */
export function planeToWorld(
  u: number,
  v: number,
  frame: PlaneFrame,
): [number, number, number] {
  return [
    frame.origin[0] + u * frame.xAxis[0] + v * frame.yAxis[0],
    frame.origin[1] + u * frame.xAxis[1] + v * frame.yAxis[1],
    frame.origin[2] + u * frame.xAxis[2] + v * frame.yAxis[2],
  ];
}

/**
 * Intersect a world-space ray with the frame's plane and return the hit as
 * sketch (u, v), or `null` for a ray (near-)parallel to the plane or hitting
 * behind the origin. This is the Phase-3 input mapping (mouse → sketch
 * coordinates with the normal 3D camera).
 */
export function rayToPlaneUv(
  rayOrigin: Vec3,
  rayDir: Vec3,
  frame: PlaneFrame,
): [number, number] | null {
  const n = frame.normal;
  const denom = rayDir[0] * n[0] + rayDir[1] * n[1] + rayDir[2] * n[2];
  if (Math.abs(denom) < 1e-9) return null;
  const diff: Vec3 = [
    frame.origin[0] - rayOrigin[0],
    frame.origin[1] - rayOrigin[1],
    frame.origin[2] - rayOrigin[2],
  ];
  const t = (diff[0] * n[0] + diff[1] * n[1] + diff[2] * n[2]) / denom;
  if (t < 0) return null;
  const p: Vec3 = [
    rayOrigin[0] + t * rayDir[0],
    rayOrigin[1] + t * rayDir[1],
    rayOrigin[2] + t * rayDir[2],
  ];
  const d: Vec3 = [p[0] - frame.origin[0], p[1] - frame.origin[1], p[2] - frame.origin[2]];
  return [
    d[0] * frame.xAxis[0] + d[1] * frame.xAxis[1] + d[2] * frame.xAxis[2],
    d[0] * frame.yAxis[0] + d[1] * frame.yAxis[1] + d[2] * frame.yAxis[2],
  ];
}
