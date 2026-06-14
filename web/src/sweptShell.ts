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
  return geo;
}
