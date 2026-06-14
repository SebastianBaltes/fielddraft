//! Sweep Gatekeeper — the pre-flight validity check that decides whether a
//! profile-along-path sweep may be meshed PARAMETRICALLY (fast, direct to
//! buffers) or must be delegated to the robust SDF fallback (which merges
//! self-overlaps correctly via `min()`).
//!
//! Three checks, cheapest first:
//!
//! 1. **C¹ / kink** — a hard corner in the spine has no well-defined swept
//!    cross-section (the miter is ambiguous). Detected by the turn angle
//!    between consecutive tangents.
//! 2. **Local curvature** — on the concave side the profile self-overlaps
//!    once the spine's radius of curvature drops below the profile radius
//!    (`κ · r ≥ 1`). Menger curvature of each interior vertex triple.
//! 3. **Global self-intersection** — spatially close but path-DISTANT spine
//!    sections (a U-turn, a spiral with too small a pitch). A capsule BVH
//!    finds the candidate pairs; narrow-phase segment distance confirms.
//!
//! Conservative by design: a false "fail" only costs a slower SDF mesh, a
//! false "pass" ships a broken mesh — so every margin rounds toward failing.
//!
//! The global check excludes path-NEIGHBOURS by ARCLENGTH (~2.5·r), not by
//! index: a finely sampled straight tube has many segments within `2r` in
//! space that are not self-intersections. Because the curvature check runs
//! first (so R > 1.1·r everywhere a sweep passes it), a genuine U-turn keeps
//! its two arms ≥ 1.1π·r ≈ 3.45·r apart in arclength — comfortably outside
//! the 2.5·r exclusion band.

/// Tuning knobs; every default rounds toward a conservative (fail) verdict.
#[derive(Debug, Clone, Copy)]
pub struct SweepGateConfig {
    /// Minimum `cos` of the angle between consecutive tangents (default
    /// cos 30°). Below this the vertex is treated as a hard kink.
    pub kink_cos_min: f32,
    /// Require `R > r · (1 + curvature_margin)` locally (default 0.1).
    pub curvature_margin: f32,
    /// Fail when two arms come within `2r · (1 + collision_margin)`
    /// centreline distance (default 0.05).
    pub collision_margin: f32,
    /// Path-neighbour exclusion band as a multiple of `r` (default 2.5).
    pub exclude_arc_factor: f32,
    /// Whether the spine is a closed loop (wraps last→first).
    pub closed: bool,
}

impl Default for SweepGateConfig {
    fn default() -> Self {
        Self {
            kink_cos_min: 0.866_025, // cos 30°
            curvature_margin: 0.1,
            collision_margin: 0.05,
            exclude_arc_factor: 2.5,
            closed: false,
        }
    }
}

/// Outcome of the pre-flight check. Indices point at the offending spine
/// vertex (kink/curvature) or the two colliding segment starts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SweepGateVerdict {
    /// Safe to mesh parametrically.
    Pass,
    /// Hard corner at spine vertex `i`.
    FailKink(usize),
    /// Curvature too tight at spine vertex `i`.
    FailCurvature(usize),
    /// Segments `i` and `j` (distant along the path) overlap in space.
    FailCollision(usize, usize),
}

impl SweepGateVerdict {
    pub fn is_pass(self) -> bool {
        matches!(self, SweepGateVerdict::Pass)
    }
}

// ── small vec3 helpers ──────────────────────────────────────────────
type V3 = [f32; 3];
fn sub(a: V3, b: V3) -> V3 { [a[0] - b[0], a[1] - b[1], a[2] - b[2]] }
fn add(a: V3, b: V3) -> V3 { [a[0] + b[0], a[1] + b[1], a[2] + b[2]] }
fn scale(a: V3, s: f32) -> V3 { [a[0] * s, a[1] * s, a[2] * s] }
fn dot(a: V3, b: V3) -> f32 { a[0] * b[0] + a[1] * b[1] + a[2] * b[2] }
fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn norm(a: V3) -> f32 { dot(a, a).sqrt() }

/// Run all three gate checks on `path` swept with a profile of bounding
/// radius `profile_radius`. Returns the first failure, else `Pass`.
pub fn check_sweep(
    path: &[V3],
    profile_radius: f32,
    cfg: &SweepGateConfig,
) -> SweepGateVerdict {
    let n = path.len();
    if n < 2 || !(profile_radius > 0.0) {
        return SweepGateVerdict::Pass; // nothing meaningful to sweep
    }

    // Interior vertices: 1..n-1 for open; all (wrapped) for closed.
    let interior: Vec<usize> = if cfg.closed {
        (0..n).collect()
    } else {
        (1..n.saturating_sub(1)).collect()
    };
    let prev = |i: usize| if i == 0 { n - 1 } else { i - 1 };
    let next = |i: usize| if i + 1 == n { 0 } else { i + 1 };

    // 1. C¹ / kink.
    for &i in &interior {
        let t0 = sub(path[i], path[prev(i)]);
        let t1 = sub(path[next(i)], path[i]);
        let (l0, l1) = (norm(t0), norm(t1));
        if l0 < 1e-6 || l1 < 1e-6 {
            continue; // degenerate (duplicate point) — not a kink
        }
        if dot(t0, t1) / (l0 * l1) < cfg.kink_cos_min {
            return SweepGateVerdict::FailKink(i);
        }
    }

    // 2. Local curvature (Menger): R = a·b·c / (4·area).
    let r_min = profile_radius * (1.0 + cfg.curvature_margin);
    for &i in &interior {
        let a = path[prev(i)];
        let b = path[i];
        let c = path[next(i)];
        let ab = norm(sub(b, a));
        let bc = norm(sub(c, b));
        let ca = norm(sub(a, c));
        let area = 0.5 * norm(cross(sub(b, a), sub(c, a)));
        if area < 1e-9 {
            continue; // collinear → infinite radius → fine
        }
        let radius = (ab * bc * ca) / (4.0 * area);
        if radius < r_min {
            return SweepGateVerdict::FailCurvature(i);
        }
    }

    // 3. Global self-intersection via capsule BVH.
    self_intersection(path, profile_radius, cfg)
}

// ── segments + arclength ────────────────────────────────────────────

struct Segment {
    a: V3,
    b: V3,
    aabb: Aabb,
    /// Arclength of this segment's midpoint along the spine.
    s_mid: f32,
    /// Half this segment's length (to convert midpoint gaps to end gaps).
    half_len: f32,
}

#[derive(Clone, Copy)]
struct Aabb {
    min: V3,
    max: V3,
}
impl Aabb {
    fn of_capsule(a: V3, b: V3, r: f32) -> Self {
        Aabb {
            min: [a[0].min(b[0]) - r, a[1].min(b[1]) - r, a[2].min(b[2]) - r],
            max: [a[0].max(b[0]) + r, a[1].max(b[1]) + r, a[2].max(b[2]) + r],
        }
    }
    fn overlaps(&self, o: &Aabb) -> bool {
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
}

fn build_segments(path: &[V3], r: f32, closed: bool) -> Vec<Segment> {
    let n = path.len();
    let count = if closed { n } else { n - 1 };
    let mut segs = Vec::with_capacity(count);
    let mut s = 0.0_f32;
    for i in 0..count {
        let a = path[i];
        let b = path[(i + 1) % n];
        let len = norm(sub(b, a));
        segs.push(Segment {
            a,
            b,
            aabb: Aabb::of_capsule(a, b, r),
            s_mid: s + len * 0.5,
            half_len: len * 0.5,
        });
        s += len;
    }
    segs
}

// ── capsule BVH (median split on segment-AABB centroids) ────────────

enum Bvh {
    Leaf(Vec<usize>),
    Node { aabb: Aabb, left: Box<Bvh>, right: Box<Bvh> },
}

const BVH_LEAF_MAX: usize = 4;

fn build_bvh(segs: &[Segment], mut idx: Vec<usize>) -> Bvh {
    if idx.len() <= BVH_LEAF_MAX {
        return Bvh::Leaf(idx);
    }
    // Bounds + longest axis of the centroid spread.
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

/// Collect segment indices whose AABB overlaps `query` into `out`.
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

fn self_intersection(path: &[V3], r: f32, cfg: &SweepGateConfig) -> SweepGateVerdict {
    let segs = build_segments(path, r, cfg.closed);
    if segs.len() < 2 {
        return SweepGateVerdict::Pass;
    }
    let total_len: f32 = {
        let last = &segs[segs.len() - 1];
        // s_mid of last + its half-length ≈ total length.
        last.s_mid + norm(sub(last.b, last.a)) * 0.5
    };
    let exclude_arc = cfg.exclude_arc_factor * r;
    let hit_threshold = 2.0 * r * (1.0 + cfg.collision_margin);
    let hit2 = hit_threshold * hit_threshold;

    let bvh = build_bvh(&segs, (0..segs.len()).collect());
    let mut candidates: Vec<usize> = Vec::new();

    for i in 0..segs.len() {
        candidates.clear();
        bvh_query(&bvh, &segs, &segs[i].aabb, &mut candidates);
        for &j in &candidates {
            if j <= i {
                continue; // unordered pair, test once
            }
            // Path-neighbour exclusion by the arclength gap between the
            // segments' NEAREST ENDS (midpoint gap minus both half-lengths),
            // so a coarsely-sampled smooth tube isn't mistaken for a
            // self-intersection. Wrap the arclength for closed loops.
            let mut ds = (segs[i].s_mid - segs[j].s_mid).abs();
            if cfg.closed {
                ds = ds.min(total_len - ds);
            }
            let end_gap = ds - (segs[i].half_len + segs[j].half_len);
            if end_gap < exclude_arc {
                continue;
            }
            if seg_seg_dist2(segs[i].a, segs[i].b, segs[j].a, segs[j].b) < hit2 {
                return SweepGateVerdict::FailCollision(i, j);
            }
        }
    }
    SweepGateVerdict::Pass
}

/// Squared minimum distance between segments [p1,q1] and [p2,q2].
/// (Ericson, *Real-Time Collision Detection*, §5.1.9.)
fn seg_seg_dist2(p1: V3, q1: V3, p2: V3, q2: V3) -> f32 {
    let d1 = sub(q1, p1);
    let d2 = sub(q2, p2);
    let r = sub(p1, p2);
    let a = dot(d1, d1);
    let e = dot(d2, d2);
    let f = dot(d2, r);
    const EPS: f32 = 1e-12;

    let (mut s, mut t);
    if a <= EPS && e <= EPS {
        return dot(r, r); // both degenerate to points
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

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> SweepGateConfig {
        SweepGateConfig::default()
    }

    #[test]
    fn straight_path_passes() {
        let path: Vec<V3> = (0..10).map(|i| [i as f32 * 5.0, 0.0, 0.0]).collect();
        assert_eq!(check_sweep(&path, 2.0, &cfg()), SweepGateVerdict::Pass);
    }

    #[test]
    fn dense_straight_tube_does_not_false_collide() {
        // 0.5mm steps, fat 5mm profile: many segments within 2r in space,
        // but all path-neighbours → must still pass.
        let path: Vec<V3> = (0..200).map(|i| [i as f32 * 0.5, 0.0, 0.0]).collect();
        assert_eq!(check_sweep(&path, 5.0, &cfg()), SweepGateVerdict::Pass);
    }

    #[test]
    fn hard_corner_fails_kink() {
        // An L: straight in +x, then a 90° turn into +y.
        let mut path: Vec<V3> = (0..6).map(|i| [i as f32 * 5.0, 0.0, 0.0]).collect();
        for i in 1..6 {
            path.push([25.0, i as f32 * 5.0, 0.0]);
        }
        assert!(matches!(check_sweep(&path, 2.0, &cfg()), SweepGateVerdict::FailKink(_)));
    }

    #[test]
    fn tight_arc_fails_curvature() {
        // Quarter circle radius 3mm, profile radius 5mm → R < r.
        let r = 3.0_f32;
        let path: Vec<V3> = (0..=16)
            .map(|i| {
                let a = i as f32 / 16.0 * std::f32::consts::FRAC_PI_2;
                [r * a.cos(), r * a.sin(), 0.0]
            })
            .collect();
        assert!(matches!(
            check_sweep(&path, 5.0, &cfg()),
            SweepGateVerdict::FailCurvature(_)
        ));
    }

    #[test]
    fn gentle_arc_passes() {
        // Radius 50mm arc, profile radius 3mm → R ≫ r, smoothly sampled.
        let r = 50.0_f32;
        let path: Vec<V3> = (0..=40)
            .map(|i| {
                let a = i as f32 / 40.0 * std::f32::consts::FRAC_PI_2;
                [r * a.cos(), r * a.sin(), 0.0]
            })
            .collect();
        assert_eq!(check_sweep(&path, 3.0, &cfg()), SweepGateVerdict::Pass);
    }

    /// Limaçon r(θ)=a+b·cosθ with b>a: a smooth analytic curve with an
    /// inner loop that crosses itself at the origin. Lets us exercise the
    /// GLOBAL check on a self-intersection that is path-distant yet smooth
    /// (so the kink/curvature checks pass first).
    fn limacon(a: f32, b: f32, n: usize) -> Vec<V3> {
        (0..n)
            .map(|i| {
                let th = i as f32 / n as f32 * std::f32::consts::TAU;
                let r = a + b * th.cos();
                [r * th.cos(), r * th.sin(), 0.0]
            })
            .collect()
    }

    #[test]
    fn self_crossing_loop_fails_collision() {
        // a=20,b=50 → ~70mm span, smooth (min curvature radius ≫ 0.5mm),
        // crosses itself at the node → thin profile must fail GLOBALLY.
        let path = limacon(20.0, 50.0, 120);
        let v = check_sweep(&path, 0.5, &SweepGateConfig { closed: true, ..cfg() });
        assert!(matches!(v, SweepGateVerdict::FailCollision(_, _)), "got {v:?}");
    }

    #[test]
    fn convex_closed_loop_passes() {
        // A plain circle (radius 50) never approaches itself; opposite arms
        // are a diameter apart → no collision, gentle curvature.
        let path: Vec<V3> = (0..72)
            .map(|i| {
                let a = i as f32 / 72.0 * std::f32::consts::TAU;
                [50.0 * a.cos(), 50.0 * a.sin(), 0.0]
            })
            .collect();
        assert_eq!(
            check_sweep(&path, 3.0, &SweepGateConfig { closed: true, ..cfg() }),
            SweepGateVerdict::Pass
        );
    }
}
