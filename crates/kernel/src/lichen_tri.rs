//! Lichen — ALLGEMEINE (chartlose) Pro-Flächen-Triangulierung über
//! Stern-Konsistenz (Ticket-Diskussion 2026-07-11, „General"-Pfad).
//!
//! Grundidee des Betreibers: Nachbarschaft über die U(p)-Umkreise, Dreiecke
//! über die Delaunay-Bedingung — OHNE globales (u,v)-Chart, damit planare,
//! gekrümmte UND chartlose Flächen (Gyroid) denselben Code laufen.
//!
//! Warum nicht greedy Advancing Front: Das Waben-Wachstum erzeugt hexagonale
//! Muster — den maximal ENTARTETEN Fall des Umkreis-Prädikats (vier Punkte
//! fast-cozirkulär). Zwei Nachbarn werten dasselbe Viereck in minimal
//! verschiedenen Tangential-Projektionen aus und wählen verschiedene
//! Diagonalen; eine greedy Front sammelt diese widersprüchlichen Münzwürfe
//! ein (Überlappungen, Stalls). Stattdessen:
//!
//! 1. **Stern pro Punkt:** Nachbarn (Spacing-Radius) in die Tangentialebene
//!    des Punkts projizieren, dort ECHTES 2D-Delaunay (spade), Stern des
//!    Punkts extrahieren.
//! 2. **Konsistenz statt Greedy:** Ein Dreieck wird akzeptiert, wenn es im
//!    Stern ALLER drei Ecken vorkommt. Keine Reihenfolge-Abhängigkeit,
//!    keine Stall-Sackgassen.
//! 3. **Randketten sind gesetzt:** Ketten-Segmente sind fixe Kanten; kein
//!    akzeptiertes Dreieck darf eine Ketten-Kante kreuzen (2D-Test in der
//!    Dreiecks-Ebene).
//! 4. **Diagnose statt Filter** (Betreiber-Prinzip): Es gibt KEINE
//!    Qualitäts-Gates. Wo Sterne sich uneinig sind, bleibt ein Loch — und
//!    wird als solches GEZÄHLT und zurückgemeldet, nicht still übermalt.
//!
//! Auf planaren Flächen ist die Tangentialebene konstant, alle Sterne
//! stimmen überein → das Ergebnis ist ein echtes Delaunay der Fläche
//! (Referenz gegen den CDT-Pfad). Gekrümmte Flächen nutzen exakt denselben
//! Code mit per-Punkt-Normalen.

use spade::{ConstrainedDelaunayTriangulation, Point2, Triangulation};
use std::collections::{HashMap, HashSet};

/// Ergebnis der allgemeinen Triangulierung einer Fläche.
pub struct GeneralFaceMesh {
    pub tris: Vec<[u32; 3]>,
    /// Offene Nicht-Ketten-Kanten (Stern-Uneinigkeit) — die DIAGNOSE.
    pub hole_edges: usize,
    /// Kanten mit >2 Dreiecken (dürfen nicht vorkommen).
    pub nonmanifold_edges: usize,
    /// Die offenen Kanten selbst (VOR dem Loch-Schließen) — fürs Overlay (rot).
    pub open_edges: Vec<(u32, u32)>,
    /// Abgelehnte Kandidaten mit `vote_min − 1` Stimmen („fast einstimmig") —
    /// fürs Overlay (orange): zeigt, WO die Sterne sich uneinig sind.
    pub weak_tris: Vec<[u32; 3]>,
}

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
fn norm(a: [f32; 3]) -> [f32; 3] {
    let l = dot(a, a).sqrt();
    if l > 1e-12 { [a[0] / l, a[1] / l, a[2] / l] } else { [0.0, 0.0, 1.0] }
}

/// Newton-Projektion auf `f = 0` (für die Sichtlinien-Prüfung).
fn project_pt(mut p: [f32; 3], fg: &dyn Fn([f32; 3]) -> (f32, [f32; 3])) -> [f32; 3] {
    for _ in 0..4 {
        let (d, g) = fg(p);
        let g2 = dot(g, g);
        if g2 < 1e-24 {
            break;
        }
        let s = d / g2;
        p = [p[0] - s * g[0], p[1] - s * g[1], p[2] - s * g[2]];
        if d.abs() < 1e-4 {
            break;
        }
    }
    p
}

/// Unit-Tangentenbasis ⟂ n.
fn basis(n: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let seed = if n[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
    let e1 = norm(sub(seed, [n[0] * dot(seed, n), n[1] * dot(seed, n), n[2] * dot(seed, n)]));
    (e1, cross(n, e1))
}

/// 2D-Segment-Schnitt (echte Kreuzung, geteilte Endpunkte zählen nicht).
pub(crate) fn segments_cross(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> bool {
    let orient = |p: [f64; 2], q: [f64; 2], r: [f64; 2]| -> f64 {
        (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0])
    };
    let (o1, o2) = (orient(a, b, c), orient(a, b, d));
    let (o3, o4) = (orient(c, d, a), orient(c, d, b));
    o1 * o2 < 0.0 && o3 * o4 < 0.0
}

/// Ketten-Kanten-Einzelnutzung EINER Fläche erzwingen (Owner-Stitching II,
/// Betreiber-Demo): eine Ketten-Kante ist FLÄCHENRAND — pro Fläche darf
/// genau EIN Dreieck sie nutzen. Zwei Nutzungen = die Triangulierung hat
/// über den eigenen Rand gefaltet (typisch: Sliver aus 3 aufeinander-
/// folgenden Kettenpunkten auf der AUSSENSEITE der Kette; sein Zentroid
/// liegt AUF der Kante, ein SDF-Klassifikator kann ihn nicht zuordnen).
/// Konfliktregel, deterministisch: bevorzugt bleibt das Dreieck mit
/// Innen-Drittpunkt, dann das auf der Innen-SEITE der gerichteten Ring-
/// Kante (s = n × (v−u), Fläche liegt links der Ring-Richtung), zuletzt
/// das lexikographisch kleinste. Rückgabe: Anzahl entfernter Dreiecke.
pub fn enforce_chain_single_use(
    points: &[[f32; 3]],
    chains: &[Vec<u32>],
    normals: &HashMap<u32, [f32; 3]>,
    tris: &mut Vec<[u32; 3]>,
) -> usize {
    // Gerichtete Ring-Kanten: (min,max) → (u,v) in Ring-Reihenfolge.
    let mut dir_of: HashMap<(u32, u32), (u32, u32)> = HashMap::new();
    let mut chain_pts: HashSet<u32> = HashSet::new();
    for ring in chains {
        for k in 0..ring.len() {
            let a = ring[k];
            let b = ring[(k + 1) % ring.len()];
            chain_pts.insert(a);
            if a != b {
                dir_of.insert((a.min(b), a.max(b)), (a, b));
            }
        }
    }
    if dir_of.is_empty() {
        return 0;
    }
    let mut by_edge: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for (ti, &[a, b, c]) in tris.iter().enumerate() {
        for (x, y) in [(a, b), (b, c), (c, a)] {
            let e = (x.min(y), x.max(y));
            if dir_of.contains_key(&e) {
                by_edge.entry(e).or_default().push(ti);
            }
        }
    }
    let mut edges: Vec<(u32, u32)> = by_edge.keys().copied().collect();
    edges.sort_unstable();
    let mut dropped = vec![false; tris.len()];
    for e in edges {
        let alive: Vec<usize> = by_edge[&e].iter().copied().filter(|&t| !dropped[t]).collect();
        if alive.len() <= 1 {
            continue;
        }
        let (u, v) = dir_of[&e];
        let (pu, pv) = (points[u as usize], points[v as usize]);
        let mid = [(pu[0] + pv[0]) * 0.5, (pu[1] + pv[1]) * 0.5, (pu[2] + pv[2]) * 0.5];
        let n = normals.get(&u).copied().unwrap_or([0.0, 0.0, 1.0]);
        let s = cross(n, sub(pv, pu));
        let score = |ti: usize| -> (i32, std::cmp::Reverse<[u32; 3]>) {
            let [a, b, c] = tris[ti];
            let third = if (a == u || a == v) && (b == u || b == v) {
                c
            } else if (a == u || a == v) && (c == u || c == v) {
                b
            } else {
                a
            };
            let interior = i32::from(!chain_pts.contains(&third));
            let side = i32::from(dot(sub(points[third as usize], mid), s) > 0.0);
            (interior * 2 + side, std::cmp::Reverse(tris[ti]))
        };
        let keep = *alive.iter().max_by_key(|&&t| score(t)).unwrap();
        for &t in &alive {
            if t != keep {
                dropped[t] = true;
            }
        }
    }
    let before = tris.len();
    let mut k = 0usize;
    tris.retain(|_| {
        let d = dropped[k];
        k += 1;
        !d
    });
    before - tris.len()
}

/// MANIFOLD-EXTRAKTION (Cocone, Säule 3) — Analyse 2026-07-12: die
/// Kandidaten-Menge (kugel-gültige Dreiecke) enthält systematisch MEHR
/// gültige Dreiecke, als in ein 2-Manifold passen; die Loch-Forensik zeigte,
/// dass ~85 % aller offenen Kanten daran scheitern, dass eine Nachbarkante
/// bereits von zwei — ebenfalls gültigen — Dreiecken belegt ist. Greedy
/// akzeptieren + Kapazität klemmen ist also strukturell falsch: aus dem
/// Kandidaten-Komplex muss eine FLÄCHE extrahiert werden.
///
/// Verfahren (Sheet-Walk, wie Ball-Pivoting/Amenta et al.): von einem
/// Seed-Dreieck aus über gerichtete Halbkanten laufen; an jeder Kante wird
/// unter allen Kandidaten der gewählt, den man ZUERST trifft, wenn man um
/// die Kante herum durch die LUFT dreht (kleinster positiver Rotationswinkel
/// um die Kantenachse, ausgehend vom aktuellen Dreieck in Richtung seiner
/// Außennormalen). Jede Halbkante bekommt so höchstens ein Dreieck → das
/// Ergebnis ist per Konstruktion 2-manifold und konsistent orientiert;
/// Ketten-Kanten sind Flächenrand (die Front stoppt dort).
fn extract_manifold(
    points: &[[f32; 3]],
    normals: &HashMap<u32, [f32; 3]>,
    cands: &[[u32; 3]],
    chain_edges: &HashSet<(u32, u32)>,
    // PIVOT-QUELLE: alle Punkte in der Nachbarschaft einer Kante. Die
    // Stern-Kandidaten allein sind NACHWEISLICH unvollständig (deshalb gab es
    // die Vervollständigungs-Passe) — der Walk muss selbst pivotieren können,
    // sonst reißt er genau dort Löcher, wo der Stern nichts vorgeschlagen hat.
    near: &dyn Fn(u32, u32) -> Vec<u32>,
    // GÜLTIGKEIT eines pivotierten Dreiecks (leere Umkugel + Wächter).
    valid: &dyn Fn(u32, u32, u32) -> bool,
) -> Vec<[u32; 3]> {
    if cands.is_empty() {
        return Vec::new();
    }
    // Kandidaten je ungerichteter Kante.
    let mut by_edge: HashMap<(u32, u32), Vec<[u32; 3]>> = HashMap::new();
    for &t in cands {
        for (x, y) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            by_edge.entry((x.min(y), x.max(y))).or_default().push(t);
        }
    }
    let apex = |t: [u32; 3], x: u32, y: u32| -> u32 {
        if t[0] != x && t[0] != y {
            t[0]
        } else if t[1] != x && t[1] != y {
            t[1]
        } else {
            t[2]
        }
    };
    // Umkreisradius² (Seed-Wahl: kleinster zuerst = sicherstes Dreieck).
    // WICHTIG: entartete Dreiecke (Sliver) liefern hier RELATIV geprüft
    // f32::MAX — ein absoluter Test (den < 1e-20) versagt bei kleinen
    // Koordinaten: der Sliver aus drei aufeinanderfolgenden KETTENPUNKTEN
    // (kollinear, Kantenlänge ~0.1) rutschte durch und bekam sogar r = 0,
    // galt damit als BESTER Kandidat und blockierte die Fächer aller
    // Ketten-Punkte (die gemessene Walk-Sackgasse an der dünnen Platte).
    let circum_r2 = |t: [u32; 3]| -> f32 {
        let (pa, pb, pc) = (
            points[t[0] as usize],
            points[t[1] as usize],
            points[t[2] as usize],
        );
        let ab = sub(pb, pa);
        let ac = sub(pc, pa);
        let ab2 = dot(ab, ab);
        let ac2 = dot(ac, ac);
        let abac = dot(ab, ac);
        let den = 2.0 * (ab2 * ac2 - abac * abac);
        // Relativ: die Gram-Determinante gegen das Produkt der Kantenlängen²
        // (= sin²  des eingeschlossenen Winkels).
        if den.abs() <= 2e-6 * ab2 * ac2 {
            return f32::MAX;
        }
        let s_ = (ac2 * (ab2 - abac)) / den;
        let t_ = (ab2 * (ac2 - abac)) / den;
        let dc = [s_ * ab[0] + t_ * ac[0], s_ * ab[1] + t_ * ac[1], s_ * ab[2] + t_ * ac[2]];
        dot(dc, dc)
    };
    // Feld-Normale eines Dreiecks (Mittel der Ecken-Normalen).
    let n_ref = |t: [u32; 3]| -> [f32; 3] {
        let mut n = [0.0f32; 3];
        for &i in t.iter() {
            if let Some(&ni) = normals.get(&i) {
                n = [n[0] + ni[0], n[1] + ni[1], n[2] + ni[2]];
            }
        }
        n
    };
    // Seeds deterministisch: kleinster Umkreis zuerst.
    let mut order: Vec<[u32; 3]> = cands.to_vec();
    order.sort_by(|&a, &b| {
        circum_r2(a)
            .partial_cmp(&circum_r2(b))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.cmp(&b))
    });
    let mut used: HashSet<[u32; 3]> = HashSet::new(); // sortierte Tripel
    let mut half: HashSet<(u32, u32)> = HashSet::new(); // belegte Halbkanten
    let mut out: Vec<[u32; 3]> = Vec::new();
    let key3 = |t: [u32; 3]| -> [u32; 3] {
        let mut k = t;
        k.sort_unstable();
        k
    };
    // BEST-FIRST-FRONT (statt FIFO): die Front wird nach der Sicherheit des
    // aktuellen Dreiecks abgearbeitet — kleinster Umkreisradius zuerst. Ein
    // FIFO-Walk legt an unsicheren Stellen früh fest und blockiert sich damit
    // die Halbkanten der später erreichten, sicheren Regionen (gemessen:
    // 92 % der offenen Kanten hatten ein voll gültiges Dreieck, dessen
    // Halbkante schon vergeben war).
    #[derive(PartialEq)]
    struct FrontItem {
        prio: f32, // Umkreisradius² des erzeugenden Dreiecks
        tri: [u32; 3],
        x: u32,
        y: u32,
    }
    impl Eq for FrontItem {}
    impl Ord for FrontItem {
        fn cmp(&self, o: &Self) -> std::cmp::Ordering {
            // BinaryHeap ist ein MAX-Heap → invertieren (kleinster Radius zuerst),
            // deterministischer Tie-Break über die Indizes.
            o.prio
                .partial_cmp(&self.prio)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| o.tri.cmp(&self.tri))
                .then_with(|| o.x.cmp(&self.x))
                .then_with(|| o.y.cmp(&self.y))
        }
    }
    impl PartialOrd for FrontItem {
        fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
            Some(self.cmp(o))
        }
    }
    let push_tri = |t: [u32; 3],
                    prio: f32,
                    out: &mut Vec<[u32; 3]>,
                    used: &mut HashSet<[u32; 3]>,
                    half: &mut HashSet<(u32, u32)>,
                    front: &mut std::collections::BinaryHeap<FrontItem>| {
        used.insert(key3(t));
        out.push(t);
        for (x, y) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            half.insert((x, y));
            front.push(FrontItem { prio, tri: t, x, y });
        }
    };
    for &seed in &order {
        if used.contains(&key3(seed)) {
            continue;
        }
        // Bereits von der Front belegte Halbkanten dürfen kein zweites Mal
        // gesetzt werden — ein Seed, dessen Kanten schon voll sind, gehört
        // nicht mehr auf die Fläche.
        let mut s = seed;
        let nr = n_ref(s);
        let (pa, pb, pc) = (
            points[s[0] as usize],
            points[s[1] as usize],
            points[s[2] as usize],
        );
        if dot(cross(sub(pb, pa), sub(pc, pa)), nr) < 0.0 {
            s.swap(1, 2);
        }
        // Kein ROGUE-Seed: ein Startdreieck darf keine Kante berühren, die
        // bereits von der laufenden Fläche benutzt wird — sonst startet
        // mitten in einem Loch ein zweites, konkurrierendes Blatt und beide
        // belegen sich gegenseitig die Halbkanten (die gemessene
        // „Walk-Sackgasse": 92 % der offenen Kanten HÄTTEN ein voll gültiges
        // Dreieck, dessen Halbkante schon vergeben ist).
        if [(s[0], s[1]), (s[1], s[2]), (s[2], s[0])].iter().any(|&(a, b)| {
            half.contains(&(a, b)) || half.contains(&(b, a))
        }) {
            continue;
        }
        let mut front: std::collections::BinaryHeap<FrontItem> =
            std::collections::BinaryHeap::new();
        push_tri(s, circum_r2(s), &mut out, &mut used, &mut half, &mut front);
        while let Some(FrontItem { tri: t, x, y, .. }) = front.pop() {
            // Der Nachbar durchläuft die Kante GEGENLÄUFIG (y → x).
            if half.contains(&(y, x)) {
                continue; // Gegenseite bereits belegt
            }
            let e = (x.min(y), x.max(y));
            if chain_edges.contains(&e) {
                continue; // Flächenrand — die Front endet hier
            }
            // Kandidaten-Apexe: Stern-Vorschläge PLUS frei pivotierte Punkte
            // der Umgebung (die Menge ist die Vereinigung; Gültigkeit prüft
            // `valid`).
            let mut apexes: Vec<u32> = by_edge
                .get(&e)
                .map(|l| l.iter().map(|&c| apex(c, x, y)).collect())
                .unwrap_or_default();
            apexes.extend(near(x, y));
            apexes.sort_unstable();
            apexes.dedup();
            let c_cur = apex(t, x, y);
            let (px, py) = (points[x as usize], points[y as usize]);
            let axis = norm(sub(py, px));
            // Referenz-Halbebene: Richtung vom Kanten-Fußpunkt zum Apex des
            // aktuellen Dreiecks, senkrecht zur Achse.
            let perp = |q: [f32; 3]| -> [f32; 3] {
                let d = sub(q, px);
                let a = dot(d, axis);
                [d[0] - a * axis[0], d[1] - a * axis[1], d[2] - a * axis[2]]
            };
            let u_cur = norm(perp(points[c_cur as usize]));
            // Rechte-Hand-Drehung um `axis` führt von u_cur zur Außennormalen
            // des aktuellen Dreiecks — sweep durch die LUFT.
            let w = cross(axis, u_cur);
            // Nach Sweep-Winkel sortiert prüfen (Gültigkeit ist teuer, der
            // erste gültige Treffer gewinnt = echtes Pivotieren).
            let mut ranked: Vec<(f32, u32)> = Vec::new();
            for d in apexes {
                if d == c_cur || d == x || d == y {
                    continue;
                }
                let pd = perp(points[d as usize]);
                if dot(pd, pd) < 1e-12 {
                    continue; // Apex auf der Achse
                }
                let u_d = norm(pd);
                let c = dot(u_cur, u_d).clamp(-1.0, 1.0);
                let sgn = dot(w, u_d);
                let mut ang = c.acos();
                if sgn < 0.0 {
                    ang = std::f32::consts::TAU - ang;
                }
                if ang < 1e-4 || ang > std::f32::consts::TAU - 1e-4 {
                    continue; // deckungsgleich mit dem aktuellen Dreieck
                }
                ranked.push((ang, d));
            }
            ranked.sort_by(|a, b| {
                a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal).then(a.1.cmp(&b.1))
            });
            // STRIKT (Env LICHEN_WALK_STRICT): der Sweep bestimmt EINDEUTIG,
            // welches Dreieck die Fläche fortsetzt — der erste GÜLTIGE
            // Kandidat in Winkelordnung. Ist dessen Halbkante schon belegt,
            // liegt ein echter Konflikt vor: dann lieber ein ehrliches Loch
            // als den nächstbesten (falschen) Kandidaten zu nehmen, dessen
            // Fehler sich weiterfrisst (gemessene „Walk-Sackgasse").
            let strict = std::env::var("LICHEN_WALK_STRICT").is_ok();
            let mut chosen: Option<u32> = None;
            for (_, d) in ranked {
                let cand = key3([x, y, d]);
                if used.contains(&cand) {
                    continue;
                }
                if !valid(x, y, d) {
                    continue;
                }
                // Halbkanten-Kapazität: keine Doppelbelegung.
                if half.contains(&(y, x)) || half.contains(&(x, d)) || half.contains(&(d, y)) {
                    if strict {
                        break; // Konflikt → ehrliches Loch statt Fehler-Fortpflanzung
                    }
                    continue;
                }
                chosen = Some(d);
                break;
            }
            let Some(d) = chosen else { continue };
            let t_new = [y, x, d];
            let prio = circum_r2(t_new);
            push_tri(t_new, prio, &mut out, &mut used, &mut half, &mut front);
        }
    }

    // ── KONFLIKT-AUFLÖSUNG (Umbrella-Reparatur) ──
    // Die greedy Front kann sich selbst blockieren: die Innen-Triangulierung
    // wächst über eine Kante hinweg und SATURIERT sie, bevor der Fächer um
    // einen (dichteren) Ketten-Punkt geschlossen ist — gemessen sind >90 %
    // aller offenen Kanten von diesem Typ („voll gültiges Dreieck, aber
    // Halbkante belegt"; die offenen Kanten der dünnen Platte liegen alle an
    // den Ketten). Der Sweep ist aber EINDEUTIG: das von ihm bestimmte
    // Dreieck IST die Fläche. Also wird der Blocker verdrängt, wenn das
    // blockierte Dreieck strikt besser ist (kleinerer Umkreis); die dabei neu
    // entstehenden offenen Kanten laufen in der nächsten Runde nach
    // (Flip-Kaskade, deterministisch, Umkreis fällt monoton → terminiert).
    for _round in 0..8 {
        let mut open: Vec<(u32, u32)> = half
            .iter()
            .filter(|&&(a, b)| !half.contains(&(b, a)))
            .copied()
            .collect();
        open.sort_unstable();
        if std::env::var("LICHEN_DEBUG_REPAIR").is_ok() {
            eprintln!("REPAIR-RUNDE {_round}: {} offene Halbkanten", open.len());
        }
        if open.is_empty() {
            break;
        }
        let mut progress = false;
        let mut stat = [0usize; 4]; // kein-kandidat, blocker-besser, gesetzt, verdrängt
        for (a, b) in open {
            if !half.contains(&(a, b)) || half.contains(&(b, a)) {
                continue; // inzwischen geschlossen
            }
            let Some(&t_cur) = out
                .iter()
                .find(|t| [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])].contains(&(a, b)))
            else {
                continue;
            };
            let c_cur = apex(t_cur, a, b);
            let (pa, pb) = (points[a as usize], points[b as usize]);
            let axis = norm(sub(pb, pa));
            let perp = |q: [f32; 3]| -> [f32; 3] {
                let d = sub(q, pa);
                let s = dot(d, axis);
                [d[0] - s * axis[0], d[1] - s * axis[1], d[2] - s * axis[2]]
            };
            let u_cur = norm(perp(points[c_cur as usize]));
            let w = cross(axis, u_cur);
            let mut ranked: Vec<(f32, u32)> = Vec::new();
            for d in near(a, b) {
                if d == c_cur || d == a || d == b {
                    continue;
                }
                let pd = perp(points[d as usize]);
                if dot(pd, pd) < 1e-12 {
                    continue;
                }
                let u_d = norm(pd);
                let mut ang = dot(u_cur, u_d).clamp(-1.0, 1.0).acos();
                if dot(w, u_d) < 0.0 {
                    ang = std::f32::consts::TAU - ang;
                }
                if ang < 1e-4 || ang > std::f32::consts::TAU - 1e-4 {
                    continue;
                }
                ranked.push((ang, d));
            }
            ranked.sort_by(|x, y| {
                x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal).then(x.1.cmp(&y.1))
            });
            let Some(&(_, d)) = ranked.iter().find(|&&(_, d)| valid(a, b, d)) else {
                stat[0] += 1;
                continue; // ehrliches Loch: kein gültiger Kandidat
            };
            let t_new = [b, a, d];
            if used.contains(&key3(t_new)) {
                stat[0] += 1;
                continue;
            }
            let mut blockers: Vec<[u32; 3]> = Vec::new();
            for (x, y) in [(a, d), (d, b)] {
                if half.contains(&(x, y)) {
                    if let Some(&tb) = out
                        .iter()
                        .find(|t| [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])].contains(&(x, y)))
                    {
                        blockers.push(tb);
                    }
                }
            }
            let r_new = circum_r2(t_new);
            if std::env::var("LICHEN_DEBUG_REPAIR").is_ok() && !blockers.is_empty() {
                eprintln!(
                    "REPAIR ({a},{b}) neu={t_new:?} r={:.4} | Blocker={:?} r={:?} | Winkel-Rang: {}",
                    r_new.sqrt(),
                    blockers,
                    blockers.iter().map(|&t| circum_r2(t).sqrt()).collect::<Vec<_>>(),
                    ranked.iter().position(|&(_, w)| w == d).unwrap_or(999)
                );
            }
            if !blockers.is_empty()
                && !blockers.iter().all(|&tb| circum_r2(tb) > r_new * 1.0001)
            {
                stat[1] += 1;
                continue; // Blocker ist nicht schlechter → ehrliches Loch
            }
            if blockers.is_empty() {
                stat[2] += 1;
            } else {
                stat[3] += 1;
            }
            for tb in blockers {
                let k = key3(tb);
                used.remove(&k);
                for (x, y) in [(tb[0], tb[1]), (tb[1], tb[2]), (tb[2], tb[0])] {
                    half.remove(&(x, y));
                }
                out.retain(|t| key3(*t) != k);
            }
            used.insert(key3(t_new));
            out.push(t_new);
            for (x, y) in [(t_new[0], t_new[1]), (t_new[1], t_new[2]), (t_new[2], t_new[0])] {
                half.insert((x, y));
            }
            progress = true;
        }
        if std::env::var("LICHEN_DEBUG_REPAIR").is_ok() {
            eprintln!(
                "  kein-Kandidat={} Blocker-besser={} frei-gesetzt={} verdrängt={}",
                stat[0], stat[1], stat[2], stat[3]
            );
        }
        if !progress {
            break;
        }
    }
    out
}

/// GLOBALER WALK (Cocone-Programm, ein Netz über ALLE Punkte).
///
/// Analyse 2026-07-12: Die per-Fläche-Extraktion liefert auf glatten Körpern
/// wasserdichte Netze, lässt aber auf dem Demo geschlossene Loch-LOOPS stehen
/// (Gyroid 31 Kanten, Bohrung 11+13) — dort, wo der Weave die Trim-NAHT
/// gedroppt hat (non-Lipschitz-Gruppen, DL-71). Ohne B-Rep-Kante gibt es keine
/// geteilte Punkt-Kette; beide Nachbarflächen enden mit freiem Rand und ihre
/// Randpunkte sind VERSCHIEDEN → ein Schlitz im Netz. Kein Prädikat und keine
/// Abtastung kann das schließen: die Flächen-Zerlegung selbst ist der Defekt.
///
/// Der globale Walk kennt keine Flächen: er pivotiert über die GESAMTE
/// Punktwolke. An echten B-Rep-Kanten liegen die Ketten-Punkte ohnehin in
/// beiden Flächen (geteilte Indizes) — der Walk läuft dort einfach hinüber und
/// reproduziert die Kante exakt (der Sweep-durch-die-Luft trifft zwangsläufig
/// zuerst den Nachbarflächen-Punkt). An gedroppten Nähten verschwindet der
/// Spalt, weil es gar keine Wand mehr gibt. Ketten-Kanten bleiben
/// KREUZUNGS-Verbot (Feature-Linien), sind aber keine Wände mehr.
pub fn triangulate_global_walk(
    points: &[[f32; 3]],
    normals: &HashMap<u32, [f32; 3]>,
    // NORMALEN-MENGE je Punkt (eine pro angrenzender Fläche): Flächenpunkte
    // tragen genau eine, KANTENPUNKTE zwei, Ecken ≥3. Das ist die
    // „Sonderbehandlung der Kanten" (Betreiber): an einem Knick ist DIE eine
    // Normale bedeutungslos (der Mittelwert zeigt in die Winkelhalbierende),
    // die Menge der Flächen-Normalen aber exakt.
    normal_sets: &[Vec<[f32; 3]>],
    // FLÄCHEN-MENGE je Punkt (Betreiber-Idee 2026-07-12, „Flächen-Einfärbung"):
    // die B-Rep-REGIONEN, auf denen der Punkt liegt (Flächen, per Union-Find
    // verschmolzen, wo keine Kette sie trennt). Zwei Verwendungen:
    // 1. BLATT-RESTRINGIERTE Leere-Kugel (immer an): an einer fast-
    //    tangentialen Naht unterschreitet der Blattabstand den Punktabstand —
    //    Punkte einer FREMDEN Region kontaminieren die Zeugen-Kugel nicht.
    //    Ketten-Punkte tragen beide Regionen und wirken beidseitig.
    // 2. FLÄCHEN-GUARD (`face_guard`): ein Dreieck ist nur gültig, wenn seine
    //    drei Ecken eine gemeinsame Region haben — sieht tangentiale
    //    Verschneidungen, die Normalen prinzipiell nicht sehen.
    face_sets: &[Vec<u32>],
    // Kontaminator-Sicht (HOST-only für Flächenpunkte, voll für Ketten/Ecken):
    // wer eine fremde Zeugen-Kugel „verunreinigen" darf — siehe api/lichen.rs.
    contam_regions: &[Vec<u32>],
    face_guard: bool,
    spacing: &[f32],
    // Kandidaten-Horizont-UNTERGRENZE je Punkt (leer = keine): am Rand des
    // Ketten-Schutzkorridors sind die Fächer-Partner bis zu R entfernt —
    // weiter als der 2,5·Spacing-Standard-Horizont sieht.
    horizon_floor: &[f32],
    members: &[u32],
    chain_edges: &HashSet<(u32, u32)>,
    curv_angle_rad: f32,
    accept: &str,
    fg: &dyn Fn([f32; 3]) -> (f32, [f32; 3]),
) -> GeneralFaceMesh {
    if members.len() < 3 {
        return GeneralFaceMesh {
            tris: Vec::new(),
            hole_edges: 0,
            nonmanifold_edges: 0,
            open_edges: Vec::new(),
            weak_tris: Vec::new(),
        };
    }
    let cell = members
        .iter()
        .map(|&i| spacing[i as usize])
        .fold(0.1f32, f32::max);
    let key = |p: [f32; 3]| -> (i32, i32, i32) {
        (
            (p[0] / cell).floor() as i32,
            (p[1] / cell).floor() as i32,
            (p[2] / cell).floor() as i32,
        )
    };
    let mut grid: HashMap<(i32, i32, i32), Vec<u32>> = HashMap::new();
    for &i in members {
        grid.entry(key(points[i as usize])).or_default().push(i);
    }
    // Horizont wie im Stern-Pfad (dilatiertes Nachbar-Spacing).
    let mut eff_r: HashMap<u32, f32> = HashMap::new();
    for &i in members {
        let pi = points[i as usize];
        let ri = 2.5 * spacing[i as usize];
        let (kx, ky, kz) = key(pi);
        let reach = (ri / cell).ceil() as i32 + 1;
        let mut m = spacing[i as usize];
        for dx in -reach..=reach {
            for dy in -reach..=reach {
                for dz in -reach..=reach {
                    let Some(bucket) = grid.get(&(kx + dx, ky + dy, kz + dz)) else {
                        continue;
                    };
                    for &j in bucket {
                        let d = sub(points[j as usize], pi);
                        if dot(d, d) <= ri * ri {
                            m = m.max(spacing[j as usize]);
                        }
                    }
                }
            }
        }
        let floor = horizon_floor.get(i as usize).copied().unwrap_or(0.0);
        eff_r.insert(i, (2.5 * m).max(floor));
    }
    // ── NORMALEN-GUARD (Betreiber-Idee 2026-07-12) ──
    // Der Cocone-Filter prüft die RICHTUNG zum Nachbarn (muss ~tangential
    // sein). Bei zwei FAST PARALLELEN Blättern im Abstand t ist eine Sehne
    // quer über die Wand zu einem lateral entfernten Punkt aber nahezu
    // tangential (Winkel ≈ atan(t/L)) — Cocone lässt sie durch, erst recht
    // mit dem weiten Kegel, den der globale Walk für scharfe Kanten braucht.
    // Genau das ist die Gyroid-∩-Wand-Situation.
    //
    // Das verlässliche Kriterium ist der Winkel zwischen den NORMALEN der
    // beiden Endpunkte: eine gültige Kante verbindet Punkte derselben
    // Fläche, deren Normalen sich per Abtast-Bedingung um höchstens den
    // Krümmungswinkel α pro Schritt drehen (Horizont ≈ 2,5 Schritte →
    // Schwelle k·α). Auf dem Gegenblatt einer dünnen Wand sind sie
    // ~antiparallel → sicher abgelehnt, egal wie tangential die Sehne liegt.
    //
    // KANTEN: dort ist eine einzelne Normale bedeutungslos. Deshalb wird über
    // die NORMALEN-MENGE geprüft: die Kante ist gültig, wenn es EIN Paar
    // (n_i ∈ N(i), n_j ∈ N(j)) innerhalb der Schwelle gibt — an einer
    // Box-Kante passt die Normale der gemeinsamen Fläche, quer durch eine
    // dünne Wand passt keine.
    // SCHWELLE = 90° („gleiche Halbkugel"). Vermessen: eine engere Schwelle
    // (6·α ≈ 30°) lehnt auf dem GYROID legitime Nachbarn ab — dort drehen
    // sich die Flächen-Normalen benachbarter Punkte schon um ~35°. Für die
    // Blatt-Trennung ist Strenge auch gar nicht nötig: das Gegenblatt einer
    // dünnen Wand steht ~ANTIPARALLEL (180°), wird also von jeder Schwelle
    // unter 180° zuverlässig erwischt. 90° ist die prinzipielle Grenze — eine
    // gültige Flächenkante verbindet nie zwei Punkte, deren Normalen in
    // entgegengesetzte Halbräume zeigen. Messung: Demo 461 → 321 offene
    // Kanten, dünne Platte weiterhin 0 Wand-Durchquerungen und wasserdicht.
    let guard_deg: f32 = std::env::var("LICHEN_NORMAL_GUARD_DEG")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(90.0);
    let _ = curv_angle_rad;
    let guard_cos = guard_deg.to_radians().cos();
    let normals_compatible = |i: u32, j: u32| -> bool {
        let (ni, nj) = (
            normal_sets.get(i as usize).map(Vec::as_slice).unwrap_or(&[]),
            normal_sets.get(j as usize).map(Vec::as_slice).unwrap_or(&[]),
        );
        if ni.is_empty() || nj.is_empty() {
            return true; // keine Information → nicht blockieren
        }
        for &a in ni {
            for &b in nj {
                if dot(a, b) >= guard_cos {
                    return true;
                }
            }
        }
        // STRIKTE AUSNAHME (2026-07-13, Naht-Apex): am ZWICKEL einer fast-
        // tangentialen Verschneidung (Gyroid-Wand läuft spitz auf den
        // Zylinder zu) MUSS das Netz um die Spitze herum von der einen
        // Wandseite zur anderen laufen — die Endpunkt-Normalen stehen dabei
        // legitim > 90° zueinander. WHY-Forensik: 243 von 283 offenen Kanten
        // des Abnahmefalls waren voll gültige Dreiecke, die NUR der
        // Normalen-Guard blockte (Winkelklassen 90–180°).
        //
        // Der Guard existiert gegen das QUERSTICHELN dünner Wände. Sein
        // eigentliches Schlupfloch ist die θ-skalierte Sichtlinien-Toleranz
        // ((0.3·l·θ) — bei antiparallelen Normalen ist θ ≈ π und die Sehne
        // darf fast beliebig tief ins Material). Die Ausnahme schließt
        // genau dieses Loch: ein >90°-Paar ist NUR erlaubt, wenn die Sehne
        // STRIKT (nicht θ-skaliert) auf der Oberfläche liegt. Eine Sehne
        // quer durch eine Platte der Dicke t taucht ~t/2 ein und fällt
        // durch; die Zwickel-Sehne schmiegt sich an die Fläche an.
        let (pi, pj) = (points[i as usize], points[j as usize]);
        let tol = 0.15 * spacing[i as usize].min(spacing[j as usize]);
        for frac in [0.25f32, 0.5, 0.75] {
            let q = [
                pi[0] + frac * (pj[0] - pi[0]),
                pi[1] + frac * (pj[1] - pi[1]),
                pi[2] + frac * (pj[2] - pi[2]),
            ];
            if fg(q).0.abs() > tol {
                return false;
            }
        }
        true
    };

    let mut chain_segs: Vec<(u32, u32)> = chain_edges.iter().copied().collect();
    chain_segs.sort_unstable();

    // Leere Kugel (Gabriel bzw. ZEUGE) über der GESAMTEN Punktwolke.
    let circumcentre = |a: u32, b: u32, c: u32| -> Option<[f32; 3]> {
        let (pa, pb, pc) = (
            points[a as usize],
            points[b as usize],
            points[c as usize],
        );
        let ab = sub(pb, pa);
        let ac = sub(pc, pa);
        let ab2 = dot(ab, ab);
        let ac2 = dot(ac, ac);
        let abac = dot(ab, ac);
        let den = 2.0 * (ab2 * ac2 - abac * abac);
        if den.abs() < 1e-20 {
            return None;
        }
        let s_ = (ac2 * (ab2 - abac)) / den;
        let t_ = (ab2 * (ac2 - abac)) / den;
        Some([
            pa[0] + s_ * ab[0] + t_ * ac[0],
            pa[1] + s_ * ab[1] + t_ * ac[1],
            pa[2] + s_ * ab[2] + t_ * ac[2],
        ])
    };
    const TOL: f32 = 2e-4;
    // Gemeinsame REGIONEN der drei Ecken — die Blatt-Identität des Dreiecks.
    // Leer, sobald eine Ecke keine Regionen-Info hat (→ keine Restriktion,
    // konservativ streng).
    let tri_regions = |a: u32, b: u32, c: u32| -> Vec<u32> {
        let (fa, fb, fc) = (
            face_sets.get(a as usize).map(Vec::as_slice).unwrap_or(&[]),
            face_sets.get(b as usize).map(Vec::as_slice).unwrap_or(&[]),
            face_sets.get(c as usize).map(Vec::as_slice).unwrap_or(&[]),
        );
        if fa.is_empty() || fb.is_empty() || fc.is_empty() {
            return Vec::new();
        }
        fa.iter().copied().filter(|x| fb.contains(x) && fc.contains(x)).collect()
    };
    let sphere_empty = |centre: [f32; 3], a: u32, b: u32, c: u32| -> bool {
        let dc = sub(points[a as usize], centre);
        let r2 = dot(dc, dc);
        let r = r2.sqrt();
        if r > 5.0 * cell {
            return false;
        }
        // BLATT-RESTRIKTION (restricted Delaunay je glattem Patch, Cheng–Dey–
        // Levine): nur Punkte, die eine Region mit dem Dreieck teilen (oder
        // ohne Regionen-Info), zählen als Kontamination. Fremdblatt-Punkte im
        // Keil einer fast-tangentialen Naht blockieren sonst JEDEN Kandidaten
        // beider Blätter wechselseitig (Gyroid∩Zylinder: Blattabstand 0,04 mm
        // bei Punktabstand 0,1 mm). Dünne Wände bleiben geschützt: beide
        // Blätter derselben Fläche tragen dieselbe Region, und Flächen OHNE
        // trennende Kette sind zu einer Region verschmolzen.
        let tri_f = tri_regions(a, b, c);
        let (kx, ky, kz) = key(centre);
        let reach = (r / cell).ceil() as i32 + 1;
        for dx in -reach..=reach {
            for dy in -reach..=reach {
                for dz in -reach..=reach {
                    let Some(bucket) = grid.get(&(kx + dx, ky + dy, kz + dz)) else {
                        continue;
                    };
                    for &j in bucket {
                        if j == a || j == b || j == c {
                            continue;
                        }
                        let d = sub(points[j as usize], centre);
                        if dot(d, d) < r2 * (1.0 - TOL) {
                            if !tri_f.is_empty() {
                                let fj = contam_regions
                                    .get(j as usize)
                                    .map(Vec::as_slice)
                                    .unwrap_or(&[]);
                                if !fj.is_empty() && !fj.iter().any(|x| tri_f.contains(x)) {
                                    continue; // fremdes Blatt — zählt nicht
                                }
                            }
                            return false;
                        }
                    }
                }
            }
        }
        true
    };
    let empty = |a: u32, b: u32, c: u32| -> bool {
        let Some(c0) = circumcentre(a, b, c) else {
            return false;
        };
        if accept != "witness" {
            return sphere_empty(c0, a, b, c);
        }
        // ZEUGEN-Umkugel: Zentrum per 1D-Newton auf f = 0 schieben.
        let (pa, pb, pc) = (
            points[a as usize],
            points[b as usize],
            points[c as usize],
        );
        let n_t = norm(cross(sub(pb, pa), sub(pc, pa)));
        let cap = 5.0 * cell;
        let mut t = 0.0f32;
        for _ in 0..12 {
            let x = [c0[0] + t * n_t[0], c0[1] + t * n_t[1], c0[2] + t * n_t[2]];
            let (f, g) = fg(x);
            if f.abs() < 1e-3 {
                return sphere_empty(x, a, b, c);
            }
            let g = if dot(g, g) > 1e-12 { g } else { crate::lichen::robust_grad(x, &fg) };
            let dn = dot(g, n_t);
            if dn.abs() < 1e-6 {
                break;
            }
            t -= f / dn;
            if !t.is_finite() || t.abs() > cap {
                break;
            }
        }
        // KEIN Gabriel-Fallback bei Newton-Versagen — vermessen (RESEARCH_LOG
        // 2026-07-13): die ungeschobene Umkreiskugel als Ersatzkriterium
        // riss MEHR auf (384 → 478 offene Kanten), weil sie im Naht-Keil
        // Dreiecke zulässt, deren Zeugen-Zentrum gar nicht auf der Fläche
        // liegt. Ablehnen ist dort ehrlicher.
        false
    };
    // DIAGNOSE-Zwilling von `empty`: WARUM ist die Kugel nicht leer? Meldet
    // Newton-Versagen (Zeugen-Zentrum nicht auf f=0 gefunden — am Knick ist
    // ∇f unstetig!) getrennt von echter Kontamination (wer liegt drin, wie
    // tief, Ketten-Punkt?). Nur von der WHY-Forensik benutzt.
    let empty_why = |a: u32, b: u32, c: u32| -> String {
        let Some(c0) = circumcentre(a, b, c) else {
            return "kein Umkreis (degeneriert)".into();
        };
        let mut centre = c0;
        let mut note = String::new();
        if accept == "witness" {
            let (pa, pb, pc) = (
                points[a as usize],
                points[b as usize],
                points[c as usize],
            );
            let n_t = norm(cross(sub(pb, pa), sub(pc, pa)));
            let cap = 5.0 * cell;
            let mut t = 0.0f32;
            let mut found = false;
            for _ in 0..12 {
                let x = [c0[0] + t * n_t[0], c0[1] + t * n_t[1], c0[2] + t * n_t[2]];
                let (f, g) = fg(x);
                if f.abs() < 1e-3 {
                    centre = x;
                    found = true;
                    break;
                }
                let g = if dot(g, g) > 1e-12 { g } else { crate::lichen::robust_grad(x, &fg) };
                let dn = dot(g, n_t);
                if dn.abs() < 1e-6 {
                    note = format!("NEWTON-STALL→Gabriel (t={t:.4}); ");
                    break;
                }
                t -= f / dn;
                if !t.is_finite() || t.abs() > cap {
                    note = format!("NEWTON-DIVERGENZ→Gabriel (t={t:.3}); ");
                    break;
                }
            }
            if !found && note.is_empty() {
                note = "NEWTON-12-Iter→Gabriel; ".into();
            }
        }
        let chain_pts: std::collections::HashSet<u32> =
            chain_segs.iter().flat_map(|&(u, v)| [u, v]).collect();
        let dc = sub(points[a as usize], centre);
        let r2 = dot(dc, dc);
        if r2.sqrt() > 5.0 * cell {
            return format!("Radius {:.3} > 5·cell", r2.sqrt());
        }
        let tri_f = tri_regions(a, b, c);
        let mut skipped_region = 0usize;
        let mut off: Vec<(f32, u32)> = Vec::new();
        for (j, pj) in points.iter().enumerate() {
            let j = j as u32;
            if j == a || j == b || j == c {
                continue;
            }
            let d = sub(*pj, centre);
            let q = dot(d, d);
            if q < r2 * (1.0 - TOL) {
                if !tri_f.is_empty() {
                    let fj = contam_regions.get(j as usize).map(Vec::as_slice).unwrap_or(&[]);
                    if !fj.is_empty() && !fj.iter().any(|x| tri_f.contains(x)) {
                        skipped_region += 1;
                        continue;
                    }
                }
                off.push((q.sqrt() / r2.sqrt(), j));
            }
        }
        off.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap());
        let s: Vec<String> = off
            .iter()
            .take(4)
            .map(|&(ratio, j)| {
                format!(
                    "#{j}{}@{:.2}r",
                    if chain_pts.contains(&j) { "K" } else { "" },
                    ratio
                )
            })
            .collect();
        format!(
            "{note}kontaminiert: {} Punkte [{}] r={:.4} (+{skipped_region} fremdblatt-ignoriert)",
            off.len(),
            s.join(", "),
            r2.sqrt()
        )
    };
    // Ketten-Kanten sind FEATURE-Linien: kein Dreieck darf sie kreuzen (sie
    // dürfen aber Dreiecks-Kanten SEIN — nur so läuft der Walk über die Kante).
    // Liefert das ERSTE kreuzende Segment (Diagnose); `crosses_chain` ist der
    // boolesche Deckel darüber.
    let crossing_seg = |a: u32, b: u32, c: u32| -> Option<(u32, u32)> {
        let (pa, pb, pc) = (
            points[a as usize],
            points[b as usize],
            points[c as usize],
        );
        let n = norm(cross(sub(pb, pa), sub(pc, pa)));
        let (e1, e2) = basis(n);
        let to2 = |p: [f32; 3]| -> [f64; 2] {
            let d = sub(p, pa);
            [dot(d, e1) as f64, dot(d, e2) as f64]
        };
        let t2 = [to2(pa), to2(pb), to2(pc)];
        // Dreiecks-Radius (längste Kante) und Zentroid für den 3D-Nähe-Cull.
        let r_tri = {
            let mut m = 0.0f32;
            for (x, y) in [(pa, pb), (pb, pc), (pc, pa)] {
                let d = sub(y, x);
                m = m.max(dot(d, d));
            }
            m.sqrt()
        };
        let cen = [
            (pa[0] + pb[0] + pc[0]) / 3.0,
            (pa[1] + pb[1] + pc[1]) / 3.0,
            (pa[2] + pb[2] + pc[2]) / 3.0,
        ];
        // BLATT-RESTRIKTION (2026-07-13, wie die Zeugen-Kugel): eine Kette,
        // die KEINE B-Rep-Region mit dem Dreieck teilt, ist ein fremdes
        // Blatt — ihre „Kreuzung" ist eine reine Projektion (der Test
        // projiziert jedes Segment im r_tri-Umkreis in die Dreiecksebene;
        // am Zwickel liegen fremde Nähte 0,1–0,2 mm ÜBER dem Dreieck und
        // vetoen jeden legitimen Fächer — Rest-Forensik: 13 von 21 offenen
        // Kanten). Die Region des Segments = gemeinsame Regionen seiner
        // Endpunkte (die Kette liegt auf den Flächen, die beide teilen);
        // ohne Regionen-Info bleibt das Veto konservativ bestehen.
        let tri_f = tri_regions(a, b, c);
        for &(u, v) in &chain_segs {
            if u == a || u == b || u == c || v == a || v == b || v == c {
                continue;
            }
            if !tri_f.is_empty() {
                let (fu, fv) = (
                    face_sets.get(u as usize).map(Vec::as_slice).unwrap_or(&[]),
                    face_sets.get(v as usize).map(Vec::as_slice).unwrap_or(&[]),
                );
                let seg_has_common = fu.iter().any(|x| fv.contains(x));
                let shares_tri = fu.iter().any(|x| fv.contains(x) && tri_f.contains(x));
                if seg_has_common && !shares_tri {
                    continue; // fremdes Blatt — nur Projektion, kein Kreuzen
                }
            }
            let (pu, pv) = (points[u as usize], points[v as usize]);
            // 3D-NÄHE-CULL (echter Punkt-Segment-Abstand zum Zentroid). Der
            // frühere Cull nutzte `cell` = GRÖSSTES Spacing der gesamten
            // Wolke — im globalen Walk ist das h (Zentimeter!), also wurden
            // weit entfernte Ketten in die Dreiecks-Ebene projiziert und
            // „kreuzten" dort scheinbar. Auf der Demo war das mit 279 von 655
            // offenen Kanten der größte Einzelblocker.
            let seg = sub(pv, pu);
            let l2 = dot(seg, seg).max(1e-20);
            let t = (dot(sub(cen, pu), seg) / l2).clamp(0.0, 1.0);
            let q = [
                pu[0] + t * seg[0],
                pu[1] + t * seg[1],
                pu[2] + t * seg[2],
            ];
            let dq = sub(q, cen);
            if dot(dq, dq).sqrt() > r_tri {
                continue; // Kette liegt außerhalb der Dreiecks-Umgebung
            }
            // SUB-RESOLUTION-NÄHE (2026-07-13, Naht-Zwickel): läuft die
            // fremde Kette näher als ihr eigenes lokales Spacing an einer
            // DREIECKS-ECKE vorbei, ist die Trennung der beiden Ketten
            // unterhalb der Abtast-Auflösung — an Zwickeln konvergierender
            // fast-tangentialer Nähte projizieren solche Ketten quer durch
            // jeden legitimen Fächer (Rest-Forensik: ausnahmslos
            // „ketten-kreuzung"). Kein Veto; echte Kreuzungen MITTEN durch
            // ein Dreieck bleiben verboten (Abstand > Spacing zu allen
            // Ecken).
            let tol = spacing
                .get(u as usize)
                .copied()
                .unwrap_or(0.0)
                .min(spacing.get(v as usize).copied().unwrap_or(0.0));
            let near_vertex = [pa, pb, pc].iter().any(|&pw| {
                let tt = (dot(sub(pw, pu), seg) / l2).clamp(0.0, 1.0);
                let qq = [
                    pu[0] + tt * seg[0],
                    pu[1] + tt * seg[1],
                    pu[2] + tt * seg[2],
                ];
                let dd = sub(qq, pw);
                dot(dd, dd).sqrt() < tol
            });
            if near_vertex {
                continue;
            }
            // EBENEN-ABSTANDS-GATE (2026-07-13): der Test PROJIZIERT das
            // Segment in die Dreiecksebene — eine Kette, die komplett auf
            // EINER Seite der Ebene liegt und ihr nie näher kommt als die
            // halbe lokale Abtastweite, kann das Dreieck nicht durchstoßen
            // (eine echte Auf-der-Fläche-Kreuzung liegt sagitta-nah an der
            // Ebene, «0.5·spacing). An Zwickeln konvergierender Nähte
            // schweben fremde Ketten 0.09–0.14 mm ÜBER dem Dreieck und
            // vetoen sonst jeden Fächer, auch wenn die Regionen-Restriktion
            // (unten) mangels gemeinsamer Dreiecks-Region blind ist.
            let d0 = dot(sub(pu, pa), n);
            let d1 = dot(sub(pv, pa), n);
            let tol_pierce = 0.5 * tol;
            if d0 * d1 > 0.0 && d0.abs().min(d1.abs()) > tol_pierce {
                continue;
            }
            let (su, sv) = (to2(pu), to2(pv));
            for e in [(0usize, 1usize), (1, 2), (2, 0)] {
                if segments_cross(t2[e.0], t2[e.1], su, sv) {
                    return Some((u, v));
                }
            }
        }
        None
    };
    let crosses_chain = |a: u32, b: u32, c: u32| -> bool { crossing_seg(a, b, c).is_some() };
    // Gültigkeit: leere Kugel + kein Ketten-Kreuzen + Sichtlinien (Sehnen-
    // Mittelpunkte und Zentroid auf der KÖRPER-Oberfläche; im globalen Walk
    // gibt es keine „eigene Trägerfläche" mehr — das Body-Feld ist die
    // Wahrheit) + Cocone gegen fremde Blätter. `valid_geom` = alles außer
    // der leeren Kugel (das Naht-Stitching unten braucht genau diese
    // Trennung: geometrische Wahrheit ja, Abtast-Kriterium nein).
    let valid_geom = |a: u32, b: u32, c: u32| -> bool {
        let (pa, pb, pc) = (
            points[a as usize],
            points[b as usize],
            points[c as usize],
        );
        // DEGENERATIONS-GUARD: (fast) kollineare Dreiecke sind keine Fläche.
        // Der Sliver aus drei aufeinanderfolgenden KETTENPUNKTEN ist der
        // Standardfall — er bekommt einen numerisch entarteten Umkreis, gilt
        // damit in jeder Umkreis-Sortierung als „bester" Kandidat und
        // blockiert die Fächer aller Ketten-Punkte.
        {
            let ab = sub(pb, pa);
            let ac = sub(pc, pa);
            let cr = cross(ab, ac);
            let area2 = dot(cr, cr).sqrt();
            let lab = dot(ab, ab).sqrt();
            let lac = dot(ac, ac).sqrt();
            if area2 <= 1e-2 * lab * lac {
                return false; // sin(Winkel) ≤ 0.01
            }
        }
        // FLÄCHEN-GUARD (topologisch, opt-in): die drei Ecken brauchen eine
        // gemeinsame B-Rep-Region. Er sieht, was Normalen nicht sehen können —
        // zwei Flächen, die sich TANGENTIAL nähern, haben ähnliche Normalen,
        // aber verschiedene IDs.
        if face_guard {
            let (fa, fb, fc) = (
                face_sets.get(a as usize).map(Vec::as_slice).unwrap_or(&[]),
                face_sets.get(b as usize).map(Vec::as_slice).unwrap_or(&[]),
                face_sets.get(c as usize).map(Vec::as_slice).unwrap_or(&[]),
            );
            if !fa.is_empty() && !fb.is_empty() && !fc.is_empty() {
                let common = fa
                    .iter()
                    .any(|x| fb.contains(x) && fc.contains(x));
                if !common {
                    return false;
                }
            }
        }
        // NORMALEN-GUARD (s.o.): DER Blatt-Trenner. Der frühere Cocone-Kegel
        // ist hier ERSATZLOS entfallen — er prüfte die Richtung zum Nachbarn
        // gegen die GEMITTELTE Punkt-Normale, die an jeder B-Rep-Kante
        // bedeutungslos ist (sie zeigt in die Winkelhalbierende), und lehnte
        // damit legitime Kanten-Dreiecke ab (Demo: 321 → 303 offene Kanten
        // nach seiner Entfernung; die dünne Platte bleibt bei 0
        // Wand-Durchquerungen und wasserdicht). Was er leisten sollte —
        // fremde Blätter fernhalten — leistet der Normalen-Guard exakt und
        // ohne Kanten-Kollateralschaden.
        for (i, j) in [(a, b), (b, c), (c, a)] {
            if !normals_compatible(i, j) {
                return false;
            }
        }
        // SICHTLINIE je Sehne: der Mittelpunkt muss auf der Oberfläche liegen,
        // erlaubte Eintauchtiefe = Sagitta aus dem Normalen-Knick.
        for (i, j) in [(a, b), (b, c), (c, a)] {
            let (pi, pj) = (points[i as usize], points[j as usize]);
            let mid = [
                (pi[0] + pj[0]) * 0.5,
                (pi[1] + pj[1]) * 0.5,
                (pi[2] + pj[2]) * 0.5,
            ];
            let d = sub(pj, pi);
            let dl = dot(d, d).sqrt();
            let theta = match (normals.get(&i), normals.get(&j)) {
                (Some(&ni), Some(&nj)) => dot(ni, nj).clamp(-1.0, 1.0).acos(),
                _ => 0.0,
            };
            let tol = (0.3 * dl * theta).max(0.1 * spacing[i as usize]);
            if fg(mid).0.abs() > tol {
                return false;
            }
        }
        if crosses_chain(a, b, c) {
            return false;
        }
        // Zentroid auf der Oberfläche (fängt Luft-Dreiecke über Buchten).
        let cen = [
            (pa[0] + pb[0] + pc[0]) / 3.0,
            (pa[1] + pb[1] + pc[1]) / 3.0,
            (pa[2] + pb[2] + pc[2]) / 3.0,
        ];
        let e_max = {
            let mut m = 0.0f32;
            for (x, y) in [(pa, pb), (pb, pc), (pc, pa)] {
                let d = sub(y, x);
                m = m.max(dot(d, d));
            }
            m.sqrt()
        };
        let theta = {
            let mut t = 0.0f32;
            for (i, j) in [(a, b), (b, c), (c, a)] {
                if let (Some(&ni), Some(&nj)) = (normals.get(&i), normals.get(&j)) {
                    t = t.max(dot(ni, nj).clamp(-1.0, 1.0).acos());
                }
            }
            t
        };
        if fg(cen).0.abs() > (0.3 * e_max * theta).max(0.1 * spacing[a as usize]) {
            return false;
        }
        true
    };
    let valid = |a: u32, b: u32, c: u32| -> bool { valid_geom(a, b, c) && empty(a, b, c) };
    let near = |x: u32, y: u32| -> Vec<u32> {
        let (px, py) = (points[x as usize], points[y as usize]);
        let mid = [
            (px[0] + py[0]) * 0.5,
            (px[1] + py[1]) * 0.5,
            (px[2] + py[2]) * 0.5,
        ];
        let r = eff_r
            .get(&x)
            .copied()
            .unwrap_or(cell)
            .max(eff_r.get(&y).copied().unwrap_or(cell));
        let (kx, ky, kz) = key(mid);
        let reach = (r / cell).ceil() as i32 + 1;
        let mut out: Vec<u32> = Vec::new();
        for dx in -reach..=reach {
            for dy in -reach..=reach {
                for dz in -reach..=reach {
                    let Some(bucket) = grid.get(&(kx + dx, ky + dy, kz + dz)) else {
                        continue;
                    };
                    for &w in bucket {
                        if w == x || w == y {
                            continue;
                        }
                        let d = sub(points[w as usize], mid);
                        if dot(d, d) <= r * r {
                            out.push(w);
                        }
                    }
                }
            }
        }
        out
    };
    // SEEDS: der Walk braucht ein Startdreieck je Zusammenhangskomponente.
    // Seed-Kandidaten = die engsten gültigen Dreiecke; extract_manifold
    // sortiert sie selbst nach Umkreisradius und startet neu, solange
    // unbenutzte Kandidaten übrig sind.
    let mut seeds: Vec<[u32; 3]> = Vec::new();
    for &i in members {
        let pi = points[i as usize];
        let ri = eff_r[&i];
        let (kx, ky, kz) = key(pi);
        let reach = (ri / cell).ceil() as i32 + 1;
        let mut nb: Vec<(f32, u32)> = Vec::new();
        for dx in -reach..=reach {
            for dy in -reach..=reach {
                for dz in -reach..=reach {
                    let Some(bucket) = grid.get(&(kx + dx, ky + dy, kz + dz)) else {
                        continue;
                    };
                    for &j in bucket {
                        if j == i {
                            continue;
                        }
                        let d = sub(points[j as usize], pi);
                        let dd = dot(d, d);
                        if dd <= ri * ri {
                            nb.push((dd, j));
                        }
                    }
                }
            }
        }
        nb.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        nb.truncate(8);
        'seed: for a in 0..nb.len() {
            for b in a + 1..nb.len() {
                let t = [i, nb[a].1, nb[b].1];
                if valid(t[0], t[1], t[2]) {
                    seeds.push(t);
                    break 'seed;
                }
            }
        }
    }
    let mut tris = extract_manifold(points, normals, &seeds, &HashSet::new(), &near, &valid);

    let count_edges = |tris: &[[u32; 3]]| -> HashMap<(u32, u32), u32> {
        let mut edge_use: HashMap<(u32, u32), u32> = HashMap::new();
        for &[a, b, c] in tris {
            for (x, y) in [(a, b), (b, c), (c, a)] {
                *edge_use.entry((x.min(y), x.max(y))).or_default() += 1;
            }
        }
        edge_use
    };
    let mut edge_use = count_edges(&tris);
    let mut open_edges: Vec<(u32, u32)> =
        edge_use.iter().filter(|(_, &u)| u == 1).map(|(&e, _)| e).collect();
    open_edges.sort_unstable();

    // ── NAHT-STITCHING (2026-07-13, deterministischer Post-Pass) ──
    // Die letzten offenen Kanten des Abnahmefalls sind MIKRO-LOOPS (3–4
    // Kanten, jeder Rand-Vertex Grad 2) an Zwickeln konvergierender Nähte.
    // Ihre Schließ-Dreiecke sind voll orientierbar (jede offene Kante hat
    // genau EINE belegte Halbkante) und liegen strikt auf der Fläche —
    // abgelehnt wurden sie nur, weil der Zeugen-Newton am SDF-Knick nicht
    // konvergiert bzw. die Kugel im überfüllten Keil nie leer wird (beides
    // Abtast-Artefakte, keine Geometrie). Der Post-Pass schließt solche
    // Loops NUR wenn jedes Dreieck `valid_geom` besteht — Degenerations-
    // Guard, Normalen-Guard, Sichtlinien je Sehne + Zentroid (θ-skalierte
    // Sagitta-Toleranz — kein blindes Loch-Stopfen) und Ketten-Kreuzung.
    // Einzig das Zeugen-Kugel-Kriterium entfällt: das Loch ist topologisch
    // eindeutig, additiv und manifold-sicher (die neuen Dreiecke belegen
    // exakt die freien Halbkanten).
    let stitch_on = std::env::var("LICHEN_STITCH").map(|v| v != "0").unwrap_or(true);
    if stitch_on && !open_edges.is_empty() {
        let open_set: HashSet<(u32, u32)> = open_edges.iter().copied().collect();
        // Gerichteter Loch-Rand: existierende Halbkante (u→v) ⇒ das Loch
        // wird v→u durchlaufen. KANTEN-basierter Walk (2026-07-13,
        // adversariale Härtung): die frühere Vertex-Nachfolger-Map ließ
        // Vertizes mit >1 Nachfolger komplett aus — zwei Mikro-Loops, die
        // sich einen Zwickel-Vertex teilen (dünne Gyroid-Wand: zwei 5er-
        // Loops am Grad-4-Vertex), blieben so beide offen. Jetzt werden
        // HALBKANTEN konsumiert; an einem geteilten Vertex läuft der Walk
        // deterministisch über den kleinsten unverbrauchten Nachfolger —
        // eine falsche Paarung fällt unten durch valid_geom/Manifold-Schutz.
        let mut succs: HashMap<u32, Vec<u32>> = HashMap::new();
        for &[a, b, c] in &tris {
            for (x, y) in [(a, b), (b, c), (c, a)] {
                if open_set.contains(&(x.min(y), x.max(y))) {
                    succs.entry(y).or_default().push(x);
                }
            }
        }
        for v in succs.values_mut() {
            v.sort_unstable();
        }
        let stitch_ok = |a: u32, b: u32, c: u32| -> bool { valid_geom(a, b, c) };
        let mut used: HashSet<(u32, u32)> = HashSet::new(); // gerichtete Loch-Halbkante
        let mut added: Vec<[u32; 3]> = Vec::new();
        let mut starts: Vec<u32> = succs.keys().copied().collect();
        starts.sort_unstable();
        for &s0 in &starts {
            'nextloop: loop {
                // Unverbrauchte Start-Halbkante an s0 suchen.
                let Some(&n0) = succs[&s0].iter().find(|&&n| !used.contains(&(s0, n)))
                else {
                    break 'nextloop;
                };
                // Loop ablaufen (max 5 — Mikro-Löcher; größere bleiben
                // ehrlich offen). Halbkanten nur PROBEWEISE konsumieren.
                let mut ws: Vec<u32> = vec![s0, n0];
                let mut path: Vec<(u32, u32)> = vec![(s0, n0)];
                let ok = loop {
                    let cur = *ws.last().unwrap();
                    let nx = succs.get(&cur).and_then(|v| {
                        v.iter()
                            .find(|&&n| !used.contains(&(cur, n)) && !path.contains(&(cur, n)))
                            .copied()
                    });
                    let Some(nx) = nx else { break false };
                    path.push((cur, nx));
                    if nx == s0 {
                        break true;
                    }
                    if ws.contains(&nx) || ws.len() >= 5 {
                        break false;
                    }
                    ws.push(nx);
                };
                if !ok {
                    // Start-Halbkante als erledigt markieren, sonst Endlosschleife.
                    used.insert((s0, n0));
                    continue 'nextloop;
                }
                let free = |x: u32, y: u32| -> bool {
                    !edge_use.contains_key(&(x.min(y), x.max(y)))
                };
                let fill: Option<Vec<[u32; 3]>> = match ws.len() {
                    3 => {
                        let t = [ws[0], ws[1], ws[2]];
                        stitch_ok(t[0], t[1], t[2]).then(|| vec![t])
                    }
                    4 => {
                        // Diagonale wählen: beide Dreiecke müssen bestehen und
                        // die Diagonale darf noch nicht belegt sein
                        // (Manifold-Schutz).
                        let split = |i: usize| -> Option<Vec<[u32; 3]>> {
                            let (a, b, c, d) =
                                (ws[i], ws[(i + 1) % 4], ws[(i + 2) % 4], ws[(i + 3) % 4]);
                            (free(a, c) && stitch_ok(a, b, c) && stitch_ok(a, c, d))
                                .then(|| vec![[a, b, c], [a, c, d]])
                        };
                        split(0).or_else(|| split(1))
                    }
                    5 => {
                        // Fächer von einer Ecke (3 Dreiecke, 2 freie
                        // Diagonalen); alle 5 Rotationen probieren.
                        let fan = |i: usize| -> Option<Vec<[u32; 3]>> {
                            let at = |k: usize| ws[(i + k) % 5];
                            let (a, b, c, d, e) = (at(0), at(1), at(2), at(3), at(4));
                            (free(a, c)
                                && free(a, d)
                                && stitch_ok(a, b, c)
                                && stitch_ok(a, c, d)
                                && stitch_ok(a, d, e))
                            .then(|| vec![[a, b, c], [a, c, d], [a, d, e]])
                        };
                        (0..5).find_map(fan)
                    }
                    _ => None,
                };
                if let Some(ts) = fill {
                    added.extend(ts);
                    for &(x, y) in &path {
                        used.insert((x, y));
                    }
                    // Neue Diagonalen sofort als belegt führen, damit kein
                    // zweiter Loop sie non-manifold macht.
                    for t in added.iter().rev().take(ws.len() - 2) {
                        for (x, y) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                            *edge_use.entry((x.min(y), x.max(y))).or_insert(0) += 1;
                        }
                    }
                } else {
                    used.insert((s0, n0));
                }
            }
        }
        if !added.is_empty() {
            if std::env::var("LICHEN_WHY").is_ok() {
                eprintln!("STITCH: {} Mikro-Loop-Dreiecke ergänzt", added.len());
            }
            tris.extend(added);
            edge_use = count_edges(&tris);
            open_edges = edge_use.iter().filter(|(_, &u)| u == 1).map(|(&e, _)| e).collect();
            open_edges.sort_unstable();
        }
    }
    let nonmanifold_edges = edge_use.values().filter(|&&u| u > 2).count();

    // ── FORENSIK (LICHEN_WHY): warum bleibt eine Kante offen? ──
    if std::env::var("LICHEN_WHY").is_ok() && !open_edges.is_empty() {
        let mut reasons: HashMap<&'static str, usize> = HashMap::new();
        let mut guard_hist: std::collections::BTreeMap<i32, usize> = Default::default();
        // Die Apexe der BEREITS an (u,v) hängenden Dreiecke ausschließen —
        // sonst meldet die Forensik das vorhandene Dreieck als „gültigen
        // Kandidaten" und diagnostiziert eine Sackgasse, wo keine ist.
        let mut existing_apex: HashMap<(u32, u32), Vec<u32>> = HashMap::new();
        for &[a, b, c] in &tris {
            for (x, y, z) in [(a, b, c), (b, c, a), (c, a, b)] {
                existing_apex.entry((x.min(y), x.max(y))).or_default().push(z);
            }
        }
        for &(u, v) in &open_edges {
            let mut why = "kein Kandidat im Horizont";
            let mut best_state = 0u8;
            let ex = existing_apex.get(&(u.min(v), u.max(v))).cloned().unwrap_or_default();
            for w in near(u, v) {
                if w == u || w == v || ex.contains(&w) {
                    continue;
                }
                let s = if !empty(u, v, w) {
                    1 // Kugel nicht leer
                } else if crosses_chain(u, v, w) {
                    2
                } else if !valid(u, v, w) {
                    // Welcher Wächter genau? (Reihenfolge wie in `valid`)
                    let deg = {
                        let (pa, pb, pc) = (
                            points[u as usize],
                            points[v as usize],
                            points[w as usize],
                        );
                        let ab = sub(pb, pa);
                        let ac = sub(pc, pa);
                        let cr = cross(ab, ac);
                        dot(cr, cr).sqrt()
                            <= 1e-2 * dot(ab, ab).sqrt() * dot(ac, ac).sqrt()
                    };
                    let face_block = face_guard && {
                        let (fa, fb, fc) = (
                            face_sets.get(u as usize).map(Vec::as_slice).unwrap_or(&[]),
                            face_sets.get(v as usize).map(Vec::as_slice).unwrap_or(&[]),
                            face_sets.get(w as usize).map(Vec::as_slice).unwrap_or(&[]),
                        );
                        !fa.is_empty()
                            && !fb.is_empty()
                            && !fc.is_empty()
                            && !fa.iter().any(|x| fb.contains(x) && fc.contains(x))
                    };
                    if deg {
                        3
                    } else if face_block {
                        7 // FLÄCHEN-GUARD
                    } else if [(u, v), (v, w), (w, u)]
                        .iter()
                        .any(|&(i, j)| !normals_compatible(i, j))
                    {
                        4 // NORMALEN-GUARD
                    } else {
                        5 // Sichtlinie / Zentroid
                    }
                } else {
                    6 // vollständig gültig → Walk-Sackgasse
                };
                if s > best_state {
                    best_state = s;
                }
            }
            why = match best_state {
                1 => "nur nicht-leere Kugeln",
                2 => "ketten-kreuzung",
                3 => "degeneriert (Sliver)",
                4 => "NORMALEN-GUARD blockt",
                5 => "sichtlinie/zentroid blockt",
                6 => "voll gültig → Walk-Sackgasse (Halbkanten belegt)",
                7 => "FLÄCHEN-GUARD blockt",
                _ => why,
            };
            if best_state == 4 {
                // Winkel-Histogramm der Guard-Ablehnungen: >150° = echte
                // Wand-Durchquerung (korrekt), 90–120° = Grenzfall.
                let mut best_ang = 180.0f32;
                let ex2 = existing_apex.get(&(u.min(v), u.max(v))).cloned().unwrap_or_default();
                for w in near(u, v) {
                    if w == u || w == v || ex2.contains(&w) || !empty(u, v, w) {
                        continue;
                    }
                    for (i, j) in [(u, v), (v, w), (w, u)] {
                        if normals_compatible(i, j) {
                            continue;
                        }
                        let (ni, nj) = (
                            normal_sets.get(i as usize).map(Vec::as_slice).unwrap_or(&[]),
                            normal_sets.get(j as usize).map(Vec::as_slice).unwrap_or(&[]),
                        );
                        let mut best = -1.0f32;
                        for &x in ni {
                            for &y in nj {
                                best = best.max(dot(x, y));
                            }
                        }
                        best_ang = best_ang.min(best.clamp(-1.0, 1.0).acos().to_degrees());
                    }
                }
                *guard_hist.entry(((best_ang / 30.0) as i32) * 30).or_insert(0usize) += 1;
            }
            *reasons.entry(why).or_default() += 1;
            // Gezielte Diagnose um einen Ort: LICHEN_WHY_NEAR="x,y,z,r",
            // mit LICHEN_WHY_DETAIL zusätzlich die Kandidatenliste.
            if let Ok(spec) = std::env::var("LICHEN_WHY_NEAR") {
                let q: Vec<f32> = spec.split(',').filter_map(|x| x.parse().ok()).collect();
                if q.len() == 4 {
                    let (pu, pv) = (points[u as usize], points[v as usize]);
                    let m = [
                        (pu[0] + pv[0]) * 0.5,
                        (pu[1] + pv[1]) * 0.5,
                        (pu[2] + pv[2]) * 0.5,
                    ];
                    let dd = ((m[0] - q[0]).powi(2) + (m[1] - q[1]).powi(2) + (m[2] - q[2]).powi(2))
                        .sqrt();
                    if dd < q[3] {
                        eprintln!("  NEAR-OPEN #{u}{:?} – #{v}{:?} → {why}", pu, pv);
                        if std::env::var("LICHEN_WHY_DETAIL").is_ok() {
                            let mut cands: Vec<(f32, u32)> = near(u, v)
                                .into_iter()
                                .filter(|&w| w != u && w != v && !ex.contains(&w))
                                .map(|w| {
                                    let e2 = sub(points[w as usize], pu);
                                    (dot(e2, e2).sqrt(), w)
                                })
                                .collect();
                            cands.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
                            for (dw, w) in cands.into_iter().take(6) {
                                let ng: Vec<(u32, u32)> = [(u, v), (v, w), (w, u)]
                                    .into_iter()
                                    .filter(|&(i, j)| !normals_compatible(i, j))
                                    .collect();
                                let is_empty = empty(u, v, w);
                                let xseg = crossing_seg(u, v, w);
                                eprintln!(
                                    "      cand #{w} d={dw:.3} leer={} kette={:?} normguard={:?} valid={}{}",
                                    is_empty,
                                    xseg.map(|(x, y)| format!(
                                        "{x}{:?}–{y}{:?}",
                                        points[x as usize], points[y as usize]
                                    )),
                                    ng,
                                    valid(u, v, w),
                                    if is_empty {
                                        String::new()
                                    } else {
                                        format!(" | {}", empty_why(u, v, w))
                                    }
                                );
                            }
                        }
                    }
                }
            }
        }
        let mut rs: Vec<(&str, usize)> = reasons.into_iter().collect();
        rs.sort_by_key(|&(_, n)| std::cmp::Reverse(n));
        // Loop-Struktur.
        let mut adj: HashMap<u32, Vec<u32>> = HashMap::new();
        for &(u, v) in &open_edges {
            adj.entry(u).or_default().push(v);
            adj.entry(v).or_default().push(u);
        }
        let dhist: std::collections::BTreeMap<usize, usize> =
            adj.values().fold(Default::default(), |mut m, l| {
                *m.entry(l.len()).or_default() += 1;
                m
            });
        eprintln!(
            "WHY-GLOBAL {} offene Kanten: {rs:?} | Grad-Histogramm: {dhist:?}",
            open_edges.len()
        );
        if !guard_hist.is_empty() {
            eprintln!("  Guard-Ablehnungen nach Normalen-Winkel (30°-Klassen): {guard_hist:?}");
        }
        for &(u, v) in open_edges.iter().take(3) {
            let ex = existing_apex.get(&(u.min(v), u.max(v))).cloned().unwrap_or_default();
            eprintln!(
                "  OPEN {u}{:?} N={:?} — {v}{:?} N={:?}",
                points[u as usize], normal_sets.get(u as usize),
                points[v as usize], normal_sets.get(v as usize)
            );
            let mut shown = 0;
            for w in near(u, v) {
                if w == u || w == v || ex.contains(&w) {
                    continue;
                }
                let st = if !empty(u, v, w) {
                    "kugel-nicht-leer".to_string()
                } else if crosses_chain(u, v, w) {
                    "ketten-kreuzung".to_string()
                } else {
                    let bad: Vec<(u32, u32)> = [(u, v), (v, w), (w, u)]
                        .into_iter()
                        .filter(|&(i, j)| !normals_compatible(i, j))
                        .collect();
                    if !bad.is_empty() {
                        // Wie schlimm? bestes Normalen-Paar der schlechtesten Kante.
                        let worst = bad
                            .iter()
                            .map(|&(i, j)| {
                                let (ni, nj) = (
                                    normal_sets.get(i as usize).map(Vec::as_slice).unwrap_or(&[]),
                                    normal_sets.get(j as usize).map(Vec::as_slice).unwrap_or(&[]),
                                );
                                let mut best = -1.0f32;
                                for &x in ni {
                                    for &y in nj {
                                        best = best.max(dot(x, y));
                                    }
                                }
                                best
                            })
                            .fold(1.0f32, f32::min);
                        format!(
                            "NORMALEN-GUARD (bester Winkel {:.0}°)",
                            worst.clamp(-1.0, 1.0).acos().to_degrees()
                        )
                    } else if !valid(u, v, w) {
                        "cocone/sichtlinie/zentroid".to_string()
                    } else {
                        "GÜLTIG".to_string()
                    }
                };
                if st != "kugel-nicht-leer" && shown < 4 {
                    eprintln!("    Kandidat {w}{:?} N={:?} → {st}", points[w as usize], normal_sets.get(w as usize));
                    shown += 1;
                }
            }
        }
    }
    GeneralFaceMesh {
        hole_edges: open_edges.len(),
        nonmanifold_edges,
        open_edges,
        weak_tris: Vec::new(),
        tris,
    }
}

/// Allgemeine Triangulierung EINER Fläche.
///
/// * `points`/`normals`/`spacing`: globale Punkttabelle (Welt), Pro-Punkt-
///   Flächen-Normale und Spacing-Radius (`LPoint.space`).
/// * `members`: die globalen Indizes dieser Fläche (Rand + Innen).
/// * `chains`: geschlossene Rand-Ringe der Fläche (globale Indizes, Wrap
///   implizit) — ihre Segmente sind gesetzte Kanten.
/// * `curv_angle_rad`: der GLOBALE Krümmungsparameter (max. Normalen-Knick
///   pro Abtastschritt) — koppelt den Cocone-Öffnungswinkel (s.u.).
/// * `accept`: Akzeptanz-Modus `"vote" | "gabriel" | "hybrid"` (schaltbar aus
///   dem Dialog; Env `LICHEN_ACCEPT` überstimmt für Experimente).
/// * `owner`: OWNER-STITCHING — Ketten-Punkte gehören BEIDEN Nachbarflächen,
///   also kann ein Dreieck aus 3 Ketten-Punkten in beiden Sternläufen mit
///   VERSCHIEDENER Zerlegung entstehen (global non-manifold, obwohl jede
///   Fläche für sich sauber ist). Solche Voll-Ketten-Dreiecke behält nur die
///   Fläche, deren Klassifikator das Dreieckszentrum beansprucht.
pub fn triangulate_face_general(
    points: &[[f32; 3]],
    normals: &HashMap<u32, [f32; 3]>,
    spacing: &[f32],
    members: &[u32],
    chains: &[Vec<u32>],
    curv_angle_rad: f32,
    accept: &str,
    owner: Option<&dyn Fn([f32; 3]) -> bool>,
    global_chain_edges: Option<&HashSet<(u32, u32)>>,
    fill_holes: bool,
    extract_walk_opt: bool,
    fg: &dyn Fn([f32; 3]) -> (f32, [f32; 3]),
    own_near: &dyn Fn([f32; 3]) -> bool,
) -> GeneralFaceMesh {
    let member_set: HashSet<u32> = members.iter().copied().collect();

    // KEIN Koordinaten-Jitter: spades exakte Prädikate entscheiden exakt
    // cozirkuläre Hex-Quads von sich aus KONSISTENT (gleiche Koordinaten →
    // gleiche Entscheidung, einfügereihenfolge-unabhängig). Ein Versuch mit
    // Simulation-of-Simplicity-Jitter hat die Loch-Zahl der Box von 103 auf
    // 1762 VERSCHLECHTERT — er verwandelt symmetrische Ties in asymmetrische
    // Fast-Ties, die am Nachbarschafts-Horizont pro Stern verschieden kippen.


    // Ketten-Kanten (gesetzt) als sortierte Paare.
    let mut chain_edges: HashSet<(u32, u32)> = HashSet::new();
    for ring in chains {
        for k in 0..ring.len() {
            let a = ring[k];
            let b = ring[(k + 1) % ring.len()];
            if a != b {
                chain_edges.insert((a.min(b), a.max(b)));
            }
        }
    }

    // Spatial-Hash für Nachbarsuche (Zellgröße = max Spacing der Fläche).
    let cell = members
        .iter()
        .map(|&i| spacing[i as usize])
        .fold(0.1f32, f32::max);
    let key = |p: [f32; 3]| -> (i32, i32, i32) {
        (
            (p[0] / cell).floor() as i32,
            (p[1] / cell).floor() as i32,
            (p[2] / cell).floor() as i32,
        )
    };
    let mut grid: HashMap<(i32, i32, i32), Vec<u32>> = HashMap::new();
    for &i in members {
        grid.entry(key(points[i as usize])).or_default().push(i);
    }

    // ── Symmetrischer Horizont: dilatierter Nachbar-Radius ──
    // Der Stern-Vergleich braucht IDENTISCHE Zeugen-Mengen an Dichte-
    // Übergängen: ein dichter Punkt (space 0.2) neben groben Nachbarn
    // (space 3) muss deren Zeugen sehen, sonst berechnet sein Stern ein
    // anderes Delaunay als ihre. eff_r[i] = 2.5 · max(space über die
    // Nachbarschaft von i) — einmal dilatiert.
    let mut eff_r: HashMap<u32, f32> = HashMap::new();
    for &i in members {
        let pi = points[i as usize];
        let ri = 2.5 * spacing[i as usize];
        let (kx, ky, kz) = key(pi);
        let reach = (ri / cell).ceil() as i32 + 1;
        let mut m = spacing[i as usize];
        for dx in -reach..=reach {
            for dy in -reach..=reach {
                for dz in -reach..=reach {
                    let Some(bucket) = grid.get(&(kx + dx, ky + dy, kz + dz)) else {
                        continue;
                    };
                    for &j in bucket {
                        let d = sub(points[j as usize], pi);
                        if dot(d, d) <= ri * ri {
                            m = m.max(spacing[j as usize]);
                        }
                    }
                }
            }
        }
        eff_r.insert(i, 2.5 * m);
    }

    // ── Cocone-Öffnungswinkel aus dem GLOBALEN Krümmungsparameter ──
    // Ein legitimer Nachbar in Abstand r kippt maximal um ≈ r·κ aus der
    // Tangentialebene; die Abtastung garantiert space ≤ α/κ, der Horizont
    // reicht bis eff_r = 2.5·space → max. Kipp ≈ 2.5·α. Daher Halbwinkel =
    // 2.5 × curv_angle, geklemmt auf [15°, 60°]: der Boden fängt ∇f-Rauschen
    // bei sehr kleinem α ab, die Kappe hält die Blatt-Trennung (Gyroid-
    // Gegenwand ~90°) intakt. Env `LICHEN_COCONE_DEG` überstimmt (Messung).
    let cocone_deg: f32 = std::env::var("LICHEN_COCONE_DEG")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| (2.5 * curv_angle_rad.to_degrees()).clamp(15.0, 60.0));
    let cocone_sin = cocone_deg.to_radians().sin();

    // ── Stern pro Punkt: Nachbarn → Tangentialebene → 2D-Delaunay ──
    let mut chain_segs: Vec<(u32, u32)> = chain_edges.iter().copied().collect();
    chain_segs.sort_unstable();
    let mut votes: HashMap<(u32, u32, u32), u8> = HashMap::new();
    for &i in members {
        let pi = points[i as usize];
        let ni = match normals.get(&i) {
            Some(&n) => n,
            None => continue,
        };
        let (e1, e2) = basis(ni);
        let ri = eff_r[&i];
        // Kandidaten sammeln.
        let (kx, ky, kz) = key(pi);
        let reach = (ri / cell).ceil() as i32 + 1;
        let mut local: Vec<(u32, Point2<f64>)> = Vec::new();
        for dx in -reach..=reach {
            for dy in -reach..=reach {
                for dz in -reach..=reach {
                    let Some(bucket) = grid.get(&(kx + dx, ky + dy, kz + dz)) else {
                        continue;
                    };
                    for &j in bucket {
                        if j == i {
                            continue;
                        }
                        let pj = points[j as usize];
                        let d = sub(pj, pi);
                        let r = ri.max(eff_r[&j]);
                        if dot(d, d) > r * r {
                            continue;
                        }
                        // COCONE (Amenta/Bern; Betreiber-Bedingung): Nachbarn
                        // müssen „fast koplanar" zur Tangentialebene liegen —
                        // |⟨d,n⟩| ≤ sin(cocone)·|d|, Winkel an den globalen
                        // Krümmungsparameter gekoppelt (s.o.). Ein fremdes
                        // Blatt (Gyroid-Gegenwand) liegt im Doppelkegel um die
                        // Normale und wird NIE verbunden; U(p) darf dadurch
                        // größer sein als der Blatt-Abstand.
                        let dl = dot(d, d).sqrt();
                        if dot(d, ni).abs() > cocone_sin * dl {
                            continue;
                        }
                        // SDF-SICHTLINIE (Konzept §1 Schritt 6): die Sehne
                        // p→q muss nahe der OBERFLÄCHE bleiben. Punkte über
                        // eine konkave Bucht (Luft) oder QUER DURCH eine
                        // Kugelkappe (Material!) sind in 3D nah, aber
                        // flächenfremd. Die erlaubte Eintauchtiefe ist die
                        // SAGITTA — und die folgt aus dem NORMALEN-KNICK der
                        // Endpunkte (Kreisbogen: L²/8r = L·θ/8): parallele
                        // Normalen (flache Wand, Durchquerungs-Sehne) → ~0
                        // erlaubt; kippende Normalen (Sehne AUF der Kugel) →
                        // genau die Kugel-Sagitta. Eine pauschale 0,15·L-
                        // Toleranz ließ Kappen-Durchquerungen durch
                        // (Betreiber-Screenshot „Kanten durch den Körper").
                        let midp = [
                            (pi[0] + pj[0]) * 0.5,
                            (pi[1] + pj[1]) * 0.5,
                            (pi[2] + pj[2]) * 0.5,
                        ];
                        let theta = {
                            let nj = normals.get(&j).copied().unwrap_or(ni);
                            dot(ni, nj).clamp(-1.0, 1.0).acos()
                        };
                        let tol = (0.3 * dl * theta).max(0.1 * spacing[i as usize]);
                        let f_mid = fg(midp).0;
                        if f_mid.abs() > tol {
                            continue;
                        }
                        // REGIONEN-ZUGEHÖRIGKEIT (Doppelkappen-Problem): der
                        // auf die Body-Oberfläche projizierte Sehnen-Mittel-
                        // punkt muss auf der EIGENEN Trägerfläche liegen.
                        // Eine Sehne von der Vorder- zur Rückkappe der in
                        // der Wand steckenden Mini-Kugel ist aus Kugel-Sicht
                        // eine LEGITIME Kugel-Sehne (Normalen/Sagitta können
                        // sie nicht ablehnen) — aber ihr Mittelpunkt
                        // projiziert auf die WAND, nicht auf die Kugel.
                        // Perf: liegt mid schon quasi AUF der Oberfläche,
                        // genügt der direkte own_near-Test (kein Newton).
                        let probe = if f_mid.abs() <= 0.05 * spacing[i as usize] {
                            midp
                        } else {
                            project_pt(midp, fg)
                        };
                        if !own_near(probe) {
                            continue;
                        }
                        local.push((
                            j,
                            Point2::new(dot(d, e1) as f64, dot(d, e2) as f64),
                        ));
                    }
                }
            }
        }
        if std::env::var("LICHEN_DEBUG_TRI").is_ok() && (i == members[members.len() / 2]) {
            eprintln!(
                "TRI-STAR mid-member {i}: local={} n={:?} p={:?} space={} eff_r={}",
                local.len(), ni, pi, spacing[i as usize], ri
            );
        }
        if local.len() < 2 {
            continue;
        }
        // 2D-CONSTRAINED-Delaunay über {Zentrum} ∪ Nachbarn; Stern =
        // Dreiecke am Zentrum. Die lokalen Ketten-Segmente sind Constraints:
        // Randketten sind GESETZTE Kanten, alle Sterne respektieren sie →
        // beide Nachbarflächen einer Kette produzieren dieselben Rand-
        // Dreieckskanten (sonst blieben Ketten-Segmente einseitig unbenutzt).
        let mut dt: ConstrainedDelaunayTriangulation<Point2<f64>> =
            ConstrainedDelaunayTriangulation::new();
        let mut back: HashMap<(u64, u64), u32> = HashMap::new();
        let mut handle_of: HashMap<u32, spade::handles::FixedVertexHandle> = HashMap::new();
        let keyf = |p: Point2<f64>| (p.x.to_bits(), p.y.to_bits());
        let centre = Point2::new(0.0, 0.0);
        if let Ok(h) = dt.insert(centre) {
            handle_of.insert(i, h);
        }
        back.insert(keyf(centre), i);
        for &(j, p2) in &local {
            if let Ok(h) = dt.insert(p2) {
                back.entry(keyf(dt.vertex(h).position())).or_insert(j);
                handle_of.entry(j).or_insert(h);
            }
        }
        for &(u, v) in &chain_segs {
            let (Some(&hu), Some(&hv)) = (handle_of.get(&u), handle_of.get(&v)) else {
                continue;
            };
            if hu != hv && dt.can_add_constraint(hu, hv) {
                dt.add_constraint(hu, hv);
            }
        }
        for f in dt.inner_faces() {
            let [a, b, c] = f.positions();
            let (Some(&ga), Some(&gb), Some(&gc)) =
                (back.get(&keyf(a)), back.get(&keyf(b)), back.get(&keyf(c)))
            else {
                continue;
            };
            if ga != i && gb != i && gc != i {
                continue; // nicht im Stern des Zentrums
            }
            if ga == gb || gb == gc || gc == ga {
                continue;
            }
            let mut t = [ga, gb, gc];
            t.sort_unstable();
            *votes.entry((t[0], t[1], t[2])).or_default() += 1;
        }
    }

    // ── Konsistenz: Dreieck braucht alle drei Sterne ──
    // ── Akzeptanz: Stern-Konsens ODER Umkugel (Gabriel) ──
    // Umkugel-Bedingung (Betreiber-Idee): die KLEINSTE Kugel durch die drei
    // Ecken (Umkreis als Äquator, Zentrum in der Dreiecks-Ebene) muss frei
    // von anderen Punkten sein. Frame-frei und symmetrisch — sie stellt die
    // Eindeutigkeit her, die per-Stern-Tangentialebenen auf gekrümmten
    // Flächen verlieren; im ebenen Fall degeneriert sie zum Umkreis-Test.
    // Tie-Break (Betreiber-Frage „stellt die Umkugel Eindeutigkeit her?"):
    // fast — bei exakt cozirkulären Punktmengen (Hex-Quads des Waben-
    // Wachstums) liegen ALLE auf EINER Kugel, beide Diagonalen sind leer.
    // Deshalb DETERMINISTISCHER Fächer-Tie-Break: Zeugen AUF der Kugel
    // (|d²−r²| ≤ tol·r²) bilden mit dem Dreieck die Tie-Menge S; kanonische
    // Triangulierung von S = Fächer vom kleinsten globalen Index m aus, in
    // zyklischer Winkel-Reihenfolge um das Umkreiszentrum. Ein Dreieck wird
    // nur akzeptiert, wenn es GENAU ein Fächer-Dreieck (m, s_k, s_{k+1}) ist —
    // beide Sichten desselben Quads wählen so dieselbe Diagonale.
    const GABRIEL_TOL: f32 = 2e-4;
    // Umkreiszentrum in der Dreiecks-Ebene (Kramer über Baryzentrik).
    let circumcentre = |a: u32, b: u32, c: u32| -> Option<[f32; 3]> {
        let (pa, pb, pc) = (
            points[a as usize],
            points[b as usize],
            points[c as usize],
        );
        let ab = sub(pb, pa);
        let ac = sub(pc, pa);
        let ab2 = dot(ab, ab);
        let ac2 = dot(ac, ac);
        let abac = dot(ab, ac);
        let den = 2.0 * (ab2 * ac2 - abac * abac);
        if den.abs() < 1e-20 {
            return None; // degeneriert
        }
        let s_ = (ac2 * (ab2 - abac)) / den;
        let t_ = (ab2 * (ac2 - abac)) / den;
        Some([
            pa[0] + s_ * ab[0] + t_ * ac[0],
            pa[1] + s_ * ab[1] + t_ * ac[1],
            pa[2] + s_ * ab[2] + t_ * ac[2],
        ])
    };
    // Leere-Prüfung EINER Kugel durch (a,b,c) mit gegebenem Zentrum, inkl.
    // deterministischem Fächer-Tie-Break (siehe Kommentar oben). Geteilt von
    // Gabriel (Zentrum in der Dreiecks-Ebene = kleinste Kugel) und der
    // ZEUGEN-Umkugel (Zentrum auf der Fläche = restricted Delaunay).
    let sphere_empty_fan = |centre: [f32; 3], a: u32, b: u32, c: u32| -> bool {
        let (pa, pb, pc) = (
            points[a as usize],
            points[b as usize],
            points[c as usize],
        );
        let dc = sub(pa, centre);
        let r2 = dot(dc, dc);
        let r = r2.sqrt();
        // PERF-/SINN-DECKEL: fast-kollineare Kandidaten haben explodierende
        // Umkreisradien — der Punktfreiheits-Scan iterierte dann reach³ mit
        // riesigem r (Betreiber-Fall: EINE Gyroid-Fläche 241 s). Ein Dreieck
        // mit Umkugel weit über dem Nachbarschafts-Horizont ist ohnehin nie
        // ein lokales (restricted-)Delaunay-Dreieck → ablehnen statt scannen.
        if r > 5.0 * cell {
            return false;
        }
        // Punktfreiheit lokal prüfen (Spatial-Hash); Zeugen im Tie-Band sammeln.
        let mut ties: Vec<u32> = Vec::new();
        let (kx, ky, kz) = key(centre);
        let reach = (r / cell).ceil() as i32 + 1;
        for dx in -reach..=reach {
            for dy in -reach..=reach {
                for dz in -reach..=reach {
                    let Some(bucket) = grid.get(&(kx + dx, ky + dy, kz + dz)) else {
                        continue;
                    };
                    for &j in bucket {
                        if j == a || j == b || j == c {
                            continue;
                        }
                        let d = sub(points[j as usize], centre);
                        let d2 = dot(d, d);
                        if d2 < r2 * (1.0 - GABRIEL_TOL) {
                            return false; // strikt innen → Kugel nicht leer
                        }
                        if d2 <= r2 * (1.0 + GABRIEL_TOL) {
                            ties.push(j); // exakt auf der Kugel → Tie-Zeuge
                        }
                    }
                }
            }
        }
        if ties.is_empty() {
            return true; // strikt leer — eindeutig
        }
        // Fächer-Tie-Break über die zyklische Ordnung der Tie-Menge.
        let n = norm(cross(sub(pb, pa), sub(pc, pa)));
        let (e1, e2) = basis(n);
        let mut set: Vec<u32> = vec![a, b, c];
        set.extend(ties);
        set.sort_unstable();
        set.dedup();
        let m = set[0];
        // Winkel um das Zentrum in der Dreiecks-Ebene; bei exakt gleichem
        // Winkel entscheidet der Index (stabil, framefrei bis auf die
        // Dreiecks-Ebene — die für alle Sichten desselben Quads gleich ist).
        let ang = |i: u32| -> f64 {
            let d = sub(points[i as usize], centre);
            (dot(d, e2) as f64).atan2(dot(d, e1) as f64)
        };
        let mut ordered: Vec<u32> = set.clone();
        ordered.sort_by(|&x, &y| {
            ang(x).partial_cmp(&ang(y)).unwrap_or(std::cmp::Ordering::Equal).then(x.cmp(&y))
        });
        // Rotieren, sodass m vorn steht; Fächer = (m, s_k, s_{k+1}).
        let pos = ordered.iter().position(|&x| x == m).unwrap_or(0);
        ordered.rotate_left(pos);
        let tri: HashSet<u32> = [a, b, c].into_iter().collect();
        if !tri.contains(&m) {
            return false;
        }
        for w in ordered[1..].windows(2) {
            if tri.contains(&w[0]) && tri.contains(&w[1]) {
                return true;
            }
        }
        false
    };
    let gabriel_empty = |a: u32, b: u32, c: u32| -> bool {
        circumcentre(a, b, c).map_or(false, |c0| sphere_empty_fan(c0, a, b, c))
    };
    // ZEUGEN-UMKUGEL (restricted Delaunay, Cocone-Programm): ein Dreieck
    // gehört zur flächen-restringierten Delaunay-Triangulierung genau dann,
    // wenn eine leere Umkugel existiert, deren Zentrum AUF der Fläche liegt —
    // das Zentrum wird entlang der dualen Geraden (Dreiecks-Normale durchs
    // Umkreiszentrum) per 1D-Newton auf f = 0 geschoben („umgekehrtes
    // Raymarching", Betreiber-Formulierung). Auf planaren Flächen ist t = 0
    // und die Bedingung degeneriert exakt zu Gabriel. Gegenüber der
    // kleinsten Kugel ist sie auf gekrümmten Flächen ZULÄSSIGER (lehnt
    // legitime Dreiecke an Dichte-Übergängen nicht ab) — es ist das
    // Prädikat, für das der Amenta/Bern-Beweis gilt.
    let witness_empty = |a: u32, b: u32, c: u32| -> bool {
        let Some(c0) = circumcentre(a, b, c) else {
            return false;
        };
        let (pa, pb, pc) = (
            points[a as usize],
            points[b as usize],
            points[c as usize],
        );
        let n_t = norm(cross(sub(pb, pa), sub(pc, pa)));
        let cap = 5.0 * cell;
        let mut t = 0.0f32;
        let mut converged = false;
        for _ in 0..12 {
            let x = [c0[0] + t * n_t[0], c0[1] + t * n_t[1], c0[2] + t * n_t[2]];
            let (f, g) = fg(x);
            if f.abs() < 1e-3 {
                converged = true;
                break;
            }
            // robust_grad: der Dual-Gradient ist EXAKT null auf planaren
            // Flächen (Abs-Dual bei 0) — ohne FD-Fallback stallte der
            // Newton dort mit dn≈0 (95 von 446 Fails im Demo-Messlauf).
            let g = if dot(g, g) > 1e-12 { g } else { crate::lichen::robust_grad(x, &fg) };
            let dn = dot(g, n_t);
            if dn.abs() < 1e-6 {
                break; // duale Gerade läuft tangential zum Feld
            }
            t -= f / dn;
            if !t.is_finite() || t.abs() > cap {
                break; // kein Flächen-Schnitt im lokalen Fenster
            }
        }
        if !converged {
            if std::env::var("LICHEN_DEBUG_WITNESS").is_ok() {
                eprintln!("WITNESS-FAIL t={t} c0={c0:?}");
            }
            return false; // kein Zeuge auf der Fläche → nicht restricted-Delaunay
        }
        sphere_empty_fan(
            [c0[0] + t * n_t[0], c0[1] + t * n_t[1], c0[2] + t * n_t[2]],
            a,
            b,
            c,
        )
    };
    // Akzeptanz-Modus: aus dem Dialog (`LichenOptions.accept`), Env
    // `LICHEN_ACCEPT` überstimmt für Messreihen. "gabriel" = Umkugel mit
    // deterministischem Fächer-Tie-Break (s.o.), "hybrid" = Stern-Konsens
    // ODER Umkugel. Kandidaten liefern in allen Modi die Sterne.
    let accept_mode = std::env::var("LICHEN_ACCEPT").unwrap_or_else(|_| accept.to_string());
    let vote_min: u8 = std::env::var("LICHEN_VOTE_MIN")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    let accepted = |a: u32, b: u32, c: u32, v: u8| -> bool {
        match accept_mode.as_str() {
            "gabriel" => gabriel_empty(a, b, c),
            "witness" => witness_empty(a, b, c),
            "hybrid" => v >= vote_min || gabriel_empty(a, b, c),
            _ => v >= vote_min,
        }
    };
    // Kugel-Prädikat der Kandidaten-Vervollständigung: im witness-Modus
    // verifiziert auch sie mit der Zeugen-Umkugel (EIN Prädikat überall).
    let sphere_verify = |a: u32, b: u32, c: u32| -> bool {
        if accept_mode == "witness" { witness_empty(a, b, c) } else { gabriel_empty(a, b, c) }
    };
    // MANIFOLD-EXTRAKTION (Env LICHEN_EXTRACT=walk, Analyse s. extract_manifold):
    // statt greedy zu akzeptieren und die Kanten-Kapazität zu klemmen, wird aus
    // der Kandidaten-Menge eine FLÄCHE herausgelaufen. Kandidaten = alle
    // kugel-gültigen Dreiecke (das Prädikat ist laut Forensik nicht das Problem).
    let extract_walk =
        extract_walk_opt || std::env::var("LICHEN_EXTRACT").as_deref() == Ok("walk");
    let mut tris: Vec<[u32; 3]> = Vec::new();
    if std::env::var("LICHEN_DEBUG_TRI").is_ok() {
        let mut hist = [0usize; 4];
        for &v in votes.values() {
            hist[(v as usize).min(3)] += 1;
        }
        eprintln!(
            "TRI votes={} hist(1/2/3+)={}/{}/{} mode={accept_mode} members={} eff_r0={:?}",
            votes.len(), hist[1], hist[2], hist[3], members.len(),
            members.first().map(|m| eff_r.get(m).copied())
        );
    }
    // Abgelehnte Fast-Konsens-Kandidaten (vote_min − 1 Stimmen) — Diagnose-
    // Overlay (orange): genau die Stellen, an denen die Sterne uneins sind.
    let mut weak_tris: Vec<[u32; 3]> = Vec::new();
    for (&(a, b, c), &v) in votes.iter() {
        if accepted(a, b, c, v) {
            tris.push([a, b, c]);
        } else if v + 1 >= vote_min {
            weak_tris.push([a, b, c]);
        }
    }
    tris.sort_unstable();
    weak_tris.sort_unstable();
    // Im Walk-Modus sind die akzeptierten Dreiecke nur KANDIDATEN — die
    // Fläche wird daraus extrahiert (nach den geometrischen Gültigkeits-
    // Wächtern unten, aber VOR den Kapazitäts-/Seiten-Regeln, die der Walk
    // strukturell ersetzt).

    // ── Owner-Stitching: Voll-Ketten-Dreiecke nur für die eigene Fläche ──
    if let Some(owner) = owner {
        let chain_pts: HashSet<u32> =
            chains.iter().flat_map(|r| r.iter().copied()).collect();
        tris.retain(|&[a, b, c]| {
            if !(chain_pts.contains(&a) && chain_pts.contains(&b) && chain_pts.contains(&c)) {
                return true; // mindestens ein Innen-Punkt → eindeutig unsere
            }
            let (pa, pb, pc) = (
                points[a as usize],
                points[b as usize],
                points[c as usize],
            );
            owner([
                (pa[0] + pb[0] + pc[0]) / 3.0,
                (pa[1] + pb[1] + pc[1]) / 3.0,
                (pa[2] + pb[2] + pc[2]) / 3.0,
            ])
        });
    }

    // ── Ketten-Kreuzungs-Verbot: kein Dreieck über eine gesetzte Kante ──
    // 2D-Test in der Ebene des jeweiligen Dreiecks (symmetrisch für alle
    // Beteiligten). Nur Ketten-Segmente in der Nähe prüfen. Als Closure,
    // damit auch die Kandidaten-Vervollständigung (unten) ihn nutzt.
    let crosses_chain = |a: u32, b: u32, c: u32| -> bool {
        let (pa, pb, pc) = (
            points[a as usize],
            points[b as usize],
            points[c as usize],
        );
        let n = norm(cross(sub(pb, pa), sub(pc, pa)));
        let (e1, e2) = basis(n);
        let to2 = |p: [f32; 3]| -> [f64; 2] {
            let d = sub(p, pa);
            [dot(d, e1) as f64, dot(d, e2) as f64]
        };
        let t2 = [to2(pa), to2(pb), to2(pc)];
        let r_tri = {
            let mut m = 0.0f32;
            for p in [pb, pc] {
                let d = sub(p, pa);
                m = m.max(dot(d, d));
            }
            m.sqrt()
        };
        for &(u, v) in &chain_segs {
            // Segment teilt einen Eckpunkt → keine Kreuzung möglich.
            if u == a || u == b || u == c || v == a || v == b || v == c {
                continue;
            }
            let (pu, pv) = (points[u as usize], points[v as usize]);
            // Nähe-Kull: weit entfernte Ketten-Segmente überspringen.
            let du = sub(pu, pa);
            if dot(du, du).sqrt() > r_tri + cell {
                continue;
            }
            // Segment grob in die Dreiecks-Ebene projizieren und auf
            // Kreuzung mit den drei Dreiecks-Kanten testen.
            let (su, sv) = (to2(pu), to2(pv));
            for e in [(0usize, 1usize), (1, 2), (2, 0)] {
                if segments_cross(t2[e.0], t2[e.1], su, sv) {
                    return true;
                }
            }
        }
        false
    };
    tris.retain(|&[a, b, c]| !crosses_chain(a, b, c));

    // ── Fremde Ketten-Kanten sind harte Wände (Betreiber-Demo): am Ecken-
    // Cluster kann eine DRITTE Fläche ein Dreieck über die Ketten-Kante
    // eines fremden Flächenpaars spannen — für sie ist die Kante weder
    // Constraint noch Einzelnutzungs-geschützt. Global verboten. ──
    if let Some(gce) = global_chain_edges {
        tris.retain(|&[a, b, c]| {
            for (x, y) in [(a, b), (b, c), (c, a)] {
                let e = (x.min(y), x.max(y));
                if gce.contains(&e) && !chain_edges.contains(&e) {
                    return false;
                }
            }
            true
        });
    }

    if extract_walk {
        // Die geometrischen Gültigkeits-Wächter (Ketten-Kreuzung, fremde
        // Wände) sind oben gelaufen; jetzt die Fläche herauslaufen. Die
        // nachfolgenden Kapazitäts-/Seiten-/Vervollständigungs-Blöcke
        // entfallen — der Walk erzeugt per Konstruktion ein 2-Manifold.
        //
        // Pivot-Quelle: alle Punkte im Horizont der Kanten-Endpunkte, gefiltert
        // wie im Stern (Cocone-Kegel + Regionen-Sichtlinie); Gültigkeit = das
        // gewählte Kugel-Prädikat + Ketten-Regeln.
        let near = |x: u32, y: u32| -> Vec<u32> {
            let (px, py) = (points[x as usize], points[y as usize]);
            let mid = [
                (px[0] + py[0]) * 0.5,
                (px[1] + py[1]) * 0.5,
                (px[2] + py[2]) * 0.5,
            ];
            let r = eff_r
                .get(&x)
                .copied()
                .unwrap_or(cell)
                .max(eff_r.get(&y).copied().unwrap_or(cell));
            let (kx, ky, kz) = key(mid);
            let reach = (r / cell).ceil() as i32 + 1;
            let mut out: Vec<u32> = Vec::new();
            for dx in -reach..=reach {
                for dy in -reach..=reach {
                    for dz in -reach..=reach {
                        let Some(bucket) = grid.get(&(kx + dx, ky + dy, kz + dz)) else {
                            continue;
                        };
                        for &w in bucket {
                            if w == x || w == y {
                                continue;
                            }
                            let d = sub(points[w as usize], mid);
                            if dot(d, d) <= r * r {
                                out.push(w);
                            }
                        }
                    }
                }
            }
            out
        };
        let valid = |a: u32, b: u32, c: u32| -> bool {
            if crosses_chain(a, b, c) {
                return false;
            }
            if let Some(gce) = global_chain_edges {
                for (x, y) in [(a, b), (b, c), (c, a)] {
                    let e = (x.min(y), x.max(y));
                    if gce.contains(&e) && !chain_edges.contains(&e) {
                        return false;
                    }
                }
            }
            // Cocone: kein fremdes Blatt verbinden (an allen drei Ecken).
            for (i, j) in [(a, b), (b, c), (c, a)] {
                if let Some(&ni) = normals.get(&i) {
                    let d = sub(points[j as usize], points[i as usize]);
                    let dl = dot(d, d).sqrt();
                    if dl > 1e-9 && dot(d, ni).abs() > cocone_sin * dl {
                        return false;
                    }
                }
            }
            // Flächen-Treue: Zentroid auf der eigenen Trägerfläche.
            let (pa, pb, pc) = (
                points[a as usize],
                points[b as usize],
                points[c as usize],
            );
            let cen = [
                (pa[0] + pb[0] + pc[0]) / 3.0,
                (pa[1] + pb[1] + pc[1]) / 3.0,
                (pa[2] + pb[2] + pc[2]) / 3.0,
            ];
            if !own_near(project_pt(cen, fg)) {
                return false;
            }
            match accept_mode.as_str() {
                "witness" => witness_empty(a, b, c),
                _ => gabriel_empty(a, b, c),
            }
        };
        tris = extract_manifold(points, normals, &tris, &chain_edges, &near, &valid);
        enforce_chain_single_use(points, chains, normals, &mut tris);
    } else {

    // ── Ketten-Kanten-Einzelnutzung: Rand-Folds dieser Fläche entfernen ──
    enforce_chain_single_use(points, chains, normals, &mut tris);

    // ── GEGENSEITEN-PFLICHT innerer Kanten: die zwei Dreiecke einer
    // Kante müssen auf verschiedenen Seiten liegen (Tangentialrahmen der
    // Kante) — zwei gleichseitige Dreiecke sind ein Fold (Überlappung),
    // den die Orientierungs-Propagation sonst als Konflikt meldet.
    // Deterministisch: das Dreieck mit kleinerem Umkreisradius bleibt. ──
    {
        let mut by_edge: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
        for (ti, &[a, b, c]) in tris.iter().enumerate() {
            for (x, y) in [(a, b), (b, c), (c, a)] {
                by_edge.entry((x.min(y), x.max(y))).or_default().push(ti);
            }
        }
        let circum_r2 = |a: u32, b: u32, c: u32| -> f32 {
            let (pa, pb, pc) = (
                points[a as usize],
                points[b as usize],
                points[c as usize],
            );
            let ab = sub(pb, pa);
            let ac = sub(pc, pa);
            let ab2 = dot(ab, ab);
            let ac2 = dot(ac, ac);
            let abac = dot(ab, ac);
            let den = 2.0 * (ab2 * ac2 - abac * abac);
            if den.abs() < 1e-20 {
                return f32::MAX;
            }
            let s_ = (ac2 * (ab2 - abac)) / den;
            let t_ = (ab2 * (ac2 - abac)) / den;
            let dc = [s_ * ab[0] + t_ * ac[0], s_ * ab[1] + t_ * ac[1], s_ * ab[2] + t_ * ac[2]];
            dot(dc, dc)
        };
        let mut edges: Vec<(u32, u32)> = by_edge.keys().copied().collect();
        edges.sort_unstable();
        let mut dropped = vec![false; tris.len()];
        for e in edges {
            let alive: Vec<usize> =
                by_edge[&e].iter().copied().filter(|&t| !dropped[t]).collect();
            if alive.len() < 2 {
                continue;
            }
            let (u, v) = e;
            let Some(&n) = normals.get(&u) else { continue };
            let (pu, pv) = (points[u as usize], points[v as usize]);
            let mid = [(pu[0] + pv[0]) * 0.5, (pu[1] + pv[1]) * 0.5, (pu[2] + pv[2]) * 0.5];
            let sdir = cross(n, sub(pv, pu));
            let third = |ti: usize| -> u32 {
                let [a, b, c] = tris[ti];
                if a != u && a != v {
                    a
                } else if b != u && b != v {
                    b
                } else {
                    c
                }
            };
            // PRO SEITE genau ein Dreieck (deckt auch >2-Nutzungen ab —
            // non-manifolde Kanten sind immer Mehrfach-Belegung einer Seite):
            // je Seite bleibt das mit kleinstem Umkreisradius.
            let sd_len = dot(sdir, sdir).sqrt().max(1e-12);
            let mut best_per_side: [Option<(f32, [u32; 3], usize)>; 2] = [None, None];
            for &ti in &alive {
                let dw = sub(points[third(ti) as usize], mid);
                let sv = dot(dw, sdir);
                // Fast-tangentiale Drittpunkte haben ein RAUSCH-Vorzeichen —
                // nie droppen (sonst reißt ein guter Nachbar ein Loch).
                let dw_len = dot(dw, dw).sqrt().max(1e-12);
                if (sv / (sd_len * dw_len)).abs() < 0.1 {
                    continue;
                }
                let side = usize::from(sv > 0.0);
                let r2 = circum_r2(tris[ti][0], tris[ti][1], tris[ti][2]);
                let cand = (r2, tris[ti], ti);
                match &best_per_side[side] {
                    Some(b) if (b.0, b.1) <= (r2, tris[ti]) => dropped[ti] = true,
                    Some(b) => {
                        dropped[b.2] = true;
                        best_per_side[side] = Some(cand);
                    }
                    None => best_per_side[side] = Some(cand),
                }
            }
        }
        let mut k = 0usize;
        tris.retain(|_| {
            let d = dropped[k];
            k += 1;
            !d
        });
    }


    // ── KANDIDATEN-VERVOLLSTÄNDIGUNG (Teil des Basis-Algorithmus, KEIN
    // kosmetischer Füller: das Umkugel-Prädikat entscheidet): eine offene
    // Nicht-Ketten-Kante heißt, das Delaunay-Gegenstück existiert, aber
    // kein Stern hat es vorgeschlagen (Horizont-Effekt; Fächer-Partner des
    // Tie-Breaks). Für jede offene Kante wird der dritte Punkt gesucht,
    // dessen Dreieck Umkugel-verifiziert ist und keine Regel verletzt
    // (Ketten-Kreuzung, fremde Wände, Kapazität: innere Kante ≤2, eigene
    // Ketten-Kante ≤1 Nutzung). Deterministisch: kleinster Umkreisradius
    // gewinnt, Kanten in sortierter Reihenfolge, iterieren bis Fixpunkt. ──
    {
        let mut edge_use: HashMap<(u32, u32), u32> = HashMap::new();
        let mut tri_set: HashSet<[u32; 3]> = HashSet::new();
        for &[a, b, c] in &tris {
            let mut k3 = [a, b, c];
            k3.sort_unstable();
            tri_set.insert(k3);
            for (x, y) in [(a, b), (b, c), (c, a)] {
                *edge_use.entry((x.min(y), x.max(y))).or_default() += 1;
            }
        }
        let cap_of = |e: &(u32, u32)| -> u32 {
            if chain_edges.contains(e) { 1 } else { 2 }
        };
        let circum_r2 = |a: u32, b: u32, c: u32| -> f32 {
            let (pa, pb, pc) = (
                points[a as usize],
                points[b as usize],
                points[c as usize],
            );
            let ab = sub(pb, pa);
            let ac = sub(pc, pa);
            let ab2 = dot(ab, ab);
            let ac2 = dot(ac, ac);
            let abac = dot(ab, ac);
            let den = 2.0 * (ab2 * ac2 - abac * abac);
            if den.abs() < 1e-20 {
                return f32::MAX;
            }
            let s_ = (ac2 * (ab2 - abac)) / den;
            let t_ = (ab2 * (ac2 - abac)) / den;
            let dc = [s_ * ab[0] + t_ * ac[0], s_ * ab[1] + t_ * ac[1], s_ * ab[2] + t_ * ac[2]];
            dot(dc, dc)
        };
        for _round in 0..8 {
            let mut open: Vec<(u32, u32)> = edge_use
                .iter()
                .filter(|(&e, &u)| u == 1 && !chain_edges.contains(&e))
                .map(|(&e, _)| e)
                .collect();
            open.sort_unstable();
            let mut progress = false;
            for (u, v) in open {
                if edge_use.get(&(u, v)).copied().unwrap_or(0) != 1 {
                    continue; // durch frühere Ergänzung schon geschlossen
                }
                let (pu, pv) = (points[u as usize], points[v as usize]);
                let mid = [
                    (pu[0] + pv[0]) * 0.5,
                    (pu[1] + pv[1]) * 0.5,
                    (pu[2] + pv[2]) * 0.5,
                ];
                let r = eff_r.get(&u).copied().unwrap_or(cell).max(
                    eff_r.get(&v).copied().unwrap_or(cell),
                );
                let nu = normals.get(&u).copied();
                // FALSCHE-SEITE-SCHUTZ: das existierende Dreieck auf (u,v)
                // hat einen dritten Punkt x — der neue dritte Punkt w muss
                // auf der GEGENSEITE der Kante liegen, sonst überlappt das
                // neue Dreieck das alte (Fold → Orientierungs-Konflikt).
                let side_ref: Option<f32> = tris.iter().find_map(|&[a, b, c]| {
                    let has_u = a == u || b == u || c == u;
                    let has_v = a == v || b == v || c == v;
                    if !(has_u && has_v) {
                        return None;
                    }
                    let x = if a != u && a != v {
                        a
                    } else if b != u && b != v {
                        b
                    } else {
                        c
                    };
                    nu.map(|n| {
                        let s = cross(n, sub(points[v as usize], points[u as usize]));
                        dot(sub(points[x as usize], mid), s)
                    })
                });
                let (kx, ky, kz) = key(mid);
                let reach = (r / cell).ceil() as i32 + 1;
                // Billige Filter zuerst; die TEURE Umkugel-Verifikation läuft
                // nur über die ≤6 lokalsten Kandidaten (kleinster Umkreis-
                // radius zuerst) bis zum ersten Treffer.
                let mut cands: Vec<(f32, u32)> = Vec::new();
                for dx in -reach..=reach {
                    for dy in -reach..=reach {
                        for dz in -reach..=reach {
                            let Some(bucket) = grid.get(&(kx + dx, ky + dy, kz + dz)) else {
                                continue;
                            };
                            for &w in bucket {
                                if w == u || w == v {
                                    continue;
                                }
                                let d = sub(points[w as usize], mid);
                                if dot(d, d) > r * r {
                                    continue;
                                }
                                // Cocone: kein fremdes Blatt verbinden.
                                if let Some(n) = nu {
                                    let dl = dot(d, d).sqrt();
                                    if dl > 1e-9 && dot(d, n).abs() > cocone_sin * dl {
                                        continue;
                                    }
                                }
                                let mut k3 = [u, v, w];
                                k3.sort_unstable();
                                if tri_set.contains(&k3) {
                                    continue;
                                }
                                // Kapazität der beiden neuen Kanten.
                                let e1 = (u.min(w), u.max(w));
                                let e2 = (v.min(w), v.max(w));
                                if edge_use.get(&e1).copied().unwrap_or(0) >= cap_of(&e1)
                                    || edge_use.get(&e2).copied().unwrap_or(0) >= cap_of(&e2)
                                {
                                    continue;
                                }
                                // Fremde Ketten-Wände.
                                if let Some(gce) = global_chain_edges {
                                    if (gce.contains(&e1) && !chain_edges.contains(&e1))
                                        || (gce.contains(&e2) && !chain_edges.contains(&e2))
                                    {
                                        continue;
                                    }
                                }
                                // Gegenseiten-Pflicht (s.o.).
                                if let (Some(sx), Some(n)) = (side_ref, nu) {
                                    let sdir = cross(n, sub(points[v as usize], points[u as usize]));
                                    let sw = dot(sub(points[w as usize], mid), sdir);
                                    if sw * sx >= 0.0 {
                                        continue;
                                    }
                                }
                                // Degenerations-Schutz: (fast) kollineare
                                // Kandidaten stiften nur Orientierungs- und
                                // Flächen-Müll — sin(Winkel) > 1e-3 fordern.
                                let ab = sub(pv, pu);
                                let aw = sub(points[w as usize], pu);
                                let cr = cross(ab, aw);
                                let area2 = dot(cr, cr).sqrt();
                                let lab = dot(ab, ab).sqrt();
                                let law = dot(aw, aw).sqrt();
                                if area2 <= 1e-3 * lab * law {
                                    continue;
                                }
                                let r2 = circum_r2(u, v, w);
                                if r2 <= r * r {
                                    cands.push((r2, w));
                                }
                            }
                        }
                    }
                }
                cands.sort_by(|a, b| {
                    a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal).then(a.1.cmp(&b.1))
                });
                cands.truncate(6);
                // DREIECKS-LOCH-PRÄDIKAT (topologisch, kein Gabriel nötig):
                // schließt ein Kandidat ALLE DREI Kanten eines exakten
                // 3er-Lochs (beide Nachbar-Kanten ebenfalls offen), ist er
                // die EINZIGE manifold-Vervollständigung — Gabriel darf ihn
                // an Dichte-Übergängen ablehnen (dichter Zeuge knapp in der
                // Umkugel), das Loch bliebe sonst für immer (r50-Kugel).
                let closes_hole = |w: u32| -> bool {
                    let e1 = (u.min(w), u.max(w));
                    let e2 = (v.min(w), v.max(w));
                    edge_use.get(&e1) == Some(&1) && edge_use.get(&e2) == Some(&1)
                };
                let best = cands
                    .iter()
                    .copied()
                    .find(|&(_, w)| closes_hole(w) && !crosses_chain(u, v, w))
                    .or_else(|| {
                        cands
                            .into_iter()
                            .find(|&(_, w)| !crosses_chain(u, v, w) && sphere_verify(u, v, w))
                    });
                if let Some((_, w)) = best {
                    tris.push([u, v, w]);
                    let mut k3 = [u, v, w];
                    k3.sort_unstable();
                    tri_set.insert(k3);
                    for (x, y) in [(u, v), (v, w), (w, u)] {
                        *edge_use.entry((x.min(y), x.max(y))).or_default() += 1;
                    }
                    progress = true;
                }
            }
            if !progress {
                break;
            }
        }
        tris.sort_unstable();
    }

    } // Ende Nicht-Walk-Pfad

    // ── Diagnose: Kanten-Zählung (2-Manifold, Löcher) ──
    let mut edge_use: HashMap<(u32, u32), u32> = HashMap::new();
    for &[a, b, c] in &tris {
        for (x, y) in [(a, b), (b, c), (c, a)] {
            *edge_use.entry((x.min(y), x.max(y))).or_default() += 1;
        }
    }
    let nonmanifold_edges = edge_use.values().filter(|&&u| u > 2).count();
    let open_edges: Vec<(u32, u32)> = edge_use
        .iter()
        .filter(|(&e, &u)| u == 1 && !chain_edges.contains(&e) && member_set.contains(&e.0))
        .map(|(&e, _)| e)
        .collect();
    let hole_edges = open_edges.len();

    // ── LOCH-FORENSIK (Env LICHEN_WHY): für jede offene Kante den nächsten
    // Kandidaten suchen und protokollieren, WELCHER Wächter das fehlende
    // Dreieck ablehnt. Ohne diese Zuordnung ist jede Prädikat-Änderung ein
    // Schuss ins Blaue. ──
    if std::env::var("LICHEN_WHY").is_ok() && !open_edges.is_empty() {
        let mut reasons: HashMap<&'static str, usize> = HashMap::new();
        let mut guard_hist: std::collections::BTreeMap<i32, usize> = Default::default();
        for &(u, v) in &open_edges {
            let (pu, pv) = (points[u as usize], points[v as usize]);
            let mid = [
                (pu[0] + pv[0]) * 0.5,
                (pu[1] + pv[1]) * 0.5,
                (pu[2] + pv[2]) * 0.5,
            ];
            let r = eff_r.get(&u).copied().unwrap_or(cell).max(eff_r.get(&v).copied().unwrap_or(cell));
            // Kandidaten nach Umkreisradius sortiert.
            let mut cands: Vec<(f32, u32)> = Vec::new();
            let (kx, ky, kz) = key(mid);
            let reach = (r / cell).ceil() as i32 + 1;
            for dx in -reach..=reach {
                for dy in -reach..=reach {
                    for dz in -reach..=reach {
                        let Some(bucket) = grid.get(&(kx + dx, ky + dy, kz + dz)) else { continue };
                        for &w in bucket {
                            if w == u || w == v {
                                continue;
                            }
                            let d = sub(points[w as usize], mid);
                            let dd = dot(d, d);
                            if dd <= r * r {
                                cands.push((dd, w));
                            }
                        }
                    }
                }
            }
            cands.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            cands.truncate(12);
            let mut why = "kein Kandidat";
            for &(_, w) in &cands {
                if crosses_chain(u, v, w) {
                    why = "ketten-kreuzung";
                    continue;
                }
                if !(gabriel_empty(u, v, w) || witness_empty(u, v, w)) {
                    why = "kein leeres Kugel-Prädikat";
                    continue;
                }
                // Kugel-gültiger Kandidat gefunden — welcher WÄCHTER blockt?
                let e1 = (u.min(w), u.max(w));
                let e2 = (v.min(w), v.max(w));
                let cap_of = |e: &(u32, u32)| if chain_edges.contains(e) { 1 } else { 2 };
                if edge_use.get(&e1).copied().unwrap_or(0) >= cap_of(&e1)
                    || edge_use.get(&e2).copied().unwrap_or(0) >= cap_of(&e2)
                {
                    // Sind die BLOCKIERENDEN Dreiecke selbst kugel-gültig?
                    let blocker_edge =
                        if edge_use.get(&e1).copied().unwrap_or(0) >= cap_of(&e1) { e1 } else { e2 };
                    let mut all_valid = true;
                    let mut any = false;
                    for &[x, y, z] in tris.iter() {
                        let es = [
                            (x.min(y), x.max(y)),
                            (y.min(z), y.max(z)),
                            (z.min(x), z.max(x)),
                        ];
                        if es.contains(&blocker_edge) {
                            any = true;
                            if !(gabriel_empty(x, y, z) || witness_empty(x, y, z)) {
                                all_valid = false;
                            }
                        }
                    }
                    why = if !any {
                        "kapazität (kein Blocker?)"
                    } else if all_valid {
                        "kapazität — Blocker sind GÜLTIG (echter Manifold-Konflikt)"
                    } else {
                        "kapazität — Blocker ist UNGÜLTIG (falsch akzeptiert)"
                    };
                    break;
                }
                if let Some(gce) = global_chain_edges {
                    if (gce.contains(&e1) && !chain_edges.contains(&e1))
                        || (gce.contains(&e2) && !chain_edges.contains(&e2))
                    {
                        why = "fremde Ketten-Wand";
                        break;
                    }
                }
                // Cocone-Nachbarfilter (an u).
                if let Some(&nu) = normals.get(&u) {
                    let d = sub(points[w as usize], points[u as usize]);
                    let dl = dot(d, d).sqrt();
                    if dl > 1e-9 && dot(d, nu).abs() > cocone_sin * dl {
                        why = "cocone";
                        break;
                    }
                }
                // Sichtlinie/Region (wie im Stern).
                let pw = points[w as usize];
                let midw = [
                    (points[u as usize][0] + pw[0]) * 0.5,
                    (points[u as usize][1] + pw[1]) * 0.5,
                    (points[u as usize][2] + pw[2]) * 0.5,
                ];
                if !own_near(project_pt(midw, fg)) {
                    why = "regionen-sichtlinie";
                    break;
                }
                why = "kugel-ok, kein Wächter greift (Kandidaten-Lücke!)";
                break;
            }
            *reasons.entry(why).or_default() += 1;
        }
        let mut rs: Vec<(&str, usize)> = reasons.into_iter().collect();
        rs.sort_by_key(|&(_, n)| std::cmp::Reverse(n));
        eprintln!("WHY {} offene Kanten: {rs:?}", open_edges.len());
        // Loop-Struktur der Löcher: Grad-Verteilung + Loop-Längen.
        let mut deg: HashMap<u32, usize> = HashMap::new();
        for &(u, v) in &open_edges {
            *deg.entry(u).or_default() += 1;
            *deg.entry(v).or_default() += 1;
        }
        let mut adj: HashMap<u32, Vec<u32>> = HashMap::new();
        for &(u, v) in &open_edges {
            adj.entry(u).or_default().push(v);
            adj.entry(v).or_default().push(u);
        }
        let mut seen: HashSet<u32> = HashSet::new();
        let mut comps: Vec<(usize, usize)> = Vec::new(); // (verts, edges)
        for &start in adj.keys() {
            if seen.contains(&start) {
                continue;
            }
            let mut stack = vec![start];
            seen.insert(start);
            let (mut nv, mut ne) = (0usize, 0usize);
            while let Some(x) = stack.pop() {
                nv += 1;
                for &y in adj.get(&x).map(Vec::as_slice).unwrap_or(&[]) {
                    ne += 1;
                    if seen.insert(y) {
                        stack.push(y);
                    }
                }
            }
            comps.push((nv, ne / 2));
        }
        comps.sort_unstable();
        let dhist: std::collections::BTreeMap<usize, usize> =
            deg.values().fold(Default::default(), |mut m, &d| {
                *m.entry(d).or_default() += 1;
                m
            });
        eprintln!("  Loch-Loops (Ecken,Kanten): {comps:?}  Grad-Histogramm: {dhist:?}");
    }

    // ── Loch-Schließen (deterministisch, NACH der Diagnose) ──
    // Stern-Uneinigkeit bleibt an Dichte-Übergängen möglich (der dichte Punkt
    // sieht die fernen groben Zeugen eines gemischten Dreiecks nicht). Die
    // offenen Kanten bilden geschlossene Loops; Loops, die mindestens eine
    // NICHT-Ketten-Kante enthalten (= Defekt, keine gewollte Rand-/Loch-
    // Kontur), werden als Polygon im Newell-Frame per CDT gefüllt.
    if hole_edges > 0 && fill_holes && std::env::var("LICHEN_NO_FILL").is_err() {
        // Gerichtete offene Kanten: für die einzige Dreiecks-Nutzung (a,b)
        // die GEGENRICHTUNG (b,a) merken — so umläuft der Loop das Loch.
        let mut open_dir: HashMap<u32, Vec<u32>> = HashMap::new();
        for &[a, b, c] in &tris {
            for (x, y) in [(a, b), (b, c), (c, a)] {
                if edge_use[&(x.min(y), x.max(y))] == 1 {
                    open_dir.entry(y).or_default().push(x);
                }
            }
        }
        let mut visited: HashSet<(u32, u32)> = HashSet::new();
        let mut loops_to_fill: Vec<Vec<u32>> = Vec::new();
        for (&start, nexts) in &open_dir {
            for &second in nexts {
                if visited.contains(&(start, second)) {
                    continue;
                }
                // Loop laufen: von (start→second) immer die nächste offene
                // gerichtete Kante am aktuellen Endpunkt.
                let mut ring = vec![start, second];
                visited.insert((start, second));
                let mut guard = 0;
                loop {
                    guard += 1;
                    if guard > 256 {
                        ring.clear();
                        break;
                    }
                    let cur = *ring.last().unwrap();
                    let prev = ring[ring.len() - 2];
                    let Some(cands) = open_dir.get(&cur) else {
                        ring.clear();
                        break;
                    };
                    // GEOMETRISCHER Walker: an Verzweigungen (Punkt mit
                    // mehreren offenen Kanten, z.B. Acht-Kreuzungen) die
                    // schärfste LINKS-Kurve in der Tangentialebene nehmen —
                    // das läuft die minimalen Defekt-Loops ab, statt an der
                    // Kreuzung beliebig (erstbester Kandidat) falsch
                    // abzubiegen und Loops zu verschmelzen.
                    let p_prev = points[prev as usize];
                    let p_cur = points[cur as usize];
                    let n_cur = normals.get(&cur).copied().unwrap_or([0.0, 0.0, 1.0]);
                    let d_in = sub(p_cur, p_prev);
                    let turn = |n: u32| -> f64 {
                        let d_out = sub(points[n as usize], p_cur);
                        let s = dot(cross(d_in, d_out), n_cur) as f64;
                        let c = dot(d_in, d_out) as f64;
                        s.atan2(c)
                    };
                    let Some(&nxt) = cands
                        .iter()
                        .filter(|&&n| n != prev && !visited.contains(&(cur, n)))
                        .max_by(|&&n1, &&n2| {
                            turn(n1)
                                .partial_cmp(&turn(n2))
                                .unwrap_or(std::cmp::Ordering::Equal)
                                .then(n1.cmp(&n2))
                        })
                    else {
                        ring.clear();
                        break;
                    };
                    visited.insert((cur, nxt));
                    if nxt == start {
                        break; // geschlossen
                    }
                    ring.push(nxt);
                }
                if ring.len() < 3 || ring.len() > 32 {
                    // >32: das ist die legitime Rand-/Loch-Kontur der Fläche
                    // (Perimeter), kein lokaler Stern-Defekt — nicht füllen.
                    continue;
                }
                // Nur DEFEKT-Loops füllen: mindestens eine Nicht-Ketten-Kante.
                let mut has_defect = false;
                for k in 0..ring.len() {
                    let (a, b) = (ring[k], ring[(k + 1) % ring.len()]);
                    if !chain_edges.contains(&(a.min(b), a.max(b))) {
                        has_defect = true;
                        break;
                    }
                }
                if !has_defect {
                    continue;
                }
                // Ring-Validierung: nur EINFACHE Polygone füllen (keine
                // doppelten Vertices, keine sich kreuzenden Segmente im
                // Newell-Frame) — der Loop-Walker kann an Kreuzungspunkten
                // falsch abbiegen, und spade paniked auf sich schneidenden
                // Constraints.
                let unique: HashSet<u32> = ring.iter().copied().collect();
                if unique.len() != ring.len() {
                    continue;
                }
                let n = {
                    let mut nn = [0.0f64; 3];
                    for k in 0..ring.len() {
                        let p1 = points[ring[k] as usize];
                        let p2 = points[ring[(k + 1) % ring.len()] as usize];
                        nn[0] += (p1[1] as f64 - p2[1] as f64) * (p1[2] as f64 + p2[2] as f64);
                        nn[1] += (p1[2] as f64 - p2[2] as f64) * (p1[0] as f64 + p2[0] as f64);
                        nn[2] += (p1[0] as f64 - p2[0] as f64) * (p1[1] as f64 + p2[1] as f64);
                    }
                    nn
                };
                let nl = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                if nl < 1e-12 {
                    continue;
                }
                let nn = norm([
                    (n[0] / nl) as f32,
                    (n[1] / nl) as f32,
                    (n[2] / nl) as f32,
                ]);
                let (e1, e2) = basis(nn);
                let p0 = points[ring[0] as usize];
                let flat: Vec<[f64; 2]> = ring
                    .iter()
                    .map(|&i| {
                        let d = sub(points[i as usize], p0);
                        [dot(d, e1) as f64, dot(d, e2) as f64]
                    })
                    .collect();
                let mut simple = true;
                'outer: for i in 0..flat.len() {
                    for j in i + 1..flat.len() {
                        let (a1, a2) = (flat[i], flat[(i + 1) % flat.len()]);
                        let (b1, b2) = (flat[j], flat[(j + 1) % flat.len()]);
                        // benachbarte Segmente teilen einen Punkt
                        if (i + 1) % flat.len() == j || (j + 1) % flat.len() == i {
                            continue;
                        }
                        if segments_cross(a1, a2, b1, b2) {
                            simple = false;
                            break 'outer;
                        }
                    }
                }
                if simple {
                    loops_to_fill.push(ring);
                }
            }
        }
        // Kapazitäts-bewusst anfügen: der Füller darf keine Kante über
        // ihre Manifold-Kapazität heben (innere ≤2, eigene Kette ≤1) — die
        // Kandidaten-Vervollständigung kann Kanten bereits gefüllt haben.
        let mut fill_use: HashMap<(u32, u32), u32> = HashMap::new();
        for &[a, b, c] in &tris {
            for (x, y) in [(a, b), (b, c), (c, a)] {
                *fill_use.entry((x.min(y), x.max(y))).or_default() += 1;
            }
        }
        for ring in loops_to_fill {
            for t in crate::mesher_cdt::triangulate_planar_face(points, &[ring], &[], 0.0) {
                let [a, b, c] = t;
                let ok = [(a, b), (b, c), (c, a)].iter().all(|&(x, y)| {
                    let e = (x.min(y), x.max(y));
                    let cap = if chain_edges.contains(&e) { 1 } else { 2 };
                    fill_use.get(&e).copied().unwrap_or(0) < cap
                });
                if !ok {
                    continue;
                }
                for (x, y) in [(a, b), (b, c), (c, a)] {
                    *fill_use.entry((x.min(y), x.max(y))).or_default() += 1;
                }
                tris.push(t);
            }
        }
        // Der Füller kennt weder fremde Wände noch die Einzelnutzung —
        // beide Regeln nach dem Füllen erneut durchsetzen (Betreiber-Demo:
        // 2 non-manifolde Rest-Kanten kamen genau von Füll-Dreiecken).
        if let Some(gce) = global_chain_edges {
            tris.retain(|&[a, b, c]| {
                for (x, y) in [(a, b), (b, c), (c, a)] {
                    let e = (x.min(y), x.max(y));
                    if gce.contains(&e) && !chain_edges.contains(&e) {
                        return false;
                    }
                }
                true
            });
        }
        enforce_chain_single_use(points, chains, normals, &mut tris);
    }
    // ── FLÄCHEN-TREUE (Konzept §1 Schritt 6, Dreiecks-Ebene): das
    // Zentroid jedes Dreiecks muss auf der Oberfläche liegen — fängt
    // Luft-Dreiecke aller Quellen (Sterne, Vervollständigung, Füller)
    // über konkaven Buchten. Toleranz wie die Sehnen-Sagitta. ──
    tris.retain(|&[a, b, c]| {
        let (pa, pb, pc) = (
            points[a as usize],
            points[b as usize],
            points[c as usize],
        );
        let cen = [
            (pa[0] + pb[0] + pc[0]) / 3.0,
            (pa[1] + pb[1] + pc[1]) / 3.0,
            (pa[2] + pb[2] + pc[2]) / 3.0,
        ];
        let e2max = {
            let mut m = 0.0f32;
            for (x, y) in [(pa, pb), (pb, pc), (pc, pa)] {
                let d = sub(y, x);
                m = m.max(dot(d, d));
            }
            m.sqrt()
        };
        // Erlaubte Eintauchtiefe aus dem maximalen Normalen-Knick der
        // drei Ecken (s. Sichtlinien-Kommentar oben).
        let theta = {
            let n0 = normals.get(&a).copied();
            let n1 = normals.get(&b).copied();
            let n2 = normals.get(&c).copied();
            let mut t = 0.0f32;
            for (x, y) in [(n0, n1), (n1, n2), (n2, n0)] {
                if let (Some(x), Some(y)) = (x, y) {
                    t = t.max(dot(x, y).clamp(-1.0, 1.0).acos());
                }
            }
            t
        };
        if fg(cen).0.abs() > (0.3 * e2max * theta).max(0.1 * spacing[a as usize]) {
            return false;
        }
        own_near(project_pt(cen, fg))
    });

    // ── Orientierung: die Stern-Abstimmung arbeitet auf SORTIERTEN Index-
    // Tripeln, das Winding ist danach beliebig (gemessen: ~770 gleich-
    // gerichtete Doppelkanten auf dem Demo). Jedes Dreieck an der Flächen-
    // Normalen seiner Ecken ausrichten: cross(b−a, c−a) soll mit dem
    // Normalen-Mittel der drei Ecken übereinstimmen, sonst Ecken tauschen. ──
    for t in &mut tris {
        let (pa, pb, pc) = (
            points[t[0] as usize],
            points[t[1] as usize],
            points[t[2] as usize],
        );
        let n_tri = cross(sub(pb, pa), sub(pc, pa));
        let mut n_ref = [0.0f32; 3];
        for &i in t.iter() {
            if let Some(n) = normals.get(&i) {
                n_ref = [n_ref[0] + n[0], n_ref[1] + n[1], n_ref[2] + n[2]];
            }
        }
        if dot(n_tri, n_ref) < 0.0 {
            t.swap(1, 2);
        }
    }

    #[cfg(test)]
    for (&(a, b), &u) in edge_use.iter() {
        if u == 1 && !chain_edges.contains(&(a, b)) {
            let (pa, pb) = (points[a as usize], points[b as usize]);
            eprintln!(
                "  HOLE edge {a}-{b} sp=({:.2},{:.2}) at [{:.2},{:.2},{:.2}]-[{:.2},{:.2},{:.2}]",
                spacing[a as usize], spacing[b as usize],
                pa[0], pa[1], pa[2], pb[0], pb[1], pb[2]
            );
        }
    }

    GeneralFaceMesh { tris, hole_edges, nonmanifold_edges, open_edges, weak_tris }
}
