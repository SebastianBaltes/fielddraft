//! Iteration-3-Forschungs-Spike „offset_tree" (#384, SYNTHESIS.md §9).
//!
//! Teil A — Extrude/Revolve-Blatt-Exaktheit am ECHTEN Kernel
//! (`CsgNode::Extrude`/`Revolve`, `EvalMode::Visual`): sind die heutigen
//! Sweep-Blätter echte SDFs, und wo genau nicht (2D-Boolean-Profile,
//! 2D-Offset f−δ, achsberührende Revolve-Profile)?
//!
//! Teil B — Kanal-Korrektur-Prototyp an EINER gekrümmten Schnittkante:
//! Offset nach außen über Schnitt zweier orthogonaler Zylinder
//! (Steinmetz-Kante). Reines f64-Labor, kein Kernel-Code. Vergleich
//! naiv (Schnitt offsetteter Blätter) vs. Kanal-Korrektur (Keil-Abzug),
//! in zwei Gating-Varianten, gegen eine exakte Ground Truth
//! (Konvexität ⇒ SDF(S↑δ) = SDF(S) − δ; dist(x,S) via geschlossener
//! Projektions-Identität, validiert gegen Dykstra + Ball-Dilation).
//!
//! Quelle der Konstruktion: J. R. Rossignac & A. A. G. Requicha,
//! „Offsetting operations in solid modelling", CAGD 3 (1986) 129–148
//! (§2.4 Boolean-Identitäten, §2.6 n-Offsets/Kanal-Flächen, §3.1 PCC).
//!
//! Aufruf (immer --release):
//!   cargo run -p metricity-lab --release --bin iter3_offset_tree -- a
//!   cargo run -p metricity-lab --release --bin iter3_offset_tree -- b
//!   cargo run -p metricity-lab --release --bin iter3_offset_tree -- all
//!
//! Ausgaben: docs/research/sdf-metricity/iter3-offset-tree/*.csv|json

use std::path::PathBuf;

use fielddraft_kernel::csg::CsgNode;
use fielddraft_kernel::sketch2d::{Sketch2d, Sketch2dPrimitive, SketchSegment};
use fielddraft_protocol::FeatureId;

use metricity_lab::metrics::Stats;
use metricity_lab::report::{summary_row, write_csv, write_json, SUMMARY_HEADER};
use metricity_lab::runner::{run_scene, RunConfig};
use metricity_lab::scenes::Scene;
use metricity_lab::util::{add, cross, dot, norm, normalize, scale, sub, SplitMix64};

use rayon::prelude::*;

fn out_dir() -> PathBuf {
    // Vom Workspace-Root aus aufrufen (cargo run tut das).
    PathBuf::from("docs/research/sdf-metricity/iter3-offset-tree")
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let what = args.first().map(String::as_str).unwrap_or("all");
    std::fs::create_dir_all(out_dir()).expect("mkdir out");
    match what {
        "a" => part_a(),
        "b" => part_b(),
        "bval" => {
            let dense = EdgePolyline::build(200_000);
            b_validate_gt(&dense);
        }
        "bdbg" => b_debug_nearfield_outlier(),
        "all" => {
            part_a();
            part_b();
        }
        other => {
            eprintln!("unbekanntes Kommando {other}; nutze a|b|all");
            std::process::exit(2);
        }
    }
}

// ═══════════════════════════════════════════════════════════════════
// Teil A — Blatt-Exaktheit Extrude/Revolve (echter Kernel)
// ═══════════════════════════════════════════════════════════════════

fn seg_line(a: [f32; 2], b: [f32; 2]) -> SketchSegment {
    SketchSegment::Line { start: a, end: b }
}

fn seg_arc(c: [f32; 2], r: f32, a0: f32, sw: f32) -> SketchSegment {
    SketchSegment::Arc { center: c, radius: r, start_angle: a0, sweep_angle: sw }
}

/// Geschlossener Pfad: Rechteck-Slot mit konvexer Bogen-Nase (r=15)
/// rechts und konkaver Bogen-Kerbe (r=5) in der linken Kante.
/// Enthält konvexe + konkave Bögen und scharfe Ecken — ein einzelnes
/// Path-Primitiv, dessen 2D-SDF laut Konstruktion exakt ist.
fn profile_slot_notch() -> Sketch2d {
    use std::f32::consts::PI;
    Sketch2d::Prim(Sketch2dPrimitive::Path {
        segments: vec![
            seg_line([-30.0, -15.0], [20.0, -15.0]),
            seg_arc([20.0, 0.0], 15.0, -PI / 2.0, PI),
            seg_line([20.0, 15.0], [-30.0, 15.0]),
            seg_line([-30.0, 15.0], [-30.0, 5.0]),
            seg_arc([-30.0, 0.0], 5.0, PI / 2.0, -PI),
            seg_line([-30.0, -5.0], [-30.0, -15.0]),
        ],
        closed: true,
    })
}

/// 2D-Boolean-Profil: Rechteck minus Randkreis (Biss in der Oberkante,
/// zwei einspringende Ecken) — der max(a,−b)-Fan in 2D.
fn profile_rect_minus_circle() -> Sketch2d {
    Sketch2d::Difference(
        Box::new(Sketch2d::Prim(Sketch2dPrimitive::Rectangle {
            center: [0.0, 0.0],
            half_extents: [30.0, 15.0],
        })),
        Box::new(Sketch2d::Prim(Sketch2dPrimitive::Circle {
            center: [0.0, 15.0],
            radius: 10.0,
        })),
    )
}

/// 2D-Offset-Profil: L-Polygon (konkave Ecke) um +4 dilatiert via
/// Sketch2d::Offset — das f−δ des 2D-Pfads.
fn profile_l_offset() -> Sketch2d {
    Sketch2d::Offset {
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
    }
}

/// Ring-Profil (achsenfern, u ∈ [18,32]): gerundetes Rechteck aus
/// Linien + 4 Bögen. Revolve davon muss laut §9 exakt sein.
fn profile_ring_rounded_rect() -> Sketch2d {
    use std::f32::consts::PI;
    Sketch2d::Prim(Sketch2dPrimitive::Path {
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
    })
}

fn extrude_scene(name: &str, desc: &str, sketch: Sketch2d, hh: f32) -> Scene {
    Scene {
        name: name.to_string(),
        description: desc.to_string(),
        node: CsgNode::Extrude {
            id: FeatureId::from_u32(1),
            sketch: Box::new(sketch),
            half_height: hh,
            wall_subs: None,
        },
        gt_grid: 0,
    }
}

fn revolve_scene(name: &str, desc: &str, sketch: Sketch2d) -> Scene {
    Scene {
        name: name.to_string(),
        description: desc.to_string(),
        node: CsgNode::Revolve {
            id: FeatureId::from_u32(1),
            sketch: Box::new(sketch),
            wall_subs: None,
        },
        gt_grid: 0,
    }
}

fn part_a() {
    println!("── Teil A: Blatt-Exaktheit Extrude/Revolve ──");
    let scenes = vec![
        extrude_scene(
            "iter3_extrude_path_arcs",
            "Extrude eines einzelnen geschlossenen Pfads (konvexer+konkaver Bogen, Ecken), h=+-15",
            profile_slot_notch(),
            15.0,
        ),
        extrude_scene(
            "iter3_extrude_bool_profile",
            "Extrude eines 2D-Boolean-Profils (Rechteck minus Randkreis), h=+-12",
            profile_rect_minus_circle(),
            12.0,
        ),
        extrude_scene(
            "iter3_extrude_offset_profile",
            "Extrude eines 2D-Offset-Profils (L-Polygon, Sketch2d::Offset +4), h=+-10",
            profile_l_offset(),
            10.0,
        ),
        revolve_scene(
            "iter3_revolve_ring_arcs",
            "Revolve eines achsenfernen Pfad-Profils (gerundetes Rechteck u in [18,32])",
            profile_ring_rounded_rect(),
        ),
        revolve_scene(
            "iter3_revolve_solid_axis",
            "Revolve eines achsberuehrenden Rechtecks (u in [0,30], v in [-20,20]) -> Zylinder r=30 h=40",
            Sketch2d::Prim(Sketch2dPrimitive::Rectangle {
                center: [15.0, 0.0],
                half_extents: [15.0, 20.0],
            }),
        ),
    ];

    let cfg = RunConfig { bench_evals: 200_000, ..RunConfig::default() };
    let mut rows = Vec::new();
    for scene in &scenes {
        println!("  Szene {} …", scene.name);
        let res = run_scene(scene, &cfg);
        write_json(&out_dir().join(format!("a_scene_{}.json", scene.name)), &res)
            .expect("write json");
        rows.push(summary_row(&res));
        println!(
            "    near_out max {:.5}  near_in max {:.5}  mid_in max {:.5}  far_in max {:.5}  tape_dev {:.5}",
            res.distance.near_out.max,
            res.distance.near_in.max,
            res.distance.mid_in.max,
            res.distance.far_in.max,
            res.tape_max_abs_diff
        );
    }
    write_csv(&out_dir().join("a_summary.csv"), SUMMARY_HEADER, &rows).expect("write csv");

    a5_axis_scan();
    a5_analytic_stats();
    a_2d_profile_errors();
    println!("── Teil A fertig ──");
}

/// A5: Der Revolve eines achsberührenden Profils gegen die analytische
/// Zylinder-Wahrheit, als Linien-Scan entlang der Radialen (y=z=0).
fn a5_axis_scan() {
    let node = CsgNode::Revolve {
        id: FeatureId::from_u32(1),
        sketch: Box::new(Sketch2d::Prim(Sketch2dPrimitive::Rectangle {
            center: [15.0, 0.0],
            half_extents: [15.0, 20.0],
        })),
        wall_subs: None,
    };
    let f = |p: [f64; 3]| -> f64 {
        node.sdf(
            [p[0] as f32, p[1] as f32, p[2] as f32],
            fielddraft_kernel::csg::EvalMode::Visual,
        ) as f64
    };
    // Exaktes SDF des Zylinders r=30, |y|<=20 (Revolve um Y).
    let truth = |p: [f64; 3]| -> f64 {
        cyl_sdf((p[0] * p[0] + p[2] * p[2]).sqrt(), p[1].abs(), 30.0, 20.0)
    };
    let mut rows = Vec::new();
    let n = 300;
    for i in 0..=n {
        let x = 29.95 * i as f64 / n as f64;
        let p = [x, 0.0, 0.0];
        rows.push(format!("{:.4},{:.6},{:.6},{:.6}", x, f(p), truth(p), f(p) - truth(p)));
    }
    write_csv(
        &out_dir().join("a5_axis_scan.csv"),
        "x,f_kernel,d_true,err",
        &rows,
    )
    .expect("write a5 scan");
}

/// A5: Bänder-Statistik gegen die analytische Wahrheit (kein GT nötig).
fn a5_analytic_stats() {
    let node = CsgNode::Revolve {
        id: FeatureId::from_u32(1),
        sketch: Box::new(Sketch2d::Prim(Sketch2dPrimitive::Rectangle {
            center: [15.0, 0.0],
            half_extents: [15.0, 20.0],
        })),
        wall_subs: None,
    };
    let truth = |p: [f64; 3]| -> f64 {
        cyl_sdf((p[0] * p[0] + p[2] * p[2]).sqrt(), p[1].abs(), 30.0, 20.0)
    };
    let mut rng = SplitMix64::new(777);
    let mut pts = Vec::new();
    for _ in 0..40_000 {
        pts.push([rng.range(-42.0, 42.0), rng.range(-32.0, 32.0), rng.range(-42.0, 42.0)]);
    }
    let rows: Vec<(f64, f64)> = pts
        .par_iter()
        .map(|&p| {
            let fk = node.sdf(
                [p[0] as f32, p[1] as f32, p[2] as f32],
                fielddraft_kernel::csg::EvalMode::Visual,
            ) as f64;
            (truth(p), fk - truth(p))
        })
        .collect();
    let size = 84.0f64;
    let band = |lo: f64, hi: f64, inside: bool| -> Stats {
        Stats::from_signed(
            rows.iter()
                .filter(|(d, _)| {
                    let a = d.abs();
                    a >= lo && a < hi && ((*d < 0.0) == inside)
                })
                .map(|(_, e)| *e)
                .collect(),
        )
    };
    let mut out = Vec::new();
    for (name, lo, hi, inside) in [
        ("near_out", 0.0, 0.02 * size, false),
        ("near_in", 0.0, 0.02 * size, true),
        ("mid_out", 0.02 * size, 0.10 * size, false),
        ("mid_in", 0.02 * size, 0.10 * size, true),
        ("far_out", 0.10 * size, f64::INFINITY, false),
        ("far_in", 0.10 * size, f64::INFINITY, true),
    ] {
        let s = band(lo, hi, inside);
        out.push(format!(
            "{},{},{:.6},{:.6},{:.6},{:.6}",
            name, s.count, s.mean, s.p95, s.max, s.mean_signed
        ));
    }
    write_csv(
        &out_dir().join("a5_analytic_bands.csv"),
        "band,count,mean,p95,max,bias",
        &out,
    )
    .expect("write a5 bands");
}

/// 2D-Ray-Wahrheit: exakte Distanz zur Nullmenge eines 1-Lipschitz-
/// 2D-Felds via geführtem March + Bisektion über `dirs` Richtungen.
/// (Alle Sketch-Felder sind min/max/f−δ exakter Distanzen ⇒ 1-Lipschitz,
/// der geführte Schritt |f|·0.9 kann keine Nullstelle überspringen.)
fn dist_to_zero_2d(f: &(dyn Fn([f64; 2]) -> f64 + Sync), p: [f64; 2], dirs: usize, max_d: f64) -> f64 {
    let f0 = f(p);
    if f0.abs() < 1e-5 {
        return 0.0;
    }
    let sgn = if f0 < 0.0 { -1.0 } else { 1.0 };
    let mut best = max_d;
    for k in 0..dirs {
        let ang = k as f64 * std::f64::consts::TAU / dirs as f64;
        let (s, c) = ang.sin_cos();
        let mut t = 0.0f64;
        let mut g_prev = sgn * f0;
        loop {
            if t >= best {
                break;
            }
            let step = (g_prev * 0.9).max(2e-4);
            let tn = (t + step).min(best);
            let gn = sgn * f([p[0] + c * tn, p[1] + s * tn]);
            if gn <= 0.0 {
                let (mut lo, mut hi) = (t, tn);
                for _ in 0..50 {
                    let m = (lo + hi) * 0.5;
                    if sgn * f([p[0] + c * m, p[1] + s * m]) <= 0.0 {
                        hi = m;
                    } else {
                        lo = m;
                    }
                }
                if hi < best {
                    best = hi;
                }
                break;
            }
            if tn >= best {
                break;
            }
            t = tn;
            g_prev = gn;
        }
    }
    best
}

/// 2D-Metrikfehler der Boolean-/Offset-Profile direkt auf Sketch2d-Ebene
/// (isoliert den 2D-Anteil vom 3D-Lift).
fn a_2d_profile_errors() {
    let cases: Vec<(&str, Sketch2d, [f64; 4])> = vec![
        ("bool_rect_minus_circle", profile_rect_minus_circle(), [-36.0, 36.0, -21.0, 32.0]),
        ("offset_l_plus4", profile_l_offset(), [-30.0, 30.0, -30.0, 30.0]),
        ("path_slot_notch", profile_slot_notch(), [-36.0, 41.0, -21.0, 21.0]),
    ];
    let mut rows = Vec::new();
    for (name, sk, bb) in &cases {
        let f = |p: [f64; 2]| -> f64 { sk.sdf([p[0] as f32, p[1] as f32]) as f64 };
        let mut rng = SplitMix64::new(4242);
        let mut pts = Vec::new();
        while pts.len() < 1500 {
            let p = [rng.range(bb[0], bb[1]), rng.range(bb[2], bb[3])];
            if f(p).abs() <= 8.0 {
                pts.push(p);
            }
        }
        let case_rows: Vec<String> = pts
            .par_iter()
            .map(|&p| {
                let fv = f(p);
                let d = dist_to_zero_2d(&f, p, 512, 60.0) * fv.signum();
                format!("{},{:.4},{:.4},{:.6},{:.6},{:.6}", name, p[0], p[1], d, fv, fv - d)
            })
            .collect();
        // Kurz-Statistik auf stdout.
        let errs: Vec<f64> = pts
            .par_iter()
            .map(|&p| {
                let fv = f(p);
                fv - dist_to_zero_2d(&f, p, 512, 60.0) * fv.signum()
            })
            .collect();
        let inside: Vec<f64> = pts
            .iter()
            .zip(&errs)
            .filter(|(p, _)| f(**p) < 0.0)
            .map(|(_, e)| *e)
            .collect();
        let outside: Vec<f64> = pts
            .iter()
            .zip(&errs)
            .filter(|(p, _)| f(**p) >= 0.0)
            .map(|(_, e)| *e)
            .collect();
        let si = Stats::from_signed(inside);
        let so = Stats::from_signed(outside);
        println!(
            "  2D {name}: innen mean {:.5} p95 {:.5} max {:.5} bias {:.5} | aussen mean {:.5} p95 {:.5} max {:.5} bias {:.5}",
            si.mean, si.p95, si.max, si.mean_signed, so.mean, so.p95, so.max, so.mean_signed
        );
        rows.extend(case_rows);
    }
    write_csv(
        &out_dir().join("a_2d_profile_errors.csv"),
        "profile,x,y,d_true,f,err",
        &rows,
    )
    .expect("write 2d errors");
}

// ═══════════════════════════════════════════════════════════════════
// Teil B — Kanal-Korrektur an der Steinmetz-Kante (f64-Labor)
// ═══════════════════════════════════════════════════════════════════
//
// A = Zylinder r1=30 entlang z (|z|<=50), B = Zylinder r2=20 entlang x
// (|x|<=50). S = A ∩ B ist KONVEX ⇒ SDF(S↑δ) = SDF(S) − δ exakt.
// Schnittkante: zwei Steinmetz-Loops x = ±sqrt(r1²−r2²cos²θ),
// y = r2·cosθ, z = r2·sinθ. Kappen liegen fern (|z|,|x| ≤ 30 < 50).

const R1: f64 = 30.0;
const HZ1: f64 = 50.0;
const R2: f64 = 20.0;
const HX2: f64 = 50.0;

fn cyl_sdf(rad: f64, ax: f64, r: f64, h: f64) -> f64 {
    let d = rad - r;
    let w = ax - h;
    let outside = d.max(0.0).hypot(w.max(0.0));
    outside + d.max(w).min(0.0)
}

fn fa(p: [f64; 3]) -> f64 {
    cyl_sdf((p[0] * p[0] + p[1] * p[1]).sqrt(), p[2].abs(), R1, HZ1)
}

fn fb(p: [f64; 3]) -> f64 {
    cyl_sdf((p[1] * p[1] + p[2] * p[2]).sqrt(), p[0].abs(), R2, HX2)
}

fn proj_a(p: [f64; 3]) -> [f64; 3] {
    let z = p[2].clamp(-HZ1, HZ1);
    let r = (p[0] * p[0] + p[1] * p[1]).sqrt();
    if r <= R1 {
        [p[0], p[1], z]
    } else {
        let s = R1 / r;
        [p[0] * s, p[1] * s, z]
    }
}

fn proj_b(p: [f64; 3]) -> [f64; 3] {
    let x = p[0].clamp(-HX2, HX2);
    let r = (p[1] * p[1] + p[2] * p[2]).sqrt();
    if r <= R2 {
        [x, p[1], p[2]]
    } else {
        let s = R2 / r;
        [x, p[1] * s, p[2] * s]
    }
}

/// Punkt auf der Schnittkante (Loop `sx`=±1) bei Parameter θ.
fn edge_point(sx: f64, theta: f64) -> [f64; 3] {
    let y = R2 * theta.cos();
    let z = R2 * theta.sin();
    let x = sx * (R1 * R1 - y * y).sqrt();
    [x, y, z]
}

/// Analytische Flächennormalen an einem (approximativen) Kantenpunkt.
fn edge_normals(q: [f64; 3]) -> ([f64; 3], [f64; 3]) {
    let na = normalize([q[0], q[1], 0.0]).expect("na");
    let nb = normalize([0.0, q[1], q[2]]).expect("nb");
    (na, nb)
}

/// Dykstra-Projektion auf S = A ∩ B (unabhängiger Validator).
/// Rückgabe (dist, iterationen).
fn dykstra_dist(p: [f64; 3]) -> (f64, usize) {
    if fa(p) <= 0.0 && fb(p) <= 0.0 {
        return (0.0, 0);
    }
    let mut x = p;
    let mut pa = [0.0; 3];
    let mut pb = [0.0; 3];
    let mut iters = 0;
    for it in 0..6000 {
        let prev = x;
        let ya = proj_a(add(x, pa));
        pa = sub(add(x, pa), ya);
        let yb = proj_b(add(ya, pb));
        pb = sub(add(ya, pb), yb);
        x = yb;
        iters = it + 1;
        if norm(sub(x, prev)) < 1e-13 {
            break;
        }
    }
    (norm(sub(p, x)), iters)
}

// ── Kanten-Approximationen ─────────────────────────────────────────

/// Polyline über beide Loops, uniform in θ, mit Chunk-Pruning.
struct EdgePolyline {
    /// Je Loop: Punkte (geschlossen, pts[n] == Nachbar von pts[0]).
    loops: Vec<Vec<[f64; 3]>>,
    /// Chunk-Bounding-Spheres: (loop, start_idx, len, center, radius).
    chunks: Vec<(usize, usize, usize, [f64; 3], f64)>,
}

impl EdgePolyline {
    fn build(n: usize) -> Self {
        let mut loops = Vec::new();
        for sx in [1.0, -1.0] {
            let pts: Vec<[f64; 3]> = (0..n)
                .map(|i| edge_point(sx, i as f64 * std::f64::consts::TAU / n as f64))
                .collect();
            loops.push(pts);
        }
        let chunk_len = 16usize.min(n);
        let mut chunks = Vec::new();
        for (li, pts) in loops.iter().enumerate() {
            let n = pts.len();
            let mut s = 0;
            while s < n {
                let len = chunk_len.min(n - s);
                // Segment i geht von pts[i] nach pts[(i+1)%n]; der Chunk
                // deckt Segmente s..s+len ab, braucht also Punkte bis s+len.
                let mut c = [0.0; 3];
                for k in 0..=len {
                    c = add(c, pts[(s + k) % n]);
                }
                c = scale(c, 1.0 / (len + 1) as f64);
                let mut r: f64 = 0.0;
                for k in 0..=len {
                    r = r.max(norm(sub(pts[(s + k) % n], c)));
                }
                chunks.push((li, s, len, c, r));
                s += len;
            }
        }
        Self { loops, chunks }
    }

    /// Nächster Punkt auf der Polyline: (Distanz, Fußpunkt).
    fn closest(&self, p: [f64; 3]) -> (f64, [f64; 3]) {
        let mut best = f64::INFINITY;
        let mut best_q = p;
        for &(li, s, len, c, r) in &self.chunks {
            if norm(sub(p, c)) - r >= best {
                continue;
            }
            let pts = &self.loops[li];
            let n = pts.len();
            for k in 0..len {
                let a = pts[(s + k) % n];
                let b = pts[(s + k + 1) % n];
                let ab = sub(b, a);
                let t = (dot(sub(p, a), ab) / dot(ab, ab)).clamp(0.0, 1.0);
                let q = add(a, scale(ab, t));
                let d = norm(sub(p, q));
                if d < best {
                    best = d;
                    best_q = q;
                }
            }
        }
        (best, best_q)
    }
}

/// Kreisbogen im Raum (Arc-Spline-Baustein): exakte Distanz.
#[derive(Clone)]
struct Arc3d {
    c: [f64; 3],
    u: [f64; 3],
    v: [f64; 3],
    r: f64,
    sweep: f64,
    bound_c: [f64; 3],
    bound_r: f64,
}

impl Arc3d {
    /// Bogen durch drei Punkte (P0 → P1 → P2).
    fn through(p0: [f64; 3], p1: [f64; 3], p2: [f64; 3]) -> Self {
        let a = sub(p1, p0);
        let b = sub(p2, p0);
        let n = cross(a, b);
        let n2 = dot(n, n);
        assert!(n2 > 1e-18, "Arc3d::through: kollinear");
        // Zirkumzentrum: c = p0 + (|b|²(n×a) + |a|²(b×n)) / (2|n|²)
        let c = add(
            p0,
            scale(
                add(scale(cross(n, a), dot(b, b)), scale(cross(b, n), dot(a, a))),
                1.0 / (2.0 * n2),
            ),
        );
        let r = norm(sub(p0, c));
        let mut w = normalize(n).unwrap();
        let u = normalize(sub(p0, c)).unwrap();
        let mut v = cross(w, u);
        let ang = |q: [f64; 3], v_: [f64; 3]| -> f64 {
            let d = sub(q, c);
            let mut a = dot(d, v_).atan2(dot(d, u));
            if a < 0.0 {
                a += std::f64::consts::TAU;
            }
            a
        };
        let mut a1 = ang(p1, v);
        let mut a2 = ang(p2, v);
        if a1 > a2 {
            // Orientierung umdrehen, damit P1 innerhalb [0, sweep] liegt.
            w = scale(w, -1.0);
            v = cross(w, u);
            a1 = ang(p1, v);
            a2 = ang(p2, v);
        }
        debug_assert!(a1 <= a2 + 1e-12);
        let _ = a1;
        let mid = add(c, scale(add(scale(u, (a2 * 0.5).cos()), scale(v, (a2 * 0.5).sin())), r));
        let bound_c = scale(add(p0, add(p2, mid)), 1.0 / 3.0);
        let bound_r = [p0, p1, p2, mid]
            .iter()
            .map(|q| norm(sub(*q, bound_c)))
            .fold(0.0f64, f64::max)
            + 1e-9;
        let _ = w;
        Self { c, u, v, r, sweep: a2, bound_c, bound_r }
    }

    fn point(&self, phi: f64) -> [f64; 3] {
        add(
            self.c,
            scale(add(scale(self.u, phi.cos()), scale(self.v, phi.sin())), self.r),
        )
    }

    fn closest(&self, p: [f64; 3]) -> (f64, [f64; 3]) {
        let d = sub(p, self.c);
        let x = dot(d, self.u);
        let y = dot(d, self.v);
        let rho = (x * x + y * y).sqrt();
        if rho < 1e-12 {
            let q = self.point(0.0);
            return (norm(sub(p, q)), q);
        }
        let mut phi = y.atan2(x);
        if phi < 0.0 {
            phi += std::f64::consts::TAU;
        }
        if phi <= self.sweep {
            let q = add(self.c, scale(add(scale(self.u, phi.cos()), scale(self.v, phi.sin())), self.r));
            (norm(sub(p, q)), q)
        } else {
            let q0 = self.point(0.0);
            let q1 = self.point(self.sweep);
            let d0 = norm(sub(p, q0));
            let d1 = norm(sub(p, q1));
            if d0 <= d1 {
                (d0, q0)
            } else {
                (d1, q1)
            }
        }
    }
}

/// Arc-Spline über beide Loops: je `n_arcs` Bögen durch Tripel
/// aufeinanderfolgender uniform-θ-Stützpunkte.
struct EdgeArcSpline {
    arcs: Vec<Arc3d>,
}

impl EdgeArcSpline {
    fn build(n_arcs: usize) -> Self {
        let mut arcs = Vec::new();
        for sx in [1.0, -1.0] {
            let m = 2 * n_arcs; // Stützpunkte (je Bogen 2 Intervalle)
            for i in 0..n_arcs {
                let th = |k: usize| (2 * i + k) as f64 * std::f64::consts::TAU / m as f64;
                arcs.push(Arc3d::through(
                    edge_point(sx, th(0)),
                    edge_point(sx, th(1)),
                    edge_point(sx, th(2)),
                ));
            }
        }
        Self { arcs }
    }

    fn closest(&self, p: [f64; 3]) -> (f64, [f64; 3]) {
        let mut best = f64::INFINITY;
        let mut best_q = p;
        for arc in &self.arcs {
            if norm(sub(p, arc.bound_c)) - arc.bound_r >= best {
                continue;
            }
            let (d, q) = arc.closest(p);
            if d < best {
                best = d;
                best_q = q;
            }
        }
        (best, best_q)
    }
}

trait EdgeApprox: Sync {
    fn closest(&self, p: [f64; 3]) -> (f64, [f64; 3]);
}

impl EdgeApprox for EdgePolyline {
    fn closest(&self, p: [f64; 3]) -> (f64, [f64; 3]) {
        EdgePolyline::closest(self, p)
    }
}

impl EdgeApprox for EdgeArcSpline {
    fn closest(&self, p: [f64; 3]) -> (f64, [f64; 3]) {
        EdgeArcSpline::closest(self, p)
    }
}

/// Max. Abweichung der Approximation von der wahren Kurve
/// (dichte Kurvenpunkte → Distanz zur Approximation).
fn chord_error(approx: &dyn EdgeApprox, dense: usize) -> f64 {
    (0..dense)
        .into_par_iter()
        .map(|i| {
            let th = (i as f64 + 0.5) * std::f64::consts::TAU / dense as f64;
            let q = edge_point(1.0, th);
            approx.closest(q).0
        })
        .reduce(|| 0.0, f64::max)
}

// ── Felder ─────────────────────────────────────────────────────────

/// Exakte Ground Truth: SDF(S↑δ) = SDF(S) − δ (Konvexität).
/// dist(x,S) über die Projektions-Identität:
/// dist = min( [proj_A(x)∈B]·d_A, [proj_B(x)∈A]·d_B, dist(x, Kante) ).
struct TrueField<'a> {
    dense_edge: &'a EdgePolyline,
    delta: f64,
}

impl TrueField<'_> {
    fn dist_s(&self, p: [f64; 3]) -> f64 {
        let va = fa(p);
        let vb = fb(p);
        if va <= 0.0 && vb <= 0.0 {
            return va.max(vb); // innen exakt (max = Intersection innen)
        }
        let mut d = self.dense_edge.closest(p).0;
        if fb(proj_a(p)) <= 1e-9 {
            d = d.min(va.max(0.0));
        }
        if fa(proj_b(p)) <= 1e-9 {
            d = d.min(vb.max(0.0));
        }
        d
    }

    fn eval(&self, p: [f64; 3]) -> f64 {
        self.dist_s(p) - self.delta
    }
}

/// Naiv: Schnitt der offsetteten Blätter (scharfe Kante).
fn f_naiv(p: [f64; 3], delta: f64) -> f64 {
    fa(p).max(fb(p)) - delta
}

/// C1: naiv ∖ Keil, Keil = {f_A>0 ∧ f_B>0 ∧ d_Kante>δ} — das
/// Original-Körper-Gating („naiv ∪/∩ Kanal" aus §9 wörtlich genommen).
fn f_c1(p: [f64; 3], delta: f64, edge: &dyn EdgeApprox) -> f64 {
    let (de, _q) = edge.closest(p);
    let wedge = fa(p).min(fb(p)).min(de - delta);
    f_naiv(p, delta).max(wedge)
}

/// C2: naiv ∖ Keil mit Normalkegel-Gating am nächsten Kantenpunkt
/// (duale Kegelvektoren aus den analytischen Flächennormalen) — die
/// korrekt getrimmte Rossignac–Requicha-Konstruktion.
fn f_c2(p: [f64; 3], delta: f64, edge: &dyn EdgeApprox) -> f64 {
    let (de, q) = edge.closest(p);
    let (na, nb) = edge_normals(q);
    let cg = dot(na, nb);
    let d = sub(p, q);
    let ma = normalize(sub(na, scale(nb, cg))).unwrap_or(na);
    let mb = normalize(sub(nb, scale(na, cg))).unwrap_or(nb);
    let sa = dot(d, ma);
    let sb = dot(d, mb);
    let wedge = sa.min(sb).min(de - delta);
    f_naiv(p, delta).max(wedge)
}

// ── Messungen Teil B ───────────────────────────────────────────────

/// Geführter March entlang eines Strahls auf einem 1-Lipschitz-Feld,
/// erste Nullstelle via Bisektion. Rückgabe t oder None.
fn first_zero_lipschitz(
    f: &dyn Fn([f64; 3]) -> f64,
    o: [f64; 3],
    dir: [f64; 3],
    tmax: f64,
) -> Option<f64> {
    let mut t = 0.0f64;
    let mut g_prev = f(o);
    if g_prev <= 0.0 {
        return Some(0.0);
    }
    loop {
        let step = (g_prev * 0.9).max(1e-4);
        let tn = t + step;
        if tn > tmax {
            return None;
        }
        let gn = f(add(o, scale(dir, tn)));
        if gn <= 0.0 {
            let (mut lo, mut hi) = (t, tn);
            for _ in 0..60 {
                let m = (lo + hi) * 0.5;
                if f(add(o, scale(dir, m))) <= 0.0 {
                    hi = m;
                } else {
                    lo = m;
                }
            }
            return Some(hi);
        }
        t = tn;
        g_prev = gn;
    }
}

/// Erste Vorzeichenwechsel-Stelle eines beliebigen (nicht
/// notwendigerweise Lipschitz-sauberen) Felds in einem Fenster um t0.
fn first_sign_change_window(
    f: &dyn Fn([f64; 3]) -> f64,
    o: [f64; 3],
    dir: [f64; 3],
    t0: f64,
    half_window: f64,
    steps: usize,
) -> Option<f64> {
    let t_start = (t0 - half_window).max(0.0);
    let t_end = t0 + half_window;
    let dt = (t_end - t_start) / steps as f64;
    let mut prev_t = t_start;
    let mut prev_s = f(add(o, scale(dir, t_start))) > 0.0;
    for i in 1..=steps {
        let t = t_start + i as f64 * dt;
        let s = f(add(o, scale(dir, t))) > 0.0;
        if s != prev_s {
            let (mut lo, mut hi) = (prev_t, t);
            for _ in 0..60 {
                let m = (lo + hi) * 0.5;
                if (f(add(o, scale(dir, m))) > 0.0) == prev_s {
                    lo = m;
                } else {
                    hi = m;
                }
            }
            return Some(hi);
        }
        prev_t = t;
        prev_s = s;
    }
    None
}

struct RaySet {
    origins: Vec<[f64; 3]>,
    dirs: Vec<[f64; 3]>,
}

/// Strahlen auf die wahre Offset-Fläche: gemischt kantennah + global.
fn build_rays(n_edge: usize, n_global: usize, delta: f64, seed: u64) -> RaySet {
    let mut rng = SplitMix64::new(seed);
    let mut origins = Vec::new();
    let mut dirs = Vec::new();
    for _ in 0..n_edge {
        let sx = if rng.next_f64() < 0.5 { 1.0 } else { -1.0 };
        let th = rng.range(0.0, std::f64::consts::TAU);
        let q = edge_point(sx, th);
        let u = rng.unit_dir();
        let o = add(q, scale(u, 25.0 + 3.0 * delta));
        let jitter = scale(rng.unit_dir(), rng.range(0.0, 1.5 * delta));
        let target = add(q, jitter);
        if let Some(d) = normalize(sub(target, o)) {
            origins.push(o);
            dirs.push(d);
        }
    }
    for _ in 0..n_global {
        let u = rng.unit_dir();
        let o = scale(u, 95.0);
        let target = [rng.range(-25.0, 25.0), rng.range(-18.0, 18.0), rng.range(-18.0, 18.0)];
        if let Some(d) = normalize(sub(target, o)) {
            origins.push(o);
            dirs.push(d);
        }
    }
    RaySet { origins, dirs }
}

fn part_b() {
    println!("── Teil B: Kanal-Korrektur Steinmetz-Kante ──");
    let dense = EdgePolyline::build(200_000);

    b_validate_gt(&dense);
    b_curve_approx();
    b_zeroset_and_nearfield(&dense);
    b_costs(&dense);
    println!("── Teil B fertig ──");
}

/// GT-Validierung: Projektions-Identität vs. Dykstra vs. Ball-Dilation.
fn b_validate_gt(dense: &EdgePolyline) {
    let tf = TrueField { dense_edge: dense, delta: 0.0 };
    let mut rng = SplitMix64::new(99);
    let pts: Vec<[f64; 3]> = (0..20_000)
        .map(|_| {
            [
                rng.range(-45.0, 45.0),
                rng.range(-35.0, 35.0),
                rng.range(-35.0, 35.0),
            ]
        })
        .collect();
    // (1) Identität vs Dykstra.
    let diffs: Vec<(f64, usize)> = pts
        .par_iter()
        .map(|&p| {
            // Nur der Außenabstand ist vergleichbar: dist_s liefert innen
            // den signierten max-Wert, Dykstra für Innenpunkte 0.
            let d_id = tf.dist_s(p).max(0.0);
            let (d_dy, it) = dykstra_dist(p);
            ((d_id - d_dy).abs(), it)
        })
        .collect();
    let max_diff = diffs.iter().map(|d| d.0).fold(0.0f64, f64::max);
    let mean_iters = diffs.iter().map(|d| d.1).sum::<usize>() as f64 / diffs.len() as f64;
    let max_iters = diffs.iter().map(|d| d.1).max().unwrap();
    println!(
        "  GT-Validierung: |Identitaet - Dykstra| max {:.3e} mm (20k Punkte), Dykstra-Iter mean {:.1} max {}",
        max_diff, mean_iters, max_iters
    );

    // (2) Ball-Dilation-Vorzeichen-Check (Fibonacci-Sphäre, Radien-Shells).
    let delta = 2.0;
    let n_dirs = 1000;
    let golden = std::f64::consts::PI * (3.0 - 5.0f64.sqrt());
    let ball_dirs: Vec<[f64; 3]> = (0..n_dirs)
        .map(|i| {
            let y = 1.0 - 2.0 * (i as f64 + 0.5) / n_dirs as f64;
            let r = (1.0 - y * y).sqrt();
            let phi = golden * i as f64;
            [r * phi.cos(), y, r * phi.sin()]
        })
        .collect();
    let mut rng2 = SplitMix64::new(123);
    let near_pts: Vec<[f64; 3]> = (0..3000)
        .map(|_| {
            let sx = if rng2.next_f64() < 0.5 { 1.0 } else { -1.0 };
            let q = edge_point(sx, rng2.range(0.0, std::f64::consts::TAU));
            let u = rng2.unit_dir();
            add(q, scale(u, delta + rng2.range(-0.6, 0.6)))
        })
        .collect();
    let tfd = TrueField { dense_edge: dense, delta };
    let disagreements: Vec<f64> = near_pts
        .par_iter()
        .filter_map(|&p| {
            let inside_true = tfd.eval(p) <= 0.0;
            let mut inside_ball = fa(p).max(fb(p)) <= 0.0;
            if !inside_ball {
                'outer: for shell in [0.25, 0.5, 0.75, 1.0] {
                    for u in &ball_dirs {
                        if fa(add(p, scale(*u, delta * shell)))
                            .max(fb(add(p, scale(*u, delta * shell))))
                            <= 0.0
                        {
                            inside_ball = true;
                            break 'outer;
                        }
                    }
                }
            }
            if inside_true != inside_ball {
                Some(tfd.eval(p).abs())
            } else {
                None
            }
        })
        .collect();
    let max_margin = disagreements.iter().fold(0.0f64, |a, &b| a.max(b));
    println!(
        "  Ball-Dilation-Check (delta=2, 3000 kantennahe Punkte, 1000 Richtungen x 4 Shells): {} Abweichungen, max |f_true| dabei {:.3e} mm",
        disagreements.len(),
        max_margin
    );
    write_csv(
        &out_dir().join("b_validation.csv"),
        "check,value",
        &[
            format!("identity_vs_dykstra_max_abs_diff_mm,{max_diff:.3e}"),
            format!("dykstra_mean_iters,{mean_iters:.1}"),
            format!("dykstra_max_iters,{max_iters}"),
            format!("ball_sign_disagreements_of_3000,{}", disagreements.len()),
            format!("ball_sign_disagreement_max_margin_mm,{max_margin:.3e}"),
        ],
    )
    .expect("write validation");
}

/// Kurven-Approximationsfehler über der Segment-/Bogenzahl.
fn b_curve_approx() {
    let mut rows = Vec::new();
    println!("  Kurven-Approximation (Fehler in mm, Ziel 0.001):");
    for n in [8usize, 16, 32, 64, 128, 256, 512, 1024, 2048] {
        let pl = EdgePolyline::build(n);
        let e = chord_error(&pl, 400_000);
        println!("    Polyline  N je Loop {:5} -> max {:.6}", n, e);
        rows.push(format!("polyline,{n},{e:.7}"));
    }
    for n in [4usize, 8, 16, 32, 64, 128, 256] {
        let asp = EdgeArcSpline::build(n);
        let e = chord_error(&asp, 400_000);
        println!("    ArcSpline N je Loop {:5} -> max {:.6}", n, e);
        rows.push(format!("arcspline,{n},{e:.7}"));
    }
    write_csv(
        &out_dir().join("b_curve_approx.csv"),
        "kind,n_per_loop,max_err_mm",
        &rows,
    )
    .expect("write curve approx");
}

/// Zero-Set-Fehler + Nahfeld je Variante, δ-Sweep und Segment-Sweep.
fn b_zeroset_and_nearfield(dense: &EdgePolyline) {
    let mut zs_rows = Vec::new();
    let mut nf_rows = Vec::new();

    // Referenz-Approximationen für den Varianten-Vergleich.
    let deltas = [0.5, 2.0, 5.0];
    for &delta in &deltas {
        let tf = TrueField { dense_edge: dense, delta };
        let rays = build_rays(3000, 1000, delta, 0xB00B5 ^ (delta * 100.0) as u64);
        // Wahre Treffpunkte.
        let true_hits: Vec<([f64; 3], [f64; 3], f64)> = rays
            .origins
            .par_iter()
            .zip(rays.dirs.par_iter())
            .filter_map(|(&o, &d)| {
                first_zero_lipschitz(&|p| tf.eval(p), o, d, 200.0).map(|t| (o, d, t))
            })
            .collect();
        println!(
            "  delta={delta}: {} / {} Strahlen treffen die wahre Offset-Flaeche",
            true_hits.len(),
            rays.origins.len()
        );

        let variants: Vec<(String, Box<dyn Fn([f64; 3]) -> f64 + Sync>)> = {
            let mut v: Vec<(String, Box<dyn Fn([f64; 3]) -> f64 + Sync>)> = Vec::new();
            v.push(("naiv".into(), Box::new(move |p| f_naiv(p, delta))));
            for n in [64usize, 256, 1024] {
                let e = EdgePolyline::build(n);
                v.push((
                    format!("c1_poly{n}"),
                    Box::new(move |p| f_c1(p, delta, &e)),
                ));
            }
            for n in [16usize, 64, 128, 256, 512, 1024] {
                let e = EdgePolyline::build(n);
                v.push((
                    format!("c2_poly{n}"),
                    Box::new(move |p| f_c2(p, delta, &e)),
                ));
            }
            for n in [8usize, 16, 32, 64, 128] {
                let e = EdgeArcSpline::build(n);
                v.push((
                    format!("c2_arc{n}"),
                    Box::new(move |p| f_c2(p, delta, &e)),
                ));
            }
            v
        };

        for (name, f) in &variants {
            // Zero-Set: Varianten-Nullstelle nahe des wahren Treffers,
            // Fehler = |f_true| am Varianten-Oberflächenpunkt (f_true
            // ist exaktes SDF ⇒ exakte geometrische Abweichung).
            let (devs, misses): (Vec<f64>, usize) = {
                let results: Vec<Option<f64>> = true_hits
                    .par_iter()
                    .map(|&(o, d, t)| {
                        first_sign_change_window(f, o, d, t, 1.2 * delta + 0.5, 240)
                            .map(|tv| tf.eval(add(o, scale(d, tv))).abs())
                    })
                    .collect();
                let misses = results.iter().filter(|r| r.is_none()).count();
                (results.into_iter().flatten().collect(), misses)
            };
            let s = Stats::from_signed(devs);
            println!(
                "    {name:14} zero-set  mean {:.6}  p95 {:.6}  max {:.6}  (misses {})",
                s.mean, s.p95, s.max, misses
            );
            zs_rows.push(format!(
                "{delta},{name},{},{:.7},{:.7},{:.7},{misses}",
                s.count, s.mean, s.p95, s.max
            ));

            // Nahfeld |f − f_true| im Band um die Kanal-Zone.
            let mut rngb = SplitMix64::new(4711 + (delta * 10.0) as u64);
            let band_pts: Vec<[f64; 3]> = (0..4000)
                .map(|_| {
                    let sx = if rngb.next_f64() < 0.5 { 1.0 } else { -1.0 };
                    let q = edge_point(sx, rngb.range(0.0, std::f64::consts::TAU));
                    let u = rngb.unit_dir();
                    add(q, scale(u, delta + rngb.range(-0.5, 0.5) * delta))
                })
                .collect();
            let errs: Vec<f64> = band_pts
                .par_iter()
                .map(|&p| f(p) - tf.eval(p))
                .collect();
            let sn = Stats::from_signed(errs);
            nf_rows.push(format!(
                "{delta},{name},{},{:.7},{:.7},{:.7},{:.7}",
                sn.count, sn.mean, sn.p95, sn.max, sn.mean_signed
            ));
        }
    }
    write_csv(
        &out_dir().join("b_zeroset.csv"),
        "delta,variant,n_rays,mean_mm,p95_mm,max_mm,misses",
        &zs_rows,
    )
    .expect("write zeroset");
    write_csv(
        &out_dir().join("b_nearfield.csv"),
        "delta,variant,n_pts,mean_abs_mm,p95_mm,max_mm,bias_mm",
        &nf_rows,
    )
    .expect("write nearfield");
}

/// Diagnose: wo im Kanal-Band weicht f_c2 maximal von f_true ab, und
/// welcher Term ist verantwortlich?
fn b_debug_nearfield_outlier() {
    let dense = EdgePolyline::build(200_000);
    let delta = 2.0;
    let tf = TrueField { dense_edge: &dense, delta };
    let edge = EdgePolyline::build(512);
    let mut rngb = SplitMix64::new(4711 + (delta * 10.0) as u64);
    let band_pts: Vec<[f64; 3]> = (0..4000)
        .map(|_| {
            let sx = if rngb.next_f64() < 0.5 { 1.0 } else { -1.0 };
            let q = edge_point(sx, rngb.range(0.0, std::f64::consts::TAU));
            let u = rngb.unit_dir();
            add(q, scale(u, delta + rngb.range(-0.5, 0.5) * delta))
        })
        .collect();
    let mut worst: Vec<([f64; 3], f64)> = band_pts
        .par_iter()
        .map(|&p| (p, (f_c2(p, delta, &edge) - tf.eval(p)).abs()))
        .collect();
    worst.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

    // Außen-/Innen-Split: ist der Außenraum (March-Pfad) sauber?
    let split = |inside: bool| -> Stats {
        Stats::from_signed(
            band_pts
                .par_iter()
                .filter_map(|&p| {
                    let ft = tf.eval(p);
                    if (ft < 0.0) == inside {
                        Some(f_c2(p, delta, &edge) - ft)
                    } else {
                        None
                    }
                })
                .collect::<Vec<f64>>(),
        )
    };
    let so = split(false);
    let si = split(true);
    println!(
        "c2_poly512 Band-Split: AUSSEN mean {:.6} p95 {:.6} max {:.6} bias {:+.6} | INNEN mean {:.6} p95 {:.6} max {:.6} bias {:+.6}",
        so.mean, so.p95, so.max, so.mean_signed, si.mean, si.p95, si.max, si.mean_signed
    );
    let overest: f64 = band_pts
        .par_iter()
        .filter_map(|&p| {
            let ft = tf.eval(p);
            if ft > 0.0 {
                Some(f_c2(p, delta, &edge) - ft)
            } else {
                None
            }
        })
        .reduce(|| f64::NEG_INFINITY, f64::max);
    println!("c2_poly512 max AUSSEN-Ueberschaetzung (March-Gefahr wenn >0): {overest:+.6}");
    for (p, err) in worst.iter().take(8) {
        let p = *p;
        let (de, q) = edge.closest(p);
        let (na, nb) = edge_normals(q);
        let cg = dot(na, nb);
        let d = sub(p, q);
        let ma = normalize(sub(na, scale(nb, cg))).unwrap();
        let mb = normalize(sub(nb, scale(na, cg))).unwrap();
        println!(
            "err {err:.4}  p [{:.2},{:.2},{:.2}]  f_true {:.4}  f_naiv {:.4}  f_c2 {:.4}  fa {:.3} fb {:.3}  d_e {:.3} sA {:.3} sB {:.3}  q* [{:.2},{:.2},{:.2}]",
            p[0], p[1], p[2],
            tf.eval(p),
            f_naiv(p, delta),
            f_c2(p, delta, &edge),
            fa(p), fb(p), de,
            dot(d, ma), dot(d, mb),
            q[0], q[1], q[2],
        );
    }
}

/// Eval-Kosten der Kanal-Felder (ns/Eval, Band-Punkte).
fn b_costs(dense: &EdgePolyline) {
    let delta = 2.0;
    let mut rng = SplitMix64::new(31337);
    let pts: Vec<[f64; 3]> = (0..50_000)
        .map(|_| {
            let sx = if rng.next_f64() < 0.5 { 1.0 } else { -1.0 };
            let q = edge_point(sx, rng.range(0.0, std::f64::consts::TAU));
            add(q, scale(rng.unit_dir(), rng.range(0.0, 3.0 * delta)))
        })
        .collect();
    let bench = |f: &dyn Fn([f64; 3]) -> f64, n: usize| -> f64 {
        let mut acc = 0.0;
        for p in pts.iter().take(2000) {
            acc += f(*p);
        }
        let start = std::time::Instant::now();
        for i in 0..n {
            acc += f(pts[i % pts.len()]);
        }
        std::hint::black_box(acc);
        start.elapsed().as_secs_f64() / n as f64 * 1e9
    };
    let mut rows = Vec::new();
    let ns = bench(&|p| f_naiv(p, delta), 2_000_000);
    println!("  Kosten: naiv {ns:.1} ns/Eval");
    rows.push(format!("naiv,0,{ns:.1}"));
    for n in [64usize, 256, 1024] {
        let e = EdgePolyline::build(n);
        let ns = bench(&|p| f_c2(p, delta, &e), 400_000);
        println!("  Kosten: c2_poly{n} {ns:.1} ns/Eval ({} Segmente gesamt)", 2 * n);
        rows.push(format!("c2_poly,{n},{ns:.1}"));
    }
    for n in [16usize, 64, 128] {
        let e = EdgeArcSpline::build(n);
        let ns = bench(&|p| f_c2(p, delta, &e), 400_000);
        println!("  Kosten: c2_arc{n} {ns:.1} ns/Eval ({} Boegen gesamt)", 2 * n);
        rows.push(format!("c2_arc,{n},{ns:.1}"));
    }
    let tfd = TrueField { dense_edge: dense, delta };
    let ns = bench(&|p| tfd.eval(p), 100_000);
    rows.push(format!("gt_identity_dense200k,200000,{ns:.1}"));
    write_csv(
        &out_dir().join("b_costs.csv"),
        "variant,n_per_loop,ns_per_eval",
        &rows,
    )
    .expect("write costs");
}
