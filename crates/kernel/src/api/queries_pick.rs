//! Szenen-Queries und Picking (Szenen-AABB, Ray-Pick mit Element-
//! Auflösung).

use wasm_bindgen::prelude::*;

use crate::Kernel;
use crate::bvh::BoundedCsg;
use crate::csg::{CsgNode};
use crate::csg;
use crate::raycast::{raycast, RaycastParams};
#[wasm_bindgen]
impl Kernel {
    /// K8.10: list every document-level variable as a JSON
    /// array of `{name, value}` pairs, sorted by name (the
    /// BTreeMap ordering).
    /// K9.1: return the overall scene AABB as a JSON object
    /// `{min: [x,y,z], max: [x,y,z], size: [w,h,d]}` in mm.
    /// Empty scenes return all-zero extents.
    #[wasm_bindgen(js_name = sceneAabb)]
    pub fn scene_aabb(&self) -> Result<String, JsError> {
        let tree = csg::CsgNode::from_decoded(&self.decoded)
            .apply_edge_fillets(&self.edge_fillets)
            .apply_edge_chamfers(&self.edge_chamfers);
        let bb = tree.aabb();
        let size = bb.size();
        let result = serde_json::json!({
            "min": bb.min,
            "max": bb.max,
            "size": size,
        });
        serde_json::to_string(&result).map_err(|e| JsError::new(&e.to_string()))
    }

    /// Debug (#350): the resolved edge-fillet zones as a JSON array. For every
    /// active `fillet_edge` it returns the world-space edge polyline and the
    /// mask radius (`blend_radius × FILLET_MASK_SCALE`) — the capsule/pill the
    /// blend is masked to, i.e. exactly what shapes the SDF. The UI draws this
    /// on demand from a per-feature debug checkbox so a fillet's reach is
    /// visible. Empty array when no fillets are resolved.
    #[wasm_bindgen(js_name = filletZones)]
    pub fn fillet_zones(&self) -> String {
        let zones: Vec<serde_json::Value> = self
            .edge_fillets
            .iter()
            .map(|f| {
                serde_json::json!({
                    "filletId": f.fillet_id.as_u32_or_root(),
                    "polyline": f.mask_polyline,
                    "maskRadius": f.mask_radius,
                    "blendRadius": f.blend_radius,
                })
            })
            .collect();
        serde_json::to_string(&zones).unwrap_or_else(|_| "[]".to_string())
    }

    /// PERSISTENT-NAMING-PLAN N4 — self-healing documents. Walks every
    /// feature call's params, finds element queries (`qFace`/`qEdge`/
    /// `qVertex`, single or in a MultiQuery list) and re-canonicalises them
    /// against the current woven B-Reps:
    ///
    /// - a **healed** reference (hash migrated, fingerprint re-bound it) is
    ///   rewritten to the fresh hash + fresh fingerprint, so the stored
    ///   fingerprint never decays across successive edits;
    /// - an exactly-resolving **hash-only legacy** reference is upgraded to
    ///   the fingerprint-carrying long form, making it healable from now on;
    /// - an unresolvable reference is left untouched (the honest error
    ///   surfaces where the feature consumes it).
    ///
    /// Intended call site: right before a project save (including
    /// auto-save) — healing is a normalisation of stored references, not a
    /// user edit, so it deliberately bypasses the undo stack and never
    /// triggers a rebuild (a healed reference resolves to the same
    /// geometry by construction). Returns a JSON array report:
    /// `[{"feature","from","to","rehashed"}, …]` — empty when nothing
    /// changed.
    #[wasm_bindgen(js_name = healElementRefs)]
    pub fn heal_element_refs(&mut self) -> String {
        // Weave every body's B-Rep and stamp the scope-wide naming (#431);
        // cached — a no-op when current.
        self.ensure_named_breps();
        let breps: Vec<&crate::brep::BRep> =
            self.bodies.iter().filter_map(|b| b.brep_cached()).collect();

        let mut report: Vec<serde_json::Value> = Vec::new();
        for call in &mut self.document.feature_calls {
            let feature = call.id.to_string();
            heal_params_value(&mut call.params, &breps, &feature, &mut report);
        }
        serde_json::Value::Array(report).to_string()
    }

    /// Resolve a canonical query string against the current document's
    /// woven B-Reps (Onshape-Import Schritt 4: the import pipeline checks
    /// that a translated plane query binds UNIQUELY before storing it).
    /// Returns a JSON array of matches, each
    /// `{"kind","idHash","query","centroid":[x,y,z]}` — `query` is the
    /// fingerprint-carrying canonical element form, `centroid` the
    /// geometric anchor the importer can disambiguate against. A parse
    /// error returns `{"error": "..."}` instead of an array.
    #[wasm_bindgen(js_name = resolveQuery)]
    pub fn resolve_query(&mut self, query: &str) -> String {
        let expr = match crate::queries::parse_query(query) {
            Ok(e) => e,
            Err(e) => {
                return serde_json::json!({ "error": e.to_string() }).to_string();
            }
        };
        self.ensure_named_breps(); // #431: scope-wide naming stamp
        let breps: Vec<crate::brep::BRep> = self
            .bodies
            .iter()
            .filter_map(|b| b.brep_cached())
            .cloned()
            .collect();
        let refs = crate::queries::resolve_query_topo(&breps, &expr);
        let out: Vec<serde_json::Value> = refs
            .iter()
            .map(|r| {
                serde_json::json!({
                    "kind": match r.kind {
                        crate::brep::naming::EntityKind::Vertex => "vertex",
                        crate::brep::naming::EntityKind::Edge => "edge",
                        crate::brep::naming::EntityKind::Face => "face",
                    },
                    "idHash": r.id_hash.to_string(),
                    "query": crate::queries::element_query(r).to_string(),
                    "centroid": r.fingerprint.centroid,
                })
            })
            .collect();
        serde_json::Value::Array(out).to_string()
    }

    /// offset_tree Phase C3 (#384): Diagnose der Kanal-Korrekturkörper je
    /// Body — Kanäle/Bögen des expandierten Weave-Baums plus das billige
    /// **Vorwarnsignal** (`interference`: Zahl der Kurvenpaare mit
    /// Kurve-Kurve-Distanz < δ_a+δ_b bzw. Selbstannäherung — dort ist die
    /// Kanal-Kantenliste strukturell verdächtig, der Octree-Weave des
    /// expandierten Körpers ist der maßgebliche Detektor). JSON-Array in
    /// Body-Reihenfolge: `[{"channels","arcs","interference"}, …]`.
    #[wasm_bindgen(js_name = offsetChannelDiagnostics)]
    pub fn offset_channel_diagnostics(&self) -> String {
        let out: Vec<serde_json::Value> = self
            .bodies
            .iter()
            .map(|b| {
                let bounded = crate::body::build_bounded_for_body_local(
                    &self.decoded,
                    &b.feature_indices,
                    &b.placement,
                );
                let (n_ch, n_arcs) = crate::offset_channel::count_channels(&bounded);
                let interference = if n_ch > 0 {
                    crate::offset_channel::channels_interference(&bounded)
                } else {
                    0
                };
                serde_json::json!({
                    "channels": n_ch,
                    "arcs": n_arcs,
                    "interference": interference,
                })
            })
            .collect();
        serde_json::Value::Array(out).to_string()
    }

    // ── picking / provenance ──────────────────────────────────────────
    //
    // K2 wire-format: the kernel takes a world-space ray (origin +
    // direction) and returns a small JSON blob that the frontend can
    // decode without touching the provenance structs directly. On a
    // miss the payload is `{"hit":false}`.

    /// Sphere-trace a ray through the current CSG tree and return a
    /// JSON description of the first hit, if any. See `raycast.rs`
    /// for the numerical scheme.
    ///
    /// offset_tree Phase C5 (#384): der Pick-Baum fährt dieselbe
    /// Offset-Normalisierung wie die Render-/Mesh-Baupfade (Pushdown →
    /// Blatt-Fold → Kanal-Synthese → Pruning, Render-Toleranz) — sonst
    /// liegt die Ray-Tiefe nahe Offset-Nähten bis ~0.4·δ neben der
    /// gerenderten Fläche und Kanal-Flächen sind nicht pickbar
    /// (Memory-Gotcha „Edge-Fillet Render-Pfad"). Gecacht in
    /// `Kernel::pick_tree` (invalidiert je Remesh), damit die
    /// Kanal-Synthese pro Regen statt pro Hover läuft.
    #[wasm_bindgen(js_name = pick)]
    pub fn pick(
        &mut self,
        ox: f32,
        oy: f32,
        oz: f32,
        dx: f32,
        dy: f32,
        dz: f32,
    ) -> String {
        if self.pick_tree.is_none() {
            let csg = CsgNode::from_decoded(&self.decoded)
                .apply_edge_fillets(&self.edge_fillets)
                .apply_edge_chamfers(&self.edge_chamfers)
                .push_down_offsets()
                .fold_offset_leaves();
            let mut bounded = crate::offset_channel::synthesize_offset_channels(
                BoundedCsg::from_csg(&csg),
                crate::offset_channel::CHANNEL_TOL_RENDER,
            );
            crate::offset_channel::prune_dead_channels(
                &mut bounded,
                crate::offset_channel::CHANNEL_TOL_RENDER,
            );
            self.pick_tree = Some(bounded);
        }
        let bounded = self.pick_tree.as_ref().unwrap();
        let params = RaycastParams::default();
        match raycast(bounded, [ox, oy, oz], [dx, dy, dz], params) {
            None => r#"{"hit":false}"#.to_string(),
            Some(hit) => {
                // Two parallel arrays, both tiny and stable — hand-formatted
                // to avoid serde_json::Value churn:
                //   `contributors`: the flat root *feature* ids (sub-index
                //     stripped). This is what selection, query-picking and
                //     edge fillets consume — they map an id to a feature in
                //     the tree, so they need the feature, not the face.
                //     Synthetic contact prisms carry an id that no document
                //     feature owns; they report their OWNER feature instead
                //     (#384 §9, see below).
                //   `branches`: the packed BranchIds (feature << SUB_BITS |
                //     sub) in the same order. The analytic B-Rep edge overlay
                //     matches a hovered edge by its colliding-face signature,
                //     which is exactly these packed per-face ids.
                //   `synthetic`: the second stage — one entry per hit face
                //     that the offset/shell expansion SYNTHESISED, with its
                //     parent references. Empty for ordinary geometry.
                let mut contributors = String::from("[");
                let mut branches = String::from("[");
                // #384 Plan §9: die zweistufige Auflösung. `synthetic` listet
                // je getroffener SYNTHETISCHER Fläche (Offset-Kanal,
                // Vertex-Kugel, Kontakt-Prisma) ihre Klasse plus die
                // Eltern-Referenzen; für alles andere bleibt es leer.
                let mut synthetic: Vec<String> = Vec::new();
                for (i, id) in hit.contributors.as_slice().iter().enumerate() {
                    if i > 0 {
                        contributors.push(',');
                        branches.push(',');
                    }
                    let syn = crate::offset_channel::resolve_synthetic(bounded, id.as_u64());
                    // Ein Kontakt-Prisma trägt eine SYNTHETISCHE Feature-Id
                    // (Bit-19-Marker), die im Dokument nicht existiert — roh
                    // gemeldet selektiert die UI damit nichts. Primär gilt das
                    // Owner-Feature; nur wenn selbst das unbekannt ist
                    // (Sentinel), bleibt die rohe Id stehen (ehrlich statt
                    // erfunden). Die Arrays bleiben dabei parallel.
                    let primary = syn
                        .as_ref()
                        .filter(|s| s.kind == crate::offset_channel::SyntheticKind::Prism)
                        .and_then(|s| s.owner)
                        .unwrap_or_else(|| id.feature_id());
                    contributors.push_str(&primary.to_string());
                    branches.push_str(&id.as_u64().to_string());
                    if let Some(s) = syn {
                        let kind = match s.kind {
                            crate::offset_channel::SyntheticKind::Channel => "channel",
                            crate::offset_channel::SyntheticKind::VertexSphere => "vertex",
                            crate::offset_channel::SyntheticKind::Prism => "prism",
                        };
                        let parents = s
                            .parents
                            .iter()
                            .map(|p| p.to_string())
                            .collect::<Vec<_>>()
                            .join(",");
                        let owner = match s.owner {
                            Some(o) => o.to_string(),
                            None => "null".to_string(),
                        };
                        synthetic.push(format!(
                            r#"{{"branch":{},"kind":"{kind}","owner":{owner},"parents":[{parents}]}}"#,
                            s.branch
                        ));
                    }
                }
                contributors.push(']');
                branches.push(']');
                let synthetic = format!("[{}]", synthetic.join(","));
                format!(
                    r#"{{"hit":true,"point":[{},{},{}],"normal":[{},{},{}],"t":{},"element":"{}","contributors":{},"branches":{},"synthetic":{}}}"#,
                    hit.point[0],
                    hit.point[1],
                    hit.point[2],
                    hit.normal[0],
                    hit.normal[1],
                    hit.normal[2],
                    hit.t,
                    hit.element.label(),
                    contributors,
                    branches,
                    synthetic,
                )
            }
        }
    }
}

/// Recursive param walk for [`Kernel::heal_element_refs`]: every string leaf
/// is a healing candidate; objects and arrays recurse.
fn heal_params_value(
    v: &mut serde_json::Value,
    breps: &[&crate::brep::BRep],
    feature: &str,
    report: &mut Vec<serde_json::Value>,
) {
    match v {
        serde_json::Value::Object(map) => {
            for val in map.values_mut() {
                heal_params_value(val, breps, feature, report);
            }
        }
        serde_json::Value::Array(arr) => {
            for val in arr.iter_mut() {
                heal_params_value(val, breps, feature, report);
            }
        }
        serde_json::Value::String(s) => {
            if let Some(next) = heal_query_string(s, breps, feature, report) {
                *s = next;
            }
        }
        _ => {}
    }
}

/// Re-canonicalise the element queries inside one (Multi)Query string.
/// Returns the rewritten string when anything changed; `None` leaves the
/// param untouched (including every non-query string — a part that doesn't
/// parse as an element query passes through verbatim).
fn heal_query_string(
    s: &str,
    breps: &[&crate::brep::BRep],
    feature: &str,
    report: &mut Vec<serde_json::Value>,
) -> Option<String> {
    use crate::brep::naming::{EntityHandle, EntityKind, TopoRef};
    use crate::queries::{element_query, locate_element, parse_query, QueryExpr};

    // Cheap gate: only strings that could contain a query at all.
    if !s.trim_start().starts_with('q') {
        return None;
    }
    let mq = fielddraft_protocol::MultiQuery::new(s);
    let parts: Vec<String> = mq.queries().iter().map(|p| p.to_string()).collect();
    if parts.is_empty() {
        return None;
    }

    let mut changed = false;
    let mut out: Vec<String> = Vec::with_capacity(parts.len());
    for part in parts {
        let (kind, eref) = match parse_query(&part) {
            Ok(QueryExpr::Face(r)) => (EntityKind::Face, r),
            Ok(QueryExpr::Edge(r)) => (EntityKind::Edge, r),
            Ok(QueryExpr::Vertex(r)) => (EntityKind::Vertex, r),
            _ => {
                out.push(part);
                continue;
            }
        };
        let Some((bi, handle)) = locate_element(breps.iter().copied(), kind, &eref) else {
            // Unresolvable — keep the stored reference; the consuming
            // feature reports the honest error.
            out.push(part);
            continue;
        };
        let brep = breps[bi];
        let topo = match handle {
            EntityHandle::Face(id) => TopoRef::from_face(brep, id),
            EntityHandle::Edge(id) => TopoRef::from_edge(brep, id),
            EntityHandle::Vertex(id) => TopoRef::from_vertex(brep, id),
        };
        let canonical = element_query(&topo).to_string();
        if canonical != part {
            report.push(serde_json::json!({
                "feature": feature,
                "from": part,
                "to": canonical,
                "rehashed": topo.id_hash != eref.id_hash,
            }));
            changed = true;
        }
        out.push(canonical);
    }
    changed.then(|| out.join(", "))
}

#[cfg(test)]
mod heal_tests {
    use super::*;
    use crate::brep::naming::TopoRef;
    use crate::brep::octree::Octree;
    use crate::brep::weaving::weave;
    use crate::bvh::BoundedCsg;
    use crate::csg::CsgNode;
    use crate::queries::element_query;
    use fielddraft_protocol::{FeatureId, Primitive};

    fn cube_brep(half: f32) -> crate::brep::BRep {
        let body = BoundedCsg::from_csg(&CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Box { center: [0.0; 3], half_extents: [half; 3] },
        });
        let octree = Octree::build(&body, 0.25);
        weave(&body, &octree)
    }

    #[test]
    fn heal_upgrades_hash_only_and_normalises() {
        let brep = cube_brep(5.0);
        let want = TopoRef::from_face(&brep, crate::brep::FaceId(2));
        let canonical = element_query(&want).to_string();

        let mut report = Vec::new();
        // Legacy hash-only ref upgrades to the fingerprint long form.
        let healed = heal_query_string(
            &format!("qFace({})", want.id_hash),
            &[&brep],
            "f1",
            &mut report,
        );
        assert_eq!(healed.as_deref(), Some(canonical.as_str()));
        assert_eq!(report.len(), 1);
        assert_eq!(report[0]["rehashed"], false);

        // An already-canonical ref is a no-op (no churn on every save).
        let mut report2 = Vec::new();
        assert_eq!(heal_query_string(&canonical, &[&brep], "f1", &mut report2), None);
        assert!(report2.is_empty());

        // Non-query strings pass through untouched.
        assert_eq!(heal_query_string("hello", &[&brep], "f1", &mut Vec::new()), None);
        assert_eq!(
            heal_query_string("qCreatedBy(box_1)", &[&brep], "f1", &mut Vec::new()),
            None
        );
    }

    #[test]
    fn heal_rewrites_stale_hash_via_fingerprint() {
        let brep = cube_brep(5.0);
        let want = TopoRef::from_face(&brep, crate::brep::FaceId(2));
        // A stale ref: dead hash, but the fingerprint still pins the face.
        let stale = format!(
            "qFace(1234567890 @ {} {} {} {})",
            want.fingerprint.centroid[0],
            want.fingerprint.centroid[1],
            want.fingerprint.centroid[2],
            want.fingerprint.size
        );
        let mut report = Vec::new();
        let healed = heal_query_string(&stale, &[&brep], "f1", &mut report);
        assert_eq!(healed.as_deref(), Some(element_query(&want).to_string().as_str()));
        assert_eq!(report[0]["rehashed"], true);

        // Unresolvable refs stay verbatim (mixed MultiQuery keeps both parts).
        let mixed = format!("qFace(999 @ 1e6 1e6 1e6 0.001), {stale}");
        let mut report3 = Vec::new();
        let healed3 = heal_query_string(&mixed, &[&brep], "f1", &mut report3).unwrap();
        assert!(healed3.starts_with("qFace(999 @ 1e6 1e6 1e6 0.001), "));
        assert_eq!(report3.len(), 1, "only the resolvable part is rewritten");
    }
}
