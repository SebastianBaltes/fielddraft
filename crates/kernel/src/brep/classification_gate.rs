//! Phase 0 of `docs/BREP-CLASSIFICATION-PLAN.md`: the invariant gate for
//! coincident-geometry weaving (ticket o2j9).
//!
//! Every test asserts the hard invariants of a closed solid on the woven
//! B-Rep — `manifold_violations().is_empty()` and the generalised
//! Euler–Poincaré characteristic `(V + closed_edges) − E + (F − inner_loops)
//! == 2 − 2·genus` — on BOTH weave routes (branch-seed and octree), plus
//! toggle-invariance of the stable element handles between the routes.
//!
//! The naive `V − E + F` undercounts two legal B-Rep constructs: a closed
//! rim edge (a circle is a 1-cell plus an implicit 0-cell, so it must count
//! one extra vertex) and a face with inner loops (an annulus has χ = 0, not
//! 1). The generalised form handles both; for all-disk planar solids it
//! reduces to `V − E + F == 2`. Verified empirically: plain box 2, plain
//! cylinder 2, box-with-through-hole (genus 1) 0.
//!
//! Tests marked `#[ignore]` are **known red** and define "done" for the
//! plan's later phases. Three failure families, discovered while building
//! this gate:
//!
//! - **Coincidence** (the plan's subject, Phases 2/3): flush stacks, partial
//!   coverage, coplanar walls — ghost membranes, duplicated rims, missing
//!   seams.
//! - **F0-1 multi-region branches**: a branch whose boundary contribution is
//!   disconnected (or pierced into an annulus) is still materialised as ONE
//!   face — even fully transversal crossings are non-manifold today, on both
//!   routes identically. No coincidence involved; same attribution stage,
//!   same fix locus (face materialisation must split per region).
//! - **F0-2 curved-face loop orientation**: `order_loops` pushes closed rim
//!   circles with a fixed `reversed = false` for every consumer and leaves
//!   curved-face winding "as ordered", so the fwd/rev manifold convention is
//!   violated by a PLAIN CYLINDER (2 violations). Pre-existing and invisible
//!   until now because mesh/render never read the B-Rep.

use super::naming::TopoRef;
use super::octree::Octree;
use super::seed_weaver::weave_branch_seeds;
use super::weaving::weave;
use super::{BRep, EdgeId, FaceId, VertexId};
use crate::bvh::BoundedCsg;
use crate::csg::CsgNode;
use crate::sketch2d::{Sketch2d, Sketch2dPrimitive};
use fielddraft_protocol::{FeatureId, Primitive, Transform};

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

fn rect_extrude(id: u32, half_extents: [f32; 2], half_height: f32, z_offset: f32) -> CsgNode {
    let extrude = CsgNode::Extrude {
        id: FeatureId::from_u32(id),
        sketch: Box::new(Sketch2d::Prim(Sketch2dPrimitive::Rectangle {
            center: [0.0, 0.0],
            half_extents,
        })),
        half_height,
    };
    if z_offset == 0.0 {
        extrude
    } else {
        CsgNode::Transform(
            Transform { translate: [0.0, 0.0, z_offset], ..Transform::IDENTITY },
            Box::new(extrude),
        )
    }
}

fn union(a: CsgNode, b: CsgNode) -> BoundedCsg {
    BoundedCsg::from_csg(&CsgNode::Union(Box::new(a), Box::new(b)))
}

fn difference(a: CsgNode, b: CsgNode) -> BoundedCsg {
    BoundedCsg::from_csg(&CsgNode::Difference(Box::new(a), Box::new(b)))
}

// ─── invariant assertions ───────────────────────────────────────────────────

/// Sorted multiset of stable element handles — what `qFace`/`qEdge`/`qVertex`
/// address. Mirrors `topo_handles` in the `weaving.rs` tests.
fn handles(brep: &BRep) -> (Vec<u64>, Vec<u64>, Vec<u64>) {
    let mut f: Vec<u64> = (0..brep.faces.len())
        .map(|k| TopoRef::from_face(brep, FaceId(k as u32)).id_hash)
        .collect();
    let mut e: Vec<u64> = (0..brep.edges.len())
        .map(|k| TopoRef::from_edge(brep, EdgeId(k as u32)).id_hash)
        .collect();
    let mut v: Vec<u64> = (0..brep.vertices.len())
        .map(|k| TopoRef::from_vertex(brep, VertexId(k as u32)).id_hash)
        .collect();
    f.sort_unstable();
    e.sort_unstable();
    v.sort_unstable();
    (f, e, v)
}

/// Generalised Euler–Poincaré characteristic: closed-loop edges count their
/// implicit vertex, faces count `1 − inner_loops` (an annulus is χ = 0).
fn euler_poincare(brep: &BRep) -> i64 {
    let closed_edges =
        brep.edges.iter().filter(|e| e.start.is_none() && e.end.is_none()).count();
    let inner_loops: usize = brep.faces.iter().map(|f| f.inner.len()).sum();
    (brep.vertices.len() + closed_edges) as i64 - brep.edges.len() as i64
        + brep.faces.len() as i64
        - inner_loops as i64
}

/// The hard invariants of a closed solid of the given genus, plus referential
/// integrity. `label` names route + scenario in failure output.
fn assert_solid_invariants(brep: &BRep, genus: i64, label: &str) {
    brep.validate()
        .unwrap_or_else(|errs| panic!("{label}: referential integrity broken: {errs:?}"));
    let violations = brep.manifold_violations();
    assert!(
        violations.is_empty(),
        "{label}: {} manifold violation(s), e.g. {:?}",
        violations.len(),
        violations.iter().take(4).collect::<Vec<_>>()
    );
    assert_eq!(
        euler_poincare(brep),
        2 - 2 * genus,
        "{label}: Euler–Poincaré must be {} (closed genus-{genus} solid); \
         V={} E={} F={} closed_edges={} inner_loops={}",
        2 - 2 * genus,
        brep.vertices.len(),
        brep.edges.len(),
        brep.faces.len(),
        brep.edges.iter().filter(|e| e.start.is_none()).count(),
        brep.faces.iter().map(|f| f.inner.len()).sum::<usize>()
    );
}

/// Run the full gate on one scenario: both routes individually sound, and
/// byte-identical in element handles (the `id_hash` toggle-invariance from
/// tickets oa9h/vyee).
fn assert_gate(body: &BoundedCsg, cell: f32, genus: i64, label: &str) {
    let seed = weave_branch_seeds(body)
        .unwrap_or_else(|| panic!("{label}: body must be seed-weavable"));
    assert_solid_invariants(&seed, genus, &format!("{label} [seed route]"));

    let octree = Octree::build(body, cell);
    let sdf = weave(body, &octree);
    assert_solid_invariants(&sdf, genus, &format!("{label} [octree route]"));

    assert_eq!(
        handles(&seed),
        handles(&sdf),
        "{label}: seed and octree weave must yield identical element handles"
    );
}

// ─── generic scenarios — green, regression guards ───────────────────────────

#[test]
fn gate_plain_box() {
    let body = BoundedCsg::from_csg(&box_leaf(1, [0.0; 3], [5.0, 5.0, 5.0]));
    assert_gate(&body, 0.5, 0, "plain box");
}

#[test]
fn gate_union_overlapping_boxes() {
    // Generic transversal overlap: every face pair meets at an angle.
    let body = union(
        box_leaf(1, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
        box_leaf(2, [4.0, 3.0, 2.0], [5.0, 5.0, 5.0]),
    );
    assert_gate(&body, 0.5, 0, "overlapping boxes");
}

// ─── F0-2: curved-face loop orientation — FIXED, green guards ────────────────

#[test]
fn gate_plain_cylinder() {
    // F0-2 regression guard: a lone cylinder used to violate the manifold
    // orientation convention (both rim circles consumed `reversed = false` by
    // wall AND cap). Fixed by winding enforcement in `order_loops` — sampled
    // ring areas + uv winding metrics + complement parity.
    let body = BoundedCsg::from_csg(&cylinder_leaf(1, [0.0; 3], 5.0, 4.0));
    assert_gate(&body, 0.4, 0, "plain cylinder");
}

#[test]
fn gate_remove_through_hole_genus1() {
    // F0-2 regression guard: box with a cylindrical through-hole — genus 1,
    // Euler–Poincaré 0. Exercises rim orientation AND complement parity (the
    // subtracted cylinder's wall faces the body with its primitive normal
    // flipped).
    let body = difference(
        box_leaf(1, [0.0; 3], [5.0, 5.0, 5.0]),
        cylinder_leaf(2, [0.0; 3], 2.5, 6.0),
    );
    assert_gate(&body, 0.4, 1, "box with through-hole");
}

// ─── F0-1: multi-region branches — known red, NOT coincidence ────────────────

#[test]
fn gate_union_crossing_boxes_transversal() {
    // A plus sign with DIFFERENT heights: fully transversal, no coplanarity
    // anywhere — and still non-manifold on both routes (Euler 0, 12
    // orientation violations). The taller box splits the flatter box's side
    // walls into two disconnected rectangles and is itself pierced into
    // annulus walls; one-face-per-branch cannot represent either. This is the
    // totality/injectivity failure of branch→face attribution WITHOUT any
    // coincidence — same fix locus as Phase 3 (per-region face
    // materialisation), documented as F0-1 in the plan.
    let body = union(
        box_leaf(1, [0.0, 0.0, 0.0], [12.0, 3.0, 3.0]),
        box_leaf(2, [0.0, 0.0, 0.0], [3.0, 12.0, 4.0]),
    );
    assert_gate(&body, 0.5, 0, "crossing boxes (transversal)");
}

// ─── coincidence scenarios — known red until Phases 2/3 land ────────────────

#[test]
#[ignore = "BREP-CLASSIFICATION-PLAN Phase 3 — koplanare ON-same-Wände (Naht-Kante), bekannt rot"]
fn gate_union_crossing_boxes_coplanar_tops() {
    // A plus sign with EQUAL heights: the side walls cross transversally, but
    // the top/bottom faces of both boxes are coplanar same-orientation
    // (ON-same) — they must survive as separate faces joined by seam edges.
    // Combines F0-1 with the coincidence classification.
    let body = union(
        box_leaf(1, [0.0, 0.0, 0.0], [12.0, 3.0, 3.0]),
        box_leaf(2, [0.0, 0.0, 0.0], [3.0, 12.0, 3.0]),
    );
    assert_gate(&body, 0.5, 0, "crossing boxes (coplanar tops)");
}

#[test]
fn gate_union_flush_boxes_same_cross_section() {
    // The o2j9 headline: two boxes, identical cross-section, stacked flush at
    // z = 5. Expected solid: a single 10×10×20 box with a seam ring (Euler 2).
    // Today: ghost membranes + duplicated rims → Euler −16.
    let body = union(
        box_leaf(1, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
        box_leaf(2, [0.0, 0.0, 10.0], [5.0, 5.0, 5.0]),
    );
    assert_gate(&body, 0.5, 0, "flush boxes, same cross-section");
}

#[test]
fn gate_union_flush_smaller_box_on_larger() {
    // Partial coverage: a small block sitting flush on a big one. The big
    // box's +Z face must survive as a ring face (inner seam loop).
    let body = union(
        box_leaf(1, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
        box_leaf(2, [0.0, 0.0, 7.5], [2.5, 2.5, 2.5]),
    );
    assert_gate(&body, 0.25, 0, "flush smaller box on larger");
}

#[test]
fn gate_union_coplanar_extrude_stack_small() {
    // The o2j9 user scenario at sketch scale: two rectangle extrudes stacked
    // flush along Z.
    let body = union(
        rect_extrude(1, [5.0, 4.0], 2.0, 0.0),
        rect_extrude(2, [5.0, 4.0], 2.0, 4.0),
    );
    assert_gate(&body, 0.25, 0, "coplanar extrude stack (small)");
}

#[test]
fn gate_union_coplanar_extrude_stack_large() {
    // The same stack at ≳100 mm: guards the absolute-epsilon scale fragility
    // from tickets oa9h/vyee alongside the coincidence fix.
    let body = union(
        rect_extrude(1, [60.0, 50.0], 25.0, 0.0),
        rect_extrude(2, [60.0, 50.0], 25.0, 50.0),
    );
    assert_gate(&body, 3.0, 0, "coplanar extrude stack (large)");
}

#[test]
fn gate_union_coaxial_cylinder_stack_seed_route() {
    // The cylinder twin of the flush-box case (shaft, boss-on-boss): equal
    // radius, coaxial, stacked flush. Coincident *cylinder* walls — the
    // registry is surface-generic, not plane-only. Green on the production
    // (branch-seed) route: membrane caps culled, the seam rim merged into
    // one closed edge joining both wall faces.
    let body = union(
        cylinder_leaf(1, [0.0, 0.0, 0.0], 5.0, 4.0),
        cylinder_leaf(2, [0.0, 8.0, 0.0], 5.0, 4.0),
    );
    let seed = weave_branch_seeds(&body).expect("cylinder stack is seed-weavable");
    assert_solid_invariants(&seed, 0, "coaxial cylinder stack [seed route]");
}

#[test]
fn gate_union_coaxial_cylinder_stack_octree_route() {
    // Phase 4 regression guard (green since the class-aware octree
    // classification): the octree route used to fragment the coincident
    // seam rim into short arcs (the oa9h/vyee contributor smear — plane
    // seams were saved by the DL-75 analytic fast-path, cylinder seams were
    // not). Cell classification now counts coincidence CLASSES, so the
    // shared wall is one carrier (Face cells) and the rim counts exactly
    // two carriers (Edge cells).
    let body = union(
        cylinder_leaf(1, [0.0, 0.0, 0.0], 5.0, 4.0),
        cylinder_leaf(2, [0.0, 8.0, 0.0], 5.0, 4.0),
    );
    assert_gate(&body, 0.4, 0, "coaxial cylinder stack");
}

#[test]
fn gate_union_flush_extrude_covers_box_face() {
    // The ticket's intermediate body, idealised: a box leaf whose face is
    // fully covered by a flush face-extrude prism of identical cross-section.
    // All four prism walls are coplanar with box side faces, the contact
    // plane is an ON-opposite membrane pair. Both routes pass here because
    // the axis-aligned Rectangle prim keeps the octree's DL-75 analytic
    // fast-path alive; the rotated Path variants below do not (Phase 4).
    let body = union(
        box_leaf(1, [0.0, 0.0, 0.0], [50.0, 50.0, 50.0]),
        rect_extrude(5, [50.0, 50.0], 26.0, 76.0),
    );
    assert_gate(&body, 3.0, 0, "flush extrude covers box face");
}

/// The ticket's REAL feature 5, byte-exact from the decoded spine: a
/// face-extrude on the box's +Y face whose sketch is a closed 4-line PATH
/// (not a Rectangle prim), placed by a rotation quat. Exactly coincident
/// everywhere (the anchor fix).
fn ticket_extrude5() -> CsgNode {
    let path = Sketch2d::Prim(Sketch2dPrimitive::Path {
        segments: vec![
            fielddraft_protocol::SketchSegment::Line { start: [-50.0, -50.0], end: [50.0, -50.0] },
            fielddraft_protocol::SketchSegment::Line { start: [50.0, -50.0], end: [50.0, 50.0] },
            fielddraft_protocol::SketchSegment::Line { start: [50.0, 50.0], end: [-50.0, 50.0] },
            fielddraft_protocol::SketchSegment::Line { start: [-50.0, 50.0], end: [-50.0, -50.0] },
        ],
        closed: true,
    });
    let extrude = CsgNode::Extrude {
        id: FeatureId::from_u32(5),
        sketch: Box::new(path),
        half_height: 26.456285,
    };
    let inner = CsgNode::Transform(
        Transform { translate: [0.0, 0.0, 26.456285], ..Transform::IDENTITY },
        Box::new(extrude),
    );
    CsgNode::Transform(
        Transform {
            translate: [0.0, 50.0, 0.0],
            rotation: [0.5, 0.5, 0.5, -0.5],
            scale: [1.0, 1.0, 1.0],
        },
        Box::new(inner),
    )
}

/// The ticket's REAL feature 17: extruded from prism5's +X face; the carrier
/// plane is exact, but the in-plane profile coords carry the mesh-derived
/// µm drift (50.00049 / −50.0009 / 26.457184 / −26.456078).
fn ticket_extrude17() -> CsgNode {
    let path = Sketch2d::Prim(Sketch2dPrimitive::Path {
        segments: vec![
            fielddraft_protocol::SketchSegment::Line { start: [50.00049, 26.457184], end: [-50.0009, 26.457184] },
            fielddraft_protocol::SketchSegment::Line { start: [-50.0009, 26.457184], end: [-50.0008, -26.456078] },
            fielddraft_protocol::SketchSegment::Line { start: [-50.0008, -26.456078], end: [50.00039, -26.456078] },
            fielddraft_protocol::SketchSegment::Line { start: [50.00039, -26.456078], end: [50.00049, 26.457184] },
        ],
        closed: true,
    });
    let extrude = CsgNode::Extrude {
        id: FeatureId::from_u32(17),
        sketch: Box::new(path),
        half_height: 49.372684,
    };
    let inner = CsgNode::Transform(
        Transform { translate: [0.0, 0.0, 49.372684], ..Transform::IDENTITY },
        Box::new(extrude),
    );
    CsgNode::Transform(
        Transform {
            translate: [50.0, 76.45608, -0.00020685856],
            rotation: [0.0, 0.70710677, 0.0, 0.70710677],
            scale: [1.0, 1.0, 1.0],
        },
        Box::new(inner),
    )
}

#[test]
fn gate_ticket_o2j9_box_plus_extrude5_seed_route() {
    // The ticket's first intermediate body (box + face-extrude 5): rotated
    // placement, Path sketch, exact coincidence on all four walls + membrane.
    // Production (seed) route must weave it manifold; the octree route smears
    // at the rotated coincident walls (Phase 4, like the cylinder stack).
    let body = union(box_leaf(1, [0.0, 0.0, 0.0], [50.0, 50.0, 50.0]), ticket_extrude5());
    let seed = weave_branch_seeds(&body).expect("ticket box+extrude5 is seed-weavable");
    assert_solid_invariants(&seed, 0, "ticket box+extrude5 [seed route]");
}

/// The ticket's REAL feature 18 (the reported two-face extrude): ONE extrude
/// feature over the UNION of two mesh-derived Path profiles — the box's +Z
/// face and prism17's +Z wall, both in the carrier plane z = 50 — which abut
/// along a shared, µm-drifted edge at sketch-y ≈ 50. The union's internal
/// seam yields two near-coincident ON-opposite wall groups WITHIN the same
/// feature.
fn ticket_extrude18() -> CsgNode {
    use fielddraft_protocol::SketchSegment::Line;
    let path_a = Sketch2d::Prim(Sketch2dPrimitive::Path {
        segments: vec![
            Line { start: [-26.456581, 50.0002], end: [-26.457207, -50.00069] },
            Line { start: [-26.457207, -50.00069], end: [26.456264, -50.000565] },
            Line { start: [26.456264, -50.000565], end: [26.456078, 50.000515] },
            Line { start: [26.456078, 50.000515], end: [-26.456581, 50.0002] },
        ],
        closed: true,
    });
    let path_b = Sketch2d::Prim(Sketch2dPrimitive::Path {
        segments: vec![
            Line { start: [-26.457901, 148.74646], end: [-26.456581, 50.0002] },
            Line { start: [-26.456581, 50.0002], end: [26.456078, 50.000515] },
            Line { start: [26.456078, 50.000515], end: [26.457157, 148.74646] },
            Line { start: [26.457157, 148.74646], end: [-26.457901, 148.74646] },
        ],
        closed: true,
    });
    let extrude = CsgNode::Extrude {
        id: FeatureId::from_u32(18),
        sketch: Box::new(Sketch2d::Union(Box::new(path_a), Box::new(path_b))),
        half_height: 12.5,
    };
    let inner = CsgNode::Transform(
        Transform { translate: [0.0, 0.0, 12.5], ..Transform::IDENTITY },
        Box::new(extrude),
    );
    CsgNode::Transform(
        Transform {
            translate: [-0.00020580407, 76.45626, 50.0],
            rotation: [0.0, 0.0, -0.70710677, 0.70710677],
            scale: [1.0, 1.0, 1.0],
        },
        Box::new(inner),
    )
}

#[test]
fn gate_ticket_o2j9_full_two_face_extrude_seed_route() {
    // The ticket's full 4-feature body: deep coincident stack + the two-face
    // extrude whose union profile carries an internal µm-drifted seam.
    let body = BoundedCsg::from_csg(&CsgNode::Union(
        Box::new(CsgNode::Union(
            Box::new(CsgNode::Union(
                Box::new(box_leaf(1, [0.0, 0.0, 0.0], [50.0, 50.0, 50.0])),
                Box::new(ticket_extrude5()),
            )),
            Box::new(ticket_extrude17()),
        )),
        Box::new(ticket_extrude18()),
    ));
    let seed = weave_branch_seeds(&body).expect("ticket 4-feature body is seed-weavable");
    assert_solid_invariants(&seed, 0, "ticket full two-face extrude [seed route]");
}

#[test]
fn gate_ticket_o2j9_box_plus_extrude5_plus_extrude17_seed_route() {
    // The ticket's full loaded body. Two regression roots live here:
    // (a) prism17's walls carry µm in-plane profile drift, so wall pairs
    //     against prism5/box are near-coincident — without the registry gate
    //     in `seed_branch_pairs` SSI manufactures a µrad-angle "intersection
    //     line" from the drift noise;
    // (b) the body's concave corner line (box∪prism5∪prism17 leave one empty
    //     quadrant) extends to infinity with both wall branches still
    //     reported as nearest-feature contributors — without the |f| ≤ eps
    //     on-boundary gate in `both_branches_active` the seam march runs to
    //     `max_arc` and bails the whole body to the octree (the reported
    //     E=0/F=24 ghost-face collapse).
    let body = BoundedCsg::from_csg(&CsgNode::Union(
        Box::new(CsgNode::Union(
            Box::new(box_leaf(1, [0.0, 0.0, 0.0], [50.0, 50.0, 50.0])),
            Box::new(ticket_extrude5()),
        )),
        Box::new(ticket_extrude17()),
    ));
    let seed = weave_branch_seeds(&body).expect("ticket 3-feature body is seed-weavable");
    assert_solid_invariants(&seed, 0, "ticket box+extrude5+extrude17 [seed route]");
}

#[test]
fn gate_remove_flush_zero_thickness_wall() {
    // Subtraction whose tool face coincides with the body face at x = 5: the
    // wall between them has exactly zero thickness and must vanish entirely.
    // Expected solid: the box x ∈ [−5, 0] — six faces, Euler 2, no membrane.
    let body = difference(
        box_leaf(1, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
        box_leaf(2, [2.5, 0.0, 0.0], [2.5, 6.0, 6.0]),
    );
    assert_gate(&body, 0.5, 0, "flush remove, zero-thickness wall");
}

// ─── kissing contact — policy test, documents behaviour ─────────────────────

#[test]
fn gate_union_edge_touching_boxes_policy() {
    // Two boxes sharing exactly one edge (x = 5, y = 5). The union is
    // topologically non-manifold at that edge — no weave can make it a valid
    // 2-manifold; this is geometry, not a pipeline weakness. Policy (see plan,
    // Nicht-Ziele): the weave must complete with intact referential integrity;
    // `manifold_violations()` is allowed to name the contact. No invariant
    // assert on Euler/manifoldness — this test documents the contract that
    // kissing contact never panics or corrupts the arena.
    let body = union(
        box_leaf(1, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
        box_leaf(2, [10.0, 10.0, 0.0], [5.0, 5.0, 5.0]),
    );
    let octree = Octree::build(&body, 0.5);
    let sdf = weave(&body, &octree);
    sdf.validate()
        .unwrap_or_else(|errs| panic!("kissing contact [octree]: integrity broken: {errs:?}"));
    if let Some(seed) = weave_branch_seeds(&body) {
        seed.validate()
            .unwrap_or_else(|errs| panic!("kissing contact [seed]: integrity broken: {errs:?}"));
    }
}
