# #439 — verworfener Interior-Carrier-Cull

**Status: NICHT EINSATZFÄHIG. Dieser Stand ist bewusst nicht in `main`.**

Er wird aufgehoben, weil er gemessen funktioniert — er nimmt #419 Körper 10 von
acht offenen Flächen auf **eine** und die Manifold-Verletzungen von 15 auf 4 —
und an genau einer benannten Stelle bricht: `Thicken 6` des echten Dokuments
importiert nicht mehr. Das ist wortgleich die Bruchstelle von #438, aber aus
einem anderen Grund; der Unterschied steht unter (e).

Zugehöriger Commit in `main`: die Messinstrumente (`test(kernel) #439`) — drei
`#[ignore]`-Sonden im Onshape-Modul und der Minimalfall
`brep::weaving::tests::interior_extrude_swallowed_whole_leaves_no_face`.

---

## (a) Was #439 gefragt hat und was gemessen herauskam

Das Ticket vermutete, `both_branches_active_classed` sei falsch — entweder
scheitere die On-Boundary-Prüfung `|f_body(p)| > eps` oder die
Contributor-Menge nenne nicht beide Branches.

**Beides ist falsch. Der Aktivitätstest hat recht: die Naht existiert nicht.**

Gemessen am Paar (131111, 131112) von Körper 10, dichte Abtastung der
analytischen Schnittgeraden über ±1,2·Diagonale bei 0,0039 mm (39 974 Proben,
eps = 1e-5·diag = 0,00065 mm):

| eps-Faktor | eps [mm] | \|f\| ≤ eps | beide Contributor | ACTIVE |
| --- | --- | --- | --- | --- |
| ×1 | 0,000650 | 2 | 1 | **1** |
| ×10 | 0,006496 | 6 | 3 | 3 |
| ×100 | 0,064958 | 66 | 33 | 33 |
| ×1000 | 0,649576 | 666 | 333 | 333 |

Also: **nicht null aktive Punkte, sondern genau einer** — und die Zahl wächst
exakt linear mit eps, wie es sich für einen TRANSVERSALEN Schnitt gehört. Die
Contributor-Prüfung besteht überall dort, wo die On-Boundary-Prüfung besteht
(am Minimum ist die Menge `[112638, 131111, 131112, 133118]`, beide Branches
sind drin). Es scheitert also ausschliesslich Teilprüfung 1 — und zwar zu
Recht: das Feldprofil entlang der Geraden liest im Extrusionsbereich

    z = 6,93 mm   f = −0,93   nächste Fläche 112638
    z = 3,93 mm   f = −3,18   nächste Fläche 110599
    z = 0,93 mm   f = −0,93   nächste Fläche 112638
    z = −0,08 mm  f = +0,07   nächste Flächen 112638, 133118

Die Gerade liegt 1–6 mm TIEF IM MATERIAL, und die nächste Randfläche ist nie
eine der beiden Facetten.

## (b) Warum — die Konstellation

- Die Branches 131111/131112 haben im B-Rep **gar keine Fläche und keine
  Kante**. Das Ticket hat für seine Messung ein Paar gewählt, das nicht zur
  Familie der offenen Flächen gehört.
- Die fünf offenen Facettenflächen sind 131093, 131094, 131097, 131098, 131099.
  Ihre einzige Kante ist **kein Kappen-Rand der eigenen Extrusion**, sondern
  eine Kante gegen Branch **112638** — die untere Kappe von `Extrude 27` —
  und liegt bei z ≈ 0.
- `Extrude 32` (op `Add`, Branches 131072…131116 + Kappen 133118/133119) wird
  von `Extrude 27` **vollständig verschluckt**: 0 von 2680 Materialproben
  seines AABB liegen ausserhalb von 27. z-Bereiche 32: [0, 9,70], 27:
  [0, 15,85] — die UNTEREN Kappen sind koplanar.
- An den Facettenmitten liest das Körperfeld exakt `f = −z` (nächste Fläche
  112638) über zwei Dekaden: z = 0,05 → −0,05; 0,1 → −0,1; … 3,2 → −3,2.
  Die Wand ist also Rand auf einer LINIE (z = 0, wo die Kappenebene Rand ist)
  und nirgends sonst. Eine Linie berandet keine Fläche.
- Der Tracer findet diese Linien zu Recht: dort sind beide Felder null und
  beide Branches Contributor. `materialize_faces` gibt anschliessend jedem
  gekratzten Wand-Branch eine Fläche — fünf davon mit genau einer Kante — und
  lässt die Kappenfläche 112638 mit den restlichen Kratzern in ihrer Schleife
  zurück, die dadurch nicht mehr zu ordnen ist.

## (c) Der Minimalfall

Ein 64-Facetten-Extrude vollständig innerhalb eines grösseren, untere Kappen
bündig, obere Kappe innen (die #419-Anordnung), reproduziert den Defekt
**nicht**: das Ergebnis ist schon ohne Reparatur (8, 12, 6), null offene
Flächen, wasserdicht — die manifold-getriebene Repair-Stufe in `finish_weave`
(`reattribute_coincident_memberships`) räumt die Zusatzflächen weg. Was dem
Minimalfall gegenüber dem echten Dokument fehlt, ist ein Körper, den diese
Stufe nicht mehr retten kann (Körper 10 hat elf weitere Features, darunter
einen entfernenden Revolve und vier Bohrungen).

Der Fall ist trotzdem als Test in `main` (er hält den Zwischenstand fest, in
dem der Cull ohne den Kratzer-Drop lief: dort werden daraus 7 Flächen und 76
Kanten).

## (d) Die Reparatur

Zwei Funktionen in `crates/kernel/src/brep/weaving/canonicalize.rs`, aufgerufen
aus `canonicalize_coincident` in `weaving/mod.rs` — **ausserhalb** des
`if !registry.is_empty()`-Blocks, denn ein vollständig innerer Träger ist keine
Koinzidenz-Eigenschaft:

```rust
    // #439: a carrier that is body boundary NOWHERE owns no face, and the
    // curves it leaves on the faces it crosses are scratches, not edges.
    let interior =
        cull_interior_carrier_branches(body, brep, halfspaces, &memberships, &culled, diag);
    drop_interior_scratches(brep, &mut memberships, &interior);
    culled.extend(interior);
    drop_fin_edges(body, brep, &mut memberships, &culled, halfspaces, diag);
```

Dafür muss `let aabb/diag` vor den `if`-Block gezogen und `drop_fin_edges` aus
ihm herausgenommen werden (es lief bisher nur bei nicht-leerer Registry).

```rust
/// #439 — cull carriers that are body boundary **nowhere**.
///
/// [`cull_membrane_branches`] only looks at carriers a coincidence class
/// marks as suspicious. A carrier can be fully interior without any
/// coincidence at all: a union whose second operand lies ENTIRELY inside the
/// first (measured on #419 body 10: `Extrude 32` adds nothing — 0 of 2680
/// material samples fall outside `Extrude 27`) contributes walls that never
/// reach ∂body. Such a wall still crosses the body's boundary wherever the
/// boundary happens to run through it — on #419 the two extrudes share the
/// z = 0 cap plane, so each interior wall meets that plane in a LINE on which
/// the union field is exactly zero and both branches are contributors. The
/// tracer is right to find that line; it is a scratch ON the cap face, not an
/// edge OF the wall, because the wall has no boundary area to bound. The
/// weaver then materialised a face per scratched wall (measured: five faces
/// with exactly one edge each) and left the cap face itself unorderable,
/// because its loop carried those interior scratches.
///
/// The test is the sidedness probe of [`branch_is_membrane`], moved OFF the
/// edge: at three stations along each of the branch's edges, step INSIDE the
/// carrier perpendicular to the edge (both ways, at 1·ε and 4·ε), snap back
/// onto the carrier and ask whether the body field changes sign across it.
/// The tangential step is what makes the answer meaningful — on the edge
/// itself every probe sits on the OTHER carrier's boundary, where both sides
/// read zero. One crossing anywhere keeps the branch; a branch is culled only
/// when every probe on every edge reads "same side". A carrier without an
/// analytic snap, or with no probe that can be evaluated, is kept.
///
/// Deliberately NOT part of the test: who OWNS the carrier. #438 measured
/// that deciding ownership between coplanar, equally-oriented sibling
/// branches moves face naming and breaks `qWall` queries. Two flush caps both
/// see a sign change here, so both survive — the question is never asked.
pub(super) fn cull_interior_carrier_branches(
    body: &BoundedCsg,
    brep: &BRep,
    halfspaces: &BranchSurfaceMap,
    memberships: &[Vec<u32>],
    already: &std::collections::HashSet<u32>,
    diag: f64,
) -> std::collections::HashSet<u32> {
    use crate::brep::vec3::add;
    use crate::csg::EvalMode;
    let eps = crate::brep::tol::on_boundary(diag);
    let mut edges_of: HashMap<u32, Vec<EdgeId>> = HashMap::new();
    for (i, members) in memberships.iter().enumerate() {
        for &b in members {
            edges_of.entry(b).or_default().push(EdgeId(i as u32));
        }
    }
    let mut culled = std::collections::HashSet::new();
    let mut branches: Vec<u32> = edges_of.keys().copied().collect();
    branches.sort_unstable();
    for b in branches {
        if already.contains(&b) {
            continue;
        }
        let Some((hs, xf)) = halfspaces.get(&b) else {
            continue;
        };
        let surface = SurfaceGeometry::Analytic { surface: hs.clone(), transform: *xf };
        let mut probed = 0usize;
        let mut crossing = false;
        'branch: for &EdgeId(e) in &edges_of[&b] {
            let pts = crate::brep::edge_sampling::sample_edge(&brep.edges[e as usize].geometry);
            if pts.len() < 3 {
                continue;
            }
            for frac in [0.25f64, 0.5, 0.75] {
                let i = (((pts.len() - 1) as f64 * frac) as usize).clamp(1, pts.len() - 2);
                let t = unit(sub(pts[i + 1], pts[i - 1]));
                let Some(q) = snap_to_carrier(&surface, pts[i]) else {
                    continue;
                };
                // Validity gate: the probe reads the body field at points of
                // the ANALYTIC carrier, so it only means anything where the
                // carrier IS the surface. Under a warp/displace the traced
                // edge sits on the deformed surface while `halfspaces` still
                // names the undeformed leaf, and the snap moves the point far
                // off the edge — measured on `lichen_adv_warp_twist_and_bend`
                // and `hybrid_displaced_box_*`, where probing anyway culled
                // live boundary branches. Such a station is not evaluated.
                if norm(sub(q, pts[i])) > eps {
                    continue;
                }
                let Some(n) = surface_normal_at(&surface, q) else {
                    continue;
                };
                let d = cross(n, t);
                if norm(d) < 0.5 {
                    continue;
                }
                let d = unit(d);
                for k in [1.0f64, 4.0] {
                    for side in [1.0f64, -1.0] {
                        let Some(s) = snap_to_carrier(&surface, add(q, scale(d, eps * k * side)))
                        else {
                            continue;
                        };
                        let Some(ns) = surface_normal_at(&surface, s) else {
                            continue;
                        };
                        let f = |sg: f64| -> f32 {
                            body.sdf(
                                [
                                    (s[0] + ns[0] * eps * sg) as f32,
                                    (s[1] + ns[1] * eps * sg) as f32,
                                    (s[2] + ns[2] * eps * sg) as f32,
                                ],
                                EvalMode::Logical,
                            )
                        };
                        probed += 1;
                        if (f(1.0) < 0.0) != (f(-1.0) < 0.0) {
                            crossing = true;
                            break 'branch;
                        }
                    }
                }
            }
        }
        if probed > 0 && !crossing {
            culled.insert(b);
        }
    }
    culled
}

/// #439 — drop the scratches a culled interior carrier left behind.
///
/// A B-Rep edge is the shared boundary of TWO faces. Once
/// [`cull_interior_carrier_branches`] has established that one of an edge's
/// carriers is body boundary nowhere, that carrier owns no face, and the
/// edge bounds nothing on that side: it is a curve lying INSIDE the other
/// carrier's face. On #419 body 10 those are the ten scratches the interior
/// extrude's walls leave on the shared z = 0 cap plane; keeping them left the
/// cap face unorderable and its edges singly referenced (measured: 8 open
/// faces and +7 manifold violations against dropping them).
///
/// This is deliberately unconditional where [`drop_fin_edges`] probes: the
/// fin drop must stay careful because a MEMBRANE cull can be wrong about a
/// partially-boundary carrier, whereas the interior cull already required
/// every probe on every edge of the branch to read "no crossing". Only edges
/// naming a branch from THIS cull are dropped, and they are dropped from all
/// their faces at once — no face is renamed, so #438's `qWall` failure mode
/// (ownership transfer between coincident siblings) is not reachable.
pub(super) fn drop_interior_scratches(
    brep: &mut BRep,
    memberships: &mut Vec<Vec<u32>>,
    interior_culled: &std::collections::HashSet<u32>,
) {
    if interior_culled.is_empty() {
        return;
    }
    let keep: Vec<bool> = memberships
        .iter()
        .map(|m| !m.iter().any(|b| interior_culled.contains(b)))
        .collect();
    if keep.iter().all(|&k| k) {
        return;
    }
    compact_edges_and_vertices(brep, &keep, memberships);
}
```

**Schrittweiten.** `eps = tol::on_boundary(diag) = diag·1e-4`. Tangentialer
Schritt aus der Kante heraus: 1·eps UND 4·eps (eine Kreuzung bei irgendeiner
Weite genügt zum Behalten). Normalenprobe ±1·eps. Stationen bei 25/50/75 % der
Kantenlänge.

**Das Gültigkeits-Gate ist nicht optional.** Ohne `norm(sub(q, pts[i])) > eps →
continue` fallen `api::lichen::tests::lichen_adv_warp_twist_and_bend_watertight`
(85 Loch-Kanten) und `brep::seed_weaver::tests::hybrid_displaced_box_mixed_sign_chain_samples_on_body`:
unter einem Warp/Displace liegt die getracte Kante auf der VERFORMTEN Fläche,
während `halfspaces` den unverformten Leaf nennt — die Probe misst dann an der
falschen Stelle und cullt lebende Randflächen.

**Bewusst NICHT im Test: wem der Träger gehört.** #438 ist an genau dieser
Frage gescheitert. Zwei bündige Kappen sehen hier beide einen Vorzeichenwechsel
und überleben beide; die Frage wird nie gestellt.

## (e) Gemessene Zahlen mit der Reparatur

Nativ, `cargo nextest --profile full`.

### #419 Körper 10 (voller Import)

|  | vorher | nachher |
| --- | --- | --- |
| `brep_counts` | (23, 30, 17) | (16, 25, 11) |
| offene Flächen | **8** | **1** |
| Manifold-Verletzungen | 15 | **4** |
| wasserdicht | false | false |

Flächenweise: die fünf Einkant-Flächen 131093/131094/131097/131098/131099
entfallen; die Kappenfläche 112638 SCHLIESST (9 Kanten offen → 6 Kanten
geschlossen); die Kappe des verschluckten Extrudes 133118 (5 Kanten, offen)
entfällt ebenfalls — jede ihrer Kanten war ein Kratzer auf der Ebene, die sie
mit 112638 teilt. Übrig bleibt 110605, eine andere Familie.

`open_faces` des ganzen Dokuments: `[0,0,0,0,0,0,2,0,0,1,8,0]` →
`[0,0,0,0,0,0,2,0,0,1,1,0]`. Kein anderer Körper bewegt sich.

### #419 Körper 4 (Revolve-Remove-Replay)

|  | vorher | nachher |
| --- | --- | --- |
| `brep_counts` | (51, 86, 37) | (51, 78, 34) |
| offene Flächen | 24 | 20 |
| Manifold-Verletzungen | 73 | 65 |
| wasserdicht | false | false |

Drei Träger sind dort nirgends Rand: 41108, 45195, 45204. Gegenprobe auf einem
240×240-Raster über jede Trägerebene im Körper-AABB: **0** On-Boundary-Proben,
gegen 6/2/3 bei den behaltenen Nachbarn 40973/41111/43006. Ihre drei Flächen
(3, 3 und 4 Kanten) waren alle bereits offen. 45201 wird zum geschlossenen Ring
(3 offen → 4 geschlossen); 43007 (24→21), 45186 (7→5), 41087 (6→5), 41111
(6→4), 41114 (5→4) verlieren Kratzer und bleiben aus anderen Gründen offen.
Keine vorher geschlossene Fläche bricht, die Vertex-Zahl bleibt 51.

### Testsuiten

`cargo nextest --workspace --profile full`: **1464/1466**, die zwei
Fehlschläge sind ausschliesslich die beiden oben angepassten Onshape-Pins.
Alle `lichen_adv_*_watertight` grün. `npx tsc -b` grün. Fünf der sechs Dateien
der Onshape-Slow-Suite grün.

## (f) Die Bruchstelle

`web/src/onshape/onshapeImport419E2E.slow.test.ts`:

    Thicken 6: Flächen-Query "qWall(23, 130)" löst auf 0 Flächen auf (erwartet 1) — Fläche ausgelassen
    Thicken 6: keine auflösbare Fläche — Feature übersprungen

Wortgleich mit #438 — aber der Mechanismus ist ein anderer und **nativ
messbar**, was #438 ausdrücklich gefehlt hat. `qWall(23, 130)` ist Branch
`23·4096 + 130 = 94338`; er liegt in Körper 9 des nativen Replays. Dessen
Coverage:

    coverage branch 94338: on-boundary samples     0, contributor    0  (eps 0,0078)
    coverage branch 94347: on-boundary samples 13924, contributor 1788  (eps 0,0078)

Der Cull ist dort also **korrekt**: dieser Träger ist nirgends Rand. Die
Fläche, auf die der Import sein `Thicken 6` stützt, ist eine Kümmerfläche mit
Flächeninhalt null — im heutigen `main` ist es Fläche 7 von Körper 9, mit
**genau einer** (selbstschliessenden) Kante, und genau zwei Kanten des Körpers
nennen 94338 überhaupt. Der Import benutzt sie trotzdem.

Das ist der Unterschied zu #438: dort verschob eine Um-Zuordnung den NAMEN
einer real existierenden Fläche. Hier steht eine geometrische Wahrheit gegen
eine Namenserwartung des Importers.

## (g) Was der Nächste zuerst braucht

Die Frage ist keine Kernel-interne mehr, und sie hat zwei mögliche Antworten:

1. **Der Importer muss mit fehlenden Flächen umgehen.** Eine `qWall`, die auf
   einen nirgends-Rand-Träger zeigt, kann nichts liefern; ein Thicken darauf
   hat auch in Onshape keine Entsprechung in unserem Körper. Dann gehört das
   Feature ehrlich als „nicht importierbar" gemeldet — und die Erwartungsliste
   der E2E um `Thicken 6` erweitert, mit dieser Begründung.
2. **Oder unser Körper 9 ist falsch.** 106 Features des Dokuments importieren
   ohnehin nicht; wenn eines davon den Träger 94338 zum Rand machen würde,
   wäre die Query im vollständigen Modell auflösbar und der Cull träfe dort
   nie zu. Das wäre zu prüfen, BEVOR man den Importer ändert.

Vor beidem steht dieselbe Vorarbeit: eine Aussage darüber, was `Thicken 6` im
Onshape-Original tatsächlich verdickt.
