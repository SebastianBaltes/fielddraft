//! CSG → `fidget` bridge + interval-octree Manifold-Dual-Contouring mesher.
//!
//! The export path's fourth mesher (dropdown: "Interval-octree DC"). It
//! translates our decoded `BoundedCsg` field into a `fidget::context::Tree`
//! (the same implicit expression, op for op) and hands it to fidget's
//! interval-arithmetic octree + Manifold Dual Contouring. Why this matters
//! for the no-B-Rep / non-1-Lipschitz regime:
//!
//!  * **Interval arithmetic** subdivides the octree by *certified* sign
//!    bounds on the actual expression — no Lipschitz/distance assumption —
//!    so the topology of gyroid/TPMS fields is captured safely where our
//!    distance-bounded narrow-band octree could misjudge it.
//!  * **Dual Contouring with QEF** recovers sharp feature edges from the
//!    field itself (normal discontinuities), which Surface Nets rounds
//!    away — the closest we get to "exact edges" without a B-Rep.
//!  * Adaptive → far fewer triangles than a dense uniform grid.
//!
//! Only the analytically-expressible subset is bridged (the primitives
//! and operators whose field is a closed-form expression). Anything
//! backed by sampled data or a 2D sketch tree — imported meshes, extrude
//! / revolve / sweep / helix carriers, masked blends, shells — returns
//! `Err`, and the export falls back to another method for that body.

use crate::bvh::BoundedCsg;
use crate::dc::Mesh;
use fidget::context::Tree;
use fidget::mesh::{Octree, Settings};
use fidget::vm::VmShape;
use crate::csg::BlendMode;
use fielddraft_protocol::{Primitive, Transform};

/// max(t, 0) — the "outside" part of a distance combine.
fn relu(t: Tree) -> Tree {
    t.max(0.0)
}
/// min(t, 0) — the "inside" part of a distance combine.
fn nrelu(t: Tree) -> Tree {
    t.min(0.0)
}
/// -t (Tree has no Neg impl we rely on; multiply by -1).
fn neg(t: Tree) -> Tree {
    t * -1.0
}

/// Rotation matrix (row-major 3×3, f64) from a unit quaternion `[x,y,z,w]`.
fn quat_to_mat(q: [f32; 4]) -> [[f64; 3]; 3] {
    let (x, y, z, w) = (q[0] as f64, q[1] as f64, q[2] as f64, q[3] as f64);
    [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y - w * z),
            2.0 * (x * z + w * y),
        ],
        [
            2.0 * (x * y + w * z),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z - w * x),
        ],
        [
            2.0 * (x * z - w * y),
            2.0 * (y * z + w * x),
            1.0 - 2.0 * (x * x + y * y),
        ],
    ]
}

/// world → local point as three affine `Tree` expressions:
/// `local = Rᵀ·(p − t) / scale`. Mirrors `sdf_dual::world_to_local_d3`.
fn world_to_local(t: &Transform, p: &[Tree; 3]) -> [Tree; 3] {
    let d = [
        p[0].clone() - t.translate[0] as f64,
        p[1].clone() - t.translate[1] as f64,
        p[2].clone() - t.translate[2] as f64,
    ];
    // Inverse rotation = transpose of the rotation matrix.
    let m = quat_to_mat(t.rotation);
    let rot = |col: usize| -> Tree {
        // Rᵀ row `col` · d = m[0][col]*d0 + m[1][col]*d1 + m[2][col]*d2
        d[0].clone() * m[0][col] + d[1].clone() * m[1][col] + d[2].clone() * m[2][col]
    };
    [
        rot(0) * (1.0 / t.scale[0] as f64),
        rot(1) * (1.0 / t.scale[1] as f64),
        rot(2) * (1.0 / t.scale[2] as f64),
    ]
}

/// Squared length of a 2-vector as a Tree.
fn len2(a: &Tree, b: &Tree) -> Tree {
    a.clone() * a.clone() + b.clone() * b.clone()
}

/// Implicit field of a primitive at local point `p`, as a Tree. Mirrors
/// `sdf_eval::sdf_primitive_generic` exactly for the analytic primitives;
/// `Err` for anything sampled/sketch-based.
fn primitive_to_tree(prim: &Primitive, p: &[Tree; 3]) -> Result<Tree, String> {
    let f = |v: f32| v as f64;
    match prim {
        Primitive::Sphere { center, radius } => {
            let dx = p[0].clone() - f(center[0]);
            let dy = p[1].clone() - f(center[1]);
            let dz = p[2].clone() - f(center[2]);
            Ok((dx.clone() * dx + dy.clone() * dy + dz.clone() * dz).sqrt() - f(*radius))
        }
        Primitive::Box { center, half_extents } => {
            let q = [
                (p[0].clone() - f(center[0])).abs() - f(half_extents[0]),
                (p[1].clone() - f(center[1])).abs() - f(half_extents[1]),
                (p[2].clone() - f(center[2])).abs() - f(half_extents[2]),
            ];
            let outside = (relu(q[0].clone()) * relu(q[0].clone())
                + relu(q[1].clone()) * relu(q[1].clone())
                + relu(q[2].clone()) * relu(q[2].clone()))
            .sqrt();
            let qmax = q[0].clone().max(q[1].clone()).max(q[2].clone());
            Ok(outside + nrelu(qmax))
        }
        Primitive::Cylinder { center, radius, half_height } => {
            let dx = p[0].clone() - f(center[0]);
            let dy = p[1].clone() - f(center[1]);
            let dz = p[2].clone() - f(center[2]);
            let radial = len2(&dx, &dz).sqrt() - f(*radius);
            let axial = dy.abs() - f(*half_height);
            let outside = len2(&relu(radial.clone()), &relu(axial.clone())).sqrt();
            let inside = nrelu(radial.clone().max(axial.clone()));
            Ok(outside + inside)
        }
        Primitive::Torus { center, major_radius, minor_radius } => {
            let dx = p[0].clone() - f(center[0]);
            let dy = p[1].clone() - f(center[1]);
            let dz = p[2].clone() - f(center[2]);
            let q0 = len2(&dx, &dz).sqrt() - f(*major_radius);
            Ok(len2(&q0, &dy).sqrt() - f(*minor_radius))
        }
        Primitive::Capsule { radius, half_height } => {
            let r = len2(&p[0], &p[2]).sqrt();
            let hh = f(*half_height);
            let y_clamped = p[1].clone().max(-hh).min(hh);
            let dy = p[1].clone() - y_clamped;
            Ok(len2(&r, &dy).sqrt() - f(*radius))
        }
        Primitive::Cone { r_bottom, r_top, half_height } => {
            let r = len2(&p[0], &p[2]).sqrt();
            let hh = f(*half_height);
            let t = ((p[1].clone() + hh) * (1.0 / (2.0 * hh))).max(0.0).min(1.0);
            let r_at_y = t.clone() * (f(*r_top) - f(*r_bottom)) + f(*r_bottom);
            let dr = (*r_bottom - *r_top) as f64;
            let two_hh = 2.0 * hh;
            let slant = (dr * dr + two_hh * two_hh).sqrt();
            let wall_scale = if slant > 1e-9 { two_hh / slant } else { 1.0 };
            let d_wall = (r - r_at_y) * wall_scale;
            let d_axial = p[1].clone().abs() - hh;
            let outside = len2(&relu(d_wall.clone()), &relu(d_axial.clone())).sqrt();
            let inside = nrelu(d_wall.clone().max(d_axial.clone()));
            Ok(outside + inside)
        }
        Primitive::Gyroid { cell_size, thickness } => {
            let s = f(*cell_size);
            let x = p[0].clone() * (1.0 / s);
            let y = p[1].clone() * (1.0 / s);
            let z = p[2].clone() * (1.0 / s);
            let g = x.clone().sin() * y.clone().cos()
                + y.clone().sin() * z.clone().cos()
                + z.clone().sin() * x.clone().cos();
            Ok((g.abs() - f(*thickness)) * (s * 0.2887))
        }
        other => Err(format!(
            "primitive {:?} is not closed-form expressible for interval-DC",
            std::mem::discriminant(other)
        )),
    }
}

/// Exact-circular smooth-min (IQ round union) as a Tree. Mirrors
/// `sdf_dual::smin_poly_generic` (Fillet mode):
/// `max(r, min) − ‖(relu(r−da), relu(r−db))‖`.
fn smin_poly(da: &Tree, db: &Tree, r: f64) -> Tree {
    if r <= 0.0 {
        return da.clone().min(db.clone());
    }
    let ua = relu(neg(da.clone()) + r);
    let ub = relu(neg(db.clone()) + r);
    let min_ab = da.clone().min(db.clone());
    min_ab.max(r) - (ua.clone() * ua + ub.clone() * ub).sqrt()
}

/// Chamfer union as a Tree. Mirrors `sdf_dual::chamfer_union_generic`.
fn chamfer_union(da: &Tree, db: &Tree, r: f64) -> Tree {
    let sqrt_half = std::f64::consts::FRAC_1_SQRT_2;
    let min_ab = da.clone().min(db.clone());
    let chamfer = (da.clone() + db.clone() - r) * sqrt_half;
    min_ab.min(chamfer)
}

fn blend_min(da: &Tree, db: &Tree, r: f64, mode: BlendMode) -> Tree {
    match mode {
        BlendMode::Fillet => smin_poly(da, db, r),
        BlendMode::Chamfer => chamfer_union(da, db, r),
    }
}

/// Implicit field of a `BoundedCsg` node at point `p`, as a Tree. Mirrors
/// `sdf_dual::eval_bounded_d3` (Visual mode) op for op — minus the
/// AABB-distance pruning shortcuts, which are optimisations that don't
/// change the field. `Err` for any non-closed-form node.
fn node_to_tree(node: &BoundedCsg, p: &[Tree; 3]) -> Result<Tree, String> {
    match node {
        BoundedCsg::Leaf { primitive, world_transform, .. } => {
            let local = world_to_local(world_transform, p);
            let d = primitive_to_tree(primitive, &local)?;
            Ok(d * world_transform.min_scale() as f64)
        }
        BoundedCsg::Union { left, right, .. } => {
            Ok(node_to_tree(left, p)?.min(node_to_tree(right, p)?))
        }
        BoundedCsg::Intersection { left, right, .. } => {
            Ok(node_to_tree(left, p)?.max(node_to_tree(right, p)?))
        }
        BoundedCsg::Difference { left, right, .. } => {
            Ok(node_to_tree(left, p)?.max(neg(node_to_tree(right, p)?)))
        }
        BoundedCsg::SmoothUnion { left, right, radius, mode, .. } => {
            let da = node_to_tree(left, p)?;
            let db = node_to_tree(right, p)?;
            Ok(blend_min(&da, &db, *radius as f64, *mode))
        }
        BoundedCsg::SmoothDifference { left, right, radius, mode, .. } => {
            let da = node_to_tree(left, p)?;
            let db = node_to_tree(right, p)?;
            Ok(neg(blend_min(&neg(da), &db, *radius as f64, *mode)))
        }
        BoundedCsg::Twist { inner, rate, .. } => {
            let angle = p[1].clone() * *rate as f64;
            let (s, c) = (angle.clone().sin(), angle.cos());
            let warped = [
                p[0].clone() * c.clone() - p[2].clone() * s.clone(),
                p[1].clone(),
                p[0].clone() * s + p[2].clone() * c,
            ];
            node_to_tree(inner, &warped)
        }
        BoundedCsg::Bend { inner, rate, .. } => {
            let angle = p[0].clone() * *rate as f64;
            let (s, c) = (angle.clone().sin(), angle.cos());
            let warped = [
                c.clone() * p[0].clone() - s.clone() * p[1].clone(),
                s * p[0].clone() + c * p[1].clone(),
                p[2].clone(),
            ];
            node_to_tree(inner, &warped)
        }
        BoundedCsg::Offset { inner, delta, .. } => {
            Ok(node_to_tree(inner, p)? - *delta as f64)
        }
        BoundedCsg::Displace { inner, depth, freq, .. } => {
            // Achsfaktoren 1.3/0.7 wie in ALLEN übrigen Repräsentationen
            // (bvh/cell/dual/VM) — vorher wich fidget hier ab (reines
            // sin·sin·sin ohne Faktoren = eine ANDERE Fläche).
            let bump = (p[0].clone() * *freq as f64).sin()
                * (p[1].clone() * *freq as f64 * 1.3).sin()
                * (p[2].clone() * *freq as f64 * 0.7).sin()
                * *depth as f64;
            Ok(node_to_tree(inner, p)? - bump)
        }
        BoundedCsg::Morph { left, right, t, .. } => {
            let da = node_to_tree(left, p)?;
            let db = node_to_tree(right, p)?;
            Ok(da * (1.0 - *t as f64) + db * *t as f64)
        }
        BoundedCsg::Empty => Err("empty body".into()),
        _ => Err("node contains a sketch/data carrier not expressible for interval-DC".into()),
    }
}

/// Mesh a body with fidget's interval-octree Manifold Dual Contouring.
/// `aabb` is the body's world AABB (min, max); `cell_mm` the target cell
/// size (drives the octree depth). Returns `Err` when the field isn't
/// closed-form (caller falls back).
pub fn mesh_body_fidget(
    node: &BoundedCsg,
    aabb_min: [f32; 3],
    aabb_max: [f32; 3],
    cell_mm: f32,
) -> Result<Mesh, String> {
    // Cubic model box centred on the AABB, with margin so the surface sits
    // strictly inside [-1,+1]³ (fidget meshes exactly that box).
    let center = [
        0.5 * (aabb_min[0] + aabb_max[0]) as f32,
        0.5 * (aabb_min[1] + aabb_max[1]) as f32,
        0.5 * (aabb_min[2] + aabb_max[2]) as f32,
    ];
    let size = [
        aabb_max[0] - aabb_min[0],
        aabb_max[1] - aabb_min[1],
        aabb_max[2] - aabb_min[2],
    ];
    let max_size = size[0].max(size[1]).max(size[2]).max(1e-3);
    let half = 0.5 * max_size + (2.0 * cell_mm).max(0.02 * max_size);

    // Octree depth from the target cell size: 2·half / 2^depth ≈ cell.
    let depth = ((2.0 * half / cell_mm.max(1e-3)).log2().ceil() as i32).clamp(3, 8) as u8;

    // Build the field as a function of the model axes: world = center + half·model.
    let (mx, my, mz) = Tree::axes();
    let world = [
        mx * half as f64 + center[0] as f64,
        my * half as f64 + center[1] as f64,
        mz * half as f64 + center[2] as f64,
    ];
    let field = node_to_tree(node, &world)?;
    let shape = VmShape::from(field);

    let settings = Settings {
        depth,
        threads: None,
        ..Settings::default()
    };
    let octree = Octree::build(&shape, &settings)
        .ok_or_else(|| "fidget octree build cancelled".to_string())?;
    let raw = octree.walk_dual();
    if raw.vertices.is_empty() || raw.triangles.is_empty() {
        return Err("fidget produced an empty mesh".into());
    }

    // Model → world for the vertices.
    let mut positions = Vec::with_capacity(raw.vertices.len() * 3);
    for v in &raw.vertices {
        positions.push(center[0] + v.x * half);
        positions.push(center[1] + v.y * half);
        positions.push(center[2] + v.z * half);
    }
    let mut indices = Vec::with_capacity(raw.triangles.len() * 3);
    for t in &raw.triangles {
        indices.push(t.x as u32);
        indices.push(t.y as u32);
        indices.push(t.z as u32);
    }

    // Weld coincident vertices + drop degenerate/duplicate triangles.
    // fidget's DC places many byte-distinct but geometrically identical
    // vertices at cell boundaries; without welding they read as
    // degenerate (zero-area) triangles and phantom non-manifold edges.
    let (positions, indices) = weld_and_clean(&positions, &indices);
    if indices.is_empty() {
        return Err("fidget mesh degenerate after weld".into());
    }

    // SDF-gradient normals on the welded vertices.
    let vcount = positions.len() / 3;
    let mut normals = vec![0.0f32; positions.len()];
    for i in 0..vcount {
        let p = [positions[i * 3], positions[i * 3 + 1], positions[i * 3 + 2]];
        let n = crate::dc::sdf_normal(node, p, cell_mm * 0.25);
        normals[i * 3] = n[0];
        normals[i * 3 + 1] = n[1];
        normals[i * 3 + 2] = n[2];
    }

    Ok(Mesh {
        positions,
        normals,
        indices,
        ..Mesh::default()
    })
}

/// Merge vertices that share a position (quantised to a sub-micron grid),
/// then drop triangles that became degenerate (a repeated welded vertex)
/// or duplicate (same three welded vertices). Restores a clean manifold
/// from fidget's boundary-duplicated DC output.
fn weld_and_clean(positions: &[f32], indices: &[u32]) -> (Vec<f32>, Vec<u32>) {
    use std::collections::HashMap;
    // Weld only BIT-identical positions — exactly the coincident vertices
    // fidget emits at shared cell corners (the ones the position-welding
    // validator counts as degenerate). A tolerance weld would instead fuse
    // the two distinct vertices that meet at a sharp rim and manufacture a
    // non-manifold edge there.
    let mut canon: HashMap<[u32; 3], u32> = HashMap::new();
    let mut welded: Vec<f32> = Vec::new();
    let mut remap = vec![0u32; positions.len() / 3];
    for i in 0..positions.len() / 3 {
        let key = [
            positions[i * 3].to_bits(),
            positions[i * 3 + 1].to_bits(),
            positions[i * 3 + 2].to_bits(),
        ];
        let id = *canon.entry(key).or_insert_with(|| {
            let id = (welded.len() / 3) as u32;
            welded.extend_from_slice(&positions[i * 3..i * 3 + 3]);
            id
        });
        remap[i] = id;
    }
    // Drop only triangles that became degenerate (a repeated welded
    // vertex). fidget's Manifold DC is already 2-manifold once coincident
    // vertices are merged — de-duplicating "identical" triangles would
    // instead delete needed faces and open boundaries.
    let mut out: Vec<u32> = Vec::new();
    for t in indices.chunks_exact(3) {
        let (a, b, c) = (
            remap[t[0] as usize],
            remap[t[1] as usize],
            remap[t[2] as usize],
        );
        if a == b || b == c || a == c {
            continue;
        }
        out.extend_from_slice(&[a, b, c]);
    }
    (welded, out)
}
