//! Modular path sources for sweeps.
//!
//! The Gatekeeper, the mesher and the SDF consume ONLY a [`SampledPath`] (a
//! 3D polyline + a closed flag) — how that spine was DEFINED is fully
//! decoupled behind [`PathProvider`]. Nothing downstream of a path may couple
//! to a particular input method; new methods plug in as new providers.
//!
//! A provider is self-contained: it is built once (resolving a document
//! path-source + feature context into whatever it needs), then `sample()`s
//! its spine with no further context — so the consumer side never sees the
//! source.
//!
//! Roadmap of methods (one [`PathMethod`] / provider each):
//!
//! Phase 1 & 2 (short/mid term)
//!   * [`PathMethod::Sketch2d`]            — planar path from a 2D sketch (v1).
//!   * [`PathMethod::EdgeChain`]           — selected existing model edges.
//!   * [`PathMethod::OrthogonalSketches`]  — space curve from two planar
//!                                           sketches (projection / crossing).
//!   * [`PathMethod::Gizmo3d`]             — control points routed in space +
//!                                           spline interpolation. *(built)*
//!
//! Phase 3 (long term)
//!   * [`PathMethod::IntersectionCurve`]   — intersection of two bodies.
//!   * [`PathMethod::SurfaceProjection`]   — projection / wrapping onto a face.
//!   * [`PathMethod::OffsetCurve`]         — 3D offset of an existing curve.
//!   * [`PathMethod::EquationDriven`]      — equation-driven space curve.
//!   * [`PathMethod::AutoRoute`]           — Manhattan routing between points.

use crate::sweep_path::{norm, sub, SweepPath, V3};

/// The sole contract handed downstream: a sampled spine polyline.
#[derive(Debug, Clone, PartialEq)]
pub struct SampledPath {
    pub points: Vec<V3>,
    pub closed: bool,
}

impl SampledPath {
    /// Build the shared [`SweepPath`] (frames + arclength + BVH). `None` for
    /// fewer than two points.
    pub fn into_sweep_path(self) -> Option<SweepPath> {
        SweepPath::new(self.points, self.closed, 0.0)
    }
}

/// Which definition method produced (or failed to produce) a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathMethod {
    // Phase 1 & 2.
    Sketch2d,
    EdgeChain,
    OrthogonalSketches,
    Gizmo3d,
    // Phase 3.
    IntersectionCurve,
    SurfaceProjection,
    OffsetCurve,
    EquationDriven,
    AutoRoute,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PathError {
    /// Fewer than two defining points / an empty source.
    Empty,
    /// The source resolved but is geometrically unusable.
    Degenerate(&'static str),
    /// A declared method whose provider is not implemented yet.
    NotImplemented(PathMethod),
}

/// A modular path source. Once constructed it samples its spine without
/// further context, keeping every consumer fully decoupled from the source.
pub trait PathProvider {
    fn method(&self) -> PathMethod;
    fn sample(&self) -> Result<SampledPath, PathError>;
}

// ── Provider: existing model edges (3D edge chain) ─────────────────

/// Sweep along selected B-Rep edges. The feature resolves an edge query into
/// world-space edge polylines (via `resolve_fillet_edges`); this provider
/// greedily chains them end-to-end into one ordered spine (reversing pieces
/// as needed) and detects a closed loop.
pub struct EdgeChainPath {
    pub polylines: Vec<Vec<V3>>,
}

const EDGE_JOIN_TOL: f32 = 1e-2;

impl PathProvider for EdgeChainPath {
    fn method(&self) -> PathMethod {
        PathMethod::EdgeChain
    }

    fn sample(&self) -> Result<SampledPath, PathError> {
        let mut remaining: Vec<Vec<V3>> =
            self.polylines.iter().filter(|p| p.len() >= 2).cloned().collect();
        if remaining.is_empty() {
            return Err(PathError::Empty);
        }
        let near = |a: V3, b: V3| norm(sub(a, b)) < EDGE_JOIN_TOL;

        let mut chain = remaining.remove(0);
        loop {
            let tail = *chain.last().unwrap();
            let head = chain[0];
            let mut extended = false;
            for i in 0..remaining.len() {
                let ps = remaining[i][0];
                let pe = *remaining[i].last().unwrap();
                if near(tail, ps) {
                    let p = remaining.remove(i);
                    chain.extend_from_slice(&p[1..]);
                } else if near(tail, pe) {
                    let mut p = remaining.remove(i);
                    p.reverse();
                    chain.extend_from_slice(&p[1..]);
                } else if near(head, pe) {
                    let p = remaining.remove(i);
                    let mut joined = p;
                    joined.extend_from_slice(&chain[1..]);
                    chain = joined;
                } else if near(head, ps) {
                    let mut p = remaining.remove(i);
                    p.reverse();
                    p.extend_from_slice(&chain[1..]);
                    chain = p;
                } else {
                    continue;
                }
                extended = true;
                break;
            }
            if !extended {
                break;
            }
        }

        // Closed-loop detection: a closed model edge (a circular rim) is
        // sampled WITHOUT repeating its closing vertex, so the ends sit one
        // sampling step apart, not coincident. Treat the chain as closed when
        // the end gap is within ~1.5× the local segment spacing.
        let n = chain.len();
        let last = chain[n - 1];
        let end_gap = norm(sub(chain[0], last));
        let step = norm(sub(chain[1], chain[0]))
            .max(norm(sub(chain[n - 1], chain[n - 2])));
        let closed = n > 3 && end_gap < (EDGE_JOIN_TOL).max(1.5 * step);
        let points = if closed && near(chain[0], last) {
            chain[..n - 1].to_vec() // drop the exactly-repeated closing point
        } else {
            chain
        };
        if points.len() < 2 {
            return Err(PathError::Degenerate("edge chain too short"));
        }
        Ok(SampledPath { points, closed })
    }
}

// ── Provider: 3D gizmo routing (control points + Catmull-Rom spline) ─

/// Route a spine through control points placed in space, interpolated with a
/// uniform Catmull-Rom spline (passes through every control point with C¹
/// continuity). Two nodes degenerate to a straight segment.
pub struct Gizmo3dPath {
    pub nodes: Vec<V3>,
    pub closed: bool,
    /// Samples emitted per control-point span (≥ 1).
    pub samples_per_span: usize,
}

impl PathProvider for Gizmo3dPath {
    fn method(&self) -> PathMethod {
        PathMethod::Gizmo3d
    }

    fn sample(&self) -> Result<SampledPath, PathError> {
        let n = self.nodes.len();
        if n < 2 {
            return Err(PathError::Empty);
        }
        if n == 2 && !self.closed {
            return Ok(SampledPath { points: self.nodes.clone(), closed: false });
        }
        let points = catmull_rom(&self.nodes, self.closed, self.samples_per_span.max(1));
        Ok(SampledPath { points, closed: self.closed })
    }
}

// ── Provider: planar 2D sketch curve lifted into 3D ────────────────

/// A planar curve from a 2D sketch, lifted into 3D via the sketch's plane
/// frame. The feature resolves the sketch's ordered boundary curve and its
/// frame (with context); this provider does the method-specific 2D→3D lift
/// (`world = origin + x·x_axis + y·y_axis`). A closed loop sweeps around
/// itself.
pub struct Sketch2dPath {
    /// Ordered 2D curve points in the sketch plane.
    pub curve: Vec<[f32; 2]>,
    pub origin: V3,
    pub x_axis: V3,
    pub y_axis: V3,
    pub closed: bool,
}

impl PathProvider for Sketch2dPath {
    fn method(&self) -> PathMethod {
        PathMethod::Sketch2d
    }

    fn sample(&self) -> Result<SampledPath, PathError> {
        if self.curve.len() < 2 {
            return Err(PathError::Empty);
        }
        let points = self
            .curve
            .iter()
            .map(|&[x, y]| {
                [
                    self.origin[0] + x * self.x_axis[0] + y * self.y_axis[0],
                    self.origin[1] + x * self.x_axis[1] + y * self.y_axis[1],
                    self.origin[2] + x * self.x_axis[2] + y * self.y_axis[2],
                ]
            })
            .collect();
        Ok(SampledPath { points, closed: self.closed })
    }
}

/// Sample a uniform Catmull-Rom spline through `nodes`. Open ends use
/// reflected phantom points so the curve still passes through the endpoints.
fn catmull_rom(nodes: &[V3], closed: bool, sps: usize) -> Vec<V3> {
    let n = nodes.len() as isize;
    let get = |i: isize| -> V3 {
        if closed {
            nodes[((i % n + n) % n) as usize]
        } else if i < 0 {
            reflect(nodes[0], nodes[1])
        } else if i >= n {
            reflect(nodes[(n - 1) as usize], nodes[(n - 2) as usize])
        } else {
            nodes[i as usize]
        }
    };
    let spans = if closed { n } else { n - 1 };
    let mut out = Vec::with_capacity((spans as usize) * sps + 1);
    for s in 0..spans {
        let p0 = get(s - 1);
        let p1 = get(s);
        let p2 = get(s + 1);
        let p3 = get(s + 2);
        for k in 0..sps {
            out.push(cr_point(p0, p1, p2, p3, k as f32 / sps as f32));
        }
    }
    if !closed {
        out.push(nodes[(n - 1) as usize]); // final endpoint (t=1 of last span)
    }
    out
}

/// Phantom control point: reflect `a` across the boundary node `... ` — i.e.
/// `2·boundary − inner` gives a tangent-preserving virtual neighbour.
fn reflect(boundary: V3, inner: V3) -> V3 {
    [
        2.0 * boundary[0] - inner[0],
        2.0 * boundary[1] - inner[1],
        2.0 * boundary[2] - inner[2],
    ]
}

/// Uniform Catmull-Rom (τ=0.5) point on the P1→P2 span at `t∈[0,1]`.
fn cr_point(p0: V3, p1: V3, p2: V3, p3: V3, t: f32) -> V3 {
    let t2 = t * t;
    let t3 = t2 * t;
    let mut out = [0.0_f32; 3];
    for k in 0..3 {
        out[k] = 0.5
            * (2.0 * p1[k]
                + (-p0[k] + p2[k]) * t
                + (2.0 * p0[k] - 5.0 * p1[k] + 4.0 * p2[k] - p3[k]) * t2
                + (-p0[k] + 3.0 * p1[k] - 3.0 * p2[k] + p3[k]) * t3);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sweep_gate::{check_sweep, SweepGateConfig, SweepGateVerdict};

    fn approx_pt(a: V3, b: V3, eps: f32) -> bool {
        (a[0] - b[0]).abs() < eps && (a[1] - b[1]).abs() < eps && (a[2] - b[2]).abs() < eps
    }

    #[test]
    fn two_nodes_make_a_straight_segment() {
        let g = Gizmo3dPath { nodes: vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0]], closed: false, samples_per_span: 8 };
        let sp = g.sample().unwrap();
        assert_eq!(sp.points.len(), 2);
        assert!(!sp.closed);
    }

    #[test]
    fn fewer_than_two_nodes_is_empty() {
        let g = Gizmo3dPath { nodes: vec![[0.0, 0.0, 0.0]], closed: false, samples_per_span: 4 };
        assert_eq!(g.sample(), Err(PathError::Empty));
    }

    #[test]
    fn spline_passes_through_every_control_node() {
        let nodes = vec![
            [0.0, 0.0, 0.0],
            [10.0, 5.0, 0.0],
            [20.0, -3.0, 4.0],
            [30.0, 0.0, 0.0],
        ];
        let sps = 10;
        let g = Gizmo3dPath { nodes: nodes.clone(), closed: false, samples_per_span: sps };
        let sp = g.sample().unwrap();
        // Span start s*sps lands exactly on node s; final point is the last node.
        for (s, node) in nodes.iter().enumerate().take(nodes.len() - 1) {
            assert!(approx_pt(sp.points[s * sps], *node, 1e-4), "node {s}");
        }
        assert!(approx_pt(*sp.points.last().unwrap(), *nodes.last().unwrap(), 1e-4));
    }

    #[test]
    fn closed_spline_wraps_without_an_appended_endpoint() {
        let nodes = vec![
            [0.0, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [10.0, 10.0, 0.0],
            [0.0, 10.0, 0.0],
        ];
        let sps = 6;
        let g = Gizmo3dPath { nodes: nodes.clone(), closed: true, samples_per_span: sps };
        let sp = g.sample().unwrap();
        assert!(sp.closed);
        assert_eq!(sp.points.len(), nodes.len() * sps); // no separate endpoint
        for (s, node) in nodes.iter().enumerate() {
            assert!(approx_pt(sp.points[s * sps], *node, 1e-4), "node {s}");
        }
    }

    #[test]
    fn edge_chain_joins_polylines_into_one_ordered_spine() {
        // Three edges that meet end-to-end (one reversed) → a single chain.
        let g = EdgeChainPath {
            polylines: vec![
                vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0]],
                vec![[20.0, 0.0, 0.0], [10.0, 0.0, 0.0]], // reversed orientation
                vec![[20.0, 0.0, 0.0], [20.0, 10.0, 0.0]],
            ],
        };
        let sp = g.sample().unwrap();
        assert!(!sp.closed);
        assert!(approx_pt(sp.points[0], [0.0, 0.0, 0.0], 1e-5));
        assert!(approx_pt(*sp.points.last().unwrap(), [20.0, 10.0, 0.0], 1e-5));
    }

    #[test]
    fn edge_chain_closes_a_sampled_circle_and_passes_the_gate() {
        // A circular rim edge sampled as 32 points over 0..348.75° (the closing
        // vertex is NOT repeated) — must be detected as CLOSED so the sweep
        // gate doesn't see the near-touching ends as a self-intersection.
        let n = 32;
        let pts: Vec<V3> = (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                [40.0 * a.cos(), 0.0, 40.0 * a.sin()]
            })
            .collect();
        let g = EdgeChainPath { polylines: vec![pts] };
        let sp_sampled = g.sample().unwrap();
        assert!(sp_sampled.closed, "a sampled full circle must be closed");

        let path = sp_sampled.into_sweep_path().unwrap();
        let v = crate::sweep_gate::check_sweep(&path, 5.66, &Default::default());
        assert_eq!(v, crate::sweep_gate::SweepGateVerdict::Pass, "closed circle passes");
    }

    #[test]
    fn edge_chain_detects_a_closed_loop() {
        let g = EdgeChainPath {
            polylines: vec![
                vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0]],
                vec![[10.0, 0.0, 0.0], [10.0, 10.0, 0.0]],
                vec![[10.0, 10.0, 0.0], [0.0, 10.0, 0.0]],
                vec![[0.0, 10.0, 0.0], [0.0, 0.0, 0.0]],
            ],
        };
        let sp = g.sample().unwrap();
        assert!(sp.closed, "four edges forming a square → closed");
    }

    #[test]
    fn sketch2d_provider_lifts_the_curve_onto_its_plane() {
        // A curve on the XZ plane (x_axis=+X, y_axis=+Z): 2D (x,y) → 3D (x,0,y),
        // shifted by the plane origin.
        let g = Sketch2dPath {
            curve: vec![[10.0, 0.0], [10.0, 20.0], [-10.0, 20.0]],
            origin: [0.0, 5.0, 0.0],
            x_axis: [1.0, 0.0, 0.0],
            y_axis: [0.0, 0.0, 1.0],
            closed: true,
        };
        let sp = g.sample().unwrap();
        assert!(sp.closed);
        assert!(approx_pt(sp.points[0], [10.0, 5.0, 0.0], 1e-5));
        assert!(approx_pt(sp.points[1], [10.0, 5.0, 20.0], 1e-5));
        assert!(approx_pt(sp.points[2], [-10.0, 5.0, 20.0], 1e-5));
    }

    #[test]
    fn provider_feeds_the_gatekeeper_end_to_end() {
        // A gentle S routed in space → spline → SweepPath → Gatekeeper passes.
        let g = Gizmo3dPath {
            nodes: vec![
                [0.0, 0.0, 0.0],
                [40.0, 30.0, 0.0],
                [80.0, -30.0, 10.0],
                [120.0, 0.0, 0.0],
            ],
            closed: false,
            samples_per_span: 24,
        };
        let path = g.sample().unwrap().into_sweep_path().unwrap();
        let v = check_sweep(&path, 2.0, &SweepGateConfig::default());
        assert_eq!(v, SweepGateVerdict::Pass, "gentle routed spline should pass");
    }
}
