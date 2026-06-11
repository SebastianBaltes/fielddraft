//! The symbolic source: branch → analytic half-space map and complement
//! parity, walked straight off the CSG tree (DL-19/DL-20).

use std::collections::HashMap;

use crate::bvh::BoundedCsg;
use crate::compose_transforms;
use crate::csg::{offset_halfspace_inward, primitive_halfspaces, BranchId};
use fielddraft_protocol::{HalfSpace, Transform};

// ─── small vector helpers (f64): shared, see brep::vec3 ─────────────────────
use super::*;

/// Walk a [`BoundedCsg`] and build the map `branch (as u32) → (HalfSpace,
/// Transform)` for every analytic sub-face of every leaf. The half-space
/// enumeration is delegated to [`primitive_halfspaces`] (the single source of
/// truth, co-located with `expand_evaluate_leaf` in `csg.rs`, DL-19).
///
/// The stored [`Transform`] maps the half-space's centred local frame to world
/// space: the primitive `center` offset (applied as `lp = p − center` inside
/// the expansion, *not* part of `world_transform`) is folded in as a local
/// pre-translation, `world_transform ∘ translate(center)` (DL-20).
pub fn branch_halfspace_map(body: &BoundedCsg) -> HashMap<u32, (HalfSpace, Transform)> {
    let mut out = HashMap::new();
    collect_halfspaces(body, &mut out);
    out
}

pub(super) fn collect_halfspaces(node: &BoundedCsg, out: &mut HashMap<u32, (HalfSpace, Transform)>) {
    match node {
        BoundedCsg::Leaf {
            id,
            primitive,
            world_transform,
            ..
        } => {
            if let Some(hs) = primitive_halfspaces(primitive) {
                // Fold the primitive `center` plus each face's `local_offset`
                // into the face transform as a local pre-translation: world ∘
                // translate(center + local_offset) (DL-20). The offset is zero
                // for every face except a cone wall, which shifts to its true
                // apex so its `HalfSpace::Cone` SDF lands on both rims (DL-76).
                for f in hs.faces() {
                    let translate = [
                        hs.center[0] + f.local_offset[0],
                        hs.center[1] + f.local_offset[1],
                        hs.center[2] + f.local_offset[2],
                    ];
                    let face_xf = compose_transforms(
                        world_transform,
                        &Transform { translate, ..Transform::IDENTITY },
                    );
                    let branch = BranchId::from_feature_sub(id, f.sub);
                    out.insert(branch.as_u32(), (f.half_space.clone(), face_xf));
                }
            }
            // Primitives without a half-space split (Sphere/Torus/Gyroid/...)
            // and sketch sweeps (Extrude/Revolve) get no analytic entry; their
            // faces fall back to Tessellated in the weaver (DL-25).
        }
        BoundedCsg::Union { left, right, .. }
        | BoundedCsg::Difference { left, right, .. }
        | BoundedCsg::Intersection { left, right, .. }
        | BoundedCsg::SmoothUnion { left, right, .. }
        | BoundedCsg::SmoothDifference { left, right, .. }
        | BoundedCsg::MaskedBlend { left, right, .. }
        | BoundedCsg::Morph { left, right, .. } => {
            collect_halfspaces(left, out);
            collect_halfspaces(right, out);
        }
        BoundedCsg::Twist { inner, .. }
        | BoundedCsg::Bend { inner, .. }
        | BoundedCsg::Displace { inner, .. } => collect_halfspaces(inner, out),
        BoundedCsg::Offset { inner, delta, .. } => {
            // A `δ`-offset shifts each analytic face's surface parameter by `δ`
            // along its normal, keeping the same family and placement — the same
            // closed-form fold the Shell inner wall uses, but signed (outward for
            // δ>0). `offset_halfspace_inward(hs, −δ)` grows the surface by δ.
            // Faces whose offset leaves the family (Cone/Gyroid) get no entry and
            // fall back to `Field::Leaf{Offset}` (curved seam tracer), so the
            // surface is still woven — just not analytically.
            let mut sub = HashMap::new();
            collect_halfspaces(inner, &mut sub);
            for (key, (hs, xf)) in sub {
                if let Some(off) = crate::csg::offset_halfspace_inward(&hs, -delta) {
                    out.insert(key, (off, xf));
                }
            }
        }
        BoundedCsg::Shell { inner, thickness, .. } => {
            // The outer faces are geometrically unchanged (the shell preserves
            // `d = 0`), so they keep their entries. Additionally register each
            // outer analytic face's inward offset under its inner-wall branch id
            // (DL-75) so the weave can classify the hollow's inner wall as a
            // pickable face. The inner wall shares the outer face's transform —
            // `offset_halfspace_inward` only shifts the surface parameter — and
            // skips kinds without an analytic offset (Cone/Gyroid) and degenerate
            // walls (radius/minor ≤ thickness), which fall back to tessellated.
            let mut sub = HashMap::new();
            collect_halfspaces(inner, &mut sub);
            for (key, (hs, xf)) in &sub {
                if let Some(inner_hs) = offset_halfspace_inward(hs, *thickness) {
                    out.entry(BranchId(*key).inner_wall().as_u32())
                        .or_insert((inner_hs, *xf));
                }
            }
            out.extend(sub);
        }
        BoundedCsg::Extrude {
            id,
            sketch,
            half_height,
            world_transform,
            ..
        } => collect_extrude_halfspaces(id, sketch, *half_height, world_transform, out),
        BoundedCsg::Revolve {
            id,
            sketch,
            world_transform,
            ..
        } => collect_revolve_halfspaces(id, sketch, world_transform, out),
        BoundedCsg::Empty => {}
    }
}

/// Complement parity per feature (F0-2): the set of `feature_id`s sitting
/// under an odd number of subtraction branches in the CSG tree. The half-space
/// map stores every face with its **primitive-outward** orientation; for a
/// subtracted feature the body-outward normal is the complement, so its loop
/// winding must flip. Decided symbolically on the tree — never by sampling.
///
/// Shell inner walls flip too, but per *branch* (the [`BranchId::INNER_WALL_FLAG`]
/// bit), see [`branch_is_complemented`].
pub(crate) fn complemented_features(body: &BoundedCsg) -> std::collections::HashSet<u32> {
    let mut out = std::collections::HashSet::new();
    collect_parity(body, false, &mut out);
    out
}

/// Whether `branch`'s primitive-outward surface normal points INTO the body —
/// feature complement parity XOR the shell inner-wall flag (a hollow's inner
/// wall faces the cavity, i.e. opposite its carrier surface).
pub(crate) fn branch_is_complemented(
    branch: BranchId,
    complemented: &std::collections::HashSet<u32>,
) -> bool {
    complemented.contains(&branch.feature_id()) != branch.is_inner_wall()
}

pub(super) fn collect_parity(node: &BoundedCsg, flipped: bool, out: &mut std::collections::HashSet<u32>) {
    match node {
        BoundedCsg::Leaf { id, .. }
        | BoundedCsg::Extrude { id, .. }
        | BoundedCsg::Revolve { id, .. } => {
            if flipped {
                out.insert(id.as_u32_or_root().unwrap_or(0));
            }
        }
        BoundedCsg::Union { left, right, .. }
        | BoundedCsg::Intersection { left, right, .. }
        | BoundedCsg::SmoothUnion { left, right, .. }
        | BoundedCsg::Morph { left, right, .. } => {
            collect_parity(left, flipped, out);
            collect_parity(right, flipped, out);
        }
        BoundedCsg::Difference { left, right, .. }
        | BoundedCsg::SmoothDifference { left, right, .. } => {
            collect_parity(left, flipped, out);
            collect_parity(right, !flipped, out);
        }
        BoundedCsg::MaskedBlend { left, right, is_difference, .. } => {
            collect_parity(left, flipped, out);
            collect_parity(right, flipped != *is_difference, out);
        }
        BoundedCsg::Twist { inner, .. }
        | BoundedCsg::Bend { inner, .. }
        | BoundedCsg::Displace { inner, .. }
        | BoundedCsg::Shell { inner, .. }
        | BoundedCsg::Offset { inner, .. } => collect_parity(inner, flipped, out),
        BoundedCsg::Empty => {}
    }
}

// ─── Extrude / Revolve analytic faces (DL-50) ────────────────────────────────
