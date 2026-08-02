//! #384 §8x / P5a — **Der Aussenzeugen-Kern für gerade Nähte.**
//!
//! §8j steht auf einem Satz, der bis heute keiner ist: *„Jeder Naht-Punkt liegt
//! auf `∂Ω`, also gilt `dist(x,∁Ω) ≤ de` bedingungslos."* §8s hat ihn
//! ausdrücklich als **Modell-Annahme auf Trace-Auflösung** benannt, §8u hat
//! nachgetragen, warum er sich mit Feld-Mitteln nicht nachreichen lässt:
//! **`|f| ≤ ε` beweist keine Randzugehörigkeit** — beim Pseudo-SDF ist ein
//! kleiner Feldwert kein Zeuge für irgendetwas.
//!
//! Dieses Modul liefert den fehlenden Zeugen, und zwar **geometrisch**: zu
//! jedem Punkt `q` der modellierten Naht `Q` eine Abbildung `y(q) ∈ ∁Ω` mit
//! `|q − y(q)| ≤ η`. Daraus folgt `dist(q, ∁Ω) ≤ η` — die Aussage, die §8j
//! braucht, nur eben mit einem ehrlichen `η` statt mit `0`.
//!
//! ## Was P5a NICHT tut
//!
//! **Keine Feldänderung.** `channel_limit`, `channel_apply` und jeder
//! Evaluator bleiben unangetastet; das Golden ist byte-identisch. Die
//! η-Klemme (`δ − de − η`) ist P5b, und sie braucht zusätzlich die
//! f32-Brücke `de_real → de_f32` — der Platz dafür ist in [`EtaBudget`]
//! benannt und ausdrücklich **unbelegt**.
//!
//! ## Warum hier NICHT das f32-Modell gilt
//!
//! [`crate::offset_interval`] schliesst den Wert ein, den der **f32-Auswerter
//! liefert** — das ist dort richtig, weil die Zusage über diesen Wert spricht.
//! Hier ist der Gegenstand ein anderer: `Ω` ist eine **geometrische Menge**,
//! kein Feldwert. Ob ein Punkt in `∁Ω` liegt, ist eine Aussage über reelle
//! Koordinaten; sie durch das f32-Gitter zu schicken wäre schlicht die falsche
//! Frage. Dieses Modul rechnet deshalb in **reeller** Intervall-Arithmetik:
//! `next_down`/`next_up` nach jeder Schranke, aber **kein** f32-Schritt.

use crate::bvh::BoundedCsg;
use crate::offset_channel::{ChannelArc, OffsetChannel};
use crate::offset_interval::{next_down, next_up, Iv};
use fielddraft_protocol::{Primitive, Transform};

// ════════════════════════════════════════════════════════════════════════════
// Reelle Intervall-Arithmetik (kein f32-Schritt — s. Modulkopf)
// ════════════════════════════════════════════════════════════════════════════

#[inline]
fn rseal(lo: f64, hi: f64) -> Option<Iv> {
    Iv::new(next_down(lo), next_up(hi))
}

#[inline]
fn radd(a: Iv, b: Iv) -> Option<Iv> {
    rseal(a.lo + b.lo, a.hi + b.hi)
}

#[inline]
fn rsub(a: Iv, b: Iv) -> Option<Iv> {
    rseal(a.lo - b.hi, a.hi - b.lo)
}

#[inline]
fn rmul(a: Iv, b: Iv) -> Option<Iv> {
    let p = [a.lo * b.lo, a.lo * b.hi, a.hi * b.lo, a.hi * b.hi];
    if p.iter().any(|v| v.is_nan()) {
        return None;
    }
    rseal(
        p.iter().copied().fold(f64::INFINITY, f64::min),
        p.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    )
}

#[inline]
fn rdiv(a: Iv, b: Iv) -> Option<Iv> {
    if b.lo <= 0.0 && b.hi >= 0.0 {
        return None;
    }
    let p = [a.lo / b.lo, a.lo / b.hi, a.hi / b.lo, a.hi / b.hi];
    if p.iter().any(|v| v.is_nan()) {
        return None;
    }
    rseal(
        p.iter().copied().fold(f64::INFINITY, f64::min),
        p.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    )
}

/// `√` über einem nichtnegativ geschnittenen Radikanden. Wie §8u: ein
/// negatives `lo` ist ausschliesslich die eigene Aussen-Rundung, das ganze
/// Intervall negativ wäre ein Fehler.
#[inline]
fn rsqrt(a: Iv) -> Option<Iv> {
    if a.hi < 0.0 {
        return None;
    }
    let r = rseal(a.lo.max(0.0).sqrt(), a.hi.max(0.0).sqrt())?;
    Iv::new(r.lo.max(0.0), r.hi.max(0.0))
}

#[inline]
fn rnorm(v: [Iv; 3]) -> Option<Iv> {
    let mut s = Iv::point(0.0);
    for x in v {
        s = radd(s, rmul(x, x)?)?;
    }
    rsqrt(s)
}

#[inline]
fn pt(x: f64) -> Iv {
    Iv::point(x)
}

#[inline]
fn f32p3(v: [f32; 3]) -> [Iv; 3] {
    [pt(v[0] as f64), pt(v[1] as f64), pt(v[2] as f64)]
}

// ════════════════════════════════════════════════════════════════════════════
// Beweispflicht 1 — Aussenmitgliedschaft, rein geometrisch
// ════════════════════════════════════════════════════════════════════════════

/// **Liegt die GANZE Box `b` beweisbar ausserhalb von `Ω(node)`?**
///
/// Der Beweis kommt ausschliesslich aus der CSG-Struktur und den
/// Member-Flächen — **nie** aus einem Feldwert. Die Regeln:
///
/// | Knoten | `b ∩ Ω = ∅` ist bewiesen, wenn … |
/// |---|---|
/// | `Empty` | immer |
/// | `Leaf` Box | eine Achse im lokalen Rahmen TRENNT: `min_b |local_k| > h_k` |
/// | `Leaf` Sphere | `min_b |y − c| > r` |
/// | `Leaf` Cylinder | radial `min_b √(dx²+dz²) > r` ODER axial `min_b |dy| > hh` |
/// | `Union(A,B)` | `A` UND `B` beide bewiesen aussen |
/// | `Intersection(A,B)` | `A` ODER `B` bewiesen aussen |
/// | `Difference(A,B)` | `A` bewiesen aussen (die rechte Seite kann nur wegnehmen) |
/// | `Offset(inner, δ)`, `δ ≤ 0` | `inner` bewiesen aussen (der erodierte Körper ist eine TEILMENGE) |
/// | `InnerWall` | wie das Kind (feld-transparent) |
/// | alles andere | **nicht bewiesen** ⇒ `false` (fail-closed) |
///
/// Jede Vergleichsschranke wird nach aussen gerundet, und verlangt wird eine
/// **strikte** Trennung um mindestens `margin`: eine Berührung ist kein Beweis
/// für „ausserhalb".
///
/// **`margin` muss endlich und `≥ 0` sein** (§8x-Korrektur, achtes Review).
/// Eine negative Marge würde die Strenge aufheben und einen Randpunkt als
/// „aussen" durchgehen lassen — und das ist ein real erreichbarer API-Pfad,
/// kein blosser Mutationstest. Ungültige Margen liefern deshalb `false`:
/// fail-closed, nicht „der Aufrufer wird schon wissen was er tut".
pub fn box_outside_omega(node: &BoundedCsg, b: [Iv; 3], margin: f64) -> bool {
    if !(margin.is_finite() && margin >= 0.0) {
        return false;
    }
    box_outside_inner(node, b, margin)
}

fn box_outside_inner(node: &BoundedCsg, b: [Iv; 3], margin: f64) -> bool {
    let sep = |v: Option<Iv>| -> bool { matches!(v, Some(s) if s.lo > margin) };
    match node {
        BoundedCsg::Empty => true,
        BoundedCsg::Leaf { primitive, world_transform, .. } => {
            let Some(local) = local_box(world_transform, b) else { return false };
            match primitive {
                Primitive::Box { center, half_extents } => (0..3).any(|k| {
                    let d = rsub(local[k], pt(center[k] as f64));
                    sep(d.and_then(|d| rsub(d.abs(), pt(half_extents[k] as f64))))
                }),
                Primitive::Sphere { center, radius } => {
                    let Some(d) = sub3(local, f32p3(*center)) else { return false };
                    sep(rnorm(d).and_then(|n| rsub(n, pt(*radius as f64))))
                }
                Primitive::Cylinder { center, radius, half_height } => {
                    let Some(d) = sub3(local, f32p3(*center)) else { return false };
                    let radial = (|| {
                        let xx = rmul(d[0], d[0])?;
                        let zz = rmul(d[2], d[2])?;
                        let s = radd(xx, zz)?;
                        rsub(rsqrt(s)?, pt(*radius as f64))
                    })();
                    let axial = rsub(d[1].abs(), pt(*half_height as f64));
                    sep(radial) || sep(axial)
                }
                _ => false,
            }
        }
        BoundedCsg::Union { left, right, .. } => {
            box_outside_inner(left, b, margin) && box_outside_inner(right, b, margin)
        }
        BoundedCsg::Intersection { left, right, .. } => {
            box_outside_inner(left, b, margin) || box_outside_inner(right, b, margin)
        }
        // Die rechte Seite kann nur Material WEGNEHMEN; wer links draussen ist,
        // ist auch im Ergebnis draussen.
        BoundedCsg::Difference { left, .. } => box_outside_inner(left, b, margin),
        // `δ ≤ 0`: der Offset-Körper ist `{sdf ≤ δ} ⊆ {sdf ≤ 0}` = das Kind.
        // Für `δ > 0` ist er GRÖSSER, und die Schlussrichtung dreht sich um —
        // dann ist hier nichts bewiesen.
        BoundedCsg::Offset { inner, delta, .. } => {
            *delta <= 0.0 && box_outside_inner(inner, b, margin)
        }
        BoundedCsg::InnerWall { inner, .. } => box_outside_inner(inner, b, margin),
        _ => false,
    }
}

#[inline]
fn sub3(a: [Iv; 3], b: [Iv; 3]) -> Option<[Iv; 3]> {
    Some([rsub(a[0], b[0])?, rsub(a[1], b[1])?, rsub(a[2], b[2])?])
}

/// `world_to_local` in REELLER Intervall-Arithmetik. Für die Identität
/// (`q = (0,0,0,1)`, `scale = 1`) ist das Ergebnis exakt.
fn local_box(t: &Transform, p: [Iv; 3]) -> Option<[Iv; 3]> {
    let v = sub3(p, f32p3(t.translate))?;
    let (qx, qy, qz, qw) = (
        pt(-(t.rotation[0] as f64)),
        pt(-(t.rotation[1] as f64)),
        pt(-(t.rotation[2] as f64)),
        pt(t.rotation[3] as f64),
    );
    let two = pt(2.0);
    let mk = |a: Iv, x: Iv, bb: Iv, y: Iv| -> Option<Iv> {
        rmul(two, rsub(rmul(a, x)?, rmul(bb, y)?)?)
    };
    let t0 = mk(qy, v[2], qz, v[1])?;
    let t1 = mk(qz, v[0], qx, v[2])?;
    let t2 = mk(qx, v[1], qy, v[0])?;
    let comb = |vk: Iv, base: Iv, a: Iv, bb: Iv, x: Iv, y: Iv| -> Option<Iv> {
        let s = radd(vk, rmul(qw, base)?)?;
        let s = radd(s, rmul(a, x)?)?;
        rsub(s, rmul(bb, y)?)
    };
    let e0 = comb(v[0], t0, qy, qz, t2, t1)?;
    let e1 = comb(v[1], t1, qz, qx, t0, t2)?;
    let e2 = comb(v[2], t2, qx, qy, t1, t0)?;
    Some([
        rdiv(e0, pt(t.scale[0] as f64))?,
        rdiv(e1, pt(t.scale[1] as f64))?,
        rdiv(e2, pt(t.scale[2] as f64))?,
    ])
}

// ════════════════════════════════════════════════════════════════════════════
// Beweispflicht 4 — das Box-Ball-Lemma (Baustein für P6a)
// ════════════════════════════════════════════════════════════════════════════

/// **Der grösste Abstand einer achsenparallelen Zelle zu einem Punkt**,
/// nach oben gerichtet gerundet.
///
/// **Lemma.** Für `C = ∏[lo_k, hi_k]` und festes `y` ist
/// `|x − y|² = Σ_k (x_k − y_k)²` **separabel**, und jeder Summand ist als
/// konvexe Funktion von `x_k` auf `[lo_k, hi_k]` an einem Endpunkt maximal.
/// Also wird das Maximum **an einer Ecke** angenommen, und zwar
/// komponentenweise an derjenigen, die je Achse weiter von `y_k` weg liegt:
/// `max_k = max(|lo_k − y_k|, |hi_k − y_k|)`. ∎
///
/// Die untere Schranke ist die übliche Punkt-Box-Distanz
/// (`max(lo_k − y_k, 0, y_k − hi_k)`); beide werden **gerichtet** gerundet —
/// die obere aufwärts, die untere abwärts.
pub fn box_point_dist_bounds(lo: [f64; 3], hi: [f64; 3], y: [f64; 3]) -> (f64, f64) {
    let (mut smin, mut smax) = (0.0f64, 0.0f64);
    for k in 0..3 {
        // Oben: die weiter entfernte Seite.
        let far = next_up((lo[k] - y[k]).abs()).max(next_up((hi[k] - y[k]).abs()));
        smax = next_up(smax + next_up(far * far));
        // Unten: null, wenn `y_k` im Intervall liegt.
        let near = next_down((lo[k] - y[k]).max(0.0).max(y[k] - hi[k]));
        let near = near.max(0.0);
        smin = next_down(smin + next_down(near * near));
    }
    (next_down(smin.max(0.0).sqrt()), next_up(smax.sqrt()))
}

// ════════════════════════════════════════════════════════════════════════════
// Die Zeugen-Konstruktion
// ════════════════════════════════════════════════════════════════════════════

/// **Das geometrische η — ein geschützter Beweiswert.**
///
/// `dist(q, ∁Ω) ≤ mm()` für JEDEN Punkt `q` der abgedeckten Naht. Der Wert
/// entsteht ausschliesslich in [`cover_channel`] aus einer vollständigen,
/// lückenlosen Abdeckung; er ist von aussen nicht konstruierbar.
///
/// **Warum die Felder privat sind** (§8x-Korrektur, achtes Review): die erste
/// Fassung war ein `struct` mit öffentlichen Feldern und einer Methode
/// `total_for_clamp()`, die `None` lieferte, solange die f32-Brücke unbelegt
/// war. Die Doku behauptete daraufhin „kein Aufrufer bekommt eine Zahl, die er
/// noch nicht verwenden darf" — technisch falsch: jeder konnte
/// `dist_bridge: Some(0.0)` hinschreiben und die Klemme freischalten. Ein
/// Beweiswert, den der Aufrufer selbst zusammensetzen kann, ist kein
/// Beweiswert.
///
/// Deshalb: `GeometricEta` ist **opak**, und einen `ClampEta` gibt es hier gar
/// nicht. Ihn zu bilden ist P5b vorbehalten — aus DIESEM Wert plus einer dort
/// herzuleitenden f32-Brücke `de_real → de_f32`, und auf keinem anderen Weg.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GeometricEta {
    offset: f64,
    cover: f64,
    coord: f64,
}

impl GeometricEta {
    /// `η` in mm — die Zusage `dist(q, ∁Ω) ≤ η`.
    pub fn mm(&self) -> f64 {
        next_up(next_up(self.offset + self.cover) + self.coord)
    }
    /// Der Zeugen-Versatz-Anteil (Berichterstattung).
    pub fn offset_mm(&self) -> f64 {
        self.offset
    }
    /// Der Abdeckungs-Anteil — bei dieser Konstruktion **exakt null**, und das
    /// ist eine Aussage, kein Glück: die Zeugen sind keine Punkte, sondern eine
    /// ABBILDUNG über einem ganzen Naht-Stück. Jedes `q` des Stücks hat seinen
    /// eigenen Zeugen in derselben nachgewiesenen Aussen-Box. Eine Familie von
    /// Zeugen-PUNKTEN hätte hier `≥ Stücklänge/2` stehen.
    pub fn cover_mm(&self) -> f64 {
        self.cover
    }
    /// Die f64-Rundung des Stück-Box-Aufbaus — von der Aussen-Rundung der
    /// Intervalle aufgefangen und deshalb bereits in `offset` enthalten; hier
    /// separat, damit sie nicht vergessen wird.
    pub fn coord_mm(&self) -> f64 {
        self.coord
    }
}

/// Ein bewiesenes Naht-Stück: die Parameter-Spanne und ihr Zeugen-Versatz.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CoveredPiece {
    pub arc: usize,
    pub t0: f64,
    pub t1: f64,
    /// **Der tatsächlich geprüfte Verschiebungsvektor**, komponentenweise in
    /// f64. Die Abbildung ist `y(q) = q + shift` — buchstäblich mit DIESEN
    /// Zahlen, denn genau aus ihnen wurde die Aussen-Box gebaut.
    ///
    /// §8x-Korrektur (achtes Review): vorher standen hier eine Richtung `d`
    /// und ein Skalar `off = next_up(t·|d|)`, und der Konsument rekonstruierte
    /// `q + off·d` — einen ANDEREN Punkt als den bewiesenen `q + t·d`. Über
    /// einer nichtkonvexen Union folgt aus der Aussenlage bei `t·d` NICHT die
    /// Aussenlage ein Stück weiter in derselben Richtung; es gab kein Lemma,
    /// das die beiden verbindet. Jetzt sind Beweis, Auslieferung und Budget
    /// **dieselben Daten**.
    pub shift: [f64; 3],
    /// Eine OBERE Schranke von `|shift|` — das, was ins η-Budget eingeht.
    pub offset: f64,
}

/// Das Ergebnis für EINEN Kanal.
#[derive(Debug, Clone, PartialEq)]
pub struct WitnessCover {
    pub pieces: usize,
    pub covered: usize,
    /// Naht-Länge gesamt und ungedeckt (mm).
    pub seam_len: f64,
    pub uncovered_len: f64,
    /// `None`, sobald auch nur ein Stück ohne Zeugen bleibt — eine Zusage über
    /// „die Naht" ist nur so viel wert wie ihr schlechtestes Stück.
    pub eta: Option<GeometricEta>,
    pub covered_pieces: Vec<CoveredPiece>,
}

/// Stellschrauben der Suche.
#[derive(Debug, Clone, Copy)]
pub struct WitnessCfg {
    /// Stücke je Bogen.
    pub pieces_per_arc: usize,
    /// Kleinster und grösster Versatz der Leiter (mm).
    pub min_offset: f64,
    pub max_offset: f64,
    /// Stufen der geometrischen Leiter.
    pub steps: usize,
    /// Verlangte strikte Trennung im Aussen-Beweis (mm).
    pub margin: f64,
}

impl WitnessCfg {
    /// **Ist diese Konfiguration überhaupt verwendbar?** (§8x-Korrektur,
    /// achtes Review.)
    ///
    /// Die erste Fassung prüfte gar nichts, und das war nicht bloss unsauber:
    /// bei einem NEGATIVEN `min_offset` konnte die Suche einen echten Zeugen
    /// finden, `|shift|` wäre positiv, aber ein negativ gerechneter Versatz
    /// hätte `worst_offset.max(off)` bei `0` gelassen — `η = 0` trotz
    /// positiven Abstands. Eine Zusage, die aus einer kaputten Eingabe
    /// entsteht, ist schlimmer als keine.
    pub fn valid(&self) -> bool {
        self.pieces_per_arc > 0
            && self.steps > 0
            && self.min_offset.is_finite()
            && self.max_offset.is_finite()
            && self.margin.is_finite()
            && self.min_offset > 0.0
            && self.min_offset <= self.max_offset
            && self.margin >= 0.0
    }
}

/// Ein Bogen, den die Konstruktion überhaupt anfassen darf: gerades Segment,
/// endliche Koeffizienten, nicht entartet.
fn arc_usable(arc: &ChannelArc) -> bool {
    arc.sweep < 0.0
        && arc.r.is_finite()
        && arc.r as f64 >= 1.0e-9
        && arc.c.iter().all(|v| v.is_finite())
        && arc.u.iter().all(|v| v.is_finite())
        && arc.bc.iter().all(|v| v.is_finite())
        && arc.br.is_finite()
}

impl Default for WitnessCfg {
    fn default() -> Self {
        WitnessCfg {
            pieces_per_arc: 24,
            min_offset: 1.0e-4,
            max_offset: 4.0,
            steps: 18,
            margin: 1.0e-9,
        }
    }
}

/// Die Box eines Naht-Stücks `{c + û·(s·r) : s ∈ [t0,t1]}`, reell
/// eingeschlossen.
fn piece_box(arc: &ChannelArc, t0: f64, t1: f64) -> Option<[Iv; 3]> {
    let r = arc.r as f64;
    let s = Iv::new(next_down(t0 * r), next_up(t1 * r))?;
    let mut out = [pt(0.0); 3];
    for k in 0..3 {
        out[k] = radd(pt(arc.c[k] as f64), rmul(pt(arc.u[k] as f64), s)?)?;
    }
    Some(out)
}

fn nlerp64(a: [f32; 3], b: [f32; 3], t: f64) -> Option<[f64; 3]> {
    let v = [
        a[0] as f64 + (b[0] - a[0]) as f64 * t,
        a[1] as f64 + (b[1] - a[1]) as f64 * t,
        a[2] as f64 + (b[2] - a[2]) as f64 * t,
    ];
    let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    (n > 1.0e-12).then(|| [v[0] / n, v[1] / n, v[2] / n])
}

/// Der Richtungs-Vorrat: die 26 Gitter-Richtungen plus die aus den
/// Dual-Vektoren des Kanals abgeleiteten Kandidaten (beide Vorzeichen).
///
/// Die Dual-Vektoren sind **Hinweise, keine Annahmen** — sie verkürzen die
/// Suche, aber die Gültigkeit eines Zeugen hängt ausschliesslich am
/// geometrischen Beweis in [`box_outside_omega`].
fn directions(arc: &ChannelArc, tm: f64) -> Vec<[f64; 3]> {
    let mut out: Vec<[f64; 3]> = Vec::with_capacity(40);
    if let (Some(m1), Some(m2)) = (
        nlerp64(arc.m1s, arc.m1e, tm),
        nlerp64(arc.m2s, arc.m2e, tm),
    ) {
        let sum = [m1[0] + m2[0], m1[1] + m2[1], m1[2] + m2[2]];
        let n = (sum[0] * sum[0] + sum[1] * sum[1] + sum[2] * sum[2]).sqrt();
        for v in [Some(m1), Some(m2), (n > 1e-12).then(|| [sum[0] / n, sum[1] / n, sum[2] / n])]
            .into_iter()
            .flatten()
        {
            out.push(v);
            out.push([-v[0], -v[1], -v[2]]);
        }
    }
    for ix in -1i32..=1 {
        for iy in -1i32..=1 {
            for iz in -1i32..=1 {
                if ix == 0 && iy == 0 && iz == 0 {
                    continue;
                }
                let n = ((ix * ix + iy * iy + iz * iz) as f64).sqrt();
                out.push([ix as f64 / n, iy as f64 / n, iz as f64 / n]);
            }
        }
    }
    out
}

/// **Die Zeugen-Abdeckung EINES Kanals.**
///
/// Für jedes Naht-Stück `P` wird eine Richtung `d` und ein Versatz `t` gesucht,
/// für die die **ganze** verschobene Stück-Box `P + t·d` beweisbar in `∁Ω`
/// liegt. Gelingt das, ist `y(q) = q + t·d` für jedes `q ∈ P` ein Aussenzeuge
/// mit `|q − y(q)| = t` — eine ABBILDUNG, kein Punkt, und deshalb ohne
/// Abdeckungs-Fehler (s. [`EtaBudget::cover`]).
///
/// Gekrümmte Bögen sind nicht unterstützt (P4); der Kanal wird dann als
/// vollständig ungedeckt gemeldet, nicht als gedeckt.
pub fn cover_channel(omega: &BoundedCsg, ch: &OffsetChannel, cfg: WitnessCfg) -> WitnessCover {
    let mut cover = WitnessCover {
        pieces: 0,
        covered: 0,
        seam_len: 0.0,
        uncovered_len: 0.0,
        eta: None,
        covered_pieces: Vec::new(),
    };
    // Ungültige Konfiguration ⇒ vollständig ungedeckt, `eta = None`. Kein
    // Rechnen auf kaputten Parametern.
    if !cfg.valid() {
        cover.pieces = ch.arcs.len().max(1);
        cover.seam_len = ch.arcs.iter().map(|a| a.r as f64).filter(|v| v.is_finite()).sum();
        cover.uncovered_len = cover.seam_len;
        return cover;
    }
    let mut worst_offset = 0.0f64;
    let mut all_ok = !ch.arcs.is_empty();
    for (ai, arc) in ch.arcs.iter().enumerate() {
        let len = if arc.r.is_finite() { arc.r as f64 } else { 0.0 };
        cover.seam_len += len;
        if !arc_usable(arc) {
            // Gekrümmt (P4) oder entartet. Ehrlich als ungedeckt zählen.
            cover.pieces += cfg.pieces_per_arc;
            cover.uncovered_len += len;
            all_ok = false;
            continue;
        }
        for i in 0..cfg.pieces_per_arc {
            let t0 = i as f64 / cfg.pieces_per_arc as f64;
            let t1 = (i + 1) as f64 / cfg.pieces_per_arc as f64;
            cover.pieces += 1;
            let plen = len / cfg.pieces_per_arc as f64;
            let Some(pb) = piece_box(arc, t0, t1) else {
                cover.uncovered_len += plen;
                all_ok = false;
                continue;
            };
            let dirs = directions(arc, 0.5 * (t0 + t1));
            let mut found: Option<([f64; 3], f64)> = None;
            'ladder: for s in 0..cfg.steps {
                let frac = s as f64 / (cfg.steps - 1).max(1) as f64;
                let t = cfg.min_offset * (cfg.max_offset / cfg.min_offset).powf(frac);
                for d in &dirs {
                    // Der Verschiebungsvektor, EINMAL berechnet — und genau er
                    // geht sowohl in die geprüfte Box als auch in die
                    // Auslieferung und ins Budget.
                    let shift = [t * d[0], t * d[1], t * d[2]];
                    if !shift.iter().all(|v| v.is_finite()) {
                        continue;
                    }
                    let mut wb = [pt(0.0); 3];
                    let mut ok = true;
                    for k in 0..3 {
                        match radd(pb[k], pt(shift[k])) {
                            Some(v) => wb[k] = v,
                            None => {
                                ok = false;
                                break;
                            }
                        }
                    }
                    if ok && box_outside_omega(omega, wb, cfg.margin) {
                        // `|shift|` nach OBEN — und zwar aus demselben Vektor,
                        // nicht aus `t` und einer nominellen Einheitslänge.
                        let Some(nrm) = rnorm([pt(shift[0]), pt(shift[1]), pt(shift[2])]) else {
                            continue;
                        };
                        if !nrm.hi.is_finite() {
                            continue;
                        }
                        found = Some((shift, nrm.hi));
                        break 'ladder;
                    }
                }
            }
            match found {
                Some((shift, off)) => {
                    cover.covered += 1;
                    worst_offset = worst_offset.max(off);
                    cover.covered_pieces.push(CoveredPiece { arc: ai, t0, t1, shift, offset: off });
                }
                None => {
                    cover.uncovered_len += plen;
                    all_ok = false;
                }
            }
        }
    }
    if all_ok && cover.covered == cover.pieces && cover.pieces > 0 && worst_offset.is_finite() {
        cover.eta = Some(GeometricEta { offset: worst_offset, cover: 0.0, coord: 0.0 });
    }
    cover
}

/// `Ω` eines Stopp-Baums: das Kind des `Offset`-Knotens unter dem Kanal —
/// also der Körper VOR der Erosion, über den §8j spricht.
///
/// `None`, wenn der Baum nicht diese Form hat (fail-closed).
pub fn omega_of(stop_inner: &BoundedCsg) -> Option<&BoundedCsg> {
    match stop_inner {
        BoundedCsg::Offset { inner, delta, .. } if *delta <= 0.0 => Some(inner),
        _ => None,
    }
}
