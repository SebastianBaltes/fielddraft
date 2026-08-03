//! #384 §8u — **Die Intervall-Stufe, Paket 1: Zellen statt Stichproben.**
//!
//! §8r hat die Frage gestellt und zur Hälfte beantwortet. Für eine
//! achsenparallele Zelle `C` (Mittelpunkt `c`, Halbdiagonale `ρ`) ist zu
//! zeigen:
//!
//! ```text
//! kein x ∈ C erfüllt   f(x) ≥ 0  ∧  dist(x,∁Ω) > δ + tol
//! ```
//!
//! — die **Unter-Claim-Freiheit** der Zelle. Zwei hinreichende Zweige:
//!
//! 1. **`U(c) + ρ ≤ δ + tol`.** Gültig ohne jede Intervall-Arithmetik, weil
//!    `dist(·,∁Ω)` 1-Lipschitz ist ([`certify_cell`], Zweig 1).
//! 2. **`sup_C f < 0`.** Braucht eine echte obere Schranke des KORRIGIERTEN
//!    Feldes über der Zelle — der Baustein, den §8r/§8s als fehlend benannt
//!    haben. Dieses Modul baut ihn, für einen scharf umrissenen Ausschnitt.
//!
//! ## Was dieses Paket zusagt — und was NICHT
//!
//! Zweig 2 wird ausschliesslich für Stopps ausgewertet, deren Terme
//! **vollständig unterstützt** sind: Vertex-Kugeln ([`crate::offset_channel::vertex_apply`])
//! werden Operation für Operation eingeschlossen; Kanal-Terme werden **nie
//! ausgewertet**, sondern höchstens als über der GANZEN Zelle *neutral*
//! bewiesen (Fernfeld-Skip, s. [`channel_is_neutral`]). Alles andere — ein
//! nicht bewiesener Kanal, ein Rim-Block, ein nicht unterstützter Operator,
//! eine Entartung, NaN, ein entarteter Nenner, ein Budget-Ende — endet in
//! [`Unknown`]. Kein halber Beweis.
//!
//! **Ein Überlauf nach `±∞` ist ausdrücklich KEIN `Unknown`** (§8u-Korrektur):
//! `f32_up`/`f32_down` bilden ihn korrekt ab — genau so rechnet der
//! f32-Auswerter auch —, und Unendlichkeiten sind gültige Intervall-Schranken.
//! Erst wo aus ihnen ein `NaN` entsteht (`∞ − ∞`, `0 · ∞`, `∞/∞`), wird
//! abgebrochen. Die Produktions-Schalter, die selbst auf Endlichkeit prüfen —
//! `best.is_finite()` in `lb_value` —, sind als Schalter modelliert und nicht
//! als Rundungsfrage.
//!
//! **Die Naht-Zertifizierung ist NICHT Teil dieses Pakets.** `channel_limit`
//! steht auf dem §8j-Satz „jeder Naht-Punkt liegt auf `∂Ω`"; dieser Satz ist
//! bis heute eine Modell-Annahme auf Trace-Auflösung (§8s, „Was OFFEN
//! bleibt"). Und er lässt sich mit den Mitteln dieses Moduls auch nicht
//! nachreichen: **`|f| ≤ ε` über einer Menge beweist NICHT, dass die Menge auf
//! dem Rand liegt** — genau das ist die Pseudo-SDF-Falle, und ein noch so
//! enges Intervall um null ändert daran nichts. Was fehlt, ist ein
//! geometrischer AUSSEN-Zeuge (ein Punkt mit bewiesenem `f > 0` in
//! kontrolliertem Abstand) bzw. eine η-fehlerbehaftete Klemme `δ − de − η`.
//! Nichts in diesem Modul darf so gelesen werden, als sei diese Lücke
//! geschlossen.
//!
//! ## Das f32-Modell — warum hier keine geschätzte Fehlerkonstante steht
//!
//! Eingeschlossen wird **nicht** der reelle Wert des idealen Feldes, sondern
//! der Wert, den der f32-Auswerter an einem beliebigen Punkt der Zelle
//! LIEFERT. Das ist der Wert, über den die Zusage spricht, und es macht jedes
//! geschätzte Rundungs-Budget überflüssig: jede Produktions-Operation wird als
//! „exakte reelle Operation, danach Rundung nach f32" modelliert
//! ([`Ctx::fadd`] & Co.). Die exakte reelle Operation wird in f64 mit
//! Aussen-Rundung ([`next_down`]/[`next_up`]) eingeschlossen, die f32-Rundung
//! danach mit **gerichteter** Rundung auf das f32-Gitter ([`f32_down`]/
//! [`f32_up`]).
//!
//! Beide Schritte sind **gerichtete Repräsentierbarkeits-Klammern**, keine
//! Fehlerschätzungen: sie sagen „das nächste darstellbare nach aussen", nicht
//! „so gross wird der Fehler schon nicht werden". Genau darin liegt der
//! Unterschied zu einem `emax`-Budget (§8t) — es gibt hier nichts zu
//! kalibrieren und nichts, das zu knapp gerechnet sein könnte.
//!
//! Für das PRODUKT zweier f32-Punktwerte ist der f64-Schritt exakt
//! (24 + 24 = 48 ≤ 53 Mantissenbits), das Modell dort also scharf. Für Summen
//! gilt das NICHT allgemein — bei grossem Exponentenabstand rundet auch f64
//! (§8u-Korrektur; die erste Fassung behauptete es fälschlich für `+ − ×`).
//! Die Aussen-Rundung ist deshalb überall tragend.
//!
//! ## Warum keine Fremd-Crate
//!
//! `inari` — die einzige ausgereifte Rust-Intervall-Crate — hat ein
//! `compile_error!` für alles ausser x86-64 und unterstützt `wasm32` nicht.
//! Der Grund ist strukturell und trifft jede Crate dieser Bauart: sie stellt
//! die **Rundungsrichtung der FPU** um, und WASM kennt keine gerichteten
//! Rundungsmodi. Die Aussen-Rundung muss deshalb per `next_down`/`next_up`
//! nach jeder Schranke passieren — genau das tut dieses Modul, in rund
//! zweihundert Zeilen statt einer Abhängigkeit.
//!
//! ## Warum kein Lipschitz-Ansatz
//!
//! Der naheliegende Weg wäre eine Lipschitz-Konstante des korrigierten Feldes
//! und `sup_C f ≤ f(c) + Lip·ρ`. §8r hat ihn schon verworfen: die Terme
//! schalten hart (`select`, Kegel-Gates), und §8m hat die latente
//! `safe_sqrt`-Entartung dokumentiert — eine endliche Lipschitz-Konstante
//! existiert nicht. Die Intervall-Auswertung braucht keine: sie schaltet
//! mit ([`Decision::Both`] ⇒ Hülle beider Zweige) statt zu glätten.

use crate::bvh::BoundedCsg;
use crate::offset_channel::{ChannelArc, OffsetChannel, VertexSphere, END_PLANE_OFF, LB_AVAIL};
use crate::offset_lb::{LbBlock, LbPiece, LB_NONE};
use fielddraft_protocol::{Primitive, Transform};

// ════════════════════════════════════════════════════════════════════════════
// Ebene 1 — die Algebra
// ════════════════════════════════════════════════════════════════════════════

/// Der nächstgrössere f64 (IEEE `nextUp`). `NaN` und `+∞` bleiben stehen.
#[inline]
pub fn next_up(x: f64) -> f64 {
    if x.is_nan() || x == f64::INFINITY {
        return x;
    }
    if x == 0.0 {
        // Beide Nullen (`+0.0` und `−0.0`) gehen auf die kleinste positive
        // subnormale Zahl — `−0.0 == 0.0` ist wahr, der Zweig deckt also beide.
        return f64::from_bits(1);
    }
    let b = x.to_bits();
    // Positive f64 sind als Bitmuster monoton steigend, negative fallend.
    f64::from_bits(if x > 0.0 { b + 1 } else { b - 1 })
}

/// Der nächstkleinere f64 (IEEE `nextDown`).
#[inline]
pub fn next_down(x: f64) -> f64 {
    -next_up(-x)
}

/// Das grösste f32 `≤ x`, als f64 zurückgegeben (gerichtete Rundung nach −∞).
///
/// Läuft `x` unter `−f32::MAX`, ist die Antwort `−∞` — genau das, was die
/// f32-Arithmetik dort auch liefert (Überlauf nach unendlich).
#[inline]
pub fn f32_down(x: f64) -> f64 {
    -f32_up(-x)
}

/// Das kleinste f32 `≥ x`, als f64 (gerichtete Rundung nach +∞).
#[inline]
pub fn f32_up(x: f64) -> f64 {
    if x.is_nan() {
        return f64::NAN;
    }
    // `as f32` sättigt seit Rust 1.45 nach ±∞ statt UB; die Rundung ist
    // round-to-nearest-even, kann also unter `x` fallen.
    let y = x as f32;
    if (y as f64) >= x {
        return y as f64;
    }
    // `y < x`, also ist `y` endlich und der nächstgrössere f32 ist ≥ x
    // (zwischen zwei benachbarten f32 liegt kein weiterer).
    let n = if y == 0.0 {
        f32::from_bits(1)
    } else if y > 0.0 {
        f32::from_bits(y.to_bits() + 1)
    } else {
        f32::from_bits(y.to_bits() - 1)
    };
    n as f64
}

/// Ein nichtleeres, aussen gerundetes Intervall reeller Zahlen.
///
/// **Invariante:** `lo ≤ hi`, beide nicht `NaN`. Unendliche Schranken sind
/// erlaubt (das Produktionsfeld kennt `±1e30`-Sentinels und `f32::INFINITY`
/// als leeren Knoten).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Iv {
    pub lo: f64,
    pub hi: f64,
}

impl Iv {
    /// Die ganze Zahlengerade — die maximal konservative Aussage.
    pub const ENTIRE: Iv = Iv { lo: f64::NEG_INFINITY, hi: f64::INFINITY };

    /// `None` bei `NaN` oder verdrehten Schranken (Fail-closed).
    #[inline]
    pub fn new(lo: f64, hi: f64) -> Option<Iv> {
        if lo.is_nan() || hi.is_nan() || lo > hi {
            None
        } else {
            Some(Iv { lo, hi })
        }
    }

    /// Ein exakter Punkt.
    #[inline]
    pub fn point(x: f64) -> Iv {
        Iv { lo: x, hi: x }
    }

    /// Eine f32-Konstante — exakt nach f64 gehoben (Domänen-Containment,
    /// Beweispflicht 2: f32 ⊂ f64, die Hebung ist verlustfrei).
    #[inline]
    pub fn f32p(x: f32) -> Iv {
        Iv { lo: x as f64, hi: x as f64 }
    }

    #[inline]
    pub fn width(self) -> f64 {
        self.hi - self.lo
    }

    #[inline]
    pub fn contains(self, x: f64) -> bool {
        x >= self.lo && x <= self.hi
    }

    /// Die konvexe Hülle beider Intervalle — **exakt**, keine Rundung
    /// (`min`/`max` wählen einen Operanden aus, sie rechnen nicht).
    #[inline]
    pub fn hull(self, o: Iv) -> Iv {
        Iv { lo: self.lo.min(o.lo), hi: self.hi.max(o.hi) }
    }

    /// Der Schnitt, `None` wenn leer.
    #[inline]
    pub fn intersect(self, o: Iv) -> Option<Iv> {
        Iv::new(self.lo.max(o.lo), self.hi.min(o.hi))
    }

    /// Negation — exakt.
    #[inline]
    pub fn neg(self) -> Iv {
        Iv { lo: -self.hi, hi: -self.lo }
    }

    /// Komponentenweises `min` — exakt.
    #[inline]
    pub fn min(self, o: Iv) -> Iv {
        Iv { lo: self.lo.min(o.lo), hi: self.hi.min(o.hi) }
    }

    /// Komponentenweises `max` — exakt.
    #[inline]
    pub fn max(self, o: Iv) -> Iv {
        Iv { lo: self.lo.max(o.lo), hi: self.hi.max(o.hi) }
    }

    /// Betrag — exakt. Enthält das Intervall die Null, ist die untere
    /// Schranke null.
    #[inline]
    pub fn abs(self) -> Iv {
        if self.lo >= 0.0 {
            self
        } else if self.hi <= 0.0 {
            self.neg()
        } else {
            Iv { lo: 0.0, hi: (-self.lo).max(self.hi) }
        }
    }

    /// Sicher `< o` über dem ganzen Intervall?
    #[inline]
    pub fn certainly_lt(self, o: Iv) -> bool {
        self.hi < o.lo
    }
    /// Sicher `≤ o`?
    #[inline]
    pub fn certainly_le(self, o: Iv) -> bool {
        self.hi <= o.lo
    }
    /// Sicher `> o`?
    #[inline]
    pub fn certainly_gt(self, o: Iv) -> bool {
        self.lo > o.hi
    }
    /// Sicher `≥ o`?
    #[inline]
    pub fn certainly_ge(self, o: Iv) -> bool {
        self.lo >= o.hi
    }
}

/// Das Ergebnis eines Schalter-Tests über der ganzen Zelle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Die Bedingung gilt für JEDEN Punkt der Zelle.
    True,
    /// Sie gilt für KEINEN.
    False,
    /// Unentschieden — beide Zweige sind erreichbar, es MUSS gehüllt werden.
    Both,
}

/// `a ≤ b` über der Zelle.
#[inline]
pub fn decide_le(a: Iv, b: Iv) -> Decision {
    if a.certainly_le(b) {
        Decision::True
    } else if a.certainly_gt(b) {
        Decision::False
    } else {
        Decision::Both
    }
}

/// `a > b` über der Zelle (die Negation von [`decide_le`]).
#[inline]
pub fn decide_gt(a: Iv, b: Iv) -> Decision {
    match decide_le(a, b) {
        Decision::True => Decision::False,
        Decision::False => Decision::True,
        Decision::Both => Decision::Both,
    }
}

/// `a < b` über der Zelle.
#[inline]
pub fn decide_lt(a: Iv, b: Iv) -> Decision {
    if a.certainly_lt(b) {
        Decision::True
    } else if a.certainly_ge(b) {
        Decision::False
    } else {
        Decision::Both
    }
}

/// `a ≥ b` über der Zelle.
#[inline]
pub fn decide_ge(a: Iv, b: Iv) -> Decision {
    match decide_lt(a, b) {
        Decision::True => Decision::False,
        Decision::False => Decision::True,
        Decision::Both => Decision::Both,
    }
}

// ════════════════════════════════════════════════════════════════════════════
// Fail-closed: Unknown, Budget, Kontext
// ════════════════════════════════════════════════════════════════════════════

/// **Kein Ergebnis.** Der benannte Grund, warum die Einschliessung abgebrochen
/// hat. Ein `Unknown` ist nie „vermutlich in Ordnung" — es ist die Weigerung,
/// etwas zu behaupten.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unknown(pub &'static str);

/// Eine Einschliessung — oder ein benanntes Nichtwissen.
pub type Enc = Result<Iv, Unknown>;

/// Die Hülle zweier Zweige. Ist EINER unbekannt, ist es die Hülle auch —
/// eine Hülle über einem unbekannten Zweig wäre eine Erfindung.
#[inline]
pub fn hull_enc(a: Enc, b: Enc) -> Enc {
    Ok(a?.hull(b?))
}

/// **Rückdreh-Schalter — ausschliesslich für die Gegenproben.**
///
/// Die Doktrin verlangt, dass die Test-Familien die Soundness wirklich TRAGEN
/// und nicht bloss zufällig grün sind. Dafür muss man an genau einer Stelle
/// die Sicherung ziehen können. Die Zertifizierung baut den Kontext immer mit
/// [`Default`] (beide Schalter aus); nur
/// `interval_counter_probe_*_384` setzt sie.
/// **Warum „Rundung weglassen" als Gegenprobe nicht taugt.** Die
/// f64-Aussen-Rundung ist überall tragend — die erste §8u-Fassung behauptete
/// hier, sie sei für `+ − ×` an f32-Endpunkten ein No-op, und das war für die
/// ADDITION falsch (§8u-Korrektur): exakt ist in f64 nur das PRODUKT zweier
/// f32 (24 + 24 = 48 ≤ 53 Mantissenbits); eine Summe mit grossem
/// Exponentenabstand ist es nicht. Sie bleibt trotzdem schwer zu
/// falsifizieren, weil der nachfolgende f32-Schritt viel gröber ist und die
/// f64-Unschärfe fast immer verschluckt — das ist eine Aussage über das
/// Sampling, nicht über die Notwendigkeit. Falsifizierbar wird die Sicherung
/// erst, wenn man sie — wie schon `emax = −0.05` in §8t — **zu scharf**
/// stellt: ein ulp nach INNEN.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Knobs {
    /// Ein ulp nach INNEN statt nach aussen runden. Damit ist keine einzige
    /// Schranke mehr garantiert.
    pub inward_rounding: bool,
    /// Statt der Hülle den ERSTEN erreichbaren Zweig nehmen — der klassische
    /// „das wird schon der richtige sein"-Fehler.
    pub first_branch_only: bool,
    /// Die Quaternion-Auswertung **reassoziieren** (`v + (qw·t + qy·t₂ − qz·t₁)`
    /// statt der Produktions-Klammerung `((v + qw·t) + qy·t₂) − qz·t₁`). Über
    /// ℝ identisch, über f32 nicht — das war der erste blockierende Befund des
    /// vierten Mathe-Reviews.
    pub reassociate_rotation: bool,
    /// Den ERSTEN Kandidaten nehmen, statt über alle zu hüllen (§8v/§8w). Der
    /// klassische Argmin-Fehler: `q*`, `m̂₁`, `m̂₂` springen beim
    /// Gewinnerwechsel, ein blind gewählter Kandidat ist also keine
    /// Einschliessung.
    pub first_arc_wins: bool,
    /// Genau EINEN Kandidaten aus der Hülle streichen (§8w-Gegenprobe). Milder
    /// als `first_arc_wins` und damit die schärfere Probe: sie trifft nur den
    /// Vereinigungs-Schritt, nicht die ganze Kandidaten-Bestimmung.
    pub drop_last_candidate: bool,
    /// Annehmen, Bogen 0 werde IMMER ausgewertet (§8w-Korrektur-Gegenprobe).
    /// Genau diese Annahme war der Blocker des sechsten Reviews: läuft `dbs₀`
    /// nach `+∞` über, überspringt die Produktion auch den ersten Bogen und
    /// liefert `None`.
    pub assume_first_arc_evaluated: bool,
    /// #384 §8z: nur die ERSTE Alternative eines Bogens nehmen (Gegenprobe).
    pub arc_first_alt_only: bool,
    /// #384 §8z: die letzte Alternative streichen (Gegenprobe).
    pub arc_drop_last_alt: bool,
    /// #384 §8z: den Fusspunkt-Ast weglassen (Gegenprobe).
    pub arc_drop_foot_alt: bool,
    /// #384 §8z: den Endpunkt-Ast weglassen (Gegenprobe).
    pub arc_drop_end_alt: bool,
    /// #384 §8z: `q`/`t` VOR dem Downstream getrennt hüllen (Gegenprobe —
    /// sound, aber nachweislich breiter).
    pub arc_hull_qt_early: bool,
    /// #384 §8z: `ρ` NICHT vor der Division schneiden (Gegenprobe).
    pub arc_skip_rho_intersect: bool,
    /// #384 §8z: gekrümmte Bögen wieder pauschal verweigern — die **Ablation**,
    /// die den A1-Erfolg gezielt entfernt.
    pub arc_refuse_curved: bool,
    /// #384 §8z: ein unbekanntes `L` verwirft wieder den ganzen Stopp
    /// (Ablation der `Entire`-Brücke).
    pub lb_unknown_rejects: bool,
    /// Den `best.is_finite()`-Schalter in `lb_value` **weglassen**. Genau der
    /// fehlte in der ersten §8u-Fassung; ein modelliertes `+∞` wählt dann den
    /// `L`-Zweig, während die Produktion den `G`-Zweig nimmt.
    pub skip_lb_finite_gate: bool,
}

/// Auswertungs-Kontext: Rundungs-Politik und Terminierungs-Budget.
#[derive(Debug, Clone)]
pub struct Ctx {
    pub knobs: Knobs,
    ops: u64,
    max_ops: u64,
    channel_evals: u64,
    max_candidates: usize,
    curved_arc_evals: u64,
    curved_downstream: u64,
}

impl Ctx {
    /// Ein Kontext mit dem üblichen Budget.
    ///
    /// **Es ist ein GEMEINSAMES Traversal-Budget, kein Budget je Zelle**
    /// (§8u-Korrektur): derselbe `Ctx` wird durch die adaptive Rekursion
    /// gereicht, die 4·10⁶ Operationen gelten also für den ganzen Aufruf. Das
    /// ist die konservative Richtung — ein erschöpftes Budget endet in
    /// `Unknown`, nie in einer Zertifizierung.
    pub fn new() -> Ctx {
        Ctx {
            knobs: Knobs::default(),
            ops: 0,
            max_ops: 4_000_000,
            channel_evals: 0,
            max_candidates: 0,
            curved_arc_evals: 0,
            curved_downstream: 0,
        }
    }

    pub fn with_knobs(knobs: Knobs) -> Ctx {
        Ctx { knobs, ..Ctx::new() }
    }

    /// Verbrauchte Operationen (Terminierungs-Zeuge).
    pub fn ops(&self) -> u64 {
        self.ops
    }

    /// Wie oft ein Kanal-Term wirklich AUSGEWERTET wurde (also nicht über den
    /// Fernfeld-Skip neutral war). Ein Pin, der nur zählt, wie viele Zellen
    /// eine Einschliessung liefern, könnte auch dann grün sein, wenn kein
    /// einziger Kanal je gerechnet wurde.
    pub fn channel_evals(&self) -> u64 {
        self.channel_evals
    }

    /// Die grösste Kandidaten-Menge, die das Selektions-Lemma (§8w) auf diesem
    /// Kontext gebildet hat. `1` heisst „überall eindeutig", `≥ 2` heisst
    /// „mindestens einmal gehüllt".
    pub fn max_candidates(&self) -> usize {
        self.max_candidates
    }

    /// #384 §8z: wie oft ein GEKRÜMMTER Bogen eingeschlossen wurde. Das
    /// Erfolgskriterium von A1 verlangt eine Zertifizierung, an der ein
    /// gekrümmter Kandidat tatsächlich beteiligt war — nicht bloss eine, die
    /// zufällig neben einem liegt.
    pub fn curved_arc_evals(&self) -> u64 {
        self.curved_arc_evals
    }

    /// #384 §8z-Korrektur — **der starke Beteiligungs-Nachweis.**
    ///
    /// [`Self::curved_arc_evals`] zählt am EINTRITT von `enc_arc_closest`,
    /// also VOR der §8w-Auswahl (`can_update`) und vor dem Downstream. Es
    /// belegt damit nur „ein gekrümmter Bogen wurde eingeschlossen" — nicht
    /// „ein gekrümmter Kandidat hat die Auswahl überlebt und sein Downstream
    /// ist in die Hülle eingegangen". Dieser Zähler wird erst in
    /// [`enclose_channel_apply`] innerhalb der Kandidaten-/Alternativen-Schleife
    /// erhöht, und zwar nur für einen wirklich gekrümmten Bogen.
    pub fn curved_downstream(&self) -> u64 {
        self.curved_downstream
    }

    #[inline]
    fn tick(&mut self) -> Result<(), Unknown> {
        self.ops += 1;
        if self.ops > self.max_ops {
            Err(Unknown("Budget"))
        } else {
            Ok(())
        }
    }

    /// **Der Abschluss einer Produktions-Operation** — die EINE Stelle, an
    /// der die Rundungs-Politik sitzt. Zwei Schritte, beide tragend:
    ///
    /// 1. **f64-Aussen-Rundung.** Die reelle Reichweite der Operation über den
    ///    Operanden-Intervallen wird in f64 gerechnet; `next_down`/`next_up`
    ///    fangen die f64-Rundung ab. (Exakt ist der f64-Schritt nur beim
    ///    PRODUKT zweier f32 — 24 + 24 = 48 ≤ 53 Mantissenbits. Bei `+`/`−`
    ///    mit grossem Exponentenabstand und bei `÷`/`√` ist er es nicht.)
    /// 2. **Gerichtete f32-Rundung.** OHNE diesen Schritt wäre das Modell
    ///    **unsound**: der Produktionswert ist `fl32(reell)`, und `fl32`
    ///    rundet zur nächsten Zahl — also unter Umständen ÜBER die reelle
    ///    obere Schranke hinaus. `f32_up` deckt genau das ab.
    #[inline]
    fn seal(&self, lo: f64, hi: f64) -> Enc {
        if self.knobs.inward_rounding {
            // Gegenprobe: ein ulp nach INNEN — die zu scharfe Schranke.
            return Iv::new(f32_up(next_up(lo)), f32_down(next_down(hi))).ok_or(Unknown("NaN"));
        }
        Iv::new(f32_down(next_down(lo)), f32_up(next_up(hi))).ok_or(Unknown("NaN"))
    }

    /// f32-Addition.
    #[inline]
    pub fn fadd(&mut self, a: Iv, b: Iv) -> Enc {
        self.tick()?;
        self.seal(a.lo + b.lo, a.hi + b.hi)
    }

    /// f32-Subtraktion.
    #[inline]
    pub fn fsub(&mut self, a: Iv, b: Iv) -> Enc {
        self.tick()?;
        self.seal(a.lo - b.hi, a.hi - b.lo)
    }

    /// f32-Multiplikation (Vier-Produkte-Form).
    #[inline]
    pub fn fmul(&mut self, a: Iv, b: Iv) -> Enc {
        self.tick()?;
        let p = [a.lo * b.lo, a.lo * b.hi, a.hi * b.lo, a.hi * b.hi];
        if p.iter().any(|v| v.is_nan()) {
            // `0 · ∞` — fail-closed statt geraten.
            return Err(Unknown("NaN(mul)"));
        }
        let lo = p.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = p.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        self.seal(lo, hi)
    }

    /// f32-Division. **Nur mit zertifiziert null-freiem Nenner** — ein
    /// null-haltiger Nenner ist kein „grosses Intervall", er ist keine Zahl.
    #[inline]
    pub fn fdiv(&mut self, a: Iv, b: Iv) -> Enc {
        self.tick()?;
        if b.lo <= 0.0 && b.hi >= 0.0 {
            return Err(Unknown("Division durch nullhaltiges Intervall"));
        }
        let p = [a.lo / b.lo, a.lo / b.hi, a.hi / b.lo, a.hi / b.hi];
        if p.iter().any(|v| v.is_nan()) {
            return Err(Unknown("NaN(div)"));
        }
        let lo = p.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = p.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        self.seal(lo, hi)
    }

    /// **Das Quadrat** — `x*x` mit DEMSELBEN Operanden (#384 §8z).
    ///
    /// Die generische Vier-Produkte-Regel behandelt die beiden Faktoren als
    /// unabhängig und liefert für ein nullüberspannendes `x` fälschlich eine
    /// NEGATIVE untere Schranke (`x.lo·x.hi < 0`). Die Produktion rechnet aber
    /// `x*x`, und ein f32-Quadrat ist nie negativ. Die korrekte — und engere —
    /// Einschliessung ist `[0, max(x.lo², x.hi²)]`, sobald `x` die Null
    /// enthält, sonst das Intervall der Endpunkt-Quadrate.
    #[inline]
    pub fn fsquare(&mut self, a: Iv) -> Enc {
        self.tick()?;
        let (l2, h2) = (a.lo * a.lo, a.hi * a.hi);
        if l2.is_nan() || h2.is_nan() {
            return Err(Unknown("NaN(square)"));
        }
        let lo = if a.lo <= 0.0 && a.hi >= 0.0 { 0.0 } else { l2.min(h2) };
        let r = self.seal(lo, l2.max(h2))?;
        // Wie bei `fsqrt`: die Aussen-Rundung darf die untere Schranke nicht
        // unter null drücken — ein f32-Quadrat ist nie negativ. Ohne diese
        // Klemme läge `lo` beim kleinsten negativen Subnormal, und der
        // Kommentar „`[0, …]`" wäre schlicht falsch.
        Iv::new(r.lo.max(0.0), r.hi.max(0.0)).ok_or(Unknown("NaN(square)"))
    }

    /// **Die sichere Wurzel** (§8u-Lemma 8).
    ///
    /// `sqrt([l,h] ∩ [0,∞)) = [√max(0,l), √max(0,h)]` ist eine korrekte
    /// Einschliessung, AUCH wenn `l < 0 ≤ h`: der Produktionsradikand ist an
    /// diesen Stellen eine Quadratsumme bzw. ein Quotient nichtnegativer
    /// Grössen und damit in f32 nie negativ; ein negatives `l` ist
    /// ausschliesslich MEINE Aussen-Rundung. Die unbeschränkte Ableitung bei
    /// null ist für die Intervall-Arithmetik irrelevant — sie zerstört
    /// Lipschitz-Beweise, nicht Einschliessungen.
    ///
    /// Ist das GANZE Intervall negativ (`h < 0`), ist etwas anderes kaputt als
    /// die Rundung: Fail-closed.
    #[inline]
    pub fn fsqrt(&mut self, a: Iv) -> Enc {
        self.tick()?;
        if a.hi < 0.0 {
            return Err(Unknown("sqrt(<0)"));
        }
        let r = self.seal(a.lo.max(0.0).sqrt(), a.hi.max(0.0).sqrt())?;
        // Die f32-Wurzel eines nichtnegativen Radikanden ist nichtnegativ; die
        // Aussen-Rundung darf die untere Schranke nicht unter null drücken.
        Iv::new(r.lo.max(0.0), r.hi.max(0.0)).ok_or(Unknown("NaN(sqrt)"))
    }

    /// Skalarprodukt in **Produktions-Reihenfolge**: `((a₀b₀ + a₁b₁) + a₂b₂)`.
    /// Die Klammerung ist nicht kosmetisch — sie bestimmt, welche Rundungen
    /// wann passieren, und das Modell muss der echten Reihenfolge folgen
    /// (Beweispflicht 5).
    #[inline]
    pub fn fdot(&mut self, a: [Iv; 3], b: [Iv; 3]) -> Enc {
        let m0 = self.fmul(a[0], b[0])?;
        let m1 = self.fmul(a[1], b[1])?;
        let m2 = self.fmul(a[2], b[2])?;
        let s = self.fadd(m0, m1)?;
        self.fadd(s, m2)
    }

    /// `|v|` in Produktions-Reihenfolge (`dot(v,v).sqrt()`).
    ///
    /// **Zur Dependency-Breite:** sind die drei Komponenten UNABHÄNGIGE
    /// affine Formen der Zellkoordinaten (der Regelfall `v = p − const`), ist
    /// diese natürliche Auswertung bereits die EXAKTE Reichweite — die
    /// Quadratsumme separiert dann über die Koordinaten, und das Minimum bzw.
    /// Maximum wird komponentenweise angenommen. Erst wo die Komponenten
    /// korreliert sind (`rad = d − ax·⟨d,ax⟩` in `rev_dist`) bleibt echte
    /// Überschätzung übrig; sie kostet dort Abdeckung, nie Soundness.
    #[inline]
    pub fn fnorm(&mut self, v: [Iv; 3]) -> Enc {
        let q = self.fdot(v, v)?;
        self.fsqrt(q)
    }

}

impl Default for Ctx {
    fn default() -> Self {
        Ctx::new()
    }
}

/// **Der Schalter** (Beweispflicht 4): ein Zweig darf NUR allein stehen, wenn
/// die Bedingung über der GANZEN Zelle entschieden ist; sonst die Hülle beider
/// erreichbaren Zweige.
#[inline]
fn sw<F, G>(c: &mut Ctx, d: Decision, on_true: F, on_false: G) -> Enc
where
    F: FnOnce(&mut Ctx) -> Enc,
    G: FnOnce(&mut Ctx) -> Enc,
{
    match d {
        Decision::True => on_true(c),
        Decision::False => on_false(c),
        Decision::Both => {
            if c.knobs.first_branch_only {
                return on_true(c);
            }
            hull_enc(on_true(c), on_false(c))
        }
    }
}

// ════════════════════════════════════════════════════════════════════════════
// Ebene 2 — Domäne und geometrische Reichweiten-Funktionen
// ════════════════════════════════════════════════════════════════════════════

/// **Die Domänen-Grenze.** Ein Auswertungs-Gebiet liefert die drei
/// Welt-Koordinaten als Intervalle — mehr braucht die Algebra nicht zu wissen.
///
/// Heute gibt es genau eine Implementierung ([`Box3`]). Die Grenze ist so
/// geschnitten, dass die späteren Pakete (`LineParam` mit `t ∈ [t₀,t₁]`,
/// `ArcParam` mit einem Winkelintervall) hier andocken können, OHNE die
/// Algebra oder die Einschliessungs-Funktionen anzufassen: sie sehen
/// ausschliesslich `[Iv; 3]`.
pub trait CellDomain {
    /// Die Weltkoordinaten, bereits so geweitet, dass sie **jede
    /// f32-Rundung** eines Gebiets-Punktes enthalten.
    fn world_f32(&self) -> [Iv; 3];
    /// Der Mittelpunkt (für Zweig 1 und für die Unterteilung).
    fn center(&self) -> [f64; 3];
    /// Die Halbdiagonale, **aufgerundet**.
    fn rho(&self) -> f64;
}

/// Eine achsenparallele Zelle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Box3 {
    pub lo: [f64; 3],
    pub hi: [f64; 3],
}

impl Box3 {
    pub fn new(lo: [f64; 3], hi: [f64; 3]) -> Box3 {
        Box3 { lo, hi }
    }

    /// Eine Würfel-Zelle um `c` mit Halbkante `h`.
    pub fn cube(c: [f64; 3], h: f64) -> Box3 {
        Box3 { lo: [c[0] - h, c[1] - h, c[2] - h], hi: [c[0] + h, c[1] + h, c[2] + h] }
    }

    /// Halbieren entlang der längsten Achse (adaptive Unterteilung).
    ///
    /// **`None`, wenn die Zelle numerisch nicht mehr teilbar ist** (§8u-Korrektur):
    /// bei benachbarten oder sehr grossen f64-Endpunkten fällt `0.5·(lo+hi)` mit
    /// einem Endpunkt zusammen, und die „Unterteilung" liefert dann die
    /// Elternzelle zurück — eine Endlosrekursion, kein Fortschritt. Die
    /// Terminierung steht also nicht auf „jede Ebene halbiert", sondern auf
    /// dieser expliziten Weigerung plus der Maximaltiefe.
    pub fn split(&self) -> Option<(Box3, Box3)> {
        let mut k = 0;
        for j in 1..3 {
            if self.hi[j] - self.lo[j] > self.hi[k] - self.lo[k] {
                k = j;
            }
        }
        let m = 0.5 * (self.lo[k] + self.hi[k]);
        if !m.is_finite() || m <= self.lo[k] || m >= self.hi[k] {
            return None;
        }
        let mut a = *self;
        let mut b = *self;
        a.hi[k] = m;
        b.lo[k] = m;
        Some((a, b))
    }
}

impl CellDomain for Box3 {
    fn world_f32(&self) -> [Iv; 3] {
        // Domänen-Containment (Beweispflicht 2): das Feld wird am
        // f32-gerundeten Punkt ausgewertet; `[f32_down(lo), f32_up(hi)]`
        // enthält `fl32(x)` für JEDES reelle `x ∈ [lo,hi]`, weil `fl32`
        // monoton ist.
        [
            Iv { lo: f32_down(self.lo[0]), hi: f32_up(self.hi[0]) },
            Iv { lo: f32_down(self.lo[1]), hi: f32_up(self.hi[1]) },
            Iv { lo: f32_down(self.lo[2]), hi: f32_up(self.hi[2]) },
        ]
    }

    /// Der Mittelpunkt — **so, wie er tatsächlich herauskommt**, inklusive
    /// seiner f64-Rundung, und in die Zelle geklemmt.
    ///
    /// Die Klemme ist nicht Kosmetik (§8u-Korrektur): `0.5·(lo+hi)` kann bei
    /// grossen Beträgen runden oder überlaufen. `ρ` wird gegen GENAU diesen
    /// Punkt gerechnet, also muss er nachweislich in der Zelle liegen.
    fn center(&self) -> [f64; 3] {
        let mut c = [0.0f64; 3];
        for k in 0..3 {
            let m = 0.5 * (self.lo[k] + self.hi[k]);
            c[k] = if m.is_finite() { m.clamp(self.lo[k], self.hi[k]) } else { self.lo[k] };
        }
        c
    }

    /// Die Halbdiagonale **relativ zum tatsächlichen [`Self::center`]**.
    ///
    /// §8u-Korrektur (Befund des vierten Mathe-Reviews): vorher wurde `ρ` aus
    /// den Halbbreiten `(hi−lo)/2` gebildet, `u_at` aber am GERUNDETEN
    /// Mittelpunkt ausgewertet. Bei `lo = 10¹⁶`, `hi = lo + 2` fällt der
    /// gerundete Mittelpunkt mit `lo` zusammen, die Halbbreite ist `1`, der
    /// andere Endpunkt aber `2` entfernt — `|x − c| ≤ ρ` galt dann nicht.
    /// Jetzt ist `ρ_k = max(c_k − lo_k, hi_k − c_k)`, jede Differenz nach
    /// oben gerundet: für `c ∈ [lo,hi]` (garantiert durch die Klemme) folgt
    /// `|x_k − c_k| ≤ ρ_k` für jedes `x_k ∈ [lo_k, hi_k]`, und damit
    /// `|x − c| ≤ √(Σ ρ_k²) ≤ ρ`.
    ///
    /// Aufgerundet in JEDEM Schritt — `ρ` ist eine Zusage („weiter kann kein
    /// Zellpunkt vom Mittelpunkt weg sein"), also darf sie nie zu klein sein.
    fn rho(&self) -> f64 {
        let c = self.center();
        let mut s = 0.0f64;
        for k in 0..3 {
            let h = next_up(c[k] - self.lo[k]).max(next_up(self.hi[k] - c[k])).max(0.0);
            s = next_up(s + next_up(h * h));
        }
        next_up(s.sqrt())
    }
}

/// Eine f32-Konstante als Punkt-Tripel.
#[inline]
fn c3(v: [f32; 3]) -> [Iv; 3] {
    [Iv::f32p(v[0]), Iv::f32p(v[1]), Iv::f32p(v[2])]
}

/// `p − const` komponentenweise, in f32.
#[inline]
fn fsub3(c: &mut Ctx, a: [Iv; 3], b: [Iv; 3]) -> Result<[Iv; 3], Unknown> {
    Ok([c.fsub(a[0], b[0])?, c.fsub(a[1], b[1])?, c.fsub(a[2], b[2])?])
}

/// **Eine affine Ebene über der Zelle.** `⟨p, n̂⟩ − c` in der
/// Produktions-Reihenfolge von [`crate::offset_channel::end_plane_gate`]
/// (`p₀g₀ + p₁g₁ + p₂g₂ − g₃`), op-weise f32-eingeschlossen.
///
/// Die drei Koordinaten sind über einer Box unabhängig, die REELLE affine Form
/// nimmt Minimum und Maximum also je an einer Ecke an — es gibt hier keine
/// Dependency-Überschätzung.
///
/// **Was das NICHT heisst** (§8v-Korrektur nach dem fünften Review): „exakt"
/// wäre als Schärfe-Aussage zu stark. Jede Operation geht durch
/// [`Ctx::seal`], und die Aussen-Rundung weitet um bis zu ein f32-ulp je
/// Schritt. Die Zusage ist Einschliessung, nicht Schärfe.
pub fn affine_over_box(c: &mut Ctx, p: [Iv; 3], g: [f32; 4]) -> Enc {
    let m0 = c.fmul(p[0], Iv::f32p(g[0]))?;
    let m1 = c.fmul(p[1], Iv::f32p(g[1]))?;
    let m2 = c.fmul(p[2], Iv::f32p(g[2]))?;
    let s = c.fadd(m0, m1)?;
    let s = c.fadd(s, m2)?;
    c.fsub(s, Iv::f32p(g[3]))
}

// ════════════════════════════════════════════════════════════════════════════
// Ebene 3 — Einschliessung des `L`-Blocks
// ════════════════════════════════════════════════════════════════════════════

/// `seg_dist2` aus [`crate::offset_lb`], eingeschlossen.
///
/// `a`, `b` sind Stück-KONSTANTEN, `ab` und `ll` also exakte f32-Werte; nur
/// `ap = p − a` trägt die Zelle. Die Abhängigkeit zwischen `ap` und `t`
/// (beide hängen von `p` ab) macht `q = ap − ab·t` breiter als nötig — das
/// kostet Abdeckung, nicht Soundness.
fn enc_seg_dist2(c: &mut Ctx, p: [Iv; 3], a: [f32; 3], b: [f32; 3]) -> Enc {
    let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let ll = ab[0] * ab[0] + ab[1] * ab[1] + ab[2] * ab[2];
    let ap = fsub3(c, p, c3(a))?;
    let abi = c3(ab);
    let t = if ll > 1e-20 {
        let num = c.fdot(ap, abi)?;
        let q = c.fdiv(num, Iv::f32p(ll))?;
        // `clamp(·, 0, 1)` — exakt, keine Rundung.
        q.max(Iv::point(0.0)).min(Iv::point(1.0))
    } else {
        Iv::point(0.0)
    };
    let mut q = [Iv::point(0.0); 3];
    for k in 0..3 {
        let m = c.fmul(abi[k], t)?;
        q[k] = c.fsub(ap[k], m)?;
    }
    c.fdot(q, q)
}

/// `tri_dist` aus [`crate::offset_lb`], eingeschlossen.
///
/// Der Orientierungstest `inside ≥ 2` ist ein harter Schalter. Er wird über
/// der ZELLE entschieden: nur wenn alle drei Vorzeichen-Tests für jeden
/// Zellpunkt `+1` liefern, darf der Ebenen-Zweig allein stehen; genügt einer
/// sicher nicht, steht der Kanten-Zweig allein; sonst wird gehüllt. Die drei
/// Tests sind affine Formen über der Zelle (`cross(e, nor)` ist konstant),
/// also exakt entscheidbar, wo sie entscheidbar sind.
fn enc_tri_dist(c: &mut Ctx, p: [Iv; 3], a: [f32; 3], b: [f32; 3], t: [f32; 3]) -> Enc {
    let sub = |u: [f32; 3], v: [f32; 3]| [u[0] - v[0], u[1] - v[1], u[2] - v[2]];
    let cross = |u: [f32; 3], v: [f32; 3]| {
        [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]]
    };
    let ba = sub(b, a);
    let cb = sub(t, b);
    let ac = sub(a, t);
    let nor = cross(ba, ac);
    let nn = nor[0] * nor[0] + nor[1] * nor[1] + nor[2] * nor[2];

    let edge = |c: &mut Ctx| -> Enc {
        let d0 = enc_seg_dist2(c, p, a, b)?;
        let d1 = enc_seg_dist2(c, p, b, t)?;
        let d2 = enc_seg_dist2(c, p, t, a)?;
        c.fsqrt(d0.min(d1).min(d2))
    };

    if !(nn > 1e-24) {
        return edge(c);
    }

    // Die drei Vorzeichen-Tests `⟨cross(e,nor), w⟩ ≥ 0`.
    let tests = [(cross(ba, nor), a), (cross(cb, nor), b), (cross(ac, nor), t)];
    let mut all_true = true;
    let mut any_false = false;
    for (n, anchor) in tests {
        let w = fsub3(c, p, c3(anchor))?;
        let s = c.fdot(c3(n), w)?;
        match decide_ge(s, Iv::point(0.0)) {
            Decision::True => {}
            Decision::False => {
                any_false = true;
                all_true = false;
            }
            Decision::Both => all_true = false,
        }
    }

    let plane = |c: &mut Ctx| -> Enc {
        let w = fsub3(c, p, c3(a))?;
        let h = c.fdot(c3(nor), w)?;
        let hh = c.fmul(h, h)?;
        let q = c.fdiv(hh, Iv::f32p(nn))?;
        c.fsqrt(q)
    };

    let d = if all_true {
        Decision::True
    } else if any_false {
        Decision::False
    } else {
        Decision::Both
    };
    sw(c, d, plane, edge)
}

/// Ein Stück des `L`-Blocks. [`LbPiece::Rev`] ist in Paket 1 **nicht**
/// unterstützt: `rev_dist` projiziert auf die Achse (`rad = d − ax·⟨d,ax⟩`)
/// und erzeugt damit korrelierte Komponenten, deren natürliche Auswertung so
/// breit wird, dass sie nichts mehr entscheidet. Die saubere Auflösung — die
/// affine Zerlegung `rad = (I − ax axᵀ)·d` plus eine hergeleitete Schranke für
/// die f32-Rundung der PRODUKTIONS-Form — steht in „Was OFFEN bleibt".
fn enc_piece_dist(c: &mut Ctx, p: [Iv; 3], piece: &LbPiece) -> Enc {
    match *piece {
        LbPiece::Tri { a, b, c: t, .. } => enc_tri_dist(c, p, a, b, t),
        LbPiece::Rev { .. } => Err(Unknown("LbPiece::Rev")),
    }
}

/// **Die Einschliessung von `L` über der Zelle** (Beweispflicht 7).
///
/// [`crate::offset_lb::lb_value`] hat drei Bestandteile, und alle drei müssen
/// mit:
///
/// * **Innen-Indikator** `base > δ` ⇒ `LB_NONE`. `base` ist der naive
///   Stoppwert `d₀`, also selbst ein Intervall — überspannt die Zelle die
///   Schwelle, werden BEIDE Fälle gehüllt.
/// * **Fernfeld-Kugel** `|p − center| > skip_r` ⇒ `LB_NONE`. Ebenso.
/// * **Das `min` über die Randstücke**, danach `− slack`.
///
/// Der §8t-Prune wird NICHT modelliert: er ist bit-identisch zum flachen Lauf
/// (`lb_pruning_is_bit_exact_384` plus das Budget-Lemma in `LbPiece::tri`),
/// die Einschliessung des flachen Laufs deckt ihn also mit ab.
///
/// **Warum nicht der 1-Lipschitz-Weg?** `[L(c) − ρ, L(c) + ρ]` wäre die
/// elegante Form, aber sie schliesst den REELLEN Stück-Abstand ein, nicht den
/// f32-Wert, den `lb_value` liefert. Der Übergang bräuchte eine ZWEISEITIGE
/// f32-Fehlerschranke für `tri_dist`; §8t hat nur eine EINSEITIGE hergeleitet,
/// und die auch nur gegen die Trägerebenen-Distanz, nicht gegen
/// `dist(p, T)`. Die direkte Auswertung braucht keine solche Konstante — sie
/// ist der Grund, warum in diesem Modul keine steht.
pub fn enclose_lb_value(c: &mut Ctx, p: [Iv; 3], base: Iv, blk: &LbBlock) -> Enc {
    let none = Iv::f32p(LB_NONE);
    let outside = decide_gt(base, Iv::f32p(blk.delta));
    let dcen = {
        let d = fsub3(c, p, c3(blk.center))?;
        c.fnorm(d)?
    };
    let far = decide_gt(dcen, Iv::f32p(blk.skip_r));

    // `||` in Produktions-Reihenfolge: neutral, sobald EINE der beiden gilt.
    let neutral = match (outside, far) {
        (Decision::True, _) | (_, Decision::True) => Decision::True,
        (Decision::False, Decision::False) => Decision::False,
        _ => Decision::Both,
    };

    let value = |c: &mut Ctx| -> Enc {
        let mut best: Option<Iv> = None;
        for piece in &blk.pieces {
            let d = enc_piece_dist(c, p, piece)?;
            best = Some(match best {
                Some(b) => b.min(d),
                None => d,
            });
        }
        // **Der `is_finite`-Schalter** (§8u-Korrektur, dritter blockierender
        // Befund). Die Produktion startet mit `best = +∞` und aktualisiert nur
        // bei `d < best`; bleibt `best` nicht-endlich — leerer Block ODER
        // ausschliesslich nicht-endliche Stück-Distanzen (Überlauf der
        // Quadratsumme in f32) —, liefert sie `LB_NONE` und NICHT `∞ − slack`.
        //
        // Das ist kein bloss breiteres Intervall: `extra > LB_AVAIL`
        // entscheidet danach zwischen `L`- und `G`-Gate. Ein modelliertes `+∞`
        // wählt den `L`-Zweig, während die Produktion den `G`-Zweig nimmt — die
        // beiden Ergebnismengen enthalten einander dann NICHT.
        let Some(b) = best else {
            // Leerer Block: `best` bleibt `∞` ⇒ neutral, ohne Schalter.
            return Ok(none);
        };
        if c.knobs.skip_lb_finite_gate {
            return c.fsub(b, Iv::f32p(blk.slack));
        }
        let fin = if b.lo.is_finite() && b.hi.is_finite() {
            Decision::True
        } else if b.lo == f64::INFINITY {
            Decision::False
        } else {
            Decision::Both
        };
        sw(c, fin, |c| c.fsub(b, Iv::f32p(blk.slack)), |_| Ok(none))
    };

    sw(c, neutral, |_| Ok(none), value)
}

// ════════════════════════════════════════════════════════════════════════════
// Ebene 3 — Einschliessung von `vertex_apply`
// ════════════════════════════════════════════════════════════════════════════

/// [`crate::offset_channel::vertex_limit`], eingeschlossen.
pub fn enclose_vertex_limit(c: &mut Ctx, p: [Iv; 3], vs: &VertexSphere) -> Enc {
    let dq = fsub3(c, p, c3(vs.v))?;
    let rho = c.fnorm(dq)?;
    let neutral = Iv::f32p(-END_PLANE_OFF);
    let d = decide_ge(rho, Iv::f32p(vs.trust));
    let val = |c: &mut Ctx| c.fsub(Iv::f32p(vs.delta), rho);
    sw(c, d, |_| Ok(neutral), val)
}

/// [`crate::offset_channel::concave_gate`], eingeschlossen.
fn enclose_concave_gate(c: &mut Ctx, gate_w: f32, base: Iv, extra: Iv) -> Enc {
    let d = decide_gt(extra, Iv::f32p(LB_AVAIL));
    sw(c, d, |_| Ok(extra), |c| c.fsub(Iv::f32p(gate_w), base))
}

/// **Die vollständige Einschliessung von [`crate::offset_channel::vertex_apply`]**
/// (Beweispflicht 5 und 6).
///
/// Ausgewertet wird gegen die ECHTE Produktions-Reihenfolge, Operation für
/// Operation, inklusive:
///
/// * des Fernfeld-**Skips** (`trust − ρ ≤ ∓d`) — ein SKIP ist ein Zweig wie
///   jeder andere: er darf nur allein stehen, wenn er für JEDEN Punkt der
///   Zelle und den ganzen laufenden `d`-Bereich gilt (Beweispflicht 6);
/// * der `L`/`G`-Umschaltung in `concave_gate`;
/// * der Rollen-Trennung `w_geo` / `w` und des DREI-Wege-Schalters am Ende.
///
/// `base` ist `d₀`, `d` der laufende Wert, `extra` das `L − δ` des Stopps —
/// alle drei als Intervalle, weil sie über der Zelle variieren.
pub fn enclose_vertex_apply(
    c: &mut Ctx,
    p: [Iv; 3],
    base: Iv,
    d: Iv,
    vs: &VertexSphere,
    extra: Iv,
) -> Enc {
    let dq = fsub3(c, p, c3(vs.v))?;
    let rho = c.fnorm(dq)?;
    let lhs = c.fsub(Iv::f32p(vs.trust), rho)?;
    let rhs = if vs.concave { d.neg() } else { d };
    let skip = decide_le(lhs, rhs);

    let body = |c: &mut Ctx| -> Enc {
        let s1 = c.fdot(dq, c3(vs.gates[0]))?;
        let s2 = c.fdot(dq, c3(vs.gates[1]))?;
        let s3 = c.fdot(dq, c3(vs.gates[2]))?;
        let gate = if vs.concave {
            enclose_concave_gate(c, vs.gate, base, extra)?
        } else {
            c.fadd(Iv::f32p(vs.gate), base)?
        };
        let rd = c.fsub(rho, Iv::f32p(vs.delta))?;
        let tr = c.fsub(Iv::f32p(vs.trust), rho)?;
        let w_geo = s1.min(s2).min(s3).min(rd).min(tr);
        let w = w_geo.min(gate);
        if !vs.concave {
            return Ok(d.max(w));
        }
        // Drei-Wege-Schalter — geteilt mit `channel_apply` (§8v), damit beide
        // Terme nachweislich dieselbe Fallunterscheidung modellieren.
        three_way_concave(c, d, w, w_geo)
    };

    sw(c, skip, |_| Ok(d), body)
}

// ════════════════════════════════════════════════════════════════════════════
// Ebene 3 — der Stopp und der Baum darunter
// ════════════════════════════════════════════════════════════════════════════

/// [`crate::offset_channel::stop_apply`], eingeschlossen — für Stopps, deren
/// Terme vollständig unterstützt sind.
fn enclose_stop(
    c: &mut Ctx,
    p: [Iv; 3],
    d0: Iv,
    channels: &[OffsetChannel],
    verts: &[VertexSphere],
    lb: Option<&LbBlock>,
) -> Enc {
    let mut d = d0;
    let mut lim = Iv::f32p(-END_PLANE_OFF);
    // **Zwei Durchgänge** (§8v). Erst der billige Ausschluss: sind ALLE Kanäle
    // schon gegen `d₀` neutral, bleibt `d` über die ganze Kanal-Schleife `d₀`,
    // alle Grenzwert-Beiträge sind `−1e30`, und `L` wird nur gebraucht, wenn
    // überhaupt eine Vertex-Kugel ihr Gate liest. Erst wenn das nicht aufgeht,
    // wird sequenziell voll ausgewertet — dann kostet `L` seinen Preis.
    let all_neutral = channels.iter().all(|ch| channel_is_neutral(c, p, d0, ch));
    let need_lb = !verts.is_empty() || !all_neutral;
    // #384 §8z — **ein unbekanntes `L` darf den Stopp nicht verwerfen.**
    //
    // Bis A1 propagierte ein `Unknown` aus `enclose_lb_value` (etwa wegen
    // `LbPiece::Rev`) und machte den ganzen Stopp unbekannt. Das ist strenger
    // als nötig: `L` erreicht das Ergebnis ausschliesslich über
    // `extra → concave_gate → gate` und dort über `w = min(w_geo, gate)`. Im
    // konkaven Zweig liefert JEDER der drei Ausgänge `min(d, ·)` oder `d`,
    // also stets `≤ d` — die OBERE Schranke hängt damit gar nicht von `gate`
    // ab. Ein `Entire` für `lbv` ist deshalb eine gültige (maximal
    // konservative) Einschliessung: sie kostet nur die untere Schranke.
    //
    // Fail-closed bleibt gewahrt — `Entire` behauptet nichts.
    let lbv = match lb {
        Some(b) if need_lb => {
            let r = enclose_lb_value(c, p, d0, b);
            if c.knobs.lb_unknown_rejects {
                r?
            } else {
                r.unwrap_or(Iv::ENTIRE)
            }
        }
        _ => Iv::f32p(LB_NONE),
    };
    if !all_neutral {
        for ch in channels {
            lim = lim.max(enclose_channel_limit(c, p, ch)?);
            let extra = c.fsub(lbv, Iv::f32p(ch.delta))?;
            d = enclose_channel_apply(c, p, d0, d, ch, extra)?;
        }
    }
    for vs in verts {
        lim = lim.max(enclose_vertex_limit(c, p, vs)?);
        let extra = c.fsub(lbv, Iv::f32p(vs.delta))?;
        d = enclose_vertex_apply(c, p, d0, d, vs, extra)?;
    }
    let concave = channels.first().map(|x| x.concave).or_else(|| verts.first().map(|v| v.concave));
    Ok(match concave {
        Some(true) => d.max(lim),
        Some(false) => d.min(lim.neg()),
        None => d,
    })
}

// ════════════════════════════════════════════════════════════════════════════
// §8v — Kanal-Terme: gerade Segmente mit eindeutiger Nearest-Zuordnung
// ════════════════════════════════════════════════════════════════════════════

/// Die vier Halbraum-Slots von [`crate::offset_channel::end_plane_gate`] bzw.
/// [`crate::offset_channel::face_plane_gate`], eingeschlossen.
///
/// Produktion: `s = +∞; for g { s = s.min(p₀g₀ + p₁g₁ + p₂g₂ − g₃) }`. Jedes
/// Glied ist eine affine Form über der Zelle, wird also ohne
/// Dependency-Überschätzung eingeschlossen (s. [`affine_over_box`]); das `min`
/// ist eine Auswahl ohne Rundung.
///
/// **Das `min` selbst ist trotzdem eine Überschätzung** (§8v-Korrektur nach
/// dem fünften Review): die vier Ebenen teilen sich `p`, und das
/// Intervall-`min` verliert diese Kopplung. Schulbeispiel: `h₁ = x`,
/// `h₂ = −x` über `x ∈ [−1,1]` hat als reelles Minimum `[−1, 0]`, das
/// Intervall-`min` liefert `[−1, 1]`. Sound, aber nicht scharf — „exakt" wäre
/// hier die falsche Vokabel.
fn enc_gate_planes(c: &mut Ctx, p: [Iv; 3], planes: &[[f32; 4]; 4]) -> Enc {
    let mut s = Iv::point(f64::INFINITY);
    for g in planes {
        s = s.min(affine_over_box(c, p, *g)?);
    }
    Ok(s)
}

/// Die Einschliessung von [`crate::offset_channel::arc_closest`] für ein
/// **Geradensegment** (`sweep < 0`): `(de, q, t)`.
///
/// Bemerkenswert: die Produktion hat hier **keine Verzweigung**. Der Wechsel
/// zwischen Lotfusspunkt und Endpunkt steckt allein im `clamp(0,1)` von `t`,
/// und `clamp` ist `max` gefolgt von `min` — beides exakte Auswahl, keine
/// Rundung. Der „Endpunkt- gegen Projektions-Zweig", den man erwarten würde,
/// ist also gar kein Schalter, sondern eine Sättigung, und die Intervall-Form
/// bildet sie ohne Hülle exakt ab. (Die Pflicht-Familie dazu prüft genau das:
/// Zellen teils innerhalb, teils ausserhalb des Segment-Slabs.)
fn enc_segment_closest(
    c: &mut Ctx,
    p: [Iv; 3],
    arc: &ChannelArc,
) -> Result<(Iv, [Iv; 3], Iv), Unknown> {
    let len = arc.r.max(1e-12);
    let pc = fsub3(c, p, c3(arc.c))?;
    let num = c.fdot(pc, c3(arc.u))?;
    let t = c.fdiv(num, Iv::f32p(len))?;
    let t = t.max(Iv::point(0.0)).min(Iv::point(1.0));
    let tl = c.fmul(t, Iv::f32p(len))?;
    let mut q = [Iv::point(0.0); 3];
    for k in 0..3 {
        let m = c.fmul(Iv::f32p(arc.u[k]), tl)?;
        q[k] = c.fadd(Iv::f32p(arc.c[k]), m)?;
    }
    let dq = fsub3(c, p, q)?;
    let de = c.fnorm(dq)?;
    Ok((de, q, t))
}

/// **Das Normalisierungslemma** — [`nlerp`], eingeschlossen.
///
/// Produktion: `v = a + (b−a)·t`, `n = |v|`, und dann `v/n` falls `n > 1e-9`,
/// sonst `a`. Drei Dinge sind zu beweisen:
///
/// * **Nie durch ein nullhaltiges Intervall dividieren.** Im
///   Normalisierungs-Zweig GILT `n > 1e-9` — diese Bedingung wird deshalb in
///   den Zweig hinein GESCHNITTEN (`n ∩ [1e-9, ∞)`), bevor dividiert wird.
///   Ohne den Schnitt wäre die untere Schranke von `n` bei einem unentschiedenen
///   Schalter null, und die Division müsste fail-closed abbrechen — der Zweig
///   ginge verloren, obwohl er wohldefiniert ist.
/// * **Der Rückfall ist ein Zweig, kein Sonderfall.** Überspannt `n` die
///   Schwelle `1e-9`, wird die Hülle aus normalisiertem Zweig UND `a` gebildet.
/// * **Fast antipodale Endvektoren** sind genau der Fall, in dem `v` durch
///   Auslöschung klein wird und `n` die Schwelle überspannt. Die Pflicht-Familie
///   fährt ihn.
fn enc_nlerp(
    c: &mut Ctx,
    a: [f32; 3],
    b: [f32; 3],
    t: Iv,
) -> Result<[Iv; 3], Unknown> {
    let mut v = [Iv::point(0.0); 3];
    for k in 0..3 {
        let ab = Iv::f32p(b[k] - a[k]);
        let m = c.fmul(ab, t)?;
        v[k] = c.fadd(Iv::f32p(a[k]), m)?;
    }
    let n = c.fnorm(v)?;
    let thr = Iv::f32p(1.0e-9);
    let dec = decide_gt(n, thr);
    let norm_branch = |c: &mut Ctx| -> Result<[Iv; 3], Unknown> {
        // Der Schnitt: in diesem Zweig ist `n > 1e-9` bewiesen.
        // `thr.lo` statt eines eigenen Literals: die Begründung des Schnitts
        // hängt an der PRODUKTIONS-Konstante, nicht an einer Kopie davon.
        let nn = n.intersect(Iv { lo: thr.lo, hi: f64::INFINITY }).ok_or(Unknown("nlerp n"))?;
        Ok([c.fdiv(v[0], nn)?, c.fdiv(v[1], nn)?, c.fdiv(v[2], nn)?])
    };
    match dec {
        Decision::True => norm_branch(c),
        Decision::False => Ok(c3(a)),
        Decision::Both => {
            if c.knobs.first_branch_only {
                return norm_branch(c);
            }
            let x = norm_branch(c)?;
            let y = c3(a);
            Ok([x[0].hull(y[0]), x[1].hull(y[1]), x[2].hull(y[2])])
        }
    }
}

/// Das Ergebnis von [`crate::offset_channel::channel_nearest`], sofern über der
/// ganzen Zelle EIN Bogen beweisbar gewinnt.
/// #384 §8z (P4-A1) — **eine erreichbare Produktions-Alternative EINES Bogens.**
///
/// `arc_closest` hat für einen gekrümmten Bogen drei erreichbare Ausgänge:
/// Achse (`ρ < 1e-9`), Fusspunkt (`ρ ≥ 1e-9` ∧ Sektor innen) und Endpunkt
/// (`ρ ≥ 1e-9` ∧ Sektor aussen, dort Start oder Ende). Über einer Zelle können
/// mehrere davon erreichbar sein.
///
/// **`de`, `q` und `t` werden GEMEINSAM geführt** (Beweispflicht 7). Sie
/// getrennt zu hüllen wäre zwar sound, aber unnötig breit — es kombinierte den
/// `q` des Endpunkt-Astes mit dem `t` des Fusspunkt-Astes, ein Paar, das die
/// Produktion nie erzeugt.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ArcAlt {
    pub de: Iv,
    pub q: [Iv; 3],
    pub t: Iv,
}

/// #384 §8z — **ein Bogen als GENAU EIN §8w-Kandidat.**
///
/// Die Produktionsschleife hat je Bogen genau einen Index und genau einen
/// Abstand in der strikten `< best`-Reihenfolge. Die internen Äste als eigene
/// §8w-Kandidaten zu führen würde deshalb die Tie-Reihenfolge, den
/// Prune-Zustand und den „letzten sicheren Aktualisierer" verfälschen —
/// deshalb: ein Bogen, ein Kandidat, mit `de_hull` für Argmin und
/// `channel_limit`, und den Alternativen für den Downstream.
#[derive(Debug, Clone)]
struct ArcCand {
    /// Der Index des Bogens im Kanal — der Downstream braucht GENAU dessen
    /// `m`-Vektoren für `enc_nlerp`.
    idx: usize,
    alts: Vec<ArcAlt>,
    de_hull: Iv,
}

/// #384 §8z (P4-A1) — **`arc_closest` für einen GEKRÜMMTEN Bogen,
/// eingeschlossen.**
///
/// Bis §8y verweigerte die Intervall-Stufe Krümmung pauschal („Paket 4"). A0
/// hat die Auswertung algebraisch gemacht — hier wird sie eingeschlossen,
/// Operation für Operation in der PRODUKTIONS-Klammerung (Beweispflicht 1).
///
/// ## Die drei erreichbaren Ausgänge
///
/// | Ausgang | Bedingung | Rückgabe |
/// |---|---|---|
/// | Achse | `ρ < 1e-9` | `(\|p−q₀\|, q₀, 0)` |
/// | Fusspunkt | `ρ ≥ 1e-9` ∧ `a ≥ 0` ∧ `b ≥ 0` | Projektion, `t = D/D_e` |
/// | Endpunkt | `ρ ≥ 1e-9` ∧ Sektor aussen | näherer Endpunkt, Tie → START |
///
/// Jeder über der Zelle ERREICHBARE Ausgang wird als eigene [`ArcAlt`]
/// geführt; die Hülle über sie ist sound (Beweispflicht 7).
///
/// ## Die tragende Stelle: `ρ` vor der Division schneiden
///
/// Ist der ρ-Schalter gemischt, rechnet der Nicht-Achsen-Ast mit
/// `ρ ∩ [1e-9, ∞)`, **bevor** `x/ρ` gebildet wird. Der Schnitt ist zulässig,
/// weil die Produktion auf diesem Ast `ρ ≥ 1e-9` bereits geprüft hat.
///
/// **Er ist eine SCHÄRFE-Massnahme, keine Soundness-Pflicht**
/// (Richtigstellung nach dem zwölften Review; die erste A1-Fassung behauptete
/// „Division durchs rohe Intervall wäre unzulässig"). Ohne den Schnitt
/// verweigert [`Ctx::fdiv`] ein nullhaltiges Roh-Intervall ohnehin
/// fail-closed, und die Division durch ein nullfreies Roh-Intervall ist sound
/// — nur breiter und schlechter konditioniert. Richtig und gewollt bleibt der
/// Schnitt trotzdem: er hebt `1/ρ` aus der Nähe der Schwelle heraus.
///
/// Die Kondition `1/ρ` bleibt bei nullfreiem Intervall ein reines
/// BREITEN-Problem; dagegen hilft Unterteilung oder `Unknown` — **nie ein
/// Fehlerbudget**.
///
/// ## Die Diamant-Nenner
///
/// `x + y` bzw. `y − x` müssen per [`Ctx::fdiv`] als **nullfrei bewiesen**
/// werden. Aus der punktweisen Beziehung „`ρ > 0` ⇒ `x + y > 0` im ersten
/// Quadranten" darf KEINE Box-Untergrenze werden: über einer Zelle ist die
/// Korrelation zwischen `x` und `y` verloren. Nicht beweisbar nullfrei ⇒
/// `Unknown` und Unterteilung.
///
/// **Keine Dichotomie zum ρ-Schalter.** Die erste A1-Fassung behauptete,
/// nullfreie Diamant-Nenner und ein gemischter ρ-Schalter schlössen einander
/// aus. Das ist im f32-Intervallmodell **kein Satz**: bei sehr engem `x` um
/// null und `y` wenige ulp oberhalb der Schwelle sind `x + y` und `y − x`
/// klar positiv, während die auswärts versiegelte Kette
/// `sqrt(x² + y²)` mit ihrer UNTEREN Schranke unter die Schwelle rutschen
/// kann. Beides tritt dann gleichzeitig auf.
pub(crate) fn enc_arc_closest(
    c: &mut Ctx,
    p: [Iv; 3],
    arc: &ChannelArc,
) -> Result<Vec<ArcAlt>, Unknown> {
    // Fail-closed: ungültige A0-Daten werden nie eingeschlossen.
    if c.knobs.arc_refuse_curved {
        return Err(Unknown("Kanal-Bogen mit Krümmung (Ablation)"));
    }
    if !crate::offset_channel::arc_data_valid(arc) {
        return Err(Unknown("Bogen-Daten ungültig"));
    }
    c.curved_arc_evals += 1;
    let zero = Iv::point(0.0);
    let d = fsub3(c, p, c3(arc.c))?;
    let x = c.fdot(d, c3(arc.u))?;
    let y = c.fdot(d, c3(arc.v))?;
    // `ρ = sqrt(x*x + y*y)` — ZWEI Summanden, in Produktions-Klammerung.
    // `fnorm` wäre ein Drei-Term-Skalarprodukt und damit eine andere Op-Kette.
    let xx = c.fsquare(x)?;
    let yy = c.fsquare(y)?;
    let rho2 = c.fadd(xx, yy)?;
    let rho = c.fsqrt(rho2)?;
    let (e0, e1) = (Iv::f32p(arc.e[0]), Iv::f32p(arc.e[1]));
    let r = Iv::f32p(arc.r);
    // Start- und Endpunkt.
    let mut q0 = [zero; 3];
    let mut q1 = [zero; 3];
    for k in 0..3 {
        let m = c.fmul(r, Iv::f32p(arc.u[k]))?;
        q0[k] = c.fadd(Iv::f32p(arc.c[k]), m)?;
        let a = c.fmul(Iv::f32p(arc.u[k]), e0)?;
        let b = c.fmul(Iv::f32p(arc.v[k]), e1)?;
        let sum = c.fadd(a, b)?;
        let rm = c.fmul(r, sum)?;
        q1[k] = c.fadd(Iv::f32p(arc.c[k]), rm)?;
    }
    let d0 = {
        let v = fsub3(c, p, q0)?;
        c.fnorm(v)?
    };
    let mut alts: Vec<ArcAlt> = Vec::new();
    let thr = Iv::f32p(1.0e-9);
    let on_axis = decide_lt(rho, thr);
    if on_axis != Decision::False {
        alts.push(ArcAlt { de: d0, q: q0, t: zero });
    }
    if on_axis != Decision::True {
        // **Der Schnitt vor jeder Division.**
        let rho_nz = if c.knobs.arc_skip_rho_intersect {
            rho
        } else {
            rho.intersect(Iv { lo: thr.lo, hi: f64::INFINITY })
                .ok_or(Unknown("ρ-Schnitt leer"))?
        };
        // Sektor: `a = y ≥ 0` UND `b = x·e₁ − y·e₀ ≥ 0`.
        let bv = {
            let m0 = c.fmul(x, e1)?;
            let m1 = c.fmul(y, e0)?;
            c.fsub(m0, m1)?
        };
        let a_ge = decide_ge(y, zero);
        let b_ge = decide_ge(bv, zero);
        let inside = match (a_ge, b_ge) {
            (Decision::True, Decision::True) => Decision::True,
            (Decision::False, _) | (_, Decision::False) => Decision::False,
            _ => Decision::Both,
        };
        if inside != Decision::False && !c.knobs.arc_drop_foot_alt {
            let cs = c.fdiv(x, rho_nz)?;
            let sn = c.fdiv(y, rho_nz)?;
            let mut q = [zero; 3];
            for k in 0..3 {
                let a = c.fmul(Iv::f32p(arc.u[k]), cs)?;
                let b = c.fmul(Iv::f32p(arc.v[k]), sn)?;
                let sum = c.fadd(a, b)?;
                let rm = c.fmul(r, sum)?;
                q[k] = c.fadd(Iv::f32p(arc.c[k]), rm)?;
            }
            let de = {
                let v = fsub3(c, p, q)?;
                c.fnorm(v)?
            };
            // Diamant: `x ≥ 0 ? y/(x+y) : 1 + (−x)/(y−x)`.
            let xq = decide_ge(x, zero);
            let dm_q1 = |c: &mut Ctx| -> Enc {
                let den = c.fadd(x, y)?;
                c.fdiv(y, den)
            };
            let dm_q2 = |c: &mut Ctx| -> Enc {
                let den = c.fsub(y, x)?;
                let num = c.fdiv(x.neg(), den)?;
                c.fadd(Iv::point(1.0), num)
            };
            let dm = sw(c, xq, dm_q1, dm_q2)?;
            let t = c.fdiv(dm, Iv::f32p(arc.d_e))?;
            let t = t.max(zero).min(Iv::point(1.0));
            alts.push(ArcAlt { de, q, t });
        }
        if inside != Decision::True {
            let d1 = {
                let v = fsub3(c, p, q1)?;
                c.fnorm(v)?
            };
            // `d0 <= d1` — **Gleichheit gehört zum START**.
            let pick_start = decide_le(d0, d1);
            if pick_start != Decision::False {
                alts.push(ArcAlt { de: d0, q: q0, t: zero });
            }
            if pick_start != Decision::True && !c.knobs.arc_drop_end_alt {
                alts.push(ArcAlt { de: d1, q: q1, t: Iv::point(1.0) });
            }
        }
    }
    if alts.is_empty() {
        return Err(Unknown("leere Bogen-Alternativen"));
    }
    if c.knobs.arc_first_alt_only {
        alts.truncate(1);
    }
    if c.knobs.arc_drop_last_alt && alts.len() > 1 {
        alts.pop();
    }
    Ok(alts)
}

/// **Das Selektions-Lemma** (§8w) — welche Bögen kann die Produktion an einem
/// Punkt der Zelle zurückgeben?
///
/// `channel_nearest` läuft die Bögen in Array-Ordnung ab, hält ein laufendes
/// `best` (Start `+∞`) und aktualisiert `best`, `q*`, `m̂₁`, `m̂₂` **genau
/// dann**, wenn `de_k < best` — ein STRIKTER Vergleich. Daraus folgt:
///
/// * Wird ÜBERHAUPT ein Bogen ausgewertet, wählt die Produktion an jedem Punkt
///   **genau EINEN** — den zuletzt aktualisierenden. Der Rückgabewert ist
///   dessen vollständiger Downstream.
/// * **Bei Gleichstand gewinnt der FRÜHERE Index.** `de_k < best` ist bei
///   `de_k == best` falsch, der spätere Bogen aktualisiert also nicht. Die
///   Kandidaten-Bestimmung muss das einschliessen — im Zweifel wird ein Bogen
///   AUFGENOMMEN (mehr Kandidaten = breiterer Hull = sound).
/// * **Es kann aber auch GAR KEINER ausgewertet werden**, und dann liefert
///   `channel_nearest` `None`. Die erste §8w-Fassung behauptete hier „Bogen 0
///   wird nie übersprungen, der Prune vergleicht gegen `best = +∞`" — das ist
///   unter f32-**Überlauf falsch** (Befund des sechsten Reviews): `dbs₀ ≥ best`
///   ist für `dbs₀ = +∞` und `best = +∞` nach IEEE **wahr**. Eine geometrisch
///   gültige, nur sehr lose Bogen-Kugel genügt: bei `bc = (2·10¹⁹, 2·10¹⁹, 0)`
///   überläuft schon die Quadratsumme, `norm` wird `+∞`, und der EINZIGE Bogen
///   wird übersprungen — obwohl `de₀` endlich und klein ist. `de` und `dbs`
///   stammen aus verschiedenen Daten; ein endliches `de` schützt nicht.
///
/// Der `None`-Ausgang ist deshalb ein eigener Kandidat und wird MITGEHÜLLT
/// (`channel_apply` gibt dann `d` zurück, `channel_limit` das neutrale
/// `−1e30`). Ausgeschlossen ist er, sobald **irgendein** Bogen ein sicher
/// endliches `dbs` hat: der erste ausgewertete Bogen trifft dann auf
/// `best = +∞` und aktualisiert wegen `de < +∞` garantiert.
///
/// Also liegt der Produktionswert in der VEREINIGUNG der Kandidaten-Ergebnisse,
/// und die Hülle darüber ist eine gültige Einschliessung (§8u-Beweispflicht 4,
/// nur über einer indizierten statt einer binären Fallunterscheidung).
///
/// ## Das Kandidaten-Kriterium, hergeleitet
///
/// `best_before_k` ist das Minimum der `de_j` über die AUSGEWERTETEN `j < k`.
/// Zwei Schranken, beide ohne jede geometrische Zusatzannahme:
///
/// ```text
/// lower(best_before_k) ≥ min_{j<k} lower(de_j)      (Minimum einer Teilmenge)
/// upper(best_before_k) ≤ min_{j<k, j bewiesen ausgewertet} upper(de_j)
/// ```
///
/// Damit:
///
/// * **Bogen `k` ist bewiesen AUSGEWERTET**, wenn `upper(dbs_k) < lower(best_before_k)`
///   (der Prune greift dann sicher nicht). Für `k = 0` ist
///   `lower(best_before_0) = +∞`, der Test lautet dort also
///   `upper(dbs₀) < +∞` — und eben NICHT „immer wahr".
/// * **Bogen `k` kann aktualisieren**, wenn `lower(de_k) < upper(best_before_k)`.
///   Ist das falsch, kann er `best` nie unterbieten und ist kein Kandidat.
/// * **Bogen `k` aktualisiert SICHER**, wenn er bewiesen ausgewertet ist und
///   `upper(de_k) < lower(best_before_k)`.
///
/// Der letzte sichere Aktualisierer `f` schneidet alle früheren ab: wer vor `f`
/// liegt, kann nicht der LETZTE sein. Kandidaten sind also
/// `{ k ≥ f : k kann aktualisieren }`.
///
/// **Ohne Containment-Annahme.** Die naheliegende Abkürzung wäre „der Bogen
/// liegt in seiner Bounding-Sphere, also ist `de_j ≥ dbs_j`, also prunet der
/// Prune nur Verlierer". Diese Invariante wird hier NICHT vorausgesetzt — die
/// Herleitung oben benutzt ausschliesslich, dass `best` ein Minimum über eine
/// TEILMENGE ist, und das gilt unabhängig davon, was der Prune tut. (§8v
/// brauchte dafür noch eine eigene Prune-Prüfung; sie entfällt, weil die
/// Kandidaten-Menge den Fall jetzt mitträgt.)
///
/// **Bögen mit Krümmung sind `Unknown`.** `arc_closest` braucht dort `atan2`,
/// `sin` und `cos`; das trigonometrische Enclosure ist Paket 4. Enthält ein
/// Kanal auch nur einen gekrümmten Bogen, verweigert diese Funktion — auch für
/// die Konkurrenz-Schranken, damit kein halber Beweis entsteht.
struct Candidates {
    arcs: Vec<ArcCand>,
    /// Kann die Produktion über dieser Zelle `None` liefern (alle Bögen
    /// übersprungen)? Dann gehört der neutrale Ausgang in die Hülle.
    none_possible: bool,
}

fn enc_channel_candidates(
    c: &mut Ctx,
    p: [Iv; 3],
    ch: &OffsetChannel,
) -> Result<Candidates, Unknown> {
    c.channel_evals += 1;
    if ch.arcs.is_empty() {
        return Err(Unknown("Kanal ohne Bogen"));
    }
    let n = ch.arcs.len();
    let mut geo: Vec<(Iv, Vec<ArcAlt>)> = Vec::with_capacity(n);
    for arc in &ch.arcs {
        // #384 §8z: Segmente behalten ihren Ein-Alternativen-Pfad; gekrümmte
        // Bögen liefern seit A1 ihre erreichbaren Alternativen.
        let alts: Vec<ArcAlt> = if arc.sweep < 0.0 {
            let (de, q, t) = enc_segment_closest(c, p, arc)?;
            vec![ArcAlt { de, q, t }]
        } else {
            enc_arc_closest(c, p, arc)?
        };
        let mut hull = alts[0].de;
        for a in &alts[1..] {
            hull = hull.hull(a.de);
        }
        let g = (hull, alts);
        // Ein nicht-endliches `de` hiesse Überlauf in der Quadratsumme; dann
        // ist die Argmin-Logik nicht mehr modellierbar.
        if !g.0.lo.is_finite() || !g.0.hi.is_finite() {
            return Err(Unknown("Bogen-Distanz nicht endlich"));
        }
        geo.push(g);
    }
    // Prune-Grösse je Bogen (nur für den „bewiesen ausgewertet"-Test).
    let mut dbs = Vec::with_capacity(n);
    for arc in &ch.arcs {
        let v = fsub3(c, p, c3(arc.bc))?;
        let nb = c.fnorm(v)?;
        dbs.push(c.fsub(nb, Iv::f32p(arc.br))?);
    }
    let mut proven_eval = vec![false; n];
    let mut can_update = vec![false; n];
    let mut last_forced = 0usize;
    for k in 0..n {
        // `best_before_k`.
        let lo_before =
            (0..k).map(|j| geo[j].0.lo).fold(f64::INFINITY, f64::min);
        let hi_before = (0..k)
            .filter(|&j| proven_eval[j])
            .map(|j| geo[j].0.hi)
            .fold(f64::INFINITY, f64::min);
        // Für `k = 0` ist `lo_before = +∞`; der Test lautet dort `dbs₀ < +∞`
        // und NICHT „immer wahr" — `dbs₀ ≥ best` ist bei beidseitigem `+∞`
        // nach IEEE erfüllt, die Produktion überspringt dann auch den ersten
        // Bogen (Blocker des sechsten Reviews).
        proven_eval[k] = dbs[k].hi < lo_before;
        can_update[k] = geo[k].0.lo < hi_before;
        if proven_eval[k] && geo[k].0.hi < lo_before {
            last_forced = k;
        }
    }
    // `None` ist ausgeschlossen, sobald IRGENDEIN Bogen ein sicher endliches
    // `dbs` hat: der erste ausgewertete Bogen trifft dann auf `best = +∞` und
    // aktualisiert wegen `de < +∞` garantiert. Sind dagegen alle `dbs`
    // möglicherweise unendlich, kann die Schleife durchlaufen, ohne je zu
    // aktualisieren — dann liefert `channel_nearest` `None`.
    let none_possible =
        !dbs.iter().any(|d| d.hi < f64::INFINITY) && !c.knobs.assume_first_arc_evaluated;
    let mut out = Vec::new();
    for k in last_forced..n {
        if !can_update[k] {
            continue;
        }
        out.push(ArcCand { idx: k, alts: geo[k].1.clone(), de_hull: geo[k].0 });
        if c.knobs.first_arc_wins {
            break;
        }
    }
    if c.knobs.drop_last_candidate && out.len() > 1 {
        // Gegenprobe: GENAU EINEN Kandidaten streichen — den letzten. (Die
        // erste Fassung brach nach dem ersten aufgenommenen ab und strich bei
        // drei oder mehr Kandidaten alle bis auf einen; das ist die stumpfere
        // Mutation und trifft nicht mehr nur den Vereinigungs-Schritt.)
        out.pop();
    }
    if out.is_empty() && !none_possible {
        return Err(Unknown("leere Kandidaten-Menge"));
    }
    c.max_candidates = c.max_candidates.max(out.len());
    Ok(Candidates { arcs: out, none_possible })
}

/// [`crate::offset_channel::channel_limit`], eingeschlossen.
pub fn enclose_channel_limit(c: &mut Ctx, p: [Iv; 3], ch: &OffsetChannel) -> Enc {
    // #384 §8ac / P5b-1A — **fail-closed vor der neuen Feldform.**
    //
    // Diese Einschliessung modelliert `fl(δ − min(de, h))`. Trägt der Kanal
    // einen BEWIESENEN Laufzeitmodus, rechnet die Produktion aber
    // `fl(δ − min(fl(de+A), fl(h+A_h)))` — eine andere Kette mit zwei
    // zusätzlichen Rundungen. Die alte zweiseitige Zusage gilt dafür nicht
    // mehr, und sie hier stillschweigend weiterzuführen wäre genau der
    // verbotene Doppelschritt: neue Feldform ausliefern UND behaupten, sie sei
    // eingeschlossen.
    //
    // Also `Unknown`, bis **P5b-2** `de_safe`/`h_safe`/`min` op-genau
    // modelliert. `enclose_channel_apply` bleibt unberührt — der
    // `channel_apply`-Term hat sich nicht bewegt.
    // #384 §8ac (19. Review): ohne `Ω` lässt sich der Modus nicht auflösen —
    // hier zählt aber ohnehin nur, OB der Kanal überhaupt einen bewiesenen
    // Modus tragen soll. Der gespeicherte Modus ist dafür die konservative
    // Antwort: er verweigert auch dann, wenn der Beweis zur Laufzeit gar nicht
    // greifen würde. Verweigern ist hier immer sicher.
    if ch.stored_mode_is_proved() {
        return Err(Unknown("P5b-1A: bewiesener Laufzeitmodus, Einschliessung erst in P5b-2"));
    }
    let neutral = Iv::f32p(-END_PLANE_OFF);
    let bc = fsub3(c, p, c3(ch.bound_c))?;
    let nb = c.fnorm(bc)?;
    let dbs = c.fsub(nb, Iv::f32p(ch.bound_r))?;
    if decide_ge(dbs, Iv::f32p(ch.trust)) == Decision::True {
        return Ok(neutral);
    }
    // §8w: über ALLE möglichen Gewinner hüllen.
    let cands = enc_channel_candidates(c, p, ch)?;
    let s4 = enc_gate_planes(c, p, &ch.face_planes)?;
    // Der `None`-Ausgang von `channel_nearest`: `channel_limit` gibt dann das
    // neutrale `−1e30` zurück.
    let mut inner: Option<Iv> = if cands.none_possible { Some(neutral) } else { None };
    for near in &cands.arcs {
        // **Limit-Lemma**: `channel_limit` liest ausschliesslich `de`; die
        // `de`-Projektion der Alternativen genügt.
        let far = decide_ge(near.de_hull, Iv::f32p(ch.trust));
        let de = near.de_hull;
        let val = |c: &mut Ctx| -> Enc {
            let h = c.fadd(s4, Iv::f32p(ch.delta))?;
            c.fsub(Iv::f32p(ch.delta), de.min(h))
        };
        let v = sw(c, far, |_| Ok(neutral), val)?;
        inner = Some(match inner {
            Some(o) => o.hull(v),
            None => v,
        });
    }
    let inner = inner.ok_or(Unknown("leere Kandidaten-Menge"))?;
    match decide_ge(dbs, Iv::f32p(ch.trust)) {
        Decision::False => Ok(inner),
        _ => Ok(inner.hull(neutral)),
    }
}

/// **Ist dieser Kanal über der GANZEN Zelle neutral?** (§8u; der billige
/// Ausschluss, der ohne jede Naht-Geometrie auskommt.)
///
/// Mit `dbs = |p − bound_c| − bound_r ≤ de` gilt
///
/// ```text
/// dbs ≥ K   ⇒   channel_limit ≡ −1e30              (erste Zeile von `channel_limit`)
/// dbs ≥ K ∧ (K − dbs ≤ ∓d)  ⇒  channel_apply ≡ d   (`channel_skip`)
/// ```
pub fn channel_is_neutral(c: &mut Ctx, p: [Iv; 3], d: Iv, ch: &OffsetChannel) -> bool {
    let Ok(bc) = fsub3(c, p, c3(ch.bound_c)) else { return false };
    let Ok(nb) = c.fnorm(bc) else { return false };
    let Ok(dbs) = c.fsub(nb, Iv::f32p(ch.bound_r)) else { return false };
    if decide_ge(dbs, Iv::f32p(ch.trust)) != Decision::True {
        return false;
    }
    let Ok(lhs) = c.fsub(Iv::f32p(ch.trust), dbs) else { return false };
    let rhs = if ch.concave { d.neg() } else { d };
    decide_le(lhs, rhs) == Decision::True
}

/// [`crate::offset_channel::channel_apply`], eingeschlossen — die
/// §8v-Erweiterung.
///
/// Struktur identisch zu [`enclose_vertex_apply`], nur mit dem
/// Nearest-Segment statt des Punkt-Spines und zwei zusätzlichen affinen Gates.
pub fn enclose_channel_apply(
    c: &mut Ctx,
    p: [Iv; 3],
    base: Iv,
    d: Iv,
    ch: &OffsetChannel,
    extra: Iv,
) -> Enc {
    // Der Fernfeld-Skip, als Schalter.
    let bc = fsub3(c, p, c3(ch.bound_c))?;
    let nb = c.fnorm(bc)?;
    let dbs = c.fsub(nb, Iv::f32p(ch.bound_r))?;
    let lhs = c.fsub(Iv::f32p(ch.trust), dbs)?;
    let rhs = if ch.concave { d.neg() } else { d };
    // `channel_skip` ist ein UND aus zwei Bedingungen.
    let skip = match (decide_le(lhs, rhs), decide_ge(dbs, Iv::f32p(ch.trust))) {
        (Decision::True, Decision::True) => Decision::True,
        (Decision::False, _) | (_, Decision::False) => Decision::False,
        _ => Decision::Both,
    };
    if skip == Decision::True {
        return Ok(d);
    }
    let body = |c: &mut Ctx| -> Enc {
        // §8w: der komplette Downstream JE Kandidat, danach die Hülle über die
        // Ergebnisse. Nicht über `q*`/`m̂` zuerst zu hüllen ist wesentlich —
        // das wäre zwar auch sound, aber deutlich breiter.
        let cands = enc_channel_candidates(c, p, ch)?;
        let gate = if ch.concave {
            enclose_concave_gate(c, ch.gate, base, extra)?
        } else {
            c.fadd(Iv::f32p(ch.gate), base)?
        };
        let s3 = enc_gate_planes(c, p, &ch.end_planes)?;
        let s4 = enc_gate_planes(c, p, &ch.face_planes)?;
        // Der `None`-Ausgang: `channel_apply` gibt dann `d` unverändert zurück.
        let mut out: Option<Iv> = if cands.none_possible { Some(d) } else { None };
        for near in &cands.arcs {
            // **Downstream-Lemma**: JEDE erreichbare Alternative läuft
            // vollständig durch `t → enc_nlerp → channel_apply`; erst die
            // ERGEBNISSE werden gehüllt. `q`/`t` vorher getrennt zu hüllen wäre
            // sound, aber breiter — es kombinierte Paare, die die Produktion
            // nie erzeugt (Gegenprobe: `Knobs::arc_hull_qt_early`).
            let alts: Vec<ArcAlt> = if c.knobs.arc_hull_qt_early {
                let mut q = near.alts[0].q;
                let mut t = near.alts[0].t;
                for a in &near.alts[1..] {
                    for k in 0..3 {
                        q[k] = q[k].hull(a.q[k]);
                    }
                    t = t.hull(a.t);
                }
                vec![ArcAlt { de: near.de_hull, q, t }]
            } else {
                near.alts.clone()
            };
            let arc = &ch.arcs[near.idx];
            if !(arc.sweep < 0.0) {
                c.curved_downstream += 1;
            }
            for alt in &alts {
                let m1 = enc_nlerp(c, arc.m1s, arc.m1e, alt.t)?;
                let m2 = enc_nlerp(c, arc.m2s, arc.m2e, alt.t)?;
                let dq = fsub3(c, p, alt.q)?;
                let s1 = c.fdot(dq, m1)?;
                let s2 = c.fdot(dq, m2)?;
                let rd = c.fsub(alt.de, Iv::f32p(ch.delta))?;
                let tr = c.fsub(Iv::f32p(ch.trust), alt.de)?;
                let w_geo = s1.min(s2).min(s3).min(s4).min(rd).min(tr);
                let w = w_geo.min(gate);
                let v = if !ch.concave {
                    d.max(w)
                } else {
                    three_way_concave(c, d, w, w_geo)?
                };
                out = Some(match out {
                    Some(o) => o.hull(v),
                    None => v,
                });
            }
        }
        out.ok_or(Unknown("leere Kandidaten-Menge"))
    };
    sw(c, skip, |_| Ok(d), body)
}

/// Der Drei-Wege-Schalter am Ende von `channel_apply`/`vertex_apply`
/// (konkaver Zweig) — geteilt, damit beide Terme bit-gleich modelliert sind.
fn three_way_concave(c: &mut Ctx, d: Iv, w: Iv, w_geo: Iv) -> Enc {
    let zero = Iv::point(0.0);
    let mut out: Option<Iv> = None;
    let mut add = |v: Iv| {
        out = Some(match out {
            Some(o) => o.hull(v),
            None => v,
        })
    };
    let w_pos = decide_gt(w, zero);
    if w_pos != Decision::False {
        add(d.min(w.neg()));
        if c.knobs.first_branch_only {
            return out.ok_or(Unknown("leerer Schalter"));
        }
    }
    if w_pos != Decision::True {
        let g_neg = decide_le(w_geo, zero);
        if g_neg != Decision::False {
            add(d.min(w_geo.neg()));
            if c.knobs.first_branch_only {
                return out.ok_or(Unknown("leerer Schalter"));
            }
        }
        if g_neg != Decision::True {
            add(d);
        }
    }
    out.ok_or(Unknown("leerer Schalter"))
}

/// [`crate::bvh::distance_point_to_aabb`], eingeschlossen — nur für die
/// Prune-Fallanalyse in [`enclose_node`].
fn enc_dist_aabb(c: &mut Ctx, p: [Iv; 3], bb: crate::csg::Aabb) -> Enc {
    if bb.is_empty() {
        return Ok(Iv::point(f64::INFINITY));
    }
    // **Der exakte Null-Kurzschluss** (§8u-Korrektur). Liegt die ganze Zelle in
    // der AABB, ist `(min−p).max(0).max(p−max)` für JEDE Achse exakt `0.0` —
    // beide Differenzen sind `≤ 0`, und `max` rechnet nicht, es wählt. Also
    // sind Quadratsumme und Wurzel exakt null, und die Produktion liefert
    // exakt `0.0`.
    //
    // Ohne diesen Kurzschluss weitet die Aussen-Rundung die Null auf ein
    // winziges POSITIVES Intervall — und genau daran hing die Prune-Bedingung
    // `second_bound > 0.0` in [`enclose_node`]: der Prune-Fall wurde
    // zugelassen, `min(d_first, bnd)` kollabierte auf ~0, und die obere
    // Schranke einer Vereinigung tief im Material sprang von `−4` auf `0`.
    // Das war keine Unsoundness (zu WEIT ist sicher), aber es hat Zweig 2
    // im ganzen Material-Inneren blind gemacht.
    if (0..3).all(|k| p[k].lo >= bb.min[k] as f64 && p[k].hi <= bb.max[k] as f64) {
        return Ok(Iv::point(0.0));
    }
    let mut q = [Iv::point(0.0); 3];
    for k in 0..3 {
        let a = c.fsub(Iv::f32p(bb.min[k]), p[k])?;
        let b = c.fsub(p[k], Iv::f32p(bb.max[k]))?;
        q[k] = a.max(Iv::point(0.0)).max(b);
    }
    let s = c.fdot(q, q)?;
    let d = c.fsqrt(s)?;
    // Eine f32-Distanz ist nichtnegativ; die Aussen-Rundung darf die untere
    // Schranke nicht unter null drücken.
    Iv::new(d.lo.max(0.0), d.hi.max(0.0)).ok_or(Unknown("NaN(aabb)"))
}

/// Ein Primitiv, eingeschlossen. Unterstützt sind `Box`, `Sphere` und
/// `Cylinder` — die drei geschlossenen Formen, aus denen die §8p-Matrix
/// besteht. Alles andere ist [`Unknown`].
fn enc_primitive(c: &mut Ctx, prim: &Primitive, p: [Iv; 3]) -> Enc {
    let zero = Iv::point(0.0);
    match prim {
        Primitive::Box { center, half_extents } => {
            let mut q = [zero; 3];
            for k in 0..3 {
                let t = c.fsub(p[k], Iv::f32p(center[k]))?;
                q[k] = c.fsub(t.abs(), Iv::f32p(half_extents[k]))?;
            }
            let qc = [q[0].max(zero), q[1].max(zero), q[2].max(zero)];
            let s = c.fdot(qc, qc)?;
            let outside = c.fsqrt(s)?;
            let qmax = q[0].max(q[1]).max(q[2]);
            let inside = qmax.min(zero);
            c.fadd(outside, inside)
        }
        Primitive::Sphere { center, radius } => {
            let d = fsub3(c, p, c3(*center))?;
            let n = c.fnorm(d)?;
            c.fsub(n, Iv::f32p(*radius))
        }
        Primitive::Cylinder { center, radius, half_height } => {
            let dx = c.fsub(p[0], Iv::f32p(center[0]))?;
            let dy = c.fsub(p[1], Iv::f32p(center[1]))?;
            let dz = c.fsub(p[2], Iv::f32p(center[2]))?;
            let xx = c.fmul(dx, dx)?;
            let zz = c.fmul(dz, dz)?;
            let s = c.fadd(xx, zz)?;
            let r = c.fsqrt(s)?;
            let radial = c.fsub(r, Iv::f32p(*radius))?;
            let axial = c.fsub(dy.abs(), Iv::f32p(*half_height))?;
            let rc = radial.max(zero);
            let ac = axial.max(zero);
            let rr = c.fmul(rc, rc)?;
            let aa = c.fmul(ac, ac)?;
            let sum = c.fadd(rr, aa)?;
            let outside = c.fsqrt(sum)?;
            let inside = radial.max(axial).min(zero);
            c.fadd(outside, inside)
        }
        _ => Err(Unknown("Primitiv")),
    }
}

/// [`crate::transform::world_to_local`], eingeschlossen. Für die Identität
/// (`q = (0,0,0,1)`, `scale = 1`) ist die Einschliessung exakt — jedes
/// Produkt ist dann `0 · Intervall = 0` bzw. `1 · Intervall`.
fn enc_world_to_local(c: &mut Ctx, t: &Transform, p: [Iv; 3]) -> Result<[Iv; 3], Unknown> {
    let v = fsub3(c, p, c3(t.translate))?;
    let (qx, qy, qz, qw) = (
        Iv::f32p(-t.rotation[0]),
        Iv::f32p(-t.rotation[1]),
        Iv::f32p(-t.rotation[2]),
        Iv::f32p(t.rotation[3]),
    );
    let two = Iv::point(2.0);
    let mk = |c: &mut Ctx, a: Iv, x: Iv, b: Iv, y: Iv| -> Enc {
        let m0 = c.fmul(a, x)?;
        let m1 = c.fmul(b, y)?;
        let s = c.fsub(m0, m1)?;
        c.fmul(two, s)
    };
    let t0 = mk(c, qy, v[2], qz, v[1])?;
    let t1 = mk(c, qz, v[0], qx, v[2])?;
    let t2 = mk(c, qx, v[1], qy, v[0])?;
    // **Die Produktions-Klammerung, Zeichen für Zeichen** (§8u-Korrektur).
    // `v[0] + qw * t[0] + qy * t[2] - qz * t[1]` parst Rust links-assoziativ
    // als `(((v₀ ⊕ (qw⊗t₀)) ⊕ (qy⊗t₂)) ⊖ (qz⊗t₁))`. Die erste §8u-Fassung
    // rechnete stattdessen erst den Rotationsanteil zusammen und addierte `v`
    // zum Schluss — über ℝ dasselbe, über f32 NICHT: bei Auslöschung zwischen
    // `v` und dem Rotationsanteil weicht die Reassoziation weit mehr ab, als
    // die Ein-ULP-Weitung deckt. Damit trug Beweispflicht 5 für gedrehte
    // Blätter nicht.
    let comb = |c: &mut Ctx, vk: Iv, base: Iv, a: Iv, b: Iv, x: Iv, y: Iv| -> Enc {
        let m0 = c.fmul(qw, base)?;
        let m1 = c.fmul(a, x)?;
        let m2 = c.fmul(b, y)?;
        if c.knobs.reassociate_rotation {
            // Gegenprobe: die falsche (reassoziierte) Reihenfolge.
            let s = c.fadd(m0, m1)?;
            let s = c.fsub(s, m2)?;
            return c.fadd(vk, s);
        }
        let s = c.fadd(vk, m0)?;
        let s = c.fadd(s, m1)?;
        c.fsub(s, m2)
    };
    let e0 = comb(c, v[0], t0, qy, qz, t2, t1)?;
    let e1 = comb(c, v[1], t1, qz, qx, t0, t2)?;
    let e2 = comb(c, v[2], t2, qx, qy, t1, t0)?;
    Ok([
        c.fdiv(e0, Iv::f32p(t.scale[0]))?,
        c.fdiv(e1, Iv::f32p(t.scale[1]))?,
        c.fdiv(e2, Iv::f32p(t.scale[2]))?,
    ])
}

/// **Die Einschliessung des ausgelieferten Feldes** über der Zelle
/// (`EvalMode::Visual`, leerer Open-Set — der Pfad, den die
/// Nullmengen-Messung fährt).
///
/// Die AABB-Prunes in `Union`/`Difference` werden NICHT als korrekt
/// vorausgesetzt (das wäre ein geerbtes Lipschitz-Versprechen), sondern als
/// FALLUNTERSCHEIDUNG behandelt: der Rückgabewert ist entweder der
/// `min`/`max`-Ausdruck oder — wenn der Prune greift — der Wert des ersten
/// Kindes unter der Prune-Bedingung. Eingeschlossen wird die Vereinigung
/// beider Fälle.
pub fn enclose_node(c: &mut Ctx, node: &BoundedCsg, p: [Iv; 3]) -> Enc {
    c.tick()?;
    match node {
        BoundedCsg::Empty => Ok(Iv::point(f64::INFINITY)),
        BoundedCsg::Leaf { primitive, world_transform, .. } => {
            let local = enc_world_to_local(c, world_transform, p)?;
            let d = enc_primitive(c, primitive, local)?;
            c.fmul(d, Iv::f32p(world_transform.min_scale()))
        }
        BoundedCsg::Union { left, right, .. } => {
            let dl = enclose_node(c, left, p)?;
            let dr = enclose_node(c, right, p)?;
            let m = dl.min(dr);
            // Der Prune gibt `d_first` zurück, wenn `dist(p, AABB(second)) > 0`
            // UND `d_first < dist(p, AABB(second))`. In BEIDEN Fällen gilt
            // `R ≥ min(dl,dr)`; nach oben deckelt die Prune-Bedingung selbst.
            let mut hi = m.hi;
            for (first, second) in [(dl, right.aabb()), (dr, left.aabb())] {
                let bnd = enc_dist_aabb(c, p, second)?;
                if bnd.hi > 0.0 {
                    hi = hi.max(first.hi.min(bnd.hi));
                }
            }
            Iv::new(m.lo, hi).ok_or(Unknown("NaN(union)"))
        }
        BoundedCsg::Difference { left, right, .. } => {
            let dl = enclose_node(c, left, p)?;
            let dr = enclose_node(c, right, p)?;
            let m = dl.max(dr.neg());
            // Prune ⇒ `dl`; sonst `max(dl, −dr) ≥ dl`. Die Hülle deckt beide.
            Ok(dl.hull(m))
        }
        BoundedCsg::Intersection { left, right, .. } => {
            let dl = enclose_node(c, left, p)?;
            let dr = enclose_node(c, right, p)?;
            Ok(dl.max(dr))
        }
        BoundedCsg::Offset { inner, delta, .. } => {
            let d = enclose_node(c, inner, p)?;
            c.fsub(d, Iv::f32p(*delta))
        }
        BoundedCsg::InnerWall { inner, .. } => enclose_node(c, inner, p),
        BoundedCsg::ArcChannel { inner, channels, verts, rims, lb, .. } => {
            if !rims.is_empty() {
                return Err(Unknown("Rim-Block"));
            }
            let d0 = enclose_node(c, inner, p)?;
            enclose_stop(c, p, d0, channels, verts, lb.as_ref())
        }
        _ => Err(Unknown("Operator")),
    }
}

// ════════════════════════════════════════════════════════════════════════════
// Der Konsument — Zell-Zertifizierung
// ════════════════════════════════════════════════════════════════════════════

/// Das Urteil über EINE Zelle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Zweig 1: `U(c) + ρ ≤ δ + tol` — kein Punkt der Zelle liegt tief genug,
    /// um überhaupt ein Unter-Claim sein zu KÖNNEN.
    Branch1,
    /// Zweig 2: `sup_C f < 0` — das Feld behauptet über der ganzen Zelle
    /// Material, ein Unter-Claim ist per Definition ausgeschlossen.
    Branch2,
    /// Weder noch. Der Grund ist benannt.
    Unknown(&'static str),
}

impl Verdict {
    pub fn certified(self) -> bool {
        !matches!(self, Verdict::Unknown(_))
    }
}

/// **Die Zertifizierung EINER Zelle** (Beweispflicht 9).
///
/// * **Zweig 1.** `dist(·,∁Ω)` ist 1-Lipschitz (Standard: für `y ∈ ∁Ω` ist
///   `dist(x,∁Ω) ≤ |x−y| ≤ |x−c| + |c−y|`, Infimum über `y` gibt
///   `dist(x,∁Ω) ≤ |x−c| + dist(c,∁Ω)`). Mit `|x−c| ≤ ρ` und
///   `dist(c,∁Ω) ≤ U(c)` folgt für JEDES `x ∈ C`:
///   `dist(x,∁Ω) ≤ U(c) + ρ ≤ δ + tol`, die zweite Hälfte der
///   Unter-Claim-Bedingung (`dist − δ > tol`) ist also für keinen Zellpunkt
///   erfüllbar. **Der Zweig braucht keine Intervall-Arithmetik und macht
///   keine Aussage über `f`.**
/// * **Zweig 2.** Ist `hi(f(C)) < 0`, dann ist `f(x) < 0` für jeden Zellpunkt,
///   die erste Hälfte der Bedingung (`f ≥ 0`) also für keinen erfüllbar.
///
/// `u_at` liefert eine obere Schranke von `dist(c,∁Ω)` (der §8r-Fuss-Zeuge)
/// oder `None`. Die Reihenfolge ist Absicht: Zweig 1 ist billig und deckt die
/// randnahen Zellen, Zweig 2 die tiefen.
pub fn certify_cell(
    cell: &Box3,
    field: &BoundedCsg,
    delta: f64,
    tol: f64,
    u_at: &dyn Fn([f64; 3]) -> Option<f64>,
    c: &mut Ctx,
) -> Verdict {
    let rho = cell.rho();
    if let Some(u) = u_at(cell.center()) {
        // **Beide Seiten gerichtet** (§8u-Korrektur): die linke Seite nach
        // oben, die RECHTE nach unten. Vorher rundete nur die linke aussen —
        // rundet `δ + tol` in f64 nach oben, akzeptierte die Prüfung knapp
        // OBERHALB der Schwelle. Jetzt impliziert das Bestehen
        // `u + ρ ≤ δ + tol` in ℝ.
        if next_up(u + rho) <= next_down(delta + tol) {
            return Verdict::Branch1;
        }
    }
    match enclose_node(c, field, cell.world_f32()) {
        Ok(iv) => {
            if iv.hi < 0.0 {
                Verdict::Branch2
            } else {
                Verdict::Unknown("sup f ≥ 0")
            }
        }
        Err(Unknown(why)) => Verdict::Unknown(why),
    }
}

/// Zertifizierung mit **adaptiver Unterteilung**. Erreicht die Rekursion
/// `max_depth`, ist das Ergebnis `Unknown` — nie Erfolg.
///
/// **Terminierung** (Beweispflicht 10, §8u-korrigiert): sie steht auf DREI
/// Schranken, nicht auf „jede Ebene halbiert". Erstens die Maximaltiefe
/// (`≤ 2^max_depth` Teilzellen). Zweitens [`Box3::split`], das eine numerisch
/// nicht mehr teilbare Zelle ehrlich verweigert — bei benachbarten oder sehr
/// grossen f64-Endpunkten fällt der Mittelpunkt mit einem Endpunkt zusammen,
/// und ohne diese Weigerung liefe die Rekursion auf der Elternzelle weiter.
/// Drittens das gemeinsame Operations-Budget in [`Ctx`].
///
/// Rückgabe: `Ok((n_branch1, n_branch2))` mit der Zahl der Teilzellen je
/// Zweig, oder der erste `Unknown`-Grund.
pub fn certify_cell_adaptive(
    cell: &Box3,
    field: &BoundedCsg,
    delta: f64,
    tol: f64,
    u_at: &dyn Fn([f64; 3]) -> Option<f64>,
    max_depth: u32,
    c: &mut Ctx,
) -> Result<(usize, usize), &'static str> {
    match certify_cell(cell, field, delta, tol, u_at, c) {
        Verdict::Branch1 => Ok((1, 0)),
        Verdict::Branch2 => Ok((0, 1)),
        Verdict::Unknown(why) => {
            if max_depth == 0 {
                return Err(why);
            }
            let Some((a, b)) = cell.split() else {
                return Err("Zelle numerisch nicht mehr teilbar");
            };
            let ra = certify_cell_adaptive(&a, field, delta, tol, u_at, max_depth - 1, c)?;
            let rb = certify_cell_adaptive(&b, field, delta, tol, u_at, max_depth - 1, c)?;
            Ok((ra.0 + rb.0, ra.1 + rb.1))
        }
    }
}
