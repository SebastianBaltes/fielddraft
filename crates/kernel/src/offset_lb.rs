//! #384 §8o — **die untere Distanzschranke `L(x) ≤ dist(x,∁Ω)` als
//! zugeschnittene Vereinigungs-B-Rep im Tape.**
//!
//! ## Warum
//!
//! Der konkave Kanal-/Kugel-Term darf höchstens `G = (c−1)·δ + Marge` Material
//! addieren (`gate = G − d₀`, §8i). An der Stufen-Nabe braucht die Naht
//! `d₀ − f_wahr = 1.200` gegen `G = 0.868` — der Term hat den exakten Wert
//! längst (`w_geo = dist − δ`), das Gate hält ihn zurück. §8n hat den Ausweg
//! bewiesen und beziffert:
//!
//! ```text
//! gate = max(G − d₀, L(x) − δ)   mit   L(x) ≤ dist(x, ∁Ω)
//! ```
//!
//! Der §8i-Satz überlebt (`f ≥ min(d₀ − G, δ − L)`), und der NEUE Anteil kann
//! **strukturell kein Phantom-Material erzeugen**: wo `L − δ` bindet, ist
//! `f ≥ δ − L ≥ δ − dist = e` — das Feld liegt ÜBER dem Zielfeld, die Korrektur
//! kann nur fehlendes Material auffüllen. Die alte `G`-Kappe ist der Spezialfall
//! `L = max_k depth_k` (die TRIVIALE Member-Teilmenge).
//!
//! §8n hat auch gemessen, dass die Keil-Form (`min` über `∏_k n_k` konvexe
//! Keile) exponentiell ist — der Rippen-Kamm hat fünf Boxen, also `6⁵ = 7776`
//! Keile. Der lineare Weg ist dieses Modul: für `x ∈ Ω` ist
//! `dist(x,∁Ω) = dist(x,∂Ω)`, und `∂Ω` zerfällt in `Σ_k n_k` **zugeschnittene
//! Randstücke**. Das Zuschneiden passiert EINMAL in der Synthese, zur Laufzeit
//! bleibt ein `min` über Stücke.
//!
//! ## Die Sicherheitsrichtung (bindend)
//!
//! * **Kein Stück darf fehlen.** Fehlt ein Randstück, wird das `min` zu gross,
//!   `L` überschätzt die Distanz und das Gate erlaubt Phantom-Material. Deshalb
//!   startet die Synthese mit ALLEN Facetten ALLER Member (`∂Ω ⊆ ⋃_k ∂P_k`) und
//!   entfernt nur, was **beweisbar im Inneren** eines anderen Members liegt.
//! * **Zuschnitt darf konservativ zu GROSS sein.** Ein Stück-Obermenge macht
//!   das `min` kleiner, also `L` kleiner — sicher, nur schwächer. Jeder
//!   „kann ich nicht darstellen"-Zweig dieses Moduls behält deshalb das Stück
//!   ungeschnitten.
//! * **Ausserhalb `Ω` ist die rohe Stück-Schranke UNGÜLTIG** (`min` über `∂Ω`
//!   ist dort `> 0`, `dist(x,∁Ω)` aber `= 0`). Der Auswerter kappt sie mit dem
//!   Innen-Indikator, den er ohnehin hat: `d₀ − δ = min_k f_k` ist das naive
//!   Vereinigungsfeld, also `x ∈ Ω ⟺ d₀ ≤ δ`. Siehe [`lb_value`].
//!
//! ## Stück-Typen
//!
//! Zwei, beide mit geschlossener EXAKTER Distanzformel:
//!
//! * [`LbPiece::Tri`] — Welt-Dreieck. Deckt jede ebene Facette ab
//!   (Fächer-Triangulierung eines konvexen Rings).
//! * [`LbPiece::Rev`] — ein um eine Achse rotiertes `(r,z)`-Segment:
//!   Zylindermantel-Band (`r0 = r1`), Kreisring/Scheibe (`z0 = z1`),
//!   Kegelstumpf sonst. Damit bleiben Zylinder EXAKT — eine Polygonisierung
//!   müsste ihren Sagitta-Fehler global von `L` abziehen und würde die Nabe
//!   genau um diesen Betrag verfehlen (bei 16 Facetten: 0.115 mm gegen eine
//!   Zusage von 0.010).
//!
//! Beide teilen sich EINEN Record fester Länge ([`LB_STRIDE`] Worte) — der
//! Tape-Vertrag verlangt eine Payload-Länge, die alle vier Tape-Spiegel mit
//! einer Multiplikation ausrechnen können.

use serde::Serialize;

use crate::bvh::BoundedCsg;
use crate::transform::quat_rotate;
use fielddraft_protocol::Primitive;

/// Worte je Stück-Record im Tape (`kind` + 10 Nutzworte).
pub const LB_STRIDE: usize = 11;

/// Kopf-Worte des `OP_STOP_LB`-Payloads: `n`, `delta`, `slack`, `c[3]`,
/// `skip_r`.
pub const LB_HEADER: usize = 7;

/// Neutraler Wert des `L`-Registers (Spiegel von
/// [`crate::offset_channel::END_PLANE_OFF`]): `−1e30` ⇒
/// `gate = max(G − d₀, −1e30 − δ) = G − d₀`, also bit-identisch zum Zustand
/// vor §8o. Das ist der Kontroll-Äquivalenz-Pfad („leeres `L`").
pub const LB_NONE: f32 = -crate::offset_channel::END_PLANE_OFF;

/// Ein Randstück der zugeschnittenen Vereinigungs-B-Rep.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub enum LbPiece {
    /// Welt-Dreieck `(a,b,c)`.
    Tri {
        a: [f32; 3],
        b: [f32; 3],
        c: [f32; 3],
    },
    /// Rotationsfläche: das Segment `(r0,z0) → (r1,z1)` im `(r,z)`-Halbebenen-
    /// Profil um die Achse `o + t·ax` (`|ax| = 1`), voll umlaufend.
    Rev {
        o: [f32; 3],
        ax: [f32; 3],
        r0: f32,
        z0: f32,
        r1: f32,
        z1: f32,
    },
}

impl LbPiece {
    /// Der Record im Tape-Layout (`LB_STRIDE` Worte, erstes Wort = `kind`).
    pub fn to_words(self) -> [f32; LB_STRIDE] {
        let mut w = [0.0f32; LB_STRIDE];
        match self {
            LbPiece::Tri { a, b, c } => {
                w[0] = 0.0;
                w[1..4].copy_from_slice(&a);
                w[4..7].copy_from_slice(&b);
                w[7..10].copy_from_slice(&c);
            }
            LbPiece::Rev { o, ax, r0, z0, r1, z1 } => {
                w[0] = 1.0;
                w[1..4].copy_from_slice(&o);
                w[4..7].copy_from_slice(&ax);
                w[7] = r0;
                w[8] = z0;
                w[9] = r1;
                w[10] = z1;
            }
        }
        w
    }
}

/// Der `L`-Block EINES dualen Offset-Stopps: alle Randstücke seiner
/// Member-Vereinigung plus die drei Skalare, die der Auswerter braucht.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LbBlock {
    pub pieces: Vec<LbPiece>,
    /// `|δ|` des Stopps — der Innen-Indikator vergleicht `d₀` damit.
    pub delta: f32,
    /// Konservativer Abzug (f32-Rundung der Stück-Koordinaten). Ein Stück, das
    /// durch Rundung um `ε` VON `x` weg wandert, würde `L` um `ε` überschätzen;
    /// `slack` zieht das ab.
    pub slack: f32,
    /// Zentrum/Radius der Fernfeld-Kugel: ausserhalb kann KEIN Term dieses
    /// Stopps mehr gaten (`de > K` ⇒ `w_geo < 0` ⇒ `gate` unbenutzt), also ist
    /// das ganze `min` überflüssig. Reine Kostenoptimierung, feldwert-neutral.
    pub center: [f32; 3],
    pub skip_r: f32,
}

// ─── Auswertung (f32; CPU-Referenz aller Spiegel) ────────────────────────────

#[inline]
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
#[inline]
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
#[inline]
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
#[inline]
fn norm(a: [f32; 3]) -> f32 {
    dot(a, a).sqrt()
}

/// Quadrierte Distanz zu einer Strecke `a→b`.
#[inline]
fn seg_dist2(p: [f32; 3], a: [f32; 3], b: [f32; 3]) -> f32 {
    let ab = sub(b, a);
    let ap = sub(p, a);
    let ll = dot(ab, ab);
    let t = if ll > 1e-20 {
        (dot(ap, ab) / ll).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let q = [ap[0] - ab[0] * t, ap[1] - ab[1] * t, ap[2] - ab[2] * t];
    dot(q, q)
}

/// Exakte Distanz zu einem Welt-Dreieck (Inklusive-Rand). Entartete
/// (kollineare) Dreiecke fallen auf das Kanten-Minimum zurück — für sie IST
/// das Dreieck die Vereinigung seiner Kanten, also bleibt die Formel exakt.
#[inline]
pub fn tri_dist(p: [f32; 3], a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> f32 {
    let ba = sub(b, a);
    let cb = sub(c, b);
    let ac = sub(a, c);
    // `nor = (b−a) × (a−c)` — NICHT `(b−a) × (c−a)`. Das Vorzeichen ist hier
    // nicht kosmetisch: die drei Kanten-Tests unten prüfen `⟨(e × nor), w⟩ ≥ 0`
    // und schlagen bei gedrehtem `nor` ALLE um, das Innere wird nie erkannt und
    // die Formel fällt überall auf das Kanten-Minimum zurück (an der Nabe:
    // 2.3253 statt 2.3156 — 0.0097 zu viel, also Phantom-Material).
    let nor = cross(ba, ac);
    let nn = dot(nor, nor);
    let edge = seg_dist2(p, a, b).min(seg_dist2(p, b, c)).min(seg_dist2(p, c, a));
    if nn <= 1e-24 {
        return edge.sqrt();
    }
    let s = |e: [f32; 3], w: [f32; 3]| -> f32 {
        if dot(cross(e, nor), w) >= 0.0 {
            1.0
        } else {
            -1.0
        }
    };
    let inside = s(ba, sub(p, a)) + s(cb, sub(p, b)) + s(ac, sub(p, c));
    if inside >= 2.0 {
        let h = dot(nor, sub(p, a));
        (h * h / nn).sqrt()
    } else {
        edge.sqrt()
    }
}

/// Exakte Distanz zu einer Rotationsfläche.
///
/// Für `q = o + z·ax + r·û` (`û ⊥ ax`, `|û| = 1`) ist
/// `|p − q|² = (r_p − r)² + (t_p − z)²` minimiert über `û` (der nächste Punkt
/// liegt in DERSELBEN Halbebene wie `p`) — die 3D-Distanz ist also exakt die
/// 2D-Distanz von `(r_p, t_p)` zum Profil-Segment.
#[inline]
pub fn rev_dist(p: [f32; 3], o: [f32; 3], ax: [f32; 3], r0: f32, z0: f32, r1: f32, z1: f32) -> f32 {
    let d = sub(p, o);
    let t = dot(d, ax);
    let rad = [d[0] - ax[0] * t, d[1] - ax[1] * t, d[2] - ax[2] * t];
    let r = norm(rad);
    let (ex, ey) = (r1 - r0, z1 - z0);
    let (px, py) = (r - r0, t - z0);
    let ll = ex * ex + ey * ey;
    let s = if ll > 1e-20 {
        ((px * ex + py * ey) / ll).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (qx, qy) = (px - ex * s, py - ey * s);
    (qx * qx + qy * qy).sqrt()
}

/// Distanz zu EINEM Stück.
#[inline]
pub fn piece_dist(p: [f32; 3], piece: &LbPiece) -> f32 {
    match *piece {
        LbPiece::Tri { a, b, c } => tri_dist(p, a, b, c),
        LbPiece::Rev { o, ax, r0, z0, r1, z1 } => rev_dist(p, o, ax, r0, z0, r1, z1),
    }
}

/// **Die untere Distanzschranke des Stopps.** `base` ist der naive Stoppwert
/// `d₀` (`OP_STOP_BASE`).
///
/// Drei Bestandteile, alle drei tragend:
///
/// 1. **Der Fernfeld-Skip.** Ausserhalb der Term-Kugel kann `gate` niemanden
///    mehr erreichen — der Rückgabewert ist neutral. Reine Kosten.
/// 2. **Das `min` über die Randstücke.** Für `x ∈ Ω` ist
///    `dist(x,∁Ω) = dist(x,∂Ω)`, und `⋃ Stücke ⊇ ∂Ω` (Synthese-Invariante),
///    also `min ≤ dist(x,∂Ω)`.
/// 3. **Der Innen-Indikator.** Für `x ∉ Ω` ist `dist(x,∁Ω) = 0`, das `min`
///    aber `> 0` — dort MUSS die Schranke abgeschaltet werden. Das naive
///    Vereinigungsfeld ist `d₀ − δ = min_k f_k`, und weil jedes `f_k`
///    vorzeichenrichtig ist, gilt `x ∉ Ω ⟹ min_k f_k > 0 ⟹ d₀ > δ`. Der
///    Test ist also vollständig.
///
///    Er ist zugleich FELDSTETIG: auf `∂Ω` ist `d₀ = δ` UND `min = 0`, der
///    Sprung von `0` auf `−1e30` passiert also genau dort, wo beide Zweige
///    `gate = max(G − d₀, ·)` auf denselben Wert `G − δ` führen (`G > 0`).
#[inline]
pub fn lb_value(p: [f32; 3], base: f32, blk: &LbBlock) -> f32 {
    if base > blk.delta || norm(sub(p, blk.center)) > blk.skip_r {
        return LB_NONE;
    }
    let mut best = f32::INFINITY;
    for piece in &blk.pieces {
        let d = piece_dist(p, piece);
        if d < best {
            best = d;
        }
    }
    if !best.is_finite() {
        return LB_NONE;
    }
    best - blk.slack
}

// ─── Synthese ────────────────────────────────────────────────────────────────

/// Obergrenze der Stücke je Stopp. Wird sie überschritten, fällt die Synthese
/// auf den ungeschnittenen Facetten-Satz zurück (konservativ) bzw. verweigert
/// den ganzen Block.
const MAX_LB_PIECES: usize = 512;

/// Ein Member als konvexer Festkörper — die einzige Form, in der die Synthese
/// „Inneres" und „Austritt" beweisbar bestimmen kann.
#[derive(Debug, Clone)]
enum LbSolid {
    /// Konvexes Polytop: `n_i·q ≤ c_i`, plus die Facetten-Ringe.
    Poly { faces: Vec<LbFace> },
    /// Zylinder um `o + t·ax`, Radius `r`, `t ∈ [z0, z1]`.
    Cyl {
        o: [f64; 3],
        ax: [f64; 3],
        r: f64,
        z0: f64,
        z1: f64,
    },
}

#[derive(Debug, Clone)]
struct LbFace {
    n: [f64; 3],
    c: f64,
    ring: Vec<[f64; 3]>,
}

/// Ein AUSTRITT eines Members: `∁int P = ⋃_i Exit_i`. Für jedes Stück `p` gilt
/// `p \ int P = ⋃_i (p ∩ Exit_i)` — deshalb darf nie ein Austritt entfallen,
/// dessen Schnitt nicht leer ist.
#[derive(Debug, Clone, Copy)]
enum LbExit {
    /// `n·q ≥ c`.
    Half { n: [f64; 3], c: f64 },
    /// `dist(q, Achse) ≥ r`.
    Radial { o: [f64; 3], ax: [f64; 3], r: f64 },
}

/// Ein Arbeits-Stück der Synthese (f64).
#[derive(Debug, Clone)]
enum WPiece {
    /// Konvexes Polygon in der Ebene `n·q = c` (Ring in Umlauf-Ordnung).
    Poly {
        n: [f64; 3],
        c: f64,
        ring: Vec<[f64; 3]>,
    },
    Rev {
        o: [f64; 3],
        ax: [f64; 3],
        r0: f64,
        z0: f64,
        r1: f64,
        z1: f64,
    },
}

#[inline]
fn fd(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
#[inline]
fn fs(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
#[inline]
fn fl(a: [f64; 3]) -> f64 {
    fd(a, a).sqrt()
}
#[inline]
fn fu(a: [f64; 3]) -> [f64; 3] {
    let n = fl(a).max(1e-30);
    [a[0] / n, a[1] / n, a[2] / n]
}

/// Sind zwei Achsen dieselbe (bis auf `tol`)? **Die Toleranz ist metrisch, nicht
/// 1-ULP-artig:** die Achsrichtung entsteht aus einer f32-Quaternion, also trägt
/// sie bei einem Hebel von wenigen Millimetern schon ~1e−7 Abweichung. Mit
/// `1e−9` fiel an der Stufen-Nabe der Zylinder-Zylinder-Zuschnitt komplett aus
/// (A-Deckel blieb Vollscheibe, `L` = 1.184 statt 2.316). Der Fehler, den eine
/// als koaxial behandelte Fast-Achse in die Stück-Lage einträgt, ist selbst
/// `≤ tol` und wird vom `slack` (eine Zehnerpotenz grösser) abgedeckt.
#[inline]
fn coaxial(o1: [f64; 3], ax1: [f64; 3], o2: [f64; 3], ax2: [f64; 3], tol: f64) -> bool {
    fd(ax1, ax2).abs() > 1.0 - 1e-6 && frad(o1, o2, ax2) <= tol
}

/// Radialabstand von `q` zur Achse `o + t·ax`.
#[inline]
fn frad(q: [f64; 3], o: [f64; 3], ax: [f64; 3]) -> f64 {
    let d = fs(q, o);
    let t = fd(d, ax);
    fl([d[0] - ax[0] * t, d[1] - ax[1] * t, d[2] - ax[2] * t])
}

impl LbSolid {
    fn exits(&self) -> Vec<LbExit> {
        match self {
            LbSolid::Poly { faces } => faces
                .iter()
                .map(|f| LbExit::Half { n: f.n, c: f.c })
                .collect(),
            LbSolid::Cyl { o, ax, r, z0, z1 } => vec![
                LbExit::Radial { o: *o, ax: *ax, r: *r },
                LbExit::Half { n: *ax, c: fd(*o, *ax) + *z1 },
                LbExit::Half {
                    n: [-ax[0], -ax[1], -ax[2]],
                    c: -(fd(*o, *ax) + *z0),
                },
            ],
        }
    }

    /// Die eigenen Randflächen als Start-Stücke.
    fn faces(&self) -> Vec<WPiece> {
        match self {
            LbSolid::Poly { faces } => faces
                .iter()
                .map(|f| WPiece::Poly {
                    n: f.n,
                    c: f.c,
                    ring: f.ring.clone(),
                })
                .collect(),
            LbSolid::Cyl { o, ax, r, z0, z1 } => vec![
                WPiece::Rev { o: *o, ax: *ax, r0: *r, z0: *z0, r1: *r, z1: *z1 },
                WPiece::Rev { o: *o, ax: *ax, r0: 0.0, z0: *z0, r1: *r, z1: *z0 },
                WPiece::Rev { o: *o, ax: *ax, r0: 0.0, z0: *z1, r1: *r, z1: *z1 },
            ],
        }
    }
}

impl WPiece {
    /// OBERE Schranke von `max_{q ∈ Stück} n·q`. Muss eine echte Obermenge
    /// abdecken: sie entscheidet über das WEGWERFEN eines Stücks, und ein zu
    /// KLEINER Wert würde ein Randstück fälschlich für vergraben halten.
    fn max_lin(&self, n: [f64; 3]) -> f64 {
        match self {
            WPiece::Poly { ring, .. } => {
                ring.iter().map(|v| fd(*v, n)).fold(f64::NEG_INFINITY, f64::max)
            }
            WPiece::Rev { o, ax, r0, z0, r1, z1 } => {
                let na = fd(n, *ax);
                let perp = (fd(n, n) - na * na).max(0.0).sqrt();
                let base = fd(*o, n);
                (base + z0 * na + r0 * perp).max(base + z1 * na + r1 * perp)
            }
        }
    }

    /// UNTERE Schranke von `min_{q ∈ Stück} n·q` (nur für die Abkürzung
    /// „Stück liegt ganz im Austritt" — ein zu kleiner Wert kostet nur
    /// Zuschnitt-Schärfe, nie Gültigkeit).
    fn min_lin(&self, n: [f64; 3]) -> f64 {
        match self {
            WPiece::Poly { ring, .. } => {
                ring.iter().map(|v| fd(*v, n)).fold(f64::INFINITY, f64::min)
            }
            WPiece::Rev { o, ax, r0, z0, r1, z1 } => {
                let na = fd(n, *ax);
                let perp = (fd(n, n) - na * na).max(0.0).sqrt();
                let base = fd(*o, n);
                (base + z0 * na - r0 * perp).min(base + z1 * na - r1 * perp)
            }
        }
    }

    /// UNTERE Schranke des Radialabstands (nur für die Abkürzung „Stück liegt
    /// ganz im Radial-Austritt").
    ///
    /// **Der Achsen-Durchstoss ist Pflicht, nicht Kür.** Ein zu GROSSER Wert
    /// lässt die Abkürzung greifen und überspringt damit den Zuschnitt durch
    /// den GANZEN Member — an der Stufen-Nabe wäre das genau der Fall
    /// „Plattendeckel wird von Zylinder A nicht beschnitten", und die Schranke
    /// fiele von 2.316 auf 0.684 zurück. Deshalb wird der Durchstosspunkt der
    /// Achse durch die Facettenebene getestet: liegt er im Ring, ist das
    /// Minimum `0`.
    fn min_rad(&self, o: [f64; 3], ax: [f64; 3], tol: f64) -> f64 {
        match self {
            WPiece::Poly { n, c, ring } => {
                let na = fd(*n, ax);
                if na.abs() > 1e-9 {
                    // Achsen-Durchstoss durch die Ebene `n·q = c`.
                    let t = (*c - fd(o, *n)) / na;
                    let hit = [o[0] + ax[0] * t, o[1] + ax[1] * t, o[2] + ax[2] * t];
                    if point_in_ring(ring, *n, hit) {
                        return 0.0;
                    }
                }
                let mut best = f64::INFINITY;
                for i in 0..ring.len() {
                    let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
                    best = best.min(seg_rad_min(a, b, o, ax));
                }
                best
            }
            WPiece::Rev { o: po, ax: pax, r0, r1, .. } => {
                if coaxial(*po, *pax, o, ax, tol) {
                    r0.min(*r1)
                } else {
                    0.0
                }
            }
        }
    }

    /// OBERE Schranke des Radialabstands zur Achse `(o, ax)`. Für ein konvexes
    /// Polygon ist der Radialabstand konvex, das Maximum liegt also in einer
    /// Ecke (EXAKT). Für ein `Rev`-Stück mit FREMDER Achse gibt es keine
    /// geschlossene Form — dann liefert die Bounding-Sphere eine gültige
    /// Obermenge (konservativ: mehr Schnitte, weniger Wegwerfen).
    fn max_rad(&self, o: [f64; 3], ax: [f64; 3], tol: f64) -> f64 {
        match self {
            WPiece::Poly { ring, .. } => ring
                .iter()
                .map(|v| frad(*v, o, ax))
                .fold(f64::NEG_INFINITY, f64::max),
            WPiece::Rev { o: po, ax: pax, r0, z0, r1, z1 } => {
                if coaxial(*po, *pax, o, ax, tol) {
                    // Koaxial — exakt.
                    return r0.max(*r1);
                }
                // Bounding-Sphere der Rotationsfläche.
                let mid = 0.5 * (z0 + z1);
                let c = [
                    po[0] + pax[0] * mid,
                    po[1] + pax[1] * mid,
                    po[2] + pax[2] * mid,
                ];
                let half = 0.5 * (z1 - z0).abs();
                let rad = ((half * half + r0.max(*r1) * r0.max(*r1)).sqrt()).max(r0.max(*r1));
                frad(c, o, ax) + rad
            }
        }
    }

    /// Schneidet das Stück mit einem Austritt. `Ok(None)` = leer,
    /// `Err(())` = nicht darstellbar (der Aufrufer behält das Stück ganz).
    #[allow(clippy::result_unit_err)]
    fn clip(&self, e: &LbExit, tol: f64) -> Result<Option<WPiece>, ()> {
        match (self, e) {
            (WPiece::Poly { n, c, ring }, LbExit::Half { n: hn, c: hc }) => {
                let out = clip_ring(ring, *hn, *hc);
                if out.len() < 3 {
                    Ok(None)
                } else {
                    Ok(Some(WPiece::Poly { n: *n, c: *c, ring: out }))
                }
            }
            (WPiece::Poly { n, ring, .. }, LbExit::Radial { o, ax, r }) => {
                // Nur wenn die Facetten-Ebene SENKRECHT zur Achse steht: dann
                // ist `Ring ∩ {rad ≥ r}` in dem Kreisring
                // `[r, max_rad(Ring)]` derselben Ebene enthalten — eine
                // OBERMENGE (das Polygon selbst fällt weg), also sicher.
                if fd(*n, *ax).abs() < 1.0 - 1e-6 {
                    return Err(());
                }
                let zt = fd(fs(ring[0], *o), *ax);
                let rmax = self.max_rad(*o, *ax, tol);
                if rmax <= *r {
                    return Ok(None);
                }
                Ok(Some(WPiece::Rev {
                    o: *o,
                    ax: *ax,
                    r0: *r,
                    z0: zt,
                    r1: rmax,
                    z1: zt,
                }))
            }
            (WPiece::Rev { o, ax, r0, z0, r1, z1 }, LbExit::Half { n: hn, c: hc }) => {
                // Darstellbar, wenn der Halbraum AXIAL ist (`n ∥ ax`): dann ist
                // `n·q = n·o + t·(n·ax)` unabhängig vom Umlaufwinkel und der
                // Schnitt bleibt ein `(r,z)`-Segment.
                let na = fd(*hn, *ax);
                if na.abs() < 1.0 - 1e-6 {
                    return Err(());
                }
                let base = fd(*o, *hn);
                // `base + t·na ≥ hc`  ⇔  `t ≥ (hc−base)/na` (na>0) bzw. `≤`.
                let bound = (hc - base) / na;
                let (lo, hi) = if na > 0.0 {
                    (bound, f64::INFINITY)
                } else {
                    (f64::NEG_INFINITY, bound)
                };
                clip_seg_t(*o, *ax, *r0, *z0, *r1, *z1, lo, hi)
            }
            (WPiece::Rev { o, ax, r0, z0, r1, z1 }, LbExit::Radial { o: ro, ax: rax, r }) => {
                // Nur koaxial darstellbar.
                if !coaxial(*ro, *rax, *o, *ax, tol) {
                    return Err(());
                }
                clip_seg_r(*o, *ax, *r0, *z0, *r1, *z1, *r)
            }
        }
    }
}

/// Liegt `q` (bereits in der Ebene) im konvexen Ring? Umlaufsinn-frei über
/// das Vorzeichen der Kantenkreuzprodukte gegen `n`.
fn point_in_ring(ring: &[[f64; 3]], n: [f64; 3], q: [f64; 3]) -> bool {
    let m = ring.len();
    if m < 3 {
        return false;
    }
    let (mut pos, mut neg) = (false, false);
    for i in 0..m {
        let a = ring[i];
        let b = ring[(i + 1) % m];
        let e = fs(b, a);
        let w = fs(q, a);
        let cr = [
            e[1] * w[2] - e[2] * w[1],
            e[2] * w[0] - e[0] * w[2],
            e[0] * w[1] - e[1] * w[0],
        ];
        let s = fd(cr, n);
        if s > 1e-12 {
            pos = true;
        }
        if s < -1e-12 {
            neg = true;
        }
    }
    !(pos && neg)
}

/// Kleinster Radialabstand entlang der Strecke `a→b` (konvex in `s`, also
/// analytisch lösbar).
fn seg_rad_min(a: [f64; 3], b: [f64; 3], o: [f64; 3], ax: [f64; 3]) -> f64 {
    let proj = |q: [f64; 3]| {
        let d = fs(q, o);
        let t = fd(d, ax);
        [d[0] - ax[0] * t, d[1] - ax[1] * t, d[2] - ax[2] * t]
    };
    let u = proj(a);
    let v = fs(proj(b), u);
    let vv = fd(v, v);
    let s = if vv > 1e-20 {
        (-fd(u, v) / vv).clamp(0.0, 1.0)
    } else {
        0.0
    };
    fl([u[0] + v[0] * s, u[1] + v[1] * s, u[2] + v[2] * s])
}

/// Sutherland–Hodgman: `Ring ∩ {n·q ≥ c}`.
fn clip_ring(ring: &[[f64; 3]], n: [f64; 3], c: f64) -> Vec<[f64; 3]> {
    let m = ring.len();
    let mut out: Vec<[f64; 3]> = Vec::with_capacity(m + 2);
    for i in 0..m {
        let a = ring[i];
        let b = ring[(i + 1) % m];
        let (da, db) = (fd(a, n) - c, fd(b, n) - c);
        if da >= 0.0 {
            out.push(a);
        }
        if (da > 0.0 && db < 0.0) || (da < 0.0 && db > 0.0) {
            let t = da / (da - db);
            out.push([
                a[0] + (b[0] - a[0]) * t,
                a[1] + (b[1] - a[1]) * t,
                a[2] + (b[2] - a[2]) * t,
            ]);
        }
    }
    out
}

/// Klemmt das `(r,z)`-Profil-Segment auf `t ∈ [lo, hi]`.
#[allow(clippy::too_many_arguments, clippy::result_unit_err)]
fn clip_seg_t(
    o: [f64; 3],
    ax: [f64; 3],
    r0: f64,
    z0: f64,
    r1: f64,
    z1: f64,
    lo: f64,
    hi: f64,
) -> Result<Option<WPiece>, ()> {
    let (mut a, mut b) = (0.0f64, 1.0f64);
    let dz = z1 - z0;
    if dz.abs() < 1e-12 {
        if z0 < lo - 1e-12 || z0 > hi + 1e-12 {
            return Ok(None);
        }
    } else {
        let ta = (lo - z0) / dz;
        let tb = (hi - z0) / dz;
        let (t0, t1) = if ta <= tb { (ta, tb) } else { (tb, ta) };
        a = a.max(t0);
        b = b.min(t1);
        if a >= b - 1e-12 {
            return Ok(None);
        }
    }
    Ok(Some(WPiece::Rev {
        o,
        ax,
        r0: r0 + (r1 - r0) * a,
        z0: z0 + dz * a,
        r1: r0 + (r1 - r0) * b,
        z1: z0 + dz * b,
    }))
}

/// Klemmt das Profil-Segment auf `r ≥ rmin` (koaxialer Radial-Austritt).
#[allow(clippy::result_unit_err)]
fn clip_seg_r(
    o: [f64; 3],
    ax: [f64; 3],
    r0: f64,
    z0: f64,
    r1: f64,
    z1: f64,
    rmin: f64,
) -> Result<Option<WPiece>, ()> {
    let dr = r1 - r0;
    let (mut a, mut b) = (0.0f64, 1.0f64);
    if dr.abs() < 1e-12 {
        if r0 < rmin - 1e-12 {
            return Ok(None);
        }
    } else {
        let t = (rmin - r0) / dr;
        if dr > 0.0 {
            a = a.max(t);
        } else {
            b = b.min(t);
        }
        if a >= b - 1e-12 {
            return Ok(None);
        }
    }
    Ok(Some(WPiece::Rev {
        o,
        ax,
        r0: r0 + dr * a,
        z0: z0 + (z1 - z0) * a,
        r1: r0 + dr * b,
        z1: z0 + (z1 - z0) * b,
    }))
}

/// Schnitt-Test `Stück ∩ Austritt ≠ ∅`. **Darf nie `false` liefern, wenn der
/// Schnitt nicht leer ist** — sonst würde ein echtes Randstück verworfen. Alle
/// benutzten Schranken sind deshalb OBERE Schranken.
fn hits(p: &WPiece, e: &LbExit, tol: f64) -> bool {
    match e {
        LbExit::Half { n, c } => p.max_lin(*n) >= *c - tol,
        LbExit::Radial { o, ax, r } => p.max_rad(*o, *ax, tol) >= *r - tol,
    }
}

/// Abkürzung: liegt das Stück GANZ in diesem Austritt? Dann berührt es das
/// Member-Innere nicht und braucht keinen Zuschnitt. Ein falsches `false`
/// kostet nur Schärfe.
fn contained(p: &WPiece, e: &LbExit, tol: f64) -> bool {
    match e {
        LbExit::Half { n, c } => p.min_lin(*n) >= *c - tol,
        LbExit::Radial { o, ax, r } => p.min_rad(*o, *ax, tol) >= *r - tol,
    }
}

/// Ein Member in [`LbSolid`]-Form, oder `None` (dann verweigert der ganze
/// Stopp seinen `L`-Block — eine unbekannte Member-Form heisst unbekanntes
/// `∂Ω`, und ein fehlendes Randstück ist der eine Fehler, den `L` nicht machen
/// darf).
fn lb_solid(node: &BoundedCsg) -> Option<LbSolid> {
    let BoundedCsg::Leaf { primitive, world_transform, .. } = node else {
        return None;
    };
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
        let r = quat_rotate(rot, sp);
        [r[0] as f64 + tr[0], r[1] as f64 + tr[1], r[2] as f64 + tr[2]]
    };
    let rot_dir = |p: [f64; 3]| -> [f64; 3] {
        let r = quat_rotate(rot, [p[0] as f32, p[1] as f32, p[2] as f32]);
        fu([r[0] as f64, r[1] as f64, r[2] as f64])
    };
    match primitive {
        Primitive::Box { center, half_extents } => {
            let c = [center[0] as f64, center[1] as f64, center[2] as f64];
            let h = [
                half_extents[0] as f64,
                half_extents[1] as f64,
                half_extents[2] as f64,
            ];
            if h[0] <= 0.0 || h[1] <= 0.0 || h[2] <= 0.0 {
                return None;
            }
            let unit = |k: usize| {
                let mut e = [0.0f64; 3];
                e[k] = 1.0;
                e
            };
            let mut faces = Vec::with_capacity(6);
            for k in 0..3 {
                let (a, b) = ((k + 1) % 3, (k + 2) % 3);
                for sgn in [1.0f64, -1.0] {
                    let nl = [
                        unit(k)[0] * sgn,
                        unit(k)[1] * sgn,
                        unit(k)[2] * sgn,
                    ];
                    let n = rot_dir(nl);
                    let fc = [
                        c[0] + nl[0] * h[k],
                        c[1] + nl[1] * h[k],
                        c[2] + nl[2] * h[k],
                    ];
                    let (ea, eb) = (unit(a), unit(b));
                    let mut ring = Vec::with_capacity(4);
                    for (sa, sb) in [(1.0f64, 1.0f64), (1.0, -1.0), (-1.0, -1.0), (-1.0, 1.0)] {
                        ring.push(l2w([
                            fc[0] + ea[0] * sa * h[a] + eb[0] * sb * h[b],
                            fc[1] + ea[1] * sa * h[a] + eb[1] * sb * h[b],
                            fc[2] + ea[2] * sa * h[a] + eb[2] * sb * h[b],
                        ]));
                    }
                    let cw = fd(n, l2w(fc));
                    faces.push(LbFace { n, c: cw, ring });
                }
            }
            Some(LbSolid::Poly { faces })
        }
        Primitive::Cylinder { center, radius, half_height } => {
            if *radius <= 0.0 || *half_height <= 0.0 {
                return None;
            }
            // `Primitive::Cylinder` hat die Achse in LOKAL-Y (s. `sdf_eval`).
            let ax = rot_dir([0.0, 1.0, 0.0]);
            let o = l2w([center[0] as f64, center[1] as f64, center[2] as f64]);
            let hh = *half_height as f64 * scl;
            Some(LbSolid::Cyl {
                o,
                ax,
                r: *radius as f64 * scl,
                z0: -hh,
                z1: hh,
            })
        }
        _ => None,
    }
}

/// Flacht die Union-Kette des Stopps in seine Member — identisch zu
/// `offset_channel::flatten_members` im `union_form`-Zweig (dort sind alle
/// Vorzeichen `+1`, und NUR dieser Zweig hat einen konkaven Stopp).
fn flatten_union<'a>(node: &'a BoundedCsg, out: &mut Vec<&'a BoundedCsg>) {
    match node {
        BoundedCsg::Union { left, right, .. } => {
            flatten_union(left, out);
            flatten_union(right, out);
        }
        other => out.push(other),
    }
}

/// Fächer-Triangulierung eines konvexen Rings in [`LbPiece::Tri`].
fn emit_ring(ring: &[[f64; 3]], out: &mut Vec<LbPiece>) {
    if ring.len() < 3 {
        return;
    }
    let v = |q: [f64; 3]| [q[0] as f32, q[1] as f32, q[2] as f32];
    for i in 1..ring.len() - 1 {
        out.push(LbPiece::Tri {
            a: v(ring[0]),
            b: v(ring[i]),
            c: v(ring[i + 1]),
        });
    }
}

fn emit_piece(p: &WPiece, out: &mut Vec<LbPiece>) {
    match p {
        WPiece::Poly { ring, .. } => emit_ring(ring, out),
        WPiece::Rev { o, ax, r0, z0, r1, z1 } => out.push(LbPiece::Rev {
            o: [o[0] as f32, o[1] as f32, o[2] as f32],
            ax: [ax[0] as f32, ax[1] as f32, ax[2] as f32],
            r0: *r0 as f32,
            z0: *z0 as f32,
            r1: *r1 as f32,
            z1: *z1 as f32,
        }),
    }
}

/// **Synthese des `L`-Blocks eines Stopps.**
///
/// `nodes` sind die Member der Vereinigung in Normalform (nur der KONKAVE
/// Stopp hat eine — `union_form`, alle Vorzeichen `+1`). `center`/`skip_r`
/// beschreiben die Kugel, ausserhalb derer kein Term dieses Stopps mehr gaten
/// kann.
///
/// Der Algorithmus ist eine sequentielle Verfeinerung: jede Facette jedes
/// Members startet als EIN Stück und wird von jedem anderen Member entweder
/// (a) verworfen (beweisbar vergraben), (b) in seine Austritts-Teile zerlegt,
/// oder (c) unangetastet gelassen (nicht darstellbar ⇒ Obermenge ⇒ sicher).
pub fn synthesize_lb(
    inner: &BoundedCsg,
    delta: f32,
    center: [f32; 3],
    skip_r: f32,
) -> Option<LbBlock> {
    let mut nodes: Vec<&BoundedCsg> = Vec::new();
    flatten_union(inner, &mut nodes);
    if nodes.len() < 2 {
        return None;
    }
    let solids: Vec<LbSolid> = nodes.iter().map(|n| lb_solid(n)).collect::<Option<_>>()?;
    let exits: Vec<Vec<LbExit>> = solids.iter().map(|s| s.exits()).collect();
    // Längenmass des Modells für die Toleranzen.
    let mut scale = 1.0f64;
    for s in &solids {
        match s {
            LbSolid::Poly { faces } => {
                for f in faces {
                    for v in &f.ring {
                        scale = scale.max(fl(*v));
                    }
                }
            }
            LbSolid::Cyl { o, r, z0, z1, .. } => {
                scale = scale.max(fl(*o) + r.max(z0.abs()).max(z1.abs()));
            }
        }
    }
    let tol = 1e-9 * scale;
    // Geometrische Achsen-Toleranz (s. [`coaxial`]) — eine Grössenordnung
    // unter dem `slack`, der sie deckt.
    let ctol = 1e-6 * scale;

    let mut out: Vec<LbPiece> = Vec::new();
    for (k, sk) in solids.iter().enumerate() {
        for face in sk.faces() {
            let mut pieces = vec![face.clone()];
            for m in 0..solids.len() {
                if m == k {
                    continue;
                }
                let mut next: Vec<WPiece> = Vec::with_capacity(pieces.len());
                for p in &pieces {
                    // Berührt das Stück das Innere von `P_m` überhaupt?
                    if exits[m].iter().any(|e| contained(p, e, tol)) {
                        next.push(p.clone());
                        continue;
                    }
                    let live: Vec<&LbExit> =
                        exits[m].iter().filter(|e| hits(p, e, tol)).collect();
                    if live.is_empty() {
                        // Beweisbar vergraben (jeder Austritt verfehlt es) ⇒
                        // das Stück ist KEIN Teil von `∂Ω`.
                        continue;
                    }
                    let mut split: Vec<WPiece> = Vec::with_capacity(live.len());
                    let mut ok = true;
                    for e in live {
                        match p.clip(e, ctol) {
                            Ok(Some(q)) => split.push(q),
                            Ok(None) => {}
                            Err(()) => {
                                ok = false;
                                break;
                            }
                        }
                    }
                    if ok {
                        next.extend(split);
                    } else {
                        next.push(p.clone());
                    }
                }
                pieces = next;
                if pieces.len() > MAX_LB_PIECES {
                    // Budget gerissen: zurück auf die UNGESCHNITTENE Facette.
                    // Das ist eine Obermenge des wahren Randstücks, also
                    // sicher — nur schwächer.
                    pieces = vec![face.clone()];
                    break;
                }
            }
            for p in &pieces {
                emit_piece(p, &mut out);
            }
            if out.len() > MAX_LB_PIECES {
                return None;
            }
        }
    }
    if out.is_empty() {
        return None;
    }
    Some(LbBlock {
        pieces: out,
        delta,
        slack: (1e-5 * scale as f32).max(1e-5),
        center,
        skip_r,
    })
}

// ─── Fusspunkte (Dual-Spiegel: Enveloppen-Satz) ──────────────────────────────

/// Nächster Punkt eines Dreiecks (Ericson, *Real-Time Collision Detection*
/// §5.1.5) — die baryzentrische Fallunterscheidung, nicht die Distanzformel.
#[inline]
pub fn tri_foot(p: [f32; 3], a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> [f32; 3] {
    let ab = sub(b, a);
    let ac = sub(c, a);
    let ap = sub(p, a);
    let d1 = dot(ab, ap);
    let d2 = dot(ac, ap);
    let lerp = |o: [f32; 3], v: [f32; 3], t: f32| [o[0] + v[0] * t, o[1] + v[1] * t, o[2] + v[2] * t];
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = sub(p, b);
    let d3 = dot(ab, bp);
    let d4 = dot(ac, bp);
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return lerp(a, ab, d1 / (d1 - d3));
    }
    let cp = sub(p, c);
    let d5 = dot(ab, cp);
    let d6 = dot(ac, cp);
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return lerp(a, ac, d2 / (d2 - d6));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        return lerp(b, sub(c, b), (d4 - d3) / ((d4 - d3) + (d5 - d6)));
    }
    let den = 1.0 / (va + vb + vc);
    let v = vb * den;
    let w = vc * den;
    [
        a[0] + ab[0] * v + ac[0] * w,
        a[1] + ab[1] * v + ac[1] * w,
        a[2] + ab[2] * v + ac[2] * w,
    ]
}

/// Nächster Punkt einer Rotationsfläche: `(r*, z*)` auf dem Profil-Segment,
/// zurückgedreht in DIE Halbebene von `p`.
#[inline]
pub fn rev_foot(
    p: [f32; 3],
    o: [f32; 3],
    ax: [f32; 3],
    r0: f32,
    z0: f32,
    r1: f32,
    z1: f32,
) -> [f32; 3] {
    let d = sub(p, o);
    let t = dot(d, ax);
    let rad = [d[0] - ax[0] * t, d[1] - ax[1] * t, d[2] - ax[2] * t];
    let r = norm(rad);
    let (ex, ey) = (r1 - r0, z1 - z0);
    let (px, py) = (r - r0, t - z0);
    let ll = ex * ex + ey * ey;
    let s = if ll > 1e-20 {
        ((px * ex + py * ey) / ll).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (rs, zs) = (r0 + ex * s, z0 + ey * s);
    // Radiale Einheitsrichtung; auf der Achse ist jede Richtung nächster Punkt
    // — dann eine beliebige Senkrechte (der Distanzwert ist dort ohnehin
    // richtungsunabhängig).
    let u = if r > 1e-9 {
        [rad[0] / r, rad[1] / r, rad[2] / r]
    } else {
        let alt = if ax[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
        let w = cross(ax, alt);
        let n = norm(w).max(1e-20);
        [w[0] / n, w[1] / n, w[2] / n]
    };
    [
        o[0] + ax[0] * zs + u[0] * rs,
        o[1] + ax[1] * zs + u[1] * rs,
        o[2] + ax[2] * zs + u[2] * rs,
    ]
}

/// Der Fusspunkt des Stücks, das das `min` gewinnt — `None`, wenn der Block
/// neutral ist (ausserhalb `Ω` oder ausserhalb der Term-Kugel).
///
/// Der Dual-Pfad friert ihn als Konstante ein: `∂|x−q*|/∂x = (x−q*)/|x−q*|`
/// ist der EXAKTE Gradient der Distanzfunktion (Enveloppen-Satz), genau wie
/// beim Kanal-Fusspunkt `q*` in `sdf_dual`.
pub fn lb_foot(p: [f32; 3], base: f32, blk: &LbBlock) -> Option<[f32; 3]> {
    if base > blk.delta || norm(sub(p, blk.center)) > blk.skip_r {
        return None;
    }
    let mut best = f32::INFINITY;
    let mut foot = None;
    for piece in &blk.pieces {
        let d = piece_dist(p, piece);
        if d < best {
            best = d;
            foot = Some(match *piece {
                LbPiece::Tri { a, b, c } => tri_foot(p, a, b, c),
                LbPiece::Rev { o, ax, r0, z0, r1, z1 } => rev_foot(p, o, ax, r0, z0, r1, z1),
            });
        }
    }
    foot
}
