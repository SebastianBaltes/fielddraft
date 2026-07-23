//! M14d: STEP command builder — converts the Feature spine into a
//! linearized StepCommand[] pipeline that the OCCT Web Worker can
//! execute to build B-Rep geometry.
//!
//! The builder walks features top-to-bottom, skipping excluded and
//! non-geometric entries (using the M14b classifier). For each
//! exportable feature it emits:
//!   1. A primitive command (box, sphere, cylinder, etc.)
//!   2. An optional transform command (if placement ≠ identity)
//!   3. A boolean command folding it into the running accumulator
//!
//! The first exportable feature seeds the accumulator directly (no
//! boolean needed). Subsequent features use union/difference/intersect
//! to fold in.

use fielddraft_protocol::{BoolOp, Primitive, Sketch2d, Sketch2dPrimitive, Transform};
use serde_json::{json, Value};

use crate::csg::DecodedFeature;
use crate::step_classifier::{classify_one_feature, StepSupport};

/// Build the StepCommand JSON array from the feature spine.
/// Returns a `serde_json::Value` that is a JSON array of command objects
/// matching the TypeScript `StepCommand` union in `occt.worker.ts`.
pub fn build_step_commands(features: &[DecodedFeature]) -> Value {
    let mut commands: Vec<Value> = Vec::new();
    let mut accumulator: Option<String> = None;
    let mut idx = 0u32;

    for feat in features {
        let cf = classify_one_feature(feat);
        match &cf.support {
            StepSupport::Hard | StepSupport::Degraded(_) => {}
            _ => continue,
        }

        let Some(prim) = feat.export_primitive() else {
            continue;
        };
        let prim_handle = format!("prim_{idx}");
        if let Some(cmd) = primitive_command(&prim_handle, &prim) {
            commands.push(cmd);
        } else {
            continue;
        }

        let shape_handle = if !feat.transform.is_identity() {
            let xform_handle = format!("xform_{idx}");
            commands.push(transform_command(&xform_handle, &prim_handle, &feat.transform));
            xform_handle
        } else {
            prim_handle
        };

        match &accumulator {
            None => {
                accumulator = Some(shape_handle);
            }
            Some(acc) => {
                let bool_handle = format!("bool_{idx}");
                commands.push(bool_command(&bool_handle, &feat.op, acc, &shape_handle));
                accumulator = Some(bool_handle);
            }
        }

        idx += 1;
    }

    Value::Array(commands)
}

fn primitive_command(handle: &str, prim: &Primitive) -> Option<Value> {
    match prim {
        Primitive::Box { center, half_extents } => Some(json!({
            "op": "box",
            "handle": handle,
            "cx": center[0],
            "cy": center[1],
            "cz": center[2],
            "hx": half_extents[0],
            "hy": half_extents[1],
            "hz": half_extents[2]
        })),
        Primitive::Sphere { center, radius } => Some(json!({
            "op": "sphere",
            "handle": handle,
            "cx": center[0],
            "cy": center[1],
            "cz": center[2],
            "radius": radius
        })),
        Primitive::Cylinder { center, radius, half_height } => Some(json!({
            "op": "cylinder",
            "handle": handle,
            "cx": center[0],
            "cy": center[1],
            "cz": center[2],
            "radius": radius,
            "halfHeight": half_height
        })),
        Primitive::Cone { r_bottom, r_top, half_height } => Some(json!({
            "op": "cone",
            "handle": handle,
            "cx": 0.0,
            "cy": 0.0,
            "cz": 0.0,
            "rBottom": r_bottom,
            "rTop": r_top,
            "halfHeight": half_height
        })),
        Primitive::Torus { center, major_radius, minor_radius } => Some(json!({
            "op": "torus",
            "handle": handle,
            "cx": center[0],
            "cy": center[1],
            "cz": center[2],
            "majorRadius": major_radius,
            "minorRadius": minor_radius
        })),
        Primitive::HexPrism { apothem, half_height } => Some(json!({
            "op": "hexprism",
            "handle": handle,
            "apothem": apothem,
            "halfHeight": half_height
        })),
        Primitive::Extrude { sketch, half_depth_z, center, wall_subs: _ } => {
            let wire_pts = sketch_to_wire_pts(sketch);
            if wire_pts.is_empty() {
                return None;
            }
            Some(json!({
                "op": "extrude",
                "handle": handle,
                "wirePts": wire_pts,
                "depth": half_depth_z * 2.0,
                "cx": center[0],
                "cy": center[1],
                "cz": center[2]
            }))
        }
        Primitive::Revolve { sketch, center, wall_subs: _ } => {
            let wire_pts = sketch_to_wire_pts(sketch);
            if wire_pts.is_empty() {
                return None;
            }
            Some(json!({
                "op": "revolve",
                "handle": handle,
                "wirePts": wire_pts,
                "angle": std::f64::consts::TAU,
                "cx": center[0],
                "cy": center[1],
                "cz": center[2]
            }))
        }
        // These are classified as Excluded/NonGeometric by the classifier
        // but we still match them for completeness.
        Primitive::Capsule { .. }
        | Primitive::Ellipsoid { .. }
        | Primitive::Thread { .. }
        | Primitive::Helix { .. }
        | Primitive::Gyroid { .. }
        | Primitive::ImportedMesh { .. }
        | Primitive::CustomSdf { .. }
        // #385 P2: no dedicated OCCT loft command yet — the morph surface is
        // non-analytic, so STEP export uses the meshed body (analytic loft
        // export is P3).
        | Primitive::Loft { .. } => None,
    }
}

fn transform_command(handle: &str, source: &str, t: &Transform) -> Value {
    json!({
        "op": "transform",
        "handle": handle,
        "source": source,
        "tx": t.translate[0],
        "ty": t.translate[1],
        "tz": t.translate[2],
        "qx": t.rotation[0],
        "qy": t.rotation[1],
        "qz": t.rotation[2],
        "qw": t.rotation[3],
        "scale": t.scale
    })
}

fn bool_command(handle: &str, op: &BoolOp, a: &str, b: &str) -> Value {
    // OCCT-side commands stay geometric: Add and New are both fuses.
    let op_name = match op {
        BoolOp::Add | BoolOp::New => "union",
        BoolOp::Remove => "difference",
        BoolOp::Intersect => "intersect",
    };
    json!({
        "op": op_name,
        "handle": handle,
        "a": a,
        "b": b
    })
}

/// Extract a flat wire-point list from a Sketch2d tree.
///
/// The OCCT worker expects `wirePts: number[]` — a flat array of
/// [x0, y0, x1, y1, ...] 2D coordinates forming a closed polygon.
///
/// For simple cases (single polygon primitive), we return the vertices
/// directly. For circles/rectangles, we tessellate into polygon
/// approximations. For boolean sketch trees, we only extract the
/// outermost primitive's wire — CSG sketch booleans can't be
/// represented as a single OCCT wire, so we degrade to the bounding
/// shape.
fn sketch_to_wire_pts(sketch: &Sketch2d) -> Vec<f64> {
    match sketch {
        Sketch2d::Prim(prim) => prim_to_wire(prim),
        Sketch2d::Union(a, _)
        | Sketch2d::Difference(a, _)
        | Sketch2d::Intersection(a, _) => sketch_to_wire_pts(a),
        Sketch2d::Offset { inner, .. } => sketch_to_wire_pts(inner),
        Sketch2d::Empty => Vec::new(),
    }
}

fn prim_to_wire(prim: &Sketch2dPrimitive) -> Vec<f64> {
    match prim {
        Sketch2dPrimitive::Polygon { vertices } => {
            let mut pts = Vec::with_capacity(vertices.len() * 2);
            for v in vertices {
                pts.push(v[0] as f64);
                pts.push(v[1] as f64);
            }
            pts
        }
        Sketch2dPrimitive::Rectangle { center, half_extents } => {
            let cx = center[0] as f64;
            let cy = center[1] as f64;
            let hx = half_extents[0] as f64;
            let hy = half_extents[1] as f64;
            vec![
                cx - hx, cy - hy,
                cx + hx, cy - hy,
                cx + hx, cy + hy,
                cx - hx, cy + hy,
            ]
        }
        Sketch2dPrimitive::Circle { center, radius } => {
            let cx = center[0] as f64;
            let cy = center[1] as f64;
            let r = *radius as f64;
            let n = 64;
            let mut pts = Vec::with_capacity(n * 2);
            for i in 0..n {
                let a = std::f64::consts::TAU * (i as f64) / (n as f64);
                pts.push(cx + r * a.cos());
                pts.push(cy + r * a.sin());
            }
            pts
        }
        Sketch2dPrimitive::Sector { center, radius, start_angle, sweep_angle } => {
            let cx = center[0] as f64;
            let cy = center[1] as f64;
            let r = *radius as f64;
            let start = *start_angle as f64;
            let sweep = *sweep_angle as f64;
            let n = 32;
            let mut pts = Vec::with_capacity((n + 2) * 2);
            // Center point
            pts.push(cx);
            pts.push(cy);
            // Arc points
            for i in 0..=n {
                let a = start + sweep * (i as f64) / (n as f64);
                pts.push(cx + r * a.cos());
                pts.push(cy + r * a.sin());
            }
            pts
        }
        Sketch2dPrimitive::Path { segments, .. } => {
            // Flatten the analytic segment chain into an outline
            // polyline. Lines contribute their endpoints; curves are
            // sampled at PATH_FLATTEN_SAMPLES steps. The hull/outline
            // need not be exact, so flattening is acceptable here.
            use fielddraft_protocol::sketch2d::{SketchSegment, PATH_FLATTEN_SAMPLES};
            let mut pts = Vec::new();
            for seg in segments {
                let steps = match seg {
                    SketchSegment::Line { .. } => 1,
                    _ => PATH_FLATTEN_SAMPLES,
                };
                // Emit the segment start, then each subsequent sample
                // up to (but not including) the next segment's start
                // to avoid duplicate shared vertices.
                let start_pt = seg.point_at(0.0);
                pts.push(start_pt[0] as f64);
                pts.push(start_pt[1] as f64);
                for k in 1..steps {
                    let t = k as f32 / steps as f32;
                    let p = seg.point_at(t);
                    pts.push(p[0] as f64);
                    pts.push(p[1] as f64);
                }
            }
            pts
        }
    }
}

// ─── tests ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::csg::PrimitiveModifiers;
    use fielddraft_protocol::{BoolOp, FeatureId};

    /// Build a single test feature, decoding into the open IR with
    /// explicit modifiers so a test can exercise a blended/warped
    /// primitive without a fat builder struct.
    fn make_feature_mods(name: &str, prim: Primitive, mods: PrimitiveModifiers) -> DecodedFeature {
        DecodedFeature::from_parts(
            FeatureId::from_u32(1),
            name.into(),
            BoolOp::New,
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
    fn single_box_produces_one_command() {
        let f = make_feature("Box", Primitive::Box {
            center: [1.0, 2.0, 3.0],
            half_extents: [10.0, 20.0, 30.0],
        });
        let cmds = build_step_commands(&[f]);
        let arr = cmds.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["op"], "box");
        assert_eq!(arr[0]["cx"], 1.0);
        assert_eq!(arr[0]["hx"], 10.0);
        assert_eq!(arr[0]["handle"], "prim_0");
    }

    #[test]
    fn box_with_transform_produces_two_commands() {
        let mut f = make_feature("Box", Primitive::Box {
            center: [0.0; 3],
            half_extents: [5.0; 3],
        });
        f.transform.translate = [10.0, 0.0, 0.0];
        let cmds = build_step_commands(&[f]);
        let arr = cmds.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["op"], "box");
        assert_eq!(arr[1]["op"], "transform");
        assert_eq!(arr[1]["source"], "prim_0");
        assert_eq!(arr[1]["tx"], 10.0);
    }

    #[test]
    fn two_features_with_union() {
        let f1 = make_feature("Box", Primitive::Box {
            center: [0.0; 3],
            half_extents: [5.0; 3],
        });
        let f2 = make_feature("Sphere", Primitive::Sphere {
            center: [0.0; 3],
            radius: 3.0,
        });
        let cmds = build_step_commands(&[f1, f2]);
        let arr = cmds.as_array().unwrap();
        // box + sphere + union
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0]["op"], "box");
        assert_eq!(arr[1]["op"], "sphere");
        assert_eq!(arr[2]["op"], "union");
        assert_eq!(arr[2]["a"], "prim_0");
        assert_eq!(arr[2]["b"], "prim_1");
    }

    #[test]
    fn difference_operation() {
        let f1 = make_feature("Box", Primitive::Box {
            center: [0.0; 3],
            half_extents: [10.0; 3],
        });
        let mut f2 = make_feature("Cyl", Primitive::Cylinder {
            center: [0.0; 3],
            radius: 3.0,
            half_height: 15.0,
        });
        f2.op = BoolOp::Remove;
        let cmds = build_step_commands(&[f1, f2]);
        let arr = cmds.as_array().unwrap();
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[2]["op"], "difference");
    }

    #[test]
    fn excluded_features_are_skipped() {
        let f1 = make_feature("Box", Primitive::Box {
            center: [0.0; 3],
            half_extents: [5.0; 3],
        });
        let f2 = make_feature_mods(
            "Twisted",
            Primitive::Box {
                center: [0.0; 3],
                half_extents: [5.0; 3],
            },
            PrimitiveModifiers { twist_rate: 0.1, ..PrimitiveModifiers::NONE },
        );
        let f3 = make_feature("Sphere", Primitive::Sphere {
            center: [0.0; 3],
            radius: 2.0,
        });
        let cmds = build_step_commands(&[f1, f2, f3]);
        let arr = cmds.as_array().unwrap();
        // box + sphere + union (twisted box is excluded)
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0]["op"], "box");
        assert_eq!(arr[1]["op"], "sphere");
        assert_eq!(arr[2]["op"], "union");
    }

    #[test]
    fn gyroid_is_skipped() {
        let f = make_feature("Gyroid", Primitive::Gyroid {
            cell_size: 5.0,
            thickness: 1.0,
        });
        let cmds = build_step_commands(&[f]);
        let arr = cmds.as_array().unwrap();
        assert!(arr.is_empty());
    }

    #[test]
    fn cone_command() {
        let f = make_feature("Cone", Primitive::Cone {
            r_bottom: 5.0,
            r_top: 2.0,
            half_height: 10.0,
        });
        let cmds = build_step_commands(&[f]);
        let arr = cmds.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["op"], "cone");
        assert_eq!(arr[0]["rBottom"], 5.0);
        assert_eq!(arr[0]["rTop"], 2.0);
        assert_eq!(arr[0]["halfHeight"], 10.0);
    }

    #[test]
    fn torus_command() {
        let f = make_feature("Torus", Primitive::Torus {
            center: [1.0, 2.0, 3.0],
            major_radius: 10.0,
            minor_radius: 2.0,
        });
        let cmds = build_step_commands(&[f]);
        let arr = cmds.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["op"], "torus");
        assert_eq!(arr[0]["majorRadius"], 10.0);
        assert_eq!(arr[0]["minorRadius"], 2.0);
    }

    #[test]
    fn hexprism_command() {
        let f = make_feature("Hex", Primitive::HexPrism {
            apothem: 5.0,
            half_height: 3.0,
        });
        let cmds = build_step_commands(&[f]);
        let arr = cmds.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["op"], "hexprism");
        assert_eq!(arr[0]["apothem"], 5.0);
        assert_eq!(arr[0]["halfHeight"], 3.0);
    }

    #[test]
    fn extrude_polygon_wire() {
        use fielddraft_protocol::sketch2d::{Sketch2d, Sketch2dPrimitive};

        let sketch = Sketch2d::Prim(Sketch2dPrimitive::Polygon {
            vertices: vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]],
        });
        let f = make_feature("Extrude", Primitive::Extrude {
            sketch,
            half_depth_z: 5.0,
            center: [1.0, 2.0, 3.0],
            wall_subs: None,
        });
        let cmds = build_step_commands(&[f]);
        let arr = cmds.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["op"], "extrude");
        assert_eq!(arr[0]["depth"], 10.0); // 2 * half_depth_z
        let wire = arr[0]["wirePts"].as_array().unwrap();
        assert_eq!(wire.len(), 8); // 4 vertices * 2 coords
    }

    #[test]
    fn extrude_rectangle_wire() {
        use fielddraft_protocol::sketch2d::{Sketch2d, Sketch2dPrimitive};

        let sketch = Sketch2d::Prim(Sketch2dPrimitive::Rectangle {
            center: [5.0, 5.0],
            half_extents: [3.0, 4.0],
        });
        let f = make_feature("Extrude", Primitive::Extrude {
            sketch,
            half_depth_z: 7.0,
            center: [0.0, 0.0, 0.0],
            wall_subs: None,
        });
        let cmds = build_step_commands(&[f]);
        let arr = cmds.as_array().unwrap();
        assert_eq!(arr[0]["op"], "extrude");
        let wire = arr[0]["wirePts"].as_array().unwrap();
        assert_eq!(wire.len(), 8); // 4 corners * 2 coords
    }

    #[test]
    fn extrude_circle_wire_is_tessellated() {
        use fielddraft_protocol::sketch2d::{Sketch2d, Sketch2dPrimitive};

        let sketch = Sketch2d::Prim(Sketch2dPrimitive::Circle {
            center: [0.0, 0.0],
            radius: 5.0,
        });
        let f = make_feature("Extrude", Primitive::Extrude {
            sketch,
            half_depth_z: 3.0,
            center: [0.0, 0.0, 0.0],
            wall_subs: None,
        });
        let cmds = build_step_commands(&[f]);
        let arr = cmds.as_array().unwrap();
        assert_eq!(arr[0]["op"], "extrude");
        let wire = arr[0]["wirePts"].as_array().unwrap();
        assert_eq!(wire.len(), 128); // 64 points * 2 coords
    }

    #[test]
    fn revolve_produces_command() {
        use fielddraft_protocol::sketch2d::{Sketch2d, Sketch2dPrimitive};

        let sketch = Sketch2d::Prim(Sketch2dPrimitive::Rectangle {
            center: [5.0, 0.0],
            half_extents: [2.0, 3.0],
        });
        let f = make_feature("Revolve", Primitive::Revolve {
            sketch,
            center: [0.0, 0.0, 0.0],
            wall_subs: None,
        });
        let cmds = build_step_commands(&[f]);
        let arr = cmds.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["op"], "revolve");
        let angle = arr[0]["angle"].as_f64().unwrap();
        assert!((angle - std::f64::consts::TAU).abs() < 1e-10);
    }

    #[test]
    fn degraded_features_still_export() {
        let f = make_feature_mods(
            "Box",
            Primitive::Box {
                center: [0.0; 3],
                half_extents: [5.0; 3],
            },
            PrimitiveModifiers { blend_radius: 2.0, ..PrimitiveModifiers::NONE }, // Degraded, but still exportable
        );
        let cmds = build_step_commands(&[f]);
        let arr = cmds.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["op"], "box");
    }

    #[test]
    fn three_features_chain_booleans() {
        let f1 = make_feature("Box", Primitive::Box {
            center: [0.0; 3],
            half_extents: [10.0; 3],
        });
        let mut f2 = make_feature("Cyl1", Primitive::Cylinder {
            center: [0.0; 3],
            radius: 3.0,
            half_height: 15.0,
        });
        f2.op = BoolOp::Remove;
        let mut f3 = make_feature("Cyl2", Primitive::Cylinder {
            center: [5.0, 0.0, 0.0],
            radius: 2.0,
            half_height: 15.0,
        });
        f3.op = BoolOp::Remove;

        let cmds = build_step_commands(&[f1, f2, f3]);
        let arr = cmds.as_array().unwrap();
        // box + cyl1 + diff(box, cyl1) + cyl2 + diff(prev, cyl2)
        assert_eq!(arr.len(), 5);
        assert_eq!(arr[2]["op"], "difference");
        assert_eq!(arr[2]["a"], "prim_0");
        assert_eq!(arr[2]["b"], "prim_1");
        assert_eq!(arr[4]["op"], "difference");
        assert_eq!(arr[4]["a"], "bool_1");
        assert_eq!(arr[4]["b"], "prim_2");
    }

    #[test]
    fn empty_features_produces_empty_array() {
        let cmds = build_step_commands(&[]);
        let arr = cmds.as_array().unwrap();
        assert!(arr.is_empty());
    }
}
