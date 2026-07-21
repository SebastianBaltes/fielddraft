//! Tests der offset_tree-Pipeline-Invarianten (#384, Follow-up 2).
//!
//! Je Invariante: (a) ein POSITIV-Test über einen echten Pipeline-Lauf (realer
//! Baum durch die Pässe → alle Checks ok) und (b) ein NEGATIV-Test, der einen
//! von Hand konstruierten verletzenden Baum direkt an die Checker-Funktion gibt
//! und die Fehlermeldung prüft (die Checker sind pure Funktionen — kein
//! `should_panic` nötig).

use super::*;

use fielddraft_protocol::{FeatureId, HalfSpace, Primitive, Transform};

use crate::csg::{BranchId, CsgNode, OpenSurface};
use crate::sketch2d::{Sketch2d, Sketch2dPrimitive, SketchSegment};

fn id(n: u32) -> FeatureId {
    FeatureId::from_u32(n)
}

fn box_leaf(fid: u32, he: [f32; 3]) -> CsgNode {
    CsgNode::Leaf {
        id: id(fid),
        primitive: Primitive::Box { center: [0.0; 3], half_extents: he },
    }
}

fn sphere_leaf(fid: u32, r: f32) -> CsgNode {
    CsgNode::Leaf {
        id: id(fid),
        primitive: Primitive::Sphere { center: [0.0; 3], radius: r },
    }
}

/// Ein geschlossenes Extrude-Profil mit QuadBezier-Segment — die Familie, die
/// `fold_offset_leaves` bewusst VERWEIGERT (kein arc-exakter Bézier-Offset).
fn quad_bezier_extrude(fid: u32) -> CsgNode {
    CsgNode::Extrude {
        id: id(fid),
        sketch: Box::new(Sketch2d::Prim(Sketch2dPrimitive::Path {
            segments: vec![
                SketchSegment::Line { start: [0.0, 0.0], end: [10.0, 0.0] },
                SketchSegment::Line { start: [10.0, 0.0], end: [5.0, 10.0] },
                SketchSegment::QuadBezier { p0: [5.0, 10.0], p1: [0.0, 10.0], p2: [0.0, 0.0] },
            ],
            closed: true,
        })),
        half_height: 5.0,
        wall_subs: None,
    }
}

// ─── Invariante 1: kein geschlossener Shell nach expand_shells ────────────

#[test]
fn pos_closed_shell_pipeline_satisfies_all_invariants() {
    let node = CsgNode::Shell(Box::new(box_leaf(1, [20.0, 20.0, 20.0])), 2.0, Vec::new());

    let after_expand = node.expand_shells();
    assert!(check_no_closed_shells(&after_expand).is_ok());
    assert!(check_inner_wall_position(&after_expand).is_ok());
    assert_after_expand_shells(&after_expand); // Wire-Helfer paniken nicht

    let after_pd = after_expand.push_down_offsets();
    assert!(check_offsets_pushed(&after_pd).is_ok());
    assert!(check_inner_wall_position(&after_pd).is_ok());
    assert_after_push_down_offsets(&after_pd);

    let after_fold = after_pd.fold_offset_leaves();
    assert!(check_offset_leaves_folded(&after_fold).is_ok());
    assert!(check_offsets_pushed(&after_fold).is_ok());
    assert!(check_inner_wall_position(&after_fold).is_ok());
    assert_after_fold_offset_leaves(&after_fold);
}

#[test]
fn pos_open_shell_is_not_flagged_closed() {
    let open = CsgNode::Shell(
        Box::new(box_leaf(1, [10.0, 10.0, 10.0])),
        1.0,
        vec![OpenSurface {
            branch: BranchId::from_feature_sub(&id(1), 4).as_u32(),
            surface: HalfSpace::Plane { normal: [0.0, 0.0, 1.0], offset: 10.0 },
            placement: Transform::IDENTITY,
        }],
    );
    // Offene Shell bleibt auf dem Feld-Pfad — keine Verletzung.
    assert!(check_no_closed_shells(&open.clone().expand_shells()).is_ok());
}

#[test]
fn neg_closed_shell_is_flagged() {
    let bad = CsgNode::Union(
        Box::new(box_leaf(1, [10.0, 10.0, 10.0])),
        Box::new(CsgNode::Shell(Box::new(box_leaf(2, [5.0, 5.0, 5.0])), 1.0, Vec::new())),
    );
    let err = check_no_closed_shells(&bad).unwrap_err();
    assert!(err.contains("geschlossener Shell"), "message: {err}");
    assert!(err.contains("Union.r"), "path should point at the shell: {err}");
}

// ─── Invariante 2: keine pushbaren Offsets nach push_down_offsets ─────────

#[test]
fn pos_offset_difference_pipeline_leaves_no_pushable_offset() {
    // Offset(Difference(box, sphere), −1): Erosion schiebt in die Difference
    // (erode(A) ∖ dilate(B)); die Blatt-Offsets bleiben als Stopps stehen.
    let tree = CsgNode::Offset(
        Box::new(CsgNode::Difference(
            Box::new(box_leaf(1, [20.0, 20.0, 20.0])),
            Box::new(sphere_leaf(2, 5.0)),
        )),
        -1.0,
    );
    let after_pd = tree.push_down_offsets();
    assert!(
        check_offsets_pushed(&after_pd).is_ok(),
        "residual: {:?}",
        check_offsets_pushed(&after_pd)
    );
    let after_fold = after_pd.fold_offset_leaves();
    assert!(check_offsets_pushed(&after_fold).is_ok());
    assert!(check_offset_leaves_folded(&after_fold).is_ok());
}

#[test]
fn neg_dilation_over_union_is_flagged() {
    // Dilation (δ>0) über Union IST pushbar → ein stehender Offset ist ein Bug.
    let bad = CsgNode::Offset(
        Box::new(CsgNode::Union(
            Box::new(box_leaf(1, [10.0, 10.0, 10.0])),
            Box::new(box_leaf(2, [10.0, 10.0, 10.0])),
        )),
        1.0,
    );
    let err = check_offsets_pushed(&bad).unwrap_err();
    assert!(err.contains("push_down_offsets"), "message: {err}");
    assert!(err.contains("Union"), "message: {err}");
}

#[test]
fn neg_offset_over_rigid_transform_is_flagged() {
    // Rigide Transform (unit scale) → pushbar.
    let bad = CsgNode::Offset(
        Box::new(CsgNode::Transform(
            Transform::IDENTITY,
            Box::new(box_leaf(1, [10.0, 10.0, 10.0])),
        )),
        1.0,
    );
    assert!(check_offsets_pushed(&bad).is_err());
}

// ─── Invariante 3: verbleibende Blatt-Offsets sind echte Verweigerungen ───

#[test]
fn pos_genuine_fold_refusal_satisfies_invariant3() {
    // QuadBezier-Extrude: der Fold verweigert → Offset(Extrude, −1) steht noch,
    // ist aber eine ECHTE, dokumentierte Verweigerung.
    let tree = CsgNode::Offset(Box::new(quad_bezier_extrude(1)), -1.0);
    let after_fold = tree.push_down_offsets().fold_offset_leaves();
    assert!(
        matches!(&after_fold, CsgNode::Offset(_, d) if *d < 0.0),
        "expected a residual erosion offset, got {after_fold:?}"
    );
    assert!(check_offset_leaves_folded(&after_fold).is_ok());
}

#[test]
fn pos_dilation_leaf_is_not_flagged() {
    // Dilation über einem Blatt bleibt bewusst als f−δ und ist keine Verletzung.
    let ok = CsgNode::Offset(Box::new(sphere_leaf(1, 5.0)), 2.0);
    assert!(check_offset_leaves_folded(&ok).is_ok());
}

#[test]
fn neg_foldable_erosion_leaf_is_flagged() {
    // Kugel-Erosion IST faltbar (r−a) → ein ungefalteter Rest ist ein Bug.
    let bad = CsgNode::Offset(Box::new(sphere_leaf(1, 5.0)), -2.0);
    let err = check_offset_leaves_folded(&bad).unwrap_err();
    assert!(
        err.contains("faltbar") || err.contains("fold_offset_leaves"),
        "message: {err}"
    );
}

// ─── Invariante 4: InnerWall nur als rechtes Kind einer Difference ────────

#[test]
fn pos_innerwall_right_of_difference_is_ok() {
    let good = CsgNode::Difference(
        Box::new(box_leaf(1, [10.0, 10.0, 10.0])),
        Box::new(CsgNode::InnerWall(Box::new(CsgNode::Offset(
            Box::new(box_leaf(1, [10.0, 10.0, 10.0])),
            -2.0,
        )))),
    );
    assert!(check_inner_wall_position(&good).is_ok());
}

#[test]
fn neg_innerwall_left_of_difference_is_flagged() {
    let bad = CsgNode::Difference(
        Box::new(CsgNode::InnerWall(Box::new(box_leaf(1, [5.0, 5.0, 5.0])))),
        Box::new(box_leaf(2, [5.0, 5.0, 5.0])),
    );
    let err = check_inner_wall_position(&bad).unwrap_err();
    assert!(err.contains("InnerWall"), "message: {err}");
    assert!(err.contains("Difference.l"), "path: {err}");
}

#[test]
fn pos_erosion_over_closed_shell_keeps_innerwall_and_folds() {
    // Befund F1 aus dem #384-Fuzz-Harness: Offset(closed_shell, δ<0). Der
    // Pushdown dilatiert den InnerWall-Subtrahenden der expandierten Shell;
    // vor dem Fix blieb der Offset ÜBER dem InnerWall stehen (Invariante 4
    // verletzt, Höhle ungefaltet). InnerWall ist feld-transparent, also muss
    // der Offset hindurchkommutieren.
    let node = CsgNode::Offset(
        Box::new(CsgNode::Shell(Box::new(box_leaf(1, [25.0, 25.0, 25.0])), 3.0, Vec::new())),
        -2.0,
    );

    let expanded = node.clone().expand_shells();
    let pushed = expanded.clone().push_down_offsets();
    assert!(check_inner_wall_position(&pushed).is_ok(), "{:?}", check_inner_wall_position(&pushed));
    assert!(check_offsets_pushed(&pushed).is_ok(), "{:?}", check_offsets_pushed(&pushed));

    // Pushdown ist feld-exakt: vor dem Falten punktweise identisch zur
    // unge-pushten Expansion.
    for p in [[0.0, 0.0, 0.0], [24.0, 0.0, 0.0], [21.5, 21.5, 0.0], [30.0, 5.0, -3.0]] {
        let a = expanded.sdf(p, crate::csg::EvalMode::Visual);
        let b = pushed.sdf(p, crate::csg::EvalMode::Visual);
        assert_eq!(a.to_bits(), b.to_bits(), "field drift at {p:?}: {a} vs {b}");
    }

    // Nach dem Falten: Höhle exakt eingefaltet — kein Offset-Node mehr im
    // gesamten Baum (Außen-Erosion UND dilatierte Höhle sind Box-faltbar).
    let folded = pushed.fold_offset_leaves();
    assert!(check_offset_leaves_folded(&folded).is_ok());
    assert!(check_inner_wall_position(&folded).is_ok());
    fn count_offsets(n: &CsgNode) -> usize {
        let own = usize::from(matches!(n, CsgNode::Offset(..)));
        own + children(n).iter().map(|(c, _)| count_offsets(c)).sum::<usize>()
    }
    assert_eq!(count_offsets(&folded), 0, "cavity/outer offsets must fold: {folded:?}");
}

#[test]
fn neg_innerwall_under_union_is_flagged() {
    let bad = CsgNode::Union(
        Box::new(box_leaf(1, [5.0, 5.0, 5.0])),
        Box::new(CsgNode::InnerWall(Box::new(box_leaf(2, [5.0, 5.0, 5.0])))),
    );
    assert!(check_inner_wall_position(&bad).is_err());
}
