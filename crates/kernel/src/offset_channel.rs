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
use crate::csg::{Aabb, CsgNode, EvalMode};
use crate::sketch2d::{BoundedSketch2d, Sketch2d, Sketch2dPrimitive, SketchSegment};
use fielddraft_protocol::{FeatureId, Primitive, Transform};

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
    pub branch: u64,
    /// Plan §9, Stufe 2 des zweistufigen Pickings: die BranchIds der BEIDEN
    /// Kanten-Eltern-Flächen, geordnet (`parents[0] <= parents[1]`) — dieselbe
    /// Ordnung, aus der `branch` seinen Feature-Anteil zieht. Der Kanal *erbt*
    /// sie als Mess-/Snap-Referenz, NICHT als Identität: gepickt wird der
    /// Kanal (`branch`), gemessen/gesnappt wird an den Eltern. Reine
    /// Metadaten — feldneutral, kein Opcode sieht sie.
    pub parents: [u64; 2],
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
    pub branch: u64,
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

/// Konservative AABB eines Rand-Fillet-Satzes (offset_tree Phase 4, #398).
/// Nur der konkave Fall ADDIERT Material (Fillet füllt den Sliver); der Zusatz
/// liegt innerhalb `trust` um das Kanten-Segment (Kapsel). Der konvexe Fall
/// schrumpft nur — trägt nichts zur AABB bei (wie bei Kanälen/Vertices).
pub fn rims_aabb(rims: &[crate::offset_rim::RimBlock]) -> Aabb {
    let mut bb = Aabb::EMPTY;
    for block in rims {
        if !block.concave {
            continue;
        }
        for r in &block.recs {
            let t = r.trust;
            for p in [r.a, r.b] {
                bb = Aabb::union(
                    bb,
                    Aabb {
                        min: [p[0] - t, p[1] - t, p[2] - t],
                        max: [p[0] + t, p[1] + t, p[2] + t],
                    },
                );
            }
        }
        // FC Phase 4 (#398): gemischter Arc-Block. Segment (sweep<0): Kapsel um
        // c…c+u·r. Bogen (sweep>0): konservativ die Kugel um das Zentrum mit
        // Radius (r_arc + trust) — enthält den ganzen Bogen samt Fillet-Schneise.
        for r in &block.arc_recs {
            let t = r.trust;
            if r.sweep < 0.0 {
                let a = r.c;
                let b = [r.c[0] + r.u[0] * r.r, r.c[1] + r.u[1] * r.r, r.c[2] + r.u[2] * r.r];
                for p in [a, b] {
                    bb = Aabb::union(
                        bb,
                        Aabb {
                            min: [p[0] - t, p[1] - t, p[2] - t],
                            max: [p[0] + t, p[1] + t, p[2] + t],
                        },
                    );
                }
            } else {
                let rad = r.r + t;
                bb = Aabb::union(
                    bb,
                    Aabb {
                        min: [r.c[0] - rad, r.c[1] - rad, r.c[2] - rad],
                        max: [r.c[0] + rad, r.c[1] + rad, r.c[2] + rad],
                    },
                );
            }
        }
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

/// Tangenten-Einheitsvektor eines Bogens/Segments bei `t ∈ {0,1}`, in
/// Kurvenrichtung (Start → Ende).
fn arc_tangent_at(arc: &ChannelArc, t: f32) -> Option<[f32; 3]> {
    let v = if arc.sweep < 0.0 {
        arc.u
    } else {
        let phi = t * arc.sweep;
        let (s, c) = phi.sin_cos();
        [
            -arc.u[0] * s + arc.v[0] * c,
            -arc.u[1] * s + arc.v[1] * c,
            -arc.u[2] * s + arc.v[2] * c,
        ]
    };
    let l = norm(v);
    if !(l > 1e-9) {
        return None;
    }
    Some([v[0] / l, v[1] / l, v[2] / l])
}

/// #384 §8e — **Rand-Zeuge**: sucht von `x` aus einen Punkt `y` auf der
/// Nullmenge des Komposit-Feldes `f` (der Körperoberfläche des Stopps) und
/// liefert `|x − y|`. Weil `y` NACHWEISLICH auf dem Rand liegt, ist das eine
/// harte OBERE Schranke für `dist(x, ∂Körper)` — und damit ein
/// Gegenbeispiel gegen jede Distanz-Behauptung, die darüber hinausgeht.
///
/// Gedämpfter Newton-Marsch (Gradient JE SCHRITT neu — ein Strahl in
/// Startrichtung verfehlt an Boolean-Kanten den nächsten Rand: über einer
/// Taschen-Mündung zeigt der Gradient nach oben, der nächste Rand liegt
/// seitlich an der Taschenwand). Gesucht wird nur bis `reach`; weiter
/// entfernte Randpunkte widerlegen nichts. `None` = kein Zeuge (kein
/// Beweis, keine Meldung).
fn boundary_witness(
    f: &dyn Fn([f32; 3]) -> f32,
    x: [f32; 3],
    reach: f32,
    scale: f32,
) -> Option<f32> {
    if !(reach > 0.0) {
        return None;
    }
    let eps = (scale * 1e-4).max(1e-5);
    let h = (scale * 1e-3).max(1e-5);
    let mut y = x;
    for _ in 0..32 {
        let fy = f(y);
        if !fy.is_finite() {
            return None;
        }
        if fy.abs() <= eps {
            let u = norm(sub(y, x)) + fy.abs();
            return if u <= reach { Some(u) } else { None };
        }
        let g = [
            f([y[0] + h, y[1], y[2]]) - f([y[0] - h, y[1], y[2]]),
            f([y[0], y[1] + h, y[2]]) - f([y[0], y[1] - h, y[2]]),
            f([y[0], y[1], y[2] + h]) - f([y[0], y[1], y[2] - h]),
        ];
        let l = norm(g);
        if !(l > 1e-9) || !l.is_finite() {
            return None;
        }
        // Newton-Schritt entlang des normierten Gradienten, gedämpft auf den
        // Suchradius (sonst spaziert der Marsch aus der Reichweite).
        let step = (fy * 2.0 * h / l).clamp(-reach, reach);
        y = [
            y[0] - g[0] / l * step,
            y[1] - g[1] / l * step,
            y[2] - g[2] / l * step,
        ];
        if norm(sub(y, x)) > 2.0 * reach {
            return None;
        }
    }
    None
}

/// #384 §8e — **Beweis-Sonde gegen den Bound-Over-Claim am Nahtende.**
///
/// Die C2-Konstruktion ist nur dort exakt, wo der nächste Punkt der
/// Naht-Kurve AUCH der nächste Randpunkt des Körpers ist. In der Umgebung
/// eines offenen Spine-Endes ist das nicht garantiert: jenseits des Endes
/// klemmt `arc_closest` den Fußpunkt auf den Endpunkt (das schließt seit
/// [`close_open_end_caps`] das End-Gate), und seitlich davon kann eine
/// NACHBAR-Naht näher liegen als die eigene — dann behauptet `de − δ` mehr
/// Distanz, als es gibt. Verschärft wird das durch die sequenzielle
/// Anwendung: hat ein Kanal `d` bereits (korrekt) angehoben, öffnet das
/// `gate = G + d` des NÄCHSTEN weiter als beim naiven `d`.
///
/// Die Sonde tastet um jedes offene Ende den Wirkbereich ab (Kegel der
/// Endpunkt-Gates, `δ < |x−E| < K`), wendet die Korrektur **vollständig**
/// an (alle Kanäle, dann die Vertex-Kugeln — genau wie der Evaluator) und
/// sucht ein **Gegenbeispiel**: einen Randpunkt des Körpers, der näher
/// liegt, als das korrigierte Feld behauptet. Ein solcher Zeuge ist ein
/// Beweis, kein Verdacht. Rückgabe: (Verletzung mm, Schranke, Ort).
fn end_region_violation(
    channels: &[OffsetChannel],
    verts: &[VertexSphere],
    field: &dyn Fn([f32; 3]) -> f32,
    delta_signed: f32,
    tol: f32,
) -> Option<(f32, f32, [f32; 3])> {
    let mut worst: Option<(f32, f32, [f32; 3])> = None;
    for ch in channels {
        if ch.arcs.is_empty() {
            continue;
        }
        let dabs = ch.delta;
        if !(dabs > 0.0) || !(ch.trust > dabs) {
            continue;
        }
        let first = &ch.arcs[0];
        let last = ch.arcs.last().unwrap();
        let start = arc_point_at(first, 0.0);
        let end = arc_point_at(last, 1.0);
        if norm(sub(start, end)) < 1e-4 {
            continue; // geschlossene Kurve — kein Ende, kein Kappen-Bereich
        }
        // Die Gates am Ende stammen seit `close_open_end_caps` aus dem
        // End-Gate; für den Kegel-Filter zählen die GEOMETRISCHEN Gates des
        // vorletzten Stützpunkts (sonst filterte die Sonde ihren eigenen Fix).
        let ends = [
            (start, nlerp(first.m1s, first.m1e, 1.0), nlerp(first.m2s, first.m2e, 1.0)),
            (end, nlerp(last.m1e, last.m1s, 1.0), nlerp(last.m2e, last.m2s, 1.0)),
        ];
        for (e, m1, m2) in ends {
            for dir in FIB_DIRS.iter() {
                if dot(*dir, m1) <= 0.05 || dot(*dir, m2) <= 0.05 {
                    continue;
                }
                for k in 1..=10 {
                    // Wirkfenster des Terms: δ < de < K.
                    let r = dabs + (ch.trust - dabs) * (k as f32 / 11.0);
                    let x = [e[0] + dir[0] * r, e[1] + dir[1] * r, e[2] + dir[2] * r];
                    let d = field(x) - delta_signed;
                    if !d.is_finite() {
                        continue;
                    }
                    let corr = verts_apply(channels_apply(d, x, channels), x, verts);
                    // Geprüft wird die NULLMENGE (die Zusage des Stopps), und
                    // nur, was der Term VERURSACHT: das naive Feld muss den
                    // Punkt richtig klassifizieren, das korrigierte falsch.
                    // Bewusst NICHT geprüft: reine Wert-Unterschätzungen des
                    // konkaven Terms — die Luftseiten-Klemme `min(d, −w_geo)`
                    // unterschätzt die Distanz absichtlich (march-sicher).
                    let flipped = if delta_signed > 0.0 {
                        d <= 0.0 && corr > 0.0
                    } else {
                        d >= 0.0 && corr < 0.0
                    };
                    if !flipped {
                        continue;
                    }
                    // Zeuge: ein Randpunkt näher als δ beweist die wahre Lage.
                    // Dilation: `dist(x, Körper) ≤ u < δ` ⇒ x liegt IN der
                    // Dilatation (das Feld behauptet außen). Erosion:
                    // `dist(x, ∁Körper) ≤ u < δ` ⇒ die δ-Kugel um x ragt aus dem
                    // Körper, x liegt also NICHT in der Erosion (das Feld
                    // behauptet innen). Beides ist ein harter Widerspruch.
                    let Some(u) = boundary_witness(field, x, dabs - tol, ch.trust) else {
                        continue;
                    };
                    if u >= dabs - tol {
                        continue;
                    }
                    let viol = if delta_signed > 0.0 {
                        corr + (dabs - u)
                    } else {
                        (dabs - u) - corr
                    };
                    if viol > tol && worst.map_or(true, |(w, _, _)| viol > w) {
                        worst = Some((viol, ch.gate, x));
                    }
                }
            }
        }
    }
    worst
}

/// Teilstück eines Bogens/Segments für `t ∈ [t0, t1] ⊆ [0,1]` — samt
/// nlerp-interpolierten Kegelvektoren und frischer Bounding-Sphere. Basis
/// des End-Kappen-Gates (#384 §8e).
fn arc_subrange(arc: &ChannelArc, t0: f32, t1: f32) -> Option<ChannelArc> {
    if !(t1 > t0) {
        return None;
    }
    let m1s = nlerp(arc.m1s, arc.m1e, t0);
    let m2s = nlerp(arc.m2s, arc.m2e, t0);
    let m1e = nlerp(arc.m1s, arc.m1e, t1);
    let m2e = nlerp(arc.m2s, arc.m2e, t1);
    let mut out = if arc.sweep < 0.0 {
        segment_arc(arc_point_at(arc, t0), arc_point_at(arc, t1))?
    } else {
        let phi0 = t0 * arc.sweep;
        let (s0, c0) = phi0.sin_cos();
        let u = [
            arc.u[0] * c0 + arc.v[0] * s0,
            arc.u[1] * c0 + arc.v[1] * s0,
            arc.u[2] * c0 + arc.v[2] * s0,
        ];
        let v = [
            -arc.u[0] * s0 + arc.v[0] * c0,
            -arc.u[1] * s0 + arc.v[1] * c0,
            -arc.u[2] * s0 + arc.v[2] * c0,
        ];
        finish_arc(ChannelArc {
            c: arc.c,
            u,
            v,
            r: arc.r,
            sweep: (t1 - t0) * arc.sweep,
            bc: [0.0; 3],
            br: 0.0,
            m1s: [0.0; 3],
            m2s: [0.0; 3],
            m1e: [0.0; 3],
            m2e: [0.0; 3],
        })
    };
    out.m1s = m1s;
    out.m2s = m2s;
    out.m1e = m1e;
    out.m2e = m2e;
    Some(out)
}

/// #384 §8e — **End-Kappen-Gate.** Am offenen Ende einer Spine klemmt
/// `arc_closest` den Fußpunkt auf den Endpunkt: jenseits des Nahtendes misst
/// `de` die Distanz zu EINEM Punkt, während die beiden Kegel-Gates der Kante
/// weiter offen stehen. Der Term wirkt dort bis `K ≈ 1.95·δ` über die Naht
/// hinaus — quer über Flächen, die dort ungestört weiterlaufen (§8d-Befund:
/// 0.760 mm Fehlstelle an der Außenkante des Trägerkörpers; Spiegel am
/// Erosions-Stopp: 0.800 mm Phantom-Material an den Enden des
/// `notch_union`-Dieders). Die C2-Exaktheit gilt dort nicht — der nächste
/// Randpunkt ist nicht mehr der Fußpunkt.
///
/// Der Opcode kann das nicht selbst wissen: er hat je Bogen genau ZWEI
/// Gates, und ein drittes wäre eine Semantik-Änderung in sechs
/// Evaluator-Spiegeln. Aber die Gates sind **Daten**. Am terminalen
/// Bogenstück (`0.25·δ`) wird das zweite Gate auf die **Endebenen-Normale**
/// `∓t̂` gedreht:
///
/// * jenseits der Endebene ist `⟨x−E, ∓t̂⟩ < 0` ⇒ `w < 0` ⇒ der Term ist
///   strukturell AUS — genau die Kappe, die den Bound-Over-Claim trug;
/// * davor ist der Fußpunkt innerlich und `⟨x−q*, t̂⟩ = 0`, das Gate also
///   neutral; die nlerp-Interpolation blendet die Korrektur auf dem kurzen
///   Endstück stetig aus (kein Sprung im Feld).
///
/// Geschlossene Spines haben kein Ende und bleiben unberührt; an gesnappten
/// Enden übernimmt die Vertex-Kugel mit ihren DREI Gates — genau die
/// R&R-Arbeitsteilung „Kanten-Kanäle enden IN den Vertices" (C4).
fn close_open_end_caps(channels: &mut [OffsetChannel], budget: usize) {
    let mut arcs_total: usize = channels.iter().map(|c| c.arcs.len()).sum();
    for ch in channels.iter_mut() {
        if ch.arcs.is_empty() {
            continue;
        }
        let start = arc_point_at(&ch.arcs[0], 0.0);
        let end = arc_point_at(ch.arcs.last().unwrap(), 1.0);
        if norm(sub(start, end)) < 1e-4 {
            continue; // geschlossene Kurve — kein Ende, keine Kappe
        }
        let cap_len = 0.25 * ch.delta;
        for at_start in [false, true] {
            let idx = if at_start { 0 } else { ch.arcs.len() - 1 };
            let arc = ch.arcs[idx].clone();
            let Some(t) = arc_tangent_at(&arc, if at_start { 0.0 } else { 1.0 }) else {
                continue;
            };
            // Gate-Vektor = Endebenen-Normale, nach INNEN gerichtet.
            let gate_n = if at_start { t } else { [-t[0], -t[1], -t[2]] };
            let len = if arc.sweep < 0.0 { arc.r } else { arc.r * arc.sweep };
            if len > 2.0 * cap_len && arcs_total < budget {
                let f = cap_len / len;
                let (t0, t1) = if at_start { (0.0, f) } else { (1.0 - f, 1.0) };
                let (Some(mut cap), Some(rest)) = (
                    arc_subrange(&arc, t0, t1),
                    if at_start {
                        arc_subrange(&arc, t1, 1.0)
                    } else {
                        arc_subrange(&arc, 0.0, t0)
                    },
                ) else {
                    continue;
                };
                if at_start {
                    cap.m2s = gate_n;
                    ch.arcs[idx] = rest;
                    ch.arcs.insert(idx, cap);
                } else {
                    cap.m2e = gate_n;
                    ch.arcs[idx] = rest;
                    ch.arcs.push(cap);
                }
                arcs_total += 1;
            } else if at_start {
                // Bogen zu kurz (oder Arc-Budget erschöpft): das Gate gilt für
                // den ganzen terminalen Bogen — die Korrektur blendet über ihn
                // aus, die Kappe ist zu.
                ch.arcs[idx].m2s = gate_n;
            } else {
                ch.arcs[idx].m2e = gate_n;
            }
        }
    }
}

/// Deterministische Richtungs-Stichprobe (Fibonacci-Kugel, 240 Punkte) für
/// [`end_region_violation`]. Als Konstante berechnet — die Sonde läuft je
/// Kanal-Ende und darf keine Allokation kosten.
static FIB_DIRS: std::sync::LazyLock<Vec<[f32; 3]>> = std::sync::LazyLock::new(|| {
    let n = 240usize;
    let ga = std::f64::consts::PI * (3.0 - 5.0f64.sqrt());
    (0..n)
        .map(|i| {
            let z = 1.0 - 2.0 * (i as f64 + 0.5) / n as f64;
            let r = (1.0 - z * z).max(0.0).sqrt();
            let th = ga * i as f64;
            [(r * th.cos()) as f32, (r * th.sin()) as f32, z as f32]
        })
        .collect()
});

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
            // Explizit über den Konstruktor, nicht handgepackt: das
            // Instanz-Feld (#429/E1) sitzt zwischen Feature und Sub.
            branch: crate::csg::BranchId::from_parts(owner, 0, sub).as_u64(),
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
    branch: u64,
    parents: [u64; 2],
    report: &mut crate::offset_capability::CapabilityReport,
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
        // Durchgehend tangential — es gibt keine echte Dieder-Naht, also
        // keinen f−δ-Fehler zu korrigieren (benign, kein Downgrade).
        return None;
    }
    if min_half < MIN_HALF_ANGLE_SIN {
        // Zu spitz — kein Kanal; der f−δ-Fehler an der Naht bleibt unbeschränkt.
        report.note_conservative(
            "Boolean-Naht mit zu spitzem Winkel — keine Kanal-Korrektur möglich, Näherung f−δ.",
        );
        return None;
    }
    // Prognostizierter Naht-Fehler ohne Kanal: δ·(c − 1). Liegt er unter
    // der Toleranz, lohnt kein Kanal (Pruning-Kriterium der B0-Messreihe) —
    // die Naht bleibt f−δ, aber der Fehler ist beweisbar ≤ tol
    // (toleranzbeschränkt, nicht unbeschränkt).
    if ctx.delta * (worst_c - 1.0) < ctx.tol {
        report.note_tolerance(format!(
            "Flache Boolean-Naht ohne Kanal — Näherungsfehler beweisbar ≤ {} mm.",
            ctx.tol
        ));
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
        parents,
    })
}

/// #394 (Ehrlichkeits-Heuristik, Befund F2 des #384-Fuzz): Erkennt einen
/// **koplanaren/tangentialen internen Grenzflächen-Kontakt** zwischen zwei
/// Booleschen Membern eines dualen Stopps — zwei planare Carrier-Flächen,
/// die in WELT-Koordinaten koinzident, gegenläufig orientiert
/// (anti-parallel) und in ≥2 Achsen flächig überlappend sind.
///
/// Das ist genau der F2-Fall: bündig gestapelte/berührende Boxen. Dort
/// unterschätzt `min(A, B)` an der submergierten gemeinsamen Fläche die
/// wahre Tiefe (die nächste Primitivfläche liegt IM Nachbar-Körper); an
/// einem zero-thickness-Kontakt findet die SSI-Nahtsuche keine abdeckende
/// Nahtkurve, und ein Arc-Spline-Kanal (1D-Kurve) kann einen flächigen 2D-
/// Grenzbereich ohnehin nicht korrigieren. `min(A, B) ∓ δ` bleibt dort nur
/// eine Näherung — der gemessene Nullmengen-Fehler liegt weit über jeder
/// Toleranz (Fuzz p99 ≫ 1 mm), während Status ohne diese Heuristik
/// fälschlich `exact`/`tolerance_bounded` bliebe.
///
/// **Rein struktureller Test** aus den Carrier-Halfspace-Karten — kein
/// Sampling, keine Feldauswertung. Er meldet nur (Kandidat 1 ist reine
/// Ehrlichkeit, s. offset_capability-Doku); das Feld bleibt bit-identisch.
/// TRANSVERSALE Dieder-Nähte (nᵢ·nⱼ weg von −1) korrigiert die Kanal-
/// Synthese exakt/toleranzbeschränkt und lösen NICHT aus; tangentiale
/// Punkt-/Kurvenkontakte gekrümmter Flächen (Kugel-auf-Box — kein planarer
/// Carrier koinzident mit der Box-Fläche) lösen ebenfalls NICHT aus.
fn coplanar_interface_present(
    branch_maps: &[crate::brep::BranchSurfaceMap],
    branch_lists: &[Vec<crate::brep::branch_pairs::Branch>],
    signs: &[f32],
    delta_abs: f32,
    coincide_tol: f32,
) -> bool {
    // Welt-Ebenen `(n̂_eff, O_eff, Flächen-AABB)` je Member aus den planaren
    // Carriern. #397: **sign-bewusst** — die für den Stopp relevante
    // Orientierung ist die EFFEKTIVE Normale `eff = −sign·n̂` (und der effektive
    // Offset `O = −sign·o`), mit `sign` aus `flatten_members` (Difference-
    // Rechtskind trägt −1). In der Erosions-Normalform sind alle Vorzeichen +1,
    // dann ist `eff = −n̂`, `O = −o`: der gemeinsame Vorzeichen-Flip lässt den
    // anti-parallel/koinzidenz-Test (Skalarprodukt, |O_i+O_j|) BIT-IDENTISCH —
    // der Erosions-Pfad ändert sein Verhalten nicht. Am Dilations-Stopp
    // (De-Morgan-Komplement) macht `eff` aus dem parallel-normalen Difference-
    // Kontakt (flush cut) einen anti-parallelen — genau den blinden Fleck, den
    // die WAHRE-Normalen-Fassung nicht sah.
    let planes: Vec<Vec<([f32; 3], f32, Aabb)>> = branch_lists
        .iter()
        .zip(branch_maps.iter())
        .zip(signs.iter())
        .map(|((bl, map), &sign)| {
            let mut out = Vec::new();
            for b in bl {
                let Some((fielddraft_protocol::HalfSpace::Plane { normal, offset }, xf)) =
                    map.get(&b.id.as_u64())
                else {
                    continue;
                };
                let wn = crate::transform::quat_rotate(xf.rotation, *normal);
                let ln = norm(wn);
                if ln < 1e-6 {
                    continue;
                }
                let wn = [wn[0] / ln, wn[1] / ln, wn[2] / ln];
                // Lokaler Ebenen-Fußpunkt `n·offset` → Welt (uniforme Skala).
                let lp = [normal[0] * offset, normal[1] * offset, normal[2] * offset];
                let sp = [lp[0] * xf.scale[0], lp[1] * xf.scale[1], lp[2] * xf.scale[2]];
                let wp = crate::transform::quat_rotate(xf.rotation, sp);
                let wp = [wp[0] + xf.translate[0], wp[1] + xf.translate[1], wp[2] + xf.translate[2]];
                let wo = dot(wn, wp);
                // Effektive (Komplement-)Normale/-Offset.
                let en = [-sign * wn[0], -sign * wn[1], -sign * wn[2]];
                let eo = -sign * wo;
                out.push((en, eo, b.aabb));
            }
            out
        })
        .collect();

    // Toleranzen δ-skaliert: ein flächiger Kontakt innerhalb ~½δ ist bereits
    // der degenerierte Fall (die Kontaktzone liegt im Offset-Band). Die
    // Koinzidenz-Toleranz kommt vom Aufrufer (der eff-Pfad braucht eine engere
    // — s. Aufruf-Doku).
    let overlap_min = (0.5 * delta_abs).max(1e-3);
    for i in 0..planes.len() {
        for j in (i + 1)..planes.len() {
            for &(ni, oi, ai) in &planes[i] {
                for &(nj, oj, aj) in &planes[j] {
                    // Anti-parallel + koinzident (der Vorzeichen-Flip der
                    // Normale kippt zugleich das Vorzeichen von o).
                    if dot(ni, nj) > -0.9995 {
                        continue;
                    }
                    if (oi + oj).abs() > coincide_tol {
                        continue;
                    }
                    // Echte 2D-Grenzfläche (≥2 Achsen mit Überlapp), kein
                    // blosser Kanten-/Punktkontakt. Der `gap` überbrückt die
                    // f32-Rundung zweier eigentlich koinzidenter Flächen-AABBs
                    // (bündige Boxen berühren sich in der Normalachse mit ~1 ULP
                    // Lücke → `intersect` sonst leer); er ist ≪ overlap_min,
                    // erzeugt also keine falsche 2D-Überlappung an Kanten.
                    let gap = (0.05 * delta_abs).max(1e-2);
                    let ov = Aabb::intersect(ai.expanded(gap), aj.expanded(gap));
                    if ov.is_empty() {
                        continue;
                    }
                    let sz = ov.size();
                    if (0..3).filter(|&k| sz[k] > overlap_min).count() >= 2 {
                        return true;
                    }
                }
            }
        }
    }
    false
}

// ─── F395 (#395): Kontakt-Prisma + ∂P-Rim für koplanare Kontaktflächen ───────
//
// Design-SSOT: docs/research/sdf-metricity/F395-CONTACT-PATCH-DESIGN.md.
//
// Bei EROSION `−δ` über einer Union mit einer submergierten koplanaren
// Kontaktfläche (bündig gestapelte Boxen) lässt `min(f_i) + |δ|` einen
// flächigen Phantom-Schlitz auf der gemeinsamen Ebene Π stehen (#394 meldet
// das ehrlich als `conservative`). Der Korrekturterm ist ein **Kontakt-Prisma**
// S = Extrude(P) über dem Kontaktpolygon P = Verschnitt der beiden Kontakt-
// Flächen-Loops (Design §1.3, Kandidat a). Weil S ⊆ U und S keine innere
// Fläche auf Π hat, füllt `min(f_i, f_S) + |δ|` den Schlitz exakt (gleiche
// Querschnitte) bzw. bis auf den δ·(1/sin(α/2)−1)-Sliver am Rand ∂P, den der
// bestehende `OP_ARC_CHANNEL` schließt (Stufe 2, ∂P statt SSI als Seed-Quelle).
//
// **Bauart (Design §3.2, „regulärer Member"):** das Prisma ist ein
// `BoundedCsg::Extrude`, dessen `world_transform` die orientierte Π-Lage trägt
// (lokale Z-Achse → n̂, lokale XY → Π-Basis). Kein neuer Opcode — alle
// Evaluator-Spiegel (CPU/eval_opcodes/sdf-vm/fidget/CGAL) sehen ihn als
// gewöhnliches Extrude. Der Verschnitt der zwei konvexen Kontakt-Rechtecke
// (Box-Flächen) läuft per Sutherland–Hodgman (konvex∩konvex ist metrisch
// exakt und ohne die Orientierungs-/Overlapping-Degenerationen von
// cavalier_contours am beweistragenden Gleich-Querschnitt-Fall; die Lib bleibt
// der Pfad für nicht-konvexe Extrude-Flächen, sobald diese dazukommen).
//
// **Scope Stufe 1+2:** nur Erosion über Union (`union_form`), nur Box-Member
// (planar-facettierte Extrudes folgen). Alles andere → Verweigerung, #394
// bleibt.

/// f64-Geometrie-Helfer (der Verschnitt/die Projektion rechnen in f64, damit
/// die koplanaren Nähte robust sind; erst der fertige `BoundedCsg` ist f32).
#[inline]
fn fdot(a: [f64; 3], b: [f64; 3]) -> f64 { a[0] * b[0] + a[1] * b[1] + a[2] * b[2] }
#[inline]
fn fcross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
#[inline]
fn fnorm(a: [f64; 3]) -> f64 { fdot(a, a).sqrt() }
#[inline]
fn fnrm(a: [f64; 3]) -> [f64; 3] {
    let n = fnorm(a).max(1e-30);
    [a[0] / n, a[1] / n, a[2] / n]
}

/// Eine planare Member-Fläche in Weltkoordinaten (für den Kontakt-Verschnitt).
///
/// Die Kontur ist ein geschlossener Loop aus Welt-3D-Punkten mit **optionalem
/// Bulge je Kante** (cavalier-Konvention: `bulge[i] = tan(θ/4)` des Bogens von
/// `loop3[i]` nach `loop3[(i+1)%n]`, `0` = Gerade). Der Bulge ist so signiert
/// abgelegt, dass er **verbatim** in eine `cavalier_contours::Polyline` fließt,
/// nachdem `loop3` mit `plane_frame(n)` in die Ebene projiziert wurde — der
/// Facetten-Extraktor (`member_contact_faces`) rechnet die Orientierungs-
/// Umkehr der ±n̂-Deckel bereits ein. Eine Box-Rechteck-Fläche ist vier Punkte
/// mit Bulge `0` (byte-gleich zum alten Rechteck-Pfad).
struct ContactFace {
    n: [f64; 3],           // Außennormale (Welt, Einheit)
    o: f64,                // Ebenen-Offset n·x = o
    loop3: Vec<[f64; 3]>,  // Welt-Loop der Fläche (Rechteck-/Profil-Ecken)
    /// Signierter Bulge je Loop-Kante (`loop3[i]→loop3[i+1]`); Länge =
    /// `loop3.len()`. Box-/Mantel-Rechtecke: alle `0`. Vom cavalier-Pfad
    /// `clip_faces` gelesen.
    bulges: Vec<f64>,
    depth: f64,            // volle Member-Dicke normal zur Fläche
    /// `true`: konvexes achsen-aligniertes Box-Rechteck → der Verschnitt darf
    /// den metrisch-exakten Sutherland–Hodgman-Fast-Path nehmen. `false`:
    /// beliebige (ggf. nicht-konvexe / bulge-tragende) Extrude-Kontur → der
    /// Verschnitt MUSS über `cavalier_contours` laufen.
    box_rect: bool,
}

/// Die sechs Flächen eines Box-Members in Weltkoordinaten, oder `None`, wenn
/// der Member keine achsen-alignierte Box mit (nahezu) uniformer Skala ist
/// (dann ist P nicht exakt bestimmbar → Verweigerung, Design §1.5).
fn box_contact_faces(node: &BoundedCsg) -> Option<Vec<ContactFace>> {
    let BoundedCsg::Leaf {
        primitive: Primitive::Box { center, half_extents },
        world_transform,
        ..
    } = node
    else {
        return None;
    };
    let s = world_transform.scale;
    // Uniforme (isotrope) Skala verlangt: sonst bleiben die Flächen zwar
    // planar, aber die Normalen-/Dicken-Rechnung unten (rotation-only) ist
    // nicht mehr metrisch — dann lieber verweigern.
    if (s[0] - s[1]).abs() > 1e-4 * s[0].abs().max(1.0)
        || (s[0] - s[2]).abs() > 1e-4 * s[0].abs().max(1.0)
        || s[0] <= 0.0
    {
        return None;
    }
    let scl = s[0] as f64;
    let rot = world_transform.rotation;
    let tr = [
        world_transform.translate[0] as f64,
        world_transform.translate[1] as f64,
        world_transform.translate[2] as f64,
    ];
    let l2w = |p: [f64; 3]| -> [f64; 3] {
        let sp = [(p[0] * scl) as f32, (p[1] * scl) as f32, (p[2] * scl) as f32];
        let r = crate::transform::quat_rotate(rot, sp);
        [r[0] as f64 + tr[0], r[1] as f64 + tr[1], r[2] as f64 + tr[2]]
    };
    let rot_dir = |p: [f64; 3]| -> [f64; 3] {
        let r = crate::transform::quat_rotate(rot, [p[0] as f32, p[1] as f32, p[2] as f32]);
        fnrm([r[0] as f64, r[1] as f64, r[2] as f64])
    };
    let c = [center[0] as f64, center[1] as f64, center[2] as f64];
    let h = [half_extents[0] as f64, half_extents[1] as f64, half_extents[2] as f64];
    let unit = |k: usize| -> [f64; 3] {
        let mut e = [0.0; 3];
        e[k] = 1.0;
        e
    };
    let mut out = Vec::with_capacity(6);
    for k in 0..3 {
        let a = (k + 1) % 3;
        let b = (k + 2) % 3;
        for sgn in [1.0f64, -1.0] {
            let nl = [unit(k)[0] * sgn, unit(k)[1] * sgn, unit(k)[2] * sgn];
            let n = rot_dir(nl);
            // Flächen-Fußpunkt (lokal) + vier Rechteck-Ecken (lokal → Welt).
            let fc = [c[0] + nl[0] * h[k], c[1] + nl[1] * h[k], c[2] + nl[2] * h[k]];
            let ea = unit(a);
            let eb = unit(b);
            let mut loop3 = Vec::with_capacity(4);
            for (sa, sb) in [(1.0f64, 1.0f64), (1.0, -1.0), (-1.0, -1.0), (-1.0, 1.0)] {
                let corner = [
                    fc[0] + ea[0] * sa * h[a] + eb[0] * sb * h[b],
                    fc[1] + ea[1] * sa * h[a] + eb[1] * sb * h[b],
                    fc[2] + ea[2] * sa * h[a] + eb[2] * sb * h[b],
                ];
                loop3.push(l2w(corner));
            }
            let fcw = l2w(fc);
            out.push(ContactFace {
                n,
                o: fdot(n, fcw),
                loop3,
                bulges: vec![0.0; 4], // Rechteck-Ecken, keine Bögen
                depth: 2.0 * h[k] * scl,
                box_rect: true,
            });
        }
    }
    Some(out)
}

// ─── FC Phase 1 (#398): Facetten-Extraktion für Extrude-Member + cavalier- ────
// Verschnitt (Folgeprogramm c, docs/…/FC-NONCONVEX-CONTACT-DESIGN.md §2/§3).
//
// FELDNEUTRAL & UNVERDRAHTET: `member_contact_faces` und `clip_faces` sind der
// verallgemeinerte Nachbau von `box_contact_faces` + `clip_convex` für
// NICHT-konvexe / facettierte Kontakte. Die Synthese (`synthesize_contact_*`,
// `contact_clips`) benutzt sie in Phase 1 NOCH NICHT — Nicht-Box-Kontakte
// bleiben verweigert (`refused`), kein Tape-Byte ändert sich. Verdrahtung ist
// Phase 2.

/// Eine 2D-Kontur in der Kontaktebene Π: geschlossener Loop aus In-Ebenen-
/// Punkten mit optionalem Bulge je Kante (cavalier-Konvention:
/// `bulges[i] = tan(θ/4)` des Bogens von `verts[i]` nach `verts[(i+1)%n]`,
/// `0` = Gerade). Ein Box-/Mantel-Rechteck ist vier Punkte mit Bulge `0`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Contour2d {
    pub(crate) verts: Vec<[f64; 2]>,
    pub(crate) bulges: Vec<f64>,
}

impl Contour2d {
    /// Gerade Kontur ohne Bögen (Bulges alle `0`).
    fn straight(verts: Vec<[f64; 2]>) -> Self {
        let bulges = vec![0.0; verts.len()];
        Self { verts, bulges }
    }
    /// Trägt die Kontur mindestens einen echten Bogen? (P2: `build_prism_component`
    /// verweigert Bogen-Konturen; P3 baut sie als `Path`-Prisma.)
    pub(crate) fn has_arcs(&self) -> bool {
        self.bulges.iter().any(|b| b.abs() > 1e-12)
    }
}

/// Eine Zusammenhangskomponente eines (ggf. nicht-konvexen, mehrfach-
/// zusammenhängenden) Kontakts P: äußere Kontur (CCW) samt optionalen Löchern
/// (aus `cavalier`s `neg_plines`; hier auf CCW normalisiert). Der Box-Box-Fall
/// ist EINE Komponente ohne Löcher, byte-gleich zum bisherigen Ein-Polygon-P.
#[derive(Clone, Debug)]
pub(crate) struct ClipComponent {
    pub(crate) outer: Contour2d,
    pub(crate) holes: Vec<Contour2d>,
}

/// Ein Profil-Segment in Sketch-2D-Koordinaten: Startpunkt + Kantentyp zum
/// nächsten Segment-Start. Zwischenschritt der Facetten-Extraktion.
enum ProfileSeg {
    /// Gerade — trägt eine Mantel-Rechteck-Facette.
    Line { start: [f64; 2] },
    /// Kreisbogen (Deckel-Bulge = `tan(θ/4)`); Mantel gekrümmt → keine Facette.
    Arc { start: [f64; 2], bulge: f64 },
    /// Quadratische Bézier — kein exakter Bulge → der Deckel wird verweigert.
    Bezier { start: [f64; 2] },
}

impl ProfileSeg {
    fn start(&self) -> [f64; 2] {
        match self {
            ProfileSeg::Line { start } | ProfileSeg::Arc { start, .. } | ProfileSeg::Bezier { start } => *start,
        }
    }
    fn bulge(&self) -> f64 {
        match self {
            ProfileSeg::Arc { bulge, .. } => *bulge,
            _ => 0.0,
        }
    }
}

/// Zerlegt eine EINZELNE Sketch-Primitive in ihre geschlossene Segmentkette
/// (Sketch-2D, CCW-orientiert bei den analytisch konvertierten Formen). `None`
/// für nicht darstellbare Profile (offene Pfade). Booleans im Sketch-Baum
/// (Innenrand/Abzug) sind Phase-2-Scope und werden vom Aufrufer verweigert.
fn profile_segments(prim: &Sketch2dPrimitive) -> Option<Vec<ProfileSeg>> {
    match prim {
        Sketch2dPrimitive::Rectangle { center, half_extents } => {
            let (cx, cy) = (center[0] as f64, center[1] as f64);
            let (hx, hy) = (half_extents[0] as f64, half_extents[1] as f64);
            Some(vec![
                ProfileSeg::Line { start: [cx - hx, cy - hy] },
                ProfileSeg::Line { start: [cx + hx, cy - hy] },
                ProfileSeg::Line { start: [cx + hx, cy + hy] },
                ProfileSeg::Line { start: [cx - hx, cy + hy] },
            ])
        }
        Sketch2dPrimitive::Polygon { vertices } => {
            if vertices.len() < 3 {
                return None;
            }
            Some(vertices.iter().map(|v| ProfileSeg::Line { start: [v[0] as f64, v[1] as f64] }).collect())
        }
        Sketch2dPrimitive::Circle { center, radius } => {
            let (cx, cy, r) = (center[0] as f64, center[1] as f64, *radius as f64);
            // Voller Kreis als zwei CCW-Halbkreise (Bulge = tan(π/4) = 1).
            Some(vec![
                ProfileSeg::Arc { start: [cx + r, cy], bulge: 1.0 },
                ProfileSeg::Arc { start: [cx - r, cy], bulge: 1.0 },
            ])
        }
        Sketch2dPrimitive::Sector { center, radius, start_angle, sweep_angle } => {
            let (cx, cy, r) = (center[0] as f64, center[1] as f64, *radius as f64);
            let (a0, w) = (*start_angle as f64, *sweep_angle as f64);
            let p1 = [cx + r * a0.cos(), cy + r * a0.sin()];
            let p2 = [cx + r * (a0 + w).cos(), cy + r * (a0 + w).sin()];
            // Mittelpunkt → Bogenanfang (Gerade) → Bogenende (Bogen) → zurück (Gerade).
            Some(vec![
                ProfileSeg::Line { start: [cx, cy] },
                ProfileSeg::Arc { start: p1, bulge: (0.25 * w).tan() },
                ProfileSeg::Line { start: p2 },
            ])
        }
        Sketch2dPrimitive::Path { segments, closed } => {
            if !closed || segments.is_empty() {
                return None; // offene Kontur trägt keinen Deckel
            }
            let mut out = Vec::with_capacity(segments.len());
            for seg in segments {
                match seg {
                    SketchSegment::Line { start, .. } => {
                        out.push(ProfileSeg::Line { start: [start[0] as f64, start[1] as f64] });
                    }
                    SketchSegment::Arc { center, radius, start_angle, sweep_angle } => {
                        let s = [
                            center[0] as f64 + *radius as f64 * (*start_angle as f64).cos(),
                            center[1] as f64 + *radius as f64 * (*start_angle as f64).sin(),
                        ];
                        out.push(ProfileSeg::Arc { start: s, bulge: (0.25 * *sweep_angle as f64).tan() });
                    }
                    SketchSegment::QuadBezier { p0, .. } => {
                        out.push(ProfileSeg::Bezier { start: [p0[0] as f64, p0[1] as f64] });
                    }
                }
            }
            Some(out)
        }
    }
}

/// Verallgemeinerung von [`box_contact_faces`] auf alle Leaf-Typen mit
/// analytisch-planaren Facetten (Design §2):
///
/// * **Box** → unverändert `box_contact_faces` (Fast-Path; die sechs Rechtecke,
///   `box_rect = true`).
/// * **Extrude** → zwei Deckel-/Boden-Facetten (Profil-Kontur mit Bulge-Bögen
///   für Arc-Segmente; Rectangle/Circle/Sector/Polygon analytisch, QuadBezier
///   im Profil ⇒ Deckel verweigert) plus je GERADEM Profil-Segment eine
///   Mantel-Rechteck-Facette.
/// * **Revolve/Helix/Sweep/…** → `None` (keine planaren Facetten).
///
/// Alle Extrude-Facetten tragen `box_rect = false` und zwingen damit den
/// cavalier-Verschnitt (die Box-Exaktheit bleibt dem SH-Fast-Path vorbehalten).
fn member_contact_faces(node: &BoundedCsg) -> Option<Vec<ContactFace>> {
    match node {
        BoundedCsg::Leaf { primitive: Primitive::Box { .. }, .. } => box_contact_faces(node),
        BoundedCsg::Extrude { sketch, half_height, world_transform, .. } => {
            extrude_contact_faces(sketch, *half_height as f64, world_transform)
        }
        _ => None,
    }
}

/// Deckel-/Boden- + Mantel-Facetten eines Extrude-Members. Nur EINZEL-Prim-
/// Profile (Booleans im Sketch-Baum = Phase-2-Scope → `None`). Uniforme,
/// positive Skala verlangt (sonst sind Normalen/Dicken nicht mehr metrisch —
/// wie bei der Box).
fn extrude_contact_faces(
    sketch: &BoundedSketch2d,
    half_height: f64,
    world_transform: &Transform,
) -> Option<Vec<ContactFace>> {
    let BoundedSketch2d::Prim { primitive, .. } = sketch else {
        return None; // zusammengesetztes Profil (Innenrand/Abzug) → Phase 2
    };
    let segs = profile_segments(primitive)?;
    if segs.len() < 2 || half_height <= 1e-6 {
        return None;
    }
    let s = world_transform.scale;
    if (s[0] - s[1]).abs() > 1e-4 * s[0].abs().max(1.0)
        || (s[0] - s[2]).abs() > 1e-4 * s[0].abs().max(1.0)
        || s[0] <= 0.0
    {
        return None;
    }
    let scl = s[0] as f64;
    let rot = world_transform.rotation;
    let tr = [
        world_transform.translate[0] as f64,
        world_transform.translate[1] as f64,
        world_transform.translate[2] as f64,
    ];
    let l2w = |p: [f64; 3]| -> [f64; 3] {
        let sp = [(p[0] * scl) as f32, (p[1] * scl) as f32, (p[2] * scl) as f32];
        let r = crate::transform::quat_rotate(rot, sp);
        [r[0] as f64 + tr[0], r[1] as f64 + tr[1], r[2] as f64 + tr[2]]
    };
    let rot_dir = |p: [f64; 3]| -> [f64; 3] {
        let r = crate::transform::quat_rotate(rot, [p[0] as f32, p[1] as f32, p[2] as f32]);
        fnrm([r[0] as f64, r[1] as f64, r[2] as f64])
    };

    let has_bezier = segs.iter().any(|s| matches!(s, ProfileSeg::Bezier { .. }));
    let verts2d: Vec<[f64; 2]> = segs.iter().map(|s| s.start()).collect();
    let bulges2d: Vec<f64> = segs.iter().map(|s| s.bulge()).collect();
    // `bulge = tan(θ/4)` divergiert für Bogen-/Sektor-Sweeps → 2π: ein einzelnes
    // Segment nahe Vollkreis erzeugt einen numerisch entarteten Bulge, der den
    // cavalier-Verschnitt vergiften würde. Solche Profile ehrlich verweigern.
    if bulges2d.iter().any(|b| !b.is_finite() || b.abs() > 1e4) {
        return None;
    }
    let mut out = Vec::new();

    // ── Deckel/Boden: die zwei Cap-Ebenen z = ±half_height ──────────────────
    // Der Sketch-Bulge ist CCW-positiv in (x,y). Die Projektion via
    // plane_frame(n̂) kehrt die Orientierung am Boden-Deckel (n̂ = −Rẑ) um; das
    // gleicht `s·bulge` (s = +1 oben, −1 unten) VORAB aus, sodass der Bulge
    // verbatim in die cavalier-Polyline fließt (siehe `ContactFace`-Doku).
    if !has_bezier {
        for cap_s in [1.0f64, -1.0] {
            let n = rot_dir([0.0, 0.0, cap_s]);
            let loop3: Vec<[f64; 3]> =
                verts2d.iter().map(|p| l2w([p[0], p[1], cap_s * half_height])).collect();
            let bulges: Vec<f64> = bulges2d.iter().map(|b| cap_s * b).collect();
            let o = fdot(n, loop3[0]);
            out.push(ContactFace {
                n,
                o,
                loop3,
                bulges,
                depth: 2.0 * half_height * scl,
                box_rect: false,
            });
        }
    }

    // ── Mantel: je GERADEM Segment ein extrudiertes Rechteck ────────────────
    let prim_sdf = |p: [f64; 2]| -> f64 { primitive.sdf([p[0] as f32, p[1] as f32]) as f64 };
    let nseg = segs.len();
    for k in 0..nseg {
        let ProfileSeg::Line { start: a } = &segs[k] else { continue };
        let a = *a;
        let b = segs[(k + 1) % nseg].start();
        let e = [b[0] - a[0], b[1] - a[1]];
        let el = (e[0] * e[0] + e[1] * e[1]).sqrt();
        if el < 1e-9 {
            continue;
        }
        // In-Ebenen-Außennormale (Richtung wachsender Profil-SDF).
        let mut cand = [e[1] / el, -e[0] / el];
        let mid = [0.5 * (a[0] + b[0]), 0.5 * (a[1] + b[1])];
        let eps = 1e-3 * el.max(1.0);
        let d_plus = prim_sdf([mid[0] + eps * cand[0], mid[1] + eps * cand[1]]);
        let d_minus = prim_sdf([mid[0] - eps * cand[0], mid[1] - eps * cand[1]]);
        if d_plus < d_minus {
            cand = [-cand[0], -cand[1]];
        }
        let n = rot_dir([cand[0], cand[1], 0.0]);
        // Member-Dicke normal zur Wand: Profil-Ausdehnung hinter der Wand.
        let o_face = cand[0] * a[0] + cand[1] * a[1];
        let inner_min = verts2d
            .iter()
            .map(|v| cand[0] * v[0] + cand[1] * v[1])
            .fold(f64::INFINITY, f64::min);
        let depth = ((o_face - inner_min) * scl).max(0.0);
        let loop3 = vec![
            l2w([a[0], a[1], -half_height]),
            l2w([b[0], b[1], -half_height]),
            l2w([b[0], b[1], half_height]),
            l2w([a[0], a[1], half_height]),
        ];
        let o = fdot(n, loop3[0]);
        out.push(ContactFace {
            n,
            o,
            loop3,
            bulges: vec![0.0; 4],
            depth,
            box_rect: false,
        });
    }

    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

// ─── cavalier-Verschnitt (nicht-konvexe / bulge-tragende Kontakte) ───────────

/// Projiziert eine [`ContactFace`] in die Ebene `(e1,e2)` und baut daraus eine
/// CCW-normalisierte `cavalier`-Polyline (Bulge verbatim — die Orientierung ist
/// bereits im Facetten-Bulge eingerechnet).
fn face_to_pline(
    face: &ContactFace,
    e1: [f64; 3],
    e2: [f64; 3],
) -> cavalier_contours::polyline::Polyline<f64> {
    use cavalier_contours::polyline::{PlineCreation, PlineSource, PlineSourceMut, Polyline};
    let mut pl = Polyline::with_capacity(face.loop3.len(), true);
    for (i, w) in face.loop3.iter().enumerate() {
        pl.add(fdot(e1, *w), fdot(e2, *w), face.bulges.get(i).copied().unwrap_or(0.0));
    }
    if pl.area() < 0.0 {
        pl.invert_direction_mut();
    }
    pl
}

/// Konvertiert eine `cavalier`-Polyline in eine [`Contour2d`] und normalisiert
/// die Orientierung auf CCW (positive Fläche).
fn contour_from_pline(pl: &cavalier_contours::polyline::Polyline<f64>) -> Contour2d {
    use cavalier_contours::polyline::{PlineSource, PlineSourceMut};
    let mut pl = pl.clone();
    if pl.area() < 0.0 {
        pl.invert_direction_mut();
    }
    let n = pl.vertex_count();
    let mut verts = Vec::with_capacity(n);
    let mut bulges = Vec::with_capacity(n);
    for i in 0..n {
        let v = pl.at(i);
        verts.push([v.x, v.y]);
        bulges.push(v.bulge);
    }
    Contour2d { verts, bulges }
}

/// Even-Odd-Punkt-in-Kontur (Sehnen-Näherung der Bögen — genügt der
/// Loch-Komponenten-Zuordnung; die Kontur-Geometrie selbst bleibt exakt).
fn point_in_contour(c: &Contour2d, p: [f64; 2]) -> bool {
    let n = c.verts.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let vi = c.verts[i];
        let vj = c.verts[j];
        if (vi[1] > p[1]) != (vj[1] > p[1]) {
            let t = (p[1] - vi[1]) / (vj[1] - vi[1]);
            if p[0] < vi[0] + t * (vj[0] - vi[0]) {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

/// Der 2D-Kontakt-Verschnitt zweier koplanarer Member-Facetten in der
/// gemeinsamen Ebene `(e1,e2)` (Design §3). Box∩Box (`box_rect` bei BEIDEN)
/// läuft metrisch-exakt über Sutherland–Hodgman und liefert EINE konvexe
/// Komponente ohne Loch — byte-gleich zum bisherigen Pfad. Sobald EINE Facette
/// keine Box-Fläche ist, rechnet `cavalier_contours` den booleschen Verschnitt
/// und spaltet ihn in Zusammenhangskomponenten (`pos_plines`) und Löcher
/// (`neg_plines`) auf. Sliver-Komponenten unter `overlap_min²` werden verworfen.
fn clip_faces(
    fa: &ContactFace,
    fb: &ContactFace,
    e1: [f64; 3],
    e2: [f64; 3],
    cleanup_tol: f64,
    overlap_min: f64,
) -> Vec<ClipComponent> {
    let proj = |w: &[f64; 3]| -> [f64; 2] { [fdot(e1, *w), fdot(e2, *w)] };

    if fa.box_rect && fb.box_rect {
        // Konvex∩konvex — der exakte SH-Fast-Path (kein cavalier).
        let mut la: Vec<[f64; 2]> = fa.loop3.iter().map(proj).collect();
        let mut lb: Vec<[f64; 2]> = fb.loop3.iter().map(proj).collect();
        if area2d(&la) < 0.0 {
            la.reverse();
        }
        if area2d(&lb) < 0.0 {
            lb.reverse();
        }
        let mut p2d = cleanup_poly(clip_convex(&la, &lb), cleanup_tol);
        if p2d.len() < 3 || area2d(&p2d).abs() < overlap_min * overlap_min {
            return Vec::new();
        }
        if area2d(&p2d) < 0.0 {
            p2d.reverse();
        }
        return vec![ClipComponent { outer: Contour2d::straight(p2d), holes: Vec::new() }];
    }

    // Nicht-konvex / bulge-tragend → cavalier boolean(And).
    use cavalier_contours::polyline::{BooleanOp, PlineSource};
    let pa = face_to_pline(fa, e1, e2);
    let pb = face_to_pline(fb, e1, e2);
    let res = pa.boolean(&pb, BooleanOp::And);

    // Löcher (neg_plines) auf CCW normalisierte Konturen, mit einem inneren
    // Referenzpunkt für die Komponenten-Zuordnung.
    let holes: Vec<Contour2d> = res
        .neg_plines
        .iter()
        .filter_map(|h| {
            let c = contour_from_pline(&h.pline);
            (c.verts.len() >= 3).then_some(c)
        })
        .collect();

    let mut comps = Vec::new();
    for pos in &res.pos_plines {
        // Sliver-Schwelle über die (bogen-exakte) cavalier-Fläche.
        if pos.pline.area().abs() < overlap_min * overlap_min {
            continue;
        }
        let outer = contour_from_pline(&pos.pline);
        if outer.verts.len() < 3 {
            continue;
        }
        // Löcher der Komponente zuordnen (Referenzpunkt im Loch-Inneren).
        let my_holes: Vec<Contour2d> = holes
            .iter()
            .filter(|h| {
                let c = [
                    h.verts.iter().map(|v| v[0]).sum::<f64>() / h.verts.len() as f64,
                    h.verts.iter().map(|v| v[1]).sum::<f64>() / h.verts.len() as f64,
                ];
                point_in_contour(&outer, c)
            })
            .cloned()
            .collect();
        comps.push(ClipComponent { outer, holes: my_holes });
    }
    comps
}

/// Orthonormale In-Ebenen-Basis (e1, e2) zur Ebenen-Normale n̂ (rechtshändig:
/// e2 = n × e1), plus die Quaternion, die (X,Y,Z) → (e1,e2,n̂) rotiert.
fn plane_frame(n: [f64; 3]) -> ([f64; 3], [f64; 3], [f32; 4]) {
    let n = fnrm(n);
    // Achse mit kleinster |Komponente| als Kreuz-Partner → stabile e1.
    let ax = if n[0].abs() <= n[1].abs() && n[0].abs() <= n[2].abs() {
        [1.0, 0.0, 0.0]
    } else if n[1].abs() <= n[2].abs() {
        [0.0, 1.0, 0.0]
    } else {
        [0.0, 0.0, 1.0]
    };
    let e1 = fnrm(fcross(ax, n));
    let e2 = fnrm(fcross(n, e1));
    (e1, e2, quat_from_cols(e1, e2, n))
}

/// Rotations-Quaternion aus einer rechtshändigen orthonormalen Spaltenbasis
/// (e1,e2,n als Spalten der Rotationsmatrix), Shepperd-Methode.
fn quat_from_cols(e1: [f64; 3], e2: [f64; 3], n: [f64; 3]) -> [f32; 4] {
    let m = [
        [e1[0], e2[0], n[0]],
        [e1[1], e2[1], n[1]],
        [e1[2], e2[2], n[2]],
    ];
    let tr = m[0][0] + m[1][1] + m[2][2];
    let (mut x, mut y, mut z, mut w);
    if tr > 0.0 {
        let s = (tr + 1.0).sqrt() * 2.0;
        w = 0.25 * s;
        x = (m[2][1] - m[1][2]) / s;
        y = (m[0][2] - m[2][0]) / s;
        z = (m[1][0] - m[0][1]) / s;
    } else if m[0][0] > m[1][1] && m[0][0] > m[2][2] {
        let s = (1.0 + m[0][0] - m[1][1] - m[2][2]).sqrt() * 2.0;
        w = (m[2][1] - m[1][2]) / s;
        x = 0.25 * s;
        y = (m[0][1] + m[1][0]) / s;
        z = (m[0][2] + m[2][0]) / s;
    } else if m[1][1] > m[2][2] {
        let s = (1.0 + m[1][1] - m[0][0] - m[2][2]).sqrt() * 2.0;
        w = (m[0][2] - m[2][0]) / s;
        x = (m[0][1] + m[1][0]) / s;
        y = 0.25 * s;
        z = (m[1][2] + m[2][1]) / s;
    } else {
        let s = (1.0 + m[2][2] - m[0][0] - m[1][1]).sqrt() * 2.0;
        w = (m[1][0] - m[0][1]) / s;
        x = (m[0][2] + m[2][0]) / s;
        y = (m[1][2] + m[2][1]) / s;
        z = 0.25 * s;
    }
    let l = (x * x + y * y + z * z + w * w).sqrt().max(1e-30);
    x /= l;
    y /= l;
    z /= l;
    w /= l;
    [x as f32, y as f32, z as f32, w as f32]
}

/// 2D-Signed-Area (Shoelace).
fn area2d(poly: &[[f64; 2]]) -> f64 {
    let n = poly.len();
    if n < 3 {
        return 0.0;
    }
    let mut a = 0.0;
    for i in 0..n {
        let j = (i + 1) % n;
        a += poly[i][0] * poly[j][1] - poly[j][0] * poly[i][1];
    }
    0.5 * a
}

/// Schnitt der Geraden `p1p2` mit der Geraden `ab` (für Sutherland–Hodgman).
fn line_intersect(p1: [f64; 2], p2: [f64; 2], a: [f64; 2], b: [f64; 2]) -> Option<[f64; 2]> {
    let r = [p2[0] - p1[0], p2[1] - p1[1]];
    let s = [b[0] - a[0], b[1] - a[1]];
    let denom = r[0] * s[1] - r[1] * s[0];
    if denom.abs() < 1e-12 {
        return None;
    }
    let t = ((a[0] - p1[0]) * s[1] - (a[1] - p1[1]) * s[0]) / denom;
    Some([p1[0] + t * r[0], p1[1] + t * r[1]])
}

/// Sutherland–Hodgman: `subject` an der KONVEXEN `clip` (CCW) klippen.
/// Für konvexe Rechtecke ist der Verschnitt metrisch exakt.
fn clip_convex(subject: &[[f64; 2]], clip: &[[f64; 2]]) -> Vec<[f64; 2]> {
    let mut output = subject.to_vec();
    let n = clip.len();
    for e in 0..n {
        if output.is_empty() {
            break;
        }
        let a = clip[e];
        let b = clip[(e + 1) % n];
        let edge = [b[0] - a[0], b[1] - a[1]];
        // Innen = links der CCW-Kante (Kreuzprodukt ≥ 0).
        let inside = |p: [f64; 2]| edge[0] * (p[1] - a[1]) - edge[1] * (p[0] - a[0]) >= -1e-9;
        let input = std::mem::take(&mut output);
        let m = input.len();
        for k in 0..m {
            let cur = input[k];
            let prev = input[(k + m - 1) % m];
            let cur_in = inside(cur);
            let prev_in = inside(prev);
            if cur_in {
                if !prev_in {
                    if let Some(ip) = line_intersect(prev, cur, a, b) {
                        output.push(ip);
                    }
                }
                output.push(cur);
            } else if prev_in {
                if let Some(ip) = line_intersect(prev, cur, a, b) {
                    output.push(ip);
                }
            }
        }
    }
    output
}

/// Aufeinanderfolgende Duplikate + kollineare Zwischenpunkte entfernen.
fn cleanup_poly(mut poly: Vec<[f64; 2]>, tol: f64) -> Vec<[f64; 2]> {
    // Duplikate.
    poly.dedup_by(|a, b| (a[0] - b[0]).abs() < tol && (a[1] - b[1]).abs() < tol);
    if poly.len() >= 2 {
        let f = poly[0];
        let l = *poly.last().unwrap();
        if (f[0] - l[0]).abs() < tol && (f[1] - l[1]).abs() < tol {
            poly.pop();
        }
    }
    if poly.len() < 3 {
        return poly;
    }
    // Kollineare Ecken.
    let mut out: Vec<[f64; 2]> = Vec::with_capacity(poly.len());
    let n = poly.len();
    for i in 0..n {
        let p = poly[(i + n - 1) % n];
        let q = poly[i];
        let r = poly[(i + 1) % n];
        let cr = (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0]);
        if cr.abs() > tol * tol {
            out.push(q);
        }
    }
    if out.len() < 3 {
        poly
    } else {
        out
    }
}

/// Ergebnis der Kontakt-Prisma-Synthese eines Stopps.
struct ContactResult {
    /// Kontakt-Prismen (als reguläre `Extrude`-Member in den Baum zu hängen).
    prisms: Vec<BoundedCsg>,
    /// `true`, wenn P bei ALLEN gebauten Prismen dem vollen Querschnitt beider
    /// Member entspricht (S = U) → metrisch exakt, keine Toleranz nötig.
    exact: bool,
    /// Eine koplanare Kontaktfläche wurde erkannt, die NICHT exakt korrigierbar
    /// war (Nicht-Box-Member, degenerierter Verschnitt) → conservative bleibt.
    refused: bool,
    /// Größte verbleibende Rand-Sliver-Schranke (B0-Klasse, ≈0.75·δ) über alle
    /// nicht-exakten Prismen — die ehrliche Toleranz von Stufe 1 (Stufe 2 offen).
    rim_bound: f32,
    /// Member-Paare (i,j) mit einem Kontakt-Prisma — für sie unterdrückt die
    /// SSI-Naht-Synthese ihre (degenerierten) Kanäle. Enthält **nur bündige**
    /// Kontakte: an einem DURCHDRINGENDEN Kontakt ist die Naht am Kontaktrand
    /// eine echte transversale Dieder-Kante (kein degenerierter koinzidenter
    /// Carrier), die die SSI-Kanäle korrekt korrigieren — sie darf nicht
    /// unterdrückt werden (#384-Penetrations-Befund).
    pairs: Vec<(usize, usize)>,
    /// Mindestens ein Prisma stammt aus einem DURCHDRINGENDEN Kontakt (s > 0)
    /// mit ungleichem Querschnitt. `contact_clips` (die Rim-Spiegel-Quelle)
    /// nimmt nur bündige Kontakte auf, also schließt kein ∂P-Rand-Fillet
    /// dessen Saum → die Toleranzzusage muss die weitere B0-Schranke nennen
    /// statt der engen `channel_tol` eines bündigen Nachbar-Kontakts.
    pen_unrimmed: bool,
}

/// Signierte Lage zweier anti-paralleler Member-Facetten zueinander (#384,
/// Penetrations-Befund).
///
/// `fa.n = −fb.n =: n̂`. Member i liegt auf `n̂·x ≤ o_a`, Member j auf
/// `n̂·x ≥ −o_b`. Damit ist
///
/// ```text
///     s := o_a + o_b
/// ```
///
/// die **signierte Durchdringungstiefe**: `s = 0` bündiger Kontakt, `s > 0`
/// die zwei Member durchdringen sich um `s`, `s < 0` es klafft ein Luftspalt
/// der Weite `−s`. Vor #384 testete die Synthese nur `|s| ≤ 0.5·δ` — ein
/// **unsignierter** Fenstertest, der alle drei Fälle vermengte und dabei drei
/// Falschaussagen erzeugte (jeweils unter dem Stempel `exact`):
///
/// * `s < 0` (Luftspalt bis 0.5·δ): das Prisma **überbrückte die Luft** und
///   erfand Material, wo die wahre Erosion leer ist (gemessen: Spalt 0.1 mm
///   bei δ=2 ⇒ 8 mm Phantom-Material).
/// * `0 < s ≤ 0.5·δ`: das Prisma war an EINER Ebene (`o_a`) verankert und
///   ragte darum um genau `s` über die Gegenseite hinaus (gemessen: 1 mm
///   Phantom-Material über der wahren Erosionsfläche).
/// * `0.5·δ < s < 2·δ`: gar kein Prisma — die naive Erosion
///   `min(f_i) + δ` riss einen **Phantom-Schlitz** der Dicke `2δ − s`
///   (gemessen bis 1.47 mm Nullmengenfehler bei δ=2, ≈0.735·δ).
///
/// Diese Funktion trennt die Fälle. Rückgabe:
///
/// * `None` — kein Kontakt: echter Luftspalt (`s < −round_tol`) ODER
///   `s ≥ 2·δ` (die erodierten Member überlappen noch, es entsteht gar kein
///   Schlitz → keine Korrektur nötig, das naive Feld ist dort zero-set-exakt).
/// * `Some(pen)` — Kontakt mit Durchdringung `pen ≥ 0`; `pen == 0` ist der
///   bündige Fall (bit-identisch zum Vor-#384-Pfad).
///
/// `round_tol` ist die f32-Rundungsklasse (ein ECHT bündiger Kontakt hat
/// |s| ~1e-4; genutzt wird das bereits kalibrierte `0.02·δ`-Fenster aus der
/// #397-Dünnwand-Abgrenzung). NUR innerhalb dieser Klasse wird `pen` auf 0
/// geklemmt — damit bleiben echte bündige Kontakte byte-identisch, während
/// jede messbare Durchdringung ihre wahre Tiefe behält. Das vorherige
/// Box-Fenster `0.5·δ` als Klemme wäre falsch: es klemmte auch eine
/// Durchdringung von 1 mm auf „bündig" und erzeugte genau den zweiten Befund
/// oben.
fn face_pair_penetration(s: f64, delta: f64, round_tol: f64) -> Option<f64> {
    if s < -round_tol {
        return None; // echter Luftspalt → die Member berühren sich nicht
    }
    if s >= 2.0 * delta {
        return None; // kein Phantom-Schlitz → keine Korrektur nötig
    }
    if s <= round_tol {
        return Some(0.0); // bündig (Rundungsklasse) — Bestandspfad
    }
    Some(s)
}

/// Ergebnis des reinen Facetten-Scans eines Stopps (#384) — die analytische
/// Vorab-Antwort auf „gibt es hier überhaupt einen Kontakt?".
struct ContactScan {
    /// Mindestens ein Facetten-Paar ist ein Kontakt-Kandidat (bündig ODER
    /// durchdringend, `face_pair_penetration` liefert `Some`).
    candidate: bool,
    /// Mindestens ein Kandidat ist DURCHDRINGEND (`pen > 0`).
    penetrating: bool,
    /// Für JEDEN Member sind die planaren Facetten analytisch bekannt. Nur dann
    /// ist `!candidate` ein BEWEIS für „kein koplanarer Kontakt" (sonst fehlen
    /// dem Scan schlicht Flächen — gekrümmte Member, Revolve/Sweep/Mesh).
    complete: bool,
}

/// Scannt die planaren Member-Facetten eines Stopps mit exakt dem Kriterium der
/// Prisma-Synthese (`face_pair_penetration`). Zwei Verwendungen in
/// `synthesize_stop`:
///
/// * `penetrating` ist der ADDITIVE Trigger für den Prisma-Pfad — das
///   Carrier-basierte `coplanar_interface_present`-Gate sieht durchdringende
///   Kontakte nicht (es verlangt überlappende Facetten-AABBs, und bei
///   Durchdringung liegen die zwei Ebenen um `s` auseinander).
/// * `complete && !candidate` WIDERLEGT das Gate: alle Facetten sind bekannt
///   und kein Paar ist in Kontakt (jedes hat einen echten Luftspalt oder
///   überlappt tiefer als 2·δ) → die #394-Verdachtsmeldung wäre ein Fehlalarm.
fn contact_face_scan(members: &[Member], delta: f64) -> ContactScan {
    let round_tol = (0.02 * delta).max(1e-3);
    let faces: Vec<Option<Vec<ContactFace>>> =
        members.iter().map(|m| member_contact_faces(m.node)).collect();
    let mut scan = ContactScan {
        candidate: false,
        penetrating: false,
        complete: faces.iter().all(|f| f.is_some()),
    };
    for i in 0..members.len() {
        for j in (i + 1)..members.len() {
            let (Some(fi), Some(fj)) = (&faces[i], &faces[j]) else {
                continue;
            };
            for fa in fi {
                for fb in fj {
                    if fdot(fa.n, fb.n) > -0.9995 {
                        continue;
                    }
                    if let Some(pen) = face_pair_penetration(fa.o + fb.o, delta, round_tol) {
                        scan.candidate = true;
                        scan.penetrating |= pen > 0.0;
                    }
                }
            }
        }
    }
    scan
}

/// Baut die Kontakt-Prismen für die koplanaren Kontaktflächen zweier Union-
/// Member unter Erosion (Design §3, Stufe 1). Verweigert (setzt `refused`) für
/// gekrümmte/nicht-Box-Kontakte oder degenerierte Verschnitte — dort bleibt
/// #394-conservative. Der ∂P-Rand-Sliver (Stufe 2) bleibt als `rim_bound`
/// gemeldete Toleranz stehen (Rückfall-Klausel #395).
fn synthesize_contact_prisms(
    members: &[Member],
    ctx: &StopCtx,
    min_feat: &[u32],
    prism_idx: &mut u32,
) -> ContactResult {
    let mut res = ContactResult {
        prisms: Vec::new(),
        exact: true,
        refused: false,
        rim_bound: 0.0,
        pairs: Vec::new(),
        pen_unrimmed: false,
    };
    let delta = ctx.delta as f64;
    // #397-Dünnwand-Abgrenzung / #384-Rundungsklasse: ein echter submergierter
    // Flush-Kontakt ist bis auf f32-Rundung koinzident (~1e-4), eine bündig
    // gestapelte Wand der Stärke w unterscheidet sich um genau w. Seit #384 ist
    // das für BEIDE Facettenklassen (Box wie Extrude) die Klemme auf „bündig" —
    // jede messbare Abweichung wird signiert als Durchdringung (s>0) bzw.
    // Luftspalt (s<0) behandelt, statt sie in ein 0.5·δ-Fenster zu falten.
    let coincide_eff = (0.02 * delta).max(1e-3);
    let overlap_min = (0.5 * delta).max(1e-3);
    let clip_tol = (1e-3 * delta).max(1e-4);

    // FC Phase 2 (#398): verallgemeinerte Member-Facetten — Box-Fast-Path
    // (`box_rect = true`) PLUS Extrude-Cap/-Mantel-Konturen (`box_rect = false`).
    // Ein Member ohne analytisch-planare Facetten (Revolve/Sweep/Mesh) liefert
    // `None`; seine Kontakte werden hier nicht erfasst (die Kanal-/Capability-
    // Ehrlichkeit greift beim Aufrufer). Der Box-Box-Fall läuft über den
    // metrisch-exakten SH-Fast-Path in `clip_faces` (byte-identisches Tape).
    let faces: Vec<Option<Vec<ContactFace>>> =
        members.iter().map(|m| member_contact_faces(m.node)).collect();

    for i in 0..members.len() {
        for j in (i + 1)..members.len() {
            let (Some(fi), Some(fj)) = (&faces[i], &faces[j]) else {
                continue;
            };
            for fa in fi {
                for fb in fj {
                    // Anti-parallel + signierte Lage (#384): bündig, durch-
                    // dringend oder Luftspalt — `face_pair_penetration` trennt
                    // die drei Fälle. Bündig heißt jetzt „innerhalb der
                    // f32-Rundungsklasse `coincide_eff`" (nicht mehr „innerhalb
                    // 0.5·δ"), Durchdringung bis 2·δ trägt ihre wahre Tiefe.
                    if fdot(fa.n, fb.n) > -0.9995 {
                        continue;
                    }
                    let Some(pen) = face_pair_penetration(fa.o + fb.o, delta, coincide_eff) else {
                        continue;
                    };
                    let n = fnrm(fa.n); // Member i liegt auf −n, Member j auf +n.
                    let (e1, e2, _) = plane_frame(n);
                    // Der Verschnitt: Box∩Box exakt via SH, sonst cavalier boolean(And)
                    // mit Aufspaltung in Zusammenhangskomponenten + Löcher (§3).
                    let comps = clip_faces(fa, fb, e1, e2, clip_tol, overlap_min);
                    if comps.is_empty() {
                        continue; // kein flächiger Verschnitt / zu klein
                    }
                    // Prisma bauen: Member i-Dicke auf −n, Member j-Dicke auf +n.
                    let o = fa.o;
                    // Volle Facetten-Flächen für die S = U-Exaktheitsprüfung.
                    let proj = |w: &[f64; 3]| -> [f64; 2] { [fdot(e1, *w), fdot(e2, *w)] };
                    let area_a = area2d(&fa.loop3.iter().map(proj).collect::<Vec<_>>()).abs();
                    let area_b = area2d(&fb.loop3.iter().map(proj).collect::<Vec<_>>()).abs();
                    // Provenance-Owner (kleinste Feature-Id) fließt in die
                    // synthetische Prisma-Feature-Id ein (Owner-tragendes Band,
                    // s. `encode_prism_feat`).
                    let owner = min_feat[i].min(min_feat[j]);
                    // Prisma-Ausdehnung entlang n̂: von Member i's Rückseite
                    // (`o − depth_neg`) bis zu Member j's Rückseite. Die liegt
                    // bei `(−o_b) + depth_pos = o − pen + depth_pos`, ALSO ist
                    // die effektive +n̂-Tiefe `depth_pos − pen` (#384). Bei
                    // bündigem Kontakt (`pen == 0`) ist das bit-identisch zum
                    // Bestandspfad; bei Durchdringung endet das Prisma jetzt
                    // exakt an der Gegenseite statt um `pen` darüber hinaus.
                    // Wird j vollständig von i's Slab verschluckt
                    // (`depth_pos ≤ pen`), bleibt das Prisma i's Slab über P —
                    // immer noch S ⊆ U, nur ohne Korrekturwirkung.
                    let depth_pos = (fb.depth - pen).max(0.0);
                    for comp in &comps {
                        let feat = encode_prism_feat(owner, *prism_idx);
                        let Some(prism) =
                            build_prism_component(comp, e1, e2, n, o, fa.depth, depth_pos, feat)
                        else {
                            // Bogen-Kontur (P3) → nicht als Polygon baubar → ehrlich
                            // conservative (der flächige Rest bliebe sonst still).
                            res.refused = true;
                            continue;
                        };
                        // Beweistragende Eigenschaft S ⊆ U stichprobenartig prüfen
                        // (Design §4: das Prisma darf über keine schräge Nachbar-
                        // fläche hinausragen). Verletzt → verwerfen, conservative.
                        let guard = (2e-3 * delta as f32).max(1e-3);
                        if !prism_subset_of_union(&prism, members, guard) {
                            res.refused = true;
                            continue;
                        }
                        *prism_idx += 1;
                        res.prisms.push(prism);
                        // Diese Member sind koplanar korrigiert — die SSI-Naht-
                        // Synthese über genau dieses Paar liefert nur die
                        // degenerierten/spurious Nähte, die #394 überhaupt erst
                        // zur Verweigerung zwangen (koinzidente/tangentiale
                        // Carrier). Sie wird für das Paar unterdrückt (Design §3.4:
                        // der Kontakt gehört dem Prisma + ∂P-Rim, nicht der SSI).
                        //
                        // NUR für BÜNDIGE Kontakte (#384): durchdringen sich die
                        // Member, ist die Naht am Kontaktrand eine echte,
                        // transversale Dieder-Kante (Deckfläche des einen ×
                        // Seitenwand des anderen, Normalen 90° statt koinzident).
                        // Die korrigiert die SSI-Kanal-Synthese sauber — sie hier
                        // zu unterdrücken nähme dem Stopp seine einzige
                        // ∂P-Korrektur (der ∂P-Rim spiegelt nur bündige Kontakte).
                        if pen <= 0.0 && !res.pairs.contains(&(i, j)) {
                            res.pairs.push((i, j));
                        }

                        // Gleicher Querschnitt (P = beide Flächen, EINE Komponente,
                        // kein Loch) ⇒ S = U ⇒ exakt; sonst bleibt der ∂P-Sliver
                        // (Stufe 2 / Toleranz). Mehr-Komponenten- / Loch- /
                        // nicht-konvexe Kontakte sind nie exakt (der ∂P-Saum ist
                        // reicher).
                        let ap = area2d(&comp.outer.verts).abs();
                        let exact_pair = comps.len() == 1
                            && comp.holes.is_empty()
                            && (ap - area_a).abs() < 1e-3 * area_a.max(1.0)
                            && (ap - area_b).abs() < 1e-3 * area_b.max(1.0);
                        if !exact_pair {
                            res.exact = false;
                            if pen > 0.0 {
                                // Durchdringender Kontakt mit ungleichem
                                // Querschnitt: `contact_clips` (Rim-Quelle)
                                // nimmt nur bündige Kontakte auf, hier schließt
                                // also kein ∂P-Rand-Fillet den Saum.
                                res.pen_unrimmed = true;
                            }
                            // Der verbleibende Fehler ist AUSSCHLIESSLICH der
                            // konkave B0-Sliver am Kontaktrand ∂P (Kanten
                            // δ·(1/sin(α/2)−1) ≈ 0.41·δ bei 90°-Stufen, Ecken
                            // (√3−1)·δ ≈ 0.73·δ). Stufe 2 (∂P-Rand-Fillet) schließt
                            // die GERADEN ∂P-Kanten (auch nicht-konvexer Kontakte,
                            // FC Phase 3); an Reflex-Vertices/Bögen bleibt der
                            // ehrliche B0-Rest stehen (Design §5).
                            res.rim_bound = res.rim_bound.max(0.75 * ctx.delta);
                        }
                    }
                }
            }
        }
    }
    res
}

// ─── Kontakt-Prisma-Provenance (Owner-tragendes Feature-Id-Schema) ──────
//
// Synthetische Kontakt-Prismen (`build_prism_member`/`build_prism_contour`)
// werden als `CsgNode::Extrude` mit einer eigenen `FeatureId` eingehängt. Diese
// Feature-Id ist KEINE echte Nutzer-Feature-Id — der Offset/Shell-Term, der das
// Prisma synthetisiert, trägt körperweise keine Feature-Id (Plan §9). Nach dem
// Owner-Muster der Vertex-Kugeln/Kanäle (`(owner << 12) | sub`, sub ∈ 0x700+)
// führen wir stattdessen den bereits berechneten **Owner** (kleinste beteiligte
// Eltern-Member-Feature-Id) verlustfrei durch die Synthese.
//
// Die Feature-Id wird beim Branch-Enumerieren des Extrude in ihr eigenes Feld
// der Branch-Id gehoben (`BranchId::FEATURE_SHIFT`), also MUSS sie in 20 Bit
// passen (sonst aliast sie im Feature-Feld). Bit-Layout (20 Bit):
//
//   bit 19    = 1              PRISM_MARKER — reserviert die obere Hälfte des
//                              20-Bit-Raums für synthetische Prismen. Echte
//                              numerische Feature-Ids sind kleine Dokument-
//                              Zähler (weit unter 0x80000); ein Dokument, das
//                              je dorthin zählt, sprengt vorher die BranchId.
//   bit 18    = HOLE_FLAG      0 = Außen-Prisma, 1 = Loch-Prisma (Difference-
//                              Kind). Hält Außen- und Loch-Ids unterscheidbar.
//   bits 17:6 = owner          12 Bit → 0..4094; 0xFFF ist Sentinel für „Owner
//                              nicht kodierbar" (≥ 4095) und dekodiert zu None —
//                              NIE zu einer aliased fremden Feature-Id. UNIFORM
//                              bei Außen & Loch, damit `prism_owner` beide
//                              gleich dekodiert.
//   bits 5:0  = disc           Außen: laufender Prisma-Index (seq).
//                              Loch:  (seq_low << 2) | hole_idx (4+2 Bit).
//
// Kollisionsfreiheit: echte Feature-Ids haben Bit 19 = 0 (Marker aus); die
// 0x700+-Sketch-Join-Sub-Ids (`offset_fold::JOIN_ARC_SUB_BASE`) leben im
// SUB-Feld anderer Branches, nie im Feature-Id-Feld eines Prismas.
const PRISM_MARKER: u32 = 0x8_0000; // Bit 19
const PRISM_HOLE_FLAG: u32 = 0x4_0000; // Bit 18
const PRISM_OWNER_SHIFT: u32 = 6;
const PRISM_OWNER_MASK: u32 = 0xFFF; // 12 Bit
const PRISM_DISC_MASK: u32 = 0x3F; // 6 Bit
const PRISM_OWNER_UNKNOWN: u32 = 0xFFF; // Sentinel: Owner ≥ 4095, nicht kodierbar

/// Klemmt einen Owner ins 12-Bit-Feld: passt er nicht, wird der Sentinel
/// gespeichert (dekodiert zu `None`) statt per Masken-Kappung zu einer
/// FALSCHEN fremden Feature-Id zu aliasen. Provenance ist feldneutrale
/// Metadaten — Verlust ist erlaubt, Fehlattribution nicht.
#[inline]
fn clamp_prism_owner(owner: u32) -> u32 {
    if owner >= PRISM_OWNER_UNKNOWN { PRISM_OWNER_UNKNOWN } else { owner }
}

/// Kodiert die Feature-Id eines Außen-Kontakt-Prismas: `owner` + laufender
/// Index `seq` (in der Praxis sind Kontakt-Owner niedrig nummerierte
/// Basis-Features, der Sentinel-Pfad ist theoretisch).
#[inline]
fn encode_prism_feat(owner: u32, seq: u32) -> u32 {
    PRISM_MARKER | (clamp_prism_owner(owner) << PRISM_OWNER_SHIFT) | (seq & PRISM_DISC_MASK)
}

/// Kodiert die Feature-Id eines Loch-Prismas — gleicher `owner`, HOLE_FLAG
/// gesetzt, Disambiguator aus dem niederen Teil des Eltern-`seq` plus
/// `hole_idx` (0..3).
#[inline]
fn encode_prism_hole_feat(owner: u32, seq: u32, hole_idx: u32) -> u32 {
    PRISM_MARKER
        | PRISM_HOLE_FLAG
        | (clamp_prism_owner(owner) << PRISM_OWNER_SHIFT)
        | (((seq & 0xF) << 2) | (hole_idx & 0x3))
}

/// True, wenn `feat` (eine flache Feature-Id) ein synthetisches Kontakt-Prisma
/// benennt — Außen ODER Loch.
#[inline]
pub fn is_prism_feat(feat: u32) -> bool {
    feat & PRISM_MARKER != 0
}

/// True, wenn `feat` ein Loch-Prisma (Difference-Kind) benennt.
#[inline]
pub fn is_prism_hole_feat(feat: u32) -> bool {
    is_prism_feat(feat) && (feat & PRISM_HOLE_FLAG != 0)
}

/// Gewinnt den Owner (kleinste beteiligte Eltern-Member-Feature-Id) aus einer
/// Prisma-Feature-Id zurück. `None`, wenn `feat` kein Prisma ist ODER der Owner
/// beim Kodieren nicht ins 12-Bit-Feld passte (Sentinel). Für Picking/
/// Provenance: eine gepickte Prisma-Fläche löst zweistufig auf — primär das
/// Owner-Feature (dieses Ergebnis), sekundär die Eltern-Flächen im eps-Band.
#[inline]
pub fn prism_owner(feat: u32) -> Option<u32> {
    if !is_prism_feat(feat) {
        return None;
    }
    let ow = (feat >> PRISM_OWNER_SHIFT) & PRISM_OWNER_MASK;
    if ow == PRISM_OWNER_UNKNOWN { None } else { Some(ow) }
}

/// Baut einen Kontakt-Prisma-Member `Extrude(P)` mit Π-orientiertem
/// `world_transform` (lokale Z → n̂). `depth_neg`/`depth_pos` sind die
/// Member-Dicken auf der −n̂- bzw. +n̂-Seite von Π (`n·x = o`).
fn build_prism_member(
    p2d: &[[f64; 2]],
    _e1: [f64; 3],
    _e2: [f64; 3],
    n: [f64; 3],
    o: f64,
    depth_neg: f64,
    depth_pos: f64,
    feat: u32,
) -> Option<BoundedCsg> {
    if p2d.len() < 3 {
        return None;
    }
    let half_h = 0.5 * (depth_neg + depth_pos);
    let center_n = o + 0.5 * (depth_pos - depth_neg);
    if half_h <= 1e-6 {
        return None;
    }
    let verts: Vec<[f32; 2]> = p2d.iter().map(|q| [q[0] as f32, q[1] as f32]).collect();
    let sketch = Sketch2d::Prim(Sketch2dPrimitive::Polygon { vertices: verts });
    let ext = CsgNode::Extrude {
        id: FeatureId::from_u32(feat),
        sketch: Box::new(sketch),
        half_height: half_h as f32,
        wall_subs: None,
    };
    let (_, _, quat) = plane_frame(n);
    let carry = Transform {
        translate: [(center_n * n[0]) as f32, (center_n * n[1]) as f32, (center_n * n[2]) as f32],
        rotation: quat,
        scale: [1.0; 3],
    };
    Some(BoundedCsg::from_csg_with_carry(&ext, &carry))
}

/// Baut das Prisma EINER Kontur — `Extrude(Polygon)` bei rein geraden Kanten
/// (byte-identisch zu [`build_prism_member`]), `Extrude(Path)` bei Bögen (FC
/// Phase 4, #398). Der Path trägt die cavalier-Bulges verlustfrei als exakte
/// `SketchSegment::Arc`-Bögen.
fn build_prism_contour(
    contour: &Contour2d,
    e1: [f64; 3],
    e2: [f64; 3],
    n: [f64; 3],
    o: f64,
    depth_neg: f64,
    depth_pos: f64,
    feat: u32,
) -> Option<BoundedCsg> {
    if !contour.has_arcs() {
        return build_prism_member(&contour.verts, e1, e2, n, o, depth_neg, depth_pos, feat);
    }
    let verts = &contour.verts;
    if verts.len() < 3 {
        return None;
    }
    let half_h = 0.5 * (depth_neg + depth_pos);
    let center_n = o + 0.5 * (depth_pos - depth_neg);
    if half_h <= 1e-6 {
        return None;
    }
    // Kontur → geschlossene SketchSegment-Kette: Bulge ≠ 0 → exakter Arc, sonst Line.
    let m = verts.len();
    let mut segments: Vec<SketchSegment> = Vec::with_capacity(m);
    for i in 0..m {
        let a = verts[i];
        let b = verts[(i + 1) % m];
        let bulge = contour.bulges.get(i).copied().unwrap_or(0.0);
        if bulge.abs() < 1e-12 {
            segments.push(SketchSegment::Line {
                start: [a[0] as f32, a[1] as f32],
                end: [b[0] as f32, b[1] as f32],
            });
        } else {
            let chord = [b[0] - a[0], b[1] - a[1]];
            let clen = (chord[0] * chord[0] + chord[1] * chord[1]).sqrt();
            if clen < 1e-9 {
                continue;
            }
            let ab = bulge.abs();
            let radius = clen * (ab * ab + 1.0) / (4.0 * ab);
            let s_sag = ab * clen / 2.0;
            let mm = radius - s_sag;
            let (mut ox, mut oy) = (-mm * chord[1] / clen, mm * chord[0] / clen);
            if bulge < 0.0 {
                ox = -ox;
                oy = -oy;
            }
            let cx = a[0] + chord[0] / 2.0 + ox;
            let cy = a[1] + chord[1] / 2.0 + oy;
            let start_angle = (a[1] - cy).atan2(a[0] - cx);
            let sweep_angle = 4.0 * bulge.atan(); // signiert (cavalier-Konvention)
            segments.push(SketchSegment::Arc {
                center: [cx as f32, cy as f32],
                radius: radius as f32,
                start_angle: start_angle as f32,
                sweep_angle: sweep_angle as f32,
            });
        }
    }
    if segments.len() < 2 {
        return None;
    }
    let sketch = Sketch2d::Prim(Sketch2dPrimitive::Path { segments, closed: true });
    let ext = CsgNode::Extrude {
        id: FeatureId::from_u32(feat),
        sketch: Box::new(sketch),
        half_height: half_h as f32,
        wall_subs: None,
    };
    let (_, _, quat) = plane_frame(n);
    let carry = Transform {
        translate: [(center_n * n[0]) as f32, (center_n * n[1]) as f32, (center_n * n[2]) as f32],
        rotation: quat,
        scale: [1.0; 3],
    };
    let _ = (e1, e2);
    Some(BoundedCsg::from_csg_with_carry(&ext, &carry))
}

/// FC Phase 2 (#398): baut das Kontakt-Prisma EINER [`ClipComponent`] (Design §3).
/// Der Außenrand wird ein `Extrude(Polygon)`-Member (via [`build_prism_member`]),
/// jedes Loch als `Difference{ outer, hole }` (identisches `world_transform`, volle
/// z-Durchdringung) abgezogen. Für eine loch- und bogenfreie Komponente ist das
/// Ergebnis **byte-identisch** zu [`build_prism_member`] am alten Ein-Polygon-P
/// (Golden-Gate). Trägt die Kontur echte Bögen (`bulge ≠ 0`), kann das
/// Polygon-/Path-Tape sie in Phase 2 NICHT verlustfrei führen → ehrliche
/// Verweigerung (`None`, KEINE Sehnen-Näherung; Bogen-Konturen sind P3-Scope,
/// Design §2/§8).
fn build_prism_component(
    comp: &ClipComponent,
    e1: [f64; 3],
    e2: [f64; 3],
    n: [f64; 3],
    o: f64,
    depth_neg: f64,
    depth_pos: f64,
    feat: u32,
) -> Option<BoundedCsg> {
    // FC Phase 4 (#398): bogen-tragende Konturen werden als `Extrude(Path)`-
    // Prisma gebaut (exakte Bogen-Distanz via OP_PRIM2D_PATH — in ALLEN
    // Evaluator-Spiegeln vorhanden, es ist ein Nutzer-Primitiv). Rein gerade
    // Konturen bleiben `Extrude(Polygon)` (byte-identisch zum Bestand).
    let outer = build_prism_contour(&comp.outer, e1, e2, n, o, depth_neg, depth_pos, feat)?;
    if comp.holes.is_empty() {
        return Some(outer);
    }
    let mut node = outer;
    for (hi, hole) in comp.holes.iter().enumerate() {
        // Loch-Feature-Id: gleicher Owner wie das Außen-Prisma, aber HOLE_FLAG
        // gesetzt (unterscheidbar). Das ROHE Owner-Feld durchreichen (nicht
        // `prism_owner`-dekodieren): so propagiert auch der Sentinel korrekt,
        // statt zu Owner 0 zu kollabieren. `seq` = Disc des Außen-Prismas.
        let owner = (feat >> PRISM_OWNER_SHIFT) & PRISM_OWNER_MASK;
        let hfeat = encode_prism_hole_feat(owner, feat & PRISM_DISC_MASK, hi as u32);
        let Some(hole_prism) =
            build_prism_contour(hole, e1, e2, n, o, depth_neg, depth_pos, hfeat)
        else {
            continue;
        };
        let bb = node.aabb(); // Difference schrumpft → linke AABB.
        node = BoundedCsg::Difference {
            left: Box::new(node),
            right: Box::new(hole_prism),
            aabb: bb,
        };
    }
    Some(node)
}

/// Stichproben-Prüfung S ⊆ U: an einem groben Gitter über der Prisma-AABB
/// darf kein Punkt mit `f_S < −guard` außerhalb der Union (`min f_i > guard`)
/// liegen. (Design §4-Risiko: schräge Nachbarfläche.)
fn prism_subset_of_union(prism: &BoundedCsg, members: &[Member], guard: f32) -> bool {
    let bb = prism.aabb();
    if bb.is_empty() {
        return true;
    }
    let n = 6;
    for ix in 0..=n {
        for iy in 0..=n {
            for iz in 0..=n {
                let p = [
                    bb.min[0] + (bb.max[0] - bb.min[0]) * ix as f32 / n as f32,
                    bb.min[1] + (bb.max[1] - bb.min[1]) * iy as f32 / n as f32,
                    bb.min[2] + (bb.max[2] - bb.min[2]) * iz as f32 / n as f32,
                ];
                let fs = prism.sdf(p, EvalMode::Visual);
                if fs < -guard {
                    let fu = members
                        .iter()
                        .map(|m| m.value(p))
                        .fold(f32::INFINITY, f32::min);
                    if fu > guard {
                        return false;
                    }
                }
            }
        }
    }
    true
}

// ─── F395 Stufe 3: Kontakt-Prisma für den DILATIONS-Fall (Komplement-Dualität) ─
//
// Dilation `+δ` über einer Intersection/Difference mit koplanarer submergierter
// Kontaktfläche: `max(f_i) − δ` legt einen Phantom-Fin über das Kontaktpolygon P
// (dual zum Erosions-Schlitz). Per Komplement-Dualität ist das die Stufe-1-
// Situation im Komplement: die inneren Constraints `max(s_i·f_i)` haben das
// Komplement `min(−s_i·f_i)` (Union von Komplementen), und Stufe 1 hängt dort ein
// Kontakt-Prisma S als zusätzlichen Union-Member. Rück-negiert heißt das:
//
//     f_korr(p) = max(f_i, −f_S) − δ   (Prisma als SCHNITT-Constraint subtrahiert)
//
// Das ist exakt `Difference{ inner, S }` (Feld `max(f_inner, −f_S)`) VOR dem `+δ`
// — KEIN neuer Opcode, alle fünf Evaluator-Spiegel sehen ein gewöhnliches Extrude
// als Difference-Rechtskind.
//
// **Orientierung (hergeleitet + per Feldprobe verifiziert):** in der Komplement-
// Union ist die effektive Außennormale eines Members `eff = −sign·n` (sign aus
// `flatten_members`: ein Difference-Rechtskind trägt −1). Die Stufe-1-Erkennung
// (anti-parallel + koinzident) läuft damit auf `eff_i·eff_j < −0.9995` und
// `(O_i+O_j)≈0` mit `O = −sign·o`. Für einen Difference A−B (s_A=+1, s_B=−1) heißt
// das: die BEIDEN Kontaktflächen haben **parallele** wahre Außennormalen (n_A∥n_B,
// koinzident) — geometrisch die bündig ausgeschnittene Tasche (flush cut), deren
// Taschenmund koplanar mit einer A-Außenfläche liegt.
//
// **Cap-Tiefen:** ein Member, dessen Komplement-Solid die BOX selbst ist (sign=−1,
// z. B. der subtrahierte Körper), liefert seine echte Box-Dicke als Cap. Ein
// Member, dessen Komplement-Solid `¬box` ist (sign=+1), hat auf seiner Seite keine
// endliche Dicke — das Prisma bekommt dort eine Marge `p_circ + 1.5·δ` (die
// Feldprobe zeigt: eine Marge < ~inradius(P) lässt den Fin zurückkehren; erst mit
// `cap ≥ lateraler Reichweite von P` dominiert die Seitenwand-Distanz und der
// Restfehler fällt auf die B0-Keilklasse ≤ 0.75·δ am ∂P-Rand).
//
// **Duale Sicherheit (Analogon zu S⊆U):** das Prisma darf nur dort subtrahieren,
// wo die wahre Dilation Luft hat, d. h. `S ⊆ {f_inner ≥ 0}` (Komplement des inneren
// Solids). `prism_outside_inner` prüft das stichprobenartig; Verletzung → verwerfen,
// conservative bleibt.

/// Duale Sicherheits-Stichprobe `S ⊆ {f_inner ≥ 0}` (Design §4, Komplement-Bild):
/// an keinem Punkt tief in S (`f_S < −guard`) darf das innere Feld tief im Material
/// liegen (`max_i(member.value) < −guard`) — sonst würde `max(f_inner,−f_S)`
/// echtes Material entfernen. Spiegel von [`prism_subset_of_union`].
fn prism_outside_inner(prism: &BoundedCsg, members: &[Member], guard: f32) -> bool {
    let bb = prism.aabb();
    if bb.is_empty() {
        return true;
    }
    let n = 6;
    for ix in 0..=n {
        for iy in 0..=n {
            for iz in 0..=n {
                let p = [
                    bb.min[0] + (bb.max[0] - bb.min[0]) * ix as f32 / n as f32,
                    bb.min[1] + (bb.max[1] - bb.min[1]) * iy as f32 / n as f32,
                    bb.min[2] + (bb.max[2] - bb.min[2]) * iz as f32 / n as f32,
                ];
                let fs = prism.sdf(p, EvalMode::Visual);
                if fs < -guard {
                    let fi = members
                        .iter()
                        .map(|m| m.value(p))
                        .fold(f32::NEG_INFINITY, f32::max);
                    if fi < -guard {
                        return false;
                    }
                }
            }
        }
    }
    true
}

/// Baut die Kontakt-Prismen für den DILATIONS-Fall (Design §1.4, Stufe 3): sign-
/// bewusste Erkennung koplanarer submergierter Kontaktflächen über die Komplement-
/// Dualität, gleicher Sutherland–Hodgman-Verschnitt, gleiche Verweigerungs-
/// Ehrlichkeit. Die Prismen werden vom Aufrufer als `Difference`-Rechtskind
/// (Schnitt-Constraint `−f_S`) VOR das `+δ` gehängt. Nur Box-Box-Kontakte;
/// mindestens ein Nicht-Box-Partner an einem sonst gültigen Kandidaten → `refused`.
fn synthesize_contact_prisms_dilation(
    members: &[Member],
    ctx: &StopCtx,
    min_feat: &[u32],
    prism_idx: &mut u32,
) -> ContactResult {
    let mut res = ContactResult {
        prisms: Vec::new(),
        exact: false, // Dilation-Prisma verspricht nie S=U' (¬box unbeschränkt) → immer toleranzbeschränkt.
        refused: false,
        rim_bound: 0.0,
        pairs: Vec::new(),
        pen_unrimmed: false,
    };
    let delta = ctx.delta as f64;
    // #384 (Dilations-Duale): die signierte Lage `s = O_i + O_j` im KOMPLEMENT
    // trennt die drei Lagen genauso wie am Erosions-Stopp — nur bedeutet sie
    // hier etwas anderes (s. Doku oben): `s = 0` bündiger Schnitt, `s > 0` der
    // Subtrahend ragt um `s` aus dem Solid heraus, `s < 0` er ist um `|s|`
    // VERGRABEN (Deckel/Steg der Dicke `|s|` — gar kein Kontakt). Klemme auf
    // „bündig" ist die f32-Rundungsklasse `0.02·δ` (#397) für BEIDE
    // Facettenklassen; das alte symmetrische `0.5·δ`-Box-Fenster hielt jeden
    // vergrabenen Hohlraum mit Deckel < 0.5·δ für einen Kontakt und trug ihn ab.
    let round_tol = (0.02 * delta).max(1e-3);
    let overlap_min = (0.5 * delta).max(1e-3);
    let clip_tol = (1e-3 * delta).max(1e-4);

    // FC Phase 2 (#398): verallgemeinerte Member-Facetten (Box-Fast-Path +
    // Extrude-Cap/-Mantel). Nicht-Box-Member ohne planare Facetten liefern `None`
    // und nehmen an keinem Kontakt teil. Der Box-Box-Fall läuft über den SH-Fast-
    // Path in `clip_faces` (byte-identisches Tape).
    let faces: Vec<Option<Vec<ContactFace>>> =
        members.iter().map(|m| member_contact_faces(m.node)).collect();

    for i in 0..members.len() {
        for j in (i + 1)..members.len() {
            let (Some(fi), Some(fj)) = (&faces[i], &faces[j]) else {
                continue;
            };
            let si = members[i].sign as f64;
            let sj = members[j].sign as f64;
            for fa in fi {
                for fb in fj {
                    // Effektive (Komplement-)Normalen und -Offsets: eff = −sign·n,
                    // O = −sign·o. Stufe-1-Erkennung im Komplement.
                    let eff_i = [-si * fa.n[0], -si * fa.n[1], -si * fa.n[2]];
                    let eff_j = [-sj * fb.n[0], -sj * fb.n[1], -sj * fb.n[2]];
                    if fdot(eff_i, eff_j) > -0.9995 {
                        continue;
                    }
                    let oi = -si * fa.o;
                    let oj = -sj * fb.o;
                    let Some(pen) = face_pair_penetration(oi + oj, delta, round_tol) else {
                        continue;
                    };
                    let n = fnrm(eff_i); // Komplement-Außennormale von Member i (Π-Normale).
                    let (e1, e2, _) = plane_frame(n);
                    let comps = clip_faces(fa, fb, e1, e2, clip_tol, overlap_min);
                    if comps.is_empty() {
                        continue; // Kanten-/Punktkontakt / zu klein
                    }
                    let o = fdot(n, fa.loop3[0]);
                    let owner = min_feat[i].min(min_feat[j]);
                    for comp in &comps {
                        let p2d = &comp.outer.verts;
                        // Zirkumradius von P (In-Ebenen) → Cap-Marge der ¬box-Seite.
                        let cen = {
                            let (mut cx, mut cy) = (0.0f64, 0.0);
                            for q in p2d {
                                cx += q[0];
                                cy += q[1];
                            }
                            [cx / p2d.len() as f64, cy / p2d.len() as f64]
                        };
                        let p_circ = p2d
                            .iter()
                            .map(|q| ((q[0] - cen[0]).powi(2) + (q[1] - cen[1]).powi(2)).sqrt())
                            .fold(0.0f64, f64::max);
                        let margin = p_circ + 1.5 * delta;
                        // Cap-Tiefen: −n-Seite = Member i, +n-Seite = Member j. Box-
                        // Komplement (sign=−1) → echte Dicke; ¬box (sign=+1) → Marge.
                        let depth_neg = if si < 0.0 { fa.depth } else { margin };
                        // #384: das Prisma ist an Member i's Komplement-Ebene
                        // verankert; Member j's Komplement beginnt erst bei
                        // `O_i − pen`, seine Rückseite liegt also bei
                        // `o + depth_pos − pen`. Ohne den Abzug ragte das Prisma
                        // bei einer Durchdringung um genau `pen` über die
                        // Gegenseite hinaus — in ECHTES Material (gemessen: der
                        // Taschenboden sank um `pen`, bis 0.5·δ). Für die
                        // ¬box-Seite (sign=+1) ist `margin` keine echte Rückseite,
                        // sondern eine synthetische Reichweite AB DER ANKEREBENE
                        // (sie muss ≥ lateraler Reichweite von P bleiben, s. o.) —
                        // dort wird nichts abgezogen. Bei `pen == 0` ist beides
                        // bit-identisch zum Bestandspfad.
                        let depth_pos =
                            if sj < 0.0 { (fb.depth - pen).max(0.0) } else { margin };
                        let feat = encode_prism_feat(owner, *prism_idx);
                        let Some(prism) =
                            build_prism_component(comp, e1, e2, n, o, depth_neg, depth_pos, feat)
                        else {
                            res.refused = true; // Bogen-Kontur (P3) → ehrlich conservative
                            continue;
                        };
                        // Duale Sicherheit S ⊆ {f_inner ≥ 0}.
                        let guard = (2e-3 * delta as f32).max(1e-3);
                        if !prism_outside_inner(&prism, members, guard) {
                            res.refused = true;
                            continue;
                        }
                        *prism_idx += 1;
                        res.prisms.push(prism);
                        // SSI-Nähte bleiben hier für JEDES Kontakt-Paar
                        // unterdrückt — anders als am Erosions-Stopp (#384), und
                        // das ist gemessen, nicht gespiegelt: am Dilations-Stopp
                        // SUBTRAHIEREN die Kanäle (konvexe Dualkante), und der
                        // Kanal über der Mündungs-Naht eines durchdringenden
                        // Kontakts griff im Versuch bis an die AUSSENkante des
                        // Trägerkörpers durch und trug dort legitimes Material ab
                        // (0.760 mm Fehlstelle bei δ = 2 gegen 0.395 mm ehrlichem
                        // B0-Keil mit Unterdrückung). Beide Varianten halten die
                        // 0.75·δ-Zusage; die kleinere gewinnt.
                        if !res.pairs.contains(&(i, j)) {
                            res.pairs.push((i, j));
                        }
                        if pen > 0.0 {
                            // Der ∂P-Rim spiegelt (wie am Erosions-Stopp) nur
                            // bündige Kontakte — ein durchdringender hat keine
                            // gemeinsame Ebene Π. Also darf die Zusage nicht die
                            // enge `channel_tol` eines bündigen Nachbar-Kontakts
                            // am selben Stopp nennen.
                            res.pen_unrimmed = true;
                        }
                        // Restfehler: konvexer B0-Keil am Kontaktrand ∂P (dual zum
                        // konkaven Sliver), ehrlich als ≈0.75·δ gemeldet. Den
                        // GERADEN ∂P-Anteil schließt der ∂P-Rand-Fillet (Stufe 2,
                        // FC Phase 3); Reflex-Vertices/Bögen bleiben ehrlich stehen.
                        res.rim_bound = res.rim_bound.max(0.75 * ctx.delta);
                    }
                }
            }
        }
    }
    res
}

// ─── F396/#398 Phase 1: Kontakt-Clip als Datensatz für die ∂P-Rim-Synthese ───
//
// Der `RimFillet` (F396-RIM-OPCODE-DESIGN.md) braucht die GEKLIPPTE Kontakt-
// Kontur P samt Frame, den beteiligten Member-Flächen und deren SOLID-Außen-
// normalen. Genau diese Daten fallen beim Kontakt-Prisma-Verschnitt
// (`synthesize_contact_prisms` / `_dilation`) an, werden dort aber verworfen.
// [`contact_clips`] rekonstruiert sie als reine Datensätze — eine faithful,
// ISOLIERTE Spiegelung der Prisma-Accept/Refuse-Logik (identische Detektion,
// identischer Sutherland–Hodgman-Verschnitt, identische duale Sicherheits-
// Probe). Sie hängt NICHTS in den Baum und ändert das ausgelieferte Feld
// nicht; sie wird ausschließlich von `offset_rim` (Synthese) + dessen Tests
// konsumiert.
//
// **Phase-4-Entscheidung (#398, Vereinheitlichung bewusst NICHT vollzogen):**
// Die TEURE, fehleranfällige Logik ist bereits geteilt — `box_contact_faces`,
// `clip_convex`, `cleanup_poly`, `plane_frame`, `build_prism_member`,
// `prism_subset_of_union`/`prism_outside_inner` haben je genau EINE Definition
// und werden von allen drei Pfaden (`synthesize_contact_prisms`, `_dilation`,
// `contact_clips`) aufgerufen. Verbleibende „Duplikation" ist NUR die dünne
// Paar-Iterations-Orchestrierung — und die erzeugt Ausgaben in
// VERSCHIEDENEN Zahlendomänen: der Prisma-Pfad baut f32-`Extrude`-Geometrie
// (deren Vertizes byte-genau ins Golden-Tape fließen — verpflichtender
// Docker-Build-Gate), der Clip-Pfad liefert f64-`ContactClip`-Records. Ein
// gemeinsamer Extraktor müsste beide in EINEM Durchlauf ableiten; das zwänge
// entweder die f32-Prismen-Vertizes durch f64-Zwischenwerte (ULP-Drift →
// Golden-Byte-Änderung) oder die Rim-Records durch f32-Rundung (Drift der
// emittierten Rim-Block-Bytes). Beides ist NICHT beweisbar bit-neutral, und
// die Goldens gaten auf Byte-Identität. Feldstabilität schlägt DRY: die
// Orchestrierungen bleiben getrennt (Golden byte-identisch verifiziert,
// f398 + Offset-Fuzz grün).

/// Ein Kontakt-Verschnitt (Polygon P in der Kontaktebene Π + Frame + die zwei
/// beteiligten Member-Flächen), aus dem `offset_rim` die ∂P-Reflexkanten
/// ableitet. Alle Vektoren in Welt-f64.
pub(crate) struct ContactClip {
    /// Π-Normale (Welt, Einheit). Die geklippte Kontur liegt auf `n·x = o`.
    pub(crate) n: [f64; 3],
    /// In-Ebenen-Basis (e1, e2) zur Rück-Projektion (2D → Welt:
    /// `w = u·e1 + v·e2 + o·n`).
    pub(crate) e1: [f64; 3],
    pub(crate) e2: [f64; 3],
    pub(crate) o: f64,
    /// Geklippte Kontaktkontur P (CCW, In-Ebenen-2D).
    pub(crate) p2d: Vec<[f64; 2]>,
    /// Member-i-Fläche als CCW-Rechteck (In-Ebenen-2D) — der Clip-Subjekt.
    pub(crate) la: Vec<[f64; 2]>,
    /// Member-j-Fläche als CCW-Rechteck (In-Ebenen-2D) — der Clip.
    pub(crate) lb: Vec<[f64; 2]>,
    /// SOLID-Außennormale (Welt) der Member-i-Kontaktfläche (`sign_i · n̂_box`).
    pub(crate) n_i: [f64; 3],
    /// SOLID-Außennormale (Welt) der Member-j-Kontaktfläche (`sign_j · n̂_box`).
    pub(crate) n_j: [f64; 3],
    /// Member-Vorzeichen (`+1` additiv, `−1` subtrahiert) — bestimmt, ob die
    /// In-Ebenen-Wandnormale einer Stufen-Kante ins Material oder in die Luft
    /// zeigt (Komplement-Dualität, F396 §1.4).
    pub(crate) sign_i: f32,
    pub(crate) sign_j: f32,
    /// FC-Phase-3-Verallgemeinerung (#398): die Kontaktfläche P als Liste von
    /// Zusammenhangskomponenten mit optionalen Löchern (nur solche, an denen ein
    /// Kontakt-Prisma tatsächlich platziert würde — faithful Mirror der
    /// Prisma-Synthese). Der Box-Box-Pfad füllt hier GENAU EINE gerade Komponente
    /// ohne Loch, deren `outer` das Ein-Polygon-`p2d` spiegelt (byte-identisch).
    /// `offset_rim::rims_from_clip` iteriert diese Komponenten (outer + Löcher)
    /// und klassifiziert deren gerade ∂P-Kanten über `edge_on_contour`.
    pub(crate) components: Vec<ClipComponent>,
}

#[inline]
fn fscl(a: [f64; 3], s: f64) -> [f64; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

/// Rekonstruiert die Kontakt-Verschnitte eines Stopp-Kind-Baums als Datensätze
/// (KEIN Baum-Eingriff, feldneutral). `delta_signed < 0` = Erosion/konkav
/// (Union-Form), `> 0` = Dilation/konvex (Intersection/Difference-Form). Nur
/// Box-Box-Kontakte; degenerierte/duale-unsichere Verschnitte werden — exakt
/// wie in der Prisma-Synthese — verworfen.
pub(crate) fn contact_clips(inner: &BoundedCsg, delta_signed: f64) -> Vec<ContactClip> {
    let concave = delta_signed < 0.0;
    let union_form = concave;
    let mut members = Vec::new();
    flatten_members(inner, union_form, 1.0, &mut members);
    let delta = delta_signed.abs();
    // #397-Dünnwand-Abgrenzung wie in `synthesize_contact_prisms`: Facetten
    // koinzidieren nur bis auf f32-Rundung (~1e-4), eine gestapelte Wand der
    // Stärke w unterscheidet sich um genau w → enge Toleranz, sonst spiegelte
    // der Rim einen Kontakt, den die Prisma-Synthese verweigert. Seit #384 für
    // BEIDE Facettenklassen und BEIDE Stopp-Richtungen (das lose 0.5·δ-Fenster
    // gab es nur noch für Box-Box am Dilations-Stopp und war dort genau der
    // Bound-Over-Claim, s. §8d).
    let coincide_eff = (0.02 * delta).max(1e-3);
    let overlap_min = (0.5 * delta).max(1e-3);

    // FC Phase 3 (#398): Facetten-Quelle UND ∂P-Rand-Fillet verallgemeinert
    // (Design §5/§8). Box-Box läuft über den metrisch-exakten SH-Fast-Path in
    // `clip_faces` (EINE gerade Komponente ohne Loch) und emittiert byte-
    // identisch zum vorigen Pfad. Nicht-Box-Kontakte (Extrude-Cap/-Mantel)
    // gehen über den cavalier-Verschnitt und liefern ihre Zusammenhangs-
    // komponenten (`pos_plines`) + Löcher (`neg_plines`) als `components`;
    // `offset_rim::rims_from_clip` klassifiziert deren gerade ∂P-Kanten über
    // `edge_on_contour` und lässt Reflex-Vertices/Bögen ehrlich ungerundet.
    let faces: Vec<Option<Vec<ContactFace>>> =
        members.iter().map(|m| member_contact_faces(m.node)).collect();

    // Kleinste Feature-Id je Member — Provenance-Owner der Sicherheits-Prisma-
    // Probe (Spiegel von `synthesize_contact_prisms`). Das hier gebaute Prisma
    // ist ein Wegwerf-Körper NUR für die duale Untermengen-Probe; sein `feat`
    // erreicht keinen Output (der `ContactClip` trägt keine Prisma-Feature-Id).
    // Wir kodieren den Owner dennoch schema-konsistent (keine Magic-Konstante).
    let min_feat: Vec<u32> = members
        .iter()
        .map(|m| {
            crate::brep::branch_pairs::enumerate_branches(m.node)
                .iter()
                .map(|b| b.id.feature_id())
                .min()
                .unwrap_or(0)
        })
        .collect();

    let mut out = Vec::new();
    let mut idx = 0u32;
    for i in 0..members.len() {
        for j in (i + 1)..members.len() {
            let (Some(fi), Some(fj)) = (&faces[i], &faces[j]) else {
                continue;
            };
            let si = members[i].sign as f64;
            let sj = members[j].sign as f64;
            let owner = min_feat[i].min(min_feat[j]);
            for fa in fi {
                for fb in fj {
                    // Detektion: koplanar-antiparallel + koinzident. Erosion an
                    // den WAHREN Normalen, Dilation an den effektiven (Komplement).
                    let n = if concave {
                        if fdot(fa.n, fb.n) > -0.9995 {
                            continue;
                        }
                        // #384: NUR bündige Kontakte spiegeln (Faithful Mirror
                        // von `synthesize_contact_prisms`, dessen `pen == 0`-
                        // Zweig). Ein Luftspalt (s < −Rundungsklasse) ist gar
                        // kein Kontakt; ein DURCHDRINGENDER Kontakt (s > 0) hat
                        // keine gemeinsame Ebene Π — der Rim legte seine Spine
                        // sonst auf die falsche (submergierte) Ebene. Dort
                        // korrigieren stattdessen die regulären SSI-Kanäle den
                        // ∂P-Saum, und die Capability sagt die weitere
                        // B0-Schranke zu.
                        if (fa.o + fb.o).abs() > coincide_eff {
                            continue;
                        }
                        fnrm(fa.n)
                    } else {
                        let eff_i = fscl(fa.n, -si);
                        let eff_j = fscl(fb.n, -sj);
                        if fdot(eff_i, eff_j) > -0.9995 {
                            continue;
                        }
                        // #384 (dual): auch hier NUR bündige Kontakte spiegeln.
                        // `s < −Rundungsklasse` ist gar kein Kontakt (vergrabener
                        // Hohlraum hinter einem Deckel), `s > 0` eine
                        // Durchdringung ohne gemeinsame Ebene Π — der Rim legte
                        // seine Spine sonst auf die falsche Ebene und sagte die
                        // enge Bau-Toleranz für einen Saum zu, den er gar nicht
                        // schließt.
                        if (-si * fa.o + -sj * fb.o).abs() > coincide_eff {
                            continue;
                        }
                        fnrm(eff_i)
                    };
                    let o = fdot(n, fa.loop3[0]);
                    let (e1, e2, _) = plane_frame(n);
                    let proj = |w: &[f64; 3]| -> [f64; 2] { [fdot(e1, *w), fdot(e2, *w)] };
                    let mut la: Vec<[f64; 2]> = fa.loop3.iter().map(proj).collect();
                    let mut lb: Vec<[f64; 2]> = fb.loop3.iter().map(proj).collect();
                    if area2d(&la) < 0.0 {
                        la.reverse();
                    }
                    if area2d(&lb) < 0.0 {
                        lb.reverse();
                    }
                    let clip_tol = (1e-3 * delta).max(1e-4);
                    let components_all = clip_faces(fa, fb, e1, e2, clip_tol, overlap_min);
                    // Faithful Mirror der Prisma-Synthese (`synthesize_contact_prisms`):
                    // je Komponente die duale Sicherheits-Probe (S ⊆ U bzw. außerhalb
                    // des Inneren). Nur Komponenten, an denen ein Prisma tatsächlich
                    // platziert würde, tragen einen Rim. Für Box∩Box ist das GENAU die
                    // eine Komponente mit derselben Probe wie zuvor (byte-identisch).
                    let guard = (2e-3 * delta as f32).max(1e-3);
                    let mut components: Vec<ClipComponent> = Vec::new();
                    for comp in components_all {
                        let p2d = &comp.outer.verts;
                        if p2d.len() < 3 {
                            continue;
                        }
                        let (depth_neg, depth_pos) = if concave {
                            (fa.depth, fb.depth)
                        } else {
                            let cen = {
                                let (mut cx, mut cy) = (0.0f64, 0.0);
                                for q in p2d {
                                    cx += q[0];
                                    cy += q[1];
                                }
                                [cx / p2d.len() as f64, cy / p2d.len() as f64]
                            };
                            let p_circ = p2d
                                .iter()
                                .map(|q| ((q[0] - cen[0]).powi(2) + (q[1] - cen[1]).powi(2)).sqrt())
                                .fold(0.0f64, f64::max);
                            let margin = p_circ + 1.5 * delta;
                            (
                                if si < 0.0 { fa.depth } else { margin },
                                if sj < 0.0 { fb.depth } else { margin },
                            )
                        };
                        let feat = encode_prism_feat(owner, idx);
                        let Some(prism) =
                            build_prism_member(p2d, e1, e2, n, o, depth_neg, depth_pos, feat)
                        else {
                            continue;
                        };
                        let safe = if concave {
                            prism_subset_of_union(&prism, &members, guard)
                        } else {
                            prism_outside_inner(&prism, &members, guard)
                        };
                        if safe {
                            components.push(comp);
                        }
                    }
                    let Some(comp0) = components.first() else {
                        continue;
                    };
                    let p2d = comp0.outer.verts.clone();
                    idx += 1;
                    out.push(ContactClip {
                        n,
                        e1,
                        e2,
                        o,
                        p2d,
                        la,
                        lb,
                        n_i: fscl(fa.n, si),
                        n_j: fscl(fb.n, sj),
                        sign_i: members[i].sign,
                        sign_j: members[j].sign,
                        components,
                    });
                }
            }
        }
    }
    out
}

// STUFE 2 (∂P-Rim-Kanäle + C4-Vertex-Kugeln) ist OFFEN (Rückfall-Klausel #395).
// Der geplante Weg (Design §3.1/§3.4): ∂P in bündige (α≈180°, kein Kanal) und
// Stufen-Segmente zerlegen (Klassifikation über point-in-loop der beiden
// projizierten Carrier-Flächen an einem Rand-Probepunkt), die Stufen als
// 2-Punkt-Seeds mit Normalen (n1 = Außenwand der kürzeren Box, n2 = Kontakt-
// normale der längeren) in die bestehende `build_channel`/`OP_ARC_CHANNEL`-
// Maschinerie einspeisen und die ∂P-Ecken über `VertexSphere` schließen. Ein
// erster Prototyp dieses Rims (2-Punkt-Seed → `build_channel`) hat den Sliver
// bei kleinem δ geschlossen, bei großem δ aber MEHR Fehler erzeugt als der
// blanke Prism-Rest (Über-Addition an den ∂P-Ecken ohne Vertex-Kugeln); er ist
// darum bewusst NICHT im Baupfad. Stufe 1 (Prisma) allein liefert die ehrliche
// B0-Toleranz ≈0.75·δ; siehe F395-CONTACT-PATCH-DESIGN.md §Umsetzungs-Status.

/// Synthetisiert Kanal-Korrekturkörper + Vertex-Kugeln für EINEN
/// Stopp-Knoten (`Offset { inner, delta }` mit dualem Boolean-Kind).
/// Leere Listen = vollständige Verweigerung, `f − δ` bleibt.
fn synthesize_stop(
    inner: &BoundedCsg,
    delta: f32,
    tol: f32,
    report: &mut crate::offset_capability::CapabilityReport,
) -> (
    Vec<OffsetChannel>,
    Vec<VertexSphere>,
    Vec<BoundedCsg>,
    Vec<crate::offset_rim::RimBlock>,
    // #384 §8e: `true` = die enge Kanal-Zusage ist an diesem Stopp widerlegt
    // (der Aufrufer unterdrückt dann seine `tol`-Notiz).
    bool,
) {
    let union_form = delta < 0.0;
    let mut members = Vec::new();
    flatten_members(inner, union_form, 1.0, &mut members);
    if members.len() < 2 {
        return (Vec::new(), Vec::new(), Vec::new(), Vec::new(), false);
    }
    // Phase 4 (#398): der ∂P-Rand-Fillet dieses Stopps aus DERSELBEN
    // `contact_clips`-Quelle wie die Prismen (faithful Accept/Refuse-Mirror,
    // Konkavitäts-Gate inklusive → leer bei versetztem Kontakt). Wird unten in
    // den Tolerance-Notes berücksichtigt (schließt den ∂P-Sliver auf tol) und
    // an den ArcChannel-Knoten weitergereicht.
    let rims: Vec<crate::offset_rim::RimBlock> =
        crate::offset_rim::RimBlock::from_stop(inner, delta).into_iter().collect();
    // Größter ehrlicher B0-Rest (≈0.75·δ) an ungerundeten ∂P-Reflex-Vertices/
    // Bögen eines nicht-konvexen Kontakts (FC Phase 3, Design §5). Bei konvexem
    // P (Box-Box, Zwei-Insel-Rechtecke) ist er 0 → der Rim schließt den ganzen
    // ∂P-Saum auf `tol`. `rim_partial`: Rim greift, lässt aber einen ehrlichen
    // Rest — die Capability meldet dann die weitere (≈0.75·δ) statt der engen
    // (`tol`) Schranke.
    let rim_residual: f32 = rims.iter().map(|b| b.residual_bound).fold(0.0, f32::max);
    let rim_closes = !rims.is_empty() && rim_residual <= 0.0;
    let rim_partial = !rims.is_empty() && rim_residual > 0.0;
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

    // Kleinste Feature-Id je Member (Provenance von Kanälen/Prismen/Vertices).
    let min_feat: Vec<u32> = branch_lists
        .iter()
        .map(|bl| bl.iter().map(|b| b.id.feature_id()).min().unwrap_or(0))
        .collect();

    // Vorzeichen je Member (Normalform) für die sign-bewusste Koplanaritäts-
    // Heuristik. In `union_form` sind alle +1; ein Difference-Rechtskind am
    // Dilations-Stopp trägt −1 (De Morgan, `flatten_members`).
    let signs: Vec<f32> = members.iter().map(|m| m.sign).collect();
    // Die Heuristik mit WAHREN Carrier-Normalen: alle Vorzeichen +1 ⇒
    // `eff = −n̂` ⇒ der anti-parallel/koinzident-Test läuft auf die rohen
    // Carrier-Normalen (Verhalten vor #397). Lockere Koinzidenz 0.5·δ wie
    // gehabt (bündige Boxen berühren sich mit ~1 ULP).
    let signs_true: Vec<f32> = vec![1.0; members.len()];
    let coincide_true = (0.5 * ctx.delta).max(1e-3);

    let mut channels: Vec<OffsetChannel> = Vec::new();
    // #395 (Befund F2, Fix): koplanarer submergierter Grenzflächen-Kontakt.
    // Bei EROSION über Union bauen wir das Kontakt-Prisma (Stufe 1) + die
    // ∂P-Rim-Kanäle (Stufe 2) und heben den Status von der #394-conservative-
    // Degradierung auf `exact` (S = U) bzw. `tolerance_bounded` (∂P-Sliver-
    // Rest). Ist P nicht exakt bestimmbar (Nicht-Box-Member, gekrümmter
    // Kontakt, degenerierter Verschnitt), bleibt die #394-Ehrlichkeit
    // (conservative) — Verweigerung schlägt Falschkorrektur (Design §3.4).
    let mut prisms: Vec<BoundedCsg> = Vec::new();
    // Member-Paare mit Kontakt-Prisma → ihre (degenerierte) SSI-Naht-Synthese
    // wird unten übersprungen.
    let mut prism_pairs: Vec<(usize, usize)> = Vec::new();
    if union_form {
        // #395 Stufe 1: EROSION über Union — koplanares Kontakt-Prisma als Union-Member.
        // Erkennung über die rohen Carrier-Normalen (union_form: signs≡+1 ⇒ die
        // sign-bewusste Fassung ist hier bit-identisch zur Vor-#397-Heuristik).
        // FC Phase 2 (#398): der Prisma-Pfad wird jetzt AUCH für Extrude-
        // Kontakte getriggert. Das Box-Gate (`coplanar_interface_present`) sieht
        // nur analytische Halfspace-Facetten — `branch_halfspace_map` registriert
        // KEINE Extrude/Revolve-Sweeps. Ein zusätzlicher Nicht-Box-Trigger holt
        // die Extrude-Cap/-Mantel-Kontakte herein; für reine Box-Szenen ist er
        // `false`, das Gate bleibt bit-identisch (Golden-Byte-Beweis).
        let box_gate = coplanar_interface_present(
            &branch_maps,
            &branch_lists,
            &signs_true,
            ctx.delta,
            coincide_true,
        );
        let nonbox_present = members
            .iter()
            .any(|m| !matches!(m.node, BoundedCsg::Leaf { primitive: Primitive::Box { .. }, .. }));
        // #384: Facetten-Scan mit dem Kriterium der Synthese. `penetrating` ist
        // der additive Trigger (das Carrier-Gate sieht Durchdringungen nicht);
        // `complete && !candidate` widerlegt das Carrier-Gate (reiner Luftspalt).
        let scan = contact_face_scan(&members, ctx.delta as f64);
        if box_gate || nonbox_present || scan.penetrating {
            let mut prism_idx = 0u32;
            let contact = synthesize_contact_prisms(&members, &ctx, &min_feat, &mut prism_idx);
            if !contact.prisms.is_empty() {
                prisms = contact.prisms;
                prism_pairs = contact.pairs;
                if contact.refused {
                    report.note_conservative(
                        "Koplanare Kontaktfläche nur teilweise korrigierbar (ein Kontakt-Partner \
                         nicht analytisch fassbar) — Rest bleibt Näherung f−δ.",
                    );
                } else if contact.pen_unrimmed {
                    // #384: DURCHDRINGENDER Kontakt (Überlappung < 2·δ) mit
                    // ungleichem Querschnitt. Das Prisma tilgt den flächigen
                    // Phantom-Schlitz; am Kontaktrand ∂P steht eine echte
                    // transversale Dieder-Naht, die die SSI-Kanäle korrigieren
                    // (für durchdringende Paare NICHT unterdrückt). Der ∂P-Rim
                    // spiegelt nur bündige Kontakte, also wird hier die weitere
                    // B0-Schranke zugesagt statt der engen `channel_tol` —
                    // ehrlich statt Bound-Over-Claim (Phase-4-Multi-Clip-Regel).
                    report.note_tolerance(format!(
                        "Durchdringender Kontakt zweier Union-Member (Überlappung < 2·δ) per \
                         Kontakt-Prisma korrigiert; Restfehler = konkaver B0-Sliver am \
                         Kontaktrand ∂P ≤ {:.3} mm (≈0.75·δ).",
                        contact.rim_bound
                    ));
                } else if rim_closes {
                    // Stufe 1 (Prisma) tilgt den flächigen Innenfehler; Stufe 2
                    // (∂P-Rand-Fillet, OP_RIM_FILLET) rundet den verbleibenden
                    // konkaven B0-Sliver am Kontaktrand ∂P metrisch exakt auf
                    // Radius δ. Rest = nur noch die Fillet-Approximation an die
                    // Bau-Toleranz `tol` (wie ein Kanal), nicht mehr 0.75·δ.
                    report.note_tolerance(format!(
                        "Koplanare submergierte Kontaktfläche per Kontakt-Prisma + ∂P-Rand-Fillet \
                         (OP_RIM_FILLET) korrigiert; Restfehler = Fillet-Approximation ≤ {tol} mm \
                         (Bau-Toleranz)."
                    ));
                } else if rim_partial {
                    // FC Phase 3 (#398): nicht-konvexer Kontakt — der Rim rundet
                    // die geraden ∂P-Stufen-Kanten (auf `tol`), lässt aber die
                    // Reflex-(Konkav-)Vertices bzw. Bogen-Stufen ehrlich stehen
                    // (Design §5: Trieder-Misch aus Kontakt-Rim + SSI-Naht,
                    // #395-Stufe-2-Klemme). Es bleibt der lokale B0-Rest.
                    report.note_tolerance(format!(
                        "Koplanare submergierte Kontaktfläche (nicht-konvex) per Kontakt-Prisma + \
                         ∂P-Rand-Fillet (OP_RIM_FILLET) korrigiert; gerade ∂P-Kanten auf ≤ {tol} mm \
                         gerundet, verbleibender B0-Rest an Reflex-Vertices/Bögen ≤ {:.3} mm \
                         (≈0.75·δ, Design §5).",
                        rim_residual
                    ));
                } else if !contact.exact {
                    // Prisma gebaut, aber der Rand-Fillet VERWEIGERT (versetzter/
                    // gemischter Kontakt mit konvexen ∂P-Kanten — Konkavitäts-
                    // Gate, Design §1.1). Es bleibt der ehrliche B0-Sliver/-Keil
                    // am Kontaktrand ∂P (Kanten ≈0.41·δ, Ecken ≈0.73·δ).
                    report.note_tolerance(format!(
                        "Koplanare submergierte Kontaktfläche per Kontakt-Prisma korrigiert; \
                         Restfehler = konkaver B0-Sliver am Kontaktrand ∂P ≤ {:.3} mm \
                         (≈0.75·δ; Rand-Fillet am versetzten Kontakt verweigert).",
                        contact.rim_bound
                    ));
                }
                // Exakt (S = U): keine Notiz — der Stopp bleibt `exact`.
            } else if contact.refused {
                // Kontakt erkannt, aber nicht (voll) baubar — z. B. Bogen-Kontur
                // (P3-Scope) oder ein Partner ohne planare Facetten → ehrlich #394.
                report.note_conservative(
                    "Koplanarer Kontakt nicht (vollständig) als Prisma fassbar (Bogen-Kontur \
                     oder Nicht-Box-Partner ohne planare Facetten) — Rest bleibt Näherung f−δ.",
                );
            } else if box_gate && !(scan.complete && !scan.candidate) {
                // Box-Kontakt erkannt, aber kein Prisma baubar (degeneriert) → #394.
                // Ausnahme (#384): der Facetten-Scan hat ALLE Flächen gesehen und
                // KEIN Paar in Kontakt gefunden — das Carrier-Gate hat dann bloß
                // zwei nahe, aber durch einen echten Luftspalt getrennte Flächen
                // gesehen. Getrennte Körper erodieren unabhängig und exakt; eine
                // Verdachtsmeldung wäre hier ein Fehlalarm.
                report.note_conservative(
                    "Koplanarer/tangentialer Kontakt zweier Boolescher Kinder ohne exakt \
                     bestimmbares Kontaktpolygon (Nicht-Box-Member/gekrümmt/degeneriert) — \
                     Offset in der Kontaktzone nur Näherung (submergierte Fläche, f−δ unbeschränkt).",
                );
            }
            // Nicht-Box vorhanden, aber kein echter Kontakt → gutartig, keine Notiz.
        }
    } else {
        // #395 Stufe 3: DILATION über Intersection/Difference — koplanares
        // Kontakt-Prisma als Schnitt-Constraint (Komplement-Dualität). Sign-
        // bewusste Erkennung auf den gefalteten Membern (die Erosions-Gate-Heuristik
        // `coplanar_interface_present` nutzt WAHRE Normalen und feuert für den
        // parallel-normalen Difference-Kontakt nicht — die Erkennung sitzt darum in
        // der Prisma-Funktion selbst, effektive Normalen −sign·n).
        let mut prism_idx = 0u32;
        let contact =
            synthesize_contact_prisms_dilation(&members, &ctx, &min_feat, &mut prism_idx);
        if !contact.prisms.is_empty() {
            prisms = contact.prisms;
            prism_pairs = contact.pairs;
            if contact.pen_unrimmed {
                // #384 (dual): DURCHDRINGENDER Kontakt am Dilations-Stopp — der
                // Subtrahend ragt um `s < 2·δ` aus dem Solid heraus. Das Prisma
                // tilgt den flächigen Phantom-Fin; der ∂P-Rim spiegelt nur
                // bündige Kontakte, also wird hier die weitere B0-Schranke
                // zugesagt statt der engen `channel_tol` (Phase-4-Multi-Clip-Regel).
                report.note_tolerance(format!(
                    "Durchdringender koplanarer Kontakt an einem Dilations-Stopp (Überstand < 2·δ) \
                     per Kontakt-Prisma (Komplement-Dualität) korrigiert; Restfehler = konvexer \
                     B0-Keil am Kontaktrand ∂P ≤ {:.3} mm (≈0.75·δ).",
                    contact.rim_bound
                ));
            } else if rim_closes {
                report.note_tolerance(format!(
                    "Koplanare submergierte Kontaktfläche an einem Dilations-Stopp per Kontakt-Prisma \
                     (Komplement-Dualität) + ∂P-Rand-Fillet (OP_RIM_FILLET) korrigiert; Restfehler = \
                     Fillet-Approximation ≤ {tol} mm (Bau-Toleranz)."
                ));
            } else if rim_partial {
                report.note_tolerance(format!(
                    "Koplanare submergierte Kontaktfläche (nicht-konvex) an einem Dilations-Stopp per \
                     Kontakt-Prisma (Komplement-Dualität) + ∂P-Rand-Fillet (OP_RIM_FILLET) korrigiert; \
                     gerade ∂P-Kanten auf ≤ {tol} mm gerundet, verbleibender B0-Rest an \
                     Reflex-Vertices/Bögen ≤ {:.3} mm (≈0.75·δ, Design §5).",
                    rim_residual
                ));
            } else {
                report.note_tolerance(format!(
                    "Koplanare submergierte Kontaktfläche an einem Dilations-Stopp per Kontakt-Prisma \
                     (Komplement-Dualität) korrigiert; Restfehler = konvexer B0-Keil am Kontaktrand ∂P \
                     ≤ {:.3} mm (≈0.75·δ; Rand-Fillet am versetzten Kontakt verweigert).",
                    contact.rim_bound
                ));
            }
        } else if contact.refused {
            report.note_conservative(
                "Koplanarer Kontakt an einem Dilations-Stopp (Intersection/Difference) ohne exakt \
                 bestimmbares Kontaktpolygon (Nicht-Box-Member/degenerierter Verschnitt) — Offset \
                 in der Kontaktzone bleibt Näherung f+δ.",
            );
        } else if {
            // Ehrlichkeits-Fallback (#394 + #397): die Prisma-Synthese sieht nur
            // Box-Member; ein koplanarer/tangentialer Kontakt planarer Nicht-Box-
            // Carrier (Extrude, …) am Dilations-Stopp bliebe sonst STILL ohne
            // Notiz. Er wird über ZWEI Erkennungen abgedeckt (OR):
            //
            //  (a) WAHRE Carrier-Normalen (signs≡+1, lockere Koinzidenz 0.5·δ):
            //      der vor #397 bestehende Zweig — anti-parallel-normale Kontakte
            //      (z. B. berührende Difference, koplanare Intersection).
            //  (b) EFFEKTIVE Normalen (echte Member-Vorzeichen, ENGE Koinzidenz):
            //      #397-Verallgemeinerung — die parallel-normale Nicht-Box-Tasche
            //      (Extrude-flush-cut A−B), deren wahre Normalen parallel, deren
            //      effektive (De-Morgan-Komplement) aber anti-parallel sind.
            //
            // **Dünnwand-Abgrenzung (PFLICHT-Kalibrierung #397):** bei sign-Flip
            // sehen die zwei Außenflächen einer dünnen Wand (Box/Extrude − kleinere
            // Box/Extrude, Wandstärke w) effektiv anti-parallel aus, und ihre
            // Ebenen-Offsets unterscheiden sich um genau w (|O_i+O_j| = w). Mit der
            // lockeren 0.5·δ-Koinzidenz aus (a) würde jede Wand w < 0.5·δ fälschlich
            // als koinzidenter submergierter Kontakt gelten → legitime Dünnwände zu
            // conservative degradiert (nicht akzeptabel). Ein ECHTER submergierter
            // Kontakt (flush cut) ist dagegen bis auf f32-Transform-Rundung
            // koinzident (|O_i+O_j| ~1e-4). Die enge Toleranz (0.02·δ, ≥1e-3) trennt
            // beide sauber: sie fängt die Rundungsklasse, verwirft aber jede Wand
            // ≥ ~0.3·δ (Kalibrierung δ=2: 0.04 mm ≪ 0.6 mm dünnste getestete Wand).
            let coincide_eff = (0.02 * ctx.delta).max(1e-3);
            coplanar_interface_present(
                &branch_maps,
                &branch_lists,
                &signs_true,
                ctx.delta,
                coincide_true,
            ) || coplanar_interface_present(
                &branch_maps,
                &branch_lists,
                &signs,
                ctx.delta,
                coincide_eff,
            )
        } {
            report.note_conservative(
                "Koplanarer/tangentialer Kontakt an einem Dilations-Stopp (Intersection/Difference) \
                 ohne Prisma-Korrektur — Offset in der Kontaktzone bleibt Näherung f+δ.",
            );
        }
    }

    // Blockierte Enden je Kanal (Index in `channels`, Trace-Schrittweite,
    // Enden) — für das Spine-Snapping an die Vertex-Kugeln (C4).
    let mut chan_ends: Vec<(usize, f32, Vec<BlockedEnd>)> = Vec::new();
    // ∂P-Rim-Kanäle (Stufe 2) zählen bereits gegen das Arc-Budget.
    let mut total_arcs: usize = channels.iter().map(|c| c.arcs.len()).sum();
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

    for i in 0..members.len() {
        for j in (i + 1)..members.len() {
            // #395: Paare mit Kontakt-Prisma überspringen — ihre SSI-Nähte sind
            // die koinzident/tangential degenerierten, die der Prisma-Pfad
            // korrekt behandelt (sonst leckte ihr Normalkegel Material in die
            // Luft über der Kontaktebene, S ⊄ U).
            if prism_pairs.contains(&(i, j)) {
                continue;
            }
            // Schrittweite aus der Überlappungs-Skala.
            let overlap = Aabb::intersect(members[i].node.aabb(), members[j].node.aabb());
            if overlap.is_empty() {
                continue;
            }
            let osz = overlap.size();
            let oscale = osz[0].max(osz[1]).max(osz[2]).max(ctx.delta);
            let step = (oscale * 0.02).clamp(ctx.tol.max(1e-4) * 4.0, oscale * 0.05);

            for ba in &branch_lists[i] {
                let Some((hs_a, xf_a)) = branch_maps[i].get(&ba.id.as_u64()) else {
                    // Member ohne analytische Halfspace-Karte (Sweep, Gyroid,
                    // Mesh) — seine Nähte lassen sich nicht per Kanal
                    // korrigieren; f−δ bleibt dort unbeschränkt.
                    report.note_conservative(
                        "Boolean-Naht an einer nicht abbildbaren Fläche (Sweep/Gyroid/Mesh ohne Halfspace-Karte) — keine Kanal-Korrektur, Näherung f−δ.",
                    );
                    continue;
                };
                for bb in &branch_lists[j] {
                    let Some((hs_b, xf_b)) = branch_maps[j].get(&bb.id.as_u64()) else {
                        report.note_conservative(
                            "Boolean-Naht an einer nicht abbildbaren Fläche (Sweep/Gyroid/Mesh ohne Halfspace-Karte) — keine Kanal-Korrektur, Näherung f−δ.",
                        );
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
                        crate::brep::ssi::SsiResult::Unsupported => {
                            // SSI kann dieses Flächenpaar (Torus, Gyroid∩Gyroid,
                            // Sweep) nicht seeden — die Naht bleibt unkorrigiert.
                            report.note_conservative(
                                "Boolean-Naht auf einer Fläche, die die Kanal-Synthese nicht seeden kann (Torus/Gyroid/Sweep) — Näherung f−δ.",
                            );
                            continue;
                        }
                    };
                    for comp in seeds {
                        for seed in comp {
                            if total_arcs >= MAX_ARCS_PER_STOP {
                                // Budget: bisherige Kanäle behalten; Vertex-
                                // Synthese aus den bis hier gesammelten Enden.
                                // Kein stiller Cap (#384 offset_capability):
                                // die restlichen Nähte bleiben f−δ.
                                report.note_conservative(
                                    "Kanal-Budget an einem Boolean-Stopp erschöpft — verbleibende Nähte bleiben unkorrigiert, Näherung f−δ.",
                                );
                                let verts = synthesize_vertices(
                                    &members, &triple_cands, &ctx, union_form,
                                    gate_eps, gscale, &min_feat, &mut channel_idx,
                                );
                                close_open_end_caps(&mut channels, MAX_ARCS_PER_STOP);
                                let capped = audit_channel_end_caps(
                                    &channels, &verts, inner, &prisms, delta, tol, report,
                                );
                                return (channels, verts, prisms, rims, capped);
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
                            let (pa, pb) = if ba.id.as_u64() <= bb.id.as_u64() {
                                (ba.id, bb.id)
                            } else {
                                (bb.id, ba.id)
                            };
                            if pa.sub() >= 0x700 || pb.sub() >= 0x700 {
                                continue; // Kollision mit dem Band — verweigern
                            }
                            let sub = 0x700 + (channel_idx % 0xFD);
                            // Erbt die Muster-Instanz des Trägers (#429/E1).
                            let branch =
                                crate::csg::BranchId::from_parts(pa.feature_id(), pa.instance(), sub)
                                    .as_u64();
                            // Plan §9 Stufe 2: BEIDE Eltern mitführen. `branch`
                            // kann nur einen tragen (ein Feature-Feld); die
                            // sekundäre Mess-/Snap-Referenz braucht das Paar.
                            let parents = [pa.as_u64(), pb.as_u64()];
                            if let Some(ch) =
                                build_channel(&pts, closed, &ctx, branch, parents, report)
                            {
                                total_arcs += ch.arcs.len();
                                if total_arcs > MAX_ARCS_PER_STOP {
                                    // Kein stiller Cap (#384 offset_capability).
                                    report.note_conservative(
                                        "Kanal-Budget an einem Boolean-Stopp erschöpft — verbleibende Nähte bleiben unkorrigiert, Näherung f−δ.",
                                    );
                                    let verts = synthesize_vertices(
                                        &members, &triple_cands, &ctx, union_form,
                                        gate_eps, gscale, &min_feat, &mut channel_idx,
                                    );
                                    snap_channel_ends_to_vertices(
                                        &mut channels, &chan_ends, &verts,
                                    );
                                    close_open_end_caps(&mut channels, MAX_ARCS_PER_STOP);
                                    let capped = audit_channel_end_caps(
                                        &channels, &verts, inner, &prisms, delta, tol, report,
                                    );
                                    return (channels, verts, prisms, rims, capped);
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
    close_open_end_caps(&mut channels, MAX_ARCS_PER_STOP);
    let capped = audit_channel_end_caps(&channels, &verts, inner, &prisms, delta, tol, report);
    (channels, verts, prisms, rims, capped)
}

/// #384 §8e: prüft NACH dem End-Kappen-Gate, ob die Korrektur in der
/// Umgebung eines Nahtendes noch Punkte FALSCH KLASSIFIZIERT — mit einem
/// Randpunkt-Zeugen als Beweis ([`end_region_violation`]).
///
/// Warum es das noch gibt, obwohl die Kappe zu ist: der Term kennt nur
/// SEINE Naht. Läuft eine fremde Fläche näher an einem Punkt vorbei als die
/// eigene Naht (die Deckfläche am Ende eines Dieders, die Nachbar-Naht einer
/// Taschen-Ecke), dann behauptet `de − δ` mehr Distanz, als es gibt — das
/// ist das im Modulkopf notierte Phase-C-Restrisiko („Fremdgeometrie NÄHER
/// als K an der Kurve im Kegelgebiet"), jetzt gemessen statt vermutet. Der
/// Opcode kann es nicht sehen (drittes Gate = Semantik-Änderung in sechs
/// Evaluator-Spiegeln), die Synthese kann es nicht wegparametrieren — also
/// wird es GEMELDET: der Stopp sagt dort nicht mehr die Bau-Toleranz zu,
/// sondern die Gate-/B0-Schranke, mindestens aber den gemessenen Fehler.
/// Das Feld bleibt unberührt (die Korrektur ist über die Naht hinweg
/// weiterhin die beste verfügbare, s. §8e).
///
/// Rückgabe: `true`, wenn die enge Zusage gefallen ist (der Aufrufer
/// unterdrückt dann seine `tol`-Notiz).
fn audit_channel_end_caps(
    channels: &[OffsetChannel],
    verts: &[VertexSphere],
    inner: &BoundedCsg,
    prisms: &[BoundedCsg],
    delta_signed: f32,
    tol: f32,
    report: &mut crate::offset_capability::CapabilityReport,
) -> bool {
    if channels.is_empty() {
        return false;
    }
    let field = |x: [f32; 3]| -> f32 {
        let mut v = inner.sdf(x, EvalMode::Visual);
        for p in prisms {
            // Spiegel der Prisma-Verdrahtung in `synthesize_offset_channels_reported`.
            v = if delta_signed < 0.0 {
                v.min(p.sdf(x, EvalMode::Visual))
            } else {
                v.max(-p.sdf(x, EvalMode::Visual))
            };
        }
        v
    };
    let Some((viol, gate, at)) =
        end_region_violation(channels, verts, &field, delta_signed, tol)
    else {
        return false;
    };
    // Schranke: die bewiesene Ein-Term-Auslenkung `(c−1)·δ + 4·tol` (= das
    // Gate, zugleich die B0-Schranke der unkorrigierten Naht) — mindestens
    // aber der GEMESSENE Fehler. Mehrere Terme an einem Punkt können sich
    // addieren (das `gate = G + d` des nächsten Kanals sieht das bereits
    // angehobene `d`), darum ist der Zeugenwert Teil der Schranke.
    let bound = gate.max(viol);
    report.note_tolerance(format!(
        "Duale Boolean-Kante über Arc-Spline-Kanäle korrigiert — am Nahtende aber NICHT auf die \
         Bau-Toleranz: dort greift der Term über Fremdgeometrie (Randpunkt-Zeuge bei \
         [{:.2}, {:.2}, {:.2}], nachgewiesene Fehlklassifikation {viol:.3} mm). Fehler ≤ \
         {bound:.3} mm (Gate-/B0-Schranke).",
        at[0], at[1], at[2]
    ));
    true
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
    let mut report = crate::offset_capability::CapabilityReport::new();
    synthesize_offset_channels_reported(node, tol, &mut report)
}

/// Like [`synthesize_offset_channels`] but records the per-stop capability
/// outcome into `report` (#384 offset_capability). Geometrically identical to
/// the plain entry point — the report is purely additive, so the field stays
/// bit-identical: a corrected dual seam is toleranzbeschränkt (arc-spline
/// approximation to `tol`), a dual seam whose synthesis refused stays
/// conservative (`f − δ`).
pub fn synthesize_offset_channels_reported(
    node: BoundedCsg,
    tol: f32,
    report: &mut crate::offset_capability::CapabilityReport,
) -> BoundedCsg {
    match node {
        BoundedCsg::Offset { inner, delta, aabb } => {
            let inner = Box::new(synthesize_offset_channels_reported(*inner, tol, report));
            let dual_stop = match (&*inner, delta) {
                (BoundedCsg::Union { .. }, d) if d < 0.0 => true,
                (BoundedCsg::Intersection { .. }, d) if d > 0.0 => true,
                (BoundedCsg::Difference { .. }, d) if d > 0.0 => true,
                _ => false,
            };
            if !dual_stop || delta == 0.0 || !delta.is_finite() {
                return BoundedCsg::Offset { inner, delta, aabb };
            }
            // `synthesize_stop` records the per-seam outcome into `report`
            // (conservative for genuine geometric refusals — unsupported SSI
            // surfaces, too-sharp angles, missing halfspace maps; tolerance for
            // below-threshold seams). An empty result therefore needs no
            // blanket note: it is either already flagged conservative there or
            // was genuinely correction-free (benign).
            // Phase 4 (#398): `synthesize_stop` liefert jetzt auch die ∂P-Rand-
            // Fillet-Blöcke des Stopps (dieselbe `contact_clips`-Quelle wie die
            // Prismen, Konkavitäts-Gate inklusive) — sie schließen den δ-schmalen
            // Sliver/Keil, den das Prisma stehen lässt, und fließen in die
            // Capability-Note (Rest = tol statt 0.75·δ).
            let (channels, verts, prisms, rims, cap_over_claim) =
                synthesize_stop(&inner, delta, tol, report);
            // #395: die Kontakt-Prismen als reguläre Boolean-Member VOR dem ±δ in
            // den gefalteten Baum hängen (Design §3.2). Da S ⊆ U (Erosion) bzw.
            // S ⊆ {f_inner ≥ 0} (Dilation) bleibt die naive Boolean-Nullmenge
            // unverändert, nur die Offset-Nullmenge wird korrekt. Report-unabhängig
            // (rein geometrisch) → das Feld bleibt zwischen Report-/Nicht-Report-
            // Pfad bit-identisch.
            //   Erosion `−δ` über Union: `min(f_i, f_S)` (Union-Member, füllt den
            //     Phantom-Schlitz — Stufe 1).
            //   Dilation `+δ` über Intersection/Difference: `max(f_inner, −f_S)`
            //     (Difference-Rechtskind, tilgt den Phantom-Fin — Stufe 3,
            //     Komplement-Dualität).
            let inner: Box<BoundedCsg> = if prisms.is_empty() {
                inner
            } else {
                let mut node = *inner;
                for prism in prisms {
                    node = if delta < 0.0 {
                        let bb = Aabb::union(node.aabb(), prism.aabb());
                        BoundedCsg::Union {
                            left: Box::new(node),
                            right: Box::new(prism),
                            aabb: bb,
                        }
                    } else {
                        // Difference: das Ergebnis kann nur schrumpfen → linke AABB.
                        let bb = node.aabb();
                        BoundedCsg::Difference {
                            left: Box::new(node),
                            right: Box::new(prism),
                            aabb: bb,
                        }
                    };
                }
                Box::new(node)
            };
            let stop = BoundedCsg::Offset { inner, delta, aabb };
            if channels.is_empty() && verts.is_empty() && rims.is_empty() {
                // Reiner Prisma-Fall (Status exact/tolerance ist schon in
                // `synthesize_stop` notiert) oder gar keine Korrektur.
                return stop;
            }
            if (!channels.is_empty() || !verts.is_empty()) && !cap_over_claim {
                // #384 §8e: `cap_over_claim` heißt, die Sonde hat die enge
                // Zusage an diesem Stopp mit einem Randpunkt-Zeugen widerlegt
                // und die ehrliche (weitere) Schranke bereits notiert — dann
                // wäre diese Notiz ein Bound-Over-Claim.
                report.note_tolerance(format!(
                    "Duale Boolean-Kante über Arc-Spline-Kanäle toleranzbeschränkt korrigiert (Ziel-Toleranz ≤ {tol} mm)."
                ));
            }
            let aabb = Aabb::union(
                Aabb::union(
                    Aabb::union(stop.aabb(), channels_aabb(&channels)),
                    verts_aabb(&verts),
                ),
                rims_aabb(&rims),
            );
            BoundedCsg::ArcChannel {
                inner: Box::new(stop),
                channels,
                verts,
                rims,
                aabb,
            }
        }
        BoundedCsg::Union { left, right, aabb } => BoundedCsg::Union {
            left: Box::new(synthesize_offset_channels_reported(*left, tol, report)),
            right: Box::new(synthesize_offset_channels_reported(*right, tol, report)),
            aabb,
        },
        BoundedCsg::Difference { left, right, aabb } => BoundedCsg::Difference {
            left: Box::new(synthesize_offset_channels_reported(*left, tol, report)),
            right: Box::new(synthesize_offset_channels_reported(*right, tol, report)),
            aabb,
        },
        BoundedCsg::Intersection { left, right, aabb } => BoundedCsg::Intersection {
            left: Box::new(synthesize_offset_channels_reported(*left, tol, report)),
            right: Box::new(synthesize_offset_channels_reported(*right, tol, report)),
            aabb,
        },
        BoundedCsg::SmoothUnion { left, right, radius, mode, aabb } => BoundedCsg::SmoothUnion {
            left: Box::new(synthesize_offset_channels_reported(*left, tol, report)),
            right: Box::new(synthesize_offset_channels_reported(*right, tol, report)),
            radius,
            mode,
            aabb,
        },
        BoundedCsg::SmoothDifference { left, right, radius, mode, aabb } => {
            BoundedCsg::SmoothDifference {
                left: Box::new(synthesize_offset_channels_reported(*left, tol, report)),
                right: Box::new(synthesize_offset_channels_reported(*right, tol, report)),
                radius,
                mode,
                aabb,
            }
        }
        BoundedCsg::Morph { left, right, t, aabb } => BoundedCsg::Morph {
            left: Box::new(synthesize_offset_channels_reported(*left, tol, report)),
            right: Box::new(synthesize_offset_channels_reported(*right, tol, report)),
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
            left: Box::new(synthesize_offset_channels_reported(*left, tol, report)),
            right: Box::new(synthesize_offset_channels_reported(*right, tol, report)),
            mask_polyline,
            mask_radius,
            blend_radius,
            blend_mode,
            is_difference,
            provenance,
            aabb,
        },
        BoundedCsg::Twist { inner, rate, aabb } => BoundedCsg::Twist {
            inner: Box::new(synthesize_offset_channels_reported(*inner, tol, report)),
            rate,
            aabb,
        },
        BoundedCsg::Bend { inner, rate, aabb } => BoundedCsg::Bend {
            inner: Box::new(synthesize_offset_channels_reported(*inner, tol, report)),
            rate,
            aabb,
        },
        BoundedCsg::Displace { inner, depth, freq, aabb } => BoundedCsg::Displace {
            inner: Box::new(synthesize_offset_channels_reported(*inner, tol, report)),
            depth,
            freq,
            aabb,
        },
        BoundedCsg::Shell { inner, thickness, open_faces, aabb } => BoundedCsg::Shell {
            inner: Box::new(synthesize_offset_channels_reported(*inner, tol, report)),
            thickness,
            open_faces,
            aabb,
        },
        // #384 D2: recurse into the eroded copy so any dual offset stop inside it
        // (a shelled boolean body) gets its channel correction bodies too.
        BoundedCsg::InnerWall { inner, aabb } => BoundedCsg::InnerWall {
            inner: Box::new(synthesize_offset_channels_reported(*inner, tol, report)),
            aabb,
        },
        BoundedCsg::ArcChannel { inner, channels, verts, rims, aabb } => BoundedCsg::ArcChannel {
            inner: Box::new(synthesize_offset_channels_reported(*inner, tol, report)),
            channels,
            verts,
            rims,
            aabb,
        },
        other @ (BoundedCsg::Empty
        | BoundedCsg::Leaf { .. }
        | BoundedCsg::Extrude { .. }
        | BoundedCsg::Revolve { .. }
        | BoundedCsg::Helix { .. }
        | BoundedCsg::Sweep { .. }
        | BoundedCsg::Loft { .. }) => other,
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
            BoundedCsg::ArcChannel { inner, channels, verts, rims, aabb } => {
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
                        Aabb::union(
                            Aabb::union(inner.aabb(), channels_aabb(channels)),
                            verts_aabb(verts),
                        ),
                        rims_aabb(rims),
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

// ─── Zweistufiges Picking (Plan §9, Stufe 2) ────────────────────────────────
//
// Die synthetischen Terme dieses Moduls (Kanäle, Vertex-Kugeln, Kontakt-
// Prismen) tragen Provenance, die vor v0.51.0 NIEMAND auflöste: eine gepickte
// Kanalfläche meldete nur die Branch-Id EINES der beiden Kanten-Eltern, eine
// gepickte Prismenfläche eine synthetische Feature-Id (Bit-19-Marker), die im
// Dokument gar nicht existiert — die Web-Schicht selektierte damit nichts.
//
// [`resolve_synthetic`] ist die Rückgewinnung: primär bleibt die gepickte
// Fläche selbst (Identität, `branch`), sekundär liefert sie die Eltern als
// Mess-/Snap-Referenz bzw. das Owner-Feature.

/// Klasse einer synthetischen Offset-Fläche.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyntheticKind {
    /// Arc-Spline-Kanal an einer dualen Boolean-Naht (Phase B).
    Channel,
    /// Rossignac–Requicha-Vertex-Kugel an einer Triple-Junction (Phase C4).
    VertexSphere,
    /// Kontakt-Prisma an einer koplanaren Kontaktfläche (#395/#398).
    Prism,
}

/// Sekundäre Auflösung eines Pick-Treffers auf einer synthetischen Fläche.
#[derive(Debug, Clone, PartialEq)]
pub struct SyntheticFace {
    /// Die gepickte Branch-Id selbst (primäre Identität).
    pub branch: u64,
    pub kind: SyntheticKind,
    /// Kanal: die BEIDEN Kanten-Eltern-BranchIds (geordnet). Vertex-Kugel und
    /// Prisma führen kein Branch-Paar mit — dort ist die Liste leer und
    /// `owner` trägt die Provenance.
    pub parents: Vec<u64>,
    /// Owner-Feature (Prisma: aus [`prism_owner`]; Kanal/Vertex-Kugel: das
    /// Feature-Feld der Branch-Id). `None` nur, wenn ein Prisma-Owner beim
    /// Kodieren nicht ins 12-Bit-Feld passte (Sentinel) — dann ist die
    /// Provenance ehrlich unbekannt statt falsch.
    pub owner: Option<u32>,
}

/// Basis des reservierten Sub-Bands synthetischer Offset-Flächen (Plan §9).
/// Teilt sich das Band mit `offset_fold::JOIN_ARC_SUB_BASE`, deshalb ist der
/// Sub allein KEIN Beweis: [`resolve_synthetic`] verlangt zusätzlich einen
/// Treffer im Baum.
pub const CHANNEL_SUB_BASE: u32 = 0x700;

/// Plan §9, Stufe 2: löst eine gepickte Branch-Id gegen den Pick-/Mesh-Baum
/// auf. `None` = keine synthetische Fläche (normale Feature-Geometrie).
pub fn resolve_synthetic(tree: &BoundedCsg, branch: u64) -> Option<SyntheticFace> {
    let id = crate::csg::BranchId(branch);
    // Kontakt-Prismen sind an der Feature-Id erkennbar (Bit-19-Marker) — kein
    // Baum-Lauf nötig, das Schema ist selbsttragend.
    if is_prism_feat(id.feature_id()) {
        return Some(SyntheticFace {
            branch,
            kind: SyntheticKind::Prism,
            parents: Vec::new(),
            owner: prism_owner(id.feature_id()),
        });
    }
    if id.sub() < CHANNEL_SUB_BASE {
        return None;
    }
    fn walk(n: &BoundedCsg, branch: u64) -> Option<SyntheticFace> {
        match n {
            BoundedCsg::ArcChannel { inner, channels, verts, .. } => {
                for c in channels {
                    if c.branch == branch {
                        return Some(SyntheticFace {
                            branch,
                            kind: SyntheticKind::Channel,
                            parents: c.parents.to_vec(),
                            owner: Some(crate::csg::BranchId(branch).feature_id()),
                        });
                    }
                }
                for v in verts {
                    if v.branch == branch {
                        return Some(SyntheticFace {
                            branch,
                            kind: SyntheticKind::VertexSphere,
                            parents: Vec::new(),
                            owner: Some(crate::csg::BranchId(branch).feature_id()),
                        });
                    }
                }
                walk(inner, branch)
            }
            BoundedCsg::Union { left, right, .. }
            | BoundedCsg::Difference { left, right, .. }
            | BoundedCsg::Intersection { left, right, .. }
            | BoundedCsg::SmoothUnion { left, right, .. }
            | BoundedCsg::SmoothDifference { left, right, .. }
            | BoundedCsg::MaskedBlend { left, right, .. }
            | BoundedCsg::Morph { left, right, .. } => {
                walk(left, branch).or_else(|| walk(right, branch))
            }
            BoundedCsg::Twist { inner, .. }
            | BoundedCsg::Bend { inner, .. }
            | BoundedCsg::Displace { inner, .. }
            | BoundedCsg::Shell { inner, .. }
            | BoundedCsg::InnerWall { inner, .. }
            | BoundedCsg::Offset { inner, .. } => walk(inner, branch),
            _ => None,
        }
    }
    walk(tree, branch)
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
