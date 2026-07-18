//! offset_tree Phase B (#384): Kanal-Korrekturkörper an den **dualen**
//! Offset-Stopp-Fällen des Pushdowns (docs/OFFSET-TREE-PLAN.md §6).
//!
//! Nach `push_down_offsets` + `fold_offset_leaves` bleiben Offsets genau an
//! den Knoten stehen, wo die Minkowski-Algebra NICHT exakt distribuiert:
//!
//! * **+δ (Dilation) über Intersection/Difference**: `max(f_i) − δ` trägt
//!   einen konvexen Überschuss-Keil entlang jeder Schnittkante (gemessen
//!   bis 0.414·δ, Spike iter3 Teil B). Korrektur: Keil **subtrahieren**.
//! * **−δ (Erosion) über Union**: `min(f_i) + |δ|` lässt den konkaven
//!   Sliver fehlenden Materials entlang jeder Naht-Kante aus (Tiefe
//!   δ·(1/sin(α/2) − 1), B0-Messreihe). Korrektur: Sliver **addieren**.
//!
//! Beide Korrekturen sind dieselbe Konstruktion in Komplement-Dualität:
//! die korrekt getrimmte Rossignac–Requicha-Kanalfläche in Feldform,
//! **Variante C2** (Normalkegel-Gating am nächsten Kurvenpunkt — das
//! wörtliche §9-Keil-Rezept C1 ist bewiesen falsch, LOG.md Gotcha 1):
//!
//! ```text
//! q*        = nächster Punkt der Kanten-Kurve zu x (Arc-Spline)
//! m̂_A, m̂_B  = duale Kegelvektoren  m_A ∝ n_A − (n_A·n_B)·n_B  (auf der Kurve)
//! w(x)      = min( ⟨x−q*, m̂_A⟩, ⟨x−q*, m̂_B⟩, |x−q*| − δ, K − |x−q*|, gate )
//! konvex:   f = max(d, w)          konkav:  f = min(d, −w)   (m̂ negiert gespeichert)
//! ```
//!
//! Die zwei zusätzlichen min-Terme gegenüber dem Spike sind
//! **Nichtkonvexitäts-Wächter** (der C2-Exaktheitsbeweis gilt nur für
//! konvexe Körper) und ändern die Nullstellenmenge beweisbar nicht:
//!
//! * `K − de` (Trust-Radius): der Keil wirkt nur bis zum Kanal-Ausmaß
//!   `K ≈ c·δ` von der Kurve; jenseits ist er strukturell abgeschaltet —
//!   ohne das würde der Normalkegel einer Kante quer über einen Luftspalt
//!   in gegenüberliegende Körperteile "lecken" (Loch/Phantom). Der Term
//!   ist stetig (er nutzt `de` selbst, nicht die Bounding-Sphere); die
//!   Bounding-Sphere liefert nur den **exakten** Skip
//!   (`K − de_lower ≤ d` ⇒ Ergebnis == d, reine Kostenoptimierung).
//! * `gate = d + c·δ` (konvex) bzw. `c·δ − d` (konkav): kappt die Wirkung
//!   strikt auf die Umgebung der naiven Oberfläche; `c = 1/cos(γ_max/2)`
//!   bzw. `1/sin(α_min/2)` ist gerade groß genug, dass die legitime
//!   Korrektur (Tiefe (c−1)·δ) nie gekappt wird.
//!
//! Restrisiko (Plan §9, Phase-C-Witness): Fremdgeometrie NÄHER als K an
//! der Kurve im Kegelgebiet — dort gibt es keine Exaktheitsgarantie.
//!
//! ## Synthese
//!
//! Läuft auf [`BoundedCsg`] (Transform-Ketten bereits in Welt-Koordinaten
//! gefaltet) in den Mesh-/SDF-Baupfaden (`body.rs::build_bounded_*_rounded`),
//! NICHT im Weave-Baupfad — der Weave webt in Phase B weiter den naiven
//! Körper (Weave-Refresh des expandierten Körpers ist Phase C, Plan §7).
//!
//! Kantenquelle ist die **Seed-Maschinerie des parametrischen Weaves**
//! (Branch-Enumeration + analytische SSI-Seeds) plus ein eigener
//! Prädiktor-Korrektor-Tracer auf den **Kind-Feldern** des Stopp-Knotens
//! (`sdf_dual::sdf_gradient`): die Naht ist die gemeinsame Nullmenge
//! zweier Member-Felder, ihre Normalen sind die analytischen
//! Feld-Gradienten AUF der Kurve (1D-Ökonomie, R&R §3.1). Das ist
//! synchron zur Regen-Zeit verfügbar — kein Warten auf den
//! Async-Weaver, keine Zweistufigkeit (docs/ARCHITECTURE.md).
//!
//! **Verweigerungs-Doktrin** wie in `offset_fold`: wo Seeds fehlen
//! (SSI-unsupported: Torus/Gyroid/Sweeps ohne Halfspace-Karte), Winkel
//! degeneriert spitz sind oder Budgets reißen, bleibt `f − δ` stehen —
//! nie approximieren, nie raten. Jede synthetisierte Kanal-Korrektur ist
//! strikt besser als der Fallback; eine ausgelassene ist genau der
//! heutige Zustand.

use serde::Serialize;

use crate::bvh::BoundedCsg;
use crate::csg::{Aabb, EvalMode};

// ─── Daten ───────────────────────────────────────────────────────────────────

/// Ein Baustein der Kanal-Spine: Kreisbogen im Raum (PCC / Arc-Spline,
/// R&R §3.1) oder — Degenerationsfall kollinearer Stützpunkte — ein
/// Liniensegment (`sweep < 0`).
///
/// Bogen: Punkt(φ) = `c + r·(u·cosφ + v·sinφ)`, φ ∈ [0, sweep].
/// Segment: Punkt(t) = `c + u·(t·r)`, t ∈ [0, 1] (`r` = Länge, `v` = 0).
///
/// `m1*`/`m2*` sind die dualen Kegelvektoren an Start/Ende (Einheitslänge),
/// entlang des Bogens per nlerp interpoliert. Für konkave Kanäle sind sie
/// bereits **negiert** gespeichert, sodass die Auswertung vorzeichenfrei
/// dieselbe ist (Komplement-Dualität).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChannelArc {
    pub c: [f32; 3],
    pub u: [f32; 3],
    pub v: [f32; 3],
    pub r: f32,
    /// Bogen-Winkel in rad (`> 0`), oder `-1.0` für ein Liniensegment.
    pub sweep: f32,
    /// Bounding-Sphere des Bogens (Pruning des argmin-Scans).
    pub bc: [f32; 3],
    pub br: f32,
    pub m1s: [f32; 3],
    pub m2s: [f32; 3],
    pub m1e: [f32; 3],
    pub m2e: [f32; 3],
}

/// Ein Kanal-Korrekturkörper: eine Naht-Kurve (Arc-Spline) samt
/// Gating-Daten für EINEN dualen Offset-Stopp.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OffsetChannel {
    pub arcs: Vec<ChannelArc>,
    /// |δ| des Stopps (> 0).
    pub delta: f32,
    /// `true`: −δ-an-Union-Stopp (Material addieren; m̂ negiert gespeichert).
    /// `false`: +δ-an-Intersection/Difference-Stopp (Keil subtrahieren).
    pub concave: bool,
    /// Trust-Radius K (Kanal-Ausmaß von der Kurve, > δ).
    pub trust: f32,
    /// Gate-Weite G = c·δ + Marge (siehe Modul-Doku).
    pub gate: f32,
    /// Bounding-Sphere der ganzen Kurve (exakter Fernfeld-Skip).
    pub bound_c: [f32; 3],
    pub bound_r: f32,
    /// Provenance-BranchId des Kanal-Terms (0x700+-Band, Plan §9).
    pub branch: u32,
}

// ─── f32-Vektor-Helfer (lokal, um keine Modul-Abhängigkeit zu ziehen) ────────

#[inline]
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
#[inline]
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
#[inline]
fn norm(a: [f32; 3]) -> f32 {
    dot(a, a).sqrt()
}

// ─── Auswertung (f32; CPU-Referenz — GPU/C++/fidget spiegeln das 1:1) ────────

/// Nächster Punkt eines [`ChannelArc`] zu `p`: (Distanz, Fußpunkt, t∈[0,1]).
#[inline]
pub fn arc_closest(arc: &ChannelArc, p: [f32; 3]) -> (f32, [f32; 3], f32) {
    if arc.sweep < 0.0 {
        // Liniensegment: c → c + u·r.
        let len = arc.r.max(1e-12);
        let t = (dot(sub(p, arc.c), arc.u) / len).clamp(0.0, 1.0);
        let q = [
            arc.c[0] + arc.u[0] * (t * len),
            arc.c[1] + arc.u[1] * (t * len),
            arc.c[2] + arc.u[2] * (t * len),
        ];
        return (norm(sub(p, q)), q, t);
    }
    let d = sub(p, arc.c);
    let x = dot(d, arc.u);
    let y = dot(d, arc.v);
    let rho = (x * x + y * y).sqrt();
    let arc_point = |phi: f32| -> [f32; 3] {
        let (s, c) = phi.sin_cos();
        [
            arc.c[0] + arc.r * (arc.u[0] * c + arc.v[0] * s),
            arc.c[1] + arc.r * (arc.u[1] * c + arc.v[1] * s),
            arc.c[2] + arc.r * (arc.u[2] * c + arc.v[2] * s),
        ]
    };
    if rho < 1e-9 {
        let q = arc_point(0.0);
        return (norm(sub(p, q)), q, 0.0);
    }
    let mut phi = y.atan2(x);
    if phi < 0.0 {
        phi += std::f32::consts::TAU;
    }
    if phi <= arc.sweep {
        let q = arc_point(phi);
        (norm(sub(p, q)), q, phi / arc.sweep)
    } else {
        let q0 = arc_point(0.0);
        let q1 = arc_point(arc.sweep);
        let d0 = norm(sub(p, q0));
        let d1 = norm(sub(p, q1));
        if d0 <= d1 {
            (d0, q0, 0.0)
        } else {
            (d1, q1, 1.0)
        }
    }
}

#[inline]
fn nlerp(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let v = [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ];
    let n = norm(v);
    if n > 1e-9 {
        [v[0] / n, v[1] / n, v[2] / n]
    } else {
        a
    }
}

/// Nächster Kurvenpunkt eines Kanals: `(de, q*, m̂1, m̂2)` — die vier
/// Gating-Daten der C2-Konstruktion. `None` wenn der Kanal leer ist.
/// Auch der Anker des Dual-Spiegels in `sdf_dual` (q*/m̂ dort als
/// Konstanten eingefroren — Enveloppen-Satz für die Distanz).
pub fn channel_nearest(p: [f32; 3], ch: &OffsetChannel) -> Option<(f32, [f32; 3], [f32; 3], [f32; 3])> {
    let mut best = f32::INFINITY;
    let mut best_q = p;
    let mut best_m1 = [0.0f32; 3];
    let mut best_m2 = [0.0f32; 3];
    for arc in &ch.arcs {
        if norm(sub(p, arc.bc)) - arc.br >= best {
            continue;
        }
        let (de, q, t) = arc_closest(arc, p);
        if de < best {
            best = de;
            best_q = q;
            best_m1 = nlerp(arc.m1s, arc.m1e, t);
            best_m2 = nlerp(arc.m2s, arc.m2e, t);
        }
    }
    if best.is_finite() {
        Some((best, best_q, best_m1, best_m2))
    } else {
        None
    }
}

/// Exakter Fernfeld-Skip über die Kurven-Bounding-Sphere: `de ≥ dbs` ⇒
/// w ≤ K − de ≤ K − dbs. Ist K − dbs ≤ d (konvex) bzw. dbs − K ≥ d
/// (konkav), kann der Kanal das Ergebnis nicht ändern (reine
/// Kostenoptimierung, feldwert-identisch — Parität aller Spiegel).
#[inline]
pub fn channel_skip(d: f32, p: [f32; 3], ch: &OffsetChannel) -> bool {
    let dbs = norm(sub(p, ch.bound_c)) - ch.bound_r;
    ch.trust - dbs <= if ch.concave { -d } else { d }
}

/// Wendet EINEN Kanal auf den naiven Stopp-Wert `d` an (Modul-Doku).
/// Die CPU-Referenz aller Spiegel (Opcode-Interpreter, WGSL, C++, fidget).
pub fn channel_apply(d: f32, p: [f32; 3], ch: &OffsetChannel) -> f32 {
    if channel_skip(d, p, ch) {
        return d;
    }
    let Some((de, q, m1, m2)) = channel_nearest(p, ch) else {
        return d;
    };
    let dq = sub(p, q);
    let s1 = dot(dq, m1);
    let s2 = dot(dq, m2);
    let gate = if ch.concave { ch.gate - d } else { ch.gate + d };
    let w = s1
        .min(s2)
        .min(de - ch.delta)
        .min(ch.trust - de)
        .min(gate);
    if ch.concave {
        d.min(-w)
    } else {
        d.max(w)
    }
}

/// Alle Kanäle eines Stopps nacheinander anwenden (Reihenfolge = Daten).
pub fn channels_apply(mut d: f32, p: [f32; 3], channels: &[OffsetChannel]) -> f32 {
    for ch in channels {
        d = channel_apply(d, p, ch);
    }
    d
}

/// Konservative AABB eines Kanal-Satzes: Kurven-Bounds + Wirkradius.
/// Nur der konkave Fall ADDIERT Material; der Zusatz liegt innerhalb
/// `trust` um die Kurve.
pub fn channels_aabb(channels: &[OffsetChannel]) -> Aabb {
    let mut bb = Aabb::EMPTY;
    for ch in channels {
        if !ch.concave {
            continue;
        }
        let r = ch.bound_r + ch.trust;
        bb = Aabb::union(
            bb,
            Aabb {
                min: [ch.bound_c[0] - r, ch.bound_c[1] - r, ch.bound_c[2] - r],
                max: [ch.bound_c[0] + r, ch.bound_c[1] + r, ch.bound_c[2] + r],
            },
        );
    }
    bb
}

// ─── Synthese ────────────────────────────────────────────────────────────────

/// Standard-Zieltoleranz der Kurvenapproximation in mm (Zero-Set-Klasse).
/// 0.005 mm liegt zwischen Render- (0.01) und Export-Klasse (0.001); die
/// Staffelung (Plan §9) ist Phase C.
pub const DEFAULT_CHANNEL_TOL: f32 = 0.005;

/// Budget: maximale Bögen je Stopp-Knoten (alle Kanäle zusammen). Ein
/// Kanal der 0.001-mm-Klasse braucht ~64 Bögen je Steinmetz-Loop; ein
/// Gehäuse-Shell mit Dutzenden Kanten bleibt unter dem Opcode-Budget
/// (27 Floats je Bogen, `SDF_MAX_OPCODES` = 16384).
const MAX_ARCS_PER_STOP: usize = 512;

/// Budget: maximale Trace-Punkte je Naht-Kurve.
const MAX_TRACE_POINTS: usize = 4096;

/// Tangentialitäts-Schwelle: unterhalb dieses Normalenwinkels (rad) ist
/// die Naht quasi-tangential — der vorhergesagte Fehler liegt unter jeder
/// Toleranz und die dualen Kegelvektoren degenerieren.
const MIN_NORMAL_ANGLE: f32 = 0.03;

/// Spitzwinkel-Gate: `cos(γ/2)` (konvex) bzw. `sin(α/2)` (konkav)
/// unterhalb dessen der Kanal verweigert wird (c-Faktor > 5).
const MIN_HALF_ANGLE_SIN: f32 = 0.2;

/// Ein Boolean-Member des Stopp-Knotens in Normalform: Feld =
/// `sign · subtree.sdf` (Intersection-Form: Material ⇔ alle ≤ 0;
/// Union-Form: Material ⇔ min ≤ 0).
struct Member<'a> {
    node: &'a BoundedCsg,
    sign: f32,
}

impl Member<'_> {
    fn value(&self, p: [f32; 3]) -> f32 {
        self.sign * self.node.sdf(p, EvalMode::Visual)
    }
    fn value_grad(&self, p: [f32; 3]) -> (f32, [f32; 3]) {
        let (v, g) = crate::sdf_dual::sdf_gradient(self.node, p);
        (
            self.sign * v,
            [self.sign * g[0], self.sign * g[1], self.sign * g[2]],
        )
    }
}

/// Flacht den Kind-Baum eines Stopps in Member-Normalform.
/// `union_form = true`: Union-Ketten flach (Vorzeichen +).
/// `union_form = false`: Intersection/Difference-Ketten flach; der
/// Subtrahend einer Difference geht mit Vorzeichen −, eine Union unter
/// negativem Vorzeichen flacht als mehrere negative Member
/// (De Morgan: ¬(b ∪ c) = ¬b ∩ ¬c).
fn flatten_members<'a>(node: &'a BoundedCsg, union_form: bool, sign: f32, out: &mut Vec<Member<'a>>) {
    match node {
        BoundedCsg::Union { left, right, .. } if union_form == (sign > 0.0) => {
            flatten_members(left, union_form, sign, out);
            flatten_members(right, union_form, sign, out);
        }
        BoundedCsg::Intersection { left, right, .. } if !union_form && sign > 0.0 => {
            flatten_members(left, union_form, sign, out);
            flatten_members(right, union_form, sign, out);
        }
        BoundedCsg::Difference { left, right, .. } if !union_form && sign > 0.0 => {
            flatten_members(left, union_form, sign, out);
            flatten_members(right, union_form, -1.0, out);
        }
        other => out.push(Member { node: other, sign }),
    }
}

/// Ein Trace-Punkt der Naht: Position + die zwei Einheits-Normalen der
/// Member-Felder (Konvention: n zeigt aus dem jeweiligen Constraint
/// heraus, d. h. ∇(sign·f)).
#[derive(Clone, Copy)]
struct SeamPoint {
    p: [f32; 3],
    n1: [f32; 3],
    n2: [f32; 3],
}

/// Newton-Projektion eines Punkts auf die gemeinsame Nullmenge zweier
/// Member-Felder. `None` bei Nicht-Konvergenz oder degenerierten
/// Gradienten (tangentiale Naht).
fn project_to_seam(a: &Member, b: &Member, start: [f32; 3], scale: f32) -> Option<SeamPoint> {
    let mut p = start;
    for _ in 0..24 {
        let (fa, ga) = a.value_grad(p);
        let (fb, gb) = b.value_grad(p);
        let na = norm(ga);
        let nb = norm(gb);
        // NaN-Wächter: exakt AUF einer Blattfläche kann der Dual-Gradient
        // eine sqrt(0)-Ableitung ziehen (∞/NaN) — Abbruch, der Aufrufer
        // versucht einen jitterten Start.
        if !(na.is_finite() && nb.is_finite() && fa.is_finite() && fb.is_finite()) {
            return None;
        }
        if na < 1e-6 || nb < 1e-6 {
            return None;
        }
        let ga = [ga[0] / na, ga[1] / na, ga[2] / na];
        let gb = [gb[0] / nb, gb[1] / nb, gb[2] / nb];
        let c = dot(ga, gb);
        let det = 1.0 - c * c;
        if det < 1e-6 {
            return None; // tangential
        }
        // Löse fa·? — Schritt in span(ga, gb): p ← p − (α·ga + β·gb)
        // mit [1 c; c 1]·[α β]ᵀ = [fa/|ga|… bereits normiert: fa, fb].
        let fa_n = fa / na;
        let fb_n = fb / nb;
        let alpha = (fa_n - c * fb_n) / det;
        let beta = (fb_n - c * fa_n) / det;
        p = [
            p[0] - alpha * ga[0] - beta * gb[0],
            p[1] - alpha * ga[1] - beta * gb[1],
            p[2] - alpha * ga[2] - beta * gb[2],
        ];
        if fa_n.abs().max(fb_n.abs()) < 1e-5 * scale.max(1.0) {
            let (_, ga) = a.value_grad(p);
            let (_, gb) = b.value_grad(p);
            let na = norm(ga);
            let nb = norm(gb);
            if na < 1e-6 || nb < 1e-6 {
                return None;
            }
            return Some(SeamPoint {
                p,
                n1: [ga[0] / na, ga[1] / na, ga[2] / na],
                n2: [gb[0] / nb, gb[1] / nb, gb[2] / nb],
            });
        }
    }
    None
}

/// Gate: liegt der Naht-Punkt auf dem Rand des Komposits? Alle ÜBRIGEN
/// Member müssen strikt auf der Material-Seite ihres Constraints liegen
/// (Intersection-Form: f_k < −ε; Union-Form: f_k > +ε — sonst ist die
/// Kante von einem dritten Körper verdeckt/verschluckt, Triple-Junction).
fn on_composite_boundary(
    members: &[Member],
    skip: (usize, usize),
    p: [f32; 3],
    union_form: bool,
    eps: f32,
) -> bool {
    for (k, m) in members.iter().enumerate() {
        if k == skip.0 || k == skip.1 {
            continue;
        }
        let v = m.value(p);
        if union_form {
            if v <= eps {
                return false;
            }
        } else if v >= -eps {
            return false;
        }
    }
    true
}

/// Prädiktor-Korrektor-Trace der Naht zwischen Member `i` und `j` ab
/// `seed`, in beide Richtungen, bis Loop-Schluss / Gate-Ende / Budget.
/// Liefert die geordnete Punktkette (bei Loop: geschlossen OHNE
/// dupliziertes Ende) und ob es ein geschlossener Loop ist.
fn trace_seam(
    members: &[Member],
    i: usize,
    j: usize,
    seed: SeamPoint,
    union_form: bool,
    step: f32,
    fit_tol: f32,
    gate_eps: f32,
) -> Option<(Vec<SeamPoint>, bool)> {
    let a = &members[i];
    let b = &members[j];
    let tangent = |sp: &SeamPoint| -> Option<[f32; 3]> {
        let t = [
            sp.n1[1] * sp.n2[2] - sp.n1[2] * sp.n2[1],
            sp.n1[2] * sp.n2[0] - sp.n1[0] * sp.n2[2],
            sp.n1[0] * sp.n2[1] - sp.n1[1] * sp.n2[0],
        ];
        let n = norm(t);
        if n < MIN_NORMAL_ANGLE {
            return None; // quasi-tangentiale Naht
        }
        Some([t[0] / n, t[1] / n, t[2] / n])
    };
    tangent(&seed)?;
    let scale = norm(sub(seed.p, [0.0; 3])).max(1.0);

    // Krümmungs-adaptive Schrittweite: die Sagitta zwischen zwei
    // Trace-Punkten (κ·h²/8) muss unter der halben Fit-Toleranz bleiben,
    // sonst prüft der Greedy-Fit die Kurve zwischen den Stützpunkten
    // nicht. κ wird aus der Tangenten-Drehung des letzten Schritts
    // geschätzt und der Schritt nachgeführt.
    let step_max = step;
    let step_min = (fit_tol * 4.0).min(step_max);
    let march = |dir_sign: f32| -> (Vec<SeamPoint>, bool) {
        let mut pts: Vec<SeamPoint> = Vec::new();
        let mut cur = seed;
        let mut prev_t: Option<[f32; 3]> = None;
        let mut h = step_min.max(step_max * 0.1);
        for _ in 0..MAX_TRACE_POINTS {
            let Some(mut t) = tangent(&cur) else { break };
            if let Some(pt) = prev_t {
                if dot(t, pt) < 0.0 {
                    t = [-t[0], -t[1], -t[2]];
                }
                // κ ≈ Δθ/h des letzten Schritts → Ziel-Sagitta fit_tol/2.
                let cosang = dot(t, pt).clamp(-1.0, 1.0);
                let theta = cosang.acos();
                if theta > 1e-6 {
                    let kappa = theta / h;
                    let ideal = (4.0 * fit_tol / kappa).sqrt();
                    h = ideal.clamp(step_min, step_max);
                } else {
                    h = (h * 2.0).min(step_max);
                }
            } else if dir_sign < 0.0 {
                t = [-t[0], -t[1], -t[2]];
            }
            let pred = [
                cur.p[0] + t[0] * h,
                cur.p[1] + t[1] * h,
                cur.p[2] + t[2] * h,
            ];
            let Some(next) = project_to_seam(a, b, pred, scale) else { break };
            // Abbruch: Schritt kollabiert (Ecke) oder springt (Ast-Wechsel).
            let moved = norm(sub(next.p, cur.p));
            if moved < h * 0.25 || moved > h * 2.5 {
                break;
            }
            if !on_composite_boundary(members, (i, j), next.p, union_form, gate_eps) {
                break;
            }
            // Loop-Schluss gegen den SEED (nur vorwärts).
            if dir_sign > 0.0 && pts.len() > 3 && norm(sub(next.p, seed.p)) < h * 0.75 {
                pts.push(next);
                return (pts, true);
            }
            pts.push(next);
            prev_t = Some(t);
            cur = next;
        }
        (pts, false)
    };

    let (fwd, closed) = march(1.0);
    if closed {
        let mut pts = vec![seed];
        pts.extend(fwd);
        return Some((pts, true));
    }
    let (bwd, _) = march(-1.0);
    let mut pts: Vec<SeamPoint> = bwd.into_iter().rev().collect();
    pts.push(seed);
    pts.extend(fwd);
    if pts.len() < 3 {
        return None;
    }
    Some((pts, false))
}

// ─── Arc-Spline-Fit (PCC) ───────────────────────────────────────────────────

/// Kreisbogen durch drei Punkte; `None` bei (Quasi-)Kollinearität.
fn arc_through(p0: [f32; 3], p1: [f32; 3], p2: [f32; 3]) -> Option<ChannelArc> {
    let a = sub(p1, p0);
    let b = sub(p2, p0);
    let n = [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ];
    let n2 = dot(n, n);
    let chord = norm(sub(p2, p0)).max(1e-9);
    // Kollinear (oder Radius > ~1e4·Chord): Segment-Fallback des Callers.
    if n2.sqrt() < 1e-4 * chord * chord {
        return None;
    }
    let bb2 = dot(b, b);
    let aa2 = dot(a, a);
    let cxn_a = [
        n[1] * a[2] - n[2] * a[1],
        n[2] * a[0] - n[0] * a[2],
        n[0] * a[1] - n[1] * a[0],
    ];
    let bxn = [
        b[1] * n[2] - b[2] * n[1],
        b[2] * n[0] - b[0] * n[2],
        b[0] * n[1] - b[1] * n[0],
    ];
    let inv = 1.0 / (2.0 * n2);
    let c = [
        p0[0] + (cxn_a[0] * bb2 + bxn[0] * aa2) * inv,
        p0[1] + (cxn_a[1] * bb2 + bxn[1] * aa2) * inv,
        p0[2] + (cxn_a[2] * bb2 + bxn[2] * aa2) * inv,
    ];
    let r = norm(sub(p0, c));
    if !r.is_finite() || r < 1e-9 {
        return None;
    }
    let mut w = {
        let nn = norm(n);
        [n[0] / nn, n[1] / nn, n[2] / nn]
    };
    let u = {
        let d = sub(p0, c);
        let dn = norm(d);
        [d[0] / dn, d[1] / dn, d[2] / dn]
    };
    let mut v = [
        w[1] * u[2] - w[2] * u[1],
        w[2] * u[0] - w[0] * u[2],
        w[0] * u[1] - w[1] * u[0],
    ];
    let ang = |q: [f32; 3], v_: [f32; 3]| -> f32 {
        let d = sub(q, c);
        let mut a = dot(d, v_).atan2(dot(d, u));
        if a < 0.0 {
            a += std::f32::consts::TAU;
        }
        a
    };
    let mut a1 = ang(p1, v);
    let mut a2 = ang(p2, v);
    if a1 > a2 {
        w = [-w[0], -w[1], -w[2]];
        v = [
            w[1] * u[2] - w[2] * u[1],
            w[2] * u[0] - w[0] * u[2],
            w[0] * u[1] - w[1] * u[0],
        ];
        a1 = ang(p1, v);
        a2 = ang(p2, v);
    }
    // Sweep-Kappe bei π: die 3-Punkt-Bounding-Sphere in `finish_arc` ist
    // nur für Halbbögen scharf; längere Bögen zerlegt der Fit ohnehin.
    if a1 > a2 + 1e-6 || a2 <= 1e-9 || a2 > std::f32::consts::PI {
        return None;
    }
    Some(finish_arc(ChannelArc {
        c,
        u,
        v,
        r,
        sweep: a2,
        bc: [0.0; 3],
        br: 0.0,
        m1s: [0.0; 3],
        m2s: [0.0; 3],
        m1e: [0.0; 3],
        m2e: [0.0; 3],
    }))
}

/// Segment-„Bogen" von `p0` nach `p1`.
fn segment_arc(p0: [f32; 3], p1: [f32; 3]) -> Option<ChannelArc> {
    let d = sub(p1, p0);
    let len = norm(d);
    if len < 1e-9 {
        return None;
    }
    Some(finish_arc(ChannelArc {
        c: p0,
        u: [d[0] / len, d[1] / len, d[2] / len],
        v: [0.0; 3],
        r: len,
        sweep: -1.0,
        bc: [0.0; 3],
        br: 0.0,
        m1s: [0.0; 3],
        m2s: [0.0; 3],
        m1e: [0.0; 3],
        m2e: [0.0; 3],
    }))
}

/// Bounding-Sphere eines Bogens nachtragen.
fn finish_arc(mut arc: ChannelArc) -> ChannelArc {
    let p0 = arc_point_at(&arc, 0.0);
    let p1 = arc_point_at(&arc, 0.5);
    let p2 = arc_point_at(&arc, 1.0);
    let bc = [
        (p0[0] + p1[0] + p2[0]) / 3.0,
        (p0[1] + p1[1] + p2[1]) / 3.0,
        (p0[2] + p1[2] + p2[2]) / 3.0,
    ];
    let br = norm(sub(p0, bc))
        .max(norm(sub(p1, bc)))
        .max(norm(sub(p2, bc)))
        + 1e-4;
    arc.bc = bc;
    arc.br = br;
    arc
}

/// Punkt bei Parameter t ∈ [0,1] (Bogen: φ = t·sweep; Segment linear).
fn arc_point_at(arc: &ChannelArc, t: f32) -> [f32; 3] {
    if arc.sweep < 0.0 {
        [
            arc.c[0] + arc.u[0] * (t * arc.r),
            arc.c[1] + arc.u[1] * (t * arc.r),
            arc.c[2] + arc.u[2] * (t * arc.r),
        ]
    } else {
        let phi = t * arc.sweep;
        let (s, c) = phi.sin_cos();
        [
            arc.c[0] + arc.r * (arc.u[0] * c + arc.v[0] * s),
            arc.c[1] + arc.r * (arc.u[1] * c + arc.v[1] * s),
            arc.c[2] + arc.r * (arc.u[2] * c + arc.v[2] * s),
        ]
    }
}

/// Greedy-PCC-Fit: wachse Runs von Trace-Punkten, solange der Bogen durch
/// (Start, Mitte, Ende) alle Zwischenpunkte innerhalb `fit_tol` trifft.
/// Liefert je Bogen die Indizes (start, end) in `pts`.
fn fit_arcs(pts: &[SeamPoint], closed: bool, fit_tol: f32) -> Vec<(usize, usize, ChannelArc)> {
    let n = pts.len();
    let mut out = Vec::new();
    let mut s = 0usize;
    let last = if closed { n } else { n - 1 };
    while s < last {
        let mut e = (s + 2).min(last);
        let mut best: Option<(usize, ChannelArc)> = None;
        loop {
            let pe = pts[e % n].p;
            let pm = pts[(s + (e - s) / 2) % n].p;
            let cand = arc_through(pts[s].p, pm, pe).or_else(|| segment_arc(pts[s].p, pe));
            let Some(cand) = cand else { break };
            let mut ok = true;
            for k in (s + 1)..e {
                let (d, _, _) = arc_closest(&cand, pts[k % n].p);
                if d > fit_tol {
                    ok = false;
                    break;
                }
            }
            if !ok {
                break;
            }
            best = Some((e, cand));
            if e >= last {
                break;
            }
            e = (e + ((e - s).max(1))).min(last); // exponentiell wachsen
        }
        match best {
            Some((e, arc)) => {
                out.push((s, e, arc));
                s = e;
            }
            None => {
                // Minimalstück: Segment über einen Schritt.
                if let Some(seg) = segment_arc(pts[s].p, pts[(s + 1) % n].p) {
                    out.push((s, s + 1, seg));
                }
                s += 1;
            }
        }
    }
    out
}

/// Duale Kegelvektoren aus den zwei Einheits-Normalen; `None` bei
/// Quasi-Tangentialität.
fn dual_cone(n1: [f32; 3], n2: [f32; 3], negate: bool) -> Option<([f32; 3], [f32; 3])> {
    let c = dot(n1, n2);
    let m1 = [n1[0] - c * n2[0], n1[1] - c * n2[1], n1[2] - c * n2[2]];
    let m2 = [n2[0] - c * n1[0], n2[1] - c * n1[1], n2[2] - c * n1[2]];
    let l1 = norm(m1);
    let l2 = norm(m2);
    if l1 < 1e-4 || l2 < 1e-4 {
        return None;
    }
    let s = if negate { -1.0 } else { 1.0 };
    Some((
        [s * m1[0] / l1, s * m1[1] / l1, s * m1[2] / l1],
        [s * m2[0] / l2, s * m2[1] / l2, s * m2[2] / l2],
    ))
}

/// Kontext eines Stopp-Knotens für die Synthese.
struct StopCtx {
    delta: f32,
    concave: bool,
    tol: f32,
}

/// Baut aus einer getracten Naht-Kurve einen [`OffsetChannel`].
/// `None` = verweigern (tangential, spitzwinklig, Budget).
fn build_channel(
    pts: &[SeamPoint],
    closed: bool,
    ctx: &StopCtx,
    branch: u32,
) -> Option<OffsetChannel> {
    // Winkel-Statistik: γ = Winkel zwischen den Normalen.
    let mut min_half = f32::INFINITY; // konkav: sin(α/2) = cos(γ/2)? s. u.
    let mut worst_c = 0.0f32;
    for sp in pts {
        let cg = dot(sp.n1, sp.n2).clamp(-1.0, 1.0);
        let gamma = cg.acos();
        if gamma < MIN_NORMAL_ANGLE {
            continue;
        }
        // Konvexer Keil: Überschuss-Faktor c₁ = 1/cos(γ/2).
        // Konkaver Sliver: α = π − γ, Faktor 1/sin(α/2) = 1/cos(γ/2).
        // (Beide Fälle teilen denselben Halbwinkel-Kosinus.)
        let ch = (gamma * 0.5).cos();
        min_half = min_half.min(ch);
        worst_c = worst_c.max(1.0 / ch.max(1e-3));
    }
    if !min_half.is_finite() {
        return None; // durchgehend tangential
    }
    if min_half < MIN_HALF_ANGLE_SIN {
        return None; // zu spitz — verweigern (f − δ bleibt)
    }
    // Prognostizierter Naht-Fehler ohne Kanal: δ·(c − 1). Liegt er unter
    // der Toleranz, lohnt kein Kanal (Pruning-Kriterium der B0-Messreihe).
    if ctx.delta * (worst_c - 1.0) < ctx.tol {
        return None;
    }

    let fit_tol = (ctx.tol * 0.5).max(1e-5);
    let fitted = fit_arcs(pts, closed, fit_tol);
    if fitted.is_empty() {
        return None;
    }

    let n = pts.len();
    let mut arcs = Vec::with_capacity(fitted.len());
    for (s, e, mut arc) in fitted {
        let sp_s = &pts[s % n];
        let sp_e = &pts[e % n];
        let (m1s, m2s) = dual_cone(sp_s.n1, sp_s.n2, ctx.concave)?;
        let (m1e, m2e) = dual_cone(sp_e.n1, sp_e.n2, ctx.concave)?;
        arc.m1s = m1s;
        arc.m2s = m2s;
        arc.m1e = m1e;
        arc.m2e = m2e;
        arcs.push(arc);
    }

    // Kurven-Bounding-Sphere.
    let mut bc = [0.0f64; 3];
    for sp in pts {
        bc[0] += sp.p[0] as f64;
        bc[1] += sp.p[1] as f64;
        bc[2] += sp.p[2] as f64;
    }
    let inv = 1.0 / pts.len() as f64;
    let bc = [
        (bc[0] * inv) as f32,
        (bc[1] * inv) as f32,
        (bc[2] * inv) as f32,
    ];
    let mut br = 0.0f32;
    for sp in pts {
        br = br.max(norm(sub(sp.p, bc)));
    }

    let c_factor = 1.0 / min_half;
    let gate = ctx.delta * c_factor + 4.0 * ctx.tol;
    let trust = ctx.delta * c_factor + 0.5 * ctx.delta + 8.0 * ctx.tol;
    Some(OffsetChannel {
        arcs,
        delta: ctx.delta,
        concave: ctx.concave,
        trust,
        gate,
        bound_c: bc,
        bound_r: br + 1e-3,
        branch,
    })
}

/// Synthetisiert die Kanal-Korrekturkörper für EINEN Stopp-Knoten
/// (`Offset { inner, delta }` mit dualem Boolean-Kind). Liefert die
/// Kanal-Liste oder leer (= vollständige Verweigerung, `f − δ` bleibt).
fn synthesize_stop(inner: &BoundedCsg, delta: f32, tol: f32) -> Vec<OffsetChannel> {
    let union_form = delta < 0.0;
    let mut members = Vec::new();
    flatten_members(inner, union_form, 1.0, &mut members);
    if members.len() < 2 {
        return Vec::new();
    }
    let ctx = StopCtx {
        delta: delta.abs(),
        concave: union_form,
        tol,
    };

    // Analytische Branch-Karten je Member (Seeds). Member ohne Karte
    // (Sweeps, Nicht-Halfspace-Primitive) liefern keine Seeds — deren
    // Nähte bleiben unkorrigiert (Verweigerung, nie falsch).
    let branch_maps: Vec<crate::brep::BranchSurfaceMap> = members
        .iter()
        .map(|m| crate::brep::weaving::branch_halfspace_map(m.node))
        .collect();
    let branch_lists: Vec<Vec<crate::brep::branch_pairs::Branch>> = members
        .iter()
        .map(|m| crate::brep::branch_pairs::enumerate_branches(m.node))
        .collect();

    let mut channels: Vec<OffsetChannel> = Vec::new();
    let mut total_arcs = 0usize;
    let mut claimed: Vec<[f32; 3]> = Vec::new(); // Punkte bereits getracter Kurven
    let mut channel_idx = 0u32;

    let gate_eps = (ctx.delta * 1e-3).max(1e-4);

    for i in 0..members.len() {
        for j in (i + 1)..members.len() {
            // Schrittweite aus der Überlappungs-Skala.
            let overlap = Aabb::intersect(members[i].node.aabb(), members[j].node.aabb());
            if overlap.is_empty() {
                continue;
            }
            let osz = overlap.size();
            let oscale = osz[0].max(osz[1]).max(osz[2]).max(ctx.delta);
            let step = (oscale * 0.02).clamp(ctx.tol.max(1e-4) * 4.0, oscale * 0.05);

            for ba in &branch_lists[i] {
                let Some((hs_a, xf_a)) = branch_maps[i].get(&ba.id.as_u32()) else {
                    continue;
                };
                for bb in &branch_lists[j] {
                    let Some((hs_b, xf_b)) = branch_maps[j].get(&bb.id.as_u32()) else {
                        continue;
                    };
                    let pair_box = Aabb::intersect(
                        Aabb::intersect(ba.aabb.expanded(1e-3), bb.aabb.expanded(1e-3)),
                        overlap.expanded(1e-3),
                    );
                    if pair_box.is_empty() {
                        continue;
                    }
                    let seeds = match crate::brep::ssi::ssi_seeds(hs_a, xf_a, hs_b, xf_b, &pair_box)
                    {
                        crate::brep::ssi::SsiResult::Components(c) => c,
                        crate::brep::ssi::SsiResult::Unsupported => continue,
                    };
                    for comp in seeds {
                        for seed in comp {
                            if total_arcs >= MAX_ARCS_PER_STOP {
                                return channels; // Budget: bisherige Kanäle behalten
                            }
                            // Schon von einer früheren Kurve abgedeckt?
                            if claimed.iter().any(|q| norm(sub(*q, seed)) < step * 1.5) {
                                continue;
                            }
                            // Seeds können EXAKT auf Blattflächen liegen (SSI
                            // liefert Oberflächenpunkte) — dort ziehen Dual-
                            // Gradienten sqrt(0)-NaNs. Deterministische Jitter
                            // um den Seed heilen das (Newton zieht zurück).
                            let jit = (oscale * 1e-4).max(1e-4);
                            let mut sp = None;
                            'jitter: for off in [
                                [0.0, 0.0, 0.0],
                                [jit, jit, jit],
                                [-jit, jit, -jit],
                                [jit, -jit, jit],
                                [-jit, -jit, -jit],
                            ] {
                                let start = [
                                    seed[0] + off[0],
                                    seed[1] + off[1],
                                    seed[2] + off[2],
                                ];
                                if let Some(s) =
                                    project_to_seam(&members[i], &members[j], start, oscale)
                                {
                                    sp = Some(s);
                                    break 'jitter;
                                }
                            }
                            let Some(sp) = sp else { continue };
                            if !on_composite_boundary(
                                &members,
                                (i, j),
                                sp.p,
                                union_form,
                                gate_eps,
                            ) {
                                continue;
                            }
                            if claimed.iter().any(|q| norm(sub(*q, sp.p)) < step * 1.5) {
                                continue;
                            }
                            let fit_tol = (ctx.tol * 0.5).max(1e-5);
                            let Some((pts, closed)) =
                                trace_seam(&members, i, j, sp, union_form, step, fit_tol, gate_eps)
                            else {
                                continue;
                            };
                            for p in &pts {
                                claimed.push(p.p);
                            }
                            // Provenance: geordnetes Eltern-Paar → Owner =
                            // kleinere Feature-Id, Sub aus dem 0x700+-Band
                            // (deterministisch in Synthese-Reihenfolge;
                            // Kollisionswächter wie offset_fold: reale Subs
                            // liegen strikt unter 0x700).
                            let (pa, pb) = if ba.id.as_u32() <= bb.id.as_u32() {
                                (ba.id, bb.id)
                            } else {
                                (bb.id, ba.id)
                            };
                            if pa.sub() >= 0x700 || pb.sub() >= 0x700 {
                                continue; // Kollision mit dem Band — verweigern
                            }
                            let sub = 0x700 + (channel_idx % 0xFD);
                            let branch = (pa.feature_id() << 12) | sub;
                            if let Some(ch) = build_channel(&pts, closed, &ctx, branch) {
                                total_arcs += ch.arcs.len();
                                if total_arcs > MAX_ARCS_PER_STOP {
                                    return channels;
                                }
                                channel_idx += 1;
                                channels.push(ch);
                            }
                        }
                    }
                }
            }
        }
    }
    channels
}

/// offset_tree Phase B: Baum-Pass über [`BoundedCsg`] — hängt an jeden
/// dualen Offset-Stopp die synthetisierten Kanal-Korrekturkörper
/// ([`BoundedCsg::ArcChannel`]). Läuft in den Mesh-/SDF-Baupfaden nach
/// `from_csg`; Weave-/Pick-Pfade bleiben in Phase B auf dem naiven Baum.
pub fn synthesize_offset_channels(node: BoundedCsg, tol: f32) -> BoundedCsg {
    match node {
        BoundedCsg::Offset { inner, delta, aabb } => {
            let inner = Box::new(synthesize_offset_channels(*inner, tol));
            let dual_stop = match (&*inner, delta) {
                (BoundedCsg::Union { .. }, d) if d < 0.0 => true,
                (BoundedCsg::Intersection { .. }, d) if d > 0.0 => true,
                (BoundedCsg::Difference { .. }, d) if d > 0.0 => true,
                _ => false,
            };
            if !dual_stop || delta == 0.0 || !delta.is_finite() {
                return BoundedCsg::Offset { inner, delta, aabb };
            }
            let channels = synthesize_stop(&inner, delta, tol);
            let stop = BoundedCsg::Offset { inner, delta, aabb };
            if channels.is_empty() {
                return stop;
            }
            let aabb = Aabb::union(stop.aabb(), channels_aabb(&channels));
            BoundedCsg::ArcChannel {
                inner: Box::new(stop),
                channels,
                aabb,
            }
        }
        BoundedCsg::Union { left, right, aabb } => BoundedCsg::Union {
            left: Box::new(synthesize_offset_channels(*left, tol)),
            right: Box::new(synthesize_offset_channels(*right, tol)),
            aabb,
        },
        BoundedCsg::Difference { left, right, aabb } => BoundedCsg::Difference {
            left: Box::new(synthesize_offset_channels(*left, tol)),
            right: Box::new(synthesize_offset_channels(*right, tol)),
            aabb,
        },
        BoundedCsg::Intersection { left, right, aabb } => BoundedCsg::Intersection {
            left: Box::new(synthesize_offset_channels(*left, tol)),
            right: Box::new(synthesize_offset_channels(*right, tol)),
            aabb,
        },
        BoundedCsg::SmoothUnion { left, right, radius, mode, aabb } => BoundedCsg::SmoothUnion {
            left: Box::new(synthesize_offset_channels(*left, tol)),
            right: Box::new(synthesize_offset_channels(*right, tol)),
            radius,
            mode,
            aabb,
        },
        BoundedCsg::SmoothDifference { left, right, radius, mode, aabb } => {
            BoundedCsg::SmoothDifference {
                left: Box::new(synthesize_offset_channels(*left, tol)),
                right: Box::new(synthesize_offset_channels(*right, tol)),
                radius,
                mode,
                aabb,
            }
        }
        BoundedCsg::Morph { left, right, t, aabb } => BoundedCsg::Morph {
            left: Box::new(synthesize_offset_channels(*left, tol)),
            right: Box::new(synthesize_offset_channels(*right, tol)),
            t,
            aabb,
        },
        BoundedCsg::MaskedBlend {
            left,
            right,
            mask_polyline,
            mask_radius,
            blend_radius,
            blend_mode,
            is_difference,
            provenance,
            aabb,
        } => BoundedCsg::MaskedBlend {
            left: Box::new(synthesize_offset_channels(*left, tol)),
            right: Box::new(synthesize_offset_channels(*right, tol)),
            mask_polyline,
            mask_radius,
            blend_radius,
            blend_mode,
            is_difference,
            provenance,
            aabb,
        },
        BoundedCsg::Twist { inner, rate, aabb } => BoundedCsg::Twist {
            inner: Box::new(synthesize_offset_channels(*inner, tol)),
            rate,
            aabb,
        },
        BoundedCsg::Bend { inner, rate, aabb } => BoundedCsg::Bend {
            inner: Box::new(synthesize_offset_channels(*inner, tol)),
            rate,
            aabb,
        },
        BoundedCsg::Displace { inner, depth, freq, aabb } => BoundedCsg::Displace {
            inner: Box::new(synthesize_offset_channels(*inner, tol)),
            depth,
            freq,
            aabb,
        },
        BoundedCsg::Shell { inner, thickness, open_faces, aabb } => BoundedCsg::Shell {
            inner: Box::new(synthesize_offset_channels(*inner, tol)),
            thickness,
            open_faces,
            aabb,
        },
        BoundedCsg::ArcChannel { inner, channels, aabb } => BoundedCsg::ArcChannel {
            inner: Box::new(synthesize_offset_channels(*inner, tol)),
            channels,
            aabb,
        },
        other @ (BoundedCsg::Empty
        | BoundedCsg::Leaf { .. }
        | BoundedCsg::Extrude { .. }
        | BoundedCsg::Revolve { .. }
        | BoundedCsg::Helix { .. }
        | BoundedCsg::Sweep { .. }) => other,
    }
}

/// Debug-Helfer: zählt Kanäle/Bögen im Baum (Tests, Telemetrie).
pub fn count_channels(node: &BoundedCsg) -> (usize, usize) {
    fn walk(n: &BoundedCsg, acc: &mut (usize, usize)) {
        match n {
            BoundedCsg::ArcChannel { inner, channels, .. } => {
                acc.0 += channels.len();
                acc.1 += channels.iter().map(|c| c.arcs.len()).sum::<usize>();
                walk(inner, acc);
            }
            BoundedCsg::Union { left, right, .. }
            | BoundedCsg::Difference { left, right, .. }
            | BoundedCsg::Intersection { left, right, .. }
            | BoundedCsg::SmoothUnion { left, right, .. }
            | BoundedCsg::SmoothDifference { left, right, .. }
            | BoundedCsg::MaskedBlend { left, right, .. }
            | BoundedCsg::Morph { left, right, .. } => {
                walk(left, acc);
                walk(right, acc);
            }
            BoundedCsg::Twist { inner, .. }
            | BoundedCsg::Bend { inner, .. }
            | BoundedCsg::Displace { inner, .. }
            | BoundedCsg::Shell { inner, .. }
            | BoundedCsg::Offset { inner, .. } => walk(inner, acc),
            _ => {}
        }
    }
    let mut acc = (0, 0);
    walk(node, &mut acc);
    acc
}

#[cfg(test)]
#[path = "offset_channel_tests.rs"]
mod tests;
