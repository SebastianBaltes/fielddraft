//! Face boundary-loop ordering: the reflex-aware turning-angle walk
//! (DL-58), ring metrics, and outer/inner classification with manifold
//! winding.


use crate::brep::{
    BRep, EdgeGeometry, EdgeId,
    SurfaceGeometry, VertexId,
};
use fielddraft_protocol::HalfSpace;

// ─── small vector helpers (f64): shared, see brep::vec3 ─────────────────────
use crate::brep::vec3::{normalize as unit, sub};
use crate::brep::vec3::norm;
use super::*;

/// One ordered boundary ring: `(edge, reversed)` entries, head-to-tail
/// connected. The pervasive currency of loop ordering.
pub(super) type Ring = Vec<(EdgeId, bool)>;

/// The ordered boundary loops of one face: an outer ring plus zero or more
/// inner (hole) rings, all guaranteed connected and consistently wound.
pub(super) struct FaceLoops {
    pub(super) outer: Ring,
    pub(super) inner: Vec<Ring>,
}

/// In-plane orthonormal basis `(u, v)` with `u × v = normal`, for projecting
/// ring points into the tangent plane of `normal`.
fn plane_basis(normal: [f64; 3]) -> ([f64; 3], [f64; 3]) {
    let u = {
        let a = if normal[0].abs() < 0.9 {
            [1.0, 0.0, 0.0]
        } else {
            [0.0, 1.0, 0.0]
        };
        unit(project_tangent(a, normal))
    };
    let v = [
        normal[1] * u[2] - normal[2] * u[1],
        normal[2] * u[0] - normal[0] * u[2],
        normal[0] * u[1] - normal[1] * u[0],
    ];
    (u, v)
}

/// Shoelace signed area of a closed 2D polygon.
fn shoelace_area(uv: &[(f64, f64)]) -> f64 {
    let mut area = 0.0;
    for i in 0..uv.len() {
        let (x0, y0) = uv[i];
        let (x1, y1) = uv[(i + 1) % uv.len()];
        area += x0 * y1 - x1 * y0;
    }
    area * 0.5
}

/// Unwrap a periodic `u` against its predecessor so a sequence crossing the
/// ±π seam stays continuous.
fn unwrap_u(mut u: f64, prev_u: f64) -> f64 {
    use std::f64::consts::{PI, TAU};
    while u - prev_u > PI {
        u -= TAU;
    }
    while u - prev_u < -PI {
        u += TAU;
    }
    u
}

/// Order one connected ring out of `pool` (indices into `edge_ids`), starting at
/// `seed`, using **turning-angle disambiguation** so reflex vertices (where ≥3
/// of the face's edges meet) are traversed correctly. At each step we sit at the
/// current walk vertex `cur`, having arrived along reversed-incoming tangent
/// `back` (the direction pointing back the way we came, in the face tangent
/// plane); among all unused candidate edges incident to `cur` we pick the one
/// whose outgoing tangent makes the **largest CCW turn** from `back` about the
/// face normal — i.e. the next edge encountered sweeping counter-clockwise,
/// which keeps the face interior consistently on one side and hugs the local
/// boundary even at a reflex vertex or a vertex shared with a hole.
///
/// Marks consumed indices in `used`. Returns the ordered ring, or `None` if the
/// chain cannot close (degenerate topology).
#[allow(clippy::too_many_arguments)]
pub(super) fn order_one_ring(
    brep: &BRep,
    edge_ids: &[EdgeId],
    used: &mut [bool],
    seed: usize,
    normal_at: &dyn Fn([f64; 3]) -> Option<[f64; 3]>,
) -> Option<Vec<(EdgeId, bool)>> {
    let endpoints = |e: EdgeId| -> Option<(VertexId, VertexId)> {
        let edge = brep.edge(e);
        Some((edge.start?, edge.end?))
    };

    let (first_start, mut cur) = endpoints(edge_ids[seed])?;
    used[seed] = true;
    let mut ring: Vec<(EdgeId, bool)> = vec![(edge_ids[seed], false)];
    // Reversed-incoming tangent: pointing from the new `cur` back toward the
    // seed's start (i.e. the negated outgoing tangent of the seed edge).
    let mut back = outgoing_tangent(brep, edge_ids[seed], cur, true)?;

    loop {
        // Gather all unused candidate edges incident to `cur`.
        let mut best: Option<(usize, bool, f64)> = None; // (index, reversed, angle)
        let normal = normal_at(brep.vertex(cur).position);
        for (i, &eid) in edge_ids.iter().enumerate() {
            if used[i] {
                continue;
            }
            let (s, e) = endpoints(eid)?;
            let reversed = if s == cur {
                false
            } else if e == cur {
                true
            } else {
                continue;
            };
            let out = match outgoing_tangent(brep, eid, cur, reversed) {
                Some(t) => t,
                None => continue,
            };
            let angle = match normal {
                Some(n) => {
                    let bt = project_tangent(back, n);
                    let ot = project_tangent(out, n);
                    if norm(bt) < 1e-9 || norm(ot) < 1e-9 {
                        // Degenerate projection: fall back to "any" (angle 0).
                        0.0
                    } else {
                        // Largest CCW turn from `back` to the outgoing tangent.
                        // Map into [0, 2π) so the maximum is the sharpest hug.
                        let a = signed_angle(bt, ot, n);
                        if a <= 0.0 {
                            a + std::f64::consts::TAU
                        } else {
                            a
                        }
                    }
                }
                None => 0.0, // no usable normal → first-match (greedy) behaviour
            };
            let better = match best {
                None => true,
                Some((_, _, ba)) => angle > ba,
            };
            if better {
                best = Some((i, reversed, angle));
            }
            if normal.is_none() {
                break; // greedy fallback: take the first incident edge
            }
        }

        let (i, reversed, _) = best?;
        used[i] = true;
        let (s, e) = endpoints(edge_ids[i])?;
        let next = if reversed { s } else { e };
        ring.push((edge_ids[i], reversed));
        // New reversed-incoming tangent at `next`: negated outgoing tangent of
        // the edge we just traversed, i.e. its outgoing tangent leaving `next`
        // when oriented the *opposite* way.
        back = outgoing_tangent(brep, edge_ids[i], next, !reversed)?;
        cur = next;

        if cur == first_start {
            return Some(ring);
        }
    }
}

/// Signed area of a ring projected into the tangent plane defined by `normal`,
/// using vertex positions. Positive = the ring winds counter-clockwise seen
/// from the side `normal` points toward. Edges without bounding vertices (full
/// circles) are skipped; a single closed-circle edge yields area 0 and is
/// classified by length elsewhere. Production classification samples rings
/// densely and takes the shoelace area in the tangent plane
/// ([`group_rings_planar`]); this chord version remains the tests'
/// winding oracle.
#[cfg(test)]
pub(super) fn ring_signed_area(brep: &BRep, ring: &[(EdgeId, bool)], normal: [f64; 3]) -> f64 {
    let (u, v) = plane_basis(normal);
    let proj = |p: [f64; 3]| -> (f64, f64) {
        (
            p[0] * u[0] + p[1] * u[1] + p[2] * u[2],
            p[0] * v[0] + p[1] * v[1] + p[2] * v[2],
        )
    };
    let mut area = 0.0;
    for &(eid, reversed) in ring {
        let edge = brep.edge(eid);
        let (Some(s), Some(e)) = (edge.start, edge.end) else {
            continue;
        };
        let (a, b) = if reversed { (e, s) } else { (s, e) };
        let (x0, y0) = proj(brep.vertex(a).position);
        let (x1, y1) = proj(brep.vertex(b).position);
        area += x0 * y1 - x1 * y0;
    }
    area * 0.5
}

/// Total chord length of a ring. Used to rank the *full-revolution* trim
/// circles of a curved wall (cylinder/cone tube) against each other — among
/// those periodic boundaries the longest is taken as the outer bound, the rest
/// as inner (DL-58). Bounded (vertex-delimited) loops are classified instead by
/// parametric signed area ([`ring_uv_area`]), which world-space chord length
/// would misrank on a tapered surface.
pub(super) fn ring_extent(brep: &BRep, ring: &[(EdgeId, bool)]) -> f64 {
    let mut len = 0.0;
    for &(eid, _) in ring {
        let edge = brep.edge(eid);
        if let (Some(s), Some(e)) = (edge.start, edge.end) {
            len += norm(sub(brep.vertex(s).position, brep.vertex(e).position));
        } else {
            // Closed-circle edge: use its control polygon span as a proxy.
            if let EdgeGeometry::Nurbs(c) = &edge.geometry {
                for w in c.control_points.windows(2) {
                    len += norm(sub(w[0], w[1]));
                }
            }
        }
    }
    len
}

/// Parametric signed area of a bounded ring, computed in the surface's `(u, v)`
/// domain ([`surface_uv`]). This is the curved-surface analogue of
/// [`ring_signed_area`]: it measures how much *parameter* area the loop
/// encloses, which — unlike world-space chord length — is invariant to a
/// tapered surface's varying radius, so the true outer boundary outranks holes
/// regardless of their physical size. `u` is unwrapped across the periodic seam
/// so the shoelace stays continuous. Returns `None` if any vertex has no
/// `(u, v)` (e.g. a full-revolution circle edge, or an unparameterized surface).
pub(super) fn ring_uv_area(brep: &BRep, ring: &[(EdgeId, bool)], surface: &SurfaceGeometry) -> Option<f64> {
    // Collect the ring's ordered tail vertices (b of edge i == a of edge i+1,
    // so the tails are the polygon vertices), mapped to (u, v) and unwrapped.
    let mut uv: Vec<(f64, f64)> = Vec::with_capacity(ring.len());
    for &(eid, reversed) in ring {
        let edge = brep.edge(eid);
        let (Some(s), Some(e)) = (edge.start, edge.end) else {
            return None;
        };
        let tail = if reversed { e } else { s };
        let (mut u, v) = surface_uv(surface, brep.vertex(tail).position)?;
        if let Some(&(prev_u, _)) = uv.last() {
            u = unwrap_u(u, prev_u);
        }
        uv.push((u, v));
    }
    if uv.len() < 3 {
        return None;
    }
    Some(shoelace_area(&uv))
}

/// Concatenated, traversal-oriented dense sample points of a ring (curve
/// tessellation via [`crate::brep::edge_sampling::sample_edge`]). Unlike the
/// vertex-only chords of [`ring_signed_area`], this gives closed rim circles
/// (no bounding vertices) a usable polygon, so their winding has a sign.
pub(super) fn ring_sample_points(brep: &BRep, ring: &[(EdgeId, bool)]) -> Vec<[f64; 3]> {
    let mut pts: Vec<[f64; 3]> = Vec::new();
    for &(eid, reversed) in ring {
        let mut s = crate::brep::edge_sampling::sample_edge(&brep.edge(eid).geometry);
        if reversed {
            s.reverse();
        }
        pts.extend(s);
    }
    pts
}

/// Winding metrics of a ring in the surface's `(u, v)` domain, with `u`
/// unwrapped across the periodic seam (see [`ring_uv_metrics`]).
///
/// Sign convention: the `(u = atan2(z, x), v)` parameterizations of
/// [`surface_uv`] are all **left-handed** with respect to the
/// primitive-outward normal (`∂p/∂u × ∂p/∂v` points inward, since `u`
/// increases clockwise about local +Y) — "face on the left in 3D" therefore
/// maps to "face on the RIGHT in the uv plane".
#[derive(Clone, Copy)]
pub(super) struct RingUvMetrics {
    /// ≈ 0 for a loop that closes in the parameter domain (bounded), ±2π for
    /// a full-revolution rim around the axis.
    pub(super) net_du: f64,
    /// Locates the ring along the second parameter.
    pub(super) mean_v: f64,
    /// Signed area of the loop closed in unwrapped coordinates — the winding
    /// signal for bounded loops (~0 for rims).
    pub(super) shoelace: f64,
}

/// Compute [`RingUvMetrics`] from dense ring samples. Parametric poles (e.g.
/// a sphere cap apex sample) are skipped; returns `None` below 3 usable
/// samples or on unparameterized surfaces.
pub(super) fn ring_uv_metrics(points: &[[f64; 3]], surface: &SurfaceGeometry) -> Option<RingUvMetrics> {
    let mut uv: Vec<(f64, f64)> = Vec::with_capacity(points.len() + 1);
    for p in points {
        let Some((mut u, v)) = surface_uv(surface, *p) else {
            continue; // parametric pole (e.g. a sphere cap apex sample)
        };
        if let Some(&(prev_u, _)) = uv.last() {
            u = unwrap_u(u, prev_u);
        }
        uv.push((u, v));
    }
    if uv.len() < 3 {
        return None;
    }
    Some(RingUvMetrics {
        net_du: uv.last().unwrap().0 - uv[0].0,
        mean_v: uv.iter().map(|&(_, v)| v).sum::<f64>() / uv.len() as f64,
        shoelace: shoelace_area(&uv),
    })
}

/// Whether a ring is a single full-revolution circle edge (no bounding
/// vertices) — the natural periodic boundary of a cylinder/cone/sphere/torus
/// wall, peeled off whole when assembling rings for [`order_loop_groups`].
pub(super) fn is_full_revolution(brep: &BRep, ring: &[(EdgeId, bool)]) -> bool {
    ring.len() == 1 && {
        let e = brep.edge(ring[0].0);
        e.start.is_none() && e.end.is_none()
    }
}

/// F0-1: order a branch's edges into one [`FaceLoops`] PER CONNECTED
/// BOUNDARY REGION. A branch whose kept surface splits into several
/// disjoint regions (the transversal plus: the taller box cuts the
/// flatter one's wall into two separate rectangles) must materialise one
/// face per region — "one face per branch" packs disjoint rings into a
/// single invalid face. Containment decides the grouping, NOT mere edge
/// connectivity: an annulus's hole rim is disconnected from its outer rim
/// yet belongs to the same face.
///
/// Planar surfaces nest rings by even-odd containment depth in the
/// tangent plane (even depth = a region's outer ring, odd depth = hole of
/// its immediate container — smallest containing ring). Curved surfaces
/// keep a single-face classification ([`classify_rings_curved`]) —
/// multi-region on curved carriers is a documented limitation; the
/// periodic uv domain has no global containment order.
pub(super) fn order_loop_groups(
    brep: &BRep,
    edge_ids: &[EdgeId],
    surface: &SurfaceGeometry,
    complemented: bool,
) -> Option<Vec<FaceLoops>> {
    let rings = assemble_rings(brep, edge_ids, surface)?;
    let s = if complemented { -1.0 } else { 1.0 };

    let is_planar = matches!(
        surface,
        SurfaceGeometry::Analytic { surface: HalfSpace::Plane { .. }, .. }
    );
    if is_planar {
        if let Some(groups) = group_rings_planar(brep, &rings, surface, s) {
            return Some(groups);
        }
        // No usable normal: fall through to the curved-path extent rules.
    }
    Some(vec![classify_rings_curved(brep, rings, surface, s)])
}

/// Even-odd (ray-casting) point-in-polygon test in 2D.
fn point_in_polygon(p: (f64, f64), poly: &[(f64, f64)]) -> bool {
    let mut inside = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if (yi > p.1) != (yj > p.1) {
            let x_int = xi + (p.1 - yi) * (xj - xi) / (yj - yi);
            if p.0 < x_int {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

/// Planar multi-region grouping (F0-1): nest the assembled rings by
/// even-odd containment depth in the face's tangent plane and emit one
/// [`FaceLoops`] per even-depth ring, with its odd-depth children as
/// holes. Winding per group follows the planar F0-2 convention: outer CCW
/// w.r.t. the body-outward normal (`area · s > 0`), holes the opposite
/// way. `None` if no usable normal can be evaluated (caller falls through
/// to the curved rules).
fn group_rings_planar(
    brep: &BRep,
    rings: &[Ring],
    surface: &SurfaceGeometry,
    s: f64,
) -> Option<Vec<FaceLoops>> {
    let rep_pos = rings
        .iter()
        .flat_map(|ring| ring.iter())
        .find_map(|&(eid, _)| brep.edge(eid).start.map(|v| brep.vertex(v).position))
        .or_else(|| ring_sample_points(brep, &rings[0]).first().copied())?;
    let n = surface_normal_at(surface, rep_pos)?;
    let (bu, bv) = plane_basis(n);
    let project = |p: [f64; 3]| -> (f64, f64) {
        (
            p[0] * bu[0] + p[1] * bu[1] + p[2] * bu[2],
            p[0] * bv[0] + p[1] * bv[1] + p[2] * bv[2],
        )
    };
    let polys: Vec<Vec<(f64, f64)>> = rings
        .iter()
        .map(|r| ring_sample_points(brep, r).into_iter().map(project).collect())
        .collect();
    if polys.iter().any(|p| p.len() < 3) {
        return None;
    }
    let mut areas: Vec<f64> = polys.iter().map(|p| shoelace_area(p)).collect();

    // Containment by majority vote over ring i's OWN boundary samples
    // (NOT its centroid — concentric rings like a bore hole in a box face
    // share their centroid, which would make each "contain" the other).
    // Samples exactly on the other ring's outline (shared seam vertices)
    // vote arbitrarily but are outnumbered by the clear interior/exterior
    // majority.
    let ring_inside = |i: usize, j: usize| -> bool {
        let inside = polys[i]
            .iter()
            .filter(|&&p| point_in_polygon(p, &polys[j]))
            .count();
        inside * 2 > polys[i].len()
    };
    let mut parent: Vec<Option<usize>> = vec![None; rings.len()];
    let mut depth: Vec<usize> = vec![0; rings.len()];
    for i in 0..rings.len() {
        let mut containers: Vec<usize> = (0..rings.len())
            .filter(|&j| j != i && ring_inside(i, j))
            .collect();
        depth[i] = containers.len();
        // Immediate parent = smallest containing ring by |area|.
        containers.sort_by(|&a, &b| areas[a].abs().total_cmp(&areas[b].abs()));
        parent[i] = containers.first().copied();
    }

    // One face per even-depth ring; odd-depth rings are holes of their
    // immediate (even-depth) container.
    let enforce = |ring: &mut Ring, area: &mut f64, want_positive: bool| {
        if (*area * s > 0.0) != want_positive {
            ring.reverse();
            flip_orientations(ring);
            *area = -*area;
        }
    };
    let mut groups: Vec<(usize, FaceLoops)> = Vec::new();
    for i in 0..rings.len() {
        if depth[i] % 2 != 0 {
            continue;
        }
        let mut outer = rings[i].clone();
        enforce(&mut outer, &mut areas[i], true);
        groups.push((i, FaceLoops { outer, inner: Vec::new() }));
    }
    for i in 0..rings.len() {
        if depth[i] % 2 == 0 {
            continue;
        }
        let Some(p) = parent[i] else { continue };
        let mut hole = rings[i].clone();
        enforce(&mut hole, &mut areas[i], false);
        if let Some((_, group)) = groups.iter_mut().find(|(outer_i, _)| *outer_i == p) {
            group.inner.push(hole);
        } else {
            // Parent isn't an even-depth ring (numerically degenerate
            // nesting) — refuse rather than emit an orphan hole.
            return None;
        }
    }
    if groups.is_empty() {
        return None;
    }
    Some(groups.into_iter().map(|(_, g)| g).collect())
}

/// Assemble a face's edges into closed rings: self-closing circle edges (no
/// bounding vertices) each form their own ring — a face can be bounded by
/// one or more such circles (a cylinder wall between two trim circles) — and
/// the remaining vertex-bounded edges are ordered by the turning-angle walk.
/// `None` if the edges cannot close into rings (degenerate topology).
fn assemble_rings(
    brep: &BRep,
    edge_ids: &[EdgeId],
    surface: &SurfaceGeometry,
) -> Option<Vec<Ring>> {
    if edge_ids.is_empty() {
        return None;
    }
    let mut rings: Vec<Ring> = Vec::new();
    let mut used = vec![false; edge_ids.len()];
    for (i, &eid) in edge_ids.iter().enumerate() {
        let edge = brep.edge(eid);
        if edge.start.is_none() && edge.end.is_none() {
            used[i] = true;
            rings.push(vec![(eid, false)]);
        }
    }

    let normal_at = |p: [f64; 3]| surface_normal_at(surface, p);
    while let Some(seed) = (0..edge_ids.len()).find(|&i| !used[i]) {
        let ring = order_one_ring(brep, edge_ids, &mut used, seed, &normal_at)?;
        rings.push(ring);
    }
    if rings.is_empty() {
        return None;
    }
    Some(rings)
}

/// Curved-face outer/inner classification and winding (also the planar
/// fallback without a usable normal):
///
/// - If any ring is a full-revolution trim circle, it is the surface's
///   natural periodic boundary and encloses every bounded sub-loop, so the
///   outer bound is the longest such circle (DL-58 extent rule — the
///   cylinder/cone tube + drilled-box convention). All other rings are inner.
/// - Otherwise every ring is a bounded loop: classify by parametric (u, v)
///   signed area, the largest by magnitude being outer. This is invariant to
///   a tapered surface's varying radius, so a physically large hole no longer
///   out-ranks a radially small true boundary (which world-space chord length
///   would get wrong).
/// - With no usable parameterization (tessellated / gyroid) fall back to the
///   extent rule.
///
/// Winding (F0-2; it used to be left "as ordered"): in the uv plane, "face on
/// the left in 3D body-outward" means "face on the RIGHT" for an
/// uncomplemented branch (left-handed parameterization, see
/// [`RingUvMetrics`]) — negated by `s`.
///
/// - Bounded loops (closing in uv): an outer bound of a uv region with the
///   region on the right winds CW → shoelace sign must be `−s`; holes the
///   opposite (`+s`).
/// - Full-revolution rims (net_du = ±2π): the required traversal direction
///   depends on which side of the rim the face lies along v, NOT on the
///   outer/inner role — face below (smaller v) → `du · s > 0`, face above
///   → `du · s < 0`. The face side comes from the mean v of the face's
///   OTHER rings; a lone rim (e.g. a spherical cap cut) has no reference
///   and keeps its walk order (documented v1 limitation).
///
/// Unparameterized surfaces (tessellated / gyroid) keep their walk order.
fn classify_rings_curved(
    brep: &BRep,
    mut rings: Vec<Ring>,
    surface: &SurfaceGeometry,
    s: f64,
) -> FaceLoops {
    let extent_max = |rings: &[Ring]| {
        (0..rings.len())
            .max_by(|&a, &b| ring_extent(brep, &rings[a]).total_cmp(&ring_extent(brep, &rings[b])))
            .unwrap()
    };
    let full_rev: Vec<usize> = (0..rings.len())
        .filter(|&i| is_full_revolution(brep, &rings[i]))
        .collect();
    let outer_idx = if !full_rev.is_empty() {
        *full_rev
            .iter()
            .max_by(|&&a, &&b| ring_extent(brep, &rings[a]).total_cmp(&ring_extent(brep, &rings[b])))
            .unwrap()
    } else if let Some(areas) = rings
        .iter()
        .map(|r| ring_uv_area(brep, r, surface))
        .collect::<Option<Vec<f64>>>()
    {
        (0..rings.len())
            .max_by(|&a, &b| areas[a].abs().total_cmp(&areas[b].abs()))
            .unwrap()
    } else {
        extent_max(&rings)
    };

    let metrics: Vec<Option<RingUvMetrics>> = rings
        .iter()
        .map(|r| ring_uv_metrics(&ring_sample_points(brep, r), surface))
        .collect();
    let ring_count = rings.len();
    let mean_v_other = |skip: usize| -> Option<f64> {
        let vs: Vec<f64> = (0..ring_count)
            .filter(|&j| j != skip)
            .filter_map(|j| metrics[j].map(|m| m.mean_v))
            .collect();
        if vs.is_empty() {
            None
        } else {
            Some(vs.iter().sum::<f64>() / vs.len() as f64)
        }
    };
    for i in 0..rings.len() {
        let Some(RingUvMetrics { net_du, mean_v, shoelace }) = metrics[i] else {
            continue;
        };
        let bounded = net_du.abs() < std::f64::consts::PI;
        let flip = if bounded {
            let required = if i == outer_idx { -s } else { s };
            shoelace * required < 0.0
        } else {
            let Some(v_other) = mean_v_other(i) else {
                continue;
            };
            let required_du = if v_other < mean_v { s } else { -s };
            net_du * required_du < 0.0
        };
        if flip {
            rings[i].reverse();
            flip_orientations(&mut rings[i]);
        }
    }

    let outer = rings.remove(outer_idx);
    FaceLoops { outer, inner: rings }
}

/// Reverse the per-entry orientation flags of a ring after reversing its order,
/// so head-to-tail connectivity is preserved.
pub(super) fn flip_orientations(ring: &mut [(EdgeId, bool)]) {
    for entry in ring.iter_mut() {
        entry.1 = !entry.1;
    }
}

// ─── public entry point ─────────────────────────────────────────────────────
