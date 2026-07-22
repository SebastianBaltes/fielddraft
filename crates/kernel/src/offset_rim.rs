//! offset_tree #398 Phase 1: Synthese der ∂P-Reflexkanten-Fillets (RimFillet)
//! aus dem Kontakt-Prisma-Verschnitt (Design: `docs/research/sdf-metricity/
//! F396-RIM-OPCODE-DESIGN.md`).
//!
//! Das Kontakt-Prisma (`offset_channel::synthesize_contact_prisms{,_dilation}`)
//! tilgt den flächigen 2D-Innenfehler eines koplanaren submergierten Kontakts,
//! lässt aber an der Kontaktrand-Kante ∂P einen δ-schmalen konkaven B0-Sliver
//! (Erosion) bzw. konvexen Keil (Dilation) stehen. Dieses Modul leitet aus der
//! geklippten Kontaktkontur P — wie sie [`crate::offset_channel::contact_clips`]
//! liefert — die Rand-Fillet-Features ab, die den Rest tilgen:
//!
//! * Je ∂P-Kante wird **bündig** (Kante liegt auf einer echten Körperkante
//!   BEIDER Member — dort ist das Prisma-Feld schon exakt, KEIN Fillet) vs.
//!   **Stufe** (Reflexkante: der Prisma-Rand taucht in die Wand des größeren
//!   Members) klassifiziert. Die Klassifikation ist rein analytisch (Kanten-auf-
//!   Kontur-Rand-Test in der Kontaktebene, `edge_on_contour`), kein
//!   Sampling-Epsilon. FC Phase 3 (#398): die Klassifikation läuft über
//!   beliebige (auch nicht-konvexe) Member-Konturen; Reflex-Vertices von P
//!   bleiben ehrlich ungerundet (Design §5).
//! * Die zwei Wand-Außennormalen einer Stufen-Kante werden als SOLID-Außen-
//!   normalen orientiert (`n̂_A` = Kontaktfläche des Über-Standers, `n̂_B` = Seiten-
//!   wand des Rand-Members). Die Halbraum-Maske ist konkav Material-seitig
//!   (`⟨p−q, n̂⟩ ≤ 0`), konvex Luft-seitig (`≥ 0`) — Vorzeichenkonvention exakt
//!   wie im Prototyp `f396_rim_prototype.rs`.
//! * Treffen an einer ∂P-Ecke zwei Stufen-Kanten mit Wänden aus
//!   VERSCHIEDENEN Membern (echter Trieder-Reflex) zusammen, wird zusätzlich ein
//!   Punkt-Fillet (Vertex-Feature, 3 Wand-Masken) synthetisiert. Gehören beide
//!   Seitenwände demselben Member (konvexe Box-Ecke, z. B. Box-Stack), rundet
//!   die Nearest-Feature-Komposition die Ecke ohne Extra-Term (Design §2.4).
//!
//! Die Auswertung (`value`/`apply_rims`) ist die f64-Referenzsemantik — der
//! Vertrag, an dem sich die Phase-2-Evaluator-Spiegel (WGSL/CGAL/fidget)
//! messen. Sie ist eine 1:1-Übernahme des Prototyps. Dieses Modul wird in
//! Phase 1 NIRGENDS im Regen-Pfad aufgerufen (nur von Tests); es ändert das
//! ausgelieferte Feld nicht.

use crate::bvh::BoundedCsg;
use crate::offset_channel::{contact_clips, ContactClip};

// ─── f64-Vektor-Helfer (lokal, kein Modul-Zwang) ────────────────────────────

#[inline]
fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
#[inline]
fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
#[inline]
fn scl(a: [f64; 3], s: f64) -> [f64; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}
#[inline]
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
#[inline]
fn norm(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}
#[inline]
fn nrm(a: [f64; 3]) -> [f64; 3] {
    let n = norm(a).max(1e-30);
    [a[0] / n, a[1] / n, a[2] / n]
}
#[inline]
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

// ─── Datenstruktur ──────────────────────────────────────────────────────────

/// Ein Rand-Fillet an EINEM Segment der Kontaktrand-Kurve ∂P (Kante) oder an
/// einer ∂P-Ecke (`a == b`, trihedraler Reflex-Vertex).
///
/// `concave = true` (Erosion): additiv (`min`), `g ← min(g, δ − dist)`.
/// `concave = false` (Dilation): subtraktiv (`max`), `g ← max(g, dist − δ)`.
///
/// `n_a`, `n_b` (+ optional `n_c` am Trieder) sind die SOLID-Außennormalen der
/// Wände, die an ∂P zusammenstoßen. Die Halbraum-Maske hält den Term auf der
/// korrekten Seite BEIDER (bzw. aller drei) Wände — konkav Materialseite
/// (`⟨p−q, n̂⟩ ≤ 0`), konvex Luftseite (`≥ 0`). Alle Koordinaten Welt-f64
/// (Konvertierung zu f32 erst beim Opcode-Encoding, Phase 2).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RimFeature {
    /// Segment-Endpunkte (Kante) oder `a == b` (Ecke / trihedrale Reflexecke).
    pub a: [f64; 3],
    pub b: [f64; 3],
    pub n_a: [f64; 3],
    pub n_b: [f64; 3],
    /// Optionale dritte Wand (trihedrale Reflexecke, sonst `n_c = [0; 3]`).
    pub n_c: [f64; 3],
    /// Fillet-Radius (Betrag von δ, > 0).
    pub delta: f64,
    /// Trust-Radius K (Kanal-Ausmaß von der Kurve, > δ; Konvention 3·δ).
    pub trust: f64,
    /// `true`: Erosion (konkav, additiv). `false`: Dilation (konvex, subtraktiv).
    pub concave: bool,
}

impl RimFeature {
    /// Radiale Distanz zur Spine + Lotfußpunkt.
    pub fn closest(&self, p: [f64; 3]) -> (f64, [f64; 3]) {
        let (de, q, _) = self.closest_t(p);
        (de, q)
    }

    /// Wie [`closest`], liefert zusätzlich den (ungeklemmten) Projektions-
    /// parameter `t` für die End-Cap-Maske.
    pub fn closest_t(&self, p: [f64; 3]) -> (f64, [f64; 3], f64) {
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

    /// Korrektur-Feldwert, wenn der Punkt in Maske UND Trust liegt, sonst
    /// `None` (Term inaktiv). Konkav: `δ − dist`; konvex: `dist − δ`.
    ///
    /// Drei Masken-Bestandteile (F396 §2.2):
    /// * **Wand-Halbräume** (`n_a`, `n_b`, ggf. `n_c`): Material-/Luftseite.
    /// * **End-Caps** senkrecht zur Kante: der Fillet endet AN den ∂P-Ecken;
    ///   `t ∈ [0, 1]` strikt (die Ecke rundet die Nachbarkante via Nearest-
    ///   Feature, NICHT ein Aufweichen der Caps).
    /// * **Trust** K: jenseits ist die Kante nicht mehr die nächste ∂U-Fläche.
    pub fn value(&self, p: [f64; 3]) -> Option<f64> {
        let (de, q, t) = self.closest_t(p);
        if de > self.trust {
            return None;
        }
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
            if self.concave {
                dot(dq, n) <= eps
            } else {
                dot(dq, n) >= -eps
            }
        };
        if !(side(self.n_a) && side(self.n_b) && side(self.n_c)) {
            return None;
        }
        Some(if self.concave {
            self.delta - de
        } else {
            de - self.delta
        })
    }
}

/// Wendet den Satz Rand-Fillets auf den naiven (Prisma-)Wert `g` an:
/// NEAREST-Feature-Komposition (Design §2.4). Konkav: `min(g, δ − dist_nearest)`
/// (die NÄCHSTE Reflexkante bestimmt den Radius — NICHT sequenzielles `min`
/// über alle Kanten, das nähme fälschlich die FERNSTE und überfüllte). Konvex
/// dual mit `max`. Leerer Satz ⇒ `g` unverändert.
pub fn apply_rims(g: f64, p: [f64; 3], rims: &[RimFeature]) -> f64 {
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
            if rims.is_empty() {
                g
            } else if rims[0].concave {
                g.min(v)
            } else {
                g.max(v)
            }
        }
        None => g,
    }
}

// ─── Bogen-Rand-Fillet (FC Phase 4, #398) ────────────────────────────────────

/// Ein Rand-Fillet an einem BOGEN-Segment von ∂P (konvexer Kontakt-Bogen) ODER
/// — als Uniform-Träger des GEMISCHTEN Blocks — an einem geraden Segment
/// (`sweep < 0`: `c` = Startpunkt, `u` = Einheitsrichtung, `r` = Länge, `n_b` =
/// konstante Wandnormale). Beim Bogen (`sweep > 0`): `c` = Zentrum, `(u,v)` =
/// orthonormale Ebenenbasis (Bogenpunkt `c + r·(u·cosφ + v·sinφ)`,
/// `φ ∈ [0, sweep]`, `u` = Zentrum→Bogenanfang), und die Wandnormale ist RADIAL:
/// `n_b(q) = wall_sign·(q − c)/r`.
///
/// Warum ein GEMISCHTER Block (Bögen + Geraden zusammen): an einer Linie↔Bogen-
/// Ecke von ∂P (abgerundetes Rechteck, Sektor) überlappen beide Feature-
/// Schneisen. Nur EINE gemeinsame Nearest-Feature-Auswahl (kleinste
/// Kanten-Distanz gewinnt) vermeidet die Über-Addition der FERNEREN Kante — ein
/// sequenzielles `min` über getrennte Blöcke (gerade `OP_RIM_FILLET` +
/// `OP_RIM_FILLET_ARC`) nähme an der Ecke fälschlich die fernere und überfüllte
/// (Design §5, wie beim geraden Block). Darum trägt der Arc-Block bei
/// Bogen-Präsenz ALLE Features des Stopps (Geraden als Segment-Records).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RimArcFeature {
    pub c: [f64; 3],
    pub u: [f64; 3],
    pub v: [f64; 3],
    pub n_a: [f64; 3],
    pub n_b: [f64; 3],
    pub r: f64,
    /// `> 0`: Bogen-Sweep (rad); `< 0`: gerades Segment (Länge in `r`).
    pub sweep: f64,
    /// Bogen: `±1` (SOLID-Außennormale = `wall_sign·radial`); Segment: ungenutzt.
    pub wall_sign: f64,
    pub delta: f64,
    pub trust: f64,
    pub concave: bool,
}

impl RimArcFeature {
    /// Segment-Träger aus einem geraden [`RimFeature`] (im gemischten Block).
    /// `n_c` (Trieder-Vertex) entfällt — Trieder-Vertices treten nur an
    /// versetzten Kontakten auf, die das Konkavitäts-Gate verweigert; sie können
    /// nie mit Bögen im selben Block koexistieren.
    pub fn from_line(f: &RimFeature) -> RimArcFeature {
        debug_assert!(f.n_c.iter().all(|c| c.abs() < 1e-12), "Trieder-Vertex im Arc-Block");
        let d = sub(f.b, f.a);
        let len = norm(d);
        RimArcFeature {
            c: f.a,
            u: if len > 1e-12 { scl(d, 1.0 / len) } else { [1.0, 0.0, 0.0] },
            v: [0.0; 3],
            n_a: f.n_a,
            n_b: f.n_b,
            r: len,
            sweep: -1.0,
            wall_sign: 0.0,
            delta: f.delta,
            trust: f.trust,
            concave: f.concave,
        }
    }

    /// Lotfußpunkt `q`, Kanten-Distanz `de`, `in_cap` (Projektion innerhalb der
    /// Segment-/Bogen-Enden). f64-Vertrag; f32-Spiegel [`rim_arc_foot`].
    fn foot(&self, p: [f64; 3]) -> (f64, [f64; 3], bool) {
        let cap_eps = 1e-6;
        if self.sweep < 0.0 {
            let len = self.r.max(1e-12);
            let s = dot(sub(p, self.c), self.u);
            let t = s / len;
            let sc = s.clamp(0.0, len);
            let q = add(self.c, scl(self.u, sc));
            (norm(sub(p, q)), q, !(t < -cap_eps || t > 1.0 + cap_eps))
        } else {
            let dc = sub(p, self.c);
            let x = dot(dc, self.u);
            let y = dot(dc, self.v);
            let mut phi = y.atan2(x);
            if phi < 0.0 {
                phi += std::f64::consts::TAU;
            }
            let in_cap = phi <= self.sweep + cap_eps;
            let pc = phi.clamp(0.0, self.sweep);
            let (s, cs) = pc.sin_cos();
            let q = add(self.c, add(scl(self.u, self.r * cs), scl(self.v, self.r * s)));
            (norm(sub(p, q)), q, in_cap)
        }
    }

    fn wall_normal_at(&self, q: [f64; 3]) -> [f64; 3] {
        if self.sweep < 0.0 {
            self.n_b
        } else {
            nrm(scl(sub(q, self.c), self.wall_sign))
        }
    }

    pub fn closest(&self, p: [f64; 3]) -> (f64, [f64; 3]) {
        let (de, q, _) = self.foot(p);
        (de, q)
    }

    pub fn value(&self, p: [f64; 3]) -> Option<f64> {
        let (de, q, in_cap) = self.foot(p);
        if !in_cap || de > self.trust {
            return None;
        }
        let dq = sub(p, q);
        let eps = 1e-9;
        let n_b = self.wall_normal_at(q);
        let side = |n: [f64; 3]| -> bool {
            if norm(n) < 0.5 {
                return true;
            }
            if self.concave {
                dot(dq, n) <= eps
            } else {
                dot(dq, n) >= -eps
            }
        };
        if !(side(self.n_a) && side(n_b)) {
            return None;
        }
        Some(if self.concave { self.delta - de } else { de - self.delta })
    }
}

/// f64-Referenz-Komposition eines GEMISCHTEN Blocks (Nearest-Feature über ALLE
/// Segment- UND Bogen-Features, Gleichstand → erster). Spiegel von [`apply_rims`]
/// für den Arc-Block.
pub fn apply_rims_arc(g: f64, p: [f64; 3], feats: &[RimArcFeature]) -> f64 {
    let mut best: Option<(f64, f64, bool)> = None;
    for f in feats {
        if let Some(v) = f.value(p) {
            let (de, _) = f.closest(p);
            match best {
                Some((bd, _, _)) if bd <= de => {}
                _ => best = Some((de, v, f.concave)),
            }
        }
    }
    match best {
        Some((_, v, concave)) => {
            if concave {
                g.min(v)
            } else {
                g.max(v)
            }
        }
        None => g,
    }
}

/// f32-Record EINES gemischten Arc-Block-Features (Segment oder Bogen); die
/// verlustbehaftete f32-Projektion eines [`RimArcFeature`]. `concave` steht (wie
/// beim geraden Block) im Block-Header, nicht im Record.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RimArcRecord {
    pub c: [f32; 3],
    pub u: [f32; 3],
    pub v: [f32; 3],
    pub n_a: [f32; 3],
    pub n_b: [f32; 3],
    pub r: f32,
    pub sweep: f32,
    pub wall_sign: f32,
    pub delta: f32,
    pub trust: f32,
}

impl RimArcRecord {
    pub fn from_feature(f: &RimArcFeature) -> RimArcRecord {
        let c = |v: [f64; 3]| [v[0] as f32, v[1] as f32, v[2] as f32];
        RimArcRecord {
            c: c(f.c),
            u: c(f.u),
            v: c(f.v),
            n_a: c(f.n_a),
            n_b: c(f.n_b),
            r: f.r as f32,
            sweep: f.sweep as f32,
            wall_sign: f.wall_sign as f32,
            delta: f.delta as f32,
            trust: f.trust as f32,
        }
    }
}

/// f32-Lotfußpunkt eines Arc-Records: `(de, q, in_cap)`. f32-Spiegel von
/// [`RimArcFeature::foot`] — die eps-Literale exakt gleich.
fn rim_arc_foot(rec: &RimArcRecord, p: [f32; 3]) -> (f32, [f32; 3], bool) {
    let cap_eps = 1e-6f32;
    if rec.sweep < 0.0 {
        let len = rec.r.max(1e-12);
        let s = dot32(sub32(p, rec.c), rec.u);
        let t = s / len;
        let sc = s.clamp(0.0, len);
        let q = [rec.c[0] + rec.u[0] * sc, rec.c[1] + rec.u[1] * sc, rec.c[2] + rec.u[2] * sc];
        (norm32(sub32(p, q)), q, !(t < -cap_eps || t > 1.0 + cap_eps))
    } else {
        let dc = sub32(p, rec.c);
        let x = dot32(dc, rec.u);
        let y = dot32(dc, rec.v);
        let mut phi = y.atan2(x);
        if phi < 0.0 {
            phi += std::f32::consts::TAU;
        }
        let in_cap = phi <= rec.sweep + cap_eps;
        let pc = phi.clamp(0.0, rec.sweep);
        let (s, cs) = pc.sin_cos();
        let q = [
            rec.c[0] + rec.r * (rec.u[0] * cs + rec.v[0] * s),
            rec.c[1] + rec.r * (rec.u[1] * cs + rec.v[1] * s),
            rec.c[2] + rec.r * (rec.u[2] * cs + rec.v[2] * s),
        ];
        (norm32(sub32(p, q)), q, in_cap)
    }
}

#[inline]
fn rim_arc_wall_normal(rec: &RimArcRecord, q: [f32; 3]) -> [f32; 3] {
    if rec.sweep < 0.0 {
        rec.n_b
    } else {
        let d = [q[0] - rec.c[0], q[1] - rec.c[1], q[2] - rec.c[2]];
        let n = norm32(d).max(1e-30);
        [rec.wall_sign * d[0] / n, rec.wall_sign * d[1] / n, rec.wall_sign * d[2] / n]
    }
}

/// Nearest-Feature-Auswahl (f32) über einen gemischten Arc-Block. Rückgabe:
/// (Index, Lotfußpunkt `q`, Kanten-Distanz `de`). Anker für den CPU-Opcode
/// (`OP_RIM_FILLET_ARC`) UND den Dual-Spiegel (`sdf_dual::apply_rims_arc_dual`).
pub fn select_rim_arc_f32(
    p: [f32; 3],
    concave: bool,
    recs: &[RimArcRecord],
) -> Option<(usize, [f32; 3], f32)> {
    let mut best: Option<(usize, [f32; 3], f32)> = None;
    for (i, rec) in recs.iter().enumerate() {
        let (de, q, in_cap) = rim_arc_foot(rec, p);
        if !in_cap || de > rec.trust {
            continue;
        }
        let dq = sub32(p, q);
        let n_b = rim_arc_wall_normal(rec, q);
        let eps = 1e-9f32;
        let side = |n: [f32; 3]| -> bool {
            if norm32(n) < 0.5 {
                return true;
            }
            let s = dot32(dq, n);
            if concave {
                s <= eps
            } else {
                s >= -eps
            }
        };
        if !(side(rec.n_a) && side(n_b)) {
            continue;
        }
        match best {
            Some((_, _, bd)) if bd <= de => {}
            _ => best = Some((i, q, de)),
        }
    }
    best
}

/// f32-Spiegel von [`apply_rims_arc`] — die Referenz für den
/// `OP_RIM_FILLET_ARC`-Opcode-Decode.
pub fn apply_rims_arc_f32(g: f32, p: [f32; 3], concave: bool, recs: &[RimArcRecord]) -> f32 {
    match select_rim_arc_f32(p, concave, recs) {
        Some((i, _, de)) => {
            let d = recs[i].delta;
            if concave {
                g.min(d - de)
            } else {
                g.max(de - d)
            }
        }
        None => g,
    }
}

// ─── f32-Opcode-Form (offset_tree Phase 2, #398) ─────────────────────────────

#[inline]
fn sub32(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
#[inline]
fn dot32(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
#[inline]
fn norm32(a: [f32; 3]) -> f32 {
    dot32(a, a).sqrt()
}

/// f32-Record EINES Rand-Fillet-Features, wie ihn der Opcode-Strom trägt: die
/// verlustbehaftete f32-Projektion eines [`RimFeature`]. `concave` ist NICHT
/// Teil des Records — es ist pro Offset-Stopp homogen (die Nearest-Feature-
/// Komposition mischt konkav/konvex nie innerhalb eines Stopps) und steht im
/// Block-Header. Ein Record ist genau ein maskiertes Kanten- oder Ecken-Feature;
/// die punkt-abhängige Nearest-Feature-Auswahl wird erst zur Auswertung
/// aufgelöst.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RimRecord {
    pub a: [f32; 3],
    pub b: [f32; 3],
    pub n_a: [f32; 3],
    pub n_b: [f32; 3],
    pub n_c: [f32; 3],
    pub delta: f32,
    pub trust: f32,
}

impl RimRecord {
    /// f32-Projektion eines Welt-f64-[`RimFeature`]. `concave` wandert (homogen
    /// je Stopp) in den Block-Header und ist hier nicht Teil des Records.
    pub fn from_feature(f: &RimFeature) -> RimRecord {
        let c = |v: [f64; 3]| [v[0] as f32, v[1] as f32, v[2] as f32];
        RimRecord {
            a: c(f.a),
            b: c(f.b),
            n_a: c(f.n_a),
            n_b: c(f.n_b),
            n_c: c(f.n_c),
            delta: f.delta as f32,
            trust: f.trust as f32,
        }
    }
}

/// Ein homogener Rand-Fillet-Block EINES dualen Offset-Stopps (offset_tree
/// Phase 4, #398): alle Records teilen `concave` (die Nearest-Feature-
/// Komposition mischt konkav/konvex nie innerhalb eines Stopps). Trägt der
/// [`crate::bvh::BoundedCsg::ArcChannel`]-Korrekturknoten; der Compiler
/// emittiert je Block genau EIN [`crate::sdf_compiler::encode_rim_block`]
/// NACH dem Basisfeld-Wert des Stopps.
#[derive(Clone, Debug, PartialEq)]
pub struct RimBlock {
    pub concave: bool,
    pub recs: Vec<RimRecord>,
    /// FC Phase 4 (#398): der GEMISCHTE Arc-Block. Nicht-leer GENAU DANN, wenn
    /// der Stopp mindestens ein konvexes Bogen-∂P-Segment trägt — dann wandern
    /// ALLE Features (auch die geraden, als Segment-Records) hier hinein und
    /// `recs` bleibt leer (der Compiler emittiert `OP_RIM_FILLET_ARC` statt
    /// `OP_RIM_FILLET`). Bei rein geradem Kontakt (Box, Polygon-Profil) ist
    /// `arc_recs` leer und alles läuft byte-identisch über `recs` /
    /// `OP_RIM_FILLET` (Golden-Feldneutralität).
    pub arc_recs: Vec<RimArcRecord>,
    /// Ehrlicher B0-Restfehler (≈0.75·δ) an ∂P-Stellen, die der Rim NICHT
    /// rundet: Reflex-(Konkav-)Vertices eines nicht-konvexen Kontakts P und
    /// Bogen-Stufen (Design §5 — dort verwöben Kontakt-Rim und intrinsische
    /// SSI-Naht, die #395-Stufe-2-Klemme). `0.0`, wenn der Rim den ganzen
    /// ∂P-Saum schließt (konvexes P wie Box-Box). REIN für die Capability —
    /// NICHT Teil des Tapes (`encode_rim_block` liest nur `concave`+`recs`).
    pub residual_bound: f32,
}

impl RimBlock {
    /// Synthetisiert den Rim-Block eines dualen Offset-Stopps (`inner` = das
    /// Boolean-Kind, `delta_signed` = das ±δ des Stopps). Leerer Kontakt oder
    /// vollständige Verweigerung ⇒ `None` (kein leerer Block emittiert). Die
    /// Records kommen aus derselben `contact_clips`-Quelle wie die Prisma-
    /// Emission (faithful Accept/Refuse-Mirror), damit Fillet und Prisma
    /// deckungsgleich sind. `residual_bound` meldet den ehrlichen ∂P-Rest an
    /// Reflex-Vertices/Bögen (0 bei konvexem P).
    pub fn from_stop(inner: &BoundedCsg, delta_signed: f32) -> Option<RimBlock> {
        let (feats, arc_feats, residual) = synth_rim_features(inner, delta_signed);
        if feats.is_empty() && arc_feats.is_empty() {
            return None;
        }
        let concave = delta_signed < 0.0;
        if arc_feats.is_empty() {
            // Rein gerader Kontakt → OP_RIM_FILLET (byte-identisch zum Bestand).
            let recs: Vec<RimRecord> = feats.iter().map(RimRecord::from_feature).collect();
            Some(RimBlock { concave, recs, arc_recs: Vec::new(), residual_bound: residual as f32 })
        } else {
            // Bogen präsent → GEMISCHTER Block: alle geraden Features als
            // Segment-Records + die Bögen, in EINER Nearest-Feature-Auswahl.
            let mut arc_recs: Vec<RimArcRecord> = feats
                .iter()
                .map(|f| RimArcRecord::from_feature(&RimArcFeature::from_line(f)))
                .collect();
            arc_recs.extend(arc_feats.iter().map(RimArcRecord::from_feature));
            Some(RimBlock { concave, recs: Vec::new(), arc_recs, residual_bound: residual as f32 })
        }
    }
}

/// Radialdistanz + (geklemmter) Lotfußpunkt `q` + (ungeklemmter) Parameter `t`.
/// f32-Spiegel von [`RimFeature::closest_t`].
fn rim_closest_t(rec: &RimRecord, p: [f32; 3]) -> (f32, [f32; 3], f32) {
    let d = sub32(rec.b, rec.a);
    let len2 = dot32(d, d);
    if len2 < 1e-24 {
        return (norm32(sub32(p, rec.a)), rec.a, 0.0);
    }
    let t_raw = dot32(sub32(p, rec.a), d) / len2;
    let t = t_raw.clamp(0.0, 1.0);
    let q = [rec.a[0] + d[0] * t, rec.a[1] + d[1] * t, rec.a[2] + d[2] * t];
    (norm32(sub32(p, q)), q, t_raw)
}

/// Nearest-Feature-Auswahl (f32) — das maskierte Feature mit kleinster
/// Kanten-Distanz. Masken exakt wie [`RimFeature::value`] (Trust, End-Caps
/// `t ∈ [0, 1]` strikt mit `cap_eps = 1e-6`, Wand-Halbräume mit `eps = 1e-9`,
/// ungenutzte `n_c = [0; 3]` → Maske true). Bei Gleichstand gewinnt der ERSTE
/// Kandidat (wie [`apply_rims`]). Rückgabe: (Index, Lotfußpunkt `q`,
/// Kanten-Distanz `de`). Anker sowohl für den CPU-Opcode als auch für den
/// Dual-Spiegel (`sdf_dual::apply_rims_dual`).
pub fn select_rim_f32(
    p: [f32; 3],
    concave: bool,
    recs: &[RimRecord],
) -> Option<(usize, [f32; 3], f32)> {
    let mut best: Option<(usize, [f32; 3], f32)> = None;
    for (i, rec) in recs.iter().enumerate() {
        let (de, q, t) = rim_closest_t(rec, p);
        if de > rec.trust {
            continue;
        }
        let seg = norm32(sub32(rec.b, rec.a));
        if seg > 1e-9 {
            let cap_eps = 1e-6;
            if t < -cap_eps || t > 1.0 + cap_eps {
                continue;
            }
        }
        let dq = sub32(p, q);
        let eps = 1e-9;
        let side = |n: [f32; 3]| -> bool {
            if norm32(n) < 0.5 {
                return true; // ungenutzte dritte Wand
            }
            if concave {
                dot32(dq, n) <= eps
            } else {
                dot32(dq, n) >= -eps
            }
        };
        if !(side(rec.n_a) && side(rec.n_b) && side(rec.n_c)) {
            continue;
        }
        match best {
            Some((_, _, bd)) if bd <= de => {}
            _ => best = Some((i, q, de)),
        }
    }
    best
}

/// f32-Spiegel von [`apply_rims`] — die Referenz für den CPU-Opcode-Decode.
/// Konkav: `min(g, δ − de_nearest)`; konvex: `max(g, de_nearest − δ)`. Leerer
/// Satz oder kein maskiertes Feature ⇒ `g` unverändert.
pub fn apply_rims_f32(g: f32, p: [f32; 3], concave: bool, recs: &[RimRecord]) -> f32 {
    match select_rim_f32(p, concave, recs) {
        Some((i, _, de)) => {
            let d = recs[i].delta;
            if concave {
                g.min(d - de)
            } else {
                g.max(de - d)
            }
        }
        None => g,
    }
}

// ─── Synthese ───────────────────────────────────────────────────────────────

/// Wie eine ∂P-Kante zur Kontur des Kontakt-Members steht.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EdgeClass {
    /// Kante liegt auf der Randlinie BEIDER Member-Flächen (koinzidente echte
    /// Körperkante) → das Prisma-Feld ist dort schon exakt, kein Fillet.
    Flush,
    /// Reflexstufe: die Kante liegt auf Member `wall`'s Rand und IM Inneren
    /// des Über-Standers → Fillet nötig. `wall` ist 0 (= Member i) oder 1 (j).
    Step { wall: u8 },
    /// Kein Rand-Bezug (degenerierter/kollinearer Verschnitt) → verworfen.
    Degenerate,
}

/// Liegt das Segment `a–b` (bis auf `tol`) auf einer Randlinie der (beliebigen,
/// auch nicht-konvexen) CCW-Kontur `contour`? Rein analytischer Kollinearitäts-/
/// Koinzidenz-Test (Lot-Distanz beider Endpunkte zu je einer Kanten-Geraden der
/// Kontur), KEIN Sampling-Epsilon. FC Phase 3 (#398): Verallgemeinerung von
/// `edge_on_rect` (vier Rechteck-Geraden) auf einen Loop mit beliebig vielen
/// Geraden-Segmenten — für ein Rechteck bit-identisch zum vorigen Test (dieselbe
/// Schleife über dieselben vier Kanten). Bogen-tragende Kontur-Segmente
/// (Member-Bulge) werden hier nicht behandelt; die Facetten-Konturen `la`/`lb`
/// sind Polygon-Züge (aus `loop3`), Bogen-∂P-Kanten filtert der Aufrufer vorab.
fn edge_on_contour(contour: &[[f64; 2]], a: [f64; 2], b: [f64; 2], tol: f64) -> bool {
    let m = contour.len();
    if m < 2 {
        return false;
    }
    for k in 0..m {
        let r0 = contour[k];
        let r1 = contour[(k + 1) % m];
        let d = [r1[0] - r0[0], r1[1] - r0[1]];
        let len = (d[0] * d[0] + d[1] * d[1]).sqrt();
        if len < 1e-12 {
            continue;
        }
        let perp = |q: [f64; 2]| (d[0] * (q[1] - r0[1]) - d[1] * (q[0] - r0[0])) / len;
        if perp(a).abs() < tol && perp(b).abs() < tol {
            return true;
        }
    }
    false
}

/// Klassifiziert eine ∂P-Kante über die beiden projizierten Member-Konturen
/// (`la`/`lb` — für Boxen Rechtecke, für Extrude-Deckel/-Mantel beliebige
/// Polygon-Züge).
fn classify_edge(clip: &ContactClip, a: [f64; 2], b: [f64; 2], tol: f64) -> EdgeClass {
    let on_i = edge_on_contour(&clip.la, a, b, tol);
    let on_j = edge_on_contour(&clip.lb, a, b, tol);
    match (on_i, on_j) {
        (true, true) => EdgeClass::Flush,
        // Kante auf j's Rand, IM Inneren von i → Über-Stander i, Wand = j.
        (false, true) => EdgeClass::Step { wall: 1 },
        // Kante auf i's Rand, IM Inneren von j → Über-Stander j, Wand = i.
        (true, false) => EdgeClass::Step { wall: 0 },
        (false, false) => EdgeClass::Degenerate,
    }
}

/// 2D → Welt (Rück-Projektion auf die Kontaktebene `n·x = o`).
fn lift(clip: &ContactClip, uv: [f64; 2]) -> [f64; 3] {
    add(
        add(scl(clip.e1, uv[0]), scl(clip.e2, uv[1])),
        scl(clip.n, clip.o),
    )
}

/// 2D-Richtung → Welt (nur Rotation, kein Ebenen-Offset).
fn lift_dir(clip: &ContactClip, uv: [f64; 2]) -> [f64; 3] {
    add(scl(clip.e1, uv[0]), scl(clip.e2, uv[1]))
}

/// Ein synthetisiertes Kanten-Feature samt Kontextdaten für die Ecken-/Trieder-
/// Erkennung (lokal zur Kontur; wird danach zu [`RimFeature`] reduziert).
struct StepEdge {
    /// Member-Index (0/1), dessen Seitenwand die Reflexstufe bildet.
    wall_member: u8,
    /// Gemeinsame Kontaktflächen-Normale (Über-Stander) — der `n_a` des Fillets.
    contact_n: [f64; 3],
    /// Seitenwand-Normale (`n_b`).
    wall_n: [f64; 3],
    /// ∂P-Kontur-Index des Start-Vertex dieser Kante.
    v_start: usize,
    /// ∂P-Kontur-Index des End-Vertex.
    v_end: usize,
    /// Welt-Position des End-Vertex (für die Ecken-Synthese).
    end_world: [f64; 3],
}

/// Leitet die Rand-Fillet-Features EINES Kontakt-Verschnitts ab. FC Phase 3
/// (#398): die ∂P-Loops kommen aus den Zusammenhangskomponenten des Kontakts
/// (`ContactClip.components` — äußere Kontur + Löcher, Design §3/§5). Fehlen
/// `components` (direkt konstruierte Clips der Unit-Tests), fällt die Synthese
/// auf das Ein-Polygon-`p2d` zurück. `residual` sammelt den ehrlichen B0-Rest
/// (≈0.75·δ) an ungerundeten ∂P-Reflex-Vertices/Bögen.
fn rims_from_clip(
    clip: &ContactClip,
    delta_signed: f64,
    out: &mut Vec<RimFeature>,
    arcs_out: &mut Vec<RimArcFeature>,
    residual: &mut f64,
) {
    let concave = delta_signed < 0.0;
    let da = delta_signed.abs();
    if da <= 0.0 {
        return;
    }
    let trust = 3.0 * da;
    // Koinzidenz-Toleranz für den Kanten-auf-Rand-Test: großzügig gegenüber
    // f64-Rundung/rotierten Frames, aber weit unter dem Über-Stand (½·(Fläche_A
    // − Fläche_B)) einer echten Stufe.
    let tol = (1e-3 * da).max(1e-5);

    if clip.components.is_empty() {
        // Fallback: Ein-Polygon-P (offset_rim-Unit-Tests). Für den echten
        // Regen-Pfad ist `components` stets befüllt (Box-Box: genau EINE gerade
        // Komponente == p2d, byte-identisch zu diesem Fallback).
        rim_loop(clip, &clip.p2d, None, false, concave, da, trust, tol, out, arcs_out, residual);
    } else {
        for comp in &clip.components {
            rim_loop(
                clip,
                &comp.outer.verts,
                Some(&comp.outer.bulges),
                false,
                concave,
                da,
                trust,
                tol,
                out,
                arcs_out,
                residual,
            );
            for hole in &comp.holes {
                rim_loop(
                    clip,
                    &hole.verts,
                    Some(&hole.bulges),
                    true,
                    concave,
                    da,
                    trust,
                    tol,
                    out,
                    arcs_out,
                    residual,
                );
            }
        }
    }
}

/// Verarbeitet EINEN geschlossenen ∂P-Loop (äußere Kontur oder Loch) einer
/// Kontakt-Komponente: klassifiziert jede GERADE Kante über `edge_on_contour`,
/// synthetisiert die Stufen-Kanten-Fillets samt Konkavitäts-Gate und meldet den
/// ehrlichen B0-Rest an Reflex-Vertices/Bögen. `is_hole` kehrt die
/// In-Ebenen-Außennormale um (bei einem Loch zeigt „weg vom Material" in den
/// Hohlraum).
#[allow(clippy::too_many_arguments)]
fn rim_loop(
    clip: &ContactClip,
    verts: &[[f64; 2]],
    bulges: Option<&[f64]>,
    is_hole: bool,
    concave: bool,
    da: f64,
    trust: f64,
    tol: f64,
    out: &mut Vec<RimFeature>,
    arcs_out: &mut Vec<RimArcFeature>,
    residual: &mut f64,
) {
    let n = verts.len();
    if n < 3 {
        return;
    }
    // Nicht-konvexer Loop (Reflex-Vertex vorhanden): der Trust-Radius der
    // Kanten-Fillets wird auf δ (= Fillet-Radius) begrenzt. Bei einem konvexen
    // P (Box, Zwei-Insel-Rechtecke) darf eine Kante über 3·δ füllen — die
    // Nearest-Feature-Komposition + End-Caps halten das exakt (Over-Fill = 0,
    // Box-Golden byte-identisch). Bei nicht-konvexem P reicht die Slab einer
    // FERNEN Stufen-Kante über 3·δ in die Reflex-Schneise (Kerbe) und würde dort
    // δ−de < 0 als Material fehlinterpretieren (Over-Fill). Mit Trust = δ ist der
    // Beitrag jenseits des Fillet-Radius exakt 0 (δ−δ = 0 an der Trust-Grenze),
    // die Kerbe bleibt ehrlich ungefüllt (Design §5). Box behält 3·δ.
    //
    // Ein KONKAVER Bogen (Zentrum luft-seitig, „Biss" in P) ist derselbe Fall in
    // Bogen-Gestalt: er macht den Loop nicht-konvex, OHNE einen Reflex-Vertex in
    // den Sehnen-Ecken zu hinterlassen — `loop_has_reflex` sieht ihn nicht. Der
    // Bogen selbst wird verweigert (residual), aber die GERADEN Kanten-Slabs
    // würden mit 3·δ in die Biss-Region lecken (z. B. schmales Rechteck mit
    // Bogen-Biss: die Gegenkante erreicht de ∈ (δ, 3·δ] → Over-Fill). Darum
    // zählt er für die Trust-Wahl wie ein Reflex-Vertex.
    let has_reflex = loop_has_reflex(verts);
    let has_concave_arc = bulges.is_some_and(|b| {
        (0..n).any(|k| {
            let bk = b.get(k).copied().unwrap_or(0.0);
            bk.abs() > 1e-12 && arc_is_concave(verts[k], verts[(k + 1) % n], bk, is_hole)
        })
    });
    let edge_trust = if has_reflex || has_concave_arc { da } else { trust };
    let mut steps: Vec<StepEdge> = Vec::new();
    let mut edge_feats: Vec<RimFeature> = Vec::new();
    let mut loop_residual = has_reflex;
    // FC Phase 4 (#398): Bogen-∂P-Kanten (Bulge ≠ 0) werden ZURÜCKGESTELLT und
    // erst NACH dem Konkavitäts-Gate synthetisiert — der Wand-Member kommt aus
    // den homogenen geraden Stufen (ein Bogen-Segment trägt keinen zuverlässigen
    // `edge_on_contour`-Anker gegen die geradlinig projizierten Member-Konturen
    // `la`/`lb`). Element: `(k, u0, u1, bulge)`.
    let mut arc_edges: Vec<(usize, [f64; 2], [f64; 2], f64)> = Vec::new();
    for k in 0..n {
        if let Some(b) = bulges {
            let bk = b.get(k).copied().unwrap_or(0.0);
            if bk.abs() > 1e-12 {
                arc_edges.push((k, verts[k], verts[(k + 1) % n], bk));
                continue;
            }
        }
        let u0 = verts[k];
        let u1 = verts[(k + 1) % n];
        let a3 = lift(clip, u0);
        let b3 = lift(clip, u1);
        // Kante Länge ~0 → überspringen (Degeneration).
        if norm(sub(b3, a3)) < 1e-9 {
            continue;
        }
        let cls = classify_edge(clip, u0, u1, tol);
        let wall = match cls {
            EdgeClass::Step { wall } => wall,
            _ => continue, // bündig / degeneriert → kein Fillet
        };
        // In-Ebenen-Außennormale der CCW-Kante (weg vom P-MATERIAL): äußere
        // Kontur → rechts (dy, −dx); Loch (Material außen) → in den Hohlraum,
        // links (−dy, dx).
        let d2 = [u1[0] - u0[0], u1[1] - u0[1]];
        let out2 = if is_hole {
            nrm2([-d2[1], d2[0]])
        } else {
            nrm2([d2[1], -d2[0]])
        };
        // Über-Stander liefert n_a (dessen SOLID-Kontaktflächen-Außennormale),
        // Wand-Member liefert n_b (SOLID-Seitenwand = sign · In-Ebenen-Außen).
        let (contact_n, wall_sign) = if wall == 1 {
            (clip.n_i, clip.sign_j as f64) // Wand = j
        } else {
            (clip.n_j, clip.sign_i as f64) // Wand = i
        };
        let wall_n = nrm(scl(lift_dir(clip, out2), wall_sign));
        let contact_n = nrm(contact_n);
        let feat = RimFeature {
            a: a3,
            b: b3,
            n_a: contact_n,
            n_b: wall_n,
            n_c: [0.0; 3],
            delta: da,
            trust: edge_trust,
            concave,
        };
        edge_feats.push(feat);
        steps.push(StepEdge {
            wall_member: wall,
            contact_n,
            wall_n,
            v_start: k,
            v_end: (k + 1) % n,
            end_world: b3,
        });
    }

    // Konkavitäts-Gate (#398 Phase 4, per Loop): der additive/subtraktive Fillet
    // ist NUR im ENTHALTENEN Kontakt gültig — dort, wo der Über-Stander die
    // gesamte Kontur des kleineren Members überragt ("A steht überall über B",
    // Design §1.1/§5). Genau dann liegt dieser Loop VOLLSTÄNDIG im Inneren EINER
    // Fläche und ALLE seine ∂P-Stufen-Kanten teilen sich denselben Wand-Member.
    //
    // Bei einem VERSETZTEN (staggered) Kontakt wechseln sich A- und B-Ränder an
    // ∂P ab: an den A-Rändern (Wand = i) tritt eine Fläche des Über-Standers frei
    // zu Tage — die Kante ist dort KONVEX (Stufe nach unten), kein konkaver
    // Sliver. Der (konkave) Fillet würde sie fälschlich auffüllen (Over-Fill bis
    // δ). Gemischte Wand-Member ⇒ Loop komplett verweigert.
    match steps.first().map(|s| s.wall_member) {
        Some(first) => {
            if steps.iter().any(|s| s.wall_member != first) {
                // Gemischt → versetzter Kontakt: die A-Rand-Kanten sind konvexe
                // Stufen (kein konkaver Sliver), der Fillet würde sie fälschlich
                // auffüllen → dieser Loop wird komplett verweigert. ABER: der
                // konkave B0-Sliver an den ECHTEN Stufen-Kanten dieses Loops
                // bleibt ungerundet. Das MUSS die Capability sehen (#398 Phase 4,
                // Multi-Clip-Mixed-Refusal): schließt an DEMSELBEN Stopp ein
                // anderer Loop sauber (residual 0), meldete `synthesize_stop`
                // sonst die enge `tol`-Schranke, obwohl hier ≈0.75·δ offen bleibt.
                // Ehrlicher Rest statt Über-Optimismus.
                *residual = residual.max(0.75 * da);
                return;
            }
        }
        None => {
            // Keine geraden Stufen-Kanten. Gibt es Bogen-Kanten, fehlt der
            // Wand-Member-Anker → reiner Bogen-Loop (z. B. Kreis-Cap-Kontakt):
            // ehrlich verweigern (Design §5, Scope-Grenze — der Bogen-Rim
            // verlangt eine homogene gerade Stufe als Anker). Sonst: bündiger
            // Loop, kein Sliver.
            if !arc_edges.is_empty() {
                *residual = residual.max(0.75 * da);
            }
            return;
        }
    }
    // Homogener Wand-Member steht fest (Gate bestanden).
    let wall = steps[0].wall_member;

    // FC Phase 4 (#398): konvexe Bogen-∂P-Kanten als ChannelArc-Spine-Fillets.
    // Nur bei homogenem Wand-Member (Gate oben); der Bogen erbt diesen Member.
    // KONKAVE Bögen (Material-seitig hohl, Zentrum auf der Luftseite) sind der
    // gekrümmte Reflex-Fall — dort konvergierte der Fillet nach innen und
    // überfüllte für große δ; ehrlich verweigern (Design §5, `residual`).
    let mut arc_feats: Vec<RimArcFeature> = Vec::new();
    for (_k, u0, u1, bulge) in &arc_edges {
        match build_arc_feature(clip, *u0, *u1, *bulge, is_hole, wall, concave, da, edge_trust) {
            Some(f) => arc_feats.push(f),
            None => loop_residual = true, // konkaver/entarteter Bogen → ehrlicher Rest
        }
    }

    // Reflex-(Konkav-)Vertices eines nicht-konvexen ∂P bleiben bewusst
    // UNGERUNDET (Design §5, `has_reflex` oben): dort verwöben Kontakt-Rim und
    // intrinsische konkave Körperkante (SSI-Naht) zum Trieder-Misch — die
    // #395-Stufe-2-Klemme. Die adjazenten Kanten-Fillets enden an ihren
    // End-Caps, die Vertex-Umgebung bleibt auf dem Prisma-Wert → lokaler
    // B0-Unterschuss (≈0.75·δ), NIE Überschuss. Ehrlich melden statt falsch
    // runden (auch für verweigerte Bogen-Stufen, `loop_residual`).
    if loop_residual {
        *residual = residual.max(0.75 * da);
    }
    out.extend(edge_feats);
    arcs_out.extend(arc_feats);
    synthesize_vertices(&steps, da, edge_trust, concave, out);
}

/// Zentrum + Radius eines cavalier-Bulge-Bogens `u0→u1` (die
/// `seg_arc_radius_and_center`-Formel). `None` bei entarteter Sehne/Bulge.
fn arc_center_radius(u0: [f64; 2], u1: [f64; 2], bulge: f64) -> Option<([f64; 2], f64)> {
    let chord = [u1[0] - u0[0], u1[1] - u0[1]];
    let clen = (chord[0] * chord[0] + chord[1] * chord[1]).sqrt();
    if clen < 1e-9 {
        return None;
    }
    let ab = bulge.abs();
    if ab < 1e-9 {
        return None;
    }
    let radius = clen * (ab * ab + 1.0) / (4.0 * ab);
    let s_sag = ab * clen / 2.0;
    let m = radius - s_sag;
    let (mut ox, mut oy) = (-m * chord[1] / clen, m * chord[0] / clen);
    if bulge < 0.0 {
        ox = -ox;
        oy = -oy;
    }
    Some(([u0[0] + chord[0] / 2.0 + ox, u0[1] + chord[1] / 2.0 + oy], radius))
}

/// Ist der Bogen `u0→u1` (Bulge ≠ 0) KONKAV — Zentrum auf der Luftseite, also
/// ein „Biss" in P (gekrümmter Reflex)? In-Ebenen-Außennormale out2 (weg vom
/// P-Material) wie bei geraden Kanten; liegt das Zentrum auf der MATERIAL-Seite
/// (out2·(c−mid) < 0), krümmt der Bogen VON der Kerbe weg → konvex → baubar.
/// Entartete Bögen zählen als konkav (werden ohnehin verweigert).
fn arc_is_concave(u0: [f64; 2], u1: [f64; 2], bulge: f64, is_hole: bool) -> bool {
    let Some((center2d, _)) = arc_center_radius(u0, u1, bulge) else {
        return true;
    };
    let chord = [u1[0] - u0[0], u1[1] - u0[1]];
    let out2 = if is_hole {
        nrm2([-chord[1], chord[0]])
    } else {
        nrm2([chord[1], -chord[0]])
    };
    let mid = [0.5 * (u0[0] + u1[0]), 0.5 * (u0[1] + u1[1])];
    let to_center = [center2d[0] - mid[0], center2d[1] - mid[1]];
    out2[0] * to_center[0] + out2[1] * to_center[1] >= -1e-9
}

/// Baut das Bogen-Rand-Fillet EINER ∂P-Bogen-Kante (`u0→u1`, cavalier-Bulge
/// `bulge`) im homogenen Wand-Member `wall`. `None`, wenn der Bogen KONKAV ist
/// (Zentrum auf der Luftseite → gekrümmter Reflex, Over-Fill-Gefahr) oder
/// entartet — dann bleibt der ehrliche B0-Rest (`residual`) stehen (Design §5).
#[allow(clippy::too_many_arguments)]
fn build_arc_feature(
    clip: &ContactClip,
    u0: [f64; 2],
    u1: [f64; 2],
    bulge: f64,
    is_hole: bool,
    wall: u8,
    concave: bool,
    da: f64,
    edge_trust: f64,
) -> Option<RimArcFeature> {
    let (center2d, radius) = arc_center_radius(u0, u1, bulge)?;
    if arc_is_concave(u0, u1, bulge, is_hole) {
        return None; // Zentrum auf Luftseite → konkaver Bogen → verweigern
    }

    // Welt-Rahmen: c = Zentrum, u = Zentrum→Bogenanfang, v = sign(bulge)·(n̂×u),
    // sweep = |4·atan(bulge)| (Bogenpunkt c + r·(u·cosφ + v·sinφ), φ∈[0,sweep]).
    let c_w = lift(clip, center2d);
    let u_w = nrm(lift_dir(clip, [u0[0] - center2d[0], u0[1] - center2d[1]]));
    let ncc = nrm(cross(clip.n, u_w));
    let v_w = if bulge >= 0.0 { ncc } else { [-ncc[0], -ncc[1], -ncc[2]] };
    let sweep = 4.0 * bulge.abs().atan();

    // Über-Stander liefert n_a; der Wand-Member liefert das Vorzeichen der
    // SOLID-Radialnormale (Erosion +1, Dilation −1 — wie beim geraden `wall_n`).
    let (contact_n, member_sign) = if wall == 1 {
        (clip.n_i, clip.sign_j as f64)
    } else {
        (clip.n_j, clip.sign_i as f64)
    };
    Some(RimArcFeature {
        c: c_w,
        u: u_w,
        v: v_w,
        n_a: nrm(contact_n),
        n_b: [0.0; 3],
        r: radius,
        sweep,
        wall_sign: member_sign,
        delta: da,
        trust: edge_trust,
        concave,
    })
}

/// Hat der geschlossene Polygon-Loop mindestens einen Reflex-(nicht-konvexen)
/// Vertex? Orientierungs-unabhängig über einen Vorzeichenwechsel der
/// aufeinanderfolgenden Kanten-Kreuzprodukte (konvex ⇒ alle gleiches Vorzeichen).
/// Kollineare Ecken (Kreuz ≈ 0) zählen nicht. Ein Dreieck ist stets konvex.
fn loop_has_reflex(verts: &[[f64; 2]]) -> bool {
    let n = verts.len();
    if n < 4 {
        return false;
    }
    let mut sign = 0i32;
    for k in 0..n {
        let p = verts[(k + n - 1) % n];
        let q = verts[k];
        let r = verts[(k + 1) % n];
        let cr = (q[0] - p[0]) * (r[1] - q[1]) - (q[1] - p[1]) * (r[0] - q[0]);
        if cr.abs() < 1e-12 {
            continue;
        }
        let s = if cr > 0.0 { 1 } else { -1 };
        if sign == 0 {
            sign = s;
        } else if sign != s {
            return true;
        }
    }
    false
}

/// Trihedraler Reflex-Vertex: treffen an einer ∂P-Ecke zwei Stufen-Kanten mit
/// Seitenwänden aus VERSCHIEDENEN Membern (nicht-parallel) zusammen, bildet die
/// Ecke einen echten 3-Wand-Reflex → Punkt-Fillet mit `n_a`/`n_b` = die zwei
/// Seitenwände, `n_c` = die gemeinsame Kontaktfläche. Gehören beide Wände
/// DEMSELBEN Member (konvexe Box-Ecke), rundet die Nearest-Feature-Komposition
/// die Ecke ohne Extra-Term (Design §1.3/§2.4) → kein Vertex.
fn synthesize_vertices(
    steps: &[StepEdge],
    da: f64,
    trust: f64,
    concave: bool,
    out: &mut Vec<RimFeature>,
) {
    for (i, e0) in steps.iter().enumerate() {
        for e1 in steps.iter().skip(i + 1) {
            // Teilen sich die beiden Kanten einen ∂P-Vertex?
            let shared = if e0.v_end == e1.v_start {
                Some(e0.end_world)
            } else if e1.v_end == e0.v_start {
                Some(e1.end_world)
            } else {
                None
            };
            let Some(v) = shared else { continue };
            // Gleiche Wand-Member (konvexe Box-Ecke) → Nearest-Feature genügt.
            if e0.wall_member == e1.wall_member {
                continue;
            }
            // Parallele Wände → keine echte Trieder-Ecke.
            if dot(e0.wall_n, e1.wall_n).abs() > 0.99 {
                continue;
            }
            out.push(RimFeature {
                a: v,
                b: v,
                n_a: e0.wall_n,
                n_b: e1.wall_n,
                n_c: nrm(e0.contact_n),
                delta: da,
                trust,
                concave,
            });
        }
    }
}

#[inline]
fn nrm2(a: [f64; 2]) -> [f64; 2] {
    let n = (a[0] * a[0] + a[1] * a[1]).sqrt().max(1e-30);
    [a[0] / n, a[1] / n]
}

/// Leitet die ∂P-Rand-Fillet-Features eines Stopp-Kind-Baums ab
/// (`delta_signed < 0` Erosion/konkav, `> 0` Dilation/konvex). `inner` ist das
/// Kind des Offset-Stopps (Union bei Erosion, Intersection/Difference bei
/// Dilation) in Welt-Koordinaten-`BoundedCsg`-Form. Leere Liste = keine
/// koplanaren Box-Kontakte oder vollständige Verweigerung.
pub fn synthesize_rim_features(inner: &BoundedCsg, delta_signed: f32) -> Vec<RimFeature> {
    synth_rim_features(inner, delta_signed).0
}

/// Wie [`synthesize_rim_features`], aber die konvexen BOGEN-∂P-Fillets (FC
/// Phase 4, #398). Leere Liste = keine konvexen Bogen-Kontakte.
pub fn synthesize_rim_arc_features(inner: &BoundedCsg, delta_signed: f32) -> Vec<RimArcFeature> {
    synth_rim_features(inner, delta_signed).1
}

/// Wie [`synthesize_rim_features`], liefert zusätzlich den ehrlichen B0-Rest
/// (≈0.75·δ) an ungerundeten ∂P-Reflex-Vertices/Bögen (`0.0` bei konvexem P wie
/// Box-Box). Anker für [`RimBlock::from_stop`] und die Capability-Meldung.
pub fn synth_rim_features(
    inner: &BoundedCsg,
    delta_signed: f32,
) -> (Vec<RimFeature>, Vec<RimArcFeature>, f64) {
    let ds = delta_signed as f64;
    let mut out = Vec::new();
    let mut arcs = Vec::new();
    let mut residual = 0.0f64;
    for clip in contact_clips(inner, ds) {
        rims_from_clip(&clip, ds, &mut out, &mut arcs, &mut residual);
    }
    (out, arcs, residual)
}

/// Läuft den ganzen (gefalteten) `BoundedCsg`-Baum ab und sammelt die Rand-
/// Fillet-Features aller dualen Offset-Stopps (`Union+(−δ)`,
/// `Intersection/Difference+(+δ)`) — Spiegel des Stopp-Kriteriums in
/// [`crate::offset_channel::synthesize_offset_channels_reported`].
pub fn synthesize_rim_features_for_tree(root: &BoundedCsg) -> Vec<RimFeature> {
    let mut out = Vec::new();
    walk(root, &mut out);
    out
}

fn walk(node: &BoundedCsg, out: &mut Vec<RimFeature>) {
    match node {
        BoundedCsg::Offset { inner, delta, .. } => {
            let dual = matches!(
                (&**inner, *delta),
                (BoundedCsg::Union { .. }, d) if d < 0.0
            ) || matches!(
                (&**inner, *delta),
                (BoundedCsg::Intersection { .. }, d) if d > 0.0
            ) || matches!(
                (&**inner, *delta),
                (BoundedCsg::Difference { .. }, d) if d > 0.0
            );
            if dual && *delta != 0.0 && delta.is_finite() {
                out.extend(synthesize_rim_features(inner, *delta));
            }
            walk(inner, out);
        }
        BoundedCsg::Union { left, right, .. }
        | BoundedCsg::Intersection { left, right, .. }
        | BoundedCsg::Difference { left, right, .. } => {
            walk(left, out);
            walk(right, out);
        }
        BoundedCsg::Shell { inner, .. } => walk(inner, out),
        _ => {}
    }
}

/// Wie [`synthesize_rim_features_for_tree`], aber die konvexen BOGEN-∂P-Fillets
/// (FC Phase 4, #398) — läuft dieselben dualen Stopps ab.
pub fn synthesize_rim_arc_features_for_tree(root: &BoundedCsg) -> Vec<RimArcFeature> {
    let mut out = Vec::new();
    walk_arc(root, &mut out);
    out
}

fn walk_arc(node: &BoundedCsg, out: &mut Vec<RimArcFeature>) {
    match node {
        BoundedCsg::Offset { inner, delta, .. } => {
            let dual = matches!((&**inner, *delta), (BoundedCsg::Union { .. }, d) if d < 0.0)
                || matches!((&**inner, *delta), (BoundedCsg::Intersection { .. }, d) if d > 0.0)
                || matches!((&**inner, *delta), (BoundedCsg::Difference { .. }, d) if d > 0.0);
            if dual && *delta != 0.0 && delta.is_finite() {
                out.extend(synthesize_rim_arc_features(inner, *delta));
            }
            walk_arc(inner, out);
        }
        BoundedCsg::Union { left, right, .. }
        | BoundedCsg::Intersection { left, right, .. }
        | BoundedCsg::Difference { left, right, .. } => {
            walk_arc(left, out);
            walk_arc(right, out);
        }
        BoundedCsg::Shell { inner, .. } => walk_arc(inner, out),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test-Wrapper: die geraden Rim-Features EINES Clips (Bögen verworfen) —
    /// die meisten Unit-Tests prüfen nur den geraden Pfad.
    fn rims_from_clip_l(clip: &ContactClip, ds: f64, out: &mut Vec<RimFeature>, residual: &mut f64) {
        let mut _arcs = Vec::new();
        rims_from_clip(clip, ds, out, &mut _arcs, residual);
    }

    fn clip_axis(concave: bool) -> ContactClip {
        // Box-Stack-artiger Verschnitt in der x=10-Ebene: la (großer A-Deckel
        // 20×20) ⊃ lb (kleiner B-Querschnitt 14×12). Frame e1,e2 wie
        // plane_frame([1,0,0]): e1=(0,0,-1), e2=(0,1,0), n=(1,0,0).
        let e1 = [0.0, 0.0, -1.0];
        let e2 = [0.0, 1.0, 0.0];
        let n = [1.0, 0.0, 0.0];
        let o = 10.0;
        // la = A-Fläche (u=−z∈[−10,10], v=y∈[−10,10]), CCW.
        let la = vec![[-10.0, -10.0], [10.0, -10.0], [10.0, 10.0], [-10.0, 10.0]];
        // lb = P = B-Fläche (u∈[−6,6], v∈[−7,7]), CCW.
        let lb = vec![[-6.0, -7.0], [6.0, -7.0], [6.0, 7.0], [-6.0, 7.0]];
        let p2d = lb.clone();
        let sign = 1.0f32;
        // Solid-Außennormalen: A auf −n (Material x≤10) → n_i=+x; B auf +n
        // (Material x≥10) → n_j=−x. Für Dilation kehrt sign_j das Wandvorzeichen.
        ContactClip {
            n,
            e1,
            e2,
            o,
            p2d,
            la,
            lb,
            n_i: [1.0, 0.0, 0.0],
            n_j: [-1.0, 0.0, 0.0],
            sign_i: sign,
            sign_j: if concave { 1.0 } else { -1.0 },
            components: Vec::new(),
        }
    }

    #[test]
    fn classify_step_vs_flush() {
        let clip = clip_axis(true);
        // Alle vier ∂P-Kanten (= B-Rand, IM Inneren von A) sind Stufen mit
        // Wand-Member j.
        let n = clip.p2d.len();
        for k in 0..n {
            let a = clip.p2d[k];
            let b = clip.p2d[(k + 1) % n];
            assert_eq!(
                classify_edge(&clip, a, b, 1e-4),
                EdgeClass::Step { wall: 1 },
                "Kante {k} sollte Stufe (Wand=j) sein"
            );
        }
        // Eine Kante, die auf A's Rand UND B's Rand läge (bündig), z. B. wenn
        // lb bis an la's Rand reicht: konstruiere eine koinzidente Kante.
        let mut flush = clip_axis(true);
        // lb-Oberkante v=10 = la-Oberkante v=10 → bündig.
        flush.lb = vec![[-6.0, -7.0], [6.0, -7.0], [6.0, 10.0], [-6.0, 10.0]];
        flush.p2d = flush.lb.clone();
        let np = flush.p2d.len();
        let mut saw_flush = false;
        for k in 0..np {
            let a = flush.p2d[k];
            let b = flush.p2d[(k + 1) % np];
            if classify_edge(&flush, a, b, 1e-4) == EdgeClass::Flush {
                saw_flush = true;
            }
        }
        assert!(saw_flush, "koinzidente Oberkante sollte als bündig erkannt werden");
    }

    #[test]
    fn concave_normals_point_into_material() {
        let clip = clip_axis(true);
        let mut rims = Vec::new();
        rims_from_clip_l(&clip, -1.0, &mut rims, &mut 0.0);
        assert_eq!(rims.len(), 4, "vier Stufen-Kanten, keine Trieder-Vertizes");
        for r in &rims {
            assert!(r.concave);
            assert_eq!(r.n_c, [0.0; 3]);
            // n_a = A-Deckel-Außennormale = +x (zeigt in die Luft über Π).
            assert!((r.n_a[0] - 1.0).abs() < 1e-9, "n_a = +x erwartet, got {:?}", r.n_a);
            // n_b = B-Seitenwand-Außennormale, rein in-plane (x-Komponente 0).
            assert!(r.n_b[0].abs() < 1e-9, "n_b in-plane erwartet, got {:?}", r.n_b);
            assert!((norm(r.n_b) - 1.0).abs() < 1e-9);
        }
        // Die +v-Kante (B-Oberkante y=+7) muss n_b = +y tragen (Material y≤7).
        let top = rims
            .iter()
            .find(|r| (r.a[1] - 7.0).abs() < 1e-9 && (r.b[1] - 7.0).abs() < 1e-9)
            .expect("Oberkante y=7 vorhanden");
        assert!((top.n_b[1] - 1.0).abs() < 1e-9, "Oberkanten-n_b = +y, got {:?}", top.n_b);
    }

    #[test]
    fn convex_flips_wall_sign() {
        // Dilation: sign_j = −1 kehrt die Seitenwand-Normale ins Luft-Vorzeichen.
        let clip = clip_axis(false);
        let mut rims = Vec::new();
        rims_from_clip_l(&clip, 1.0, &mut rims, &mut 0.0);
        assert_eq!(rims.len(), 4);
        let top = rims
            .iter()
            .find(|r| (r.a[1] - 7.0).abs() < 1e-9 && (r.b[1] - 7.0).abs() < 1e-9)
            .expect("Oberkante vorhanden");
        assert!(!top.concave);
        // Bei Dilation kehrt das Wand-Vorzeichen: Oberkante n_b = −y.
        assert!((top.n_b[1] + 1.0).abs() < 1e-9, "Dilation-Oberkanten-n_b = −y, got {:?}", top.n_b);
        // n_a bleibt die SOLID-Kontaktnormale des Über-Standers (+x).
        assert!((top.n_a[0] - 1.0).abs() < 1e-9);
    }

    #[test]
    fn no_trihedral_for_same_wall_member() {
        // Box-Stack: alle vier Kanten Wand-Member j → konvexe Box-Ecken, kein
        // Vertex-Feature.
        let clip = clip_axis(true);
        let mut rims = Vec::new();
        rims_from_clip_l(&clip, -1.0, &mut rims, &mut 0.0);
        assert!(
            rims.iter().all(|r| r.a != r.b),
            "keine Punkt-Fillets (a==b) beim Box-Stack erwartet"
        );
    }

    #[test]
    fn trihedral_for_distinct_wall_members() {
        // Konstruiere zwei benachbarte Stufen-Kanten mit Wänden aus
        // VERSCHIEDENEN Membern (nicht-parallel) → ein Vertex-Feature.
        let v = [10.0, 7.0, 6.0];
        let steps = vec![
            StepEdge {
                wall_member: 0,
                contact_n: [1.0, 0.0, 0.0],
                wall_n: [0.0, 1.0, 0.0],
                v_start: 0,
                v_end: 1,
                end_world: v,
            },
            StepEdge {
                wall_member: 1,
                contact_n: [1.0, 0.0, 0.0],
                wall_n: [0.0, 0.0, 1.0],
                v_start: 1,
                v_end: 2,
                end_world: [0.0, 0.0, 0.0],
            },
        ];
        let mut out = Vec::new();
        synthesize_vertices(&steps, 1.0, 3.0, true, &mut out);
        assert_eq!(out.len(), 1, "genau ein Trieder-Vertex");
        let vx = out[0];
        assert_eq!(vx.a, v);
        assert_eq!(vx.a, vx.b);
        assert!((norm(vx.n_a) - 1.0).abs() < 1e-9);
        assert!((norm(vx.n_b) - 1.0).abs() < 1e-9);
        assert!((norm(vx.n_c) - 1.0).abs() < 1e-9, "n_c (Kontaktwand) gesetzt");
    }

    // ─── Phase 2: f32-Opcode-Parität ───────────────────────────────────────

    use crate::sdf_compiler::{encode_rim_block, eval_opcodes, OP_PRIM_CONST};

    /// Baut ein Test-Tape `[OP_PRIM_CONST g] ++ OP_RIM_FILLET-Block` und wertet
    /// es aus (der Rim-Opcode pops g, pusht die Korrektur). KEIN Regen-Pfad.
    fn eval_rim_tape(g: f32, p: [f32; 3], concave: bool, recs: &[RimRecord]) -> f32 {
        let mut tape = vec![f32::from_bits(OP_PRIM_CONST), g];
        encode_rim_block(concave, recs, &mut tape);
        eval_opcodes(&tape, p)
    }

    struct Lcg(u64);
    impl Lcg {
        fn f(&mut self) -> f32 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1);
            ((self.0 >> 33) as f32) / (1u64 << 31) as f32
        }
        fn r(&mut self, lo: f32, hi: f32) -> f32 {
            lo + (hi - lo) * self.f()
        }
    }

    #[test]
    fn rim_opcode_matches_f32_reference() {
        // Der eval_opcodes-Decode ist bit-identisch zu `apply_rims_f32`.
        let clip = clip_axis(true);
        let mut rims64 = Vec::new();
        rims_from_clip_l(&clip, -1.5, &mut rims64, &mut 0.0);
        let recs: Vec<RimRecord> = rims64.iter().map(RimRecord::from_feature).collect();
        assert!(recs.len() >= 4);
        let mut rng = Lcg(0x1234_5678);
        for _ in 0..5000 {
            let p = [rng.r(2.0, 18.0), rng.r(-12.0, 12.0), rng.r(-12.0, 12.0)];
            let g = rng.r(-5.0, 5.0);
            let want = apply_rims_f32(g, p, true, &recs);
            let got = eval_rim_tape(g, p, true, &recs);
            assert_eq!(
                got.to_bits(),
                want.to_bits(),
                "Opcode ≠ apply_rims_f32 bei p={p:?} g={g}: {got} vs {want}"
            );
        }
    }

    #[test]
    fn rim_f32_matches_f64_reference() {
        // f32-Referenz ~ f64-Vertrag `apply_rims` im ∂P-Band (f32-Toleranz).
        for concave in [true, false] {
            let clip = clip_axis(concave);
            let mut rims64 = Vec::new();
            let ds = if concave { -2.0 } else { 2.0 };
            rims_from_clip_l(&clip, ds, &mut rims64, &mut 0.0);
            let recs: Vec<RimRecord> = rims64.iter().map(RimRecord::from_feature).collect();
            let mut rng = Lcg(if concave { 0xAAAA } else { 0xBBBB });
            let mut max_diff = 0.0f32;
            for _ in 0..20000 {
                // Punkte ums Kontaktband x≈10, y∈[−9,9], z∈[−8,8].
                let p64 = [
                    rng.r(6.0, 13.0) as f64,
                    rng.r(-9.0, 9.0) as f64,
                    rng.r(-8.0, 8.0) as f64,
                ];
                let p = [p64[0] as f32, p64[1] as f32, p64[2] as f32];
                let g = rng.r(-4.0, 4.0);
                let want = apply_rims(g as f64, p64, &rims64) as f32;
                let got = apply_rims_f32(g, p, concave, &recs);
                max_diff = max_diff.max((got - want).abs());
                // Nie über-/unterfüllen: der Opcode-Wert darf die Referenz feld-
                // seitig nicht unterschreiten (konkav) bzw. überschreiten
                // (konvex) — über die f32-Rundung (~1e-3 bei Szenengröße ≤32 mm)
                // hinaus. Das ist die harte Sicherheits-Invariante.
                if concave {
                    assert!(
                        got >= want - 3e-3,
                        "konkav underfill: got {got} < want {want} bei p={p:?}"
                    );
                } else {
                    assert!(
                        got <= want + 3e-3,
                        "konvex overshoot: got {got} > want {want} bei p={p:?}"
                    );
                }
            }
            assert!(
                max_diff < 3e-3,
                "f32↔f64-Parität ({}) verletzt: max |Δ| = {max_diff:.3e}",
                if concave { "konkav" } else { "konvex" }
            );
        }
    }

    #[test]
    fn rim_nearest_feature_rounds_corner() {
        // Box-Stack-Ecke: zwei benachbarte ∂P-Kanten. An einem Punkt mit
        // de1 ≠ de2 MUSS die Nearest-Feature-Auswahl das NÄCHSTE Feature nehmen
        // (δ − min de). Sequenzielles min über beide Terme = δ − max de wäre ein
        // Fehler (Über-Fill) — der Assert stellt die Rundung sicher.
        let clip = clip_axis(true);
        let mut rims64 = Vec::new();
        rims_from_clip_l(&clip, -1.0, &mut rims64, &mut 0.0);
        let recs: Vec<RimRecord> = rims64.iter().map(RimRecord::from_feature).collect();
        // Punkt im 3. Quadranten der (v=7,x=10)- und (u=6→z=−6,x=10)-Ecke.
        let p = [9.7f32, 6.5, -5.6];
        // Alle maskierten (de, corr = δ − de) sammeln.
        let mut masked: Vec<(f32, f32)> = Vec::new();
        for r in &rims64 {
            if let Some(v) = r.value([p[0] as f64, p[1] as f64, p[2] as f64]) {
                masked.push((r.closest([p[0] as f64, p[1] as f64, p[2] as f64]).0 as f32, v as f32));
            }
        }
        assert!(masked.len() >= 2, "Ecke braucht ≥2 maskierte Kanten, hat {}", masked.len());
        let nearest = masked.iter().cloned().fold((f32::INFINITY, 0.0), |acc, x| if x.0 < acc.0 { x } else { acc });
        let seq_min = masked.iter().map(|&(_, c)| c).fold(f32::INFINITY, f32::min);
        // Die Nearest-Distanzen der zwei Ecken-Kanten sind verschieden →
        // Nearest-corr ≠ sequenzielles min.
        assert!(nearest.1 > seq_min + 0.01, "Ecke degeneriert (min==max): nearest={:?} seq={}", nearest, seq_min);
        // Große Basis g ⇒ min(g, corr) = corr: der Opcode nimmt die NÄCHSTE.
        let got = eval_rim_tape(100.0, p, true, &recs);
        assert!(
            (got - nearest.1).abs() < 1e-4,
            "Nearest-Feature verletzt: got {got}, erwartet {} (nicht seq-min {seq_min})",
            nearest.1
        );
    }

    #[test]
    fn rim_empty_block_is_neutral() {
        let p = [3.0f32, 4.0, 5.0];
        for concave in [true, false] {
            assert_eq!(apply_rims_f32(1.234, p, concave, &[]), 1.234);
            let got = eval_rim_tape(1.234, p, concave, &[]);
            assert_eq!(got.to_bits(), 1.234f32.to_bits(), "leerer Block muss neutral sein");
        }
    }

    #[test]
    fn degenerate_empty_and_zero_length() {
        // Leeres P → keine Features.
        let mut clip = clip_axis(true);
        clip.p2d = vec![[0.0, 0.0], [1.0, 0.0]]; // < 3 Ecken
        let mut rims = Vec::new();
        rims_from_clip_l(&clip, -1.0, &mut rims, &mut 0.0);
        assert!(rims.is_empty(), "P < 3 Ecken → keine Features");
        // δ = 0 → keine Features.
        let clip2 = clip_axis(true);
        let mut rims2 = Vec::new();
        rims_from_clip_l(&clip2, 0.0, &mut rims2, &mut 0.0);
        assert!(rims2.is_empty(), "δ=0 → keine Features");
    }

    // ─── FC Phase 4 (#398): Multi-Clip-Mixed-Refusal (Bound-Over-Claim) ─────

    /// Ein VERSETZTER (staggered) Kontakt: die ∂P-Ränder stammen ABWECHSELND
    /// von Member i (Über-Stander A) und j (B). Das Konkavitäts-Gate verweigert
    /// den ganzen Loop (gemischte `wall_member`) — richtig, denn die A-Rand-
    /// Kanten sind konvexe Stufen. Der Fix (#398 P4): die Verweigerung MUSS den
    /// ehrlichen B0-Rest (≈0.75·δ) an den ECHTEN Stufen-Kanten melden, sonst
    /// meldete `synthesize_stop` an einem Stopp mit einem daneben SAUBER
    /// schließenden Loop fälschlich die enge `tol`-Schranke.
    fn clip_staggered() -> ContactClip {
        let mut c = clip_axis(true);
        // lb ragt bei v=+12 über la (v=10) hinaus → P wird oben von la (A)
        // geklemmt: die Oberkante v=10 ist A-Rand (Wand=i), die anderen drei
        // sind B-Rand (Wand=j) → gemischt.
        c.lb = vec![[-6.0, -7.0], [6.0, -7.0], [6.0, 12.0], [-6.0, 12.0]];
        c.p2d = vec![[-6.0, -7.0], [6.0, -7.0], [6.0, 10.0], [-6.0, 10.0]];
        c
    }

    #[test]
    fn mixed_refusal_reports_residual() {
        // (a) Der versetzte Loop verweigert (keine Features) …
        let clip = clip_staggered();
        let mut out = Vec::new();
        let mut residual = 0.0f64;
        rims_from_clip_l(&clip, -2.0, &mut out, &mut residual);
        assert!(out.is_empty(), "versetzter Kontakt → kein Fillet");
        // … meldet aber den ehrlichen B0-Rest (≈0.75·δ, δ=2).
        assert!(
            (residual - 0.75 * 2.0).abs() < 1e-9,
            "Mixed-Refusal muss residual ≈ 0.75·δ melden, got {residual}"
        );
        // (b) Ein sauberer (homogener) Loop meldet KEINEN Rest.
        let clean = clip_axis(true);
        let mut out2 = Vec::new();
        let mut res2 = 0.0f64;
        rims_from_clip_l(&clean, -2.0, &mut out2, &mut res2);
        assert_eq!(out2.len(), 4, "homogener Box-Stack → vier Kanten");
        assert_eq!(res2, 0.0, "homogener Loop → kein B0-Rest");
        // (c) Aggregat über BEIDE (Multi-Clip): der Rest dominiert die Schranke.
        let mut all = Vec::new();
        let mut agg = 0.0f64;
        rims_from_clip_l(&clean, -2.0, &mut all, &mut agg);
        rims_from_clip_l(&clip, -2.0, &mut all, &mut agg);
        assert_eq!(all.len(), 4, "nur der saubere Loop trägt Features");
        assert!(agg > 0.0, "der verweigerte Loop hebt die aggregierte Schranke");
    }

    // ─── FC Phase 3/4 (#398): Hole-Kontur-Rim-Fixture ──────────────────────

    /// Direkt konstruierter Kontakt MIT LOCH (die `components`-Hole-Maschinerie
    /// aus Phase 3 ist im Regen-Pfad schlummernd — `member_contact_faces`
    /// verweigert zusammengesetzte Profile). P = großes Quadrat mit quadratischem
    /// Innenloch (Kontakt-Annulus): A (i) überragt alles, B (j) trägt Außen- UND
    /// Innenwand. Nur der HOLE-Loop erzeugt hier Fillets (die Außenkontur liegt
    /// weder auf A- noch auf B-Rand → degeneriert übersprungen). Der Test
    /// verifiziert die INVERTIERTE In-Ebenen-Normale (Design §5: bei einem Loch
    /// zeigt „weg vom Material" in den Hohlraum).
    fn clip_with_hole() -> ContactClip {
        use crate::offset_channel::{ClipComponent, Contour2d};
        let mut c = clip_axis(true);
        // la = A-Fläche [-10,10]²; lb = die HOLE-Randlinie (damit die Loch-
        // Kanten als Wand=j klassifizieren — edge_on_contour testet Kollinearität
        // mit lb's Kantengeraden).
        c.la = vec![[-10.0, -10.0], [10.0, -10.0], [10.0, 10.0], [-10.0, 10.0]];
        c.lb = vec![[-3.0, -3.0], [3.0, -3.0], [3.0, 3.0], [-3.0, 3.0]];
        // Komponente: Außenrand P = [-8,8]², ein Loch = [-3,3]² (beide CCW).
        let outer = Contour2d {
            verts: vec![[-8.0, -8.0], [8.0, -8.0], [8.0, 8.0], [-8.0, 8.0]],
            bulges: vec![0.0; 4],
        };
        let hole = Contour2d {
            verts: vec![[-3.0, -3.0], [3.0, -3.0], [3.0, 3.0], [-3.0, 3.0]],
            bulges: vec![0.0; 4],
        };
        c.p2d = outer.verts.clone();
        c.components = vec![ClipComponent { outer, holes: vec![hole] }];
        c
    }

    #[test]
    fn hole_rim_orients_into_cavity() {
        let clip = clip_with_hole();
        let mut rims = Vec::new();
        let mut residual = 0.0f64;
        rims_from_clip_l(&clip, -2.0, &mut rims, &mut residual);
        // Vier Loch-Kanten, keine Trieder (homogener Wand-Member j).
        assert_eq!(rims.len(), 4, "vier Hole-Stufen-Kanten erwartet, got {}", rims.len());
        // Loch-Zentrum in Welt: u=v=0 → lift(0,0) = o·n = [10,0,0].
        let hole_center = [10.0, 0.0, 0.0];
        for r in &rims {
            assert!(r.concave);
            // n_a = A-Kontaktnormale = +x (Über-Stander i).
            assert!((r.n_a[0] - 1.0).abs() < 1e-9, "n_a = +x, got {:?}", r.n_a);
            // n_b (Wand-Außennormale) ist rein in-plane …
            assert!(r.n_b[0].abs() < 1e-9, "n_b in-plane, got {:?}", r.n_b);
            // … und zeigt IN DEN HOHLRAUM (zum Loch-Zentrum): der Kanten-Mittel-
            // punkt + ε·n_b liegt NÄHER am Zentrum als der Mittelpunkt selbst.
            let mid = [
                0.5 * (r.a[0] + r.b[0]),
                0.5 * (r.a[1] + r.b[1]),
                0.5 * (r.a[2] + r.b[2]),
            ];
            let to_center = sub(hole_center, mid);
            assert!(
                dot(r.n_b, to_center) > 0.0,
                "Hole-n_b muss in den Hohlraum (zum Zentrum) zeigen: n_b={:?} mid={:?}",
                r.n_b, mid
            );
        }

        // Grid-Scan: KEIN Over-Fill in den Hohlraum. Konservative Ground-Truth:
        // ein Punkt AUSSERHALB der (un-erodierten) Union A∪B kann nach Erosion
        // niemals Material sein → das korrigierte Feld darf dort nicht < 0
        // behaupten. A: x≤10 (SDF x−10); B-Annulus: x∈[10,12] ∩ (P∖Loch).
        let da = 2.0f64;
        let sdf_a = |p: [f64; 3]| p[0] - 10.0;
        // 2D-Quadrat-SDF in (u,v) = (−z, y).
        let sq = |u: f64, v: f64, h: f64| {
            let dx = u.abs() - h;
            let dy = v.abs() - h;
            let outside = (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt();
            outside + dx.max(dy).min(0.0)
        };
        let sdf_b = |p: [f64; 3]| {
            let (u, v) = (-p[2], p[1]);
            let annulus = sq(u, v, 8.0).max(-sq(u, v, 3.0)); // groß ∖ Loch
            let slab = (10.0 - p[0]).max(p[0] - 12.0);
            annulus.max(slab)
        };
        let mut over = 0.0f64;
        let g = 24;
        for i in 0..=g {
            for j in 0..=g {
                for k in 0..=g {
                    let p = [
                        8.0 + 6.0 * i as f64 / g as f64,   // x∈[8,14] um Π=10
                        -6.0 + 12.0 * j as f64 / g as f64, // y∈[−6,6]
                        -6.0 + 12.0 * k as f64 / g as f64, // z∈[−6,6]
                    ];
                    let u = sdf_a(p).min(sdf_b(p)); // Union
                    let naive = u + da; // Erosion −δ ⇒ naiv f+δ
                    let corr = apply_rims(naive, p, &rims);
                    // Außerhalb der un-erodierten Union → darf nie Material sein.
                    if u > 0.05 && corr < -1e-6 {
                        over = over.max(-corr);
                    }
                }
            }
        }
        assert!(over < 1e-3, "Hole-Rim überfüllt in den Hohlraum: {over:.4} mm");

        // Fill: an einem Punkt in der konkaven Loch-Schneise (Annulus-Material,
        // dicht an der Loch-Wand u=3, unter Π) ist der Rim aktiv und SENKT den
        // naiven Wert (füllt den B0-Sliver).
        let p = [9.4, 0.0, -3.4]; // x<10 (in A), u=3.4 knapp außerhalb Loch → Annulus
        let u = sdf_a(p).min(sdf_b(p));
        let naive = u + da;
        let corr = apply_rims(naive, p, &rims);
        assert!(corr < naive, "Hole-Rim muss den Sliver füllen: corr {corr} !< naiv {naive}");
    }

    // ─── FC Phase 4 (#398): konkaver Bogen-„Biss" begrenzt den Trust ────────

    /// Ein konkaver Bogen (Zentrum luft-seitig) macht den Loop nicht-konvex,
    /// ohne einen Reflex-Vertex in den SEHNEN-Ecken zu hinterlassen —
    /// `loop_has_reflex` allein sähe einen konvexen Loop und ließe die geraden
    /// Kanten mit Trust 3·δ in die Biss-Region lecken (δ−de < 0 → Over-Fill,
    /// der P3-Reflex-Leak in Bogen-Gestalt). Der Fix zählt konkave Bögen für
    /// die Trust-Wahl wie Reflex-Vertices: Trust = δ, Bogen verweigert
    /// (residual), gerade Kanten bleiben gerundet.
    #[test]
    fn concave_arc_bite_limits_trust() {
        use crate::offset_channel::{ClipComponent, Contour2d};
        let mut clip = clip_axis(true);
        // lb-Rechteck, linke Kante (verts[3]→verts[0]) als konkaver Bogen
        // (bulge < 0 auf CCW-Loop → Zentrum außerhalb, „Biss" in P).
        let outer = Contour2d {
            verts: clip.lb.clone(),
            bulges: vec![0.0, 0.0, 0.0, -0.5],
        };
        clip.components = vec![ClipComponent { outer, holes: Vec::new() }];
        let mut rims = Vec::new();
        let mut arcs = Vec::new();
        let mut residual = 0.0f64;
        rims_from_clip(&clip, -2.0, &mut rims, &mut arcs, &mut residual);
        // Konkaver Bogen → kein Bogen-Feature, ehrlicher B0-Rest.
        assert!(arcs.is_empty(), "konkaver Bogen darf KEIN Feature bauen");
        assert!(
            (residual - 0.75 * 2.0).abs() < 1e-9,
            "konkaver Bogen muss residual ≈ 0.75·δ melden, got {residual}"
        );
        // Die drei geraden Kanten runden weiter …
        assert_eq!(rims.len(), 3, "drei gerade Stufen-Kanten erwartet");
        // … aber mit Trust = δ (nicht 3·δ): sonst leckte die Gegenkanten-Slab
        // über den Sehnen-Rand in die Biss-Region.
        for r in &rims {
            assert!(
                (r.trust - 2.0).abs() < 1e-9,
                "Trust muss auf δ begrenzt sein (konkaver Bogen im Loop), got {}",
                r.trust
            );
        }
        // Gegenprobe: derselbe Loop mit KONVEXEM Bogen (bulge > 0) behält 3·δ.
        let mut clip2 = clip_axis(true);
        let outer2 = Contour2d {
            verts: clip2.lb.clone(),
            bulges: vec![0.0, 0.0, 0.0, 0.5],
        };
        clip2.components = vec![ClipComponent { outer: outer2, holes: Vec::new() }];
        let mut rims2 = Vec::new();
        let mut arcs2 = Vec::new();
        let mut res2 = 0.0f64;
        rims_from_clip(&clip2, -2.0, &mut rims2, &mut arcs2, &mut res2);
        assert_eq!(arcs2.len(), 1, "konvexer Bogen baut ein Bogen-Feature");
        assert_eq!(res2, 0.0, "konvexer Bogen-Loop → kein B0-Rest");
        for r in &rims2 {
            assert!(
                (r.trust - 6.0).abs() < 1e-9,
                "konvexer Loop behält Trust 3·δ, got {}",
                r.trust
            );
        }
    }
}
