//! Reach For The Arcs (Sellán et al., SIGGRAPH 2024) — full port.
//!
//! Pipeline: SDF grid → spheres → tangency point cloud → sPSR → mesh.
//! Optionally refined via a fine-tuning loop that reprojects tangency
//! points onto the evolving sPSR surface.

mod closest_point;
mod finetune;
mod poisson;
mod sphere_tree;

use crate::bvh::BoundedCsg;
use crate::csg::{EvalMode, MAX_CONTRIBUTORS};
use crate::dc::{sdf_normal, Mesh, CONTRIBUTOR_SENTINEL, DC_MARGIN_CELLS};
use sphere_tree::{SdfSphere, SphereAabbTree};

pub fn reach_for_the_arcs(node: &BoundedCsg, resolution: u32) -> Mesh {
    if resolution == 0 {
        return Mesh::default();
    }
    let raw = node.aabb();
    if raw.is_empty() {
        return Mesh::default();
    }

    let raw_size = raw.size();
    let max_dim = raw_size[0].max(raw_size[1]).max(raw_size[2]);
    if max_dim <= 0.0 {
        return Mesh::default();
    }
    let cell = max_dim / resolution as f32;
    let grid = raw.expanded(cell * DC_MARGIN_CELLS);
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

    let mut spheres = Vec::with_capacity(cnx * cny * cnz);
    for k in 0..cnz {
        for j in 0..cny {
            for i in 0..cnx {
                let idx = i + j * cnx + k * cnx * cny;
                let d = sdf_vals[idx];
                if d.abs() < 1e-10 {
                    continue;
                }
                spheres.push(SdfSphere {
                    center: corner_point(i, j, k),
                    radius: d.abs(),
                    positive: d > 0.0,
                });
            }
        }
    }

    if spheres.is_empty() {
        return Mesh::default();
    }

    let tree = SphereAabbTree::build(&spheres);

    // ── 3. Compute initial tangency set ────────────────────────────
    //
    // For each SDF sphere, the tangency point is the closest point on
    // the sphere surface to the zero iso-surface.  This is found by
    // projecting along the SDF gradient: a_i = p_i − d_i·n_i.

    let grad_eps = cell * 0.25;
    let mut tangency_pos = Vec::with_capacity(spheres.len());
    let mut tangency_nrm = Vec::with_capacity(spheres.len());
    let mut tangency_active = Vec::with_capacity(spheres.len());

    for sphere in &spheres {
        let g = sdf_normal(node, sphere.center, grad_eps);
        let glen = (g[0] * g[0] + g[1] * g[1] + g[2] * g[2]).sqrt();
        if glen < 1e-8 {
            tangency_pos.push(sphere.center);
            tangency_nrm.push([0.0, 1.0, 0.0]);
            tangency_active.push(false);
            continue;
        }
        let n = [g[0] / glen, g[1] / glen, g[2] / glen];
        let d = if sphere.positive { sphere.radius } else { -(sphere.radius) };
        tangency_pos.push([
            sphere.center[0] - d * n[0],
            sphere.center[1] - d * n[1],
            sphere.center[2] - d * n[2],
        ]);
        tangency_nrm.push(n);
        tangency_active.push(true);
    }

    let (points, normals): (Vec<_>, Vec<_>) = tangency_pos
        .iter()
        .zip(tangency_nrm.iter())
        .zip(tangency_active.iter())
        .filter(|((_, _), active)| **active)
        .map(|((p, n), _)| (*p, *n))
        .unzip();

    if points.is_empty() {
        return Mesh::default();
    }

    // ── 4. sPSR → initial mesh ─────────────────────────────────────

    let (mut psr_verts, mut psr_tris) = match poisson::reconstruct(&points, &normals) {
        Some(m) => m,
        None => return Mesh::default(),
    };

    // ── 5. Fine-tuning loop ────────────────────────────────────────

    let k_max = if resolution <= 8 { 0 } else if resolution <= 16 { 1 } else if resolution <= 32 { 2 } else { 3 };

    for _iter in 0..k_max {
        let updated = finetune::finetune_iteration(
            &spheres,
            &tree,
            &mut tangency_pos,
            &tangency_active,
            &psr_verts,
            &psr_tris,
        );
        if !updated {
            break;
        }

        let (pts, nrm): (Vec<_>, Vec<_>) = tangency_pos
            .iter()
            .zip(tangency_nrm.iter_mut())
            .zip(tangency_active.iter())
            .zip(spheres.iter())
            .filter(|(((_, _), active), _)| **active)
            .map(|(((pos, nrm_slot), _), sphere)| {
                let d = [
                    pos[0] - sphere.center[0],
                    pos[1] - sphere.center[1],
                    pos[2] - sphere.center[2],
                ];
                let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                let sign = if sphere.positive { 1.0 } else { -1.0 };
                let n = if len > 1e-10 {
                    [sign * d[0] / len, sign * d[1] / len, sign * d[2] / len]
                } else {
                    [0.0, sign, 0.0]
                };
                *nrm_slot = n;
                (*pos, n)
            })
            .unzip();

        if pts.is_empty() {
            break;
        }

        match poisson::reconstruct(&pts, &nrm) {
            Some((v, t)) => {
                psr_verts = v;
                psr_tris = t;
            }
            None => break,
        }
    }

    // ── 6. Convert to output Mesh ──────────────────────────────────

    let normal_eps = cell * 0.25;
    let contributor_eps = cell * 0.5;
    let vc = psr_verts.len() / 3;
    let positions = psr_verts;
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
        let mut element = cs.element();
        if element == crate::csg::ProxyElement::None {
            element = crate::csg::ProxyElement::Face;
        }
        let slice = cs.as_slice();
        let mut contribs = [CONTRIBUTOR_SENTINEL; MAX_CONTRIBUTORS];
        for slot in 0..MAX_CONTRIBUTORS {
            if slot < slice.len() {
                contribs[slot] = slice[slot]
                    .as_u32_or_root()
                    .unwrap_or(CONTRIBUTOR_SENTINEL);
            }
        }
        classifications.push(element.tag());
        contributors.extend_from_slice(&contribs);
    }

    Mesh {
        positions,
        normals: normals_out,
        indices: psr_tris,
        classifications,
        contributors,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::csg::CsgNode;
    use fielddraft_protocol::{BoolOp, Feature, FeatureId, Primitive, Transform};

    fn make_feature(id: u32, prim: Primitive) -> Feature {
        Feature {
            id: FeatureId::from_u32(id),
            name: String::new(),
            op: BoolOp::Union,
            primitive: prim,
            transform: Transform::IDENTITY,
            blend_radius: 0.0,
            chamfer_size: 0.0,
            twist_rate: 0.0,
            bend_rate: 0.0,
            displace_depth: 0.0,
            displace_freq: 0.0,
            shell_thickness: 0.0,
            target_ids: vec![],
        }
    }

    fn bounded_sphere(center: [f32; 3], radius: f32) -> crate::bvh::BoundedCsg {
        let f = make_feature(1, Primitive::Sphere { center, radius });
        let csg = CsgNode::from_features(&[f]);
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
        let csg = CsgNode::from_features(&[f]);
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
    fn rfta_empty_scene() {
        let mesh = reach_for_the_arcs(&crate::bvh::BoundedCsg::Empty, 4);
        assert!(mesh.is_empty());
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
