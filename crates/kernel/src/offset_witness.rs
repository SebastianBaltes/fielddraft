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
//! was die Produktion wirklich rechnet: [`CertifiedSeamAddend`] trägt `η_geo`
//! plus die drei f32-Posten `ε_q` (Punkt-Brücke), `ε_d` (Norm-Brücke) und
//! `field_round` (Rundungsbudget des NAHT-Astes der eingefroren definierten
//! Feldkette) und liefert daraus einen `f32`-Addenden.
//!
//! **Der Naht-Ast, und nur er.** Zugesagt ist
//! `fl32(δ − fl32(de + A)) ≤ δ − de − Δ`. Über die vollständige Kette
//! `fl(δ − min(fl(de+A), h))` sagt P5b-0 **nichts**: der `h`-Ast ruht auf der
//! §8j-Stützebenen-Aussage, die weder ein Rundungs- noch ein geometrisches
//! Budget hat (Restposten R1, §8aa). Das `h`-Budget ist zwingende Auflage vor
//! P5b-1.
//!
//! **Immer noch keine Feldänderung.** `channel_limit`, `channel_apply` und
//! jeder Evaluator bleiben unangetastet; das Golden ist byte-identisch. Kein
//! Konsument liest [`OffsetChannel::seam_addend`] — der Verbrauch ist P5b-1,
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
use crate::csg::OpenBranchSet;
use crate::offset_channel::{ChannelArc, OffsetChannel, END_PLANE_OFF};
#[cfg(test)]
use crate::offset_channel::{channel_nearest, face_plane_gate, norm as norm32, sub as sub32};
use crate::offset_interval::{next_down, next_up, Iv};
use crate::sketch2d::{BoundedSketch2d, Sketch2dPrimitive};
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

#[inline]
fn rnorm2(v: [Iv; 2]) -> Option<Iv> {
    rsqrt(radd(rmul(v[0], v[0])?, rmul(v[1], v[1])?)?)
}

/// `−a`, exakt (Vorzeichenwechsel ist in binary64 fehlerfrei).
#[inline]
fn rneg(a: Iv) -> Option<Iv> {
    Iv::new(-a.hi, -a.lo)
}

// ════════════════════════════════════════════════════════════════════════════
// Endlichkeit — ausdrückliche Vorbedingung jedes Beweisschritts
// ════════════════════════════════════════════════════════════════════════════
//
// §8ae-Korrektur (sechzehntes Review, Blocker 1). Die Hüll-Konstruktion
// akkumulierte Polygon-Ecken mit `f64::min`/`f64::max`. Beide sind KEINE
// Beweis-Operationen: sie verwerfen einen `NaN`-Operanden zugunsten des
// endlichen. Ein Quadrat plus einer Ecke `(NaN,NaN)` lieferte deshalb die
// endliche Quadrat-Hülle, und `omega_cover(…, Active)` behauptete eine
// Überdeckung, die keine ist.
//
// Die Lehre ist allgemeiner als der eine Fundort: die Intervall-Helfer
// (`rseal` über `Iv::new`, `rmul`, `rdiv`) fangen `NaN` zwar zuverlässig ab,
// aber sie tun es als NEBENWIRKUNG. Ein Beweis, dessen Fail-closed-Verhalten
// von der Implementierung einer Hilfsfunktion abhängt, ist nicht geführt,
// sondern geliehen. Endlichkeit wird deshalb ab hier ÜBERALL ausdrücklich
// verlangt, bevor eine Zahl in eine Hülle oder in den Aussenbeweis eingeht —
// auch dort, wo die Intervall-Kette sie ohnehin schon abgefangen hätte.

#[inline]
fn fin2(v: &[f32; 2]) -> bool {
    v.iter().all(|x| x.is_finite())
}

#[inline]
fn fin3(v: &[f32; 3]) -> bool {
    v.iter().all(|x| x.is_finite())
}

/// Jede Zahl einer Platzierung — Translation, Quaternion, Skalierung.
#[inline]
fn transform_finite(t: &Transform) -> bool {
    fin3(&t.translate) && t.rotation.iter().all(|x| x.is_finite()) && fin3(&t.scale)
}

/// Die Zahlen eines 3D-Blattes, das der Beweis überhaupt anfassen darf.
/// `false` heisst schlicht: nicht beweisbar (weder aussen noch überdeckt).
#[inline]
fn primitive_finite(p: &Primitive) -> bool {
    match p {
        Primitive::Box { center, half_extents } => fin3(center) && fin3(half_extents),
        Primitive::Sphere { center, radius } => fin3(center) && radius.is_finite(),
        Primitive::Cylinder { center, radius, half_height } => {
            fin3(center) && radius.is_finite() && half_height.is_finite()
        }
        _ => false,
    }
}

/// Dasselbe für ein 2D-Profil-Blatt.
#[inline]
fn sketch_prim_finite(p: &Sketch2dPrimitive) -> bool {
    match p {
        Sketch2dPrimitive::Rectangle { center, half_extents } => {
            fin2(center) && fin2(half_extents)
        }
        Sketch2dPrimitive::Circle { center, radius } => fin2(center) && radius.is_finite(),
        Sketch2dPrimitive::Polygon { vertices } => vertices.iter().all(fin2),
        _ => false,
    }
}

// ════════════════════════════════════════════════════════════════════════════
// #384 §8ae / Stufe A — die Stufen-Schaltung des Extrude-Aussenbeweises
// ════════════════════════════════════════════════════════════════════════════

/// **Ist der Extrude-Aussenbeweis schon feldwirksam?**
///
/// Die Auflage aus P5b-1A: eine neue Feldform ausliefern UND sie erstmals
/// beweisen darf nie derselbe Schritt sein. Ein neuer Aussenbeweis deckt neue
/// Naht-Stücke, deckt damit neue Kanäle, prägt damit neue Naht-Addenden — und
/// die gehen in `channel_limit` ein. Das IST eine Feldänderung, auch wenn kein
/// einziges Zeichen des Beweises im Feld auftaucht.
///
/// Deshalb hat der Beweis eine Stufe:
///
/// * [`ExtrudeCoverStage::ShadowOnly`] (Stufe A, heute) — der Beweis existiert,
///   ist gepinnt und wird gemessen, aber [`box_outside_omega`] benutzt ihn
///   NICHT. Jeder Konsument (Naht-Addend über [`cover_channel`], `Ω`-Box-
///   Überdeckung über [`omega_cover`]) sieht exakt das Verhalten von vorher.
/// * [`ExtrudeCoverStage::Active`] (Stufe B, separat) — derselbe Beweis, jetzt
///   feldwirksam. Die Umschaltung ist genau EINE Zeile: die Konstante
///   [`EXTRUDE_COVER_STAGE`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtrudeCoverStage {
    /// Stufe A: bewiesen und gemessen, aber nicht konsumiert.
    ShadowOnly,
    /// Stufe B: feldwirksam.
    Active,
}

/// **Das Stufen-Gate.** Stufe A hielt den Extrude-Beweis im Shadow; seit
/// Stufe B (§8ae) steht es auf `Active` und der Beweis ist feldwirksam.
///
/// Die Umschaltung war genau diese eine Zeile — zusammen mit der
/// merge-blockierenden Auflage aus Befund 2 des sechzehnten Reviews, die
/// [`omega_has_open_extrude`] an der Konsumgrenze erfüllt: im OFFENEN
/// Auswertungspfad gilt der Beweis nicht, und dort zieht sich der Modus auf
/// Legacy zurück.
pub const EXTRUDE_COVER_STAGE: ExtrudeCoverStage = ExtrudeCoverStage::Active;

/// **Das Stufen-Gate der gekrümmten Nähte** (#384 P5c). Dasselbe Muster wie
/// [`EXTRUDE_COVER_STAGE`], für denselben Grund — und hier mit einem
/// **gemessenen** Preis.
///
/// * `ShadowOnly` (**P5c-A**, heute) — [`piece_box`] trägt gekrümmte
///   Minor-Bögen, `arc_usable` lässt sie durch, der Shadow-Bericht misst
///   `238/238` statt `236/2`. Das FELD sieht exakt das Verhalten von vorher:
///   [`certify_channel_with`] meldet die beiden Kreisnähte der Stufen-Nabe
///   weiter als `Curved`, und ihre Klemme bleibt das heutige `channel_limit`.
/// * `Active` (**P5c-C**, eigenes Ticket) — feldwirksam.
///
/// **Warum die Aktivierung NICHT hierher gehört.** Der Beweis ist gültig, aber
/// er ist SCHWÄCHER als die unbewiesene Klemme, die er ersetzen würde: die
/// Kreisnähte brauchen `η_geo ≈ 0,81 mm`, und über
/// `lim = δ − (best + A_seam).min(…)` lockert das das Gate. Feldwirksam
/// geschaltet entstehen an der Nabe **41 Punkte Phantom-Material** in der
/// Ablation ohne `L`-Block (vorher 0); im Produktionsfeld fängt der `L`-Block
/// sie ab, aber die Sicherheit der Nabe ruhte vorher doppelt und ruhte danach
/// nur noch auf ihm. Dieser Redundanz-Verlust ist eine eigene Entscheidung
/// und gehört nicht in ein Ticket, dessen Gegenstand der Cover-Beweis ist.
///
/// **Und es ist keine Leiter-Frage** (anders als #466): `η` konvergiert unter
/// Verfeinerung gegen `≈ 0,81 mm` (steps 18/65/257/1025 → 1,150/0,901/0,830/
/// 0,813). Die Sprosse erklärt nur den Faktor 1,4, der Rest ist Geometrie —
/// eine feinere Leiter kauft die 41 Punkte also NICHT zurück.
pub const CURVED_COVER_STAGE: ExtrudeCoverStage = ExtrudeCoverStage::ShadowOnly;

/// **§8ae Stufe B — trägt `Ω` ein `Extrude`-Blatt, dessen Feature GEÖFFNET
/// ist?** Wenn ja, gilt der Extrude-Aussenbeweis für diesen Auswertungspunkt
/// NICHT, und der Aufrufer muss den Modus auf
/// [`crate::offset_channel::LegacyReason::OpenRetainedPath`] zurückziehen.
///
/// ## Warum das nötig ist
///
/// [`box_outside_omega`] beweist die Menge `{max(d₂, |z| − hh) ≤ 0}` — das,
/// was `bvh.rs::sdf_open` im `else`-Zweig über `sdf_extrude` rechnet
/// (`crates/kernel/src/bvh.rs:636..651`). Bei `open.has_feature(id)` läuft die
/// Auswertung stattdessen über `sketch.sdf_retained` und
/// [`crate::csg::extrude_retained_combine`]
/// (`crates/kernel/src/csg.rs:3784..3799`), und das ist eine ANDERE Menge:
/// jede nicht angeklammerte Kappe lässt ihren `max`-Term aus, bei BEIDEN
/// offenen Kappen bleibt `d = d₂` — profil-intern ein UNENDLICHES Prisma. Der
/// typische Aussenzeuge der geschlossenen Extrusion, der axiale Grund
/// `|z| > hh`, liegt dort im MATERIAL. Der Zeuge wäre schlicht falsch.
///
/// ## Warum konservativ über den GANZEN Baum
///
/// Der Aussenbeweis steigt bei `Difference`/`Intersection` nur in EINEN Ast
/// ab; dieser Test läuft trotzdem durch beide. Ein Soundness-Gate, das die
/// Abstiegs-Logik des Beweises nachbildet, müsste bei jeder Änderung dort
/// mitgezogen werden — und ein vergessener Ast wäre still unsound. Die
/// konservative Fassung kann nur ZU VIEL zurückziehen, nie zu wenig.
///
/// ## Kosten
///
/// Ein Baumlauf über `Ω`, **nicht** pro Punkt: der einzige Aufrufer
/// ([`crate::offset_channel::resolve_stop_modes_at`]) merkt das Ergebnis je
/// Aufruf und rechnet es erst hinter demselben Trust-Vorfilter, hinter dem
/// ohnehin der weit teurere [`omega_snapshot`] hängt. `open.is_empty()` steht
/// davor und ist im Regelfall — kein offenes Shell-Face weit und breit — die
/// ganze Arbeit.
pub(crate) fn omega_has_open_extrude(node: &BoundedCsg, open: &OpenBranchSet<'_>) -> bool {
    match node {
        BoundedCsg::Extrude { id, .. } => open.has_feature(id.as_u32_or_root().unwrap_or(0)),
        BoundedCsg::Union { left, right, .. }
        | BoundedCsg::Intersection { left, right, .. }
        | BoundedCsg::Difference { left, right, .. } => {
            omega_has_open_extrude(left, open) || omega_has_open_extrude(right, open)
        }
        BoundedCsg::Offset { inner, .. }
        | BoundedCsg::InnerWall { inner, .. }
        | BoundedCsg::Shell { inner, .. }
        | BoundedCsg::Displace { inner, .. } => omega_has_open_extrude(inner, open),
        // Jede andere Form kann kein `Extrude`-Blatt tragen, das dieser Beweis
        // je angefasst hätte — `box_outside_inner` liefert dort `false`, der
        // Modus ist ohnehin Legacy.
        _ => false,
    }
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
/// | `Extrude` (§8ae, nur Stufe B) | axial `min_b |local_z| > hh` ODER das Profil ist in 2D bewiesen aussen |
/// | alles andere | **nicht bewiesen** ⇒ `false` (fail-closed) |
///
/// Der `Extrude`-Arm läuft über das Stufen-Gate [`EXTRUDE_COVER_STAGE`]: in
/// Stufe A ([`ExtrudeCoverStage::ShadowOnly`]) liefert er wie bisher `false`,
/// und diese Funktion verhält sich Bit für Bit wie vor §8ae.
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
    box_outside_omega_staged(node, b, margin, EXTRUDE_COVER_STAGE)
}

/// Wie [`box_outside_omega`], aber mit ausdrücklich gewählter Stufe. Die
/// Produktion ruft **nie** diese Fassung — sie existiert für die §8ae-Pins und
/// für die Shadow-Messung des Gewinns, den Stufe B holen wird.
///
/// **Warum hier trotzdem KEIN `#[cfg(test)]` steht** (§8ae, sechzehntes
/// Review, Sollte 3): [`box_outside_omega`] und [`cover_channel_staged`]
/// gehen im Produktions-Build durch diese Funktion — nur eben mit
/// [`EXTRUDE_COVER_STAGE`] statt mit einer frei gewählten Stufe. Sie ist
/// zudem eine reine ABFRAGE und versiegelt nichts; test-only ist genau der
/// schreibende Weg, [`certify_for_probe`]. Dasselbe gilt für
/// [`cover_channel_staged`] und [`omega_cover_staged`].
pub(crate) fn box_outside_omega_staged(
    node: &BoundedCsg,
    b: [Iv; 3],
    margin: f64,
    stage: ExtrudeCoverStage,
) -> bool {
    if !(margin.is_finite() && margin >= 0.0) {
        return false;
    }
    box_outside_inner(node, b, margin, stage)
}

fn box_outside_inner(node: &BoundedCsg, b: [Iv; 3], margin: f64, stage: ExtrudeCoverStage) -> bool {
    let sep = |v: Option<Iv>| -> bool { matches!(v, Some(s) if s.lo > margin) };
    match node {
        BoundedCsg::Empty => true,
        BoundedCsg::Leaf { primitive, world_transform, .. } => {
            if !primitive_finite(primitive) {
                return false;
            }
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
        // #384 §8ae — das Extrusions-Blatt. `Ω = {p : max(d₂(x,y), |z| − hh) ≤ 0}`
        // im LOKALEN Rahmen (die Weltskala multipliziert den Feldwert und kann
        // das Vorzeichen nicht drehen, für die MENGE ist sie also irrelevant).
        // Zwei unabhängige Ausschlussgründe, einer genügt:
        //
        // * **axial** — die ganze Box liegt jenseits von `±hh` in lokal-`z`;
        // * **profil** — die Projektion der Box auf die Skizzenebene liegt
        //   beweisbar ausserhalb des 2D-Profils.
        //
        // Die Projektion ist EXAKT: `local` ist eine achsenparallele
        // Intervall-Box, ihre `(x,y)`-Komponenten sind die Projektion.
        //
        // ## Vorbedingung: der GESCHLOSSENE Auswertungspfad
        //
        // (§8ae-Korrektur, sechzehntes Review, Befund 2.) Bewiesen wird hier
        // die Menge `{max(d₂, |z| − hh) ≤ 0}` — also genau das, was
        // `bvh.rs::sdf_open` im `else`-Zweig über `sdf_extrude` rechnet
        // (`crates/kernel/src/bvh.rs:634..651`). Bei
        // `open.has_feature(id)` läuft die Auswertung stattdessen über
        // `sketch.sdf_retained` und `crate::csg::extrude_retained_combine`
        // (`crates/kernel/src/csg.rs:3784..3799`), und das ist eine ANDERE
        // MENGE: jede nicht angeklammerte Kappe lässt ihren `max`-Term aus,
        // bei beiden offenen Kappen bleibt schlicht `d = d₂` — profil-intern
        // ein UNENDLICHES Prisma. Ein Aussenzeuge der geschlossenen Extrusion
        // (typisch: der axiale Grund, `|z| > hh`) liegt dort im Material.
        //
        // Für Stufe A ist das folgenlos — der Arm liefert ohnehin `false`.
        // Stufe B darf erst aktiv werden, wenn sie bei nichtleerem relevantem
        // `OpenSet` den Modus auf Legacy zurückzieht ODER separat gegen die
        // retained Menge bewiesen ist. Solange das nicht dasteht, gilt der
        // Beweis ausdrücklich NUR für den geschlossenen Pfad.
        BoundedCsg::Extrude { sketch, half_height, world_transform, .. } => {
            if stage != ExtrudeCoverStage::Active || !half_height.is_finite() {
                return false;
            }
            let Some(local) = local_box(world_transform, b) else { return false };
            let axial = rsub(local[2].abs(), pt((*half_height as f64).abs()));
            sep(axial) || sketch_outside_inner(sketch, [local[0], local[1]], margin)
        }
        BoundedCsg::Union { left, right, .. } => {
            box_outside_inner(left, b, margin, stage) && box_outside_inner(right, b, margin, stage)
        }
        BoundedCsg::Intersection { left, right, .. } => {
            box_outside_inner(left, b, margin, stage) || box_outside_inner(right, b, margin, stage)
        }
        // Die rechte Seite kann nur Material WEGNEHMEN; wer links draussen ist,
        // ist auch im Ergebnis draussen.
        BoundedCsg::Difference { left, .. } => box_outside_inner(left, b, margin, stage),
        // `δ ≤ 0`: der Offset-Körper ist `{sdf ≤ δ} ⊆ {sdf ≤ 0}` = das Kind.
        // Für `δ > 0` ist er GRÖSSER, und die Schlussrichtung dreht sich um —
        // dann ist hier nichts bewiesen.
        BoundedCsg::Offset { inner, delta, .. } => {
            delta.is_finite() && *delta <= 0.0 && box_outside_inner(inner, b, margin, stage)
        }
        BoundedCsg::InnerWall { inner, .. } => box_outside_inner(inner, b, margin, stage),
        _ => false,
    }
}

/// **Liegt die GANZE 2D-Box `p` beweisbar ausserhalb des Profils `node`?**
///
/// Strukturgleich zu [`box_outside_inner`], nur eine Dimension tiefer:
///
/// | Knoten | `p ∩ Profil = ∅` ist bewiesen, wenn … |
/// |---|---|
/// | `Empty` | immer |
/// | `Rectangle` | eine Achse TRENNT: `min_p |x_k − c_k| > h_k` (analog `Box`) |
/// | `Circle` | `min_p |x − c| > r` (analog `Sphere`) |
/// | `Polygon` | eine Kantengerade trennt — s. u. |
/// | `Sector`, `Path` | **nie** ⇒ `false` (s. §8ae-Doku) |
/// | `Union(A,B)` | `A` UND `B` |
/// | `Intersection(A,B)` | `A` ODER `B` |
/// | `Difference(A,B)` | `A` |
/// | `Offset(inner, d)`, `d ≤ 0` | `inner` (das erodierte Profil ist TEILMENGE) |
///
/// Für `d > 0` ist das Offset-Profil GRÖSSER als sein Kind, und die
/// Schlussrichtung dreht sich um — genau wie beim 3D-`Offset`-Arm.
fn sketch_outside_inner(node: &BoundedSketch2d, p: [Iv; 2], margin: f64) -> bool {
    let sep = |v: Option<Iv>| -> bool { matches!(v, Some(s) if s.lo > margin) };
    match node {
        BoundedSketch2d::Empty => true,
        BoundedSketch2d::Prim { primitive, .. } if !sketch_prim_finite(primitive) => false,
        BoundedSketch2d::Prim { primitive, .. } => match primitive {
            Sketch2dPrimitive::Rectangle { center, half_extents } => (0..2).any(|k| {
                let d = rsub(p[k], pt(center[k] as f64));
                sep(d.and_then(|d| rsub(d.abs(), pt((half_extents[k] as f64).abs()))))
            }),
            Sketch2dPrimitive::Circle { center, radius } => {
                let Some(dx) = rsub(p[0], pt(center[0] as f64)) else { return false };
                let Some(dy) = rsub(p[1], pt(center[1] as f64)) else { return false };
                sep(rnorm2([dx, dy]).and_then(|n| rsub(n, pt((*radius as f64).abs()))))
            }
            Sketch2dPrimitive::Polygon { vertices } => polygon_outside(vertices, p, margin),
            // `Sector` und `Path` sind ehrlich unbewiesen — kein Fall der
            // Zertifizierungs-Matrix führt darauf (§8ae-Erhebung).
            _ => false,
        },
        BoundedSketch2d::Union { left, right, .. } => {
            sketch_outside_inner(left, p, margin) && sketch_outside_inner(right, p, margin)
        }
        BoundedSketch2d::Intersection { left, right, .. } => {
            sketch_outside_inner(left, p, margin) || sketch_outside_inner(right, p, margin)
        }
        BoundedSketch2d::Difference { left, .. } => sketch_outside_inner(left, p, margin),
        BoundedSketch2d::Offset { inner, distance, .. } => {
            distance.is_finite() && *distance <= 0.0 && sketch_outside_inner(inner, p, margin)
        }
    }
}

/// **Der Polygon-Ausschluss — ohne Konvexitäts-Orakel, und trotzdem rigoros.**
///
/// Ein „trennender Kanten-Halbraum" beweist bei einem beliebigen Polygon
/// **nichts**: bei einer konkaven Kerbe (und erst recht bei einem Sternpolygon)
/// liegt Material auf BEIDEN Seiten der Kantengeraden. Der naive Test wäre
/// genau der weiche Beweis, den die Doktrin verbietet.
///
/// Bewiesen wird deshalb je Kante eine STÄRKERE Vorbedingung, und sie ist
/// lokal prüfbar:
///
/// 1. Liegen **alle** Ecken auf derselben (abgeschlossenen) Seite der
///    Kantengeraden `L_i`, so liegt der ganze Polygonzug in diesem Halbraum
///    `H_i` (Halbräume sind konvex, jede Kante ist Konvexkombination zweier
///    Ecken).
/// 2. Dann liegt auch die AUSGEFÜLLTE Menge in `H_i`: ein Punkt `x ∉ H_i` lässt
///    sich innerhalb des offenen Komplements von `H_i` — das den Rand nicht
///    trifft — ins Unendliche schieben, seine Kreuzungszahl ist also `0`, und
///    die Even-Odd-Regel (`sdf_polygon`) stellt ihn nach aussen. ∎
/// 3. Trennt die Box strikt (um mehr als `margin`, in echtem Abstand, also
///    durch `|d_i|` geteilt), ist sie bewiesen aussen.
///
/// Für ein konvexes Polygon erfüllt JEDE Kante Schritt 1 — der Test ist dort
/// der volle Halbraum-Schnitt. Für ein nicht-konvexes fallen genau die Kanten
/// weg, an denen der naive Test gelogen hätte; in einer konkaven Kerbe bleibt
/// nichts übrig, und das Ergebnis ist ehrlich `false`.
fn polygon_outside(v: &[[f32; 2]], p: [Iv; 2], margin: f64) -> bool {
    // Weniger als drei Ecken: `sdf_polygon` liefert `+∞`, die Menge ist leer.
    // Trotzdem fail-closed — ein entartetes Profil ist kein Beweisgegenstand.
    if v.len() < 3 {
        return false;
    }
    // Endlichkeit ausdrücklich, nicht geliehen (§8ae, sechzehntes Review).
    if !v.iter().all(fin2) {
        return false;
    }
    let n = v.len();
    let vx = |j: usize, k: usize| pt(v[j][k] as f64);
    // `cross(d, q − a)` als Intervall.
    let cross = |dx: Iv, dy: Iv, ax: Iv, ay: Iv, qx: Iv, qy: Iv| -> Option<Iv> {
        rsub(rmul(dx, rsub(qy, ay)?)?, rmul(dy, rsub(qx, ax)?)?)
    };
    for i in 0..n {
        let j = (i + 1) % n;
        let (ax, ay) = (vx(i, 0), vx(i, 1));
        let Some(dx) = rsub(vx(j, 0), ax) else { continue };
        let Some(dy) = rsub(vx(j, 1), ay) else { continue };
        // Eine Kante ohne beweisbar positive Länge trägt keine Gerade.
        let Some(dn) = rnorm2([dx, dy]) else { continue };
        if !(dn.lo > 0.0) {
            continue;
        }
        // Schritt 1: auf welcher Seite liegen ALLE Ecken?
        let (mut all_left, mut all_right) = (true, true);
        for m in 0..n {
            // Die beiden Ecken der Kante liegen PER KONSTRUKTION exakt auf
            // `L_i` — sie sind in beiden abgeschlossenen Halbräumen. Sie hier
            // mitzurechnen wäre kein Mehr an Strenge, sondern nur das
            // Aussenrundungs-Intervall um die exakte Null: `c ∋ 0` mit
            // `c.lo < 0 < c.hi` würde JEDE Kante verwerfen und den ganzen
            // Polygon-Arm still tot stellen.
            if m == i || m == j {
                continue;
            }
            match cross(dx, dy, ax, ay, vx(m, 0), vx(m, 1)) {
                Some(c) => {
                    all_left &= c.lo >= 0.0;
                    all_right &= c.hi <= 0.0;
                }
                None => {
                    all_left = false;
                    all_right = false;
                }
            }
            if !all_left && !all_right {
                break;
            }
        }
        if !all_left && !all_right {
            continue;
        }
        // Schritt 3: trennt die Box strikt — in ECHTEM Abstand.
        let Some(c) = cross(dx, dy, ax, ay, p[0], p[1]) else { continue };
        // `H = {cross ≥ 0}` ⇒ der Abstand nach aussen ist `−cross/|d|`;
        // `H = {cross ≤ 0}` ⇒ `+cross/|d|`.
        let cand = if all_left { rneg(c) } else { Some(c) };
        let ok = cand
            .and_then(|s| rdiv(s, dn))
            .is_some_and(|dist| dist.lo > margin);
        if ok {
            return true;
        }
    }
    false
}

#[inline]
fn sub3(a: [Iv; 3], b: [Iv; 3]) -> Option<[Iv; 3]> {
    Some([rsub(a[0], b[0])?, rsub(a[1], b[1])?, rsub(a[2], b[2])?])
}

/// `world_to_local` in REELLER Intervall-Arithmetik. Für die Identität
/// (`q = (0,0,0,1)`, `scale = 1`) ist das Ergebnis exakt.
fn local_box(t: &Transform, p: [Iv; 3]) -> Option<[Iv; 3]> {
    if !transform_finite(t) {
        return None;
    }
    local_linear(t, sub3(p, f32p3(t.translate))?)
}

/// Der LINEARE Teil von [`local_box`] — `world_to_local` ohne die
/// Translation: `A·v = S⁻¹ · M(conj q) · v`.
///
/// §8ab-Korrektur (fünfzehntes Review, Blocker 1): herausgezogen, damit die
/// Vorwärts-Abbildung `A⁻¹` aus GENAU DIESER Abbildung entsteht — als
/// rigorose Matrix-Inverse, nicht als Quaternion-Ratespiel. Die Spalten von
/// `A` sind `A·e₀`, `A·e₁`, `A·e₂`.
fn local_linear(t: &Transform, v: [Iv; 3]) -> Option<[Iv; 3]> {
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

/// Ein Bogen, den die Konstruktion überhaupt anfassen darf: endliche
/// Koeffizienten, nicht entartet — **Segment ODER gekrümmter Minor-Bogen**
/// (#384 P5c-A).
///
/// Für den gekrümmten Fall kommen drei Forderungen dazu, und alle drei sind
/// **Voraussetzungen des Beweises in [`piece_box`]**, keine Hygiene:
/// * `v` endlich — der Bogen spannt in `(u,v)` auf, das Segment liest `v` nie.
/// * `e` endlich — Provenienz der Endrichtung.
/// * `0 < D_e ≤ 2`. Das ist die **Minor-Bogen-Invariante**: `D` ist streng
///   monoton in `φ ∈ [0,π]` (s. [`diamond`]), `D_e ≤ 2` heisst also
///   `α ≤ π`. Genau darauf ruht die Sagitta-Schranke — für einen Major-Bogen
///   ist die Sehne die des GEGENbogens und die Schranke schlicht falsch.
///   `arc_through` kappt bei π, die Invariante wird hier trotzdem geprüft
///   statt geglaubt.
fn arc_usable(arc: &ChannelArc) -> bool {
    let base = arc.r.is_finite()
        && arc.r as f64 >= 1.0e-9
        && arc.c.iter().all(|v| v.is_finite())
        && arc.u.iter().all(|v| v.is_finite())
        && arc.bc.iter().all(|v| v.is_finite())
        && arc.br.is_finite();
    if arc.sweep < 0.0 {
        return base;
    }
    // Blocker 2 des Bogen-Reviews: Endlichkeit allein bindet `e` und `D_e`
    // NICHT aneinander. Ein Klon mit `e = [10,0]`, `D_e = 0.5` käme sonst
    // durch — `piece_box` schlösse die brave D-Kurve ein, während
    // `arc_closest` seinen Endpunkt aus dem gespeicherten `e` bildet und weit
    // ausserhalb landet. `arc_data_valid` ist die Stelle, an der diese Bindung
    // (`D_e == diamond(e)` bitgenau, `e[1] >= 0`, `sweep ∈ (0,π]`,
    // kontrollierte Endrichtungsnorm) schon steht — sie zu wiederholen hiesse,
    // sie ein zweites Mal falsch zu machen.
    base && crate::offset_channel::arc_data_valid(arc)
        && arc.d_e.is_finite()
        && arc.d_e > 0.0
        && arc.d_e <= 2.0
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

/// Test-Zugang zum Gate [`arc_usable`] (#384 P5c-A). `piece_box` ist die reine
/// KONSTRUKTION und fragt das Gate nicht selbst — das tut der Aufrufer
/// [`cover_channel_staged`]. Die Gegenprobe zur Datenbindung muss deshalb
/// hier ansetzen und nicht an der Box.
#[cfg(test)]
pub fn arc_usable_for_test(arc: &ChannelArc) -> bool {
    arc_usable(arc)
}

/// Test-Zugang zu [`piece_box`] (#384 P5c-A-Falsifikator). Kein
/// Produktionspfad ruft das.
#[cfg(test)]
pub fn piece_box_for_test(arc: &ChannelArc, t0: f64, t1: f64) -> Option<[Iv; 3]> {
    piece_box(arc, t0, t1)
}

/// Die achsparallele Hülle zweier Intervalle.
#[inline]
fn rhull(a: Iv, b: Iv) -> Option<Iv> {
    Iv::new(a.lo.min(b.lo), a.hi.max(b.hi))
}

/// Der **normierte Richtungsvektor** `(cos φ, sin φ)` zum Diamant-Parameter
/// `D`, reell eingeschlossen — die Intervall-Fassung von
/// [`crate::offset_channel::diamond_dir`] plus Normierung.
///
/// Der Zweig-Schalter bei `D = 1` ist stetig (beide Zweige liefern dort
/// `(0,1)`, s. Lemma an `diamond_dir`); ein Intervall, das die Naht
/// überspannt, nimmt deshalb schlicht die Hülle beider Zweige und schliesst
/// den wahren Wert weiterhin ein.
fn diamond_dir_box(d: Iv) -> Option<[Iv; 2]> {
    let lo_branch = || -> Option<[Iv; 2]> { Some([rsub(pt(1.0), d)?, d]) };
    let hi_branch =
        || -> Option<[Iv; 2]> { Some([rneg(rsub(d, pt(1.0))?)?, rsub(pt(2.0), d)?]) };
    let w = if d.hi <= 1.0 {
        lo_branch()?
    } else if d.lo >= 1.0 {
        hi_branch()?
    } else {
        let (a, b) = (lo_branch()?, hi_branch()?);
        [rhull(a[0], b[0])?, rhull(a[1], b[1])?]
    };
    let n = rnorm2(w)?;
    Some([rdiv(w[0], n)?, rdiv(w[1], n)?])
}

/// Die Box eines Naht-Stücks, reell eingeschlossen.
///
/// **Segment** (`sweep < 0`): `{c + û·(s·r) : s ∈ [t0,t1]}`, unverändert.
///
/// **Gekrümmter Bogen** (#384 P5c-A): `{c + r·(u·cos φ + v·sin φ)}` über dem
/// Parameter-Fenster `[t0,t1]`. Die Parametrisierung ist **nicht frei
/// gewählt** — [`arc_closest`](crate::offset_channel::arc_closest) definiert
/// `t = D(z)/D_e`, also ist `t` bereits der normierte Diamant-Parameter, und
/// die Stück-Zerlegung muss genau dieselbe Skala benutzen, gegen die die
/// Fusspunkte geprüft werden. Eine gleichmässige Winkelteilung wäre hier
/// nicht schöner, sondern inkonsistent.
///
/// **Die Einschliessung.** Endpunkte `A = P(t0)`, `B = P(t1)` exakt aus den
/// Diamant-Richtungen (kein `sin`/`cos` — §8y). Dazwischen bauchen die
/// Bogenpunkte über die Sehne hinaus; diese Bauchigkeit wird **im
/// Einheitskreis** geschrankt und erst danach mit `r` aufgeblasen:
///
/// > Mit `L(λ)` als der Sehne und `λ(φ)` als der Orthogonalprojektion gilt
/// > `P(φ) − L(λ) = r·(u·Δc + v·Δs)`, wobei `(Δc, Δs)` **allein von den
/// > Winkeln abhängt** — `u` und `v` kommen darin nicht vor. `|(Δc,Δs)|` ist
/// > die Sagitta des Einheitskreises, `σ = 1 − √(1−h²) = h²/(1+√(1−h²)) ≤ h²`
/// > mit `h = |ŵ(t0) − ŵ(t1)|/2`. Komponentenweise folgt
/// > `|P_k − L_k| ≤ r·σ·(|u_k| + |v_k|)`.
///
/// Das ist der Grund, warum hier **nichts über `(u,v)` vorausgesetzt** wird —
/// keine Orthonormalität, keine Toleranz, kein Fudge-Faktor. Wären `u` und
/// `v` nicht orthonormal, wäre die Kurve eine Ellipse und eine
/// Sehnen-Sagitta-Schranke falsch; die Schranke oben bliebe gültig, weil sie
/// die Bauchigkeit im Parameterraum misst und die Achsenlängen erst danach
/// einrechnet. Vorausgesetzt wird ausschliesslich `D_e ≤ 2` (Minor-Bogen),
/// und das prüft [`arc_usable`].
fn piece_box(arc: &ChannelArc, t0: f64, t1: f64) -> Option<[Iv; 3]> {
    let r = arc.r as f64;
    if arc.sweep < 0.0 {
        let s = Iv::new(next_down(t0 * r), next_up(t1 * r))?;
        let mut out = [pt(0.0); 3];
        for k in 0..3 {
            out[k] = radd(pt(arc.c[k] as f64), rmul(pt(arc.u[k] as f64), s)?)?;
        }
        return Some(out);
    }
    let de = pt(arc.d_e as f64);
    let w0 = diamond_dir_box(rmul(pt(t0), de)?)?;
    let w1 = diamond_dir_box(rmul(pt(t1), de)?)?;
    let ri = pt(r);

    // Die beiden Endpunkte `c + r·(u·ŵ₀ + v·ŵ₁)`.
    let endpoint = |w: [Iv; 2]| -> Option<[Iv; 3]> {
        let mut q = [pt(0.0); 3];
        for k in 0..3 {
            let t = radd(
                rmul(pt(arc.u[k] as f64), w[0])?,
                rmul(pt(arc.v[k] as f64), w[1])?,
            )?;
            q[k] = radd(pt(arc.c[k] as f64), rmul(ri, t)?)?;
        }
        Some(q)
    };
    let a = endpoint(w0)?;
    let b = endpoint(w1)?;

    // Die Sagitta des EINHEITSKREISES: `σ ≤ h²` mit `h = |ŵ₀ − ŵ₁|/2`.
    let mut chord2 = pt(0.0);
    for k in 0..2 {
        let d = rsub(w1[k], w0[k])?;
        chord2 = radd(chord2, rmul(d, d)?)?;
    }
    let sigma = rmul(chord2, pt(0.25))?;

    let mut out = [pt(0.0); 3];
    for k in 0..3 {
        // `r·σ·(|u_k| + |v_k|)` — die Bauchigkeit in DIESER Koordinate.
        let axes = radd(pt((arc.u[k] as f64).abs()), pt((arc.v[k] as f64).abs()))?;
        let bulge = rmul(rmul(ri, sigma)?, axes)?;
        let hull = rhull(a[k], b[k])?;
        out[k] = rseal(hull.lo - bulge.hi, hull.hi + bulge.hi)?;
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
///
/// **Was `t` NICHT ist** (§8ae Stufe B, 24. Review). Gefunden wird der erste
/// zertifizierte Kandidat aus einem FESTEN Vorrat: den 26 Richtungen aus
/// [`directions`] und den `cfg.steps` Stufen der geometrischen Leiter
/// `t_s = min_offset · (max_offset/min_offset)^(s/(steps−1))`. Das ist NICHT
/// der geometrisch kleinste taugliche Versatz — die Leiter springt mit
/// Quotient ≈ 1,865, der wahre erste Erfolg derselben Richtung kann zwischen
/// zwei Stufen liegen, eine andere Richtung sogar darunter. Ein grosses `η`
/// ist deshalb eine Eigenschaft der SUCHE, nicht der Geometrie: es schwächt
/// die Korrektur, macht den Beweis aber nicht falsch (die verschobene Box ist
/// wirklich bewiesen aussen). Wer `η_geo` als geometrische Grösse liest,
/// liest es falsch — z. B. steht das Maximum der heutigen Matrix,
/// 0,3305387… mm, exakt auf der 14. Leiterstufe und nicht auf einer
/// Eigenschaft des Rippen-Kamms.
pub fn cover_channel(omega: &BoundedCsg, ch: &OffsetChannel, cfg: WitnessCfg) -> WitnessCover {
    cover_channel_staged(omega, ch, cfg, EXTRUDE_COVER_STAGE)
}

/// Wie [`cover_channel`], aber mit ausdrücklich gewählter Stufe (§8ae). Nur für
/// die Pins und die Shadow-Messung — die Produktion geht über
/// [`cover_channel`] und damit über [`EXTRUDE_COVER_STAGE`].
pub(crate) fn cover_channel_staged(
    omega: &BoundedCsg,
    ch: &OffsetChannel,
    cfg: WitnessCfg,
    stage: ExtrudeCoverStage,
) -> WitnessCover {
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
                    if ok && box_outside_omega_staged(omega, wb, cfg.margin, stage) {
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

/// #384 §8ae — **nur für die Messung**: hat dieser Kanal unter der gewählten
/// Stufe eine VOLLSTÄNDIGE Zeugen-Abdeckung? Das ist die Kennzahl, an der
/// Stufe A gemessen wird; feldwirksam wird davon nichts.
///
/// `#[cfg(test)]` (§8ae, sechzehntes Review, Sollte 3): eine Messfunktion, die
/// den Stufen-Schalter von aussen setzt, hat in einem Produktions-Build nichts
/// verloren — dort gibt es nur [`EXTRUDE_COVER_STAGE`].
#[cfg(test)]
pub(crate) fn cover_shadow_probe(
    omega: &BoundedCsg,
    ch: &OffsetChannel,
    cfg: WitnessCfg,
    active: bool,
) -> bool {
    let stage =
        if active { ExtrudeCoverStage::Active } else { ExtrudeCoverStage::ShadowOnly };
    cover_channel_staged(omega, ch, cfg, stage).eta.is_some()
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
    // #384 P5c-A, Blocker 1 des Bogen-Reviews: die Herleitung UND die Rechnung
    // unten sind ausschliesslich das Segmentlemma `q(t) = c + u·(t·r)` — sie
    // lesen weder `v` noch `e`/`D_e`, kennen die Sektor-Zweige von
    // `arc_closest` nicht und budgetieren insbesondere nicht, dass der
    // Produktions-Endpunkt aus dem gespeicherten, NICHT exakt normierten `e`
    // kommt, während `piece_box` dort `normalize(diamond_dir(D_e))` benutzt.
    // Seit P5c-A lässt `arc_usable` gekrümmte Bögen durch; ohne diese Sperre
    // gäbe die Brücke für sie stillschweigend ein Budget zurück, das für sie
    // nie hergeleitet wurde — und der Kanal trüge ein `Proved`, das den
    // f32-Schritt gar nicht abdeckt. Die gekrümmte Brücke ist P5c-B (#463).
    if !(arc.sweep < 0.0) {
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
/// g      = face_plane_gate(p, ch)          // unverändert
/// h      = fl(g + δ)                       // unverändert
/// de_eta = fl(de + A)                      // §8aa — ein f32-Add auf der Naht
/// h_safe = fl(h + A_h)                     // §8ab — ein f32-Add auf der Ebene
/// lim    = fl(δ − min(de_eta, h_safe))     // unverändert
/// ```
///
/// §8ab hat den zweiten Addenden dazugelegt: `A` bezahlt die Nahtdistanz,
/// `A_h` die Stützebene. Beide gehen NUR auf ihren eigenen Ast — das
/// pauschale `δ − min(de,h) − A − A_h` wäre strikt gröber.
///
/// `A` geht **nur** auf die Nahtdistanz. Die Stützebene `h` ist unabhängig
/// bewiesen (§8j) und wird nicht mit-verschlechtert; `δ − min(de+A, h)` ist
/// strikt schärfer als das pauschale `δ − min(de,h) − A`.
///
/// **`A = A_h = 0` ist bitweise das heutige Feld** (`de + 0.0 == de` für jedes
/// `de ≥ 0`, und `de` ist eine Norm). Das ist die Feldneutralitäts-Zusage von
/// P5b-1 für jeden Kanal ohne Addenden — gepinnt, nicht behauptet.
///
/// **Diese Funktion ist NICHT bewiesen** (§8aa-Korrektur, Blocker 1). Bewiesen
/// ist ihr NAHT-Ast: fällt das `min` auf `de_eta`, greift Lemma 5. Fällt es auf
/// `h`, liefert sie `fl(δ − h)` — denselben Ausdruck wie heute, mit demselben
/// fehlenden Budget (Restposten R1: die Stützebenen-Synthese lässt
/// `h_min ≥ −8·gate_eps` zu, und Ebenennorm, `dot(p,n)−c`, `g+δ` sowie `δ−h`
/// laufen ungerichtet in f32). Die erste §8aa-Fassung stellte die Funktion
/// `pub` neben einen scheinbar passenden öffentlichen Beweiswert — genau die
/// Kombination, aus der ein Konsument eine Vollketten-Garantie ablesen würde,
/// die es nicht gibt. Deshalb jetzt `pub(crate)` und nur in Test-Builds
/// vorhanden: sie ist der Gegenstand des Rundungslemmas und der
/// Feldneutralitäts-Pins, **kein Evaluator und keine Zusage**.
#[cfg(test)]
pub(crate) fn frozen_channel_limit(
    p: [f32; 3],
    ch: &OffsetChannel,
    eta_add: f32,
    h_add: f32,
) -> f32 {
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
    frozen_limit_tail(ch.delta, de, face_plane_gate(p, ch) + ch.delta, eta_add, h_add)
}

/// **Der arithmetische Schwanz der eingefrorenen Kette** — genau die drei
/// f32-Ops, über die Lemma 5 spricht, ohne jede Geometrie davor.
///
/// [`frozen_channel_limit`] ruft diese Funktion auf; ein Auseinanderdriften
/// von Beweisgegenstand und Feldform ist damit nicht bloss unwahrscheinlich,
/// sondern unmöglich. Die ulp-Familie der Gegenprobe kann `de`, `h` und `δ`
/// so frei wählen, wie es der Beweis behauptet — über die Geometrie käme man
/// an die interessanten Bitmuster gar nicht heran.
#[cfg(test)]
#[inline]
pub(crate) fn frozen_limit_tail(delta: f32, de: f32, h: f32, eta_add: f32, h_add: f32) -> f32 {
    let de_eta = de + eta_add;
    let h_safe = h + h_add;
    delta - de_eta.min(h_safe)
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
// ─── Lemma 6 — die Kanalbindung, bitweise ───────────────────────────────────
//
// §8aa-Korrektur (dreizehntes Review, Blocker 2): die erste Fassung verglich
// einen **64-Bit-FNV-Wert** und nannte das „Identität". Ein 64-Bit-Hash ist
// notwendigerweise kollisionsfähig; dass sieben ausgewählte Ein-ulp-Mutationen
// keine Kollision erzeugten, beweist nichts über die Menge aller Kanäle. Der
// Hash ist jetzt nur noch **Vorfilter und Diagnose**; autoritativ ist ein
// exakter kanonischer Byte-Schnappschuss, der BITWEISE verglichen wird.

#[inline]
fn put_u64(o: &mut Vec<u8>, v: u64) {
    o.extend_from_slice(&v.to_le_bytes());
}

#[inline]
fn put_f32(o: &mut Vec<u8>, v: f32) {
    // Bitmuster, NICHT der Wert: `-0.0` und `0.0` sind für die Geometrie
    // dasselbe, für einen Schnappschuss aber verschiedene Daten — und
    // Verschiedenheit darf hier nie unter den Tisch fallen.
    o.extend_from_slice(&v.to_bits().to_le_bytes());
}

#[inline]
fn put_f32x3(o: &mut Vec<u8>, v: [f32; 3]) {
    for x in v {
        put_f32(o, x);
    }
}

/// FNV-1a über den Schnappschuss — **Vorfilter und Diagnose, keine Identität.**
fn prefilter(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// #384 §8ac-Korrektur (achtzehntes Review) — **die EINE Serialisierung des
/// Kanal-Schnappschusses.**
///
/// Sie hat zwei Verbraucher: [`channel_snapshot`] baut daraus einen Puffer,
/// [`CertifiedSeamAddend::matches_channel`] hält sie Byte für Byte gegen einen
/// gespeicherten. Eine zweite Handkopie wäre die nächste Lücke — deshalb eine
/// Quelle, zwei Senken.
pub(crate) trait SnapshotSink {
    fn put(&mut self, b: &[u8]);
    /// Darf die Serialisierung abbrechen? (Der Vergleicher weiss oft früh,
    /// dass es nicht mehr passt.)
    fn done(&self) -> bool {
        false
    }
}

struct VecSink(Vec<u8>);
impl SnapshotSink for VecSink {
    #[inline]
    fn put(&mut self, b: &[u8]) {
        self.0.extend_from_slice(b);
    }
}

/// Vergleicht die erzeugten Bytes gegen einen gespeicherten Puffer, ohne zu
/// allokieren.
struct CmpSink<'a> {
    want: &'a [u8],
    pos: usize,
    ok: bool,
}
impl SnapshotSink for CmpSink<'_> {
    #[inline]
    fn put(&mut self, b: &[u8]) {
        if !self.ok {
            return;
        }
        let end = self.pos + b.len();
        if end > self.want.len() || &self.want[self.pos..end] != b {
            self.ok = false;
            return;
        }
        self.pos = end;
    }
    #[inline]
    fn done(&self) -> bool {
        !self.ok
    }
}

#[inline]
fn sn_u64<S: SnapshotSink>(s: &mut S, v: u64) {
    s.put(&v.to_le_bytes());
}

#[inline]
fn sn_f32<S: SnapshotSink>(s: &mut S, v: f32) {
    // Bitmuster, NICHT der Wert: `-0.0` und `0.0` sind für die Geometrie
    // dasselbe, für einen Schnappschuss aber verschiedene Daten.
    s.put(&v.to_bits().to_le_bytes());
}

#[inline]
fn sn_f32x3<S: SnapshotSink>(s: &mut S, v: [f32; 3]) {
    for x in v {
        sn_f32(s, x);
    }
}

/// **Die kanonische Serialisierung eines Kanals** — jedes Bit, das
/// [`crate::offset_channel::channel_limit`] oder [`cover_channel`] liest.
fn serialize_channel<S: SnapshotSink>(ch: &OffsetChannel, s: &mut S) {
    s.put(b"FD384-CH1");
    sn_u64(s, ch.arcs.len() as u64);
    for a in &ch.arcs {
        if s.done() {
            return;
        }
        sn_f32x3(s, a.c);
        sn_f32x3(s, a.u);
        sn_f32x3(s, a.v);
        sn_f32(s, a.r);
        sn_f32(s, a.sweep);
        sn_f32(s, a.e[0]);
        sn_f32(s, a.e[1]);
        sn_f32(s, a.d_e);
        sn_f32x3(s, a.bc);
        sn_f32(s, a.br);
        sn_f32x3(s, a.m1s);
        sn_f32x3(s, a.m2s);
        sn_f32x3(s, a.m1e);
        sn_f32x3(s, a.m2e);
    }
    sn_f32(s, ch.delta);
    s.put(&[ch.concave as u8]);
    sn_f32(s, ch.trust);
    sn_f32(s, ch.gate);
    sn_f32x3(s, ch.bound_c);
    sn_f32(s, ch.bound_r);
    for g in ch.end_planes.iter().chain(ch.face_planes.iter()) {
        for x in g {
            sn_f32(s, *x);
        }
    }
    sn_u64(s, ch.branch);
    sn_u64(s, ch.parents[0]);
    sn_u64(s, ch.parents[1]);
}

/// **Der kanonische Schnappschuss eines Kanals**, als Puffer.
///
/// Er wird bitweise verglichen, nicht gehasht. Zwei Kanäle mit demselben
/// Schnappschuss sind in allen beweisrelevanten Daten identisch — das ist eine
/// Aussage über die Bytes, keine Wahrscheinlichkeitsaussage.
pub fn channel_snapshot(ch: &OffsetChannel) -> Vec<u8> {
    let mut s = VecSink(Vec::with_capacity(64 + ch.arcs.len() * 96));
    serialize_channel(ch, &mut s);
    s.0
}

/// **Der kanonische Schnappschuss von `Ω`** — und zwar genau der Daten, die
/// [`box_outside_omega`] liest.
///
/// **Transfer-Lemma.** `box_outside_inner` liest ausschliesslich: den
/// Varianten-Tag; bei `Leaf` die Weltmatrix und — nur für Box/Sphere/Zylinder —
/// die Primitiv-Parameter (jedes andere Blatt liefert bedingungslos `false`);
/// bei `Extrude` (§8ae) die Weltmatrix, `half_height` und den Profilbaum —
/// dort wiederum nur die Parameter von `Rectangle`/`Circle`/`Polygon`, das
/// `distance` eines Profil-`Offset` und die Struktur der Profil-Booleans
/// (`Sector`/`Path` liefern bedingungslos `false`); bei `Offset` das `delta`;
/// und rekursiv die Kinder von `Union`, `Intersection`, `Difference`,
/// `Offset`, `InnerWall`. **Jede andere Variante
/// liefert bedingungslos `false`.** Ein Schnappschuss, der genau diese Daten
/// kanonisch kodiert, bestimmt deshalb den Wert von [`box_outside_omega`] für
/// JEDE Box und JEDE Marge. Gilt `omega_snapshot(Ω') = omega_snapshot(Ω)`, so
/// überträgt sich jeder für `Ω` geführte Zeugen-Beweis unverändert auf `Ω'`. ∎
///
/// Das ist die Eigenschaft, auf die es ankommt — sie ist **stärker** als
/// „die Bäume sind gleich" es sein müsste und **schärfer** als ein Hash es
/// sein kann. Alle nicht unterstützten Varianten fallen bewusst in EINEN Topf:
/// sie sind für den Beweis ununterscheidbar, weil sie alle nichts beweisen.
fn serialize_omega<S: SnapshotSink>(node: &BoundedCsg, s: &mut S) {
    /// Der Profilbaum unter einem `Extrude` — dieselbe Regel wie oben: alles,
    /// was `sketch_outside_inner` liest, kommt kanonisch rein; alles, was dort
    /// bedingungslos `false` liefert, fällt in EINEN Topf.
    fn sk<S: SnapshotSink>(n: &BoundedSketch2d, s: &mut S) {
        if s.done() {
            return;
        }
        match n {
            BoundedSketch2d::Empty => s.put(&[40]),
            BoundedSketch2d::Prim { primitive, .. } => match primitive {
                Sketch2dPrimitive::Rectangle { center, half_extents } => {
                    s.put(&[41]);
                    sn_f32(s, center[0]);
                    sn_f32(s, center[1]);
                    sn_f32(s, half_extents[0]);
                    sn_f32(s, half_extents[1]);
                }
                Sketch2dPrimitive::Circle { center, radius } => {
                    s.put(&[42]);
                    sn_f32(s, center[0]);
                    sn_f32(s, center[1]);
                    sn_f32(s, *radius);
                }
                Sketch2dPrimitive::Polygon { vertices } => {
                    s.put(&[43]);
                    sn_u64(s, vertices.len() as u64);
                    for v in vertices {
                        sn_f32(s, v[0]);
                        sn_f32(s, v[1]);
                    }
                }
                // `Sector`/`Path`: beweisen nichts, also ununterscheidbar.
                _ => s.put(&[49]),
            },
            BoundedSketch2d::Union { left, right, .. } => {
                s.put(&[44]);
                sk(left, s);
                sk(right, s);
            }
            BoundedSketch2d::Intersection { left, right, .. } => {
                s.put(&[45]);
                sk(left, s);
                sk(right, s);
            }
            BoundedSketch2d::Difference { left, right, .. } => {
                s.put(&[46]);
                sk(left, s);
                sk(right, s);
            }
            BoundedSketch2d::Offset { inner, distance, .. } => {
                s.put(&[47]);
                sn_f32(s, *distance);
                sk(inner, s);
            }
        }
    }
    fn go<S: SnapshotSink>(n: &BoundedCsg, s: &mut S) {
        if s.done() {
            return;
        }
        match n {
            BoundedCsg::Empty => s.put(&[1]),
            BoundedCsg::Leaf { primitive, world_transform, .. } => {
                s.put(&[2]);
                sn_f32x3(s, world_transform.translate);
                for x in world_transform.rotation {
                    sn_f32(s, x);
                }
                sn_f32x3(s, world_transform.scale);
                match primitive {
                    Primitive::Box { center, half_extents } => {
                        s.put(&[21]);
                        sn_f32x3(s, *center);
                        sn_f32x3(s, *half_extents);
                    }
                    Primitive::Sphere { center, radius } => {
                        s.put(&[22]);
                        sn_f32x3(s, *center);
                        sn_f32(s, *radius);
                    }
                    Primitive::Cylinder { center, radius, half_height } => {
                        s.put(&[23]);
                        sn_f32x3(s, *center);
                        sn_f32(s, *radius);
                        sn_f32(s, *half_height);
                    }
                    // Jedes andere Blatt liefert `false` — seine Parameter
                    // sind für den Beweis ununterscheidbar.
                    _ => s.put(&[29]),
                }
            }
            BoundedCsg::Union { left, right, .. } => {
                s.put(&[3]);
                go(left, s);
                go(right, s);
            }
            BoundedCsg::Intersection { left, right, .. } => {
                s.put(&[4]);
                go(left, s);
                go(right, s);
            }
            BoundedCsg::Difference { left, right, .. } => {
                s.put(&[5]);
                go(left, s);
                go(right, s);
            }
            BoundedCsg::Offset { inner, delta, .. } => {
                s.put(&[6]);
                sn_f32(s, *delta);
                go(inner, s);
            }
            BoundedCsg::InnerWall { inner, .. } => {
                s.put(&[7]);
                go(inner, s);
            }
            // #384 §8ae — das Extrusions-Blatt. Kodiert wird es in BEIDEN
            // Stufen: in Stufe A liest der Beweis diese Daten zwar nicht, aber
            // ein Schnappschuss, der bei der Umschaltung erst nachgezogen
            // werden müsste, wäre genau die Sorte Kopplung, die das
            // Transfer-Lemma still ungültig macht.
            BoundedCsg::Extrude { sketch, half_height, world_transform, .. } => {
                s.put(&[9]);
                sn_f32x3(s, world_transform.translate);
                for x in world_transform.rotation {
                    sn_f32(s, x);
                }
                sn_f32x3(s, world_transform.scale);
                sn_f32(s, *half_height);
                sk(sketch, s);
            }
            // Alle übrigen Varianten: bedingungslos `false`, also ein Topf.
            _ => s.put(&[8]),
        }
    }
    s.put(b"FD384-OM1");
    go(node, s);
}

/// Der kanonische `Ω`-Schnappschuss als Puffer. Teilt sich die Serialisierung
/// mit dem Strom-Vergleich in [`CertifiedSeamAddend::matches_omega`] — eine
/// Quelle, zwei Senken, genau wie beim Kanal.
pub fn omega_snapshot(node: &BoundedCsg) -> Vec<u8> {
    let mut s = VecSink(Vec::with_capacity(256));
    serialize_omega(node, &mut s);
    s.0
}


// ════════════════════════════════════════════════════════════════════════════
// §8ab / P5b-1H — das h-Budget: die Stützebene, bewiesen statt angenommen
// ════════════════════════════════════════════════════════════════════════════
//
// §8aa hat den NAHT-Ast bewiesen und den `h`-Ast als Restposten R1 offen
// gelassen: fällt das `min` auf die Stützebene, liefert die Produktion
// `fl(δ − h)`, und dafür gab es weder ein geometrisches noch ein
// Rundungsbudget. Gemessen war R1 dabei alles andere als theoretisch — der
// `h`-Ast gewinnt an 4,1 % der Proben, und die gezielte Bisektion trifft das
// Umhäng-Band an 200 von 200 Strahlen.
//
// P5b-1H schliesst die Lücke — **ohne** das Feld anzufassen.
//
// ## Die Kernidee: global stützen statt lokal
//
// §8j führt die Gültigkeit der Ebene LOKAL (`Ω ∩ B(Naht, 2K) ⊆ {h ≥ 0}`) und
// braucht dafür ein Lokalitäts-Argument über den Lotfusspunkt — das an den
// Rändern des Trust-Radius nicht ganz aufgeht und ausserdem den Fall `h < 0`
// (Auswertungspunkt AUSSERHALB `Ω`) gar nicht erfasst.
//
// Hier wird stattdessen die **globale** Aussage bewiesen:
//
// ```text
// Ω ⊆ { Ĥ ≥ −τ }        mit  Ĥ(x) = ⟨x, n⟩ − C + δ
// ```
//
// Daraus folgt die ganze Zusage in EINER Zeile, ohne Fallunterscheidung und
// ohne Lokalität:
//
// **Halbraum-Lemma.** Ist `Ω ⊆ Ω'`, so gilt `SDF_Ω ≥ SDF_{Ω'}` punktweise,
// also `−SDF_Ω ≤ −SDF_{Ω'}`. Für den Halbraum `Ω' = {Ĥ ≥ −τ}` ist
// `−SDF_{Ω'}(x) = (Ĥ(x) + τ)/|n|` **exakt** — das ist die signierte
// euklidische Distanz zur Ebene. Also
//
// ```text
// −SDF_Ω(x) ≤ (Ĥ(x) + τ) / |n|   für JEDES x.  ∎
// ```
//
// `−SDF_Ω` ist genau die Grösse, die §8j als obere Schranke braucht: innerhalb
// `Ω` ist sie `dist(x,∁Ω)`, ausserhalb ist sie `−dist(x,Ω) < 0`. Beide Fälle
// sind erledigt, und der Punkt AUSSERHALB `Ω` — der bei der lokalen Führung
// durchs Raster fiel — ist mit drin.
//
// ## Was das kostet
//
// `Ω ⊆ {Ĥ ≥ −τ}` global ist eine STÄRKERE Forderung als `Ω ∩ B ⊆ {Ĥ ≥ 0}`
// lokal. Sie kann an Kanälen scheitern, deren Körper weit hinter die Ebene
// reicht. Das ist erwünscht: solche Kanäle bekommen dann **kein** h-Budget und
// werden ehrlich gezählt, statt dass ein Lokalitäts-Argument mit unbewiesenen
// Rändern hingebogen wird.
//
// ## Warum `τ` gemessen und nicht bezahlt wird
//
// Die Synthese lässt eine Stützseiten-Verletzung von `8·gate_eps` ausdrücklich
// zu (`set_face_plane_gates`). `gate_eps` steht dem Beweis aber gar nicht zur
// Verfügung — es ist ein Synthese-Parameter und im Kanal nicht gespeichert.
// Einen unsichtbaren Aufschlag zu „bezahlen" wäre der verbotene Fudge-Faktor.
// Stattdessen wird `τ` hier **selbst hergeleitet**: aus einer strukturellen
// Box-Überdeckung von `Ω` in reeller Intervall-Arithmetik. Damit hängt nichts
// mehr an der f32-Rechnung der Synthese, und der tatsächliche Wert wird
// messbar statt geschätzt.

/// Eine achsenparallele Welt-Box (reell, aussen gerundet).
pub(crate) type WorldBox = ([f64; 3], [f64; 3]);

/// Die Vorwärts-Abbildung `lokal → Welt` einer Box, als **rigorose Inverse der
/// tatsächlichen Rückwärts-Abbildung**.
///
/// §8ab-Korrektur (fünfzehntes Review, Blocker 1). Die erste Fassung baute die
/// Vorwärtsrichtung aus `quat_rotate(q, ·)` und behandelte sie als Umkehrung
/// von `quat_inverse_rotate(q, ·) = quat_rotate(conj q, ·)`. Das sind aber nur
/// bei `|q| = 1` inverse Matrizen: allgemein ist
/// `R(q)·R(conj q) = |q|⁴ · I`-artig verzerrt, und weder `Transform` noch die
/// Deserialisierung erzwingen `|q| = 1` — f32-Quaternion-Komposition liefert
/// ohnehin keine exakt normierte Viererzahl.
///
/// Die Familie des Reviews macht es scharf: `q = (0,0,0.5,0)`, `scale = 1`,
/// lokale Box `[−1,1]³`. Die Rückwärts-Abbildung skaliert in `x`/`y` um `0.5`,
/// `Ω` reicht in Welt-`x`/`y` also bis `±2`; die alte Vorwärtsbox reichte nur
/// bis `±0.5`. Die „Überdeckung" überdeckte nicht — `τ` wurde unterschätzt und
/// ein falsches `HBudget` geprägt.
///
/// **Eine blosse Toleranzprüfung `||q| − 1| < ε` würde das nicht heilen**: sie
/// gäbe keine hergeleitete Box-Aufweitung. Deshalb wird hier die
/// 3×3-Matrix `A` der WIRKLICHEN Rückwärts-Abbildung ([`local_linear`],
/// Spalten `A·e_k`) rigoros in Intervall-Arithmetik **invertiert**. Ist die
/// Determinante nicht beweisbar von null weg (das Intervall enthält 0), gibt
/// es keine Überdeckung — fail-closed.
///
/// Die acht abgebildeten Ecken werden gehüllt: für eine feste affine Abbildung
/// ist die AABB des Bildes genau die AABB der Eckbilder, und über einer
/// Intervall-Matrix ist die Eckhülle eine Obermenge davon.
pub(crate) fn local_box_to_world(t: &Transform, lo: [f64; 3], hi: [f64; 3]) -> Option<WorldBox> {
    // Endlichkeit ausdrücklich (§8ae, sechzehntes Review): weder eine
    // Platzierung noch eine lokale Box mit `NaN`/`±∞` ist ein Beweisgegenstand.
    if !transform_finite(t) || !lo.iter().chain(hi.iter()).all(|v| v.is_finite()) {
        return None;
    }
    // `A`, spaltenweise: `col[k] = A·e_k`.
    let mut col = [[pt(0.0); 3]; 3];
    for k in 0..3 {
        let mut e = [pt(0.0); 3];
        e[k] = pt(1.0);
        col[k] = local_linear(t, e)?;
    }
    // `a[r][c]` — Zeile r, Spalte c.
    let a = |r: usize, c: usize| col[c][r];
    // Determinante und Adjunkte (Kofaktor-Form), alles eingeschlossen.
    let m2 = |r0: usize, r1: usize, c0: usize, c1: usize| -> Option<Iv> {
        rsub(rmul(a(r0, c0), a(r1, c1))?, rmul(a(r0, c1), a(r1, c0))?)
    };
    let c00 = m2(1, 2, 1, 2)?;
    let c01 = m2(1, 2, 0, 2)?;
    let c02 = m2(1, 2, 0, 1)?;
    let det = rsub(
        radd(rmul(a(0, 0), c00)?, rmul(a(0, 2), c02)?)?,
        rmul(a(0, 1), c01)?,
    )?;
    // **Fail-closed**: eine Determinante, die die Null enthält, ist keine
    // beweisbare Inverse. Das fängt `scale = 0`, `q = 0` und jede entartete
    // Komposition ab.
    if det.lo <= 0.0 && det.hi >= 0.0 {
        return None;
    }
    // `A⁻¹ = adj(A)/det`; `adj(A)[r][c] = (−1)^{r+c} · M_{cr}`.
    let mut inv = [[pt(0.0); 3]; 3];
    for r in 0..3 {
        for c in 0..3 {
            let (r0, r1) = ([1, 0, 0][c], [2, 2, 1][c]);
            let (c0, c1) = ([1, 0, 0][r], [2, 2, 1][r]);
            let mut m = m2(r0, r1, c0, c1)?;
            if (r + c) % 2 == 1 {
                m = Iv::new(-m.hi, -m.lo)?;
            }
            inv[r][c] = rdiv(m, det)?;
        }
    }
    let mut wlo = [f64::INFINITY; 3];
    let mut whi = [f64::NEG_INFINITY; 3];
    for cx in 0..8 {
        let l = [
            if cx & 1 == 0 { lo[0] } else { hi[0] },
            if cx & 2 == 0 { lo[1] } else { hi[1] },
            if cx & 4 == 0 { lo[2] } else { hi[2] },
        ];
        for k in 0..3 {
            let mut acc = pt(t.translate[k] as f64);
            for j in 0..3 {
                acc = radd(acc, rmul(inv[k][j], pt(l[j]))?)?;
            }
            wlo[k] = wlo[k].min(acc.lo);
            whi[k] = whi[k].max(acc.hi);
        }
    }
    (wlo.iter().all(|v| v.is_finite()) && whi.iter().all(|v| v.is_finite()))
        .then_some((wlo, whi))
}

/// **Eine strukturelle Box-Überdeckung von `Ω`** — `Ω ⊆ ⋃ Boxen`.
///
/// Die Regeln spiegeln [`box_outside_omega`], nur in die andere Richtung:
///
/// | Knoten | `Ω ⊆ …` |
/// |---|---|
/// | `Empty` | leere Liste |
/// | `Leaf` Box | die abgebildete lokale Box |
/// | `Leaf` Sphere | die abgebildete lokale Hüllbox `c ± r` |
/// | `Leaf` Cylinder | die abgebildete lokale Hüllbox `c ± (r, hh, r)` |
/// | `Union(A,B)` | Überdeckung von `A` PLUS die von `B` |
/// | `Intersection(A,B)` | die von `A` (`A∩B ⊆ A`) |
/// | `Difference(A,B)` | die von `A` (rechts nimmt nur weg) |
/// | `Offset(inner, δ≤0)` | die von `inner` (der erodierte Körper ist TEILMENGE) |
/// | `InnerWall` | die des Kindes |
/// | `Extrude` (§8ae, nur Stufe B) | die abgebildete Profil-Hülle × `±|hh|` |
/// | alles andere | **`None`** — fail-closed |
///
/// `None` heisst: kein h-Budget. Eine Überdeckung, die auch nur ein Blatt
/// rät, wäre wertlos.
pub(crate) fn omega_cover(node: &BoundedCsg) -> Option<Vec<WorldBox>> {
    omega_cover_staged(node, EXTRUDE_COVER_STAGE)
}

/// **Die lokale 2D-Hüllbox EINES Profils** — dieselben Formen, die
/// [`sketch_outside_inner`] beweist, nur in die andere Richtung (§8ae).
///
/// Bewusst NICHT über `BoundedSketch2d::aabb()`: dessen `Offset`-Arm weitet
/// die Box um `distance` auf, und dass die Dilation eines PSEUDO-Abstandsfeldes
/// in dieser Aufweitung liegt, ist kein Satz. Hier wird jede Form selbst
/// gehüllt, positive Profil-Offsets werden verweigert.
///
/// `None` = keine beweisbare Hülle. `Some(None)` = leere Menge (nichts zu
/// überdecken).
#[allow(clippy::type_complexity)]
fn sketch_cover_box(n: &BoundedSketch2d) -> Option<Option<([f64; 2], [f64; 2])>> {
    let hull = |a: Option<([f64; 2], [f64; 2])>, b: Option<([f64; 2], [f64; 2])>| match (a, b) {
        (None, x) | (x, None) => x,
        (Some((alo, ahi)), Some((blo, bhi))) => Some((
            [alo[0].min(blo[0]), alo[1].min(blo[1])],
            [ahi[0].max(bhi[0]), ahi[1].max(bhi[1])],
        )),
    };
    match n {
        BoundedSketch2d::Empty => Some(None),
        // **Endlichkeit zuerst** (§8ae-Korrektur, sechzehntes Review, Blocker
        // 1). Die Akkumulation unten läuft über `f64::min`/`f64::max`, und die
        // VERWERFEN einen `NaN`-Operanden. Ohne dieses Gate lieferte ein
        // Quadrat plus `(NaN,NaN)` die endliche Quadrat-Hülle — eine
        // „Überdeckung", die nichts überdeckt.
        BoundedSketch2d::Prim { primitive, .. } if !sketch_prim_finite(primitive) => None,
        BoundedSketch2d::Prim { primitive, .. } => match primitive {
            Sketch2dPrimitive::Rectangle { center, half_extents } => {
                let mut lo = [0.0f64; 2];
                let mut hi = [0.0f64; 2];
                for k in 0..2 {
                    let (c, e) = (center[k] as f64, (half_extents[k] as f64).abs());
                    lo[k] = next_down(c - e);
                    hi[k] = next_up(c + e);
                }
                Some(Some((lo, hi)))
            }
            Sketch2dPrimitive::Circle { center, radius } => {
                let r = (*radius as f64).abs();
                let mut lo = [0.0f64; 2];
                let mut hi = [0.0f64; 2];
                for k in 0..2 {
                    lo[k] = next_down(center[k] as f64 - r);
                    hi[k] = next_up(center[k] as f64 + r);
                }
                Some(Some((lo, hi)))
            }
            Sketch2dPrimitive::Polygon { vertices } => {
                if vertices.len() < 3 {
                    return None;
                }
                let mut lo = [f64::INFINITY; 2];
                let mut hi = [f64::NEG_INFINITY; 2];
                for v in vertices {
                    for k in 0..2 {
                        lo[k] = lo[k].min(v[k] as f64);
                        hi[k] = hi[k].max(v[k] as f64);
                    }
                }
                Some(Some((lo, hi)))
            }
            // `Sector`/`Path`: keine bewiesene Hülle — fail-closed.
            _ => None,
        },
        BoundedSketch2d::Union { left, right, .. } => {
            Some(hull(sketch_cover_box(left)?, sketch_cover_box(right)?))
        }
        // `A ∩ B ⊆ A` und `A \ B ⊆ A`.
        BoundedSketch2d::Intersection { left, .. } | BoundedSketch2d::Difference { left, .. } => {
            sketch_cover_box(left)
        }
        BoundedSketch2d::Offset { inner, distance, .. } => {
            (distance.is_finite() && *distance <= 0.0)
                .then(|| sketch_cover_box(inner))
                .flatten()
        }
    }
}

/// Wie [`omega_cover`], mit ausdrücklich gewählter Stufe (§8ae).
pub(crate) fn omega_cover_staged(
    node: &BoundedCsg,
    stage: ExtrudeCoverStage,
) -> Option<Vec<WorldBox>> {
    let extrude_ok = stage == ExtrudeCoverStage::Active;
    fn go(n: &BoundedCsg, out: &mut Vec<WorldBox>, extrude_ok: bool) -> bool {
        match n {
            BoundedCsg::Empty => true,
            BoundedCsg::Leaf { primitive, world_transform, .. } => {
                if !primitive_finite(primitive) {
                    return false;
                }
                let (lo, hi) = match primitive {
                    Primitive::Box { center, half_extents } => {
                        let mut lo = [0.0f64; 3];
                        let mut hi = [0.0f64; 3];
                        for k in 0..3 {
                            let c = center[k] as f64;
                            let e = (half_extents[k] as f64).abs();
                            lo[k] = next_down(c - e);
                            hi[k] = next_up(c + e);
                        }
                        (lo, hi)
                    }
                    Primitive::Sphere { center, radius } => {
                        let r = (*radius as f64).abs();
                        let mut lo = [0.0f64; 3];
                        let mut hi = [0.0f64; 3];
                        for k in 0..3 {
                            lo[k] = next_down(center[k] as f64 - r);
                            hi[k] = next_up(center[k] as f64 + r);
                        }
                        (lo, hi)
                    }
                    Primitive::Cylinder { center, radius, half_height } => {
                        let r = (*radius as f64).abs();
                        let hh = (*half_height as f64).abs();
                        let e = [r, hh, r];
                        let mut lo = [0.0f64; 3];
                        let mut hi = [0.0f64; 3];
                        for k in 0..3 {
                            lo[k] = next_down(center[k] as f64 - e[k]);
                            hi[k] = next_up(center[k] as f64 + e[k]);
                        }
                        (lo, hi)
                    }
                    _ => return false,
                };
                match local_box_to_world(world_transform, lo, hi) {
                    Some(b) => {
                        out.push(b);
                        true
                    }
                    None => false,
                }
            }
            // #384 §8ae — das Extrusions-Blatt: die lokale Profil-Hülle mal
            // `[−|hh|, +|hh|]`, danach durch dieselbe rigorose Vorwärts-
            // Abbildung wie jedes andere Blatt. Gate: Stufe B.
            BoundedCsg::Extrude { sketch, half_height, world_transform, .. } => {
                if !extrude_ok || !half_height.is_finite() {
                    return false;
                }
                let Some(prof) = sketch_cover_box(sketch) else { return false };
                // Leeres Profil ⇒ leere Menge ⇒ nichts zu überdecken.
                let Some((plo, phi)) = prof else { return true };
                let hh = (*half_height as f64).abs();
                let lo = [plo[0], plo[1], next_down(-hh)];
                let hi = [phi[0], phi[1], next_up(hh)];
                match local_box_to_world(world_transform, lo, hi) {
                    Some(b) => {
                        out.push(b);
                        true
                    }
                    None => false,
                }
            }
            BoundedCsg::Union { left, right, .. } => {
                go(left, out, extrude_ok) && go(right, out, extrude_ok)
            }
            // `A ∩ B ⊆ A` und `A \ B ⊆ A`: die linke Überdeckung genügt.
            BoundedCsg::Intersection { left, .. } => go(left, out, extrude_ok),
            BoundedCsg::Difference { left, .. } => go(left, out, extrude_ok),
            BoundedCsg::Offset { inner, delta, .. } => {
                delta.is_finite() && *delta <= 0.0 && go(inner, out, extrude_ok)
            }
            BoundedCsg::InnerWall { inner, .. } => go(inner, out, extrude_ok),
            _ => false,
        }
    }
    let mut out = Vec::new();
    go(node, &mut out, extrude_ok).then_some(out)
}

/// Ist dieser Slot der unbelegte Sentinel `[0,0,0,−1e30]`?
#[inline]
fn is_sentinel_plane(g: &[f32; 4]) -> bool {
    g[0] == 0.0 && g[1] == 0.0 && g[2] == 0.0 && g[3] == -END_PLANE_OFF
}

/// **Die Stützseiten-Verletzung `τ` einer Ebene, rigoros hergeleitet.**
///
/// `τ = max(0, − min_{y ∈ ⋃Boxen} Ĥ(y))` mit `Ĥ(y) = ⟨y,n⟩ − C + δ`, wobei
/// `C` der GESPEICHERTE Slot-Wert ist. Damit taucht das von der Synthese
/// verwendete `c₀` nirgends auf: bewiesen wird direkt über die Zahlen, die im
/// Kanal stehen — die Rundung der Synthese kann den Beweis nicht mehr
/// beeinflussen.
///
/// Das Minimum einer affinen Form über einer Box ist separabel und wird an
/// einer Ecke angenommen; jede Achse einzeln, nach UNTEN gerundet.
fn plane_tau(cover: &[WorldBox], n: [f32; 3], c: f32, delta: f32) -> Option<f64> {
    let mut worst = f64::INFINITY;
    for (lo, hi) in cover {
        let mut s = 0.0f64;
        for k in 0..3 {
            let nk = n[k] as f64;
            let t = if nk >= 0.0 { next_down(nk * lo[k]) } else { next_down(nk * hi[k]) };
            s = next_down(s + t);
        }
        let v = next_down(next_down(s - c as f64) + delta as f64);
        worst = worst.min(v);
    }
    if cover.is_empty() {
        // `Ω = ∅`: jede Ebene stützt, `τ = 0`.
        return Some(0.0);
    }
    if !worst.is_finite() {
        return None;
    }
    Some(next_up((-worst).max(0.0)))
}

/// **Die aktive Domäne, rückwärts aus dem f32-Ausstieg hergeleitet.**
///
/// §8ab-Korrektur (fünfzehntes Review, Blocker 2). Die erste Fassung setzte
/// `|p − bound_c| < bound_r + trust` **reell** an und begründete das mit dem
/// Ausstieg von `channel_limit`. Der prüft aber
/// `fl(norm_f32(fl(p − bound_c)) − bound_r) < trust`, und daraus folgt die
/// reelle Ungleichung nicht: Subtraktion, Quadratsumme, Wurzel und die letzte
/// Subtraktion können allesamt UNTERSCHÄTZEN. Ein `next_up` auf der
/// f64-Addition bezahlte nur den letzten Schritt, nicht die f32-Kette davor —
/// `pmax`, `R`, der Normposten und `ρ_aff` waren damit formal zu klein.
///
/// Hergeleitet wird jetzt op-genau, in drei Schritten:
///
/// ```text
/// (1) fl(nb − r) < K            ⇒ nb − r < (K + 2⁻¹⁵⁰)/(1−u)
/// (2) |d| ≤ (nb + 2⁻¹⁵⁰)·(1−u)^{−7/2} + √(7·2⁻¹⁵⁰)      (Norm-Brücke, §8aa Lemma 3)
/// (3) d = fl(p − bound_c) komponentenweise ⇒ |p − bound_c| ≤ |d|/(1−u)
/// ```
///
/// Schritt (1) gilt für beide Vorzeichen von `nb − r`: ist es negativ, ist die
/// Schranke ohnehin erfüllt. Schritt (3) nutzt Hauser — die Subtraktion zweier
/// f32-Zahlen hat im Subnormalbereich keinen Unterlauf-Posten, der relative
/// Fehler bleibt also `u`.
///
/// Zurück kommt eine OBERE Schranke von `|p − bound_c|` über der ganzen
/// aktiven Domäne. Sie ist um `≈ 4·10⁻⁷` relativ grösser als das naive
/// `bound_r + trust`; genau diese Differenz war der Blocker.
pub(crate) fn active_span(ch: &OffsetChannel) -> Option<f64> {
    let (r, k) = (ch.bound_r as f64, ch.trust as f64);
    if !(r.is_finite() && k.is_finite() && k > 0.0) {
        return None;
    }
    let omu = next_down(1.0 - F32_U);
    if !(omu > 0.0) {
        return None;
    }
    // (1)
    let nb = next_up(r + next_up(next_up(k + F32_SUB) / omu));
    // (2) — dieselbe Herleitung wie `norm_bridge_mm`, hier auf `nb` angewandt.
    let mut q = 1.0f64;
    for _ in 0..7 {
        q = next_down(q * omu);
    }
    if !(q > 0.0) {
        return None;
    }
    let inv = next_up(1.0 / next_down(q.sqrt()));
    let d = next_up(
        next_up(next_up(next_up(nb + F32_SUB) * inv) + next_up(next_up(7.0 * F32_SUB).sqrt())),
    );
    // (3)
    let span = next_up(d / omu);
    (span.is_finite() && span >= 0.0).then_some(span)
}

/// **Das h-Budget eines Kanals.**
///
/// Zugesagt ist: fällt das `min` der eingefrorenen Kette auf die Stützebene,
/// so überschreitet `fl(δ − fl(h + A_h))` die gültige Feld-Untergrenze nicht.
/// Zusammen mit [`CertifiedSeamAddend`] ist damit die **vollständige** Kette
/// `fl(δ − min(fl(de+A), fl(h+A_h)))` bewiesen — der Restposten R1 aus §8aa
/// ist geschlossen.
///
/// **Kein Konsument.** Wie beim Naht-Addenden liest kein Evaluator diesen
/// Wert; der Verbrauch ist P5b-1A.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HBudget {
    tau_mm: f64,
    norm_mm: f64,
    reach_mm: f64,
    round_mm: f64,
    encoded_addend: f32,
    slots: u8,
}

impl HBudget {
    /// Die grösste bewiesene Stützseiten-Verletzung über alle belegten Slots
    /// (mm). `0`, wo die Ebene wirklich stützt.
    pub fn tau_mm(&self) -> f64 {
        self.tau_mm
    }
    /// Der Anteil aus `|n| ≠ 1` (mm).
    pub fn norm_mm(&self) -> f64 {
        self.norm_mm
    }
    /// Die bewiesene Reichweiten-Schranke `|Ĥ|` im aktiven Bereich (mm) — die
    /// Grösse, die den ADDITIVEN Normfehler überhaupt erst zulässt.
    pub fn reach_mm(&self) -> f64 {
        self.reach_mm
    }
    /// Der reine Rundungsanteil (mm).
    pub fn round_mm(&self) -> f64 {
        self.round_mm
    }
    /// Der GEOMETRISCHE Anteil `A^geom = τ/ν + R·|1−ν|/ν` (mm) — der Teil, den
    /// das Halbraum-Lemma verlangt, ohne jedes Rundungsbudget.
    pub fn geom_mm(&self) -> f64 {
        next_up(self.tau_mm + self.norm_mm)
    }
    /// Der kodierte f32-Addend (Diagnose; die autoritative Ausgabe ist
    /// [`crate::offset_channel::OffsetChannel::h_addend`]).
    pub fn addend(&self) -> f32 {
        self.encoded_addend
    }
    /// Zahl der belegten (nicht-Sentinel) Ebenen-Slots. `0` heisst: der
    /// `h`-Ast ist an diesem Kanal unerreichbar, die Zusage ist gegenstandslos
    /// wahr.
    pub fn slots(&self) -> u8 {
        self.slots
    }
}

/// Warum ein Kanal (k)ein h-Budget bekommen hat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HStatus {
    /// h-Budget bewiesen.
    Proved,
    /// Alle vier Slots sind Sentinel — der `h`-Ast kann nie binden. Die Zusage
    /// gilt gegenstandslos; ehrlicher, das getrennt zu zählen.
    Vacuous,
    /// Keine strukturelle Box-Überdeckung von `Ω` (nicht unterstütztes Blatt,
    /// `Offset(δ>0)`, unbekannter Knoten).
    NoCover,
    /// Eine belegte Ebene stützt `Ω` global nicht (oder nur mit einem `τ`,
    /// das den Addenden unbrauchbar machen würde).
    NotSupporting,
    /// Entartete Ebenendaten: `|n|` nicht von null weg beschränkt, nicht
    /// endliche Koeffizienten.
    Degenerate,
    /// Das Rundungsbudget lässt sich nicht schliessen.
    NoBudget,
}

/// Die grösste Stützseiten-Verletzung, die wir noch in einen Addenden
/// verwandeln — darüber ist die Ebene keine Stützebene mehr, sondern eine
/// Ausrede. `0.5 mm` ist weit über allem Gemessenen (Maximum der Matrix:
/// s. §8ab) und dient nur als fail-closed-Deckel.
const H_TAU_LIMIT_MM: f64 = 0.5;

/// **Die Herleitung des h-Addenden.**
///
/// Für jeden belegten Slot `k`:
///
/// ```text
/// ν_k = |n_k|                                    (reell, eingeschlossen)
/// τ_k = max(0, −min_Ω Ĥ_k)                       (Box-Überdeckung, s. plane_tau)
/// S_k(x) = (Ĥ_k(x) + τ_k)/ν_k  ≥  −SDF_Ω(x)      (Halbraum-Lemma)
/// ```
///
/// **Die ADDITIVE Form** — und der Beweis, der sie trägt. Aus
/// `S_k − Ĥ_k = (Ĥ_k(1−ν_k) + τ_k)/ν_k` folgt mit einer OBEREN Schranke
/// `|Ĥ_k| ≤ R_k`:
///
/// ```text
/// A_k^geom = (R_k·|1−ν_k| + τ_k) / ν_k     ⇒   S_k ≤ Ĥ_k + A_k^geom.
/// ```
///
/// Die Reichweiten-Schranke `R_k` ist **bewiesen, nicht angenommen** — und
/// zwar über [`active_span`], NICHT über die naive reelle Lesart des
/// Early-Outs: `channel_limit` steigt aus, sobald
/// `fl(norm_f32(fl(p − bound_c)) − bound_r) ≥ trust`, und erst die
/// Rückwärts-Brücke von `active_span` macht daraus eine reelle Schranke
/// `|p − bound_c| ≤ span` (die naive Form `bound_r + trust` wurde in der
/// Review-16-Runde an 13 Punkten widerlegt). Im aktiven Bereich gilt
/// `|p_j| ≤ |bound_c_j| + span`, und damit
/// `|Ĥ_k| ≤ Σ_j |p_j|·|n_kj| + |C_k| + |δ| =: R_k`. Ohne diese Schranke
/// wäre die additive Form nicht zulässig und man müsste die Division
/// `(Ĥ+τ)/|n|` ins Feld nehmen.
///
/// **Ein Addend für den ganzen Kanal.** `A^geom = max_k A_k^geom`, angewandt
/// NACH dem `min` über die Slots. Sound, weil die Zusage für JEDEN Slot
/// einzeln gilt: ist `k'` der f32-Gewinner des `min`, so ist
/// `h ≈ Ĥ_{k'}` und `−SDF_Ω ≤ S_{k'} ≤ Ĥ_{k'} + A_{k'}^geom ≤ Ĥ_{k'} + A^geom`.
/// Welcher Slot gewinnt, ist dem Beweis also gleichgültig — er braucht keinen
/// „richtigen" Gewinner, nur irgendeinen gültigen.
///
/// **Sentinel-Slots** (`[0,0,0,−1e30]`) gehen NICHT ein. Ihr Wert ist
/// `fl(0 − (−1e30)) = 1e30`. Dass sie den `min` nicht gewinnen können, ist
/// seit dem fünfzehnten Review **bewiesen statt behauptet**: für jeden
/// belegten Slot wird die obere f32-Schranke seiner ganzen Auswertungskette
/// (`s3`, inklusive aller Rundungen) gegen `1e30` gehalten. Reicht sie heran —
/// bei grossen, aber gültigen Koordinaten oder `trust` durchaus möglich —,
/// gibt es kein Budget ([`HStatus::NoBudget`]), denn dann könnte der
/// unbelegte Sentinel gewinnen, und für DEN gibt es keinen Halbraumsatz.
/// Sind ALLE Slots Sentinel, ist `h = 1e30` und der `h`-Ast unerreichbar
/// ([`HStatus::Vacuous`]).
///
/// **Das Rundungsbudget** entlang der eingefrorenen Kette
/// `a_k = fl(fl(fl(p₀n₀ + p₁n₁) + p₂n₂) − C_k)`, `g = min_k a_k`,
/// `h = fl(g + δ)`, `hs = fl(h + A_h)`, `lim = fl(δ − min(de_eta, hs))`:
///
/// ```text
/// A_h ≥ [ A^geom + ρ_aff + ρ_h + u·(2R + |δ|) + 3·2⁻¹⁵⁰ ] / (1 − 3u)
/// ```
///
/// mit `ρ_aff` = Rundung der affinen Slot-Auswertung und `ρ_h` = Rundung von
/// `fl(g+δ)`. Dieselbe Bauart wie Lemma 5 des Naht-Astes: die Addition kann
/// nach unten, die Schluss-Subtraktion nach oben runden, und beide sind drin.
pub(crate) fn h_budget(
    ch: &OffsetChannel,
    cover: &[WorldBox],
    knobs: SeamProofKnobs,
) -> Result<HBudget, HStatus> {
    // Der aktive Bereich: `|p − bound_c| ≤ span` aus `active_span` — die
    // op-genaue Rückwärts-Brücke über den f32-Early-Out von `channel_limit`
    // (nicht die naive reelle Lesart `bound_r + trust`).
    let mut pmax = [0.0f64; 3];
    let Some(span) = active_span(ch) else {
        return Err(HStatus::Degenerate);
    };
    for k in 0..3 {
        pmax[k] = next_up((ch.bound_c[k] as f64).abs() + span);
        if !pmax[k].is_finite() {
            return Err(HStatus::Degenerate);
        }
    }
    let d_abs = (ch.delta as f64).abs();
    let mut slots = 0u8;
    let mut a_geom = 0.0f64;
    let (mut tau_max, mut norm_max, mut reach_max) = (0.0f64, 0.0f64, 0.0f64);
    let mut rho_aff = 0.0f64;
    for g in &ch.face_planes {
        if is_sentinel_plane(g) {
            continue;
        }
        let n = [g[0], g[1], g[2]];
        let c = g[3];
        if !(n.iter().all(|v| v.is_finite()) && c.is_finite()) {
            return Err(HStatus::Degenerate);
        }
        // `ν = |n|`, eingeschlossen. Ein `ν`, das die Null enthält, ist keine
        // Ebene — fail-closed.
        let Some(nu) = rnorm([pt(n[0] as f64), pt(n[1] as f64), pt(n[2] as f64)]) else {
            return Err(HStatus::Degenerate);
        };
        if !(nu.lo > 0.5 && nu.hi < 2.0) {
            return Err(HStatus::Degenerate);
        }
        slots += 1;
        // Reichweite `R_k`.
        let mut reach = 0.0f64;
        for k in 0..3 {
            reach = next_up(reach + next_up(pmax[k] * (n[k] as f64).abs()));
        }
        let reach = next_up(next_up(reach + (c as f64).abs()) + d_abs);
        if !reach.is_finite() {
            return Err(HStatus::Degenerate);
        }
        reach_max = reach_max.max(reach);
        // `τ_k` aus der Überdeckung.
        let Some(tau) = plane_tau(cover, n, c, ch.delta) else {
            return Err(HStatus::NotSupporting);
        };
        if !(tau.is_finite() && tau <= H_TAU_LIMIT_MM) {
            return Err(HStatus::NotSupporting);
        }
        let tau = if knobs.h_drop_tau { 0.0 } else { tau };
        tau_max = tau_max.max(tau);
        // `A_k^geom = (R·|1−ν| + τ)/ν`, jede Schranke in die ungünstige Richtung.
        let dev = next_up((1.0 - nu.lo).abs().max((nu.hi - 1.0).abs()));
        let normpart = if knobs.h_drop_norm { 0.0 } else { next_up(reach * dev) };
        norm_max = norm_max.max(next_up(normpart / nu.lo));
        let ak = next_up(next_up(normpart + tau) / nu.lo);
        if !ak.is_finite() {
            return Err(HStatus::NoBudget);
        }
        a_geom = a_geom.max(ak);
        // `ρ_aff`: die Rundung von `fl(fl(fl(p₀n₀+p₁n₁)+p₂n₂) − C)`.
        let mut t = [0.0f64; 3];
        let mut e = 0.0f64;
        for k in 0..3 {
            t[k] = next_up(pmax[k] * (n[k] as f64).abs());
            e = next_up(e + next_up(next_up(F32_U * t[k]) + F32_SUB));
        }
        let s1 = next_up(next_up(t[0] + t[1]) + e);
        e = next_up(e + next_up(next_up(F32_U * s1) + F32_SUB));
        let s2 = next_up(next_up(s1 + t[2]) + e);
        e = next_up(e + next_up(next_up(F32_U * s2) + F32_SUB));
        let s3 = next_up(next_up(s2 + (c as f64).abs()) + e);
        e = next_up(e + next_up(next_up(F32_U * s3) + F32_SUB));
        rho_aff = rho_aff.max(e);
        // §8ab-Korrektur (Blocker 3): **der Sentinel darf den `min` nicht
        // gewinnen.** Das Argument „ein belegter Slot ist durch `R` beschränkt,
        // der Sentinel liefert 1e30" war ungeprüft: bei grossen, aber völlig
        // gültigen Koordinaten kann `a_k` über 1e30 steigen. Dann gewinnt der
        // unbelegte Sentinel, und für DIESEN Gewinner gibt es keinen
        // Halbraumsatz. Also hier beweisen — inklusive Rundung, `s3` ist die
        // obere Schranke des f32-Werts — oder fail-closed.
        if !(s3 < END_PLANE_OFF as f64) {
            return Err(HStatus::NoBudget);
        }
    }
    if slots == 0 {
        return Ok(HBudget {
            tau_mm: 0.0,
            norm_mm: 0.0,
            reach_mm: 0.0,
            round_mm: 0.0,
            encoded_addend: 0.0,
            slots: 0,
        });
    }
    // `ρ_h`: die Rundung von `fl(g + δ)`. `|g + δ| ≤ R + ρ_aff + |δ|`.
    let gd = next_up(next_up(reach_max + rho_aff) + d_abs);
    let rho_h = next_up(next_up(F32_U * gd) + F32_SUB);
    // Das Gesamtbudget.
    let u3 = next_up(3.0 * F32_U);
    let den = next_down(1.0 - u3);
    if !(den > 0.0) {
        return Err(HStatus::NoBudget);
    }
    let mix = next_up(next_up(2.0 * reach_max) + d_abs);
    let num = next_up(
        next_up(next_up(next_up(a_geom + rho_aff) + rho_h) + next_up(F32_U * mix))
            + next_up(3.0 * F32_SUB),
    );
    let a_min = if knobs.h_drop_round { a_geom } else { next_up(num / den) };
    if !(a_min.is_finite() && a_min >= a_geom) {
        return Err(HStatus::NoBudget);
    }
    let Some(mut addend) = f32_ceil(a_min) else {
        return Err(HStatus::NoBudget);
    };
    if let Some(f) = knobs.h_force_addend {
        addend = f;
    }
    for _ in 0..knobs.h_shave_ulps {
        addend = f32::from_bits(addend.to_bits().saturating_sub(1));
    }
    if !(addend.is_finite() && addend >= 0.0) {
        return Err(HStatus::NoBudget);
    }
    Ok(HBudget {
        tau_mm: tau_max,
        norm_mm: norm_max,
        reach_mm: reach_max,
        round_mm: next_up(a_min - a_geom),
        encoded_addend: addend,
        slots,
    })
}

// ─── Der Beweistyp ──────────────────────────────────────────────────────────

/// **Das Siegel des Schreibwegs.**
///
/// Ein `SeamProofToken` lässt sich **nur in diesem Modul** herstellen (das
/// Tupel-Feld ist privat). [`crate::offset_channel::OffsetChannel`] verlangt
/// ihn für den einzigen Weg, auf dem eine Beweis-Zelle beschrieben werden
/// kann. Kein anderes Modul des Kerns kann also eine Klemme setzen —
/// unabhängig davon, was es sich zusammenkopiert.
///
/// §8aa-Korrektur (Blocker 3): vorher war das Feld `pub(crate)`, und
/// „verpflanzungssicher" stand allein auf einem Hash-Vergleich. Jetzt ist der
/// SCHREIBWEG versiegelt und der Vergleich exakt.
///
/// ```compile_fail,E0603
/// // Ausserhalb von `offset_witness` gibt es keinen Weg an das Siegel.
/// // Der Fehlercode ist mit angegeben, damit der Doctest nicht aus einem
/// // BELIEBIGEN Grund grün wird — `compile_fail` ohne Code ist genau die
/// // Falle, in der ein Tippfehler wie ein Beweis aussieht.
/// let _t = fielddraft_kernel::offset_witness::SeamProofToken(());
/// ```
pub struct SeamProofToken(());

impl SeamProofToken {
    /// Nur `offset_witness` prägt Siegel — und nur dort, wo unmittelbar davor
    /// bewiesen wurde.
    fn mint() -> Self {
        SeamProofToken(())
    }
}

/// #384 §8ac-Korrektur (siebzehntes Review, Sollte 2) — **die atomare
/// Zertifizierung des FINALEN Kanals.**
///
/// Vorher war `mint_proof_token` `pub(crate)`: jedes Kernmodul konnte ein
/// Siegel prägen, und damit bestand die dokumentierte Typsystem-Grenze nicht
/// mehr. Jetzt verlässt der Token dieses Modul nie — die Synthese ruft EINE
/// Funktion, die zertifiziert und den Laufzeitmodus in demselben Schritt
/// auflöst.
///
/// Gibt den aufgelösten Modus zurück (für den Tally: gezählt wird, was
/// wirklich aufgelöst wurde, nicht was `certify` gemeldet hat).
pub fn certify_and_resolve_final_channel(
    omega: &BoundedCsg,
    ch: &mut OffsetChannel,
    cfg: WitnessCfg,
) -> crate::offset_channel::ChannelLimitMode {
    let certified = certify_channel(omega, ch, cfg).0 == SeamProofStatus::Proved;
    ch.resolve_limit_mode(SeamProofToken::mint(), omega, certified)
}

/// #384 §8ac-Korrektur — kein `Ω` am Stopp ⇒ kein Beweis, Modus auf Legacy.
/// Auch dieser Weg braucht das Siegel und bekommt es nur hier.
pub fn force_legacy_final_channel(
    ch: &mut OffsetChannel,
    why: crate::offset_channel::LegacyReason,
) {
    ch.force_legacy(SeamProofToken::mint(), why);
}

/// **Der bewiesene Naht-Addend eines Kanals.**
///
/// **Was zugesagt ist — und NUR das.** Für jeden Punkt `x` mit
/// `de = de_f32(x) < trust` gilt
///
/// ```text
/// dist(x, ∁Ω) ≤ de + η_geo + ε_q + ε_d  =:  de + Δ        (Lemma 4)
/// fl32(δ − fl32(de + A)) ≤ δ − de − Δ                      (Lemma 5)
/// ```
///
/// — also die Untergrenze **im Naht-Ast**. Über die vollständige
/// Produktionskette `fl(δ − min(fl(de+A), h))` sagt dieser Typ
/// **ausdrücklich nichts**: fällt das `min` auf die Stützebene `h`, ist der
/// gelieferte Wert `fl(δ − h)`, und der ruht auf der §8j-Stützebenen-Aussage,
/// die **kein** Rundungs- und **kein** geometrisches Budget hat (Restposten
/// R1). Wer die volle Kette braucht, braucht zuerst ein `h`-Budget.
///
/// §8aa-Korrektur (Blocker 1): der Typ hiess `EtaClamp` und seine Doku
/// behauptete genau diese Vollketten-Zusage. Er zertifizierte damit mehr, als
/// bewiesen war — noch ohne Schaden am Feld, weil kein Evaluator ihn liest,
/// aber ein Beweistyp, der zu viel verspricht, ist genau die Falle, in die
/// P5b-1 sonst getappt wäre. Der Name sagt jetzt, was er kann: **Seam**.
///
/// **Die Zahl gibt es nur gegen Kanal UND `Ω`.** Beide werden bitweise gegen
/// den beim Beweis genommenen Schnappschuss gehalten (s. [`channel_snapshot`],
/// [`omega_snapshot`]). Ein 64-Bit-Hash ist zusätzlich gespeichert, aber nur
/// als Vorfilter — er entscheidet nie allein.
#[derive(Debug, Clone, PartialEq)]
pub struct CertifiedSeamAddend {
    geometric_mm: f64,
    q_bridge_mm: f64,
    norm_bridge_mm: f64,
    field_round_mm: f64,
    encoded_addend: f32,
    channel_prefilter: u64,
    channel_snapshot: Vec<u8>,
    omega_prefilter: u64,
    omega_snapshot: Vec<u8>,
    /// #384 §8ab / P5b-1H — das h-Budget, falls es bewiesen ist. Es wohnt in
    /// DERSELBEN versiegelten Zelle wie der Naht-Addend und läuft beim Abruf
    /// durch DIESELBE Schnappschuss-Prüfung: ein zweiter Vergleichs-Code wäre
    /// eine zweite Wahrheit.
    h: Option<HBudget>,
}

impl CertifiedSeamAddend {
    /// Der f32-Addend `A` — nur gegen **den** Kanal und **das** `Ω`, für die er
    /// bewiesen wurde. Beide Vergleiche sind bitweise; der Hash ist bloss der
    /// billige Vorfilter davor.
    pub fn addend_for(&self, ch: &OffsetChannel, omega: &BoundedCsg) -> Option<f32> {
        if !self.channel_bytes_accepted(&channel_snapshot(ch)) {
            return None;
        }
        if !self.matches_omega(omega) {
            return None;
        }
        Some(self.encoded_addend)
    }
    /// #384 §8ac-Korrektur (achtzehntes Review) — **die Freeze-Prüfung:
    /// erzeugt den Schnappschuss des AKTUELLEN Kanals im Strom und hält ihn
    /// Byte für Byte gegen den gespeicherten. Ohne Allokation.**
    ///
    /// Warum exakt und nicht gehasht: die Vorgängerfassung verglich einen
    /// FNV-64-Digest, und der ist **deterministisch** umgehbar — kein
    /// Zufallstreffer nötig. Das letzte gehashte Wort ist `parents[1]`, ein
    /// öffentliches `u64`; der letzte Schritt ist
    /// `D = (q ⊕ parents[1])·P mod 2⁶⁴` mit ungeradem `P`, also mod 2⁶⁴
    /// invertierbar. Nach einer beliebigen beweisrelevanten Mutation lässt sich
    /// `parents[1]' = q' ⊕ (D_alt · P⁻¹)` setzen, und der Digest ist wieder
    /// derselbe. Genau das führt `activation_forged_digest_still_falls_back_384`
    /// vor. Es widerspricht ausserdem der eigenen §8aa-Doktrin: ein FNV-64 darf
    /// nie allein entscheiden.
    pub(crate) fn matches_channel(&self, ch: &OffsetChannel) -> bool {
        let mut c = CmpSink { want: &self.channel_snapshot, pos: 0, ok: true };
        serialize_channel(ch, &mut c);
        c.ok && c.pos == self.channel_snapshot.len()
    }

    /// #384 §8ac-Korrektur (neunzehntes Review) — **die zweite Hälfte des
    /// Beweispaares: ist das AKTUELLE `Ω` noch das bewiesene?**
    ///
    /// Derselbe allokationsfreie Strom-Vergleich wie
    /// [`Self::matches_channel`], nur über `Ω`. Ohne ihn stand die
    /// Laufzeit-Prüfung auf einem Bein: der Kanal-Schnappschuss passt
    /// weiterhin, wenn man einen UNVERÄNDERTEN bewiesenen Kanal unter einen
    /// Stopp mit anderem `Ω` kopiert — die §8aa-B3-Familie, wieder offen.
    pub(crate) fn matches_omega(&self, omega: &BoundedCsg) -> bool {
        let mut c = CmpSink { want: &self.omega_snapshot, pos: 0, ok: true };
        serialize_omega(omega, &mut c);
        c.ok && c.pos == self.omega_snapshot.len()
    }

    /// Wie [`Self::matches_omega`], aber gegen eine **bereits serialisierte**
    /// `Ω`-Byte-Folge.
    ///
    /// §8ac-Korrektur (Review 20, Sollte 6): `resolve_stop_modes_closed` serialisierte
    /// `Ω` EINMAL JE KANAL — bei 24 Kanälen also 24-mal denselben Baum. Der
    /// Vergleich ist derselbe (bitweise, exakt), nur die Erzeugung passiert
    /// jetzt einmal je Stopp-Besuch und wird per `memcmp` gegen jeden Kanal
    /// gehalten. Keine Abschwächung: identische Bytes, identische Entscheidung.
    pub(crate) fn matches_omega_bytes(&self, bytes: &[u8]) -> bool {
        bytes == &self.omega_snapshot[..]
    }

    /// Die autoritative Kanal-Prüfung: Vorfilter, dann bitweiser Vergleich.
    /// Genau EINE Instanz dieser Logik — [`addend_for`](Self::addend_for) und
    /// der Byte-Positions-Pin laufen durch dieselbe Funktion.
    fn channel_bytes_accepted(&self, cs: &[u8]) -> bool {
        prefilter(cs) == self.channel_prefilter && cs == &self.channel_snapshot[..]
    }
    /// **Nur für die Pins** (Review 14, Sollte-11): schickt eine rohe
    /// Byte-Folge durch DIESELBE Prüfung, die `addend_for` nimmt. Vorher
    /// behauptete der Byte-Untertest nur `Vec`-Ungleichheit — das folgt aus
    /// der Sprachsemantik und misst nichts am Abrufpfad.
    #[cfg(test)]
    pub(crate) fn channel_bytes_accepted_for_test(&self, cs: &[u8]) -> bool {
        self.channel_bytes_accepted(cs)
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
    /// Der reine Rundungsanteil des Addenden (mm) — für den NAHT-Ast.
    pub fn field_round_mm(&self) -> f64 {
        self.field_round_mm
    }
    /// Die bewiesene reelle Gesamtschranke `Δ = η_geo + ε_q + ε_d` (mm) — die
    /// Zahl, gegen die Lemma 4 spricht (OHNE Rundungsbudget).
    pub fn bridge_total_mm(&self) -> f64 {
        next_up(next_up(self.geometric_mm + self.q_bridge_mm) + self.norm_bridge_mm)
    }
    /// #384 §8ab — der bewiesene **h**-Addend `A_h`, gegen denselben Kanal und
    /// dasselbe `Ω` wie der Naht-Addend. `None`, wenn dieser Kanal kein
    /// h-Budget hat (dann bleibt sein `h`-Ast auf der Modell-Annahme, s. R1).
    pub fn h_addend_for(&self, ch: &OffsetChannel, omega: &BoundedCsg) -> Option<f32> {
        if !self.channel_bytes_accepted(&channel_snapshot(ch)) {
            return None;
        }
        if !self.matches_omega(omega) {
            return None;
        }
        Some(self.h.as_ref()?.encoded_addend)
    }
    /// Das h-Budget selbst (Diagnose/Bericht).
    pub fn h_budget(&self) -> Option<&HBudget> {
        self.h.as_ref()
    }
    /// Vorfilter-Hashes (Diagnose). **Nicht** die Identität — s. Typ-Doku.
    pub fn prefilters(&self) -> (u64, u64) {
        (self.channel_prefilter, self.omega_prefilter)
    }
    /// Länge der beiden Schnappschüsse in Bytes (Diagnose).
    pub fn snapshot_lens(&self) -> (usize, usize) {
        (self.channel_snapshot.len(), self.omega_snapshot.len())
    }
}

/// Warum ein Kanal (k)einen Naht-Addenden bekommen hat. Der Shadow-Bericht von
/// P5b-0 zählt genau diese Fälle — und `Proved` ist der einzige, in dem ein
/// Wert entsteht.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeamProofStatus {
    /// Naht-Addend bewiesen und versiegelt eingebettet.
    Proved,
    /// Mindestens ein Bogen ist gekrümmt ⇒ Bogen-Zeugen sind P5c.
    Curved,
    /// Cover unvollständig: kein Zeuge, nicht unterstütztes `Ω`-Blatt,
    /// entarteter Bogen, leerer Kanal.
    NoCover,
    /// Cover vollständig, aber `ε_q`, `ε_d` oder das Rundungsbudget scheitern.
    NoBridge,
    /// [`WitnessCfg`] ungültig.
    BadConfig,
    /// #384 §8ab — **Scope-Sperre**: der Kanal ist KONVEX.
    ///
    /// `CertifiedSeamAddend` beweist die konkave Erosions-Aussage
    /// `−SDF_Ω(x) ≤ de + Δ` über `Ω` = dem Körper VOR der Erosion. Am
    /// Dilations-Stopp ist die Dualität eine andere (`∁Ω` ist dort eine
    /// Vereinigung unbeschränkter Komplemente, s. `set_face_plane_gates`), und
    /// einen konvexen Dualsatz gibt es nicht. Bis dahin: fail-closed schon
    /// beim Prägen, nicht erst beim Lesen.
    NotConcave,
}

/// Ablationen für die Gegenproben. `pub(crate)`: ein geschwächter Beweis darf
/// ausserhalb der Pins gar nicht erst entstehen können.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct SeamProofKnobs {
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
    /// #384 §8ab — `τ := 0` im h-Budget.
    pub h_drop_tau: bool,
    /// #384 §8ab — der Normfehler-Anteil `R·|1−ν|/ν := 0`.
    pub h_drop_norm: bool,
    /// #384 §8ab — Rundungsbudget des h-Astes weg: `A_h := ceil_f32(A^geom)`.
    pub h_drop_round: bool,
    /// #384 §8ab — den h-Addenden um `n` ulp verkleinern.
    pub h_shave_ulps: u32,
    /// #384 §8ab — den h-Addenden fest vorgeben.
    pub h_force_addend: Option<f32>,
    /// #384 §8ae — die Extrude-Stufe ÜBERSTIMMEN. `None` = das
    /// Produktions-Gate [`EXTRUDE_COVER_STAGE`]. Nur die Shadow-Messung setzt
    /// das; kein Produktionspfad kommt an diese Knöpfe.
    pub extrude_stage: Option<ExtrudeCoverStage>,
    /// #384 P5c-A — die Stufe der GEKRÜMMTEN Nähte überstimmen. `None` = das
    /// Produktions-Gate [`CURVED_COVER_STAGE`]. Nur die Shadow-Messung setzt
    /// das; kein Produktionspfad kommt an diese Knöpfe.
    pub curved_stage: Option<ExtrudeCoverStage>,
}

/// **Die Zertifizierung EINES Kanals — die einzige Entstehungsstelle eines
/// [`CertifiedSeamAddend`].**
///
/// Reihenfolge der Ausgänge ist Absicht: `BadConfig` vor `Curved` vor
/// `NoCover` vor `NoBridge`. Ein gekrümmter Bogen ist ein STRUKTURELLER
/// Ausschluss (P5c), kein gescheiterter Zeuge — die Zählung darf beides nicht
/// vermengen.
///
/// **Die Invariante von P5b-0**: ein Addend existiert ⟺ jeder Punkt jedes
/// Bogenstücks dieses Kanals ist gedeckt UND jeder Bogen ist gerade UND alle
/// drei Brücken existieren. Andernfalls bleibt das heutige `channel_limit`
/// stehen — ausdrücklich weiter als **unbewiesene Modell-Annahme** (§8j/§8s),
/// nicht still neutralisiert.
///
/// **Lebenszyklus-Auflage für P5b-1** (§8aa-Korrektur, Blocker 3): der Aufruf
/// gehört an den FINALEN Kanal — nach Vertex-Snapping und nach
/// `set_end_plane_gates`/`set_face_plane_gates`. Danach darf keines der
/// beweisrelevanten Kanal- oder `Ω`-Daten mehr unabhängig verändert oder neu
/// gepaart werden. In P5b-0 erzwingt das der Schnappschuss-Vergleich: jede
/// spätere Änderung an Kanal ODER `Ω` macht den Addenden unlesbar.
pub fn certify_channel(
    omega: &BoundedCsg,
    ch: &mut OffsetChannel,
    cfg: WitnessCfg,
) -> (SeamProofStatus, WitnessCover) {
    certify_channel_with(omega, ch, cfg, SeamProofKnobs::default()).0
}

/// Wie [`certify_channel`], gibt zusätzlich den h-Ausgang zurück. `None`
/// heisst: der Naht-Addend ist gescheitert, ein h-Budget wurde gar nicht erst
/// versucht (ohne versiegelte Zelle gäbe es keinen Ort dafür).
pub fn certify_channel_reported(
    omega: &BoundedCsg,
    ch: &mut OffsetChannel,
    cfg: WitnessCfg,
) -> ((SeamProofStatus, WitnessCover), Option<HStatus>) {
    certify_channel_with(omega, ch, cfg, SeamProofKnobs::default())
}

pub(crate) fn certify_channel_with(
    omega: &BoundedCsg,
    ch: &mut OffsetChannel,
    cfg: WitnessCfg,
    knobs: SeamProofKnobs,
) -> ((SeamProofStatus, WitnessCover), Option<HStatus>) {
    let mut hs_out: Option<HStatus> = None;
    // §8ac-Korrektur: `seal_seam_proof` entzieht den Laufzeitmodus selbst —
    // die Invariante sitzt im Schreibweg, nicht hier (achtzehntes Review,
    // Empfehlung 2).
    ch.seal_seam_proof(SeamProofToken::mint(), None);
    if !cfg.valid() {
        return ((SeamProofStatus::BadConfig, cover_channel(omega, ch, cfg)), None);
    }
    // #384 §8ab — Scope-Sperre: nur der konkave Zweig. Die ganze Zusage spricht
    // über `Ω` = dem Körper VOR der Erosion; am Dilations-Stopp ist die
    // Dualität eine andere, und einen konvexen Dualsatz gibt es nicht.
    if !ch.concave {
        return ((
            SeamProofStatus::NotConcave,
            cover_channel_staged(omega, ch, cfg, knobs.extrude_stage.unwrap_or(EXTRUDE_COVER_STAGE)),
        ), None);
    }
    if ch.arcs.is_empty() {
        return ((
            SeamProofStatus::NoCover,
            cover_channel_staged(omega, ch, cfg, knobs.extrude_stage.unwrap_or(EXTRUDE_COVER_STAGE)),
        ), None);
    }
    // Gekrümmt UND unbeweisbar zuerst — ein struktureller Ausschluss, kein
    // Zeugen-Fehlschlag. Seit P5c-A trägt die Konstruktion gekrümmte
    // Minor-Bögen (s. `piece_box`); `Curved` meldet nur noch, was ausserhalb
    // ihres Geltungsbereichs liegt — vor allem ein gerissenes `0 < D_e ≤ 2`.
    // Die Bedingung bleibt bewusst auf `sweep >= 0` beschränkt: ein ENTARTETES
    // SEGMENT ist ein Zeugen-Fehlschlag und gehört nach `NoCover`, nicht hier
    // hinein — sonst vermengt die Zählung genau die beiden Ausgänge, die sie
    // trennen soll.
    let curved_active =
        knobs.curved_stage.unwrap_or(CURVED_COVER_STAGE) == ExtrudeCoverStage::Active;
    if ch
        .arcs
        .iter()
        .any(|a| !(a.sweep < 0.0) && (!curved_active || !arc_usable(a)))
    {
        return ((
            SeamProofStatus::Curved,
            cover_channel_staged(omega, ch, cfg, knobs.extrude_stage.unwrap_or(EXTRUDE_COVER_STAGE)),
        ), None);
    }
    let stage = knobs.extrude_stage.unwrap_or(EXTRUDE_COVER_STAGE);
    let cover = cover_channel_staged(omega, ch, cfg, stage);
    let Some(eta) = cover.eta else {
        return ((SeamProofStatus::NoCover, cover), None);
    };
    let geo = eta.mm();
    if !(geo.is_finite() && geo >= 0.0) {
        return ((SeamProofStatus::NoBridge, cover), None);
    }
    // `ε_q`: das Maximum über alle Bögen — die Zusage gilt für den Kanal, also
    // für seinen schlechtesten Bogen.
    let mut eps_q = 0.0f64;
    for a in &ch.arcs {
        let Some(v) = point_bridge_mm(a) else {
            return ((SeamProofStatus::NoBridge, cover), None);
        };
        eps_q = eps_q.max(v);
    }
    if knobs.drop_q_bridge {
        eps_q = 0.0;
    }
    let Some(mut eps_d) = norm_bridge_mm(ch.trust) else {
        return ((SeamProofStatus::NoBridge, cover), None);
    };
    if knobs.drop_norm_bridge {
        eps_d = 0.0;
    }
    let bridge_total = next_up(next_up(geo + eps_q) + eps_d);
    let (field_round, addend) = if knobs.drop_field_round {
        let Some(a) = f32_ceil(bridge_total) else {
            return ((SeamProofStatus::NoBridge, cover), None);
        };
        (0.0, a)
    } else {
        let Some(v) = round_budget(ch.delta, ch.trust, bridge_total) else {
            return ((SeamProofStatus::NoBridge, cover), None);
        };
        v
    };
    let mut addend = knobs.force_addend.unwrap_or(addend);
    for _ in 0..knobs.shave_ulps {
        addend = f32::from_bits(addend.to_bits().saturating_sub(1));
    }
    if !(addend.is_finite() && addend >= 0.0) {
        return ((SeamProofStatus::NoBridge, cover), None);
    }
    // #384 §8ab — das h-Budget. Es scheitert unabhängig vom Naht-Addenden;
    // scheitert es, bleibt der `h`-Ast dieses Kanals auf der Modell-Annahme.
    let h = match omega_cover_staged(omega, stage) {
        None => {
            hs_out = Some(HStatus::NoCover);
            None
        }
        Some(cover) => match h_budget(ch, &cover, knobs) {
            Ok(b) => {
                hs_out = Some(if b.slots == 0 { HStatus::Vacuous } else { HStatus::Proved });
                Some(b)
            }
            Err(e) => {
                hs_out = Some(e);
                None
            }
        },
    };
    let cs = channel_snapshot(ch);
    let os = omega_snapshot(omega);
    let proof = CertifiedSeamAddend {
        geometric_mm: geo,
        q_bridge_mm: eps_q,
        norm_bridge_mm: eps_d,
        field_round_mm: field_round,
        encoded_addend: addend,
        channel_prefilter: prefilter(&cs),
        channel_snapshot: cs,
        omega_prefilter: prefilter(&os),
        omega_snapshot: os,
        h,
    };
    ch.seal_seam_proof(SeamProofToken::mint(), Some(proof));
    ((SeamProofStatus::Proved, cover), hs_out)
}

/// #384 §8ae — **nur für die Shadow-Messung**: dieselbe Zertifizierung, aber
/// mit ausdrücklich gewählter Extrude-Stufe. Sie schreibt in eine KOPIE des
/// Kanals; feldwirksam wird nichts.
///
/// **`#[cfg(test)]`** (§8ae, sechzehntes Review, Sollte 3). Dies ist der
/// EINZIGE Weg, auf dem ein `Active`-Beweis in eine versiegelte Kanal-Zelle
/// kommt. „Es gibt heute keinen Nicht-Test-Aufrufer" ist eine Beobachtung,
/// keine Struktur; in Produktions-Builds existiert die Funktion jetzt gar
/// nicht — genau wie [`transplant_proof`] seit Review 14.
#[cfg(test)]
pub(crate) fn certify_for_probe(
    omega: &BoundedCsg,
    ch: &mut OffsetChannel,
    cfg: WitnessCfg,
    active: bool,
) -> ((SeamProofStatus, WitnessCover), Option<HStatus>) {
    let stage =
        if active { ExtrudeCoverStage::Active } else { ExtrudeCoverStage::ShadowOnly };
    certify_channel_with(
        omega,
        ch,
        cfg,
        SeamProofKnobs { extrude_stage: Some(stage), ..SeamProofKnobs::default() },
    )
}

/// **NUR für die Gegenproben.** Schiebt den Beweis von `src` über den
/// versiegelten Weg auf `dst`.
///
/// Diese Funktion existiert, damit die Ablehnung PRÜFBAR ist. Ohne sie wäre
/// eine Verpflanzung ausserhalb dieses Moduls zwar unmöglich (das Siegel lässt
/// sich nicht beschaffen), aber eben auch nicht als Pin zu zeigen — und ein
/// unbeobachteter Schutz ist in diesem Programm kein Schutz. In
/// Produktions-Builds existiert sie gar nicht (Review 14, Sollte-10): sonst
/// könnte jedes Crate-Modul sie rufen, und „kein anderes Modul kann
/// verpflanzen" wäre nur noch fast wahr.
#[cfg(test)]
pub(crate) fn transplant_proof(src: &OffsetChannel, dst: &mut OffsetChannel) {
    let p = src.seam_proof().cloned();
    dst.seal_seam_proof(SeamProofToken::mint(), p);
}

/// Der Shadow-Bericht: wie viele Kanäle eines Stopps/Feldes einen bewiesenen
/// **Naht-Addenden** bekommen — und aus welchem Grund die übrigen nicht.
///
/// §8aa-Korrektur (Sollte-15): das Feld hiess `proved_eta` und wurde als
/// „Kanal bewiesen" gelesen. Bewiesen ist der Naht-Ast, nicht die ausgelieferte
/// `channel_limit`-Kette.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SeamProofTally {
    pub proved_seam: usize,
    pub curved: usize,
    pub no_cover: usize,
    pub no_bridge: usize,
    pub bad_config: usize,
    pub not_concave: usize,
    // #384 §8ab — das h-Budget, gezählt UNTER den naht-zertifizierten Kanälen.
    pub h_proved: usize,
    pub h_vacuous: usize,
    pub h_no_cover: usize,
    pub h_not_supporting: usize,
    pub h_degenerate: usize,
    pub h_no_budget: usize,
}

impl SeamProofTally {
    pub fn add(&mut self, s: SeamProofStatus) {
        match s {
            SeamProofStatus::Proved => self.proved_seam += 1,
            SeamProofStatus::Curved => self.curved += 1,
            SeamProofStatus::NoCover => self.no_cover += 1,
            SeamProofStatus::NoBridge => self.no_bridge += 1,
            SeamProofStatus::BadConfig => self.bad_config += 1,
            SeamProofStatus::NotConcave => self.not_concave += 1,
        }
    }
    /// Der h-Ausgang wird NUR für naht-zertifizierte Kanäle gezählt — ohne
    /// Naht-Addend gibt es gar keine versiegelte Zelle, in der ein h-Budget
    /// wohnen könnte.
    pub fn add_h(&mut self, s: HStatus) {
        match s {
            HStatus::Proved => self.h_proved += 1,
            HStatus::Vacuous => self.h_vacuous += 1,
            HStatus::NoCover => self.h_no_cover += 1,
            HStatus::NotSupporting => self.h_not_supporting += 1,
            HStatus::Degenerate => self.h_degenerate += 1,
            HStatus::NoBudget => self.h_no_budget += 1,
        }
    }
    pub fn total(&self) -> usize {
        self.proved_seam
            + self.curved
            + self.no_cover
            + self.no_bridge
            + self.bad_config
            + self.not_concave
    }
    pub fn h_total(&self) -> usize {
        self.h_proved
            + self.h_vacuous
            + self.h_no_cover
            + self.h_not_supporting
            + self.h_degenerate
            + self.h_no_budget
    }
}
