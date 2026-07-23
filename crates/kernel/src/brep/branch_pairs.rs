//! Static branch enumeration over the CSG tree (parametric-weaving rebuild,
//! Phase 1 foundation — see `docs/PARAMETRIC-WEAVING-PLAN.md`).
//!
//! A *branch* is one analytic sub-surface of a leaf primitive — a box face, a
//! cylinder wall, an extrude cap, a revolve wall group, … — identified by the
//! same [`BranchId`] the SDF evaluate path tags points with. This module walks
//! a [`BoundedCsg`] and produces the flat set of every branch in the tree, each
//! carrying the owning leaf's world-space [`Aabb`] as a safe over-approximation
//! of the branch's support.
//!
//! Scope is deliberately tight: this is *only* the enumeration. Candidate-pair
//! generation, intrinsic-multiplicity SSI, and per-pair seeding (Phase 1 proper)
//! and the active-interval march (Phase 2) all build on top of this and live
//! elsewhere.
//!
//! ## Branch decomposition per leaf kind
//!
//! - **Analytic primitives** (Box / Cylinder / Cone / Sphere / Torus): the
//!   sub-faces of [`primitive_halfspaces`] — the single source of truth shared
//!   with the SDF tie-break — mapped to a [`SurfaceKind`] by their half-space.
//! - **Extrude**: one [`SurfaceKind::ExtrudeWall`] per sketch boundary group
//!   (`0 .. sketch.edge_count()`) plus the two reserved caps
//!   ([`BranchId::EXTRUDE_CAP_TOP_SUB`] / [`BranchId::EXTRUDE_CAP_BOTTOM_SUB`],
//!   tagged [`SurfaceKind::ExtrudeCap`]).
//! - **Revolve**: one [`SurfaceKind::RevolveWall`] per sketch boundary group; a
//!   revolve has **no** caps.
//! - **Out-of-scope carriers** (Twist / Bend / Displace and any non-half-space
//!   primitive such as Thread / Gyroid / Ellipsoid / Capsule / HexPrism /
//!   ImportedMesh): these have no closed-form branch-pair seeding. We do **not**
//!   invent branches for them — they fall back to the octree — but we *do* still
//!   recurse the tree structurally so branches under them (e.g. a box inside a
//!   `Twist`) are not lost.

use crate::bvh::BoundedCsg;
use crate::csg::{primitive_halfspaces, Aabb, BranchId};
use fielddraft_protocol::{FeatureId, HalfSpace};

/// The analytic surface class a [`Branch`] lies on. Mirrors the half-space
/// families plus the sketch-sweep wall/cap split; [`SurfaceKind::Tessellated`]
/// is reserved for branches that exist topologically but have no closed-form
/// surface (currently unused by the enumerator — out-of-scope carriers emit no
/// branches at all — but kept so downstream pair logic can tag fallbacks).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceKind {
    Plane,
    Cylinder,
    Cone,
    Sphere,
    Torus,
    ExtrudeWall,
    ExtrudeCap,
    RevolveWall,
    Tessellated,
}

/// One enumerated branch: its [`BranchId`], analytic [`SurfaceKind`], and the
/// owning leaf's world-space [`Aabb`]. The AABB is a conservative bound on the
/// branch's support (the whole leaf, not the trimmed face), which is exactly
/// what Phase 1 pair-pruning wants — a cheap, sound overlap test.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Branch {
    pub id: BranchId,
    pub kind: SurfaceKind,
    pub aabb: Aabb,
}

/// Whether a [`BranchPair`] joins two sub-faces of the **same** leaf operand
/// (`Intrinsic` — the operand's own edges) or two sub-faces across a Boolean
/// (`Cross` — edges carved by the boolean op). The distinction matters in Phase
/// 1b: intrinsic pairs need intrinsic-multiplicity SSI (a box's parallel faces
/// never meet), cross pairs are genuine carve intersections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairOrigin {
    Intrinsic,
    Cross,
}

/// A candidate pair of branches whose surfaces *might* meet in an edge, with the
/// per-axis AABB intersection of the two branches as the seed search box for
/// Phase 1b. The pair is canonicalised so `a.as_u32() <= b.as_u32()`.
///
/// This is a **candidate**: pair enumeration deliberately over-generates (a box
/// yields all C(6,2)=15 intrinsic pairs including the 3 parallel-opposite-face
/// pairs that never intersect). Phase 1b's closed-form SSI component count is
/// what drops pairs with no real intersection. Soundness here means "never miss
/// a real edge", not "only emit real edges".
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BranchPair {
    pub a: BranchId,
    pub b: BranchId,
    pub kind_a: SurfaceKind,
    pub kind_b: SurfaceKind,
    pub overlap: Aabb,
    pub origin: PairOrigin,
}

// Slack for the AABB overlap test (see `brep::tol`). Real edges live exactly
// where two AABBs *touch* — adjacent solids sharing a face, a cutting cylinder
// tangent to a box wall — so a strict `min < max` test would wrongly prune
// them; with this slack a touching/coincident pair counts as overlapping.
use super::tol::AABB_TOUCH_SLACK as OVERLAP_EPS;

/// Statically enumerate **every** branch in the CSG tree, each carrying its
/// owning leaf's world AABB. Out-of-scope carriers (Twist/Bend/Displace and
/// non-half-space primitives) contribute no branches of their own but are still
/// recursed through, so in-scope branches nested beneath them are preserved.
pub fn enumerate_branches(body: &BoundedCsg) -> Vec<Branch> {
    let mut out = Vec::new();
    walk(body, &mut out);
    out
}

/// Enumerate candidate branch pairs whose surfaces *might* meet in an edge,
/// pruned by world-AABB overlap. See [`BranchPair`] for the candidate/soundness
/// contract: this over-generates and Phase 1b prunes.
///
/// One pass, no subtree re-walking: [`collect_pairs`] returns each subtree's
/// branch list *and* appends its pairs, so a Boolean node pairs its two operands'
/// already-collected branch lists rather than re-enumerating them.
pub fn enumerate_branch_pairs(body: &BoundedCsg) -> Vec<BranchPair> {
    let mut out = Vec::new();
    collect_pairs(body, &mut out);
    out
}

/// Map a primitive sub-face's [`HalfSpace`] to its [`SurfaceKind`]. The
/// half-space family *is* the surface class for analytic primitives, so this is
/// a direct 1:1 mapping. `Gyroid` cannot reach here (no primitive that emits a
/// `HalfSpace::Gyroid` is enumerated by [`primitive_halfspaces`]) but is mapped
/// to `Tessellated` for totality.
fn kind_of_halfspace(hs: &HalfSpace) -> SurfaceKind {
    match hs {
        HalfSpace::Plane { .. } => SurfaceKind::Plane,
        HalfSpace::InfiniteCylinder { .. } => SurfaceKind::Cylinder,
        HalfSpace::Cone { .. } => SurfaceKind::Cone,
        HalfSpace::Sphere { .. } => SurfaceKind::Sphere,
        HalfSpace::Torus { .. } => SurfaceKind::Torus,
        HalfSpace::Gyroid { .. } => SurfaceKind::Tessellated,
        // Cannot reach here either (primitives never emit profile carriers;
        // revolve walls are enumerated via `push_sketch_walls`), but the
        // honest kind for totality is the revolve wall.
        HalfSpace::RevolvedProfile { .. } => SurfaceKind::RevolveWall,
        // Procedural fillet carrier — not a seed-route surface class (its SSI
        // has no closed form); a body with a blend face weaves on the octree
        // route.
        HalfSpace::Blend { .. } => SurfaceKind::Tessellated,
    }
}

/// Emit the wall-group branches of a sketch sweep: subs `0 .. group_count`,
/// tagged with `wall_kind`. Shared by Extrude and Revolve.
fn push_sketch_walls(
    id: &FeatureId,
    group_count: u32,
    wall_kind: SurfaceKind,
    aabb: Aabb,
    wall_subs: Option<&[u32]>,
    out: &mut Vec<Branch>,
) {
    for g in 0..group_count {
        out.push(Branch {
            id: BranchId::from_feature_sub(id, crate::csg::map_wall_sub(wall_subs, g)),
            kind: wall_kind,
            aabb,
        });
    }
}

/// Build the branches of a single in-scope leaf node (Leaf / Extrude / Revolve),
/// the one place the per-leaf decomposition lives so both [`walk`] and
/// [`collect_pairs`] share it verbatim. Returns an empty vec for any other node
/// kind (Booleans, carriers, Empty) — those are structural, not leaves.
fn leaf_branches(node: &BoundedCsg) -> Vec<Branch> {
    let mut out = Vec::new();
    match node {
        BoundedCsg::Leaf { id, primitive, aabb, .. } => {
            // Analytic primitives decompose via the shared half-space source of
            // truth. Primitives with no closed-form split (Sphere/Torus return
            // a single face here; Thread/Ellipsoid/… return `None`) emit
            // only what `primitive_halfspaces` yields — no invented branches.
            if let Some(hs) = primitive_halfspaces(primitive) {
                for f in hs.faces() {
                    out.push(Branch {
                        id: BranchId::from_feature_sub(id, f.sub),
                        kind: kind_of_halfspace(&f.half_space),
                        aabb: *aabb,
                    });
                }
            } else if matches!(primitive, fielddraft_protocol::Primitive::Gyroid { .. }) {
                // A gyroid is ONE closed analytic face (Sphere/Torus topology,
                // SA-6), enumerated here directly instead of extending
                // `primitive_halfspaces` — that is also the hot-SDF SSOT, whose
                // `_` attribution arm already reports the whole feature as
                // `BranchId::from_feature` == sub 0, so branch identity matches
                // without touching the hot path. Its rim seams seed via SSI
                // (gyroid as implicit operand) and trace via the dual field.
                //
                // The branch AABB is INFINITE, not the leaf's: the lattice is
                // space-filling, so `Aabb::of_primitive(Gyroid)` is the EMPTY
                // sentinel — which would fail every `aabb_overlap` and prune
                // all its cross pairs. An infinite box overlaps everything and
                // the pair's overlap collapses to the OTHER operand's AABB,
                // exactly the region where rim seams can live.
                out.push(Branch {
                    id: BranchId::from_feature_sub(id, 0),
                    kind: SurfaceKind::Tessellated,
                    aabb: Aabb {
                        min: [f32::NEG_INFINITY; 3],
                        max: [f32::INFINITY; 3],
                    },
                });
            }
            // else: out-of-scope primitive (Thread/Ellipsoid/Capsule/
            // HexPrism/ImportedMesh) → octree fallback, no branches.
        }

        BoundedCsg::Extrude { id, sketch, aabb, wall_subs, .. } => {
            // Wall groups `0 .. edge_count`, mapped to their stable subs …
            push_sketch_walls(
                id,
                sketch.edge_count(),
                SurfaceKind::ExtrudeWall,
                *aabb,
                wall_subs.as_deref().map(|v| v.as_slice()),
                &mut out,
            );
            // … plus the two reserved caps (DL-50).
            for cap_sub in [
                BranchId::EXTRUDE_CAP_TOP_SUB,
                BranchId::EXTRUDE_CAP_BOTTOM_SUB,
            ] {
                out.push(Branch {
                    id: BranchId::from_feature_sub(id, cap_sub),
                    kind: SurfaceKind::ExtrudeCap,
                    aabb: *aabb,
                });
            }
        }

        BoundedCsg::Revolve { id, sketch, aabb, wall_subs, .. } => {
            // Revolve has no caps — only wall groups.
            push_sketch_walls(
                id,
                sketch.edge_count(),
                SurfaceKind::RevolveWall,
                *aabb,
                wall_subs.as_deref().map(|v| v.as_slice()),
                &mut out,
            );
        }

        _ => {}
    }
    out
}

fn walk(node: &BoundedCsg, out: &mut Vec<Branch>) {
    match node {
        BoundedCsg::Empty => {}

        BoundedCsg::Leaf { .. }
        | BoundedCsg::Extrude { .. }
        | BoundedCsg::Revolve { .. }
        | BoundedCsg::Helix { .. }
        | BoundedCsg::Sweep { .. }
        // #385 P2: loft bodies are weave-excluded (see `body.rs`), so this walk
        // never actually sees one; `leaf_branches` returns no branches for it.
        | BoundedCsg::Loft { .. } => {
            out.extend(leaf_branches(node));
        }

        // Boolean / blend nodes: recurse both operands.
        BoundedCsg::Union { left, right, .. }
        | BoundedCsg::Difference { left, right, .. }
        | BoundedCsg::Intersection { left, right, .. }
        | BoundedCsg::SmoothUnion { left, right, .. }
        | BoundedCsg::SmoothDifference { left, right, .. }
        | BoundedCsg::MaskedBlend { left, right, .. }
        | BoundedCsg::Morph { left, right, .. } => {
            walk(left, out);
            walk(right, out);
        }

        // A Shell duplicates every wrapped branch as its inward-offset inner
        // wall (the cavity boundary, [`BranchId::inner_wall`]) — the same ids
        // the registry's DL-75 entries and the SDF contributor tagging use.
        // OPENED faces are skipped: the retained-boundary erosion passes the
        // cavity straight through them, so they have no inner wall (and the
        // registry deliberately registers none — enumerating one would bail
        // the body to the octree over a missing carrier). Zero-area carriers
        // (a revolve's on-axis group) still enumerate; their seeds simply
        // land inactive and are dropped.
        BoundedCsg::Shell { inner, open_faces, .. } => {
            let before = out.len();
            walk(inner, out);
            let walls: Vec<Branch> = out[before..]
                .iter()
                .filter(|b| !open_faces.iter().any(|f| f.branch == b.id.as_u32()))
                .map(|b| Branch { id: b.id.inner_wall(), ..*b })
                .collect();
            out.extend(walls);
        }

        // #384 D2: the dualised closed shell's eroded copy. Its branches ARE the
        // cavity inner wall, so remap every one to the DL-75 inner-wall band —
        // the same ids the registry's inner-wall carriers and the SDF
        // `evaluate_open` remap use. The outer branches come from the enclosing
        // `Difference`'s other operand (the un-eroded body), so we emit ONLY the
        // walls here (mirror of the `Shell` arm's `walls`, minus the outer copy).
        BoundedCsg::InnerWall { inner, .. } => {
            let before = out.len();
            walk(inner, out);
            for b in &mut out[before..] {
                b.id = b.id.inner_wall();
            }
        }

        // Single-child carriers: recurse the inner subtree. Twist/Bend/Displace
        // are out-of-scope warps (their *own* surface is non-analytic), but the
        // branches of the wrapped geometry must still be enumerated; Offset
        // preserves its inner branches' identity (DL-25 folds analytic carriers
        // back into a shifted half-space downstream).
        BoundedCsg::Twist { inner, .. }
        | BoundedCsg::Bend { inner, .. }
        | BoundedCsg::Displace { inner, .. }
        | BoundedCsg::Offset { inner, .. }
        | BoundedCsg::ArcChannel { inner, .. } => walk(inner, out),
    }
}

/// Per-axis AABB intersection with the inclusive [`OVERLAP_EPS`] touch test.
/// Returns `None` if the boxes are farther than `OVERLAP_EPS` apart on any axis.
/// The returned box is always valid (`min <= max`): on a touching axis where the
/// raw intersection inverts, both bounds collapse to the axis midpoint so the
/// result stays usable as a (possibly flat) seed search region.
fn aabb_overlap(a: &Aabb, b: &Aabb) -> Option<Aabb> {
    let mut min = [0.0f32; 3];
    let mut max = [0.0f32; 3];
    for i in 0..3 {
        // Real edges live where the boxes touch, so the test is inclusive of
        // coincident faces (`<=` with a small absolute slack), never strict.
        if a.min[i] > b.max[i] + OVERLAP_EPS || b.min[i] > a.max[i] + OVERLAP_EPS {
            return None;
        }
        let lo = a.min[i].max(b.min[i]);
        let hi = a.max[i].min(b.max[i]);
        if lo <= hi {
            min[i] = lo;
            max[i] = hi;
        } else {
            // Touching/coincident-within-eps: collapse to the midpoint so the
            // seed box is flat but valid rather than inverted.
            let mid = 0.5 * (lo + hi);
            min[i] = mid;
            max[i] = mid;
        }
    }
    Some(Aabb { min, max })
}

/// Build a canonicalised [`BranchPair`] (`a.as_u32() <= b.as_u32()`) from two
/// branches and a pre-computed overlap box, so pairs are deterministic and
/// `(a,b)` is never emitted alongside `(b,a)`.
fn make_pair(x: &Branch, y: &Branch, overlap: Aabb, origin: PairOrigin) -> BranchPair {
    let (a, b) = if x.id.as_u32() <= y.id.as_u32() { (x, y) } else { (y, x) };
    BranchPair {
        a: a.id,
        b: b.id,
        kind_a: a.kind,
        kind_b: b.kind,
        overlap,
        origin,
    }
}

/// Recursively collect candidate pairs and return the subtree's branch list in a
/// single pass. Leaves emit all intrinsic C(n,2) pairs over their own branches;
/// Boolean nodes emit AABB-overlapping cross pairs between their two already-
/// collected operand branch lists; carriers pass through.
fn collect_pairs(node: &BoundedCsg, out: &mut Vec<BranchPair>) -> Vec<Branch> {
    match node {
        BoundedCsg::Empty => Vec::new(),

        BoundedCsg::Leaf { .. }
        | BoundedCsg::Extrude { .. }
        | BoundedCsg::Revolve { .. }
        | BoundedCsg::Helix { .. }
        | BoundedCsg::Sweep { .. }
        // #385 P2: weave-excluded, never reached in practice (leaf_branches → ∅).
        | BoundedCsg::Loft { .. } => {
            let bs = leaf_branches(node);
            // Intrinsic pairs: the operand's own candidate edges. All branches of
            // a leaf share its AABB, so the overlap box is simply that AABB.
            for i in 0..bs.len() {
                for j in (i + 1)..bs.len() {
                    out.push(make_pair(&bs[i], &bs[j], bs[i].aabb, PairOrigin::Intrinsic));
                }
            }
            bs
        }

        // Boolean / blend nodes: pair every left branch with every right branch
        // that overlaps it. Cross pairs are the carve intersections.
        BoundedCsg::Union { left, right, .. }
        | BoundedCsg::Difference { left, right, .. }
        | BoundedCsg::Intersection { left, right, .. }
        | BoundedCsg::SmoothUnion { left, right, .. }
        | BoundedCsg::SmoothDifference { left, right, .. }
        | BoundedCsg::MaskedBlend { left, right, .. }
        | BoundedCsg::Morph { left, right, .. } => {
            let lb = collect_pairs(left, out);
            let rb = collect_pairs(right, out);
            for a in &lb {
                for b in &rb {
                    if let Some(overlap) = aabb_overlap(&a.aabb, &b.aabb) {
                        out.push(make_pair(a, b, overlap, PairOrigin::Cross));
                    }
                }
            }
            let mut all = lb;
            all.extend(rb);
            all
        }

        // A Shell adds the cavity's candidate edges: inner-wall × inner-wall
        // (the cavity's own corners — e.g. the vase's cavity-floor ring) and
        // outer × inner-wall (rim circles where a surviving outer face's
        // annulus meets the cavity wall). A branch paired with its OWN inner
        // wall is a constant-offset pair with no intersection; SSI rejects it
        // with zero components, so it is not special-cased here.
        BoundedCsg::Shell { inner, open_faces, .. } => {
            let bs = collect_pairs(inner, out);
            let walls: Vec<Branch> = bs
                .iter()
                .filter(|b| !open_faces.iter().any(|f| f.branch == b.id.as_u32()))
                .map(|b| Branch { id: b.id.inner_wall(), ..*b })
                .collect();
            let origin = |x: &Branch, y: &Branch| {
                if x.id.feature_id() == y.id.feature_id() {
                    PairOrigin::Intrinsic
                } else {
                    PairOrigin::Cross
                }
            };
            for i in 0..walls.len() {
                for j in (i + 1)..walls.len() {
                    if let Some(overlap) = aabb_overlap(&walls[i].aabb, &walls[j].aabb) {
                        out.push(make_pair(&walls[i], &walls[j], overlap, origin(&walls[i], &walls[j])));
                    }
                }
            }
            for a in &bs {
                for w in &walls {
                    if let Some(overlap) = aabb_overlap(&a.aabb, &w.aabb) {
                        out.push(make_pair(a, w, overlap, origin(a, w)));
                    }
                }
            }
            let mut all = bs;
            all.extend(walls);
            all
        }

        // #384 D2: the dualised closed shell's eroded copy — remap its branch
        // list AND its intrinsic pairs (the cavity's own corners) to the DL-75
        // inner-wall band. Setting the same flag bit on both endpoints preserves
        // the canonical `a <= b` ordering (all subs are `< INNER_WALL_FLAG` in a
        // freshly eroded copy). The outer×inner-wall rim pairs are produced by
        // the enclosing `Difference`'s cross-pairing.
        BoundedCsg::InnerWall { inner, .. } => {
            let mut sub = Vec::new();
            let bs = collect_pairs(inner, &mut sub);
            for p in &mut sub {
                p.a = p.a.inner_wall();
                p.b = p.b.inner_wall();
            }
            out.extend(sub);
            bs.into_iter()
                .map(|b| Branch { id: b.id.inner_wall(), ..b })
                .collect()
        }

        // Single-child carriers: pass through the inner branch list, no pair at
        // this node (the warp surface is non-analytic / offset folds downstream).
        BoundedCsg::Twist { inner, .. }
        | BoundedCsg::Bend { inner, .. }
        | BoundedCsg::Displace { inner, .. }
        | BoundedCsg::Offset { inner, .. }
        | BoundedCsg::ArcChannel { inner, .. } => collect_pairs(inner, out),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::csg::CsgNode;
    use crate::sketch2d::{Sketch2d, Sketch2dPrimitive};
    use fielddraft_protocol::Primitive;

    fn aabb_non_degenerate(a: &Aabb) -> bool {
        a.min[0] <= a.max[0] && a.min[1] <= a.max[1] && a.min[2] <= a.max[2]
    }

    fn assert_all_aabbs_sane(branches: &[Branch]) {
        for b in branches {
            assert!(
                aabb_non_degenerate(&b.aabb),
                "branch {:?} has a degenerate AABB {:?}",
                b.id.as_u32(),
                b.aabb
            );
        }
    }

    fn box_leaf(id: u32, center: [f32; 3], half_extents: [f32; 3]) -> CsgNode {
        CsgNode::Leaf {
            id: FeatureId::from_u32(id),
            primitive: Primitive::Box { center, half_extents },
        }
    }

    fn cylinder_leaf(id: u32, center: [f32; 3], radius: f32, half_height: f32) -> CsgNode {
        CsgNode::Leaf {
            id: FeatureId::from_u32(id),
            primitive: Primitive::Cylinder { center, radius, half_height },
        }
    }

    #[test]
    fn box_has_six_plane_branches() {
        let body = BoundedCsg::from_csg(&box_leaf(1, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]));
        let branches = enumerate_branches(&body);
        assert_eq!(branches.len(), 6, "a box has six faces");
        assert!(
            branches.iter().all(|b| b.kind == SurfaceKind::Plane),
            "every box face is a plane: {branches:?}"
        );
        // Sub-ids are exactly 0..=5 (the `primitive_halfspaces` convention).
        let mut subs: Vec<u32> = branches.iter().map(|b| b.id.sub()).collect();
        subs.sort_unstable();
        assert_eq!(subs, vec![0, 1, 2, 3, 4, 5]);
        assert_all_aabbs_sane(&branches);
    }

    #[test]
    fn cylinder_has_wall_plus_two_caps() {
        let body = BoundedCsg::from_csg(&cylinder_leaf(2, [0.0, 0.0, 0.0], 5.0, 8.0));
        let branches = enumerate_branches(&body);
        assert_eq!(branches.len(), 3, "cylinder = wall + 2 caps");
        let cyls = branches.iter().filter(|b| b.kind == SurfaceKind::Cylinder).count();
        let planes = branches.iter().filter(|b| b.kind == SurfaceKind::Plane).count();
        assert_eq!(cyls, 1, "one cylindrical wall");
        assert_eq!(planes, 2, "two planar caps");
        assert_all_aabbs_sane(&branches);
    }

    #[test]
    fn extrude_of_rectangle_has_four_walls_and_two_caps() {
        let sketch = Sketch2d::Prim(Sketch2dPrimitive::Rectangle {
            center: [0.0, 0.0],
            half_extents: [5.0, 3.0],
        });
        let csg = CsgNode::Extrude { wall_subs: None,
            id: FeatureId::from_u32(7),
            sketch: Box::new(sketch),
            half_height: 2.0,
        };
        let body = BoundedCsg::from_csg(&csg);
        let branches = enumerate_branches(&body);

        let walls = branches.iter().filter(|b| b.kind == SurfaceKind::ExtrudeWall).count();
        let caps = branches.iter().filter(|b| b.kind == SurfaceKind::ExtrudeCap).count();
        assert_eq!(walls, 4, "rectangle → 4 wall groups");
        assert_eq!(caps, 2, "extrude → top + bottom cap");
        assert_eq!(branches.len(), 6);

        // The reserved cap sub-ids must be present.
        let subs: Vec<u32> = branches.iter().map(|b| b.id.sub()).collect();
        assert!(
            subs.contains(&BranchId::EXTRUDE_CAP_TOP_SUB),
            "top-cap sub 0x{:X} missing: {subs:?}",
            BranchId::EXTRUDE_CAP_TOP_SUB
        );
        assert!(
            subs.contains(&BranchId::EXTRUDE_CAP_BOTTOM_SUB),
            "bottom-cap sub 0x{:X} missing: {subs:?}",
            BranchId::EXTRUDE_CAP_BOTTOM_SUB
        );
        // Wall groups are subs 0..4.
        for g in 0..4u32 {
            assert!(subs.contains(&g), "wall group {g} missing: {subs:?}");
        }
        assert_all_aabbs_sane(&branches);
    }

    #[test]
    fn revolve_has_wall_groups_and_no_caps() {
        // A rectangle profile at x≥0 revolved around Y → a hollow-free
        // cylinder-like solid. Rectangle = 4 boundary groups.
        let sketch = Sketch2d::Prim(Sketch2dPrimitive::Rectangle {
            center: [5.0, 0.0],
            half_extents: [2.0, 10.0],
        });
        let csg = CsgNode::Revolve { wall_subs: None,
            id: FeatureId::from_u32(3),
            sketch: Box::new(sketch),
        };
        let body = BoundedCsg::from_csg(&csg);
        let branches = enumerate_branches(&body);

        assert_eq!(branches.len(), 4, "rectangle → 4 revolve wall groups");
        assert!(
            branches.iter().all(|b| b.kind == SurfaceKind::RevolveWall),
            "revolve emits only walls, no caps: {branches:?}"
        );
        // Explicitly assert no cap sub-ids leaked in.
        let subs: Vec<u32> = branches.iter().map(|b| b.id.sub()).collect();
        assert!(!subs.contains(&BranchId::EXTRUDE_CAP_TOP_SUB));
        assert!(!subs.contains(&BranchId::EXTRUDE_CAP_BOTTOM_SUB));
        assert_all_aabbs_sane(&branches);
    }

    #[test]
    fn difference_is_the_union_of_both_operands_branches() {
        // Box − Cylinder: 6 box planes + (1 wall + 2 caps) = 9 branches.
        let csg = CsgNode::Difference(
            Box::new(box_leaf(1, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0])),
            Box::new(cylinder_leaf(2, [0.0, 0.0, 0.0], 5.0, 12.0)),
        );
        let body = BoundedCsg::from_csg(&csg);
        let branches = enumerate_branches(&body);

        assert_eq!(branches.len(), 6 + 3, "box(6) ∪ cylinder(3) branches");
        let planes = branches.iter().filter(|b| b.kind == SurfaceKind::Plane).count();
        let cyls = branches.iter().filter(|b| b.kind == SurfaceKind::Cylinder).count();
        assert_eq!(planes, 6 + 2, "6 box faces + 2 cylinder caps");
        assert_eq!(cyls, 1);

        // Branches from the two operands carry distinct feature ids.
        let feats: std::collections::HashSet<u32> =
            branches.iter().map(|b| b.id.feature_id()).collect();
        assert_eq!(feats.len(), 2, "two leaves → two feature ids: {feats:?}");
        assert_all_aabbs_sane(&branches);
    }

    #[test]
    fn out_of_scope_warp_still_recurses_to_inner_branches() {
        // A Twist wrapping a box: the warp itself is out of scope (no
        // analytic branch), but the inner box's 6 branches must survive.
        let csg = CsgNode::Twist(
            Box::new(box_leaf(1, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0])),
            0.01,
        );
        let body = BoundedCsg::from_csg(&csg);
        let branches = enumerate_branches(&body);
        assert_eq!(branches.len(), 6, "inner box branches survive the warp");
        assert!(branches.iter().all(|b| b.kind == SurfaceKind::Plane));
    }

    // ---- candidate branch-pair enumeration ----------------------------------

    fn assert_no_dup_pairs(pairs: &[BranchPair]) {
        let mut keys: Vec<(u32, u32)> =
            pairs.iter().map(|p| (p.a.as_u32(), p.b.as_u32())).collect();
        let n = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), n, "duplicate (a,b) pairs present");
    }

    fn assert_canonical(pairs: &[BranchPair]) {
        for p in pairs {
            assert!(
                p.a.as_u32() <= p.b.as_u32(),
                "pair not canonicalised: {} > {}",
                p.a.as_u32(),
                p.b.as_u32()
            );
        }
    }

    fn assert_all_overlaps_valid(pairs: &[BranchPair]) {
        for p in pairs {
            assert!(
                aabb_non_degenerate(&p.overlap),
                "pair ({},{}) has an inverted overlap box {:?}",
                p.a.as_u32(),
                p.b.as_u32(),
                p.overlap
            );
        }
    }

    fn count_by_origin(pairs: &[BranchPair], origin: PairOrigin) -> usize {
        pairs.iter().filter(|p| p.origin == origin).count()
    }

    #[test]
    fn lone_box_yields_fifteen_intrinsic_pairs() {
        let body = BoundedCsg::from_csg(&box_leaf(1, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]));
        let pairs = enumerate_branch_pairs(&body);
        assert_eq!(pairs.len(), 15, "C(6,2) = 15 candidate pairs");
        assert_eq!(count_by_origin(&pairs, PairOrigin::Intrinsic), 15);
        assert_eq!(count_by_origin(&pairs, PairOrigin::Cross), 0);
        assert!(
            pairs.iter().all(|p| p.origin == PairOrigin::Intrinsic),
            "a lone leaf has only intrinsic pairs"
        );
        assert_no_dup_pairs(&pairs);
        assert_canonical(&pairs);
        assert_all_overlaps_valid(&pairs);
    }

    #[test]
    fn box_minus_concentric_cylinder_counts() {
        // Cylinder AABB ⊂ box AABB → every box×cylinder branch pair overlaps.
        let csg = CsgNode::Difference(
            Box::new(box_leaf(1, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0])),
            Box::new(cylinder_leaf(2, [0.0, 0.0, 0.0], 5.0, 12.0)),
        );
        let body = BoundedCsg::from_csg(&csg);
        let pairs = enumerate_branch_pairs(&body);

        // Cross = 6 box faces × 3 cylinder branches (wall + 2 caps) = 18.
        let cross = count_by_origin(&pairs, PairOrigin::Cross);
        assert_eq!(cross, 18, "6×3 overlapping cross pairs");
        // Intrinsic = box C(6,2)=15 + cylinder C(3,2)=3 = 18.
        let intrinsic = count_by_origin(&pairs, PairOrigin::Intrinsic);
        assert_eq!(intrinsic, 15 + 3, "box(15) + cylinder(3) intrinsic pairs");
        assert_eq!(pairs.len(), 36, "18 cross + 18 intrinsic");

        // Cross pairs always join two *different* feature ids.
        for p in pairs.iter().filter(|p| p.origin == PairOrigin::Cross) {
            assert_ne!(
                p.a.feature_id(),
                p.b.feature_id(),
                "cross pair must span the two operands"
            );
        }
        assert_no_dup_pairs(&pairs);
        assert_canonical(&pairs);
        assert_all_overlaps_valid(&pairs);
    }

    #[test]
    fn far_apart_union_prunes_all_cross_pairs() {
        // Two boxes whose AABBs are disjoint → the AABB prune fires.
        let csg = CsgNode::Union(
            Box::new(box_leaf(1, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0])),
            Box::new(box_leaf(2, [100.0, 0.0, 0.0], [10.0, 10.0, 10.0])),
        );
        let body = BoundedCsg::from_csg(&csg);
        let pairs = enumerate_branch_pairs(&body);

        assert_eq!(count_by_origin(&pairs, PairOrigin::Cross), 0, "prune fires");
        assert_eq!(count_by_origin(&pairs, PairOrigin::Intrinsic), 15 + 15);
        assert_eq!(pairs.len(), 30);
        assert_no_dup_pairs(&pairs);
        assert_all_overlaps_valid(&pairs);
    }

    #[test]
    fn face_touching_union_keeps_cross_pairs() {
        // Centers 20 apart on x, half-extent 10 each → the boxes share exactly
        // one face at x = 10. Inclusive overlap must keep cross pairs.
        let csg = CsgNode::Union(
            Box::new(box_leaf(1, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0])),
            Box::new(box_leaf(2, [20.0, 0.0, 0.0], [10.0, 10.0, 10.0])),
        );
        let body = BoundedCsg::from_csg(&csg);
        let pairs = enumerate_branch_pairs(&body);

        assert!(
            count_by_origin(&pairs, PairOrigin::Cross) > 0,
            "touching faces must yield cross pairs (inclusive overlap)"
        );
        assert_all_overlaps_valid(&pairs);
        assert_canonical(&pairs);
    }

    #[test]
    fn cross_pair_kinds_are_populated() {
        // Box − cylinder: pick a cross pair and verify its kinds match the
        // underlying branches' kinds (box face = Plane; cylinder = Cylinder/Plane).
        let csg = CsgNode::Difference(
            Box::new(box_leaf(1, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0])),
            Box::new(cylinder_leaf(2, [0.0, 0.0, 0.0], 5.0, 12.0)),
        );
        let body = BoundedCsg::from_csg(&csg);
        let pairs = enumerate_branch_pairs(&body);

        // The box-wall × cylinder-wall cross pair: one side Plane, one Cylinder.
        let has_plane_cyl = pairs.iter().any(|p| {
            p.origin == PairOrigin::Cross
                && ((p.kind_a == SurfaceKind::Plane && p.kind_b == SurfaceKind::Cylinder)
                    || (p.kind_a == SurfaceKind::Cylinder && p.kind_b == SurfaceKind::Plane))
        });
        assert!(has_plane_cyl, "expected a Plane×Cylinder cross pair: {pairs:?}");

        // Every cross pair's kinds are non-Tessellated for this analytic body.
        for p in pairs.iter().filter(|p| p.origin == PairOrigin::Cross) {
            assert_ne!(p.kind_a, SurfaceKind::Tessellated);
            assert_ne!(p.kind_b, SurfaceKind::Tessellated);
        }
    }
}
