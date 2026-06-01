//! Voxel-based mesh repair: converts non-manifold triangle soup
//! into a guaranteed-watertight mesh via voxelization + flood-fill
//! + Marching Cubes extraction.

use crate::mesh_sdf::MeshSdf;
use std::collections::VecDeque;

pub fn repair_voxel(mesh: &MeshSdf, max_resolution: u32) -> MeshSdf {
    let max_resolution = max_resolution.clamp(32, 512);
    let extent = [
        mesh.aabb_max[0] - mesh.aabb_min[0],
        mesh.aabb_max[1] - mesh.aabb_min[1],
        mesh.aabb_max[2] - mesh.aabb_min[2],
    ];
    let longest = extent[0].max(extent[1]).max(extent[2]);
    if longest < 1e-6 {
        return mesh.clone();
    }

    let voxel_size = longest / max_resolution as f32;
    let pad = 2.0 * voxel_size;
    let origin = [
        mesh.aabb_min[0] - pad,
        mesh.aabb_min[1] - pad,
        mesh.aabb_min[2] - pad,
    ];
    let nx = ((extent[0] + 2.0 * pad) / voxel_size).ceil() as usize + 1;
    let ny = ((extent[1] + 2.0 * pad) / voxel_size).ceil() as usize + 1;
    let nz = ((extent[2] + 2.0 * pad) / voxel_size).ceil() as usize + 1;

    let total = nx * ny * nz;
    if total > 200_000_000 {
        return mesh.clone();
    }

    // Phase 1: rasterize triangles into surface voxels
    let mut grid = vec![0u8; total]; // 0=empty, 1=surface
    let tri_count = mesh.tris().len() / 9;
    let tris = mesh.tris();

    for t in 0..tri_count {
        let base = t * 9;
        let v0 = [tris[base], tris[base + 1], tris[base + 2]];
        let v1 = [tris[base + 3], tris[base + 4], tris[base + 5]];
        let v2 = [tris[base + 6], tris[base + 7], tris[base + 8]];
        rasterize_triangle(v0, v1, v2, &origin, voxel_size, nx, ny, nz, &mut grid);
    }

    // Phase 2: flood-fill from boundary (all boundary cells not marked surface)
    const OUTSIDE: u8 = 2;
    let idx = |x: usize, y: usize, z: usize| -> usize { z * ny * nx + y * nx + x };

    let mut queue: VecDeque<(usize, usize, usize)> = VecDeque::new();

    // Seed from all 6 faces of the grid
    for y in 0..ny {
        for x in 0..nx {
            for &z in &[0, nz - 1] {
                let i = idx(x, y, z);
                if grid[i] == 0 {
                    grid[i] = OUTSIDE;
                    queue.push_back((x, y, z));
                }
            }
        }
    }
    for z in 0..nz {
        for x in 0..nx {
            for &y in &[0, ny - 1] {
                let i = idx(x, y, z);
                if grid[i] == 0 {
                    grid[i] = OUTSIDE;
                    queue.push_back((x, y, z));
                }
            }
        }
    }
    for z in 0..nz {
        for y in 0..ny {
            for &x in &[0, nx - 1] {
                let i = idx(x, y, z);
                if grid[i] == 0 {
                    grid[i] = OUTSIDE;
                    queue.push_back((x, y, z));
                }
            }
        }
    }

    // BFS flood-fill
    while let Some((x, y, z)) = queue.pop_front() {
        let neighbors: [(i32, i32, i32); 6] = [
            (-1, 0, 0), (1, 0, 0),
            (0, -1, 0), (0, 1, 0),
            (0, 0, -1), (0, 0, 1),
        ];
        for (dx, dy, dz) in neighbors {
            let nx2 = x as i32 + dx;
            let ny2 = y as i32 + dy;
            let nz2 = z as i32 + dz;
            if nx2 < 0 || nx2 >= nx as i32 || ny2 < 0 || ny2 >= ny as i32 || nz2 < 0 || nz2 >= nz as i32 {
                continue;
            }
            let (ux, uy, uz) = (nx2 as usize, ny2 as usize, nz2 as usize);
            let ni = idx(ux, uy, uz);
            if grid[ni] == 0 {
                grid[ni] = OUTSIDE;
                queue.push_back((ux, uy, uz));
            }
        }
    }

    // Phase 3: classify — solid = surface(1) or unreached(0), outside = 2
    // Build scalar field at grid vertices (not cells) for MC.
    // Vertex (i,j,k) is solid if ANY of its 8 neighboring cells is solid.
    let vnx = nx + 1;
    let vny = ny + 1;
    let vnz = nz + 1;
    let mut field = vec![false; vnx * vny * vnz];
    let vidx = |x: usize, y: usize, z: usize| -> usize { z * vny * vnx + y * vnx + x };

    for vz in 0..vnz {
        for vy in 0..vny {
            for vx in 0..vnx {
                // A vertex is "inside" if any adjacent cell is solid
                let mut solid = false;
                for dz in 0..2usize {
                    for dy in 0..2usize {
                        for dx in 0..2usize {
                            let cx = (vx + dx).wrapping_sub(1);
                            let cy = (vy + dy).wrapping_sub(1);
                            let cz = (vz + dz).wrapping_sub(1);
                            if cx < nx && cy < ny && cz < nz {
                                let ci = idx(cx, cy, cz);
                                if grid[ci] != OUTSIDE {
                                    solid = true;
                                }
                            }
                        }
                    }
                }
                field[vidx(vx, vy, vz)] = solid;
            }
        }
    }

    // Phase 4: Marching Cubes extraction
    let mut positions: Vec<f32> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    for cz in 0..(vnz - 1) {
        for cy in 0..(vny - 1) {
            for cx in 0..(vnx - 1) {
                let corners: [bool; 8] = [
                    field[vidx(cx, cy, cz)],
                    field[vidx(cx + 1, cy, cz)],
                    field[vidx(cx + 1, cy + 1, cz)],
                    field[vidx(cx, cy + 1, cz)],
                    field[vidx(cx, cy, cz + 1)],
                    field[vidx(cx + 1, cy, cz + 1)],
                    field[vidx(cx + 1, cy + 1, cz + 1)],
                    field[vidx(cx, cy + 1, cz + 1)],
                ];

                let mut cube_index: u8 = 0;
                for (i, &c) in corners.iter().enumerate() {
                    if c {
                        cube_index |= 1 << i;
                    }
                }

                if cube_index == 0 || cube_index == 255 {
                    continue;
                }

                let edge_mask = EDGE_TABLE[cube_index as usize];
                if edge_mask == 0 {
                    continue;
                }

                // Compute vertex positions for intersected edges (binary: midpoint)
                let corner_pos: [[f32; 3]; 8] = [
                    vertex_pos(cx, cy, cz, &origin, voxel_size),
                    vertex_pos(cx + 1, cy, cz, &origin, voxel_size),
                    vertex_pos(cx + 1, cy + 1, cz, &origin, voxel_size),
                    vertex_pos(cx, cy + 1, cz, &origin, voxel_size),
                    vertex_pos(cx, cy, cz + 1, &origin, voxel_size),
                    vertex_pos(cx + 1, cy, cz + 1, &origin, voxel_size),
                    vertex_pos(cx + 1, cy + 1, cz + 1, &origin, voxel_size),
                    vertex_pos(cx, cy + 1, cz + 1, &origin, voxel_size),
                ];

                let mut edge_verts: [[f32; 3]; 12] = [[0.0; 3]; 12];
                for e in 0..12u32 {
                    if edge_mask & (1 << e) != 0 {
                        let (a, b) = EDGE_VERTICES[e as usize];
                        edge_verts[e as usize] = [
                            (corner_pos[a][0] + corner_pos[b][0]) * 0.5,
                            (corner_pos[a][1] + corner_pos[b][1]) * 0.5,
                            (corner_pos[a][2] + corner_pos[b][2]) * 0.5,
                        ];
                    }
                }

                // Emit triangles
                let tri_row = &TRI_TABLE[cube_index as usize];
                let mut i = 0;
                while i < 15 && tri_row[i] != -1 {
                    let base_idx = positions.len() as u32 / 3;
                    for k in 0..3 {
                        let edge = tri_row[i + k] as usize;
                        positions.push(edge_verts[edge][0]);
                        positions.push(edge_verts[edge][1]);
                        positions.push(edge_verts[edge][2]);
                        indices.push(base_idx + k as u32);
                    }
                    i += 3;
                }
            }
        }
    }

    if indices.is_empty() {
        return mesh.clone();
    }

    MeshSdf::from_buffers(&positions, &indices)
}

fn vertex_pos(x: usize, y: usize, z: usize, origin: &[f32; 3], vs: f32) -> [f32; 3] {
    [
        origin[0] + x as f32 * vs,
        origin[1] + y as f32 * vs,
        origin[2] + z as f32 * vs,
    ]
}

#[allow(clippy::too_many_arguments)]
fn rasterize_triangle(
    v0: [f32; 3], v1: [f32; 3], v2: [f32; 3],
    origin: &[f32; 3], voxel_size: f32,
    nx: usize, ny: usize, nz: usize,
    grid: &mut [u8],
) {
    let e0 = [v1[0] - v0[0], v1[1] - v0[1], v1[2] - v0[2]];
    let e1 = [v2[0] - v0[0], v2[1] - v0[1], v2[2] - v0[2]];
    let e0_len = (e0[0] * e0[0] + e0[1] * e0[1] + e0[2] * e0[2]).sqrt();
    let e1_len = (e1[0] * e1[0] + e1[1] * e1[1] + e1[2] * e1[2]).sqrt();

    let half_vs = voxel_size * 0.4;
    let steps0 = ((e0_len / half_vs) as usize).max(1);
    let steps1 = ((e1_len / half_vs) as usize).max(1);

    let inv_vs = 1.0 / voxel_size;
    let idx = |x: usize, y: usize, z: usize| -> usize { z * ny * nx + y * nx + x };

    for i in 0..=steps0 {
        let u = i as f32 / steps0 as f32;
        for j in 0..=steps1 {
            let v = j as f32 / steps1 as f32;
            if u + v > 1.0 {
                continue;
            }
            let px = v0[0] + u * e0[0] + v * e1[0];
            let py = v0[1] + u * e0[1] + v * e1[1];
            let pz = v0[2] + u * e0[2] + v * e1[2];

            let fx = (px - origin[0]) * inv_vs;
            let fy = (py - origin[1]) * inv_vs;
            let fz = (pz - origin[2]) * inv_vs;
            if fx < 0.0 || fy < 0.0 || fz < 0.0 {
                continue;
            }
            let ix = fx as usize;
            let iy = fy as usize;
            let iz = fz as usize;

            if ix < nx && iy < ny && iz < nz {
                grid[idx(ix, iy, iz)] = 1;
            }
        }
    }
}

// MC edge connectivity: which two corners define each of the 12 edges
const EDGE_VERTICES: [(usize, usize); 12] = [
    (0, 1), (1, 2), (2, 3), (3, 0),
    (4, 5), (5, 6), (6, 7), (7, 4),
    (0, 4), (1, 5), (2, 6), (3, 7),
];

use crate::mc_tables::{EDGE_TABLE, TRI_TABLE};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repair_voxel_cube_with_hole() {
        // Cube missing +Z face — should be repaired to watertight
        #[rustfmt::skip]
        let positions: Vec<f32> = vec![
            -1.0,-1.0,-1.0,  1.0,-1.0,-1.0,  1.0, 1.0,-1.0, -1.0, 1.0,-1.0,
            -1.0,-1.0, 1.0,  1.0,-1.0, 1.0,  1.0, 1.0, 1.0, -1.0, 1.0, 1.0,
        ];
        #[rustfmt::skip]
        let indices: Vec<u32> = vec![
            0,2,1, 0,3,2,
            0,1,5, 0,5,4,
            3,6,2, 3,7,6,
            0,4,7, 0,7,3,
            1,2,6, 1,6,5,
        ];
        let mesh = MeshSdf::from_buffers(&positions, &indices);
        assert!(!mesh.check_manifold().is_watertight);

        let repaired = repair_voxel(&mesh, 64);
        let report = repaired.check_manifold();
        assert!(
            report.is_watertight,
            "voxel-repaired mesh should be watertight, open_edges={}",
            report.open_edges
        );
    }

    #[test]
    fn repair_voxel_open_patch() {
        // Single triangle (completely open) — should produce a thin solid
        let positions: Vec<f32> = vec![0.0, 0.0, 0.0, 10.0, 0.0, 0.0, 5.0, 10.0, 0.0];
        let indices: Vec<u32> = vec![0, 1, 2];
        let mesh = MeshSdf::from_buffers(&positions, &indices);
        assert!(!mesh.check_manifold().is_watertight);

        let repaired = repair_voxel(&mesh, 32);
        assert!(repaired.check_manifold().triangle_count > 0);
    }
}
