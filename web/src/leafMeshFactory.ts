/**
 * Leaf-primitive mesh factory — builds the exact parametric three.js
 * geometry for one hybrid CSG leaf (box/sphere/…/extrude/revolve/helix/
 * sweep) plus its local→world placement.
 *
 * Pure three.js + kernel-data logic, shared by every consumer that needs a
 * leaf hull mesh: the deferred SDF pipeline (raster proxies), the classic
 * WebGL body path, the screen-space CSG compositor and the B-Rep selection
 * highlights.
 */

import * as THREE from "three";
import { buildHelixGeometry, decodeHelixParams } from "./helixMesh";
import { buildSweepGeometry } from "./sweepMesh";
import { ensureOutwardOrientation } from "./sweptShell";
import {
  subdivideAlongAxis,
  applyWarp,
  warpAxis,
  maxStepForWarp,
} from "./warpMesh";
import type { HybridLeafInfo } from "./kernel";

/** A cheap fingerprint of everything {@link createPrimitiveGeometry} bakes into
 *  a leaf's mesh — its kind, shape params (box size, extrude depth, …) and
 *  contour (extrude/revolve profile). Used by `updateDeferredData` to
 *  rebuild a stale proxy mesh when the GEOMETRY changed (e.g. an extrude depth
 *  edit, ticket c9ic) while skipping pure-transform changes (translate). */
export function leafGeoKey(leaf: HybridLeafInfo): string {
  let key = `${leaf.kind}|${leaf.params.join(",")}`;
  const c = leaf.contour;
  if (c) {
    let sum = 0;
    const fold = (pts: [number, number][]) => {
      for (const [x, y] of pts)
        sum =
          (Math.imul(sum, 31) + Math.round(x * 1e3) + Math.round(y * 1e4)) | 0;
    };
    fold(c.outer);
    for (const h of c.holes) fold(h);
    key += `|${c.outer.length}:${c.holes.length}:${sum}`;
  }
  return key;
}

export function createPrimitiveGeometry(
  leaf: HybridLeafInfo,
): THREE.BufferGeometry | null {
  const p = leaf.params;
  switch (leaf.kind) {
    case "box":
      return new THREE.BoxGeometry(p[0] * 2, p[1] * 2, p[2] * 2);
    case "sphere":
      return new THREE.SphereGeometry(p[0], 32, 24);
    case "cylinder":
      return new THREE.CylinderGeometry(p[0], p[0], p[1] * 2, 32);
    case "torus": {
      const tgeo = new THREE.TorusGeometry(p[0], p[1], 24, 48);
      tgeo.rotateX(Math.PI / 2);
      return tgeo;
    }
    case "cone":
      return new THREE.CylinderGeometry(p[1], p[0], p[2] * 2, 32);
    case "capsule":
      return new THREE.CapsuleGeometry(p[0], p[1] * 2, 16, 24);
    case "extrude":
      return createExtrudeGeometry(leaf);
    case "revolve":
      return createRevolveGeometry(leaf);
    case "helix":
      return createHelixGeometry(leaf);
    case "sweep":
      return createSweepGeometry(leaf);
    default:
      return null;
  }
}

function createExtrudeGeometry(
  leaf: HybridLeafInfo,
): THREE.BufferGeometry | null {
  const contour = leaf.contour;
  if (!contour || contour.outer.length < 3) return null;
  const halfHeight = leaf.params[0];
  const shape = new THREE.Shape(
    contour.outer.map(([x, y]) => new THREE.Vector2(x, y)),
  );
  for (const hole of contour.holes) {
    shape.holes.push(
      new THREE.Path(hole.map(([x, y]) => new THREE.Vector2(x, y))),
    );
  }
  // ExtrudeGeometry self-normalises the shape orientation (ShapeUtils orients
  // outer CCW / holes CW regardless of the input loop direction), so its
  // winding + normals are already outward — no ensureOutwardOrientation needed
  // here (verified: signed volume > 0 for both CW and CCW input contours).
  const geo = new THREE.ExtrudeGeometry(shape, {
    depth: 2 * halfHeight,
    bevelEnabled: false,
  });
  geo.translate(0, 0, -halfHeight);
  return geo;
}

function createRevolveGeometry(
  leaf: HybridLeafInfo,
): THREE.BufferGeometry | null {
  const contour = leaf.contour;
  if (!contour || contour.outer.length < 3) return null;
  // The kernel's contour is an IMPLICITLY-CLOSED loop — its last vertex
  // connects back to the first to bound a solid cross-section (that's why the
  // SDF, which fills that region, is correct: a204 "SDF VM korrekt").
  // LatheGeometry only links CONSECUTIVE profile points though, so without an
  // explicit closing vertex the lathe is left OPEN: the annular wall between the
  // last and first rings is missing. The opaque G-buffer captures nothing across
  // that hole, so those pixels read straight through to the background — the
  // deferred "inside-out"/see-through (tickets ceat/0b8m/a72n). It surfaces on
  // the user's ANGLE/GLES WebGPU but not on native Vulkan because backends
  // raster/cull the open seam differently — which is exactly why it never
  // reproduced in automation. Close the loop so the proxy is a complete solid.
  const points = contour.outer.map(([x, y]) => new THREE.Vector2(x, y));
  const firstP = points[0];
  const lastP = points[points.length - 1];
  if (firstP.distanceTo(lastP) > 1e-5) points.push(firstP.clone());
  const geo = new THREE.LatheGeometry(points, 48);
  ensureOutwardOrientation(geo);
  return geo;
}

function createHelixGeometry(
  leaf: HybridLeafInfo,
): THREE.BufferGeometry | null {
  if (!leaf.contour) return null;
  return buildHelixGeometry(leaf.contour, decodeHelixParams(leaf.params));
}

function createSweepGeometry(
  leaf: HybridLeafInfo,
): THREE.BufferGeometry | null {
  if (!leaf.contour || !leaf.sweep) return null;
  return buildSweepGeometry(leaf.sweep.points, leaf.contour, {
    closed: leaf.sweep.closed,
    twist: leaf.sweep.twist,
  });
}

/** The leaf's local→world placement matrix (quat·scale + center offset). */
export function leafWorldMatrix(leaf: HybridLeafInfo): THREE.Matrix4 {
  const t = leaf.transform;
  const translate = new THREE.Vector3(t[0], t[1], t[2]);
  const quat = new THREE.Quaternion(t[3], t[4], t[5], t[6]);
  const scale = new THREE.Vector3(t[7], t[8], t[9]);
  const centerOffset = getCenterOffset(leaf);
  if (centerOffset) {
    const localOffset = new THREE.Vector3(
      centerOffset[0] * scale.x,
      centerOffset[1] * scale.y,
      centerOffset[2] * scale.z,
    );
    localOffset.applyQuaternion(quat);
    translate.add(localOffset);
  }
  return new THREE.Matrix4().compose(translate, quat, scale);
}

export function applyLeafTransform(
  mesh: THREE.Mesh,
  leaf: HybridLeafInfo,
): void {
  mesh.matrixAutoUpdate = false;
  mesh.matrix.copy(leafWorldMatrix(leaf));
  mesh.matrixWorldNeedsUpdate = true;
}

/**
 * Build a Twist/Bend leaf's geometry already in WORLD space and deformed:
 * the primitive is baked through `leafWorldMatrix`, subdivided along the
 * warp axis for smoothness, then warped per-vertex. The resulting mesh
 * carries an IDENTITY transform (do NOT call applyLeafTransform on it).
 * Returns null for a leaf with no warp or no meshable primitive.
 */
export function buildWarpedLeafGeometry(
  leaf: HybridLeafInfo,
): THREE.BufferGeometry | null {
  if (!leaf.warp) return null;
  const geo = createPrimitiveGeometry(leaf);
  if (!geo) return null;
  geo.applyMatrix4(leafWorldMatrix(leaf));
  subdivideAlongAxis(geo, warpAxis(leaf.warp), maxStepForWarp(leaf.warp));
  applyWarp(geo, leaf.warp);
  geo.computeVertexNormals();
  return geo;
}

function getCenterOffset(
  leaf: HybridLeafInfo,
): [number, number, number] | null {
  const p = leaf.params;
  switch (leaf.kind) {
    case "box":
      return [p[3], p[4], p[5]];
    case "sphere":
      return [p[1], p[2], p[3]];
    case "cylinder":
      return [p[2], p[3], p[4]];
    case "torus":
      return [p[2], p[3], p[4]];
    default:
      return null;
  }
}
