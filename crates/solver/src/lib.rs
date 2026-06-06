//! FieldDraft 2D-constraint-solver data model (K5a).
//!
//! This crate is the Rust side of the three-layer solver
//! architecture described in README §4.4. K5a commits to the
//! **separate-WASM integration path** (option 1 from §4.4): the
//! PlaneGCS solver runs as a second WebAssembly module inside the
//! regeneration worker, loaded via `@salusoft89/planegcs` on the
//! TypeScript side. This Rust crate therefore doesn't talk to
//! PlaneGCS directly and carries no C/C++ toolchain dependency.
//! Its job is to define the **wire shape** that flows between
//! Rust sketch features and the TypeScript solver wrapper:
//! entities, constraints, diagnostics, and the sketch-state
//! aggregate that a sketch feature produces and the solver
//! consumes.
//!
//! ## What lives here
//!
//! * [`Entity`] — the five PlaneGCS primitive shapes we commit to
//!   supporting in the MVP: `Point`, `Line`, `Circle`, `Arc`, and
//!   `Origin` (a fixed reference point that anchors every
//!   sketch). Each entity has an `EntityId` handle plus its typed
//!   parameters.
//! * [`Constraint`] — the full "required constraint" list from
//!   the README's §K5a checklist: `Coincident`, `Parallel`,
//!   `Horizontal`, `Vertical`, `Perpendicular`, `Distance`,
//!   `Angle`, `Tangent`, `Concentric`, `Equal`, `Midpoint`,
//!   `Symmetric`. Each carries the entity / point references it
//!   binds plus any numeric value (length / angle).
//! * [`SketchState`] — the serialisable aggregate of one sketch:
//!   an auto-incrementing id counter, the entity list, and the
//!   constraint list. The TypeScript solver wrapper takes this
//!   as its input and returns a solved copy of it.
//! * [`SketchDiagnostics`] — the structured translation of
//!   PlaneGCS' `getConflicting` / `getRedundant` /
//!   `getPartiallyRedundant` / `getMalformed` output that the
//!   adapter layer's diagnostics pipeline (K5b) will colour-code
//!   for the UI.
//!
//! ## What does NOT live here (yet)
//!
//! * The solve call itself — that runs in TypeScript via
//!   `web/src/solver/planegcs.ts` (K5a web side). Rust code
//!   serialises a `SketchState`, hands it to the worker, gets
//!   back a solved state. No synchronous Rust ↔ PlaneGCS calls
//!   exist in the separate-WASM path.
//! * The four adapter-layer sub-components from README §4.4
//!   (InferenceEngine, DragController, DoF-Preflight,
//!   Diagnostics pipeline). Those arrive in K5b on top of this
//!   data model.
//! * Any `ConstraintSolver` trait or Rust-side solver backend.
//!   The separate-WASM integration leaves the solver call on the
//!   JS side; the Rust crate stays data-only.

use serde::{Deserialize, Serialize};

// ─── identifiers ────────────────────────────────────────────────

/// Stable handle for an entity (point / line / circle / arc) or a
/// constraint inside a single [`SketchState`].
///
/// IDs are sketch-local — two different sketches can both use
/// `EntityId(1)` without clashing because each sketch owns its
/// own auto-increment counter. Cross-sketch references are
/// expressed at a higher level (via the K4.5.7 query system).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct EntityId(pub u32);

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct ConstraintId(pub u32);

/// 2D point in the sketch plane's local coordinates. Millimetres
/// throughout — the sketch plane inherits the kernel's native
/// length unit, same as [`Length::to_mm`](fielddraft_protocol::Length::to_mm).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point2 {
    pub x: f64,
    pub y: f64,
}

impl Point2 {
    pub const ORIGIN: Self = Self { x: 0.0, y: 0.0 };

    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

// ─── entities ───────────────────────────────────────────────────

/// One sketch entity — a geometric primitive the solver can
/// constrain. Mirrors the PlaneGCS primitive set that K5a commits
/// to supporting in the MVP; additional types (ellipse, B-spline)
/// can be added later without breaking the wire shape because the
/// enum is versioned through [`fielddraft_protocol::SCHEMA_VERSION`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Entity {
    /// A free point in the sketch plane.
    Point { id: EntityId, at: Point2 },
    /// A straight line segment between two named endpoints. The
    /// solver tracks the endpoints as independent DOFs, which is
    /// how PlaneGCS models lines internally.
    Line {
        id: EntityId,
        start: EntityId,
        end: EntityId,
    },
    /// A full circle with a centre point and a radius length.
    /// Radius is a scalar DOF the solver can drive via Distance
    /// constraints (`Radius`) or `Equal` constraints between two
    /// circles.
    Circle {
        id: EntityId,
        center: EntityId,
        radius: f64,
    },
    /// A circular arc: centre + start point + end point + radius.
    /// Sweep direction is encoded by the order of start/end and
    /// a CCW convention (matching PlaneGCS).
    Arc {
        id: EntityId,
        center: EntityId,
        start: EntityId,
        end: EntityId,
        radius: f64,
    },
    /// The immovable origin of the sketch. Every sketch has
    /// exactly one origin; it's the anchor that keeps the
    /// constraint graph's DOF count from running away and it's
    /// what sketches reference when they say "horizontal to the
    /// X axis".
    Origin { id: EntityId },
}

impl Entity {
    /// Numeric tag for diagnostics and serialisation stability.
    pub fn kind_name(&self) -> &'static str {
        match self {
            Entity::Point { .. } => "point",
            Entity::Line { .. } => "line",
            Entity::Circle { .. } => "circle",
            Entity::Arc { .. } => "arc",
            Entity::Origin { .. } => "origin",
        }
    }

    pub fn id(&self) -> EntityId {
        match *self {
            Entity::Point { id, .. }
            | Entity::Line { id, .. }
            | Entity::Circle { id, .. }
            | Entity::Arc { id, .. }
            | Entity::Origin { id } => id,
        }
    }
}

// ─── constraints ────────────────────────────────────────────────

/// One constraint on a sketch. The twelve variants cover the
/// "required" constraint set from the README §K5a checklist —
/// enough to drive Onshape-style sketching once the K5b adapter
/// layer lights up. Every variant carries the entity / point
/// references it binds; numeric variants also carry their target
/// value in millimetres (distances) or degrees (angles).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Constraint {
    /// Two points coincide. Used both as a hard constraint (the
    /// user-facing "coincident" tool) and as the solver backbone
    /// for line-endpoint sharing.
    Coincident {
        id: ConstraintId,
        a: EntityId,
        b: EntityId,
    },
    /// Two lines are parallel (same direction vector, either
    /// orientation).
    Parallel {
        id: ConstraintId,
        a: EntityId,
        b: EntityId,
    },
    /// A single line is horizontal in sketch coordinates.
    Horizontal { id: ConstraintId, line: EntityId },
    /// A single line is vertical in sketch coordinates.
    Vertical { id: ConstraintId, line: EntityId },
    /// Two lines meet at 90 degrees.
    Perpendicular {
        id: ConstraintId,
        a: EntityId,
        b: EntityId,
    },
    /// Distance between two points (mm). The `driven` flag
    /// marks the constraint as informational only — the solver
    /// still evaluates it but doesn't treat it as a hard equation
    /// (README §4.4 driven-vs-driving dimension story).
    Distance {
        id: ConstraintId,
        a: EntityId,
        b: EntityId,
        value_mm: f64,
        #[serde(default)]
        driven: bool,
    },
    /// Angle between two lines, measured in degrees so the wire
    /// form matches the UI and documentation. The solver
    /// converts to radians at the boundary. Same `driven` flag
    /// semantics as [`Constraint::Distance`].
    Angle {
        id: ConstraintId,
        a: EntityId,
        b: EntityId,
        value_deg: f64,
        #[serde(default)]
        driven: bool,
    },
    /// A line is tangent to a circle or arc at the entity's edge.
    Tangent {
        id: ConstraintId,
        line: EntityId,
        curve: EntityId,
    },
    /// Two circles or arcs share their centre point.
    Concentric {
        id: ConstraintId,
        a: EntityId,
        b: EntityId,
    },
    /// Pin a point at its current position — removes both of the
    /// point's DOFs. Realised on the PlaneGCS side via the entity's
    /// `fixed: true` flag (the same mechanism the origin uses), not
    /// a dedicated solver primitive.
    Fix {
        id: ConstraintId,
        point: EntityId,
    },
    /// Two equivalent entities have the same primary scalar —
    /// same length for lines, same radius for circles/arcs.
    Equal {
        id: ConstraintId,
        a: EntityId,
        b: EntityId,
    },
    /// A point lies at the midpoint of a line.
    Midpoint {
        id: ConstraintId,
        point: EntityId,
        line: EntityId,
    },
    /// Two points are symmetric about a line.
    Symmetric {
        id: ConstraintId,
        a: EntityId,
        b: EntityId,
        axis: EntityId,
    },
    /// A circle or arc has a fixed radius (mm). Dimensional, with the
    /// same `driven` semantics as [`Constraint::Distance`].
    Radius {
        id: ConstraintId,
        entity: EntityId,
        value_mm: f64,
        #[serde(default)]
        driven: bool,
    },
    /// A circle or arc has a fixed diameter (mm). Stored as the
    /// diameter so the UI annotation round-trips; the solver pins
    /// `2·radius`. Dimensional, same `driven` semantics.
    Diameter {
        id: ConstraintId,
        entity: EntityId,
        value_mm: f64,
        #[serde(default)]
        driven: bool,
    },
}

impl Constraint {
    pub fn id(&self) -> ConstraintId {
        match *self {
            Constraint::Coincident { id, .. }
            | Constraint::Parallel { id, .. }
            | Constraint::Horizontal { id, .. }
            | Constraint::Vertical { id, .. }
            | Constraint::Perpendicular { id, .. }
            | Constraint::Distance { id, .. }
            | Constraint::Angle { id, .. }
            | Constraint::Tangent { id, .. }
            | Constraint::Concentric { id, .. }
            | Constraint::Fix { id, .. }
            | Constraint::Equal { id, .. }
            | Constraint::Midpoint { id, .. }
            | Constraint::Symmetric { id, .. }
            | Constraint::Radius { id, .. }
            | Constraint::Diameter { id, .. } => id,
        }
    }

    pub fn kind_name(&self) -> &'static str {
        match self {
            Constraint::Coincident { .. } => "coincident",
            Constraint::Parallel { .. } => "parallel",
            Constraint::Horizontal { .. } => "horizontal",
            Constraint::Vertical { .. } => "vertical",
            Constraint::Perpendicular { .. } => "perpendicular",
            Constraint::Distance { .. } => "distance",
            Constraint::Angle { .. } => "angle",
            Constraint::Tangent { .. } => "tangent",
            Constraint::Concentric { .. } => "concentric",
            Constraint::Fix { .. } => "fix",
            Constraint::Equal { .. } => "equal",
            Constraint::Midpoint { .. } => "midpoint",
            Constraint::Symmetric { .. } => "symmetric",
            Constraint::Radius { .. } => "radius",
            Constraint::Diameter { .. } => "diameter",
        }
    }

    /// Whether this constraint carries a user-editable numeric
    /// value (distance in mm or angle in degrees). Used by the
    /// K5b DoF-preflight code and the UI's driving/driven
    /// toggle — non-dimensional constraints like `Parallel` or
    /// `Coincident` have no numeric knob to flip.
    pub fn is_dimensional(&self) -> bool {
        matches!(
            self,
            Constraint::Distance { .. }
                | Constraint::Angle { .. }
                | Constraint::Radius { .. }
                | Constraint::Diameter { .. }
        )
    }
}

// ─── sketch state ───────────────────────────────────────────────

/// Serialisable snapshot of one sketch — everything the solver
/// needs to produce a solved configuration, and everything a
/// sketch feature needs to persist in the document.
///
/// The `next_entity_id` / `next_constraint_id` counters mirror
/// [`Document::next_feature_id`](fielddraft_protocol::Document)
/// from the K4.5 feature spine: they persist across edits so
/// stable IDs survive round-trips through disk, undo, and
/// PlaneGCS solves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SketchState {
    /// Human-readable label for the sketch. Typically set to the
    /// owning feature's name in the spine.
    #[serde(default)]
    pub name: String,
    /// Counter that seeds the next [`EntityId`]. Starts at 1 —
    /// the origin entity, if present, is conventionally id 0.
    #[serde(default = "next_id_default")]
    pub next_entity_id: u32,
    #[serde(default = "next_id_default")]
    pub next_constraint_id: u32,
    #[serde(default)]
    pub entities: Vec<Entity>,
    #[serde(default)]
    pub constraints: Vec<Constraint>,
}

const fn next_id_default() -> u32 {
    1
}

impl Default for SketchState {
    fn default() -> Self {
        // IDs start at 1 in both the Rust constructor and the
        // serde default so id 0 stays reserved for the canonical
        // origin entity that [`SketchState::with_origin`] inserts.
        Self {
            name: String::new(),
            next_entity_id: 1,
            next_constraint_id: 1,
            entities: Vec::new(),
            constraints: Vec::new(),
        }
    }
}

impl SketchState {
    /// Create a fresh sketch with no entities and no constraints.
    /// The entity counter starts at 1 so id 0 remains available
    /// for a well-known origin entity inserted via
    /// [`SketchState::with_origin`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a sketch that starts with an immovable origin at
    /// `(0, 0)` as entity id 0. Most user-facing sketches start
    /// this way — it's the anchor that keeps the constraint
    /// graph's DOF count from running away.
    pub fn with_origin() -> Self {
        let mut sketch = Self::default();
        sketch.entities.push(Entity::Origin { id: EntityId(0) });
        sketch.next_entity_id = 1;
        sketch
    }

    pub fn allocate_entity_id(&mut self) -> EntityId {
        let id = EntityId(self.next_entity_id);
        self.next_entity_id = self.next_entity_id.saturating_add(1);
        id
    }

    pub fn allocate_constraint_id(&mut self) -> ConstraintId {
        let id = ConstraintId(self.next_constraint_id);
        self.next_constraint_id = self.next_constraint_id.saturating_add(1);
        id
    }

    /// Push an entity with a fresh id and return the id.
    /// Convenience builder for tests and headless sketch setup —
    /// production code that wants specific ids uses
    /// [`allocate_entity_id`](Self::allocate_entity_id) directly.
    pub fn add_point(&mut self, at: Point2) -> EntityId {
        let id = self.allocate_entity_id();
        self.entities.push(Entity::Point { id, at });
        id
    }

    pub fn add_line(&mut self, start: EntityId, end: EntityId) -> EntityId {
        let id = self.allocate_entity_id();
        self.entities.push(Entity::Line { id, start, end });
        id
    }

    pub fn add_circle(&mut self, center: EntityId, radius: f64) -> EntityId {
        let id = self.allocate_entity_id();
        self.entities.push(Entity::Circle {
            id,
            center,
            radius,
        });
        id
    }

    pub fn add_arc(
        &mut self,
        center: EntityId,
        start: EntityId,
        end: EntityId,
        radius: f64,
    ) -> EntityId {
        let id = self.allocate_entity_id();
        self.entities.push(Entity::Arc {
            id,
            center,
            start,
            end,
            radius,
        });
        id
    }

    /// Add a constraint and return its id. Panics only on id
    /// counter overflow (impossible in practice — `u32::MAX`
    /// constraints per sketch would blow the solver's working
    /// set long before this matters).
    pub fn add_constraint(
        &mut self,
        build: impl FnOnce(ConstraintId) -> Constraint,
    ) -> ConstraintId {
        let id = self.allocate_constraint_id();
        let constraint = build(id);
        debug_assert_eq!(
            constraint.id(),
            id,
            "add_constraint builder must use the id it's handed"
        );
        self.constraints.push(constraint);
        id
    }

    /// Look up an entity by id. O(N) — sketch entity counts are
    /// small (tens, not thousands), so a linear scan is fine.
    pub fn entity(&self, id: EntityId) -> Option<&Entity> {
        self.entities.iter().find(|e| e.id() == id)
    }

    pub fn constraint(&self, id: ConstraintId) -> Option<&Constraint> {
        self.constraints.iter().find(|c| c.id() == id)
    }
}

// ─── diagnostics ────────────────────────────────────────────────

/// Structured translation of PlaneGCS' diagnostic output into
/// entity / constraint states. Consumed by the K5b diagnostics
/// pipeline and the UI's three-colour model (blue / black / red).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SketchDiagnostics {
    /// Did the last solve converge? If false, at least one
    /// constraint couldn't be satisfied within tolerance.
    pub converged: bool,
    /// Final degree-of-freedom count. Negative values indicate
    /// the sketch is over-constrained; zero is fully
    /// constrained; positive values are under-constrained
    /// ("blue" in the UI).
    pub dof: i32,
    /// Conflicting constraints — removing any one of them would
    /// restore a solvable sketch.
    #[serde(default)]
    pub conflicting: Vec<ConstraintId>,
    /// Redundant constraints — already implied by other
    /// constraints. Safe to delete, but doesn't cause a solve
    /// failure.
    #[serde(default)]
    pub redundant: Vec<ConstraintId>,
    /// Partially redundant — part of a cycle where removing this
    /// one would still leave the sketch solvable.
    #[serde(default)]
    pub partially_redundant: Vec<ConstraintId>,
    /// Malformed — the constraint references entities that don't
    /// exist or has a structurally invalid setup.
    #[serde(default)]
    pub malformed: Vec<ConstraintId>,
}

impl SketchDiagnostics {
    /// Is the sketch fully constrained (DOF == 0 and no errors)?
    pub fn fully_constrained(&self) -> bool {
        self.converged
            && self.dof == 0
            && self.conflicting.is_empty()
            && self.redundant.is_empty()
            && self.malformed.is_empty()
    }

    /// Is the sketch under-constrained? (DOF > 0)
    pub fn under_constrained(&self) -> bool {
        self.dof > 0
    }

    /// Is the sketch over-constrained or in conflict?
    pub fn over_constrained(&self) -> bool {
        self.dof < 0 || !self.conflicting.is_empty()
    }
}

/// Solver response: the updated sketch state plus diagnostics.
/// Wraps the two together so callers can't accidentally use one
/// without the other.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SolveResult {
    pub state: SketchState,
    pub diagnostics: SketchDiagnostics,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── entities and constraints ─────────────────────────────

    #[test]
    fn allocate_entity_id_is_monotonic() {
        let mut s = SketchState::new();
        let a = s.allocate_entity_id();
        let b = s.allocate_entity_id();
        let c = s.allocate_entity_id();
        assert_eq!(a.0, 1);
        assert_eq!(b.0, 2);
        assert_eq!(c.0, 3);
    }

    #[test]
    fn with_origin_inserts_id_zero_and_bumps_counter() {
        let s = SketchState::with_origin();
        assert_eq!(s.entities.len(), 1);
        assert!(matches!(
            s.entities[0],
            Entity::Origin { id: EntityId(0) }
        ));
        assert_eq!(s.next_entity_id, 1);
    }

    #[test]
    fn add_point_returns_the_new_id_and_stores_the_entity() {
        let mut s = SketchState::with_origin();
        let p = s.add_point(Point2::new(10.0, 20.0));
        assert_eq!(p.0, 1);
        match s.entity(p).unwrap() {
            Entity::Point { at, .. } => {
                assert_eq!(at.x, 10.0);
                assert_eq!(at.y, 20.0);
            }
            other => panic!("expected Point, got {other:?}"),
        }
    }

    #[test]
    fn add_line_references_endpoints_by_id() {
        let mut s = SketchState::with_origin();
        let a = s.add_point(Point2::new(0.0, 0.0));
        let b = s.add_point(Point2::new(10.0, 0.0));
        let line = s.add_line(a, b);
        match s.entity(line).unwrap() {
            Entity::Line { start, end, .. } => {
                assert_eq!(*start, a);
                assert_eq!(*end, b);
            }
            other => panic!("expected Line, got {other:?}"),
        }
    }

    #[test]
    fn add_circle_and_arc_round_trip_through_lookup() {
        let mut s = SketchState::new();
        let center = s.add_point(Point2::ORIGIN);
        let c = s.add_circle(center, 5.0);
        match s.entity(c).unwrap() {
            Entity::Circle { radius, .. } => assert_eq!(*radius, 5.0),
            other => panic!("expected Circle, got {other:?}"),
        }

        let start = s.add_point(Point2::new(5.0, 0.0));
        let end = s.add_point(Point2::new(0.0, 5.0));
        let arc = s.add_arc(center, start, end, 5.0);
        match s.entity(arc).unwrap() {
            Entity::Arc {
                radius,
                start: s_id,
                end: e_id,
                ..
            } => {
                assert_eq!(*radius, 5.0);
                assert_eq!(*s_id, start);
                assert_eq!(*e_id, end);
            }
            other => panic!("expected Arc, got {other:?}"),
        }
    }

    #[test]
    fn add_constraint_assigns_a_fresh_id_via_the_builder() {
        let mut s = SketchState::with_origin();
        let p = s.add_point(Point2::new(5.0, 0.0));
        let c_id = s.add_constraint(|id| Constraint::Coincident {
            id,
            a: EntityId(0),
            b: p,
        });
        assert_eq!(s.constraints.len(), 1);
        assert_eq!(c_id.0, 1);
        assert_eq!(s.constraints[0].id(), c_id);
    }

    #[test]
    fn is_dimensional_only_matches_distance_and_angle() {
        let dist = Constraint::Distance {
            id: ConstraintId(0),
            a: EntityId(0),
            b: EntityId(1),
            value_mm: 10.0,
            driven: false,
        };
        let ang = Constraint::Angle {
            id: ConstraintId(1),
            a: EntityId(0),
            b: EntityId(1),
            value_deg: 45.0,
            driven: false,
        };
        let par = Constraint::Parallel {
            id: ConstraintId(2),
            a: EntityId(0),
            b: EntityId(1),
        };
        assert!(dist.is_dimensional());
        assert!(ang.is_dimensional());
        assert!(!par.is_dimensional());
    }

    // ─── serialisation round-trips ────────────────────────────

    #[test]
    fn sketch_state_round_trips_through_json() {
        let mut s = SketchState::with_origin();
        let p1 = s.add_point(Point2::new(0.0, 0.0));
        let p2 = s.add_point(Point2::new(10.0, 0.0));
        let line = s.add_line(p1, p2);
        s.add_constraint(|id| Constraint::Horizontal { id, line });
        s.add_constraint(|id| Constraint::Distance {
            id,
            a: p1,
            b: p2,
            value_mm: 10.0,
            driven: false,
        });

        let json = serde_json::to_string(&s).unwrap();
        let parsed: SketchState = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, s);
    }

    #[test]
    fn entity_wire_form_carries_kind_tag() {
        let e = Entity::Point {
            id: EntityId(1),
            at: Point2::new(1.0, 2.0),
        };
        let wire = serde_json::to_value(&e).unwrap();
        assert_eq!(wire["kind"], "point");
        assert_eq!(wire["id"], 1);
        assert_eq!(wire["at"]["x"], 1.0);
    }

    #[test]
    fn constraint_wire_form_carries_kind_tag_and_numeric_value() {
        let c = Constraint::Distance {
            id: ConstraintId(7),
            a: EntityId(1),
            b: EntityId(2),
            value_mm: 25.0,
            driven: true,
        };
        let wire = serde_json::to_value(&c).unwrap();
        assert_eq!(wire["kind"], "distance");
        assert_eq!(wire["id"], 7);
        assert_eq!(wire["value_mm"], 25.0);
        assert_eq!(wire["driven"], true);
    }

    // ─── diagnostics ──────────────────────────────────────────

    #[test]
    fn diagnostics_default_is_not_converged() {
        let d = SketchDiagnostics::default();
        assert!(!d.converged);
        assert_eq!(d.dof, 0);
    }

    #[test]
    fn fully_constrained_requires_dof_zero_and_clean_diagnostics() {
        let clean = SketchDiagnostics {
            converged: true,
            dof: 0,
            conflicting: Vec::new(),
            redundant: Vec::new(),
            partially_redundant: Vec::new(),
            malformed: Vec::new(),
        };
        assert!(clean.fully_constrained());

        let under = SketchDiagnostics {
            dof: 3,
            ..clean.clone()
        };
        assert!(!under.fully_constrained());
        assert!(under.under_constrained());

        let over = SketchDiagnostics {
            dof: -1,
            ..clean.clone()
        };
        assert!(over.over_constrained());

        let conflict = SketchDiagnostics {
            conflicting: vec![ConstraintId(1)],
            ..clean.clone()
        };
        assert!(conflict.over_constrained());
        assert!(!conflict.fully_constrained());
    }

    #[test]
    fn solve_result_round_trips_through_json() {
        let mut state = SketchState::with_origin();
        state.add_point(Point2::new(5.0, 5.0));
        let diagnostics = SketchDiagnostics {
            converged: true,
            dof: 2,
            ..SketchDiagnostics::default()
        };
        let result = SolveResult { state, diagnostics };
        let json = serde_json::to_string(&result).unwrap();
        let parsed: SolveResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, result);
    }

    // ─── kind name helpers ────────────────────────────────────

    #[test]
    fn entity_kind_name_matches_wire_tag() {
        let origin = Entity::Origin { id: EntityId(0) };
        assert_eq!(origin.kind_name(), "origin");
        let pt = Entity::Point {
            id: EntityId(1),
            at: Point2::ORIGIN,
        };
        assert_eq!(pt.kind_name(), "point");
    }

    #[test]
    fn constraint_kind_name_matches_wire_tag() {
        let c = Constraint::Tangent {
            id: ConstraintId(1),
            line: EntityId(1),
            curve: EntityId(2),
        };
        assert_eq!(c.kind_name(), "tangent");
    }
}
