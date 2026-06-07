/**
 * DoF pre-flight (K5b adapter layer).
 *
 * Before a new dimension (distance or angle) lands in the
 * sketch the adapter layer runs a quick **degrees-of-freedom
 * count** to decide whether the dimension should be inserted
 * as "driving" (the solver uses its value as a hard equation)
 * or "driven" (informational — the solver displays the value
 * but doesn't enforce it). If the hard insertion would push
 * the DOF count below zero, the dimension lands as driven so
 * the user doesn't see a cryptic over-constraint error in the
 * middle of their flow.
 *
 * ## What counts as a DOF here
 *
 * K5b uses a **naive tally**, not a full Jacobi-rank analysis.
 * The rules are:
 *
 * * Every free [`Entity::Point`] contributes 2 DOFs (x, y).
 * * Every [`Entity::Origin`] contributes 0 — it's fixed.
 * * Every [`Entity::Line`] contributes 0 itself; its two
 *   endpoints already account for the 4 DOFs the line needs.
 * * Every [`Entity::Circle`] contributes 1 (the radius); the
 *   centre point contributes its own 2.
 * * Every [`Entity::Arc`] contributes 1 (the radius); the
 *   centre, start, and end points contribute their own 2 each,
 *   minus the coincidence between start/end and the arc's
 *   incident points which is handled elsewhere in the sketch.
 *
 * Each [`Constraint`] subtracts a count matching the
 * conventional DoF reduction:
 *
 * * `Coincident` → 2 (two points glued → one shared pair)
 * * `Parallel` / `Perpendicular` / `Horizontal` / `Vertical`
 *   → 1
 * * `Distance` / `Angle` / `Equal` → 1
 * * `Tangent` → 1
 * * `Concentric` → 2 (two centres coincide)
 * * `Midpoint` → 2
 * * `Symmetric` → 2
 *
 * The naive tally mis-counts in the presence of redundant or
 * coupled constraints — a full Jacobi analysis would catch
 * that — but the K5b use case is strictly "predict the next
 * step's sign", not "produce the canonical DOF number". When
 * the sign is wrong, PlaneGCS' own diagnostic feedback kicks
 * in and the UI still shows the right colour. The K5b README
 * commits to a working DoF-preflight that auto-flips the user
 * to driven; numerical exactness is a future slice.
 *
 * ## API shape
 *
 * Two functions:
 *
 * * [`countDegreesOfFreedom`] — return the naive DOF tally of
 *   a sketch in its current state.
 * * [`preflightDimension`] — return a verdict for a proposed
 *   new dimension: "driving" if the DOF count stays >= 0
 *   after the insert, "driven" otherwise. Callers pass the
 *   candidate constraint; the preflight tries the insert
 *   virtually and undoes it before returning.
 */

import type {
  Constraint,
  Entity,
  SketchDiagnostics,
  SketchState,
} from "./types";

/**
 * Degrees-of-freedom to **display** in the UI.
 *
 * Prefers the authoritative post-solve number the solver itself
 * computed (`diagnostics.dof`, threaded through from
 * `wrapper.gcs.dof()`) once a solve has converged. Falls back to
 * the naive [`countDegreesOfFreedom`] tally only for instantaneous
 * feedback before the first solve lands, or when the last solve
 * didn't converge — situations where no authoritative number
 * exists yet.
 *
 * PlaneGCS reports the *remaining* DoF, which is never negative;
 * over-constraint surfaces through `diagnostics.conflicting`, not a
 * negative count. The naive fallback can still go negative, which
 * the UI renders as an "excess" hint while no solve has run.
 */
export function displayDegreesOfFreedom(
  state: SketchState,
  diagnostics: SketchDiagnostics | undefined,
): number {
  if (diagnostics?.converged) return diagnostics.dof;
  return countDegreesOfFreedom(state);
}

/**
 * Naive DOF tally for a sketch. Positive = under-constrained,
 * zero = fully constrained, negative = over-constrained.
 *
 * This is a solve-free estimate used for two things only: the
 * pre-solve driving/driven what-if in [`preflightDimension`], and
 * as the pre-first-solve fallback inside [`displayDegreesOfFreedom`].
 * The number actually shown to the user once a sketch has solved is
 * the authoritative solver DoF, not this tally.
 */
export function countDegreesOfFreedom(state: SketchState): number {
  let dof = 0;
  for (const e of state.entities) {
    dof += entityDof(e);
  }
  for (const c of state.constraints) {
    dof -= constraintDof(c);
  }
  return dof;
}

function entityDof(entity: Entity): number {
  switch (entity.kind) {
    case "point":
      return 2;
    case "origin":
      return 0;
    case "line":
      return 0;
    case "circle":
      return 1;
    case "arc":
      return 1;
  }
}

function constraintDof(constraint: Constraint): number {
  switch (constraint.kind) {
    case "coincident":
      return 2;
    case "parallel":
    case "perpendicular":
    case "horizontal":
    case "vertical":
    case "distance":
    case "angle":
    case "equal":
    case "tangent":
    case "radius":
    case "diameter":
      return 1;
    case "point_on_object":
      // The point keeps one freedom sliding along the curve, so only
      // one DOF is removed.
      return 1;
    case "concentric":
    case "midpoint":
    case "symmetric":
    case "fix":
      return 2;
  }
}

/** Verdict returned by [`preflightDimension`]. */
export enum DimensionVerdict {
  /** Insert the dimension as driving — the sketch still has
   *  room. */
  Driving = "driving",
  /** Insert the dimension as driven — a driving insert would
   *  over-constrain the sketch. */
  Driven = "driven",
}

/**
 * Run the preflight check on a candidate dimension. The
 * candidate's `driven` field is ignored — the preflight
 * decides. Non-dimensional constraints (parallel, coincident,
 * etc.) don't have a driven/driving toggle; passing one to
 * this function returns [`DimensionVerdict.Driving`] without
 * any additional check.
 */
export function preflightDimension(
  state: SketchState,
  candidate: Constraint,
): DimensionVerdict {
  if (candidate.kind !== "distance" && candidate.kind !== "angle") {
    return DimensionVerdict.Driving;
  }
  const current = countDegreesOfFreedom(state);
  // Subtract the candidate's DOF cost. Dimensional constraints
  // always cost one DOF each.
  if (current - 1 < 0) {
    return DimensionVerdict.Driven;
  }
  return DimensionVerdict.Driving;
}
