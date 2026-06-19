/**
 * Shared shell assembly for swept solids (helix, general sweep).
 *
 * Given a stack of cross-section rings — one full profile per spine sample,
 * every ring holding the SAME concatenated loop points (outer first, then
 * each hole) as flat 3D positions — this stitches the lateral walls between
 * consecutive rings and (for an open sweep) triangulates the two end caps.
 *
 * The cross-section layout matches `THREE.ShapeUtils.triangulateShape`, so
 * cap triangle indices map directly onto the per-ring vertex blocks.
 */

import * as THREE from "three";
import type { ContourData } from "./kernel";

/** Loop offset ranges within one ring's concatenated point block. */
export interface LoopRanges {
  ranges: { start: number; count: number }[];
  pointsPerRing: number;
}

/** Outer loop first, then each hole — the canonical concatenation order. */
export function contourLoopRanges(contour: ContourData): LoopRanges {
  const loops = [contour.outer, ...contour.holes];
  const ranges: { start: number; count: number }[] = [];
  let offset = 0;
  for (const loop of loops) {
    ranges.push({ start: offset, count: loop.length });
    offset += loop.length;
  }
  return { ranges, pointsPerRing: offset };
}

/**
 * Build the indexed shell geometry from pre-placed ring positions.
 * `positions` holds `nRings · pointsPerRing` vertices (3 floats each).
 * `closed` wraps the last ring back to the first and omits the end caps
 * (used for closed-loop sweeps).
 */
export function assembleShell(
  positions: Float32Array,
  nRings: number,
  contour: ContourData,
  opts: { closed?: boolean } = {},
): THREE.BufferGeometry {
  const closed = opts.closed ?? false;
  const { ranges, pointsPerRing } = contourLoopRanges(contour);
  const indices: number[] = [];

  // ── Lateral walls between consecutive rings (wrapping if closed) ───
  const ringSpans = closed ? nRings : nRings - 1;
  for (const { start, count } of ranges) {
    for (let k = 0; k < ringSpans; k++) {
      const k1 = (k + 1) % nRings;
      const a0 = k * pointsPerRing + start;
      const a1 = k1 * pointsPerRing + start;
      for (let j = 0; j < count; j++) {
        const jn = (j + 1) % count;
        indices.push(a0 + j, a0 + jn, a1 + jn, a0 + j, a1 + jn, a1 + j);
      }
    }
  }

  // ── End caps for an open sweep ─────────────────────────────────────
  if (!closed) {
    const outerV = contour.outer.map(([x, y]) => new THREE.Vector2(x, y));
    const holeV = contour.holes.map((h) =>
      h.map(([x, y]) => new THREE.Vector2(x, y)),
    );
    const capTris = THREE.ShapeUtils.triangulateShape(outerV, holeV);
    const lastBase = (nRings - 1) * pointsPerRing;
    for (const [i0, i1, i2] of capTris) {
      // First ring faces backwards → reverse winding so its normal points out.
      indices.push(i0, i2, i1);
      indices.push(lastBase + i0, lastBase + i1, lastBase + i2);
    }
  }

  const geo = new THREE.BufferGeometry();
  geo.setAttribute("position", new THREE.BufferAttribute(positions, 3));
  geo.setIndex(indices);
  geo.computeVertexNormals();
  // The lateral-wall winding above follows the contour's point order, so a
  // CW sketch loop yields an inside-out shell. Normalise it (see below).
  ensureOutwardOrientation(geo);
  return geo;
}

/**
 * Force an indexed, closed triangle mesh to OUTWARD orientation — winding and
 * the normal attribute, independently. Used by every extrusion family that
 * inherits its winding from the sketch's arbitrary CW/CCW contour order
 * (revolve via LatheGeometry, sweep + helix via {@link assembleShell}); a CW
 * loop would otherwise render inside-out (the deferred G-buffer captures the
 * FrontSide, so an inward wall culls the near face and writes the far one).
 *
 * Winding: the signed volume `V = (1/6)·Σ v0·(v1×v2)` equals the enclosed
 * volume when wound outward, so `V < 0` ⇒ inside-out ⇒ reverse every triangle.
 * This is the divergence theorem, exact for ANY closed manifold — concave,
 * holed, or torus-topology (a revolved loop that never meets the axis) — not a
 * radial heuristic that would cancel out on a torus.
 *
 * Normals: a builder's stored normals can disagree with the winding (e.g.
 * `LatheGeometry` on a −x profile yields outward winding but inward normals).
 * With the winding now outward the geometric face normal points out, so if the
 * stored normals anti-correlate with it across the mesh, negate them. The
 * builder's own (smooth) normals are preserved — only their sign is fixed.
 *
 * No-ops on a mesh that is already outward (e.g. `ExtrudeGeometry`, which
 * self-normalises its shape orientation) or non-indexed (returns early).
 */
export function ensureOutwardOrientation(geo: THREE.BufferGeometry): void {
  const idx = geo.getIndex();
  const pos = geo.getAttribute("position");
  if (!idx || !pos) return;
  const a = new THREE.Vector3();
  const b = new THREE.Vector3();
  const c = new THREE.Vector3();
  const e1 = new THREE.Vector3();
  const e2 = new THREE.Vector3();
  const faceN = new THREE.Vector3();

  let vol6 = 0;
  for (let t = 0; t < idx.count; t += 3) {
    a.fromBufferAttribute(pos, idx.getX(t));
    b.fromBufferAttribute(pos, idx.getX(t + 1));
    c.fromBufferAttribute(pos, idx.getX(t + 2));
    vol6 += a.dot(e1.crossVectors(b, c));
  }
  if (vol6 < 0) {
    const arr = idx.array as Uint16Array | Uint32Array;
    for (let t = 0; t < arr.length; t += 3) {
      const tmp = arr[t + 1];
      arr[t + 1] = arr[t + 2];
      arr[t + 2] = tmp;
    }
    idx.needsUpdate = true;
  }

  const nor = geo.getAttribute("normal");
  if (!nor) return;
  let agree = 0;
  for (let t = 0; t < idx.count; t += 3) {
    const ia = idx.getX(t);
    const ib = idx.getX(t + 1);
    const ic = idx.getX(t + 2);
    a.fromBufferAttribute(pos, ia);
    b.fromBufferAttribute(pos, ib);
    c.fromBufferAttribute(pos, ic);
    faceN.crossVectors(e1.subVectors(b, a), e2.subVectors(c, a));
    const nx = nor.getX(ia) + nor.getX(ib) + nor.getX(ic);
    const ny = nor.getY(ia) + nor.getY(ib) + nor.getY(ic);
    const nz = nor.getZ(ia) + nor.getZ(ib) + nor.getZ(ic);
    agree += faceN.x * nx + faceN.y * ny + faceN.z * nz;
  }
  if (agree < 0) {
    const arr = nor.array as Float32Array;
    for (let i = 0; i < arr.length; i++) arr[i] = -arr[i];
    nor.needsUpdate = true;
  }
}
