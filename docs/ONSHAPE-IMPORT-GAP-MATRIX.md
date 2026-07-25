# Onshape-Import — Gap-Matrix (Survey #409–#421)

Survey der 13 Onshape-Dokumente gegen den FieldDraft-Importer
(`web/src/onshape/onshapeImport.ts`). Datenquelle: Live-`.../features`-Endpoint
(alle 13 HTTP 200), Roh-JSON unter `/tmp/onshape_survey/<ticket>.json`.

## Heute unterstützte Onshape-Feature-Typen

Der Importer dispatcht in `importOnshapeFeatures` (onshapeImport.ts) nur auf:

| Onshape featureType | Zeile | FieldDraft-Feature |
| --- | --- | --- |
| `newSketch` | :344 | addSketch (translateOnshapeSketch) |
| `extrude`   | :385 | extrude (BLIND / THROUGH_ALL / UP_TO_SURFACE / UP_TO_FACE; NEW/ADD/REMOVE/INTERSECT; symmetric; 2. Richtung) |
| `loft`      | :440 | loft (nur bodyType SOLID) |
| `shell`     | :515 | shell |
| `fillet`    | :538 | fillet_edge (nur filletType EDGE, control RADIUS, CIRCULAR) |
| `chamfer`   | —    | chamfer_edge (nur EQUAL_OFFSETS bzw. OFFSET_ANGLE @ 45°) |
| `hole`      | —    | hole (simple/drilled, blind + through-all, Sketch-Punkt-Ziele) |

Alles andere fällt in den Sammel-Zweig (:594) → `errors: "Feature-Typ '<x>' wird noch nicht importiert"` (Feature übersprungen, Import läuft weiter).

## Vorkommen aller Feature-Typen (Summe über 13 Docs)

| Feature | Anz. | Status | FieldDraft-Kernel |
| --- | ---: | --- | --- |
| extrude | 238 | OK | vorhanden |
| newSketch | 225 | OK | vorhanden |
| assignVariable | 119 | FEHLT | Kernel HAT setVariable (api/variables.rs) — nur Wiring |
| fillet | 88 | OK | vorhanden |
| chamfer | 38 | OK (EQUAL_OFFSETS / 45°) | Kernel-Feature `chamfer_edge` seit v0.31.0 |
| booleanBodies | 27 | FEHLT | Kernel HAT boolean — nur Wiring |
| thicken | 25 | FEHLT | Kernel HAT thicken — nur Wiring |
| mateConnector | 22 | FEHLT | Kernel HAT mate_connector — nur Wiring (Referenzgeo). **Zensus zählt nur Top-Level**: implizite MCs als Sketch-Ebene stecken in `subFeatures` (#416: 3×) |
| revolve | 20 | OK (volle Umdrehung) | Übersetzer seit #416; Teil-Umdrehungen kann der Kernel nicht |
| mirror | 19 | FEHLT | Kernel HAT mirror — nur Wiring |
| circularPattern | 16 | FEHLT | Kernel HAT circular_pattern — nur Wiring |
| loft | 16 | OK (SOLID) | 6× SURFACE werden übersprungen |
| transform | 11 | FEHLT | kein Body-Transform-Feature |
| deleteBodies | 10 | FEHLT | kein Gegenstück (kann als No-op/Suppress laufen) |
| hole | 9 | OK (simple/drilled) | Übersetzer da; cbore/csink/tip/up-to-* melden Warnung (hole P2+) |
| shell | 6 | OK | vorhanden |
| printedHole | 6 | FEHLT | auf hole abbildbar |
| linearPattern | 4 | FEHLT | Kernel HAT linear_pattern — nur Wiring |
| cPlane | 4 | FEHLT | kein Datum-Plane-Feature (Sketches reiten auf Face-Queries) |
| laserJoint | 3 | FEHLT | Custom-Feature (Fingerzinken) — kein Gegenstück |
| sweep | 3 | FEHLT | Kernel HAT sweep — nur Wiring (Pfad-Query) |
| projectCurves | 3 | FEHLT | verwandt mit PROJECTED-Lücke |
| threadCreator | 2 | FEHLT | kosmetisches Gewinde — Warnung/Skip |
| importDerived | 2 | FEHLT | Cross-Document-Derived-Part — groß |
| helix | 2 | FEHLT | Kernel HAT helix — nur Wiring |
| plasticThreads | 1 | FEHLT | kosmetisch — Skip |
| enclose | 1 | FEHLT | kein Gegenstück — groß |

## Parameter-Detail (Untertyp-Lücken)

- extrude endBound: BLIND 197 (ok), UP_TO_SURFACE 23 (ok, gebacken+Warnung), THROUGH_ALL 12 (ok), **UP_TO_NEXT 6 (FEHLT** — kein endBoundEntityFace-Target, `endCondition` meldet „Zielfläche nicht auflösbar").
- extrude operation: NEW 92 / REMOVE 79 / ADD 62 / INTERSECT 5 — alle unterstützt.
- fillet: 88× EDGE + RADIUS + CIRCULAR — voll unterstützt.
- chamfer: der #414-Chamfer ist `OFFSET_ANGLE` @ 45° (nicht `EQUAL_OFFSETS`) — bei
  rechtwinklig zusammenstoßenden Flächen deckungsgleich mit der symmetrischen Fase,
  wird mit genau dieser Einschränkung als Warnung übernommen. Ungleiche Schenkel
  (`TWO_OFFSETS`, `OPPOSITE_OFFSETS`, Winkel ≠ 45°) werden ausgelassen, nicht genähert.
- hole style: C_SINK 4, SIMPLE 5, (unspez.) 6.
- revolve: `RevolveDef` hat KEINEN Winkel — es dreht immer volle 360°. `FULL`
  ist damit exakt; `ONE_SIDED`/`TWO_SIDED`/`SYMMETRIC` werden nur übernommen,
  wenn ihre Winkel sich zu 360° addieren (Onshape schreibt genau das für ein
  30°/330°-Paar), sonst mit Nennung von Typ und Winkel ausgelassen. Als Achse
  nimmt `revolve.axis` nur `qSketchLine` oder `qEdge`; unsere Kanten-Übersetzung
  liefert `qCommonEdge`, deshalb ist eine B-Rep-Kante als Achse heute nicht
  abbildbar (im Korpus ist die Achse praktisch immer eine Konstruktionslinie).
- loft bodyType: SOLID 10 (ok), **SURFACE 6 (übersprungen** — alle in #418).
- Ausdrücke: 6004 literal-parsebar, **153 Variablen/Formeln** (`#Brettstaerke`, `3 mm+#HoleExtraGap`, …) — `mmOf` kann nur Literale, Variablenbezüge fallen auf Defaults zurück → falsche Maße.

## Gap-Matrix (Zeile = Ticket, Zelle = Zustand)

`OK` = alle Feature-Typen unterstützt · Feature-Liste = fehlende Typen (Anzahl)

| Ticket | Feats | Zustand |
| --- | ---: | --- |
| #410 | 18 | **Struktur korrekt** (Shell-Container hohl + Deckel, SDF-verifiziert), aber Import blockiert den Main-Thread ~4–5 min; Fillet-Reste (CAP_EDGE/CAP_FACE, qCommonEdge löst nichts auf) — P0-Livetest 2026-07-24 |
| #412 | 2  | Feature-Typen OK, aber **Geometrie falsch**: Extrude referenziert 4 Sketch-Regionen (qCompressed → SKETCH_ENTITY), Importer extrudiert den ganzen Sketch → ein Block statt Platten — P0-Livetest 2026-07-24 |
| #413 | 6  | **Struktur korrekt** (SDF-verifiziert: Wanne, 2-mm-Wände); Rest: SWEPT_/CAP_EDGE-Fillets übersprungen, Geschwister-Kanten nicht unterscheidbar — P0-Livetest 2026-07-24 |
| #414 | 27 | Feature-Typen **OK** (chamfer + hole übersetzt), aber **Geometrie noch falsch**: Wall-Provenienz region-eingeschränkter Profile fehlt → Chamfer-Kante, Sketch-2-Ebene und 6 Fillets lösen nicht auf — s. Update 2026-07-25 (3) |
| #416 | 19 | Feature-Typen **OK** (chamfer + revolve übersetzt), aber **Geometrie falsch**: 3 der 5 Sketches liegen auf impliziten Mate-Connectors (Sub-Features!) → 12 der 19 Features fallen aus; dazu ein Kernel-Bug bei abwärts gezeichneter Revolve-Achse — s. Update 2026-07-25 (4) |
| #415 | 20 | assignVariable(1), thicken(1), booleanBodies(1) |
| #409 | 48 | assignVariable(8), hole(4), mirror(3), linearPattern(2), booleanBodies(1), chamfer(1) |
| #419 | 151 | chamfer(12), booleanBodies(8), revolve(7), thicken(7), assignVariable(6), hole(2), printedHole(2), circularPattern(1), linearPattern(1) |
| #420 | 23 | assignVariable(7), booleanBodies(1), helix(1), sweep(1) |
| #421 | 9  | helix(1), assignVariable(1), mateConnector(1), hole(1) |
| #411 | 28 | transform(5), revolve(3), threadCreator(1), plasticThreads(1), booleanBodies(1), mirror(1), circularPattern(1), importDerived(1) |
| #418 | 120 | assignVariable(9), booleanBodies(9), chamfer(8), fillet(ok), revolve(3), projectCurves(3), sweep(2), mirror(2), deleteBodies(2), thicken(2), cPlane(1), enclose(1), circularPattern(1), transform(1), loft-SURFACE(6) |
| #417 | 449 | assignVariable(87), mateConnector(21), thicken(15), chamfer(14), circularPattern(13), mirror(13), deleteBodies(8), revolve(6), booleanBodies(6), transform(5), printedHole(4), cPlane(3), laserJoint(3), threadCreator(1), importDerived(1), linearPattern(1), hole(1) |

## Feature-Lücken: was fehlt, Aufwand, welche Tickets

| Lücke | Aufwand | blockiert Tickets |
| --- | --- | --- |
| assignVariable (setVariable-Wiring + Var-fähiges `mmOf`) | M | 409,415,417,418,419,420,421 |
| booleanBodies (2 Body-Set-Queries → boolean) | M | 409,411,415,417,418,419,420 |
| ~~chamfer~~ ERLEDIGT (`chamfer_edge`, EQUAL_OFFSETS/45°) | — | ~~409,414,416,417,418,419~~ |
| ~~revolve~~ ERLEDIGT (Profil + Sketch-Linien-Achse, volle Umdrehung) | — | ~~411,416,417,418,419~~ |
| hole ERLEDIGT (P1-Umfang); **printedHole** noch offen | S | 417,419 (printedHole) |
| mirror (Spiegelebenen-Query + entities) | M | 409,411,417,418,419 |
| circularPattern (+ linearPattern) (Achse/Richtung + Anzahl + entities) | M | 409,411,417,418,419 |
| thicken (Face-Query + Dicke) | S–M | 415,417,418,419 |
| sweep (Profil + Pfad-Query) | M | 418,420 |
| helix | S–M | 420,421 |
| mateConnector als **Sketch-Ebene** (Ursprung + Achsenlage aus `originType`/`entityInferenceType`) | M | 416,417,421 |
| transform (Body verschieben/kopieren — kein Kernel-Feature) | L | 411,417,418 |
| deleteBodies (No-op/Suppress) | S | 417,418,419 |
| cPlane (Datum-Ebene als benennbare Referenz) | M | 417,418 |
| projectCurves | M–L | 418 |
| UP_TO_NEXT (extrude-End-Condition) | S | (verteilt: 6×) |
| loft SURFACE | L (Flächenkörper) | 418 |
| threadCreator / plasticThreads (kosmetisch → Warnung) | S | 411,417,421 |
| laserJoint (Fingerzinken-Custom-Feature) | L | 411,417 |
| importDerived (Cross-Document) | L | 411,417 |
| enclose | L | 418 |

## #412 Region-Query-Auflösung — Befund & Teillösung (2026-07-24)

Der Importer liest jetzt ALLE Profil-Queries eines Extrudes (nicht nur die erste), dekodiert jede qCompressed-SKETCH_ENTITY-Query zu den berandenden sketchEntityIds (`onshapeRegion.ts::decodeRegionQuery`), berechnet daraus einen Innenpunkt (Zentroid der referenzierten Segment-Geometrie, Imprint-abgeleitete `.top/.bottom`-Ids fallen auf ihr Basis-Rechteck zurück) und fragt den Kernel `regionQueryAtPoint` → `qRegion(sketch, i)`. Die distinkten Regionen werden zu einer `profiles`-MultiQuery verbunden.

**Fundamentaler Modell-Mismatch (Kernbefund):** Onshape identifiziert Regionen per **Imprint** — überlappende/verschachtelte Sketch-Kurven zerteilen die Ebene in viele kleine Faces, jede einzeln wählbar. FieldDrafts Regionenmodell ist ein **SDF-Vereinigungs-Flood-Fill** (`sketch_regions.rs::find_regions`): verschachtelte/überlappende geschlossene Profile verschmelzen zu EINER Region. Für #412 (4 Platten als Imprint-Subfaces eines großen Rechtecks) findet der Kernel daher nur **1 Region** — die 4 Onshape-Regionen haben keine FieldDraft-Entsprechung.

Folge/Verhalten: Wo die Auswahl 1:1 auflösbar ist (getrennte, nicht verschachtelte Regionen — der Normalfall), extrudiert der Import jetzt korrekt nur die gewählten Regionen. Wo sie es nicht ist (Imprint wie #412), gibt es eine **sichtbare Warnung** („Onshape wählte N Regionen, FieldDraft findet dafür nur M") und Fallback auf den ganzen Sketch — nie mehr stille Falschgeometrie. Für #412 bleibt die Geometrie damit noch falsch (1 Block statt 4 Platten), aber laut und dokumentiert.

**Offene Kernel-Arbeit für echte #412-Korrektheit:** Imprint-artige Regionen-Subdivision im Sketch-Regionenmodell (Faces an internen Sehnen/Kurven trennen, nicht nur SDF-Union-Flood-Fill). Größerer Kernel-Change, hier bewusst nicht mitgemacht.

### Update 2026-07-25 — Imprint-Arrangement gelandet, Resolver-Nachschlag offen

Die Kernel-Subdivision ist da (v0.30.0, 68073b43): `sketch_arrangement.rs` baut ein echtes planares Arrangement über den rohen sub_calls (das Lowering verwirft offene Trennlinien, darum nicht über `Sketch2d`) und ist die einzige Wahrheit für alle Region-Query-Pfade; `sketch_regions.rs` ist nur noch Adapter. T-Junctions zerteilen, Dangling-Spitzen nicht; un-geschnittene Kurven werden als exakte Segmente re-emittiert. Der Web-Resolver sondiert beidseitig neben jeder von der Query benannten Kante (`regionFaceIndex`) statt einen Zentroid zu raten.

**ABER — Live-Ground-Truth-Vergleich gegen die Onshape-API deckte einen Resolver-Fehlgriff auf:** Für Garagenregal_rechts liefert die Auflösung `qRegions(1,[1,2,3,4])` ohne Warnung; korrekt wäre `qRegions(1,[2,3,4])` MIT Reduktions-Warnung (Onshape hat 3 Parts; sein eigener Imprint zerteilt Part JHD in 2 Regionen, die in FieldDrafts Face 4 zusammenfallen). Die „flächenkleinste angrenzende Face"-Heuristik wählte für eine Query Face 1 — einen 604×1500-Block, der komplett außerhalb der Onshape-Geometrie liegt. Fix (Adjazenz-Scoring/Beranden-Validierung) in Arbeit.

**Methodik-Lehre:** Erwartungswerte für Import-Tests NIE aus der eigenen Auflösung ableiten — Ground Truth kommt von der Onshape-API: `GET /api/v6/parts/d/…/e/…` (Part-Liste) + `…/partid/<id>/boundingboxes` (Referenz-AABBs, Meter) + `partstudios/…/shadedviews` (Referenzbild), Basic-Auth mit den Keys aus `.env`.

### Update 2026-07-25 (2) — korrekte Geometrie, leerer Viewport: der Mesher sah die Bretter nicht

Nach dem Resolver-Fix (v0.31.1) war die Geometrie richtig, das Projekt rendert im WebGL-Modus trotzdem **nichts**: `meshedBodies` kam mit `positions.length === 0` aus dem Mesher-Worker — ohne Fehler, ohne Warnung. Ursache ist NICHT der Import, sondern der Viewport-Mesher: das DC-Gitter leitet seine Zellgröße allein aus der **längsten** Körperachse ab (`dc.rs`, `cell = max_dim / resolution`). Garagenregal_rechts ist 4 m lang und besteht aus **22-mm-Brettern**; bei der App-Auflösung 64 sind die Zellen 62 mm groß, keine Gitterebene landet in einem Brett, kein Vorzeichenwechsel — leeres Mesh. Bei Auflösung 48 fielen zufällig ein paar Ebenen hinein (596 Dreiecke Schrott), bei 32/64 keine: das „mal geht's, mal nicht" war reine Gitter-Resonanz.

Fix an der Wurzel: `dc.rs::resolution_for_thin_features` (nur Viewport-Pfad, `build_mesh_for`) verfeinert das Gitter, bis zwei Samples über das dünnste Feature des Körpers passen (Leaf-AABBs, Sketch-Profil-AABBs inkl. Schneidwerkzeugen, Shell-Wandstärke) — gedeckelt durch ein Sampling-Budget (Gitterecken × statische Feldkosten-Schätzung) und nur, wenn die bezahlbare Zelle ein Feature wirklich in Reichweite bringt. Regressionstest spielt den echten Worker-Pfad nach (`onshapeImportE2E.slow.test.ts`, „renders through the app's mesher-worker path").

**Lehre für die Testsuite:** Struktur-, SDF- und AABB-Prüfungen beweisen NICHT, dass der Nutzer etwas sieht. Import-Akzeptanztests brauchen mindestens eine Zusicherung über das **Render-Mesh bei App-Auflösung**.

Perf-Nebenschauplatz erledigt: #410-Freeze gelöst (v0.29.1 per-Body-BRep-Cache + v0.30.0 Batch-Meshing; Browser 4–5 min → ~22 s). Folge-Ticket #423: Octree-Weave-Fallback bei gestapelten koplanaren Extrudes desselben Profils (~6 s, Kabelbox-Deckel).

## Empfohlene Paket-Reihenfolge

1. **P0 Quick Wins verifizieren:** ERLEDIGT 2026-07-24 — Ergebnis ernüchternd: nur #413 strukturell korrekt. Neue P0-Folgearbeiten: (a) **Sketch-Region-Query-Auflösung** (qCompressed → sketchEntityIds → Region), ohne die #412 falsch bleibt und vermutlich viele weitere Dokumente stille Falschgeometrie liefern; (b) **#410-Import-Freeze** profilen/fixen; (c) App-Bug #422 ERLEDIGT (v0.28.1, c4f0c619 — view-lose Projekte starten als Iso-Fit auf frische CSG-Bounds, Alt-Pose wird vergessen).
2. **P1 Chamfer + Hole:** ERLEDIGT 2026-07-25 (Übersetzer + Web-Verdrahtung). Schließt #414
   **nicht** komplett — Restblocker ist die Wall-Provenienz region-eingeschränkter Profile
   (s. u.), nicht mehr die Feature-Typen. Entblockt 409/416/417/418/419.
3. **P2 Revolve:** ERLEDIGT 2026-07-25 (Übersetzer + E2E). Schließt **#416**
   NICHT — dort sind zwei andere Blocker aufgetaucht (Mate-Connector-Ebenen,
   Kernel-Achsenrichtung, s. u.). Entblockt 411/417/418/419.
4. **P3 Fundament Boolean + Variablen:** booleanBodies-Wiring + assignVariable→setVariable + var-fähiges `mmOf`. Prerequisite für fast alles; mit P1/P2 nun in Reichweite: 415,419,420.
5. **P4 Thicken:** schließt mit P3 **#415**.
6. **P5 Patterns + Mirror** (circular/linear/mirror): schließt mit P1–P4 **#409** und **#419**.
7. **P6 Sweep + Helix + mateConnector + deleteBodies(No-op) + UP_TO_NEXT:** schließt **#420** und **#421**.
8. **P7 Long-Tail 417/418/411:** transform, cPlane, projectCurves, laserJoint, importDerived, enclose, SURFACE-Loft, threadCreator. Die drei Mega-/Spezialdokumente; zuletzt, weil teuer und wenig Streuwirkung.

Kern-Einsicht: 11 der 26 fehlenden Typen sind **reine Übersetzer-Verdrahtung** — der Kernel kann revolve/sweep/helix/hole/thicken/boolean/circular_pattern/linear_pattern/mirror/mate_connector/Variablen bereits. Echte Kernel-Neuarbeit brauchen nur chamfer (Edge-Feature), transform, cPlane, projectCurves, SURFACE-loft, laserJoint, importDerived, enclose.

## #414 Chamfer + Hole — Übersetzer geliefert, Restblocker benannt (2026-07-25)

**Neu übersetzt.** `chamfer` → je aufgelöster Kante ein `chamfer_edge`
(Kernel-Feature seit v0.31.0). Übernommen werden `EQUAL_OFFSETS` (Maß = `width`)
und `OFFSET_ANGLE` bei exakt 45° (Maß = `width1`) — letzteres mit der ehrlichen
Warnung, dass es nur an rechtwinklig zusammenstoßenden Flächen deckungsgleich ist.
`TWO_OFFSETS`, `OPPOSITE_OFFSETS` und jeder andere Winkel werden **ausgelassen**
statt symmetrisch genähert. `tangentPropagation` wird gemeldet, nicht nachgezogen.

`hole` → ein `hole`-Feature je Onshape-Feature, mit N Sketch-Punkt-Zielen
(`qSketchPoint(sketch, entity)` aus der `VERTEX`/`SKETCH_ENTITY`-Query), dem
Durchmesser **wörtlich aus dem Dokument** (kein Umweg über FieldDrafts ISO-Tabelle),
`blind`/`through_all` und explizitem Body-Scope aus `mergeScope`. Alles darüber
hinaus meldet den konkret fehlenden Aspekt: Counterbore/Countersink samt Maßen,
Bohrspitze, Start-Referenz, Endversatz; `up_to_next`/`up_to_entity` werden als
„durch alles" genähert und genau so benannt. Gewinde bleibt kosmetisch — gebohrt
wird das Kernloch, exakt wie Onshape es auch tut.

**Nebenbei geschlossen:** Kanten-Queries der Form `CAP_EDGE` (Onshape benennt den
Kantenzug direkt statt über seine zwei Flächen) werden jetzt übersetzt — dieselbe
Cap-Rim-Form wie `CAP_FACE`. Das ist der Fillet-Rest aus #410/#413 und betrifft
Fillet und Chamfer gemeinsam. Die Cap-Seite folgt außerdem endlich dem
Extrude-Vorzeichen (negative Tiefe ⇒ `CAP_TOP` liegt in der Sketch-Ebene) — im
#414-Dokument extrudiert das Basis-Feature mit −50 mm.

**Restblocker für #414 (KEIN Chamfer-/Hole-Thema).** Die Ground-Truth-AABB wird in
X und Y noch verfehlt. Ursache ist die Folgearbeit aus
`docs/SKETCH-REGION-IMPRINT-PLAN.md` §5: **region-eingeschränkte Profile tragen
keine Entity-Provenienz**, ihre Wall-Subs fallen auf positionale Nummerierung
zurück. Im #414-Dokument ist `Extrude 1` region-eingeschränkt, und genau daran
hängen die Chamfer-Kante (`qWall(2, 8)`), die Ebene von `Sketch 2` (`qWall(2, 17)`
— und damit der Zielpunkt des Lochs) sowie sechs Fillets: alle lösen auf null
Flächen auf. Gegenprobe mit Gesamt-Sketch-Profilen (Region-API weggelassen): der
Chamfer landet, regeneriert und trägt Material ab — der Übersetzer ist es also
nicht. Zweiter, kleinerer Rest: `Extrude 2` und `Extrude 4` finden ihre Region gar
nicht und fallen auf den ganzen Sketch zurück (sichtbare Warnung), was die
Übermaße in X/Y erklärt.

**Testabdeckung.** `onshapeImport.test.ts` prüft den Übersetzungs-Vertrag gegen die
echte Fixture (`fixtures/onshape-414-features.json`, 27 Features aus dem
`/features`-Endpoint). `onshapeImport414E2E.slow.test.ts` fährt das echte WASM:
Ground Truth aus `GET /parts` + `/boundingboxes` (1 Part „JHD", AABB
x[−65.05, 65.0] · y[−70.13, 0] · z[−50, 50] mm) — Körperzahl und Z-Ausdehnung
stimmen, die volle AABB ist als `it.fails` festgenagelt, damit sie beim Schließen
der Provenienz-Lücke laut wird. Dazu ein Render-Mesh-Check über den echten
Mesher-Worker-Pfad bei App-Auflösung, ein Gitter-Differenz-Beweis, dass die Fase
nur Material **abträgt** und keines hinzufügt, und die Bohrungs-Parameter durch die
echte API-Kette (`add_feature_call` → `body_sdf_eval`, Ø6 blind 50 mm, Boden an
der richtigen Stelle).

## #416 Revolve — Übersetzer geliefert, zwei Restblocker (2026-07-25)

**Neu übersetzt.** `revolve` → ein FieldDraft-`revolve`. Profil-Auswahl läuft
durch dieselbe Region-Auflösung wie beim Extrude, Boolean-Op und `booleanScope`
durch dieselben Helfer wie bei jedem körperbildenden Feature (die drei
Kopien davon sind zu `operationOf` / `booleanTargets` zusammengezogen). Neu ist
allein die **Achse**: Onshape benennt sie mit einer Query auf eine Kante, in der
Praxis auf eine (meist Konstruktions-)Sketch-Linie — FieldDraft adressiert die
als `qSketchLine(sketch, entity)`, und der Kernel hebt deren 2D-Endpunkte durch
den Sketch-Frame in die Welt. Nennt die Query etwas anderes (B-Rep-Kante,
Datum-Achse, ein Bogen), wird das Feature mit genannter Ursache **ausgelassen**
statt um eine geratene Achse gedreht. Teil-Umdrehungen ebenso: `RevolveDef` hat
keinen Winkel, deshalb kommt nur durch, was sich zu 360° addiert.

Nebenbei bekommt eine unübersetzbare **Mate-Connector-Ebene** jetzt eine Meldung,
die das fehlende Onshape-Konzept benennt, statt „not a qCompressed query“.

**Restblocker 1 — Sketches auf impliziten Mate-Connectors (KEIN Revolve-Thema).**
Sketch 2, Sketch 4 und Sketch 6 des #416-Dokuments liegen nicht auf einer Ebene
oder Fläche, sondern auf einem **Mate-Connector, den der Sketch selbst als
Sub-Feature mitbringt** (`subFeatures[0]`, `originType: ON_ENTITY` +
`entityInferenceType: CENTER/MID_POINT/POINT`). Der Feature-Typ-Zensus oben zählt
nur Top-Level-Features und hat diese 3 daher nie gesehen — die Zeile „#416:
chamfer(2), revolve(1)“ war insofern unvollständig. Folge: von 19 Features landen
nur 4 (Sketch 1 + 3 Extrudes), die Ground-Truth-AABB wird weit verfehlt
(FieldDraft x[−4, 32] · y[−26,7, 11] · z[−65, 39,5] mm gegen Onshape
x[−70,05, 3,07] · y[−26,77, 7,13] · z[−73,08, 35,0] mm). Das ist die Lücke
`mateConnector`, hier in ihrer teuersten Form: nicht als Referenzgeometrie,
sondern als **Sketch-Ebene** samt Ursprung und Achsenlage.

**Restblocker 2 — Kernel-Bug: Revolve-Achse abwärts gezeichnet.**
`features/revolve.rs` legt die Achse auf die lokale Linie `x = 0` und prüft dann,
ob das Profil dabei auf der NEGATIVEN Seite gelandet ist (die Revolve-SDF faltet
jede Abfrage auf einen Radius ≥ 0 und würde es dort nie erreichen). Diese Prüfung
ist ein exaktes `BoundedSketch2d::from_sketch(&aligned).aabb().max[0] <= 0.0`.
Das #416-Dokument zeichnet seine Konstruktionsachse **abwärts**, die Ausrichtung
dreht das Profil also um 180° nach −x — wo sein Maximum nicht 0 ist, sondern
**+2,2e−7 mm**: der numerische Überstand des Bogen-Endpunkts, der genau auf der
Achse sitzt. Um dieses Haar verfehlt der exakte Test die Rückdrehung, das Profil
bleibt auf −x, und heraus kommt ein Körper mit korrekter AABB, aber **leerem
Feld** — `bodySdfEval` ist überall positiv, der Mesher liefert null Dreiecke.
Gegenprobe im Slow-Test: dieselbe Strecke, nur andersherum gezeichnet
(`startParam`/`endParam` getauscht), ergibt die korrekte Kugel und rendert. Vom
Importer aus ist das nicht zu umgehen — eine `qSketchLine`-Query trägt keine
Richtung. Fix gehört in den Kernel: die Seitenwahl an der Profil-**Fläche** (oder
einer relativen Toleranz) festmachen statt an einem exakten Vorzeichentest.

**Testabdeckung.** `onshapeImport.test.ts` prüft den Übersetzungs-Vertrag gegen
die echte Fixture (`fixtures/onshape-416-features.json`, 19 Features aus dem
`/features`-Endpoint): Achse als `qSketchLine`, ADD mit explizitem Ziel, Skip bei
Teil-Umdrehung und bei SURFACE, und die Mate-Connector-Meldung.
`onshapeImport416E2E.slow.test.ts` fährt das echte WASM gegen die
API-Ground-Truth (1 Part „Part 1“, AABB s. o.): die Mate-Connector-Meldungen sind
festgenagelt, Part-Zahl + AABB als `it.fails`. Der Revolve selbst läuft dort auf
einem auf die Right-Ebene **umgepflanzten** Sketch 4 (sonst verschluckt ihn
Restblocker 1) — mit der Zusicherung, dass aus dem Halbkreis R = 4,94 mm eine
Kugel wird, samt Render-Mesh über den echten Mesher-Worker-Pfad bei
App-Auflösung; die abwärts gezeichnete Original-Achse ist als `it.fails` gepinnt.

**Chamfer-Stand im Dokument:** beide Fasen sind `OFFSET_ANGLE` @ 45° mit
`width1 = 0,5 cm` → je 5 mm, also von v0.32.0 abgedeckt. Sie kommen im
Volldokument trotzdem nicht an: ihre Kanten-Queries zeigen auf Wände von
Extrudes, deren Profil-Sketch an Restblocker 1 hängt.
