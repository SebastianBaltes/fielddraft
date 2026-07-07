//! Fallback-patch machinery for faces WITHOUT a usable chart (Hybrid-PASM
//! plan §6, path F3): faces whose carrier has no 2D parametrisation
//! (Torus/Gyroid halfspaces, tessellated gyroid sheets, future custom SDFs)
//! or whose chart failed (profile fold guard) are meshed as an octree
//! dual-contouring PATCH over the face's attributed region, retreated by a
//! clearance band from the B-Rep rim, and then ZIPPED to the exact rim
//! chains. The rim vertices are the shared global skeleton slots, so the
//! seam is watertight BY CONSTRUCTION — same structural contract as the
//! per-face CDT, only the interior quality drops to DC level.
//!
//! Everything here works on plain triangle soups in OUTPUT index space; the
//! caller owns attribution, clearance and the patch triangulation itself
//! (`mesher_points::connect_octree_edges`).

use std::collections::HashMap;

/// Squared distance between two points.
fn d2(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
}

/// Distance from `p` to the closed polyline ring `ring` (segment-accurate).
pub(crate) fn dist_to_ring(p: [f32; 3], ring: &[[f32; 3]]) -> f32 {
    let n = ring.len();
    if n == 0 {
        return f32::MAX;
    }
    if n == 1 {
        return d2(p, ring[0]).sqrt();
    }
    let mut best = f32::MAX;
    for k in 0..n {
        let a = ring[k];
        let b = ring[(k + 1) % n];
        let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let len2 = ab[0] * ab[0] + ab[1] * ab[1] + ab[2] * ab[2];
        let t = if len2 > 0.0 {
            (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1] + (p[2] - a[2]) * ab[2]) / len2)
                .clamp(0.0, 1.0)
        } else {
            0.0
        };
        let q = [a[0] + t * ab[0], a[1] + t * ab[1], a[2] + t * ab[2]];
        best = best.min(d2(p, q));
    }
    best.sqrt()
}

/// Connected components of a triangle soup (shared-vertex connectivity).
/// Returns one component id per triangle (ids are arbitrary but dense-ish).
pub(crate) fn tri_components(tris: &[[u32; 3]]) -> Vec<usize> {
    // Union-find over the (sparse) vertex ids.
    let mut parent: HashMap<u32, u32> = HashMap::new();
    fn find(parent: &mut HashMap<u32, u32>, mut x: u32) -> u32 {
        let mut root = x;
        while let Some(&p) = parent.get(&root) {
            if p == root {
                break;
            }
            root = p;
        }
        // Path compression.
        while let Some(&p) = parent.get(&x) {
            if p == root {
                break;
            }
            parent.insert(x, root);
            x = p;
        }
        root
    }
    for t in tris {
        for &v in t {
            parent.entry(v).or_insert(v);
        }
        let r0 = find(&mut parent, t[0]);
        let r1 = find(&mut parent, t[1]);
        let r2 = find(&mut parent, t[2]);
        parent.insert(r1, r0);
        parent.insert(r2, r0);
    }
    let mut ids: HashMap<u32, usize> = HashMap::new();
    let mut out = Vec::with_capacity(tris.len());
    for t in tris {
        let r = find(&mut parent, t[0]);
        let next = ids.len();
        out.push(*ids.entry(r).or_insert(next));
    }
    out
}

/// Split a closed walk into SIMPLE cycles at pinch (bowtie) vertices: when
/// a vertex reappears, the sub-cycle between its two occurrences is cut out
/// as its own ring. Fan-filling or zipping a figure-8 ring would reuse its
/// pinch edges — simple cycles keep every downstream consumer safe.
fn split_simple(ring: Vec<u32>) -> Vec<Vec<u32>> {
    let mut out = Vec::new();
    let mut stack: Vec<u32> = Vec::new();
    let mut at: HashMap<u32, usize> = HashMap::new();
    for v in ring {
        if let Some(&k) = at.get(&v) {
            let sub: Vec<u32> = stack.drain(k..).collect();
            for s in &sub {
                at.remove(s);
            }
            if sub.len() >= 3 {
                out.push(sub);
            }
        }
        at.insert(v, stack.len());
        stack.push(v);
    }
    if stack.len() >= 3 {
        out.push(stack);
    }
    out
}

/// Extract the boundary loops of a triangle soup: edges used exactly once,
/// walked into closed rings. Open chains (dead ends) and degenerate rings
/// (< 3 vertices) are dropped — the caller detects the resulting hole via
/// its watertightness gate. At a bowtie vertex (4 boundary edges) the walk
/// picks an arbitrary unused continuation; the resulting walks are split
/// into SIMPLE cycles before they are returned.
pub(crate) fn boundary_loops(tris: &[[u32; 3]]) -> Vec<Vec<u32>> {
    let mut edge_use: HashMap<(u32, u32), u32> = HashMap::new();
    for t in tris {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            *edge_use.entry((a.min(b), a.max(b))).or_default() += 1;
        }
    }
    // Boundary adjacency: vertex → neighbours along boundary edges.
    let mut adj: HashMap<u32, Vec<u32>> = HashMap::new();
    for (&(a, b), &n) in &edge_use {
        if n == 1 {
            adj.entry(a).or_default().push(b);
            adj.entry(b).or_default().push(a);
        }
    }
    let mut used: std::collections::HashSet<(u32, u32)> = std::collections::HashSet::new();
    let mut loops: Vec<Vec<u32>> = Vec::new();
    let mut starts: Vec<u32> = adj.keys().copied().collect();
    starts.sort_unstable(); // deterministic order
    for &s in &starts {
        let Some(nbrs) = adj.get(&s) else { continue };
        for &first in nbrs {
            let key = (s.min(first), s.max(first));
            if used.contains(&key) {
                continue;
            }
            used.insert(key);
            let mut ring = vec![s, first];
            let (mut prev, mut cur) = (s, first);
            let closed = loop {
                let Some(cand) = adj.get(&cur) else { break false };
                let next = cand
                    .iter()
                    .copied()
                    .find(|&x| x != prev && !used.contains(&(cur.min(x), cur.max(x))));
                let Some(next) = next else { break false };
                used.insert((cur.min(next), cur.max(next)));
                if next == s {
                    break true;
                }
                ring.push(next);
                prev = cur;
                cur = next;
            };
            if closed && ring.len() >= 3 {
                loops.extend(split_simple(ring));
            }
        }
    }
    loops
}

/// Stitch two CLOSED rings of vertex indices with a greedy shortest-diagonal
/// zipper. Produces `a.len() + b.len()` triangles forming a watertight band:
/// every `a`-edge and every `b`-edge is used exactly once, every diagonal
/// exactly twice. `pos` maps an index to its world position. The rings may
/// run in opposite directions — the relative orientation is detected from
/// the starting neighbourhood.
pub(crate) fn zip_rings(a: &[u32], b: &[u32], pos: &dyn Fn(u32) -> [f32; 3]) -> Vec<[u32; 3]> {
    let (na, nb) = (a.len(), b.len());
    if na < 3 || nb < 3 {
        return Vec::new();
    }
    // Closest pair = the zipper's anchor.
    let (mut i0, mut j0, mut best) = (0usize, 0usize, f32::MAX);
    for i in 0..na {
        for j in 0..nb {
            let d = d2(pos(a[i]), pos(b[j]));
            if d < best {
                best = d;
                i0 = i;
                j0 = j;
            }
        }
    }
    // Align directions: advance `a` one step; `b` should advance towards it.
    let a1 = pos(a[(i0 + 1) % na]);
    let fwd = d2(a1, pos(b[(j0 + 1) % nb])) <= d2(a1, pos(b[(j0 + nb - 1) % nb]));
    let ra: Vec<u32> = (0..na).map(|k| a[(i0 + k) % na]).collect();
    let rb: Vec<u32> = if fwd {
        (0..nb).map(|k| b[(j0 + k) % nb]).collect()
    } else {
        (0..nb).map(|k| b[(j0 + nb - k) % nb]).collect()
    };
    let mut tris = Vec::with_capacity(na + nb);
    let (mut ai, mut bj) = (0usize, 0usize);
    while ai < na || bj < nb {
        let va = ra[ai % na];
        let vb = rb[bj % nb];
        let advance_a = if bj >= nb {
            true
        } else if ai >= na {
            false
        } else {
            d2(pos(ra[(ai + 1) % na]), pos(vb)) <= d2(pos(va), pos(rb[(bj + 1) % nb]))
        };
        let t = if advance_a {
            let vn = ra[(ai + 1) % na];
            ai += 1;
            [va, vb, vn]
        } else {
            let vn = rb[(bj + 1) % nb];
            bj += 1;
            [va, vb, vn]
        };
        if t[0] != t[1] && t[1] != t[2] && t[2] != t[0] {
            tris.push(t);
        }
    }
    tris
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two concentric squares (8 vertices each, offset ring) zip into a
    /// watertight band: ring edges once, diagonals twice.
    #[test]
    fn zip_rings_band_is_watertight() {
        // Outer ring 0..8 (r=2), inner ring 8..16 (r=1), both in the z=0
        // plane; inner deliberately rotated and reversed.
        let mut positions: Vec<[f32; 3]> = Vec::new();
        for k in 0..8 {
            let a = std::f32::consts::TAU * k as f32 / 8.0;
            positions.push([2.0 * a.cos(), 2.0 * a.sin(), 0.0]);
        }
        for k in 0..8 {
            let a = std::f32::consts::TAU * (k as f32 + 0.4) / 8.0;
            positions.push([a.cos(), a.sin(), 1.0]);
        }
        let ring_a: Vec<u32> = (0..8).collect();
        let ring_b: Vec<u32> = (8..16).rev().collect(); // opposite direction
        let pos = |i: u32| positions[i as usize];
        let tris = zip_rings(&ring_a, &ring_b, &pos);
        assert_eq!(tris.len(), 16, "zipper emits |a| + |b| triangles");
        let mut edge_use: HashMap<(u32, u32), u32> = HashMap::new();
        for t in &tris {
            for (x, y) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                *edge_use.entry((x.min(y), x.max(y))).or_default() += 1;
            }
        }
        let ring_edge = |r: &[u32], k: usize| {
            let (x, y) = (r[k], r[(k + 1) % r.len()]);
            (x.min(y), x.max(y))
        };
        for k in 0..8 {
            assert_eq!(edge_use[&ring_edge(&ring_a, k)], 1, "outer ring edge once");
            assert_eq!(edge_use[&ring_edge(&ring_b, k)], 1, "inner ring edge once");
        }
        for (_, &n) in &edge_use {
            assert!(n <= 2, "no edge more than twice");
        }
    }

    /// A 2×2 quad grid (8 triangles) has exactly one boundary loop with the
    /// 8 rim vertices.
    #[test]
    fn boundary_loops_of_a_grid_patch() {
        // Vertices 0..9 laid out 3×3; quads split into tris.
        let q = |a: u32, b: u32, c: u32, d: u32| [[a, b, c], [a, c, d]];
        let mut tris = Vec::new();
        for (a, b, c, d) in [(0, 1, 4, 3), (1, 2, 5, 4), (3, 4, 7, 6), (4, 5, 8, 7)] {
            tris.extend_from_slice(&q(a, b, c, d));
        }
        let loops = boundary_loops(&tris);
        assert_eq!(loops.len(), 1, "one closed rim");
        assert_eq!(loops[0].len(), 8, "8 rim vertices (centre 4 is interior)");
        assert!(!loops[0].contains(&4), "interior vertex not on the rim");
    }

    #[test]
    fn tri_components_split_disjoint_patches() {
        let tris = vec![[0, 1, 2], [1, 2, 3], [10, 11, 12]];
        let comp = tri_components(&tris);
        assert_eq!(comp[0], comp[1]);
        assert_ne!(comp[0], comp[2]);
    }

    #[test]
    fn dist_to_ring_measures_segments() {
        let ring = vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 10.0, 0.0], [0.0, 10.0, 0.0]];
        // Point above the middle of the bottom edge: distance 2 to the
        // SEGMENT (a vertex-only test would report √29).
        let d = dist_to_ring([5.0, 2.0, 0.0], &ring);
        assert!((d - 2.0).abs() < 1e-5, "segment distance, got {d}");
    }
}
