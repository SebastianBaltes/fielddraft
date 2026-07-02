//! Sketch sub-feature-tree: per-sketch `Vec<FeatureCall>` spine.
//!
//! K6b.1, the first slice of the sketch editor. This module stands
//! up the kernel-side infrastructure that the memory note in
//! `project_sketch_subtree.md` requires: every sketch owns its own
//! fully-typed feature spine, every sketch tool is a feature with
//! a schema, and regeneration walks the sub-spine the same way
//! top-level regeneration walks the document spine.
//!
//! ## Shape of the design
//!
//! * A new top-level `FeatureKind` called `sketch` lives in the
//!   main [`FeatureRegistry`]. Its definition JSON carries two
//!   fields:
//!     - `plane` — a [`Query`] identifying the planar target the
//!       sketch is locked against. K6b.1 stores the query
//!       verbatim but does not resolve it; the face-lock UX is
//!       K6b.2's job.
//!     - `sub_calls` — an ordered array of nested
//!       [`FeatureCall`]s, the sketch's own feature spine.
//!
//! * Each entry of `sub_calls` dispatches through a **separate**
//!   [`SketchToolRegistry`]. That registry's types implement
//!   [`SketchToolKind`], not [`FeatureKind`] — a sketch tool's
//!   job is to push entities into a [`SketchState`], not to
//!   mutate the document spine. Keeping the two registries
//!   disjoint means a top-level call like `box` can't accidentally
//!   appear inside a sketch and vice versa; the type namespaces
//!   stay clean and regeneration errors point at the right layer.
//!
//! * The sketch feature's `execute` builds a fresh [`SketchState`],
//!   walks its sub_calls through the sketch-tool registry, and
//!   stashes the resulting state on
//!   [`Context::sketches`] keyed by the sketch's feature id. Later
//!   slices (K6b.2+) will route that state through the PlaneGCS
//!   solver and resolve downstream `qSketchRegion` queries against
//!   it. K6b.1 stops short of solving because the solver lives in
//!   JavaScript — the state is ready for the JS-side wrapper to
//!   pick up.
//!
//! ## Built-in sketch tools in K6b.1
//!
//! * `sketch_point { at: [x, y] }` — one free point.
//! * `sketch_line  { start: [x, y], end: [x, y] }` — two points
//!   plus a line entity linking them.
//!
//! That's the minimum set needed to smoke-test the sub-spine
//! pipeline end-to-end. Rectangle / Circle / Arc / Trim / Mirror /
//! Offset land in K6b.3 and K6b.4, each as their own
//! `SketchToolKind` impl.

use std::collections::HashMap;

use serde_json::Value;

use fielddraft_protocol::{FeatureCall, FeatureId, Query};
use fielddraft_solver::{Constraint, ConstraintId, EntityId, Point2, SketchState};

use crate::features::{Context, FeatureError, FeatureKind, FeatureSchema};
use crate::sketch2d::{ellipse_quad_beziers, Sketch2d, Sketch2dPrimitive, SketchSegment};

// ─── K7.0: Context-side sketch handle ───────────────────────────

/// Bundle of everything a downstream feature (extrude, revolve,
/// etc.) needs about a resolved sketch: the solver state the
/// kernel walked during regeneration, plus the original sub-spine
/// calls so profile extractors can tell a `sketch_rectangle`
/// call apart from four free `sketch_line` segments. Stored in
/// [`Context::sketches`] keyed by the sketch feature's id.
#[derive(Debug, Clone)]
pub struct SketchData {
    pub state: SketchState,
    pub sub_calls: Vec<FeatureCall>,
    /// C1: the 3D placement frame the 2D sketch lives on. Sketch
    /// entities stay 2D (`(u, v)` in the frame's `x_axis`/`y_axis`
    /// plane); lowering (extrude/revolve) composes
    /// [`SketchFrame::to_transform`] into the emitted feature transform
    /// so the profile sits on the real plane. `origin_plane_xy` yields
    /// the identity frame so legacy XY sketches are unchanged.
    pub frame: SketchFrame,
}

/// C1: a 3D placement for a 2D sketch — an orthonormal right-handed
/// frame `(origin; x_axis, y_axis, normal)`. The sketch's `(u, v)`
/// coordinates map to world as `origin + u·x_axis + v·y_axis`; the
/// `normal` (== `x_axis × y_axis`) is the extrude/offset direction.
///
/// Geometry-neutral: this is just a coordinate frame, no datum-plane
/// or mate-connector semantics baked in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SketchFrame {
    pub origin: [f32; 3],
    pub x_axis: [f32; 3],
    pub y_axis: [f32; 3],
    pub normal: [f32; 3],
}

impl SketchFrame {
    /// The identity frame: origin at world 0, axes aligned with
    /// `+X`/`+Y`/`+Z`. [`SketchFrame::to_transform`] returns
    /// [`Transform::IDENTITY`] for this frame, so a sketch on it lowers
    /// exactly as a pre-C1 XY sketch did.
    pub const IDENTITY: Self = Self {
        origin: [0.0, 0.0, 0.0],
        x_axis: [1.0, 0.0, 0.0],
        y_axis: [0.0, 1.0, 0.0],
        normal: [0.0, 0.0, 1.0],
    };

    /// World point → the frame's `(u, v)` plane coordinates, by
    /// orthogonal projection: `u = (p − origin)·x_axis`,
    /// `v = (p − origin)·y_axis`. The inverse of
    /// `origin + u·x_axis + v·y_axis`; for a point already on the plane
    /// it round-trips exactly. Off-plane points project orthogonally
    /// (the normal component is dropped). Used to express a picked axis
    /// line in the sketch's own 2D coordinates.
    pub fn world_to_local_2d(&self, p: [f32; 3]) -> [f32; 2] {
        let d = [
            p[0] - self.origin[0],
            p[1] - self.origin[1],
            p[2] - self.origin[2],
        ];
        let dot = |a: [f32; 3]| a[0] * d[0] + a[1] * d[1] + a[2] * d[2];
        [dot(self.x_axis), dot(self.y_axis)]
    }

    /// Rigid frame→world [`Transform`]: a rotation whose columns are the
    /// frame axes plus a translation to the origin. Composed into the
    /// lowered feature transform so the 2D profile (built in XY) lands on
    /// this plane.
    pub fn to_transform(&self) -> fielddraft_protocol::Transform {
        fielddraft_protocol::Transform {
            translate: self.origin,
            rotation: basis_to_quat(self.x_axis, self.y_axis, self.normal),
            scale: [1.0, 1.0, 1.0],
        }
    }
}

/// Build a unit quaternion `(x, y, z, w)` from an orthonormal
/// right-handed basis given as the three rotation-matrix columns
/// (`col0 = x_axis`, `col1 = y_axis`, `col2 = z_axis`). Standard
/// matrix→quaternion extraction (Shepperd's method, trace branch).
pub(crate) fn basis_to_quat(c0: [f32; 3], c1: [f32; 3], c2: [f32; 3]) -> [f32; 4] {
    let m00 = c0[0];
    let m10 = c0[1];
    let m20 = c0[2];
    let m01 = c1[0];
    let m11 = c1[1];
    let m21 = c1[2];
    let m02 = c2[0];
    let m12 = c2[1];
    let m22 = c2[2];
    let trace = m00 + m11 + m22;
    if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        [(m21 - m12) / s, (m02 - m20) / s, (m10 - m01) / s, 0.25 * s]
    } else if m00 > m11 && m00 > m22 {
        let s = (1.0 + m00 - m11 - m22).sqrt() * 2.0;
        [0.25 * s, (m01 + m10) / s, (m02 + m20) / s, (m21 - m12) / s]
    } else if m11 > m22 {
        let s = (1.0 + m11 - m00 - m22).sqrt() * 2.0;
        [(m01 + m10) / s, 0.25 * s, (m12 + m21) / s, (m02 - m20) / s]
    } else {
        let s = (1.0 + m22 - m00 - m11).sqrt() * 2.0;
        [(m02 + m20) / s, (m12 + m21) / s, 0.25 * s, (m10 - m01) / s]
    }
}

/// C1: the placement frame for one of the three well-known origin
/// datum planes, addressed by the feature id the web emits
/// (`qCreatedBy(origin_plane_*)`). Returns `None` for any other id.
///
/// * `origin_plane_xy` → origin 0, normal `+Z`, axes `+X`/`+Y`
///   (the identity frame — keeps legacy XY sketches byte-for-byte).
/// * `origin_plane_xz` → normal `+Y`, axes `+X`/`+Z`.
/// * `origin_plane_yz` → normal `+X`, axes `+Y`/`+Z`.
pub fn datum_plane_frame(id: &FeatureId) -> Option<SketchFrame> {
    match id.to_string().as_str() {
        "origin_plane_xy" => Some(SketchFrame::IDENTITY),
        "origin_plane_xz" => Some(SketchFrame {
            origin: [0.0, 0.0, 0.0],
            x_axis: [1.0, 0.0, 0.0],
            y_axis: [0.0, 0.0, 1.0],
            normal: [0.0, 1.0, 0.0],
        }),
        "origin_plane_yz" => Some(SketchFrame {
            origin: [0.0, 0.0, 0.0],
            x_axis: [0.0, 1.0, 0.0],
            y_axis: [0.0, 0.0, 1.0],
            normal: [1.0, 0.0, 0.0],
        }),
        _ => None,
    }
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize3(v: [f32; 3]) -> [f32; 3] {
    let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if n > 1e-9 {
        [v[0] / n, v[1] / n, v[2] / n]
    } else {
        [0.0, 0.0, 1.0]
    }
}

/// C1: build a sketch frame on a B-Rep face. The origin is the face
/// centroid and the normal is the face's (world-space) analytic plane
/// normal; the in-plane `x_axis`/`y_axis` are synthesised by crossing
/// the normal with the world axis least aligned with it (so the basis
/// is well-conditioned for any face orientation).
///
/// `placement` lifts the body-local woven face into world space (the
/// weave is body-local, [`crate::body::Body::placement`] applied at
/// consumption).
pub fn face_plane_frame(
    brep: &crate::brep::BRep,
    id: crate::brep::FaceId,
    placement: &fielddraft_protocol::Transform,
) -> SketchFrame {
    use crate::brep::naming::Fingerprint;

    // Centroid (body-local) → world via the placement.
    let fp = Fingerprint::of_face(brep, id);
    let mut c_local = [
        fp.centroid[0] as f32,
        fp.centroid[1] as f32,
        fp.centroid[2] as f32,
    ];
    // The fingerprint centroid is sample-derived and sits up to ~µm off the
    // carrier. For an analytic planar face, project it onto the EXACT plane
    // (ticket o2j9): a face-extrude anchored here lands flush on the carrier
    // to f32 precision, so stacked extrudes are exactly coincident and the
    // coincidence registry takes its exact path instead of chasing drift.
    if let crate::brep::SurfaceGeometry::Analytic {
        surface: fielddraft_protocol::HalfSpace::Plane { normal, offset },
        transform,
    } = &brep.face(id).surface
    {
        let n = normalize3(crate::quat_rotate(transform.rotation, *normal));
        let s = transform.scale[0];
        let d = s * offset
            + n[0] * transform.translate[0]
            + n[1] * transform.translate[1]
            + n[2] * transform.translate[2];
        let err = c_local[0] * n[0] + c_local[1] * n[1] + c_local[2] * n[2] - d;
        c_local = [
            c_local[0] - err * n[0],
            c_local[1] - err * n[1],
            c_local[2] - err * n[2],
        ];
    }
    let origin = {
        let p = crate::quat_rotate(
            placement.rotation,
            [
                c_local[0] * placement.scale[0],
                c_local[1] * placement.scale[1],
                c_local[2] * placement.scale[2],
            ],
        );
        [
            p[0] + placement.translate[0],
            p[1] + placement.translate[1],
            p[2] + placement.translate[2],
        ]
    };

    // Local analytic plane normal, rotated into world. Curved faces
    // fall back to +Z locally.
    let n_local = match &brep.face(id).surface {
        crate::brep::SurfaceGeometry::Analytic {
            surface: fielddraft_protocol::HalfSpace::Plane { normal, .. },
            transform,
        } => crate::quat_rotate(transform.rotation, *normal),
        _ => [0.0, 0.0, 1.0],
    };
    let normal = normalize3(crate::quat_rotate(placement.rotation, n_local));

    // Pick the world axis least aligned with the normal, cross twice
    // for an orthonormal in-plane basis.
    let ax = normal[0].abs();
    let ay = normal[1].abs();
    let az = normal[2].abs();
    let seed = if ax <= ay && ax <= az {
        [1.0, 0.0, 0.0]
    } else if ay <= az {
        [0.0, 1.0, 0.0]
    } else {
        [0.0, 0.0, 1.0]
    };
    let x_axis = normalize3(cross(seed, normal));
    let y_axis = normalize3(cross(normal, x_axis));
    SketchFrame {
        origin,
        x_axis,
        y_axis,
        normal,
    }
}

// ─── sketch-tool layer ──────────────────────────────────────────

/// One sketch-local tool. Stateless, like [`FeatureKind`], but
/// mutates a [`SketchState`] instead of a [`Context`]. Sketch tools
/// have no access to the document spine — they live entirely
/// inside the sketch they're registered against, which is how the
/// sub-spine stays isolated from the top-level feature tree.
pub trait SketchToolKind: Send + Sync {
    /// Registry key — stable, snake_case, matches the `type_name`
    /// field on a [`FeatureCall`] that appears inside a sketch's
    /// `sub_calls` list.
    fn type_name(&self) -> &'static str;

    /// Schema JSON for this tool's definition struct, same shape
    /// as [`FeatureKind::schema_json`]. Used by the K6b.2
    /// Sketch-toolbar to feed the generic FeatureDialog.
    fn schema_json(&self) -> Value;

    /// Validate the definition JSON and push entities / constraints
    /// into `state`. On error the sketch-level `execute` stops —
    /// same fail-stop rule as the top-level registry.
    fn apply(
        &self,
        state: &mut SketchState,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError>;
}

/// Registry of sketch tools keyed by `type_name`. Separate from
/// [`crate::features::FeatureRegistry`] so sketch-local types and
/// document-level types live in disjoint namespaces.
pub struct SketchToolRegistry {
    tools: HashMap<String, Box<dyn SketchToolKind>>,
}

impl Default for SketchToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SketchToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Empty registry pre-populated with every built-in sketch
    /// tool.
    pub fn with_builtins() -> Self {
        let mut r = Self::new();
        r.register(Box::new(SketchPointTool));
        r.register(Box::new(SketchLineTool));
        r.register(Box::new(SketchRectangleTool));
        r.register(Box::new(SketchCircleTool));
        r.register(Box::new(SketchPolygonTool));
        r.register(Box::new(SketchSplineTool));
        r.register(Box::new(SketchPathTool));
        r.register(Box::new(SketchRegularPolygonTool));
        r.register(Box::new(SketchArcTool));
        r.register(Box::new(SketchEllipseTool));
        r.register(Box::new(SketchSlotTool));
        r.register(Box::new(SketchMirrorTool));
        r.register(Box::new(SketchOffsetTool));
        r.register(Box::new(SketchProjectTool));
        r.register(Box::new(SketchHorizontalConstraintTool));
        r.register(Box::new(SketchVerticalConstraintTool));
        r.register(Box::new(SketchCoincidentConstraintTool));
        r.register(Box::new(SketchDistanceConstraintTool));
        r.register(Box::new(SketchAngleConstraintTool));
        r.register(Box::new(SketchEqualConstraintTool));
        r.register(Box::new(SketchConcentricConstraintTool));
        r.register(Box::new(SketchFixConstraintTool));
        r.register(Box::new(SketchPointOnObjectConstraintTool));
        r.register(Box::new(SketchParallelConstraintTool));
        r.register(Box::new(SketchCollinearConstraintTool));
        r.register(Box::new(SketchPerpendicularConstraintTool));
        r.register(Box::new(SketchTangentConstraintTool));
        r.register(Box::new(SketchMidpointConstraintTool));
        r.register(Box::new(SketchSymmetricConstraintTool));
        r.register(Box::new(SketchRadiusConstraintTool));
        r.register(Box::new(SketchDiameterConstraintTool));
        r
    }

    pub fn register(&mut self, tool: Box<dyn SketchToolKind>) {
        self.tools.insert(tool.type_name().to_string(), tool);
    }

    pub fn get(&self, type_name: &str) -> Option<&dyn SketchToolKind> {
        self.tools.get(type_name).map(|b| b.as_ref())
    }

    pub fn type_names(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }

    pub fn schema_for(&self, type_name: &str) -> Option<Value> {
        self.tools.get(type_name).map(|t| t.schema_json())
    }

    /// Walk a sub-spine, dispatching each call through `get` and
    /// applying it to `state`. Fails fast like the top-level
    /// regenerator.
    pub fn apply_all(
        &self,
        state: &mut SketchState,
        calls: &[FeatureCall],
    ) -> Result<(), FeatureError> {
        for call in calls {
            let tool = self
                .tools
                .get(&call.type_name)
                .ok_or_else(|| FeatureError::UnknownFeatureType(call.type_name.clone()))?;
            tool.apply(state, &call.id, &call.params)?;
        }
        Ok(())
    }
}

// ─── sketch_point ───────────────────────────────────────────────

/// `sketch_point` — a free 2D point in sketch coordinates.
#[derive(FeatureSchema)]
#[schema(type = "sketch_point")]
pub struct SketchPointDef {
    pub at: [f32; 2],
}

pub struct SketchPointTool;

impl SketchToolKind for SketchPointTool {
    fn type_name(&self) -> &'static str {
        SketchPointDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        SketchPointDef::schema_json()
    }

    fn apply(
        &self,
        state: &mut SketchState,
        _id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = SketchPointDef::parse(def)?;
        state.add_point(Point2::new(d.at[0] as f64, d.at[1] as f64));
        Ok(())
    }
}

// ─── sketch_line ────────────────────────────────────────────────

/// `sketch_line` — a 2D line segment between two endpoints. The
/// endpoints become their own points in the sketch state; the
/// line entity links them.
#[derive(FeatureSchema)]
#[schema(type = "sketch_line")]
pub struct SketchLineDef {
    pub start: [f32; 2],
    pub end: [f32; 2],
}

pub struct SketchLineTool;

impl SketchToolKind for SketchLineTool {
    fn type_name(&self) -> &'static str {
        SketchLineDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        SketchLineDef::schema_json()
    }

    fn apply(
        &self,
        state: &mut SketchState,
        _id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = SketchLineDef::parse(def)?;
        let start = state.add_point(Point2::new(d.start[0] as f64, d.start[1] as f64));
        let end = state.add_point(Point2::new(d.end[0] as f64, d.end[1] as f64));
        state.add_line(start, end);
        Ok(())
    }
}

// ─── K6b.3e: rectangle ──────────────────────────────────────────
//
// `sketch_rectangle` is the first *composite* sketch tool: a
// single sub-spine call that materialises into four points,
// four lines and four H/V constraints in one apply pass. The
// UI sees one entry in the sub-spine list and one preview on
// the 2D canvas, but the underlying `SketchState` is wired up
// the same way Onshape's corner-rectangle landing is — four
// horizontal/vertical-constrained edges the solver can keep
// axis-aligned through downstream edits.
//
// The allocation order below is load-bearing: `buildDraftSketchState`
// on the TS side replicates it byte-for-byte so predicted entity
// ids match the kernel's. Do not reorder the `add_point` /
// `add_line` / `add_constraint` calls without updating the TS
// mirror.

/// `sketch_rectangle` — axis-aligned rectangle defined by two
/// diagonal corners. Internally lowers to four corner points,
/// four edge lines and four H/V constraints.
#[derive(FeatureSchema)]
#[schema(type = "sketch_rectangle")]
pub struct SketchRectangleDef {
    pub corner_a: [f32; 2],
    pub corner_b: [f32; 2],
}

pub struct SketchRectangleTool;

impl SketchToolKind for SketchRectangleTool {
    fn type_name(&self) -> &'static str {
        SketchRectangleDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        SketchRectangleDef::schema_json()
    }

    fn apply(
        &self,
        state: &mut SketchState,
        _id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = SketchRectangleDef::parse(def)?;
        let ax = d.corner_a[0] as f64;
        let ay = d.corner_a[1] as f64;
        let bx = d.corner_b[0] as f64;
        let by = d.corner_b[1] as f64;
        // Corners in a fixed allocation order: bl, br, tr, tl.
        // The TS mirror predicts entity ids from this ordering,
        // so changing it is a cross-crate break.
        let min_x = ax.min(bx);
        let min_y = ay.min(by);
        let max_x = ax.max(bx);
        let max_y = ay.max(by);
        let bl = state.add_point(Point2::new(min_x, min_y));
        let br = state.add_point(Point2::new(max_x, min_y));
        let tr = state.add_point(Point2::new(max_x, max_y));
        let tl = state.add_point(Point2::new(min_x, max_y));
        // Edges, same order: bottom, right, top, left.
        let bottom = state.add_line(bl, br);
        let right = state.add_line(br, tr);
        let top = state.add_line(tr, tl);
        let left = state.add_line(tl, bl);
        // Horizontal first (bottom, top), then vertical (right,
        // left) — the TS constraint-id mirror uses this order.
        state.add_constraint(|id| Constraint::Horizontal { id, line: bottom });
        state.add_constraint(|id| Constraint::Horizontal { id, line: top });
        state.add_constraint(|id| Constraint::Vertical { id, line: right });
        state.add_constraint(|id| Constraint::Vertical { id, line: left });
        Ok(())
    }
}

// ─── K6b.3f: circle ─────────────────────────────────────────────
//
// Simpler than rectangle: one center point + one circle entity,
// no internal constraints (Onshape circles land "free" by
// default and the user adds radius / coincident / tangent
// constraints later if they want them). The allocation order
// is two entities total — the center point first, then the
// circle — so the TS mirror only needs to bump next_entity_id
// by two.

/// `sketch_circle` — centre + radius. Internally lowers to a
/// point + a circle entity referencing that point.
#[derive(FeatureSchema)]
#[schema(type = "sketch_circle")]
pub struct SketchCircleDef {
    pub center: [f32; 2],
    pub radius: f32,
}

pub struct SketchCircleTool;

impl SketchToolKind for SketchCircleTool {
    fn type_name(&self) -> &'static str {
        SketchCircleDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        SketchCircleDef::schema_json()
    }

    fn apply(
        &self,
        state: &mut SketchState,
        _id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = SketchCircleDef::parse(def)?;
        let center = state.add_point(Point2::new(d.center[0] as f64, d.center[1] as f64));
        state.add_circle(center, d.radius.max(0.0) as f64);
        Ok(())
    }
}

/// `sketch_arc` — circular sector (pie wedge). Lowers to
/// `Sketch2dPrimitive::Sector`. The user specifies center,
/// radius, start_angle (rad), and sweep_angle (rad).
#[derive(FeatureSchema)]
#[schema(type = "sketch_arc")]
pub struct SketchArcDef {
    pub center: [f32; 2],
    pub radius: f32,
    pub start_angle: f32,
    pub sweep_angle: f32,
}

pub struct SketchArcTool;

impl SketchToolKind for SketchArcTool {
    fn type_name(&self) -> &'static str {
        SketchArcDef::TYPE_NAME
    }
    fn schema_json(&self) -> Value {
        SketchArcDef::schema_json()
    }
    fn apply(
        &self,
        state: &mut SketchState,
        _id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = SketchArcDef::parse(def)?;
        state.add_point(Point2::new(d.center[0] as f64, d.center[1] as f64));
        Ok(())
    }
}

/// `sketch_ellipse` — a closed ellipse (DL-49 Option B). Ellipses
/// are first-class CAD primitives but not represented exactly: at
/// construction time they decompose into a closed `Path` of
/// tangent-continuous quadratic-Bézier segments (see
/// [`ellipse_quad_beziers`]). Accuracy is the `segments` knob
/// (`0` = auto from a 10 µm tolerance). G1-grouping collapses the
/// whole loop into a single B-Rep edge.
#[derive(FeatureSchema)]
#[schema(type = "sketch_ellipse")]
pub struct SketchEllipseDef {
    pub center: [f32; 2],
    pub radius_x: f32,
    pub radius_y: f32,
    /// CCW rotation of the major axis, radians.
    #[param(default = 0.0)]
    pub rotation: f32,
    /// Number of Bézier segments; `0` selects an automatic count
    /// from the build-time tolerance.
    #[param(default = 0.0)]
    pub segments: f32,
}

/// Convert FieldDraft's `(centre, radius_x, radius_y, rotation)`
/// ellipse parameterisation into PlaneGCS' `(focus, semi-minor)` form.
/// The major axis is whichever of `radius_x` / `radius_y` is larger;
/// the focus sits at distance `sqrt(a² − b²)` from the centre along it.
/// A tiny floor keeps the focus distinct from the centre for
/// near-circular ellipses, so the solver never sees a degenerate
/// (zero-distance) focus.
///
/// MUST stay byte-for-byte in sync with `ellipseFocusRadmin` in
/// `web/src/sketchSession.ts` — both sides feed the same PlaneGCS
/// solver and must produce identical focus / radmin values.
fn ellipse_focus_radmin(
    cx: f64,
    cy: f64,
    radius_x: f64,
    radius_y: f64,
    rotation: f64,
) -> ((f64, f64), f64) {
    let a = radius_x.max(radius_y);
    let b = radius_x.min(radius_y);
    let c = (a * a - b * b).max(0.0).sqrt().max(1e-4);
    let major_angle = if radius_x >= radius_y {
        rotation
    } else {
        rotation + std::f64::consts::FRAC_PI_2
    };
    ((cx + c * major_angle.cos(), cy + c * major_angle.sin()), b)
}

pub struct SketchEllipseTool;

impl SketchToolKind for SketchEllipseTool {
    fn type_name(&self) -> &'static str {
        SketchEllipseDef::TYPE_NAME
    }
    fn schema_json(&self) -> Value {
        SketchEllipseDef::schema_json()
    }
    fn apply(
        &self,
        state: &mut SketchState,
        _id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = SketchEllipseDef::parse(def)?;
        // Register the ellipse as a real solver entity so constraints
        // (tangent, point-on) can bind to it. PlaneGCS parameterises an
        // ellipse by centre + one focus + semi-minor radius; the
        // semi-major radius and orientation fall out of the focus
        // position. We allocate centre, focus, then ellipse — in that
        // fixed order, mirrored byte-for-byte by the TS
        // `buildDraftSketchState` so the shared entity-id space stays
        // aligned.
        let cx = d.center[0] as f64;
        let cy = d.center[1] as f64;
        let (focus, radmin) = ellipse_focus_radmin(
            cx,
            cy,
            d.radius_x as f64,
            d.radius_y as f64,
            d.rotation as f64,
        );
        let center = state.add_point(Point2::new(cx, cy));
        let focus1 = state.add_point(Point2::new(focus.0, focus.1));
        state.add_ellipse(center, focus1, radmin);
        Ok(())
    }
}

/// `sketch_slot` — a centre-to-centre rounded slot (stadium). Two
/// parallel straight edges capped by two semicircles of radius
/// `radius`, drawn by dragging end-centre A → end-centre B. Lowers
/// to a closed `Sketch2dPrimitive::Path` of 2 line + 2 arc segments
/// (mirrors how `sketch_ellipse` lowers to a closed Path), so it can
/// serve as an extrudable region boundary — unlike `sketch_arc`,
/// which lowers to a FILLED `Sector`.
#[derive(FeatureSchema)]
#[schema(type = "sketch_slot")]
pub struct SketchSlotDef {
    /// Centre of the first (A) end cap.
    pub center_a: [f32; 2],
    /// Centre of the second (B) end cap.
    pub center_b: [f32; 2],
    /// Slot radius (half-width). Default 8 mm.
    #[param(default = 8.0)]
    pub radius: f32,
}

pub struct SketchSlotTool;

impl SketchToolKind for SketchSlotTool {
    fn type_name(&self) -> &'static str {
        SketchSlotDef::TYPE_NAME
    }
    fn schema_json(&self) -> Value {
        SketchSlotDef::schema_json()
    }
    fn apply(
        &self,
        state: &mut SketchState,
        _id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = SketchSlotDef::parse(def)?;
        // Register the two end-cap centres in a fixed order (A then
        // B) so the TS entity-id mirror stays aligned. The slot
        // outline itself is a Path primitive (not SketchState
        // entities), like the ellipse.
        state.add_point(Point2::new(d.center_a[0] as f64, d.center_a[1] as f64));
        state.add_point(Point2::new(d.center_b[0] as f64, d.center_b[1] as f64));
        Ok(())
    }
}

// Trim is not a kernel tool: it's an interactive editing operation on
// the web side that rewrites the sketch sub-call list in place (a clicked
// curve is split at its nearest intersections and the picked piece is
// dropped), emitting ordinary `sketch_line` / `sketch_arc` calls the
// kernel already understands. See `web/src/sketchTrim.ts`.

/// `sketch_mirror` — mirrors all existing geometric sub-calls
/// across the specified axis ("x" mirrors across the X axis,
/// "y" across Y). V0 implementation: the mirror happens at
/// commit time in `build_sketch2d_from_sub_calls` by
/// duplicating every primitive with flipped coordinates.
#[derive(FeatureSchema)]
#[schema(type = "sketch_mirror")]
pub struct SketchMirrorDef {
    /// 0 = mirror across X axis (flip Y), 1 = across Y (flip X).
    #[param(default = 0.0)]
    pub axis: f32,
}

pub struct SketchMirrorTool;

impl SketchToolKind for SketchMirrorTool {
    fn type_name(&self) -> &'static str {
        SketchMirrorDef::TYPE_NAME
    }
    fn schema_json(&self) -> Value {
        SketchMirrorDef::schema_json()
    }
    fn apply(
        &self,
        _state: &mut SketchState,
        _id: &FeatureId,
        _def: &Value,
    ) -> Result<(), FeatureError> {
        Ok(())
    }
}

/// `sketch_offset` — creates an offset copy of the sketch
/// profile at a given distance. V0: expands/contracts by
/// adjusting the SDF at build time (sdf − offset_distance).
#[derive(FeatureSchema)]
#[schema(type = "sketch_offset")]
pub struct SketchOffsetDef {
    /// Offset distance in mm. Positive = outward, negative = inward.
    pub distance: f32,
}

pub struct SketchOffsetTool;

impl SketchToolKind for SketchOffsetTool {
    fn type_name(&self) -> &'static str {
        SketchOffsetDef::TYPE_NAME
    }
    fn schema_json(&self) -> Value {
        SketchOffsetDef::schema_json()
    }
    fn apply(
        &self,
        _state: &mut SketchState,
        _id: &FeatureId,
        _def: &Value,
    ) -> Result<(), FeatureError> {
        Ok(())
    }
}

/// `sketch_project` — an ASSOCIATIVE projection of an external B-Rep edge
/// onto the sketch plane (ticket jcb7).
///
/// Unlike the old "adopt a faint guide as a frozen `sketch_line`" approach,
/// this sub-call stores the *query* that names the source edge
/// (`edge: qEdge(hash)`), not just a snapshot of its coordinates. The cached
/// `start`/`end` 2D endpoints are a **persistence fallback** only: on every
/// regeneration [`SketchFeature::execute`] re-resolves the edge against the
/// live B-Rep, projects its sampled endpoints onto the sketch frame, and
/// overwrites these fields with the current coordinates — so the projected
/// line FOLLOWS the body when a parameter upstream changes.
///
/// The result is a plain line entity in the [`SketchState`] (two points + a
/// line, allocated exactly like [`SketchLineTool`] so the TS entity-id mirror
/// stays in lockstep) that the constraint solver can reference. `construction`
/// defaults to `true` because a projected reference is, by CAD convention,
/// construction geometry: visible, snappable, constrainable, but excluded
/// from the extrudable profile.
#[derive(FeatureSchema)]
#[schema(type = "sketch_project")]
pub struct SketchProjectDef {
    /// The source B-Rep edge, as a query (`qEdge(<id_hash>)`). Re-resolved
    /// every regen so the projection tracks the body.
    pub edge: Query,
    /// Cached 2D start endpoint in sketch coordinates — the live re-resolution
    /// in `execute` overwrites this; it only survives as a fallback when the
    /// edge no longer resolves (e.g. the source feature was deleted).
    pub start: [f32; 2],
    /// Cached 2D end endpoint in sketch coordinates (see `start`).
    pub end: [f32; 2],
    /// `true` ⇒ reference/construction geometry (default), excluded from the
    /// extrudable profile.
    #[param(default = true)]
    pub construction: bool,
}

pub struct SketchProjectTool;

impl SketchToolKind for SketchProjectTool {
    fn type_name(&self) -> &'static str {
        SketchProjectDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        SketchProjectDef::schema_json()
    }

    fn apply(
        &self,
        state: &mut SketchState,
        _id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        // The `start`/`end` here are already the live-resolved coordinates:
        // `SketchFeature::execute` rewrites them before `apply_all` runs
        // (`resolve_projected_sub_calls`). We just materialise the line — same
        // entity allocation as `SketchLineTool` so the TS mirror matches.
        let d = SketchProjectDef::parse(def)?;
        let start = state.add_point(Point2::new(d.start[0] as f64, d.start[1] as f64));
        let end = state.add_point(Point2::new(d.end[0] as f64, d.end[1] as f64));
        state.add_line(start, end);
        Ok(())
    }
}

/// `sketch_polygon` — closed polygon from a vertex list.
/// Lowers directly to a `Sketch2dPrimitive::Polygon` region
/// that the extrude/revolve can consume. Requires at least
/// 3 vertices.
#[derive(FeatureSchema)]
#[schema(type = "sketch_polygon")]
pub struct SketchPolygonDef {
    /// Flat array of 2D vertices: `[x0, y0, x1, y1, …]`.
    /// Parsed into `Vec<[f32; 2]>` at apply time.
    pub vertices: Vec<f32>,
}

pub struct SketchPolygonTool;

impl SketchToolKind for SketchPolygonTool {
    fn type_name(&self) -> &'static str {
        SketchPolygonDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        SketchPolygonDef::schema_json()
    }

    fn apply(
        &self,
        state: &mut SketchState,
        _id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = SketchPolygonDef::parse(def)?;
        if d.vertices.len() < 6 {
            return Err(FeatureError::InvalidParameter {
                feature: SketchPolygonDef::TYPE_NAME,
                name: "vertices",
                message: "need at least 3 vertices (6 floats)".to_string(),
            });
        }
        // Add each vertex as a point entity.
        for chunk in d.vertices.chunks(2) {
            if chunk.len() == 2 {
                state.add_point(Point2::new(chunk[0] as f64, chunk[1] as f64));
            }
        }
        Ok(())
    }
}

/// `sketch_spline` — a smooth spline through / governed by a list of
/// points. Like [`SketchPolygonTool`] it registers one point entity
/// per point (so grips / selection / the TS id mirror align). The
/// smooth curve itself is NOT a SketchState entity: at build time it
/// lowers to a `Sketch2dPrimitive::Path` of quadratic Béziers.
///
/// Two modes (no backward-compat — `mode` is required-with-default):
///
/// * **`Interpolate`** (`SplineMode::Interpolate`) — a Catmull-Rom
///   spline that passes through every point (see
///   [`catmull_rom_quad_beziers`]).
/// * **`Control`** (`SplineMode::Control`) — a *real* cubic
///   (degree-3) B-spline whose points are the **control polygon**.
///   For OPEN curves a clamped-uniform knot vector makes the curve
///   pass through the first/last control points; for CLOSED curves a
///   periodic/uniform knot vector wraps it into a loop. The B-spline
///   is lowered EXACTLY (no point-sampling of the whole curve): each
///   knot span is converted to one cubic Bézier via Boehm knot
///   insertion, then each cubic is subdivided to the deviation
///   tolerance [`SPLINE_LOWER_TOL`] into [`SketchSegment::QuadBezier`]
///   segments (see [`bspline_quad_beziers`]).
///
/// `closed` selects a wrap-around loop (extrudable region) versus an
/// open reference curve. Requires ≥ 2 control points.
#[derive(FeatureSchema)]
#[schema(type = "sketch_spline")]
pub struct SketchSplineDef {
    /// Flat array of 2D points: `[x0, y0, x1, y1, …]`. In
    /// `Interpolate` mode the curve passes through them; in `Control`
    /// mode they are the B-spline control polygon.
    pub points: Vec<f32>,
    /// `true` ⇒ the spline wraps into a closed loop; `false` ⇒ an
    /// open curve.
    #[param(default = false)]
    pub closed: bool,
    /// Which spline family to build from `points`.
    #[param(default = SplineMode::Interpolate)]
    pub mode: SplineMode,
}

/// Spline family selector for [`SketchSplineDef`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplineMode {
    /// Catmull-Rom interpolating spline (passes through the points).
    #[default]
    Interpolate,
    /// Cubic (degree-3) B-spline with the points as control polygon.
    Control,
}

pub struct SketchSplineTool;

impl SketchToolKind for SketchSplineTool {
    fn type_name(&self) -> &'static str {
        SketchSplineDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        SketchSplineDef::schema_json()
    }

    fn apply(
        &self,
        state: &mut SketchState,
        _id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = SketchSplineDef::parse(def)?;
        if d.points.len() < 4 {
            return Err(FeatureError::InvalidParameter {
                feature: SketchSplineDef::TYPE_NAME,
                name: "points",
                message: "need at least 2 control points (4 floats)".to_string(),
            });
        }
        // Register each control point as a point entity (mirrors
        // SketchPolygonTool), so the TS entity-id mirror aligns. The
        // smooth curve is a Path primitive built at lower time.
        let mut cp_ids: Vec<EntityId> = Vec::with_capacity(d.points.len() / 2);
        for chunk in d.points.chunks(2) {
            if chunk.len() == 2 {
                cp_ids.push(state.add_point(Point2::new(chunk[0] as f64, chunk[1] as f64)));
            }
        }

        // F23 endpoint tangent ("G1") support: for an OPEN spline the
        // curve's endpoint tangent runs PARALLEL to the first/last
        // control-polygon edge in BOTH modes:
        //   - Clamped cubic B-spline (control mode): the tangent at the
        //     clamped start endpoint runs exactly along cp[0]→cp[1], and
        //     at the end endpoint along cp[N-2]→cp[N-1].
        //   - Catmull-Rom (interpolation mode) with the phantom-
        //     duplication end condition (p_{-1}=p0, p_n=p_{n-1}):
        //     P'(0)=0.5·(p1−p0) and P'(1)=0.5·(p[n-1]−p[n-2]) — i.e.
        //     parallel to the same first/last chord. The 0.5 magnitude
        //     is irrelevant for G1 tangency, which is direction-only
        //     (parallel/perpendicular/angle/tangent), so the leg is an
        //     EXACT G1 tangent handle, identical in form to control
        //     mode.
        // We expose those endpoint "legs" as ordinary `line` entities so
        // ALL existing line constraints (parallel/perpendicular/angle/
        // tangent/horizontal/vertical/length) apply for free — no new
        // constraint type needed.
        //
        // Scope (MUST match the TS mirror in web/src/sketchSession.ts
        // exactly): legs whenever !closed (ANY mode). Closed: periodic,
        // no free endpoints → no legs, in both modes.
        //
        // Allocation ORDER (critical, the web mirror depends on it):
        // first the N control-point entities (above), THEN the start
        // leg, THEN (if N>=3) the end leg. The legs reference existing
        // points so they add NO new DOF, and they live ONLY in the
        // constraint/entity world — `build_sketch2d_from_sub_calls`
        // lowers the spline from the sub-call params, not from these
        // entities, so the extrude profile is unaffected.
        if !d.closed {
            let n = cp_ids.len();
            // START leg = Line(cp[0], cp[1]).
            state.add_line(cp_ids[0], cp_ids[1]);
            // END leg = Line(cp[N-2], cp[N-1]) — only distinct when N>=3
            // (for N==2 start and end coincide → exactly one leg).
            if n >= 3 {
                state.add_line(cp_ids[n - 2], cp_ids[n - 1]);
            }
        }

        // W1: for an OPEN control-polygon spline, allocate a real
        // B-spline solver entity whose poles ARE the control points, so
        // point-on-spline constraints can bind to the curve itself (the
        // endpoint legs above only constrain the tangents). Degree is
        // clamped exactly like the drawn curve in `bspline_quad_beziers`
        // (`3.min(n-1)`), and the implied clamped-uniform knot vector
        // therefore matches the geometry the user sees.
        //
        // Scope: control mode only — interpolating (Catmull-Rom) splines
        // pass THROUGH their points rather than treating them as poles,
        // and periodic (closed) splines use a different knot convention;
        // both keep only the endpoint-leg G1 handling above.
        //
        // Allocation ORDER (the web mirror in buildDraftSketchState MUST
        // match): control points, then the leg line(s), then the
        // B-spline entity LAST so existing entity ids are unchanged.
        if d.mode == SplineMode::Control && !d.closed {
            let n = cp_ids.len();
            let degree = 3u32.min((n - 1) as u32);
            state.add_bspline(cp_ids.clone(), degree, false);
        }
        Ok(())
    }
}

/// One segment of a [`SketchPathDef`] contour. A `Line` walks
/// straight to `to`; a `Quad` is a quadratic Bézier with control
/// point `ctrl` ending at `to`. The contour's start point lives on
/// the contour, so the previous segment's end (or the contour
/// start, for the first segment) is the implicit `p0`.
///
/// This is a tagged enum serialised as `{ "kind": "line", "to": …}`
/// / `{ "kind": "quad", "ctrl": …, "to": … }` — the same wire shape
/// the TS importer (`web/src/importVector.ts`) emits. Cubic Béziers
/// are deliberately absent: any cubic upstream must be subdivided
/// into quads at construction time (mirrors [`SketchSegment`]).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PathSegmentDef {
    /// Straight line to `to` from the previous point.
    Line { to: [f32; 2] },
    /// Quadratic Bézier through control `ctrl`, ending at `to`.
    Quad { ctrl: [f32; 2], to: [f32; 2] },
}

/// One contour of a [`SketchPathDef`]: a chain of [`PathSegmentDef`]
/// segments starting at `start`. `closed` marks the contour as a
/// fillable loop (an implicit closing segment back to `start` is
/// added at lower time if the last segment doesn't already land
/// there). Open contours are non-region reference curves.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PathContourDef {
    /// First point of the contour (implicit `p0` of segment 0).
    pub start: [f32; 2],
    /// Ordered segments walking from `start`.
    pub segments: Vec<PathSegmentDef>,
    /// `true` ⇒ a closed, fillable loop; `false` ⇒ an open curve.
    #[serde(default)]
    pub closed: bool,
}

/// `sketch_path` — an explicit path of arbitrary line + quadratic
/// Bézier segments, grouped into one or more contours with
/// **even-odd hole semantics**. This is the general region
/// primitive the SVG importer (F22-full), filled glyphs (F24) and
/// the B-spline tool (F23) all lower onto.
///
/// Lowering ([`build_sketch2d_from_sub_calls`]):
///
/// * Each CLOSED contour becomes a `Sketch2dPrimitive::Path` of
///   `SketchSegment::Line` / `QuadBezier` (a `Line { to }` →
///   `Line { start: prev, end: to }`; a `Quad { ctrl, to }` →
///   `QuadBezier { p0: prev, p1: ctrl, p2: to }`).
/// * Multiple contours of ONE `sketch_path` combine by nesting
///   depth: containment (point-in-contour at build time) decides
///   which contour sits inside which. Even-depth contours (0, 2, …)
///   are **added** (`Union`); odd-depth contours (1, 3, …) are
///   **subtracted** (`Difference`) from their immediate even-depth
///   parent — so a ring inside an outer becomes a hole, and an
///   island inside that hole becomes solid again.
/// * A lone OPEN contour produces no region (skipped for filling),
///   because a profile must be closed; open contours exist for
///   non-region use (reference curves, future B-spline previews).
///
/// Like the polygon/spline tools it registers point entities so the
/// TS entity-id mirror stays aligned — one per on-/off-curve vertex
/// (contour start, then per segment ctrl?/to) so every vertex is
/// hit-testable and grip-draggable.
#[derive(FeatureSchema)]
#[schema(type = "sketch_path")]
pub struct SketchPathDef {
    /// One or more contours. Multiple contours nest by even-odd
    /// containment to form holes.
    pub contours: Vec<PathContourDef>,
}

pub struct SketchPathTool;

impl SketchToolKind for SketchPathTool {
    fn type_name(&self) -> &'static str {
        SketchPathDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        SketchPathDef::schema_json()
    }

    fn apply(
        &self,
        state: &mut SketchState,
        _id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = SketchPathDef::parse(def)?;
        if d.contours.is_empty() {
            return Err(FeatureError::InvalidParameter {
                feature: SketchPathDef::TYPE_NAME,
                name: "contours",
                message: "need at least one contour".to_string(),
            });
        }
        // Register a point entity per on-/off-curve vertex so the TS
        // entity-id mirror aligns AND the vertices become hit-testable /
        // grip-draggable (like SketchSplineTool's control points). The
        // path GEOMETRY (extrude/region) is still lowered from the raw
        // sub-call params via `build_sketch2d_from_sub_calls`, NOT from
        // these entities — so adding vertices does NOT change the profile.
        //
        // CANONICAL allocation ORDER (the web mirrors in
        // web/src/sketchSession.ts `buildDraftSketchState` and
        // web/src/solver/replaySolve.ts `rebuildSubCallsFromSolved` MUST
        // match this byte-for-byte, or later sub-calls' entity-id refs
        // corrupt). For each contour, in contour order:
        //   1. the contour `start` point
        //   2. then for each segment in order:
        //        - Quad → allocate `ctrl`, THEN `to`
        //        - Line → allocate `to`
        // The implicit closing edge of a closed contour adds NO entity
        // (it returns to the existing `start`).
        for c in &d.contours {
            state.add_point(Point2::new(c.start[0] as f64, c.start[1] as f64));
            for seg in &c.segments {
                match seg {
                    PathSegmentDef::Quad { ctrl, to } => {
                        state.add_point(Point2::new(ctrl[0] as f64, ctrl[1] as f64));
                        state.add_point(Point2::new(to[0] as f64, to[1] as f64));
                    }
                    PathSegmentDef::Line { to } => {
                        state.add_point(Point2::new(to[0] as f64, to[1] as f64));
                    }
                }
            }
        }
        Ok(())
    }
}

/// Lower one [`PathContourDef`] into a list of [`SketchSegment`]s.
/// Returns `None` when the contour has no segments. When `closed`,
/// an implicit closing `Line` back to `start` is appended unless the
/// last segment already lands on `start`.
fn path_contour_segments(contour: &PathContourDef) -> Option<Vec<SketchSegment>> {
    if contour.segments.is_empty() {
        return None;
    }
    let mut out: Vec<SketchSegment> = Vec::with_capacity(contour.segments.len() + 1);
    let mut prev = contour.start;
    for seg in &contour.segments {
        match seg {
            PathSegmentDef::Line { to } => {
                out.push(SketchSegment::Line {
                    start: prev,
                    end: *to,
                });
                prev = *to;
            }
            PathSegmentDef::Quad { ctrl, to } => {
                out.push(SketchSegment::QuadBezier {
                    p0: prev,
                    p1: *ctrl,
                    p2: *to,
                });
                prev = *to;
            }
        }
    }
    if contour.closed {
        let dx = prev[0] - contour.start[0];
        let dy = prev[1] - contour.start[1];
        if dx * dx + dy * dy > 1e-12 {
            out.push(SketchSegment::Line {
                start: prev,
                end: contour.start,
            });
        }
    }
    Some(out)
}

/// Sample points along a lowered segment chain for point-in-contour
/// containment tests. Curves are flattened (a few points per quad)
/// since containment only needs a representative polygon.
fn contour_polyline(segments: &[SketchSegment]) -> Vec<[f32; 2]> {
    let mut pts: Vec<[f32; 2]> = Vec::new();
    for seg in segments {
        match seg {
            SketchSegment::Line { start, end } => {
                if pts.is_empty() {
                    pts.push(*start);
                }
                pts.push(*end);
            }
            SketchSegment::QuadBezier { .. } | SketchSegment::Arc { .. } => {
                if pts.is_empty() {
                    pts.push(seg.point_at(0.0));
                }
                for k in 1..=6 {
                    pts.push(seg.point_at(k as f32 / 6.0));
                }
            }
        }
    }
    pts
}

/// Sample an OPEN sketch curve as an ordered 2D polyline — the spine for a
/// sweep along a non-closed path. Gathers the path-friendly entities
/// (`sketch_line`, `sketch_spline`) in draw order and flattens them; assumes
/// the user drew them sequentially (each entity continuing the previous).
/// Construction geometry is skipped. Returns `None` for fewer than two points.
pub fn open_path_curve_from_sub_calls(sub_calls: &[FeatureCall]) -> Option<Vec<[f32; 2]>> {
    let mut segments: Vec<SketchSegment> = Vec::new();
    for call in sub_calls {
        if call
            .params
            .get("construction")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            continue;
        }
        match call.type_name.as_str() {
            "sketch_line" => {
                if let Ok(d) = SketchLineDef::parse(&call.params) {
                    segments.push(SketchSegment::Line { start: d.start, end: d.end });
                }
            }
            "sketch_spline" => {
                if let Ok(d) = SketchSplineDef::parse(&call.params) {
                    segments.extend(lower_spline_segments(&d));
                }
            }
            _ => {}
        }
    }
    if segments.is_empty() {
        return None;
    }
    let pts = contour_polyline(&segments);
    if pts.len() < 2 {
        None
    } else {
        Some(pts)
    }
}

/// Even-odd point-in-polygon test (ray casting).
fn point_in_polyline(pt: [f32; 2], poly: &[[f32; 2]]) -> bool {
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let yi = poly[i][1];
        let yj = poly[j][1];
        if ((yi > pt[1]) != (yj > pt[1]))
            && (pt[0] < (poly[j][0] - poly[i][0]) * (pt[1] - yi) / (yj - yi) + poly[i][0])
        {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Lower a whole `sketch_path` def into one [`Sketch2d`] tree with
/// even-odd hole semantics. Returns `None` when no closed contour
/// yields a region. See [`SketchPathDef`] for the depth rules.
fn build_sketch_path(def: &SketchPathDef) -> Option<Sketch2d> {
    // Lower each contour; keep only closed contours with ≥1 segment
    // for region building (open contours are reference-only).
    struct Lowered {
        segments: Vec<SketchSegment>,
        poly: Vec<[f32; 2]>,
        rep: [f32; 2], // representative interior-ish point for containment
    }
    let mut lowered: Vec<Lowered> = Vec::new();
    for c in &def.contours {
        if !c.closed {
            continue;
        }
        let Some(segs) = path_contour_segments(c) else {
            continue;
        };
        let poly = contour_polyline(&segs);
        if poly.len() < 3 {
            continue;
        }
        // Representative point: a point just INSIDE this contour near
        // its first vertex. A vertex itself sits ON other contours'
        // boundaries ambiguously and the centroid can fall inside a
        // hole, so neither is robust. Instead nudge the first vertex a
        // hair toward the polygon centroid — that lands strictly
        // inside a simple contour while staying clearly outside any
        // sibling it doesn't enclose. (Containment here only ranks
        // nesting between *different* contours of one path.)
        let mut cx = 0.0f32;
        let mut cy = 0.0f32;
        for p in &poly {
            cx += p[0];
            cy += p[1];
        }
        let cx = cx / poly.len() as f32;
        let cy = cy / poly.len() as f32;
        let v0 = poly[0];
        let rep = [v0[0] + (cx - v0[0]) * 1e-3, v0[1] + (cy - v0[1]) * 1e-3];
        lowered.push(Lowered {
            segments: segs,
            poly,
            rep,
        });
    }
    if lowered.is_empty() {
        return None;
    }

    // Compute nesting depth: depth[i] = number of OTHER contours that
    // contain contour i's representative point.
    let n = lowered.len();
    let mut depth = vec![0usize; n];
    for i in 0..n {
        for (j, other) in lowered.iter().enumerate() {
            if i == j {
                continue;
            }
            if point_in_polyline(lowered[i].rep, &other.poly) {
                depth[i] += 1;
            }
        }
    }

    // Each contour → a Path prim node.
    let prim_of = |l: &Lowered| {
        Sketch2d::Prim(Sketch2dPrimitive::Path {
            segments: l.segments.clone(),
            closed: true,
        })
    };

    // Find the immediate parent of each odd-depth contour: the
    // enclosing contour with the greatest depth strictly less than
    // its own (i.e. depth[i]-1).
    let parent_of = |i: usize| -> Option<usize> {
        let target = depth[i].checked_sub(1)?;
        let mut best: Option<usize> = None;
        for j in 0..n {
            if j == i || depth[j] != target {
                continue;
            }
            if point_in_polyline(lowered[i].rep, &lowered[j].poly) {
                best = Some(j);
                break;
            }
        }
        best
    };

    // Build each even-depth contour as a solid, subtracting its
    // direct odd-depth children as holes. Then Union all even-depth
    // results together. Holes that themselves contain a deeper
    // even-depth island are handled because that island is built as
    // its own top-level solid and Union'd back in (even-odd).
    let mut acc: Option<Sketch2d> = None;
    for i in 0..n {
        if depth[i] % 2 != 0 {
            continue; // odd → a hole, handled by its parent
        }
        let mut node = prim_of(&lowered[i]);
        // Subtract direct children (depth+1) that nest in this one.
        for k in 0..n {
            if depth[k] == depth[i] + 1 && parent_of(k) == Some(i) {
                node = Sketch2d::Difference(Box::new(node), Box::new(prim_of(&lowered[k])));
            }
        }
        acc = Some(match acc {
            None => node,
            Some(prev) => Sketch2d::Union(Box::new(prev), Box::new(node)),
        });
    }
    acc
}

/// Lower a Catmull-Rom interpolating spline through `control_pts`
/// into a chain of [`SketchSegment::QuadBezier`] segments that stays
/// within `tol` (model units) of the exact spline.
///
/// Each Catmull-Rom span (`p1`→`p2`, neighbours `p0`/`p3` supplying
/// the tangents `(p2−p0)/2` and `(p3−p1)/2`) IS a cubic Bézier —
/// exactly:
///
///   b0 = p1,  b1 = p1 + (p2−p0)/6,  b2 = p2 − (p3−p1)/6,  b3 = p2
///
/// — so the lowering converts each span exactly and delegates to the
/// error-bounded [`cubic_to_quads`]. No whole-curve sampling: the
/// interpolated control points are hit exactly (span endpoints are
/// preserved through subdivision), the segment count adapts to
/// curvature (a straight-ish span is a single quad), and junctions
/// inherit the spline's C1 tangents exactly.
///
/// * Open splines (`closed == false`) duplicate the first/last
///   control point to synthesise the end tangents.
/// * Closed splines wrap the neighbour indices and append a final
///   span back to the first point.
///
/// Returns an empty vec for fewer than 2 control points.
pub fn catmull_rom_quad_beziers(
    control_pts: &[[f32; 2]],
    closed: bool,
    tol: f64,
) -> Vec<SketchSegment> {
    let n = control_pts.len();
    if n < 2 {
        return Vec::new();
    }
    let span_count = if closed { n } else { n - 1 };
    let mut segs = Vec::with_capacity(span_count * 2);
    for i in 0..span_count {
        let (i0, i1, i2, i3) = if closed {
            ((i + n - 1) % n, i % n, (i + 1) % n, (i + 2) % n)
        } else {
            (
                if i == 0 { 0 } else { i - 1 },
                i,
                i + 1,
                if i + 2 >= n { n - 1 } else { i + 2 },
            )
        };
        let p = |idx: usize| -> [f64; 2] {
            [control_pts[idx][0] as f64, control_pts[idx][1] as f64]
        };
        let p0 = p(i0);
        let p1 = p(i1);
        let p2 = p(i2);
        let p3 = p(i3);
        let b1 = [p1[0] + (p2[0] - p0[0]) / 6.0, p1[1] + (p2[1] - p0[1]) / 6.0];
        let b2 = [p2[0] - (p3[0] - p1[0]) / 6.0, p2[1] - (p3[1] - p1[1]) / 6.0];
        cubic_to_quads(p1, b1, b2, p2, tol, &mut segs);
    }
    segs
}

/// Lower a cubic (degree-3) B-spline with control polygon
/// `control_pts` into a chain of [`SketchSegment::QuadBezier`]
/// segments, EXACTLY (no whole-curve point sampling).
///
/// Knot-vector scheme:
/// * **Open** (`closed == false`): a clamped-uniform knot vector of
///   degree 3 — `[0,0,0,0, 1,2,…, m,m,m,m]`. The clamping makes the
///   curve interpolate the first and last control points, so those
///   are the curve endpoints (and the registered point entities), and
///   a Coincident constraint on them is a true endpoint constraint.
/// * **Closed** (`closed == true`): a periodic/uniform knot vector.
///   The control polygon is wrapped by repeating the first `degree`
///   control points at the end (`P0…P_{n-1}, P0,P1,P2`) over a uniform
///   knot vector, giving a smooth C² wrap-around loop (an extrudable
///   region).
///
/// Exact lowering pipeline:
/// 1. Build the (possibly wrapped) control points + knot vector.
/// 2. Convert each non-empty knot span into ONE cubic Bézier via
///    Boehm's knot-insertion algorithm (insert each interior knot up
///    to multiplicity `degree`, i.e. the Bézier control points of a
///    span are read straight off the fully-refined polygon).
/// 3. Subdivide each cubic Bézier into quadratic Béziers within the
///    deviation tolerance (the kernel `SketchSegment` has no cubic).
///
/// Edge cases (no panic, NaN-free):
/// * `< 2` control points → empty.
/// * exactly `2` → a single straight `Line`.
/// * exactly `3` → a degree-2 curve: one quad through the control
///   triangle (open) / a small wrapped loop (closed).
/// * collinear control points → the cubic collapses to a straight
///   curve (still emitted, no division by zero).
pub fn bspline_quad_beziers(
    control_pts: &[[f64; 2]],
    closed: bool,
    tol: f64,
) -> Vec<SketchSegment> {
    let n = control_pts.len();
    if n < 2 {
        return Vec::new();
    }
    // Two points: a straight line, regardless of mode.
    if n == 2 {
        return vec![SketchSegment::Line {
            start: [control_pts[0][0] as f32, control_pts[0][1] as f32],
            end: [control_pts[1][0] as f32, control_pts[1][1] as f32],
        }];
    }

    // Degree: cubic when we have enough control points, else drop to
    // n-1 so 3 control points give a degree-2 curve, 2 a line.
    let degree = 3usize.min(n - 1);

    // Build the working control polygon + clamped/periodic knots and
    // the valid loop / curve parameter domain [u_lo, u_hi].
    let (poly, knots, u_lo, u_hi): (Vec<[f64; 2]>, Vec<f64>, f64, f64) = if closed {
        // Periodic: wrap the first `degree` control points to the end
        // and use a uniform knot vector over the padded polygon. The
        // closed loop lives on the parameter sub-domain
        // [knots[degree], knots[degree + n]] — the part where all the
        // wrapped basis functions overlap into a seamless C² loop.
        let mut poly = control_pts.to_vec();
        for k in 0..degree {
            poly.push(control_pts[k % n]);
        }
        let m = poly.len() + degree + 1; // knot count
        let knots: Vec<f64> = (0..m).map(|i| i as f64).collect();
        let u_lo = knots[degree];
        let u_hi = knots[degree + n];
        (poly, knots, u_lo, u_hi)
    } else {
        // Clamped-uniform: degree+1 repeats at each end.
        let poly = control_pts.to_vec();
        let interior = poly.len() - degree - 1; // # interior knots
        let mut knots: Vec<f64> = Vec::with_capacity(poly.len() + degree + 1);
        for _ in 0..=degree {
            knots.push(0.0);
        }
        for i in 1..=interior {
            knots.push(i as f64);
        }
        let last = (interior + 1) as f64;
        for _ in 0..=degree {
            knots.push(last);
        }
        (poly, knots, 0.0, last)
    };

    // Decompose the B-spline into per-span Bézier control points via
    // repeated knot insertion (Boehm), restricted to the [u_lo, u_hi]
    // domain (clamping both ends to multiplicity `degree`). The refined
    // polygon then reads off as consecutive Bézier segments of
    // `degree+1` control points sharing endpoints.
    let beziers = bspline_to_beziers(&poly, &knots, degree, u_lo, u_hi);

    // Subdivide each Bézier (degree 2 or 3) into quads.
    let mut segs: Vec<SketchSegment> = Vec::new();
    for bez in &beziers {
        match bez.len() {
            4 => cubic_to_quads(bez[0], bez[1], bez[2], bez[3], tol, &mut segs),
            3 => {
                // Already a quad.
                segs.push(SketchSegment::QuadBezier {
                    p0: [bez[0][0] as f32, bez[0][1] as f32],
                    p1: [bez[1][0] as f32, bez[1][1] as f32],
                    p2: [bez[2][0] as f32, bez[2][1] as f32],
                });
            }
            2 => segs.push(SketchSegment::Line {
                start: [bez[0][0] as f32, bez[0][1] as f32],
                end: [bez[1][0] as f32, bez[1][1] as f32],
            }),
            _ => {}
        }
    }
    segs
}

/// Boehm knot-insertion decomposition restricted to the parameter
/// domain `[u_lo, u_hi]`: turn a B-spline (`poly`, `knots`, `degree`)
/// into a list of per-span Bézier control-point arrays (each of length
/// `degree+1`) covering exactly that domain.
///
/// Method: clamp both domain ends to multiplicity `degree` (so the
/// refined polygon has a control point landing exactly on the curve at
/// `u_lo` and `u_hi`), raise every interior knot inside `(u_lo, u_hi)`
/// to multiplicity `degree`, then read off consecutive Bézier segments
/// starting at the polygon index that corresponds to `u_lo`.
fn bspline_to_beziers(
    poly: &[[f64; 2]],
    knots: &[f64],
    degree: usize,
    u_lo: f64,
    u_hi: f64,
) -> Vec<Vec<[f64; 2]>> {
    let mut poly: Vec<[f64; 2]> = poly.to_vec();
    let mut knots: Vec<f64> = knots.to_vec();

    // Distinct knot values within [u_lo, u_hi] (inclusive of the ends)
    // that must reach multiplicity `degree`.
    let mut vals: Vec<f64> = Vec::new();
    let push_val = |v: f64, vals: &mut Vec<f64>| {
        if !vals.iter().any(|&p| (p - v).abs() < 1e-9) {
            vals.push(v);
        }
    };
    push_val(u_lo, &mut vals);
    for &k in &knots {
        if k > u_lo + 1e-9 && k < u_hi - 1e-9 {
            push_val(k, &mut vals);
        }
    }
    push_val(u_hi, &mut vals);
    vals.sort_by(|a, b| a.partial_cmp(b).unwrap());

    for &val in &vals {
        let mult = knots.iter().filter(|&&k| (k - val).abs() < 1e-9).count();
        for _ in mult..degree {
            insert_knot(&mut poly, &mut knots, degree, val);
        }
    }

    // Find the polygon index whose corresponding knot reaches u_lo:
    // after clamping, the control point at the start of the first
    // in-domain span is the one whose left knot equals u_lo. Count the
    // control points whose knot index is < the first knot == u_lo at
    // full multiplicity. The standard relation: the i-th control point
    // is associated with knots[i+1..=i+degree]; the first span in the
    // domain begins at the control point index = (index of first knot
    // == u_lo) — for a clamped end that is `degree` repeats. We locate
    // the first knot index k0 with knots[k0] == u_lo and knots reaching
    // full multiplicity there; the span start control index is k0.
    let k0 = knots
        .iter()
        .position(|&k| (k - u_lo).abs() < 1e-9)
        .unwrap_or(0);

    // Number of in-domain spans = distinct interior values + 1.
    let span_count = vals.len() - 1;
    let mut out: Vec<Vec<[f64; 2]>> = Vec::with_capacity(span_count);
    for s in 0..span_count {
        let start = k0 + s * degree;
        let mut seg: Vec<[f64; 2]> = Vec::with_capacity(degree + 1);
        for k in 0..=degree {
            let idx = (start + k).min(poly.len() - 1);
            seg.push(poly[idx]);
        }
        out.push(seg);
    }
    out
}

/// Insert knot value `u` once into (`poly`, `knots`) for a B-spline of
/// the given `degree` (Boehm's algorithm). Mutates both in place.
fn insert_knot(poly: &mut Vec<[f64; 2]>, knots: &mut Vec<f64>, degree: usize, u: f64) {
    // Find the span index k such that knots[k] <= u < knots[k+1].
    // Clamp at poly.len()-1 so the affected control indices
    // (k-degree+1)..=k stay in range (Boehm requires k ≤ n-1).
    let mut k = degree;
    while k + 1 < knots.len() && knots[k + 1] <= u && k + 1 <= poly.len() - 1 {
        k += 1;
    }
    // New control points P'_i for i in (k-degree+1)..=k are blended.
    let mut new_poly: Vec<[f64; 2]> = Vec::with_capacity(poly.len() + 1);
    for i in 0..=(k - degree) {
        new_poly.push(poly[i]);
    }
    for i in (k - degree + 1)..=k {
        let denom = knots[i + degree] - knots[i];
        let a = if denom.abs() < 1e-12 {
            0.0
        } else {
            (u - knots[i]) / denom
        };
        let prev = poly[i - 1];
        let cur = poly[i];
        new_poly.push([
            (1.0 - a) * prev[0] + a * cur[0],
            (1.0 - a) * prev[1] + a * cur[1],
        ]);
    }
    for i in k..poly.len() {
        new_poly.push(poly[i]);
    }
    *poly = new_poly;
    knots.insert(k + 1, u);
}

/// Subdivide a cubic Bézier (`p0,p1,p2,p3`) into the SMALLEST uniform
/// chain of [`SketchSegment::QuadBezier`] segments that provably
/// deviates from the cubic by at most `tol` (cu2qu-style: try
/// n = 1, 2, 3, … equal-parameter pieces and take the first n whose
/// pieces all pass). Pushes onto `out`.
///
/// Each emitted quad keeps the cubic's exact end-tangent DIRECTIONS
/// (its control point is the intersection of the piece's end
/// tangents), so the chain is exactly G1 at every junction — the path
/// G1-grouping (and with it branch/face identity in the BRep) does
/// not depend on `tol`. Near-parallel tangents (straight or
/// inflection-carrying pieces) fall back to the "midpoint" control
/// `q = (3(p1+p2) − p0 − p3)/4`.
///
/// The per-piece error bound is analytic, no sampling: elevate the
/// candidate quad to a cubic and diff the control polygons — the
/// difference curve is `e1·B1(t) + e2·B2(t)` with
/// `e1 = p1 − (p0+2c)/3`, `e2 = p2 − (p3+2c)/3`, and since
/// `B1(t)+B2(t) = 3t(1−t) ≤ 3/4`, the same-parameter deviation is
/// ≤ `¾·max(‖e1‖,‖e2‖)`. It is zero iff the piece IS the quad, and
/// shrinks with the third-order term (~n⁻³), so n stays small.
fn cubic_to_quads(
    p0: [f64; 2],
    p1: [f64; 2],
    p2: [f64; 2],
    p3: [f64; 2],
    tol: f64,
    out: &mut Vec<SketchSegment>,
) {
    // Split at inflection points first: each sub-piece is then convex,
    // so the G1-preserving tangent-intersection control exists for
    // every piece, and the cubic's own tangent at the inflection is
    // shared exactly by both sides — the chain stays exactly G1 even
    // across inflections. Inflections of a Bézier cubic are the roots
    // of cross(B'(t), B''(t)) = cross(b,c)·t² + cross(a,c)·t +
    // cross(a,b) with a = p1−p0, b = p2−2p1+p0, c = p3−3p2+3p1−p0.
    let a = [p1[0] - p0[0], p1[1] - p0[1]];
    let b = [p2[0] - 2.0 * p1[0] + p0[0], p2[1] - 2.0 * p1[1] + p0[1]];
    let c = [
        p3[0] - 3.0 * p2[0] + 3.0 * p1[0] - p0[0],
        p3[1] - 3.0 * p2[1] + 3.0 * p1[1] - p0[1],
    ];
    let cross = |u: [f64; 2], v: [f64; 2]| u[0] * v[1] - u[1] * v[0];
    let (qa, qb, qc) = (cross(b, c), cross(a, c), cross(a, b));
    let mut cuts = [0.0f64, 1.0, 1.0, 1.0];
    let mut n_cuts = 1;
    let push_root = |t: f64, cuts: &mut [f64; 4], n_cuts: &mut usize| {
        if t > 1e-6 && t < 1.0 - 1e-6 && t.is_finite() {
            cuts[*n_cuts] = t;
            *n_cuts += 1;
        }
    };
    let curve_scale = (a[0].abs() + b[0].abs() + c[0].abs())
        .max(a[1].abs() + b[1].abs() + c[1].abs())
        .max(1e-300);
    if qa.abs() > 1e-12 * curve_scale * curve_scale {
        let disc = qb * qb - 4.0 * qa * qc;
        if disc > 0.0 {
            let r = -0.5 * (qb + qb.signum() * disc.sqrt());
            let r = if r == 0.0 { f64::MIN_POSITIVE } else { r };
            let (t1, t2) = (r / qa, qc / r);
            push_root(t1.min(t2), &mut cuts, &mut n_cuts);
            push_root(t1.max(t2), &mut cuts, &mut n_cuts);
        }
    } else if qb.abs() > 1e-12 * curve_scale * curve_scale {
        push_root(-qc / qb, &mut cuts, &mut n_cuts);
    }
    cuts[n_cuts] = 1.0;
    n_cuts += 1;
    for w in 0..n_cuts - 1 {
        let (s0, s1, s2, s3) = cubic_piece(p0, p1, p2, p3, cuts[w], cuts[w + 1]);
        convex_cubic_to_quads(s0, s1, s2, s3, tol, 24, out);
    }
}

/// Adaptive quad subdivision of an inflection-free cubic piece (see
/// [`cubic_to_quads`]): accept a single quad when [`quad_for_cubic`]
/// finds one within `tol`, otherwise bisect — refinement stays local
/// to the curvy / near-inflection parts.
fn convex_cubic_to_quads(
    p0: [f64; 2],
    p1: [f64; 2],
    p2: [f64; 2],
    p3: [f64; 2],
    tol: f64,
    depth: u32,
    out: &mut Vec<SketchSegment>,
) {
    if let Some(ctrl) = quad_for_cubic(p0, p1, p2, p3, tol) {
        out.push(SketchSegment::QuadBezier {
            p0: [p0[0] as f32, p0[1] as f32],
            p1: [ctrl[0] as f32, ctrl[1] as f32],
            p2: [p3[0] as f32, p3[1] as f32],
        });
        return;
    }
    if depth == 0 {
        // Pathological cusp: emit the midpoint-control quad, whose
        // deviation err_mid is microscopic at this depth.
        let q = [
            (3.0 * (p1[0] + p2[0]) - p0[0] - p3[0]) / 4.0,
            (3.0 * (p1[1] + p2[1]) - p0[1] - p3[1]) / 4.0,
        ];
        out.push(SketchSegment::QuadBezier {
            p0: [p0[0] as f32, p0[1] as f32],
            p1: [q[0] as f32, q[1] as f32],
            p2: [p3[0] as f32, p3[1] as f32],
        });
        return;
    }
    let mid = |a: [f64; 2], b: [f64; 2]| [0.5 * (a[0] + b[0]), 0.5 * (a[1] + b[1])];
    let p01 = mid(p0, p1);
    let p12 = mid(p1, p2);
    let p23 = mid(p2, p3);
    let p012 = mid(p01, p12);
    let p123 = mid(p12, p23);
    let p0123 = mid(p012, p123);
    convex_cubic_to_quads(p0, p01, p012, p0123, tol, depth - 1, out);
    convex_cubic_to_quads(p0123, p123, p23, p3, tol, depth - 1, out);
}

/// Extract the sub-cubic of (`p0..p3`) on the parameter interval
/// `[ta, tb]` (two de Casteljau splits).
fn cubic_piece(
    p0: [f64; 2],
    p1: [f64; 2],
    p2: [f64; 2],
    p3: [f64; 2],
    ta: f64,
    tb: f64,
) -> ([f64; 2], [f64; 2], [f64; 2], [f64; 2]) {
    let lerp = |a: [f64; 2], b: [f64; 2], t: f64| {
        [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
    };
    // Split off [ta, 1], then take the leading fraction of it.
    let split_right = |p0: [f64; 2], p1: [f64; 2], p2: [f64; 2], p3: [f64; 2], t: f64| {
        let p01 = lerp(p0, p1, t);
        let p12 = lerp(p1, p2, t);
        let p23 = lerp(p2, p3, t);
        let p012 = lerp(p01, p12, t);
        let p123 = lerp(p12, p23, t);
        let pm = lerp(p012, p123, t);
        (pm, p123, p23, p3)
    };
    let split_left = |p0: [f64; 2], p1: [f64; 2], p2: [f64; 2], p3: [f64; 2], t: f64| {
        let p01 = lerp(p0, p1, t);
        let p12 = lerp(p1, p2, t);
        let p23 = lerp(p2, p3, t);
        let p012 = lerp(p01, p12, t);
        let p123 = lerp(p12, p23, t);
        let pm = lerp(p012, p123, t);
        (p0, p01, p012, pm)
    };
    let (a0, a1, a2, a3) = split_right(p0, p1, p2, p3, ta);
    let local = if (1.0 - ta).abs() < 1e-12 {
        0.0
    } else {
        (tb - ta) / (1.0 - ta)
    };
    split_left(a0, a1, a2, a3, local)
}

/// Junction-angle budget for the midpoint-control fallback (radians,
/// per piece end). Capping the end-tangent rotation caps every
/// lowering-induced junction break at ≤ 2·QUAD_JUNCTION_ANGLE_MAX
/// ≈ 1.15° — comfortably below the G1-grouping threshold
/// (`PATH_G1_COS_THRESHOLD` 0.999 ≙ 2.56°), which therefore keeps
/// separating REAL sketch corners only. Without this cap, a piece at
/// the deviation tolerance can rotate its end tangent by ~3.5·tol/
/// handle — enough to cross the grouping threshold and split faces.
const QUAD_JUNCTION_ANGLE_MAX: f64 = 0.01;

/// Try to approximate a convex (inflection-free) cubic piece by ONE
/// quad within `tol`; returns the quad control point on success.
///
/// Preference order:
/// 1. Tangent-intersection control — preserves both end-tangent
///    directions EXACTLY (G1-exact junctions). Used when the piece
///    turns enough for the intersection to be well-conditioned and
///    the analytic deviation bound `¾·max(‖e1‖,‖e2‖)` (difference of
///    the degree-elevated control polygons, see [`cubic_to_quads`])
///    is within `tol`.
/// 2. Midpoint control `q = (3(p1+p2) − p0 − p3)/4` — exact deviation
///    bound `√3/36·‖d3‖` (cu2qu's bound), accepted when that is
///    within `tol` AND the end-tangent direction error stays inside
///    [`QUAD_JUNCTION_ANGLE_MAX`]. This is the only choice for
///    near-straight pieces, where the tangent intersection is
///    ill-conditioned (it can land arbitrarily far out).
fn quad_for_cubic(
    p0: [f64; 2],
    p1: [f64; 2],
    p2: [f64; 2],
    p3: [f64; 2],
    tol: f64,
) -> Option<[f64; 2]> {
    let d3 = [
        p3[0] - 3.0 * p2[0] + 3.0 * p1[0] - p0[0],
        p3[1] - 3.0 * p2[1] + 3.0 * p1[1] - p0[1],
    ];
    let d3_len = (d3[0] * d3[0] + d3[1] * d3[1]).sqrt();
    let err_mid = (3.0f64.sqrt() / 36.0) * d3_len;
    let t0 = [p1[0] - p0[0], p1[1] - p0[1]];
    let t1 = [p2[0] - p3[0], p2[1] - p3[1]];
    let l0 = (t0[0] * t0[0] + t0[1] * t0[1]).sqrt();
    let l1 = (t1[0] * t1[0] + t1[1] * t1[1]).sqrt();
    let denom = t0[0] * t1[1] - t0[1] * t1[0];
    // 1) Tangent intersection, when the turn is well-conditioned
    //    (|sin| of the angle between the end tangents ≥ ~1.1°).
    if denom.abs() > 0.02 * (l0 * l1).max(1e-300) {
        let s = ((p3[0] - p0[0]) * t1[1] - (p3[1] - p0[1]) * t1[0]) / denom;
        let c = [p0[0] + s * t0[0], p0[1] + s * t0[1]];
        if c[0].is_finite() && c[1].is_finite() {
            let e1 = [
                p1[0] - (p0[0] + 2.0 * c[0]) / 3.0,
                p1[1] - (p0[1] + 2.0 * c[1]) / 3.0,
            ];
            let e2 = [
                p2[0] - (p3[0] + 2.0 * c[0]) / 3.0,
                p2[1] - (p3[1] + 2.0 * c[1]) / 3.0,
            ];
            let m = (e1[0] * e1[0] + e1[1] * e1[1])
                .max(e2[0] * e2[0] + e2[1] * e2[1])
                .sqrt();
            if 0.75 * m <= tol {
                return Some(c);
            }
        }
    }
    // 2) Midpoint control with deviation + junction-angle budget.
    //    The elevated-control differences are e1 = d3/6, e2 = −d3/6
    //    EXACTLY, so the end-tangent rotation is the perpendicular
    //    component: δθ_i ≈ |cross(d3, t_i)| / (6·‖t_i‖²).
    let rot = |t: [f64; 2], l: f64| -> f64 {
        (d3[0] * t[1] - d3[1] * t[0]).abs() / (6.0 * (l * l).max(1e-300))
    };
    let angle_ok =
        rot(t0, l0) <= QUAD_JUNCTION_ANGLE_MAX && rot(t1, l1) <= QUAD_JUNCTION_ANGLE_MAX;
    if err_mid <= tol && angle_ok {
        return Some([
            (3.0 * (p1[0] + p2[0]) - p0[0] - p3[0]) / 4.0,
            (3.0 * (p1[1] + p2[1]) - p0[1] - p3[1]) / 4.0,
        ]);
    }
    None
}

/// `sketch_regular_polygon` — a regular N-gon defined by centre,
/// radius and side count. Expands to N point entities forming a
/// closed loop, exactly like the free [`SketchPolygonTool`]. The
/// vertex math here MUST stay byte-for-byte identical to the
/// TypeScript `buildDraftSketchState` branch, or constraint refs
/// drift between the predicted and regenerated entity ids.
#[derive(FeatureSchema)]
#[schema(type = "sketch_regular_polygon")]
pub struct SketchRegularPolygonDef {
    pub center: [f32; 2],
    pub radius: f32,
    /// Side count as a schema number; rounded and clamped to ≥ 3.
    #[param(default = 6.0)]
    pub sides: f32,
    /// `true` ⇒ vertices lie on the radius circle (inscribed);
    /// `false` ⇒ the given radius is the apothem (circumscribed).
    #[param(default = true)]
    pub inscribed: bool,
}

/// Shared vertex computation for the regular polygon. Returns the
/// N vertices in CCW order starting at the top (angle = π/2), or
/// `None` if the parameters are degenerate (n < 3 or radius < ε).
/// This is the canonical formula the TS side mirrors.
pub fn regular_polygon_vertices(
    center: [f32; 2],
    radius: f32,
    sides: f32,
    inscribed: bool,
) -> Option<Vec<[f32; 2]>> {
    let n = (sides.round() as i64).max(3);
    if !(radius.abs() >= 1e-6) {
        return None;
    }
    let nf = n as f32;
    let eff = if inscribed {
        radius
    } else {
        radius / (std::f32::consts::PI / nf).cos()
    };
    let mut verts = Vec::with_capacity(n as usize);
    for k in 0..n {
        let angle = std::f32::consts::FRAC_PI_2 + std::f32::consts::TAU * (k as f32) / nf;
        verts.push([center[0] + eff * angle.cos(), center[1] + eff * angle.sin()]);
    }
    Some(verts)
}

pub struct SketchRegularPolygonTool;

impl SketchToolKind for SketchRegularPolygonTool {
    fn type_name(&self) -> &'static str {
        SketchRegularPolygonDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        SketchRegularPolygonDef::schema_json()
    }

    fn apply(
        &self,
        state: &mut SketchState,
        _id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = SketchRegularPolygonDef::parse(def)?;
        let verts = regular_polygon_vertices(d.center, d.radius, d.sides, d.inscribed).ok_or(
            FeatureError::InvalidParameter {
                feature: SketchRegularPolygonDef::TYPE_NAME,
                name: "radius",
                message: "need radius ≥ 1e-6 and sides ≥ 3".to_string(),
            },
        )?;
        for v in verts {
            state.add_point(Point2::new(v[0] as f64, v[1] as f64));
        }
        Ok(())
    }
}

// ─── K6b.3d: constraint tools ───────────────────────────────────
//
// K6b.3d teaches the sketch sub-spine to carry constraint calls
// alongside the geometric ones. Each constraint tool references
// sketch-local entity ids via raw `u32` params — the UI predicts
// those ids by replicating the kernel's entity allocation order
// in `buildDraftSketchState`, so the committed calls target the
// same entities the kernel will create at regeneration time.
//
// The constraints land in `SketchState::constraints`. Actual
// solving happens in JavaScript via the K5a `solveSketch`
// wrapper — the kernel just persists the constraint network so
// it survives save / load / edit cycles.

macro_rules! constraint_tool {
    ($def:ident, $tool:ident, |$st:ident, $d:ident| $body:expr) => {
        pub struct $tool;
        impl SketchToolKind for $tool {
            fn type_name(&self) -> &'static str {
                $def::TYPE_NAME
            }
            fn schema_json(&self) -> Value {
                $def::schema_json()
            }
            fn apply(
                &self,
                $st: &mut SketchState,
                _id: &FeatureId,
                def: &Value,
            ) -> Result<(), FeatureError> {
                let $d = $def::parse(def)?;
                $body;
                Ok(())
            }
        }
    };
}

#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_horizontal")]
pub struct SketchHorizontalConstraintDef {
    pub line: u32,
}
constraint_tool!(
    SketchHorizontalConstraintDef,
    SketchHorizontalConstraintTool,
    |state, d| state.add_constraint(|cid: ConstraintId| Constraint::Horizontal {
        id: cid,
        line: EntityId(d.line)
    })
);

#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_vertical")]
pub struct SketchVerticalConstraintDef {
    pub line: u32,
}
constraint_tool!(
    SketchVerticalConstraintDef,
    SketchVerticalConstraintTool,
    |state, d| state.add_constraint(|cid: ConstraintId| Constraint::Vertical {
        id: cid,
        line: EntityId(d.line)
    })
);

#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_coincident")]
pub struct SketchCoincidentConstraintDef {
    pub a: u32,
    pub b: u32,
}
constraint_tool!(
    SketchCoincidentConstraintDef,
    SketchCoincidentConstraintTool,
    |state, d| state.add_constraint(|cid: ConstraintId| Constraint::Coincident {
        id: cid,
        a: EntityId(d.a),
        b: EntityId(d.b)
    })
);

#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_distance")]
pub struct SketchDistanceConstraintDef {
    pub entity_a: u32,
    pub entity_b: u32,
    pub value_mm: f32,
}
constraint_tool!(
    SketchDistanceConstraintDef,
    SketchDistanceConstraintTool,
    |state, d| state.add_constraint(|cid: ConstraintId| Constraint::Distance {
        id: cid,
        a: EntityId(d.entity_a),
        b: EntityId(d.entity_b),
        value_mm: d.value_mm as f64,
        driven: false,
    })
);

#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_angle")]
pub struct SketchAngleConstraintDef {
    pub line_a: u32,
    pub line_b: u32,
    pub value_deg: f32,
}
constraint_tool!(
    SketchAngleConstraintDef,
    SketchAngleConstraintTool,
    |state, d| state.add_constraint(|cid: ConstraintId| Constraint::Angle {
        id: cid,
        a: EntityId(d.line_a),
        b: EntityId(d.line_b),
        value_deg: d.value_deg as f64,
        driven: false,
    })
);

#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_equal")]
pub struct SketchEqualConstraintDef {
    pub entity_a: u32,
    pub entity_b: u32,
}
constraint_tool!(
    SketchEqualConstraintDef,
    SketchEqualConstraintTool,
    |state, d| state.add_constraint(|cid: ConstraintId| Constraint::Equal {
        id: cid,
        a: EntityId(d.entity_a),
        b: EntityId(d.entity_b)
    })
);

#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_concentric")]
pub struct SketchConcentricConstraintDef {
    pub a: u32,
    pub b: u32,
}
constraint_tool!(
    SketchConcentricConstraintDef,
    SketchConcentricConstraintTool,
    |state, d| state.add_constraint(|cid: ConstraintId| Constraint::Concentric {
        id: cid,
        a: EntityId(d.a),
        b: EntityId(d.b)
    })
);

#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_fix")]
pub struct SketchFixConstraintDef {
    pub point: u32,
}
constraint_tool!(
    SketchFixConstraintDef,
    SketchFixConstraintTool,
    |state, d| state.add_constraint(|cid: ConstraintId| Constraint::Fix {
        id: cid,
        point: EntityId(d.point)
    })
);

#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_point_on_object")]
pub struct SketchPointOnObjectConstraintDef {
    pub point: u32,
    pub curve: u32,
}
constraint_tool!(
    SketchPointOnObjectConstraintDef,
    SketchPointOnObjectConstraintTool,
    |state, d| state.add_constraint(|cid: ConstraintId| Constraint::PointOnObject {
        id: cid,
        point: EntityId(d.point),
        curve: EntityId(d.curve)
    })
);

#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_parallel")]
pub struct SketchParallelConstraintDef {
    pub line_a: u32,
    pub line_b: u32,
}
constraint_tool!(
    SketchParallelConstraintDef,
    SketchParallelConstraintTool,
    |state, d| state.add_constraint(|cid| Constraint::Parallel {
        id: cid,
        a: EntityId(d.line_a),
        b: EntityId(d.line_b)
    })
);

#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_collinear")]
pub struct SketchCollinearConstraintDef {
    pub line_a: u32,
    pub line_b: u32,
}
constraint_tool!(
    SketchCollinearConstraintDef,
    SketchCollinearConstraintTool,
    |state, d| state.add_constraint(|cid| Constraint::Collinear {
        id: cid,
        a: EntityId(d.line_a),
        b: EntityId(d.line_b)
    })
);

#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_perpendicular")]
pub struct SketchPerpendicularConstraintDef {
    pub line_a: u32,
    pub line_b: u32,
}
constraint_tool!(
    SketchPerpendicularConstraintDef,
    SketchPerpendicularConstraintTool,
    |state, d| state.add_constraint(|cid| Constraint::Perpendicular {
        id: cid,
        a: EntityId(d.line_a),
        b: EntityId(d.line_b)
    })
);

#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_tangent")]
pub struct SketchTangentConstraintDef {
    pub line: u32,
    pub curve: u32,
}
constraint_tool!(
    SketchTangentConstraintDef,
    SketchTangentConstraintTool,
    |state, d| state.add_constraint(|cid| Constraint::Tangent {
        id: cid,
        line: EntityId(d.line),
        curve: EntityId(d.curve)
    })
);

#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_midpoint")]
pub struct SketchMidpointConstraintDef {
    pub point: u32,
    pub line: u32,
}
constraint_tool!(
    SketchMidpointConstraintDef,
    SketchMidpointConstraintTool,
    |state, d| state.add_constraint(|cid| Constraint::Midpoint {
        id: cid,
        point: EntityId(d.point),
        line: EntityId(d.line)
    })
);

#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_symmetric")]
pub struct SketchSymmetricConstraintDef {
    pub entity_a: u32,
    pub entity_b: u32,
    pub axis: u32,
}
constraint_tool!(
    SketchSymmetricConstraintDef,
    SketchSymmetricConstraintTool,
    |state, d| state.add_constraint(|cid| Constraint::Symmetric {
        id: cid,
        a: EntityId(d.entity_a),
        b: EntityId(d.entity_b),
        axis: EntityId(d.axis)
    })
);

#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_radius")]
pub struct SketchRadiusConstraintDef {
    pub entity: u32,
    pub value_mm: f32,
}
constraint_tool!(
    SketchRadiusConstraintDef,
    SketchRadiusConstraintTool,
    |state, d| state.add_constraint(|cid| Constraint::Radius {
        id: cid,
        entity: EntityId(d.entity),
        value_mm: d.value_mm as f64,
        driven: false
    })
);

#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_diameter")]
pub struct SketchDiameterConstraintDef {
    pub entity: u32,
    pub value_mm: f32,
}
constraint_tool!(
    SketchDiameterConstraintDef,
    SketchDiameterConstraintTool,
    |state, d| state.add_constraint(|cid| Constraint::Diameter {
        id: cid,
        entity: EntityId(d.entity),
        value_mm: d.value_mm as f64,
        driven: false
    })
);

// ─── top-level `sketch` feature ─────────────────────────────────

/// `sketch` — a top-level feature that owns a sub-spine of sketch
/// tools. The definition JSON is parsed by hand because it carries
/// a nested `Vec<FeatureCall>` that the `FeatureSchema` derive
/// does not model (the derive intentionally stays flat).
pub struct SketchFeature {
    tools: SketchToolRegistry,
}

impl Default for SketchFeature {
    fn default() -> Self {
        Self {
            tools: SketchToolRegistry::with_builtins(),
        }
    }
}

impl SketchFeature {
    pub const TYPE_NAME: &'static str = "sketch";

    pub fn new() -> Self {
        Self::default()
    }

    /// Schema JSON that the K6b.2 outliner can hand to the generic
    /// FeatureDialog. K6b.1 emits a minimal shape: a `query`
    /// parameter for the plane and a `generic` sub_calls param
    /// that falls into the JSON-textarea escape hatch on the TS
    /// side. K6b.2 will specialise the sub_calls display with a
    /// dedicated tree widget.
    pub fn static_schema_json() -> Value {
        serde_json::json!({
            "type": Self::TYPE_NAME,
            "params": [
                {
                    "key": "plane",
                    "name": "Plane",
                    "type": "query",
                    "required": true,
                    "entity_filter": "FACE",
                },
                {
                    "key": "sub_calls",
                    "name": "Sketch tools",
                    "type": "generic",
                    "required": false,
                },
            ],
        })
    }

    pub fn tools(&self) -> &SketchToolRegistry {
        &self.tools
    }
}

/// Parsed view of a sketch feature's definition JSON.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedSketchDef {
    pub plane: Query,
    pub sub_calls: Vec<FeatureCall>,
}

impl ParsedSketchDef {
    pub fn parse(def: &Value) -> Result<Self, FeatureError> {
        let plane_slot = def.get("plane").ok_or(FeatureError::MissingParameter {
            feature: SketchFeature::TYPE_NAME,
            name: "plane",
        })?;
        let plane: Query = serde_json::from_value(plane_slot.clone()).map_err(|e| {
            FeatureError::InvalidParameter {
                feature: SketchFeature::TYPE_NAME,
                name: "plane",
                message: format!("expected a Query string: {e}"),
            }
        })?;

        let sub_calls = match def.get("sub_calls") {
            None | Some(Value::Null) => Vec::new(),
            Some(slot) => {
                let arr = slot
                    .as_array()
                    .ok_or_else(|| FeatureError::InvalidParameter {
                        feature: SketchFeature::TYPE_NAME,
                        name: "sub_calls",
                        message: "expected an array of FeatureCalls".to_string(),
                    })?;
                let mut out = Vec::with_capacity(arr.len());
                for (i, entry) in arr.iter().enumerate() {
                    let call: FeatureCall = serde_json::from_value(entry.clone()).map_err(|e| {
                        FeatureError::InvalidParameter {
                            feature: SketchFeature::TYPE_NAME,
                            name: "sub_calls",
                            message: format!("entry {i} is not a FeatureCall: {e}"),
                        }
                    })?;
                    out.push(call);
                }
                out
            }
        };

        Ok(Self { plane, sub_calls })
    }
}

/// Project a world-space edge polyline onto a sketch frame, returning the
/// 2D `(start, end)` endpoints in the frame's `(u, v)` coordinates.
///
/// A straight edge round-trips exactly; a curved edge degrades to its chord
/// (first→last sample) — the same convention [`Context::resolve_axis_world_segment`]
/// uses for an edge-as-axis. Returns `None` for a degenerate (<2-point)
/// polyline. Pure and frame-only so it is unit-testable without a B-Rep
/// (ticket jcb7).
pub fn project_edge_polyline_to_frame(
    polyline: &[[f32; 3]],
    frame: &SketchFrame,
) -> Option<([f32; 2], [f32; 2])> {
    if polyline.len() < 2 {
        return None;
    }
    let start = frame.world_to_local_2d(polyline[0]);
    let end = frame.world_to_local_2d(polyline[polyline.len() - 1]);
    Some((start, end))
}

/// Walk the sub-calls and, for every `sketch_project`, re-resolve its `edge`
/// query against the live B-Rep and overwrite the cached `start`/`end` params
/// with the current projection onto `frame`. This is what makes a projected
/// edge ASSOCIATIVE: the coordinates the downstream `apply` materialises are
/// always the body's *current* geometry, never a frozen snapshot (ticket jcb7).
///
/// An edge that no longer resolves (source feature deleted, etc.) keeps its
/// cached fallback coordinates — the projection stays put rather than
/// vanishing or erroring the whole sketch.
fn resolve_projected_sub_calls(
    ctx: &mut Context<'_>,
    frame: &SketchFrame,
    sub_calls: &mut [FeatureCall],
) {
    for call in sub_calls.iter_mut() {
        if call.type_name != SketchProjectDef::TYPE_NAME {
            continue;
        }
        let Some(edge_slot) = call.params.get("edge") else {
            continue;
        };
        let Ok(query) = serde_json::from_value::<Query>(edge_slot.clone()) else {
            continue;
        };
        let Ok(expr) = crate::queries::parse_query(query.as_str()) else {
            continue;
        };
        let polylines = ctx.resolve_edge_polylines(&expr);
        let Some(poly) = polylines.iter().find(|p| p.len() >= 2) else {
            continue;
        };
        let Some((start, end)) = project_edge_polyline_to_frame(poly, frame) else {
            continue;
        };
        if let Some(obj) = call.params.as_object_mut() {
            obj.insert("start".to_string(), serde_json::json!([start[0], start[1]]));
            obj.insert("end".to_string(), serde_json::json!([end[0], end[1]]));
        }
    }
}

impl FeatureKind for SketchFeature {
    fn type_name(&self) -> &'static str {
        Self::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        Self::static_schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let parsed = ParsedSketchDef::parse(def)?;

        // C1: resolve the sketch's 3D placement frame from the `plane`
        // query. Three resolution paths, no silent fallback:
        //   * `qCreatedBy(origin_plane_*)` → the well-known datum frame
        //     (`origin_plane_xy` is the identity frame).
        //   * a face-yielding query (`qFace(h)`, or `qCreatedBy`/`qFaceOf`
        //     of a prior feature's face) → that face's plane frame.
        //   * anything else → a hard error.
        let expr = crate::queries::parse_query(parsed.plane.as_str()).map_err(|e| {
            FeatureError::InvalidParameter {
                feature: SketchFeature::TYPE_NAME,
                name: "plane",
                message: format!("could not parse plane query: {e}"),
            }
        })?;
        let frame = ctx
            .resolve_plane_frame(&expr)
            .ok_or(FeatureError::InvalidParameter {
                feature: SketchFeature::TYPE_NAME,
                name: "plane",
                message: "plane query resolved to no datum and no face".to_string(),
            })?;

        // jcb7: associative re-resolution. Before building the sketch state,
        // refresh every `sketch_project` sub-call's cached endpoints from the
        // live B-Rep so the projection tracks the body across edits. The
        // rewritten `sub_calls` flow into both the state and the stashed
        // `SketchData`, so downstream consumers (profile, lowering) see the
        // current coordinates too.
        let mut sub_calls = parsed.sub_calls;
        resolve_projected_sub_calls(ctx, &frame, &mut sub_calls);

        let mut state = SketchState::with_origin();
        self.tools.apply_all(&mut state, &sub_calls)?;
        ctx.sketches.insert(
            id.clone(),
            SketchData {
                state,
                sub_calls,
                frame,
            },
        );
        Ok(())
    }
}

// ─── K7.0: Sketch2d synthesis ───────────────────────────────────
//
// The regen pipeline stashes a [`SketchData`] per sketch feature
// in [`Context::sketches`]. For downstream features (extrude,
// revolve, ...) to consume a sketch as a 2D profile, we need a
// translation from the committed sub-spine into a [`Sketch2d`]
// tree the existing CSG pipeline can lower to a 3D volume.
//
// First cut only handles the *composite* geometric tools that
// carry "this is a closed region" intent: `sketch_rectangle` and
// `sketch_circle`. Free `sketch_point` and `sketch_line` calls
// are ignored — they don't implicitly form a region without an
// inference pass that clusters connected segments into closed
// loops, and that's K7.1 work. Multiple region primitives from
// one sketch are unioned together so a sketch with several
// overlapping shapes lowers to one filled area.

/// Drop closed-Path boundary segments that lie ON the revolve axis
/// (sketch x = 0). The half-profile of a body of revolution is
/// conventionally closed by an edge along the axis; revolving makes
/// that edge interior, not surface — keeping it would leave a phantom
/// zero-distance "wall" along the whole axis (a 2 mm shell then leaves
/// a solid rod standing in the middle, and the weave grows a
/// degenerate radius-0 cylinder face). Dropping the segments keeps the
/// even-odd parity for every sample at r ≥ 0 — a +x ray cast from
/// r ≥ 0 never crosses x = 0 — while the distance magnitude now comes
/// from the real wall.
pub fn strip_revolve_axis_segments(sketch: Sketch2d) -> Sketch2d {
    const AXIS_EPS: f32 = 1e-4;
    fn on_axis(seg: &SketchSegment) -> bool {
        let max_abs_x = match seg {
            SketchSegment::Line { start, end } => start[0].abs().max(end[0].abs()),
            SketchSegment::QuadBezier { p0, p1, p2 } => {
                p0[0].abs().max(p1[0].abs()).max(p2[0].abs())
            }
            // A circular arc can't lie on a straight line.
            SketchSegment::Arc { .. } => return false,
        };
        max_abs_x <= AXIS_EPS
    }
    match sketch {
        Sketch2d::Prim(Sketch2dPrimitive::Path {
            segments,
            closed: true,
        }) => {
            let kept: Vec<SketchSegment> = segments.into_iter().filter(|s| !on_axis(s)).collect();
            Sketch2d::Prim(Sketch2dPrimitive::Path {
                segments: kept,
                closed: true,
            })
        }
        Sketch2d::Union(a, b) => Sketch2d::Union(
            Box::new(strip_revolve_axis_segments(*a)),
            Box::new(strip_revolve_axis_segments(*b)),
        ),
        other => other,
    }
}

/// Lower a spline definition to its analytic segment chain —
/// shared by the open-spline region arm and the loop-inference
/// pass below.
fn lower_spline_segments(d: &SketchSplineDef) -> Vec<SketchSegment> {
    let pts: Vec<[f32; 2]> = d
        .points
        .chunks(2)
        .filter(|c| c.len() == 2)
        .map(|c| [c[0], c[1]])
        .collect();
    if pts.len() < 2 {
        return Vec::new();
    }
    match d.mode {
        SplineMode::Interpolate => catmull_rom_quad_beziers(&pts, d.closed, SPLINE_LOWER_TOL),
        SplineMode::Control => {
            let cp: Vec<[f64; 2]> = pts.iter().map(|p| [p[0] as f64, p[1] as f64]).collect();
            bspline_quad_beziers(&cp, d.closed, SPLINE_LOWER_TOL)
        }
    }
}

/// Max deviation (mm) of the quad-Bézier lowering from the exact
/// spline. The lowered segments ARE the kernel geometry (SDF, mesher,
/// weave, GPU stream all consume them), so this is a true geometric
/// tolerance, not a display setting. 0.02 mm is far below feature
/// scale and weave tolerances while keeping segment counts small —
/// the count scales with curvature and only as tol^(−1/3).
pub const SPLINE_LOWER_TOL: f64 = 0.02;

/// One chainable element for the loop-inference pass: the lowered
/// segments of a free `sketch_line` or open `sketch_spline`, plus the
/// index of the sub-call it came from.
#[derive(Clone)]
struct ChainElement {
    call_index: usize,
    segments: Vec<SketchSegment>,
}

/// Endpoint-coincidence tolerance for loop inference (mm). Snapped
/// geometry is exactly coincident (the solver equalises coordinates),
/// so this only needs to absorb f32 rounding — a visibly open contour
/// must NOT silently close.
const LOOP_JOIN_TOL: f32 = 1e-3;

/// K7.1 loop inference: chain free open elements (lines, open splines)
/// into closed contours by endpoint coincidence. Greedy walk per start
/// element; at a junction of ≥ 3 elements the first match wins (no
/// backtracking — V1). Returns the closed loops (keyed by the call
/// index of their first member, so the caller can splice each loop
/// into the accumulator at the position the user drew it) plus the
/// set of consumed call indices.
/// Split lone straight edges at T-junctions (ticket mdye): where another
/// element's endpoint lands on a `Line`'s INTERIOR — an overshooting edge whose
/// corner is only touched partway along — cut the line there so both halves
/// become endpoint-joinable elements. The overshoot half then peels off as a
/// dangling stub in the 2-core prune inside [`infer_closed_loops`], leaving the
/// real loop closed and extrudable — consistent with the sketch-view fill
/// (`sketchRegions.ts` planarizes the same way). Only single-line elements are
/// split: an overshoot is always a straight edge, so arcs/paths stay intact.
fn split_line_tjunctions(elems: &[ChainElement]) -> Vec<ChainElement> {
    let close = |a: [f32; 2], b: [f32; 2]| {
        (a[0] - b[0]).abs() <= LOOP_JOIN_TOL && (a[1] - b[1]).abs() <= LOOP_JOIN_TOL
    };
    let mut junctions: Vec<[f32; 2]> = Vec::new();
    for e in elems {
        if let (Some(f), Some(l)) = (e.segments.first(), e.segments.last()) {
            junctions.push(f.start_point());
            junctions.push(l.end_point());
        }
    }
    let mut out: Vec<ChainElement> = Vec::new();
    for e in elems {
        if let [SketchSegment::Line { start, end }] = e.segments.as_slice() {
            let (start, end) = (*start, *end);
            let d = [end[0] - start[0], end[1] - start[1]];
            let len2 = d[0] * d[0] + d[1] * d[1];
            if len2 > LOOP_JOIN_TOL * LOOP_JOIN_TOL {
                let len = len2.sqrt();
                let mut cuts: Vec<(f32, [f32; 2])> = Vec::new();
                for &j in &junctions {
                    let t = ((j[0] - start[0]) * d[0] + (j[1] - start[1]) * d[1]) / len2;
                    if t <= 0.0 || t >= 1.0 {
                        continue;
                    }
                    let proj = [start[0] + t * d[0], start[1] + t * d[1]];
                    if !close(j, proj) {
                        continue; // off the line
                    }
                    if t * len <= LOOP_JOIN_TOL || (1.0 - t) * len <= LOOP_JOIN_TOL {
                        continue; // effectively at an endpoint
                    }
                    cuts.push((t, proj));
                }
                if !cuts.is_empty() {
                    cuts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
                    let mut prev = start;
                    for (_, c) in cuts {
                        if close(prev, c) {
                            continue;
                        }
                        out.push(ChainElement {
                            call_index: e.call_index,
                            segments: vec![SketchSegment::Line { start: prev, end: c }],
                        });
                        prev = c;
                    }
                    if !close(prev, end) {
                        out.push(ChainElement {
                            call_index: e.call_index,
                            segments: vec![SketchSegment::Line { start: prev, end }],
                        });
                    }
                    continue;
                }
            }
        }
        out.push(e.clone());
    }
    out
}

fn infer_closed_loops(
    elems: &[ChainElement],
) -> (HashMap<usize, Vec<Vec<SketchSegment>>>, std::collections::HashSet<usize>) {
    // Ticket mdye: split overshooting edges at T-junctions first, so a loop that
    // is only touched partway along another line still closes.
    let elems_owned = split_line_tjunctions(elems);
    let elems = &elems_owned[..];
    let close = |a: [f32; 2], b: [f32; 2]| {
        (a[0] - b[0]).abs() <= LOOP_JOIN_TOL && (a[1] - b[1]).abs() <= LOOP_JOIN_TOL
    };
    let mut loops: HashMap<usize, Vec<Vec<SketchSegment>>> = HashMap::new();
    let mut consumed = std::collections::HashSet::new();
    let mut used = vec![false; elems.len()];

    // Ticket mdye: prune DANGLING stubs before loop-walking. An overhanging
    // line — a "flag pole" hanging off a closed rectangle's corner — leaves one
    // endpoint touched by no other element. Such an edge can never be part of a
    // closed loop, yet the greedy walk below could follow it off the branch
    // vertex into a dead end and then fail to close the real loop. Iteratively
    // retire every element that has a FREE endpoint (the 2-core): a whole chain
    // of stubs peels off end-first, leaving clean loops. A self-closed element
    // (start == end, e.g. a closed spline) is a loop on its own — never pruned.
    let endpoints: Vec<([f32; 2], [f32; 2])> = elems
        .iter()
        .map(|e| match (e.segments.first(), e.segments.last()) {
            (Some(f), Some(l)) => (f.start_point(), l.end_point()),
            _ => ([0.0, 0.0], [0.0, 0.0]),
        })
        .collect();
    loop {
        let mut changed = false;
        for i in 0..elems.len() {
            if used[i] || elems[i].segments.is_empty() {
                continue;
            }
            let (a, b) = endpoints[i];
            if close(a, b) {
                continue; // self-closed element — a loop on its own
            }
            let touches = |p: [f32; 2]| {
                (0..elems.len()).any(|j| {
                    j != i
                        && !used[j]
                        && !elems[j].segments.is_empty()
                        && (close(p, endpoints[j].0) || close(p, endpoints[j].1))
                })
            };
            if !touches(a) || !touches(b) {
                used[i] = true; // retire the stub; never a loop member
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    for start in 0..elems.len() {
        if used[start] || elems[start].segments.is_empty() {
            continue;
        }
        let mut chain: Vec<SketchSegment> = elems[start].segments.clone();
        let mut members = vec![start];
        used[start] = true;
        let chain_start = chain.first().unwrap().start_point();
        let mut chain_end = chain.last().unwrap().end_point();
        while !close(chain_end, chain_start) {
            let mut found = false;
            for (j, elem) in elems.iter().enumerate() {
                if used[j] || elem.segments.is_empty() {
                    continue;
                }
                let s = elem.segments.first().unwrap().start_point();
                let e = elem.segments.last().unwrap().end_point();
                if close(chain_end, s) {
                    chain.extend(elem.segments.iter().cloned());
                    chain_end = e;
                } else if close(chain_end, e) {
                    chain.extend(elem.segments.iter().rev().map(|sg| sg.reversed()));
                    chain_end = s;
                } else {
                    continue;
                }
                used[j] = true;
                members.push(j);
                found = true;
                break;
            }
            if !found {
                break;
            }
        }
        if close(chain_end, chain_start) && chain.len() >= 2 {
            for &m in &members {
                consumed.insert(elems[m].call_index);
            }
            let anchor = members.iter().map(|&m| elems[m].call_index).min().unwrap();
            loops.entry(anchor).or_default().push(chain);
        } else {
            // Open chain: release the members so they can join a loop
            // seeded from a different start element. `start` itself
            // stays used — re-seeding from it would just rebuild the
            // same open chain.
            for &m in &members[1..] {
                used[m] = false;
            }
        }
    }
    (loops, consumed)
}

/// Walk a sketch's committed sub_calls and assemble a
/// [`Sketch2d`] tree of every closed-region primitive they
/// contain. Returns [`Sketch2d::Empty`] when the sub-spine has
/// no region-forming entries — downstream features should treat
/// that as "no profile available" and fail fast.
/// K8.13: variant that takes raw JSON sub-call values
/// (the format stored in FeatureCall params). Parses each
/// into a FeatureCall and delegates.
pub fn build_sketch2d_from_sub_calls_raw(raw: &[serde_json::Value]) -> Sketch2d {
    let calls: Vec<FeatureCall> = raw
        .iter()
        .filter_map(|v| {
            let type_name = v.get("type")?.as_str()?.to_string();
            let id_str = v.get("id")?.as_str().unwrap_or("0");
            let id = FeatureId::from_u32(id_str.parse::<u32>().unwrap_or(0));
            let params = v.get("params").cloned().unwrap_or(serde_json::json!({}));
            Some(FeatureCall::new(type_name, id, params))
        })
        .collect();
    build_sketch2d_from_sub_calls(&calls)
}

/// Grid density for region detection when resolving a profile query
/// (`qRegion`/`qRegions`/`qAllRegions`). Matches `listSketchRegions`' default
/// so the region INDEX the UI emitted (from the kernel's own flood-fill order)
/// addresses the same region here.
pub const PROFILE_REGION_RESOLUTION: u32 = 128;

/// Resolve a profile query against the sketch table to a `(Sketch2d, frame)`,
/// or `None` when it doesn't resolve to a non-empty profile (ticket dhjn).
///
/// Honoured query forms — all addressing one sketch feature:
///   * `qCreatedBy(sketch_id)` — the WHOLE sketch (every closed region).
///   * `qAllRegions(sketch_id)` — same as above, explicit.
///   * `qRegion(sketch_id, i)` — only the i-th connected region.
///   * `qRegions(sketch_id, [i, j, …])` — only the listed regions.
///
/// Region indices come from [`crate::sketch_regions::find_regions`] (its
/// flood-fill order), which is exactly what `listSketchRegions` exposes to the
/// UI — so the index the user picked addresses the same region here. The
/// returned profile is the EXACT CSG of the selected region(s)
/// ([`crate::sketch_regions::restrict_sketch_to_points`]), holes included.
pub fn resolve_profile_query(
    query: &str,
    sketches: &std::collections::HashMap<FeatureId, SketchData>,
) -> Option<(Sketch2d, SketchFrame)> {
    use crate::queries::QueryExpr;
    let expr = crate::queries::parse_query(query).ok()?;
    let (sketch_id, indices): (FeatureId, Option<Vec<u32>>) = match expr {
        QueryExpr::CreatedBy(id) | QueryExpr::AllRegions(id) => (id, None),
        QueryExpr::Region(id, i) => (id, Some(vec![i])),
        QueryExpr::Regions(id, list) => (id, Some(list)),
        _ => return None,
    };
    let sketch_data = sketches.get(&sketch_id)?;
    let full = build_sketch2d_from_sub_calls(&sketch_data.sub_calls);
    if matches!(full, Sketch2d::Empty) {
        return None;
    }
    let sketch = match indices {
        None => full,
        Some(indices) => {
            // Map each requested index to its region's guaranteed-interior
            // point, then restrict the profile to the branches containing them.
            let regions =
                crate::sketch_regions::find_regions(&full, PROFILE_REGION_RESOLUTION);
            let points: Vec<[f32; 2]> = indices
                .iter()
                .filter_map(|&i| regions.iter().find(|r| r.index == i))
                .map(|r| r.interior)
                .collect();
            if points.is_empty() {
                return None;
            }
            let restricted =
                crate::sketch_regions::restrict_sketch_to_points(&full, &points);
            if matches!(restricted, Sketch2d::Empty) {
                return None;
            }
            restricted
        }
    };
    Some((sketch, sketch_data.frame))
}

pub fn build_sketch2d_from_sub_calls(sub_calls: &[FeatureCall]) -> Sketch2d {
    // K7.1 loop inference: free `sketch_line`s and open `sketch_spline`s
    // form no region on their own, but a chain of them whose endpoints
    // coincide (the snap tools guarantee exact coincidence) encloses an
    // area exactly like a polygon/closed-spline does. Gather the
    // chainable elements, stitch closed loops, and splice each loop into
    // the accumulator at the position of its first member so the
    // order-sensitive modifiers (mirror, offset) keep their semantics.
    let mut chainable: Vec<ChainElement> = Vec::new();
    for (idx, call) in sub_calls.iter().enumerate() {
        if call
            .params
            .get("construction")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            continue;
        }
        match call.type_name.as_str() {
            "sketch_line" => {
                if let Ok(d) = SketchLineDef::parse(&call.params) {
                    let len = ((d.end[0] - d.start[0]).powi(2)
                        + (d.end[1] - d.start[1]).powi(2))
                    .sqrt();
                    if len > LOOP_JOIN_TOL {
                        chainable.push(ChainElement {
                            call_index: idx,
                            segments: vec![SketchSegment::Line {
                                start: d.start,
                                end: d.end,
                            }],
                        });
                    }
                }
            }
            "sketch_spline" => {
                if let Ok(d) = SketchSplineDef::parse(&call.params) {
                    if !d.closed {
                        let segments = lower_spline_segments(&d);
                        if !segments.is_empty() {
                            chainable.push(ChainElement {
                                call_index: idx,
                                segments,
                            });
                        }
                    }
                }
            }
            // Ticket 6b41: a `sketch_arc` is an open curve whose two endpoints
            // can close a contour together with lines/splines/other arcs (e.g. a
            // diameter line + a semicircular arc → a half-disc). Offer it to the
            // chainer as a single `Arc` segment so that loop is detected as a
            // closed region. A full-circle arc (|sweep| ≈ 2π) is already closed
            // on its own, so it stays out of the chainer and is lowered to a
            // `Sector` below; likewise a lone open arc that joins no loop falls
            // through to the `Sector` (pie-wedge) lowering via `consumed`.
            "sketch_arc" => {
                if let Ok(d) = SketchArcDef::parse(&call.params) {
                    let nearly_full =
                        (d.sweep_angle.abs() - std::f32::consts::TAU).abs() < 1e-3;
                    if d.radius > 0.0 && d.sweep_angle.abs() > 1e-6 && !nearly_full {
                        chainable.push(ChainElement {
                            call_index: idx,
                            segments: vec![SketchSegment::Arc {
                                center: d.center,
                                radius: d.radius,
                                start_angle: d.start_angle,
                                sweep_angle: d.sweep_angle,
                            }],
                        });
                    }
                }
            }
            _ => {}
        }
    }
    let (mut inferred_loops, consumed) = infer_closed_loops(&chainable);

    let mut acc: Option<Sketch2d> = None;
    for (idx, call) in sub_calls.iter().enumerate() {
        if let Some(loops) = inferred_loops.remove(&idx) {
            for segments in loops {
                let node = Sketch2d::Prim(Sketch2dPrimitive::Path {
                    segments,
                    closed: true,
                });
                acc = Some(match acc {
                    None => node,
                    Some(prev) => Sketch2d::Union(Box::new(prev), Box::new(node)),
                });
            }
        }
        if consumed.contains(&idx) {
            continue;
        }
        // Construction geometry is reference-only: it participates in the
        // constraint solve (the TS side still builds solver entities for
        // it) but never contributes to the extrudable region. The flag
        // is a plain param so it costs nothing in the tool schemas.
        if call
            .params
            .get("construction")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            continue;
        }
        let prim = match call.type_name.as_str() {
            "sketch_rectangle" => {
                let d = match SketchRectangleDef::parse(&call.params) {
                    Ok(d) => d,
                    Err(_) => continue,
                };
                let (min_x, max_x) = sort_pair(d.corner_a[0], d.corner_b[0]);
                let (min_y, max_y) = sort_pair(d.corner_a[1], d.corner_b[1]);
                let half_x = 0.5 * (max_x - min_x);
                let half_y = 0.5 * (max_y - min_y);
                if half_x <= 0.0 || half_y <= 0.0 {
                    continue;
                }
                let center = [0.5 * (min_x + max_x), 0.5 * (min_y + max_y)];
                Some(Sketch2dPrimitive::Rectangle {
                    center,
                    half_extents: [half_x, half_y],
                })
            }
            "sketch_circle" => {
                let d = match SketchCircleDef::parse(&call.params) {
                    Ok(d) => d,
                    Err(_) => continue,
                };
                if d.radius <= 0.0 {
                    continue;
                }
                Some(Sketch2dPrimitive::Circle {
                    center: d.center,
                    radius: d.radius,
                })
            }
            "sketch_arc" => {
                let d = match SketchArcDef::parse(&call.params) {
                    Ok(d) => d,
                    Err(_) => continue,
                };
                if d.radius <= 0.0 || d.sweep_angle.abs() < 1e-6 {
                    continue;
                }
                Some(Sketch2dPrimitive::Sector {
                    center: d.center,
                    radius: d.radius,
                    start_angle: d.start_angle,
                    sweep_angle: d.sweep_angle,
                })
            }
            "sketch_ellipse" => {
                let d = match SketchEllipseDef::parse(&call.params) {
                    Ok(d) => d,
                    Err(_) => continue,
                };
                if d.radius_x <= 0.0 || d.radius_y <= 0.0 {
                    continue;
                }
                // Option B: decompose into a closed Path of quadratic
                // Béziers (G1-continuous ⇒ one B-Rep edge). 10 µm
                // default tolerance; `segments` overrides the count.
                let segs = ellipse_quad_beziers(
                    d.center,
                    d.radius_x,
                    d.radius_y,
                    d.rotation,
                    0.0,
                    std::f32::consts::TAU,
                    d.segments.max(0.0) as u32,
                    0.01,
                );
                if segs.is_empty() {
                    continue;
                }
                Some(Sketch2dPrimitive::Path {
                    segments: segs,
                    closed: true,
                })
            }
            "sketch_slot" => {
                let d = match SketchSlotDef::parse(&call.params) {
                    Ok(d) => d,
                    Err(_) => continue,
                };
                match slot_path_segments(d.center_a, d.center_b, d.radius) {
                    Some(segments) => Some(Sketch2dPrimitive::Path {
                        segments,
                        closed: true,
                    }),
                    None => continue,
                }
            }
            "sketch_polygon" => {
                let d = match SketchPolygonDef::parse(&call.params) {
                    Ok(d) => d,
                    Err(_) => continue,
                };
                let verts: Vec<[f32; 2]> = d
                    .vertices
                    .chunks(2)
                    .filter(|c| c.len() == 2)
                    .map(|c| [c[0], c[1]])
                    .collect();
                if verts.len() < 3 {
                    continue;
                }
                Some(Sketch2dPrimitive::Polygon { vertices: verts })
            }
            "sketch_spline" => {
                // Closed splines (and open ones no loop consumed) lower
                // to a Path; the quad-Bézier fit lives in
                // [`lower_spline_segments`], shared with loop inference.
                let d = match SketchSplineDef::parse(&call.params) {
                    Ok(d) => d,
                    Err(_) => continue,
                };
                let segs = lower_spline_segments(&d);
                if segs.is_empty() {
                    continue;
                }
                Some(Sketch2dPrimitive::Path {
                    segments: segs,
                    closed: d.closed,
                })
            }
            "sketch_path" => {
                // An explicit line+quad path of one or more contours,
                // combined with even-odd hole semantics into a subtree.
                // Union the whole subtree into the accumulator (it may
                // itself contain Difference nodes for holes).
                let d = match SketchPathDef::parse(&call.params) {
                    Ok(d) => d,
                    Err(_) => continue,
                };
                let Some(node) = build_sketch_path(&d) else {
                    continue;
                };
                acc = Some(match acc {
                    None => node,
                    Some(prev) => Sketch2d::Union(Box::new(prev), Box::new(node)),
                });
                continue;
            }
            "sketch_regular_polygon" => {
                let d = match SketchRegularPolygonDef::parse(&call.params) {
                    Ok(d) => d,
                    Err(_) => continue,
                };
                match regular_polygon_vertices(d.center, d.radius, d.sides, d.inscribed) {
                    Some(verts) => Some(Sketch2dPrimitive::Polygon { vertices: verts }),
                    None => continue,
                }
            }
            "sketch_mirror" => {
                // Mirror the accumulated tree so far across an axis.
                let d = match SketchMirrorDef::parse(&call.params) {
                    Ok(d) => d,
                    Err(_) => continue,
                };
                if let Some(ref tree) = acc {
                    let mirrored = mirror_sketch2d(tree, d.axis as u32);
                    acc = Some(Sketch2d::Union(
                        Box::new(acc.take().unwrap()),
                        Box::new(mirrored),
                    ));
                }
                continue;
            }
            "sketch_offset" => {
                // Offset the accumulated profile by inflating its SDF.
                // Positive = grow (outward), negative = shrink (inward).
                // The `Sketch2d::Offset` node carries the distance through
                // to evaluation (`sketch2d.rs`) and SDF compilation
                // (`sdf_compiler.rs`), so the whole profile so far is
                // wrapped in one offset node.
                let d = match SketchOffsetDef::parse(&call.params) {
                    Ok(d) => d,
                    Err(_) => continue,
                };
                if acc.is_some() {
                    acc = Some(Sketch2d::Offset {
                        inner: Box::new(acc.take().unwrap()),
                        distance: d.distance,
                    });
                }
                continue;
            }
            _ => None,
        };
        let Some(prim) = prim else { continue };
        let node = Sketch2d::Prim(prim);
        acc = Some(match acc {
            None => node,
            Some(prev) => Sketch2d::Union(Box::new(prev), Box::new(node)),
        });
    }
    acc.unwrap_or(Sketch2d::Empty)
}

/// Mirror a Sketch2d tree across an axis.
/// axis=0 → mirror across X (flip Y), axis=1 → across Y (flip X).
fn mirror_sketch2d(tree: &Sketch2d, axis: u32) -> Sketch2d {
    match tree {
        Sketch2d::Empty => Sketch2d::Empty,
        Sketch2d::Prim(prim) => Sketch2d::Prim(mirror_prim(prim, axis)),
        Sketch2d::Union(a, b) => Sketch2d::Union(
            Box::new(mirror_sketch2d(a, axis)),
            Box::new(mirror_sketch2d(b, axis)),
        ),
        Sketch2d::Difference(a, b) => Sketch2d::Difference(
            Box::new(mirror_sketch2d(a, axis)),
            Box::new(mirror_sketch2d(b, axis)),
        ),
        Sketch2d::Intersection(a, b) => Sketch2d::Intersection(
            Box::new(mirror_sketch2d(a, axis)),
            Box::new(mirror_sketch2d(b, axis)),
        ),
        Sketch2d::Offset { inner, distance } => Sketch2d::Offset {
            inner: Box::new(mirror_sketch2d(inner, axis)),
            distance: *distance,
        },
    }
}

fn mirror_prim(prim: &Sketch2dPrimitive, axis: u32) -> Sketch2dPrimitive {
    let flip = |p: [f32; 2]| -> [f32; 2] {
        if axis == 0 {
            [p[0], -p[1]]
        } else {
            [-p[0], p[1]]
        }
    };
    match prim {
        Sketch2dPrimitive::Circle { center, radius } => Sketch2dPrimitive::Circle {
            center: flip(*center),
            radius: *radius,
        },
        Sketch2dPrimitive::Rectangle {
            center,
            half_extents,
        } => Sketch2dPrimitive::Rectangle {
            center: flip(*center),
            half_extents: *half_extents,
        },
        Sketch2dPrimitive::Polygon { vertices } => Sketch2dPrimitive::Polygon {
            vertices: vertices.iter().map(|v| flip(*v)).collect(),
        },
        Sketch2dPrimitive::Sector {
            center,
            radius,
            start_angle,
            sweep_angle,
        } => {
            // Mirror the angular range.
            let new_start = if axis == 0 {
                -start_angle - sweep_angle
            } else {
                std::f32::consts::PI - start_angle - sweep_angle
            };
            Sketch2dPrimitive::Sector {
                center: flip(*center),
                radius: *radius,
                start_angle: new_start,
                sweep_angle: *sweep_angle,
            }
        }
        Sketch2dPrimitive::Path { segments, closed } => Sketch2dPrimitive::Path {
            segments: segments.iter().map(|s| mirror_segment(s, axis)).collect(),
            closed: *closed,
        },
    }
}

/// Reflect a single sketch segment about the mirror axis. `axis
/// == 0` flips Y (reflect across the X-axis), `axis == 1` flips X.
/// Reflection reverses orientation, so an `Arc`'s sweep sign flips
/// and its start angle maps accordingly (θ → −θ for axis 0, θ →
/// π−θ for axis 1).
fn mirror_segment(seg: &SketchSegment, axis: u32) -> SketchSegment {
    let flip = |p: [f32; 2]| -> [f32; 2] {
        if axis == 0 {
            [p[0], -p[1]]
        } else {
            [-p[0], p[1]]
        }
    };
    match seg {
        SketchSegment::Line { start, end } => SketchSegment::Line {
            start: flip(*start),
            end: flip(*end),
        },
        SketchSegment::QuadBezier { p0, p1, p2 } => SketchSegment::QuadBezier {
            p0: flip(*p0),
            p1: flip(*p1),
            p2: flip(*p2),
        },
        SketchSegment::Arc {
            center,
            radius,
            start_angle,
            sweep_angle,
        } => {
            let new_start = if axis == 0 {
                -start_angle
            } else {
                std::f32::consts::PI - start_angle
            };
            SketchSegment::Arc {
                center: flip(*center),
                radius: *radius,
                start_angle: new_start,
                sweep_angle: -sweep_angle,
            }
        }
    }
}

fn sort_pair(a: f32, b: f32) -> (f32, f32) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

/// Build the closed stadium boundary for a centre-to-centre slot:
/// two parallel straight edges between the end caps, joined by two
/// outward-bulging semicircles. Returns `None` for a degenerate slot
/// (coincident centres or non-positive radius).
///
/// Winding (consistent CCW for r·n on the +n side first):
/// Line(A+r·n → B+r·n); Arc capping B; Line(B−r·n → A−r·n); Arc
/// capping A. Each cap is a semicircle (`sweep_angle = −π`) starting
/// at its first boundary point's angle so the cap bulges away from
/// the opposite centre — verified by the build test's bounding box.
fn slot_path_segments(
    center_a: [f32; 2],
    center_b: [f32; 2],
    radius: f32,
) -> Option<Vec<SketchSegment>> {
    let ax = center_a[0] as f64;
    let ay = center_a[1] as f64;
    let bx = center_b[0] as f64;
    let by = center_b[1] as f64;
    let r = radius as f64;
    let dx = bx - ax;
    let dy = by - ay;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-6 || r < 1e-6 {
        return None;
    }
    // Unit axis A→B and its left normal n = (−u.y, u.x).
    let ux = dx / len;
    let uy = dy / len;
    let nx = -uy;
    let ny = ux;
    // Four boundary points.
    let a_plus = [(ax + r * nx) as f32, (ay + r * ny) as f32]; // A+r·n
    let b_plus = [(bx + r * nx) as f32, (by + r * ny) as f32]; // B+r·n
    let b_minus = [(bx - r * nx) as f32, (by - r * ny) as f32]; // B−r·n
    let a_minus = [(ax - r * nx) as f32, (ay - r * ny) as f32]; // A−r·n
                                                                // Cap B starts at B+r·n (angle of +n); sweep −π takes it through
                                                                // +u (outside, away from A) to B−r·n.
    let cap_b_start = (ny as f32).atan2(nx as f32);
    // Cap A starts at A−r·n (angle of −n); sweep −π takes it through
    // −u (outside, away from B) to A+r·n.
    let cap_a_start = (-ny as f32).atan2(-nx as f32);
    let pi = std::f32::consts::PI;
    Some(vec![
        SketchSegment::Line {
            start: a_plus,
            end: b_plus,
        },
        SketchSegment::Arc {
            center: [bx as f32, by as f32],
            radius,
            start_angle: cap_b_start,
            sweep_angle: -pi,
        },
        SketchSegment::Line {
            start: b_minus,
            end: a_minus,
        },
        SketchSegment::Arc {
            center: [ax as f32, ay as f32],
            radius,
            start_angle: cap_a_start,
            sweep_angle: -pi,
        },
    ])
}

// ─── tests ──────────────────────────────────────────────────────

#[cfg(test)]
#[path = "sketch_features_tests.rs"]
mod tests;
