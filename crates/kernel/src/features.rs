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
        let variables = ctx.document.variables.clone();
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
    primitive: Primitive,
    translate: [f32; 3],
    mods: PrimitiveModifiers,
) {
    let name = optional_display_name(def, || format!("{type_label} {id}"));
    let rotation = optional_rotation(def);
    let scale = optional_scale(def);
    ctx.features.push(DecodedFeature::from_parts(
        id.clone(),
        name,
        op,
        primitive,
        Transform { translate, rotation, scale },
        mods,
        vec![],
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
        push_primitive(ctx, id, def, "Box", d.op,
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
        push_primitive(ctx, id, def, "Sphere", d.op,
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
        push_primitive(ctx, id, def, "Cylinder", d.op,
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
        push_primitive(ctx, id, def, "Cone", d.op,
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
        push_primitive(ctx, id, def, "Thread", d.op,
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
        push_primitive(ctx, id, def, "Torus", d.op,
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
        push_primitive(ctx, id, def, "Capsule", d.op,
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
        push_primitive(ctx, id, def, "Ellipsoid", d.op,
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
        push_primitive(ctx, id, def, "HexPrism", d.op,
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

// ─── K13.2: Sweep Feature (straight-line v0) ────────────────────
//
// V0 sweep: extrudes a sketch profile along an arbitrary 3D
// direction vector. Internally lowers to Primitive::Extrude
// with a rotation that aligns the local Z axis to the sweep
// direction. Curved sweeps (arc/spline path) come in v1 via
// mesh_sdf lofting.

/// `sweep` — straight-line sweep along a direction vector.
#[derive(FeatureSchema)]
#[schema(type = "sweep")]
pub struct SweepDef {
    pub profiles: ::fielddraft_protocol::Query,
    pub direction: [f32; 3],
    pub length: ::fielddraft_protocol::Length,
    #[param(default = ::fielddraft_protocol::BoolOp::Add)]
    pub operation: BoolOp,
    #[param(default = [0.0, 0.0, 0.0])]
    pub center: [f32; 3],
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
        let length_mm = d.length.to_mm();
        if length_mm <= 0.0 {
            return Err(FeatureError::InvalidParameter {
                feature: SweepDef::TYPE_NAME,
                name: "length",
                message: format!("must be > 0 mm, got {length_mm}"),
            });
        }

        // Resolve the sketch profile.
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

        // Build rotation quaternion that maps Z → direction.
        let dx = d.direction[0];
        let dy = d.direction[1];
        let dz = d.direction[2];
        let dir_len = (dx * dx + dy * dy + dz * dz).sqrt();
        if dir_len < 1e-6 {
            return Err(FeatureError::InvalidParameter {
                feature: SweepDef::TYPE_NAME,
                name: "direction",
                message: "must be a non-zero vector".to_string(),
            });
        }
        let dir = [dx / dir_len, dy / dir_len, dz / dir_len];

        // Rotation from Z=(0,0,1) to dir via axis-angle.
        let z = [0.0_f32, 0.0, 1.0];
        let dot = z[2] * dir[2] + z[1] * dir[1] + z[0] * dir[0];
        let rotation = if dot > 0.9999 {
            [0.0, 0.0, 0.0, 1.0] // already aligned
        } else if dot < -0.9999 {
            [0.0, 1.0, 0.0, 0.0] // 180° around Y
        } else {
            // axis = cross(z, dir), angle = acos(dot)
            let ax = z[1] * dir[2] - z[2] * dir[1];
            let ay = z[2] * dir[0] - z[0] * dir[2];
            let az = z[0] * dir[1] - z[1] * dir[0];
            let axis_len = (ax * ax + ay * ay + az * az).sqrt();
            let half = (dot.clamp(-1.0, 1.0)).acos() * 0.5;
            let s = half.sin() / axis_len;
            [ax * s, ay * s, az * s, half.cos()]
        };

        let name = optional_display_name(def, || format!("Sweep {id}"));

        ctx.features.push(DecodedFeature::from_parts(
            id.child("0"),
            name,
            d.operation,
            Primitive::Extrude {
                sketch,
                half_depth_z: length_mm / 2.0,
                center: d.center,
            },
            Transform {
                translate: [0.0, 0.0, 0.0],
                rotation,
                scale: [1.0, 1.0, 1.0],
            },
            PrimitiveModifiers::NONE,
            vec![],
        ));
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

/// `mate_connector` — local coordinate system on a surface.
///
/// C2: the connector is now anchored to a B-Rep **face** (resolved
/// mid-loop against the live geometry) rather than carrying a baked
/// world origin/normal. Its CSYS origin is the face centroid shifted by
/// `offset` in the face's in-plane basis; its Z-axis is the face normal.
#[derive(FeatureSchema)]
#[schema(type = "mate_connector")]
pub struct MateConnectorDef {
    /// Face this connector attaches to (e.g. `qFace(<id_hash>)`).
    #[param(entity_filter = "FACE")]
    pub face: ::fielddraft_protocol::Query,
    /// In-plane `(u, v)` offset from the face centroid, in the face's
    /// synthesised `x_axis`/`y_axis` basis. Defaults to no offset.
    #[param(default = [0.0, 0.0])]
    pub offset: [f32; 2],
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
        // features. C2: resolve the `face` query against the live
        // geometry, derive the CSYS, and stash it for the viewport.
        let d = MateConnectorDef::parse(def)?;
        let expr = crate::queries::parse_query(d.face.as_str()).map_err(|e| {
            FeatureError::InvalidParameter {
                feature: MateConnectorDef::TYPE_NAME,
                name: "face",
                message: format!("could not parse face query: {e}"),
            }
        })?;
        let frame = ctx.resolve_face_frame(&expr).ok_or(FeatureError::InvalidParameter {
            feature: MateConnectorDef::TYPE_NAME,
            name: "face",
            message: "face query resolved to no face".to_string(),
        })?;
        // Shift the centroid by the in-plane offset.
        let origin = [
            frame.origin[0] + d.offset[0] * frame.x_axis[0] + d.offset[1] * frame.y_axis[0],
            frame.origin[1] + d.offset[0] * frame.x_axis[1] + d.offset[1] * frame.y_axis[1],
            frame.origin[2] + d.offset[0] * frame.x_axis[2] + d.offset[1] * frame.y_axis[2],
        ];
        ctx.mate_connectors.push(ResolvedMateConnector {
            id: id.clone(),
            origin,
            normal: frame.normal,
            x_axis: frame.x_axis,
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
            vec![],
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

        let target_ids: Vec<_> = d
            .targets
            .queries()
            .iter()
            .filter_map(|q| {
                let inner = q.strip_prefix("qCreatedBy(")?.strip_suffix(')')?;
                inner.parse::<FeatureId>().ok()
            })
            .collect();

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
            vec![],
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
    /// [`Context::sketches`] at regeneration time. Only
    /// `qCreatedBy(sketch_id)` is honoured today; other query
    /// variants fall through to the fallback extents.
    pub profiles: ::fielddraft_protocol::Query,

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
    /// running feature spine. Defaults to [`BoolOp::Add`]
    /// (Onshape-style): the extrusion merges into the body it
    /// touches, so e.g. extruding a box face grows that box
    /// instead of stacking a second body onto it.
    #[param(default = ::fielddraft_protocol::BoolOp::Add)]
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

    /// Planar B-Rep face(s) to extrude instead of a sketch — the face-extrude
    /// path (P1). Comma-separated `qFace(<id_hash>)` selections; all faces must
    /// be coplanar. When non-empty this takes precedence over `profiles`: the
    /// faces' boundary loops are projected into their shared plane and unioned
    /// into the extrude profile (see [`Context::resolve_extrude_faces`]).
    #[param(name = "Faces", default = ::fielddraft_protocol::MultiQuery::new(""), entity_filter = "FACE")]
    pub faces: ::fielddraft_protocol::MultiQuery,

    /// Direction mode. `true` (default) grows the profile symmetrically by
    /// `±depth/2` about the plane — the established sketch-extrude behaviour.
    /// `false` is a one-sided (blind) extrude `[−depth_back, +depth]` along the
    /// plane normal; the web face-extrude tool sends `false` for the Onshape
    /// default of growing outward from the picked face.
    #[param(name = "Symmetric", default = true)]
    pub symmetric: bool,

    /// Backward depth for a two-sided (asymmetric) extrude, measured against the
    /// plane normal. Only consulted when `symmetric` is `false`; `0` (default)
    /// makes a pure blind extrude.
    #[param(name = "Back depth", default = ::fielddraft_protocol::Length::mm(0.0))]
    pub depth_back: ::fielddraft_protocol::Length,
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
        // A coplanar `qFace(...)` selection drives the profile and takes
        // precedence over `profiles`. The selected faces' boundary loops are
        // projected into their shared plane, unioned, and swept exactly like a
        // sketch profile; the face frame places that profile back onto the plane.
        let face_queries: Vec<String> =
            d.faces.queries().iter().map(|s| s.to_string()).collect();
        if !face_queries.is_empty() {
            let (sketch, frame) =
                ctx.resolve_extrude_faces(&face_queries).map_err(|message| {
                    FeatureError::InvalidParameter {
                        feature: ExtrudeDef::TYPE_NAME,
                        name: "faces",
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
            let target_ids = ctx.face_owner_target_ids(&face_queries);
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
        if let Ok(crate::queries::QueryExpr::CreatedBy(sketch_id)) =
            crate::queries::parse_query(d.profiles.as_str())
        {
            if let Some(sketch_data) = ctx.sketches.get(&sketch_id) {
                let sketch2d = crate::sketch_features::build_sketch2d_from_sub_calls(
                    &sketch_data.sub_calls,
                );
                if !matches!(sketch2d, ::fielddraft_protocol::Sketch2d::Empty) {
                    resolved_sketch = Some(sketch2d);
                    sketch_frame = sketch_data.frame;
                } else {
                    sketch_was_found_but_empty = true;
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

        ctx.features.push(DecodedFeature::from_parts(
            id.clone(),
            name,
            d.operation,
            primitive,
            transform,
            mods,
            vec![],
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
    /// Sketch profile this revolve consumes. `qCreatedBy(sketch_id)`
    /// is the only query variant honoured today.
    pub profiles: ::fielddraft_protocol::Query,

    /// Boolean op used to fold the revolved body into the running
    /// feature spine. Defaults to [`BoolOp::Add`] (Onshape-style).
    #[param(default = ::fielddraft_protocol::BoolOp::Add)]
    pub operation: BoolOp,

    /// Override for the revolve centre in world space.
    #[param(default = [0.0, 0.0, 0.0])]
    pub center: [f32; 3],

    /// Wall thickness for hollowing the revolved body. `0` (the
    /// default) leaves it solid; any positive value lowers to a
    /// [`CsgNode::Shell`] (`|sdf| − thickness`) via
    /// [`PrimitiveModifiers`], producing a thin-walled closed shell.
    #[param(name = "Shell thickness", default = ::fielddraft_protocol::Length::mm(0.0))]
    pub shell_thickness: ::fielddraft_protocol::Length,
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
        // produce any geometry at all.
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
            feature: RevolveDef::TYPE_NAME,
            name: "profiles",
        })?;
        // The axis edge of a half-profile is interior after revolving,
        // not surface — strip it so distances near the axis are real
        // (shell would otherwise keep a phantom rod along the axis).
        let sketch = crate::sketch_features::strip_revolve_axis_segments(sketch);

        let name = optional_display_name(def, || format!("Revolve {id}"));
        let rotation = optional_rotation(def);
        let scale = optional_scale(def);

        // C1: same frame→world composition as ExtrudeFeature.
        let transform = crate::compose_transforms(
            &sketch_frame.to_transform(),
            &Transform {
                translate: [0.0, 0.0, 0.0],
                rotation,
                scale,
            },
        );

        ctx.features.push(DecodedFeature::from_parts(
            id.clone(),
            name,
            d.operation,
            Primitive::Revolve {
                sketch,
                center: d.center,
            },
            transform,
            PrimitiveModifiers {
                shell_thickness: d.shell_thickness.to_mm().max(0.0),
                ..PrimitiveModifiers::NONE
            },
            vec![],
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
        _id: &FeatureId,
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
                    }
                }
                _ => {
                    feature.op = BoolOp::Remove;
                }
            }
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
    #[param(name = "Bodies", default = ::fielddraft_protocol::MultiQuery::new(""))]
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
mod tests {
    use super::*;
    use serde_json::json;

    fn id(n: u32) -> FeatureId {
        FeatureId::from_u32(n)
    }

    fn empty_ctx() -> Document {
        Document::default()
    }

    #[test]
    fn with_builtins_registers_the_k1_primitives_and_k4_5_8_extrude() {
        let registry = FeatureRegistry::with_builtins();
        assert_eq!(registry.len(), 28);
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
        assert_eq!(params.len(), 10);
    }

    #[test]
    fn schema_for_returns_none_for_unknown_types() {
        let registry = FeatureRegistry::with_builtins();
        assert!(registry.schema_for("nonexistent_type").is_none());
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
                "cylinder",
                "ellipsoid",
                "extrude",
                "fillet_edge",
                "gyroid",
                "hex_prism",
                "hole",
                "imported_mesh",
                "linear_pattern",
                "mate_connector",
                "material",
                "mirror",
                "revolve",
                "rotate",
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
        // primitive should match the K4.5.2 hand-rolled surface
        // exactly — eleven params after shell_thickness was added.
        let schema = BoxDef::schema_json();
        assert_eq!(schema["type"], json!("box"));
        let params = schema["params"].as_array().unwrap();
        assert_eq!(params.len(), 11);

        assert_eq!(params[0]["key"], json!("half_extents"));
        assert_eq!(params[0]["required"], json!(true));

        assert_eq!(params[1]["key"], json!("center"));
        assert_eq!(params[1]["required"], json!(false));

        assert_eq!(params[2]["key"], json!("translate"));
        assert_eq!(params[2]["required"], json!(false));

        assert_eq!(params[3]["key"], json!("op"));
        assert_eq!(params[3]["name"], json!("Op"));
        assert_eq!(params[3]["required"], json!(false));
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
        json!({
            "profiles": profile_query,
            "depth": { "value": depth_mm, "unit": "mm" },
            "profile_half_x": half_x,
            "profile_half_y": half_y,
        })
    }

    #[test]
    fn extrude_def_schema_has_every_runtime_type() {
        let schema = ExtrudeDef::schema_json();
        assert_eq!(schema["type"], json!("extrude"));
        let params = schema["params"].as_array().unwrap();
        assert_eq!(params.len(), 10);

        // Each key / type pair documents the K4.5.4f vocabulary
        // demonstrated in a single definition.
        let map: std::collections::HashMap<&str, &serde_json::Value> =
            params.iter().map(|p| (p["key"].as_str().unwrap(), p)).collect();
        assert_eq!(map["profiles"]["type"], json!("query"));
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
        // Face-extrude (P1): an optional multi-face source + direction modes.
        assert_eq!(map["faces"]["type"], json!("multi_query"));
        assert_eq!(map["faces"]["required"], json!(false));
        assert_eq!(map["symmetric"]["type"], json!("bool"));
        assert_eq!(map["symmetric"]["required"], json!(false));
        assert_eq!(map["depth_back"]["type"], json!("length"));
        assert_eq!(map["depth_back"]["required"], json!(false));
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
        doc.variables.insert("width".to_string(), 50.0);
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
        // Profile: a 4 × 20 mm rectangle at r=5, so revolving
        // it around the Y axis gives a tube.
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
                    }
                ]
            }),
        );
        let revolve_call = FeatureCall::new(
            "revolve",
            revolve_id.clone(),
            json!({
                "profiles": format!("qCreatedBy({sketch_id})"),
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
    fn revolve_errors_when_sketch_cannot_be_resolved() {
        let registry = FeatureRegistry::with_builtins();
        let mut doc = empty_ctx();
        let mut ctx = Context::new(&mut doc);
        let err = registry
            .regenerate(
                &mut ctx,
                &[FeatureCall::new(
                    "revolve",
                    id(1),
                    json!({
                        "profiles": "qCreatedBy(sketch_999)",
                    }),
                )],
                None,
            )
            .unwrap_err();
        assert!(matches!(
            err,
            FeatureError::MissingParameter {
                feature: "revolve",
                name: "profiles",
            }
        ));
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

        let refs = ctx.resolve_topo(&crate::queries::QueryExpr::Face(want.id_hash));
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
                            "profiles": "",
                            "faces": format!("qFace({hash})"),
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
                            "profiles": "",
                            "faces": format!("qFace({hash})"),
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
                            "profiles": "",
                            "faces": format!("qFace({top})"),
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
                "profiles": "",
                "faces": face_q,
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
            json!({ "radius": 3.0, "half_height": 5.0, "op": "remove" }),
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
    fn thicken_rejects_a_composite_non_analytic_carrier() {
        // A boolean carrier (union of two revolved blobs) is deferred past M0:
        // offsetting the whole boolean would thicken every surface, so a pick on
        // a non-analytic face whose source feature is composite returns a clear
        // error rather than silently-wrong geometry.
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
        let err = ThickenFeature.execute(&mut ctx, &id(2), &def).unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.contains("composite") || msg.contains("deferred"), "got: {msg}");
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
            let frame = crate::sketch_features::face_plane_frame(
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

        // Place a mate connector on that face; assert resolved CSYS.
        registry
            .regenerate(
                &mut ctx,
                &[FeatureCall::new(
                    "mate_connector",
                    id(2),
                    json!({ "face": format!("qFace({top_hash})"), "offset": [0.0, 0.0] }),
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

        // Offset shifts the origin in-plane (x_axis/y_axis), never along
        // the normal.
        ctx.mate_connectors.clear();
        registry
            .regenerate(
                &mut ctx,
                &[FeatureCall::new(
                    "mate_connector",
                    id(3),
                    json!({ "face": format!("qFace({top_hash})"), "offset": [3.0, 0.0] }),
                )],
                None,
            )
            .unwrap();
        let shifted = ctx.mate_connectors[0].origin;
        assert!((shifted[2] - 10.0).abs() < 1e-2, "offset stays in-plane (z unchanged)");
        let in_plane_shift = ((shifted[0] - mc_origin[0]).powi(2)
            + (shifted[1] - mc_origin[1]).powi(2))
        .sqrt();
        assert!((in_plane_shift - 3.0).abs() < 1e-2, "offset magnitude 3, got {in_plane_shift}");
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
        let revolve_call = FeatureCall::new("revolve", id(2), json!({ "profiles": "qCreatedBy(1)" }));
        (sketch_call, revolve_call)
    }

    /// Regenerates the canonical shelled vase (sketch + revolve + 2 mm shell
    /// opened on the planar top rim face) into `ctx`. The shell needs the
    /// woven top face's stable hash, so this pays one full weave.
    fn regen_shelled_vase(registry: &FeatureRegistry, ctx: &mut Context<'_>) {
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
        registry
            .regenerate(ctx, &[sketch_call, revolve_call, shell_call], None)
            .unwrap();
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
        let revolve_call = FeatureCall::new("revolve", id(2), json!({ "profiles": "qCreatedBy(1)" }));
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
                    "profiles": "qEdge(18417744930603108066)",
                    "faces": "qFace(4315924989883055031)",
                    "depth": { "value": 52.91257140451136, "unit": "mm" },
                    "depth_back": { "value": 0.0, "unit": "mm" },
                    "symmetric": false,
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
                "profiles": "",
                "faces": "qFace(6025886176426920981)",
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
}
