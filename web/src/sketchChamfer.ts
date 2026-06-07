/**
 * Interactive line–line chamfer for the 2D sketcher (UI gap follow-up,
 * straight-edge sibling of [`sketchFillet`](./sketchFillet)).
 *
 * "Chamfer" — the user picks two line sub-calls that meet (or nearly
 * meet) at a corner. The sharp corner is replaced by a straight bevel
 * line connecting two setback points, each at distance `dist` from the
 * corner along its own line, and the two lines are trimmed back to those
 * setback points. Like fillet, this is a pure, DOM-free rewrite that
 * reads the solved geometry from the sub-call params and emits ordinary
 * `sketch_line` sub-calls — the kernel needs no chamfer tool. The store
 * splices the trimmed lines + the new bevel line in via the existing
 * [`replaceSketchSubCallAt`](./sketchSession) rewrite helpers and
 * re-solves.
 *
 * Geometry: given the corner vertex V where the two lines meet and the
 * far ends F1, F2, the unit directions away from the corner are
 * d1 = unit(F1 − V), d2 = unit(F2 − V). The setback points are
 *   S1 = V + dist·d1, S2 = V + dist·d2
 * The bevel is the straight line S1→S2. The two lines are shortened so
 * their corner endpoint becomes the respective setback point, the far
 * endpoint untouched.
 */

import type { Curve, Vec2 } from "./sketchTrim";

/** Re-export the shared geometry vocabulary so callers (tests, store)
 *  don't reach into two modules for the same `Curve` / `Vec2` types. */
export type { Curve, Vec2 } from "./sketchTrim";

const EPS = 1e-9;

// ── small vector helpers ────────────────────────────────────────────

const sub = (p: Vec2, q: Vec2): Vec2 => [p[0] - q[0], p[1] - q[1]];
const add = (p: Vec2, q: Vec2): Vec2 => [p[0] + q[0], p[1] + q[1]];
const scale = (p: Vec2, s: number): Vec2 => [p[0] * s, p[1] * s];
const cross = (p: Vec2, q: Vec2): number => p[0] * q[1] - p[1] * q[0];
const norm = (p: Vec2): number => Math.hypot(p[0], p[1]);
const dist = (p: Vec2, q: Vec2): number => Math.hypot(p[0] - q[0], p[1] - q[1]);

function unit(p: Vec2): Vec2 | null {
  const n = norm(p);
  if (n < EPS) return null;
  return [p[0] / n, p[1] / n];
}

/** The result of a line–line chamfer: the bevel line plus the two
 *  shortened lines (far endpoints preserved, corner endpoints moved to
 *  the setback points). */
export interface ChamferResult {
  line: Curve;
  trimmed1: Curve;
  trimmed2: Curve;
}

/**
 * Bevel the corner between two line curves with a straight chamfer at
 * setback distance `dist`. Returns the bevel line and the two shortened
 * lines, or null when no chamfer is possible:
 *  - either operand isn't a line, or has zero length,
 *  - `dist` is not a finite, strictly-positive number,
 *  - the lines are parallel / antiparallel (no corner to bevel),
 *  - the lines don't share a near corner (closest endpoints far apart),
 *  - the setback exceeds either leg length.
 *
 * The shared corner is the closest pair of endpoints (one from each
 * line); the other two endpoints are the far ends that survive.
 */
export function chamferLines(
  l1: Curve,
  l2: Curve,
  dist_: number,
): ChamferResult | null {
  if (l1.kind !== "line" || l2.kind !== "line") return null;
  if (!Number.isFinite(dist_) || dist_ <= EPS) return null;

  // Identify the shared corner V: the closest pair of endpoints (one
  // from each line). The far ends are the survivors.
  const ends1: [Vec2, Vec2] = [l1.a, l1.b];
  const ends2: [Vec2, Vec2] = [l2.a, l2.b];
  let best = Infinity;
  let i1 = 0;
  let i2 = 0;
  for (let a = 0; a < 2; a++) {
    for (let b = 0; b < 2; b++) {
      const d = dist(ends1[a], ends2[b]);
      if (d < best) {
        best = d;
        i1 = a;
        i2 = b;
      }
    }
  }

  const corner1 = ends1[i1];
  const corner2 = ends2[i2];
  const far1 = ends1[1 - i1];
  const far2 = ends2[1 - i2];

  // Lengths of the two lines: the setback can't exceed either.
  const len1 = dist(corner1, far1);
  const len2 = dist(corner2, far2);
  if (len1 < EPS || len2 < EPS) return null;

  // The corner gap must be small relative to the lines — otherwise the
  // two endpoints aren't really a shared corner.
  if (best > Math.min(len1, len2)) return null;

  // Each line's direction away from its own corner endpoint toward its
  // far end. Computed per-line (not from an averaged vertex) so a true
  // parallel pair is detected even with a small corner offset.
  const d1 = unit(sub(far1, corner1));
  const d2 = unit(sub(far2, corner2));
  if (!d1 || !d2) return null;

  // Parallel / antiparallel → no corner to bevel.
  if (Math.abs(cross(d1, d2)) < EPS) return null;

  // Corner vertex V: intersection of the two infinite support lines, so
  // the setback geometry is exact even when the drawn endpoints leave a
  // small gap. (cross(d1,d2) ≠ 0 here, so the system is solvable.)
  const denom = cross(d1, d2);
  const w = sub(corner2, corner1);
  const s = cross(w, d2) / denom; // corner1 + s·d1 = intersection
  const V: Vec2 = add(corner1, scale(d1, s));

  // Setback can't run past either segment end.
  if (dist_ > len1 - EPS || dist_ > len2 - EPS) return null;

  const S1 = add(V, scale(d1, dist_));
  const S2 = add(V, scale(d2, dist_));

  return {
    line: { kind: "line", a: S1, b: S2 },
    trimmed1: lineWithCorner(l1, i1, S1),
    trimmed2: lineWithCorner(l2, i2, S2),
  };
}

/** Rebuild a line with its corner endpoint (index `i`: 0 = a, 1 = b)
 *  moved to `to`, the far endpoint untouched. */
function lineWithCorner(line: Curve & { kind: "line" }, i: number, to: Vec2): Curve {
  return i === 0
    ? { kind: "line", a: to, b: line.b }
    : { kind: "line", a: line.a, b: to };
}
