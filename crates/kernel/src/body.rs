//! Multi-body partitioning of the linear feature spine.
//!
//! Up to K3 part 1 the kernel treated the whole document as one
//! monolithic CSG tree: every mutation rebuilt the single mesh that
//! covered everything. That's fine for a one-part document, but the
//! README's K3 bullet is explicit: each disjoint solid in the
//! document should own its own CSG graph, its own BVH and its own
//! mesh cache, and mutations that only touch body `A` must leave
//! body `B`'s mesh unchanged (and its GPU buffers unbound).
//!
//! This module owns the partitioning side of that promise. Given a
//! linear feature list it walks the features in order, groups them
//! into bodies by world-space AABB overlap, and hands back one
//! `BodyPartition` per resulting body. The [`Body`] struct on top of
//! that holds the per-body BVH, mesh, version counter and payload
//! hash used by the incremental re-meshing pass in `lib.rs`.
//!
//! Partitioning rules (kept deliberately small for the MVP):
//!
//! * **Union** — compute the world AABB of the feature. Collect every
//!   existing body whose AABB overlaps it. Zero hits start a new
//!   body; one hit appends to that body; two or more hits *merge*
//!   those bodies into the first one and absorb the new feature.
//!   The per-body AABB grows to cover the new payload.
//! * **Difference** / **Intersection** — look up overlapping bodies
//!   the same way, attach the feature to the *first* overlapping
//!   body (deterministic, stable under renames), and **drop** the
//!   feature if there's nothing to act on. A subtractive sphere with
//!   no body underneath carves nothing either way; explicitly
//!   dropping it keeps the partition function obviously
//!   idempotent-per-body.
//! * Feature order inside a body is always the original document
//!   order. That's the order `CsgNode::from_features` relies on to
//!   seed the first Union and fold the rest.
//!
//! What this rule set deliberately does *not* handle, leaving it to
//! later slices:
//!
//! * Explicit "new body" vs "merge with existing" controls on the
//!   extrude tool (Onshape-style). That's a K7 UI concern.
//! * Difference/Intersection features that genuinely span multiple
//!   bodies (e.g. a hole drilled through two separated plates). We
//!   give them to the first overlapping body only. A future slice
//!   can duplicate the feature across bodies if users complain.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use fielddraft_protocol::{BoolOp, Feature, FeatureId, Primitive};

use crate::bvh::BoundedCsg;
use crate::csg::{Aabb, CsgNode};
use crate::dc::Mesh;

/// One partitioned body as seen by the [`partition`] pass — just the
/// feature indices and the accumulated world AABB, without any
/// mesh/BVH state. Lives long enough for `lib.rs` to diff it against
/// the previous frame's bodies and decide what to remesh.
#[derive(Debug, Clone)]
pub struct BodyPartition {
    /// Indices into the original `Document::features` list, in
    /// original order. The first entry is always the seed Union that
    /// started the body (or the first Union that merged into it if
    /// the seed has since been removed — see the merge case in
    /// [`partition`]).
    pub feature_indices: Vec<usize>,
    /// Running world-space AABB of the body. Grows monotonically
    /// under Union, stays put under Difference / Intersection.
    pub aabb: Aabb,
}

impl BodyPartition {
    /// Stable id for this body. Derived from the lowest feature id
    /// among its members: that id survives across rebuilds as long
    /// as the feature lives in the document, so the frontend can
    /// keep the body's BufferGeometry around even after unrelated
    /// mutations reorder the body list.
    ///
    /// K8.0 accommodates hierarchical ids (linear_pattern
    /// clones): flat-numeric ids sort first by their u32
    /// value, hierarchical ids (clones) fall back to
    /// `u32::MAX` so a body that contains both a
    /// pre-existing primitive and some pattern clones still
    /// picks the primitive as its stable id.
    pub fn seed_id(&self, features: &[Feature]) -> FeatureId {
        self.feature_indices
            .iter()
            .map(|&i| features[i].id.clone())
            .min_by_key(|id| id.as_u32_if_flat().unwrap_or(u32::MAX))
            .expect("BodyPartition is always non-empty")
    }
}

/// A partitioned body plus its meshing state. Owned by `Kernel`.
#[derive(Debug, Clone)]
pub struct Body {
    /// Stable identifier — see [`BodyPartition::seed_id`].
    pub id: FeatureId,
    pub feature_indices: Vec<usize>,
    pub aabb: Aabb,
    /// Hash of the per-feature payload at the time of the last
    /// successful mesh rebuild. `rebuild_bodies` skips re-meshing
    /// when the hash still matches after a partition.
    pub payload_hash: u64,
    /// Voxel grid resolution this body's cached mesh was built at.
    /// Stored alongside the hash so a change from active to context
    /// LOD (or vice versa) correctly invalidates the cache even
    /// when the feature payload is unchanged.
    pub mesh_resolution: u32,
    pub mesh: Mesh,
    /// Per-body version counter. Bumps whenever the mesh is
    /// actually rebuilt (payload hash changed or LOD switched),
    /// not on bookkeeping updates like feature-index renumbering.
    pub version: u32,
}

/// Walk the feature spine once and return one [`BodyPartition`] per
/// resulting body. See the module docs for the rules.
pub fn partition(features: &[Feature]) -> Vec<BodyPartition> {
    let mut groups: Vec<BodyPartition> = Vec::new();
    for (i, feature) in features.iter().enumerate() {
        let feat_aabb = feature_world_aabb(feature);

        // Which existing groups does this feature touch?
        let overlaps: Vec<usize> = groups
            .iter()
            .enumerate()
            .filter(|(_, g)| aabbs_overlap(g.aabb, feat_aabb))
            .map(|(idx, _)| idx)
            .collect();

        match feature.op {
            BoolOp::Union => match overlaps.len() {
                0 => {
                    groups.push(BodyPartition {
                        feature_indices: vec![i],
                        aabb: feat_aabb,
                    });
                }
                1 => {
                    let idx = overlaps[0];
                    groups[idx].feature_indices.push(i);
                    groups[idx].aabb = Aabb::union(groups[idx].aabb, feat_aabb);
                }
                _ => merge_overlapping(&mut groups, &overlaps, i, feat_aabb),
            },
            BoolOp::Difference | BoolOp::Intersect => {
                if let Some(&target) = overlaps.first() {
                    groups[target].feature_indices.push(i);
                    // Diff/Inter never grow the AABB; leave it alone.
                }
                // else: orphan feature, silently dropped.
            }
        }
    }
    groups
}

/// Merge every group listed in `overlap_indices` into the first of
/// them, absorb the bridging feature, and drop the rest from the
/// list. Feature order inside the merged body is re-sorted by
/// original index so the downstream `CsgNode::from_features` fold
/// still replays the document's original sequence.
fn merge_overlapping(
    groups: &mut Vec<BodyPartition>,
    overlap_indices: &[usize],
    new_feature: usize,
    new_feature_aabb: Aabb,
) {
    // Walk from highest to lowest so the `remove` calls don't
    // shift earlier indices underneath us. Collect the absorbed
    // feature lists and AABBs as we go.
    let mut sorted = overlap_indices.to_vec();
    sorted.sort_unstable();
    let (first, rest) = sorted.split_first().expect("merge needs ≥2 groups");
    let target_idx = *first;

    let mut absorbed_features: Vec<usize> = Vec::new();
    let mut absorbed_aabb = Aabb::EMPTY;
    for &idx in rest.iter().rev() {
        let removed = groups.remove(idx);
        absorbed_features.extend(removed.feature_indices);
        absorbed_aabb = Aabb::union(absorbed_aabb, removed.aabb);
    }

    // `target_idx` doesn't need shifting: every `idx` in `rest` is
    // strictly greater than `target_idx` because we sorted ascending.
    let target = &mut groups[target_idx];
    target.feature_indices.extend(absorbed_features);
    target.feature_indices.push(new_feature);
    target.feature_indices.sort_unstable();
    target.aabb = Aabb::union(target.aabb, Aabb::union(absorbed_aabb, new_feature_aabb));
}

/// World-space AABB of a single feature's primitive, transformed by
/// its own `Transform`.
fn feature_world_aabb(feature: &Feature) -> Aabb {
    Aabb::of_primitive(&feature.primitive).transformed(&feature.transform)
}

fn aabbs_overlap(a: Aabb, b: Aabb) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    a.min[0] <= b.max[0]
        && a.max[0] >= b.min[0]
        && a.min[1] <= b.max[1]
        && a.max[1] >= b.min[1]
        && a.min[2] <= b.max[2]
        && a.max[2] >= b.min[2]
}

/// Deterministic hash of a body's feature payload. Drives the
/// incremental re-meshing decision in `Kernel::rebuild_bodies`: when
/// the hash is unchanged across a mutation, the existing mesh is
/// kept verbatim.
///
/// The hash covers every f32 as its raw `u32` bit pattern — that
/// sidesteps `f32`'s lack of `Hash` and, because the kernel only
/// ever produces bit-identical floats for identical inputs, is
/// stable across rebuilds.
pub fn hash_body_payload(features: &[Feature], indices: &[usize]) -> u64 {
    let mut hasher = DefaultHasher::new();
    // Salt the hash with the feature count so an otherwise
    // coincidental f32 pattern from a different feature set doesn't
    // look identical.
    indices.len().hash(&mut hasher);
    for &i in indices {
        let f = &features[i];
        f.id.hash(&mut hasher);
        f.op.tag().hash(&mut hasher);
        hash_primitive(&f.primitive, &mut hasher);
        for v in f.transform.translate {
            v.to_bits().hash(&mut hasher);
        }
        for v in f.transform.rotation {
            v.to_bits().hash(&mut hasher);
        }
        f.transform.scale.to_bits().hash(&mut hasher);
    }
    hasher.finish()
}

fn hash_primitive(primitive: &Primitive, hasher: &mut DefaultHasher) {
    match primitive {
        Primitive::Box { center, half_extents } => {
            0u8.hash(hasher);
            for v in center {
                v.to_bits().hash(hasher);
            }
            for v in half_extents {
                v.to_bits().hash(hasher);
            }
        }
        Primitive::Sphere { center, radius } => {
            1u8.hash(hasher);
            for v in center {
                v.to_bits().hash(hasher);
            }
            radius.to_bits().hash(hasher);
        }
        Primitive::Cylinder {
            center,
            radius,
            half_height,
        } => {
            2u8.hash(hasher);
            for v in center {
                v.to_bits().hash(hasher);
            }
            radius.to_bits().hash(hasher);
            half_height.to_bits().hash(hasher);
        }
        // K7.3: hash the full sketch payload so a profile
        // edit bumps the body cache and triggers a remesh.
        Primitive::Extrude {
            sketch,
            half_depth_z,
            center,
        } => {
            3u8.hash(hasher);
            for v in center {
                v.to_bits().hash(hasher);
            }
            half_depth_z.to_bits().hash(hasher);
            hash_sketch2d(sketch, hasher);
        }
        // K8.4: same reasoning as Extrude — any profile edit
        // must bump the body-cache hash so the remesh fires.
        Primitive::Revolve { sketch, center } => {
            4u8.hash(hasher);
            for v in center {
                v.to_bits().hash(hasher);
            }
            hash_sketch2d(sketch, hasher);
        }
        Primitive::Torus { center, major_radius, minor_radius } => {
            6u8.hash(hasher);
            for v in center {
                v.to_bits().hash(hasher);
            }
            major_radius.to_bits().hash(hasher);
            minor_radius.to_bits().hash(hasher);
        }
        Primitive::Gyroid { cell_size, thickness } => {
            5u8.hash(hasher);
            cell_size.to_bits().hash(hasher);
            thickness.to_bits().hash(hasher);
        }
    }
}

/// K7.3: walk a [`Sketch2d`] tree and push every leaf's
/// coordinates into the hasher. Used by [`hash_primitive`] so
/// the body-cache reuse check notices profile edits.
fn hash_sketch2d(
    sketch: &fielddraft_protocol::Sketch2d,
    hasher: &mut DefaultHasher,
) {
    use fielddraft_protocol::{Sketch2d, Sketch2dPrimitive};
    match sketch {
        Sketch2d::Empty => 0u8.hash(hasher),
        Sketch2d::Prim(prim) => {
            1u8.hash(hasher);
            match prim {
                Sketch2dPrimitive::Circle { center, radius } => {
                    0u8.hash(hasher);
                    for v in center {
                        v.to_bits().hash(hasher);
                    }
                    radius.to_bits().hash(hasher);
                }
                Sketch2dPrimitive::Rectangle {
                    center,
                    half_extents,
                } => {
                    1u8.hash(hasher);
                    for v in center {
                        v.to_bits().hash(hasher);
                    }
                    for v in half_extents {
                        v.to_bits().hash(hasher);
                    }
                }
                Sketch2dPrimitive::Polygon { vertices } => {
                    2u8.hash(hasher);
                    vertices.len().hash(hasher);
                    for vert in vertices {
                        for v in vert {
                            v.to_bits().hash(hasher);
                        }
                    }
                }
            }
        }
        Sketch2d::Union(a, b) => {
            2u8.hash(hasher);
            hash_sketch2d(a, hasher);
            hash_sketch2d(b, hasher);
        }
        Sketch2d::Difference(a, b) => {
            3u8.hash(hasher);
            hash_sketch2d(a, hasher);
            hash_sketch2d(b, hasher);
        }
        Sketch2d::Intersection(a, b) => {
            4u8.hash(hasher);
            hash_sketch2d(a, hasher);
            hash_sketch2d(b, hasher);
        }
        Sketch2d::Offset { inner, distance } => {
            5u8.hash(hasher);
            hash_sketch2d(inner, hasher);
            distance.to_bits().hash(hasher);
        }
    }
}

/// Build the `BoundedCsg` that the mesher and raycaster actually
/// query for a given body. Extracted as a free function so both
/// `Kernel::rebuild_bodies` and the raycaster's hot path can share
/// the logic: take the body's subset of features, fold them into a
/// `CsgNode` via `from_features`, then BVH-wrap.
pub fn build_bounded_for_body(features: &[Feature], indices: &[usize]) -> BoundedCsg {
    let sub: Vec<Feature> = indices.iter().map(|&i| features[i].clone()).collect();
    let csg = CsgNode::from_features(&sub);
    BoundedCsg::from_csg(&csg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fielddraft_protocol::{Feature, Transform};

    fn box_feat(id: u32, op: BoolOp, center: [f32; 3], half: [f32; 3]) -> Feature {
        Feature {
            id: FeatureId::from_u32(id),
            name: format!("Box {id}"),
            op,
            primitive: Primitive::Box {
                center,
                half_extents: half,
            },
            transform: Transform::IDENTITY,
            blend_radius: 0.0,
            chamfer_size: 0.0,
            twist_rate: 0.0,
            bend_rate: 0.0,
            displace_depth: 0.0,
            displace_freq: 0.0,
        }
    }

    fn sphere_feat(id: u32, op: BoolOp, center: [f32; 3], radius: f32) -> Feature {
        Feature {
            id: FeatureId::from_u32(id),
            name: format!("Sphere {id}"),
            op,
            primitive: Primitive::Sphere { center, radius },
            transform: Transform::IDENTITY,
            blend_radius: 0.0,
            chamfer_size: 0.0,
            twist_rate: 0.0,
            bend_rate: 0.0,
            displace_depth: 0.0,
            displace_freq: 0.0,
        }
    }

    #[test]
    fn single_union_feature_produces_one_body() {
        let features = vec![box_feat(1, BoolOp::Union, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0])];
        let bodies = partition(&features);
        assert_eq!(bodies.len(), 1);
        assert_eq!(bodies[0].feature_indices, vec![0]);
        assert_eq!(bodies[0].seed_id(&features), FeatureId::from_u32(1));
    }

    #[test]
    fn two_disjoint_unions_produce_two_bodies() {
        let features = vec![
            box_feat(1, BoolOp::Union, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
            box_feat(2, BoolOp::Union, [100.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
        ];
        let bodies = partition(&features);
        assert_eq!(bodies.len(), 2);
        assert_eq!(bodies[0].feature_indices, vec![0]);
        assert_eq!(bodies[1].feature_indices, vec![1]);
    }

    #[test]
    fn overlapping_unions_merge_into_one_body() {
        let features = vec![
            box_feat(1, BoolOp::Union, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
            box_feat(2, BoolOp::Union, [7.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
        ];
        let bodies = partition(&features);
        assert_eq!(bodies.len(), 1);
        assert_eq!(bodies[0].feature_indices, vec![0, 1]);
    }

    #[test]
    fn difference_attaches_to_overlapping_body() {
        let features = vec![
            box_feat(1, BoolOp::Union, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
            sphere_feat(2, BoolOp::Difference, [0.0, 0.0, 0.0], 3.0),
        ];
        let bodies = partition(&features);
        assert_eq!(bodies.len(), 1);
        assert_eq!(bodies[0].feature_indices, vec![0, 1]);
    }

    #[test]
    fn orphan_difference_is_dropped() {
        let features = vec![
            box_feat(1, BoolOp::Union, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
            // sphere 100 mm away, no body to subtract from.
            sphere_feat(2, BoolOp::Difference, [100.0, 0.0, 0.0], 3.0),
        ];
        let bodies = partition(&features);
        assert_eq!(bodies.len(), 1);
        assert_eq!(
            bodies[0].feature_indices,
            vec![0],
            "orphan difference must not attach"
        );
    }

    #[test]
    fn bridging_union_merges_two_existing_bodies() {
        let features = vec![
            box_feat(1, BoolOp::Union, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
            box_feat(2, BoolOp::Union, [30.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
            // bridging box spanning both
            box_feat(3, BoolOp::Union, [15.0, 0.0, 0.0], [20.0, 5.0, 5.0]),
        ];
        let bodies = partition(&features);
        assert_eq!(bodies.len(), 1);
        // Feature order is preserved by original index.
        assert_eq!(bodies[0].feature_indices, vec![0, 1, 2]);
    }

    #[test]
    fn feature_order_is_preserved_after_merge() {
        // Union A, Difference on A, Union B (far), bridging Union.
        // After the bridge merges A and B, the feature order inside
        // the single resulting body must still be [A, Diff, B, bridge].
        let features = vec![
            box_feat(1, BoolOp::Union, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
            sphere_feat(2, BoolOp::Difference, [0.0, 0.0, 0.0], 2.0),
            box_feat(3, BoolOp::Union, [30.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
            box_feat(4, BoolOp::Union, [15.0, 0.0, 0.0], [20.0, 5.0, 5.0]),
        ];
        let bodies = partition(&features);
        assert_eq!(bodies.len(), 1);
        assert_eq!(bodies[0].feature_indices, vec![0, 1, 2, 3]);
    }

    #[test]
    fn body_id_is_the_minimum_feature_id() {
        // Body gets id = min of its feature ids, not the first
        // element's. Reorder in the list shouldn't change the id.
        let features = vec![
            box_feat(7, BoolOp::Union, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
            box_feat(3, BoolOp::Union, [7.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
        ];
        let bodies = partition(&features);
        assert_eq!(bodies.len(), 1);
        assert_eq!(bodies[0].seed_id(&features), FeatureId::from_u32(3));
    }

    #[test]
    fn hash_is_stable_across_equivalent_feature_sets() {
        let features = vec![
            box_feat(1, BoolOp::Union, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
            sphere_feat(2, BoolOp::Difference, [0.0, 0.0, 0.0], 2.0),
        ];
        let a = hash_body_payload(&features, &[0, 1]);
        let b = hash_body_payload(&features, &[0, 1]);
        assert_eq!(a, b);
    }

    #[test]
    fn hash_changes_when_any_field_changes() {
        let baseline =
            vec![box_feat(1, BoolOp::Union, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0])];
        let h0 = hash_body_payload(&baseline, &[0]);

        let translated = vec![box_feat(1, BoolOp::Union, [0.5, 0.0, 0.0], [5.0, 5.0, 5.0])];
        assert_ne!(h0, hash_body_payload(&translated, &[0]));

        let bigger = vec![box_feat(1, BoolOp::Union, [0.0, 0.0, 0.0], [6.0, 5.0, 5.0])];
        assert_ne!(h0, hash_body_payload(&bigger, &[0]));

        let diff = vec![box_feat(1, BoolOp::Difference, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0])];
        assert_ne!(h0, hash_body_payload(&diff, &[0]));
    }
}
