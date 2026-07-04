//! Dense Surface Nets meshing for the export path — a robust generic
//! mesher for fields that are *not* 1-Lipschitz.
//!
//! Unlike the adaptive Manifold-DC route (which prunes cells with a
//! Lipschitz distance bound on a narrow-band octree), Surface Nets
//! evaluates the SDF at **every** corner of a uniform grid and places
//! one dual vertex per sign-changing cell. Nothing is skipped, so the
//! topology of a field the octree's bound could misjudge — gyroid /
//! TPMS shells, dense helix windings — is captured reliably. The mesh
//! is watertight by construction (uniform grid, no T-junctions); it is
//! deliberately heavy and the export pass hands it straight to
//! [`crate::decimate`] to bring the triangle count back down.
//!
//! The meshing itself is the battle-tested `fast-surface-nets` crate
//! (bonsairobo) — correct quad winding and central-difference normals
//! out of the box. We own only the SDF sampling, the world-space
//! transform, and an optional Newton projection of the centroid
//! vertices onto the exact isosurface (tightening the deviation before
//! decimation measures it).

use crate::bvh::BoundedCsg;
use crate::csg::EvalMode;
use crate::dc::{Mesh, DC_MARGIN_CELLS};
use fast_surface_nets::{surface_nets as fsn, SurfaceNetsBuffer};
use ndshape::{RuntimeShape, Shape};

/// Mesh a body with dense uniform Surface Nets at `resolution` cells
/// along the longest AABB axis. Returns an empty mesh for an empty
/// scene. Positions carry per-vertex SDF-gradient normals; the viewport
/// sidecar channels stay empty (the exporters don't read them).
pub fn surface_nets(node: &BoundedCsg, resolution: u32) -> Mesh {
    if resolution == 0 {
        return Mesh::default();
    }
    let raw = node.aabb();
    if raw.is_empty() {
        return Mesh::default();
    }
    let size = raw.size();
    let max_dim = size[0].max(size[1]).max(size[2]);
    if max_dim <= 0.0 {
        return Mesh::default();
    }
    let cell = max_dim / resolution as f32;
    let grid = raw.expanded(cell * DC_MARGIN_CELLS);
    let gmin = grid.min;
    let gsize = grid.size();

    // Sample dimensions: one more corner than cells per axis. The
    // DC_MARGIN padding keeps the surface ≥2 cells off the boundary, so
    // the positive-boundary faces fast-surface-nets omits carry nothing.
    let dx = (gsize[0] / cell).ceil().max(1.0) as u32 + 1;
    let dy = (gsize[1] / cell).ceil().max(1.0) as u32 + 1;
    let dz = (gsize[2] / cell).ceil().max(1.0) as u32 + 1;
    let shape = RuntimeShape::<u32, 3>::new([dx, dy, dz]);

    // Sample the field at every corner into the shape's linear layout.
    let mut sdf = vec![0.0f32; shape.size() as usize];
    for z in 0..dz {
        for y in 0..dy {
            for x in 0..dx {
                let p = [
                    gmin[0] + x as f32 * cell,
                    gmin[1] + y as f32 * cell,
                    gmin[2] + z as f32 * cell,
                ];
                sdf[shape.linearize([x, y, z]) as usize] =
                    node.sdf(p, EvalMode::Visual);
            }
        }
    }

    let mut buffer = SurfaceNetsBuffer::default();
    fsn(
        &sdf,
        &shape,
        [0, 0, 0],
        [dx - 1, dy - 1, dz - 1],
        &mut buffer,
    );
    if buffer.positions.is_empty() || buffer.indices.is_empty() {
        return Mesh::default();
    }

    // Map grid-index positions to world, Newton-project onto the exact
    // isosurface, and take the SDF gradient as the vertex normal.
    let vcount = buffer.positions.len();
    let mut positions = vec![0.0f32; vcount * 3];
    let mut normals = vec![0.0f32; vcount * 3];
    for (i, gp) in buffer.positions.iter().enumerate() {
        let mut p = [
            gmin[0] + gp[0] * cell,
            gmin[1] + gp[1] * cell,
            gmin[2] + gp[2] * cell,
        ];
        project_to_surface(node, &mut p, cell);
        let n = crate::dc::sdf_normal(node, p, cell * 0.25);
        positions[i * 3] = p[0];
        positions[i * 3 + 1] = p[1];
        positions[i * 3 + 2] = p[2];
        normals[i * 3] = n[0];
        normals[i * 3 + 1] = n[1];
        normals[i * 3 + 2] = n[2];
    }

    Mesh {
        positions,
        normals,
        indices: buffer.indices,
        ..Mesh::default()
    }
}

/// A few Newton steps moving `p` onto the SDF zero-set along the
/// gradient, clamped so a bad step can't fling the vertex out of its
/// cell (keeps the dense mesh well-formed for decimation).
fn project_to_surface(node: &BoundedCsg, p: &mut [f32; 3], cell: f32) {
    const ITERS: usize = 3;
    let max_move = cell * 0.75;
    for _ in 0..ITERS {
        let d = node.sdf(*p, EvalMode::Visual);
        if d.abs() < 1e-6 {
            break;
        }
        let g = crate::dc::sdf_normal(node, *p, cell * 0.25);
        let step = d.clamp(-max_move, max_move);
        p[0] -= step * g[0];
        p[1] -= step * g[1];
        p[2] -= step * g[2];
    }
}
