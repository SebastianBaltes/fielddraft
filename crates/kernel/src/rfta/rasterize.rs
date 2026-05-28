//! Free-space sampling via sphere rasterization grid.
//!
//! Ref: Paper Section 4.1 (Figure 8, 10), `outside_points_from_rasterization.cpp`
//!
//! Given a set of spheres (one sign class), produces points that lie
//! *outside* all spheres — the "free space" near sphere boundaries.

use super::sphere_tree::SdfSphere;

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

    // Resolution: ceil(64 * (n/16)^(1/3))
    // Reference: reach_for_the_arcs.py — int(np.ceil(64*(len(U)/16)**(1/3)))
    // Capped at 256 to keep memory feasible in WASM (~34 MB at 256^3).
    let r = ((64.0 * (n as f64 / 16.0).cbrt()).ceil() as usize).clamp(64, 256);

    let cell_size = 1.0 / r as f32;
    let raster_shrink = 0.95_f32;

    let total_cells = r * r * r;
    let mut occupied = vec![false; total_cells];

    // Mark cells occupied by each sphere (with raster_shrink)
    for sphere in spheres {
        let shrunk_r = raster_shrink * sphere.radius;
        let shrunk_r_sq = shrunk_r * shrunk_r;

        let imin = ((sphere.center[0] - shrunk_r) / cell_size).floor() as i32;
        let imax = ((sphere.center[0] + shrunk_r) / cell_size).ceil() as i32;
        let jmin = ((sphere.center[1] - shrunk_r) / cell_size).floor() as i32;
        let jmax = ((sphere.center[1] + shrunk_r) / cell_size).ceil() as i32;
        let kmin = ((sphere.center[2] - shrunk_r) / cell_size).floor() as i32;
        let kmax = ((sphere.center[2] + shrunk_r) / cell_size).ceil() as i32;

        let ri = r as i32;
        for k in kmin.max(0)..kmax.min(ri) {
            for j in jmin.max(0)..jmax.min(ri) {
                for i in imin.max(0)..imax.min(ri) {
                    let idx = i as usize + j as usize * r + k as usize * r * r;
                    let cc = cell_center(i as usize, j as usize, k as usize, cell_size);
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

    // Narrow-band filter: keep unoccupied cells that have at least one
    // occupied neighbor in their 3×3×3 neighborhood.
    let mut near_boundary = vec![false; total_cells];
    let ri = r as i32;
    for k in 0..r {
        for j in 0..r {
            for i in 0..r {
                let idx = i + j * r + k * r * r;
                if occupied[idx] {
                    continue;
                }
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
    for k in 0..r {
        for j in 0..r {
            for i in 0..r {
                let idx = i + j * r + k * r * r;
                if !occupied[idx] && near_boundary[idx] {
                    result.push(cell_center(i, j, k, cell_size));
                }
            }
        }
    }

    result
}

/// Jittered cell center using a spatial hash (no storage needed).
fn cell_center(i: usize, j: usize, k: usize, cell_size: f32) -> [f32; 3] {
    let base = (i as u32)
        .wrapping_mul(73856093)
        ^ (j as u32).wrapping_mul(19349663)
        ^ (k as u32).wrapping_mul(83492791);
    let jx = hash_to_f32(base);
    let jy = hash_to_f32(base.wrapping_add(1));
    let jz = hash_to_f32(base.wrapping_add(2));
    [
        (i as f32 + jx) * cell_size,
        (j as f32 + jy) * cell_size,
        (k as f32 + jz) * cell_size,
    ]
}

/// Deterministic hash → [0, 1).
fn hash_to_f32(seed: u32) -> f32 {
    let h = seed.wrapping_mul(2654435761);
    (h >> 8) as f32 / 16777216.0
}
