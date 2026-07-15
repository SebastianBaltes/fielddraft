//! Voxel-Debug-API (#369): sample the scene solid on a regular 3D grid.
//!
//! The whole scene is voxelised on a cube grid whose resolution counts voxels
//! along the LONGEST scene-AABB axis (the other axes get proportionally many,
//! so voxels stay cubic). The occupancy is `1` where the voxel centre is inside
//! the solid (scene SDF < 0), `0` outside. Sampling is done in the kernel — one
//! WASM call for the whole grid — because per-voxel round-trips across the WASM
//! boundary would be far too slow (res=64 → 262 k crossings).
//!
//! Two calls share the SAME deterministic grid geometry so the frontend can
//! align occupancy arrays across snapshots:
//!   · `voxelGridInfo(res)`  → `[nx, ny, nz, ox, oy, oz, spacing]` (cheap, AABB
//!     math only, no sampling);
//!   · `voxelizeOccupancy(res)` → `nx·ny·nz` bytes, row-major with x fastest,
//!     then y, then z (the expensive SDF sampling).
//!
//! The scene field mirrors the Debug ▸ SDF-plane / mesher sampler: the union
//! over every body of its exact mesher SDF (`EvalMode::Visual`), so edge
//! fillets and booleans are reflected exactly — which is the point of the
//! feature (objectively comparing geometry with/without a fillet, #373).

use wasm_bindgen::prelude::*;

use crate::Kernel;

/// Hard cap on the total voxel count, independent of the requested resolution:
/// a bound against a runaway grid eating memory or hanging the main thread with
/// millions of synchronous SDF evaluations. ~200³.
const VOXEL_CAP: usize = 8_000_000;

#[wasm_bindgen]
impl Kernel {
    /// Deterministic grid geometry for `resolution` voxels along the longest
    /// scene-AABB axis: `(dims, origin, spacing)`. `origin` is the min corner of
    /// voxel `(0,0,0)`; voxel `(ix,iy,iz)`'s centre is
    /// `origin + (idx + 0.5)·spacing`. A one-voxel empty border is added on every
    /// side so surface voxels are never clipped by the tight AABB. Returns
    /// `None` for an empty scene. Cheap — only unions the stored per-body AABBs.
    fn voxel_grid(&self, resolution: u32) -> Option<([usize; 3], [f32; 3], f32)> {
        let mut bb = crate::csg::Aabb::EMPTY;
        for b in &self.bodies {
            bb = crate::csg::Aabb::union(bb, b.aabb);
        }
        if bb.is_empty() {
            return None;
        }
        let size = bb.size();
        let longest = size[0].max(size[1]).max(size[2]);
        if !(longest > 0.0) {
            return None;
        }
        let mut res = resolution.clamp(2, 256) as usize;
        loop {
            let spacing = longest / res as f32;
            let dims = [
                (size[0] / spacing).ceil() as usize + 2,
                (size[1] / spacing).ceil() as usize + 2,
                (size[2] / spacing).ceil() as usize + 2,
            ];
            let total = dims[0] * dims[1] * dims[2];
            if total <= VOXEL_CAP || res <= 2 {
                let origin = [
                    bb.min[0] - spacing,
                    bb.min[1] - spacing,
                    bb.min[2] - spacing,
                ];
                return Some((dims, origin, spacing));
            }
            // Shrink the resolution until the grid fits the cap.
            res = (res * 3 / 4).max(2);
        }
    }

    /// Grid geometry for the requested resolution as
    /// `[nx, ny, nz, ox, oy, oz, spacing]` (all `f32`), or `[]` for an empty
    /// scene. Read-only and cheap — the dialog can call it live.
    #[wasm_bindgen(js_name = voxelGridInfo)]
    pub fn voxel_grid_info(&self, resolution: u32) -> Vec<f32> {
        match self.voxel_grid(resolution) {
            Some((d, o, s)) => vec![d[0] as f32, d[1] as f32, d[2] as f32, o[0], o[1], o[2], s],
            None => Vec::new(),
        }
    }

    /// Occupancy of the voxel grid: `nx·ny·nz` bytes, `1` where the voxel centre
    /// is inside the solid (scene SDF < 0), `0` outside. Row-major, x fastest,
    /// then y, then z — the linear index of `(ix,iy,iz)` is
    /// `ix + iy·nx + iz·nx·ny`. Empty scene → `[]`. Pairs with
    /// [`Kernel::voxel_grid_info`] for the same resolution (identical grid).
    #[wasm_bindgen(js_name = voxelizeOccupancy)]
    pub fn voxelize_occupancy(&self, resolution: u32) -> Vec<u8> {
        let Some((dims, origin, spacing)) = self.voxel_grid(resolution) else {
            return Vec::new();
        };
        // Build each body's bounded CSG once; the scene field is their union —
        // the same field the meshers / SDF-plane debug see (EvalMode::Visual).
        let bodies: Vec<crate::bvh::BoundedCsg> = self
            .bodies
            .iter()
            .map(|b| crate::body::build_bounded_with_primitives(&self.decoded, &b.feature_indices).0)
            .collect();
        let mut occ = vec![0u8; dims[0] * dims[1] * dims[2]];
        let mut idx = 0usize;
        for iz in 0..dims[2] {
            let z = origin[2] + (iz as f32 + 0.5) * spacing;
            for iy in 0..dims[1] {
                let y = origin[1] + (iy as f32 + 0.5) * spacing;
                for ix in 0..dims[0] {
                    let x = origin[0] + (ix as f32 + 0.5) * spacing;
                    let p = [x, y, z];
                    let mut d = f32::MAX;
                    for b in &bodies {
                        d = d.min(b.sdf(p, crate::csg::EvalMode::Visual));
                        if d < 0.0 {
                            break; // inside one body ⇒ occupied; no need to test the rest
                        }
                    }
                    if d < 0.0 {
                        occ[idx] = 1;
                    }
                    idx += 1;
                }
            }
        }
        occ
    }
}
