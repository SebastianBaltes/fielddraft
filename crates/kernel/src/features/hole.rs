//! `hole` — Onshape-style bore feature (#404).
//!
//! Scope so far: styles `simple` / `counterbore` / `countersink` (P3), types
//! `drilled` / `clearance` / `tapped` (P2 — the ISO tables in
//! [`hole_iso`](super::hole_iso) pick the bore Ø), a drill point at the blind
//! floor (P4), all four end conditions `blind` / `through_all` / `up_to_next` /
//! `up_to_entity` and the three start modes (P5).
//! A hole targets **one or more locations**
//! — sketch points (`qSketchPoint`), mate connectors (`qCreatedBy(<mc>)`) or
//! B-Rep vertices/faces — and drills an identical cutter at each, along that
//! anchor's axis (the sketch-plane normal / connector +Z / face normal). The
//! `style` / `type` / `end_type` fields are string enums with variant lists so
//! P2+ (counterbore / countersink, clearance / tapped, up-to-entity) extend
//! them by adding a variant without breaking the wire format.
//!
//! All physical measurements are [`Length`](fielddraft_protocol::Length)
//! (`{value, unit}` wire form) — a bare `f32` silently fails every UI call
//! (#405). Merge scope is mandatory (Onshape semantics): an empty `targets`
//! warns and drills nothing.

use serde_json::Value;

use fielddraft_protocol::{BoolOp, FeatureId, Primitive, Transform};

use crate::csg::{DecodedFeature, PrimitiveModifiers};

use super::merge_scope::target_ids_from_queries;
use super::{Context, FeatureError, FeatureKind, FeatureSchema, optional_display_name, positive_f32};

/// `hole` — parametric bore. Drills N identical cutters, one per location.
#[derive(FeatureSchema)]
#[schema(type = "hole")]
pub struct HoleDef {
    /// Hole style (geometry base form). `simple` is the bare shaft;
    /// `counterbore` stacks a wider flat-bottomed cylinder at the mouth,
    /// `countersink` a cone (P3). Each stacked part is its own Remove cutter
    /// coaxial with the shaft — a union of removes is the same solid as
    /// removing the union, so the stack needs no new SDF opcode: every
    /// evaluator (CPU tape, BVH, dual, fidget, TSL, CGAL) already understands
    /// `Primitive::Cylinder` and `Primitive::Cone`.
    #[param(
        name = "Style",
        default = ::std::string::String::from("simple"),
        variants = "simple=Simple,counterbore=Counterbore,countersink=Countersink"
    )]
    pub style: String,

    /// Hole type (purpose) — picks which ISO column drives the bore Ø:
    /// `drilled` = nominal, `clearance` = ISO 273 for the chosen [`fit`],
    /// `tapped` = the coarse-pitch tap drill (core hole).
    ///
    /// [`fit`]: HoleDef::fit
    #[param(
        name = "Type",
        default = ::std::string::String::from("drilled"),
        variants = "drilled=Drilled,clearance=Clearance,tapped=Tapped"
    )]
    pub hole_type: String,

    /// Clearance-hole fit (ISO 273 fine / medium / coarse rows). Only read
    /// for `hole_type = clearance`; ignored otherwise.
    #[param(
        name = "Fit",
        default = ::std::string::String::from("normal"),
        variants = "close=Close,normal=Normal,loose=Loose",
        visible_when = "hole_type=clearance"
    )]
    pub fit: String,

    /// Metric standard size (drives the bore diameter). `custom` switches to
    /// the free `diameter` field below. Variants mirror
    /// [`crate::features::hole_iso::ISO_METRIC`] (guarded by a schema/table
    /// drift test — the derive needs a literal, so the list is spelled out).
    #[param(
        name = "Size",
        default = ::std::string::String::from("M6"),
        variants = "custom=Custom (Ø),M1.6=M1.6,M2=M2,M2.5=M2.5,M3=M3,M3.5=M3.5,M4=M4,M5=M5,M6=M6,M8=M8,M10=M10,M12=M12,M14=M14,M16=M16,M18=M18,M20=M20,M22=M22,M24=M24"
    )]
    pub standard_size: String,

    /// Free bore diameter — used only when `standard_size = custom`.
    #[param(
        name = "Diameter",
        default = ::fielddraft_protocol::Length::mm(6.0),
        visible_when = "standard_size=custom"
    )]
    pub diameter: ::fielddraft_protocol::Length,

    /// Where the bore BEGINS — Onshape's "Start" block (P5). The bore always
    /// travels along the same axis; this only moves `t = 0`, i.e. the point
    /// [`depth`](HoleDef::depth) and the counterbore / countersink seat are
    /// measured from.
    ///
    /// * `sketch_plane` (default) — the anchor's own plane: the sketch plane a
    ///   `qSketchPoint` lives on, a mate connector's origin, a vertex. P1–P4
    ///   behaviour, so an existing document is unchanged.
    /// * `part` — the first surface of the model the axis enters. Lets a sketch
    ///   float above the part and still drill "6 mm into the material" rather
    ///   than "6 mm below a plane that is 20 mm up in the air".
    /// * `selected_plane` — [`start_plane`](HoleDef::start_plane).
    #[param(
        name = "Start",
        default = ::std::string::String::from("sketch_plane"),
        variants = "sketch_plane=Sketch plane,part=Part surface,selected_plane=Selected plane"
    )]
    pub start_mode: String,

    /// The plane the bore starts at — only read for
    /// `start_mode = selected_plane`. A planar B-Rep face, an origin datum
    /// plane or a mate connector; the bore axis is intersected with it.
    #[param(
        name = "Start plane",
        default = ::fielddraft_protocol::Query::new(""),
        entity_filter = "FACE",
        visible_when = "start_mode=selected_plane"
    )]
    pub start_plane: ::fielddraft_protocol::Query,

    /// End condition (Onshape's "Termination"):
    ///
    /// * `blind` — exactly [`depth`](HoleDef::depth) past the start.
    /// * `through_all` — spans the target body's whole extent along the axis.
    /// * `up_to_next` — stops at the next material boundary in the drilling
    ///   direction: the far wall of the first slab the axis passes through
    ///   (P5, found on the live field of everything modelled so far).
    /// * `up_to_entity` — stops at the plane named by
    ///   [`up_to_entity`](HoleDef::up_to_entity) (P5).
    ///
    /// The last two are shifted by [`end_offset`](HoleDef::end_offset). Only
    /// `blind` gets a drill point — the other three end at a surface the bore
    /// breaks through or is told to meet, and a cone there would poke past it.
    #[param(
        name = "End",
        default = ::std::string::String::from("blind"),
        variants = "blind=Blind,through_all=Through all,up_to_next=Up to next,up_to_entity=Up to entity"
    )]
    pub end_type: String,

    /// Bore depth measured from the START (see
    /// [`start_mode`](HoleDef::start_mode)) into the body — only used for
    /// `end_type = blind`.
    #[param(
        name = "Depth",
        default = ::fielddraft_protocol::Length::mm(10.0),
        visible_when = "end_type=blind"
    )]
    pub depth: ::fielddraft_protocol::Length,

    /// The face/plane the bore ends at — only read for
    /// `end_type = up_to_entity`. Same three kinds as
    /// [`start_plane`](HoleDef::start_plane): a **planar** B-Rep face, an origin
    /// datum plane, or a mate connector. A curved face is rejected with a named
    /// error instead of being reduced to its centroid.
    #[param(
        name = "Up to entity",
        default = ::fielddraft_protocol::Query::new(""),
        entity_filter = "FACE",
        visible_when = "end_type=up_to_entity"
    )]
    pub up_to_entity: ::fielddraft_protocol::Query,

    /// Onshape's "Offset from end", read for `up_to_next` / `up_to_entity`:
    /// how far past the found surface the bore ends. **Positive is deeper**
    /// (further along the drilling direction), negative stops short of it —
    /// so `−2 mm` on a bore up-to the far wall leaves a 2 mm floor.
    #[param(
        name = "Offset from end",
        default = ::fielddraft_protocol::Length::mm(0.0),
        visible_when = "end_type=up_to_next|up_to_entity"
    )]
    pub end_offset: ::fielddraft_protocol::Length,

    /// Counterbore diameter — the wide flat seat at the mouth that swallows a
    /// cap-screw head. Only read for `style = counterbore`. Clamped to at
    /// least the bore Ø: a counterbore narrower than its own shaft would
    /// carve nothing and just confuse the preview.
    #[param(
        name = "Counterbore Ø",
        default = ::fielddraft_protocol::Length::mm(11.0),
        visible_when = "style=counterbore"
    )]
    pub cbore_diameter: ::fielddraft_protocol::Length,

    /// Counterbore depth, measured from the anchor face down to the flat
    /// seat. Only read for `style = counterbore`.
    #[param(
        name = "Counterbore depth",
        default = ::fielddraft_protocol::Length::mm(6.0),
        visible_when = "style=counterbore"
    )]
    pub cbore_depth: ::fielddraft_protocol::Length,

    /// Countersink diameter **at the anchor face** — the wide end of the
    /// cone. Only read for `style = countersink`; must exceed the bore Ø.
    #[param(
        name = "Countersink Ø",
        default = ::fielddraft_protocol::Length::mm(12.0),
        visible_when = "style=countersink"
    )]
    pub csink_diameter: ::fielddraft_protocol::Length,

    /// Countersink **included** cone angle (the full apex angle, not the
    /// half-angle): 90° metric, 82° UNC, 100°/120° sheet metal. The cone
    /// depth follows from it and the two diameters — it is never entered
    /// directly, exactly like Onshape. Only read for `style = countersink`.
    #[param(
        name = "Countersink angle",
        default = ::fielddraft_protocol::Angle::deg(90.0),
        visible_when = "style=countersink"
    )]
    pub csink_angle: ::fielddraft_protocol::Angle,

    /// Flip the bore direction (Onshape's opposite-direction arrow). By
    /// default the hole drills *into* the body (opposite the anchor's outward
    /// normal / sketch-plane normal); set this to drill along it instead.
    #[param(name = "Opposite direction", default = false, ui_hint = "opposite_direction")]
    pub opposite_direction: bool,

    /// Drill-point angle at a BLIND bore's floor (P4). A real twist drill
    /// leaves a cone, not a flat bottom: `118°` is the general-purpose
    /// standard, `135°` the split-point / hard-material grind, `flat` an
    /// end-mill floor, `custom` reads [`tip_custom`]. Ignored for
    /// `through_all` — a hole that goes all the way through has no floor.
    ///
    /// Like Onshape, the value is the **included** angle: 118° means the cone
    /// walls are 59° off the bore axis, so its height is `R / tan(59°)`
    /// (≈ 0.3·Ø, the machinist's rule of thumb).
    ///
    /// [`tip_custom`]: HoleDef::tip_custom
    #[param(
        name = "Tip angle",
        default = ::std::string::String::from("a118"),
        variants = "a118=118°,a135=135°,flat=Flat,custom=Custom",
        visible_when = "end_type=blind"
    )]
    pub tip: String,

    /// Free drill-point included angle — used only when `tip = custom`.
    #[param(
        name = "Tip angle (custom)",
        default = ::fielddraft_protocol::Angle::deg(118.0),
        visible_when = "end_type=blind, tip=custom"
    )]
    pub tip_custom: ::fielddraft_protocol::Angle,

    /// What `depth` measures on a blind bore with a drill point (P4):
    /// `false` (default) = down to the **shoulder**, the last full-diameter
    /// point, with the cone sitting BELOW it — the drawing convention;
    /// `true` = down to the cone's **tip**, so the full-diameter part ends
    /// `R / tan(angle/2)` earlier. Irrelevant for `tip = flat`, where both
    /// readings coincide.
    #[param(
        name = "Depth to tip",
        default = false,
        visible_when = "end_type=blind"
    )]
    pub depth_to_tip: bool,

    /// Bore locations: comma-separated point/anchor queries — sketch points
    /// (`qSketchPoint(<sketch>, <entity>)`), mate connectors
    /// (`qCreatedBy(<mc>)`), B-Rep vertices (`qVertex(<hash>)`) or faces. Each
    /// resolves to an origin + axis and drills one identical cutter. Empty
    /// (dialog just opened) drills nothing.
    #[param(name = "Locations", default = ::fielddraft_protocol::MultiQuery::new(""), entity_filter = "VERTEX,SKETCHPOINT")]
    pub locations: ::fielddraft_protocol::MultiQuery,

    /// Merge scope: the body/bodies the hole drills, as `qCreatedBy(<seed>)`
    /// queries. MANDATORY — an empty scope warns and drills nothing (Onshape
    /// semantics: scope is always explicit).
    #[param(name = "Scope", default = ::fielddraft_protocol::MultiQuery::new(""), entity_filter = "BODY")]
    pub targets: ::fielddraft_protocol::MultiQuery,
}

/// The effective bore diameter (mm) for the chosen size **and type** (#404 P2).
///
/// `standard_size = custom` (or an unknown key from a stale document — never
/// silently drill a 0-Ø hole) always takes the free `diameter`. Otherwise the
/// hole type picks the ISO column:
///
/// | type | column | M6 |
/// |---|---|---|
/// | `drilled` | nominal Ø (ISO 261) | 6.0 |
/// | `clearance` | ISO 273 close/normal/loose per [`HoleDef::fit`] | 6.4 / 6.6 / 7.0 |
/// | `tapped` | coarse-pitch tap drill = nominal − pitch | 5.0 |
///
/// Tapped is **cosmetic** (Onshape semantics): the volume removed is the core
/// hole, the thread is annotation — no helix is ever swept.
fn effective_diameter_mm(d: &HoleDef) -> f32 {
    let Some(t) = super::hole_iso::metric_by_key(&d.standard_size) else {
        return d.diameter.to_mm();
    };
    match d.hole_type.as_str() {
        "clearance" => t.clearance_mm(&d.fit),
        "tapped" => t.tap_drill_mm,
        // `drilled` and anything a future variant adds: the nominal bore.
        _ => t.nominal_mm,
    }
}

/// A millimetre value with its trailing zeros trimmed: `6.00 → "6"`,
/// `7.70 → "7.7"`, `3.25 → "3.25"`. For NAMES only — never for geometry.
fn trim_mm(v: f32) -> String {
    let s = format!("{v:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() { "0".to_string() } else { s.to_string() }
}

/// The auto-name of ONE bore: what the settings add up to, in the words a
/// machinist reads off a drawing — `M6 tapped`, `Ø7.7 drilled counterbore`
/// (#404 P6).
///
/// Deliberately built from the SIZE, the TYPE and the STYLE and nothing else:
/// those three are what distinguishes two holes in a tree at a glance. The end
/// condition and the drill point are settings you look up, not identity.
fn auto_name_base(d: &HoleDef) -> String {
    // Mirrors `effective_diameter_mm`'s decision so the name can never claim a
    // norm the geometry did not use.
    let size = match super::hole_iso::metric_by_key(&d.standard_size) {
        Some(t) => t.key.to_string(),
        None => format!("Ø{}", trim_mm(d.diameter.to_mm())),
    };
    let mut name = format!("{size} {}", d.hole_type);
    match d.style.as_str() {
        "counterbore" => name.push_str(" counterbore"),
        "countersink" => name.push_str(" countersink"),
        _ => {}
    }
    name
}

/// The auto-generated DISPLAY NAME of a whole `hole` feature call (#404 P6):
/// [`auto_name_base`] plus `×N` when it drills more than one bore — the
/// outliner's `M6 tapped ×3` instead of `Hole 7`.
///
/// `None` when the params do not parse as a hole at all, so the caller keeps
/// its generic fallback rather than inventing a name for a broken definition.
/// A user-set `name` is honoured by the caller and never reaches here.
pub fn auto_name(params: &Value) -> Option<String> {
    let d = HoleDef::parse(params).ok()?;
    let base = auto_name_base(&d);
    let bores = d.locations.queries().iter().filter(|q| !q.trim().is_empty()).count();
    Some(if bores > 1 { format!("{base} ×{bores}") } else { base })
}

fn normalize3(v: [f32; 3]) -> [f32; 3] {
    let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if n > 1e-9 {
        [v[0] / n, v[1] / n, v[2] / n]
    } else {
        [0.0, 0.0, 1.0]
    }
}

/// Unit quaternion `(x, y, z, w)` rotating `from` onto `to` (both unit). The
/// cutter cylinder's axis is local +Y, so this orients it along the bore
/// direction. Shortest-arc construction with an antiparallel special case.
fn quat_from_to(from: [f32; 3], to: [f32; 3]) -> [f32; 4] {
    let dot = from[0] * to[0] + from[1] * to[1] + from[2] * to[2];
    if dot > 0.999_999 {
        return [0.0, 0.0, 0.0, 1.0];
    }
    if dot < -0.999_999 {
        // 180°: rotate about any axis perpendicular to `from`.
        let axis = if from[0].abs() < 0.9 {
            [
                0.0 - from[2] * from[0],
                0.0 - from[2] * from[1],
                1.0 - from[2] * from[2],
            ]
        } else {
            [
                1.0 - from[0] * from[0],
                0.0 - from[0] * from[1],
                0.0 - from[0] * from[2],
            ]
        };
        let a = normalize3(axis);
        return [a[0], a[1], a[2], 0.0];
    }
    let c = [
        from[1] * to[2] - from[2] * to[1],
        from[2] * to[0] - from[0] * to[2],
        from[0] * to[1] - from[1] * to[0],
    ];
    let w = 1.0 + dot;
    let inv = 1.0 / (c[0] * c[0] + c[1] * c[1] + c[2] * c[2] + w * w).sqrt();
    [c[0] * inv, c[1] * inv, c[2] * inv, w * inv]
}

/// Place an origin-centred, **local +Y**-axis primitive so that it spans
/// `[t_lo, t_hi]` measured along `dir` from `origin`. Returns the primitive's
/// `half_height` plus the transform that puts it there.
///
/// Every cutter part of a hole (shaft, counterbore, countersink cone, drill
/// tip) is authored through this one function, so `t` is a single shared
/// coordinate: `t = 0` is the anchor face, growing positive INTO the material.
fn axial_span(origin: [f32; 3], dir: [f32; 3], t_lo: f32, t_hi: f32) -> (f32, Transform) {
    let half_height = (t_hi - t_lo) / 2.0;
    let mid_t = (t_hi + t_lo) / 2.0;
    (
        half_height,
        Transform {
            translate: [
                origin[0] + dir[0] * mid_t,
                origin[1] + dir[1] * mid_t,
                origin[2] + dir[2] * mid_t,
            ],
            // The cutter primitives (Cylinder, Cone) run along local +Y.
            rotation: quat_from_to([0.0, 1.0, 0.0], dir),
            scale: [1.0, 1.0, 1.0],
        },
    )
}

/// Resolve a P5 start/end plane query to `(origin, unit normal)` in world
/// space, failing with a **named** parameter error so the dialog can point at
/// the field that is wrong rather than at the feature.
///
/// Accepts the same three plane kinds a sketch may stand on
/// ([`Context::resolve_planar_frame`]): a planar B-Rep face, an origin datum
/// plane, a mate connector. A CURVED face is refused here instead of being
/// reduced to its centroid — an end condition is a statement about a plane.
fn resolve_hole_plane(
    ctx: &mut Context<'_>,
    query: &str,
    name: &'static str,
) -> Result<([f32; 3], [f32; 3]), FeatureError> {
    let bad = |message: String| FeatureError::InvalidParameter {
        feature: HoleDef::TYPE_NAME,
        name,
        message,
    };
    let q = query.trim();
    if q.is_empty() {
        return Err(bad(
            "pick a planar face, an origin datum plane or a mate connector".to_string(),
        ));
    }
    let expr = crate::queries::parse_query(q)
        .map_err(|e| bad(format!("could not parse the query {q:?}: {e}")))?;
    let frame = ctx.resolve_planar_frame(&expr).ok_or_else(|| {
        bad(format!(
            "{q} does not resolve to a PLANAR face, an origin datum plane or a mate connector"
        ))
    })?;
    Ok((frame.origin, normalize3(frame.normal)))
}

/// Parameter `t` (mm along `dir` from `origin`) where the bore axis meets the
/// plane `(plane_origin, plane_normal)`, or `None` when the axis runs parallel
/// to it (no intersection, or all of them).
fn ray_plane_t(
    origin: [f32; 3],
    dir: [f32; 3],
    plane_origin: [f32; 3],
    plane_normal: [f32; 3],
) -> Option<f32> {
    let denom = dir[0] * plane_normal[0] + dir[1] * plane_normal[1] + dir[2] * plane_normal[2];
    if denom.abs() < 1e-6 {
        return None;
    }
    let num = (plane_origin[0] - origin[0]) * plane_normal[0]
        + (plane_origin[1] - origin[1]) * plane_normal[1]
        + (plane_origin[2] - origin[2]) * plane_normal[2];
    let t = num / denom;
    t.is_finite().then_some(t)
}

/// Step floor for the axis march. The step is normally the field's own
/// distance — which by the SDF property can never carry the walk THROUGH a
/// wall — but that distance goes to zero at a surface, so a floor is what makes
/// the walk terminate (and it bounds how thin a wall may be before it is
/// missed: 0.01 mm, well under any real one).
const MARCH_MIN_STEP: f32 = 0.01;
/// Bisection tolerance for a located boundary crossing (mm).
const MARCH_TOL: f32 = 1e-4;
/// Hard budget so a bore axis that GRAZES a surface (distance ~0 for a long
/// stretch, hence `MARCH_MIN_STEP` all the way) cannot hang regeneration.
const MARCH_MAX_STEPS: u32 = 40_000;

/// What [`first_boundary`] is looking for along the bore axis.
#[derive(Clone, Copy, PartialEq)]
enum Boundary {
    /// Material → air: the far wall of the slab the axis is inside — Onshape's
    /// "next" face.
    Exit,
    /// Air → material: the first surface of the part the axis runs into.
    Entry,
}

/// March the live field of everything modelled so far along the bore axis and
/// return the first `t > t_from` at which it crosses the requested boundary
/// (#404 P5).
///
/// Sphere-traced: each step is `|f|`, so no wall can be stepped over, floored
/// by [`MARCH_MIN_STEP`] so the walk still advances on a surface. The bracketing
/// sign flip is then bisected to [`MARCH_TOL`], which is what makes
/// `up_to_next` land ON the face rather than within a sampling grid of it.
fn first_boundary(
    field: &crate::csg::CsgNode,
    origin: [f32; 3],
    dir: [f32; 3],
    t_from: f32,
    t_max: f32,
    want: Boundary,
) -> Option<f32> {
    use crate::csg::EvalMode;
    let at = |t: f32| {
        field.sdf(
            [
                origin[0] + dir[0] * t,
                origin[1] + dir[1] * t,
                origin[2] + dir[2] * t,
            ],
            EvalMode::Visual,
        )
    };
    let mut t = t_from;
    let mut d = at(t);
    for _ in 0..MARCH_MAX_STEPS {
        if t >= t_max {
            return None;
        }
        let t_next = (t + d.abs().max(MARCH_MIN_STEP)).min(t_max);
        let d_next = at(t_next);
        let crossed = match want {
            Boundary::Exit => d < 0.0 && d_next >= 0.0,
            Boundary::Entry => d > 0.0 && d_next <= 0.0,
        };
        if crossed {
            let (mut lo, mut hi) = (t, t_next);
            let inside_lo = d < 0.0;
            while hi - lo > MARCH_TOL {
                let mid = 0.5 * (lo + hi);
                if (at(mid) < 0.0) == inside_lo {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            return Some(0.5 * (lo + hi));
        }
        t = t_next;
        d = d_next;
    }
    None
}

/// The counterbore / countersink geometry the chosen `style` stacks on top of
/// the plain shaft, already reduced to numbers (#404 P3).
#[derive(Clone, Copy)]
enum StyleStack {
    /// `style = simple` — nothing but the shaft.
    None,
    /// A wider flat-bottomed cylinder at the mouth.
    Counterbore { radius: f32, depth: f32 },
    /// A cone at the mouth: `radius` at the anchor face, tapering to the bore
    /// radius `depth` below it. `slope = tan(included/2)` is carried so the
    /// mouth overshoot widens the cone along its own wall instead of clipping
    /// it flat — that is what keeps the face circle at the entered diameter.
    Countersink { radius: f32, depth: f32, slope: f32 },
}

/// Reduce the style-specific schema fields to [`StyleStack`] numbers, or fail
/// the feature with a NAMED parameter error the dialog surfaces.
///
/// `bore_radius` is the shaft radius the stack has to meet: a counterbore is
/// clamped up to it (a narrower one would carve nothing), a countersink must
/// genuinely exceed it or there is no cone to build.
fn style_stack(d: &HoleDef, bore_radius: f32) -> Result<StyleStack, FeatureError> {
    match d.style.as_str() {
        "counterbore" => {
            let dia = d.cbore_diameter.to_mm();
            positive_f32(dia, HoleDef::TYPE_NAME, "cbore_diameter")?;
            let depth = d.cbore_depth.to_mm();
            positive_f32(depth, HoleDef::TYPE_NAME, "cbore_depth")?;
            Ok(StyleStack::Counterbore { radius: (dia / 2.0).max(bore_radius), depth })
        }
        "countersink" => {
            let dia = d.csink_diameter.to_mm();
            positive_f32(dia, HoleDef::TYPE_NAME, "csink_diameter")?;
            let radius = dia / 2.0;
            if radius <= bore_radius {
                return Err(FeatureError::InvalidParameter {
                    feature: HoleDef::TYPE_NAME,
                    name: "csink_diameter",
                    message: format!(
                        "must exceed the bore Ø ({:.3} mm), got {dia:.3} mm",
                        bore_radius * 2.0
                    ),
                });
            }
            // Included angle → half-angle off the axis. Outside (0°, 180°) the
            // cone degenerates into a needle or inverts into a disc.
            let half_angle = d.csink_angle.to_rad() / 2.0;
            if !(half_angle > 1e-3 && half_angle < std::f32::consts::FRAC_PI_2 - 1e-3) {
                return Err(FeatureError::InvalidParameter {
                    feature: HoleDef::TYPE_NAME,
                    name: "csink_angle",
                    message: format!(
                        "included angle must be strictly between 0° and 180°, got {:.1}°",
                        half_angle.to_degrees() * 2.0
                    ),
                });
            }
            // Cone depth from the two radii and the wall slope: the radius
            // shrinks by `tan(half_angle)` per mm of depth. A 90° sink on a
            // Ø6 bore with a Ø12 face circle is exactly 3 mm deep.
            let slope = half_angle.tan();
            Ok(StyleStack::Countersink { radius, depth: (radius - bore_radius) / slope, slope })
        }
        // `simple` and any variant a later phase adds: the bare shaft.
        _ => Ok(StyleStack::None),
    }
}

/// Height (mm) of the drill-point cone at a blind bore's floor, or `None` for
/// a flat bottom (#404 P4).
///
/// The schema carries the **included** angle; the cone's wall is half of that
/// off the axis, so a bore of radius `r` needs `r / tan(included/2)` of axial
/// room. 118° → 0.3005·Ø, 135° → 0.2071·Ø — the two numbers stamped on every
/// twist-drill chart.
fn tip_cone_height(d: &HoleDef, bore_radius: f32) -> Result<Option<f32>, FeatureError> {
    let included_deg = match d.tip.as_str() {
        "flat" => return Ok(None),
        "a118" => 118.0,
        "a135" => 135.0,
        "custom" => d.tip_custom.to_rad().to_degrees(),
        // An unknown key from a newer document: a flat floor is the reading
        // that cannot silently mis-cut.
        _ => return Ok(None),
    };
    let half = included_deg.to_radians() / 2.0;
    if !(half > 1e-3 && half < std::f32::consts::FRAC_PI_2 - 1e-3) {
        return Err(FeatureError::InvalidParameter {
            feature: HoleDef::TYPE_NAME,
            name: "tip_custom",
            message: format!(
                "included tip angle must be strictly between 0° and 180°, got {included_deg:.1}°"
            ),
        });
    }
    Ok(Some(bore_radius / half.tan()))
}

pub struct HoleFeature;

impl FeatureKind for HoleFeature {
    fn type_name(&self) -> &'static str {
        HoleDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        HoleDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = HoleDef::parse(def)?;

        let diameter_mm = effective_diameter_mm(&d);
        positive_f32(diameter_mm, HoleDef::TYPE_NAME, "diameter")?;
        let radius = diameter_mm / 2.0;

        // End condition. Anything a later phase adds to the variant list reads
        // as `blind` — the one interpretation that cannot silently cut further
        // than the number in the dialog.
        let end = d.end_type.as_str();
        let through_all = end == "through_all";
        let up_to_next = end == "up_to_next";
        let up_to_entity = end == "up_to_entity";
        let blind = !(through_all || up_to_next || up_to_entity);
        let depth_mm = d.depth.to_mm();
        if blind {
            positive_f32(depth_mm, HoleDef::TYPE_NAME, "depth")?;
        }
        // Signed on purpose: a negative offset stops SHORT of the found face.
        let end_offset_mm = d.end_offset.to_mm();
        let start_from_part = d.start_mode == "part";
        let start_from_plane = d.start_mode == "selected_plane";

        // Resolve every location query to an (origin, axis) frame. Unresolvable
        // or axis-less anchors are skipped (Onshape drills what it can); a
        // location list that is entirely empty (dialog just opened) simply
        // emits no geometry, exactly like the empty-Thicken/Revolve path.
        let mut frames: Vec<([f32; 3], [f32; 3])> = Vec::new();
        for q in d.locations.queries() {
            let q = q.trim();
            if q.is_empty() {
                continue;
            }
            let Ok(expr) = crate::queries::parse_query(q) else {
                continue;
            };
            // `t = 0.5`: for an edge anchor pick its midpoint (unused by
            // point/face/MC anchors, which ignore the parameter).
            if let Some(anc) = ctx.resolve_mate_anchor(&expr, 0.5) {
                if let Some(axis) = anc.primary.or(anc.auto_secondary) {
                    frames.push((anc.origin, normalize3(axis)));
                }
            }
        }

        // No locations picked yet — the dialog was just opened. Emit nothing
        // (and no scope warning) rather than flagging a half-built feature;
        // the bores appear once a point/anchor is picked. Same pattern as the
        // empty-Thicken / empty-Revolve path.
        if frames.is_empty() {
            return Ok(());
        }

        // Scope is mandatory: a hole drills the bodies named in `targets`.
        // Empty → warn and drill nothing (partition seeds it as a standalone
        // red preview — but we still emit nothing so the model is untouched).
        let target_ids = target_ids_from_queries(&d.targets);
        if let Some(w) = ctx.merge_scope_warning(BoolOp::Remove, &target_ids) {
            ctx.warn_feature(id, w);
        }

        // P5 start/end planes. Resolved ONCE — they are the same for every
        // location — and before the field probe below, which borrows
        // `ctx.features` immutably while these need `ctx` mutably.
        let start_plane = if start_from_plane {
            Some(resolve_hole_plane(ctx, d.start_plane.as_str(), "start_plane")?)
        } else {
            None
        };
        let end_plane = if up_to_entity {
            Some(resolve_hole_plane(ctx, d.up_to_entity.as_str(), "up_to_entity")?)
        } else {
            None
        };

        // Through-all needs the body extent along each bore axis; `up_to_next`
        // and `start_mode = part` need it as the march budget. Compute the
        // world-space vertex span of the woven bodies once; per location we
        // project onto that location's own axis.
        let woven_pts: Vec<[f32; 3]> = if through_all || up_to_next || start_from_part {
            let woven = ctx.brep_cache.ensure(&ctx.features);
            let mut pts = Vec::new();
            for (brep, placement) in woven {
                for v in &brep.vertices {
                    let p = crate::quat_rotate(
                        placement.rotation,
                        [
                            v.position[0] as f32 * placement.scale[0],
                            v.position[1] as f32 * placement.scale[1],
                            v.position[2] as f32 * placement.scale[2],
                        ],
                    );
                    pts.push([
                        p[0] + placement.translate[0],
                        p[1] + placement.translate[1],
                        p[2] + placement.translate[2],
                    ]);
                }
            }
            pts
        } else {
            Vec::new()
        };

        // The cutters inherit the feature's name, so a renamed hole renames its
        // geometry too; without a user name they get the auto-name WITHOUT the
        // `×N` suffix (each cutter is one bore, and the per-location `#i` below
        // already counts them).
        let name = optional_display_name(def, || auto_name_base(&d));
        // Small mouth overshoot so the cutter fully breaks the surface at the
        // anchor (avoids a coincident-face sliver at the bore opening). It sits
        // OUTSIDE the material, so the blind depth into the body is unaffected.
        const MOUTH_EPS: f32 = 0.1;

        // The live field of everything modelled SO FAR — the instrument
        // `up_to_next` and `start_mode = part` read the material boundaries off.
        // Built here, BEFORE the first cutter is pushed, so a hole can never
        // terminate on its own bore (or on its neighbour's).
        let field = (up_to_next || start_from_part)
            .then(|| crate::csg::CsgNode::from_decoded(&ctx.features));

        // P3 style stack + P4 drill point, resolved once — identical at every
        // location. Only a BLIND bore has a floor to cone: through-all breaks
        // out the far side, and up-to-next / up-to-entity end ON a surface the
        // user named, which a drill point would poke straight through.
        let stack = style_stack(&d, radius)?;
        let tip_height = if blind { tip_cone_height(&d, radius)? } else { None };

        let bad = |name: &'static str, message: String| FeatureError::InvalidParameter {
            feature: HoleDef::TYPE_NAME,
            name,
            message,
        };

        for (i, (origin, axis)) in frames.iter().enumerate() {
            // Drill INTO the body: opposite the outward anchor axis by default;
            // `opposite_direction` flips it.
            let dir = if d.opposite_direction {
                *axis
            } else {
                [-axis[0], -axis[1], -axis[2]]
            };

            // Everything modelled so far, projected onto THIS location's axis:
            // the through-all span, and the budget the field march may walk.
            let (mut span_lo, mut span_hi) = (-MOUTH_EPS, MOUTH_EPS);
            for p in &woven_pts {
                let t = (p[0] - origin[0]) * dir[0]
                    + (p[1] - origin[1]) * dir[1]
                    + (p[2] - origin[2]) * dir[2];
                span_lo = span_lo.min(t);
                span_hi = span_hi.max(t);
            }
            let t_far = span_hi + 1.0;

            // ── P5: where the bore BEGINS (`t = 0` for every measured depth) ──
            let t_start = match (start_plane, start_from_part) {
                (Some((p0, n)), _) => ray_plane_t(*origin, dir, p0, n).ok_or_else(|| {
                    bad(
                        "start_plane",
                        "the bore axis runs PARALLEL to the chosen start plane — \
                         it never meets it"
                            .to_string(),
                    )
                })?,
                (None, true) => {
                    let f = field.as_ref().expect("start_mode = part builds the field");
                    if f.sdf(*origin, crate::csg::EvalMode::Visual) <= 0.0 {
                        // The anchor already sits IN the material: the part
                        // surface it would start from is the anchor itself.
                        0.0
                    } else {
                        first_boundary(f, *origin, dir, 0.0, t_far, Boundary::Entry).ok_or_else(
                            || {
                                bad(
                                    "start_mode",
                                    "start-from-part found no material along the bore axis — \
                                     the axis misses every body"
                                        .to_string(),
                                )
                            },
                        )?
                    }
                }
                _ => 0.0,
            };

            // ── P5: where it ENDS ──
            let t_end = if through_all {
                t_far
            } else if up_to_next {
                let f = field.as_ref().expect("up_to_next builds the field");
                let hit = first_boundary(
                    f,
                    *origin,
                    dir,
                    t_start + MARCH_MIN_STEP,
                    t_far,
                    Boundary::Exit,
                )
                .ok_or_else(|| {
                    bad(
                        "end_type",
                        "up-to-next found no next face — the bore axis never leaves material \
                         again in the drilling direction"
                            .to_string(),
                    )
                })?;
                hit + end_offset_mm
            } else if let Some((p0, n)) = end_plane {
                ray_plane_t(*origin, dir, p0, n).ok_or_else(|| {
                    bad(
                        "up_to_entity",
                        "the bore axis runs PARALLEL to the chosen end plane — \
                         it never meets it"
                            .to_string(),
                    )
                })? + end_offset_mm
            } else {
                t_start + depth_mm
            };
            if !through_all && t_end <= t_start {
                return Err(bad(
                    if blind { "depth" } else { "end_offset" },
                    format!(
                        "the bore would end {:.3} mm BEFORE it starts — nothing to drill",
                        t_start - t_end
                    ),
                ));
            }

            // P4 depth semantics. `shaft_end` is where the full-diameter part
            // stops; `tip_span` is the cone below it, if any.
            //
            // - `depth_to_tip = false` (default): `depth` reaches the SHOULDER
            //   and the cone hangs below it — the bore ends up `tip_height`
            //   deeper than the number, exactly like a real drill.
            // - `depth_to_tip = true`: `depth` reaches the TIP, so the
            //   full-diameter part stops `tip_height` earlier.
            let (shaft_end, tip_span) = match tip_height {
                None => (t_end, None),
                Some(h) if d.depth_to_tip => {
                    let shoulder = t_end - h;
                    if shoulder <= t_start {
                        return Err(bad(
                            "depth",
                            format!(
                                "depth-to-tip {depth_mm:.3} mm is shorter than the {h:.3} mm \
                                 drill point — the bore never reaches full diameter"
                            ),
                        ));
                    }
                    (shoulder, Some((shoulder, t_end)))
                }
                Some(h) => (t_end, Some((t_end, t_end + h))),
            };

            // Bore extent along `dir`, measured from the anchor origin:
            // [t_lo, t_hi]. Through-all spans the whole body (both ways from the
            // anchor — a start plane only moves what the DEPTHS are measured
            // from, and through-all measures none); everything else runs from
            // the start plane to the resolved end.
            let (t_lo, t_hi) = if through_all {
                // Pad both ends so the cutter clears the body surface cleanly.
                (span_lo - 1.0, span_hi + 1.0)
            } else {
                (t_start - MOUTH_EPS, shaft_end)
            };

            // This location's cutter stack as
            // `(child-id local, name suffix, primitive, placement)`. The shaft
            // keeps the bare `<hole>/<i>` id it has had since P1.
            let mut parts: Vec<(String, &'static str, Primitive, Transform)> = Vec::new();

            let (half_height, xf) = axial_span(*origin, dir, t_lo, t_hi);
            parts.push((
                i.to_string(),
                "",
                Primitive::Cylinder { center: [0.0, 0.0, 0.0], radius, half_height },
                xf,
            ));

            match stack {
                StyleStack::None => {}
                StyleStack::Counterbore { radius: cbore_r, depth: cbore_d } => {
                    // A flat-bottomed wide cylinder from the mouth down. The
                    // seat therefore sits exactly `cbore_depth` below the START
                    // plane — the same origin `depth` is measured from.
                    let (hh, xf) =
                        axial_span(*origin, dir, t_start - MOUTH_EPS, t_start + cbore_d);
                    parts.push((
                        format!("{i}c"),
                        " counterbore",
                        Primitive::Cylinder {
                            center: [0.0, 0.0, 0.0],
                            radius: cbore_r,
                            half_height: hh,
                        },
                        xf,
                    ));
                }
                StyleStack::Countersink { radius: csink_r, depth: csink_d, slope } => {
                    // A frustum whose radius is EXACTLY `csink_r` at the START
                    // plane and the bore radius `csink_d` below it. The mouth
                    // overshoot extends it further out along its own wall
                    // (hence the `+ MOUTH_EPS * slope`), so the circle the user
                    // sees on the face keeps the entered diameter.
                    let (hh, xf) =
                        axial_span(*origin, dir, t_start - MOUTH_EPS, t_start + csink_d);
                    parts.push((
                        format!("{i}s"),
                        " countersink",
                        // `Primitive::Cone` runs along local +Y with `r_bottom`
                        // at −half_height — the mouth end, since +Y maps onto
                        // the drilling direction.
                        Primitive::Cone {
                            r_bottom: csink_r + MOUTH_EPS * slope,
                            r_top: radius,
                            half_height: hh,
                        },
                        xf,
                    ));
                }
            }

            if let Some((tip_lo, tip_hi)) = tip_span {
                // The drill point: a cone of the bore radius at the shoulder
                // narrowing to a true point at `tip_hi`. `Primitive::Cone`
                // puts `r_bottom` at local −Y, which is the shallow end.
                let (hh, xf) = axial_span(*origin, dir, tip_lo, tip_hi);
                parts.push((
                    format!("{i}t"),
                    " tip",
                    Primitive::Cone { r_bottom: radius, r_top: 0.0, half_height: hh },
                    xf,
                ));
            }

            // Hierarchical child ids (`<hole>/<i>`, `<hole>/<i>c`, …) so
            // `sync_feature_calls` drops the derived primitives on the next
            // sync — the `hole` FeatureCall stays the authoritative wire form.
            let bore_name = if i == 0 { name.clone() } else { format!("{name} #{}", i + 1) };
            for (local, name_suffix, primitive, transform) in parts {
                ctx.features.push(DecodedFeature::from_parts(
                    id.child(local),
                    format!("{bore_name}{name_suffix}"),
                    BoolOp::Remove,
                    primitive,
                    transform,
                    PrimitiveModifiers::NONE,
                    target_ids.clone(),
                ));
            }
        }
        Ok(())
    }
}
