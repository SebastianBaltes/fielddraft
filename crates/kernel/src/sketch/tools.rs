//! [`SketchToolKind`]-Trait, [`SketchToolRegistry`] und die
//! Geometrie-Tool-Definitionen (Point, Line, Rectangle, Circle, Arc,
//! Ellipse, Slot, Mirror, Offset, Project, Polygon, Spline, Path,
//! RegularPolygon) samt Pfad-Kontur-Helfern.

use std::collections::HashMap;

use serde_json::Value;

use fielddraft_protocol::{FeatureCall, FeatureId, Query};
use fielddraft_solver::{Constraint, EntityId, Point2, SketchState};

use crate::features::{FeatureError, FeatureSchema};
use crate::sketch2d::{Sketch2d, Sketch2dPrimitive, SketchSegment};

use super::constraints::{
    SketchAngleConstraintTool, SketchCoincidentConstraintTool, SketchCollinearConstraintTool,
    SketchConcentricConstraintTool, SketchDiameterConstraintTool, SketchDistanceConstraintTool,
    SketchEqualConstraintTool, SketchFixConstraintTool, SketchHorizontalConstraintTool,
    SketchMidpointConstraintTool, SketchParallelConstraintTool, SketchPerpendicularConstraintTool,
    SketchPointOnObjectConstraintTool, SketchRadiusConstraintTool, SketchSymmetricConstraintTool,
    SketchTangentConstraintTool, SketchVerticalConstraintTool,
};
use super::lower::lower_spline_segments;
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
        // Full arc entity model: centre + start + end points, then the
        // arc linking them. Allocation order (centre, start, end, arc)
        // is load-bearing — the TS mirror (buildDraftSketchState /
        // sketchGrips / rebuildSubCallsFromSolved) replicates it
        // byte-for-byte. Endpoint positions derive from the angular
        // parameterisation; the solver treats them as first-class
        // points so constraints can bind arc endpoints.
        let cx = d.center[0] as f64;
        let cy = d.center[1] as f64;
        let r = d.radius.max(0.0) as f64;
        let a0 = d.start_angle as f64;
        let a1 = a0 + d.sweep_angle as f64;
        let center = state.add_point(Point2::new(cx, cy));
        let start = state.add_point(Point2::new(cx + r * a0.cos(), cy + r * a0.sin()));
        let end = state.add_point(Point2::new(cx + r * a1.cos(), cy + r * a1.sin()));
        // Solver arcs sweep CCW from start to end — a negative-sweep
        // (CW) arc swaps the endpoint roles (mirrors the TS session).
        if d.sweep_angle >= 0.0 {
            state.add_arc(center, start, end, r);
        } else {
            state.add_arc(center, end, start, r);
        }
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
pub(crate) fn path_contour_segments(contour: &PathContourDef) -> Option<Vec<SketchSegment>> {
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
pub(crate) fn build_sketch_path(def: &SketchPathDef) -> Option<Sketch2d> {
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
