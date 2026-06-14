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
//!    sections (a U-turn, a spiral with too small a pitch). The path's
//!    capsule BVH ([`crate::sweep_path`]) finds candidate pairs; a
//!    narrow-phase segment distance confirms.
//!
//! Conservative by design: a false "fail" only costs a slower SDF mesh, a
//! false "pass" ships a broken mesh — so every margin rounds toward failing.
//!
//! The global check excludes path-NEIGHBOURS by the ARCLENGTH gap between
//! the segments' nearest ENDS (`|Δs_mid| − halfLen_i − halfLen_j`), not by
//! index: a finely sampled straight tube has many segments within `2r` in
//! space that are not self-intersections. Because the curvature check runs
//! first (so R > 1.1·r everywhere a sweep passes it), a genuine U-turn keeps
//! its two arms ≥ 1.1π·r ≈ 3.45·r apart in arclength — comfortably outside
//! the 2.5·r exclusion band.

use crate::sweep_path::{cross, dot, norm, sub, seg_seg_dist2, SweepPath};

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
}

impl Default for SweepGateConfig {
    fn default() -> Self {
        Self {
            kink_cos_min: 0.866_025, // cos 30°
            curvature_margin: 0.1,
            collision_margin: 0.05,
            exclude_arc_factor: 2.5,
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

/// Run all three gate checks on `path` swept with a profile of bounding
/// radius `profile_radius`. Returns the first failure, else `Pass`.
pub fn check_sweep(
    path: &SweepPath,
    profile_radius: f32,
    cfg: &SweepGateConfig,
) -> SweepGateVerdict {
    let pts = &path.points;
    let n = pts.len();
    if n < 2 || !(profile_radius > 0.0) {
        return SweepGateVerdict::Pass; // nothing meaningful to sweep
    }

    // Interior vertices: 1..n-1 for open; all (wrapped) for closed.
    let interior: Vec<usize> = if path.closed {
        (0..n).collect()
    } else {
        (1..n.saturating_sub(1)).collect()
    };
    let prev = |i: usize| if i == 0 { n - 1 } else { i - 1 };
    let next = |i: usize| if i + 1 == n { 0 } else { i + 1 };

    // 1. C¹ / kink.
    for &i in &interior {
        let t0 = sub(pts[i], pts[prev(i)]);
        let t1 = sub(pts[next(i)], pts[i]);
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
        let a = pts[prev(i)];
        let b = pts[i];
        let c = pts[next(i)];
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

    // 3. Global self-intersection via the path's capsule BVH.
    self_intersection(path, profile_radius, cfg)
}

fn self_intersection(path: &SweepPath, r: f32, cfg: &SweepGateConfig) -> SweepGateVerdict {
    let segs = path.segments();
    if segs.len() < 2 {
        return SweepGateVerdict::Pass;
    }
    let exclude_arc = cfg.exclude_arc_factor * r;
    let hit_threshold = 2.0 * r * (1.0 + cfg.collision_margin);
    let hit2 = hit_threshold * hit_threshold;

    let mut candidates: Vec<usize> = Vec::new();
    for i in 0..segs.len() {
        candidates.clear();
        // Broad-phase: dilate this segment's AABB by the hit threshold so the
        // BVH (raw segment AABBs) returns every segment within reach.
        let q = segs[i].aabb.expanded(hit_threshold);
        path.query_overlaps(&q, &mut candidates);
        for &j in &candidates {
            if j <= i {
                continue; // unordered pair, test once
            }
            // Path-neighbour exclusion by the arclength gap between the
            // segments' NEAREST ENDS, so a coarsely-sampled smooth tube is
            // not mistaken for a self-intersection. Wrap for closed loops.
            let mut ds = (segs[i].s_mid - segs[j].s_mid).abs();
            if path.closed {
                ds = ds.min(path.total_len - ds);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sweep_path::{SweepPath, V3};

    fn cfg() -> SweepGateConfig {
        SweepGateConfig::default()
    }
    fn open(pts: Vec<V3>) -> SweepPath {
        SweepPath::new(pts, false, 0.0).unwrap()
    }
    fn closed(pts: Vec<V3>) -> SweepPath {
        SweepPath::new(pts, true, 0.0).unwrap()
    }

    #[test]
    fn straight_path_passes() {
        let path = open((0..10).map(|i| [i as f32 * 5.0, 0.0, 0.0]).collect());
        assert_eq!(check_sweep(&path, 2.0, &cfg()), SweepGateVerdict::Pass);
    }

    #[test]
    fn dense_straight_tube_does_not_false_collide() {
        // 0.5mm steps, fat 5mm profile: many segments within 2r in space,
        // but all path-neighbours → must still pass.
        let path = open((0..200).map(|i| [i as f32 * 0.5, 0.0, 0.0]).collect());
        assert_eq!(check_sweep(&path, 5.0, &cfg()), SweepGateVerdict::Pass);
    }

    #[test]
    fn hard_corner_fails_kink() {
        let mut pts: Vec<V3> = (0..6).map(|i| [i as f32 * 5.0, 0.0, 0.0]).collect();
        for i in 1..6 {
            pts.push([25.0, i as f32 * 5.0, 0.0]);
        }
        assert!(matches!(check_sweep(&open(pts), 2.0, &cfg()), SweepGateVerdict::FailKink(_)));
    }

    #[test]
    fn tight_arc_fails_curvature() {
        let r = 3.0_f32;
        let pts: Vec<V3> = (0..=16)
            .map(|i| {
                let a = i as f32 / 16.0 * std::f32::consts::FRAC_PI_2;
                [r * a.cos(), r * a.sin(), 0.0]
            })
            .collect();
        assert!(matches!(
            check_sweep(&open(pts), 5.0, &cfg()),
            SweepGateVerdict::FailCurvature(_)
        ));
    }

    #[test]
    fn gentle_arc_passes() {
        let r = 50.0_f32;
        let pts: Vec<V3> = (0..=40)
            .map(|i| {
                let a = i as f32 / 40.0 * std::f32::consts::FRAC_PI_2;
                [r * a.cos(), r * a.sin(), 0.0]
            })
            .collect();
        assert_eq!(check_sweep(&open(pts), 3.0, &cfg()), SweepGateVerdict::Pass);
    }

    /// Limaçon r(θ)=a+b·cosθ with b>a: a smooth analytic curve with an
    /// inner loop that crosses itself at the origin.
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
        let path = closed(limacon(20.0, 50.0, 120));
        let v = check_sweep(&path, 0.5, &cfg());
        assert!(matches!(v, SweepGateVerdict::FailCollision(_, _)), "got {v:?}");
    }

    #[test]
    fn convex_closed_loop_passes() {
        let pts: Vec<V3> = (0..72)
            .map(|i| {
                let a = i as f32 / 72.0 * std::f32::consts::TAU;
                [50.0 * a.cos(), 50.0 * a.sin(), 0.0]
            })
            .collect();
        assert_eq!(check_sweep(&closed(pts), 3.0, &cfg()), SweepGateVerdict::Pass);
    }
}
