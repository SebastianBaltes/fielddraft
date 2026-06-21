/**
 * TypeScript mirrors of the `fielddraft-solver` Rust crate's wire
 * types (K5a web side).
 *
 * FieldDraft's solver lives in two halves under the K5a "separate
 * WASM" integration path: `crates/solver` defines the Rust-side
 * data model (entities, constraints, diagnostics) with serde, and
 * this file mirrors that data model as TypeScript interfaces so
 * the worker-side PlaneGCS wrapper can consume the exact same
 * JSON shape the Rust sketch features produce. The mirrors are
 * hand-maintained for now; a later slice may add a schema
 * generator to derive them from the Rust types automatically.
 *
 * Every field here matches the serde wire form on the Rust side
 * one-to-one. If you change a field here, change it in
 * `crates/solver/src/lib.rs` in the same commit (and vice versa).
 */

/** Stable, sketch-local handle. Mirrors Rust `EntityId(u32)`. */
export type EntityId = number;

/** Stable, sketch-local handle. Mirrors Rust `ConstraintId(u32)`. */
export type ConstraintId = number;

/** 2D point in sketch coordinates, millimetres. */
export interface Point2 {
  x: number;
  y: number;
}

// ─── entities ────────────────────────────────────────────────────

export type Entity =
  | { kind: "point"; id: EntityId; at: Point2 }
  | { kind: "line"; id: EntityId; start: EntityId; end: EntityId }
  | { kind: "circle"; id: EntityId; center: EntityId; radius: number }
  | {
      kind: "arc";
      id: EntityId;
      center: EntityId;
      start: EntityId;
      end: EntityId;
      radius: number;
    }
  | {
      // PlaneGCS parameterisation: centre + one focus point + the
      // semi-minor radius. Semi-major and orientation are implied by
      // the focus (`a = hypot(radmin, |centre − focus1|)`, major axis
      // along centre→focus1). `focus1` is an internal construction
      // point — allocated so the solver can reshape the ellipse, but
      // excluded from hit-testing / box-select (see sketchHitTest).
      // Mirrors Rust `Entity::Ellipse` in `crates/solver/src/lib.rs`.
      kind: "ellipse";
      id: EntityId;
      center: EntityId;
      focus1: EntityId;
      radmin: number;
    }
  | {
      // A B-spline curve: an ordered list of control points (`poles`)
      // plus the curve `degree` and a `periodic` flag. The knot vector
      // is implied (clamped-uniform for `!periodic`). The poles are
      // ordinary point entities; the B-spline carries no movable scalar.
      // Only open control-polygon splines are modelled (point-on-spline
      // binds to this entity). Mirrors Rust `Entity::BSpline`.
      kind: "bspline";
      id: EntityId;
      poles: EntityId[];
      degree: number;
      periodic: boolean;
    }
  | { kind: "origin"; id: EntityId };

// ─── constraints ─────────────────────────────────────────────────

/**
 * One sketch constraint. Matches the twelve variants of the Rust
 * `Constraint` enum on the wire. Numeric variants carry their
 * `value_mm` (millimetres) or `value_deg` (degrees) plus an
 * optional `driven` flag that the UI will flip via the K5b
 * DoF-preflight path.
 */
export type Constraint =
  | { kind: "coincident"; id: ConstraintId; a: EntityId; b: EntityId }
  // Two lines share one infinite supporting line (Onshape's "Coincident"
  // applied to two lines). Realised in the solver as parallel + a point of
  // one line lying on the other — see `translateConstraint`.
  | { kind: "collinear"; id: ConstraintId; a: EntityId; b: EntityId }
  | { kind: "parallel"; id: ConstraintId; a: EntityId; b: EntityId }
  | { kind: "horizontal"; id: ConstraintId; line: EntityId }
  | { kind: "vertical"; id: ConstraintId; line: EntityId }
  | { kind: "perpendicular"; id: ConstraintId; a: EntityId; b: EntityId }
  | {
      kind: "distance";
      id: ConstraintId;
      a: EntityId;
      b: EntityId;
      value_mm: number;
      driven?: boolean;
    }
  | {
      kind: "angle";
      id: ConstraintId;
      a: EntityId;
      b: EntityId;
      value_deg: number;
      driven?: boolean;
    }
  | { kind: "tangent"; id: ConstraintId; line: EntityId; curve: EntityId }
  | { kind: "concentric"; id: ConstraintId; a: EntityId; b: EntityId }
  | { kind: "fix"; id: ConstraintId; point: EntityId }
  | {
      kind: "point_on_object";
      id: ConstraintId;
      point: EntityId;
      curve: EntityId;
    }
  | { kind: "equal"; id: ConstraintId; a: EntityId; b: EntityId }
  | { kind: "midpoint"; id: ConstraintId; point: EntityId; line: EntityId }
  | {
      kind: "symmetric";
      id: ConstraintId;
      a: EntityId;
      b: EntityId;
      axis: EntityId;
    }
  | {
      kind: "radius";
      id: ConstraintId;
      entity: EntityId;
      value_mm: number;
      driven?: boolean;
    }
  | {
      kind: "diameter";
      id: ConstraintId;
      entity: EntityId;
      value_mm: number;
      driven?: boolean;
    };

/** Does this constraint carry an editable numeric value? */
export function isDimensional(c: Constraint): boolean {
  return (
    c.kind === "distance" ||
    c.kind === "angle" ||
    c.kind === "radius" ||
    c.kind === "diameter"
  );
}

// ─── sketch state ────────────────────────────────────────────────

export interface SketchState {
  name: string;
  next_entity_id: number;
  next_constraint_id: number;
  entities: Entity[];
  constraints: Constraint[];
}

/** Empty sketch with no origin. ID counters start at 1 so id 0
 *  stays reserved for a canonical origin inserted via
 *  [`sketchWithOrigin`]. */
export function emptySketch(name = ""): SketchState {
  return {
    name,
    next_entity_id: 1,
    next_constraint_id: 1,
    entities: [],
    constraints: [],
  };
}

/** Fresh sketch that starts with an immovable origin entity at
 *  `(0, 0)` under id 0. */
export function sketchWithOrigin(name = ""): SketchState {
  const s = emptySketch(name);
  s.entities.push({ kind: "origin", id: 0 });
  return s;
}

// ─── diagnostics ─────────────────────────────────────────────────

export interface SketchDiagnostics {
  /** Did the last solve converge within tolerance? */
  converged: boolean;
  /** Signed degrees of freedom. Positive = under-constrained
   *  (blue in the UI), zero = fully constrained (black), negative
   *  or non-empty `conflicting` = over-constrained (red). */
  dof: number;
  conflicting: ConstraintId[];
  redundant: ConstraintId[];
  partially_redundant: ConstraintId[];
  malformed: ConstraintId[];
}

export function emptyDiagnostics(): SketchDiagnostics {
  return {
    converged: false,
    dof: 0,
    conflicting: [],
    redundant: [],
    partially_redundant: [],
    malformed: [],
  };
}

export function fullyConstrained(d: SketchDiagnostics): boolean {
  return (
    d.converged &&
    d.dof === 0 &&
    d.conflicting.length === 0 &&
    d.redundant.length === 0 &&
    d.malformed.length === 0
  );
}

// ─── solve result ────────────────────────────────────────────────

export interface SolveResult {
  state: SketchState;
  diagnostics: SketchDiagnostics;
}
