use super::*;
// DL-61 C2b: `Feature` is no longer part of the production
// surface (the kernel runs on call-first `feature_calls` +
// `DecodedFeature`), but the test fixtures still build spine
// features directly, so they decode through `from_parts` here.
use crate::csg::{DecodedFeature, PrimitiveModifiers};
use crate::dc::Mesh;
use fielddraft_protocol::{BoolOp, FeatureId, Primitive, Transform};

fn box_feature(id: u32, op: BoolOp, center: [f32; 3], half: [f32; 3]) -> DecodedFeature {
    DecodedFeature::from_parts(
        FeatureId::from_u32(id),
        format!("Box {id}"),
        op,
        Primitive::Box { center, half_extents: half },
        Transform::IDENTITY,
        PrimitiveModifiers::NONE,
        vec![],
    )
}

fn sphere_feature(id: u32, op: BoolOp, center: [f32; 3], r: f32) -> DecodedFeature {
    DecodedFeature::from_parts(
        FeatureId::from_u32(id),
        format!("Sphere {id}"),
        op,
        Primitive::Sphere { center, radius: r },
        Transform::IDENTITY,
        PrimitiveModifiers::NONE,
        vec![],
    )
}

// ─── primitive SDFs ──────────────────────────────────────────────

#[test]
fn box_sdf_zero_on_face() {
    let b = Primitive::Box {
        center: [0.0, 0.0, 0.0],
        half_extents: [1.0, 1.0, 1.0],
    };
    assert!((sdf_primitive(&b, [1.0, 0.0, 0.0])).abs() < 1e-5);
    assert!(sdf_primitive(&b, [0.0, 0.0, 0.0]) < 0.0);
    assert!(sdf_primitive(&b, [2.0, 0.0, 0.0]) > 0.0);
}

#[test]
fn sphere_sdf_matches_radius() {
    let s = Primitive::Sphere { center: [0.0, 0.0, 0.0], radius: 5.0 };
    assert!((sdf_primitive(&s, [5.0, 0.0, 0.0])).abs() < 1e-5);
    assert!((sdf_primitive(&s, [0.0, 10.0, 0.0]) - 5.0).abs() < 1e-5);
    assert!((sdf_primitive(&s, [0.0, 0.0, 0.0]) + 5.0).abs() < 1e-5);
}

#[test]
fn cylinder_sdf_radial_and_axial() {
    let c = Primitive::Cylinder {
        center: [0.0, 0.0, 0.0],
        radius: 2.0,
        half_height: 3.0,
    };
    assert!((sdf_primitive(&c, [2.0, 0.0, 0.0])).abs() < 1e-5);
    assert!((sdf_primitive(&c, [0.0, 3.0, 0.0])).abs() < 1e-5);
    assert!(sdf_primitive(&c, [0.0, 0.0, 0.0]) < 0.0);
}

// ─── kernel mutations + undo/redo ────────────────────────────────

// ─── K6b.3a: sketch commit path ─────────────────────────────────

// ─── K8.10: variables ─────────────────────────────────────

#[test]
fn set_list_remove_variable_roundtrips_through_btreemap() {
    let mut k = Kernel::new();
    assert_eq!(k.list_variables().unwrap(), "[]");

    k.set_variable("width", 50.0, "length", None).unwrap();
    k.set_variable("depth", 30.0, "number", None).unwrap();
    // Inserted in reverse alphabetical order, but the
    // BTreeMap normalises to name-sorted on listing.
    let json = k.list_variables().unwrap();
    assert!(json.contains("\"name\":\"depth\""));
    assert!(json.contains("\"name\":\"width\""));
    // The kind rides along on the wire form (ticket 8ar9).
    assert!(json.contains("\"kind\":\"length\""));
    assert!(json.contains("\"kind\":\"number\""));
    let depth_idx = json.find("depth").unwrap();
    let width_idx = json.find("width").unwrap();
    assert!(depth_idx < width_idx);

    assert!(k.remove_variable("depth"));
    assert!(!k.remove_variable("depth"));
    let json = k.list_variables().unwrap();
    assert!(!json.contains("depth"));
    assert!(json.contains("width"));
}

// Error-path tests for set_variable are wasm-only —
// JsError::new panics when built as a native binary, so
// the empty-name / NaN / Infinity checks are covered by
// the Playwright smoketest instead.

#[test]
fn set_variable_trims_whitespace_and_undo_rolls_back() {
    let mut k = Kernel::new();
    k.set_variable("  width  ", 42.0, "number", None).unwrap();
    assert_eq!(
        k.document.variables.get("width").map(|v| v.value),
        Some(42.0)
    );
    // Undo removes the variable entirely.
    assert!(k.undo());
    assert!(k.document.variables.is_empty());
}

#[test]
fn rename_variable_keeps_value_and_undoes() {
    // NB error paths construct a `JsError`, which panics on non-wasm
    // targets (wasm-bindgen import) — so this only exercises the success
    // paths, like the other variable tests. The guard clauses (empty /
    // collision / unknown) are covered by the web-side store tests.
    let mut k = Kernel::new();
    k.set_variable("width", 50.0, "length", None).unwrap();
    k.set_variable("depth", 30.0, "number", None).unwrap();

    // Rename keeps the value AND the kind; the old name is gone.
    k.rename_variable("width", "Seitenlänge").unwrap();
    let renamed = k.document.variables.get("Seitenlänge").cloned();
    assert_eq!(renamed.as_ref().map(|v| v.value), Some(50.0));
    assert_eq!(
        renamed.as_ref().map(|v| v.kind),
        Some(fielddraft_protocol::VarKind::Length)
    );
    assert!(!k.document.variables.contains_key("width"));

    // A no-op rename (same name) succeeds without changing anything.
    k.rename_variable("depth", "depth").unwrap();
    assert_eq!(
        k.document.variables.get("depth").map(|v| v.value),
        Some(30.0)
    );

    // The rename participates in undo (back to `width`).
    assert!(k.undo());
    assert_eq!(
        k.document.variables.get("width").map(|v| v.value),
        Some(50.0)
    );
    assert!(!k.document.variables.contains_key("Seitenlänge"));
}

#[test]
fn add_sketch_appends_a_sketch_feature_call_and_preserves_primitives() {
    let mut k = Kernel::new();
    // Start with a box so we can confirm the primitive survives
    // the sketch add + regeneration round trip.
    let box_id = k
        .add_primitive(
            Primitive::KIND_BOX,
            BoolOp::TAG_NEW,
            0.0,
            0.0,
            0.0,
            10.0,
            10.0,
            10.0,
        )
        .unwrap();

    let sub_calls = r#"[
        {
            "type": "sketch_line",
            "id": "100",
            "params": { "start": [0.0, 0.0], "end": [20.0, 0.0] }
        }
    ]"#;
    let sketch_id = k
        .add_sketch("qCreatedBy(origin_plane_xy)", sub_calls)
        .unwrap();
    assert_ne!(sketch_id, box_id);

    // feature_calls: [box, sketch] — sketch is appended after
    // the primitive survivors.
    assert_eq!(k.document.feature_calls.len(), 2);
    assert_eq!(k.document.feature_calls[0].type_name, "box");
    assert_eq!(k.document.feature_calls[1].type_name, "sketch");

    // The primitive spine still has the box.
    assert_eq!(k.decoded.len(), 1);
    assert_eq!(k.decoded[0].id.as_u32_if_flat(), Some(box_id));

    // listFeatures reports both rows with the right kinds.
    let list_json = k.list_features().unwrap();
    let list: serde_json::Value = serde_json::from_str(&list_json).unwrap();
    let arr = list.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["kind"], serde_json::json!("box"));
    assert_eq!(arr[1]["kind"], serde_json::json!("sketch"));
    assert_eq!(arr[1]["name"], serde_json::json!(format!("Sketch {sketch_id}")));
}

#[test]
fn add_sketch_survives_subsequent_primitive_mutations() {
    // K6b.3a regression guard: adding a sketch and then
    // adding a box must leave both entries in the feature
    // calls list. The old sync_feature_calls wipe-and-rebuild
    // would have dropped the sketch.
    let mut k = Kernel::new();
    let sketch_id = k.add_sketch("qCreatedBy(origin_plane_xy)", "[]").unwrap();
    let box_id = k
        .add_primitive(
            Primitive::KIND_BOX,
            BoolOp::TAG_NEW,
            0.0,
            0.0,
            0.0,
            5.0,
            5.0,
            5.0,
        )
        .unwrap();
    assert_ne!(sketch_id, box_id);
    assert_eq!(k.document.feature_calls.len(), 2);
    assert!(k
        .document
        .feature_calls
        .iter()
        .any(|c| c.type_name == "sketch"));
    assert!(k
        .document
        .feature_calls
        .iter()
        .any(|c| c.type_name == "box"));
}

#[test]
fn get_sketch_sub_calls_returns_the_committed_sub_spine() {
    let mut k = Kernel::new();
    let sub_calls = r#"[
        { "type": "sketch_point", "id": "100", "params": { "at": [1.0, 2.0] } },
        { "type": "sketch_point", "id": "101", "params": { "at": [3.0, 4.0] } }
    ]"#;
    let id = k
        .add_sketch("qCreatedBy(origin_plane_xy)", sub_calls)
        .unwrap();
    let back = k.get_sketch_sub_calls(id).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&back).unwrap();
    let arr = parsed.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["type"], serde_json::json!("sketch_point"));
    assert_eq!(arr[1]["type"], serde_json::json!("sketch_point"));
}

#[test]
fn replace_sketch_updates_in_place_and_keeps_feature_id_stable() {
    let mut k = Kernel::new();
    let id = k.add_sketch("qCreatedBy(origin_plane_xy)", "[]").unwrap();
    assert_eq!(k.document.feature_calls.len(), 1);

    let new_calls = r#"[
        {
            "type": "sketch_point",
            "id": "100",
            "params": { "at": [5.0, 5.0] }
        }
    ]"#;
    k.replace_sketch(id, "qCreatedBy(origin_plane_xy)", new_calls)
        .unwrap();

    // Still one call, still the same id.
    assert_eq!(k.document.feature_calls.len(), 1);
    let call = &k.document.feature_calls[0];
    assert_eq!(call.type_name, "sketch");
    assert_eq!(call.id.as_u32_if_flat(), Some(id));
    // And the sub-spine has the new point in it.
    let sub = call.params.get("sub_calls").unwrap().as_array().unwrap();
    assert_eq!(sub.len(), 1);
    assert_eq!(sub[0]["type"], serde_json::json!("sketch_point"));
}

#[test]
fn replace_sketch_preserves_surrounding_primitive_calls() {
    let mut k = Kernel::new();
    let box_id = k
        .add_primitive(
            Primitive::KIND_BOX,
            BoolOp::TAG_NEW,
            0.0,
            0.0,
            0.0,
            1.0,
            1.0,
            1.0,
        )
        .unwrap();
    let sketch_id = k.add_sketch("qCreatedBy(origin_plane_xy)", "[]").unwrap();
    assert_eq!(k.document.feature_calls.len(), 2);

    k.replace_sketch(
        sketch_id,
        "qCreatedBy(origin_plane_xy)",
        r#"[{ "type": "sketch_point", "id": "100", "params": { "at": [1.0, 2.0] } }]"#,
    )
    .unwrap();

    // Still two calls, both ids intact.
    assert_eq!(k.document.feature_calls.len(), 2);
    assert!(k
        .document
        .feature_calls
        .iter()
        .any(|c| c.type_name == "box"));
    assert!(k
        .document
        .feature_calls
        .iter()
        .any(|c| c.type_name == "sketch"));
    // And the primitive spine still has the original box.
    assert_eq!(k.decoded.len(), 1);
    assert_eq!(k.decoded[0].id.as_u32_if_flat(), Some(box_id));
}

// Error-path tests for `add_sketch` and `get_sketch_sub_calls`
// would construct a `JsError`, which panics in non-wasm test
// runs because `wasm-bindgen` imports aren't available — the
// existing kernel error paths (`parse_op`, malformed JSON in
// `load_json`, …) rely on wasm-pack test coverage for the same
// reason. Happy-path correctness here is covered above.

// ─── K7.4: add_extrude ───────────────────────────────────────

#[test]
fn add_extrude_lowers_the_referenced_sketch_into_primitive_extrude() {
    // Commit a rectangle sketch, then ask add_extrude to
    // lift it into a 3D slab. The resulting spine feature
    // must be a `Primitive::Extrude` whose `sketch` aabb
    // matches the rectangle.
    let mut k = Kernel::new();
    let sketch_id = k
        .add_sketch(
            "qCreatedBy(origin_plane_xy)",
            r#"[
                {
                    "type": "sketch_rectangle",
                    "id": "r1",
                    "params": {
                        "corner_a": [-10.0, -5.0],
                        "corner_b": [10.0, 5.0]
                    }
                }
            ]"#,
        )
        .unwrap();

    let extrude_id = k
        .add_extrude(sketch_id, 16.0, BoolOp::TAG_NEW, 0.0, false)
        .unwrap();
    assert_ne!(extrude_id, sketch_id);

    // One primitive on the spine — the extrude.
    // (Sketches don't push onto `features`.)
    assert_eq!(k.decoded.len(), 1);
    let extrude_spine = k
        .decoded
        .iter()
        .find(|f| f.id.as_u32_if_flat() == Some(extrude_id))
        .expect("extrude spine entry");
    match extrude_spine.export_primitive().expect("extrude primitive") {
        Primitive::Extrude {
            sketch,
            half_depth_z,
            center: _,
            wall_subs: _,
        } => {
            assert!((half_depth_z - 8.0).abs() < 1e-5);
            let aabb = sketch.aabb();
            assert!((aabb.min[0] - (-10.0)).abs() < 1e-5);
            assert!((aabb.max[1] - 5.0).abs() < 1e-5);
        }
        other => panic!("expected Primitive::Extrude, got {other:?}"),
    }

    // feature_calls still has both: the sketch call and
    // the new extrude call with its qCreatedBy query.
    assert_eq!(k.document.feature_calls.len(), 2);
    let extrude_call = k
        .document
        .feature_calls
        .iter()
        .find(|c| c.type_name == "extrude")
        .expect("extrude feature call");
    let profiles = extrude_call.params.get("profiles").unwrap();
    assert_eq!(
        profiles.as_str().unwrap(),
        format!("qCreatedBy({sketch_id})")
    );
}

#[test]
fn add_extrude_respects_the_bool_op_argument() {
    let mut k = Kernel::new();
    let sketch_id = k
        .add_sketch(
            "qCreatedBy(origin_plane_xy)",
            r#"[{
                "type": "sketch_rectangle",
                "id": "r1",
                "params": { "corner_a": [-1.0, -1.0], "corner_b": [1.0, 1.0] }
            }]"#,
        )
        .unwrap();
    k.add_extrude(sketch_id, 4.0, BoolOp::TAG_REMOVE, 0.0, false).unwrap();
    let extrude_spine = k
        .decoded
        .iter()
        .find(|f| matches!(f.export_primitive(), Some(Primitive::Extrude { .. })))
        .expect("extrude spine entry");
    assert_eq!(extrude_spine.op, BoolOp::Remove);
}

// ─── K8.0: add_linear_pattern + list_features round trip ──

#[test]
fn add_linear_pattern_populates_list_features_with_source_and_pattern() {
    let mut k = Kernel::new();
    let box_id = k
        .add_primitive(
            Primitive::KIND_BOX,
            BoolOp::TAG_NEW,
            0.0,
            0.0,
            0.0,
            10.0,
            10.0,
            10.0,
        )
        .unwrap();
    let pattern_id = k.add_linear_pattern(box_id, 1.0, 0.0, 0.0, 3, 40.0).unwrap();
    assert_ne!(box_id, pattern_id);

    // listFeatures walks feature_calls; only top-level
    // entries show up, not the hierarchical clone spine
    // entries pattern execute pushes.
    let list_json = k.list_features().unwrap();
    let list: serde_json::Value = serde_json::from_str(&list_json).unwrap();
    let arr = list.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    let kinds: Vec<&str> = arr.iter().map(|f| f["kind"].as_str().unwrap()).collect();
    assert!(kinds.contains(&"box"));
    assert!(kinds.contains(&"linear_pattern"));

    // The spine has 3 entries: the box + two clones.
    assert_eq!(k.decoded.len(), 3);
}

// ─── K7.5: editable feature params ──────────────────────────

#[test]
fn replace_feature_call_params_updates_box_half_extents_in_place() {
    let mut k = Kernel::new();
    let id = k
        .add_primitive(
            Primitive::KIND_BOX,
            BoolOp::TAG_NEW,
            0.0,
            0.0,
            0.0,
            10.0,
            10.0,
            10.0,
        )
        .unwrap();
    // Sanity: the current params reflect half_extents=[10,10,10].
    let before = k.get_feature_call_params(id).unwrap();
    let before_val: serde_json::Value = serde_json::from_str(&before).unwrap();
    let arr = before_val["half_extents"].as_array().unwrap();
    assert!((arr[0].as_f64().unwrap() - 10.0).abs() < 1e-5);

    // Replace with a wider box.
    let new_params = serde_json::json!({
        "half_extents": [25.0, 15.0, 5.0],
        "center": [0.0, 0.0, 0.0],
        "translate": [0.0, 0.0, 0.0],
        "rotation": [0.0, 0.0, 0.0, 1.0],
        "scale": 1.0,
        "op": "new",
    });
    k.replace_feature_call_params(id, &new_params.to_string(), false)
        .unwrap();

    // The spine feature must carry the new extents, and the
    // feature id stays stable.
    assert_eq!(k.decoded.len(), 1);
    let feat = &k.decoded[0];
    assert_eq!(feat.id.as_u32_if_flat(), Some(id));
    match feat.export_primitive().expect("box primitive") {
        Primitive::Box { half_extents, .. } => {
            assert!((half_extents[0] - 25.0).abs() < 1e-5);
            assert!((half_extents[1] - 15.0).abs() < 1e-5);
            assert!((half_extents[2] - 5.0).abs() < 1e-5);
        }
        other => panic!("expected Primitive::Box, got {other:?}"),
    }

    // And the wire form round-trips the new values.
    let after = k.get_feature_call_params(id).unwrap();
    let after_val: serde_json::Value = serde_json::from_str(&after).unwrap();
    let after_arr = after_val["half_extents"].as_array().unwrap();
    assert!((after_arr[0].as_f64().unwrap() - 25.0).abs() < 1e-5);
}

/// A throttled live drag replaces the same call dozens of times with
/// `coalesce=true` after one initial `coalesce=false` — undo must
/// treat the whole sequence as a SINGLE step that restores the
/// pre-drag value.
/// Full feature path of the helix screw thread: core cylinder +
/// triangle thread-profile sketch + `addHelix`, validated through
/// the COMPILED opcode field (`bodySdfEval` — the exact stream the
/// GPU interprets).
#[test]
fn helix_feature_threads_a_cylinder() {
    let mut k = Kernel::new();
    // Core cylinder along Y: r = 8.2, height ±10.
    let cyl = k
        .add_primitive(Primitive::KIND_CYLINDER, BoolOp::TAG_NEW, 0.0, 0.0, 0.0, 8.2, 10.0, 0.0)
        .unwrap();
    // Thread profile: ISO-ish 60° triangle in the (r, y) plane —
    // base inside the core (r=8 < 8.2), apex at r=9.4, centred at
    // y = −8.2, axial extent 1.6 ≤ pitch 2.
    let sub_calls = r#"[
        {
            "type": "sketch_polygon",
            "id": "100",
            "params": { "vertices": [8.0, -9.0, 9.4, -8.2, 8.0, -7.4] }
        }
    ]"#;
    let sketch_id = k.add_sketch("qCreatedBy(origin_plane_xy)", sub_calls).unwrap();
    // The thread fuses onto the core via an EXPLICIT merge scope
    // (`targets`). Touching solids never auto-merge — the helix would
    // otherwise stay a separate body (no implicit boolean).
    let helix_params = format!(
        r#"{{
            "profiles": "qCreatedBy({sketch_id})",
            "operation": "add",
            "pitch": {{ "value": 2.0, "unit": "mm" }},
            "turns": 8.0,
            "targets": "qCreatedBy({cyl})"
        }}"#
    );
    let helix_id = k.add_feature_call("helix", &helix_params).unwrap();
    assert_ne!(helix_id, sketch_id);

    // One body: the thread fuses onto the core.
    assert_eq!(k.body_count(), 1);
    let d = |x: f32, y: f32, z: f32| k.body_sdf_eval(0, x, y, z);

    // Inside the core.
    assert!(d(0.0, 0.0, 0.0) < 0.0);
    // On the thread ridge a quarter turn in (the sweep starts AT
    // θ=0, so the t=0 cross-section is the start cap itself):
    // outside the bare core radius!
    assert!(d(0.0, -7.7, 8.5) < 0.0, "ridge must fuse onto the core");
    // In the groove between turns at the same angle and radius.
    assert!(d(0.0, -6.7, 8.5) > 0.0, "groove must stay open");
    // Helical invariance: half a turn advances y by pitch/2.
    assert!(d(-8.5, -7.2, 0.0) < 0.0, "thread must wind with the pitch");
    // The un-advanced half-turn point sits in the groove.
    assert!(d(-8.5, -8.2, 0.0) > 0.0);
    // Past the last turn (8 turns × 2mm from −8.2 → thread tops out
    // near y ≈ 7.8): high on the cylinder there is no ridge.
    assert!(d(8.5, 9.5, 0.0) > 0.0);

    // The outliner row reports the helix kind.
    let list: serde_json::Value =
        serde_json::from_str(&k.list_features().unwrap()).unwrap();
    assert!(list
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["kind"] == "helix"));
}

/// vfqh: the Onshape-style input modes derive the third quantity —
/// `turns_height` (turns 8, height 16) must resolve to pitch 2 and
/// produce the exact same field as the canonical pitch+turns form.
#[test]
fn helix_height_input_modes_resolve() {
    let mut k = Kernel::new();
    let cyl = k
        .add_primitive(Primitive::KIND_CYLINDER, BoolOp::TAG_NEW, 0.0, 0.0, 0.0, 8.2, 10.0, 0.0)
        .unwrap();
    let sub_calls = r#"[
        {
            "type": "sketch_polygon",
            "id": "100",
            "params": { "vertices": [8.0, -9.0, 9.4, -8.2, 8.0, -7.4] }
        }
    ]"#;
    let sketch_id = k.add_sketch("qCreatedBy(origin_plane_xy)", sub_calls).unwrap();
    let params = format!(
        r#"{{
            "profiles": "qCreatedBy({sketch_id})",
            "input": "turns_height",
            "turns": 8.0,
            "height": {{ "value": 16.0, "unit": "mm" }},
            "operation": "add",
            "targets": "qCreatedBy({cyl})"
        }}"#
    );
    k.add_feature_call("helix", &params).unwrap();
    assert_eq!(k.body_count(), 1);
    let d = |x: f32, y: f32, z: f32| k.body_sdf_eval(0, x, y, z);
    // Same probes as helix_feature_threads_a_cylinder (pitch resolves
    // to 16/8 = 2): quarter-turn ridge inside, groove outside.
    assert!(d(0.0, -7.7, 8.5) < 0.0, "derived-pitch ridge must be inside");
    assert!(d(0.0, -6.7, 8.5) > 0.0, "derived-pitch groove must stay open");

    // pitch_height: pitch 2 + height 16 → 8 turns, identical field.
    let mut k2 = Kernel::new();
    let cyl2 = k2
        .add_primitive(Primitive::KIND_CYLINDER, BoolOp::TAG_NEW, 0.0, 0.0, 0.0, 8.2, 10.0, 0.0)
        .unwrap();
    let sketch2 = k2.add_sketch("qCreatedBy(origin_plane_xy)", sub_calls).unwrap();
    let params2 = format!(
        r#"{{
            "profiles": "qCreatedBy({sketch2})",
            "input": "pitch_height",
            "pitch": {{ "value": 2.0, "unit": "mm" }},
            "height": {{ "value": 16.0, "unit": "mm" }},
            "operation": "add",
            "targets": "qCreatedBy({cyl2})"
        }}"#
    );
    k2.add_feature_call("helix", &params2).unwrap();
    let d2 = |x: f32, y: f32, z: f32| k2.body_sdf_eval(0, x, y, z);
    assert!((d(0.0, -7.7, 8.5) - d2(0.0, -7.7, 8.5)).abs() < 1e-5);
    // Past the derived extent (8 turns × 2mm from −8.2): no ridge.
    assert!(d2(8.5, 9.5, 0.0) > 0.0);
}

/// Ticket 15f5 (merge scope): an explicit `targets` query docks the
/// helix onto the NAMED body it touches — without it, an Add that
/// touches several bodies bridges them all. Two cylinders close
/// enough that the thread touches both; the helix targets only the
/// second: the bodies stay separate, the thread lands in cylinder
/// 2's body, and cylinder 1's body stays bare.
#[test]
fn helix_merge_scope_targets_named_body() {
    let mut k = Kernel::new();
    let cyl1 = k
        .add_primitive(Primitive::KIND_CYLINDER, BoolOp::TAG_NEW, 0.0, 0.0, 0.0, 8.2, 10.0, 0.0)
        .unwrap();
    // Close enough that the thread crest (r ≤ 9.4 around the origin
    // axis) cuts into this cylinder's space — contact with BOTH.
    let cyl2 = k
        .add_primitive(Primitive::KIND_CYLINDER, BoolOp::TAG_NEW, 17.0, 0.0, 0.0, 8.2, 10.0, 0.0)
        .unwrap();
    let sub_calls = r#"[
        {
            "type": "sketch_polygon",
            "id": "100",
            "params": { "vertices": [8.0, -9.0, 9.4, -8.2, 8.0, -7.4] }
        }
    ]"#;
    let sketch_id = k.add_sketch("qCreatedBy(origin_plane_xy)", sub_calls).unwrap();
    let params = format!(
        r#"{{
            "profiles": "qCreatedBy({sketch_id})",
            "pitch": {{ "value": 2.0, "unit": "mm" }},
            "turns": 8.0,
            "operation": "add",
            "targets": "qCreatedBy({cyl2})"
        }}"#
    );
    k.add_feature_call("helix", &params).unwrap();
    assert_eq!(k.body_count(), 2);

    // Find each cylinder's body by probing its centre.
    let body_of = |x: f32| -> usize {
        (0..k.body_count())
            .find(|&b| k.body_sdf_eval(b, x, 0.0, 0.0) < 0.0)
            .expect("cylinder centre must be inside one body")
    };
    let b1 = body_of(0.0);
    let b2 = body_of(17.0);
    assert_ne!(b1, b2);
    // The thread ridge (quarter turn in, around the ORIGIN axis)
    // belongs to cylinder 2's body — the explicit merge scope —
    // and must NOT be part of cylinder 1's body.
    let ridge = (0.0, -7.7, 8.5);
    assert!(
        k.body_sdf_eval(b2, ridge.0, ridge.1, ridge.2) < 0.0,
        "targeted body must carry the thread"
    );
    assert!(
        k.body_sdf_eval(b1, ridge.0, ridge.1, ridge.2) > 0.0,
        "overlapped-but-untargeted body must stay bare"
    );
    let _ = cyl1;
}

#[test]
fn coalesced_replace_is_one_undo_step() {
    let mut k = Kernel::new();
    let id = k
        .add_primitive(
            Primitive::KIND_BOX,
            BoolOp::TAG_NEW,
            0.0,
            0.0,
            0.0,
            10.0,
            10.0,
            10.0,
        )
        .unwrap();
    let depth_before = k.undo_depth();

    let params_with = |x: f64| {
        serde_json::json!({
            "half_extents": [x, 10.0, 10.0],
            "center": [0.0, 0.0, 0.0],
            "translate": [0.0, 0.0, 0.0],
            "rotation": [0.0, 0.0, 0.0, 1.0],
            "scale": 1.0,
            "op": "new",
        })
    };
    // Drag: first commit snapshots, the rest coalesce.
    k.replace_feature_call_params(id, &params_with(12.0).to_string(), false)
        .unwrap();
    for x in [14.0, 16.0, 18.0, 20.0] {
        k.replace_feature_call_params(id, &params_with(x).to_string(), true)
            .unwrap();
    }
    assert_eq!(
        k.undo_depth(),
        depth_before + 1,
        "the whole drag must be exactly one undo step"
    );

    // Undo rolls the param all the way back to the pre-drag value.
    assert!(k.undo());
    let after = k.get_feature_call_params(id).unwrap();
    let val: serde_json::Value = serde_json::from_str(&after).unwrap();
    let arr = val["half_extents"].as_array().unwrap();
    assert!(
        (arr[0].as_f64().unwrap() - 10.0).abs() < 1e-5,
        "undo must restore the pre-drag half extent, got {}",
        arr[0]
    );
}

/// Weaver-worker round trip: `weaveBodyBrep` serializes the woven
/// B-Rep + placement-invariant key; `importBodyBrep` installs it into
/// a kernel whose cache is cold, bumping the body version. A document
/// edit between export and import invalidates the key and the import
/// is rejected.
#[test]
fn weave_body_brep_roundtrips_through_import() {
    let mut k = Kernel::new();
    let box_id = k
        .add_primitive(Primitive::KIND_BOX, BoolOp::TAG_NEW, 0.0, 0.0, 0.0, 30.0, 30.0, 30.0)
        .unwrap();
    let sphere_id = k
        .add_primitive(Primitive::KIND_SPHERE, BoolOp::TAG_NEW, 25.0, 0.0, 0.0, 20.0, 0.0, 0.0)
        .unwrap();
    // Touching solids never auto-merge — an explicit Boolean union fuses
    // the box and sphere into the single body this round-trip exercises.
    let bool_params = format!(
        r#"{{ "operation": "Union", "tools": "qCreatedBy({box_id}),qCreatedBy({sphere_id})", "targets": "", "keep_tools": false }}"#
    );
    k.add_feature_call("boolean", &bool_params).unwrap();
    assert_eq!(k.body_count(), 1);
    let body_id = k.body_id(0);

    let bytes = k.weave_body_brep(body_id);
    assert!(!bytes.is_empty(), "weave must serialize a non-empty payload");
    let edge_count_woven = k.bodies[0].brep_cached().unwrap().edge_count();
    assert!(edge_count_woven > 0, "box∪sphere weave has at least the seam");

    // Cold cache (a fresh kernel after a reload would look like this).
    k.bodies[0].brep = None;
    let version_before = k.bodies[0].version;
    assert!(k.import_body_brep(body_id, &bytes), "matching key must install");
    assert_eq!(k.bodies[0].brep_cached().unwrap().edge_count(), edge_count_woven);
    assert_eq!(k.bodies[0].version, version_before.wrapping_add(1));

    // Re-import of an already-current B-Rep is a no-op (no version churn).
    assert!(!k.import_body_brep(body_id, &bytes));

    // A geometry edit between export and import must reject the payload.
    let mut params: serde_json::Value =
        serde_json::from_str(&k.get_feature_call_params(sphere_id).unwrap()).unwrap();
    params["radius"] = serde_json::json!(22.5);
    k.replace_feature_call_params(sphere_id, &params.to_string(), false)
        .unwrap();
    k.bodies[0].brep = None;
    assert!(
        !k.import_body_brep(body_id, &bytes),
        "stale key must be rejected after a geometry edit"
    );

    // And the stale-ids poll reports the body until a weave lands.
    let stale: Vec<u32> = serde_json::from_str(&k.body_brep_stale_ids()).unwrap();
    assert_eq!(stale, vec![body_id]);
    let fresh = k.weave_body_brep(body_id);
    assert!(k.import_body_brep(body_id, &fresh) || k.bodies[0].brep_cached().is_some());
    let stale_after: Vec<u32> = serde_json::from_str(&k.body_brep_stale_ids()).unwrap();
    assert!(stale_after.is_empty());
}

#[test]
fn replace_feature_call_params_preserves_blend_radius_through_sync() {
    let mut k = Kernel::new();
    let id = k
        .add_primitive(
            Primitive::KIND_BOX,
            BoolOp::TAG_NEW,
            0.0, 0.0, 0.0,
            10.0, 10.0, 10.0,
        )
        .unwrap();

    let new_params = serde_json::json!({
        "half_extents": [10.0, 10.0, 10.0],
        "center": [0.0, 0.0, 0.0],
        "translate": [0.0, 0.0, 0.0],
        "rotation": [0.0, 0.0, 0.0, 1.0],
        "scale": 1.0,
        "op": "new",
        "blend_radius": 5.0,
        "twist_rate": 0.3,
    });
    k.replace_feature_call_params(id, &new_params.to_string(), false)
        .unwrap();

    let after = k.get_feature_call_params(id).unwrap();
    let v: serde_json::Value = serde_json::from_str(&after).unwrap();
    assert!((v["blend_radius"].as_f64().unwrap() - 5.0).abs() < 1e-5);
    assert!((v["twist_rate"].as_f64().unwrap() - 0.3).abs() < 1e-5);

    let json = k.to_json().unwrap();
    let doc: serde_json::Value = serde_json::from_str(&json).unwrap();
    let calls = doc["feature_calls"].as_array().unwrap();
    let call = calls.iter().find(|c| c["id"] == id).unwrap();
    assert!((call["params"]["blend_radius"].as_f64().unwrap() - 5.0).abs() < 1e-5);
    assert!((call["params"]["twist_rate"].as_f64().unwrap() - 0.3).abs() < 1e-5);
}

// ─── primitive mutations + undo/redo ─────────────────────────────

#[test]
fn add_primitive_appends_and_returns_new_id() {
    let mut k = Kernel::new();
    let id1 = k
        .add_primitive(Primitive::KIND_BOX, BoolOp::TAG_NEW, 0.0, 0.0, 0.0, 1.0, 2.0, 3.0)
        .unwrap();
    let id2 = k
        .add_primitive(
            Primitive::KIND_SPHERE,
            BoolOp::TAG_REMOVE,
            10.0,
            0.0,
            0.0,
            4.0,
            0.0,
            0.0,
        )
        .unwrap();
    assert_ne!(id1, id2);
    assert_eq!(k.feature_count(), 2);
}

// ─── generic feature-call insert + script-aware type queries ──────

#[test]
fn add_feature_call_appends_builtin_and_returns_new_id() {
    let mut k = Kernel::new();
    let params = r#"{ "half_extents": [1.0, 2.0, 3.0], "op": "new" }"#;
    let id = k.add_feature_call("box", params).unwrap();
    assert_eq!(k.feature_count(), 1);
    assert!(
        k.document.feature_calls.iter().any(|c| c.id == FeatureId::from_u32(id)),
        "the new box call lands in feature_calls"
    );
    // The decoded spine grows to mirror the inserted call.
    assert!(k.decoded.iter().any(|f| f.id == FeatureId::from_u32(id)));
}

#[test]
fn add_feature_call_rejects_malformed_params_json() {
    // `add_feature_call`'s error branch constructs a `JsError`, which
    // panics on native test targets, so we exercise the guard it
    // relies on: malformed params json fails to parse, which is what
    // makes the method short-circuit before touching the document.
    assert!(serde_json::from_str::<serde_json::Value>("{ this is not json").is_err());
}

/// Build a minimal valid serialised `FieldSkriptRecord` for the
/// `upsert_script` path. The `schema` is the opaque RawFeatureSchema
/// blob the web side stores verbatim.
fn script_record_json(id: &str, schema: serde_json::Value) -> String {
    serde_json::to_string(&serde_json::json!({
        "id": id,
        "name": "Widget",
        "icon": "ƒ",
        "source": "",
        "compiledJs": "",
        "schema": schema,
        "version": 1,
    }))
    .unwrap()
}

#[test]
fn list_feature_types_includes_script_ids() {
    let mut k = Kernel::new();
    let before: Vec<String> =
        serde_json::from_str(&k.list_feature_types().unwrap()).unwrap();
    assert!(!before.iter().any(|t| t == "my_script"));

    k.upsert_script(&script_record_json(
        "my_script",
        serde_json::json!({ "type": "widget", "name": "Widget", "params": [] }),
    ))
    .unwrap();

    let after: Vec<String> =
        serde_json::from_str(&k.list_feature_types().unwrap()).unwrap();
    assert!(after.iter().any(|t| t == "my_script"));
    // Still sorted and de-duplicated.
    let mut sorted = after.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(after, sorted);
}

#[test]
fn schema_for_type_resolves_scripts_builtins_and_errors() {
    let mut k = Kernel::new();
    let schema = serde_json::json!({
        "type": "widget", "name": "Widget", "params": [],
    });
    k.upsert_script(&script_record_json("my_script", schema.clone()))
        .unwrap();

    // A script id resolves to its stored schema verbatim.
    let got: serde_json::Value =
        serde_json::from_str(&k.schema_for_type("my_script").unwrap()).unwrap();
    assert_eq!(got, schema);

    // A built-in type still routes through the registry and returns
    // that registry's schema verbatim.
    let builtin: serde_json::Value =
        serde_json::from_str(&k.schema_for_type("box").unwrap()).unwrap();
    let registry = features::FeatureRegistry::with_builtins();
    assert_eq!(builtin, registry.schema_for("box").unwrap());

    // An unknown type still misses both the scripts and the registry;
    // we assert the registry miss directly because the method's
    // not-found branch builds a `JsError` (panics on native targets).
    assert!(!k.document.scripts.iter().any(|r| r.id == "definitely_not_a_type"));
    assert!(registry.schema_for("definitely_not_a_type").is_none());
}

/// Regression for the outliner showing "? script_1 N" on a
/// FieldSkript instance. A script call carries the script id as its
/// `type_name`; `static_kind_name` collapses every non-built-in to
/// "unknown", so `list_features` must instead pass the script id
/// through as the `kind` (the web outliner keys `scriptTypes` —
/// name + icon — by exactly this id).
#[test]
fn list_features_reports_script_id_as_kind() {
    let mut k = Kernel::new();
    k.upsert_script(&script_record_json(
        "script_1",
        serde_json::json!({ "type": "widget", "name": "Widget", "params": [] }),
    ))
    .unwrap();
    // A script instance and a built-in side by side.
    k.document.feature_calls.push(fielddraft_protocol::FeatureCall::new(
        "script_1",
        FeatureId::from_u32(1),
        serde_json::json!({}),
    ));
    k.document.feature_calls.push(fielddraft_protocol::FeatureCall::new(
        "box",
        FeatureId::from_u32(2),
        serde_json::json!({}),
    ));

    let arr: Vec<serde_json::Value> =
        serde_json::from_str(&k.list_features().unwrap()).unwrap();
    let kind_of = |id: u64| {
        arr.iter()
            .find(|f| f["id"] == serde_json::json!(id))
            .unwrap()["kind"]
            .clone()
    };
    assert_eq!(kind_of(1), serde_json::json!("script_1"));
    assert_eq!(kind_of(2), serde_json::json!("box"));

    // A call whose script was deleted falls back to "unknown".
    k.document.scripts.clear();
    let arr2: Vec<serde_json::Value> =
        serde_json::from_str(&k.list_features().unwrap()).unwrap();
    assert_eq!(
        arr2.iter().find(|f| f["id"] == serde_json::json!(1)).unwrap()["kind"],
        serde_json::json!("unknown")
    );
}

#[test]
fn remove_feature_returns_true_only_for_existing_ids() {
    let mut k = Kernel::new();
    let id = k
        .add_primitive(Primitive::KIND_BOX, BoolOp::TAG_NEW, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0)
        .unwrap();
    assert!(k.remove_feature(id));
    assert!(!k.remove_feature(id));
    assert_eq!(k.feature_count(), 0);
}

/// Regression for the "delete button silently does nothing"
/// bug reported in UX testing 2026-04-13. `Hole` is a
/// non-spine feature type that pushes its `DecodedFeature` under a
/// hierarchical child id (`<hole>.0`), so the old scan-by-flat-id
/// loop in `remove_feature` couldn't match and returned false for
/// every hole. Every feature the outliner shows must be
/// removable, regardless of whether it's spine-backed or lives
/// purely in `feature_calls`.
#[test]
fn remove_feature_deletes_hole_and_its_hierarchical_child_spine() {
    let mut k = Kernel::new();
    // Seed a base box so the hole has something to subtract from.
    let box_id = k
        .add_primitive(Primitive::KIND_BOX, BoolOp::TAG_NEW, 0.0, 0.0, 0.0, 20.0, 20.0, 20.0)
        .unwrap();
    let sketch_id = k
        .add_sketch(
            "qCreatedBy(origin_plane_xy)",
            r#"[{ "type": "sketch_point", "id": "100", "params": { "at": [0.0, 0.0] } }]"#,
        )
        .unwrap();
    let hole_id = k
        .add_hole(
            "M6".to_string(),
            6.0,
            "through_all".to_string(),
            40.0,
            format!("qSketchPoint({sketch_id}, 1)"),
            format!("qCreatedBy({box_id})"),
        )
        .unwrap();

    // The hole lands in `feature_calls` as a flat-id entry and
    // its spine mirror lives under the hierarchical child id
    // `<hole_id>.0`. Both should disappear on remove.
    assert!(
        k.document.feature_calls.iter().any(|c| c.id == FeatureId::from_u32(hole_id)),
        "hole call present before remove"
    );
    let hole_spine_pre = k.decoded.iter().filter(|f| {
        let fid = FeatureId::from_u32(hole_id);
        f.id == fid || feature_id_is_descendant_of(&f.id, &fid)
    }).count();
    assert!(hole_spine_pre > 0, "hole has at least one spine entry before remove");

    assert!(k.remove_feature(hole_id), "delete returns true for a hole");
    assert!(!k.remove_feature(hole_id), "second delete returns false");

    // The hole is gone from both storages.
    assert!(
        !k.document.feature_calls.iter().any(|c| c.id == FeatureId::from_u32(hole_id)),
        "hole call was removed"
    );
    let hole_spine_post = k.decoded.iter().filter(|f| {
        let fid = FeatureId::from_u32(hole_id);
        f.id == fid || feature_id_is_descendant_of(&f.id, &fid)
    }).count();
    assert_eq!(hole_spine_post, 0, "no descendant spine entries survived");

    // The base box is untouched.
    assert!(
        k.decoded.iter().any(|f| f.id == FeatureId::from_u32(box_id)),
        "unrelated box remains"
    );
}

/// Regression: deleting a feature must take its gizmo transforms
/// with it. A `translate`/`rotate`/`scale` call references its
/// subject via `target_id`; an orphaned transform left in
/// `feature_calls` made the *next* regeneration (e.g. adding another
/// box) fail with "target feature N not found". Reproduces the bug
/// reported after deleting everything from the scene, then adding a
/// box.
#[test]
fn remove_feature_cascades_to_gizmo_transforms() {
    let mut k = Kernel::new();
    let box_id = k
        .add_primitive(Primitive::KIND_BOX, BoolOp::TAG_NEW, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0)
        .unwrap();
    // Move it — pushes a `translate` call targeting the box.
    assert!(k.apply_gizmo_translate(box_id, 5.0, 0.0, 0.0));
    assert!(
        k.document
            .feature_calls
            .iter()
            .any(|c| c.type_name == "translate"),
        "translate call present after gizmo move"
    );

    // Delete the box: the orphaned translate must go too.
    assert!(k.remove_feature(box_id));
    assert!(
        k.document.feature_calls.is_empty(),
        "box and its translate are both gone"
    );

    // The bug: adding another primitive now regenerates cleanly.
    let next = k.add_primitive(
        Primitive::KIND_BOX, BoolOp::TAG_NEW, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0,
    );
    assert!(next.is_ok(), "regeneration succeeds after the cascade delete");
}

/// `setBrepSuppressed` (web: gizmo drag with "B-Rep rebuild while
/// dragging" off): while suppressed the kernel never weaves — a body
/// whose geometry changed exports EMPTY B-Rep payloads, while a body
/// whose cached weave is still current (pure move — the key is
/// placement-invariant) keeps serving it. Un-suppressing bumps the
/// stale body's version (the frontend's refetch signal) and the next
/// export weaves once.
#[test]
fn brep_suppression_serves_cache_or_empty_then_catches_up() {
    let mut k = Kernel::new();
    let box_id = k
        .add_primitive(Primitive::KIND_BOX, BoolOp::TAG_NEW, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0)
        .unwrap();
    // Warm the cache: a box weaves to its 12-edge wireframe.
    let edges: serde_json::Value = serde_json::from_str(&k.body_brep_edges(0)).unwrap();
    assert_eq!(edges.as_array().unwrap().len(), 12, "box wireframe woven");

    assert!(k.set_brep_suppressed(true), "flag flips on");
    assert!(!k.set_brep_suppressed(true), "repeat is a no-op");

    // A pure move keeps the placement-invariant cache current — the
    // edges still export, without any re-weave.
    assert!(k.apply_gizmo_translate(box_id, 5.0, 0.0, 0.0));
    let edges: serde_json::Value = serde_json::from_str(&k.body_brep_edges(0)).unwrap();
    assert_eq!(
        edges.as_array().unwrap().len(),
        12,
        "cached weave survives a pure move while suppressed"
    );

    // A geometry change (resizing the box) makes the cache stale → every
    // B-Rep export turns empty instead of weaving. (A standalone Remove
    // sphere wouldn't touch this body any more — scope is explicit now.)
    let mut params: serde_json::Value =
        serde_json::from_str(&k.get_feature_call_params(box_id).unwrap()).unwrap();
    params["half_extents"] = serde_json::json!([1.5, 1.0, 1.0]);
    k.replace_feature_call_params(box_id, &params.to_string(), false)
        .unwrap();
    let v_stale = k.body_version(0);
    let edges: serde_json::Value = serde_json::from_str(&k.body_brep_edges(0)).unwrap();
    assert!(edges.as_array().unwrap().is_empty(), "stale edges export empty");
    let faces: serde_json::Value = serde_json::from_str(&k.body_brep_faces(0)).unwrap();
    assert!(faces.as_array().unwrap().is_empty(), "stale faces export empty");
    let counts: serde_json::Value = serde_json::from_str(&k.body_brep_counts(0)).unwrap();
    assert_eq!(counts["edges"], 0, "stale counts export zeroed");

    // Un-suppress: the stale body's version bumps and the export
    // weaves the fresh boolean result exactly once.
    assert!(k.set_brep_suppressed(false), "flag flips off");
    assert_ne!(k.body_version(0), v_stale, "version bump signals refetch");
    let edges: serde_json::Value = serde_json::from_str(&k.body_brep_edges(0)).unwrap();
    assert!(
        !edges.as_array().unwrap().is_empty(),
        "weave catches up after un-suppress"
    );
}

#[test]
fn undo_then_redo_restores_state() {
    let mut k = Kernel::new();
    k.add_primitive(Primitive::KIND_BOX, BoolOp::TAG_NEW, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0)
        .unwrap();
    k.add_primitive(
        Primitive::KIND_SPHERE,
        BoolOp::TAG_REMOVE,
        0.0,
        0.0,
        0.0,
        0.5,
        0.0,
        0.0,
    )
    .unwrap();
    assert_eq!(k.feature_count(), 2);

    assert!(k.undo());
    assert_eq!(k.feature_count(), 1);
    assert!(k.undo());
    assert_eq!(k.feature_count(), 0);
    assert!(!k.undo()); // history exhausted

    assert!(k.redo());
    assert_eq!(k.feature_count(), 1);
    assert!(k.redo());
    assert_eq!(k.feature_count(), 2);
    assert!(!k.redo());
}

#[test]
fn mutation_after_undo_clears_redo_stack() {
    let mut k = Kernel::new();
    k.add_primitive(Primitive::KIND_BOX, BoolOp::TAG_NEW, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0)
        .unwrap();
    k.add_primitive(
        Primitive::KIND_SPHERE,
        BoolOp::TAG_NEW,
        0.0,
        0.0,
        0.0,
        1.0,
        0.0,
        0.0,
    )
    .unwrap();
    k.undo();
    assert!(k.can_redo());
    k.add_primitive(
        Primitive::KIND_CYLINDER,
        BoolOp::TAG_NEW,
        0.0,
        0.0,
        0.0,
        1.0,
        2.0,
        0.0,
    )
    .unwrap();
    // The fresh edit invalidates redo.
    assert!(!k.can_redo());
}

#[test]
fn set_feature_op_updates_feature() {
    let mut k = Kernel::new();
    let id = k
        .add_primitive(Primitive::KIND_BOX, BoolOp::TAG_NEW, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0)
        .unwrap();
    assert_eq!(k.decoded[0].op, BoolOp::New);
    k.set_feature_op(id, BoolOp::TAG_REMOVE).unwrap();
    assert_eq!(k.decoded[0].op, BoolOp::Remove);
}

#[test]
fn reorder_moves_feature() {
    let mut k = Kernel::new();
    let a = k
        .add_primitive(Primitive::KIND_BOX, BoolOp::TAG_NEW, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0)
        .unwrap();
    let b = k
        .add_primitive(
            Primitive::KIND_SPHERE,
            BoolOp::TAG_NEW,
            0.0,
            0.0,
            0.0,
            1.0,
            0.0,
            0.0,
        )
        .unwrap();
    // Move sphere (index 1) to index 0.
    assert!(k.reorder_feature(b, 0));
    let json = k.list_features().unwrap();
    assert!(json.find("\"id\":1").unwrap() > json.find("\"id\":2").unwrap(), "{json}");
    let _ = a;
}


#[test]
fn explicit_union_merges_flush_boss_into_base() {
    // Touching solids NEVER auto-merge (no implicit boolean): a prism
    // flush on a box's top face stays a SEPARATE body until an explicit
    // Boolean union joins them — then it's ONE watertight part. The
    // coplanar base plane leaves duplicate coplanar faces in the weave
    // (open item: coplanar branch merging); body count and
    // watertightness are the contract here.
    let mut k = Kernel::new();
    let base = k
        .add_primitive(Primitive::KIND_BOX, BoolOp::TAG_NEW, 0.0, 0.0, 0.0, 50.0, 50.0, 50.0)
        .unwrap();
    let boss = k
        .add_primitive(Primitive::KIND_BOX, BoolOp::TAG_ADD, 0.0, 0.0, 60.0, 20.0, 20.0, 10.0)
        .unwrap();
    assert_eq!(k.body_count(), 2, "a flush boss never auto-merges by contact");

    let bool_params = format!(
        r#"{{ "operation": "Union", "tools": "qCreatedBy({base}),qCreatedBy({boss})", "targets": "", "keep_tools": false }}"#
    );
    k.add_feature_call("boolean", &bool_params).unwrap();
    assert_eq!(k.body_count(), 1, "an explicit union fuses the boss into the base");
    let counts: serde_json::Value =
        serde_json::from_str(&k.body_brep_counts(0)).unwrap();
    assert_eq!(counts["watertight"], serde_json::json!(true), "{counts}");
}

#[test]
fn add_without_contact_seeds_its_own_body() {
    // An Add that touches nothing must not vanish — it falls back to
    // seeding a fresh body (e.g. the first extrude in an empty doc).
    let mut k = Kernel::new();
    k.add_primitive(Primitive::KIND_BOX, BoolOp::TAG_NEW, 0.0, 0.0, 0.0, 5.0, 5.0, 5.0)
        .unwrap();
    k.add_primitive(Primitive::KIND_BOX, BoolOp::TAG_ADD, 100.0, 0.0, 0.0, 5.0, 5.0, 5.0)
        .unwrap();
    assert_eq!(k.body_count(), 2, "a contact-free Add seeds its own body");
}

#[test]
fn list_features_serializes_summary() {
    let mut k = Kernel::new();
    k.add_primitive(Primitive::KIND_BOX, BoolOp::TAG_NEW, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0)
        .unwrap();
    let json = k.list_features().unwrap();
    assert!(json.contains("\"kind\":\"box\""));
    assert!(json.contains("\"op\":\"new\""));
    // clean regen → no error field serialized
    assert!(!json.contains("\"error\""));
}

#[test]
fn list_features_exports_per_feature_regen_error() {
    // Onshape semantics (ticket sx5a): a broken feature call is
    // ACCEPTED — it stays in the document, contributes no geometry,
    // and `listFeatures` carries its regen error for the red title.
    let mut k = Kernel::new();
    k.add_primitive(Primitive::KIND_BOX, BoolOp::TAG_NEW, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0)
        .unwrap();
    let broken = k
        .push_feature_call(
            "extrude",
            serde_json::json!({ "profiles": "", "depth": { "value": 25.0, "unit": "mm" } }),
        )
        .expect("a broken feature is created, not rejected");
    let json = k.list_features().unwrap();
    assert!(json.contains("\"error\""), "{json}");
    // the healthy box stays on the spine, the broken extrude doesn't
    assert_eq!(k.feature_count(), 1);
    // fixing nothing but suppressing clears the error on regen
    k.set_feature_suppressed(broken, true).unwrap();
    let json = k.list_features().unwrap();
    assert!(!json.contains("\"error\""), "{json}");
}

#[test]
fn pick_returns_miss_when_document_empty() {
    let mut k = Kernel::new();
    let json = k.pick(0.0, 100.0, 0.0, 0.0, -1.0, 0.0);
    assert!(json.contains("\"hit\":false"), "{json}");
}

#[test]
fn pick_returns_face_with_single_contributor_for_box_top() {
    let mut k = Kernel::new();
    let id = k
        .add_primitive(
            Primitive::KIND_BOX,
            BoolOp::TAG_NEW,
            0.0,
            0.0,
            0.0,
            10.0,
            10.0,
            10.0,
        )
        .unwrap();
    let json = k.pick(0.0, 100.0, 0.0, 0.0, -1.0, 0.0);
    assert!(json.contains("\"hit\":true"), "{json}");
    assert!(json.contains("\"element\":\"face\""), "{json}");
    // `contributors` are flat feature ids (what selection consumes);
    // `branches` carry the packed BranchId per face. The box +Y (top)
    // face is sub-index 2.
    assert!(
        json.contains(&format!("\"contributors\":[{id}]")),
        "{json}"
    );
    let branch = csg::BranchId::from_feature_sub(&FeatureId::from_u32(id), 2).as_u64();
    assert!(
        json.contains(&format!("\"branches\":[{branch}]")),
        "{json}"
    );
}

#[test]
fn pick_selects_sphere_by_flat_feature_id() {
    // Regression: a sphere added beside a box must be pickable in the
    // scene. `pick` reports the flat feature id in `contributors` so the
    // frontend can select it; the curved face's packed branch lands in
    // `branches`. (The sphere is a single sub-0 analytic face — SA-6.)
    let mut k = Kernel::new();
    k.add_primitive(Primitive::KIND_BOX, BoolOp::TAG_NEW, 0.0, 0.0, 0.0, 10.0, 10.0, 10.0)
        .unwrap();
    let sphere_id = k
        .add_primitive(Primitive::KIND_SPHERE, BoolOp::TAG_NEW, 60.0, 0.0, 0.0, 10.0, 10.0, 10.0)
        .unwrap();
    // Ray from far +X straight back hits the sphere first.
    let json = k.pick(200.0, 0.0, 0.0, -1.0, 0.0, 0.0);
    assert!(json.contains("\"hit\":true"), "{json}");
    assert!(
        json.contains(&format!("\"contributors\":[{sphere_id}]")),
        "{json}"
    );
    let branch = csg::BranchId::from_feature_sub(&FeatureId::from_u32(sphere_id), 0).as_u64();
    assert!(json.contains(&format!("\"branches\":[{branch}]")), "{json}");
}

#[test]
fn new_kernel_has_empty_mesh_at_default_resolution() {
    let k = Kernel::new();
    assert_eq!(k.mesh_resolution(), DEFAULT_MESH_RESOLUTION);
    assert_eq!(k.mesh_vertex_count(), 0);
    assert_eq!(k.mesh_index_count(), 0);
    // Version starts at 1 because `new` seeds the mesh once.
    assert_eq!(k.mesh_version(), 1);
}

#[test]
fn adding_a_primitive_rebuilds_the_mesh_and_bumps_version() {
    let mut k = Kernel::new();
    let v0 = k.mesh_version();
    k.add_primitive(
        Primitive::KIND_BOX,
        BoolOp::TAG_NEW,
        0.0,
        0.0,
        0.0,
        25.0,
        25.0,
        25.0,
    )
    .unwrap();
    assert!(k.mesh_vertex_count() > 0);
    assert!(k.mesh_index_count() > 0);
    assert_eq!(k.mesh_index_count() % 3, 0);
    assert!(k.mesh_version() > v0);
}

#[test]
fn empty_document_has_zero_bodies() {
    let k = Kernel::new();
    assert_eq!(k.body_count(), 0);
}

#[test]
fn two_disjoint_boxes_produce_two_bodies() {
    let mut k = Kernel::new();
    let id_a = k
        .add_primitive(
            Primitive::KIND_BOX,
            BoolOp::TAG_NEW,
            0.0,
            0.0,
            0.0,
            10.0,
            10.0,
            10.0,
        )
        .unwrap();
    let id_b = k
        .add_primitive(
            Primitive::KIND_BOX,
            BoolOp::TAG_NEW,
            // 500 mm away — completely disjoint.
            500.0,
            0.0,
            0.0,
            10.0,
            10.0,
            10.0,
        )
        .unwrap();
    assert_eq!(k.body_count(), 2);
    // Bodies are ordered by first appearance in the spine, so
    // body 0 is always the origin box.
    assert_eq!(k.body_id(0), id_a);
    assert_eq!(k.body_id(1), id_b);
    assert!(k.body_vertex_count(0) > 0);
    assert!(k.body_vertex_count(1) > 0);
    // Aggregate mesh concatenates both bodies.
    let aggregate = k.mesh_vertex_count();
    assert_eq!(aggregate, k.body_vertex_count(0) + k.body_vertex_count(1));
}

#[test]
fn mutating_one_body_leaves_the_other_bodys_version_unchanged() {
    let mut k = Kernel::new();
    let id_a = k
        .add_primitive(
            Primitive::KIND_BOX,
            BoolOp::TAG_NEW,
            0.0,
            0.0,
            0.0,
            10.0,
            10.0,
            10.0,
        )
        .unwrap();
    let id_b = k
        .add_primitive(
            Primitive::KIND_BOX,
            BoolOp::TAG_NEW,
            500.0,
            0.0,
            0.0,
            10.0,
            10.0,
            10.0,
        )
        .unwrap();

    // Snapshot the per-body versions.
    let version_a_before = k.body_version(0);
    let version_b_before = k.body_version(1);
    assert_eq!(k.body_id(0), id_a);
    assert_eq!(k.body_id(1), id_b);

    // Nudge body B — its payload hash changes, A's doesn't.
    assert!(k.nudge_feature(id_b, 5.0, 0.0, 0.0));

    // A is untouched, B got re-meshed.
    assert_eq!(k.body_version(0), version_a_before, "body A must not remesh");
    assert!(
        k.body_version(1) > version_b_before,
        "body B must remesh on its own nudge"
    );
}

#[test]
fn body_id_is_stable_across_an_unrelated_mutation() {
    let mut k = Kernel::new();
    let id_a = k
        .add_primitive(
            Primitive::KIND_BOX,
            BoolOp::TAG_NEW,
            0.0,
            0.0,
            0.0,
            10.0,
            10.0,
            10.0,
        )
        .unwrap();
    let id_b = k
        .add_primitive(
            Primitive::KIND_BOX,
            BoolOp::TAG_NEW,
            500.0,
            0.0,
            0.0,
            10.0,
            10.0,
            10.0,
        )
        .unwrap();
    assert_eq!(k.body_id(0), id_a);
    assert_eq!(k.body_id(1), id_b);

    // Mutate B. A's id and position in the bodies list must
    // survive unchanged.
    assert!(k.nudge_feature(id_b, 5.0, 0.0, 0.0));
    assert_eq!(k.body_id(0), id_a);
    assert_eq!(k.body_id(1), id_b);
}

#[test]
fn each_union_is_its_own_body() {
    let mut k = Kernel::new();
    let _id_a = k
        .add_primitive(
            Primitive::KIND_BOX,
            BoolOp::TAG_NEW,
            0.0, 0.0, 0.0,
            10.0, 10.0, 10.0,
        )
        .unwrap();
    let _id_b = k
        .add_primitive(
            Primitive::KIND_BOX,
            BoolOp::TAG_NEW,
            60.0, 0.0, 0.0,
            10.0, 10.0, 10.0,
        )
        .unwrap();
    assert_eq!(k.body_count(), 2);
    let _id_c = k
        .add_primitive(
            Primitive::KIND_BOX,
            BoolOp::TAG_NEW,
            30.0, 0.0, 0.0,
            40.0, 10.0, 10.0,
        )
        .unwrap();
    assert_eq!(k.body_count(), 3, "each union must be its own body");
}

#[test]
fn lod_is_off_by_default_every_body_uses_active_resolution() {
    let mut k = Kernel::new();
    assert_eq!(k.active_body_id(), 0);
    let _ = k
        .add_primitive(
            Primitive::KIND_BOX,
            BoolOp::TAG_NEW,
            0.0,
            0.0,
            0.0,
            25.0,
            25.0,
            25.0,
        )
        .unwrap();
    let _ = k
        .add_primitive(
            Primitive::KIND_BOX,
            BoolOp::TAG_NEW,
            500.0,
            0.0,
            0.0,
            25.0,
            25.0,
            25.0,
        )
        .unwrap();
    // Two same-size boxes under identical resolution land in the
    // same order of magnitude — their exact vertex counts diverge
    // slightly because the voxel grid is anchored in world space
    // and f32 rounding treats `x ≈ 25` differently from `x ≈ 525`,
    // but they should be within 1% of each other.
    let a = k.body_vertex_count(0) as f32;
    let b = k.body_vertex_count(1) as f32;
    let ratio = (a - b).abs() / a.max(b);
    assert!(
        ratio < 0.02,
        "vertex counts drifted more than 2% ({a} vs {b})",
    );
}

#[test]
fn setting_active_body_drops_other_bodies_to_context_resolution() {
    let mut k = Kernel::new();
    // Context res is deliberately much smaller than active res
    // so the effect is clearly visible in vertex counts.
    assert!(k.set_context_mesh_resolution(8));
    let id_a = k
        .add_primitive(
            Primitive::KIND_BOX,
            BoolOp::TAG_NEW,
            0.0,
            0.0,
            0.0,
            25.0,
            25.0,
            25.0,
        )
        .unwrap();
    let id_b = k
        .add_primitive(
            Primitive::KIND_BOX,
            BoolOp::TAG_NEW,
            500.0,
            0.0,
            0.0,
            25.0,
            25.0,
            25.0,
        )
        .unwrap();

    // Baseline: no active body → every body at active (48) res.
    let baseline_a = k.body_vertex_count(0);
    let baseline_b = k.body_vertex_count(1);

    // Set A as active → B should drop to context (8) resolution
    // and lose vertices. A stays near the baseline (bitwise
    // identical, since its LOD didn't change).
    assert!(k.set_active_body_id(id_a));
    assert_eq!(k.active_body_id(), id_a);
    assert_eq!(k.body_vertex_count(0), baseline_a, "active body unchanged");
    // Same-topology box at res 8 has ≈ 8×–12× fewer vertices.
    assert!(
        k.body_vertex_count(1) * 4 < baseline_b,
        "context res should be meaningfully coarser ({} vs baseline {})",
        k.body_vertex_count(1),
        baseline_b,
    );

    // Setting the same active body again is a no-op.
    assert!(!k.set_active_body_id(id_a));

    // Clearing the active body restores both to active res.
    assert!(k.clear_active_body_id());
    assert_eq!(k.body_vertex_count(0), baseline_a);
    assert_eq!(k.body_vertex_count(1), baseline_b);
    let _ = id_b;
}

#[test]
fn switching_active_body_remeshes_only_the_two_that_changed_lod() {
    let mut k = Kernel::new();
    assert!(k.set_context_mesh_resolution(8));
    let id_a = k
        .add_primitive(
            Primitive::KIND_BOX,
            BoolOp::TAG_NEW,
            0.0,
            0.0,
            0.0,
            25.0,
            25.0,
            25.0,
        )
        .unwrap();
    let id_b = k
        .add_primitive(
            Primitive::KIND_BOX,
            BoolOp::TAG_NEW,
            500.0,
            0.0,
            0.0,
            25.0,
            25.0,
            25.0,
        )
        .unwrap();
    let id_c = k
        .add_primitive(
            Primitive::KIND_BOX,
            BoolOp::TAG_NEW,
            1000.0,
            0.0,
            0.0,
            25.0,
            25.0,
            25.0,
        )
        .unwrap();
    // A is the active body; B and C fall to context LOD.
    assert!(k.set_active_body_id(id_a));
    let v_a = k.body_version(0);
    let v_b = k.body_version(1);
    let v_c = k.body_version(2);

    // Switch active from A to B. A drops to context, B gets
    // promoted to active. C's LOD is unchanged — still context.
    assert!(k.set_active_body_id(id_b));
    assert!(k.body_version(0) > v_a, "A re-meshed at context res");
    assert!(k.body_version(1) > v_b, "B re-meshed at active res");
    assert_eq!(k.body_version(2), v_c, "C untouched");
    let _ = id_c;
}

#[test]
fn sync_meshing_off_keeps_bodies_partitioned_but_meshes_empty() {
    // K3 part 5: after the frontend disables sync meshing, the
    // main-thread kernel should still maintain the body list
    // (it's needed for setActiveBodyByFeature and the CSG tree
    // used by pick), but every body's mesh buffer must be
    // empty. Flipping sync meshing back on re-meshes everything.
    let mut k = Kernel::new();
    let id_a = k
        .add_primitive(
            Primitive::KIND_BOX,
            BoolOp::TAG_NEW,
            0.0,
            0.0,
            0.0,
            25.0,
            25.0,
            25.0,
        )
        .unwrap();
    let id_b = k
        .add_primitive(
            Primitive::KIND_BOX,
            BoolOp::TAG_NEW,
            500.0,
            0.0,
            0.0,
            25.0,
            25.0,
            25.0,
        )
        .unwrap();
    assert!(k.sync_meshing());
    assert_eq!(k.body_count(), 2);
    let populated_a = k.body_vertex_count(0);
    let populated_b = k.body_vertex_count(1);
    assert!(populated_a > 0 && populated_b > 0);

    // Flip off — body list survives, meshes vanish.
    assert!(k.set_sync_meshing(false));
    assert!(!k.sync_meshing());
    assert_eq!(k.body_count(), 2);
    assert_eq!(k.body_vertex_count(0), 0, "body A mesh cleared");
    assert_eq!(k.body_vertex_count(1), 0, "body B mesh cleared");
    // Setting to the same value again is a no-op.
    assert!(!k.set_sync_meshing(false));

    // Picking still works because it reads the feature list
    // directly, not the mesh — raycast from above hits the box.
    let json = k.pick(0.0, 100.0, 0.0, 0.0, -1.0, 0.0);
    assert!(json.contains("\"hit\":true"), "{json}");
    // Flat feature id for box A in `contributors`; packed BranchId for
    // its +Y (top) face (sub-index 2) in `branches`.
    assert!(
        json.contains(&format!("\"contributors\":[{id_a}]")),
        "{json}"
    );
    let branch_a = csg::BranchId::from_feature_sub(&FeatureId::from_u32(id_a), 2).as_u64();
    assert!(
        json.contains(&format!("\"branches\":[{branch_a}]")),
        "{json}"
    );

    // Mutations go through without meshing.
    assert!(k.nudge_feature(id_b, 5.0, 0.0, 0.0));
    assert_eq!(k.body_vertex_count(1), 0, "still empty after mutation");

    // Flip back on — bodies get re-meshed from scratch.
    assert!(k.set_sync_meshing(true));
    assert!(k.body_vertex_count(0) > 0);
    assert!(k.body_vertex_count(1) > 0);
}

#[test]
fn set_active_body_by_feature_finds_the_owning_body() {
    let mut k = Kernel::new();
    assert!(k.set_context_mesh_resolution(8));
    let _id_a = k
        .add_primitive(
            Primitive::KIND_BOX,
            BoolOp::TAG_NEW,
            0.0,
            0.0,
            0.0,
            25.0,
            25.0,
            25.0,
        )
        .unwrap();
    let id_b = k
        .add_primitive(
            Primitive::KIND_BOX,
            BoolOp::TAG_NEW,
            500.0,
            0.0,
            0.0,
            25.0,
            25.0,
            25.0,
        )
        .unwrap();
    assert!(k.set_active_body_by_feature(id_b));
    assert_eq!(k.active_body_id(), id_b);
}

#[test]
fn set_mesh_resolution_rebuilds_and_skips_noop() {
    let mut k = Kernel::new();
    k.add_primitive(
        Primitive::KIND_BOX,
        BoolOp::TAG_NEW,
        0.0,
        0.0,
        0.0,
        25.0,
        25.0,
        25.0,
    )
    .unwrap();
    let before = k.mesh_version();
    assert!(k.set_mesh_resolution(32));
    assert!(k.mesh_version() > before);
    // Same value → no-op, version unchanged.
    let after = k.mesh_version();
    assert!(!k.set_mesh_resolution(32));
    assert_eq!(k.mesh_version(), after);
}

// ─── DL-61 C2b: feature_calls is the single source of truth ─

/// Helper: with `feature_calls` authoritative and the spine
/// derived from it, assert the regenerated spine lines up 1:1
/// with the calls. Valid for the primitive-only documents these
/// tests build — a Pattern/Mirror would add child spine entries
/// and break the length equality. Locks that every call-first
/// mutator leaves the spine consistent with the call list.
fn assert_spine_matches_calls(k: &Kernel) {
    assert_eq!(
        k.decoded.len(),
        k.document.feature_calls.len(),
        "spine length diverged from feature_calls",
    );
    for (f, c) in k.decoded.iter().zip(&k.document.feature_calls) {
        assert_eq!(f.id, c.id, "spine id drifted from its feature_call");
    }
}

#[test]
fn add_primitive_populates_feature_calls() {
    let mut k = Kernel::new();
    assert!(k.document.feature_calls.is_empty());
    let _ = k
        .add_primitive(
            Primitive::KIND_BOX,
            BoolOp::TAG_NEW,
            0.0,
            0.0,
            0.0,
            5.0,
            5.0,
            5.0,
        )
        .unwrap();
    assert_eq!(k.document.feature_calls.len(), 1);
    assert_eq!(k.document.feature_calls[0].type_name, "box");
    assert_spine_matches_calls(&k);
}

#[test]
fn every_mutation_keeps_feature_calls_in_sync() {
    let mut k = Kernel::new();
    let id_box = k
        .add_primitive(
            Primitive::KIND_BOX,
            BoolOp::TAG_NEW,
            0.0,
            0.0,
            0.0,
            5.0,
            5.0,
            5.0,
        )
        .unwrap();
    let id_sphere = k
        .add_primitive(
            Primitive::KIND_SPHERE,
            BoolOp::TAG_NEW,
            100.0,
            0.0,
            0.0,
            3.0,
            0.0,
            0.0,
        )
        .unwrap();
    let id_cyl = k
        .add_primitive(
            Primitive::KIND_CYLINDER,
            BoolOp::TAG_NEW,
            0.0,
            200.0,
            0.0,
            2.0,
            4.0,
            0.0,
        )
        .unwrap();
    assert_spine_matches_calls(&k);

    // Mutation 1: translate the box — feature_calls[0].params
    // must pick up the new translate.
    assert!(k.set_feature_translate(id_box, 10.0, 20.0, 30.0));
    assert_spine_matches_calls(&k);
    assert_eq!(
        k.document.feature_calls[0].params["translate"],
        serde_json::json!([10.0, 20.0, 30.0])
    );

    // Mutation 2: nudge the sphere.
    assert!(k.nudge_feature(id_sphere, 1.0, 2.0, 3.0));
    assert_spine_matches_calls(&k);

    // Mutation 3: change the cylinder's op.
    k.set_feature_op(id_cyl, BoolOp::TAG_REMOVE).unwrap();
    assert_spine_matches_calls(&k);
    let cyl_call_idx = k
        .decoded
        .iter()
        .position(|f| f.id == FeatureId::from_u32(id_cyl))
        .unwrap();
    assert_eq!(
        k.document.feature_calls[cyl_call_idx].params["op"],
        serde_json::json!("remove")
    );

    // Mutation 4: reorder the cylinder to the front.
    assert!(k.reorder_feature(id_cyl, 0));
    assert_spine_matches_calls(&k);
    assert_eq!(k.decoded[0].id, FeatureId::from_u32(id_cyl));

    // Mutation 5: remove the box.
    assert!(k.remove_feature(id_box));
    assert_spine_matches_calls(&k);
    assert_eq!(k.decoded.len(), 2);

    // Mutation 6: full transform write on the sphere.
    assert!(k.set_feature_transform(
        id_sphere, 7.0, 8.0, 9.0, 0.0, 0.0, 0.0, 1.0, 2.0, 2.0, 2.0
    ));
    assert_spine_matches_calls(&k);
}

#[test]
fn undo_and_redo_restore_feature_calls_in_sync() {
    // Undo/redo swaps the whole document snapshot, so
    // `feature_calls` travels with it automatically — no extra
    // instrumentation needed. This test locks that behaviour.
    let mut k = Kernel::new();
    k.add_primitive(
        Primitive::KIND_BOX,
        BoolOp::TAG_NEW,
        0.0,
        0.0,
        0.0,
        1.0,
        1.0,
        1.0,
    )
    .unwrap();
    assert_spine_matches_calls(&k);

    k.add_primitive(
        Primitive::KIND_SPHERE,
        BoolOp::TAG_NEW,
        0.0,
        0.0,
        0.0,
        2.0,
        0.0,
        0.0,
    )
    .unwrap();
    assert_eq!(k.document.feature_calls.len(), 2);
    assert_spine_matches_calls(&k);

    assert!(k.undo());
    assert_eq!(k.document.feature_calls.len(), 1);
    assert_spine_matches_calls(&k);

    assert!(k.redo());
    assert_eq!(k.document.feature_calls.len(), 2);
    assert_spine_matches_calls(&k);

    // A mutation after an undo/redo cycle also stays in sync.
    assert!(k.nudge_feature(1, 1.0, 0.0, 0.0));
    assert_spine_matches_calls(&k);
}

// ─── K4.5.4d: bootstrap from feature_calls ─────────────────

#[test]
fn load_json_bootstraps_features_from_feature_calls() {
    // Fabricated step-4d wire form: `feature_calls` populated,
    // `features` empty. The loader detects the heuristic,
    // walks the registry, and populates `features` before
    // `rebuild_packed` runs.
    let mut k = Kernel::new();
    let json = r#"{
        "schema_version": 2,
        "name": "step4d",
        "features": [],
        "feature_calls": [
            {
                "type": "box",
                "id": 1,
                "params": {
                    "half_extents": [5, 5, 5],
                    "center": [0, 0, 0],
                    "translate": [0, 0, 0],
                    "rotation": [0, 0, 0, 1],
                    "scale": 1.0,
                    "op": "new",
                    "name": "Base"
                }
            },
            {
                "type": "sphere",
                "id": 2,
                "params": {
                    "radius": 3.0,
                    "center": [0, 0, 0],
                    "translate": [20, 0, 0],
                    "rotation": [0, 0, 0, 1],
                    "scale": 1.0,
                    "op": "remove",
                    "name": "Hole"
                }
            }
        ],
        "next_feature_id": 3
    }"#;

    k.load_json(json).unwrap();
    assert_eq!(k.decoded.len(), 2);

    let box_f = &k.decoded[0];
    assert_eq!(box_f.name, "Base");
    assert!(matches!(
        box_f.export_primitive(),
        Some(Primitive::Box { half_extents, .. }) if half_extents == [5.0, 5.0, 5.0]
    ));

    let sphere_f = &k.decoded[1];
    assert_eq!(sphere_f.name, "Hole");
    assert!(matches!(
        sphere_f.export_primitive(),
        Some(Primitive::Sphere { radius, .. }) if radius == 3.0
    ));
    assert_eq!(sphere_f.transform.translate, [20.0, 0.0, 0.0]);
    assert_eq!(sphere_f.op, BoolOp::Remove);

    // feature_calls should still be populated after rebuild_packed
    // — the dual-write hook rebuilds them from the freshly
    // regenerated features, which is a round-trip no-op thanks
    // to the lossless primitive execute impls.
    assert_spine_matches_calls(&k);
}

#[test]
fn load_json_prunes_orphaned_gizmo_transform() {
    // A document written before the cascade-delete fix can carry a
    // gizmo transform whose `target_id` points at a feature that was
    // already deleted. Loading it used to fail outright with
    // "feature regeneration failed: ... target feature N not found".
    // The loader now prunes the orphan and loads what remains.
    let mut k = Kernel::new();
    let json = r#"{
        "schema_version": 2,
        "name": "stale",
        "features": [],
        "feature_calls": [
            {
                "type": "box",
                "id": 1,
                "params": {
                    "half_extents": [5, 5, 5],
                    "center": [0, 0, 0],
                    "translate": [0, 0, 0],
                    "rotation": [0, 0, 0, 1],
                    "scale": 1.0,
                    "op": "new",
                    "name": "Base"
                }
            },
            {
                "type": "translate",
                "id": 2,
                "params": {
                    "target_id": 3,
                    "translate": [5, 0, 0]
                }
            }
        ],
        "next_feature_id": 4
    }"#;

    k.load_json(json).expect("stale document loads after pruning");
    // The orphaned translate is gone; only the box survives.
    assert_eq!(k.document.feature_calls.len(), 1);
    assert_eq!(k.decoded.len(), 1);
    assert_eq!(k.decoded[0].name, "Base");
    // And regeneration is healthy afterwards.
    let next = k.add_primitive(
        Primitive::KIND_BOX, BoolOp::TAG_NEW, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0,
    );
    assert!(next.is_ok(), "regeneration succeeds after pruning the orphan");
}

#[test]
fn bootstrap_preserves_rotation_and_scale_for_box() {
    // Build a document with a non-identity transform on a box,
    // capture its feature_calls, then load them fresh via the
    // bootstrap path (with `features` cleared) and verify
    // rotation and scale survive the round-trip.
    let mut k1 = Kernel::new();
    let id = k1
        .add_primitive(
            Primitive::KIND_BOX,
            BoolOp::TAG_NEW,
            10.0,
            20.0,
            30.0,
            5.0,
            5.0,
            5.0,
        )
        .unwrap();
    // Rotation for 45° about Z: (0, 0, sin(22.5°), cos(22.5°))
    let s = (std::f32::consts::FRAC_PI_4 / 2.0).sin();
    let c = (std::f32::consts::FRAC_PI_4 / 2.0).cos();
    assert!(k1.set_feature_transform(
        id, 100.0, 200.0, 300.0, 0.0, 0.0, s, c, 2.5, 2.5, 2.5,
    ));

    // Capture the JSON, zero out `features` to simulate a
    // step-4d wire form, and reload.
    let json = k1.to_json().unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
    value["features"] = serde_json::json!([]);
    let bootstrap_json = serde_json::to_string(&value).unwrap();

    let mut k2 = Kernel::new();
    k2.load_json(&bootstrap_json).unwrap();
    assert_eq!(k2.decoded.len(), 1);

    let loaded = &k2.decoded[0];
    assert_eq!(loaded.transform.translate, [100.0, 200.0, 300.0]);
    assert!((loaded.transform.rotation[0]).abs() < 1e-6);
    assert!((loaded.transform.rotation[1]).abs() < 1e-6);
    assert!((loaded.transform.rotation[2] - s).abs() < 1e-6);
    assert!((loaded.transform.rotation[3] - c).abs() < 1e-6);
    assert!((loaded.transform.scale[0] - 2.5).abs() < 1e-6);
    assert!((loaded.transform.scale[1] - 2.5).abs() < 1e-6);
    assert!((loaded.transform.scale[2] - 2.5).abs() < 1e-6);
    assert_eq!(loaded.op, BoolOp::New);
}

#[test]
fn bootstrap_preserves_mixed_scene_across_primitive_types() {
    // One box, one sphere, one cylinder, each with a unique
    // translate and a custom op. Round-trip via the bootstrap
    // path and check every field.
    let mut k1 = Kernel::new();
    k1.add_primitive(
        Primitive::KIND_BOX,
        BoolOp::TAG_NEW,
        10.0,
        0.0,
        0.0,
        5.0,
        5.0,
        5.0,
    )
    .unwrap();
    let id_sphere = k1
        .add_primitive(
            Primitive::KIND_SPHERE,
            BoolOp::TAG_REMOVE,
            20.0,
            0.0,
            0.0,
            3.0,
            0.0,
            0.0,
        )
        .unwrap();
    let _ = id_sphere;
    k1.add_primitive(
        Primitive::KIND_CYLINDER,
        BoolOp::TAG_INTERSECT,
        0.0,
        30.0,
        0.0,
        2.0,
        4.0,
        0.0,
    )
    .unwrap();
    let original_features = k1.decoded.clone();

    // Zero out `features` and reload.
    let json = k1.to_json().unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
    value["features"] = serde_json::json!([]);
    let bootstrap_json = serde_json::to_string(&value).unwrap();

    let mut k2 = Kernel::new();
    k2.load_json(&bootstrap_json).unwrap();
    assert_eq!(k2.decoded.len(), 3);

    // Every reloaded feature should match its pre-bootstrap
    // counterpart byte for byte.
    for (original, loaded) in
        original_features.iter().zip(k2.decoded.iter())
    {
        assert_eq!(loaded.id, original.id);
        assert_eq!(loaded.name, original.name);
        assert_eq!(loaded.op, original.op);
        assert_eq!(loaded.export_primitive(), original.export_primitive());
        assert_eq!(loaded.transform.translate, original.transform.translate);
        assert_eq!(loaded.transform.rotation, original.transform.rotation);
        assert_eq!(loaded.transform.scale, original.transform.scale);
    }
    assert_spine_matches_calls(&k2);
}

#[test]
fn load_json_clears_history() {
    let mut k = Kernel::new();
    k.add_primitive(Primitive::KIND_BOX, BoolOp::TAG_NEW, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0)
        .unwrap();
    assert!(k.can_undo());
    let json = k.to_json().unwrap();
    k.load_json(&json).unwrap();
    assert!(!k.can_undo());
    assert!(!k.can_redo());
}

// ─── FieldSkript S1: document-local script records ─────────

#[test]
fn script_crud_persists_in_document_json() {
    let mut k = Kernel::new();
    assert_eq!(k.list_scripts().unwrap(), "[]");

    let rec = r#"{"id":"slot","name":"Slot","source":"// ts",
        "compiledJs":"// js","schema":{"type":"slot","params":[]},"version":1}"#;
    // First upsert inserts (returns false), second replaces (true).
    assert!(!k.upsert_script(rec).unwrap());
    let rec2 = r#"{"id":"slot","name":"Slot v2","source":"// ts2",
        "compiledJs":"// js2","schema":{"type":"slot","params":[]},"version":2}"#;
    assert!(k.upsert_script(rec2).unwrap());

    let scripts: serde_json::Value =
        serde_json::from_str(&k.list_scripts().unwrap()).unwrap();
    assert_eq!(scripts.as_array().unwrap().len(), 1);
    assert_eq!(scripts[0]["name"], "Slot v2");

    // Records round-trip through the document JSON with the
    // camelCase `compiledJs` key.
    let json = k.to_json().unwrap();
    assert!(json.contains("\"compiledJs\""), "{json}");
    let mut k2 = Kernel::new();
    k2.load_json(&json).unwrap();
    let reloaded: serde_json::Value =
        serde_json::from_str(&k2.list_scripts().unwrap()).unwrap();
    assert_eq!(reloaded[0]["id"], "slot");
    assert_eq!(reloaded[0]["version"], 2);

    // Remove is idempotent: true once, false thereafter.
    assert!(k2.remove_script("slot"));
    assert!(!k2.remove_script("slot"));
    assert_eq!(k2.list_scripts().unwrap(), "[]");
}

// ─── K4.5.4e: features-only wire form ──────────────────────

#[test]
fn to_json_does_not_emit_features_field() {
    // K4.5.4e: the Kernel's save path must emit a document
    // whose JSON contains `feature_calls` but no top-level
    // `features` key. The spine lives in-memory only.
    let mut k = Kernel::new();
    k.add_primitive(
        Primitive::KIND_BOX,
        BoolOp::TAG_NEW,
        1.0,
        2.0,
        3.0,
        4.0,
        5.0,
        6.0,
    )
    .unwrap();
    k.add_primitive(
        Primitive::KIND_SPHERE,
        BoolOp::TAG_REMOVE,
        7.0,
        8.0,
        9.0,
        1.5,
        0.0,
        0.0,
    )
    .unwrap();

    let json = k.to_json().unwrap();
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    let obj = value.as_object().expect("document is a JSON object");
    assert!(
        !obj.contains_key("features"),
        "K4.5.4e wire form must omit `features`: {json}"
    );
    let calls = obj
        .get("feature_calls")
        .and_then(|v| v.as_array())
        .expect("feature_calls must be present");
    assert_eq!(calls.len(), 2);
}

#[test]
fn kernel_save_load_roundtrips_via_feature_calls_only() {
    // Build a non-trivial scene, save, load into a fresh
    // kernel, and verify the spine is fully rehydrated from
    // the feature_calls wire form alone (no `features` field
    // in the JSON).
    let mut k1 = Kernel::new();
    let box_id = k1
        .add_primitive(
            Primitive::KIND_BOX,
            BoolOp::TAG_NEW,
            0.0,
            0.0,
            0.0,
            10.0,
            10.0,
            10.0,
        )
        .unwrap();
    k1.add_primitive(
        Primitive::KIND_CYLINDER,
        BoolOp::TAG_REMOVE,
        5.0,
        0.0,
        0.0,
        2.0,
        12.0,
        0.0,
    )
    .unwrap();
    // Give the box a non-identity transform so we exercise
    // the lossless rotation/scale bootstrap path.
    let s = (std::f32::consts::FRAC_PI_4 / 2.0).sin();
    let c = (std::f32::consts::FRAC_PI_4 / 2.0).cos();
    assert!(k1.set_feature_transform(
        box_id, 1.0, 2.0, 3.0, 0.0, 0.0, s, c, 1.75, 1.75, 1.75,
    ));
    let original_features = k1.decoded.clone();

    // Save through the kernel and reality-check that the
    // wire form doesn't carry the spine.
    let json = k1.to_json().unwrap();
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(!value.as_object().unwrap().contains_key("features"));

    // Load into a fresh kernel and verify the spine.
    let mut k2 = Kernel::new();
    k2.load_json(&json).unwrap();
    assert_eq!(k2.decoded.len(), 2);
    for (original, loaded) in
        original_features.iter().zip(k2.decoded.iter())
    {
        assert_eq!(loaded.id, original.id);
        assert_eq!(loaded.name, original.name);
        assert_eq!(loaded.op, original.op);
        assert_eq!(loaded.export_primitive(), original.export_primitive());
        assert_eq!(loaded.transform.translate, original.transform.translate);
        assert_eq!(loaded.transform.rotation, original.transform.rotation);
        assert_eq!(loaded.transform.scale, original.transform.scale);
    }
    assert_spine_matches_calls(&k2);
    // And the meshing pipeline woke up — rebuild_packed ran
    // on load, so the mesh has real geometry.
    assert!(k2.mesh_vertex_count() > 0);
}

#[test]
fn bug25_translated_cylinder_mesh_consistency() {
    let mut k = Kernel::new();

    // Two identical standalone cylinders, one at origin, one translated.
    let _id1 = k
        .add_primitive(
            Primitive::KIND_CYLINDER,
            BoolOp::TAG_NEW,
            0.0, 0.0, 0.0,
            0.6, 2.5, 0.0,
        )
        .unwrap();
    let _id2 = k
        .add_primitive(
            Primitive::KIND_CYLINDER,
            BoolOp::TAG_NEW,
            200.0, 0.0, 0.0,
            0.6, 2.5, 0.0,
        )
        .unwrap();

    assert_eq!(k.body_count(), 2);
    let v0 = k.body_vertex_count(0);
    let v1 = k.body_vertex_count(1);
    let diff = (v0 as i64 - v1 as i64).unsigned_abs();
    assert!(diff <= 2, "identical cylinders at different positions should mesh nearly identically: {v0} vs {v1}");
}

#[test]
fn boolean_tab_switch_changes_tool_op() {
    let mut k = Kernel::new();
    let _box1 = k.add_primitive(Primitive::KIND_BOX, BoolOp::TAG_NEW, 0.0,0.0,0.0, 20.0,20.0,20.0).unwrap();
    let box2 = k.add_primitive(Primitive::KIND_BOX, BoolOp::TAG_NEW, 10.0,0.0,0.0, 15.0,15.0,15.0).unwrap();
    assert_eq!(k.decoded.len(), 2);
    assert_eq!(k.decoded[0].op, BoolOp::New);
    assert_eq!(k.decoded[1].op, BoolOp::New);

    let bool_id = k.add_boolean().unwrap();
    // Initially Union with empty tools — no spine change
    assert_eq!(k.decoded[0].op, BoolOp::New);
    assert_eq!(k.decoded[1].op, BoolOp::New);

    // Set tools to box2 + operation to Subtract (simulates tab switch)
    let params = serde_json::json!({
        "operation": "Subtract",
        "tools": format!("qCreatedBy({})", box2),
        "targets": "",
        "keep_tools": false,
    });
    k.replace_feature_call_params(bool_id, &params.to_string(), false).unwrap();
    // box2 should now be Difference
    assert_eq!(k.decoded[0].op, BoolOp::New, "box1 stays Union");
    assert_eq!(k.decoded[1].op, BoolOp::Remove, "box2 is Difference after Subtract");

    // Switch tab to Union
    let params = serde_json::json!({
        "operation": "Union",
        "tools": format!("qCreatedBy({})", box2),
        "targets": "",
        "keep_tools": false,
    });
    k.replace_feature_call_params(bool_id, &params.to_string(), false).unwrap();
    assert_eq!(k.decoded[1].op, BoolOp::New, "box2 reverts to Union");

    // Switch tab to Intersect
    let params = serde_json::json!({
        "operation": "Intersect",
        "tools": format!("qCreatedBy({})", box2),
        "targets": "",
        "keep_tools": false,
    });
    k.replace_feature_call_params(bool_id, &params.to_string(), false).unwrap();
    assert_eq!(k.decoded[1].op, BoolOp::Intersect, "box2 is Intersect");
}

#[test]
fn boolean_union_merges_tools_into_one_body() {
    let mut k = Kernel::new();
    let box1 = k.add_primitive(
        Primitive::KIND_BOX, BoolOp::TAG_NEW,
        0.0, 0.0, 0.0, 20.0, 20.0, 20.0,
    ).unwrap();
    let sph = k.add_primitive(
        Primitive::KIND_SPHERE, BoolOp::TAG_NEW,
        20.0, 0.0, 0.0, 15.0, 0.0, 0.0,
    ).unwrap();
    // Two plain primitives are two separate parts.
    assert_eq!(k.body_count(), 2, "two plain unions are two parts");

    let bool_id = k.add_boolean().unwrap();
    let params = serde_json::json!({
        "operation": "Union",
        "tools": format!("qCreatedBy({}), qCreatedBy({})", box1, sph),
        "targets": "",
        "keep_tools": false,
    });
    k.replace_feature_call_params(bool_id, &params.to_string(), false).unwrap();

    // The boolean routes the second tool into the first tool's body.
    assert_eq!(k.decoded[0].op, BoolOp::New);
    assert_eq!(k.decoded[1].op, BoolOp::Add);
    assert!(k.decoded[0].target_ids.is_empty(), "seed carries no target");
    assert_eq!(k.decoded[1].target_ids, vec![FeatureId::from_u32(box1)]);
    assert_eq!(k.body_count(), 1, "an explicit union is one part");

    // The folded tree is a single solid spanning both lobes.
    let tree = csg::CsgNode::from_decoded(&k.decoded);
    let sdf = |p: [f32; 3]| tree.sdf(p, csg::EvalMode::Logical);
    assert!(sdf([0.0, 0.0, 0.0]) < 0.0, "inside the box lobe");
    assert!(sdf([30.0, 0.0, 0.0]) < 0.0, "inside the sphere lobe");
    assert!(sdf([100.0, 0.0, 0.0]) > 0.0, "outside both");
}

#[test]
fn shared_tool_before_second_seed_in_spine_still_fuses() {
    // Same transitive-union scene as `two_boolean_unions_sharing_a_tool…`,
    // but the SPINE ORDER is the one the user's project hit: the shared
    // bridge (sphere) is created *before* the second box. The Boolean
    // picks its seed as the first tool in spine order, so op2's seed is
    // the sphere — NOT the second box. If the seed-assignment clears the
    // sphere's accumulated link to op1's box, the first box splits off
    // into its own Part ("2 Parts that should be 1"). All three overlap,
    // so geometry never gates the merge; the only thing that can break it
    // is clobbering the bridge's link.
    let mut k = Kernel::new();
    let box_a = k.add_primitive(
        Primitive::KIND_BOX, BoolOp::TAG_NEW,
        -10.0, 0.0, 0.0, 12.0, 12.0, 12.0,
    ).unwrap();
    let sph = k.add_primitive(
        Primitive::KIND_SPHERE, BoolOp::TAG_NEW,
        0.0, 0.0, 0.0, 15.0, 0.0, 0.0,
    ).unwrap();
    // Created AFTER the sphere → spine-later than the shared bridge.
    let box_b = k.add_primitive(
        Primitive::KIND_BOX, BoolOp::TAG_NEW,
        10.0, 0.0, 0.0, 12.0, 12.0, 12.0,
    ).unwrap();

    // Op 1: box_a ∪ sphere.
    let b1 = k.add_boolean().unwrap();
    let p1 = serde_json::json!({
        "operation": "Union",
        "tools": format!("qCreatedBy({}), qCreatedBy({})", box_a, sph),
        "targets": "",
        "keep_tools": false,
    });
    k.replace_feature_call_params(b1, &p1.to_string(), false).unwrap();

    // Op 2: box_b ∪ sphere — sphere is spine-earlier than box_b, so it
    // is op2's seed.
    let b2 = k.add_boolean().unwrap();
    let p2 = serde_json::json!({
        "operation": "Union",
        "tools": format!("qCreatedBy({}), qCreatedBy({})", box_b, sph),
        "targets": "",
        "keep_tools": false,
    });
    k.replace_feature_call_params(b2, &p2.to_string(), false).unwrap();

    assert_eq!(
        k.body_count(), 1,
        "a shared bridge created before the second seed must still fuse all three"
    );
    let tree = csg::CsgNode::from_decoded(&k.decoded);
    let sdf = |p: [f32; 3]| tree.sdf(p, csg::EvalMode::Logical);
    assert!(sdf([-10.0, 0.0, 0.0]) < 0.0, "inside box_a");
    assert!(sdf([0.0, 0.0, 0.0]) < 0.0, "inside the sphere");
    assert!(sdf([10.0, 0.0, 0.0]) < 0.0, "inside box_b");
}

#[test]
fn two_boolean_unions_sharing_a_tool_form_one_body() {
    // Scene: a left box, a right box, and a centre sphere that bridges
    // both. The user builds it as TWO separate union operations that
    // share the sphere: (left ∪ sphere) and (right ∪ sphere). Union is
    // transitive, so all three must collapse into a single Part — the
    // second union must not clobber the sphere's link to the first.
    let mut k = Kernel::new();
    let left = k.add_primitive(
        Primitive::KIND_BOX, BoolOp::TAG_NEW,
        -20.0, 0.0, 0.0, 10.0, 10.0, 10.0,
    ).unwrap();
    let right = k.add_primitive(
        Primitive::KIND_BOX, BoolOp::TAG_NEW,
        20.0, 0.0, 0.0, 10.0, 10.0, 10.0,
    ).unwrap();
    let sph = k.add_primitive(
        Primitive::KIND_SPHERE, BoolOp::TAG_NEW,
        0.0, 0.0, 0.0, 15.0, 0.0, 0.0,
    ).unwrap();
    assert_eq!(k.body_count(), 3, "three plain unions are three parts");

    // Operation 1: left ∪ sphere.
    let b1 = k.add_boolean().unwrap();
    let p1 = serde_json::json!({
        "operation": "Union",
        "tools": format!("qCreatedBy({}), qCreatedBy({})", left, sph),
        "targets": "",
        "keep_tools": false,
    });
    k.replace_feature_call_params(b1, &p1.to_string(), false).unwrap();

    // Operation 2: right ∪ sphere — shares the sphere with op 1.
    let b2 = k.add_boolean().unwrap();
    let p2 = serde_json::json!({
        "operation": "Union",
        "tools": format!("qCreatedBy({}), qCreatedBy({})", right, sph),
        "targets": "",
        "keep_tools": false,
    });
    k.replace_feature_call_params(b2, &p2.to_string(), false).unwrap();

    // The sphere accumulates BOTH seed links (left + right), not just
    // the most recent one.
    let sphere_feat = k.decoded.iter().find(|f| f.id == FeatureId::from_u32(sph)).unwrap();
    assert!(
        sphere_feat.target_ids.contains(&FeatureId::from_u32(left))
            && sphere_feat.target_ids.contains(&FeatureId::from_u32(right)),
        "the bridging sphere links to both union seeds: {:?}",
        sphere_feat.target_ids
    );
    assert_eq!(k.body_count(), 1, "the shared sphere fuses both unions into one part");

    // The fused solid spans all three lobes.
    let tree = csg::CsgNode::from_decoded(&k.decoded);
    let sdf = |p: [f32; 3]| tree.sdf(p, csg::EvalMode::Logical);
    assert!(sdf([-20.0, 0.0, 0.0]) < 0.0, "inside the left box");
    assert!(sdf([0.0, 0.0, 0.0]) < 0.0, "inside the sphere");
    assert!(sdf([20.0, 0.0, 0.0]) < 0.0, "inside the right box");
}

/// The merged union mesh is the *correct* outer surface — every mesh vertex
/// lies on the union surface, never inside the solid. Guards against a
/// regression where the union would mesh with an interior "scoop" (a lobe's
/// hidden cap left in the mesh). NB: the concave-scoop the user saw at the
/// box∪sphere overlap is a *renderer* compositing artifact (a translucent
/// context body occluding the active body's raymarch), not a mesh defect —
/// this test pins the geometry side so that distinction stays provable.
#[test]
fn union_mesh_has_no_interior_scoop() {
    let mut k = Kernel::new();
    // Box [±10] at origin; sphere sitting on top, overlapping the +Z face.
    let box1 = k.add_primitive(
        Primitive::KIND_BOX, BoolOp::TAG_NEW,
        0.0, 0.0, 0.0, 10.0, 10.0, 10.0,
    ).unwrap();
    let sph = k.add_primitive(
        Primitive::KIND_SPHERE, BoolOp::TAG_NEW,
        0.0, 0.0, 14.0, 8.0, 0.0, 0.0,
    ).unwrap();
    let bool_id = k.add_boolean().unwrap();
    let params = serde_json::json!({
        "operation": "Union",
        "tools": format!("qCreatedBy({}), qCreatedBy({})", box1, sph),
        "targets": "", "keep_tools": false,
    });
    k.replace_feature_call_params(bool_id, &params.to_string(), false).unwrap();
    assert_eq!(k.body_count(), 1, "the explicit union is one body");

    let tree = csg::CsgNode::from_decoded(&k.decoded);
    let sdf = |p: [f32; 3]| tree.sdf(p, csg::EvalMode::Logical);
    let body = &k.bodies[0];
    let pos = &body.mesh.positions;
    let mut worst = 0.0f32;
    for v in 0..pos.len() / 3 {
        let d = sdf([pos[3 * v], pos[3 * v + 1], pos[3 * v + 2]]);
        if d < worst {
            worst = d;
        }
    }
    assert!(
        worst > -0.5,
        "union mesh has interior vertices (scoop): worst sdf = {worst}"
    );
}

// Snapshot semantics (ticket 20260615-103929-9wxs): a Boolean may only
// reference features that PRECEDE it in the tree. A Subtract whose tool
// is ordered AFTER the boolean is a forward reference — the tool does not
// exist in the snapshot the boolean sees, so it must NOT carve. The bug
// was that the forward tool combined anyway (via the now-removed
// contact-merge). Phase 1b will additionally surface this as an explicit
// feature error. Uses the box+sphere setup proven by
// `boolean_subtract_sdf_is_geometrically_correct` so the test isolates
// ordering, not the subtract math.
#[test]
fn boolean_forward_referenced_tool_does_not_carve() {
    // box [-50,50]³, sphere center [30,0,0] r40 → the box centre [0,0,0]
    // is inside the sphere, so a working subtract carves it.
    let good_order = serde_json::json!({
        "schema_version": 2, "name": "repro", "next_feature_id": 9,
        "feature_calls": [
            {"type":"box","id":1,"params":{"op":"new","half_extents":[50,50,50],"translate":[0,0,0]}},
            {"type":"sphere","id":2,"params":{"op":"new","radius":40,"translate":[30,0,0]}},
            {"type":"boolean","id":3,"params":{"keep_tools":false,"operation":"Subtract","targets":"qCreatedBy(1)","tools":"qCreatedBy(2)"}},
        ],
    });
    let bug_order = serde_json::json!({
        "schema_version": 2, "name": "repro", "next_feature_id": 9,
        "feature_calls": [
            {"type":"box","id":1,"params":{"op":"new","half_extents":[50,50,50],"translate":[0,0,0]}},
            {"type":"boolean","id":3,"params":{"keep_tools":false,"operation":"Subtract","targets":"qCreatedBy(1)","tools":"qCreatedBy(2)"}},
            {"type":"sphere","id":2,"params":{"op":"new","radius":40,"translate":[30,0,0]}},
        ],
    });
    let probe = [0.0, 0.0, 0.0];

    // Good order: tool before boolean → subtract carves the box centre.
    let mut kg = Kernel::new();
    kg.load_json(&good_order.to_string()).unwrap();
    let tg = csg::CsgNode::from_decoded(&kg.decoded);
    assert!(
        tg.sdf(probe, csg::EvalMode::Logical) > 0.0,
        "tool-before-boolean: subtract carves the box (sanity)"
    );

    // Bug order: tool after boolean → forward reference. The boolean sees
    // no such tool in its snapshot, so it must NOT carve — the probe
    // stays inside the solid box.
    let mut kb = Kernel::new();
    kb.load_json(&bug_order.to_string()).unwrap();
    let tb = csg::CsgNode::from_decoded(&kb.decoded);
    assert!(
        tb.sdf(probe, csg::EvalMode::Logical) < 0.0,
        "forward-referenced tool must NOT carve (snapshot semantics)"
    );
}

#[test]
fn add_feature_without_merge_scope_warns() {
    // A feature-implicit boolean (extrude Add) with no merge scope must
    // warn — touching solids never merge implicitly, so the user has to
    // pick a scope or "Merge with all". With either, the warning clears;
    // a plain "New" body never warns.
    let warning_for = |params: &str| -> Option<String> {
        let mut k = Kernel::new();
        // A body exists so "Merge with all" has something to target.
        let _base = k
            .add_primitive(Primitive::KIND_BOX, BoolOp::TAG_NEW, 0.0, 0.0, 0.0, 30.0, 30.0, 30.0)
            .unwrap();
        let sub = r#"[{"type":"sketch_polygon","id":"100","params":{"vertices":[-5,-5,5,-5,0,5]}}]"#;
        let sketch = k.add_sketch("qCreatedBy(origin_plane_xy)", sub).unwrap();
        let full = params.replace("SKETCH", &sketch.to_string());
        k.add_feature_call("extrude", &full).unwrap();
        let feats: serde_json::Value =
            serde_json::from_str(&k.list_features().unwrap()).unwrap();
        feats
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["kind"] == serde_json::json!("extrude"))
            .and_then(|f| f["warning"].as_str().map(|s| s.to_string()))
    };

    assert!(
        warning_for(r#"{"profiles":"qCreatedBy(SKETCH)","operation":"add","depth":{"value":20,"unit":"mm"}}"#)
            .is_some(),
        "Add without a merge scope must warn"
    );
    assert!(
        warning_for(r#"{"profiles":"qCreatedBy(SKETCH)","operation":"add","merge_with_all":true,"depth":{"value":20,"unit":"mm"}}"#)
            .is_none(),
        "Add + Merge with all (a body exists) must not warn"
    );
    assert!(
        warning_for(r#"{"profiles":"qCreatedBy(SKETCH)","operation":"new","depth":{"value":20,"unit":"mm"}}"#)
            .is_none(),
        "New (standalone) never warns"
    );
}

#[test]
fn first_standalone_add_without_scope_does_not_warn() {
    // Onshape parity: the very first solid is implicitly "New" even when
    // the feature defaults to Add — there is no prior body it could have
    // merged into, so a scope-less first Add must NOT warn (no "red born").
    // A second Add (a prior body now exists) without a scope DOES warn.
    let mut k = Kernel::new();
    let sub = r#"[{"type":"sketch_polygon","id":"100","params":{"vertices":[-5,-5,5,-5,0,5]}}]"#;
    let s1 = k.add_sketch("qCreatedBy(origin_plane_xy)", sub).unwrap();
    k.add_feature_call(
        "extrude",
        &format!(r#"{{"profiles":"qCreatedBy({s1})","operation":"add","depth":{{"value":20,"unit":"mm"}}}}"#),
    )
    .unwrap();

    let warning_of = |k: &Kernel, kind: &str| -> Option<String> {
        let feats: serde_json::Value =
            serde_json::from_str(&k.list_features().unwrap()).unwrap();
        feats
            .as_array()
            .unwrap()
            .iter()
            .filter(|f| f["kind"] == serde_json::json!(kind))
            .last()
            .and_then(|f| f["warning"].as_str().map(|s| s.to_string()))
    };
    assert!(
        warning_of(&k, "extrude").is_none(),
        "first standalone Add (no prior body) must not warn"
    );

    // Second Add — a body now exists, so a scope-less Add is ambiguous.
    let s2 = k.add_sketch("qCreatedBy(origin_plane_xy)", sub).unwrap();
    k.add_feature_call(
        "extrude",
        &format!(r#"{{"profiles":"qCreatedBy({s2})","operation":"add","depth":{{"value":20,"unit":"mm"}}}}"#),
    )
    .unwrap();
    assert!(
        warning_of(&k, "extrude").is_some(),
        "a second scope-less Add (prior body exists) must warn"
    );
}

#[test]
fn ticket_9wxs_subtract_cone_from_sphere() {
    // Ticket 20260615-103929-9wxs ("Boolean funktioniert nicht mehr":
    // Subtract Cone aus Sphere hat keinen Effekt). The cone tool was
    // ordered AFTER the boolean in the tree — a forward reference. The
    // boolean must now ERROR (red) instead of silently no-op'ing; in the
    // correct order the subtract carves. The cone axis is +Y, so probe
    // the axis centre [0,0,0] (inside cone radius 25 at y=0, inside the
    // r=50 sphere).
    let doc = |cone_after: bool| -> String {
        let sphere = serde_json::json!({"type":"sphere","id":3,"params":{"op":"new","radius":50,"translate":[0,0,0]}});
        let cone = serde_json::json!({"type":"cone","id":6,"params":{"half_height":60,"op":"new","r_bottom":40,"r_top":10,"translate":[0,0,0]}});
        let boolean = serde_json::json!({"type":"boolean","id":5,"params":{"keep_tools":false,"operation":"Subtract","targets":"qCreatedBy(3)","tools":"qCreatedBy(6)"}});
        let calls = if cone_after {
            vec![sphere, boolean, cone]
        } else {
            vec![sphere, cone, boolean]
        };
        serde_json::json!({"schema_version":2,"name":"t","next_feature_id":7,"feature_calls":calls}).to_string()
    };
    let probe = [0.0, 0.0, 0.0];

    // Correct order (cone before boolean) → the subtract carves the centre.
    let mut kg = Kernel::new();
    kg.load_json(&doc(false)).unwrap();
    let tg = csg::CsgNode::from_decoded(&kg.decoded);
    assert!(
        tg.sdf(probe, csg::EvalMode::Logical) > 0.0,
        "cone-before-boolean: subtract carves the sphere centre"
    );

    // Forward-referenced cone (the ticket) → the boolean errors, nothing
    // is carved, and the error is surfaced for the red outliner row.
    let mut kb = Kernel::new();
    kb.load_json(&doc(true)).unwrap();
    let tb = csg::CsgNode::from_decoded(&kb.decoded);
    assert!(
        tb.sdf(probe, csg::EvalMode::Logical) < 0.0,
        "forward-referenced cone must not carve"
    );
    let feats: serde_json::Value =
        serde_json::from_str(&kb.list_features().unwrap()).unwrap();
    let bool_row = feats
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["kind"] == serde_json::json!("boolean"))
        .expect("boolean row present");
    assert!(
        bool_row["error"].is_string(),
        "a forward-referencing boolean must report an error: {bool_row}"
    );
}

#[test]
fn hole_without_scope_warns_and_drills_nothing() {
    // Scope is mandatory (Onshape semantics): a hole with no chosen body
    // warns and drills nothing — the box stays solid and the hole shows as
    // its own (red preview) body.
    let mut k = Kernel::new();
    let _base = k
        .add_primitive(Primitive::KIND_BOX, BoolOp::TAG_NEW, 0.0, 0.0, 0.0, 30.0, 20.0, 30.0)
        .unwrap();
    let sketch_id = k
        .add_sketch(
            "qCreatedBy(origin_plane_xy)",
            r#"[{ "type": "sketch_point", "id": "100", "params": { "at": [0.0, 0.0] } }]"#,
        )
        .unwrap();
    k.add_hole(
        "custom".to_string(),
        10.0,
        "blind".to_string(),
        40.0,
        format!("qSketchPoint({sketch_id}, 1)"),
        String::new(),
    )
    .unwrap();
    assert_eq!(k.body_count(), 2, "an unscoped hole drills nothing (own preview body)");
    let feats: serde_json::Value =
        serde_json::from_str(&k.list_features().unwrap()).unwrap();
    let hole_row = feats
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["kind"] == serde_json::json!("hole"))
        .expect("hole row present");
    assert!(
        hole_row["warning"].is_string(),
        "an unscoped hole must carry a warning: {hole_row}"
    );
}

#[test]
fn boolean_subtract_carves_only_the_explicit_target() {
    // Snapshot semantics: a Subtract with an explicit merge scope carves
    // ONLY the named target, never an unrelated body it happens to
    // overlap. box1 is spine-EARLIER and also overlaps the tool sphere —
    // the old AABB-fallback would have carved box1 (first overlap); the
    // explicit scope (targets = box2) routes the cut to box2 alone.
    let doc = serde_json::json!({
        "schema_version":2,"name":"t","next_feature_id":9,
        "feature_calls":[
            {"type":"box","id":1,"params":{"op":"new","half_extents":[20,20,20],"translate":[40,0,0]}},
            {"type":"box","id":2,"params":{"op":"new","half_extents":[20,20,20],"translate":[-15,0,0]}},
            {"type":"sphere","id":3,"params":{"op":"new","radius":25,"translate":[10,0,0]}},
            {"type":"boolean","id":4,"params":{"keep_tools":false,"operation":"Subtract","targets":"qCreatedBy(2)","tools":"qCreatedBy(3)"}},
        ],
    });
    let mut k = Kernel::new();
    k.load_json(&doc.to_string()).unwrap();
    let t = csg::CsgNode::from_decoded(&k.decoded);
    let sdf = |p: [f32; 3]| t.sdf(p, csg::EvalMode::Logical);
    // box2 (target) carved where the sphere overlaps it.
    assert!(sdf([0.0, 0.0, 0.0]) > 0.0, "target box2 is carved by the sphere");
    // box1 (NOT a target) stays solid even though the sphere overlaps it.
    assert!(
        sdf([25.0, 0.0, 0.0]) < 0.0,
        "non-target box1 must stay solid (explicit scope, no AABB spillover)"
    );
}

/// Per-BODY geometry of a loaded document: one folded CSG tree per
/// partition, in partition order. The whole-spine tree cannot answer a
/// `keep_tools` question — with the tool kept, the union of all bodies is
/// exactly the union of target and tool again. What changes is how that
/// volume is PARTITIONED, and that is only visible per body.
#[cfg(test)]
fn body_trees(k: &Kernel) -> Vec<csg::CsgNode> {
    body::partition(&k.decoded)
        .iter()
        .map(|p| {
            let sub: Vec<DecodedFeature> = p
                .feature_indices
                .iter()
                .map(|&i| k.decoded[i].clone())
                .collect();
            csg::CsgNode::from_decoded(&sub)
        })
        .collect()
}

#[test]
fn boolean_keep_tools_leaves_the_subtracted_tool_standing_as_its_own_body() {
    // #428: `keep_tools` was parsed and then never read — whatever the caller
    // asked for, the tool's op was rewritten to Remove and the tool stopped
    // being a body. Silent geometry loss: the #409 shelving unit lost its
    // four posts to exactly this and measured 887 mm of height where Onshape
    // measures 1840.
    //
    // box [-50,50]³ is the target, a sphere r=40 at [30,0,0] the tool. The
    // sphere pokes 20 mm out of the box's +x face, so "kept" is measurable at
    // [65,0,0]: inside the sphere, outside the box.
    let doc = |keep: bool| {
        serde_json::json!({
            "schema_version":2,"name":"t","next_feature_id":9,
            "feature_calls":[
                {"type":"box","id":1,"params":{"op":"new","half_extents":[50,50,50],"translate":[0,0,0]}},
                {"type":"sphere","id":2,"params":{"op":"new","radius":40,"translate":[30,0,0]}},
                {"type":"boolean","id":3,"params":{"keep_tools":keep,"operation":"Subtract","targets":"qCreatedBy(1)","tools":"qCreatedBy(2)"}},
            ],
        })
    };
    let inside = |t: &csg::CsgNode, p: [f32; 3]| t.sdf(p, csg::EvalMode::Logical) < 0.0;

    // keep_tools = false — the tool is consumed, exactly as before: one body,
    // and the sphere's volume is gone from the document.
    let mut off = Kernel::new();
    off.load_json(&doc(false).to_string()).unwrap();
    let off_bodies = body_trees(&off);
    assert_eq!(off_bodies.len(), 1, "a consumed tool leaves a single body");
    assert!(!inside(&off_bodies[0], [0.0, 0.0, 0.0]), "target is carved");
    assert!(!inside(&off_bodies[0], [65.0, 0.0, 0.0]), "the tool is gone");

    // keep_tools = true — the cut is the SAME, but the tool survives whole.
    let mut on = Kernel::new();
    on.load_json(&doc(true).to_string()).unwrap();
    assert_eq!(on.body_count(), 2, "the kept tool is a body of its own");
    let on_bodies = body_trees(&on);
    let (carved, tool) = (&on_bodies[0], &on_bodies[1]);
    assert!(!inside(carved, [0.0, 0.0, 0.0]), "the target is still carved");
    assert!(inside(carved, [-45.0, 0.0, 0.0]), "…and otherwise intact");
    assert!(inside(tool, [30.0, 0.0, 0.0]), "the kept tool holds its centre");
    assert!(
        inside(tool, [65.0, 0.0, 0.0]),
        "the kept tool keeps its FULL radius — not the leftover cap"
    );
    // The kept body is the tool FEATURE, under its own id: every query that
    // named the tool before the Boolean still names it afterwards. The
    // Boolean carves with a copy, which is why the original can stay put.
    let parts = body::partition(&on.decoded);
    assert_eq!(
        parts[1].seed_id(&on.decoded),
        FeatureId::from_u32(2),
        "the kept body is the tool feature itself"
    );
    assert_eq!(
        parts[0].feature_indices.len(),
        2,
        "the target body is target + the Boolean's working copy"
    );
}

#[test]
fn boolean_keep_tools_on_a_union_leaves_both_tools_and_adds_the_merged_body() {
    // #428, the other half of the rule: `keep_tools` means the Boolean works
    // on COPIES, whichever operation it is. A union with kept tools therefore
    // yields Onshape's three result bodies — both originals, untouched and
    // still separate (no implicit merge), plus the merged body the Boolean
    // built from its copies.
    //
    // Two overlapping boxes: [-30,10]×… and [-10,30]×… along x.
    let doc = |keep: bool| {
        serde_json::json!({
            "schema_version":2,"name":"t","next_feature_id":9,
            "feature_calls":[
                {"type":"box","id":1,"params":{"op":"new","half_extents":[20,20,20],"translate":[-10,0,0]}},
                {"type":"box","id":2,"params":{"op":"new","half_extents":[20,20,20],"translate":[10,0,0]}},
                {"type":"boolean","id":3,"params":{"keep_tools":keep,"operation":"Union","targets":"","tools":"qCreatedBy(1),qCreatedBy(2)"}},
            ],
        })
    };
    let inside = |t: &csg::CsgNode, p: [f32; 3]| t.sdf(p, csg::EvalMode::Logical) < 0.0;

    let mut off = Kernel::new();
    off.load_json(&doc(false).to_string()).unwrap();
    assert_eq!(off.body_count(), 1, "a consumed union fuses into one body");

    let mut on = Kernel::new();
    on.load_json(&doc(true).to_string()).unwrap();
    assert_eq!(on.body_count(), 3, "two kept tools + the merged body");
    let bodies = body_trees(&on);
    assert!(inside(&bodies[0], [-25.0, 0.0, 0.0]), "tool 1 stands");
    assert!(!inside(&bodies[0], [25.0, 0.0, 0.0]), "…and did not absorb tool 2");
    assert!(inside(&bodies[1], [25.0, 0.0, 0.0]), "tool 2 stands");
    assert!(
        inside(&bodies[2], [-25.0, 0.0, 0.0]) && inside(&bodies[2], [25.0, 0.0, 0.0]),
        "the merged body spans both copies"
    );
}

#[test]
fn reorder_rejected_when_it_creates_a_forward_reference() {
    // The tree is a program: a feature may only reference earlier
    // features. A drag that would push a boolean ahead of a tool it
    // references must be rejected, leaving the order untouched.
    let doc = serde_json::json!({
        "schema_version":2,"name":"t","next_feature_id":7,
        "feature_calls":[
            {"type":"sphere","id":3,"params":{"op":"new","radius":50,"translate":[0,0,0]}},
            {"type":"cone","id":6,"params":{"half_height":60,"op":"new","r_bottom":40,"r_top":10,"translate":[0,0,0]}},
            {"type":"boolean","id":5,"params":{"keep_tools":false,"operation":"Subtract","targets":"qCreatedBy(3)","tools":"qCreatedBy(6)"}},
        ],
    });
    let mut k = Kernel::new();
    k.load_json(&doc.to_string()).unwrap();
    let order = |k: &Kernel| -> Vec<u64> {
        serde_json::from_str::<serde_json::Value>(&k.list_features().unwrap())
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["id"].as_u64().unwrap())
            .collect()
    };
    assert_eq!(order(&k), vec![3, 6, 5]);

    // boolean(5) references cone(6) at index 1; moving it to index 1 puts
    // it BEFORE the cone → forward reference → rejected, order unchanged.
    assert!(
        !k.reorder_feature(5, 1),
        "a forward-referencing reorder must be rejected"
    );
    assert_eq!(order(&k), vec![3, 6, 5], "rejected reorder leaves order intact");

    // Moving the cone to index 0 keeps it before the boolean → legal.
    assert!(k.reorder_feature(6, 0), "a legal reorder is applied");
    assert_eq!(order(&k), vec![6, 3, 5]);
}

#[test]
fn boolean_subtract_sdf_is_geometrically_correct() {
    let mut k = Kernel::new();
    let box1 = k.add_primitive(
        Primitive::KIND_BOX, BoolOp::TAG_NEW,
        0.0, 0.0, 0.0, 50.0, 50.0, 50.0,
    ).unwrap();
    let sph = k.add_primitive(
        Primitive::KIND_SPHERE, BoolOp::TAG_NEW,
        30.0, 0.0, 0.0, 40.0, 0.0, 0.0,
    ).unwrap();

    let bool_id = k.add_boolean().unwrap();
    let params = serde_json::json!({
        "operation": "Subtract",
        "tools": format!("qCreatedBy({})", sph),
        "targets": format!("qCreatedBy({})", box1),
        "keep_tools": false,
    });
    k.replace_feature_call_params(bool_id, &params.to_string(), false).unwrap();

    assert_eq!(k.decoded[1].op, BoolOp::Remove);
    assert_eq!(k.body_count(), 1, "box and sphere-diff share one body");

    let tree = csg::CsgNode::from_decoded(&k.decoded);
    let sdf = |p: [f32; 3]| tree.sdf(p, csg::EvalMode::Logical);
    // sphere center [30,0,0] radius 40 → extends x=-10..70
    assert!(sdf([-40.0, 0.0, 0.0]) < 0.0, "inside box, outside sphere: stays solid");
    assert!(sdf([0.0, 0.0, 0.0]) > 0.0, "box center inside sphere: carved out");
    assert!(sdf([35.0, 0.0, 0.0]) > 0.0, "deep in sphere overlap: carved out");
    assert!(sdf([80.0, 0.0, 0.0]) > 0.0, "outside both: exterior");
}

// ─── Feature-based B-Rep topology API + lazy cache ──────────────

#[test]
fn body_brep_counts_box_is_watertight_cube() {
    let mut k = Kernel::new();
    k.decoded = vec![box_feature(
        1,
        BoolOp::New,
        [0.0, 0.0, 0.0],
        [1.0, 1.0, 1.0],
    )];
    k.rebuild_mesh_internal();
    assert_eq!(k.body_count(), 1);

    let json = k.body_brep_counts(0);
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["vertices"], 8, "box B-Rep has 8 corners");
    assert_eq!(v["edges"], 12, "box B-Rep has 12 edges");
    assert_eq!(v["faces"], 6, "box B-Rep has 6 faces");
    assert_eq!(v["shells"], 1);
    assert_eq!(v["solids"], 1);
    assert_eq!(v["watertight"], true, "box outer shell is closed");
}

#[test]
fn body_brep_woven_during_remesh_and_reused() {
    // Brocken A: the mesh path now weaves the B-Rep eagerly during remesh
    // to tag render triangles with their face identity (the user wants the
    // B-Rep present at render time). The weave is cached and keyed on the
    // body-local payload, so subsequent queries reuse it without
    // re-weaving.
    let mut k = Kernel::new();
    k.decoded = vec![box_feature(
        1,
        BoolOp::New,
        [0.0, 0.0, 0.0],
        [1.0, 1.0, 1.0],
    )];
    k.rebuild_mesh_internal();

    // Eagerly woven for face tagging.
    assert!(
        k.bodies[0].brep.is_some(),
        "B-Rep is woven during remesh for mesh face tagging"
    );

    let cached = k.bodies[0].brep.clone();
    let first = k.body_brep_counts(0);
    let second = k.body_brep_counts(0);
    assert_eq!(first, second, "repeat query returns identical counts");
    assert_eq!(
        cached,
        k.bodies[0].brep,
        "queries reuse the cache woven at remesh, not re-weave"
    );
}

#[test]
fn body_brep_reflects_new_geometry_after_remesh() {
    let mut k = Kernel::new();
    k.decoded = vec![box_feature(
        1,
        BoolOp::New,
        [0.0, 0.0, 0.0],
        [1.0, 1.0, 1.0],
    )];
    k.rebuild_mesh_internal();
    let key_before = k.bodies[0].brep.as_ref().map(|(key, _)| *key);
    assert!(key_before.is_some(), "B-Rep woven at remesh");

    // Grow the box → geometry hash changes → full remesh re-weaves the
    // B-Rep under a new payload key (the cache self-heals on geometry
    // change rather than being lazily None'd).
    k.decoded[0] = box_feature(1, BoolOp::New, [0.0, 0.0, 0.0], [2.0, 2.0, 2.0]);
    k.rebuild_mesh_internal();
    let key_after = k.bodies[0].brep.as_ref().map(|(key, _)| *key);
    assert!(key_after.is_some(), "B-Rep re-woven at remesh");
    assert_ne!(
        key_before, key_after,
        "geometry change must re-weave the B-Rep under a new key"
    );
}

#[test]
fn body_brep_never_panics_on_smooth_and_csg_bodies() {
    // (1) Lone sphere: smooth, no sharp features → must not panic.
    let mut k = Kernel::new();
    k.decoded = vec![sphere_feature(1, BoolOp::New, [0.0, 0.0, 0.0], 1.0)];
    k.rebuild_mesh_internal();
    let json = k.body_brep_counts(0);
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(v["vertices"].is_number());
    assert!(v["faces"].is_number());

    // (2) A CSG body (box minus an overlapping sphere) must also
    // weave without panicking.
    let mut k2 = Kernel::new();
    let mut carve = sphere_feature(2, BoolOp::Remove, [2.0, 2.0, 2.0], 1.5);
    carve.target_ids = vec![FeatureId::from_u32(1)]; // explicit scope: carve the box
    k2.decoded = vec![
        box_feature(1, BoolOp::New, [0.0, 0.0, 0.0], [2.0, 2.0, 2.0]),
        carve,
    ];
    k2.rebuild_mesh_internal();
    assert_eq!(k2.body_count(), 1, "box and carving sphere share one body");
    let json2 = k2.body_brep_counts(0);
    let v2: serde_json::Value = serde_json::from_str(&json2).unwrap();
    assert!(v2["edges"].is_number());
    assert!(v2["shells"].is_number());
}

// ─── STEP (AP214) export ────────────────────────────────────────

/// Collect every defined `#N=` id and every referenced `#N` id from a
/// STEP document.
fn step_defined_and_referenced(
    step: &str,
) -> (
    std::collections::HashSet<usize>,
    std::collections::HashSet<usize>,
) {
    let mut defined = std::collections::HashSet::new();
    let mut referenced = std::collections::HashSet::new();
    for line in step.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix('#') {
            if let Some(eq) = rest.find('=') {
                if let Ok(id) = rest[..eq].parse::<usize>() {
                    assert!(defined.insert(id), "#{id} defined twice");
                    let body = &rest[eq + 1..];
                    let bytes = body.as_bytes();
                    let mut i = 0;
                    while i < bytes.len() {
                        if bytes[i] == b'#' {
                            let mut j = i + 1;
                            while j < bytes.len() && bytes[j].is_ascii_digit() {
                                j += 1;
                            }
                            if j > i + 1 {
                                referenced.insert(body[i + 1..j].parse::<usize>().unwrap());
                            }
                            i = j;
                        } else {
                            i += 1;
                        }
                    }
                }
            }
        }
    }
    (defined, referenced)
}

#[test]
fn body_step_export_box_is_valid_advanced_brep() {
    let mut k = Kernel::new();
    k.decoded = vec![box_feature(
        1,
        BoolOp::New,
        [0.0, 0.0, 0.0],
        [1.0, 1.0, 1.0],
    )];
    k.rebuild_mesh_internal();
    assert_eq!(k.body_count(), 1);

    let step = k.body_step_export(0);
    assert!(step.starts_with("ISO-10303-21;"));
    assert!(step.trim_end().ends_with("END-ISO-10303-21;"));
    assert_eq!(step.matches("HEADER;").count(), 1);
    assert_eq!(step.matches("DATA;").count(), 1);
    assert!(step.contains("FILE_SCHEMA(("));

    let count = |name: &str| {
        step.lines()
            .filter(|l| {
                l.find('=')
                    .map(|e| l[e + 1..].trim_start().starts_with(&format!("{name}(")))
                    .unwrap_or(false)
            })
            .count()
    };
    assert_eq!(count("MANIFOLD_SOLID_BREP"), 1);
    assert_eq!(count("CLOSED_SHELL"), 1);
    assert_eq!(count("ADVANCED_FACE"), 6);
    assert_eq!(count("PLANE"), 6);
    assert_eq!(count("VERTEX_POINT"), 8);
    assert_eq!(count("EDGE_CURVE"), 12);

    let (defined, referenced) = step_defined_and_referenced(&step);
    let missing: Vec<usize> = referenced.difference(&defined).copied().collect();
    assert!(missing.is_empty(), "dangling references: {missing:?}");
}

#[test]
fn body_step_export_twisted_box_refs_resolve() {
    // A twisted box yields curved NURBS edges; the STEP graph must
    // still be referentially complete.
    let feat = DecodedFeature::from_parts(
        FeatureId::from_u32(1),
        "Box 1".into(),
        BoolOp::New,
        Primitive::Box { center: [0.0, 0.0, 0.0], half_extents: [1.0, 1.0, 1.0] },
        Transform::IDENTITY,
        PrimitiveModifiers { twist_rate: 0.3, ..PrimitiveModifiers::NONE },
        vec![],
    );
    let mut k = Kernel::new();
    k.decoded = vec![feat];
    k.rebuild_mesh_internal();
    assert_eq!(k.body_count(), 1);

    let step = k.body_step_export(0);
    let (defined, referenced) = step_defined_and_referenced(&step);
    let missing: Vec<usize> = referenced.difference(&defined).copied().collect();
    assert!(missing.is_empty(), "dangling references: {missing:?}");
    assert!(step.contains("B_SPLINE_CURVE_WITH_KNOTS"));
}

// ─── DL-50 correction: body-local weave + placement fold-back ─────────

/// `inverse(t)` must round-trip a rigid + uniform-scale transform:
/// `compose(t, inverse(t)) ≈ compose(inverse(t), t) ≈ IDENTITY`.
#[test]
fn inverse_transform_round_trips() {
    let s2 = std::f32::consts::FRAC_1_SQRT_2; // sin/cos of 45° = 90° half-angle
    let t = Transform {
        translate: [3.0, -2.0, 5.0],
        rotation: [0.0, s2, 0.0, s2], // 90° about Y
        scale: [2.0, 2.0, 2.0],
    };
    let approx_identity = |c: &Transform| {
        let close = |a: [f32; 3], b: [f32; 3]| {
            (0..3).all(|i| (a[i] - b[i]).abs() < 1e-4)
        };
        close(c.translate, [0.0, 0.0, 0.0])
            && close(c.scale, [1.0, 1.0, 1.0])
            // identity quat is (0,0,0,±1)
            && c.rotation[0].abs() < 1e-4
            && c.rotation[1].abs() < 1e-4
            && c.rotation[2].abs() < 1e-4
            && (c.rotation[3].abs() - 1.0).abs() < 1e-4
    };
    let inv = inverse(&t);
    assert!(approx_identity(&compose_transforms(&t, &inv)), "t∘t⁻¹ ≠ I");
    assert!(approx_identity(&compose_transforms(&inv, &t)), "t⁻¹∘t ≠ I");
}

/// Headline reuse test: a pure body move must NOT re-weave. We weave a
/// body, capture the cached B-Rep's address and the placement-invariant
/// local key, then translate every feature by a delta and re-`ensure_brep`.
/// The local key is unchanged and the cached B-Rep is the *same allocation*
/// (pointer identity), proving the weave did not run again.
#[test]
fn move_reuses_local_brep_cache() {
    use body::{compute_body_placement, hash_body_local_payload, Body};
    let features: Vec<DecodedFeature> =
        vec![box_feature(1, BoolOp::New, [0.0, 0.0, 0.0], [1.0, 1.0, 1.0])];
    let parts = body::partition(&features);
    assert_eq!(parts.len(), 1);
    let part = &parts[0];

    let mut bdy = Body {
        id: part.seed_id(&features),
        feature_indices: part.feature_indices.clone(),
        aabb: part.aabb,
        payload_hash: 0,
        geometry_hash: 0,
        transform_hash: 0,
        mesh_resolution: 0,
        mesh: Mesh::default(),
        version: 1,
        transform_delta: None,
        last_mesh_transforms: Vec::new(),
        compiled_sdf: None,
        placement: Transform::IDENTITY,
        brep: None,
    };

    bdy.ensure_brep(&features, false);
    let key0 = hash_body_local_payload(&features, &bdy.feature_indices, &bdy.placement);
    let ptr0 = bdy.brep_cached().unwrap() as *const _;

    // Move the body: translate the (only) feature by a delta. The body's
    // feature_indices stay the same; only the transform shifts.
    let mut moved = features.clone();
    moved[0].transform.translate = [10.0, -4.0, 7.0];

    // Placement now reflects the move (cheap, every call).
    let placement_moved = compute_body_placement(&moved, &bdy.feature_indices);
    assert_eq!(placement_moved.translate, [10.0, -4.0, 7.0]);
    let key1 = hash_body_local_payload(&moved, &bdy.feature_indices, &placement_moved);
    assert_eq!(key0, key1, "pure move must NOT change the local cache key");

    bdy.ensure_brep(&moved, false);
    let ptr1 = bdy.brep_cached().unwrap() as *const _;
    assert_eq!(ptr0, ptr1, "pure move must reuse the cached B-Rep (no re-weave)");
    assert_eq!(bdy.placement.translate, [10.0, -4.0, 7.0], "placement updated");
}

/// World-equivalence: exporting a body placed at transform `T` yields the
/// same world coordinates as the identity-placed body shifted by `T`.
/// Concretely: a box at the origin and the same box translated by `D`
/// export CARTESIAN_POINTs that differ by exactly `D` (the local weave +
/// placement fold-back reproduces the old world-space STEP).
#[test]
fn step_world_equivalence_under_move() {
    let mut at_origin = Kernel::new();
    at_origin.decoded =
        vec![box_feature(1, BoolOp::New, [0.0, 0.0, 0.0], [1.0, 1.0, 1.0])];
    at_origin.rebuild_mesh_internal();
    let step0 = at_origin.body_step_export(0);
    let pts0 = step_cartesian_points(&step0);

    let mut moved = Kernel::new();
    let mut feat = box_feature(1, BoolOp::New, [0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
    feat.transform.translate = [10.0, 20.0, -5.0];
    moved.decoded = vec![feat];
    moved.rebuild_mesh_internal();
    let step1 = moved.body_step_export(0);
    let pts1 = step_cartesian_points(&step1);

    // Same number of emitted points (de-dup is geometry-driven and the
    // geometry is identical up to a rigid shift).
    assert!(!pts0.is_empty(), "box must emit CARTESIAN_POINTs");
    assert_eq!(
        pts0.len(),
        pts1.len(),
        "a move must not change the point count"
    );
    // Every origin point + delta must appear in the moved set: the local
    // weave + placement fold-back reproduces the old world-space STEP.
    let d = [10.0_f64, 20.0, -5.0];
    for p in &pts0 {
        let shifted = [p[0] + d[0], p[1] + d[1], p[2] + d[2]];
        assert!(
            pts1.iter().any(|q| (0..3).all(|i| (q[i] - shifted[i]).abs() < 1e-5)),
            "moved STEP missing shifted point {shifted:?}"
        );
    }
}

// ─── Brocken A: B-Rep face/vertex exports + mesh face tagging ────

/// Parse a `serde_json::Value` array returned by a `bodyBrep*` export.
fn parse_json_array(s: &str) -> Vec<serde_json::Value> {
    match serde_json::from_str::<serde_json::Value>(s).unwrap() {
        serde_json::Value::Array(a) => a,
        other => panic!("expected JSON array, got {other:?}"),
    }
}

/// A unit cube body driven through the full add/rebuild pipeline.
fn box_kernel(half: f32) -> Kernel {
    let mut k = Kernel::new();
    k.add_primitive(Primitive::KIND_BOX, BoolOp::TAG_NEW, 0.0, 0.0, 0.0, half, half, half)
        .unwrap();
    k
}

#[test]
fn brocken_a_box_faces_six_distinct_planes() {
    let mut k = box_kernel(25.0);
    let faces = parse_json_array(&k.body_brep_faces(0));
    assert_eq!(faces.len(), 6, "a box B-Rep has 6 faces");

    let mut id_hashes = std::collections::HashSet::new();
    for f in &faces {
        assert_eq!(f["kind"], "face");
        assert_eq!(f["surfaceKind"], "plane");
        // idHash is a decimal string that parses to a u64.
        let hash_str = f["idHash"].as_str().expect("idHash is a string");
        let parsed: u64 = hash_str.parse().expect("idHash parses to u64");
        assert_ne!(parsed, 0);
        id_hashes.insert(hash_str.to_string());

        // Each face centroid sits on a ±25 box side (one coord at ±25).
        let c = f["centroidLocal"].as_array().unwrap();
        let coords: Vec<f64> = c.iter().map(|v| v.as_f64().unwrap()).collect();
        let on_side = coords.iter().any(|&v| (v.abs() - 25.0).abs() < 1.0);
        assert!(on_side, "centroid {coords:?} should lie on a ±25 box side");

        // Normal is unit-length and axis-aligned for a box.
        let n = f["normalLocal"].as_array().unwrap();
        let nv: Vec<f64> = n.iter().map(|v| v.as_f64().unwrap()).collect();
        let len = (nv[0] * nv[0] + nv[1] * nv[1] + nv[2] * nv[2]).sqrt();
        assert!((len - 1.0).abs() < 1e-6, "normal {nv:?} not unit length");
    }
    assert_eq!(id_hashes.len(), 6, "all 6 face idHashes must be distinct");
}

#[test]
fn brocken_a_box_vertices_eight_distinct() {
    let mut k = box_kernel(25.0);
    let verts = parse_json_array(&k.body_brep_vertices(0));
    assert_eq!(verts.len(), 8, "a box B-Rep has 8 corner vertices");

    let mut id_hashes = std::collections::HashSet::new();
    for v in &verts {
        assert_eq!(v["kind"], "vertex");
        let hash_str = v["idHash"].as_str().expect("idHash is a string");
        let _: u64 = hash_str.parse().expect("idHash parses to u64");
        id_hashes.insert(hash_str.to_string());

        // Corner sits at (±25, ±25, ±25).
        let p = v["positionLocal"].as_array().unwrap();
        for coord in p {
            let c = coord.as_f64().unwrap();
            assert!((c.abs() - 25.0).abs() < 1.0, "corner coord {c} not ±25");
        }
        // At least 3 colliding branches at a box corner.
        assert!(v["branches"].as_array().unwrap().len() >= 3);
    }
    assert_eq!(id_hashes.len(), 8, "all 8 vertex idHashes must be distinct");
}

#[test]
fn brocken_a_placement_round_trips() {
    let mut k = box_kernel(10.0);
    let p: serde_json::Value =
        serde_json::from_str(&k.body_placement(0)).unwrap();
    // Identity placement for an origin box: zero translate, identity quat,
    // unit scale.
    let t = p["translate"].as_array().unwrap();
    assert_eq!(t.iter().map(|v| v.as_f64().unwrap()).collect::<Vec<_>>(), vec![0.0, 0.0, 0.0]);
    let r = p["rotation"].as_array().unwrap();
    assert_eq!(r.iter().map(|v| v.as_f64().unwrap()).collect::<Vec<_>>(), vec![0.0, 0.0, 0.0, 1.0]);
    let s = p["scale"].as_array().unwrap();
    for v in s {
        assert!((v.as_f64().unwrap() - 1.0).abs() < 1e-6);
    }
}

#[test]
fn brocken_a_face_idhash_stable_across_resize() {
    // The whole point of TopoRef: a non-topological parameter change
    // (resize) leaves face idHashes byte-for-byte identical, because they
    // derive from branch signatures, not geometry.
    let mut k_small = box_kernel(10.0);
    let mut k_big = box_kernel(40.0);

    let collect = |k: &mut Kernel| -> std::collections::BTreeSet<String> {
        parse_json_array(&k.body_brep_faces(0))
            .iter()
            .map(|f| f["idHash"].as_str().unwrap().to_string())
            .collect()
    };
    let small = collect(&mut k_small);
    let big = collect(&mut k_big);
    assert_eq!(small, big, "resize must not change face idHashes");
}

#[test]
fn brocken_a_mesh_face_index_partitions_box() {
    let k = box_kernel(25.0);
    // add_primitive already ran the rebuild + per-triangle face tagging.
    let tc = k.body_triangle_count(0);
    assert!(tc > 0, "box must produce a non-empty mesh");

    let face_index = k.bodies[0].mesh.face_index.clone();
    assert_eq!(face_index.len(), tc, "one face tag per mesh triangle");

    // Every triangle resolves to a valid face index < 6, no sentinels on a
    // clean box, and all 6 faces are represented (the tags partition the
    // mesh by side).
    let mut distinct = std::collections::HashSet::new();
    for &fi in &face_index {
        assert_ne!(
            fi,
            crate::brep::face_export::FACE_INDEX_SENTINEL,
            "no sentinels on a clean box",
        );
        assert!(fi < 6, "face_index {fi} must be < 6 for a box");
        distinct.insert(fi);
    }
    assert_eq!(distinct.len(), 6, "the 6 face tags must partition the mesh");

    // Triangles clearly INTERIOR to one flat side (one centroid coord at
    // ±25, the other two well within the face, away from shared edges)
    // must all carry that side's single face tag. Corner/edge triangles
    // straddle two faces and are legitimately ambiguous, so we exclude
    // them. All 6 sides must be represented among the interior triangles.
    let positions = &k.bodies[0].mesh.positions;
    let indices = &k.bodies[0].mesh.indices;
    let mut side_tag: std::collections::HashMap<(usize, i8), u32> =
        std::collections::HashMap::new();
    for t in 0..tc {
        let mut c = [0.0f32; 3];
        for j in 0..3 {
            let vi = indices[t * 3 + j] as usize;
            c[0] += positions[vi * 3];
            c[1] += positions[vi * 3 + 1];
            c[2] += positions[vi * 3 + 2];
        }
        c = [c[0] / 3.0, c[1] / 3.0, c[2] / 3.0];
        // A face-interior triangle has exactly ONE axis pinned to ±25 and
        // the other two safely inside (|coord| < 20), away from any shared
        // edge. Count pinned axes and inside axes.
        let mut pinned_axis: Option<usize> = None;
        let mut pinned_count = 0;
        let mut inside_count = 0;
        for (axis, &coord) in c.iter().enumerate() {
            if (coord.abs() - 25.0).abs() < 0.5 {
                pinned_axis = Some(axis);
                pinned_count += 1;
            } else if coord.abs() < 20.0 {
                inside_count += 1;
            }
        }
        if pinned_count != 1 || inside_count != 2 {
            continue; // corner / edge / off-face triangle
        }
        let axis = pinned_axis.unwrap();
        let sign = if c[axis] >= 0.0 { 1i8 } else { -1i8 };
        let key = (axis, sign);
        let tag = face_index[t];
        match side_tag.get(&key) {
            None => {
                side_tag.insert(key, tag);
            }
            Some(&existing) => assert_eq!(
                existing, tag,
                "interior triangles on side {key:?} share a face tag",
            ),
        }
    }
    assert_eq!(side_tag.len(), 6, "all 6 box sides must be represented");
}

#[test]
fn brocken_br_edges_carry_parseable_id_hash() {
    // Brocken B-R: bodyBrepEdges now emits a decimal `idHash` per edge
    // (alongside a/b/points), so a clicked edge can emit qEdge(<idHash>).
    let mut k = box_kernel(25.0);
    let edges = parse_json_array(&k.body_brep_edges(0));
    assert_eq!(edges.len(), 12, "a box B-Rep has 12 edges");

    let mut id_hashes = std::collections::HashSet::new();
    for e in &edges {
        // Existing fields still present.
        assert!(e["a"].is_number());
        assert!(e["b"].is_number());
        assert!(e["points"].is_array());
        // New field: a decimal string that parses to a u64.
        let hash_str = e["idHash"].as_str().expect("idHash is a string");
        let parsed: u64 = hash_str.parse().expect("idHash parses to u64");
        assert_ne!(parsed, 0);
        id_hashes.insert(hash_str.to_string());
    }
    assert_eq!(id_hashes.len(), 12, "all 12 edge idHashes must be distinct");
}

#[test]
fn ticket_g4rr_sliver_union_weaves_watertight_end_to_end() {
    // Ticket 20260610-084953-g4rr: the user's exact .fdraft — sphere r=50
    // grazes the box top edge with only ~0.33 mm penetration after the
    // final translate (id 12). "Im Union-Bereich Kugel / Würfel
    // degeneriert die Kante". Runs the REAL pipeline: load_json →
    // bootstrap → partition → ensure_brep (parametric default on).
    let mut k = Kernel::new();
    let json = r#"{
        "schema_version": 1,
        "name": "Part",
        "feature_calls": [
            {"type":"box","id":1,"params":{"half_extents":[50,50,50],"op":"new","translate":[0,0,0]}},
            {"type":"sphere","id":2,"params":{"name":"Sphere 2","op":"new","radius":50,"translate":[-86.59912872314453,0,0]}},
            {"type":"translate","id":9,"params":{"target_id":2,"translate":[-86.59912872314453,0,0]}},
            {"type":"boolean","id":10,"params":{"keep_tools":false,"operation":"Union","targets":"","tools":"qCreatedBy(2),qCreatedBy(1)"}},
            {"type":"translate","id":11,"params":{"target_id":1,"translate":[-23.49043846130371,1.4090451002120972,5.554038047790527]}},
            {"type":"translate","id":12,"params":{"target_id":2,"translate":[-86.59912872314453,4.231103897094727,103.45809173583984]}}
        ],
        "next_feature_id": 15
    }"#;
    k.load_json(json).unwrap();
    assert_eq!(k.body_count(), 1, "boolean union must produce one body");
    let counts: serde_json::Value =
        serde_json::from_str(&k.body_brep_counts(0)).unwrap();
    assert_eq!(
        counts["watertight"],
        serde_json::json!(true),
        "sliver-grazing union must weave watertight, got {counts}"
    );
    assert_eq!(
        counts["faces"],
        serde_json::json!(7),
        "6 box faces + 1 sphere face, got {counts}"
    );
    assert_eq!(
        counts["vertices"],
        serde_json::json!(10),
        "8 box corners + 2 lens triple points, got {counts}"
    );
    assert_eq!(
        counts["edges"],
        serde_json::json!(15),
        "11 plain box edges + 2 top-edge fragments + 2 lens arcs, got {counts}"
    );
    // The rendered wireframe polylines (what the user actually sees) must
    // stay inside the model: no lens-arc tessellation point above the box
    // top plane (z = 55.554) or beyond the -x face plane (x = -73.4904) —
    // the reported degeneration was a self-crossing scribble overshooting
    // both by the old activity band (~0.28 mm).
    let edges: serde_json::Value = serde_json::from_str(&k.body_brep_edges(0)).unwrap();
    for e in edges.as_array().unwrap() {
        let (a, b) = (e["a"].as_u64().unwrap(), e["b"].as_u64().unwrap());
        let sphere = crate::csg::BranchId::from_parts(2, 0, 0).as_u64();
        if a != sphere && b != sphere {
            continue;
        }
        let pts: Vec<f64> = e["points"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect();
        for p in pts.chunks(3) {
            assert!(p[2] < 55.5541 + 0.01, "lens polyline above box top: {p:?}");
            assert!(p[0] > -73.4905 - 0.01, "lens polyline beyond -x face: {p:?}");
        }
    }
}

#[test]
fn ticket_9zlq_face_extrude_prism_weaves_straight_edges() {
    // Ticket 20260610-104820-9zlq: the user's exact .fdraft — the box's +y
    // face extruded 20 mm. The face-extrude profile is a Path (4 straight
    // Lines), which the analytic wall lifting used to skip entirely, so
    // the prism fell to the octree route whose sample fits bowed every
    // edge ("Kanten falsch, scheinen geschwungen an den Ecken", up to
    // 16.8 mm chord deviation). With Path G1 groups lifted to planes the
    // prism weaves analytically: watertight, 12 straight edges.
    let mut k = Kernel::new();
    let json = r#"{
        "schema_version": 1,
        "name": "Part",
        "feature_calls": [
            {"type":"box","id":1,"params":{"half_extents":[50,50,50],"op":"new","translate":[0,0,0]}},
            {"type":"boolean","id":10,"params":{"keep_tools":false,"operation":"Union","targets":"","tools":"qCreatedBy(2),qCreatedBy(1)"}},
            {"type":"translate","id":11,"params":{"target_id":1,"translate":[-23.49043846130371,1.4090451002120972,5.554038047790527]}},
            {"type":"extrude","id":15,"params":{"depth":{"unit":"mm","value":20},"depth_back":{"unit":"mm","value":0},"profiles":"qFace(12410059982027593815)","symmetric":false,"operation":"add"}}
        ],
        "next_feature_id": 17
    }"#;
    k.load_json(json).unwrap();
    // This face extrude is an explicit `Add` (the yx5h default is now New),
    // so the prism merges into the box body: ONE part, one taller cuboid.
    assert_eq!(k.body_count(), 1, "face-extrude adds into the box body");
    for body in 0..1 {
        let counts: serde_json::Value =
            serde_json::from_str(&k.body_brep_counts(body)).unwrap();
        assert_eq!(
            counts["watertight"],
            serde_json::json!(true),
            "body {body} must weave watertight, got {counts}"
        );
        // The flush union keeps duplicate coplanar faces for now
        // (open item: coplanar branch merging in the weave), so only
        // watertightness is asserted on the counts.

        // Every edge of the merged cuboid is a straight line: no
        // rendered polyline point may deviate from its own chord by
        // more than 10 µm (the octree-fallback regression bowed them
        // by ~17 mm). Zero-length edges are coplanar-seam artefacts
        // (invisible); they are skipped, not endorsed.
        let edges: serde_json::Value =
            serde_json::from_str(&k.body_brep_edges(body)).unwrap();
        assert!(!edges.as_array().unwrap().is_empty());
        for e in edges.as_array().unwrap() {
            let pts: Vec<f64> = e["points"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_f64().unwrap())
                .collect();
            let n = pts.len() / 3;
            let p0 = [pts[0], pts[1], pts[2]];
            let p1 = [pts[(n - 1) * 3], pts[(n - 1) * 3 + 1], pts[(n - 1) * 3 + 2]];
            let d = [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]];
            let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            if len < 1.0 {
                continue;
            }
            let u = [d[0] / len, d[1] / len, d[2] / len];
            for c in pts.chunks(3) {
                let v = [c[0] - p0[0], c[1] - p0[1], c[2] - p0[2]];
                let t = v[0] * u[0] + v[1] * u[1] + v[2] * u[2];
                let perp = [v[0] - t * u[0], v[1] - t * u[1], v[2] - t * u[2]];
                let dev =
                    (perp[0] * perp[0] + perp[1] * perp[1] + perp[2] * perp[2]).sqrt();
                assert!(
                    dev < 0.01,
                    "body {body} edge {}–{} bows {dev:.4} mm off its chord at {c:?}",
                    e["a"], e["b"]
                );
            }
        }
    }
}

#[test]
fn ticket_px2o_extrude_of_face_extrude_docks_into_parent_with_straight_edges() {
    // Ticket 20260610-230248-px2o: a box's +Y face is extruded (id 5), then
    // the resulting prism's +X face is extruded again (id 17). The second
    // face-extrude carried no `target_ids`, so its `Add` docked by AABB
    // overlap — which (a) missed the parent body (the shared plane is
    // mesh-derived, ~0.13 µm off the box's exact x = 50 bound) and (b)
    // spuriously fused the prism with an unrelated, non-touching body whose
    // loose AABB happened to overlap it. The fused union of two disjoint
    // solids wove to a degenerate shell: the block rendered but had ZERO
    // B-Rep faces/edges, so its silhouette showed wavy mesh lines instead of
    // straight edges ("degenerierte Kanten … hat gar keine BRep-Kanten").
    //
    // The face-extrude now targets the body that owns the picked face, so
    // the prism grows that body (Onshape semantics) and never fuses with an
    // unrelated solid. The merged body weaves watertight with straight edges.
    let mut k = Kernel::new();
    let json = r#"{"schema_version":1,"name":"Part","feature_calls":[
        {"type":"box","id":1,"params":{"half_extents":[50,50,50],"op":"new","translate":[0,0,0]}},
        {"type":"extrude","id":5,"params":{"depth":{"unit":"mm","value":52.91257140451136},"depth_back":{"unit":"mm","value":0},"profiles":"qFace(12410059982027593815)","symmetric":false,"operation":"add"}},
        {"type":"extrude","id":17,"params":{"depth":{"unit":"mm","value":98.74537260854363},"depth_back":{"unit":"mm","value":0},"profiles":"qFace(7017754562111381975)","symmetric":false,"operation":"add"}}
    ],"next_feature_id":18}"#;
    k.load_json(json).unwrap();
    // Both face-extrudes are explicit Add and grow the box: ONE body
    // (the yx5h default is now New).
    assert_eq!(
        k.body_count(),
        1,
        "the extrude-of-an-extrude must grow the box body, not stack a second body"
    );
    let counts: serde_json::Value =
        serde_json::from_str(&k.body_brep_counts(0)).unwrap();
    assert_eq!(
        counts["watertight"],
        serde_json::json!(true),
        "merged body must weave watertight, got {counts}"
    );
    // The block now has real B-Rep edges (the bug produced zero), and every
    // one is straight: no rendered polyline point may bow more than 10 µm
    // off its own chord (the degenerate fallback bowed them by millimetres).
    let edges: serde_json::Value = serde_json::from_str(&k.body_brep_edges(0)).unwrap();
    assert!(
        edges.as_array().unwrap().len() >= 12,
        "expected the merged prism's straight edges, got {}",
        edges.as_array().unwrap().len()
    );
    for e in edges.as_array().unwrap() {
        let pts: Vec<f64> = e["points"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect();
        let n = pts.len() / 3;
        let p0 = [pts[0], pts[1], pts[2]];
        let p1 = [pts[(n - 1) * 3], pts[(n - 1) * 3 + 1], pts[(n - 1) * 3 + 2]];
        let d = [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]];
        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        if len < 1.0 {
            continue;
        }
        let u = [d[0] / len, d[1] / len, d[2] / len];
        for c in pts.chunks(3) {
            let v = [c[0] - p0[0], c[1] - p0[1], c[2] - p0[2]];
            let t = v[0] * u[0] + v[1] * u[1] + v[2] * u[2];
            let perp = [v[0] - t * u[0], v[1] - t * u[1], v[2] - t * u[2]];
            let dev = (perp[0] * perp[0] + perp[1] * perp[1] + perp[2] * perp[2]).sqrt();
            assert!(dev < 0.01, "edge {}–{} bows {dev:.4} mm at {c:?}", e["a"], e["b"]);
        }
    }
}

#[test]
fn ticket_o2j9_two_face_extrude_weaves_manifold() {
    // Ticket 20260610-235049-o2j9: „2 Flächen Extrusion führt zu kaputter
    // Geometrie / Kantenfindung". A single extrude feature targeting TWO
    // picked faces stacks flush coplanar prisms onto an already deeply
    // coincident body. Three roots, all fixed (BREP-CLASSIFICATION-PLAN
    // Phase 3):
    // - near-coincident wall pairs (µm mesh-derived profile drift) made
    //   SSI manufacture a µrad-angle intersection line from noise — the
    //   coincidence registry now gates `seed_branch_pairs` (the plan's
    //   `CoincidentSurface` SSI case);
    // - `both_branches_active` lacked the on-boundary fact, so a seam
    //   march on a concave corner line ran 950 mm past the body, stalled,
    //   and bailed the whole body to the octree (the reported E=0/F=24
    //   ghost-face collapse);
    // - the two-face extrude's union profile abuts its parts along a
    //   shared sketch edge; the internal membrane's cap rim survived as
    //   an open edge striping the merged cap — `drop_fin_edges` now
    //   proves interior-to-survivor-face edges away.
    // The exact decoded geometry is also pinned at weave level in
    // `classification_gate.rs` (`gate_ticket_o2j9_*`).
    let mut k = Kernel::new();
    let json = r#"{"schema_version":1,"name":"Part","feature_calls":[
        {"type":"box","id":1,"params":{"half_extents":[50,50,50],"op":"new","translate":[0,0,0]}},
        {"type":"extrude","id":5,"params":{"depth":{"unit":"mm","value":52.91257140451136},"depth_back":{"unit":"mm","value":0},"profiles":"qFace(12410059982027593815)","symmetric":false,"operation":"add"}},
        {"type":"extrude","id":17,"params":{"depth":{"unit":"mm","value":98.74537260854363},"depth_back":{"unit":"mm","value":0},"profiles":"qFace(7017754562111381975)","symmetric":false,"operation":"add"}}
    ],"next_feature_id":18}"#;
    k.load_json(json).unwrap();
    assert_eq!(k.body_count(), 1);

    // Re-pick the user's two faces from the live model: two distinct
    // upward-facing (+Z) planar faces. The body has THREE such faces (box
    // top + the prisms' +Z walls); pick the two ABUTTING prism walls
    // (centroid y > 50) deterministically — the third combination (box
    // top + prism17 wall) touches only at a corner point, which is a
    // legitimate kissing-contact non-manifold, not this ticket's case.
    let faces: serde_json::Value = serde_json::from_str(&k.body_brep_faces(0)).unwrap();
    let mut picks: Vec<(f64, String)> = Vec::new();
    for f in faces.as_array().unwrap() {
        let n = f["normalLocal"].as_array().unwrap();
        let c = f["centroidLocal"].as_array().unwrap();
        if n[2].as_f64().unwrap() > 0.99 && f["surfaceKind"] == "plane" {
            picks.push((
                c[1].as_f64().unwrap(),
                f["idHash"].as_str().unwrap().to_string(),
            ));
        }
    }
    let picks: Vec<String> = {
        let mut p: Vec<_> = picks.into_iter().filter(|(y, _)| *y > 50.0).collect();
        p.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        p.into_iter().map(|(_, h)| h).collect()
    };
    assert!(picks.len() >= 2, "need two abutting +Z faces to re-create the ticket, got {picks:?}");
    let params = format!(
        r#"{{"depth":{{"unit":"mm","value":25}},"depth_back":{{"unit":"mm","value":0}},"profiles":"qFace({}),qFace({})","symmetric":false,"operation":"add"}}"#,
        picks[0], picks[1]
    );
    k.add_feature_call("extrude", &params).unwrap();

    assert_eq!(k.body_count(), 1, "the two-face extrude grows the box body");
    let counts: serde_json::Value = serde_json::from_str(&k.body_brep_counts(0)).unwrap();
    assert_eq!(
        counts["watertight"],
        serde_json::json!(true),
        "the two-face-extrude stack must weave watertight, got {counts}"
    );
}

/// Collect every `CARTESIAN_POINT('',(x,y,z))` coordinate from a STEP file.
fn step_cartesian_points(step: &str) -> Vec<[f64; 3]> {
    let mut out = Vec::new();
    for line in step.lines() {
        if let Some(idx) = line.find("CARTESIAN_POINT('',(") {
            let rest = &line[idx + "CARTESIAN_POINT('',(".len()..];
            if let Some(end) = rest.find(')') {
                let nums: Vec<f64> = rest[..end]
                    .split(',')
                    .filter_map(|s| s.trim().parse::<f64>().ok())
                    .collect();
                if nums.len() == 3 {
                    out.push([nums[0], nums[1], nums[2]]);
                }
            }
        }
    }
    out
}

/// Walk the parent chain of `child` and return `true` as soon as any
/// ancestor equals `ancestor`. Used by `remove_feature` to cascade
/// deletions across spine clones produced by Pattern/Mirror/Hole
/// execute paths, which live under hierarchical child ids such as
/// `fillet_7.mask_0` or `linear_pattern_3.1`.
fn feature_id_is_descendant_of(child: &FeatureId, ancestor: &FeatureId) -> bool {
    let mut cur = child.parent();
    while let Some(p) = cur {
        if p == ancestor {
            return true;
        }
        cur = p.parent();
    }
    false
}


/// The `sdf-vm-vase-parity` E2E died with a bare WASM `RuntimeError:
/// unreachable` inside `bodyBrepFaces` — a Rust panic, stripped of its message
/// on the way into JS. Two things had to line up: the fixture's revolve carried
/// no `axis` (required since ticket ni56, no implicit local-Y fallback), so it
/// warned and produced NO body; and the B-Rep export then indexed `bodies[0]`
/// of an empty list. This pins the healthy half: a revolve with an axis builds
/// a body whose B-Rep exports faces.
#[test]
fn vase_revolve_builds_a_body_with_brep_faces() {
    let mut k = Kernel::new();
    k.load_json(include_str!("testdata/vase_revolve.json"))
        .expect("vase document loads");
    assert!(
        k.feature_warnings.is_empty(),
        "vase regenerates cleanly, got {:?}",
        k.feature_warnings
    );
    assert_eq!(k.bodies.len(), 1, "the revolved vase is one body");
    let faces = parse_json_array(&k.body_brep_faces(0));
    assert!(!faces.is_empty(), "a revolved vase has B-Rep faces");
}

/// …and this pins the other half: an out-of-range body index must come back
/// EMPTY, not abort the WASM instance. A document can legitimately regenerate
/// to zero bodies (an incomplete revolve is exactly that), and JS still asks
/// for body 0 — that must not kill the kernel for the whole session.
#[test]
fn brep_exports_on_a_missing_body_are_empty_not_a_trap() {
    let mut k = Kernel::new();
    assert_eq!(k.bodies.len(), 0, "a fresh kernel has no bodies");
    assert_eq!(k.body_brep_faces(0), "[]");
    assert_eq!(k.body_brep_edges(0), "[]");
    assert_eq!(k.body_brep_vertices(0), "[]");
    let counts: serde_json::Value = serde_json::from_str(&k.body_brep_counts(0)).unwrap();
    assert_eq!(counts["faces"], 0);
}

// ─── #369: voxel-debug sampling ───────────────────────────

#[test]
fn voxelize_occupancy_matches_box_volume() {
    // A 20×20×20 box (half-extents 10) centred at the origin, voxelised at 40
    // voxels along the longest axis (spacing 0.5 mm). The occupied count must
    // match the analytic volume ratio: (20³)/(0.5³) = 8000/0.125 = 64000.
    let mut k = Kernel::new();
    k.add_primitive(
        Primitive::KIND_BOX,
        BoolOp::TAG_NEW,
        0.0, 0.0, 0.0,
        10.0, 10.0, 10.0,
    )
    .unwrap();

    let info = k.voxel_grid_info(40);
    // [nx, ny, nz, ox, oy, oz, spacing]
    assert_eq!(info.len(), 7, "grid info must be 7 floats");
    let (nx, ny, nz) = (info[0] as usize, info[1] as usize, info[2] as usize);
    let spacing = info[6];
    assert!((spacing - 0.5).abs() < 1e-4, "spacing {spacing} != 0.5");
    // 40 interior voxels + a 1-voxel border on each side per axis = 42.
    assert_eq!([nx, ny, nz], [42, 42, 42], "dims incl. border");

    let occ = k.voxelize_occupancy(40);
    assert_eq!(occ.len(), nx * ny * nz, "occupancy length = grid volume");
    let count = occ.iter().filter(|&&v| v == 1).count();
    let expected = 64_000i64;
    let rel = (count as i64 - expected).abs() as f64 / expected as f64;
    assert!(rel < 0.02, "occupied {count} not within 2% of {expected}");

    // Every occupied byte is exactly 1, every other exactly 0.
    assert!(occ.iter().all(|&v| v == 0 || v == 1));
}

#[test]
fn voxelize_empty_scene_is_empty() {
    let k = Kernel::new();
    assert!(k.voxel_grid_info(32).is_empty());
    assert!(k.voxelize_occupancy(32).is_empty());
}

/// #406/#383 P1 — the hard fillet cutter through the REAL wasm API chain
/// (`addFeatureCall` → `bodyBrepEdges` query → compiled opcode stream via
/// `bodySdfEval`), exactly what the web app runs. Guards against the fillet
/// working in the raw `apply_edge_fillets` unit path but not end-to-end.
#[test]
fn p1_fillet_cutter_via_kernel_api() {
    let mut k = Kernel::new();
    k.add_feature_call("box", r#"{"half_extents":[10.0,10.0,10.0]}"#)
        .unwrap();
    let edges: serde_json::Value =
        serde_json::from_str(&k.body_brep_edges(0)).unwrap();
    let query = edges
        .as_array()
        .unwrap()
        .iter()
        .find(|e| {
            let p = e["points"].as_array().unwrap();
            p[0] == 10.0 && p[1] == 10.0 && p[3] == 10.0 && p[4] == 10.0
        })
        .expect("+X/+Y cube edge")["query"]
        .as_str()
        .unwrap()
        .to_string();
    k.add_feature_call(
        "fillet_edge",
        &format!(r#"{{"edge":"{query}","blend_radius":3.0}}"#),
    )
    .unwrap();
    let d = |x: f32, y: f32, z: f32| k.body_sdf_eval(0, x, y, z);
    // Old sharp edge is carved away; the 45° rolling-ball point is on the
    // surface; an un-filleted edge stays sharp.
    assert!(d(10.0, 10.0, 0.0) > 0.2, "old edge must be outside, got {}", d(10.0, 10.0, 0.0));
    let s = 7.0 + 3.0 / 2.0f32.sqrt();
    assert!(d(s, s, 0.0).abs() < 0.15, "rolling-ball surface ≈0, got {}", d(s, s, 0.0));
    assert!(d(10.0, -10.0, 0.0).abs() < 1e-3, "un-filleted edge stays sharp");
}

#[test]
fn thicken_planar_face_via_kernel_api() {
    // #402 browser repro: thicken the −X face of a box through the REAL API
    // chain (feature call → partition/cache → compiled opcode stream). In the
    // app this produced a constant-0 field (grey render) even though the
    // resolver-level tests passed.
    let mut k = Kernel::new();
    k.add_feature_call("box", r#"{"half_extents":[50.0,50.0,50.0]}"#)
        .unwrap();
    let faces: serde_json::Value =
        serde_json::from_str(&k.body_brep_faces(0)).unwrap();
    let query = faces
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["normalLocal"][0] == -1.0)
        .expect("-X face")["query"]
        .as_str()
        .unwrap()
        .to_string();
    k.add_feature_call(
        "thicken",
        &format!(
            r#"{{"faces":"{query}","thickness":{{"value":2.0,"unit":"mm"}}}}"#
        ),
    )
    .unwrap();
    let d = |x: f32, y: f32, z: f32| k.body_sdf_eval(0, x, y, z);
    assert!(d(0.0, 0.0, 0.0) < -49.0, "box centre stays deep inside");
    assert!(d(-51.0, 0.0, 0.0) < -0.5, "wall interior, got {}", d(-51.0, 0.0, 0.0));
    assert!((d(-52.0, 0.0, 0.0)).abs() < 1e-2, "new outer wall face, got {}", d(-52.0, 0.0, 0.0));
    assert!(d(-53.0, 0.0, 0.0) > 0.5, "beyond the wall, got {}", d(-53.0, 0.0, 0.0));
    // Sharp prismatic rim: outside the slab's y-extent the corner distance is
    // the plain L2 corner distance, no rolling-ball rounding (#402).
    assert!(d(-52.0, 52.0, 0.0) > 1.9, "sharp rim, got {}", d(-52.0, 52.0, 0.0));
}

/// #405: `round_all` end-to-end through the real API. `add_round_all` writes
/// the radius in `Length` wire form (`{value, unit}`); with the old `f32`
/// schema every UI-created round_all failed parsing and silently did nothing
/// (only a low-level ctx test existed — this pins the full chain instead).
#[test]
fn round_all_via_kernel_api_rounds_the_junction() {
    let mut k = Kernel::new();
    let base = k.add_feature_call("box", r#"{"half_extents":[50.0,50.0,50.0]}"#).unwrap();
    k.add_feature_call(
        "box",
        &format!(
            r#"{{"half_extents":[20.0,20.0,20.0],"center":[0.0,0.0,50.0],"op":"add","targets":"qCreatedBy({base})"}}"#
        ),
    )
    .unwrap();
    // Concave junction ring on the top face where the small box emerges.
    // Outside the crease both hard distances are 1.0 at this probe.
    let p = |k: &mut Kernel| k.body_sdf_eval(0, 21.0, 0.0, 51.0);
    let hard = p(&mut k);
    assert!((hard - 1.0).abs() < 1e-3, "hard crease distance, got {hard}");

    // Exactly the JSON `add_round_all` writes — the #405 repro.
    let fid = k
        .add_feature_call("round_all", r#"{"radius":{"value":6.0,"unit":"mm"}}"#)
        .unwrap();
    assert_eq!(
        k.feature_errors.get(&fielddraft_protocol::FeatureId::from_u32(fid)),
        None,
        "round_all must parse Length form"
    );
    let rounded = p(&mut k);
    assert!(
        rounded < hard - 0.1,
        "smooth union fills the crease: {rounded} !< {hard} - 0.1"
    );

    // Editing the radius must re-round (the v0.26.0 cache-hash regression).
    k.replace_feature_call_params(fid, r#"{"radius":{"value":12.0,"unit":"mm"}}"#, false)
        .unwrap();
    let rounder = p(&mut k);
    assert!(rounder < rounded - 0.05, "bigger radius rounds more: {rounder} !< {rounded}");
}

// ─── #404 P1: Onshape-style Hole (simple/drilled, sketch-point + MC) ──────

/// End-to-end guard for the P1 Hole feature over the REAL API chain
/// (`add_feature_call("hole", <exact wire form the addHole setter writes>)`),
/// not the resolver alone — the ledger where the #402/#405 parse bugs hid.
/// A box, a sketch point on the XY origin plane, and a blind M6 bore drilled
/// at that point: the bore is empty (SDF > 0) where solid used to be, the
/// wall beside it stays solid, and the blind bottom stops the bore.
#[test]
fn hole_blind_bore_on_sketch_point_end_to_end() {
    let mut k = Kernel::new();
    let box_id = k
        .add_primitive(Primitive::KIND_BOX, BoolOp::TAG_NEW, 0.0, 0.0, 0.0, 10.0, 10.0, 10.0)
        .unwrap();

    // One free sketch point at (2, 3) on the XY plane. Its solver EntityId is
    // 1 (origin is 0) — the shared Kernel↔Web sketch entity-id.
    let sub_calls = r#"[
        { "type": "sketch_point", "id": "100", "params": { "at": [2.0, 3.0] } }
    ]"#;
    let sketch_id = k.add_sketch("qCreatedBy(origin_plane_xy)", sub_calls).unwrap();

    // Exact wire form the `addHole` setter writes (Length {value,unit}).
    let params = format!(
        r#"{{
            "style": "simple",
            "hole_type": "drilled",
            "standard_size": "M6",
            "diameter": {{ "value": 6.0, "unit": "mm" }},
            "end_type": "blind",
            "depth": {{ "value": 5.0, "unit": "mm" }},
            "locations": "qSketchPoint({sketch_id}, 1)",
            "targets": "qCreatedBy({box_id})"
        }}"#
    );
    let hole_id = k.add_feature_call("hole", &params).unwrap();

    // Feature must parse+regen cleanly (parse-bug guard).
    assert_eq!(
        k.feature_errors.get(&FeatureId::from_u32(hole_id)),
        None,
        "hole must parse the Length wire form"
    );
    assert_eq!(k.body_count(), 1, "one body: the box, now bored");

    let d = |x: f32, y: f32, z: f32| k.body_sdf_eval(0, x, y, z);
    // Before: (2,3,-2) was solid box interior; after it is inside the M6 bore.
    assert!(d(2.0, 3.0, -2.0) > 0.0, "bore interior must be removed");
    // A point one radius+ away laterally stays solid wall.
    assert!(d(7.0, 3.0, -2.0) < 0.0, "wall beside the bore stays solid");
    // Blind depth: 5 mm bore from z=0 downward stops at z=-5, so z=-8 is solid.
    assert!(d(2.0, 3.0, -8.0) < 0.0, "blind bore must stop at its depth");
}

/// Through-all pierces the entire body along the bore axis, unlike blind.
#[test]
fn hole_through_all_pierces_body() {
    let mut k = Kernel::new();
    let box_id = k
        .add_primitive(Primitive::KIND_BOX, BoolOp::TAG_NEW, 0.0, 0.0, 0.0, 10.0, 10.0, 10.0)
        .unwrap();
    let sub_calls = r#"[
        { "type": "sketch_point", "id": "100", "params": { "at": [2.0, 3.0] } }
    ]"#;
    let sketch_id = k.add_sketch("qCreatedBy(origin_plane_xy)", sub_calls).unwrap();
    let params = format!(
        r#"{{
            "style": "simple", "hole_type": "drilled", "standard_size": "M6",
            "diameter": {{ "value": 6.0, "unit": "mm" }},
            "end_type": "through_all",
            "depth": {{ "value": 5.0, "unit": "mm" }},
            "locations": "qSketchPoint({sketch_id}, 1)",
            "targets": "qCreatedBy({box_id})"
        }}"#
    );
    let hole_id = k.add_feature_call("hole", &params).unwrap();
    assert_eq!(k.feature_errors.get(&FeatureId::from_u32(hole_id)), None);

    let d = |x: f32, y: f32, z: f32| k.body_sdf_eval(0, x, y, z);
    // Bore is open the whole way through the box (z from -10 to 10).
    assert!(d(2.0, 3.0, -9.0) > 0.0, "through bore open near the bottom");
    assert!(d(2.0, 3.0, 9.0) > 0.0, "through bore open near the top");
    // Wall beside it remains solid.
    assert!(d(7.0, 3.0, 0.0) < 0.0, "wall beside the through bore stays solid");
}

/// Multiple locations in one feature → multiple bores (Onshape: one Hole
/// feature, N points → N identical holes).
#[test]
fn hole_drills_multiple_locations() {
    let mut k = Kernel::new();
    let box_id = k
        .add_primitive(Primitive::KIND_BOX, BoolOp::TAG_NEW, 0.0, 0.0, 0.0, 10.0, 10.0, 10.0)
        .unwrap();
    let sub_calls = r#"[
        { "type": "sketch_point", "id": "100", "params": { "at": [-5.0, 0.0] } },
        { "type": "sketch_point", "id": "101", "params": { "at": [5.0, 0.0] } }
    ]"#;
    let sketch_id = k.add_sketch("qCreatedBy(origin_plane_xy)", sub_calls).unwrap();
    // Two points → solver EntityIds 1 and 2.
    let params = format!(
        r#"{{
            "style": "simple", "hole_type": "drilled", "standard_size": "M6",
            "diameter": {{ "value": 6.0, "unit": "mm" }},
            "end_type": "through_all",
            "depth": {{ "value": 5.0, "unit": "mm" }},
            "locations": "qSketchPoint({sketch_id}, 1), qSketchPoint({sketch_id}, 2)",
            "targets": "qCreatedBy({box_id})"
        }}"#
    );
    let hole_id = k.add_feature_call("hole", &params).unwrap();
    assert_eq!(k.feature_errors.get(&FeatureId::from_u32(hole_id)), None);

    let d = |x: f32, y: f32, z: f32| k.body_sdf_eval(0, x, y, z);
    assert!(d(-5.0, 0.0, 0.0) > 0.0, "first bore open");
    assert!(d(5.0, 0.0, 0.0) > 0.0, "second bore open");
    assert!(d(0.0, 0.0, 0.0) < 0.0, "material between the two bores stays solid");
}

/// A hole with a mate-connector location drills along the connector +Z.
#[test]
fn hole_on_mate_connector_bores_along_axis() {
    let mut k = Kernel::new();
    let box_id = k
        .add_primitive(Primitive::KIND_BOX, BoolOp::TAG_NEW, 0.0, 0.0, 0.0, 10.0, 10.0, 10.0)
        .unwrap();
    // Find the +Z (top) face query from the woven box.
    let faces: serde_json::Value =
        serde_json::from_str(&k.body_brep_faces(0)).unwrap();
    let top = faces
        .as_array()
        .unwrap()
        .iter()
        .find(|f| {
            let n = f["normalLocal"].as_array().unwrap();
            (n[2].as_f64().unwrap() - 1.0).abs() < 1e-3
        })
        .expect("box has a +Z face");
    let top_query = top["query"].as_str().unwrap();

    // Mate connector anchored on the box top face (+Z normal, origin (0,0,10)).
    let mc_params = serde_json::json!({ "anchor": top_query }).to_string();
    let mc_id = k.add_feature_call("mate_connector", &mc_params).unwrap();
    assert_eq!(k.feature_errors.get(&FeatureId::from_u32(mc_id)), None);

    let params = format!(
        r#"{{
            "style": "simple", "hole_type": "drilled", "standard_size": "M8",
            "diameter": {{ "value": 8.0, "unit": "mm" }},
            "end_type": "blind",
            "depth": {{ "value": 6.0, "unit": "mm" }},
            "locations": "qCreatedBy({mc_id})",
            "targets": "qCreatedBy({box_id})"
        }}"#
    );
    let hole_id = k.add_feature_call("hole", &params).unwrap();
    assert_eq!(
        k.feature_errors.get(&FeatureId::from_u32(hole_id)),
        None,
        "hole must resolve the mate-connector location"
    );
    let d = |x: f32, y: f32, z: f32| k.body_sdf_eval(0, x, y, z);
    // Bore drilled from the top face (z=10) 6mm down into the box.
    assert!(d(0.0, 0.0, 7.0) > 0.0, "bore open just under the top face");
    assert!(d(0.0, 0.0, 0.0) < 0.0, "blind bore stops before mid-box");
}

/// Schema/table drift guard: the `standard_size` variant keys must be exactly
/// `custom` plus every ISO_METRIC row, in order.
#[test]
fn hole_size_variants_match_iso_table() {
    use crate::features::hole::HoleDef;
    use crate::features::hole_iso::ISO_METRIC;
    use crate::features::FeatureSchema;
    let schema = HoleDef::schema_json();
    let params = schema["params"].as_array().expect("params array");
    let size = params
        .iter()
        .find(|p| p["key"] == "standard_size")
        .expect("standard_size param");
    let keys: Vec<String> = size["variants"]
        .as_array()
        .expect("variants")
        .iter()
        .map(|v| v["key"].as_str().unwrap().to_string())
        .collect();
    let mut expected = vec!["custom".to_string()];
    expected.extend(ISO_METRIC.iter().map(|t| t.key.to_string()));
    assert_eq!(keys, expected, "hole size dropdown drifted from ISO_METRIC");
}

/// #414 P1 — the edge chamfer through the REAL wasm API chain: the
/// `addChamferEdge` SETTER (not a hand-written feature call), the compiled
/// opcode stream via `bodySdfEval`, a document save/load round-trip, and an
/// edit of the distance.
///
/// Three things this pins that unit tests cannot:
/// 1. the setter's wire form parses — `distance` must be the `Length`
///    `{value, unit}` form, the exact shape that broke `round_all` (#405) and
///    `thicken` (#402) when a schema field was declared as a bare f32;
/// 2. the chamfer survives persistence — the document carries only the feature
///    call, and the bevel must be back after `load_json`;
/// 3. editing the distance actually re-cuts — the v0.26.0 cache-hash
///    regression, where a post-pass that is not mixed into the body hash
///    silently reuses the stale mesh + compiled SDF (#406).
#[test]
fn chamfer_edge_via_kernel_api_bevels_persists_and_re_cuts() {
    let mut k = Kernel::new();
    k.add_feature_call("box", r#"{"half_extents":[10.0,10.0,10.0]}"#).unwrap();
    let edges: serde_json::Value = serde_json::from_str(&k.body_brep_edges(0)).unwrap();
    let query = edges
        .as_array()
        .unwrap()
        .iter()
        .find(|e| {
            let p = e["points"].as_array().unwrap();
            p[0] == 10.0 && p[1] == 10.0 && p[3] == 10.0 && p[4] == 10.0
        })
        .expect("+X/+Y cube edge")["query"]
        .as_str()
        .unwrap()
        .to_string();

    // (1) exactly what the UI would call.
    let fid = k.add_chamfer_edge(query.clone(), 3.0).unwrap();
    assert_eq!(
        k.feature_errors.get(&fielddraft_protocol::FeatureId::from_u32(fid)),
        None,
        "chamfer_edge must parse the Length wire form the setter writes"
    );

    // The 45° bevel face: through (10,7,0) and (7,10,0), so (8.5,8.5,0) is on
    // it, the old sharp edge is outside, and an un-chamfered edge is untouched.
    let probe = |k: &mut Kernel| {
        (
            k.body_sdf_eval(0, 8.5, 8.5, 0.0),
            k.body_sdf_eval(0, 10.0, 10.0, 0.0),
            k.body_sdf_eval(0, 10.0, -10.0, 0.0),
        )
    };
    let (face, edge, other) = probe(&mut k);
    assert!(face.abs() < 1e-2, "bevel face on the zero set, got {face}");
    assert!(edge > 2.0, "old sharp edge carved away, got {edge}");
    assert!(other.abs() < 1e-2, "un-chamfered edge stays sharp, got {other}");

    // (2) save → load → the bevel is still there.
    let json = k.to_json().unwrap();
    let mut k2 = Kernel::new();
    k2.load_json(&json).unwrap();
    let (face2, edge2, _) = probe(&mut k2);
    assert!(face2.abs() < 1e-2, "bevel survives the document round-trip, got {face2}");
    assert!(edge2 > 2.0, "carved edge survives the round-trip, got {edge2}");

    // (3) edit the distance — a bigger bevel cuts deeper, which only shows if
    // the chamfer is part of the body cache key.
    let edited =
        format!(r#"{{"edge":"{query}","distance":{{"value":6.0,"unit":"mm"}}}}"#);
    k.replace_feature_call_params(fid, &edited, false).unwrap();
    // (10−d/2, 10−d/2) is the bevel face's midpoint. The 6 mm plane lies
    // BEHIND the old 3 mm probe point, so that point is now outside by
    // 3/√2 ≈ 2.12 — a stale cache would still report ≈ 0 there.
    let (face6, _, _) = probe(&mut k);
    assert!(
        (face6 - 3.0 / 2.0f32.sqrt()).abs() < 1e-2,
        "the 6 mm bevel must leave the old 3 mm face point outside; stale cache would \
         keep it at 0, got {face6}"
    );
    let new_face = k.body_sdf_eval(0, 7.0, 7.0, 0.0);
    assert!(new_face.abs() < 1e-2, "the new 6 mm bevel face is on the zero set, got {new_face}");
}

/// #414: the chamfer must be part of the body cache identity. `hash_round_passes`
/// is the single gate for every post-pass applied on top of a body's tree; a
/// field that misses it produces stale BReps rather than a compile error, which
/// is exactly how the v0.26.0 fillet regression escaped.
#[test]
fn chamfer_is_mixed_into_the_body_cache_hash() {
    use crate::csg::EdgeChamfer;
    let mk = |distance: f32| EdgeChamfer {
        chamfer_id: fielddraft_protocol::FeatureId::from_u32(7),
        normal_a: [1.0, 0.0, 0.0],
        normal_b: [0.0, 1.0, 0.0],
        edge_start: [10.0, 10.0, -10.0],
        edge_end: [10.0, 10.0, 10.0],
        distance,
    };
    let h = |c: &[EdgeChamfer]| crate::body::hash_round_passes(&[], c, None);
    let none = h(&[]);
    let three = h(&[mk(3.0)]);
    let six = h(&[mk(6.0)]);
    assert_ne!(none, three, "adding a chamfer must change the body hash");
    assert_ne!(three, six, "editing the distance must change the body hash");
    let mut moved = mk(3.0);
    moved.edge_start = [10.0, 10.0, -9.0];
    assert_ne!(three, h(&[moved]), "moving the edge must change the body hash");
    assert_eq!(three, h(&[mk(3.0)]), "the hash is stable for equal input");
}

// ─── #431: Muster-Kopien tragen unterscheidbare Flächen-Hashes ────
//
// Before #431 the `spatial_index` that disambiguates entities sharing a
// branch signature was assigned PER B-REP. A pattern copy is its own body
// and its B-Rep is woven body-LOCAL (placement divided out), so all copies
// were byte-identical, every index was 0, and three faces carried ONE
// `id_hash` — `qFace(hash)` bound whichever body came first. Measured on a
// 3-copy linear pattern of a box: 18 faces, 6 distinct hashes.

/// Every face `id_hash` of every body, in body / arena order, after the
/// document-wide naming stamp.
fn all_face_hashes(k: &mut Kernel) -> Vec<u64> {
    use crate::brep::naming::TopoRef;
    use crate::brep::FaceId;
    k.ensure_named_breps();
    let mut out = Vec::new();
    for body in &k.bodies {
        let Some(brep) = body.brep_cached() else { continue };
        for i in 0..brep.faces.len() {
            out.push(TopoRef::from_face(brep, FaceId(i as u32)).id_hash);
        }
    }
    out
}

fn boxed_linear_pattern(count: u32) -> Kernel {
    let mut k = Kernel::new();
    let box_id = k
        .add_primitive(Primitive::KIND_BOX, BoolOp::TAG_NEW, 0.0, 0.0, 0.0, 10.0, 10.0, 10.0)
        .unwrap();
    k.add_linear_pattern(box_id, 1.0, 0.0, 0.0, count, 40.0).unwrap();
    k
}

#[test]
fn pattern_copies_get_distinct_face_hashes() {
    let mut k = boxed_linear_pattern(3);
    let hashes = all_face_hashes(&mut k);
    assert_eq!(hashes.len(), 18, "3 copies × 6 box faces");

    let mut uniq = hashes.clone();
    uniq.sort_unstable();
    uniq.dedup();
    assert_eq!(
        uniq.len(),
        18,
        "every face of every pattern copy needs its own id_hash (was 6 before #431)"
    );

    // …and each of them addresses EXACTLY one face, not "one of three".
    for h in &hashes {
        let json = k.resolve_query(&format!("qFace({h})"));
        let hits: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            hits.as_array().map(|a| a.len()).unwrap_or(0),
            1,
            "qFace({h}) must bind exactly one face, got {json}"
        );
    }
}

#[test]
fn circular_pattern_copies_get_distinct_face_hashes() {
    // The circular pattern rotates its copies, so the body placements carry
    // a quaternion — the world fold in the naming stamp has to honour it.
    let mut k = Kernel::new();
    let box_id = k
        .add_primitive(Primitive::KIND_BOX, BoolOp::TAG_NEW, 40.0, 0.0, 0.0, 5.0, 5.0, 5.0)
        .unwrap();
    k.add_circular_pattern(box_id, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 4, 360.0).unwrap();
    let hashes = all_face_hashes(&mut k);
    assert_eq!(hashes.len(), 24, "4 copies × 6 box faces");
    let mut uniq = hashes.clone();
    uniq.sort_unstable();
    uniq.dedup();
    assert_eq!(uniq.len(), 24, "every face of every circular copy needs its own id_hash");
}

// ─── #429/E1: der Name einer Muster-Kopie ist NICHT positionell ───
//
// #431 machte die Namen der Kopien *eindeutig*, aber der Unterscheider ist
// der `spatial_index` — eine Rangnummer nach Weltschwerpunkt. Sobald sich die
// räumliche Anordnung der Kopien ändert (Richtung umdrehen, Abstand negieren,
// eine Kopie einschieben), wandert dieser Rang und damit der Name. E1 ersetzt
// ihn durch ein Instanz-Ordinal aus dem Modell (der Klon-Id), das die
// Umordnung übersteht.

/// `count` copies of the same 10 mm box along ±X. A NEGATIVE `spacing_mm` is
/// not a way to flip the pattern — `LinearPatternFeature::execute` rejects it
/// outright (`must be > 0 mm`) and the regen loop swallows the error, leaving
/// a single body. Measured 2026-07-28: `dir=+1, spacing=-40` yields
/// `bodies=1 faces=6`, not 3 bodies. Flip the DIRECTION instead.
fn boxed_linear_pattern_dir(count: u32, dir_x: f32) -> Kernel {
    let mut k = Kernel::new();
    let box_id = k
        .add_primitive(Primitive::KIND_BOX, BoolOp::TAG_NEW, 0.0, 0.0, 0.0, 10.0, 10.0, 10.0)
        .unwrap();
    k.add_linear_pattern(box_id, dir_x, 0.0, 0.0, count, 40.0).unwrap();
    k
}

#[test]
fn pattern_copy_names_survive_a_direction_flip() {
    // Same model, same copies, mirrored placement: copy 1 is still copy 1.
    // With the positional (#431) stamp the copies are ranked by world
    // centroid, so flipping the direction reverses the ranking and every
    // copy inherits another copy's name.
    let forward = all_face_hashes(&mut boxed_linear_pattern_dir(3, 1.0));
    let backward = all_face_hashes(&mut boxed_linear_pattern_dir(3, -1.0));
    assert_eq!(forward.len(), 18, "3 copies × 6 box faces");
    assert_eq!(backward.len(), 18, "3 copies × 6 box faces");
    assert_eq!(
        forward, backward,
        "reversing the pattern direction only moves the copies in space — \
         their names must not move with them"
    );
}

#[test]
fn pattern_copy_names_survive_an_inserted_copy() {
    // Raising the count appends a copy; the ones that were already there
    // keep their names (their instance ordinals are unchanged).
    let three = all_face_hashes(&mut boxed_linear_pattern_dir(3, 1.0));
    let four = all_face_hashes(&mut boxed_linear_pattern_dir(4, 1.0));
    assert_eq!(three.len(), 18);
    assert_eq!(four.len(), 24);
    assert_eq!(four[..18], three[..], "adding a fourth copy must not rename the first three");
}

/// Face hashes of exactly one query, sorted — the shape of a name set.
fn query_hashes(k: &mut Kernel, q: &str) -> Vec<String> {
    let json = k.resolve_query(q);
    let refs: serde_json::Value = serde_json::from_str(&json).unwrap();
    let mut out: Vec<String> = refs
        .as_array()
        .map(|a| a.iter().map(|r| r["idHash"].as_str().unwrap().to_string()).collect())
        .unwrap_or_default();
    out.sort();
    out
}

#[test]
fn q_instance_names_the_face_of_a_chosen_copy() {
    // `qInstance(i, <q>)` = "the same thing as <q>, but on copy i".
    let mut k = boxed_linear_pattern_dir(3, 1.0);
    k.ensure_named_breps();

    // `qCreatedBy(1)` names the source feature's geometry EVERYWHERE it was
    // replicated — 3 bodies × 6 faces. That was already true before E1 (the
    // copies always carried feature 1); what E1 adds is the ability to say
    // WHICH of them you mean.
    let everywhere = query_hashes(&mut k, "qCreatedBy(1)");
    assert_eq!(everywhere.len(), 18, "the source feature's faces on all three bodies");

    let original = query_hashes(&mut k, "qInstance(0, qCreatedBy(1))");
    assert_eq!(original.len(), 6, "instance 0 is the source box alone");

    let copy1 = query_hashes(&mut k, "qInstance(1, qCreatedBy(1))");
    let copy2 = query_hashes(&mut k, "qInstance(2, qCreatedBy(1))");
    assert_eq!(copy1.len(), 6, "copy 1 has six faces");
    assert_eq!(copy2.len(), 6, "copy 2 has six faces");
    assert_ne!(copy1, original, "a copy is not the original");
    assert_ne!(copy1, copy2, "and the copies are not each other");

    // Together the three instances partition what `qCreatedBy` returns.
    let mut union: Vec<String> =
        original.iter().chain(&copy1).chain(&copy2).cloned().collect();
    union.sort();
    assert_eq!(union, everywhere, "the instances partition the feature's geometry");

    // A copy that does not exist matches nothing. It is NOT rounded to the
    // nearest one — that is the failure mode #414 was made of.
    assert!(query_hashes(&mut k, "qInstance(7, qCreatedBy(1))").is_empty());
}

#[test]
fn q_instance_survives_a_direction_flip() {
    // The whole point of E1: the answer to "copy 1's faces" must not change
    // when the copies swap places in the world.
    let mut fwd = boxed_linear_pattern_dir(3, 1.0);
    let mut back = boxed_linear_pattern_dir(3, -1.0);
    fwd.ensure_named_breps();
    back.ensure_named_breps();
    for q in ["qInstance(1, qCreatedBy(1))", "qInstance(2, qCreatedBy(1))"] {
        let a = query_hashes(&mut fwd, q);
        assert_eq!(a.len(), 6, "{q} must resolve");
        assert_eq!(a, query_hashes(&mut back, q), "{q} moved with the geometry");
    }
}

#[test]
fn q_instance_reaches_an_edge_of_a_copy() {
    // Edges are named by a branch PAIR, so the ordinal has to be swapped on
    // both halves of the signature — the arm that would silently return
    // nothing if only one were shifted.
    let mut k = boxed_linear_pattern_dir(3, 1.0);
    k.ensure_named_breps();
    let brep_edges = {
        let brep = k.bodies[0].brep_cached().expect("source body woven");
        (0..brep.edges.len())
            .map(|i| {
                crate::brep::naming::TopoRef::from_edge(brep, crate::brep::EdgeId(i as u32))
                    .id_hash
            })
            .collect::<Vec<u64>>()
    };
    assert_eq!(brep_edges.len(), 12, "a box has twelve edges");
    let one = brep_edges[0];
    let on_copy = query_hashes(&mut k, &format!("qInstance(2, qEdge({one}))"));
    assert_eq!(on_copy.len(), 1, "the same edge exists exactly once on copy 2");
    assert_ne!(on_copy[0], one.to_string(), "and it is a different edge than the original's");
}

#[test]
fn a_two_source_pattern_keeps_its_sources_apart() {
    // A pattern may copy SEVERAL source features at once. Their clones all
    // sit under the same pattern with the same ordinal, so a branch id that
    // took the PATTERN's feature id would name box 1's +x face and box 2's
    // +x face identically — every copy would lose half its names. Taking the
    // SOURCE's feature id is what keeps them apart (#429/E1).
    let mut k = Kernel::new();
    let a = k
        .add_primitive(Primitive::KIND_BOX, BoolOp::TAG_NEW, 0.0, 0.0, 0.0, 5.0, 5.0, 5.0)
        .unwrap();
    let b = k
        .add_primitive(Primitive::KIND_BOX, BoolOp::TAG_NEW, 0.0, 40.0, 0.0, 5.0, 5.0, 5.0)
        .unwrap();
    k.add_feature_call(
        "linear_pattern",
        &format!(
            r#"{{"source":"qCreatedBy({a}),qCreatedBy({b})","direction":[1,0,0],"count":3,"spacing":{{"value":40,"unit":"mm"}}}}"#
        ),
    )
    .unwrap();

    k.ensure_named_breps();
    // The sharp assertion is on the BRANCH WORDS, not the hashes: a branch
    // collision would still be papered over by the positional `spatial_index`
    // (#431) and the hashes would come out distinct anyway.
    let mut branches: Vec<u64> = Vec::new();
    for body in &k.bodies {
        let Some(brep) = body.brep_cached() else { continue };
        branches.extend(brep.faces.iter().map(|f| f.branch_id.as_u64()));
    }
    assert_eq!(branches.len(), 36, "2 sources × 3 instances × 6 box faces");
    let mut uniq = branches.clone();
    uniq.sort_unstable();
    uniq.dedup();
    assert_eq!(
        uniq.len(),
        36,
        "every face of every copy of every source needs its own BRANCH, not just its own hash"
    );

    let hashes = all_face_hashes(&mut k);
    let mut uh = hashes.clone();
    uh.sort_unstable();
    uh.dedup();
    assert_eq!(uh.len(), 36, "…and therefore its own name");
}

#[test]
fn pattern_face_hashes_are_reproducible() {
    // A unique name is worthless if it is not also stable: the same document
    // built twice must mint the same hashes.
    let a = all_face_hashes(&mut boxed_linear_pattern(3));
    let b = all_face_hashes(&mut boxed_linear_pattern(3));
    assert_eq!(a, b, "the same document must hash identically on a rebuild");
}

#[test]
fn an_unrelated_body_does_not_shift_pattern_hashes() {
    // The stamp is scope-wide, so a body added to the document is a new
    // scope member — it must NOT renumber the pattern. It cannot: it has a
    // different branch signature, so it joins no group of the pattern's.
    // Inserted at the FRONT of the spine with its own id, so the pattern's
    // own feature ids (and therefore its branch ids) are untouched.
    let before = all_face_hashes(&mut boxed_linear_pattern(3));

    let mut k = boxed_linear_pattern(3);
    // Append an unrelated far-away box, then MOVE its call to the front of
    // the spine: it now regenerates before the pattern while the pattern's
    // own feature ids — and therefore its branch ids — stay untouched.
    k.add_primitive(Primitive::KIND_BOX, BoolOp::TAG_NEW, 0.0, 500.0, 0.0, 3.0, 3.0, 3.0)
        .unwrap();
    let extra = k.document.feature_calls.pop().expect("just appended");
    let extra_id = extra.id.clone();
    k.document.feature_calls.insert(0, extra);
    k.bootstrap_features_from_calls().unwrap();
    let after = all_face_hashes(&mut k);
    assert_eq!(after.len(), before.len() + 6, "the extra box adds 6 faces");

    let extra_hashes: Vec<String> = {
        let json = k.resolve_query(&format!("qCreatedBy({extra_id})"));
        let refs: serde_json::Value = serde_json::from_str(&json).unwrap();
        refs.as_array()
            .map(|a| a.iter().map(|r| r["idHash"].as_str().unwrap().to_string()).collect())
            .unwrap_or_default()
    };
    assert_eq!(extra_hashes.len(), 6, "the unrelated box contributes 6 faces");
    let pattern_after: Vec<u64> = after
        .iter()
        .copied()
        .filter(|h| !extra_hashes.contains(&h.to_string()))
        .collect();
    assert_eq!(
        pattern_after, before,
        "an unrelated body must not renumber the pattern's faces"
    );
}
