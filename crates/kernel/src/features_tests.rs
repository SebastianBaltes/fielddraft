use super::*;
use serde_json::json;

use fielddraft_protocol::{Primitive, Transform};

use crate::csg::{CsgNode, PrimitiveModifiers};

fn id(n: u32) -> FeatureId {
    FeatureId::from_u32(n)
}

fn empty_ctx() -> Document {
    Document::default()
}

#[test]
fn with_builtins_registers_the_k1_primitives_and_k4_5_8_extrude() {
    let registry = FeatureRegistry::with_builtins();
    assert_eq!(registry.len(), 31);
    assert!(registry.get("box").is_some());
    assert!(registry.get("sphere").is_some());
    assert!(registry.get("cylinder").is_some());
    assert!(registry.get("extrude").is_some());
    assert!(registry.get("thicken").is_some());
    assert!(registry.get("revolve").is_some());
    assert!(registry.get("hole").is_some());
    assert!(registry.get("torus").is_some());
    assert!(registry.get("thread").is_some());
    assert!(registry.get("cone").is_some());
    assert!(registry.get("capsule").is_some());
    assert!(registry.get("sketch").is_some());
    assert!(registry.get("linear_pattern").is_some());
    assert!(registry.get("circular_pattern").is_some());
    assert!(registry.get("mirror").is_some());
    assert!(registry.get("hex_prism").is_some());
    assert!(registry.get("gyroid").is_some());
    assert!(registry.get("ellipsoid").is_some());
}

// ─── K6a: schema lookup on the registry ───────────────────

#[test]
fn schema_for_returns_the_derive_generated_schema_per_type() {
    let registry = FeatureRegistry::with_builtins();
    let box_schema = registry.schema_for("box").expect("box schema");
    assert_eq!(box_schema["type"], json!("box"));
    assert!(box_schema["params"].is_array());

    let extrude_schema = registry
        .schema_for("extrude")
        .expect("extrude schema");
    assert_eq!(extrude_schema["type"], json!("extrude"));
    let params = extrude_schema["params"].as_array().unwrap();
    assert_eq!(params.len(), 11);
}

#[test]
fn schema_for_returns_none_for_unknown_types() {
    let registry = FeatureRegistry::with_builtins();
    assert!(registry.schema_for("nonexistent_type").is_none());
}

#[test]
fn custom_sdf_maps_to_its_kind_and_carries_the_formula_hint() {
    // Ticket c50f3e15: a `custom_sdf` feature must resolve to the
    // "custom_sdf" outliner kind — NOT "unknown". When it fell through to
    // "unknown", the outliner's `schemaForType` lookup failed and the detail
    // panel showed "No feature selected" instead of the editable dialog, so
    // the formula could never be changed (it stayed the default sphere).
    assert_eq!(
        crate::Kernel::static_kind_name("custom_sdf"),
        "custom_sdf",
        "custom_sdf must not fall through to \"unknown\""
    );
    let registry = FeatureRegistry::with_builtins();
    assert!(registry.get("custom_sdf").is_some(), "custom_sdf registered");
    // The Expression param drives the multi-line formula widget via its hint.
    let schema = registry.schema_for("custom_sdf").expect("custom_sdf schema");
    let expr = schema["params"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["key"] == json!("expression"))
        .expect("expression param");
    assert_eq!(expr["ui_hint"], json!("formula"));
}

#[test]
fn type_names_contains_all_four_builtins() {
    let registry = FeatureRegistry::with_builtins();
    let mut names = registry.type_names();
    names.sort();
    assert_eq!(
        names,
        vec![
            "boolean",
            "box",
            "capsule",
            "circular_pattern",
            "cone",
            "custom_sdf",
            "cylinder",
            "ellipsoid",
            "extrude",
            "fillet_edge",
            "gyroid",
            "helix",
            "hex_prism",
            "hole",
            "imported_mesh",
            "linear_pattern",
            "mate_connector",
            "material",
            "mirror",
            "revolve",
            "rotate",
            "round_all",
            "scale",
            "shell",
            "sketch",
            "sphere",
            "sweep",
            "thicken",
            "thread",
            "torus",
            "translate",
        ]
    );
}

#[test]
fn register_is_last_wins() {
    // A test stub that succeeds unconditionally and returns the
    // type_name "box" — it overrides the built-in.
    struct StubBox;
    impl FeatureKind for StubBox {
        fn type_name(&self) -> &'static str {
            "box"
        }
        fn schema_json(&self) -> Value {
            json!({"type": "box", "params": []})
        }
        fn execute(
            &self,
            _ctx: &mut Context<'_>,
            _id: &FeatureId,
            _def: &Value,
        ) -> Result<(), FeatureError> {
            Err(FeatureError::InvalidParameter {
                feature: "box",
                name: "stub",
                message: "stub rejects everything".into(),
            })
        }
    }
    let mut registry = FeatureRegistry::with_builtins();
    registry.register(Box::new(StubBox));
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    let call = FeatureCall::new(
        "box",
        id(1),
        json!({ "half_extents": [1, 1, 1] }),
    );
    let err = registry.regenerate(&mut ctx, &[call], None).unwrap_err();
    match err {
        FeatureError::InvalidParameter { name: "stub", .. } => {}
        other => panic!("expected stub error, got {other:?}"),
    }
}

#[test]
fn box_primitive_appends_one_spine_feature() {
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);

    let call = FeatureCall::new(
        "box",
        id(1),
        json!({
            "half_extents": [10.0, 20.0, 30.0],
            "translate": [100.0, 0.0, 0.0],
            "op": "new"
        }),
    );
    registry.regenerate(&mut ctx, &[call], None).unwrap();

    assert_eq!(ctx.features.len(), 1);
    let f = &ctx.features[0];
    assert_eq!(f.id, id(1));
    assert_eq!(f.op, BoolOp::New);
    assert_eq!(f.transform.translate, [100.0, 0.0, 0.0]);
    match &f.export_primitive().expect("box primitive") {
        Primitive::Box {
            center,
            half_extents,
        } => {
            assert_eq!(*center, [0.0, 0.0, 0.0]);
            assert_eq!(*half_extents, [10.0, 20.0, 30.0]);
        }
        other => panic!("expected Primitive::Box, got {other:?}"),
    }
}

#[test]
fn sphere_and_cylinder_also_land_on_the_spine() {
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);

    let calls = vec![
        FeatureCall::new("sphere", id(1), json!({ "radius": 5.0 })),
        FeatureCall::new(
            "cylinder",
            id(2),
            json!({
                "radius": 2.0,
                "half_height": 10.0,
                "op": "remove"
            }),
        ),
    ];
    registry.regenerate(&mut ctx, &calls, None).unwrap();

    assert_eq!(ctx.features.len(), 2);
    assert!(matches!(
        ctx.features[0].export_primitive(),
        Some(Primitive::Sphere { radius, .. }) if radius == 5.0
    ));
    assert!(matches!(
        ctx.features[1].export_primitive(),
        Some(Primitive::Cylinder {
            radius,
            half_height,
            ..
        }) if radius == 2.0 && half_height == 10.0
    ));
    assert_eq!(ctx.features[1].op, BoolOp::Remove);
}

#[test]
fn unknown_feature_type_is_an_error() {
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    let call = FeatureCall::new("not_a_feature", id(1), json!({}));
    let err = registry.regenerate(&mut ctx, &[call], None).unwrap_err();
    assert_eq!(
        err,
        FeatureError::UnknownFeatureType("not_a_feature".into())
    );
    assert!(ctx.features.is_empty());
}

#[test]
fn missing_required_parameter_is_an_error() {
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    // Box without `half_extents`.
    let call = FeatureCall::new("box", id(1), json!({}));
    let err = registry.regenerate(&mut ctx, &[call], None).unwrap_err();
    assert_eq!(
        err,
        FeatureError::MissingParameter {
            feature: "box",
            name: "half_extents"
        }
    );
    assert!(ctx.features.is_empty());
}

#[test]
fn invalid_vec3_shape_is_an_error() {
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    let call = FeatureCall::new(
        "box",
        id(1),
        json!({ "half_extents": [1.0, 2.0] }),
    );
    let err = registry.regenerate(&mut ctx, &[call], None).unwrap_err();
    match err {
        FeatureError::InvalidParameter {
            feature: "box",
            name: "half_extents",
            ..
        } => {}
        other => panic!("expected InvalidParameter, got {other:?}"),
    }
}

#[test]
fn negative_radius_is_an_error() {
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    let call =
        FeatureCall::new("sphere", id(1), json!({ "radius": -3.0 }));
    let err = registry.regenerate(&mut ctx, &[call], None).unwrap_err();
    match err {
        FeatureError::InvalidParameter {
            feature: "sphere",
            name: "radius",
            ..
        } => {}
        other => panic!("expected InvalidParameter, got {other:?}"),
    }
}

#[test]
fn regeneration_is_error_tolerant() {
    // Build: one good sphere, one bad cylinder (missing half_height),
    // one good box. Onshape semantics (ticket sx5a): the cylinder's
    // error is RECORDED against its feature id and skipped, while the
    // sphere AND the box both land on the spine.
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    let calls = vec![
        FeatureCall::new("sphere", id(1), json!({ "radius": 5.0 })),
        FeatureCall::new("cylinder", id(2), json!({ "radius": 2.0 })),
        FeatureCall::new(
            "box",
            id(3),
            json!({ "half_extents": [1.0, 1.0, 1.0] }),
        ),
    ];
    let errors = registry.regenerate(&mut ctx, &calls, None);
    assert_eq!(errors.0.len(), 1, "exactly the cylinder must fail");
    assert_eq!(errors.0[0].0, id(2));
    assert!(matches!(
        errors.0[0].1,
        FeatureError::MissingParameter {
            feature: "cylinder",
            name: "half_height"
        }
    ));
    assert_eq!(
        ctx.features.len(),
        2,
        "error tolerance: sphere and box must both land"
    );
    assert!(matches!(
        ctx.features[0].export_primitive(),
        Some(Primitive::Sphere { .. })
    ));
    assert!(matches!(
        ctx.features[1].export_primitive(),
        Some(Primitive::Box { .. })
    ));
}

#[test]
fn feature_call_round_trips_through_json() {
    let call = FeatureCall::new(
        "box",
        id(7),
        json!({
            "half_extents": [10, 20, 30],
            "translate": [100, 0, 0]
        }),
    );
    let wire = serde_json::to_string(&call).unwrap();
    let parsed: FeatureCall = serde_json::from_str(&wire).unwrap();
    assert_eq!(call, parsed);
}

#[test]
fn feature_call_json_uses_type_not_type_name_field() {
    // The Serde rename `#[serde(rename = "type")]` is the
    // canonical §4.6 wire format; nothing else in the project
    // should ever emit `type_name` on the wire.
    let call =
        FeatureCall::new("box", id(1), json!({ "half_extents": [1, 1, 1] }));
    let wire = serde_json::to_value(&call).unwrap();
    assert_eq!(wire.get("type"), Some(&json!("box")));
    assert!(wire.get("type_name").is_none());
}

// ─── K4.5.3: FeatureSchema derive ──────────────────────────

/// Tiny fixture that exercises every field type the K4.5.3
/// derive supports. When a future step grows the derive to
/// cover `Length`, `Angle`, `Query` and friends, those can
/// slot in here alongside the existing four.
#[derive(FeatureSchema, Debug, PartialEq)]
#[schema(type = "fixture")]
struct FixtureDef {
    depth: f32,
    origin: [f32; 3],
    enabled: bool,
    label: String,
}

#[test]
fn derive_generates_type_name_constant_and_method() {
    assert_eq!(FixtureDef::TYPE_NAME, "fixture");
    assert_eq!(FixtureDef::type_name(), "fixture");
}

#[test]
fn derive_emits_schema_with_every_field() {
    let schema = FixtureDef::schema_json();
    assert_eq!(schema["type"], json!("fixture"));
    let params = schema["params"]
        .as_array()
        .expect("params must be an array");
    assert_eq!(params.len(), 4);

    let depth = &params[0];
    assert_eq!(depth["key"], json!("depth"));
    assert_eq!(depth["name"], json!("Depth"));
    assert_eq!(depth["type"], json!("f32"));
    assert_eq!(depth["required"], json!(true));

    let origin = &params[1];
    assert_eq!(origin["key"], json!("origin"));
    assert_eq!(origin["type"], json!("vec3"));

    let enabled = &params[2];
    assert_eq!(enabled["key"], json!("enabled"));
    assert_eq!(enabled["type"], json!("bool"));

    let label = &params[3];
    assert_eq!(label["key"], json!("label"));
    assert_eq!(label["type"], json!("string"));
}

#[test]
fn derive_prettifies_multi_word_identifiers() {
    // `half_extents` → `Half extents`: separate words with a
    // space and capitalise only the first word. This is what
    // the FeatureDialog UI will display (K6a).
    #[derive(FeatureSchema)]
    #[schema(type = "multi_word")]
    #[allow(dead_code)]
    struct MultiWordDef {
        half_extents: [f32; 3],
    }
    let schema = MultiWordDef::schema_json();
    let params = schema["params"].as_array().unwrap();
    assert_eq!(params[0]["name"], json!("Half extents"));
}

#[test]
fn derive_parse_accepts_well_formed_json() {
    let def = json!({
        "depth": 10.5,
        "origin": [1.0, 2.0, 3.0],
        "enabled": true,
        "label": "greeting"
    });
    let parsed = FixtureDef::parse(&def).unwrap();
    assert_eq!(
        parsed,
        FixtureDef {
            depth: 10.5,
            origin: [1.0, 2.0, 3.0],
            enabled: true,
            label: "greeting".to_string(),
        }
    );
}

#[test]
fn derive_parse_reports_missing_fields() {
    let def = json!({
        "depth": 10.5,
        "origin": [1.0, 2.0, 3.0],
        "enabled": true
        // missing `label`
    });
    let err = FixtureDef::parse(&def).unwrap_err();
    assert_eq!(
        err,
        FeatureError::MissingParameter {
            feature: "fixture",
            name: "label"
        }
    );
}

#[test]
fn derive_parse_reports_wrong_types() {
    // number where string is expected
    let def = json!({
        "depth": 10.5,
        "origin": [1.0, 2.0, 3.0],
        "enabled": true,
        "label": 42
    });
    let err = FixtureDef::parse(&def).unwrap_err();
    match err {
        FeatureError::InvalidParameter {
            feature: "fixture",
            name: "label",
            ..
        } => {}
        other => panic!("expected InvalidParameter on `label`, got {other:?}"),
    }
}

#[test]
fn derive_parse_reports_wrong_vec3_shape() {
    let def = json!({
        "depth": 10.5,
        "origin": [1.0, 2.0], // only two elements
        "enabled": true,
        "label": "greeting"
    });
    let err = FixtureDef::parse(&def).unwrap_err();
    match err {
        FeatureError::InvalidParameter {
            feature: "fixture",
            name: "origin",
            ..
        } => {}
        other => panic!("expected InvalidParameter on `origin`, got {other:?}"),
    }
}

#[test]
fn derive_parse_rejects_non_number_vec3_element() {
    let def = json!({
        "depth": 10.5,
        "origin": [1.0, "oops", 3.0],
        "enabled": true,
        "label": "greeting"
    });
    let err = FixtureDef::parse(&def).unwrap_err();
    match err {
        FeatureError::InvalidParameter {
            feature: "fixture",
            name: "origin",
            ..
        } => {}
        other => panic!("expected InvalidParameter on `origin`, got {other:?}"),
    }
}

// ─── K4.5.4a: #[param] attribute ──────────────────────────

/// Every field carries a default or a name override, so the
/// K4.5.4a attribute vocabulary gets exercised end-to-end.
#[derive(FeatureSchema, Debug, PartialEq)]
#[schema(type = "param_fixture")]
#[allow(dead_code)]
struct ParamFixture {
    #[param(name = "Pretty Depth", default = 5.0)]
    depth: f32,
    #[param(default = [1.0, 2.0, 3.0])]
    origin: [f32; 3],
    #[param(default = ::fielddraft_protocol::BoolOp::Remove)]
    op: BoolOp,
}

#[test]
fn param_name_attribute_overrides_display_name() {
    let schema = ParamFixture::schema_json();
    let params = schema["params"].as_array().unwrap();
    let depth = &params[0];
    assert_eq!(depth["key"], json!("depth"));
    assert_eq!(depth["name"], json!("Pretty Depth"));
    // Other fields keep the auto-prettified snake_case name.
    assert_eq!(params[1]["name"], json!("Origin"));
    assert_eq!(params[2]["name"], json!("Op"));
}

#[test]
fn param_default_marks_field_as_optional_in_schema() {
    let schema = ParamFixture::schema_json();
    let params = schema["params"].as_array().unwrap();
    // Every field has a default → required: false.
    for p in params {
        assert_eq!(p["required"], json!(false), "param {:?}", p);
    }
}

#[test]
fn param_default_falls_back_on_missing_keys() {
    // Empty JSON → every field takes its default.
    let parsed = ParamFixture::parse(&json!({})).unwrap();
    assert_eq!(
        parsed,
        ParamFixture {
            depth: 5.0,
            origin: [1.0, 2.0, 3.0],
            op: BoolOp::Remove,
        }
    );
}

#[test]
fn param_default_is_ignored_when_key_is_present() {
    let parsed = ParamFixture::parse(&json!({
        "depth": 42.0,
        "origin": [9.0, 9.0, 9.0],
        "op": "intersect"
    }))
    .unwrap();
    assert_eq!(
        parsed,
        ParamFixture {
            depth: 42.0,
            origin: [9.0, 9.0, 9.0],
            op: BoolOp::Intersect,
        }
    );
}

#[test]
fn param_default_fields_still_validate_wrong_types() {
    // Explicit wrong type must still error — the default only
    // kicks in for missing / null slots.
    let err = ParamFixture::parse(&json!({ "depth": "nope" })).unwrap_err();
    match err {
        FeatureError::InvalidParameter {
            feature: "param_fixture",
            name: "depth",
            ..
        } => {}
        other => panic!("expected InvalidParameter on `depth`, got {other:?}"),
    }
}

#[test]
fn param_default_null_slot_falls_back_to_default() {
    // `null` must behave the same as a missing key.
    let parsed = ParamFixture::parse(&json!({ "depth": null })).unwrap();
    assert_eq!(parsed.depth, 5.0);
}

#[test]
fn param_boolop_accepts_all_four_operator_strings() {
    for (input, expected) in [
        ("add", BoolOp::Add),
        ("new", BoolOp::New),
        ("remove", BoolOp::Remove),
        ("intersect", BoolOp::Intersect),
    ] {
        let def = json!({ "op": input });
        let parsed = ParamFixture::parse(&def).unwrap();
        assert_eq!(parsed.op, expected);
    }
}

#[test]
fn param_boolop_rejects_unknown_operator() {
    let def = json!({ "op": "totally_bogus" });
    let err = ParamFixture::parse(&def).unwrap_err();
    match err {
        FeatureError::InvalidOp {
            feature: "param_fixture",
            value,
        } => assert_eq!(value, "totally_bogus"),
        other => panic!("expected InvalidOp, got {other:?}"),
    }
}

#[test]
fn box_def_schema_matches_the_k1_parameter_surface() {
    // Regression: the derive-generated schema for the box
    // primitive should match the K4.5.2 hand-rolled surface —
    // twelve params after the boolean `targets` BODY picker was
    // added alongside the existing `op` enum so a boolean
    // primitive's op + target are editable in the dialog.
    let schema = BoxDef::schema_json();
    assert_eq!(schema["type"], json!("box"));
    let params = schema["params"].as_array().unwrap();
    assert_eq!(params.len(), 12);

    assert_eq!(params[0]["key"], json!("half_extents"));
    assert_eq!(params[0]["required"], json!(true));

    assert_eq!(params[1]["key"], json!("center"));
    assert_eq!(params[1]["required"], json!(false));

    assert_eq!(params[2]["key"], json!("translate"));
    assert_eq!(params[2]["required"], json!(false));

    assert_eq!(params[3]["key"], json!("op"));
    assert_eq!(params[3]["name"], json!("Op"));
    assert_eq!(params[3]["type"], json!("enum"));
    assert_eq!(params[3]["required"], json!(false));

    // The boolean target picker sits right after the op enum: a
    // BODY multi_query, optional (empty for New).
    assert_eq!(params[4]["key"], json!("targets"));
    assert_eq!(params[4]["type"], json!("multi_query"));
    assert_eq!(params[4]["entity_filter"], json!("BODY"));
    assert_eq!(params[4]["required"], json!(false));
}

#[test]
fn derive_parse_reports_missing_f32_field() {
    let def = json!({
        "origin": [1.0, 2.0, 3.0],
        "enabled": true,
        "label": "greeting"
    });
    let err = FixtureDef::parse(&def).unwrap_err();
    assert_eq!(
        err,
        FeatureError::MissingParameter {
            feature: "fixture",
            name: "depth"
        }
    );
}

// ─── legacy equivalence (K4.5.2) ───────────────────────────

#[test]
fn feature_tree_matches_legacy_add_primitive_path() {
    // The pivotal equivalence test: build the same geometry
    // two ways — through the K1-style `Document::allocate_id +
    // features.push` direct path, and through the K4.5 feature
    // registry — and verify the resulting documents are
    // structurally identical. That's the guarantee K4.5 is
    // built on: *the new execution layer is a faithful proxy
    // for the old one, with zero behaviour drift*.
    // DL-61 C2b: the spine no longer lives on the Document, so
    // the "legacy" reference list is built in a local Vec here.
    let mut legacy_doc = Document::default();
    let mut legacy: Vec<DecodedFeature> = Vec::new();
    for (kind, params) in [
        ("box", ([10.0, 10.0, 10.0], [0.0, 0.0, 0.0], BoolOp::New)),
        (
            "box",
            ([5.0, 5.0, 5.0], [80.0, 0.0, 0.0], BoolOp::Remove),
        ),
    ] {
        let id = legacy_doc.allocate_id();
        let (half, translate, op) = params;
        legacy.push(DecodedFeature::from_parts(
            id.clone(),
            format!("Box {id}"),
            op,
            Primitive::Box {
                center: [0.0; 3],
                half_extents: half,
            },
            Transform {
                translate,
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0, 1.0, 1.0],
            },
            PrimitiveModifiers::NONE,
            vec![],
        ));
        let _ = kind;
    }

    // Same scenario via the registry.
    let mut registry_doc = Document::default();
    let mut ctx = Context::new(&mut registry_doc);
    let registry = FeatureRegistry::with_builtins();
    let calls = vec![
        FeatureCall::new(
            "box",
            id(1),
            json!({
                "half_extents": [10.0, 10.0, 10.0],
                "translate": [0.0, 0.0, 0.0],
                "op": "new"
            }),
        ),
        FeatureCall::new(
            "box",
            id(2),
            json!({
                "half_extents": [5.0, 5.0, 5.0],
                "translate": [80.0, 0.0, 0.0],
                "op": "remove"
            }),
        ),
    ];
    registry.regenerate(&mut ctx, &calls, None).unwrap();

    // The `next_feature_id` counter differs (the registry path
    // never called `allocate_id`), so compare the feature lists
    // directly rather than the whole document.
    assert_eq!(legacy, ctx.features);
}

// ─── K4.5.4f: Length / Angle / Query / generic enum ───────

/// K4.5.4f fixture that exercises every new param type the
/// derive learned in this slice: `Length`, `Angle`, `Query`,
/// plus a user-defined enum that falls through to the generic
/// `serde_json::from_value` path — standing in for the §4.6
/// "Enum" parameter slot.
#[derive(
    Debug,
    PartialEq,
    ::serde::Serialize,
    ::serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
enum Finish {
    Anodised,
    Polished,
    Brushed,
}

#[derive(FeatureSchema, Debug, PartialEq)]
#[schema(type = "runtime_fixture")]
#[allow(dead_code)]
struct RuntimeFixture {
    depth: ::fielddraft_protocol::Length,
    sweep: ::fielddraft_protocol::Angle,
    seed: ::fielddraft_protocol::Query,
    finish: Finish,
}

#[test]
fn runtime_fixture_schema_reports_typed_param_names() {
    let schema = RuntimeFixture::schema_json();
    let params = schema["params"].as_array().unwrap();
    assert_eq!(params.len(), 4);
    assert_eq!(params[0]["key"], json!("depth"));
    assert_eq!(params[0]["type"], json!("length"));
    assert_eq!(params[1]["key"], json!("sweep"));
    assert_eq!(params[1]["type"], json!("angle"));
    assert_eq!(params[2]["key"], json!("seed"));
    assert_eq!(params[2]["type"], json!("query"));
    // The user-defined enum lands in the generic fallback
    // slot; the schema hint is `"generic"`.
    assert_eq!(params[3]["key"], json!("finish"));
    assert_eq!(params[3]["type"], json!("generic"));
}

#[test]
fn runtime_fixture_round_trips_all_new_types() {
    let def = json!({
        "depth": {"value": 25.0, "unit": "mm"},
        "sweep": {"value": 90.0, "unit": "deg"},
        "seed": "qCreatedBy(box_1)",
        "finish": "polished"
    });
    let parsed = RuntimeFixture::parse(&def).unwrap();
    assert_eq!(parsed.depth, ::fielddraft_protocol::Length::mm(25.0));
    assert_eq!(parsed.sweep, ::fielddraft_protocol::Angle::deg(90.0));
    assert_eq!(parsed.seed.as_str(), "qCreatedBy(box_1)");
    assert_eq!(parsed.finish, Finish::Polished);
}

#[test]
fn length_param_rejects_missing_unit_field() {
    // serde's `from_value` errors out when the Length struct
    // is missing a required field — the derive maps that to
    // InvalidParameter with the offending field name.
    let def = json!({
        "depth": {"value": 10.0},
        "sweep": {"value": 0.0, "unit": "rad"},
        "seed": "q",
        "finish": "brushed"
    });
    let err = RuntimeFixture::parse(&def).unwrap_err();
    match err {
        FeatureError::InvalidParameter {
            feature: "runtime_fixture",
            name: "depth",
            ..
        } => {}
        other => panic!("expected InvalidParameter on `depth`, got {other:?}"),
    }
}

#[test]
fn angle_param_rejects_unknown_unit() {
    let def = json!({
        "depth": {"value": 1.0, "unit": "mm"},
        "sweep": {"value": 100.0, "unit": "gradian"},
        "seed": "q",
        "finish": "brushed"
    });
    let err = RuntimeFixture::parse(&def).unwrap_err();
    match err {
        FeatureError::InvalidParameter {
            feature: "runtime_fixture",
            name: "sweep",
            ..
        } => {}
        other => panic!("expected InvalidParameter on `sweep`, got {other:?}"),
    }
}

#[test]
fn query_param_rejects_non_string() {
    let def = json!({
        "depth": {"value": 1.0, "unit": "mm"},
        "sweep": {"value": 0.0, "unit": "rad"},
        "seed": 42,
        "finish": "brushed"
    });
    let err = RuntimeFixture::parse(&def).unwrap_err();
    match err {
        FeatureError::InvalidParameter {
            feature: "runtime_fixture",
            name: "seed",
            ..
        } => {}
        other => panic!("expected InvalidParameter on `seed`, got {other:?}"),
    }
}

#[test]
fn generic_enum_fallback_rejects_unknown_variant() {
    let def = json!({
        "depth": {"value": 1.0, "unit": "mm"},
        "sweep": {"value": 0.0, "unit": "rad"},
        "seed": "q",
        "finish": "chromed"
    });
    let err = RuntimeFixture::parse(&def).unwrap_err();
    match err {
        FeatureError::InvalidParameter {
            feature: "runtime_fixture",
            name: "finish",
            ..
        } => {}
        other => panic!("expected InvalidParameter on `finish`, got {other:?}"),
    }
}

/// A second fixture that proves `#[param(default = ...)]`
/// still works when the field type is a runtime typed param
/// (K4.5.4f × K4.5.4a interaction).
#[derive(FeatureSchema, Debug, PartialEq)]
#[schema(type = "runtime_defaults")]
#[allow(dead_code)]
struct RuntimeDefaults {
    #[param(default = ::fielddraft_protocol::Length::mm(5.0))]
    depth: ::fielddraft_protocol::Length,
    #[param(default = ::fielddraft_protocol::Angle::deg(0.0))]
    sweep: ::fielddraft_protocol::Angle,
}

#[test]
fn runtime_defaults_apply_on_missing_keys() {
    let parsed = RuntimeDefaults::parse(&json!({})).unwrap();
    assert_eq!(parsed.depth, ::fielddraft_protocol::Length::mm(5.0));
    assert_eq!(parsed.sweep, ::fielddraft_protocol::Angle::deg(0.0));
}

#[test]
fn runtime_defaults_marked_optional_in_schema() {
    let schema = RuntimeDefaults::schema_json();
    let params = schema["params"].as_array().unwrap();
    for p in params {
        assert_eq!(p["required"], json!(false), "param {p:?}");
    }
}

// ─── K4.5.8: ExtrudeDef reference implementation ──────────

/// Convenience: build an extrude definition JSON with the
/// K4.5.8 schema. The `depth` is expressed as a
/// `{value, unit}` object so the `Length` parser path runs.
fn extrude_def(
    profile_query: &str,
    depth_mm: f32,
    half_x: f32,
    half_y: f32,
) -> serde_json::Value {
    // These executor-mechanics tests predate the yx5h default flip
    // (symmetric → false, operation → New). They verify the centred /
    // Add lowering specifically, so the helper pins those explicitly
    // rather than riding on the schema defaults.
    json!({
        "profiles": profile_query,
        "depth": { "value": depth_mm, "unit": "mm" },
        "profile_half_x": half_x,
        "profile_half_y": half_y,
        "symmetric": true,
        "operation": "add",
    })
}

#[test]
fn extrude_def_schema_has_every_runtime_type() {
    let schema = ExtrudeDef::schema_json();
    assert_eq!(schema["type"], json!("extrude"));
    let params = schema["params"].as_array().unwrap();
    assert_eq!(params.len(), 11);

    // Each key / type pair documents the K4.5.4f vocabulary
    // demonstrated in a single definition.
    let map: std::collections::HashMap<&str, &serde_json::Value> =
        params.iter().map(|p| (p["key"].as_str().unwrap(), p)).collect();
    assert_eq!(map["profiles"]["type"], json!("multi_query"));
    assert_eq!(map["profiles"]["required"], json!(true));
    assert_eq!(map["depth"]["type"], json!("length"));
    assert_eq!(map["depth"]["required"], json!(true));
    assert_eq!(map["profile_half_x"]["type"], json!("f32"));
    assert_eq!(map["profile_half_y"]["type"], json!("f32"));
    // K6a.1: BoolOp flows through the generic `enum` slot and
    // carries its variant list + a `horizontal_enum` UI hint.
    assert_eq!(map["operation"]["type"], json!("enum"));
    assert_eq!(map["operation"]["required"], json!(false));
    assert_eq!(map["operation"]["ui_hint"], json!("horizontal_enum"));
    let variants = map["operation"]["variants"]
        .as_array()
        .expect("operation variants");
    let variant_keys: Vec<&str> = variants
        .iter()
        .map(|v| v["key"].as_str().unwrap())
        .collect();
    assert_eq!(variant_keys, vec!["add", "new", "remove", "intersect"]);
    assert_eq!(map["center"]["type"], json!("vec3"));
    assert_eq!(map["center"]["required"], json!(false));
    // #9: optional shell thickness hollows the extruded body.
    assert_eq!(map["shell_thickness"]["type"], json!("length"));
    assert_eq!(map["shell_thickness"]["required"], json!(false));
    // Face-extrude (P1) now rides on `profiles` (ticket k280) — no separate
    // `faces` field. Direction modes remain.
    assert_eq!(map["symmetric"]["type"], json!("bool"));
    assert_eq!(map["symmetric"]["required"], json!(false));
    assert_eq!(map["depth_back"]["type"], json!("length"));
    assert_eq!(map["depth_back"]["required"], json!(false));
    // Ticket 15f5: optional merge scope (explicit target bodies).
    assert_eq!(map["targets"]["type"], json!("multi_query"));
    assert_eq!(map["targets"]["required"], json!(false));
    assert_eq!(map["targets"]["entity_filter"], json!("BODY"));
}

#[test]
fn extrude_shell_thickness_lowers_to_a_shell_node() {
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    let mut def = extrude_def("qCreatedBy(s)", 40.0, 5.0, 7.0);
    def["shell_thickness"] = json!({ "value": 2.0, "unit": "mm" });
    ExtrudeFeature.execute(&mut ctx, &id(1), &def).unwrap();

    let f = &ctx.features[0];
    // The geometry node is wrapped in a Shell of the requested
    // wall thickness; the inner solid is unchanged (export_primitive
    // walks through the Shell wrapper to the underlying box).
    match &f.node {
        crate::csg::CsgNode::Shell(_, t, _) => assert!((t - 2.0).abs() < 1e-5),
        other => panic!("expected CsgNode::Shell, got {other:?}"),
    }
    match &f.export_primitive().expect("primitive") {
        Primitive::Box { half_extents, .. } => {
            assert_eq!(*half_extents, [5.0, 7.0, 20.0]);
        }
        other => panic!("expected Primitive::Box, got {other:?}"),
    }
}

#[test]
fn extrude_zero_shell_thickness_stays_solid() {
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    ExtrudeFeature
        .execute(&mut ctx, &id(1), &extrude_def("qCreatedBy(s)", 40.0, 5.0, 7.0))
        .unwrap();
    // No shell param ⇒ default 0 ⇒ no Shell wrapper.
    assert!(
        !matches!(&ctx.features[0].node, crate::csg::CsgNode::Shell(..)),
        "zero shell_thickness must not wrap the node in a Shell",
    );
}

#[test]
fn extrude_execute_lowers_to_a_box_with_depth_as_z_half_extent() {
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    let feature = ExtrudeFeature;
    feature
        .execute(
            &mut ctx,
            &id(1),
            &extrude_def("qCreatedBy(sketch_1)", 40.0, 5.0, 7.0),
        )
        .unwrap();

    assert_eq!(ctx.features.len(), 1);
    let f = &ctx.features[0];
    assert_eq!(f.id, id(1));
    assert_eq!(f.op, BoolOp::Add);
    match &f.export_primitive().expect("primitive") {
        Primitive::Box { center, half_extents } => {
            assert_eq!(*center, [0.0, 0.0, 0.0]);
            // profile_half_x / profile_half_y carry through
            // unchanged; depth (40 mm) lowers to a Z
            // half-extent of 20 mm so the box is centred on
            // the extrude plane.
            assert_eq!(*half_extents, [5.0, 7.0, 20.0]);
        }
        other => panic!("expected Primitive::Box, got {other:?}"),
    }
}

#[test]
fn extrude_converts_non_mm_depth_to_model_units() {
    // 5 cm should land as z half-extent = 25 mm.
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    let feature = ExtrudeFeature;
    let def = json!({
        "profiles": "qCreatedBy(s)",
        "depth": { "value": 5.0, "unit": "cm" },
        "profile_half_x": 1.0,
        "profile_half_y": 1.0,
    });
    feature.execute(&mut ctx, &id(1), &def).unwrap();
    let f = &ctx.features[0];
    match &f.export_primitive().expect("primitive") {
        Primitive::Box { half_extents, .. } => {
            assert!((half_extents[2] - 25.0).abs() < 1e-5);
        }
        other => panic!("expected Primitive::Box, got {other:?}"),
    }
}

#[test]
fn extrude_honours_center_and_operation_parameters() {
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    let feature = ExtrudeFeature;
    let def = json!({
        "profiles": "qCreatedBy(s)",
        "depth": { "value": 10.0, "unit": "mm" },
        "profile_half_x": 2.0,
        "profile_half_y": 3.0,
        "operation": "remove",
        // Centred extrude (symmetric) so `center` is the box centre
        // exactly; the yx5h default is one-sided.
        "symmetric": true,
        "center": [100.0, 50.0, 0.0],
    });
    feature.execute(&mut ctx, &id(7), &def).unwrap();
    let f = &ctx.features[0];
    assert_eq!(f.op, BoolOp::Remove);
    match &f.export_primitive().expect("primitive") {
        Primitive::Box { center, half_extents } => {
            assert_eq!(*center, [100.0, 50.0, 0.0]);
            assert_eq!(half_extents[2], 5.0);
        }
        other => panic!("expected Primitive::Box, got {other:?}"),
    }
}

#[test]
fn extrude_qregion_extrudes_only_the_picked_region() {
    // Ticket tgdg: a two-region sketch (rectangle + a separate circle). The
    // extrude must now resolve `qRegion(s, i)` through the same path Revolve
    // uses, so a single picked region extrudes JUST that region — not the
    // whole sketch. Before the fix the executor honoured only
    // `qCreatedBy(sketch)` and a `qRegion` fell through to the half-extent
    // fallback (a wrong box / no body).
    use crate::sketch::{SketchData, SketchFrame};
    use fielddraft_solver::SketchState;
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    let sketch_calls = vec![
        ::fielddraft_protocol::FeatureCall::new(
            "sketch_rectangle",
            id(2),
            json!({ "corner_a": [-10.0, -10.0], "corner_b": [10.0, 10.0] }),
        ),
        ::fielddraft_protocol::FeatureCall::new(
            "sketch_circle",
            id(3),
            json!({ "center": [40.0, 0.0], "radius": 5.0 }),
        ),
    ];
    ctx.sketches.insert(
        id(1),
        SketchData {
            state: SketchState::with_origin(),
            sub_calls: sketch_calls.clone(),
            frame: SketchFrame::IDENTITY,
        },
    );

    // Find which region index is the circle (centre x ≈ 40).
    let whole = crate::sketch::build_sketch2d_from_sub_calls(&sketch_calls);
    let regions = crate::sketch_regions::find_regions(&whole, 96);
    let circle = regions.iter().find(|r| r.centroid[0] > 20.0).unwrap();

    let def = json!({
        "profiles": format!("qRegion(1, {})", circle.index),
        "depth": { "value": 10.0, "unit": "mm" },
    });
    ExtrudeFeature.execute(&mut ctx, &id(4), &def).unwrap();
    match &ctx.features[0].export_primitive().expect("extrude primitive") {
        Primitive::Extrude { sketch, .. } => {
            assert!(sketch.sdf([40.0, 0.0]) < 0.0, "circle region kept");
            assert!(sketch.sdf([0.0, 0.0]) > 0.0, "rectangle region dropped");
        }
        other => panic!("expected Primitive::Extrude, got {other:?}"),
    }
}

#[test]
fn extrude_rejects_non_positive_profile_and_depth() {
    let feature = ExtrudeFeature;
    let mut doc = empty_ctx();

    // Zero half_x
    {
        let mut ctx = Context::new(&mut doc);
        let def = extrude_def("q", 10.0, 0.0, 1.0);
        let err = feature.execute(&mut ctx, &id(1), &def).unwrap_err();
        assert!(matches!(
            err,
            FeatureError::InvalidParameter {
                feature: "extrude",
                name: "profile_half_x",
                ..
            }
        ));
    }
    // Zero extent (depth 0, no depth_back) — nothing to sweep.
    {
        let mut ctx = Context::new(&mut doc);
        let def = json!({
            "profiles": "q",
            "depth": { "value": 0.0, "unit": "mm" },
            "profile_half_x": 1.0,
            "profile_half_y": 1.0,
        });
        let err = feature.execute(&mut ctx, &id(1), &def).unwrap_err();
        assert!(matches!(
            err,
            FeatureError::InvalidParameter {
                feature: "extrude",
                name: "depth",
                ..
            }
        ));
    }
}

#[test]
fn extrude_negative_depth_sweeps_in_the_minus_normal_direction() {
    // Onshape-style depth drag through the profile plane (ticket x7af):
    // symmetric treats the sign as irrelevant (±|d|/2), blind mode spans
    // the sorted interval over {−back, depth} — a negative depth grows
    // the prism on the −normal side of the plane.
    let feature = ExtrudeFeature;
    let mut doc = empty_ctx();

    // Symmetric: |−5|/2 = 2.5 half-extent, centred on the plane.
    {
        let mut ctx = Context::new(&mut doc);
        let def = json!({
            "profiles": "q",
            "depth": { "value": -5.0, "unit": "mm" },
            "profile_half_x": 1.0,
            "profile_half_y": 1.0,
            "symmetric": true,
        });
        feature.execute(&mut ctx, &id(1), &def).unwrap();
        match &ctx.features[0].export_primitive().expect("primitive") {
            Primitive::Box { center, half_extents } => {
                assert_eq!(*center, [0.0, 0.0, 0.0]);
                assert!((half_extents[2] - 2.5).abs() < 1e-6);
            }
            other => panic!("expected Primitive::Box, got {other:?}"),
        }
    }
    // Blind: span [−10, 0] → centre z = −5, half-extent 5.
    {
        let mut ctx = Context::new(&mut doc);
        let def = json!({
            "profiles": "q",
            "depth": { "value": -10.0, "unit": "mm" },
            "profile_half_x": 1.0,
            "profile_half_y": 1.0,
            "symmetric": false,
        });
        feature.execute(&mut ctx, &id(2), &def).unwrap();
        match &ctx.features[0].export_primitive().expect("primitive") {
            Primitive::Box { center, half_extents } => {
                assert!((center[2] - -5.0).abs() < 1e-6);
                assert!((half_extents[2] - 5.0).abs() < 1e-6);
            }
            other => panic!("expected Primitive::Box, got {other:?}"),
        }
    }
}

#[test]
fn extrude_is_reachable_via_the_builtin_registry() {
    // End-to-end: look up "extrude" on the default registry
    // and regenerate a single call. This is the path any
    // document load or worker regeneration would take.
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    let registry = FeatureRegistry::with_builtins();
    let calls = vec![FeatureCall::new(
        "extrude",
        id(1),
        extrude_def("qCreatedBy(sketch_1)", 12.0, 3.0, 4.0),
    )];
    registry.regenerate(&mut ctx, &calls, None).unwrap();
    assert_eq!(ctx.features.len(), 1);
    match &ctx.features[0].export_primitive().expect("primitive") {
        Primitive::Box { half_extents, .. } => {
            assert_eq!(*half_extents, [3.0, 4.0, 6.0]);
        }
        other => panic!("expected Primitive::Box, got {other:?}"),
    }
}

/// Rust-level stand-in for the §4.6 `test_extrude_chain` JS
/// example. The JS body hangs on `fd.registerFeature`,
/// `fd.sketch`, and `fd.extrude` — none of which are wired
/// through the worker yet. This Rust test mirrors the
/// behaviour that `test_extrude_chain` will eventually drive
/// end-to-end: build `count` extrude calls in a loop, each
/// with a depth that grows linearly, regenerate the whole
/// chain through the registry, and verify the resulting
/// spine. Once the JS-side `fd.registerFeature` path lands,
/// an equivalent Playwright-layer test will run alongside
/// this one and compare outputs.
#[test]
fn extrude_chain_regenerates_five_features_with_growing_depths() {
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    let registry = FeatureRegistry::with_builtins();

    const COUNT: u32 = 5;
    const SPACING_MM: f32 = 20.0;
    let calls: Vec<FeatureCall> = (0..COUNT)
        .map(|i| {
            let depth_mm = ((i + 1) as f32) * 5.0;
            FeatureCall::new(
                "extrude",
                id(i + 1),
                json!({
                    "profiles": format!("qCreatedBy(sketch_{i})"),
                    "depth": { "value": depth_mm, "unit": "mm" },
                    "profile_half_x": 5.0,
                    "profile_half_y": 5.0,
                    "center": [(i as f32) * SPACING_MM, 0.0, 0.0],
                    "operation": "new",
                    // Centred so `center` is the box centre (yx5h default
                    // is one-sided, which would shift centre[2] by depth/2).
                    "symmetric": true,
                }),
            )
        })
        .collect();

    registry.regenerate(&mut ctx, &calls, None).unwrap();

    assert_eq!(ctx.features.len(), COUNT as usize);
    for (i, f) in ctx.features.iter().enumerate() {
        let expected_depth = ((i as u32 + 1) as f32) * 5.0;
        let expected_center_x = (i as f32) * SPACING_MM;
        match &f.export_primitive().expect("primitive") {
            Primitive::Box { center, half_extents } => {
                assert_eq!(*center, [expected_center_x, 0.0, 0.0]);
                assert!(
                    (half_extents[2] - expected_depth / 2.0).abs() < 1e-5,
                    "step {i}: half-z expected {}, got {}",
                    expected_depth / 2.0,
                    half_extents[2]
                );
            }
            other => panic!("expected Primitive::Box, got {other:?}"),
        }
    }
}

#[test]
fn extrude_query_field_round_trips_through_the_parser() {
    // The profiles field is accepted as an opaque Query
    // string in K4.5.8. Verify that well-formed query
    // expressions from K4.5.7's vocabulary parse into the
    // definition without rejection.
    let feature = ExtrudeFeature;
    for q in [
        "qCreatedBy(sketch_1)",
        "qFaceOf(42)",
        "qEdgeBetween(1, 2)",
    ] {
        let mut doc = empty_ctx();
        let mut ctx = Context::new(&mut doc);
        feature
            .execute(&mut ctx, &id(1), &extrude_def(q, 10.0, 1.0, 1.0))
            .unwrap_or_else(|e| panic!("query `{q}` failed: {e}"));
    }
}

// ─── K8.0: linear pattern ─────────────────────────────────

#[test]
fn linear_pattern_emits_count_minus_one_offset_clones() {
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    let box_id = id(1);
    let pattern_id = id(2);

    let box_call = FeatureCall::new(
        "box",
        box_id.clone(),
        json!({
            "half_extents": [5.0, 5.0, 5.0],
            "center": [0.0, 0.0, 0.0],
            "translate": [0.0, 0.0, 0.0],
            "op": "new"
        }),
    );
    let pattern_call = FeatureCall::new(
        "linear_pattern",
        pattern_id.clone(),
        json!({
            "source": "qCreatedBy(1)",
            "direction": [1.0, 0.0, 0.0],
            "count": 4,
            "spacing": { "value": 20.0, "unit": "mm" }
        }),
    );
    registry
        .regenerate(&mut ctx, &[box_call, pattern_call], None)
        .unwrap();

    // Source (1) + three clones = four spine entries.
    assert_eq!(ctx.features.len(), 4);
    let source = ctx
        .features
        .iter()
        .find(|f| f.id == box_id)
        .expect("source box");
    assert_eq!(source.transform.translate, [0.0, 0.0, 0.0]);

    // Clones live at pattern_id/1_1, /1_2, /1_3 (namespaced by
    // the source feature id) with 20 mm increments along +X.
    for i in 1..4_u32 {
        let clone_id = pattern_id.child(format!("1_{i}"));
        let clone = ctx
            .features
            .iter()
            .find(|f| f.id == clone_id)
            .expect("clone spine entry");
        let expected_x = 20.0_f32 * i as f32;
        assert!((clone.transform.translate[0] - expected_x).abs() < 1e-5);
        // Primitive payload is cloned verbatim.
        let clone_prim = clone.export_primitive();
        let source_prim = source.export_primitive();
        if let (
            Some(Primitive::Box { half_extents, .. }),
            Some(Primitive::Box { half_extents: src_he, .. }),
        ) = (&clone_prim, &source_prim)
        {
            assert_eq!(half_extents, src_he);
        } else {
            panic!("expected box primitives");
        }
    }
}

#[test]
fn linear_pattern_count_1_is_a_noop() {
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    registry
        .regenerate(
            &mut ctx,
            &[
                FeatureCall::new(
                    "box",
                    id(1),
                    json!({
                        "half_extents": [1.0, 1.0, 1.0],
                        "op": "new"
                    }),
                ),
                FeatureCall::new(
                    "linear_pattern",
                    id(2),
                    json!({
                        "source": "qCreatedBy(1)",
                        "direction": [1.0, 0.0, 0.0],
                        "count": 1,
                        "spacing": { "value": 20.0, "unit": "mm" }
                    }),
                ),
            ],
            None,
        )
        .unwrap();
    // Just the source box — no clones added.
    assert_eq!(ctx.features.len(), 1);
}

#[test]
fn linear_pattern_normalises_the_direction_vector() {
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    registry
        .regenerate(
            &mut ctx,
            &[
                FeatureCall::new(
                    "box",
                    id(1),
                    json!({
                        "half_extents": [1.0, 1.0, 1.0],
                        "op": "new"
                    }),
                ),
                FeatureCall::new(
                    "linear_pattern",
                    id(2),
                    json!({
                        "source": "qCreatedBy(1)",
                        // Non-unit vector — the executor
                        // must normalise so spacing still
                        // lands at 10 mm, not 10 × √3 mm.
                        "direction": [3.0, 4.0, 0.0],
                        "count": 2,
                        "spacing": { "value": 10.0, "unit": "mm" }
                    }),
                ),
            ],
            None,
        )
        .unwrap();
    let clone = &ctx.features[1];
    // Unit (3,4,0) = (0.6, 0.8, 0). Offset at i=1 is 10 mm
    // along that direction → (6, 8, 0).
    assert!((clone.transform.translate[0] - 6.0).abs() < 1e-4);
    assert!((clone.transform.translate[1] - 8.0).abs() < 1e-4);
    assert!((clone.transform.translate[2] - 0.0).abs() < 1e-4);
}

#[test]
fn linear_pattern_errors_on_missing_source() {
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    let err = registry
        .regenerate(
            &mut ctx,
            &[FeatureCall::new(
                "linear_pattern",
                id(1),
                json!({
                    "source": "qCreatedBy(999)",
                    "direction": [1.0, 0.0, 0.0],
                    "count": 3,
                    "spacing": { "value": 20.0, "unit": "mm" }
                }),
            )],
            None,
        )
        .unwrap_err();
    assert!(matches!(
        err,
        FeatureError::MissingParameter {
            feature: "linear_pattern",
            ..
        }
    ));
}

// ─── K8.1: circular pattern ───────────────────────────────

#[test]
fn circular_pattern_places_clones_around_y_axis_at_even_angles() {
    // A unit box at (10, 0, 0) circular-patterned around
    // the Y axis 4 times should land clones at (0, 0, -10),
    // (-10, 0, 0), (0, 0, 10) — ±1e-4 since the rotation
    // math goes through f32 trig.
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);

    let box_call = FeatureCall::new(
        "box",
        id(1),
        json!({
            "half_extents": [1.0, 1.0, 1.0],
            "translate": [10.0, 0.0, 0.0],
            "op": "new"
        }),
    );
    let pattern_call = FeatureCall::new(
        "circular_pattern",
        id(2),
        json!({
            "source": "qCreatedBy(1)",
            "axis": [0.0, 1.0, 0.0],
            "center": [0.0, 0.0, 0.0],
            "count": 4,
            "angle_deg": 360.0
        }),
    );
    registry
        .regenerate(&mut ctx, &[box_call, pattern_call], None)
        .unwrap();

    // 1 source + 3 clones.
    assert_eq!(ctx.features.len(), 4);
    let expectations = [
        (1_u32, [0.0, 0.0, -10.0]),
        (2_u32, [-10.0, 0.0, 0.0]),
        (3_u32, [0.0, 0.0, 10.0]),
    ];
    for (i, expected) in expectations {
        let clone_id = id(2).child(format!("1_{i}"));
        let clone = ctx
            .features
            .iter()
            .find(|f| f.id == clone_id)
            .unwrap_or_else(|| panic!("missing clone {i}"));
        // indexes both translate[k] and expected[k]; k also used in messages
        #[allow(clippy::needless_range_loop)]
        for k in 0..3 {
            assert!(
                (clone.transform.translate[k] - expected[k]).abs() < 1e-4,
                "clone {i} axis {k}: expected {} got {}",
                expected[k],
                clone.transform.translate[k]
            );
        }
    }
}

#[test]
fn circular_pattern_count_1_is_a_noop() {
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    registry
        .regenerate(
            &mut ctx,
            &[
                FeatureCall::new(
                    "box",
                    id(1),
                    json!({
                        "half_extents": [1.0, 1.0, 1.0],
                        "translate": [5.0, 0.0, 0.0],
                        "op": "new"
                    }),
                ),
                FeatureCall::new(
                    "circular_pattern",
                    id(2),
                    json!({
                        "source": "qCreatedBy(1)",
                        "axis": [0.0, 1.0, 0.0],
                        "count": 1,
                    }),
                ),
            ],
            None,
        )
        .unwrap();
    // No clones — just the source.
    assert_eq!(ctx.features.len(), 1);
}

#[test]
fn circular_pattern_respects_the_center_pivot_for_non_origin_rotations() {
    // Source at (10, 0, 2), pivot at (5, 0, 0), axis +Y,
    // 2 instances at 180° sweep: the one clone sits at the
    // opposite side of the pivot from the source.
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    registry
        .regenerate(
            &mut ctx,
            &[
                FeatureCall::new(
                    "sphere",
                    id(1),
                    json!({
                        "radius": 1.0,
                        "translate": [10.0, 0.0, 2.0],
                        "op": "new"
                    }),
                ),
                FeatureCall::new(
                    "circular_pattern",
                    id(2),
                    json!({
                        "source": "qCreatedBy(1)",
                        "axis": [0.0, 1.0, 0.0],
                        "center": [5.0, 0.0, 0.0],
                        "count": 2,
                        "angle_deg": 180.0
                    }),
                ),
            ],
            None,
        )
        .unwrap();
    // 180° step = 90° per instance. At i=1 the clone sits
    // 90° around the Y axis from the source, relative to
    // the pivot. Offset (10-5, 0, 2-0) = (5, 0, 2). 90°
    // CCW around +Y maps (x, y, z) → (z, y, -x) = (2, 0, -5).
    // Plus pivot: (7, 0, -5).
    let clone = &ctx.features[1];
    assert!((clone.transform.translate[0] - 7.0).abs() < 1e-4);
    assert!((clone.transform.translate[1] - 0.0).abs() < 1e-4);
    assert!((clone.transform.translate[2] + 5.0).abs() < 1e-4);
}

#[test]
fn circular_pattern_errors_on_zero_axis() {
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    let err = registry
        .regenerate(
            &mut ctx,
            &[
                FeatureCall::new(
                    "box",
                    id(1),
                    json!({ "half_extents": [1.0, 1.0, 1.0], "op": "new" }),
                ),
                FeatureCall::new(
                    "circular_pattern",
                    id(2),
                    json!({
                        "source": "qCreatedBy(1)",
                        "axis": [0.0, 0.0, 0.0],
                        "count": 4
                    }),
                ),
            ],
            None,
        )
        .unwrap_err();
    assert!(matches!(
        err,
        FeatureError::InvalidParameter {
            feature: "circular_pattern",
            name: "axis",
            ..
        }
    ));
}

// ─── K8.11: formula engine ────────────────────────────────

#[test]
fn regenerate_resolves_length_formulas_against_variables() {
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    doc.variables.insert(
        "width".to_string(),
        fielddraft_protocol::Variable::new(
            50.0,
            fielddraft_protocol::VarKind::Length,
        ),
    );
    let mut ctx = Context::new(&mut doc);
    let call = FeatureCall::new(
        "hole",
        id(1),
        json!({
            // Diameter references a variable via the
            // K8.11 sigil; depth stays a plain literal.
            "diameter": { "value": "=width / 5", "unit": "mm" },
            "depth": { "value": 40.0, "unit": "mm" },
            "translate": [0.0, 0.0, 0.0],
        }),
    );
    registry.regenerate(&mut ctx, &[call], None).unwrap();
    assert_eq!(ctx.features.len(), 1);
    let f = &ctx.features[0];
    match &f.export_primitive().expect("primitive") {
        Primitive::Cylinder { radius, .. } => {
            // width(50) / 5 / 2 (hole → radius) = 5
            assert!((*radius - 5.0).abs() < 1e-5);
        }
        other => panic!("expected Primitive::Cylinder, got {other:?}"),
    }
}

#[test]
fn regenerate_surfaces_unknown_variable_errors() {
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    let call = FeatureCall::new(
        "hole",
        id(1),
        json!({
            "diameter": { "value": "=never_defined", "unit": "mm" },
            "depth": { "value": 40.0, "unit": "mm" },
        }),
    );
    let err = registry.regenerate(&mut ctx, &[call], None).unwrap_err();
    match err {
        FeatureError::InvalidParameter { feature, message, .. } => {
            assert_eq!(feature, "<formula>");
            assert!(message.contains("never_defined"));
        }
        other => panic!("expected InvalidParameter, got {other:?}"),
    }
}

// ─── K8.9: hole ───────────────────────────────────────────

#[test]
fn hole_lowers_to_difference_cylinder_with_diameter_halved() {
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    let call = FeatureCall::new(
        "hole",
        id(1),
        json!({
            "diameter": { "value": 6.0, "unit": "mm" },
            "depth": { "value": 40.0, "unit": "mm" },
            "translate": [10.0, 0.0, 0.0],
        }),
    );
    registry.regenerate(&mut ctx, &[call], None).unwrap();
    assert_eq!(ctx.features.len(), 1);
    let f = &ctx.features[0];
    // Spine entry uses the hierarchical child id `1/0` so
    // sync_feature_calls skips it.
    assert_eq!(f.id, id(1).child("0"));
    assert!(f.id.parent().is_some());
    assert_eq!(f.op, BoolOp::Remove);
    assert_eq!(f.transform.translate, [10.0, 0.0, 0.0]);
    match &f.export_primitive().expect("primitive") {
        Primitive::Cylinder {
            center,
            radius,
            half_height,
        } => {
            assert_eq!(*center, [0.0, 0.0, 0.0]);
            assert!((*radius - 3.0).abs() < 1e-5);
            assert!((*half_height - 20.0).abs() < 1e-5);
        }
        other => panic!("expected Primitive::Cylinder, got {other:?}"),
    }
}

#[test]
fn hole_rejects_non_positive_diameter() {
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    let err = registry
        .regenerate(
            &mut ctx,
            &[FeatureCall::new(
                "hole",
                id(1),
                json!({
                    "diameter": { "value": 0.0, "unit": "mm" },
                    "depth": { "value": 10.0, "unit": "mm" },
                }),
            )],
            None,
        )
        .unwrap_err();
    assert!(matches!(
        err,
        FeatureError::InvalidParameter {
            feature: "hole",
            name: "diameter",
            ..
        }
    ));
}

// ─── K8.7: suppress ───────────────────────────────────────

#[test]
fn suppressed_feature_call_is_skipped_by_regenerate() {
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    let mut box_call = FeatureCall::new(
        "box",
        id(1),
        json!({ "half_extents": [1.0, 1.0, 1.0], "op": "new" }),
    );
    let sphere_call = FeatureCall::new(
        "sphere",
        id(2),
        json!({ "radius": 2.0, "op": "new" }),
    );
    box_call.suppressed = true;
    registry
        .regenerate(&mut ctx, &[box_call, sphere_call], None)
        .unwrap();
    // Only the sphere made it onto the spine.
    assert_eq!(ctx.features.len(), 1);
    assert_eq!(ctx.features[0].id, id(2));
}

// ─── K8.2: mirror ─────────────────────────────────────────

#[test]
fn mirror_reflects_translate_through_yz_plane() {
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    registry
        .regenerate(
            &mut ctx,
            &[
                FeatureCall::new(
                    "box",
                    id(1),
                    json!({
                        "half_extents": [1.0, 1.0, 1.0],
                        "translate": [10.0, 5.0, -3.0],
                        "op": "new"
                    }),
                ),
                FeatureCall::new(
                    "mirror",
                    id(2),
                    json!({
                        "source": "qCreatedBy(1)",
                        // YZ datum plane: normal +X through origin.
                        "plane": "qCreatedBy(origin_plane_yz)"
                    }),
                ),
            ],
            None,
        )
        .unwrap();
    // Source + one mirrored clone.
    assert_eq!(ctx.features.len(), 2);
    assert_eq!(ctx.features[0].transform.translate, [10.0, 5.0, -3.0]);
    assert_eq!(ctx.features[1].transform.translate, [-10.0, 5.0, -3.0]);
}

#[test]
fn mirror_through_a_real_face_uses_its_offset_plane() {
    // The plane query can address a real B-Rep face, not just a
    // datum. A box centred at origin (half 5) has a +X face at
    // x = 5; mirroring a body at x = 20 through that face lands
    // it at x = -10 (reflection is sign-of-normal independent).
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);

    // Box 1 supplies the mirror plane; weave it to learn the
    // id_hash of its +X face (the face with the largest centroid x).
    let plane_box = FeatureCall::new(
        "box",
        id(1),
        json!({ "half_extents": [5.0, 5.0, 5.0], "op": "new" }),
    );
    registry
        .regenerate(&mut ctx, std::slice::from_ref(&plane_box), None)
        .unwrap();
    let woven = crate::body::weave_all(&ctx.features);
    let (brep, _placement) = &woven[0];
    let plus_x = (0..brep.face_count())
        .map(|i| crate::brep::FaceId(i as u32))
        .max_by(|&a, &b| {
            let ca = crate::brep::naming::Fingerprint::of_face(brep, a).centroid[0];
            let cb = crate::brep::naming::Fingerprint::of_face(brep, b).centroid[0];
            ca.partial_cmp(&cb).unwrap()
        })
        .expect("a box face");
    let face_hash = crate::brep::naming::TopoRef::from_face(brep, plus_x).id_hash;

    let mut doc2 = empty_ctx();
    let mut ctx2 = Context::new(&mut doc2);
    registry
        .regenerate(
            &mut ctx2,
            &[
                plane_box.clone(),
                FeatureCall::new(
                    "box",
                    id(2),
                    json!({
                        "half_extents": [1.0, 1.0, 1.0],
                        "translate": [20.0, 0.0, 0.0],
                        "op": "new"
                    }),
                ),
                FeatureCall::new(
                    "mirror",
                    id(3),
                    json!({
                        "source": "qCreatedBy(2)",
                        "plane": format!("qFace({face_hash})"),
                    }),
                ),
            ],
            None,
        )
        .unwrap();
    // The mirrored clone of box 2 (at x=20) reflects through x=5
    // to x=-10.
    let clone = ctx2
        .features
        .iter()
        .find(|f| f.id == id(3).child("2_1"))
        .expect("mirror clone");
    assert!((clone.transform.translate[0] - (-10.0)).abs() < 1e-4);
    assert!(clone.transform.translate[1].abs() < 1e-4);
    assert!(clone.transform.translate[2].abs() < 1e-4);
}

#[test]
fn mirror_errors_on_unresolvable_plane() {
    // A plane query that resolves to neither a datum nor a real
    // face is a clean InvalidParameter on `plane`.
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    let err = registry
        .regenerate(
            &mut ctx,
            &[
                FeatureCall::new(
                    "box",
                    id(1),
                    json!({ "half_extents": [1.0, 1.0, 1.0], "op": "new" }),
                ),
                FeatureCall::new(
                    "mirror",
                    id(2),
                    json!({
                        "source": "qCreatedBy(1)",
                        // No feature 999, so no face frame.
                        "plane": "qFace(999)"
                    }),
                ),
            ],
            None,
        )
        .unwrap_err();
    assert!(matches!(
        err,
        FeatureError::InvalidParameter {
            feature: "mirror",
            name: "plane",
            ..
        }
    ));
}

// ─── K7.1: extrude consumes real sketches ─────────────────

#[test]
fn extrude_derives_profile_extents_from_a_resolved_rectangle_sketch() {
    // Build a sketch with a 20x10 rectangle centred at
    // (5, 3), commit it via the registry so the context
    // sketches map gets populated, then run an extrude
    // that references it. The extrude's half-extents must
    // come from the sketch's AABB, not from the
    // `profile_half_x/y` fallback.
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    let sketch_id = id(1);
    let extrude_id = id(2);

    let sketch_call = FeatureCall::new(
        "sketch",
        sketch_id.clone(),
        json!({
            "plane": "qCreatedBy(origin_plane_xy)",
            "sub_calls": [
                {
                    "type": "sketch_rectangle",
                    "id": "r1",
                    "params": {
                        "corner_a": [-5.0, -2.0],
                        "corner_b": [15.0, 8.0],
                    }
                }
            ]
        }),
    );
    let extrude_call = FeatureCall::new(
        "extrude",
        extrude_id.clone(),
        json!({
            "profiles": format!("qCreatedBy({sketch_id})"),
            "depth": { "value": 12.0, "unit": "mm" },
            // Explicit bogus fallbacks: the resolver should
            // override these with AABB-derived values.
            "profile_half_x": 999.0,
            "profile_half_y": 999.0,
        }),
    );
    registry
        .regenerate(&mut ctx, &[sketch_call, extrude_call], None)
        .unwrap();

    let extrude_feature = ctx
        .features
        .iter()
        .find(|f| f.id == extrude_id)
        .expect("extrude spine entry");
    // K7.3: the extrude now lowers to a real
    // Primitive::Extrude carrying the Sketch2d profile
    // rather than a rectangular Box stand-in.
    match &extrude_feature.export_primitive().expect("primitive") {
        Primitive::Extrude {
            sketch,
            half_depth_z,
            center: _,
        } => {
            assert!((half_depth_z - 6.0).abs() < 1e-5);
            let aabb = sketch.aabb();
            assert!((aabb.min[0] - (-5.0)).abs() < 1e-5);
            assert!((aabb.max[0] - 15.0).abs() < 1e-5);
            assert!((aabb.min[1] - (-2.0)).abs() < 1e-5);
            assert!((aabb.max[1] - 8.0).abs() < 1e-5);
        }
        other => panic!("expected Primitive::Extrude, got {other:?}"),
    }
}

#[test]
fn extrude_derives_profile_extents_from_a_resolved_circle_sketch() {
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    let sketch_id = id(1);
    let extrude_id = id(2);

    let sketch_call = FeatureCall::new(
        "sketch",
        sketch_id.clone(),
        json!({
            "plane": "qCreatedBy(origin_plane_xy)",
            "sub_calls": [
                {
                    "type": "sketch_circle",
                    "id": "c1",
                    "params": { "center": [2.0, 4.0], "radius": 7.0 }
                }
            ]
        }),
    );
    let extrude_call = FeatureCall::new(
        "extrude",
        extrude_id.clone(),
        json!({
            "profiles": format!("qCreatedBy({sketch_id})"),
            "depth": { "value": 10.0, "unit": "mm" },
            "profile_half_x": 999.0,
            "profile_half_y": 999.0,
        }),
    );
    registry
        .regenerate(&mut ctx, &[sketch_call, extrude_call], None)
        .unwrap();

    let extrude_feature = ctx
        .features
        .iter()
        .find(|f| f.id == extrude_id)
        .expect("extrude spine entry");
    // K7.3: circle sketch no longer lowers to its bounding
    // box — Primitive::Extrude carries the full Sketch2d,
    // so the downstream mesher gets a real cylinder.
    match &extrude_feature.export_primitive().expect("primitive") {
        Primitive::Extrude { sketch, .. } => {
            assert!(matches!(
                sketch,
                ::fielddraft_protocol::Sketch2d::Prim(
                    ::fielddraft_protocol::Sketch2dPrimitive::Circle { .. },
                )
            ));
            let aabb = sketch.aabb();
            // Circle centre (2, 4) radius 7 → aabb extents ±7
            // around the centre.
            assert!((aabb.min[0] - (-5.0)).abs() < 1e-5);
            assert!((aabb.max[0] - 9.0).abs() < 1e-5);
        }
        other => panic!("expected Primitive::Extrude, got {other:?}"),
    }
}

// ─── K8.4: revolve ────────────────────────────────────────

#[test]
fn revolve_lowers_rectangle_sketch_into_primitive_revolve() {
    // Profile: a 4 × 20 mm rectangle at r=5, revolved around the local-Y
    // axis at x=0 (an explicit construction line — ni56 requires one) →
    // a tube.
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    let sketch_id = id(1);
    let revolve_id = id(2);

    let sketch_call = FeatureCall::new(
        "sketch",
        sketch_id.clone(),
        json!({
            "plane": "qCreatedBy(origin_plane_xy)",
            "sub_calls": [
                {
                    "type": "sketch_rectangle",
                    "id": "r1",
                    "params": {
                        "corner_a": [5.0, -10.0],
                        "corner_b": [9.0, 10.0],
                    }
                },
                // Axis line at x=0. The rectangle takes entity ids 1–8
                // (4 corners + 4 edges), so this line is entity 11.
                {
                    "type": "sketch_line",
                    "id": "axis",
                    "params": { "start": [0.0, -10.0], "end": [0.0, 10.0] }
                }
            ]
        }),
    );
    let revolve_call = FeatureCall::new(
        "revolve",
        revolve_id.clone(),
        json!({
            "profiles": format!("qCreatedBy({sketch_id})"),
            "axis": format!("qSketchLine({sketch_id}, 11)"),
        }),
    );
    registry
        .regenerate(&mut ctx, &[sketch_call, revolve_call], None)
        .unwrap();

    let revolve_feature = ctx
        .features
        .iter()
        .find(|f| f.id == revolve_id)
        .expect("revolve spine entry");
    match &revolve_feature.export_primitive().expect("primitive") {
        Primitive::Revolve { sketch, center } => {
            assert_eq!(*center, [0.0, 0.0, 0.0]);
            let aabb = sketch.aabb();
            assert!((aabb.min[0] - 5.0).abs() < 1e-5);
            assert!((aabb.max[0] - 9.0).abs() < 1e-5);
            assert!((aabb.min[1] - (-10.0)).abs() < 1e-5);
            assert!((aabb.max[1] - 10.0).abs() < 1e-5);
        }
        other => panic!("expected Primitive::Revolve, got {other:?}"),
    }
}

#[test]
fn revolve_axis_alignment_maps_axis_line_onto_local_y() {
    // Pure-math check of the rigid 2D alignment + its 3D inverse.
    // A vertical axis at x = 5 must map the profile so x = 5 → x = 0,
    // and the inverse must translate the revolved body back by +5 in x.
    let (m, m_inv) = revolve_axis_alignment([5.0, -10.0], [5.0, 10.0]);
    assert!((m.apply([5.0, 3.0])[0]).abs() < 1e-5, "axis lands on x=0");
    assert!((m.apply([9.0, 3.0])[0] - 4.0).abs() < 1e-5, "radius preserved");
    assert!((m_inv.translate[0] - 5.0).abs() < 1e-5, "inverse shifts back +5 x");
    assert!((m_inv.translate[1]).abs() < 1e-5);

    // A 45° axis through the origin rotates the frame: the axis
    // direction (1,1) must come out along +y (x = 0 for both ends).
    let (m2, _) = revolve_axis_alignment([0.0, 0.0], [1.0, 1.0]);
    assert!((m2.apply([0.0, 0.0])[0]).abs() < 1e-5);
    assert!((m2.apply([1.0, 1.0])[0]).abs() < 1e-5);
}

#[test]
fn revolve_with_sketch_line_axis_realigns_profile() {
    // Same 4 × 20 tube profile at r = 5..9, but the revolve axis is an
    // explicit construction `sketch_line` sitting at x = 5. After
    // alignment the profile's radial extent must run 0..4 (distance
    // from the chosen axis), not 5..9.
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    let sketch_id = id(1);
    let revolve_id = id(2);

    let sketch_call = FeatureCall::new(
        "sketch",
        sketch_id.clone(),
        json!({
            "plane": "qCreatedBy(origin_plane_xy)",
            "sub_calls": [
                {
                    "type": "sketch_rectangle",
                    "id": "r1",
                    "params": { "corner_a": [5.0, -10.0], "corner_b": [9.0, 10.0] }
                },
                {
                    "type": "sketch_line",
                    "id": "10",
                    "params": {
                        "start": [5.0, -10.0],
                        "end": [5.0, 10.0],
                        "construction": true
                    }
                }
            ]
        }),
    );
    let revolve_call = FeatureCall::new(
        "revolve",
        revolve_id.clone(),
        json!({
            "profiles": format!("qCreatedBy({sketch_id})"),
            // pr8a: qSketchLine addresses the LINE ENTITY id. The
            // rectangle takes ids 1–8 (4 corners + 4 edges), so the
            // construction line's entity is 11 (start 9, end 10, line 11).
            "axis": format!("qSketchLine({sketch_id}, 11)"),
        }),
    );
    registry
        .regenerate(&mut ctx, &[sketch_call, revolve_call], None)
        .unwrap();

    let revolve_feature = ctx
        .features
        .iter()
        .find(|f| f.id == revolve_id)
        .expect("revolve spine entry");
    match &revolve_feature.export_primitive().expect("primitive") {
        Primitive::Revolve { sketch, center } => {
            assert_eq!(*center, [0.0, 0.0, 0.0]);
            let aabb = sketch.aabb();
            assert!((aabb.min[0] - 0.0).abs() < 1e-4, "min x = 0, got {}", aabb.min[0]);
            assert!((aabb.max[0] - 4.0).abs() < 1e-4, "max x = 4, got {}", aabb.max[0]);
        }
        other => panic!("expected Primitive::Revolve, got {other:?}"),
    }
}

#[test]
fn revolve_axis_direction_keeps_profile_on_positive_x() {
    // Regression for tickets a204/ifn2: the revolve SDF folds every query to
    // radius √(x²+z²) ≥ 0, so a profile that the axis-alignment lands on the
    // NEGATIVE-x side is never reached → an EMPTY solid (nothing rendered,
    // only B-Rep edges). Here the axis line points DOWNWARD (5,10)→(5,-10),
    // which without the fix flips the +x rectangle (x=6..9) onto −x. The fix
    // reverses the axis direction so the aligned profile stays on +x — the
    // same revolve line, the same solid, a NON-empty SDF.
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    let sketch_id = id(1);
    let revolve_id = id(2);

    let sketch_call = FeatureCall::new(
        "sketch",
        sketch_id.clone(),
        json!({
            "plane": "qCreatedBy(origin_plane_xy)",
            "sub_calls": [
                {
                    "type": "sketch_rectangle",
                    "id": "r1",
                    "params": { "corner_a": [6.0, -8.0], "corner_b": [9.0, 8.0] }
                },
                {
                    "type": "sketch_line",
                    "id": "10",
                    "params": {
                        "start": [5.0, 10.0],
                        "end": [5.0, -10.0],
                        "construction": true
                    }
                }
            ]
        }),
    );
    let revolve_call = FeatureCall::new(
        "revolve",
        revolve_id.clone(),
        json!({
            "profiles": format!("qCreatedBy({sketch_id})"),
            // pr8a: qSketchLine addresses the LINE ENTITY id. The
            // rectangle takes ids 1–8 (4 corners + 4 edges), so the
            // construction line's entity is 11 (start 9, end 10, line 11).
            "axis": format!("qSketchLine({sketch_id}, 11)"),
        }),
    );
    registry
        .regenerate(&mut ctx, &[sketch_call, revolve_call], None)
        .unwrap();

    let revolve_feature = ctx
        .features
        .iter()
        .find(|f| f.id == revolve_id)
        .expect("revolve spine entry");
    match &revolve_feature.export_primitive().expect("primitive") {
        Primitive::Revolve { sketch, .. } => {
            let aabb = sketch.aabb();
            // The aligned profile must be wholly on +x (radius 1..4 from the
            // picked axis), never on −x, regardless of axis direction.
            assert!(
                aabb.min[0] >= -1e-4,
                "aligned profile leaked onto −x (empty-SDF bug): min x = {}",
                aabb.min[0]
            );
            assert!(
                (aabb.min[0] - 1.0).abs() < 1e-3 && (aabb.max[0] - 4.0).abs() < 1e-3,
                "expected radial extent 1..4, got {}..{}",
                aabb.min[0],
                aabb.max[0]
            );
        }
        other => panic!("expected Primitive::Revolve, got {other:?}"),
    }
}

#[test]
fn helix_with_sketch_line_axis_realigns_profile() {
    // A small circular thread profile at r≈5, with an explicit
    // construction `sketch_line` axis at x = 5. After alignment the
    // profile's radial extent must run 0..2 (distance from the chosen
    // axis), proving the helix winds around the picked line.
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    let sketch_id = id(1);
    let helix_id = id(2);

    let sketch_call = FeatureCall::new(
        "sketch",
        sketch_id.clone(),
        json!({
            "plane": "qCreatedBy(origin_plane_xy)",
            "sub_calls": [
                {
                    "type": "sketch_circle",
                    "id": "c1",
                    "params": { "center": [7.0, 0.0], "radius": 1.0 }
                },
                {
                    "type": "sketch_line",
                    "id": "10",
                    "params": {
                        "start": [5.0, -10.0],
                        "end": [5.0, 10.0],
                        "construction": true
                    }
                }
            ]
        }),
    );
    let helix_call = FeatureCall::new(
        "helix",
        helix_id.clone(),
        json!({
            "profiles": format!("qCreatedBy({sketch_id})"),
            // pr8a: qSketchLine now addresses the LINE ENTITY id. The
            // circle (centre + entity) takes ids 1–2, so the construction
            // line's entity is 5 (start 3, end 4, line 5).
            "axis": format!("qSketchLine({sketch_id}, 5)"),
        }),
    );
    registry
        .regenerate(&mut ctx, &[sketch_call, helix_call], None)
        .unwrap();

    let helix_feature = ctx
        .features
        .iter()
        .find(|f| f.id == helix_id)
        .expect("helix spine entry");
    match &helix_feature.export_primitive().expect("primitive") {
        Primitive::Helix { sketch, center, .. } => {
            assert_eq!(*center, [0.0, 0.0, 0.0]);
            let aabb = sketch.aabb();
            // Circle centre was at x=7, axis at x=5 → centre now x=2,
            // radius 1 → extent 1..3.
            assert!((aabb.min[0] - 1.0).abs() < 1e-4, "min x = 1, got {}", aabb.min[0]);
            assert!((aabb.max[0] - 3.0).abs() < 1e-4, "max x = 3, got {}", aabb.max[0]);
        }
        other => panic!("expected Primitive::Helix, got {other:?}"),
    }
}

#[test]
fn revolve_axis_not_coplanar_warns_and_produces_no_body() {
    // Ticket dnfe + ni56: a revolve axis off the profile plane (here a line
    // on the XY plane used as the axis of a YZ-plane revolve — its x≠0
    // endpoints are off the YZ plane) is geometrically invalid → warn AND
    // produce NO body (no implicit local-Y fallback anymore).
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    let profile_sketch = id(1);
    let revolve_id = id(2);
    let axis_sketch = id(3);

    let calls = vec![
        // Profile on the YZ plane (revolve plane), offset from the axis.
        FeatureCall::new("sketch", profile_sketch.clone(), json!({
            "plane": "qCreatedBy(origin_plane_yz)",
            "sub_calls": [
                { "type": "sketch_rectangle", "id": "r1",
                  "params": { "corner_a": [5.0, -10.0], "corner_b": [9.0, 10.0] } }
            ]
        })),
        // Axis line on the XY plane → world x≠0, off the YZ plane.
        FeatureCall::new("sketch", axis_sketch.clone(), json!({
            "plane": "qCreatedBy(origin_plane_xy)",
            "sub_calls": [
                { "type": "sketch_line", "id": "20",
                  "params": { "start": [5.0, -10.0], "end": [5.0, 10.0] } }
            ]
        })),
        FeatureCall::new("revolve", revolve_id.clone(), json!({
            "profiles": format!("qCreatedBy({profile_sketch})"),
            // pr8a: LINE ENTITY id. The axis sketch holds a single line,
            // so its entity is 3 (start 1, end 2, line 3).
            "axis": format!("qSketchLine({axis_sketch}, 3)"),
        })),
    ];
    registry.regenerate(&mut ctx, &calls, None).unwrap();

    assert!(
        ctx.feature_warnings.iter().any(|(fid, msg)| *fid == revolve_id
            && msg.contains("nicht in der Sketch-Ebene")
            && msg.contains("kein Körper")),
        "off-plane revolve axis must warn with 'kein Körper', got {:?}",
        ctx.feature_warnings
    );
    // ni56: produced NO body (the off-plane axis is rejected outright).
    assert!(
        !ctx.features.iter().any(|f| f.id == revolve_id),
        "off-plane revolve axis must yield no body"
    );
}

#[test]
fn revolve_without_axis_is_invalid_and_warns() {
    // Ticket ni56: a revolve with a resolvable profile but NO axis must
    // produce no body and record a clear warning (invalid feature).
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    let sketch_id = id(1);
    let revolve_id = id(2);
    let calls = vec![
        FeatureCall::new("sketch", sketch_id.clone(), json!({
            "plane": "qCreatedBy(origin_plane_xy)",
            "sub_calls": [
                { "type": "sketch_rectangle", "id": "r1",
                  "params": { "corner_a": [5.0, -10.0], "corner_b": [9.0, 10.0] } }
            ]
        })),
        // No axis field → invalid.
        FeatureCall::new("revolve", revolve_id.clone(), json!({
            "profiles": format!("qCreatedBy({sketch_id})"),
        })),
    ];
    registry.regenerate(&mut ctx, &calls, None).unwrap();
    assert!(
        !ctx.features.iter().any(|f| f.id == revolve_id),
        "revolve without an axis must yield no body"
    );
    assert!(
        ctx.feature_warnings.iter().any(|(fid, msg)| *fid == revolve_id
            && msg.contains("keine Achse")),
        "missing-axis revolve must warn, got {:?}",
        ctx.feature_warnings
    );
}

#[test]
fn revolve_without_profile_is_invalid_and_warns() {
    // Ticket ni56: a revolve whose profile query can't be resolved must
    // produce no body and warn (instead of erroring out). This is the
    // empty/just-created revolve case (ticket 1rax).
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    let revolve_id = id(1);
    registry
        .regenerate(
            &mut ctx,
            &[FeatureCall::new("revolve", revolve_id.clone(), json!({
                "profiles": "qCreatedBy(sketch_999)",
            }))],
            None,
        )
        .unwrap();
    assert!(
        !ctx.features.iter().any(|f| f.id == revolve_id),
        "revolve without a profile must yield no body"
    );
    assert!(
        ctx.feature_warnings.iter().any(|(fid, msg)| *fid == revolve_id
            && msg.contains("kein gültiges Profil")),
        "missing-profile revolve must warn, got {:?}",
        ctx.feature_warnings
    );
}

#[test]
fn ifog_triangle_extrude_pipeline_has_all_wall_seams() {
    // Ticket ifog: an oblique-angle (triangle) extrude through the real
    // feature pipeline — loop-inferred Path profile, frame transform,
    // weave_parametric — must produce all 9 edges (3 vertical wall-wall
    // seams + 3 top + 3 bottom). It showed only 6 before the line_point
    // determinant fix dropped every oblique corner seam.
    let registry = FeatureRegistry::with_builtins();
    let calls = vec![
        FeatureCall::new("sketch", id(1), json!({
            "plane": "qCreatedBy(origin_plane_xy)",
            "sub_calls": [
                { "id": "1", "type": "sketch_line", "params": { "start": [-66.25, 384.19], "end": [-30.61, 298.87] } },
                { "id": "2", "type": "sketch_line", "params": { "start": [-30.61, 298.87], "end": [-4.31, 499.70] } },
                { "id": "3", "type": "sketch_line", "params": { "start": [-4.31, 499.70], "end": [-66.25, 384.19] } }
            ]
        })),
        FeatureCall::new("extrude", id(2), json!({
            "profiles": "qCreatedBy(1)", "operation": "new",
            "depth": { "unit": "mm", "value": 240.0 }
        })),
    ];
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    registry.regenerate(&mut ctx, &calls, None).unwrap();
    let woven = ctx.brep_cache.ensure(&ctx.features);
    assert_eq!(woven.len(), 1, "one extrude body");
    let brep = &woven[0].0;
    assert_eq!(brep.face_count(), 5, "3 walls + 2 caps");
    assert_eq!(brep.edge_count(), 9, "3 wall seams + 3 top + 3 bottom");
}

#[test]
fn ifog_hexagon_helix_gets_constructive_edges() {
    // Ticket ifog: a helix produced NO B-Rep edges (its surface is
    // weave-excluded / non-Lipschitz). The constructive swept-wireframe
    // traces the 6 profile corners into helix curves + start/end cap rims:
    // 6 longitudinal + 6 start-rim + 6 end-rim = 18 edges, 12 vertices.
    let registry = FeatureRegistry::with_builtins();
    let calls = vec![
        FeatureCall::new("sketch", id(1), json!({
            "plane": "qCreatedBy(origin_plane_xy)",
            "sub_calls": [
                { "id": "1", "type": "sketch_line", "params": { "start": [458.72, 406.03], "end": [739.01, 398.37] } },
                { "id": "2", "type": "sketch_line", "params": { "start": [739.01, 398.37], "end": [719.85, 126.68] } },
                { "id": "4", "type": "sketch_line", "params": { "start": [719.85, 126.68], "end": [601.60, 297.30] } },
                { "id": "6", "type": "sketch_line", "params": { "start": [601.60, 297.30], "end": [491.20, 216.28] } },
                { "id": "8", "type": "sketch_line", "params": { "start": [491.20, 216.28], "end": [393.08, 295.77] } },
                { "id": "10", "type": "sketch_line", "params": { "start": [393.08, 295.77], "end": [458.72, 406.03] } }
            ]
        })),
        FeatureCall::new("helix", id(2), json!({
            "profiles": "qCreatedBy(1)", "operation": "new",
            "pitch": { "unit": "mm", "value": 1000.0 }, "turns": 1.0
        })),
    ];
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    registry.regenerate(&mut ctx, &calls, None).unwrap();
    let woven = ctx.brep_cache.ensure(&ctx.features);
    assert_eq!(woven.len(), 1, "one helix body");
    let brep = &woven[0].0;
    assert_eq!(brep.edge_count(), 18, "6 longitudinal helix + 6+6 cap rims");
    assert_eq!(brep.vertex_count(), 12, "6 corners × start/end caps");
    // Ticket 1t40: the 6 longitudinal seams are genuine curves → smooth
    // NURBS after the Hermite upgrade; the 12 cap rims stay straight lines.
    let nurbs = brep
        .edges
        .iter()
        .filter(|e| matches!(e.geometry, crate::brep::EdgeGeometry::Nurbs(_)))
        .count();
    assert_eq!(nurbs, 6, "6 helical seams are smooth NURBS curves");
    // Ticket exus: the two open-end caps are now real, PICKABLE analytic
    // plane faces (before, they materialised as empty tessellated faces and
    // couldn't be raycast). The helical walls stay non-analytic (mesh-only).
    let cap_planes = brep
        .faces
        .iter()
        .filter(|f| {
            matches!(
                f.surface,
                crate::brep::SurfaceGeometry::Analytic {
                    surface: ::fielddraft_protocol::HalfSpace::Plane { .. },
                    ..
                }
            )
        })
        .count();
    assert_eq!(cap_planes, 2, "start + end cap are analytic plane faces");
}

#[test]
fn extrude_falls_back_to_profile_half_extents_when_sketch_is_missing() {
    // Query that doesn't point at any context sketch — the
    // executor must fall through to the hand-written
    // fallback extents so the K4.5.8 test surface stays
    // intact.
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    ExtrudeFeature
        .execute(
            &mut ctx,
            &id(1),
            &extrude_def("qCreatedBy(sketch_999)", 10.0, 3.0, 4.0),
        )
        .unwrap();
    let f = &ctx.features[0];
    if let Primitive::Box { half_extents, .. } = &f.export_primitive().expect("primitive") {
        assert_eq!(*half_extents, [3.0, 4.0, 5.0]);
    } else {
        panic!("expected Primitive::Box");
    }
}

/// Regression for the UX-test bug where clicking the Extrude
/// toolbar button on a sketch that only has lines/points
/// crashed with "profile_half_x invalid: must be > 0".
/// When the profiles query resolves to an actually-stored
/// sketch *and* the caller left the legacy fallback fields
/// at 0 (the K7.3 flow), the extrude should return a clean,
/// user-facing error instead of falling through to the
/// profile_half_x positive-guard.
#[test]
fn extrude_returns_clean_error_when_sketch_is_empty_and_no_fallback_dims() {
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    let sketch_id = id(1);
    let extrude_id = id(2);

    let result = registry.regenerate(
        &mut ctx,
        &[
            FeatureCall::new(
                "sketch",
                sketch_id.clone(),
                json!({
                    "plane": "qCreatedBy(origin_plane_xy)",
                    "sub_calls": [
                        {
                            "type": "sketch_line",
                            "id": "l1",
                            "params": {
                                "start": [0.0, 0.0],
                                "end": [10.0, 0.0],
                            }
                        }
                    ]
                }),
            ),
            FeatureCall::new(
                "extrude",
                extrude_id.clone(),
                json!({
                    "profiles": format!("qCreatedBy({sketch_id})"),
                    "depth": { "value": 8.0, "unit": "mm" },
                    "profile_half_x": 0.0,
                    "profile_half_y": 0.0,
                }),
            ),
        ],
        None,
    );
    let err = result.expect_err("extrude must fail on empty sketch with zero fallback");
    match err {
        FeatureError::InvalidParameter { name, message, .. } => {
            assert_eq!(name, "profiles");
            assert!(
                message.contains("closed regions"),
                "expected user-facing message, got {message}"
            );
        }
        other => panic!("expected InvalidParameter, got {other:?}"),
    }
}

#[test]
fn extrude_falls_back_when_sketch_has_no_region_primitives() {
    // Sketch with only a line — build_sketch2d_from_sub_calls
    // returns Sketch2d::Empty, and the resolver falls through.
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    let sketch_id = id(1);
    let extrude_id = id(2);

    registry
        .regenerate(
            &mut ctx,
            &[
                FeatureCall::new(
                    "sketch",
                    sketch_id.clone(),
                    json!({
                        "plane": "qCreatedBy(origin_plane_xy)",
                        "sub_calls": [
                            {
                                "type": "sketch_line",
                                "id": "l1",
                                "params": {
                                    "start": [0.0, 0.0],
                                    "end": [10.0, 0.0],
                                }
                            }
                        ]
                    }),
                ),
                FeatureCall::new(
                    "extrude",
                    extrude_id.clone(),
                    json!({
                        "profiles": format!("qCreatedBy({sketch_id})"),
                        "depth": { "value": 8.0, "unit": "mm" },
                        "profile_half_x": 2.5,
                        "profile_half_y": 2.5,
                    }),
                ),
            ],
            None,
        )
        .unwrap();
    let extrude_feature = ctx
        .features
        .iter()
        .find(|f| f.id == extrude_id)
        .expect("extrude spine entry");
    if let Primitive::Box { half_extents, .. } = &extrude_feature.export_primitive().expect("primitive") {
        assert!((half_extents[0] - 2.5).abs() < 1e-5);
        assert!((half_extents[1] - 2.5).abs() < 1e-5);
    } else {
        panic!("expected Primitive::Box");
    }
}

// ─── C0: mid-loop element-query resolution ─────────────────────

fn box_call(n: u32, half: f32) -> FeatureCall {
    FeatureCall::new(
        "box",
        id(n),
        json!({
            "half_extents": [half, half, half],
        }),
    )
}

#[test]
fn context_resolve_topo_resolves_qface_against_features_so_far() {
    // Build a box, weave it once to learn a face id_hash, then assert
    // Context::resolve_topo (mid-loop) re-resolves that face against
    // the features accumulated so far.
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    registry
        .regenerate(&mut ctx, std::slice::from_ref(&box_call(1, 10.0)), None)
        .unwrap();

    // Learn a real face id_hash from the woven body.
    let woven = crate::body::weave_all(&ctx.features);
    assert_eq!(woven.len(), 1, "one body");
    let (brep, _placement) = &woven[0];
    assert_eq!(brep.face_count(), 6, "a box has 6 faces");
    let want = crate::brep::naming::TopoRef::from_face(brep, crate::brep::FaceId(0));

    let refs = ctx.resolve_topo(&crate::queries::QueryExpr::Face(want.id_hash.into()));
    assert_eq!(refs.len(), 1, "qFace(h) resolves to exactly one face mid-loop");
    assert_eq!(refs[0].id_hash, want.id_hash);
    // qCreatedBy(box) yields all six faces.
    let all = ctx.resolve_topo(&crate::queries::QueryExpr::CreatedBy(id(1)));
    assert_eq!(all.len(), 6, "qCreatedBy(box) yields all six faces");
}

#[test]
fn context_brep_cache_and_scratch_weave_identically() {
    // Shared weave parity: the regeneration Context path and the
    // FieldSkript scratch path both route through body::weave_all, so
    // they must produce identical B-Rep topology for the same spine.
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    registry
        .regenerate(&mut ctx, std::slice::from_ref(&box_call(1, 7.0)), None)
        .unwrap();

    let via_context = ctx.brep_cache.ensure(&ctx.features).to_vec();
    let via_weave_all = crate::body::weave_all(&ctx.features);
    assert_eq!(via_context.len(), via_weave_all.len());
    for ((b_ctx, p_ctx), (b_all, p_all)) in via_context.iter().zip(via_weave_all.iter()) {
        assert_eq!(b_ctx.face_count(), b_all.face_count());
        assert_eq!(b_ctx.edge_count(), b_all.edge_count());
        assert_eq!(b_ctx.vertex_count(), b_all.vertex_count());
        assert_eq!(p_ctx, p_all, "placements match");
    }
}

// ─── Face extrude (P1): B-Rep faces as an extrude source ──────

/// Find the planar face whose world normal points along `want`.
fn planar_face_hash(brep: &crate::brep::BRep, want: [f32; 3]) -> Option<u64> {
    use crate::brep::{FaceId, SurfaceGeometry};
    use fielddraft_protocol::HalfSpace;
    for i in 0..brep.face_count() {
        let fid = FaceId(i as u32);
        if let SurfaceGeometry::Analytic {
            surface: HalfSpace::Plane { normal, .. },
            transform,
        } = &brep.face(fid).surface
        {
            let n = crate::quat_rotate(transform.rotation, *normal);
            if n[0] * want[0] + n[1] * want[1] + n[2] * want[2] > 0.99 {
                return Some(crate::brep::naming::TopoRef::from_face(brep, fid).id_hash);
            }
        }
    }
    None
}

/// Find a curved (non-planar analytic) face — e.g. a cylinder wall.
fn curved_face_hash(brep: &crate::brep::BRep) -> Option<u64> {
    use crate::brep::{FaceId, SurfaceGeometry};
    use fielddraft_protocol::HalfSpace;
    for i in 0..brep.face_count() {
        let fid = FaceId(i as u32);
        match &brep.face(fid).surface {
            SurfaceGeometry::Analytic { surface: HalfSpace::Plane { .. }, .. } => {}
            _ => return Some(crate::brep::naming::TopoRef::from_face(brep, fid).id_hash),
        }
    }
    None
}

/// Does any leaf `Path` of the sketch tree carry an analytic `Arc`?
fn has_arc(s: &::fielddraft_protocol::Sketch2d) -> bool {
    use ::fielddraft_protocol::{Sketch2d as S, Sketch2dPrimitive as P, SketchSegment as Seg};
    match s {
        S::Prim(P::Path { segments, .. }) => {
            segments.iter().any(|seg| matches!(seg, Seg::Arc { .. }))
        }
        S::Prim(_) | S::Empty => false,
        S::Union(a, b) | S::Difference(a, b) | S::Intersection(a, b) => {
            has_arc(a) || has_arc(b)
        }
        S::Offset { inner, .. } => has_arc(inner),
    }
}

fn weave_first(ctx: &mut Context<'_>) -> crate::brep::BRep {
    crate::body::weave_all(&ctx.features)[0].0.clone()
}

#[test]
fn extrude_box_top_face_builds_blind_square_slab() {
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    registry
        .regenerate(&mut ctx, std::slice::from_ref(&box_call(1, 10.0)), None)
        .unwrap();
    let brep = weave_first(&mut ctx);
    let hash = planar_face_hash(&brep, [0.0, 0.0, 1.0]).expect("a +Z face");

    let mut doc2 = empty_ctx();
    let mut ctx2 = Context::new(&mut doc2);
    registry
        .regenerate(
            &mut ctx2,
            &[
                box_call(1, 10.0),
                FeatureCall::new(
                    "extrude",
                    id(2),
                    json!({
                        "profiles": format!("qFace({hash})"),
                        "depth": { "value": 5.0, "unit": "mm" },
                        "symmetric": false,
                    }),
                ),
            ],
            None,
        )
        .unwrap();

    let ex = ctx2.features.iter().find(|f| f.id == id(2)).expect("extrude");
    let prim = ex.export_primitive().expect("primitive");
    let Primitive::Extrude { sketch, half_depth_z, center } = prim else {
        panic!("expected Primitive::Extrude, got {prim:?}");
    };
    // Blind 5 mm grows [0, 5] along the face normal → half 2.5, centre +2.5.
    assert!((half_depth_z - 2.5).abs() < 1e-4, "half {half_depth_z}");
    assert!((center[2] - 2.5).abs() < 1e-4, "centre z {}", center[2]);
    // The 20×20 top face projects to a [-10, 10]² profile.
    let bb = sketch.aabb();
    assert!((bb.min[0] + 10.0).abs() < 1e-2 && (bb.max[0] - 10.0).abs() < 1e-2, "aabb {bb:?}");
    assert!((bb.min[1] + 10.0).abs() < 1e-2 && (bb.max[1] - 10.0).abs() < 1e-2, "aabb {bb:?}");
    assert!(sketch.sdf([0.0, 0.0]) < 0.0, "centre inside the profile");
    assert!(sketch.sdf([30.0, 0.0]) > 0.0, "far point outside");
}

#[test]
fn extrude_routes_qface_profiles_to_face_extrude() {
    // Ticket 20fc: a planar `qFace(...)` placed in the `profiles` field (the
    // natural "pick a side face as the profile" gesture) must extrude that
    // face, not fail as a non-resolvable sketch.
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    registry
        .regenerate(&mut ctx, std::slice::from_ref(&box_call(1, 10.0)), None)
        .unwrap();
    let brep = weave_first(&mut ctx);
    let hash = planar_face_hash(&brep, [0.0, 0.0, 1.0]).expect("a +Z face");

    let mut doc2 = empty_ctx();
    let mut ctx2 = Context::new(&mut doc2);
    registry
        .regenerate(
            &mut ctx2,
            &[
                box_call(1, 10.0),
                FeatureCall::new(
                    "extrude",
                    id(2),
                    json!({
                        "profiles": format!("qFace({hash})"),
                        "depth": { "value": 5.0, "unit": "mm" },
                    }),
                ),
            ],
            None,
        )
        .unwrap();
    // A failed feature is skipped (no geometry), so finding the extrude with
    // a real Extrude primitive proves the qFace profile routed correctly.
    let ex = ctx2.features.iter().find(|f| f.id == id(2)).expect("extrude built");
    let Primitive::Extrude { sketch, .. } = ex.export_primitive().expect("primitive") else {
        panic!("expected a face-profile Extrude");
    };
    let bb = sketch.aabb();
    assert!((bb.max[0] - 10.0).abs() < 1e-2, "20mm top face profile, aabb {bb:?}");
}

#[test]
fn extrude_cylinder_cap_builds_circular_arc_profile() {
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    let cyl = FeatureCall::new(
        "cylinder",
        id(1),
        json!({ "radius": 3.0, "half_height": 4.0 }),
    );
    registry
        .regenerate(&mut ctx, std::slice::from_ref(&cyl), None)
        .unwrap();
    let brep = weave_first(&mut ctx);
    // A cylinder's cap normal is ±Y; take whichever exists.
    let hash = planar_face_hash(&brep, [0.0, 1.0, 0.0])
        .or_else(|| planar_face_hash(&brep, [0.0, -1.0, 0.0]))
        .expect("a cap face");

    let mut doc2 = empty_ctx();
    let mut ctx2 = Context::new(&mut doc2);
    registry
        .regenerate(
            &mut ctx2,
            &[
                cyl.clone(),
                FeatureCall::new(
                    "extrude",
                    id(2),
                    json!({
                        "profiles": format!("qFace({hash})"),
                        "depth": { "value": 2.0, "unit": "mm" },
                        "symmetric": false,
                    }),
                ),
            ],
            None,
        )
        .unwrap();
    let ex = ctx2.features.iter().find(|f| f.id == id(2)).expect("extrude");
    let Primitive::Extrude { sketch, .. } = ex.export_primitive().expect("primitive") else {
        panic!("expected Primitive::Extrude");
    };
    // The cap rim is one closed circular edge → an exact Arc, not a polygon.
    assert!(has_arc(&sketch), "circular cap should yield an analytic Arc");
    let bb = sketch.aabb();
    let (wx, wy) = (bb.max[0] - bb.min[0], bb.max[1] - bb.min[1]);
    assert!((wx - wy).abs() < 1e-2 * wx.max(1.0), "round profile: {wx} vs {wy}");
    assert!(((bb.min[0] + bb.max[0]) * 0.5).abs() < 1e-2, "centred in u");
    assert!(sketch.sdf([0.0, 0.0]) < 0.0, "centre inside the disc");
}

#[test]
fn extrude_faces_rejects_non_coplanar_selection() {
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    registry
        .regenerate(&mut ctx, std::slice::from_ref(&box_call(1, 10.0)), None)
        .unwrap();
    let brep = weave_first(&mut ctx);
    let top = planar_face_hash(&brep, [0.0, 0.0, 1.0]).unwrap();
    let side = planar_face_hash(&brep, [1.0, 0.0, 0.0]).unwrap();

    let err = ctx
        .resolve_extrude_faces(&[format!("qFace({top})"), format!("qFace({side})")])
        .unwrap_err();
    assert!(err.contains("coplanar"), "got: {err}");
}

/// Endpoints of an analytic profile segment (for ring-connectivity checks).
fn seg_endpoints(seg: &::fielddraft_protocol::SketchSegment) -> ([f32; 2], [f32; 2]) {
    use ::fielddraft_protocol::SketchSegment as Seg;
    match seg {
        Seg::Line { start, end } => (*start, *end),
        Seg::QuadBezier { p0, p2, .. } => (*p0, *p2),
        Seg::Arc { center, radius, start_angle, sweep_angle } => {
            let e = start_angle + sweep_angle;
            (
                [center[0] + radius * start_angle.cos(), center[1] + radius * start_angle.sin()],
                [center[0] + radius * e.cos(), center[1] + radius * e.sin()],
            )
        }
    }
}

/// Assert every `Path` leaf of the region is a CONNECTED closed ring:
/// each segment starts where the previous one ended, and the last ends
/// at the first's start. A shuffled loop produces a self-intersecting
/// "bowtie" profile (ticket oa9h).
fn assert_paths_connected(s: &::fielddraft_protocol::Sketch2d) {
    use ::fielddraft_protocol::{Sketch2d as S, Sketch2dPrimitive as P};
    match s {
        S::Prim(P::Path { segments, closed }) => {
            assert!(*closed, "face profile paths must be closed");
            assert!(!segments.is_empty());
            let tol = 1e-2_f32;
            let (first_start, mut prev_end) = seg_endpoints(&segments[0]);
            for seg in &segments[1..] {
                let (start, end) = seg_endpoints(seg);
                let d = ((start[0] - prev_end[0]).powi(2) + (start[1] - prev_end[1]).powi(2)).sqrt();
                assert!(d < tol, "segment chain broken: jump of {d} at {start:?} (prev end {prev_end:?})");
                prev_end = end;
            }
            let d = ((first_start[0] - prev_end[0]).powi(2) + (first_start[1] - prev_end[1]).powi(2)).sqrt();
            assert!(d < tol, "ring not closed: gap of {d}");
        }
        S::Prim(_) | S::Empty => {}
        S::Union(a, b) | S::Difference(a, b) | S::Intersection(a, b) => {
            assert_paths_connected(a);
            assert_paths_connected(b);
        }
        S::Offset { inner, .. } => assert_paths_connected(inner),
    }
}

/// Find the planar face with world normal `want` OWNED by feature `owner`.
fn planar_face_hash_of(
    brep: &crate::brep::BRep,
    want: [f32; 3],
    owner: u32,
) -> Option<u64> {
    use crate::brep::{FaceId, SurfaceGeometry};
    use fielddraft_protocol::HalfSpace;
    for i in 0..brep.face_count() {
        let fid = FaceId(i as u32);
        if brep.face(fid).branch_id.feature_id() != owner {
            continue;
        }
        if let SurfaceGeometry::Analytic {
            surface: HalfSpace::Plane { normal, .. },
            transform,
        } = &brep.face(fid).surface
        {
            let n = crate::quat_rotate(transform.rotation, *normal);
            if n[0] * want[0] + n[1] * want[1] + n[2] * want[2] > 0.99 {
                return Some(crate::brep::naming::TopoRef::from_face(brep, fid).id_hash);
            }
        }
    }
    None
}

#[test]
fn extrude_box_side_face_after_top_extrude_keeps_connected_profile() {
    // Ticket oa9h: box, extrude the TOP face, then extrude a SIDE face of
    // the original box. The side face's boundary must project to a
    // connected 20×20 ring — a shuffled loop extrudes a twisted bowtie.
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    registry
        .regenerate(&mut ctx, std::slice::from_ref(&box_call(1, 10.0)), None)
        .unwrap();
    let brep = weave_first(&mut ctx);
    let top = planar_face_hash(&brep, [0.0, 0.0, 1.0]).expect("a +Z face");

    let mut doc2 = empty_ctx();
    let mut ctx2 = Context::new(&mut doc2);
    registry
        .regenerate(
            &mut ctx2,
            &[
                box_call(1, 10.0),
                FeatureCall::new(
                    "extrude",
                    id(2),
                    json!({
                        "profiles": format!("qFace({top})"),
                        "depth": { "value": 5.0, "unit": "mm" },
                        "symmetric": false,
                    }),
                ),
            ],
            None,
        )
        .unwrap();

    let brep2 = weave_first(&mut ctx2);
    let side =
        planar_face_hash_of(&brep2, [1.0, 0.0, 0.0], 1).expect("the box's +X side face");

    let (region, _frame) =
        ctx2.resolve_extrude_faces(&[format!("qFace({side})")]).expect("resolvable side face");
    assert_paths_connected(&region);
    let bb = region.aabb();
    let (w, h) = (bb.max[0] - bb.min[0], bb.max[1] - bb.min[1]);
    assert!((w - 20.0).abs() < 0.2, "side profile width {w} (aabb {bb:?})");
    assert!((h - 20.0).abs() < 0.2, "side profile height {h} (aabb {bb:?})");
    let c = [(bb.min[0] + bb.max[0]) * 0.5, (bb.min[1] + bb.max[1]) * 0.5];
    assert!(region.sdf(c) < 0.0, "profile centre must be inside");
}

#[test]
fn extrude_faces_rejects_curved_face() {
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    registry
        .regenerate(
            &mut ctx,
            std::slice::from_ref(&FeatureCall::new(
                "cylinder",
                id(1),
                json!({ "radius": 3.0, "half_height": 4.0 }),
            )),
            None,
        )
        .unwrap();
    let brep = weave_first(&mut ctx);
    let wall = curved_face_hash(&brep).expect("a curved cylinder wall face");

    let err = ctx.resolve_extrude_faces(&[format!("qFace({wall})")]).unwrap_err();
    assert!(
        err.contains("planar") || err.contains("curved"),
        "got: {err}"
    );
}

#[test]
fn extrude_direction_modes_set_half_and_centre() {
    let registry = FeatureRegistry::with_builtins();
    // Learn the box's top-face handle once.
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    registry
        .regenerate(&mut ctx, std::slice::from_ref(&box_call(1, 10.0)), None)
        .unwrap();
    let brep = weave_first(&mut ctx);
    let face_q = format!("qFace({})", planar_face_hash(&brep, [0.0, 0.0, 1.0]).unwrap());

    // Each mode in its own fresh regen (a second extrude would bury the
    // first's top face, so they cannot share a spine).
    let half_centre = |extra: serde_json::Value| -> (f32, f32) {
        let mut params = json!({
            "profiles": face_q,
            "depth": { "value": 6.0, "unit": "mm" },
        });
        let obj = params.as_object_mut().unwrap();
        for (k, v) in extra.as_object().unwrap() {
            obj.insert(k.clone(), v.clone());
        }
        let mut d = empty_ctx();
        let mut c = Context::new(&mut d);
        registry
            .regenerate(
                &mut c,
                &[box_call(1, 10.0), FeatureCall::new("extrude", id(2), params)],
                None,
            )
            .unwrap();
        let ex = c.features.iter().find(|f| f.id == id(2)).unwrap();
        match ex.export_primitive().unwrap() {
            Primitive::Extrude { half_depth_z, center, .. } => (half_depth_z, center[2]),
            p => panic!("expected Extrude, got {p:?}"),
        }
    };
    assert_eq!(half_centre(json!({ "symmetric": true })), (3.0, 0.0), "symmetric");
    assert_eq!(half_centre(json!({ "symmetric": false })), (3.0, 3.0), "blind");
    assert_eq!(
        half_centre(json!({ "symmetric": false, "depth_back": { "value": 2.0, "unit": "mm" } })),
        (4.0, 2.0),
        "two-sided"
    );
}

// ─── Thicken: curved B-Rep faces as a thin-wall source ────────

/// First analytic face whose [`HalfSpace`] satisfies `pred`, by id_hash.
fn analytic_face_hash(
    brep: &crate::brep::BRep,
    pred: impl Fn(&fielddraft_protocol::HalfSpace) -> bool,
) -> Option<u64> {
    use crate::brep::{FaceId, SurfaceGeometry};
    for i in 0..brep.face_count() {
        let fid = FaceId(i as u32);
        if let SurfaceGeometry::Analytic { surface, .. } = &brep.face(fid).surface {
            if pred(surface) {
                return Some(crate::brep::naming::TopoRef::from_face(brep, fid).id_hash);
            }
        }
    }
    None
}

/// Pull the `(outer, inner)` offset primitives out of a single-face thicken
/// node — `Difference(Leaf(outer), Leaf(inner))`.
fn thicken_offset_pair(node: &CsgNode) -> (Primitive, Primitive) {
    let leaf = |n: &CsgNode| match n {
        CsgNode::Leaf { primitive, .. } => primitive.clone(),
        other => panic!("expected Leaf, got {other:?}"),
    };
    match node {
        CsgNode::Difference(o, i) => (leaf(o), leaf(i)),
        other => panic!("expected Difference, got {other:?}"),
    }
}

#[test]
fn thicken_sphere_face_builds_concentric_shell() {
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    let sphere = FeatureCall::new("sphere", id(1), json!({ "radius": 5.0 }));
    registry
        .regenerate(&mut ctx, std::slice::from_ref(&sphere), None)
        .unwrap();
    let brep = weave_first(&mut ctx);
    let hash = analytic_face_hash(&brep, |s| {
        matches!(s, fielddraft_protocol::HalfSpace::Sphere { .. })
    })
    .expect("a spherical face");
    // Read the real surface radius so the assertion is scale-agnostic.
    let r = match &brep.face(
        crate::brep::FaceId(
            (0..brep.face_count())
                .find(|&i| {
                    crate::brep::naming::TopoRef::from_face(&brep, crate::brep::FaceId(i as u32))
                        .id_hash
                        == hash
                })
                .unwrap() as u32,
        ),
    )
    .surface
    {
        crate::brep::SurfaceGeometry::Analytic {
            surface: fielddraft_protocol::HalfSpace::Sphere { radius },
            ..
        } => *radius,
        other => panic!("not a sphere face: {other:?}"),
    };

    let mut doc2 = empty_ctx();
    let mut ctx2 = Context::new(&mut doc2);
    registry
        .regenerate(
            &mut ctx2,
            &[
                sphere.clone(),
                FeatureCall::new(
                    "thicken",
                    id(2),
                    json!({
                        "faces": format!("qFace({hash})"),
                        "thickness": { "value": 1.0, "unit": "mm" },
                        "symmetric": false,
                    }),
                ),
            ],
            None,
        )
        .unwrap();
    let th = ctx2.features.iter().find(|f| f.id == id(2)).expect("thicken");
    let (outer, inner) = thicken_offset_pair(&th.node);
    match (outer, inner) {
        (
            Primitive::Sphere { radius: ro, .. },
            Primitive::Sphere { radius: ri, .. },
        ) => {
            assert!((ro - (r + 1.0)).abs() < 1e-3, "outer {ro} vs {}", r + 1.0);
            assert!((ri - r).abs() < 1e-3, "inner {ri} vs {r}");
        }
        other => panic!("expected two spheres, got {other:?}"),
    }
}

#[test]
fn thicken_cylinder_wall_builds_bounded_tube() {
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    let cyl = FeatureCall::new(
        "cylinder",
        id(1),
        json!({ "radius": 3.0, "half_height": 4.0 }),
    );
    registry
        .regenerate(&mut ctx, std::slice::from_ref(&cyl), None)
        .unwrap();
    let brep = weave_first(&mut ctx);
    let hash = analytic_face_hash(&brep, |s| {
        matches!(s, fielddraft_protocol::HalfSpace::InfiniteCylinder { .. })
    })
    .expect("a cylinder wall face");

    let mut doc2 = empty_ctx();
    let mut ctx2 = Context::new(&mut doc2);
    registry
        .regenerate(
            &mut ctx2,
            &[
                cyl.clone(),
                FeatureCall::new(
                    "thicken",
                    id(2),
                    json!({
                        "faces": format!("qFace({hash})"),
                        "thickness": { "value": 1.0, "unit": "mm" },
                        "symmetric": false,
                    }),
                ),
            ],
            None,
        )
        .unwrap();
    let th = ctx2.features.iter().find(|f| f.id == id(2)).expect("thicken");
    let (outer, inner) = thicken_offset_pair(&th.node);
    match (outer, inner) {
        (
            Primitive::Cylinder { radius: ro, half_height: ho, center: co },
            Primitive::Cylinder { radius: ri, half_height: hi, center: ci },
        ) => {
            assert!((ro - 4.0).abs() < 1e-2, "outer r {ro}");
            assert!((ri - 3.0).abs() < 1e-2, "inner r {ri}");
            // Axial extent recovered from the ±4 cap rims → centred, half 4.
            assert!((ho - 4.0).abs() < 1e-2 && (hi - 4.0).abs() < 1e-2, "half {ho}/{hi}");
            assert!(co[1].abs() < 1e-2 && ci[1].abs() < 1e-2, "centred axially");
        }
        other => panic!("expected two cylinders, got {other:?}"),
    }
}

#[test]
fn thicken_torus_face_offsets_the_minor_radius() {
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    let torus = FeatureCall::new(
        "torus",
        id(1),
        json!({ "major_radius": 10.0, "minor_radius": 2.0 }),
    );
    registry
        .regenerate(&mut ctx, std::slice::from_ref(&torus), None)
        .unwrap();
    let brep = weave_first(&mut ctx);
    let hash = analytic_face_hash(&brep, |s| {
        matches!(s, fielddraft_protocol::HalfSpace::Torus { .. })
    })
    .expect("a toroidal face");

    let mut doc2 = empty_ctx();
    let mut ctx2 = Context::new(&mut doc2);
    registry
        .regenerate(
            &mut ctx2,
            &[
                torus.clone(),
                FeatureCall::new(
                    "thicken",
                    id(2),
                    json!({
                        "faces": format!("qFace({hash})"),
                        "thickness": { "value": 0.5, "unit": "mm" },
                        "symmetric": false,
                    }),
                ),
            ],
            None,
        )
        .unwrap();
    let th = ctx2.features.iter().find(|f| f.id == id(2)).expect("thicken");
    let (outer, inner) = thicken_offset_pair(&th.node);
    match (outer, inner) {
        (
            Primitive::Torus { major_radius: ro_maj, minor_radius: ro_min, .. },
            Primitive::Torus { major_radius: ri_maj, minor_radius: ri_min, .. },
        ) => {
            assert!((ro_maj - 10.0).abs() < 1e-3 && (ri_maj - 10.0).abs() < 1e-3, "major fixed");
            assert!((ro_min - 2.5).abs() < 1e-3, "outer minor {ro_min}");
            assert!((ri_min - 2.0).abs() < 1e-3, "inner minor {ri_min}");
        }
        other => panic!("expected two tori, got {other:?}"),
    }
}

#[test]
fn thicken_rejects_planar_face_pointing_at_extrude() {
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    registry
        .regenerate(&mut ctx, std::slice::from_ref(&box_call(1, 10.0)), None)
        .unwrap();
    let brep = weave_first(&mut ctx);
    let top = planar_face_hash(&brep, [0.0, 0.0, 1.0]).unwrap();

    let err = ctx
        .resolve_thicken_faces(&id(2), &[format!("qFace({top})")], 2.0, 0.0)
        .unwrap_err();
    assert!(err.contains("planar") || err.contains("Extrude"), "got: {err}");
}

#[test]
fn thicken_cone_face_builds_offset_frustums() {
    // DL-76: the cone wall now weaves as an apex-correct analytic Cone with
    // two rims, so thicken reconstructs the frustum and offsets it. A 2 mm
    // *normal* (wall) thickness is a *radial* shift Δ = 2/cos α of both rims.
    // cone r_bottom=5 @ y=−6, r_top=2 @ y=+6, hh=6 ⇒ α = atan(3/12).
    let registry = FeatureRegistry::with_builtins();
    let cone = FeatureCall::new(
        "cone",
        id(1),
        json!({ "r_bottom": 5.0, "r_top": 2.0, "half_height": 6.0 }),
    );
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    registry
        .regenerate(&mut ctx, std::slice::from_ref(&cone), None)
        .unwrap();
    let brep = weave_first(&mut ctx);
    let hash = analytic_face_hash(&brep, |s| {
        matches!(s, fielddraft_protocol::HalfSpace::Cone { .. })
    })
    .expect("an apex-correct conical wall face (DL-76)");

    let mut doc2 = empty_ctx();
    let mut ctx2 = Context::new(&mut doc2);
    registry
        .regenerate(
            &mut ctx2,
            &[
                cone.clone(),
                FeatureCall::new(
                    "thicken",
                    id(2),
                    json!({
                        "faces": format!("qFace({hash})"),
                        "thickness": { "value": 2.0, "unit": "mm" },
                        "symmetric": false,
                    }),
                ),
            ],
            None,
        )
        .unwrap();
    let th = ctx2.features.iter().find(|f| f.id == id(2)).expect("thicken");
    let (outer, inner) = thicken_offset_pair(&th.node);
    let d = 2.0 / (0.25_f32.atan()).cos(); // radial shift for a 2 mm normal wall
    match (outer, inner) {
        (
            Primitive::Cone { r_bottom: rbo, r_top: rto, half_height: hho },
            Primitive::Cone { r_bottom: rbi, r_top: rti, half_height: hhi },
        ) => {
            // Outer frustum: both rims grown by the radial shift; same taper.
            assert!((rbo - (5.0 + d)).abs() < 0.15, "outer r_bottom {rbo}");
            assert!((rto - (2.0 + d)).abs() < 0.15, "outer r_top {rto}");
            // Inner frustum: thickness_back = 0 → the original rims.
            assert!((rbi - 5.0).abs() < 0.15, "inner r_bottom {rbi}");
            assert!((rti - 2.0).abs() < 0.15, "inner r_top {rti}");
            // Axial span recovered from the rims.
            assert!((hho - 6.0).abs() < 0.1 && (hhi - 6.0).abs() < 0.1, "half {hho}/{hhi}");
        }
        other => panic!("expected two frustums, got {other:?}"),
    }
    // The axial centre folds into the feature placement: apex (y≈14) plus the
    // frustum's apex-local centre (y≈−14) lands the origin-centred frustums
    // back at the body origin (y≈0).
    assert!(
        th.transform.translate[1].abs() < 0.4,
        "frustum re-centred at body origin, got y={}",
        th.transform.translate[1]
    );
}

#[test]
fn thicken_uniform_scale_renormalises_offset_to_world_mm() {
    // A 2× uniformly-scaled body: a 2 mm *world* wall is a 1 mm *local* shift,
    // so the displayed thickness stays 2 mm whatever the body scale. Without
    // the renormalisation a scaled body would over-thicken by the scale factor.
    use crate::brep::{FaceId, SurfaceGeometry};
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    let sphere = FeatureCall::new("sphere", id(1), json!({ "radius": 5.0 }));
    registry
        .regenerate(&mut ctx, std::slice::from_ref(&sphere), None)
        .unwrap();
    let brep = weave_first(&mut ctx);
    let (fid, r) = (0..brep.face_count())
        .find_map(|i| {
            let fid = FaceId(i as u32);
            match &brep.face(fid).surface {
                SurfaceGeometry::Analytic {
                    surface: fielddraft_protocol::HalfSpace::Sphere { radius },
                    ..
                } => Some((fid, *radius)),
                _ => None,
            }
        })
        .expect("a spherical face");

    let scaled = Transform { scale: [2.0, 2.0, 2.0], ..Transform::IDENTITY };
    let solid =
        crate::brep::face_thicken::thicken_face(&brep, fid, &scaled, 2.0, 0.0).unwrap();
    match (solid.outer, solid.inner) {
        (
            Primitive::Sphere { radius: ro, .. },
            Primitive::Sphere { radius: ri, .. },
        ) => {
            // world 2 mm / scale 2 = 1 mm local.
            assert!((ro - (r + 1.0)).abs() < 1e-3, "outer {ro} vs {}", r + 1.0);
            assert!((ri - r).abs() < 1e-3, "inner {ri} vs {r}");
        }
        other => panic!("expected two spheres, got {other:?}"),
    }
    assert!((solid.world.scale[0] - 2.0).abs() < 1e-4, "scale stays in placement");
}

#[test]
fn thicken_rejects_non_uniform_scale() {
    use crate::brep::{FaceId, SurfaceGeometry};
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    let sphere = FeatureCall::new("sphere", id(1), json!({ "radius": 5.0 }));
    registry
        .regenerate(&mut ctx, std::slice::from_ref(&sphere), None)
        .unwrap();
    let brep = weave_first(&mut ctx);
    let fid = (0..brep.face_count())
        .map(|i| FaceId(i as u32))
        .find(|&f| {
            matches!(
                &brep.face(f).surface,
                SurfaceGeometry::Analytic {
                    surface: fielddraft_protocol::HalfSpace::Sphere { .. },
                    ..
                }
            )
        })
        .expect("a spherical face");
    // Anisotropic scale → the offset surface is an ellipsoid shell, not a
    // sphere: thicken must refuse rather than emit wrong geometry.
    let aniso = Transform { scale: [2.0, 1.0, 1.0], ..Transform::IDENTITY };
    let err = crate::brep::face_thicken::thicken_face(&brep, fid, &aniso, 2.0, 0.0)
        .unwrap_err();
    assert!(err.contains("non-uniform"), "got: {err}");
}

/// Weave a single solid sphere and return its `(brep, spherical FaceId,
/// surface radius)` — a one-face body to drive the up-to-face geometry tests.
#[cfg(test)]
fn weave_sphere_face(registry: &FeatureRegistry, r: f32) -> (crate::brep::BRep, crate::brep::FaceId, f32) {
    use crate::brep::{FaceId, SurfaceGeometry};
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    let s = FeatureCall::new("sphere", id(1), json!({ "radius": r }));
    registry.regenerate(&mut ctx, std::slice::from_ref(&s), None).unwrap();
    let brep = weave_first(&mut ctx);
    let (fid, rad) = (0..brep.face_count())
        .find_map(|i| {
            let fid = FaceId(i as u32);
            match &brep.face(fid).surface {
                SurfaceGeometry::Analytic {
                    surface: fielddraft_protocol::HalfSpace::Sphere { radius },
                    ..
                } => Some((fid, *radius)),
                _ => None,
            }
        })
        .expect("a spherical face");
    (brep, fid, rad)
}

#[test]
fn offset_to_target_grows_source_up_to_coaxial_target() {
    // Two independent, concentric sphere bodies (radius 5 and 7). Thickening
    // the small one up to the big one is a 2 mm outward wall; the reverse is a
    // 2 mm inward wall. The signed gap picks the direction.
    let registry = FeatureRegistry::with_builtins();
    let (src, sfid, sr) = weave_sphere_face(&registry, 5.0);
    let (tgt, tfid, tr) = weave_sphere_face(&registry, 7.0);
    let idt = Transform::IDENTITY;

    let (oo, io) =
        crate::brep::face_thicken::offset_to_target(&src, sfid, &idt, &tgt, tfid, &idt).unwrap();
    assert!((oo - (tr - sr)).abs() < 1e-3 && io.abs() < 1e-6, "outward {oo}/{io}");

    let (oo2, io2) =
        crate::brep::face_thicken::offset_to_target(&tgt, tfid, &idt, &src, sfid, &idt).unwrap();
    assert!(oo2.abs() < 1e-6 && (io2 - (tr - sr)).abs() < 1e-3, "inward {oo2}/{io2}");
}

#[test]
fn offset_to_target_renormalises_target_scale() {
    // Target on a 2×-scaled body: its world radius is 7 × 2 = 14, so a source
    // sphere of world radius 5 grows by a 9 mm *world* wall — the offset is in
    // world millimetres regardless of either body's scale.
    let registry = FeatureRegistry::with_builtins();
    let (src, sfid, sr) = weave_sphere_face(&registry, 5.0);
    let (tgt, tfid, tr) = weave_sphere_face(&registry, 7.0);
    let idt = Transform::IDENTITY;
    let scaled = Transform { scale: [2.0, 2.0, 2.0], ..Transform::IDENTITY };

    let (oo, io) = crate::brep::face_thicken::offset_to_target(
        &src, sfid, &idt, &tgt, tfid, &scaled,
    )
    .unwrap();
    assert!((oo - (tr * 2.0 - sr)).abs() < 1e-3 && io.abs() < 1e-6, "outward {oo}/{io}");
}

#[test]
fn offset_to_target_rejects_non_coaxial_and_mismatched_kinds() {
    let registry = FeatureRegistry::with_builtins();
    let (sbrep, sfid, _) = weave_sphere_face(&registry, 5.0);

    // A cylinder body, for a type-mismatched target.
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    registry
        .regenerate(
            &mut ctx,
            std::slice::from_ref(&FeatureCall::new(
                "cylinder",
                id(1),
                json!({ "radius": 3.0, "half_height": 4.0 }),
            )),
            None,
        )
        .unwrap();
    let cbrep = weave_first(&mut ctx);
    let cfid = (0..cbrep.face_count())
        .map(|i| crate::brep::FaceId(i as u32))
        .find(|&f| {
            matches!(
                &cbrep.face(f).surface,
                crate::brep::SurfaceGeometry::Analytic {
                    surface: fielddraft_protocol::HalfSpace::InfiniteCylinder { .. },
                    ..
                }
            )
        })
        .expect("a cylinder wall face");
    let idt = Transform::IDENTITY;

    // Sphere source, cylinder target → different kinds.
    let err = crate::brep::face_thicken::offset_to_target(&sbrep, sfid, &idt, &cbrep, cfid, &idt)
        .unwrap_err();
    assert!(err.contains("coaxial analytic kind") || err.contains("same"), "kind: {err}");

    // Same sphere, but the target placement is shifted off-centre → not concentric.
    let shifted = Transform { translate: [3.0, 0.0, 0.0], ..Transform::IDENTITY };
    let err2 = crate::brep::face_thicken::offset_to_target(&sbrep, sfid, &idt, &sbrep, sfid, &shifted)
        .unwrap_err();
    assert!(err2.contains("concentric") || err2.contains("coaxial"), "coaxial: {err2}");
}

#[test]
fn thicken_up_to_face_fills_a_tube_wall() {
    // A tube (cylinder r=5 minus a coaxial r=3 bore) has two coaxial analytic
    // cylinder faces in one body. Thickening the bore *up to* the outer wall
    // grows the wall from r=3 out to r=5 → the offset pair Cyl(5) − Cyl(3).
    use crate::brep::{naming::TopoRef, FaceId, SurfaceGeometry};
    let registry = FeatureRegistry::with_builtins();
    let outer = FeatureCall::new("cylinder", id(1), json!({ "radius": 5.0, "half_height": 4.0 }));
    let bore = FeatureCall::new(
        "cylinder",
        id(2),
        json!({ "radius": 3.0, "half_height": 5.0, "op": "remove", "targets": "qCreatedBy(1)" }),
    );
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    registry.regenerate(&mut ctx, &[outer.clone(), bore.clone()], None).unwrap();
    let brep = weave_first(&mut ctx);

    // The two coaxial cylinder faces, keyed by surface radius.
    let cyl_face = |target_r: f32| -> String {
        (0..brep.face_count())
            .find_map(|i| {
                let fid = FaceId(i as u32);
                match &brep.face(fid).surface {
                    SurfaceGeometry::Analytic {
                        surface: fielddraft_protocol::HalfSpace::InfiniteCylinder { radius },
                        ..
                    } if (*radius - target_r).abs() < 0.2 => {
                        Some(format!("qFace({})", TopoRef::from_face(&brep, fid).id_hash))
                    }
                    _ => None,
                }
            })
            .unwrap_or_else(|| panic!("no cylinder face at radius {target_r}"))
    };
    let bore_q = cyl_face(3.0);
    let wall_q = cyl_face(5.0);

    let mut doc2 = empty_ctx();
    let mut ctx2 = Context::new(&mut doc2);
    registry
        .regenerate(
            &mut ctx2,
            &[
                outer,
                bore,
                FeatureCall::new(
                    "thicken",
                    id(3),
                    json!({ "faces": bore_q, "up_to_face": wall_q }),
                ),
            ],
            None,
        )
        .unwrap();
    let th = ctx2.features.iter().find(|f| f.id == id(3)).expect("thicken");
    let (outer_p, inner_p) = thicken_offset_pair(&th.node);
    match (outer_p, inner_p) {
        (
            Primitive::Cylinder { radius: ro, .. },
            Primitive::Cylinder { radius: ri, .. },
        ) => {
            assert!((ro - 5.0).abs() < 0.2, "outer reaches the wall radius: {ro}");
            assert!((ri - 3.0).abs() < 0.2, "inner stays at the bore radius: {ri}");
        }
        other => panic!("expected two cylinders, got {other:?}"),
    }
}

#[test]
fn thicken_direction_modes_set_outer_and_inner_radii() {
    let registry = FeatureRegistry::with_builtins();
    let sphere = FeatureCall::new("sphere", id(1), json!({ "radius": 5.0 }));
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    registry
        .regenerate(&mut ctx, std::slice::from_ref(&sphere), None)
        .unwrap();
    let brep = weave_first(&mut ctx);
    let face_q = format!(
        "qFace({})",
        analytic_face_hash(&brep, |s| matches!(
            s,
            fielddraft_protocol::HalfSpace::Sphere { .. }
        ))
        .unwrap()
    );

    let radii = |extra: serde_json::Value| -> (f32, f32) {
        let mut params = json!({
            "faces": face_q,
            "thickness": { "value": 2.0, "unit": "mm" },
        });
        let obj = params.as_object_mut().unwrap();
        for (k, v) in extra.as_object().unwrap() {
            obj.insert(k.clone(), v.clone());
        }
        let mut d = empty_ctx();
        let mut c = Context::new(&mut d);
        registry
            .regenerate(
                &mut c,
                &[sphere.clone(), FeatureCall::new("thicken", id(2), params)],
                None,
            )
            .unwrap();
        let th = c.features.iter().find(|f| f.id == id(2)).unwrap();
        match thicken_offset_pair(&th.node) {
            (
                Primitive::Sphere { radius: ro, .. },
                Primitive::Sphere { radius: ri, .. },
            ) => (ro, ri),
            other => panic!("expected spheres, got {other:?}"),
        }
    };
    // r = 5. symmetric: ±1 → (6, 4). outward: +2 / +0 → (7, 5).
    // two-sided: +2 outward, 1 inward → (7, 4).
    let (ro, ri) = radii(json!({ "symmetric": true }));
    assert!((ro - 6.0).abs() < 1e-3 && (ri - 4.0).abs() < 1e-3, "symmetric {ro}/{ri}");
    let (ro, ri) = radii(json!({ "symmetric": false }));
    assert!((ro - 7.0).abs() < 1e-3 && (ri - 5.0).abs() < 1e-3, "outward {ro}/{ri}");
    let (ro, ri) = radii(
        json!({ "symmetric": false, "thickness_back": { "value": 1.0, "unit": "mm" } }),
    );
    assert!((ro - 7.0).abs() < 1e-3 && (ri - 4.0).abs() < 1e-3, "two-sided {ro}/{ri}");
}

#[test]
fn thicken_def_schema_has_six_params() {
    let schema = ThickenDef::schema_json();
    let params = schema["params"].as_array().expect("params array");
    assert_eq!(
        params.len(),
        6,
        "faces, thickness, symmetric, thickness_back, up_to_face, operation"
    );
    let names: Vec<&str> =
        params.iter().filter_map(|p| p["name"].as_str()).collect();
    assert!(names.iter().any(|n| n.contains("Faces")), "{names:?}");
    assert!(names.iter().any(|n| n.contains("Thickness")), "{names:?}");
    assert!(names.iter().any(|n| n.contains("Up to face")), "{names:?}");
}

/// Revolve a closed, clearly non-circular "blob" profile (at `x > 0`) around
/// the Y axis → a closed surface of revolution with NO analytic half-space,
/// so its wall weaves as a `Tessellated` face. This is the carrier the
/// per-primitive analytic thicken cannot reach.
fn blob_revolve_feature(n: u32) -> DecodedFeature {
    use ::fielddraft_protocol::{Sketch2d, Sketch2dPrimitive, SketchSegment};
    let qb = |p0: [f32; 2], p1: [f32; 2], p2: [f32; 2]| SketchSegment::QuadBezier { p0, p1, p2 };
    let profile = Sketch2d::Prim(Sketch2dPrimitive::Path {
        segments: vec![
            qb([5.0, 0.0], [7.0, 2.0], [5.0, 3.0]),
            qb([5.0, 3.0], [3.0, 2.0], [3.5, 0.0]),
            qb([3.5, 0.0], [3.0, -2.0], [5.0, -3.0]),
            qb([5.0, -3.0], [7.0, -2.0], [5.0, 0.0]),
        ],
        closed: true,
    });
    DecodedFeature {
        id: id(n),
        name: format!("Revolve {n}"),
        op: BoolOp::New,
        blend_radius: 0.0,
        blend_mode: crate::csg::BlendMode::Fillet,
        target_ids: vec![],
        transform: Transform::IDENTITY,
        node: CsgNode::Revolve { id: id(n), sketch: Box::new(profile) },
    }
}

/// Find a non-analytic (`Tessellated`) face's id_hash.
/// A face WITHOUT a parameter-shift analytic offset: tessellated, or a
/// revolved-profile carrier (analytic for the weaver, but thickened via the
/// general field-offset band like the tessellated faces it used to be).
fn tessellated_face_hash(brep: &crate::brep::BRep) -> Option<u64> {
    use crate::brep::{FaceId, SurfaceGeometry};
    (0..brep.face_count()).find_map(|i| {
        let fid = FaceId(i as u32);
        let band_route = match &brep.face(fid).surface {
            SurfaceGeometry::Tessellated { .. } => true,
            SurfaceGeometry::Analytic { surface, .. } => matches!(
                surface,
                fielddraft_protocol::HalfSpace::RevolvedProfile { .. }
            ),
        };
        band_route.then(|| crate::brep::naming::TopoRef::from_face(brep, fid).id_hash)
    })
}

#[test]
fn thicken_closed_non_analytic_face_weaves_a_watertight_band() {
    // M0 end to end: a closed NON-analytic face (revolved blob) → pick its
    // Tessellated wall → Thicken. The general field-offset path builds
    // `Shell(Offset(carrier, out), out+in)` from the source revolve's carrier
    // and that band weaves watertight with selectable inner + outer walls —
    // a face the analytic per-primitive thicken would reject outright.
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    ctx.features.push(blob_revolve_feature(1));

    // The blob's wall really is non-analytic.
    let brep = crate::body::weave_all(&ctx.features)[0].0.clone();
    let hash = tessellated_face_hash(&brep).expect("the revolved blob has a Tessellated wall");

    // Drive the actual feature (resolve → carrier lookup → band node).
    let def = json!({
        "faces": format!("qFace({hash})"),
        "thickness": { "value": 1.0, "unit": "mm" },
        "thickness_back": { "value": 0.6, "unit": "mm" },
        "symmetric": false,
    });
    ThickenFeature.execute(&mut ctx, &id(2), &def).expect("thicken a closed non-analytic face");
    assert_eq!(ctx.features.len(), 2, "thicken appended its wall feature");

    // The wall node is the general band: Shell(Offset(carrier, out), out+in).
    let thicken = ctx.features[1].clone();
    match &thicken.node {
        CsgNode::Shell(inner, _, open) => {
            assert!(open.is_empty(), "a closed band has no open faces");
            assert!(matches!(**inner, CsgNode::Offset(_, _)), "Shell wraps an Offset carrier");
        }
        other => panic!("expected Shell(Offset(..)) band, got {other:?}"),
    }

    // Weave the band body on its own → watertight shell, selectable walls.
    let band = crate::body::weave_all(std::slice::from_ref(&thicken));
    assert_eq!(band.len(), 1, "the thicken wall is one body");
    let brep = &band[0].0;
    assert!(brep.validate().is_ok(), "band B-Rep must be well-formed");
    assert!(
        brep.shells.iter().any(|s| s.is_closed),
        "thickened wall must weave a closed (watertight) shell"
    );
    // The curved walls now carry RevolvedProfile carriers (analytic for the
    // weaver, still band-thickened); before that they were Tessellated.
    let tess: Vec<_> = (0..brep.face_count())
        .filter(|&i| match &brep.face(crate::brep::FaceId(i as u32)).surface {
            crate::brep::SurfaceGeometry::Tessellated { .. } => true,
            crate::brep::SurfaceGeometry::Analytic { surface, .. } => matches!(
                surface,
                fielddraft_protocol::HalfSpace::RevolvedProfile { .. }
            ),
        })
        .collect();
    assert!(
        tess.len() >= 2,
        "a closed non-analytic band has inner AND outer curved walls, got {}",
        tess.len()
    );
    for &i in &tess {
        let h = crate::brep::naming::TopoRef::from_face(brep, crate::brep::FaceId(i as u32)).id_hash;
        assert_ne!(h, 0, "each band wall must be selectable (qFace handle)");
    }
}

/// A closed swept (Bézier) extrude whose four curved side walls are BOUNDED
/// non-analytic faces (the M1 target — each is a clean 4-edge Kantenzug).
fn blob_extrude_feature(n: u32) -> DecodedFeature {
    use fielddraft_protocol::{Sketch2d, Sketch2dPrimitive, SketchSegment};
    let qb = |p0: [f32; 2], p1: [f32; 2], p2: [f32; 2]| SketchSegment::QuadBezier { p0, p1, p2 };
    let profile = Sketch2d::Prim(Sketch2dPrimitive::Path {
        segments: vec![
            qb([3.0, 0.0], [4.0, 4.0], [0.0, 3.0]),
            qb([0.0, 3.0], [-4.0, 4.0], [-3.0, 0.0]),
            qb([-3.0, 0.0], [-4.0, -4.0], [0.0, -3.0]),
            qb([0.0, -3.0], [4.0, -4.0], [3.0, 0.0]),
        ],
        closed: true,
    });
    DecodedFeature {
        id: id(n),
        name: format!("Extrude {n}"),
        op: BoolOp::New,
        blend_radius: 0.0,
        blend_mode: crate::csg::BlendMode::Fillet,
        target_ids: vec![],
        transform: Transform::IDENTITY,
        node: CsgNode::Extrude {
            id: id(n),
            sketch: Box::new(profile),
            half_height: 2.0,
        },
    }
}

/// XY-plane bounding-box area of a woven band (vertices + Tessellated samples).
fn xy_footprint(brep: &crate::brep::BRep) -> f32 {
    let mut mn = [f64::INFINITY; 2];
    let mut mx = [f64::NEG_INFINITY; 2];
    let mut acc = |p: &[f64; 3]| {
        for k in 0..2 {
            mn[k] = mn[k].min(p[k]);
            mx[k] = mx[k].max(p[k]);
        }
    };
    for v in &brep.vertices {
        acc(&v.position);
    }
    for i in 0..brep.face_count() {
        if let crate::brep::SurfaceGeometry::Tessellated { positions, .. } =
            &brep.face(crate::brep::FaceId(i as u32)).surface
        {
            for p in positions {
                acc(p);
            }
        }
    }
    ((mx[0] - mn[0]).max(0.0) * (mx[1] - mn[1]).max(0.0)) as f32
}

#[test]
fn thicken_one_extrude_wall_trims_to_a_bounded_patch() {
    // M1 end to end: an extrude's curved side wall is a BOUNDED non-analytic
    // face (a 4-edge Kantenzug). Picking ONE wall + Thicken builds the general
    // band (a shell of the *whole* blob) AND laterally trims it via the swept
    // edge-loop prism, so the result is just that one wall's slab — not a shell
    // wrapping all four walls.
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    ctx.features.push(blob_extrude_feature(1));

    // Pick one Bézier wall (the first Tessellated face).
    let brep = crate::body::weave_all(&ctx.features)[0].0.clone();
    let hash = tessellated_face_hash(&brep).expect("the blob extrude has Tessellated walls");

    let def = json!({
        "faces": format!("qFace({hash})"),
        "thickness": { "value": 1.0, "unit": "mm" },
        "thickness_back": { "value": 0.5, "unit": "mm" },
        "symmetric": false,
    });
    ThickenFeature.execute(&mut ctx, &id(2), &def).expect("thicken one extrude wall");

    // The wall node trims the band: Intersection(Shell(Offset(..)), prism).
    let thicken = ctx.features[1].clone();
    match &thicken.node {
        CsgNode::Intersection(a, _) => assert!(
            matches!(a.as_ref(), CsgNode::Shell(inner, _, _)
                if matches!(inner.as_ref(), CsgNode::Offset(_, _))),
            "the trimmed band's first operand must be the M0 Shell(Offset(..))"
        ),
        other => panic!("an open wall's band must be laterally trimmed, got {other:?}"),
    }

    // Weave the trimmed band → watertight, well-formed.
    let band = crate::body::weave_all(std::slice::from_ref(&thicken));
    let tb = &band[0].0;
    assert!(tb.validate().is_ok(), "trimmed band must be well-formed");
    assert!(
        tb.shells.iter().any(|s| s.is_closed),
        "trimmed wall must weave a closed (watertight) shell"
    );

    // The trim really bounds it: the untrimmed whole-blob band spans all four
    // quadrants; the one-wall band's XY footprint must be markedly smaller.
    let trimmed_area = xy_footprint(tb);
    let carrier = match blob_extrude_feature(1).node {
        CsgNode::Extrude { sketch, half_height, .. } => CsgNode::Extrude {
            id: id(9),
            sketch,
            half_height,
        },
        other => other,
    };
    let fb =
        crate::brep::face_offset::offset_band(carrier, 1.0, 0.5, Transform::IDENTITY).unwrap();
    let full_feature = DecodedFeature {
        id: id(3),
        name: "full".into(),
        op: BoolOp::New,
        blend_radius: 0.0,
        blend_mode: crate::csg::BlendMode::Fillet,
        target_ids: vec![],
        transform: Transform::IDENTITY,
        node: fb.node,
    };
    let full = xy_footprint(&crate::body::weave_all(std::slice::from_ref(&full_feature))[0].0);
    assert!(
        trimmed_area < 0.6 * full,
        "one-wall band footprint {trimmed_area:.1} must be well under the full shell {full:.1}"
    );
}

#[test]
fn thicken_isolates_the_carrier_of_a_composite_body() {
    // A boolean owner (union of two revolved blobs): the picked face's
    // carrier is isolated by probing the face's loop samples against each
    // single-primitive subtree (`face_carrier`), so the wall grows from the
    // ONE revolve the face lives on — offsetting the whole boolean would
    // thicken every surface. This used to be a hard "deferred past M0"
    // error.
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    let mut composite = blob_revolve_feature(1);
    composite.node = CsgNode::Union(
        Box::new(composite.node.clone()),
        Box::new(CsgNode::Transform(
            Transform { translate: [14.0, 0.0, 0.0], ..Transform::IDENTITY },
            Box::new(match blob_revolve_feature(1).node {
                CsgNode::Revolve { sketch, .. } => CsgNode::Revolve { id: id(1), sketch },
                other => other,
            }),
        )),
    );
    ctx.features.push(composite);
    let brep = crate::body::weave_all(&ctx.features)[0].0.clone();
    let hash = tessellated_face_hash(&brep).expect("a Tessellated wall");
    let def = json!({ "faces": format!("qFace({hash})"), "thickness": { "value": 1.0, "unit": "mm" } });
    ThickenFeature
        .execute(&mut ctx, &id(2), &def)
        .expect("a composite owner must thicken via carrier isolation");

    // The wall feature's node must be built from exactly ONE of the two
    // revolves (the probed carrier), never the whole union.
    fn count_revolves(node: &CsgNode) -> usize {
        match node {
            CsgNode::Revolve { .. } => 1,
            CsgNode::Transform(_, inner)
            | CsgNode::Shell(inner, _, _)
            | CsgNode::Offset(inner, _) => count_revolves(inner),
            CsgNode::Union(a, b)
            | CsgNode::Difference(a, b)
            | CsgNode::Intersection(a, b) => count_revolves(a) + count_revolves(b),
            _ => 0,
        }
    }
    let wall = ctx
        .features
        .iter()
        .find(|f| f.id == id(2))
        .expect("the thicken wall feature must land on the spine");
    assert_eq!(
        count_revolves(&wall.node),
        1,
        "the wall must grow from the single probed carrier, got node {:?}",
        wall.node
    );
}

// ─── C2: mate-connector-on-face ────────────────────────────────

#[test]
fn mate_connector_resolves_face_query_to_origin_and_normal() {
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);

    // A 10mm half-extent box centred at origin: its +Z face sits at
    // z = 10, normal +Z, centroid (0, 0, 10).
    registry
        .regenerate(&mut ctx, std::slice::from_ref(&box_call(1, 10.0)), None)
        .unwrap();

    // Find the +Z face id_hash from the woven body.
    let woven = crate::body::weave_all(&ctx.features);
    let (brep, placement) = &woven[0];
    let mut top_hash = None;
    for i in 0..brep.face_count() {
        let frame = crate::sketch::face_plane_frame(
            brep,
            crate::brep::FaceId(i as u32),
            placement,
        );
        if (frame.normal[2] - 1.0).abs() < 1e-4 {
            top_hash =
                Some(crate::brep::naming::TopoRef::from_face(brep, crate::brep::FaceId(i as u32)).id_hash);
        }
    }
    let top_hash = top_hash.expect("box has a +Z face");

    // Face anchor, no `primary` → CSYS from the face's own frame.
    registry
        .regenerate(
            &mut ctx,
            &[FeatureCall::new(
                "mate_connector",
                id(2),
                json!({ "anchor": format!("qFace({top_hash})") }),
            )],
            None,
        )
        .unwrap();
    assert_eq!(ctx.mate_connectors.len(), 1);
    let mc_origin = ctx.mate_connectors[0].origin;
    let mc_normal = ctx.mate_connectors[0].normal;
    assert!((mc_normal[2] - 1.0).abs() < 1e-3, "normal is +Z, got {mc_normal:?}");
    assert!((mc_origin[2] - 10.0).abs() < 1e-2, "origin on the +Z face, got {mc_origin:?}");
    assert!(mc_origin[0].abs() < 1e-2 && mc_origin[1].abs() < 1e-2, "centroid centred");

    // `flip` reverses the primary (Z) axis, leaving the origin alone.
    ctx.mate_connectors.clear();
    registry
        .regenerate(
            &mut ctx,
            &[FeatureCall::new(
                "mate_connector",
                id(3),
                json!({ "anchor": format!("qFace({top_hash})"), "flip": true }),
            )],
            None,
        )
        .unwrap();
    let flipped = &ctx.mate_connectors[0];
    assert!((flipped.normal[2] + 1.0).abs() < 1e-3, "flip → -Z, got {:?}", flipped.normal);
    assert!((flipped.origin[2] - 10.0).abs() < 1e-2, "flip keeps the origin");
}

#[test]
fn mate_connector_offset_between_and_owner() {
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    registry
        .regenerate(&mut ctx, std::slice::from_ref(&box_call(1, 10.0)), None)
        .unwrap();

    // +Z face (z=10) and -Z face (z=-10) hashes.
    let woven = crate::body::weave_all(&ctx.features);
    let (brep, placement) = &woven[0];
    let (mut top_hash, mut bot_hash) = (None, None);
    for i in 0..brep.face_count() {
        let frame = crate::sketch::face_plane_frame(
            brep,
            crate::brep::FaceId(i as u32),
            placement,
        );
        let hash = crate::brep::naming::TopoRef::from_face(brep, crate::brep::FaceId(i as u32)).id_hash;
        if (frame.normal[2] - 1.0).abs() < 1e-4 {
            top_hash = Some(hash);
        } else if (frame.normal[2] + 1.0).abs() < 1e-4 {
            bot_hash = Some(hash);
        }
    }
    let (top_hash, bot_hash) = (top_hash.expect("+Z face"), bot_hash.expect("-Z face"));

    // Realignment offset: +5 along the connector's own +Z lifts the origin
    // from z=10 to z=15; owner qCreatedBy(1) → owner_id = Some(1).
    registry
        .regenerate(
            &mut ctx,
            &[FeatureCall::new(
                "mate_connector",
                id(2),
                json!({
                    "anchor": format!("qFace({top_hash})"),
                    "offset": [0.0, 0.0, 5.0],
                    "owner": "qCreatedBy(1)",
                }),
            )],
            None,
        )
        .unwrap();
    let mc = &ctx.mate_connectors[0];
    assert!((mc.origin[2] - 15.0).abs() < 1e-2, "offset lifts +Z origin to 15, got {:?}", mc.origin);
    assert_eq!(mc.owner_id, Some(1), "owner qCreatedBy(1) → seed id 1");

    // Between-entities: midpoint of the +Z (z=10) and -Z (z=-10) face
    // centroids sits at z=0.
    ctx.mate_connectors.clear();
    registry
        .regenerate(
            &mut ctx,
            &[FeatureCall::new(
                "mate_connector",
                id(3),
                json!({
                    "anchor": format!("qFace({top_hash})"),
                    "anchor2": format!("qFace({bot_hash})"),
                }),
            )],
            None,
        )
        .unwrap();
    let mid = &ctx.mate_connectors[0];
    assert!(mid.origin[2].abs() < 1e-2, "between → midpoint z=0, got {:?}", mid.origin);
    assert_eq!(mid.owner_id, None, "no owner query → None");
}

#[test]
fn mate_connector_anchors_to_edge_midpoint_and_vertex() {
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    registry
        .regenerate(&mut ctx, std::slice::from_ref(&box_call(1, 10.0)), None)
        .unwrap();

    let woven = crate::body::weave_all(&ctx.features);
    let (brep, _placement) = &woven[0];

    // The +Z face (for orientation) and a TOP edge (both endpoints z≈10).
    let mut top_face_hash = None;
    for i in 0..brep.face_count() {
        let f = brep.face(crate::brep::FaceId(i as u32));
        if let crate::brep::SurfaceGeometry::Analytic {
            surface: fielddraft_protocol::HalfSpace::Plane { normal, .. },
            ..
        } = &f.surface
        {
            if (normal[2] - 1.0).abs() < 1e-4 {
                top_face_hash = Some(
                    crate::brep::naming::TopoRef::from_face(brep, crate::brep::FaceId(i as u32))
                        .id_hash,
                );
            }
        }
    }
    let top_face_hash = top_face_hash.expect("box +Z face");

    let mut top_edge_hash = None;
    for i in 0..brep.edges.len() {
        let eid = crate::brep::EdgeId(i as u32);
        let poly = crate::brep::edge_sampling::sample_edge(&brep.edge(eid).geometry);
        if poly.len() >= 2 && poly.iter().all(|p| (p[2] - 10.0).abs() < 1e-3) {
            top_edge_hash =
                Some(crate::brep::naming::TopoRef::from_edge(brep, eid).id_hash);
            break;
        }
    }
    let top_edge_hash = top_edge_hash.expect("box has a top edge");

    // A top corner vertex (z≈10).
    let mut corner_hash = None;
    for i in 0..brep.vertices.len() {
        let vid = crate::brep::VertexId(i as u32);
        if (brep.vertex(vid).position[2] - 10.0).abs() < 1e-3 {
            corner_hash =
                Some(crate::brep::naming::TopoRef::from_vertex(brep, vid).id_hash);
            break;
        }
    }
    let corner_hash = corner_hash.expect("box has a top vertex");

    // Edge anchor WITHOUT a secondary face (ticket 5j9l: an edge alone
    // suffices, no "needs primary face" error). The edge IS +Z (ticket
    // o3er): a top edge is horizontal, so its tangent has ~zero z. The
    // secondary face is auto-picked; X stays ⟂ Z and unit.
    registry
        .regenerate(
            &mut ctx,
            &[FeatureCall::new(
                "mate_connector",
                id(2),
                json!({ "anchor": format!("qEdge({top_edge_hash})"), "t": 0.5 }),
            )],
            None,
        )
        .unwrap();
    let edge_mc = ctx.mate_connectors[0].clone();
    assert!((edge_mc.origin[2] - 10.0).abs() < 1e-2, "edge origin on top, got {:?}", edge_mc.origin);
    let zlen = (edge_mc.normal[0].powi(2) + edge_mc.normal[1].powi(2) + edge_mc.normal[2].powi(2)).sqrt();
    assert!((zlen - 1.0).abs() < 1e-3, "Z unit");
    assert!(edge_mc.normal[2].abs() < 1e-3, "edge Z = horizontal tangent, got {:?}", edge_mc.normal);
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    assert!(dot(edge_mc.x_axis, edge_mc.normal).abs() < 1e-4, "X ⟂ Z");
    let xlen = (edge_mc.x_axis[0].powi(2) + edge_mc.x_axis[1].powi(2) + edge_mc.x_axis[2].powi(2)).sqrt();
    assert!((xlen - 1.0).abs() < 1e-3, "X unit");

    // reorient=1 rotates the secondary axis 90° about Z → X' = Z × X.
    ctx.mate_connectors.clear();
    registry
        .regenerate(
            &mut ctx,
            &[FeatureCall::new(
                "mate_connector",
                id(2),
                json!({ "anchor": format!("qEdge({top_edge_hash})"), "t": 0.5, "reorient": 1 }),
            )],
            None,
        )
        .unwrap();
    let rot = &ctx.mate_connectors[0];
    let expect = [
        edge_mc.normal[1] * edge_mc.x_axis[2] - edge_mc.normal[2] * edge_mc.x_axis[1],
        edge_mc.normal[2] * edge_mc.x_axis[0] - edge_mc.normal[0] * edge_mc.x_axis[2],
        edge_mc.normal[0] * edge_mc.x_axis[1] - edge_mc.normal[1] * edge_mc.x_axis[0],
    ];
    for k in 0..3 {
        assert!((rot.x_axis[k] - expect[k]).abs() < 1e-3, "reorient 90°: {:?} vs {:?}", rot.x_axis, expect);
    }

    // Vertex anchor with an explicit secondary face: origin at the corner,
    // +Z = the +Z face normal.
    ctx.mate_connectors.clear();
    registry
        .regenerate(
            &mut ctx,
            &[FeatureCall::new(
                "mate_connector",
                id(3),
                json!({
                    "anchor": format!("qVertex({corner_hash})"),
                    "secondary": format!("qFace({top_face_hash})"),
                }),
            )],
            None,
        )
        .unwrap();
    let vtx_mc = &ctx.mate_connectors[0];
    assert!((vtx_mc.origin[2] - 10.0).abs() < 1e-2, "vertex origin at corner z=10");
    assert!(
        (vtx_mc.origin[0].abs() - 10.0).abs() < 1e-2 && (vtx_mc.origin[1].abs() - 10.0).abs() < 1e-2,
        "vertex origin at a ±10/±10 corner, got {:?}", vtx_mc.origin
    );
    assert!((vtx_mc.normal[2] - 1.0).abs() < 1e-3, "vertex Z from secondary +Z face");

    // Vertex anchor WITHOUT a secondary: auto-picks an adjacent face normal
    // (a box corner's faces are the ±X/±Y/±Z axes) — no error.
    ctx.mate_connectors.clear();
    registry
        .regenerate(
            &mut ctx,
            &[FeatureCall::new(
                "mate_connector",
                id(3),
                json!({ "anchor": format!("qVertex({corner_hash})") }),
            )],
            None,
        )
        .unwrap();
    let auto = &ctx.mate_connectors[0];
    let zl = (auto.normal[0].powi(2) + auto.normal[1].powi(2) + auto.normal[2].powi(2)).sqrt();
    assert!((zl - 1.0).abs() < 1e-3, "auto vertex Z unit, got {:?}", auto.normal);
}

// ─── DL-61: fillet edge query → SMin polyline tube ──────────────

/// A box `box` feature with a translate (folds into the body
/// placement when it's the anchor) and a small +Z step box on top.
fn box_call_translated(n: u32, half: f32, translate: [f32; 3]) -> FeatureCall {
    FeatureCall::new(
        "box",
        id(n),
        json!({
            "half_extents": [half, half, half],
            "translate": translate,
        }),
    )
}

/// Build a base box (anchor, union) and a smaller box SUBTRACTED from
/// it. Union features each seed their own body (bodies stay separate),
/// so a Difference that overlaps body 1 is what keeps both features in
/// a single woven brep — yielding real feature-1×feature-2 edges (the
/// rim of the cut) plus a Difference junction in the CSG tree for the
/// fillet to match. Returns one such edge's `qEdge` id_hash and the
/// world-space mid-span point of its polyline.
fn step_geometry(
    registry: &FeatureRegistry,
    ctx: &mut Context<'_>,
    anchor_translate: [f32; 3],
) -> (u64, [f32; 3]) {
    registry
        .regenerate(
            ctx,
            &[
                box_call_translated(1, 10.0, anchor_translate),
                // Smaller box poking out the +Z top, subtracted: spans
                // z=[5,15] (overlaps box 1's [-10,10]) so its cut rim at
                // z≈10 is a feature-1×feature-2 edge in one body.
                FeatureCall::new(
                    "box",
                    id(2),
                    json!({
                        "half_extents": [5.0, 5.0, 5.0],
                        "center": [0.0, 0.0, 10.0],
                        "translate": anchor_translate,
                        "op": "remove",
                        "targets": "qCreatedBy(1)",
                    }),
                ),
            ],
            None,
        )
        .unwrap();

    let woven = crate::body::weave_all(&ctx.features);
    assert_eq!(woven.len(), 1, "one connected body");
    let (brep, placement) = &woven[0];

    // Find an edge whose signature mixes feature 1 and feature 2.
    use crate::brep::naming::TopoRef;
    use crate::brep::EdgeId;
    let mut found = None;
    for i in 0..brep.edges.len() {
        let e = brep.edge(EdgeId(i as u32));
        let s0 = e.signature.0.feature_id();
        let s1 = e.signature.1.feature_id();
        if (s0 == 1 && s1 == 2) || (s0 == 2 && s1 == 1) {
            let hash = TopoRef::from_edge(brep, EdgeId(i as u32)).id_hash;
            // Mid-span world point of the edge polyline.
            let pts: Vec<[f32; 3]> = match &e.geometry {
                crate::brep::EdgeGeometry::Polyline(p) => {
                    p.iter().map(|q| [q[0] as f32, q[1] as f32, q[2] as f32]).collect()
                }
                crate::brep::EdgeGeometry::Hermite(s) => s
                    .iter()
                    .map(|h| [h.point[0] as f32, h.point[1] as f32, h.point[2] as f32])
                    .collect(),
                crate::brep::EdgeGeometry::Nurbs(c) => c
                    .control_points
                    .iter()
                    .map(|q| [q[0] as f32, q[1] as f32, q[2] as f32])
                    .collect(),
            };
            let mid_local = pts[pts.len() / 2];
            // local → world via placement (same math as the resolver).
            let mid_world = super::place_point(placement, mid_local);
            found = Some((hash, mid_world));
            break;
        }
    }
    found.expect("a feature-1×feature-2 edge exists at the step")
}

/// At the edge mid-span, just outside both surfaces, the filleted
/// (Visual) SDF must be strictly LESS than the un-filleted hard `min`
/// (smin pulls the surface outward to round the corner); far away the
/// SDF is unchanged. With identity placement.
#[test]
fn fillet_edge_query_rounds_the_edge_identity_placement() {
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    let (edge_hash, mid_world) = step_geometry(&registry, &mut ctx, [0.0, 0.0, 0.0]);

    // Apply the fillet via the edge query.
    registry
        .regenerate(
            &mut ctx,
            &[FeatureCall::new(
                "fillet_edge",
                id(3),
                json!({ "edge": format!("qEdge({edge_hash})"), "blend_radius": 3.0 }),
            )],
            None,
        )
        .unwrap();
    assert_eq!(ctx.edge_fillets.len(), 1, "one fillet pushed");
    assert_eq!(ctx.edge_fillets[0].feature_a.as_u32_or_root(), Some(1));
    assert_eq!(ctx.edge_fillets[0].feature_b.as_u32_or_root(), Some(2));

    let hard = crate::csg::CsgNode::from_decoded(&ctx.features);
    let filleted = hard.clone().apply_edge_fillets(&ctx.edge_fillets);
    assert!(
        matches!(filleted, crate::csg::CsgNode::MaskedBlend { .. })
            || count_masked_blends(&filleted) >= 1,
        "the matching junction became a MaskedBlend",
    );

    assert_rounded_at_edge(&hard, &filleted, mid_world);
}

/// The decisive coordinate-space test: a NON-identity body placement.
/// An identity-only test cannot catch a missing placement transform;
/// this one fails unless the polyline is mapped local→world.
#[test]
fn fillet_edge_query_rounds_the_edge_nonidentity_placement() {
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    // Translate the whole body so its placement != IDENTITY.
    let (edge_hash, mid_world) = step_geometry(&registry, &mut ctx, [100.0, -40.0, 7.0]);

    registry
        .regenerate(
            &mut ctx,
            &[FeatureCall::new(
                "fillet_edge",
                id(3),
                json!({ "edge": format!("qEdge({edge_hash})"), "blend_radius": 3.0 }),
            )],
            None,
        )
        .unwrap();
    assert_eq!(ctx.edge_fillets.len(), 1);

    // Discriminator: the stored polyline must be in WORLD space —
    // every point shifted by the body placement (~+100 in x). Without
    // applying `placement` it would sit at the body-local origin and
    // this would fail (and the near-edge probe below would find no
    // perturbation).
    let poly = &ctx.edge_fillets[0].mask_polyline;
    assert!(
        poly.iter().all(|p| p[0] > 90.0),
        "polyline must be world-space (x≈+100), got {poly:?}",
    );

    let hard = crate::csg::CsgNode::from_decoded(&ctx.features);
    let filleted = hard.clone().apply_edge_fillets(&ctx.edge_fillets);
    assert_rounded_at_edge(&hard, &filleted, mid_world);
}

#[test]
fn fillet_edge_query_resolving_nothing_is_a_hard_error() {
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    registry
        .regenerate(&mut ctx, std::slice::from_ref(&box_call(1, 10.0)), None)
        .unwrap();
    let err = registry
        .regenerate(
            &mut ctx,
            &[FeatureCall::new(
                "fillet_edge",
                id(2),
                json!({ "edge": "qEdge(999999)", "blend_radius": 3.0 }),
            )],
            None,
        )
        .unwrap_err();
    assert!(format!("{err:?}").contains("edge"), "error mentions the edge param: {err:?}");
}

/// d: the `round_all` feature captures a document-wide round radius, and the
/// eval CSG (mesh/GPU-SDF path) rounds every Boolean junction with it.
#[test]
fn round_all_feature_rounds_every_junction() {
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    // Two overlapping boxes (a union) + a global round.
    registry
        .regenerate(
            &mut ctx,
            &[
                box_call(1, 10.0),
                FeatureCall::new(
                    "box",
                    id(2),
                    json!({ "half_extents": [10.0, 10.0, 10.0], "center": [14.0, 0.0, 0.0] }),
                ),
                FeatureCall::new("round_all", id(3), json!({ "radius": 3.0 })),
            ],
            None,
        )
        .unwrap();
    assert_eq!(ctx.global_round, Some(3.0), "round_all captured the radius");

    // The rounded eval CSG pulls the convex union edge below the hard min.
    let hard = crate::csg::CsgNode::from_decoded(&ctx.features);
    let rounded = hard.clone().apply_global_round(3.0);
    // Convex edge near (4, 10, 0) where box1's +Y face meets box2's −X region.
    let p = [4.0, 10.0, 0.0];
    let d_hard = hard.sdf(p, crate::csg::EvalMode::Visual);
    let d_round = rounded.sdf(p, crate::csg::EvalMode::Visual);
    assert!(d_round < d_hard, "round {d_round} not < hard {d_hard}");

    // radius 0 clears the global round.
    registry
        .regenerate(
            &mut ctx,
            &[box_call(1, 10.0), FeatureCall::new("round_all", id(3), json!({ "radius": 0.0 }))],
            None,
        )
        .unwrap();
    assert_eq!(ctx.global_round, None, "radius 0 clears the round");
}

/// c-M1: the procedural blend carrier. A filleted junction must weave the
/// blend as its OWN analytic face — `HalfSpace::Blend`, keyed on the fillet
/// feature's branch — not a phantom sharp edge (B-Rep = Logical CSG before)
/// and not a nameless Tessellated patch. This is the fix for the Onshape
/// "can't reference a fillet" pain: the blend face now has a stable identity.
#[test]
fn fillet_blend_weaves_as_its_own_analytic_face() {
    use crate::brep::SurfaceGeometry;
    use crate::csg::BranchId;
    use fielddraft_protocol::HalfSpace;

    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    let (edge_hash, _mid) = step_geometry(&registry, &mut ctx, [0.0, 0.0, 0.0]);
    registry
        .regenerate(
            &mut ctx,
            &[FeatureCall::new(
                "fillet_edge",
                id(3),
                json!({ "edge": format!("qEdge({edge_hash})"), "blend_radius": 3.0 }),
            )],
            None,
        )
        .unwrap();

    let hard = crate::csg::CsgNode::from_decoded(&ctx.features);
    let filleted = hard.apply_edge_fillets(&ctx.edge_fillets);
    let body = crate::bvh::BoundedCsg::from_csg(&filleted);

    // The surface registry detects the blend carrier, keyed on the fillet
    // feature's own branch (sub 0).
    let fillet_branch = BranchId::from_feature(&id(3)).as_u32();
    let map = crate::brep::weaving::branch_halfspace_map(&body);
    let entry = map.get(&fillet_branch).unwrap_or_else(|| {
        panic!("no blend carrier registered for the fillet branch (keys: {:?})", map.keys())
    });
    assert!(
        matches!(entry.0, HalfSpace::Blend { is_difference: true, .. }),
        "carrier is a difference Blend, got {:?}",
        entry.0
    );

    // The octree weave materialises a face on that carrier.
    let bb = body.aabb();
    let largest = bb.size().into_iter().fold(0.0_f32, f32::max);
    let min_cell = (largest / 48.0).clamp(1e-3, 10.0);
    let octree = crate::brep::octree::Octree::build(&body, min_cell);
    let brep = crate::brep::weaving::weave(&body, &octree);

    let blend_faces: Vec<_> = brep
        .faces
        .iter()
        .filter(|f| {
            matches!(&f.surface, SurfaceGeometry::Analytic { surface: HalfSpace::Blend { .. }, .. })
        })
        .collect();
    assert!(
        !blend_faces.is_empty(),
        "at least one woven face lies on the blend carrier (face kinds: {:?})",
        brep.faces
            .iter()
            .map(|f| crate::brep::face_export::surface_kind(&f.surface))
            .collect::<Vec<_>>()
    );

    // Its persistent identity is the fillet branch — stable, provenance-based,
    // not a sampled fingerprint.
    for f in &blend_faces {
        assert_eq!(
            f.branch_id.feature_id(),
            BranchId(fillet_branch).feature_id(),
            "blend face carries the fillet branch identity, got {:?}",
            f.branch_id
        );
    }
}

/// c-M2 target: a cylinder embedded in a box, filleted at the circular
/// contact edge. The blend is an exact torus band (round-min between a
/// cylinder and a plane); its two seams are CLOSED circles (blend↔cylinder
/// at height r, blend↔box-top at radius R+r) — no runout, the cleanest
/// manifold fillet.
fn cylinder_on_box_filleted(radius: f32) -> (crate::bvh::BoundedCsg, u32) {
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    // Solid box (top y=10) with a blind cylindrical hole (R=6, axis Y) cut
    // from the top — a Difference keeps both features in ONE woven body. The
    // top rim of the hole is a circle (feature-1×feature-2 edge).
    registry
        .regenerate(
            &mut ctx,
            &[
                FeatureCall::new(
                    "box",
                    id(1),
                    json!({ "half_extents": [15.0, 10.0, 15.0] }),
                ),
                FeatureCall::new(
                    "cylinder",
                    id(2),
                    json!({
                        "radius": 6.0,
                        "half_height": 8.0,
                        "center": [0.0, 6.0, 0.0],
                        "op": "remove",
                        "targets": "qCreatedBy(1)",
                    }),
                ),
            ],
            None,
        )
        .unwrap();
    let woven = crate::body::weave_all(&ctx.features);
    let (brep, placement) = &woven[0];
    // The contact circle: a feature-1×feature-2 edge.
    use crate::brep::naming::TopoRef;
    use crate::brep::EdgeId;
    let mut edge_hash = None;
    for i in 0..brep.edges.len() {
        let e = brep.edge(EdgeId(i as u32));
        let (s0, s1) = (e.signature.0.feature_id(), e.signature.1.feature_id());
        if (s0 == 1 && s1 == 2) || (s0 == 2 && s1 == 1) {
            edge_hash = Some(TopoRef::from_edge(brep, EdgeId(i as u32)).id_hash);
            break;
        }
    }
    let _ = placement;
    let edge_hash = edge_hash.expect("cylinder-box contact edge exists");
    registry
        .regenerate(
            &mut ctx,
            &[FeatureCall::new(
                "fillet_edge",
                id(3),
                json!({ "edge": format!("qEdge({edge_hash})"), "blend_radius": radius }),
            )],
            None,
        )
        .unwrap();
    let hard = crate::csg::CsgNode::from_decoded(&ctx.features);
    let filleted = hard.apply_edge_fillets(&ctx.edge_fillets);
    let fillet_branch = crate::csg::BranchId::from_feature(&id(3)).as_u32();
    (crate::bvh::BoundedCsg::from_csg(&filleted), fillet_branch)
}

/// c-M2: a filleted cylinder↔plane junction weaves MANIFOLD-CLEAN. The two
/// G1-tangent seams (blend↔cylinder, blend↔box-top) — which the octree tracer
/// cannot sample — are materialised symbolically as exact circles, so the
/// blend face is a proper torus band, the box-top gets its hole, and the solid
/// closes (Euler–Poincaré χ = 2, zero orientation violations).
#[test]
fn cylinder_on_box_fillet_weaves_manifold() {
    use crate::brep::SurfaceGeometry;
    use fielddraft_protocol::HalfSpace;

    let (body, fillet_branch) = cylinder_on_box_filleted(3.0);
    let bb = body.aabb();
    let largest = bb.size().into_iter().fold(0.0_f32, f32::max);
    let min_cell = (largest / 48.0).clamp(1e-3, 10.0);
    let octree = crate::brep::octree::Octree::build(&body, min_cell);
    let brep = crate::brep::weaving::weave(&body, &octree);

    let violations = brep.manifold_violations();
    assert!(
        violations.is_empty(),
        "filleted body must be manifold, got {} violation(s): {:?}",
        violations.len(),
        violations.iter().take(4).collect::<Vec<_>>()
    );

    // Generalised Euler–Poincaré of a closed genus-0 solid: (V + closed_edges)
    // − E + (F − inner_loops) == 2.
    let closed_edges =
        brep.edges.iter().filter(|e| e.start.is_none() && e.end.is_none()).count();
    let inner_loops: usize = brep.faces.iter().map(|f| f.inner.len()).sum();
    let chi = (brep.vertices.len() + closed_edges) as i64 - brep.edges.len() as i64
        + brep.faces.len() as i64
        - inner_loops as i64;
    assert_eq!(chi, 2, "closed genus-0 solid: χ must be 2 (V + closed − E + F − inner)");

    // The blend is a torus BAND: one face on the blend carrier with two loops
    // (outer + one inner), i.e. its two circular seams.
    let blend_faces: Vec<_> = brep
        .faces
        .iter()
        .filter(|f| {
            matches!(&f.surface, SurfaceGeometry::Analytic { surface: HalfSpace::Blend { .. }, .. })
        })
        .collect();
    assert_eq!(blend_faces.len(), 1, "exactly one blend face");
    assert_eq!(blend_faces[0].inner.len(), 1, "blend band has two circular boundaries");

    // Both seams exist: blend↔cylinder and blend↔box-top, each a closed circle.
    let blend_id = crate::csg::BranchId(fillet_branch).feature_id();
    let seam_partners: std::collections::HashSet<u32> = brep
        .edges
        .iter()
        .filter(|e| {
            e.start.is_none()
                && (e.signature.0.feature_id() == blend_id
                    || e.signature.1.feature_id() == blend_id)
        })
        .map(|e| {
            if e.signature.0.feature_id() == blend_id {
                e.signature.1.feature_id()
            } else {
                e.signature.0.feature_id()
            }
        })
        .collect();
    assert_eq!(seam_partners.len(), 2, "blend has two distinct circular seams, got {seam_partners:?}");
}

/// End-to-end regression for the UI vase exercise: a Front-plane
/// sketch whose profile is FREE lines (axis, rim, foot, bottom) plus
/// an OPEN interpolated spline (the wall), stitched into a region by
/// K7.1 loop inference, revolved around Y, then shelled 2 mm through
/// the top rim face. Before loop inference this produced a footless
/// cup (the open spline was implicitly chord-closed and all lines
/// were dropped).
/// Timing probe for the vase weave hang (not a regression test): stage
/// breakdown of octree build vs. wireframe extraction vs. finish, printed
/// with `--nocapture`. Mirrors `Body::ensure_brep`'s min_cell derivation.
#[test]
#[ignore = "manual timing probe, run with --ignored --nocapture"]
fn vase_weave_timing_probe() {
    use std::time::Instant;
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    let (sketch_call, revolve_call) = vase_sketch_and_revolve_calls();
    registry
        .regenerate(&mut ctx, &[sketch_call, revolve_call], None)
        .unwrap();
    let solid = crate::csg::CsgNode::from_decoded(&ctx.features);
    let body = crate::bvh::BoundedCsg::from_csg(&solid);
    let bb = body.aabb();
    let size = bb.size();
    let largest = size[0].max(size[1]).max(size[2]);
    let min_cell = (largest / 32.0).clamp(1e-3, 10.0);
    eprintln!("probe: aabb {:?} min_cell {min_cell}", (bb.min, bb.max));

    let t = Instant::now();
    let octree = crate::brep::octree::Octree::build(&body, min_cell);
    eprintln!("probe: octree build {:?} ({} leaves)", t.elapsed(), octree.leaves().len());

    let t = Instant::now();
    let wire = crate::brep::edge_walker::build_wireframe(&body, &octree);
    eprintln!(
        "probe: wireframe {:?} ({} edges, {} vertices)",
        t.elapsed(),
        wire.edges.len(),
        wire.vertices.len()
    );

    let t = Instant::now();
    let brep = crate::brep::weaving::finish_weave(wire, &body);
    eprintln!("probe: finish_weave {:?} ({} faces)", t.elapsed(), brep.faces.len());
}

/// Diagnostic dump for the degenerate vase B-Rep edges (figure-eight at
/// the neck, jittery rim/foot circles): weave the SHELLED vase exactly
/// like `Body::ensure_brep` and print every edge's branch signature,
/// sample count and geometric extents. Not a regression test.
#[test]
#[ignore = "manual diagnostic dump, run with --ignored --nocapture"]
fn vase_shelled_brep_edge_dump() {
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    let (sketch_call, revolve_call) = vase_sketch_and_revolve_calls();
    registry
        .regenerate(&mut ctx, &[sketch_call.clone(), revolve_call.clone()], None)
        .unwrap();
    let mut top_hash = None;
    for (brep, _) in ctx.brep_cache.ensure(&ctx.features) {
        for i in 0..brep.faces.len() {
            let fid = crate::brep::FaceId(i as u32);
            if let crate::brep::SurfaceGeometry::Analytic { surface, transform } =
                &brep.face(fid).surface
            {
                if let fielddraft_protocol::HalfSpace::Plane { normal, .. } = surface {
                    if normal[1] > 0.99 && (transform.translate[1] - 68.0).abs() < 1e-3 {
                        top_hash =
                            Some(crate::brep::naming::TopoRef::from_face(brep, fid).id_hash);
                    }
                }
            }
        }
    }
    let top_hash = top_hash.expect("top rim face");
    let shell_call = FeatureCall::new(
        "shell",
        id(3),
        json!({
            "open_faces": format!("qFace({top_hash})"),
            "thickness": { "value": 2.0, "unit": "mm" },
        }),
    );
    registry
        .regenerate(&mut ctx, &[sketch_call, revolve_call, shell_call], None)
        .unwrap();

    let solid = crate::csg::CsgNode::from_decoded(&ctx.features);
    let body = crate::bvh::BoundedCsg::from_csg(&solid);
    let bb = body.aabb();
    let size = bb.size();
    let largest = size[0].max(size[1]).max(size[2]);
    let min_cell = (largest / 32.0).clamp(1e-3, 10.0);
    let octree = crate::brep::octree::Octree::build(&body, min_cell);
    let wire = crate::brep::edge_walker::build_wireframe(&body, &octree);
    eprintln!(
        "dump: shelled vase wireframe {} edges, {} vertices",
        wire.edges.len(),
        wire.vertices.len()
    );
    for (i, e) in wire.edges.iter().enumerate() {
        // Hermite tangent sanity: the NURBS fit scales each sample's
        // tangent by the chord and trusts its SIGN. A tangent pointing
        // against the direction of travel (dot(tangent, chord) < 0)
        // makes the fitted Bézier loop backwards — count those.
        if let crate::brep::EdgeGeometry::Hermite(s) = &e.geometry {
            let mut backwards = 0usize;
            for w in s.windows(2) {
                let chord = [
                    w[1].point[0] - w[0].point[0],
                    w[1].point[1] - w[0].point[1],
                    w[1].point[2] - w[0].point[2],
                ];
                let d0 = w[0].tangent[0] * chord[0]
                    + w[0].tangent[1] * chord[1]
                    + w[0].tangent[2] * chord[2];
                if d0 < 0.0 {
                    backwards += 1;
                }
            }
            eprintln!("dump: edge {i}: {} backwards tangents of {}", backwards, s.len());
        }
        let pts: Vec<[f64; 3]> = match &e.geometry {
            crate::brep::EdgeGeometry::Hermite(s) => s.iter().map(|h| h.point).collect(),
            crate::brep::EdgeGeometry::Nurbs(n) => n.control_points.clone(),
            crate::brep::EdgeGeometry::Polyline(p) => p.clone(),
        };
        let (mut ymin, mut ymax) = (f64::INFINITY, f64::NEG_INFINITY);
        let (mut rmin, mut rmax) = (f64::INFINITY, f64::NEG_INFINITY);
        for p in &pts {
            let r = (p[0] * p[0] + p[2] * p[2]).sqrt();
            ymin = ymin.min(p[1]);
            ymax = ymax.max(p[1]);
            rmin = rmin.min(r);
            rmax = rmax.max(r);
        }
        let sig = e.signature;
        // Chain-order quality: consecutive step lengths. A well-ordered
        // closed rim chain steps ~2πr/n; long chords mean the greedy
        // spatial chaining jumped across the loop (the on-screen
        // "figure-eight").
        let mut max_step = 0.0f64;
        let mut long_steps = 0usize;
        let nominal = if pts.len() > 1 {
            let mut total = 0.0f64;
            for w in pts.windows(2) {
                let d = ((w[1][0] - w[0][0]).powi(2)
                    + (w[1][1] - w[0][1]).powi(2)
                    + (w[1][2] - w[0][2]).powi(2))
                .sqrt();
                total += d;
                max_step = max_step.max(d);
            }
            total / (pts.len() - 1) as f64
        } else {
            0.0
        };
        for w in pts.windows(2) {
            let d = ((w[1][0] - w[0][0]).powi(2)
                + (w[1][1] - w[0][1]).powi(2)
                + (w[1][2] - w[0][2]).powi(2))
            .sqrt();
            if d > 3.0 * nominal {
                long_steps += 1;
            }
        }
        // Angular ordering: a clean closed rim walks the circle
        // monotonically (|Σ dθ| ≈ 2π, no sign reversals). Reversals
        // mean the greedy chain doubles back — the fitted NURBS then
        // loops across the interior (the on-screen figure-eight).
        let mut reversals = 0usize;
        let mut total_dtheta = 0.0f64;
        let mut prev_dtheta = 0.0f64;
        for w in pts.windows(2) {
            let t0 = w[0][2].atan2(w[0][0]);
            let t1 = w[1][2].atan2(w[1][0]);
            let mut dt = t1 - t0;
            if dt > std::f64::consts::PI {
                dt -= std::f64::consts::TAU;
            } else if dt < -std::f64::consts::PI {
                dt += std::f64::consts::TAU;
            }
            if prev_dtheta != 0.0 && dt != 0.0 && dt.signum() != prev_dtheta.signum() {
                reversals += 1;
            }
            if dt != 0.0 {
                prev_dtheta = dt;
            }
            total_dtheta += dt;
        }
        eprintln!(
            "dump: edge {i}: sig=({}.{}, {}.{}) samples={} y=[{ymin:.2},{ymax:.2}] r=[{rmin:.2},{rmax:.2}] step_avg={nominal:.2} step_max={max_step:.2} long_steps={long_steps} dtheta_total={:.2} reversals={reversals} start={:?} end={:?}",
            sig.0.feature_id(),
            sig.0.sub(),
            sig.1.feature_id(),
            sig.1.sub(),
            pts.len(),
            total_dtheta,
            e.start,
            e.end,
        );
    }
    let brep = crate::brep::weaving::finish_weave(wire, &body);
    eprintln!("dump: finished brep: {} faces, {} edges", brep.faces.len(), brep.edges.len());
    for (i, f) in brep.faces.iter().enumerate() {
        let kind = match &f.surface {
            crate::brep::SurfaceGeometry::Analytic { surface, .. } => format!("{surface:?}"),
            crate::brep::SurfaceGeometry::Tessellated { .. } => "tessellated".to_string(),
        };
        let kind_short: String = kind.chars().take(60).collect();
        eprintln!(
            "dump: face {i}: branch={}.{} outer_loop={} inner={} surface={kind_short}",
            f.branch_id.feature_id(),
            f.branch_id.sub(),
            f.outer.edges.len(),
            f.inner.len(),
        );
    }

    // The viewport rides `Body::ensure_brep` with parametric_weave=true —
    // dump THAT route too (branch-seed weave with octree fallback), since
    // it is what bodyBrepEdges actually shows.
    match crate::brep::weaving::weave_parametric(&body) {
        None => eprintln!("dump: weave_parametric returned None (octree fallback)"),
        Some(pbrep) => {
            eprintln!(
                "dump: PARAMETRIC brep: {} faces, {} edges, {} vertices",
                pbrep.faces.len(),
                pbrep.edges.len(),
                pbrep.vertices.len()
            );
            for (i, e) in pbrep.edges.iter().enumerate() {
                let pts: Vec<[f64; 3]> = match &e.geometry {
                    crate::brep::EdgeGeometry::Hermite(s) => {
                        s.iter().map(|h| h.point).collect()
                    }
                    crate::brep::EdgeGeometry::Nurbs(n) => n.control_points.clone(),
                    crate::brep::EdgeGeometry::Polyline(p) => p.clone(),
                };
                let (mut ymin, mut ymax) = (f64::INFINITY, f64::NEG_INFINITY);
                let (mut rmin, mut rmax) = (f64::INFINITY, f64::NEG_INFINITY);
                for p in &pts {
                    let r = (p[0] * p[0] + p[2] * p[2]).sqrt();
                    ymin = ymin.min(p[1]);
                    ymax = ymax.max(p[1]);
                    rmin = rmin.min(r);
                    rmax = rmax.max(r);
                }
                let sig = e.signature;
                eprintln!(
                    "dump: P-edge {i}: sig=({}.{}, {}.{}) samples={} y=[{ymin:.2},{ymax:.2}] r=[{rmin:.2},{rmax:.2}] start={:?} end={:?}",
                    sig.0.feature_id(),
                    sig.0.sub(),
                    sig.1.feature_id(),
                    sig.1.sub(),
                    pts.len(),
                    e.start,
                    e.end,
                );
            }
        }
    }
}

/// The vase exercise's sketch + revolve calls, shared by the E2E test and
/// the timing probe.
fn vase_sketch_and_revolve_calls() -> (FeatureCall, FeatureCall) {
    let sketch_call = FeatureCall::new(
        "sketch",
        id(1),
        json!({
            "plane": "qCreatedBy(origin_plane_xy)",
            "sub_calls": [
                { "type": "sketch_point", "id": "p1", "params": { "at": [0.0, -70.0] } },
                { "type": "sketch_point", "id": "p2", "params": { "at": [0.0, 68.0] } },
                { "type": "sketch_point", "id": "p3", "params": { "at": [45.0, 68.0] } },
                { "type": "sketch_point", "id": "p4", "params": { "at": [30.0, -62.0] } },
                { "type": "sketch_point", "id": "p5", "params": { "at": [26.0, -70.0] } },
                { "type": "sketch_line", "id": "l1", "params": { "start": [0.0, -70.0], "end": [0.0, 68.0] } },
                { "type": "sketch_line", "id": "l2", "params": { "start": [0.0, 68.0], "end": [45.0, 68.0] } },
                { "type": "sketch_line", "id": "l3", "params": { "start": [30.0, -62.0], "end": [26.0, -70.0] } },
                { "type": "sketch_line", "id": "l4", "params": { "start": [26.0, -70.0], "end": [0.0, -70.0] } },
                { "type": "sketch_spline", "id": "s1", "params": {
                    "points": [45.0, 68.0, 33.044, 50.471, 26.707, 42.324, 38.476, 20.596, 50.245, -9.732, 44.360, -45.039, 30.0, -62.0],
                    "closed": false, "mode": "interpolate" } }
            ]
        }),
    );
    // ni56: revolve requires an explicit axis. l1 (0,−70)→(0,68) is the
    // x=0 local-Y axis line, entity id 8 in the solved sketch.
    let revolve_call = FeatureCall::new("revolve", id(2), json!({ "profiles": "qCreatedBy(1)", "axis": "qSketchLine(1, 8)" }));
    (sketch_call, revolve_call)
}

/// Regenerates the canonical shelled vase (sketch + revolve + 2 mm shell
/// opened on the planar top rim face) into `ctx` and returns the three
/// feature calls (so follow-up tests can append to the timeline). The
/// shell needs the woven top face's stable hash, so this pays one full
/// weave.
fn regen_shelled_vase(registry: &FeatureRegistry, ctx: &mut Context<'_>) -> Vec<FeatureCall> {
    let (sketch_call, revolve_call) = vase_sketch_and_revolve_calls();
    registry
        .regenerate(ctx, &[sketch_call.clone(), revolve_call.clone()], None)
        .unwrap();
    let mut top_hash = None;
    for (brep, _) in ctx.brep_cache.ensure(&ctx.features) {
        for i in 0..brep.faces.len() {
            let fid = crate::brep::FaceId(i as u32);
            if let crate::brep::SurfaceGeometry::Analytic { surface, transform } =
                &brep.face(fid).surface
            {
                if let fielddraft_protocol::HalfSpace::Plane { normal, .. } = surface {
                    if normal[1] > 0.99 && (transform.translate[1] - 68.0).abs() < 1e-3 {
                        top_hash =
                            Some(crate::brep::naming::TopoRef::from_face(brep, fid).id_hash);
                    }
                }
            }
        }
    }
    let top_hash = top_hash.expect("weave exposes the planar top rim face");
    let shell_call = FeatureCall::new(
        "shell",
        id(3),
        json!({
            "open_faces": format!("qFace({top_hash})"),
            "thickness": { "value": 2.0, "unit": "mm" },
        }),
    );
    // The intermediate regen above (sketch + revolve, run only to expose
    // the rim face for the shell query) left its decoded features in
    // `ctx`. Production runs every regenerate on a FRESH Context (see
    // `Kernel::bootstrap_features_from_calls`), so mirror that here:
    // without the reset the revolve is decoded twice and — now that
    // touching solids no longer auto-merge — partitions into two bodies.
    ctx.features.clear();
    ctx.edge_fillets.clear();
    ctx.mate_connectors.clear();
    let calls = vec![sketch_call, revolve_call, shell_call];
    registry.regenerate(ctx, &calls, None).unwrap();
    calls
}

/// The GPU SDF-VM consumes the body's compiled opcode stream. This walks
/// the exact same path (partition → build_bounded_for_body → compile_body)
/// and probes the CPU-mirror interpreter `eval_opcodes`: the shell opening
/// and cavity must survive compilation into the stream the raymarcher
/// sees, the stream must match the logical SDF, and it must stay small
/// enough to raymarch interactively.
#[test]
fn vase_sdf_vm_opcode_stream_keeps_shell_open() {
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    regen_shelled_vase(&registry, &mut ctx);

    let parts = crate::body::partition(&ctx.features);
    assert_eq!(parts.len(), 1, "one vase body");
    let bounded =
        crate::body::build_bounded_for_body(&ctx.features, &parts[0].feature_indices);
    let compiled = crate::sdf_compiler::compile_body(&bounded);
    let d = |p: [f32; 3]| crate::sdf_compiler::eval_opcodes(&compiled.opcodes, p);

    // Shape probes: the shell opening, cavity and walls must survive
    // compilation into the stream the raymarcher sees. (Before
    // OP_PRIM2D_PATH the flattened profile polygon closed itself with a
    // synthetic edge ON the revolve axis, which Shell inflated into a
    // phantom 2 mm rod: d(1,0,0) was −1.0.)
    assert!(d([0.0, 0.0, 0.0]) > 0.0, "cavity carved out: {}", d([0.0, 0.0, 0.0]));
    assert!(d([1.0, 0.0, 0.0]) > 0.0, "no phantom axis rod: {}", d([1.0, 0.0, 0.0]));
    assert!(d([0.0, 67.0, 0.0]) > 0.0, "top rim open: {}", d([0.0, 67.0, 0.0]));
    // Retained-boundary erosion: the wall keeps its FULL 2 mm thickness up
    // to the mouth (no knife-edge taper — the user-visible "Singularität
    // zum Deckel"). 0.5 mm inside the outer flare at y = 67.5 is solid;
    // radially well inside the inner wall it is open.
    assert!(d([44.2, 67.5, 0.0]) < 0.0, "rim band solid: {}", d([44.2, 67.5, 0.0]));
    assert!(d([40.0, 67.5, 0.0]) > 0.0, "mouth open inside rim: {}", d([40.0, 67.5, 0.0]));
    assert!(d([49.0, -10.0, 0.0]) < 0.0, "belly wall solid: {}", d([49.0, -10.0, 0.0]));
    assert!(d([0.0, -69.0, 0.0]) < 0.0, "bottom closed: {}", d([0.0, -69.0, 0.0]));
    assert!(d([60.0, 0.0, 0.0]) > 0.0, "outside: {}", d([60.0, 0.0, 0.0]));

    // Full parity with the logical CSG SDF over a deterministic grid of
    // the body AABB — opcode stream and kernel evaluation must be the
    // same field, not approximations of each other.
    let logical = crate::csg::CsgNode::from_decoded(&ctx.features);
    let (lo, hi) = (compiled.aabb.min, compiled.aabb.max);
    let n = 16;
    for iz in 0..=n {
        for iy in 0..=n {
            for ix in 0..=n {
                let p = [
                    lo[0] + (hi[0] - lo[0]) * ix as f32 / n as f32,
                    lo[1] + (hi[1] - lo[1]) * iy as f32 / n as f32,
                    lo[2] + (hi[2] - lo[2]) * iz as f32 / n as f32,
                ];
                let dv = d(p);
                let lv = logical.sdf(p, crate::csg::EvalMode::Logical);
                assert!(
                    (dv - lv).abs() < 1e-3,
                    "opcode/logical SDF diverge at {p:?}: {dv} vs {lv}",
                );
            }
        }
    }

    // Interactivity budget: the analytic path records keep the stream
    // small (the flattened polyline was ~4650 floats and raymarched at
    // ~2 FPS).
    assert!(
        compiled.opcodes.len() < 2048,
        "opcode stream too large: {} floats",
        compiled.opcodes.len(),
    );
}

/// Stage A of the revolve seed-weave (Parametric-Weaving rebuild): every
/// wall group of the shelled vase's revolve — including the curved spline
/// groups that previously fell out of the map entirely — carries an
/// analytic half-space, and the new `RevolvedProfile` carriers' zero set
/// lies ON the body surface (outer wall) resp. exactly `thickness` inside
/// it (shell inner wall).
#[test]
fn vase_revolve_branch_map_covers_every_wall_group() {
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    regen_shelled_vase(&registry, &mut ctx);

    let parts = crate::body::partition(&ctx.features);
    assert_eq!(parts.len(), 1, "one vase body");
    let bounded =
        crate::body::build_bounded_for_body(&ctx.features, &parts[0].feature_indices);

    let map = crate::brep::weaving::branch_halfspace_map(&bounded);
    let branches = crate::brep::branch_pairs::enumerate_branches(&bounded);
    assert!(!branches.is_empty(), "vase enumerates revolve wall branches");
    for b in &branches {
        assert!(
            map.contains_key(&b.id.as_u32()),
            "branch {} ({:?}) missing from the half-space map",
            b.id.as_u32(),
            b.kind
        );
    }

    // The spline wall groups must be present as RevolvedProfile carriers
    // (offset 0 = the outer surface), and the shell must have registered
    // their inner walls (same segments, offset = 2 mm).
    let logical = crate::csg::CsgNode::from_decoded(&ctx.features);
    let mut profile_carriers = 0;
    let mut inner_profile_carriers = 0;
    for (key, (hs, xf)) in map.iter() {
        let fielddraft_protocol::HalfSpace::RevolvedProfile {
            segments, ccw, offset, extend_start, extend_end,
        } = hs
        else {
            continue;
        };
        let is_inner = crate::csg::BranchId(*key).is_inner_wall();
        if is_inner {
            assert!((offset - 2.0).abs() < 1e-6, "inner wall offset is the shell thickness");
            inner_profile_carriers += 1;
        } else {
            assert_eq!(*offset, 0.0, "outer wall carrier has no offset");
            profile_carriers += 1;
        }

        for seg in segments {
            for t in [0.15f32, 0.5, 0.85] {
                let p2 = seg.point_at(t);
                // Inner-wall sample: step `offset` along the material-side
                // normal so the point sits on the offset surface.
                let (_, n2) =
                    fielddraft_protocol::sketch2d::group_signed_distance_and_normal(
                        segments, *ccw, p2, *extend_start, *extend_end,
                    );
                let q2 = [p2[0] - offset * n2[0], p2[1] - offset * n2[1]];
                for theta in [0.0f32, 1.3, 2.9] {
                    let (s, c) = theta.sin_cos();
                    let local = [q2[0] * c, q2[1], q2[0] * s];
                    let carrier = crate::csg::sdf_halfspace(hs, local);
                    assert!(
                        carrier.abs() < 1e-3,
                        "carrier {key} not zero on its own surface: {carrier} at {local:?} \
                         (p2={p2:?} n2={n2:?} q2={q2:?} seg={seg:?} ccw={ccw} offset={offset})"
                    );
                    if !is_inner {
                        let r = crate::quat_rotate(xf.rotation, [
                            local[0] * xf.scale[0],
                            local[1] * xf.scale[1],
                            local[2] * xf.scale[2],
                        ]);
                        let world = [
                            r[0] + xf.translate[0],
                            r[1] + xf.translate[1],
                            r[2] + xf.translate[2],
                        ];
                        let body = logical.sdf(world, crate::csg::EvalMode::Logical);
                        assert!(
                            body.abs() < 1e-2,
                            "outer carrier {key} point not on body surface: {body} at {world:?}"
                        );
                    }
                }
            }
        }
    }
    assert!(
        profile_carriers >= 1,
        "the spline wall groups must yield RevolvedProfile carriers"
    );
    assert!(
        inner_profile_carriers >= 1,
        "the shell must register inner-wall RevolvedProfile carriers"
    );
}

/// User-reported regression (2026-06-12): face-extruding the opened
/// vase's rim annulus froze the app for ~30 s (the combined body bailed
/// to the octree over non-coaxial RevolvedProfile × extrude-cylinder
/// pairs — the circle-wall carrier was emitted as a SIDEWAYS cylinder,
/// `InfiniteCylinder`'s axis is local Y, the extrusion runs along local
/// Z) and the extruded face's B-Rep collapsed to a point through that
/// same broken carrier. With the axis folded into the face frame the
/// body weaves analytically: every edge an exact ring, including the
/// tube's top rim circles.
#[test]
fn vase_face_extrude_on_rim_weaves_analytically_with_exact_rings() {
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    regen_shelled_vase(&registry, &mut ctx);

    // The rim annulus face (plane +Y at y = 68) — the face the user picked.
    let mut top_hash = None;
    for (brep, _) in ctx.brep_cache.ensure(&ctx.features) {
        for i in 0..brep.faces.len() {
            let fid = crate::brep::FaceId(i as u32);
            if let crate::brep::SurfaceGeometry::Analytic { surface, transform } =
                &brep.face(fid).surface
            {
                if let fielddraft_protocol::HalfSpace::Plane { normal, .. } = surface {
                    if normal[1] > 0.99 && (transform.translate[1] - 68.0).abs() < 1e-3 {
                        top_hash =
                            Some(crate::brep::naming::TopoRef::from_face(brep, fid).id_hash);
                    }
                }
            }
        }
    }
    let top_hash = top_hash.expect("rim annulus face present");

    let (sketch_call, revolve_call) = vase_sketch_and_revolve_calls();
    let calls = vec![
        sketch_call,
        revolve_call,
        FeatureCall::new(
            "shell",
            id(3),
            json!({
                "open_faces": format!("qFace({top_hash})"),
                "thickness": { "value": 2.0, "unit": "mm" },
            }),
        ),
        FeatureCall::new(
            "extrude",
            id(4),
            json!({
                "profiles": format!("qFace({top_hash})"),
                "depth": { "value": 10.0, "unit": "mm" },
                "symmetric": false,
                // Rim tube grows the vase body (Add); yx5h default is New.
                "operation": "add",
            }),
        ),
    ];
    registry.regenerate(&mut ctx, &calls, None).unwrap();

    let parts = crate::body::partition(&ctx.features);
    assert_eq!(parts.len(), 1, "extrude docks onto the vase body");
    let bounded =
        crate::body::build_bounded_for_body(&ctx.features, &parts[0].feature_indices);
    let brep = crate::brep::seed_weaver::weave_branch_seeds(&bounded)
        .expect("vase + rim-tube must weave via the analytic seed path (octree took >70 s)");
    assert!(brep.validate().is_ok(), "combined B-Rep must be well-formed");

    let mut rings: Vec<(f32, f32)> = Vec::new();
    for e in &brep.edges {
        let samples = crate::brep::edge_sampling::sample_edge(&e.geometry);
        let (mut rmin, mut rmax) = (f32::INFINITY, f32::NEG_INFINITY);
        let (mut ymin, mut ymax) = (f32::INFINITY, f32::NEG_INFINITY);
        for p in &samples {
            let r = ((p[0] * p[0] + p[2] * p[2]) as f32).sqrt();
            rmin = rmin.min(r);
            rmax = rmax.max(r);
            ymin = ymin.min(p[1] as f32);
            ymax = ymax.max(p[1] as f32);
        }
        assert!(
            rmax - rmin < 0.05 && ymax - ymin < 0.05,
            "edge is not a clean ring: r ∈ [{rmin}, {rmax}], y ∈ [{ymin}, {ymax}]"
        );
        rings.push((0.5 * (ymin + ymax), 0.5 * (rmin + rmax)));
    }
    let has_ring = |y: f32, r: f32| {
        rings.iter().any(|&(ry, rr)| (ry - y).abs() < 0.05 && (rr - r).abs() < 0.05)
    };
    // Tube top rim (the face the user saw collapse to a point).
    assert!(has_ring(78.0, 45.0), "tube outer top rim missing: {rings:?}");
    assert!(has_ring(78.0, 42.58), "tube inner top rim missing: {rings:?}");
    // Vase rim circles survive at the joint.
    assert!(has_ring(68.0, 45.0), "vase outer rim missing: {rings:?}");
    assert!(has_ring(68.0, 42.58), "vase inner rim missing: {rings:?}");
}

/// The Option-(c) target (project_vase_brep_edge_degeneration): the
/// CLOSED-shelled vase weaves over the analytic branch-seed path, and
/// every B-Rep edge is an exact circle — constant radius and height along
/// the whole ring. The octree route's rim jitter (r wandering 44.1…45
/// through the blind `edge_residual`) cannot pass the 0.05 mm bound here.
///
/// The OPENED vase weaves over the seed path too (retained-boundary shell
/// erosion): constant 2 mm wall up to the mouth, where the lid annulus
/// meets the inner wall in the inner rim circle (r ≈ 42.58: 2 mm
/// perpendicular to the tilted spline wall = 2/sin α in the rim plane).
#[test]
fn vase_seed_weave_yields_exact_circle_edges() {
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    let (sketch_call, revolve_call) = vase_sketch_and_revolve_calls();
    registry
        .regenerate(&mut ctx, &[sketch_call, revolve_call], None)
        .unwrap();
    // The shell FEATURE refuses to run without picked open faces (the
    // dialog-in-progress contract), so the closed shell is wrapped at CSG
    // level — the same node the feature would emit, minus the openings.
    let solid = crate::csg::CsgNode::from_decoded(&ctx.features);
    let shelled = crate::csg::CsgNode::Shell(Box::new(solid), 2.0, vec![]);
    let bounded = crate::bvh::BoundedCsg::from_csg(&shelled);

    let brep = crate::brep::seed_weaver::weave_branch_seeds(&bounded)
        .expect("the closed-shelled vase must weave via the analytic seed path");
    assert!(brep.validate().is_ok(), "vase B-Rep must be well-formed");
    assert!(
        brep.shells.iter().any(|s| s.is_closed),
        "vase seed weave must be watertight"
    );

    // Every edge is a closed ring of constant (radius, height).
    let mut rings: Vec<(f32, f32)> = Vec::new(); // (y, r)
    for e in &brep.edges {
        assert!(
            e.start.is_none() && e.end.is_none(),
            "a coaxial body has only closed ring edges"
        );
        let samples = crate::brep::edge_sampling::sample_edge(&e.geometry);
        assert!(samples.len() >= 8, "ring edge has a dense sampling");
        let (mut rmin, mut rmax) = (f32::INFINITY, f32::NEG_INFINITY);
        let (mut ymin, mut ymax) = (f32::INFINITY, f32::NEG_INFINITY);
        for p in &samples {
            let r = ((p[0] * p[0] + p[2] * p[2]) as f32).sqrt();
            rmin = rmin.min(r);
            rmax = rmax.max(r);
            ymin = ymin.min(p[1] as f32);
            ymax = ymax.max(p[1] as f32);
        }
        assert!(
            rmax - rmin < 0.05 && ymax - ymin < 0.05,
            "edge is not a clean circle: r ∈ [{rmin}, {rmax}], y ∈ [{ymin}, {ymax}]"
        );
        rings.push((0.5 * (ymin + ymax), 0.5 * (rmin + rmax)));
    }

    let has_ring = |y: f32, r: f32, tol: f32| {
        rings.iter().any(|&(ry, rr)| (ry - y).abs() < tol && (rr - r).abs() < tol)
    };
    // Outer rim circle at exactly (y=68, r=45) — THE jitter regression
    // bound — plus the foot ring and the spline∩cone knick ring.
    assert!(has_ring(68.0, 45.0, 0.05), "outer rim circle r=45 missing: {rings:?}");
    assert!(has_ring(-70.0, 26.0, 0.05), "foot circle r=26 missing: {rings:?}");
    assert!(has_ring(-62.0, 30.0, 0.05), "knick ring r=30 missing: {rings:?}");
    // Cavity rings of the closed shell: the ceiling corner at y=66 (2 mm
    // below the top) and the floor corner at y=−68.
    assert!(
        rings.iter().any(|&(ry, rr)| (ry - 66.0).abs() < 0.1 && (38.0..45.0).contains(&rr)),
        "cavity ceiling ring near y=66 missing: {rings:?}"
    );
    assert!(
        rings.iter().any(|&(ry, rr)| (ry + 68.0).abs() < 0.1 && (20.0..29.0).contains(&rr)),
        "cavity floor ring near y=−68 missing: {rings:?}"
    );

    // The OPENED vase weaves over the seed path as well — exact rings,
    // including the NEW inner rim circle where the lid annulus meets the
    // inner wall (retained-boundary erosion, no knife edge).
    let mut doc2 = empty_ctx();
    let mut ctx2 = Context::new(&mut doc2);
    regen_shelled_vase(&registry, &mut ctx2);
    let parts2 = crate::body::partition(&ctx2.features);
    let bounded2 =
        crate::body::build_bounded_for_body(&ctx2.features, &parts2[0].feature_indices);
    let brep2 = crate::brep::seed_weaver::weave_branch_seeds(&bounded2)
        .expect("the opened vase must weave via the analytic seed path");
    assert!(brep2.validate().is_ok(), "opened-vase B-Rep must be well-formed");

    let mut rings2: Vec<(f32, f32)> = Vec::new();
    for e in &brep2.edges {
        let samples = crate::brep::edge_sampling::sample_edge(&e.geometry);
        assert!(samples.len() >= 8, "ring edge has a dense sampling");
        let (mut rmin, mut rmax) = (f32::INFINITY, f32::NEG_INFINITY);
        let (mut ymin, mut ymax) = (f32::INFINITY, f32::NEG_INFINITY);
        for p in &samples {
            let r = ((p[0] * p[0] + p[2] * p[2]) as f32).sqrt();
            rmin = rmin.min(r);
            rmax = rmax.max(r);
            ymin = ymin.min(p[1] as f32);
            ymax = ymax.max(p[1] as f32);
        }
        assert!(
            rmax - rmin < 0.05 && ymax - ymin < 0.05,
            "opened-vase edge is not a clean circle: r ∈ [{rmin}, {rmax}], y ∈ [{ymin}, {ymax}]"
        );
        rings2.push((0.5 * (ymin + ymax), 0.5 * (rmin + rmax)));
    }
    let has_ring2 = |y: f32, r: f32, tol: f32| {
        rings2.iter().any(|&(ry, rr)| (ry - y).abs() < tol && (rr - r).abs() < tol)
    };
    assert!(has_ring2(68.0, 45.0, 0.05), "outer rim circle r=45 missing: {rings2:?}");
    // Inner rim: the 2 mm wall measured PERPENDICULAR to the tilted
    // spline wall — in the rim plane that is 2/sin(α) ≈ 2.42 mm inward
    // of r=45 for this profile's start tangent.
    assert!(has_ring2(68.0, 42.58, 0.05), "inner rim circle r≈42.58 missing: {rings2:?}");
    assert!(has_ring2(-70.0, 26.0, 0.05), "foot circle r=26 missing: {rings2:?}");
    assert!(has_ring2(-62.0, 30.0, 0.05), "knick ring r=30 missing: {rings2:?}");
    assert!(
        rings2.iter().any(|&(ry, rr)| (ry + 68.0).abs() < 0.1 && (20.0..29.0).contains(&rr)),
        "cavity floor ring near y=−68 missing: {rings2:?}"
    );
}

/// The user's cutaway: the opened, shelled vase MINUS a box exposing the
/// cavity. Every box plane is non-coaxial with the revolve axis, so the
/// plane×RevolvedProfile pairs run the general SSI scan with the profile
/// as the implicit operand — before that scan arm existed, ANY such pair
/// reported `Unsupported` and the whole body bailed to the ~25 s octree
/// route (all-or-nothing contract).
#[test]
fn vase_cutaway_with_box_weaves_via_seed_path() {
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    let mut calls = regen_shelled_vase(&registry, &mut ctx);
    // Box buried into the +X side: covers x ∈ [18, 102], well past the
    // belly (r_max ≈ 50.2) and both rims, |z| ≤ 42, full height.
    calls.push(FeatureCall::new(
        "box",
        id(4),
        json!({
            "half_extents": [42.0, 85.0, 42.0],
            "translate": [60.0, 0.0, 0.0],
            "op": "remove",
            "targets": "qCreatedBy(2)",
        }),
    ));
    // Fresh decode (mirror production's per-regen Context) — the helper
    // already populated `ctx` once; re-running the full call list without
    // clearing would decode the revolve twice and, since touching solids
    // no longer auto-merge, split the vase into duplicate bodies.
    ctx.features.clear();
    ctx.edge_fillets.clear();
    ctx.mate_connectors.clear();
    registry.regenerate(&mut ctx, &calls, None).unwrap();

    let parts = crate::body::partition(&ctx.features);
    assert_eq!(parts.len(), 1, "one cutaway body");
    let bounded =
        crate::body::build_bounded_for_body(&ctx.features, &parts[0].feature_indices);
    let brep = crate::brep::seed_weaver::weave_branch_seeds(&bounded)
        .expect("the box-cutaway vase must weave via the analytic seed path");
    assert!(brep.validate().is_ok(), "cutaway B-Rep must be well-formed");

    // The cut plane x=18 slices the (revolved) walls: the seam edges live
    // ON that plane, are non-circular, and are TRIMMED arcs (the box cut
    // opens every former ring) — so both open edges with endpoints and
    // edges pinned to the cut plane must exist.
    let mut has_open_edge = false;
    let mut has_cut_plane_edge = false;
    let mut has_trimmed_rim_arc = false;
    for e in &brep.edges {
        let samples = crate::brep::edge_sampling::sample_edge(&e.geometry);
        if samples.is_empty() {
            continue;
        }
        if e.start.is_some() || e.end.is_some() {
            has_open_edge = true;
        }
        let on_cut = samples.iter().all(|p| (p[0] - 18.0).abs() < 0.05);
        if on_cut && samples.len() >= 8 {
            has_cut_plane_edge = true;
        }
        // The outer rim circle (y=68, r=45) survives as a trimmed arc.
        let on_rim = samples.iter().all(|p| {
            let r = ((p[0] * p[0] + p[2] * p[2]) as f32).sqrt();
            (p[1] as f32 - 68.0).abs() < 0.05 && (r - 45.0).abs() < 0.05
        });
        if on_rim && (e.start.is_some() || e.end.is_some()) {
            has_trimmed_rim_arc = true;
        }
    }
    assert!(has_open_edge, "box cut must trim former rings into open arcs");
    assert!(
        has_cut_plane_edge,
        "a seam edge on the cut plane x=18 (plane × revolved wall) is missing"
    );
    assert!(
        has_trimmed_rim_arc,
        "the outer rim r=45 must survive as a trimmed arc at y=68"
    );
}

#[test]
fn vase_from_free_lines_and_spline_revolves_and_shells() {
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    let sketch_call = FeatureCall::new(
        "sketch",
        id(1),
        json!({
            "plane": "qCreatedBy(origin_plane_xy)",
            "sub_calls": [
                { "type": "sketch_point", "id": "p1", "params": { "at": [0.0, -70.0] } },
                { "type": "sketch_point", "id": "p2", "params": { "at": [0.0, 68.0] } },
                { "type": "sketch_point", "id": "p3", "params": { "at": [45.0, 68.0] } },
                { "type": "sketch_point", "id": "p4", "params": { "at": [30.0, -62.0] } },
                { "type": "sketch_point", "id": "p5", "params": { "at": [26.0, -70.0] } },
                { "type": "sketch_line", "id": "l1", "params": { "start": [0.0, -70.0], "end": [0.0, 68.0] } },
                { "type": "sketch_line", "id": "l2", "params": { "start": [0.0, 68.0], "end": [45.0, 68.0] } },
                { "type": "sketch_line", "id": "l3", "params": { "start": [30.0, -62.0], "end": [26.0, -70.0] } },
                { "type": "sketch_line", "id": "l4", "params": { "start": [26.0, -70.0], "end": [0.0, -70.0] } },
                { "type": "sketch_spline", "id": "s1", "params": {
                    "points": [45.0, 68.0, 33.044, 50.471, 26.707, 42.324, 38.476, 20.596, 50.245, -9.732, 44.360, -45.039, 30.0, -62.0],
                    "closed": false, "mode": "interpolate" } }
            ]
        }),
    );
    // ni56: revolve requires an explicit axis. l1 (0,−70)→(0,68) is the
    // x=0 local-Y axis line, entity id 8 in the solved sketch.
    let revolve_call = FeatureCall::new("revolve", id(2), json!({ "profiles": "qCreatedBy(1)", "axis": "qSketchLine(1, 8)" }));
    registry
        .regenerate(&mut ctx, &[sketch_call.clone(), revolve_call.clone()], None)
        .unwrap();

    // The solid before shelling: interior filled, foot present (the
    // loop-inference regression), nothing outside the silhouette.
    let solid = crate::csg::CsgNode::from_decoded(&ctx.features);
    let sdf = |t: &crate::csg::CsgNode, p: [f32; 3]| t.sdf(p, crate::csg::EvalMode::Logical);
    assert!(sdf(&solid, [0.0, 0.0, 0.0]) <= 0.0, "core solid");
    assert!(sdf(&solid, [49.0, -10.0, 0.0]) < 0.0, "belly solid");
    assert!(sdf(&solid, [20.0, -69.0, 0.0]) < 0.0, "foot present");
    assert!(sdf(&solid, [60.0, 0.0, 0.0]) > 0.0, "outside");
    assert!(sdf(&solid, [40.0, 42.0, 0.0]) > 0.0, "neck waist clears");

    // Find the planar top rim face (normal +Y at y = 68) in the weave —
    // the face a user would pick to open the vase.
    let mut top_hash = None;
    for (brep, _) in ctx.brep_cache.ensure(&ctx.features) {
        for i in 0..brep.faces.len() {
            let fid = crate::brep::FaceId(i as u32);
            if let crate::brep::SurfaceGeometry::Analytic { surface, transform } =
                &brep.face(fid).surface
            {
                if let fielddraft_protocol::HalfSpace::Plane { normal, .. } = surface {
                    if normal[1] > 0.99 && (transform.translate[1] - 68.0).abs() < 1e-3 {
                        top_hash =
                            Some(crate::brep::naming::TopoRef::from_face(brep, fid).id_hash);
                    }
                }
            }
        }
    }
    let top_hash = top_hash.expect("weave exposes the planar top rim face");

    let shell_call = FeatureCall::new(
        "shell",
        id(3),
        json!({
            "open_faces": format!("qFace({top_hash})"),
            "thickness": { "value": 2.0, "unit": "mm" },
        }),
    );
    registry
        .regenerate(&mut ctx, &[sketch_call, revolve_call, shell_call], None)
        .unwrap();

    let hollow = crate::csg::CsgNode::from_decoded(&ctx.features);
    assert!(sdf(&hollow, [0.0, 0.0, 0.0]) > 0.0, "core carved out");
    assert!(sdf(&hollow, [0.0, 67.0, 0.0]) > 0.0, "top rim is open");
    assert!(sdf(&hollow, [49.0, -10.0, 0.0]) < 0.0, "belly wall remains");
    assert!(sdf(&hollow, [0.0, -69.0, 0.0]) < 0.0, "bottom stays closed");
    assert!(sdf(&hollow, [60.0, 0.0, 0.0]) > 0.0, "outside stays outside");
}

#[test]
fn shell_feature_hollows_the_target_body_and_opens_the_picked_face() {
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    registry
        .regenerate(
            &mut ctx,
            &[
                box_call(1, 10.0),
                FeatureCall::new(
                    "shell",
                    id(2),
                    json!({
                        "open_faces": "qCreatedBy(1)",
                        "thickness": { "value": 2.0, "unit": "mm" },
                    }),
                ),
            ],
            None,
        )
        .unwrap();

    assert_eq!(ctx.features.len(), 2, "box + shell modifier");
    let shell = &ctx.features[1];
    assert!(shell.is_shell_modifier(), "second feature is the shell sentinel");
    assert!(shell.target_ids.contains(&id(1)), "targets the box body");
    match &shell.node {
        crate::csg::CsgNode::Shell(inner, t, open) => {
            assert!(matches!(**inner, crate::csg::CsgNode::Empty));
            assert!((t - 2.0).abs() < 1e-6, "thickness in mm");
            assert_eq!(open.len(), 1, "one open plane from the picked face");
        }
        other => panic!("expected Shell sentinel, got {other:?}"),
    }

    // The hollow is visible in the assembled tree (the centre of a solid
    // box would be deep inside; here it is carved away).
    let tree = crate::csg::CsgNode::from_decoded(&ctx.features);
    assert!(
        tree.sdf([0.0, 0.0, 0.0], crate::csg::EvalMode::Logical) > 0.0,
        "centre is hollow after shelling",
    );
}

#[test]
fn shell_feature_rejects_a_query_that_resolves_to_no_face() {
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    registry
        .regenerate(&mut ctx, std::slice::from_ref(&box_call(1, 10.0)), None)
        .unwrap();
    let err = registry
        .regenerate(
            &mut ctx,
            &[FeatureCall::new(
                "shell",
                id(2),
                json!({
                    "open_faces": "qFace(999999)",
                    "thickness": { "value": 2.0, "unit": "mm" },
                }),
            )],
            None,
        )
        .unwrap_err();
    assert!(
        format!("{err:?}").contains("face"),
        "error mentions the face param: {err:?}",
    );
}

fn count_masked_blends(node: &crate::csg::CsgNode) -> usize {
    use crate::csg::CsgNode;
    match node {
        CsgNode::MaskedBlend { a, b, .. } => {
            1 + count_masked_blends(a) + count_masked_blends(b)
        }
        CsgNode::Union(a, b)
        | CsgNode::Difference(a, b)
        | CsgNode::Intersection(a, b)
        | CsgNode::SmoothUnion(a, b, _, _)
        | CsgNode::SmoothDifference(a, b, _, _)
        | CsgNode::Morph(a, b, _) => count_masked_blends(a) + count_masked_blends(b),
        CsgNode::Transform(_, i)
        | CsgNode::Twist(i, _)
        | CsgNode::Bend(i, _)
        | CsgNode::Shell(i, _, _)
        | CsgNode::Displace(i, _, _) => count_masked_blends(i),
        _ => 0,
    }
}

/// Assert the fillet rounds the corner near `mid_world`. The blend
/// perturbs the SDF only inside the mask tube — so on a dense probe
/// cloud around the edge mid-span at least one point must read
/// SIGNIFICANTLY different from the un-filleted hard combine (the
/// direction depends on union vs difference: a union fillet pushes the
/// surface outward / more negative, a difference fillet fills the
/// concave corner / more positive). Crucially, far from the edge the
/// two SDFs must AGREE — that is what proves the polyline sits ON the
/// queried edge (in the right coordinate space) rather than drifting
/// off into empty space (which would leave the near-edge unchanged and
/// pass only the trivial far-field check).
fn assert_rounded_at_edge(
    hard: &crate::csg::CsgNode,
    filleted: &crate::csg::CsgNode,
    mid_world: [f32; 3],
) {
    use crate::csg::EvalMode;
    let r = 2.0_f32;
    let mut max_delta = 0.0_f32;
    for ix in -2..=2 {
        for iy in -2..=2 {
            for iz in -2..=2 {
                let p = [
                    mid_world[0] + ix as f32 * r * 0.5,
                    mid_world[1] + iy as f32 * r * 0.5,
                    mid_world[2] + iz as f32 * r * 0.5,
                ];
                let dh = hard.sdf(p, EvalMode::Visual);
                let df = filleted.sdf(p, EvalMode::Visual);
                max_delta = max_delta.max((df - dh).abs());
            }
        }
    }
    assert!(
        max_delta > 0.05,
        "fillet must perturb the SDF near the edge at {mid_world:?} (max |Δ| = {max_delta})",
    );

    // Far from the edge → identical (mask inactive). A polyline in the
    // wrong space would make THIS the only place the blend fires, so
    // pairing it with the near-edge perturbation above pins the space.
    let far = [mid_world[0] + 80.0, mid_world[1] + 80.0, mid_world[2] + 80.0];
    let dh_far = hard.sdf(far, EvalMode::Visual);
    let df_far = filleted.sdf(far, EvalMode::Visual);
    assert!(
        (dh_far - df_far).abs() < 1e-3,
        "far from the edge the SDF is unchanged: hard {dh_far} vs filleted {df_far}",
    );
}

#[test]
fn extrude_side_face_resolves_clean_profile_at_user_scale() {
    // Ticket vyee: the EXACT user document — 100 mm box, full-top-face
    // extrude (depth 52.91), then a face extrude of the box's +X side.
    // The octree weave collapses at this scale (truncated edges, missing
    // extrusion faces), so query resolution must ride the parametric
    // (branch-seed) weave: the side face resolves to a CONNECTED 100×100
    // profile with the +X plane normal, and the final solid grows +25 in X.
    let registry = FeatureRegistry::with_builtins();
    let mut doc = empty_ctx();
    let mut ctx = Context::new(&mut doc);
    let calls = vec![
        box_call(1, 50.0),
        FeatureCall::new(
            "extrude",
            id(5),
            json!({
                "profiles": "qFace(4315924989883055031)",
                "depth": { "value": 52.91257140451136, "unit": "mm" },
                "depth_back": { "value": 0.0, "unit": "mm" },
                "symmetric": false,
                // Face extrude that grows the parent box (Add); yx5h flips
                // the default to New, so pin Add to keep the merge.
                "operation": "add",
            }),
        ),
    ];
    registry.regenerate(&mut ctx, &calls, None).unwrap();

    {
        let brep = weave_first(&mut ctx);
        for i in 0..brep.face_count() {
            let fid = crate::brep::FaceId(i as u32);
            if crate::brep::naming::TopoRef::from_face(&brep, fid).id_hash
                != 6025886176426920981
            {
                continue;
            }
            let face = brep.face(fid);
            let mut loops: Vec<&crate::brep::Loop> = vec![&face.outer];
            loops.extend(face.inner.iter());
            for (li, lp) in loops.iter().enumerate() {
                eprintln!("-- face {i} loop {li}: {} edges", lp.edges.len());
                for (eid, rev) in &lp.edges {
                    let pl =
                        crate::brep::edge_sampling::sample_edge(&brep.edge(*eid).geometry);
                    let a = pl.first().cloned().unwrap_or_default();
                    let b = pl.last().cloned().unwrap_or_default();
                    eprintln!("   edge {eid:?} rev {rev} pts {} {a:?} -> {b:?}", pl.len());
                }
            }
        }
    }
    let (region, frame) = ctx
        .resolve_extrude_faces(&["qFace(6025886176426920981)".to_string()])
        .expect("ticket face must resolve");
    assert!(
        frame.normal[0] > 0.99,
        "side face must extrude along +X, got {:?}",
        frame.normal
    );
    assert_paths_connected(&region);
    let bb = region.aabb();
    let (w, h) = (bb.max[0] - bb.min[0], bb.max[1] - bb.min[1]);
    assert!((w - 100.0).abs() < 0.2, "side profile width {w} (aabb {bb:?})");
    assert!((h - 100.0).abs() < 0.2, "side profile height {h} (aabb {bb:?})");

    // Full doc incl. the 25 mm side extrude → the solid must actually grow
    // sideways (+X), not collapse (the octree route wove a y=-50 sliver).
    let mut calls2 = calls.clone();
    calls2.push(FeatureCall::new(
        "extrude",
        id(8),
        json!({
            "profiles": "qFace(6025886176426920981)",
            "depth": { "value": 25.0, "unit": "mm" },
            "depth_back": { "value": 0.0, "unit": "mm" },
            "symmetric": false,
        }),
    ));
    let mut doc2 = empty_ctx();
    let mut ctx2 = Context::new(&mut doc2);
    registry.regenerate(&mut ctx2, &calls2, None).unwrap();

    // The RENDERED solid (SDF) must grow sideways: a 25 mm slab on the +X
    // side face, NOT a sliver at the bottom (what the broken profile swept).
    let solid = crate::csg::CsgNode::from_decoded(&ctx2.features);
    let inside = [
        [60.0, 0.0, 0.0],    // middle of the new slab
        [74.0, 40.0, 40.0],  // near the slab's outer corner
        [0.0, 80.0, 0.0],    // original top extrude untouched
    ];
    for p in inside {
        let d = solid.sdf(p, crate::csg::EvalMode::Visual);
        assert!(d < 0.0, "{p:?} must be inside the solid, sdf {d}");
    }
    let outside = [
        [76.0, 0.0, 0.0],    // just past the slab
        [60.0, 55.0, 0.0],   // above the slab (it spans y ∈ [-50, 50] only)
        [0.0, -55.0, 0.0],   // below the body — nothing may grow downward
    ];
    for p in outside {
        let d = solid.sdf(p, crate::csg::EvalMode::Visual);
        assert!(d > 0.0, "{p:?} must be outside the solid, sdf {d}");
    }

    // The weave must at least keep the established geometry intact (the
    // octree route collapsed everything to a y=-50 sliver).
    let brep2 = weave_first(&mut ctx2);
    let mut mn = [f32::MAX; 3];
    let mut mx = [f32::MIN; 3];
    for v in &brep2.vertices {
        for k in 0..3 {
            mn[k] = mn[k].min(v.position[k] as f32);
            mx[k] = mx[k].max(v.position[k] as f32);
        }
    }
    assert!((mx[1] - 102.91257).abs() < 0.1, "top must stay at y=102.91, got {mx:?}");
    assert!((mn[1] + 50.0).abs() < 0.1, "bottom must stay at y=-50, got {mn:?}");
    let mut owners: std::collections::BTreeMap<u32, usize> = Default::default();
    for i in 0..brep2.face_count() {
        *owners
            .entry(brep2.face(crate::brep::FaceId(i as u32)).branch_id.feature_id())
            .or_default() += 1;
    }
    eprintln!("final brep: vert aabb {mn:?}..{mx:?} faces by owner {owners:?}");
}
