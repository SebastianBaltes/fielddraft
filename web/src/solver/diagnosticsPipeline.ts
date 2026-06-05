/**
 * Diagnostics pipeline (K5b adapter layer).
 *
 * Turns the raw [`SketchDiagnostics`] from [`solveSketch`] into
 * per-entity and per-constraint state annotations that the UI
 * layer can render directly as the three-colour model the
 * README §4.4 specifies: blue = under-constrained, black = fully
 * constrained, red = over-constrained or in conflict.
 *
 * The transformation is a pure function of `(state,
 * diagnostics)` — no solver calls, no side effects, no cached
 * state. That makes it easy to test in isolation and safe to
 * re-run on every frame if the UI wants live updates.
 */

import type {
  Constraint,
  ConstraintId,
  EntityId,
  SketchDiagnostics,
  SketchState,
} from "./types";

/** Visual state for one entity or constraint in the sketch. */
export enum SketchElementState {
  /** Under-constrained — the DOFs this element contributes
   *  aren't fully pinned. Drawn in blue. */
  UnderConstrained = "under",
  /** Fully constrained — the element is pinned by enough
   *  constraints that the solver converges on one solution.
   *  Drawn in black. */
  FullyConstrained = "full",
  /** Over-constrained or in conflict with another constraint.
   *  Drawn in red. */
  OverConstrained = "over",
  /** Malformed or references missing entities. Drawn in red
   *  with an error badge. */
  Malformed = "malformed",
}

/** Aggregated per-entity / per-constraint annotations. */
export interface SketchElementStates {
  /** Per-entity visual state keyed by [`EntityId`]. */
  entities: Map<EntityId, SketchElementState>;
  /** Per-constraint visual state keyed by [`ConstraintId`]. */
  constraints: Map<ConstraintId, SketchElementState>;
  /** One-shot overall state the UI status bar can show. */
  overall: SketchElementState;
}

/**
 * Classify every entity and constraint in the sketch from the
 * solver's diagnostics. Entities bound by an over-constrained
 * or malformed constraint pick up the highest-severity state
 * their bindings imply.
 */
export function classifySketch(
  state: SketchState,
  diagnostics: SketchDiagnostics,
): SketchElementStates {
  const conflictingSet = new Set(diagnostics.conflicting);
  const redundantSet = new Set(diagnostics.redundant);
  const malformedSet = new Set(diagnostics.malformed);

  const hasAnyError =
    conflictingSet.size > 0 || malformedSet.size > 0;

  // The overall state reflects the whole sketch at a glance.
  // Under-constrained is a "blue" hint, not an error: it means
  // "you can still add more constraints". An over-constrained
  // sketch outranks everything else, because the solver likely
  // didn't converge and the user needs to fix that first.
  let overall: SketchElementState = SketchElementState.FullyConstrained;
  if (hasAnyError) {
    overall = malformedSet.size > 0
      ? SketchElementState.Malformed
      : SketchElementState.OverConstrained;
  } else if (!diagnostics.converged || diagnostics.dof > 0) {
    overall = SketchElementState.UnderConstrained;
  } else if (diagnostics.dof < 0) {
    overall = SketchElementState.OverConstrained;
  }

  // Per-constraint classification. Start everyone at "full",
  // then downgrade conflicting / malformed / redundant to
  // their specific states.
  const constraints = new Map<ConstraintId, SketchElementState>();
  for (const c of state.constraints) {
    if (malformedSet.has(c.id)) {
      constraints.set(c.id, SketchElementState.Malformed);
    } else if (conflictingSet.has(c.id)) {
      constraints.set(c.id, SketchElementState.OverConstrained);
    } else if (redundantSet.has(c.id)) {
      // Redundant isn't an error — it's a hint the user could
      // clean up. The UI draws it as over-constrained yellow
      // in some sketchers; we classify it as full-constrained
      // here and let the UI layer add its own badge.
      constraints.set(c.id, SketchElementState.FullyConstrained);
    } else {
      constraints.set(c.id, SketchElementState.FullyConstrained);
    }
  }

  // Per-entity classification. An entity's state is the worst
  // state of any constraint that binds it. Entities with no
  // binding constraints at all inherit the overall state, so a
  // freshly dropped point in an otherwise solved sketch still
  // shows up as under-constrained blue instead of black.
  const entities = new Map<EntityId, SketchElementState>();
  const boundEntities = new Set<EntityId>();
  for (const c of state.constraints) {
    for (const id of entitiesReferencedBy(c)) {
      boundEntities.add(id);
      const current = entities.get(id) ?? SketchElementState.FullyConstrained;
      const cState = constraints.get(c.id) ?? SketchElementState.FullyConstrained;
      entities.set(id, worseOf(current, cState));
    }
  }

  // Entities not referenced by any constraint inherit the
  // overall state so the UI colours them consistently with the
  // sketch as a whole — a lonely point in an under-constrained
  // sketch is also under-constrained.
  for (const e of state.entities) {
    if (!boundEntities.has(e.id)) {
      entities.set(e.id, overall);
    }
  }

  return { entities, constraints, overall };
}

/**
 * Return the worst of two states. The ordering is Malformed >
 * OverConstrained > UnderConstrained > FullyConstrained — a
 * single malformed reference makes the entity malformed, a
 * conflict makes it over-constrained, and an under-constrained
 * binding only downgrades a fully-constrained starting point.
 */
function worseOf(
  a: SketchElementState,
  b: SketchElementState,
): SketchElementState {
  const rank = (s: SketchElementState) => {
    switch (s) {
      case SketchElementState.Malformed:
        return 3;
      case SketchElementState.OverConstrained:
        return 2;
      case SketchElementState.UnderConstrained:
        return 1;
      case SketchElementState.FullyConstrained:
        return 0;
    }
  };
  return rank(a) >= rank(b) ? a : b;
}

/** List the entity ids a constraint references. Used to
 *  propagate per-constraint states back onto the entities. */
export function entitiesReferencedBy(
  constraint: Constraint,
): EntityId[] {
  switch (constraint.kind) {
    case "coincident":
    case "parallel":
    case "perpendicular":
    case "concentric":
    case "equal":
      return [constraint.a, constraint.b];
    case "horizontal":
    case "vertical":
      return [constraint.line];
    case "distance":
    case "angle":
      return [constraint.a, constraint.b];
    case "tangent":
      return [constraint.line, constraint.curve];
    case "midpoint":
      return [constraint.point, constraint.line];
    case "symmetric":
      return [constraint.a, constraint.b, constraint.axis];
    case "radius":
    case "diameter":
      return [constraint.entity];
  }
}
