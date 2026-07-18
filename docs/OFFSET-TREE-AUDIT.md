# offset_tree Phase A — Konsumenten-Audit der Leaf-Primitive-Identität

Stand 2026-07-18, HEAD c1199122. Pflicht-Vorarbeit zu `docs/OFFSET-TREE-PLAN.md` §5
(„Konsumenten-Audit"), angemahnt vom Doc-Kommentar an
`crates/kernel/src/csg.rs::push_down_offsets` (csg.rs:1471–1501): das analytische
Blatt-Einfalten ändert die Leaf-Primitive-Identität (`CsgNode::Leaf { id: FeatureId,
primitive: Primitive }`, csg.rs:696–700), die Feature-Snap- und B-Rep-Carrier-Pfade
konsumieren. Alle Pfade wurden im Code verifiziert (Datei:Zeile jeweils angegeben).

## 0. Identitäts-Landkarte: wer produziert, wer konsumiert

Es gibt heute VIER Baupfade vom Feature-Spine zum ausgewerteten Baum — und nur drei
davon laufen durch `push_down_offsets`:

| Baupfad | Ort | pushdown? | Konsumenten |
|---|---|---|---|
| `build_bounded_for_body_rounded` | body.rs:974–990 (Pass at 988) | ja | GPU-SDF-Compile (kernel_core.rs:385–393), Raycast/Pick (`build_bounded_for_body`, body.rs:960) |
| `build_bounded_with_primitives_rounded` | body.rs:1026–1047 (Pass at 1040, Map-Extraktion DANACH at 1041) | ja | Regen→Mesher (kernel_core.rs:358), Export-Mesher (export_mesh.rs), native_prepare, api/voxel, api/lichen (Feld + hs_map) |
| `build_bounded_for_body_local` | body.rs:999–1013 | **NEIN** | der komplette Weave (`ensure_brep`, body.rs:192–193; debug_octree_cells body.rs:298) |
| Feature-Spine direkt (`DecodedFeature.node` / `export_primitive`) | csg.rs:1106 | n/a | step_classifier, step_commands, brep/face_offset (via features/context_resolve.rs:389) |

Identitätsträger sind drei Dinge, die das Einfalten getrennt behandeln muss:

1. **FeatureId** des Blatts → Contributor-Tracking im Mesh (kernel_core.rs:374–383,
   dc-Contributors), Primitive-Map-Schlüssel (`leaf_primitives`, csg.rs:1622–1634,
   flacher u32), `leaf_world_transforms` (bvh.rs:310), Picking.
2. **Sub-Face-Raum** (`BranchId::from_feature_sub(id, sub)`, csg.rs:70): fixe
   Sub-Nummerierung aus `primitive_halfspaces` (csg.rs:2971 — Box 0–5, Zylinder 0–2, …)
   bzw. `wall_subs`-Tabellen der Sketch-Sweeps (Sketch-Entity-Ordinale,
   surface_registry.rs:207–310). Persistente Queries (Edge-Fillets, Face-Picks,
   Thicken) referenzieren diesen Raum.
3. **Primitive-Parameter** → analytische Carrier (`primitive_halfspaces`), AABB
   (`Aabb::of_primitive`, csg.rs:367), dc-Abwicklung, Opcodes.

Heute wird ein `CsgNode::Offset` über einem Blatt vom Weave bereits analytisch
verarbeitet: der Offset-Arm der Surface-Registry (surface_registry.rs:114–129)
verschiebt jeden Carrier per `offset_halfspace_inward(hs, −δ)` (csg.rs:3447 —
Plane offset−t, Zylinder/Kugel r−t, Torus minor−t; Cone/Gyroid/Blend → None →
tessellierter Fallback). Branch-Enumeration und Pair-Sammlung traversieren Offset
transparent (branch_pairs.rs:303–309, 449; seed_weaver.rs:343). Das Einfalten muss
zu genau dieser Verschiebung deckungsgleich sein.

## 1. kernel_core.rs (Regen → Mesher)

**Befund.** `build_mesh_for` (kernel_core.rs:339–370) baut per
`build_bounded_with_primitives_rounded` und reicht die Map `u32 → Primitive` als
`prim_refs` an `dc::dual_contour_with_primitives` (kernel_core.rs:358–368). Die Map
wird NACH dem Pushdown extrahiert (body.rs:1040–1041) — nach dem Einfalten enthält
sie also automatisch die offsetteten Parameter. Der ImportedMesh-Passthrough
(kernel_core.rs:346–356) matcht auf dem Spine-Node eines Ein-Feature-Bodies; ein
Offset ist immer ein zweites Feature bzw. ein Wrapper → Passthrough greift dann
nicht, kein Konflikt. Contributor-IDs im Mesh sind die Blatt-FeatureIds.

**Sicht:** offsettete Parameter (die Map beschreibt, was der Mesher marcht).
**Bruchrisiko naiv:** keins, SOLANGE der Fold (a) im selben Pass vor der
Map-Extraktion läuft und (b) die Blatt-`id` unverändert lässt — sonst reißen
Contributor-Zuordnung und Map-Schlüssel.
**Mechanismus:** Primitive ist die neue Wahrheit; FeatureId-Erhalt ist Pflicht.
**Test:** bestehende dc-/Mesher-Tests; neu: csg_tests-Unit „Fold erhält Blatt-id
und leaf_primitives-Schlüssel" je Fold-Regel.

## 2. dc.rs (Developable-Simplifikation)

**Befund.** `simplify_dc_mesh` → `simplify_developable_faces` (dc.rs:138–146, 733)
gruppiert Dreiecke per Contributor-Id und Primitive-Match auf Cylinder/Cone
(dc.rs:768–772), liest center/radius/half_height (dc.rs:789–797) plus
`leaf_world_transforms` (dc.rs:145) und wickelt die Mantelfläche in (θ,h) ab.
Wichtig: der Radius skaliert nur die UV-Achse fürs Ear-Clipping (dc.rs:865); die
Retriangulierung re-indiziert ausschließlich vorhandene Mesh-Vertices
(dc.rs:872–879), projiziert nie auf den Carrier zurück — ein um δ falscher Radius
verzerrt nur die Parametrisierung, nie die Geometrie.

**Sicht:** offsettete Parameter (konsistent zur gemarchten Fläche); toleriert aber
auch Original-Radius bei nicht gefalteter Dilation (f−δ), weil nur UV-Skala.
**Bruchrisiko naiv:** gering; einziges Risiko ist ein NEUER Primitive-Variant
(z. B. RoundedBox), der aus dem Cylinder/Cone-Match herausfällt — kein Fehler, nur
entgangene Simplifikation.
**Mechanismus:** nichts nötig bei parametererhaltendem Fold (Cylinder bleibt
Cylinder).
**Test:** bestehende Simplifikations-Tests; optional Smoke „offsetteter Zylinder
wird weiterhin abgewickelt".

## 3. export_mesh.rs (Export-Mesher + Feature-Snap Kanten/Ecken)

**Befund.** Zwei getrennte Identitäts-Konsumzweige:

* Primitive-Map → ManifoldDc/Simplify (export_mesh.rs:548, 777, 1443;
  raw_dense_mesh 440–456; mdc_uniform_export_body 1149, simplify at 1186):
  wie Konsument 2, unkritisch.
* **Feature-Snap:** `build_pin_geometry` (export_mesh.rs:1582) und
  `pin_mesh_to_brep_edges` (1654) holen Kanten-Polylines und Ecken aus dem
  **gewobenen B-Rep** (`ensure_brep` → build_bounded_for_body_local, OHNE
  Pushdown/Fold) und pinnen MDC-Vertices darauf. Schutzmechanismus: Kanten, die
  mehr als `ON_SURFACE_TOL = 0.02` mm vom Export-Feld abliegen, werden als
  Geisterkanten verworfen (export_mesh.rs:1612–1635).

**Sicht:** eindeutig die OFFSETTETE Kante — gepinnt wird auf das, was gemarcht wird.
**Bruchrisiko naiv:** das Kernrisiko von Phase A. Wo der Fold die Nullmenge
tatsächlich korrigiert (Extrude-Profil-Booleans: 2D-Fehler bis 5.8 mm laut Plan §5),
divergieren Mesher-Feld (gefaltet) und Weave-Feld (ungefaltet) um Millimeter →
alle Pins der betroffenen Kanten fallen still weg (Qualitätsverlust, kein Crash).
Bei den exakten Blättern (Kugel/Zylinder/Box …) ist der Fold nullmengen-erhaltend
und nichts divergiert.
**Mechanismus:** der Fold-Pass muss in ALLE Baupfade, auch
`build_bounded_for_body_local` (siehe A2-Punkt 1) — dann sehen Mesher und Weave
dieselbe Nullmenge.
**Test:** Feature-Snap-E2E an offsetteter Kante (Plan §5); zusätzlich Kernel-Test,
der bei einem offsetteten Extrude-mit-Profil-Boolean `edges_pinned > 0` asserted
(fängt die stille Pin-Verwerfung).

## 4. native_prepare.rs (CGAL-Server-Mesher)

**Befund.** Pro Body zwei Quellen ins Interchange-File: Opcodes aus
`build_bounded_with_primitives` + `compile_body` (native_prepare.rs:61–65, mit
Pushdown/Fold) und geschützte 1D-Features als B-Rep-Kanten-Polylines aus
`ensure_brep` (native_prepare.rs:91–110, OHNE Pushdown/Fold). CGAL `Mesh_3`
erzwingt die Kanten als Feature-Kurven im Volumen.

**Sicht:** offsettete Fläche/Kanten (beides beschreibt dasselbe zu meshende Objekt).
**Bruchrisiko naiv:** identisch zu Konsument 3, aber OHNE Toleranz-Guard: liegen
die Weave-Kanten neben der Opcode-Nullmenge, schützt CGAL falsche Kurven — Artefakte
bis Nicht-Konvergenz. Zweites Risiko: jede Fold-Regel ändert den Opcode-Strom →
Golden-Check des Server-Meshers ist laut Projektregel Pflicht.
**Mechanismus:** Weave/Feld-Lockstep (A2-Punkt 1); Golden-Check bei jeder
Opcode-wirksamen Fold-Regel.
**Test:** Golden-Check; neu: Interchange-Paritätstest „jede EDGE-Stützstelle
erfüllt |eval(opcodes)| < tol" für einen offsetteten Body — der Test deckt die
Divergenz generisch ab, unabhängig von der konkreten Fold-Regel.

## 5. api/voxel.rs

**Befund.** `voxelize_occupancy` (api/voxel.rs:91–134) konsumiert ausschließlich
das Feld (`.0` von build_bounded_with_primitives_rounded, api/voxel.rs:101), keine
Primitive-Map, keine Blatt-Identität.

**Sicht:** offsettet (Feld). **Bruchrisiko:** keins; wo der Fold die Nullmenge
korrigiert (Profil-Booleans), ändert sich die Occupancy absichtlich.
**Mechanismus:** keiner. **Test:** bestehende Voxel-Tests genügen.

## 6. api/lichen.rs

**Befund.** Der Lichen-Mesher konsumiert BEIDE Welten gleichzeitig: Welt-Felder
aus `build_bounded_with_primitives` (api/lichen.rs:75, 4709 — mit Pushdown) UND das
gewobene B-Rep als Constraint-Geometrie (Kanten/Ecken/Faces, api/lichen.rs:237–281,
470–849) PLUS `branch_halfspace_map` über den Welt-Baum (api/lichen.rs:849) für
analytische Face-Normalen/Projektion. Heute ist das konsistent, weil der
Registry-Offset-Arm (surface_registry.rs:114–129) und der Pushdown dieselben
Carrier liefern — `r − (−δ)` und `r + δ` sind IEEE-bit-identisch.

**Sicht:** offsettet (alle drei Quellen müssen dieselbe Fläche beschreiben).
**Bruchrisiko naiv:** dreifache Divergenz möglich: Feld (gefaltet) vs. B-Rep
(ungewoben-ungefaltet) vs. hs_map (gefaltet). Zusätzlich Carrier-Familienwechsel:
ein gefaltetes Erosions-Cone-Blatt liefert via `primitive_halfspaces` einen
analytischen Cone-Carrier, während der heutige Offset-Arm für Cone `None` liefert
(csg.rs:3467) — die hs_map bekäme Einträge, zu denen der Weave tessellierte Faces
gebaut hat. Kein Absturz (Lookup per Branch-Key), aber Normalen-Quelle und
Facegeometrie stammen dann aus verschiedenen Berechnungen.
**Mechanismus:** ein gemeinsamer Fold-Pass für alle Baupfade; Carrier-Ableitung
danach ausschließlich aus dem gefalteten Blatt (der Offset-Arm wird für gefaltete
Fälle zum toten Code und bleibt nur für die Stopp-Knoten-Reste zuständig).
**Test:** Lichen-Smoke auf offsettetem Body (Kantenzug bleibt geschlossen,
`brep_dist`-Korridor greift).

## 7. brep/seed_weaver.rs (+ branch_pairs, surface_registry, coincidence)

**Befund.** Der parametrische Weave enumeriert Blatt-Branches und Paar-Kandidaten
durch Offset-Knoten hindurch (branch_pairs.rs:303–309, 449; seed_weaver.rs:324–347)
und bezieht die analytischen Carrier aus `branch_halfspace_map` →
`primitive_halfspaces(primitive)` je Blatt (surface_registry.rs:30–56) bzw. dem
Offset-Arm (114–129). SSI-Seeds, Kanten-Trace und Koinzidenz-Klassifikation
rechnen auf diesen Carriern; Projektregel: Koinzidenz analytisch, nie
Sampling-Epsilon.

**Sicht:** offsettete Carrier — die gewobene Kante ist die gerenderte.
**Bruchrisiko naiv (größtes im Audit):**
1. Fold nur in den with_primitives/rounded-Pfaden → der Weave-Baupfad
   (body.rs:1011, ohne Pushdown) sieht weiter Original-Blätter + Offset-Knoten.
   Für in-family-Folds bit-identisch, für Profil-Bereinigung (Clipper) NICHT →
   Kanten liegen neben dem gemarchten Feld (Konsument 3/4 erben das).
2. Neuer Primitive-Variant (Box+δ → RoundedBox): `primitive_halfspaces` kennt ihn
   nicht → gar keine Carrier → ganzer Body fällt vom parametrischen Weave auf den
   Octree zurück; zusätzlich explodiert der Sub-Face-Raum (Box 6 Subs → RoundedBox
   6 Ebenen + 12 Zylinder + 8 Kugeln), was JEDE persistierte Branch-Referenz
   entwertet.
3. Clipper-umgebautes Extrude-Profil ohne Entity-Provenance → `wall_subs`
   (csg.rs:795, surface_registry.rs:207–310) zeigen auf tote/verschobene
   Sketch-Entity-Ordinale → Face-Naming, Face-Picking und persistierte Queries
   (Fillets!) rutschen auf falsche Wände.
4. Bit-Drift zwischen Fold-Arithmetik und `offset_halfspace_inward`: die
   analytische Koinzidenz-Erkennung vergleicht Carrier-Parameter exakt; ein via
   anderem Rechenweg gefaltetes r+δ, das nicht bit-gleich zum verschobenen
   Carrier eines Nachbar-Stopp-Knotens ist, lässt Koinzidenz-Paare auseinanderfallen.
**Mechanismus:** Fold sub-face-erhaltend einschränken (keine neuen Varianten, s.
Gesamtempfehlung), Fold-Pass in den local-Build ziehen, Fold-Arithmetik als
gemeinsame Helper neben `offset_halfspace_inward` implementieren (eine Funktion,
zwei Aufrufer), `wall_subs`-Propagation durch den 2D-Offset.
**Test:** csg_tests-Unit „Fold(Blatt) ≡ offset_halfspace_inward(Carrier) bit-exakt"
je Familie; Weave-E2E: offsetteter Box/Zylinder-Boolean behält analytische Kanten
(naming_gate grün, kein Octree-Fallback).

## 8. brep/face_offset.rs (Thicken-Carrier)

**Befund.** `face_carrier` (face_offset.rs:83–109, Aufrufer
features/context_resolve.rs:356–398) rekonstruiert den Carrier eines gepickten
Faces aus dem **Feature-Spine-Node** (`source.node` — Original-Definition, nie der
Regen-Baum) und verifiziert per |SDF|-Probe an Face-Loop-Samples (tol ≈ 1e-3·diag).
Shell/Offset-Wrapper liefern bewusst keinen Carrier (face_offset.rs:56–66,
133–136) → ehrlicher Fehler „no single closed carrier".

**Sicht:** ORIGINAL — by design (der Thicken-Band wird aus der Quelldefinition
aufgebaut, `remap_leaf_ids` vergibt ohnehin frische Ids, face_offset.rs:140).
**Bruchrisiko naiv:** keins durch Phase A — der Fold findet zur Regen-Zeit statt
und berührt den Spine nicht. Der heutige Zustand „Face auf offsettetem Body kann
nicht gethickent werden" (Probe schlägt fehl, weil die Samples δ neben dem
Original-Carrier liegen) bleibt exakt so bestehen; Phase A könnte ihn später sogar
beheben, indem sie denselben Fold auf den Carrier anwendet, bevor die Probe läuft.
**Mechanismus:** keiner für A2; Verbesserung als Follow-up notieren.
**Test:** bestehender Fehlerpfad (features_tests.rs:4188 ff. Umfeld) genügt.

## 9. step_classifier.rs / step_commands.rs (STEP-Export, Original-Sicht)

**Befund.** `classify_one_feature` (step_classifier.rs:136–191) und
`build_step_commands` (step_commands.rs:25–67, via `export_primitive`,
csg.rs:1106) arbeiten ausschließlich auf dem Feature-Spine. `peel_modifiers`
(step_classifier.rs:103–134) schält Twist/Bend/Displace/Shell — NICHT Offset: ein
Offset-Feature fällt auf den Composite-Arm und wird `Excluded` (step_classifier.rs:143).
Das ist der Ist-Zustand vor und nach Phase A.

**Sicht:** ORIGINAL — korrekt, denn dieser Pfad baut Geometrie aus
Feature-Definitionen neu (OCCT), nicht aus dem Feld.
**Bruchrisiko naiv:** keins, solange kein neuer Primitive-Variant eingeführt wird
(dann erzwingt der exhaustive Match in `classify_primitive`, step_classifier.rs:193,
ohnehin eine Compile-Zeit-Entscheidung).
**Mechanismus:** keiner. Follow-up (nicht A2): Offset-auf-Analytik könnte nach
Phase A als Hard klassifiziert und mit demselben Fold-Code in einen exportierbaren
Primitive übersetzt werden — dieselbe Regel-SSOT, zweiter Aufrufer.
**Test:** Unit „Offset-Feature bleibt Excluded" als Verhaltens-Anker.

## 10. Weitere gefundene Konsumenten (Kurzform)

* **Raycast/Pick + GPU-SDF-Compile** (body.rs:960, kernel_core.rs:385): reine
  Feld-Konsumenten über die pushdown-Pfade — folgen dem Fold automatisch. Ein
  Regen mit Fold, Pick ohne Fold, gäbe Pick-Geometrie ≠ Render-Geometrie; da beide
  denselben Builder nutzen, kein Zusatzmechanismus nötig.
* **AABB** (`Aabb::of_primitive` csg.rs:367; BoundedCsg::Offset-AABB
  bvh.rs:1138–1152): heute konservativ |δ|-inflationiert; nach dem Fold liefert das
  Blatt die exakte AABB aus den neuen Parametern — Verbesserung, kein Risiko.
* **Body-Cache-Hash** (csg.rs:687–692, `hash_body_payload_split`): hasht den
  Spine-seitigen Baum vor dem Regen — Fold ist deterministische Funktion der
  gehashten Eingaben, Cache-Semantik unverändert.

## Gesamtempfehlung: Identitätsmodell des eingefalteten Blatts

**„Die Primitive-Parameter sind nach dem Fold die einzige Wahrheit; die Identität
trägt weiterhin die unveränderte FeatureId samt unverändertem Sub-Face-Raum. Keine
Doppel-Identität am Blatt."**

Begründung: Die Trennung Original-Sicht vs. offsettete Sicht existiert in der
Architektur bereits strukturell — alle Original-Konsumenten (Feature-UI,
step_classifier/step_commands, face_offset) lesen den Feature-Spine
(`DecodedFeature.node` / `export_primitive`), den der Regen-zeitige Fold nie
anfasst; alle Geometrie-Konsumenten lesen einen der Regen-Baupfade. Ein
Doppel-Identitäts-Feld am Blatt hätte keinen einzigen Konsumenten. Damit das
trägt, wird Phase A auf **sub-face-erhaltende Folds** eingeschränkt: gefaltet wird
nur, was Familie UND Sub-Nummerierung erhält — Erosion in-family (Box→kleinere
Box, r−|δ|, Cone-Apex-Shift, Extrude mit geschrumpftem Profil + wall_subs-
Propagation) sowie die Profil-Bereinigung; **Dilation bleibt f−δ und der geplante
RoundedBox-Umbau entfällt**, denn f−δ ist auf einem exakten Blatt bereits das
exakte Dilations-SDF (dasselbe Argument, mit dem der Plan beim Extrude auf den
+δ-Umbau verzichtet) — ein neuer Primitive-Variant würde dagegen
primitive_halfspaces/Weave/Naming/dc/step_classifier gleichzeitig aufreißen und
den persistierten BranchId-Raum entwerten, ohne Exaktheit zu gewinnen. Der Fold
läuft als EIN gemeinsamer Pass direkt nach `push_down_offsets` in ALLEN
Baupfaden (inkl. `build_bounded_for_body_local`), und seine Arithmetik ist
dieselbe Funktion, die auch `offset_halfspace_inward` speist — damit sind Feld,
Weave-Carrier und hs_map bit-konsistent und die analytische Koinzidenz-Leitregel
bleibt intakt.

## A2-Pflichtanpassungen VOR dem ersten Parameter-Umbau

1. **Fold-Pass in den Weave-Baupfad:** `build_bounded_for_body_local`
   (body.rs:999–1013) um denselben Normalisierungsschritt ergänzen wie
   body.rs:988/1040 (Pushdown + Fold), sonst divergieren Mesher-Feld und
   B-Rep-Kanten bei jeder nullmengen-korrigierenden Regel → Feature-Snap-Pins
   fallen still (export_mesh.rs:1612), CGAL-Schutzkanten liegen neben der Fläche
   (native_prepare.rs:103).
2. **Gemeinsame Fold-Arithmetik-SSOT:** die per-Familie-Regeln als eine Funktion
   neben `offset_halfspace_inward` (csg.rs:3447) implementieren und von beiden
   Seiten (Blatt-Fold, Carrier-Shift der verbleibenden Stopp-Knoten) aufrufen;
   Unit-Test „bit-exakt gleich" je Familie.
3. **FeatureId- und Sub-Raum-Erhalt vertraglich machen:** Fold-Regeln dürfen `id`
   und Sub-Nummerierung nie ändern; Unit-Test über `leaf_primitives`-Schlüssel und
   `branch_halfspace_map`-Keys vor/nach Fold.
4. **wall_subs-Propagation durch den 2D-Offset:** der Clipper-/cavalier_contours-
   Spike (Plan §5) bekommt als hartes Auswahlkriterium „Entity-Provenance pro
   Ausgabesegment", und die Extrude/Revolve-Fold-Regel schreibt die propagierten
   Ordinale in `wall_subs` zurück; neue Verrundungsbögen an konvexen Ecken erben
   deterministisch (z. B. vom im Umlaufsinn vorangehenden Segment).
5. **Kein neuer Primitive-Variant in Phase A** (RoundedBox streichen, Dilation
   bleibt f−δ) — falls später doch nötig, ist das ein eigener Konsumenten-Umbau
   (primitive_halfspaces, naming, dc-Matches, step_classifier) mit eigenem Audit.
6. **Regressionsnetz scharf schalten:** CGAL-Golden-Check bei jeder Fold-Regel;
   neuer Interchange-Paritätstest (EDGE-Punkte gegen Opcode-Feld, Konsument 4);
   Feature-Snap-E2E an offsetteter Kante inkl. `edges_pinned > 0`-Assert
   (Konsument 3); Weave-E2E „offsetteter Boolean behält analytische Kanten".

## Freigabe

**Phase A2 ist freigegeben**, sobald die Punkte 1–5 umgesetzt sind (Punkt 6
begleitet die erste Fold-Regel). Es wurden keine architektonischen Blocker
gefunden: kein Konsument braucht eine Doppel-Identität, kein persistiertes Format
ist betroffen, und die beiden echten Bruchstellen (Weave-Baupfad ohne
Normalisierung; wall_subs unter Profil-Rewrite) sind lokal behebbar. NICHT
freigegeben ist unter diesem Audit: das Einführen neuer Primitive-Varianten und
achsnahe Revolve-Folds (#390 bleibt Vorbedingung, Plan §5).
