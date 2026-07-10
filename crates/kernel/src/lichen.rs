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
    /// Obergrenze der Interior-Punkte (Runaway-Schutz).
    pub max_points: usize,
    /// Advancing-Front-Triangulierung ausführen. Aus = nur Punktwolke.
    pub triangulate: bool,
}

impl Default for LichenOptions {
    fn default() -> Self {
        Self {
            target_edge_length: 4.0,
            grow: true,
            curv_angle_deg: 20.0,
            use_reach: true,
            reach_tol: 1.0,
            max_points: 20000,
            triangulate: true,
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

/// Newton-Projektion auf `f = 0` (echter Step `p -= f·∇f/|∇f|²`). Liefert
/// `(p, sdf, Einheitsnormale)`.
fn project<Fg: Fn([f32; 3]) -> (f32, [f32; 3])>(mut p: [f32; 3], fg: &Fg) -> ([f32; 3], f32, [f32; 3]) {
    for _ in 0..5 {
        let (d, g) = fg(p);
        let g2 = dot(g, g);
        if g2 < 1e-24 { return (p, d, [0.0, 0.0, 1.0]); }
        let s = d / g2;
        p = [p[0] - s * g[0], p[1] - s * g[1], p[2] - s * g[2]];
        if d.abs() < 1e-5 { break; }
    }
    let (d, g) = fg(p);
    (p, d, norm(g))
}

/// Maximale Hauptkrümmung `κ_max` UND ihre Tangentialrichtung `d_max`
/// (Shape-Operator per zentraler Differenz des Normalenfelds — Muster aus
/// `mesher_points::metric_at`). `gradn` liefert die Einheitsnormale.
fn curvature<Gn: Fn([f32; 3]) -> [f32; 3]>(p: [f32; 3], n: [f32; 3], h: f32, gradn: &Gn) -> (f32, [f32; 3]) {
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

/// Spacing-Radius `space = min(cap, R_curv, R_reach)` (§2, OHNE `R_BRep`),
/// unten geklemmt. `cap = h`. Steuert Punktdichte + Graph-Konnektivität; die
/// Kanten-Grenze übernimmt die harte `on_face`-Wand (nicht mehr die Dichte),
/// der angezeigte U(p)-Kreis wird separat via `R_BRep` beschränkt.
fn local_radius<Fg>(p: [f32; 3], n: [f32; 3], h: f32, opts: &LichenOptions, fg: &Fg) -> f32
where
    Fg: Fn([f32; 3]) -> (f32, [f32; 3]),
{
    let gradn = |q: [f32; 3]| norm(fg(q).1);
    let mut r = h;
    let (kmax, _) = curvature(p, n, h, &gradn);
    if kmax > 1e-6 {
        // R_curv = α_rad / κ_max — bindet nur, wo κ_max > α_rad/h (Radius < h/α),
        // sonst dominiert der h-Cap. Auf flachen/leicht gekrümmten Flächen also
        // wirkungslos, erst auf eng gekrümmten Bereichen greifend.
        r = r.min(opts.curv_angle_deg.to_radians() / kmax);
    }
    if opts.use_reach {
        r = r.min(reach(p, n, h, opts.reach_tol, fg));
    }
    r.clamp(0.2 * h, h)
}

#[inline]
fn gkey(p: [f32; 3], cell: f32) -> (i32, i32, i32) {
    ((p[0] / cell).floor() as i32, (p[1] / cell).floor() as i32, (p[2] / cell).floor() as i32)
}

/// Kleinster euklidischer Abstand von `p` zu einem Punkt in `pts` (Spatial-Hash).
fn nearest_dist(grid: &HashMap<(i32, i32, i32), Vec<u32>>, pts: &[LPoint], cell: f32, p: [f32; 3]) -> f32 {
    let (kx, ky, kz) = gkey(p, cell);
    let mut best = f32::MAX;
    for dx in -1..=1 {
        for dy in -1..=1 {
            for dz in -1..=1 {
                if let Some(v) = grid.get(&(kx + dx, ky + dy, kz + dz)) {
                    for &i in v {
                        let d = dist(pts[i as usize].p, p);
                        if d < best {
                            best = d;
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
pub fn grow_surface<Fg, Fb, On>(
    seeds: &[LPoint],
    h: f32,
    opts: &LichenOptions,
    fg: &Fg,
    brep_dist: &Fb,
    on_face: &On,
) -> Vec<LPoint>
where
    Fg: Fn([f32; 3]) -> (f32, [f32; 3]),
    Fb: Fn([f32; 3]) -> f32,
    On: Fn([f32; 3]) -> bool,
{
    let cell = h.max(1e-4);
    let mut pts: Vec<LPoint> = seeds.to_vec();
    let mut grid: HashMap<(i32, i32, i32), Vec<u32>> = HashMap::new();
    for (i, s) in pts.iter().enumerate() {
        grid.entry(gkey(s.p, cell)).or_default().push(i as u32);
    }
    let n_seeds = pts.len();
    let cap = n_seeds + opts.max_points;
    let gradn = |q: [f32; 3]| norm(fg(q).1);

    let mut queue: VecDeque<usize> = (0..n_seeds).collect();
    while let Some(i) = queue.pop_front() {
        if pts.len() >= cap {
            break;
        }
        let p = pts[i].p;
        let n = pts[i].n;
        let r = local_radius(p, n, h, opts, fg);
        // Referenzrichtung u entlang der maximalen Hauptkrümmung (§1.2); auf
        // planaren/sphärischen Flächen (κ≈0) ein beliebiger Tangentenvektor.
        let (kmax, dmax) = curvature(p, n, h, &gradn);
        let (u, w) = if kmax > 1e-4 {
            (dmax, cross(n, dmax))
        } else {
            tangent_basis(n)
        };
        for k in 0..6 {
            let ang = k as f32 * std::f32::consts::FRAC_PI_3;
            let dir = add(scale(u, ang.cos()), scale(w, ang.sin()));
            let (cp, cd, cn) = project(add(p, scale(dir, r)), fg);
            if cd.abs() > 0.2 * r {
                continue; // nicht auf der Oberfläche gelandet
            }
            if !visible(p, cp, r, fg) {
                continue; // hintere Wand
            }
            // Harte Kanten-Wand: der Kandidat muss auf DERSELBEN BRep-Fläche
            // liegen. Übersprang er eine Kante, ist er auf einer Nachbarfläche
            // oder außerhalb → verwerfen (Kanten sind harte Grenzen).
            if !on_face(cp) {
                continue;
            }
            // Saubere Abtastung: Kandidat näher als 0,8·r an einem bestehenden
            // Punkt → verwerfen (Spacing ≈ r; eine über-dichte Wolke bricht die
            // leere-Umkreis-Bedingung des Meshers).
            if nearest_dist(&grid, &pts, cell, cp) < 0.8 * r {
                continue;
            }
            // Eigener Spacing-Radius des Kandidaten; Anzeige-U zusätzlich durch
            // den Kantenabstand beschränkt (Kreis bleibt in der Fläche).
            let cspace = local_radius(cp, cn, h, opts, fg);
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
    pts.split_off(n_seeds)
}


/// Adaptive B-Rep-Kanten-Unterteilung (§3.2): setzt einen neuen Punkt, sobald
/// der akkumulierte **Knickwinkel** `angle_rad` (Radiant) ODER die Bogenlänge
/// `h` überschritten wird — dicht in Kurven, spärlich auf Geraden. Endpunkte
/// (die Ecken) bleiben exakt erhalten. Damit steuert der Krümmungswinkel auch
/// die Randdichte.
pub fn resample_edge_by_curvature(pts: &[[f32; 3]], h: f32, angle_rad: f32) -> Vec<[f32; 3]> {
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
    let mut out: Vec<[f32; 3]> = vec![pts[0]];
    let mut acc_len = 0.0f32;
    let mut acc_ang = 0.0f32;
    let mut push = |out: &mut Vec<[f32; 3]>, p: [f32; 3]| {
        if out.last().map_or(true, |&q| dist(q, p) > 1e-9) {
            out.push(p);
        }
    };
    for i in 1..pts.len() {
        let a = pts[i - 1];
        let b = pts[i];
        let seg = dist(a, b);
        // Längen-Unterteilung INNERHALB des Segments (Geraden spüren nur h).
        let mut t0 = 0.0f32;
        while seg > 1e-12 && acc_len + (1.0 - t0) * seg >= h {
            let t = t0 + (h - acc_len) / seg;
            push(&mut out, [
                a[0] + t * (b[0] - a[0]),
                a[1] + t * (b[1] - a[1]),
                a[2] + t * (b[2] - a[2]),
            ]);
            acc_len = 0.0;
            acc_ang = 0.0;
            t0 = t;
        }
        acc_len += (1.0 - t0) * seg;
        // Knickwinkel am inneren Vertex b (zwischen a→b und b→next).
        if i + 1 < pts.len() {
            let d1 = norm(sub(b, a));
            let d2 = norm(sub(pts[i + 1], b));
            acc_ang += dot(d1, d2).clamp(-1.0, 1.0).acos();
            if acc_ang >= ang_thresh {
                push(&mut out, b);
                acc_len = 0.0;
                acc_ang = 0.0;
            }
        }
    }
    push(&mut out, pts[pts.len() - 1]);
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
pub fn pack(all: &[LPoint], tris: &[[u32; 3]], point_faces: &[Vec<(u32, [f32; 3])>]) -> Vec<f32> {
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
    out
}
