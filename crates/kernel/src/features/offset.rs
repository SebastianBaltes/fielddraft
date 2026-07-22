//! `offset` — signierter Körper-Offset (±δ) als Makro-Expansion.
//!
//! Das Produkt-Feature der #384-offset_tree-Pipeline (docs/OFFSET-TREE-PLAN.md
//! §8). Der Nutzer wählt einen oder mehrere Zielkörper (Scope) und einen
//! signierten Betrag δ; positiv dilatiert, negativ erodiert. Das Feature
//! speichert **nur** δ + Scope und senkt zu einem Sentinel-Knoten
//! `Offset(Empty, δ)` ab — analog zum `shell`-Feature. `from_decoded` fängt
//! den Sentinel ab und legt einen echten `CsgNode::Offset(subtree, δ)` um den
//! bereits akkumulierten Subtree des Zielkörpers (siehe [`wrap_offset`]).
//!
//! Die eigentliche Arbeit erledigen die schon produktiven Baum-Pässe, die in
//! `body.rs::build_bounded_*` nach `from_decoded` laufen:
//! `push_down_offsets` (v0.16.4) schiebt das δ an die Blätter/Stopp-Knoten,
//! `fold_offset_leaves` (v0.16.6) faltet Erosionen exakt in die Blätter, und
//! die Kanal-Synthese (v0.16.7/8) korrigiert die dualen Boolean-Stopps. Das
//! Feature selbst trägt davon nichts im Dokument — alles entsteht je Regen.

use serde_json::Value;

use fielddraft_protocol::{BoolOp, FeatureId, Transform};

use crate::csg::{CsgNode, DecodedFeature};

use super::merge_scope::{expand_to_owner_body_members, target_ids_from_queries};
use super::{Context, FeatureError, FeatureKind, FeatureSchema, optional_display_name};

/// `offset` — signierter Offset (±δ) eines oder mehrerer Zielkörper.
#[derive(FeatureSchema)]
#[schema(type = "offset")]
pub struct OffsetDef {
    /// Zielkörper, deren Oberfläche verschoben wird. Kommagetrennte
    /// `qCreatedBy(<feature_id>)`-Auswahl (Scope-Pflicht wie bei
    /// Remove/Intersect); leer → das Feature wartet auf eine Auswahl.
    #[param(name = "Bodies", default = ::fielddraft_protocol::MultiQuery::new(""), entity_filter = "BODY")]
    pub targets: ::fielddraft_protocol::MultiQuery,

    /// Signierter Offset-Betrag: positiv wächst nach außen (Dilation),
    /// negativ nach innen (Erosion). 0 ist ein No-Op.
    #[param(name = "Offset", default = ::fielddraft_protocol::Length::mm(1.0))]
    pub delta: ::fielddraft_protocol::Length,
}

pub struct OffsetFeature;

impl FeatureKind for OffsetFeature {
    fn type_name(&self) -> &'static str {
        OffsetDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        OffsetDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = OffsetDef::parse(def)?;

        // Kein Scope gewählt — der Dialog wurde gerade geöffnet. Nichts emittieren
        // (kein Regen-Fehler) und den Nutzer per nicht-fataler Warnung auf die
        // Scope-Pflicht hinweisen; sobald ein Körper gewählt ist, erscheint der
        // Offset.
        let named = target_ids_from_queries(&d.targets);
        if named.is_empty() {
            ctx.warn_feature(
                id,
                "Kein Zielkörper gewählt — das Offset-Feature braucht einen \
                 expliziten Scope. Wähle einen oder mehrere Körper."
                    .to_string(),
            );
            return Ok(());
        }

        // A `qCreatedBy(f)` query names a BODY, not a single feature's subtree.
        // When that body was assembled by Add-primitive docking (or a Boolean
        // union), it spans several sibling subtrees under distinct feature ids;
        // scoping the offset to just the named id would erode only that member
        // and leave the docked siblings at original size (#399). Expand to the
        // owner body's full member set so `wrap_offset` wraps the entire body
        // tree — exactly as the Difference path (a Remove keeps the body
        // together, so its whole subtree is already wrapped). The Difference
        // path never split because a `Difference` node stays under one branch;
        // the Add path splits because docked members live in sibling `Union`
        // branches that carry their own ids.
        let target_ids = expand_to_owner_body_members(&ctx.features, &named);

        // δ = 0 ist die Identität — nichts zu tun.
        let delta = d.delta.to_mm();
        if delta == 0.0 {
            return Ok(());
        }

        // Twist/Bend haben keinen exakten Offset (keine Isometrie, Plan §2) —
        // statt still ein `f − δ` stehen zu lassen, klar abweisen. Der Scope
        // eines Zielkörpers ist die Feature-Gruppe seiner Partition.
        let parts = crate::body::partition(&ctx.features);
        let warped = parts.iter().any(|p| {
            let in_scope = p.feature_indices.iter().any(|&fi| {
                target_ids
                    .iter()
                    .any(|t| ctx.features[fi].id.root_ancestor() == t)
            });
            in_scope
                && p.feature_indices
                    .iter()
                    .any(|&fi| ctx.features[fi].node.contains_warp())
        });
        if warped {
            return Err(FeatureError::InvalidParameter {
                feature: OffsetDef::TYPE_NAME,
                name: "targets",
                message: "Offset auf einem Körper mit Twist/Bend ist nicht möglich — \
                          eine Verdrehung/Biegung hat keinen exakten Offset. Entferne \
                          den Twist/Bend oder setze den Offset davor."
                    .to_string(),
            });
        }

        let name = optional_display_name(def, || format!("Offset {id}"));

        ctx.features.push(DecodedFeature {
            id: id.clone(),
            name,
            // `Remove` ist reiner Partitions-Hinweis (in den Zielkörper docken);
            // `from_decoded` fängt den Sentinel vor jedem Boolean-Fold ab.
            op: BoolOp::Remove,
            blend_radius: 0.0,
            blend_mode: crate::csg::BlendMode::Fillet,
            target_ids,
            transform: Transform::IDENTITY,
            node: CsgNode::Offset(Box::new(CsgNode::Empty), delta),
        });
        Ok(())
    }
}
