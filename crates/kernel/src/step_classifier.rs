//! STEP export classifier — determines which features in the
//! document can be exported as exact B-Rep geometry via OpenCASCADE,
//! which need degradation, and which must be excluded entirely.
//!
//! M14b — this is the foundation layer for the STEP export pipeline.
//! The classifier walks the legacy `Feature` spine (which carries
//! typed modifier fields like `blend_radius`, `twist_rate`, etc.)
//! and produces a per-feature classification plus a human-readable
//! degradation report.
//!
//! Classification rules:
//!
//! | Category | Examples |
//! |----------|---------|
//! | **Hard** | Box, Sphere, Cylinder, Cone, Torus, Extrude, Revolve, Hole |
//! | **Degraded** | blend_radius>0 (→ sharp edge), chamfer (→ sharp edge) |
//! | **Excluded** | Gyroid, Thread, ImportedMesh, Twist, Bend, Displacement |
//! | **NonGeometric** | MateConnector, Sketch (consumed by extrude) |

use fielddraft_protocol::{Feature, FeatureId, Primitive};

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

pub fn classify(features: &[Feature]) -> StepReport {
    let features = features.iter().map(classify_one_feature).collect();
    StepReport { features }
}

pub fn classify_one_feature(f: &Feature) -> ClassifiedFeature {
    let prim_support = classify_primitive(&f.primitive);

    if matches!(prim_support, StepSupport::NonGeometric | StepSupport::Excluded(_)) {
        return ClassifiedFeature {
            id: f.id.clone(),
            name: f.name.clone(),
            support: prim_support,
        };
    }

    let mut degradations = Vec::new();

    if f.blend_radius > 0.0 {
        degradations.push("smooth fillet → sharp edge".into());
    }
    if f.chamfer_size > 0.0 {
        degradations.push("chamfer → sharp edge".into());
    }
    if f.shell_thickness > 0.0 {
        degradations.push("shell → excluded (no B-Rep equivalent)".into());
    }

    let mut excluded_reason = None;

    if f.twist_rate != 0.0 {
        excluded_reason = Some("twist deformation not representable in B-Rep");
    }
    if f.bend_rate != 0.0 {
        excluded_reason = Some("bend deformation not representable in B-Rep");
    }
    if f.displace_depth != 0.0 && f.displace_freq != 0.0 {
        excluded_reason = Some("procedural displacement not representable in B-Rep");
    }

    let support = if let Some(reason) = excluded_reason {
        StepSupport::Excluded(reason.into())
    } else if !degradations.is_empty() {
        StepSupport::Degraded(degradations)
    } else {
        match prim_support {
            StepSupport::Hard => StepSupport::Hard,
            StepSupport::Degraded(reasons) => StepSupport::Degraded(reasons),
            other => other,
        }
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
    use fielddraft_protocol::{BoolOp, FeatureId, MeshSdf, Transform};

    fn make_feature(name: &str, prim: Primitive) -> Feature {
        Feature {
            id: FeatureId::from_u32(1),
            name: name.into(),
            op: BoolOp::Union,
            primitive: prim,
            transform: Transform::IDENTITY,
            blend_radius: 0.0,
            chamfer_size: 0.0,
            twist_rate: 0.0,
            bend_rate: 0.0,
            displace_depth: 0.0,
            displace_freq: 0.0,
            shell_thickness: 0.0,
            target_ids: vec![],
        }
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
        let mut f = make_feature("Box", Primitive::Box {
            center: [0.0; 3],
            half_extents: [10.0; 3],
        });
        f.blend_radius = 2.0;
        let r = classify(&[f]);
        assert_eq!(r.degraded_count(), 1);
        assert_eq!(r.hard_count(), 0);
    }

    #[test]
    fn twist_excludes_hard_primitive() {
        let mut f = make_feature("Box", Primitive::Box {
            center: [0.0; 3],
            half_extents: [10.0; 3],
        });
        f.twist_rate = 0.1;
        let r = classify(&[f]);
        assert_eq!(r.excluded_count(), 1);
    }

    #[test]
    fn bend_excludes_hard_primitive() {
        let mut f = make_feature("Cyl", Primitive::Cylinder {
            center: [0.0; 3],
            radius: 5.0,
            half_height: 10.0,
        });
        f.bend_rate = 0.05;
        let r = classify(&[f]);
        assert_eq!(r.excluded_count(), 1);
    }

    #[test]
    fn displacement_excludes() {
        let mut f = make_feature("Box", Primitive::Box {
            center: [0.0; 3],
            half_extents: [10.0; 3],
        });
        f.displace_depth = 0.5;
        f.displace_freq = 2.0;
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
        let mut degraded = make_feature("Cyl", Primitive::Cylinder {
            center: [0.0; 3],
            radius: 5.0,
            half_height: 10.0,
        });
        degraded.blend_radius = 1.5;

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
