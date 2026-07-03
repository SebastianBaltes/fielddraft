//! `boolean` — explizites Union/Difference/Intersection über Ziel-Bodies.

use serde_json::Value;

use fielddraft_protocol::{BoolOp, FeatureId, MultiQuery};

use super::{Context, FeatureError, FeatureKind, FeatureSchema};
// ─── Boolean Feature ────────────────────────────────────────────
//
// Dedicated Boolean feature with tabbed dialog (Union / Subtract /
// Intersect). The schema is hand-crafted to emit the "tabs" widget
// structure; the derive is used only for parse().

#[derive(FeatureSchema)]
#[schema(type = "boolean")]
pub struct BooleanDef {
    #[param(name = "Operation", default = "Union".to_string())]
    pub operation: String,
    #[param(name = "Tools", entity_filter = "BODY")]
    pub tools: MultiQuery,
    #[param(name = "Targets", default = MultiQuery::new(""), entity_filter = "BODY")]
    pub targets: MultiQuery,
    #[param(name = "Keep tools", default = false)]
    pub keep_tools: bool,
}

pub struct BooleanFeature;

impl FeatureKind for BooleanFeature {
    fn type_name(&self) -> &'static str {
        BooleanDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        serde_json::json!({
            "type": "boolean",
            "params": [
                {
                    "key": "operation",
                    "name": "Operation",
                    "type": "tabs",
                    "required": false,
                    "tabs": [
                        {
                            "label": "Union",
                            "children": [
                                { "key": "tools", "name": "Tools", "type": "multi_query", "required": true }
                            ]
                        },
                        {
                            "label": "Subtract",
                            "children": [
                                { "key": "tools", "name": "Tools", "type": "multi_query", "required": true },
                                { "key": "targets", "name": "Targets", "type": "multi_query", "required": true }
                            ]
                        },
                        {
                            "label": "Intersect",
                            "children": [
                                { "key": "tools", "name": "Tools", "type": "multi_query", "required": true }
                            ]
                        }
                    ]
                },
                { "key": "keep_tools", "name": "Keep tools", "type": "bool", "required": false }
            ]
        })
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = BooleanDef::parse(def)?;

        let op = match d.operation.as_str() {
            "Subtract" => BoolOp::Remove,
            "Intersect" => BoolOp::Intersect,
            _ => BoolOp::Add,
        };

        let resolve_ids = |mq: &MultiQuery| -> Vec<FeatureId> {
            mq.queries()
                .iter()
                .filter_map(|q| {
                    let inner = q.strip_prefix("qCreatedBy(")?.strip_suffix(')')?;
                    inner.parse::<FeatureId>().ok()
                })
                .collect()
        };

        let tool_ids = resolve_ids(&d.tools);
        if tool_ids.is_empty() {
            return Ok(());
        }

        // Snapshot semantics (ticket 9wxs): a Boolean may only reference
        // features that PRECEDE it in the tree. A tool/target that isn't in
        // the decoded prefix is a forward reference (or a deleted feature) —
        // fail with a clear error instead of silently doing nothing (the
        // "Subtract Cone aus Sphere hat keinen Effekt" regression). Match on
        // the root ancestor so child-id features (e.g. sweep) still resolve.
        let scope_target_ids = resolve_ids(&d.targets);
        let present = |fid: &FeatureId| {
            ctx.features
                .iter()
                .any(|f| f.id.root_ancestor() == fid.root_ancestor())
        };
        if let Some(missing) = tool_ids
            .iter()
            .chain(scope_target_ids.iter())
            .find(|fid| !present(fid))
        {
            return Err(FeatureError::InvalidParameter {
                feature: BooleanDef::TYPE_NAME,
                name: "tools",
                message: format!(
                    "Referenz auf Feature {missing}, das vor diesem Boolean nicht existiert \
                     (Vorwärts-Referenz oder gelöscht). Ein Boolean darf nur frühere Features \
                     verwenden — verschiebe das Boolean hinter seine Operanden."
                ),
            });
        }

        // The first tool in spine order keeps op=Union so it seeds a
        // BodyPartition. Subsequent tools attach to that body — a union
        // routes them in via an explicit `target_ids` link to the seed
        // (pure spatial overlap never merges unions); intersect/subtract
        // dock via AABB overlap.
        let first_tool_pos = ctx
            .features
            .iter()
            .position(|f| tool_ids.contains(&f.id));
        let first_tool_id = first_tool_pos.and_then(|p| ctx.features.get(p).map(|f| f.id.clone()));

        for (idx, feature) in ctx.features.iter_mut().enumerate() {
            if !tool_ids.contains(&feature.id) {
                continue;
            }
            match op {
                op if op.is_union() => {
                    // Seed the body with the first tool; route every other
                    // tool into that same body via an explicit link so the
                    // partition pass merges them into one Part instead of
                    // spawning a body per tool. Non-seed tools become `Add`
                    // (the op that follows `target_ids`); the seed stays
                    // `New` unless it already carries links — then it must
                    // be `Add` so the partition pass still follows them.
                    // Two rules keep union transitive across operations:
                    //
                    // * Non-seed tools *accumulate* their links (push, don't
                    //   overwrite): a part shared by two unions (a sphere
                    //   unioned to a left and a right solid) carries BOTH seed
                    //   links so all three fuse into one Part.
                    // * The seed *keeps* whatever links it already carries —
                    //   we must NOT clear it. On a fresh decode the seed
                    //   starts empty, so a non-shared seed still seeds its own
                    //   body; but a tool shared with an EARLIER union arrives
                    //   already linked to that union's seed. The Boolean picks
                    //   its seed as the first tool in *spine order*, so when
                    //   the shared bridge is spine-earlier than this op's
                    //   other tool, the bridge becomes the seed — clearing it
                    //   would drop the earlier link and split the part (the
                    //   "2 Parts that should be 1" bug).
                    if Some(idx) != first_tool_pos {
                        feature.op = BoolOp::Add;
                        if let Some(seed) = &first_tool_id {
                            if !feature.target_ids.contains(seed) {
                                feature.target_ids.push(seed.clone());
                            }
                        }
                    } else if feature.target_ids.is_empty() {
                        feature.op = BoolOp::New;
                    } else {
                        feature.op = BoolOp::Add;
                    }
                }
                BoolOp::Intersect => {
                    if tool_ids.len() > 1 && Some(idx) == first_tool_pos {
                        feature.op = if feature.target_ids.is_empty() {
                            BoolOp::New
                        } else {
                            BoolOp::Add
                        };
                    } else {
                        feature.op = BoolOp::Intersect;
                        // Route the tool to an EXPLICIT target — never AABB
                        // contact. Explicit scope wins; otherwise a multi-tool
                        // intersect links its non-seed tools to the seed so
                        // they intersect it (the seed body). A lone tool with
                        // no scope stays untargeted → standalone preview + the
                        // warning below.
                        if !scope_target_ids.is_empty() {
                            for t in &scope_target_ids {
                                if !feature.target_ids.contains(t) {
                                    feature.target_ids.push(t.clone());
                                }
                            }
                        } else if tool_ids.len() > 1 {
                            if let Some(seed) = &first_tool_id {
                                if !feature.target_ids.contains(seed) {
                                    feature.target_ids.push(seed.clone());
                                }
                            }
                        }
                    }
                }
                _ => {
                    feature.op = BoolOp::Remove;
                    // As above: a Subtract carves ONLY the named targets, never
                    // an unrelated body it happens to overlap. With no scope it
                    // stays untargeted → standalone preview + the warning below.
                    for t in &scope_target_ids {
                        if !feature.target_ids.contains(t) {
                            feature.target_ids.push(t.clone());
                        }
                    }
                }
            }
        }

        // Scope is mandatory for Subtract/Intersect (Onshape semantics): a
        // Subtract with no targets, or a single-tool Intersect with no target,
        // has nothing to act on — warn so the empty selector is visible (the
        // tools render as standalone red previews via the partition pass).
        let needs_scope = match op {
            BoolOp::Remove => scope_target_ids.is_empty(),
            BoolOp::Intersect => scope_target_ids.is_empty() && tool_ids.len() <= 1,
            _ => false,
        };
        if needs_scope {
            ctx.warn_feature(
                id,
                "Kein Zielkörper gewählt — Subtract/Intersect braucht einen Scope. \
                 Wähle, von welchem Körper abgezogen bzw. womit geschnitten wird."
                    .to_string(),
            );
        }

        Ok(())
    }
}
