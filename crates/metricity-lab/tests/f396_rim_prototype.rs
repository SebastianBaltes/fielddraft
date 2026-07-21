//! F396 — Prototyp & Messung: additiv-beschränkter, halbraum-maskierter
//! Rand-Fillet-Opcode für den ∂P-Rest der Kontakt-Prisma-Korrektur
//! (Design-/Mess-Spike, KEIN Produktions-Opcode; siehe
//! `docs/research/sdf-metricity/F396-RIM-OPCODE-DESIGN.md`).
//!
//! Ausgangslage (#395 Stufe 1/3 im Kernel): das Kontakt-Prisma tilgt den
//! flächigen 2D-Innenfehler eines koplanaren Kontakts vollständig, lässt aber
//! an der Kontaktrand-Reflexkante ∂P einen δ-schmalen konkaven B0-Sliver
//! (Erosion) bzw. konvexen Keil (Dilation) stehen. Der bestehende Arc-Kanal
//! (`OP_ARC_CHANNEL`) kann ihn NICHT schließen (F395 §Umsetzungs-Status: die
//! Kegel-Klemme setzt auf Π `w_geo=0` → Phantom-Patch zurück).
//!
//! Dieser Spike prüft einen NEUEN Korrektur-Baustein rein in metricity-lab
//! (f64-Wrapper AUF dem echten Kernel-Prisma-Feld, keine Kernel-Änderung):
//!
//!   Reflex-Fillet:  g_neu(p) = min( g_prisma(p),  δ − dist(p, ∂P-Reflexkante) )
//!                   NUR innerhalb der Material-Halbraum-Maske beider Wände
//!                   und innerhalb der Trust-Region (dist ≤ K).
//!
//! Kern-Herleitung (Box-Stack he[10,10,10] ⊃ he[8,7,6], Erosion): quer zur
//! Reflexkante ist die WAHRE Erosion U⊖δ ein Viertelkreis-Fillet vom Radius δ
//! um die Kante (die Kante liegt IN der Kontaktebene Π), also f_U = −dist(p,
//! Kante) in der Materialschneise. Das naive min(f_A,f_B,f_S) legt dort die
//! SCHARFE Chebyshev-Ecke bei (−δ,−δ) → Nullmengen-Fehler δ·(√2−1) ≈ 0.414·δ
//! an der Kante. Der Fillet-Term ersetzt exakt die Ecke durch den Kreis. Auf Π
//! selbst gilt −dist(Kante) = f_S (Prisma) → KEINE Über-Addition (der Grund,
//! warum der Term der Kegel-Klemme entgeht).
//!
//! Lauf: `cargo test -p metricity-lab --test f396_rim_prototype -- --nocapture`

use rayon::prelude::*;

use fielddraft_kernel::bvh::BoundedCsg;
use fielddraft_kernel::csg::{CsgNode, EvalMode};
use fielddraft_kernel::offset_capability::CapabilityReport;
use fielddraft_kernel::offset_channel::{
    prune_dead_channels, synthesize_offset_channels_reported, CHANNEL_TOL_RENDER,
};
use fielddraft_protocol::{FeatureId, Primitive, Transform};

use metricity_lab::field::{CsgField, Field, FnField};
use metricity_lab::ground_truth::{GroundTruth, GtOptions};
use metricity_lab::util::SplitMix64;

// ─── f64-Vektor-/Quaternion-Helfer ─────────────────────────────────────────

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] { [a[0] - b[0], a[1] - b[1], a[2] - b[2]] }
fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] { [a[0] + b[0], a[1] + b[1], a[2] + b[2]] }
fn scl(a: [f64; 3], s: f64) -> [f64; 3] { [a[0] * s, a[1] * s, a[2] * s] }
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 { a[0] * b[0] + a[1] * b[1] + a[2] * b[2] }
fn norm(a: [f64; 3]) -> f64 { dot(a, a).sqrt() }

/// Rotiert `v` mit Einheitsquaternion `q = (x,y,z,w)`.
fn qrot(q: [f64; 4], v: [f64; 3]) -> [f64; 3] {
    let (x, y, z, w) = (q[0], q[1], q[2], q[3]);
    let u = [x, y, z];
    let uv = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
    let uuv = [u[1] * uv[2] - u[2] * uv[1], u[2] * uv[0] - u[0] * uv[2], u[0] * uv[1] - u[1] * uv[0]];
    [
        v[0] + 2.0 * (w * uv[0] + uuv[0]),
        v[1] + 2.0 * (w * uv[1] + uuv[1]),
        v[2] + 2.0 * (w * uv[2] + uuv[2]),
    ]
}

fn quat_axis_angle(axis: [f64; 3], ang: f64) -> [f64; 4] {
    let n = norm(axis).max(1e-30);
    let a = [axis[0] / n, axis[1] / n, axis[2] / n];
    let s = (ang * 0.5).sin();
    [a[0] * s, a[1] * s, a[2] * s, (ang * 0.5).cos()]
}

fn f32q(q: [f64; 4]) -> [f32; 4] { [q[0] as f32, q[1] as f32, q[2] as f32, q[3] as f32] }

/// Achsen-aligniertes Box-SDF (exakt) für die reine-f64-Trihedral-Isolation.
fn box_sdf(p: [f64; 3], c: [f64; 3], h: [f64; 3]) -> f64 {
    let d = [(p[0] - c[0]).abs() - h[0], (p[1] - c[1]).abs() - h[1], (p[2] - c[2]).abs() - h[2]];
    let outside = norm([d[0].max(0.0), d[1].max(0.0), d[2].max(0.0)]);
    let inside = d[0].max(d[1]).max(d[2]).min(0.0);
    outside + inside
}

// ─── Der Reflex-Fillet-Korrekturbaustein (f64-Prototyp des Opcodes) ─────────

/// Ein Rand-Fillet an EINEM Segment der Kontaktrand-Kurve ∂P.
///
/// `concave = true` (Erosion): additiv (min), `g ← min(g, δ − dist)`.
/// `concave = false` (Dilation): subtraktiv (max), `g ← max(g, dist − δ)`.
///
/// `n_a`, `n_b` sind die AUSSENnormalen der zwei Wände, die an ∂P zusammen-
/// stoßen. Material-/Luft-Maske: der Term wirkt NUR, wo der Punkt auf der
/// korrekten Seite BEIDER Wände liegt — konkav: Materialseite beider
/// (`⟨p−q, n⟩ ≤ 0`); konvex: Luftseite beider (`⟨p−q, n⟩ ≥ 0`). Das ist die
/// zentrale Halbraum-Maske, die den Term von der submergierten Gegenseite
/// (wo die Prisma-Korrektur schon exakt ist) fernhält.
#[derive(Clone, Copy)]
struct RimFeature {
    /// Segment-Endpunkte (Kante) oder a==b (Ecke / trihedrale Reflexecke).
    a: [f64; 3],
    b: [f64; 3],
    n_a: [f64; 3],
    n_b: [f64; 3],
    /// Optionale dritte Wand (trihedrale Reflexecke, sonst n_c = 0).
    n_c: [f64; 3],
    delta: f64,
    trust: f64,
    concave: bool,
}

impl RimFeature {
    fn closest(&self, p: [f64; 3]) -> (f64, [f64; 3]) {
        let (de, q, _) = self.closest_t(p);
        (de, q)
    }

    /// Wie [`closest`], liefert zusätzlich den (ungeklemmten) Projektions-
    /// parameter `t` für die End-Cap-Maske.
    fn closest_t(&self, p: [f64; 3]) -> (f64, [f64; 3], f64) {
        let d = sub(self.b, self.a);
        let len2 = dot(d, d);
        if len2 < 1e-24 {
            return (norm(sub(p, self.a)), self.a, 0.0);
        }
        let t_raw = dot(sub(p, self.a), d) / len2;
        let t = t_raw.clamp(0.0, 1.0);
        let q = add(self.a, scl(d, t));
        (norm(sub(p, q)), q, t_raw)
    }

    /// Liefert den Korrektur-Feldwert, wenn der Punkt in Maske UND Trust liegt,
    /// sonst `None` (Term inaktiv). Konkav: `δ − dist`; konvex: `dist − δ`.
    ///
    /// Drei Masken-Bestandteile:
    /// * **Wand-Halbräume** (n_a, n_b, ggf. n_c): Material-/Luftseite.
    /// * **End-Caps** senkrecht zur Kante: der Fillet endet AN den ∂P-Ecken —
    ///   jenseits der Segment-Enden existiert die Reflexkante nicht mehr, und
    ///   ein endpunkt-geklemmter Kugel-Fillet würde über ∂P hinaus überfüllen
    ///   (empirisch = δ). Nur wo der Lotfußpunkt IM Segment liegt (t ∈ [0,1]),
    ///   ist −dist(Kante) = f_U (Doc §1). Die ∂P-Ecke selbst rundet die
    ///   Nachbarkante, deren Lotfuß dort im Inneren liegt.
    /// * **Trust** K: jenseits ist die Kante nicht mehr die nächste ∂U-Fläche.
    fn value(&self, p: [f64; 3]) -> Option<f64> {
        let (de, q, t) = self.closest_t(p);
        if de > self.trust {
            return None;
        }
        // End-Cap: Lotfuß muss (bis auf Rundungs-ε) im Segment liegen.
        let seg = norm(sub(self.b, self.a));
        if seg > 1e-9 {
            let cap_eps = 1e-6;
            if t < -cap_eps || t > 1.0 + cap_eps {
                return None;
            }
        }
        let dq = sub(p, q);
        let eps = 1e-9;
        let side = |n: [f64; 3]| -> bool {
            if norm(n) < 0.5 {
                return true; // ungenutzte dritte Wand
            }
            if self.concave { dot(dq, n) <= eps } else { dot(dq, n) >= -eps }
        };
        if !(side(self.n_a) && side(self.n_b) && side(self.n_c)) {
            return None;
        }
        Some(if self.concave { self.delta - de } else { de - self.delta })
    }
}

/// Wendet den Satz Rand-Fillets auf den naiven (Prisma-)Wert `g` an:
/// NEAREST-Feature-Komposition. Konkav: `min(g, δ − dist_nearest)` (die
/// nächste Reflexkante bestimmt den Radius; siehe Doc §2 — NICHT sequenzielles
/// min über alle Kanten, das nähme fälschlich die FERNSTE). Konvex dual mit max.
fn apply_rims(g: f64, p: [f64; 3], rims: &[RimFeature]) -> f64 {
    // Nächstes maskiertes Feature bestimmen (kleinste Distanz).
    let mut best: Option<(f64, f64)> = None; // (dist, corr_value)
    for r in rims {
        if let Some(v) = r.value(p) {
            let (de, _) = r.closest(p);
            match best {
                Some((bd, _)) if bd <= de => {}
                _ => best = Some((de, v)),
            }
        }
    }
    match best {
        Some((_, v)) => {
            // Konkav: v = δ − dist (additiv, min). Konvex: v = dist − δ (max).
            // Das Vorzeichen steckt in v; wir erkennen es an r.concave über das
            // erste Feature (alle im Satz teilen es).
            if rims[0].concave { g.min(v) } else { g.max(v) }
        }
        None => g,
    }
}

// ─── Feld-Wrapper ──────────────────────────────────────────────────────────

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

// ─── Kernel-Prisma-Feld bauen (Regen-Pfad-Mirror) ──────────────────────────

fn id(n: u32) -> FeatureId { FeatureId::from_u32(n) }

fn compile_prism_field(node: &CsgNode) -> (BoundedCsg, CapabilityReport) {
    let s3 = node.clone().expand_shells().push_down_offsets().fold_offset_leaves();
    let mut report = CapabilityReport::new();
    s3.collect_offset_capability(&mut report);
    let mut field =
        synthesize_offset_channels_reported(BoundedCsg::from_csg(&s3), CHANNEL_TOL_RENDER, &mut report);
    prune_dead_channels(&mut field, CHANNEL_TOL_RENDER);
    (field, report)
}

// ─── Szenen-Geometrie ──────────────────────────────────────────────────────

struct Scene {
    name: &'static str,
    node: CsgNode,
    base: CsgNode,
    delta: f32, // signiert (< 0 Erosion, > 0 Dilation)
    rims: Vec<RimFeature>,
}

/// Box-Stack he_a ⊃ he_b (B auf +x-Fläche von A, koplanarer submergierter
/// Kontakt), Erosion −|δ|. Optionale globale Rotation `q`.
fn scene_box_stack(delta_abs: f64, q: Option<[f64; 4]>) -> Scene {
    let he_a = [10.0, 10.0, 10.0];
    let he_b = [8.0, 7.0, 6.0];
    // A @ origin, B-Zentrum bei x = he_a.x + he_b.x → −x-Fläche von B bei
    // x = he_a.x (koplanar mit A's +x-Fläche).
    let bx = he_a[0] + he_b[0];
    let a_leaf = CsgNode::Leaf { id: id(1), primitive: Primitive::Box { center: [0.0; 3], half_extents: [he_a[0] as f32, he_a[1] as f32, he_a[2] as f32] } };
    let b_leaf = CsgNode::Transform(
        Transform { translate: [bx as f32, 0.0, 0.0], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0; 3] },
        Box::new(CsgNode::Leaf { id: id(2), primitive: Primitive::Box { center: [0.0; 3], half_extents: [he_b[0] as f32, he_b[1] as f32, he_b[2] as f32] } }),
    );
    let mut base = CsgNode::Union(Box::new(a_leaf), Box::new(b_leaf));
    let qf = q.unwrap_or([0.0, 0.0, 0.0, 1.0]);
    if q.is_some() {
        base = CsgNode::Transform(Transform { translate: [0.0; 3], rotation: f32q(qf), scale: [1.0; 3] }, Box::new(base));
    }
    let node = CsgNode::Offset(Box::new(base.clone()), -(delta_abs as f32));

    // Kontaktrechteck P auf Π (x = he_a.x): y ∈ [−he_b.y, he_b.y], z ∈ [−he_b.z, he_b.z].
    let px = he_a[0];
    let (py, pz) = (he_b[1], he_b[2]);
    let trust = 3.0 * delta_abs;
    // Vier Reflexkanten von ∂P: A-Aussennormale = +x, B-Wandaussennormale je Kante.
    let raw: [([f64; 3], [f64; 3], [f64; 3], [f64; 3]); 4] = [
        ([px, py, -pz], [px, py, pz], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),   // y = +py
        ([px, -py, -pz], [px, -py, pz], [1.0, 0.0, 0.0], [0.0, -1.0, 0.0]), // y = −py
        ([px, -py, pz], [px, py, pz], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),   // z = +pz
        ([px, -py, -pz], [px, py, -pz], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]), // z = −pz
    ];
    let map_p = |p: [f64; 3]| if q.is_some() { qrot(qf, p) } else { p };
    let map_n = |n: [f64; 3]| if q.is_some() { qrot(qf, n) } else { n };
    let rims: Vec<RimFeature> = raw
        .iter()
        .map(|&(a, b, na, nb)| RimFeature {
            a: map_p(a), b: map_p(b), n_a: map_n(na), n_b: map_n(nb), n_c: [0.0; 3],
            delta: delta_abs, trust, concave: true,
        })
        .collect();
    Scene { name: if q.is_some() { "box_stack_rot" } else { "box_stack" }, node, base, delta: -(delta_abs as f32), rims }
}

/// Bündig ausgeschnittene Tasche (Difference A−B, B's +x-Fläche koplanar mit
/// A's +x-Fläche), Dilation +|δ|. Der Taschenmund ∂P trägt KONVEXE Keil-Kanten.
fn scene_flush_pocket(delta_abs: f64) -> Scene {
    let ha = [16.0f64, 16.0, 16.0];
    // Tasche: schmaler in y,z, endliche Tiefe (Sackloch).
    let (pdx, py, pz) = (10.0f64, 7.0, 6.0);
    let a = CsgNode::Leaf { id: id(1), primitive: Primitive::Box { center: [0.0; 3], half_extents: [ha[0] as f32, ha[1] as f32, ha[2] as f32] } };
    let bcx = ha[0] - pdx; // +x-Fläche von B bündig bei x = ha.x
    let b = CsgNode::Transform(
        Transform { translate: [bcx as f32, 0.0, 0.0], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0; 3] },
        Box::new(CsgNode::Leaf { id: id(2), primitive: Primitive::Box { center: [0.0; 3], half_extents: [pdx as f32, py as f32, pz as f32] } }),
    );
    let base = CsgNode::Difference(Box::new(a), Box::new(b));
    let node = CsgNode::Offset(Box::new(base.clone()), delta_abs as f32);

    // Taschenmund ∂P auf x = ha.x: y ∈ [−py, py], z ∈ [−pz, pz]. KONVEXE Lippe:
    // Solid ist außen {x ≤ ha.x} und außerhalb der Tasche. An der +y-Kante ist
    // Aussennormale von A's Deckfläche +x und der Taschenwand +y (Solid: y ≥ py).
    let px = ha[0];
    let trust = 3.0 * delta_abs;
    let raw: [([f64; 3], [f64; 3], [f64; 3], [f64; 3]); 4] = [
        ([px, py, -pz], [px, py, pz], [1.0, 0.0, 0.0], [0.0, -1.0, 0.0]),   // y = +py Lippe (Luft: y<py)
        ([px, -py, -pz], [px, -py, pz], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),  // y = −py
        ([px, -py, pz], [px, py, pz], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]),   // z = +pz
        ([px, -py, -pz], [px, py, -pz], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),  // z = −pz
    ];
    let rims: Vec<RimFeature> = raw
        .iter()
        .map(|&(a, b, na, nb)| RimFeature {
            a, b, n_a: na, n_b: nb, n_c: [0.0; 3], delta: delta_abs, trust, concave: false,
        })
        .collect();
    Scene { name: "flush_pocket", node, base, delta: delta_abs as f32, rims }
}

// ─── Messung (Nullmengen-Abweichung, Doppel-GT wie offset_fuzz) ────────────

struct Zs { p99: f64, max: f64, mean: f64, n: usize }

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
    // Optional: nur Oberflächenpunkte NAHE ∂P (Distanz zur nächsten Reflexkante
    // < 1.5·|δ|), um den Randfehler isoliert zu messen.
    let da = delta_signed.abs();
    let near = |p: [f64; 3]| -> bool {
        rims.iter().map(|r| r.closest(p).0).fold(f64::INFINITY, f64::min) < 1.5 * da
    };
    let pool: Vec<[f64; 3]> = if near_rim_only {
        surf.iter().copied().filter(|&p| near(p)).collect()
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

/// Duale-Sicherheits-Probe: die Rim-Korrektur darf NIRGENDS Material über die
/// wahre Erosion/Dilation hinaus hinzufügen/entfernen. Wir prüfen auf einem
/// Gitter, dass das korrigierte Vorzeichen nur dort von „außen" auf „innen"
/// (konkav) bzw. „innen" auf „außen" (konvex) kippt, wo die wahre Offset-GT es
/// ebenfalls tut (Toleranz `tol`). Rückgabe: maximale Über-Fill-Distanz.
fn dual_safety_overfill(
    corrected: &dyn Field,
    base_gt: &GroundTruth,
    lo: [f64; 3], hi: [f64; 3],
    delta_signed: f64,
    grid: usize,
) -> f64 {
    let da = delta_signed.abs();
    let concave = delta_signed < 0.0;
    let mut worst = 0.0f64;
    let step = [(hi[0] - lo[0]) / grid as f64, (hi[1] - lo[1]) / grid as f64, (hi[2] - lo[2]) / grid as f64];
    let pts: Vec<[f64; 3]> = (0..grid).flat_map(|i| (0..grid).flat_map(move |j| (0..grid).map(move |k| (i, j, k)))).map(|(i, j, k)| {
        [lo[0] + (i as f64 + 0.5) * step[0], lo[1] + (j as f64 + 0.5) * step[1], lo[2] + (k as f64 + 0.5) * step[2]]
    }).collect();
    let over: f64 = pts.par_iter().map(|&p| {
        let g = corrected.eval(p);
        // Wahre Offset-Feld: signierte Distanz zum Grundkörper − Offset.
        let d_true = base_gt.distance(p) - delta_signed; // < 0 ⇒ innen (erodiert/dilatiert)
        // Overfill: korrigiert sagt „innen" (g<0), aber wahr sagt „außen" (d_true>0).
        if concave {
            if g < 0.0 && d_true > 0.0 { d_true } else { 0.0 }
        } else {
            // Dilation: korrigiert sagt „außen" (g>0) wo wahr „innen" (d_true<0)?
            // Overfill (zu viel entfernt) = g>0 & d_true<0.
            if g > 0.0 && d_true < 0.0 { -d_true } else { 0.0 }
        }
    }).reduce(|| 0.0f64, f64::max);
    worst = worst.max(over);
    let _ = da;
    worst
}

// ─── Der Mess-Lauf ─────────────────────────────────────────────────────────

fn run_scene(s: &Scene, grid: usize, probe: usize) -> (Option<Zs>, Option<Zs>, Option<Zs>, Option<Zs>, f64, String) {
    let (field, report) = compile_prism_field(&s.node);
    let da = s.delta.abs() as f64;

    // Basis-GT (Grundkörper) für d_base.
    let base_field = CsgField::visual(&s.base);
    let baabb = s.base.aabb();
    let blo = [baabb.min[0] as f64, baabb.min[1] as f64, baabb.min[2] as f64];
    let bhi = [baabb.max[0] as f64, baabb.max[1] as f64, baabb.max[2] as f64];
    let base_gt = GroundTruth::build(&base_field, blo, bhi, &GtOptions { grid, ..GtOptions::default() });

    // Feld-AABB (aus dem Kernel-Feld, gepolstert).
    let fa = field.aabb();
    let flo = [fa.min[0] as f64 - 1.0, fa.min[1] as f64 - 1.0, fa.min[2] as f64 - 1.0];
    let fhi = [fa.max[0] as f64 + 1.0, fa.max[1] as f64 + 1.0, fa.max[2] as f64 + 1.0];

    let prism = BoundedField(&field);
    let full = RimField { base: &field, rims: s.rims.clone() };

    // Vorher/Nachher: gesamte Oberfläche UND ∂P-nah.
    let z_prism_all = measure(&prism, &base_gt, flo, fhi, s.delta as f64, &s.rims, grid, probe, 2, false);
    let z_full_all = measure(&full, &base_gt, flo, fhi, s.delta as f64, &s.rims, grid, probe, 3, false);
    let z_prism_rim = measure(&prism, &base_gt, flo, fhi, s.delta as f64, &s.rims, grid, probe, 2, true);
    let z_full_rim = measure(&full, &base_gt, flo, fhi, s.delta as f64, &s.rims, grid, probe, 3, true);

    let overfill = dual_safety_overfill(&full, &base_gt, flo, fhi, s.delta as f64, 40);

    let status = format!("{:?} n_rims={}", report.status(), s.rims.len());
    let _ = da;
    (z_prism_all, z_full_all, z_prism_rim, z_full_rim, overfill, status)
}

/// Reine-f64-Isolation der TRIHEDRALEN Reflexecke (√3-Ecke). Drei orthogonale
/// Halbraum-Boxen bilden U = {x≤0} ∪ {y≤0} ∪ {z≤0}; Luft ist der Oktant
/// {x>0,y>0,z>0}, die Reflexecke sitzt im Ursprung. Unter Erosion −δ rundet die
/// wahre Distanz zu einer Kugel-Oktanten (f_U = −dist(Ursprung) in der
/// Materialschneise); das naive `min(x,y,z)+δ` legt die scharfe Chebyshev-Ecke
/// bei (−δ,−δ,−δ) → Fehler δ·(√3−1) ≈ 0.73·δ. Der Vertex-Fillet
/// `min(g, δ − dist(Ecke))`, maskiert auf alle DREI Wände, plus die drei
/// Reflexkanten-Fillets tilgen ihn. KEIN Kernel (transversale Nähte würde der
/// Kernel selbst channeln — hier isolieren wir das Vertex-Primitiv analytisch).
fn measure_trihedral_vertex(delta_abs: f64, grid: usize, probe: usize) -> (f64, f64, f64, f64) {
    let l = 40.0f64;
    let boxes = [
        ([-l, 0.0, 0.0], [l, l, l]), // {x ≤ 0}
        ([0.0, -l, 0.0], [l, l, l]), // {y ≤ 0}
        ([0.0, 0.0, -l], [l, l, l]), // {z ≤ 0}
    ];
    let naive_base = move |p: [f64; 3]| boxes.iter().map(|&(c, h)| box_sdf(p, c, h)).fold(f64::INFINITY, f64::min);
    let base = FnField(naive_base);
    // Reflex-Features: drei Kanten (entlang +x, +y, +z ab Ursprung) + Vertex.
    let trust = 3.0 * delta_abs;
    let e = 40.0f64;
    let rims = vec![
        // Kante entlang +z: Wände +x (box_x), +y (box_y).
        RimFeature { a: [0.0, 0.0, 0.0], b: [0.0, 0.0, e], n_a: [1.0, 0.0, 0.0], n_b: [0.0, 1.0, 0.0], n_c: [0.0; 3], delta: delta_abs, trust, concave: true },
        // Kante entlang +x: Wände +y, +z.
        RimFeature { a: [0.0, 0.0, 0.0], b: [e, 0.0, 0.0], n_a: [0.0, 1.0, 0.0], n_b: [0.0, 0.0, 1.0], n_c: [0.0; 3], delta: delta_abs, trust, concave: true },
        // Kante entlang +y: Wände +x, +z.
        RimFeature { a: [0.0, 0.0, 0.0], b: [0.0, e, 0.0], n_a: [1.0, 0.0, 0.0], n_b: [0.0, 0.0, 1.0], n_c: [0.0; 3], delta: delta_abs, trust, concave: true },
        // Trihedrale Reflexecke: Punkt-Fillet, maskiert auf ALLE drei Wände.
        RimFeature { a: [0.0, 0.0, 0.0], b: [0.0, 0.0, 0.0], n_a: [1.0, 0.0, 0.0], n_b: [0.0, 1.0, 0.0], n_c: [0.0, 0.0, 1.0], delta: delta_abs, trust, concave: true },
    ];
    // Messbereich um den Ursprung (die Ecke).
    let lo = [-2.5 * delta_abs - 1.0, -2.5 * delta_abs - 1.0, -2.5 * delta_abs - 1.0];
    let hi = [2.5 * delta_abs + 1.0, 2.5 * delta_abs + 1.0, 2.5 * delta_abs + 1.0];
    let base_gt = GroundTruth::build(&base, [-l, -l, -l], [l, l, l], &GtOptions { grid, ..GtOptions::default() });

    let d_signed = -delta_abs;
    let naive_f = FnField(move |p| naive_base(p) - d_signed);
    let full_f = FnField(move |p| apply_rims(naive_base(p) - d_signed, p, &rims));

    // Nur ecken-nah messen (∂P-nah = nahe dem Ursprung).
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

fn fmt(z: &Option<Zs>) -> String {
    match z {
        Some(z) => format!("p99={:.4} max={:.4} mean={:.4} n={}", z.p99, z.max, z.mean, z.n),
        None => "—".to_string(),
    }
}

#[test]
fn f396_rim_prototype_report() {
    let grid = 64;
    let probe = 400;
    eprintln!("=== F396 Rand-Fillet-Prototyp (grid={grid}, probe={probe}) ===");
    eprintln!("Herleitung: Kante δ·(√2−1)={:.4}·δ, Ecke δ·(√3−1)={:.4}·δ",
        std::f64::consts::SQRT_2 - 1.0, 3.0f64.sqrt() - 1.0);

    struct Case { build: Box<dyn Fn(f64) -> Scene>, deltas: Vec<f64> }
    let cases = vec![
        Case { build: Box::new(|d| scene_box_stack(d, None)), deltas: vec![0.5, 1.0, 2.0] },
        Case { build: Box::new(|d| scene_box_stack(d, Some(quat_axis_angle([0.3, 1.0, 0.5], 0.7)))), deltas: vec![1.0, 2.0] },
        Case { build: Box::new(|d| scene_flush_pocket(d)), deltas: vec![1.0, 2.0, 4.0] },
    ];

    let mut any_improved = false;
    for case in &cases {
        for &d in &case.deltas {
            let s = (case.build)(d);
            let (pa, fa, pr, fr, over, status) = run_scene(&s, grid, probe);
            eprintln!("\n[{:>18} δ={:+.1}] {status}", s.name, s.delta);
            eprintln!("  GESAMT   prism: {}", fmt(&pa));
            eprintln!("           full:  {}", fmt(&fa));
            eprintln!("  ∂P-NAH   prism: {}", fmt(&pr));
            eprintln!("           full:  {}", fmt(&fr));
            eprintln!("  ∂P-nah prism/δ: {:.3}   full/δ: {:.3}   overfill={:.4} mm",
                pr.as_ref().map(|z| z.p99 / d).unwrap_or(0.0),
                fr.as_ref().map(|z| z.p99 / d).unwrap_or(0.0),
                over);
            if let (Some(p), Some(f)) = (&pr, &fr) {
                if f.p99 < p.p99 * 0.6 { any_improved = true; }
            }
            // Overfill-Schranke: die additiv-beschränkte Korrektur darf die wahre
            // Erosion/Dilation nicht überschreiten (bis auf GT-/f32-Rauschen).
            assert!(over < 0.15 * d + 0.05,
                "{} δ={}: Rim-Korrektur überfüllt ({:.4} mm > Schranke)", s.name, s.delta, over);
        }
    }
    // ─ Trihedrale √3-Reflexecke (reine-f64-Isolation des Vertex-Primitivs) ─
    eprintln!("\n=== Trihedrale Reflexecke (√3), reine f64, Vertex-Fillet ===");
    for &d in &[1.0f64, 2.0] {
        let (np99, nmax, fp99, over) = measure_trihedral_vertex(d, grid, probe);
        eprintln!(
            "[ trihedral_vertex δ={:+.1}] naiv: p99={:.4} max={:.4} ({:.3}·δ) → full: p99={:.4} ({:.3}·δ)  overfill={:.4}",
            -d, np99, nmax, nmax / d, fp99, fp99 / d, over
        );
        assert!(fp99 < 0.2 * d, "trihedral δ={d}: Vertex-Fillet erreicht Zielmarke nicht (p99 {fp99:.4})");
        assert!(over < 0.15 * d + 0.05, "trihedral δ={d}: Vertex-Fillet überfüllt ({over:.4})");
    }

    assert!(any_improved, "kein Szenario zeigte eine Rand-Fehler-Reduktion");
}
