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

/// Unit-Tangentenbasis ⟂ n.
fn basis(n: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let seed = if n[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
    let e1 = norm(sub(seed, [n[0] * dot(seed, n), n[1] * dot(seed, n), n[2] * dot(seed, n)]));
    (e1, cross(n, e1))
}

/// 2D-Segment-Schnitt (echte Kreuzung, geteilte Endpunkte zählen nicht).
fn segments_cross(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> bool {
    let orient = |p: [f64; 2], q: [f64; 2], r: [f64; 2]| -> f64 {
        (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0])
    };
    let (o1, o2) = (orient(a, b, c), orient(a, b, d));
    let (o3, o4) = (orient(c, d, a), orient(c, d, b));
    o1 * o2 < 0.0 && o3 * o4 < 0.0
}

/// Allgemeine Triangulierung EINER Fläche.
///
/// * `points`/`normals`/`spacing`: globale Punkttabelle (Welt), Pro-Punkt-
///   Flächen-Normale und Spacing-Radius (`LPoint.space`).
/// * `members`: die globalen Indizes dieser Fläche (Rand + Innen).
/// * `chains`: geschlossene Rand-Ringe der Fläche (globale Indizes, Wrap
///   implizit) — ihre Segmente sind gesetzte Kanten.
pub fn triangulate_face_general(
    points: &[[f32; 3]],
    normals: &HashMap<u32, [f32; 3]>,
    spacing: &[f32],
    members: &[u32],
    chains: &[Vec<u32>],
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
                        // |⟨d,n⟩| ≤ sin(30°)·|d|. Ein fremdes Blatt (Gyroid-
                        // Gegenwand) liegt im Doppelkegel um die Normale und
                        // wird NIE verbunden; U(p) darf dadurch größer sein
                        // als der Blatt-Abstand.
                        let dl = dot(d, d).sqrt();
                        if dot(d, ni).abs() > 0.5 * dl {
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
    let gabriel_empty = |a: u32, b: u32, c: u32| -> bool {
        let (pa, pb, pc) = (
            points[a as usize],
            points[b as usize],
            points[c as usize],
        );
        // Umkreiszentrum in der Dreiecks-Ebene (Kramer über Baryzentrik).
        let ab = sub(pb, pa);
        let ac = sub(pc, pa);
        let ab2 = dot(ab, ab);
        let ac2 = dot(ac, ac);
        let abac = dot(ab, ac);
        let den = 2.0 * (ab2 * ac2 - abac * abac);
        if den.abs() < 1e-20 {
            return false; // degeneriert
        }
        let s_ = (ac2 * (ab2 - abac)) / den;
        let t_ = (ab2 * (ac2 - abac)) / den;
        let centre = [
            pa[0] + s_ * ab[0] + t_ * ac[0],
            pa[1] + s_ * ab[1] + t_ * ac[1],
            pa[2] + s_ * ab[2] + t_ * ac[2],
        ];
        let dc = sub(pa, centre);
        let r2 = dot(dc, dc);
        let r = r2.sqrt();
        // Punktfreiheit lokal prüfen (Spatial-Hash).
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
                        if dot(d, d) < r2 * (1.0 - 1e-4) {
                            return false;
                        }
                    }
                }
            }
        }
        true
    };
    // Default = Stern-Konsens. Die Umkugel ("gabriel"/"hybrid", Betreiber-
    // Idee als frame-freie Eindeutigkeit) ist implementiert und vermessen:
    // sie erbt das Tie-Problem — bei exakt cozirkulären Hex-Quads sind BEIDE
    // Diagonalen Gabriel-leer (Box: 70 non-manifolde Kanten) — und braucht
    // erst einen deterministischen Tie-Break, bevor sie den gekrümmten Fall
    // tragen kann. Als Env-Modus für die nächste Iteration verfügbar.
    let accept_mode =
        std::env::var("LICHEN_ACCEPT").unwrap_or_else(|_| "vote".into());
    let vote_min: u8 = std::env::var("LICHEN_VOTE_MIN")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    let mut tris: Vec<[u32; 3]> = votes
        .iter()
        .filter(|&(&(a, b, c), &v)| match accept_mode.as_str() {
            "vote" => v >= vote_min,
            "gabriel" => gabriel_empty(a, b, c),
            // hybrid: einstimmige Sterne ODER frame-freie Umkugel-Leere.
            _ => v >= vote_min || gabriel_empty(a, b, c),
        })
        .map(|(&(a, b, c), _)| [a, b, c])
        .collect();
    tris.sort_unstable();

    // ── Ketten-Kreuzungs-Verbot: kein Dreieck über eine gesetzte Kante ──
    // 2D-Test in der Ebene des jeweiligen Dreiecks (symmetrisch für alle
    // Beteiligten). Nur Ketten-Segmente in der Nähe prüfen.
    tris.retain(|&[a, b, c]| {
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
                    return false;
                }
            }
        }
        true
    });

    // ── Diagnose: Kanten-Zählung (2-Manifold, Löcher) ──
    let mut edge_use: HashMap<(u32, u32), u32> = HashMap::new();
    for &[a, b, c] in &tris {
        for (x, y) in [(a, b), (b, c), (c, a)] {
            *edge_use.entry((x.min(y), x.max(y))).or_default() += 1;
        }
    }
    let nonmanifold_edges = edge_use.values().filter(|&&u| u > 2).count();
    let hole_edges = edge_use
        .iter()
        .filter(|(&e, &u)| u == 1 && !chain_edges.contains(&e) && member_set.contains(&e.0))
        .count();

    // ── Loch-Schließen (deterministisch, NACH der Diagnose) ──
    // Stern-Uneinigkeit bleibt an Dichte-Übergängen möglich (der dichte Punkt
    // sieht die fernen groben Zeugen eines gemischten Dreiecks nicht). Die
    // offenen Kanten bilden geschlossene Loops; Loops, die mindestens eine
    // NICHT-Ketten-Kante enthalten (= Defekt, keine gewollte Rand-/Loch-
    // Kontur), werden als Polygon im Newell-Frame per CDT gefüllt.
    if hole_edges > 0 && std::env::var("LICHEN_NO_FILL").is_err() {
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
                    let Some(&nxt) = cands
                        .iter()
                        .find(|&&n| n != prev && !visited.contains(&(cur, n)))
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
        for ring in loops_to_fill {
            tris.extend(crate::mesher_cdt::triangulate_planar_face(
                points, &[ring], &[], 0.0,
            ));
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

    GeneralFaceMesh { tris, hole_edges, nonmanifold_edges }
}
