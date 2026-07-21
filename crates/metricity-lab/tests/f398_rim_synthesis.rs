//! #398 Phase 1 — Validierung der ∂P-Rand-Fillet-SYNTHESE
//! (`fielddraft_kernel::offset_rim`).
//!
//! Der Prototyp `f396_rim_prototype.rs` hat gezeigt, DASS der additiv-
//! beschränkte, halbraum-maskierte Rand-Fillet den ∂P-Rest der Kontakt-Prisma-
//! Korrektur tilgt — mit HAND-gebauten `RimFeature`s. Dieser Test prüft, dass
//! die neue KERNEL-Synthese (`offset_rim::synthesize_rim_features_for_tree`)
//! aus dem echten Kontakt-Prisma-Verschnitt exakt diese Features ableitet:
//!
//! * Szenen über den ECHTEN Regen-Pfad-Mirror (`expand_shells →
//!   push_down_offsets → fold_offset_leaves → synthesize_offset_channels_reported
//!   → prune_dead_channels`, wie `offset_fuzz`).
//! * Rim-Features aus `offset_rim` (NICHT hand-gebaut).
//! * Anwendung mit der Referenz-`value()`/`apply_rims`-Semantik desselben
//!   Moduls (der Vertrag, an dem sich die Phase-2-Spiegel messen).
//! * Doppel-GT-Messung wie `offset_fuzz`/`f396`: ∂P-naher Restfehler ≤ 0.05·δ,
//!   Over-Fill exakt 0.0000 auf dem Vorzeichen-Grid.
//!
//! Lauf: `cargo test -p metricity-lab --test f398_rim_synthesis -- --nocapture`

use rayon::prelude::*;

use fielddraft_kernel::bvh::BoundedCsg;
use fielddraft_kernel::csg::{CsgNode, EvalMode};
use fielddraft_kernel::offset_capability::CapabilityReport;
use fielddraft_kernel::offset_channel::{
    prune_dead_channels, synthesize_offset_channels_reported, CHANNEL_TOL_RENDER,
};
use fielddraft_kernel::offset_rim::{
    apply_rims, synthesize_rim_features_for_tree, RimFeature, RimRecord,
};
use fielddraft_kernel::sdf_compiler::{encode_rim_block, eval_opcodes, OP_PRIM_CONST};
use fielddraft_protocol::{FeatureId, Primitive, Transform};

use metricity_lab::field::{CsgField, Field, FnField};
use metricity_lab::ground_truth::{GroundTruth, GtOptions};
use metricity_lab::util::SplitMix64;

// ─── f64-Helfer ─────────────────────────────────────────────────────────────

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 { a[0] * b[0] + a[1] * b[1] + a[2] * b[2] }
fn norm(a: [f64; 3]) -> f64 { dot(a, a).sqrt() }

fn quat_axis_angle(axis: [f64; 3], ang: f64) -> [f64; 4] {
    let n = norm(axis).max(1e-30);
    let a = [axis[0] / n, axis[1] / n, axis[2] / n];
    let s = (ang * 0.5).sin();
    [a[0] * s, a[1] * s, a[2] * s, (ang * 0.5).cos()]
}
fn f32q(q: [f64; 4]) -> [f32; 4] { [q[0] as f32, q[1] as f32, q[2] as f32, q[3] as f32] }

fn box_sdf(p: [f64; 3], c: [f64; 3], h: [f64; 3]) -> f64 {
    let d = [(p[0] - c[0]).abs() - h[0], (p[1] - c[1]).abs() - h[1], (p[2] - c[2]).abs() - h[2]];
    let outside = norm([d[0].max(0.0), d[1].max(0.0), d[2].max(0.0)]);
    let inside = d[0].max(d[1]).max(d[2]).min(0.0);
    outside + inside
}

fn id(n: u32) -> FeatureId { FeatureId::from_u32(n) }

// ─── Kernel-Prisma-Feld + Rim-Synthese ──────────────────────────────────────

/// Der gefaltete Regen-Pfad-Baum (vor Kanal-Synthese) — Quelle der Rim-Features.
fn folded_root(node: &CsgNode) -> BoundedCsg {
    let s3 = node.clone().expand_shells().push_down_offsets().fold_offset_leaves();
    BoundedCsg::from_csg(&s3)
}

/// Kompiliertes Kontakt-Prisma-Feld (Regen-Pfad-Mirror, wie `offset_fuzz`).
fn compile_prism_field(node: &CsgNode) -> (BoundedCsg, CapabilityReport) {
    let s3 = node.clone().expand_shells().push_down_offsets().fold_offset_leaves();
    let mut report = CapabilityReport::new();
    s3.collect_offset_capability(&mut report);
    let mut field =
        synthesize_offset_channels_reported(BoundedCsg::from_csg(&s3), CHANNEL_TOL_RENDER, &mut report);
    prune_dead_channels(&mut field, CHANNEL_TOL_RENDER);
    (field, report)
}

// ─── Feld-Wrapper ───────────────────────────────────────────────────────────

struct BoundedField<'a>(&'a BoundedCsg);
impl Field for BoundedField<'_> {
    fn eval(&self, p: [f64; 3]) -> f64 {
        self.0.sdf([p[0] as f32, p[1] as f32, p[2] as f32], EvalMode::Visual) as f64
    }
}
struct RimField<'a> {
    base: &'a BoundedCsg,
    rims: Vec<RimFeature>,
}
impl Field for RimField<'_> {
    fn eval(&self, p: [f64; 3]) -> f64 {
        let g = self.base.sdf([p[0] as f32, p[1] as f32, p[2] as f32], EvalMode::Visual) as f64;
        apply_rims(g, p, &self.rims)
    }
}

// ─── Szenen ─────────────────────────────────────────────────────────────────

struct Scene {
    name: &'static str,
    node: CsgNode,
    base: CsgNode,
    delta: f32, // signiert
}

fn scene_box_stack(delta_abs: f64, q: Option<[f64; 4]>) -> Scene {
    let he_a = [10.0, 10.0, 10.0];
    let he_b = [8.0, 7.0, 6.0];
    let bx = he_a[0] + he_b[0];
    let a_leaf = CsgNode::Leaf { id: id(1), primitive: Primitive::Box { center: [0.0; 3], half_extents: [he_a[0] as f32, he_a[1] as f32, he_a[2] as f32] } };
    let b_leaf = CsgNode::Transform(
        Transform { translate: [bx as f32, 0.0, 0.0], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0; 3] },
        Box::new(CsgNode::Leaf { id: id(2), primitive: Primitive::Box { center: [0.0; 3], half_extents: [he_b[0] as f32, he_b[1] as f32, he_b[2] as f32] } }),
    );
    let mut base = CsgNode::Union(Box::new(a_leaf), Box::new(b_leaf));
    if let Some(qf) = q {
        base = CsgNode::Transform(Transform { translate: [0.0; 3], rotation: f32q(qf), scale: [1.0; 3] }, Box::new(base));
    }
    let node = CsgNode::Offset(Box::new(base.clone()), -(delta_abs as f32));
    Scene { name: if q.is_some() { "box_stack_rot" } else { "box_stack" }, node, base, delta: -(delta_abs as f32) }
}

fn scene_flush_pocket(delta_abs: f64) -> Scene {
    let ha = [16.0f64, 16.0, 16.0];
    let (pdx, py, pz) = (10.0f64, 7.0, 6.0);
    let a = CsgNode::Leaf { id: id(1), primitive: Primitive::Box { center: [0.0; 3], half_extents: [ha[0] as f32, ha[1] as f32, ha[2] as f32] } };
    let bcx = ha[0] - pdx;
    let b = CsgNode::Transform(
        Transform { translate: [bcx as f32, 0.0, 0.0], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0; 3] },
        Box::new(CsgNode::Leaf { id: id(2), primitive: Primitive::Box { center: [0.0; 3], half_extents: [pdx as f32, py as f32, pz as f32] } }),
    );
    let base = CsgNode::Difference(Box::new(a), Box::new(b));
    let node = CsgNode::Offset(Box::new(base.clone()), delta_abs as f32);
    Scene { name: "flush_pocket", node, base, delta: delta_abs as f32 }
}

// ─── Messung (Doppel-GT wie offset_fuzz / f396) ─────────────────────────────

struct Zs { p99: f64, max: f64, mean: f64, n: usize }

fn near_rim_dist(p: [f64; 3], rims: &[RimFeature]) -> f64 {
    rims.iter().map(|r| r.closest(p).0).fold(f64::INFINITY, f64::min)
}

fn measure(
    field: &dyn Field,
    base_gt: &GroundTruth,
    lo: [f64; 3], hi: [f64; 3],
    delta_signed: f64,
    rims: &[RimFeature],
    grid: usize, n_probe: usize, seed: u64,
    near_rim_only: bool,
) -> Option<Zs> {
    let opts = GtOptions { grid, ..GtOptions::default() };
    let gt = GroundTruth::build(field, lo, hi, &opts);
    let surf = gt.surface_points();
    if surf.is_empty() { return None; }
    let da = delta_signed.abs();
    let pool: Vec<[f64; 3]> = if near_rim_only {
        surf.iter().copied().filter(|&p| near_rim_dist(p, rims) < 1.5 * da).collect()
    } else {
        surf.to_vec()
    };
    if pool.is_empty() { return None; }
    let mut rng = SplitMix64::new(seed ^ 0x9E37_79B9);
    let probe: Vec<[f64; 3]> = (0..n_probe.min(pool.len()))
        .map(|_| pool[(rng.next_u64() as usize) % pool.len()])
        .collect();
    let mut errs: Vec<f64> = probe
        .par_iter()
        .map(|&p| (base_gt.distance(p) - delta_signed).abs())
        .filter(|e| e.is_finite())
        .collect();
    if errs.is_empty() { return None; }
    let n = errs.len();
    let mean = errs.iter().sum::<f64>() / n as f64;
    errs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let max = *errs.last().unwrap();
    let p99 = errs[((n as f64 * 0.99) as usize).min(n - 1)];
    Some(Zs { p99, max, mean, n })
}

fn dual_safety_overfill(
    corrected: &dyn Field,
    base_gt: &GroundTruth,
    lo: [f64; 3], hi: [f64; 3],
    delta_signed: f64,
    grid: usize,
) -> f64 {
    let concave = delta_signed < 0.0;
    let step = [(hi[0] - lo[0]) / grid as f64, (hi[1] - lo[1]) / grid as f64, (hi[2] - lo[2]) / grid as f64];
    let pts: Vec<[f64; 3]> = (0..grid).flat_map(|i| (0..grid).flat_map(move |j| (0..grid).map(move |k| (i, j, k)))).map(|(i, j, k)| {
        [lo[0] + (i as f64 + 0.5) * step[0], lo[1] + (j as f64 + 0.5) * step[1], lo[2] + (k as f64 + 0.5) * step[2]]
    }).collect();
    pts.par_iter().map(|&p| {
        let g = corrected.eval(p);
        let d_true = base_gt.distance(p) - delta_signed;
        if concave {
            if g < 0.0 && d_true > 0.0 { d_true } else { 0.0 }
        } else if g > 0.0 && d_true < 0.0 { -d_true } else { 0.0 }
    }).reduce(|| 0.0f64, f64::max)
}

fn fmt(z: &Option<Zs>) -> String {
    match z {
        Some(z) => format!("p99={:.4} max={:.4} mean={:.4} n={}", z.p99, z.max, z.mean, z.n),
        None => "—".to_string(),
    }
}

fn run_scene(s: &Scene, grid: usize, probe: usize) {
    let (field, report) = compile_prism_field(&s.node);
    let root = folded_root(&s.node);
    let rims = synthesize_rim_features_for_tree(&root);
    let da = s.delta.abs() as f64;

    // Kanten- vs. Vertex-Features aufschlüsseln.
    let n_edges = rims.iter().filter(|r| r.a != r.b).count();
    let n_verts = rims.len() - n_edges;

    let base_field = CsgField::visual(&s.base);
    let baabb = s.base.aabb();
    let blo = [baabb.min[0] as f64, baabb.min[1] as f64, baabb.min[2] as f64];
    let bhi = [baabb.max[0] as f64, baabb.max[1] as f64, baabb.max[2] as f64];
    let base_gt = GroundTruth::build(&base_field, blo, bhi, &GtOptions { grid, ..GtOptions::default() });

    let fa = field.aabb();
    let flo = [fa.min[0] as f64 - 1.0, fa.min[1] as f64 - 1.0, fa.min[2] as f64 - 1.0];
    let fhi = [fa.max[0] as f64 + 1.0, fa.max[1] as f64 + 1.0, fa.max[2] as f64 + 1.0];

    let prism = BoundedField(&field);
    let full = RimField { base: &field, rims: rims.clone() };

    let z_prism_rim = measure(&prism, &base_gt, flo, fhi, s.delta as f64, &rims, grid, probe, 2, true);
    let z_full_rim = measure(&full, &base_gt, flo, fhi, s.delta as f64, &rims, grid, probe, 3, true);
    let overfill = dual_safety_overfill(&full, &base_gt, flo, fhi, s.delta as f64, 40);

    eprintln!("\n[{:>15} δ={:+.1}] {:?}  rims={} (Kanten={n_edges}, Vertizes={n_verts})",
        s.name, s.delta, report.status(), rims.len());
    eprintln!("  ∂P-NAH prism: {}", fmt(&z_prism_rim));
    eprintln!("         full:  {}", fmt(&z_full_rim));
    eprintln!("  ∂P-nah prism/δ: {:.3}   full/δ: {:.3}   overfill={:.4} mm",
        z_prism_rim.as_ref().map(|z| z.p99 / da).unwrap_or(0.0),
        z_full_rim.as_ref().map(|z| z.p99 / da).unwrap_or(0.0),
        overfill);

    // Synthese muss die Rand-Features tatsächlich erzeugt haben.
    assert!(n_edges >= 4, "{} δ={}: erwarte ≥4 Stufen-Kanten, got {n_edges}", s.name, s.delta);
    // ∂P-Rand-Fehler nach der Synthese ≤ 0.05·δ (Prototyp: ≤ 0.02·δ).
    if let Some(f) = &z_full_rim {
        assert!(f.p99 <= 0.05 * da, "{} δ={}: ∂P-Rest {:.4} > 0.05·δ={:.4}", s.name, s.delta, f.p99, 0.05 * da);
    } else {
        panic!("{} δ={}: keine ∂P-nahen Messpunkte", s.name, s.delta);
    }
    // Over-Fill exakt 0 (bis auf GT-/f32-Rauschen).
    assert!(overfill < 0.05 * da + 0.02, "{} δ={}: Over-Fill {:.4} zu groß", s.name, s.delta, overfill);
}

// ─── Phase 2: CPU-Opcode-Parität gegen die f64-Referenz ─────────────────────

/// Wertet den `OP_RIM_FILLET`-Opcode auf `g` aus (Tape `[CONST g] ++ Block`).
/// KEIN Regen-Pfad — nur der Test konstruiert das Tape.
fn eval_rim_opcode(g: f32, p: [f32; 3], concave: bool, recs: &[RimRecord]) -> f32 {
    let mut tape = vec![f32::from_bits(OP_PRIM_CONST), g];
    encode_rim_block(concave, recs, &mut tape);
    eval_opcodes(&tape, p)
}

/// Parität des f32-Opcodes gegen den f64-Vertrag `apply_rims` auf den
/// SYNTHETISIERTEN Features der Phase-1-Szenen. Zwei Invarianten je Szene:
/// (1) `max |Opcode − Referenz|` im ∂P-Band unter f32-Toleranz;
/// (2) KEINE Über-Fill: der Opcode-Wert unterschreitet (konkav) bzw.
///     überschreitet (konvex) die Referenz nie über die f32-Rundung hinaus —
///     besonders an den Masken-/Cap-Rändern (gezielt ±1e-5 um die Halbraum-
///     Grenzen an Π geprobt).
fn parity_scene(s: &Scene) {
    let (field, _report) = compile_prism_field(&s.node);
    let root = folded_root(&s.node);
    let rims = synthesize_rim_features_for_tree(&root);
    assert!(!rims.is_empty(), "{}: Synthese lieferte keine Features", s.name);
    let recs: Vec<RimRecord> = rims.iter().map(RimRecord::from_feature).collect();
    let concave = s.delta < 0.0;
    let da = s.delta.abs() as f64;

    let fa = field.aabb();
    let lo = [fa.min[0] as f64 - 1.0, fa.min[1] as f64 - 1.0, fa.min[2] as f64 - 1.0];
    let hi = [fa.max[0] as f64 + 1.0, fa.max[1] as f64 + 1.0, fa.max[2] as f64 + 1.0];

    let mut rng = SplitMix64::new(0xF398 ^ (da.to_bits()));
    let mut samples: Vec<[f64; 3]> = Vec::new();
    // (a) ∂P-Band-Samples: nahe der Reflexkurve, im Korrektur-Band.
    let mut tries = 0;
    while samples.len() < 6000 && tries < 400_000 {
        tries += 1;
        let p = [
            lo[0] + (hi[0] - lo[0]) * (rng.next_u64() as f64 / u64::MAX as f64),
            lo[1] + (hi[1] - lo[1]) * (rng.next_u64() as f64 / u64::MAX as f64),
            lo[2] + (hi[2] - lo[2]) * (rng.next_u64() as f64 / u64::MAX as f64),
        ];
        if near_rim_dist(p, &rims) < 1.4 * da {
            samples.push(p);
        }
    }
    // (b) Masken-Rand-Proben, in ECHTER Bandgeometrie verankert: für jeden
    //     bereits maskiert-aktiven Band-Punkt zusätzlich ±1e-5 entlang der
    //     Wand-Normalen der aktiven Kante — das kreuzt die Halbraum-Grenze
    //     ⟨p−q, n̂⟩ = 0 genau dort, wo δ−de ≈ g (Design §4.3). Die f32-Rundung
    //     darf die Maske verschieben, aber nie überfüllen.
    let band0 = samples.clone();
    for &p in band0.iter().take(1500) {
        for r in &rims {
            if r.value(p).is_none() {
                continue;
            }
            for &n in &[r.n_a, r.n_b, r.n_c] {
                if norm(n) < 0.5 {
                    continue;
                }
                for &e in &[-1e-5, 1e-5] {
                    samples.push([p[0] + e * n[0], p[1] + e * n[1], p[2] + e * n[2]]);
                }
            }
        }
    }

    let mut max_diff = 0.0f64;
    let mut max_overfill = 0.0f64;
    for &p in &samples {
        let pf = [p[0] as f32, p[1] as f32, p[2] as f32];
        let g = field.sdf(pf, EvalMode::Visual);
        let want = apply_rims(g as f64, p, &rims);
        let got = eval_rim_opcode(g, pf, concave, &recs) as f64;
        max_diff = max_diff.max((got - want).abs());
        // Über-Fill = Feld-seitiges Unterschreiten (konkav) / Überschreiten (konvex).
        let of = if concave { want - got } else { got - want };
        max_overfill = max_overfill.max(of);
    }
    eprintln!(
        "[{:>15} δ={:+.1}] rims={} parity max|Δ|={:.2e}  max-overfill={:.2e}",
        s.name, s.delta, rims.len(), max_diff, max_overfill
    );
    // f32-Toleranz bei Szenengröße ≤ ~34 mm: ~1e-4; Masken-Flips an Π sind
    // wertstetig (δ−de = g) → kein Sprung. Großzügige, aber harte Schranke.
    assert!(max_diff < 5e-4, "{}: Opcode↔f64 max|Δ| {:.3e} zu groß", s.name, max_diff);
    // Über-Fill nur bis zur f32-Rundung (nie strukturell).
    assert!(max_overfill < 5e-4, "{}: Opcode überfüllt um {:.3e}", s.name, max_overfill);
}

#[test]
fn f398_rim_opcode_parity() {
    eprintln!("=== #398 Phase 2 — Opcode↔f64-Parität ===");
    for &d in &[0.5f64, 1.0, 2.0] {
        parity_scene(&scene_box_stack(d, None));
    }
    parity_scene(&scene_box_stack(1.0, Some(quat_axis_angle([0.3, 1.0, 0.5], 0.7))));
    parity_scene(&scene_box_stack(2.0, Some(quat_axis_angle([0.3, 1.0, 0.5], 0.7))));
    for &d in &[1.0f64, 2.0, 4.0] {
        parity_scene(&scene_flush_pocket(d));
    }
}

#[test]
fn f398_rim_synthesis_report() {
    let grid = 64;
    let probe = 400;
    eprintln!("=== #398 Rand-Fillet-SYNTHESE (grid={grid}, probe={probe}) ===");

    for &d in &[0.5f64, 1.0, 2.0] {
        run_scene(&scene_box_stack(d, None), grid, probe);
    }
    for &d in &[1.0f64, 2.0] {
        run_scene(&scene_box_stack(d, Some(quat_axis_angle([0.3, 1.0, 0.5], 0.7))), grid, probe);
    }
    for &d in &[1.0f64, 2.0, 4.0] {
        run_scene(&scene_flush_pocket(d), grid, probe);
    }

    // ─ Trihedrale √3-Reflexecke (reine f64): validiert die Vertex-`value()`-
    //   Semantik von `offset_rim::RimFeature` end-to-end (der Kernel-Prisma-
    //   Pfad erzeugt hier keinen Trieder — drei separate Boxen). ─
    eprintln!("\n=== Trihedrale Reflexecke (√3), reine f64, offset_rim-Vertex ===");
    for &d in &[1.0f64, 2.0] {
        let (np99, nmax, fp99, over) = measure_trihedral_vertex(d, grid, probe);
        eprintln!("[ trihedral δ={:+.1}] naiv p99={:.4} max={:.4} ({:.3}·δ) → full p99={:.4} ({:.3}·δ)  overfill={:.4}",
            -d, np99, nmax, nmax / d, fp99, fp99 / d, over);
        assert!(fp99 < 0.2 * d, "trihedral δ={d}: Vertex-Fillet erreicht Ziel nicht (p99 {fp99:.4})");
        assert!(over < 0.15 * d + 0.05, "trihedral δ={d}: Vertex-Fillet überfüllt ({over:.4})");
    }
}

/// Reine-f64-Isolation der √3-Reflexecke (drei orthogonale Halbraum-Boxen),
/// mit direkt konstruierten `offset_rim::RimFeature`s (drei Kanten + Vertex).
fn measure_trihedral_vertex(delta_abs: f64, grid: usize, probe: usize) -> (f64, f64, f64, f64) {
    let l = 40.0f64;
    let boxes = [
        ([-l, 0.0, 0.0], [l, l, l]),
        ([0.0, -l, 0.0], [l, l, l]),
        ([0.0, 0.0, -l], [l, l, l]),
    ];
    let naive_base = move |p: [f64; 3]| boxes.iter().map(|&(c, h)| box_sdf(p, c, h)).fold(f64::INFINITY, f64::min);
    let base = FnField(naive_base);
    let trust = 3.0 * delta_abs;
    let e = 40.0f64;
    let mk = |a: [f64; 3], b: [f64; 3], na: [f64; 3], nb: [f64; 3], nc: [f64; 3]| RimFeature {
        a, b, n_a: na, n_b: nb, n_c: nc, delta: delta_abs, trust, concave: true,
    };
    let rims = vec![
        mk([0.0, 0.0, 0.0], [0.0, 0.0, e], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0; 3]),
        mk([0.0, 0.0, 0.0], [e, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.0; 3]),
        mk([0.0, 0.0, 0.0], [0.0, e, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0; 3]),
        mk([0.0, 0.0, 0.0], [0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
    ];
    let lo = [-2.5 * delta_abs - 1.0, -2.5 * delta_abs - 1.0, -2.5 * delta_abs - 1.0];
    let hi = [2.5 * delta_abs + 1.0, 2.5 * delta_abs + 1.0, 2.5 * delta_abs + 1.0];
    let base_gt = GroundTruth::build(&base, [-l, -l, -l], [l, l, l], &GtOptions { grid, ..GtOptions::default() });

    let d_signed = -delta_abs;
    let naive_f = FnField(move |p| naive_base(p) - d_signed);
    let full_f = FnField(move |p| apply_rims(naive_base(p) - d_signed, p, &rims));

    let z_naive = measure(&naive_f, &base_gt, lo, hi, d_signed, &[], grid, probe, 11, false);
    let z_full = measure(&full_f, &base_gt, lo, hi, d_signed, &[], grid, probe, 12, false);
    let over = dual_safety_overfill(&full_f, &base_gt, lo, hi, d_signed, 44);
    (
        z_naive.as_ref().map(|z| z.p99).unwrap_or(0.0),
        z_naive.as_ref().map(|z| z.max).unwrap_or(0.0),
        z_full.as_ref().map(|z| z.p99).unwrap_or(0.0),
        over,
    )
}
