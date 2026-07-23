//! Query-/B-Rep-Resolution auf [`Context`]: `resolve_face_frame`,
//! `resolve_extrude_faces`, `resolve_fillet_edges`, `resolve_mate_anchor`
//! & Co. — aus dem features.rs-Monolithen ausgegliedert.

use fielddraft_protocol::{FeatureId, Transform};

/// One resolved fillet edge: the two junction operand features, their full
/// wall [`crate::csg::BranchId`] u32s (edge signature, with sub) and the
/// edge's world-space polyline for the mask tube.
///
/// #383/#406 hard cutter: when BOTH adjacent faces are analytic planes, their
/// world-space outward normals are captured (`plane_normal_a`/`_b`). Together
/// with the polyline endpoints this is everything the analytic fillet cutter
/// needs (see [`crate::csg::FilletCutter`]); a curved/tessellated neighbour
/// leaves the field `None` and the fillet stays on the `smin` mask path.
pub struct FilletEdgeHit {
    pub feature_a: FeatureId,
    pub feature_b: FeatureId,
    pub branch_a: u32,
    pub branch_b: u32,
    pub polyline: Vec<[f32; 3]>,
    /// World-space normal of the planar face on `branch_a`, if that face is a
    /// plane. `None` for a curved/tessellated face.
    pub plane_normal_a: Option<[f32; 3]>,
    /// World-space normal of the planar face on `branch_b`, if planar.
    pub plane_normal_b: Option<[f32; 3]>,
}

use crate::csg::{CsgNode};

use super::mate_connector::{face_normal_for_branch, polyline_point_tangent, MateAnchorFrame};
use super::transforms::compose_placement;
use super::thicken::thicken_diff_node;
use super::Context;
impl<'a> Context<'a> {
    /// C0/C1: resolve a face-yielding query to the first matching face,
    /// returning its [`SketchFrame`](crate::sketch::SketchFrame)
    /// in **world** space (the woven B-Rep is body-local; the paired
    /// placement is applied for us).
    ///
    /// Walks the woven solids in body order and, for each, scans for the
    /// faces the query selects (by `id_hash` for `qFace`, by branch
    /// feature id for `qCreatedBy`/`qFaceOf`), returning the first hit.
    pub fn resolve_face_frame(
        &mut self,
        expr: &crate::queries::QueryExpr,
    ) -> Option<crate::sketch::SketchFrame> {
        use crate::brep::FaceId;
        use crate::queries::QueryExpr;

        let woven = self.brep_cache.ensure(&self.features);
        if let QueryExpr::Face(eref) = expr {
            let (bi, fid) = locate_face(woven, eref)?;
            let (brep, placement) = &woven[bi];
            return Some(crate::sketch::face_plane_frame(brep, fid, placement));
        }
        for (brep, placement) in woven {
            for i in 0..brep.faces.len() {
                let fid = FaceId(i as u32);
                let hit = match expr {
                    QueryExpr::CreatedBy(f) | QueryExpr::FaceOf(f) => f
                        .as_u32_or_root()
                        .is_some_and(|t| brep.faces[i].branch_id.feature_id() == t),
                    _ => false,
                };
                if hit {
                    return Some(crate::sketch::face_plane_frame(
                        brep, fid, placement,
                    ));
                }
            }
        }
        // Every RICHER form (`qCreatedBy(op, ROLE)`, `qWall`, `qIntersection`,
        // `qAdjacentTo`, …) goes through the shared element resolver instead of
        // a second match arm here that would forever lag the query grammar.
        for (brep, placement) in woven {
            let refs = crate::queries::resolve_query_topo(std::slice::from_ref(brep), expr);
            let Some(r) = refs
                .iter()
                .find(|r| r.kind == crate::brep::naming::EntityKind::Face)
            else {
                continue;
            };
            if let Some(fid) = face_by_hash(brep, r.id_hash) {
                return Some(crate::sketch::face_plane_frame(brep, fid, placement));
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
        use crate::brep::{FaceId, SurfaceGeometry};
        use crate::queries::QueryExpr;

        let woven = self.brep_cache.ensure(&self.features);
        let located = match expr {
            QueryExpr::Face(eref) => Some(locate_face(woven, eref)?),
            _ => None,
        };
        // Richer forms (`qCreatedBy(op, ROLE)`, `qWall`, …) resolve through the
        // shared element resolver — same reasoning as `resolve_plane_frame`.
        // Without this a shell's `open_faces: qCreatedBy(op, CAP_TOP)` matched
        // nothing and the body silently stayed closed.
        let by_hash: Option<(usize, FaceId)> = match expr {
            QueryExpr::Face(_) | QueryExpr::CreatedBy(_) | QueryExpr::FaceOf(_) => None,
            _ => woven.iter().enumerate().find_map(|(bi, (brep, _))| {
                let refs = crate::queries::resolve_query_topo(std::slice::from_ref(brep), expr);
                let r = refs
                    .iter()
                    .find(|r| r.kind == crate::brep::naming::EntityKind::Face)?;
                face_by_hash(brep, r.id_hash).map(|fid| (bi, fid))
            }),
        };
        for (bi, (brep, placement)) in woven.iter().enumerate() {
            for i in 0..brep.faces.len() {
                let fid = FaceId(i as u32);
                let hit = match expr {
                    QueryExpr::Face(_) => located == Some((bi, fid)),
                    QueryExpr::CreatedBy(f) | QueryExpr::FaceOf(f) => f
                        .as_u32_or_root()
                        .is_some_and(|t| brep.faces[i].branch_id.feature_id() == t),
                    _ => by_hash == Some((bi, fid)),
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
    ) -> Result<(::fielddraft_protocol::Sketch2d, crate::sketch::SketchFrame), String> {
        use crate::brep::FaceId;
        use crate::queries::QueryExpr;

        let woven = self.brep_cache.ensure(&self.features);

        // Locate each requested face → (brep index, FaceId, world frame).
        let mut hits: Vec<(usize, FaceId, crate::sketch::SketchFrame)> = Vec::new();
        for q in queries {
            let expr = crate::queries::parse_query(q)
                .map_err(|e| format!("invalid face query `{q}`: {e:?}"))?;
            let QueryExpr::Face(eref) = expr else {
                return Err(format!(
                    "extrude faces must be `qFace(<id_hash>)` selections; got `{q}`"
                ));
            };
            let Some((bi, fid)) = locate_face(woven, &eref) else {
                return Err(format!(
                    "face {} not found in the current model",
                    eref.id_hash
                ));
            };
            let (brep, placement) = &woven[bi];
            // Extrude is a PLANAR operation by design (see the decision log in
            // GEOMETRY-KERNEL-CONCEPT.md §10): a curved face has no canonical
            // extrude profile, and the well-defined "grow a curved face"
            // operation is Thicken. Reject early with the actionable pointer
            // instead of failing later in the profile projection.
            let planar = matches!(
                &brep.face(fid).surface,
                crate::brep::SurfaceGeometry::Analytic {
                    surface: fielddraft_protocol::HalfSpace::Plane { .. },
                    ..
                }
            );
            if !planar {
                return Err(
                    "selected face is curved — Extrude needs a planar profile; use Thicken \
                     to grow a wall along a curved face's normal"
                        .to_string(),
                );
            }
            hits.push((bi, fid, crate::sketch::face_plane_frame(brep, fid, placement)));
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

    /// #385 P1: resolve N profile queries into an ordered list of
    /// [`LoftSection`]s plus the shared world [`SketchFrame`] the loft stacks
    /// along.
    ///
    /// Each query resolves like an extrude profile: a `qFace(...)` selection
    /// routes through [`Context::resolve_extrude_faces`] (single planar face),
    /// everything else through [`crate::sketch::resolve_profile_query`] (sketch
    /// region). The **loft axis** is the first resolved profile's plane normal;
    /// every other profile plane must be **parallel** to it — but EITHER
    /// direction is accepted (`|n_i·n_0| > 1 − 1e-6`). Anti-parallel profiles
    /// (planes facing each other, opposite normals) are supported since P6b:
    /// the P2 per-section `frame_map` is a full rigid 3D transform, so an
    /// anti-parallel section's map carries a π-rotation about an in-plane axis,
    /// which realises the required 2D mirror as a proper rotation (SDF values
    /// are isometry-invariant). Only genuinely tilted planes are rejected.
    /// Laterally-shifted planes are allowed. Sections are sorted by their signed
    /// projection onto the axis and their `offset` is expressed relative to the
    /// first (lowest) section, so `profiles[0].offset == 0`.
    ///
    /// The returned frame is the first *sorted* section's own frame (its
    /// in-plane axes, origin on its plane) with the loft axis as normal. When
    /// that first section is anti-parallel to the axis its own `(x, y)` basis is
    /// right-handed with `−axis`, so one in-plane axis is flipped to keep
    /// `(x, y, axis)` right-handed — otherwise `basis_to_quat` would emit a bad
    /// rotation. `frame.to_transform()` then places the section stack correctly
    /// with local +Z along the loft axis.
    ///
    /// Errors (returned as a message the caller wraps in a [`FeatureError`]):
    /// fewer than two resolvable profiles, a profile that resolves to no closed
    /// region, a non-parallel or opposing (flipped) plane, or two profiles on
    /// the same plane (duplicate offset).
    pub fn resolve_loft_profiles(
        &mut self,
        queries: &[String],
    ) -> Result<(Vec<::fielddraft_protocol::LoftSection>, crate::sketch::SketchFrame), String> {
        use crate::queries::QueryExpr;

        // 1. Resolve each query → (sketch, frame). A face routes through the
        //    face-extrude path; anything else through the sketch-region path.
        let mut resolved: Vec<(::fielddraft_protocol::Sketch2d, crate::sketch::SketchFrame)> =
            Vec::new();
        for q in queries {
            let is_face = matches!(crate::queries::parse_query(q), Ok(QueryExpr::Face(_)));
            if is_face {
                let (sketch, frame) = self.resolve_extrude_faces(std::slice::from_ref(q))?;
                resolved.push((sketch, frame));
            } else {
                match crate::sketch::resolve_profile_query(q, &self.sketches) {
                    Some((sketch, frame, _traced)) => resolved.push((sketch, frame)),
                    None => {
                        return Err(format!(
                            "loft profile `{q}` resolved no closed sketch region — draw a \
                             rectangle, circle or polygon first, or pick a planar face"
                        ));
                    }
                }
            }
        }

        if resolved.len() < 2 {
            return Err(format!(
                "a loft needs at least two profiles; got {}",
                resolved.len()
            ));
        }

        // 2. Loft axis = first profile's plane normal. Every other plane must be
        //    PARALLEL, but either direction is fine: a section whose normal is
        //    anti-parallel (`n·n0 ≈ −1`) is supported since P6b — its P2
        //    `frame_map` carries a π in-plane rotation that mirrors the profile
        //    into the shared frame as a proper rigid transform. Only genuinely
        //    tilted planes (`|n·n0| < 1 − 1e-6`) are rejected. Lateral shifts
        //    are fine.
        let axis = resolved[0].1.normal;
        let o0 = resolved[0].1.origin;
        let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        for (i, (_, f)) in resolved.iter().enumerate().skip(1) {
            let c = dot(f.normal, axis);
            if c.abs() <= 1.0 - 1e-6 {
                return Err(format!(
                    "loft profile #{i} is not parallel to the first profile's plane \
                     (tilted profiles are not supported yet; n·n0 = {c:.6})"
                ));
            }
        }

        // 3. Signed projection of each profile origin onto the axis, relative to
        //    the first resolved profile. Sort ascending along the axis.
        let mut with_key: Vec<(::fielddraft_protocol::Sketch2d, crate::sketch::SketchFrame, f32)> =
            resolved
                .into_iter()
                .map(|(s, f)| {
                    let d = [f.origin[0] - o0[0], f.origin[1] - o0[1], f.origin[2] - o0[2]];
                    let key = dot(d, axis);
                    (s, f, key)
                })
                .collect();
        with_key.sort_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal));

        // 4. Reject two profiles on the same plane (duplicate offset).
        for w in with_key.windows(2) {
            if (w[1].2 - w[0].2).abs() < 1e-4 {
                return Err(
                    "two loft profiles lie on the same plane — profiles must sit on distinct \
                     parallel planes"
                        .to_string(),
                );
            }
        }

        // 5. Baseline offsets to the first sorted section and build the section
        //    list. The shared frame keeps the first resolved profile's in-plane
        //    basis and axis normal, with the origin lifted onto the first sorted
        //    section's plane so local Z=0 sits at `profiles[0]`.
        let base_key = with_key[0].2;
        let ref0 = &with_key[0].1; // first sorted section's own frame
        // The shared frame's normal is `axis` (the offset-measuring direction).
        // Its in-plane basis is taken from the first sorted section, but if that
        // section is anti-parallel to `axis` its `(x, y)` pair is right-handed
        // with `−axis`; pairing it with `+axis` would be left-handed and make
        // `basis_to_quat` (which assumes a proper rotation) emit garbage. Flip
        // one in-plane axis in that case to keep `(x, y, axis)` right-handed.
        // The final world geometry is invariant to this choice (the per-section
        // `frame_map` = inverse(inverse(B_i) ∘ L) cancels L), so we only need a
        // valid right-handed L; co-directional lofts keep ref0's basis verbatim.
        let (fx, fy) = if dot(ref0.normal, axis) > 0.0 {
            (ref0.x_axis, ref0.y_axis)
        } else {
            (ref0.x_axis, [-ref0.y_axis[0], -ref0.y_axis[1], -ref0.y_axis[2]])
        };
        let frame = crate::sketch::SketchFrame {
            origin: ref0.origin,
            x_axis: fx,
            y_axis: fy,
            normal: axis,
        };
        // P2 reframing: `frame` is the shared loft frame (loft-local → world is
        // `L = frame.to_transform()`). Each section keeps its own 2D basis, so
        // its field must be re-expressed in the shared frame for the morph. The
        // per-section map `M_i` is the rigid transform whose `world_to_local`
        // takes a loft-local point into section `i`'s own `(u, v)` plane:
        //
        //   world_to_local(M_i, x) = world_to_local(B_i, to_world(L, x))
        //
        // with `B_i = f_i.to_transform()` (section-own → world). Since
        // `world_to_local(t, ·) == to_world(inverse(t), ·)` for a rigid
        // transform, this composes to `M_i = inverse(inverse(B_i) ∘ L)`. For
        // the first sorted section `M_0` is (numerically) the identity.
        let l = frame.to_transform();
        let sections: Vec<::fielddraft_protocol::LoftSection> = with_key
            .iter()
            .map(|(s, f_i, key)| {
                let b_i = f_i.to_transform();
                let frame_map =
                    crate::inverse(&crate::compose_transforms(&crate::inverse(&b_i), &l));
                ::fielddraft_protocol::LoftSection {
                    sketch: s.clone(),
                    offset: key - base_key,
                    frame_map,
                }
            })
            .collect();
        Ok((sections, frame))
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
        use crate::queries::QueryExpr;

        // Locate the woven body (partition order) that carries any picked face.
        let mut owner_bi: Option<usize> = None;
        {
            let woven = self.brep_cache.ensure(&self.features);
            for q in queries {
                let Ok(QueryExpr::Face(eref)) = crate::queries::parse_query(q) else {
                    continue;
                };
                if let Some((bi, _)) = locate_face(woven, &eref) {
                    owner_bi = Some(bi);
                    break;
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
        
        use crate::brep::SurfaceGeometry;
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
                let QueryExpr::Face(eref) = expr else {
                    return Err(format!(
                        "thicken faces must be `qFace(<id_hash>)` selections; got `{q}`"
                    ));
                };
                let Some((bi, fid)) = locate_face(woven, &eref) else {
                    return Err(format!(
                        "face {} not found in the current model",
                        eref.id_hash
                    ));
                };
                let (brep, placement) = &woven[bi];
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
                        loops: crate::brep::face_lateral_trim::sample_face_loops(brep, fid),
                    }),
                    SurfaceGeometry::Analytic { .. } => {
                        let s = crate::brep::face_thicken::thicken_face(
                            brep, fid, placement, outer_off, inner_off,
                        )?;
                        walls.push((qi, thicken_diff_node(id, qi, &s), s.world));
                    }
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
            // Composite (boolean) owner nodes descend to the ONE leaf whose
            // surface holds the picked face, probed at the face's own loop
            // samples in the feature-local frame (see
            // [`crate::brep::face_offset::face_carrier`]). Shell/Offset
            // owners have no single closed carrier and stay an honest error.
            let inv_feature = crate::inverse(&source.transform);
            let samples_local: Vec<[f32; 3]> = req
                .loops
                .outer
                .iter()
                .chain(req.loops.inner.iter().flatten())
                .step_by((req.loops.outer.len() / 16).max(1))
                .map(|p| {
                    place_point(&inv_feature, [p[0] as f32, p[1] as f32, p[2] as f32])
                })
                .collect();
            let probe_tol = {
                // Scale-relative: a face sample sits on its carrier to weave
                // noise; the boolean partner reads a feature-scale distance.
                let diag = samples_local
                    .iter()
                    .flat_map(|p| p.iter())
                    .fold(0.0_f32, |m, &c| m.max(c.abs()));
                (diag * 1e-3).max(1e-3)
            };
            let carrier = crate::brep::face_offset::face_carrier(
                &source.node,
                &samples_local,
                probe_tol,
            )
            .ok_or_else(|| {
                "face's carrier could not be isolated from its composite (boolean) body — \
                 a Shell/Offset owner has no single closed carrier to offset"
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
    /// same-kind surface (mismatched kinds, differing cone half-angles and
    /// non-coaxial targets are rejected with a descriptive message).
    pub fn resolve_thicken_to_face(
        &mut self,
        id: &FeatureId,
        face_queries: &[String],
        target_query: &str,
    ) -> Result<Vec<(CsgNode, Transform)>, String> {
        use crate::brep::FaceId;
        use crate::queries::QueryExpr;

        let woven = self.brep_cache.ensure(&self.features);

        // Locate a `qFace(<hash>)` selection → (body index, FaceId).
        let find_face = |q: &str| -> Result<(usize, FaceId), String> {
            let expr = crate::queries::parse_query(q)
                .map_err(|e| format!("invalid face query `{q}`: {e:?}"))?;
            let QueryExpr::Face(eref) = expr else {
                return Err(format!("thicken faces must be `qFace(<id_hash>)` selections; got `{q}`"));
            };
            locate_face(woven, &eref).ok_or_else(|| {
                format!("face {} not found in the current model", eref.id_hash)
            })
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
    ) -> Option<crate::sketch::SketchFrame> {
        if let crate::queries::QueryExpr::CreatedBy(d) = expr {
            if let Some(datum) = crate::sketch::datum_plane_frame(d) {
                return Some(datum);
            }
        }
        self.resolve_face_frame(expr)
    }

    /// DL-61: resolve an edge-yielding query into per-edge fillet inputs
    /// ([`FilletEdgeHit`]: operand features, full wall branch ids and the
    /// world-space polyline).
    ///
    /// COORDINATE SPACE: the woven B-Reps are **body-local** (DL-56); the
    /// paired `placement` maps body-local → world. The CSG tree that the
    /// MaskedBlend lives in evaluates in **world** space — every leaf is
    /// wrapped in `CsgNode::Transform(feature.transform, …)` by
    /// [`from_decoded`](crate::csg::CsgNode::from_decoded), and a feature's
    /// `transform` is its world placement. So the mask must sit on the
    /// edge in world space: we apply `placement` (rotate·scale + translate,
    /// same math as [`crate::sketch::face_plane_frame`]) to every
    /// polyline point. The non-identity-placement test proves this — with
    /// identity placement a missing transform would pass undetected.
    ///
    /// The feature pair comes from the edge's branch-pair `signature`:
    /// each [`BranchId`](crate::csg::BranchId)'s `feature_id()` is exactly
    /// the junction operand id [`find_matching_fillet`] keys on.
    pub fn resolve_fillet_edges(
        &mut self,
        expr: &crate::queries::QueryExpr,
    ) -> Vec<FilletEdgeHit> {
        use crate::brep::{EdgeGeometry, EdgeId};

        let woven = self.brep_cache.ensure(&self.features);
        let located = match expr {
            crate::queries::QueryExpr::Edge(eref) => match locate_edge(woven, eref) {
                Some(hit) => Some(hit),
                None => return Vec::new(),
            },
            _ => None,
        };
        let by_hash = resolved_edge_hashes(woven, expr);

        let mut out = Vec::new();
        for (bi, (brep, placement)) in woven.iter().enumerate() {
            for i in 0..brep.edges.len() {
                let id = EdgeId(i as u32);
                let edge = brep.edge(id);
                // Match the edge against the query. `qEdge(hash)` matches
                // via the shared healing-aware locator; `qEdgeBetween(a, b)`
                // matches by the (unordered) signature feature pair.
                let hit = match expr {
                    crate::queries::QueryExpr::Edge(_) => located == Some((bi, id)),
                    crate::queries::QueryExpr::EdgeBetween(a, b) => {
                        let s0 = edge.signature.0.feature_id();
                        let s1 = edge.signature.1.feature_id();
                        let fa = a.as_u32_or_root();
                        let fb = b.as_u32_or_root();
                        matches!((fa, fb), (Some(fa), Some(fb))
                            if (s0 == fa && s1 == fb) || (s0 == fb && s1 == fa))
                    }
                    _ => by_hash
                        .contains(&crate::brep::naming::TopoRef::from_edge(brep, id).id_hash),
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
                // #383/#406: capture the two adjacent faces' world normals when
                // they are analytic planes — the analytic hard cutter's inputs.
                let plane_normal_a = planar_face_normal(brep, placement, edge.signature.0);
                let plane_normal_b = planar_face_normal(brep, placement, edge.signature.1);
                out.push(FilletEdgeHit {
                    feature_a,
                    feature_b,
                    branch_a: edge.signature.0.as_u32(),
                    branch_b: edge.signature.1.as_u32(),
                    polyline: world,
                    plane_normal_a,
                    plane_normal_b,
                });
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
        use crate::brep::EdgeId;

        let woven = self.brep_cache.ensure(&self.features);
        let located = match expr {
            crate::queries::QueryExpr::Edge(eref) => match locate_edge(woven, eref) {
                Some(hit) => Some(hit),
                None => return Vec::new(),
            },
            _ => None,
        };
        let by_hash = resolved_edge_hashes(woven, expr);
        let mut out = Vec::new();
        for (bi, (brep, placement)) in woven.iter().enumerate() {
            for i in 0..brep.edges.len() {
                let id = EdgeId(i as u32);
                let edge = brep.edge(id);
                let hit = match expr {
                    crate::queries::QueryExpr::Edge(_) => located == Some((bi, id)),
                    crate::queries::QueryExpr::EdgeBetween(a, b) => {
                        let s0 = edge.signature.0.feature_id();
                        let s1 = edge.signature.1.feature_id();
                        let fa = a.as_u32_or_root();
                        let fb = b.as_u32_or_root();
                        matches!((fa, fb), (Some(fa), Some(fb))
                            if (s0 == fa && s1 == fb) || (s0 == fb && s1 == fa))
                    }
                    _ => by_hash
                        .contains(&crate::brep::naming::TopoRef::from_edge(brep, id).id_hash),
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
                use crate::brep::EdgeId;
                let woven = self.brep_cache.ensure(&self.features);
                let located = match expr {
                    QueryExpr::Edge(eref) => match locate_edge(woven, eref) {
                        Some(hit) => Some(hit),
                        None => return None,
                    },
                    _ => None,
                };
                for (bi, (brep, placement)) in woven.iter().enumerate() {
                    for i in 0..brep.edges.len() {
                        let eid = EdgeId(i as u32);
                        let edge = brep.edge(eid);
                        let hit = match expr {
                            QueryExpr::Edge(_) => located == Some((bi, eid)),
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
            QueryExpr::Vertex(eref) => {
                use crate::brep::naming::EntityHandle;
                let woven = self.brep_cache.ensure(&self.features);
                let (bi, handle) = crate::queries::locate_element(
                    woven.iter().map(|(b, _)| b),
                    crate::brep::naming::EntityKind::Vertex,
                    eref,
                )?;
                let EntityHandle::Vertex(vid) = handle else { return None };
                let (brep, placement) = &woven[bi];
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
                Some(MateAnchorFrame {
                    origin,
                    primary: None,
                    auto_secondary,
                })
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

/// Two world [`SketchFrame`](crate::sketch::SketchFrame)s are coplanar
/// iff their normals are (anti)parallel and the second origin lies in the first
/// frame's plane. Used to gate a coplanar multi-face extrude selection.
fn coplanar(
    a: &crate::sketch::SketchFrame,
    b: &crate::sketch::SketchFrame,
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


/// The id_hashes a RICHER edge query (`qCommonEdge(a, b)`, …) resolves to.
/// The two native forms (`qEdge`, `qEdgeBetween`) keep their own fast paths;
/// everything else goes through the shared element resolver, so a new query
/// form works in every edge consumer the day it parses — instead of silently
/// resolving to nothing until each consumer grows another match arm.
fn resolved_edge_hashes(
    woven: &[(crate::brep::BRep, fielddraft_protocol::Transform)],
    expr: &crate::queries::QueryExpr,
) -> std::collections::HashSet<u64> {
    use crate::queries::QueryExpr;
    if matches!(expr, QueryExpr::Edge(_) | QueryExpr::EdgeBetween(_, _)) {
        return Default::default();
    }
    woven
        .iter()
        .flat_map(|(brep, _)| {
            crate::queries::resolve_query_topo(std::slice::from_ref(brep), expr)
        })
        .filter(|r| r.kind == crate::brep::naming::EntityKind::Edge)
        .map(|r| r.id_hash)
        .collect()
}

/// The face of `brep` carrying `id_hash` — the bridge from a resolved
/// [`TopoRef`](crate::brep::naming::TopoRef) back to an arena handle.
fn face_by_hash(brep: &crate::brep::BRep, id_hash: u64) -> Option<crate::brep::FaceId> {
    use crate::brep::naming::TopoRef;
    (0..brep.faces.len())
        .map(|i| crate::brep::FaceId(i as u32))
        .find(|&fid| TopoRef::from_face(brep, fid).id_hash == id_hash)
}

/// Locate a `qFace` element reference across the woven bodies —
/// fingerprint-healing included (see [`crate::queries::locate_element`]).
fn locate_face(
    woven: &[(crate::brep::BRep, fielddraft_protocol::Transform)],
    eref: &crate::queries::ElementRef,
) -> Option<(usize, crate::brep::FaceId)> {
    use crate::brep::naming::{EntityHandle, EntityKind};
    match crate::queries::locate_element(woven.iter().map(|(b, _)| b), EntityKind::Face, eref)? {
        (bi, EntityHandle::Face(fid)) => Some((bi, fid)),
        _ => None,
    }
}

/// Locate a `qEdge` element reference across the woven bodies.
fn locate_edge(
    woven: &[(crate::brep::BRep, fielddraft_protocol::Transform)],
    eref: &crate::queries::ElementRef,
) -> Option<(usize, crate::brep::EdgeId)> {
    use crate::brep::naming::{EntityHandle, EntityKind};
    match crate::queries::locate_element(woven.iter().map(|(b, _)| b), EntityKind::Edge, eref)? {
        (bi, EntityHandle::Edge(eid)) => Some((bi, eid)),
        _ => None,
    }
}

/// Apply a body `placement` (rotate·scale then translate) to a point.
/// Mirrors the centroid mapping in
/// [`crate::sketch::face_plane_frame`].
pub(crate) fn place_point(placement: &fielddraft_protocol::Transform, p: [f32; 3]) -> [f32; 3] {
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

/// #383/#406: world-space normal of the PLANAR face on `branch`, or `None` if
/// no face on that branch is an analytic plane. Unlike
/// [`face_normal_for_branch`] this refuses curved/tessellated faces — the hard
/// fillet cutter is only defined between two planes.
fn planar_face_normal(
    brep: &crate::brep::BRep,
    placement: &fielddraft_protocol::Transform,
    branch: crate::csg::BranchId,
) -> Option<[f32; 3]> {
    use crate::brep::SurfaceGeometry;
    use fielddraft_protocol::HalfSpace;
    for i in 0..brep.faces.len() {
        if brep.faces[i].branch_id == branch {
            if let SurfaceGeometry::Analytic { surface: HalfSpace::Plane { .. }, .. } =
                &brep.faces[i].surface
            {
                return Some(
                    crate::sketch::face_plane_frame(brep, crate::brep::FaceId(i as u32), placement)
                        .normal,
                );
            }
        }
    }
    None
}
