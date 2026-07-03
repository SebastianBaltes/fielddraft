//! `helix` — Profil entlang einer Schraubenlinie (Onshape-Parametrik).

use serde_json::Value;

use fielddraft_protocol::{BoolOp, FeatureId, Primitive, Transform};

use crate::csg::{DecodedFeature, PrimitiveModifiers};

use super::merge_scope::{resolve_merge_scope, target_ids_from_queries};
use super::revolve::{axis_in_sketch_plane, revolve_axis_alignment, transform_sketch_2d};
use super::{Context, FeatureError, FeatureKind, FeatureSchema, optional_display_name, optional_rotation, optional_scale};
/// `helix` — Rust-native feature that sweeps a sketch profile along a
/// HELICAL path around the local Y axis: the real screw-thread
/// building block. The sketch is interpreted exactly like Revolve's —
/// `x` = radial distance from the axis, `y` = axial position — and one
/// profile instance is swept per revolution, advancing `pitch` per
/// turn for `turns` revolutions. The profile's axial extent should
/// stay ≤ `pitch` (adjacent turns must not overlap; every standard
/// thread form satisfies this). Union it with a core cylinder for an
/// external thread, or subtract it from a bore for an internal one.
#[derive(FeatureSchema)]
#[schema(type = "helix")]
pub struct HelixDef {
    /// Sketch profile this helix consumes. `qCreatedBy(sketch_id)`
    /// is the only query variant honoured today.
    pub profiles: ::fielddraft_protocol::Query,

    /// Which two parameters define the sweep (Onshape's "Input type"
    /// + height end condition): the third is derived at regen time —
    /// `pitch_height` derives turns, `turns_height` derives pitch.
    #[param(
        name = "Input",
        default = ::std::string::String::from("pitch_turns"),
        variants = "pitch_turns=Pitch + Turns,pitch_height=Pitch + Height,turns_height=Turns + Height"
    )]
    pub input: String,

    /// Axial advance per revolution (used unless input = Turns + Height).
    #[param(name = "Pitch", default = ::fielddraft_protocol::Length::mm(2.0))]
    pub pitch: ::fielddraft_protocol::Length,

    /// Number of revolutions, fractional turns allowed (used unless
    /// input = Pitch + Height).
    #[param(name = "Turns", default = 8.0)]
    pub turns: f32,

    /// Total axial height of the sweep (used by the *_height inputs).
    #[param(name = "Height", default = ::fielddraft_protocol::Length::mm(16.0))]
    pub height: ::fielddraft_protocol::Length,

    /// Angle (degrees) around the axis where the sweep starts,
    /// measured in the winding direction.
    #[param(name = "Start angle", default = 0.0)]
    pub start_angle: f32,

    /// Winding direction — `true` for a left-hand thread.
    #[param(name = "Left-handed", default = false)]
    pub left_handed: bool,

    /// Taper half-angle in DEGREES (NPT-style conical threads): the
    /// radius shrinks by `tan(angle)` per mm along +y. `0` = straight.
    #[param(name = "Taper angle", default = 0.0)]
    pub taper_angle: f32,

    /// Boolean op used to fold the helical body into the running
    /// feature spine. Defaults to [`BoolOp::Add`].
    #[param(default = ::fielddraft_protocol::BoolOp::Add)]
    pub operation: BoolOp,

    /// Merge scope: explicit target bodies for the boolean op, as
    /// `qCreatedBy(<seed_feature>)` queries. Empty + `merge_with_all` off =
    /// no merge (standalone body; warned for Add/Remove/Intersect).
    #[param(name = "Merge scope", default = ::fielddraft_protocol::MultiQuery::new(""), entity_filter = "BODY")]
    pub targets: ::fielddraft_protocol::MultiQuery,

    /// Merge with all (Onshape): fold the body into EVERY body it touches
    /// instead of an explicit scope. Off by default — touching solids never
    /// merge implicitly; opt in here or name targets above.
    #[param(name = "Merge with all", default = false)]
    pub merge_with_all: bool,

    /// Helix axis, as a query on a single straight line. Accepts a B-Rep
    /// edge (`qEdge(hash)`) or a sketch line (`qSketchLine(sketch, line)`)
    /// — both resolve to a world-space line the profile winds around.
    /// Empty (the default) falls back to the sketch's local Y axis at
    /// `x = 0`. Same mechanism as the Revolve axis (see [`RevolveDef`]).
    #[param(default = ::fielddraft_protocol::Query::new(""), entity_filter = "EDGE,SKETCHLINE")]
    pub axis: ::fielddraft_protocol::Query,
}

pub struct HelixFeature;

impl FeatureKind for HelixFeature {
    fn type_name(&self) -> &'static str {
        HelixDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        HelixDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = HelixDef::parse(def)?;

        // Resolve the `profiles` query against the sketch table —
        // identical to RevolveFeature (a helix without a real profile
        // produces no geometry at all).
        let mut sketch_frame = crate::sketch_features::SketchFrame::IDENTITY;
        let sketch = (|| -> Option<::fielddraft_protocol::Sketch2d> {
            let expr = crate::queries::parse_query(d.profiles.as_str()).ok()?;
            let crate::queries::QueryExpr::CreatedBy(sketch_id) = expr else {
                return None;
            };
            let sketch_data = ctx.sketches.get(&sketch_id)?;
            let sketch2d = crate::sketch_features::build_sketch2d_from_sub_calls(
                &sketch_data.sub_calls,
            );
            if matches!(sketch2d, ::fielddraft_protocol::Sketch2d::Empty) {
                None
            } else {
                sketch_frame = sketch_data.frame;
                Some(sketch2d)
            }
        })()
        .ok_or(FeatureError::MissingParameter {
            feature: HelixDef::TYPE_NAME,
            name: "profiles",
        })?;

        let name = optional_display_name(def, || format!("Helix {id}"));
        let rotation = optional_rotation(def);
        let scale = optional_scale(def);

        let base_transform = crate::compose_transforms(
            &sketch_frame.to_transform(),
            &Transform {
                translate: [0.0, 0.0, 0.0],
                rotation,
                scale,
            },
        );

        // Axis query → wind around an arbitrary in-plane line. Same
        // mechanism as RevolveFeature: align the picked line onto the
        // canonical local-Y axis with a rigid 2D map `M`, pre-multiply the
        // profile by `M`, and post-compose `M⁻¹` into the placement so the
        // helix winds around the chosen axis while the profile stays put.
        // `helix_profile_anchor` then reads the (now axis-relative) radius
        // from the transformed sketch. Empty axis → identity → legacy
        // local-Y behaviour.
        let axis_world = (|| -> Option<([f32; 3], [f32; 3])> {
            let expr = crate::queries::parse_query(d.axis.as_str()).ok()?;
            ctx.resolve_axis_world_segment(&expr)
        })();
        // Ticket dnfe: the helix axis must lie in the profile plane too.
        let axis_2d = match axis_world {
            Some((w0, w1)) if axis_in_sketch_plane(&sketch_frame, w0, w1) => Some((
                sketch_frame.world_to_local_2d(w0),
                sketch_frame.world_to_local_2d(w1),
            )),
            Some(_) => {
                ctx.warn_feature(
                    id,
                    "Helix-Achse liegt nicht in der Sketch-Ebene und wird ignoriert."
                        .to_string(),
                );
                None
            }
            None => None,
        };
        let (sketch, transform) = match axis_2d {
            None => (sketch, base_transform),
            Some((a, b)) => {
                let (m, m_inv) = revolve_axis_alignment(a, b);
                let aligned = transform_sketch_2d(sketch, &m);
                (aligned, crate::compose_transforms(&base_transform, &m_inv))
            }
        };

        // Resolve the (pitch, turns) pair from the chosen input mode —
        // the height variants derive the third quantity Onshape-style.
        let height = d.height.to_mm().max(0.001);
        let (pitch, turns) = match d.input.as_str() {
            "pitch_height" => {
                let p = d.pitch.to_mm().max(0.001);
                (p, (height / p).max(0.0))
            }
            "turns_height" => {
                let t = d.turns.max(0.001);
                ((height / t).max(0.001), t)
            }
            _ => (d.pitch.to_mm().max(0.001), d.turns.max(0.0)),
        };

        let target_ids = resolve_merge_scope(
            ctx,
            target_ids_from_queries(&d.targets),
            d.merge_with_all,
        );
        if let Some(w) = ctx.merge_scope_warning(d.operation, &target_ids) {
            ctx.warn_feature(id, w);
        }
        ctx.features.push(DecodedFeature::from_parts(
            id.clone(),
            name,
            d.operation,
            Primitive::Helix {
                sketch,
                // Axis expressed via profile + placement alignment (above),
                // so the primitive centre is always the local origin.
                center: [0.0, 0.0, 0.0],
                pitch,
                turns,
                left_handed: d.left_handed,
                taper: d.taper_angle.to_radians().tan(),
                start_angle: d.start_angle.to_radians(),
            },
            transform,
            PrimitiveModifiers::NONE,
            target_ids,
        ));
        Ok(())
    }
}
