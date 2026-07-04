//! Built-in QEM decimation — the from-scratch alternative to the
//! `baby_shark`-backed [`crate::decimate::simplify_qem`].
//!
//! Garland & Heckbert quadric error metrics with cheapest-first edge
//! collapse, bounded by the accumulated squared-distance error. Two
//! invariants keep the export watertight on every collapse:
//!
//!  * **Link condition** — an interior manifold edge `(u,v)` collapses
//!    only if `u` and `v` share exactly the two edge-opposite vertices
//!    (no non-local seam weld).
//!  * **No degenerate / no flip** — every surviving ring triangle keeps
//!    its facing (new normal within 90° of the old) AND stays non-sliver
//!    (no edge collapses to ~zero length, which the position-welding
//!    validator would flag as a degenerate triangle).
//!
//! Determinism: the ring of vertices re-priced after a collapse is
//! gathered through a `BTreeSet`, so the collapse order (and thus the
//! output mesh) is identical run to run — the export must be
//! reproducible.

use crate::decimate::DecimateStats;
use crate::dc::Mesh;
use std::collections::{BTreeSet, BinaryHeap};

/// Reject a collapse that would shrink any surviving triangle edge below
/// this length (mm) — such an edge welds to a degenerate triangle.
const MIN_EDGE: f64 = 1e-5;

/// Symmetric 4×4 quadric as 10 upper-triangular coefficients:
/// `[a², ab, ac, ad, b², bc, bd, c², cd, d²]` for plane `(a,b,c,d)`.
type Quadric = [f64; 10];

fn quadric_from_plane(n: [f64; 3], d: f64) -> Quadric {
    let (a, b, c) = (n[0], n[1], n[2]);
    [
        a * a,
        a * b,
        a * c,
        a * d,
        b * b,
        b * c,
        b * d,
        c * c,
        c * d,
        d * d,
    ]
}

fn quadric_add(mut q: Quadric, r: &Quadric) -> Quadric {
    for i in 0..10 {
        q[i] += r[i];
    }
    q
}

fn quadric_error(q: &Quadric, x: [f64; 3]) -> f64 {
    let (px, py, pz) = (x[0], x[1], x[2]);
    q[0] * px * px
        + 2.0 * q[1] * px * py
        + 2.0 * q[2] * px * pz
        + 2.0 * q[3] * px
        + q[4] * py * py
        + 2.0 * q[5] * py * pz
        + 2.0 * q[6] * py
        + q[7] * pz * pz
        + 2.0 * q[8] * pz
        + q[9]
}

fn optimal_position(q: &Quadric) -> Option<[f64; 3]> {
    let a = [[q[0], q[1], q[2]], [q[1], q[4], q[5]], [q[2], q[5], q[7]]];
    let det = a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1])
        - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
        + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0]);
    if det.abs() < 1e-10 {
        return None;
    }
    let rhs = [-q[3], -q[6], -q[8]];
    let col = |c: usize| -> [[f64; 3]; 3] {
        let mut m = a;
        for r in 0..3 {
            m[r][c] = rhs[r];
        }
        m
    };
    let det3 = |m: &[[f64; 3]; 3]| -> f64 {
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    };
    Some([det3(&col(0)) / det, det3(&col(1)) / det, det3(&col(2)) / det])
}

struct HeapEntry {
    neg_cost: f64,
    u: usize,
    v: usize,
    ver_u: u32,
    ver_v: u32,
    target: [f64; 3],
}
impl PartialEq for HeapEntry {
    fn eq(&self, o: &Self) -> bool {
        self.neg_cost == o.neg_cost
    }
}
impl Eq for HeapEntry {}
impl PartialOrd for HeapEntry {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for HeapEntry {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        self.neg_cost
            .partial_cmp(&o.neg_cost)
            .unwrap_or(std::cmp::Ordering::Equal)
            // Tie-break on vertex ids so equal-cost collapses pop in a
            // fixed order — reinforces run-to-run determinism.
            .then(self.u.cmp(&o.u))
            .then(self.v.cmp(&o.v))
    }
}

struct Decimator {
    verts: Vec<[f64; 3]>,
    alive_v: Vec<bool>,
    ver: Vec<u32>,
    quad: Vec<Quadric>,
    tris: Vec<Option<[usize; 3]>>,
    inc: Vec<Vec<usize>>,
    heap: BinaryHeap<HeapEntry>,
    max_error_sq: f64,
}

impl Decimator {
    fn tri_normal(&self, t: [usize; 3]) -> ([f64; 3], f64) {
        normal_area(self.verts[t[0]], self.verts[t[1]], self.verts[t[2]])
    }

    /// Normal + min-edge-length of triangle `t` with `moved`/`other`
    /// relocated to `p`.
    fn tri_probe_moved(
        &self,
        t: [usize; 3],
        moved: usize,
        other: usize,
        p: [f64; 3],
    ) -> ([f64; 3], f64) {
        let pos = |i: usize| -> [f64; 3] {
            if i == moved || i == other {
                p
            } else {
                self.verts[i]
            }
        };
        let (a, b, c) = (pos(t[0]), pos(t[1]), pos(t[2]));
        let (n, _) = normal_area(a, b, c);
        let min_edge = dist(a, b).min(dist(b, c)).min(dist(c, a));
        (n, min_edge)
    }

    fn edge_triangles(&self, u: usize, v: usize) -> (Vec<usize>, Vec<usize>) {
        let mut opp = Vec::new();
        let mut tri_ids = Vec::new();
        for &t in &self.inc[u] {
            if let Some(tri) = self.tris[t] {
                if tri.contains(&v) {
                    tri_ids.push(t);
                    for &w in &tri {
                        if w != u && w != v {
                            opp.push(w);
                        }
                    }
                }
            }
        }
        (opp, tri_ids)
    }

    fn collapse_valid(&self, u: usize, v: usize, p: [f64; 3]) -> bool {
        let (opp, edge_tris) = self.edge_triangles(u, v);
        if edge_tris.len() != 2 {
            return false; // interior manifold edge only
        }
        // Link condition.
        let mut ring_u = BTreeSet::new();
        for &t in &self.inc[u] {
            if let Some(tri) = self.tris[t] {
                for &w in &tri {
                    if w != u {
                        ring_u.insert(w);
                    }
                }
            }
        }
        for &t in &self.inc[v] {
            if let Some(tri) = self.tris[t] {
                for &w in &tri {
                    if w != v && w != u && ring_u.contains(&w) && !opp.contains(&w) {
                        return false;
                    }
                }
            }
        }
        // No flip, no sliver on any surviving incident triangle.
        for &vtx in &[u, v] {
            let other = if vtx == u { v } else { u };
            for &t in &self.inc[vtx] {
                if let Some(tri) = self.tris[t] {
                    if tri.contains(&u) && tri.contains(&v) {
                        continue; // collapses away
                    }
                    let (old_n, old_area) = self.tri_normal(tri);
                    if old_area < 1e-20 {
                        continue;
                    }
                    let (new_n, min_edge) = self.tri_probe_moved(tri, vtx, other, p);
                    if min_edge < MIN_EDGE {
                        return false; // would weld to a degenerate triangle
                    }
                    let dot = old_n[0] * new_n[0] + old_n[1] * new_n[1] + old_n[2] * new_n[2];
                    if dot < 1e-3 {
                        return false;
                    }
                }
            }
        }
        true
    }

    fn edge_cost(&self, u: usize, v: usize) -> (f64, [f64; 3]) {
        let q = quadric_add(self.quad[u], &self.quad[v]);
        let mut best = self.verts[v];
        let mut best_e = quadric_error(&q, best);
        for &c in &[self.verts[u], mid(self.verts[u], self.verts[v])] {
            let e = quadric_error(&q, c);
            if e < best_e {
                best_e = e;
                best = c;
            }
        }
        if let Some(c) = optimal_position(&q) {
            let e = quadric_error(&q, c);
            if e < best_e {
                best_e = e;
                best = c;
            }
        }
        (best_e.max(0.0), best)
    }

    fn push_edge(&mut self, u: usize, v: usize) {
        if u == v || !self.alive_v[u] || !self.alive_v[v] {
            return;
        }
        let (cost, target) = self.edge_cost(u, v);
        self.heap.push(HeapEntry {
            neg_cost: -cost,
            u,
            v,
            ver_u: self.ver[u],
            ver_v: self.ver[v],
            target,
        });
    }

    fn apply_collapse(&mut self, u: usize, v: usize, p: [f64; 3]) -> Vec<usize> {
        self.verts[v] = p;
        self.quad[v] = quadric_add(self.quad[u], &self.quad[v]);

        let u_tris = self.inc[u].clone();
        for t in u_tris {
            if let Some(mut tri) = self.tris[t] {
                if tri.contains(&v) {
                    self.tris[t] = None;
                } else {
                    for w in tri.iter_mut() {
                        if *w == u {
                            *w = v;
                        }
                    }
                    self.tris[t] = Some(tri);
                    self.inc[v].push(t);
                }
            }
        }
        self.alive_v[u] = false;
        self.inc[u].clear();

        self.inc[v].retain(|&t| self.tris[t].is_some());
        // Deterministic ring order (BTreeSet) → reproducible collapses.
        let mut ring = BTreeSet::new();
        for &t in &self.inc[v] {
            if let Some(tri) = self.tris[t] {
                for &w in &tri {
                    if w != v {
                        ring.insert(w);
                    }
                }
            }
        }
        self.ver[v] += 1;
        for &w in &ring {
            self.ver[w] += 1;
        }
        ring.into_iter().collect()
    }
}

fn normal_area(a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> ([f64; 3], f64) {
    let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let n = [
        ab[1] * ac[2] - ab[2] * ac[1],
        ab[2] * ac[0] - ab[0] * ac[2],
        ab[0] * ac[1] - ab[1] * ac[0],
    ];
    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    if len < 1e-20 {
        ([0.0, 0.0, 0.0], 0.0)
    } else {
        ([n[0] / len, n[1] / len, n[2] / len], len)
    }
}

fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

fn mid(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [0.5 * (a[0] + b[0]), 0.5 * (a[1] + b[1]), 0.5 * (a[2] + b[2])]
}

/// Decimate `mesh` in place with error-bounded QEM edge collapses.
/// Manifold + watertight preserving; per-vertex normals recomputed from
/// the survivors. Mirrors [`crate::decimate::simplify_qem`]'s signature.
pub fn simplify_qem_builtin(mesh: &mut Mesh, max_error_sq: f32) -> DecimateStats {
    let vcount = mesh.positions.len() / 3;
    let tcount = mesh.indices.len() / 3;
    if vcount == 0 || tcount == 0 {
        return DecimateStats {
            collapses: 0,
            triangles_before: tcount,
            triangles_after: tcount,
        };
    }

    let verts: Vec<[f64; 3]> = (0..vcount)
        .map(|i| {
            [
                mesh.positions[i * 3] as f64,
                mesh.positions[i * 3 + 1] as f64,
                mesh.positions[i * 3 + 2] as f64,
            ]
        })
        .collect();
    let tris: Vec<Option<[usize; 3]>> = (0..tcount)
        .map(|t| {
            Some([
                mesh.indices[t * 3] as usize,
                mesh.indices[t * 3 + 1] as usize,
                mesh.indices[t * 3 + 2] as usize,
            ])
        })
        .collect();

    let mut inc: Vec<Vec<usize>> = vec![Vec::new(); vcount];
    for (t, tri) in tris.iter().enumerate() {
        if let Some(tri) = tri {
            for &w in tri {
                inc[w].push(t);
            }
        }
    }

    let mut dec = Decimator {
        verts,
        alive_v: vec![true; vcount],
        ver: vec![0; vcount],
        quad: vec![[0.0; 10]; vcount],
        tris,
        inc,
        heap: BinaryHeap::new(),
        max_error_sq: max_error_sq as f64,
    };

    for t in 0..tcount {
        if let Some(tri) = dec.tris[t] {
            let (n, area) = dec.tri_normal(tri);
            if area < 1e-20 {
                continue;
            }
            let a = dec.verts[tri[0]];
            let d = -(n[0] * a[0] + n[1] * a[1] + n[2] * a[2]);
            let q = quadric_from_plane(n, d);
            for &w in &tri {
                dec.quad[w] = quadric_add(dec.quad[w], &q);
            }
        }
    }

    let mut seen = BTreeSet::new();
    for t in 0..tcount {
        if let Some(tri) = dec.tris[t] {
            for e in 0..3 {
                let (mut a, mut b) = (tri[e], tri[(e + 1) % 3]);
                if a > b {
                    std::mem::swap(&mut a, &mut b);
                }
                if seen.insert((a, b)) {
                    dec.push_edge(a, b);
                }
            }
        }
    }

    let mut collapses = 0usize;
    while let Some(entry) = dec.heap.pop() {
        let cost = -entry.neg_cost;
        if cost > dec.max_error_sq {
            break;
        }
        let (u, v) = (entry.u, entry.v);
        if !dec.alive_v[u]
            || !dec.alive_v[v]
            || dec.ver[u] != entry.ver_u
            || dec.ver[v] != entry.ver_v
        {
            continue;
        }
        if !dec.collapse_valid(u, v, entry.target) {
            continue;
        }
        let ring = dec.apply_collapse(u, v, entry.target);
        collapses += 1;
        for &w in &ring {
            dec.push_edge(v, w);
        }
    }

    // Rebuild compacted mesh.
    let mut remap = vec![u32::MAX; vcount];
    let mut positions: Vec<f32> = Vec::new();
    for i in 0..vcount {
        if dec.alive_v[i] {
            remap[i] = (positions.len() / 3) as u32;
            positions.push(dec.verts[i][0] as f32);
            positions.push(dec.verts[i][1] as f32);
            positions.push(dec.verts[i][2] as f32);
        }
    }
    let mut indices: Vec<u32> = Vec::new();
    for tri in dec.tris.iter().flatten() {
        if tri[0] == tri[1] || tri[1] == tri[2] || tri[0] == tri[2] {
            continue;
        }
        indices.push(remap[tri[0]]);
        indices.push(remap[tri[1]]);
        indices.push(remap[tri[2]]);
    }

    let vc = positions.len() / 3;
    let mut normals = vec![0.0f32; positions.len()];
    for t in indices.chunks_exact(3) {
        let (i, j, k) = (t[0] as usize, t[1] as usize, t[2] as usize);
        let p = |a: usize| [positions[a * 3], positions[a * 3 + 1], positions[a * 3 + 2]];
        let (a, b, c) = (p(i), p(j), p(k));
        let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let n = [
            ab[1] * ac[2] - ab[2] * ac[1],
            ab[2] * ac[0] - ab[0] * ac[2],
            ab[0] * ac[1] - ab[1] * ac[0],
        ];
        for &idx in &[i, j, k] {
            normals[idx * 3] += n[0];
            normals[idx * 3 + 1] += n[1];
            normals[idx * 3 + 2] += n[2];
        }
    }
    for i in 0..vc {
        let n = [normals[i * 3], normals[i * 3 + 1], normals[i * 3 + 2]];
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if len > 1e-12 {
            normals[i * 3] = n[0] / len;
            normals[i * 3 + 1] = n[1] / len;
            normals[i * 3 + 2] = n[2] / len;
        } else {
            normals[i * 3 + 1] = 1.0;
        }
    }

    let triangles_after = indices.len() / 3;
    mesh.positions = positions;
    mesh.normals = normals;
    mesh.indices = indices;
    mesh.classifications = Vec::new();
    mesh.contributors = Vec::new();
    mesh.face_index = Vec::new();

    DecimateStats {
        collapses,
        triangles_before: tcount,
        triangles_after,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh_validate::validate_topology;

    fn octahedron() -> Mesh {
        let positions = vec![
            1.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0,
            -1.0,
        ];
        let indices = vec![
            0, 2, 4, 2, 1, 4, 1, 3, 4, 3, 0, 4, 2, 0, 5, 1, 2, 5, 3, 1, 5, 0, 3, 5,
        ];
        Mesh {
            positions,
            normals: vec![0.0; 18],
            indices,
            ..Mesh::default()
        }
    }

    #[test]
    fn builtin_decimation_preserves_watertightness() {
        let mut mesh = octahedron();
        assert!(validate_topology(&mesh).watertight);
        simplify_qem_builtin(&mut mesh, 100.0);
        let v = validate_topology(&mesh);
        assert_eq!(v.boundary_edges, 0);
        assert_eq!(v.non_manifold_edges, 0);
        assert_eq!(v.degenerate_triangles, 0, "sliver guard must hold");
        assert!(mesh.triangle_count() == 0 || v.watertight);
    }

    #[test]
    fn builtin_empty_mesh_is_noop() {
        let mut mesh = Mesh::default();
        let stats = simplify_qem_builtin(&mut mesh, 1.0);
        assert_eq!(stats.collapses, 0);
    }
}
