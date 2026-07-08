//! N0 of `docs/plans/PERSISTENT-NAMING-PLAN.md`: the handle-resolve gate for
//! persistent naming (ARCHITECTURE-ROADMAP Punkt 4).
//!
//! Where `classification_gate` guards the *topology* of the woven B-Rep, this
//! gate guards the *identity* of its elements: a reference taken before an
//! edit must, after the rebuild, either bind to the geometrically same
//! element or degrade honestly (`Ambiguous`/`Missing`) — it must NEVER bind
//! silently to a different element (finding B3 in the plan).
//!
//! Phase N1 (unified candidate scoring + closeness-ambiguity in
//! `naming::resolve`, group-relative sweep epsilon) turned the two known-red
//! scenarios green; all tests are hard guards now.

use super::naming::{
    resolve, EntityHandle, EntityKind, ResolutionState, TopoRef,
};
use super::octree::Octree;
use super::weaving::weave;
use super::{BRep, Edge, EdgeGeometry, FaceId};
use crate::bvh::BoundedCsg;
use crate::csg::{BranchId, CsgNode};
use fielddraft_protocol::{FeatureId, Primitive};

// ─── scenario builders ──────────────────────────────────────────────────────

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

fn weave_octree(body: &BoundedCsg, cell: f32) -> BRep {
    let octree = Octree::build(body, cell);
    weave(body, &octree)
}

/// The face whose fingerprint centroid is nearest to `at`, restricted to
/// faces of `feature` — how the tests pick "the +Y wall of the box" without
/// depending on branch-sub conventions. The feature filter matters: a
/// through-hole's cylinder wall shares its centroid with the pierced cap.
fn face_nearest(brep: &BRep, feature: u32, at: [f64; 3]) -> FaceId {
    let mut best = (f64::MAX, FaceId(0));
    for i in 0..brep.faces.len() {
        let id = FaceId(i as u32);
        if brep.face(id).branch_id.feature_id() != feature {
            continue;
        }
        let c = TopoRef::from_face(brep, id).fingerprint.centroid;
        let d2 = (0..3).map(|k| (c[k] - at[k]).powi(2)).sum::<f64>();
        if d2 < best.0 {
            best = (d2, id);
        }
    }
    best.1
}

// ─── B3: silent misbinding under a spatial-order flip — known red ────────────

/// Two edges of the identical branch signature whose lexicographic
/// centroid order (X → Y → Z, DL-32) flips between two builds: in the old
/// build both centroids share X exactly, so Y decides; in the new build a
/// nanometre X-jitter (well below any real edit, above the 1e-9 sweep
/// epsilon) makes X decide — with the opposite outcome. The old `id_hash`
/// now names the *other* edge, 10 units away.
///
/// `resolve` must bind each stored reference to the geometrically same edge
/// (the fingerprint says so unambiguously); today the hash-hit early path
/// returns the wrong edge with a *Resolved*-band score.
#[test]
fn gate_swap_no_misbind() {
    let a = BranchId(1 << 4);
    let b = BranchId(2 << 4);
    let edge_at = |x: f64, y: f64| {
        Edge::new(
            EdgeGeometry::Polyline(vec![[x, y, 0.0], [x, y, 1.0]]),
            a,
            b,
            None,
            None,
        )
    };

    // Old build: X identical → the Y sweep orders lo (y=−5) before hi (y=+5).
    let mut old = BRep::new();
    let lo_old = old.add_edge(edge_at(0.0, -5.0));
    let hi_old = old.add_edge(edge_at(0.0, 5.0));
    let ref_lo = TopoRef::from_edge(&old, lo_old);
    let ref_hi = TopoRef::from_edge(&old, hi_old);
    assert_ne!(ref_lo.id_hash, ref_hi.id_hash);

    // New build: ±5 nm X-jitter → the X sweep now orders hi before lo, so the
    // spatial indices (and thus the hashes) swap carriers.
    let mut new = BRep::new();
    let lo_new = new.add_edge(edge_at(5e-9, -5.0));
    let hi_new = new.add_edge(edge_at(-5e-9, 5.0));

    let res_lo = resolve(&ref_lo, &new);
    let res_hi = resolve(&ref_hi, &new);
    assert_eq!(
        res_lo.handle,
        Some(EntityHandle::Edge(lo_new)),
        "lo reference must bind to the geometrically same edge, not the swapped hash carrier"
    );
    assert_eq!(
        res_hi.handle,
        Some(EntityHandle::Edge(hi_new)),
        "hi reference must bind to the geometrically same edge, not the swapped hash carrier"
    );
}

// ─── B2: region split — honest ambiguity instead of arbitrary binding ───────

/// A referenced wall is split into two disjoint regions by a crossing
/// feature (F0-1 per-region materialisation): the old reference's centroid
/// sits exactly between the two pieces, so neither is the "same" face. The
/// resolution must say so (`Ambiguous`) — today the surviving
/// `face_id(branch, 0)` hash-hits whichever piece the sweep ordered first
/// and reports a `Resolved`-band score for an arbitrary pick.
#[test]
fn gate_region_split_is_honest() {
    // Long flat box; its +Y wall (y = 3) spans x ∈ [−12, 12], z ∈ [−3, 3].
    let flat = box_leaf(1, [0.0, 0.0, 0.0], [12.0, 3.0, 3.0]);
    let old = weave_octree(&BoundedCsg::from_csg(&flat), 0.5);
    let wall_old = face_nearest(&old, 1, [0.0, 3.0, 0.0]);
    let wall_ref = TopoRef::from_face(&old, wall_old);
    let wall_branch = old.face(wall_old).branch_id;

    // Union a taller crossing box: the wall splits into x < −3 and x > 3
    // pieces with centroids at (∓7.5, 3, 0) — symmetric about the old
    // centroid (0, 3, 0).
    let body = BoundedCsg::from_csg(&CsgNode::Union(
        Box::new(box_leaf(1, [0.0, 0.0, 0.0], [12.0, 3.0, 3.0])),
        Box::new(box_leaf(2, [0.0, 0.0, 0.0], [3.0, 12.0, 4.0])),
    ));
    let new = weave_octree(&body, 0.5);
    let pieces: Vec<FaceId> = (0..new.faces.len())
        .map(|i| FaceId(i as u32))
        .filter(|&id| new.face(id).branch_id == wall_branch)
        .collect();
    assert_eq!(
        pieces.len(),
        2,
        "scenario precondition: the wall must split into two regions"
    );

    let res = resolve(&wall_ref, &new);
    // Whatever it binds to must at least stay on the wall's branch…
    if let Some(EntityHandle::Face(f)) = res.handle {
        assert_eq!(
            new.face(f).branch_id,
            wall_branch,
            "a wall reference must not jump to another branch"
        );
    }
    // …but the pick between two equidistant pieces is arbitrary, and the
    // state must say so instead of reporting confident success.
    assert_eq!(
        res.state,
        ResolutionState::Ambiguous,
        "two symmetric candidates → honest ambiguity, got {:?} (score {:.3})",
        res.state,
        res.score
    );
}

// ─── green guards: identity that already holds and must survive N1 ──────────

/// Drilling a through-hole turns the referenced +Y face into an annulus
/// (the cylinder axis runs along Y): same branch, still a single region →
/// spatial index 0 → the hash is byte-identical and the reference resolves
/// exactly. Inner loops are not part of the signature — pin that.
#[test]
fn gate_ring_face_identity() {
    let solid = box_leaf(1, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0]);
    let old = weave_octree(&BoundedCsg::from_csg(&solid), 0.5);
    let top_old = face_nearest(&old, 1, [0.0, 5.0, 0.0]);
    let top_ref = TopoRef::from_face(&old, top_old);

    let drilled = BoundedCsg::from_csg(&CsgNode::Difference(
        Box::new(box_leaf(1, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0])),
        Box::new(cylinder_leaf(2, [0.0, 0.0, 0.0], 2.5, 6.0)),
    ));
    let new = weave_octree(&drilled, 0.4);
    let top_new = face_nearest(&new, 1, [0.0, 5.0, 0.0]);
    assert!(
        !new.face(top_new).inner.is_empty(),
        "scenario precondition: the top face must have gained an inner loop"
    );

    assert_eq!(
        TopoRef::from_face(&new, top_new).id_hash,
        top_ref.id_hash,
        "an inner loop must not change the face identity"
    );
    let res = resolve(&top_ref, &new);
    assert_eq!(res.state, ResolutionState::Resolved);
    assert_eq!(res.handle, Some(EntityHandle::Face(top_new)));
}

/// A pure parameter edit (resize) keeps every hash byte-identical — the
/// constructive half of the identity (BranchId from feature UUIDs) at work.
/// Mirrors the naming.rs unit tests through the gate lens so N1 cannot
/// regress it.
#[test]
fn gate_resize_exact() {
    let old = weave_octree(
        &BoundedCsg::from_csg(&box_leaf(1, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0])),
        0.5,
    );
    let wall_ref = TopoRef::from_face(&old, face_nearest(&old, 1, [5.0, 0.0, 0.0]));

    let new = weave_octree(
        &BoundedCsg::from_csg(&box_leaf(1, [0.0, 0.0, 0.0], [6.0, 5.0, 5.0])),
        0.5,
    );
    let wall_new = face_nearest(&new, 1, [6.0, 0.0, 0.0]);
    assert_eq!(
        TopoRef::from_face(&new, wall_new).id_hash,
        wall_ref.id_hash,
        "a resize must not change the face identity"
    );
    let res = resolve(&wall_ref, &new);
    assert_eq!(res.state, ResolutionState::Resolved);
    assert_eq!(res.handle, Some(EntityHandle::Face(wall_new)));
}

/// Same-signature entities that are NOT symmetric must keep resolving to
/// their own slot when nothing moved at all — the trivial stability floor
/// for multi-region identity.
#[test]
fn gate_multiregion_self_resolution() {
    let body = BoundedCsg::from_csg(&CsgNode::Union(
        Box::new(box_leaf(1, [0.0, 0.0, 0.0], [12.0, 3.0, 3.0])),
        Box::new(box_leaf(2, [0.0, 0.0, 0.0], [3.0, 12.0, 4.0])),
    ));
    let brep = weave_octree(&body, 0.5);
    let mut multi_seen = false;
    for i in 0..brep.faces.len() {
        let id = FaceId(i as u32);
        let r = TopoRef::from_face(&brep, id);
        let res = resolve(&r, &brep);
        assert_eq!(
            res.handle,
            Some(EntityHandle::Face(id)),
            "self-resolution must return the same slot (face {i})"
        );
        assert_eq!(res.state, ResolutionState::Resolved);
        multi_seen = multi_seen
            || (0..brep.faces.len())
                .filter(|&j| brep.faces[j].branch_id == brep.face(id).branch_id)
                .count()
                > 1;
    }
    assert!(
        multi_seen,
        "scenario precondition: at least one branch must own multiple faces"
    );
    // The same floor holds for edges and vertices.
    for kind in [EntityKind::Edge, EntityKind::Vertex] {
        let n = match kind {
            EntityKind::Edge => brep.edges.len(),
            _ => brep.vertices.len(),
        };
        for i in 0..n {
            let r = match kind {
                EntityKind::Edge => TopoRef::from_edge(&brep, super::EdgeId(i as u32)),
                _ => TopoRef::from_vertex(&brep, super::VertexId(i as u32)),
            };
            let res = resolve(&r, &brep);
            assert_eq!(res.state, ResolutionState::Resolved, "{kind:?} {i}");
        }
    }
}
