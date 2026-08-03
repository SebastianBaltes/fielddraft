//! #384 §8x / P5a — **Der Aussenzeugen-Kern für gerade Nähte**, und
//! #384 §8aa / P5b-0 — **die f32-Brücke und der Klemm-Typ darüber.**
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
//! ## Was P5b-0 dazulegt — und was es NICHT tut
//!
//! P5a endet bei der modellierten Naht. P5b-0 baut die Brücke von dort zu dem,
//! was die Produktion wirklich rechnet: [`EtaClamp`] trägt `η_geo` plus die
//! drei f32-Posten `ε_q` (Punkt-Brücke), `ε_d` (Norm-Brücke) und
//! `field_round` (Rundungsbudget der EINGEFROREN definierten Feldkette,
//! s. [`frozen_channel_limit`]) und liefert daraus einen `f32`-Addenden.
//!
//! **Immer noch keine Feldänderung.** `channel_limit`, `channel_apply` und
//! jeder Evaluator bleiben unangetastet; das Golden ist byte-identisch. Kein
//! Konsument liest [`OffsetChannel::eta_addend`] — der Verbrauch ist P5b-1,
//! der Intervallspiegel P5b-2, die Bogen-Zeugen P5c.
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
use crate::offset_channel::{
    channel_nearest, face_plane_gate, norm as norm32, sub as sub32, ChannelArc, OffsetChannel,
    END_PLANE_OFF,
};
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
/// Abdeckungs-Fehler (s. [`GeometricEta::cover_mm`]).
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

// ════════════════════════════════════════════════════════════════════════════
// §8aa / P5b-0 — die f32-Brücke, das Rundungsbudget und der Klemm-Typ
// ════════════════════════════════════════════════════════════════════════════
//
// P5a endet mit einer Aussage über die MODELLIERTE Naht `Q`:
// `dist(q, ∁Ω) ≤ η_geo` für jedes `q ∈ Q`. Die Produktion sieht `Q` nie. Sie
// sieht einen f32-Fusspunkt `q_f32` und eine f32-Norm `de`, und zwischen
// beiden Welten liegen genau drei Posten:
//
// * `ε_q` — wie weit `q_f32` von einem WIRKLICHEN Punkt der Naht abliegt;
// * `ε_d` — wie weit die f32-Norm den reellen Abstand `|x − q_f32|`
//   UNTERSCHÄTZEN kann;
// * `field_round` — was die spätere Feldkette `fl(δ − min(fl(de+A), h))` an
//   Rundung obendrauf legt.
//
// Erst die Summe darf geklemmt werden. Ein blosses `ceil_f32(η_geo)` wäre
// genau der Fudge-Faktor, den die Doktrin verbietet.

/// `u = 2⁻²⁴` — die Einheitsrundung von binary32 unter
/// Round-to-Nearest-Even. Für jedes reelle `z` gilt
/// `|fl32(z) − z| ≤ u·|z| + 2⁻¹⁵⁰`.
pub const F32_U: f64 = 1.0 / 16_777_216.0;

/// `2⁻¹⁵⁰` — die HALBE kleinste Subnormale von binary32, also die absolute
/// Fehlerschranke im Unterlaufbereich. Der Wert ist bitgenau `2⁻¹⁵⁰`
/// (gepinnt in `clamp_f32_constants_are_exact_384`).
pub const F32_SUB: f64 = 7.006492321624085e-46;

/// Kleinstes `f32`, das `≥ v` ist. `None` bei nicht-endlichem Ergebnis —
/// eine Klemme aus `inf` ist keine Klemme.
pub(crate) fn f32_ceil(v: f64) -> Option<f32> {
    if !v.is_finite() || v < 0.0 {
        return None;
    }
    let mut a = v as f32;
    if !a.is_finite() {
        return None;
    }
    // Höchstens ein Schritt nötig (RN kann nur um ein halbes ulp danebenliegen),
    // die Schleife ist die ehrliche Variante ohne dieses Argument.
    let mut guard = 0;
    while (a as f64) < v {
        a = f32::from_bits(a.to_bits() + 1);
        guard += 1;
        if guard > 4 || !a.is_finite() {
            return None;
        }
    }
    Some(a)
}

// ─── Lemma 2 — die Punkt-Brücke `ε_q` ───────────────────────────────────────

/// **Lemma 2 (Punkt-Brücke).** Für jeden geraden Bogen und jeden
/// Auswertungspunkt liegt der von [`crate::offset_channel::arc_closest`]
/// zurückgegebene Fusspunkt `q_f32` höchstens `ε_q` von einem **wirklichen**
/// Punkt der modellierten Naht entfernt.
///
/// **Warum das ohne „arc_closest rechnet den exakten Nearest-Punkt" auskommt.**
/// Die Produktion klemmt `t` auf `[0,1]`. Der reelle Punkt `q(t) = c + u·(t·r)`
/// liegt damit ohne jede weitere Annahme IN der modellierten Menge
/// `Q = {c + u·(s·r) : s ∈ [0,1]}` — derselben Menge, die [`cover_channel`]
/// über [`piece_box`] lückenlos kachelt. Ob `t` der exakte Projektions-
/// parameter ist, ist für die Brücke **egal**; Projektions- und Endpunktzweig
/// sind gemeinsam abgedeckt. Genau deshalb trägt die Zuordnungs-Abweichung
/// `ε_D` (Diamant/nlerp) hier keinen Posten: `channel_limit` liest `q` gar
/// nicht, sondern nur `de`, und `de` ist die Norm zu DIESEM `q_f32`.
///
/// **`len == r` ist ein Satz, keine Hoffnung.** `arc_closest` rechnet mit
/// `len = arc.r.max(1e-12)`; [`arc_usable`] verlangt `r ≥ 10⁻⁹ > 10⁻¹²`, also
/// ist `len` **bitweise** `arc.r`. Ohne diesen Schritt spräche die Produktion
/// über ein anderes Segment als der Beweis.
///
/// **Die Kette, op für op** (`arc_closest`, gerader Zweig, Komponente `k`):
///
/// ```text
/// ã = fl(t·r)          |ã − t·r| ≤ u·r + 2⁻¹⁵⁰                =: e₁
/// b̃ = fl(u_k·ã)        |b̃ − u_k·ã| ≤ u·|u_k·ã| + 2⁻¹⁵⁰        =: e₂
/// q̃ = fl(c_k + b̃)      |q̃ − (c_k+b̃)| ≤ u·|c_k+b̃| + 2⁻¹⁵⁰     =: e₃
/// ```
///
/// und damit `|q̃_k − (c_k + u_k·t·r)| ≤ e₃ + e₂ + |u_k|·e₁`. Die Norm über
/// die drei Komponenten wird nach OBEN gerundet. `t ∈ [0,1]` liefert
/// `|t·r| ≤ r` — die einzige Stelle, an der die Klemmung eingeht.
pub(crate) fn point_bridge_mm(arc: &ChannelArc) -> Option<f64> {
    if !arc_usable(arc) {
        return None;
    }
    let r = arc.r as f64;
    if !(r >= 1.0e-9) {
        return None;
    }
    let mut sum = 0.0f64;
    for k in 0..3 {
        let uk = (arc.u[k] as f64).abs();
        let ck = (arc.c[k] as f64).abs();
        let e1 = next_up(next_up(F32_U * r) + F32_SUB);
        let a_hi = next_up(r + e1);
        let b_hi = next_up(uk * a_hi);
        let e2 = next_up(next_up(F32_U * b_hi) + F32_SUB);
        let err_b = next_up(e2 + next_up(uk * e1));
        let c_hi = next_up(next_up(ck + b_hi) + e2);
        let e3 = next_up(next_up(F32_U * c_hi) + F32_SUB);
        let err = next_up(e3 + err_b);
        sum = next_up(sum + next_up(err * err));
    }
    let v = next_up(sum.sqrt());
    (v.is_finite() && v >= 0.0).then_some(v)
}

// ─── Lemma 3 — die Norm-Brücke `ε_d` ────────────────────────────────────────

/// **Lemma 3 (Norm-Brücke).** Für jeden Punkt `x` und den Fusspunkt `q_f32`
/// unterschätzt die f32-Norm den reellen Abstand höchstens um `ε_d`:
/// `|x − q_f32| ≤ de + ε_d`.
///
/// **Herleitung.** `d_k = fl(x_k − q_k)` ist eine Subtraktion ZWEIER f32-Zahlen:
/// liegt das exakte Ergebnis subnormal, ist es exakt darstellbar (Hauser), also
/// gilt hier `|d_k − (x_k−q_k)| ≤ u·|x_k−q_k|` **ohne** Unterlauf-Posten. Für
/// Quadrat, die beiden Additionen und die Wurzel greift je
/// `fl(z) ≥ z(1−u) − 2⁻¹⁵⁰`:
///
/// ```text
/// Σ s_k ≥ D²(1−u)³ − 3·2⁻¹⁵⁰
/// S     ≥ D²(1−u)⁵ − 5·2⁻¹⁵⁰
/// de    ≥ √S·(1−u) − 2⁻¹⁵⁰
/// ⇒ D   ≤ (de + 2⁻¹⁵⁰)·(1−u)^{−7/2} + √(6·2⁻¹⁵⁰·(1−u)^{−5})
/// ```
///
/// Die Wurzel darf **nicht** weggelassen werden: `√` ist bei `S ≈ 0` nicht
/// Lipschitz, ein absoluter Fehler `ε_S` im Radikanden schlägt dort als
/// `√ε_S` durch. Der Posten ist mit `≈ 6·10⁻²³ mm` bedeutungslos — aber er
/// steht da, weil „bedeutungslos" keine Beweisführung ist.
///
/// `de < trust` ist von `channel_limit` **geprüft** (zwei Ausgänge), deshalb
/// darf `trust` hier als obere Schranke von `de` dienen.
pub(crate) fn norm_bridge_mm(trust: f32) -> Option<f64> {
    let n = trust as f64;
    if !(n.is_finite() && n > 0.0) {
        return None;
    }
    // (1−u)^{−7/2}, nach OBEN.
    let omu = next_down(1.0 - F32_U);
    let mut q = 1.0f64;
    for _ in 0..7 {
        q = next_down(q * omu);
    }
    if !(q > 0.0) {
        return None;
    }
    let inv = next_up(1.0 / next_down(q.sqrt()));
    let head = next_up(next_up(next_up(n + F32_SUB) * inv) - n);
    let tail = next_up(next_up(6.0 * F32_SUB * 1.001).sqrt());
    let v = next_up(head + tail);
    (v.is_finite() && v >= 0.0).then_some(v)
}

// ─── Lemma 5 — das Rundungsbudget der EINGEFRORENEN Feldkette ───────────────

/// **Die eingefrorene Produktionsform von P5b-1** — hier als Referenz, damit
/// das Rundungslemma über eine DEFINITIVE Op-Folge spricht und nicht über eine
/// Absicht.
///
/// Heute (`channel_limit`, unverändert):
///
/// ```text
/// g   = face_plane_gate(p, ch)          // min über vier affine Formen
/// h   = fl(g + δ)
/// lim = fl(δ − min(de, h))
/// ```
///
/// Ab P5b-1 (**genau diese Klammerung, genau diese f32-Ops**):
///
/// ```text
/// g      = face_plane_gate(p, ch)       // unverändert
/// h      = fl(g + δ)                    // unverändert
/// de_eta = fl(de + A)                   // NEU — ein einziger f32-Add
/// lim    = fl(δ − min(de_eta, h))       // unverändert
/// ```
///
/// `A` geht **nur** auf die Nahtdistanz. Die Stützebene `h` ist unabhängig
/// bewiesen (§8j) und wird nicht mit-verschlechtert; `δ − min(de+A, h)` ist
/// strikt schärfer als das pauschale `δ − min(de,h) − A`.
///
/// **`A = 0` ist bitweise das heutige Feld** (`de + 0.0 == de` für jedes
/// `de ≥ 0`, und `de` ist eine Norm). Das ist die Feldneutralitäts-Zusage von
/// P5b-1 für jeden Kanal ohne Klemme — gepinnt, nicht behauptet.
///
/// Diese Funktion hat in P5b-0 **keinen Aufrufer ausserhalb der Pins**. Sie
/// ist der Gegenstand des Rundungslemmas, nicht ein Evaluator.
pub fn frozen_channel_limit(p: [f32; 3], ch: &OffsetChannel, eta_add: f32) -> f32 {
    let dbs = norm32(sub32(p, ch.bound_c)) - ch.bound_r;
    if dbs >= ch.trust {
        return -END_PLANE_OFF;
    }
    let Some((de, _, _, _)) = channel_nearest(p, ch) else {
        return -END_PLANE_OFF;
    };
    if de >= ch.trust {
        return -END_PLANE_OFF;
    }
    frozen_limit_tail(ch.delta, de, face_plane_gate(p, ch) + ch.delta, eta_add)
}

/// **Der arithmetische Schwanz der eingefrorenen Kette** — genau die drei
/// f32-Ops, über die Lemma 5 spricht, ohne jede Geometrie davor.
///
/// [`frozen_channel_limit`] ruft diese Funktion auf; ein Auseinanderdriften
/// von Beweisgegenstand und Feldform ist damit nicht bloss unwahrscheinlich,
/// sondern unmöglich. Die ulp-Familie der Gegenprobe kann `de`, `h` und `δ`
/// so frei wählen, wie es der Beweis behauptet — über die Geometrie käme man
/// an die interessanten Bitmuster gar nicht heran.
#[inline]
pub fn frozen_limit_tail(delta: f32, de: f32, h: f32, eta_add: f32) -> f32 {
    let de_eta = de + eta_add;
    delta - de_eta.min(h)
}

/// **Lemma 5 (Rundungslemma).** Das kleinste `A`, für das die eingefrorene
/// Kette im η-Zweig die bewiesene Untergrenze nie überschreitet.
///
/// Verlangt ist `fl(δ − fl(de+A)) ≤ δ − de − Δ` mit `Δ = η_geo + ε_q + ε_d`.
/// Mit `fl(z) ≤ z + u|z| + 2⁻¹⁵⁰` und `fl(z) ≥ z(1−u) − 2⁻¹⁵⁰` (für `z ≥ 0`)
/// genügt
///
/// ```text
/// (de+A)(1 − 2u − u²) ≥ de + Δ + u·|δ| + 3·2⁻¹⁵⁰
/// ```
///
/// und mit `de < trust` (von `channel_limit` geprüft)
///
/// ```text
/// A ≥ [ trust·(2u+u²) + Δ + u·|δ| + 3·2⁻¹⁵⁰ ] / (1 − 2u − u²).
/// ```
///
/// **Beide** Rundungen sind darin: die Addition `fl(de+A)` kann nach UNTEN
/// gehen (Term `trust·(2u+u²)` und der `de`-Anteil), die Subtraktion
/// `fl(δ − ·)` nach OBEN (Term `u·|δ|` plus der `u·de_eta`-Anteil, der im
/// Nenner steckt). Ein blosses `ceil_f32(η_geo + Brücke)` hätte weder das eine
/// noch das andere.
///
/// Gibt `(field_round_mm, A_f32)` zurück: den reinen Rundungsanteil
/// `A_min − Δ` und das kleinste `f32`, das `A_min` erreicht.
pub(crate) fn round_budget(delta: f32, trust: f32, bridge_total: f64) -> Option<(f64, f32)> {
    let d = (delta as f64).abs();
    let k = trust as f64;
    if !(d.is_finite() && k.is_finite() && k > 0.0) {
        return None;
    }
    if !(bridge_total.is_finite() && bridge_total >= 0.0) {
        return None;
    }
    let u2 = next_up(next_up(2.0 * F32_U) + next_up(F32_U * F32_U));
    let num = next_up(
        next_up(next_up(next_up(k * u2) + bridge_total) + next_up(F32_U * d))
            + next_up(3.0 * F32_SUB),
    );
    let den = next_down(1.0 - u2);
    if !(den > 0.0) {
        return None;
    }
    let a_min = next_up(num / den);
    if !(a_min.is_finite() && a_min >= bridge_total) {
        return None;
    }
    let a = f32_ceil(a_min)?;
    if !(a > 0.0) {
        return None;
    }
    Some((next_up(a_min - bridge_total), a))
}

// ─── Lemma 6 — die Kanalbindung ─────────────────────────────────────────────

#[inline]
fn fnv(h: &mut u64, v: u64) {
    *h ^= v;
    *h = h.wrapping_mul(0x0000_0100_0000_01b3);
}

#[inline]
fn fnv_f32(h: &mut u64, v: f32) {
    // Bitmuster, NICHT der Wert: `-0.0` und `0.0` sind für die Geometrie
    // dasselbe, für einen Fingerabdruck aber verschiedene Daten — und
    // Verschiedenheit darf hier nie unter den Tisch fallen.
    fnv(h, v.to_bits() as u64);
}

#[inline]
fn fnv_f32x3(h: &mut u64, v: [f32; 3]) {
    for x in v {
        fnv_f32(h, x);
    }
}

/// **Der Fingerabdruck eines Kanals** — über ALLE Daten, die `channel_limit`
/// und die Zeugensuche lesen: jeden Bogen, `δ`, `trust`, die Bounding-Sphere
/// und beide Halbraum-Sätze. Die Klemme selbst geht NICHT ein (sonst könnte
/// sie sich nicht mit dem Kanal vergleichen, in dem sie sitzt).
///
/// Er ist die zweite Hälfte der Kanalbindung: die erste ist die Einbettung
/// (der Wert wohnt im Kanal), die zweite diese Probe (er wird beim Lesen
/// gegen den Kanal gehalten). Ein umkopierter oder nachträglich veränderter
/// Kanal fällt durch.
pub fn channel_identity(ch: &OffsetChannel) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    fnv(&mut h, ch.arcs.len() as u64);
    for a in &ch.arcs {
        fnv_f32x3(&mut h, a.c);
        fnv_f32x3(&mut h, a.u);
        fnv_f32x3(&mut h, a.v);
        fnv_f32(&mut h, a.r);
        fnv_f32(&mut h, a.sweep);
        fnv_f32(&mut h, a.e[0]);
        fnv_f32(&mut h, a.e[1]);
        fnv_f32(&mut h, a.d_e);
        fnv_f32x3(&mut h, a.bc);
        fnv_f32(&mut h, a.br);
        fnv_f32x3(&mut h, a.m1s);
        fnv_f32x3(&mut h, a.m2s);
        fnv_f32x3(&mut h, a.m1e);
        fnv_f32x3(&mut h, a.m2e);
    }
    fnv_f32(&mut h, ch.delta);
    fnv(&mut h, ch.concave as u64);
    fnv_f32(&mut h, ch.trust);
    fnv_f32(&mut h, ch.gate);
    fnv_f32x3(&mut h, ch.bound_c);
    fnv_f32(&mut h, ch.bound_r);
    for g in ch.end_planes.iter().chain(ch.face_planes.iter()) {
        for x in g {
            fnv_f32(&mut h, *x);
        }
    }
    fnv(&mut h, ch.branch);
    fnv(&mut h, ch.parents[0]);
    fnv(&mut h, ch.parents[1]);
    h
}

/// Provenienz-Fingerabdruck von `Ω`. Rein dokumentarisch: zur Auswertungszeit
/// gibt es kein `Ω` mehr, gegen das man ihn halten könnte. Die Garantie kommt
/// daher, dass [`certify_channel`] Beweis und Einbettung in EINEM Schritt tut —
/// der Fingerabdruck macht nur nachvollziehbar, WORÜBER bewiesen wurde.
pub fn omega_fingerprint(node: &BoundedCsg) -> u64 {
    fn go(n: &BoundedCsg, h: &mut u64) {
        match n {
            BoundedCsg::Empty => fnv(h, 1),
            BoundedCsg::Leaf { primitive, world_transform, .. } => {
                fnv(h, 2);
                fnv_f32x3(h, world_transform.translate);
                for x in world_transform.rotation {
                    fnv_f32(h, x);
                }
                fnv_f32x3(h, world_transform.scale);
                match primitive {
                    Primitive::Box { center, half_extents } => {
                        fnv(h, 21);
                        fnv_f32x3(h, *center);
                        fnv_f32x3(h, *half_extents);
                    }
                    Primitive::Sphere { center, radius } => {
                        fnv(h, 22);
                        fnv_f32x3(h, *center);
                        fnv_f32(h, *radius);
                    }
                    Primitive::Cylinder { center, radius, half_height } => {
                        fnv(h, 23);
                        fnv_f32x3(h, *center);
                        fnv_f32(h, *radius);
                        fnv_f32(h, *half_height);
                    }
                    // Andere Blätter kann `box_outside_omega` ohnehin nicht
                    // beweisen; sie tragen nur ihren Diskriminator.
                    _ => fnv(h, 29),
                }
            }
            BoundedCsg::Union { left, right, .. } => {
                fnv(h, 3);
                go(left, h);
                go(right, h);
            }
            BoundedCsg::Intersection { left, right, .. } => {
                fnv(h, 4);
                go(left, h);
                go(right, h);
            }
            BoundedCsg::Difference { left, right, .. } => {
                fnv(h, 5);
                go(left, h);
                go(right, h);
            }
            BoundedCsg::Offset { inner, delta, .. } => {
                fnv(h, 6);
                fnv_f32(h, *delta);
                go(inner, h);
            }
            BoundedCsg::InnerWall { inner, .. } => {
                fnv(h, 7);
                go(inner, h);
            }
            _ => fnv(h, 8),
        }
    }
    let mut h: u64 = 0x9e37_79b9_7f4a_7c15;
    go(node, &mut h);
    h
}

// ─── Der Klemm-Typ ──────────────────────────────────────────────────────────

/// **Die η-Klemme eines Kanals — ein geschützter Beweiswert.**
///
/// Die Zusage, die dahintersteht (Lemma 4, Dreiecksungleichung): für JEDEN
/// Punkt `x` mit `de = de_f32(x) < trust` gilt
///
/// ```text
/// dist(x, ∁Ω) ≤ de + η_geo + ε_q + ε_d,
/// ```
///
/// und mit dem Rundungsbudget aus Lemma 5 liefert die eingefrorene Feldkette
/// `fl(δ − min(fl(de + A), h))` einen Wert, der die gültige Feld-Untergrenze
/// `δ − dist(x,∁Ω)` nicht überschreitet.
///
/// **Alle Felder sind privat, und es gibt keinen öffentlichen Konstruktor.**
/// `GeometricEta` war in §8x aus demselben Grund opak: ein Beweiswert, den der
/// Aufrufer selbst zusammensetzen kann, ist kein Beweiswert. Die einzige
/// Entstehungsstelle ist [`certify_channel`], und sie verlangt **fünf**
/// Nachweise gemeinsam — vollständiges Channel-Cover, geometrisches `η`,
/// Punkt-Brücke, Norm-Brücke, Rundungsbudget — plus die Bindung an genau den
/// geprüften Kanal.
///
/// Die Zahl selbst gibt es nur über [`OffsetChannel::eta_addend`]; die
/// Einzelposten sind lesbar, aber ausdrücklich **Diagnose**: aus ihnen lässt
/// sich kein gültiger Addend zusammenaddieren, weil das Rundungsbudget vom
/// `δ`/`trust` genau dieses Kanals abhängt.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EtaClamp {
    geometric_mm: f64,
    q_bridge_mm: f64,
    norm_bridge_mm: f64,
    field_round_mm: f64,
    encoded_addend: f32,
    channel_identity: u64,
    omega_fingerprint: u64,
}

impl EtaClamp {
    /// Der f32-Addend `A` — **nur** gegen den Kanal, für den er bewiesen wurde.
    pub fn addend_for(&self, ch: &OffsetChannel) -> Option<f32> {
        (channel_identity(ch) == self.channel_identity).then_some(self.encoded_addend)
    }
    /// `η_geo` aus [`cover_channel`] (mm).
    pub fn geometric_mm(&self) -> f64 {
        self.geometric_mm
    }
    /// `ε_q` — die Punkt-Brücke (mm).
    pub fn q_bridge_mm(&self) -> f64 {
        self.q_bridge_mm
    }
    /// `ε_d` — die Norm-Brücke (mm).
    pub fn norm_bridge_mm(&self) -> f64 {
        self.norm_bridge_mm
    }
    /// Der reine Rundungsanteil des Addenden (mm).
    pub fn field_round_mm(&self) -> f64 {
        self.field_round_mm
    }
    /// Die bewiesene reelle Gesamtschranke `Δ = η_geo + ε_q + ε_d` (mm) — die
    /// Zahl, gegen die Lemma 4 spricht (OHNE Rundungsbudget).
    pub fn bridge_total_mm(&self) -> f64 {
        next_up(next_up(self.geometric_mm + self.q_bridge_mm) + self.norm_bridge_mm)
    }
    /// Provenienz von `Ω` (Diagnose, s. [`omega_fingerprint`]).
    pub fn omega_fingerprint(&self) -> u64 {
        self.omega_fingerprint
    }
}

/// Warum ein Kanal (k)eine Klemme bekommen hat. Der Shadow-Bericht von P5b-0
/// zählt genau diese Fälle — und `Proved` ist der einzige, in dem ein Wert
/// entsteht.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClampStatus {
    /// Klemme bewiesen und eingebettet.
    Proved,
    /// Mindestens ein Bogen ist gekrümmt ⇒ Bogen-Zeugen sind P5c.
    Curved,
    /// Cover unvollständig: kein Zeuge, nicht unterstütztes `Ω`-Blatt,
    /// entarteter Bogen.
    NoCover,
    /// Cover vollständig, aber `ε_q`, `ε_d` oder das Rundungsbudget scheitern.
    NoBridge,
    /// [`WitnessCfg`] ungültig.
    BadConfig,
}

/// Ablationen für die Gegenproben. `pub(crate)`: eine geschwächte Klemme darf
/// ausserhalb der Pins gar nicht erst entstehen können.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ClampKnobs {
    /// `ε_q := 0`.
    pub drop_q_bridge: bool,
    /// `ε_d := 0`.
    pub drop_norm_bridge: bool,
    /// Rundungsbudget weg: `A := ceil_f32(Δ)`.
    pub drop_field_round: bool,
    /// Den kodierten Addenden um `n` ulp verkleinern.
    pub shave_ulps: u32,
    /// Den Addenden fest vorgeben (Binärsuche der ulp-Familie).
    pub force_addend: Option<f32>,
}

/// **Die Zertifizierung EINES Kanals — die einzige Entstehungsstelle einer
/// [`EtaClamp`].**
///
/// Reihenfolge der Ausgänge ist Absicht: `BadConfig` vor `Curved` vor
/// `NoCover` vor `NoBridge`. Ein gekrümmter Bogen ist ein STRUKTURELLER
/// Ausschluss (P5c), kein gescheiterter Zeuge — die Zählung darf beides nicht
/// vermengen.
///
/// **Die Invariante von P5b-0**: `eta_clamp.is_some()` ⟺ jeder Punkt jedes
/// Bogenstücks dieses Kanals ist gedeckt UND jeder Bogen ist gerade UND alle
/// drei Brücken existieren. Bei `None` bleibt das heutige `channel_limit`
/// stehen — ausdrücklich weiter als **unbewiesene Modell-Annahme** (§8j/§8s),
/// nicht still neutralisiert.
pub fn certify_channel(
    omega: &BoundedCsg,
    ch: &mut OffsetChannel,
    cfg: WitnessCfg,
) -> (ClampStatus, WitnessCover) {
    certify_channel_with(omega, ch, cfg, ClampKnobs::default())
}

pub(crate) fn certify_channel_with(
    omega: &BoundedCsg,
    ch: &mut OffsetChannel,
    cfg: WitnessCfg,
    knobs: ClampKnobs,
) -> (ClampStatus, WitnessCover) {
    ch.eta_clamp = None;
    if !cfg.valid() {
        return (ClampStatus::BadConfig, cover_channel(omega, ch, cfg));
    }
    if ch.arcs.is_empty() {
        return (ClampStatus::NoCover, cover_channel(omega, ch, cfg));
    }
    // Gekrümmt zuerst — ein struktureller Ausschluss, kein Zeugen-Fehlschlag.
    if ch.arcs.iter().any(|a| !(a.sweep < 0.0)) {
        return (ClampStatus::Curved, cover_channel(omega, ch, cfg));
    }
    let cover = cover_channel(omega, ch, cfg);
    let Some(eta) = cover.eta else {
        return (ClampStatus::NoCover, cover);
    };
    let geo = eta.mm();
    if !(geo.is_finite() && geo >= 0.0) {
        return (ClampStatus::NoBridge, cover);
    }
    // `ε_q`: das Maximum über alle Bögen — die Klemme gilt für den Kanal, also
    // für seinen schlechtesten Bogen.
    let mut eps_q = 0.0f64;
    for a in &ch.arcs {
        let Some(v) = point_bridge_mm(a) else {
            return (ClampStatus::NoBridge, cover);
        };
        eps_q = eps_q.max(v);
    }
    if knobs.drop_q_bridge {
        eps_q = 0.0;
    }
    let Some(mut eps_d) = norm_bridge_mm(ch.trust) else {
        return (ClampStatus::NoBridge, cover);
    };
    if knobs.drop_norm_bridge {
        eps_d = 0.0;
    }
    let bridge_total = next_up(next_up(geo + eps_q) + eps_d);
    let (field_round, addend) = if knobs.drop_field_round {
        let Some(a) = f32_ceil(bridge_total) else {
            return (ClampStatus::NoBridge, cover);
        };
        (0.0, a)
    } else {
        let Some(v) = round_budget(ch.delta, ch.trust, bridge_total) else {
            return (ClampStatus::NoBridge, cover);
        };
        v
    };
    let mut addend = knobs.force_addend.unwrap_or(addend);
    for _ in 0..knobs.shave_ulps {
        addend = f32::from_bits(addend.to_bits().saturating_sub(1));
    }
    if !(addend.is_finite() && addend >= 0.0) {
        return (ClampStatus::NoBridge, cover);
    }
    ch.eta_clamp = Some(EtaClamp {
        geometric_mm: geo,
        q_bridge_mm: eps_q,
        norm_bridge_mm: eps_d,
        field_round_mm: field_round,
        encoded_addend: addend,
        channel_identity: channel_identity(ch),
        omega_fingerprint: omega_fingerprint(omega),
    });
    (ClampStatus::Proved, cover)
}

/// Der Shadow-Bericht: wie viele Kanäle eines Stopps/Feldes eine Klemme
/// bekommen — und aus welchem Grund die übrigen nicht.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClampTally {
    pub proved_eta: usize,
    pub curved: usize,
    pub no_cover: usize,
    pub no_bridge: usize,
    pub bad_config: usize,
}

impl ClampTally {
    pub fn add(&mut self, s: ClampStatus) {
        match s {
            ClampStatus::Proved => self.proved_eta += 1,
            ClampStatus::Curved => self.curved += 1,
            ClampStatus::NoCover => self.no_cover += 1,
            ClampStatus::NoBridge => self.no_bridge += 1,
            ClampStatus::BadConfig => self.bad_config += 1,
        }
    }
    pub fn total(&self) -> usize {
        self.proved_eta + self.curved + self.no_cover + self.no_bridge + self.bad_config
    }
}
