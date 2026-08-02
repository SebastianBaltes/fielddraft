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

/// Circular-CD smooth-min (#384) as a Tree. Mirrors
/// `sdf_dual::smin_poly_generic` (Fillet mode):
/// `min − (r/2)·(1 + h − √(1 − h·(h−2)))`, `h = relu(r − |a−b|)/r`.
fn smin_poly(da: &Tree, db: &Tree, r: f64) -> Tree {
    if r <= 0.0 {
        return da.clone().min(db.clone());
    }
    let h = relu(neg((da.clone() - db.clone()).abs()) + r) * (1.0 / r);
    let s = (neg(h.clone() * (h.clone() - 2.0)) + 1.0).sqrt();
    let min_ab = da.clone().min(db.clone());
    min_ab - (h + 1.0 - s) * (r * 0.5)
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
        // offset_tree Phase B: exakter symbolischer Spiegel des Kanal-Terms
        // (`offset_channel::channel_apply`) — argmin-Bogen und min/max-
        // Gewinner via `compare`-Selects, Bogen-Distanz closed-form mit
        // `atan2`. Kein Bounding-Sphere-Skip (feldwert-identisch, der Skip
        // ist reine Laufzeit-Optimierung der numerischen Interpreter).
        BoundedCsg::ArcChannel { inner, channels, verts, rims, lb, .. } => {
            let mut d = node_to_tree(inner, p)?;
            // #384 §8i: fidget ist ein Ausdrucks-DAG, keine Stack-Maschine —
            // das „Basis-Register" ist hier schlicht der TEILAUSDRUCK VOR den
            // Kanal-/Vertex-Termen. Kein Opcode nötig, kein State: derselbe
            // Knoten `d0` wird in jedes Gate hineingehängt (der DAG teilt ihn,
            // also kostet die Mehrfachnutzung nichts).
            let d0 = d.clone();
            // #384 §8o: EIN `L`-Teilausdruck je Stopp, von allen Termen geteilt
            // (der DAG teilt ihn, die Mehrfachnutzung kostet nichts).
            let lbt = match lb {
                Some(b) => lb_to_tree(&d0, p, b),
                None => Tree::from(crate::offset_lb::LB_NONE as f64),
            };
            // #384 §8j: das Grenzwert-Register ist im DAG ebenfalls kein
            // State, sondern ein `max` über die Term-Beiträge.
            let mut lim: Option<Tree> = None;
            let mut acc = |l: Tree, lim: &mut Option<Tree>| {
                *lim = Some(match lim.take() {
                    Some(prev) => prev.max(l),
                    None => l,
                });
            };
            for ch in channels {
                let (nd, l) = channel_to_tree(&d0, &lbt, &d, p, ch);
                d = nd;
                acc(l, &mut lim);
            }
            for vs in verts {
                let (nd, l) = vertex_to_tree(&d0, &lbt, &d, p, vs);
                d = nd;
                acc(l, &mut lim);
            }
            // #384 §8j: die gemeinsame Klemme (Spiegel von `OP_STOP_LIMIT`).
            if let (Some(l), Some(concave)) = (
                lim,
                channels
                    .first()
                    .map(|c| c.concave)
                    .or_else(|| verts.first().map(|v| v.concave)),
            ) {
                d = if concave { d.max(l) } else { d.min(neg(l)) };
            }
            // offset_tree Phase 4 (#398): ∂P-Rand-Fillet als Tree — symbolischer
            // Spiegel von `offset_rim::apply_rims_f32`.
            for block in rims {
                d = rim_to_tree(&d, p, block);
                d = rim_arc_to_tree(&d, p, block);
            }
            Ok(d)
        }
        BoundedCsg::Empty => Err("empty body".into()),
        _ => Err("node contains a sketch/data carrier not expressible for interval-DC".into()),
    }
}

// ─── offset_tree Phase B: Kanal-Korrektur als Tree ──────────────────

/// `x` wenn `a < b`, sonst `y` (bei Gleichheit `y`) — numerischer Select
/// über `compare` (−1/0/+1) ohne Verzweigung.
fn select_lt(a: &Tree, b: &Tree, x: &Tree, y: &Tree) -> Tree {
    let m = relu(b.clone().compare(a.clone())); // 1 wenn b > a, sonst 0
    y.clone() + (x.clone() - y.clone()) * m
}

fn t_dot(a: &[Tree; 3], b: &[Tree; 3]) -> Tree {
    a[0].clone() * b[0].clone() + a[1].clone() * b[1].clone() + a[2].clone() * b[2].clone()
}

fn t_sub(a: &[Tree; 3], b: &[Tree; 3]) -> [Tree; 3] {
    [
        a[0].clone() - b[0].clone(),
        a[1].clone() - b[1].clone(),
        a[2].clone() - b[2].clone(),
    ]
}

fn t_const3(v: [f32; 3]) -> [Tree; 3] {
    [
        Tree::from(v[0] as f64),
        Tree::from(v[1] as f64),
        Tree::from(v[2] as f64),
    ]
}

fn t_norm(a: &[Tree; 3]) -> Tree {
    t_dot(a, a).sqrt()
}

/// #384 §8p — die konkave Gate-Basis als Tree: `L − δ`, wo `L` verfügbar ist,
/// sonst `G − d₀`. Spiegel von [`crate::offset_channel::concave_gate`].
///
/// Die Fallunterscheidung läuft als MULTIPLIKATIVE Maske
/// `m·(L−δ) + (1−m)·(G−d₀)` — dieselbe Bauart wie in [`lb_to_tree`] und aus
/// demselben Grund: `select_lt` addiert intern `(x−y)`, und mit `y = −1e30`
/// löscht das in f32 die Mantisse des anderen Zweigs (Präzedenzfall 1.435 mm).
/// `0 · (−1e30) = −0` ist dagegen exakt.
fn lb_gate_tree(lb: &Tree, gate_w: f32, delta: f32, base: &Tree) -> Tree {
    // `m = 1`, wenn `L > LB_AVAIL` — also eine echte Stück-Distanz vorliegt.
    let m = relu(lb.clone().compare(Tree::from(crate::offset_channel::LB_AVAIL as f64)));
    (lb.clone() - delta as f64) * m.clone()
        + (Tree::from(gate_w as f64) - base.clone()) * (Tree::from(1.0) - m)
}

/// #384 §8o — Spiegel von [`crate::offset_lb::lb_value`] als Tree.
///
/// Im DAG ist das billig: die Stück-Koordinaten sind KONSTANTEN, also sind
/// Ebenen-Normale, Kanten-Richtungen und die drei Innen-Halbebenen eines
/// Dreiecks konstante Koeffizienten — nur `p` ist symbolisch. Ein Stück kostet
/// damit eine Handvoll affiner Formen plus die `min`-Faltung, und `min` ist ein
/// nativer fidget-Op mit Intervall-Pruning.
///
/// Die Neutral-Maske läuft NICHT über `select_lt` (dessen `y + (x−y)·m` löscht
/// bei einem 1e30-Sentinel die Mantisse aus), sondern additiv — derselbe
/// Präzedenzfall wie beim Grenzwert-Register.
fn lb_to_tree(base: &Tree, p: &[Tree; 3], blk: &crate::offset_lb::LbBlock) -> Tree {
    let mut best: Option<Tree> = None;
    for piece in &blk.pieces {
        let d = match *piece {
            crate::offset_lb::LbPiece::Tri { a, b, c, .. } => tri_dist_tree(p, a, b, c),
            crate::offset_lb::LbPiece::Rev { o, ax, r0, z0, r1, z1 } => {
                rev_dist_tree(p, o, ax, r0, z0, r1, z1)
            }
        };
        best = Some(match best.take() {
            Some(prev) => prev.min(d),
            None => d,
        });
    }
    let Some(best) = best else {
        return Tree::from(crate::offset_lb::LB_NONE as f64);
    };
    // Innen-Indikator (`d₀ ≤ δ`) UND Fernfeld-Kugel.
    let rc = t_norm(&t_sub(p, &t_const3(blk.center)));
    let on = relu(Tree::from(blk.delta as f64).compare(base.clone()))
        * relu(Tree::from(blk.skip_r as f64).compare(rc));
    (best - blk.slack as f64) * on.clone()
        - (Tree::from(1.0) - on) * (crate::offset_channel::END_PLANE_OFF as f64)
}

/// Distanz zu einem Dreieck mit KONSTANTEN Ecken — Spiegel von
/// [`crate::offset_lb::tri_dist`].
fn tri_dist_tree(p: &[Tree; 3], a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> Tree {
    let f = |v: [f32; 3]| [v[0] as f64, v[1] as f64, v[2] as f64];
    let (a, b, c) = (f(a), f(b), f(c));
    let vsub = |u: [f64; 3], v: [f64; 3]| [u[0] - v[0], u[1] - v[1], u[2] - v[2]];
    let vcross = |u: [f64; 3], v: [f64; 3]| {
        [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ]
    };
    let vdot = |u: [f64; 3], v: [f64; 3]| u[0] * v[0] + u[1] * v[1] + u[2] * v[2];
    let ba = vsub(b, a);
    let cb = vsub(c, b);
    let ac = vsub(a, c);
    let nor = vcross(ba, ac);
    let nn = vdot(nor, nor);
    let seg = |u: [f64; 3], v: [f64; 3]| -> Tree {
        let e = vsub(v, u);
        let ll = vdot(e, e).max(1e-20);
        let w = t_sub(p, &t_const3([u[0] as f32, u[1] as f32, u[2] as f32]));
        let t = (t_dot(&w, &t_const3([e[0] as f32, e[1] as f32, e[2] as f32])) * (1.0 / ll))
            .max(0.0)
            .min(1.0);
        let q = [
            w[0].clone() - t.clone() * e[0],
            w[1].clone() - t.clone() * e[1],
            w[2].clone() - t * e[2],
        ];
        t_norm(&q)
    };
    let edge = seg(a, b).min(seg(b, c)).min(seg(c, a));
    if nn <= 1e-24 {
        return edge;
    }
    let inv = 1.0 / nn.sqrt();
    let plane = (t_dot(p, &t_const3([
        (nor[0] * inv) as f32,
        (nor[1] * inv) as f32,
        (nor[2] * inv) as f32,
    ])) - vdot(nor, a) * inv)
        .abs();
    // Innen-Test: die drei affinen Formen `⟨e × nor, p − v⟩`.
    let mut inside: Option<Tree> = None;
    for (e, v) in [(ba, a), (cb, b), (ac, c)] {
        let g = vcross(e, nor);
        let gl = vdot(g, g).sqrt().max(1e-30);
        let form = t_dot(p, &t_const3([
            (g[0] / gl) as f32,
            (g[1] / gl) as f32,
            (g[2] / gl) as f32,
        ])) - vdot(g, v) / gl;
        inside = Some(match inside.take() {
            Some(prev) => prev.min(form),
            None => form,
        });
    }
    let inside = inside.expect("drei Kanten");
    // `inside ≥ 0` ⇒ Lot auf die Ebene, sonst Kanten-Minimum.
    select_lt(&Tree::from(0.0), &inside, &plane, &edge)
}

/// Distanz zu einer Rotationsfläche mit KONSTANTEM Profil — Spiegel von
/// [`crate::offset_lb::rev_dist`].
#[allow(clippy::too_many_arguments)]
fn rev_dist_tree(
    p: &[Tree; 3],
    o: [f32; 3],
    ax: [f32; 3],
    r0: f32,
    z0: f32,
    r1: f32,
    z1: f32,
) -> Tree {
    let d = t_sub(p, &t_const3(o));
    let axt = t_const3(ax);
    let t = t_dot(&d, &axt);
    let rad = [
        d[0].clone() - t.clone() * ax[0] as f64,
        d[1].clone() - t.clone() * ax[1] as f64,
        d[2].clone() - t.clone() * ax[2] as f64,
    ];
    let r = t_norm(&rad);
    let (ex, ey) = ((r1 - r0) as f64, (z1 - z0) as f64);
    let px = r - r0 as f64;
    let py = t - z0 as f64;
    let ll = ex * ex + ey * ey;
    let s = if ll > 1e-20 {
        ((px.clone() * ex + py.clone() * ey) * (1.0 / ll)).max(0.0).min(1.0)
    } else {
        Tree::from(0.0)
    };
    let qx = px - s.clone() * ex;
    let qy = py - s * ey;
    (qx.clone() * qx + qy.clone() * qy).sqrt()
}

/// Spiegel von [`crate::offset_channel::channel_apply`] als Tree.
/// `base` ist der naive Stopp-Teilausdruck d₀ (#384 §8i) — im DAG die
/// Entsprechung des `OP_STOP_BASE`-Registers der Stack-Maschinen.
fn channel_to_tree(
    base: &Tree,
    lb: &Tree,
    d: &Tree,
    p: &[Tree; 3],
    ch: &crate::offset_channel::OffsetChannel,
) -> (Tree, Tree) {
    // Laufende argmin-Register: (best, q, m1, m2) als Trees.
    let mut best = Tree::from(f64::INFINITY.min(1e30));
    let mut best_q = p.clone();
    let mut best_m1 = t_const3([0.0; 3]);
    let mut best_m2 = t_const3([0.0; 3]);

    for arc in &ch.arcs {
        let (de, q, t) = if arc.sweep < 0.0 {
            // Segment c → c + u·r.
            let c = t_const3(arc.c);
            let u = t_const3(arc.u);
            let len = (arc.r as f64).max(1e-12);
            let t = (t_dot(&t_sub(p, &c), &u) * (1.0 / len)).max(0.0).min(1.0);
            let q = [
                c[0].clone() + u[0].clone() * t.clone() * len,
                c[1].clone() + u[1].clone() * t.clone() * len,
                c[2].clone() + u[2].clone() * t.clone() * len,
            ];
            (t_norm(&t_sub(p, &q)), q, t)
        } else {
            // #384 §8y: verzweigungsfrei UND trigonometrie-frei. Der Sektor ist
            // ein Kreuzprodukt-Paar, der Fusspunkt die normierte Projektion,
            // der Parameter der Diamant-Quotient.
            let c = t_const3(arc.c);
            let u = t_const3(arc.u);
            let v = t_const3(arc.v);
            let dc = t_sub(p, &c);
            let x = t_dot(&dc, &u);
            let y = t_dot(&dc, &v);
            let (e0, e1) = (arc.e[0] as f64, arc.e[1] as f64);
            let r = arc.r as f64;
            let zero = Tree::from(0.0);
            let rho = (x.clone() * x.clone() + y.clone() * y.clone()).sqrt().max(1e-9);
            // Start- und Endpunkt, algebraisch.
            let q0 = [
                c[0].clone() + u[0].clone() * r,
                c[1].clone() + u[1].clone() * r,
                c[2].clone() + u[2].clone() * r,
            ];
            let q1 = [
                c[0].clone() + (u[0].clone() * e0 + v[0].clone() * e1) * r,
                c[1].clone() + (u[1].clone() * e0 + v[1].clone() * e1) * r,
                c[2].clone() + (u[2].clone() * e0 + v[2].clone() * e1) * r,
            ];
            let cs = x.clone() / rho.clone();
            let sn = y.clone() / rho.clone();
            let q_in = [
                c[0].clone() + (u[0].clone() * cs.clone() + v[0].clone() * sn.clone()) * r,
                c[1].clone() + (u[1].clone() * cs.clone() + v[1].clone() * sn.clone()) * r,
                c[2].clone() + (u[2].clone() * cs.clone() + v[2].clone() * sn.clone()) * r,
            ];
            // Diamant: `x ≥ 0 ? y/(x+y) : 1 + (−x)/(y−x)`, als select.
            let dm_q1 = y.clone() / (x.clone() + y.clone());
            let dm_q2 = Tree::from(1.0) + (-x.clone()) / (y.clone() - x.clone());
            let dm = select_lt(&x, &zero, &dm_q2, &dm_q1);
            let t_in = (dm * (1.0 / arc.d_e as f64)).max(0.0).min(1.0);
            let d_in = t_norm(&t_sub(p, &q_in));
            let d0 = t_norm(&t_sub(p, &q0));
            let d1 = t_norm(&t_sub(p, &q1));
            // Endpunkt-Fall: näherer der beiden Enden, Gleichstand → Start.
            let d_end = d0.clone().min(d1.clone());
            let t_end = select_lt(&d1, &d0, &Tree::from(1.0), &Tree::from(0.0));
            let q_end = [
                select_lt(&d1, &d0, &q1[0], &q0[0]),
                select_lt(&d1, &d0, &q1[1], &q0[1]),
                select_lt(&d1, &d0, &q1[2], &q0[2]),
            ];
            // Sektor: `a = y ≥ 0 ∧ b = x·e₁ − y·e₀ ≥ 0`. Als zwei selects:
            // ausserhalb, wenn a < 0 ODER b < 0.
            let bcr = x.clone() * e1 - y.clone() * e0;
            let pick = |x_in: &Tree, x_out: &Tree| {
                let after_a = select_lt(&y, &zero, x_out, x_in);
                select_lt(&bcr, &zero, x_out, &after_a)
            };
            let de = pick(&d_in, &d_end);
            let q = [
                pick(&q_in[0], &q_end[0]),
                pick(&q_in[1], &q_end[1]),
                pick(&q_in[2], &q_end[2]),
            ];
            let t = pick(&t_in, &t_end);
            (de, q, t)
        };
        // m-Vektoren am Fußpunkt (nlerp) — Normierung symbolisch.
        let lerp3 = |a: [f32; 3], b: [f32; 3]| -> [Tree; 3] {
            let a = t_const3(a);
            let b = t_const3(b);
            [
                a[0].clone() + (b[0].clone() - a[0].clone()) * t.clone(),
                a[1].clone() + (b[1].clone() - a[1].clone()) * t.clone(),
                a[2].clone() + (b[2].clone() - a[2].clone()) * t.clone(),
            ]
        };
        let normed = |w: [Tree; 3]| -> [Tree; 3] {
            let n = t_norm(&w).max(1e-9);
            [
                w[0].clone() * n.clone().recip(),
                w[1].clone() * n.clone().recip(),
                w[2].clone() * n.recip(),
            ]
        };
        let m1 = normed(lerp3(arc.m1s, arc.m1e));
        let m2 = normed(lerp3(arc.m2s, arc.m2e));
        // Läuft der Bogen das argmin an? (de < best → übernehmen.)
        let upd = |x: &Tree, y: &Tree| select_lt(&de, &best, x, y);
        best_q = [
            upd(&q[0], &best_q[0]),
            upd(&q[1], &best_q[1]),
            upd(&q[2], &best_q[2]),
        ];
        best_m1 = [
            upd(&m1[0], &best_m1[0]),
            upd(&m1[1], &best_m1[1]),
            upd(&m1[2], &best_m1[2]),
        ];
        best_m2 = [
            upd(&m2[0], &best_m2[0]),
            upd(&m2[1], &best_m2[1]),
            upd(&m2[2], &best_m2[2]),
        ];
        best = best.min(de);
    }

    let dq = t_sub(p, &best_q);
    let s1 = t_dot(&dq, &best_m1);
    let s2 = t_dot(&dq, &best_m2);
    // #384 §8o/§8p: ist `L` verfügbar, gatet `L − δ` ALLEIN (Spiegel von
    // `concave_gate`); sonst bleibt die `G`-Kappe aus §8i. Die Maske läuft
    // MULTIPLIKATIV, nicht über `select_lt`: dessen `y + (x−y)·m` löscht bei
    // einem 1e30-Sentinel die Mantisse (Präzedenzfall 1.435 mm), während
    // `0 · (−1e30) = −0` exakt ist.
    let gate = if ch.concave {
        lb_gate_tree(&lb, ch.gate, ch.delta, &base)
    } else {
        Tree::from(ch.gate as f64) + base.clone()
    };
    // #384 §8f: drittes Gate — vier affine End-Halbräume (Konstanten).
    let mut s3: Option<Tree> = None;
    for g in &ch.end_planes {
        let v = p[0].clone() * g[0] as f64
            + p[1].clone() * g[1] as f64
            + p[2].clone() * g[2] as f64
            - g[3] as f64;
        s3 = Some(match s3 {
            Some(prev) => prev.min(v),
            None => v,
        });
    }
    // #384 §8j: viertes Gate — vier Fremdflächen-Halbräume (Konstanten).
    let mut s4: Option<Tree> = None;
    for g in &ch.face_planes {
        let v = p[0].clone() * g[0] as f64
            + p[1].clone() * g[1] as f64
            + p[2].clone() * g[2] as f64
            - g[3] as f64;
        s4 = Some(match s4 {
            Some(prev) => prev.min(v),
            None => v,
        });
    }
    let s4 = s4.expect("face_planes ist ein Array fester Länge");
    // #384 §8j: Grenzwert-Beitrag `δ − min(de, h)`, neutral ab `de ≥ K`.
    let trust_t = Tree::from(ch.trust as f64);
    let lim_on = Tree::from(ch.delta as f64)
        - best.clone().min(s4.clone() + ch.delta as f64);
    // NICHT über `select_lt`: dessen `y + (x−y)·m` löscht bei einem
    // 1e30-Sentinel die Mantisse aus (x + 1e30 == 1e30 in f32).
    // Additive Maske stattdessen — exakt für beide Zweige.
    let on = relu(trust_t.compare(best.clone()));
    let lim = lim_on
        - (Tree::from(1.0) - on) * (crate::offset_channel::END_PLANE_OFF as f64);
    // Rollen-Trennung (C4) — Spiegel von `channel_apply`.
    let w_geo = s1
        .min(s2)
        .min(s3.expect("end_planes ist ein Array fester Länge"))
        .min(s4)
        .min(best.clone() - ch.delta as f64)
        .min(neg(best) + ch.trust as f64);
    let w = w_geo.clone().min(gate);
    let out = if ch.concave {
        let zero = Tree::from(0.0);
        let clamp = d.clone().min(neg(w_geo.clone()));
        let inactive = select_lt(&zero, &w_geo, d, &clamp);
        let active = d.clone().min(neg(w.clone()));
        select_lt(&zero, &w, &active, &inactive)
    } else {
        d.clone().max(w)
    };
    (out, lim)
}

/// Spiegel von [`crate::offset_channel::vertex_apply`] als Tree
/// (offset_tree Phase C4). Deutlich einfacher als der Kanal: Punkt-Spine,
/// keine argmin-Selektion — ρ = |p − v|, drei konstante Gates. `base` ist
/// wie beim Kanal der naive Stopp-Teilausdruck d₀ (#384 §8i).
fn vertex_to_tree(
    base: &Tree,
    lb: &Tree,
    d: &Tree,
    p: &[Tree; 3],
    vs: &crate::offset_channel::VertexSphere,
) -> (Tree, Tree) {
    let v = t_const3(vs.v);
    let dq = t_sub(p, &v);
    let rho = t_norm(&dq);
    let rho_l = rho.clone();
    let s1 = t_dot(&dq, &t_const3(vs.gates[0]));
    let s2 = t_dot(&dq, &t_const3(vs.gates[1]));
    let s3 = t_dot(&dq, &t_const3(vs.gates[2]));
    // #384 §8o/§8p: Gate-Basis, s. `channel_to_tree` (`lb_gate_tree`).
    let gate = if vs.concave {
        lb_gate_tree(&lb, vs.gate, vs.delta, &base)
    } else {
        Tree::from(vs.gate as f64) + base.clone()
    };
    // Rollen-Trennung (C4) — Spiegel von `vertex_apply`.
    let w_geo = s1
        .min(s2)
        .min(s3)
        .min(rho.clone() - vs.delta as f64)
        .min(neg(rho) + vs.trust as f64);
    let w = w_geo.clone().min(gate);
    // #384 §8j: Grenzwert-Beitrag `δ − ρ`, neutral ab `ρ ≥ K`.
    let on = relu(Tree::from(vs.trust as f64).compare(rho_l.clone()));
    let lim = (Tree::from(vs.delta as f64) - rho_l)
        - (Tree::from(1.0) - on) * (crate::offset_channel::END_PLANE_OFF as f64);
    let out = if vs.concave {
        let zero = Tree::from(0.0);
        let clamp = d.clone().min(neg(w_geo.clone()));
        let inactive = select_lt(&zero, &w_geo, d, &clamp);
        let active = d.clone().min(neg(w.clone()));
        select_lt(&zero, &w, &active, &inactive)
    } else {
        d.clone().max(w)
    };
    (out, lim)
}

/// Spiegel von [`crate::offset_rim::apply_rims_f32`] als Tree (offset_tree
/// Phase 4, #398). Hard-Mask-Nearest-Select: je Record eine EXAKTE 0/1-Maske
/// (Trust · End-Caps · Wand-Halbräume — jedes Indikator `1 − relu(compare)` ist
/// exakt 0 oder 1), maskiert-heraus ⇒ maskierte Distanz `de_eff = BIG` → der
/// Record scheidet aus dem argmin aus. Der Gewinner (kleinste maskierte
/// Distanz, ERSTER bei Gleichstand) bestimmt `corr = concave ? δ−de : de−δ`,
/// komponiert via `min`/`max`. Kein aktiver Record (`any = 0`) ⇒ `d` unverändert.
/// eps-Literale exakt wie [`crate::offset_rim::select_rim_f32`].
fn rim_to_tree(d: &Tree, p: &[Tree; 3], block: &crate::offset_rim::RimBlock) -> Tree {
    let concave = block.concave;
    let big = 1e30_f64;
    // 1 wenn a ≤ b, sonst 0 (compare(a,b) = +1 gdw a > b).
    let le = |a: &Tree, b: &Tree| Tree::from(1.0) - relu(a.clone().compare(b.clone()));
    // 1 wenn a ≥ b, sonst 0.
    let ge = |a: &Tree, b: &Tree| Tree::from(1.0) - relu(b.clone().compare(a.clone()));
    let blend = |flag: &Tree, x: &Tree, y: &Tree| y.clone() + (x.clone() - y.clone()) * flag.clone();

    let mut best_de = Tree::from(big);
    let mut win_de = Tree::from(big);
    let mut win_delta = Tree::from(0.0);
    let mut any = Tree::from(0.0);

    for rec in &block.recs {
        let dvec = [rec.b[0] - rec.a[0], rec.b[1] - rec.a[1], rec.b[2] - rec.a[2]];
        let len2 = (dvec[0] * dvec[0] + dvec[1] * dvec[1] + dvec[2] * dvec[2]) as f64;
        let a3 = t_const3(rec.a);
        let (de, q, t_opt): (Tree, [Tree; 3], Option<Tree>) = if len2 < 1e-24 {
            // Punkt-Spine (a == b): keine End-Cap-Maske.
            (t_norm(&t_sub(p, &a3)), a3.clone(), None)
        } else {
            let inv = 1.0 / len2;
            let dv = t_const3(dvec);
            let t_raw = t_dot(&t_sub(p, &a3), &dv) * inv;
            let t = t_raw.clone().max(0.0).min(1.0);
            let q = [
                a3[0].clone() + dv[0].clone() * t.clone(),
                a3[1].clone() + dv[1].clone() * t.clone(),
                a3[2].clone() + dv[2].clone() * t.clone(),
            ];
            (t_norm(&t_sub(p, &q)), q, Some(t_raw))
        };
        let dq = t_sub(p, &q);
        // Maske: Trust.
        let mut pass = le(&de, &Tree::from(rec.trust as f64));
        // End-Caps t ∈ [−cap, 1+cap] (nur echtes Segment).
        if let Some(t_raw) = &t_opt {
            let cap = 1e-6;
            pass = pass * ge(t_raw, &Tree::from(-cap)) * le(t_raw, &Tree::from(1.0 + cap));
        }
        // Wand-Halbräume (ungenutzte Wand |n| < 0.5 → Maske true, in Rust entschieden).
        let eps = 1e-9;
        for n in [rec.n_a, rec.n_b, rec.n_c] {
            let nn = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            if nn < 0.5 {
                continue;
            }
            let dot = t_dot(&dq, &t_const3(n));
            let ind = if concave {
                le(&dot, &Tree::from(eps)) // dot ≤ eps
            } else {
                ge(&dot, &Tree::from(-eps)) // dot ≥ −eps
            };
            pass = pass * ind;
        }
        let de_eff = blend(&pass, &de, &Tree::from(big));
        // Neuer argmin gdw best_de > de_eff (strikt → erster gewinnt bei Gleichstand).
        let is_new = relu(best_de.clone().compare(de_eff.clone()));
        win_de = blend(&is_new, &de, &win_de);
        win_delta = blend(&is_new, &Tree::from(rec.delta as f64), &win_delta);
        best_de = blend(&is_new, &de_eff, &best_de);
        any = any.max(is_new);
    }
    let corr = if concave {
        win_delta - win_de
    } else {
        win_de - win_delta
    };
    let candidate = if concave { d.clone().min(corr) } else { d.clone().max(corr) };
    blend(&any, &candidate, d)
}

/// Spiegel von [`crate::offset_rim::apply_rims_arc_f32`] als Tree (FC Phase 4,
/// #398) — der gemischte Segment-/Bogen-Block. Hard-Mask-Nearest-Select wie
/// [`rim_to_tree`], aber je Record mit Segment- ODER Bogen-Fußpunkt (die
/// Fallunterscheidung `sweep < 0` steht zur BAUZEIT fest, wird also in Rust
/// verzweigt, nicht im Tree). eps-Literale exakt wie
/// [`crate::offset_rim::select_rim_arc_f32`].
fn rim_arc_to_tree(d: &Tree, p: &[Tree; 3], block: &crate::offset_rim::RimBlock) -> Tree {
    if block.arc_recs.is_empty() {
        return d.clone();
    }
    let concave = block.concave;
    let big = 1e30_f64;
    let le = |a: &Tree, b: &Tree| Tree::from(1.0) - relu(a.clone().compare(b.clone()));
    let ge = |a: &Tree, b: &Tree| Tree::from(1.0) - relu(b.clone().compare(a.clone()));
    let blend = |flag: &Tree, x: &Tree, y: &Tree| y.clone() + (x.clone() - y.clone()) * flag.clone();

    let mut best_de = Tree::from(big);
    let mut win_de = Tree::from(big);
    let mut win_delta = Tree::from(0.0);
    let mut any = Tree::from(0.0);

    for rec in &block.arc_recs {
        let c = t_const3(rec.c);
        let (de, q, cap_pass, n_b): (Tree, [Tree; 3], Tree, [Tree; 3]) = if rec.sweep < 0.0 {
            // Segment c → c + u·r (u Einheitsrichtung, r Länge).
            let u = t_const3(rec.u);
            let len = (rec.r as f64).max(1e-12);
            let s = t_dot(&t_sub(p, &c), &u);
            let t_raw = s.clone() * (1.0 / len);
            let sc = s.max(0.0).min(len);
            let q = [
                c[0].clone() + u[0].clone() * sc.clone(),
                c[1].clone() + u[1].clone() * sc.clone(),
                c[2].clone() + u[2].clone() * sc,
            ];
            let cap = 1e-6;
            let cap_pass = ge(&t_raw, &Tree::from(-cap)) * le(&t_raw, &Tree::from(1.0 + cap));
            (t_norm(&t_sub(p, &q)), q, cap_pass, t_const3(rec.n_b))
        } else {
            // Bogen: c=Zentrum, (u,v)=Basis, r=Radius, Wandnormale RADIAL.
            let u = t_const3(rec.u);
            let v = t_const3(rec.v);
            let dc = t_sub(p, &c);
            let x = t_dot(&dc, &u);
            let y = t_dot(&dc, &v);
            let mut phi = y.clone().atan2(x.clone());
            let zero = Tree::from(0.0);
            phi = select_lt(&phi, &zero, &(phi.clone() + std::f64::consts::TAU), &phi);
            let sweep = rec.sweep as f64;
            // Fußpunkt an φ_c = min(φ, sweep) (φ ≥ 0 bereits).
            let phi_c = phi.clone().min(sweep);
            let (s, co) = (phi_c.clone().sin(), phi_c.cos());
            let q = [
                c[0].clone() + (u[0].clone() * co.clone() + v[0].clone() * s.clone()) * rec.r as f64,
                c[1].clone() + (u[1].clone() * co.clone() + v[1].clone() * s.clone()) * rec.r as f64,
                c[2].clone() + (u[2].clone() * co + v[2].clone() * s) * rec.r as f64,
            ];
            // in_cap: φ ≤ sweep + cap.
            let cap = 1e-6;
            let cap_pass = le(&phi, &Tree::from(sweep + cap));
            // Radialnormale wall_sign·(q−c)/r.
            let dqc = [q[0].clone() - c[0].clone(), q[1].clone() - c[1].clone(), q[2].clone() - c[2].clone()];
            let nn = t_norm(&dqc).max(1e-9);
            let ws = rec.wall_sign as f64;
            let n_b = [
                dqc[0].clone() * nn.clone().recip() * ws,
                dqc[1].clone() * nn.clone().recip() * ws,
                dqc[2].clone() * nn.recip() * ws,
            ];
            (t_norm(&t_sub(p, &q)), q, cap_pass, n_b)
        };
        let dq = t_sub(p, &q);
        // Maske: Trust · Cap · Wand-Halbräume (n_a konstant, n_b variabel).
        let mut pass = le(&de, &Tree::from(rec.trust as f64)) * cap_pass;
        let eps = 1e-9;
        // n_a (konstant, |n| ≥ 1).
        {
            let dot = t_dot(&dq, &t_const3(rec.n_a));
            let ind = if concave { le(&dot, &Tree::from(eps)) } else { ge(&dot, &Tree::from(-eps)) };
            pass = pass * ind;
        }
        // n_b (Segment: konstant; Bogen: radial — beide |n| ≈ 1).
        {
            let dot = t_dot(&dq, &n_b);
            let ind = if concave { le(&dot, &Tree::from(eps)) } else { ge(&dot, &Tree::from(-eps)) };
            pass = pass * ind;
        }
        let de_eff = blend(&pass, &de, &Tree::from(big));
        let is_new = relu(best_de.clone().compare(de_eff.clone()));
        win_de = blend(&is_new, &de, &win_de);
        win_delta = blend(&is_new, &Tree::from(rec.delta as f64), &win_delta);
        best_de = blend(&is_new, &de_eff, &best_de);
        any = any.max(is_new);
    }
    let corr = if concave { win_delta - win_de } else { win_de - win_delta };
    let candidate = if concave { d.clone().min(corr) } else { d.clone().max(corr) };
    blend(&any, &candidate, d)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::csg::{CsgNode, EvalMode};
    use fidget::shape::EzShape;
    use fielddraft_protocol::FeatureId;

    /// offset_tree Phase B: der symbolische Kanal-Spiegel (`channel_to_tree`,
    /// compare-Selects + atan2) muss den CPU-Referenzwert der Baum-Eval
    /// reproduzieren — Steinmetz-Fixture mit echten Bögen, kantennahe Punkte.
    #[test]
    fn arc_channel_fidget_tree_matches_cpu() {
        let half = std::f32::consts::FRAC_1_SQRT_2;
        let cyl = |id: u32, r: f32, rot: [f32; 4]| {
            CsgNode::Transform(
                Transform {
                    translate: [0.0; 3],
                    rotation: rot,
                    scale: [1.0; 3],
                },
                Box::new(CsgNode::Leaf {
                    id: FeatureId::from_u32(id),
                    primitive: Primitive::Cylinder {
                        center: [0.0; 3],
                        radius: r,
                        half_height: 18.0,
                    },
                }),
            )
        };
        let node = CsgNode::Offset(
            Box::new(CsgNode::Intersection(
                Box::new(cyl(1, 9.0, [half, 0.0, 0.0, half])),
                Box::new(cyl(2, 6.0, [0.0, 0.0, -half, half])),
            )),
            1.5,
        );
        let bounded = crate::offset_channel::synthesize_offset_channels(
            crate::bvh::BoundedCsg::from_csg(&node),
            0.01,
        );
        let (n_ch, _) = crate::offset_channel::count_channels(&bounded);
        assert!(n_ch > 0, "Fixture ohne Kanäle");

        let (x, y, z) = Tree::axes();
        let tree = node_to_tree(&bounded, &[x, y, z]).expect("fidget-Übersetzung");
        let shape = VmShape::from(tree);
        let mut eval = VmShape::new_point_eval();
        let tape = shape.ez_point_tape();

        // Kantennahe + globale Punkte (deterministisch).
        let mut worst = 0.0f32;
        let mut k = 0u64;
        let mut next = || {
            k = k.wrapping_add(0x9E3779B97F4A7C15);
            let mut v = k;
            v = (v ^ (v >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
            (v >> 40) as f32 / (1u64 << 24) as f32 * 2.0 - 1.0
        };
        for _ in 0..800 {
            let th = next() * std::f32::consts::PI;
            let sx = if next() > 0.0 { 1.0 } else { -1.0 };
            let ey = 6.0 * th.cos();
            let ez = 6.0 * th.sin();
            let ex = sx * (81.0f32 - ey * ey).sqrt();
            let p = [
                ex + next() * 4.0,
                ey + next() * 4.0,
                ez + next() * 4.0,
            ];
            let d_cpu = bounded.sdf(p, EvalMode::Visual);
            let (d_fid, _) = eval.eval(&tape, p[0], p[1], p[2]).expect("fidget eval");
            worst = worst.max((d_cpu - d_fid).abs());
        }
        assert!(
            worst < 5e-4,
            "fidget-Kanal-Spiegel weicht ab: max |Δ| = {worst:.6}"
        );
    }

    /// offset_tree Phase C4: der symbolische Vertex-Kugel-Spiegel
    /// (`vertex_to_tree`) ist wertgleich zur CPU-Referenz.
    #[test]
    fn vertex_sphere_fidget_tree_matches_cpu() {
        use crate::csg::CsgNode;
        use fielddraft_protocol::{FeatureId, Primitive, Transform};
        let plate = |fid: u32, c: [f32; 3], h: [f32; 3]| {
            CsgNode::Transform(
                Transform {
                    translate: c,
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.0; 3],
                },
                Box::new(CsgNode::Leaf {
                    id: FeatureId::from_u32(fid),
                    primitive: Primitive::Box { center: [0.0; 3], half_extents: h },
                }),
            )
        };
        let corner = CsgNode::Offset(
            Box::new(CsgNode::Union(
                Box::new(CsgNode::Union(
                    Box::new(plate(1, [0.0, -5.0, 0.0], [20.0, 5.0, 20.0])),
                    Box::new(plate(2, [-5.0, 0.0, 0.0], [5.0, 20.0, 20.0])),
                )),
                Box::new(plate(3, [0.0, 0.0, -5.0], [20.0, 20.0, 5.0])),
            )),
            -1.5,
        );
        let bounded = crate::offset_channel::synthesize_offset_channels(
            crate::bvh::BoundedCsg::from_csg(&corner),
            0.01,
        );
        assert!(
            crate::offset_channel::count_vertex_spheres(&bounded) > 0,
            "Fixture ohne Vertex-Kugeln"
        );
        let (x, y, z) = Tree::axes();
        let tree = node_to_tree(&bounded, &[x, y, z]).expect("fidget-Übersetzung");
        let shape = VmShape::from(tree);
        let mut eval = VmShape::new_point_eval();
        let tape = shape.ez_point_tape();
        let mut seed = 0xC4F1u64;
        let mut next = move || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            ((seed >> 33) as f32 / (1u64 << 31) as f32) * 2.0 - 1.0
        };
        let mut worst = 0.0f32;
        for _ in 0..1500 {
            let p = [next() * 8.0, next() * 8.0, next() * 8.0];
            let d_cpu = bounded.sdf(p, EvalMode::Visual);
            let (d_fid, _) = eval.eval(&tape, p[0], p[1], p[2]).expect("fidget eval");
            worst = worst.max((d_cpu - d_fid).abs());
        }
        assert!(
            worst < 5e-4,
            "fidget-Vertex-Spiegel weicht ab: max |Δ| = {worst:.6}"
        );
    }
}
