//! STEP export classifier — determines which features in the
//! document can be exported as exact B-Rep geometry via OpenCASCADE,
//! which need degradation, and which must be excluded entirely.
//!
//! M14b — this is the foundation layer for the STEP export pipeline.
//! The classifier walks each feature's decoded geometry node
//! ([`DecodedFeature`]): the leaf primitive plus any domain-warp/shell
//! modifier wrappers, combined with the junction blend, decide the
//! per-feature classification and a human-readable degradation report.
//!
//! Classification rules:
//!
//! | Category | Examples |
//! |----------|---------|
//! | **Hard** | Box, Sphere, Cylinder, Cone, Torus, Extrude, Revolve, Hole |
//! | **Degraded** | blend_radius>0 (→ sharp edge), chamfer (→ sharp edge) |
//! | **Excluded** | Gyroid, Thread, ImportedMesh, Twist, Bend, Displacement |
//! | **NonGeometric** | MateConnector, Sketch (consumed by extrude) |

use fielddraft_protocol::{FeatureId, Primitive};

use crate::csg::{BlendMode, CsgNode, DecodedFeature};

#[derive(Debug, Clone, PartialEq)]
pub enum StepSupport {
    Hard,
    Degraded(Vec<String>),
    Excluded(String),
    NonGeometric,
}

#[derive(Debug, Clone)]
pub struct ClassifiedFeature {
    pub id: FeatureId,
    pub name: String,
    pub support: StepSupport,
}

#[derive(Debug, Clone)]
pub struct StepReport {
    pub features: Vec<ClassifiedFeature>,
}

impl StepReport {
    pub fn hard_count(&self) -> usize {
        self.features.iter().filter(|f| matches!(f.support, StepSupport::Hard)).count()
    }

    pub fn degraded_count(&self) -> usize {
        self.features.iter().filter(|f| matches!(f.support, StepSupport::Degraded(_))).count()
    }

    pub fn excluded_count(&self) -> usize {
        self.features.iter().filter(|f| matches!(f.support, StepSupport::Excluded(_))).count()
    }

    pub fn geometric_count(&self) -> usize {
        self.features.iter().filter(|f| !matches!(f.support, StepSupport::NonGeometric)).count()
    }

    pub fn to_json(&self) -> serde_json::Value {
        use serde_json::json;
        json!({
            "hard": self.hard_count(),
            "degraded": self.degraded_count(),
            "excluded": self.excluded_count(),
            "total_geometric": self.geometric_count(),
            "features": self.features.iter()
                .filter(|f| !matches!(f.support, StepSupport::NonGeometric))
                .map(|f| {
                    json!({
                        "id": f.id.to_string(),
                        "name": f.name,
                        "status": match &f.support {
                            StepSupport::Hard => "hard".to_string(),
                            StepSupport::Degraded(reasons) => format!("degraded: {}", reasons.join("; ")),
                            StepSupport::Excluded(reason) => format!("excluded: {}", reason),
                            StepSupport::NonGeometric => "skip".to_string(),
                        },
                    })
                })
                .collect::<Vec<_>>(),
        })
    }
}

pub fn classify(features: &[DecodedFeature]) -> StepReport {
    let features = features.iter().map(classify_one_feature).collect();
    StepReport { features }
}

/// Modifier wrappers present on a feature's local geometry node.
struct NodeModifiers {
    twist: bool,
    bend: bool,
    displace: bool,
    shell: bool,
}

/// Peel the domain-warp/shell wrappers (and the extrude/revolve
/// self-centering `Transform`) off a local geometry node, returning the
/// innermost geometry node plus which modifiers were present.
fn peel_modifiers(node: &CsgNode) -> (&CsgNode, NodeModifiers) {
    let mut cur = node;
    let mut mods = NodeModifiers {
        twist: false,
        bend: false,
        displace: false,
        shell: false,
    };
    loop {
        match cur {
            CsgNode::Twist(inner, _) => {
                mods.twist = true;
                cur = inner;
            }
            CsgNode::Bend(inner, _) => {
                mods.bend = true;
                cur = inner;
            }
            CsgNode::Displace(inner, _, _) => {
                mods.displace = true;
                cur = inner;
            }
            CsgNode::Shell(inner, _, _) => {
                mods.shell = true;
                cur = inner;
            }
            CsgNode::Transform(_, inner) => cur = inner,
            _ => break,
        }
    }
    (cur, mods)
}

pub fn classify_one_feature(f: &DecodedFeature) -> ClassifiedFeature {
    let (geom, mods) = peel_modifiers(&f.node);
    let prim_support = match geom {
        CsgNode::Leaf { primitive, .. } => classify_primitive(primitive),
        CsgNode::Extrude { .. } | CsgNode::Revolve { .. } => StepSupport::Hard,
        // An arbitrary (e.g. future FieldSkript) node has no single
        // primitive to map onto an OCCT shape.
        _ => StepSupport::Excluded("composite feature node not representable in B-Rep".into()),
    };

    if matches!(prim_support, StepSupport::NonGeometric | StepSupport::Excluded(_)) {
        return ClassifiedFeature {
            id: f.id.clone(),
            name: f.name.clone(),
            support: prim_support,
        };
    }

    let mut degradations = Vec::new();

    if f.blend_radius > 0.0 {
        match f.blend_mode {
            BlendMode::Fillet => degradations.push("smooth fillet → sharp edge".into()),
            BlendMode::Chamfer => degradations.push("chamfer → sharp edge".into()),
        }
    }
    if mods.shell {
        degradations.push("shell → excluded (no B-Rep equivalent)".into());
    }

    let mut excluded_reason = None;

    if mods.twist {
        excluded_reason = Some("twist deformation not representable in B-Rep");
    }
    if mods.bend {
        excluded_reason = Some("bend deformation not representable in B-Rep");
    }
    if mods.displace {
        excluded_reason = Some("procedural displacement not representable in B-Rep");
    }

    let support = if let Some(reason) = excluded_reason {
        StepSupport::Excluded(reason.into())
    } else if !degradations.is_empty() {
        StepSupport::Degraded(degradations)
    } else {
        prim_support
    };

    ClassifiedFeature {
        id: f.id.clone(),
        name: f.name.clone(),
        support,
    }
}

fn classify_primitive(p: &Primitive) -> StepSupport {
    match p {
        // Exact OCCT equivalents
        Primitive::Box { .. } => StepSupport::Hard,
        Primitive::Sphere { .. } => StepSupport::Hard,
        Primitive::Cylinder { .. } => StepSupport::Hard,
        Primitive::Cone { .. } => StepSupport::Hard,
        Primitive::Torus { .. } => StepSupport::Hard,
        Primitive::Extrude { .. } => StepSupport::Hard,
        Primitive::Revolve { .. } => StepSupport::Hard,

        // Approximate OCCT mappings
        Primitive::Capsule { .. } => StepSupport::Degraded(
            vec!["capsule → sphere+cylinder union approximation".into()],
        ),
        Primitive::Ellipsoid { .. } => StepSupport::Degraded(
            vec!["ellipsoid → scaled sphere approximation".into()],
        ),
        Primitive::HexPrism { .. } => StepSupport::Hard,

        // Not representable in B-Rep
        Primitive::Thread { .. } => StepSupport::Excluded(
            "helix thread not representable as simple B-Rep".into(),
        ),
        Primitive::Gyroid { .. } => StepSupport::Excluded(
            "TPMS/gyroid not representable in B-Rep".into(),
        ),
        Primitive::ImportedMesh { .. } => StepSupport::Excluded(
            "imported mesh is already tessellated, not B-Rep".into(),
        ),
    }
}

// ─── tests ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::csg::PrimitiveModifiers;
    use fielddraft_protocol::{BoolOp, FeatureId, MeshSdf, Transform};

    /// Decode a single test feature into the open IR with explicit
    /// modifiers (blend/twist/shell/…), so a test can exercise a
    /// degraded or warped primitive without a fat builder struct.
    fn make_feature_mods(name: &str, prim: Primitive, mods: PrimitiveModifiers) -> DecodedFeature {
        DecodedFeature::from_parts(
            FeatureId::from_u32(1),
            name.into(),
            BoolOp::Union,
            prim,
            Transform::IDENTITY,
            mods,
            vec![],
        )
    }

    fn make_feature(name: &str, prim: Primitive) -> DecodedFeature {
        make_feature_mods(name, prim, PrimitiveModifiers::NONE)
    }

    #[test]
    fn box_is_hard() {
        let f = make_feature("Box", Primitive::Box {
            center: [0.0; 3],
            half_extents: [10.0; 3],
        });
        let r = classify(&[f]);
        assert_eq!(r.hard_count(), 1);
    }

    #[test]
    fn cylinder_is_hard() {
        let f = make_feature("Cyl", Primitive::Cylinder {
            center: [0.0; 3],
            radius: 5.0,
            half_height: 10.0,
        });
        let r = classify(&[f]);
        assert_eq!(r.hard_count(), 1);
    }

    #[test]
    fn gyroid_is_excluded() {
        let f = make_feature("Gyroid", Primitive::Gyroid {
            cell_size: 5.0,
            thickness: 1.0,
        });
        let r = classify(&[f]);
        assert_eq!(r.excluded_count(), 1);
        assert_eq!(r.hard_count(), 0);
    }

    #[test]
    fn blend_radius_degrades_hard_primitive() {
        let f = make_feature_mods(
            "Box",
            Primitive::Box {
                center: [0.0; 3],
                half_extents: [10.0; 3],
            },
            PrimitiveModifiers { blend_radius: 2.0, ..PrimitiveModifiers::NONE },
        );
        let r = classify(&[f]);
        assert_eq!(r.degraded_count(), 1);
        assert_eq!(r.hard_count(), 0);
    }

    #[test]
    fn twist_excludes_hard_primitive() {
        let f = make_feature_mods(
            "Box",
            Primitive::Box {
                center: [0.0; 3],
                half_extents: [10.0; 3],
            },
            PrimitiveModifiers { twist_rate: 0.1, ..PrimitiveModifiers::NONE },
        );
        let r = classify(&[f]);
        assert_eq!(r.excluded_count(), 1);
    }

    #[test]
    fn bend_excludes_hard_primitive() {
        let f = make_feature_mods(
            "Cyl",
            Primitive::Cylinder {
                center: [0.0; 3],
                radius: 5.0,
                half_height: 10.0,
            },
            PrimitiveModifiers { bend_rate: 0.05, ..PrimitiveModifiers::NONE },
        );
        let r = classify(&[f]);
        assert_eq!(r.excluded_count(), 1);
    }

    #[test]
    fn displacement_excludes() {
        let f = make_feature_mods(
            "Box",
            Primitive::Box {
                center: [0.0; 3],
                half_extents: [10.0; 3],
            },
            PrimitiveModifiers {
                displace_depth: 0.5,
                displace_freq: 2.0,
                ..PrimitiveModifiers::NONE
            },
        );
        let r = classify(&[f]);
        assert_eq!(r.excluded_count(), 1);
    }

    #[test]
    fn imported_mesh_is_excluded() {
        let f = make_feature("Mesh", Primitive::ImportedMesh {
            mesh: Box::new(MeshSdf::from_buffers(&[], &[])),
        });
        let r = classify(&[f]);
        assert_eq!(r.excluded_count(), 1);
    }

    #[test]
    fn mixed_scene_report() {
        let hard = make_feature("Box", Primitive::Box {
            center: [0.0; 3],
            half_extents: [10.0; 3],
        });
        let excluded = make_feature("Gyroid", Primitive::Gyroid {
            cell_size: 5.0,
            thickness: 1.0,
        });
        let degraded = make_feature_mods(
            "Cyl",
            Primitive::Cylinder {
                center: [0.0; 3],
                radius: 5.0,
                half_height: 10.0,
            },
            PrimitiveModifiers { blend_radius: 1.5, ..PrimitiveModifiers::NONE },
        );

        let r = classify(&[hard, excluded, degraded]);
        assert_eq!(r.hard_count(), 1);
        assert_eq!(r.excluded_count(), 1);
        assert_eq!(r.degraded_count(), 1);
        assert_eq!(r.geometric_count(), 3);
    }

    #[test]
    fn json_report_has_expected_fields() {
        let f = make_feature("Box", Primitive::Box {
            center: [0.0; 3],
            half_extents: [10.0; 3],
        });
        let r = classify(&[f]);
        let json = r.to_json();
        assert_eq!(json["hard"], 1);
        assert_eq!(json["excluded"], 0);
        assert_eq!(json["degraded"], 0);
        assert!(json["features"].is_array());
    }

    #[test]
    fn capsule_is_degraded() {
        let f = make_feature("Cap", Primitive::Capsule {
            radius: 3.0,
            half_height: 5.0,
        });
        let r = classify(&[f]);
        assert_eq!(r.degraded_count(), 1);
    }

    #[test]
    fn thread_is_excluded() {
        let f = make_feature("Thread", Primitive::Thread {
            nominal_radius: 3.0,
            pitch: 0.5,
            depth: 0.3,
            half_height: 5.0,
        });
        let r = classify(&[f]);
        assert_eq!(r.excluded_count(), 1);
    }
}
