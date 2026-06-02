//! Reach For The Arcs (Sellán et al., SIGGRAPH 2024) — full port.
//!
//! Pipeline: SDF grid → spheres → normalize → sign separation →
//! rasterize → locally_make_feasible → merge → fine-tune → sPSR → mesh.

mod closest_point;
mod feasibility;
mod finetune;
mod poisson;
mod rasterize;
mod sphere_tree;

use crate::bvh::BoundedCsg;
use crate::csg::{EvalMode, MAX_CONTRIBUTORS};
use crate::dc::{sdf_normal, Mesh, CONTRIBUTOR_SENTINEL};
use feasibility::TangentPoint;
use sphere_tree::{SdfSphere, SphereAabbTree};

/// Upper bound on RFTA's internal SDF sampling resolution.
///
/// RFTA samples the SDF on a grid and emits one sphere per corner, so the
/// sphere/tangent-point count grows as `res³`. Those points feed sPSR (whose
/// unoptimized solver is super-linear) and the fine-tuning loop, so an
/// uncapped display resolution of 32/64 would push the count to 30k–270k and
/// stall the worker for minutes. RFTA is a sampled-grid method by design —
/// the reference runs on fixed grids — so capping the *internal* sampling
/// grid is a legitimate parameter, decoupled from the display resolution.
/// Output detail is still governed by the sPSR octree depth.
const MAX_SAMPLE_RES: u32 = 16;

pub fn reach_for_the_arcs(node: &BoundedCsg, resolution: u32) -> Mesh {
    if resolution == 0 {
        return Mesh::default();
    }
    let raw = node.aabb();
    if raw.is_empty() {
        return Mesh::default();
    }

    // Decouple the internal sampling grid from the display resolution.
    let sample_res = resolution.min(MAX_SAMPLE_RES);

    let raw_size = raw.size();
    let max_dim = raw_size[0].max(raw_size[1]).max(raw_size[2]);
    if max_dim <= 0.0 {
        return Mesh::default();
    }
    let cell = max_dim / sample_res as f32;
    // RFTA does not need grid expansion (unlike MDC/DC which need cells
    // on both sides of the boundary). The tight AABB already covers the
    // object. Expansion creates huge spheres far from the surface whose
    // feasibility arcs face away from the true surface.
    let grid = raw;
    let size = grid.size();

    let nx = ((size[0] / cell).ceil() as usize).max(1);
    let ny = ((size[1] / cell).ceil() as usize).max(1);
    let nz = ((size[2] / cell).ceil() as usize).max(1);

    let cnx = nx + 1;
    let cny = ny + 1;
    let cnz = nz + 1;

    let corner_point = |i: usize, j: usize, k: usize| -> [f32; 3] {
        [
            grid.min[0] + i as f32 * cell,
            grid.min[1] + j as f32 * cell,
            grid.min[2] + k as f32 * cell,
        ]
    };

    // ── 1. Sample SDF at grid corners ──────────────────────────────

    let mut sdf_vals = vec![0.0_f32; cnx * cny * cnz];
    for k in 0..cnz {
        for j in 0..cny {
            for i in 0..cnx {
                let idx = i + j * cnx + k * cnx * cny;
                sdf_vals[idx] = node.sdf(corner_point(i, j, k), EvalMode::Visual);
            }
        }
    }

    // ── 2. Build SDF spheres from grid corners ─────────────────────

    let tol = 1e-10_f32;
    let mut spheres_raw: Vec<(SdfSphere, [f32; 3])> = Vec::with_capacity(cnx * cny * cnz);
    for k in 0..cnz {
        for j in 0..cny {
            for i in 0..cnx {
                let idx = i + j * cnx + k * cnx * cny;
                let d = sdf_vals[idx];
                if d.abs() < tol {
                    continue;
                }
                let p = corner_point(i, j, k);
                spheres_raw.push((
                    SdfSphere {
                        center: p,  // will be normalized below
                        radius: d.abs(),
                        positive: d > 0.0,
                    },
                    p, // original position for un-normalization
                ));
            }
        }
    }

    if spheres_raw.is_empty() {
        return Mesh::default();
    }

    // ── 3. Normalize to [0,1]³ ─────────────────────────────────────

    let (norm_offset, norm_scale) = compute_normalization(&spheres_raw);
    let inv_scale = 1.0 / norm_scale;

    let spheres: Vec<SdfSphere> = spheres_raw
        .iter()
        .map(|(s, _)| SdfSphere {
            center: normalize_point(s.center, norm_offset, inv_scale),
            radius: s.radius * inv_scale,
            positive: s.positive,
        })
        .collect();

    // ── 4. Sign separation: process positive and negative separately ──

    // Split by sign
    let (pos_indices, neg_indices) = split_by_sign(&spheres);

    let mut all_tangent_points: Vec<TangentPoint> = Vec::new();

    for (sign_indices, positive) in [(&pos_indices, true), (&neg_indices, false)] {
        if sign_indices.is_empty() {
            continue;
        }

        // Build subset of spheres for this sign class
        let subset: Vec<SdfSphere> = sign_indices
            .iter()
            .map(|&i| SdfSphere {
                center: spheres[i].center,
                radius: spheres[i].radius,
                positive: spheres[i].positive,
            })
            .collect();

        let sub_tree = SphereAabbTree::build(&subset);

        // Rasterize: find outside points
        let outside_pts = rasterize::outside_points(&subset, &sub_tree);

        // Locally make feasible: compute initial tangent points
        let tangent_pts = feasibility::locally_make_feasible(
            &subset,
            &sub_tree,
            &outside_pts,
            positive,
        );

        // Convert local indices to global and collect
        for (local_idx, tp_opt) in tangent_pts.into_iter().enumerate() {
            if let Some(tp) = tp_opt {
                all_tangent_points.push(TangentPoint {
                    position: tp.position,
                    normal: tp.normal,
                    sphere_idx: sign_indices[local_idx],
                });
            }
        }
    }

    if all_tangent_points.is_empty() {
        return Mesh::default();
    }

    // ── 5. Initial sPSR → mesh ─────────────────────────────────────

    let (points, normals) = tangent_points_to_arrays(&all_tangent_points);

    let psr_depth = poisson::depth_for_resolution(sample_res);

    let (mut psr_verts, mut psr_tris) = match poisson::reconstruct(&points, &normals, psr_depth) {
        Some(m) => m,
        None => return Mesh::default(),
    };

    // ── 6. Fine-tuning loop ────────────────────────────────────────

    // Fine-tuning re-runs sPSR each iteration and adds tangent points, so its
    // cost grows super-linearly. Keyed off the (capped) sampling resolution and
    // kept low — the initial reconstruction is already a valid surface; these
    // iterations only refine it onto the evolving mesh.
    let k_max = if sample_res <= 8 { 0 } else { 1 };

    // We need per-sign trees for fine-tuning feasibility checks.
    // Build a combined tree for all spheres.
    let full_tree = SphereAabbTree::build(&spheres);

    for _iter in 0..k_max {
        // Fine-tune positive tangent points
        let mut pos_tps: Vec<TangentPoint> = all_tangent_points
            .iter()
            .filter(|tp| tp.sphere_idx < spheres.len() && spheres[tp.sphere_idx].positive)
            .copied()
            .collect();

        let pos_changed = if !pos_tps.is_empty() {
            finetune::finetune_iteration(
                &spheres,
                &full_tree,
                &mut pos_tps,
                &psr_verts,
                &psr_tris,
                true,
            )
        } else {
            false
        };

        // Fine-tune negative tangent points
        let mut neg_tps: Vec<TangentPoint> = all_tangent_points
            .iter()
            .filter(|tp| tp.sphere_idx < spheres.len() && !spheres[tp.sphere_idx].positive)
            .copied()
            .collect();

        let neg_changed = if !neg_tps.is_empty() {
            finetune::finetune_iteration(
                &spheres,
                &full_tree,
                &mut neg_tps,
                &psr_verts,
                &psr_tris,
                false,
            )
        } else {
            false
        };

        if !pos_changed && !neg_changed {
            break;
        }

        // Merge back
        all_tangent_points.clear();
        all_tangent_points.extend(pos_tps);
        all_tangent_points.extend(neg_tps);

        // Reconstruct mesh
        let (pts, nrm) = tangent_points_to_arrays(&all_tangent_points);
        if pts.is_empty() {
            break;
        }

        match poisson::reconstruct(&pts, &nrm, psr_depth) {
            Some((v, t)) => {
                psr_verts = v;
                psr_tris = t;
            }
            None => break,
        }
    }

    // ── 7. Un-normalize coordinates ────────────────────────────────

    let vc = psr_verts.len() / 3;
    for i in 0..vc {
        psr_verts[i * 3] = psr_verts[i * 3] * norm_scale + norm_offset[0];
        psr_verts[i * 3 + 1] = psr_verts[i * 3 + 1] * norm_scale + norm_offset[1];
        psr_verts[i * 3 + 2] = psr_verts[i * 3 + 2] * norm_scale + norm_offset[2];
    }

    // ── 8. Trim sPSR mesh: remove faces far from the zero isosurface ──
    // Remove triangles where any vertex's SDF exceeds the grid cell size.
    let trim_threshold = cell;

    let (trimmed_positions, trimmed_indices) =
        trim_mesh(&psr_verts, &psr_tris, node, trim_threshold);

    // ── 9. Convert to output Mesh ──────────────────────────────────

    let vc = trimmed_positions.len() / 3;
    let normal_eps = cell * 0.25;
    let contributor_eps = cell * 0.5;
    let positions = trimmed_positions;
    let mut normals_out = Vec::with_capacity(vc * 3);
    let mut classifications = Vec::with_capacity(vc);
    let mut contributors = Vec::with_capacity(vc * MAX_CONTRIBUTORS);

    for i in 0..vc {
        let p = [
            positions[i * 3],
            positions[i * 3 + 1],
            positions[i * 3 + 2],
        ];
        let n = sdf_normal(node, p, normal_eps);
        normals_out.extend_from_slice(&n);

        let (_, cs) = node.evaluate(p, contributor_eps, EvalMode::Visual);
        // Mesh-level proxy works at feature granularity; the per-face
        // sub-ids stay in the analytic B-Rep path.
        let cs = cs.feature_collapsed();
        let mut element = cs.element();
        if element == crate::csg::ProxyElement::None {
            element = crate::csg::ProxyElement::Face;
        }
        let slice = cs.as_slice();
        let mut contribs = [CONTRIBUTOR_SENTINEL; MAX_CONTRIBUTORS];
        for slot in 0..MAX_CONTRIBUTORS {
            if slot < slice.len() {
                contribs[slot] = slice[slot].feature_id();
            }
        }
        classifications.push(element.tag());
        contributors.extend_from_slice(&contribs);
    }

    Mesh {
        positions,
        normals: normals_out,
        indices: trimmed_indices,
        classifications,
        contributors,
        face_index: Vec::new(),
    }
}

/// Compute normalization parameters to map all sphere centers+radii into [0,1]³.
fn compute_normalization(spheres: &[(SdfSphere, [f32; 3])]) -> ([f32; 3], f32) {
    let mut amin = [f32::INFINITY; 3];
    let mut amax = [f32::NEG_INFINITY; 3];
    for (s, _) in spheres {
        for a in 0..3 {
            amin[a] = amin[a].min(s.center[a] - s.radius);
            amax[a] = amax[a].max(s.center[a] + s.radius);
        }
    }
    let extent = [amax[0] - amin[0], amax[1] - amin[1], amax[2] - amin[2]];
    let scale = extent[0].max(extent[1]).max(extent[2]).max(1e-10);
    (amin, scale)
}

/// Trim sPSR mesh by removing triangles whose centroid is far from the isosurface.
/// Returns (compacted positions, compacted indices).
fn trim_mesh(
    verts: &[f32],
    tris: &[u32],
    node: &BoundedCsg,
    threshold: f32,
) -> (Vec<f32>, Vec<u32>) {
    let vc = verts.len() / 3;
    let tc = tris.len() / 3;

    let mut keep_face = vec![false; tc];
    let mut vertex_used = vec![false; vc];

    // Evaluate SDF at all vertices once
    let mut vert_sdf = vec![0.0f32; vc];
    for i in 0..vc {
        let p = [verts[i * 3], verts[i * 3 + 1], verts[i * 3 + 2]];
        vert_sdf[i] = node.sdf(p, EvalMode::Visual).abs();
    }

    for fi in 0..tc {
        let i0 = tris[fi * 3] as usize;
        let i1 = tris[fi * 3 + 1] as usize;
        let i2 = tris[fi * 3 + 2] as usize;
        let max_sdf = vert_sdf[i0].max(vert_sdf[i1]).max(vert_sdf[i2]);
        if max_sdf <= threshold {
            keep_face[fi] = true;
            vertex_used[i0] = true;
            vertex_used[i1] = true;
            vertex_used[i2] = true;
        }
    }

    let mut remap = vec![u32::MAX; vc];
    let mut new_verts = Vec::new();
    let mut new_idx = 0u32;
    for i in 0..vc {
        if vertex_used[i] {
            remap[i] = new_idx;
            new_verts.push(verts[i * 3]);
            new_verts.push(verts[i * 3 + 1]);
            new_verts.push(verts[i * 3 + 2]);
            new_idx += 1;
        }
    }

    let mut new_tris = Vec::new();
    for fi in 0..tc {
        if keep_face[fi] {
            new_tris.push(remap[tris[fi * 3] as usize]);
            new_tris.push(remap[tris[fi * 3 + 1] as usize]);
            new_tris.push(remap[tris[fi * 3 + 2] as usize]);
        }
    }

    (new_verts, new_tris)
}

fn normalize_point(p: [f32; 3], offset: [f32; 3], inv_scale: f32) -> [f32; 3] {
    [
        (p[0] - offset[0]) * inv_scale,
        (p[1] - offset[1]) * inv_scale,
        (p[2] - offset[2]) * inv_scale,
    ]
}

fn split_by_sign(spheres: &[SdfSphere]) -> (Vec<usize>, Vec<usize>) {
    let mut pos = Vec::new();
    let mut neg = Vec::new();
    for (i, s) in spheres.iter().enumerate() {
        if s.positive {
            pos.push(i);
        } else {
            neg.push(i);
        }
    }
    (pos, neg)
}

fn tangent_points_to_arrays(tps: &[TangentPoint]) -> (Vec<[f32; 3]>, Vec<[f32; 3]>) {
    let points: Vec<[f32; 3]> = tps.iter().map(|tp| tp.position).collect();
    let normals: Vec<[f32; 3]> = tps.iter().map(|tp| tp.normal).collect();
    (points, normals)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::csg::{CsgNode, DecodedFeature, PrimitiveModifiers};
    use fielddraft_protocol::{BoolOp, FeatureId, Primitive, Transform};

    fn make_feature(id: u32, prim: Primitive) -> DecodedFeature {
        DecodedFeature::from_parts(
            FeatureId::from_u32(id),
            String::new(),
            BoolOp::Union,
            prim,
            Transform::IDENTITY,
            PrimitiveModifiers::NONE,
            vec![],
        )
    }

    fn bounded_sphere(center: [f32; 3], radius: f32) -> crate::bvh::BoundedCsg {
        let f = make_feature(1, Primitive::Sphere { center, radius });
        let csg = CsgNode::from_decoded(&[f]);
        crate::bvh::BoundedCsg::from_csg(&csg)
    }

    #[test]
    fn rfta_sphere_produces_mesh() {
        let node = bounded_sphere([0.0; 3], 20.0);
        let mesh = reach_for_the_arcs(&node, 4);
        assert!(mesh.vertex_count() > 0);
        assert!(mesh.triangle_count() > 0);
        assert_eq!(mesh.indices.len() % 3, 0);
    }

    #[test]
    fn rfta_sphere_vertices_near_surface() {
        let f = make_feature(1, Primitive::Sphere { center: [0.0; 3], radius: 20.0 });
        let csg = CsgNode::from_decoded(&[f]);
        let node = crate::bvh::BoundedCsg::from_csg(&csg);
        let mesh = reach_for_the_arcs(&node, 4);
        let mut max_err = 0.0_f32;
        for i in 0..mesh.vertex_count() {
            let p = [
                mesh.positions[i * 3],
                mesh.positions[i * 3 + 1],
                mesh.positions[i * 3 + 2],
            ];
            let d = csg.sdf(p, EvalMode::Logical).abs();
            if d > max_err {
                max_err = d;
            }
        }
        assert!(
            max_err < 10.0,
            "RFTA sphere vertices drifted off surface by {max_err}",
        );
    }

    #[test]
    fn rfta_box_terminates_and_meshes() {
        // Regression guard for the res-32/64 hang: a solid box at a high
        // display resolution must still terminate quickly (internal sampling
        // is capped) and produce a non-trivial mesh.
        let f = make_feature(1, Primitive::Box { center: [0.0; 3], half_extents: [50.0, 50.0, 50.0] });
        let csg = CsgNode::from_decoded(&[f]);
        let node = crate::bvh::BoundedCsg::from_csg(&csg);
        let mesh = reach_for_the_arcs(&node, 64);
        assert!(mesh.triangle_count() > 100, "box produced too few triangles");
        assert_eq!(mesh.indices.len() % 3, 0);
    }

    #[test]
    fn rfta_empty_scene() {
        let mesh = reach_for_the_arcs(&crate::bvh::BoundedCsg::Empty, 4);
        assert!(mesh.is_empty());
    }

    #[test]
    fn rfta_differs_from_mdc() {
        let prim = Primitive::Sphere { center: [0.0; 3], radius: 20.0 };
        let f = make_feature(1, prim.clone());
        let csg = CsgNode::from_decoded(&[f]);
        let node = crate::bvh::BoundedCsg::from_csg(&csg);
        let prims: std::collections::HashMap<u32, &Primitive> =
            [(1, &prim)].into_iter().collect();

        let rfta_mesh = reach_for_the_arcs(&node, 8);
        let mdc_mesh = crate::dc::dual_contour_with_primitives(&node, 8, &prims);

        assert!(rfta_mesh.vertex_count() > 0, "RFTA produced empty mesh");
        assert!(mdc_mesh.vertex_count() > 0, "MDC produced empty mesh");
        assert_ne!(
            rfta_mesh.vertex_count(),
            mdc_mesh.vertex_count(),
            "RFTA ({} verts, {} tris) and MDC ({} verts, {} tris) produced identical vertex counts — likely same algorithm",
            rfta_mesh.vertex_count(), rfta_mesh.triangle_count(),
            mdc_mesh.vertex_count(), mdc_mesh.triangle_count(),
        );
    }

    #[test]
    fn rfta_classifications_valid() {
        let node = bounded_sphere([0.0; 3], 20.0);
        let mesh = reach_for_the_arcs(&node, 4);
        assert_eq!(mesh.classifications.len(), mesh.vertex_count());
        assert_eq!(
            mesh.contributors.len(),
            mesh.vertex_count() * MAX_CONTRIBUTORS,
        );
    }
}

