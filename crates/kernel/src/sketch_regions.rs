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
    /// Centroid of the region in sketch-plane coordinates.
    pub centroid: [f32; 2],
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

    for start in 0..(res * res) {
        if !inside[start] || labels[start] != 0 {
            continue;
        }
        // BFS flood fill.
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
            SketchRegion {
                index: i as u32,
                centroid: [(sum_x[i] / c) as f32, (sum_y[i] / c) as f32],
                aabb: Aabb2 {
                    min: [r_min_x[i], r_min_y[i]],
                    max: [r_max_x[i], r_max_y[i]],
                },
                area: count[i] as f32 * cell_area,
            }
        })
        .collect()
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
}
