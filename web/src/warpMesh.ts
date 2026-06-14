/**
 * Domain-warp meshing for Twist / Bend leaves.
 *
 * Twist and Bend are not sweeps — they are space deformations applied to a
 * whole child subtree. In the BoundedCsg tree every operator runs in WORLD
 * space (transforms are baked into leaves), so the warp axis is world Y
 * (twist) / world X (bend). The mesher therefore bakes a leaf's world
 * transform into its vertices, subdivides along the warp axis so the
 * deformation stays smooth, then displaces each vertex.
 *
 * Twist is a diffeomorphism (`rotateY(rate·y)` is globally injective) → it
 * never self-intersects, so no validity gate is needed. Bend can fold, so
 * the kernel only emits a Bend warp when `|rate|·y_extent < 1`; that same
 * bound makes the fixed-point inverse below a contraction.
 *
 * This module is PURE (geometry in, geometry out) so it can be unit-tested
 * without a GPU; the orchestration (createPrimitiveGeometry → bake → here)
 * lives in sdfDeferredRenderer.
 */

import * as THREE from "three";
import type { LeafWarp } from "./kernel";

/** Target band angle — vertices are subdivided so no band exceeds this. */
const TARGET_ANGLE = 0.12; // ~7° per subdivision band
/** Cap on uniform 1→4 subdivision levels (×4^k triangles). */
const MAX_SUBDIV_LEVELS = 5;
/** Newton iterations for the (implicit) bend inverse — quadratic, so few. */
const BEND_ITERS = 6;

/** World axis the warp deforms around: twist→Y (1), bend→X (0). */
export function warpAxis(warp: LeafWarp): 0 | 1 | 2 {
  return warp.kind === "twist" ? 1 : 0;
}

/** Max spacing along the warp axis before a band exceeds TARGET_ANGLE. */
export function maxStepForWarp(warp: LeafWarp): number {
  return TARGET_ANGLE / Math.max(Math.abs(warp.rate), 1e-6);
}

/**
 * Uniformly subdivide (1→4 midpoint) until the geometry's extent along
 * `axis` is resolved to ≤ `maxStep` per band, capped at MAX_SUBDIV_LEVELS.
 * Conforming + watertight (shared midpoint cache). Mutates and returns geo;
 * any existing normals are dropped (recompute after warping).
 */
export function subdivideAlongAxis(
  geo: THREE.BufferGeometry,
  axis: 0 | 1 | 2,
  maxStep: number,
): THREE.BufferGeometry {
  const posAttr = geo.getAttribute("position");
  let pos = Array.from(posAttr.array as ArrayLike<number>);
  const index = geo.getIndex();
  let idx: number[] = index
    ? Array.from(index.array as ArrayLike<number>)
    : Array.from({ length: posAttr.count }, (_, i) => i);

  let mn = Infinity;
  let mx = -Infinity;
  for (let i = axis; i < pos.length; i += 3) {
    if (pos[i] < mn) mn = pos[i];
    if (pos[i] > mx) mx = pos[i];
  }
  const extent = mx - mn;

  let levels = 0;
  if (extent > 0 && maxStep > 0) {
    levels = Math.min(
      MAX_SUBDIV_LEVELS,
      Math.max(0, Math.ceil(Math.log2(extent / maxStep))),
    );
  }
  for (let s = 0; s < levels; s++) [pos, idx] = subdivideOnce(pos, idx);

  geo.setAttribute("position", new THREE.Float32BufferAttribute(pos, 3));
  geo.setIndex(idx);
  geo.deleteAttribute("normal");
  return geo;
}

/** One conforming 1→4 split of every triangle via its edge midpoints. */
function subdivideOnce(pos: number[], idx: number[]): [number[], number[]] {
  const mid = new Map<number, number>();
  const midpoint = (a: number, b: number): number => {
    const key = a < b ? a * 0x100000 + b : b * 0x100000 + a;
    let m = mid.get(key);
    if (m === undefined) {
      m = pos.length / 3;
      pos.push(
        (pos[a * 3] + pos[b * 3]) / 2,
        (pos[a * 3 + 1] + pos[b * 3 + 1]) / 2,
        (pos[a * 3 + 2] + pos[b * 3 + 2]) / 2,
      );
      mid.set(key, m);
    }
    return m;
  };
  const out: number[] = [];
  for (let i = 0; i < idx.length; i += 3) {
    const a = idx[i];
    const b = idx[i + 1];
    const c = idx[i + 2];
    const ab = midpoint(a, b);
    const bc = midpoint(b, c);
    const ca = midpoint(c, a);
    out.push(a, ab, ca, ab, b, bc, ca, bc, c, ab, bc, ca);
  }
  return [pos, out];
}

/**
 * Deform world-space vertices in place by the warp. Inverse of the kernel's
 * domain warp: a child surface vertex `q` maps to the world point `p` it
 * came from. Caller recomputes normals afterwards.
 */
export function applyWarp(geo: THREE.BufferGeometry, warp: LeafWarp): void {
  const attr = geo.getAttribute("position") as THREE.BufferAttribute;
  const a = attr.array as Float32Array;
  const rate = warp.rate;
  if (warp.kind === "twist") {
    // Kernel evals child at rotateY(+rate·y)·p, so the surface point p of
    // child vertex q is rotateY(−rate·y)·q (same y).
    for (let i = 0; i < a.length; i += 3) {
      const x = a[i];
      const y = a[i + 1];
      const z = a[i + 2];
      const c = Math.cos(rate * y);
      const s = Math.sin(rate * y);
      a[i] = c * x + s * z;
      a[i + 2] = -s * x + c * z;
    }
  } else {
    // Bend evals child at rotateZ(+rate·px)·p, with α = rate·px depending on
    // the PRE-image px → solve f(px)=cos(α)·qx+sin(α)·qy−px=0 by Newton
    // (quadratic; a plain fixed point converges only as rate·|py| which is
    // not bounded by the y_extent gate when the child reaches far in x).
    for (let i = 0; i < a.length; i += 3) {
      const qx = a[i];
      const qy = a[i + 1];
      let px = qx;
      for (let k = 0; k < BEND_ITERS; k++) {
        const al = rate * px;
        const c = Math.cos(al);
        const s = Math.sin(al);
        const f = c * qx + s * qy - px;
        const df = rate * (-s * qx + c * qy) - 1; // = rate·py − 1 < 0 in-gate
        if (Math.abs(df) < 1e-9) break;
        px -= f / df;
      }
      const al = rate * px;
      a[i] = px;
      a[i + 1] = -Math.sin(al) * qx + Math.cos(al) * qy;
    }
  }
  attr.needsUpdate = true;
}
