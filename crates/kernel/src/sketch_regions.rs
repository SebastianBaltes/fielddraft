//! K8.13: Sketch region detection via flood-fill on a 2D SDF grid.
//!
//! Evaluates the sketch's SDF on a uniform grid, identifies
//! connected negative (inside) regions, and returns metadata
//! per region (centroid, AABB, area estimate). The extrude
//! feature can then consume a specific region instead of the
//! entire sketch profile.

use fielddraft_protocol::sketch2d::{Aabb2, Sketch2d};
use serde::Serialize;

/// One connected inside-region of a sketch profile.
#[derive(Debug, Clone, Serialize)]
pub struct SketchRegion {
    /// Zero-based region index (stable within one analysis).
    pub index: u32,
    /// Centroid of the region in sketch-plane coordinates. NB: for a
    /// non-convex region (a ring) the centroid can fall in the HOLE, so
    /// it is NOT a reliable inside-point — use [`SketchRegion::interior`]
    /// for membership tests.
    pub centroid: [f32; 2],
    /// A point GUARANTEED to lie inside the region (a flood-fill cell
    /// centre). Used to restrict the full sketch profile down to this one
    /// region — see [`restrict_sketch_to_point`].
    pub interior: [f32; 2],
    /// Axis-aligned bounding box of the region.
    pub aabb: Aabb2,
    /// Approximate area in mm² (cell count × cell area).
    pub area: f32,
}

/// Analyse a sketch and return all connected inside-regions.
/// `resolution` controls the grid density (cells per axis within
/// the sketch's AABB, padded by 10%).
pub fn find_regions(sketch: &Sketch2d, resolution: u32) -> Vec<SketchRegion> {
    let bb = sketch.aabb();
    if bb.is_empty() {
        return Vec::new();
    }

    // Pad the AABB by 10% so edges aren't clipped.
    let pad_x = (bb.max[0] - bb.min[0]) * 0.1;
    let pad_y = (bb.max[1] - bb.min[1]) * 0.1;
    let min_x = bb.min[0] - pad_x;
    let max_x = bb.max[0] + pad_x;
    let min_y = bb.min[1] - pad_y;
    let max_y = bb.max[1] + pad_y;

    let res = resolution.max(8) as usize;
    let dx = (max_x - min_x) / res as f32;
    let dy = (max_y - min_y) / res as f32;
    let cell_area = dx * dy;

    // Evaluate SDF on the grid: true = inside (negative SDF).
    let mut inside = vec![false; res * res];
    for iy in 0..res {
        for ix in 0..res {
            let px = min_x + (ix as f32 + 0.5) * dx;
            let py = min_y + (iy as f32 + 0.5) * dy;
            let d = sketch.sdf([px, py]);
            inside[iy * res + ix] = d < 0.0;
        }
    }

    // Flood-fill to find connected components.
    let mut labels = vec![0u32; res * res];
    let mut next_label = 1u32;
    // The seed cell of each flood fill is a guaranteed-interior representative
    // point for that region (cell index → world point below).
    let mut seed_cell: Vec<usize> = Vec::new();

    for start in 0..(res * res) {
        if !inside[start] || labels[start] != 0 {
            continue;
        }
        // BFS flood fill.
        let label = next_label;
        next_label += 1;
        seed_cell.push(start);
        let mut queue = vec![start];
        labels[start] = label;
        while let Some(idx) = queue.pop() {
            let ix = idx % res;
            let iy = idx / res;
            for (nx, ny) in neighbors(ix, iy, res) {
                let ni = ny * res + nx;
                if inside[ni] && labels[ni] == 0 {
                    labels[ni] = label;
                    queue.push(ni);
                }
            }
        }
    }

    let region_count = (next_label - 1) as usize;
    if region_count == 0 {
        return Vec::new();
    }

    // Accumulate per-region stats.
    let mut sum_x = vec![0.0_f64; region_count];
    let mut sum_y = vec![0.0_f64; region_count];
    let mut count = vec![0u32; region_count];
    let mut r_min_x = vec![f32::INFINITY; region_count];
    let mut r_max_x = vec![f32::NEG_INFINITY; region_count];
    let mut r_min_y = vec![f32::INFINITY; region_count];
    let mut r_max_y = vec![f32::NEG_INFINITY; region_count];

    for iy in 0..res {
        for ix in 0..res {
            let lbl = labels[iy * res + ix];
            if lbl == 0 {
                continue;
            }
            let ri = (lbl - 1) as usize;
            let px = min_x + (ix as f32 + 0.5) * dx;
            let py = min_y + (iy as f32 + 0.5) * dy;
            sum_x[ri] += px as f64;
            sum_y[ri] += py as f64;
            count[ri] += 1;
            r_min_x[ri] = r_min_x[ri].min(px);
            r_max_x[ri] = r_max_x[ri].max(px);
            r_min_y[ri] = r_min_y[ri].min(py);
            r_max_y[ri] = r_max_y[ri].max(py);
        }
    }

    (0..region_count)
        .filter(|&i| count[i] > 0)
        .map(|i| {
            let c = count[i] as f64;
            let seed = seed_cell[i];
            let sx = seed % res;
            let sy = seed / res;
            SketchRegion {
                index: i as u32,
                centroid: [(sum_x[i] / c) as f32, (sum_y[i] / c) as f32],
                interior: [
                    min_x + (sx as f32 + 0.5) * dx,
                    min_y + (sy as f32 + 0.5) * dy,
                ],
                aabb: Aabb2 {
                    min: [r_min_x[i], r_min_y[i]],
                    max: [r_max_x[i], r_max_y[i]],
                },
                area: count[i] as f32 * cell_area,
            }
        })
        .collect()
}

/// Index of the connected region containing the sketch-space point `p`, or
/// `None` when `p` is outside every region (ticket dhjn). Uses the SAME
/// flood-fill grid as [`find_regions`], so the returned index addresses the
/// same region a `qRegion(sketch_id, i)` query resolves to. The point is
/// snapped to the nearest grid cell; a click anywhere inside a region's fill
/// therefore maps to that region's index.
pub fn region_index_at_point(sketch: &Sketch2d, p: [f32; 2], resolution: u32) -> Option<u32> {
    let bb = sketch.aabb();
    if bb.is_empty() {
        return None;
    }
    let pad_x = (bb.max[0] - bb.min[0]) * 0.1;
    let pad_y = (bb.max[1] - bb.min[1]) * 0.1;
    let min_x = bb.min[0] - pad_x;
    let max_x = bb.max[0] + pad_x;
    let min_y = bb.min[1] - pad_y;
    let max_y = bb.max[1] + pad_y;
    let res = resolution.max(8) as usize;
    let dx = (max_x - min_x) / res as f32;
    let dy = (max_y - min_y) / res as f32;

    // Same per-cell inside test + flood fill, but we only need the label of the
    // cell containing `p`. Reuse the seed-cell ORDER (scan order) so labels match
    // `find_regions` exactly.
    let mut inside = vec![false; res * res];
    for iy in 0..res {
        for ix in 0..res {
            let px = min_x + (ix as f32 + 0.5) * dx;
            let py = min_y + (iy as f32 + 0.5) * dy;
            inside[iy * res + ix] = sketch.sdf([px, py]) < 0.0;
        }
    }
    // Locate p's cell.
    if p[0] < min_x || p[0] > max_x || p[1] < min_y || p[1] > max_y {
        return None;
    }
    let cx = (((p[0] - min_x) / dx) as i32).clamp(0, res as i32 - 1) as usize;
    let cy = (((p[1] - min_y) / dy) as i32).clamp(0, res as i32 - 1) as usize;
    let target = cy * res + cx;
    if !inside[target] {
        return None;
    }
    let mut labels = vec![0u32; res * res];
    let mut next_label = 1u32;
    for start in 0..(res * res) {
        if !inside[start] || labels[start] != 0 {
            continue;
        }
        let label = next_label;
        next_label += 1;
        let mut queue = vec![start];
        labels[start] = label;
        while let Some(idx) = queue.pop() {
            let ix = idx % res;
            let iy = idx / res;
            for (nx, ny) in neighbors(ix, iy, res) {
                let ni = ny * res + nx;
                if inside[ni] && labels[ni] == 0 {
                    labels[ni] = label;
                    queue.push(ni);
                }
            }
        }
        if labels[target] != 0 {
            break;
        }
    }
    let lbl = labels[target];
    if lbl == 0 {
        None
    } else {
        Some(lbl - 1)
    }
}

/// Restrict a sketch profile to the connected region(s) containing the given
/// sketch-space points (ticket dhjn). The profile that
/// [`crate::sketch_features::build_sketch2d_from_sub_calls`] produces is a
/// left-folded `Union` of every loop/primitive (holes encoded as
/// `Difference`). We flatten that top-level `Union` chain into its branches,
/// keep each branch whose own SDF is inside (< 0) at ANY of the points, and
/// re-union the survivors. A branch is anything NOT itself a top-level
/// `Union` node — so a ring's `Difference(outer, inner)` stays intact and the
/// returned profile is the EXACT CSG of the picked region, not a faceted
/// polygon. With no surviving branch the result is [`Sketch2d::Empty`].
pub fn restrict_sketch_to_points(full: &Sketch2d, points: &[[f32; 2]]) -> Sketch2d {
    let mut branches: Vec<&Sketch2d> = Vec::new();
    flatten_union(full, &mut branches);
    let kept: Vec<Sketch2d> = branches
        .into_iter()
        .filter(|b| points.iter().any(|&p| b.sdf(p) < 0.0))
        .cloned()
        .collect();
    let mut acc = Sketch2d::Empty;
    for b in kept {
        acc = match acc {
            Sketch2d::Empty => b,
            prev => Sketch2d::Union(Box::new(prev), Box::new(b)),
        };
    }
    acc
}

/// Collect the branches of a top-level `Union` chain. Recurses only through
/// `Union` nodes; every other node (Prim/Difference/Intersection/Offset) is a
/// leaf branch kept whole.
fn flatten_union<'a>(node: &'a Sketch2d, out: &mut Vec<&'a Sketch2d>) {
    match node {
        Sketch2d::Union(a, b) => {
            flatten_union(a, out);
            flatten_union(b, out);
        }
        Sketch2d::Empty => {}
        other => out.push(other),
    }
}

fn neighbors(x: usize, y: usize, res: usize) -> impl Iterator<Item = (usize, usize)> {
    let mut out = Vec::with_capacity(4);
    if x > 0 {
        out.push((x - 1, y));
    }
    if x + 1 < res {
        out.push((x + 1, y));
    }
    if y > 0 {
        out.push((x, y - 1));
    }
    if y + 1 < res {
        out.push((x, y + 1));
    }
    out.into_iter()
}

#[cfg(test)]
mod tests {
    use super::*;
    use fielddraft_protocol::sketch2d::Sketch2dPrimitive;

    #[test]
    fn single_circle_produces_one_region() {
        let sketch = Sketch2d::Prim(Sketch2dPrimitive::Circle {
            center: [0.0, 0.0],
            radius: 10.0,
        });
        let regions = find_regions(&sketch, 64);
        assert_eq!(regions.len(), 1);
        assert!(regions[0].area > 200.0); // π·10² ≈ 314
        assert!(regions[0].centroid[0].abs() < 1.0);
        assert!(regions[0].centroid[1].abs() < 1.0);
    }

    #[test]
    fn two_separate_circles_produce_two_regions() {
        let a = Sketch2d::Prim(Sketch2dPrimitive::Circle {
            center: [-20.0, 0.0],
            radius: 5.0,
        });
        let b = Sketch2d::Prim(Sketch2dPrimitive::Circle {
            center: [20.0, 0.0],
            radius: 5.0,
        });
        let sketch = Sketch2d::Union(Box::new(a), Box::new(b));
        let regions = find_regions(&sketch, 128);
        assert_eq!(regions.len(), 2);
    }

    #[test]
    fn ring_with_hole_produces_one_region() {
        let outer = Sketch2d::Prim(Sketch2dPrimitive::Circle {
            center: [0.0, 0.0],
            radius: 20.0,
        });
        let inner = Sketch2d::Prim(Sketch2dPrimitive::Circle {
            center: [0.0, 0.0],
            radius: 10.0,
        });
        let ring = Sketch2d::Difference(Box::new(outer), Box::new(inner));
        let regions = find_regions(&ring, 128);
        // The ring is one connected region (the annular area).
        assert_eq!(regions.len(), 1);
    }

    #[test]
    fn empty_sketch_produces_no_regions() {
        let regions = find_regions(&Sketch2d::Empty, 32);
        assert!(regions.is_empty());
    }

    #[test]
    fn region_interior_point_is_inside() {
        // A ring's centroid sits in the hole; the interior point must not.
        let outer = Sketch2d::Prim(Sketch2dPrimitive::Circle {
            center: [0.0, 0.0],
            radius: 20.0,
        });
        let inner = Sketch2d::Prim(Sketch2dPrimitive::Circle {
            center: [0.0, 0.0],
            radius: 10.0,
        });
        let ring = Sketch2d::Difference(Box::new(outer), Box::new(inner));
        let regions = find_regions(&ring, 128);
        assert_eq!(regions.len(), 1);
        assert!(ring.sdf(regions[0].interior) < 0.0, "interior is inside");
        assert!(ring.sdf(regions[0].centroid) > 0.0, "centroid in the hole");
    }

    #[test]
    fn restrict_keeps_only_the_picked_region() {
        // Two disjoint circles → restricting to either one's interior keeps
        // only that circle's branch.
        let a = Sketch2d::Prim(Sketch2dPrimitive::Circle {
            center: [-20.0, 0.0],
            radius: 5.0,
        });
        let b = Sketch2d::Prim(Sketch2dPrimitive::Circle {
            center: [20.0, 0.0],
            radius: 5.0,
        });
        let sketch = Sketch2d::Union(Box::new(a), Box::new(b));
        let regions = find_regions(&sketch, 128);
        assert_eq!(regions.len(), 2);
        // Pick whichever region sits on the left (centre x < 0).
        let left = regions.iter().find(|r| r.centroid[0] < 0.0).unwrap();
        let restricted = restrict_sketch_to_points(&sketch, &[left.interior]);
        // Inside the left circle, outside the right one.
        assert!(restricted.sdf([-20.0, 0.0]) < 0.0);
        assert!(restricted.sdf([20.0, 0.0]) > 0.0);
        // The full sketch was inside at BOTH — proving we dropped a branch.
        assert!(sketch.sdf([20.0, 0.0]) < 0.0);
    }

    #[test]
    fn restrict_to_ring_preserves_the_hole() {
        // A ring next to a solid disc: restricting to the ring keeps its hole.
        let outer = Sketch2d::Prim(Sketch2dPrimitive::Circle {
            center: [0.0, 0.0],
            radius: 20.0,
        });
        let inner = Sketch2d::Prim(Sketch2dPrimitive::Circle {
            center: [0.0, 0.0],
            radius: 10.0,
        });
        let ring = Sketch2d::Difference(Box::new(outer), Box::new(inner));
        let disc = Sketch2d::Prim(Sketch2dPrimitive::Circle {
            center: [60.0, 0.0],
            radius: 8.0,
        });
        let sketch = Sketch2d::Union(Box::new(ring.clone()), Box::new(disc));
        let regions = find_regions(&sketch, 160);
        let ring_region = regions
            .iter()
            .find(|r| r.aabb.min[0] < 0.0)
            .expect("ring region");
        let restricted = restrict_sketch_to_points(&sketch, &[ring_region.interior]);
        assert!(restricted.sdf([15.0, 0.0]) < 0.0, "in the annulus");
        assert!(restricted.sdf([0.0, 0.0]) > 0.0, "hole preserved");
        assert!(restricted.sdf([60.0, 0.0]) > 0.0, "disc dropped");
    }
}
