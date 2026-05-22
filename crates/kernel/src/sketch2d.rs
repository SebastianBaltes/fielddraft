//! Kernel-side sketch2d bindings.
//!
//! K7.3 moved the data types `Aabb2`, `Sketch2dPrimitive` and
//! `Sketch2d` into the protocol crate so `Primitive::Extrude`
//! can carry a sketch profile directly in the wire form. This
//! module re-exports them and keeps the runtime acceleration
//! structure [`BoundedSketch2d`] — a BVH-wrapped mirror of
//! [`Sketch2d`] that caches per-node AABBs and prunes boolean
//! branches the same way [`crate::bvh::BoundedCsg`] does in 3D.

use serde::Serialize;
use std::collections::HashMap;

pub use fielddraft_protocol::sketch2d::{
    distance_point_to_aabb2, Aabb2, Sketch2d, Sketch2dPrimitive,
};

/// BVH-wrapped mirror of [`Sketch2d`]. Every node carries its
/// precomputed [`Aabb2`], and [`BoundedSketch2d::sdf`] uses it
/// to prune boolean branches.
#[derive(Debug, Clone)]
pub enum BoundedSketch2d {
    Empty,
    Prim {
        primitive: Sketch2dPrimitive,
        aabb: Aabb2,
    },
    Union {
        left: Box<BoundedSketch2d>,
        right: Box<BoundedSketch2d>,
        aabb: Aabb2,
    },
    Difference {
        left: Box<BoundedSketch2d>,
        right: Box<BoundedSketch2d>,
        aabb: Aabb2,
    },
    Intersection {
        left: Box<BoundedSketch2d>,
        right: Box<BoundedSketch2d>,
        aabb: Aabb2,
    },
    Offset {
        inner: Box<BoundedSketch2d>,
        distance: f32,
        aabb: Aabb2,
    },
}

impl BoundedSketch2d {
    pub fn from_sketch(sketch: &Sketch2d) -> Self {
        match sketch {
            Sketch2d::Empty => Self::Empty,
            Sketch2d::Prim(prim) => Self::Prim {
                primitive: prim.clone(),
                aabb: prim.aabb(),
            },
            Sketch2d::Union(a, b) => {
                let la = Self::from_sketch(a);
                let lb = Self::from_sketch(b);
                let aabb = Aabb2::union(la.aabb(), lb.aabb());
                Self::Union {
                    left: Box::new(la),
                    right: Box::new(lb),
                    aabb,
                }
            }
            Sketch2d::Difference(a, b) => {
                let la = Self::from_sketch(a);
                let lb = Self::from_sketch(b);
                let aabb = la.aabb();
                Self::Difference {
                    left: Box::new(la),
                    right: Box::new(lb),
                    aabb,
                }
            }
            Sketch2d::Intersection(a, b) => {
                let la = Self::from_sketch(a);
                let lb = Self::from_sketch(b);
                let aabb = la.aabb();
                Self::Intersection {
                    left: Box::new(la),
                    right: Box::new(lb),
                    aabb,
                }
            }
            Sketch2d::Offset { inner, distance } => {
                let li = Self::from_sketch(inner);
                let aabb = li.aabb().expanded(*distance);
                Self::Offset {
                    inner: Box::new(li),
                    distance: *distance,
                    aabb,
                }
            }
        }
    }

    pub fn aabb(&self) -> Aabb2 {
        match self {
            Self::Empty => Aabb2::EMPTY,
            Self::Prim { aabb, .. }
            | Self::Union { aabb, .. }
            | Self::Difference { aabb, .. }
            | Self::Intersection { aabb, .. }
            | Self::Offset { aabb, .. } => *aabb,
        }
    }

    /// Pruned signed-distance query. Matches [`Sketch2d::sdf`]
    /// for the same input tree up to f32 rounding, but
    /// short-circuits boolean branches whose AABB can't affect
    /// the running value. Same pruning rules as the 3D version
    /// in `bvh.rs`.
    pub fn sdf(&self, p: [f32; 2]) -> f32 {
        match self {
            Self::Empty => f32::INFINITY,
            Self::Prim { primitive, .. } => primitive.sdf(p),
            Self::Union { left, right, .. } => {
                let dist_left = distance_point_to_aabb2(p, left.aabb());
                let dist_right = distance_point_to_aabb2(p, right.aabb());
                let (first_node, second_node, second_bound) = if dist_left <= dist_right {
                    (&**left, &**right, dist_right)
                } else {
                    (&**right, &**left, dist_left)
                };
                let d_first = first_node.sdf(p);
                if second_bound > 0.0 && d_first < second_bound {
                    d_first
                } else {
                    d_first.min(second_node.sdf(p))
                }
            }
            Self::Difference { left, right, .. } => {
                let d_left = left.sdf(p);
                let dist_right = distance_point_to_aabb2(p, right.aabb());
                if dist_right > 0.0 && d_left >= -dist_right {
                    d_left
                } else {
                    d_left.max(-right.sdf(p))
                }
            }
            Self::Intersection { left, right, .. } => left.sdf(p).max(right.sdf(p)),
            Self::Offset { inner, distance, .. } => inner.sdf(p) - distance,
        }
    }
}

// ── Contour extraction via Marching Squares ─────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct ContourData {
    pub outer: Vec<[f32; 2]>,
    pub holes: Vec<Vec<[f32; 2]>>,
}

pub const CONTOUR_RESOLUTION: u32 = 128;

pub fn extract_contours(sketch: &BoundedSketch2d, resolution: u32) -> Vec<ContourData> {
    let aabb = sketch.aabb();
    if aabb.is_empty() {
        return vec![];
    }
    let width = aabb.max[0] - aabb.min[0];
    let height = aabb.max[1] - aabb.min[1];
    if width < 1e-8 || height < 1e-8 {
        return vec![];
    }

    let max_dim = width.max(height);
    let cell_size = max_dim / resolution as f32;
    let margin = cell_size * 2.0;
    let x0 = aabb.min[0] - margin;
    let y0 = aabb.min[1] - margin;
    let x1 = aabb.max[0] + margin;
    let y1 = aabb.max[1] + margin;
    let nx = ((x1 - x0) / cell_size).ceil() as usize;
    let ny = ((y1 - y0) / cell_size).ceil() as usize;
    let dx = (x1 - x0) / nx as f32;
    let dy = (y1 - y0) / ny as f32;

    let mut values = vec![0.0f32; (nx + 1) * (ny + 1)];
    for j in 0..=ny {
        for i in 0..=nx {
            values[j * (nx + 1) + i] = sketch.sdf([x0 + i as f32 * dx, y0 + j as f32 * dy]);
        }
    }

    let (segments, edge_points) = march_squares(sketch, &values, nx, ny, x0, y0, dx, dy);
    if segments.is_empty() {
        return vec![];
    }

    let loops = chain_segments(&segments, &edge_points);
    let epsilon = cell_size * 0.05;
    let simplified: Vec<Vec<[f32; 2]>> = loops
        .into_iter()
        .map(|pts| douglas_peucker_closed(&pts, epsilon))
        .filter(|pts| pts.len() >= 3)
        .collect();

    if simplified.is_empty() {
        return vec![];
    }
    classify_contours(simplified)
}

// ── Marching Squares ────────────────────────────────────────────

#[derive(Hash, Eq, PartialEq, Copy, Clone, Debug)]
enum EdgeKey {
    H(usize, usize),
    V(usize, usize),
}

fn march_squares(
    sketch: &BoundedSketch2d,
    values: &[f32],
    nx: usize,
    ny: usize,
    x0: f32,
    y0: f32,
    dx: f32,
    dy: f32,
) -> (Vec<(EdgeKey, EdgeKey)>, HashMap<EdgeKey, [f32; 2]>) {
    let mut segments: Vec<(EdgeKey, EdgeKey)> = Vec::new();
    let mut pts: HashMap<EdgeKey, [f32; 2]> = HashMap::new();

    for j in 0..ny {
        for i in 0..nx {
            let v0 = values[j * (nx + 1) + i];
            let v1 = values[j * (nx + 1) + (i + 1)];
            let v2 = values[(j + 1) * (nx + 1) + (i + 1)];
            let v3 = values[(j + 1) * (nx + 1) + i];

            let ci = ((v0 < 0.0) as u8)
                | (((v1 < 0.0) as u8) << 1)
                | (((v2 < 0.0) as u8) << 2)
                | (((v3 < 0.0) as u8) << 3);
            if ci == 0 || ci == 15 {
                continue;
            }

            let interp_h = |je: usize, va: f32, vb: f32| -> (EdgeKey, [f32; 2]) {
                let t = va / (va - vb);
                (EdgeKey::H(i, je), [x0 + (i as f32 + t) * dx, y0 + je as f32 * dy])
            };
            let interp_v = |ie: usize, va: f32, vb: f32| -> (EdgeKey, [f32; 2]) {
                let t = va / (va - vb);
                (EdgeKey::V(ie, j), [x0 + ie as f32 * dx, y0 + (j as f32 + t) * dy])
            };

            let bot = || interp_h(j, v0, v1);
            let top = || interp_h(j + 1, v3, v2);
            let lft = || interp_v(i, v0, v3);
            let rgt = || interp_v(i + 1, v1, v2);

            let mut add = |a: (EdgeKey, [f32; 2]), b: (EdgeKey, [f32; 2])| {
                segments.push((a.0, b.0));
                pts.insert(a.0, a.1);
                pts.insert(b.0, b.1);
            };

            match ci {
                1 | 14 => add(bot(), lft()),
                2 | 13 => add(bot(), rgt()),
                3 | 12 => add(lft(), rgt()),
                4 | 11 => add(rgt(), top()),
                6 | 9  => add(bot(), top()),
                7 | 8  => add(lft(), top()),
                5 => {
                    let cv = sketch.sdf([
                        x0 + (i as f32 + 0.5) * dx,
                        y0 + (j as f32 + 0.5) * dy,
                    ]);
                    if cv < 0.0 {
                        add(lft(), top());
                        add(bot(), rgt());
                    } else {
                        add(bot(), lft());
                        add(rgt(), top());
                    }
                }
                10 => {
                    let cv = sketch.sdf([
                        x0 + (i as f32 + 0.5) * dx,
                        y0 + (j as f32 + 0.5) * dy,
                    ]);
                    if cv < 0.0 {
                        add(top(), rgt());
                        add(bot(), lft());
                    } else {
                        add(lft(), top());
                        add(bot(), rgt());
                    }
                }
                _ => {}
            }
        }
    }
    (segments, pts)
}

// ── Segment chaining ────────────────────────────────────────────

fn chain_segments(
    segments: &[(EdgeKey, EdgeKey)],
    pts: &HashMap<EdgeKey, [f32; 2]>,
) -> Vec<Vec<[f32; 2]>> {
    let mut adj: HashMap<EdgeKey, Vec<(EdgeKey, usize)>> = HashMap::new();
    for (idx, &(a, b)) in segments.iter().enumerate() {
        adj.entry(a).or_default().push((b, idx));
        adj.entry(b).or_default().push((a, idx));
    }

    let mut used = vec![false; segments.len()];
    let mut loops = Vec::new();

    for start_idx in 0..segments.len() {
        if used[start_idx] {
            continue;
        }
        used[start_idx] = true;
        let (a0, b0) = segments[start_idx];
        let mut contour = vec![pts[&a0], pts[&b0]];
        let start_key = a0;
        let mut current_key = b0;

        loop {
            if current_key == start_key {
                contour.pop();
                break;
            }
            let mut found = false;
            if let Some(neighbors) = adj.get(&current_key) {
                for &(other, seg_idx) in neighbors {
                    if !used[seg_idx] {
                        used[seg_idx] = true;
                        contour.push(pts[&other]);
                        current_key = other;
                        found = true;
                        break;
                    }
                }
            }
            if !found {
                break;
            }
        }
        if contour.len() >= 3 {
            loops.push(contour);
        }
    }
    loops
}

// ── Douglas-Peucker simplification for closed contours ──────────

fn douglas_peucker_closed(points: &[[f32; 2]], epsilon: f32) -> Vec<[f32; 2]> {
    let n = points.len();
    if n <= 4 {
        return points.to_vec();
    }
    let mut farthest_idx = 0;
    let mut max_dist = 0.0f32;
    let cx = points.iter().map(|p| p[0]).sum::<f32>() / n as f32;
    let cy = points.iter().map(|p| p[1]).sum::<f32>() / n as f32;
    for (i, p) in points.iter().enumerate() {
        let d = (p[0] - cx) * (p[0] - cx) + (p[1] - cy) * (p[1] - cy);
        if d > max_dist {
            max_dist = d;
            farthest_idx = i;
        }
    }
    let mut rotated = Vec::with_capacity(n + 1);
    rotated.extend_from_slice(&points[farthest_idx..]);
    rotated.extend_from_slice(&points[..farthest_idx]);
    rotated.push(rotated[0]);
    let simplified = douglas_peucker(&rotated, epsilon);
    if simplified.len() > 1 && simplified.last() == simplified.first() {
        simplified[..simplified.len() - 1].to_vec()
    } else {
        simplified
    }
}

fn douglas_peucker(points: &[[f32; 2]], epsilon: f32) -> Vec<[f32; 2]> {
    if points.len() <= 2 {
        return points.to_vec();
    }
    let start = points[0];
    let end = points[points.len() - 1];
    let mut max_dist = 0.0f32;
    let mut max_idx = 0;
    for (i, &p) in points.iter().enumerate().skip(1).take(points.len() - 2) {
        let d = perpendicular_distance(p, start, end);
        if d > max_dist {
            max_dist = d;
            max_idx = i;
        }
    }
    if max_dist > epsilon {
        let mut left = douglas_peucker(&points[..=max_idx], epsilon);
        let right = douglas_peucker(&points[max_idx..], epsilon);
        left.pop();
        left.extend_from_slice(&right);
        left
    } else {
        vec![start, end]
    }
}

fn perpendicular_distance(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let dx = b[0] - a[0];
    let dy = b[1] - a[1];
    let len_sq = dx * dx + dy * dy;
    if len_sq < 1e-12 {
        return ((p[0] - a[0]).powi(2) + (p[1] - a[1]).powi(2)).sqrt();
    }
    ((dy * p[0] - dx * p[1] + b[0] * a[1] - b[1] * a[0]).abs()) / len_sq.sqrt()
}

// ── Winding classification + hole assignment ────────────────────

fn signed_area(pts: &[[f32; 2]]) -> f32 {
    let n = pts.len();
    let mut area = 0.0f32;
    for i in 0..n {
        let j = (i + 1) % n;
        area += pts[i][0] * pts[j][1] - pts[j][0] * pts[i][1];
    }
    area * 0.5
}

fn point_in_polygon(pt: [f32; 2], poly: &[[f32; 2]]) -> bool {
    let mut inside = false;
    let n = poly.len();
    let mut j = n - 1;
    for i in 0..n {
        let yi = poly[i][1];
        let yj = poly[j][1];
        if ((yi > pt[1]) != (yj > pt[1]))
            && (pt[0] < (poly[j][0] - poly[i][0]) * (pt[1] - yi) / (yj - yi) + poly[i][0])
        {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn classify_contours(mut contours: Vec<Vec<[f32; 2]>>) -> Vec<ContourData> {
    if contours.is_empty() {
        return vec![];
    }
    let n = contours.len();
    let areas: Vec<f32> = contours.iter().map(|c| signed_area(c).abs()).collect();

    // Sort by descending area — largest contour is outermost
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| areas[b].partial_cmp(&areas[a]).unwrap());

    // Determine nesting depth via containment: even depth = outer, odd = hole
    let mut depth = vec![0usize; n];
    let mut parent = vec![None::<usize>; n];
    for rank in 1..n {
        let i = order[rank];
        let test_pt = contours[i][0];
        for &j in order[..rank].iter().rev() {
            if point_in_polygon(test_pt, &contours[j]) {
                parent[i] = Some(j);
                depth[i] = depth[j] + 1;
                break;
            }
        }
    }

    // Ensure winding: outers CCW (positive area), holes CW (negative area)
    for i in 0..n {
        let sa = signed_area(&contours[i]);
        if depth[i] % 2 == 0 && sa < 0.0 {
            contours[i].reverse();
        } else if depth[i] % 2 == 1 && sa > 0.0 {
            contours[i].reverse();
        }
    }

    // Collect outers and attach their direct-child holes
    let mut results: Vec<ContourData> = Vec::new();
    for i in 0..n {
        if depth[i] % 2 == 0 {
            let holes: Vec<Vec<[f32; 2]>> = (0..n)
                .filter(|&j| parent[j] == Some(i) && depth[j] % 2 == 1)
                .map(|j| contours[j].clone())
                .collect();
            results.push(ContourData {
                outer: contours[i].clone(),
                holes,
            });
        }
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── primitives ────────────────────────────────────────────────

    #[test]
    fn circle_sdf_zero_on_boundary_negative_inside_positive_outside() {
        let c = Sketch2dPrimitive::Circle {
            center: [0.0, 0.0],
            radius: 5.0,
        };
        assert!((c.sdf([5.0, 0.0])).abs() < 1e-6);
        assert!(c.sdf([0.0, 0.0]) < 0.0);
        assert!(c.sdf([10.0, 0.0]) > 0.0);
    }

    #[test]
    fn rectangle_sdf_signs_match_point_containment() {
        let r = Sketch2dPrimitive::Rectangle {
            center: [0.0, 0.0],
            half_extents: [3.0, 2.0],
        };
        assert!(r.sdf([0.0, 0.0]) < 0.0);
        assert!(r.sdf([5.0, 0.0]) > 0.0);
    }

    #[test]
    fn bounded_sketch_sdf_matches_naive_tree_on_a_washer() {
        let outer = Sketch2d::Prim(Sketch2dPrimitive::Circle {
            center: [0.0, 0.0],
            radius: 10.0,
        });
        let inner = Sketch2d::Prim(Sketch2dPrimitive::Circle {
            center: [0.0, 0.0],
            radius: 4.0,
        });
        let washer = Sketch2d::Difference(Box::new(outer), Box::new(inner));
        let bounded = BoundedSketch2d::from_sketch(&washer);
        for (px, py) in [(0.0, 0.0), (5.0, 0.0), (3.0, 0.0), (12.0, 0.0)] {
            let a = washer.sdf([px, py]);
            let b = bounded.sdf([px, py]);
            assert!((a - b).abs() < 1e-4, "{} != {}", a, b);
        }
    }

    // ─── contour extraction ────────────────────────────────────────

    #[test]
    fn circle_contour_has_one_outer_no_holes() {
        let sketch = Sketch2d::Prim(Sketch2dPrimitive::Circle {
            center: [0.0, 0.0],
            radius: 10.0,
        });
        let bounded = BoundedSketch2d::from_sketch(&sketch);
        let contours = extract_contours(&bounded, CONTOUR_RESOLUTION);
        assert_eq!(contours.len(), 1, "expected 1 contour for circle");
        assert!(contours[0].holes.is_empty(), "circle should have no holes");
        assert!(contours[0].outer.len() >= 12, "contour too coarse: {} pts", contours[0].outer.len());
    }

    #[test]
    fn rectangle_contour_simplifies_to_four_corners() {
        let sketch = Sketch2d::Prim(Sketch2dPrimitive::Rectangle {
            center: [0.0, 0.0],
            half_extents: [20.0, 10.0],
        });
        let bounded = BoundedSketch2d::from_sketch(&sketch);
        let contours = extract_contours(&bounded, CONTOUR_RESOLUTION);
        assert_eq!(contours.len(), 1);
        // DP should simplify straight edges, leaving close to 4 corners
        assert!(contours[0].outer.len() <= 8, "expected ~4 corners, got {}", contours[0].outer.len());
    }

    #[test]
    fn washer_contour_has_one_outer_and_one_hole() {
        let outer = Sketch2d::Prim(Sketch2dPrimitive::Circle {
            center: [0.0, 0.0],
            radius: 10.0,
        });
        let inner = Sketch2d::Prim(Sketch2dPrimitive::Circle {
            center: [0.0, 0.0],
            radius: 4.0,
        });
        let washer = Sketch2d::Difference(Box::new(outer), Box::new(inner));
        let bounded = BoundedSketch2d::from_sketch(&washer);
        let contours = extract_contours(&bounded, CONTOUR_RESOLUTION);
        assert_eq!(contours.len(), 1, "washer should be 1 contour with a hole");
        assert_eq!(contours[0].holes.len(), 1, "washer should have 1 hole");
    }

    #[test]
    fn two_separate_circles_produce_two_contours() {
        let c1 = Sketch2d::Prim(Sketch2dPrimitive::Circle {
            center: [-20.0, 0.0],
            radius: 5.0,
        });
        let c2 = Sketch2d::Prim(Sketch2dPrimitive::Circle {
            center: [20.0, 0.0],
            radius: 5.0,
        });
        let union = Sketch2d::Union(Box::new(c1), Box::new(c2));
        let bounded = BoundedSketch2d::from_sketch(&union);
        let contours = extract_contours(&bounded, CONTOUR_RESOLUTION);
        assert_eq!(contours.len(), 2, "expected 2 separate contours");
    }

    #[test]
    fn contour_area_approximates_circle_area() {
        let radius = 10.0f32;
        let sketch = Sketch2d::Prim(Sketch2dPrimitive::Circle {
            center: [0.0, 0.0],
            radius,
        });
        let bounded = BoundedSketch2d::from_sketch(&sketch);
        let contours = extract_contours(&bounded, CONTOUR_RESOLUTION);
        assert_eq!(contours.len(), 1);
        let area = signed_area(&contours[0].outer).abs();
        let expected = std::f32::consts::PI * radius * radius;
        let rel_error = (area - expected).abs() / expected;
        assert!(rel_error < 0.05, "area {area} vs expected {expected}, error {rel_error}");
    }
}
