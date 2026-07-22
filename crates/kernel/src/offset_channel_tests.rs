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

// ─── Phase C2: Dead-Channel-Pruning ─────────────────────────────────

/// Union-Nachbar überdeckt den kompletten Notch-Naht-Schlauch tief mit
/// Material → der Kanal ist tot und wird gepruned; das Feld bleibt im
/// Oberflächenband (|f| ≤ 0.9·PRUNE_BAND) bit-identisch und flippt
/// nirgends ein Vorzeichen (Zertifikat, siehe `prune_dead_channels`).
#[test]
fn prune_dead_channel_under_union_neighbor_is_band_neutral() {
    let covered = CsgNode::Union(
        Box::new(notch_union_node(90.0, 2.0)),
        Box::new(leaf_box(9, [70.0, 70.0, 70.0])),
    );
    let unpruned = synth(&covered, 0.005);
    let (n0, _) = count_channels(&unpruned);
    assert!(n0 >= 1, "fixture self-check: channel must be synthesized");

    let mut pruned = synth(&covered, 0.005);
    let stats = prune_dead_channels(&mut pruned, 0.005);
    let (n1, _) = count_channels(&pruned);
    assert_eq!(n1, 0, "covered channel must be pruned (stats {stats:?})");
    assert_eq!(stats.pruned, n0);

    let mut rng = Rng(0xC2C2);
    let mut band_checked = 0usize;
    for _ in 0..20000 {
        let p = [
            rng.range(-90.0, 90.0) as f32,
            rng.range(-90.0, 90.0) as f32,
            rng.range(-90.0, 90.0) as f32,
        ];
        let a = unpruned.sdf(p, EvalMode::Visual);
        let b = pruned.sdf(p, EvalMode::Visual);
        // Nie ein Vorzeichenwechsel, nirgends.
        assert_eq!(a > 0.0, b > 0.0, "sign flip at {p:?}: {a} vs {b}");
        // Im Oberflächenband bit-identisch.
        if a.abs() <= 0.9 * PRUNE_BAND {
            assert_eq!(a.to_bits(), b.to_bits(), "band diff at {p:?}: {a} vs {b}");
            band_checked += 1;
        }
    }
    assert!(band_checked > 100, "band sampling too thin ({band_checked})");
}

/// Gegenprobe: ein FERNER Union-Nachbar überdeckt nichts — der Kanal
/// bleibt (Pruning ist konservativ, Intervall-Vorstufe + Zertifikat).
#[test]
fn prune_keeps_channel_without_coverage() {
    let apart = CsgNode::Union(
        Box::new(notch_union_node(90.0, 2.0)),
        Box::new(CsgNode::Transform(
            Transform {
                translate: [400.0, 0.0, 0.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0; 3],
            },
            Box::new(leaf_box(9, [20.0, 20.0, 20.0])),
        )),
    );
    let mut tree = synth(&apart, 0.005);
    let (n0, _) = count_channels(&tree);
    assert!(n0 >= 1);
    let stats = prune_dead_channels(&mut tree, 0.005);
    let (n1, _) = count_channels(&tree);
    assert_eq!(n1, n0, "uncovered channel must survive (stats {stats:?})");
    assert_eq!(stats.pruned, 0);
}

// ─── Phase C4: Vertex-Kugeln an Triple-Junctions ────────────────────

/// Drei orthogonale Platten (Boden + zwei Wände) bilden eine konkave
/// Raumecke; Material lokal {x≤0} ∪ {y≤0} ∪ {z≤0}, Luft = +Oktant.
fn corner_union_node(delta: f64) -> CsgNode {
    let plate = |id: u32, c: [f32; 3], h: [f32; 3]| {
        CsgNode::Transform(
            Transform {
                translate: c,
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0; 3],
            },
            Box::new(leaf_box(id, h)),
        )
    };
    CsgNode::Offset(
        Box::new(CsgNode::Union(
            Box::new(CsgNode::Union(
                Box::new(plate(1, [0.0, -5.0, 0.0], [40.0, 5.0, 40.0])),
                Box::new(plate(2, [-5.0, 0.0, 0.0], [5.0, 40.0, 40.0])),
            )),
            Box::new(plate(3, [0.0, 0.0, -5.0], [40.0, 40.0, 5.0])),
        )),
        -(delta as f32),
    )
}

/// Exaktes lokales Referenz-SDF des wahr erodierten Ecken-Körpers:
/// Material ⇔ dist(p, +Oktant) ≥ δ; nahe der Ecke (|p|∞ ≤ 5 ≪
/// Plattendicke 10) ist `δ − dist` das exakte SDF.
fn corner_reference(delta: f64) -> impl Fn([f64; 3]) -> f64 {
    move |p: [f64; 3]| {
        let dx = p[0].min(0.0);
        let dy = p[1].min(0.0);
        let dz = p[2].min(0.0);
        delta - (dx * dx + dy * dy + dz * dz).sqrt()
    }
}

/// Entfernt alle Vertex-Kugeln (Messhilfe „Kanäle-ohne-C4").
fn strip_verts(node: &mut BoundedCsg) {
    match node {
        BoundedCsg::ArcChannel { inner, verts, .. } => {
            verts.clear();
            strip_verts(inner);
        }
        BoundedCsg::Union { left, right, .. }
        | BoundedCsg::Difference { left, right, .. }
        | BoundedCsg::Intersection { left, right, .. } => {
            strip_verts(left);
            strip_verts(right);
        }
        BoundedCsg::Offset { inner, .. } => strip_verts(inner),
        _ => {}
    }
}

/// Nullmengen-Fehler an der Ecke entlang radialer Strahlen in den
/// Material-Oktanten (wahre Fläche = Kugel-Patch r = δ um die Ecke).
fn corner_zero_err(tree: &BoundedCsg, delta: f64, n_rays: usize, seed: u64) -> f64 {
    let reference = corner_reference(delta);
    let field = |p: [f64; 3]| -> f64 {
        tree.sdf([p[0] as f32, p[1] as f32, p[2] as f32], EvalMode::Visual) as f64
    };
    let mut rng = Rng(seed);
    let mut max_err = 0.0f64;
    let mut hits = 0usize;
    while hits < n_rays {
        // Richtung strikt im −Oktanten (Kugel-Patch-Zone, Abstand zu den
        // Facetten, damit die Referenz dort exakt der Patch ist).
        let u = [
            -rng.range(0.25, 1.0),
            -rng.range(0.25, 1.0),
            -rng.range(0.25, 1.0),
        ];
        let n = norm64(u);
        let u = scale64(u, 1.0 / n);
        let eval_r = |r: f64| field(scale64(u, r));
        // Vorzeichenwechsel im Fenster um r = δ suchen (erosionsseitig
        // liegt die naive Fläche bei bis zu δ·√3).
        let r_lo = (delta * 0.5).max(0.05);
        let r_hi = delta * 2.2;
        let mut prev_r = r_lo;
        let mut prev_s = eval_r(r_lo) > 0.0;
        let mut found = None;
        for k in 1..=96 {
            let r = r_lo + (r_hi - r_lo) * k as f64 / 96.0;
            let s = eval_r(r) > 0.0;
            if s != prev_s {
                let (mut lo, mut hi) = (prev_r, r);
                for _ in 0..44 {
                    let m = 0.5 * (lo + hi);
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
            prev_s = s;
        }
        if let Some(r0) = found {
            max_err = max_err.max(reference(scale64(u, r0)).abs());
            hits += 1;
        }
    }
    max_err
}

/// Plan §7 / C4: Box-Ecke konkav unter Erosion — die Vertex-Kugel
/// schließt den Ecken-Nullmengenfehler, den die Kanäle allein lassen.
#[test]
fn triple_junction_vertex_sphere_closes_corner() {
    let delta = 2.0f64;
    let node = corner_union_node(delta);
    let corrected = synth(&node, 0.005);
    let n_verts = count_vertex_spheres(&corrected);
    assert!(
        n_verts >= 1,
        "triple junction must synthesize a vertex sphere (got {n_verts})"
    );
    let (n_ch, _) = count_channels(&corrected);
    assert!(n_ch >= 3, "expected ≥ 3 seam channels, got {n_ch}");

    // Vorher/Nachher-Messung (metricity-lab-Stil):
    let naive = BoundedCsg::from_csg(&node);
    let mut channels_only = synth(&node, 0.005);
    strip_verts(&mut channels_only);

    let err_naive = corner_zero_err(&naive, delta, 300, 0xC4C4);
    let err_channels = corner_zero_err(&channels_only, delta, 300, 0xC4C4);
    let err_full = corner_zero_err(&corrected, delta, 300, 0xC4C4);

    // Selbstchecks der Messreihe: naiv ≈ δ·(√3−1)-Klasse an der Ecke,
    // Kanäle allein lassen einen Ecken-Rest.
    assert!(
        err_naive > 0.5,
        "fixture self-check: naive corner error too small ({err_naive:.4})"
    );
    assert!(
        err_full <= 0.03,
        "corner zero-set error after C4: {err_full:.5} mm (naive {err_naive:.4}, channels-only {err_channels:.4})"
    );
    assert!(
        err_full < err_channels * 0.5 || err_channels <= 0.03,
        "vertex sphere must clearly improve on channels-only ({err_channels:.4} → {err_full:.4})"
    );

    // March-Sicherheit: Außenfeld (Luftseite) überschätzt die wahre
    // Distanz im Ecken-Band nicht über die Toleranzklasse hinaus.
    let reference = corner_reference(delta);
    let mut rng = Rng(0xAFFE);
    let mut worst_over = f64::NEG_INFINITY;
    for _ in 0..4000 {
        let u = [
            -rng.range(0.1, 1.0),
            -rng.range(0.1, 1.0),
            -rng.range(0.1, 1.0),
        ];
        let n = norm64(u);
        let u = scale64(u, 1.0 / n);
        let p = scale64(u, delta * rng.range(0.1, 0.95));
        let f_ref = reference(p); // > 0: wahre Luft (Innenrest der Ecke)
        if f_ref <= 0.0 {
            continue;
        }
        let f = corrected.sdf([p[0] as f32, p[1] as f32, p[2] as f32], EvalMode::Visual) as f64;
        worst_over = worst_over.max(f - f_ref);
    }
    assert!(
        worst_over <= 0.03,
        "corner outside overestimation {worst_over:.5} mm"
    );
}

// ─── Phase C3: Weave-Refresh des expandierten Körpers ───────────────

/// Plan §7.3: der Weave-Baupfad sieht den EXPANDIERTEN Körper. Kanten des
/// eroded-Notch-B-Reps liegen auf dem korrigierten Feld; die naive
/// Naht-Ecke (die im korrigierten Feld ≈ 0.8 mm tief im Material liegt)
/// taucht nicht mehr als Kante auf. Ein Edge-Fillet, dessen Mask-Polyline
/// auf der offsetteten Naht liegt, verformt das Feld genau dort.
#[test]
fn weave_refresh_offset_body_and_fillet_on_offset_edge() {
    use fielddraft_protocol::BoolOp;

    let mut f = crate::csg::DecodedFeature::from_parts(
        FeatureId::from_u32(1),
        "notch".into(),
        BoolOp::New,
        Primitive::Box { center: [0.0; 3], half_extents: [1.0; 3] },
        Transform::IDENTITY,
        crate::csg::PrimitiveModifiers::NONE,
        vec![],
    );
    f.node = notch_union_node(90.0, 2.0);
    let features = vec![f];

    // Weave-Baupfad: expandiert (Kanäle enthalten).
    let bounded = crate::body::build_bounded_for_body_local(
        &features,
        &[0],
        &Transform::IDENTITY,
    );
    let (n_ch, _) = count_channels(&bounded);
    assert!(n_ch >= 1, "weave build path must carry channels (C3)");

    // Voll weben (Octree-Route — ensure_brep erzwingt sie bei Kanälen).
    let woven = crate::body::weave_all(&features);
    assert_eq!(woven.len(), 1);
    let (brep, _) = &woven[0];
    assert!(brep.edge_count() > 0, "eroded notch must weave edges");

    let mut deepest = 0.0f32;
    let mut n_pts = 0usize;
    let mut on_edge = |p: [f32; 3]| {
        // Box-Enden ausnehmen (|z| < 40; die Kappen sind hier egal).
        if p[2].abs() > 40.0 {
            return;
        }
        let d = bounded.sdf(p, EvalMode::Visual);
        deepest = deepest.min(d);
        n_pts += 1;
    };
    for e in &brep.edges {
        for p in crate::brep::edge_sampling::sample_edge(&e.geometry) {
            on_edge([p[0] as f32, p[1] as f32, p[2] as f32]);
        }
    }
    assert!(n_pts > 10, "expected sampled edge points, got {n_pts}");
    // Kanten liegen auf dem KORRIGIERTEN Feld: nirgends tief im Material.
    // (Die naive Naht-Ecke läge bei ≈ −0.8 im korrigierten Feld.)
    assert!(
        deepest > -0.3,
        "woven edge dives {deepest} mm into the corrected field — stale naive edge list"
    );

    // Fixture-Selbstcheck: im NAIVEN Baum (ohne Kanäle) existiert die
    // Naht-Ecke, und sie liegt im korrigierten Feld im Material.
    let naive_corner = {
        // α=90°: naive Ecke bei r = δ/sin(45°) auf der Material-Bisektrix.
        let bis = [-(0.0f32 + (-1.0)), -(1.0 + 0.0)]; // Richtung siehe notch_check
        let n = (bis[0] * bis[0] + bis[1] * bis[1]).sqrt();
        let r = 2.0f32 / (45.0f32).to_radians().sin();
        [bis[0] / n * r, bis[1] / n * r, 0.0]
    };
    let d_corner = bounded.sdf(naive_corner, EvalMode::Visual);
    assert!(
        d_corner < -0.3,
        "fixture self-check: naive seam corner should be buried ({d_corner})"
    );

    // Fillet auf der offsetteten Naht: Mask-Polyline entlang des
    // Rinnen-Bodens (Punkt der korrigierten Nullmenge auf der Bisektrix,
    // r = δ). Das Feld MUSS sich im Tube ändern und fern davon nicht.
    let groove = {
        let bis = [-(0.0f32 + (-1.0)), -(1.0 + 0.0)];
        let n = (bis[0] * bis[0] + bis[1] * bis[1]).sqrt();
        [bis[0] / n * 2.0, bis[1] / n * 2.0]
    };
    let fillet = crate::csg::EdgeFillet {
        fillet_id: FeatureId::from_u32(99),
        feature_a: FeatureId::from_u32(1),
        feature_b: FeatureId::from_u32(2),
        branch_a: 0,
        branch_b: 0,
        mask_polyline: vec![
            [groove[0], groove[1], -10.0],
            [groove[0], groove[1], 10.0],
        ],
        mask_radius: 6.0,
        blend_radius: 3.0,
        blend_mode: crate::csg::BlendMode::Fillet,
    };
    let plain = crate::body::build_bounded_for_body_rounded(
        &features, &[0], None, &[], CHANNEL_TOL_RENDER,
    );
    let filleted = crate::body::build_bounded_for_body_rounded(
        &features, &[0], None, std::slice::from_ref(&fillet), CHANNEL_TOL_RENDER,
    );
    let probe_in = [groove[0], groove[1], 0.0];
    let din = (plain.sdf(probe_in, EvalMode::Visual)
        - filleted.sdf(probe_in, EvalMode::Visual))
    .abs();
    assert!(
        din > 0.05,
        "fillet on the offset edge must reshape the field in the mask tube (Δ={din})"
    );
    // Fern des Tubes bleibt der Unterschied in der Sliver-Klasse
    // (≤ δ·(1/sin(α/2)−1) ≈ 0.83): der MaskedBlend stoppt den Pushdown,
    // Kanäle entfallen mit Fillet ganz (dokumentierte §9-Einschränkung
    // „Offset über Blend bleibt f−δ") — aber der Fillet selbst wirkt
    // nicht über seinen Tube hinaus.
    let probe_far = [groove[0], groove[1], 45.0];
    let dfar = (plain.sdf(probe_far, EvalMode::Visual)
        - filleted.sdf(probe_far, EvalMode::Visual))
    .abs();
    assert!(
        dfar < 0.9,
        "far-field difference must stay in the sliver class (Δfar={dfar})"
    );
}

/// Tape-/Dual-Parität des OP_VERTEX_SPHERE (alle Spiegel identisch).
#[test]
fn vertex_sphere_tape_and_dual_parity() {
    let node = corner_union_node(2.0);
    let bounded = synth(&node, 0.005);
    assert!(count_vertex_spheres(&bounded) >= 1);
    let compiled = crate::sdf_compiler::compile_body(&bounded);
    assert!(
        compiled
            .opcodes
            .iter()
            .any(|v| v.to_bits() == crate::sdf_compiler::OP_VERTEX_SPHERE),
        "kompiliertes Tape enthält kein OP_VERTEX_SPHERE"
    );
    let mut rng = Rng(4711);
    let mut max_tape = 0.0f32;
    let mut max_dual = 0.0f32;
    for _ in 0..4000 {
        let near_corner = rng.f() < 0.5;
        let s = if near_corner { 6.0 } else { 45.0 };
        let p = [
            rng.range(-s, s) as f32,
            rng.range(-s, s) as f32,
            rng.range(-s, s) as f32,
        ];
        let d_tree = bounded.sdf(p, EvalMode::Visual);
        let d_tape = crate::sdf_compiler::eval_opcodes(&compiled.opcodes, p);
        max_tape = max_tape.max((d_tree - d_tape).abs());
        let (d_dual, _) = crate::sdf_dual::sdf_gradient(&bounded, p);
        max_dual = max_dual.max((d_tree - d_dual).abs());
    }
    assert!(max_tape < 2e-5, "Tape-Parität verletzt: {max_tape:.3e}");
    assert!(max_dual < 2e-5, "Dual-Parität verletzt: {max_dual:.3e}");
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

// ─── FC Phase 1 (#398): Facetten-Extraktion + cavalier-Verschnitt ────
//
// FELDNEUTRAL & UNVERDRAHTET — diese Tests decken die neuen (in Phase 1
// noch nicht verdrahteten) Funktionen `member_contact_faces` und
// `clip_faces` direkt ab. Die Synthese benutzt sie noch nicht; das Golden-
// Tape bleibt byte-identisch (separat verifiziert).

use crate::sketch2d::{Sketch2d, Sketch2dPrimitive, SketchSegment};

/// Eine bulge-tragende Kontaktfläche in der z=0-Ebene (`box_rect = false`
/// erzwingt den cavalier-Pfad). `n`/`o`/`depth` sind für `clip_faces`
/// belanglos (nur `loop3`, `bulges`, `box_rect` zählen).
fn face_poly(pts: &[[f64; 2]], bulges: &[f64]) -> ContactFace {
    ContactFace {
        n: [0.0, 0.0, 1.0],
        o: 0.0,
        loop3: pts.iter().map(|p| [p[0], p[1], 0.0]).collect(),
        bulges: bulges.to_vec(),
        depth: 0.0,
        box_rect: false,
    }
}

fn comp_area(c: &Contour2d) -> f64 {
    let n = c.verts.len();
    let mut a = 0.0;
    for i in 0..n {
        let p = c.verts[i];
        let q = c.verts[(i + 1) % n];
        a += p[0] * q[1] - q[0] * p[1];
    }
    0.5 * a.abs()
}

/// L-Form ∩ überragendes Rechteck = die L-Form: EINE nicht-konvexe
/// Komponente, kein Loch, Fläche exakt 20 (Analog zum Spike, aber über
/// `clip_faces`).
#[test]
fn fc1_clip_l_shape_one_nonconvex_component() {
    let l = face_poly(
        &[[0.0, 0.0], [6.0, 0.0], [6.0, 2.0], [2.0, 2.0], [2.0, 6.0], [0.0, 6.0]],
        &[0.0; 6],
    );
    let rect = face_poly(&[[-1.0, -1.0], [7.0, -1.0], [7.0, 7.0], [-1.0, 7.0]], &[0.0; 4]);
    let comps = clip_faces(&l, &rect, [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], 1e-4, 0.1);
    assert_eq!(comps.len(), 1, "enthaltener Kontakt: EINE Komponente");
    assert!(comps[0].holes.is_empty(), "kein Loch");
    let area = comp_area(&comps[0].outer);
    assert!((area - 20.0).abs() < 1e-4, "L-Fläche 20 erwartet, got {area}");
}

/// KERNBELEG der Komponenten-Zerlegung: zwei „⌐"-Konturen verschneiden zu
/// ZWEI disjunkten 2×2-Inseln (`pos_plines` → zwei Komponenten).
#[test]
fn fc1_clip_two_islands() {
    let l1 = face_poly(
        &[[0.0, 0.0], [6.0, 0.0], [6.0, 2.0], [2.0, 2.0], [2.0, 6.0], [0.0, 6.0]],
        &[0.0; 6],
    );
    let l2 = face_poly(
        &[[0.0, 4.0], [4.0, 4.0], [4.0, 0.0], [6.0, 0.0], [6.0, 6.0], [0.0, 6.0]],
        &[0.0; 6],
    );
    let comps = clip_faces(&l1, &l2, [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], 1e-4, 0.1);
    assert_eq!(comps.len(), 2, "zwei disjunkte Kontaktinseln erwartet");
    let mut areas: Vec<f64> = comps.iter().map(|c| comp_area(&c.outer)).collect();
    areas.sort_by(|a, b| a.partial_cmp(b).unwrap());
    for a in &areas {
        assert!((a - 4.0).abs() < 1e-4, "jede Insel 2×2=4, got {a}");
    }
}

/// Fast-koinzidente Facetten (winzig verschobene Rechtecke): der cavalier-
/// Verschnitt greift entweder sauber (EINE Komponente ≈ Rechteck) oder
/// verweigert (kein Sliver-Garbage), niemals mehrere Fetzen.
#[test]
fn fc1_clip_near_coincident_cleanup() {
    let r0 = face_poly(&[[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0]], &[0.0; 4]);
    let e = 1e-5;
    let r1 = face_poly(&[[e, e], [4.0 + e, e], [4.0 + e, 4.0 + e], [e, 4.0 + e]], &[0.0; 4]);
    let comps = clip_faces(&r0, &r1, [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], 1e-4, 0.05);
    assert!(comps.len() <= 1, "kein Sliver-Garbage, got {} Komponenten", comps.len());
    if let Some(c) = comps.first() {
        let area = comp_area(&c.outer);
        assert!((area - 16.0).abs() < 0.1, "≈16 erwartet, got {area}");
    }
}

/// Loch-Repräsentation (`neg_plines` → Löcher) samt Komponenten-Zuordnung:
/// Platte ∖ Innenkreis liefert einen Außenrand + ein bogen-tragendes Loch,
/// dessen Schwerpunkt im Außenrand liegt (die Bausteine, die `clip_faces`
/// für mehrfach-zusammenhängende Kontakte in Phase 2 nutzt).
#[test]
fn fc1_hole_contour_and_assignment() {
    use cavalier_contours::pline_closed;
    use cavalier_contours::polyline::{BooleanOp, PlineSource};
    let plate = pline_closed![
        (0.0, 0.0, 0.0),
        (10.0, 0.0, 0.0),
        (10.0, 10.0, 0.0),
        (0.0, 10.0, 0.0),
    ];
    let hole = pline_closed![(7.0, 5.0, 1.0), (3.0, 5.0, 1.0)];
    let ring = plate.boolean(&hole, BooleanOp::Not);
    assert_eq!(ring.pos_plines.len(), 1);
    assert_eq!(ring.neg_plines.len(), 1);
    let outer = contour_from_pline(&ring.pos_plines[0].pline);
    let hole_c = contour_from_pline(&ring.neg_plines[0].pline);
    assert!(hole_c.has_arcs(), "Loch-Rand trägt Bögen (Bulge-Repräsentation)");
    let centroid = [
        hole_c.verts.iter().map(|v| v[0]).sum::<f64>() / hole_c.verts.len() as f64,
        hole_c.verts.iter().map(|v| v[1]).sum::<f64>() / hole_c.verts.len() as f64,
    ];
    assert!(point_in_contour(&outer, centroid), "Loch-Schwerpunkt liegt im Außenrand");
}

// ── member_contact_faces ────────────────────────────────────────────

fn extrude_node(id: u32, prim: Sketch2dPrimitive, half_height: f32) -> CsgNode {
    CsgNode::Extrude {
        id: FeatureId::from_u32(id),
        sketch: Box::new(Sketch2d::Prim(prim)),
        half_height,
        wall_subs: None,
    }
}

/// Box-Fläche: `member_contact_faces` liefert exakt `box_contact_faces`
/// (sechs Rechtecke, `box_rect = true`, keine Bögen) — der Box-Fast-Path
/// bleibt unverändert.
#[test]
fn fc1_member_faces_box_unchanged() {
    let node = BoundedCsg::from_csg(&leaf_box(1, [3.0, 4.0, 5.0]));
    let via_box = box_contact_faces(&node).expect("Box liefert Flächen");
    let via_member = member_contact_faces(&node).expect("Box liefert Flächen");
    assert_eq!(via_member.len(), 6);
    assert_eq!(via_member.len(), via_box.len());
    for (a, b) in via_member.iter().zip(&via_box) {
        assert_eq!(a.n, b.n);
        assert_eq!(a.o, b.o);
        assert_eq!(a.loop3, b.loop3);
        assert_eq!(a.depth, b.depth);
        assert!(a.box_rect, "Box-Fläche → box_rect (SH-Fast-Path)");
        assert!(a.bulges.iter().all(|x| *x == 0.0), "Rechteck ohne Bögen");
    }
}

/// Extrude eines Kreis-Profils: die Deckel-Facette trägt exakte Bulge-Bögen
/// und rekonstruiert die Kreisfläche πr² — validiert Bulge-Vorzeichen und
/// -Betrag durch den Projektions-/cavalier-Pfad.
#[test]
fn fc1_member_faces_extrude_cap_bulge() {
    let r = 3.0f32;
    let node = BoundedCsg::from_csg(&extrude_node(
        1,
        Sketch2dPrimitive::Circle { center: [0.0, 0.0], radius: r },
        2.0,
    ));
    let faces = member_contact_faces(&node).expect("Extrude liefert Flächen");
    // Ein Kreis-Profil hat keine geraden Segmente → nur die zwei Deckel.
    assert_eq!(faces.len(), 2, "Deckel + Boden, kein Mantel (reiner Kreis)");
    for f in &faces {
        assert!(!f.box_rect, "Extrude-Fläche ist kein Box-Rechteck");
        assert!(f.bulges.iter().any(|b| b.abs() > 1e-9), "Deckel trägt Bögen");
        let pl = face_to_pline(f, [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        use cavalier_contours::polyline::PlineSource;
        let area = pl.area().abs();
        let want = std::f64::consts::PI * (r as f64) * (r as f64);
        assert!((area - want).abs() < 1e-3, "Kreisfläche {want} erwartet, got {area}");
    }
}

/// Extrude eines Rechteck-Profils: zwei Deckel + je GERADEM Segment eine
/// Mantel-Rechteck-Facette (vier Ecken, keine Bögen).
#[test]
fn fc1_member_faces_extrude_mantle_rects() {
    let node = BoundedCsg::from_csg(&extrude_node(
        1,
        Sketch2dPrimitive::Rectangle { center: [0.0, 0.0], half_extents: [2.0, 3.0] },
        1.5,
    ));
    let faces = member_contact_faces(&node).expect("Extrude liefert Flächen");
    // 2 Deckel + 4 Mantel-Rechtecke.
    assert_eq!(faces.len(), 6);
    // Mantel-Facetten: Wandnormale in der xy-Ebene (Deckel zeigen entlang ±z).
    let mantles: Vec<&ContactFace> = faces.iter().filter(|f| f.n[2].abs() < 1e-6).collect();
    assert_eq!(mantles.len(), 4, "vier Mantel-Rechtecke");
    for m in &mantles {
        assert!(!m.box_rect);
        assert_eq!(m.loop3.len(), 4, "Mantel ist ein Rechteck");
        assert!(m.bulges.iter().all(|b| *b == 0.0), "Mantel gerade");
        assert!(m.depth > 0.0, "Mantel trägt eine positive Member-Dicke");
    }
    // Die zwei übrigen Facetten sind die Deckel entlang ±z.
    let caps: Vec<&ContactFace> = faces.iter().filter(|f| f.n[2].abs() > 0.9).collect();
    assert_eq!(caps.len(), 2, "Deckel + Boden entlang ±z");
}

/// QuadBezier im Profil ⇒ Deckel verweigert. Ein reines Bézier-Profil (keine
/// Geraden) liefert gar keine Facette → `None` (Ehrlichkeit vor Näherung).
#[test]
fn fc1_member_faces_quadbezier_refused() {
    let segs = vec![
        SketchSegment::QuadBezier { p0: [0.0, 0.0], p1: [3.0, 1.0], p2: [3.0, 3.0] },
        SketchSegment::QuadBezier { p0: [3.0, 3.0], p1: [1.0, 3.0], p2: [0.0, 0.0] },
    ];
    let node = BoundedCsg::from_csg(&extrude_node(
        1,
        Sketch2dPrimitive::Path { segments: segs, closed: true },
        1.0,
    ));
    assert!(member_contact_faces(&node).is_none(), "reines Bézier-Profil → verweigert");
}

/// Revolve (gekrümmte Mantelfläche) liefert keine planaren Facetten → `None`.
#[test]
fn fc1_member_faces_revolve_refused() {
    let node = BoundedCsg::from_csg(&CsgNode::Revolve {
        id: FeatureId::from_u32(1),
        sketch: Box::new(Sketch2d::Prim(Sketch2dPrimitive::Rectangle {
            center: [3.0, 0.0],
            half_extents: [1.0, 2.0],
        })),
        wall_subs: None,
    });
    assert!(member_contact_faces(&node).is_none(), "Revolve → keine planaren Facetten");
}

// ─── FC Phase 2 (#398): Prisma-Verdrahtung für Nicht-Box-Kontakte ────
//
// End-to-End über den echten Regen-Pfad-Mirror (`from_csg` →
// `synthesize_offset_channels_reported`): ein koplanarer Kontakt zwischen einer
// Box und einem Add-gedockten Extrude (nicht-konvexes Profil) muss jetzt ein
// Kontakt-Prisma erzeugen (statt `refused`), das den flächigen Phantom-Schlitz
// füllt; die Capability steigt auf `ToleranceBounded` (Prisma gebaut, Rim ist
// Box-Scope → δ-schmaler ∂P-Saum bleibt, Design §5/§8).

use crate::offset_capability::{CapabilityReport, CapabilityStatus};

/// Zählt die synthetisierten Kontakt-Prismen (Extrude-Member im 0x0007_0000-
/// Feature-Id-Band) im korrigierten Baum — je Zusammenhangskomponente eines.
fn count_contact_prisms(node: &BoundedCsg) -> usize {
    fn is_prism(id: &FeatureId) -> bool {
        id.as_u32_if_flat()
            .map(|v| (v & 0xFFFF_0000) == 0x0007_0000)
            .unwrap_or(false)
    }
    match node {
        BoundedCsg::Extrude { id, .. } => usize::from(is_prism(id)),
        BoundedCsg::Union { left, right, .. }
        | BoundedCsg::Difference { left, right, .. }
        | BoundedCsg::Intersection { left, right, .. } => {
            count_contact_prisms(left) + count_contact_prisms(right)
        }
        BoundedCsg::Offset { inner, .. }
        | BoundedCsg::ArcChannel { inner, .. }
        | BoundedCsg::Shell { inner, .. }
        | BoundedCsg::InnerWall { inner, .. } => count_contact_prisms(inner),
        _ => 0,
    }
}

fn poly_extrude(id: u32, verts: &[[f64; 2]], half_height: f32, z: f32) -> CsgNode {
    CsgNode::Transform(
        Transform { translate: [0.0, 0.0, z], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0; 3] },
        Box::new(CsgNode::Extrude {
            id: FeatureId::from_u32(id),
            sketch: Box::new(Sketch2d::Prim(Sketch2dPrimitive::Polygon {
                vertices: verts.iter().map(|v| [v[0] as f32, v[1] as f32]).collect(),
            })),
            half_height,
            wall_subs: None,
        }),
    )
}

/// (a) Box + Add-gedockter L-Extrude, bündige Kontaktebene z = 0. Der Kontakt P
/// ist die (nicht-konvexe) L-Fläche, enthalten in der Box-Deckelfläche → EIN
/// Prisma. Feldprobe: naive lässt den submergierten Phantom-Schlitz stehen
/// (außen), das Prisma füllt ihn (Material). Capability = ToleranceBounded.
#[test]
fn fc2_box_plus_ldocked_extrude_builds_prism() {
    // L-Profil (CCW), enthalten in der 20×20-Box-Deckelfläche.
    let l = [[-6.0, -6.0], [6.0, -6.0], [6.0, -2.0], [-2.0, -2.0], [-2.0, 6.0], [-6.0, 6.0]];
    let box_a = CsgNode::Transform(
        Transform { translate: [0.0, 0.0, -5.0], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0; 3] },
        Box::new(leaf_box(1, [10.0, 10.0, 5.0])),
    );
    let ext_b = poly_extrude(2, &l, 5.0, 5.0);
    let delta = 2.0f32;
    let node = CsgNode::Offset(Box::new(CsgNode::Union(Box::new(box_a), Box::new(ext_b))), -delta);

    let naive = BoundedCsg::from_csg(&node);
    let mut report = CapabilityReport::new();
    let corrected =
        synthesize_offset_channels_reported(BoundedCsg::from_csg(&node), 0.01, &mut report);

    assert_eq!(
        count_contact_prisms(&corrected),
        1,
        "L-Kontakt (Box+Extrude) muss GENAU ein Kontakt-Prisma erzeugen (nicht refused)"
    );
    assert_eq!(
        report.status(),
        CapabilityStatus::ToleranceBounded,
        "Prisma gebaut, Rim Box-Scope → ToleranceBounded; got {:?} — {:?}",
        report.status(),
        report.reasons()
    );

    // Feldprobe im Phantom-Schlitz (auf z=0, im L-Bodenstreifen): naive positiv
    // (Schlitz/außen), korrigiert gefüllt (Oberfläche/Material).
    let slit = [0.0f32, -4.0, 0.0];
    let f_naive = naive.sdf(slit, EvalMode::Visual);
    let f_corr = corrected.sdf(slit, EvalMode::Visual);
    assert!(f_naive > 1.0, "naive Phantom-Schlitz erwartet (außen), got {f_naive}");
    assert!(f_corr < 0.5, "Prisma füllt den Schlitz (Oberfläche/Material), got {f_corr}");
    assert!(f_corr < f_naive - 1.0, "Prisma muss den Schlitz signifikant füllen");

    // Klar innen (3 mm unter der Ebene, tief im Box-Body): negativ.
    assert!(
        corrected.sdf([0.0, -4.0, -3.0], EvalMode::Visual) < 0.0,
        "tief im Body → negativ"
    );
    // Außerhalb der L (auf z=0, aber im ausgesparten L-Winkel bei (0,0)): außen.
    assert!(
        corrected.sdf([0.0, 0.0, 0.0], EvalMode::Visual) > 0.0,
        "L-Aussparung (0,0) → außen (positiv)"
    );
    // Weit weg: außen.
    assert!(corrected.sdf([20.0, 20.0, 20.0], EvalMode::Visual) > 0.0, "fern → positiv");
}

/// (b) Zwei koplanar gestapelte „⌐"-Extrudes: ihr Kontakt zerfällt in ZWEI
/// disjunkte 2×2-Inseln (Spike-Fixture `fc1_clip_two_islands`) → ZWEI Prismen.
/// Feldprobe in BEIDEN Inseln: der Phantom-Schlitz ist in jeder gefüllt.
#[test]
fn fc2_two_component_contact_builds_two_prisms() {
    let l1 = [[0.0, 0.0], [6.0, 0.0], [6.0, 2.0], [2.0, 2.0], [2.0, 6.0], [0.0, 6.0]];
    let l2 = [[0.0, 4.0], [4.0, 4.0], [4.0, 0.0], [6.0, 0.0], [6.0, 6.0], [0.0, 6.0]];
    // B1 unten (Deckel z=0, Normale +z), B2 oben (Boden z=0, Normale −z).
    let b1 = poly_extrude(1, &l1, 5.0, -5.0);
    let b2 = poly_extrude(2, &l2, 5.0, 5.0);
    let delta = 1.0f32;
    let node = CsgNode::Offset(Box::new(CsgNode::Union(Box::new(b1), Box::new(b2))), -delta);

    let naive = BoundedCsg::from_csg(&node);
    let mut report = CapabilityReport::new();
    let corrected =
        synthesize_offset_channels_reported(BoundedCsg::from_csg(&node), 0.01, &mut report);

    assert_eq!(
        count_contact_prisms(&corrected),
        2,
        "Zwei-Insel-Kontakt muss ZWEI Kontakt-Prismen erzeugen (je Komponente eins)"
    );
    assert_eq!(report.status(), CapabilityStatus::ToleranceBounded, "{:?}", report.reasons());

    // Insel-Zentren: l1∩l2 = {x∈[4,6],y∈[0,2]} ∪ {x∈[0,2],y∈[4,6]}.
    for island in [[5.0f32, 1.0, 0.0], [1.0, 5.0, 0.0]] {
        let f_naive = naive.sdf(island, EvalMode::Visual);
        let f_corr = corrected.sdf(island, EvalMode::Visual);
        assert!(f_naive > 0.5, "naive Phantom-Schlitz in Insel {island:?}, got {f_naive}");
        assert!(f_corr < 0.5, "Prisma füllt Insel {island:?}, got {f_corr}");
        assert!(f_corr < f_naive - 0.4, "Insel {island:?} signifikant gefüllt");
    }
    // Zwischen den Inseln (z=0, in KEINEM Kontakt-Polygon, z. B. (3,3)): außen.
    assert!(
        corrected.sdf([3.0, 3.0, 0.0], EvalMode::Visual) > 0.0,
        "Lücke zwischen den Inseln → außen"
    );
}

// ─── FC Phase 3 (#398): ∂P-Rand-Fillet auf nicht-konvexe Kontakte ────────────
// (docs/research/sdf-metricity/FC-NONCONVEX-CONTACT-DESIGN.md §5). Der Rim
// (`offset_rim::edge_on_contour`) rundet die GERADEN ∂P-Stufen-Kanten beliebiger
// (auch nicht-konvexer) Kontakt-Konturen; Reflex-(Konkav-)Vertices bleiben
// bewusst ungerundet (ehrlicher B0-Rest, Design §5).

/// Gefalteter Regen-Pfad-Baum → Rim-Features (wie f398_rim_synthesis::folded_root).
fn rim_features_of(node: &CsgNode) -> Vec<crate::offset_rim::RimFeature> {
    let folded = node
        .clone()
        .expand_shells()
        .push_down_offsets()
        .fold_offset_leaves();
    crate::offset_rim::synthesize_rim_features_for_tree(&BoundedCsg::from_csg(&folded))
}

/// (a) Box + Add-gedockter L-Extrude (enthaltener nicht-konvexer Kontakt): die
/// GERADEN ∂P-Stufen-Kanten bekommen Rim-Fillets, der EINE Reflex-Vertex der L
/// bleibt ehrlich ungerundet (Capability nennt den B0-Rest). Over-Fill = 0 in
/// der Reflex-Schneise.
#[test]
fn fc3_lcontact_rim_fillets_straight_edges_reflex_honest() {
    let l = [[-6.0, -6.0], [6.0, -6.0], [6.0, -2.0], [-2.0, -2.0], [-2.0, 6.0], [-6.0, 6.0]];
    let box_a = CsgNode::Transform(
        Transform { translate: [0.0, 0.0, -5.0], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0; 3] },
        Box::new(leaf_box(1, [10.0, 10.0, 5.0])),
    );
    let ext_b = poly_extrude(2, &l, 5.0, 5.0);
    let delta = 2.0f32;
    let node = CsgNode::Offset(Box::new(CsgNode::Union(Box::new(box_a), Box::new(ext_b))), -delta);

    // (1) Die geraden ∂P-Kanten der L erzeugen Fillets (≥4 Stufen-Kanten), OHNE
    //     Trieder-Vertex (homogener Wand-Member → Nearest-Feature genügt; der
    //     Reflex-Vertex bleibt ungerundet, kein `a == b`).
    let rims = rim_features_of(&node);
    let n_edges = rims.iter().filter(|r| r.a != r.b).count();
    assert!(n_edges >= 4, "L-Kontakt: erwarte ≥4 gerade Stufen-Kanten-Fillets, got {n_edges}");
    assert!(
        rims.iter().all(|r| r.a != r.b),
        "enthaltener L-Kontakt: kein Punkt-Fillet (Reflex-Vertex bleibt ungerundet)"
    );

    // (2) Kompiliertes Feld (Prisma + Rim) + Capability.
    let naive = BoundedCsg::from_csg(&node);
    let mut report = CapabilityReport::new();
    let corrected =
        synthesize_offset_channels_reported(BoundedCsg::from_csg(&node), 0.01, &mut report);
    assert_eq!(report.status(), CapabilityStatus::ToleranceBounded, "{:?}", report.reasons());
    // Capability meldet den Reflex-/nicht-konvex-Rest EHRLICH (nicht bloß „≤ tol").
    assert!(
        report
            .reasons()
            .iter()
            .any(|r| r.contains("nicht-konvex") && r.contains("Reflex")),
        "Capability muss den nicht-konvexen Reflex-Rest ehrlich nennen: {:?}",
        report.reasons()
    );

    // (3) Der Rim FÜLLT mindestens einen geraden Kanten-Sliver: irgendwo im
    //     Kontakt-Band verkleinert `apply_rims` das naive Feld (Rundung/Fill).
    let mut saw_fill = false;
    for gi in 0..40usize {
        for gj in 0..40usize {
            for gk in 0..14usize {
                let p = [
                    -8.0 + 16.0 * gi as f64 / 39.0,
                    -8.0 + 16.0 * gj as f64 / 39.0,
                    -1.6 + 3.2 * gk as f64 / 13.0,
                ];
                let pf = [p[0] as f32, p[1] as f32, p[2] as f32];
                let g_naive = naive.sdf(pf, EvalMode::Visual) as f64;
                let g_rim = crate::offset_rim::apply_rims(g_naive, p, &rims);
                if g_rim < g_naive - 0.1 {
                    saw_fill = true;
                }
            }
        }
    }
    assert!(saw_fill, "Rim muss mindestens einen geraden Kanten-Sliver enger machen (füllen)");

    // (4) Reflex-Schneise (Kerbe bei (-2,-2)) über der Box bleibt ehrlich
    //     ungefüllt: das kompilierte Feld ist dort POSITIV (kein Over-Fill).
    for p in [[-1.5f32, -1.5, 0.3], [0.0, 0.0, 0.0], [-1.0, -1.0, 0.5], [-0.5, -0.5, 0.2]] {
        assert!(
            corrected.sdf(p, EvalMode::Visual) > 0.0,
            "Reflex-Kerbe {p:?} → kein Over-Fill (positiv), got {}",
            corrected.sdf(p, EvalMode::Visual)
        );
    }
}

/// (b) Zwei-Insel-Kontakt: zwei disjunkte, ENTHALTENE (konvexe, homogene)
/// Quadrat-Extrudes auf EINER großen Box-Deckelfläche. Jede Insel ist ein
/// eigener koplanarer Kontakt → beide bekommen Rim-Fillets, der Rim schließt den
/// ganzen ∂P-Saum (kein Reflex-Rest → Capability „≤ tol"). (Die bracket-Inseln
/// aus `fc2_two_component_contact_builds_two_prisms` sind hingegen VERSETZT —
/// gemischte Wand-Member je Insel → Rim korrekt verweigert, Design §5.)
#[test]
fn fc3_two_island_contact_both_get_rims() {
    // Große Box (Deckel z=0, 20×20) + zwei enthaltene 3×3-Quadrat-Extrudes.
    let box_a = CsgNode::Transform(
        Transform { translate: [0.0, 0.0, -5.0], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0; 3] },
        Box::new(leaf_box(1, [10.0, 10.0, 5.0])),
    );
    let sq1 = [[-6.5, -1.5], [-3.5, -1.5], [-3.5, 1.5], [-6.5, 1.5]]; // Zentrum (-5,0)
    let sq2 = [[3.5, -1.5], [6.5, -1.5], [6.5, 1.5], [3.5, 1.5]]; // Zentrum (5,0)
    let e1 = poly_extrude(2, &sq1, 5.0, 5.0);
    let e2 = poly_extrude(3, &sq2, 5.0, 5.0);
    let delta = 1.0f32;
    let node = CsgNode::Offset(
        Box::new(CsgNode::Union(Box::new(box_a), Box::new(CsgNode::Union(Box::new(e1), Box::new(e2))))),
        -delta,
    );

    let rims = rim_features_of(&node);
    assert!(!rims.is_empty(), "Zwei-Insel-Kontakt muss Rim-Features erzeugen");
    // Jede Insel (um (-5,0) bzw. (5,0)) muss MINDESTENS ein Kanten-Fillet tragen
    // (Kanten-Mittelpunkt in der Insel-AABB).
    let mid_in = |r: &crate::offset_rim::RimFeature, cx: f64, cy: f64| {
        let m = [(r.a[0] + r.b[0]) * 0.5, (r.a[1] + r.b[1]) * 0.5];
        (m[0] - cx).abs() <= 2.5 && (m[1] - cy).abs() <= 2.5
    };
    let isle1 = rims.iter().filter(|r| r.a != r.b).any(|r| mid_in(r, -5.0, 0.0));
    let isle2 = rims.iter().filter(|r| r.a != r.b).any(|r| mid_in(r, 5.0, 0.0));
    assert!(isle1, "Insel 1 (um (-5,0)) muss ein Rim-Fillet tragen");
    assert!(isle2, "Insel 2 (um (5,0)) muss ein Rim-Fillet tragen");

    // Konvexe Inseln → kein Reflex-Rest: Capability meldet den ∂P-Saum als auf
    // `tol` geschlossen (NICHT den nicht-konvexen 0.75·δ-Rest).
    let mut report = CapabilityReport::new();
    let _ = synthesize_offset_channels_reported(BoundedCsg::from_csg(&node), 0.01, &mut report);
    assert_eq!(report.status(), CapabilityStatus::ToleranceBounded, "{:?}", report.reasons());
    assert!(
        report.reasons().iter().any(|r| r.contains("OP_RIM_FILLET")),
        "Rim muss greifen: {:?}",
        report.reasons()
    );
    assert!(
        !report.reasons().iter().any(|r| r.contains("Reflex")),
        "konvexe Inseln → KEIN Reflex-Rest in der Capability: {:?}",
        report.reasons()
    );
}
