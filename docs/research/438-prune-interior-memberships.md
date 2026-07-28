# #438 — verworfene Teil-Membran-Reparatur

**Status: NICHT EINSATZFÄHIG. Dieser Stand ist bewusst nicht in `main`.**

Er wird trotzdem aufgehoben, weil er gemessen funktioniert hat — an drei
Minimalfällen vollständig und am echten #419-Dokument mit nachvollziehbarem
Gewinn — und an genau einer, benannten Stelle bricht. Ohne diese Datei müsste
die nächste Runde mehrere Stunden Messarbeit wiederholen, um an denselben Punkt
zu kommen. Ticket #438 steht auf `parked`; die Prosa dazu steht im ersten
Agenten-Kommentar dort.

Zugehörige Commits in `main` (das, was ausgeliefert wurde):

- `e90bb879` — `fix(kernel) #438 (v0.39.8)`: Determinismus-Fix im Seed-Weber.
  Unabhängig von der hier beschriebenen Reparatur, aber **Voraussetzung**: ohne
  ihn schwankt #419 Körper 4 zwischen 101 und 102 Kanten nach dem Merge, und
  keine der unten stehenden Zahlen ist reproduzierbar.
- `252ce93b` — `test(kernel) #438`: die drei Minimalfälle als Instrumente auf
  den FALSCHEN Ist-Zustand.

---

## (a) Wo der Code eingehängt wird

Eine neue Funktion `prune_interior_memberships` in
`crates/kernel/src/brep/weaving/canonicalize.rs`, aufgerufen aus
`canonicalize_coincident` in `crates/kernel/src/brep/weaving/mod.rs`, als
**letzter Schritt der Koinzidenz-Kanonisierung, direkt nach `drop_fin_edges`**:

```rust
        if !registry.is_empty() {
            drop_point_contact_edges(brep, &mut memberships, diag);
            weld_seam_vertices(brep, &mut memberships, diag);
            merge_coincident_edges(brep, &registry, &mut memberships, diag);
            culled =
                cull_membrane_branches(body, brep, &registry, halfspaces, &memberships, diag);
            drop_fin_edges(body, brep, &mut memberships, &culled, halfspaces, diag);
            prune_interior_memberships(
                body, brep, &mut memberships, &culled, halfspaces, diag,
            );
        }
```

Die Reihenfolge ist nicht beliebig: der Pass braucht `culled` (er sieht nur
Kanten an, die einen gecullten Branch nennen) und läuft nach `drop_fin_edges`,
damit dessen eigene Beweisführung unangetastet bleibt.

Zwei Doc-Kommentare wurden mitgeführt (rein redaktionell): der Modulkopf von
`canonicalize.rs` und die Aufzählung in der Doc von `canonicalize_coincident`
bekamen je eine Zeile über die neue Stufe.

**Imports:** `canonicalize.rs` hat bereits `HashMap`, `cross`, `normalize as
unit`, `scale`, `sub`, `norm` und `BranchId`. `add` kommt über das dortige
`use super::*;` aus `weaving/mod.rs`. Es war kein neuer `use` nötig.

## (b) Das Kriterium in Worten

`cull_membrane_branches` entscheidet **pro Branch, alles oder nichts**. Ein
Träger, der nur über einem TEIL seiner Fläche Membran ist, überlebt deshalb
ganz — und `materialize_faces` gibt seiner Fläche danach jede Kante, deren
Signatur ihn nennt, die inneren eingeschlossen. Die Signatur sagt nur, *welche
zwei Träger sich hier schneiden*; sobald ein Träger teils Rand und teils
Inneres ist, ist das die falsche Frage.

Der Pass stellt die richtige: er tritt von der Kante aus **innerhalb des
Trägers** nach beiden Seiten und fragt, ob sich die Zugehörigkeit dort
unterscheidet. Ein Trägerpunkt `p` gehört zur aktiven Region von Branch `b`,
wenn beides gilt:

1. **Der Träger ist dort echter Rand** — das Körperfeld wechselt quer zum
   Träger das Vorzeichen (`sdf(p + n·eps)` und `sdf(p − n·eps)` haben
   verschiedene Vorzeichen). Der Test ist orientierungsfrei, deshalb
   funktionieren komplementierte Branches ohne Sonderfall. Kein
   Vorzeichenwechsel heißt: entweder Material auf beiden Seiten (innere
   Membran) oder Leere auf beiden Seiten (in der Luft).
2. **`b` besitzt ihn** — `b` ist unter den Contributors bei `p`. Das ist der
   Teil, der die Region eines gleichorientierten koinzidenten Geschwisters von
   der eigenen unterscheidet. Es ist auch der Teil, an dem alles gescheitert
   ist (siehe (d)).

Eine Kante begrenzt `b`, wenn sich die Zugehörigkeit zwischen den beiden Seiten
unterscheidet. Drei Stationen (bei 25 %, 50 %, 75 % der Kantenlänge) stimmen ab,
und **jede** Station, die einen Unterschied sieht, gewinnt — eine schlecht
platzierte Probe darf nie eine echte Randkante entfernen. Ist keine Station
entscheidbar (kein analytischer Träger-Snap, entartete Tangente, zu wenige
Stützpunkte), liefert der Test `None` und der Aufrufer behält die Zuordnung.

**Schrittweiten.** Alle Maße hängen an `tol::on_boundary(diag) = diag · 1e-4`,
sind also skalenrelativ (auf #419 Körper 4: `eps ≈ 0,309 mm` bei 3088 mm
Diagonale):

- Normalen-Probe: `± 1 · eps` — dieselbe Größe, die `branch_is_membrane` und
  `drop_fin_edges` benutzen.
- Tangentialer Schritt aus der Kante heraus: `4 · eps`. Gemessene Plateaus:
  1,5–8 · eps liefern an allen Minimalfällen dasselbe Ergebnis; auf #419
  Körper 4 ist 1,5–4 · eps stabil (E = 68/69), ab 8 · eps beginnt Drift. 4 · eps
  wurde gewählt, weil es den größten Abstand zur ±eps-Normalenprobe und zum
  Contributor-Band hat und noch im Plateau liegt.
- Contributor-Band: `0,3 · eps` (wie in `drop_fin_edges`).

Eine Kante, die der Test für **jeden** überlebenden Branch ablehnt, begrenzt gar
nichts und wird gelöscht. Das ist dasselbe Urteil, das `drop_fin_edges` über
Spuren auf einer maßlosen `f = 0`-Fläche fällt — nur auf breiterer Beweislage:
der Fin-Drop sieht nur Kanten an, denen der Cull weniger als zwei Überlebende
gelassen hat, und ein bündiger SCHNITT hinterlässt reichlich Flächen-Spuren mit
zwei NOMINELLEN Überlebenden.

## (c) Der vollständige Code

Das ist der letzte Stand (dreistufige Fassung mit Flächenschutz). Er hat
kompiliert und die volle Rust-Suite bestanden.

```rust
// ─── in crates/kernel/src/brep/weaving/canonicalize.rs, ────────────────────
// ─── eingefügt vor `compact_edges_and_vertices` ────────────────────────────

/// Does `edge` bound branch `b`'s **kept region** — the part of `b`'s carrier
/// that is actually body boundary owned by `b`?
///
/// The signature only says *which two carriers cross here*, which is the wrong
/// question once a carrier is boundary over part of itself and interior over
/// the rest (a PARTIAL membrane). This asks the right one: step off the edge to
/// both sides *within the carrier* and check whether the kept-ness differs.
///
/// A carrier point `p` belongs to `b`'s kept region iff
///
/// 1. the carrier is a genuine boundary at `p` — the body field changes sign
///    across it (orientation-agnostic, so complemented branches work), and
/// 2. `b` owns it — `b` is among the contributors at `p` (this is what tells a
///    same-oriented coincident sibling's region from `b`'s own).
///
/// `None` = undecidable (no analytic carrier snap, degenerate tangent, too few
/// samples); the caller then keeps the membership. Three stations along the
/// edge vote, and ANY station that sees a difference wins — a partially
/// mis-probed station must never remove a real boundary edge.
pub(super) fn edge_bounds_branch_region(
    body: &BoundedCsg,
    edge: &Edge,
    branch: u32,
    surface: &SurfaceGeometry,
    eps: f64,
) -> Option<bool> {
    use crate::csg::EvalMode;
    let pts = crate::brep::edge_sampling::sample_edge(&edge.geometry);
    if pts.len() < 3 {
        return None;
    }
    // Tangential step: clear of the boundary band and of contributor
    // attribution noise, far below any face extent the tracer can resolve.
    let step = 4.0 * eps;
    let ctol = (eps * 0.3) as f32;
    let kept_at = |p: [f64; 3]| -> Option<bool> {
        let q = snap_to_carrier(surface, p)?;
        let n = surface_normal_at(surface, q)?;
        let probe = |side: f64| -> f32 {
            body.sdf(
                [
                    (q[0] + n[0] * eps * side) as f32,
                    (q[1] + n[1] * eps * side) as f32,
                    (q[2] + n[2] * eps * side) as f32,
                ],
                EvalMode::Logical,
            )
        };
        if (probe(1.0) < 0.0) == (probe(-1.0) < 0.0) {
            return Some(false); // no sign change: interior membrane or airborne
        }
        let (_, contributors) = body.evaluate(
            [q[0] as f32, q[1] as f32, q[2] as f32],
            ctol,
            EvalMode::Logical,
        );
        Some(contributors.as_slice().contains(&BranchId(branch)))
    };

    let mut decided = false;
    for frac in [0.25, 0.5, 0.75] {
        let i = (((pts.len() - 1) as f64 * frac).round() as usize).clamp(1, pts.len() - 2);
        let t = sub(pts[i + 1], pts[i - 1]);
        if norm(t) < 1e-12 {
            continue;
        }
        let t = unit(t);
        let q = snap_to_carrier(surface, pts[i])?;
        let n = surface_normal_at(surface, q)?;
        let d = cross(n, t);
        if norm(d) < 0.5 {
            continue; // edge not tangent to its own carrier — no usable side
        }
        let d = unit(d);
        let plus = kept_at(add(q, scale(d, step)))?;
        let minus = kept_at(sub(q, scale(d, step)))?;
        if plus != minus {
            return Some(true);
        }
        decided = true;
    }
    if decided { Some(false) } else { None }
}

/// Re-attribute the rim of every culled membrane (#438). `cull_membrane_branches`
/// decides per BRANCH, all or nothing, so a carrier that is a membrane over only
/// PART of itself survives whole — and `materialize_faces` then hands its face
/// every edge whose signature names it, the INTERIOR ones included. Those extra
/// edges are exactly the rim of the culled partner where it runs across the
/// survivor, so this pass visits the edges that carry a culled branch and drops
/// from their membership every surviving branch whose kept region the edge does
/// not actually bound ([`edge_bounds_branch_region`]).
///
/// It only looks at edges the cull touched: without a culled membrane a
/// branch's traced region IS its kept region and the signature is exact, so a
/// transversal body never pays for this and can never be disturbed by it.
///
/// An edge the probe rejects for EVERY surviving branch bounds nothing at all
/// and is dropped. That is the same judgement [`drop_fin_edges`] makes on a
/// measure-zero `f = 0` sheet, on wider evidence: the fin drop only looks at
/// edges left with fewer than two survivors by the cull, and a flush REMOVE
/// leaves plenty of sheet traces that keep two NOMINAL survivors — measured on
/// the #419 document, 18 such traces of 4–26 mm, each reading „no material
/// transition" on both sides of both carriers across two decades of probe
/// offset.
pub(super) fn prune_interior_memberships(
    body: &BoundedCsg,
    brep: &mut BRep,
    memberships: &mut Vec<Vec<u32>>,
    culled: &std::collections::HashSet<u32>,
    halfspaces: &BranchSurfaceMap,
    diag: f64,
) {
    if culled.is_empty() {
        return;
    }
    let eps = crate::brep::tol::on_boundary(diag);
    let live = |ms: &[u32]| ms.iter().filter(|b| !culled.contains(*b)).count();

    // Pass 1 — the verdicts, not yet applied.
    let mut proposed: Vec<Option<Vec<u32>>> = vec![None; brep.edges.len()];
    for i in 0..brep.edges.len() {
        if !memberships[i].iter().any(|b| culled.contains(b)) {
            continue;
        }
        let mut members = memberships[i].clone();
        members.retain(|&b| {
            if culled.contains(&b) {
                return true; // face materialisation skips these anyway
            }
            let Some((hs, xf)) = halfspaces.get(&b) else {
                return true;
            };
            let surface = SurfaceGeometry::Analytic { surface: hs.clone(), transform: *xf };
            edge_bounds_branch_region(body, &brep.edges[i], b, &surface, eps).unwrap_or(true)
        });
        if members != memberships[i] {
            proposed[i] = Some(members);
        }
    }

    // Pass 2 — SURFACE PRESERVATION. This pass repairs boundaries; making a
    // surface disappear is `cull_membrane_branches`' decision and needs its
    // whole-branch proof. A branch whose every edge is taken away materialises
    // no face at all, and then a `qWall` picking that wall resolves to nothing
    // and the user's feature fails to build — measured on the #419 import,
    // where `Thicken 6` lost `qWall(23, 130)` this way. So any branch left with
    // zero edges keeps the verdicts that emptied it; a face with a broken
    // boundary is still a face one can pick, and no other stage recovers it.
    //
    // MEASURED INEFFECTIVE against that very failure — see the doc file.
    let mut remaining: HashMap<u32, usize> = HashMap::new();
    for (i, m) in memberships.iter().enumerate() {
        let after = proposed[i].as_ref().unwrap_or(m);
        for &b in after {
            if !culled.contains(&b) {
                *remaining.entry(b).or_insert(0) += 1;
            }
        }
    }
    let starved: std::collections::HashSet<u32> = memberships
        .iter()
        .flatten()
        .copied()
        .filter(|b| !culled.contains(b) && remaining.get(b).copied().unwrap_or(0) == 0)
        .collect();
    if !starved.is_empty() {
        for (i, m) in memberships.iter().enumerate() {
            if proposed[i].is_some() && m.iter().any(|b| starved.contains(b)) {
                proposed[i] = None;
            }
        }
    }

    // Pass 3 — apply. An edge left bounding nothing is dropped.
    let mut keep = vec![true; brep.edges.len()];
    let mut changed = false;
    for i in 0..brep.edges.len() {
        let Some(members) = proposed[i].take() else {
            continue;
        };
        changed = true;
        if live(&members) == 0 {
            // Bounds nothing at all: a trace on a measure-zero `f = 0` sheet
            // that `drop_fin_edges` never looked at because it kept two
            // NOMINAL survivors. Same judgement, wider evidence.
            keep[i] = false;
        } else {
            memberships[i] = members;
        }
    }
    if changed {
        compact_edges_and_vertices(brep, &keep, memberships);
    }
}
```

**Die einfachere zweistufige Fassung.** Vor dem Flächenschutz gab es eine
Variante ohne `proposed`/Pass 2, die die Verdikte direkt anwandte:

```rust
        // ... statt Pass 1–3, in einer Schleife:
        if members == memberships[i] {
            continue;
        }
        changed = true;
        if live(&members) == 0 {
            keep[i] = false;
        } else {
            memberships[i] = members;
        }
```

Sie ist an allen gemessenen Zahlen **identisch** zur dreistufigen Fassung —
auch an #419 Körper 4 (48/68/35, open 22, viol 53). Der Flächenschutz hat dort
also nie gegriffen. Wer weiterarbeitet, sollte mit der einfachen Fassung
anfangen; Pass 2 ist toter Ballast, solange nicht geklärt ist, über welchen Weg
die Fläche tatsächlich verlorengeht.

## (d) Gemessene Zahlen mit dieser Reparatur

Alles nativ, `cargo nextest --profile full`, nach dem Determinismus-Fix
`e90bb879`.

### Minimalfälle (alle vier grün MIT der Reparatur)

| Fall | ohne Reparatur | mit Reparatur |
| --- | --- | --- |
| Union, ein Reststreifen (`partial_membrane_on_coincident_carrier_*`) | open 1, nicht wasserdicht, Branch 4098 mit 7 Kanten, (14, 23, 11) | **open 0, wasserdicht, 0 Verletzungen, 4098 mit 4 Kanten, (14, 23, 11)** |
| Union, zwei Reststreifen (B mittig) | open 1, eine Fläche mit 10 Kanten | **open 0, wasserdicht, zwei Flächen zu je 4 Kanten (`strips == [4, 4]`)** |
| Bündiger Schnitt (`Difference`) | (14, 21, 8), open 4, viol 18, +x-Wand mit 7 Kanten | **(12, 18, 8), open 0, viol 0, wasserdicht** — Euler 12 − 18 + 8 = 2 |
| Skaleninvarianz 0,01× / 1× / 100× | — | **open 0, wasserdicht, (14, 23, 11) auf allen drei Skalen** |

Branch 4098 behält im ersten Fall genau: den −x-Rand, die beiden **äusseren**
±z-Hälften und die Trennlinie. Die Teilung der ±z-Ränder bei x = 0 bleibt
erhalten (sie ist richtig), und 4100/4101 behalten beide Hälften — ihre Ringe
haben danach 5 Kanten und sind geschlossen.

### #419 Körper 4 (`Extrude 8 ∖ Revolve 5`)

|  | vorher | nachher |
| --- | --- | --- |
| `brep_counts` | (51, 86, 37) | (48, 68, 35) |
| `open_faces` | [0, 0, 0, 0, **24**] | [0, 0, 0, 0, **22**] |
| Manifold-Verletzungen | 73 | 53 |
| wasserdicht | false | false |

Flächenweise, vollständig:

- **repariert** (offen → geschlossener Ring): 41066 (6 → 4 Kanten),
  41114 (5 → 3).
- **entfallen** (beide waren offen, beide Kümmerflächen): 41084 (2 Kanten),
  45165 (1 Kante). Eine Fläche aus ein bis zwei Kanten war nie eine Fläche.
- **neu offen**: 45171 (4 → 3), 45177 (7 → 5). Das waren geschlossene Ringe,
  die über Kanten liefen, die der Test auf **beiden** Seiten und bei **allen**
  Sondenabständen ablehnt — ihr Schluss lief über nicht-begrenzende Kanten.
  Jetzt ehrlich offen. Das ist eine Verschlechterung der Zahl und eine
  Verbesserung der Wahrheit.
- Rechnung: 24 − 2 repariert − 2 entfallen + 2 aufgedeckt = 22.
- Die übrigen zwölf Flächen schrumpfen (41063 6→4, 41075 2→1, 41087 6→2,
  41099 7→4, 41108 3→1, 41111 6→1, 43006 19→18, 43007 24→19, 45186 7→4,
  45195 4→3, 45201 3→2, 45204 3→1), waren aber vorher wie nachher offen.

### Belastbarkeit der 18 Löschungen

Alle 18 gelöschten Kanten sind **4–26 mm** lang und liegen auf `∂body`
(`|f| ≈ 0`, Contributors am Mittelpunkt nennen die betroffenen Branches). Das
allein macht sie nicht zu Rand: sie liegen auf einer maßlosen `f = 0`-Fläche.

Gegenprobe über zwei Dekaden: für jedes entfernte (Kante, Branch)-Paar wurde die
Normalenprobe bei `k · eps` mit `k ∈ {0,1; 0,3; 1; 3; 10}` wiederholt, also von
0,03 mm bis 3,1 mm. **39 von 41 Paaren lesen „kein Vorzeichenwechsel" auf beiden
Seiten bei allen fünf Abständen.** Die zwei Ausnahmen (Kante e29/Branch 41084
und e47/41105) gehören zu Kanten, die NICHT gelöscht wurden; bei e29 ist 41084
am Mittelpunkt gar nicht unter den Contributors, die Entfernung dort ist also
korrekt und stammt aus Kriterium 2, nicht aus Kriterium 1.

### Testsuiten

Mit der Reparatur und den angepassten Pins: `cargo nextest --profile full`
**1467/1467 grün**, inklusive aller `lichen_adv_*_watertight`, aller
Klassifikations- und Naming-Gates und `onshape_419_full_import_stays_bounded`
(dessen `open_faces` sich nicht ändert). `just test`, `npx tsc -b` und die
übrigen fünf Dateien der Onshape-Slow-Suite ebenfalls grün.

### Anzupassende Pins

In `crates/kernel/src/onshape_regression_tests.rs`,
`onshape_419_revolve_remove_stays_bounded`:

```rust
    assert_eq!(brep_counts(&mut kernel, 4), (48, 68, 35), ...);   // war (51, 86, 37)
    assert_eq!(open_faces(&mut kernel), vec![0, 0, 0, 0, 22], ...); // war [.., 24]
```

Die Begründung dafür steht oben unter „Flächenweise, vollständig" und gehört bei
einer Wiederaufnahme in den Test-Kommentar.

## (e) Die bekannte Bruchstelle

`web/src/onshape/onshapeImport419E2E.slow.test.ts` (echtes Dokument, echter
Wasm-Kernel) meldet mit dieser Reparatur neu:

```
Thicken 6: Flächen-Query "qWall(23, 130)" löst auf 0 Flächen auf (erwartet 1) — Fläche ausgelassen
Thicken 6: keine auflösbare Fläche — Feature übersprungen
```

Ein Feature, das vorher importierte, importiert nicht mehr. Das ist der Grund
für das Parken: der Ist-Zustand ist laut Ticket kein Blocker (Körper werden
gebaut und gerendert), ein verlorenes Feature wäre einer.

**Eingegrenzt durch Bisektion**, jeweils mit eigenem `just build-wasm`:

| Konfiguration | E2E |
| --- | --- |
| `main` (HEAD) | grün |
| nur Determinismus-Fix `e90bb879` | grün |
| + Um-Zuordnung **ohne jede Löschung** | **rot** |
| + Um-Zuordnung + Löschung | rot |
| + Um-Zuordnung + Löschung + Flächenschutz (Pass 2) | rot |

Es liegt also **nicht** an den gelöschten Kanten, sondern an der Um-Zuordnung
selbst. `QueryExpr::Wall` in `crates/kernel/src/queries.rs` löst über die
Branch-ID auf (`b.feature_id() == target && sub == *entity && face_role(face) ==
want`) und iteriert dazu die Flächen aller B-Reps. Bei koplanaren,
**gleichorientierten** koinzidenten Geschwister-Branches — der `+x`-Wand eines
Quaders und der bündigen `+x`-Wand des daraufstehenden — entscheidet Kriterium 2
(Contributors), wem die gemeinsame Trägerfläche gehört. Verliert ein Branch
dadurch seine letzte Kante, entsteht keine Fläche und die Query läuft leer.

### Zwei Rettungsversuche, beide gemessen gescheitert — nicht wiederholen

1. **Flächenschutz** (Pass 2 oben): kein Branch darf auf null Kanten fallen, die
   betroffenen Verdikte werden zurückgenommen. Ergebnis: Rust-Suite bleibt grün,
   #419 Körper 4 unverändert bei 48/68/35 — und **`Thicken 6` bleibt rot**. Der
   Verlust läuft also nicht über „null Kanten". An dieser Stelle ist mein Modell
   des Mechanismus unvollständig; ich weiß nicht, über welchen Weg die Fläche
   verschwindet. Ausgeschlossen ist immerhin: `assemble_rings` liefert nie
   `Some(vec![])` (entweder `None` → Fallback-Fläche, oder eine nichtleere
   Liste), und `face_role` hängt nur an der Branch-ID, ändert sich also nicht.
2. **Contributor-Prüfung streichen** (nur noch Kriterium 1, damit die
   Eigentumsfrage gar nicht erst gestellt wird). Die Erwartung war, dass die
   gemeinsame Randkante dann bei beiden Geschwistern bleibt. Ergebnis:
   **zerlegt die Reparatur selbst** — 4 von 5 Minimalfall-Tests fallen aus,
   darunter der vorher grüne `full_membrane_on_coincident_carrier_still_weaves_closed`.
   Ohne Kriterium 2 überschießt der Pass an anderer Stelle. Die
   Handrechnung, die das Gegenteil vorhersagte, war falsch.

### Was der Nächste zuerst braucht

Einen **nativen Repro von `qWall(23, 130)`**. Der Befund hängt derzeit
ausschließlich an der JS-E2E-Suite: jede Verifikation kostet einen Wasm-Build
plus Lauf (~5 min), und `eprintln!` aus dem Wasm erscheint im Vitest-Log **nicht**
(geprüft: auch die vorhandenen `weave: WARNING`-Zeilen tauchen dort nicht auf).
Der native Replay `onshape419_full_import.json` (49 Aufrufe, 12 Körper) enthält
`Thicken 6` nicht. Ohne diesen Repro ist der Mechanismus nicht zu Ende messbar,
und jede weitere Hypothese bliebe eine Vermutung.

Danach ist die eigentliche Frage keine Kernel-interne mehr: **die Zuordnung
„welcher von zwei koinzidenten Geschwister-Branches besitzt die gemeinsame
Trägerfläche" darf die Namensgebung nicht verschieben.** Entweder muss die
Region-Zuordnung diese Frage offenlassen (beide behalten die gemeinsame
Randkante — dann braucht es einen anderen Weg als das blosse Streichen von
Kriterium 2), oder die Query-Auflösung muss Koinzidenz-Klassen kennen. Das ist
ein Umbau über zwei Schichten.

## (f) Vollständigkeit dieser Rekonstruktion

Verlustfrei erhalten sind: beide Funktionen im Wortlaut, die Aufrufstelle, die
Schrittweiten und ihre Plateaus, sämtliche Messzahlen und die Bisektionstabelle.

Nicht im Wortlaut erhalten, weil rein redaktionell und trivial neu zu schreiben:
die zwei Doc-Kommentar-Zeilen in `weaving/mod.rs` und im Modulkopf von
`canonicalize.rs`, sowie die Test-Rümpfe der vier Minimalfall-Tests in ihrer
REPARIERTEN Fassung — deren Konstruktionen stehen unverändert in `main`
(`252ce93b`), nur die Erwartungswerte müssen von der Instrument-Fassung auf die
Tabelle unter (d) umgestellt werden.
