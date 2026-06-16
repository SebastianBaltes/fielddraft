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

import type { Curve, SubCall, Vec2 } from "./sketchTrim";
import { subCallOfCurve } from "./sketchTrim";

/** Re-export the shared geometry vocabulary so callers (tests, store)
 *  don't reach into two modules for the same `Curve` / `Vec2` types. */
export type { Curve, Vec2 } from "./sketchTrim";

const EPS = 1e-9;
const TAU = Math.PI * 2;

// ── Plan item 4: post-hoc re-edit recipe (web-only hybrid) ──────────
//
// A baked fillet is NOT parametric in the kernel — `filletSketchAt`
// trims the two source lines and appends an ordinary `sketch_arc`. To
// make that arc re-editable in-scene WITHOUT a kernel change we attach
// a `fillet` recipe param to the emitted arc sub-call. The kernel's
// `SketchArcDef::parse` ignores unknown params (no `deny_unknown_fields`)
// and round-trips them verbatim through `replaceSketch` → store →
// `getSketchSubCalls`, so the recipe survives commit/reopen; only the
// web reads it. Geometry/entity allocation is unchanged — this is
// purely additive on the params Value.
//
// The two trimmed line sub-calls are tagged with a stable `fd_id`
// (array indices shift under reorder, so we reference by id), and the
// recipe stores the ORIGINAL (pre-trim) line geometry so a radius
// change can re-derive from scratch.

/** A line referenced by a corner recipe: the original, pre-trim
 *  endpoints `{ a, b }` in sketch mm. */
export interface RecipeLine {
  a: Vec2;
  b: Vec2;
}

/** The `fillet` recipe attached to a baked rounding arc's params. */
export interface FilletRecipe {
  line_a_id: string;
  line_b_id: string;
  radius: number;
  orig_a: RecipeLine;
  orig_b: RecipeLine;
}

/** Param key carrying a sub-call's stable id (additive; kernel-ignored). */
export const FD_ID_PARAM = "fd_id";

let fdIdCounter = 0;

/** Generate a process-unique stable sub-call id. Not persisted-unique
 *  across sessions, but a baked corner and its source lines are always
 *  tagged in one `filletSketchAt`/`chamferSketchAt` call, so the ids
 *  only need to be unique within the spine at write time. */
export function nextFdId(): string {
  fdIdCounter += 1;
  return `fd${Date.now().toString(36)}_${fdIdCounter.toString(36)}`;
}

/** Read a sub-call's stable id, or null if it carries none. */
export function fdIdOf(call: SubCall): string | null {
  const id = call.params[FD_ID_PARAM];
  return typeof id === "string" ? id : null;
}

/** Read the `fillet` recipe off a `sketch_arc` sub-call, or null if the
 *  call isn't a recipe-tagged fillet arc. Defensive about shape so a
 *  hand-edited / malformed param never throws at pick time. */
export function filletRecipeOf(call: SubCall): FilletRecipe | null {
  if (call.type !== "sketch_arc") return null;
  const r = call.params.fillet as Partial<FilletRecipe> | undefined;
  if (!r || typeof r !== "object") return null;
  if (typeof r.line_a_id !== "string" || typeof r.line_b_id !== "string") {
    return null;
  }
  if (!Number.isFinite(r.radius)) return null;
  if (!isRecipeLine(r.orig_a) || !isRecipeLine(r.orig_b)) return null;
  return r as FilletRecipe;
}

function isRecipeLine(v: unknown): v is RecipeLine {
  if (!v || typeof v !== "object") return false;
  const l = v as RecipeLine;
  return (
    Array.isArray(l.a) &&
    l.a.length === 2 &&
    Array.isArray(l.b) &&
    l.b.length === 2
  );
}

/** Build the `fillet` recipe object from the two original (pre-trim)
 *  line curves, their stable ids, and the radius used. */
export function filletRecipe(
  lineAId: string,
  lineBId: string,
  origA: Curve & { kind: "line" },
  origB: Curve & { kind: "line" },
  radius: number,
): FilletRecipe {
  return {
    line_a_id: lineAId,
    line_b_id: lineBId,
    radius,
    orig_a: { a: origA.a, b: origA.b },
    orig_b: { a: origB.a, b: origB.b },
  };
}

/**
 * Plan item 4: re-derive a tagged fillet at a new radius and return the
 * rewritten sub-call list, or null when nothing changes (arc not found /
 * not a fillet, a referenced source line is gone, or the new radius is
 * geometrically impossible — see [`filletLines`]).
 *
 * Re-runs `filletLines(orig_a, orig_b, newRadius)` from the recipe's
 * pristine pre-trim geometry, then rewrites the two referenced line
 * sub-calls (matched by their stable `fd_id`) to the freshly trimmed
 * remainders and the arc sub-call to the new rounding arc carrying an
 * updated recipe. Pure: no solve, no DOM — the store wraps it with
 * `updateSketch` + a re-solve.
 *
 * KNOWN LIMITATION (chained fillets): `orig_a` / `orig_b` snapshot the
 * source lines as they were at FIRST creation. If a later fillet/chamfer
 * trimmed one of those same lines again, re-editing this corner restores
 * the line to its first-creation length (it does not see the second
 * trim). Re-edit corners outermost-first, or recreate the corner, to
 * avoid surprises. Not over-engineered here by design.
 */
export function reeditFilletRadius(
  subCalls: ReadonlyArray<SubCall>,
  arcSubCallId: string,
  newRadius: number,
): SubCall[] | null {
  const arcIdx = subCalls.findIndex((c) => fdIdOf(c) === arcSubCallId);
  if (arcIdx < 0) return null;
  const recipe = filletRecipeOf(subCalls[arcIdx]);
  if (!recipe) return null;

  const origA: Curve = { kind: "line", a: recipe.orig_a.a, b: recipe.orig_a.b };
  const origB: Curve = { kind: "line", a: recipe.orig_b.a, b: recipe.orig_b.b };
  const fillet = filletLines(origA, origB, newRadius);
  if (!fillet) return null;

  const idxA = subCalls.findIndex((c) => fdIdOf(c) === recipe.line_a_id);
  const idxB = subCalls.findIndex((c) => fdIdOf(c) === recipe.line_b_id);
  if (idxA < 0 || idxB < 0) return null;

  const carry = (params: Record<string, unknown>): Record<string, unknown> =>
    params.construction !== undefined
      ? { construction: params.construction }
      : {};

  const out = subCalls.slice();
  // Rewrite the two source lines to their new trimmed remainders, keeping
  // their stable ids so any further re-edit still resolves them.
  out[idxA] = subCallOfCurve(fillet.trimmed1, {
    ...carry(subCalls[idxA].params),
    [FD_ID_PARAM]: recipe.line_a_id,
  });
  out[idxB] = subCallOfCurve(fillet.trimmed2, {
    ...carry(subCalls[idxB].params),
    [FD_ID_PARAM]: recipe.line_b_id,
  });
  // Rewrite the arc, refreshing the recipe's radius (orig geometry and
  // ids are unchanged — re-edits always derive from first-creation).
  out[arcIdx] = subCallOfCurve(fillet.arc, {
    ...carry(subCalls[arcIdx].params),
    [FD_ID_PARAM]: arcSubCallId,
    fillet: { ...recipe, radius: newRadius },
  });
  return out;
}

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
