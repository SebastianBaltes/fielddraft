//! Primitiv-Features (Box, Sphere, Cylinder, Cone, Thread, Torus,
//! Capsule, Ellipsoid, HexPrism) samt `push_primitive`-Helfer.

use serde_json::Value;

use fielddraft_protocol::{BoolOp, FeatureId, MultiQuery, Primitive, Transform};

use crate::csg::{DecodedFeature, PrimitiveModifiers};

use super::merge_scope::target_ids_from_queries;
use super::{Context, FeatureError, FeatureKind, FeatureSchema, optional_display_name, optional_rotation, optional_scale, positive_f32};
// ─── built-in K1 primitives ──────────────────────────────────────
//
// These are the reference `FeatureKind` impls for the three K1
// primitives. Their parameter parsing is now driven by the
// `FeatureSchema` derive from `fielddraft-features-macros`
// (K4.5.3), so each `execute` body shrinks to a two-step recipe:
// parse the definition JSON into the typed `{Box,Sphere,Cylinder}
// Def` struct, then lower it into a `DecodedFeature` (via
// `DecodedFeature::from_parts`) and push it onto the spine.
//
// The display name still has to be assembled at runtime because
// it depends on the `FeatureId` that's passed into `execute` —
// the derive's `default` attribute can only quote Rust rvalues,
// it can't capture closures over the id. That one extra line at
// the top of each execute function is the only hand-written
// parameter logic that remains in these three built-ins.

#[allow(clippy::too_many_arguments)] // geometry helper; param-struct refactor out of scope
fn push_primitive(
    ctx: &mut Context<'_>,
    id: &FeatureId,
    def: &Value,
    type_label: &str,
    op: BoolOp,
    targets: &MultiQuery,
    primitive: Primitive,
    translate: [f32; 3],
    mods: PrimitiveModifiers,
) {
    let name = optional_display_name(def, || format!("{type_label} {id}"));
    let rotation = optional_rotation(def);
    let scale = optional_scale(def);
    // A primitive set to Remove/Intersect (or Add) needs an explicit scope —
    // the body it acts on — just like the other booleans. The `targets` query
    // (`qCreatedBy(...)`) names the bodies it folds into; with none, warn and
    // let the partition pass seed a standalone (red preview) body. (New = a
    // fresh standalone solid, never warned.)
    let target_ids = target_ids_from_queries(targets);
    if let Some(w) = ctx.merge_scope_warning(op, &target_ids) {
        ctx.warn_feature(id, w);
    }
    ctx.features.push(DecodedFeature::from_parts(
        id.clone(),
        name,
        op,
        primitive,
        Transform { translate, rotation, scale },
        mods,
        target_ids,
    ));
}

/// `box` — K1 axis-aligned box primitive.
#[derive(FeatureSchema)]
#[schema(type = "box")]
pub struct BoxDef {
    #[param(unit = "mm")]
    pub half_extents: [f32; 3],
    #[param(default = [0.0, 0.0, 0.0])]
    pub center: [f32; 3],
    #[param(default = [0.0, 0.0, 0.0])]
    pub translate: [f32; 3],
    #[param(name = "Op", default = ::fielddraft_protocol::BoolOp::New)]
    pub op: BoolOp,
    /// Target bodies for the boolean op (Add/Remove/Intersect), as
    /// `qCreatedBy(<seed_feature>)` queries. Empty for New (a fresh standalone
    /// solid); required for the other ops (else warned + red preview).
    #[param(name = "Targets", default = ::fielddraft_protocol::MultiQuery::new(""), entity_filter = "BODY")]
    pub targets: ::fielddraft_protocol::MultiQuery,
    #[param(name = "Blend radius", default = 0.0)]
    pub blend_radius: f32,
    #[param(name = "Chamfer size", default = 0.0)]
    pub chamfer_size: f32,
    #[param(name = "Twist rate", default = 0.0)]
    pub twist_rate: f32,
    #[param(name = "Bend rate", default = 0.0)]
    pub bend_rate: f32,
    #[param(name = "Displace depth", default = 0.0)]
    pub displace_depth: f32,
    #[param(name = "Displace freq", default = 0.0)]
    pub displace_freq: f32,
    #[param(name = "Shell thickness", default = 0.0)]
    pub shell_thickness: f32,
}

pub struct BoxPrimitive;

impl FeatureKind for BoxPrimitive {
    fn type_name(&self) -> &'static str {
        BoxDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        BoxDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = BoxDef::parse(def)?;
        push_primitive(ctx, id, def, "Box", d.op, &d.targets,
            Primitive::Box { center: d.center, half_extents: d.half_extents },
            d.translate,
            PrimitiveModifiers {
                blend_radius: d.blend_radius, chamfer_size: d.chamfer_size,
                twist_rate: d.twist_rate, bend_rate: d.bend_rate,
                displace_depth: d.displace_depth, displace_freq: d.displace_freq,
                shell_thickness: d.shell_thickness,
            },
        );
        Ok(())
    }
}

/// `sphere` — K1 sphere primitive.
#[derive(FeatureSchema)]
#[schema(type = "sphere")]
pub struct SphereDef {
    #[param(unit = "mm")]
    pub radius: f32,
    #[param(default = [0.0, 0.0, 0.0])]
    pub center: [f32; 3],
    #[param(default = [0.0, 0.0, 0.0])]
    pub translate: [f32; 3],
    #[param(default = ::fielddraft_protocol::BoolOp::New)]
    pub op: BoolOp,
    /// Target bodies for the boolean op (Add/Remove/Intersect), as
    /// `qCreatedBy(<seed_feature>)` queries. Empty for New; required otherwise.
    #[param(name = "Targets", default = ::fielddraft_protocol::MultiQuery::new(""), entity_filter = "BODY")]
    pub targets: ::fielddraft_protocol::MultiQuery,
    #[param(name = "Blend radius", default = 0.0)]
    pub blend_radius: f32,
    #[param(name = "Chamfer size", default = 0.0)]
    pub chamfer_size: f32,
    #[param(name = "Twist rate", default = 0.0)]
    pub twist_rate: f32,
    #[param(name = "Bend rate", default = 0.0)]
    pub bend_rate: f32,
    #[param(name = "Displace depth", default = 0.0)]
    pub displace_depth: f32,
    #[param(name = "Displace freq", default = 0.0)]
    pub displace_freq: f32,
    #[param(name = "Shell thickness", default = 0.0)]
    pub shell_thickness: f32,
}

pub struct SpherePrimitive;

impl FeatureKind for SpherePrimitive {
    fn type_name(&self) -> &'static str {
        SphereDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        SphereDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = SphereDef::parse(def)?;
        positive_f32(d.radius, SphereDef::TYPE_NAME, "radius")?;
        push_primitive(ctx, id, def, "Sphere", d.op, &d.targets,
            Primitive::Sphere { center: d.center, radius: d.radius },
            d.translate,
            PrimitiveModifiers {
                blend_radius: d.blend_radius, chamfer_size: d.chamfer_size,
                twist_rate: d.twist_rate, bend_rate: d.bend_rate,
                displace_depth: d.displace_depth, displace_freq: d.displace_freq,
                shell_thickness: d.shell_thickness,
            },
        );
        Ok(())
    }
}

/// `cylinder` — K1 Y-axis-aligned cylinder primitive.
#[derive(FeatureSchema)]
#[schema(type = "cylinder")]
pub struct CylinderDef {
    #[param(unit = "mm")]
    pub radius: f32,
    #[param(unit = "mm")]
    pub half_height: f32,
    #[param(default = [0.0, 0.0, 0.0])]
    pub center: [f32; 3],
    #[param(default = [0.0, 0.0, 0.0])]
    pub translate: [f32; 3],
    #[param(default = ::fielddraft_protocol::BoolOp::New)]
    pub op: BoolOp,
    /// Target bodies for the boolean op (Add/Remove/Intersect), as
    /// `qCreatedBy(<seed_feature>)` queries. Empty for New; required otherwise.
    #[param(name = "Targets", default = ::fielddraft_protocol::MultiQuery::new(""), entity_filter = "BODY")]
    pub targets: ::fielddraft_protocol::MultiQuery,
    #[param(name = "Blend radius", default = 0.0)]
    pub blend_radius: f32,
    #[param(name = "Chamfer size", default = 0.0)]
    pub chamfer_size: f32,
    #[param(name = "Twist rate", default = 0.0)]
    pub twist_rate: f32,
    #[param(name = "Bend rate", default = 0.0)]
    pub bend_rate: f32,
    #[param(name = "Displace depth", default = 0.0)]
    pub displace_depth: f32,
    #[param(name = "Displace freq", default = 0.0)]
    pub displace_freq: f32,
    #[param(name = "Shell thickness", default = 0.0)]
    pub shell_thickness: f32,
}

pub struct CylinderPrimitive;

impl FeatureKind for CylinderPrimitive {
    fn type_name(&self) -> &'static str {
        CylinderDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        CylinderDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = CylinderDef::parse(def)?;
        positive_f32(d.radius, CylinderDef::TYPE_NAME, "radius")?;
        positive_f32(d.half_height, CylinderDef::TYPE_NAME, "half_height")?;
        push_primitive(ctx, id, def, "Cylinder", d.op, &d.targets,
            Primitive::Cylinder { center: d.center, radius: d.radius, half_height: d.half_height },
            d.translate,
            PrimitiveModifiers {
                blend_radius: d.blend_radius, chamfer_size: d.chamfer_size,
                twist_rate: d.twist_rate, bend_rate: d.bend_rate,
                displace_depth: d.displace_depth, displace_freq: d.displace_freq,
                shell_thickness: d.shell_thickness,
            },
        );
        Ok(())
    }
}

// ─── Cone (truncated cone / frustum) ─────────────────────────────

/// `cone` — truncated cone along the Y axis.
#[derive(FeatureSchema)]
#[schema(type = "cone")]
pub struct ConeDef {
    #[param(unit = "mm")]
    pub r_bottom: f32,
    #[param(unit = "mm")]
    pub r_top: f32,
    #[param(unit = "mm")]
    pub half_height: f32,
    #[param(default = [0.0, 0.0, 0.0])]
    pub translate: [f32; 3],
    #[param(default = ::fielddraft_protocol::BoolOp::New)]
    pub op: BoolOp,
    /// Target bodies for the boolean op (Add/Remove/Intersect), as
    /// `qCreatedBy(<seed_feature>)` queries. Empty for New; required otherwise.
    #[param(name = "Targets", default = ::fielddraft_protocol::MultiQuery::new(""), entity_filter = "BODY")]
    pub targets: ::fielddraft_protocol::MultiQuery,
    #[param(name = "Blend radius", default = 0.0)]
    pub blend_radius: f32,
    #[param(name = "Chamfer size", default = 0.0)]
    pub chamfer_size: f32,
    #[param(name = "Twist rate", default = 0.0)]
    pub twist_rate: f32,
    #[param(name = "Bend rate", default = 0.0)]
    pub bend_rate: f32,
    #[param(name = "Displace depth", default = 0.0)]
    pub displace_depth: f32,
    #[param(name = "Displace freq", default = 0.0)]
    pub displace_freq: f32,
    #[param(name = "Shell thickness", default = 0.0)]
    pub shell_thickness: f32,
}

pub struct ConePrimitive;

impl FeatureKind for ConePrimitive {
    fn type_name(&self) -> &'static str {
        ConeDef::TYPE_NAME
    }
    fn schema_json(&self) -> Value {
        ConeDef::schema_json()
    }
    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = ConeDef::parse(def)?;
        positive_f32(d.half_height, ConeDef::TYPE_NAME, "half_height")?;
        push_primitive(ctx, id, def, "Cone", d.op, &d.targets,
            Primitive::Cone { r_bottom: d.r_bottom, r_top: d.r_top, half_height: d.half_height },
            d.translate,
            PrimitiveModifiers {
                blend_radius: d.blend_radius, chamfer_size: d.chamfer_size,
                twist_rate: d.twist_rate, bend_rate: d.bend_rate,
                displace_depth: d.displace_depth, displace_freq: d.displace_freq,
                shell_thickness: d.shell_thickness,
            },
        );
        Ok(())
    }
}

// ─── K12: Thread ─────────────────────────────────────────────────

/// `thread` — metric-style screw thread via direct helix SDF.
#[derive(FeatureSchema)]
#[schema(type = "thread")]
pub struct ThreadDef {
    /// Nominal (outer) radius in mm.
    #[param(unit = "mm")]
    pub nominal_radius: f32,
    /// Pitch — distance per revolution in mm.
    #[param(unit = "mm")]
    pub pitch: f32,
    /// Thread depth in mm (groove amplitude).
    #[param(unit = "mm")]
    pub depth: f32,
    /// Half the total bolt length along Y.
    #[param(unit = "mm")]
    pub half_height: f32,
    #[param(default = [0.0, 0.0, 0.0])]
    pub translate: [f32; 3],
    #[param(default = ::fielddraft_protocol::BoolOp::New)]
    pub op: BoolOp,
    /// Target bodies for the boolean op (Add/Remove/Intersect), as
    /// `qCreatedBy(<seed_feature>)` queries. Empty for New; required otherwise.
    #[param(name = "Targets", default = ::fielddraft_protocol::MultiQuery::new(""), entity_filter = "BODY")]
    pub targets: ::fielddraft_protocol::MultiQuery,
    #[param(name = "Blend radius", default = 0.0)]
    pub blend_radius: f32,
    #[param(name = "Chamfer size", default = 0.0)]
    pub chamfer_size: f32,
}

pub struct ThreadFeature;

impl FeatureKind for ThreadFeature {
    fn type_name(&self) -> &'static str {
        ThreadDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        ThreadDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = ThreadDef::parse(def)?;
        positive_f32(d.nominal_radius, ThreadDef::TYPE_NAME, "nominal_radius")?;
        positive_f32(d.pitch, ThreadDef::TYPE_NAME, "pitch")?;
        positive_f32(d.depth, ThreadDef::TYPE_NAME, "depth")?;
        positive_f32(d.half_height, ThreadDef::TYPE_NAME, "half_height")?;
        push_primitive(ctx, id, def, "Thread", d.op, &d.targets,
            Primitive::Thread {
                nominal_radius: d.nominal_radius, pitch: d.pitch,
                depth: d.depth, half_height: d.half_height,
            },
            d.translate,
            PrimitiveModifiers {
                blend_radius: d.blend_radius, chamfer_size: d.chamfer_size,
                twist_rate: 0.0, bend_rate: 0.0,
                displace_depth: 0.0, displace_freq: 0.0,
                shell_thickness: 0.0,
            },
        );
        Ok(())
    }
}

// ─── Torus ───────────────────────────────────────────────────────

/// `torus` — Y-axis-aligned torus (donut).
#[derive(FeatureSchema)]
#[schema(type = "torus")]
pub struct TorusDef {
    /// Distance from the axis to the centre of the tube.
    #[param(unit = "mm")]
    pub major_radius: f32,
    /// Radius of the tube cross-section.
    #[param(unit = "mm")]
    pub minor_radius: f32,
    #[param(default = [0.0, 0.0, 0.0])]
    pub center: [f32; 3],
    #[param(default = [0.0, 0.0, 0.0])]
    pub translate: [f32; 3],
    #[param(default = ::fielddraft_protocol::BoolOp::New)]
    pub op: BoolOp,
    /// Target bodies for the boolean op (Add/Remove/Intersect), as
    /// `qCreatedBy(<seed_feature>)` queries. Empty for New; required otherwise.
    #[param(name = "Targets", default = ::fielddraft_protocol::MultiQuery::new(""), entity_filter = "BODY")]
    pub targets: ::fielddraft_protocol::MultiQuery,
    #[param(name = "Blend radius", default = 0.0)]
    pub blend_radius: f32,
    #[param(name = "Chamfer size", default = 0.0)]
    pub chamfer_size: f32,
    #[param(name = "Twist rate", default = 0.0)]
    pub twist_rate: f32,
    #[param(name = "Bend rate", default = 0.0)]
    pub bend_rate: f32,
    #[param(name = "Displace depth", default = 0.0)]
    pub displace_depth: f32,
    #[param(name = "Displace freq", default = 0.0)]
    pub displace_freq: f32,
    #[param(name = "Shell thickness", default = 0.0)]
    pub shell_thickness: f32,
}

pub struct TorusPrimitive;

impl FeatureKind for TorusPrimitive {
    fn type_name(&self) -> &'static str {
        TorusDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        TorusDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = TorusDef::parse(def)?;
        positive_f32(d.major_radius, TorusDef::TYPE_NAME, "major_radius")?;
        positive_f32(d.minor_radius, TorusDef::TYPE_NAME, "minor_radius")?;
        push_primitive(ctx, id, def, "Torus", d.op, &d.targets,
            Primitive::Torus { center: d.center, major_radius: d.major_radius, minor_radius: d.minor_radius },
            d.translate,
            PrimitiveModifiers {
                blend_radius: d.blend_radius, chamfer_size: d.chamfer_size,
                twist_rate: d.twist_rate, bend_rate: d.bend_rate,
                displace_depth: d.displace_depth, displace_freq: d.displace_freq,
                shell_thickness: d.shell_thickness,
            },
        );
        Ok(())
    }
}

// ─── Capsule ─────────────────────────────────────────────────────

/// `capsule` — Y-axis-aligned capsule (line-segment swept sphere).
/// `half_height` is the half-length of the inner segment;
/// total height = 2 * half_height + 2 * radius.
#[derive(FeatureSchema)]
#[schema(type = "capsule")]
pub struct CapsuleDef {
    #[param(unit = "mm")]
    pub radius: f32,
    #[param(unit = "mm")]
    pub half_height: f32,
    #[param(default = [0.0, 0.0, 0.0])]
    pub translate: [f32; 3],
    #[param(default = ::fielddraft_protocol::BoolOp::New)]
    pub op: BoolOp,
    /// Target bodies for the boolean op (Add/Remove/Intersect), as
    /// `qCreatedBy(<seed_feature>)` queries. Empty for New; required otherwise.
    #[param(name = "Targets", default = ::fielddraft_protocol::MultiQuery::new(""), entity_filter = "BODY")]
    pub targets: ::fielddraft_protocol::MultiQuery,
    #[param(name = "Blend radius", default = 0.0)]
    pub blend_radius: f32,
    #[param(name = "Chamfer size", default = 0.0)]
    pub chamfer_size: f32,
    #[param(name = "Twist rate", default = 0.0)]
    pub twist_rate: f32,
    #[param(name = "Bend rate", default = 0.0)]
    pub bend_rate: f32,
    #[param(name = "Displace depth", default = 0.0)]
    pub displace_depth: f32,
    #[param(name = "Displace freq", default = 0.0)]
    pub displace_freq: f32,
    #[param(name = "Shell thickness", default = 0.0)]
    pub shell_thickness: f32,
}

pub struct CapsulePrimitive;

impl FeatureKind for CapsulePrimitive {
    fn type_name(&self) -> &'static str {
        CapsuleDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        CapsuleDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = CapsuleDef::parse(def)?;
        positive_f32(d.radius, CapsuleDef::TYPE_NAME, "radius")?;
        positive_f32(d.half_height, CapsuleDef::TYPE_NAME, "half_height")?;
        push_primitive(ctx, id, def, "Capsule", d.op, &d.targets,
            Primitive::Capsule { radius: d.radius, half_height: d.half_height },
            d.translate,
            PrimitiveModifiers {
                blend_radius: d.blend_radius, chamfer_size: d.chamfer_size,
                twist_rate: d.twist_rate, bend_rate: d.bend_rate,
                displace_depth: d.displace_depth, displace_freq: d.displace_freq,
                shell_thickness: d.shell_thickness,
            },
        );
        Ok(())
    }
}

// ─── Ellipsoid ───────────────────────────────────────────────────

/// `ellipsoid` — axis-aligned ellipsoid with independent X/Y/Z radii.
/// SDF is approximate (coordinate scaling + sphere) but accurate enough
/// for DC meshing.
#[derive(FeatureSchema)]
#[schema(type = "ellipsoid")]
pub struct EllipsoidDef {
    #[param(default = [1.0, 1.0, 1.0], unit = "mm")]
    pub radii: [f32; 3],
    #[param(default = [0.0, 0.0, 0.0])]
    pub translate: [f32; 3],
    #[param(default = ::fielddraft_protocol::BoolOp::New)]
    pub op: BoolOp,
    /// Target bodies for the boolean op (Add/Remove/Intersect), as
    /// `qCreatedBy(<seed_feature>)` queries. Empty for New; required otherwise.
    #[param(name = "Targets", default = ::fielddraft_protocol::MultiQuery::new(""), entity_filter = "BODY")]
    pub targets: ::fielddraft_protocol::MultiQuery,
    #[param(name = "Blend radius", default = 0.0)]
    pub blend_radius: f32,
    #[param(name = "Chamfer size", default = 0.0)]
    pub chamfer_size: f32,
    #[param(name = "Twist rate", default = 0.0)]
    pub twist_rate: f32,
    #[param(name = "Bend rate", default = 0.0)]
    pub bend_rate: f32,
    #[param(name = "Displace depth", default = 0.0)]
    pub displace_depth: f32,
    #[param(name = "Displace freq", default = 0.0)]
    pub displace_freq: f32,
    #[param(name = "Shell thickness", default = 0.0)]
    pub shell_thickness: f32,
}

pub struct EllipsoidPrimitive;

impl FeatureKind for EllipsoidPrimitive {
    fn type_name(&self) -> &'static str {
        EllipsoidDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        EllipsoidDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = EllipsoidDef::parse(def)?;
        positive_f32(d.radii[0], EllipsoidDef::TYPE_NAME, "radii[0]")?;
        positive_f32(d.radii[1], EllipsoidDef::TYPE_NAME, "radii[1]")?;
        positive_f32(d.radii[2], EllipsoidDef::TYPE_NAME, "radii[2]")?;
        push_primitive(ctx, id, def, "Ellipsoid", d.op, &d.targets,
            Primitive::Ellipsoid { radii: d.radii },
            d.translate,
            PrimitiveModifiers {
                blend_radius: d.blend_radius, chamfer_size: d.chamfer_size,
                twist_rate: d.twist_rate, bend_rate: d.bend_rate,
                displace_depth: d.displace_depth, displace_freq: d.displace_freq,
                shell_thickness: d.shell_thickness,
            },
        );
        Ok(())
    }
}

// ─── HexPrism ────────────────────────────────────────────────────

/// `hex_prism` — Y-axis-aligned regular hexagonal prism.
/// `apothem` is the inradius (centre to flat face); circumradius ≈ apothem / 0.866.
#[derive(FeatureSchema)]
#[schema(type = "hex_prism")]
pub struct HexPrismDef {
    #[param(unit = "mm")]
    pub apothem: f32,
    #[param(unit = "mm")]
    pub half_height: f32,
    #[param(default = [0.0, 0.0, 0.0])]
    pub translate: [f32; 3],
    #[param(default = ::fielddraft_protocol::BoolOp::New)]
    pub op: BoolOp,
    /// Target bodies for the boolean op (Add/Remove/Intersect), as
    /// `qCreatedBy(<seed_feature>)` queries. Empty for New; required otherwise.
    #[param(name = "Targets", default = ::fielddraft_protocol::MultiQuery::new(""), entity_filter = "BODY")]
    pub targets: ::fielddraft_protocol::MultiQuery,
    #[param(name = "Blend radius", default = 0.0)]
    pub blend_radius: f32,
    #[param(name = "Chamfer size", default = 0.0)]
    pub chamfer_size: f32,
    #[param(name = "Twist rate", default = 0.0)]
    pub twist_rate: f32,
    #[param(name = "Bend rate", default = 0.0)]
    pub bend_rate: f32,
    #[param(name = "Displace depth", default = 0.0)]
    pub displace_depth: f32,
    #[param(name = "Displace freq", default = 0.0)]
    pub displace_freq: f32,
    #[param(name = "Shell thickness", default = 0.0)]
    pub shell_thickness: f32,
}

pub struct HexPrismPrimitive;

impl FeatureKind for HexPrismPrimitive {
    fn type_name(&self) -> &'static str {
        HexPrismDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        HexPrismDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = HexPrismDef::parse(def)?;
        positive_f32(d.apothem, HexPrismDef::TYPE_NAME, "apothem")?;
        positive_f32(d.half_height, HexPrismDef::TYPE_NAME, "half_height")?;
        push_primitive(ctx, id, def, "HexPrism", d.op, &d.targets,
            Primitive::HexPrism { apothem: d.apothem, half_height: d.half_height },
            d.translate,
            PrimitiveModifiers {
                blend_radius: d.blend_radius, chamfer_size: d.chamfer_size,
                twist_rate: d.twist_rate, bend_rate: d.bend_rate,
                displace_depth: d.displace_depth, displace_freq: d.displace_freq,
                shell_thickness: d.shell_thickness,
            },
        );
        Ok(())
    }
}
