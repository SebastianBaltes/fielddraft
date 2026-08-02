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
//! eine Entartung, NaN, Überlauf, Budget — endet in [`Unknown`]. Kein halber
//! Beweis.
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
//! ## Das f32-Modell — warum hier keine Fehlerkonstante steht
//!
//! Eingeschlossen wird **nicht** der reelle Wert des idealen Feldes, sondern
//! der Wert, den der f32-Auswerter an einem beliebigen Punkt der Zelle
//! LIEFERT. Das ist der Wert, über den die Zusage spricht, und es macht jede
//! Rundungs-Konstante überflüssig: jede Produktions-Operation wird als
//! „exakte reelle Operation, danach Rundung nach f32" modelliert
//! ([`Ctx::fadd`] & Co.). Die exakte reelle Operation wird in f64 mit
//! Aussen-Rundung ([`next_down`]/[`next_up`]) eingeschlossen, die f32-Rundung
//! danach mit **gerichteter** Rundung auf das f32-Gitter ([`f32_down`]/
//! [`f32_up`]) — beides exakt darstellbar, weil jedes f32 ein f64 ist.
//!
//! Für Summen und Produkte von f32-Punktwerten ist der f64-Schritt sogar
//! FEHLERFREI (24 + 24 ≤ 53 Mantissenbits), das Modell also scharf: bei einer
//! Punkt-Zelle liefert die Einschliessung exakt den Produktionswert. Getestet
//! wird genau das (`interval_encloses_the_production_vertex_term_384`).
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
use crate::offset_channel::{OffsetChannel, VertexSphere, END_PLANE_OFF, LB_AVAIL};
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
/// **Warum „Rundung weglassen" als Gegenprobe NICHT reicht.** Nach jedem
/// [`Ctx::seal`] liegen beide Schranken auf dem f32-Gitter. Für `+`, `−` und
/// `×` rechnet f64 mit solchen Operanden **exakt** (24 + 24 ≤ 53
/// Mantissenbits), die f64-Aussen-Rundung ist dort also nachweislich
/// wirkungslos und durch kein Sampling zu widerlegen. Falsifizierbar wird die
/// Sicherung erst, wenn man sie — wie schon `emax = −0.05` in §8t — **zu
/// scharf** stellt: ein ulp nach INNEN.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Knobs {
    /// Ein ulp nach INNEN statt nach aussen runden. Damit ist keine einzige
    /// Schranke mehr garantiert.
    pub inward_rounding: bool,
    /// Statt der Hülle den ERSTEN erreichbaren Zweig nehmen — der klassische
    /// „das wird schon der richtige sein"-Fehler.
    pub first_branch_only: bool,
}

/// Auswertungs-Kontext: Rundungs-Politik und Terminierungs-Budget.
#[derive(Debug, Clone)]
pub struct Ctx {
    pub knobs: Knobs,
    ops: u64,
    max_ops: u64,
}

impl Ctx {
    /// Ein Kontext mit dem üblichen Budget (eine Million Operationen je
    /// Zelle — genug für einen `L`-Block mit 512 Stücken, endlich genug für
    /// die Terminierung).
    pub fn new() -> Ctx {
        Ctx { knobs: Knobs::default(), ops: 0, max_ops: 4_000_000 }
    }

    pub fn with_knobs(knobs: Knobs) -> Ctx {
        Ctx { knobs, ..Ctx::new() }
    }

    /// Verbrauchte Operationen (Terminierungs-Zeuge).
    pub fn ops(&self) -> u64 {
        self.ops
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
    ///    fangen die f64-Rundung ab. (Für `+`, `−`, `×` mit f32-Endpunkten ist
    ///    der f64-Schritt exakt und dieser Teil ein No-op; für `÷` und `√`
    ///    ist er es nicht.)
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
    pub fn split(&self) -> (Box3, Box3) {
        let mut k = 0;
        for j in 1..3 {
            if self.hi[j] - self.lo[j] > self.hi[k] - self.lo[k] {
                k = j;
            }
        }
        let m = 0.5 * (self.lo[k] + self.hi[k]);
        let mut a = *self;
        let mut b = *self;
        a.hi[k] = m;
        b.lo[k] = m;
        (a, b)
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

    fn center(&self) -> [f64; 3] {
        [
            0.5 * (self.lo[0] + self.hi[0]),
            0.5 * (self.lo[1] + self.hi[1]),
            0.5 * (self.lo[2] + self.hi[2]),
        ]
    }

    fn rho(&self) -> f64 {
        // Aufgerundet in jedem Schritt — `ρ` ist eine ZUSAGE („weiter kann
        // kein Zellpunkt vom Mittelpunkt weg sein"), also darf sie nie zu
        // klein sein.
        let mut s = 0.0f64;
        for k in 0..3 {
            let h = next_up(0.5 * next_up(self.hi[k] - self.lo[k]));
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

/// **Affine Ebene über der Zelle, exakt.** `⟨p, n̂⟩ − c` in der
/// Produktions-Reihenfolge von [`crate::offset_channel::end_plane_gate`]
/// (`p₀g₀ + p₁g₁ + p₂g₂ − g₃`).
///
/// Über einer Box ist das **die exakte Reichweite** der affinen Form: die drei
/// Koordinaten sind unabhängig, Minimum und Maximum werden also
/// komponentenweise an je einer Ecke angenommen — die natürliche Auswertung
/// ist hier keine Überschätzung.
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
        match best {
            // Leerer Block: `best` bleibt `∞`, `is_finite` ist falsch ⇒ neutral.
            None => Ok(none),
            Some(b) => c.fsub(b, Iv::f32p(blk.slack)),
        }
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
        // Drei-Wege-Schalter. Ein Zweig darf NUR entfallen, wenn er über der
        // ganzen Zelle unerreichbar ist.
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
    };

    sw(c, skip, |_| Ok(d), body)
}

// ════════════════════════════════════════════════════════════════════════════
// Ebene 3 — der Stopp und der Baum darunter
// ════════════════════════════════════════════════════════════════════════════

/// **Ist dieser Kanal über der GANZEN Zelle neutral?**
///
/// Kanal-Terme werden in Paket 1 nie ausgewertet. Sie dürfen aber
/// *wegbewiesen* werden, und dafür gibt es genau EIN Argument, das ohne die
/// Naht-Geometrie auskommt: den Fernfeld-Skip über die Bounding-Sphere der
/// Kurve. Mit `dbs = |p − bound_c| − bound_r ≤ de` gilt
///
/// ```text
/// dbs ≥ K   ⇒   channel_limit ≡ −1e30              (erste Zeile von `channel_limit`)
/// dbs ≥ K ∧ (K − dbs ≤ ∓d)  ⇒  channel_apply ≡ d   (`channel_skip`)
/// ```
///
/// Beides muss über der ganzen Zelle UND für den ganzen laufenden `d`-Bereich
/// gelten. Genau das prüft diese Funktion — sie ist die einzige Stelle, an der
/// ein Kanal überhaupt vorkommt, und sie behauptet nichts über seinen Wert.
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
    // Die Kanäle zuerst: sie sind der billige Ausschluss, und `L` wird nur
    // gebraucht, wenn überhaupt eine Vertex-Kugel ihr Gate liest. Die
    // Reihenfolge ist auswertungs-neutral — `lb_value` hängt nur von `p` und
    // `d₀` ab, nicht vom laufenden `d`.
    for ch in channels {
        if !channel_is_neutral(c, p, d, ch) {
            return Err(Unknown("Kanal-Term (nicht als neutral bewiesen)"));
        }
    }
    let lbv = match lb {
        Some(b) if !verts.is_empty() => enclose_lb_value(c, p, d0, b)?,
        _ => Iv::f32p(LB_NONE),
    };
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

/// [`crate::bvh::distance_point_to_aabb`], eingeschlossen — nur für die
/// Prune-Fallanalyse in [`enclose_node`].
fn enc_dist_aabb(c: &mut Ctx, p: [Iv; 3], bb: crate::csg::Aabb) -> Enc {
    if bb.is_empty() {
        return Ok(Iv::point(f64::INFINITY));
    }
    let mut q = [Iv::point(0.0); 3];
    for k in 0..3 {
        let a = c.fsub(Iv::f32p(bb.min[k]), p[k])?;
        let b = c.fsub(p[k], Iv::f32p(bb.max[k]))?;
        q[k] = a.max(Iv::point(0.0)).max(b);
    }
    let s = c.fdot(q, q)?;
    c.fsqrt(s)
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
    let comb = |c: &mut Ctx, base: Iv, a: Iv, b: Iv, x: Iv, y: Iv| -> Enc {
        let m0 = c.fmul(qw, base)?;
        let m1 = c.fmul(a, x)?;
        let m2 = c.fmul(b, y)?;
        let s = c.fadd(m0, m1)?;
        c.fsub(s, m2)
    };
    let r0 = comb(c, t0, qy, qz, t2, t1)?;
    let r1 = comb(c, t1, qz, qx, t0, t2)?;
    let r2 = comb(c, t2, qx, qy, t1, t0)?;
    let e0 = c.fadd(v[0], r0)?;
    let e1 = c.fadd(v[1], r1)?;
    let e2 = c.fadd(v[2], r2)?;
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
        // Aufwärts gerundet: die Summe darf die Schranke nicht unterlaufen.
        if next_up(u + rho) <= delta + tol {
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
/// `max_depth`, ist das Ergebnis `Unknown` — nie Erfolg (Beweispflicht 10,
/// Terminierung: die Tiefe ist beschränkt, jede Ebene halbiert, die Zahl der
/// Zellen ist `≤ 2^max_depth`).
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
            let (a, b) = cell.split();
            let ra = certify_cell_adaptive(&a, field, delta, tol, u_at, max_depth - 1, c)?;
            let rb = certify_cell_adaptive(&b, field, delta, tol, u_at, max_depth - 1, c)?;
            Ok((ra.0 + rb.0, ra.1 + rb.1))
        }
    }
}
