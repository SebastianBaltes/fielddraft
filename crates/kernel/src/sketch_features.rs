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
///   insertion, then each cubic is subdivided to a flatness tolerance
///   into [`SketchSegment::QuadBezier`] segments (see
///   [`bspline_quad_beziers`]).
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

/// Sample a Catmull-Rom interpolating spline through `control_pts`
/// and emit one [`SketchSegment::QuadBezier`] per sub-step, forming
/// a Path that passes through every control point.
///
/// Method: each Catmull-Rom span (between consecutive control
/// points `p1`→`p2`, with neighbours `p0`/`p3` for tangents) is a
/// cubic. We sample the span at `steps` uniformly-spaced parameters
/// and, between successive samples, fit a quadratic Bézier whose
/// control point `p1` is the intersection of the curve tangents at
/// the two sample endpoints (falling back to the chord midpoint
/// when the tangents are near-parallel). Sampling densely keeps the
/// quadratic approximation visually faithful while reusing the
/// kernel's exact QuadBezier SDF machinery.
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
    steps: u32,
) -> Vec<SketchSegment> {
    let n = control_pts.len();
    if n < 2 {
        return Vec::new();
    }
    let steps = steps.max(1);

    // Catmull-Rom position at parameter t∈[0,1] on the span p1→p2.
    let cr = |p0: [f32; 2], p1: [f32; 2], p2: [f32; 2], p3: [f32; 2], t: f32| -> [f32; 2] {
        let t2 = t * t;
        let t3 = t2 * t;
        let f = |a: f32, b: f32, c: f32, d: f32| {
            0.5 * ((2.0 * b)
                + (-a + c) * t
                + (2.0 * a - 5.0 * b + 4.0 * c - d) * t2
                + (-a + 3.0 * b - 3.0 * c + d) * t3)
        };
        [f(p0[0], p1[0], p2[0], p3[0]), f(p0[1], p1[1], p2[1], p3[1])]
    };
    // Catmull-Rom tangent (derivative) at parameter t on the span.
    let cr_tan = |p0: [f32; 2], p1: [f32; 2], p2: [f32; 2], p3: [f32; 2], t: f32| -> [f32; 2] {
        let t2 = t * t;
        let f = |a: f32, b: f32, c: f32, d: f32| {
            0.5 * ((-a + c)
                + 2.0 * (2.0 * a - 5.0 * b + 4.0 * c - d) * t
                + 3.0 * (-a + 3.0 * b - 3.0 * c + d) * t2)
        };
        [f(p0[0], p1[0], p2[0], p3[0]), f(p0[1], p1[1], p2[1], p3[1])]
    };

    let span_count = if closed { n } else { n - 1 };
    let mut segs = Vec::with_capacity(span_count * steps as usize);
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
        let p0 = control_pts[i0];
        let p1 = control_pts[i1];
        let p2 = control_pts[i2];
        let p3 = control_pts[i3];

        for s in 0..steps {
            let ta = s as f32 / steps as f32;
            let tb = (s + 1) as f32 / steps as f32;
            let a = cr(p0, p1, p2, p3, ta);
            let b = cr(p0, p1, p2, p3, tb);
            let tan_a = cr_tan(p0, p1, p2, p3, ta);
            let tan_b = cr_tan(p0, p1, p2, p3, tb);
            // Quad-Bézier control point = intersection of the two
            // tangent lines (a + u·tan_a) and (b − w·tan_b); falls
            // back to the chord midpoint if near-parallel.
            let ctrl = line_intersection(a, tan_a, b, tan_b)
                .unwrap_or([0.5 * (a[0] + b[0]), 0.5 * (a[1] + b[1])]);
            segs.push(SketchSegment::QuadBezier {
                p0: a,
                p1: ctrl,
                p2: b,
            });
        }
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
/// 3. Subdivide each cubic Bézier to a flatness tolerance into
///    quadratic Béziers (the kernel `SketchSegment` has no cubic).
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
    flatness: f64,
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
            4 => cubic_to_quads(bez[0], bez[1], bez[2], bez[3], flatness, &mut segs),
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

/// Subdivide a cubic Bézier (`p0,p1,p2,p3`) into [`SketchSegment::
/// QuadBezier`] segments, recursing until the cubic is flat enough
/// (control points within `flatness` of the chord) to be replaced by
/// a single quad whose control point is the average of the cubic's
/// elevated mid controls. Pushes onto `out`.
fn cubic_to_quads(
    p0: [f64; 2],
    p1: [f64; 2],
    p2: [f64; 2],
    p3: [f64; 2],
    flatness: f64,
    out: &mut Vec<SketchSegment>,
) {
    // Flatness: max distance of p1,p2 from the chord p0→p3.
    let dx = p3[0] - p0[0];
    let dy = p3[1] - p0[1];
    let chord = (dx * dx + dy * dy).sqrt();
    let dist = |p: [f64; 2]| -> f64 {
        if chord < 1e-12 {
            ((p[0] - p0[0]).powi(2) + (p[1] - p0[1]).powi(2)).sqrt()
        } else {
            ((p[0] - p0[0]) * dy - (p[1] - p0[1]) * dx).abs() / chord
        }
    };
    let d = dist(p1).max(dist(p2));
    if d <= flatness {
        // Approximate the cubic by a single quad. The quad control
        // point is the intersection of the end tangents (p0→p1 and
        // p3→p2); fall back to the chord midpoint when degenerate.
        let t0 = [p1[0] - p0[0], p1[1] - p0[1]];
        let t1 = [p2[0] - p3[0], p2[1] - p3[1]];
        let ctrl = line_intersection(
            [p0[0] as f32, p0[1] as f32],
            [t0[0] as f32, t0[1] as f32],
            [p3[0] as f32, p3[1] as f32],
            [t1[0] as f32, t1[1] as f32],
        )
        .unwrap_or([
            (0.5 * (p0[0] + p3[0])) as f32,
            (0.5 * (p0[1] + p3[1])) as f32,
        ]);
        // Guard against NaN/Inf from a pathological intersection.
        let ctrl = if ctrl[0].is_finite() && ctrl[1].is_finite() {
            ctrl
        } else {
            [
                (0.5 * (p0[0] + p3[0])) as f32,
                (0.5 * (p0[1] + p3[1])) as f32,
            ]
        };
        out.push(SketchSegment::QuadBezier {
            p0: [p0[0] as f32, p0[1] as f32],
            p1: ctrl,
            p2: [p3[0] as f32, p3[1] as f32],
        });
        return;
    }
    // de Casteljau split at t = 0.5.
    let mid = |a: [f64; 2], b: [f64; 2]| [0.5 * (a[0] + b[0]), 0.5 * (a[1] + b[1])];
    let p01 = mid(p0, p1);
    let p12 = mid(p1, p2);
    let p23 = mid(p2, p3);
    let p012 = mid(p01, p12);
    let p123 = mid(p12, p23);
    let p0123 = mid(p012, p123);
    cubic_to_quads(p0, p01, p012, p0123, flatness, out);
    cubic_to_quads(p0123, p123, p23, p3, flatness, out);
}

/// Evaluate a cubic (degree-3) B-spline at parameter `t` via de Boor,
/// for the open (clamped-uniform) knot vector. `t` is in `[0, 1]`,
/// remapped onto the clamped knot domain. This mirrors the TS
/// `bsplinePoints` de Boor and is used by tests to validate the
/// exact lowering against direct evaluation.
#[cfg(test)]
pub fn bspline_eval_open(control_pts: &[[f64; 2]], degree: usize, t: f64) -> [f64; 2] {
    let n = control_pts.len();
    let interior = n - degree - 1;
    let mut knots: Vec<f64> = Vec::with_capacity(n + degree + 1);
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
    let u = t * last;
    de_boor(control_pts, &knots, degree, u)
}

/// de Boor evaluation of a B-spline at parameter `u`.
#[cfg(test)]
fn de_boor(poly: &[[f64; 2]], knots: &[f64], degree: usize, u: f64) -> [f64; 2] {
    let n = poly.len();
    // Find knot span k: knots[k] <= u < knots[k+1] (clamp so the last
    // affecting control point stays in range, k ∈ degree..=n-1).
    let mut k = degree;
    while k + 1 <= n - 1 && knots[k + 1] <= u {
        k += 1;
    }
    // Local copy of the degree+1 affecting control points.
    let mut d: Vec<[f64; 2]> = (0..=degree)
        .map(|j| poly[(k - degree + j).min(n - 1)])
        .collect();
    for r in 1..=degree {
        for j in (r..=degree).rev() {
            let i = k - degree + j;
            let denom = knots[i + degree - r + 1] - knots[i];
            let alpha = if denom.abs() < 1e-12 {
                0.0
            } else {
                (u - knots[i]) / denom
            };
            d[j] = [
                (1.0 - alpha) * d[j - 1][0] + alpha * d[j][0],
                (1.0 - alpha) * d[j - 1][1] + alpha * d[j][1],
            ];
        }
    }
    d[degree]
}

/// Intersect line `(p + t·d)` with line `(q + s·e)`. Returns `None`
/// when the directions are near-parallel.
fn line_intersection(p: [f32; 2], d: [f32; 2], q: [f32; 2], e: [f32; 2]) -> Option<[f32; 2]> {
    let denom = d[0] * e[1] - d[1] * e[0];
    if denom.abs() < 1e-6 {
        return None;
    }
    let t = ((q[0] - p[0]) * e[1] - (q[1] - p[1]) * e[0]) / denom;
    Some([p[0] + t * d[0], p[1] + t * d[1]])
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
        SplineMode::Interpolate => catmull_rom_quad_beziers(&pts, d.closed, 16),
        SplineMode::Control => {
            let cp: Vec<[f64; 2]> = pts.iter().map(|p| [p[0] as f64, p[1] as f64]).collect();
            bspline_quad_beziers(&cp, d.closed, 0.05)
        }
    }
}

/// One chainable element for the loop-inference pass: the lowered
/// segments of a free `sketch_line` or open `sketch_spline`, plus the
/// index of the sub-call it came from.
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
fn infer_closed_loops(
    elems: &[ChainElement],
) -> (HashMap<usize, Vec<Vec<SketchSegment>>>, std::collections::HashSet<usize>) {
    let close = |a: [f32; 2], b: [f32; 2]| {
        (a[0] - b[0]).abs() <= LOOP_JOIN_TOL && (a[1] - b[1]).abs() <= LOOP_JOIN_TOL
    };
    let mut loops: HashMap<usize, Vec<Vec<SketchSegment>>> = HashMap::new();
    let mut consumed = std::collections::HashSet::new();
    let mut used = vec![false; elems.len()];
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
        tools
            .apply_all(&mut state, std::slice::from_ref(&call))
            .unwrap();
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
        tools
            .apply_all(&mut state, std::slice::from_ref(&call))
            .unwrap();
        // two points + one line
        assert_eq!(state.entities.len(), 3);
    }

    #[test]
    fn sketch_ellipse_tool_allocates_centre_focus_and_ellipse() {
        let tools = SketchToolRegistry::with_builtins();
        let mut state = SketchState::new(); // no origin — keep the count easy
        let call = FeatureCall::new(
            "sketch_ellipse",
            id(1),
            json!({ "center": [0.0, 0.0], "radius_x": 10.0, "radius_y": 4.0 }),
        );
        tools
            .apply_all(&mut state, std::slice::from_ref(&call))
            .unwrap();
        // centre point (id 1) + focus point (id 2) + ellipse (id 3).
        assert_eq!(state.entities.len(), 3);
        match &state.entities[2] {
            fielddraft_solver::Entity::Ellipse {
                center,
                focus1,
                radmin,
                ..
            } => {
                assert_eq!(*center, EntityId(1));
                assert_eq!(*focus1, EntityId(2));
                // semi-minor radius = min(radius_x, radius_y) = 4.
                assert!((*radmin - 4.0).abs() < 1e-9);
            }
            other => panic!("expected Ellipse at index 2, got {other:?}"),
        }
        // Focus sits on the major (x) axis at sqrt(10² − 4²) ≈ 9.165.
        match &state.entities[1] {
            fielddraft_solver::Entity::Point { at, .. } => {
                assert!((at.x - (100.0_f64 - 16.0).sqrt()).abs() < 1e-6);
                assert!(at.y.abs() < 1e-9);
            }
            other => panic!("expected focus Point at index 1, got {other:?}"),
        }
    }

    #[test]
    fn unknown_sketch_tool_type_is_reported() {
        let tools = SketchToolRegistry::with_builtins();
        let mut state = SketchState::new();
        let call = FeatureCall::new("not_a_tool", id(1), json!({}));
        let err = tools
            .apply_all(&mut state, std::slice::from_ref(&call))
            .unwrap_err();
        assert_eq!(
            err,
            FeatureError::UnknownFeatureType("not_a_tool".to_string())
        );
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
        registry
            .regenerate(&mut ctx, std::slice::from_ref(&call), None)
            .unwrap();

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
        assert_eq!(
            err,
            FeatureError::UnknownFeatureType("not_a_tool".to_string())
        );
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
                "sketch_constraint_concentric".to_string(),
                "sketch_constraint_diameter".to_string(),
                "sketch_constraint_distance".to_string(),
                "sketch_constraint_equal".to_string(),
                "sketch_constraint_fix".to_string(),
                "sketch_constraint_horizontal".to_string(),
                "sketch_constraint_midpoint".to_string(),
                "sketch_constraint_parallel".to_string(),
                "sketch_constraint_perpendicular".to_string(),
                "sketch_constraint_point_on_object".to_string(),
                "sketch_constraint_radius".to_string(),
                "sketch_constraint_symmetric".to_string(),
                "sketch_constraint_tangent".to_string(),
                "sketch_constraint_vertical".to_string(),
                "sketch_ellipse".to_string(),
                "sketch_line".to_string(),
                "sketch_mirror".to_string(),
                "sketch_offset".to_string(),
                "sketch_path".to_string(),
                "sketch_point".to_string(),
                "sketch_polygon".to_string(),
                "sketch_rectangle".to_string(),
                "sketch_regular_polygon".to_string(),
                "sketch_slot".to_string(),
                "sketch_spline".to_string(),
            ]
        );
    }

    #[test]
    fn sketch_regular_polygon_square_inscribed_emits_four_points() {
        let tools = SketchToolRegistry::with_builtins();
        let mut state = SketchState::with_origin();
        let call = FeatureCall::new(
            "sketch_regular_polygon",
            id(1),
            json!({ "center": [0.0, 0.0], "radius": 10.0, "sides": 4.0, "inscribed": true }),
        );
        tools
            .apply_all(&mut state, std::slice::from_ref(&call))
            .unwrap();
        let pts: Vec<(f64, f64)> = state
            .entities
            .iter()
            .filter_map(|e| match e {
                fielddraft_solver::Entity::Point { at, .. } => Some((at.x, at.y)),
                _ => None,
            })
            .collect();
        assert_eq!(pts.len(), 4);
        // Square inscribed in r=10: vertices at top, left, bottom, right
        // (angle = π/2 + k·π/2, CCW).
        let expected = [(0.0, 10.0), (-10.0, 0.0), (0.0, -10.0), (10.0, 0.0)];
        for (got, exp) in pts.iter().zip(expected.iter()) {
            assert!(
                (got.0 - exp.0).abs() < 1e-4 && (got.1 - exp.1).abs() < 1e-4,
                "vertex {:?} != expected {:?}",
                got,
                exp
            );
        }
    }

    #[test]
    fn sketch_regular_polygon_edge_cases() {
        // sides < 3 clamps to 3.
        let v = regular_polygon_vertices([0.0, 0.0], 5.0, 2.0, true).unwrap();
        assert_eq!(v.len(), 3);
        // non-integer sides rounds (5.6 → 6).
        let v = regular_polygon_vertices([0.0, 0.0], 5.0, 5.6, true).unwrap();
        assert_eq!(v.len(), 6);
        // circumscribed circumradius > inscribed for the same radius.
        let hex_in = regular_polygon_vertices([0.0, 0.0], 10.0, 6.0, true).unwrap();
        let hex_circ = regular_polygon_vertices([0.0, 0.0], 10.0, 6.0, false).unwrap();
        let r_in = (hex_in[0][0].powi(2) + hex_in[0][1].powi(2)).sqrt();
        let r_circ = (hex_circ[0][0].powi(2) + hex_circ[0][1].powi(2)).sqrt();
        assert!(r_circ > r_in + 1e-3, "{} !> {}", r_circ, r_in);
        // radius 0 → no vertices (rejected).
        assert!(regular_polygon_vertices([0.0, 0.0], 0.0, 6.0, true).is_none());
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
        tools
            .apply_all(&mut state, std::slice::from_ref(&call))
            .unwrap();
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
        tools
            .apply_all(&mut state, std::slice::from_ref(&call))
            .unwrap();
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
            .find(|e| {
                matches!(
                    e,
                    fielddraft_solver::Entity::Point {
                        id: fielddraft_solver::EntityId(1),
                        ..
                    }
                )
            })
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
            Constraint::Horizontal {
                line: EntityId(3),
                ..
            }
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
                    FeatureCall::new("sketch_constraint_vertical", id(5), json!({ "line": 5 })),
                ],
            )
            .unwrap();
        assert_eq!(state.constraints.len(), 2);
        assert!(state.constraints.iter().any(|c| matches!(
            c,
            Constraint::Coincident {
                a: EntityId(1),
                b: EntityId(2),
                ..
            }
        )));
        assert!(state.constraints.iter().any(|c| matches!(
            c,
            Constraint::Vertical {
                line: EntityId(5),
                ..
            }
        )));
    }

    #[test]
    fn sketch_concentric_constraint_tool_wires_through() {
        let tools = SketchToolRegistry::with_builtins();
        let mut state = SketchState::with_origin();
        // Two circles so the concentric constraint has curve refs.
        tools
            .apply_all(
                &mut state,
                &[
                    FeatureCall::new(
                        "sketch_circle",
                        id(1),
                        json!({ "center": [0.0, 0.0], "radius": 3.0 }),
                    ),
                    FeatureCall::new(
                        "sketch_circle",
                        id(2),
                        json!({ "center": [9.0, 0.0], "radius": 4.0 }),
                    ),
                    FeatureCall::new(
                        "sketch_constraint_concentric",
                        id(3),
                        json!({ "a": 2, "b": 4 }),
                    ),
                ],
            )
            .unwrap();
        assert_eq!(state.constraints.len(), 1);
        assert!(state.constraints.iter().any(|c| matches!(
            c,
            Constraint::Concentric {
                a: EntityId(2),
                b: EntityId(4),
                ..
            }
        )));
    }

    #[test]
    fn sketch_fix_constraint_tool_wires_through() {
        let tools = SketchToolRegistry::with_builtins();
        let mut state = SketchState::with_origin();
        // A single point to pin in place.
        tools
            .apply_all(
                &mut state,
                &[
                    FeatureCall::new("sketch_point", id(1), json!({ "at": [3.0, 4.0] })),
                    FeatureCall::new("sketch_constraint_fix", id(2), json!({ "point": 1 })),
                ],
            )
            .unwrap();
        assert_eq!(state.constraints.len(), 1);
        assert!(state.constraints.iter().any(|c| matches!(
            c,
            Constraint::Fix {
                point: EntityId(1),
                ..
            }
        )));
    }

    #[test]
    fn sketch_point_on_object_constraint_tool_wires_through() {
        let tools = SketchToolRegistry::with_builtins();
        let mut state = SketchState::with_origin();
        // A circle (id 1) and a free point (id 2) constrained onto it.
        tools
            .apply_all(
                &mut state,
                &[
                    FeatureCall::new(
                        "sketch_circle",
                        id(1),
                        json!({ "center": [0.0, 0.0], "radius": 5.0 }),
                    ),
                    FeatureCall::new("sketch_point", id(2), json!({ "at": [3.0, 4.0] })),
                    FeatureCall::new(
                        "sketch_constraint_point_on_object",
                        id(3),
                        json!({ "point": 2, "curve": 1 }),
                    ),
                ],
            )
            .unwrap();
        assert_eq!(state.constraints.len(), 1);
        assert!(state.constraints.iter().any(|c| matches!(
            c,
            Constraint::PointOnObject {
                point: EntityId(2),
                curve: EntityId(1),
                ..
            }
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
        if let Sketch2d::Prim(Sketch2dPrimitive::Rectangle { half_extents, .. }) = sketch {
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
        if let Sketch2d::Prim(Sketch2dPrimitive::Circle { center, radius }) = sketch {
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
    fn build_sketch2d_emits_a_slot_as_closed_line_arc_path() {
        // A horizontal slot A=(0,0) B=(20,0) r=5 lowers to a closed
        // Path of 2 lines + 2 arcs whose caps bulge OUTWARD, so the
        // bounding box is x∈[−5,25], y∈[−5,5].
        let calls = vec![FeatureCall::new(
            "sketch_slot",
            id(1),
            json!({ "center_a": [0.0, 0.0], "center_b": [20.0, 0.0], "radius": 5.0 }),
        )];
        let sketch = build_sketch2d_from_sub_calls(&calls);
        let Sketch2d::Prim(Sketch2dPrimitive::Path { segments, closed }) = &sketch else {
            panic!("expected a closed Path primitive, got {sketch:?}");
        };
        assert!(*closed, "slot path must be closed");
        assert_eq!(segments.len(), 4, "expected 2 lines + 2 arcs");
        assert_eq!(
            segments
                .iter()
                .filter(|s| matches!(s, SketchSegment::Line { .. }))
                .count(),
            2
        );
        assert_eq!(
            segments
                .iter()
                .filter(|s| matches!(s, SketchSegment::Arc { .. }))
                .count(),
            2
        );

        // Bounding box from flattened contour samples: caps must bulge
        // outward (to x=−5 and x=+25), straight edges sit at y=±5.
        let mut min_x = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        for seg in segments {
            for k in 0..=16 {
                let t = k as f32 / 16.0;
                let p = seg.point_at(t);
                min_x = min_x.min(p[0]);
                max_x = max_x.max(p[0]);
                min_y = min_y.min(p[1]);
                max_y = max_y.max(p[1]);
            }
        }
        assert!((min_x - (-5.0)).abs() < 1e-3, "min_x={min_x}");
        assert!((max_x - 25.0).abs() < 1e-3, "max_x={max_x}");
        assert!((min_y - (-5.0)).abs() < 1e-3, "min_y={min_y}");
        assert!((max_y - 5.0).abs() < 1e-3, "max_y={max_y}");

        // Sign sanity: centre line inside, well outside the caps out.
        assert!(sketch.sdf([10.0, 0.0]) < 0.0, "slot interior");
        assert!(sketch.sdf([0.0, 0.0]) < 0.0, "cap-A centre inside");
        assert!(sketch.sdf([20.0, 0.0]) < 0.0, "cap-B centre inside");
        assert!(sketch.sdf([-6.0, 0.0]) > 0.0, "past cap A outside");
        assert!(sketch.sdf([26.0, 0.0]) > 0.0, "past cap B outside");
        assert!(sketch.sdf([10.0, 6.0]) > 0.0, "past straight edge outside");
    }

    #[test]
    fn build_sketch2d_rejects_degenerate_slot() {
        // Zero-length (coincident centres) → no primitive.
        let calls = vec![FeatureCall::new(
            "sketch_slot",
            id(1),
            json!({ "center_a": [5.0, 5.0], "center_b": [5.0, 5.0], "radius": 4.0 }),
        )];
        assert!(matches!(
            build_sketch2d_from_sub_calls(&calls),
            Sketch2d::Empty
        ));
        // Zero radius → no primitive.
        let calls = vec![FeatureCall::new(
            "sketch_slot",
            id(1),
            json!({ "center_a": [0.0, 0.0], "center_b": [20.0, 0.0], "radius": 0.0 }),
        )];
        assert!(matches!(
            build_sketch2d_from_sub_calls(&calls),
            Sketch2d::Empty
        ));
    }

    #[test]
    fn slot_tool_apply_registers_two_centre_points() {
        // The TS mirror must add exactly two points (A then B). Origin
        // is id 0, so A=1, B=2.
        let tools = SketchToolRegistry::with_builtins();
        let mut state = SketchState::with_origin();
        let call = FeatureCall::new(
            "sketch_slot",
            id(1),
            json!({ "center_a": [1.0, 2.0], "center_b": [9.0, 2.0], "radius": 3.0 }),
        );
        tools
            .apply_all(&mut state, std::slice::from_ref(&call))
            .unwrap();
        let pts: Vec<(f64, f64)> = state
            .entities
            .iter()
            .filter_map(|e| match e {
                fielddraft_solver::Entity::Point { at, .. } => Some((at.x, at.y)),
                _ => None,
            })
            .collect();
        // Exactly two centre points, A then B.
        assert_eq!(pts.len(), 2);
        assert!((pts[0].0 - 1.0).abs() < 1e-6 && (pts[0].1 - 2.0).abs() < 1e-6);
        assert!((pts[1].0 - 9.0).abs() < 1e-6 && (pts[1].1 - 2.0).abs() < 1e-6);
    }

    #[test]
    fn sketch_spline_tool_registers_one_point_per_control_point() {
        // The TS mirror must add exactly N points (one per control
        // point), in order. Origin is id 0, so the four control points
        // land at ids 1..=4.
        let tools = SketchToolRegistry::with_builtins();
        let mut state = SketchState::with_origin();
        let call = FeatureCall::new(
            "sketch_spline",
            id(1),
            json!({
                "points": [0.0, 0.0, 10.0, 5.0, 20.0, 0.0, 10.0, -5.0],
                "closed": false
            }),
        );
        tools
            .apply_all(&mut state, std::slice::from_ref(&call))
            .unwrap();
        let pts: Vec<(f64, f64)> = state
            .entities
            .iter()
            .filter_map(|e| match e {
                fielddraft_solver::Entity::Point { at, .. } => Some((at.x, at.y)),
                _ => None,
            })
            .collect();
        assert_eq!(pts.len(), 4);
        assert!((pts[0].0 - 0.0).abs() < 1e-6 && (pts[0].1 - 0.0).abs() < 1e-6);
        assert!((pts[3].0 - 10.0).abs() < 1e-6 && (pts[3].1 + 5.0).abs() < 1e-6);
    }

    #[test]
    fn sketch_spline_too_few_points_is_rejected() {
        let tools = SketchToolRegistry::with_builtins();
        let mut state = SketchState::with_origin();
        let call = FeatureCall::new("sketch_spline", id(1), json!({ "points": [1.0, 2.0] }));
        assert!(tools
            .apply_all(&mut state, std::slice::from_ref(&call))
            .is_err());
    }

    #[test]
    fn build_sketch2d_emits_a_closed_spline_as_quad_bezier_path() {
        // A closed spline through 4 points lowers to a closed Path of
        // many QuadBezier segments whose flattened bbox roughly covers
        // the control points (diamond ±10).
        let calls = vec![FeatureCall::new(
            "sketch_spline",
            id(1),
            json!({
                "points": [0.0, 10.0, 10.0, 0.0, 0.0, -10.0, -10.0, 0.0],
                "closed": true
            }),
        )];
        let sketch = build_sketch2d_from_sub_calls(&calls);
        let Sketch2d::Prim(Sketch2dPrimitive::Path { segments, closed }) = &sketch else {
            panic!("expected a closed Path primitive, got {sketch:?}");
        };
        assert!(*closed, "closed spline path must be closed");
        assert!(
            segments.len() >= 4,
            "expected multiple segments, got {}",
            segments.len()
        );
        assert!(segments
            .iter()
            .all(|s| matches!(s, SketchSegment::QuadBezier { .. })));
        // Flattened bbox roughly covers the control diamond.
        let mut min_x = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        for seg in segments {
            for k in 0..=8 {
                let p = seg.point_at(k as f32 / 8.0);
                min_x = min_x.min(p[0]);
                max_x = max_x.max(p[0]);
                min_y = min_y.min(p[1]);
                max_y = max_y.max(p[1]);
            }
        }
        assert!(min_x <= -9.0 && max_x >= 9.0, "x bbox {min_x}..{max_x}");
        assert!(min_y <= -9.0 && max_y >= 9.0, "y bbox {min_y}..{max_y}");
        // The interior is inside the closed region.
        assert!(sketch.sdf([0.0, 0.0]) < 0.0, "spline centre inside");
    }

    #[test]
    fn build_sketch2d_open_spline_is_an_open_path() {
        let calls = vec![FeatureCall::new(
            "sketch_spline",
            id(1),
            json!({ "points": [0.0, 0.0, 10.0, 5.0, 20.0, 0.0], "closed": false }),
        )];
        let sketch = build_sketch2d_from_sub_calls(&calls);
        let Sketch2d::Prim(Sketch2dPrimitive::Path { closed, .. }) = &sketch else {
            panic!("expected a Path primitive, got {sketch:?}");
        };
        assert!(!*closed, "open spline path must be open");
    }

    #[test]
    fn catmull_rom_quad_beziers_interpolates_control_points() {
        // The fitted Path must pass through every control point: the
        // start of each span sits exactly on a control point.
        let pts = [[0.0, 0.0], [10.0, 5.0], [20.0, 0.0], [30.0, 5.0]];
        let segs = catmull_rom_quad_beziers(&pts, false, 8);
        assert!(!segs.is_empty());
        // First sample of the first span is p0.
        let first = segs[0].point_at(0.0);
        assert!((first[0] - 0.0).abs() < 1e-3 && (first[1] - 0.0).abs() < 1e-3);
        // Last sample of the last span is the final control point.
        let last = segs.last().unwrap().point_at(1.0);
        assert!((last[0] - 30.0).abs() < 1e-3 && (last[1] - 5.0).abs() < 1e-3);
        // < 2 points → empty.
        assert!(catmull_rom_quad_beziers(&[[0.0, 0.0]], false, 8).is_empty());
    }

    // ---- F23-full: real cubic B-spline (control mode) ----------------

    /// Sample a lowered segment chain at a fraction of total arc-ish
    /// parameter (segment index proportion), returning the point.
    fn seg_point(segs: &[SketchSegment], frac: f32) -> [f32; 2] {
        let n = segs.len();
        if n == 0 {
            return [0.0, 0.0];
        }
        let pos = (frac * n as f32).clamp(0.0, n as f32);
        let i = (pos.floor() as usize).min(n - 1);
        let local = pos - i as f32;
        segs[i].point_at(local.clamp(0.0, 1.0))
    }

    #[test]
    fn spline_def_roundtrip_with_mode() {
        // The new `mode` field round-trips through parse, defaulting to
        // interpolate when absent and accepting "control".
        let d_default = SketchSplineDef::parse(&json!({ "points": [0.0, 0.0, 1.0, 1.0] })).unwrap();
        assert_eq!(d_default.mode, SplineMode::Interpolate);
        assert!(!d_default.closed);
        let d_ctrl = SketchSplineDef::parse(&json!({
            "points": [0.0, 0.0, 1.0, 1.0, 2.0, 0.0, 3.0, 1.0],
            "closed": true,
            "mode": "control"
        }))
        .unwrap();
        assert_eq!(d_ctrl.mode, SplineMode::Control);
        assert!(d_ctrl.closed);
    }

    #[test]
    fn bspline_control_mode_lowers_to_quad_path() {
        // A 4-control-point open B-spline (control mode) lowers to a
        // non-empty open Path of Line/Quad segments.
        let calls = vec![FeatureCall::new(
            "sketch_spline",
            id(1),
            json!({
                "points": [0.0, 0.0, 10.0, 20.0, 20.0, -20.0, 30.0, 0.0],
                "closed": false,
                "mode": "control"
            }),
        )];
        let sketch = build_sketch2d_from_sub_calls(&calls);
        let Sketch2d::Prim(Sketch2dPrimitive::Path { segments, closed }) = &sketch else {
            panic!("expected a Path primitive, got {sketch:?}");
        };
        assert!(!*closed);
        assert!(!segments.is_empty());
        assert!(segments.iter().all(|s| matches!(
            s,
            SketchSegment::QuadBezier { .. } | SketchSegment::Line { .. }
        )));
        // No NaN/Inf anywhere.
        for s in segments {
            for k in 0..=8 {
                let p = s.point_at(k as f32 / 8.0);
                assert!(p[0].is_finite() && p[1].is_finite(), "NaN/Inf in lowering");
            }
        }
    }

    #[test]
    fn bspline_clamped_endpoints_are_first_and_last_control() {
        // A clamped open cubic B-spline interpolates its first/last
        // control points — the lowered chain starts at P0 and ends at
        // P_{n-1}. (This is what makes endpoint Coincident a real
        // endpoint constraint.)
        let cp = [
            [0.0, 0.0],
            [10.0, 20.0],
            [20.0, -20.0],
            [30.0, 0.0],
            [40.0, 10.0],
        ];
        let segs = bspline_quad_beziers(&cp, false, 0.01);
        assert!(!segs.is_empty());
        let start = segs[0].point_at(0.0);
        let end = segs.last().unwrap().point_at(1.0);
        assert!(
            (start[0] - 0.0).abs() < 1e-3 && (start[1] - 0.0).abs() < 1e-3,
            "start {start:?}"
        );
        assert!(
            (end[0] - 40.0).abs() < 1e-3 && (end[1] - 10.0).abs() < 1e-3,
            "end {end:?}"
        );
    }

    // ---- F23 endpoint tangent: control-polygon end-legs ---------------

    /// Apply a spline tool call and return the resulting entities.
    fn spline_entities(params: Value) -> Vec<fielddraft_solver::Entity> {
        let tools = SketchToolRegistry::with_builtins();
        let mut state = SketchState::with_origin();
        let call = FeatureCall::new("sketch_spline", id(1), params);
        tools
            .apply_all(&mut state, std::slice::from_ref(&call))
            .unwrap();
        state.entities
    }

    /// Collect (start, end) of every Line entity.
    fn legs(entities: &[fielddraft_solver::Entity]) -> Vec<(u32, u32)> {
        entities
            .iter()
            .filter_map(|e| match e {
                fielddraft_solver::Entity::Line { start, end, .. } => Some((start.0, end.0)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn spline_control_open_n4_has_two_legs() {
        // control + open, N>=3 → 2 legs: start=Line(cp0,cp1),
        // end=Line(cp[N-2],cp[N-1]). Origin is id 0, so cp ids are 1..=4.
        let ents = spline_entities(json!({
            "points": [0.0, 0.0, 10.0, 20.0, 20.0, -20.0, 30.0, 0.0],
            "closed": false,
            "mode": "control"
        }));
        let pts = ents
            .iter()
            .filter(|e| matches!(e, fielddraft_solver::Entity::Point { .. }))
            .count();
        assert_eq!(pts, 4, "4 control points");
        // cp ids: 1,2,3,4. start leg = (1,2), end leg = (3,4).
        assert_eq!(legs(&ents), vec![(1, 2), (3, 4)]);
    }

    #[test]
    fn spline_control_open_n2_has_one_leg() {
        // N==2: start and end coincide → exactly one leg.
        let ents = spline_entities(json!({
            "points": [0.0, 0.0, 10.0, 20.0],
            "closed": false,
            "mode": "control"
        }));
        // cp ids 1,2; single leg (1,2).
        assert_eq!(legs(&ents), vec![(1, 2)]);
    }

    #[test]
    fn spline_interpolate_open_n4_has_two_legs() {
        // interpolate + open, N>=3 → 2 legs, identical in form to control
        // mode: Catmull-Rom endpoint tangent P'(0)=0.5·(p1−p0) is parallel
        // to cp0→cp1 and P'(1)=0.5·(p[n-1]−p[n-2]) parallel to cp[N-2]→
        // cp[N-1]. Origin is id 0, so cp ids are 1..=4.
        let ents = spline_entities(json!({
            "points": [0.0, 0.0, 10.0, 20.0, 20.0, -20.0, 30.0, 0.0],
            "closed": false,
            "mode": "interpolate"
        }));
        let pts = ents
            .iter()
            .filter(|e| matches!(e, fielddraft_solver::Entity::Point { .. }))
            .count();
        assert_eq!(pts, 4, "4 control points");
        // cp ids: 1,2,3,4. start leg = (1,2), end leg = (3,4).
        assert_eq!(legs(&ents), vec![(1, 2), (3, 4)]);
    }

    #[test]
    fn spline_interpolate_open_n2_has_one_leg() {
        // N==2: start and end coincide → exactly one leg.
        let ents = spline_entities(json!({
            "points": [0.0, 0.0, 10.0, 20.0],
            "closed": false,
            "mode": "interpolate"
        }));
        // cp ids 1,2; single leg (1,2).
        assert_eq!(legs(&ents), vec![(1, 2)]);
    }

    #[test]
    fn spline_interpolate_closed_has_no_legs() {
        // Closed (periodic) interpolation spline: no free endpoints → no legs.
        let ents = spline_entities(json!({
            "points": [0.0, 0.0, 10.0, 20.0, 20.0, -20.0, 30.0, 0.0],
            "closed": true,
            "mode": "interpolate"
        }));
        assert!(
            legs(&ents).is_empty(),
            "closed interpolate spline: no endpoint legs"
        );
    }

    #[test]
    fn spline_control_closed_has_no_legs() {
        let ents = spline_entities(json!({
            "points": [0.0, 0.0, 10.0, 20.0, 20.0, -20.0, 30.0, 0.0],
            "closed": true,
            "mode": "control"
        }));
        assert!(legs(&ents).is_empty(), "closed spline: no endpoint legs");
    }

    /// Collect (poles, degree) of every BSpline entity.
    fn bsplines(entities: &[fielddraft_solver::Entity]) -> Vec<(Vec<u32>, u32)> {
        entities
            .iter()
            .filter_map(|e| match e {
                fielddraft_solver::Entity::BSpline { poles, degree, .. } => {
                    Some((poles.iter().map(|p| p.0).collect(), *degree))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn spline_control_open_n4_allocates_cubic_bspline_after_legs() {
        // control + open, N=4 → one BSpline (poles = cp ids 1..=4, degree 3),
        // allocated AFTER the two legs so the leg ids are unchanged.
        let ents = spline_entities(json!({
            "points": [0.0, 0.0, 10.0, 20.0, 20.0, -20.0, 30.0, 0.0],
            "closed": false,
            "mode": "control"
        }));
        assert_eq!(legs(&ents), vec![(1, 2), (3, 4)]);
        assert_eq!(bsplines(&ents), vec![(vec![1, 2, 3, 4], 3)]);
    }

    #[test]
    fn spline_control_open_clamps_degree_to_pole_count() {
        // N=2 → degree 1 (a polyline), N=3 → degree 2 (quadratic), matching
        // `bspline_quad_beziers`'s `3.min(n-1)` so solver knots == drawn curve.
        let n2 = spline_entities(json!({
            "points": [0.0, 0.0, 10.0, 20.0],
            "closed": false, "mode": "control"
        }));
        assert_eq!(bsplines(&n2), vec![(vec![1, 2], 1)]);
        let n3 = spline_entities(json!({
            "points": [0.0, 0.0, 10.0, 20.0, 20.0, 0.0],
            "closed": false, "mode": "control"
        }));
        assert_eq!(bsplines(&n3), vec![(vec![1, 2, 3], 2)]);
    }

    #[test]
    fn spline_interpolate_open_has_no_bspline_entity() {
        // Catmull-Rom passes THROUGH its points (not poles) → no solver
        // B-spline entity; endpoint legs remain the only G1 handle.
        let ents = spline_entities(json!({
            "points": [0.0, 0.0, 10.0, 20.0, 20.0, -20.0, 30.0, 0.0],
            "closed": false, "mode": "interpolate"
        }));
        assert!(bsplines(&ents).is_empty());
    }

    #[test]
    fn spline_control_closed_has_no_bspline_entity() {
        // Periodic knot convention differs → no solver entity for closed yet.
        let ents = spline_entities(json!({
            "points": [0.0, 0.0, 10.0, 20.0, 20.0, -20.0, 30.0, 0.0],
            "closed": true, "mode": "control"
        }));
        assert!(bsplines(&ents).is_empty());
    }

    #[test]
    fn tangent_constraint_on_leg_roundtrips_through_serde() {
        // A Tangent (and Parallel) constraint referencing a leg line id
        // round-trips through serde JSON. The leg is just a `line`, so
        // existing line constraints apply unchanged.
        let tangent = Constraint::Tangent {
            id: ConstraintId(7),
            line: EntityId(3),
            curve: EntityId(9),
        };
        let json = serde_json::to_value(&tangent).unwrap();
        let back: Constraint = serde_json::from_value(json).unwrap();
        assert_eq!(tangent, back);

        let parallel = Constraint::Parallel {
            id: ConstraintId(8),
            a: EntityId(3),
            b: EntityId(5),
        };
        let json = serde_json::to_value(&parallel).unwrap();
        let back: Constraint = serde_json::from_value(json).unwrap();
        assert_eq!(parallel, back);
    }

    #[test]
    fn build_geometry_is_unchanged_by_leg_lines() {
        // The leg lines live ONLY in the entity/constraint world. The
        // extrude profile is lowered from the sub-call params via
        // build_sketch2d_from_sub_calls, which never reads entities, so
        // the resulting Path primitive is identical whether or not legs
        // would have been allocated. We assert the lowered Path here is
        // exactly what bspline_control_mode_lowers_to_quad_path expects.
        let calls = vec![FeatureCall::new(
            "sketch_spline",
            id(1),
            json!({
                "points": [0.0, 0.0, 10.0, 20.0, 20.0, -20.0, 30.0, 0.0],
                "closed": false,
                "mode": "control"
            }),
        )];
        let sketch = build_sketch2d_from_sub_calls(&calls);
        let Sketch2d::Prim(Sketch2dPrimitive::Path { segments, closed }) = &sketch else {
            panic!("expected a Path primitive, got {sketch:?}");
        };
        assert!(!*closed);
        assert!(!segments.is_empty());
        // All segments are still Line/QuadBezier — no spurious leg geometry.
        assert!(segments.iter().all(|s| matches!(
            s,
            SketchSegment::QuadBezier { .. } | SketchSegment::Line { .. }
        )));
        // Endpoints unchanged: chain starts at cp0 and ends at cp[N-1].
        let start = segments[0].point_at(0.0);
        let end = segments.last().unwrap().point_at(1.0);
        assert!((start[0] - 0.0).abs() < 1e-3 && (start[1] - 0.0).abs() < 1e-3);
        assert!((end[0] - 30.0).abs() < 1e-3 && (end[1] - 0.0).abs() < 1e-3);
    }

    /// Min distance from `q` to the polyline of lowered segments.
    fn dist_to_chain(segs: &[SketchSegment], q: [f64; 2]) -> f64 {
        let mut best = f64::INFINITY;
        for s in segs {
            let mut prev = s.point_at(0.0);
            for k in 1..=16 {
                let cur = s.point_at(k as f32 / 16.0);
                // distance from q to segment prev→cur
                let ax = prev[0] as f64;
                let ay = prev[1] as f64;
                let bx = cur[0] as f64;
                let by = cur[1] as f64;
                let dx = bx - ax;
                let dy = by - ay;
                let len2 = dx * dx + dy * dy;
                let t = if len2 < 1e-12 {
                    0.0
                } else {
                    (((q[0] - ax) * dx + (q[1] - ay) * dy) / len2).clamp(0.0, 1.0)
                };
                let px = ax + t * dx;
                let py = ay + t * dy;
                best = best.min(((q[0] - px).powi(2) + (q[1] - py).powi(2)).sqrt());
                prev = cur;
            }
        }
        best
    }

    #[test]
    fn bspline_midparam_matches_de_boor() {
        // The exactly-lowered curve must pass through the de Boor
        // evaluation of the same B-spline at several parameters,
        // including the mid-parameter, within the flatness tolerance.
        // (de Boor is the analytic ground truth; the lowered chain
        // must lie on it.)
        let cp = [
            [0.0, 0.0],
            [10.0, 30.0],
            [30.0, 30.0],
            [40.0, 0.0],
            [50.0, 20.0],
        ];
        let segs = bspline_quad_beziers(&cp, false, 0.005);
        for &t in &[0.0, 0.25, 0.5, 0.75, 1.0] {
            let analytic = bspline_eval_open(&cp, 3, t);
            let d = dist_to_chain(&segs, analytic);
            assert!(
                d < 0.05,
                "de Boor t={t} {analytic:?} is {d} from lowered chain"
            );
        }
        // Spot-check the hand-relevant mid value sits on the curve.
        let mid = bspline_eval_open(&cp, 3, 0.5);
        assert!(mid[0].is_finite() && mid[1].is_finite());
    }

    #[test]
    fn bspline_two_control_points_is_a_line() {
        let cp = [[0.0, 0.0], [10.0, 5.0]];
        let segs = bspline_quad_beziers(&cp, false, 0.05);
        assert_eq!(segs.len(), 1);
        assert!(matches!(segs[0], SketchSegment::Line { .. }));
        let start = segs[0].point_at(0.0);
        let end = segs[0].point_at(1.0);
        assert!((start[0] - 0.0).abs() < 1e-4 && (end[0] - 10.0).abs() < 1e-4);
    }

    #[test]
    fn bspline_three_control_points_is_degree_two() {
        // 3 control points → degree 2; the lowered curve still starts
        // and ends at the first/last control point (clamped quad).
        let cp = [[0.0, 0.0], [5.0, 10.0], [10.0, 0.0]];
        let segs = bspline_quad_beziers(&cp, false, 0.05);
        assert!(!segs.is_empty());
        let start = segs[0].point_at(0.0);
        let end = segs.last().unwrap().point_at(1.0);
        assert!((start[0] - 0.0).abs() < 1e-3 && (start[1] - 0.0).abs() < 1e-3);
        assert!((end[0] - 10.0).abs() < 1e-3 && (end[1] - 0.0).abs() < 1e-3);
        // Mid bows up toward the middle control point.
        let mid = seg_point(&segs, 0.5);
        assert!(mid[1] > 0.5, "degree-2 curve should bow up, mid={mid:?}");
    }

    #[test]
    fn bspline_collinear_stays_straight() {
        // Collinear control points → a straight curve (all samples on
        // the x-axis), no division-by-zero / NaN.
        let cp = [[0.0, 0.0], [10.0, 0.0], [20.0, 0.0], [30.0, 0.0]];
        let segs = bspline_quad_beziers(&cp, false, 0.05);
        assert!(!segs.is_empty());
        for s in &segs {
            for k in 0..=8 {
                let p = s.point_at(k as f32 / 8.0);
                assert!(p[1].abs() < 1e-3, "off-axis point {p:?}");
                assert!(p[0].is_finite() && p[1].is_finite());
            }
        }
    }

    #[test]
    fn bspline_closed_wraps_into_a_region() {
        // A closed (periodic) cubic B-spline over a square-ish control
        // polygon lowers to a closed Path whose interior contains the
        // centroid (extrudable region), and the curve forms a loop.
        let calls = vec![FeatureCall::new(
            "sketch_spline",
            id(1),
            json!({
                "points": [-10.0, -10.0, 10.0, -10.0, 10.0, 10.0, -10.0, 10.0],
                "closed": true,
                "mode": "control"
            }),
        )];
        let sketch = build_sketch2d_from_sub_calls(&calls);
        let Sketch2d::Prim(Sketch2dPrimitive::Path { segments, closed }) = &sketch else {
            panic!("expected closed Path, got {sketch:?}");
        };
        assert!(*closed, "closed B-spline path must be closed");
        assert!(segments.len() >= 4);
        // The wrap is a true loop: end of last seg ≈ start of first.
        let first = segments[0].point_at(0.0);
        let last = segments.last().unwrap().point_at(1.0);
        let gap = ((first[0] - last[0]).powi(2) + (first[1] - last[1]).powi(2)).sqrt();
        assert!(gap < 1e-2, "closed loop gap {gap}");
        // Centre inside the region.
        assert!(sketch.sdf([0.0, 0.0]) < 0.0, "closed spline centre inside");
        // No NaN/Inf.
        for s in segments {
            for k in 0..=8 {
                let p = s.point_at(k as f32 / 8.0);
                assert!(p[0].is_finite() && p[1].is_finite());
            }
        }
    }

    #[test]
    fn bspline_too_few_control_points_no_panic() {
        // < 2 control points → empty (no panic), and a closed 3-point
        // spline lowers without panicking.
        assert!(bspline_quad_beziers(&[[0.0, 0.0]], false, 0.05).is_empty());
        assert!(bspline_quad_beziers(&[], true, 0.05).is_empty());
        let segs = bspline_quad_beziers(&[[0.0, 0.0], [10.0, 0.0], [5.0, 8.0]], true, 0.05);
        assert!(!segs.is_empty());
        for s in &segs {
            let p = s.point_at(0.5);
            assert!(p[0].is_finite() && p[1].is_finite());
        }
    }

    #[test]
    fn bspline_endpoint_coincident_constraint_wires_to_first_control() {
        // F23-full solver: a clamped B-spline's first control point IS
        // the curve endpoint AND a registered point entity (control mode
        // registers one point per control point, same as interpolate).
        // So a Coincident constraint referencing that entity id is a
        // true endpoint constraint the solver acts on. Here we confirm
        // the constraint wires through to the spline's first control
        // point entity (id 2 after origin=0, free point=1, spline
        // controls=2..=5).
        let tools = SketchToolRegistry::with_builtins();
        let mut state = SketchState::with_origin();
        let calls = vec![
            FeatureCall::new("sketch_point", id(1), json!({ "at": [5.0, 5.0] })),
            FeatureCall::new(
                "sketch_spline",
                id(2),
                json!({
                    "points": [0.0, 0.0, 10.0, 20.0, 20.0, -20.0, 30.0, 0.0],
                    "closed": false,
                    "mode": "control"
                }),
            ),
            FeatureCall::new(
                "sketch_constraint_coincident",
                id(3),
                json!({ "a": 1, "b": 2 }),
            ),
        ];
        tools.apply_all(&mut state, &calls).unwrap();
        // Entity 2 is the spline's FIRST control point, at (0,0) — the
        // clamped curve endpoint.
        let first_ctrl = state.entities.iter().find(|e| {
            matches!(
                e,
                fielddraft_solver::Entity::Point {
                    id: EntityId(2),
                    ..
                }
            )
        });
        if let Some(fielddraft_solver::Entity::Point { at, .. }) = first_ctrl {
            assert!((at.x - 0.0).abs() < 1e-6 && (at.y - 0.0).abs() < 1e-6);
        } else {
            panic!("entity 2 is not the spline's first control point");
        }
        // The coincident constraint binds the free point (1) to that
        // first control point (2).
        assert!(state.constraints.iter().any(|c| matches!(
            c,
            Constraint::Coincident {
                a: EntityId(1),
                b: EntityId(2),
                ..
            }
        )));
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
    fn loop_inference_closes_a_rectangle_of_free_lines() {
        // Four free lines whose endpoints coincide exactly — drawn in
        // mixed directions to exercise the reversed() chaining. K7.1
        // must stitch them into one closed region.
        let calls = vec![
            FeatureCall::new(
                "sketch_line",
                id(1),
                json!({ "start": [0.0, 0.0], "end": [10.0, 0.0] }),
            ),
            FeatureCall::new(
                "sketch_line",
                id(2),
                json!({ "start": [10.0, 8.0], "end": [10.0, 0.0] }),
            ),
            FeatureCall::new(
                "sketch_line",
                id(3),
                json!({ "start": [10.0, 8.0], "end": [0.0, 8.0] }),
            ),
            FeatureCall::new(
                "sketch_line",
                id(4),
                json!({ "start": [0.0, 0.0], "end": [0.0, 8.0] }),
            ),
        ];
        let sketch = build_sketch2d_from_sub_calls(&calls);
        assert!(sketch.sdf([5.0, 4.0]) < 0.0, "rectangle centre inside");
        assert!(sketch.sdf([15.0, 4.0]) > 0.0, "right of rectangle outside");
        assert!(sketch.sdf([5.0, -3.0]) > 0.0, "below rectangle outside");
    }

    #[test]
    fn loop_inference_mixes_lines_and_open_spline() {
        // A vase-like profile: lines on three sides, an open
        // interpolated spline as the fourth. The chain closes through
        // the spline's endpoints.
        let calls = vec![
            FeatureCall::new(
                "sketch_line",
                id(1),
                json!({ "start": [0.0, 0.0], "end": [0.0, 20.0] }),
            ),
            FeatureCall::new(
                "sketch_line",
                id(2),
                json!({ "start": [0.0, 20.0], "end": [10.0, 20.0] }),
            ),
            FeatureCall::new(
                "sketch_spline",
                id(3),
                json!({
                    "points": [10.0, 20.0, 12.0, 10.0, 8.0, 0.0],
                    "closed": false,
                    "mode": "interpolate"
                }),
            ),
            FeatureCall::new(
                "sketch_line",
                id(4),
                json!({ "start": [8.0, 0.0], "end": [0.0, 0.0] }),
            ),
        ];
        let sketch = build_sketch2d_from_sub_calls(&calls);
        assert!(sketch.sdf([5.0, 10.0]) < 0.0, "interior inside");
        assert!(sketch.sdf([-5.0, 10.0]) > 0.0, "left of axis outside");
        assert!(sketch.sdf([20.0, 10.0]) > 0.0, "far right outside");
    }

    #[test]
    fn loop_inference_leaves_open_chains_empty() {
        // Three sides of a square — endpoint chain never closes, so
        // no region may be invented.
        let calls = vec![
            FeatureCall::new(
                "sketch_line",
                id(1),
                json!({ "start": [0.0, 0.0], "end": [10.0, 0.0] }),
            ),
            FeatureCall::new(
                "sketch_line",
                id(2),
                json!({ "start": [10.0, 0.0], "end": [10.0, 8.0] }),
            ),
            FeatureCall::new(
                "sketch_line",
                id(3),
                json!({ "start": [10.0, 8.0], "end": [0.0, 8.0] }),
            ),
        ];
        assert!(matches!(
            build_sketch2d_from_sub_calls(&calls),
            Sketch2d::Empty
        ));
    }

    #[test]
    fn loop_inference_two_disjoint_loops_union() {
        // Two separate closed triangles of free lines → a Union with
        // both interiors filled.
        let mut calls = Vec::new();
        let tri = |o: f32, base: u32| -> Vec<FeatureCall> {
            vec![
                FeatureCall::new(
                    "sketch_line",
                    id(base),
                    json!({ "start": [o, 0.0], "end": [o + 6.0, 0.0] }),
                ),
                FeatureCall::new(
                    "sketch_line",
                    id(base + 1),
                    json!({ "start": [o + 6.0, 0.0], "end": [o + 3.0, 6.0] }),
                ),
                FeatureCall::new(
                    "sketch_line",
                    id(base + 2),
                    json!({ "start": [o + 3.0, 6.0], "end": [o, 0.0] }),
                ),
            ]
        };
        calls.extend(tri(0.0, 1));
        calls.extend(tri(20.0, 4));
        let sketch = build_sketch2d_from_sub_calls(&calls);
        assert!(sketch.sdf([3.0, 2.0]) < 0.0, "first triangle inside");
        assert!(sketch.sdf([23.0, 2.0]) < 0.0, "second triangle inside");
        assert!(sketch.sdf([13.0, 2.0]) > 0.0, "between triangles outside");
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
            FeatureCall::new("sketch_constraint_horizontal", id(3), json!({ "line": 3 })),
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
        let calls = vec![FeatureCall::new(
            "sketch_mirror",
            id(1),
            json!({ "axis": 0.0 }),
        )];
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
        assert!(
            sketch.sdf([13.0, 0.0]) < 0.0,
            "point inside the grown profile"
        );
        // Beyond the grown boundary it is still outside.
        assert!(
            sketch.sdf([16.0, 0.0]) > 0.0,
            "point past the grown boundary"
        );
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
        assert!(
            sketch.sdf([50.0, 0.0]) > 0.0,
            "construction circle not extruded"
        );
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
        assert!(
            xy.to_transform().is_identity(),
            "xy datum is the identity placement"
        );

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
                &[FeatureCall::new(
                    "box",
                    id(1),
                    json!({ "half_extents": [5.0, 5.0, 5.0] }),
                )],
                None,
            )
            .unwrap();
        let woven = crate::body::weave_all(&ctx.features);
        let (brep, placement) = &woven[0];
        for i in 0..brep.face_count() {
            let frame = face_plane_frame(brep, crate::brep::FaceId(i as u32), placement);
            assert!(
                is_orthonormal(&frame),
                "face {i} frame not orthonormal: {frame:?}"
            );
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
        assert_eq!(
            data.frame,
            SketchFrame::IDENTITY,
            "xy sketch is the identity frame"
        );
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
        registry
            .regenerate(&mut ctx, std::slice::from_ref(&box_call), None)
            .unwrap();

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
        assert!(
            (frame.normal[2] - 1.0).abs() < 1e-3,
            "sketch normal +Z, got {:?}",
            frame.normal
        );
        assert!(
            (frame.origin[2] - 5.0).abs() < 1e-2,
            "sketch origin on the +Z face, got {:?}",
            frame.origin
        );

        // The extrude inherits a non-identity placement (the frame),
        // unlike an xy sketch — its translate lifts the profile to z≈5.
        let extrude = ctx.features.iter().find(|f| f.id == extrude_id).unwrap();
        assert!(
            (extrude.transform.translate[2] - 5.0).abs() < 1e-2,
            "extrude placement lifted to the face, got {:?}",
            extrude.transform.translate
        );
    }

    // ─── sketch_path (F22-full) ─────────────────────────────────────

    #[test]
    fn sketch_path_def_round_trips_through_serde() {
        // A def with a closed contour mixing line + quad segments must
        // serialise and parse back identically through the FeatureSchema
        // generic (serde) path.
        let original = SketchPathDef {
            contours: vec![PathContourDef {
                start: [0.0, 0.0],
                segments: vec![
                    PathSegmentDef::Line { to: [10.0, 0.0] },
                    PathSegmentDef::Quad {
                        ctrl: [10.0, 10.0],
                        to: [0.0, 10.0],
                    },
                    PathSegmentDef::Line { to: [0.0, 0.0] },
                ],
                closed: true,
            }],
        };
        let value = json!({
            "contours": [serde_json::to_value(&original.contours[0]).unwrap()]
        });
        let parsed = SketchPathDef::parse(&value).expect("parse");
        assert_eq!(parsed.contours, original.contours);
        // Tagged-enum wire shape: quad carries kind/ctrl/to.
        let seg_json = serde_json::to_value(PathSegmentDef::Quad {
            ctrl: [1.0, 2.0],
            to: [3.0, 4.0],
        })
        .unwrap();
        assert_eq!(seg_json["kind"], "quad");
        assert_eq!(seg_json["ctrl"], json!([1.0, 2.0]));
    }

    /// Collect all registered Point entity coords in entity order.
    #[cfg(test)]
    fn path_point_coords(state: &SketchState) -> Vec<(f64, f64)> {
        state
            .entities
            .iter()
            .filter_map(|e| match e {
                fielddraft_solver::Entity::Point { at, .. } => Some((at.x, at.y)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn sketch_path_tool_allocates_canonical_vertex_entities() {
        // CANONICAL order: contour start, then per segment
        // (Quad → ctrl, to; Line → to). A one-contour path with
        // [line, quad, line] => start + (to) + (ctrl, to) + (to) = 5
        // point entities in that exact order & coords.
        let tools = SketchToolRegistry::with_builtins();
        let mut state = SketchState::new(); // no origin, keep coords clean
        let call = FeatureCall::new(
            "sketch_path",
            id(1),
            json!({
                "contours": [
                    { "start": [0.0, 0.0], "closed": false, "segments": [
                        { "kind": "line", "to": [10.0, 0.0] },
                        { "kind": "quad", "ctrl": [15.0, 5.0], "to": [20.0, 10.0] },
                        { "kind": "line", "to": [0.0, 10.0] }
                    ]}
                ]
            }),
        );
        tools
            .apply_all(&mut state, std::slice::from_ref(&call))
            .unwrap();
        let pts = path_point_coords(&state);
        assert_eq!(
            pts,
            vec![
                (0.0, 0.0),   // start
                (10.0, 0.0),  // line.to
                (15.0, 5.0),  // quad.ctrl
                (20.0, 10.0), // quad.to
                (0.0, 10.0),  // line.to
            ]
        );
    }

    #[test]
    fn sketch_path_tool_two_contours_allocate_in_order() {
        let tools = SketchToolRegistry::with_builtins();
        let mut state = SketchState::new();
        let call = FeatureCall::new(
            "sketch_path",
            id(1),
            json!({
                "contours": [
                    { "start": [0.0, 0.0], "closed": false, "segments": [
                        { "kind": "line", "to": [10.0, 0.0] }
                    ]},
                    { "start": [3.0, 3.0], "closed": false, "segments": [
                        { "kind": "quad", "ctrl": [5.0, 5.0], "to": [7.0, 3.0] }
                    ]}
                ]
            }),
        );
        tools
            .apply_all(&mut state, std::slice::from_ref(&call))
            .unwrap();
        let pts = path_point_coords(&state);
        assert_eq!(
            pts,
            vec![
                (0.0, 0.0),  // c0 start
                (10.0, 0.0), // c0 line.to
                (3.0, 3.0),  // c1 start
                (5.0, 5.0),  // c1 quad.ctrl
                (7.0, 3.0),  // c1 quad.to
            ]
        );
    }

    #[test]
    fn sketch_path_tool_closed_contour_adds_no_closing_entity() {
        // A closed contour's implicit closing edge returns to `start`
        // and allocates NO extra entity.
        let tools = SketchToolRegistry::with_builtins();
        let mut state = SketchState::new();
        let call = FeatureCall::new(
            "sketch_path",
            id(1),
            json!({
                "contours": [
                    { "start": [0.0, 0.0], "closed": true, "segments": [
                        { "kind": "line", "to": [10.0, 0.0] },
                        { "kind": "line", "to": [10.0, 10.0] },
                        { "kind": "line", "to": [0.0, 10.0] }
                    ]}
                ]
            }),
        );
        tools
            .apply_all(&mut state, std::slice::from_ref(&call))
            .unwrap();
        let pts = path_point_coords(&state);
        // start + 3 line.to = 4 (NOT 5 — closing edge adds nothing).
        assert_eq!(pts.len(), 4);
        assert_eq!(
            pts,
            vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]
        );
    }

    #[test]
    fn build_geometry_is_unchanged_by_path_vertex_entities() {
        // The vertex point entities live ONLY in the entity world. The
        // extrude profile is lowered from the sub-call params via
        // build_sketch2d_from_sub_calls, which never reads entities, so
        // the lowered Path is identical regardless of vertex allocation.
        let calls = vec![FeatureCall::new(
            "sketch_path",
            id(1),
            json!({
                "contours": [
                    { "start": [0.0, 0.0], "closed": true, "segments": [
                        { "kind": "line", "to": [40.0, 0.0] },
                        { "kind": "line", "to": [40.0, 20.0] },
                        { "kind": "quad", "ctrl": [20.0, 40.0], "to": [0.0, 20.0] }
                    ]}
                ]
            }),
        )];
        let sketch = build_sketch2d_from_sub_calls(&calls);
        let Sketch2d::Prim(Sketch2dPrimitive::Path { segments, closed }) = &sketch else {
            panic!("expected a Path primitive, got {sketch:?}");
        };
        assert!(*closed);
        // Same segment make-up as before: only Line/QuadBezier, no
        // spurious vertex geometry.
        assert!(segments.iter().all(|s| matches!(
            s,
            SketchSegment::QuadBezier { .. } | SketchSegment::Line { .. }
        )));
        // Quad apex region check unchanged from the geometry test.
        assert!(sketch.sdf([20.0, 26.0]) < 0.0, "under quad bulge inside");
        assert!(sketch.sdf([20.0, 38.0]) > 0.0, "above bulge outside");
    }

    #[test]
    fn build_sketch_path_single_closed_contour_fills() {
        // A single closed triangle contour lowers to one Path region.
        let calls = vec![FeatureCall::new(
            "sketch_path",
            id(1),
            json!({
                "contours": [
                    { "start": [0.0, 0.0], "closed": true, "segments": [
                        { "kind": "line", "to": [20.0, 0.0] },
                        { "kind": "line", "to": [10.0, 20.0] }
                    ]}
                ]
            }),
        )];
        let sketch = build_sketch2d_from_sub_calls(&calls);
        let Sketch2d::Prim(Sketch2dPrimitive::Path { closed, .. }) = &sketch else {
            panic!("expected a closed Path primitive, got {sketch:?}");
        };
        assert!(*closed);
        // Interior centroid-ish point inside, far point outside.
        assert!(sketch.sdf([10.0, 6.0]) < 0.0, "interior inside");
        assert!(sketch.sdf([100.0, 100.0]) > 0.0, "far point outside");
    }

    #[test]
    fn build_sketch_path_implicitly_closes_unclosed_segment_chain() {
        // The last segment doesn't return to start; the lowering must
        // append the closing edge so the region fills.
        let d = SketchPathDef {
            contours: vec![PathContourDef {
                start: [0.0, 0.0],
                segments: vec![
                    PathSegmentDef::Line { to: [10.0, 0.0] },
                    PathSegmentDef::Line { to: [10.0, 10.0] },
                    PathSegmentDef::Line { to: [0.0, 10.0] },
                ],
                closed: true,
            }],
        };
        let segs = path_contour_segments(&d.contours[0]).unwrap();
        // 3 explicit + 1 implicit closing line.
        assert_eq!(segs.len(), 4);
        let tree = build_sketch_path(&d).unwrap();
        assert!(tree.sdf([5.0, 5.0]) < 0.0, "square interior inside");
    }

    #[test]
    fn build_sketch_path_donut_yields_difference_with_real_hole() {
        // Outer 0..20 square with an inner 5..15 square hole. The
        // build must subtract the hole: a point in the ring is inside,
        // a point in the hole is outside.
        let calls = vec![FeatureCall::new(
            "sketch_path",
            id(1),
            json!({
                "contours": [
                    { "start": [0.0, 0.0], "closed": true, "segments": [
                        { "kind": "line", "to": [20.0, 0.0] },
                        { "kind": "line", "to": [20.0, 20.0] },
                        { "kind": "line", "to": [0.0, 20.0] }
                    ]},
                    { "start": [5.0, 5.0], "closed": true, "segments": [
                        { "kind": "line", "to": [15.0, 5.0] },
                        { "kind": "line", "to": [15.0, 15.0] },
                        { "kind": "line", "to": [5.0, 15.0] }
                    ]}
                ]
            }),
        )];
        let sketch = build_sketch2d_from_sub_calls(&calls);
        let Sketch2d::Difference(_, _) = &sketch else {
            panic!("expected a Difference (outer − hole), got {sketch:?}");
        };
        // Point in the ring (between outer and inner): inside.
        assert!(sketch.sdf([2.0, 10.0]) < 0.0, "ring inside");
        // Point in the hole centre: outside.
        assert!(sketch.sdf([10.0, 10.0]) > 0.0, "hole outside");
        // Point far outside the outer: outside.
        assert!(sketch.sdf([100.0, 100.0]) > 0.0, "far outside");
    }

    #[test]
    fn build_sketch_path_island_in_hole_is_solid_again() {
        // Even-odd nesting: outer (depth 0) ⊃ hole (depth 1) ⊃ island
        // (depth 2). The island must be solid again.
        let outer = |sz: f32, o: f32| {
            json!({ "start": [o, o], "closed": true, "segments": [
                { "kind": "line", "to": [o + sz, o] },
                { "kind": "line", "to": [o + sz, o + sz] },
                { "kind": "line", "to": [o, o + sz] }
            ]})
        };
        let calls = vec![FeatureCall::new(
            "sketch_path",
            id(1),
            json!({ "contours": [ outer(40.0, 0.0), outer(24.0, 8.0), outer(8.0, 16.0) ] }),
        )];
        let sketch = build_sketch2d_from_sub_calls(&calls);
        // Island centre (20,20): solid. Ring between hole & island
        // (e.g. (10,20) inside hole region): hole → outside. Outer
        // ring (2,20): inside.
        assert!(sketch.sdf([20.0, 20.0]) < 0.0, "island solid");
        assert!(sketch.sdf([11.0, 20.0]) > 0.0, "hole outside");
        assert!(sketch.sdf([2.0, 20.0]) < 0.0, "outer ring inside");
    }

    #[test]
    fn build_sketch_path_open_lone_contour_skipped() {
        // An open contour alone forms no region.
        let calls = vec![FeatureCall::new(
            "sketch_path",
            id(1),
            json!({
                "contours": [
                    { "start": [0.0, 0.0], "closed": false, "segments": [
                        { "kind": "line", "to": [10.0, 0.0] },
                        { "kind": "quad", "ctrl": [20.0, 10.0], "to": [30.0, 0.0] }
                    ]}
                ]
            }),
        )];
        assert!(matches!(
            build_sketch2d_from_sub_calls(&calls),
            Sketch2d::Empty
        ));
    }

    #[test]
    fn build_sketch_path_quad_segment_curves_outward() {
        // A closed contour whose top edge is a quad bulging up to y=30
        // must include points above the chord (y>20) in its region.
        let calls = vec![FeatureCall::new(
            "sketch_path",
            id(1),
            json!({
                "contours": [
                    { "start": [0.0, 0.0], "closed": true, "segments": [
                        { "kind": "line", "to": [40.0, 0.0] },
                        { "kind": "line", "to": [40.0, 20.0] },
                        { "kind": "quad", "ctrl": [20.0, 40.0], "to": [0.0, 20.0] }
                    ]}
                ]
            }),
        )];
        let sketch = build_sketch2d_from_sub_calls(&calls);
        // The quad apex reaches y≈30 at x=20; a point at (20, 26) lies
        // under the bulge → inside.
        assert!(sketch.sdf([20.0, 26.0]) < 0.0, "under quad bulge inside");
        // A point well above the bulge is outside.
        assert!(sketch.sdf([20.0, 38.0]) > 0.0, "above bulge outside");
    }
}
