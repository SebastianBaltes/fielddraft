//! A2-Verifikation „Erosions-Blatt-Einfalten" (#384, offset_tree Phase A2).
//!
//! Vergleicht je Erosions-Fall das UNGEFALTETE Feld (`Offset(blatt, −δ)`
//! nach `push_down_offsets`, also f − δ) mit dem GEFALTETEN Feld
//! (`fold_offset_leaves`) gegen die jeweils eigene Ground Truth
//! (Distanz zur eigenen Nullmenge — Metrizität). Erwartung: das
//! gefaltete Blatt ist wieder ein exaktes SDF (Fehler in der
//! f32-Rausch-Klasse), während f − δ bei Erosion nur nullmengen-exakt
//! ist (Außenfeld-Unterschätzung an konvexen Ecken, beim Union-Profil
//! sogar Nullmengen-Fehler am Hals).
//!
//! Harness: Wiederverwendung von `metricity_lab::runner::run_scene`
//! (wie `iter3_offset_tree.rs`); eigene Ausgabedatei
//! `a2_fold_check.csv` — die Forschungs-CSVs von Iteration 3 bleiben
//! unangetastet.
//!
//! Aufruf: cargo run -p metricity-lab --release --bin a2_fold_check

use std::path::PathBuf;

use fielddraft_kernel::csg::CsgNode;
use fielddraft_kernel::sketch2d::{Sketch2d, Sketch2dPrimitive, SketchSegment};
use fielddraft_protocol::{FeatureId, Primitive};

use metricity_lab::report::{summary_row, write_csv, SUMMARY_HEADER};
use metricity_lab::runner::{run_scene, RunConfig};
use metricity_lab::scenes::Scene;

fn out_dir() -> PathBuf {
    PathBuf::from("docs/research/sdf-metricity/iter3-offset-tree")
}

fn leaf(id: u32, primitive: Primitive) -> CsgNode {
    CsgNode::Leaf { id: FeatureId::from_u32(id), primitive }
}

fn seg_line(a: [f32; 2], b: [f32; 2]) -> SketchSegment {
    SketchSegment::Line { start: a, end: b }
}

fn seg_arc(c: [f32; 2], r: f32, a0: f32, sw: f32) -> SketchSegment {
    SketchSegment::Arc { center: c, radius: r, start_angle: a0, sweep_angle: sw }
}

/// Erosions-Fälle: (Name, Blattknoten, δ<0).
fn cases() -> Vec<(&'static str, CsgNode, f32)> {
    use std::f32::consts::PI;
    let slot_notch = Sketch2d::Prim(Sketch2dPrimitive::Path {
        segments: vec![
            seg_line([-30.0, -15.0], [20.0, -15.0]),
            seg_arc([20.0, 0.0], 15.0, -PI / 2.0, PI),
            seg_line([20.0, 15.0], [-30.0, 15.0]),
            seg_line([-30.0, 15.0], [-30.0, 5.0]),
            seg_arc([-30.0, 0.0], 5.0, PI / 2.0, -PI),
            seg_line([-30.0, -5.0], [-30.0, -15.0]),
        ],
        closed: true,
    });
    let bool_profile = Sketch2d::Difference(
        Box::new(Sketch2d::Prim(Sketch2dPrimitive::Rectangle {
            center: [0.0, 0.0],
            half_extents: [30.0, 15.0],
        })),
        Box::new(Sketch2d::Prim(Sketch2dPrimitive::Circle {
            center: [0.0, 15.0],
            radius: 10.0,
        })),
    );
    let union_profile = Sketch2d::Union(
        Box::new(Sketch2d::Prim(Sketch2dPrimitive::Circle {
            center: [-8.0, 0.0],
            radius: 10.0,
        })),
        Box::new(Sketch2d::Prim(Sketch2dPrimitive::Circle {
            center: [8.0, 0.0],
            radius: 10.0,
        })),
    );
    let offset_profile = Sketch2d::Offset {
        inner: Box::new(Sketch2d::Prim(Sketch2dPrimitive::Polygon {
            vertices: vec![
                [-20.0, -20.0],
                [20.0, -20.0],
                [20.0, 0.0],
                [0.0, 0.0],
                [0.0, 20.0],
                [-20.0, 20.0],
            ],
        })),
        distance: 4.0,
    };
    let ring = Sketch2d::Prim(Sketch2dPrimitive::Path {
        segments: vec![
            seg_line([21.0, -10.0], [29.0, -10.0]),
            seg_arc([29.0, -7.0], 3.0, -PI / 2.0, PI / 2.0),
            seg_line([32.0, -7.0], [32.0, 7.0]),
            seg_arc([29.0, 7.0], 3.0, 0.0, PI / 2.0),
            seg_line([29.0, 10.0], [21.0, 10.0]),
            seg_arc([21.0, 7.0], 3.0, PI / 2.0, PI / 2.0),
            seg_line([18.0, 7.0], [18.0, -7.0]),
            seg_arc([21.0, -7.0], 3.0, PI, PI / 2.0),
        ],
        closed: true,
    });
    vec![
        (
            "a2_box",
            leaf(1, Primitive::Box { center: [0.0; 3], half_extents: [20.0, 12.0, 8.0] }),
            -2.0,
        ),
        (
            "a2_cylinder",
            leaf(1, Primitive::Cylinder { center: [0.0; 3], radius: 15.0, half_height: 10.0 }),
            -2.0,
        ),
        (
            "a2_cone",
            leaf(1, Primitive::Cone { r_bottom: 18.0, r_top: 7.0, half_height: 12.0 }),
            -2.0,
        ),
        (
            "a2_torus",
            leaf(1, Primitive::Torus { center: [0.0; 3], major_radius: 20.0, minor_radius: 6.0 }),
            -2.0,
        ),
        (
            "a2_extrude_path_arcs",
            CsgNode::Extrude {
                id: FeatureId::from_u32(1),
                sketch: Box::new(slot_notch),
                half_height: 15.0,
                wall_subs: None,
            },
            -2.0,
        ),
        (
            "a2_extrude_bool_profile",
            CsgNode::Extrude {
                id: FeatureId::from_u32(1),
                sketch: Box::new(bool_profile),
                half_height: 12.0,
                wall_subs: None,
            },
            -2.0,
        ),
        (
            "a2_extrude_union_profile",
            CsgNode::Extrude {
                id: FeatureId::from_u32(1),
                sketch: Box::new(union_profile),
                half_height: 12.0,
                wall_subs: None,
            },
            -2.0,
        ),
        (
            "a2_extrude_offset_profile",
            CsgNode::Extrude {
                id: FeatureId::from_u32(1),
                sketch: Box::new(offset_profile),
                half_height: 10.0,
                wall_subs: None,
            },
            -1.0,
        ),
        (
            "a2_revolve_ring_arcs",
            CsgNode::Revolve {
                id: FeatureId::from_u32(1),
                sketch: Box::new(ring),
                wall_subs: None,
            },
            -1.0,
        ),
    ]
}

fn main() {
    std::fs::create_dir_all(out_dir()).expect("mkdir out");
    let cfg = RunConfig { bench_evals: 0, ..RunConfig::default() };
    let mut rows = Vec::new();
    println!("── A2-Fold-Check: f−δ (unfolded) vs gefaltetes Blatt (folded) ──");
    for (name, node, delta) in cases() {
        let unfolded = CsgNode::Offset(Box::new(node.clone()), delta).push_down_offsets();
        let folded = unfolded.clone().fold_offset_leaves();
        assert!(
            !matches!(folded, CsgNode::Offset(_, _)),
            "{name}: erosion case must fold",
        );
        for (variant, tree) in [("unfolded", unfolded), ("folded", folded)] {
            let scene = Scene {
                name: format!("{name}_{variant}"),
                description: format!("erosion δ={delta} {variant}"),
                node: tree,
                gt_grid: 0,
            };
            println!("  {} …", scene.name);
            let res = run_scene(&scene, &cfg);
            println!(
                "    near_out max {:.5}  near_in max {:.5}  mid_in max {:.5}  far_out max {:.5}",
                res.distance.near_out.max,
                res.distance.near_in.max,
                res.distance.mid_in.max,
                res.distance.far_out.max,
            );
            rows.push(summary_row(&res));
        }
    }
    write_csv(&out_dir().join("a2_fold_check.csv"), SUMMARY_HEADER, &rows)
        .expect("write csv");
    println!("CSV → {}", out_dir().join("a2_fold_check.csv").display());
}
