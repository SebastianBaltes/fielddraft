/**
 * Interactive line–line fillet for the 2D sketcher (UI gap follow-up,
 * sibling of [`sketchTrim`](./sketchTrim)).
 *
 * "Fillet" — the user picks two line sub-calls that meet (or nearly
 * meet) at a corner. The sharp corner is replaced by a tangent arc of
 * the requested radius, and the two lines are trimmed back to their
 * tangent points. Like trim, this is a pure, DOM-free rewrite that
 * reads the solved geometry from the sub-call params and emits ordinary
 * `sketch_line` / `sketch_arc` sub-calls — the kernel needs no fillet
 * tool. The store splices the trimmed lines + the new arc in via the
 * existing [`replaceSketchSubCallAt`](./sketchSession) rewrite helpers
 * and re-solves.
 *
 * Geometry: given the corner vertex V where the two lines meet and the
 * far ends F1, F2, the unit directions away from the corner are
 * d1 = unit(F1 − V), d2 = unit(F2 − V). The fillet centre sits on the
 * angle bisector. With θ the angle between the lines (cosθ = d1·d2):
 *   tangent distance t = r / tan(θ/2)
 *   tangent points     T1 = V + t·d1, T2 = V + t·d2
 *   arc centre         C  = V + (r / sin(θ/2)) · unit(d1 + d2)
 * The arc is the minor (short-way) arc from T1 to T2 around C; its
 * midpoint bulges toward the corner V. The two lines are shortened so
 * their corner endpoint becomes the respective tangent point, the far
 * endpoint untouched.
 */

import type { Curve, Vec2 } from "./sketchTrim";

/** Re-export the shared geometry vocabulary so callers (tests, store)
 *  don't reach into two modules for the same `Curve` / `Vec2` types. */
export type { Curve, Vec2 } from "./sketchTrim";

const EPS = 1e-9;
const TAU = Math.PI * 2;

// ── small vector helpers ────────────────────────────────────────────

const sub = (p: Vec2, q: Vec2): Vec2 => [p[0] - q[0], p[1] - q[1]];
const add = (p: Vec2, q: Vec2): Vec2 => [p[0] + q[0], p[1] + q[1]];
const scale = (p: Vec2, s: number): Vec2 => [p[0] * s, p[1] * s];
const dot = (p: Vec2, q: Vec2): number => p[0] * q[0] + p[1] * q[1];
const cross = (p: Vec2, q: Vec2): number => p[0] * q[1] - p[1] * q[0];
const norm = (p: Vec2): number => Math.hypot(p[0], p[1]);
const dist = (p: Vec2, q: Vec2): number => Math.hypot(p[0] - q[0], p[1] - q[1]);

function unit(p: Vec2): Vec2 | null {
  const n = norm(p);
  if (n < EPS) return null;
  return [p[0] / n, p[1] / n];
}

/** The result of a line–line fillet: the rounding arc plus the two
 *  shortened lines (far endpoints preserved, corner endpoints moved to
 *  the tangent points). */
export interface FilletResult {
  arc: Curve;
  trimmed1: Curve;
  trimmed2: Curve;
}

/**
 * Round the corner between two line curves with a tangent arc of radius
 * `r`. Returns the arc and the two shortened lines, or null when no
 * fillet is possible:
 *  - either operand isn't a line, or has zero length,
 *  - `r` is not a finite, strictly-positive number,
 *  - the lines are parallel / antiparallel (no corner to round),
 *  - the lines don't share a near corner (closest endpoints far apart),
 *  - the radius is so large a tangent point would fall past a far end.
 *
 * The shared corner is the closest pair of endpoints (one from each
 * line); the other two endpoints are the far ends that survive.
 */
export function filletLines(
  l1: Curve,
  l2: Curve,
  r: number,
): FilletResult | null {
  if (l1.kind !== "line" || l2.kind !== "line") return null;
  if (!Number.isFinite(r) || r <= EPS) return null;

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

  // Lengths of the two lines: the tangent distance can't exceed either.
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

  // Parallel / antiparallel → no corner to round.
  if (Math.abs(cross(d1, d2)) < EPS) return null;

  // Corner vertex V: intersection of the two infinite support lines, so
  // the tangent geometry is exact even when the drawn endpoints leave a
  // small gap. (cross(d1,d2) ≠ 0 here, so the system is solvable.)
  const denom = cross(d1, d2);
  const w = sub(corner2, corner1);
  const s = cross(w, d2) / denom; // corner1 + s·d1 = intersection
  const V: Vec2 = add(corner1, scale(d1, s));

  // Half-angle of the corner. cosθ = d1·d2 (θ ∈ (0, π)); the bisector is
  // unit(d1 + d2), undefined exactly at θ = π (antiparallel, already
  // excluded above).
  const cosTheta = Math.max(-1, Math.min(1, dot(d1, d2)));
  const theta = Math.acos(cosTheta);
  const half = theta / 2;
  const sinHalf = Math.sin(half);
  const tanHalf = Math.tan(half);
  if (sinHalf < EPS || tanHalf < EPS) return null;

  const t = r / tanHalf; // tangent distance from V along each line
  if (t > len1 - EPS || t > len2 - EPS) return null; // radius too large

  const bis = unit(add(d1, d2));
  if (!bis) return null;

  const T1 = add(V, scale(d1, t));
  const T2 = add(V, scale(d2, t));
  const C = add(V, scale(bis, r / sinHalf));

  // Arc from T1 to T2 around C, sweeping the short way. The minor arc is
  // the one whose midpoint lies on the corner side of C (toward V): we
  // pick the sweep sign from cross(C→T1, C→T2) and clamp to the minor
  // angle, then verify the midpoint bulges toward V.
  const start = Math.atan2(T1[1] - C[1], T1[0] - C[0]);
  const endAng = Math.atan2(T2[1] - C[1], T2[0] - C[0]);
  let sweep = endAng - start;
  // Wrap into (−π, π] — the minor arc between the tangent points.
  while (sweep <= -Math.PI) sweep += TAU;
  while (sweep > Math.PI) sweep -= TAU;

  const arc: Curve = { kind: "arc", c: C, r, start, sweep };

  // Sanity: the arc midpoint must bulge toward the corner (be closer to
  // V than the chord midpoint would suggest — i.e. on V's side of C).
  // If our minor-arc sign put the midpoint away from V, flip it.
  const mid = pointOnArc(arc, 0.5);
  const chordMid: Vec2 = [(T1[0] + T2[0]) / 2, (T1[1] + T2[1]) / 2];
  if (dist(mid, V) > dist(chordMid, V) + EPS) {
    // Wrong side: take the complementary (still ≤ π in magnitude only if
    // we negate the small sweep). Negating walks the other way round.
    sweep = sweep > 0 ? sweep - TAU : sweep + TAU;
  }

  return {
    arc: { kind: "arc", c: C, r, start, sweep },
    trimmed1: lineWithCorner(l1, i1, T1),
    trimmed2: lineWithCorner(l2, i2, T2),
  };
}

/** Point on an arc at normalized parameter s ∈ [0, 1] (0 = start). */
function pointOnArc(arc: Curve & { kind: "arc" }, s: number): Vec2 {
  const a = arc.start + arc.sweep * s;
  return [arc.c[0] + arc.r * Math.cos(a), arc.c[1] + arc.r * Math.sin(a)];
}

/** Rebuild a line with its corner endpoint (index `i`: 0 = a, 1 = b)
 *  moved to `to`, the far endpoint untouched. */
function lineWithCorner(line: Curve & { kind: "line" }, i: number, to: Vec2): Curve {
  return i === 0
    ? { kind: "line", a: to, b: line.b }
    : { kind: "line", a: line.a, b: to };
}
