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
                            return false; // strikt innen → Umkugel nicht leer
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
        let n = norm(cross(ab, ac));
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
            "hybrid" => v >= vote_min || gabriel_empty(a, b, c),
            _ => v >= vote_min,
        }
    };
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

    // ── Ketten-Kanten-Einzelnutzung: Rand-Folds dieser Fläche entfernen ──
    enforce_chain_single_use(points, chains, normals, &mut tris);

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
                let best = cands
                    .into_iter()
                    .find(|&(_, w)| !crosses_chain(u, v, w) && gabriel_empty(u, v, w));
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
            if alive.len() != 2 {
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
            let s0 = dot(sub(points[third(alive[0]) as usize], mid), sdir);
            let s1 = dot(sub(points[third(alive[1]) as usize], mid), sdir);
            if s0 * s1 <= 0.0 {
                continue; // Gegenseiten (oder degeneriert) — ok
            }
            let t0 = alive[0];
            let t1 = alive[1];
            let (r0, r1) = (
                circum_r2(tris[t0][0], tris[t0][1], tris[t0][2]),
                circum_r2(tris[t1][0], tris[t1][1], tris[t1][2]),
            );
            let drop = if (r0, tris[t0]) > (r1, tris[t1]) { t0 } else { t1 };
            dropped[drop] = true;
        }
        let mut k = 0usize;
        tris.retain(|_| {
            let d = dropped[k];
            k += 1;
            !d
        });
    }

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
