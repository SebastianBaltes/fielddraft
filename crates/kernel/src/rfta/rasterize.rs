//! Free-space sampling via sphere rasterization grid.
//!
//! Ref: Paper Section 4.1 (Figure 8, 10), `outside_points_from_rasterization.cpp`
//!
//! Given a set of spheres (one sign class), produces points that lie
//! *outside* all spheres — the "free space" near sphere boundaries.

use super::sphere_tree::SdfSphere;

/// Simple LCG pseudo-random number generator for deterministic jitter.
struct Lcg {
    state: u32,
}

impl Lcg {
    fn new(seed: u32) -> Self {
        Self { state: seed }
    }

    /// Returns a value in [0, 1).
    fn next_f32(&mut self) -> f32 {
        // LCG parameters from Numerical Recipes
        self.state = self.state.wrapping_mul(1664525).wrapping_add(1013904223);
        (self.state >> 8) as f32 / 16777216.0
    }
}

/// Compute outside points via sphere rasterization in [0,1]^3.
///
/// `spheres` must already be normalized to [0,1]^3 and be of one sign class.
/// Returns positions of grid cells in the narrow band that are not occupied
/// by any sphere.
pub fn outside_points(spheres: &[SdfSphere]) -> Vec<[f32; 3]> {
    let n = spheres.len();
    if n == 0 {
        return vec![];
    }

    // Resolution: r = 64 * ceil(n^(1/3) / 16)
    // Paper Section 4.1, reference: reach_for_the_arcs.py
    let r = (64.0 * ((n as f64).cbrt() / 16.0).ceil()) as usize;
    let r = r.max(64);

    let cell_size = 1.0 / r as f32;
    let raster_shrink = 0.95_f32;

    // Initialize grid: false = unoccupied, true = occupied
    let total_cells = r * r * r;
    let mut occupied = vec![false; total_cells];

    // Generate jittered cell centers
    let mut rng = Lcg::new(3452);
    let mut cell_centers = Vec::with_capacity(total_cells);
    for k in 0..r {
        for j in 0..r {
            for i in 0..r {
                let jx = rng.next_f32();
                let jy = rng.next_f32();
                let jz = rng.next_f32();
                cell_centers.push([
                    (i as f32 + jx) * cell_size,
                    (j as f32 + jy) * cell_size,
                    (k as f32 + jz) * cell_size,
                ]);
            }
        }
    }

    // Mark cells occupied by each sphere (with raster_shrink)
    for sphere in spheres {
        let shrunk_r = raster_shrink * sphere.radius;
        let shrunk_r_sq = shrunk_r * shrunk_r;

        // Bounding box in grid coordinates
        let imin = ((sphere.center[0] - shrunk_r) / cell_size).floor() as i32;
        let imax = ((sphere.center[0] + shrunk_r) / cell_size).ceil() as i32;
        let jmin = ((sphere.center[1] - shrunk_r) / cell_size).floor() as i32;
        let jmax = ((sphere.center[1] + shrunk_r) / cell_size).ceil() as i32;
        let kmin = ((sphere.center[2] - shrunk_r) / cell_size).floor() as i32;
        let kmax = ((sphere.center[2] + shrunk_r) / cell_size).ceil() as i32;

        let ri = r as i32;
        for k in kmin.max(0)..imax_clamp(kmax, ri) {
            for j in jmin.max(0)..imax_clamp(jmax, ri) {
                for i in imin.max(0)..imax_clamp(imax, ri) {
                    let idx = i as usize + j as usize * r + k as usize * r * r;
                    let cc = cell_centers[idx];
                    let dx = cc[0] - sphere.center[0];
                    let dy = cc[1] - sphere.center[1];
                    let dz = cc[2] - sphere.center[2];
                    if dx * dx + dy * dy + dz * dz < shrunk_r_sq {
                        occupied[idx] = true;
                    }
                }
            }
        }
    }

    // Narrow-band filter: remove unoccupied cells whose entire
    // 3x3x3 neighborhood is also unoccupied (too far from surface).
    let mut near_boundary = vec![false; total_cells];
    let ri = r as i32;
    for k in 0..r {
        for j in 0..r {
            for i in 0..r {
                let idx = i + j * r + k * r * r;
                if occupied[idx] {
                    continue;
                }
                // Check if any neighbor in 3x3x3 is occupied
                let mut has_occupied_neighbor = false;
                'outer: for dk in -1i32..=1 {
                    for dj in -1i32..=1 {
                        for di in -1i32..=1 {
                            let ni = i as i32 + di;
                            let nj = j as i32 + dj;
                            let nk = k as i32 + dk;
                            if ni < 0 || ni >= ri || nj < 0 || nj >= ri || nk < 0 || nk >= ri {
                                continue;
                            }
                            let nidx = ni as usize + nj as usize * r + nk as usize * r * r;
                            if occupied[nidx] {
                                has_occupied_neighbor = true;
                                break 'outer;
                            }
                        }
                    }
                }
                if has_occupied_neighbor {
                    near_boundary[idx] = true;
                }
            }
        }
    }

    // Collect unoccupied cells in the narrow band
    let mut result = Vec::new();
    for idx in 0..total_cells {
        if !occupied[idx] && near_boundary[idx] {
            result.push(cell_centers[idx]);
        }
    }

    result
}

/// Clamp max index to grid resolution.
fn imax_clamp(val: i32, r: i32) -> i32 {
    val.min(r)
}
