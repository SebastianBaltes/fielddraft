//! „Lichen" — direkter 3D-Oberflächen-Mesher aus SDF + BRep-Features.
//!
//! Vollständige Umsetzung von `docs/Mesher-Konzept.md`:
//!
//! 1. **Seeds** — BRep-Ecken (flag 3) + Kantenzüge (flag 2); isolierte Flächen
//!    per Newton (in `api/lichen.rs`).
//! 2. **U(p)** (§2) — `min(cap, R_curv, R_BRep, R_reach)`: Krümmung (Shape-
//!    Operator per FD), Abstand zur nächsten BRep-Feature, äußerer Reach
//!    (SDF-Außenmarsch mit Fußpunkt-Verfolgung).
//! 3. **Waben-Wachstum** (§1) — hexagonale Front pro BRep-Fläche, Referenz-
//!    richtung entlang der Hauptkrümmung `d_max`. Eine **harte Kanten-Wand**
//!    (`on_face`-Prädikat) verwirft jeden Kandidaten, der eine BRep-Kante
//!    überspringt — jede Fläche bleibt in ihren Kanten, kein Punkt gelangt auf
//!    eine Nachbarfläche oder ins Leere.
//!
//! Die eigentliche **Triangulierung** je Fläche (leere Umkreisbedingung /
//! §4, §5) übernimmt der robuste, wasserdichte Constrained-Delaunay in
//! [`crate::mesher_cdt`] (spade), aufgerufen pro BRep-Fläche in `api/lichen.rs`
//! mit den flächenübergreifend geteilten Rand-Ketten — so entsteht ein
//! geschlossenes 2-manifoldes Netz, in dem kein Dreieck eine Kante überspannt.
//! Lichens Eigenheit ist die SDF-Reach-Punktverteilung mit harten Kanten; nur
//! die Triangulierung teilt sich der robuste CDT-Kern mit dem PASM-Mesher.

use serde::Deserialize;
use std::collections::{HashMap, VecDeque};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LichenOptions {
    /// Ziel-Kantenlänge `h` (mm): globales Referenz-Spacing und Cap für `U(p)`.
    pub target_edge_length: f32,
    /// Waben-Wachstum aktiv. Aus = nur Seeds.
    pub grow: bool,
    /// Maximaler Normalen-Knickwinkel pro Kante in **Grad** (§2.1): `R_curv =
    /// α_rad / κ_max` mit `α_rad = curv_angle_deg.to_radians()`. Anschaulich der
    /// Winkel, um den sich die Flächennormale über eine Kante drehen darf —
    /// kleiner = feiner auf gekrümmten Flächen. Steuert zugleich die
    /// Rand-Unterteilung (max. Knickwinkel entlang der Kante).
    pub curv_angle_deg: f32,
    /// Reach-Term (SDF-Außenmarsch mit Fußpunkt-Verfolgung) in `U(p)` nutzen.
    pub use_reach: bool,
    /// Fußpunkt-Abstands-Schwelle (mm): wandert der Fußpunkt beim Außenmarsch
    /// weiter als dies von `p` weg, gilt die äußere Medialachse als gekreuzt →
    /// begrenzt `U(p)`.
    pub reach_tol: f32,
    /// Untere Grenze des Punktabstands auf Kanten (mm): winkelgetriebene
    /// Verdichtung stoppt unterhalb dieses Abstands — sehr kleine Kreise
    /// bekommen nicht mehr die volle Winkel-Punktzahl. Geschlossene Kanten
    /// behalten unabhängig davon ≥4 Punkte (Degenerations-Schutz).
    pub min_edge_spacing: f32,
    /// Obergrenze der Interior-Punkte (Runaway-Schutz).
    pub max_points: usize,
    /// Advancing-Front-Triangulierung ausführen. Aus = nur Punktwolke.
    pub triangulate: bool,
    /// ALLGEMEINE (chartlose) Triangulierung über Stern-Konsistenz statt
    /// Chart-CDT — ein Code für plan, gekrümmt und (künftig) Gyroid; siehe
    /// [`crate::lichen_tri`]. Experimentell, Diagnose-first (Löcher werden
    /// gezählt, nicht übermalt).
    pub general_tri: bool,
    /// Akzeptanz-Bedingung des allgemeinen Pfads (schaltbar, Betreiber-Wunsch
    /// 2026-07-11): `"auto"` (Default) = pro Fläche nach Trägertyp — doppelt
    /// gekrümmt → Umkugel, planar/abwickelbar → Stern-Konsens; `"vote"` =
    /// Stern-Konsens (Dreieck in allen 3 Sternen), `"gabriel"` = Umkugel-
    /// Leere mit deterministischem Fächer-Tie-Break, `"hybrid"` = Konsens
    /// ODER Umkugel.
    pub accept: String,
    /// Loch-Füller des General-Pfads (Fallback NACH der Diagnose). Aus =
    /// der BASIS-Algorithmus wird pur gemessen (Betreiber-Wunsch: der
    /// Kern muss auch ohne Fallbacks bestehen).
    pub fill_holes: bool,
    /// MANIFOLD-EXTRAKTION (Cocone-Säule 3) statt greedy Akzeptanz +
    /// Kapazitäts-Klemme: aus der Kandidaten-Menge wird per Sheet-Walk
    /// (Pivotieren um die Front-Kante, kleinster Sweep-Winkel durch die Luft)
    /// eine FLÄCHE herausgelaufen. Ersetzt strukturell die Kandidaten-
    /// Vervollständigung und den Loch-Füller.
    pub extract_walk: bool,
    /// GLOBALER WALK: EIN Netz über die gesamte Punktwolke statt pro B-Rep-
    /// Fläche. Nötig, wo der Weave Trim-Nähte gedroppt hat (Gyroid): ohne
    /// geteilte Punkt-Kette endet jede Fläche mit freiem Rand und es bleibt
    /// ein Schlitz. Ketten-Kanten sind dann Feature-Linien (Kreuzungs-Verbot),
    /// aber keine Wände mehr. Impliziert die Manifold-Extraktion.
    pub global_walk: bool,
    /// ε-SAMPLING gegen die local feature size (`space ≤ ε · lfs`, lfs =
    /// Distanz zur Medialachse INNEN wie außen). 0 = aus. Theorie (Amenta/
    /// Bern) verlangt ε ≲ 0,1; praktisch reicht meist mehr. Das ist die
    /// Vorbedingung, unter der die leere Umkugel das Gegenblatt einer dünnen
    /// Wand nicht erreichen kann — ohne sie kompensiert der Wächter-Turm.
    pub lfs_factor: f32,
}

impl Default for LichenOptions {
    fn default() -> Self {
        Self {
            target_edge_length: 4.0,
            grow: true,
            curv_angle_deg: 20.0,
            use_reach: true,
            reach_tol: 1.0,
            min_edge_spacing: 0.01,
            max_points: 20000,
            triangulate: true,
            general_tri: false,
            accept: "auto".into(),
            fill_holes: true,
            extract_walk: false,
            global_walk: false,
            lfs_factor: 0.0,
        }
    }
}

/// Ein Lichen-Punkt. `flag`: 1 = Oberflächen-Partikel, 2 = Kantenzug, 3 = Ecke.
#[derive(Clone, Copy)]
pub struct LPoint {
    pub p: [f32; 3],
    pub sdf: f32,
    pub n: [f32; 3],
    /// Anzeige-Radius des U(p)-Kreises (im `cellSize`-Slot des Overlay-Formats).
    /// Durch `R_BRep` beschränkt → der gezeichnete Kreis überschreitet NIE eine
    /// BRep-Kante.
    pub u: f32,
    /// Spacing-/Konnektivitäts-Radius: die tatsächliche Punktdichte (`min(h,
    /// R_curv, R_reach)`, OHNE `R_BRep`). Da die harte `on_face`-Wand das
    /// Kanten-Überschreiten garantiert, muss die Dichte an Kanten NICHT
    /// schrumpfen — so bleibt die Abtastung ~uniform und der Graph zusammen-
    /// hängend. Der Mesher/Graph nutzt `space`, das Overlay `u`.
    pub space: f32,
    pub flag: f32,
}

// ── kleine Vektor-Helfer über [f32;3] ────────────────────────────────────
#[inline] fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] { [a[0] + b[0], a[1] + b[1], a[2] + b[2]] }
#[inline] fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] { [a[0] - b[0], a[1] - b[1], a[2] - b[2]] }
#[inline] fn scale(a: [f32; 3], s: f32) -> [f32; 3] { [a[0] * s, a[1] * s, a[2] * s] }
#[inline] fn dot(a: [f32; 3], b: [f32; 3]) -> f32 { a[0] * b[0] + a[1] * b[1] + a[2] * b[2] }
#[inline] fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
#[inline] fn len(a: [f32; 3]) -> f32 { dot(a, a).sqrt() }
#[inline] fn dist(a: [f32; 3], b: [f32; 3]) -> f32 { len(sub(a, b)) }
#[inline] fn norm(a: [f32; 3]) -> [f32; 3] {
    let l = len(a);
    if l > 1e-12 { scale(a, 1.0 / l) } else { [0.0, 0.0, 1.0] }
}
/// Öffentlich für die API-Schicht (Seed-Normalen).
pub fn unit(a: [f32; 3]) -> [f32; 3] { norm(a) }

/// Normalisierter Mittelwert einer Normalen-Liste; `None` bei leer/entartet.
pub fn mean_unit(ns: &[[f32; 3]]) -> Option<[f32; 3]> {
    if ns.is_empty() {
        return None;
    }
    let mut s = [0.0f32; 3];
    for v in ns {
        s = add(s, *v);
    }
    if len(s) > 1e-9 { Some(norm(s)) } else { None }
}

/// Unit-Tangentenbasis `(e1, e2) ⟂ n`.
fn tangent_basis(n: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let seed = if n[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
    let e1 = norm(sub(seed, scale(n, dot(seed, n))));
    let e2 = cross(n, e1);
    (e1, e2)
}

/// ∇f mit FD-Fallback: der Dual-Gradient ist EXAKT auf planaren Flächen
/// null (Abs-Dual bei 0 — die Box liefert d≈1e-10, g=[0,0,0]); Newton
/// konvergiert Punkte genau dorthin. Zentrale Differenzen retten den Fall —
/// ohne diesen Fallback bekamen ALLE Box-Growth-Punkte die Default-Normale
/// [0,0,1], was Wachstums-Frames, Krümmungsschätzung (Überdichte!) und die
/// Stern-Projektion zugleich vergiftete.
pub(crate) fn robust_grad<Fg: Fn([f32; 3]) -> (f32, [f32; 3])>(p: [f32; 3], fg: &Fg) -> [f32; 3] {
    let g = fg(p).1;
    if dot(g, g) > 1e-12 {
        return g;
    }
    let e = 1e-3f32;
    [
        fg([p[0] + e, p[1], p[2]]).0 - fg([p[0] - e, p[1], p[2]]).0,
        fg([p[0], p[1] + e, p[2]]).0 - fg([p[0], p[1] - e, p[2]]).0,
        fg([p[0], p[1], p[2] + e]).0 - fg([p[0], p[1], p[2] - e]).0,
    ]
}

/// Newton-Projektion auf `f = 0` (echter Step `p -= f·∇f/|∇f|²`). Liefert
/// `(p, sdf, Einheitsnormale)`.
fn project<Fg: Fn([f32; 3]) -> (f32, [f32; 3])>(mut p: [f32; 3], fg: &Fg) -> ([f32; 3], f32, [f32; 3]) {
    for _ in 0..5 {
        let (d, g) = fg(p);
        let mut g2 = dot(g, g);
        let mut g = g;
        if g2 < 1e-24 {
            g = robust_grad(p, fg);
            g2 = dot(g, g);
            if g2 < 1e-24 {
                return (p, d, [0.0, 0.0, 1.0]);
            }
        }
        let s = d / g2;
        p = [p[0] - s * g[0], p[1] - s * g[1], p[2] - s * g[2]];
        if d.abs() < 1e-5 { break; }
    }
    let d = fg(p).0;
    (p, d, norm(robust_grad(p, fg)))
}

/// Maximale Hauptkrümmung `κ_max` UND ihre Tangentialrichtung `d_max`
/// (Shape-Operator per zentraler Differenz des Normalenfelds — Muster aus
/// `mesher_points::metric_at`). `gradn` liefert die Einheitsnormale.
/// Shape-Operator per FD eines NORMALENFELDS. Der Aufrufer wählt das Feld:
/// die TRÄGERFLÄCHEN-Normale (glatt, unendlich — keine Kanten-Knicke, kein
/// Klein-Fenster-Rauschen) ist die korrekte Quelle fürs Wachstum; das Body-
/// ∇f taugt NICHT (misst an B-Rep-Kanten den Knick als Krümmung → Mikro-
/// Spacing-Bänder, ~15× Überdichte auf der Box).
pub fn curvature<Gn: Fn([f32; 3]) -> [f32; 3]>(
    p: [f32; 3],
    n: [f32; 3],
    h: f32,
    gradn: &Gn,
) -> (f32, [f32; 3]) {
    let (e1, e2) = tangent_basis(n);
    let delta = (0.1 * h).clamp(1e-3, 0.5);
    let dn = |dir: [f32; 3]| -> [f32; 3] {
        let gp = gradn(add(p, scale(dir, delta)));
        let gm = gradn(sub(p, scale(dir, delta)));
        scale(sub(gp, gm), 1.0 / (2.0 * delta))
    };
    let dne1 = dn(e1);
    let dne2 = dn(e2);
    let a = dot(dne1, e1);
    let c = dot(dne2, e2);
    let b = 0.5 * (dot(dne1, e2) + dot(dne2, e1));
    let mean = 0.5 * (a + c);
    let diff = 0.5 * (a - c);
    let rad = (diff * diff + b * b).sqrt();
    let k1 = mean + rad;
    let k2 = mean - rad;
    let (c1, c2) = if b.abs() > 1e-9 {
        let ang = 0.5 * (2.0 * b).atan2(a - c);
        (ang.cos(), ang.sin())
    } else if a >= c {
        (1.0, 0.0)
    } else {
        (0.0, 1.0)
    };
    let v1 = add(scale(e1, c1), scale(e2, c2)); // Richtung von k1
    if k1.abs() >= k2.abs() {
        (k1.abs(), norm(v1))
    } else {
        (k2.abs(), norm(cross(n, v1)))
    }
}

/// Äußerer Reach via SDF-Marsch entlang `+n` mit Fußpunkt-Verfolgung:
/// `q(t) = x − f·∇f/|∇f|²`. Springt der Fußpunkt signifikant von `p` weg,
/// wurde die äußere Medialachse gekreuzt → `R_reach = t`. Sonst `cap`.
fn reach<Fg: Fn([f32; 3]) -> (f32, [f32; 3])>(p: [f32; 3], n: [f32; 3], cap: f32, tol: f32, fg: &Fg) -> f32 {
    let mut t = 0.1 * cap;
    while t < cap {
        let x = add(p, scale(n, t));
        let (f, g) = fg(x);
        let g2 = dot(g, g);
        if g2 > 1e-20 {
            let s = f / g2;
            let q = [x[0] - s * g[0], x[1] - s * g[1], x[2] - s * g[2]];
            if dist(q, p) > tol {
                return t;
            }
        }
        t *= 1.3;
    }
    cap
}

/// INNERER Reach — Distanz zur INNEREN Medialachse (Marsch entlang −n ins
/// Material, mit Fußpunkt-Verfolgung).
///
/// Konzept-§2.3 hatte das „zur Performance-Optimierung" ausgelassen und die
/// Absicherung dünner Wände dem Sichtlinien-Test überlassen. Die Analyse vom
/// 2026-07-12 zeigt, dass genau diese Annahme FALSCH ist: die local feature
/// size (gegen die das ε-Sampling-Theorem formuliert ist) ist die Distanz zur
/// Medialachse — INNEN wie außen. Auf der 1-mm-Gyroid-Wand ist lfs ≈ 0,5 mm,
/// die Abtastung lag bei 0,2 mm (ε = 0,4 statt ≤ 0,1): die leere Umkugel
/// erreicht dort das GEGENBLATT. Die Flächen-Zerlegung hat das bisher nur
/// maskiert (die harte Wand verhinderte flächenübergreifende Kanten); der
/// globale Walk legt es offen. Der Sichtlinien-/Cocone-Turm ist die
/// Kompensation dieser Lücke, nicht ihre Lösung.
/// Erkennung über die ABFLACHUNG des Distanzfelds (robust und billig): solange
/// keine Medialachse gekreuzt ist, gilt für einen Punkt in Tiefe `t` entlang
/// −n exakt `−f(x) = t` (p ist ja sein nächster Oberflächenpunkt). Sobald ein
/// anderes Blatt näher liegt, wächst |f| langsamer als t. Die Fußpunkt-
/// Verfolgung des Konzepts (§2.3) versagt hier: auf einer 1-mm-Wand springt
/// der Fußpunkt nur ~1 mm weit — unter der Standard-Toleranz `reach_tol`.
fn reach_inner<Fg: Fn([f32; 3]) -> (f32, [f32; 3])>(
    p: [f32; 3],
    n: [f32; 3],
    cap: f32,
    fg: &Fg,
) -> f32 {
    let mut t = 0.02 * cap;
    let floor = 1e-4;
    while t < cap {
        let x = sub(p, scale(n, t));
        let depth = -fg(x).0;
        if depth < 0.9 * t {
            return t.max(floor); // Medialachse gekreuzt
        }
        t *= 1.25;
    }
    cap
}

/// Spacing-Radius `space = min(cap, R_curv, R_reach)` (§2, OHNE `R_BRep`),
/// unten geklemmt. `cap = h`. Steuert Punktdichte + Graph-Konnektivität; die
/// Kanten-Grenze übernimmt die harte `on_face`-Wand (nicht mehr die Dichte),
/// der angezeigte U(p)-Kreis wird separat via `R_BRep` beschränkt.
fn local_radius<Fg>(
    p: [f32; 3],
    n: [f32; 3],
    h: f32,
    kmax: f32,
    opts: &LichenOptions,
    fg: &Fg,
) -> f32
where
    Fg: Fn([f32; 3]) -> (f32, [f32; 3]),
{
    let mut r = h;
    let _ = n;
    if kmax > 1e-6 {
        // R_curv = α_rad / κ_max — bindet nur, wo κ_max > α_rad/h (Radius < h/α),
        // sonst dominiert der h-Cap. Auf flachen/leicht gekrümmten Flächen also
        // wirkungslos, erst auf eng gekrümmten Bereichen greifend.
        r = r.min(opts.curv_angle_deg.to_radians() / kmax);
    }
    if opts.use_reach {
        r = r.min(reach(p, n, h, opts.reach_tol, fg));
    }
    // lfs-ABTASTUNG (ε-Sampling): die Punktdichte muss einen Bruchteil der
    // local feature size einhalten — auch gegen die INNERE Medialachse
    // (dünne Wände!). `lfs_factor` = ε; 0 = aus (altes Verhalten).
    if opts.lfs_factor > 0.0 {
        let lfs = reach(p, n, h, opts.reach_tol, fg).min(reach_inner(p, n, h, fg));
        r = r.min(opts.lfs_factor * lfs);
    }
    // Untere Grenze = Min. Kantenabstand (konsistent mit der Kanten-
    // Abtastung), NICHT 0.2·h: der frühere 0.2h-Floor verwarf die gesamte
    // Krümmungs-Verfeinerung — auf einem 6-mm-Teil bei h=10 war r überall
    // auf 2 mm geklemmt, R_curv (z.B. 0.3 mm auf einer Gyroid-Wand) kam nie
    // zum Zug und das Wachstum fand genau einen Innenpunkt (Ticket ee07b6f1).
    r.clamp(opts.min_edge_spacing.max(1e-4).min(h), h)
}

#[inline]
fn gkey(p: [f32; 3], cell: f32) -> (i32, i32, i32) {
    ((p[0] / cell).floor() as i32, (p[1] / cell).floor() as i32, (p[2] / cell).floor() as i32)
}

/// Kleinster Abstand von `p` zu einem Punkt in `pts` INNERHALB `rmax`
/// (Spatial-Hash, radius-beschränkte Box). Jenseits von `rmax` → MAX.
fn nearest_dist(
    grid: &HashMap<(i32, i32, i32), Vec<u32>>,
    pts: &[LPoint],
    cell: f32,
    p: [f32; 3],
    rmax: f32,
) -> f32 {
    nearest_pt(grid, pts, cell, p, rmax).0
}

/// Wie [`nearest_dist`], liefert zusätzlich das SPACING des nächsten Punkts —
/// für den symmetrischen Konflikt-Test (ein feiner Kandidat neben groben
/// Bestandspunkten ist relativ zu DEREN Spacing redundant). PERF: die Suche
/// ist auf `rmax` beschränkt — mit feiner Zelle (h/4) skaliert sie mit der
/// LOKALEN Dichte statt mit ganzen Mikro-Clustern in einer h-Zelle.
fn nearest_pt(
    grid: &HashMap<(i32, i32, i32), Vec<u32>>,
    pts: &[LPoint],
    cell: f32,
    p: [f32; 3],
    rmax: f32,
) -> (f32, f32) {
    let (kx, ky, kz) = gkey(p, cell);
    let reach = (rmax / cell).ceil() as i32;
    let mut best = (f32::MAX, 0.0f32);
    for dx in -reach..=reach {
        for dy in -reach..=reach {
            for dz in -reach..=reach {
                if let Some(v) = grid.get(&(kx + dx, ky + dy, kz + dz)) {
                    for &i in v {
                        let d = dist(pts[i as usize].p, p);
                        if d < best.0 {
                            best = (d, pts[i as usize].space);
                        }
                    }
                }
            }
        }
    }
    best
}

/// SDF-Sichtlinien-Test `p → q` (§3.4): verwerfen, wenn das Segment am Abgang
/// von `p` oder `q` oder am Mittelpunkt signifikant (relativ zur lokalen Skala
/// `r`) ins Material (negatives SDF) eintaucht.
fn visible<Fg: Fn([f32; 3]) -> (f32, [f32; 3])>(p: [f32; 3], q: [f32; 3], r: f32, fg: &Fg) -> bool {
    let d = sub(q, p);
    let dl = len(d);
    if dl < 1e-6 {
        return false;
    }
    let dir = scale(d, 1.0 / dl);
    let thresh = -0.15 * r;
    if fg(add(p, scale(dir, 0.25 * dl))).0 < thresh {
        return false;
    }
    if fg(sub(q, scale(dir, 0.25 * dl))).0 < thresh {
        return false;
    }
    if fg(scale(add(p, q), 0.5)).0 < thresh {
        return false;
    }
    true
}

/// Waben-Wachstum (§1) auf EINER Fläche: von den (Rand-)Seeds ausgehend
/// hexagonale Kandidaten im Radius `U(p)`, Referenzrichtung entlang der
/// Hauptkrümmung `d_max`; auf die Oberfläche projizieren, per Sichtlinie
/// filtern. `on_face(p)` ist die **harte Kanten-Wand**: ein Kandidat, der eine
/// BRep-Kante überspringt, landet auf einer Nachbarfläche (oder im freien Raum)
/// → `on_face` schlägt fehl → verworfen. So bleibt jede Fläche in ihren Kanten
/// und wächst nie auf eine andere Fläche oder ins Leere. Liefert die neu
/// gewachsenen Interior-Punkte (flag 1).
pub fn grow_surface<Fg, Fb, On, Sc>(
    seeds: &[LPoint],
    existing: &[LPoint],
    h: f32,
    opts: &LichenOptions,
    fg: &Fg,
    brep_dist: &Fb,
    on_face: &On,
    surf_curv: &Sc,
) -> Vec<LPoint>
where
    Fg: Fn([f32; 3]) -> (f32, [f32; 3]),
    Fb: Fn([f32; 3]) -> f32,
    On: Fn([f32; 3], [f32; 3], f32) -> bool,
    Sc: Fn([f32; 3]) -> (f32, [f32; 3]),
{
    let cell = (0.25 * h).max(1e-4);
    let mut pts: Vec<LPoint> = seeds.to_vec();
    let mut grid: HashMap<(i32, i32, i32), Vec<u32>> = HashMap::new();
    for (i, s) in pts.iter().enumerate() {
        grid.entry(gkey(s.p, cell)).or_default().push(i as u32);
    }
    // GLOBALE Punkte (Ketten/Ecken/bereits gewachsene ANDERER Flächen) als
    // zusätzliche Konflikt-Quelle: an TANGENTIALEN Nähten wachsen zwei fast
    // parallele Flächen sonst ihre Punkte 0,04 mm übereinander — die Sterne
    // mischen die Doppelblätter zu Folds (Orientierungs-Konflikte).
    let mut xgrid: HashMap<(i32, i32, i32), Vec<u32>> = HashMap::new();
    for (i, s) in existing.iter().enumerate() {
        xgrid.entry(gkey(s.p, cell)).or_default().push(i as u32);
    }
    let n_seeds = pts.len();
    let cap = n_seeds + opts.max_points;

    // HARTES OBERFLÄCHEN-GATE (Betreiber-Bild 2026-07-12, „Grow findet
    // Punkte außerhalb des Körpers"): die Schritt-Toleranz `|f| ≤ 0.2·rr`
    // wächst mit dem Wachstumsradius — bei h=10 akzeptierte sie 2 mm Luft.
    // An Distanzfeld-Graten (konkave Buchten, tangentiale Nähte) stallt
    // Newton genau dort. Jeder Kandidat wird vor der Aufnahme nachprojiziert
    // und ABSOLUT geprüft; wer nicht auf die Fläche konvergiert, fliegt.
    let surf_tol = (5e-4 * h).max(1e-4);
    let mut rej = [0usize; 6]; // sdf, visible, face, redundant, rebalance, ok
    let mut queue: VecDeque<usize> = (0..n_seeds).collect();
    while let Some(i) = queue.pop_front() {
        if pts.len() >= cap {
            break;
        }
        let p = pts[i].p;
        let n = pts[i].n;
        // Krümmung IMMER von der TRÄGERFLÄCHE (glatt, kantenfrei) — daraus
        // Wachstums-Radius und Referenzrichtung. GRADING: höchstens ×1.5
        // gegenüber dem eigenen Spacing (Dichte-Übergänge von dichten
        // Ketten/Nachbarn ins gröbere Innere bleiben stetig).
        let (kmax, dmax) = surf_curv(p);
        let r = local_radius(p, n, h, kmax, opts, fg).min(1.5 * pts[i].space);
        let (u, w) = if kmax > 1e-4 {
            (dmax, cross(n, dmax))
        } else {
            tangent_basis(n)
        };
        'dirs: for k in 0..6 {
            let ang = k as f32 * std::f32::consts::FRAC_PI_3;
            let dir = add(scale(u, ang.cos()), scale(w, ang.sin()));
            // SCHRITT-HALBIERUNG (Sub-h-Features): überspringt der Kandidat
            // die Fläche (Wand-Treffer / hintere Wand / neben der Fläche),
            // wird NICHT verworfen, sondern mit halbem Radius nachgesetzt —
            // auf einem 6-mm-Feature bei r≈3 landete sonst JEDER Kandidat
            // auf der Nachbarwand und die Fläche wuchs gar nicht.
            let r_full = r;
            let mut rr = r;
            let (cp, cd, cn, nd, r_eff) = 'step: {
                for _ in 0..5 {
                    let (cp, cd, cn) = project(add(p, scale(dir, rr)), fg);
                    if cd.abs() > 0.2 * rr {
                        rej[0] += 1;
                        rr *= 0.5;
                        continue;
                    }
                    if !visible(p, cp, rr, fg) {
                        rej[1] += 1;
                        rr *= 0.5;
                        continue;
                    }
                    // Harte Kanten-Wand: der Kandidat muss auf DERSELBEN
                    // BRep-Fläche liegen (Kanten sind harte Grenzen).
                    if !on_face(p, cp, rr) {
                        rej[2] += 1;
                        rr *= 0.5;
                        continue;
                    }
                    // REDUNDANZ triggert ebenfalls die Halbierung: auf einem
                    // schmalen Patch landet der volle Schritt direkt an der
                    // Gegen-Kette (nd winzig) — ein kleinerer Schritt bleibt
                    // mittig und findet Platz. Der symmetrische r_eff-Wächter
                    // (Spacing des nächsten Bestandspunkts, gedeckelt auf
                    // r_full) verhindert das Einnisten neben groben Nachbarn.
                    let (nd_own, ns_own) = nearest_pt(&grid, &pts, cell, cp, r_full);
                    let (nd_x, ns_x) = nearest_pt(&xgrid, existing, cell, cp, r_full);
                    let (nd, nspace) =
                        if nd_own <= nd_x { (nd_own, ns_own) } else { (nd_x, ns_x) };
                    let r_eff = rr.max(nspace.min(r_full));
                    if nd < 0.45 * r_eff {
                        rej[3] += 1;
                        rr *= 0.5;
                        continue;
                    }
                    break 'step (cp, cd, cn, nd, r_eff);
                }
                continue 'dirs;
            };
            // KASKADEN-BREMSE: der halbierte Schritt gilt nur der PLATZIERUNG
            // (Abstands-Checks mit rr) — das SPACING des neuen Punkts bleibt
            // am vollen Radius orientiert, sonst gebären halbierte Punkte
            // halbierte Kinder (Verdichtungs-Kaskade: Zylinder 0→84 Löcher).
            let (cp, cd, cn, nd, r_eff) = (cp, cd, cn, nd, r_eff);
            let r = r_eff;
            // Saubere Abtastung: Kandidat näher als 0,8·r an einem bestehenden
            // Punkt (Spacing ≈ r; eine über-dichte Wolke bricht die leere-
            // Umkreis-Bedingung des Meshers).
            let mut cp = cp;
            let mut cd = cd;
            let mut cn = cn;
            if nd < 0.8 * r {
                // §1.5 KONFLIKT-REBALANCING (Konzept-Schritt 5): NICHT einfach
                // verwerfen — an Front-Kollisionen und Dichte-Übergängen reißt
                // das genau die Lücken (0,8r < Lücke < Dreiecks-tauglich), an
                // denen die Sterne später uneins sind. Stattdessen den
                // Kandidaten iterativ von den Konflikt-Nachbarn wegdrücken
                // (Ziel ≈ 0,85r), neu auf die Fläche projizieren und nur
                // verwerfen, wenn auch ausbalanciert kein sauberer Abstand
                // entsteht (dann ist er ECHT redundant).
                let mut q = cp;
                let mut settled = false;
                for _ in 0..4 {
                    let (kx, ky, kz) = gkey(q, cell);
                    let mut disp = [0.0f32; 3];
                    let mut conflicts = 0;
                    let rb_reach = ((0.85 * r) / cell).ceil() as i32;
                    for dx in -rb_reach..=rb_reach {
                        for dy in -rb_reach..=rb_reach {
                            for dz in -rb_reach..=rb_reach {
                                let Some(v) = grid.get(&(kx + dx, ky + dy, kz + dz)) else {
                                    continue;
                                };
                                for &wi in v {
                                    let d = sub(q, pts[wi as usize].p);
                                    let l = len(d);
                                    if l < 1e-9 || l >= 0.85 * r {
                                        continue;
                                    }
                                    disp = add(disp, scale(d, (0.85 * r - l) / l));
                                    conflicts += 1;
                                }
                            }
                        }
                    }
                    if conflicts == 0 {
                        settled = true;
                        break;
                    }
                    q = add(q, scale(disp, 0.7));
                    let (qp, qd, qn) = project(q, fg);
                    if qd.abs() > 0.2 * r {
                        break; // von der Fläche gerutscht
                    }
                    q = qp;
                    cd = qd;
                    cn = qn;
                }
                if !settled
                    || nearest_dist(&grid, &pts, cell, q, r)
                        .min(nearest_dist(&xgrid, existing, cell, q, r))
                        < 0.75 * r
                    || !visible(p, q, r, fg)
                    || !on_face(p, q, r)
                {
                    rej[4] += 1;
                    continue;
                }
                cp = q;
            }
            // Oberflächen-Gate (s. surf_tol oben): nachprojizieren, dann
            // kompromisslos absolut prüfen.
            if cd.abs() > surf_tol {
                let (qp, qd, qn) = project(cp, fg);
                if qd.abs() > surf_tol {
                    rej[0] += 1;
                    continue;
                }
                cp = qp;
                cd = qd;
                cn = qn;
            }
            // Eigener Spacing-Radius des Kandidaten (Krümmung von der
            // Trägerfläche); Anzeige-U zusätzlich durch den Kantenabstand
            // beschränkt (Kreis bleibt in der Fläche).
            let cspace =
                local_radius(cp, cn, h, surf_curv(cp).0, opts, fg).min(1.5 * r_full);
            let cu = cspace.min(brep_dist(cp));
            let j = pts.len();
            pts.push(LPoint { p: cp, sdf: cd, n: cn, u: cu, space: cspace, flag: 1.0 });
            grid.entry(gkey(cp, cell)).or_default().push(j as u32);
            queue.push_back(j);
            if pts.len() >= cap {
                break;
            }
        }
    }
    if std::env::var("LICHEN_DEBUG_GROW").is_ok() {
        eprintln!(
            "GROW-REJ sdf={} vis={} face={} redundant={} rebalance={}",
            rej[0], rej[1], rej[2], rej[3], rej[4]
        );
    }
    pts.split_off(n_seeds)
}


/// Adaptive B-Rep-Kanten-Unterteilung (§3.2): setzt einen neuen Punkt, sobald
/// der akkumulierte **Knickwinkel** `angle_rad` (Radiant) ODER die Bogenlänge
/// `h` überschritten wird — dicht in Kurven, spärlich auf Geraden. Endpunkte
/// (die Ecken) bleiben exakt erhalten. Damit steuert der Krümmungswinkel auch
/// die Randdichte. `min_spacing` deckelt die winkelgetriebene Verdichtung
/// nach unten: kein Punkt näher als dies am zuletzt gesetzten — sehr kleine
/// Kreise entarten sonst zur Punktwolke in Mikrometer-Abständen (Ticket
/// 6d3f4a7f); die Mindest-Punktzahl geschlossener Kanten sichert der Caller.
/// `end_bounds` sind virtuelle Nachbarsegment-Längen VOR dem ersten bzw.
/// NACH dem letzten Punkt (Ticket cec70614): an einer Ecke, in der auch eine
/// dicht abgetastete Kante mündet, muss das Grading dieser Kette am Rand mit
/// deren Endabstand weiterlaufen — sonst klafft zwischen U(Ecke) und dem
/// ersten Kettenpunkt eine Lücke. `f32::INFINITY` = kein Rand-Zwang.
pub fn resample_edge_by_curvature(
    pts: &[[f32; 3]],
    h: f32,
    angle_rad: f32,
    min_spacing: f32,
    end_bounds: (f32, f32),
) -> Vec<[f32; 3]> {
    if pts.len() < 2 {
        return pts.to_vec();
    }
    // Direkt auf der ORIGINAL-Polyline arbeiten (die B-Rep-Tessellierung ist
    // bereits krümmungsadaptiv): Knickwinkel an den Original-Vertices
    // akkumulieren, Bogenlänge kontinuierlich — lange gerade Segmente werden
    // bei h interpoliert unterteilt. Ein uniformes Pre-Sampling mit Schritt
    // ~0.15·h (früherer Ansatz) zerstörte kleine, stark gekrümmte Kanten:
    // ein 6-mm-Kreis wurde bei h=10 zu ~4 Punkten, bevor der Winkel-Test
    // überhaupt messen konnte — der Krümmungswinkel war dort wirkungslos.
    let ang_thresh = angle_rad.max(0.01); // rad; Floor, damit α=0 nicht „nie"

    // Kumulative Bogenlängen der Original-Polyline: Emissionen werden als
    // Bogen-POSITIONEN gesammelt, damit die anschließende Grading-Passe neue
    // Punkte exakt AUF der Polyline interpolieren kann.
    let mut cum: Vec<f32> = Vec::with_capacity(pts.len());
    let mut acc = 0.0f32;
    cum.push(0.0);
    for i in 1..pts.len() {
        acc += dist(pts[i - 1], pts[i]);
        cum.push(acc);
    }
    let total = acc;
    if total <= 1e-12 {
        return vec![pts[0], pts[pts.len() - 1]];
    }

    // ── Pass 1: Emissions-Positionen (h-Länge ODER Knickwinkel) ──
    let mut s_out: Vec<f32> = vec![0.0];
    let mut acc_ang = 0.0f32;
    for i in 1..pts.len() {
        // Längen-Unterteilung: alle h Bogenlänge seit der letzten Emission
        // (Geraden spüren nur h).
        while cum[i] - *s_out.last().unwrap() >= h {
            let s = s_out.last().unwrap() + h;
            s_out.push(s);
            acc_ang = 0.0;
        }
        // Knickwinkel am inneren Vertex i (zwischen den Nachbarsegmenten).
        // Die Winkel-Emission respektiert den Mindestabstand zum zuletzt
        // gesetzten Punkt (Ticket 6d3f4a7f).
        if i + 1 < pts.len() {
            let d1 = norm(sub(pts[i], pts[i - 1]));
            let d2 = norm(sub(pts[i + 1], pts[i]));
            acc_ang += dot(d1, d2).clamp(-1.0, 1.0).acos();
            if acc_ang >= ang_thresh && cum[i] - *s_out.last().unwrap() >= min_spacing {
                s_out.push(cum[i]);
                acc_ang = 0.0;
            }
        }
    }
    // Trailing-Remnant tilgen (Ticket 2e7eab20 „andersrum"): die Emission
    // läuft im min_spacing-Raster und lässt vor dem Kettenende ein beliebig
    // kleines Rest-Segment übrig (z.B. 0.017 mm bei min 0.2) — das drückt
    // U(Ecke) auf µm und zwingt die Überdeckung in Verdichtungs-Kaskaden.
    // Interior-Punkte näher als min_spacing am Ende fallen weg; der letzte
    // Abstand wird Teil des vorletzten Segments.
    while s_out.len() > 1 && total - *s_out.last().unwrap() < min_spacing {
        s_out.pop();
    }
    if total - *s_out.last().unwrap() > 1e-9 {
        s_out.push(total);
    } else {
        *s_out.last_mut().unwrap() = total;
    }

    // ── Pass 2: Grading (Ticket e1a10bcd) — benachbarte Segmentlängen dürfen
    // höchstens um GRADE wachsen. Da U(p) eines Kettenpunkts der Abstand zum
    // nächsten Nachbarn ist, garantiert GRADE < 2 die vollständige Über-
    // deckung jeder Kante durch einander ÜBERLAPPENDE U(p)-Kreise:
    // U_a + U_b ≥ 2·d/GRADE > d. Ohne Grading springt die Dichte an einer
    // Ecken-Häufung abrupt (0.01 mm neben 10 mm) und die Kreise verketten
    // sich nicht. Nur EINFÜGEN, nie verschieben — Winkel-Emissionen bleiben
    // exakt auf ihren Polyline-Vertices.
    const GRADE: f32 = 1.5;
    for _ in 0..48 {
        let n = s_out.len();
        let mut next: Vec<f32> = Vec::with_capacity(n + 8);
        next.push(s_out[0]);
        let mut split_any = false;
        let floor = min_spacing.max(1e-6);
        for k in 1..n {
            let d = s_out[k] - s_out[k - 1];
            // ECHTE Nachbarsegment-Längen (bzw. Ecken-Rand-Bedingungen).
            let dl_r = if k >= 2 { s_out[k - 1] - s_out[k - 2] } else { end_bounds.0 };
            let dr_r = if k + 1 < n { s_out[k + 1] - s_out[k] } else { end_bounds.1 };
            // ÜBERDECKUNG (Ticket 2e7eab20, hart — kein min_spacing-Floor):
            // U(p) ist der Abstand zum nächsten Nachbarn, also ist ein
            // Segment genau dann von den U-Kreisen seiner Enden überdeckt,
            // wenn min(dl,d) + min(d,dr) ≥ d. Verfehlt es das (beidseitig
            // kleinere Nachbarn — z.B. eine 0.017-mm-Mikro-Kante an einer
            // Ecke neben einem 0.3-mm-Segment), MUSS gesplittet werden,
            // auch unterhalb des Mindestabstands: Geometrie, die feiner als
            // min_spacing ist, erzwingt lokal feinere Punkte.
            let cover_gap =
                dl_r.min(d) + dr_r.min(d) < d * (1.0 - 1e-3) && d > 2e-6;
            // GRADING (Ticket e1a10bcd, weich — mit Floor gegen µm-Kaskaden
            // aus Rest-Schnipseln): Nachbarlängen wachsen höchstens um GRADE.
            let dl_f = dl_r.max(floor);
            let dr_f = dr_r.max(floor);
            let ratio_split =
                d > GRADE * dl_f.min(dr_f) * (1.0 + 1e-3) && d > 2.0 * floor;
            if cover_gap || ratio_split {
                // Vom kleineren Nachbarn aus geometrisch wachsen.
                let base = if cover_gap {
                    dl_r.min(dr_r).max(1e-6)
                } else {
                    dl_f.min(dr_f)
                };
                let step = (GRADE * base).min(0.5 * d);
                if dl_r <= dr_r {
                    next.push(s_out[k - 1] + step);
                } else {
                    next.push(s_out[k] - step);
                }
                split_any = true;
            }
            next.push(s_out[k]);
        }
        s_out = next;
        if !split_any {
            break;
        }
    }

    // ── Pass 3: Bogen-Positionen → Punkte (Interpolation auf der Polyline) ──
    let mut out: Vec<[f32; 3]> = Vec::with_capacity(s_out.len());
    let mut seg = 1usize;
    for &s in &s_out {
        while seg + 1 < cum.len() && cum[seg] < s {
            seg += 1;
        }
        let (s0, s1) = (cum[seg - 1], cum[seg]);
        let t = if s1 > s0 { ((s - s0) / (s1 - s0)).clamp(0.0, 1.0) } else { 0.0 };
        let (a, b) = (pts[seg - 1], pts[seg]);
        let p = [
            a[0] + t * (b[0] - a[0]),
            a[1] + t * (b[1] - a[1]),
            a[2] + t * (b[2] - a[2]),
        ];
        if out.last().map_or(true, |&q| dist(q, p) > 1e-9) {
            out.push(p);
        }
    }
    out
}

/// Packt Punkte + Dreiecke ins flache Overlay-Format
/// `[nPoints, nTris, nFallbackTris, points×9 (x,y,z,sdf,nx,ny,nz,U,flag),
/// indices×3, nFaceRec, faceRec×4 (pointIdx,nx,ny,nz)]`.
///
/// Der Trailing-Block hält je Punkt EINEN U(p)-Kreis-Record PRO Fläche, auf der
/// der Punkt liegt: Flächenpunkte tragen genau eine (ihre ∇f-Normale),
/// Kantenpunkte zwei, Ecken ≥3 — so kann das Overlay „2 Kreise pro Randpunkt"
/// (bzw. je Fläche einen) zeichnen. `pack` gemittelt die Normalen NICHT weg wie
/// der `LPoint.n`-Slot. Selbstbeschreibend (Count voran), damit die Punkt- und
/// Index-Offsets exakt wie zuvor liegen. `point_faces[i]` = die Flächen-IDs
/// (+ zugehörige Normalen) von Punkt `i`.
/// Qualitäts-Kennzahlen eines Dreiecksnetzes — geteilt von Unit-Tests,
/// `pack_with_diag` (→ Lichen-Dialog-Readout) und Diagnose-Ausgaben.
/// Diagnose-first (Betreiber-Prinzip): alles wird GEZÄHLT, nichts gefiltert.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshQuality {
    /// Kanten mit genau EINEM Dreieck (offener Rand/Loch).
    pub boundary_edges: usize,
    /// Kanten mit MEHR als zwei Dreiecken.
    pub nonmanifold_edges: usize,
    /// Dreiecke mit (nahezu) Null-Fläche (< 1e-12 mm²) oder doppelten Ecken.
    pub degenerate_tris: usize,
    /// Zusammenhangskomponenten über die Dreiecks-Konnektivität.
    pub components: usize,
    /// Euler-Charakteristik V − E + F (geschlossene Kugel-Topologie = 2 pro
    /// Komponente).
    pub euler: i64,
    /// Gerichtete Kanten, die MEHRFACH in derselben Richtung vorkommen —
    /// 0 = konsistent orientierbar (bei manifold+watertight).
    pub orientation_violations: usize,
}

impl MeshQuality {
    pub fn manifold(&self) -> bool {
        self.nonmanifold_edges == 0
    }
    pub fn watertight(&self) -> bool {
        self.boundary_edges == 0 && self.nonmanifold_edges == 0
    }
}

/// Kennzahlen über die finale Dreiecksliste berechnen (Indizes in `points`).
pub fn mesh_quality(points: &[[f32; 3]], tris: &[[u32; 3]]) -> MeshQuality {
    use std::collections::HashMap;
    let mut edge_use: HashMap<(u32, u32), u32> = HashMap::new();
    let mut dir_use: HashMap<(u32, u32), u32> = HashMap::new();
    let mut degenerate = 0usize;
    // Union-Find über die benutzten Vertices.
    let mut parent: HashMap<u32, u32> = HashMap::new();
    fn find(parent: &mut HashMap<u32, u32>, x: u32) -> u32 {
        let p = *parent.entry(x).or_insert(x);
        if p == x {
            return x;
        }
        let r = find(parent, p);
        parent.insert(x, r);
        r
    }
    for &[a, b, c] in tris {
        if a == b || b == c || c == a {
            degenerate += 1;
            continue;
        }
        let (pa, pb, pc) = (
            points[a as usize],
            points[b as usize],
            points[c as usize],
        );
        let u = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
        let v = [pc[0] - pa[0], pc[1] - pa[1], pc[2] - pa[2]];
        let cx = (u[1] * v[2] - u[2] * v[1]) as f64;
        let cy = (u[2] * v[0] - u[0] * v[2]) as f64;
        let cz = (u[0] * v[1] - u[1] * v[0]) as f64;
        if 0.5 * (cx * cx + cy * cy + cz * cz).sqrt() < 1e-12 {
            degenerate += 1;
        }
        for (x, y) in [(a, b), (b, c), (c, a)] {
            *edge_use.entry((x.min(y), x.max(y))).or_default() += 1;
            *dir_use.entry((x, y)).or_default() += 1;
        }
        let ra = find(&mut parent, a);
        let rb = find(&mut parent, b);
        parent.insert(rb, ra);
        let rb2 = find(&mut parent, b);
        let rc = find(&mut parent, c);
        parent.insert(rc, rb2);
    }
    let boundary_edges = edge_use.values().filter(|&&u| u == 1).count();
    let nonmanifold_edges = edge_use.values().filter(|&&u| u > 2).count();
    let orientation_violations = dir_use.values().filter(|&&u| u > 1).count();
    let verts: std::collections::HashSet<u32> =
        tris.iter().flat_map(|t| t.iter().copied()).collect();
    let mut roots: std::collections::HashSet<u32> = std::collections::HashSet::new();
    for &v in &verts {
        let r = find(&mut parent, v);
        roots.insert(r);
    }
    let euler = verts.len() as i64 - edge_use.len() as i64 + tris.len() as i64;
    MeshQuality {
        boundary_edges,
        nonmanifold_edges,
        degenerate_tris: degenerate,
        components: roots.len(),
        euler,
        orientation_violations,
    }
}

/// Konsistente Orientierung über Kanten-Nachbarschaft propagieren: Seed je
/// Zusammenhangskomponente an der Feld-Normalen (Mittel der Ecken-Normalen),
/// dann BFS über manifold-Kanten (genau 2 Dreiecke) — Nachbarn müssen die
/// geteilte Kante GEGENLÄUFIG durchlaufen. Rückgabe: Konflikte (Kanten, an
/// denen beide Richtungen erzwungen wären — nur an Defekten möglich).
pub fn orient_consistent(all: &[LPoint], tris: &mut Vec<[u32; 3]>) -> usize {
    // Konflikte werden nicht nur GEZÄHLT, sondern AUFGELÖST: ein Fold
    // (zwei Dreiecke durchlaufen eine Kante gleichläufig, obwohl beide
    // konsistent orientiert sind) ist IMMER geometrisch falsch — an
    // tangentialen Nähten (Mini-Kugel in der Wand, Betreiber-Repro mit
    // 25 Konflikten) überleben sie alle seitenbasierten Wächter. Pro
    // Konflikt fliegt deterministisch das Dreieck mit dem größeren
    // Umkreisradius; danach wird neu propagiert (max 4 Runden). Das
    // hinterlässt ein EHRLICHES Loch statt eines falschen Folds.
    let circum_r2 = |t: &[u32; 3]| -> f32 {
        let (pa, pb, pc) = (
            all[t[0] as usize].p,
            all[t[1] as usize].p,
            all[t[2] as usize].p,
        );
        let ab = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
        let ac = [pc[0] - pa[0], pc[1] - pa[1], pc[2] - pa[2]];
        let ab2 = ab[0] * ab[0] + ab[1] * ab[1] + ab[2] * ab[2];
        let ac2 = ac[0] * ac[0] + ac[1] * ac[1] + ac[2] * ac[2];
        let abac = ab[0] * ac[0] + ab[1] * ac[1] + ab[2] * ac[2];
        let den = 2.0 * (ab2 * ac2 - abac * abac);
        if den.abs() < 1e-20 {
            return f32::MAX;
        }
        let s_ = (ac2 * (ab2 - abac)) / den;
        let t_ = (ab2 * (ac2 - abac)) / den;
        let dc = [
            s_ * ab[0] + t_ * ac[0],
            s_ * ab[1] + t_ * ac[1],
            s_ * ab[2] + t_ * ac[2],
        ];
        dc[0] * dc[0] + dc[1] * dc[1] + dc[2] * dc[2]
    };
    for _ in 0..4 {
        let pairs = orient_pass(all, tris);
        if pairs.is_empty() {
            return 0;
        }
        let mut drop: Vec<usize> = pairs
            .into_iter()
            .map(|(ti, tj)| {
                let (ri, rj) = (circum_r2(&tris[ti]), circum_r2(&tris[tj]));
                if (ri, tris[ti]) >= (rj, tris[tj]) { ti } else { tj }
            })
            .collect();
        drop.sort_unstable();
        drop.dedup();
        for &d in drop.iter().rev() {
            tris.swap_remove(d);
        }
        tris.sort_unstable();
    }
    orient_pass(all, tris).len()
}

/// Ein Propagations-Durchlauf: orientiert `tris` konsistent und liefert die
/// Konflikt-Paare (Kante von beiden Dreiecken gleichläufig durchlaufen).
fn orient_pass(all: &[LPoint], tris: &mut [[u32; 3]]) -> Vec<(usize, usize)> {
    use std::collections::HashMap;
    let mut edge_tris: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for (ti, &[a, b, c]) in tris.iter().enumerate() {
        for (x, y) in [(a, b), (b, c), (c, a)] {
            edge_tris.entry((x.min(y), x.max(y))).or_default().push(ti);
        }
    }
    // Durchläuft `t` die Kante (u,v) in DIESER Richtung?
    let runs = |t: &[u32; 3], u: u32, v: u32| -> bool {
        (t[0] == u && t[1] == v) || (t[1] == u && t[2] == v) || (t[2] == u && t[0] == v)
    };
    let mut visited = vec![false; tris.len()];
    let mut conflicts: Vec<(usize, usize)> = Vec::new();
    for seed in 0..tris.len() {
        if visited[seed] {
            continue;
        }
        // Seed an der Feld-Normalen ausrichten.
        let t = tris[seed];
        let (pa, pb, pc) = (
            all[t[0] as usize].p,
            all[t[1] as usize].p,
            all[t[2] as usize].p,
        );
        let n_tri = [
            (pb[1] - pa[1]) * (pc[2] - pa[2]) - (pb[2] - pa[2]) * (pc[1] - pa[1]),
            (pb[2] - pa[2]) * (pc[0] - pa[0]) - (pb[0] - pa[0]) * (pc[2] - pa[2]),
            (pb[0] - pa[0]) * (pc[1] - pa[1]) - (pb[1] - pa[1]) * (pc[0] - pa[0]),
        ];
        let mut n_ref = [0.0f32; 3];
        for &i in &t {
            let n = all[i as usize].n;
            n_ref = [n_ref[0] + n[0], n_ref[1] + n[1], n_ref[2] + n[2]];
        }
        if n_tri[0] * n_ref[0] + n_tri[1] * n_ref[1] + n_tri[2] * n_ref[2] < 0.0 {
            tris[seed].swap(1, 2);
        }
        visited[seed] = true;
        let mut queue = std::collections::VecDeque::from([seed]);
        while let Some(ti) = queue.pop_front() {
            let cur = tris[ti];
            for (u, v) in [(cur[0], cur[1]), (cur[1], cur[2]), (cur[2], cur[0])] {
                let e = (u.min(v), u.max(v));
                let Some(list) = edge_tris.get(&e) else { continue };
                if list.len() != 2 {
                    continue; // Rand/Defekt: nicht propagieren
                }
                for &tj in list {
                    if tj == ti {
                        continue;
                    }
                    // Konsistent = Nachbar durchläuft (u,v) GEGENLÄUFIG (v,u).
                    let consistent = runs(&tris[tj], v, u);
                    if visited[tj] {
                        if !consistent {
                            conflicts.push((ti, tj));
                            if std::env::var("LICHEN_DEBUG_ORIENT").is_ok() {
                                let pf = |i: u32| {
                                    let lp = &all[i as usize];
                                    format!("#{i} f{} [{:.2},{:.2},{:.2}]", lp.flag, lp.p[0], lp.p[1], lp.p[2])
                                };
                                eprintln!(
                                    "ORIENT-KONFLIKT Kante {}|{}: tri_i={:?} tri_j={:?} | {} {}",
                                    u, v, tris[ti], tris[tj], pf(u), pf(v)
                                );
                            }
                        }
                        continue;
                    }
                    if !consistent {
                        tris[tj].swap(1, 2);
                    }
                    visited[tj] = true;
                    queue.push_back(tj);
                }
            }
        }
    }
    conflicts
}

pub fn pack(all: &[LPoint], tris: &[[u32; 3]], point_faces: &[Vec<(u32, [f32; 3])>]) -> Vec<f32> {
    pack_with_diag(all, tris, point_faces, &[], &[])
}

/// ÜBERLANGE Kanten des finalen Netzes: Länge > 3 × max(space der Endpunkte).
/// Das ist die sichtbare Signatur einer lokalen Umkugel-Verletzung bzw. einer
/// unbesiedelten Region (Betreiber-Bild „Kanten quer durchs Kugelinnere") —
/// eine Delaunay-Kante zwischen korrekt abgetasteten Nachbarn ist nie viel
/// länger als das lokale Spacing. Fürs Overlay (violett) + Dialog-Zähler.
fn long_edges_of(all: &[LPoint], tris: &[[u32; 3]]) -> Vec<(u32, u32)> {
    let mut seen: std::collections::HashSet<(u32, u32)> = std::collections::HashSet::new();
    let mut out: Vec<(u32, u32)> = Vec::new();
    for &[a, b, c] in tris {
        for (x, y) in [(a, b), (b, c), (c, a)] {
            let e = (x.min(y), x.max(y));
            if !seen.insert(e) {
                continue;
            }
            let (pa, pb) = (&all[e.0 as usize], &all[e.1 as usize]);
            let l = dist(pa.p, pb.p);
            if l > 3.0 * pa.space.max(pb.space) {
                out.push(e);
            }
        }
    }
    out.sort_unstable();
    out
}

/// Wie [`pack`], plus DIAGNOSE-Trailing-Blöcke (beide Pfade):
/// `[nOpenEdge, (a,b)×n, nWeakTri, (a,b,c)×n, nLongEdge, (a,b)×n]` NACH dem
/// Face-Normal-Block — offene Loch-Kanten (rot), Fast-Konsens-Kandidaten
/// (orange) und überlange Kanten (violett, kernel-berechnet) fürs Overlay.
pub fn pack_with_diag(
    all: &[LPoint],
    tris: &[[u32; 3]],
    point_faces: &[Vec<(u32, [f32; 3])>],
    open_edges: &[(u32, u32)],
    weak_tris: &[[u32; 3]],
) -> Vec<f32> {
    let n = all.len();
    let mut recs: Vec<f32> = Vec::new();
    let mut rec_count = 0usize;
    for (i, lp) in all.iter().enumerate() {
        let fns = point_faces.get(i).map(Vec::as_slice).unwrap_or(&[]);
        if fns.is_empty() {
            recs.extend_from_slice(&[i as f32, lp.n[0], lp.n[1], lp.n[2]]);
            rec_count += 1;
        } else {
            for &(_, nf) in fns {
                recs.extend_from_slice(&[i as f32, nf[0], nf[1], nf[2]]);
                rec_count += 1;
            }
        }
    }
    let mut out = Vec::with_capacity(3 + n * 9 + tris.len() * 3 + 1 + recs.len());
    out.push(n as f32);
    out.push(tris.len() as f32);
    out.push(0.0); // noch keine Fallback-Dreiecke
    for lp in all {
        out.extend_from_slice(&[
            lp.p[0], lp.p[1], lp.p[2], lp.sdf, lp.n[0], lp.n[1], lp.n[2], lp.u, lp.flag,
        ]);
    }
    for t in tris {
        out.push(t[0] as f32);
        out.push(t[1] as f32);
        out.push(t[2] as f32);
    }
    out.push(rec_count as f32);
    out.extend_from_slice(&recs);
    out.push(open_edges.len() as f32);
    for &(a, b) in open_edges {
        out.push(a as f32);
        out.push(b as f32);
    }
    out.push(weak_tris.len() as f32);
    for t in weak_tris {
        out.push(t[0] as f32);
        out.push(t[1] as f32);
        out.push(t[2] as f32);
    }
    let long_edges = long_edges_of(all, tris);
    out.push(long_edges.len() as f32);
    for &(a, b) in &long_edges {
        out.push(a as f32);
        out.push(b as f32);
    }
    // Qualitäts-Block (fix 6 Werte, fürs Dialog-Readout): [boundaryEdges,
    // nonmanifoldEdges, degenerateTris, components, euler, orientViolations].
    let positions: Vec<[f32; 3]> = all.iter().map(|lp| lp.p).collect();
    let q = mesh_quality(&positions, tris);
    out.push(q.boundary_edges as f32);
    out.push(q.nonmanifold_edges as f32);
    out.push(q.degenerate_tris as f32);
    out.push(q.components as f32);
    out.push(q.euler as f32);
    out.push(q.orientation_violations as f32);
    out
}
