/**
 * Shared resolver for a selected B-Rep face's world-space geometry.
 *
 * Both the Element-info readout (ElementInfo.tsx) and the viewport context
 * menu ("View Normal to Face") need a selected face's world-space centroid
 * and outward normal. This is the single place that scans the kernel's bodies
 * for the matching `idHash` and folds the body-local centroid/normal through
 * the body placement into world space — keeping that mapping in exactly one
 * testable spot.
 */

import {
  localToWorld,
  type BRepFace,
  type Kernel,
  type Placement,
} from "./kernel";

/** A face's world-space geometry plus its analytic surface kind. */
export interface FaceWorldGeometry {
  surfaceKind: BRepFace["surfaceKind"];
  centroid: [number, number, number];
  /** Outward unit normal in world space. */
  normal: [number, number, number];
}

/** Rotate a body-local direction by a placement's quaternion (no translation,
 *  no scale) to get its world-space orientation. */
export function rotateByPlacement(
  d: [number, number, number],
  pl: Placement,
): [number, number, number] {
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

/**
 * Resolve the world-space centroid + normal of the face with `idHash` by
 * scanning every body in the kernel. Returns `null` when no body carries a
 * face with that id (e.g. the selection went stale after a regen).
 */
export function resolveFaceWorldGeometry(
  kernel: Pick<Kernel, "bodyCount" | "bodyBrepFaces" | "bodyPlacement">,
  idHash: string,
): FaceWorldGeometry | null {
  const count = kernel.bodyCount();
  for (let i = 0; i < count; i++) {
    let faces: BRepFace[];
    let placement: Placement;
    try {
      faces = kernel.bodyBrepFaces(i);
      placement = kernel.bodyPlacement(i);
    } catch {
      continue;
    }
    const face = faces.find((f) => f.idHash === idHash);
    if (!face) continue;
    return {
      surfaceKind: face.surfaceKind,
      centroid: localToWorld(face.centroidLocal, placement),
      normal: rotateByPlacement(face.normalLocal, placement),
    };
  }
  return null;
}
