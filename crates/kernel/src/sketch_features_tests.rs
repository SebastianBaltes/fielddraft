use super::*;

use std::str::FromStr;

use fielddraft_protocol::{Document, FeatureCall, FeatureId};
use serde_json::json;

use std::collections::HashMap;

use fielddraft_solver::{Constraint, ConstraintId, EntityId};
use serde_json::Value;

use crate::features::{Context, FeatureError, FeatureRegistry};
use crate::sketch2d::{Sketch2d, Sketch2dPrimitive, SketchSegment};

#[test]
fn open_path_curve_chains_drawn_lines_in_order() {
    // An L drawn as two connected lines → an ordered open polyline.
    let sub_calls: Vec<FeatureCall> = serde_json::from_value(json!([
        {"type":"sketch_line","id":"1","params":{"start":[0.0,0.0],"end":[10.0,0.0]}},
        {"type":"sketch_line","id":"2","params":{"start":[10.0,0.0],"end":[10.0,8.0]}}
    ]))
    .unwrap();
    let curve = open_path_curve_from_sub_calls(&sub_calls).unwrap();
    assert!(curve.len() >= 3, "got {} points", curve.len());
    assert_eq!(curve.first(), Some(&[0.0, 0.0]));
    assert_eq!(curve.last(), Some(&[10.0, 8.0]));
    // No closed loop here — extract_contours would yield nothing.
    let bounded = crate::sketch2d::BoundedSketch2d::from_sketch(
        &build_sketch2d_from_sub_calls(&sub_calls),
    );
    assert!(crate::sketch2d::extract_contours(&bounded, crate::sketch2d::CONTOUR_RESOLUTION).is_empty());
}

fn id(n: u32) -> FeatureId {
    FeatureId::from_u32(n)
}

#[test]
fn sketch_point_tool_pushes_a_point() {
    let tools = SketchToolRegistry::with_builtins();
    let mut state = SketchState::with_origin();
    let call = FeatureCall::new("sketch_point", id(1), json!({ "at": [3.0, 4.0] }));
    tools
        .apply_all(&mut state, std::slice::from_ref(&call))
        .unwrap();
    // origin + the new point
    assert_eq!(state.entities.len(), 2);
}

#[test]
fn sketch_line_tool_pushes_two_points_and_a_line() {
    let tools = SketchToolRegistry::with_builtins();
    let mut state = SketchState::new(); // no origin — keep the count easy
    let call = FeatureCall::new(
        "sketch_line",
        id(1),
        json!({ "start": [0.0, 0.0], "end": [10.0, 0.0] }),
    );
    tools
        .apply_all(&mut state, std::slice::from_ref(&call))
        .unwrap();
    // two points + one line
    assert_eq!(state.entities.len(), 3);
}

#[test]
fn sketch_ellipse_tool_allocates_centre_focus_and_ellipse() {
    let tools = SketchToolRegistry::with_builtins();
    let mut state = SketchState::new(); // no origin — keep the count easy
    let call = FeatureCall::new(
        "sketch_ellipse",
        id(1),
        json!({ "center": [0.0, 0.0], "radius_x": 10.0, "radius_y": 4.0 }),
    );
    tools
        .apply_all(&mut state, std::slice::from_ref(&call))
        .unwrap();
    // centre point (id 1) + focus point (id 2) + ellipse (id 3).
    assert_eq!(state.entities.len(), 3);
    match &state.entities[2] {
        fielddraft_solver::Entity::Ellipse {
            center,
            focus1,
            radmin,
            ..
        } => {
            assert_eq!(*center, EntityId(1));
            assert_eq!(*focus1, EntityId(2));
            // semi-minor radius = min(radius_x, radius_y) = 4.
            assert!((*radmin - 4.0).abs() < 1e-9);
        }
        other => panic!("expected Ellipse at index 2, got {other:?}"),
    }
    // Focus sits on the major (x) axis at sqrt(10² − 4²) ≈ 9.165.
    match &state.entities[1] {
        fielddraft_solver::Entity::Point { at, .. } => {
            assert!((at.x - (100.0_f64 - 16.0).sqrt()).abs() < 1e-6);
            assert!(at.y.abs() < 1e-9);
        }
        other => panic!("expected focus Point at index 1, got {other:?}"),
    }
}

#[test]
fn unknown_sketch_tool_type_is_reported() {
    let tools = SketchToolRegistry::with_builtins();
    let mut state = SketchState::new();
    let call = FeatureCall::new("not_a_tool", id(1), json!({}));
    let err = tools
        .apply_all(&mut state, std::slice::from_ref(&call))
        .unwrap_err();
    assert_eq!(
        err,
        FeatureError::UnknownFeatureType("not_a_tool".to_string())
    );
}

#[test]
fn sketch_feature_regenerates_sub_spine_into_context_sketches() {
    let mut registry = FeatureRegistry::new();
    registry.register(Box::new(SketchFeature::new()));

    let sketch_id = id(1);
    let call = FeatureCall::new(
        SketchFeature::TYPE_NAME,
        sketch_id.clone(),
        json!({
            "plane": "qCreatedBy(origin_plane_xy)",
            "sub_calls": [
                {
                    "type": "sketch_point",
                    "id": "100",
                    "params": { "at": [0.0, 0.0] }
                },
                {
                    "type": "sketch_line",
                    "id": "101",
                    "params": {
                        "start": [0.0, 0.0],
                        "end": [20.0, 0.0]
                    }
                }
            ]
        }),
    );

    let mut doc = Document::default();
    let mut ctx = Context::new(&mut doc);
    registry
        .regenerate(&mut ctx, std::slice::from_ref(&call), None)
        .unwrap();

    let data = ctx
        .sketches
        .get(&sketch_id)
        .expect("sketch should be stashed under its feature id");
    // origin (1) + sketch_point (1) + sketch_line's two points + line (3)
    assert_eq!(data.state.entities.len(), 5);
    // Sub-spine snapshot preserved for downstream profile
    // extractors (K7.0).
    assert_eq!(data.sub_calls.len(), 2);
}

#[test]
fn sketch_feature_rejects_unknown_sub_tool() {
    let mut registry = FeatureRegistry::new();
    registry.register(Box::new(SketchFeature::new()));

    let call = FeatureCall::new(
        SketchFeature::TYPE_NAME,
        id(1),
        json!({
            "plane": "qCreatedBy(origin_plane_xy)",
            "sub_calls": [
                { "type": "not_a_tool", "id": "100", "params": {} }
            ]
        }),
    );

    let mut doc = Document::default();
    let mut ctx = Context::new(&mut doc);
    let err = registry
        .regenerate(&mut ctx, std::slice::from_ref(&call), None)
        .unwrap_err();
    assert_eq!(
        err,
        FeatureError::UnknownFeatureType("not_a_tool".to_string())
    );
}

#[test]
fn sketch_feature_requires_plane() {
    let mut registry = FeatureRegistry::new();
    registry.register(Box::new(SketchFeature::new()));

    let call = FeatureCall::new(SketchFeature::TYPE_NAME, id(1), json!({}));
    let mut doc = Document::default();
    let mut ctx = Context::new(&mut doc);
    let err = registry
        .regenerate(&mut ctx, std::slice::from_ref(&call), None)
        .unwrap_err();
    assert!(matches!(
        err,
        FeatureError::MissingParameter {
            feature: SketchFeature::TYPE_NAME,
            name: "plane"
        }
    ));
}

#[test]
fn sketch_feature_roundtrips_through_json() {
    let call = FeatureCall::new(
        SketchFeature::TYPE_NAME,
        id(1),
        json!({
            "plane": "qCreatedBy(origin_plane_xy)",
            "sub_calls": [
                { "type": "sketch_point", "id": "100", "params": { "at": [1.0, 2.0] } }
            ]
        }),
    );
    let wire = serde_json::to_string(&call).unwrap();
    let back: FeatureCall = serde_json::from_str(&wire).unwrap();
    assert_eq!(back, call);
}

#[test]
fn sketch_feature_is_included_in_with_builtins() {
    let registry = FeatureRegistry::with_builtins();
    assert!(registry.get(SketchFeature::TYPE_NAME).is_some());
}

#[test]
fn sketch_registry_type_names_lists_all_builtins() {
    // K6b.3f adds sketch_circle to the built-in set.
    let tools = SketchToolRegistry::with_builtins();
    let mut names = tools.type_names();
    names.sort();
    assert_eq!(
        names,
        vec![
            "sketch_arc".to_string(),
            "sketch_circle".to_string(),
            "sketch_constraint_angle".to_string(),
            "sketch_constraint_coincident".to_string(),
            "sketch_constraint_collinear".to_string(),
            "sketch_constraint_concentric".to_string(),
            "sketch_constraint_diameter".to_string(),
            "sketch_constraint_distance".to_string(),
            "sketch_constraint_equal".to_string(),
            "sketch_constraint_fix".to_string(),
            "sketch_constraint_horizontal".to_string(),
            "sketch_constraint_midpoint".to_string(),
            "sketch_constraint_parallel".to_string(),
            "sketch_constraint_perpendicular".to_string(),
            "sketch_constraint_point_on_object".to_string(),
            "sketch_constraint_radius".to_string(),
            "sketch_constraint_symmetric".to_string(),
            "sketch_constraint_tangent".to_string(),
            "sketch_constraint_vertical".to_string(),
            "sketch_ellipse".to_string(),
            "sketch_line".to_string(),
            "sketch_mirror".to_string(),
            "sketch_offset".to_string(),
            "sketch_path".to_string(),
            "sketch_point".to_string(),
            "sketch_polygon".to_string(),
            "sketch_project".to_string(),
            "sketch_rectangle".to_string(),
            "sketch_regular_polygon".to_string(),
            "sketch_slot".to_string(),
            "sketch_spline".to_string(),
        ]
    );
}

#[test]
fn sketch_regular_polygon_square_inscribed_emits_four_points() {
    let tools = SketchToolRegistry::with_builtins();
    let mut state = SketchState::with_origin();
    let call = FeatureCall::new(
        "sketch_regular_polygon",
        id(1),
        json!({ "center": [0.0, 0.0], "radius": 10.0, "sides": 4.0, "inscribed": true }),
    );
    tools
        .apply_all(&mut state, std::slice::from_ref(&call))
        .unwrap();
    let pts: Vec<(f64, f64)> = state
        .entities
        .iter()
        .filter_map(|e| match e {
            fielddraft_solver::Entity::Point { at, .. } => Some((at.x, at.y)),
            _ => None,
        })
        .collect();
    assert_eq!(pts.len(), 4);
    // Square inscribed in r=10: vertices at top, left, bottom, right
    // (angle = π/2 + k·π/2, CCW).
    let expected = [(0.0, 10.0), (-10.0, 0.0), (0.0, -10.0), (10.0, 0.0)];
    for (got, exp) in pts.iter().zip(expected.iter()) {
        assert!(
            (got.0 - exp.0).abs() < 1e-4 && (got.1 - exp.1).abs() < 1e-4,
            "vertex {:?} != expected {:?}",
            got,
            exp
        );
    }
}

#[test]
fn sketch_regular_polygon_edge_cases() {
    // sides < 3 clamps to 3.
    let v = regular_polygon_vertices([0.0, 0.0], 5.0, 2.0, true).unwrap();
    assert_eq!(v.len(), 3);
    // non-integer sides rounds (5.6 → 6).
    let v = regular_polygon_vertices([0.0, 0.0], 5.0, 5.6, true).unwrap();
    assert_eq!(v.len(), 6);
    // circumscribed circumradius > inscribed for the same radius.
    let hex_in = regular_polygon_vertices([0.0, 0.0], 10.0, 6.0, true).unwrap();
    let hex_circ = regular_polygon_vertices([0.0, 0.0], 10.0, 6.0, false).unwrap();
    let r_in = (hex_in[0][0].powi(2) + hex_in[0][1].powi(2)).sqrt();
    let r_circ = (hex_circ[0][0].powi(2) + hex_circ[0][1].powi(2)).sqrt();
    assert!(r_circ > r_in + 1e-3, "{} !> {}", r_circ, r_in);
    // radius 0 → no vertices (rejected).
    assert!(regular_polygon_vertices([0.0, 0.0], 0.0, 6.0, true).is_none());
}

#[test]
fn sketch_circle_tool_emits_a_center_point_and_a_circle() {
    let tools = SketchToolRegistry::with_builtins();
    let mut state = SketchState::with_origin();
    let call = FeatureCall::new(
        "sketch_circle",
        id(1),
        json!({ "center": [5.0, -2.0], "radius": 7.5 }),
    );
    tools
        .apply_all(&mut state, std::slice::from_ref(&call))
        .unwrap();
    let point_count = state
        .entities
        .iter()
        .filter(|e| matches!(e, fielddraft_solver::Entity::Point { .. }))
        .count();
    let circle_count = state
        .entities
        .iter()
        .filter(|e| matches!(e, fielddraft_solver::Entity::Circle { .. }))
        .count();
    assert_eq!(point_count, 1);
    assert_eq!(circle_count, 1);
    assert!(state.constraints.is_empty());
    // Circle radius is preserved.
    let circle = state
        .entities
        .iter()
        .find(|e| matches!(e, fielddraft_solver::Entity::Circle { .. }))
        .unwrap();
    if let fielddraft_solver::Entity::Circle { radius, .. } = circle {
        assert!((radius - 7.5).abs() < 1e-9);
    } else {
        panic!("expected circle entity");
    }
}

#[test]
fn sketch_rectangle_tool_emits_four_points_four_lines_and_four_constraints() {
    let tools = SketchToolRegistry::with_builtins();
    let mut state = SketchState::with_origin();
    let call = FeatureCall::new(
        "sketch_rectangle",
        id(1),
        json!({ "corner_a": [0.0, 0.0], "corner_b": [20.0, 10.0] }),
    );
    tools
        .apply_all(&mut state, std::slice::from_ref(&call))
        .unwrap();
    let point_count = state
        .entities
        .iter()
        .filter(|e| matches!(e, fielddraft_solver::Entity::Point { .. }))
        .count();
    let line_count = state
        .entities
        .iter()
        .filter(|e| matches!(e, fielddraft_solver::Entity::Line { .. }))
        .count();
    assert_eq!(point_count, 4);
    assert_eq!(line_count, 4);
    assert_eq!(state.constraints.len(), 4);
    let h = state
        .constraints
        .iter()
        .filter(|c| matches!(c, Constraint::Horizontal { .. }))
        .count();
    let v = state
        .constraints
        .iter()
        .filter(|c| matches!(c, Constraint::Vertical { .. }))
        .count();
    assert_eq!(h, 2);
    assert_eq!(v, 2);
}

#[test]
fn sketch_rectangle_normalises_diagonal_corners() {
    // Drag from top-right to bottom-left should still produce
    // a rectangle whose bl corner is at (min_x, min_y).
    let tools = SketchToolRegistry::with_builtins();
    let mut state = SketchState::with_origin();
    tools
        .apply_all(
            &mut state,
            std::slice::from_ref(&FeatureCall::new(
                "sketch_rectangle",
                id(1),
                json!({ "corner_a": [20.0, 10.0], "corner_b": [0.0, 0.0] }),
            )),
        )
        .unwrap();
    // First allocated entity is bl, which must be at (0, 0).
    let bl = state
        .entities
        .iter()
        .find(|e| {
            matches!(
                e,
                fielddraft_solver::Entity::Point {
                    id: fielddraft_solver::EntityId(1),
                    ..
                }
            )
        })
        .unwrap();
    if let fielddraft_solver::Entity::Point { at, .. } = bl {
        assert!((at.x - 0.0).abs() < 1e-9);
        assert!((at.y - 0.0).abs() < 1e-9);
    } else {
        panic!("expected point entity");
    }
}

#[test]
fn sketch_horizontal_constraint_tool_pushes_a_horizontal_constraint() {
    let tools = SketchToolRegistry::with_builtins();
    let mut state = SketchState::with_origin();
    // Seed a line so we have an entity id to reference.
    let line_call = FeatureCall::new(
        "sketch_line",
        id(1),
        json!({ "start": [0.0, 0.0], "end": [10.0, 0.5] }),
    );
    let cstr_call = FeatureCall::new(
        "sketch_constraint_horizontal",
        id(2),
        // start-point=1, end-point=2, line=3 under with_origin().
        json!({ "line": 3 }),
    );
    tools
        .apply_all(&mut state, &[line_call, cstr_call])
        .unwrap();
    assert_eq!(state.constraints.len(), 1);
    assert!(matches!(
        state.constraints[0],
        Constraint::Horizontal {
            line: EntityId(3),
            ..
        }
    ));
}

#[test]
fn sketch_vertical_and_coincident_constraint_tools_wire_through() {
    let tools = SketchToolRegistry::with_builtins();
    let mut state = SketchState::with_origin();
    // Two free points so the coincident constraint has refs.
    tools
        .apply_all(
            &mut state,
            &[
                FeatureCall::new("sketch_point", id(1), json!({ "at": [5.0, 5.0] })),
                FeatureCall::new("sketch_point", id(2), json!({ "at": [5.0, 6.0] })),
                FeatureCall::new(
                    "sketch_constraint_coincident",
                    id(3),
                    json!({ "a": 1, "b": 2 }),
                ),
                FeatureCall::new(
                    "sketch_line",
                    id(4),
                    json!({ "start": [0.0, 0.0], "end": [0.5, 10.0] }),
                ),
                FeatureCall::new("sketch_constraint_vertical", id(5), json!({ "line": 5 })),
            ],
        )
        .unwrap();
    assert_eq!(state.constraints.len(), 2);
    assert!(state.constraints.iter().any(|c| matches!(
        c,
        Constraint::Coincident {
            a: EntityId(1),
            b: EntityId(2),
            ..
        }
    )));
    assert!(state.constraints.iter().any(|c| matches!(
        c,
        Constraint::Vertical {
            line: EntityId(5),
            ..
        }
    )));
}

#[test]
fn sketch_concentric_constraint_tool_wires_through() {
    let tools = SketchToolRegistry::with_builtins();
    let mut state = SketchState::with_origin();
    // Two circles so the concentric constraint has curve refs.
    tools
        .apply_all(
            &mut state,
            &[
                FeatureCall::new(
                    "sketch_circle",
                    id(1),
                    json!({ "center": [0.0, 0.0], "radius": 3.0 }),
                ),
                FeatureCall::new(
                    "sketch_circle",
                    id(2),
                    json!({ "center": [9.0, 0.0], "radius": 4.0 }),
                ),
                FeatureCall::new(
                    "sketch_constraint_concentric",
                    id(3),
                    json!({ "a": 2, "b": 4 }),
                ),
            ],
        )
        .unwrap();
    assert_eq!(state.constraints.len(), 1);
    assert!(state.constraints.iter().any(|c| matches!(
        c,
        Constraint::Concentric {
            a: EntityId(2),
            b: EntityId(4),
            ..
        }
    )));
}

#[test]
fn sketch_collinear_constraint_tool_wires_through() {
    // uf62: two lines made collinear via the Coincident tool's
    // line+line case emit a `Collinear` constraint referencing both
    // line ids.
    let tools = SketchToolRegistry::with_builtins();
    let mut state = SketchState::with_origin();
    tools
        .apply_all(
            &mut state,
            &[
                FeatureCall::new(
                    "sketch_line",
                    id(1),
                    json!({ "start": [0.0, 0.0], "end": [10.0, 0.0] }),
                ),
                FeatureCall::new(
                    "sketch_line",
                    id(2),
                    json!({ "start": [0.0, 5.0], "end": [10.0, 5.0] }),
                ),
                FeatureCall::new(
                    "sketch_constraint_collinear",
                    id(3),
                    json!({ "line_a": 3, "line_b": 6 }),
                ),
            ],
        )
        .unwrap();
    assert_eq!(state.constraints.len(), 1);
    assert!(state.constraints.iter().any(|c| matches!(
        c,
        Constraint::Collinear {
            a: EntityId(3),
            b: EntityId(6),
            ..
        }
    )));
}

#[test]
fn sketch_fix_constraint_tool_wires_through() {
    let tools = SketchToolRegistry::with_builtins();
    let mut state = SketchState::with_origin();
    // A single point to pin in place.
    tools
        .apply_all(
            &mut state,
            &[
                FeatureCall::new("sketch_point", id(1), json!({ "at": [3.0, 4.0] })),
                FeatureCall::new("sketch_constraint_fix", id(2), json!({ "point": 1 })),
            ],
        )
        .unwrap();
    assert_eq!(state.constraints.len(), 1);
    assert!(state.constraints.iter().any(|c| matches!(
        c,
        Constraint::Fix {
            point: EntityId(1),
            ..
        }
    )));
}

#[test]
fn sketch_point_on_object_constraint_tool_wires_through() {
    let tools = SketchToolRegistry::with_builtins();
    let mut state = SketchState::with_origin();
    // A circle (id 1) and a free point (id 2) constrained onto it.
    tools
        .apply_all(
            &mut state,
            &[
                FeatureCall::new(
                    "sketch_circle",
                    id(1),
                    json!({ "center": [0.0, 0.0], "radius": 5.0 }),
                ),
                FeatureCall::new("sketch_point", id(2), json!({ "at": [3.0, 4.0] })),
                FeatureCall::new(
                    "sketch_constraint_point_on_object",
                    id(3),
                    json!({ "point": 2, "curve": 1 }),
                ),
            ],
        )
        .unwrap();
    assert_eq!(state.constraints.len(), 1);
    assert!(state.constraints.iter().any(|c| matches!(
        c,
        Constraint::PointOnObject {
            point: EntityId(2),
            curve: EntityId(1),
            ..
        }
    )));
}

// ─── K7.0: Sketch2d synthesis ──────────────────────────────

#[test]
fn build_sketch2d_returns_empty_for_a_blank_sketch() {
    let sketch = build_sketch2d_from_sub_calls(&[]);
    assert!(matches!(sketch, Sketch2d::Empty));
}

#[test]
fn build_sketch2d_emits_a_rectangle_primitive() {
    let calls = vec![FeatureCall::new(
        "sketch_rectangle",
        id(1),
        json!({ "corner_a": [0.0, 0.0], "corner_b": [20.0, 10.0] }),
    )];
    let sketch = build_sketch2d_from_sub_calls(&calls);
    if let Sketch2d::Prim(Sketch2dPrimitive::Rectangle {
        center,
        half_extents,
    }) = sketch
    {
        assert!((center[0] - 10.0).abs() < 1e-6);
        assert!((center[1] - 5.0).abs() < 1e-6);
        assert!((half_extents[0] - 10.0).abs() < 1e-6);
        assert!((half_extents[1] - 5.0).abs() < 1e-6);
    } else {
        panic!("expected Sketch2d::Prim::Rectangle");
    }
}

#[test]
fn build_sketch2d_normalises_diagonal_rectangles() {
    // Dragged from top-right to bottom-left: corner_a is the
    // top-right, corner_b the bottom-left. The synthesis must
    // still emit a positively-oriented rectangle.
    let calls = vec![FeatureCall::new(
        "sketch_rectangle",
        id(1),
        json!({ "corner_a": [20.0, 10.0], "corner_b": [-5.0, -5.0] }),
    )];
    let sketch = build_sketch2d_from_sub_calls(&calls);
    if let Sketch2d::Prim(Sketch2dPrimitive::Rectangle { half_extents, .. }) = sketch {
        assert!(half_extents[0] > 0.0);
        assert!(half_extents[1] > 0.0);
    } else {
        panic!("expected rectangle primitive");
    }
}

#[test]
fn build_sketch2d_emits_a_circle_primitive() {
    let calls = vec![FeatureCall::new(
        "sketch_circle",
        id(1),
        json!({ "center": [3.0, -4.0], "radius": 7.0 }),
    )];
    let sketch = build_sketch2d_from_sub_calls(&calls);
    if let Sketch2d::Prim(Sketch2dPrimitive::Circle { center, radius }) = sketch {
        assert!((center[0] - 3.0).abs() < 1e-6);
        assert!((center[1] + 4.0).abs() < 1e-6);
        assert!((radius - 7.0).abs() < 1e-6);
    } else {
        panic!("expected circle primitive");
    }
}

#[test]
fn build_sketch2d_emits_an_ellipse_as_closed_quad_bezier_path() {
    // DL-49 Option B: an ellipse lowers to a closed Path of
    // quadratic Béziers, not an exact analytic primitive.
    let calls = vec![FeatureCall::new(
        "sketch_ellipse",
        id(1),
        json!({ "center": [0.0, 0.0], "radius_x": 10.0, "radius_y": 4.0 }),
    )];
    let sketch = build_sketch2d_from_sub_calls(&calls);
    let Sketch2d::Prim(Sketch2dPrimitive::Path { segments, closed }) = &sketch else {
        panic!("expected a closed Path primitive, got {sketch:?}");
    };
    assert!(*closed, "ellipse path must be closed");
    assert!(segments.len() >= 4, "expected several segments");
    assert!(segments
        .iter()
        .all(|s| matches!(s, SketchSegment::QuadBezier { .. })));
    // Sign sanity, including the major axis (the seam line) which
    // is the even-odd degeneracy that the seam-snap fixes.
    assert!(sketch.sdf([0.0, 0.0]) < 0.0, "centre inside");
    assert!(sketch.sdf([8.0, 0.0]) < 0.0, "inside along major axis");
    assert!(sketch.sdf([11.0, 0.0]) > 0.0, "outside past major axis");
    assert!(sketch.sdf([0.0, 5.0]) > 0.0, "outside past minor axis");
}

#[test]
fn build_sketch2d_emits_a_slot_as_closed_line_arc_path() {
    // A horizontal slot A=(0,0) B=(20,0) r=5 lowers to a closed
    // Path of 2 lines + 2 arcs whose caps bulge OUTWARD, so the
    // bounding box is x∈[−5,25], y∈[−5,5].
    let calls = vec![FeatureCall::new(
        "sketch_slot",
        id(1),
        json!({ "center_a": [0.0, 0.0], "center_b": [20.0, 0.0], "radius": 5.0 }),
    )];
    let sketch = build_sketch2d_from_sub_calls(&calls);
    let Sketch2d::Prim(Sketch2dPrimitive::Path { segments, closed }) = &sketch else {
        panic!("expected a closed Path primitive, got {sketch:?}");
    };
    assert!(*closed, "slot path must be closed");
    assert_eq!(segments.len(), 4, "expected 2 lines + 2 arcs");
    assert_eq!(
        segments
            .iter()
            .filter(|s| matches!(s, SketchSegment::Line { .. }))
            .count(),
        2
    );
    assert_eq!(
        segments
            .iter()
            .filter(|s| matches!(s, SketchSegment::Arc { .. }))
            .count(),
        2
    );

    // Bounding box from flattened contour samples: caps must bulge
    // outward (to x=−5 and x=+25), straight edges sit at y=±5.
    let mut min_x = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for seg in segments {
        for k in 0..=16 {
            let t = k as f32 / 16.0;
            let p = seg.point_at(t);
            min_x = min_x.min(p[0]);
            max_x = max_x.max(p[0]);
            min_y = min_y.min(p[1]);
            max_y = max_y.max(p[1]);
        }
    }
    assert!((min_x - (-5.0)).abs() < 1e-3, "min_x={min_x}");
    assert!((max_x - 25.0).abs() < 1e-3, "max_x={max_x}");
    assert!((min_y - (-5.0)).abs() < 1e-3, "min_y={min_y}");
    assert!((max_y - 5.0).abs() < 1e-3, "max_y={max_y}");

    // Sign sanity: centre line inside, well outside the caps out.
    assert!(sketch.sdf([10.0, 0.0]) < 0.0, "slot interior");
    assert!(sketch.sdf([0.0, 0.0]) < 0.0, "cap-A centre inside");
    assert!(sketch.sdf([20.0, 0.0]) < 0.0, "cap-B centre inside");
    assert!(sketch.sdf([-6.0, 0.0]) > 0.0, "past cap A outside");
    assert!(sketch.sdf([26.0, 0.0]) > 0.0, "past cap B outside");
    assert!(sketch.sdf([10.0, 6.0]) > 0.0, "past straight edge outside");
}

#[test]
fn build_sketch2d_rejects_degenerate_slot() {
    // Zero-length (coincident centres) → no primitive.
    let calls = vec![FeatureCall::new(
        "sketch_slot",
        id(1),
        json!({ "center_a": [5.0, 5.0], "center_b": [5.0, 5.0], "radius": 4.0 }),
    )];
    assert!(matches!(
        build_sketch2d_from_sub_calls(&calls),
        Sketch2d::Empty
    ));
    // Zero radius → no primitive.
    let calls = vec![FeatureCall::new(
        "sketch_slot",
        id(1),
        json!({ "center_a": [0.0, 0.0], "center_b": [20.0, 0.0], "radius": 0.0 }),
    )];
    assert!(matches!(
        build_sketch2d_from_sub_calls(&calls),
        Sketch2d::Empty
    ));
}

#[test]
fn slot_tool_apply_registers_two_centre_points() {
    // The TS mirror must add exactly two points (A then B). Origin
    // is id 0, so A=1, B=2.
    let tools = SketchToolRegistry::with_builtins();
    let mut state = SketchState::with_origin();
    let call = FeatureCall::new(
        "sketch_slot",
        id(1),
        json!({ "center_a": [1.0, 2.0], "center_b": [9.0, 2.0], "radius": 3.0 }),
    );
    tools
        .apply_all(&mut state, std::slice::from_ref(&call))
        .unwrap();
    let pts: Vec<(f64, f64)> = state
        .entities
        .iter()
        .filter_map(|e| match e {
            fielddraft_solver::Entity::Point { at, .. } => Some((at.x, at.y)),
            _ => None,
        })
        .collect();
    // Exactly two centre points, A then B.
    assert_eq!(pts.len(), 2);
    assert!((pts[0].0 - 1.0).abs() < 1e-6 && (pts[0].1 - 2.0).abs() < 1e-6);
    assert!((pts[1].0 - 9.0).abs() < 1e-6 && (pts[1].1 - 2.0).abs() < 1e-6);
}

#[test]
fn sketch_spline_tool_registers_one_point_per_control_point() {
    // The TS mirror must add exactly N points (one per control
    // point), in order. Origin is id 0, so the four control points
    // land at ids 1..=4.
    let tools = SketchToolRegistry::with_builtins();
    let mut state = SketchState::with_origin();
    let call = FeatureCall::new(
        "sketch_spline",
        id(1),
        json!({
            "points": [0.0, 0.0, 10.0, 5.0, 20.0, 0.0, 10.0, -5.0],
            "closed": false
        }),
    );
    tools
        .apply_all(&mut state, std::slice::from_ref(&call))
        .unwrap();
    let pts: Vec<(f64, f64)> = state
        .entities
        .iter()
        .filter_map(|e| match e {
            fielddraft_solver::Entity::Point { at, .. } => Some((at.x, at.y)),
            _ => None,
        })
        .collect();
    assert_eq!(pts.len(), 4);
    assert!((pts[0].0 - 0.0).abs() < 1e-6 && (pts[0].1 - 0.0).abs() < 1e-6);
    assert!((pts[3].0 - 10.0).abs() < 1e-6 && (pts[3].1 + 5.0).abs() < 1e-6);
}

#[test]
fn sketch_spline_too_few_points_is_rejected() {
    let tools = SketchToolRegistry::with_builtins();
    let mut state = SketchState::with_origin();
    let call = FeatureCall::new("sketch_spline", id(1), json!({ "points": [1.0, 2.0] }));
    assert!(tools
        .apply_all(&mut state, std::slice::from_ref(&call))
        .is_err());
}

#[test]
fn build_sketch2d_emits_a_closed_spline_as_quad_bezier_path() {
    // A closed spline through 4 points lowers to a closed Path of
    // many QuadBezier segments whose flattened bbox roughly covers
    // the control points (diamond ±10).
    let calls = vec![FeatureCall::new(
        "sketch_spline",
        id(1),
        json!({
            "points": [0.0, 10.0, 10.0, 0.0, 0.0, -10.0, -10.0, 0.0],
            "closed": true
        }),
    )];
    let sketch = build_sketch2d_from_sub_calls(&calls);
    let Sketch2d::Prim(Sketch2dPrimitive::Path { segments, closed }) = &sketch else {
        panic!("expected a closed Path primitive, got {sketch:?}");
    };
    assert!(*closed, "closed spline path must be closed");
    assert!(
        segments.len() >= 4,
        "expected multiple segments, got {}",
        segments.len()
    );
    assert!(segments
        .iter()
        .all(|s| matches!(s, SketchSegment::QuadBezier { .. })));
    // Flattened bbox roughly covers the control diamond.
    let mut min_x = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for seg in segments {
        for k in 0..=8 {
            let p = seg.point_at(k as f32 / 8.0);
            min_x = min_x.min(p[0]);
            max_x = max_x.max(p[0]);
            min_y = min_y.min(p[1]);
            max_y = max_y.max(p[1]);
        }
    }
    assert!(min_x <= -9.0 && max_x >= 9.0, "x bbox {min_x}..{max_x}");
    assert!(min_y <= -9.0 && max_y >= 9.0, "y bbox {min_y}..{max_y}");
    // The interior is inside the closed region.
    assert!(sketch.sdf([0.0, 0.0]) < 0.0, "spline centre inside");
}

#[test]
fn build_sketch2d_open_spline_is_an_open_path() {
    let calls = vec![FeatureCall::new(
        "sketch_spline",
        id(1),
        json!({ "points": [0.0, 0.0, 10.0, 5.0, 20.0, 0.0], "closed": false }),
    )];
    let sketch = build_sketch2d_from_sub_calls(&calls);
    let Sketch2d::Prim(Sketch2dPrimitive::Path { closed, .. }) = &sketch else {
        panic!("expected a Path primitive, got {sketch:?}");
    };
    assert!(!*closed, "open spline path must be open");
}

#[test]
fn catmull_rom_quad_beziers_interpolates_control_points() {
    // The fitted Path must pass through every control point: the
    // start of each span sits exactly on a control point.
    let pts = [[0.0, 0.0], [10.0, 5.0], [20.0, 0.0], [30.0, 5.0]];
    let segs = catmull_rom_quad_beziers(&pts, false, SPLINE_LOWER_TOL);
    assert!(!segs.is_empty());
    // First sample of the first span is p0.
    let first = segs[0].point_at(0.0);
    assert!((first[0] - 0.0).abs() < 1e-3 && (first[1] - 0.0).abs() < 1e-3);
    // Last sample of the last span is the final control point.
    let last = segs.last().unwrap().point_at(1.0);
    assert!((last[0] - 30.0).abs() < 1e-3 && (last[1] - 5.0).abs() < 1e-3);
    // < 2 points → empty.
    assert!(catmull_rom_quad_beziers(&[[0.0, 0.0]], false, SPLINE_LOWER_TOL).is_empty());
}

/// Exact uniform Catmull-Rom evaluation on the span p1→p2 — the
/// reference for the lowering's error bound.
fn cr_eval(p0: [f64; 2], p1: [f64; 2], p2: [f64; 2], p3: [f64; 2], t: f64) -> [f64; 2] {
    let t2 = t * t;
    let t3 = t2 * t;
    let f = |a: f64, b: f64, c: f64, d: f64| {
        0.5 * ((2.0 * b)
            + (-a + c) * t
            + (2.0 * a - 5.0 * b + 4.0 * c - d) * t2
            + (-a + 3.0 * b - 3.0 * c + d) * t3)
    };
    [f(p0[0], p1[0], p2[0], p3[0]), f(p0[1], p1[1], p2[1], p3[1])]
}

/// The adaptive lowering must (a) stay within SPLINE_LOWER_TOL of
/// the exact Catmull-Rom spline, (b) be exactly G1 at every quad
/// junction (branch/face identity must not depend on tol), and
/// (c) stay SMALL — the vase wall spline was 96 quads with the old
/// fixed 16-step sampling and is the canonical "wasteful stream"
/// case (user report 2026-06-12).
#[test]
fn catmull_rom_lowering_is_tight_g1_and_small() {
    // The vase wall profile (7 interpolation points, ~130 mm tall).
    let pts: [[f32; 2]; 7] = [
        [45.0, 68.0],
        [33.044, 50.471],
        [26.707, 42.324],
        [38.476, 20.596],
        [50.245, -9.732],
        [44.36, -45.039],
        [30.0, -62.0],
    ];
    let segs = catmull_rom_quad_beziers(&pts, false, SPLINE_LOWER_TOL);
    // ~37 with the adaptive lowering (the shoulder span carries
    // most of them) vs 96 with the old fixed 16-step sampling.
    assert!(
        segs.len() <= 40,
        "vase spline should lower to ~40 quads, got {}",
        segs.len()
    );

    // (b) exact G1 at junctions: outgoing/incoming tangent
    // directions agree to f32 rounding.
    for w in segs.windows(2) {
        let (SketchSegment::QuadBezier { p1: a1, p2: a2, .. },
             SketchSegment::QuadBezier { p0: b0, p1: b1, .. }) = (&w[0], &w[1])
        else {
            panic!("lowering must emit quads only");
        };
        let ta = [(a2[0] - a1[0]) as f64, (a2[1] - a1[1]) as f64];
        let tb = [(b1[0] - b0[0]) as f64, (b1[1] - b0[1]) as f64];
        let la = (ta[0] * ta[0] + ta[1] * ta[1]).sqrt();
        let lb = (tb[0] * tb[0] + tb[1] * tb[1]).sqrt();
        // Tangent-intersection junctions are exact; midpoint-
        // fallback junctions are capped at 2·QUAD_JUNCTION_ANGLE_
        // MAX = 1.15° (cos 0.9998) — well inside the grouping
        // threshold 0.999. Assert with a little slack.
        let cos = (ta[0] * tb[0] + ta[1] * tb[1]) / (la * lb).max(1e-12);
        assert!(cos > 0.9997, "junction tangent break: cos = {cos}");
    }

    // (a) error bound: dense exact-spline samples vs a dense
    // sampling of the lowered chain. The chain is sampled finely
    // enough (512/quad) that its own polyline error is ≪ tol.
    let chain: Vec<[f64; 2]> = segs
        .iter()
        .flat_map(|s| (0..=512).map(move |k| {
            let p = s.point_at(k as f32 / 512.0);
            [p[0] as f64, p[1] as f64]
        }))
        .collect();
    let n = pts.len();
    let mut worst = 0.0f64;
    for span in 0..n - 1 {
        let p = |idx: usize| -> [f64; 2] {
            [pts[idx][0] as f64, pts[idx][1] as f64]
        };
        let (p0, p1, p2, p3) = (
            p(span.saturating_sub(1)),
            p(span),
            p(span + 1),
            p((span + 2).min(n - 1)),
        );
        for k in 0..=128 {
            let s = cr_eval(p0, p1, p2, p3, k as f64 / 128.0);
            let d = chain
                .iter()
                .map(|c| ((c[0] - s[0]).powi(2) + (c[1] - s[1]).powi(2)).sqrt())
                .fold(f64::MAX, f64::min);
            worst = worst.max(d);
        }
    }
    assert!(
        worst <= SPLINE_LOWER_TOL * 1.05,
        "lowering deviates {worst} mm from the exact spline (tol {SPLINE_LOWER_TOL})"
    );
}

// ---- F23-full: real cubic B-spline (control mode) ----------------

/// Sample a lowered segment chain at a fraction of total arc-ish
/// parameter (segment index proportion), returning the point.
fn seg_point(segs: &[SketchSegment], frac: f32) -> [f32; 2] {
    let n = segs.len();
    if n == 0 {
        return [0.0, 0.0];
    }
    let pos = (frac * n as f32).clamp(0.0, n as f32);
    let i = (pos.floor() as usize).min(n - 1);
    let local = pos - i as f32;
    segs[i].point_at(local.clamp(0.0, 1.0))
}

#[test]
fn spline_def_roundtrip_with_mode() {
    // The new `mode` field round-trips through parse, defaulting to
    // interpolate when absent and accepting "control".
    let d_default = SketchSplineDef::parse(&json!({ "points": [0.0, 0.0, 1.0, 1.0] })).unwrap();
    assert_eq!(d_default.mode, SplineMode::Interpolate);
    assert!(!d_default.closed);
    let d_ctrl = SketchSplineDef::parse(&json!({
        "points": [0.0, 0.0, 1.0, 1.0, 2.0, 0.0, 3.0, 1.0],
        "closed": true,
        "mode": "control"
    }))
    .unwrap();
    assert_eq!(d_ctrl.mode, SplineMode::Control);
    assert!(d_ctrl.closed);
}

#[test]
fn bspline_control_mode_lowers_to_quad_path() {
    // A 4-control-point open B-spline (control mode) lowers to a
    // non-empty open Path of Line/Quad segments.
    let calls = vec![FeatureCall::new(
        "sketch_spline",
        id(1),
        json!({
            "points": [0.0, 0.0, 10.0, 20.0, 20.0, -20.0, 30.0, 0.0],
            "closed": false,
            "mode": "control"
        }),
    )];
    let sketch = build_sketch2d_from_sub_calls(&calls);
    let Sketch2d::Prim(Sketch2dPrimitive::Path { segments, closed }) = &sketch else {
        panic!("expected a Path primitive, got {sketch:?}");
    };
    assert!(!*closed);
    assert!(!segments.is_empty());
    assert!(segments.iter().all(|s| matches!(
        s,
        SketchSegment::QuadBezier { .. } | SketchSegment::Line { .. }
    )));
    // No NaN/Inf anywhere.
    for s in segments {
        for k in 0..=8 {
            let p = s.point_at(k as f32 / 8.0);
            assert!(p[0].is_finite() && p[1].is_finite(), "NaN/Inf in lowering");
        }
    }
}

#[test]
fn bspline_clamped_endpoints_are_first_and_last_control() {
    // A clamped open cubic B-spline interpolates its first/last
    // control points — the lowered chain starts at P0 and ends at
    // P_{n-1}. (This is what makes endpoint Coincident a real
    // endpoint constraint.)
    let cp = [
        [0.0, 0.0],
        [10.0, 20.0],
        [20.0, -20.0],
        [30.0, 0.0],
        [40.0, 10.0],
    ];
    let segs = bspline_quad_beziers(&cp, false, 0.01);
    assert!(!segs.is_empty());
    let start = segs[0].point_at(0.0);
    let end = segs.last().unwrap().point_at(1.0);
    assert!(
        (start[0] - 0.0).abs() < 1e-3 && (start[1] - 0.0).abs() < 1e-3,
        "start {start:?}"
    );
    assert!(
        (end[0] - 40.0).abs() < 1e-3 && (end[1] - 10.0).abs() < 1e-3,
        "end {end:?}"
    );
}

// ---- F23 endpoint tangent: control-polygon end-legs ---------------

/// Apply a spline tool call and return the resulting entities.
fn spline_entities(params: Value) -> Vec<fielddraft_solver::Entity> {
    let tools = SketchToolRegistry::with_builtins();
    let mut state = SketchState::with_origin();
    let call = FeatureCall::new("sketch_spline", id(1), params);
    tools
        .apply_all(&mut state, std::slice::from_ref(&call))
        .unwrap();
    state.entities
}

/// Collect (start, end) of every Line entity.
fn legs(entities: &[fielddraft_solver::Entity]) -> Vec<(u32, u32)> {
    entities
        .iter()
        .filter_map(|e| match e {
            fielddraft_solver::Entity::Line { start, end, .. } => Some((start.0, end.0)),
            _ => None,
        })
        .collect()
}

#[test]
fn spline_control_open_n4_has_two_legs() {
    // control + open, N>=3 → 2 legs: start=Line(cp0,cp1),
    // end=Line(cp[N-2],cp[N-1]). Origin is id 0, so cp ids are 1..=4.
    let ents = spline_entities(json!({
        "points": [0.0, 0.0, 10.0, 20.0, 20.0, -20.0, 30.0, 0.0],
        "closed": false,
        "mode": "control"
    }));
    let pts = ents
        .iter()
        .filter(|e| matches!(e, fielddraft_solver::Entity::Point { .. }))
        .count();
    assert_eq!(pts, 4, "4 control points");
    // cp ids: 1,2,3,4. start leg = (1,2), end leg = (3,4).
    assert_eq!(legs(&ents), vec![(1, 2), (3, 4)]);
}

#[test]
fn spline_control_open_n2_has_one_leg() {
    // N==2: start and end coincide → exactly one leg.
    let ents = spline_entities(json!({
        "points": [0.0, 0.0, 10.0, 20.0],
        "closed": false,
        "mode": "control"
    }));
    // cp ids 1,2; single leg (1,2).
    assert_eq!(legs(&ents), vec![(1, 2)]);
}

#[test]
fn spline_interpolate_open_n4_has_two_legs() {
    // interpolate + open, N>=3 → 2 legs, identical in form to control
    // mode: Catmull-Rom endpoint tangent P'(0)=0.5·(p1−p0) is parallel
    // to cp0→cp1 and P'(1)=0.5·(p[n-1]−p[n-2]) parallel to cp[N-2]→
    // cp[N-1]. Origin is id 0, so cp ids are 1..=4.
    let ents = spline_entities(json!({
        "points": [0.0, 0.0, 10.0, 20.0, 20.0, -20.0, 30.0, 0.0],
        "closed": false,
        "mode": "interpolate"
    }));
    let pts = ents
        .iter()
        .filter(|e| matches!(e, fielddraft_solver::Entity::Point { .. }))
        .count();
    assert_eq!(pts, 4, "4 control points");
    // cp ids: 1,2,3,4. start leg = (1,2), end leg = (3,4).
    assert_eq!(legs(&ents), vec![(1, 2), (3, 4)]);
}

#[test]
fn spline_interpolate_open_n2_has_one_leg() {
    // N==2: start and end coincide → exactly one leg.
    let ents = spline_entities(json!({
        "points": [0.0, 0.0, 10.0, 20.0],
        "closed": false,
        "mode": "interpolate"
    }));
    // cp ids 1,2; single leg (1,2).
    assert_eq!(legs(&ents), vec![(1, 2)]);
}

#[test]
fn spline_interpolate_closed_has_no_legs() {
    // Closed (periodic) interpolation spline: no free endpoints → no legs.
    let ents = spline_entities(json!({
        "points": [0.0, 0.0, 10.0, 20.0, 20.0, -20.0, 30.0, 0.0],
        "closed": true,
        "mode": "interpolate"
    }));
    assert!(
        legs(&ents).is_empty(),
        "closed interpolate spline: no endpoint legs"
    );
}

#[test]
fn spline_control_closed_has_no_legs() {
    let ents = spline_entities(json!({
        "points": [0.0, 0.0, 10.0, 20.0, 20.0, -20.0, 30.0, 0.0],
        "closed": true,
        "mode": "control"
    }));
    assert!(legs(&ents).is_empty(), "closed spline: no endpoint legs");
}

/// Collect (poles, degree) of every BSpline entity.
fn bsplines(entities: &[fielddraft_solver::Entity]) -> Vec<(Vec<u32>, u32)> {
    entities
        .iter()
        .filter_map(|e| match e {
            fielddraft_solver::Entity::BSpline { poles, degree, .. } => {
                Some((poles.iter().map(|p| p.0).collect(), *degree))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn spline_control_open_n4_allocates_cubic_bspline_after_legs() {
    // control + open, N=4 → one BSpline (poles = cp ids 1..=4, degree 3),
    // allocated AFTER the two legs so the leg ids are unchanged.
    let ents = spline_entities(json!({
        "points": [0.0, 0.0, 10.0, 20.0, 20.0, -20.0, 30.0, 0.0],
        "closed": false,
        "mode": "control"
    }));
    assert_eq!(legs(&ents), vec![(1, 2), (3, 4)]);
    assert_eq!(bsplines(&ents), vec![(vec![1, 2, 3, 4], 3)]);
}

#[test]
fn spline_control_open_clamps_degree_to_pole_count() {
    // N=2 → degree 1 (a polyline), N=3 → degree 2 (quadratic), matching
    // `bspline_quad_beziers`'s `3.min(n-1)` so solver knots == drawn curve.
    let n2 = spline_entities(json!({
        "points": [0.0, 0.0, 10.0, 20.0],
        "closed": false, "mode": "control"
    }));
    assert_eq!(bsplines(&n2), vec![(vec![1, 2], 1)]);
    let n3 = spline_entities(json!({
        "points": [0.0, 0.0, 10.0, 20.0, 20.0, 0.0],
        "closed": false, "mode": "control"
    }));
    assert_eq!(bsplines(&n3), vec![(vec![1, 2, 3], 2)]);
}

#[test]
fn spline_interpolate_open_has_no_bspline_entity() {
    // Catmull-Rom passes THROUGH its points (not poles) → no solver
    // B-spline entity; endpoint legs remain the only G1 handle.
    let ents = spline_entities(json!({
        "points": [0.0, 0.0, 10.0, 20.0, 20.0, -20.0, 30.0, 0.0],
        "closed": false, "mode": "interpolate"
    }));
    assert!(bsplines(&ents).is_empty());
}

#[test]
fn spline_control_closed_has_no_bspline_entity() {
    // Periodic knot convention differs → no solver entity for closed yet.
    let ents = spline_entities(json!({
        "points": [0.0, 0.0, 10.0, 20.0, 20.0, -20.0, 30.0, 0.0],
        "closed": true, "mode": "control"
    }));
    assert!(bsplines(&ents).is_empty());
}

#[test]
fn tangent_constraint_on_leg_roundtrips_through_serde() {
    // A Tangent (and Parallel) constraint referencing a leg line id
    // round-trips through serde JSON. The leg is just a `line`, so
    // existing line constraints apply unchanged.
    let tangent = Constraint::Tangent {
        id: ConstraintId(7),
        line: EntityId(3),
        curve: EntityId(9),
    };
    let json = serde_json::to_value(&tangent).unwrap();
    let back: Constraint = serde_json::from_value(json).unwrap();
    assert_eq!(tangent, back);

    let parallel = Constraint::Parallel {
        id: ConstraintId(8),
        a: EntityId(3),
        b: EntityId(5),
    };
    let json = serde_json::to_value(&parallel).unwrap();
    let back: Constraint = serde_json::from_value(json).unwrap();
    assert_eq!(parallel, back);
}

#[test]
fn build_geometry_is_unchanged_by_leg_lines() {
    // The leg lines live ONLY in the entity/constraint world. The
    // extrude profile is lowered from the sub-call params via
    // build_sketch2d_from_sub_calls, which never reads entities, so
    // the resulting Path primitive is identical whether or not legs
    // would have been allocated. We assert the lowered Path here is
    // exactly what bspline_control_mode_lowers_to_quad_path expects.
    let calls = vec![FeatureCall::new(
        "sketch_spline",
        id(1),
        json!({
            "points": [0.0, 0.0, 10.0, 20.0, 20.0, -20.0, 30.0, 0.0],
            "closed": false,
            "mode": "control"
        }),
    )];
    let sketch = build_sketch2d_from_sub_calls(&calls);
    let Sketch2d::Prim(Sketch2dPrimitive::Path { segments, closed }) = &sketch else {
        panic!("expected a Path primitive, got {sketch:?}");
    };
    assert!(!*closed);
    assert!(!segments.is_empty());
    // All segments are still Line/QuadBezier — no spurious leg geometry.
    assert!(segments.iter().all(|s| matches!(
        s,
        SketchSegment::QuadBezier { .. } | SketchSegment::Line { .. }
    )));
    // Endpoints unchanged: chain starts at cp0 and ends at cp[N-1].
    let start = segments[0].point_at(0.0);
    let end = segments.last().unwrap().point_at(1.0);
    assert!((start[0] - 0.0).abs() < 1e-3 && (start[1] - 0.0).abs() < 1e-3);
    assert!((end[0] - 30.0).abs() < 1e-3 && (end[1] - 0.0).abs() < 1e-3);
}

/// Min distance from `q` to the polyline of lowered segments.
fn dist_to_chain(segs: &[SketchSegment], q: [f64; 2]) -> f64 {
    let mut best = f64::INFINITY;
    for s in segs {
        let mut prev = s.point_at(0.0);
        for k in 1..=16 {
            let cur = s.point_at(k as f32 / 16.0);
            // distance from q to segment prev→cur
            let ax = prev[0] as f64;
            let ay = prev[1] as f64;
            let bx = cur[0] as f64;
            let by = cur[1] as f64;
            let dx = bx - ax;
            let dy = by - ay;
            let len2 = dx * dx + dy * dy;
            let t = if len2 < 1e-12 {
                0.0
            } else {
                (((q[0] - ax) * dx + (q[1] - ay) * dy) / len2).clamp(0.0, 1.0)
            };
            let px = ax + t * dx;
            let py = ay + t * dy;
            best = best.min(((q[0] - px).powi(2) + (q[1] - py).powi(2)).sqrt());
            prev = cur;
        }
    }
    best
}

#[test]
fn bspline_midparam_matches_de_boor() {
    // The exactly-lowered curve must pass through the de Boor
    // evaluation of the same B-spline at several parameters,
    // including the mid-parameter, within the flatness tolerance.
    // (de Boor is the analytic ground truth; the lowered chain
    // must lie on it.)
    let cp = [
        [0.0, 0.0],
        [10.0, 30.0],
        [30.0, 30.0],
        [40.0, 0.0],
        [50.0, 20.0],
    ];
    let segs = bspline_quad_beziers(&cp, false, 0.005);
    for &t in &[0.0, 0.25, 0.5, 0.75, 1.0] {
        let analytic = bspline_eval_open(&cp, 3, t);
        let d = dist_to_chain(&segs, analytic);
        assert!(
            d < 0.05,
            "de Boor t={t} {analytic:?} is {d} from lowered chain"
        );
    }
    // Spot-check the hand-relevant mid value sits on the curve.
    let mid = bspline_eval_open(&cp, 3, 0.5);
    assert!(mid[0].is_finite() && mid[1].is_finite());
}

#[test]
fn bspline_two_control_points_is_a_line() {
    let cp = [[0.0, 0.0], [10.0, 5.0]];
    let segs = bspline_quad_beziers(&cp, false, 0.05);
    assert_eq!(segs.len(), 1);
    assert!(matches!(segs[0], SketchSegment::Line { .. }));
    let start = segs[0].point_at(0.0);
    let end = segs[0].point_at(1.0);
    assert!((start[0] - 0.0).abs() < 1e-4 && (end[0] - 10.0).abs() < 1e-4);
}

#[test]
fn bspline_three_control_points_is_degree_two() {
    // 3 control points → degree 2; the lowered curve still starts
    // and ends at the first/last control point (clamped quad).
    let cp = [[0.0, 0.0], [5.0, 10.0], [10.0, 0.0]];
    let segs = bspline_quad_beziers(&cp, false, 0.05);
    assert!(!segs.is_empty());
    let start = segs[0].point_at(0.0);
    let end = segs.last().unwrap().point_at(1.0);
    assert!((start[0] - 0.0).abs() < 1e-3 && (start[1] - 0.0).abs() < 1e-3);
    assert!((end[0] - 10.0).abs() < 1e-3 && (end[1] - 0.0).abs() < 1e-3);
    // Mid bows up toward the middle control point.
    let mid = seg_point(&segs, 0.5);
    assert!(mid[1] > 0.5, "degree-2 curve should bow up, mid={mid:?}");
}

#[test]
fn bspline_collinear_stays_straight() {
    // Collinear control points → a straight curve (all samples on
    // the x-axis), no division-by-zero / NaN.
    let cp = [[0.0, 0.0], [10.0, 0.0], [20.0, 0.0], [30.0, 0.0]];
    let segs = bspline_quad_beziers(&cp, false, 0.05);
    assert!(!segs.is_empty());
    for s in &segs {
        for k in 0..=8 {
            let p = s.point_at(k as f32 / 8.0);
            assert!(p[1].abs() < 1e-3, "off-axis point {p:?}");
            assert!(p[0].is_finite() && p[1].is_finite());
        }
    }
}

#[test]
fn bspline_closed_wraps_into_a_region() {
    // A closed (periodic) cubic B-spline over a square-ish control
    // polygon lowers to a closed Path whose interior contains the
    // centroid (extrudable region), and the curve forms a loop.
    let calls = vec![FeatureCall::new(
        "sketch_spline",
        id(1),
        json!({
            "points": [-10.0, -10.0, 10.0, -10.0, 10.0, 10.0, -10.0, 10.0],
            "closed": true,
            "mode": "control"
        }),
    )];
    let sketch = build_sketch2d_from_sub_calls(&calls);
    let Sketch2d::Prim(Sketch2dPrimitive::Path { segments, closed }) = &sketch else {
        panic!("expected closed Path, got {sketch:?}");
    };
    assert!(*closed, "closed B-spline path must be closed");
    assert!(segments.len() >= 4);
    // The wrap is a true loop: end of last seg ≈ start of first.
    let first = segments[0].point_at(0.0);
    let last = segments.last().unwrap().point_at(1.0);
    let gap = ((first[0] - last[0]).powi(2) + (first[1] - last[1]).powi(2)).sqrt();
    assert!(gap < 1e-2, "closed loop gap {gap}");
    // Centre inside the region.
    assert!(sketch.sdf([0.0, 0.0]) < 0.0, "closed spline centre inside");
    // No NaN/Inf.
    for s in segments {
        for k in 0..=8 {
            let p = s.point_at(k as f32 / 8.0);
            assert!(p[0].is_finite() && p[1].is_finite());
        }
    }
}

#[test]
fn bspline_too_few_control_points_no_panic() {
    // < 2 control points → empty (no panic), and a closed 3-point
    // spline lowers without panicking.
    assert!(bspline_quad_beziers(&[[0.0, 0.0]], false, 0.05).is_empty());
    assert!(bspline_quad_beziers(&[], true, 0.05).is_empty());
    let segs = bspline_quad_beziers(&[[0.0, 0.0], [10.0, 0.0], [5.0, 8.0]], true, 0.05);
    assert!(!segs.is_empty());
    for s in &segs {
        let p = s.point_at(0.5);
        assert!(p[0].is_finite() && p[1].is_finite());
    }
}

#[test]
fn bspline_endpoint_coincident_constraint_wires_to_first_control() {
    // F23-full solver: a clamped B-spline's first control point IS
    // the curve endpoint AND a registered point entity (control mode
    // registers one point per control point, same as interpolate).
    // So a Coincident constraint referencing that entity id is a
    // true endpoint constraint the solver acts on. Here we confirm
    // the constraint wires through to the spline's first control
    // point entity (id 2 after origin=0, free point=1, spline
    // controls=2..=5).
    let tools = SketchToolRegistry::with_builtins();
    let mut state = SketchState::with_origin();
    let calls = vec![
        FeatureCall::new("sketch_point", id(1), json!({ "at": [5.0, 5.0] })),
        FeatureCall::new(
            "sketch_spline",
            id(2),
            json!({
                "points": [0.0, 0.0, 10.0, 20.0, 20.0, -20.0, 30.0, 0.0],
                "closed": false,
                "mode": "control"
            }),
        ),
        FeatureCall::new(
            "sketch_constraint_coincident",
            id(3),
            json!({ "a": 1, "b": 2 }),
        ),
    ];
    tools.apply_all(&mut state, &calls).unwrap();
    // Entity 2 is the spline's FIRST control point, at (0,0) — the
    // clamped curve endpoint.
    let first_ctrl = state.entities.iter().find(|e| {
        matches!(
            e,
            fielddraft_solver::Entity::Point {
                id: EntityId(2),
                ..
            }
        )
    });
    if let Some(fielddraft_solver::Entity::Point { at, .. }) = first_ctrl {
        assert!((at.x - 0.0).abs() < 1e-6 && (at.y - 0.0).abs() < 1e-6);
    } else {
        panic!("entity 2 is not the spline's first control point");
    }
    // The coincident constraint binds the free point (1) to that
    // first control point (2).
    assert!(state.constraints.iter().any(|c| matches!(
        c,
        Constraint::Coincident {
            a: EntityId(1),
            b: EntityId(2),
            ..
        }
    )));
}

#[test]
fn build_sketch2d_unions_multiple_region_primitives() {
    let calls = vec![
        FeatureCall::new(
            "sketch_rectangle",
            id(1),
            json!({ "corner_a": [-10.0, -10.0], "corner_b": [10.0, 10.0] }),
        ),
        FeatureCall::new(
            "sketch_circle",
            id(2),
            json!({ "center": [20.0, 0.0], "radius": 5.0 }),
        ),
    ];
    let sketch = build_sketch2d_from_sub_calls(&calls);
    let Sketch2d::Union(_, _) = &sketch else {
        panic!("expected Union of two primitives, got {:?}", sketch);
    };
    // Spot-check: origin lives inside the rectangle, `(20, 0)`
    // is the centre of the circle. Both should be strictly
    // inside the combined region (negative SDF).
    assert!(sketch.sdf([0.0, 0.0]) < 0.0);
    assert!(sketch.sdf([20.0, 0.0]) < 0.0);
    // A point far from both is outside.
    assert!(sketch.sdf([100.0, 100.0]) > 0.0);
}

#[test]
fn resolve_profile_query_restricts_to_one_region() {
    // Ticket dhjn: a two-region sketch (rectangle + separate circle).
    // `qCreatedBy` takes BOTH; `qRegion(_, i)` takes only the one the
    // index points at.
    let calls = vec![
        FeatureCall::new(
            "sketch_rectangle",
            id(2),
            json!({ "corner_a": [-10.0, -10.0], "corner_b": [10.0, 10.0] }),
        ),
        FeatureCall::new(
            "sketch_circle",
            id(3),
            json!({ "center": [40.0, 0.0], "radius": 5.0 }),
        ),
    ];
    let mut sketches: HashMap<FeatureId, SketchData> = HashMap::new();
    sketches.insert(
        id(1),
        SketchData {
            state: SketchState::with_origin(),
            sub_calls: calls,
            frame: SketchFrame::IDENTITY,
        },
    );

    // Whole sketch: both regions inside.
    let (whole, _, _) = resolve_profile_query("qCreatedBy(1)", &sketches).unwrap();
    assert!(whole.sdf([0.0, 0.0]) < 0.0);
    assert!(whole.sdf([40.0, 0.0]) < 0.0);

    // Find which region index is the circle (centre x ≈ 40).
    let regions =
        crate::sketch_regions::find_regions(&whole, PROFILE_REGION_RESOLUTION);
    assert_eq!(regions.len(), 2);
    let circle = regions.iter().find(|r| r.centroid[0] > 20.0).unwrap();

    let (just_circle, _, _) =
        resolve_profile_query(&format!("qRegion(1, {})", circle.index), &sketches)
            .unwrap();
    assert!(just_circle.sdf([40.0, 0.0]) < 0.0, "circle kept");
    assert!(just_circle.sdf([0.0, 0.0]) > 0.0, "rectangle dropped");
}

#[test]
fn region_query_at_point_matches_listed_index() {
    // A point inside a region resolves to that region's index — the same
    // index `find_regions` reports (ticket dhjn: UI ↔ kernel agreement).
    let sketch = Sketch2d::Union(
        Box::new(Sketch2d::Prim(Sketch2dPrimitive::Circle {
            center: [-20.0, 0.0],
            radius: 5.0,
        })),
        Box::new(Sketch2d::Prim(Sketch2dPrimitive::Circle {
            center: [20.0, 0.0],
            radius: 5.0,
        })),
    );
    let regions =
        crate::sketch_regions::find_regions(&sketch, PROFILE_REGION_RESOLUTION);
    for r in &regions {
        let idx = crate::sketch_regions::region_index_at_point(
            &sketch,
            r.interior,
            PROFILE_REGION_RESOLUTION,
        );
        assert_eq!(idx, Some(r.index));
    }
    // Outside every region → None.
    assert_eq!(
        crate::sketch_regions::region_index_at_point(
            &sketch,
            [0.0, 0.0],
            PROFILE_REGION_RESOLUTION
        ),
        None
    );
}

#[test]
fn loop_inference_closes_a_rectangle_of_free_lines() {
    // Four free lines whose endpoints coincide exactly — drawn in
    // mixed directions to exercise the reversed() chaining. K7.1
    // must stitch them into one closed region.
    let calls = vec![
        FeatureCall::new(
            "sketch_line",
            id(1),
            json!({ "start": [0.0, 0.0], "end": [10.0, 0.0] }),
        ),
        FeatureCall::new(
            "sketch_line",
            id(2),
            json!({ "start": [10.0, 8.0], "end": [10.0, 0.0] }),
        ),
        FeatureCall::new(
            "sketch_line",
            id(3),
            json!({ "start": [10.0, 8.0], "end": [0.0, 8.0] }),
        ),
        FeatureCall::new(
            "sketch_line",
            id(4),
            json!({ "start": [0.0, 0.0], "end": [0.0, 8.0] }),
        ),
    ];
    let sketch = build_sketch2d_from_sub_calls(&calls);
    assert!(sketch.sdf([5.0, 4.0]) < 0.0, "rectangle centre inside");
    assert!(sketch.sdf([15.0, 4.0]) > 0.0, "right of rectangle outside");
    assert!(sketch.sdf([5.0, -3.0]) > 0.0, "below rectangle outside");
}

#[test]
fn loop_inference_ignores_dangling_stub() {
    // Ticket mdye: a closed rectangle with an OVERHANGING line (a "flag
    // pole" hanging off the bottom-left corner). The stub must not stop the
    // rectangle from being recognised as a closed region.
    let calls = vec![
        FeatureCall::new(
            "sketch_line",
            id(1),
            json!({ "start": [0.0, 0.0], "end": [10.0, 0.0] }),
        ),
        FeatureCall::new(
            "sketch_line",
            id(2),
            json!({ "start": [10.0, 0.0], "end": [10.0, 8.0] }),
        ),
        FeatureCall::new(
            "sketch_line",
            id(3),
            json!({ "start": [10.0, 8.0], "end": [0.0, 8.0] }),
        ),
        FeatureCall::new(
            "sketch_line",
            id(4),
            json!({ "start": [0.0, 8.0], "end": [0.0, 0.0] }),
        ),
        // The overhanging stub: from the bottom-left corner straight down.
        FeatureCall::new(
            "sketch_line",
            id(5),
            json!({ "start": [0.0, 0.0], "end": [0.0, -10.0] }),
        ),
    ];
    let sketch = build_sketch2d_from_sub_calls(&calls);
    assert!(sketch.sdf([5.0, 4.0]) < 0.0, "rectangle centre still inside despite the stub");
    assert!(sketch.sdf([15.0, 4.0]) > 0.0, "right of rectangle outside");
    // The stub is a zero-area line, not a region: a point beside it is out.
    assert!(sketch.sdf([2.0, -5.0]) > 0.0, "the overhanging stub is not a filled region");
    let regions = crate::sketch_regions::find_regions(&sketch, 96);
    assert_eq!(regions.len(), 1, "exactly one region — the rectangle");
}

#[test]
fn loop_inference_closes_overshooting_edge() {
    // Ticket mdye (the actual case): the left edge OVERSHOOTS below the
    // bottom-left corner, and the bottom edge meets it partway along (a
    // T-junction, not a shared endpoint). The overhang must not stop the
    // rectangle from closing — the line is split at the junction.
    let calls = vec![
        FeatureCall::new(
            "sketch_line",
            id(1),
            json!({ "start": [0.0, 0.0], "end": [10.0, 0.0] }),
        ),
        FeatureCall::new(
            "sketch_line",
            id(2),
            json!({ "start": [10.0, 0.0], "end": [10.0, 8.0] }),
        ),
        FeatureCall::new(
            "sketch_line",
            id(3),
            json!({ "start": [10.0, 8.0], "end": [0.0, 8.0] }),
        ),
        // Left edge runs from the top-left corner PAST the bottom-left
        // corner down to (0, -10); (0, 0) is on its interior.
        FeatureCall::new(
            "sketch_line",
            id(4),
            json!({ "start": [0.0, 8.0], "end": [0.0, -10.0] }),
        ),
    ];
    let sketch = build_sketch2d_from_sub_calls(&calls);
    assert!(sketch.sdf([5.0, 4.0]) < 0.0, "rectangle centre inside despite overshoot");
    assert!(sketch.sdf([15.0, 4.0]) > 0.0, "right of rectangle outside");
    let regions = crate::sketch_regions::find_regions(&sketch, 96);
    assert_eq!(regions.len(), 1, "exactly one region — the rectangle");
}

#[test]
fn loop_inference_mixes_lines_and_open_spline() {
    // A vase-like profile: lines on three sides, an open
    // interpolated spline as the fourth. The chain closes through
    // the spline's endpoints.
    let calls = vec![
        FeatureCall::new(
            "sketch_line",
            id(1),
            json!({ "start": [0.0, 0.0], "end": [0.0, 20.0] }),
        ),
        FeatureCall::new(
            "sketch_line",
            id(2),
            json!({ "start": [0.0, 20.0], "end": [10.0, 20.0] }),
        ),
        FeatureCall::new(
            "sketch_spline",
            id(3),
            json!({
                "points": [10.0, 20.0, 12.0, 10.0, 8.0, 0.0],
                "closed": false,
                "mode": "interpolate"
            }),
        ),
        FeatureCall::new(
            "sketch_line",
            id(4),
            json!({ "start": [8.0, 0.0], "end": [0.0, 0.0] }),
        ),
    ];
    let sketch = build_sketch2d_from_sub_calls(&calls);
    assert!(sketch.sdf([5.0, 10.0]) < 0.0, "interior inside");
    assert!(sketch.sdf([-5.0, 10.0]) > 0.0, "left of axis outside");
    assert!(sketch.sdf([20.0, 10.0]) > 0.0, "far right outside");
}

#[test]
fn loop_inference_closes_arc_and_line_into_half_disc() {
    // Ticket 6b41: a semicircle drawn as a diameter LINE plus a half-circle
    // ARC must close into one region (the half-disc). The arc spans the
    // upper half (start 0, sweep +π) from (10,0) over (0,10) to (−10,0); the
    // line runs back across the diameter. Before the fix the arc was lowered
    // to a full pie-`Sector` and never chained, so the half-disc had no
    // region and the profile picker couldn't pick it.
    let calls = vec![
        FeatureCall::new(
            "sketch_arc",
            id(1),
            json!({
                "center": [0.0, 0.0],
                "radius": 10.0,
                "start_angle": 0.0,
                "sweep_angle": std::f32::consts::PI
            }),
        ),
        FeatureCall::new(
            "sketch_line",
            id(2),
            json!({ "start": [-10.0, 0.0], "end": [10.0, 0.0] }),
        ),
    ];
    let sketch = build_sketch2d_from_sub_calls(&calls);
    assert!(sketch.sdf([0.0, 5.0]) < 0.0, "inside the upper half-disc");
    assert!(sketch.sdf([0.0, -5.0]) > 0.0, "below the diameter outside");
    assert!(sketch.sdf([0.0, 15.0]) > 0.0, "above the arc outside");
    // The half-disc is exactly ONE detected region, pickable by qRegion(_,0).
    let regions =
        crate::sketch_regions::find_regions(&sketch, PROFILE_REGION_RESOLUTION);
    assert_eq!(regions.len(), 1, "one region for the half-disc");
    assert_eq!(
        crate::sketch_regions::region_index_at_point(
            &sketch,
            [0.0, 5.0],
            PROFILE_REGION_RESOLUTION
        ),
        Some(0),
        "click inside the half-disc resolves to region 0"
    );
    assert_eq!(
        crate::sketch_regions::region_index_at_point(
            &sketch,
            [0.0, -5.0],
            PROFILE_REGION_RESOLUTION
        ),
        None,
        "click below the diameter resolves to no region"
    );
}

#[test]
fn lone_full_circle_arc_stays_a_sector() {
    // A near-full-circle `sketch_arc` is closed on its own; it must NOT be
    // pulled into the line chainer (it would never chain anyway), and stays
    // lowered to a `Sector` disc so a lone arc keeps its standalone region.
    let calls = vec![FeatureCall::new(
        "sketch_arc",
        id(1),
        json!({
            "center": [0.0, 0.0],
            "radius": 8.0,
            "start_angle": 0.0,
            "sweep_angle": std::f32::consts::TAU
        }),
    )];
    let sketch = build_sketch2d_from_sub_calls(&calls);
    assert!(sketch.sdf([0.0, 0.0]) < 0.0, "disc centre inside");
    assert!(sketch.sdf([0.0, 12.0]) > 0.0, "outside the disc");
}

#[test]
fn loop_inference_leaves_open_chains_empty() {
    // Three sides of a square — endpoint chain never closes, so
    // no region may be invented.
    let calls = vec![
        FeatureCall::new(
            "sketch_line",
            id(1),
            json!({ "start": [0.0, 0.0], "end": [10.0, 0.0] }),
        ),
        FeatureCall::new(
            "sketch_line",
            id(2),
            json!({ "start": [10.0, 0.0], "end": [10.0, 8.0] }),
        ),
        FeatureCall::new(
            "sketch_line",
            id(3),
            json!({ "start": [10.0, 8.0], "end": [0.0, 8.0] }),
        ),
    ];
    assert!(matches!(
        build_sketch2d_from_sub_calls(&calls),
        Sketch2d::Empty
    ));
}

#[test]
fn loop_inference_two_disjoint_loops_union() {
    // Two separate closed triangles of free lines → a Union with
    // both interiors filled.
    let mut calls = Vec::new();
    let tri = |o: f32, base: u32| -> Vec<FeatureCall> {
        vec![
            FeatureCall::new(
                "sketch_line",
                id(base),
                json!({ "start": [o, 0.0], "end": [o + 6.0, 0.0] }),
            ),
            FeatureCall::new(
                "sketch_line",
                id(base + 1),
                json!({ "start": [o + 6.0, 0.0], "end": [o + 3.0, 6.0] }),
            ),
            FeatureCall::new(
                "sketch_line",
                id(base + 2),
                json!({ "start": [o + 3.0, 6.0], "end": [o, 0.0] }),
            ),
        ]
    };
    calls.extend(tri(0.0, 1));
    calls.extend(tri(20.0, 4));
    let sketch = build_sketch2d_from_sub_calls(&calls);
    assert!(sketch.sdf([3.0, 2.0]) < 0.0, "first triangle inside");
    assert!(sketch.sdf([23.0, 2.0]) < 0.0, "second triangle inside");
    assert!(sketch.sdf([13.0, 2.0]) > 0.0, "between triangles outside");
}

#[test]
fn build_sketch2d_ignores_lines_points_and_constraints() {
    // None of these calls form a closed region on their own,
    // so the K7.0 synthesis drops them. A sketch made only
    // of loose lines yields an empty Sketch2d.
    let calls = vec![
        FeatureCall::new(
            "sketch_line",
            id(1),
            json!({ "start": [0.0, 0.0], "end": [10.0, 0.0] }),
        ),
        FeatureCall::new("sketch_point", id(2), json!({ "at": [5.0, 5.0] })),
        FeatureCall::new("sketch_constraint_horizontal", id(3), json!({ "line": 3 })),
    ];
    assert!(matches!(
        build_sketch2d_from_sub_calls(&calls),
        Sketch2d::Empty
    ));
}

#[test]
fn build_sketch2d_mirror_unions_a_reflected_copy() {
    // A rectangle in the +x half-plane, then a mirror across the
    // Y axis (axis = 1 flips X). The result is a Union of the
    // original and its reflection: the original centre stays
    // inside and the reflected centre (−x) becomes inside too.
    let calls = vec![
        FeatureCall::new(
            "sketch_rectangle",
            id(1),
            json!({ "corner_a": [10.0, -5.0], "corner_b": [20.0, 5.0] }),
        ),
        FeatureCall::new("sketch_mirror", id(2), json!({ "axis": 1.0 })),
    ];
    let sketch = build_sketch2d_from_sub_calls(&calls);
    let Sketch2d::Union(_, _) = &sketch else {
        panic!("expected a Union (original ∪ mirrored), got {sketch:?}");
    };
    assert!(sketch.sdf([15.0, 0.0]) < 0.0, "original centre inside");
    assert!(sketch.sdf([-15.0, 0.0]) < 0.0, "mirrored centre inside");
}

#[test]
fn build_sketch2d_mirror_without_prior_geometry_is_empty() {
    // Mirror has nothing to reflect when no region precedes it.
    let calls = vec![FeatureCall::new(
        "sketch_mirror",
        id(1),
        json!({ "axis": 0.0 }),
    )];
    assert!(matches!(
        build_sketch2d_from_sub_calls(&calls),
        Sketch2d::Empty
    ));
}

#[test]
fn build_sketch2d_offset_grows_the_profile() {
    // A 20×20 rectangle (half-extent 10) grown by 5 mm. A point at
    // (13, 0) sits outside the original but inside the offset, and
    // the tree is wrapped in a single Offset node carrying +5.
    let calls = vec![
        FeatureCall::new(
            "sketch_rectangle",
            id(1),
            json!({ "corner_a": [-10.0, -10.0], "corner_b": [10.0, 10.0] }),
        ),
        FeatureCall::new("sketch_offset", id(2), json!({ "distance": 5.0 })),
    ];
    let sketch = build_sketch2d_from_sub_calls(&calls);
    let Sketch2d::Offset { distance, .. } = &sketch else {
        panic!("expected an Offset node wrapping the profile, got {sketch:?}");
    };
    assert!((distance - 5.0).abs() < 1e-6, "offset distance preserved");
    assert!(
        sketch.sdf([13.0, 0.0]) < 0.0,
        "point inside the grown profile"
    );
    // Beyond the grown boundary it is still outside.
    assert!(
        sketch.sdf([16.0, 0.0]) > 0.0,
        "point past the grown boundary"
    );
}

#[test]
fn build_sketch2d_offset_inward_shrinks_the_profile() {
    // Negative distance shrinks: a point just inside the original
    // boundary falls outside the inset profile.
    let calls = vec![
        FeatureCall::new(
            "sketch_rectangle",
            id(1),
            json!({ "corner_a": [-10.0, -10.0], "corner_b": [10.0, 10.0] }),
        ),
        FeatureCall::new("sketch_offset", id(2), json!({ "distance": -4.0 })),
    ];
    let sketch = build_sketch2d_from_sub_calls(&calls);
    assert!(sketch.sdf([8.0, 0.0]) > 0.0, "near-edge point now outside");
    assert!(sketch.sdf([0.0, 0.0]) < 0.0, "centre still inside");
}

#[test]
fn build_sketch2d_excludes_construction_geometry() {
    // A solid rectangle plus a construction circle. The circle must
    // not contribute to the region: its centre is outside the solid.
    let calls = vec![
        FeatureCall::new(
            "sketch_rectangle",
            id(1),
            json!({ "corner_a": [-10.0, -10.0], "corner_b": [10.0, 10.0] }),
        ),
        FeatureCall::new(
            "sketch_circle",
            id(2),
            json!({ "center": [50.0, 0.0], "radius": 5.0, "construction": true }),
        ),
    ];
    let sketch = build_sketch2d_from_sub_calls(&calls);
    // Only the rectangle survives — no Union with the circle.
    let Sketch2d::Prim(Sketch2dPrimitive::Rectangle { .. }) = &sketch else {
        panic!("expected a lone rectangle primitive, got {sketch:?}");
    };
    // The construction circle's centre is firmly outside the region.
    assert!(
        sketch.sdf([50.0, 0.0]) > 0.0,
        "construction circle not extruded"
    );
}

#[test]
fn build_sketch2d_construction_flag_defaults_false() {
    // Without the flag the circle is a normal region primitive.
    let calls = vec![FeatureCall::new(
        "sketch_circle",
        id(1),
        json!({ "center": [0.0, 0.0], "radius": 5.0 }),
    )];
    assert!(matches!(
        build_sketch2d_from_sub_calls(&calls),
        Sketch2d::Prim(Sketch2dPrimitive::Circle { .. })
    ));
}

#[test]
fn build_sketch2d_skips_degenerate_primitives() {
    // Zero-area rectangle and zero-radius circle don't
    // contribute to the output.
    let calls = vec![
        FeatureCall::new(
            "sketch_rectangle",
            id(1),
            json!({ "corner_a": [3.0, 3.0], "corner_b": [3.0, 3.0] }),
        ),
        FeatureCall::new(
            "sketch_circle",
            id(2),
            json!({ "center": [0.0, 0.0], "radius": 0.0 }),
        ),
    ];
    assert!(matches!(
        build_sketch2d_from_sub_calls(&calls),
        Sketch2d::Empty
    ));
}

#[test]
fn sketch_point_schema_reports_vec2_shape() {
    let schema = SketchPointDef::schema_json();
    let params = schema.get("params").unwrap().as_array().unwrap();
    assert_eq!(params.len(), 1);
    let at = &params[0];
    assert_eq!(at.get("key").unwrap(), "at");
    assert_eq!(at.get("type").unwrap(), "vec2");
    assert_eq!(at.get("required").unwrap(), true);
}

// ─── C1: sketch frames ─────────────────────────────────────────

fn is_orthonormal(f: &SketchFrame) -> bool {
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let unit = |a: [f32; 3]| (dot(a, a) - 1.0).abs() < 1e-4;
    unit(f.x_axis)
        && unit(f.y_axis)
        && unit(f.normal)
        && dot(f.x_axis, f.y_axis).abs() < 1e-4
        && dot(f.x_axis, f.normal).abs() < 1e-4
        && dot(f.y_axis, f.normal).abs() < 1e-4
}

#[test]
fn datum_plane_frames_are_orthonormal_and_well_known() {
    let xy = datum_plane_frame(&FeatureId::from_str("origin_plane_xy").unwrap()).unwrap();
    assert_eq!(xy, SketchFrame::IDENTITY);
    assert!(
        xy.to_transform().is_identity(),
        "xy datum is the identity placement"
    );

    let xz = datum_plane_frame(&FeatureId::from_str("origin_plane_xz").unwrap()).unwrap();
    assert_eq!(xz.normal, [0.0, 1.0, 0.0]);
    assert!(is_orthonormal(&xz));

    let yz = datum_plane_frame(&FeatureId::from_str("origin_plane_yz").unwrap()).unwrap();
    assert_eq!(yz.normal, [1.0, 0.0, 0.0]);
    assert!(is_orthonormal(&yz));

    assert!(datum_plane_frame(&FeatureId::from_u32(5)).is_none());
}

#[test]
fn face_plane_frame_is_orthonormal_with_face_normal() {
    // Weave a box and synthesise frames on each face; every frame must
    // be orthonormal and its normal must match the face's plane normal.
    let registry = FeatureRegistry::with_builtins();
    let mut doc = Document::default();
    let mut ctx = Context::new(&mut doc);
    registry
        .regenerate(
            &mut ctx,
            &[FeatureCall::new(
                "box",
                id(1),
                json!({ "half_extents": [5.0, 5.0, 5.0] }),
            )],
            None,
        )
        .unwrap();
    let woven = crate::body::weave_all(&ctx.features);
    let (brep, placement) = &woven[0];
    for i in 0..brep.face_count() {
        let frame = face_plane_frame(brep, crate::brep::FaceId(i as u32), placement);
        assert!(
            is_orthonormal(&frame),
            "face {i} frame not orthonormal: {frame:?}"
        );
    }
}

#[test]
fn sketch_on_origin_plane_xy_yields_identity_frame() {
    let registry = FeatureRegistry::with_builtins();
    let mut doc = Document::default();
    let mut ctx = Context::new(&mut doc);
    let sketch_id = id(1);
    registry
        .regenerate(
            &mut ctx,
            &[FeatureCall::new(
                SketchFeature::TYPE_NAME,
                sketch_id.clone(),
                json!({
                    "plane": "qCreatedBy(origin_plane_xy)",
                    "sub_calls": [
                        { "type": "sketch_rectangle", "id": "100",
                          "params": { "corner_a": [0.0, 0.0], "corner_b": [4.0, 3.0] } }
                    ]
                }),
            )],
            None,
        )
        .unwrap();
    let data = ctx.sketches.get(&sketch_id).unwrap();
    assert_eq!(
        data.frame,
        SketchFrame::IDENTITY,
        "xy sketch is the identity frame"
    );
    assert!(data.frame.to_transform().is_identity());
}

#[test]
fn sketch_on_prior_box_face_lands_on_that_face() {
    // A box, then a sketch whose plane is the box's +Z face. The
    // sketch frame's origin must sit on that face (z = +5) and its
    // normal must be +Z — proving sketch-on-face resolves mid-loop.
    let registry = FeatureRegistry::with_builtins();
    let mut doc = Document::default();
    let mut ctx = Context::new(&mut doc);

    let box_call = FeatureCall::new("box", id(1), json!({ "half_extents": [5.0, 5.0, 5.0] }));
    registry
        .regenerate(&mut ctx, std::slice::from_ref(&box_call), None)
        .unwrap();

    // Learn the +Z face id_hash.
    let woven = crate::body::weave_all(&ctx.features);
    let (brep, placement) = &woven[0];
    let mut top_hash = None;
    for i in 0..brep.face_count() {
        let f = face_plane_frame(brep, crate::brep::FaceId(i as u32), placement);
        if (f.normal[2] - 1.0).abs() < 1e-4 {
            top_hash = Some(
                crate::brep::naming::TopoRef::from_face(brep, crate::brep::FaceId(i as u32))
                    .id_hash,
            );
        }
    }
    let top_hash = top_hash.expect("box has a +Z face");

    // Re-run from scratch: box + sketch-on-face + extrude.
    let mut doc = Document::default();
    let mut ctx = Context::new(&mut doc);
    let sketch_id = id(2);
    let extrude_id = id(3);
    registry
        .regenerate(
            &mut ctx,
            &[
                box_call.clone(),
                FeatureCall::new(
                    SketchFeature::TYPE_NAME,
                    sketch_id.clone(),
                    json!({
                        "plane": format!("qFace({top_hash})"),
                        "sub_calls": [
                            { "type": "sketch_rectangle", "id": "200",
                              "params": { "corner_a": [-2.0, -2.0], "corner_b": [2.0, 2.0] } }
                        ]
                    }),
                ),
                FeatureCall::new(
                    "extrude",
                    extrude_id.clone(),
                    json!({
                        "profiles": format!("qCreatedBy({sketch_id})"),
                        "depth": { "value": 4.0, "unit": "mm" },
                    }),
                ),
            ],
            None,
        )
        .unwrap();

    // The sketch frame sits on the +Z face.
    let frame = ctx.sketches.get(&sketch_id).unwrap().frame;
    assert!(
        (frame.normal[2] - 1.0).abs() < 1e-3,
        "sketch normal +Z, got {:?}",
        frame.normal
    );
    assert!(
        (frame.origin[2] - 5.0).abs() < 1e-2,
        "sketch origin on the +Z face, got {:?}",
        frame.origin
    );

    // The extrude inherits a non-identity placement (the frame),
    // unlike an xy sketch — its translate lifts the profile to z≈5.
    let extrude = ctx.features.iter().find(|f| f.id == extrude_id).unwrap();
    assert!(
        (extrude.transform.translate[2] - 5.0).abs() < 1e-2,
        "extrude placement lifted to the face, got {:?}",
        extrude.transform.translate
    );
}

// ─── sketch_project (jcb7, associative edge projection) ─────────

#[test]
fn project_edge_polyline_to_frame_round_trips_on_identity() {
    // A horizontal world edge projects to its own XY coordinates on the
    // identity frame (z dropped).
    let frame = SketchFrame::IDENTITY;
    let poly = vec![[1.0, 2.0, 5.0], [4.0, 2.0, 5.0]];
    let (s, e) = project_edge_polyline_to_frame(&poly, &frame).unwrap();
    assert_eq!(s, [1.0, 2.0]);
    assert_eq!(e, [4.0, 2.0]);
}

#[test]
fn project_edge_polyline_to_frame_uses_chord_and_offset_origin() {
    // On a frame whose origin is shifted, the projection is relative to
    // the origin; a curved (multi-point) polyline degrades to its chord
    // (first→last sample).
    let frame = SketchFrame {
        origin: [10.0, 0.0, 0.0],
        x_axis: [1.0, 0.0, 0.0],
        y_axis: [0.0, 1.0, 0.0],
        normal: [0.0, 0.0, 1.0],
    };
    let poly = vec![[10.0, 0.0, 0.0], [12.0, 1.0, 0.0], [14.0, 0.0, 0.0]];
    let (s, e) = project_edge_polyline_to_frame(&poly, &frame).unwrap();
    assert_eq!(s, [0.0, 0.0]);
    assert_eq!(e, [4.0, 0.0]);
}

#[test]
fn project_edge_polyline_to_frame_rejects_degenerate() {
    assert!(project_edge_polyline_to_frame(&[], &SketchFrame::IDENTITY).is_none());
    assert!(
        project_edge_polyline_to_frame(&[[0.0, 0.0, 0.0]], &SketchFrame::IDENTITY).is_none()
    );
}

#[test]
fn sketch_project_tool_materialises_a_line_like_sketch_line() {
    // The tool reads the (already live-resolved) start/end and pushes the
    // same 2 points + 1 line a `sketch_line` would, so the TS entity-id
    // mirror stays aligned.
    let tools = SketchToolRegistry::with_builtins();
    let mut state = SketchState::with_origin();
    let call = FeatureCall::new(
        "sketch_project",
        id(1),
        json!({ "edge": "qEdge(42)", "start": [1.0, 1.0], "end": [3.0, 5.0] }),
    );
    tools.apply_all(&mut state, std::slice::from_ref(&call)).unwrap();
    let coords = path_point_coords(&state);
    // with_origin() seeds an origin point; the project adds two more.
    assert!(coords.contains(&(1.0, 1.0)), "start point present");
    assert!(coords.contains(&(3.0, 5.0)), "end point present");
    let lines = state
        .entities
        .iter()
        .filter(|e| matches!(e, fielddraft_solver::Entity::Line { .. }))
        .count();
    assert_eq!(lines, 1, "exactly one line entity");
}

#[test]
fn sketch_project_is_excluded_from_the_extrudable_profile() {
    // A lone projected edge forms no region — even though its underlying
    // line would otherwise chain. Construction + unknown-profile-type both
    // keep it out of `build_sketch2d_from_sub_calls`.
    let sub_calls: Vec<FeatureCall> = serde_json::from_value(json!([
        {"type":"sketch_project","id":"1","params":{
            "edge":"qEdge(7)","start":[0.0,0.0],"end":[10.0,0.0],"construction":true}}
    ]))
    .unwrap();
    let bounded = crate::sketch2d::BoundedSketch2d::from_sketch(
        &build_sketch2d_from_sub_calls(&sub_calls),
    );
    assert!(
        crate::sketch2d::extract_contours(&bounded, crate::sketch2d::CONTOUR_RESOLUTION)
            .is_empty(),
        "projected edge must not produce an extrudable region"
    );
}

#[test]
fn sketch_project_follows_the_body_associatively() {
    // End-to-end: box → sketch-on-+Z-face → project a top-rim edge. The
    // resolved sketch line must equal the edge's current geometry; when
    // the box grows, the projection FOLLOWS (re-resolved per regen).
    let registry = FeatureRegistry::with_builtins();

    // Learn a +Z face id_hash AND one of its bounding edge id_hashes from a
    // box centred at the origin (placement is identity, so local sample
    // coords ARE world coords). A top-rim edge has both endpoints at
    // z == +half.
    let learn = |half: f32| -> (u64, u64) {
        let mut doc = Document::default();
        let mut ctx = Context::new(&mut doc);
        let box_call = FeatureCall::new(
            "box",
            id(1),
            json!({ "half_extents": [half, half, half] }),
        );
        registry
            .regenerate(&mut ctx, std::slice::from_ref(&box_call), None)
            .unwrap();
        let woven = crate::body::weave_all(&ctx.features);
        let (brep, placement) = &woven[0];
        // +Z face.
        let mut top_face = None;
        for i in 0..brep.face_count() {
            let f = face_plane_frame(brep, crate::brep::FaceId(i as u32), placement);
            if (f.normal[2] - 1.0).abs() < 1e-4 {
                top_face = Some(crate::brep::FaceId(i as u32));
            }
        }
        let top_face = top_face.expect("box has a +Z face");
        let face_hash =
            crate::brep::naming::TopoRef::from_face(brep, top_face).id_hash;
        // An edge whose both endpoints sit on z == +half (top rim).
        let mut edge_hash = None;
        for i in 0..brep.edges.len() {
            let eid = crate::brep::EdgeId(i as u32);
            let edge = brep.edge(eid);
            let samples = crate::brep::edge_sampling::sample_edge(&edge.geometry);
            if samples.len() < 2 {
                continue;
            }
            let z0 = samples[0][2] as f32;
            let zn = samples[samples.len() - 1][2] as f32;
            if (z0 - half).abs() < 1e-3 && (zn - half).abs() < 1e-3 {
                edge_hash =
                    Some(crate::brep::naming::TopoRef::from_edge(brep, eid).id_hash);
                break;
            }
        }
        (face_hash, edge_hash.expect("a top-rim edge"))
    };

    let (face_hash, edge_hash) = learn(2.5);

    let sketch_id = id(2);
    let project_call = json!({
        "plane": format!("qFace({face_hash})"),
        "sub_calls": [
            { "type": "sketch_project", "id": "200", "params": {
                "edge": format!("qEdge({edge_hash})"),
                "start": [0.0, 0.0], "end": [0.0, 0.0],
                "construction": true } }
        ]
    });

    // Regen with the small box and read the resolved projected line.
    let resolved_line = |half: f32| -> ([f32; 2], [f32; 2]) {
        let mut doc = Document::default();
        let mut ctx = Context::new(&mut doc);
        registry
            .regenerate(
                &mut ctx,
                &[
                    FeatureCall::new(
                        "box",
                        id(1),
                        json!({ "half_extents": [half, half, half] }),
                    ),
                    FeatureCall::new(
                        SketchFeature::TYPE_NAME,
                        sketch_id.clone(),
                        project_call.clone(),
                    ),
                ],
                None,
            )
            .unwrap();
        let data = ctx.sketches.get(&sketch_id).unwrap();
        // The stored sub_call now carries the LIVE coordinates.
        let pc = &data.sub_calls[0].params;
        let read = |k: &str| {
            let a = pc.get(k).unwrap().as_array().unwrap();
            [a[0].as_f64().unwrap() as f32, a[1].as_f64().unwrap() as f32]
        };
        (read("start"), read("end"))
    };

    let (s_small, e_small) = resolved_line(2.5);
    // The cached [0,0],[0,0] was overwritten — the projection resolved to a
    // non-degenerate segment.
    let len_small = ((e_small[0] - s_small[0]).powi(2)
        + (e_small[1] - s_small[1]).powi(2))
    .sqrt();
    assert!(len_small > 1e-3, "projection resolved to a real segment");

    // Grow the box: same edge query, the projected line must MOVE.
    let (s_big, e_big) = resolved_line(5.0);
    let len_big = ((e_big[0] - s_big[0]).powi(2) + (e_big[1] - s_big[1]).powi(2)).sqrt();
    assert!(
        (len_big - 2.0 * len_small).abs() < 1e-2,
        "the projected edge followed the body (small {len_small}, big {len_big})"
    );
}

// ─── sketch_path (F22-full) ─────────────────────────────────────

#[test]
fn sketch_path_def_round_trips_through_serde() {
    // A def with a closed contour mixing line + quad segments must
    // serialise and parse back identically through the FeatureSchema
    // generic (serde) path.
    let original = SketchPathDef {
        contours: vec![PathContourDef {
            start: [0.0, 0.0],
            segments: vec![
                PathSegmentDef::Line { to: [10.0, 0.0] },
                PathSegmentDef::Quad {
                    ctrl: [10.0, 10.0],
                    to: [0.0, 10.0],
                },
                PathSegmentDef::Line { to: [0.0, 0.0] },
            ],
            closed: true,
        }],
    };
    let value = json!({
        "contours": [serde_json::to_value(&original.contours[0]).unwrap()]
    });
    let parsed = SketchPathDef::parse(&value).expect("parse");
    assert_eq!(parsed.contours, original.contours);
    // Tagged-enum wire shape: quad carries kind/ctrl/to.
    let seg_json = serde_json::to_value(PathSegmentDef::Quad {
        ctrl: [1.0, 2.0],
        to: [3.0, 4.0],
    })
    .unwrap();
    assert_eq!(seg_json["kind"], "quad");
    assert_eq!(seg_json["ctrl"], json!([1.0, 2.0]));
}

/// Collect all registered Point entity coords in entity order.
#[cfg(test)]
fn path_point_coords(state: &SketchState) -> Vec<(f64, f64)> {
    state
        .entities
        .iter()
        .filter_map(|e| match e {
            fielddraft_solver::Entity::Point { at, .. } => Some((at.x, at.y)),
            _ => None,
        })
        .collect()
}

#[test]
fn sketch_path_tool_allocates_canonical_vertex_entities() {
    // CANONICAL order: contour start, then per segment
    // (Quad → ctrl, to; Line → to). A one-contour path with
    // [line, quad, line] => start + (to) + (ctrl, to) + (to) = 5
    // point entities in that exact order & coords.
    let tools = SketchToolRegistry::with_builtins();
    let mut state = SketchState::new(); // no origin, keep coords clean
    let call = FeatureCall::new(
        "sketch_path",
        id(1),
        json!({
            "contours": [
                { "start": [0.0, 0.0], "closed": false, "segments": [
                    { "kind": "line", "to": [10.0, 0.0] },
                    { "kind": "quad", "ctrl": [15.0, 5.0], "to": [20.0, 10.0] },
                    { "kind": "line", "to": [0.0, 10.0] }
                ]}
            ]
        }),
    );
    tools
        .apply_all(&mut state, std::slice::from_ref(&call))
        .unwrap();
    let pts = path_point_coords(&state);
    assert_eq!(
        pts,
        vec![
            (0.0, 0.0),   // start
            (10.0, 0.0),  // line.to
            (15.0, 5.0),  // quad.ctrl
            (20.0, 10.0), // quad.to
            (0.0, 10.0),  // line.to
        ]
    );
}

#[test]
fn sketch_path_tool_two_contours_allocate_in_order() {
    let tools = SketchToolRegistry::with_builtins();
    let mut state = SketchState::new();
    let call = FeatureCall::new(
        "sketch_path",
        id(1),
        json!({
            "contours": [
                { "start": [0.0, 0.0], "closed": false, "segments": [
                    { "kind": "line", "to": [10.0, 0.0] }
                ]},
                { "start": [3.0, 3.0], "closed": false, "segments": [
                    { "kind": "quad", "ctrl": [5.0, 5.0], "to": [7.0, 3.0] }
                ]}
            ]
        }),
    );
    tools
        .apply_all(&mut state, std::slice::from_ref(&call))
        .unwrap();
    let pts = path_point_coords(&state);
    assert_eq!(
        pts,
        vec![
            (0.0, 0.0),  // c0 start
            (10.0, 0.0), // c0 line.to
            (3.0, 3.0),  // c1 start
            (5.0, 5.0),  // c1 quad.ctrl
            (7.0, 3.0),  // c1 quad.to
        ]
    );
}

#[test]
fn sketch_path_tool_closed_contour_adds_no_closing_entity() {
    // A closed contour's implicit closing edge returns to `start`
    // and allocates NO extra entity.
    let tools = SketchToolRegistry::with_builtins();
    let mut state = SketchState::new();
    let call = FeatureCall::new(
        "sketch_path",
        id(1),
        json!({
            "contours": [
                { "start": [0.0, 0.0], "closed": true, "segments": [
                    { "kind": "line", "to": [10.0, 0.0] },
                    { "kind": "line", "to": [10.0, 10.0] },
                    { "kind": "line", "to": [0.0, 10.0] }
                ]}
            ]
        }),
    );
    tools
        .apply_all(&mut state, std::slice::from_ref(&call))
        .unwrap();
    let pts = path_point_coords(&state);
    // start + 3 line.to = 4 (NOT 5 — closing edge adds nothing).
    assert_eq!(pts.len(), 4);
    assert_eq!(
        pts,
        vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]
    );
}

#[test]
fn build_geometry_is_unchanged_by_path_vertex_entities() {
    // The vertex point entities live ONLY in the entity world. The
    // extrude profile is lowered from the sub-call params via
    // build_sketch2d_from_sub_calls, which never reads entities, so
    // the lowered Path is identical regardless of vertex allocation.
    let calls = vec![FeatureCall::new(
        "sketch_path",
        id(1),
        json!({
            "contours": [
                { "start": [0.0, 0.0], "closed": true, "segments": [
                    { "kind": "line", "to": [40.0, 0.0] },
                    { "kind": "line", "to": [40.0, 20.0] },
                    { "kind": "quad", "ctrl": [20.0, 40.0], "to": [0.0, 20.0] }
                ]}
            ]
        }),
    )];
    let sketch = build_sketch2d_from_sub_calls(&calls);
    let Sketch2d::Prim(Sketch2dPrimitive::Path { segments, closed }) = &sketch else {
        panic!("expected a Path primitive, got {sketch:?}");
    };
    assert!(*closed);
    // Same segment make-up as before: only Line/QuadBezier, no
    // spurious vertex geometry.
    assert!(segments.iter().all(|s| matches!(
        s,
        SketchSegment::QuadBezier { .. } | SketchSegment::Line { .. }
    )));
    // Quad apex region check unchanged from the geometry test.
    assert!(sketch.sdf([20.0, 26.0]) < 0.0, "under quad bulge inside");
    assert!(sketch.sdf([20.0, 38.0]) > 0.0, "above bulge outside");
}

#[test]
fn build_sketch_path_single_closed_contour_fills() {
    // A single closed triangle contour lowers to one Path region.
    let calls = vec![FeatureCall::new(
        "sketch_path",
        id(1),
        json!({
            "contours": [
                { "start": [0.0, 0.0], "closed": true, "segments": [
                    { "kind": "line", "to": [20.0, 0.0] },
                    { "kind": "line", "to": [10.0, 20.0] }
                ]}
            ]
        }),
    )];
    let sketch = build_sketch2d_from_sub_calls(&calls);
    let Sketch2d::Prim(Sketch2dPrimitive::Path { closed, .. }) = &sketch else {
        panic!("expected a closed Path primitive, got {sketch:?}");
    };
    assert!(*closed);
    // Interior centroid-ish point inside, far point outside.
    assert!(sketch.sdf([10.0, 6.0]) < 0.0, "interior inside");
    assert!(sketch.sdf([100.0, 100.0]) > 0.0, "far point outside");
}

#[test]
fn build_sketch_path_implicitly_closes_unclosed_segment_chain() {
    // The last segment doesn't return to start; the lowering must
    // append the closing edge so the region fills.
    let d = SketchPathDef {
        contours: vec![PathContourDef {
            start: [0.0, 0.0],
            segments: vec![
                PathSegmentDef::Line { to: [10.0, 0.0] },
                PathSegmentDef::Line { to: [10.0, 10.0] },
                PathSegmentDef::Line { to: [0.0, 10.0] },
            ],
            closed: true,
        }],
    };
    let segs = path_contour_segments(&d.contours[0]).unwrap();
    // 3 explicit + 1 implicit closing line.
    assert_eq!(segs.len(), 4);
    let tree = build_sketch_path(&d).unwrap();
    assert!(tree.sdf([5.0, 5.0]) < 0.0, "square interior inside");
}

#[test]
fn build_sketch_path_donut_yields_difference_with_real_hole() {
    // Outer 0..20 square with an inner 5..15 square hole. The
    // build must subtract the hole: a point in the ring is inside,
    // a point in the hole is outside.
    let calls = vec![FeatureCall::new(
        "sketch_path",
        id(1),
        json!({
            "contours": [
                { "start": [0.0, 0.0], "closed": true, "segments": [
                    { "kind": "line", "to": [20.0, 0.0] },
                    { "kind": "line", "to": [20.0, 20.0] },
                    { "kind": "line", "to": [0.0, 20.0] }
                ]},
                { "start": [5.0, 5.0], "closed": true, "segments": [
                    { "kind": "line", "to": [15.0, 5.0] },
                    { "kind": "line", "to": [15.0, 15.0] },
                    { "kind": "line", "to": [5.0, 15.0] }
                ]}
            ]
        }),
    )];
    let sketch = build_sketch2d_from_sub_calls(&calls);
    let Sketch2d::Difference(_, _) = &sketch else {
        panic!("expected a Difference (outer − hole), got {sketch:?}");
    };
    // Point in the ring (between outer and inner): inside.
    assert!(sketch.sdf([2.0, 10.0]) < 0.0, "ring inside");
    // Point in the hole centre: outside.
    assert!(sketch.sdf([10.0, 10.0]) > 0.0, "hole outside");
    // Point far outside the outer: outside.
    assert!(sketch.sdf([100.0, 100.0]) > 0.0, "far outside");
}

#[test]
fn build_sketch_path_island_in_hole_is_solid_again() {
    // Even-odd nesting: outer (depth 0) ⊃ hole (depth 1) ⊃ island
    // (depth 2). The island must be solid again.
    let outer = |sz: f32, o: f32| {
        json!({ "start": [o, o], "closed": true, "segments": [
            { "kind": "line", "to": [o + sz, o] },
            { "kind": "line", "to": [o + sz, o + sz] },
            { "kind": "line", "to": [o, o + sz] }
        ]})
    };
    let calls = vec![FeatureCall::new(
        "sketch_path",
        id(1),
        json!({ "contours": [ outer(40.0, 0.0), outer(24.0, 8.0), outer(8.0, 16.0) ] }),
    )];
    let sketch = build_sketch2d_from_sub_calls(&calls);
    // Island centre (20,20): solid. Ring between hole & island
    // (e.g. (10,20) inside hole region): hole → outside. Outer
    // ring (2,20): inside.
    assert!(sketch.sdf([20.0, 20.0]) < 0.0, "island solid");
    assert!(sketch.sdf([11.0, 20.0]) > 0.0, "hole outside");
    assert!(sketch.sdf([2.0, 20.0]) < 0.0, "outer ring inside");
}

#[test]
fn build_sketch_path_open_lone_contour_skipped() {
    // An open contour alone forms no region.
    let calls = vec![FeatureCall::new(
        "sketch_path",
        id(1),
        json!({
            "contours": [
                { "start": [0.0, 0.0], "closed": false, "segments": [
                    { "kind": "line", "to": [10.0, 0.0] },
                    { "kind": "quad", "ctrl": [20.0, 10.0], "to": [30.0, 0.0] }
                ]}
            ]
        }),
    )];
    assert!(matches!(
        build_sketch2d_from_sub_calls(&calls),
        Sketch2d::Empty
    ));
}

#[test]
fn build_sketch_path_quad_segment_curves_outward() {
    // A closed contour whose top edge is a quad bulging up to y=30
    // must include points above the chord (y>20) in its region.
    let calls = vec![FeatureCall::new(
        "sketch_path",
        id(1),
        json!({
            "contours": [
                { "start": [0.0, 0.0], "closed": true, "segments": [
                    { "kind": "line", "to": [40.0, 0.0] },
                    { "kind": "line", "to": [40.0, 20.0] },
                    { "kind": "quad", "ctrl": [20.0, 40.0], "to": [0.0, 20.0] }
                ]}
            ]
        }),
    )];
    let sketch = build_sketch2d_from_sub_calls(&calls);
    // The quad apex reaches y≈30 at x=20; a point at (20, 26) lies
    // under the bulge → inside.
    assert!(sketch.sdf([20.0, 26.0]) < 0.0, "under quad bulge inside");
    // A point well above the bulge is outside.
    assert!(sketch.sdf([20.0, 38.0]) > 0.0, "above bulge outside");
}

/// Evaluate a cubic (degree-3) B-spline at parameter `t` via de Boor,
/// for the open (clamped-uniform) knot vector. `t` is in `[0, 1]`,
/// remapped onto the clamped knot domain. This mirrors the TS
/// `bsplinePoints` de Boor and is used by tests to validate the
/// exact lowering against direct evaluation.
pub fn bspline_eval_open(control_pts: &[[f64; 2]], degree: usize, t: f64) -> [f64; 2] {
    let n = control_pts.len();
    let interior = n - degree - 1;
    let mut knots: Vec<f64> = Vec::with_capacity(n + degree + 1);
    for _ in 0..=degree {
        knots.push(0.0);
    }
    for i in 1..=interior {
        knots.push(i as f64);
    }
    let last = (interior + 1) as f64;
    for _ in 0..=degree {
        knots.push(last);
    }
    let u = t * last;
    de_boor(control_pts, &knots, degree, u)
}

/// de Boor evaluation of a B-spline at parameter `u`.
fn de_boor(poly: &[[f64; 2]], knots: &[f64], degree: usize, u: f64) -> [f64; 2] {
    let n = poly.len();
    // Find knot span k: knots[k] <= u < knots[k+1] (clamp so the last
    // affecting control point stays in range, k ∈ degree..=n-1).
    let mut k = degree;
    while k + 1 <= n - 1 && knots[k + 1] <= u {
        k += 1;
    }
    // Local copy of the degree+1 affecting control points.
    let mut d: Vec<[f64; 2]> = (0..=degree)
        .map(|j| poly[(k - degree + j).min(n - 1)])
        .collect();
    for r in 1..=degree {
        for j in (r..=degree).rev() {
            let i = k - degree + j;
            let denom = knots[i + degree - r + 1] - knots[i];
            let alpha = if denom.abs() < 1e-12 {
                0.0
            } else {
                (u - knots[i]) / denom
            };
            d[j] = [
                (1.0 - alpha) * d[j - 1][0] + alpha * d[j][0],
                (1.0 - alpha) * d[j - 1][1] + alpha * d[j][1],
            ];
        }
    }
    d[degree]
}

// ─── stable wall-sub table (Onshape-Import Schritt 2) ──────────────────

/// Sub-calls of a unit square drawn as four connected lines. With the
/// origin at entity 0, each line call allocates (start, end, line) →
/// line entity ids 3, 6, 9, 12.
fn square_line_calls() -> Vec<FeatureCall> {
    serde_json::from_value(json!([
        {"type":"sketch_line","id":"1","params":{"start":[0.0,0.0],"end":[10.0,0.0]}},
        {"type":"sketch_line","id":"2","params":{"start":[10.0,0.0],"end":[10.0,10.0]}},
        {"type":"sketch_line","id":"3","params":{"start":[10.0,10.0],"end":[0.0,10.0]}},
        {"type":"sketch_line","id":"4","params":{"start":[0.0,10.0],"end":[0.0,0.0]}}
    ]))
    .unwrap()
}

#[test]
fn wall_sub_table_square_uses_line_entity_ids() {
    let calls = square_line_calls();
    let (tree, traced) = build_sketch2d_traced(&calls);
    let table = wall_sub_table(&tree, &traced).expect("attributable profile");
    assert_eq!(table.len(), 4, "four right-angle walls");
    let mut got = table.clone();
    got.sort_unstable();
    assert_eq!(got, vec![3, 6, 9, 12], "subs are the LINE entity ids");
}

#[test]
fn wall_sub_table_circle_uses_circle_entity_id() {
    let calls: Vec<FeatureCall> = serde_json::from_value(json!([
        {"type":"sketch_circle","id":"1","params":{"center":[0.0,0.0],"radius":5.0}}
    ]))
    .unwrap();
    let (tree, traced) = build_sketch2d_traced(&calls);
    // origin=0, center point=1, circle entity=2.
    assert_eq!(wall_sub_table(&tree, &traced), Some(vec![2]));
}

#[test]
fn wall_sub_table_rectangle_maps_group_order() {
    let calls: Vec<FeatureCall> = serde_json::from_value(json!([
        {"type":"sketch_rectangle","id":"1","params":{"corner_a":[0.0,0.0],"corner_b":[10.0,6.0]}}
    ]))
    .unwrap();
    let (tree, traced) = build_sketch2d_traced(&calls);
    // Points 1–4 (bl,br,tr,tl), lines bottom=5, right=6, top=7, left=8.
    // Rectangle group order is +x, −x, +y, −y → right, left, top, bottom.
    assert_eq!(wall_sub_table(&tree, &traced), Some(vec![6, 8, 7, 5]));
}

#[test]
fn wall_sub_table_maps_axis_stripped_rectangle_prim() {
    // #390: an axis-touching rectangle PRIM is lowered to a closed
    // Path with the on-axis wall dropped (`strip_revolve_axis_segments`
    // on a revolve profile). The traced Rectangle's per-group entity
    // ids must remap onto the three surviving Path walls.
    let calls: Vec<FeatureCall> = serde_json::from_value(json!([
        {"type":"sketch_rectangle","id":"1","params":{"corner_a":[0.0,-6.0],"corner_b":[10.0,6.0]}}
    ]))
    .unwrap();
    let (tree, traced) = build_sketch2d_traced(&calls);
    let stripped = crate::sketch::strip_revolve_axis_segments(tree);
    // Chain order after the strip: bottom, right, top (left = axis wall
    // dropped). Entity ids: bottom=5, right=6, top=7, left=8.
    assert_eq!(wall_sub_table(&stripped, &traced), Some(vec![5, 6, 7]));
}

#[test]
fn wall_sub_table_survives_earlier_edits() {
    // THE regression this whole scheme exists for: adding geometry BEFORE
    // the profile must not re-key its walls. Positional G1 groups shift
    // (the circle takes group 0); entity-derived subs move with the
    // entities. (Entity ids themselves shift only if the EARLIER call
    // changes its allocation count — the same stability contract
    // constraints and qSketchLine already live on.)
    let calls = square_line_calls();
    let (tree, traced) = build_sketch2d_traced(&calls);
    let before = {
        let mut t = wall_sub_table(&tree, &traced).unwrap();
        t.sort_unstable();
        t
    };

    // Same square preceded by a construction point (1 entity) — every
    // line entity id shifts by exactly that call's allocation, which is
    // what the shared entity-id space defines as identity; the WALL↔ID
    // binding itself must stay intact (each wall still names its line).
    let mut with_point: Vec<FeatureCall> = serde_json::from_value(json!([
        {"type":"sketch_point","id":"9","params":{"at":[99.0,99.0]}}
    ]))
    .unwrap();
    with_point.extend(square_line_calls());
    let (tree2, traced2) = build_sketch2d_traced(&with_point);
    let after = {
        let mut t = wall_sub_table(&tree2, &traced2).unwrap();
        t.sort_unstable();
        t
    };
    assert_eq!(
        after,
        before.iter().map(|s| s + 1).collect::<Vec<_>>(),
        "walls track their line entities through the id shift"
    );
}

#[test]
fn wall_sub_table_region_restriction_keeps_entity_subs() {
    // Square (lines 3,6,9,12) + separate circle (center 13, circle 14).
    let mut calls = square_line_calls();
    let extra: Vec<FeatureCall> = serde_json::from_value(json!([
        {"type":"sketch_circle","id":"5","params":{"center":[50.0,0.0],"radius":4.0}}
    ]))
    .unwrap();
    calls.extend(extra);

    let mut sketches = HashMap::new();
    sketches.insert(
        id(1),
        SketchData {
            state: SketchState::with_origin(),
            sub_calls: calls,
            frame: SketchFrame::IDENTITY,
        },
    );
    // Whole sketch: 4 square walls + 1 circle wall.
    let (_, _, traced) = resolve_profile_query("qCreatedBy(1)", &sketches).unwrap();
    let regions = {
        let (full, _, _) = resolve_profile_query("qCreatedBy(1)", &sketches).unwrap();
        crate::sketch_regions::find_regions(&full, PROFILE_REGION_RESOLUTION)
    };
    // Find the circle's region (interior x near 50).
    let circle_region = regions
        .iter()
        .find(|r| (r.interior[0] - 50.0).abs() < 10.0)
        .expect("circle region found");
    let (restricted, _, traced_r) = resolve_profile_query(
        &format!("qRegion(1, {})", circle_region.index),
        &sketches,
    )
    .unwrap();
    assert_eq!(
        wall_sub_table(&restricted, &traced_r),
        Some(vec![14]),
        "the restricted profile keeps the circle's ENTITY sub, not a renumbered positional 0"
    );
    // Sanity: the full profile really has all five walls attributed.
    let (full, _, traced_f) = resolve_profile_query("qCreatedBy(1)", &sketches).unwrap();
    assert_eq!(wall_sub_table(&full, &traced_f).map(|t| t.len()), Some(5));
    let _ = traced;
}

#[test]
fn wall_sub_table_falls_back_for_svg_paths() {
    // `sketch_path` allocates only point entities — no curve provenance,
    // so the WHOLE sketch stays positional (None), never half-mapped.
    let calls: Vec<FeatureCall> = serde_json::from_value(json!([
        {"type":"sketch_path","id":"1","params":{"contours":[
            {"start":[0.0,0.0],"segments":[
                {"kind":"line","to":[10.0,0.0]},
                {"kind":"line","to":[10.0,10.0]},
                {"kind":"line","to":[0.0,0.0]}
            ],"closed":true}
        ]}}
    ]))
    .unwrap();
    let (tree, traced) = build_sketch2d_traced(&calls);
    assert!(!matches!(tree, Sketch2d::Empty), "path lowered");
    assert_eq!(wall_sub_table(&tree, &traced), None);
}
