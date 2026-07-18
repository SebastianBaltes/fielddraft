//! A1-Auswahl-Spike „2D-Offset-Bibliothek" (#384, OFFSET-TREE-PLAN §5).
//!
//! Phase A des offset_tree-Plans muss Extrude/Revolve-Profile (Arc-Path-
//! Modell von `sketch2d.rs`: Linien + Kreisbögen + QuadBeziers) geometrisch
//! offsetten und Profil-Booleans auflösen. Direktive: Bibliothek statt
//! Handrollen. Dieser Spike misst die Kandidaten:
//!
//!   * `cavalier_contours` 0.7 — Pure Rust, Polylinien MIT Bulge-Bögen,
//!     Offset + Booleans (Union/Intersection/Difference/Xor).
//!   * `i_overlay` 7.0 — Pure Rust, nur Linien-Polygone (Flatten-Referenz:
//!     wie groß ist der Fehler, wenn Bögen zu Segmenten geflattet werden?).
//!   * Clipper2-Bindings — NICHT gebaut: `clipper2` wrappt `clipper2c-sys`
//!     (C++-Quellcode via cc-Crate) und fällt damit fürs
//!     wasm32-unknown-unknown-Target aus; nur dokumentiert.
//!
//! Testfälle (Plan §5): Bogen-Genauigkeit gegen analytisches r±δ,
//! Rechteck mit Bogen-Ecke, C-Profil-Split in zwei Loops, verschwindende
//! Insel/Loch (Topologiewechsel), Profil-Booleans mit Bogenkanten,
//! Bulge>1-Verhalten (Bögen >180°), grobe Timings.
//!
//! Aufruf:
//!   cargo run -p metricity-lab --release --bin a1_offset_lib_spike
//!
//! Ausgabe: docs/research/sdf-metricity/iter3-offset-tree/a1_offset_lib.csv
//! plus Konsolen-Report. Reiner Experiment-Code, kein Produktionspfad.

use std::f64::consts::PI;
use std::path::PathBuf;
use std::time::Instant;

use cavalier_contours::pline_closed;
use cavalier_contours::polyline::{
    BooleanOp, PlineOffsetOptions, PlineSource, PlineSourceMut, Polyline,
};
use cavalier_contours::shape_algorithms::{Shape, ShapeOffsetOptions};

use i_overlay::mesh::outline::offset::OutlineOffset;
use i_overlay::mesh::style::{LineJoin, OutlineStyle};

use metricity_lab::report::write_csv;

fn out_path() -> PathBuf {
    PathBuf::from("docs/research/sdf-metricity/iter3-offset-tree/a1_offset_lib.csv")
}

// ── Sampling-Helfer: Punkte dicht auf einer cavalier-Polyline ───────────

/// Rekonstruiert Center/Radius/Winkel eines Bulge-Segments (p1→p2, b≠0).
fn bulge_arc(p1: [f64; 2], p2: [f64; 2], b: f64) -> ([f64; 2], f64, f64, f64) {
    let theta = 4.0 * b.atan(); // signierter Sweep, CCW positiv
    let dx = p2[0] - p1[0];
    let dy = p2[1] - p1[1];
    let chord = (dx * dx + dy * dy).sqrt();
    let s = 0.5 * chord;
    let dir = [dx / chord, dy / chord];
    let m = [0.5 * (p1[0] + p2[0]), 0.5 * (p1[1] + p2[1])];
    // center = m + rot90(dir)·(s/tan(θ/2)); rot90(v) = (−vy, vx).
    let dcm = s / (0.5 * theta).tan();
    let center = [m[0] - dir[1] * dcm, m[1] + dir[0] * dcm];
    let radius = (s / (0.5 * theta).sin()).abs();
    let a1 = (p1[1] - center[1]).atan2(p1[0] - center[0]);
    (center, radius, a1, theta)
}

/// `n_per_seg` Sample-Punkte je Segment (Endpunkt exklusive).
fn sample_pline(pl: &Polyline<f64>, n_per_seg: usize) -> Vec<[f64; 2]> {
    let n = pl.vertex_count();
    let mut out = Vec::with_capacity(n * n_per_seg);
    for i in 0..n {
        let v1 = pl.at(i);
        let v2 = pl.at((i + 1) % n);
        let p1 = [v1.x, v1.y];
        let p2 = [v2.x, v2.y];
        if v1.bulge.abs() < 1e-14 {
            for k in 0..n_per_seg {
                let t = k as f64 / n_per_seg as f64;
                out.push([p1[0] + t * (p2[0] - p1[0]), p1[1] + t * (p2[1] - p1[1])]);
            }
        } else {
            let (c, r, a1, theta) = bulge_arc(p1, p2, v1.bulge);
            for k in 0..n_per_seg {
                let t = k as f64 / n_per_seg as f64;
                let a = a1 + t * theta;
                out.push([c[0] + r * a.cos(), c[1] + r * a.sin()]);
            }
        }
    }
    out
}

fn arc_vertex_count(pl: &Polyline<f64>) -> usize {
    (0..pl.vertex_count()).filter(|&i| pl.at(i).bulge.abs() > 1e-12).count()
}

/// max/mean von |‖p−c‖ − r_expected| über Samples.
fn radial_err(samples: &[[f64; 2]], c: [f64; 2], r: f64) -> (f64, f64) {
    let mut max = 0.0f64;
    let mut sum = 0.0f64;
    for p in samples {
        let d = ((p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2)).sqrt();
        let e = (d - r).abs();
        max = max.max(e);
        sum += e;
    }
    (max, sum / samples.len() as f64)
}

struct Csv(Vec<String>);
impl Csv {
    fn row(&mut self, case: &str, lib: &str, metric: &str, value: f64) {
        self.0.push(format!("{case},{lib},{metric},{value:.3e}"));
        println!("  {case} [{lib}] {metric} = {value:.3e}");
    }
    fn note(&mut self, case: &str, lib: &str, metric: &str, value: usize) {
        self.0.push(format!("{case},{lib},{metric},{value}"));
        println!("  {case} [{lib}] {metric} = {value}");
    }
}

// ── T1: Kreis r±δ gegen analytische Ground Truth ────────────────────────

fn t1_circle_accuracy(csv: &mut Csv) {
    println!("\nT1 Kreis-Offset-Genauigkeit (cavalier_contours, exakte Bögen)");
    // Kreis r=0.85 um (0.3,−0.7) aus vier Viertelbögen (bulge = tan(π/8)).
    let c = [0.3, -0.7];
    let r = 0.85;
    let b = (PI / 8.0).tan();
    let pl = pline_closed![
        (c[0] + r, c[1], b),
        (c[0], c[1] + r, b),
        (c[0] - r, c[1], b),
        (c[0], c[1] - r, b)
    ];
    assert!(pl.area() > 0.0, "CCW erwartet");
    // Konvention: positives Offset = links der Laufrichtung = INNEN bei CCW.
    for (name, off, r_exp) in [
        ("t1_erode", 0.2, r - 0.2),
        ("t1_dilate", -0.2, r + 0.2),
    ] {
        let res = pl.parallel_offset(off);
        assert_eq!(res.len(), 1, "{name}: genau ein Loop erwartet");
        let samples = sample_pline(&res[0], 4096);
        let (emax, emean) = radial_err(&samples, c, r_exp);
        csv.row(name, "cavalier", "radial_err_max", emax);
        csv.row(name, "cavalier", "radial_err_mean", emean);
        csv.row(name, "cavalier", "area_err", (res[0].area() - PI * r_exp * r_exp).abs());
        csv.note(name, "cavalier", "arc_vertices", arc_vertex_count(&res[0]));
    }
}

// ── T1b: Bulge > 1 (Bogen > 180°) ───────────────────────────────────────

fn t1b_bulge_gt_one(csv: &mut Csv) {
    println!("\nT1b Bulge>1: 270°-Bogen + 90°-Bogen als Kreis, Offset ±0.25");
    let r = 1.0;
    // (1,0) --270° CCW--> (0,−1) --90° CCW--> (1,0), Kreis um Ursprung.
    let b270 = (270.0f64.to_radians() / 4.0).tan(); // ≈ 2.414
    let b90 = (90.0f64.to_radians() / 4.0).tan();
    let pl = pline_closed![(r, 0.0, b270), (0.0, -r, b90)];
    for (name, off, r_exp) in [
        ("t1b_erode", 0.25, r - 0.25),
        ("t1b_dilate", -0.25, r + 0.25),
    ] {
        let res = pl.parallel_offset(off);
        assert_eq!(res.len(), 1, "{name}: genau ein Loop erwartet");
        let samples = sample_pline(&res[0], 8192);
        let (emax, _) = radial_err(&samples, [0.0, 0.0], r_exp);
        csv.row(name, "cavalier", "radial_err_max", emax);
    }
}

// ── T2: Rechteck mit Bogen-Ecke ─────────────────────────────────────────

fn t2_rect_arc_corner(csv: &mut Csv) {
    println!("\nT2 Rechteck 4×2 mit Fillet r=0.5 an Ecke (4,2), Offset ±0.2");
    let rf = 0.5; // Fillet-Radius, Zentrum (3.5, 1.5)
    let fc = [3.5, 1.5];
    let b = (PI / 8.0).tan(); // 90°-Bogen
    let pl = pline_closed![
        (0.0, 0.0, 0.0),
        (4.0, 0.0, 0.0),
        (4.0, 1.5, b),
        (3.5, 2.0, 0.0),
        (0.0, 2.0, 0.0)
    ];
    let d = 0.2;

    // Dilation (−δ): Fillet-Bogen muss exakt r=0.7 um dasselbe Zentrum sein.
    let res = pl.parallel_offset(-d);
    assert_eq!(res.len(), 1);
    let out = &res[0];
    let fillet_samples: Vec<[f64; 2]> = sample_pline(out, 2048)
        .into_iter()
        .filter(|p| p[0] > fc[0] + 1e-9 && p[1] > fc[1] + 1e-9)
        .collect();
    let (emax, _) = radial_err(&fillet_samples, fc, rf + d);
    csv.row("t2_dilate_fillet", "cavalier", "radial_err_max", emax);
    // Scharfe Ecke (0,0) wird Rundjoin-Bogen r=δ um die Ecke.
    let corner_samples: Vec<[f64; 2]> = sample_pline(out, 2048)
        .into_iter()
        .filter(|p| p[0] < -1e-9 && p[1] < -1e-9)
        .collect();
    let (emax_c, _) = radial_err(&corner_samples, [0.0, 0.0], d);
    csv.row("t2_dilate_corner_join", "cavalier", "radial_err_max", emax_c);
    // Analytische Fläche: Rechteck + Umfangsstreifen + Rundecken.
    // A(+δ) = A0 + P0·δ + π δ² (konvexes Profil; A0/P0 aus der Bibliothek
    // selbst wären zirkulär — hier hart analytisch):
    let a0 = 4.0 * 2.0 - (rf * rf - 0.25 * PI * rf * rf);
    let p0 = 2.0 * (4.0 + 2.0) - (2.0 * rf - 0.5 * PI * rf);
    let a_exp = a0 + p0 * d + PI * d * d;
    csv.row("t2_dilate", "cavalier", "area_err", (out.area() - a_exp).abs());

    // Erosion (+δ): Fillet r=0.3, sonst geschrumpftes Rechteck (scharf).
    let res = pl.parallel_offset(d);
    assert_eq!(res.len(), 1);
    let inn = &res[0];
    let fillet_samples: Vec<[f64; 2]> = sample_pline(inn, 2048)
        .into_iter()
        .filter(|p| p[0] > fc[0] + 1e-9 && p[1] > fc[1] + 1e-9)
        .collect();
    let (emax_i, _) = radial_err(&fillet_samples, fc, rf - d);
    csv.row("t2_erode_fillet", "cavalier", "radial_err_max", emax_i);
    let rf2 = rf - d;
    let a_exp = (4.0 - 2.0 * d) * (2.0 - 2.0 * d) - (rf2 * rf2 - 0.25 * PI * rf2 * rf2);
    csv.row("t2_erode", "cavalier", "area_err", (inn.area() - a_exp).abs());
    csv.note("t2_erode", "cavalier", "arc_vertices", arc_vertex_count(inn));
}

// ── T3: C-Profil spaltet bei Erosion in zwei Loops ──────────────────────

fn t3_c_profile_split(csv: &mut Csv) {
    println!("\nT3 C-Profil (Steg 0.8, Schenkel 1.2), Erosion δ=0.5 → Split");
    let pl = pline_closed![
        (0.0, 0.0, 0.0),
        (3.0, 0.0, 0.0),
        (3.0, 1.2, 0.0),
        (0.8, 1.2, 0.0),
        (0.8, 1.8, 0.0),
        (3.0, 1.8, 0.0),
        (3.0, 3.0, 0.0),
        (0.0, 3.0, 0.0)
    ];
    let res = pl.parallel_offset(0.5);
    csv.note("t3_split", "cavalier", "loop_count", res.len());
    assert_eq!(res.len(), 2, "C-Profil muss in zwei Loops spalten");
    // Wahre Minkowski-Erosion je Schenkel: Rechteck [0.5,2.5]×0.2 PLUS der
    // Rest-Bump am Reflex-Slot-Eck (Bogen r=δ um (0.8,1.2) bzw. (0.8,1.8)):
    // bump = ∫_{-0.3}^{0} (0.5 − √(0.25−u²)) du = 0.09 − 0.125·asin(0.6).
    let bump = 0.09 - 0.125 * 0.6f64.asin();
    let a_exp = 0.4 + bump;
    for (i, r) in res.iter().enumerate() {
        csv.row(&format!("t3_split_loop{i}"), "cavalier", "area_err", (r.area() - a_exp).abs());
    }
}

// ── T4: Insel verschwindet / Loch schließt sich ─────────────────────────

fn t4_topology_vanish(csv: &mut Csv) {
    println!("\nT4 Topologiewechsel: Insel r=0.3 bei δ=0.4 weg; Loch r=0.5 bei Dilation 0.6 zu");
    // (a) Insel: kleiner Kreis erodiert über den Inradius hinaus → leer.
    let b = (PI / 8.0).tan();
    let island = pline_closed![
        (0.3, 0.0, b),
        (0.0, 0.3, b),
        (-0.3, 0.0, b),
        (0.0, -0.3, b)
    ];
    let res = island.parallel_offset(0.4);
    csv.note("t4_island_erode", "cavalier", "loop_count", res.len());
    assert!(res.is_empty(), "Insel muss verschwinden");

    // (b) Shape-API: Platte 4×4 mit Kreisloch r=0.5 im Zentrum; Dilation
    // δ=0.6 > r schließt das Loch. Loch = CW-Polyline (negative Fläche).
    let mut hole = pline_closed![
        (2.5, 2.0, b),
        (2.0, 2.5, b),
        (1.5, 2.0, b),
        (2.0, 1.5, b)
    ];
    hole.invert_direction_mut(); // CW = Loch
    let plate = pline_closed![(0.0, 0.0, 0.0), (4.0, 0.0, 0.0), (4.0, 4.0, 0.0), (0.0, 4.0, 0.0)];
    let shape = Shape::from_plines(vec![plate, hole]);
    let d = 0.6;
    // Shape::parallel_offset: GLEICHE Konvention wie plines — positiv =
    // links der Laufrichtung = innen bei CCW-Außenkontur. Dilation = −δ.
    let grown = shape.parallel_offset(-d, ShapeOffsetOptions::default());
    csv.note("t4_hole_dilate", "cavalier", "ccw_loops", grown.ccw_plines.len());
    csv.note("t4_hole_dilate", "cavalier", "cw_loops(holes)", grown.cw_plines.len());
    assert_eq!(grown.ccw_plines.len(), 1);
    assert_eq!(grown.cw_plines.len(), 0, "Loch muss sich schließen");
    let a_exp = (4.0 + 2.0 * d) * (4.0 + 2.0 * d) - (4.0 - PI) * d * d;
    csv.row(
        "t4_hole_dilate",
        "cavalier",
        "area_err",
        (grown.ccw_plines[0].polyline.area() - a_exp).abs(),
    );
    // Gegenprobe: Erosion δ=0.3 lässt das Loch wachsen (bleibt 1 Loch).
    let shrunk = shape.parallel_offset(0.3, ShapeOffsetOptions::default());
    csv.note("t4_hole_erode", "cavalier", "cw_loops(holes)", shrunk.cw_plines.len());
    assert_eq!(shrunk.cw_plines.len(), 1, "Loch muss beim Erodieren bleiben");
    let hole_samples = sample_pline(&shrunk.cw_plines[0].polyline, 4096);
    let (emax, _) = radial_err(&hole_samples, [2.0, 2.0], 0.5 + 0.3);
    csv.row("t4_hole_erode", "cavalier", "hole_radial_err_max", emax);
}

// ── T5: Profil-Booleans mit Bogenkanten ─────────────────────────────────

fn t5_booleans(csv: &mut Csv) {
    println!("\nT5 Booleans Rechteck [0,2]×[0,1] ⋃/⋂/∖ Kreis r=0.4 um (2,0.5)");
    let rect = pline_closed![(0.0, 0.0, 0.0), (2.0, 0.0, 0.0), (2.0, 1.0, 0.0), (0.0, 1.0, 0.0)];
    let r = 0.4;
    let circle = pline_closed![(2.4, 0.5, 1.0), (1.6, 0.5, 1.0)];
    let a_rect = 2.0;
    let a_circ = PI * r * r;

    let uni = rect.boolean(&circle, BooleanOp::Or);
    assert_eq!(uni.pos_plines.len(), 1);
    assert_eq!(uni.neg_plines.len(), 0);
    let a = uni.pos_plines[0].pline.area();
    csv.row("t5_union", "cavalier", "area_err", (a - (a_rect + 0.5 * a_circ)).abs());
    csv.note("t5_union", "cavalier", "arc_vertices", arc_vertex_count(&uni.pos_plines[0].pline));

    let int = rect.boolean(&circle, BooleanOp::And);
    assert_eq!(int.pos_plines.len(), 1);
    let a = int.pos_plines[0].pline.area();
    csv.row("t5_intersect", "cavalier", "area_err", (a - 0.5 * a_circ).abs());

    let dif = rect.boolean(&circle, BooleanOp::Not);
    assert_eq!(dif.pos_plines.len(), 1);
    // GOTCHA: die Not-Ergebnis-Loop kommt CW zurück (signierte Fläche
    // negativ) — Orientierung ist bei Intersected-Ergebnissen NICHT
    // garantiert. Integration muss per area()-Vorzeichen normalisieren.
    let a = dif.pos_plines[0].pline.area();
    csv.note("t5_difference", "cavalier", "result_ccw", usize::from(a > 0.0));
    csv.row("t5_difference", "cavalier", "area_err", (a.abs() - (a_rect - 0.5 * a_circ)).abs());

    // Halbkreiskante der Differenz muss exakter Bogen r=0.4 bleiben.
    let samples: Vec<[f64; 2]> = sample_pline(&dif.pos_plines[0].pline, 2048)
        .into_iter()
        .filter(|p| {
            let dx = p[0] - 2.0;
            let dy = p[1] - 0.5;
            p[0] < 2.0 - 1e-6 && (dx * dx + dy * dy).sqrt() < r + 0.05
        })
        .collect();
    if !samples.is_empty() {
        let (emax, _) = radial_err(&samples, [2.0, 0.5], r);
        csv.row("t5_difference_arc", "cavalier", "radial_err_max", emax);
    }

    // Loch-erzeugende Differenz: Kreis komplett im Rechteck → neg_plines.
    let inner = pline_closed![(1.2, 0.5, 1.0), (0.8, 0.5, 1.0)];
    let dif2 = rect.boolean(&inner, BooleanOp::Not);
    csv.note("t5_difference_hole", "cavalier", "pos_loops", dif2.pos_plines.len());
    csv.note("t5_difference_hole", "cavalier", "neg_loops(holes)", dif2.neg_plines.len());
    assert_eq!(dif2.neg_plines.len(), 1, "innenliegender Abzug muss Loch liefern");
}

// ── T6: Flatten-Referenz i_overlay ──────────────────────────────────────

fn t6_ioverlay_flatten(csv: &mut Csv) {
    println!("\nT6 i_overlay (Flatten): Kreis r=1 → N Segmente, Dilation δ=0.25 vs. r=1.25");
    for n in [64usize, 256, 1024, 4096] {
        let contour: Vec<[f64; 2]> = (0..n)
            .map(|i| {
                let a = 2.0 * PI * i as f64 / n as f64;
                [a.cos(), a.sin()]
            })
            .collect();
        let style = OutlineStyle::new(0.25).line_join(LineJoin::Round(0.001));
        let t0 = Instant::now();
        let shapes = contour.outline(&style);
        let dt = t0.elapsed().as_secs_f64() * 1e3;
        assert_eq!(shapes.len(), 1);
        let outer = &shapes[0][0];
        let mut emax = 0.0f64;
        // Fehler an den Polygon-Knoten UND Kantenmitten messen (Sagitta).
        for i in 0..outer.len() {
            let p = outer[i];
            let q = outer[(i + 1) % outer.len()];
            for s in [p, [0.5 * (p[0] + q[0]), 0.5 * (p[1] + q[1])]] {
                let d = (s[0] * s[0] + s[1] * s[1]).sqrt();
                emax = emax.max((d - 1.25).abs());
            }
        }
        csv.row(&format!("t6_flat_n{n}"), "i_overlay", "radial_err_max", emax);
        csv.row(&format!("t6_flat_n{n}"), "i_overlay", "offset_ms", dt);
    }
}

// ── T7: Timings cavalier_contours ───────────────────────────────────────

fn t7_timing(csv: &mut Csv) {
    println!("\nT7 Timing cavalier_contours (1000-Segment-Zahnprofil, Offset+Boolean)");
    // Zahnrad-artiges Profil: 500 Linien + 500 Bögen, r≈10.
    let teeth = 500usize;
    let mut pl = Polyline::new_closed();
    for i in 0..teeth {
        let a0 = 2.0 * PI * i as f64 / teeth as f64;
        let a1 = 2.0 * PI * (i as f64 + 0.5) / teeth as f64;
        let r0 = 10.0;
        let r1 = 10.3;
        pl.add(r0 * a0.cos(), r0 * a0.sin(), 0.0);
        pl.add(r1 * a1.cos(), r1 * a1.sin(), 0.15); // flacher Bogen zurück
    }
    let opts = PlineOffsetOptions::default();
    let t0 = Instant::now();
    let res = pl.parallel_offset_opt(0.1, &opts);
    let dt_off = t0.elapsed().as_secs_f64() * 1e3;
    csv.note("t7_offset_1000seg", "cavalier", "loop_count", res.len());
    csv.row("t7_offset_1000seg", "cavalier", "offset_ms", dt_off);

    let mut other = pl.clone();
    other.translate_mut(5.0, 0.4);
    let t0 = Instant::now();
    let uni = pl.boolean(&other, BooleanOp::Or);
    let dt_bool = t0.elapsed().as_secs_f64() * 1e3;
    csv.note("t7_union_1000seg", "cavalier", "pos_loops", uni.pos_plines.len());
    csv.row("t7_union_1000seg", "cavalier", "boolean_ms", dt_bool);

    // Realistische Profilgröße (≈40 Segmente): Offset-Kette wie in Phase A.
    let b = (PI / 8.0).tan();
    let mut small = Polyline::new_closed();
    for i in 0..10 {
        let a0 = 2.0 * PI * i as f64 / 10.0;
        let a1 = 2.0 * PI * (i as f64 + 0.5) / 10.0;
        small.add(2.0 * a0.cos(), 2.0 * a0.sin(), 0.0);
        small.add(2.2 * a1.cos(), 2.2 * a1.sin(), b * 0.3);
    }
    let t0 = Instant::now();
    let reps = 1000;
    let mut acc = 0usize;
    for _ in 0..reps {
        acc += small.parallel_offset(0.05).len();
    }
    let dt = t0.elapsed().as_secs_f64() * 1e6 / reps as f64;
    assert!(acc >= reps);
    csv.row("t7_offset_20seg", "cavalier", "offset_us_per_call", dt);
}

// ── T8: Segment-Provenance (Audit-Kriterium, docs/OFFSET-TREE-AUDIT.md) ─

fn wrap_pos(a: f64) -> f64 {
    let mut a = a % (2.0 * PI);
    if a < 0.0 {
        a += 2.0 * PI;
    }
    a
}

fn dist2(a: [f64; 2], b: [f64; 2]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// Abstand Punkt → Quellsegment (Linie oder Bulge-Bogen), exakt.
fn seg_dist(
    v1: cavalier_contours::polyline::PlineVertex<f64>,
    v2: cavalier_contours::polyline::PlineVertex<f64>,
    p: [f64; 2],
) -> f64 {
    let p1 = [v1.x, v1.y];
    let p2 = [v2.x, v2.y];
    if v1.bulge.abs() < 1e-14 {
        let dx = p2[0] - p1[0];
        let dy = p2[1] - p1[1];
        let len2 = dx * dx + dy * dy;
        let t = (((p[0] - p1[0]) * dx + (p[1] - p1[1]) * dy) / len2).clamp(0.0, 1.0);
        dist2(p, [p1[0] + t * dx, p1[1] + t * dy])
    } else {
        let (c, r, a1, theta) = bulge_arc(p1, p2, v1.bulge);
        let ap = (p[1] - c[1]).atan2(p[0] - c[0]);
        let inside = if theta > 0.0 {
            wrap_pos(ap - a1) <= theta
        } else {
            wrap_pos(a1 - ap) <= -theta
        };
        if inside {
            (dist2(p, c) - r).abs()
        } else {
            dist2(p, p1).min(dist2(p, p2))
        }
    }
}

fn dist_to_pline(p: [f64; 2], pl: &Polyline<f64>) -> f64 {
    let n = pl.vertex_count();
    (0..n).map(|i| seg_dist(pl.at(i), pl.at((i + 1) % n), p)).fold(f64::MAX, f64::min)
}

/// Mittelpunkt eines Ergebnis-Segments (Linie oder Bogen).
fn seg_mid(
    v1: cavalier_contours::polyline::PlineVertex<f64>,
    v2: cavalier_contours::polyline::PlineVertex<f64>,
) -> [f64; 2] {
    let p1 = [v1.x, v1.y];
    let p2 = [v2.x, v2.y];
    if v1.bulge.abs() < 1e-14 {
        [0.5 * (p1[0] + p2[0]), 0.5 * (p1[1] + p2[1])]
    } else {
        let (c, r, a1, theta) = bulge_arc(p1, p2, v1.bulge);
        let a = a1 + 0.5 * theta;
        [c[0] + r * a.cos(), c[1] + r * a.sin()]
    }
}

fn t8_provenance(csv: &mut Csv) {
    println!("\nT8 Segment-Provenance: Rückprojektion (Offset) + subslices (Boolean)");
    // (a) Offset: jedes Ergebnis-Segment liegt exakt bei Abstand δ zu GENAU
    // einem Quellsegment (→ Segment-Provenance) oder zu ZWEI Quellsegmenten
    // gleichzeitig, d. h. zum gemeinsamen Vertex (→ Join-Bogen, Ecken-
    // Provenance). Kein Fuzzy-Matching nötig — der Abstand ist konstruktiv δ.
    let b = (PI / 8.0).tan();
    let pl = pline_closed![
        (0.0, 0.0, 0.0),
        (4.0, 0.0, 0.0),
        (4.0, 1.5, b),
        (3.5, 2.0, 0.0),
        (0.0, 2.0, 0.0)
    ];
    let d = 0.2;
    let res = pl.parallel_offset(-d); // Dilation → Join-Bögen an 3 Ecken
    assert_eq!(res.len(), 1);
    let out = &res[0];
    let n_src = pl.vertex_count();
    let (mut seg_mapped, mut join_mapped) = (0usize, 0usize);
    let mut errmax = 0.0f64;
    for i in 0..out.vertex_count() {
        let mid = seg_mid(out.at(i), out.at((i + 1) % out.vertex_count()));
        let dists: Vec<f64> =
            (0..n_src).map(|j| seg_dist(pl.at(j), pl.at((j + 1) % n_src), mid)).collect();
        let dmin = dists.iter().cloned().fold(f64::MAX, f64::min);
        errmax = errmax.max((dmin - d).abs());
        let near = dists.iter().filter(|&&x| x < dmin + 1e-9).count();
        if near >= 2 {
            join_mapped += 1;
        } else {
            seg_mapped += 1;
        }
    }
    csv.note("t8_offset_prov", "cavalier", "seg_mapped", seg_mapped);
    csv.note("t8_offset_prov", "cavalier", "join_mapped", join_mapped);
    csv.row("t8_offset_prov", "cavalier", "backproj_dist_err_max", errmax);
    assert_eq!(seg_mapped, 5, "alle 5 Quellsegmente müssen sich wiederfinden");
    assert_eq!(join_mapped, 3, "3 scharfe Ecken → 3 Join-Bögen");

    // (b) Boolean: BooleanResultPline.subslices liefert die Provenance
    // DIREKT aus der API — je Slice Quell-Polyline (source_is_pline1) plus
    // Vertex-Index-Fenster (view_data.start_index/end_index_offset).
    let rect = pline_closed![(0.0, 0.0, 0.0), (2.0, 0.0, 0.0), (2.0, 1.0, 0.0), (0.0, 1.0, 0.0)];
    let circle = pline_closed![(2.4, 0.5, 1.0), (1.6, 0.5, 1.0)];
    let uni = rect.boolean(&circle, BooleanOp::Or);
    let rp = &uni.pos_plines[0];
    let n1 = rp.subslices.iter().filter(|s| s.source_is_pline1).count();
    let n2 = rp.subslices.len() - n1;
    csv.note("t8_bool_prov", "cavalier", "slices_from_pline1", n1);
    csv.note("t8_bool_prov", "cavalier", "slices_from_pline2", n2);
    let mut errmax = 0.0f64;
    for s in &rp.subslices {
        let src = if s.source_is_pline1 { &rect } else { &circle };
        let sp = s.view_data.updated_start;
        let ep = s.view_data.end_point;
        errmax = errmax.max(dist_to_pline([sp.x, sp.y], src));
        errmax = errmax.max(dist_to_pline([ep.x, ep.y], src));
    }
    csv.row("t8_bool_prov", "cavalier", "slice_on_source_err_max", errmax);
    assert!(n1 >= 1 && n2 >= 1, "Union muss Slices beider Quellen tragen");
}

fn main() {
    let mut csv = Csv(Vec::new());
    println!("A1-Spike 2D-Offset-Bibliothek — cavalier_contours 0.7 vs i_overlay 7.0");
    println!("(Offset-Konvention cavalier: positiv = links der Laufrichtung = innen bei CCW)");
    t1_circle_accuracy(&mut csv);
    t1b_bulge_gt_one(&mut csv);
    t2_rect_arc_corner(&mut csv);
    t3_c_profile_split(&mut csv);
    t4_topology_vanish(&mut csv);
    t5_booleans(&mut csv);
    t6_ioverlay_flatten(&mut csv);
    t7_timing(&mut csv);
    t8_provenance(&mut csv);
    write_csv(&out_path(), "case,lib,metric,value", &csv.0).expect("write csv");
    println!("\nCSV → {}", out_path().display());
}
