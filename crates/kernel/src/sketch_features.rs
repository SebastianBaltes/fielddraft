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
        [
            (m21 - m12) / s,
            (m02 - m20) / s,
            (m10 - m01) / s,
            0.25 * s,
        ]
    } else if m00 > m11 && m00 > m22 {
        let s = (1.0 + m00 - m11 - m22).sqrt() * 2.0;
        [
            0.25 * s,
            (m01 + m10) / s,
            (m02 + m20) / s,
            (m21 - m12) / s,
        ]
    } else if m11 > m22 {
        let s = (1.0 + m11 - m00 - m22).sqrt() * 2.0;
        [
            (m01 + m10) / s,
            0.25 * s,
            (m12 + m21) / s,
            (m02 - m20) / s,
        ]
    } else {
        let s = (1.0 + m22 - m00 - m11).sqrt() * 2.0;
        [
            (m02 + m20) / s,
            (m12 + m21) / s,
            0.25 * s,
            (m10 - m01) / s,
        ]
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
    let c_local = [fp.centroid[0] as f32, fp.centroid[1] as f32, fp.centroid[2] as f32];
    let origin = {
        let p = crate::quat_rotate(placement.rotation, [
            c_local[0] * placement.scale[0],
            c_local[1] * placement.scale[1],
            c_local[2] * placement.scale[2],
        ]);
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
        r.register(Box::new(SketchArcTool));
        r.register(Box::new(SketchEllipseTool));
        r.register(Box::new(SketchMirrorTool));
        r.register(Box::new(SketchOffsetTool));
        r.register(Box::new(SketchHorizontalConstraintTool));
        r.register(Box::new(SketchVerticalConstraintTool));
        r.register(Box::new(SketchCoincidentConstraintTool));
        r.register(Box::new(SketchDistanceConstraintTool));
        r.register(Box::new(SketchAngleConstraintTool));
        r.register(Box::new(SketchEqualConstraintTool));
        r.register(Box::new(SketchParallelConstraintTool));
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
        // Mirror the circle tool: register the centre point so the
        // TS entity-id mirror stays aligned.
        state.add_point(Point2::new(d.center[0] as f64, d.center[1] as f64));
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
            fn type_name(&self) -> &'static str { $def::TYPE_NAME }
            fn schema_json(&self) -> Value { $def::schema_json() }
            fn apply(&self, $st: &mut SketchState, _id: &FeatureId, def: &Value) -> Result<(), FeatureError> {
                let $d = $def::parse(def)?;
                $body;
                Ok(())
            }
        }
    };
}

#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_horizontal")]
pub struct SketchHorizontalConstraintDef { pub line: u32 }
constraint_tool!(SketchHorizontalConstraintDef, SketchHorizontalConstraintTool,
    |state, d| state.add_constraint(|cid: ConstraintId| Constraint::Horizontal { id: cid, line: EntityId(d.line) }));

#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_vertical")]
pub struct SketchVerticalConstraintDef { pub line: u32 }
constraint_tool!(SketchVerticalConstraintDef, SketchVerticalConstraintTool,
    |state, d| state.add_constraint(|cid: ConstraintId| Constraint::Vertical { id: cid, line: EntityId(d.line) }));

#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_coincident")]
pub struct SketchCoincidentConstraintDef { pub a: u32, pub b: u32 }
constraint_tool!(SketchCoincidentConstraintDef, SketchCoincidentConstraintTool,
    |state, d| state.add_constraint(|cid: ConstraintId| Constraint::Coincident { id: cid, a: EntityId(d.a), b: EntityId(d.b) }));

#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_distance")]
pub struct SketchDistanceConstraintDef { pub entity_a: u32, pub entity_b: u32, pub value_mm: f32 }
constraint_tool!(SketchDistanceConstraintDef, SketchDistanceConstraintTool,
    |state, d| state.add_constraint(|cid: ConstraintId| Constraint::Distance {
        id: cid, a: EntityId(d.entity_a), b: EntityId(d.entity_b), value_mm: d.value_mm as f64, driven: false,
    }));

#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_angle")]
pub struct SketchAngleConstraintDef { pub line_a: u32, pub line_b: u32, pub value_deg: f32 }
constraint_tool!(SketchAngleConstraintDef, SketchAngleConstraintTool,
    |state, d| state.add_constraint(|cid: ConstraintId| Constraint::Angle {
        id: cid, a: EntityId(d.line_a), b: EntityId(d.line_b), value_deg: d.value_deg as f64, driven: false,
    }));

#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_equal")]
pub struct SketchEqualConstraintDef { pub entity_a: u32, pub entity_b: u32 }
constraint_tool!(SketchEqualConstraintDef, SketchEqualConstraintTool,
    |state, d| state.add_constraint(|cid: ConstraintId| Constraint::Equal { id: cid, a: EntityId(d.entity_a), b: EntityId(d.entity_b) }));

#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_parallel")]
pub struct SketchParallelConstraintDef { pub line_a: u32, pub line_b: u32 }
constraint_tool!(SketchParallelConstraintDef, SketchParallelConstraintTool,
    |state, d| state.add_constraint(|cid| Constraint::Parallel { id: cid, a: EntityId(d.line_a), b: EntityId(d.line_b) }));

#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_perpendicular")]
pub struct SketchPerpendicularConstraintDef { pub line_a: u32, pub line_b: u32 }
constraint_tool!(SketchPerpendicularConstraintDef, SketchPerpendicularConstraintTool,
    |state, d| state.add_constraint(|cid| Constraint::Perpendicular { id: cid, a: EntityId(d.line_a), b: EntityId(d.line_b) }));

#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_tangent")]
pub struct SketchTangentConstraintDef { pub line: u32, pub curve: u32 }
constraint_tool!(SketchTangentConstraintDef, SketchTangentConstraintTool,
    |state, d| state.add_constraint(|cid| Constraint::Tangent { id: cid, line: EntityId(d.line), curve: EntityId(d.curve) }));

#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_midpoint")]
pub struct SketchMidpointConstraintDef { pub point: u32, pub line: u32 }
constraint_tool!(SketchMidpointConstraintDef, SketchMidpointConstraintTool,
    |state, d| state.add_constraint(|cid| Constraint::Midpoint { id: cid, point: EntityId(d.point), line: EntityId(d.line) }));

#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_symmetric")]
pub struct SketchSymmetricConstraintDef { pub entity_a: u32, pub entity_b: u32, pub axis: u32 }
constraint_tool!(SketchSymmetricConstraintDef, SketchSymmetricConstraintTool,
    |state, d| state.add_constraint(|cid| Constraint::Symmetric { id: cid, a: EntityId(d.entity_a), b: EntityId(d.entity_b), axis: EntityId(d.axis) }));

#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_radius")]
pub struct SketchRadiusConstraintDef { pub entity: u32, pub value_mm: f32 }
constraint_tool!(SketchRadiusConstraintDef, SketchRadiusConstraintTool,
    |state, d| state.add_constraint(|cid| Constraint::Radius { id: cid, entity: EntityId(d.entity), value_mm: d.value_mm as f64, driven: false }));

#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_diameter")]
pub struct SketchDiameterConstraintDef { pub entity: u32, pub value_mm: f32 }
constraint_tool!(SketchDiameterConstraintDef, SketchDiameterConstraintTool,
    |state, d| state.add_constraint(|cid| Constraint::Diameter { id: cid, entity: EntityId(d.entity), value_mm: d.value_mm as f64, driven: false }));

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
                    "entityFilter": "FACE",
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
                let arr = slot.as_array().ok_or_else(|| FeatureError::InvalidParameter {
                    feature: SketchFeature::TYPE_NAME,
                    name: "sub_calls",
                    message: "expected an array of FeatureCalls".to_string(),
                })?;
                let mut out = Vec::with_capacity(arr.len());
                for (i, entry) in arr.iter().enumerate() {
                    let call: FeatureCall =
                        serde_json::from_value(entry.clone()).map_err(|e| {
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
        let frame = ctx.resolve_plane_frame(&expr).ok_or(FeatureError::InvalidParameter {
            feature: SketchFeature::TYPE_NAME,
            name: "plane",
            message: "plane query resolved to no datum and no face".to_string(),
        })?;

        let mut state = SketchState::with_origin();
        self.tools.apply_all(&mut state, &parsed.sub_calls)?;
        ctx.sketches.insert(
            id.clone(),
            SketchData {
                state,
                sub_calls: parsed.sub_calls,
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

/// Walk a sketch's committed sub_calls and assemble a
/// [`Sketch2d`] tree of every closed-region primitive they
/// contain. Returns [`Sketch2d::Empty`] when the sub-spine has
/// no region-forming entries — downstream features should treat
/// that as "no profile available" and fail fast.
/// K8.13: variant that takes raw JSON sub-call values
/// (the format stored in FeatureCall params). Parses each
/// into a FeatureCall and delegates.
pub fn build_sketch2d_from_sub_calls_raw(
    raw: &[serde_json::Value],
) -> Sketch2d {
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

pub fn build_sketch2d_from_sub_calls(sub_calls: &[FeatureCall]) -> Sketch2d {
    let mut acc: Option<Sketch2d> = None;
    for call in sub_calls {
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
    if a <= b { (a, b) } else { (b, a) }
}

// ─── tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    use std::str::FromStr;

    use fielddraft_protocol::{Document, FeatureCall, FeatureId};
    use serde_json::json;

    use crate::features::{Context, FeatureError, FeatureRegistry};

    fn id(n: u32) -> FeatureId {
        FeatureId::from_u32(n)
    }

    #[test]
    fn sketch_point_tool_pushes_a_point() {
        let tools = SketchToolRegistry::with_builtins();
        let mut state = SketchState::with_origin();
        let call = FeatureCall::new("sketch_point", id(1), json!({ "at": [3.0, 4.0] }));
        tools.apply_all(&mut state, std::slice::from_ref(&call)).unwrap();
        // origin + the new point
        assert_eq!(state.entities.len(), 2);
    }

    #[test]
    fn sketch_line_tool_pushes_two_points_and_a_line() {
        let tools = SketchToolRegistry::with_builtins();
        let mut state = SketchState::new(); // no origin — keep the count easy
        let call = FeatureCall::new(
            "sketch_line",
            id(1),
            json!({ "start": [0.0, 0.0], "end": [10.0, 0.0] }),
        );
        tools.apply_all(&mut state, std::slice::from_ref(&call)).unwrap();
        // two points + one line
        assert_eq!(state.entities.len(), 3);
    }

    #[test]
    fn unknown_sketch_tool_type_is_reported() {
        let tools = SketchToolRegistry::with_builtins();
        let mut state = SketchState::new();
        let call = FeatureCall::new("not_a_tool", id(1), json!({}));
        let err = tools.apply_all(&mut state, std::slice::from_ref(&call)).unwrap_err();
        assert_eq!(err, FeatureError::UnknownFeatureType("not_a_tool".to_string()));
    }

    #[test]
    fn sketch_feature_regenerates_sub_spine_into_context_sketches() {
        let mut registry = FeatureRegistry::new();
        registry.register(Box::new(SketchFeature::new()));

        let sketch_id = id(1);
        let call = FeatureCall::new(
            SketchFeature::TYPE_NAME,
            sketch_id.clone(),
            json!({
                "plane": "qCreatedBy(origin_plane_xy)",
                "sub_calls": [
                    {
                        "type": "sketch_point",
                        "id": "100",
                        "params": { "at": [0.0, 0.0] }
                    },
                    {
                        "type": "sketch_line",
                        "id": "101",
                        "params": {
                            "start": [0.0, 0.0],
                            "end": [20.0, 0.0]
                        }
                    }
                ]
            }),
        );

        let mut doc = Document::default();
        let mut ctx = Context::new(&mut doc);
        registry.regenerate(&mut ctx, std::slice::from_ref(&call), None).unwrap();

        let data = ctx
            .sketches
            .get(&sketch_id)
            .expect("sketch should be stashed under its feature id");
        // origin (1) + sketch_point (1) + sketch_line's two points + line (3)
        assert_eq!(data.state.entities.len(), 5);
        // Sub-spine snapshot preserved for downstream profile
        // extractors (K7.0).
        assert_eq!(data.sub_calls.len(), 2);
    }

    #[test]
    fn sketch_feature_rejects_unknown_sub_tool() {
        let mut registry = FeatureRegistry::new();
        registry.register(Box::new(SketchFeature::new()));

        let call = FeatureCall::new(
            SketchFeature::TYPE_NAME,
            id(1),
            json!({
                "plane": "qCreatedBy(origin_plane_xy)",
                "sub_calls": [
                    { "type": "not_a_tool", "id": "100", "params": {} }
                ]
            }),
        );

        let mut doc = Document::default();
        let mut ctx = Context::new(&mut doc);
        let err = registry
            .regenerate(&mut ctx, std::slice::from_ref(&call), None)
            .unwrap_err();
        assert_eq!(err, FeatureError::UnknownFeatureType("not_a_tool".to_string()));
    }

    #[test]
    fn sketch_feature_requires_plane() {
        let mut registry = FeatureRegistry::new();
        registry.register(Box::new(SketchFeature::new()));

        let call = FeatureCall::new(SketchFeature::TYPE_NAME, id(1), json!({}));
        let mut doc = Document::default();
        let mut ctx = Context::new(&mut doc);
        let err = registry
            .regenerate(&mut ctx, std::slice::from_ref(&call), None)
            .unwrap_err();
        assert!(matches!(
            err,
            FeatureError::MissingParameter {
                feature: SketchFeature::TYPE_NAME,
                name: "plane"
            }
        ));
    }

    #[test]
    fn sketch_feature_roundtrips_through_json() {
        let call = FeatureCall::new(
            SketchFeature::TYPE_NAME,
            id(1),
            json!({
                "plane": "qCreatedBy(origin_plane_xy)",
                "sub_calls": [
                    { "type": "sketch_point", "id": "100", "params": { "at": [1.0, 2.0] } }
                ]
            }),
        );
        let wire = serde_json::to_string(&call).unwrap();
        let back: FeatureCall = serde_json::from_str(&wire).unwrap();
        assert_eq!(back, call);
    }

    #[test]
    fn sketch_feature_is_included_in_with_builtins() {
        let registry = FeatureRegistry::with_builtins();
        assert!(registry.get(SketchFeature::TYPE_NAME).is_some());
    }

    #[test]
    fn sketch_registry_type_names_lists_all_builtins() {
        // K6b.3f adds sketch_circle to the built-in set.
        let tools = SketchToolRegistry::with_builtins();
        let mut names = tools.type_names();
        names.sort();
        assert_eq!(
            names,
            vec![
                "sketch_arc".to_string(),
                "sketch_circle".to_string(),
                "sketch_constraint_angle".to_string(),
                "sketch_constraint_coincident".to_string(),
                "sketch_constraint_diameter".to_string(),
                "sketch_constraint_distance".to_string(),
                "sketch_constraint_equal".to_string(),
                "sketch_constraint_horizontal".to_string(),
                "sketch_constraint_midpoint".to_string(),
                "sketch_constraint_parallel".to_string(),
                "sketch_constraint_perpendicular".to_string(),
                "sketch_constraint_radius".to_string(),
                "sketch_constraint_symmetric".to_string(),
                "sketch_constraint_tangent".to_string(),
                "sketch_constraint_vertical".to_string(),
                "sketch_ellipse".to_string(),
                "sketch_line".to_string(),
                "sketch_mirror".to_string(),
                "sketch_offset".to_string(),
                "sketch_point".to_string(),
                "sketch_polygon".to_string(),
                "sketch_rectangle".to_string(),
            ]
        );
    }

    #[test]
    fn sketch_circle_tool_emits_a_center_point_and_a_circle() {
        let tools = SketchToolRegistry::with_builtins();
        let mut state = SketchState::with_origin();
        let call = FeatureCall::new(
            "sketch_circle",
            id(1),
            json!({ "center": [5.0, -2.0], "radius": 7.5 }),
        );
        tools.apply_all(&mut state, std::slice::from_ref(&call)).unwrap();
        let point_count = state
            .entities
            .iter()
            .filter(|e| matches!(e, fielddraft_solver::Entity::Point { .. }))
            .count();
        let circle_count = state
            .entities
            .iter()
            .filter(|e| matches!(e, fielddraft_solver::Entity::Circle { .. }))
            .count();
        assert_eq!(point_count, 1);
        assert_eq!(circle_count, 1);
        assert!(state.constraints.is_empty());
        // Circle radius is preserved.
        let circle = state
            .entities
            .iter()
            .find(|e| matches!(e, fielddraft_solver::Entity::Circle { .. }))
            .unwrap();
        if let fielddraft_solver::Entity::Circle { radius, .. } = circle {
            assert!((radius - 7.5).abs() < 1e-9);
        } else {
            panic!("expected circle entity");
        }
    }

    #[test]
    fn sketch_rectangle_tool_emits_four_points_four_lines_and_four_constraints() {
        let tools = SketchToolRegistry::with_builtins();
        let mut state = SketchState::with_origin();
        let call = FeatureCall::new(
            "sketch_rectangle",
            id(1),
            json!({ "corner_a": [0.0, 0.0], "corner_b": [20.0, 10.0] }),
        );
        tools.apply_all(&mut state, std::slice::from_ref(&call)).unwrap();
        let point_count = state
            .entities
            .iter()
            .filter(|e| matches!(e, fielddraft_solver::Entity::Point { .. }))
            .count();
        let line_count = state
            .entities
            .iter()
            .filter(|e| matches!(e, fielddraft_solver::Entity::Line { .. }))
            .count();
        assert_eq!(point_count, 4);
        assert_eq!(line_count, 4);
        assert_eq!(state.constraints.len(), 4);
        let h = state
            .constraints
            .iter()
            .filter(|c| matches!(c, Constraint::Horizontal { .. }))
            .count();
        let v = state
            .constraints
            .iter()
            .filter(|c| matches!(c, Constraint::Vertical { .. }))
            .count();
        assert_eq!(h, 2);
        assert_eq!(v, 2);
    }

    #[test]
    fn sketch_rectangle_normalises_diagonal_corners() {
        // Drag from top-right to bottom-left should still produce
        // a rectangle whose bl corner is at (min_x, min_y).
        let tools = SketchToolRegistry::with_builtins();
        let mut state = SketchState::with_origin();
        tools
            .apply_all(
                &mut state,
                std::slice::from_ref(&FeatureCall::new(
                    "sketch_rectangle",
                    id(1),
                    json!({ "corner_a": [20.0, 10.0], "corner_b": [0.0, 0.0] }),
                )),
            )
            .unwrap();
        // First allocated entity is bl, which must be at (0, 0).
        let bl = state
            .entities
            .iter()
            .find(|e| matches!(e, fielddraft_solver::Entity::Point { id: fielddraft_solver::EntityId(1), .. }))
            .unwrap();
        if let fielddraft_solver::Entity::Point { at, .. } = bl {
            assert!((at.x - 0.0).abs() < 1e-9);
            assert!((at.y - 0.0).abs() < 1e-9);
        } else {
            panic!("expected point entity");
        }
    }

    #[test]
    fn sketch_horizontal_constraint_tool_pushes_a_horizontal_constraint() {
        let tools = SketchToolRegistry::with_builtins();
        let mut state = SketchState::with_origin();
        // Seed a line so we have an entity id to reference.
        let line_call = FeatureCall::new(
            "sketch_line",
            id(1),
            json!({ "start": [0.0, 0.0], "end": [10.0, 0.5] }),
        );
        let cstr_call = FeatureCall::new(
            "sketch_constraint_horizontal",
            id(2),
            // start-point=1, end-point=2, line=3 under with_origin().
            json!({ "line": 3 }),
        );
        tools
            .apply_all(&mut state, &[line_call, cstr_call])
            .unwrap();
        assert_eq!(state.constraints.len(), 1);
        assert!(matches!(
            state.constraints[0],
            Constraint::Horizontal { line: EntityId(3), .. }
        ));
    }

    #[test]
    fn sketch_vertical_and_coincident_constraint_tools_wire_through() {
        let tools = SketchToolRegistry::with_builtins();
        let mut state = SketchState::with_origin();
        // Two free points so the coincident constraint has refs.
        tools
            .apply_all(
                &mut state,
                &[
                    FeatureCall::new("sketch_point", id(1), json!({ "at": [5.0, 5.0] })),
                    FeatureCall::new("sketch_point", id(2), json!({ "at": [5.0, 6.0] })),
                    FeatureCall::new(
                        "sketch_constraint_coincident",
                        id(3),
                        json!({ "a": 1, "b": 2 }),
                    ),
                    FeatureCall::new(
                        "sketch_line",
                        id(4),
                        json!({ "start": [0.0, 0.0], "end": [0.5, 10.0] }),
                    ),
                    FeatureCall::new(
                        "sketch_constraint_vertical",
                        id(5),
                        json!({ "line": 5 }),
                    ),
                ],
            )
            .unwrap();
        assert_eq!(state.constraints.len(), 2);
        assert!(state.constraints.iter().any(|c| matches!(
            c,
            Constraint::Coincident { a: EntityId(1), b: EntityId(2), .. }
        )));
        assert!(state.constraints.iter().any(|c| matches!(
            c,
            Constraint::Vertical { line: EntityId(5), .. }
        )));
    }

    // ─── K7.0: Sketch2d synthesis ──────────────────────────────

    #[test]
    fn build_sketch2d_returns_empty_for_a_blank_sketch() {
        let sketch = build_sketch2d_from_sub_calls(&[]);
        assert!(matches!(sketch, Sketch2d::Empty));
    }

    #[test]
    fn build_sketch2d_emits_a_rectangle_primitive() {
        let calls = vec![FeatureCall::new(
            "sketch_rectangle",
            id(1),
            json!({ "corner_a": [0.0, 0.0], "corner_b": [20.0, 10.0] }),
        )];
        let sketch = build_sketch2d_from_sub_calls(&calls);
        if let Sketch2d::Prim(Sketch2dPrimitive::Rectangle {
            center,
            half_extents,
        }) = sketch
        {
            assert!((center[0] - 10.0).abs() < 1e-6);
            assert!((center[1] - 5.0).abs() < 1e-6);
            assert!((half_extents[0] - 10.0).abs() < 1e-6);
            assert!((half_extents[1] - 5.0).abs() < 1e-6);
        } else {
            panic!("expected Sketch2d::Prim::Rectangle");
        }
    }

    #[test]
    fn build_sketch2d_normalises_diagonal_rectangles() {
        // Dragged from top-right to bottom-left: corner_a is the
        // top-right, corner_b the bottom-left. The synthesis must
        // still emit a positively-oriented rectangle.
        let calls = vec![FeatureCall::new(
            "sketch_rectangle",
            id(1),
            json!({ "corner_a": [20.0, 10.0], "corner_b": [-5.0, -5.0] }),
        )];
        let sketch = build_sketch2d_from_sub_calls(&calls);
        if let Sketch2d::Prim(Sketch2dPrimitive::Rectangle {
            half_extents,
            ..
        }) = sketch
        {
            assert!(half_extents[0] > 0.0);
            assert!(half_extents[1] > 0.0);
        } else {
            panic!("expected rectangle primitive");
        }
    }

    #[test]
    fn build_sketch2d_emits_a_circle_primitive() {
        let calls = vec![FeatureCall::new(
            "sketch_circle",
            id(1),
            json!({ "center": [3.0, -4.0], "radius": 7.0 }),
        )];
        let sketch = build_sketch2d_from_sub_calls(&calls);
        if let Sketch2d::Prim(Sketch2dPrimitive::Circle { center, radius }) =
            sketch
        {
            assert!((center[0] - 3.0).abs() < 1e-6);
            assert!((center[1] + 4.0).abs() < 1e-6);
            assert!((radius - 7.0).abs() < 1e-6);
        } else {
            panic!("expected circle primitive");
        }
    }

    #[test]
    fn build_sketch2d_emits_an_ellipse_as_closed_quad_bezier_path() {
        // DL-49 Option B: an ellipse lowers to a closed Path of
        // quadratic Béziers, not an exact analytic primitive.
        let calls = vec![FeatureCall::new(
            "sketch_ellipse",
            id(1),
            json!({ "center": [0.0, 0.0], "radius_x": 10.0, "radius_y": 4.0 }),
        )];
        let sketch = build_sketch2d_from_sub_calls(&calls);
        let Sketch2d::Prim(Sketch2dPrimitive::Path { segments, closed }) = &sketch else {
            panic!("expected a closed Path primitive, got {sketch:?}");
        };
        assert!(*closed, "ellipse path must be closed");
        assert!(segments.len() >= 4, "expected several segments");
        assert!(segments
            .iter()
            .all(|s| matches!(s, SketchSegment::QuadBezier { .. })));
        // Sign sanity, including the major axis (the seam line) which
        // is the even-odd degeneracy that the seam-snap fixes.
        assert!(sketch.sdf([0.0, 0.0]) < 0.0, "centre inside");
        assert!(sketch.sdf([8.0, 0.0]) < 0.0, "inside along major axis");
        assert!(sketch.sdf([11.0, 0.0]) > 0.0, "outside past major axis");
        assert!(sketch.sdf([0.0, 5.0]) > 0.0, "outside past minor axis");
    }

    #[test]
    fn build_sketch2d_unions_multiple_region_primitives() {
        let calls = vec![
            FeatureCall::new(
                "sketch_rectangle",
                id(1),
                json!({ "corner_a": [-10.0, -10.0], "corner_b": [10.0, 10.0] }),
            ),
            FeatureCall::new(
                "sketch_circle",
                id(2),
                json!({ "center": [20.0, 0.0], "radius": 5.0 }),
            ),
        ];
        let sketch = build_sketch2d_from_sub_calls(&calls);
        let Sketch2d::Union(_, _) = &sketch else {
            panic!("expected Union of two primitives, got {:?}", sketch);
        };
        // Spot-check: origin lives inside the rectangle, `(20, 0)`
        // is the centre of the circle. Both should be strictly
        // inside the combined region (negative SDF).
        assert!(sketch.sdf([0.0, 0.0]) < 0.0);
        assert!(sketch.sdf([20.0, 0.0]) < 0.0);
        // A point far from both is outside.
        assert!(sketch.sdf([100.0, 100.0]) > 0.0);
    }

    #[test]
    fn build_sketch2d_ignores_lines_points_and_constraints() {
        // None of these calls form a closed region on their own,
        // so the K7.0 synthesis drops them. A sketch made only
        // of loose lines yields an empty Sketch2d.
        let calls = vec![
            FeatureCall::new(
                "sketch_line",
                id(1),
                json!({ "start": [0.0, 0.0], "end": [10.0, 0.0] }),
            ),
            FeatureCall::new("sketch_point", id(2), json!({ "at": [5.0, 5.0] })),
            FeatureCall::new(
                "sketch_constraint_horizontal",
                id(3),
                json!({ "line": 3 }),
            ),
        ];
        assert!(matches!(
            build_sketch2d_from_sub_calls(&calls),
            Sketch2d::Empty
        ));
    }

    #[test]
    fn build_sketch2d_mirror_unions_a_reflected_copy() {
        // A rectangle in the +x half-plane, then a mirror across the
        // Y axis (axis = 1 flips X). The result is a Union of the
        // original and its reflection: the original centre stays
        // inside and the reflected centre (−x) becomes inside too.
        let calls = vec![
            FeatureCall::new(
                "sketch_rectangle",
                id(1),
                json!({ "corner_a": [10.0, -5.0], "corner_b": [20.0, 5.0] }),
            ),
            FeatureCall::new("sketch_mirror", id(2), json!({ "axis": 1.0 })),
        ];
        let sketch = build_sketch2d_from_sub_calls(&calls);
        let Sketch2d::Union(_, _) = &sketch else {
            panic!("expected a Union (original ∪ mirrored), got {sketch:?}");
        };
        assert!(sketch.sdf([15.0, 0.0]) < 0.0, "original centre inside");
        assert!(sketch.sdf([-15.0, 0.0]) < 0.0, "mirrored centre inside");
    }

    #[test]
    fn build_sketch2d_mirror_without_prior_geometry_is_empty() {
        // Mirror has nothing to reflect when no region precedes it.
        let calls = vec![FeatureCall::new("sketch_mirror", id(1), json!({ "axis": 0.0 }))];
        assert!(matches!(
            build_sketch2d_from_sub_calls(&calls),
            Sketch2d::Empty
        ));
    }

    #[test]
    fn build_sketch2d_offset_grows_the_profile() {
        // A 20×20 rectangle (half-extent 10) grown by 5 mm. A point at
        // (13, 0) sits outside the original but inside the offset, and
        // the tree is wrapped in a single Offset node carrying +5.
        let calls = vec![
            FeatureCall::new(
                "sketch_rectangle",
                id(1),
                json!({ "corner_a": [-10.0, -10.0], "corner_b": [10.0, 10.0] }),
            ),
            FeatureCall::new("sketch_offset", id(2), json!({ "distance": 5.0 })),
        ];
        let sketch = build_sketch2d_from_sub_calls(&calls);
        let Sketch2d::Offset { distance, .. } = &sketch else {
            panic!("expected an Offset node wrapping the profile, got {sketch:?}");
        };
        assert!((distance - 5.0).abs() < 1e-6, "offset distance preserved");
        assert!(sketch.sdf([13.0, 0.0]) < 0.0, "point inside the grown profile");
        // Beyond the grown boundary it is still outside.
        assert!(sketch.sdf([16.0, 0.0]) > 0.0, "point past the grown boundary");
    }

    #[test]
    fn build_sketch2d_offset_inward_shrinks_the_profile() {
        // Negative distance shrinks: a point just inside the original
        // boundary falls outside the inset profile.
        let calls = vec![
            FeatureCall::new(
                "sketch_rectangle",
                id(1),
                json!({ "corner_a": [-10.0, -10.0], "corner_b": [10.0, 10.0] }),
            ),
            FeatureCall::new("sketch_offset", id(2), json!({ "distance": -4.0 })),
        ];
        let sketch = build_sketch2d_from_sub_calls(&calls);
        assert!(sketch.sdf([8.0, 0.0]) > 0.0, "near-edge point now outside");
        assert!(sketch.sdf([0.0, 0.0]) < 0.0, "centre still inside");
    }

    #[test]
    fn build_sketch2d_excludes_construction_geometry() {
        // A solid rectangle plus a construction circle. The circle must
        // not contribute to the region: its centre is outside the solid.
        let calls = vec![
            FeatureCall::new(
                "sketch_rectangle",
                id(1),
                json!({ "corner_a": [-10.0, -10.0], "corner_b": [10.0, 10.0] }),
            ),
            FeatureCall::new(
                "sketch_circle",
                id(2),
                json!({ "center": [50.0, 0.0], "radius": 5.0, "construction": true }),
            ),
        ];
        let sketch = build_sketch2d_from_sub_calls(&calls);
        // Only the rectangle survives — no Union with the circle.
        let Sketch2d::Prim(Sketch2dPrimitive::Rectangle { .. }) = &sketch else {
            panic!("expected a lone rectangle primitive, got {sketch:?}");
        };
        // The construction circle's centre is firmly outside the region.
        assert!(sketch.sdf([50.0, 0.0]) > 0.0, "construction circle not extruded");
    }

    #[test]
    fn build_sketch2d_construction_flag_defaults_false() {
        // Without the flag the circle is a normal region primitive.
        let calls = vec![FeatureCall::new(
            "sketch_circle",
            id(1),
            json!({ "center": [0.0, 0.0], "radius": 5.0 }),
        )];
        assert!(matches!(
            build_sketch2d_from_sub_calls(&calls),
            Sketch2d::Prim(Sketch2dPrimitive::Circle { .. })
        ));
    }

    #[test]
    fn build_sketch2d_skips_degenerate_primitives() {
        // Zero-area rectangle and zero-radius circle don't
        // contribute to the output.
        let calls = vec![
            FeatureCall::new(
                "sketch_rectangle",
                id(1),
                json!({ "corner_a": [3.0, 3.0], "corner_b": [3.0, 3.0] }),
            ),
            FeatureCall::new(
                "sketch_circle",
                id(2),
                json!({ "center": [0.0, 0.0], "radius": 0.0 }),
            ),
        ];
        assert!(matches!(
            build_sketch2d_from_sub_calls(&calls),
            Sketch2d::Empty
        ));
    }

    #[test]
    fn sketch_point_schema_reports_vec2_shape() {
        let schema = SketchPointDef::schema_json();
        let params = schema.get("params").unwrap().as_array().unwrap();
        assert_eq!(params.len(), 1);
        let at = &params[0];
        assert_eq!(at.get("key").unwrap(), "at");
        assert_eq!(at.get("type").unwrap(), "vec2");
        assert_eq!(at.get("required").unwrap(), true);
    }

    // ─── C1: sketch frames ─────────────────────────────────────────

    fn is_orthonormal(f: &SketchFrame) -> bool {
        let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        let unit = |a: [f32; 3]| (dot(a, a) - 1.0).abs() < 1e-4;
        unit(f.x_axis)
            && unit(f.y_axis)
            && unit(f.normal)
            && dot(f.x_axis, f.y_axis).abs() < 1e-4
            && dot(f.x_axis, f.normal).abs() < 1e-4
            && dot(f.y_axis, f.normal).abs() < 1e-4
    }

    #[test]
    fn datum_plane_frames_are_orthonormal_and_well_known() {
        let xy = datum_plane_frame(&FeatureId::from_str("origin_plane_xy").unwrap()).unwrap();
        assert_eq!(xy, SketchFrame::IDENTITY);
        assert!(xy.to_transform().is_identity(), "xy datum is the identity placement");

        let xz = datum_plane_frame(&FeatureId::from_str("origin_plane_xz").unwrap()).unwrap();
        assert_eq!(xz.normal, [0.0, 1.0, 0.0]);
        assert!(is_orthonormal(&xz));

        let yz = datum_plane_frame(&FeatureId::from_str("origin_plane_yz").unwrap()).unwrap();
        assert_eq!(yz.normal, [1.0, 0.0, 0.0]);
        assert!(is_orthonormal(&yz));

        assert!(datum_plane_frame(&FeatureId::from_u32(5)).is_none());
    }

    #[test]
    fn face_plane_frame_is_orthonormal_with_face_normal() {
        // Weave a box and synthesise frames on each face; every frame must
        // be orthonormal and its normal must match the face's plane normal.
        let registry = FeatureRegistry::with_builtins();
        let mut doc = Document::default();
        let mut ctx = Context::new(&mut doc);
        registry
            .regenerate(
                &mut ctx,
                &[FeatureCall::new("box", id(1), json!({ "half_extents": [5.0, 5.0, 5.0] }))],
                None,
            )
            .unwrap();
        let woven = crate::body::weave_all(&ctx.features);
        let (brep, placement) = &woven[0];
        for i in 0..brep.face_count() {
            let frame = face_plane_frame(brep, crate::brep::FaceId(i as u32), placement);
            assert!(is_orthonormal(&frame), "face {i} frame not orthonormal: {frame:?}");
        }
    }

    #[test]
    fn sketch_on_origin_plane_xy_yields_identity_frame() {
        let registry = FeatureRegistry::with_builtins();
        let mut doc = Document::default();
        let mut ctx = Context::new(&mut doc);
        let sketch_id = id(1);
        registry
            .regenerate(
                &mut ctx,
                &[FeatureCall::new(
                    SketchFeature::TYPE_NAME,
                    sketch_id.clone(),
                    json!({
                        "plane": "qCreatedBy(origin_plane_xy)",
                        "sub_calls": [
                            { "type": "sketch_rectangle", "id": "100",
                              "params": { "corner_a": [0.0, 0.0], "corner_b": [4.0, 3.0] } }
                        ]
                    }),
                )],
                None,
            )
            .unwrap();
        let data = ctx.sketches.get(&sketch_id).unwrap();
        assert_eq!(data.frame, SketchFrame::IDENTITY, "xy sketch is the identity frame");
        assert!(data.frame.to_transform().is_identity());
    }

    #[test]
    fn sketch_on_prior_box_face_lands_on_that_face() {
        // A box, then a sketch whose plane is the box's +Z face. The
        // sketch frame's origin must sit on that face (z = +5) and its
        // normal must be +Z — proving sketch-on-face resolves mid-loop.
        let registry = FeatureRegistry::with_builtins();
        let mut doc = Document::default();
        let mut ctx = Context::new(&mut doc);

        let box_call = FeatureCall::new("box", id(1), json!({ "half_extents": [5.0, 5.0, 5.0] }));
        registry.regenerate(&mut ctx, std::slice::from_ref(&box_call), None).unwrap();

        // Learn the +Z face id_hash.
        let woven = crate::body::weave_all(&ctx.features);
        let (brep, placement) = &woven[0];
        let mut top_hash = None;
        for i in 0..brep.face_count() {
            let f = face_plane_frame(brep, crate::brep::FaceId(i as u32), placement);
            if (f.normal[2] - 1.0).abs() < 1e-4 {
                top_hash = Some(
                    crate::brep::naming::TopoRef::from_face(brep, crate::brep::FaceId(i as u32))
                        .id_hash,
                );
            }
        }
        let top_hash = top_hash.expect("box has a +Z face");

        // Re-run from scratch: box + sketch-on-face + extrude.
        let mut doc = Document::default();
        let mut ctx = Context::new(&mut doc);
        let sketch_id = id(2);
        let extrude_id = id(3);
        registry
            .regenerate(
                &mut ctx,
                &[
                    box_call.clone(),
                    FeatureCall::new(
                        SketchFeature::TYPE_NAME,
                        sketch_id.clone(),
                        json!({
                            "plane": format!("qFace({top_hash})"),
                            "sub_calls": [
                                { "type": "sketch_rectangle", "id": "200",
                                  "params": { "corner_a": [-2.0, -2.0], "corner_b": [2.0, 2.0] } }
                            ]
                        }),
                    ),
                    FeatureCall::new(
                        "extrude",
                        extrude_id.clone(),
                        json!({
                            "profiles": format!("qCreatedBy({sketch_id})"),
                            "depth": { "value": 4.0, "unit": "mm" },
                        }),
                    ),
                ],
                None,
            )
            .unwrap();

        // The sketch frame sits on the +Z face.
        let frame = ctx.sketches.get(&sketch_id).unwrap().frame;
        assert!((frame.normal[2] - 1.0).abs() < 1e-3, "sketch normal +Z, got {:?}", frame.normal);
        assert!((frame.origin[2] - 5.0).abs() < 1e-2, "sketch origin on the +Z face, got {:?}", frame.origin);

        // The extrude inherits a non-identity placement (the frame),
        // unlike an xy sketch — its translate lifts the profile to z≈5.
        let extrude = ctx.features.iter().find(|f| f.id == extrude_id).unwrap();
        assert!(
            (extrude.transform.translate[2] - 5.0).abs() < 1e-2,
            "extrude placement lifted to the face, got {:?}",
            extrude.transform.translate
        );
    }
}
