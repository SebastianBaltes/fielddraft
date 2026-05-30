//! Free-space sampling via sphere rasterization grid.
//!
//! Ref: Paper Section 4.1 (Figure 8, 10), `outside_points_from_rasterization.cpp`
//!
//! Given a set of spheres (one sign class), produces points that lie
//! *outside* all spheres — the "free space" near sphere boundaries.

use super::sphere_tree::{SdfSphere, SphereAabbTree};

/// Compute outside points via sphere rasterization in [0,1]^3.
///
/// `spheres` must already be normalized to [0,1]^3 and be of one sign class;
/// `tree` is the AABB tree already built over exactly those spheres.
/// Returns positions of grid cells in the narrow band that are not occupied
/// by any sphere.
pub fn outside_points(spheres: &[SdfSphere], tree: &SphereAabbTree) -> Vec<[f32; 3]> {
    let n = spheres.len();
    if n == 0 {
        return vec![];
    }

    // Resolution: ceil(64 * (n/16)^(1/3))
    // Reference: reach_for_the_arcs.py — int(np.ceil(64*(len(U)/16)**(1/3)))
    // Capped at 128: cells scale with r³, and at the sphere counts the
    // browser feeds in (res 32 → ~30k spheres, res 64 → ~270k) the
    // reference formula yields r≈800. The raster only *seeds* outside
    // points that the fine-tuning loop then refines, so a coarser grid
    // trades initial seed density for tractability, not final fidelity.
    let r = ((64.0 * (n as f64 / 16.0).cbrt()).ceil() as usize).clamp(64, 128);

    let cell_size = 1.0 / r as f32;
    let raster_shrink = 0.95_f32;

    let total_cells = r * r * r;
    let mut occupied = vec![false; total_cells];

    // Mark occupancy per cell via the AABB tree instead of per sphere.
    // The per-sphere fill costs Σ(bbox volume); large interior spheres
    // (radius ~0.5 normalized) each cover a huge fraction of the grid,
    // so at browser sphere counts that sum explodes into billions of
    // overwrites and the worker hangs. Querying per cell is bounded by
    // the grid size, and interior cells early-exit on the first sphere
    // that contains them — same occupancy result, terminating cost.
    for k in 0..r {
        for j in 0..r {
            for i in 0..r {
                let cc = cell_center(i, j, k, cell_size);
                if tree.point_inside_any_shrunk(spheres, cc, raster_shrink) {
                    occupied[i + j * r + k * r * r] = true;
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
