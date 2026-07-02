//! Feature registry and the Feature-as-Function execution model.
//!
//! This is step 2 of the K4.5 refactor (README §4.6, §10 K4.5 bullet).
//! The goal of this step is narrow: stand up a [`FeatureKind`] trait
//! and a [`FeatureRegistry`] that can walk a `Vec<FeatureCall>`, look
//! up each entry by its type name, validate the parameter JSON against
//! the feature's expectations, and call the feature's `execute` on a
//! mutable [`Context`]. The first three built-in features — box,
//! sphere, cylinder — are the K1 primitives re-packaged into the new
//! model so we can see the regeneration pipeline end-to-end.
//!
//! ## What this module deliberately does *not* do yet
//!
//! * **No proc-macro.** The derive-based schema generator is step 3.
//!   Built-in features register manually.
//! * **No JSON schema export.** Also step 3 once the derive lands.
//! * **No Regeneration Worker integration.** The worker's global
//!   freeze + `globalThis.fd = {…}` injection is step 5.
//! * **No queries.** `qCreatedBy` / `qFaceOf` / `qEdgeBetween` are
//!   step 7.
//! * **The Kernel struct is unchanged.** `crates/kernel/src/lib.rs`
//!   still drives mutations through `add_primitive` and friends.
//!   Wiring the registry into the main kernel path is a later
//!   session — this module is exercised exclusively by its own
//!   tests for now.
//! * **No hierarchical ids are issued yet.** Step 1 (K4.5.1) gave us
//!   the typed [`FeatureId`] shape; step 2 still only uses flat
//!   numeric ids via `FeatureId::from_u32`.
//!
//! ## Naming note
//!
//! The trait is called `FeatureKind`, not `Feature`. The fat
//! `protocol::Feature` spine struct it once collided with is gone
//! (DL-61 C2b): the wire form is `Vec<FeatureCall>` and the decoded
//! runtime form is [`crate::csg::DecodedFeature`]. Each `FeatureKind`
//! lowers a `FeatureCall` into one or more `DecodedFeature`s.

use std::collections::HashMap;
use std::fmt;

use serde_json::Value;

use fielddraft_protocol::{BoolOp, Document, FeatureId, MultiQuery, Primitive, Transform};

use crate::csg::{CsgNode, DecodedFeature, PrimitiveModifiers};
use crate::path_provider::PathProvider;

/// Derive macro that generates `TYPE_NAME`, `type_name()`,
/// `schema_json()` and `parse()` helpers on a feature definition
/// struct. See `fielddraft_features_macros` for the attribute
/// vocabulary — K4.5 step 3 supports `#[feature(type = "...")]`
/// at the struct level and auto-detects field types, every field
/// is implicitly required.
pub use fielddraft_features_macros::FeatureSchema;

/// Re-export of the canonical on-disk feature-call shape. K4.5.4b
/// promoted `FeatureCall` from a kernel-internal helper to a
/// first-class wire type inside [`fielddraft_protocol`] so the
/// Document can eventually store `Vec<FeatureCall>` as its
/// authoritative serialisation form (K4.5.4c+). Existing
/// `crate::features::FeatureCall` imports keep working through
/// this re-export — the move is purely structural.
pub use fielddraft_protocol::FeatureCall;

/// Errors that can happen while looking up a feature in the registry
/// or while a feature's `execute` validates its parameter JSON.
#[derive(Debug, Clone, PartialEq)]
pub enum FeatureError {
    /// The feature call's `type` field didn't match any registered
    /// feature kind.
    UnknownFeatureType(String),
    /// A required parameter was missing from the definition JSON.
    MissingParameter {
        feature: &'static str,
        name: &'static str,
    },
    /// A parameter was present but the wrong shape (e.g. a number
    /// where a `[f32; 3]` was expected, or a negative length).
    InvalidParameter {
        feature: &'static str,
        name: &'static str,
        message: String,
    },
    /// The `op` field referenced an unknown boolean operation.
    InvalidOp { feature: &'static str, value: String },
}

impl fmt::Display for FeatureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownFeatureType(name) => {
                write!(f, "unknown feature type `{name}`")
            }
            Self::MissingParameter { feature, name } => {
                write!(f, "feature `{feature}`: missing parameter `{name}`")
            }
            Self::InvalidParameter {
                feature,
                name,
                message,
            } => write!(
                f,
                "feature `{feature}`: parameter `{name}` invalid: {message}",
            ),
            Self::InvalidOp { feature, value } => {
                write!(f, "feature `{feature}`: unknown op `{value}`")
            }
        }
    }
}

impl std::error::Error for FeatureError {}

/// Mutable world state passed to every feature's `execute` call.
///
/// K4.5 step 2 keeps this intentionally thin: the only thing a
/// feature can touch is the [`Document`] whose feature spine backs
/// the existing K1/K2/K4 mesher pipeline. Later slices will grow the
/// context to own the body cache, provenance tables and query
/// results so features can read each other's outputs — but those
/// pieces are K4.5 step 5+ scope.
///
/// The lifetime `'a` ties the context to the caller's borrow of the
/// document, so a feature can't smuggle a reference out of its
/// execution.
pub struct Context<'a> {
    pub document: &'a mut Document,
    /// DL-61 C2b: the decoded feature spine accumulator. Each `execute`
    /// pushes its [`DecodedFeature`] entries here (and the transform/
    /// boolean/pattern features mutate earlier entries in place); the
    /// kernel consumes the finished list directly once regeneration
    /// completes. The protocol [`Document`] no longer carries a
    /// `features` field — `feature_calls` is the sole persisted form.
    pub features: Vec<DecodedFeature>,
    /// K6b.1: solved sketch state per sketch feature id.
    pub sketches: HashMap<FeatureId, crate::sketch_features::SketchData>,
    /// K10.4: edge fillets collected during regen, applied as a
    /// post-processing pass on the CsgNode tree.
    pub edge_fillets: Vec<crate::csg::EdgeFillet>,
    /// C0: shared lazy B-Rep cache over `features`. A feature can
    /// resolve element-level queries (`qFace(h)`, `qCreatedBy(id)`, …)
    /// against the geometry produced by every feature *so far* —
    /// mid-loop. The content key auto-invalidates whenever a feature
    /// pushes, so no manual clearing is needed between `execute` calls.
    pub brep_cache: crate::body::BrepCache,
    /// C2: resolved CSYS of every `mate_connector` placed this regen.
    /// Captured by the kernel after regeneration so the viewport can
    /// render datum triads without re-resolving the face query.
    pub mate_connectors: Vec<ResolvedMateConnector>,
    /// Per-feature merge-scope warnings (non-fatal). A feature-implicit
    /// boolean (Add/Remove/Intersect) whose merge scope resolves to no
    /// target body records a warning here — geometry still regenerates (the
    /// body shows as a red translucent preview), but the UI flags the
    /// feature so the user picks a scope or "Merge with all".
    pub feature_warnings: Vec<(FeatureId, String)>,
}

impl<'a> Context<'a> {
    pub fn new(document: &'a mut Document) -> Self {
        Self {
            document,
            features: Vec::new(),
            sketches: HashMap::new(),
            edge_fillets: Vec::new(),
            brep_cache: crate::body::BrepCache::new(),
            mate_connectors: Vec::new(),
            feature_warnings: Vec::new(),
        }
    }

    /// C0: resolve a query against the geometry woven from the features
    /// accumulated so far. Lazily (re)weaves via [`Context::brep_cache`]
    /// then delegates to [`crate::queries::resolve_query_topo`].
    pub fn resolve_topo(
        &mut self,
        expr: &crate::queries::QueryExpr,
    ) -> Vec<crate::brep::naming::TopoRef> {
        let woven = self.brep_cache.ensure(&self.features);
        let breps: Vec<crate::brep::BRep> = woven.iter().map(|(b, _)| b.clone()).collect();
        crate::queries::resolve_query_topo(&breps, expr)
    }

    /// Seed feature id of every body partitioned from the features decoded so
    /// far — i.e. the bodies that exist *before* the feature currently being
    /// executed. This is the "all bodies" set a "Merge with all" boolean
    /// folds into (see [`resolve_merge_scope`]).
    pub fn current_body_seed_ids(&self) -> Vec<FeatureId> {
        crate::body::partition(&self.features)
            .iter()
            .map(|p| p.seed_id(&self.features))
            .collect()
    }

    /// Merge-scope warning for a feature-implicit boolean, accounting for
    /// whether any body exists *before* this feature (a scope-less first `Add`
    /// is an implicit "New", not a warning). See [`merge_scope_warning`].
    pub fn merge_scope_warning(&self, op: BoolOp, resolved_targets: &[FeatureId]) -> Option<String> {
        let has_prior_bodies = !self.current_body_seed_ids().is_empty();
        merge_scope_warning(op, resolved_targets, has_prior_bodies)
    }

    /// Record a non-fatal merge-scope warning for `id` (see
    /// [`Context::feature_warnings`]).
    pub fn warn_feature(&mut self, id: &FeatureId, message: String) {
        self.feature_warnings.push((id.clone(), message));
    }

    /// C0/C1: resolve a face-yielding query to the first matching face,
    /// returning its [`SketchFrame`](crate::sketch_features::SketchFrame)
    /// in **world** space (the woven B-Rep is body-local; the paired
    /// placement is applied for us).
    ///
    /// Walks the woven solids in body order and, for each, scans for the
    /// faces the query selects (by `id_hash` for `qFace`, by branch
    /// feature id for `qCreatedBy`/`qFaceOf`), returning the first hit.
    pub fn resolve_face_frame(
        &mut self,
        expr: &crate::queries::QueryExpr,
    ) -> Option<crate::sketch_features::SketchFrame> {
        use crate::brep::naming::TopoRef;
        use crate::brep::FaceId;
        use crate::queries::QueryExpr;

        let woven = self.brep_cache.ensure(&self.features);
        for (brep, placement) in woven {
            for i in 0..brep.faces.len() {
                let fid = FaceId(i as u32);
                let hit = match expr {
                    QueryExpr::Face(hash) => {
                        TopoRef::from_face(brep, fid).id_hash == *hash
                    }
                    QueryExpr::CreatedBy(f) | QueryExpr::FaceOf(f) => f
                        .as_u32_or_root()
                        .is_some_and(|t| brep.faces[i].branch_id.feature_id() == t),
                    _ => false,
                };
                if hit {
                    return Some(crate::sketch_features::face_plane_frame(
                        brep, fid, placement,
                    ));
                }
            }
        }
        None
    }

    /// Resolve a B-Rep face query to the world-space [`OpenSurface`] that removes
    /// it from a shell, paired with the feature id that owns the face (its
    /// branch). Works for any analytic face — Plane, Cylinder, Sphere, Cone,
    /// Torus — by composing the face's local `transform` with the body
    /// `placement` into a rigid world placement and pairing it with the face's
    /// own [`HalfSpace`]; the shell gate then evaluates that surface's SDF
    /// (`sdf_halfspace`) in world space, opening flush with the (possibly
    /// curved) face. Returns `None` for a non-matching query or a tessellated
    /// (TPMS / non-analytic) face, which has no closed-form gate.
    pub fn resolve_open_face(
        &mut self,
        expr: &crate::queries::QueryExpr,
    ) -> Option<(crate::csg::OpenSurface, FeatureId)> {
        use crate::brep::naming::TopoRef;
        use crate::brep::{FaceId, SurfaceGeometry};
        use crate::queries::QueryExpr;

        let woven = self.brep_cache.ensure(&self.features);
        for (brep, placement) in woven {
            for i in 0..brep.faces.len() {
                let fid = FaceId(i as u32);
                let hit = match expr {
                    QueryExpr::Face(hash) => TopoRef::from_face(brep, fid).id_hash == *hash,
                    QueryExpr::CreatedBy(f) | QueryExpr::FaceOf(f) => f
                        .as_u32_or_root()
                        .is_some_and(|t| brep.faces[i].branch_id.feature_id() == t),
                    _ => false,
                };
                if !hit {
                    continue;
                }
                let SurfaceGeometry::Analytic { surface, transform } = &brep.face(fid).surface
                else {
                    // Tessellated (TPMS / non-analytic) face — no closed-form gate.
                    return None;
                };
                // World placement = body placement ∘ face transform (rigid).
                let world = compose_placement(placement, transform);
                let owner = FeatureId::from_u32(brep.faces[i].branch_id.feature_id());
                return Some((
                    crate::csg::OpenSurface {
                        branch: brep.faces[i].branch_id.as_u32(),
                        surface: surface.clone(),
                        placement: world,
                    },
                    owner,
                ));
            }
        }
        None
    }

    /// Resolve a coplanar set of `qFace(...)` queries into a single 2D extrude
    /// profile plus the shared world [`SketchFrame`] it sits on (face extrude,
    /// P1). Every selected face must be planar and coplanar with the first; the
    /// boundary loops are projected into the first face's frame and unioned, so
    /// a multi-face selection extrudes as one merged profile (see
    /// [`crate::brep::face_profile`]).
    ///
    /// Errors are returned as a message the caller wraps in a [`FeatureError`]:
    /// a non-`qFace` query, an unresolvable handle, a non-planar (curved) face,
    /// or a face not coplanar with the first.
    pub fn resolve_extrude_faces(
        &mut self,
        queries: &[String],
    ) -> Result<(::fielddraft_protocol::Sketch2d, crate::sketch_features::SketchFrame), String> {
        use crate::brep::naming::TopoRef;
        use crate::brep::FaceId;
        use crate::queries::QueryExpr;

        let woven = self.brep_cache.ensure(&self.features);

        // Locate each requested face → (brep index, FaceId, world frame).
        let mut hits: Vec<(usize, FaceId, crate::sketch_features::SketchFrame)> = Vec::new();
        for q in queries {
            let expr = crate::queries::parse_query(q)
                .map_err(|e| format!("invalid face query `{q}`: {e:?}"))?;
            let QueryExpr::Face(hash) = expr else {
                return Err(format!(
                    "extrude faces must be `qFace(<id_hash>)` selections; got `{q}`"
                ));
            };
            let mut found = false;
            'scan: for (bi, (brep, placement)) in woven.iter().enumerate() {
                for i in 0..brep.faces.len() {
                    let fid = FaceId(i as u32);
                    if TopoRef::from_face(brep, fid).id_hash == hash {
                        hits.push((
                            bi,
                            fid,
                            crate::sketch_features::face_plane_frame(brep, fid, placement),
                        ));
                        found = true;
                        break 'scan;
                    }
                }
            }
            if !found {
                return Err(format!("face {hash} not found in the current model"));
            }
        }
        if hits.is_empty() {
            return Err("no faces selected to extrude".to_string());
        }

        // First face's frame is the shared (u, v) plane for the whole group.
        let frame = hits[0].2;
        for (_, _, f) in &hits[1..] {
            if !coplanar(&frame, f) {
                return Err("all selected faces must be coplanar (curved / multi-plane extrude is not supported yet)".to_string());
            }
        }

        // Project each face's loops into the shared frame and union the regions.
        let mut region: Option<::fielddraft_protocol::Sketch2d> = None;
        for (bi, fid, _) in &hits {
            let (brep, placement) = &woven[*bi];
            let r = crate::brep::face_profile::face_region_2d(brep, *fid, &frame, placement)
                .ok_or_else(|| {
                    "face has no planar extrudable profile (curved faces deferred)".to_string()
                })?;
            region = Some(match region {
                None => r,
                Some(acc) => ::fielddraft_protocol::Sketch2d::Union(Box::new(acc), Box::new(r)),
            });
        }
        Ok((region.unwrap(), frame))
    }

    /// Feature ids of the body that owns the first resolvable `qFace(...)` in
    /// `queries` — the explicit target a face-driven feature (extrude grown from
    /// a body face) docks into. A face-extrude is flush with its parent by
    /// construction, so an `Add` must grow *that* body; without the target it
    /// falls back to AABB overlap, which both misses the parent (the shared
    /// plane is mesh-derived and lands a fraction of a micron off the parent's
    /// exact bound) and can spuriously fuse an unrelated body whose loose AABB
    /// happens to overlap the prism (ticket px2o). Empty if no face resolves, so
    /// the caller keeps the legacy AABB docking.
    pub fn face_owner_target_ids(&mut self, queries: &[String]) -> Vec<FeatureId> {
        use crate::brep::naming::TopoRef;
        use crate::brep::FaceId;
        use crate::queries::QueryExpr;

        // Locate the woven body (partition order) that carries any picked face.
        let mut owner_bi: Option<usize> = None;
        {
            let woven = self.brep_cache.ensure(&self.features);
            'scan: for q in queries {
                let Ok(QueryExpr::Face(hash)) = crate::queries::parse_query(q) else {
                    continue;
                };
                for (bi, (brep, _)) in woven.iter().enumerate() {
                    for i in 0..brep.faces.len() {
                        if TopoRef::from_face(brep, FaceId(i as u32)).id_hash == hash {
                            owner_bi = Some(bi);
                            break 'scan;
                        }
                    }
                }
            }
        }
        // `ensure` weaves via `weave_all` → `partition`, so the woven body index
        // is the partition index: map it straight back to that group's features.
        let Some(bi) = owner_bi else {
            return Vec::new();
        };
        crate::body::partition(&self.features)
            .get(bi)
            .map(|p| {
                p.feature_indices
                    .iter()
                    .map(|&fi| self.features[fi].id.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Resolve a set of `qFace(...)` queries into per-face thickened-wall solids
    /// (face thicken — the curved counterpart to [`Context::resolve_extrude_faces`]).
    /// Each face's analytic surface is offset by `outer_off` along +normal and
    /// `inner_off` along −normal into a [`ThickenSolid`](crate::brep::face_thicken::ThickenSolid)
    /// pair (see [`crate::brep::face_thicken`]). The caller differences and
    /// unions them into one feature.
    ///
    /// Errors (returned as a message the caller wraps in a [`FeatureError`]): a
    /// non-`qFace` query, an unresolvable handle, or a face the surface layer
    /// refuses to thicken (planar / cone / partial cylinder / tessellated).
    pub fn resolve_thicken_faces(
        &mut self,
        id: &FeatureId,
        queries: &[String],
        outer_off: f32,
        inner_off: f32,
    ) -> Result<Vec<(CsgNode, Transform)>, String> {
        use crate::brep::naming::TopoRef;
        use crate::brep::{FaceId, SurfaceGeometry};
        use crate::queries::QueryExpr;

        // Pass 1 (woven borrowed): resolve analytic walls in place; for a
        // non-analytic (Tessellated) face record a deferred band request — the
        // general band needs the source feature node from `self.features`, which
        // cannot be borrowed while the woven cache is alive.
        struct BandReq {
            qi: usize,
            feature_id: u32,
            placement: Transform,
            /// The picked face's edge-loops (body-local) for the M1 lateral trim.
            loops: crate::brep::face_lateral_trim::FaceLoops,
        }
        let mut walls: Vec<(usize, CsgNode, Transform)> = Vec::new();
        let mut bands: Vec<BandReq> = Vec::new();
        {
            let woven = self.brep_cache.ensure(&self.features);
            for (qi, q) in queries.iter().enumerate() {
                let expr = crate::queries::parse_query(q)
                    .map_err(|e| format!("invalid face query `{q}`: {e:?}"))?;
                let QueryExpr::Face(hash) = expr else {
                    return Err(format!(
                        "thicken faces must be `qFace(<id_hash>)` selections; got `{q}`"
                    ));
                };
                let mut found = false;
                'scan: for (brep, placement) in woven.iter() {
                    for i in 0..brep.faces.len() {
                        let fid = FaceId(i as u32);
                        if TopoRef::from_face(brep, fid).id_hash == hash {
                            let face = brep.face(fid);
                            match &face.surface {
                                // A revolved-profile carrier is analytic for the
                                // weaver but has no parameter-shift offset, so it
                                // thickens via the general field-offset band —
                                // the same route these faces took when they were
                                // still tessellated.
                                SurfaceGeometry::Analytic {
                                    surface: fielddraft_protocol::HalfSpace::RevolvedProfile { .. },
                                    ..
                                }
                                | SurfaceGeometry::Tessellated { .. } => bands.push(BandReq {
                                    qi,
                                    feature_id: face.branch_id.feature_id(),
                                    placement: *placement,
                                    loops: crate::brep::face_lateral_trim::sample_face_loops(
                                        brep, fid,
                                    ),
                                }),
                                SurfaceGeometry::Analytic { .. } => {
                                    let s = crate::brep::face_thicken::thicken_face(
                                        brep, fid, placement, outer_off, inner_off,
                                    )?;
                                    walls.push((qi, thicken_diff_node(id, qi, &s), s.world));
                                }
                            }
                            found = true;
                            break 'scan;
                        }
                    }
                }
                if !found {
                    return Err(format!("face {hash} not found in the current model"));
                }
            }
        }

        // Pass 2 (woven dropped): build the general field-offset band for each
        // non-analytic face from its source feature's closed carrier.
        for req in bands {
            let source = self
                .features
                .iter()
                .find(|f| crate::csg::BranchId::from_feature(&f.id).feature_id() == req.feature_id)
                .ok_or_else(|| {
                    "selected face has no source feature in the spine — cannot thicken".to_string()
                })?;
            let carrier = crate::brep::face_offset::single_primitive_carrier(&source.node)
                .ok_or_else(|| {
                    "face's carrier is a composite (boolean) body — general thicken of a \
                     non-analytic boolean result is deferred past M0"
                        .to_string()
                })?;
            // Pre-remap carrier drives the M1 wall-normal field (the sweep
            // direction); the remapped clone carries the band's fresh ids.
            let trim_base = id.child(format!("{}", req.qi));
            let carrier_for_norm = carrier.clone();
            let carrier = crate::brep::face_offset::remap_leaf_ids(carrier, &trim_base);
            let world = compose_placement(&req.placement, &source.transform);
            let band =
                crate::brep::face_offset::offset_band(carrier, outer_off, inner_off, world)?;
            // M1 lateral trim: bound the (possibly whole-carrier) band to exactly
            // the picked face by intersecting with its swept edge-loop prism. A
            // closed face / non-extrude carrier returns `None` → plain M0 band.
            let trim = crate::brep::face_lateral_trim::lateral_trim_node(
                &carrier_for_norm,
                &band.world,
                &req.placement,
                &req.loops,
                outer_off,
                inner_off,
                &trim_base,
            );
            let node = match trim {
                Some(t) => CsgNode::Intersection(Box::new(band.node), Box::new(t)),
                None => band.node,
            };
            walls.push((req.qi, node, band.world));
        }

        if walls.is_empty() {
            return Err("no faces selected to thicken".to_string());
        }
        // Deterministic order regardless of analytic/band interleaving.
        walls.sort_by_key(|(qi, _, _)| *qi);
        Ok(walls.into_iter().map(|(_, node, world)| (node, world)).collect())
    }

    /// Thicken each source face **up to** a coaxial `target` face: the wall grows
    /// from the source surface until it reaches the target (concentric sphere,
    /// coaxial cylinder, coaxial torus). The signed normal gap
    /// ([`offset_to_target`](crate::brep::face_thicken::offset_to_target)) sets
    /// the thickness *and* direction per source face, then the same
    /// [`thicken_face`](crate::brep::face_thicken::thicken_face) reduction builds
    /// the offset-solid pair — so up-to-face reuses the whole solid pipeline.
    ///
    /// Errors on a non-`qFace` query, an unresolvable handle, a source the
    /// surface layer refuses to thicken, or a target that is not a coaxial
    /// same-kind surface (a cone / mismatched / non-coaxial target is deferred).
    pub fn resolve_thicken_to_face(
        &mut self,
        id: &FeatureId,
        face_queries: &[String],
        target_query: &str,
    ) -> Result<Vec<(CsgNode, Transform)>, String> {
        use crate::brep::naming::TopoRef;
        use crate::brep::FaceId;
        use crate::queries::QueryExpr;

        let woven = self.brep_cache.ensure(&self.features);

        // Locate a `qFace(<hash>)` selection → (body index, FaceId).
        let find_face = |q: &str| -> Result<(usize, FaceId), String> {
            let expr = crate::queries::parse_query(q)
                .map_err(|e| format!("invalid face query `{q}`: {e:?}"))?;
            let QueryExpr::Face(hash) = expr else {
                return Err(format!("thicken faces must be `qFace(<id_hash>)` selections; got `{q}`"));
            };
            for (bi, (brep, _placement)) in woven.iter().enumerate() {
                for i in 0..brep.faces.len() {
                    let fid = FaceId(i as u32);
                    if TopoRef::from_face(brep, fid).id_hash == hash {
                        return Ok((bi, fid));
                    }
                }
            }
            Err(format!("face {hash} not found in the current model"))
        };

        let (tgt_bi, tgt_fid) = find_face(target_query)?;
        let mut out = Vec::new();
        for (qi, q) in face_queries.iter().enumerate() {
            let (src_bi, src_fid) = find_face(q)?;
            let (src_brep, src_placement) = &woven[src_bi];
            let (tgt_brep, tgt_placement) = &woven[tgt_bi];
            let (outer_off, inner_off) = crate::brep::face_thicken::offset_to_target(
                src_brep, src_fid, src_placement, tgt_brep, tgt_fid, tgt_placement,
            )?;
            let s = crate::brep::face_thicken::thicken_face(
                src_brep, src_fid, src_placement, outer_off, inner_off,
            )?;
            out.push((thicken_diff_node(id, qi, &s), s.world));
        }
        if out.is_empty() {
            return Err("no faces selected to thicken".to_string());
        }
        Ok(out)
    }

    /// Resolve a *plane* query to a world-space [`SketchFrame`],
    /// accepting both the well-known datum planes
    /// (`qCreatedBy(origin_plane_xy|xz|yz)`) and real B-Rep faces.
    /// Datum planes short-circuit before touching the woven B-Rep;
    /// everything else defers to [`Context::resolve_face_frame`].
    /// Returns `None` when the query resolves to neither.
    pub fn resolve_plane_frame(
        &mut self,
        expr: &crate::queries::QueryExpr,
    ) -> Option<crate::sketch_features::SketchFrame> {
        if let crate::queries::QueryExpr::CreatedBy(d) = expr {
            if let Some(datum) = crate::sketch_features::datum_plane_frame(d) {
                return Some(datum);
            }
        }
        self.resolve_face_frame(expr)
    }

    /// DL-61: resolve an edge-yielding query into per-edge fillet inputs:
    /// `(feature_a, feature_b, world-space polyline)`.
    ///
    /// COORDINATE SPACE: the woven B-Reps are **body-local** (DL-56); the
    /// paired `placement` maps body-local → world. The CSG tree that the
    /// MaskedBlend lives in evaluates in **world** space — every leaf is
    /// wrapped in `CsgNode::Transform(feature.transform, …)` by
    /// [`from_decoded`](crate::csg::CsgNode::from_decoded), and a feature's
    /// `transform` is its world placement. So the mask must sit on the
    /// edge in world space: we apply `placement` (rotate·scale + translate,
    /// same math as [`crate::sketch_features::face_plane_frame`]) to every
    /// polyline point. The non-identity-placement test proves this — with
    /// identity placement a missing transform would pass undetected.
    ///
    /// The feature pair comes from the edge's branch-pair `signature`:
    /// each [`BranchId`](crate::csg::BranchId)'s `feature_id()` is exactly
    /// the junction operand id [`find_matching_fillet`] keys on.
    pub fn resolve_fillet_edges(
        &mut self,
        expr: &crate::queries::QueryExpr,
    ) -> Vec<(FeatureId, FeatureId, Vec<[f32; 3]>)> {
        use crate::brep::naming::TopoRef;
        use crate::brep::{EdgeGeometry, EdgeId};

        let woven = self.brep_cache.ensure(&self.features);
        let mut out = Vec::new();
        for (brep, placement) in woven {
            for i in 0..brep.edges.len() {
                let id = EdgeId(i as u32);
                let edge = brep.edge(id);
                // Match the edge against the query. `qEdge(hash)` matches
                // by id_hash; `qEdgeBetween(a, b)` matches by the (unordered)
                // signature feature pair. Reuse the existing hashing /
                // signature logic rather than duplicating it.
                let hit = match expr {
                    crate::queries::QueryExpr::Edge(hash) => {
                        TopoRef::from_edge(brep, id).id_hash == *hash
                    }
                    crate::queries::QueryExpr::EdgeBetween(a, b) => {
                        let s0 = edge.signature.0.feature_id();
                        let s1 = edge.signature.1.feature_id();
                        let fa = a.as_u32_or_root();
                        let fb = b.as_u32_or_root();
                        matches!((fa, fb), (Some(fa), Some(fb))
                            if (s0 == fa && s1 == fb) || (s0 == fb && s1 == fa))
                    }
                    _ => false,
                };
                if !hit {
                    continue;
                }

                // Extract the body-local polyline from the edge geometry.
                let local: Vec<[f32; 3]> = match &edge.geometry {
                    EdgeGeometry::Polyline(pts) => {
                        pts.iter().map(|p| [p[0] as f32, p[1] as f32, p[2] as f32]).collect()
                    }
                    EdgeGeometry::Hermite(samples) => samples
                        .iter()
                        .map(|s| [s.point[0] as f32, s.point[1] as f32, s.point[2] as f32])
                        .collect(),
                    // First approximation: use the NURBS control polygon.
                    // Control points bound the curve and give a tube that
                    // safely encloses it; exact arc-length sampling is a
                    // later refinement.
                    EdgeGeometry::Nurbs(curve) => curve
                        .control_points
                        .iter()
                        .map(|p| [p[0] as f32, p[1] as f32, p[2] as f32])
                        .collect(),
                };

                // Body-local → world via the placement.
                let world: Vec<[f32; 3]> = local
                    .iter()
                    .map(|p| place_point(placement, *p))
                    .collect();

                let feature_a = FeatureId::from_u32(edge.signature.0.feature_id());
                let feature_b = FeatureId::from_u32(edge.signature.1.feature_id());
                out.push((feature_a, feature_b, world));
            }
        }
        out
    }

    /// Resolve an edge-yielding query into per-edge world-space SAMPLED curve
    /// polylines (dense tessellation via [`sample_edge`], NOT the NURBS control
    /// polygon `resolve_fillet_edges` uses) — the spine for an edge-chain sweep.
    /// A circle's control polygon has sharp corners that would trip the sweep
    /// Gatekeeper's kink check; the sampled curve is smooth.
    pub fn resolve_edge_polylines(
        &mut self,
        expr: &crate::queries::QueryExpr,
    ) -> Vec<Vec<[f32; 3]>> {
        use crate::brep::naming::TopoRef;
        use crate::brep::EdgeId;

        let woven = self.brep_cache.ensure(&self.features);
        let mut out = Vec::new();
        for (brep, placement) in woven {
            for i in 0..brep.edges.len() {
                let id = EdgeId(i as u32);
                let edge = brep.edge(id);
                let hit = match expr {
                    crate::queries::QueryExpr::Edge(hash) => {
                        TopoRef::from_edge(brep, id).id_hash == *hash
                    }
                    crate::queries::QueryExpr::EdgeBetween(a, b) => {
                        let s0 = edge.signature.0.feature_id();
                        let s1 = edge.signature.1.feature_id();
                        let fa = a.as_u32_or_root();
                        let fb = b.as_u32_or_root();
                        matches!((fa, fb), (Some(fa), Some(fb))
                            if (s0 == fa && s1 == fb) || (s0 == fb && s1 == fa))
                    }
                    _ => false,
                };
                if !hit {
                    continue;
                }
                let world: Vec<[f32; 3]> = crate::brep::edge_sampling::sample_edge(&edge.geometry)
                    .iter()
                    .map(|p| place_point(placement, [p[0] as f32, p[1] as f32, p[2] as f32]))
                    .collect();
                out.push(world);
            }
        }
        out
    }

    /// Resolve a mate-connector anchor query to its CSYS origin and, for an
    /// edge anchor, the unit tangent at arc-length fraction `t`. Faces resolve
    /// to their centroid, vertices to their world position, edges to the point
    /// at `t` along the sampled polyline. `None` if nothing matches.
    pub fn resolve_mate_anchor(
        &mut self,
        expr: &crate::queries::QueryExpr,
        t: f32,
    ) -> Option<MateAnchorFrame> {
        use crate::queries::QueryExpr;
        match expr {
            QueryExpr::Edge(_) | QueryExpr::EdgeBetween(_, _) => {
                use crate::brep::naming::TopoRef;
                use crate::brep::EdgeId;
                let woven = self.brep_cache.ensure(&self.features);
                for (brep, placement) in woven {
                    for i in 0..brep.edges.len() {
                        let eid = EdgeId(i as u32);
                        let edge = brep.edge(eid);
                        let hit = match expr {
                            QueryExpr::Edge(hash) => {
                                TopoRef::from_edge(brep, eid).id_hash == *hash
                            }
                            QueryExpr::EdgeBetween(a, b) => {
                                let s0 = edge.signature.0.feature_id();
                                let s1 = edge.signature.1.feature_id();
                                let (fa, fb) = (a.as_u32_or_root(), b.as_u32_or_root());
                                matches!((fa, fb), (Some(fa), Some(fb))
                                    if (s0 == fa && s1 == fb) || (s0 == fb && s1 == fa))
                            }
                            _ => false,
                        };
                        if !hit {
                            continue;
                        }
                        let poly: Vec<[f32; 3]> =
                            crate::brep::edge_sampling::sample_edge(&edge.geometry)
                                .iter()
                                .map(|p| {
                                    place_point(placement, [p[0] as f32, p[1] as f32, p[2] as f32])
                                })
                                .collect();
                        let (origin, tan) = polyline_point_tangent(&poly, t)?;
                        // Ticket o3er/5j9l: the edge IS the primary (+Z) axis;
                        // an adjacent face orients the secondary axis (auto).
                        let auto_secondary = face_normal_for_branch(brep, placement, edge.signature.0)
                            .or_else(|| face_normal_for_branch(brep, placement, edge.signature.1));
                        return Some(MateAnchorFrame {
                            origin,
                            primary: Some(tan),
                            auto_secondary,
                        });
                    }
                }
                None
            }
            QueryExpr::Vertex(hash) => {
                use crate::brep::naming::TopoRef;
                use crate::brep::VertexId;
                let woven = self.brep_cache.ensure(&self.features);
                for (brep, placement) in woven {
                    for i in 0..brep.vertices.len() {
                        let vid = VertexId(i as u32);
                        if TopoRef::from_vertex(brep, vid).id_hash == *hash {
                            let v = brep.vertex(vid);
                            let origin = place_point(
                                placement,
                                [v.position[0] as f32, v.position[1] as f32, v.position[2] as f32],
                            );
                            // A vertex has no intrinsic axis; an adjacent face
                            // normal defines +Z (Onshape: "normal to the face").
                            let auto_secondary = v
                                .branches
                                .iter()
                                .find_map(|b| face_normal_for_branch(brep, placement, *b));
                            return Some(MateAnchorFrame {
                                origin,
                                primary: None,
                                auto_secondary,
                            });
                        }
                    }
                }
                None
            }
            // Face / created-by / face-of: the anchor IS a face; its normal is +Z.
            _ => self.resolve_face_frame(expr).map(|f| MateAnchorFrame {
                origin: f.origin,
                primary: Some(f.normal),
                auto_secondary: None,
            }),
        }
    }

    /// Resolve an axis-line query to a single world-space segment
    /// `(start, end)`. Two query shapes are honoured, both used by the
    /// Revolve tool's axis field:
    ///
    /// * `qEdge(hash)` / `qEdgeBetween(a, b)` — a B-Rep edge. Its sampled
    ///   world polyline's first and last points become the segment
    ///   endpoints (a straight edge round-trips exactly; a curved edge
    ///   degrades to its chord, which is the only sensible axis anyway).
    /// * `qSketchLine(sketch_id, line_id)` — a raw `sketch_line` sub-call
    ///   (including construction lines, which never reach the B-Rep). Its
    ///   2D endpoints are lifted to world through the owning sketch's
    ///   placement frame.
    ///
    /// Returns `None` when the query names nothing resolvable (so the
    /// caller falls back to the default local-Y axis).
    pub fn resolve_axis_world_segment(
        &mut self,
        expr: &crate::queries::QueryExpr,
    ) -> Option<([f32; 3], [f32; 3])> {
        match expr {
            crate::queries::QueryExpr::Edge(_)
            | crate::queries::QueryExpr::EdgeBetween(_, _) => {
                let polylines = self.resolve_edge_polylines(expr);
                let poly = polylines.into_iter().find(|p| p.len() >= 2)?;
                Some((poly[0], poly[poly.len() - 1]))
            }
            crate::queries::QueryExpr::SketchLine(sketch_id, line_id) => {
                let sketch = self.sketches.get(sketch_id)?;
                let frame = &sketch.frame;
                let lift = |uv: [f32; 2]| -> [f32; 3] {
                    [
                        frame.origin[0] + uv[0] * frame.x_axis[0] + uv[1] * frame.y_axis[0],
                        frame.origin[1] + uv[0] * frame.x_axis[1] + uv[1] * frame.y_axis[1],
                        frame.origin[2] + uv[0] * frame.x_axis[2] + uv[1] * frame.y_axis[2],
                    ]
                };
                // Ticket pr8a: resolve ANY straight line entity by id from the
                // solved sketch state — not just raw `sketch_line` sub-calls.
                // Rectangle / polygon edges are line entities too, so this lets
                // them serve as a revolve axis like a free-standing line.
                use fielddraft_solver::Entity;
                let state = &sketch.state;
                let point_at = |id: fielddraft_solver::EntityId| -> Option<[f32; 2]> {
                    state.entities.iter().find_map(|e| match e {
                        Entity::Point { id: pid, at } if *pid == id => {
                            Some([at.x as f32, at.y as f32])
                        }
                        _ => None,
                    })
                };
                for e in &state.entities {
                    if let Entity::Line { id, start, end } = e {
                        if id.0 == *line_id {
                            let a = point_at(*start)?;
                            let b = point_at(*end)?;
                            return Some((lift(a), lift(b)));
                        }
                    }
                }
                None
            }
            _ => None,
        }
    }
}

/// Two world [`SketchFrame`](crate::sketch_features::SketchFrame)s are coplanar
/// iff their normals are (anti)parallel and the second origin lies in the first
/// frame's plane. Used to gate a coplanar multi-face extrude selection.
fn coplanar(
    a: &crate::sketch_features::SketchFrame,
    b: &crate::sketch_features::SketchFrame,
) -> bool {
    let dn = a.normal[0] * b.normal[0] + a.normal[1] * b.normal[1] + a.normal[2] * b.normal[2];
    if dn.abs() < 0.999 {
        return false;
    }
    let d = [
        b.origin[0] - a.origin[0],
        b.origin[1] - a.origin[1],
        b.origin[2] - a.origin[2],
    ];
    let off = d[0] * a.normal[0] + d[1] * a.normal[1] + d[2] * a.normal[2];
    off.abs() < 1e-2
}

/// Apply a body `placement` (rotate·scale then translate) to a point.
/// Mirrors the centroid mapping in
/// [`crate::sketch_features::face_plane_frame`].
fn place_point(placement: &fielddraft_protocol::Transform, p: [f32; 3]) -> [f32; 3] {
    let scaled = [
        p[0] * placement.scale[0],
        p[1] * placement.scale[1],
        p[2] * placement.scale[2],
    ];
    let r = crate::quat_rotate(placement.rotation, scaled);
    [
        r[0] + placement.translate[0],
        r[1] + placement.translate[1],
        r[2] + placement.translate[2],
    ]
}

// ─── Mate-connector CSYS helpers ─────────────────────────────────
//
// These mirror `crate::sketch_features::face_plane_frame` exactly (same
// seed-cross convention) so a connector's web-side inference preview and the
// kernel's authoritative resolve agree to f32 on the X spin.

fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize3v(v: [f32; 3]) -> [f32; 3] {
    let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if n > 1e-9 {
        [v[0] / n, v[1] / n, v[2] / n]
    } else {
        [0.0, 0.0, 1.0]
    }
}

/// Resolved geometry of a mate-connector anchor: its origin, an optional
/// intrinsic primary (+Z) axis (edge tangent or face normal — `None` for a
/// vertex), and an auto-picked adjacent-face normal that orients the secondary
/// axis (edge) or defines +Z (vertex).
pub struct MateAnchorFrame {
    pub origin: [f32; 3],
    pub primary: Option<[f32; 3]>,
    pub auto_secondary: Option<[f32; 3]>,
}

/// World-space normal of the first planar face on `branch`, or `None`.
fn face_normal_for_branch(
    brep: &crate::brep::BRep,
    placement: &fielddraft_protocol::Transform,
    branch: crate::csg::BranchId,
) -> Option<[f32; 3]> {
    for i in 0..brep.faces.len() {
        if brep.faces[i].branch_id == branch {
            return Some(
                crate::sketch_features::face_plane_frame(
                    brep,
                    crate::brep::FaceId(i as u32),
                    placement,
                )
                .normal,
            );
        }
    }
    None
}

/// Rotate the in-plane axis `x` (⟂ `z`) by `steps`·90° about `z`. Trig-free:
/// `z × x` is the +90° image, so each quarter turn is a sign/swap.
fn reorient_secondary(x: [f32; 3], z: [f32; 3], steps: u32) -> [f32; 3] {
    let perp = cross3(z, x); // +90°
    match steps & 3 {
        0 => x,
        1 => perp,
        2 => [-x[0], -x[1], -x[2]],
        _ => [-perp[0], -perp[1], -perp[2]],
    }
}

/// In-plane X basis for a normal `n`: cross `n` with the world axis least
/// aligned with it (the `face_plane_frame` convention).
fn seed_cross_x(n: [f32; 3]) -> [f32; 3] {
    let ax = n[0].abs();
    let ay = n[1].abs();
    let az = n[2].abs();
    let seed = if ax <= ay && ax <= az {
        [1.0, 0.0, 0.0]
    } else if ay <= az {
        [0.0, 1.0, 0.0]
    } else {
        [0.0, 0.0, 1.0]
    };
    normalize3v(cross3(seed, n))
}

/// Project `v` onto the plane ⟂ `n` and normalise; falls back to the
/// seed-cross basis if `v` is (near-)parallel to `n`.
fn project_perp(v: [f32; 3], n: [f32; 3]) -> [f32; 3] {
    let d = v[0] * n[0] + v[1] * n[1] + v[2] * n[2];
    let p = [v[0] - d * n[0], v[1] - d * n[1], v[2] - d * n[2]];
    if p[0] * p[0] + p[1] * p[1] + p[2] * p[2] > 1e-12 {
        normalize3v(p)
    } else {
        seed_cross_x(n)
    }
}

/// Point + unit tangent at arc-length fraction `t ∈ [0, 1]` along a world
/// polyline. Returns `None` for an empty polyline.
fn polyline_point_tangent(poly: &[[f32; 3]], t: f32) -> Option<([f32; 3], [f32; 3])> {
    if poly.is_empty() {
        return None;
    }
    if poly.len() == 1 {
        return Some((poly[0], [1.0, 0.0, 0.0]));
    }
    let seg = |a: [f32; 3], b: [f32; 3]| -> f32 {
        ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2) + (b[2] - a[2]).powi(2)).sqrt()
    };
    let total: f32 = poly.windows(2).map(|w| seg(w[0], w[1])).sum();
    if total <= 1e-9 {
        return Some((poly[0], [1.0, 0.0, 0.0]));
    }
    let target = t.clamp(0.0, 1.0) * total;
    let mut acc = 0.0f32;
    for i in 0..poly.len() - 1 {
        let a = poly[i];
        let b = poly[i + 1];
        let l = seg(a, b);
        if acc + l >= target || i == poly.len() - 2 {
            let local = if l > 1e-9 { (target - acc) / l } else { 0.0 };
            let p = [
                a[0] + (b[0] - a[0]) * local,
                a[1] + (b[1] - a[1]) * local,
                a[2] + (b[2] - a[2]) * local,
            ];
            let tan = normalize3v([b[0] - a[0], b[1] - a[1], b[2] - a[2]]);
            return Some((p, tan));
        }
        acc += l;
    }
    None
}

/// The behaviour of one feature type.
///
/// A `FeatureKind` is a stateless function table: the registry holds
/// one boxed trait object per type name, and every [`FeatureCall`]
/// with that type name in a document's feature tree dispatches to
/// the same `execute` function. Per-call parameters live in the
/// call's `params` JSON blob, not in the trait object.
///
/// Implementors are `Send + Sync` so a future K4.5 step can hand the
/// registry over to the Regeneration Worker without wrapping every
/// feature in a lock.
pub trait FeatureKind: Send + Sync {
    /// Registry key — stable, snake_case, matches the `type` field
    /// in the serialised feature tree. Built-ins return their static
    /// `TYPE_NAME`; a FieldSkript [`ScriptFeature`](crate::script_runtime::ScriptFeature)
    /// borrows its runtime `record.id`, so the return is `&self`-bound,
    /// not `'static`.
    fn type_name(&self) -> &str;

    /// K6a: the JSON schema shape the K4.5.4a `FeatureSchema`
    /// derive generates for the definition struct this feature
    /// kind consumes. Returned as `serde_json::Value` so the
    /// wasm-bindgen surface can serialise it straight through
    /// to the TypeScript FeatureDialog generator without the
    /// kernel needing to know about the widget layer.
    fn schema_json(&self) -> Value;

    /// Validate the definition JSON and mutate the context. On
    /// success every side effect is already baked into the context;
    /// on error the side effects accumulated so far stay, but the
    /// registry's `regenerate` pass will halt and refuse to run any
    /// further calls (fail-stop / Rollback semantics per §4.6).
    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError>;
}

/// Central registry of [`FeatureKind`] impls, keyed by type name.
pub struct FeatureRegistry {
    features: HashMap<String, Box<dyn FeatureKind>>,
}

impl Default for FeatureRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl FeatureRegistry {
    /// Empty registry. Use [`FeatureRegistry::register`] to add
    /// feature kinds before calling [`FeatureRegistry::regenerate`].
    pub fn new() -> Self {
        Self {
            features: HashMap::new(),
        }
    }

    /// Empty registry pre-populated with the three K1 primitives —
    /// `box`, `sphere`, `cylinder`. This is the intended entry
    /// point for all non-test callers in K4.5 step 2 and for the
    /// tests that want to exercise the full built-in surface.
    pub fn with_builtins() -> Self {
        let mut r = Self::new();
        r.register(Box::new(BoxPrimitive));
        r.register(Box::new(SpherePrimitive));
        r.register(Box::new(CylinderPrimitive));
        r.register(Box::new(ExtrudeFeature));
        r.register(Box::new(ThickenFeature));
        r.register(Box::new(RevolveFeature));
        r.register(Box::new(HelixFeature));
        r.register(Box::new(HoleFeature));
        r.register(Box::new(TorusPrimitive));
        r.register(Box::new(CapsulePrimitive));
        r.register(Box::new(EllipsoidPrimitive));
        r.register(Box::new(HexPrismPrimitive));
        r.register(Box::new(ConePrimitive));
        r.register(Box::new(ThreadFeature));
        r.register(Box::new(MateConnectorFeature));
        r.register(Box::new(SweepFeature));
        r.register(Box::new(FilletEdgeFeature));
        r.register(Box::new(ShellFeature));
        r.register(Box::new(ImportedMeshFeature));
        r.register(Box::new(GyroidFeature));
        r.register(Box::new(LinearPatternFeature));
        r.register(Box::new(CircularPatternFeature));
        r.register(Box::new(MirrorFeature));
        r.register(Box::new(crate::sketch_features::SketchFeature::new()));
        r.register(Box::new(TranslateFeature));
        r.register(Box::new(RotateFeature));
        r.register(Box::new(ScaleFeature));
        r.register(Box::new(BooleanFeature));
        r.register(Box::new(MaterialFeature));
        r
    }

    /// Register a feature kind. If one with the same type name is
    /// already registered it's replaced silently — the last
    /// registration wins, which lets tests stub a built-in without
    /// building a fresh registry from scratch.
    pub fn register(&mut self, feature: Box<dyn FeatureKind>) {
        self.features
            .insert(feature.type_name().to_string(), feature);
    }

    /// Look up a feature kind by type name.
    pub fn get(&self, type_name: &str) -> Option<&dyn FeatureKind> {
        self.features.get(type_name).map(|b| b.as_ref())
    }

    /// How many feature kinds are registered.
    pub fn len(&self) -> usize {
        self.features.len()
    }

    pub fn is_empty(&self) -> bool {
        self.features.is_empty()
    }

    /// K6a: list every type name the registry knows about. The
    /// order is hash-map-defined and must not be relied on by
    /// callers that need a stable ordering (the FeatureDialog
    /// generator sorts the result before rendering).
    pub fn type_names(&self) -> Vec<String> {
        self.features.keys().cloned().collect()
    }

    /// K6a: look up a feature kind's schema by type name. Returns
    /// the raw [`Value`] the `FeatureSchema` derive emits —
    /// wasm-bindgen callers re-serialise it as a string before
    /// handing it across the JS boundary.
    pub fn schema_for(&self, type_name: &str) -> Option<Value> {
        self.features.get(type_name).map(|f| f.schema_json())
    }

    /// Walk a feature-call list in order and execute each entry
    /// against the context. Fails fast on the first error: nothing
    /// after the failing call runs, and the caller can inspect the
    /// partially-mutated context to show the user which feature
    /// went red.
    /// Walk the calls in document order and execute each one. A failing
    /// feature is **recorded and skipped** instead of fail-stopping the
    /// whole document (Onshape semantics, ticket sx5a): it contributes no
    /// geometry, the rest of the model regenerates normally, and the UI
    /// surfaces the per-feature error (red title + tooltip). Returns the
    /// collected per-feature errors — empty means a clean regen.
    pub fn regenerate(
        &self,
        ctx: &mut Context<'_>,
        calls: &[FeatureCall],
        rollback_after: Option<usize>,
    ) -> RegenErrors {
        // K8.11: snapshot the variables table once per
        // regenerate — execute paths don't mutate it, and
        // cloning here means the param resolver can borrow
        // without tangling with the `ctx.document` mutable
        // borrow.
        // The formula resolver only needs name → numeric value; the variable
        // kind is a UI concern, so project the table down to an f32 map.
        let variables: std::collections::BTreeMap<String, f32> = ctx
            .document
            .variables
            .iter()
            .map(|(name, var)| (name.clone(), var.value))
            .collect();
        let mut errors: Vec<(FeatureId, FeatureError)> = Vec::new();
        for (i, call) in calls.iter().enumerate() {
            if call.suppressed || rollback_after.is_some_and(|ra| i >= ra) {
                continue;
            }
            let Some(feature) = self.features.get(&call.type_name) else {
                errors.push((
                    call.id.clone(),
                    FeatureError::UnknownFeatureType(call.type_name.clone()),
                ));
                continue;
            };
            // K8.11: resolve any formula-valued params (e.g.
            // `{value: "=width/2", unit: "mm"}`) against the
            // document variables before handing them off to
            // `execute`. Features see plain numeric values and
            // don't need to know the formula engine exists.
            let mut resolved = call.params.clone();
            if let Err(e) =
                crate::expr::resolve_params_with_variables(&mut resolved, &variables)
            {
                errors.push((
                    call.id.clone(),
                    FeatureError::InvalidParameter {
                        feature: "<formula>",
                        name: "params",
                        message: format!("{} (in {})", e, call.type_name),
                    },
                ));
                continue;
            }
            if let Err(e) = feature.execute(ctx, &call.id, &resolved) {
                errors.push((call.id.clone(), e));
            }
        }
        RegenErrors(errors)
    }
}

/// Per-feature errors collected by the error-tolerant [`FeatureRegistry::
/// regenerate`] (ticket sx5a) — one entry per failing call, in document
/// order. The `unwrap`/`unwrap_err`/`expect`/`is_err` mirrors keep the
/// pre-tolerance `Result` call-site ergonomics: "unwrap" asserts a clean
/// regen, "unwrap_err" asserts at least one failure and yields the first.
#[derive(Debug, Default)]
#[must_use = "a regen may have recorded per-feature errors"]
pub struct RegenErrors(pub Vec<(FeatureId, FeatureError)>);

impl RegenErrors {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn is_ok(&self) -> bool {
        self.0.is_empty()
    }

    pub fn is_err(&self) -> bool {
        !self.0.is_empty()
    }

    /// First failing feature's error, if any.
    pub fn err(self) -> Option<FeatureError> {
        self.0.into_iter().next().map(|(_, e)| e)
    }

    /// Panic if any feature failed (mirror of the old `Result::unwrap`).
    #[track_caller]
    pub fn unwrap(self) {
        if let Some((id, err)) = self.0.first() {
            panic!("feature {id} failed to regenerate: {err}");
        }
    }

    /// Panic with `msg` if any feature failed (mirror of `Result::expect`).
    #[track_caller]
    pub fn expect(self, msg: &str) {
        if let Some((id, err)) = self.0.first() {
            panic!("{msg}: feature {id} failed to regenerate: {err}");
        }
    }

    /// Panic if the regen was CLEAN; otherwise return the first failing
    /// feature's error (mirror of the old `Result::unwrap_err`).
    #[track_caller]
    pub fn unwrap_err(self) -> FeatureError {
        match self.0.into_iter().next() {
            Some((_, e)) => e,
            None => panic!("regenerate succeeded but an error was expected"),
        }
    }

    /// Like [`Self::unwrap_err`] but with a caller message (mirror of
    /// `Result::expect_err`).
    #[track_caller]
    pub fn expect_err(self, msg: &str) -> FeatureError {
        match self.0.into_iter().next() {
            Some((_, e)) => e,
            None => panic!("{msg}: regenerate succeeded but an error was expected"),
        }
    }
}

// ─── built-in K1 primitives ──────────────────────────────────────
//
// These are the reference `FeatureKind` impls for the three K1
// primitives. Their parameter parsing is now driven by the
// `FeatureSchema` derive from `fielddraft-features-macros`
// (K4.5.3), so each `execute` body shrinks to a two-step recipe:
// parse the definition JSON into the typed `{Box,Sphere,Cylinder}
// Def` struct, then lower it into a `DecodedFeature` (via
// `DecodedFeature::from_parts`) and push it onto the spine.
//
// The display name still has to be assembled at runtime because
// it depends on the `FeatureId` that's passed into `execute` —
// the derive's `default` attribute can only quote Rust rvalues,
// it can't capture closures over the id. That one extra line at
// the top of each execute function is the only hand-written
// parameter logic that remains in these three built-ins.

#[allow(clippy::too_many_arguments)] // geometry helper; param-struct refactor out of scope
fn push_primitive(
    ctx: &mut Context<'_>,
    id: &FeatureId,
    def: &Value,
    type_label: &str,
    op: BoolOp,
    targets: &MultiQuery,
    primitive: Primitive,
    translate: [f32; 3],
    mods: PrimitiveModifiers,
) {
    let name = optional_display_name(def, || format!("{type_label} {id}"));
    let rotation = optional_rotation(def);
    let scale = optional_scale(def);
    // A primitive set to Remove/Intersect (or Add) needs an explicit scope —
    // the body it acts on — just like the other booleans. The `targets` query
    // (`qCreatedBy(...)`) names the bodies it folds into; with none, warn and
    // let the partition pass seed a standalone (red preview) body. (New = a
    // fresh standalone solid, never warned.)
    let target_ids = target_ids_from_queries(targets);
    if let Some(w) = ctx.merge_scope_warning(op, &target_ids) {
        ctx.warn_feature(id, w);
    }
    ctx.features.push(DecodedFeature::from_parts(
        id.clone(),
        name,
        op,
        primitive,
        Transform { translate, rotation, scale },
        mods,
        target_ids,
    ));
}

/// `box` — K1 axis-aligned box primitive.
#[derive(FeatureSchema)]
#[schema(type = "box")]
pub struct BoxDef {
    #[param(unit = "mm")]
    pub half_extents: [f32; 3],
    #[param(default = [0.0, 0.0, 0.0])]
    pub center: [f32; 3],
    #[param(default = [0.0, 0.0, 0.0])]
    pub translate: [f32; 3],
    #[param(name = "Op", default = ::fielddraft_protocol::BoolOp::New)]
    pub op: BoolOp,
    /// Target bodies for the boolean op (Add/Remove/Intersect), as
    /// `qCreatedBy(<seed_feature>)` queries. Empty for New (a fresh standalone
    /// solid); required for the other ops (else warned + red preview).
    #[param(name = "Targets", default = ::fielddraft_protocol::MultiQuery::new(""), entity_filter = "BODY")]
    pub targets: ::fielddraft_protocol::MultiQuery,
    #[param(name = "Blend radius", default = 0.0)]
    pub blend_radius: f32,
    #[param(name = "Chamfer size", default = 0.0)]
    pub chamfer_size: f32,
    #[param(name = "Twist rate", default = 0.0)]
    pub twist_rate: f32,
    #[param(name = "Bend rate", default = 0.0)]
    pub bend_rate: f32,
    #[param(name = "Displace depth", default = 0.0)]
    pub displace_depth: f32,
    #[param(name = "Displace freq", default = 0.0)]
    pub displace_freq: f32,
    #[param(name = "Shell thickness", default = 0.0)]
    pub shell_thickness: f32,
}

pub struct BoxPrimitive;

impl FeatureKind for BoxPrimitive {
    fn type_name(&self) -> &'static str {
        BoxDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        BoxDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = BoxDef::parse(def)?;
        push_primitive(ctx, id, def, "Box", d.op, &d.targets,
            Primitive::Box { center: d.center, half_extents: d.half_extents },
            d.translate,
            PrimitiveModifiers {
                blend_radius: d.blend_radius, chamfer_size: d.chamfer_size,
                twist_rate: d.twist_rate, bend_rate: d.bend_rate,
                displace_depth: d.displace_depth, displace_freq: d.displace_freq,
                shell_thickness: d.shell_thickness,
            },
        );
        Ok(())
    }
}

/// `sphere` — K1 sphere primitive.
#[derive(FeatureSchema)]
#[schema(type = "sphere")]
pub struct SphereDef {
    #[param(unit = "mm")]
    pub radius: f32,
    #[param(default = [0.0, 0.0, 0.0])]
    pub center: [f32; 3],
    #[param(default = [0.0, 0.0, 0.0])]
    pub translate: [f32; 3],
    #[param(default = ::fielddraft_protocol::BoolOp::New)]
    pub op: BoolOp,
    /// Target bodies for the boolean op (Add/Remove/Intersect), as
    /// `qCreatedBy(<seed_feature>)` queries. Empty for New; required otherwise.
    #[param(name = "Targets", default = ::fielddraft_protocol::MultiQuery::new(""), entity_filter = "BODY")]
    pub targets: ::fielddraft_protocol::MultiQuery,
    #[param(name = "Blend radius", default = 0.0)]
    pub blend_radius: f32,
    #[param(name = "Chamfer size", default = 0.0)]
    pub chamfer_size: f32,
    #[param(name = "Twist rate", default = 0.0)]
    pub twist_rate: f32,
    #[param(name = "Bend rate", default = 0.0)]
    pub bend_rate: f32,
    #[param(name = "Displace depth", default = 0.0)]
    pub displace_depth: f32,
    #[param(name = "Displace freq", default = 0.0)]
    pub displace_freq: f32,
    #[param(name = "Shell thickness", default = 0.0)]
    pub shell_thickness: f32,
}

pub struct SpherePrimitive;

impl FeatureKind for SpherePrimitive {
    fn type_name(&self) -> &'static str {
        SphereDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        SphereDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = SphereDef::parse(def)?;
        positive_f32(d.radius, SphereDef::TYPE_NAME, "radius")?;
        push_primitive(ctx, id, def, "Sphere", d.op, &d.targets,
            Primitive::Sphere { center: d.center, radius: d.radius },
            d.translate,
            PrimitiveModifiers {
                blend_radius: d.blend_radius, chamfer_size: d.chamfer_size,
                twist_rate: d.twist_rate, bend_rate: d.bend_rate,
                displace_depth: d.displace_depth, displace_freq: d.displace_freq,
                shell_thickness: d.shell_thickness,
            },
        );
        Ok(())
    }
}

/// `cylinder` — K1 Y-axis-aligned cylinder primitive.
#[derive(FeatureSchema)]
#[schema(type = "cylinder")]
pub struct CylinderDef {
    #[param(unit = "mm")]
    pub radius: f32,
    #[param(unit = "mm")]
    pub half_height: f32,
    #[param(default = [0.0, 0.0, 0.0])]
    pub center: [f32; 3],
    #[param(default = [0.0, 0.0, 0.0])]
    pub translate: [f32; 3],
    #[param(default = ::fielddraft_protocol::BoolOp::New)]
    pub op: BoolOp,
    /// Target bodies for the boolean op (Add/Remove/Intersect), as
    /// `qCreatedBy(<seed_feature>)` queries. Empty for New; required otherwise.
    #[param(name = "Targets", default = ::fielddraft_protocol::MultiQuery::new(""), entity_filter = "BODY")]
    pub targets: ::fielddraft_protocol::MultiQuery,
    #[param(name = "Blend radius", default = 0.0)]
    pub blend_radius: f32,
    #[param(name = "Chamfer size", default = 0.0)]
    pub chamfer_size: f32,
    #[param(name = "Twist rate", default = 0.0)]
    pub twist_rate: f32,
    #[param(name = "Bend rate", default = 0.0)]
    pub bend_rate: f32,
    #[param(name = "Displace depth", default = 0.0)]
    pub displace_depth: f32,
    #[param(name = "Displace freq", default = 0.0)]
    pub displace_freq: f32,
    #[param(name = "Shell thickness", default = 0.0)]
    pub shell_thickness: f32,
}

pub struct CylinderPrimitive;

impl FeatureKind for CylinderPrimitive {
    fn type_name(&self) -> &'static str {
        CylinderDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        CylinderDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = CylinderDef::parse(def)?;
        positive_f32(d.radius, CylinderDef::TYPE_NAME, "radius")?;
        positive_f32(d.half_height, CylinderDef::TYPE_NAME, "half_height")?;
        push_primitive(ctx, id, def, "Cylinder", d.op, &d.targets,
            Primitive::Cylinder { center: d.center, radius: d.radius, half_height: d.half_height },
            d.translate,
            PrimitiveModifiers {
                blend_radius: d.blend_radius, chamfer_size: d.chamfer_size,
                twist_rate: d.twist_rate, bend_rate: d.bend_rate,
                displace_depth: d.displace_depth, displace_freq: d.displace_freq,
                shell_thickness: d.shell_thickness,
            },
        );
        Ok(())
    }
}

// ─── Cone (truncated cone / frustum) ─────────────────────────────

/// `cone` — truncated cone along the Y axis.
#[derive(FeatureSchema)]
#[schema(type = "cone")]
pub struct ConeDef {
    #[param(unit = "mm")]
    pub r_bottom: f32,
    #[param(unit = "mm")]
    pub r_top: f32,
    #[param(unit = "mm")]
    pub half_height: f32,
    #[param(default = [0.0, 0.0, 0.0])]
    pub translate: [f32; 3],
    #[param(default = ::fielddraft_protocol::BoolOp::New)]
    pub op: BoolOp,
    /// Target bodies for the boolean op (Add/Remove/Intersect), as
    /// `qCreatedBy(<seed_feature>)` queries. Empty for New; required otherwise.
    #[param(name = "Targets", default = ::fielddraft_protocol::MultiQuery::new(""), entity_filter = "BODY")]
    pub targets: ::fielddraft_protocol::MultiQuery,
    #[param(name = "Blend radius", default = 0.0)]
    pub blend_radius: f32,
    #[param(name = "Chamfer size", default = 0.0)]
    pub chamfer_size: f32,
    #[param(name = "Twist rate", default = 0.0)]
    pub twist_rate: f32,
    #[param(name = "Bend rate", default = 0.0)]
    pub bend_rate: f32,
    #[param(name = "Displace depth", default = 0.0)]
    pub displace_depth: f32,
    #[param(name = "Displace freq", default = 0.0)]
    pub displace_freq: f32,
    #[param(name = "Shell thickness", default = 0.0)]
    pub shell_thickness: f32,
}

pub struct ConePrimitive;

impl FeatureKind for ConePrimitive {
    fn type_name(&self) -> &'static str {
        ConeDef::TYPE_NAME
    }
    fn schema_json(&self) -> Value {
        ConeDef::schema_json()
    }
    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = ConeDef::parse(def)?;
        positive_f32(d.half_height, ConeDef::TYPE_NAME, "half_height")?;
        push_primitive(ctx, id, def, "Cone", d.op, &d.targets,
            Primitive::Cone { r_bottom: d.r_bottom, r_top: d.r_top, half_height: d.half_height },
            d.translate,
            PrimitiveModifiers {
                blend_radius: d.blend_radius, chamfer_size: d.chamfer_size,
                twist_rate: d.twist_rate, bend_rate: d.bend_rate,
                displace_depth: d.displace_depth, displace_freq: d.displace_freq,
                shell_thickness: d.shell_thickness,
            },
        );
        Ok(())
    }
}

// ─── K12: Thread ─────────────────────────────────────────────────

/// `thread` — metric-style screw thread via direct helix SDF.
#[derive(FeatureSchema)]
#[schema(type = "thread")]
pub struct ThreadDef {
    /// Nominal (outer) radius in mm.
    #[param(unit = "mm")]
    pub nominal_radius: f32,
    /// Pitch — distance per revolution in mm.
    #[param(unit = "mm")]
    pub pitch: f32,
    /// Thread depth in mm (groove amplitude).
    #[param(unit = "mm")]
    pub depth: f32,
    /// Half the total bolt length along Y.
    #[param(unit = "mm")]
    pub half_height: f32,
    #[param(default = [0.0, 0.0, 0.0])]
    pub translate: [f32; 3],
    #[param(default = ::fielddraft_protocol::BoolOp::New)]
    pub op: BoolOp,
    /// Target bodies for the boolean op (Add/Remove/Intersect), as
    /// `qCreatedBy(<seed_feature>)` queries. Empty for New; required otherwise.
    #[param(name = "Targets", default = ::fielddraft_protocol::MultiQuery::new(""), entity_filter = "BODY")]
    pub targets: ::fielddraft_protocol::MultiQuery,
    #[param(name = "Blend radius", default = 0.0)]
    pub blend_radius: f32,
    #[param(name = "Chamfer size", default = 0.0)]
    pub chamfer_size: f32,
}

pub struct ThreadFeature;

impl FeatureKind for ThreadFeature {
    fn type_name(&self) -> &'static str {
        ThreadDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        ThreadDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = ThreadDef::parse(def)?;
        positive_f32(d.nominal_radius, ThreadDef::TYPE_NAME, "nominal_radius")?;
        positive_f32(d.pitch, ThreadDef::TYPE_NAME, "pitch")?;
        positive_f32(d.depth, ThreadDef::TYPE_NAME, "depth")?;
        positive_f32(d.half_height, ThreadDef::TYPE_NAME, "half_height")?;
        push_primitive(ctx, id, def, "Thread", d.op, &d.targets,
            Primitive::Thread {
                nominal_radius: d.nominal_radius, pitch: d.pitch,
                depth: d.depth, half_height: d.half_height,
            },
            d.translate,
            PrimitiveModifiers {
                blend_radius: d.blend_radius, chamfer_size: d.chamfer_size,
                twist_rate: 0.0, bend_rate: 0.0,
                displace_depth: 0.0, displace_freq: 0.0,
                shell_thickness: 0.0,
            },
        );
        Ok(())
    }
}

// ─── Torus ───────────────────────────────────────────────────────

/// `torus` — Y-axis-aligned torus (donut).
#[derive(FeatureSchema)]
#[schema(type = "torus")]
pub struct TorusDef {
    /// Distance from the axis to the centre of the tube.
    #[param(unit = "mm")]
    pub major_radius: f32,
    /// Radius of the tube cross-section.
    #[param(unit = "mm")]
    pub minor_radius: f32,
    #[param(default = [0.0, 0.0, 0.0])]
    pub center: [f32; 3],
    #[param(default = [0.0, 0.0, 0.0])]
    pub translate: [f32; 3],
    #[param(default = ::fielddraft_protocol::BoolOp::New)]
    pub op: BoolOp,
    /// Target bodies for the boolean op (Add/Remove/Intersect), as
    /// `qCreatedBy(<seed_feature>)` queries. Empty for New; required otherwise.
    #[param(name = "Targets", default = ::fielddraft_protocol::MultiQuery::new(""), entity_filter = "BODY")]
    pub targets: ::fielddraft_protocol::MultiQuery,
    #[param(name = "Blend radius", default = 0.0)]
    pub blend_radius: f32,
    #[param(name = "Chamfer size", default = 0.0)]
    pub chamfer_size: f32,
    #[param(name = "Twist rate", default = 0.0)]
    pub twist_rate: f32,
    #[param(name = "Bend rate", default = 0.0)]
    pub bend_rate: f32,
    #[param(name = "Displace depth", default = 0.0)]
    pub displace_depth: f32,
    #[param(name = "Displace freq", default = 0.0)]
    pub displace_freq: f32,
    #[param(name = "Shell thickness", default = 0.0)]
    pub shell_thickness: f32,
}

pub struct TorusPrimitive;

impl FeatureKind for TorusPrimitive {
    fn type_name(&self) -> &'static str {
        TorusDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        TorusDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = TorusDef::parse(def)?;
        positive_f32(d.major_radius, TorusDef::TYPE_NAME, "major_radius")?;
        positive_f32(d.minor_radius, TorusDef::TYPE_NAME, "minor_radius")?;
        push_primitive(ctx, id, def, "Torus", d.op, &d.targets,
            Primitive::Torus { center: d.center, major_radius: d.major_radius, minor_radius: d.minor_radius },
            d.translate,
            PrimitiveModifiers {
                blend_radius: d.blend_radius, chamfer_size: d.chamfer_size,
                twist_rate: d.twist_rate, bend_rate: d.bend_rate,
                displace_depth: d.displace_depth, displace_freq: d.displace_freq,
                shell_thickness: d.shell_thickness,
            },
        );
        Ok(())
    }
}

// ─── Capsule ─────────────────────────────────────────────────────

/// `capsule` — Y-axis-aligned capsule (line-segment swept sphere).
/// `half_height` is the half-length of the inner segment;
/// total height = 2 * half_height + 2 * radius.
#[derive(FeatureSchema)]
#[schema(type = "capsule")]
pub struct CapsuleDef {
    #[param(unit = "mm")]
    pub radius: f32,
    #[param(unit = "mm")]
    pub half_height: f32,
    #[param(default = [0.0, 0.0, 0.0])]
    pub translate: [f32; 3],
    #[param(default = ::fielddraft_protocol::BoolOp::New)]
    pub op: BoolOp,
    /// Target bodies for the boolean op (Add/Remove/Intersect), as
    /// `qCreatedBy(<seed_feature>)` queries. Empty for New; required otherwise.
    #[param(name = "Targets", default = ::fielddraft_protocol::MultiQuery::new(""), entity_filter = "BODY")]
    pub targets: ::fielddraft_protocol::MultiQuery,
    #[param(name = "Blend radius", default = 0.0)]
    pub blend_radius: f32,
    #[param(name = "Chamfer size", default = 0.0)]
    pub chamfer_size: f32,
    #[param(name = "Twist rate", default = 0.0)]
    pub twist_rate: f32,
    #[param(name = "Bend rate", default = 0.0)]
    pub bend_rate: f32,
    #[param(name = "Displace depth", default = 0.0)]
    pub displace_depth: f32,
    #[param(name = "Displace freq", default = 0.0)]
    pub displace_freq: f32,
    #[param(name = "Shell thickness", default = 0.0)]
    pub shell_thickness: f32,
}

pub struct CapsulePrimitive;

impl FeatureKind for CapsulePrimitive {
    fn type_name(&self) -> &'static str {
        CapsuleDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        CapsuleDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = CapsuleDef::parse(def)?;
        positive_f32(d.radius, CapsuleDef::TYPE_NAME, "radius")?;
        positive_f32(d.half_height, CapsuleDef::TYPE_NAME, "half_height")?;
        push_primitive(ctx, id, def, "Capsule", d.op, &d.targets,
            Primitive::Capsule { radius: d.radius, half_height: d.half_height },
            d.translate,
            PrimitiveModifiers {
                blend_radius: d.blend_radius, chamfer_size: d.chamfer_size,
                twist_rate: d.twist_rate, bend_rate: d.bend_rate,
                displace_depth: d.displace_depth, displace_freq: d.displace_freq,
                shell_thickness: d.shell_thickness,
            },
        );
        Ok(())
    }
}

// ─── Ellipsoid ───────────────────────────────────────────────────

/// `ellipsoid` — axis-aligned ellipsoid with independent X/Y/Z radii.
/// SDF is approximate (coordinate scaling + sphere) but accurate enough
/// for DC meshing.
#[derive(FeatureSchema)]
#[schema(type = "ellipsoid")]
pub struct EllipsoidDef {
    #[param(default = [1.0, 1.0, 1.0], unit = "mm")]
    pub radii: [f32; 3],
    #[param(default = [0.0, 0.0, 0.0])]
    pub translate: [f32; 3],
    #[param(default = ::fielddraft_protocol::BoolOp::New)]
    pub op: BoolOp,
    /// Target bodies for the boolean op (Add/Remove/Intersect), as
    /// `qCreatedBy(<seed_feature>)` queries. Empty for New; required otherwise.
    #[param(name = "Targets", default = ::fielddraft_protocol::MultiQuery::new(""), entity_filter = "BODY")]
    pub targets: ::fielddraft_protocol::MultiQuery,
    #[param(name = "Blend radius", default = 0.0)]
    pub blend_radius: f32,
    #[param(name = "Chamfer size", default = 0.0)]
    pub chamfer_size: f32,
    #[param(name = "Twist rate", default = 0.0)]
    pub twist_rate: f32,
    #[param(name = "Bend rate", default = 0.0)]
    pub bend_rate: f32,
    #[param(name = "Displace depth", default = 0.0)]
    pub displace_depth: f32,
    #[param(name = "Displace freq", default = 0.0)]
    pub displace_freq: f32,
    #[param(name = "Shell thickness", default = 0.0)]
    pub shell_thickness: f32,
}

pub struct EllipsoidPrimitive;

impl FeatureKind for EllipsoidPrimitive {
    fn type_name(&self) -> &'static str {
        EllipsoidDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        EllipsoidDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = EllipsoidDef::parse(def)?;
        positive_f32(d.radii[0], EllipsoidDef::TYPE_NAME, "radii[0]")?;
        positive_f32(d.radii[1], EllipsoidDef::TYPE_NAME, "radii[1]")?;
        positive_f32(d.radii[2], EllipsoidDef::TYPE_NAME, "radii[2]")?;
        push_primitive(ctx, id, def, "Ellipsoid", d.op, &d.targets,
            Primitive::Ellipsoid { radii: d.radii },
            d.translate,
            PrimitiveModifiers {
                blend_radius: d.blend_radius, chamfer_size: d.chamfer_size,
                twist_rate: d.twist_rate, bend_rate: d.bend_rate,
                displace_depth: d.displace_depth, displace_freq: d.displace_freq,
                shell_thickness: d.shell_thickness,
            },
        );
        Ok(())
    }
}

// ─── HexPrism ────────────────────────────────────────────────────

/// `hex_prism` — Y-axis-aligned regular hexagonal prism.
/// `apothem` is the inradius (centre to flat face); circumradius ≈ apothem / 0.866.
#[derive(FeatureSchema)]
#[schema(type = "hex_prism")]
pub struct HexPrismDef {
    #[param(unit = "mm")]
    pub apothem: f32,
    #[param(unit = "mm")]
    pub half_height: f32,
    #[param(default = [0.0, 0.0, 0.0])]
    pub translate: [f32; 3],
    #[param(default = ::fielddraft_protocol::BoolOp::New)]
    pub op: BoolOp,
    /// Target bodies for the boolean op (Add/Remove/Intersect), as
    /// `qCreatedBy(<seed_feature>)` queries. Empty for New; required otherwise.
    #[param(name = "Targets", default = ::fielddraft_protocol::MultiQuery::new(""), entity_filter = "BODY")]
    pub targets: ::fielddraft_protocol::MultiQuery,
    #[param(name = "Blend radius", default = 0.0)]
    pub blend_radius: f32,
    #[param(name = "Chamfer size", default = 0.0)]
    pub chamfer_size: f32,
    #[param(name = "Twist rate", default = 0.0)]
    pub twist_rate: f32,
    #[param(name = "Bend rate", default = 0.0)]
    pub bend_rate: f32,
    #[param(name = "Displace depth", default = 0.0)]
    pub displace_depth: f32,
    #[param(name = "Displace freq", default = 0.0)]
    pub displace_freq: f32,
    #[param(name = "Shell thickness", default = 0.0)]
    pub shell_thickness: f32,
}

pub struct HexPrismPrimitive;

impl FeatureKind for HexPrismPrimitive {
    fn type_name(&self) -> &'static str {
        HexPrismDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        HexPrismDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = HexPrismDef::parse(def)?;
        positive_f32(d.apothem, HexPrismDef::TYPE_NAME, "apothem")?;
        positive_f32(d.half_height, HexPrismDef::TYPE_NAME, "half_height")?;
        push_primitive(ctx, id, def, "HexPrism", d.op, &d.targets,
            Primitive::HexPrism { apothem: d.apothem, half_height: d.half_height },
            d.translate,
            PrimitiveModifiers {
                blend_radius: d.blend_radius, chamfer_size: d.chamfer_size,
                twist_rate: d.twist_rate, bend_rate: d.bend_rate,
                displace_depth: d.displace_depth, displace_freq: d.displace_freq,
                shell_thickness: d.shell_thickness,
            },
        );
        Ok(())
    }
}

// ─── K13.2: Sweep Feature (curved path) ─────────────────────────
//
// Sweeps a 2D sketch profile along an arbitrary 3D spine, built as a
// `CsgNode::Sweep`. The path source is decoupled behind the modular
// `path_provider` layer; v1 uses the Gizmo3d provider (control points +
// Catmull-Rom spline). The Gatekeeper (in classify) decides parametric
// mesh vs. SDF fallback; the profile is carried in the profile's own frame.

/// The sweep's spine source — one tagged variant per modular path-provider
/// method (see [`crate::path_provider`]). The consumer never branches on
/// this; `execute` resolves it to a `SampledPath`.
#[derive(Debug, Clone, ::serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PathSourceDef {
    /// 3D control points routed in space + Catmull-Rom spline.
    Gizmo3d {
        nodes: Vec<[f32; 3]>,
        #[serde(default)]
        closed: bool,
    },
    /// Planar curve from a 2D sketch (`qCreatedBy(<sketch>)`).
    Sketch2d {
        sketch: ::fielddraft_protocol::Query,
    },
    /// Existing model edges selected by an edge query (`qEdge`/`qEdgeBetween`).
    EdgeChain {
        edges: ::fielddraft_protocol::Query,
    },
    /// Two orthogonal sketches sharing an axis → a 3D space curve (two-view
    /// wire). `sketch_a`/`sketch_b` are `qCreatedBy(<sketch>)` selections.
    OrthogonalSketches {
        sketch_a: ::fielddraft_protocol::Query,
        sketch_b: ::fielddraft_protocol::Query,
    },
}

/// `sweep` — profile swept along a spine resolved from a modular path source.
#[derive(FeatureSchema)]
#[schema(type = "sweep")]
pub struct SweepDef {
    pub profiles: ::fielddraft_protocol::Query,
    /// Modular path source (Gizmo3d control points or a 2D sketch curve).
    pub path: PathSourceDef,
    /// Total profile twist along the spine, in degrees (0 = no twist).
    #[param(default = 0.0)]
    pub twist_deg: f32,
    #[param(default = ::fielddraft_protocol::BoolOp::Add)]
    pub operation: BoolOp,

    /// Merge scope: explicit target bodies for the boolean op, as
    /// `qCreatedBy(<seed_feature>)` queries. Empty + `merge_with_all` off =
    /// no merge (standalone body; warned for Add/Remove/Intersect).
    #[param(name = "Merge scope", default = ::fielddraft_protocol::MultiQuery::new(""), entity_filter = "BODY")]
    pub targets: ::fielddraft_protocol::MultiQuery,

    /// Merge with all (Onshape): fold the body into EVERY body it touches
    /// instead of an explicit scope. Off by default — touching solids never
    /// merge implicitly; opt in here or name targets above.
    #[param(name = "Merge with all", default = false)]
    pub merge_with_all: bool,
}

/// Resolve a `qCreatedBy(<sketch>)` query to its ordered 2D curve (closed
/// loop if it bounds a region, else the open drawn curve) plus the sketch's
/// plane frame. Shared by the Sketch2d and OrthogonalSketches path sources.
fn resolve_sketch_curve_2d(
    sketches: &std::collections::HashMap<FeatureId, crate::sketch_features::SketchData>,
    query: &::fielddraft_protocol::Query,
) -> Result<(Vec<[f32; 2]>, bool, crate::sketch_features::SketchFrame), FeatureError> {
    let id = match crate::queries::parse_query(query.as_str()) {
        Ok(crate::queries::QueryExpr::CreatedBy(s)) => s,
        _ => {
            return Err(FeatureError::InvalidParameter {
                feature: SweepDef::TYPE_NAME,
                name: "path",
                message: "sketch query must be qCreatedBy(<sketch>)".to_string(),
            })
        }
    };
    let sk = sketches.get(&id).ok_or(FeatureError::MissingParameter {
        feature: SweepDef::TYPE_NAME,
        name: "path",
    })?;
    let bounded = crate::sketch2d::BoundedSketch2d::from_sketch(
        &crate::sketch_features::build_sketch2d_from_sub_calls(&sk.sub_calls),
    );
    let closed_loop = crate::sketch2d::extract_contours(&bounded, crate::sketch2d::CONTOUR_RESOLUTION)
        .into_iter()
        .next()
        .map(|c| c.outer)
        .filter(|o| o.len() >= 2);
    let (curve, closed) = match closed_loop {
        Some(loop_curve) => (loop_curve, true),
        None => (
            crate::sketch_features::open_path_curve_from_sub_calls(&sk.sub_calls).ok_or(
                FeatureError::InvalidParameter {
                    feature: SweepDef::TYPE_NAME,
                    name: "path",
                    message: "sketch has no usable planar curve".to_string(),
                },
            )?,
            false,
        ),
    };
    Ok((curve, closed, sk.frame))
}

pub struct SweepFeature;

impl FeatureKind for SweepFeature {
    fn type_name(&self) -> &'static str {
        SweepDef::TYPE_NAME
    }
    fn schema_json(&self) -> Value {
        SweepDef::schema_json()
    }
    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = SweepDef::parse(def)?;

        // Resolve the sketch profile (same convention as extrude/revolve).
        let sketch = (|| -> Option<::fielddraft_protocol::Sketch2d> {
            let expr = crate::queries::parse_query(d.profiles.as_str()).ok()?;
            let crate::queries::QueryExpr::CreatedBy(sketch_id) = expr else {
                return None;
            };
            let sketch_data = ctx.sketches.get(&sketch_id)?;
            let s = crate::sketch_features::build_sketch2d_from_sub_calls(
                &sketch_data.sub_calls,
            );
            if matches!(s, ::fielddraft_protocol::Sketch2d::Empty) {
                None
            } else {
                Some(s)
            }
        })()
        .ok_or(FeatureError::MissingParameter {
            feature: SweepDef::TYPE_NAME,
            name: "profiles",
        })?;

        // Resolve the spine through the modular path-provider layer — each
        // source method dispatches to its provider; consumers downstream see
        // only the sampled `SweepPath`, never this source.
        let bad_path = |message: String| FeatureError::InvalidParameter {
            feature: SweepDef::TYPE_NAME,
            name: "path",
            message,
        };
        let sampled = match &d.path {
            PathSourceDef::Gizmo3d { nodes, closed } => {
                crate::path_provider::Gizmo3dPath {
                    nodes: nodes.clone(),
                    closed: *closed,
                    samples_per_span: 24,
                }
                .sample()
                .map_err(|_| bad_path("need at least two control points".to_string()))?
            }
            PathSourceDef::Sketch2d { sketch: path_q } => {
                let path_id = match crate::queries::parse_query(path_q.as_str()) {
                    Ok(crate::queries::QueryExpr::CreatedBy(s)) => s,
                    _ => {
                        return Err(bad_path(
                            "sketch path must be qCreatedBy(<sketch>)".to_string(),
                        ))
                    }
                };
                let sk = ctx.sketches.get(&path_id).ok_or(
                    FeatureError::MissingParameter {
                        feature: SweepDef::TYPE_NAME,
                        name: "path",
                    },
                )?;
                let bounded = crate::sketch2d::BoundedSketch2d::from_sketch(
                    &crate::sketch_features::build_sketch2d_from_sub_calls(&sk.sub_calls),
                );
                // A closed loop sweeps around itself; otherwise fall back to
                // the open drawn curve (a path from A to B).
                let closed_loop = crate::sketch2d::extract_contours(
                    &bounded,
                    crate::sketch2d::CONTOUR_RESOLUTION,
                )
                .into_iter()
                .next()
                .map(|c| c.outer)
                .filter(|o| o.len() >= 2);
                let (curve, closed) = match closed_loop {
                    Some(loop_curve) => (loop_curve, true),
                    None => {
                        let open = crate::sketch_features::open_path_curve_from_sub_calls(
                            &sk.sub_calls,
                        )
                        .ok_or_else(|| {
                            bad_path("sketch has no usable planar curve to sweep along".to_string())
                        })?;
                        (open, false)
                    }
                };
                crate::path_provider::Sketch2dPath {
                    curve,
                    origin: sk.frame.origin,
                    x_axis: sk.frame.x_axis,
                    y_axis: sk.frame.y_axis,
                    closed,
                }
                .sample()
                .map_err(|_| bad_path("degenerate sketch path".to_string()))?
            }
            PathSourceDef::EdgeChain { edges } => {
                let expr = crate::queries::parse_query(edges.as_str())
                    .map_err(|e| bad_path(format!("could not parse edge query: {e}")))?;
                let polylines: Vec<Vec<[f32; 3]>> = ctx.resolve_edge_polylines(&expr);
                if polylines.is_empty() {
                    return Err(bad_path("edge query resolved to no edges".to_string()));
                }
                crate::path_provider::EdgeChainPath { polylines }
                    .sample()
                    .map_err(|_| bad_path("edges do not form a usable chain".to_string()))?
            }
            PathSourceDef::OrthogonalSketches { sketch_a, sketch_b } => {
                let (curve_a, _, fa) = resolve_sketch_curve_2d(&ctx.sketches, sketch_a)?;
                let (curve_b, _, fb) = resolve_sketch_curve_2d(&ctx.sketches, sketch_b)?;
                // The two sketch planes must share an axis (their x-axes are the
                // common parameter direction of the two-view construction).
                let dot = fa.x_axis[0] * fb.x_axis[0]
                    + fa.x_axis[1] * fb.x_axis[1]
                    + fa.x_axis[2] * fb.x_axis[2];
                if dot.abs() < 0.999 {
                    return Err(bad_path(
                        "the two sketches must share an axis (orthogonal views)".to_string(),
                    ));
                }
                crate::path_provider::OrthogonalSketchesPath {
                    curve_a,
                    curve_b,
                    origin: fa.origin,
                    shared_axis: fa.x_axis,
                    dir_a: fa.y_axis,
                    dir_b: fb.y_axis,
                    samples: 96,
                }
                .sample()
                .map_err(|_| bad_path("orthogonal sketches do not form a curve".to_string()))?
            }
        };

        let name = optional_display_name(def, || format!("Sweep {id}"));
        let node_id = id.child("0");
        let target_ids = resolve_merge_scope(
            ctx,
            target_ids_from_queries(&d.targets),
            d.merge_with_all,
        );
        if let Some(w) = ctx.merge_scope_warning(d.operation, &target_ids) {
            ctx.warn_feature(id, w);
        }
        ctx.features.push(DecodedFeature {
            id: node_id.clone(),
            name,
            op: d.operation,
            blend_radius: 0.0,
            blend_mode: crate::csg::BlendMode::Fillet,
            target_ids,
            transform: Transform {
                translate: [0.0, 0.0, 0.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0, 1.0, 1.0],
            },
            node: CsgNode::Sweep {
                id: node_id,
                sketch: Box::new(sketch),
                path: sampled.points,
                closed: sampled.closed,
                twist: d.twist_deg.to_radians(),
            },
        });
        Ok(())
    }
}

// ─── K9.6: Mate Connector ────────────────────────────────────────
//
// An attachable local coordinate system on the model surface.
// Stores an origin point + a normal (Z-axis of the CSYS).
// Doesn't produce geometry — it's a datum feature visible
// in the viewport as a small axis triad. Used by assemblies
// (P1) to define attachment points.

/// `mate_connector` — local coordinate system on the model.
///
/// C2: the connector is anchored to a B-Rep element (face, edge, or
/// vertex), resolved mid-loop against the live geometry rather than
/// carrying a baked world origin/normal. The CSYS is derived from queries
/// only, so it follows the geometry parametrically across edits:
///   * **origin** — the anchor's point: a face centroid, a point at
///     arc-length `t` along an edge, or the vertex itself.
///   * **+Z (primary)** — the outward normal of the `primary` reference
///     face. Empty `primary` means "use the anchor face's own normal"
///     (only valid for a face anchor). For edge/vertex anchors `primary`
///     selects which adjacent face orients the CSYS — Onshape's "primary
///     axis entity"; cycling the orientation = a different adjacent face.
///   * **+X (secondary)** — the edge tangent (projected ⟂ Z) for an edge
///     anchor, else the seed-cross in-plane basis (matching
///     [`crate::sketch_features::face_plane_frame`]).
#[derive(FeatureSchema)]
#[schema(type = "mate_connector")]
pub struct MateConnectorDef {
    /// What the connector anchors to: a B-Rep face, edge, or vertex
    /// (`qFace`/`qEdge`/`qVertex`). The anchor also defines the primary (+Z)
    /// axis: an EDGE's direction, a FACE's normal. A VERTEX has none, so +Z
    /// comes from the secondary reference face.
    #[param(entity_filter = "FACE, EDGE, VERTEX")]
    pub anchor: ::fielddraft_protocol::Query,
    /// Position along an edge anchor, `[0, 1]` (0.5 = midpoint). Ignored for
    /// face/vertex anchors.
    #[param(name = "Position along edge", default = 0.5)]
    pub t: f32,
    /// Between-entities placement (Phase D): an optional SECOND anchor
    /// (`qFace`/`qEdge`/`qVertex`). When set, the connector origin is the
    /// MIDPOINT of the two anchors' points; the orientation still comes from the
    /// first anchor. Empty → single-entity placement (the common case).
    #[param(name = "Between (2nd anchor)", default = ::fielddraft_protocol::Query::new(""), entity_filter = "FACE, EDGE, VERTEX")]
    pub anchor2: ::fielddraft_protocol::Query,
    /// Position along the 2nd anchor if it is an edge, `[0, 1]` (0.5 = midpoint).
    #[param(name = "2nd position along edge", default = 0.5)]
    pub t2: f32,
    /// Reference FACE that orients the SECONDARY axes (an edge's adjacent face)
    /// or, for a VERTEX anchor, defines +Z. Empty → auto-picked from the
    /// anchor's adjacent geometry. Unused for a face anchor.
    #[param(name = "Secondary reference", default = ::fielddraft_protocol::Query::new(""), entity_filter = "FACE")]
    pub secondary: ::fielddraft_protocol::Query,
    /// Reorient the secondary axes in 90° steps about +Z (0..3 cycles).
    #[param(name = "Reorient secondary", default = 0.0, ui_hint = "cycle90")]
    pub reorient: f32,
    /// Flip the primary (+Z) axis 180°.
    #[param(default = false)]
    pub flip: bool,
    /// Realignment (Phase D): translate the origin along the connector's OWN
    /// axes (X, Y, Z = normal), in millimetres. Applied after the CSYS is
    /// derived, so `[0, 0, 5]` lifts the connector 5 mm along its own +Z.
    #[param(name = "Offset (own axes)", default = [0.0, 0.0, 0.0])]
    pub offset: [f32; 3],
    /// Owner part (Phase D): the body this connector belongs to
    /// (`qCreatedBy(<seed>)`). Organises the connector under a part in the UI
    /// and travels with it. Empty → inferred from the anchor geometry.
    #[param(name = "Owner part", default = ::fielddraft_protocol::Query::new(""), entity_filter = "BODY")]
    pub owner: ::fielddraft_protocol::Query,
}

/// C2: the resolved coordinate system of a placed `mate_connector`,
/// captured during regeneration so the viewport can render its triad
/// without re-resolving the face query on the JS side.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ResolvedMateConnector {
    pub id: FeatureId,
    pub origin: [f32; 3],
    /// CSYS Z-axis (the face normal).
    pub normal: [f32; 3],
    /// CSYS X-axis (the face's in-plane basis vector).
    pub x_axis: [f32; 3],
    /// Owner part's seed feature id, if an explicit `owner` query was given
    /// (Phase D). `None` → not assigned to a specific part.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_id: Option<u32>,
}

pub struct MateConnectorFeature;

impl FeatureKind for MateConnectorFeature {
    fn type_name(&self) -> &'static str {
        MateConnectorDef::TYPE_NAME
    }
    fn schema_json(&self) -> Value {
        MateConnectorDef::schema_json()
    }
    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        // Mate connectors don't produce spine geometry — they're datum
        // features. Resolve the anchor + orientation queries against the live
        // geometry, derive the CSYS, and stash it for the viewport.
        let d = MateConnectorDef::parse(def)?;
        let inval = |name: &'static str, message: String| FeatureError::InvalidParameter {
            feature: MateConnectorDef::TYPE_NAME,
            name,
            message,
        };
        let anchor = crate::queries::parse_query(d.anchor.as_str())
            .map_err(|e| inval("anchor", format!("could not parse anchor query: {e}")))?;

        let anc = ctx
            .resolve_mate_anchor(&anchor, d.t)
            .ok_or_else(|| inval("anchor", "anchor query resolved to nothing".into()))?;

        // The secondary reference face normal: explicit query, else the
        // anchor's auto-picked adjacent face. An EDGE orients its secondary
        // axes by it; a VERTEX derives +Z from it.
        let secondary_str = d.secondary.as_str();
        let sec_normal = if secondary_str.is_empty() {
            anc.auto_secondary
        } else {
            let p = crate::queries::parse_query(secondary_str)
                .map_err(|e| inval("secondary", format!("could not parse secondary query: {e}")))?;
            ctx.resolve_face_frame(&p).map(|f| f.normal)
        };

        // +Z: the anchor's intrinsic axis (edge tangent / face normal), else
        // the secondary face normal (a vertex has no intrinsic axis). No
        // anchor type errors for a missing reference any more (ticket 5j9l).
        let mut normal = anc.primary.or(sec_normal).ok_or_else(|| {
            inval("secondary", "vertex anchor needs an adjacent or reference face".into())
        })?;
        if d.flip {
            normal = [-normal[0], -normal[1], -normal[2]];
        }

        // Base +X (before reorient): for an anchor WITH an intrinsic +Z
        // (edge/face) the secondary face normal projected into the plane orients
        // it; otherwise (vertex, whose +Z already came from that face) the
        // seed-cross basis. Falls back to seed-cross when no reference exists.
        let x0 = if anc.primary.is_some() {
            match sec_normal {
                Some(n) => project_perp(n, normal),
                None => seed_cross_x(normal),
            }
        } else {
            seed_cross_x(normal)
        };
        let steps = (d.reorient.round() as i64).rem_euclid(4) as u32;
        let x_axis = reorient_secondary(x0, normal, steps);

        // Between-entities (Phase D): a 2nd anchor moves the origin to the
        // midpoint of the two anchor points. Orientation stays with the first.
        let mut origin = anc.origin;
        let anchor2_str = d.anchor2.as_str();
        if !anchor2_str.is_empty() {
            let a2 = crate::queries::parse_query(anchor2_str)
                .map_err(|e| inval("anchor2", format!("could not parse 2nd anchor query: {e}")))?;
            let anc2 = ctx
                .resolve_mate_anchor(&a2, d.t2)
                .ok_or_else(|| inval("anchor2", "2nd anchor query resolved to nothing".into()))?;
            origin = [
                (origin[0] + anc2.origin[0]) * 0.5,
                (origin[1] + anc2.origin[1]) * 0.5,
                (origin[2] + anc2.origin[2]) * 0.5,
            ];
        }

        // Realignment offset (Phase D): translate along the connector's OWN
        // axes. Y = Z × X keeps the frame right-handed.
        if d.offset != [0.0, 0.0, 0.0] {
            let y_axis = cross3(normal, x_axis);
            for k in 0..3 {
                origin[k] +=
                    d.offset[0] * x_axis[k] + d.offset[1] * y_axis[k] + d.offset[2] * normal[k];
            }
        }

        // Owner part (Phase D): a `qCreatedBy(<seed>)` names the body this
        // connector belongs to. Capture the seed id for UI grouping.
        let owner_id = {
            let owner_str = d.owner.as_str();
            if owner_str.is_empty() {
                None
            } else {
                match crate::queries::parse_query(owner_str) {
                    Ok(crate::queries::QueryExpr::CreatedBy(seed)) => seed.as_u32_or_root(),
                    _ => None,
                }
            }
        };

        ctx.mate_connectors.push(ResolvedMateConnector {
            id: id.clone(),
            origin,
            normal,
            x_axis,
            owner_id,
        });
        Ok(())
    }
}

// ─── Translate / Rotate / Scale Features ───────────────────────
//
// Three separate gizmo-driven features that each modify one
// component of a target feature's transform. Consecutive gizmo
// drags of the same type on the same target are merged by the
// kernel's `apply_gizmo_*` methods.

fn read_vec3(params: &Value, key: &str, default: [f32; 3]) -> [f32; 3] {
    if let Some(arr) = params.get(key).and_then(|v| v.as_array()) {
        let c = |i: usize| arr.get(i).and_then(|v| v.as_f64()).unwrap_or(default[i] as f64) as f32;
        [c(0), c(1), c(2)]
    } else {
        default
    }
}

fn read_target_id(params: &Value) -> u32 {
    params.get("target_id").and_then(|v| v.as_u64()).unwrap_or(0) as u32
}

fn find_target(
    ctx: &mut Context<'_>,
    target_id: u32,
    feature: &'static str,
) -> Result<usize, FeatureError> {
    let target = FeatureId::from_u32(target_id);
    ctx.features.iter().position(|f| f.id == target)
        .ok_or_else(|| FeatureError::InvalidParameter {
            feature,
            name: "target_id",
            message: format!("target feature {} not found", target_id),
        })
}

/// Compose two transforms: `outer ∘ inner`, i.e. the placement that maps a
/// point through `inner` then `outer` (`outer.apply(inner.apply(p))`). Used to
/// lift a B-Rep face's body-local surface transform into world space via the
/// body `placement`. Assumes rigid (or uniformly-scaled) placements — the
/// shell gate's curved-surface SDF presumes a unit metric, so a non-uniform
/// body scale would distort the opening taper (the common case is rigid).
pub(crate) fn compose_placement(outer: &Transform, inner: &Transform) -> Transform {
    let scaled = [
        inner.translate[0] * outer.scale[0],
        inner.translate[1] * outer.scale[1],
        inner.translate[2] * outer.scale[2],
    ];
    let rotated = crate::quat_rotate(outer.rotation, scaled);
    Transform {
        translate: [
            rotated[0] + outer.translate[0],
            rotated[1] + outer.translate[1],
            rotated[2] + outer.translate[2],
        ],
        rotation: crate::quat_multiply(outer.rotation, inner.rotation),
        scale: [
            outer.scale[0] * inner.scale[0],
            outer.scale[1] * inner.scale[1],
            outer.scale[2] * inner.scale[2],
        ],
    }
}

pub(crate) fn euler_xyz_to_quat(rx_deg: f32, ry_deg: f32, rz_deg: f32) -> [f32; 4] {
    let (s1, c1) = (rx_deg.to_radians() * 0.5).sin_cos();
    let (s2, c2) = (ry_deg.to_radians() * 0.5).sin_cos();
    let (s3, c3) = (rz_deg.to_radians() * 0.5).sin_cos();
    [
        s1 * c2 * c3 + c1 * s2 * s3,
        c1 * s2 * c3 - s1 * c2 * s3,
        c1 * c2 * s3 + s1 * s2 * c3,
        c1 * c2 * c3 - s1 * s2 * s3,
    ]
}

pub(crate) fn quat_to_euler_xyz(qx: f32, qy: f32, qz: f32, qw: f32) -> [f32; 3] {
    let m11 = 1.0 - 2.0 * (qy * qy + qz * qz);
    let m12 = 2.0 * (qx * qy - qw * qz);
    let m13 = 2.0 * (qx * qz + qw * qy);
    let m23 = 2.0 * (qy * qz - qw * qx);
    let m33 = 1.0 - 2.0 * (qx * qx + qy * qy);

    let ry = m13.clamp(-1.0, 1.0).asin();
    let (rx, rz) = if m13.abs() < 0.9999999 {
        ((-m23).atan2(m33), (-m12).atan2(m11))
    } else {
        let m32 = 2.0 * (qy * qz + qw * qx);
        let m22 = 1.0 - 2.0 * (qx * qx + qz * qz);
        (m32.atan2(m22), 0.0)
    };
    [rx.to_degrees(), ry.to_degrees(), rz.to_degrees()]
}

// ── Translate ──────────────────────────────────────────────────

pub struct TranslateFeature;

impl FeatureKind for TranslateFeature {
    fn type_name(&self) -> &'static str { "translate" }
    fn schema_json(&self) -> Value {
        serde_json::json!({
            "type": "translate",
            "params": [
                {"key": "translate", "name": "Translation", "type": "vec3", "required": false, "unit": "mm"}
            ]
        })
    }
    fn execute(
        &self,
        ctx: &mut Context<'_>,
        _id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let idx = find_target(ctx, read_target_id(def), "translate")?;
        ctx.features[idx].transform.translate =
            read_vec3(def, "translate", [0.0, 0.0, 0.0]);
        Ok(())
    }
}

// ── Rotate ─────────────────────────────────────────────────────

pub struct RotateFeature;

impl FeatureKind for RotateFeature {
    fn type_name(&self) -> &'static str { "rotate" }
    fn schema_json(&self) -> Value {
        serde_json::json!({
            "type": "rotate",
            "params": [
                {"key": "rotation", "name": "Rotation", "type": "vec3", "required": false, "unit": "\u{00b0}"}
            ]
        })
    }
    fn execute(
        &self,
        ctx: &mut Context<'_>,
        _id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let euler = read_vec3(def, "rotation", [0.0, 0.0, 0.0]);
        let idx = find_target(ctx, read_target_id(def), "rotate")?;
        ctx.features[idx].transform.rotation =
            euler_xyz_to_quat(euler[0], euler[1], euler[2]);
        Ok(())
    }
}

// ── Scale ──────────────────────────────────────────────────────

pub struct ScaleFeature;

impl FeatureKind for ScaleFeature {
    fn type_name(&self) -> &'static str { "scale" }
    fn schema_json(&self) -> Value {
        serde_json::json!({
            "type": "scale",
            "params": [
                {"key": "scale", "name": "Scale", "type": "vec3", "required": false,
                 "default": [1.0, 1.0, 1.0]}
            ]
        })
    }
    fn execute(
        &self,
        ctx: &mut Context<'_>,
        _id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let idx = find_target(ctx, read_target_id(def), "scale")?;
        ctx.features[idx].transform.scale =
            read_vec3(def, "scale", [1.0, 1.0, 1.0]);
        Ok(())
    }
}

// ─── K10.4: Fillet Edge Feature ──────────────────────────────────
//
// A feature that applies per-edge spatial-mask fillets to the
// CSG tree. The edge fillets are stored in the document and
// applied as a post-processing pass after from_features builds
// the base tree. The feature itself doesn't push a spine entry —
// it modifies the tree shape by replacing Union/Difference
// nodes with MaskedBlend where the edge specification matches.

/// DL-61: the mask tube radius as a multiple of the blend radius. The
/// tube must enclose the full ~`blend_radius` perpendicular reach of the
/// smooth blend without bleeding into neighbouring edges. Tunable.
pub const FILLET_MASK_SCALE: f32 = 1.5;

/// `fillet_edge` — per-edge fillet selected by an edge query. The edge's
/// branch-pair signature yields the feature pair at the junction; its
/// polyline becomes the SMin mask tube.
#[derive(FeatureSchema)]
#[schema(type = "fillet_edge")]
pub struct FilletEdgeDef {
    /// The edge to fillet (e.g. `qEdge(<id_hash>)`).
    #[param(name = "Edge", entity_filter = "EDGE")]
    pub edge: ::fielddraft_protocol::Query,
    /// Fillet blend radius.
    #[param(default = 5.0)]
    pub blend_radius: f32,
}

pub struct FilletEdgeFeature;

impl FeatureKind for FilletEdgeFeature {
    fn type_name(&self) -> &'static str {
        FilletEdgeDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        FilletEdgeDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        _id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = FilletEdgeDef::parse(def)?;
        if d.blend_radius < 0.0 {
            return Err(FeatureError::InvalidParameter {
                feature: FilletEdgeDef::TYPE_NAME,
                name: "blend_radius",
                message: format!("must be >= 0, got {}", d.blend_radius),
            });
        }
        let expr = crate::queries::parse_query(d.edge.as_str()).map_err(|e| {
            FeatureError::InvalidParameter {
                feature: FilletEdgeDef::TYPE_NAME,
                name: "edge",
                message: format!("could not parse edge query: {e}"),
            }
        })?;
        let resolved = ctx.resolve_fillet_edges(&expr);
        if resolved.is_empty() {
            return Err(FeatureError::InvalidParameter {
                feature: FilletEdgeDef::TYPE_NAME,
                name: "edge",
                message: "edge query resolved to no edge".to_string(),
            });
        }
        let mask_radius = d.blend_radius * FILLET_MASK_SCALE;
        for (feature_a, feature_b, mask_polyline) in resolved {
            ctx.edge_fillets.push(crate::csg::EdgeFillet {
                feature_a,
                feature_b,
                mask_polyline,
                mask_radius,
                blend_radius: d.blend_radius,
                blend_mode: crate::csg::BlendMode::Fillet,
            });
        }
        Ok(())
    }
}

// ─── Shell: hollow a body, opening it at selected faces ───
//
// The classic CAD shell-with-removed-faces. The user picks one or more
// B-Rep faces (planar or curved — any analytic surface); the body they
// belong to is hollowed to a wall of `thickness`, broken open flush with
// each picked face. It lowers to the sentinel node `Shell(Empty, t,
// open_faces)` on a `Difference` feature, which `body::partition` docks into
// the target body and `CsgNode::from_decoded` turns into an inward
// `CsgNode::Shell` wrapping that body's whole subtree (see `wrap_shell`).
// Each `OpenSurface` carries the face's analytic `HalfSpace` + world
// placement, so the gate opens flush with cylinders/spheres/cones too.
#[derive(FeatureSchema)]
#[schema(type = "shell")]
pub struct ShellDef {
    /// The planar face(s) to remove, opening the shell there
    /// (e.g. `qFace(<id_hash>)`, comma-separated for several).
    #[param(name = "Open faces", entity_filter = "FACE")]
    pub open_faces: ::fielddraft_protocol::MultiQuery,
    /// Wall thickness, grown inward from the surface.
    #[param(name = "Thickness", default = ::fielddraft_protocol::Length::mm(2.0))]
    pub thickness: ::fielddraft_protocol::Length,
}

pub struct ShellFeature;

impl FeatureKind for ShellFeature {
    fn type_name(&self) -> &'static str {
        ShellDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        ShellDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = ShellDef::parse(def)?;
        // No faces picked yet — the feature was just created and its dialog is
        // still being filled. Emit nothing rather than failing the regen; once
        // a face is picked the shell appears.
        if d.open_faces.queries().is_empty() {
            return Ok(());
        }
        let thickness = d.thickness.to_mm();
        if thickness <= 0.0 {
            return Err(FeatureError::InvalidParameter {
                feature: ShellDef::TYPE_NAME,
                name: "thickness",
                message: format!("must be > 0, got {thickness}"),
            });
        }

        // Resolve every picked face to a world-space open surface (planar or
        // curved), recording the feature each face belongs to so we can find the
        // body to hollow.
        let mut open_faces: Vec<crate::csg::OpenSurface> = Vec::new();
        let mut owner: Option<FeatureId> = None;
        for q in d.open_faces.queries() {
            let expr = crate::queries::parse_query(q).map_err(|e| {
                FeatureError::InvalidParameter {
                    feature: ShellDef::TYPE_NAME,
                    name: "open_faces",
                    message: format!("could not parse face query `{q}`: {e}"),
                }
            })?;
            match ctx.resolve_open_face(&expr) {
                Some((surface, face_owner)) => {
                    open_faces.push(surface);
                    owner.get_or_insert(face_owner);
                }
                None => {
                    return Err(FeatureError::InvalidParameter {
                        feature: ShellDef::TYPE_NAME,
                        name: "open_faces",
                        message: format!(
                            "face query `{q}` resolved to no analytic face (only \
                             tessellated/TPMS faces cannot be opened)"
                        ),
                    });
                }
            }
        }
        let Some(owner) = owner else {
            return Err(FeatureError::InvalidParameter {
                feature: ShellDef::TYPE_NAME,
                name: "open_faces",
                message: "shell needs at least one face to open".to_string(),
            });
        };

        // Gather every feature of the body that owns the first picked face — the
        // full target set so `wrap_shell` wraps the whole (possibly fused) body,
        // not just one feature.
        let parts = crate::body::partition(&ctx.features);
        let target_ids: Vec<FeatureId> = parts
            .iter()
            .find(|p| {
                p.feature_indices.iter().any(|&fi| {
                    ctx.features[fi].id.root_ancestor() == &owner
                })
            })
            .map(|p| {
                p.feature_indices
                    .iter()
                    .map(|&fi| ctx.features[fi].id.clone())
                    .collect()
            })
            .unwrap_or_else(|| vec![owner.clone()]);

        ctx.features.push(DecodedFeature {
            id: id.clone(),
            name: "Shell".to_string(),
            // `Difference` is purely a partition hint (dock into the target
            // body); `from_decoded` intercepts the sentinel node before any
            // boolean fold.
            op: BoolOp::Remove,
            blend_radius: 0.0,
            blend_mode: crate::csg::BlendMode::Fillet,
            target_ids,
            transform: Transform::IDENTITY,
            node: crate::csg::CsgNode::Shell(
                Box::new(crate::csg::CsgNode::Empty),
                thickness,
                open_faces,
            ),
        });
        Ok(())
    }
}

// ─── K13.1: Imported Mesh as CSG Solid ───────────────────────────
//
// Wraps a MeshSdf (triangle buffer + per-tri AABBs + ray-parity
// sign test) as a full CSG leaf. The mesh data serialises inline
// in the FeatureCall params so it survives save/load via
// localStorage. Performance is O(N) per sample — acceptable for
// imports up to ~50K triangles at DC default resolution.

/// `imported_mesh` — an imported triangle mesh that
/// participates in CSG Booleans.
#[derive(FeatureSchema)]
#[schema(type = "imported_mesh")]
pub struct ImportedMeshDef {
    #[param(default = [0.0, 0.0, 0.0])]
    pub translate: [f32; 3],
    #[param(name = "Scale", default = 1.0)]
    pub uniform_scale: f32,
    #[param(name = "Z-up", default = false)]
    pub swap_yz: bool,
    #[param(default = ::fielddraft_protocol::BoolOp::New)]
    pub op: BoolOp,
    /// Target bodies for the boolean op (Add/Remove/Intersect), as
    /// `qCreatedBy(<seed_feature>)` queries. Empty for New; required otherwise.
    #[param(name = "Targets", default = ::fielddraft_protocol::MultiQuery::new(""), entity_filter = "BODY")]
    pub targets: ::fielddraft_protocol::MultiQuery,
}

pub struct ImportedMeshFeature;

impl FeatureKind for ImportedMeshFeature {
    fn type_name(&self) -> &'static str {
        ImportedMeshDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        ImportedMeshDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = ImportedMeshDef::parse(def)?;

        // Compact format (v2): positions + indices stored separately.
        // Legacy format (v1): full MeshSdf under "__mesh".
        let mesh: fielddraft_protocol::MeshSdf = if let Some(pos_val) = def.get("__positions") {
            let positions: Vec<f32> = serde_json::from_value(pos_val.clone()).map_err(|e| {
                FeatureError::InvalidParameter {
                    feature: ImportedMeshDef::TYPE_NAME,
                    name: "__positions",
                    message: e.to_string(),
                }
            })?;
            let idx_val = def.get("__indices").ok_or(FeatureError::MissingParameter {
                feature: ImportedMeshDef::TYPE_NAME,
                name: "__indices",
            })?;
            let indices: Vec<u32> = serde_json::from_value(idx_val.clone()).map_err(|e| {
                FeatureError::InvalidParameter {
                    feature: ImportedMeshDef::TYPE_NAME,
                    name: "__indices",
                    message: e.to_string(),
                }
            })?;
            fielddraft_protocol::MeshSdf::from_buffers(&positions, &indices)
        } else {
            let mesh_val = def.get("__mesh").ok_or(FeatureError::MissingParameter {
                feature: ImportedMeshDef::TYPE_NAME,
                name: "__mesh",
            })?;
            serde_json::from_value(mesh_val.clone()).map_err(|e| {
                FeatureError::InvalidParameter {
                    feature: ImportedMeshDef::TYPE_NAME,
                    name: "__mesh",
                    message: e.to_string(),
                }
            })?
        };

        let name = optional_display_name(def, || format!("Imported mesh {id}"));
        let base_rotation = optional_rotation(def);
        let s = if d.uniform_scale > 0.0 { d.uniform_scale } else { 1.0 };
        let scale = [s, s, s];

        let rotation = if d.swap_yz {
            // -90° around X to convert Z-up → Y-up
            let swap: [f32; 4] = [-std::f32::consts::FRAC_1_SQRT_2, 0.0, 0.0, std::f32::consts::FRAC_1_SQRT_2];
            crate::quat_multiply(swap, base_rotation)
        } else {
            base_rotation
        };

        let target_ids = target_ids_from_queries(&d.targets);
        if let Some(w) = ctx.merge_scope_warning(d.op, &target_ids) {
            ctx.warn_feature(id, w);
        }

        ctx.features.push(DecodedFeature::from_parts(
            id.clone(),
            name,
            d.op,
            Primitive::ImportedMesh {
                mesh: Box::new(mesh),
            },
            Transform {
                translate: d.translate,
                rotation,
                scale,
            },
            PrimitiveModifiers::NONE,
            target_ids,
        ));
        Ok(())
    }
}

// ─── K11.0: Gyroid TPMS Infill ───────────────────────────────────
//
// The first T2 "SDF superpower". A gyroid is an infinite
// triply-periodic minimal surface described purely by the
// formula `sin(x)cos(y) + sin(y)cos(z) + sin(z)cos(x)`.
// Thickening via `|f| - thickness` produces a solid lattice.
// Intersecting the lattice with a host body clips it to a
// finite region — the standard workflow is "add box, add
// gyroid with op=Intersect".
//
// The spine entry uses a hierarchical child id (same pattern
// as HoleFeature) to avoid a stray duplicate call from
// sync_feature_calls.

/// `gyroid` — TPMS lattice infill.
#[derive(FeatureSchema)]
#[schema(type = "gyroid")]
pub struct GyroidDef {
    #[param(name = "Bodies", default = ::fielddraft_protocol::MultiQuery::new(""), entity_filter = "BODY")]
    pub targets: ::fielddraft_protocol::MultiQuery,
    /// Distance between repeating cells in mm.
    pub cell_size: ::fielddraft_protocol::Length,
    /// Wall half-thickness in mm. Larger = thicker walls.
    pub thickness: ::fielddraft_protocol::Length,
    #[param(default = [0.0, 0.0, 0.0])]
    pub translate: [f32; 3],
    #[param(default = ::fielddraft_protocol::BoolOp::Intersect)]
    pub op: BoolOp,
    #[param(name = "Blend radius", default = ::fielddraft_protocol::Length::mm(0.0))]
    pub blend_radius: ::fielddraft_protocol::Length,
    #[param(name = "Chamfer size", default = ::fielddraft_protocol::Length::mm(0.0))]
    pub chamfer_size: ::fielddraft_protocol::Length,
    #[param(name = "Twist rate", default = 0.0)]
    pub twist_rate: f32,
    #[param(name = "Bend rate", default = 0.0)]
    pub bend_rate: f32,
    #[param(name = "Displace depth", default = ::fielddraft_protocol::Length::mm(0.0))]
    pub displace_depth: ::fielddraft_protocol::Length,
    #[param(name = "Displace freq", default = 0.0)]
    pub displace_freq: f32,
    #[param(name = "Shell thickness", default = ::fielddraft_protocol::Length::mm(0.0))]
    pub shell_thickness: ::fielddraft_protocol::Length,
}

pub struct GyroidFeature;

impl FeatureKind for GyroidFeature {
    fn type_name(&self) -> &'static str {
        GyroidDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        let mut schema = GyroidDef::schema_json();
        if let Some(params) = schema.get_mut("params").and_then(|p| p.as_array_mut()) {
            for param in params.iter_mut() {
                if param.get("key").and_then(|k| k.as_str()) == Some("op") {
                    if let Some(variants) =
                        param.get_mut("variants").and_then(|v| v.as_array_mut())
                    {
                        variants.retain(|v| {
                            !matches!(
                                v.get("key").and_then(|k| k.as_str()),
                                Some("add") | Some("new")
                            )
                        });
                    }
                }
            }
        }
        schema
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = GyroidDef::parse(def)?;
        let cell_mm = d.cell_size.to_mm();
        let thick_mm = d.thickness.to_mm();
        positive_f32(cell_mm, GyroidDef::TYPE_NAME, "cell_size")?;
        positive_f32(thick_mm, GyroidDef::TYPE_NAME, "thickness")?;

        let op = match d.op {
            op if op.is_union() => BoolOp::Intersect,
            other => other,
        };

        let name = optional_display_name(def, || format!("Gyroid {id}"));
        let rotation = optional_rotation(def);
        let scale = optional_scale(def);

        let target_ids = target_ids_from_queries(&d.targets);

        ctx.features.push(DecodedFeature::from_parts(
            id.child("0"),
            name,
            op,
            Primitive::Gyroid {
                cell_size: cell_mm,
                thickness: thick_mm,
            },
            Transform {
                translate: d.translate,
                rotation,
                scale,
            },
            PrimitiveModifiers {
                blend_radius: d.blend_radius.to_mm(),
                chamfer_size: d.chamfer_size.to_mm(),
                twist_rate: d.twist_rate,
                bend_rate: d.bend_rate,
                displace_depth: d.displace_depth.to_mm(),
                displace_freq: d.displace_freq,
                shell_thickness: d.shell_thickness.to_mm(),
            },
            target_ids,
        ));
        Ok(())
    }
}

// ─── K8.9: Hole ──────────────────────────────────────────────────
//
// HoleFeature is a parametric subtract-cylinder. It carries the
// same geometry as a `cylinder` call with op=difference, but the
// dialog-facing schema uses `Length`-typed `diameter` and `depth`
// fields (Onshape parity) so users don't have to mentally halve
// the diameter into a radius or double the depth into a
// half-height. The execute path lowers to Primitive::Cylinder
// with half_extent math done at the model-unit boundary, which
// keeps the mesher and BVH completely ignorant of the dedicated
// Hole kind.
//
// The `op` field is intentionally absent: a hole that adds
// material isn't a hole. If the user wants to turn it into a
// union or intersect they can delete it and add a regular
// cylinder feature instead.

/// `hole` — parametric cylinder subtract with diameter + depth.
#[derive(FeatureSchema)]
#[schema(type = "hole")]
pub struct HoleDef {
    /// Diameter at the mouth of the hole.
    pub diameter: ::fielddraft_protocol::Length,
    /// Total depth along the local Y axis.
    pub depth: ::fielddraft_protocol::Length,
    /// World-space position of the hole's centre.
    #[param(default = [0.0, 0.0, 0.0])]
    pub translate: [f32; 3],

    /// Merge scope: the body/bodies the hole drills, as
    /// `qCreatedBy(<seed_feature>)` queries. MANDATORY — a hole with no scope
    /// is flagged with a warning and drills nothing (Onshape semantics: scope
    /// is always explicit).
    #[param(name = "Scope", default = ::fielddraft_protocol::MultiQuery::new(""), entity_filter = "BODY")]
    pub targets: ::fielddraft_protocol::MultiQuery,
}

pub struct HoleFeature;

impl FeatureKind for HoleFeature {
    fn type_name(&self) -> &'static str {
        HoleDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        HoleDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = HoleDef::parse(def)?;
        let diameter_mm = d.diameter.to_mm();
        let depth_mm = d.depth.to_mm();
        positive_f32(diameter_mm, HoleDef::TYPE_NAME, "diameter")?;
        positive_f32(depth_mm, HoleDef::TYPE_NAME, "depth")?;

        let name = optional_display_name(def, || format!("Hole {id}"));
        let rotation = optional_rotation(def);
        let scale = optional_scale(def);
        // Scope is mandatory: a hole drills the bodies named in `targets`.
        // With none chosen it warns and drills nothing (partition seeds it as
        // a standalone red preview).
        let target_ids = target_ids_from_queries(&d.targets);
        if let Some(w) = ctx.merge_scope_warning(BoolOp::Remove, &target_ids) {
            ctx.warn_feature(id, w);
        }
        // Spine entry uses a hierarchical child id (`<hole>/0`)
        // so `Document::sync_feature_calls` drops it on the
        // next sync — the authoritative wire form is the
        // original `hole` FeatureCall, and we don't want a
        // stray `cylinder` FeatureCall to be synthesised from
        // the derived primitive. Same pattern the K8.0 pattern
        // features use for their spine clones.
        ctx.features.push(DecodedFeature::from_parts(
            id.child("0"),
            name,
            BoolOp::Remove,
            Primitive::Cylinder {
                center: [0.0, 0.0, 0.0],
                radius: diameter_mm / 2.0,
                half_height: depth_mm / 2.0,
            },
            Transform {
                translate: d.translate,
                rotation,
                scale,
            },
            PrimitiveModifiers::NONE,
            target_ids,
        ));
        Ok(())
    }
}

// ─── ExtrudeDef (K4.5.8: Rust-native reference implementation) ──
//
// ExtrudeDef is the first K4.5 reference feature that exercises
// the full parameter vocabulary the K4.5.4f derive learned —
// `Query`, `Length`, `BoolOp`, `f32` and `[f32; 3]` all appear in
// one definition struct, so the §4.6 promise that "the derive
// covers the kernel runtime types" is demonstrated end-to-end
// rather than in unit-test fixtures only.
//
// ## Execute contract
//
// The protocol's `Primitive` enum doesn't carry an `Extrude`
// variant yet — adding one would ripple through the mesher, the
// DC pass, the BVH and the wire form, which is well out of
// scope for one slice. Instead, K4.5.8's executor **lowers** an
// ExtrudeDef into a plain `Primitive::Box`: the rectangular
// profile (half-width / half-height in X and Y) supplies the
// first two half-extents, and the extrude depth converted to
// millimetres via `Length::to_mm` supplies the Z half-extent
// (depth / 2, so the solid is centred on the extrude plane).
// The result is a real mesh-able feature that K3's dual-
// contouring pipeline handles exactly like any hand-added box
// — the difference is that the spine entry was produced by a
// FeatureCall routed through the registry, not by a direct
// `Kernel::add_primitive` call.
//
// ## What's intentionally simplified vs §4.6
//
// * **`profiles: Query` is accepted but not resolved.** K4.5.7
//   ships the query parser and a version cache, but there's no
//   sketch-producing feature upstream whose profile a query
//   could point at yet. The executor stores the query string
//   for round-trip fidelity and moves on. A later slice will
//   resolve the query into a real `Sketch2d` once sketch
//   features exist, at which point the `profile_half_x` /
//   `profile_half_y` fields can be removed in favour of the
//   sketch's own extent.
// * **Only the Z extrude direction is supported.** The §4.6
//   example includes `second_direction: bool` and
//   `depth2: Option<Length>`; `Option<T>` isn't a derive-
//   supported field type yet, and the two-sided extrude has no
//   semantic value while the mesh-level representation stays a
//   simple centred box. Both fields are deferred.
// * **Operation is `BoolOp`, not the full §4.6 "new / add /
//   remove / intersect" operation vocabulary.** The extra
//   "new" case (create a separate body) needs body-id
//   plumbing that's not yet wired through the registry path.

/// `extrude` — Rust-native feature that consumes a sketch
/// profile and lowers it to a 3D body.
///
/// K7.1 turns the `profiles` query into a real resolution
/// path: the executor parses the query, finds the referenced
/// sketch in [`Context::sketches`], synthesises a
/// [`crate::sketch2d::Sketch2d`] via
/// `build_sketch2d_from_sub_calls` and reads the profile
/// extents from the resulting AABB. The `profile_half_x` /
/// `profile_half_y` fields become optional fallbacks for
/// tests and for documents that point at a sketch the kernel
/// can't resolve yet.
///
/// Known shape-fidelity limitation: the current spine pipeline
/// has no `Primitive::Extrude` variant, so the lowering still
/// produces a `Primitive::Box` whose half-extents match the
/// sketch's bounding rectangle. A rectangle sketch is
/// pixel-perfect; a circle becomes its bounding box. Fixing
/// that is a later K7 slice which adds the Primitive variant
/// and teaches the mesher/BVH about it.
#[derive(FeatureSchema)]
#[schema(type = "extrude")]
pub struct ExtrudeDef {
    /// Sketch profile this extrude consumes. Evaluated against
    /// [`Context::sketches`] at regeneration time via
    /// [`crate::sketch_features::resolve_profile_query`]. Accepts the WHOLE
    /// sketch (`qCreatedBy(sketch_id)` / `qAllRegions`) or a single closed
    /// region of it (`qRegion`/`qRegions`) — ticket tgdg. The `SKETCHREGION`
    /// entity filter tells the viewport's profile picker to resolve a clicked
    /// filled region to a `qRegion` query instead of the whole sketch (it is
    /// the same picker path Revolve uses, ticket dhjn). Ticket 20fc: a planar
    /// B-Rep `qFace(...)` is accepted here too — picking a solid's side face as
    /// the "profile" routes through the face-extrude path, so the user need not
    /// know a separate field. Ticket k280: this is a `MultiQuery`, so several
    /// coplanar faces (or regions) accumulate in one pick session and extrude
    /// together — the single Onshape-style "faces and sketch regions to extrude"
    /// input. Every `qFace(...)` entry routes through the face-extrude path
    /// ([`Context::resolve_extrude_faces`], which unions their boundary loops);
    /// a sketch entry resolves via [`crate::sketch_features::resolve_profile_query`].
    #[param(entity_filter = "SKETCHREGION, FACE")]
    pub profiles: ::fielddraft_protocol::MultiQuery,

    /// Extrude depth along the local Z axis. Any supported
    /// length unit works; the executor converts to millimetres
    /// via [`Length::to_mm`] before lowering to a box.
    pub depth: ::fielddraft_protocol::Length,

    /// Fallback half-width for tests and for extrudes that
    /// don't yet point at a resolvable sketch. Ignored when
    /// the query successfully resolves to a non-empty sketch.
    #[param(default = 0.0)]
    pub profile_half_x: f32,

    /// Fallback half-height — same story as
    /// [`profile_half_x`].
    #[param(default = 0.0)]
    pub profile_half_y: f32,

    /// Boolean op used to fold the extruded body into the
    /// running feature spine. Defaults to [`BoolOp::New`]
    /// (ticket yx5h): a fresh extrude creates a separate body
    /// rather than merging into whatever it touches.
    #[param(default = ::fielddraft_protocol::BoolOp::New)]
    pub operation: BoolOp,

    /// Override for the extruded box origin in world space.
    /// When the sketch resolves, the X/Y components default to
    /// the sketch AABB centre; `center[2]` always supplies the
    /// extrude plane Z coordinate.
    #[param(default = [0.0, 0.0, 0.0])]
    pub center: [f32; 3],

    /// Wall thickness for hollowing the extruded body. `0` (the
    /// default) leaves it solid; any positive value lowers to a
    /// [`CsgNode::Shell`] (`|sdf| − thickness`) via
    /// [`PrimitiveModifiers`], producing a thin-walled closed shell.
    #[param(name = "Shell thickness", default = ::fielddraft_protocol::Length::mm(0.0))]
    pub shell_thickness: ::fielddraft_protocol::Length,

    /// Direction mode. `false` (default) is a one-sided (blind) extrude
    /// `[−depth_back, +depth]` along the plane normal — the Onshape default of
    /// growing outward from the profile plane (ticket yx5h). `true` grows the
    /// profile symmetrically by `±depth/2` about the plane.
    #[param(name = "Symmetric", default = false)]
    pub symmetric: bool,

    /// Backward depth for a two-sided (asymmetric) extrude, measured against the
    /// plane normal. Only consulted when `symmetric` is `false`; `0` (default)
    /// makes a pure blind extrude.
    #[param(name = "Back depth", default = ::fielddraft_protocol::Length::mm(0.0))]
    pub depth_back: ::fielddraft_protocol::Length,

    /// Merge scope: explicit target bodies for the boolean op, as
    /// `qCreatedBy(<seed_feature>)` queries. Empty falls back to face-owner
    /// docking (face extrudes); otherwise + `merge_with_all` off = no merge
    /// (standalone body; warned for Add/Remove/Intersect).
    #[param(name = "Merge scope", default = ::fielddraft_protocol::MultiQuery::new(""), entity_filter = "BODY")]
    pub targets: ::fielddraft_protocol::MultiQuery,

    /// Merge with all (Onshape): fold the body into EVERY body it touches
    /// instead of an explicit scope. Off by default — touching solids never
    /// merge implicitly; opt in here or name targets above.
    #[param(name = "Merge with all", default = false)]
    pub merge_with_all: bool,
}

pub struct ExtrudeFeature;

impl FeatureKind for ExtrudeFeature {
    fn type_name(&self) -> &'static str {
        ExtrudeDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        ExtrudeDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = ExtrudeDef::parse(def)?;

        // Length → mm at the model-unit boundary. The kernel's
        // native distance scale is millimetres, so every feature
        // that consumes a typed runtime length is responsible for
        // converting once, at the edge.
        let depth_mm = d.depth.to_mm();

        // Direction mode → (half-height, centre offset along the plane normal).
        // Symmetric (default) is ±|depth|/2 about the plane — the legacy sketch
        // behaviour (dz = 0). Blind / two-sided spans the sorted interval over
        // `{−back, +depth}` → a NEGATIVE depth extrudes in the −normal
        // direction (Onshape: the depth drag may cross the profile plane and
        // grow the body the other way, ticket x7af). Centre sits mid-span
        // along local Z (which the frame maps to the plane normal). The
        // `center[2]` shift is folded into the primitive below.
        let back_mm = d.depth_back.to_mm().max(0.0);
        let (half_depth_z, dz) = if d.symmetric {
            (depth_mm.abs() / 2.0, 0.0)
        } else {
            let lo = (-back_mm).min(depth_mm);
            let hi = (-back_mm).max(depth_mm);
            ((hi - lo) / 2.0, (hi + lo) / 2.0)
        };
        if half_depth_z <= 0.0 {
            return Err(FeatureError::InvalidParameter {
                feature: ExtrudeDef::TYPE_NAME,
                name: "depth",
                message: format!("extrude extent must be > 0 mm, got depth {depth_mm} / depth_back {back_mm}"),
            });
        }

        let name = optional_display_name(def, || format!("Extrude {id}"));
        let rotation = optional_rotation(def);
        let scale = optional_scale(def);
        let mods = PrimitiveModifiers {
            shell_thickness: d.shell_thickness.to_mm().max(0.0),
            ..PrimitiveModifiers::NONE
        };

        // ── Face-extrude path (P1) ──────────────────────────────────────────
        // `profiles` is the unified Onshape-style "faces and sketch regions"
        // input (ticket k280): every `qFace(...)` entry drives the face-extrude
        // path. Multiple coplanar faces accumulate in one pick session — their
        // boundary loops are projected into the shared plane, unioned, and swept
        // exactly like a sketch profile; the face frame places that profile back
        // onto the plane. Any face entry takes precedence over a sketch entry.
        let profile_qs: Vec<String> =
            d.profiles.queries().iter().map(|s| s.to_string()).collect();
        let face_queries: Vec<String> = profile_qs
            .iter()
            .filter(|q| {
                matches!(
                    crate::queries::parse_query(q),
                    Ok(crate::queries::QueryExpr::Face(_))
                )
            })
            .cloned()
            .collect();
        if !face_queries.is_empty() {
            let (sketch, frame) =
                ctx.resolve_extrude_faces(&face_queries).map_err(|message| {
                    FeatureError::InvalidParameter {
                        feature: ExtrudeDef::TYPE_NAME,
                        name: "profiles",
                        message,
                    }
                })?;
            let primitive = Primitive::Extrude {
                sketch,
                half_depth_z,
                center: [0.0, 0.0, dz],
            };
            let transform = crate::compose_transforms(
                &frame.to_transform(),
                &Transform { translate: [0.0, 0.0, 0.0], rotation, scale },
            );
            // Dock into the body that owns the picked face: an Add grows that
            // body (Onshape semantics), and the explicit target keeps the prism
            // from fusing with an unrelated AABB-overlapping body (ticket px2o).
            // An explicit merge scope (ticket 15f5) overrides the face owner.
            let mut target_ids = target_ids_from_queries(&d.targets);
            if target_ids.is_empty() {
                target_ids = ctx.face_owner_target_ids(&face_queries);
            }
            if target_ids.is_empty() && d.merge_with_all {
                target_ids = ctx.current_body_seed_ids();
            }
            if let Some(w) = ctx.merge_scope_warning(d.operation, &target_ids) {
                ctx.warn_feature(id, w);
            }
            ctx.features.push(DecodedFeature::from_parts(
                id.clone(),
                name,
                d.operation,
                primitive,
                transform,
                mods,
                target_ids,
            ));
            return Ok(());
        }

        // K7.3: resolve the `profiles` query against the
        // context's sketch table and lower to a real
        // `Primitive::Extrude` carrying the full Sketch2d
        // profile. The K4.5.8 fallback (Box stand-in) stays
        // for documents that don't yet point at a resolvable
        // sketch, so existing tests keep running unchanged.
        //
        // K7.22 follow-up: when the `profiles` query *does*
        // resolve to a sketch but the sketch has no closed
        // regions (e.g. the user drew lines/points but no
        // rectangle/circle), we used to fall through to the
        // legacy fallback and die on the `profile_half_x > 0`
        // guard with an unhelpful message. Distinguish the
        // three states explicitly: resolved-and-extrudable,
        // resolved-but-empty, no-resolve-use-fallback.
        let mut resolved_sketch: Option<::fielddraft_protocol::Sketch2d> = None;
        let mut sketch_frame = crate::sketch_features::SketchFrame::IDENTITY;
        let mut sketch_was_found_but_empty = false;
        // Ticket tgdg: resolve `qCreatedBy`/`qAllRegions`/`qRegion`/`qRegions`
        // through the same path Revolve uses (ticket dhjn) so a single picked
        // region extrudes just that region (holes preserved), not the whole
        // sketch. `resolve_profile_query` returns `None` both for an
        // unresolvable query (→ legacy half-extent fallback) and for a sketch
        // that resolved but has no closed regions; distinguish those so the
        // "draw a closed region first" error still fires.
        // No face entries → treat the first profile entry as the sketch source
        // (a single sketch query — whole sketch or a region group — is the
        // common case; multi-region is already one `qRegions(...)` expression).
        let profile_str: &str = profile_qs.first().map(String::as_str).unwrap_or("");
        match crate::sketch_features::resolve_profile_query(
            profile_str,
            &ctx.sketches,
        ) {
            Some((sketch2d, frame)) => {
                resolved_sketch = Some(sketch2d);
                sketch_frame = frame;
            }
            None => {
                // A `qCreatedBy(id)` that names a known sketch with no
                // extrudable region is the "found but empty" case.
                if let Ok(crate::queries::QueryExpr::CreatedBy(sketch_id))
                | Ok(crate::queries::QueryExpr::AllRegions(sketch_id)) =
                    crate::queries::parse_query(profile_str)
                {
                    if ctx.sketches.contains_key(&sketch_id) {
                        sketch_was_found_but_empty = true;
                    }
                }
            }
        }

        // K7.22 follow-up: emit a clear, user-facing error when
        // the caller *intended* to drive the extrude from a sketch
        // (profile_half_x/y both left at 0) but the sketch has no
        // extrudable regions. The legacy-API test path that passes
        // explicit non-zero half-extents still falls through to
        // the Box fallback below so existing callers keep working.
        if sketch_was_found_but_empty
            && d.profile_half_x == 0.0
            && d.profile_half_y == 0.0
        {
            return Err(FeatureError::InvalidParameter {
                feature: ExtrudeDef::TYPE_NAME,
                name: "profiles",
                message: "sketch has no closed regions to extrude — draw a rectangle, circle or polygon first".to_string(),
            });
        }

        // Ticket r2zx: the profile query named nothing extrudable (e.g. a solid
        // FACE or a whole-body `qCreatedBy(extrude_id)` was picked instead of a
        // sketch region) and no legacy half-extents were given. Fail with a
        // clear, actionable message instead of falling through to the box
        // fallback's cryptic "profile_half_x must be > 0".
        if resolved_sketch.is_none()
            && d.profile_half_x == 0.0
            && d.profile_half_y == 0.0
        {
            return Err(FeatureError::InvalidParameter {
                feature: ExtrudeDef::TYPE_NAME,
                name: "profiles",
                message: "select a closed sketch region or one or more coplanar planar faces to extrude".to_string(),
            });
        }

        // Direction mode folds into `center[2]` (along local Z / plane normal).
        let center = [d.center[0], d.center[1], d.center[2] + dz];
        let primitive = match resolved_sketch {
            Some(sketch) => Primitive::Extrude {
                sketch,
                half_depth_z,
                center,
            },
            None => {
                // Fallback: legacy hand-written half-extents.
                positive_f32(
                    d.profile_half_x,
                    ExtrudeDef::TYPE_NAME,
                    "profile_half_x",
                )?;
                positive_f32(
                    d.profile_half_y,
                    ExtrudeDef::TYPE_NAME,
                    "profile_half_y",
                )?;
                Primitive::Box {
                    center,
                    half_extents: [d.profile_half_x, d.profile_half_y, half_depth_z],
                }
            }
        };

        // C1: compose the sketch's frame→world placement (parent) with
        // the feature's own rotation/scale (child) so the profile — built
        // in local XY — sits on the real sketch plane. `origin_plane_xy`
        // and the legacy no-sketch path both yield the identity frame, so
        // existing extrudes are byte-for-byte unchanged.
        let transform = crate::compose_transforms(
            &sketch_frame.to_transform(),
            &Transform {
                translate: [0.0, 0.0, 0.0],
                rotation,
                scale,
            },
        );

        let target_ids = resolve_merge_scope(
            ctx,
            target_ids_from_queries(&d.targets),
            d.merge_with_all,
        );
        if let Some(w) = ctx.merge_scope_warning(d.operation, &target_ids) {
            ctx.warn_feature(id, w);
        }
        ctx.features.push(DecodedFeature::from_parts(
            id.clone(),
            name,
            d.operation,
            primitive,
            transform,
            mods,
            target_ids,
        ));
        Ok(())
    }
}

// ─── Thicken: offset a curved analytic face into a thin solid ─────
//
// The curved counterpart to the face-extrude path above. Where a planar face
// reduces to `Primitive::Extrude` (linear sweep of its 2D boundary), a curved
// analytic face reduces to the **difference of two coaxial offset solid
// primitives** — `Sphere(r+out) − Sphere(r−in)`, `Cyl(r+out) − Cyl(r−in)`,
// `Torus(R, r+out) − Torus(R, r−in)` — so the thickened wall inherits the whole
// solid pipeline (SDF, weave, picking) for free (see
// [`crate::brep::face_thicken`]). First throw: sphere / cylinder / torus.

/// `thicken` — make a thin solid wall from one or more **curved** analytic
/// B-Rep faces by offsetting their surfaces along the normal.
#[derive(FeatureSchema)]
#[schema(type = "thicken")]
pub struct ThickenDef {
    /// Curved B-Rep face(s) to thicken. Comma-separated `qFace(<id_hash>)`
    /// selections; each becomes its own offset-solid shell and they are unioned.
    #[param(name = "Faces", default = ::fielddraft_protocol::MultiQuery::new(""), entity_filter = "FACE")]
    pub faces: ::fielddraft_protocol::MultiQuery,

    /// Wall thickness grown along the face normal (outside the source solid).
    /// When `symmetric` is set this is split half in / half out instead.
    #[param(name = "Thickness", default = ::fielddraft_protocol::Length::mm(2.0))]
    pub thickness: ::fielddraft_protocol::Length,

    /// `true` splits the wall symmetrically about the surface (`±thickness/2`).
    /// `false` (default, Onshape-style) grows `thickness` outward and
    /// `inner_thickness` inward.
    #[param(name = "Symmetric", default = false)]
    pub symmetric: bool,

    /// Inward wall depth (along −normal) for a two-sided thicken. Only consulted
    /// when `symmetric` is `false`; `0` (default) makes a pure outward wall.
    #[param(name = "Inner thickness", default = ::fielddraft_protocol::Length::mm(0.0))]
    pub thickness_back: ::fielddraft_protocol::Length,

    /// Optional coaxial **target** face: when set, the wall grows from each source
    /// surface *up to* this face instead of by a fixed `thickness` (Onshape's
    /// "Up to face" end condition). The target must be the same coaxial analytic
    /// kind as the source (concentric sphere / coaxial cylinder / coaxial torus);
    /// the signed gap picks the direction. Empty (default) → fixed-thickness mode.
    #[param(name = "Up to face", default = ::fielddraft_protocol::Query::new(""), entity_filter = "FACE")]
    pub up_to_face: ::fielddraft_protocol::Query,

    /// Boolean op used to fold the thickened wall into the running spine.
    /// Defaults to [`BoolOp::Add`] so the wall merges into the body
    /// whose face it grew from.
    #[param(default = ::fielddraft_protocol::BoolOp::Add)]
    pub operation: BoolOp,
}

/// The analytic wall node for one face: `Difference(outer, inner)` of the two
/// coaxial offset solids, keyed under the thicken feature `id` (sub `{qi}o` /
/// `{qi}i`). The general (non-analytic) walls build their own `Shell(Offset(..))`
/// node in [`crate::brep::face_offset`]; this is only the per-primitive path.
fn thicken_diff_node(
    id: &FeatureId,
    qi: usize,
    s: &crate::brep::face_thicken::ThickenSolid,
) -> CsgNode {
    CsgNode::Difference(
        Box::new(CsgNode::Leaf {
            id: id.child(format!("{qi}o")),
            primitive: s.outer.clone(),
        }),
        Box::new(CsgNode::Leaf {
            id: id.child(format!("{qi}i")),
            primitive: s.inner.clone(),
        }),
    )
}

pub struct ThickenFeature;

impl FeatureKind for ThickenFeature {
    fn type_name(&self) -> &'static str {
        ThickenDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        ThickenDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = ThickenDef::parse(def)?;

        // No faces picked yet — the dialog was just opened. Emit nothing rather
        // than failing the regen; the wall appears once a face is picked.
        let face_queries: Vec<String> =
            d.faces.queries().iter().map(|s| s.to_string()).collect();
        if face_queries.is_empty() {
            return Ok(());
        }

        // Up-to-face mode: when a target face is given the wall grows up to it
        // and the fixed thickness / direction params are ignored. Otherwise the
        // thickness is split per the direction mode below.
        let target = d.up_to_face.as_str().trim();
        let walls = if !target.is_empty() {
            ctx.resolve_thicken_to_face(id, &face_queries, target)
                .map_err(|message| FeatureError::InvalidParameter {
                    feature: ThickenDef::TYPE_NAME,
                    name: "up_to_face",
                    message,
                })?
        } else {
            let t = d.thickness.to_mm();
            if t <= 0.0 {
                return Err(FeatureError::InvalidParameter {
                    feature: ThickenDef::TYPE_NAME,
                    name: "thickness",
                    message: format!("must be > 0 mm, got {t}"),
                });
            }
            // Direction modes → (outward offset, inward offset).
            let back = d.thickness_back.to_mm().max(0.0);
            let (outer_off, inner_off) =
                if d.symmetric { (t / 2.0, t / 2.0) } else { (t, back) };

            ctx.resolve_thicken_faces(id, &face_queries, outer_off, inner_off)
                .map_err(|message| FeatureError::InvalidParameter {
                    feature: ThickenDef::TYPE_NAME,
                    name: "faces",
                    message,
                })?
        };

        let name = optional_display_name(def, || format!("Thicken {id}"));

        // Each face resolves to a wall node in its own (carrier-/surface-)local
        // frame plus its local → world placement. A single face keeps a separable
        // placement (feature.transform = world frame → DL-56 move-cache); multiple
        // faces bake each world transform into the node and union the walls.
        let (transform, node) = if walls.len() == 1 {
            let (node, world) = walls.into_iter().next().unwrap();
            (world, node)
        } else {
            let mut acc: Option<CsgNode> = None;
            for (node, world) in walls {
                let placed = CsgNode::Transform(world, Box::new(node));
                acc = Some(match acc {
                    None => placed,
                    Some(a) => CsgNode::Union(Box::new(a), Box::new(placed)),
                });
            }
            (Transform::IDENTITY, acc.unwrap())
        };

        ctx.features.push(DecodedFeature {
            id: id.clone(),
            name,
            op: d.operation,
            blend_radius: 0.0,
            blend_mode: crate::csg::BlendMode::Fillet,
            target_ids: vec![],
            transform,
            node,
        });
        Ok(())
    }
}

// ─── K8.4: Revolve ───────────────────────────────────────────────
//
// Twin of ExtrudeFeature — consumes a sketch profile, but sweeps
// it around the local Y axis instead of translating it along Z.
// Lowers to Primitive::Revolve, which DecodedFeature::from_parts
// wraps in a CsgNode::Revolve. The sketch is interpreted in
// (r, y) coordinates: `x` becomes the radial distance from
// the Y axis. Authoring convention is `x ≥ 0`; a sketch that
// straddles the axis produces a valid but geometrically silly
// body (the centreline sweeps through itself).

/// `revolve` — Rust-native feature that sweeps a sketch profile
/// around the local Y axis.
#[derive(FeatureSchema)]
#[schema(type = "revolve")]
pub struct RevolveDef {
    /// Sketch profile this revolve consumes. Accepts the WHOLE sketch
    /// (`qCreatedBy(sketch_id)` / `qAllRegions`) or a single closed region
    /// of it (`qRegion`/`qRegions`) — ticket dhjn. The `SKETCHREGION` entity
    /// filter tells the viewport's profile picker to resolve a clicked filled
    /// region to a `qRegion` query instead of the whole sketch.
    #[param(entity_filter = "SKETCHREGION")]
    pub profiles: ::fielddraft_protocol::Query,

    /// Boolean op used to fold the revolved body into the running
    /// feature spine. Defaults to [`BoolOp::Add`] (Onshape-style).
    #[param(default = ::fielddraft_protocol::BoolOp::Add)]
    pub operation: BoolOp,

    /// Revolve axis, as a query on a single straight line. Accepts a
    /// B-Rep edge (`qEdge(hash)`) or a sketch line (`qSketchLine(sketch,
    /// line)`) — both resolve to a world-space line the profile sweeps
    /// around. REQUIRED (ticket ni56): an empty / unresolvable / off-plane
    /// axis makes the revolve invalid (no body) with a warning. There is no
    /// implicit local-Y fallback.
    #[param(default = ::fielddraft_protocol::Query::new(""), entity_filter = "EDGE,SKETCHLINE")]
    pub axis: ::fielddraft_protocol::Query,

    /// Wall thickness for hollowing the revolved body. `0` (the
    /// default) leaves it solid; any positive value lowers to a
    /// [`CsgNode::Shell`] (`|sdf| − thickness`) via
    /// [`PrimitiveModifiers`], producing a thin-walled closed shell.
    #[param(name = "Shell thickness", default = ::fielddraft_protocol::Length::mm(0.0))]
    pub shell_thickness: ::fielddraft_protocol::Length,

    /// Merge scope: explicit target bodies for the boolean op, as
    /// `qCreatedBy(<seed_feature>)` queries. Empty + `merge_with_all` off =
    /// no merge (standalone body; warned for Add/Remove/Intersect).
    #[param(name = "Merge scope", default = ::fielddraft_protocol::MultiQuery::new(""), entity_filter = "BODY")]
    pub targets: ::fielddraft_protocol::MultiQuery,

    /// Merge with all (Onshape): fold the body into EVERY body it touches
    /// instead of an explicit scope. Off by default — touching solids never
    /// merge implicitly; opt in here or name targets above.
    #[param(name = "Merge with all", default = false)]
    pub merge_with_all: bool,
}

pub struct RevolveFeature;

impl FeatureKind for RevolveFeature {
    fn type_name(&self) -> &'static str {
        RevolveDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        RevolveDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = RevolveDef::parse(def)?;

        // Resolve the `profiles` query against the sketch table.
        // Unlike ExtrudeFeature, we don't fall back to a legacy
        // half-extent stand-in: Revolve needs a real sketch to
        // produce any geometry at all. Ticket dhjn: the query may name
        // the WHOLE sketch (`qCreatedBy`) OR a single closed region of it
        // (`qRegion`/`qRegions`/`qAllRegions`) — `resolve_profile_query`
        // restricts the profile to the picked region(s) in either case.
        //
        // Ticket ni56: an incomplete revolve must produce NO body and read as
        // invalid with a clear warning — never a garbage solid. A missing /
        // unresolvable profile is exactly that, so we warn and bail out
        // (return Ok without pushing a feature) instead of erroring out.
        let Some((sketch, sketch_frame)) = crate::sketch_features::resolve_profile_query(
            d.profiles.as_str(),
            &ctx.sketches,
        ) else {
            ctx.warn_feature(
                id,
                "Revolve unvollständig: kein gültiges Profil ausgewählt — kein Körper."
                    .to_string(),
            );
            return Ok(());
        };
        let name = optional_display_name(def, || format!("Revolve {id}"));
        let rotation = optional_rotation(def);
        let scale = optional_scale(def);

        // C1: same frame→world composition as ExtrudeFeature.
        let base_transform = crate::compose_transforms(
            &sketch_frame.to_transform(),
            &Transform {
                translate: [0.0, 0.0, 0.0],
                rotation,
                scale,
            },
        );

        // Axis query → revolve around the chosen in-plane line. We
        // resolve the picked line (B-Rep edge OR sketch line) to a world
        // segment, project it into the profile's sketch frame, and apply
        // the rigid 2D map `M` that carries that line onto the canonical
        // local-Y axis (`x = 0`). The profile is pre-multiplied by `M`
        // (so the existing revolve-around-Y machinery is reused verbatim)
        // and the inverse `M⁻¹` is post-composed into the placement, so
        // the body lands exactly where the profile was drawn — only the
        // axis of revolution moves.
        //
        // Ticket ni56: an axis is REQUIRED. There is no legacy implicit
        // local-Y fallback anymore — an empty / unresolvable / off-plane axis
        // makes the revolve invalid (no body) with a clear warning, instead of
        // silently producing a body around a guessed axis (the old behaviour
        // also caused the y2xm "tail" while picking an axis: a transient regen
        // with a not-yet-resolved axis built a degenerate solid).
        let axis_2d = match classify_revolve_axis(ctx, &sketch_frame, d.axis.as_str()) {
            RevolveAxis::Valid(a, b) => (a, b),
            RevolveAxis::Missing => {
                ctx.warn_feature(
                    id,
                    "Revolve unvollständig: keine Achse ausgewählt — kein Körper."
                        .to_string(),
                );
                return Ok(());
            }
            RevolveAxis::OffPlane => {
                ctx.warn_feature(
                    id,
                    "Revolve-Achse liegt nicht in der Sketch-Ebene — kein Körper."
                        .to_string(),
                );
                return Ok(());
            }
        };

        let (a, b) = axis_2d;
        let (sketch, transform) = {
            let (m, m_inv) = revolve_axis_alignment(a, b);
            let aligned = transform_sketch_2d(sketch.clone(), &m);
            // The revolve SDF folds every query to radius √(x²+z²) ≥ 0 and
            // tests the profile there, so a profile that the axis-alignment
            // lands on the NEGATIVE-x side is never reached → an EMPTY solid
            // (tickets a204/ifn2: the SDF/deferred show only the B-Rep edges,
            // never a surface; only the WebGL lathe mesh — independent of the
            // SDF — looked right). Reversing the axis direction flips the
            // profile onto +x: the SAME revolve line, hence the SAME world
            // solid (the matching M⁻¹ places it back), so pick whichever
            // orientation lands the profile on +x.
            if crate::sketch2d::BoundedSketch2d::from_sketch(&aligned)
                .aabb()
                .max[0]
                <= 0.0
            {
                let (m2, m_inv2) = revolve_axis_alignment(b, a);
                (
                    transform_sketch_2d(sketch, &m2),
                    crate::compose_transforms(&base_transform, &m_inv2),
                )
            } else {
                (aligned, crate::compose_transforms(&base_transform, &m_inv))
            }
        };

        // The axis edge of a half-profile is interior after revolving,
        // not surface — strip it so distances near the axis are real
        // (shell would otherwise keep a phantom rod along the axis). Runs
        // AFTER `M`, so the picked axis line (now at `x = 0`) is the one
        // that gets stripped.
        let sketch = crate::sketch_features::strip_revolve_axis_segments(sketch);

        let target_ids = resolve_merge_scope(
            ctx,
            target_ids_from_queries(&d.targets),
            d.merge_with_all,
        );
        if let Some(w) = ctx.merge_scope_warning(d.operation, &target_ids) {
            ctx.warn_feature(id, w);
        }
        ctx.features.push(DecodedFeature::from_parts(
            id.clone(),
            name,
            d.operation,
            Primitive::Revolve {
                sketch,
                // The revolve axis is now expressed by aligning the
                // profile + placement (see above), so the primitive's own
                // centre is always the local origin.
                center: [0.0, 0.0, 0.0],
            },
            transform,
            PrimitiveModifiers {
                shell_thickness: d.shell_thickness.to_mm().max(0.0),
                ..PrimitiveModifiers::NONE
            },
            target_ids,
        ));
        Ok(())
    }
}

/// Classification of a revolve's `axis` query against its profile plane
/// (ticket ni56). Only [`RevolveAxis::Valid`] yields a body; the other two
/// variants make the revolve invalid (no body) with a warning.
#[derive(Debug, Clone, Copy, PartialEq)]
enum RevolveAxis {
    /// Axis resolved to an in-plane segment, given in the sketch frame's
    /// local 2D coordinates as `(a, b)`.
    Valid([f32; 2], [f32; 2]),
    /// No axis query, or the query couldn't be resolved to a world segment
    /// (e.g. an empty field or a dangling edge/line reference).
    Missing,
    /// The axis resolved but lies off the profile plane (ticket dnfe): the
    /// profile can't rotate around a line it isn't coplanar with.
    OffPlane,
}

/// Resolve a revolve's `axis` query and classify it against the profile plane.
/// An empty or unresolvable query → [`RevolveAxis::Missing`]; an off-plane
/// segment → [`RevolveAxis::OffPlane`]; otherwise the in-plane 2D endpoints.
fn classify_revolve_axis(
    ctx: &mut Context<'_>,
    sketch_frame: &crate::sketch_features::SketchFrame,
    axis_query: &str,
) -> RevolveAxis {
    let Some((w0, w1)) = crate::queries::parse_query(axis_query)
        .ok()
        .and_then(|expr| ctx.resolve_axis_world_segment(&expr))
    else {
        return RevolveAxis::Missing;
    };
    if axis_in_sketch_plane(sketch_frame, w0, w1) {
        RevolveAxis::Valid(
            sketch_frame.world_to_local_2d(w0),
            sketch_frame.world_to_local_2d(w1),
        )
    } else {
        RevolveAxis::OffPlane
    }
}

/// Whether a world-space axis segment lies IN a sketch's plane — both endpoints
/// within a length-relative tolerance of the plane. A revolve/helix axis must
/// be coplanar with the profile (the profile rotates/winds around it); an
/// off-plane pick is rejected (ticket dnfe).
fn axis_in_sketch_plane(
    frame: &crate::sketch_features::SketchFrame,
    w0: [f32; 3],
    w1: [f32; 3],
) -> bool {
    let (n, o) = (frame.normal, frame.origin);
    let dist = |p: [f32; 3]| {
        ((p[0] - o[0]) * n[0] + (p[1] - o[1]) * n[1] + (p[2] - o[2]) * n[2]).abs()
    };
    let d = [w1[0] - w0[0], w1[1] - w0[1], w1[2] - w0[2]];
    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    let tol = (len * 1e-3).max(1e-3);
    dist(w0) <= tol && dist(w1) <= tol
}

/// A rigid 2D map `p ↦ (c·x − s·y + tx, s·x + c·y + ty)` with rotation
/// angle `θ = atan2(s, c)` and unit `(c, s) = (cos θ, sin θ)`. Built by
/// [`revolve_axis_alignment`] to carry a chosen axis line onto the
/// canonical local-Y axis before revolving; applied to the profile by
/// [`transform_sketch_2d`].
#[derive(Debug, Clone, Copy)]
struct Rigid2d {
    c: f32,
    s: f32,
    tx: f32,
    ty: f32,
}

impl Rigid2d {
    fn apply(&self, p: [f32; 2]) -> [f32; 2] {
        [
            self.c * p[0] - self.s * p[1] + self.tx,
            self.s * p[0] + self.c * p[1] + self.ty,
        ]
    }
    /// The rotation angle baked into this map (radians); arcs and
    /// sectors add it to their start angle.
    fn angle(&self) -> f32 {
        self.s.atan2(self.c)
    }
}

/// Build the alignment pair for a revolve whose axis is the in-plane
/// segment `a → b` (sketch-frame 2D coordinates).
///
/// Returns `(M, M_inv)` where `M` is the rigid 2D map that sends the
/// axis line onto `x = 0` with its direction pointing `+y`, and `M_inv`
/// is the inverse as a 3D [`Transform`] (a rotation about `+Z` plus an
/// in-plane translation) that undoes `M` after the revolve so the body
/// lands back where the profile was drawn. A degenerate (zero-length)
/// segment yields the identity pair.
fn revolve_axis_alignment(a: [f32; 2], b: [f32; 2]) -> (Rigid2d, Transform) {
    let dx = b[0] - a[0];
    let dy = b[1] - a[1];
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-6 {
        return (
            Rigid2d { c: 1.0, s: 0.0, tx: 0.0, ty: 0.0 },
            Transform::IDENTITY,
        );
    }
    // Unit axis direction. The rotation R = [[dy,-dx],[dx,dy]] sends this
    // direction to (0, 1), so c = cos θ = dy, s = sin θ = dx.
    let (dxu, dyu) = (dx / len, dy / len);
    let (c, s) = (dyu, dxu);
    // Translate so the rotated axis line sits on x = 0; slide along the
    // axis (y) is irrelevant to a revolve, so leave ty = 0.
    let tx = -(c * a[0] - s * a[1]);
    let m = Rigid2d { c, s, tx, ty: 0.0 };

    // M_inv = R⁻¹(q − t): rotation by −θ about +Z, translation −R⁻¹·t.
    let theta = m.angle();
    let half = -0.5 * theta;
    let rotation = [0.0, 0.0, half.sin(), half.cos()];
    // R⁻¹·t with t = (tx, 0): (c·tx, −s·tx); negate for the translation.
    let m_inv = Transform {
        translate: [-c * tx, s * tx, 0.0],
        rotation,
        scale: [1.0, 1.0, 1.0],
    };
    (m, m_inv)
}

/// Apply a rigid 2D map to every point of a [`Sketch2d`] tree. Rotation
/// turns axis-aligned rectangles into general quads, so a [`Rectangle`]
/// is converted to a [`Polygon`] of its four (transformed) corners;
/// circles are rotation-invariant about their centre, and arc/sector
/// start angles pick up the map's rotation.
///
/// [`Rectangle`]: ::fielddraft_protocol::Sketch2dPrimitive::Rectangle
/// [`Polygon`]: ::fielddraft_protocol::Sketch2dPrimitive::Polygon
fn transform_sketch_2d(
    sketch: ::fielddraft_protocol::Sketch2d,
    m: &Rigid2d,
) -> ::fielddraft_protocol::Sketch2d {
    use ::fielddraft_protocol::{Sketch2d, Sketch2dPrimitive, SketchSegment};
    let theta = m.angle();
    let seg = |s: SketchSegment| -> SketchSegment {
        match s {
            SketchSegment::Line { start, end } => SketchSegment::Line {
                start: m.apply(start),
                end: m.apply(end),
            },
            SketchSegment::QuadBezier { p0, p1, p2 } => SketchSegment::QuadBezier {
                p0: m.apply(p0),
                p1: m.apply(p1),
                p2: m.apply(p2),
            },
            SketchSegment::Arc {
                center,
                radius,
                start_angle,
                sweep_angle,
            } => SketchSegment::Arc {
                center: m.apply(center),
                radius,
                start_angle: start_angle + theta,
                sweep_angle,
            },
        }
    };
    match sketch {
        Sketch2d::Empty => Sketch2d::Empty,
        Sketch2d::Prim(prim) => Sketch2d::Prim(match prim {
            Sketch2dPrimitive::Circle { center, radius } => Sketch2dPrimitive::Circle {
                center: m.apply(center),
                radius,
            },
            Sketch2dPrimitive::Rectangle { center, half_extents } => {
                // Rotated rect is no longer axis-aligned → emit the four
                // corners as a polygon.
                let (cx, cy) = (center[0], center[1]);
                let (hx, hy) = (half_extents[0], half_extents[1]);
                Sketch2dPrimitive::Polygon {
                    vertices: vec![
                        m.apply([cx - hx, cy - hy]),
                        m.apply([cx + hx, cy - hy]),
                        m.apply([cx + hx, cy + hy]),
                        m.apply([cx - hx, cy + hy]),
                    ],
                }
            }
            Sketch2dPrimitive::Polygon { vertices } => Sketch2dPrimitive::Polygon {
                vertices: vertices.into_iter().map(|v| m.apply(v)).collect(),
            },
            Sketch2dPrimitive::Sector {
                center,
                radius,
                start_angle,
                sweep_angle,
            } => Sketch2dPrimitive::Sector {
                center: m.apply(center),
                radius,
                start_angle: start_angle + theta,
                sweep_angle,
            },
            Sketch2dPrimitive::Path { segments, closed } => Sketch2dPrimitive::Path {
                segments: segments.into_iter().map(seg).collect(),
                closed,
            },
        }),
        Sketch2d::Union(a, b) => Sketch2d::Union(
            Box::new(transform_sketch_2d(*a, m)),
            Box::new(transform_sketch_2d(*b, m)),
        ),
        Sketch2d::Difference(a, b) => Sketch2d::Difference(
            Box::new(transform_sketch_2d(*a, m)),
            Box::new(transform_sketch_2d(*b, m)),
        ),
        Sketch2d::Intersection(a, b) => Sketch2d::Intersection(
            Box::new(transform_sketch_2d(*a, m)),
            Box::new(transform_sketch_2d(*b, m)),
        ),
        Sketch2d::Offset { inner, distance } => Sketch2d::Offset {
            inner: Box::new(transform_sketch_2d(*inner, m)),
            distance,
        },
    }
}

/// `helix` — Rust-native feature that sweeps a sketch profile along a
/// HELICAL path around the local Y axis: the real screw-thread
/// building block. The sketch is interpreted exactly like Revolve's —
/// `x` = radial distance from the axis, `y` = axial position — and one
/// profile instance is swept per revolution, advancing `pitch` per
/// turn for `turns` revolutions. The profile's axial extent should
/// stay ≤ `pitch` (adjacent turns must not overlap; every standard
/// thread form satisfies this). Union it with a core cylinder for an
/// external thread, or subtract it from a bore for an internal one.
#[derive(FeatureSchema)]
#[schema(type = "helix")]
pub struct HelixDef {
    /// Sketch profile this helix consumes. `qCreatedBy(sketch_id)`
    /// is the only query variant honoured today.
    pub profiles: ::fielddraft_protocol::Query,

    /// Which two parameters define the sweep (Onshape's "Input type"
    /// + height end condition): the third is derived at regen time —
    /// `pitch_height` derives turns, `turns_height` derives pitch.
    #[param(
        name = "Input",
        default = ::std::string::String::from("pitch_turns"),
        variants = "pitch_turns=Pitch + Turns,pitch_height=Pitch + Height,turns_height=Turns + Height"
    )]
    pub input: String,

    /// Axial advance per revolution (used unless input = Turns + Height).
    #[param(name = "Pitch", default = ::fielddraft_protocol::Length::mm(2.0))]
    pub pitch: ::fielddraft_protocol::Length,

    /// Number of revolutions, fractional turns allowed (used unless
    /// input = Pitch + Height).
    #[param(name = "Turns", default = 8.0)]
    pub turns: f32,

    /// Total axial height of the sweep (used by the *_height inputs).
    #[param(name = "Height", default = ::fielddraft_protocol::Length::mm(16.0))]
    pub height: ::fielddraft_protocol::Length,

    /// Angle (degrees) around the axis where the sweep starts,
    /// measured in the winding direction.
    #[param(name = "Start angle", default = 0.0)]
    pub start_angle: f32,

    /// Winding direction — `true` for a left-hand thread.
    #[param(name = "Left-handed", default = false)]
    pub left_handed: bool,

    /// Taper half-angle in DEGREES (NPT-style conical threads): the
    /// radius shrinks by `tan(angle)` per mm along +y. `0` = straight.
    #[param(name = "Taper angle", default = 0.0)]
    pub taper_angle: f32,

    /// Boolean op used to fold the helical body into the running
    /// feature spine. Defaults to [`BoolOp::Add`].
    #[param(default = ::fielddraft_protocol::BoolOp::Add)]
    pub operation: BoolOp,

    /// Merge scope: explicit target bodies for the boolean op, as
    /// `qCreatedBy(<seed_feature>)` queries. Empty + `merge_with_all` off =
    /// no merge (standalone body; warned for Add/Remove/Intersect).
    #[param(name = "Merge scope", default = ::fielddraft_protocol::MultiQuery::new(""), entity_filter = "BODY")]
    pub targets: ::fielddraft_protocol::MultiQuery,

    /// Merge with all (Onshape): fold the body into EVERY body it touches
    /// instead of an explicit scope. Off by default — touching solids never
    /// merge implicitly; opt in here or name targets above.
    #[param(name = "Merge with all", default = false)]
    pub merge_with_all: bool,

    /// Helix axis, as a query on a single straight line. Accepts a B-Rep
    /// edge (`qEdge(hash)`) or a sketch line (`qSketchLine(sketch, line)`)
    /// — both resolve to a world-space line the profile winds around.
    /// Empty (the default) falls back to the sketch's local Y axis at
    /// `x = 0`. Same mechanism as the Revolve axis (see [`RevolveDef`]).
    #[param(default = ::fielddraft_protocol::Query::new(""), entity_filter = "EDGE,SKETCHLINE")]
    pub axis: ::fielddraft_protocol::Query,
}

pub struct HelixFeature;

impl FeatureKind for HelixFeature {
    fn type_name(&self) -> &'static str {
        HelixDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        HelixDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = HelixDef::parse(def)?;

        // Resolve the `profiles` query against the sketch table —
        // identical to RevolveFeature (a helix without a real profile
        // produces no geometry at all).
        let mut sketch_frame = crate::sketch_features::SketchFrame::IDENTITY;
        let sketch = (|| -> Option<::fielddraft_protocol::Sketch2d> {
            let expr = crate::queries::parse_query(d.profiles.as_str()).ok()?;
            let crate::queries::QueryExpr::CreatedBy(sketch_id) = expr else {
                return None;
            };
            let sketch_data = ctx.sketches.get(&sketch_id)?;
            let sketch2d = crate::sketch_features::build_sketch2d_from_sub_calls(
                &sketch_data.sub_calls,
            );
            if matches!(sketch2d, ::fielddraft_protocol::Sketch2d::Empty) {
                None
            } else {
                sketch_frame = sketch_data.frame;
                Some(sketch2d)
            }
        })()
        .ok_or(FeatureError::MissingParameter {
            feature: HelixDef::TYPE_NAME,
            name: "profiles",
        })?;

        let name = optional_display_name(def, || format!("Helix {id}"));
        let rotation = optional_rotation(def);
        let scale = optional_scale(def);

        let base_transform = crate::compose_transforms(
            &sketch_frame.to_transform(),
            &Transform {
                translate: [0.0, 0.0, 0.0],
                rotation,
                scale,
            },
        );

        // Axis query → wind around an arbitrary in-plane line. Same
        // mechanism as RevolveFeature: align the picked line onto the
        // canonical local-Y axis with a rigid 2D map `M`, pre-multiply the
        // profile by `M`, and post-compose `M⁻¹` into the placement so the
        // helix winds around the chosen axis while the profile stays put.
        // `helix_profile_anchor` then reads the (now axis-relative) radius
        // from the transformed sketch. Empty axis → identity → legacy
        // local-Y behaviour.
        let axis_world = (|| -> Option<([f32; 3], [f32; 3])> {
            let expr = crate::queries::parse_query(d.axis.as_str()).ok()?;
            ctx.resolve_axis_world_segment(&expr)
        })();
        // Ticket dnfe: the helix axis must lie in the profile plane too.
        let axis_2d = match axis_world {
            Some((w0, w1)) if axis_in_sketch_plane(&sketch_frame, w0, w1) => Some((
                sketch_frame.world_to_local_2d(w0),
                sketch_frame.world_to_local_2d(w1),
            )),
            Some(_) => {
                ctx.warn_feature(
                    id,
                    "Helix-Achse liegt nicht in der Sketch-Ebene und wird ignoriert."
                        .to_string(),
                );
                None
            }
            None => None,
        };
        let (sketch, transform) = match axis_2d {
            None => (sketch, base_transform),
            Some((a, b)) => {
                let (m, m_inv) = revolve_axis_alignment(a, b);
                let aligned = transform_sketch_2d(sketch, &m);
                (aligned, crate::compose_transforms(&base_transform, &m_inv))
            }
        };

        // Resolve the (pitch, turns) pair from the chosen input mode —
        // the height variants derive the third quantity Onshape-style.
        let height = d.height.to_mm().max(0.001);
        let (pitch, turns) = match d.input.as_str() {
            "pitch_height" => {
                let p = d.pitch.to_mm().max(0.001);
                (p, (height / p).max(0.0))
            }
            "turns_height" => {
                let t = d.turns.max(0.001);
                ((height / t).max(0.001), t)
            }
            _ => (d.pitch.to_mm().max(0.001), d.turns.max(0.0)),
        };

        let target_ids = resolve_merge_scope(
            ctx,
            target_ids_from_queries(&d.targets),
            d.merge_with_all,
        );
        if let Some(w) = ctx.merge_scope_warning(d.operation, &target_ids) {
            ctx.warn_feature(id, w);
        }
        ctx.features.push(DecodedFeature::from_parts(
            id.clone(),
            name,
            d.operation,
            Primitive::Helix {
                sketch,
                // Axis expressed via profile + placement alignment (above),
                // so the primitive centre is always the local origin.
                center: [0.0, 0.0, 0.0],
                pitch,
                turns,
                left_handed: d.left_handed,
                taper: d.taper_angle.to_radians().tan(),
                start_angle: d.start_angle.to_radians(),
            },
            transform,
            PrimitiveModifiers::NONE,
            target_ids,
        ));
        Ok(())
    }
}

// ─── K8.0: Linear pattern ────────────────────────────────────────
//
// The first bona-fide parametric pattern feature. Given a
// source feature id, a direction vector, a count and a spacing
// distance, [`LinearPatternFeature`] emits `count - 1` extra
// copies of the source spine entry at offsets
// `i * spacing * unit_dir` for `i = 1..count`. The originals
// stay in place — Onshape semantics.
//
// Copies inherit the source's BoolOp, primitive payload and
// transform verbatim, with the offset added to
// `transform.translate`. Their feature ids are hierarchical
// children of the pattern feature (`pattern_id/1`,
// `pattern_id/2`, …) so provenance still resolves to "this
// clone came from the pattern".
//
// Known limits for this first cut:
//
// * Source must already be on the spine when the pattern
//   runs, i.e. the source call must appear *before* the
//   pattern call in `feature_calls`. Not enforced — a
//   missing source just errors out with `MissingParameter`.
// * Pattern-of-pattern isn't supported: the source lookup
//   walks `document.features` by id, and pattern children
//   are siblings of the pattern feature itself, not under it.

/// Resolve a `source` MultiQuery (a list of `qCreatedBy(id)` exprs)
/// into the set of source feature ids it references. Mirrors the
/// resolution path in [`BooleanFeature::execute`], but goes through
/// [`crate::queries::parse_query`] so malformed query strings surface
/// as a clean [`FeatureError::InvalidParameter`] rather than silently
/// dropping out. Order is preserved and duplicates are kept out.
fn resolve_source_feature_ids(
    mq: &MultiQuery,
    feature: &'static str,
) -> Result<Vec<FeatureId>, FeatureError> {
    use crate::queries::QueryExpr;
    let mut ids: Vec<FeatureId> = Vec::new();
    for q in mq.queries() {
        let expr = crate::queries::parse_query(q).map_err(|e| {
            FeatureError::InvalidParameter {
                feature,
                name: "source",
                message: format!("could not parse source query `{q}`: {e}"),
            }
        })?;
        match expr {
            QueryExpr::CreatedBy(id) | QueryExpr::FaceOf(id) => {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
            other => {
                return Err(FeatureError::InvalidParameter {
                    feature,
                    name: "source",
                    message: format!(
                        "source must reference a body via qCreatedBy(id); got `{other}`"
                    ),
                });
            }
        }
    }
    Ok(ids)
}

/// `linear_pattern` — replicate a source feature along a
/// direction vector.
#[derive(FeatureSchema)]
#[schema(type = "linear_pattern")]
pub struct LinearPatternDef {
    /// Bodies to replicate, as a list of `qCreatedBy(id)` queries.
    /// Each referenced source feature is replicated independently.
    #[param(name = "Source", entity_filter = "BODY")]
    pub source: ::fielddraft_protocol::MultiQuery,
    /// Direction vector in local frame. Doesn't have to be
    /// normalised — the executor divides by its length at
    /// apply time.
    pub direction: [f32; 3],
    /// Total number of instances *including* the source. Stored
    /// as an `f32` (not `u32`) so the generic FeatureDialog's
    /// number widget can edit it — the derive maps plain integer
    /// types to the `generic → unknown` widget and they'd never
    /// render. `execute` rounds to the nearest positive integer.
    /// `count = 1` is a no-op, `count = 5` means the pattern
    /// adds four offset copies.
    pub count: f32,
    /// Distance between consecutive instances in millimetres.
    pub spacing: ::fielddraft_protocol::Length,
}

pub struct LinearPatternFeature;

impl FeatureKind for LinearPatternFeature {
    fn type_name(&self) -> &'static str {
        LinearPatternDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        LinearPatternDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = LinearPatternDef::parse(def)?;
        let count = d.count.round().max(0.0) as u32;
        if count < 2 {
            // A one-instance pattern is a no-op. Don't error —
            // this is a useful parameter-sweep resting point.
            return Ok(());
        }
        let spacing_mm = d.spacing.to_mm();
        if spacing_mm <= 0.0 {
            return Err(FeatureError::InvalidParameter {
                feature: LinearPatternDef::TYPE_NAME,
                name: "spacing",
                message: format!("must be > 0 mm, got {spacing_mm}"),
            });
        }

        // Normalise the direction. A zero-length vector is
        // invalid — there's no axis to replicate along.
        let dx = d.direction[0];
        let dy = d.direction[1];
        let dz = d.direction[2];
        let len = (dx * dx + dy * dy + dz * dz).sqrt();
        if len < 1e-6 {
            return Err(FeatureError::InvalidParameter {
                feature: LinearPatternDef::TYPE_NAME,
                name: "direction",
                message: "must be a non-zero vector".to_string(),
            });
        }
        let unit = [dx / len, dy / len, dz / len];

        // Resolve the source bodies from the query list.
        let source_ids =
            resolve_source_feature_ids(&d.source, LinearPatternDef::TYPE_NAME)?;
        if source_ids.is_empty() {
            return Err(FeatureError::InvalidParameter {
                feature: LinearPatternDef::TYPE_NAME,
                name: "source",
                message: "must reference at least one source body".to_string(),
            });
        }

        // Replicate every referenced source body independently.
        for source_fid in &source_ids {
            let source_feature = ctx
                .features
                .iter()
                .find(|f| &f.id == source_fid)
                .ok_or(FeatureError::MissingParameter {
                    feature: LinearPatternDef::TYPE_NAME,
                    name: "source",
                })?
                .clone();

            // Emit `count - 1` offset copies. `i = 0` would be the
            // source itself, which is already on the spine.
            // Clones carry hierarchical child ids under the
            // pattern, namespaced by the source feature id so
            // multiple sources don't collide — `sync_feature_calls`
            // drops those via its parent filter so the wire form
            // stays authoritative.
            for i in 1..count {
                let i_f = i as f32;
                let offset = [
                    unit[0] * spacing_mm * i_f,
                    unit[1] * spacing_mm * i_f,
                    unit[2] * spacing_mm * i_f,
                ];
                let mut clone = source_feature.clone();
                clone.id = id.child(format!("{source_fid}_{i}"));
                clone.name = format!("{} {}", source_feature.name, i);
                clone.transform.translate = [
                    source_feature.transform.translate[0] + offset[0],
                    source_feature.transform.translate[1] + offset[1],
                    source_feature.transform.translate[2] + offset[2],
                ];
                ctx.features.push(clone);
            }
        }

        Ok(())
    }
}

// ─── K8.1: Circular pattern ──────────────────────────────────────
//
// Twin of the linear pattern: instead of offsetting clones by
// a constant translation along a direction vector, each clone is
// rotated around an `axis` through a `center` pivot by an
// angle that scales with its index. Default sweep is a full
// 360° divided evenly over `count` instances; arcs come in by
// overriding `angle_deg`.
//
// The math per clone at index `i ∈ 1..count`:
//   θ        = (i / count) · angle_deg · π / 180
//   q_delta  = axis_angle_to_quat(unit_axis, θ)
//   offset   = source.translate − center
//   clone.translate = q_delta · offset + center
//   clone.rotation  = q_delta · source.rotation
//
// This composes the pattern's rotation on top of whatever
// transform the source feature already carries, so you can
// chain circular-pattern a rotated box and every clone stays
// rotated relative to its own local axis.

/// `circular_pattern` — replicate a source feature around an
/// axis through a pivot point.
#[derive(FeatureSchema)]
#[schema(type = "circular_pattern")]
pub struct CircularPatternDef {
    /// Bodies to replicate, as a list of `qCreatedBy(id)` queries.
    /// Each referenced source feature is replicated independently.
    #[param(name = "Source", entity_filter = "BODY")]
    pub source: ::fielddraft_protocol::MultiQuery,
    /// Rotation axis in local frame. The executor normalises
    /// it; a zero vector errors out.
    pub axis: [f32; 3],
    /// Pivot point the axis passes through.
    #[param(default = [0.0, 0.0, 0.0])]
    pub center: [f32; 3],
    /// Total number of instances including the source. `f32`
    /// for dialog-widget compatibility — `execute` rounds to
    /// the nearest positive integer. See [`LinearPatternDef::count`]
    /// for the full rationale.
    pub count: f32,
    /// Total sweep in degrees. Defaults to a full 360° when
    /// omitted from the call.
    #[param(default = 360.0, unit = "\u{00b0}")]
    pub angle_deg: f32,
}

pub struct CircularPatternFeature;

impl FeatureKind for CircularPatternFeature {
    fn type_name(&self) -> &'static str {
        CircularPatternDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        CircularPatternDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = CircularPatternDef::parse(def)?;
        let count = d.count.round().max(0.0) as u32;
        if count < 2 {
            return Ok(());
        }
        // Normalise the axis.
        let ax = d.axis[0];
        let ay = d.axis[1];
        let az = d.axis[2];
        let axis_len = (ax * ax + ay * ay + az * az).sqrt();
        if axis_len < 1e-6 {
            return Err(FeatureError::InvalidParameter {
                feature: CircularPatternDef::TYPE_NAME,
                name: "axis",
                message: "must be a non-zero vector".to_string(),
            });
        }
        let unit = [ax / axis_len, ay / axis_len, az / axis_len];

        let source_ids =
            resolve_source_feature_ids(&d.source, CircularPatternDef::TYPE_NAME)?;
        if source_ids.is_empty() {
            return Err(FeatureError::InvalidParameter {
                feature: CircularPatternDef::TYPE_NAME,
                name: "source",
                message: "must reference at least one source body".to_string(),
            });
        }

        // Per-instance angle step.
        let step_rad =
            (d.angle_deg / count as f32) * std::f32::consts::PI / 180.0;

        for source_fid in &source_ids {
            let source_feature = ctx
                .features
                .iter()
                .find(|f| &f.id == source_fid)
                .ok_or(FeatureError::MissingParameter {
                    feature: CircularPatternDef::TYPE_NAME,
                    name: "source",
                })?
                .clone();

            for i in 1..count {
                let theta = step_rad * i as f32;
                let half = theta * 0.5;
                let s = half.sin();
                let q_delta = [unit[0] * s, unit[1] * s, unit[2] * s, half.cos()];

                // Rotate the source translate around the pivot.
                let offset = [
                    source_feature.transform.translate[0] - d.center[0],
                    source_feature.transform.translate[1] - d.center[1],
                    source_feature.transform.translate[2] - d.center[2],
                ];
                let rotated = crate::quat_rotate(q_delta, offset);
                let new_translate = [
                    rotated[0] + d.center[0],
                    rotated[1] + d.center[1],
                    rotated[2] + d.center[2],
                ];

                let mut clone = source_feature.clone();
                clone.id = id.child(format!("{source_fid}_{i}"));
                clone.name = format!("{} {}", source_feature.name, i);
                clone.transform.translate = new_translate;
                clone.transform.rotation =
                    crate::quat_multiply(q_delta, source_feature.transform.rotation);
                ctx.features.push(clone);
            }
        }

        Ok(())
    }
}

// ─── K8.2: Mirror ────────────────────────────────────────────────
//
// Completes the K8 Pattern block with the degenerate case: a
// single clone produced by reflecting the source feature's
// translate through a plane defined by a point and a normal.
// The rotation is kept verbatim — reflecting a rotation would
// flip chirality, which is a separate feature request (and
// would need a sign flip on one sketch axis to actually matter
// at the geometry level). For K8.2 the aim is "this body appears
// on the other side of the plane"; chiral-aware mirroring comes
// later with body-level symmetry.

/// `mirror` — reflect a source feature's translate through a
/// plane.
#[derive(FeatureSchema)]
#[schema(type = "mirror")]
pub struct MirrorDef {
    /// Bodies to mirror, as a list of `qCreatedBy(id)` queries.
    /// Each referenced source feature is mirrored independently.
    #[param(name = "Source", entity_filter = "BODY")]
    pub source: ::fielddraft_protocol::MultiQuery,
    /// Mirror plane, given as a face/datum query (e.g. `qFace(<hash>)`
    /// or `origin_plane_xy`). Its world-space frame supplies both the
    /// plane normal and a point on the plane.
    #[param(name = "Plane", entity_filter = "FACE")]
    pub plane: ::fielddraft_protocol::Query,
}

pub struct MirrorFeature;

impl FeatureKind for MirrorFeature {
    fn type_name(&self) -> &'static str {
        MirrorDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        MirrorDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = MirrorDef::parse(def)?;

        // Resolve the mirror plane from the face/datum query. Its
        // world-space frame gives both the plane normal and a point
        // on the plane.
        let plane_expr = crate::queries::parse_query(d.plane.as_str()).map_err(|e| {
            FeatureError::InvalidParameter {
                feature: MirrorDef::TYPE_NAME,
                name: "plane",
                message: format!("could not parse plane query: {e}"),
            }
        })?;
        let frame = ctx.resolve_plane_frame(&plane_expr).ok_or(
            FeatureError::InvalidParameter {
                feature: MirrorDef::TYPE_NAME,
                name: "plane",
                message: "plane query resolved to no datum and no face".to_string(),
            },
        )?;
        let n = frame.normal;
        let plane_point = frame.origin;

        let source_ids =
            resolve_source_feature_ids(&d.source, MirrorDef::TYPE_NAME)?;
        if source_ids.is_empty() {
            return Err(FeatureError::InvalidParameter {
                feature: MirrorDef::TYPE_NAME,
                name: "source",
                message: "must reference at least one source body".to_string(),
            });
        }

        for source_fid in &source_ids {
            let source_feature = ctx
                .features
                .iter()
                .find(|f| &f.id == source_fid)
                .ok_or(FeatureError::MissingParameter {
                    feature: MirrorDef::TYPE_NAME,
                    name: "source",
                })?
                .clone();

            // Reflect p through the plane: p' = p - 2 * ((p - p0) · n) * n
            let p = source_feature.transform.translate;
            let rel = [
                p[0] - plane_point[0],
                p[1] - plane_point[1],
                p[2] - plane_point[2],
            ];
            let dot = rel[0] * n[0] + rel[1] * n[1] + rel[2] * n[2];
            let reflected = [
                p[0] - 2.0 * dot * n[0],
                p[1] - 2.0 * dot * n[1],
                p[2] - 2.0 * dot * n[2],
            ];

            let mut clone = source_feature.clone();
            clone.id = id.child(format!("{source_fid}_1"));
            clone.name = format!("{} mirror", source_feature.name);
            clone.transform.translate = reflected;
            ctx.features.push(clone);
        }

        Ok(())
    }
}

// ─── parameter helpers for the K1 primitives ─────────────────────

/// Resolve a "Merge scope" `targets` multi-query into explicit
/// `target_ids` (ticket 15f5): each `qCreatedBy(<feature_id>)` entry
/// names a seed feature whose body the boolean op folds into. An empty
/// list keeps the legacy AABB-overlap docking. Shared by every
/// boolean-capable feature (gyroid, helix, extrude, revolve).
fn target_ids_from_queries(
    targets: &::fielddraft_protocol::MultiQuery,
) -> Vec<FeatureId> {
    targets
        .queries()
        .iter()
        .filter_map(|q| {
            let inner = q.strip_prefix("qCreatedBy(")?.strip_suffix(')')?;
            inner.parse::<FeatureId>().ok()
        })
        .collect()
}

/// Resolve the effective merge-scope `target_ids` of a feature-implicit
/// boolean. `explicit` are the targets parsed from the "Merge scope" query
/// (may be empty); `merge_with_all` expands an empty scope into the seed id
/// of every body that exists *before* this feature in the spine. A resolved
/// scope routes the body through the explicit targeted-dock path in
/// [`crate::body::partition`] — touching solids never merge implicitly.
fn resolve_merge_scope(
    ctx: &Context<'_>,
    explicit: Vec<FeatureId>,
    merge_with_all: bool,
) -> Vec<FeatureId> {
    if !explicit.is_empty() {
        return explicit;
    }
    if merge_with_all {
        return ctx.current_body_seed_ids();
    }
    Vec::new()
}

/// Non-fatal warning for a feature-implicit boolean whose merge scope is
/// empty: `Remove`/`Intersect` need a target body (an explicit scope or
/// "Merge with all"), otherwise the operation has nothing to act on and the
/// body shows as a red translucent preview. `New` (a standalone body) is
/// always fine. A scope-less `Add` is fine **iff it is the first/standalone
/// solid** — like Onshape, the very first solid is implicitly "New"; only an
/// `Add` that *could* have merged into an existing body (but didn't pick one)
/// is flagged. See [`Context::merge_scope_warning`].
fn merge_scope_warning(op: BoolOp, resolved_targets: &[FeatureId], has_prior_bodies: bool) -> Option<String> {
    if op == BoolOp::New || !resolved_targets.is_empty() {
        return None;
    }
    // First/standalone Add: nothing to merge into yet → implicit New, no warning.
    if op == BoolOp::Add && !has_prior_bodies {
        return None;
    }
    Some(
        "Kein Merge-Scope gewählt — Add/Remove/Intersect braucht einen Zielkörper. \
         Aktiviere „Merge with all“ oder wähle einen oder mehrere Zielkörper, \
         sonst bleibt der Körper unverbunden."
            .to_string(),
    )
}

/// Extract an optional `"name"` string parameter from the
/// definition JSON, falling back to a runtime-derived label that
/// depends on the `FeatureId` being passed into `execute`. The
/// derive can't express this fallback because its `default`
/// attribute quotes Rust rvalues statically — the id isn't in
/// scope at attribute-parse time.
fn optional_display_name(def: &Value, fallback: impl FnOnce() -> String) -> String {
    def.get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(fallback)
}

/// Extract an optional `"rotation"` quaternion (`[x, y, z, w]`) from
/// the definition JSON, falling back to the identity quaternion
/// when missing or malformed. Used by the K1 primitives'
/// `execute` so that a `FeatureCall → Feature` regeneration pass
/// preserves any rotation the user set via `setFeatureTransform`.
/// Without this helper, K4.5.4d's bootstrap would silently reset
/// every rotation to identity.
fn optional_rotation(def: &Value) -> [f32; 4] {
    def.get("rotation")
        .and_then(|v| v.as_array())
        .and_then(|arr| {
            if arr.len() != 4 {
                return None;
            }
            let mut out = [0.0_f32; 4];
            for (i, item) in arr.iter().enumerate() {
                out[i] = item.as_f64()? as f32;
            }
            Some(out)
        })
        .unwrap_or([0.0, 0.0, 0.0, 1.0])
}

/// Extract an optional `"scale"` value from the definition JSON,
/// falling back to the identity scale (`1.0`) when missing or
/// malformed. Mirrors [`optional_rotation`] — both close the
/// lossy-round-trip gap in the K4.5.4d bootstrap path.
fn optional_scale(def: &Value) -> [f32; 3] {
    match def.get("scale") {
        Some(Value::Number(n)) => {
            let s = n.as_f64().unwrap_or(1.0) as f32;
            [s, s, s]
        }
        Some(Value::Array(arr)) if arr.len() == 3 => {
            let x = arr[0].as_f64().unwrap_or(1.0) as f32;
            let y = arr[1].as_f64().unwrap_or(1.0) as f32;
            let z = arr[2].as_f64().unwrap_or(1.0) as f32;
            [x, y, z]
        }
        _ => [1.0, 1.0, 1.0],
    }
}

/// Post-parse guard for numeric fields that must be strictly
/// positive. The derive's `f32` slot parser only checks "is it a
/// number"; domain constraints like "radius > 0" are re-checked
/// here so the K1 primitives still reject zero and negative
/// values the same way the K4.5.2 hand-rolled versions did.
fn positive_f32(
    value: f32,
    feature: &'static str,
    name: &'static str,
) -> Result<(), FeatureError> {
    if value > 0.0 {
        Ok(())
    } else {
        Err(FeatureError::InvalidParameter {
            feature,
            name,
            message: format!("must be > 0, got {value}"),
        })
    }
}

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

// ─── Material ────────────────────────────────────────────────────
//
// Non-geometric feature that assigns PBR + engineering material
// properties to one or more target bodies.  The kernel stores it
// as a FeatureCall (no spine entry); the frontend reads the params
// to drive shader uniforms.

#[derive(FeatureSchema)]
#[schema(type = "material")]
pub struct MaterialDef {
    #[param(name = "Bodies", default = ::fielddraft_protocol::MultiQuery::new(""), entity_filter = "BODY")]
    pub targets: ::fielddraft_protocol::MultiQuery,

    // ── PBR optical ──────────────────────────────────────────
    #[param(name = "Color", default = [0.55, 0.65, 0.85], ui_hint = "color")]
    pub color: [f32; 3],
    #[param(name = "Roughness", default = 0.35, ui_hint = "slider")]
    pub roughness: f32,
    #[param(name = "Metalness", default = 0.0, ui_hint = "slider")]
    pub metalness: f32,
    #[param(name = "Emissive", default = [0.0, 0.0, 0.0], ui_hint = "color")]
    pub emissive: [f32; 3],
    #[param(name = "Opacity", default = 1.0, ui_hint = "slider")]
    pub opacity: f32,
    #[param(name = "Wireframe", default = false)]
    pub wireframe: bool,
    #[param(name = "Displace depth", default = 0.0)]
    pub displace_depth: f32,
    #[param(name = "Displace freq", default = 0.0)]
    pub displace_freq: f32,

    // ── Engineering ──────────────────────────────────────────
    #[param(name = "Density (kg/m³)", default = 0.0)]
    pub density: f32,
    #[param(name = "Young's modulus (GPa)", default = 0.0)]
    pub youngs_modulus: f32,
    #[param(name = "Poisson's ratio", default = 0.0)]
    pub poissons_ratio: f32,
    #[param(name = "Yield tensile (MPa)", default = 0.0)]
    pub yield_tensile: f32,
    #[param(name = "Ultimate tensile (MPa)", default = 0.0)]
    pub ultimate_tensile: f32,
    #[param(name = "Yield compressive (MPa)", default = 0.0)]
    pub yield_compressive: f32,
}

pub struct MaterialFeature;

impl FeatureKind for MaterialFeature {
    fn type_name(&self) -> &'static str {
        MaterialDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        MaterialDef::schema_json()
    }

    fn execute(
        &self,
        _ctx: &mut Context<'_>,
        _id: &FeatureId,
        _def: &Value,
    ) -> Result<(), FeatureError> {
        Ok(())
    }
}

// ─── tests ───────────────────────────────────────────────────────

#[cfg(test)]
#[path = "features_tests.rs"]
mod tests;
