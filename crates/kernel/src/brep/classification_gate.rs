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
//! - **F0-2 curved-face loop orientation** (FIXED): loop ordering used to
//!   push closed rim circles with a fixed `reversed = false` for every
//!   consumer and leave curved-face winding "as ordered", so the fwd/rev
//!   manifold convention was violated by a PLAIN CYLINDER (2 violations).
//!   Fixed by winding enforcement in `order_loop_groups` (sampled ring
//!   areas + uv winding metrics + complement parity); `gate_plain_cylinder`
//!   below is the regression guard.

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
    let extrude = CsgNode::Extrude { wall_subs: None,
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
    // wall AND cap). Fixed by winding enforcement in `order_loop_groups` — sampled
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

// ─── order-1 (tangential) contact — symbolic tangent circles ────────────────

/// The canonical order-1 case: a cylinder with a coaxial spherical cap of
/// EQUAL radius — wall and sphere meet G1 along the equator circle. The
/// tracer degenerates there (∇f_a × ∇f_b → 0 along the whole seam; the
/// octree route used to shatter into a 241-violation garbage star), so the
/// seam is materialised SYMBOLICALLY from the registry's tangent-circle
/// detection. Expected: bottom cap + wall + sphere cap, two closed rims
/// (bottom rim + tangent circle), no interior ghost cap — a closed genus-0
/// solid on BOTH routes.
#[test]
fn gate_tangent_sphere_capped_cylinder() {
    let body = union(
        cylinder_leaf(1, [0.0, 0.0, 0.0], 5.0, 5.0),
        CsgNode::Leaf {
            id: FeatureId::from_u32(2),
            primitive: Primitive::Sphere { center: [0.0, 5.0, 0.0], radius: 5.0 },
        },
    );
    assert_gate(&body, 0.4, 0, "sphere-capped cylinder (tangent equator)");
}

/// Inscribed sphere in a cone (the ice-cream): the sphere touches the cone
/// wall along a circle. Symbolic order-1 seam, both routes.
#[test]
fn gate_tangent_sphere_in_cone() {
    // Frustum r_bottom=6, r_top=2, hh=4 → slope tan α = 0.5, apex 4 above
    // the top rim (frustum centre-local apex at y = 8). Sphere centred on
    // the axis at distance d below the apex with R = d·sin α: sin α =
    // 0.5/√1.25 ≈ 0.4472; pick d = 6.7082 → R = 3.0. Apex world y = 8 →
    // centre y = 8 − 6.7082 = 1.2918.
    let body = union(
        CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Cone { r_bottom: 6.0, r_top: 2.0, half_height: 4.0 },
        },
        CsgNode::Leaf {
            id: FeatureId::from_u32(2),
            primitive: Primitive::Sphere { center: [0.0, 1.2918, 0.0], radius: 3.0 },
        },
    );
    // Premise: the registry must actually detect the order-1 contact —
    // otherwise this test would silently exercise nothing.
    let registry = crate::brep::coincidence::CoincidenceRegistry::build(&body);
    assert_eq!(
        registry.tangent_seams().len(),
        1,
        "sphere-in-cone must register exactly one tangent circle"
    );
    let brep = weave_branch_seeds(&body).expect("seed weavable");
    assert_solid_invariants(&brep, 0, "sphere in cone (tangent circle) [seed route]");
}

/// A frustum whose inscribed sphere pokes out through the BOTTOM CAP: the
/// sphere grazes the wall tangentially but never crosses it (it exits
/// through the cap), so the tangent circle is NOT a boundary seam — the
/// wall face runs through it smoothly. The registry detects the geometric
/// tangency; the boundary gate's contributor role-swap test must reject it,
/// leaving a clean transversal weave (wall + caps + sphere bulge).
#[test]
fn gate_tangent_graze_through_cap_is_no_seam() {
    let body = union(
        CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Cone { r_bottom: 1.0, r_top: 3.0, half_height: 4.0 },
        },
        CsgNode::Leaf {
            id: FeatureId::from_u32(2),
            primitive: Primitive::Sphere { center: [0.0, -3.258, 0.0], radius: 1.15 },
        },
    );
    let registry = crate::brep::coincidence::CoincidenceRegistry::build(&body);
    assert_eq!(registry.tangent_seams().len(), 1, "the graze is geometrically tangent");
    let brep = weave_branch_seeds(&body).expect("seed weavable");
    assert_solid_invariants(&brep, 0, "grazing sphere through cap [seed route]");
    // No symbolic tangent circle at the graze height — the wall is smooth.
    assert!(
        !brep.edges.iter().any(|e| {
            e.start.is_none()
                && e.end.is_none()
                && crate::brep::edge_sampling::sample_edge(&e.geometry)
                    .iter()
                    .all(|p| (p[1] - (-3.537)).abs() < 0.01)
        }),
        "a grazing tangency must not materialise a seam edge"
    );
}

/// Rounded cone tip, the TRUE seam flavour: the frustum's top rim IS the
/// tangent circle (`r_top = R·cos α`, sphere centre `R·sin α` above the
/// cap) — below the circle the wall bounds the body, above it the sphere
/// does. Mirrors the sphere-capped cylinder on a sloped wall: the ghost
/// top cap loses both its coincident circles to the symbolic seam and
/// never materialises.
#[test]
fn gate_tangent_rounded_cone_tip() {
    // slope 0.25 → sin α ≈ 0.2425; apex at y ≈ 8.4627. Tangency needs the
    // centre at apex-distance d = R/sin α ≈ 4.7415 → centre y ≈ 3.7212; the
    // contact circle then sits at apex − d·cos²α ≈ 4.0 — exactly the rim.
    let body = union(
        CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Cone {
                r_bottom: 3.115_617,
                r_top: 1.115_664,
                half_height: 4.0,
            },
        },
        CsgNode::Leaf {
            id: FeatureId::from_u32(2),
            primitive: Primitive::Sphere { center: [0.0, 3.721_2, 0.0], radius: 1.15 },
        },
    );
    let registry = crate::brep::coincidence::CoincidenceRegistry::build(&body);
    assert_eq!(
        registry.tangent_seams().len(),
        1,
        "rounded cone tip must register exactly one tangent circle"
    );
    let brep = weave_branch_seeds(&body).expect("seed weavable");
    assert_solid_invariants(&brep, 0, "rounded cone tip (tangent circle) [seed route]");
    // The symbolic seam exists: a closed cross-feature circle at the rim.
    assert!(
        brep.edges.iter().any(|e| {
            e.signature.0.feature_id() != e.signature.1.feature_id()
                && e.start.is_none()
                && e.end.is_none()
                && crate::brep::edge_sampling::sample_edge(&e.geometry)
                    .iter()
                    .all(|p| (p[1] - 4.0).abs() < 0.01)
        }),
        "the tangent circle at the rim (y = 4) must exist"
    );
}

/// The vase stack — the CONSTRUCTED profile-G1 case from the roadmap: a
/// cylindrical pot (revolve 1, straight wall → analytic cylinder lift) and
/// an arc dome (revolve 2, curved wall group → `RevolvedProfile`) whose
/// profiles share the endpoint (r=5, y=0) with an equal (vertical) end
/// tangent. The junction circle is detected on the 2D construction data
/// (profile chain ends) and materialised symbolically; the flush interface
/// discs are ON-opposite membranes and cull as usual.
#[test]
fn gate_tangent_vase_stack_profile_g1() {
    use fielddraft_protocol::{Sketch2d, Sketch2dPrimitive, SketchSegment};
    let path = |segments: Vec<SketchSegment>| {
        Sketch2d::Prim(Sketch2dPrimitive::Path { segments, closed: true })
    };
    let line = |a: [f32; 2], b: [f32; 2]| SketchSegment::Line { start: a, end: b };
    // Pot: rectangle r ∈ [0, 5], y ∈ [−4, 0].
    let pot = CsgNode::Revolve { wall_subs: None,
        id: FeatureId::from_u32(1),
        sketch: Box::new(path(vec![
            line([0.0, -4.0], [5.0, -4.0]),
            line([5.0, -4.0], [5.0, 0.0]),
            line([5.0, 0.0], [0.0, 0.0]),
            line([0.0, 0.0], [0.0, -4.0]),
        ])),
    };
    // Dome: bottom flat to (5, 0), then a 90° arc (centre (3, 0), R = 2)
    // starting with a VERTICAL tangent — G1 against the pot wall — up to
    // (3, 2), closed by the top flat and the axis.
    let dome = CsgNode::Revolve { wall_subs: None,
        id: FeatureId::from_u32(2),
        sketch: Box::new(path(vec![
            line([0.0, 0.0], [5.0, 0.0]),
            SketchSegment::Arc {
                center: [3.0, 0.0],
                radius: 2.0,
                start_angle: 0.0,
                sweep_angle: std::f32::consts::FRAC_PI_2,
            },
            line([3.0, 2.0], [0.0, 2.0]),
            line([0.0, 2.0], [0.0, 0.0]),
        ])),
    };
    let body = BoundedCsg::from_csg(&CsgNode::Union(Box::new(pot), Box::new(dome)));

    let registry = crate::brep::coincidence::CoincidenceRegistry::build(&body);
    assert_eq!(
        registry.tangent_seams().len(),
        1,
        "the profile-G1 junction must register exactly one tangent circle"
    );

    let brep = weave_branch_seeds(&body).expect("seed weavable");
    assert_solid_invariants(&brep, 0, "vase stack (profile G1) [seed route]");
    // The junction circle at (r = 5, y = 0) exists as a closed
    // cross-feature seam.
    assert!(
        brep.edges.iter().any(|e| {
            e.signature.0.feature_id() != e.signature.1.feature_id()
                && e.start.is_none()
                && e.end.is_none()
                && crate::brep::edge_sampling::sample_edge(&e.geometry).iter().all(|p| {
                    let r = (p[0] * p[0] + p[2] * p[2]).sqrt();
                    (r - 5.0).abs() < 0.01 && p[1].abs() < 0.01
                })
        }),
        "the G1 junction circle (r = 5, y = 0) must exist"
    );
}

// ─── coincidence scenarios — known red until Phases 2/3 land ────────────────


#[test]
fn gate_union_crossing_boxes_coplanar_tops() {
    // A plus sign with EQUAL heights: the side walls cross transversally, but
    // the top/bottom faces of both boxes are coplanar same-orientation
    // (ON-same) — they must survive as separate faces joined by seam edges.
    // Combines F0-1 with the coincidence classification. Fixed by the
    // overlap-aware membership re-attribution
    // (`reattribute_coincident_memberships`): the overlap square belongs to
    // the class representative, buried signature branches lose the edge,
    // interior seams inside the overlap are dropped.
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
    let extrude = CsgNode::Extrude { wall_subs: None,
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
    let extrude = CsgNode::Extrude { wall_subs: None,
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
    let extrude = CsgNode::Extrude { wall_subs: None,
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
