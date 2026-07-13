# Onshape-Import — Fortschritts-Log

Arbeitslog zum Vier-Schritte-Plan (Query-System-Erweiterung + Import-Pipeline).
Plan-Kontext: docs/ONSHAPE-IMPORT-PLAN.md. Testmodell: „Kabelbox" (7 Sketches
auf 7 verschiedenen Ebenen), live unter den IDs in web/.onshape.local.json;
Roh-Features + dekodierte Queries als Fixture in web/src/onshape/fixtures/.

## Stand in einem Satz

Schritte 1–4 sind umgesetzt und getestet; die Abnahme am echten Kernel zeigt
4 von 7 Kabelbox-Sketches korrekt und parametrisch platziert. Die restlichen 3
scheitern SICHTBAR an einer Kernel-Grenze (FieldDraft trennt Flächen nur an
Knicken → ein abgerundetes Rechteck hat EINE Mantelfläche), nicht an der
Import-Logik. Damit ist die Definition of Done NICHT erfüllt; der Blocker ist
benannt, eingegrenzt und als nächstes Projekt beschrieben (§ Schritt 5).

## Empirische Basis (2026-07-13, Live-API)

Onshapes queryString ist `query=qCompressed(1.0,"<payload>",id);` mit zwei
Payload-Kodierungen: Klartext-Dictionary (`%B5$…`) und zlib/base64
(`&<hexlen>$eJ…`). Dekodierte Struktur pro Sketch-Ebene der Kabelbox:

- Sketch 1, 3: Default-Ebene (`operationId=[Topplane]`, queryType DUMMY).
- Sketch 2, 6: `CAP_FACE` des Extrudes mit `isStart=T/F` — Rolle + Seite.
- Sketch 5: `SWEPT_FACE` des Extrudes, Herkunft = Sketch-Entity
  (`sketchEntityId="<wireId>.left"`, ORIGINAL_DEPENDENCY).
- Sketch 4, 8: `SWEPT_FACE`, disambiguiert per TOPOLOGY-Nachbarschaft
  (`SWEPT_EDGE` → TRUE_DEPENDENCY → `BLEND_FACE` des Fillets) und
  Boolean-`SPLIT` (Herkunfts-Operanden beider Extrudes).

Die drei Kernel-Lücken des Auftrags sind damit bestätigt: Rollen (Schritt 1),
stabile Herkunfts-Entity-Subs (Schritt 2), Schnitt/Nachbarschafts-Filter
(Schritt 3).

## Schritt 1 — FaceRole + qCreatedBy(op, ROLE) — ERLEDIGT (70bc032)

- `FaceRole` = WALL | CAP_TOP | CAP_BOTTOM | INNER_WALL | BLEND in
  queries.rs; `face_role(&Face)` als einzige Autorität (Blend via
  `HalfSpace::Blend`-Surface VOR den Sub-Bits geprüft — Fillet mintet sub 0).
- Parser/Display/Resolver + Web-Spiegel (queryAcceptedByFilter → FACE).
- Hash-neutral, keine Migration. b2c0472-Wiederbelebung geprüft: der
  entfernte Code war ein Feature-Level-Existenz-Evaluator, für die
  element-level Rollen-/Schnitt-Auflösung unbrauchbar — nicht wiederbelebt.

## Schritt 2 — Stabile Sketch-Entity-Ordinals — UMGESETZT

Implementiert: `Primitive::{Extrude,Revolve,Helix}.wall_subs` (protocol) →
`CsgNode`/`BoundedCsg`-Leaves (`Arc<Vec<u32>>`) → Konsumenten mappen über
`csg::map_wall_sub(ws, g)`: Contributor-Fns (extrude/revolve/helix ±retained),
Retained-Closures (csg+bvh, je 3), `branch_pairs::push_sketch_walls`,
`sketch_surfaces`-Collectors, `surface_registry` (Profile-Carriers +
Shell-Nachbarlogik jetzt positional-über-Tabelle statt `g±1`),
`wireframes` (Revolve/Helix-Corner-Kanten), `edge_walker::extrude_rim_corner`
(invers Tabelle→Position). Tabellenbau: `sketch/lower.rs`
(`build_sketch2d_traced` + `wall_sub_table`), Allokations-Autorität =
Registry-Replay (`replay_allocations`); `resolve_profile_query` liefert die
Trace als drittes Element; Revolve/Helix transformieren die Trace mit M mit
(`transform_traced_prims`, Rectangle→Polygon-Gruppen-Reorder [b,r,t,l]);
Revolve-Achsen-Strip über strip-aware Matching. Kein Hot-Path-Umbau: interne
Nummerierung bleibt positionell, Übersetzung nur an der Branch-Grenze.

Repräsentanten konkret: Line/Arc/Circle/Spline/Ellipse = Curve-Entity-ID
(min über G1-Gruppe); Slot = erste Punkt-Entity des Calls (voll G1 = 1 Wand);
Polygon/RegularPolygon = Vertex-Punkt-ID `first+k` je Kante k. Fallback
(ganze Tabelle None): sketch_path (SVG, nur Punkt-Entities), sketch_mirror
(Alias), Replay-Fehler, Überlauf ≥ 0x7FE.

Tests: 6 Unit (lower) + 3 E2E (features_tests): Wall-Subs = Line-Entity-IDs
im gewobenen B-Rep, qCreatedBy(op, ROLE) auf echtem Sweep, Heal-Test über
Entity-ID-Shift (Migrationsgarantie v1→v2). `NAMING_SCHEMA_VERSION = 2` in
brep/naming.rs dokumentiert.

### Entscheidungen

- **Repräsentant:** Sub-ID einer Mantelfläche = MIN der Solver-Curve-Entity-IDs
  (Line/Arc/Circle/Spline/…) über die Segmente ihrer G1-Randgruppe. Damit
  bleibt die ID stabil, wenn VOR dem Profil liegende Sub-Calls eingefügt/
  entfernt werden, und deterministisch, wenn eine G1-Gruppe mehrere Kanten
  umfasst (Line+Tangent-Arc-Kette, Slot).
- **Allokations-Autorität:** Replay der `SketchToolRegistry` über die
  Sub-Calls in einen Scratch-`SketchState`; pro Call werden die neu
  allozierten Curve-Entities abgelesen. KEIN vierter Lockstep — die
  `apply()`-Implementierungen selbst sind die Quelle.
- **Träger:** `Primitive::{Extrude,Revolve,Helix}.wall_subs: Option<Vec<u32>>`
  (serde default) → `BoundedSketch2d.wall_subs` → Konsumenten mappen
  positional→stabil über `wall_sub(g)`. Interne Nummerierung (Offsets im
  Sketch2d-Baum, sdf_with_edges) bleibt UNVERÄNDERT positionell — die
  Übersetzung passiert ausschließlich an der Branch-Grenze.
- **Berechnung** nach der Profil-Restriktion (qRegion!) im Feature-execute:
  instrumentiertes Lowering liefert (Prim, lokale Wall-IDs) in Baum-Reihenfolge;
  der (ggf. restrizierte) Baum wird per in-order strukturellem Matching
  dagegen gelegt; lokale Tabellen konkateniert = globale Tabelle.
- **Fallback = None (positionell, Status quo)** wenn irgendein Prim keine
  IDs liefert: sketch_path (SVG-Import, nur Punkt-Entities), sketch_mirror
  (Alias-Kollision der Kopie), ID-Überlauf ≥ 0x7FE. Kein stilles Falschbinden.
- **Nachbar-Arithmetik:** surface_registry.rs (RevolvedProfile
  extend_start/end via g±1) bleibt positionell und mappt erst beim Insert.

### Migration

- Hashes ändern sich nur für Mantelflächen von Sketch-Sweeps mit Tabelle.
- Persistierte Referenzen (`qFace(hash @ fingerprint)`) heilen über den
  N2/N3-Resolver (Fingerprint-Override); Migrationstest deckt das ab.
- `NAMING_SCHEMA_VERSION`-Konstante in brep/naming.rs eingeführt (Doku-Marke,
  Branch-Subs werden nicht persistiert — nur Query-Hashes, und die heilen).

## Schritt 3 — Query-AST — UMGESETZT

Neue Formen (queries.rs, klammer-bewusster rekursiver Parser
`split_top_level`, Display = kanonische Wire-Form, beliebig schachtelbar):
- `qWall(op, entity)` — Mantelfläche von Sweep `op` mit stabilem
  Entity-Sub `entity` (Rolle WALL erzwungen, Innenwände ausgeschlossen).
- `qIntersection(a, b)` — Mengen-Schnitt per id_hash.
- `qAdjacentTo(a, b)` — Flächen aus a, die eine B-Rep-Kante (Loop-Kanten)
  mit einer Fläche aus b teilen — FieldDrafts Gegenstück zu Onshapes
  TOPOLOGY-/SPLIT-Disambiguierung.
b2c0472 (Feature-Level-Evaluator) blieb unbrauchbar — element-level neu.
Web-Spiegel: alle drei Formen FACE-level in queryAcceptedByFilter.
Tests: Parse/Roundtrip (inkl. Nesting), Wall-by-Sub, Schnitt,
Blend-Nachbarschafts-Pick am synthetischen 3-Flächen-Body.

## Schritt 4 — Übersetzer + Pipeline + UI — IN ARBEIT

Scope-Erkenntnis: Die Kabelbox-DoD („jeder Sketch auf korrekter Fläche,
Basiskörper ändern → Ebenen wandern mit") erfordert, dass die referenzierten
KÖRPER existieren — der Import muss also neben den 7 Sketches auch die
Kabelbox-Features minimal mitbringen (Extrude new/add/remove, Shell,
Fillet, Boolean-SPLIT-Scope), sonst lösen die Ebenen-Queries ins Leere.
Der volle generische Feature-Import (alle End-Conditions etc.) bleibt
Folgearbeit.

Onshape-queryString-Format (empirisch): `query=qCompressed(1.0,"…",id);`
Payload `%…` = Klartext-BTON-Dictionary, `&<hexlen>$<b64>` = zlib.
BTON-Tokens: `M<n>` Map, `A<n>` Array, `S<len>$s` String,
`S<l1>.<l2>$s1s2` zweiteilig, `S-<ref>.<l>$suffix` Ref+Suffix,
`B<len>$Enum` + Wert-String, `R<hex>` String-Backref, `C<hex>` Klasse,
`D<±hex>` Zahl, `T`/`F` Bool. Relevante Felder: queryType (DUMMY /
SKETCH_ENTITY / CAP_FACE / SWEPT_FACE / BLEND_FACE / SPLIT / INTERSECT),
operationId (`<featureId>op<Kind>`), isStart, sketchEntityId
(`<wireId>.<entity>`), derivedFrom-Ketten, disambiguationData
(ORIGINAL_DEPENDENCY originals / TOPOLOGY entities).

### Schritt-4-Bauplan (verbindlich, Stand 129d366)

Erledigt: BTON-Decoder `web/src/onshape/onshapeQuery.ts` (decodePlaneQuery →
OnshapePlaneRef: default/cap/swept±blend±splitPeer, korpus-getestet),
`Kernel.resolveQuery(q)` (wasm, JSON-Matches mit centroid),
`translateOnshapeSketch(...).entityIds` (Onshape-entityId → FD-Solver-ID).

Noch zu bauen (`web/src/onshape/onshapeImport.ts` + UI):
1. `importOnshapeFeatures(featuresJson, kernel)` — sequenziell über die
   Feature-Liste, Onshape-featureId → FD-id-Map pflegen:
   - newSketch: planeQuery bauen (s.u.), `kernel.addSketch(planeQuery,
     subCalls)`; entityIds je Onshape-Sketch merken.
   - extrude: profiles=`qCreatedBy(<fdSketch>)` (Sketch-Herkunft: wireOp-
     FeatureId aus entities-Query bzw. `qSketchRegion(id+"<fid>")`-Regex);
     depth aus SI-value×1000 (BLIND), THROUGH_ALL→große Tiefe+Warnung,
     UP_TO_SURFACE→BLIND-Näherung+Warnung; oppositeDirection→Vorzeichen;
     operation NEW/ADD/REMOVE→new/add/remove; booleanScope(SWEPT_BODY von
     X)→targets=`qCreatedBy(<fdX>)`. `kernel.addFeatureCall("extrude",…)`.
   - shell: open_faces = Cap-Query übersetzt (`qCreatedBy(fd, CAP_*)`),
     thickness SI→mm. Typ "shell".
   - fillet: je entities-Edge-Query (INTERSECT aus CAP_FACE×SWEPT_FACE):
     `qCommonEdge(<capQuery>, <sweptQuery>)` → je ein "fillet_edge"
     (edge, blend_radius=radius·1000).
2. planeQuery-Bau aus OnshapePlaneRef:
   - default: top→qCreatedBy(origin_plane_xy), front→…_xz, right→…_yz.
   - cap: isStart=true→CAP_BOTTOM (Sketch-Seite), false→CAP_TOP —
     Zuordnung im E2E VERIFIZIEREN, ggf. drehen.
   - swept: `qWall(<fdExtrude>, <ord>)`, ord = entityIds[origin.entityId]
     des Quell-Sketches (Curve-ID = Wall-Sub per Schritt 2).
   - swept+blend/split (Kabelbox 4/8; Achtung: Blend-Fillet FH63… ist im
     Dokument GELÖSCHT → nur splitPeer nutzbar):
     `qAdjacentTo(qWall(e1,ord), qWall(<fdPeer>, <peerOrd>))`.
   - Eindeutigkeit VOR dem Speichern via kernel.resolveQuery prüfen:
     1 Treffer → Query speichern; 0/mehrere → Import-Fehler im Dialog
     (sichtbar, mit Feature-Name) — NIE still XY.
3. UI: Button „Aus Onshape importieren…" in
   web/src/account/ProjectsBrowser.tsx ({authed}-Block, testid
   projects-onshape-import), Modal nach SettingsMenu-Muster (fd-Klasse,
   Overlay/Backdrop/Escape), styles/components/onshape-import.css +
   @import in styles/index.css; URL-Feld (d/w/e-Parse), platformApi:
   getOnshapeElements/getOnshapeFeatures (`request` auf
   /api/onshape/elements|features?documentId=…). Import = neues Projekt
   (createProject) + öffnen (armProjectThumbnailCapture + onOpen) +
   importOnshapeFeatures gegen den Kernel des geöffneten Projekts.
4. Validierung: `just build-wasm` (stale-WASM-Falle!), In-Browser-Test
   (HTTPS/Tailscale, docker compose Server mit ONSHAPE_ACCESS_KEY/
   SECRET_KEY aus web/.onshape.local.json in server/.env), Kabelbox
   importieren, 7 Sketch-Ebenen prüfen, Basiskörper-Änderungstest.
   Release-Note + Versions-Bump erst mit der UI.

## Schritt 5 — E2E am echten Kernel

Der Import läuft in `web/src/onshape/onshapeImportE2E.test.ts` gegen den
ECHTEN WASM-Kernel (nicht Mock). Ergebnis:

**Was funktioniert (verifiziert):** 4 der 7 Kabelbox-Sketches werden
importiert und liegen auf einer korrekt aufgelösten Fläche — 2 auf
Standard-Ebenen, 2 auf Extrude-Caps (`qCreatedBy(op, CAP_*)`). Sie
regenerieren fehlerfrei, und ihre Ebenen-FRAMES wandern mit, wenn man die
Tiefe des Grundkörpers ändert (parametrisch, keine tote Geometrie). Extrude,
Shell und Fillet werden mit übersetzt.

**Was NICHT funktioniert (offener Blocker):** Die 3 Sketches, die in Onshape
auf einer SEITE des abgerundeten Grundkörpers sitzen (Sketch 4, 5, 8), lassen
sich nicht platzieren. Sie scheitern SICHTBAR mit benannter Ursache — kein
stiller XY-Fallback (der Kernel hat auch keinen: eine nicht auflösbare
Ebene ist ein harter Feature-Fehler).

### Wurzel-Ursache: FieldDraft trennt Flächen nur an Knicken

Eine G1-Randgruppe = eine B-Rep-Fläche, und G1 heißt „tangentenstetig". Beim
abgerundeten Rechteck (4 Linien + 4 tangentiale Fillet-Bögen) ist die GESAMTE
Mantelfläche damit EINE Gruppe. Die „linke Wand", auf der Onshapes Sketch 5
sitzt, existiert in FieldDraft schlicht nicht. Zusätzlich liftet
`lift_path_group` nur homogene Gruppen (alle Linien → Ebene, ko-zirkulare
Bögen → Zylinder), eine gemischte Linie-Bogen-Gruppe bekommt gar keinen
analytischen Träger und fällt auf Tessellierung zurück.

### Irrweg I-1: G1-Gruppen an der Trägerflächen-Grenze trennen (revertiert)

Naheliegend: `g1_continuous` zusätzlich verlangen lassen, dass beide Segmente
DENSELBEN Träger haben (Linie↔Bogen trennt, kollineare Linien / ko-zirkulare
Bögen / Bézier-Ketten verschmelzen weiter). Damit hätte das abgerundete
Rechteck 8 analytische Seitenflächen und `qWall(op, entity)` fände die linke
Wand.

**Warum es nicht geht:** Der Split erzeugt Flächengrenzen, die TANGENTIAL
sind (Ebene berührt Zylinder knickfrei). FieldDrafts Weaver findet Kanten
aber über Branch-Kreuzungen im SDF — eine tangentiale Naht ist numerisch
degeneriert und wird nicht gewoben. Der Zylinder behält dann nur EINEN Rand,
und der parametrische Export hält ihn für eine Polkappe
(`spline_dome_with_pole_exports_parametric_watertight` schlug fehl:
„cylinder cannot close onto the axis"). Der Split wurde zurückgenommen.

**Der saubere Weg** ist damit klar benannt und ein eigenes Projekt:
tangentiale Nähte weben (Trägerflächen-Split + SSI/Weaver/Tessellator/
Parametrik-Export für berührende Flächenpaare). Erst danach kann FieldDraft
Onshape-Modelle mit verrundeten Kanten voll abbilden.

### Echte Bugs, die der E2E-Lauf mitgefunden hat

- `Context::resolve_plane_frame` und `resolve_open_face` matchten
  Query-Formen HARTKODIERT (`qCreatedBy`/`qFaceOf`/`qFace`). Jede reichere
  Form fiel durch — eine Shell mit `open_faces: qCreatedBy(op, CAP_TOP)`
  öffnete deshalb STILL gar nichts. Beide routen jetzt als Fallback über den
  gemeinsamen Element-Resolver (`resolve_query_topo`), sind also automatisch
  für jede künftige Query-Form korrekt.
- `web/src/sketchPlaneFrame.ts` spiegelte die Query-Auflösung in JS und kannte
  die neuen Formen nicht (imported Sketches wären im Overlay unsichtbar
  gewesen). Unbekannte Formen gehen jetzt über `kernel.resolveQuery`.
- `wall_sub_table` sperrt Tabellen mit doppeltem Repräsentanten (zwei Wände
  auf einem Branch wären eine Fläche).

## Schritt 6 — Feature-Import vervollständigt (v0.6.570)

- **THROUGH_ALL exakt statt genähert:** Die Tiefe wird an der Szene GEMESSEN
  (2× Szenen-Diagonale ab der Sketch-Ebene). Der Überschuss schneidet nur
  Leerraum weg, das Ergebnis ist identisch zum echten Durchbruch — anders als
  bei der alten 1000-mm-Konstante, die je nach Bauteilgröße zu klein oder
  absurd groß war. (Kabelbox: 755 mm statt 1000 mm.)
- **UP_TO_SURFACE / UP_TO_FACE:** Zielfläche auflösen, Abstand der Sketch-Ebene
  zu ihr entlang der Extrude-Achse messen, als BLIND-Tiefe übernehmen.
  Geometrisch exakt, aber GEBACKEN — die Tiefe folgt der Zielfläche nicht mehr,
  wenn die sich bewegt. Das wird als Warnung gemeldet, nicht verschwiegen. Ein
  assoziatives „bis zur Fläche" bräuchte einen echten Extrude-Parameter im
  Kernel (heute gibt es `up_to_face` nur bei Thicken).
- **Symmetrisch / zweite Richtung:** direkt auf `symmetric` bzw. `depth_back`.
- **Draft:** wird nicht unterstützt → Warnung statt stillem Ignorieren.
- **Fillet-Kanten:** Shell-Innenkanten (Boden trifft Innenwand) sind jetzt
  adressierbar. Dafür war eine Lücke im Rollen-Vokabular zu schließen:
  `INNER_WALL` fasste Innenwand UND Innen-Cap zusammen, womit genau die
  Kante zwischen ihnen unbenennbar war. Neu: `INNER_CAP_TOP` /
  `INNER_CAP_BOTTOM` (der Innen-Cap trägt den Cap-Sub MIT Inner-Flag —
  `face_role` maskiert das Flag jetzt vor der Klassifikation).
- **Fillet-Optionen:** nicht-EDGE-Typ → Fehler; andere Steuerung/Querschnitt
  (Konisch, Chord) → Warnung + Radius-Näherung.

### Offene Lücke: BTON-String-Rückverweise

Onshape komprimiert wiederholte Strings zu Rückverweisen (`R<hex>`). Deren
Index-Schema ist noch nicht geknackt (weder Token- noch Part-Zählung passt
konsistent). Folge: zwei GESCHWISTER-Kanten eines Fillets dekodieren zu
denselben Markern und kollabieren zu EINER Query. Der Import dedupliziert und
meldet das, statt dieselbe Kante doppelt zu verrunden. Das Knacken der
Ref-Tabelle ist die Voraussetzung dafür, Kantenmengen exakt zu übernehmen.

## Schritt 7 — Sketch-Übersetzer-Lücken (v0.6.570)

**Der stille Bug:** Onshape bindet die ENDPUNKTE eines Fillet-Bogens an die
angrenzenden Linien (`<arc>.start` ↔ `<line>.end`). Der Übersetzer registrierte
für Bögen und Kreise aber nur die Kurve selbst — jede dieser Kopplungen fiel
durch („unresolved ref") und wurde stumm verworfen. Im Kabelbox-Basis-Sketch
gingen so 8 von 36 Constraints verloren; die verrundete Ecke hatte im Solver
keinen Halt mehr. Jetzt trägt jede Kurve Mittelpunkt, Start und Ende in die
Entity-Map ein (inkl. der `<entityId>.start`-Aliasse, die Onshapes Constraints
benutzen).

**Neu übersetzt:** alle Bemaßungen — DISTANCE, LENGTH (Onshape bemaßt die
LINIE, der Kernel misst Punkt-zu-Punkt → über die Endpunkt-Tabelle aufgelöst),
ANGLE, RADIUS, DIAMETER, mit Einheiten-Umrechnung (mm/cm/m/in/ft, deg/rad) —
sowie MIDPOINT, FIX und POINT_ON_OBJECT. Zusätzlich werden Enum-Parameter
(`sketchToolType=FILLET`) nicht mehr als Entity-Referenz missdeutet, was sonst
das ganze Constraint verwarf.

**Abdeckung an der Kabelbox:** 241 von 297 Constraints (81 %); im Basis-Sketch
34/36 statt vorher 22/36.

**Bewusst offen** (Geometrie ist da, nur die Assoziativität fehlt):
- `PROJECTED` (29×) — bindet eine Sketch-Kurve an eine Körperkante. FieldDraft
  hat mit `sketch_project` das Gegenstück; es fehlt die Übersetzung der
  Onshape-Kanten-Query auf eine FieldDraft-Query.
- `OFFSET` (16×), `LINEAR_PATTERN` (2×) — kein direktes Kernel-Gegenstück.
- `MIRROR` (4×) — Onshape spiegelt um eine EXTERNE Linie (oft eine
  Ursprungsachse); FieldDrafts `symmetric` braucht eine Achsen-ENTITY im
  Sketch, die es dafür nicht gibt.

## Nächste Schritte (empfohlene Reihenfolge)

1. **Tangentiale Nähte weben** (der Blocker, eigenes Projekt): Trägerflächen-
   Split in der G1-Gruppierung + Weaver/SSI, Tessellator und Parametrik-Export
   für berührende Flächenpaare. Danach: Kabelbox vollständig, und abgerundete
   Körper werden generell analytisch statt tesselliert.
2. **BTON-Ref-Tabelle knacken** — Voraussetzung für exakte Kantenmengen
   (Geschwister-Kanten) und für die volle Disambiguierung. Danach: Pattern /
   Hole / Sweep / Loft importieren, und `up_to_face` als echten,
   assoziativen Extrude-Parameter im Kernel.
3. **PROJECTED-Constraints assoziativ machen** — die letzte große
   Sketch-Lücke (29× in der Kabelbox). Braucht die Übersetzung von Onshapes
   Kanten-Query auf eine FieldDraft-Query; `sketch_project` existiert bereits.
4. **OAuth statt Operator-API-Key** (Konzept in server/onshape.go skizziert),
   damit jeder Nutzer seine eigenen Onshape-Dokumente importiert.
5. **Slot-Tool eigene Kurven-Entities geben** — dann trägt auch ein Slot
   stabile Wall-Subs (heute positioneller Fallback).

## Offene Punkte / Lücken

- Übersetzer-Lücken (nach Pipeline schließen): Fillet-Arcs im Sketch,
  Bemaßungs-Constraints (DISTANCE/ANGLE/RADIUS/DIAMETER), Splines.
- Extrude/Revolve-Feature-Import selbst (Stufe 3 des Plans) — hier nur
  Sketches; Ebenen-Queries sind der Proof of Concept fürs Query-Mapping.
- OAuth statt Operator-API-Key (Konzept in server/onshape.go skizziert).
