//! `hole` — Onshape-style bore feature (#404).
//!
//! Scope so far: `style = simple`, types `drilled` / `clearance` / `tapped`
//! (P2 — the ISO tables in [`hole_iso`](super::hole_iso) pick the bore Ø),
//! end conditions `blind` (exact depth) and `through_all`.
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
        variants = "close=Close,normal=Normal,loose=Loose"
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
    #[param(name = "Diameter", default = ::fielddraft_protocol::Length::mm(6.0))]
    pub diameter: ::fielddraft_protocol::Length,

    /// End condition. P1 ships `blind` (exact `depth`) and `through_all`
    /// (spans the target body's extent along the bore axis). `up_to_next` /
    /// `up_to_entity` are reserved for P5.
    #[param(
        name = "End",
        default = ::std::string::String::from("blind"),
        variants = "blind=Blind,through_all=Through all,up_to_next=Up to next,up_to_entity=Up to entity"
    )]
    pub end_type: String,

    /// Bore depth measured from the anchor into the body — only used for
    /// `end_type = blind`.
    #[param(name = "Depth", default = ::fielddraft_protocol::Length::mm(10.0))]
    pub depth: ::fielddraft_protocol::Length,

    /// Counterbore diameter — the wide flat seat at the mouth that swallows a
    /// cap-screw head. Only read for `style = counterbore`. Clamped to at
    /// least the bore Ø: a counterbore narrower than its own shaft would
    /// carve nothing and just confuse the preview.
    #[param(name = "Counterbore Ø", default = ::fielddraft_protocol::Length::mm(11.0))]
    pub cbore_diameter: ::fielddraft_protocol::Length,

    /// Counterbore depth, measured from the anchor face down to the flat
    /// seat. Only read for `style = counterbore`.
    #[param(name = "Counterbore depth", default = ::fielddraft_protocol::Length::mm(6.0))]
    pub cbore_depth: ::fielddraft_protocol::Length,

    /// Countersink diameter **at the anchor face** — the wide end of the
    /// cone. Only read for `style = countersink`; must exceed the bore Ø.
    #[param(name = "Countersink Ø", default = ::fielddraft_protocol::Length::mm(12.0))]
    pub csink_diameter: ::fielddraft_protocol::Length,

    /// Countersink **included** cone angle (the full apex angle, not the
    /// half-angle): 90° metric, 82° UNC, 100°/120° sheet metal. The cone
    /// depth follows from it and the two diameters — it is never entered
    /// directly, exactly like Onshape. Only read for `style = countersink`.
    #[param(name = "Countersink angle", default = ::fielddraft_protocol::Angle::deg(90.0))]
    pub csink_angle: ::fielddraft_protocol::Angle,

    /// Flip the bore direction (Onshape's opposite-direction arrow). By
    /// default the hole drills *into* the body (opposite the anchor's outward
    /// normal / sketch-plane normal); set this to drill along it instead.
    #[param(name = "Opposite direction", default = false, ui_hint = "opposite_direction")]
    pub opposite_direction: bool,

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

        let through_all = d.end_type == "through_all";
        let depth_mm = d.depth.to_mm();
        if !through_all {
            positive_f32(depth_mm, HoleDef::TYPE_NAME, "depth")?;
        }

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

        // Through-all needs the body extent along each bore axis. Compute the
        // world-space vertex span of the woven bodies once; per location we
        // project onto that location's own axis.
        let woven_pts: Vec<[f32; 3]> = if through_all {
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

        let name = optional_display_name(def, || format!("Hole {id}"));
        // Small mouth overshoot so the cutter fully breaks the surface at the
        // anchor (avoids a coincident-face sliver at the bore opening). It sits
        // OUTSIDE the material, so the blind depth into the body is unaffected.
        const MOUTH_EPS: f32 = 0.1;

        // P3 style stack, resolved once — identical at every location.
        let stack = style_stack(&d, radius)?;

        for (i, (origin, axis)) in frames.iter().enumerate() {
            // Drill INTO the body: opposite the outward anchor axis by default;
            // `opposite_direction` flips it.
            let dir = if d.opposite_direction {
                *axis
            } else {
                [-axis[0], -axis[1], -axis[2]]
            };

            // Bore extent along `dir`, measured from the anchor origin:
            // [t_lo, t_hi]. Blind → [-eps, depth]; through-all → span the body.
            let (t_lo, t_hi) = if through_all {
                let mut lo = -MOUTH_EPS;
                let mut hi = MOUTH_EPS;
                for p in &woven_pts {
                    let t = (p[0] - origin[0]) * dir[0]
                        + (p[1] - origin[1]) * dir[1]
                        + (p[2] - origin[2]) * dir[2];
                    lo = lo.min(t);
                    hi = hi.max(t);
                }
                // Pad both ends so the cutter clears the body surface cleanly.
                (lo - 1.0, hi + 1.0)
            } else {
                (-MOUTH_EPS, depth_mm)
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
                    // seat therefore sits exactly at `t = cbore_depth`.
                    let (hh, xf) = axial_span(*origin, dir, -MOUTH_EPS, cbore_d);
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
                    // A frustum whose radius is EXACTLY `csink_r` at the anchor
                    // face (`t = 0`) and the bore radius at `t = csink_d`. The
                    // mouth overshoot extends it further out along its own wall
                    // (hence the `+ MOUTH_EPS * slope`), so the circle the user
                    // sees on the face keeps the entered diameter.
                    let (hh, xf) = axial_span(*origin, dir, -MOUTH_EPS, csink_d);
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
