//! `loft` — Interpolation zwischen N Profil-Sketches → Solid (#385).
//!
//! P1: das Feature parst N Profil-Referenzen, löst sie über
//! [`Context::resolve_loft_profiles`](crate::features::Context::resolve_loft_profiles)
//! zu einer sortierten [`LoftSection`]-Liste (+ gemeinsamer Achsen-Frame) auf
//! und pusht ein [`Primitive::Loft`]. Der echte z-beklemmte Querschnitt-Morph
//! (`CsgNode::Loft` + Opcodes) folgt in P2 — bis dahin senkt
//! `DecodedFeature::from_parts` den Loft auf einen Platzhalter-Extrude des
//! ersten Profils über die Gesamthöhe ab (AABB-korrekt, wasserdicht).

use serde_json::Value;

use fielddraft_protocol::{BoolOp, FeatureId, Primitive, Transform};

use crate::csg::{DecodedFeature, PrimitiveModifiers};

use super::merge_scope::{resolve_merge_scope, target_ids_from_queries};
use super::{
    Context, FeatureError, FeatureKind, FeatureSchema, optional_display_name, optional_rotation,
    optional_scale,
};

/// `loft` — Rust-native Feature, das zwischen N ≥ 2 parallelen Profil-
/// Querschnitten interpoliert. Die `profiles`-`MultiQuery` akkumuliert
/// Sketch-Regionen (`qRegion`/`qCreatedBy`) und/oder planare B-Rep-Faces
/// (`qFace`) in einer Pick-Session — dasselbe „faces and sketch regions"-
/// Eingabemuster wie Extrude (Ticket k280).
#[derive(FeatureSchema)]
#[schema(type = "loft")]
pub struct LoftDef {
    /// Profil-Querschnitte des Lofts, in Stapel-Reihenfolge. Jeder Eintrag
    /// löst wie ein Extrude-Profil auf: eine `qFace(...)`-Auswahl über den
    /// Face-Extrude-Pfad (eine planare Fläche), alles andere über die
    /// Sketch-Regionen-Auflösung. Es braucht mindestens zwei Profile auf
    /// zueinander **parallelen** Ebenen; laterale Versätze sind erlaubt.
    #[param(entity_filter = "SKETCHREGION, FACE")]
    pub profiles: ::fielddraft_protocol::MultiQuery,

    /// Boolean-Op, mit der der Loft-Body in die Feature-Spine gefaltet wird.
    /// Default [`BoolOp::New`] (frischer Body), wie bei Extrude.
    #[param(default = ::fielddraft_protocol::BoolOp::New)]
    pub operation: BoolOp,

    /// Merge-Scope: explizite Ziel-Bodies als `qCreatedBy(<seed>)`-Queries.
    /// Leer + „Merge with all" aus = kein Merge (eigenständiger Body; für
    /// Add/Remove/Intersect gewarnt) — identisches Verhalten wie Extrude.
    #[param(name = "Merge scope", default = ::fielddraft_protocol::MultiQuery::new(""), entity_filter = "BODY")]
    pub targets: ::fielddraft_protocol::MultiQuery,

    /// Merge with all (Onshape): den Body in JEDEN berührten Body falten
    /// statt in einen expliziten Scope. Default aus — berührende Solids
    /// verschmelzen nie implizit.
    #[param(name = "Merge with all", default = false)]
    pub merge_with_all: bool,
}

pub struct LoftFeature;

impl FeatureKind for LoftFeature {
    fn type_name(&self) -> &'static str {
        LoftDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        LoftDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = LoftDef::parse(def)?;

        let profile_qs: Vec<String> =
            d.profiles.queries().iter().map(|s| s.to_string()).collect();

        // Resolve the N profiles into sorted sections + the shared loft-axis
        // frame. All error cases (< 2 profiles, empty region, tilted plane,
        // duplicate offset) surface as a `profiles` InvalidParameter.
        let (sections, frame) =
            ctx.resolve_loft_profiles(&profile_qs).map_err(|message| {
                FeatureError::InvalidParameter {
                    feature: LoftDef::TYPE_NAME,
                    name: "profiles",
                    message,
                }
            })?;

        let name = optional_display_name(def, || format!("Loft {id}"));
        let rotation = optional_rotation(def);
        let scale = optional_scale(def);

        // P1: no dedicated wall-sub table yet (positional subs); the real
        // side-wall provenance rides along in P2 with `CsgNode::Loft`.
        let primitive = Primitive::Loft {
            profiles: sections,
            center: [0.0, 0.0, 0.0],
            wall_subs: None,
        };

        // Compose the shared sketch-plane frame (parent placement) with the
        // feature's own rotation/scale (child), exactly like Extrude — the
        // sections were resolved in that frame's local XY.
        let transform = crate::compose_transforms(
            &frame.to_transform(),
            &Transform {
                translate: [0.0, 0.0, 0.0],
                rotation,
                scale,
            },
        );

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
            primitive,
            transform,
            PrimitiveModifiers::NONE,
            target_ids,
        ));
        Ok(())
    }
}
