/**
 * Pure geometry for the centre-to-centre slot (stadium) tool (F9).
 *
 * A slot is two parallel straight edges between end-cap centres A and B,
 * joined by two outward-bulging semicircles of radius r. The kernel
 * lowers `sketch_slot` to a closed `Path` of 2 line + 2 arc segments
 * (see `slot_path_segments` in `crates/kernel/src/sketch_features.rs`);
 * this module mirrors that boundary math so the sketcher renders exactly
 * the region that extrudes.
 *
 * Sketch space is Y-up, angles CCW from +x — the same convention as
 * `sketchArc.ts`. Kept free of React/DOM so the math is unit-testable.
 */

import type { Vec2 } from "./sketchArc";

/** Minimum length / radius (mm) below which a slot is degenerate. */
export const MIN_SLOT_LEN_MM = 1e-6;
export const MIN_SLOT_RADIUS_MM = 1e-6;

/**
 * Boundary description of a slot: the four corner points (where the
 * straight edges meet the caps) plus the two cap centres and radius.
 * Corner order matches the kernel winding:
 *   aPlus = A+r·n, bPlus = B+r·n, bMinus = B−r·n, aMinus = A−r·n.
 */
export interface SlotOutline {
  aPlus: Vec2;
  bPlus: Vec2;
  bMinus: Vec2;
  aMinus: Vec2;
  centerA: Vec2;
  centerB: Vec2;
  radius: number;
  /** Start angle (rad) of the B cap arc; sweep is −π. */
  capBStart: number;
  /** Start angle (rad) of the A cap arc; sweep is −π. */
  capAStart: number;
}

/**
 * Compute the stadium boundary for a slot drawn from end-centre A to
 * end-centre B with the given radius, or `null` if degenerate
 * (coincident centres or non-positive radius). Mirrors the kernel
 * `slot_path_segments`.
 */
export function slotOutline(a: Vec2, b: Vec2, radius: number): SlotOutline | null {
  const dx = b[0] - a[0];
  const dy = b[1] - a[1];
  const len = Math.hypot(dx, dy);
  if (len < MIN_SLOT_LEN_MM || radius < MIN_SLOT_RADIUS_MM) {
    return null;
  }
  // Unit axis A→B and its left normal n = (−u.y, u.x).
  const ux = dx / len;
  const uy = dy / len;
  const nx = -uy;
  const ny = ux;
  const r = radius;
  const aPlus: Vec2 = [a[0] + r * nx, a[1] + r * ny];
  const bPlus: Vec2 = [b[0] + r * nx, b[1] + r * ny];
  const bMinus: Vec2 = [b[0] - r * nx, b[1] - r * ny];
  const aMinus: Vec2 = [a[0] - r * nx, a[1] - r * ny];
  // Cap B starts at B+r·n (angle of +n), sweeps −π through +u (outward,
  // away from A). Cap A starts at A−r·n (angle of −n), sweeps −π through
  // −u (outward, away from B).
  const capBStart = Math.atan2(ny, nx);
  const capAStart = Math.atan2(-ny, -nx);
  return {
    aPlus,
    bPlus,
    bMinus,
    aMinus,
    centerA: [a[0], a[1]],
    centerB: [b[0], b[1]],
    radius: r,
    capBStart,
    capAStart,
  };
}
