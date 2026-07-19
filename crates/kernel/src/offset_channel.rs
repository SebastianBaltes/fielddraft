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
//! w_geo     = min(⟨x−q*, m̂_A⟩, ⟨x−q*, m̂_B⟩, |x−q*| − δ, K − |x−q*|)
//! w         = min(w_geo, gate)
//! konvex:   f = max(d, w)
//! konkav:   f = min(d, −w)      falls w > 0        (Sliver addieren)
//!           f = min(d, −w_geo)  falls w_geo ≤ 0    (Luft-Distanzschranke)
//!           f = d               sonst              (Gate kappt Addition)
//! (m̂ für konkav negiert gespeichert)
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
//! * `gate = d + G` (konvex) bzw. `G − d` (konkav) mit `G = (c−1)·δ + 4·tol`,
//!   `c = 1/cos(γ_max/2)` bzw. `1/sin(α_min/2)`: kappt die Wirkung strikt
//!   auf das legitime Band um die naive Oberfläche — der Maximalbedarf
//!   `max(de − δ ∓ d)` über das Keil-/Sliver-Band ist exakt `(c−1)·δ`
//!   (am naiven Scheitel, d = 0). Phase B hatte hier `c·δ` (ein δ zu
//!   weit): an trunkierten Kurven-Enden (Triple-Junctions) leckte die
//!   Endpunkt-Ball-Kappe dadurch bis ~δ Material über das Ende hinaus;
//!   seit C4 ist das Band scharf und die Ecke gehört der Vertex-Kugel.
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

/// offset_tree Phase C4 (#384): Rossignac–Requicha-**Vertex-Kugel** an
/// einer Triple-Junction. Wo drei Member-Flächen in einem Punkt `v`
/// zusammenstoßen, enden die Kanal-Kurven am `on_composite_boundary`-Gate
/// und lokal bleibt `f − δ` — ein echter Nullmengen-Fehler (konvexe Ecke
/// unter Dilation: Überschuss bis δ·(c_v − 1); konkave Ecke unter
/// Erosion: fehlende Material-Kugel, Komplement-Dualität). Der
/// Korrekturterm ist die C2-Konstruktion mit Punkt-Spine:
///
/// ```text
/// w(x) = min(⟨x−v, ĝ₁⟩, ⟨x−v, ĝ₂⟩, ⟨x−v, ĝ₃⟩, |x−v| − δ, K − |x−v|, gate)
/// konvex:  f = max(d, w)      konkav:  f = min(d, −w)
/// ```
///
/// Die `gates` sind die drei Facetten-Normalen des Normalkegels bei `v`
/// (ĝ_ij ∝ ν_i × ν_j, orientiert mit ĝ_ij·ν_k > 0; ν = Member-Normale,
/// für konkav negiert gespeichert — Auswertung vorzeichenfrei identisch
/// zur Kanal-Dualität). Innerhalb des Kegels ist der nächste Punkt des
/// (lokalen) Trieders exakt `v`, `|x−v| − δ` also die exakte
/// Offset-Distanz; außerhalb ist w < 0 und der Term neutral. Mehr als
/// drei Flächen an einer Ecke → Verweigerung (Synthese).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VertexSphere {
    pub v: [f32; 3],
    /// Drei Kegel-Facetten-Normalen (Einheitslänge; konkav negiert).
    pub gates: [[f32; 3]; 3],
    /// |δ| des Stopps (> 0).
    pub delta: f32,
    /// `true`: −δ-an-Union-Stopp (Material addieren).
    pub concave: bool,
    /// Trust-Radius K um `v` (> δ).
    pub trust: f32,
    /// Gate-Weite G = c_v·δ + Marge.
    pub gate: f32,
    /// Provenance-BranchId (0x700+-Band, Plan §9).
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
    // Zwei getrennte Rollen (C4-Korrektur der Phase-B-Formel):
    // * `w_geo` (reine Geometrie-Terme) trägt BEIDE Seiten: > 0 ⇒ Punkt
    //   liegt im Sliver (Material addieren); ≤ 0 ⇒ −w_geo ist eine
    //   march-sichere Distanzschranke zur Sliver-Hülle (Luftseite —
    //   ohne sie überschätzt das naive Feld die Distanz im Eckenball
    //   um bis zu (1−1/√3)·ρ).
    // * Das `gate` kappt NUR die Addition. Phase B mischte es in die
    //   Klemme: ein negatives Gate klemmte d außerhalb des Bands auf
    //   den Gate-Rest herunter, das Gate des NÄCHSTEN Kanals prüfte
    //   `gate − d` gegen dieses geklemmte statt das naive d und öffnete
    //   fälschlich — an Triple-Junctions addierte der zweite Kanal
    //   Material in wahre Luft (Vorzeichen-Kipp am Ecken-Fixture).
    let w_geo = s1.min(s2).min(de - ch.delta).min(ch.trust - de);
    let w = w_geo.min(gate);
    if ch.concave {
        if w > 0.0 {
            d.min(-w)
        } else if w_geo <= 0.0 {
            d.min(-w_geo)
        } else {
            d
        }
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

/// Wendet EINE Vertex-Kugel auf den Stopp-Wert `d` an (Phase C4) — die
/// CPU-Referenz aller Spiegel (Opcode-Interpreter, WGSL, TSL, C++,
/// fidget, Dual). Struktur identisch zu [`channel_apply`], nur mit
/// Punkt-Spine (`q* = v`, drei Gates statt zwei interpolierten).
pub fn vertex_apply(d: f32, p: [f32; 3], vs: &VertexSphere) -> f32 {
    let dq = sub(p, vs.v);
    let rho = norm(dq);
    // Exakter Fernfeld-Skip (Spiegel von `channel_skip`, Sphäre = Punkt).
    if vs.trust - rho <= if vs.concave { -d } else { d } {
        return d;
    }
    let s1 = dot(dq, vs.gates[0]);
    let s2 = dot(dq, vs.gates[1]);
    let s3 = dot(dq, vs.gates[2]);
    let gate = if vs.concave { vs.gate - d } else { vs.gate + d };
    // Rollen-Trennung wie in [`channel_apply`]: Geometrie-Klemme
    // (march-sichere Luftseite) vs. Gate (kappt nur die Addition).
    let w_geo = s1
        .min(s2)
        .min(s3)
        .min(rho - vs.delta)
        .min(vs.trust - rho);
    let w = w_geo.min(gate);
    if vs.concave {
        if w > 0.0 {
            d.min(-w)
        } else if w_geo <= 0.0 {
            d.min(-w_geo)
        } else {
            d
        }
    } else {
        d.max(w)
    }
}

/// Alle Vertex-Kugeln eines Stopps anwenden (nach den Kanälen).
pub fn verts_apply(mut d: f32, p: [f32; 3], verts: &[VertexSphere]) -> f32 {
    for vs in verts {
        d = vertex_apply(d, p, vs);
    }
    d
}

/// Konservative AABB der Vertex-Kugeln (nur konkav ADDIERT Material).
pub fn verts_aabb(verts: &[VertexSphere]) -> Aabb {
    let mut bb = Aabb::EMPTY;
    for vs in verts {
        if !vs.concave {
            continue;
        }
        let r = vs.trust;
        bb = Aabb::union(
            bb,
            Aabb {
                min: [vs.v[0] - r, vs.v[1] - r, vs.v[2] - r],
                max: [vs.v[0] + r, vs.v[1] + r, vs.v[2] + r],
            },
        );
    }
    bb
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

/// Toleranz-Staffel (Plan §9, Phase C2): Zieltoleranz der
/// Kurvenapproximation UND Schwelle des B0-Toleranz-Prunings in
/// [`build_channel`] (prognostizierter Naht-Fehler δ·(c−1) < tol ⇒ kein
/// Kanal — bei α ≥ 150° liegt der Sliver unter 0.07·δ, B0-CONCAVE-SEAM).
/// **Render-Klasse** (Viewport-Mesh, GPU-SDF, Pick, Weave): 0.01 mm.
pub const CHANNEL_TOL_RENDER: f32 = 0.01;

/// **Export-Klasse** (STL/3MF-Export, CGAL-Server-Mesher): 0.001 mm.
pub const CHANNEL_TOL_EXPORT: f32 = 0.001;

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
    for _ in 0..32 {
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
        let fa_n = fa / na;
        let fb_n = fb / nb;
        // Konvergenz VOR dem Schritt prüfen und die HIER (endlich)
        // ausgewerteten Normalen zurückgeben. Der frühere Ablauf
        // (erst Schritt, dann Neu-Auswertung am Ziel) landete bei
        // achsparallelen Ebenen-Carriern BIT-EXAKT auf der Naht —
        // dort zieht der Dual-Gradient sqrt(0)-NaNs, JEDER Jitter-Start
        // konvergierte zurück auf den singulären Punkt und die gesamte
        // Kanal-Synthese fiel für achsparallele Unions still aus
        // (Phase-C-Befund; die rotierten Notch-/Steinmetz-Fixtures
        // haben das maskiert).
        if fa_n.abs().max(fb_n.abs()) < 1e-5 * scale.max(1.0) {
            return Some(SeamPoint { p, n1: ga, n2: gb });
        }
        let c = dot(ga, gb);
        let det = 1.0 - c * c;
        if det < 1e-6 {
            return None; // tangential
        }
        // Löse fa·? — Schritt in span(ga, gb): p ← p − (α·ga + β·gb)
        // mit [1 c; c 1]·[α β]ᵀ = [fa, fb] (normiert). Leicht gedämpft
        // (0.995), damit der Iterierte nie bit-exakt auf dem Carrier
        // landet — der Restabstand (≲ 0.5 % des letzten Residuums) liegt
        // Größenordnungen unter jeder Fit-Toleranz.
        let alpha = 0.995 * (fa_n - c * fb_n) / det;
        let beta = 0.995 * (fb_n - c * fa_n) / det;
        p = [
            p[0] - alpha * ga[0] - beta * gb[0],
            p[1] - alpha * ga[1] - beta * gb[1],
            p[2] - alpha * ga[2] - beta * gb[2],
        ];
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
    blocking_member(members, &[skip.0, skip.1], p, union_form, eps).is_none()
}

/// Wie [`on_composite_boundary`], liefert aber den ERSTEN verletzenden
/// Member — den Kandidaten-Dritten einer Triple-Junction (Phase C4).
fn blocking_member(
    members: &[Member],
    skip: &[usize],
    p: [f32; 3],
    union_form: bool,
    eps: f32,
) -> Option<usize> {
    for (k, m) in members.iter().enumerate() {
        if skip.contains(&k) {
            continue;
        }
        let v = m.value(p);
        if union_form {
            if v <= eps {
                return Some(k);
            }
        } else if v >= -eps {
            return Some(k);
        }
    }
    None
}

/// Newton-Projektion auf die gemeinsame Nullmenge DREIER Member-Felder
/// (Triple-Punkt, Phase C4). Liefert Punkt + die drei Einheits-Normalen.
/// `None` bei Nicht-Konvergenz oder degenerierter Gradienten-Basis.
fn project_to_vertex(
    members: &[Member],
    tri: [usize; 3],
    start: [f32; 3],
    scale: f32,
) -> Option<([f32; 3], [[f32; 3]; 3])> {
    // Der Startpunkt ist ein projizierter Naht-Punkt und liegt EXAKT auf
    // zwei Blattflächen — dort ziehen Dual-Gradienten sqrt(0)-NaNs (wie
    // bei den SSI-Seeds). Deterministische Jitter heilen das; Newton
    // zieht zurück auf den Triple-Punkt.
    let jit = (scale * 1e-4).max(1e-4);
    'jitter: for off in [
        [0.0, 0.0, 0.0],
        [jit, jit, jit],
        [-jit, jit, -jit],
        [jit, -jit, jit],
        [-jit, -jit, -jit],
    ] {
        let mut p = [start[0] + off[0], start[1] + off[1], start[2] + off[2]];
        for _ in 0..32 {
            let mut f = [0.0f32; 3];
            let mut g = [[0.0f32; 3]; 3];
            for (r, &mi) in tri.iter().enumerate() {
                let (v, gr) = members[mi].value_grad(p);
                let n = norm(gr);
                if !(v.is_finite() && n.is_finite()) || n < 1e-6 {
                    continue 'jitter;
                }
                f[r] = v / n;
                g[r] = [gr[0] / n, gr[1] / n, gr[2] / n];
            }
            // Konvergenz VOR dem Schritt prüfen und die HIER (endlich)
            // ausgewerteten Normalen zurückgeben — eine erneute Auswertung
            // exakt AUF dem Triple-Punkt wäre wieder NaN-gefährdet.
            if f[0].abs().max(f[1].abs()).max(f[2].abs()) < 1e-5 * scale.max(1.0) {
                return Some((p, g));
            }
            // 3×3-Solve (Cramer): g · Δ = f, p ← p − Δ.
            let det = g[0][0] * (g[1][1] * g[2][2] - g[1][2] * g[2][1])
                - g[0][1] * (g[1][0] * g[2][2] - g[1][2] * g[2][0])
                + g[0][2] * (g[1][0] * g[2][1] - g[1][1] * g[2][0]);
            if det.abs() < 1e-4 {
                continue 'jitter; // (nahezu) koplanare Normalen
            }
            let col = |c: usize, rhs: &[f32; 3]| -> [[f32; 3]; 3] {
                let mut m = g;
                for r in 0..3 {
                    m[r][c] = rhs[r];
                }
                m
            };
            let det3 = |m: &[[f32; 3]; 3]| -> f32 {
                m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
                    - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
                    + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
            };
            // Leicht gedämpft wie `project_to_seam`: bit-exaktes Landen
            // auf den Carriern (sqrt(0)-NaN) strukturell vermeiden.
            let dx = 0.995 * det3(&col(0, &f)) / det;
            let dy = 0.995 * det3(&col(1, &f)) / det;
            let dz = 0.995 * det3(&col(2, &f)) / det;
            p = [p[0] - dx, p[1] - dy, p[2] - dz];
        }
    }
    None
}

/// Ein am Composite-Gate abgebrochenes Trace-Ende: der blockierende
/// dritte Member und der blockierte Punkt — Triple-Junction-Kandidat
/// (Phase C4).
#[derive(Clone, Copy)]
struct BlockedEnd {
    k: usize,
    p: [f32; 3],
}

/// Prädiktor-Korrektor-Trace der Naht zwischen Member `i` und `j` ab
/// `seed`, in beide Richtungen, bis Loop-Schluss / Gate-Ende / Budget.
/// Liefert die geordnete Punktkette (bei Loop: geschlossen OHNE
/// dupliziertes Ende), ob es ein geschlossener Loop ist, und die am
/// Composite-Gate blockierten Enden (Triple-Kandidaten, Phase C4).
fn trace_seam(
    members: &[Member],
    i: usize,
    j: usize,
    seed: SeamPoint,
    union_form: bool,
    step: f32,
    fit_tol: f32,
    gate_eps: f32,
) -> Option<(Vec<SeamPoint>, bool, Vec<BlockedEnd>)> {
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
    let march = |dir_sign: f32| -> (Vec<SeamPoint>, bool, Option<BlockedEnd>) {
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
            if let Some(k) =
                blocking_member(members, &[i, j], next.p, union_form, gate_eps)
            {
                // Triple-Kandidat: die Naht (i,j) läuft in Member k hinein.
                return (pts, false, Some(BlockedEnd { k, p: next.p }));
            }
            // Loop-Schluss gegen den SEED (nur vorwärts).
            if dir_sign > 0.0 && pts.len() > 3 && norm(sub(next.p, seed.p)) < h * 0.75 {
                pts.push(next);
                return (pts, true, None);
            }
            pts.push(next);
            prev_t = Some(t);
            cur = next;
        }
        (pts, false, None)
    };

    let (fwd, closed, blocked_f) = march(1.0);
    if closed {
        let mut pts = vec![seed];
        pts.extend(fwd);
        return Some((pts, true, Vec::new()));
    }
    let (bwd, _, blocked_b) = march(-1.0);
    let mut pts: Vec<SeamPoint> = bwd.into_iter().rev().collect();
    pts.push(seed);
    pts.extend(fwd);
    let blocked: Vec<BlockedEnd> = blocked_b.into_iter().chain(blocked_f).collect();
    if pts.len() < 3 {
        return None;
    }
    Some((pts, false, blocked))
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

// ─── Phase C4: Vertex-Kugel-Synthese an Triple-Junctions ────────────────────

/// Triple-Junction-Kandidat: sortiertes Member-Tripel + blockierter Punkt.
struct TripleCand {
    tri: [usize; 3],
    p: [f32; 3],
}

/// Überschuss-Faktor c_v der Ecke: max über Richtungen d im Normalkegel
/// von 1/max_k⟨d, ν_k⟩. Das Minimum von max_k⟨d, ν_k⟩ über den Kegel
/// liegt an der **äquilibrierten Richtung** (alle drei Dots gleich —
/// Lösung von [ν]·d = 1) oder auf einer Kegel-Kante (Paar-Bisektor,
/// falls der dritte Dot dort kleiner ist). Analytisch statt Gitter: ein
/// Baryzentrik-Sampling unterschätzte c_v am Orthogonal-Trieder um 8 %
/// — genug, um mit der (c−1)·δ-Gate-Kappe die tiefste Ecken-Korrektur
/// abzuschneiden. ∞ bei degeneriertem Kegel (Aufrufer verweigert).
fn vertex_c_factor(nu: &[[f32; 3]; 3]) -> f32 {
    let det3 = |m: &[[f32; 3]; 3]| -> f32 {
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    };
    let mut worst = 1.0f32;
    // Kandidat 1: äquilibrierte Richtung [ν]·d = (1,1,1).
    let det = det3(nu);
    if det.abs() < 1e-6 {
        return f32::INFINITY;
    }
    let col = |c: usize| -> [[f32; 3]; 3] {
        let mut m = *nu;
        for r in 0..3 {
            m[r][c] = 1.0;
        }
        m
    };
    let d = [det3(&col(0)) / det, det3(&col(1)) / det, det3(&col(2)) / det];
    let l = norm(d);
    if !(l.is_finite()) || l < 1e-6 {
        return f32::INFINITY;
    }
    let deq = [d[0] / l, d[1] / l, d[2] / l];
    let meq = dot(deq, nu[0]).max(dot(deq, nu[1])).max(dot(deq, nu[2]));
    if meq < 1e-3 {
        return f32::INFINITY;
    }
    worst = worst.max(1.0 / meq);
    // Kandidaten 2: Paar-Bisektoren (Kegel-Kanten).
    for (a, b) in [(0usize, 1usize), (1, 2), (2, 0)] {
        let s = [
            nu[a][0] + nu[b][0],
            nu[a][1] + nu[b][1],
            nu[a][2] + nu[b][2],
        ];
        let l = norm(s);
        if l < 1e-4 {
            return f32::INFINITY;
        }
        let bd = [s[0] / l, s[1] / l, s[2] / l];
        let m = dot(bd, nu[0]).max(dot(bd, nu[1])).max(dot(bd, nu[2]));
        if m < 1e-3 {
            return f32::INFINITY;
        }
        worst = worst.max(1.0 / m);
    }
    worst * 1.005
}

/// Baut aus den blockierten Trace-Enden die Vertex-Kugeln eines Stopps
/// (Phase C4). Verweigerungs-Doktrin: degenerierte Kegel (koplanar,
/// > 3 Flächen im Cluster, Newton-Nichtkonvergenz, zu spitze Ecke)
/// liefern KEINEN Term — dort bleibt lokal `f − δ`, konservativ.
fn synthesize_vertices(
    members: &[Member],
    cands: &[TripleCand],
    ctx: &StopCtx,
    union_form: bool,
    gate_eps: f32,
    scale: f32,
    min_feat: &[u32],
    vert_idx: &mut u32,
) -> Vec<VertexSphere> {
    let mut verts: Vec<VertexSphere> = Vec::new();
    let mut solved: Vec<([f32; 3], [usize; 3], bool)> = Vec::new(); // (v, tri, accepted)
    let cluster_r = (scale * 1e-3).max(10.0 * gate_eps);
    'cand: for cand in cands {
        // Cluster-Dedup: derselbe Eckpunkt fällt aus bis zu drei
        // Paar-Traces heraus (und aus beiden Trace-Richtungen).
        for (v, tri, _) in &solved {
            if norm(sub(cand.p, *v)) < cluster_r && *tri == cand.tri {
                continue 'cand;
            }
        }
        let Some((v, mut normals)) =
            project_to_vertex(members, cand.tri, cand.p, scale)
        else {
            continue;
        };
        for (sv, st, _) in &solved {
            if norm(sub(v, *sv)) < cluster_r {
                // Zweites Tripel am selben Punkt (> 3 Flächen an der Ecke):
                // beide verweigern — der Kegel ist kein Trieder.
                if *st != cand.tri {
                    if let Some(pos) = verts
                        .iter()
                        .position(|w| norm(sub(w.v, *sv)) < cluster_r)
                    {
                        verts.remove(pos);
                    }
                }
                solved.push((v, cand.tri, false));
                continue 'cand;
            }
        }
        solved.push((v, cand.tri, true));
        // Ecke muss auf dem Composite-Rand liegen (alle ÜBRIGEN Member
        // strikt materialseitig).
        if blocking_member(members, &cand.tri, v, union_form, gate_eps).is_some() {
            continue;
        }
        // Dualität: konkav (−δ an Union) arbeitet auf dem Komplement —
        // Normalen negiert, Auswertung identisch (wie Kanal-m̂).
        if ctx.concave {
            for n in normals.iter_mut() {
                *n = [-n[0], -n[1], -n[2]];
            }
        }
        // Kegel-Facetten-Gates: ĝ_ab ∝ ν_a × ν_b, orientiert zu ν_c.
        let mut gates = [[0.0f32; 3]; 3];
        let pairs = [(0usize, 1usize, 2usize), (1, 2, 0), (2, 0, 1)];
        for (gi, &(a, b, c)) in pairs.iter().enumerate() {
            let g = [
                normals[a][1] * normals[b][2] - normals[a][2] * normals[b][1],
                normals[a][2] * normals[b][0] - normals[a][0] * normals[b][2],
                normals[a][0] * normals[b][1] - normals[a][1] * normals[b][0],
            ];
            let l = norm(g);
            if l < 1e-4 {
                continue 'cand; // Paar quasi-parallel — kein Trieder
            }
            let s = if dot(g, normals[c]) >= 0.0 { 1.0 } else { -1.0 };
            gates[gi] = [s * g[0] / l, s * g[1] / l, s * g[2] / l];
        }
        // Spitzheit + Toleranz-Pruning (Spiegel der Kanal-Gates).
        let c_v = vertex_c_factor(&normals);
        if !c_v.is_finite() || 1.0 / c_v < MIN_HALF_ANGLE_SIN {
            continue; // zu spitz/degeneriert — verweigern
        }
        if ctx.delta * (c_v - 1.0) < ctx.tol {
            continue; // Ecken-Fehler unter Toleranz — kein Term nötig
        }
        // Gate wie beim Kanal auf das legitime Band gekappt (s. o.):
        // Maximalbedarf (c_v − 1)·δ am naiven Eck-Scheitel.
        let gate = ctx.delta * (c_v - 1.0) + 4.0 * ctx.tol;
        let trust = ctx.delta * c_v + 0.5 * ctx.delta + 8.0 * ctx.tol;
        // Provenance: Owner = kleinste beteiligte Feature-Id, Sub aus dem
        // 0x700+-Band (Fortsetzung des Kanal-Zählers, deterministisch).
        let owner = cand.tri.iter().map(|&m| min_feat[m]).min().unwrap_or(0);
        let sub = 0x700 + (*vert_idx % 0xFD);
        *vert_idx += 1;
        verts.push(VertexSphere {
            v,
            gates,
            delta: ctx.delta,
            concave: ctx.concave,
            trust,
            gate,
            branch: (owner << 12) | sub,
        });
        if verts.len() >= 64 {
            break;
        }
    }
    verts
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
    // Gate-Weite: die legitime Korrektur braucht w ≤ de − δ ∓ d, deren
    // Maximum über das Sliver-/Keil-Band ist (c − 1)·δ (am naiven
    // Scheitel, d = 0; Herleitung entlang der Bisektrix: de − δ ∓ d =
    // r·(1 − sin(α/2)) ≤ δ·(1/sin − 1)). Phase B setzte hier c·δ — ein
    // volles δ zu weit: an TRUNKIERTEN Kurven-Enden (Triple-Junctions,
    // Deckelkanten) leckte die Endpunkt-Ball-Kappe dadurch bis zu ~δ
    // Material über das Ende hinaus (Phase-C4-Befund am Ecken-Fixture).
    // Mit (c − 1)·δ + Marge ist die Wirkung exakt auf das legitime Band
    // um die naive Fläche gekappt; die Ecke selbst schließt die
    // Vertex-Kugel (C4).
    let gate = ctx.delta * (c_factor - 1.0) + 4.0 * ctx.tol;
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

/// Synthetisiert Kanal-Korrekturkörper + Vertex-Kugeln für EINEN
/// Stopp-Knoten (`Offset { inner, delta }` mit dualem Boolean-Kind).
/// Leere Listen = vollständige Verweigerung, `f − δ` bleibt.
fn synthesize_stop(
    inner: &BoundedCsg,
    delta: f32,
    tol: f32,
) -> (Vec<OffsetChannel>, Vec<VertexSphere>) {
    let union_form = delta < 0.0;
    let mut members = Vec::new();
    flatten_members(inner, union_form, 1.0, &mut members);
    if members.len() < 2 {
        return (Vec::new(), Vec::new());
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
    // Blockierte Enden je Kanal (Index in `channels`, Trace-Schrittweite,
    // Enden) — für das Spine-Snapping an die Vertex-Kugeln (C4).
    let mut chan_ends: Vec<(usize, f32, Vec<BlockedEnd>)> = Vec::new();
    let mut total_arcs = 0usize;
    let mut claimed: Vec<[f32; 3]> = Vec::new(); // Punkte bereits getracter Kurven
    let mut channel_idx = 0u32;

    let gate_eps = (ctx.delta * 1e-3).max(1e-4);

    // Phase C4: Triple-Junction-Kandidaten (blockierte Trace-Enden) +
    // globale Skala + kleinste Feature-Id je Member (Vertex-Provenance).
    let mut triple_cands: Vec<TripleCand> = Vec::new();
    let gscale = {
        let sz = inner.aabb().size();
        sz[0].max(sz[1]).max(sz[2]).max(ctx.delta)
    };
    let min_feat: Vec<u32> = branch_lists
        .iter()
        .map(|bl| bl.iter().map(|b| b.id.feature_id()).min().unwrap_or(0))
        .collect();

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
                                // Budget: bisherige Kanäle behalten; Vertex-
                                // Synthese aus den bis hier gesammelten Enden.
                                let verts = synthesize_vertices(
                                    &members, &triple_cands, &ctx, union_form,
                                    gate_eps, gscale, &min_feat, &mut channel_idx,
                                );
                                return (channels, verts);
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
                            let Some((pts, closed, blocked)) =
                                trace_seam(&members, i, j, sp, union_form, step, fit_tol, gate_eps)
                            else {
                                continue;
                            };
                            // Phase C4: blockierte Enden als Triple-Kandidaten
                            // merken (sortiertes Member-Tripel).
                            for be in &blocked {
                                let mut tri = [i, j, be.k];
                                tri.sort_unstable();
                                triple_cands.push(TripleCand { tri, p: be.p });
                            }
                            for p in &pts {
                                claimed.push(p.p);
                            }
                            // Exponierungs-Witness (Phase C): eine echte
                            // Komposit-Kante verlangt, dass BEIDE Flächen an
                            // der Naht frei liegen — der Schritt von der
                            // Naht entlang der Normale von Fläche i muss
                            // strikt auf der Luftseite (Union) bzw. strikt
                            // im Material (Intersection) des ANDEREN Members
                            // landen. An tangentialen Kanten-Kontakten
                            // (koplanare Deckel überlappender Boxen) liegt
                            // die Feld-Falte auf der Fremdfläche selbst
                            // (Wert ≈ 0) — es gibt keinen Luftkeil, der
                            // Kanal wäre begraben. (Vor der Trace-
                            // Robustheits-Reparatur scheiterten diese Nähte
                            // still an sqrt(0)-NaNs; der Wächter macht die
                            // Verweigerung explizit.)
                            // Nur die Union-Form braucht den Witness: bei
                            // Intersection/Difference ist eine Fläche am
                            // Naht-Punkt automatisch exponiert, sobald die
                            // Normalen nicht parallel sind (Luft = IRGENDEIN
                            // verletztes Constraint). Die Probe-Normalen
                            // kommen aus den CARRIER-Implicits (FD), nicht
                            // aus den Feld-Gradienten — auf Blatt-KANTEN
                            // (koplanare Deckel) liefert das Feld diagonale
                            // Pseudo-Normalen, die den Witness unterlaufen.
                            if union_form {
                                let h = (ctx.delta * 4e-3).max(4.0 * gate_eps);
                                let fd = h * 0.25;
                                let carrier_n = |hs: &fielddraft_protocol::HalfSpace,
                                                 xf: &fielddraft_protocol::Transform,
                                                 p: [f32; 3]|
                                 -> Option<[f32; 3]> {
                                    let f = |q: [f32; 3]| {
                                        crate::csg::sdf_halfspace(
                                            hs,
                                            crate::transform::world_to_local(xf, q),
                                        )
                                    };
                                    let g = [
                                        f([p[0] + fd, p[1], p[2]]) - f([p[0] - fd, p[1], p[2]]),
                                        f([p[0], p[1] + fd, p[2]]) - f([p[0], p[1] - fd, p[2]]),
                                        f([p[0], p[1], p[2] + fd]) - f([p[0], p[1], p[2] - fd]),
                                    ];
                                    let l = norm(g);
                                    if !l.is_finite() || l < 1e-9 {
                                        return None;
                                    }
                                    Some([g[0] / l, g[1] / l, g[2] / l])
                                };
                                let mut probed = 0usize;
                                let mut buried = 0usize;
                                for sp in pts.iter().step_by(4) {
                                    probed += 1;
                                    let (Some(nc_a), Some(nc_b)) = (
                                        carrier_n(hs_a, xf_a, sp.p),
                                        carrier_n(hs_b, xf_b, sp.p),
                                    ) else {
                                        buried += 1;
                                        continue;
                                    };
                                    // Fläche i ist an der Naht exponiert, wenn
                                    // der Schritt IN der Flächenebene zur
                                    // Luftseite von j (u_i ∝ n_j ⊥ n_i) auf
                                    // der Fläche BLEIBT (|f_i| klein — sonst
                                    // endet der Face-Patch an der Naht, der
                                    // Blatt-KANTEN-Fall) und dort j-Luft ist.
                                    let mut exposed = true;
                                    for (n_i, n_j, m_i, m_j) in [
                                        (nc_a, nc_b, &members[i], &members[j]),
                                        (nc_b, nc_a, &members[j], &members[i]),
                                    ] {
                                        let c = dot(n_j, n_i);
                                        let u = [
                                            n_j[0] - c * n_i[0],
                                            n_j[1] - c * n_i[1],
                                            n_j[2] - c * n_i[2],
                                        ];
                                        let l = norm(u);
                                        if l < 1e-4 {
                                            exposed = false;
                                            break;
                                        }
                                        let probe = [
                                            sp.p[0] + h * u[0] / l,
                                            sp.p[1] + h * u[1] / l,
                                            sp.p[2] + h * u[2] / l,
                                        ];
                                        exposed &= m_i.value(probe).abs() < 0.1 * h
                                            && m_j.value(probe) > 0.1 * h;
                                        if !exposed {
                                            break;
                                        }
                                    }
                                    if !exposed {
                                        buried += 1;
                                    }
                                }
                                if probed > 0 && buried * 2 > probed {
                                    continue; // verweigern — kein echter Luftkeil
                                }
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
                                    let verts = synthesize_vertices(
                                        &members, &triple_cands, &ctx, union_form,
                                        gate_eps, gscale, &min_feat, &mut channel_idx,
                                    );
                                    snap_channel_ends_to_vertices(
                                        &mut channels, &chan_ends, &verts,
                                    );
                                    return (channels, verts);
                                }
                                channel_idx += 1;
                                if !blocked.is_empty() {
                                    chan_ends.push((channels.len(), step, blocked.clone()));
                                }
                                channels.push(ch);
                            }
                        }
                    }
                }
            }
        }
    }
    let verts = synthesize_vertices(
        &members, &triple_cands, &ctx, union_form, gate_eps, gscale, &min_feat,
        &mut channel_idx,
    );
    snap_channel_ends_to_vertices(&mut channels, &chan_ends, &verts);
    (channels, verts)
}

/// C4-Spine-Snapping: verlängert Kanal-Kurven, deren Trace am
/// `on_composite_boundary`-Gate VOR einer Triple-Junction stoppte (der
/// Prädiktor bleibt ~1 Schritt vor der Ecke stehen), per Segment bis zum
/// gelösten Vertex. Erst damit misst `de` die Distanz zur VOLLEN Naht:
/// die Endpunkt-Ball-Kappe des kurzen Spines behauptete sonst
/// `de − δ ≥ 0` in der wahren Luft-Kugel der Ecke (Über-Addition), und
/// zwischen Trace-Ende und Ecke fehlte umgekehrt der Sliver. Mit dem
/// Snap ist die End-Kappe exakt die auf den Naht-Fächer beschränkte
/// Vertex-Kugel (R&R: Kanten-Kanäle enden IN den Vertices). Die dualen
/// Kegelvektoren des Endpunkts werden konstant fortgeschrieben
/// (Verlängerung ≤ ~2.5 Schritte, Fehler in der Fit-Toleranz-Klasse).
fn snap_channel_ends_to_vertices(
    channels: &mut [OffsetChannel],
    chan_ends: &[(usize, f32, Vec<BlockedEnd>)],
    verts: &[VertexSphere],
) {
    if verts.is_empty() {
        return;
    }
    for (ci, step, ends) in chan_ends {
        let ch = &mut channels[*ci];
        if ch.arcs.is_empty() {
            continue;
        }
        let snap_r = 3.0 * *step;
        for be in ends {
            let Some(v) = verts
                .iter()
                .map(|vs| vs.v)
                .filter(|v| norm(sub(*v, be.p)) < snap_r)
                .min_by(|a, b| {
                    norm(sub(*a, be.p))
                        .partial_cmp(&norm(sub(*b, be.p)))
                        .unwrap()
                })
            else {
                continue;
            };
            let start = arc_point_at(&ch.arcs[0], 0.0);
            let end = arc_point_at(ch.arcs.last().unwrap(), 1.0);
            let (at_start, q) = if norm(sub(start, be.p)) <= norm(sub(end, be.p)) {
                (true, start)
            } else {
                (false, end)
            };
            if norm(sub(v, q)) < 1e-4 || norm(sub(v, q)) > snap_r {
                continue;
            }
            let seg = if at_start {
                segment_arc(v, q)
            } else {
                segment_arc(q, v)
            };
            let Some(mut seg) = seg else { continue };
            if at_start {
                let a0 = &ch.arcs[0];
                seg.m1s = a0.m1s;
                seg.m2s = a0.m2s;
                seg.m1e = a0.m1s;
                seg.m2e = a0.m2s;
            } else {
                let al = ch.arcs.last().unwrap();
                seg.m1s = al.m1e;
                seg.m2s = al.m2e;
                seg.m1e = al.m1e;
                seg.m2e = al.m2e;
            }
            if at_start {
                // Prepend (Kurvenordnung bleibt Start → Ende).
                let mut arcs = Vec::with_capacity(ch.arcs.len() + 1);
                arcs.push(seg);
                arcs.append(&mut ch.arcs.to_vec());
                ch.arcs = arcs;
            } else {
                ch.arcs.push(seg);
            }
            // Kurven-Bounding-Sphere nachziehen.
            let mut br = ch.bound_r;
            for a in &ch.arcs {
                br = br.max(norm(sub(a.bc, ch.bound_c)) + a.br);
            }
            ch.bound_r = br;
        }
    }
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
            let (channels, verts) = synthesize_stop(&inner, delta, tol);
            let stop = BoundedCsg::Offset { inner, delta, aabb };
            if channels.is_empty() && verts.is_empty() {
                return stop;
            }
            let aabb = Aabb::union(
                Aabb::union(stop.aabb(), channels_aabb(&channels)),
                verts_aabb(&verts),
            );
            BoundedCsg::ArcChannel {
                inner: Box::new(stop),
                channels,
                verts,
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
        // #384 D2: recurse into the eroded copy so any dual offset stop inside it
        // (a shelled boolean body) gets its channel correction bodies too.
        BoundedCsg::InnerWall { inner, aabb } => BoundedCsg::InnerWall {
            inner: Box::new(synthesize_offset_channels(*inner, tol)),
            aabb,
        },
        BoundedCsg::ArcChannel { inner, channels, verts, aabb } => BoundedCsg::ArcChannel {
            inner: Box::new(synthesize_offset_channels(*inner, tol)),
            channels,
            verts,
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

// ─── Phase C2: Dead-Channel-Pruning gegen Union-Nachbarterme ────────────────

/// Halbweite des Oberflächenbands, in dem Pruning beweisbar feld-neutral
/// ist: innerhalb `|f| ≤ PRUNE_BAND` ändert ein gepruneter Kanal weder
/// einen Feldwert noch irgendwo ein Vorzeichen (Beweisskizze am
/// [`prune_dead_channels`]-Doc). Außerhalb des Bands ändern sich nur
/// Innen-/Fernfeld-BETRÄGE — dieselbe Konzession, die der Keil selbst
/// schon macht (Plan §2, „keine Innen-Betragsgarantie").
pub const PRUNE_BAND: f32 = 1.0;

/// Bilanz eines [`prune_dead_channels`]-Laufs (Tests/Telemetrie).
#[derive(Debug, Default, Clone, Copy)]
pub struct PruneStats {
    pub pruned: usize,
    pub kept: usize,
}

/// offset_tree Phase C2 (#384, Plan §7.2): entfernt **tote**
/// Kanal-Korrekturkörper — Kanäle, deren gesamtes Wirkgebiet von einem
/// Union-NACHBARTERM so tief mit Material überdeckt ist, dass der Kanal
/// das Kompositfeld im Oberflächenband nachweislich nicht ändert. Reine
/// Kosten-/Provenance-Hygiene, nie Korrektheit (LOG.md Teil C).
///
/// **Zertifikat (kein Sampling-Risiko):** Sei `ch` ein Kanal am Stopp S,
/// erreichbar von der Wurzel AUSSCHLIESSLICH über Union-Knoten, und `n̂`
/// das (berechnete) Minimum der dabei eingesammelten Geschwister-Felder.
/// Das Komposit ist dann `min(chain(S), n̂, …)` mit `chain` = die übrigen
/// Kanäle des Knotens (monoton, 1-Lipschitz, |chain−roh| ≤ ΣGates =: G).
/// Auf dem ROHEN Stoppfeld gilt: die Naht-Kurve liegt auf dessen
/// Nullmenge, also ist jede Kanal-Wertänderung bei Kurvenabstand
/// `de ≥ trust + s` betragsmäßig ≥ `de − trust ≥ s` und **vorzeichenrein**
/// (beide Werte gleiches Vorzeichen). Fordert man also
/// `n̂ ≤ −(PRUNE_BAND + G)` überall im Schlauch
/// `T = {de ≤ trust + PRUNE_BAND + G}`, dann ist jede Änderung im Band
/// `|Komposit| ≤ PRUNE_BAND` unmöglich (in T drückt `n̂` das Komposit
/// unter −PRUNE_BAND, außerhalb T liegen beide Werte jenseits des Bands,
/// gleichvorzeichlich) — Nullmenge und Band-Werte sind bit-identisch.
/// Die T-Abdeckung wird über die Bogen-Bounding-Sphären zertifiziert:
/// `n̂(bc) ≤ −(PRUNE_BAND + G) − L·(trust + PRUNE_BAND + G + br)` mit
/// `L` = Lipschitz-Schranke des Nachbarfelds (Stufe-2-Netz).
///
/// Vorstufe Intervall-Arithmetik: Kanäle, deren Wirk-Box in keiner
/// Nachbar-AABB liegt, können nicht überdeckt sein — keine Evals.
pub fn prune_dead_channels(root: &mut BoundedCsg, _tol: f32) -> PruneStats {
    let mut stats = PruneStats::default();
    fn region_box(ch: &OffsetChannel, reach: f32) -> Aabb {
        let r = ch.bound_r + reach;
        Aabb {
            min: [ch.bound_c[0] - r, ch.bound_c[1] - r, ch.bound_c[2] - r],
            max: [ch.bound_c[0] + r, ch.bound_c[1] + r, ch.bound_c[2] + r],
        }
    }
    fn box_contains(outer: &Aabb, inner: &Aabb) -> bool {
        !outer.is_empty()
            && !inner.is_empty()
            && (0..3).all(|k| outer.min[k] <= inner.min[k] && outer.max[k] >= inner.max[k])
    }
    fn walk(node: &mut BoundedCsg, neighbors: &[&BoundedCsg], stats: &mut PruneStats) {
        match node {
            BoundedCsg::Union { left, right, .. } => {
                {
                    let mut ctx: Vec<&BoundedCsg> = neighbors.to_vec();
                    ctx.push(&**right);
                    walk(left, &ctx, stats);
                }
                let mut ctx: Vec<&BoundedCsg> = neighbors.to_vec();
                ctx.push(&**left);
                walk(right, &ctx, stats);
            }
            BoundedCsg::ArcChannel { inner, channels, verts, aabb } => {
                if !neighbors.is_empty() {
                    let g_sum: f32 = channels.iter().map(|c| c.gate).sum();
                    let lips: Vec<f32> = neighbors
                        .iter()
                        .map(|n| crate::sdf_compiler::lipschitz_bound(n).max(1e-3))
                        .collect();
                    channels.retain(|ch| {
                        let depth = PRUNE_BAND + g_sum;
                        let reach = ch.trust + depth;
                        let rb = region_box(ch, reach);
                        // Intervall-Vorstufe: ohne AABB-Überdeckung keine Evals.
                        if !neighbors.iter().any(|n| box_contains(&n.aabb(), &rb)) {
                            stats.kept += 1;
                            return true;
                        }
                        // Zertifikat je Bogen-Bounding-Sphere: n̂ tief genug?
                        let dead = ch.arcs.iter().all(|a| {
                            let mut nhat = f32::INFINITY;
                            for (n, l) in neighbors.iter().zip(lips.iter()) {
                                let v = n.sdf(a.bc, EvalMode::Visual);
                                nhat = nhat.min(v + l * (reach + a.br));
                            }
                            nhat <= -depth
                        });
                        if dead {
                            stats.pruned += 1;
                        } else {
                            stats.kept += 1;
                        }
                        !dead
                    });
                    // AABB nachziehen (nur der konkave Fall trug bei).
                    // Vertex-Kugeln werden nicht gepruned (Zwergkosten:
                    // 16 Floats, kein argmin-Scan).
                    *aabb = Aabb::union(
                        Aabb::union(inner.aabb(), channels_aabb(channels)),
                        verts_aabb(verts),
                    );
                }
                // Tiefer liegende Stopps sehen dieselben Union-Nachbarn NICHT
                // mehr zertifiziert (der Pfad ist kein reiner min-Pfad mehr,
                // sobald weitere Knotenarten dazwischen liegen) — Kontext
                // endet hier bewusst.
                walk(inner, &[], stats);
            }
            BoundedCsg::Difference { left, right, .. }
            | BoundedCsg::Intersection { left, right, .. }
            | BoundedCsg::SmoothUnion { left, right, .. }
            | BoundedCsg::SmoothDifference { left, right, .. }
            | BoundedCsg::MaskedBlend { left, right, .. }
            | BoundedCsg::Morph { left, right, .. } => {
                walk(left, &[], stats);
                walk(right, &[], stats);
            }
            BoundedCsg::Twist { inner, .. }
            | BoundedCsg::Bend { inner, .. }
            | BoundedCsg::Displace { inner, .. }
            | BoundedCsg::Shell { inner, .. }
            | BoundedCsg::Offset { inner, .. } => walk(inner, &[], stats),
            _ => {}
        }
    }
    walk(root, &[], &mut stats);
    stats
}

// ─── Phase C3: Vorwarnsignal — Kanal-Selbst-/Paar-Schnitte ──────────────────

/// offset_tree Phase C3 (#384, Plan §7.3): billiges Vorwarnsignal, dass
/// die Kanal-Kantenliste des Ergebnisses strukturell verdächtig ist —
/// zwei Naht-Kurven kommen sich näher als die Summe ihrer Wirkradien
/// (Kurve-Kurve-Distanz < δ_a + δ_b, Normalkegel-Überlappung möglich,
/// Plan §9 „Nichtkonvexe Normalkegel-Überlappung"), oder eine Kurve
/// kommt sich selbst über nicht-benachbarte Bögen so nahe
/// (Selbstschnitt der Offset-Fläche). Der teure, existierende Detektor
/// für die dann nötige echte Topologie ist der Weave des expandierten
/// Körpers (`body::ensure_brep` nimmt bei Kanälen die Octree-Route).
/// Liefert die Zahl der auffälligen Kurvenpaare (0 = unauffällig).
pub fn channels_interference(root: &BoundedCsg) -> usize {
    fn collect<'a>(n: &'a BoundedCsg, out: &mut Vec<&'a OffsetChannel>) {
        match n {
            BoundedCsg::ArcChannel { inner, channels, .. } => {
                out.extend(channels.iter());
                collect(inner, out);
            }
            BoundedCsg::Union { left, right, .. }
            | BoundedCsg::Difference { left, right, .. }
            | BoundedCsg::Intersection { left, right, .. }
            | BoundedCsg::SmoothUnion { left, right, .. }
            | BoundedCsg::SmoothDifference { left, right, .. }
            | BoundedCsg::MaskedBlend { left, right, .. }
            | BoundedCsg::Morph { left, right, .. } => {
                collect(left, out);
                collect(right, out);
            }
            BoundedCsg::Twist { inner, .. }
            | BoundedCsg::Bend { inner, .. }
            | BoundedCsg::Displace { inner, .. }
            | BoundedCsg::Shell { inner, .. }
            | BoundedCsg::Offset { inner, .. } => collect(inner, out),
            _ => {}
        }
    }
    // Bogen-Paar-Distanz: Bounding-Sphere-Schranke, verfeinert durch
    // Punkt-Sampling (8 Stützstellen je Bogen — für ein VORWARNSIGNAL
    // ausreichend; die Entscheidung fällt der Weave).
    fn arcs_close(a: &ChannelArc, b: &ChannelArc, lim: f32) -> bool {
        if norm(sub(a.bc, b.bc)) - a.br - b.br >= lim {
            return false;
        }
        for i in 0..8 {
            let pa = arc_point_at(a, i as f32 / 7.0);
            for j in 0..8 {
                if norm(sub(pa, arc_point_at(b, j as f32 / 7.0))) < lim {
                    return true;
                }
            }
        }
        false
    }
    let mut chans = Vec::new();
    collect(root, &mut chans);
    let mut hits = 0usize;
    for i in 0..chans.len() {
        let a = chans[i];
        // Selbstschnitt: nicht-benachbarte Bögen derselben Kurve.
        let n = a.arcs.len();
        let self_hit = (0..n).any(|k| {
            ((k + 2)..n).any(|m| {
                // Wrap-Nachbarn (geschlossener Loop) ausnehmen.
                if k == 0 && m == n - 1 {
                    return false;
                }
                arcs_close(&a.arcs[k], &a.arcs[m], 2.0 * a.delta)
            })
        });
        if self_hit {
            hits += 1;
        }
        for b in chans.iter().skip(i + 1) {
            let lim = a.delta + b.delta;
            if norm(sub(a.bound_c, b.bound_c)) - a.bound_r - b.bound_r >= lim {
                continue;
            }
            let close = a
                .arcs
                .iter()
                .any(|aa| b.arcs.iter().any(|bb| arcs_close(aa, bb, lim)));
            if close {
                hits += 1;
            }
        }
    }
    hits
}

/// Debug-Helfer: zählt Vertex-Kugeln im Baum (Tests, Telemetrie, C4).
pub fn count_vertex_spheres(node: &BoundedCsg) -> usize {
    fn walk(n: &BoundedCsg, acc: &mut usize) {
        match n {
            BoundedCsg::ArcChannel { inner, verts, .. } => {
                *acc += verts.len();
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
    let mut acc = 0;
    walk(node, &mut acc);
    acc
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
