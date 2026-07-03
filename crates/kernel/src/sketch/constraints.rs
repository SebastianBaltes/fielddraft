//! Constraint-Tool-Definitionen (horizontal, vertical, coincident,
//! distance, angle, equal, concentric, fix, point_on_object, parallel,
//! collinear, perpendicular, tangent, midpoint, symmetric, radius,
//! diameter) via `constraint_tool!`-Makro.

use serde_json::Value;

use fielddraft_protocol::{FeatureId};
use fielddraft_solver::{Constraint, ConstraintId, EntityId, SketchState};

use crate::features::{FeatureError, FeatureSchema};

use super::tools::SketchToolKind;
// ─── K6b.3d: constraint tools ───────────────────────────────────
//
// K6b.3d teaches the sketch sub-spine to carry constraint calls
// alongside the geometric ones. Each constraint tool references
// sketch-local entity ids via raw `u32` params — the UI predicts
// those ids by replicating the kernel's entity allocation order
// in `buildDraftSketchState`, so the committed calls target the
// same entities the kernel will create at regeneration time.
//
// The constraints land in `SketchState::constraints`. Actual
// solving happens in JavaScript via the K5a `solveSketch`
// wrapper — the kernel just persists the constraint network so
// it survives save / load / edit cycles.

macro_rules! constraint_tool {
    ($def:ident, $tool:ident, |$st:ident, $d:ident| $body:expr) => {
        pub struct $tool;
        impl SketchToolKind for $tool {
            fn type_name(&self) -> &'static str {
                $def::TYPE_NAME
            }
            fn schema_json(&self) -> Value {
                $def::schema_json()
            }
            fn apply(
                &self,
                $st: &mut SketchState,
                _id: &FeatureId,
                def: &Value,
            ) -> Result<(), FeatureError> {
                let $d = $def::parse(def)?;
                $body;
                Ok(())
            }
        }
    };
}

#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_horizontal")]
pub struct SketchHorizontalConstraintDef {
    pub line: u32,
}
constraint_tool!(
    SketchHorizontalConstraintDef,
    SketchHorizontalConstraintTool,
    |state, d| state.add_constraint(|cid: ConstraintId| Constraint::Horizontal {
        id: cid,
        line: EntityId(d.line)
    })
);

#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_vertical")]
pub struct SketchVerticalConstraintDef {
    pub line: u32,
}
constraint_tool!(
    SketchVerticalConstraintDef,
    SketchVerticalConstraintTool,
    |state, d| state.add_constraint(|cid: ConstraintId| Constraint::Vertical {
        id: cid,
        line: EntityId(d.line)
    })
);

#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_coincident")]
pub struct SketchCoincidentConstraintDef {
    pub a: u32,
    pub b: u32,
}
constraint_tool!(
    SketchCoincidentConstraintDef,
    SketchCoincidentConstraintTool,
    |state, d| state.add_constraint(|cid: ConstraintId| Constraint::Coincident {
        id: cid,
        a: EntityId(d.a),
        b: EntityId(d.b)
    })
);

#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_distance")]
pub struct SketchDistanceConstraintDef {
    pub entity_a: u32,
    pub entity_b: u32,
    pub value_mm: f32,
}
constraint_tool!(
    SketchDistanceConstraintDef,
    SketchDistanceConstraintTool,
    |state, d| state.add_constraint(|cid: ConstraintId| Constraint::Distance {
        id: cid,
        a: EntityId(d.entity_a),
        b: EntityId(d.entity_b),
        value_mm: d.value_mm as f64,
        driven: false,
    })
);

#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_angle")]
pub struct SketchAngleConstraintDef {
    pub line_a: u32,
    pub line_b: u32,
    pub value_deg: f32,
}
constraint_tool!(
    SketchAngleConstraintDef,
    SketchAngleConstraintTool,
    |state, d| state.add_constraint(|cid: ConstraintId| Constraint::Angle {
        id: cid,
        a: EntityId(d.line_a),
        b: EntityId(d.line_b),
        value_deg: d.value_deg as f64,
        driven: false,
    })
);

#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_equal")]
pub struct SketchEqualConstraintDef {
    pub entity_a: u32,
    pub entity_b: u32,
}
constraint_tool!(
    SketchEqualConstraintDef,
    SketchEqualConstraintTool,
    |state, d| state.add_constraint(|cid: ConstraintId| Constraint::Equal {
        id: cid,
        a: EntityId(d.entity_a),
        b: EntityId(d.entity_b)
    })
);

#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_concentric")]
pub struct SketchConcentricConstraintDef {
    pub a: u32,
    pub b: u32,
}
constraint_tool!(
    SketchConcentricConstraintDef,
    SketchConcentricConstraintTool,
    |state, d| state.add_constraint(|cid: ConstraintId| Constraint::Concentric {
        id: cid,
        a: EntityId(d.a),
        b: EntityId(d.b)
    })
);

#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_fix")]
pub struct SketchFixConstraintDef {
    pub point: u32,
}
constraint_tool!(
    SketchFixConstraintDef,
    SketchFixConstraintTool,
    |state, d| state.add_constraint(|cid: ConstraintId| Constraint::Fix {
        id: cid,
        point: EntityId(d.point)
    })
);

#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_point_on_object")]
pub struct SketchPointOnObjectConstraintDef {
    pub point: u32,
    pub curve: u32,
}
constraint_tool!(
    SketchPointOnObjectConstraintDef,
    SketchPointOnObjectConstraintTool,
    |state, d| state.add_constraint(|cid: ConstraintId| Constraint::PointOnObject {
        id: cid,
        point: EntityId(d.point),
        curve: EntityId(d.curve)
    })
);

#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_parallel")]
pub struct SketchParallelConstraintDef {
    pub line_a: u32,
    pub line_b: u32,
}
constraint_tool!(
    SketchParallelConstraintDef,
    SketchParallelConstraintTool,
    |state, d| state.add_constraint(|cid| Constraint::Parallel {
        id: cid,
        a: EntityId(d.line_a),
        b: EntityId(d.line_b)
    })
);

#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_collinear")]
pub struct SketchCollinearConstraintDef {
    pub line_a: u32,
    pub line_b: u32,
}
constraint_tool!(
    SketchCollinearConstraintDef,
    SketchCollinearConstraintTool,
    |state, d| state.add_constraint(|cid| Constraint::Collinear {
        id: cid,
        a: EntityId(d.line_a),
        b: EntityId(d.line_b)
    })
);

#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_perpendicular")]
pub struct SketchPerpendicularConstraintDef {
    pub line_a: u32,
    pub line_b: u32,
}
constraint_tool!(
    SketchPerpendicularConstraintDef,
    SketchPerpendicularConstraintTool,
    |state, d| state.add_constraint(|cid| Constraint::Perpendicular {
        id: cid,
        a: EntityId(d.line_a),
        b: EntityId(d.line_b)
    })
);

#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_tangent")]
pub struct SketchTangentConstraintDef {
    pub line: u32,
    pub curve: u32,
}
constraint_tool!(
    SketchTangentConstraintDef,
    SketchTangentConstraintTool,
    |state, d| state.add_constraint(|cid| Constraint::Tangent {
        id: cid,
        line: EntityId(d.line),
        curve: EntityId(d.curve)
    })
);

#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_midpoint")]
pub struct SketchMidpointConstraintDef {
    pub point: u32,
    pub line: u32,
}
constraint_tool!(
    SketchMidpointConstraintDef,
    SketchMidpointConstraintTool,
    |state, d| state.add_constraint(|cid| Constraint::Midpoint {
        id: cid,
        point: EntityId(d.point),
        line: EntityId(d.line)
    })
);

#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_symmetric")]
pub struct SketchSymmetricConstraintDef {
    pub entity_a: u32,
    pub entity_b: u32,
    pub axis: u32,
}
constraint_tool!(
    SketchSymmetricConstraintDef,
    SketchSymmetricConstraintTool,
    |state, d| state.add_constraint(|cid| Constraint::Symmetric {
        id: cid,
        a: EntityId(d.entity_a),
        b: EntityId(d.entity_b),
        axis: EntityId(d.axis)
    })
);

#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_radius")]
pub struct SketchRadiusConstraintDef {
    pub entity: u32,
    pub value_mm: f32,
}
constraint_tool!(
    SketchRadiusConstraintDef,
    SketchRadiusConstraintTool,
    |state, d| state.add_constraint(|cid| Constraint::Radius {
        id: cid,
        entity: EntityId(d.entity),
        value_mm: d.value_mm as f64,
        driven: false
    })
);

#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_diameter")]
pub struct SketchDiameterConstraintDef {
    pub entity: u32,
    pub value_mm: f32,
}
constraint_tool!(
    SketchDiameterConstraintDef,
    SketchDiameterConstraintTool,
    |state, d| state.add_constraint(|cid| Constraint::Diameter {
        id: cid,
        entity: EntityId(d.entity),
        value_mm: d.value_mm as f64,
        driven: false
    })
);
