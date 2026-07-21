//! #393 — Empirische f32-Präzisions-Messreihe (Phase 1: NUR messen).
//!
//! Fragt: hält der f32-Kernel-Eval das 0.001-mm-Exportbudget bei ~300 mm
//! Bauraum? Statt pauschal auf f64 umzustellen, misst dieser Harness den
//! REINEN Präzisionsverlust — er stellt dem f32-Pfad einen algorithmisch
//! IDENTISCHEN f64-Spiegel gegenüber (gleiche Formeln, nur breiter). Die
//! Differenz `|f32 − f64|` an denselben Punkten IST der Rundungsfehler des
//! f32-Arithmetikpfades (kein Algorithmus-Unterschied).
//!
//! Der f64-Spiegel ([`mirror_sdf`]) transkribiert 1:1:
//!   * die Primitiv-Formeln aus `kernel/src/sdf_eval.rs::sdf_primitive_generic`
//!     (Box/Sphere/Cylinder/Torus/Cone) mit purer `sqrt` (= `SdfSqrt` für f32),
//!   * `kernel/src/transform.rs::{quat_inverse_rotate, world_to_local}` und
//!     `Transform::min_scale`,
//!   * die Boolean-/Offset-/Shell-Arme aus `kernel/src/csg.rs::sdf_open`
//!     (`Union=min`, `Difference=max(a,-b)`, `Intersection=max`,
//!     `Offset=inner-δ`, `Shell=shell_wall_sdf`).
//!
//! Metrik-Isolation: beide Evaluatoren bekommen DENSELBEN f32-gerundeten
//! Punkt `p_f32` (der f32-Pfad sieht real nie mehr als f32-Koordinaten —
//! Mesher-Grid/Ray-March sind f32). So misst `|f32−f64|` rein die
//! Arithmetik-Breite, nicht die Eingangs-Quantisierung; letztere wird
//! separat als analytischer Boden (`pos·2⁻²⁴`) ausgewiesen.
//!
//! Läufe:
//!   * `cargo test -p metricity-lab --test f393_f64_precision` — M1/M2/M3
//!     (schnell, CI-tauglich).
//!   * `cargo test -p metricity-lab --test f393_f64_precision -- --ignored`
//!     — M1-Vollmatrix + M4 (Mesh-Export, langlaufend).

use fielddraft_kernel::bvh::BoundedCsg;
use fielddraft_kernel::csg::{CsgNode, EvalMode};
use fielddraft_kernel::sdf_compiler::{compile_body, eval_opcodes_with_volumes, CompiledSdf};
use fielddraft_protocol::{FeatureId, Primitive, Transform};

use metricity_lab::field::Field;
use metricity_lab::ground_truth::{GroundTruth, GtOptions};
use metricity_lab::util::SplitMix64;

// ─────────────────────────────────────────────────────────────────────────
// f64-Spiegel-Evaluator (algorithmisch identisch zum f32-Pfad, nur f64)
// ─────────────────────────────────────────────────────────────────────────

/// Spiegel von `transform.rs::quat_inverse_rotate` in f64.
fn quat_inverse_rotate_f64(q: [f32; 4], v: [f64; 3]) -> [f64; 3] {
    let qx = -(q[0] as f64);
    let qy = -(q[1] as f64);
    let qz = -(q[2] as f64);
    let qw = q[3] as f64;
    let t = [
        2.0 * (qy * v[2] - qz * v[1]),
        2.0 * (qz * v[0] - qx * v[2]),
        2.0 * (qx * v[1] - qy * v[0]),
    ];
    [
        v[0] + qw * t[0] + qy * t[2] - qz * t[1],
        v[1] + qw * t[1] + qz * t[0] - qx * t[2],
        v[2] + qw * t[2] + qx * t[1] - qy * t[0],
    ]
}

/// Spiegel von `transform.rs::world_to_local` in f64.
fn world_to_local_f64(t: &Transform, p: [f64; 3]) -> [f64; 3] {
    let p = [
        p[0] - t.translate[0] as f64,
        p[1] - t.translate[1] as f64,
        p[2] - t.translate[2] as f64,
    ];
    let p = quat_inverse_rotate_f64(t.rotation, p);
    [
        p[0] / t.scale[0] as f64,
        p[1] / t.scale[1] as f64,
        p[2] / t.scale[2] as f64,
    ]
}

fn min_scale_f64(t: &Transform) -> f64 {
    (t.scale[0].min(t.scale[1]).min(t.scale[2])) as f64
}

/// Spiegel von `sdf_eval.rs::sdf_primitive_generic` in f64. Pure `sqrt`
/// (= die f32-Referenz-`SdfSqrt`, KEIN safe_sqrt-Epsilon). Params werden
/// aus dem f32-Knoten gehoben — genau wie der f32-Pfad sie liest; damit
/// misst der Vergleich die Eval-Präzision, nicht die Param-Präzision (das
/// ist M3).
fn mirror_primitive(prim: &Primitive, p: [f64; 3]) -> f64 {
    match prim {
        Primitive::Box { center, half_extents } => {
            let q = [
                (p[0] - center[0] as f64).abs() - half_extents[0] as f64,
                (p[1] - center[1] as f64).abs() - half_extents[1] as f64,
                (p[2] - center[2] as f64).abs() - half_extents[2] as f64,
            ];
            let q0 = q[0].max(0.0);
            let q1 = q[1].max(0.0);
            let q2 = q[2].max(0.0);
            let outside = (q0 * q0 + q1 * q1 + q2 * q2).sqrt();
            let qmax = q[0].max(q[1]).max(q[2]);
            let inside = qmax.min(0.0);
            outside + inside
        }
        Primitive::Sphere { center, radius } => {
            let dx = p[0] - center[0] as f64;
            let dy = p[1] - center[1] as f64;
            let dz = p[2] - center[2] as f64;
            (dx * dx + dy * dy + dz * dz).sqrt() - *radius as f64
        }
        Primitive::Cylinder { center, radius, half_height } => {
            let dx = p[0] - center[0] as f64;
            let dy = p[1] - center[1] as f64;
            let dz = p[2] - center[2] as f64;
            let radial = (dx * dx + dz * dz).sqrt() - *radius as f64;
            let axial = dy.abs() - *half_height as f64;
            let r_clamped = radial.max(0.0);
            let a_clamped = axial.max(0.0);
            let outside = (r_clamped * r_clamped + a_clamped * a_clamped).sqrt();
            let inside = radial.max(axial).min(0.0);
            outside + inside
        }
        Primitive::Torus { center, major_radius, minor_radius } => {
            let dx = p[0] - center[0] as f64;
            let dy = p[1] - center[1] as f64;
            let dz = p[2] - center[2] as f64;
            let q0 = (dx * dx + dz * dz).sqrt() - *major_radius as f64;
            let q1 = dy;
            (q0 * q0 + q1 * q1).sqrt() - *minor_radius as f64
        }
        Primitive::Cone { r_bottom, r_top, half_height } => {
            let r = (p[0] * p[0] + p[2] * p[2]).sqrt();
            let hh = *half_height as f64;
            let two_hh = hh * 2.0;
            let t_raw = (p[1] + hh) / two_hh;
            let t = t_raw.clamp(0.0, 1.0);
            let r_at_y = *r_bottom as f64 * (1.0 - t) + *r_top as f64 * t;
            let dr = (*r_bottom - *r_top) as f64;
            let two_hh_f = (2.0 * *half_height) as f64;
            let slant_len = (dr * dr + two_hh_f * two_hh_f).sqrt();
            let wall_scale = if slant_len > 1e-9 { two_hh_f / slant_len } else { 1.0 };
            let d_wall = (r - r_at_y) * wall_scale;
            let d_axial = p[1].abs() - hh;
            if d_wall > 0.0 && d_axial > 0.0 {
                (d_wall * d_wall + d_axial * d_axial).sqrt()
            } else if d_wall > d_axial {
                d_wall
            } else {
                d_axial
            }
        }
        other => panic!("mirror_primitive: {other:?} nicht gespiegelt (Szene außerhalb des M1-Umfangs)"),
    }
}

/// Spiegel von `csg.rs::CsgNode::sdf_open` (EvalMode::Visual, hard booleans)
/// in f64. Deckt genau die Knotentypen der Messszenen ab.
fn mirror_sdf(node: &CsgNode, p: [f64; 3]) -> f64 {
    match node {
        CsgNode::Empty => f64::INFINITY,
        CsgNode::Leaf { primitive, .. } => mirror_primitive(primitive, p),
        CsgNode::Transform(t, inner) => {
            let local = world_to_local_f64(t, p);
            mirror_sdf(inner, local) * min_scale_f64(t)
        }
        CsgNode::Union(a, b) => mirror_sdf(a, p).min(mirror_sdf(b, p)),
        CsgNode::Difference(a, b) => mirror_sdf(a, p).max(-mirror_sdf(b, p)),
        CsgNode::Intersection(a, b) => mirror_sdf(a, p).max(mirror_sdf(b, p)),
        CsgNode::Offset(inner, delta) => mirror_sdf(inner, p) - *delta as f64,
        CsgNode::Shell(inner, thickness, faces) => {
            assert!(faces.is_empty(), "mirror: nur geschlossene Shells (faces leer)");
            let d = mirror_sdf(inner, p);
            // shell_wall_sdf(d, d_ret=d, t) = d.max(-(d + t))
            d.max(-(d + *thickness as f64))
        }
        other => panic!("mirror_sdf: Knoten {other:?} nicht gespiegelt"),
    }
}

/// [`Field`]-Wrapper um den f64-Spiegel (für GroundTruth-Oberflächenwolke).
struct MirrorField<'a>(&'a CsgNode);
impl Field for MirrorField<'_> {
    fn eval(&self, p: [f64; 3]) -> f64 {
        mirror_sdf(self.0, p)
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Bau-Helfer
// ─────────────────────────────────────────────────────────────────────────

fn id(n: u32) -> FeatureId {
    FeatureId::from_u32(n)
}

fn leaf_box(fid: u32, half: [f32; 3]) -> CsgNode {
    CsgNode::Leaf { id: id(fid), primitive: Primitive::Box { center: [0.0; 3], half_extents: half } }
}

fn axis_angle_quat(axis: [f64; 3], ang: f64) -> [f32; 4] {
    let n = (axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]).sqrt();
    let a = [axis[0] / n, axis[1] / n, axis[2] / n];
    let s = (ang * 0.5).sin();
    let c = (ang * 0.5).cos();
    [(a[0] * s) as f32, (a[1] * s) as f32, (a[2] * s) as f32, c as f32]
}

fn xform(t: Transform, inner: CsgNode) -> CsgNode {
    CsgNode::Transform(t, Box::new(inner))
}

// ─────────────────────────────────────────────────────────────────────────
// Präzisions-Messkern
// ─────────────────────────────────────────────────────────────────────────

struct Stats {
    p50: f64,
    p99: f64,
    max: f64,
    n: usize,
    model_size: f64,
}

fn percentiles(mut errs: Vec<f64>, model_size: f64) -> Stats {
    errs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = errs.len();
    let pct = |q: f64| errs[((n as f64 * q) as usize).min(n - 1)];
    Stats { p50: pct(0.50), p99: pct(0.99), max: *errs.last().unwrap(), n, model_size }
}

/// Baut die f64-Spiegel-Oberflächenwolke und gibt bis zu `cap` deterministisch
/// gestreute Oberflächenpunkte zurück.
fn surface_probe(node: &CsgNode, grid: usize, cap: usize, seed: u64) -> (Vec<[f64; 3]>, f64) {
    let field = MirrorField(node);
    let a = node.aabb();
    assert!(!a.is_empty(), "leeres AABB");
    let lo = [a.min[0] as f64, a.min[1] as f64, a.min[2] as f64];
    let hi = [a.max[0] as f64, a.max[1] as f64, a.max[2] as f64];
    let opts = GtOptions { grid, refine: false, ..GtOptions::default() };
    let gt = GroundTruth::build(&field, lo, hi, &opts);
    let surf = gt.surface_points();
    let mut rng = SplitMix64::new(seed ^ 0xF393_5EED);
    let probe: Vec<[f64; 3]> =
        (0..cap.min(surf.len())).map(|_| surf[(rng.next_u64() as usize) % surf.len()]).collect();
    (probe, gt.model_size)
}

/// M1-Kern: `|f32_tree_eval − f64_mirror|` an Oberflächenpunkten, beide am
/// SELBEN f32-gerundeten Punkt (reine Arithmetik-Breite).
fn measure_tree_eval(node: &CsgNode, grid: usize, cap: usize, seed: u64) -> Stats {
    let (probe, model_size) = surface_probe(node, grid, cap, seed);
    let errs: Vec<f64> = probe
        .iter()
        .map(|&p| {
            let pf = [p[0] as f32, p[1] as f32, p[2] as f32];
            let f32v = node.sdf(pf, EvalMode::Visual) as f64;
            let f64v = mirror_sdf(node, [pf[0] as f64, pf[1] as f64, pf[2] as f64]);
            (f32v - f64v).abs()
        })
        .collect();
    percentiles(errs, model_size)
}

/// M2-Kern: kompiliertes Opcode-Tape (f32) vs f64-Spiegel, sowie Tape vs
/// direkter Baum-Eval (isoliert den Tape-Zusatzverlust).
fn measure_tape(node: &CsgNode, grid: usize, cap: usize, seed: u64) -> (Stats, Stats) {
    let (probe, model_size) = surface_probe(node, grid, cap, seed);
    let bounded = BoundedCsg::from_csg(node);
    let compiled: CompiledSdf = compile_body(&bounded);
    let mut errs_truth = Vec::with_capacity(probe.len());
    let mut errs_vs_tree = Vec::with_capacity(probe.len());
    for &p in &probe {
        let pf = [p[0] as f32, p[1] as f32, p[2] as f32];
        let tape = eval_opcodes_with_volumes(&compiled.opcodes, pf, &compiled.volumes) as f64;
        let f64v = mirror_sdf(node, [pf[0] as f64, pf[1] as f64, pf[2] as f64]);
        let tree = node.sdf(pf, EvalMode::Visual) as f64;
        errs_truth.push((tape - f64v).abs());
        errs_vs_tree.push((tape - tree).abs());
    }
    (percentiles(errs_truth, model_size), percentiles(errs_vs_tree, model_size))
}

fn line(label: &str, s: &Stats) -> String {
    format!(
        "{label:<46} n={:>6} size={:>7.1}  p50={:>10.3e}  p99={:>10.3e}  max={:>10.3e}  mm",
        s.n, s.model_size, s.p50, s.p99, s.max
    )
}

const BUDGET: f64 = 1.0e-3;

// ─────────────────────────────────────────────────────────────────────────
// Szenen-Katalog
// ─────────────────────────────────────────────────────────────────────────

/// Boolean-Kette der Tiefe `depth`: eine große Box, unioniert mit `depth`
/// kleinen, an die Oberfläche gesetzten Boxen (links-lehnender Baum).
/// Testet reine min/max-Tiefe OHNE Transform-Kaskade.
fn boolean_chain(size: f32, depth: usize) -> CsgNode {
    let h = size * 0.5;
    let mut node = leaf_box(1, [h, h, h]);
    for i in 0..depth {
        let s = h * 0.25;
        let off = h - s * 0.5;
        let sign = if i % 2 == 0 { 1.0 } else { -1.0 };
        let bump = xform(
            Transform {
                translate: [off * sign, (i as f32 * 0.13).sin() * h * 0.3, 0.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0; 3],
            },
            leaf_box(10 + i as u32, [s, s, s]),
        );
        node = CsgNode::Union(Box::new(node), Box::new(bump));
    }
    node
}

/// Transform-Kaskade der Tiefe `depth`: `depth` verschachtelte rotierte +
/// translierte Transforms um eine Box (jede Ebene = ein `world_to_local`).
/// Die Netto-Transform bleibt beschränkt (kleine Winkel/Wege).
fn transform_cascade(size: f32, depth: usize, seed: u64) -> CsgNode {
    let h = size * 0.5;
    let mut node = leaf_box(1, [h, h, h]);
    let mut rng = SplitMix64::new(seed ^ 0xCA5C_0DE);
    for _ in 0..depth {
        let axis = rng.unit_dir();
        let ang = rng.range(-0.4, 0.4);
        let tr = [
            rng.range(-size as f64 * 0.2, size as f64 * 0.2) as f32,
            rng.range(-size as f64 * 0.2, size as f64 * 0.2) as f32,
            rng.range(-size as f64 * 0.2, size as f64 * 0.2) as f32,
        ];
        node = xform(
            Transform { translate: tr, rotation: axis_angle_quat(axis, ang), scale: [1.0; 3] },
            node,
        );
    }
    node
}

/// Verschiebt einen Körper rigide um `dist` entlang der Raumdiagonale.
fn translate_far(node: CsgNode, dist: f64) -> CsgNode {
    let c = (dist / 3.0_f64.sqrt()) as f32;
    xform(
        Transform { translate: [c, c, c], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0; 3] },
        node,
    )
}

/// Dünne Wand via geschlossener Shell (Auslöschungs-Kandidat): Box, Wand `t`.
fn thin_shell(size: f32, wall: f32) -> CsgNode {
    let h = size * 0.5;
    CsgNode::Shell(Box::new(leaf_box(1, [h, h, h])), wall, Vec::new())
}

// ─────────────────────────────────────────────────────────────────────────
// M1 — Kernel-Eval-Präzision
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn m1_tree_eval_precision() {
    let grid = 44;
    let cap = 4000;
    let mut out = String::from("\n=== M1 Kernel-Baum-Eval f32 vs f64 (|f32−f64| in mm) ===\n");
    let mut worst_p99 = 0.0f64;

    // (a) Modellgröße am Ursprung.
    for &s in &[10.0f32, 100.0, 300.0] {
        let node = leaf_box(1, [s * 0.5, s * 0.5, s * 0.5]);
        let st = measure_tree_eval(&node, grid, cap, s as u64);
        out += &line(&format!("(a) Box {s}mm @Ursprung"), &st);
        out += "\n";
        worst_p99 = worst_p99.max(st.p99);
    }

    // (b) Position: 300-mm-Box am Ursprung / +300 / +1000.
    for &d in &[0.0f64, 300.0, 1000.0] {
        let node = translate_far(leaf_box(1, [150.0, 150.0, 150.0]), d);
        let st = measure_tree_eval(&node, grid, cap, (d as u64) ^ 0x11);
        out += &line(&format!("(b) Box 300mm @pos {d}mm"), &st);
        out += &format!("   [coord-quant-Boden ~{:.2e} mm]\n", d * 2f64.powi(-24));
        worst_p99 = worst_p99.max(st.p99);
    }

    // (c) Boolean-Ketten-Tiefe (reine min/max), 300 mm, @pos 300.
    for &depth in &[1usize, 5, 10, 20] {
        let node = translate_far(boolean_chain(300.0, depth), 300.0);
        let st = measure_tree_eval(&node, grid, cap, depth as u64 ^ 0x22);
        out += &line(&format!("(c) Bool-Kette Tiefe {depth} @pos 300"), &st);
        out += "\n";
        worst_p99 = worst_p99.max(st.p99);
    }

    // (d) Transform-Kaskaden-Tiefe (verschachtelte world_to_local), 100 mm.
    for &depth in &[1usize, 5, 10] {
        let node = transform_cascade(100.0, depth, depth as u64);
        let st = measure_tree_eval(&node, grid, cap, depth as u64 ^ 0x33);
        out += &line(&format!("(d) Transform-Kaskade Tiefe {depth}"), &st);
        out += "\n";
        worst_p99 = worst_p99.max(st.p99);
    }
    // (d') Transform-Kaskade @pos 1000 (Kaskade + große Translation).
    for &depth in &[1usize, 5, 10] {
        let node = translate_far(transform_cascade(100.0, depth, depth as u64), 1000.0);
        let st = measure_tree_eval(&node, grid, cap, depth as u64 ^ 0x44);
        out += &line(&format!("(d') Transform-Kaskade T{depth} @pos 1000"), &st);
        out += "\n";
        worst_p99 = worst_p99.max(st.p99);
    }

    // (e) Auslöschung: dünne Wände (Shell) 0.5 mm bei 100/300 mm, @pos 300.
    for &(size, wall) in &[(100.0f32, 0.5f32), (300.0, 0.5), (300.0, 1.0)] {
        let node = translate_far(thin_shell(size, wall), 300.0);
        let st = measure_tree_eval(&node, grid, cap, (size as u64) ^ (wall as u64));
        out += &line(&format!("(e) Shell {size}mm Wand {wall}mm @pos 300"), &st);
        out += "\n";
        worst_p99 = worst_p99.max(st.p99);
    }

    out += &format!(
        "worst p99 = {worst_p99:.3e} mm   Budget = {BUDGET:.0e} mm   Reserve = {:.0}×\n",
        BUDGET / worst_p99.max(f64::MIN_POSITIVE)
    );
    eprintln!("{out}");
    let _ = std::fs::write("/tmp/f393_m1.txt", &out);

    // Sanity, kein Vertragstest: der reine Arithmetik-Eval bleibt weit unter
    // Budget (das IST das erwartete Ergebnis). Großzügige Schranke, damit ein
    // echtes Regress (f32-Eval-Katastrophe) auffällt.
    assert!(worst_p99 < BUDGET, "M1: f32-Eval p99 {worst_p99:.3e} überschreitet Budget");
}

// ─────────────────────────────────────────────────────────────────────────
// M2 — Opcode-Tape vs Baum-Eval
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn m2_tape_vs_tree() {
    let grid = 44;
    let cap = 4000;
    let mut out = String::from(
        "\n=== M2 Opcode-Tape (f32) vs f64-Spiegel [truth] und vs Baum-Eval [tape-δ] ===\n",
    );
    let cases: &[(&str, CsgNode)] = &[
        ("Box 300mm @Ursprung", leaf_box(1, [150.0, 150.0, 150.0])),
        ("Box 300mm @pos 300", translate_far(leaf_box(1, [150.0, 150.0, 150.0]), 300.0)),
        ("Box 300mm @pos 1000", translate_far(leaf_box(1, [150.0, 150.0, 150.0]), 1000.0)),
        ("Bool-Kette T10 @pos 300", translate_far(boolean_chain(300.0, 10), 300.0)),
        ("Transform-Kaskade T10 @pos1000", translate_far(transform_cascade(100.0, 10, 7), 1000.0)),
        ("Shell 300mm Wand0.5 @pos 300", translate_far(thin_shell(300.0, 0.5), 300.0)),
    ];
    let mut worst = 0.0f64;
    for (label, node) in cases {
        let (truth, vs_tree) = measure_tape(node, grid, cap, 0xAB);
        out += &line(&format!("[truth] {label}"), &truth);
        out += "\n";
        out += &line(&format!("[tapeδ] {label}"), &vs_tree);
        out += "\n";
        worst = worst.max(truth.p99);
    }
    out += &format!("worst tape-truth p99 = {worst:.3e} mm  Budget = {BUDGET:.0e} mm\n");
    eprintln!("{out}");
    let _ = std::fs::write("/tmp/f393_m2.txt", &out);
    assert!(worst < BUDGET, "M2: Tape-truth p99 {worst:.3e} überschreitet Budget");
}

// ─────────────────────────────────────────────────────────────────────────
// M3 — Konstruktions-Arithmetik (offset_fold: v − δ)
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn m3_construction_arithmetic() {
    let mut out = String::from(
        "\n=== M3 Konstruktions-Arithmetik: geschrumpfter Parameter v−δ (f32 vs f64) ===\n\
         (offset_fold::shrink_param = offset_scalar_inward = v − δ; cavalier_contours rechnet f64)\n",
    );

    // Einzel-Subtraktion über Parameter-Magnituden (Radien/Halbhöhen bis 150 mm).
    let mut worst_single = 0.0f64;
    for &v in &[5.0f64, 25.0, 75.0, 150.0] {
        for &delta in &[0.1f64, 0.5, 2.0] {
            let f32r = (v as f32 - delta as f32) as f64;
            let f64r = v - delta;
            let err = (f32r - f64r).abs();
            worst_single = worst_single.max(err);
            out += &format!("  v={v:>6.1}  δ={delta:>4.1}  →  |Δ| = {err:.3e} mm\n");
        }
    }

    // Kaskadierte Erosion (Offset über Offset über …): N sequentielle f32-Subtraktionen.
    out += "  --- kaskadiert (N sequentielle f32-Subtraktionen von v=150) ---\n";
    let mut worst_casc = 0.0f64;
    for &n in &[1usize, 5, 10, 20] {
        let delta = 1.234_f64;
        let mut f32acc = 150.0f32;
        let mut f64acc = 150.0f64;
        for _ in 0..n {
            f32acc -= delta as f32;
            f64acc -= delta;
        }
        let err = (f32acc as f64 - f64acc).abs();
        worst_casc = worst_casc.max(err);
        out += &format!("  N={n:>3}  →  |Δ| = {err:.3e} mm\n");
    }

    out += &format!(
        "worst single |Δ| = {worst_single:.3e} mm,  worst kaskadiert |Δ| = {worst_casc:.3e} mm  \
         (Budget {BUDGET:.0e} mm)\n"
    );
    eprintln!("{out}");
    let _ = std::fs::write("/tmp/f393_m3.txt", &out);
    assert!(worst_single.max(worst_casc) < BUDGET, "M3: Konstruktions-Arithmetik über Budget");
}

// ─────────────────────────────────────────────────────────────────────────
// M1-Vollmatrix (langlaufend) — höhere Punktdichte
// ─────────────────────────────────────────────────────────────────────────

#[test]
#[ignore = "langlaufend: M1 mit ~10^4 Punkten je Bedingung"]
fn m1_tree_eval_precision_full() {
    let grid = 72;
    let cap = 12000;
    let mut out = String::from("\n=== M1 VOLL (grid=72, cap=12k) ===\n");
    let scenes: Vec<(String, CsgNode)> = vec![
        ("Box 300mm @Ursprung".into(), leaf_box(1, [150.0, 150.0, 150.0])),
        ("Box 300mm @pos 300".into(), translate_far(leaf_box(1, [150.0, 150.0, 150.0]), 300.0)),
        ("Box 300mm @pos 1000".into(), translate_far(leaf_box(1, [150.0, 150.0, 150.0]), 1000.0)),
        ("Bool-Kette T20 @pos 300".into(), translate_far(boolean_chain(300.0, 20), 300.0)),
        ("Transform-Kaskade T10 @pos1000".into(), translate_far(transform_cascade(100.0, 10, 7), 1000.0)),
        ("Shell 300mm Wand0.5 @pos 300".into(), translate_far(thin_shell(300.0, 0.5), 300.0)),
    ];
    for (label, node) in &scenes {
        let st = measure_tree_eval(node, grid, cap, 0x99);
        out += &line(label, &st);
        out += "\n";
    }
    eprintln!("{out}");
    let _ = std::fs::write("/tmp/f393_m1_full.txt", &out);
}

// ─────────────────────────────────────────────────────────────────────────
// M4 — DC/MDC-Export-Vertices gegen f64-Grundwahrheit
// ─────────────────────────────────────────────────────────────────────────

/// Signierte GT-Distanz vieler Punkte (Betrag) → p50/p99/max.
fn vertex_distance_stats(gt: &GroundTruth, verts: &[[f64; 3]]) -> Stats {
    use rayon::prelude::*;
    let errs: Vec<f64> = verts.par_iter().map(|&v| gt.distance(v).abs()).collect();
    percentiles(errs, gt.model_size)
}

#[test]
#[ignore = "langlaufend: DC-Mesh 300mm + doppelte Ground Truth"]
fn m4_export_vertices() {
    use fielddraft_kernel::dc::dual_contour;

    let mut out = String::from(
        "\n=== M4 DC-Export-Vertices @300mm — total (vs f64-GT) vs Diskretisierung (vs f32-GT) ===\n",
    );

    let cases: &[(&str, CsgNode, u32)] = &[
        ("Box 300mm", leaf_box(1, [150.0, 150.0, 150.0]), 96),
        ("Box 300mm @pos1000", translate_far(leaf_box(1, [150.0, 150.0, 150.0]), 1000.0), 96),
        ("Bool-Kette T10 300mm", boolean_chain(300.0, 10), 96),
        ("Shell 300mm Wand2", thin_shell(300.0, 2.0), 128),
    ];

    for (label, node, res) in cases {
        let bounded = BoundedCsg::from_csg(node);
        let mesh = dual_contour(&bounded, *res);
        let nv = mesh.vertex_count();
        if nv == 0 {
            out += &format!("{label}: leeres Mesh\n");
            continue;
        }
        // Vertices (f32) → f64.
        let mut verts: Vec<[f64; 3]> = (0..nv)
            .map(|i| {
                [
                    mesh.positions[3 * i] as f64,
                    mesh.positions[3 * i + 1] as f64,
                    mesh.positions[3 * i + 2] as f64,
                ]
            })
            .collect();
        // Deterministisch auf <=2000 Vertices begrenzen (GT-Distanz mit
        // Kompass ist teuer).
        let mut rng = SplitMix64::new(0x4E4);
        if verts.len() > 2000 {
            for i in (1..verts.len()).rev() {
                let j = (rng.next_u64() as usize) % (i + 1);
                verts.swap(i, j);
            }
            verts.truncate(2000);
        }

        let a = node.aabb();
        let lo = [a.min[0] as f64, a.min[1] as f64, a.min[2] as f64];
        let hi = [a.max[0] as f64, a.max[1] as f64, a.max[2] as f64];
        let opts = GtOptions { grid: 96, ..GtOptions::default() };

        // GT auf f64-Spiegel = WAHRE Oberfläche → total error.
        let mfield = MirrorField(node);
        let gt_true = GroundTruth::build(&mfield, lo, hi, &opts);
        // GT auf dem f32-Feld, das der Mesher benutzt (BoundedCsg) →
        // Diskretisierungs-/Newton-Residuum (Vertex sitzt auf DIESER Nullmenge).
        struct BoundedField<'a>(&'a BoundedCsg);
        impl Field for BoundedField<'_> {
            fn eval(&self, p: [f64; 3]) -> f64 {
                self.0.sdf([p[0] as f32, p[1] as f32, p[2] as f32], EvalMode::Visual) as f64
            }
        }
        let bfield = BoundedField(&bounded);
        let gt_f32 = GroundTruth::build(&bfield, lo, hi, &opts);

        let total = vertex_distance_stats(&gt_true, &verts);
        let discz = vertex_distance_stats(&gt_f32, &verts);
        out += &format!("{label}: {} Vertices (res={res}), Probe n={}\n", nv, total.n);
        out += &format!(
            "   total (Vertex→wahre f64-Fläche)  p50={:.3e} p99={:.3e} max={:.3e} mm\n",
            total.p50, total.p99, total.max
        );
        out += &format!(
            "   diskret (Vertex→f32-Nullmenge)   p50={:.3e} p99={:.3e} max={:.3e} mm\n",
            discz.p50, discz.p99, discz.max
        );
    }
    eprintln!("{out}");
    let _ = std::fs::write("/tmp/f393_m4.txt", &out);
}
