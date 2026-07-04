//! FieldDraft geometry kernel.
//!
//! The kernel owns the canonical `Document`, builds a CSG tree from its
//! feature spine, and meshes it via Dual Contouring. Mesh buffers are
//! exposed to JavaScript through a zero-copy pointer interface.

pub mod body;
pub mod brep;
pub mod bvh;
pub mod csg;
pub mod sdf_dual;
pub mod dc;
pub mod mdc;
pub mod mt;
pub mod rfta;
pub mod expr;
pub mod features;
// K13: mesh_sdf moved to the protocol crate so
// Primitive::ImportedMesh can carry it serialised.
pub use fielddraft_protocol::mesh_sdf;
pub mod sketch_regions;
pub mod queries;
pub mod export;
pub mod export_3mf;
pub mod export_mesh;
pub mod mdc_adaptive;
pub mod mesh_validate;
pub mod raycast;
pub mod step_classifier;
pub mod step_commands;
pub mod sdf_compiler;
pub mod sketch2d;
pub mod sketch;
pub mod path_provider;
pub mod sweep_path;
pub mod sweep_gate;
pub mod script_runtime;
pub mod transform;
pub mod sdf_eval;
pub mod finite;
pub mod vec3f;

mod api;
mod kernel_core;

pub use kernel_core::Kernel;

use wasm_bindgen::prelude::*;

/// Default voxel grid resolution for the *active* body — the one the
/// user is currently editing. 48 cells along the longest AABB axis
/// strikes a balance between rebuild latency (a handful of ms for a
/// small spine) and visible smoothness for a 300 mm build volume.
/// The frontend overrides this to 256 for high-quality display; the
/// narrow-band mesher makes that practical.
pub const DEFAULT_MESH_RESOLUTION: u32 = 48;

/// Default voxel grid resolution for *context* bodies — the ones
/// that aren't currently being edited. Half of the active
/// resolution means ≈ 8× fewer cells and ≈ 8× less meshing work
/// when the partition pass sweeps across them, while still looking
/// identifiable enough on screen to serve as background reference.
pub const DEFAULT_CONTEXT_MESH_RESOLUTION: u32 = 24;

// ─── Transform- & Referenz-SDF-Algebra: siehe transform.rs / sdf_eval.rs ──

pub use sdf_eval::{helix_compose, helix_profile_anchor, helix_profile_point, sdf_primitive};
pub use transform::{compose_transforms, inverse, quat_multiply, quat_rotate, world_to_local};
pub(crate) use transform::quat_to_euler_xyz;

/// The app version this WASM binary was stamped with at build time
/// (see `build.rs`). The web app compares it against `__APP_VERSION__`
/// on boot to catch a stale kernel module. Returns "dev" when built
/// without `FIELDDRAFT_VERSION` (e.g. `cargo test`).
#[wasm_bindgen(js_name = wasmVersion)]
pub fn wasm_version() -> String {
    env!("FIELDDRAFT_VERSION").to_string()
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
