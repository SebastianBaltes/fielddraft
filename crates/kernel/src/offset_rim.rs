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
//!   Rechteck-Rand-Test in der Kontaktebene), kein Sampling-Epsilon.
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

/// Liegt das Segment `a–b` (bis auf `tol`) auf einer Randlinie des CCW-Rechtecks
/// `rect`? Rein analytischer Kollinearitäts-/Koinzidenz-Test (Lot-Distanz beider
/// Endpunkte zu einer der vier Kanten-Geraden), KEIN Sampling-Epsilon.
fn edge_on_rect(rect: &[[f64; 2]], a: [f64; 2], b: [f64; 2], tol: f64) -> bool {
    let m = rect.len();
    if m < 2 {
        return false;
    }
    for k in 0..m {
        let r0 = rect[k];
        let r1 = rect[(k + 1) % m];
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

/// Klassifiziert eine ∂P-Kante über die beiden projizierten Member-Rechtecke.
fn classify_edge(clip: &ContactClip, a: [f64; 2], b: [f64; 2], tol: f64) -> EdgeClass {
    let on_i = edge_on_rect(&clip.la, a, b, tol);
    let on_j = edge_on_rect(&clip.lb, a, b, tol);
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

/// Leitet die Rand-Fillet-Features EINES Kontakt-Verschnitts ab.
fn rims_from_clip(clip: &ContactClip, delta_signed: f64, out: &mut Vec<RimFeature>) {
    let concave = delta_signed < 0.0;
    let da = delta_signed.abs();
    if da <= 0.0 {
        return;
    }
    let trust = 3.0 * da;
    let n = clip.p2d.len();
    if n < 3 {
        return;
    }
    // Koinzidenz-Toleranz für den Kanten-auf-Rand-Test: großzügig gegenüber
    // f64-Rundung/rotierten Frames, aber weit unter dem Über-Stand (½·(Fläche_A
    // − Fläche_B)) einer echten Stufe.
    let tol = (1e-3 * da).max(1e-5);

    let mut steps: Vec<StepEdge> = Vec::new();
    for k in 0..n {
        let u0 = clip.p2d[k];
        let u1 = clip.p2d[(k + 1) % n];
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
        // In-Ebenen-Außennormale der CCW-Kante (weg vom P-Inneren): (dy, −dx).
        let d2 = [u1[0] - u0[0], u1[1] - u0[1]];
        let out2 = nrm2([d2[1], -d2[0]]);
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
            trust,
            concave,
        };
        out.push(feat);
        steps.push(StepEdge {
            wall_member: wall,
            contact_n,
            wall_n,
            v_start: k,
            v_end: (k + 1) % n,
            end_world: b3,
        });
    }

    synthesize_vertices(&steps, da, trust, concave, out);
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
    let ds = delta_signed as f64;
    let mut out = Vec::new();
    for clip in contact_clips(inner, ds) {
        rims_from_clip(&clip, ds, &mut out);
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;

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
        rims_from_clip(&clip, -1.0, &mut rims);
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
        rims_from_clip(&clip, 1.0, &mut rims);
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
        rims_from_clip(&clip, -1.0, &mut rims);
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
        rims_from_clip(&clip, -1.5, &mut rims64);
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
            rims_from_clip(&clip, ds, &mut rims64);
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
        rims_from_clip(&clip, -1.0, &mut rims64);
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
        rims_from_clip(&clip, -1.0, &mut rims);
        assert!(rims.is_empty(), "P < 3 Ecken → keine Features");
        // δ = 0 → keine Features.
        let clip2 = clip_axis(true);
        let mut rims2 = Vec::new();
        rims_from_clip(&clip2, 0.0, &mut rims2);
        assert!(rims2.is_empty(), "δ=0 → keine Features");
    }
}
