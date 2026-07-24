//! `hole` — Onshape-style bore feature (#404).
//!
//! P1 scope: `style = simple`, `type = drilled`, end conditions `blind`
//! (exact depth) and `through_all`. A hole targets **one or more locations**
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
    /// Hole style (geometry base form). P1 ships `simple`; `counterbore` /
    /// `countersink` are reserved so P3 adds the stacked cutter without a
    /// wire-format break.
    #[param(
        name = "Style",
        default = ::std::string::String::from("simple"),
        variants = "simple=Simple,counterbore=Counterbore,countersink=Countersink"
    )]
    pub style: String,

    /// Hole type (purpose). P1 ships `drilled` (nominal-diameter bore);
    /// `clearance` / `tapped` are reserved for P2 (ISO fit / tap-drill Ø).
    #[param(
        name = "Type",
        default = ::std::string::String::from("drilled"),
        variants = "drilled=Drilled,clearance=Clearance,tapped=Tapped"
    )]
    pub hole_type: String,

    /// Metric standard size (drives the bore diameter). `custom` switches to
    /// the free `diameter` field below. Variants mirror
    /// [`crate::features::hole_iso::ISO_METRIC`] (guarded by a schema/table
    /// drift test).
    #[param(
        name = "Size",
        default = ::std::string::String::from("M6"),
        variants = "custom=Custom (Ø),M2=M2,M2.5=M2.5,M3=M3,M4=M4,M5=M5,M6=M6,M8=M8,M10=M10,M12=M12"
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

/// The effective bore diameter (mm) for the chosen size/type. P1: drilled /
/// simple use the nominal diameter (or the free `diameter` for `custom`).
fn effective_diameter_mm(d: &HoleDef) -> f32 {
    if d.standard_size == "custom" {
        d.diameter.to_mm()
    } else if let Some(t) = super::hole_iso::metric_by_key(&d.standard_size) {
        t.nominal_mm
    } else {
        // Unknown size key → fall back to the free diameter so a stale
        // document never silently drills a 0-Ø hole.
        d.diameter.to_mm()
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

            let half_height = (t_hi - t_lo) / 2.0;
            let mid_t = (t_hi + t_lo) / 2.0;
            let center = [
                origin[0] + dir[0] * mid_t,
                origin[1] + dir[1] * mid_t,
                origin[2] + dir[2] * mid_t,
            ];
            // Cylinder axis is local +Y; orient it onto `dir`.
            let rotation = quat_from_to([0.0, 1.0, 0.0], dir);

            // Hierarchical child id (`<hole>/<i>`) so `sync_feature_calls`
            // drops the derived primitives on the next sync — the `hole`
            // FeatureCall stays the authoritative wire form.
            ctx.features.push(DecodedFeature::from_parts(
                id.child(i.to_string()),
                if i == 0 { name.clone() } else { format!("{name} #{}", i + 1) },
                BoolOp::Remove,
                Primitive::Cylinder {
                    center: [0.0, 0.0, 0.0],
                    radius,
                    half_height,
                },
                Transform {
                    translate: center,
                    rotation,
                    scale: [1.0, 1.0, 1.0],
                },
                PrimitiveModifiers::NONE,
                target_ids.clone(),
            ));
        }
        Ok(())
    }
}
