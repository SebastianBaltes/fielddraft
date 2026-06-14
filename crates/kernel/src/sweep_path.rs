//! Shared spine-path infrastructure for general sweeps.
//!
//! A `SweepPath` is the source-AGNOSTIC carrier between every part of the
//! sweep pipeline: it samples a spine polyline once and computes, once,
//!
//!   * canonical **rotation-minimizing frames** (double reflection, Wang,
//!     Jüttler, Zheng & Liu, *ACM TOG* 2008) — so the web mesher and the
//!     kernel SDF place the profile identically (no seam mismatch where a
//!     parametric body meets an SDF one);
//!   * cumulative **arclength** per sample;
//!   * a segment **AABB BVH**, reused by the Gatekeeper's self-intersection
//!     check ([`crate::sweep_gate`]) and by the SDF's nearest-segment query.
//!
//! RMF is path-GLOBAL (integrated from the start), so the frame at an
//! arbitrary query's closest point cannot be recomputed locally — it must be
//! looked up here. That is the whole reason frames live on the path object.
//!
//! Closed-loop note: the open RMF generally does not close up (an angular
//! defect at the seam). Distributing that defect (Wang 2008 §4) is a future
//! refinement; today a closed sweep may carry a small seam twist.

pub type V3 = [f32; 3];

pub(crate) fn sub(a: V3, b: V3) -> V3 { [a[0] - b[0], a[1] - b[1], a[2] - b[2]] }
pub(crate) fn add(a: V3, b: V3) -> V3 { [a[0] + b[0], a[1] + b[1], a[2] + b[2]] }
pub(crate) fn scale(a: V3, s: f32) -> V3 { [a[0] * s, a[1] * s, a[2] * s] }
pub(crate) fn dot(a: V3, b: V3) -> f32 { a[0] * b[0] + a[1] * b[1] + a[2] * b[2] }
pub(crate) fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
pub(crate) fn norm(a: V3) -> f32 { dot(a, a).sqrt() }
pub(crate) fn normalize(a: V3) -> V3 {
    let l = norm(a);
    if l > 1e-12 { [a[0] / l, a[1] / l, a[2] / l] } else { [0.0, 0.0, 0.0] }
}

/// An orthonormal frame `{t, n, b}` carried along the spine.
#[derive(Debug, Clone, Copy)]
pub struct Frame {
    pub t: V3,
    pub n: V3,
    pub b: V3,
}

// ── AABB ────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug)]
pub(crate) struct Aabb {
    pub min: V3,
    pub max: V3,
}
impl Aabb {
    fn of_segment(a: V3, b: V3) -> Self {
        Aabb {
            min: [a[0].min(b[0]), a[1].min(b[1]), a[2].min(b[2])],
            max: [a[0].max(b[0]), a[1].max(b[1]), a[2].max(b[2])],
        }
    }
    pub(crate) fn expanded(&self, r: f32) -> Aabb {
        Aabb {
            min: [self.min[0] - r, self.min[1] - r, self.min[2] - r],
            max: [self.max[0] + r, self.max[1] + r, self.max[2] + r],
        }
    }
    pub(crate) fn overlaps(&self, o: &Aabb) -> bool {
        self.min[0] <= o.max[0] && self.max[0] >= o.min[0]
            && self.min[1] <= o.max[1] && self.max[1] >= o.min[1]
            && self.min[2] <= o.max[2] && self.max[2] >= o.min[2]
    }
    fn union(&self, o: &Aabb) -> Aabb {
        Aabb {
            min: [self.min[0].min(o.min[0]), self.min[1].min(o.min[1]), self.min[2].min(o.min[2])],
            max: [self.max[0].max(o.max[0]), self.max[1].max(o.max[1]), self.max[2].max(o.max[2])],
        }
    }
    fn centroid_axis(&self, axis: usize) -> f32 {
        (self.min[axis] + self.max[axis]) * 0.5
    }
    /// Squared distance from a point to this box (0 inside).
    fn dist2_to_point(&self, p: V3) -> f32 {
        let mut d = 0.0;
        for k in 0..3 {
            let v = p[k];
            if v < self.min[k] {
                let e = self.min[k] - v;
                d += e * e;
            } else if v > self.max[k] {
                let e = v - self.max[k];
                d += e * e;
            }
        }
        d
    }
}

// ── Segments ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub(crate) struct Segment {
    pub a: V3,
    pub b: V3,
    pub aabb: Aabb,
    /// Arclength of this segment's midpoint along the spine.
    pub s_mid: f32,
    /// Half this segment's length.
    pub half_len: f32,
}

// ── Capsule/segment BVH (median split on segment-AABB centroids) ────

#[derive(Debug)]
enum Bvh {
    Leaf(Vec<usize>),
    Node { aabb: Aabb, left: Box<Bvh>, right: Box<Bvh> },
}

const BVH_LEAF_MAX: usize = 4;

fn build_bvh(segs: &[Segment], mut idx: Vec<usize>) -> Bvh {
    if idx.len() <= BVH_LEAF_MAX {
        return Bvh::Leaf(idx);
    }
    let mut bb = segs[idx[0]].aabb;
    for &i in &idx[1..] {
        bb = bb.union(&segs[i].aabb);
    }
    let ext = [bb.max[0] - bb.min[0], bb.max[1] - bb.min[1], bb.max[2] - bb.min[2]];
    let axis = if ext[0] >= ext[1] && ext[0] >= ext[2] {
        0
    } else if ext[1] >= ext[2] {
        1
    } else {
        2
    };
    idx.sort_by(|&p, &q| {
        segs[p].aabb.centroid_axis(axis)
            .partial_cmp(&segs[q].aabb.centroid_axis(axis))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mid = idx.len() / 2;
    let right_idx = idx.split_off(mid);
    Bvh::Node {
        aabb: bb,
        left: Box::new(build_bvh(segs, idx)),
        right: Box::new(build_bvh(segs, right_idx)),
    }
}

fn bvh_query(bvh: &Bvh, segs: &[Segment], query: &Aabb, out: &mut Vec<usize>) {
    match bvh {
        Bvh::Leaf(items) => {
            for &i in items {
                if segs[i].aabb.overlaps(query) {
                    out.push(i);
                }
            }
        }
        Bvh::Node { aabb, left, right } => {
            if aabb.overlaps(query) {
                bvh_query(left, segs, query, out);
                bvh_query(right, segs, query, out);
            }
        }
    }
}

/// Squared minimum distance between segments `[p1,q1]` and `[p2,q2]`.
/// (Ericson, *Real-Time Collision Detection*, §5.1.9.)
pub(crate) fn seg_seg_dist2(p1: V3, q1: V3, p2: V3, q2: V3) -> f32 {
    let d1 = sub(q1, p1);
    let d2 = sub(q2, p2);
    let r = sub(p1, p2);
    let a = dot(d1, d1);
    let e = dot(d2, d2);
    let f = dot(d2, r);
    const EPS: f32 = 1e-12;

    let (mut s, mut t);
    if a <= EPS && e <= EPS {
        return dot(r, r);
    }
    if a <= EPS {
        s = 0.0;
        t = (f / e).clamp(0.0, 1.0);
    } else {
        let c = dot(d1, r);
        if e <= EPS {
            t = 0.0;
            s = (-c / a).clamp(0.0, 1.0);
        } else {
            let b = dot(d1, d2);
            let denom = a * e - b * b;
            s = if denom > EPS {
                ((b * f - c * e) / denom).clamp(0.0, 1.0)
            } else {
                0.0
            };
            t = (b * s + f) / e;
            if t < 0.0 {
                t = 0.0;
                s = (-c / a).clamp(0.0, 1.0);
            } else if t > 1.0 {
                t = 1.0;
                s = ((b - c) / a).clamp(0.0, 1.0);
            }
        }
    }
    let c1 = add(p1, scale(d1, s));
    let c2 = add(p2, scale(d2, t));
    let diff = sub(c1, c2);
    dot(diff, diff)
}

/// Closest point parameter `t∈[0,1]` and squared distance of `p` to segment.
fn pt_seg(p: V3, a: V3, b: V3) -> (f32, f32) {
    let ab = sub(b, a);
    let denom = dot(ab, ab);
    let t = if denom > 1e-12 { (dot(sub(p, a), ab) / denom).clamp(0.0, 1.0) } else { 0.0 };
    let c = add(a, scale(ab, t));
    (t, norm(sub(p, c)).powi(2))
}

// ── Frames (RMF via double reflection) ──────────────────────────────

fn compute_tangents(points: &[V3], closed: bool) -> Vec<V3> {
    let n = points.len();
    (0..n)
        .map(|i| {
            let t = if closed {
                sub(points[(i + 1) % n], points[(i + n - 1) % n])
            } else if i == 0 {
                sub(points[1], points[0])
            } else if i == n - 1 {
                sub(points[n - 1], points[n - 2])
            } else {
                sub(points[i + 1], points[i - 1])
            };
            normalize(t)
        })
        .collect()
}

/// A unit normal orthogonal to `t` (world axis least aligned with `t`).
fn seed_normal(t: V3) -> V3 {
    let r = if t[1].abs() < 0.9 { [0.0, 1.0, 0.0] } else { [1.0, 0.0, 0.0] };
    let d = dot(r, t);
    normalize([r[0] - d * t[0], r[1] - d * t[1], r[2] - d * t[2]])
}

fn rm_frames(points: &[V3], tangents: &[V3]) -> Vec<Frame> {
    let n = points.len();
    let mut frames = Vec::with_capacity(n);
    let n0 = seed_normal(tangents[0]);
    frames.push(Frame { t: tangents[0], n: n0, b: cross(tangents[0], n0) });

    for i in 0..n - 1 {
        let v1 = sub(points[i + 1], points[i]);
        let c1 = dot(v1, v1);
        let prev = frames[i];
        if c1 < 1e-12 {
            frames.push(Frame { t: tangents[i + 1], n: prev.n, b: prev.b });
            continue;
        }
        let f1n = (2.0 / c1) * dot(v1, prev.n);
        let r_l_n = sub(prev.n, scale(v1, f1n));
        let f1t = (2.0 / c1) * dot(v1, prev.t);
        let t_l = sub(prev.t, scale(v1, f1t));
        let v2 = sub(tangents[i + 1], t_l);
        let c2 = dot(v2, v2);
        let n_next = if c2 < 1e-12 {
            r_l_n
        } else {
            let f2 = (2.0 / c2) * dot(v2, r_l_n);
            sub(r_l_n, scale(v2, f2))
        };
        let n_next = normalize(n_next);
        frames.push(Frame { t: tangents[i + 1], n: n_next, b: cross(tangents[i + 1], n_next) });
    }
    frames
}

// ── SweepPath ───────────────────────────────────────────────────────

/// Result of [`SweepPath::nearest`]: where on the spine a query point lands.
#[derive(Debug, Clone, Copy)]
pub struct NearestHit {
    /// Arclength of the closest spine point.
    pub arclength: f32,
    /// Squared distance from the query to the spine.
    pub dist2: f32,
    /// The closest spine point.
    pub point: V3,
    /// Frame interpolated to the closest point (renormalised).
    pub frame: Frame,
    /// The query projected past an open path's first/last vertex (the foot
    /// clamped to a true end) — used by the SDF for end-cap composition.
    pub at_path_end: bool,
}

/// A query projected into the nearest sweep cross-section.
#[derive(Debug, Clone, Copy)]
pub struct CrossSection {
    /// Profile-plane coordinate along the RMF normal.
    pub u: f32,
    /// Profile-plane coordinate along the RMF binormal.
    pub v: f32,
    /// Axial cap coordinate for [`sweep_compose`] (`-∞` for closed loops).
    pub w: f32,
}

/// Compose a 2D cross-section distance `d2` with the open-end cap coordinate
/// `w` (IQ's extrude composition). `w = -∞` (closed loop) reduces to `d2`.
pub fn sweep_compose(d2: f32, w: f32) -> f32 {
    let outside = d2.max(0.0).hypot(w.max(0.0));
    let inside = d2.max(w).min(0.0);
    outside + inside
}

#[derive(Debug)]
pub struct SweepPath {
    pub points: Vec<V3>,
    pub frames: Vec<Frame>,
    /// Cumulative arclength at each point (`arclen[0] == 0`).
    pub arclen: Vec<f32>,
    pub closed: bool,
    pub total_len: f32,
    pub(crate) segs: Vec<Segment>,
    bvh: Bvh,
}

impl SweepPath {
    /// Sample a spine and precompute frames, arclength and the segment BVH.
    /// Returns `None` for fewer than two points.
    pub fn new(points: Vec<V3>, closed: bool) -> Option<SweepPath> {
        let n = points.len();
        if n < 2 {
            return None;
        }
        let tangents = compute_tangents(&points, closed);
        let frames = rm_frames(&points, &tangents);

        let mut arclen = Vec::with_capacity(n);
        arclen.push(0.0);
        for i in 1..n {
            arclen.push(arclen[i - 1] + norm(sub(points[i], points[i - 1])));
        }
        let wrap = if closed { norm(sub(points[0], points[n - 1])) } else { 0.0 };
        let total_len = arclen[n - 1] + wrap;

        let seg_count = if closed { n } else { n - 1 };
        let mut segs = Vec::with_capacity(seg_count);
        let mut s = 0.0_f32;
        for i in 0..seg_count {
            let a = points[i];
            let b = points[(i + 1) % n];
            let len = norm(sub(b, a));
            segs.push(Segment { a, b, aabb: Aabb::of_segment(a, b), s_mid: s + len * 0.5, half_len: len * 0.5 });
            s += len;
        }
        let bvh = build_bvh(&segs, (0..segs.len()).collect());

        Some(SweepPath { points, frames, arclen, closed, total_len, segs, bvh })
    }

    pub(crate) fn segments(&self) -> &[Segment] {
        &self.segs
    }

    /// Segment indices whose AABB overlaps `query`, appended to `out`.
    pub(crate) fn query_overlaps(&self, query: &Aabb, out: &mut Vec<usize>) {
        bvh_query(&self.bvh, &self.segs, query, out);
    }

    /// Closest point on the spine to `q` (BVH-accelerated, best-dist pruned),
    /// with the frame interpolated to that point.
    pub fn nearest(&self, q: V3) -> NearestHit {
        let mut best2 = f32::INFINITY;
        let mut best_seg = 0usize;
        let mut best_t = 0.0f32;
        self.nearest_rec(&self.bvh, q, &mut best2, &mut best_seg, &mut best_t);

        let seg = &self.segs[best_seg];
        let point = add(seg.a, scale(sub(seg.b, seg.a), best_t));
        let n = self.points.len();
        let i0 = best_seg;
        let i1 = (best_seg + 1) % n;
        let frame = lerp_frame(self.frames[i0], self.frames[i1], best_t);
        let arclength = seg.s_mid - seg.half_len + best_t * (seg.half_len * 2.0);
        let at_path_end = !self.closed
            && ((best_seg == 0 && best_t <= 1e-6)
                || (best_seg == self.segs.len() - 1 && best_t >= 1.0 - 1e-6));
        NearestHit { arclength, dist2: best2, point, frame, at_path_end }
    }

    /// Signed distance to the solid swept by `profile` (in the local
    /// `(n, b)` cross-section frame) along this spine. Assumes the spine
    /// passed the Gatekeeper (no self-intersection, curvature radius above
    /// the profile radius) so the nearest-spine-point foot is unique.
    ///
    /// Lateral surface: the 2D profile distance in the RMF cross-section.
    /// Open ends are capped via IQ's extrude composition of the profile
    /// distance with the axial overshoot past the nearer end. Closed loops
    /// have no caps.
    pub fn sdf(&self, profile: &crate::sketch2d::BoundedSketch2d, p: V3) -> f32 {
        let cs = self.cross_section(p);
        sweep_compose(profile.sdf([cs.u, cs.v]), cs.w)
    }

    /// Project a query into the nearest cross-section: the profile-plane
    /// coordinates `(u, v)` (along the RMF normal/binormal) plus the axial
    /// cap coordinate `w` for [`sweep_compose`]. Callers that need the
    /// profile's edge bands (BRep contributors) eval `(u, v)` themselves and
    /// pass the resulting 2D distance through `sweep_compose(_, w)`.
    pub fn cross_section(&self, p: V3) -> CrossSection {
        let hit = self.nearest(p);
        let d = sub(p, hit.point);
        let w = if self.closed {
            f32::NEG_INFINITY // no caps → composition reduces to the profile
        } else if hit.at_path_end {
            // Past an end: the overshoot along the tangent is positive.
            dot(d, hit.frame.t).abs()
        } else {
            // Inside the slab: −distance to the nearer cap (deep interiors
            // are large-negative → caps ignored).
            -hit.arclength.min(self.total_len - hit.arclength)
        };
        CrossSection { u: dot(d, hit.frame.n), v: dot(d, hit.frame.b), w }
    }

    fn nearest_rec(&self, bvh: &Bvh, q: V3, best2: &mut f32, best_seg: &mut usize, best_t: &mut f32) {
        match bvh {
            Bvh::Leaf(items) => {
                for &i in items {
                    let (t, d2) = pt_seg(q, self.segs[i].a, self.segs[i].b);
                    if d2 < *best2 {
                        *best2 = d2;
                        *best_seg = i;
                        *best_t = t;
                    }
                }
            }
            Bvh::Node { aabb, left, right } => {
                if aabb.dist2_to_point(q) >= *best2 {
                    return; // whole subtree is farther than the current best
                }
                // Descend the nearer child first for tighter pruning.
                let dl = node_dist2(left, q);
                let dr = node_dist2(right, q);
                if dl <= dr {
                    self.nearest_rec(left, q, best2, best_seg, best_t);
                    self.nearest_rec(right, q, best2, best_seg, best_t);
                } else {
                    self.nearest_rec(right, q, best2, best_seg, best_t);
                    self.nearest_rec(left, q, best2, best_seg, best_t);
                }
            }
        }
    }
}

fn node_dist2(bvh: &Bvh, q: V3) -> f32 {
    match bvh {
        Bvh::Leaf(_) => 0.0, // leaves are cheap; don't bother bounding
        Bvh::Node { aabb, .. } => aabb.dist2_to_point(q),
    }
}

/// Linear blend of two frames at parameter `t`. Interpolates the normal and
/// binormal directly (`t = n × b`); kept deliberately simple so the GPU VM
/// (`OP_WARP_SWEEP`) can mirror it byte-for-byte for CPU/GPU SDF parity.
fn lerp_frame(f0: Frame, f1: Frame, t: f32) -> Frame {
    let lerp = |a: V3, b: V3| normalize([
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]);
    let n = lerp(f0.n, f1.n);
    let b = lerp(f0.b, f1.b);
    Frame { t: normalize(cross(n, b)), n, b }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32, eps: f32) -> bool { (a - b).abs() < eps }

    #[test]
    fn straight_path_has_constant_frame_and_arclength() {
        let pts: Vec<V3> = (0..6).map(|i| [i as f32 * 4.0, 0.0, 0.0]).collect();
        let sp = SweepPath::new(pts, false).unwrap();
        for f in &sp.frames {
            assert!(approx(f.n[0], sp.frames[0].n[0], 1e-5));
            assert!(approx(f.n[1], sp.frames[0].n[1], 1e-5));
            assert!(approx(f.n[2], sp.frames[0].n[2], 1e-5));
        }
        assert!(approx(sp.total_len, 20.0, 1e-4));
        assert!(approx(sp.arclen[5], 20.0, 1e-4));
    }

    #[test]
    fn frames_stay_orthonormal_through_a_turn() {
        let pts: Vec<V3> = vec![
            [0.0, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [20.0, 0.0, 0.0],
            [20.0, 10.0, 0.0],
            [20.0, 20.0, 5.0],
        ];
        let sp = SweepPath::new(pts, false).unwrap();
        for f in &sp.frames {
            assert!(approx(norm(f.n), 1.0, 1e-4));
            assert!(approx(norm(f.b), 1.0, 1e-4));
            assert!(approx(dot(f.n, f.b), 0.0, 1e-4));
            assert!(approx(dot(f.t, f.n), 0.0, 1e-4));
        }
    }

    #[test]
    fn nearest_finds_the_closest_spine_point() {
        // Spine along X from 0..40; query above the middle.
        let pts: Vec<V3> = (0..=8).map(|i| [i as f32 * 5.0, 0.0, 0.0]).collect();
        let sp = SweepPath::new(pts, false).unwrap();
        let hit = sp.nearest([17.0, 6.0, 0.0]);
        assert!(approx(hit.point[0], 17.0, 1e-4));
        assert!(approx(hit.point[1], 0.0, 1e-4));
        assert!(approx(hit.dist2.sqrt(), 6.0, 1e-4));
        assert!(approx(hit.arclength, 17.0, 1e-4));
        // Frame on a +X spine: t=+X, n=+Y, b=+Z.
        assert!(approx(hit.frame.t[0], 1.0, 1e-4));
        assert!(approx(hit.frame.n[1], 1.0, 1e-4));
    }

    fn disk(radius: f32) -> crate::sketch2d::BoundedSketch2d {
        use fielddraft_protocol::{Sketch2d, Sketch2dPrimitive};
        crate::sketch2d::BoundedSketch2d::from_sketch(&Sketch2d::Prim(
            Sketch2dPrimitive::Circle { center: [0.0, 0.0], radius },
        ))
    }

    #[test]
    fn straight_sweep_equals_a_capped_cylinder_sdf() {
        // Disk radius 5 swept along X from 0..40 → cylinder, capped.
        let pts: Vec<V3> = (0..=8).map(|i| [i as f32 * 5.0, 0.0, 0.0]).collect();
        let sp = SweepPath::new(pts, false).unwrap();
        let prof = disk(5.0);
        // Axis centre: nearest lateral surface at radius 5 → −5.
        assert!(approx(sp.sdf(&prof, [20.0, 0.0, 0.0]), -5.0, 1e-3));
        // 3mm outside the lateral wall.
        assert!(approx(sp.sdf(&prof, [20.0, 8.0, 0.0]), 3.0, 1e-3));
        // 2mm past the end cap, on the axis.
        assert!(approx(sp.sdf(&prof, [42.0, 0.0, 0.0]), 2.0, 1e-3));
        // Outside the cap edge corner: √(3² + 2²).
        assert!(approx(sp.sdf(&prof, [42.0, 8.0, 0.0]), (13.0_f32).sqrt(), 1e-3));
        // On the lateral surface → ~0.
        assert!(sp.sdf(&prof, [10.0, 5.0, 0.0]).abs() < 1e-2);
    }

    #[test]
    fn curved_sweep_surface_is_near_zero() {
        // Quarter circle radius 50 (gentle), disk radius 3.
        let pts: Vec<V3> = (0..=40)
            .map(|i| {
                let a = i as f32 / 40.0 * std::f32::consts::FRAC_PI_2;
                [50.0 * a.cos(), 50.0 * a.sin(), 0.0]
            })
            .collect();
        let sp = SweepPath::new(pts, false).unwrap();
        let prof = disk(3.0);
        // A point on the outer tube wall at the arc midpoint (radius 53).
        let mid = std::f32::consts::FRAC_PI_4;
        let outer = [53.0 * mid.cos(), 53.0 * mid.sin(), 0.0];
        assert!(sp.sdf(&prof, outer).abs() < 0.2, "surface sdf {}", sp.sdf(&prof, outer));
        // 4mm outside that wall (radius 57) → ≈ +4.
        let off = [57.0 * mid.cos(), 57.0 * mid.sin(), 0.0];
        assert!(approx(sp.sdf(&prof, off), 4.0, 0.2));
        // Tube centre (on the spine) → ≈ −3.
        let onaxis = [50.0 * mid.cos(), 50.0 * mid.sin(), 0.0];
        assert!(approx(sp.sdf(&prof, onaxis), -3.0, 0.2));
    }

    #[test]
    fn nearest_matches_brute_force_on_a_curve() {
        let pts: Vec<V3> = (0..=40)
            .map(|i| {
                let a = i as f32 / 40.0 * std::f32::consts::PI;
                [30.0 * a.cos(), 30.0 * a.sin(), 0.0]
            })
            .collect();
        let sp = SweepPath::new(pts.clone(), false).unwrap();
        let q = [5.0, 5.0, 8.0];
        let hit = sp.nearest(q);
        // Brute force over all segments.
        let mut brute = f32::INFINITY;
        for w in pts.windows(2) {
            let (_, d2) = pt_seg(q, w[0], w[1]);
            brute = brute.min(d2);
        }
        assert!(approx(hit.dist2, brute, 1e-3), "bvh {} vs brute {}", hit.dist2, brute);
    }
}
