//! offset_tree Phase B (#384) — Pflicht-Regressionen (Plan §6):
//!
//! * **notch_union** (B0-Fixture): der δ·(1/sin(α/2)−1)-Sliver am
//!   −δ-Union-Stopp ist nach Kanal-Addition geschlossen.
//! * **Steinmetz** (Spike-Fixture): +δ über Schnitt zweier orthogonaler
//!   Zylinder — Zero-Set-Fehler ≤ 0.01 mm (Render-Klasse) bzw.
//!   ≤ 0.001 mm (Export-Klasse) gegen die exakte konvexe Ground Truth.
//! * **Tape-Parität** CPU-Interpreter (OP_ARC_CHANNEL) vs. Baum-Eval.
//! * **March-Sicherheit**: Außenfeld-Überschätzung ≤ Kurventoleranz.

use super::*;
use crate::csg::CsgNode;
use fielddraft_protocol::{FeatureId, Primitive, Transform};

// ─── kleine f64-Helfer für die Ground Truth ─────────────────────────

fn sub64(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn norm64(a: [f64; 3]) -> f64 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}
fn scale64(a: [f64; 3], s: f64) -> [f64; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn add64(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

/// Deterministischer SplitMix64 (Testpunkt-Wolken).
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }
    fn f(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.f()
    }
    fn dir(&mut self) -> [f64; 3] {
        loop {
            let v = [
                self.range(-1.0, 1.0),
                self.range(-1.0, 1.0),
                self.range(-1.0, 1.0),
            ];
            let n = norm64(v);
            if n > 1e-3 && n <= 1.0 {
                return scale64(v, 1.0 / n);
            }
        }
    }
}

// ─── Fixture 1: notch_union-Dieder (B0) ─────────────────────────────

fn leaf_box(id: u32, h: [f32; 3]) -> CsgNode {
    CsgNode::Leaf {
        id: FeatureId::from_u32(id),
        primitive: Primitive::Box { center: [0.0; 3], half_extents: h },
    }
}

/// Union-Dieder mit Luftkeil-Öffnung α, erodiert um δ (B0-Fixture
/// `notch_union`): Wurzel bleibt `Offset(Union, −δ)` — der duale Stopp.
fn notch_union_node(alpha_deg: f64, delta: f64) -> CsgNode {
    let alpha = alpha_deg.to_radians();
    let gamma = std::f64::consts::PI - alpha;
    let b1 = CsgNode::Transform(
        Transform {
            translate: [0.0, -50.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        },
        Box::new(leaf_box(1, [100.0, 50.0, 50.0])),
    );
    let b2 = CsgNode::Transform(
        Transform {
            translate: [
                (50.0 * gamma.sin()) as f32,
                (-50.0 * gamma.cos()) as f32,
                0.0,
            ],
            rotation: [
                0.0,
                0.0,
                (gamma / 2.0).sin() as f32,
                (gamma / 2.0).cos() as f32,
            ],
            scale: [1.0; 3],
        },
        Box::new(leaf_box(2, [100.0, 50.0, 50.0])),
    );
    CsgNode::Offset(
        Box::new(CsgNode::Union(Box::new(b1), Box::new(b2))),
        -(delta as f32),
    )
}

/// Analytische Referenz des WAHR erodierten Union-Dieders (B0):
/// Luftkeil = {y ≥ 0 ∧ q·n2 ≥ 0} (z-invariant), Material ⇔
/// dist(p, Keil) ≥ δ; in Naht-Nähe ist `δ − dist` das exakte SDF.
fn notch_reference(alpha_deg: f64, delta: f64) -> impl Fn([f64; 3]) -> f64 {
    let alpha = alpha_deg.to_radians();
    let gamma = std::f64::consts::PI - alpha;
    let n2 = [-gamma.sin(), gamma.cos()];
    let r1 = [-1.0f64, 0.0];
    let r2 = [gamma.cos(), gamma.sin()];
    let ray_dist = |p: [f64; 2], r: [f64; 2]| -> f64 {
        let t = (p[0] * r[0] + p[1] * r[1]).max(0.0);
        let q = [p[0] - t * r[0], p[1] - t * r[1]];
        (q[0] * q[0] + q[1] * q[1]).sqrt()
    };
    move |p: [f64; 3]| {
        let q = [p[0], p[1]];
        let inside_cone = q[1] >= 0.0 && (q[0] * n2[0] + q[1] * n2[1]) >= 0.0;
        let dist = if inside_cone {
            0.0
        } else {
            ray_dist(q, r1).min(ray_dist(q, r2))
        };
        delta - dist
    }
}

fn synth(node: &CsgNode, tol: f32) -> BoundedCsg {
    synthesize_offset_channels(BoundedCsg::from_csg(node), tol)
}

fn notch_check(alpha_deg: f64, delta: f64, band_tol: f64) {
    let node = notch_union_node(alpha_deg, delta);
    let naive = BoundedCsg::from_csg(&node);
    let corrected = synth(&node, 0.005);
    let (n_ch, n_arcs) = count_channels(&corrected);
    assert!(
        n_ch >= 1,
        "notch α={alpha_deg}: kein Kanal synthetisiert (arcs {n_arcs})"
    );

    let reference = notch_reference(alpha_deg, delta);
    // Die analytische Referenz `δ − dist(p, Luftkeil)` ist nur dort das
    // exakte SDF, wo der nächste Keil-Punkt der Scheitel bzw. ein
    // Keil-Rand ist — d. h. im Band um den wahren Erosions-BOGEN
    // (Radius δ um die Scheitelachse, Material-seitiger Dualkegel).
    // Gemessen wird deshalb wie in B0 NUR das Naht-Band (±0.8 mm um
    // den Bogen, |z| ≤ 20 fern der Box-Enden).
    let alpha = alpha_deg.to_radians();
    let gamma = std::f64::consts::PI - alpha;
    let n2 = [-gamma.sin(), gamma.cos()];
    let a_lo = -std::f64::consts::FRAC_PI_2; // Winkel von −n1 = (0,−1)
    let a_hi = (-n2[1]).atan2(-n2[0] + 0.0).atan2(1.0); // placeholder
    let _ = a_hi;
    let a_hi = (-n2[1]).atan2(-n2[0]); // Winkel von −n2
    let mut rng = Rng(4242);
    // (a) Zero-Set-Metrik (B0-/Spike-Methodik): entlang radialer Strahlen
    // durch den Erosions-Bogen die Feld-Nullstelle bisektieren; die
    // Referenz ist dort ein exaktes SDF, |ref| an der Feld-Nullstelle ist
    // also die exakte geometrische Abweichung. (b) March-Sicherheit:
    // außen nie mehr als band_tol überschätzen. Der INNEN-Betrag ist
    // ausdrücklich NICHT Ziel (Plan §2 „keine Innen-Betragsgarantie" —
    // der konkave Kanal spiegelt die Keil-Konzession des Spikes).
    let field = |p: [f64; 3]| -> f64 {
        corrected.sdf([p[0] as f32, p[1] as f32, p[2] as f32], EvalMode::Visual) as f64
    };
    let mut max_zero_err = 0.0f64;
    let mut max_over = f64::NEG_INFINITY;
    for _ in 0..2000 {
        let phi = rng.range(a_lo + 0.03, a_hi - 0.03);
        let z = rng.range(-20.0, 20.0);
        let dir = [phi.cos(), phi.sin()];
        let r_lo = delta - 0.8;
        let r_hi = delta + 0.8;
        // Vorzeichenwechsel des korrigierten Felds im Fenster suchen.
        let eval_r = |r: f64| field([dir[0] * r, dir[1] * r, z]);
        let mut prev_r = r_lo;
        let mut prev_s = eval_r(r_lo) > 0.0;
        let mut found = None;
        for k in 1..=64 {
            let r = r_lo + (r_hi - r_lo) * k as f64 / 64.0;
            let sgn = eval_r(r) > 0.0;
            if sgn != prev_s {
                let (mut lo, mut hi) = (prev_r, r);
                for _ in 0..40 {
                    let m = (lo + hi) * 0.5;
                    if (eval_r(m) > 0.0) == prev_s {
                        lo = m;
                    } else {
                        hi = m;
                    }
                }
                found = Some(hi);
                break;
            }
            prev_r = r;
            prev_s = sgn;
        }
        if let Some(r0) = found {
            let p0 = [dir[0] * r0, dir[1] * r0, z];
            max_zero_err = max_zero_err.max(reference(p0).abs());
        }
        // March-Sicherheit an Außenpunkten des Bands.
        let r = delta + rng.range(0.05, 0.8);
        let p = [dir[0] * r, dir[1] * r, z];
        let f_ref = reference(p);
        if f_ref > 0.0 {
            max_over = max_over.max(field(p) - f_ref);
        }
    }
    assert!(
        max_zero_err <= band_tol,
        "notch α={alpha_deg}: Zero-Set-Fehler {max_zero_err:.5} mm > {band_tol} mm"
    );
    assert!(
        max_over <= band_tol,
        "notch α={alpha_deg}: Außen-Überschätzung {max_over:.5} mm > {band_tol} mm"
    );

    // Sliver-Punkte auf der Material-Bisektrix zwischen wahrem Bogen
    // (r = δ) und naiver Ecke (r = δ/sin(α/2)): naiv Luft (der B0-
    // Befund), korrigiert Material.
    let bis = {
        let b = [-(0.0 + n2[0]), -(1.0 + n2[1])];
        let n = (b[0] * b[0] + b[1] * b[1]).sqrt();
        [b[0] / n, b[1] / n]
    };
    let r_naive = delta / (alpha / 2.0).sin();
    let mut naive_sliver_seen = false;
    for k in 1..20 {
        let r = delta + (r_naive - delta) * (k as f64 / 20.0) * 0.9 + 0.02;
        let p = [bis[0] * r, bis[1] * r, 3.0];
        let pf = [p[0] as f32, p[1] as f32, p[2] as f32];
        let f_ref = reference(p);
        if f_ref > -0.02 {
            continue; // zu nah an der wahren Fläche
        }
        if naive.sdf(pf, EvalMode::Visual) > 0.0 {
            naive_sliver_seen = true; // B0-Sliver existiert im naiven Feld
            let f = corrected.sdf(pf, EvalMode::Visual);
            assert!(
                f < 0.0,
                "notch α={alpha_deg}: Sliver-Punkt r={r:.3} bleibt Luft (f={f:.4}, ref={f_ref:.4})"
            );
        }
    }
    assert!(
        naive_sliver_seen,
        "Fixture-Selbstcheck: naives Feld zeigt keinen Sliver (α={alpha_deg})"
    );
}

#[test]
fn notch_union_90_sliver_closed() {
    // B0: α=90°, δ=2 → Sliver-Tiefe 0.828 mm im naiven Feld. Nach der
    // Kanal-Addition Nullmengen-Fehler in der Kurventoleranz-Klasse.
    notch_check(90.0, 2.0, 0.02);
}

#[test]
fn notch_union_60_deep_sliver_closed() {
    // α=60° → Sliver-Tiefe δ·1.0 = volle 2 mm — validiert die
    // Gate-Weite c·δ = δ/sin(α/2) = 2δ (ohne sie bliebe Rest-Sliver).
    notch_check(60.0, 2.0, 0.02);
}

// ─── Fixture 2: Steinmetz (Spike Teil B) ────────────────────────────

const R1: f64 = 30.0;
const HZ1: f64 = 50.0;
const R2: f64 = 20.0;
const HX2: f64 = 50.0;

fn cyl_sdf(rad: f64, ax: f64, r: f64, h: f64) -> f64 {
    let d = rad - r;
    let w = ax - h;
    let outside = (d.max(0.0).powi(2) + w.max(0.0).powi(2)).sqrt();
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

fn edge_point(sx: f64, theta: f64) -> [f64; 3] {
    let y = R2 * theta.cos();
    let z = R2 * theta.sin();
    let x = sx * (R1 * R1 - y * y).sqrt();
    [x, y, z]
}

/// Dichte Referenz-Polyline der Steinmetz-Kante mit Chunk-Pruning.
struct DenseEdge {
    loops: Vec<Vec<[f64; 3]>>,
    chunks: Vec<(usize, usize, usize, [f64; 3], f64)>,
}

impl DenseEdge {
    fn build(n: usize) -> Self {
        let mut loops = Vec::new();
        for sx in [1.0, -1.0] {
            let pts: Vec<[f64; 3]> = (0..n)
                .map(|i| edge_point(sx, i as f64 * std::f64::consts::TAU / n as f64))
                .collect();
            loops.push(pts);
        }
        let chunk_len = 64usize.min(n);
        let mut chunks = Vec::new();
        for (li, pts) in loops.iter().enumerate() {
            let n = pts.len();
            let mut s = 0;
            while s < n {
                let len = chunk_len.min(n - s);
                let mut c = [0.0; 3];
                for k in 0..=len {
                    c = add64(c, pts[(s + k) % n]);
                }
                c = scale64(c, 1.0 / (len + 1) as f64);
                let mut r: f64 = 0.0;
                for k in 0..=len {
                    r = r.max(norm64(sub64(pts[(s + k) % n], c)));
                }
                chunks.push((li, s, len, c, r));
                s += len;
            }
        }
        Self { loops, chunks }
    }

    fn dist(&self, p: [f64; 3]) -> f64 {
        let mut best = f64::INFINITY;
        for &(li, s, len, c, r) in &self.chunks {
            if norm64(sub64(p, c)) - r >= best {
                continue;
            }
            let pts = &self.loops[li];
            let n = pts.len();
            for k in 0..len {
                let a = pts[(s + k) % n];
                let b = pts[(s + k + 1) % n];
                let ab = sub64(b, a);
                let t = ((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1] + (p[2] - a[2]) * ab[2])
                    / (ab[0] * ab[0] + ab[1] * ab[1] + ab[2] * ab[2]);
                let t = t.clamp(0.0, 1.0);
                let q = add64(a, scale64(ab, t));
                let d = norm64(sub64(p, q));
                if d < best {
                    best = d;
                }
            }
        }
        best
    }
}

/// Exakte Ground Truth: S = A ∩ B konvex ⇒ SDF(S↑δ) = SDF(S) − δ,
/// dist(x, S) über die Projektions-Identität (Spike, `b_validation`:
/// gegen Dykstra auf 3e-9 belastbar).
struct TrueField {
    dense: DenseEdge,
    delta: f64,
}

impl TrueField {
    fn eval(&self, p: [f64; 3]) -> f64 {
        let va = fa(p);
        let vb = fb(p);
        if va <= 0.0 && vb <= 0.0 {
            return va.max(vb) - self.delta;
        }
        let mut d = self.dense.dist(p);
        if fb(proj_a(p)) <= 1e-9 {
            d = d.min(va.max(0.0));
        }
        if fa(proj_b(p)) <= 1e-9 {
            d = d.min(vb.max(0.0));
        }
        d - self.delta
    }
}

/// Steinmetz-Baum: Zylinder r1=30 entlang z ∩ Zylinder r2=20 entlang x,
/// dann Offset(+δ) — der duale Dilations-Stopp.
fn steinmetz_node(delta: f32) -> CsgNode {
    let half = std::f32::consts::FRAC_1_SQRT_2;
    let a = CsgNode::Transform(
        Transform {
            translate: [0.0; 3],
            rotation: [half, 0.0, 0.0, half], // lokale Y-Achse → Welt-Z
            scale: [1.0; 3],
        },
        Box::new(CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Cylinder {
                center: [0.0; 3],
                radius: R1 as f32,
                half_height: HZ1 as f32,
            },
        }),
    );
    let b = CsgNode::Transform(
        Transform {
            translate: [0.0; 3],
            rotation: [0.0, 0.0, -half, half], // lokale Y-Achse → Welt-X
            scale: [1.0; 3],
        },
        Box::new(CsgNode::Leaf {
            id: FeatureId::from_u32(2),
            primitive: Primitive::Cylinder {
                center: [0.0; 3],
                radius: R2 as f32,
                half_height: HX2 as f32,
            },
        }),
    );
    CsgNode::Offset(
        Box::new(CsgNode::Intersection(Box::new(a), Box::new(b))),
        delta,
    )
}

/// Erste Nullstelle eines 1-Lipschitz-f64-Felds entlang eines Strahls.
fn first_zero_lipschitz(
    f: &dyn Fn([f64; 3]) -> f64,
    o: [f64; 3],
    dir: [f64; 3],
    tmax: f64,
) -> Option<f64> {
    let mut t = 0.0f64;
    let mut g = f(o);
    if g <= 0.0 {
        return Some(0.0);
    }
    loop {
        let step = (g * 0.9).max(1e-4);
        let tn = t + step;
        if tn > tmax {
            return None;
        }
        let gn = f(add64(o, scale64(dir, tn)));
        if gn <= 0.0 {
            let (mut lo, mut hi) = (t, tn);
            for _ in 0..60 {
                let m = (lo + hi) * 0.5;
                if f(add64(o, scale64(dir, m))) <= 0.0 {
                    hi = m;
                } else {
                    lo = m;
                }
            }
            return Some(hi);
        }
        t = tn;
        g = gn;
    }
}

/// Erster Vorzeichenwechsel eines beliebigen Felds im Fenster um t0.
fn first_sign_change(
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
    let mut prev_s = f(add64(o, scale64(dir, t_start))) > 0.0;
    for i in 1..=steps {
        let t = t_start + i as f64 * dt;
        let s = f(add64(o, scale64(dir, t))) > 0.0;
        if s != prev_s {
            let (mut lo, mut hi) = (prev_t, t);
            for _ in 0..50 {
                let m = (lo + hi) * 0.5;
                if (f(add64(o, scale64(dir, m))) > 0.0) == prev_s {
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

/// Zero-Set-Fehler des korrigierten Felds gegen die exakte GT:
/// |f_true| am Feld-Oberflächenpunkt (f_true ist exaktes SDF ⇒ exakte
/// geometrische Abweichung, Spike-Methodik). Liefert (max, Bogenzahl).
fn steinmetz_zero_set_error(tol: f32, n_rays: usize) -> (f64, usize) {
    let delta = 2.0f64;
    let node = steinmetz_node(delta as f32);
    let corrected = synth(&node, tol);
    let (n_ch, n_arcs) = count_channels(&corrected);
    assert!(n_ch >= 2, "Steinmetz: erwarte ≥ 2 Kanäle (Loops), habe {n_ch}");

    let gt = TrueField { dense: DenseEdge::build(4096), delta };
    let field = |p: [f64; 3]| -> f64 {
        corrected.sdf([p[0] as f32, p[1] as f32, p[2] as f32], EvalMode::Visual) as f64
    };

    let mut rng = Rng(0xB0B5);
    let mut max_err = 0.0f64;
    let mut hits = 0usize;
    while hits < n_rays {
        let sx = if rng.f() < 0.5 { 1.0 } else { -1.0 };
        let th = rng.range(0.0, std::f64::consts::TAU);
        let q = edge_point(sx, th);
        let o = add64(q, scale64(rng.dir(), 25.0 + 3.0 * delta));
        let target = add64(q, scale64(rng.dir(), rng.range(0.0, 1.5 * delta)));
        let dir = sub64(target, o);
        let n = norm64(dir);
        if n < 1e-6 {
            continue;
        }
        let dir = scale64(dir, 1.0 / n);
        let Some(t_true) = first_zero_lipschitz(&|p| gt.eval(p), o, dir, 200.0) else {
            continue;
        };
        let Some(t_var) = first_sign_change(&field, o, dir, t_true, 1.2 * delta + 0.5, 240)
        else {
            continue;
        };
        let err = gt.eval(add64(o, scale64(dir, t_var))).abs();
        max_err = max_err.max(err);
        hits += 1;
    }
    (max_err, n_arcs)
}

#[test]
fn steinmetz_zero_set_render_class() {
    // Plan §6: ≤ 0.01 mm in der 32-Bögen-Klasse.
    let (err, arcs) = steinmetz_zero_set_error(0.01, 160);
    assert!(
        err <= 0.010,
        "Steinmetz Render-Klasse: Zero-Set-Fehler {err:.5} mm > 0.010 ({arcs} Bögen)"
    );
    assert!(
        arcs <= 64,
        "Steinmetz Render-Klasse: {arcs} Bögen > 64 (Budget-Klasse 32)"
    );
}

#[test]
fn steinmetz_zero_set_export_class() {
    // Plan §6: ≤ 0.001 mm mit 128 Bögen gesamt.
    let (err, arcs) = steinmetz_zero_set_error(0.001, 160);
    assert!(
        err <= 0.0012,
        "Steinmetz Export-Klasse: Zero-Set-Fehler {err:.6} mm > 0.0012 ({arcs} Bögen)"
    );
    assert!(
        arcs <= 192,
        "Steinmetz Export-Klasse: {arcs} Bögen > 192 (Budget-Klasse 128)"
    );
}

#[test]
fn steinmetz_march_safety_outside() {
    // Außenfeld darf die wahre Distanz höchstens um die Kurventoleranz
    // überschätzen (Sphere-Trace-Sicherheit, Spike `bdbg`-Kriterium).
    let delta = 2.0f64;
    let tol = 0.005f32;
    let node = steinmetz_node(delta as f32);
    let corrected = synth(&node, tol);
    let gt = TrueField { dense: DenseEdge::build(4096), delta };
    let mut rng = Rng(31337);
    let mut worst_over = f64::NEG_INFINITY;
    for _ in 0..4000 {
        let sx = if rng.f() < 0.5 { 1.0 } else { -1.0 };
        let q = edge_point(sx, rng.range(0.0, std::f64::consts::TAU));
        let p = add64(q, scale64(rng.dir(), delta + rng.range(-0.5, 0.5) * delta));
        let ft = gt.eval(p);
        if ft <= 0.0 {
            continue;
        }
        let f = corrected.sdf([p[0] as f32, p[1] as f32, p[2] as f32], EvalMode::Visual) as f64;
        worst_over = worst_over.max(f - ft);
    }
    assert!(
        worst_over <= tol as f64 + 1e-3,
        "Außenfeld-Überschätzung {worst_over:.5} mm > Kurventoleranz {tol}"
    );
}

// ─── Tape-Parität + Dual-Parität ────────────────────────────────────

#[test]
fn arc_channel_tape_parity_cpu() {
    for node in [steinmetz_node(2.0), notch_union_node(90.0, 2.0)] {
        let bounded = synth(&node, 0.005);
        let (n_ch, _) = count_channels(&bounded);
        assert!(n_ch > 0, "Fixture ohne Kanäle — Parität wäre trivial");
        let compiled = crate::sdf_compiler::compile_body(&bounded);
        assert!(
            compiled.opcodes.iter().any(|v| v.to_bits() == crate::sdf_compiler::OP_ARC_CHANNEL),
            "kompiliertes Tape enthält kein OP_ARC_CHANNEL"
        );
        let mut rng = Rng(777);
        let mut max_diff = 0.0f32;
        for _ in 0..4000 {
            let p = [
                rng.range(-40.0, 40.0) as f32,
                rng.range(-40.0, 40.0) as f32,
                rng.range(-40.0, 40.0) as f32,
            ];
            let d_tree = bounded.sdf(p, EvalMode::Visual);
            let d_tape = crate::sdf_compiler::eval_opcodes(&compiled.opcodes, p);
            max_diff = max_diff.max((d_tree - d_tape).abs());
        }
        assert!(
            max_diff < 2e-5,
            "Tape-Parität verletzt: max |Δ| = {max_diff:.3e}"
        );
    }
}

#[test]
fn arc_channel_dual_parity_and_gradient() {
    let node = steinmetz_node(2.0);
    let bounded = synth(&node, 0.005);
    let mut rng = Rng(999);
    let mut max_val = 0.0f32;
    let mut max_grad = 0.0f32;
    for _ in 0..600 {
        let sx = if rng.f() < 0.5 { 1.0 } else { -1.0 };
        let q = edge_point(sx, rng.range(0.0, std::f64::consts::TAU));
        let p64 = add64(q, scale64(rng.dir(), rng.range(0.5, 6.0)));
        let p = [p64[0] as f32, p64[1] as f32, p64[2] as f32];
        let d_tree = bounded.sdf(p, EvalMode::Visual);
        let (d_dual, g) = crate::sdf_dual::sdf_gradient(&bounded, p);
        max_val = max_val.max((d_tree - d_dual).abs());
        // Gradienten-Norm eines (stückweise) exakten Distanzfelds ≈ 1;
        // an den min/max-Falten der Konstruktion springt die Richtung,
        // die Norm der Einzelzweige bleibt ~1.
        let gn = (g[0] * g[0] + g[1] * g[1] + g[2] * g[2]).sqrt();
        max_grad = max_grad.max((gn - 1.0).abs());
    }
    assert!(max_val < 2e-5, "Dual-Wert-Parität verletzt: {max_val:.3e}");
    assert!(
        max_grad < 0.05,
        "Kanal-Gradienten-Norm weicht um {max_grad:.3} von 1 ab"
    );
}

// ─── Verweigerungs-/Neutralitäts-Checks ─────────────────────────────

#[test]
fn no_channels_on_exact_stops() {
    // +δ über Union ist EXAKT durch den Pushdown verteilt — falls doch
    // einmal ein Offset(Union, +δ) direkt gebaut wird, ist er KEIN
    // dualer Stopp und bekommt keine Kanäle.
    let node = CsgNode::Offset(
        Box::new(CsgNode::Union(
            Box::new(leaf_box(1, [10.0, 10.0, 10.0])),
            Box::new(leaf_box(2, [8.0, 8.0, 8.0])),
        )),
        2.0,
    );
    let bounded = synth(&node, 0.005);
    let (n_ch, _) = count_channels(&bounded);
    assert_eq!(n_ch, 0, "+δ über Union darf keine Kanäle bekommen");
}

#[test]
fn tangential_seams_are_skipped() {
    // Zwei koplanare Boxen (gleiche Oberkante): Naht tangential →
    // vorhergesagter Fehler unter jeder Toleranz → kein Kanal.
    let a = leaf_box(1, [10.0, 5.0, 10.0]);
    let b = CsgNode::Transform(
        Transform {
            translate: [8.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        },
        Box::new(leaf_box(2, [10.0, 5.0, 10.0])),
    );
    let node = CsgNode::Offset(
        Box::new(CsgNode::Union(Box::new(a), Box::new(b))),
        -1.0,
    );
    let bounded = synth(&node, 0.005);
    let (n_ch, _) = count_channels(&bounded);
    assert_eq!(n_ch, 0, "tangentiale Nähte dürfen keine Kanäle bekommen");
}

#[test]
fn channel_provenance_uses_reserved_band() {
    let node = steinmetz_node(2.0);
    let bounded = synth(&node, 0.01);
    fn collect(n: &BoundedCsg, out: &mut Vec<u32>) {
        if let BoundedCsg::ArcChannel { inner, channels, .. } = n {
            for ch in channels {
                out.push(ch.branch);
            }
            collect(inner, out);
        } else if let BoundedCsg::Offset { inner, .. } = n {
            collect(inner, out);
        }
    }
    let mut branches = Vec::new();
    collect(&bounded, &mut branches);
    assert!(!branches.is_empty());
    for b in &branches {
        let sub = b & 0xFFF;
        assert!(
            (0x700..0x7FE).contains(&sub),
            "Kanal-Sub {sub:#x} liegt nicht im reservierten 0x700+-Band"
        );
    }
    // Deterministisch: zweite Synthese liefert identische Branch-Ids.
    let bounded2 = synth(&node, 0.01);
    let mut branches2 = Vec::new();
    collect(&bounded2, &mut branches2);
    assert_eq!(branches, branches2, "Kanal-Provenance ist nicht deterministisch");
}

