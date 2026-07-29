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
| `assignVariable` | — | Dokument-Variable (`setVariable`), Ausdruck ausgewertet |
| `booleanBodies` | — | boolean (Union/Subtract/Intersect, expliziter Scope) |
| `thicken` | — | thicken (Flächen als `qFace`, beidseitig, Richtungspfeil, Scope) |
| `mateConnector` | — | mate_connector (Referenzgeometrie; Top-Level **und** implizit als Sketch-Sub-Feature) |

Alles andere fällt in den Sammel-Zweig (:594) → `errors: "Feature-Typ '<x>' wird noch nicht importiert"` (Feature übersprungen, Import läuft weiter).

## Vorkommen aller Feature-Typen (Summe über 13 Docs)

| Feature | Anz. | Status | FieldDraft-Kernel |
| --- | ---: | --- | --- |
| extrude | 238 | OK | vorhanden |
| newSketch | 225 | OK | vorhanden |
| assignVariable | 119 | OK (ASSIGNED) | Übersetzer seit P3; `MEASURED` (11× in #417) fehlt |
| fillet | 88 | OK | vorhanden |
| chamfer | 38 | OK (EQUAL_OFFSETS / 45°) | Kernel-Feature `chamfer_edge` seit v0.31.0 |
| booleanBodies | 27 | OK | Übersetzer seit P3 |
| thicken | 25 | OK | Übersetzer seit P4 |
| mateConnector | 22 | OK, auch als Ebene | Übersetzer seit 2026-07-26, Top-Level **und** implizit; seit 2026-07-27 auch als SKETCH-EBENE und mit Ankern auf roher Sketch-Linie sowie auf dem Part-Studio-Ursprung. Offen: Inferenz `CENTROID`. **Zensus zählt nur Top-Level**: implizite MCs als Sketch-Ebene stecken in `subFeatures` (#416: 3×) |
| revolve | 20 | OK (volle Umdrehung) | Übersetzer seit #416; Teil-Umdrehungen kann der Kernel nicht |
| mirror | 19 | OK (PART) | Übersetzer seit P5; Kernel-`mirror` spiegelt seit P5 wirklich (vorher nur der Feature-Ursprung). Mate-Connector als Spiegelebene seit 2026-07-27 |
| circularPattern | 16 | OK (PART) | Übersetzer seit P5; `equalSpace` über einen TEIL-Winkel wird verweigert statt geraten |
| loft | 16 | OK (SOLID) | 6× SURFACE werden übersprungen |
| transform | 11 | FEHLT | kein Body-Transform-Feature |
| deleteBodies | 10 | FEHLT | kein Gegenstück (kann als No-op/Suppress laufen) |
| hole | 9 | OK (simple/drilled) | Übersetzer da; cbore/csink/tip/up-to-* melden Warnung (hole P2+) |
| shell | 6 | OK | vorhanden |
| printedHole | 6 | FEHLT | auf hole abbildbar |
| linearPattern | 4 | OK (PART, 1 Richtung) | Übersetzer seit P5; ein 2D-Raster (zweite Richtung) ist ein Muster eines Musters und wird benannt gemeldet |
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
- Ausdrücke: 6004 literal-parsebar, 153 Variablen/Formeln (`#Brettstaerke`, `3 mm+#HoleExtraGap`, …) — **seit P3 ausgewertet** (`onshapeExpr.ts`), s. Update 2026-07-26.

## Gap-Matrix (Zeile = Ticket, Zelle = Zustand)

`OK` = alle Feature-Typen unterstützt · Feature-Liste = fehlende Typen (Anzahl).
Zeilen ohne datierten Kommentar stehen noch auf dem **Stand des Erst-Surveys**
(2026-07-24); welche der dort genannten Typen inzwischen übersetzt sind, sagt
„Vorkommen aller Feature-Typen" oben und „Feature-Lücken" unten.

| Ticket | Feats | Zustand |
| --- | ---: | --- |
| #410 | 18 | **Struktur korrekt** (Shell-Container hohl + Deckel, SDF-verifiziert), aber Import blockiert den Main-Thread ~4–5 min; Fillet-Reste (CAP_EDGE/CAP_FACE, qCommonEdge löst nichts auf) — P0-Livetest 2026-07-24 |
| #412 | 2  | Feature-Typen OK, aber **Geometrie falsch**: Extrude referenziert 4 Sketch-Regionen (qCompressed → SKETCH_ENTITY), Importer extrudiert den ganzen Sketch → ein Block statt Platten — P0-Livetest 2026-07-24 |
| #413 | 6  | **Struktur korrekt** (SDF-verifiziert: Wanne, 2-mm-Wände); Rest: SWEPT_/CAP_EDGE-Fillets übersprungen, Geschwister-Kanten nicht unterscheidbar — P0-Livetest 2026-07-24 |
| #414 | 27 | Feature-Typen **OK** (chamfer + hole übersetzt), aber **Geometrie noch falsch**: Wall-Provenienz region-eingeschränkter Profile fehlt → Chamfer-Kante, Sketch-2-Ebene und 6 Fillets lösen nicht auf — s. Update 2026-07-25 (3) |
| #416 | 19 | Feature-Typen **OK** (chamfer + revolve übersetzt), aber **Geometrie falsch**: 3 der 5 Sketches liegen auf impliziten Mate-Connectors (Sub-Features!) → 12 der 19 Features fallen aus; dazu ein Kernel-Bug bei abwärts gezeichneter Revolve-Achse — s. Update 2026-07-25 (4) |
| #415 | 20 | **Geometrie stimmt**: die Ground-Truth-AABB wird getroffen (Render-Mesh x[−33,0; 33,0] · y[−63,54; 63,54] · z[−3,03; 32,55] gegen Onshape x[−33; 33] · y[−63,55; 63,55] · z[−3; 32,5]). Rest: 7 Fillets lösen ihre Kanten nicht auf (kosmetisch) — s. Update 2026-07-26 (4) |
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
| ~~assignVariable~~ ERLEDIGT (setVariable-Wiring + var-fähiges `mmOf`) | — | ~~409,415,417,418,419,420,421~~ (Rest: `MEASURED`) |
| ~~booleanBodies~~ ERLEDIGT (2 Body-Set-Queries → boolean) | — | ~~409,411,415,417,418,419,420~~ |
| ~~chamfer~~ ERLEDIGT (`chamfer_edge`, EQUAL_OFFSETS/45°) | — | ~~409,414,416,417,418,419~~ |
| ~~revolve~~ ERLEDIGT (Profil + Sketch-Linien-Achse, volle Umdrehung) | — | ~~411,416,417,418,419~~ |
| hole ERLEDIGT (P1-Umfang); **printedHole** noch offen | S | 417,419 (printedHole) |
| ~~mirror~~ ERLEDIGT (Ebenen-Query + entities; **Kernel-Reflexion echt gemacht**) | — | ~~409,411,417,418,419~~ (Rest: Mate-Connector als Ebene) |
| ~~circularPattern (+ linearPattern)~~ ERLEDIGT (Achse aus Sketch-Linie/Ursprungsachse, Anzahl, entities) | — | ~~409,411,417,418,419~~ |
| ~~thicken~~ ERLEDIGT (Face-Query → `qFace`, Dicke 1/2, Richtung, Scope) | — | ~~415,417,418,419~~ |
| ~~Sketch-Frame auf Face-Ebenen~~ ERLEDIGT (Frame an Onshapes 3D-Antwort gemessen, 2D-Koordinaten mit `B_fd⁻¹ ∘ B_os` umgerechnet) | — | ~~415~~, ~~414 (Teil)~~, jedes Dokument mit Sketch auf Wandfläche |
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

> **Nachtrag 2026-07-25 / 2026-07-29:** Der Flood-Fill ist seit dem Update darunter durch ein echtes planares Arrangement (`sketch_arrangement.rs`) ersetzt — dieser Absatz beschreibt den Zustand VOR #412s Nachschlag. Der letzte Fall, der noch einmal als fehlende Subdivision gelesen wurde (#443 → #444), war keiner: siehe Update 2026-07-29 (3).

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
4. **P3 Fundament Boolean + Variablen:** ERLEDIGT 2026-07-26 (s. u.). Schließt **kein** Ticket allein — #415 braucht noch `thicken` (P4).
5. **P4 Thicken:** ERLEDIGT 2026-07-26 (s. u.). Schließt **#415 NICHT** — dort ist
   ein neuer, allgemeinerer Blocker aufgetaucht (Sketch-Frame auf Wandflächen).
   Entblockt 417/418/419.
6. **P5 Patterns + Mirror:** ERLEDIGT 2026-07-27 (s. u.). Der Übersetzer war
   nur die halbe Arbeit — `mirror` brauchte zuerst einen **echten** Kernel-Mirror
   (die alte Implementierung spiegelte nur den Feature-Ursprung). Entblockt
   409/411/417/418/419.
7. **P6 Sweep + Helix + mateConnector + deleteBodies(No-op) + UP_TO_NEXT:** schließt **#420** und **#421**.
8. **P7 Long-Tail 417/418/411:** transform, cPlane, projectCurves, laserJoint, importDerived, enclose, SURFACE-Loft, threadCreator. Die drei Mega-/Spezialdokumente; zuletzt, weil teuer und wenig Streuwirkung.

Kern-Einsicht: 11 der 26 ursprünglich fehlenden Typen sind **reine Übersetzer-Verdrahtung** — der Kernel kann revolve/sweep/helix/hole/thicken/boolean/circular_pattern/linear_pattern/mirror/mate_connector/Variablen bereits. Echte Kernel-Neuarbeit brauchen nur chamfer (Edge-Feature), transform, cPlane, projectCurves, SURFACE-loft, laserJoint, importDerived, enclose.

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

## P3 Variablen + Boolean — geliefert (2026-07-26)

**Neu übersetzt: `assignVariable`.** Onshape deklariert Variablen als Features,
also in Dokumentreihenfolge und mit Rückgriff auf frühere
(`#Gesamtlaenge = #Klingenlaenge + #Grifflaenge`). Der Walk baut genau diese
Tabelle auf und legt jede Variable zusätzlich als **FieldDraft-Dokument-Variable**
an (`setVariable`: Länge in mm, Winkel in Grad, Zahl roh). Nicht übersetzbar sind
`mode: MEASURED` (Onshape schreibt dort nur `"?"`, der Wert entsteht erst in
seiner eigenen Auswertung — 9 Variablen in #417) und Dimensionen ohne
FieldDraft-Variablenart (Fläche o. ä.). Beides wird benannt; die gemessene
Variable bleibt undefiniert, die dimensionsfremde wandert immerhin in die
Ausdruckstabelle des Imports.

Hat die Onshape-Formel eine **treue Projektion** auf FieldDrafts
(dimensionslose, mm-basierte) Formelsprache, wird sie zusätzlich als `formula`
mitgegeben — aber nur, wenn FieldDrafts eigener Evaluator (`variableInput.ts`)
daraus dieselbe Zahl errechnet. Damit bleibt die Abhängigkeit im
Variablen-Panel editierbar, ohne je driften zu können. Winkel- und
Trigonometrie-Ausdrücke sind bewusst ausgenommen (FieldDrafts Winkelbasis ist
Grad, seine `sin()` rechnet im Bogenmaß).

**Neu: variablen-fähige Maße (`web/src/onshape/onshapeExpr.ts`).** Das war die
stillste Gefahr der Suite: `mmOf` konnte nur Literale, alles andere fiel
**lautlos** auf einen fest verdrahteten Vorgabewert zurück — die Geometrie sah
fehlerfrei aus und war falsch. Der neue Evaluator ist **einheiten-bewusst**
(jeder Wert trägt Längen- und Winkel-Exponenten; Basis mm/Grad), kennt
Grundrechenarten, Klammern, unäres Minus, Potenz, `#Variablen`, alle Längen- und
Winkel-Einheiten sowie `sin cos tan asin acos atan atan2 sqrt abs sign floor
ceil ln log exp round pow min max` und `pi`. **Juxtaposition ist Multiplikation**
und bindet enger als `*`/`/` — nur so sind `2mm`, `(…) cm`, `1/cm` und
`4 mm / 2 mm` (ein Verhältnis, keine Fläche) gleichzeitig richtig. Kein `eval`,
ein handgeschriebener Rekursiv-Absteiger.

Gemessen am echten Korpus (13 Dokumente, `/features`): **128 der 140**
Variablen-Ausdrücke werten korrekt aus, inklusive der beiden härtesten aus #417 —
`(round((#BoxMirrorGap+#MirrorDiameter)/2 * sin(1.5 deg) * 1/cm, .1)) cm` → 4 mm
und `(2 * sqrt(#TelescopeWeight) / sqrt(3*pi)) cm` → 18,4264 mm. Die restlichen
12 hängen **ausnahmslos** an den 9 `MEASURED`-Variablen von #417.

**Warnungs-Design (nie mehr still).** Scheitert ein Ausdruck, nennt die Meldung
den Ausdruck, den Parameter, die Ursache (`unbekannte Variable "#X"`,
`Einheiten passen nicht zusammen`, `gemessener Wert`) und die eingesetzte Zahl:
Onshapes eigenen ausgewerteten Wert, falls vorhanden — und sonst die Vorgabe,
mit dem ausdrücklichen Zusatz, dass das Maß damit sehr wahrscheinlich FALSCH
ist. **Am Korpus gegengeprüft: für keinen einzigen dieser 140 Ausdrücke liefert
Onshape einen ausgewerteten SI-Wert** (`value` ist immer 0) — der stille
Rückfall traf also stets die hart verdrahtete Vorgabe. Dieselbe Meldung erscheint
pro Import nur einmal.

**Neu übersetzt: `booleanBodies`.** `operationType` bildet 1:1 auf FieldDrafts
drei Boolean-Tabs ab (UNION → Union, SUBTRACTION → Subtract, INTERSECTION →
Intersect). Beide Operandenlisten sind Body-Queries; jede wird auf die
**erzeugende Onshape-Operation** dekodiert und über die idMap in
`qCreatedBy(<feature>)` übersetzt — die einzige Form, die das Kernel-Boolean
annimmt. Ein nicht auflösbarer Operand führt zum **Überspringen mit Fehler**
statt zum Raten; eine Subtraktion/Verschneidung ohne Ziel ebenso
(Feature-Scope-Direktive: nie implizit mit einem zufällig überlappenden Körper
verrechnen). `keepTools` wird übernommen, Werkzeug-Aufmaß (`offset`) und
`reFillet` werden als fehlend gemeldet. Der idMap-Eintrag des Booleans zeigt auf
das Feature, das den **Ergebniskörper besitzt** (erstes Ziel bei Subtract, sonst
erstes Werkzeug) — so landet eine spätere Onshape-Query auf „den Körper aus dem
Boolean" auf dem Feature, das diese Geometrie in FieldDraft wirklich trägt.

**Testabdeckung.** `onshapeExpr.test.ts` (25 Tests) deckt Einheiten, Präzedenz,
Klammern, Variablenketten, Dimensionsfehler, Funktionen und die
Formel-Projektion ab. `onshapeImport.test.ts` prüft den Übersetzungs-Vertrag
gegen die echten Fixtures (neu: `onshape-415-features.json`, 20 Features) —
Variablen in Dokumentreihenfolge, MEASURED-Meldung, Namens-Sanierung,
Boolean-Fälle samt beider Skip-Pfade, und dass ein kaputter Ausdruck laut wird
statt still. `onshapeImport415E2E.slow.test.ts` fährt das echte WASM: die
Variablen landen samt Formel im Kernel, das `boolean`-Feature regeneriert
fehlerfrei; die Ground-Truth-AABB (1 Part „Part 1", x[−33, 33] · y[−63,55,
63,55] · z[−3, 32,81] mm) ist als `it.fails` gepinnt, weil dem Dokument noch
`thicken` (P4) fehlt.

**Offene Reste aus diesem Paket.** (a) `MEASURED`-Variablen — bräuchten eine
Messung an der importierten Geometrie (Kernel-Arbeit; 9 Variablen, 12 Ausdrücke
in #417). (b) Feature-Parameter werden **numerisch gebacken**: die
Dokument-Variable ist da und editierbar, aber ein Extrude trägt seine Tiefe als
Zahl, nicht als `=Brettstaerke`. Der parametrische Durchgriff wäre der nächste
Schritt (Einheiten-Projektion pro Parameter, dieselbe Kreuzprobe wie bei den
Variablen). (c) Jeder `setVariable`-Aufruf regeneriert das Dokument einmal
komplett — bei den 87 Variablen von #417 nur deshalb billig, weil sie am Anfang
der Historie stehen.

## Update 2026-07-26 (2) — P4 Thicken geliefert, #415-Restblocker ist der Sketch-Frame

**Neu übersetzt: `thicken`.** Onshapes Thicken dickt die Flächen auf, die es
bekommt — im ganzen Korpus (25 Vorkommen) sind das Flächen eines
Volumenkörpers, und genau dafür wurde FieldDrafts `thicken` gebaut:
`thickness1` wächst entlang der Flächennormalen nach außen, `thickness2` nach
innen, der Richtungspfeil tauscht beide. Eine planare Fläche reduziert auf
denselben prismatischen Riegel, den ein Face-Extrude baut (#402), eine
gekrümmte analytische auf konzentrische Offset-Flächen. Die Abbildung ist
damit eine **Identität, keine Näherung** — und deshalb `thicken` und nicht
Extrude: ein Extrude bräuchte ein einzelnes planares Profil und könnte eine
gekrümmte Wand gar nicht tragen.

Der einzige Übersetzungsschritt mit Zähnen ist die **Flächen-Referenz**.
Onshape benennt jede Fläche mit derselben Creation-Query wie eine Sketch-Ebene
(hier: `SWEPT_FACE` → `qWall(op, entity)`), das Kernel-Feature nimmt aber
ausschließlich die ID-Hash-Form `qFace(<hash> @ <fingerprint>)`. Jede
parametrische Query wird deshalb gegen den Mid-History-B-Rep aufgelöst und als
kanonische Element-Referenz gespeichert (die Fingerprint-Heilung der
Namensgebung trägt sie über Regenerationen). Eine Query, die auf **null oder
mehrere** Flächen bindet, wird mit genannter Anzahl **verworfen**, nicht
geraten: eine Wand aufzudicken, die Onshape nie gewählt hat, trägt Material
dort auf, wo das Dokument keines hat. Bleibt keine Fläche übrig, entfällt das
Feature mit Fehler; bleibt nur ein Teil, sagt eine Warnung wie viele von wie
vielen. Dicke 1 = 0 bei Dicke 2 > 0 wird als *gespiegelte* einseitige Wand
geschrieben (dieselbe Geometrie, nur in der Form, die FieldDraft hat);
Dicke 1 = Dicke 2 = 0 ist ein Fehler. `keepTools` betrifft nur Onshapes
Flächenkörper und wird als solches gemeldet.

**Verifiziert an der Onshape-Geometrie, nicht an der eigenen.** Im
#415-Dokument dickt Thicken 1 die beiden Seitenwände von Extrude 2 um 2 mm nach
außen auf. Onshapes eigene Tessellierung, oberhalb der Bodenplatte
geschnitten (z > 10 mm), misst dort x ∈ [−25, 25] und **y ∈ [−48,5, 48,5]** —
also exakt die ±46,5 mm der Sketch-Kante plus 2 mm. Der Slow-Test importiert
Sketch 2 + Extrude 2 + Thicken 1 als Teildokument, fährt es durch den echten
Mesher-Worker-Pfad bei App-Auflösung (FULL 64 / CONTEXT 32) und prüft genau
diese Zahlen.

**Ground-Truth-Korrektur.** `GET …/partid/JKD/boundingboxes` meldet für dieses
Teil z ∈ [−3, 32,8103] mm, die Tessellierung derselben API (322 074 Vertices)
aber z ∈ [−3, 32,5]. Die Box-Endpoint-Antwort ist für Teile mit
Blend-/Spline-Flächen ausdrücklich nur eine obere Schranke, und dieses Teil hat
sieben Fillets. Maßgeblich ist ab jetzt die Tessellierung — weiterhin Onshapes
Antwort, nur die exakte. (Festgehalten, weil die 0,31 mm eine Weile wie ein
fehlendes Feature aussahen.)

**Restblocker für #415 — der Sketch-Frame auf Wandflächen (KEIN Thicken-Thema).**
FieldDraft leitet die u/v-Basis einer flächengebundenen Sketch-Ebene
**allein aus der Flächennormalen** ab (`planeBasisFromNormal`, Konventions-Lock
mit dem Kernel: Weltachse mit der geringsten Ausrichtung zur Normalen als
Saat, zweimal kreuzen). Onshape leitet sie aus der **Parametrisierung der
Fläche** ab. Beide Regeln sind deterministisch — und verschieden. Auf der
Bodenwand dieses Dokuments (n = −Z, gemessen: `qWall(4, 12)` →
xAxis = +Y, yAxis = +X) hat Onshape xAxis = +X. Der Importer bäckt Onshapes
2D-Koordinaten aber wörtlich ein, also landen **Sketch 3** (die Bodenplatte mit
den Montagelaschen, 66 × 127 mm) und **Sketch 4** (der Deckelausschnitt) um 90°
verdreht. Ergebnis: FieldDraft x ∈ [−63,54, 63,54] · y ∈ [−48,5, 48,5] gegen
Onshape x ∈ [−33, 33] · y ∈ [−63,55, 63,55] — X und Y vertauscht, z stimmt
([−3, 32,5], beide). Die AABB bleibt deshalb als `it.fails` gepinnt.

Das ist **kein #415-Sonderfall**: es trifft jeden Sketch auf einer Wandfläche.
Die Kabelbox fällt nicht darauf herein, weil ihre flächengebundenen Sketches
auf **Cap-Flächen** sitzen, deren Frame zufällig mit dem der Ursprungsebene
zusammenfällt. Der Verdacht liegt nahe, dass auch die X/Y-Abweichung von #414
zu Teilen hierher gehört und nicht nur an der Wall-Provenienz hängt.

Aus dem Feature-Payload allein ist Onshapes Frame **nicht** ableitbar — dort
steht nur 2D-Geometrie. Zwei gangbare Wege, beide ohne Raten:
(a) die 3D-Form der Sketch-Entities über `GET /partstudios/…/sketches?output3D=true`
mitziehen und den Frame aus 2D↔3D-Korrespondenz exakt bestimmen (kostet einen
zweiten Fetch und damit einen echten Import-Pfad-Umbau); (b) die
**externen Referenzen** in den Sketch-Constraints auswerten — im #415-Sketch-3
ist das Rechteck per `OFFSET` von der Flächenberandung abgeleitet
(`externalMaster`), und ±33/±54,5 sind nachweislich die 8-mm-Aufweitung der
50 × 93-mm-Wand; eine solche Zuordnung Kante↔Sketch-Kurve legt die Drehung
eindeutig fest. Beides ist ein eigenes Paket, kein Anhängsel von P4.

**Testabdeckung.** `onshapeImport.test.ts` prüft den Übersetzungs-Vertrag gegen
die echte Fixture (12 neue Tests): zwei distinkte `qFace`-Referenzen aus zwei
Onshape-Wänden, 2 mm nach außen, ADD mit explizitem Scope, Dicke 2 → innere
Wand, Richtungspfeil → `reverse`, die Umschreibung der leeren ersten Richtung,
eine variablen-getriebene Wandstärke, und die drei Abbruchpfade (Dicke 0,
Fläche bindet nichts, Fläche bindet mehrdeutig). Der Mock-Kernel liefert dafür
jetzt **eine Fläche je distinkter Query** — vorher beantwortete er jede Query
mit derselben Fläche, unter der ein Kollaps von N Referenzen auf eine
unbemerkt durchgegangen wäre. `onshapeImport415E2E.slow.test.ts` fährt das
echte WASM: Thicken-Feature-Call, saubere Regeneration, keine eigene Meldung,
und die ±48,5-mm-Wände über den echten Mesher-Worker-Pfad.

## Update 2026-07-26 (3) — Mate-Connectors übersetzt, Sketch-Ebene braucht den Kernel

**Neu übersetzt: `mateConnector`** — sowohl der im Feature-Baum stehende
(22 Vorkommen im Korpus) als auch der **implizite**, den ein Sketch als eigenes
Sub-Feature mitbringt (#416: 3×). Onshapes Connector ist ein Koordinatensystem,
das aus Geometrie abgeleitet wird: `originType: ON_ENTITY` + `originQuery`
nennen die Entity, `entityInferenceType` welchen Punkt davon (`MID_POINT`,
`CENTER`, `POINT` samt `secondaryOriginQuery` für den gemeinten Endpunkt), und
`translationX/Y/Z` + `rotation` + `flipPrimary` die Nachjustierung. FieldDrafts
`mate_connector` ist auf denselben Regeln gebaut — +Z ist die Kantentangente
bzw. die Flächennormale, `t` die Position entlang einer Kante, `offset` eine
Verschiebung entlang der **eigenen** Achsen in mm, `flip` das Umdrehen von +Z —,
weshalb die Abbildung eine **Wahl der richtigen Entity** ist und keine
Nachbildung des Frames. Neu ist `web/src/onshape/onshapeMateConnector.ts`
(Dekodierung) plus `mateAnchorQuery`/`mateConnectorParams` im Importer.

Eine Feinheit, die keine Näherung ist: `CENTER` auf einer **kreisförmigen
Cap-Kante** wird auf die Cap-**Fläche** abgebildet. Der Mittelpunkt der
Deckkanten-Kreislinie *ist* der Mittelpunkt dieser Fläche, und deren Normale
*ist* die Achse, auf die Onshape +Z legt — die Fläche zu nennen ist also
dasselbe Koordinatensystem in der Form, die FieldDraft exakt auflöst. Eine
Drehung überlebt nur in FieldDrafts 90°-Schritten; jeder andere Winkel wird
gemeldet statt gerundet, denn ein still zurechtgebogener Connector nähme jeden
Sketch auf ihm mit. Anker werden wie Sketch-Ebenen **auf Eindeutigkeit
geprüft** (genau eine Bindung, mid-history), sonst übersprungen.

**Die Ebenen selbst — exakt vermessen, nicht abgeleitet.** Onshapes
`GET /partstudios/…/sketches?includeGeometry=true&output3D=true` meldet jede
Sketch-Entity in **Weltkoordinaten**; zusammen mit der 2D-Geometrie aus dem
Feature-Payload ist der Frame `P = Ursprung + u·x + v·y` damit
**überbestimmt** und prüfbar statt geraten. Für #416 (Residuum ~1e-14 mm, in mm):

| Sketch | Ursprung | x-Achse | y-Achse | Normale |
| --- | --- | --- | --- | --- |
| Sketch 1 (Kontrolle, Right-Ebene) | (0; 0; 0) | +Y | +Z | +X |
| Sketch 2 | (0; 6,5; −65) | +X | −Y | −Z |
| Sketch 6 | (3; −10,1; −65) | +X | −Z | +Y |
| Sketch 4 | (−50; −19,2; −55,83) | +X | +Y | +Z |

Die Kontrollzeile ist der Beleg für die Methode: Sketch 1 reproduziert
FieldDrafts eigenen `origin_plane_yz`-Frame auf die letzte Stelle. Sketch 4 ist
aus den Entity-Punkten allein **unterbestimmt** (alle liegen auf u = 0), die
x-Achse kommt deshalb aus
`GET …/sketches/<id>/tessellatedentities`, das den Bogen ausflacht. Die Rohantwort
liegt als `fixtures/onshape-416-sketches3d.json` bei; `onshapeMateConnector.test.ts`
rechnet jede 2D-Entity durch den Frame und vergleicht sie mit Onshapes 3D-Punkt.
Das sind die **Abnahmedaten für die Kernel-Arbeit**.

**Restblocker: zwei Kernel-Fähigkeiten fehlen — web-seitig ist #416 nicht zu
bewegen.** Beide sitzen in `crates/kernel/src/features/context_resolve.rs`:

1. **`resolve_plane_frame` (:727)** kennt genau zwei Ausgänge — eine der drei
   fest verdrahteten Datum-Ebenen (`sketch/frame.rs::datum_plane_frame`, :100)
   oder den Frame einer aufgelösten B-Rep-Fläche (`resolve_face_frame`, :44).
   Ein Mate-Connector ist keines von beidem, obwohl `ctx.mate_connectors`
   seinen aufgelösten `origin`/`normal`/`x_axis` bereits hält. Es fehlt der
   dritte Zweig: `qCreatedBy(<mc_feature>)` → `SketchFrame { origin, x_axis,
   y_axis = normal × x_axis, normal }` — genau die Form, die der
   `CreatedBy`-Zweig von `resolve_mate_anchor` (:1002) schon baut. Dazu gehört
   der TS-Spiegel in `web/src/sketchPlaneFrame.ts::resolvePlaneFrame`
   (Konventions-Lock). `ParsedSketchDef` (`sketch/feature.rs:74`) braucht dafür
   **kein neues Feld**: die Ebene bleibt eine Query.
2. **`resolve_mate_anchor` (:899)** hat keinen `QueryExpr::SketchLine`-Zweig.
   Er nimmt `qFace`/`qEdge`/`qVertex`/`qSketchPoint`, aber rohe Sketch-Linien
   erreichen den B-Rep nie. Sketch 2 hängt genau daran (Endpunkt einer
   Konstruktionslinie von Sketch 1), und ein `qSketchPoint` hilft nicht: der
   gäbe als +Z die **Sketch-Ebenen-Normale**, Onshape legt +Z auf die
   **Linienrichtung**. Der Zweig ist klein — `resolve_axis_world_segment` (:1029)
   hebt eine `qSketchLine` bereits in ein Weltsegment; daraus folgen Ursprung
   (bei `t`) und Tangente unmittelbar.

**Warum das web-seitig nicht zu umgehen ist.** Geprüft, nicht vermutet:

- **Keine koplanare Fläche als Ersatz.** Am echten Kernel durchgemessen: die
  Ebene von Sketch 6 (y = −10,1) schneidet **mitten durch** den Körper, dort
  liegt weder vor noch nach dem Feature eine Fläche. Sketch 2s Ebene (z = −65)
  bekäme erst später eine — nach dem Zeitpunkt, zu dem sie gebraucht wird.
- **Ein Hilfskörper wäre ein Hack.** Eine Offset-Fläche nur als Ebenenträger
  anzulegen verändert die Teileliste (die Ground Truth ist **ein** Körper) und
  hängt den Sketch an eine Geometrie, die das Dokument nie hatte.
- **Die Kette hängt an Sketch 2.** Sketch 2 → Extrude 3 → Sketch 3 → Extrude 4
  → Sketch 4 → Revolve 1: fällt Sketch 2, fällt alles. Deshalb landen weiterhin
  **4 von 19** Features, und die AABB bleibt
  x[−4; 32] · y[−26,7; 11] · z[−65; 39,5] mm gegen Onshapes
  x[−70,05; 3,07] · y[−26,77; 7,13] · z[−73,08; 35,0] mm. Der
  Ground-Truth-Test bleibt `it.fails` — mit den zwei Kernel-Einstiegspunkten
  im Kommentar.

Mit (1) + (2) landen alle drei Sketches und die 15 daran hängenden Features;
die beiden Fasen des Dokuments (OFFSET_ANGLE @ 45°, je 5 mm) sind seit v0.32.0
abgedeckt und warten nur auf ihre Wände.

**Testabdeckung.** `onshapeMateConnector.test.ts` (14 Tests): Erkennung der
unkomprimierten `BodyType.MATE_CONNECTOR`-Ebenenquery, die drei impliziten
Connectors mit Anker, Inferenz und `t`, ein einteiliger Sketch-Entity-Name
(`EDMomUfcnXmW` — die Form, an der ein nur zweiteiliger Dekoder scheitert),
Realignment in mm/Grad, und die vier Frames gegen Onshapes 3D-Geometrie.
`onshapeImport.test.ts` (8 neue Tests): Cap-Anker als `qCreatedBy(op, CAP_*)`,
Cap-Kante mit `t = 0,5`, Offset/Flip/Reorient, die gemeldete
Nicht-90°-Drehung, der übersprungene Sketch-Linien-Anker und ein Connector, der
im Feature-Baum steht. `onshapeImport416E2E.slow.test.ts` nagelt am echten
WASM fest, dass genau drei Sketches als „auf Mate-Connector" gemeldet werden,
jeder seinen Anker nennt, und dass die zweite, tiefere Lücke benannt ist.

## Update 2026-07-26 (4) — der Sketch-Frame auf Wandflächen ist zu, #415 trifft die Ground Truth

**Der Konflikt.** Ein Sketch ist 2D-Geometrie plus die starre Abbildung, die sie
in den Raum hebt: `P = Ursprung + u·x + v·y`. Für eine flächengebundene Ebene
leiten beide Systeme diese Basis deterministisch her — und verschieden:
FieldDraft allein aus der Flächennormalen (`planeBasisFromNormal`, Weltachse mit
der geringsten Ausrichtung zur Normalen als Saat, zweimal kreuzen —
Konventions-Lock mit `face_plane_frame` im Kernel), Onshape aus der
Parametrisierung der Fläche. Wer Onshapes 2D-Koordinaten wörtlich einbäckt, legt
den Sketch quer.

**Die Lösung: der IMPORTER rechnet um, keine der beiden Konventionen ändert sich.**
`B_os` wird an Onshapes eigener Antwort GEMESSEN, `B_fd` kommt aus dem
gemeinsamen Resolver `web/src/sketchPlaneFrame.ts` (die SSOT, nicht eine Kopie
der Regel), und die In-Plane-Abbildung `B_fd⁻¹ ∘ B_os` ist eine exakte
2D-Drehung/Spiegelung samt Ursprungsversatz. Für gleiche Frames — jede
Ursprungsebene — ist sie die Identität und rührt nichts an. Neu:
`web/src/onshape/onshapeSketchFrame.ts`.

**Die Frame-Quelle, und warum diese.** `GET /partstudios/…/sketches?includeGeometry=true&output3D=true`
meldet jede Sketch-Entity in Weltkoordinaten. Zusammen mit der 2D-Geometrie aus
dem Feature-Payload ist der Frame damit **überbestimmt**: eine Ausgleichsrechnung
über alle 2D↔3D-Paare liefert x, y und Ursprung, und die Gegenprobe (orthonormale
Achsen, jeder Punkt auf 1e-6 mm getroffen) sagt, ob das Ergebnis trägt. Am
#415-Dokument gemessen, Residuum ≤ 1e-9 mm:

| Sketch | Ebene | Ursprung (mm) | Onshape x | Onshape y | FieldDraft x | FieldDraft y | Abbildung |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Sketch 1 (Kontrolle) | Ursprung XY | (0; 0; 0) | +X | +Y | +X | +Y | Identität |
| Sketch 2 (Kontrolle) | Ursprung YZ | (0; 0; 0) | +Y | +Z | +Y | +Z | Identität |
| Sketch 3 | Bodenwand, n = −Z | (0; 0; −3) | +X | −Y | +Y | +X | (u, v) → (−v, u) |
| Sketch 4 | Deckelfläche, n = +Z | (0; 0; 32,5) | +X | +Y | −Y | +X | (u, v) → (−v, u) |

Die beiden Kontrollzeilen sind der Beleg für die Methode: auf den
Ursprungsebenen reproduziert die Messung FieldDrafts eigene Datum-Frames auf die
letzte Stelle, und die Umrechnung ist dort die Identität. Genau so muss es sein —
die Frames stimmten dort schon vorher überein.

Die Alternative (b) der Voranalyse (Constraint-`externalMaster`-Referenzen)
wurde verworfen: sie ist indirekt und liefert die Drehung nur, wo eine
Offset-Beziehung zur Flächenberandung besteht, während die 3D-Antwort jeden
Sketch abdeckt und **prüfbar** ist statt plausibel.

**Was umgeschrieben wird — der PAYLOAD, nicht das Übersetzungsergebnis.**
`transformSketchFeature` liefert eine Kopie des Onshape-Sketch-Features in
FieldDrafts Basis. Dadurch sehen ALLE 2D-Leser dieselben Zahlen — die
Sketch-Übersetzung, die Region-Innenpunkte (`sketchEntityVertices`, #412) und die
Linien-Erkennung der Revolve-Achse (`sketchLineEntityIds`, #416) —, ohne dass die
Umrechnung an drei Stellen gespiegelt werden müsste. Bewegt werden Punkte,
Linien-Stützpunkte und -Richtungen, Kreis-/Ellipsen-Mittelpunkte und -Achsen
sowie die Winkel eines Bogens; Längen, Radien und Entity-Ids bleiben, weil die
Abbildung starr ist. `qSketchLine`/`qSketchPoint`-Ordinale ändern sich damit
nicht — Entities werden transformiert, nicht umnummeriert; Hole-Ziele und
Revolve-Achsen bleiben dieselben Entities an der jetzt richtigen Stelle.

Drei Feinheiten, die keine Näherung sind:

- **HORIZONTAL/VERTICAL** sind an die Achsen gebunden. Vertauscht die Abbildung
  die Achsen, tauschen auch sie — sonst arbeitete der Solver gegen die gerade
  gedrehte Geometrie. Bei einer **schiefen** Drehung gibt es keine treue
  Entsprechung; dann werden sie **verworfen** (die Geometrie steht explizit da)
  und das wird gemeldet.
- **Eine Spiegelung** (die Ebenennormalen zeigen gegeneinander, det = −1) dreht
  den Umlaufsinn um: derselbe Bogen läuft gegen den Uhrzeigersinn jetzt vom
  früheren END- zum STARTpunkt, also tauschen Winkel und Endpunkt-Ids mit.
- **Onshapes Sketch-Ursprung** ist nicht zwangsläufig FieldDrafts (Entity 0 sitzt
  auf dem Flächen-Zentroid). Liegt er versetzt, werden Bedingungen auf ihn
  verworfen statt Geometrie an einen Punkt zu ziehen, den dieser Sketch nicht
  hat. Im #415-Dokument tritt der Fall nicht auf (Versatz exakt 0).

**Erst die Ebene, dann die Basis.** Weicht die gemessene NORMALE von der der
aufgelösten FieldDraft-Fläche ab, oder liegt Onshapes Ursprung neben dieser
Ebene, wird **nicht** umgerechnet, sondern ein Fehler gemeldet: dann hat die
Ebenen-Query eine andere Fläche erwischt, und das wegzurechnen hieße, einen
Fehlgriff zu kaschieren. Ist der Frame aus der 3D-Antwort nicht bestimmbar (alle
Stützpunkte kollinear — #415 „Sketch 5", vier Ellipsen auf einer Geraden), wird
der Sketch wörtlich übernommen und genau das gesagt; geraten wird nichts.

**Live-Pfad.** Neue Proxy-Route `GET /api/onshape/sketches` (`server/onshape.go`,
sechs Zeilen Handler plus Registrierung) → `platformApi.getOnshapeSketches` →
`importOnshapeProject` zieht die Antwort neben dem Feature-Baum und reicht sie
als `sources.sketches3d` in `importOnshapeFeatures`. Ein Fehlschlag versenkt den
Import **nicht**: der Rest des Dokuments ist davon unberührt, kommt an, und die
Lücke steht als Warnung obenan. Die Übergabe ist ein Parameter, kein Fetch im
Importer — deshalb speisen die Tests sie aus Fixtures und brauchen kein Netz.

**Ergebnis #415 — vorher/nachher, am Render-Mesh gemessen (mm):**

| | x | y | z |
| --- | --- | --- | --- |
| Onshape (Tessellierung) | [−33; 33] | [−63,55; 63,55] | [−3; 32,5] |
| FieldDraft **vorher** | [−63,54; 63,54] | [−48,5; 48,5] | [−3,00; 32,50] |
| FieldDraft **nachher** | [−33,00; 33,00] | [−63,54; 63,54] | [−3,03; 32,55] |

X und Y waren vertauscht; jetzt stimmen sie. Der `it.fails` „matches the Onshape
ground-truth bounding box" ist ein **echter Pass**, und daneben steht die
Gegenprobe: ohne Onshapes 3D-Antwort landet dasselbe Dokument weiterhin verdreht
(damit der Fix nicht still zum No-op werden kann).

**Warum am Render-Mesh und nicht an `sceneAabb`.** `sceneAabb` ist eine
KONSERVATIVE Schranke über den CSG-Baum. `Extrude 1` profiliert den GESAMTEN
Sketch 1 — einschließlich der sechs deckungsgleichen `…MirrorCS`-Segmente, die
Onshape bei x = −273 mm parkt und die weder eine Region beranden noch Material
tragen. Die Szenen-Box liest deshalb x ∈ [−273; 91] für einen Körper, der bei
±33 mm endet (vor wie nach diesem Paket — das ist die #412-Regionenlücke, kein
Frame-Thema). Onshapes Ground Truth ist seine **Tessellierung**, also ist
FieldDrafts Tessellierung die vergleichbare Antwort — und die einzige, die
zugleich beweist, dass der Nutzer das Teil sieht.

**Effekt auf #414 (gemessen, nicht vermutet).** Das Dokument hat einen
flächengebundenen Sketch 3, und der Verdacht der Voranalyse bestätigt sich zur
Hälfte. Render-Mesh gegen Onshapes AABB x[−65,05; 65,0] · y[−70,13; 0] ·
z[−50; 50]:

| | x | y | z |
| --- | --- | --- | --- |
| FieldDraft **vorher** | [−74,21; 65,0] | [−104,00; 2,74] | [−50; 52,01] |
| FieldDraft **nachher** | [−67,89; 65,0] | [−70,03; 2,86] | [−50; 50,12] |

Die grobe Y-Überdehnung (−104 statt −70,1) war der Frame-Konflikt und ist weg;
der Rest (x um 2,8 mm zu weit, y-Maximum 2,9 mm über 0) hängt weiter an der
**Wall-Provenienz region-eingeschränkter Profile** (docs/SKETCH-REGION-IMPRINT-PLAN.md §5),
an der dort Chamfer-Kante und sechs Fillets scheitern. #414 bleibt offen, aber
aus genau einem Grund weniger.

**Testabdeckung.** `onshapeSketchFrame.test.ts` (19 Tests) gegen die echten
Fixtures: die vier gemessenen Frames samt beider Kontrollzeilen, die Verweigerung
bei kollinearen Stützpunkten, die 90°-Abbildung mit Determinante und
Achsentausch, die Erkennung einer FREMDEN Ebene, das Umschreiben von Punkten,
Linien, Bögen (Drehung wie Spiegelung), der HORIZONTAL/VERTICAL-Tausch, das
Verwerfen bei schiefer Drehung und bei versetztem Ursprung — und zuletzt die
eigentliche Abnahme: jede umgerechnete 2D-Entity, durch FieldDrafts Frame in den
Raum gehoben, landet auf Onshapes eigenem 3D-Punkt (1e-9). `onshapeImport.test.ts`
(6 neue Tests) prüft die Verdrahtung: unangetastet bei gleichem Frame, gedreht
samt Bedingung bei anderem, mitverschoben bei versetztem Ursprung, Fehler statt
Umrechnung bei fremder Ebene, Meldung bei fehlendem Sketch in der 3D-Antwort, und
Stille, wenn gar keine 3D-Quelle mitkommt. `onshapeProjectImport.test.ts` (2 neu)
nagelt fest, dass die Antwort für **dieselbe** Part-Studio-Adresse geholt und
durchgereicht wird und dass ihr Ausfall den Import nicht versenkt.
`server/onshape_test.go` (2 neu) prüft Pfad, beide Query-Flags und dass die
HMAC-Signatur den Query-String deckt. `onshapeImport415E2E.slow.test.ts` fährt
das echte WASM gegen die Ground Truth samt Gegenprobe.

**Offene Reste dieses Pakets.** (a) Ein Sketch, dessen Stützpunkte alle auf einer
Geraden liegen, bleibt unbestimmbar — aus Punkten allein folgt der Frame dort
nicht, und die zweite Richtung wäre nur aus `…/sketches/<id>/tessellatedentities`
zu holen (im Korpus betrifft es genau einen Sketch, dessen Ellipsen der
Übersetzer ohnehin nicht kennt). (b) Sketches auf Mate-Connectors laufen weiter
in die Kernel-Lücke von Update (3) und erreichen die Umrechnung gar nicht.
(c) `sceneAabb` bleibt durch Ganz-Sketch-Profile aufgebläht — das ist die
Regionen-Auflösung, s. #412.

## Update 2026-07-27 — P5 Muster + Spiegelung, und warum `mirror` erst den Kernel brauchte

Drei Onshape-Features (`linearPattern`, `circularPattern`, `mirror`) bilden
zusammen dieselbe Form: eine Menge von KÖRPERN, eine Regel wohin die Kopien
gehen, eine Anzahl. FieldDraft hat alle drei als Kernel-Features — die Arbeit
war also Übersetzung, nicht Konstruktion. Bei zweien stimmte das auch.

### `mirror` spiegelte nicht

`MirrorFeature::execute` reflektierte den `translate` des Quell-Features und
ließ dessen Rotation „verbatim" stehen — der Kommentar nannte das ausdrücklich
eine bewusste Vereinfachung („chirality-aware mirroring comes later"). Das ist
aber keine abgeschwächte Spiegelung, sondern **gar keine**: gespiegelt wird der
URSPRUNG des Features, nicht der Körper. Für einen Sketch-basierten Körper —
und das ist im Import der Normalfall — liegt dieser Ursprung im Weltnullpunkt,
die Reflexion bildet ihn auf sich selbst ab, und die Kopie landet **exakt
deckungsgleich auf dem Original**. Sichtbar wird das nie: zwei identische
Körper am selben Ort sehen aus wie einer.

Die drei bestehenden Kernel-Tests haben das nicht gefangen, weil alle drei nur
`clone.transform.translate` prüften — genau die eine Komponente, die die
Attrappe richtig hatte. Der neue Test misst deshalb das **SDF-Feld** des Klons
(`features_tests.rs::mirror_moves_the_geometry_not_just_the_feature_origin`)
und wurde gegengeprüft, indem die alte Implementierung kurz zurückgespielt
wurde: sie fällt, die alten Tests bleiben grün.

### Die Reflexion passt exakt in die TRS-Form

Der Kernel speichert Platzierungen als Translate/Rotation/Scale. Eine
Reflexion durch `(p₀, n)` ist `x ↦ A·x + 2(n·p₀)·n` mit `A = I − 2nnᵀ`, und
`A` zerfällt exakt:

    A = (−I) · R₁₈₀(n)          weil  R₁₈₀(n) = 2nnᵀ − I

also **180°-Drehung um die Ebenennormale plus uniformer Scale −1**; das
Quaternion ist schlicht `(nₓ, n_y, n_z, 0)`. Uniform ist dabei zweimal
entscheidend: `compose_transforms` faltet Scale und Rotation getrennt, was nur
für kommutierende (= uniforme) Scales exakt ist — so wird `reflection ∘ source`
exakt statt genähert. Und |−1| = 1 macht die Abbildung zur **Isometrie**: das
SDF des gespiegelten Körpers bleibt ein echtes Distanzfeld, ohne
Lipschitz-Verlust und ohne March-Dämpfung.

Ein negativer Scale läuft dadurch fast überall ohne Sonderfall durch: die
AABB-Abbildung vereinigt ohnehin alle acht transformierten Ecken, und der
Mesher leitet sein Winding aus dem Gradienten des Feldes ab statt es durch die
Transformation zu schleppen. **Eine** Stelle brauchte einen Fix:
`Transform::min_scale()` gab den vorzeichenbehafteten Minimalwert zurück, also
−1 — und dieser Faktor multipliziert in Raymarcher, BVH und Opcode-Tape die
Distanz. Ein negierter Abstand dreht Körper und Leere um. `min_scale()` nimmt
jetzt die BETRÄGE (was `dc.rs::min_scale` immer schon tat).

### Was die Übersetzer bewusst verweigern

Geraten wird nichts; jede Lücke meldet sich benannt:

- **`equalSpace` über einen Teil-Winkel** (circularPattern): ob Onshape den
  Bogen durch `count` oder `count − 1` teilt, entscheidet über die Lage JEDER
  Kopie außer der ersten, und der Korpus enthält keinen solchen Fall zum
  Nachmessen. Die beiden exakt deckungsgleichen Fälle (`equalSpace` + 360°,
  sowie kein `equalSpace`) werden übersetzt.
- **Zweite Richtung** (linearPattern): ein 2D-Raster ist ein Muster eines
  Musters — die erste Richtung wird importiert, die zweite gemeldet.
- **Muster eines Musters**: die Kopien sind Kinder des Muster-Features, keine
  eigenen Körper; als Quelle nicht adressierbar.
- **FACE-/FEATURE-Muster**: FieldDraft vervielfältigt ganze Körper.
- **Mate-Connector als Spiegelebene**: seit 2026-07-27 übersetzt (s. Update 2).

## Update 2026-07-27 (2) — Mate-Connector-Ebenen, Ursprungs-Anker, Ausdruck-Offsets

Die beiden Kernel-Lücken, die #416 blockierten, sind **zu**. Dazu kamen zwei
Befunde, die erst sichtbar wurden, als der erste Sketch überhaupt landete.

### Was jetzt geht

1. **`resolve_plane_frame` kennt Mate-Connectors.** `qCreatedBy(<mc>)` liefert
   den CSYS des Connectors als `SketchFrame`, über den neuen Konstruktor
   `SketchFrame::from_csys(origin, normal, x_axis)` (orthonormalisiert `x`
   gegen die Normale, schließt mit `y = n × x`). Die Reihenfolge der Leiter ist
   **Datum → Mate-Connector → Fläche**, und das ist keine Kosmetik: ein
   Connector sitzt meistens AUF einer Fläche, ein Durchfallen auf den
   Flächen-Zweig lieferte also still deren Schwerpunkt und deren synthetische
   Basis statt Ursprung und Drehung des Connectors — eine Ebene am richtigen
   Ort mit dem falschen 2D-Koordinatensystem. Der TS-Spiegel
   (`web/src/sketchPlaneFrame.ts`) zieht mit, inklusive `frameFromCsys`.
2. **`resolve_mate_anchor` kennt `qSketchLine`.** `t` bedeutet dasselbe wie an
   einer B-Rep-Kante, weil beide durch `polyline_point_tangent` gehen; die
   Sketch-Ebenen-Normale wird zur Auto-Sekundärachse. Damit landet ein
   Connector auf einer Konstruktionslinie — genau das, woran man ihn hängt,
   solange es noch keinen Körper gibt.
3. **Anker auf dem Part-Studio-URSPRUNG.** Onshapes Default-Connector hängt an
   der Query `Origin`/`pointOp` — kein `sketchEntityId`, kein Feature, keine
   Geometrie. Der Decoder erkennt sie als eigene Anker-Art (`originPoint`), der
   Importer übersetzt sie in `qCreatedBy(origin_plane_xy)`, und
   `resolve_mate_anchor` kommt über die Datum-Sprosse der Ebenen-Leiter daran.
   Das ist der Anker des ERSTEN Sketches eines Dokuments; ohne ihn fiel bei
   #409 die halbe Historie.
4. **Realignment-Offsets werden AUSGEWERTET.** `readMateConnector` las
   `translationX/Y/Z` und `rotation` aus Onshapes `value`-Feld — und das ist
   für einen Ausdruck fast immer 0 (die Falle aus #415, dort für 153 Ausdrücke
   behoben, hier übersehen). `translationZ = #Hoehe-3cm` kam als 0 mm an. Der
   Leser nimmt jetzt einen `QuantityReader` entgegen, den der Importer an
   seinen Ausdrucks-Evaluator bindet. Messbar: die Szene von #409 lag vorher
   bei z ∈ [−1617; +15] mm, jetzt bei z ∈ [+192; +1825] gegen Onshapes
   [0; 1840].

### Was #409 noch offen hat — gemessen, nicht geschätzt

`onshapeImport409E2E.slow.test.ts` fixiert die Liste namentlich. 13 von 33
Körpern stehen; die restlichen Lücken sind **sieben** Klassen, keine davon P5:

| Klasse | Betroffen | Kern |
|---|---|---|
| A `qWall(op, ord)` bindet KEINE Fläche (**#424**, für #409 erledigt in **#428**) | ~~Sketch 4, 5, 13~~; #416 Sketch 6, Fasen, Fillets | ~~FieldDraft trennt Flächen nur an Knicken~~ — zweimal widerlegt, s. Update (5): die Wände lagen im Inneren eines Körpers, dem sie nie hätten beitreten dürfen |
| B falsche Ebene aufgelöst (Normale stimmt, Versatz nicht) | Sketch 6, 7, 11, 12 | Der Importer rechnet Koordinaten NICHT um, wenn er die Ebene nicht wirklich getroffen hat |
| C End-Condition UP_TO_NEXT / UP_TO_SURFACE | Extrude 8, 12 | Zielfläche nicht auflösbar bzw. liegt in der Sketch-Ebene |
| D Loch-Position ist kein Sketch-Punkt | beide `Bohrung`-Features | |
| E Mate-Connector-Inferenz `CENTROID` | Sketch 8 | `anchorT` kennt nur MID_POINT/POINT |
| F ~~Fasen-Kante als EDGE/SWEPT_EDGE~~ (**#425**, Dekoder erledigt) | Chamfer 1 | `SWEPT_EDGE` wird übersetzt; Chamfer 1 hängt jetzt an `Extrude 14`, das nicht importiert (Folgefehler, s. Update (6)) |
| G unbekannte Ebenen-Query-Form | Sketch 14 | |

Alles andere sind Folgefehler: ein Extrude ohne Profil-Sketch, ein Spiegel
ohne Quellkörper. **Klasse A ist der größte Hebel** — sie blockiert in #409
und #416 zusammen die meisten Features und ist eine Kernel-Frage
(Flächen-Segmentierung), keine Übersetzer-Frage.

### Testabdeckung, die dabei fehlte

Die P5-Übersetzer (`linearPattern`, `circularPattern`, `mirror`) hatten
**keinen einzigen Unit-Test** — nur den E2E-Lauf, der aus anderen Gründen rot
war. Nachgeholt in `onshapeImport.test.ts` („P5 — linearPattern /
circularPattern / mirror"): Abstand und Anzahl aus dem AUSDRUCK, Spiegel auf
einer Mate-Connector-Ebene, Verweigerung eines FLÄCHEN-Musters. Die
Umpflanz-Technik der #416-Tests (`standIn`) isoliert dabei den Übersetzer von
Lücken, die mit ihm nichts zu tun haben.

## Update 2026-07-27 (3) — #419 gemessen, drei Dekoder-Defekte, ein stiller Versatz

### #419 zum ersten Mal mit Zahlen statt Vermutungen

Grundwahrheit aus Onshapes REST-API: **17 Solid-Parts**, Bounding-Box
x ∈ [−3015,4; 85,6], y ∈ [−157; 163], z ∈ [−151; 169] mm. Der Import (v0.38.2)
liefert **9 Körper**, 224 Meldungen, Laufzeit **2,8 s** — vorher 26 min.

| Größe | vorher | v0.38.2 |
|---|---|---|
| Körper (Onshape: 17) | 9 | 9 |
| Meldungen | 223 | 224 |
| übersetzbare Sketch-Ebenen-Queries | 26/37 | **32/37** |
| Laufzeit | 26 min | **2,8 s** |

Dass die Körperzahl gleich bleibt, obwohl sechs Ebenen mehr dekodieren, ist der
Befund: Sketch 4, 6 und 13 scheitern jetzt **einen Schritt später und mit der
richtigen Begründung** — nicht mehr „Ebenen-Query nicht übersetzbar", sondern
„`qWall(…)` bindet 0 Flächen". Das ist Klasse A, also Kernel, nicht Übersetzer.

### Drei Dekoder-Defekte mit derselben irreführenden Meldung (v0.38.1)

1. **Ungepunktete Entity-Id.** `extractQueryRefs` erkannte eine Sketch-Entity an
   ihrer *Gestalt* `<basis>.<suffix>`. Eine schlicht gezeichnete Linie hat aber
   `pfTtMVvYeZsa` ohne Punkt. Jetzt wird der Wert am Schlüssel `sketchEntityId`
   gelesen; ein per Rück-Referenz gefüllter Wert verbraucht den Schlüssel-Slot.
2. **Seitenwand nur von `opExtrude`.** Der Kernel ist dort sweep-agnostisch
   (`revolve.rs:205` und `extrude.rs:378` füllen `wall_subs` beide aus
   `sketch::wall_sub_table`, `QueryExpr::Wall` fragt nicht nach dem Sweep), also
   zählt jetzt auch `opRevolve` — sechs #419-Sketches sitzen auf einem Mantel.
   `CAP_FACE` bleibt extrude-only: `capFaceQuery` braucht die Sweep-Richtung.
3. **`CENTROID` auf gerader Kante** ist deren Mitte (Klasse E, damit zu). Auf
   einer runden Kante bleibt sie abgelehnt — dort liegt der Schwerpunkt im
   Zentrum, also neben der Kante. Der Aufrufer bürgt für die Geradheit.

### Neue Klasse H: `startOffset` (v0.38.2)

Onshape kann die Extrusion auf einer Ebene **neben** der Skizze beginnen
(`startOffset` + `startOffsetDistance`); das Material spannt dann
[offset, offset+depth]. FieldDrafts Extrude hat kein solches Feld — `depth` und
`depth_back` messen beide von der Profilebene. Der Übersetzer hat den Versatz
bisher **stillschweigend verworfen** und den Körper um genau diesen Betrag
versetzt gebaut: #409s Extrude 7 (100 mm) schob die Szene auf z = 2154,7 mm,
314 mm über Onshapes Maximum von 1840. #419 trägt sieben solche Extrusionen.
Wird jetzt gemeldet statt still versetzt.

Damit für #409: **E ist zu** (Sketch 8 + Connector importieren), **H ist neu**
(Extrude 7 bleibt offen, aber aus dem richtigen Grund), offene Features 25 → 23.

### Die 26 Minuten waren eine Boolean gegen einen falsch platzierten Körper

Eine einzige `addFeatureCall revolve` (Revolve 5, `remove`) verbrauchte
1 566 927 ms von 1 569 951 ms — 99,8 %. Ihr Ziel war einer der Start-Offset-
Körper; seit die nicht mehr gebaut werden, ist der ganze Import in 2,8 s durch.
Die Kernel-Pathologie dahinter bleibt **unerklärt, nur nicht mehr ausgelöst**:
dieselbe Revolve als `new` kostet 0,35 s, als `remove` das 4500-fache.
Reproduzierbar auf Commit `0df5d1e7` mit der #419-Fixture — als Perf-Ticket
**#426** geführt, damit die Latenz nicht mit dem Symptom verschwindet.

## Update 2026-07-27 (4) — Klasse A gemessen: die verschmolzene Mantelfläche gibt es nicht (#424)

### Die führende Hypothese war falsch

Klasse A stand seit Beginn unter der Erklärung: FieldDraft trennt Flächen nur an
Knicken, also hat ein Profil mit tangential angesetzten Rundungen EINE
Mantelfläche, wo Onshape acht benennt — die per-Segment-Subs hätten dann keine
Flächen, auf denen sie sitzen könnten. Diese Erklärung stand sogar wörtlich in
der Fehlermeldung des Importers und im Kopf des #409-E2E-Tests.

Gemessen ist sie **falsch**. Ein abgerundetes Rechteck (vier Linien, vier
tangentiale Viertelbögen) extrudiert zu **acht getrennten WALL-Flächen** mit den
Subs 3, 7, 10, 14, 17, 21, 24, 28 — je einer pro Sketch-Entity, Sub 28
eingeschlossen, also genau der Wert aus dem Ticket-Titel. Festgehalten als
`rounded_rectangle_keeps_one_wall_face_per_sketch_entity`
(`crates/kernel/src/sketch_features_tests.rs`); geht der Test rot, ist die
Hypothese nachträglich wahr geworden.

### Die echte Ursache: region-eingeschränkte Profile hatten keine Herkunft

Der Dump aller BRep-Flächen je Feature trennt die beiden Kandidaten sauber:

| #419-Feature | Profil | Subs vorher | Subs nachher |
|---|---|---|---|
| 13 (Revolve 2) | `qRegion(6, 14)` | 0, 1, 2 | **13, 25, 28** |
| 8 (Revolve) | `qRegion(…)` | 0, 1, 2 | **64, 190, 193** |
| 7, 10, 16 (ganzer Sketch) | `qCreatedBy` | 34…166 | unverändert |

Wo das Profil der **ganze Sketch** ist, standen die stabilen Entity-Ids längst
da. Wo Onshape eine **Region** auswählt, fielen die Wände auf die positionale
Nummerierung 0, 1, 2, … zurück — und eine Entity-Id wie 28 kommt dort nie vor.
Grund: `resolve_profile_query` baute region-eingeschränkte Profile aus
`sketch_arrangement::arrange`, und dessen frisch getraversierte Atom-Face-Kontur
trug keine Provenienz (dokumentiert als Folgearbeit in
`SKETCH-REGION-IMPRINT-PLAN.md` §5).

**Behoben:** Die Arrangement-Kanten kennen ihre Quelle bereits (`Piece.src`);
sie wird jetzt über denselben Registry-Replay, aus dem der Ganz-Sketch-Trace
kommt (`sketch::replay_allocations`), auf die **Sketch-Kurven-Entity**
abgebildet und als `TracedPrim`/`TraceSources::PerSegment` an jeder `AtomicFace`
mitgeführt. Eine gepickte Region benennt ihre Wände damit exakt wie der ganze
Sketch. Nachweis end-to-end: `region_extrude_walls_resolve_by_sketch_entity`
löst `qWall(op, entity)` auf genau eine Fläche auf und prüft deren Normale.

### Was danach noch offen ist

- **`qRegions(…)` mit mehreren Regionen** bleibt positional: zwei benachbarte
  Regionen teilen sich eine Randlinie, die geteilte Sub kollidiert, und
  `wall_sub_table` verwirft die Tabelle lieber ganz, als halb zu benennen
  (#419 Feature 24 und 25). Sauber wäre, die Innenkanten der Vereinigung vor
  dem Tabellenbau zu streichen — sie sind im Ergebnis keine Wände.
- **#409s Extrude 2** ist ein dritter Fall: `qCreatedBy(4)` vereinigt VIER
  disjunkte Rechtecke, Onshape hat nur eines extrudiert und benennt dessen
  Wand. Die Tabelle ist stabil, die gesuchte Wand existiert im Körper nur
  nicht. Das ist keine Namensfrage, sondern eine Profilfrage.
- **Neuer Befund zu Klasse B:** #409s Sketch 6 nennt als `SWEPT_FACE`-Ursprung
  die Entity `<rect>.bottom` von Sketch 11 — die zugehörige Wand liegt bei
  x = 1072,5, Onshapes Sketch-Ebene aber bei z = 965, also 2,5 mm neben der
  Wand aus Linie #11 derselben Skizze. Der Dekoder liest die falsche
  Ursprungs-Entity. Solange die Wände positional numeriert waren, traf
  `qWall(12, 3)` diese Fläche **zufällig**; mit korrekter Benennung tut es das
  nicht mehr, und die Kette Sketch 6 → Extrude 11 → Sketch 7 hängt jetzt
  sichtbar ~75 mm unter Onshapes Boden. Der #409-E2E hält diese Abweichung als
  gemessene Zahl fest, nicht als Toleranz.

## Update 2026-07-27 (4) — #419 hat endlich einen Test, und der findet eine Drehung

`web/src/onshape/onshapeImport419E2E.slow.test.ts` gibt es jetzt, nach dem
Muster der vier anderen. Bis dahin war #419 das einzige gemessene Dokument
**ohne** Test: die Zahlen oben stammten aus einer Wegwerf-Probe, die danach
gelöscht wurde — nichts im Repo konnte sie reproduzieren, und nichts hätte
gemerkt, wenn sie sich ändern.

### Die Körperzahl ist nicht monoton (9 → 7 ist eine Verbesserung)

Differenziell gemessen, beide Läufe gegen dieselbe Fixture:

| Größe | v0.38.2 | v0.38.3 |
|---|---|---|
| Körper (Onshape: 17) | 9 | **7** |
| Meldungen | 224 | **212** |
| scheiternde Features | 125 | **118** |

Repariert wurden genau sieben, kaputt ging keines: `Boolean 2`, `Boolean 4`,
`Extrude 6`, `Extrude 13`, `Sketch 6`, `Sketch 13`, `Thicken 6`. Dass die
Körperzahl dabei **fällt**, liegt an den beiden Booleans: sie lösen jetzt auf
und verschmelzen Körper, die vorher einzeln herumlagen. „Mehr Körper" ist auf
diesem Dokument also kein Qualitätsmaß — der Test pinnt die Zahl deshalb
exakt (`toBe(7)`), statt sie nach oben oder unten zu beschränken.

### Das ganze Dokument steht um 90° um Z gedreht

Der Test vergleicht zum ersten Mal achsweise gegen die Grundwahrheit, und da
fällt auf, was die bloße Auflistung der Bounding-Box nie zeigte:

    unser (x, y, z) = Onshapes (−y, x, z)

Drei der vier waagerechten Schranken passen auf diese Abbildung **exakt**:
unser x ∈ [−163; 157] gegen Onshapes y ∈ [−157; 163], unser y-Minimum
−3015,4 gegen Onshapes x-Minimum. (Unser y-Maximum bleibt hinter Onshapes
85,6 zurück, weil 10 der 17 Teile fehlen — diese eine Schranke taugt als
Beleg nicht.) z stimmt auf 0,06 mm.

Das ist **keine Frame-Konvention der Bounding-Box-API**: #414, #415 und #416
prüfen x/y/z direkt gegen dieselbe API und sind grün. Verdächtig ist die
bekannte Regel-Differenz: FieldDraft leitet die u/v-Basis einer
flächengebundenen Sketch-Ebene aus der **Normalen** ab, Onshape aus der
**Parametrisierung** der Fläche (siehe #415 weiter oben). Beide Regeln sind
deterministisch und verschieden — und ein Wurzel-Sketch, der auf einer Fläche
sitzt, vererbt die Drehung an alles Nachfolgende.

Die Drehung ist **älter als v0.38.3**: die AABB ist über den #424-Fix hinweg
bit-identisch, gemessen. Der Test hält sie als die Abbildung fest, die sie
ist, nicht als weiche Schranke — an dem Tag, an dem der Frame stimmt, fallen
diese vier Zeilen und gehören durch die direkte Prüfung gegen
`GROUND_TRUTH.x` / `.y` ersetzt.

### Zusammensetzung der 212 Meldungen

Die Hälfte ist Kaskade — ein Feature scheitert nur, weil etwas fehlt, worauf
es sich bezieht:

| Anzahl | Ursache |
|---|---|
| 67 | Kaskade: Bezug auf ein nicht importiertes Feature |
| 42 | unbekannte Kanten-Query-Form (#425) |
| 41 | Kaskade: Profil-Sketch fehlt |
| 23 | `CAP_EDGE`-Ursprung nicht auflösbar |
| 9 | `qWall(…)` bindet 0 Flächen (#424) |
| 7 | Loch-Position ist kein Sketch-Punkt |
| 6 | Flächen-Referenz für `thicken` |
| 6 | unbekannte Ebenen-Query-Form |
| 3 | End-Condition `UP_TO_NEXT` / `UP_TO_SURFACE` |
| 2 | `startOffset` (verweigert, nicht still versetzt) |
| 2 | Muster-Achse nicht in Weltkoordinaten auflösbar |
| 2 | Sketch-Entity fehlt in der Übersetzung |
| 2 | Feature-Typ `printedHole` ohne Übersetzer |

Rund 50 echte Lücken also; die andere Hälfte fällt mit ihnen.

## Update 2026-07-27 (5) — Klasse B gemessen: der Dekoder liest richtig, die Ebene erbt einen Versatz (#427)

Der Befund am Ende von Update (4) — „#409s Sketch 6 nennt als `SWEPT_FACE`-
Ursprung die falsche Entity" — ist **widerlegt**. Gegen Onshapes eigene
3D-Antwort geprüft, nennt der Dekoder exakt die Entity, die Onshape nennt.

### Was die Query wirklich sagt

Sketch 6s Ebenen-Query, entpackt:

    Query { queryType: SWEPT_FACE, entityType: FACE,
            operationId: [Fki57tHha8xZm97_14 opExtrude]      // Extrude 10
            disambiguationData: [ORIGINAL_DEPENDENCY, originals: [
              SKETCH_ENTITY OUhPEGp5z47e.bottom @ FU9cdmO9DPaHZuT_14   // Sketch 11
              SKETCH_ENTITY KzkU5Mf6zXlx.<ref> @ dito
              SKETCH_ENTITY WNxhiv4O9GzA.<ref> @ dito ]] }

Die drei genannten Kanten sind **kollinear** — sie laufen in Sketch 11 alle bei
z = 965 von x = 65 bis x = 855 und verschmelzen in Onshape zu EINER Wandfläche.
Jede von ihnen benennt dieselbe Ebene; der Dekoder nimmt die erste
(`OUhPEGp5z47e.bottom`), und Onshapes 3D-Antwort bestätigt: diese Kante liegt
bei z = 965, und Sketch 6 liegt bei z = 965. Der Dekoder trifft.

### Die Kette, die den Fehler wirklich erzeugt

1. **Sketch 2s 2D-Geometrie im `features`-Payload widerspricht Onshapes eigener
   3D-Antwort** — bei zwei seiner vier Rechtecke um genau 50 mm (2D y = −500,
   3D y = −550). Die beiden betroffenen Rechtecke hängen an EXTERNEN
   `COINCIDENT`-Referenzen, und die löst Onshape erst bei der **Regenerierung**
   auf: der Feature-Payload trägt den Autoren-Startwert, nicht die Lösung.
   `measureOnshapeSketchFrame` fittet einen starren Frame über ALLE Paare, kein
   starrer Frame erklärt 16 gute und 16 um 50 mm verschobene Punkte, also
   liefert die Messung `null` → „Frame nicht bestimmbar", Koordinaten wörtlich.
2. Damit steht jede Wand von Extrude 2 um 50 mm falsch, also auch die Ebene von
   **Sketch 11** (`qWall(5, 15)`: Normale exakt richtig, Abstand 49,9999 mm).
3. **Und hier stand der Verstärker:** `alignSketchFrame` verwarf die
   Umrechnung, sobald die Ebene um irgendeinen Betrag danebenlag. Sketch 11s
   2D-Koordinaten landeten dadurch ROH auf FieldDrafts Achsen — Onshapes
   v-Koordinate 1072,5 auf FieldDrafts +X. Genau daher kam die „Wand bei
   x = 1072,5": es ist keine Wand, es ist eine 2D-Zahl auf der falschen Achse.
   Extrude 10s Mantel stand danach um 90° verdreht, und `qWall(12, 3)` — die
   RICHTIG benannte Wand — lag senkrecht zu Onshapes Ebene.

### Der Fix (v0.38.4)

`alignSketchFrame` trennt die beiden Befunde, die vorher einer waren:

| Fall | vorher | jetzt |
|---|---|---|
| Normale schief (`\|n·n\|` ≠ 1) | Fehler, keine Umrechnung | unverändert |
| Ebene parallel versetzt | Fehler, keine Umrechnung | Fehler **mit Betrag**, Umrechnung IN der Ebene |

Steht die Normale, ist die In-Plane-Abbildung exakt bestimmt — sie
wegzuwerfen verhindert keinen Fehler, sondern erzeugt einen viel größeren.
Gemessen an #409 Sketch 6:

| | vorher | nachher |
|---|---|---|
| Normalen-Skalarprodukt zu Onshapes Ebene | 0,000000 | 1,000000 |
| Abstand | −1072,5 mm | 4,86 mm |
| Szenen-Untergrenze z (Grundwahrheit 0) | −74,8 mm | +938,1 mm |

Die 4,86 mm sind ein **eigener, kleinerer Rest**: FieldDraft löst Sketch 11
selbst, und drei `DISTANCE`-Bedingungen übersetzen nicht — die eigene Lösung
weicht von Onshapes um 4,9 mm ab.

### Was Klasse B noch offen hat

- **Der Wurzelfehler (50 mm)** bleibt: der `features`-Payload ist ein
  Startwert, keine Lösung. Sauber wäre, die 2D-Koordinaten aus Onshapes
  3D-Antwort **zurückzurechnen** statt sie nur zu prüfen — d.h. den Frame
  robust (Konsens statt Kleinste-Quadrate über alles) zu messen und danach
  jede Entity, für die es einen 3D-Punkt gibt, aus ihm neu zu setzen. Gemessen:
  patcht man Sketch 2s 16 Linien aus der 3D-Antwort, verschwinden Sketch 11s
  Meldung und der 50-mm-Versatz vollständig.
- **Der 4,9-mm-Rest** braucht die fehlenden `DISTANCE`-Bedingungen.

### Nebenbefund: `Boolean 1` verschluckt das ganze Traggerüst

`Boolean 1` zieht die vier senkrechten Pfosten (Extrude 2) von den
Deckplatten ab, mit Onshapes „Werkzeuge behalten" (`keepTools=true`). Der
Importer reicht das als `keep_tools: true` durch, `crates/kernel/src/features/
boolean.rs` **parst das Feld und benutzt es nie**: die Werkzeug-Features
bekommen bedingungslos `BoolOp::Remove` und hören damit auf, eigene Körper zu
sein. Gemessen: löscht man dieses eine Feature aus der Fixture, misst die Szene
z ∈ [0; 1840] — Onshapes Bereich auf den Millimeter. Mit ihm reicht sie nur von
z = 938 (Mittelboden) bis z = 1825 (Deckplatte), 887 mm.

Bis dahin hat der #409-E2E die Spannweiten-Schranke von 1500 mm auf diese
gemessene Zahl zurückgenommen — im selben Stil wie die z-Untergrenze davor:
eine benannte Defektzahl, keine Toleranz. Sie gehört zurück auf 1500, sobald
der Boolean seine Werkzeuge behält. (Vorher war die 1500er-Schranke grün, weil
die falsch platzierte Klasse-B-Kette 75 mm unter den Boden hing — sie maß einen
Defekt, nicht die Höhe.)

## Update 2026-07-27 (5) — Klasse A war in #409 gar keine Flächen-Frage (#428)

Klasse A hatte zwei Erklärungen, und **beide waren falsch**. Die erste — „ein
abgerundetes Rechteck hat EINE Mantelfläche" — hat Update (4) gemessen und
widerlegt: es sind acht, jede unter ihrer eigenen Sketch-Entity. Die zweite,
danach eingesetzte — „Extrude 2 vereinigt vier disjunkte Rechtecke, die
gesuchten Wände liegen im Inneren dieser Vereinigung" — stimmte in der
Beobachtung, aber nicht in der Ursache. Die Wände lagen tatsächlich innen. Nur
kam die Vereinigung nicht vom Extrude.

Sie kam von `Boolean 1`. Onshape zieht dort die vier senkrechten Pfosten mit
`keepTools=true` von den Platten ab; FieldDraft hat `keep_tools` geparst und
nie gelesen (#428). Die Pfosten hörten damit auf, eigene Körper zu sein — sie
wurden in den Plattenkörper gefaltet, und ihre Mantelflächen wurden dadurch
Innenflächen eines Körpers, dem sie nie hätten beitreten dürfen. `qWall`
suchte eine Außenfläche und fand keine.

**Gemessen an #409** (v0.38.4 → v0.38.5): z-Spannweite 886,9 mm → **1840,0 mm**,
z-Bereich [938,1; 1825] → **[0; 1840]** — Onshapes eigener Bereich auf den
Millimeter. Körper 13 → 15, Import-Fehler 30 → 22. Aus `KNOWN_GAPS` fallen vier
Namen (`Sketch 4`, `Sketch 5`, `Sketch 13`, `Extrude 13`), **keiner kommt
hinzu**. Die Schranke `keepToolsSpanMm` entfällt, der Test steht wieder auf
seinen ursprünglichen 1500 mm.

**An #419** steigt die Körperzahl 7 → 10 bei **unveränderter** Fehlerliste (212
Meldungen, identische Namen): acht Booleans dieses Dokuments tragen „Werkzeuge
behalten", die zusätzlichen Körper wurden vorher schlicht verschluckt. Der
exakte Pin im E2E-Test ist entsprechend nachgezogen.

Was von Klasse A bleibt, ist **#416** (Sketch 6, Fasen, Fillets) — dort gibt es
keine Boolean mit behaltenen Werkzeugen, die Ursache muss also eine andere sein
und ist noch nicht gemessen.

**Methodische Lehre, zum dritten Mal in dieser Matrix:** eine plausible
Erklärung, die die Beobachtung trifft, ist noch keine gemessene Ursache. Beide
A-Thesen erklärten „Wand ist innen" korrekt und lagen trotzdem daneben. Erst
das Differenzial gegen einen echten Rebuild hat entschieden.

## Update 2026-07-27 (6) — Klasse F: die Kanten-Formen sind gezählt, nicht geschätzt (#425)

### Was gemessen wurde

Ticket #425 sprach von „41 Vorkommen" der Marker `EDGE`, `SWEPT_EDGE` und
`CAP_FACE` in Kanten-Position. Beides war zu korrigieren. Erstens die Zahl:
**#419 hat 102** `entities`-Queries an `fillet`/`chamfer`, das gesamte Fixture-
Korpus **206** (#409 genau 1, #414 29, #415 37, #416 22, Kabelbox 9, #339 6).
Zweitens die Methode: `EDGE` steht in **jeder einzelnen** dieser Queries — es ist
der Entity-Typ-Filter, keine Form. Und der bisherige Dekoder hat genau danach
unterschieden, nämlich nach der MENGE der Marker irgendwo im Dokument. Das ist
nachweislich falsch: die Disambiguierung einer Query schachtelt die **vollen
Queries ihrer Vorfahren** ein, eine boolean-abgeleitete Kante trägt also
`CAP_FACE`- und `SWEPT_FACE`-Marker, die Flächen gehören, die nicht ihre sind.

Unterschieden wird jetzt am **Wurzel-`queryType`**. Verteilung über alle 206,
und was daraus wird:

| Wurzel-`queryType` | Anz. | abgebildet | FieldDraft-Form |
|---|---:|---:|---|
| `INTERSECT` (Kante = Schnitt zweier Flächen) | 78 | 17 | `qCommonEdge(a, b)`, sofern BEIDE Flächen `CAP_FACE`/`SWEPT_FACE`/`OFFSET_FACE` sind; die übrigen 61 haben eine `MERGE`-, `COPY`- oder `SPLIT`-Fläche |
| `CAP_EDGE` (Deckel-Rand direkt benannt) | 42 | 40 | `qCommonEdge(cap, qWall)`; 2 nennen drei Ursprünge und sagen nicht, welcher die Wand trägt |
| `COPY` (Kante einer Muster-Kopie) | 22 | 0 | **keine** — FieldDraft benennt keine Muster-Instanz |
| `MERGE` (aus einer Boolean verschmolzene Kante) | 20 | 0 | **keine** — welche der beiden Ursprungskanten gemeint ist, sagt die Query nicht |
| `SWEPT_EDGE` (senkrechte Kante zweier Wände) | 20 | 17 | `qCommonEdge(qWall, qWall)` — **neu in #425**; 3 nennen drei Skizzenelemente |
| `BLEND_EDGE`/`BLEND_FACE` (Kante an einer früheren Verrundung) | 14 | 0 | **keine** |
| `SPLIT` (Bruchstück einer geteilten Kante) | 4 | 0 | **keine** |
| `OFFSET_EDGE` (Innenrand einer Schale) | 4 | 4 | `qCommonEdge(INNER_CAP_*, INNER_WALL)` |
| `CAP_FACE` (ganze Fläche statt Kante) · undekodierbar | 2 | 0 | **keine** |

**78 der 206 sind abbildbar**, die restlichen **128 werden namentlich
abgelehnt** — „Kante einer Muster-Kopie (COPY)" statt „Kanten-Form nicht
übersetzbar (Marker: EDGE, SWEPT_EDGE)". Die mit Abstand größte Restklasse ist
nicht eine Kanten-, sondern eine FLÄCHEN-Frage: 61 `INTERSECT`-Kanten scheitern
daran, dass eine ihrer beiden Flächen selbst aus einer Boolean oder einem Muster
stammt.

### Der BTON-Dekoder liest jetzt strukturiert, nicht flach

`parseBton` baut das Query-Dokument als Objektbaum auf. Dafür mussten die
**zwei String-Tabellen** des Formats rekonstruiert werden, und sie sind nicht
dieselbe: `C<n>` (Klasse) und die negativen Teil-Längen innerhalb eines Strings
indizieren die Tabelle **mit** Klassennamen und zusammengesetzten Strings,
`R<n>` die **ohne** beide. Eine Verwechslung fällt nicht auf — sie liefert den
Nachbar-String, und dann heißt ein Map-Schlüssel `bottom` und ein `queryType`
`opRevolve`. Kalibriert an **allen 699 BTON-Queries der Fixtures** (nicht nur den
Kanten-Queries) über eine harte Bedingung: ein Schlüssel MUSS ein
Onshape-Feldname sein. Ergebnis: je 4431 saubere `entityType`/`historyType`/
`operationId`/`queryType`-Schlüssel. In 41 der 699 Payloads driftet die Tabelle
trotzdem — dort teilt Onshape ganze Unterstrukturen über einen zweiten
Verweis-Raum, den wir nicht rekonstruieren. Diese Zweige sind
TOPOLOGY-Disambiguierung, also nichts, was die Übersetzung liest: der Parser gibt
ihnen einen synthetischen Schlüssel (`?0`, kollidiert mit keinem Feldnamen)
statt die wohlgeformte WURZEL mit ihnen wegzuwerfen — daran starben vorher 9 der
22 Kanten-Queries von #416. Ein Wert, der so entsteht, wird als **undekodiert**
gemeldet und nie als Onshape-Form ausgegeben.

### Vorher/Nachher, gemessen

| Dokument | Blend-Features vorher | nachher | Meldungen vorher → nachher |
|---|---:|---:|---|
| #416 | 6 (2 Fasen + 4 Fillets) | **7** (2 + 5) | 14 → 16 |
| #414 | 9 | **8** | 22 → 24 |
| #415 | 2 | **0** | 37 → 39 |
| #419 | 1 | **0** | 212 → 216 |
| #409 | 0 | 0 | 22 → 22 |

Neu entstehen in #416 zwei Fillets aus der **`SWEPT_EDGE`-Form**
(`qCommonEdge(qWall(6, 10), qWall(6, 17))`) und in #414 eines aus einer
`INTERSECT`-Kante, deren beide Flächen zu **verschiedenen** Sweeps gehören — der
Marker-Dekoder nahm für beide dieselbe Operation.

Weg fallen ausschließlich **erfundene** Kanten. In #414 hatten `Fillet 1` und
`Fillet 2` die **identische** Query `qCommonEdge(qCreatedBy(2, CAP_TOP),
qWall(2, 5))`; in #419 und #415 nannte der alte Dekoder für eine Fläche einer
Muster-KOPIE die entsprechende Kante des Musters-ORIGINALS. Das ist genau der
Fall, den #425 verbietet: eine still geratene Zuordnung, die die falsche Kante
verrundet, ist schlimmer als eine gemeldete Lücke.

### Was Klasse F NICHT gelöst hat

`Chamfer 1` in **#409** dekodiert jetzt korrekt als `SWEPT_EDGE` über
`dE9VSfqolyPg.bottom` × `exVl27xLp2IA` — und scheitert eine Stufe später daran,
dass `Extrude 14` (`FJaU8k5c579SOul_18`) gar nicht importiert. In **#419** gilt
das für **29 der 102** Kanten-Queries: die Form ist verstanden, der Sweep fehlt.
Klasse F ist damit als *Dekoder*-Lücke geschlossen und als *Folgefehler* der
Klassen A–C sichtbar geworden — der Fortschritt steht in der Fehlermeldung, nicht
in der Körperzahl.

**Offen bleibt** die topologische Historie: `MERGE`, `SPLIT`, `COPY`,
`BLEND_EDGE` (zusammen 60 der 206, plus 61 `INTERSECT`-Kanten mit einer solchen
Fläche) brauchen einen Namen für „die Kante NACH einer Boolean/einem Muster". Das ist dieselbe Frage wie Klasse A und gehört in
den Kernel, nicht in den Übersetzer.

## Update 2026-07-27 (7) — Klasse A: die 7 restlichen `qWall`-Fehlschläge gemessen und behoben; #416 gehört NICHT dazu

Der Rest-Umfang von Klasse A (#424) waren zuletzt **#416 Sketch 6 samt Fasen und
Fillets** sowie die **7 verbliebenen `qWall(…) löst auf 0 Flächen auf`** in #419.
Beides ist jetzt auseinanderdividiert — und es sind **zwei verschiedene Dinge**.

### #419: 7 von 7 = positionaler Fallback, eine einzige Ursache

Alle sieben Meldungen hingen an **Feature 24**, einem `qRegions(6, [1, 2])`.
Der Flächen-Dump trennt die beiden Ticket-Kandidaten eindeutig:

| Feature | Profil | Wand-Subs vorher |
|---|---|---|
| 23 | `qRegions(6, [4, 7])` | 103, 106, 109, 112, 115, 130, 148 (Entity-Ids) |
| 24 | `qRegions(6, [1, 2])` | **0, 2, 3, 5** (positionell) |
| 25 | `qRegions(6, [4, 5])` | **1, 2, 2, 6** (positionell, mit Dublette) |

Es ist **nicht** die Flächensegmentierung (die These des Tickets, in Update (4)
schon einmal widerlegt), sondern das **Profil**: `resolve_profile_query` legte
die geschlossenen Umrisse der ausgewählten Regionen übereinander. Grenzen zwei
Regionen aneinander, steht ihre gemeinsame Randlinie damit **zweimal** in der
Kontur. `wall_sub_table` verwirft eine Tabelle mit doppeltem Sub — zwei Wände mit
gleichem Sub würden zu einer Branch kollabieren — und fällt auf 0, 1, 2, … zurück.
Eine Entity-Id wie 121 kann dort nie vorkommen. Regionen 4 und 7 (Feature 23)
grenzen nicht aneinander, deshalb funktionierte dieses eine.

Ein **zweiter**, davon unabhängiger Defekt trat beim Beheben zutage:
`g1_group_segments` läuft linear und schließt das letzte Segment nie ans erste
zurück. Fiel die Naht der geschlossenen Kontur mitten in eine glatte Wand, zerfiel
diese in **zwei** Randgruppen — und stammten beide aus derselben Skizzenkurve,
wollten beide denselben Sub. Wo die Naht lag, entschied bisher der Startpunkt der
DCEL-Traversierung, also ein Zufall der Implementierung.

**Behoben in v0.39.1** (`sketch_arrangement::arrange_union` +
`rotate_to_g1_break`): die Vereinigung mehrerer Regionen bekommt ihre eigene
Außenkontur über Halbkanten-Cancellation, und jede geschlossene Kontur beginnt an
einem G1-Knick.

Gemessen an den Fixtures:

| Dokument | vorher | nachher |
|---|---|---|
| #419 Meldungen | 216 | **192** |
| #419 Features in KNOWN_GAPS | 118 | **107** |
| #419 Körper | 10 | **12** |
| #419 `qWall` bindet 0 | 7 | **5** (auf Feature 31, das es vorher nicht gab) |
| #409 Lücken | 19 | **17** (`Mirror 2` + sein Mate-Connector importieren) |

Neu importierend in #419: `Boolean 1`, `Boolean 6`, acht Extrusionen, eine
Bohrung. `fillet_edge` kehrt zurück — wie in Update (6) vorhergesagt: `Fillet 16`
löst als **vier verschiedene** Kanten-Queries auf, eine je Ecke von Extrude 24.

### #416: 0 von 5 sind Klasse A — die Meldungen haben drei andere Ursachen

`Sketch 6` samt seiner Fasen und Fillets gehört **nicht** in Klasse A. Der Dump
(`resolveQuery` auf jede Teil-Query einzeln) zeigt:

| Meldung | Teil-Query | bindet | Ursache |
|---|---|---:|---|
| Sketch 6 · Mate connector, Fillet 2 (3×) | `qCreatedBy(3, CAP_TOP)` | **0** | Feature 3 hat im gewobenen B-Rep **gar keine Deckfläche** — nur sechs WALLs. Seine Kappen sind von Extrude 2 und Extrude 5 aufgezehrt. |
| Fillet 2 | `qWall(3, 13)`, `qWall(3, 19)` | **0** | Entities 13 und 19 liegen nicht auf dem Rand der Region, die FieldDraft extrudiert (f3s Wände sind 10, 16, 26, 44, 50, 53). `qWall(3, 16)` bindet dagegen sauber. |
| Fillet 4 | `qCreatedBy(6, CAP_BOTTOM)` → 1, `qWall(13, 4)` → 1 | **0** | Beide Seiten binden je genau eine Fläche — die liegen aber auf **verschiedenen Körpern** (1 und 2), teilen also keine Kante. |

Die Wand-Sub-Tabelle von Feature 3 ist **stabil und entity-basiert**. Es fehlt
kein Name; es fehlt die Fläche bzw. der gemeinsame Körper. Das sind
Geometrie-/Docking-Fragen und gehören nicht in dieses Ticket. #416s Szenen-AABB
weicht ohnehin deutlich von Onshapes ab (Größen 102 × 67 × 105 gegen 73 × 34 × 108),
was auf eine Ursache **vor** der Benennung deutet.

### Klasse A (#424) — Stand

- #409: erledigt (Update 5, `keep_tools`).
- #419: von 12 → 7 → **5** Meldungen; die 5 sitzen auf einem Feature, das erst
  durch diesen Fix entsteht, und sind noch nicht gemessen.
- #416: **kein** Klasse-A-Fall.

### Klasse „abgeleitete Geometrie" (#429) — am Kernel gemessen

| Onshape-Form | Anzahl | FieldDraft, gemessen |
|---|---:|---|
| `MERGE` (Kante) | 20 | **Kernel kann es**: `qCommonEdge(qCreatedBy(a), qCreatedBy(b))` löst die Nahtkanten mit stabilem `id_hash` auf. Lücke liegt im **Übersetzer**. |
| `COPY` | 22 | **Kein Name.** Drei Kopien eines Quaders tragen identische Branch-Ids UND identische `id_hash` — `BranchId::from_feature` faltet die hierarchische Klon-Id auf ihren Wurzel-Vorfahren, `spatial_index` wird pro B-Rep vergeben und ist bei je einem Körper pro Kopie überall 0. Das verletzt die Eindeutigkeitszusage von `qFace(hash)`. |
| `BLEND_EDGE` / `BLEND_FACE` | 14 | Name ist entworfen (`BlendProvenance.branch` = Fillet-Id, Sub 0), die **Fläche fehlt**: an einem verrundeten Absatz liefert `qCreatedBy(fillet, BLEND)` 0. |
| `SPLIT` | 4 | nur `spatial_index` — eine Sortierung nach Schwerpunkt, also positionell. `qAdjacentTo` ist als Disambiguator vorhanden, der Übersetzer-Zweig fehlt. |
| `INTERSECT` mit solcher Fläche | 61 | Folgefehler der drei Zeilen darüber. |

Entwurf, wie diese Namen entstehen sollen: **`docs/DERIVED-NAMING-PLAN.md`**.

## Update 2026-07-28 — `R<n>` war nie eine String-Referenz: der BTON-Dekoder ist vollständig (#430)

### Was gemessen wurde

Update (6) hielt fest, dass **41 der 699 BTON-Payloads** der Fixtures nicht
sauber dekodieren: dort landete etwas in einem Map-Schlüssel-Slot, das kein
Onshape-Feldname ist, und der Parser vergab einen synthetischen `?n`-Schlüssel.
Vermutet wurde ein „dritter Verweis-Raum" für geteilte Unterstrukturen. Die
Vermutung war halb richtig und in der entscheidenden Hälfte falsch: es gibt
keinen dritten Raum, es gab von Anfang an **den falschen zweiten**.

`R<n>` referenziert **keinen String, sondern einen Wert** — den n-ten Wert, den
das Dokument fertiggestellt hat. Beide Lesarten laufen anfangs im Gleichschritt
(solange nur Strings fertig werden), und genau deshalb hielt die
String-Tabellen-Lesart 656 der 697 Payloads. Sie bricht auseinander:

- am ersten **zusammengesetzten String** — `S12.6$F…_18wireOp` sind zwei
  Pool-Einträge, aber **ein** Wert;
- an jedem **fertiggestellten Container** — jede Map und jedes Array bekommt
  eine Wert-Nummer, ein String-Pool sieht davon nichts.

Die vollständigen Regeln, empirisch an allen 697 Payloads gegen die harte
Bedingung „ein Map-Schlüssel ist ein Onshape-Feldname" kalibriert:

| Raum | Was hineinkommt | Wer indiziert |
|---|---|---|
| **String-Pool** | jeder ausgeschriebene String: Klassennamen (`B<len>$Name`), jeder literale Teil eines mehrteiligen Strings, danach der zusammengesetzte | `C<n>` (Klasse), `E<n>` (String-Wert), negative Teil-Längen (`S-13.5$right`) |
| **Wert-Tabelle** | jeder fertige Wert: String, Zahl, Map, Array — Container erst nach ihrem letzten Element | `R<n>` |

Was **keine** Wert-Nummer bekommt: Klassen-Intros/-Referenzen (`B`/`C` sind
transparent), Rück-Referenzen selbst (`R<n>` wiederholt einen Wert, es entsteht
kein neuer) und **Booleans**. Letzteres ist kein Sonderfall, sondern
Ökonomie: `T`/`F` ist ein Zeichen, ein Rückverweis nie kürzer — der Encoder
interniert sie gar nicht erst. Zahlen (`D<hex>`) dagegen schon. Das ist nicht
geraten: die #414-`startBoundEntity`-Query hängt genau an dem einen `isStart:
false` dazwischen, und mit interniertem Boolean rutscht ihr `entityType`-Slot
auf ein Array.

`E<hex>` war bis hierher als „geteilte Unterstruktur, nicht rekonstruierbar"
geführt. Es ist schlicht eine **Pool-Referenz aus einer Wert-Position**. Alle
acht Vorkommen im Korpus stehen im Wert-Slot von `instanceName` und lesen sich
als `1` — dieselbe Zeichenkette, die die übrigen 67 `instanceName`-Werte
ausschreiben.

### Ergebnis

| | vorher | nachher |
|---|---:|---:|
| BTON-Payloads ohne synthetischen Schlüssel | 656 / 697 | **697 / 697** |
| Payloads ohne unauflösbare Rück-Referenz | 372 / 697 | **697 / 697** |
| saubere `queryType`-Schlüssel im Korpus | 4431 | **7223** |
| `operationId` mit vollem `F…_n.op` | — | **7210 / 7223** (Rest: Datum-Ebenen `Top.planeOp`, ein Mate-Connector) |
| Kanten-Queries auf ein Flächenpaar abgebildet | 78 / 206 | **99 / 206** |

Jeder Map-Schlüssel des Korpus ist jetzt ein echter Onshape-Feldname (19
verschiedene), jeder `queryType` SCREAMING_CASE, jeder `entityType` eines von
`EDGE`/`FACE`/`VERTEX`/`BODY`, jeder `historyType` `CREATION`.

Die 21 zusätzlichen Kanten sind **alle** `INTERSECT`: deren Flächen-Slots lasen
vorher zu 37 von 162 Müll — `bottom`, `left`, `3`, `opBoolean`, `opRevolve`,
eine Feature-Id — oder gar nichts. Sie lesen jetzt `CAP_FACE` (50 → 72) und
`SWEPT_FACE` (36 → 53). Die Zahlen für `COPY` (19), `MERGE` (12) und `SPLIT` (6)
in Flächen-Position sind **unverändert**: der Dekoder-Fix bringt **keine** neue
abgeleitete Geometrie ans Licht, die #429 nicht ohnehin schon zählt.

Ein Nebeneffekt räumt eine Annahme weg, die #425 noch stehenlassen musste: eine
wiederholte `operationId` ist der geteilte `Id[["F…_18.wireOp"]]`-Wert selbst,
nicht das nackte Suffix `wireOp`. Der Ursprungs-Sketch musste bisher vom
Eintrag DAVOR geerbt werden — die Vererbung ist ersatzlos entfallen.

### Was das am Import ändert — wenig, und das ist der Befund

Ende-zu-Ende über alle sechs Fixture-Dokumente: **+1 importierte Verrundung**
(#339 `Fillet 1`) und **3 Meldungen weniger** (#339 8 → 6, #415 39 → 38). Die
übrigen 20 neu übersetzten Kanten scheitern eine Stufe später, an derselben
Kernel-Lücke, die die #416-Suite als `it.fails` festhält: `qWall` bindet auf
verschmolzenen Mänteln keine Fläche, und `qCommonEdge` braucht zwei gebundene
Flächen auf **einem** Körper.

Damit ist die Reihenfolge belegt statt behauptet: der Dekoder ist nicht mehr
der Engpass. Die nächste Stufe ist die Flächen-Bindung im Kernel, nicht das
Lesen der Query.

### Offen

Der EBENEN-Pfad (`extractPlaneRef`) liest weiterhin den **flachen** Token-Strom
und nicht den Baum — eine Heuristik aus der Zeit, als `parseBton` noch Lücken
hatte. Sie funktioniert (alle Ebenen-Tests grün), steht aber jetzt ohne Grund
neben einem vollständigen Parser. Umstellung wäre eigene Arbeit mit eigenen
E2E-Pins.

## Update 2026-07-28 (2) — Klasse „abgeleitete Geometrie": `MERGE`, `SPLIT` und `BLEND_EDGE` sind übersetzt (#429, v0.43.0)

Update (6) zählte die Kanten-Formen und ließ **60 der 206** als topologische
Historie stehen (`MERGE`, `SPLIT`, `COPY`, `BLEND_EDGE`), plus 61
`INTERSECT`-Kanten mit einer solchen Fläche. Drei der vier sind jetzt
übersetzt. Der Plan dahinter ist `docs/DERIVED-NAMING-PLAN.md` (Schritte 3–5).

### Eine Erkenntnis, drei Formen

Es waren nie drei Aufgaben. `MERGE` und `SPLIT` sind Flächen, die aus **anderen
Flächen** bestehen; sobald der Flächenbegriff des Dekoders ein Baum statt einer
flachen Aufzählung ist, fällt `BLEND_EDGE` als Nebenprodukt an (seine
Nachbarfläche ist in einem Drittel der Fälle selbst ein `MERGE`).

| Onshape | FieldDraft |
|---|---|
| `BLEND_FACE` | `qCreatedBy(f, BLEND)` — Rolle seit v0.41.0 |
| `BLEND_EDGE` | `qCommonEdge(qCreatedBy(f, BLEND), <blendedInto>)` |
| `MERGE` (Fläche) | `qUnion(a, b)`, links gefaltet für n > 2 — **neues Query-Vokabular** |
| `SPLIT` (Fläche) | `qAdjacentTo(basis, n₁) … nₖ`, gekettet |

Dazu ein Zweig, den der Plan nicht vorgesehen hatte: ein `SWEPT_FACE` mit
**mehreren** Ursprüngen ist derselbe Fall wie `MERGE`. In #414 ist
`v7UgcE38VD0I.bottom` der einzige Ursprung der einen Wand und einer von dreien
der anderen — drei kollineare Profilkurven, eine Onshape-Fläche. Bisher als
„genau eines erwartet" abgelehnt, jetzt `qUnion` der drei Wände.

Alle Kombinatoren sind **alles-oder-nichts**: fehlt einem Partner der Name,
wird die ganze Fläche abgelehnt statt aus dem Rest übersetzt (§ „Warum nicht
raten" des Plans).

### Zahlen — Dekoder, alle acht Fixtures

| | vorher | nachher |
|---|---:|---:|
| Kanten-Queries mit Flächenpaar | 101 | **124** |
| `BLEND_EDGE` | 0 / 13 | **13 / 13** |
| `INTERSECT` | 38 / 79 | **48 / 79** |
| `MERGE`-Flächen mit Namen | 0 / 27 | **22 / 27** |
| `SPLIT`-Flächen mit Namen | 0 / 8 | **6 / 8** |

Keine vorher übersetzte Query wurde schlechter.

### Zahlen — Ende zu Ende (#419): unverändert, und das ist der Befund

190 Meldungen auf 107 Features, 12 Körper, 49 Feature-Calls — vorher wie
nachher. Was sich verschiebt, ist die **Klasse** der Meldungen:

| Meldungsklasse | vorher | nachher |
|---|---:|---:|
| „FieldDraft hat für diese Form keinen Namen" | 26 | **14** |
| „Query nennt ein Feature, das nie importiert" (Folgefehler) | 35 | **47** |

`Fillet 9/12/16` und `Chamfer 4/10` scheitern jetzt an ihren Extrudes statt am
Dekoder — dieselbe Bewegung wie in #425, aus demselben Grund: der Engpass liegt
seit #424 flussaufwärts.

### Offen

`COPY` ist die letzte Onshape-Form ohne FieldDraft-Namen (52 Wurzel-Queries,
plus 7 `MERGE`/`SPLIT`-Flächen und 24 `INTERSECT`-Kanten mit `COPY`-Partner).
Sie zerfällt in die **Muster**-Kopie (`qInstance(i, …)` steht seit v0.40.0
bereit, der Übersetzer-Zweig fehlt) und die **Boolean**-Kopie — dieselbe Fläche
auf dem Ergebniskörper, mit gedrehter Normalen, wie sie `Thicken 6` in #419
referenziert (`docs/research/440-thicken6-face-provenance.md`). Die zweite
braucht einen Körper-Selektor im Query-Vokabular; `qUnion`/`qAdjacentTo`
kombinieren Namen innerhalb eines Körpers und decken sie nicht ab.

Der EBENEN-Pfad (`extractPlaneRef`) liest weiterhin den flachen Token-Strom —
absichtlich: eine Ebene braucht nur EINE Fläche eines verschmolzenen Paares,
und `thicken` verlangt sogar genau eine. Die neue Flächen-Übersetzung ist über
`decodeFaceRef` separat messbar, statt den Ebenen-Pfad umzustellen.

---

## Update 2026-07-29 — Klasse A, letzter Rest: eine Sketch-Kurve darf MEHRERE Waende erzeugen (#424, v0.43.1)

### Was gemessen wurde

Drei offene Punkte aus dem #424-Kommentar vom 2026-07-27, jeder mit einem
Subs-und-Rollen-Dump aller BRep-Flaechen des betroffenen Features plus
Einzelaufloesung jeder Teil-Query.

**#419 Feature 31 (`Extrude 20`) — die Titel-Hypothese ein drittes Mal
widerlegt.** Die Wand-Subs sahen nach Entity-Ids aus (16, 17, 19, 30, 31, 34 …)
und waren doch **positionell**: das Feature hat 40 Randgruppen, und die Werte
liegen alle unter 40. Ein neuer Diagnosepfad
(`sketch::wall_sub_table_diag` → `WallSubRefusal`) nennt den Grund direkt:
`DuplicateName(8312)` — Kontur 2, Sketch-Entity 120, **zweimal**. `Extrude 20`
vereinigt elf Regionen zu einem Umriss, den eine lange Linie an zwei getrennten
Abschnitten begrenzt. Dieselbe Ursache, andere Gestalt, bei #409s Feature 8
(`Extrude 4`, `wall_subs: None`, Subs 0…7): zwei DISJUNKTE Leisten, beide von
denselben zwei langen Linien eingefasst.

**#419 Sketch 11 (`qCreatedBy(39, CAP_TOP)` bindet 4) — kein Namensdefekt.**
`Extrude 25` extrudiert vier getrennte Kreise (`qRegions(38, [1, 2, 3, 4])`),
also gibt es vier Deckflaechen, alle bei z = 12,1 mm. Onshapes eigene Query ist
ein **`MERGE` zweier `CAP_FACE`s** desselben Extrudes — Onshape faltet
koplanare Kappen zu einer Flaeche, FieldDraft nicht (kein impliziter Merge).
Die vier Treffer sind also richtig; was fehlt, ist eine Ebenen-Aufloesung, die
N **koplanare** Flaechen als EINE Ebene akzeptiert. Das ist Uebersetzer-/
Importer-Arbeit und steht unter „Offen".

### Der Fix: das Wand-Vorkommens-Ordinal

Ein Wandname ist jetzt `occurrence << SUB_BITS | sketch_entity`
(`BranchId::occurrence`, 5 Bit). Der Rang zaehlt die Waende, die DIESE Kurve
erzeugt hat, in der kanonischen DFS-Ordnung des Profils. Vorher verwarf
`wall_sub_table` bei jeder Wiederholung die **ganze** Tabelle — zu Recht, zwei
Waende mit einem Sub waeren eine Branch —, und damit verloren auch die sechs
Waende ihren Namen, deren Kurve eindeutig ist.

Damit der Rang eine Eigenschaft der GEOMETRIE ist und nicht der Traversierung,
ordnet `sketch_arrangement::assemble` die Konturen (und die Loecher) jetzt nach
`loop_key` — aufsteigend `aabb.min.y`, `aabb.min.x`, Flaeche, dieselbe Regel wie
`atomic_faces`. Innerhalb einer Kontur setzt `rotate_to_g1_break` seit v0.39.1
am G1-Knick an.

Verworfen wurde ein **Kontur**-Ordinal (der erste Entwurf): es trennt #419s
Wiederholung innerhalb EINER Kontur gar nicht, und es ist der instabilere Name —
eine eingefuegte Kontur nummeriert jede spaetere um. Details und die
Layout-Begruendung stehen in `docs/DERIVED-NAMING-PLAN.md`, Schritt 6b.

`NAMING_SCHEMA_VERSION` 4 → **5**: das neue Feld sitzt zwischen `instance` und
`sub`, damit Branch-Woerter feature-major sortiert bleiben (die
Koinzidenz-Klassen des Weavers entscheiden auf dieser Sortierung), und schiebt
`instance`/`feature` um fuenf Bit hoch — also aendert sich jeder `id_hash`.

### Zahlen, gemessen

| | vorher (v0.43.0) | nachher (v0.43.1) |
|---|---:|---:|
| #409 Meldungen | 22 | **20** |
| #409 Luecken-Namen | 19 | **17** |
| #419 Meldungen / Namen / Koerper | 190 / 107 / 12 | 190 / 107 / 12 |
| #419 `qWall` bindet 0 | 3 | **2** |

In #409 verlassen `Mirror 2` und sein Mate-Connector die Luecken-Liste:
`qWall(8, 45)` — die Linke des Rahmens, die nur EINE Leiste einfasst — bindet
wieder, weil die Tabelle nicht mehr an den beiden langen Linien scheitert.

In #419 bewegt sich die Namensliste nicht, die Ursachen darunter aber schon:
`qWall(31, 32)` und `qWall(31, 26)` binden jetzt, und `Sketch 14` scheitert eine
Stufe spaeter (Onshapes Ebene steht **schief** zur aufgeloesten Wand) statt an
einer Wand ohne Namen.

### Nebenbefund, mitrepariert: eine Naht-Fuellung, die nichts reparieren kann

Der Fortschritt loeste in #419 einen **Stall** aus: `Thicken 7` bekam seine
Flaechen und der Import stieg von 37 s auf **1151 s**. Gemessen liegt das
weder am Namen noch am Thicken, sondern am Naht-Auffuellen des hybriden
Seed-Weavers: die 0,12 mm dicke Schalen-Subtraktion hinterlaesst **183**
unaufloesbare Branch-Paare, alle in einem 0,12 x 4,7 x 10,1 mm duennen Bereich,
den der Fuell-Octree mit 0,32 mm Zellen ueberschreitet. 209 s Octree-Lauf, der
per Konstruktion nichts findet — dreimal (Thicken, das Extrude danach, die
Bohrung danach).

`weave_branch_seeds_hybrid` verweigert die Fuellung jetzt, wenn der Naht-Bereich
duenner ist als die Zelle, die ihn abtasten wuerde — dasselbe Argument, mit dem
#426 die Ganzkoerper-Pflasterung abgeschafft hat, einen Schritt weiter gefuehrt.
Die Nahtkanten sind da bereits verworfen, der Koerper behaelt also die ehrliche
Teil-Topologie des Seed-Weavers; verweigert wird nur der Anschein einer
Reparatur. #419 importiert wieder in **37 s**, `Thicken 7` in 2,0 s statt 374 s.

### Offen

1. **`qCreatedBy(op, CAP_TOP)` bindet N koplanare Kappen** (#419 Sketch 11). Der
   Ebenen-Pfad des Importers verlangt genau eine Flaeche. Richtig waere: sind
   alle Treffer koplanar, ist die EBENE eindeutig — das ist keine
   Mehrdeutigkeit, sondern Onshapes `MERGE` in unserer Zerlegung.
2. **`Thicken 7` bleibt in #419s Luecken-Liste** — zwei seiner fuenf Flaechen
   (`qWall(31, 29)`, `qWall(31, 11)`) binden weiterhin nichts; die Entities
   liegen nicht auf dem Rand des vereinigten Profils.
3. **#416** ist aus Klasse A herausgemessen (Update 2026-07-27 (7)) und hat ein
   eigenes Ticket.

---

## Update 2026-07-29 (2) — koplanare Kappen sind EINE Ebene; die zwei `Thicken-7`-Waende sind ein Arrangement-Befund (#443, v0.43.2)

### 1. `qCreatedBy(op, CAP_TOP)` bindet vier Kappen — gemessen, nicht geschaetzt

Der Ebenen-Pfad des Importers verlangte genau eine Flaeche. Gemessen wurde,
was die vier Treffer von `#419 Sketch 11` GEOMETRISCH sind, mitten in der
Historie (also in dem Zustand, in dem der Importer die Query prueft):

| Fläche | Normale (Welt) | Versatz d = n·p (mm) |
|---|---|---:|
| 4830837308608304756 | (−1,000000000; −0,000000181; −0,000000251) | 74,627293084 |
| 16358552656622588677 | dieselbe auf 9 Nachkommastellen | 74,627294789 |
| 9439523930927320982 | dieselbe auf 9 Nachkommastellen | 74,627291379 |
| 2520495205232053287 | dieselbe auf 9 Nachkommastellen | 74,627293084 |

Alle vier gehoeren zu Feature 39 und liegen im **selben Koerper** (11).
Normalen-Streuung: unterhalb der neunten Nachkommastelle. Versatz-Streuung:
**3,41e-6 mm**.

Die Schwellen sind daher nicht geraten, sondern die zwei, mit denen derselbe
Importer eine Ebene ohnehin beurteilt: `alignSketchFrame` nennt eine Ebene ab
|1 − n·n₀| > 1e-6 „schief" und ab |Δd| > 1e-3 mm „daneben". Die gemessene
Versatz-Streuung liegt **293-fach** unter der Offset-Schwelle, die
Normalen-Streuung um Groessenordnungen unter der anderen. Antiparallel
(n·n₀ ≈ −1) zaehlt NICHT als koplanar: geometrisch dieselbe Ebene, aber ein
gespiegelter Sketch-Frame.

**Gespeichert wird die kanonische Element-Query EINER dieser Flaechen**, nicht
die Rollen-Query. Das ist kein Bequemlichkeits-Kurzschluss, sondern
Determinismus: der Sketch-Frame hat seinen URSPRUNG im Schwerpunkt der
aufgeloesten Flaeche. Kernel (`resolve_face_frame`: erste Flaeche des ersten
passenden Koerpers) und Web-Spiegel (`resolvePlaneFrame`: erster Treffer ueber
alle Koerper) muessen dieselbe waehlen, sonst laege der Sketch um den Abstand
zweier Kappen-Schwerpunkte verschoben IN seiner Ebene. Eine Flaeche zu benennen
macht die Wahl fuer beide Seiten identisch — dieselbe Form, die `thicken`
ohnehin verlangt, und sie heilt ueber ihren Fingerprint.

### 2. `Thicken 7`: die beiden Kurven liegen bei ONSHAPE auf dem Rand, bei uns nicht

Zu messen war laut Ticket, welche Regionen Onshape fuer `Extrude 20` wirklich
waehlt und wo die Kurven liegen. Beides ist offline aus der Fixture
entscheidbar.

**Onshape waehlt 14 Regionen** (nicht elf — elf ist unsere Abbildung davon).
Zaehlt man die Rand-Stuecke **getrimmt** (Kurve + beide Trimm-Vertices, denn
dieselbe lange Linie begrenzt ein Profil oft an mehreren getrennten
Abschnitten), dann gilt: ein Stueck, das GENAU EINE gewaehlte Region nennt, ist
Rand; eines, das zwei nennt, ist innen.

| Kurve (Onshape-Id, unser Entity) | Stuecke | davon Rand |
|---|---:|---:|
| `fhLiiJlX79hL.0` (29) | 4 | **4** |
| `VtFQu4lRFe6v.0` (11) | 5 | **3** |
| `3HvpPu0qDC3j.0` (26) | 4 | 4 |
| `1KZGONLBXaZf.0` (17) | 4 | 4 |

Bei Onshape hat `Extrude 20` also auf ALLEN vier Kurven Wand. Bei uns nur auf
zweien — und der Grund steht in unserer eigenen Anordnung.

Die vier Kurven sind die vier Seiten EINES Rechtecks (38 × 38 mm,
x ∈ [−35,989; 2,011], y ∈ [−9,95; 28,05] in Sketch-4-Koordinaten). Es liegt in
einem zweiten, 1,5 mm groesseren Rechteck `Ut9pwxjnnLNt` (41 × 41 mm). Unser
Imprint-Arrangement macht aus dem 1,5 mm breiten RING dazwischen **eine
einzige** atomare Flaeche: Index 1, Flaecheninhalt **237,0 mm²** = 41² − 38².
Und diese Ringflaeche ist der Seed von Onshapes Region 0, gehoert also zum
Profil.

Beidseitige Probe entlang jeder Kurve (±0,02 mm, 39 Stationen; Faces mit `*`
sind im Profil):

    Entity 29 (links):  1* | 2*, 8*, 10*      → beide Seiten im Profil, KEINE Wand
    Entity 11 (oben):   1* | 10*, 8*, 2*      → beide Seiten im Profil, KEINE Wand
    Entity 26 (unten):  2*, 4*, 5 | 1*        → Face 5 nicht gewaehlt → Wand
    Entity 17 (rechts): 5, 2*, 9* | 1*        → Face 5 nicht gewaehlt → Wand

Der Ring beruehrt also alle vier Innenkanten von aussen; weil er im Profil ist
und die Innenflaechen auch, verschwinden die Kanten links und oben im Inneren
der Vereinigung. Unten und rechts ueberlebt eine Wand nur, weil dort ein
Stueck weit die nicht gewaehlte Flaeche 5 (4,8 mm²) anliegt.

Onshape zerschneidet denselben Ring; seine Region 0 ist nur ein Teil davon
(ihr Rand nennt drei Seiten des aeusseren Rechtecks und Stuecke NUR der
Innen-Oberkante). Der Rest des Rings gehoert nicht zur Auswahl — deshalb
bleiben dort die Innenkanten Rand.

Dieselbe Groebe zeigt sich noch einmal in der Abbildung selbst: **14
Onshape-Regionen → 11 unserer Flaechen**, mit drei Kollisionen (Face 6 ←
Regionen 2 und 13, Face 7 ← 3 und 12, Face 13 ← 4 und 11) und **zwei
entarteten Flaechen mit Flaecheninhalt 0** (Index 3 und 11) in unserer
Anordnung. Kein `conflict`, kein `unknownCurve` — jede Region loest auf genau
eine unserer Flaechen auf, es sind nur zu wenige.

**Damit ist die Ursache NICHT im Importer.** Er darf den Ring weder weglassen
(Onshapes Region 0 ist ausgewaehlt) noch aufteilen (unsere Anordnung hat dort
nichts zu teilen). Was fehlt, ist die Imprint-artige Regionen-Subdivision im
Sketch-Regionenmodell des Kernels, die diese Datei seit #412 unter „Offene
Kernel-Arbeit" fuehrt: Flaechen an inneren Sehnen/Kurven wirklich trennen statt
per SDF-Union-Flood-Fill zu fluten. Eigenes Ticket.

### Zahlen, gemessen (Ende zu Ende, #419)

| | vorher (v0.43.1) | nachher (v0.43.2) |
|---|---:|---:|
| Meldungen | 190 | **179** |
| Feature-Namen in `KNOWN_GAPS` | 107 | **98** |
| Koerper | 12 | **15** |
| Import-Dauer | 37,1 s | **40,9 s** |

Neu importierend: `Sketch 11`, `Sketch 12`, `Boolean 3`, `Extrude 5`,
`Extrude 11`, `Extrude 14`, `Extrude 15`, `Extrude 45`, `Extrude 47`. Kein
einziger Name kommt HINZU.

### Offen

1. **`Thicken 7`** (und alles, was am selben Ring haengt): Regionen-Subdivision
   im Kernel, siehe oben. Solange sie fehlt, binden `qWall(31, 29)` und
   `qWall(31, 11)` nichts.
2. **#416** ist aus Klasse A herausgemessen (Update 2026-07-27 (7)) und hat ein
   eigenes Ticket.

## Update 2026-07-29 (3) — kein Arrangement-Defekt: Onshapes Bogen-Winkel gehoeren seinem EIGENEN Frame (#444, v0.43.3)

### 1. Die Hypothese aus Update (2) war falsch — gemessen, nicht argumentiert

Update (2) schloss aus der Region-Abbildung von `Extrude 20`, es fehle die
Imprint-Subdivision im Kernel: Onshape zerschneide den 1,5 mm breiten RING
zwischen dem 41er- und dem 38er-Rechteck, wir machten daraus eine einzige
atomare Flaeche. Nachgemessen ist beides falsch.

Der Ring ist bei uns genau EINE Flaeche (237,0 mm² = 41² − 38²) — und bei
Onshape auch. Die sechs Rand-Kanten, die eine Onshape-Region-Query nennt, sind
kein vollstaendiger Rand, sondern eine DISAMBIGUIERUNG; aus ihnen zu schliessen,
wo Onshape schneidet, traegt nicht. Das Arrangement des Kernels
(`sketch_arrangement.rs`) planarisiert korrekt und hat in diesem Fall nichts
uebersehen.

### 2. Was wirklich fehlte: `clockwise` + `xDir`/`yDir` an `BTCurveGeometryCircle`

`Sketch 4` zeichnet vier gleiche Schluesselloch-Aussparungen: eine gezeichnet,
drei ueber Onshapes Spiegelung erzeugt (`…MirrorCS`). Onshape legt einen Bogen
als Winkel in seinem EIGENEN Frame ab; `xDir`/`yDir` sind dessen x-Achse,
`clockwise` seine Haendigkeit. Eine Spiegelung LAESST die Parameter stehen und
kippt den Frame. Wir lasen `startParam`/`endParam` als Skizzenwinkel — fuer den
Default-Frame (+x, ccw) richtig, fuer jede gespiegelte Kopie falsch.

Beispiel, gemessen an `04uIDdMa1QEg1.MirrorCS` (r = 4,7 mm um (−2,689 | 23,35),
`xDir` = (−1 | 0), `clockwise` = true, Parameter −π/2 … 0):

| | Startpunkt | Endpunkt |
|---|---|---|
| gelesen als Skizzenwinkel (bis v0.43.2) | (−2,689 \| 18,65) | **(+2,011 \| 23,35)** |
| ueber den eigenen Frame (ab v0.43.3) | (−2,689 \| 18,65) | **(−7,389 \| 23,35)** |

Der wahre Endpunkt ist genau der Punkt, an dem die naechste Linie
(`04uIDdMa1QEg3.MirrorCS`) beginnt. Falsch gelesen liegt der Bogen 9,4 mm
daneben, die Tangenten treffen ihn nicht mehr, und der geschlossene Umriss der
Aussparung faellt auf.

Betroffen sind genau die Dokumente mit gespiegelten Boegen — ueber alle
Fixtures gezaehlt: `#419` (`Sketch 4`, 6 Boegen) und `kabelbox` (`Sketch 6`, 6).
**#416 hat einen einzigen Bogen, und der hat den Default-Frame** — #442 Punkt 2
(`qWall(3, 13)` / `qWall(3, 19)` binden 0) ist von dieser Reparatur nicht
beruehrt und hat weiterhin die in Update 2026-07-27 (7) gemessene Ursache.

### 3. Repariert an drei Stellen, die zusammen bleiben muessen

- `onshapeSketch.ts` — neue Funktion `arcWorldSpan` (Frame → Skizzenwinkel),
  benutzt von der Sketch-Uebersetzung; der Umlaufsinn kommt aus `clockwise`
  statt aus der Annahme „Onshape-Boegen laufen ccw“.
- `onshapeImport.ts` — `sketchEntityVertices` liest denselben Bogen ueber
  dieselbe Funktion. Ein Unit-Test haelt die beiden im Gleichschritt: driften
  sie, sucht die Region-Aufloesung eine Flaeche an einer Stelle, an der der
  Kernel keine Kurve hat, und meldet dafuer nichts.
- `onshapeSketchFrame.ts` — der Frame-Abgleich bildet jetzt den FRAME ab
  (`xDir`/`yDir`) und kippt bei einer Spiegelung `clockwise`; die Parameter
  bleiben, damit bleibt Start Start. Vorher wurden die Winkel abgebildet und
  bei det < 0 zusaetzlich Start/End getauscht.

### 4. Zahlen, gemessen (#419)

| | vorher (v0.43.2) | nachher (v0.43.3) |
|---|---:|---:|
| Atomare Flaechen in `Sketch 4` | 14 (2 davon 0,034 mm² Splitter) | **15**, alle vier Quadranten gleich |
| Onshapes 14 Regionen → unsere Flaechen | 11, drei Kollisionen | **14, kollisionsfrei** |
| `Extrude 20` Profil | `qCreatedBy`-artig ueber die ganze Platte | `qRegions(30, [0,1,2,3,5,6,7,8,9,10,11,12,13,14])` |
| `qWall(31, 29)` / `qWall(31, 11)` | 0 / 0 | **1 / 1** (ebenso 26 und 17) |
| Meldungen | 179 | **177** |
| Feature-Namen in `KNOWN_GAPS` | 98 | **97** (`Thicken 7` faellt weg) |
| Koerper | 15 | 15 |
| Import-Dauer | 40,9 s | **47,5 s** |

Die 15. Flaeche ist die 1110 mm² grosse Platte in der Mitte — und die ist
Onshapes Auswahl NICHT. Genau deshalb ist das innere Rechteck jetzt Rand:
seine vier Seiten trennen den gewaehlten Ring vom ungewaehlten Plattenkern,
also traegt jede von ihnen eine Wand. `Thicken 7` findet seine beiden wieder
(alle vier `qFace`-Verweise treffen genau die `qWall`-Flaechen) und BEISST —
unterdrueckt man es, aendert sich das B-Rep von Koerper 11 (179 → 164 Kanten).

Die +6,6 s Import-Dauer sind die Arbeit, die vorher gar nicht getan wurde:
`Extrude 20` webt jetzt einen Rahmen mit zwoelf bogenberandeten Aussparungen
statt einer vollen Platte, und `Thicken 7` baut ueberhaupt.

### Offen

1. **Imprint-Subdivision** bleibt als Thema bestehen (Update 2026-07-24, #412),
   aber dieser Fall war KEIN Beleg dafuer. Wer sie wieder aufruft, braucht
   einen Fall, in dem unsere Anordnung nachweislich zu grob ist — gemessen an
   Flaechen, nicht an der Disambiguierungsliste einer Query.
2. **#416** unveraendert (siehe oben), **#442** nicht beruehrt.

## Update 2026-07-29 (4) — #416: vier Koerper werden einer, und keine der drei Ticket-Ursachen war eine Namensfrage (#442, v0.44.0)

#442 nannte drei Reste des #416-Dokuments: eine fehlende Deckflaeche, eine
Region-Zuordnung und ein Koerper-Docking. Die Direktive war, VOR der Benennung
anzufangen — bei der Szenen-AABB, die schon 102 x 67 x 105 mm gross war, wo
Onshape 73 x 34 x 108 hat. Das war richtig: **alle drei Punkte haengen an
Uebersetzungsfehlern der frueheren Features, und alle drei sind zu.**

### Die Kaskade, Feature fuer Feature gemessen

Die Sonde importiert das Dokument neunzehnmal, jedes Mal ein Feature laenger,
und misst Koerperzahl und AABB. Das trennt Ursache von Folge in einem Durchgang:

| nach Feature | vorher (v0.43.3) | nachher (v0.44.0) |
|---|---|---|
| #5 `Extrude 3` | 2 Koerper, 73 x 33,2 x 100 | **1 Koerper**, 73 x 33,2 x 100 |
| #8 `Extrude 5` | 2 Koerper, **102 x 37,7 x 104,5** | 1 Koerper, 73 x 33,2 x 100 |
| #14 `Extrude 4` | 3 Koerper, 102 x **64,2** x 104,5 | 1 Koerper, 73 x 33,2 x 100 |
| #16 `Revolve 1` | 4 Koerper, 102 x **67,4** x 104,5 | 1 Koerper, 73 x 33,2 x 100 |
| #19 Ende | **4 Koerper**, 102 x 67,4 x 104,5 | **1 Koerper**, 73 x 33,2 x 100 |

Onshapes eigene Antwort: **ein** Teil, 73,12 x 33,90 x 108,08 mm. Importierende
Features: 14 von 19 → **18 von 19**.

### 1. Die fehlende Deckflaeche war ein Profil, das gar keine Region ist

`Extrude 5`s Profil-Query ist kein `IMPRINT`, sondern

    entityType FACE, queryType SWEPT_FACE, operationId FjQX2yN4ORMPFe0_0.opExtrude
    ORIGINAL_DEPENDENCY -> SKETCH_ENTITY QUXJzfqIzVOc

also: **die Wand, die `Extrude 2` aus der Skizzenkurve `QUXJzfqIzVOc` gefegt
hat.** Der Importer las sie als Region, fand keinen Rand und fiel auf
`qCreatedBy(<sketch>)` zurueck — die GANZE Skizze 1, 32 mm tief. Dieser Block
ueberdeckte den Bereich z ∈ [0; 32] vollstaendig und verschluckte damit die
Deckflaeche von `Extrude 1` bei z = 3. Genau deshalb band
`qCreatedBy(3, CAP_TOP)` nichts: die Flaeche existierte nicht, weil ein falsch
uebersetztes Profil sie aufgezehrt hatte.

Uebersetzt wird sie jetzt ueber dasselbe Flaechen-Vokabular wie Skizzenebenen und
Fasenkanten (`faceQueryOf`) und landet als `qWall(2, 62)` im Extrude. Der Kernel
nimmt sie an, seit `selects_brep_faces` neben `qFace` auch `qWall` und
`qCreatedBy(op, ROLE)` als Flaechen-Auswahl zaehlt — der Face-Extrude-Pfad
verlangte bisher einen Hash, den ein Import gar nicht haben kann.

**Ein Profil muss eindeutig sein**: bindet die Flaechen-Query mehr als eine
Flaeche, wird sie NICHT als Profil genommen. Onshape faltet koplanare Kappen zu
einer Flaeche und wir nicht (#443), also kann `qCreatedBy(op, CAP_TOP)` vier
binden — und deren Vereinigung waere eine Flaeche, die niemand gewaehlt hat.

### 2. Die Region-Zuordnung: Onshape nennt Kurven, die es nicht mehr gibt

Drei der vier Region-Queries von `Extrude 1` nennen `6y7lmkTVYOiC` und
`Gr9p6KZZx6Iq`. Diese Ids stehen **nirgends** im Dokument — weder in den
Sketch-Entities noch in Onshapes eigener 3D-Antwort. Eine gespeicherte Query
traegt den Rand, wie er beim Picken stand; Onshape wertet sie nachsichtig aus.
Die ganze Query daran scheitern zu lassen kostete drei von vier Regionen.

Eine MINDERHEIT unbekannter Kurven wird jetzt weggelassen, genau wie ein Rand,
der aus gar keiner Skizzenkurve stammt. Die Haelfte oder mehr bleibt eine
Absage — und das ist keine Geschmacksfrage, sondern an #412 gemessen: dessen
`Extrude 1` verliert zwei von vier Kanten, und zwar Ober- und Unterseite des
Rechtecks, das die Region ueberhaupt definiert. Was bleibt, sind zwei parallele
Senkrechte, die nichts einschliessen; gerankt wuerden sie das Blech UM die
Platten waehlen.

Damit binden `qWall(3, 13)` und `qWall(3, 19)` — die zwei Wandkurven des
Tickets — wieder, und `Fillet 2` uebersetzt sechs statt drei Kanten.

**Onshapes Flanken-Angabe, und warum sie nur halb benutzt wird.** Die
TOPOLOGY-Eintraege sind Paare `[entity, ±1]`, und die `±1` sagt, auf WELCHER
Seite der Kante die Region liegt. Ohne sie waehlt das Ranking eine Bohrung:
Onshape listet deren Rand mit, und die kleine Flaeche IM Loch hat ihren ganzen
Umfang durch diese eine Kante gedeckt (1,0) — gegen 0,18 der Platte, deren
Umfang jede weitere Bohrung mitzaehlt. Gemessen am #416-Dokument ist `+1` die
LINKE Seite der Kurvenrichtung (fuenf von fuenf Nachbarschaften bei
`Extrude 2`s erster Region). Das gilt aber nur in einer Basis, die wie Onshapes
orientiert ist: `Sketch 4` von #419 ist GESPIEGELT, und dort verschob dieselbe
Regel zwei der vierzehn Regionen, die #444 gerade zurechtgerueckt hatte. Die
Flanke wird deshalb nur als VETO benutzt und nur, wo die Haendigkeit gemessen
ist (`SketchXform.det` aus dem #415-Frame-Abgleich). Ohne Messung bleibt sie aus.

### 3. Das Docking war eine Skizze, die 60 mm daneben lag

`Fillet 4` verlangt die Kante zwischen `Extrude 4` und dem Hauptkoerper. Die gab
es nicht, weil `Extrude 4` ein eigener Koerper war — und der war es, weil
`Sketch 3` (zwei konzentrische Kreise auf der Deckflaeche von `Extrude 3`) 60 mm
neben Onshapes Stelle sass. Zwei Ursachen, beide messbar:

**a) Der Frame-Abgleich lief gar nicht.** `measureOnshapeSketchFrame` braucht
drei nicht-kollineare Korrespondenzen; zwei konzentrische Kreise liefern EINEN
Punkt. Bei einer Kreisschar um den eigenen Mittelpunkt ist die Drehung aber
nicht bloss unbekannt, sie ist **gegenstandslos** — bestimmt und messbar bleibt
genau das, was die Geometrie bewegt: die Lage. `measureConcentricSketchOrigin`
uebernimmt FieldDrafts eigene In-Plane-Achsen und setzt nur den Ursprung.
Streng auf Vollkreise und Punkte auf demselben Mittelpunkt begrenzt; schon ein
Bogen macht den Sketch drehempfindlich.

**b) Web und Kernel hatten zwei verschiedene Flaechen-Mittelpunkte.** `B_fd`
kam aus `resolvePlaneFrame`, dessen Ursprung das Mittel der Randkurven-Samples
ist (`face_centroid_local`); der Kernel baut den Sketch-Frame auf dem
Fingerprint-Schwerpunkt, projiziert auf die exakte Traegerebene
(`face_plane_frame`). Auf einem Trapez sind das zwei verschiedene Punkte —
**7,53 mm** auseinander auf `Extrude 3`s Deckflaeche. Der Abgleich rechnete also
gegen einen Frame, den der Kernel nie benutzt. `bodyBrepFaces` meldet jetzt
`planeOriginLocal` (dieselbe Funktion, `face_plane_origin_local`), und der Web
nimmt sie.

Danach sitzt `Extrude 4` auf 0,6 µm genau an Onshapes Stelle, verschmilzt mit
dem Hauptkoerper, und `Fillet 4` findet seine Kante.

**Nebenbefund, mit Zahlen:** derselbe Fehler steckte in #409s Klasse B. `Sketch 6`
und `Sketch 7` galten dort als „Ebene parallel, aber versetzt" — der Versatz war
unserer. Beide verlassen `KNOWN_GAPS` (16 → 14 Namen).

### 4. Ein Mate-Connector auf einer Kante zwischen zwei Flaechen

`resolve_mate_anchor` hatte Zweige fuer `qEdge` und `qEdgeBetween` und fiel fuer
alles andere auf die FLAECHEN-Leiter zurueck. Onshape verankert Connectoren aber
regelmaessig an „der Kante, wo diese zwei Flaechen zusammenkommen", und das ist
`qCommonEdge(a, b)`. In #416 nahm das `Sketch 6`, `Extrude 9` und `Fillet 3`
mit. Jede kantenliefernde Form geht jetzt durch denselben Element-Resolver, den
`resolve_fillet_edges` schon benutzt.

### Zahlen, gemessen

| | v0.43.3 | v0.44.0 |
|---|---|---|
| #416 Koerper | 4 | **1** (Onshape: 1) |
| #416 AABB | 102 x 67,4 x 104,5 | **73 x 33,2 x 100** (Onshape: 73,1 x 33,9 x 108,1) |
| #416 importierte Features | 14/19 | **18/19** |
| #416 Fehlermeldungen | 16 | **7** |
| #416 Import-Dauer | 0,9 s | 6,0 s |
| #409 `KNOWN_GAPS` | 16 | **14** |
| #419 Koerper | 15 | 14 (s. u.) |
| #419 `KNOWN_GAPS` | 96 | 98 (s. u.) |

Die #416-Dauer waechst, weil vorher vier Features gar nicht gebaut wurden und
`Extrude 1` eine Region statt dreier extrudierte. #419 bleibt bei 47,5 s.

**Die drei #419-Pin-Bewegungen, einzeln:**

1. **15 → 14 Koerper.** `Extrude 22` und `Extrude 25` waehlen eine KAPPE eines
   frueheren Extrudes (`CAP_FACE`) — genau die Form aus Punkt 1. Sie fielen
   bisher auf die ganze Skizze zurueck und bauen jetzt den Absatz, den Onshape
   dort hat: kleinerer Koerper an anderer Stelle, und die Partition faltet einen
   in den anderen.
2. **`qWall(31, 26)` bindet 2 statt 1.** Folge derselben Korrektur: der neue
   Absatz auf der Kappe von `Extrude 25` SCHNEIDET diese Wand in zwei Stuecke.
   Beide sind richtig und beide benannt; Onshape meldet eine Flaeche, weil es
   koplanare Flaechen verschmilzt und wir nicht. `Thicken 7` fragt jetzt drei
   der vier Waende an statt vier — die vierte braucht den
   `MERGE → qUnion`-Uebersetzer aus `docs/DERIVED-NAMING-PLAN.md` §4/E3.
3. **`KNOWN_GAPS` +2 (`Sketch 6`, `Sketch 13`).** Beide melden jetzt
   „aufgeloeste Ebene liegt −74,48 mm neben Onshapes Sketch-Ebene". Das ist kein
   neuer Defekt, sondern ein bisher unsichtbarer: ihr Frame war vorher gar nicht
   messbar (zu wenige Stuetzpunkte), erst die Kreisschar-Messung aus 3a beziffert
   ihn.

### Offen — und wo es weitergeht

1. **Ein GESCHNITTENER BOGEN auf dem Profilrand kostet die Wandnamen.** `Extrude 1`
   extrudiert drei Regionen; deren Vereinigung hat den Kreis `pprv8CpVtjqr`
   angeschnitten auf dem Rand. `sketch_arrangement` polygonisiert einen
   geschnittenen Bogen (59 Ebenen statt einer Zylinderflaeche), `wall_sub_table`
   kann die Stuecke keiner Kurve zuordnen und faellt auf POSITIONELLE Subs
   zurueck (0 … 58). Das ist doppelt teuer: die Geometrie ist ein 50-Eck, und
   `qWall(3, 13)` bindet nicht mehr die Kurve 13, sondern die 13. Wand — hier
   eine Facette eben dieses Kreises. Daran haengt der letzte #416-Rest: der
   Mate-Connector von `Sketch 6` steht damit 45,7° schief, und `Extrude 9` baut
   den Keil, der bis z = −73,17 reichen soll, woanders. Gemessen:
   `qRegions(1, [3, 6])` (ohne Bogen) behaelt Entity-Subs 10/16/26/44/50/53,
   `qRegions(1, [0, 3, 6])` (mit) faellt auf 0 … 58.
   → **Der zweite Teil gilt nicht mehr** (Update 2026-07-29 (8), #448): die
   Wandnamen sind seit #445 in Ordnung, und der Rest-Versatz des Keils war die
   Test-Attrappe, nicht der Connector. `Extrude 9` reicht bis z = −73,169
   (Onshape: −73,082). Der Bogen-Befund selbst bleibt stehen.
2. **Die Flanken-Angabe braucht die Haendigkeit** aus dem #415-Abgleich. Wo die
   fehlt (schiefe Ebene, zu wenige Stuetzpunkte), bleibt das Veto aus und das
   Ranking ist das von #440. Ein `SketchXform.det`, das auch fuer nicht
   messbare Frames aus der Flaechen-Parametrisierung folgt, wuerde das schliessen.
   → **Geschlossen in #446** (Update 2026-07-29 (7)): die Haendigkeit war nie eine
   Messung, sondern eine Buchhaltung ueber die eigene Umrechnung, und ist damit
   fuer JEDEN Sketch da.
3. **Acht Skizzen in #419 stehen 90° schief** zu Onshapes eigener Ebene
   (`Sketch 4, 5, 10, 14, 15, 28, 29, 31`, jeweils auf `qWall`/`qCreatedBy(…,
   CAP_TOP)`). Sie werden trotzdem gebaut — sie tragen dort fuenf Koerper. Der
   Winkel steht jetzt in der Meldung; das ist die Zahl, an der die Ursache haengt.
   → **Messartefakt**, aufgeklaert in #446 (Update (7) Punkt 5), vermessen in
   #448 (Update (8)): **alle acht** verschwinden, sobald die Test-Attrappe
   `resolvedMateConnectors` anbietet wie die App — Update (7) hatte „sieben von
   acht" geschaetzt. `Sketch 15` und `Sketch 31` werden dabei zu
   Klasse-B-Meldungen (parallel, aber versetzt), sind also richtig benannt und
   nicht behoben. Die einzige verbleibende schiefe Ebene ist `Sketch 9`, die
   vorher gar nicht aufloeste → **#447**.
4. #416s letzte Fehlermeldungen sind Klasse F/„abgeleitete Geometrie":
   `Chamfer 2` (SPLIT-Kantenstueck), `Fillet 2` (MERGE-Kante) und `Fillet 3`
   (Kanten, die es ohne Punkt 1 nicht gibt).
   → **`Fillet 3` loest seit #448 auf** (Update (8)) — es hing an `Sketch 6`s
   Connector-Ebene, die nur der Test-Attrappe fehlte. Es bleiben `Chamfer 2`,
   `Fillet 2` und `Fillet 5`.

---

## Update 2026-07-29 (5) — ein geschnittener Bogen bleibt ein Bogen (#445, v0.45.0)

Punkt 1 der offenen Liste aus Update (4), gemessen und geschlossen.

### 1. Die Sonde zuerst: was der Fallback wirklich war

Die Ticket-These sagte `WallSubRefusal::Unattributable`. Eine synthetische Sonde
— Quadrat 20 x 20, dessen rechte Kante einen Kreis r = 4,5 anschneidet, alle
drei Atomflaechen als `qRegions` gewaehlt — sagt etwas anderes:

    Profil-Pfad:  Path mit 39 Segmenten, davon 0 gekruemmt
    Wandtabelle:  REFUSED TooManyOccurrences(14)

Die Facetten sind also sehr wohl zuordenbar; sie tragen alle die Entity-Id des
Kreises. Nur sind es zu viele: jede Sehne bricht die G1-Gruppierung
(2 pi / 64 = 5,6° pro Gelenk gegen die Schwelle 2,56°), also wird jede Sehne eine
eigene Wand, und 35 Vorkommen derselben Kurve sprengen
`BranchId::OCCURRENCE_MAX` (31). Der Effekt ist derselbe — positionelle Subs —,
die Ursache ist es nicht. Fuer die Reparatur ist das der Unterschied zwischen
„Zuordnung nachruesten" und „gar nicht erst polygonisieren".

### 2. Die Reparatur: Parameterspannen statt Ganz-oder-gar-nicht

`sketch_arrangement` fuehrt jetzt an jedem geflatteten Mikro-Stueck die
Parameterspanne mit, die es auf seiner Quelle belegt (`Piece.t0/t1`,
`Edge.t0/t1`, orientiert `u → v`). Der Randlauf einer Flaeche wird in LAEUFE
zerlegt — maximale Folgen aufeinanderfolgender Schritte derselben Quelle mit
zusammenhaengender Parameterspanne —, und ein gekruemmter Lauf kommt als EIN
exaktes analytisches Segment zurueck (`sub_segment`: Bogen ueber
`start + sweep*t`, quadratische Bezier ueber die Blossom-Form). Der alte Test
„ganz und ungeschnitten" faellt weg; er ist der Sonderfall `t = 0 … 1`, und der
gibt die Quelle weiterhin unveraendert zurueck.

Zwei Feinheiten, die keine Kosmetik sind:

- **Der Parameter einer GESCHLOSSENEN Quelle ist zyklisch.** Ein Kreis ist ab
  Winkel 0 parametrisiert, und dieser Punkt liegt mitten in jedem Randstueck,
  das der Rand zufaellig benutzt. Ohne die Naht-Ueberbrueckung kaeme ein
  simpler Halbkreis als ZWEI Boegen zurueck, die sich an einer Naht treffen, die
  in der Geometrie niemand verlangt hat — genau die fragile Innennaht, gegen die
  `merge_cocircular_runs` existiert. Die Spanne wird deshalb AKKUMULIERT
  (`0,6 → 1,3`), nicht am letzten Schritt abgelesen.
- **Der Schnittpunkt liegt auf der SEHNE, nicht auf der Kurve.** Ein
  rekonstruierter Bogen beginnt darum bis zu einer Pfeilhoehe neben dem Nachbarn,
  der ihn geschnitten hat (~5 µm bei r = 4,5 mm und 64 Sehnen pro Umlauf). Auf
  einem geschlossenen Pfad ist das nicht kosmetisch: `path_is_inside` zaehlt
  Even-Odd pro Segment, ein Band so hoch wie die Luecke kann doppelt oder gar
  nicht gezaehlt werden (die Fehlerklasse aus #392). `stitch_junctions` loest den
  Treffpunkt darum ANALYTISCH — Gerade/Gerade, Gerade/Kreis, Kreis/Kreis haben
  alle geschlossene Formen — und zieht beide Nachbarn darauf. Eine Verbindung,
  die schon stimmt (jede Verbindung eines geraden oder ungeschnittenen Profils),
  wird nicht angefasst.

### 3. Zahlen, gemessen

`Extrude 1` des #416-Dokuments, dieselbe Skizze, derselbe Import:

| | v0.44.0 | v0.45.0 |
|---|---|---|
| Wand-Subs von `Extrude 1` | positionell 0 … 58 (59 Waende) | Entity 2, 7, 10, 13, 16, 19 (x2), 21, 23, 50, 53 |
| `qWall(3, 13)` bindet | die 13. Wand in Kontur-Reihenfolge (eine Kreisfacette) | die Wand der Skizzenkurve 13 (die untere Lange bei v = −65) |
| #416 AABB | −70 … 11,17 x **−33,17** … 6,5 x **−73,80** … 35 | −70 … 11,17 x **−26,70** … 6,5 x **−65** … 35 |
| Onshape | −70,05 … 3,07 x −26,77 … 7,13 x −73,08 … 35 | |

Die Y-Achse ist damit exakt und wandert aus dem `it.fails`-Pin in den
bestehenden Test. Der #419-Replay (`onshape_419_full_import_stays_bounded`,
49 Kernel-Aufrufe) faellt von **16 579 ms auf 5 441 ms** — echte Zylinderflaechen
entlasten das Weaving, wie vermutet. Koerper 10 desselben Dokuments geht von
(23 Ecken, 30 Kanten, 17 Flaechen) mit 8 offenen Flaechen auf (20, 27, 13) mit
**4** offenen: die vier, die verschwinden, sind die Reste des Facettenfaechers
(Subs 120 Vorkommen 0/1 und 89 Vorkommen 13/14). Jeder andere Koerper ist
Flaeche fuer Flaeche unveraendert.

### 4. Was uebrig bleibt — und warum es NICHT diese Frage ist

Der #416-AABB-Pin bleibt rot, aber aus einem anderen Grund, und der ist jetzt
beziffert. Die EBENE des Mate-Connectors von `Sketch 6` stimmt: Onshapes eigene
3D-Antwort legt den Sketch in die Ebene y = −10,10, und genau dort landet er
auch (die beiden Kappen von `Extrude 9` sitzen bei y = −7,60 / −12,60,
symmetrisch dazu). Falsch ist die DREHUNG IN der Ebene: Onshape bildet u auf
Welt −x und v auf Welt −z ab, `resolve_mate_anchor` bildet u auf +z und v auf +x
ab — 90° verdreht. Die Ursache ist die Wahl der Sekundaerachse eines
KANTEN-Ankers: sie kommt aus `face_normal_for_branch(edge.signature.0)`, und
`signature` ist nach Branch-Wort kanonisiert, also gewinnt hier die WAND
(Sub 13) gegen die Kappe (Sub 0x7FF). Der 8,17 mm lange Keil waechst deshalb in
+x aus der Kappe (x max 11,17 statt 3,07) statt in −z nach unten (z min −65 statt
−73,08). Eigenes Ticket; die richtige Regel braucht Onshapes CSYS-Konvention,
nicht ein Vorzeichen auf Verdacht.

---

## Update 2026-07-29 (6) — eine positionelle Wandtabelle ist nicht mehr auffindbar (#445, v0.46.0)

Der zweite, unabhaengige Befund derselben Messung: `qWall(op, entity)` vergleicht
`branch.sub == entity`, und ein POSITIONELLER Sub 13 sah aus wie die Entity-Id 13.
In #419 fiel das nie auf (die Entity-Ordinale dort sind dreistellig und trafen
schlicht nichts), in #416 sind sie klein und kollidierten — `qWall(3, 13)` gab die
13. Wand in Kontur-Reihenfolge zurueck und nannte sie die Wand der Kurve 13. Eine
still falsche Flaeche ist schlimmer als keine.

### 1. Die Markierung sitzt im Trichter, nicht in der Query

`BranchId::POSITIONAL_WALL_FLAG` (Bit 12 des Sub-Feldes) markiert eine Wand, deren
Sub eine Positionsnummer statt eines Skizzen-Entity-Ordinals ist. Gesetzt wird es
in `csg::map_wall_sub` — dem EINEN Trichter, durch den jede Wand-Branch entsteht
(`BranchId::from_feature_sub`). `wall_sub_table` verweigert jedes Entity-Ordinal,
das das Flag erreicht, also kann ein markierter Sub nie einem Entity gleichen.

Das ist bewusst NICHT ein Sonderfall in `qWall`: die Query bleibt Zeile fuer Zeile
dieselbe, und jeder heutige und kuenftige Leser, der ein Entity gegen einen Sub
haelt, erbt die Garantie, ohne die Regel zu kennen. Eine positionelle Wand behaelt
ihre EIGENE Branch (die Gruppennummer liegt unter dem Flag), also bleiben die
Flaechen getrennt und `qCreatedBy(op, WALL)` — das nach der Rolle fragt, nicht
nach einem Namen — findet sie weiterhin alle.

### 2. Was das am Layout kostete — und was der erste Versuch lehrte

Das Sub-Feld war voll (12 Bit: Bit 11 = `INNER_WALL_FLAG`, darunter die
Entity-Ordinale mit den reservierten Kappen-IDs an der Spitze). Der erste Versuch
setzte das Flag auf Bit 12 eines auf 13 Bit verbreiterten Feldes — und ZERSCHLUG
die Verwebung: die Randnaht eines Bezier-Extrudes fiel von 12 auf 114 Kanten,
Koerper-Flaeche `0x1003` von 4 Randkanten auf 1 plus 105 Innenschleifen. Gemessen,
nicht vermutet: dieselbe Verbreiterung OHNE Flag ist folgenlos.

Die Ursache ist eine ungeschriebene Ordnungsregel, die jetzt geschrieben ist:
**jede Wand sortiert UNTER den reservierten Kappen-IDs.** Die Verwebung sortiert
Branch-Woerter und handelt nach der Reihenfolge (Signatur-Kanonisierung,
Flaechenschleifen-Ordnung); ein Wand-Band OBERHALB der Kappen dreht das um. Das
reservierte Band ist deshalb an die Spitze des verbreiterten Feldes gewandert:

    sub (14 Bit)  = INNER_WALL_FLAG (0x2000)
                  | 0x1FFE/0x1FFF  Extrude-Kappen
                  | 0x1FFD         BLEND_SUB
                  | 0x1FE0/0x1FE8/0x1FF0  Cutter-Aux
                  | 0x1000 …       POSITIONAL_WALL_FLAG-Band
                  | 0 … 0x0FFF     Skizzen-Entity-Ordinale

Der Entity-Bereich waechst dabei von 2046 auf 4096 Ordinale. Das ganze Wort ist
`feature (20) | instance (12) | occurrence (5) | sub (14)` = 51 Bit, weiterhin
eine exakte JavaScript-Zahl (2^51 gegen 2^53, Faktor 4 Luft). Der Web-Spiegel
(`web/src/selectionLinks.ts`) ist mitgezogen — ein veralteter dekodiert jede
Feature-Id falsch.

### 3. Schema-Bump und was healt

`NAMING_SCHEMA_VERSION` 5 → **6**. Die Verbreiterung verschiebt `occurrence`,
`instance` und `feature` um ein Bit, also aendert sich JEDES `id_hash` in JEDEM
Dokument — wie bei den Bumps 3 und 5. Die BEDEUTUNG aendert sich nur fuer
unzuordenbare Profile; alle anderen aendern nur ihr Wort und healen ueber die
Fingerprint-Rebindung (`healElementRefs`). Gemessen an den Test-Dokumenten: die
sechs Onshape-Slow-Suiten (38 Tests) bleiben gruen, `just test` ebenso; die
einzigen Anpassungen waren fest verdrahtete `qFace(<hash>)`-Literale in zwei
Kernel-Tests (Box-Flaechen und eine Wand des Flaechen-Extrudes) und die Tests,
die positionelle Wand-Subs erwarten.

### 4. Zahlen, gemessen

| | v0.45.0 | v0.46.0 |
|---|---|---|
| `qWall(op, 13)` auf einer positionellen Tabelle | bindet die 13. Wand | bindet NICHTS |
| `qCreatedBy(op, WALL)` auf derselben | alle Waende | alle Waende (unveraendert) |
| Sub-Feld | 12 Bit, 2046 Entity-Ordinale | 14 Bit, 4096 Entity-Ordinale |
| Branch-Wort | 49 Bit | 51 Bit (JS-exakt bis 53) |
| `NAMING_SCHEMA_VERSION` | 5 | 6 |
| #416 AABB, #419 Koerper/Flaechen | — | unveraendert gegenueber v0.45.0 |

---

## Update 2026-07-29 (7) — die Haendigkeit ist eine Buchhaltung, kein Abgleich (#446, v0.47.0)

Punkt 2 der offenen Liste aus Update (4). Onshapes Flanken-Angabe (`[entity, ±1]`
je Randkante) war nur dort benutzbar, wo der #415-Frame-Abgleich ein `det`
lieferte; ohne das blieb das Veto aus und das Ranking war das von #440. Der
Auftrag lautete, die Haendigkeit aus der Flaechen-Parametrisierung zu RECHNEN.

Sie folgt aus etwas viel Naeherliegendem: **aus unserer eigenen Umrechnung.**

### 1. Die Sonde zuerst: was `det` ueberhaupt misst

Fuer zwei orthonormale Basen DERSELBEN Ebene ist die Determinante der
2D-Basiswechsel-Matrix exakt

    det = n_os · (x_fd × y_fd)

— reine Algebra, keine Naeherung: aus `x_os = a·x_fd + c·y_fd`,
`y_os = b·x_fd + d·y_fd` folgt `x_os × y_os = (ad − bc)·(x_fd × y_fd)`.
Beide Systeme bauen ihre In-Plane-Basis rechtshaendig auf die Normale der
Ebene, die sie meinen (FieldDraft in `planeBasisFromNormal`, Onshape mit
Welt-Z als Oben-Referenz — an 29 gemessenen Frames bestaetigt: `n = ±Z`
→ `y = ±Y`, sonst `y = Z − (Z·n)n` normiert und `x = y × n`). Also ist
`det = +1`, sobald beide dieselbe Flaeche in derselben Orientierung meinen.

Gemessen an allen fuenf Dokumenten mit 3D-Antwort (409, 414, 415, 416, 419), in
der Konfiguration der App:

| | Frames |
|---|---|
| `det = +1` | **26** |
| `det = −1` | 2 |
| Ebene 90° schief, `det` gegenstandslos | 1 |

Und die zwei `−1` sind keine Konventions-Ausreisser, sondern genau die zwei
bekannten FEHLAUFLOESUNGEN: `Sketch 15` in #419 sitzt auf der anderen Seite
einer 0,1 mm dicken Thicken-Schale (koinzidente Flaeche, umgekehrte Normale),
`Sketch 6` in #416 auf einem Mate-Connector, dessen Z verdreht steht (offen als
#447). Eine gespiegelte In-Plane-Abbildung ist damit ein DIAGNOSE-Signal ueber
die Ebene, nicht ueber die Basis-Konvention — und wird jetzt als Warnung
ausgegeben.

### 2. Und deshalb braucht die Flanke gar keine Messung

Die Haendigkeit, die das Veto braucht, ist nicht die zwischen zwei Frames im
Raum, sondern die zwischen **den 2D-Zahlen, in denen unsere Geometrie steht,**
und Onshapes eigenen. Und die kennt der Importer immer, weil er sie selbst
herstellt:

- Payload umgeschrieben (`transformSketchFeature`) → `SketchXform.det`;
- Payload woertlich uebernommen — keine 3D-Antwort, zu wenige Stuetzpunkte, ein
  Frame, der nicht bestimmbar ist, eine Ebene, die 90° schief aufloest → dort
  stehen ONSHAPES EIGENE 2D-Zahlen, die Abbildung ist die IDENTITAET, `det = +1`.

Einen dritten Fall gibt es nicht. Auch eine 90° schief aufgeloeste Ebene aendert
an den 2D-Zahlen nichts — sie haengt sie nur woanders in den Raum, waehrend die
Regionen-Topologie in der Skizze dieselbe bleibt. `SketchRecord.frameDet`
(nullable, „gemessen") heisst deshalb jetzt `basisDet` (immer da, „gerechnet"),
und `resolveRegionFaces` liest die Flanke fuer JEDEN Sketch.

Was das an Reichweite bringt (App-Konfiguration, 39 Sketches der fuenf
Dokumente): 32 hatten eine lesbare Flanke, **jetzt 39**. In der aermeren
Kernel-Attrappe der Slow-Suiten (siehe Punkt 5) waren es 19 von 39.

### 3. Der eigentliche #419-Blocker war ein Kreis, keine Haendigkeit

Update (4) hatte notiert, dass das Veto mit fest verdrahtetem `+1` zwei der
vierzehn Regionen von #419s `Sketch 4` verschob. Genau vermessen ist das KEINE
Haendigkeitsfrage:

- Beide betroffenen Regionen sind Bohrungen, deren ganzer Rand aus EINER Kante
  besteht (`edges=1/1` in der Sonde) — dem Vollkreis Ø 1,9 mm eines
  Schluesselloch-Schlitzes.
- Beide Kreise tragen `clockwise: true`. Das ist kein Zufall: es sind
  Spiegel-KOPIEN (`…MirrorC`), und eine Spiegelung dreht den Umlaufsinn um.
- `sketchEntityVertices` flachte einen Vollkreis mit fest verdrahtetem
  Start 0 und Weite +2π ab, also IMMER gegen den Uhrzeigersinn. „Links der
  Kurvenrichtung" zeigte damit fuer diese zwei Kurven nach aussen, und das Veto
  waehlte das Material UM die Bohrung statt der Bohrung.

Ein Vollkreis geht jetzt durch dieselbe Lesung wie ein Bogen (`arcWorldSpan`,
#444) und laeuft in Onshapes eigener Richtung. Danach stimmt das Veto auf allen
**82** Region-Queries des Dokuments mit dem Ranking ohne Veto ueberein — dieselben
zwei Regionen eingeschlossen.

### 4. Zahlen, gemessen

Region-Queries, die bis zum Ranking kommen (Slow-Suiten-Konfiguration):

| Dokument | Regionen | Veto scharf | schliesst Kandidaten aus | Kandidaten weg |
|---|---|---|---|---|
| #409 | 7 | 7 | 5 | 13 / 20 |
| #414 | 7 | 7 | 5 | 8 / 16 |
| #416 | 10 | 10 | 5 | 10 / 22 |
| #419 | 68 | 66 | 56 | 120 / 190 |
| **Summe** | **92** | **90** | **71** | **151 / 248** |

Die ENTSCHEIDUNG aendert sich gegenueber „ohne Veto" an genau einer Stelle:
`Extrude 1` in #416 waehlt Flaeche 0 statt Flaeche 5 — die Platte statt der
Ø 4,14 mm-Bohrung, die #442 gemessen hatte.

Dass sich sonst nichts bewegt, IST das Ergebnis: das Veto haelt an 71 Stellen
Kandidaten heraus, die das Ranking bisher nur deshalb nicht gewaehlt hat, weil
die Deckungsanteile dort zufaellig eindeutig genug waren. Aus einer Gewichtung,
die man glauben muss, ist eine Entscheidung geworden, die man nachrechnen kann.

**In der Konfiguration der APP** (Punkt 5) ist es kein Nullresultat: dort war
die Flanke von `Sketch 4` lesbar, also biss der Vollkreis-Fehler aus Punkt 3
wirklich. Drei Extrudes bekommen ihre zwei fehlenden Regionen zurueck — und
landen auf genau der Vierzehner-Menge, die die Slow-Suite in ihrer eigenen,
aermeren Konfiguration seit #444 pinnt:

| | v0.46.0 (App) | v0.47.0 (App) |
|---|---|---|
| `Extrude 20` | 12 Flaechen (7 und 14 fehlen) | **14** — `qRegions(32, [0,1,2,3,5,6,7,8,9,10,11,12,13,14])` |
| `Extrude 23` | 9 Flaechen | **11** (7 und 14 zurueck) |
| `Extrude 49` | 6 Flaechen | **8** (7 und 14 zurueck) |

Alle sechs Slow-Suiten (jetzt 39 Tests) sind gruen; Import-Dauern #416 3,3 s /
#419 54 s wie zuvor. Ein neuer Test in der #419-Suite fuehrt die App-Attrappe
mit und haelt genau dieses Ergebnis fest — sonst kann die Suite diese Klasse
von Regression strukturell nicht sehen.

### 5. Nebenbefund: die Slow-Suiten fahren eine aermere Kernel-Attrappe

Beim Vermessen fiel auf, dass `wrap()` in den Slow-Suiten `resolvedMateConnectors`
nicht anbietet — die App (`Kernel`) hat die Methode. Ohne sie kann
`resolvePlaneFrame` eine Ebene auf einem Mate-Connector nicht aufloesen; in #419
sind das drei Sketches, und weil deren Folge-Features dann fehlen, greifen acht
weitere Sketches auf falsche Waende zu und loesen 90° schief auf. Genau diese
acht sind die „acht Skizzen, die 90° schief stehen" aus Update (4) Punkt 3 —
ein Messartefakt der Testumgebung, kein Defekt des Imports.

| #419, gleicher Code | Attrappe der Suiten | volle Kernel-Oberflaeche |
|---|---|---|
| Koerper | 14 | **18** (Onshape: 17 Teile) |
| Features mit Fehler | 99 | **89** |
| Sketches mit 90° schiefer Ebene | 8 | **1** |

Eigenes Ticket (**#448**): die Pins von sechs Dateien muessen dabei einzeln neu
vermessen werden, und das ist der teure Teil.

→ **Erledigt in #448** (Update 2026-07-29 (8)). Zwei Korrekturen an diesem
Abschnitt: es sind ALLE acht (nicht sieben), und die eine verbleibende schiefe
Ebene ist keine der acht, sondern `Sketch 9` — vorher unsichtbar, weil ihre
Ebene ueberhaupt nicht aufloeste. Ausserdem bewegte die volle Oberflaeche mehr
als #419: #416 gewinnt `Fillet 3` und zwei AABB-Achsen, #409 macht einen
168,96-mm-Ebenenversatz sichtbar, der vorher nur eine Warnung war.

---

## Update 2026-07-29 (8) — die Suiten messen jetzt den Import, den die App fährt (#448)

Kein Zeile Import-Code hat sich geändert. Geändert hat sich, **was die
Testumgebung dem Übersetzer hinreicht** — und das hat vier Dokumente bewegt,
eins davon dramatisch. Wer die Zahlen unten mit älteren Updates vergleicht,
muss das mitlesen: die alten Zahlen waren nicht falsch gemessen, sie waren an
einer Attrappe gemessen, die der Benutzer nie hat.

### 1. Eine Attrappe, gegen das Interface des Importers getypt

`wrap()` lag sechsmal handgebaut in `web/src/onshape/*E2E.slow.test.ts`, und in
allen sechs Kopien fehlte `resolvedMateConnectors`. Neu ist
`web/src/onshape/importKernelDouble.ts` mit **einer** Fabrik. Der Grund, warum
das die Wurzel trifft und nicht nur den Symptomfall, sind zwei Typ-Schlösser:

- Die Attrappe ist `Required<ImportKernel>` getypt. Fast alles an
  `ImportKernel` ist optional, damit kleine Unit-Doubles klein bleiben dürfen —
  hier ist nichts optional. Lernt der Importer eine neue Frage, bricht die
  Kompilierung der Suiten, statt dass sie leise weniger messen.
- `AppKernelCoversImporter` ist eine reine Typ-Zusicherung, dass die echte
  `Kernel`-Klasse — das, was `onshapeProjectImport.ts` durchreicht — diese
  volle Oberfläche abdeckt. Driften App und Importer auseinander, hört die
  Datei auf zu kompilieren.

### 2. Was sich je Dokument bewegt hat, einzeln

`kabelbox`, `#412`, `#414`, `#415`: **bis aufs Bit unverändert** — Körperzahl,
Fehlerliste, Warnungen, AABB. Diese vier Dokumente haben keine Skizze auf einem
Mate-Connector.

**#409** — Geometrie unverändert (16 Körper, 29 Features, identische AABB), aber
**eine Warnung ist zu einem Fehler geworden**, und das ist ein echter, bisher
unsichtbarer Befund. `Sketch 8` sitzt auf dem Connector `qCreatedBy(26)`. Der
KERNEL hat die Skizze immer schon dorthin gelegt (deshalb bewegt sich nichts);
nur die Web-Seite konnte die Ebene nicht auflösen und warnte bloß „Frame nicht
auflösbar, 2D-Koordinaten wörtlich übernommen". Aufgelöst steht diese Ebene
**168,96 mm neben** Onshapes eigener 3D-Antwort für `Sketch 8`. Das ist Klasse B
(parallel, aber versetzt) und steht ab jetzt in `KNOWN_GAPS`. Dasselbe gilt
abgeschwächt für `Sketch 1`, dessen 3D-Antwort zu wenige Stützpunkte hat — das
bleibt eine Warnung.

**#416** — `Fillet 3` löst auf, 4 → 3 Features mit Fehler. Und die AABB:

| #416 | Attrappe der Suiten | volle Oberfläche | Onshape |
|---|---|---|---|
| x max | 11,17 | **3,000** | 3,074 |
| z min | −65 | **−73,169** | −73,082 |
| y max | 6,500 | 6,500 | 7,129 |

`Extrude 9`s 8,17-mm-Keil wächst aus der Deckfläche von `Extrude 4`, und seine
Skizze sitzt auf einem Connector. Ohne `resolvedMateConnectors` fiel
`resolvePlaneFrame` auf die Fläche DARUNTER zurück und baute eine Basis aus
deren Normale — ein in seiner eigenen Ebene um 90° gedrehter Frame. Der Keil
zeigte deshalb nach +x statt nach −z. **Das war die Ursache, die das `it.fails`
dieses Dokuments bisher als „Mate-Connector-Frame 90° in der Ebene verdreht"
beschrieben hat.** Gemessen: das Unterdrücken von `Extrude 9` zieht `z min`
zurück auf −65, die Zuordnung ist also keine Erzählung. Fünf der sechs Grenzen
liegen jetzt unter 0,09 mm auf Onshape; offen bleibt allein `y max`, 0,63 mm zu
kurz. Der Rest ist NICHT ein fehlender Körper: kein einzelnes Feature bewegt
beim Unterdrücken `y max`, mehrere Körper teilen sich diese Ebene. Die offene
Spur sind die drei übrigen Kanten-Namen (`Chamfer 2`, `Fillet 2`, `Fillet 5`) —
eine Verrundung auf einer konkaven Kante ist das einzige, was Material ANFÜGT.
Das Dokument hat außerdem zum ersten Mal eine Fehlerzensus-Pin bekommen; ohne
sie wäre `Fillet 3` unbemerkt besser geworden.

**#419** — das große Dokument.

| #419, gleicher Code | Attrappe der Suiten | volle Oberfläche | Onshape |
|---|---|---|---|
| Körper | 14 | **18** | 17 Teile |
| Meldungen | 180 | **170** | — |
| Features mit Fehler | 99 | **89** | — |
| Skizzen mit 90° schiefer Ebene | 8 | **1** | — |
| Szenen-AABB x | [−157; 163] | **[−3015,4; 71,5]** | [−3015,4; 85,6] |
| Szenen-AABB y | [−3015,4; 71,5] | **[−157; 163]** | [−157; 163] |

Elf Namen sind aus `KNOWN_GAPS` verschwunden: `Sketch 4`, `5`, `6`, `10`, `13`,
`14`, `28`, `29` legen sich auf ihre Connector-Ebenen, `Circular pattern 1` und
`Linear pattern 1` finden ihre Achse (eine Skizzenlinie auf so einer Ebene) in
Weltkoordinaten, und `Extrude 28`s UP_TO_SURFACE-Ziel löst auf. Einer ist
dazugekommen: `Sketch 9`.

Zwei Dinge daran sind wichtiger als die Zahlen:

- **Der 90°-Dreh um Z war kein Import-Defekt.** Die Szenen-AABB lag auf den
  vertauschten Achsen `ours(x, y, z) = Onshape(−y, x, z)`, und das war seit
  #424 als gemessener Defekt gepinnt, mit der Basis-Regel Flächennormale-statt-
  Parametrisierung (#415) als Verdächtigem. Die Mechanik stimmte, der Täter
  nicht: die Wurzel-Skizzen des Dokuments sitzen auf Mate-Connectoren, und
  genau der Rückfall auf die Fläche darunter hat den Frame gedreht. Fünf der
  sechs Grenzen liegen jetzt unter 0,06 mm auf Onshapes eigenen Achsen; `x max`
  fehlen 14 mm, weil 89 Features nicht importieren. Noch schärfer: der Körper
  von `Extrude 16` trifft **alle sechs** `evBox3d`-Werte, die Onshape für ihn
  meldet.
- **`Sketch 9` ist die Vorvermessung für #447.** Es ist die einzige verbliebene
  schiefe Ebene (`qCreatedBy(23)`, 90,0°) und war vorher nicht sichtbar, weil
  diese Ebene überhaupt nicht auflöste. Wer #447 aufnimmt, fängt hier an — und
  NICHT bei #416, dessen 90°-Symptom sich als Messartefakt herausgestellt hat
  (siehe oben). Der Verdacht „Kanten-Anker zieht die Sekundärachse aus der
  kanonisierten `edge.signature.0`" ist damit noch nicht widerlegt, aber auch
  nicht mehr belegt: der Fall, an dem er belegt schien, ist weg.

### 3. Korrekturen an früheren Updates

- **Update (4) Punkt 3** („Acht Skizzen in #419 stehen 90° schief"): alle acht
  genannten (`Sketch 4, 5, 10, 14, 15, 28, 29, 31`) verschwinden, nicht sieben
  — Update (7) Punkt 5 hat „sieben von acht" geschätzt. `Sketch 15` und
  `Sketch 31` werden dabei zu Klasse-B-Meldungen (parallel, aber 0,1 bzw.
  16,98 mm versetzt), sind also nicht behoben, nur richtig benannt. Die eine
  verbleibende schiefe Ebene ist `Sketch 9` und war vorher unsichtbar.
- **Update (4) Punkt 1** (#416, geschnittener Bogen): der zweite Teil des
  Absatzes — „der Mate-Connector von `Sketch 6` steht damit 45,7° schief, und
  `Extrude 9` baut den Keil woanders" — gilt nicht mehr. Die Wandnamen sind
  seit #445 in Ordnung, und die Restverschiebung des Keils war die Attrappe.
  `Extrude 9` reicht bis z = −73,169.
- **Update (4) Punkt 4** (#416s letzte Meldungen): `Fillet 3` gehört nicht mehr
  dazu. Es bleiben `Chamfer 2` (SPLIT-Kantenstück) und `Fillet 2` (MERGE-Kante),
  dazu `Fillet 5` (`qCommonEdge(qWall(19, 4), qWall(22, 4))` bindet nichts).
- **Der `MERGE → qUnion`-Bedarf an `Thicken 7`** (#419) ist an dieser Stelle
  nicht mehr erreichbar: `qWall(33, 26)` bindet wieder EINE Fläche, weil
  `Extrude 25` — hinter `Sketch 13` — dort landet, wo Onshape es hat, statt
  quer über diese Wand. Alle vier Wände werden benannt. Die Lücke aus
  `docs/DERIVED-NAMING-PLAN.md` §4/E3 ist davon unberührt, sie wird hier bloß
  nicht mehr getroffen.

### 4. Laufzeit

Ein #419-Import kostet mit der reicheren Attrappe 56,2 s statt 54,0 s (+4 %) —
sie baut mehr Features. Die ganze Datei liegt bei 355 s (sechs Importe). Die
fünf übrigen Suiten zusammen bleiben unter 14 s. Kein Perf-Stoppsignal.

## Update 2026-07-29 (9) — der Anker-Typ stand im Dokument, gelesen wurde der Tokenstrom (#447, v0.47.1)

Ticket #447 hiess „Mate-Connector auf Kante 90 Grad in der Ebene verdreht" und
vermutete die Ursache in der SEKUNDAERachse: `resolve_mate_anchor` waehlt die
Nachbarflaeche eines Kanten-Ankers ueber `edge.signature.0`, und diese Signatur
ist nach Branch-Wort kanonisiert — eine Sortier-Konvention, keine Geometrie.

Diese Vermutung ist **weder die Ursache noch ueberhaupt ein messbarer Fehler**,
und der beschriebene Twist existiert nicht mehr. Was stattdessen gefunden wurde,
sitzt eine Ebene frueher: **wir haben den Anker-Typ falsch gelesen.**

### 1. Was am #416-Fall wirklich steht (die Ticket-These, nachgemessen)

`Sketch 6` von #416, der Fall aus dem Ticket, sitzt heute exakt richtig. Gegen
Onshapes eigene 3D-Antwort:

| `Sketch 6` (#416) | Ursprung | x-Achse | y-Achse | Normale |
|---|---|---|---|---|
| Onshape | (3,000; −10,100; −65,000) | (1, 0, 0) | (0, 0, −1) | (0, 1, 0) |
| FieldDraft | (3,000; −10,100; −65,000) | (0, 0, −1) | (1, 0, 0) | (0, −1, 0) |

Ursprung identisch, Ebene identisch. Der Unterschied ist eine **Spiegelung**
(x und y vertauscht), keine Drehung um 90 Grad — und `alignSketchFrame` rechnet
sie exakt heraus, weshalb `x max` und `z min` dieses Dokuments
ground-truth-genau sind. Es bleibt eine `det < 0`-Warnung, kein Fehler und
keine falsche Geometrie. Die Sekundaerachsen-Regel bleibt damit **unbelegt**:
sie waehlt hier die WAND, wo Onshape die KAPPE waehlt, aber der Importer
kompensiert das vollstaendig. Nicht anfassen ohne einen Fall, der weh tut.

### 2. Der echte Defekt: `entityType` aus dem Tokenstrom statt aus dem Dokument

`decodeMateAnchor` las den Entity-Typ des Ankers als **erstes**
`EDGE|VERTEX|FACE`-Token nach dem ersten `entityType`-Schluessel. Onshape haengt
aber einen `disambiguationData`-Block **vor** die beiden Felder, die die Frage
beantworten, und jede Entity darin traegt einen eigenen `entityType` — die
Kurven, die eine Flaeche einzaeunen, sind EDGEs. Der Scan beantwortete damit
eine andere Frage („was zaeunt es ein?" statt „was IST es?"), und zwar fuer
jeden Anker falsch, den Onshape disambiguiert.

Zensus ueber alle acht Fixtures, 16 Anker, gegen das jeweils geparste
Top-Level-Dokument:

| Dokument · Connector | Onshape | gelesen | Folge |
|---|---|---|---|
| #419 `Sketch 9` | `FACE`/`IMPRINT` | `EDGE` | Ebene **90,0 Grad** schief |
| #409 `Sketch 8` | `FACE`/`IMPRINT` | `EDGE` | Ursprung **168,96 mm** daneben |
| #416 `Sketch 4` | `FACE`/`CAP_FACE` | `EDGE` | folgenlos (`CENTER` griff vorher) |
| #419 `Sketch 21` | `FACE`/`CAP_FACE` | `EDGE` | folgenlos (Kaskade dahinter) |
| #419 `#featureName` | `FACE`/`SPLIT` | `EDGE` | folgenlos (Bohrung, nie gelesen) |
| #409 `#featureName` ×2 | `EDGE`/`COPY` | `VERTEX` | folgenlos (Bohrung, nie gelesen) |
| #409 `Sketch 1` | `VERTEX`/`DUMMY` | Ursprungspunkt | korrekt (Sonderfall vorher) |
| die uebrigen 8 | — | — | korrekt |

Warum das keine Etiketten-Frage ist: das +Z eines Connectors IST die Achse
seines Ankers. Eine FLAECHE als eine ihrer Randkanten gelesen stellt das ganze
Koordinatensystem — und die Skizze darauf — um 90 Grad auf (`Sketch 9`, exakt
gemessene 90,0 Grad) oder schiebt den Ursprung an dieser Kante entlang
(`Sketch 8`, 168,96 mm).

### 3. `FACE`/`IMPRINT` ist eine Sketch-REGION, und die konnten wir schon

Onshapes Imprint-Flaeche ist genau die Form, die ein Extrude-Profil auch hat:
eine Region, benannt durch ihren Zaun aus Sketch-Kurven mit dem `±1`-Flanken-
Vorzeichen je Kurve. Der Dekoder dafuer (`decodeRegionQuery` /
`resolveRegionFaces`, #412/#442/#446) steht seit Langem — der Anker-Pfad hatte
nur seinen eigenen, aermeren Leser. Er benutzt jetzt denselben, und
`regionReaders()` ist die eine gemeinsame Definition der beiden Leser, die
`resolveRegionFaces` rankt (vorher inline im Profil-Pfad).

Kernel-seitig ist `qRegion(sketch, i)` als Mate-Anker neu
(`resolve_mate_anchor`): eine Region ist keine gewobene Geometrie, also erreicht
sie keine Flaechen-Query. Ihr Schwerpunkt (`SketchRegion::centroid`, durch den
Sketch-Frame gehoben) ist der Ursprung, die Sketch-Normale ist +Z — genau
Onshapes `CENTROID`-Inferenz. Der Schwerpunkt wird genommen, wie Onshape ihn
nimmt: als Flaechen-Schwerpunkt, der bei einem RING im Loch liegt. Ein
Mate-Connector ist ein Koordinatensystem, kein Klickpunkt.

Die Eindeutigkeits-Schranke in `emitMateConnector` nimmt `qRegion(` neben
`qSketchLine(` und den Datum-Ebenen aus: beide benennen keine B-Rep-Entity, also
meldet `resolveQuery` nichts ueber sie, und „0 Elemente" waere die Schranke
dabei, eine nie gestellte Frage zu beantworten.

### 4. Was sich gemessen bewegt hat

Diff der vollstaendigen Fehler-/Warnungslisten von #409, #416 und #419, vorher
gegen nachher — **drei Zeilen, sonst nichts**:

- **#409** `Sketch 8: die aufgelöste FieldDraft-Ebene "qCreatedBy(26)" liegt
  −168.9620 mm neben Onshapes Sketch-Ebene` — **weg**. Klasse B verliert diesen
  Eintrag, den Update (8) erst sichtbar gemacht hatte.
- **#419** `Sketch 9: Onshapes Sketch-Ebene steht 90.0° schief` — **weg**. Damit
  hat #419 **keine schiefe Ebene mehr**; der Zensus-Pin steht auf 0.
- **#419** `Sketch 21`: der Ankertext heisst `FACE der End-Deckflaeche` statt
  `EDGE der End-Deckflaeche`. Dieselbe Kaskade (`Extrude 9` fehlt), richtiger
  benannt.

Koerperzahl und Szenen-AABB aller drei Dokumente sind **bis aufs Bit
unveraendert**. Das ist die ehrliche Lesart und war zu erwarten: beide
reparierten Skizzen tragen kein Volumen, das an ihnen haengende `Extrude 7`
bzw. `Extrude 9` scheitert unabhaengig an `startOffset` (Klasse H). Die Ebenen
stimmen jetzt gegen Onshapes eigene 3D-Antwort — das ist die Aussage, nicht
eine groessere AABB.

### 5. Was #416 offen laesst — und was es NICHT ist (gemessen)

Der `it.fails`-Pin von #416 („y max, 0,63 mm zu kurz", 6,500 gegen 7,1292)
bleibt offen. Die in #448 notierte Spur — ein fehlender KONKAVER Blend koenne
`y max` anheben — ist **widerlegt**, aus zwei unabhaengigen Gruenden:

1. **Lage, gemessen.** Die drei offenen Kantennamen-Fehler liegen anderswo:
   `Fillet 5` (r = 2,43) sitzt bei y ≈ −19 an der Kugel von `Revolve 1`;
   `Chamfer 2` (5 mm/45 Grad) und die zwei scheiternden `Fillet 2`-Kanten
   (r = 1,21) liegen auf der `.left`-Seite bei y = −26,7. Die EINE
   `Fillet 2`-Kante, die y = 6,5 beruehrt, importiert bereits.
2. **Ein konkaver Blend kann eine AABB grundsaetzlich nicht vergroessern.** Das
   Material, das er in eine einspringende Kante fuellt, liegt vollstaendig im
   Keil zwischen den beiden Nachbarflaechen; jeder Extrempunkt davon liegt auf
   einer dieser Flaechen, also schon auf dem Koerper. Und die SDF sagt, dass die
   oberste Flaeche exakt die Ebene y = 6,5 ist (`bodySdfEval` liefert darueber
   genau `y − 6,5`; die vier Mesh-Spitzen bei y = 7,10 sind QEF-Artefakte des
   Dual Contouring mit SDF ≈ +0,6, also Luft).

Zwei neue Spuren aus derselben Messung, **nicht** vermessen und deshalb als
Spur markiert:

- `Sketch 1` von #416 traegt zwei Kreise r = 4,5 um (6,5; ±35), die bis
  Sketch-x = 11,0 reichen — Onshapes 7,1292 liegt zwischen 6,5 und 11,0. Der
  Import warnt, dass `Extrude 1` vier Onshape-Regionen auf drei atomare Flaechen
  abbildet, davon „3 nennen Sketch-Kurven, die dieser Sketch nicht enthaelt".
- Der Fehler-Zensus der Suiten sieht **keine Regenerierungs-Fehler**, nur
  Import-Fehler. `listFeatures()` zeigt in #416 zwei: beide `Chamfer 1`, vom
  Kernel mit „Kante ist KONKAV" abgelehnt — an einer Kante, deren
  Nachbarnormalen (−1, 0, 0) und (0, 0, 1) sie offensichtlich KONVEX machen.
  Ein unterdruecktes 5-mm-Chamfer laesst mehr Material stehen, erklaert die
  Luecke also auch nicht; es ist eine ungepinnte Fehlerklasse.

Beides liegt als **#449** ab, nicht hier weiterverfolgt.

## Update 2026-07-29 (10) — die Ground Truth war die falsche Box (#449, v0.47.2)

Ticket #449 hiess „#416-Import: y max bleibt 0,63 mm zu kurz" und brachte zwei
Spuren mit (eine verlorene Sketch-Region; ein falsches KONKAV-Urteil). Die
erste Spur ist gegenstandslos, die zweite bestaetigt sich — aber die
eigentliche Antwort liegt vor beiden:

**`GET …/partid/{id}/boundingboxes` liefert nicht den Umriss der Geometrie,
sondern Onshapes LOSE Box.** Die 0,63 mm haben nie gefehlt; die Vergleichszahl
war 0,63 mm zu gross.

### 1. Die Messung, drei unabhaengige Onshape-Antworten

`evBox3d` nimmt ein `tight`-Flag. Mit `tight: false` umschliesst es die
Kontrollnetze der Flaechen statt des getrimmten Koerpers, und genau das
liefert der REST-Endpunkt. Beide Boxen in EINEM FeatureScript-Aufruf ueber
`qBodyType(qEverything(EntityType.BODY), BodyType.SOLID)`, #416:

| | x min | y min | z min | x max | y max | z max |
|---|---:|---:|---:|---:|---:|---:|
| lose (= REST-Endpunkt) | −70,0501 | −26,7740 | −73,0819 | 3,0740 | **7,1292** | 35,0000 |
| **TIGHT** | −70,0001 | −26,7000 | −73,0013 | 3,0000 | **6,5000** | 35,0000 |
| Tessellierung (0,05 mm) | −70,0002 | −26,7000 | −73,0013 | 3,0000 | **6,5000** | 35,0000 |
| **FieldDraft** | −70,0000 | −26,7000 | −73,1688 | 3,0000 | **6,5000** | 35,0000 |

Die Tessellierung (68 Flaechen, 43 993 Facetten) schuldet weder der einen noch
der anderen Box etwas und trifft die tight-Box auf 0,0001 mm in allen sechs
Grenzen. Damit ist die Sache entschieden, nicht argumentiert.

FieldDraft trifft **fuenf der sechs Grenzen auf ≤ 0,0001 mm**. Die
SDF-Bisektion aus #447/#448, die immer wieder die Deckflaeche BEI y = 6,5 fand,
hatte die ganze Zeit recht.

Zensus ueber alle Fixture-Dokumente, die noch erreichbar sind:

| Dokument | Schlupf lose gegen tight |
|---|---|
| #409 (33 Koerper) | **keiner**, alle sechs Grenzen identisch — das Dokument traegt keine Verrundungen |
| #414 | x min 0,0547 mm · y min 0,1285 mm |
| #415 | z max 0,3103 mm |
| #416 | x min 0,050 · y min 0,074 · z min 0,081 · x max 0,074 · **y max 0,629** mm |

Der Schlupf sitzt auf verrundeten und Spline-Flaechen; wo keine sind, gibt es
keinen. Er ist **pro Grenze** verschieden gross, also nicht wegtolerierbar.

### 2. Was das an Pins bewegt hat, einzeln

* **#416** `GROUND_TRUTH` lose → tight (alle sechs Werte). `y max` wandert in
  den bestehenden AABB-Test; die Toleranz der fuenf exakten Grenzen faellt von
  0,5 mm auf 0,005 mm, weil ein Gate, das 100× weiter ist als die gemessene
  Groesse, exakt nicht von beinahe unterscheiden kann.
* **#416** `it.fails("matches the ground-truth AABB")` **entfaellt**.
* **#414** `GROUND_TRUTH` x min −65,0547 → −65,0000, y min −70,1285 → −70,0000.
  Damit faellt auch dort `it.fails("matches the FULL ground-truth AABB")`: das
  Dokument importiert **exakt** auf Onshapes tight-Box, alle sechs Grenzen. Der
  Kommentar an diesem Pin behauptete einen Restfehler von „~2,8 mm in x und y"
  — der ist laengst weg, und niemand hat es gemerkt, weil der Pin aus einem
  ANDEREN Grund weiter rot war. Genau das ist die Gefahr eines `it.fails`,
  dessen Begruendung nicht nachgemessen wird.
* **#415** unveraendert. Die Datei hatte den Schlupf (0,31 mm in z max) schon
  von Hand gegen die Tessellierung korrigiert, ohne den Mechanismus zu kennen —
  die Vermutung dort („der Endpunkt ist bei Blend-/Spline-Flaechen
  approximativ") war richtig. Nur der Kopfkommentar benennt jetzt die Ursache.
* **#409** unveraendert, nachgemessen.
* **#419** war zunaechst nicht nachmessbar: die Dokument-ID stand nirgends im
  Repo — der Suiten-Kopf schrieb woertlich `documents/…/e/…`. Sie liess sich
  ueber einen Sweep von 68 Dokumenten nach „17 Vollkoerper" zurueckholen
  („Bath Interferometer", `a40fbe5244d0a1c6abfe8c96` /
  `w/f7007df96ecd69d81728cd05` / `e/450a3eda61b15636607c33c5`) und steht jetzt
  im Kopf. Nachgemessen: lose UND tight liefern dieselben sechs Zahlen. Die
  Ground Truth ist von 0,1-mm-Rundungen auf die exakten Werte gehoben
  (x [−3015,405; 85,635] · y [−157; 163] · z [−150,95; 169,05]); die Toleranz
  bleibt 0,5 mm und kein Pin bewegt sich.

  **Lehre daraus, unabhaengig vom Ergebnis:** eine Ground Truth ohne
  Dokument-ID ist nicht reproduzierbar. Die Dokument-IDs gehoeren in JEDEN
  Suiten-Kopf.

### 3. Was WIRKLICH offen bleibt: z min, 0,1675 mm zu weit (#450)

Die einzige echte Abweichung von #416, und sie zeigt in die andere Richtung:
wir reichen mit z min auf −73,1688, Onshape auf −73,0013 — wir sind
**0,1675 mm ZU WEIT**, nicht zu kurz. Kein Losigkeits-Artefakt: die lose Box
liegt mit −73,0819 noch diesseits unseres Wertes.

`sceneAabb` ist dokumentiert lose (`CsgNode::aabb`, „good enough to bound the
mesher grid"; `Difference` gibt den linken Operanden zurueck), taugt also
allein nicht als Beleg. Die SDF schon: bei z = −73,1 steht Material ueber
x ∈ [2,5; 3,0], y ∈ [−12,5; −7,75] (0,25-mm-Raster), und die Bisektion dieser
Saeule legt die Oberflaeche auf −73,168. Das ist die **Spitze von `Extrude 9`s
8,17-mm-Keil** — dasselbe Feature, das diese Suite schon als Treiber von z min
kennt (unterdrueckt man es, springt die Grenze auf −65). 0,17 mm
Keilspitze auf einem 108 mm langen Teil ist ein Bruchteil eines Grades in der
Ausrichtung oder Endbedingung des Keils, kein fehlendes Feature. Gepinnt als
gewoehnliches `it` auf die gemessene Zahl.

### 4. Spur 1 (verlorene Sketch-Region): gegenstandslos, nicht widerlegt

Die Warnung steht unveraendert: `Extrude 1` bildet vier Onshape-Regionen auf
drei atomare Flaechen ab, drei davon nennen Sketch-Kurven, die dieser Sketch
nicht enthaelt (Update (4) §2). Sie **kann** die y-max-Luecke aber nicht
erklaeren, weil es keine gibt. Ob eine der vier Regionen inhaltlich verloren
geht, bleibt die offene Frage aus Update (4) — sie kostet dieses Dokument
nachweislich keine Ausdehnung (fuenf von sechs Grenzen exakt), was ein
Innenliegen nicht ausschliesst.

### 5. Der Fehler-Zensus sah nur die HAELFTE

Die Suiten pinnten `result.errors` — was der Importer zu SCHREIBEN abgelehnt
hat. Was der Kernel aus dem Geschriebenen zu BAUEN ablehnt, hat keine einzige
Suite als Ganzes abgefragt; vier hatten einen privaten Helfer, der immer nur
EIN benanntes Feature stichprobenartig prueft. Die beiden Flaechen sind
disjunkt — `Chamfer 1` von #416 kommt sauber durch den Import und beide
Chamfer-Aufrufe, die es schreibt, werden danach abgelehnt, ohne ein Wort davon
in `result.errors`.

Neu: `regenCensus()` in `importKernelDouble.ts`, beschriftet ueber
`result.idMap` mit dem ONSHAPE-Namen. Die vier privaten Kopien sind durch
`regenErrors()` aus derselben Datei ersetzt. Gemessene Pins, alle sechs Suiten:

| Suite | Features | Regen-Fehler |
|---|---:|---|
| E2E Kabelbox | 19 | 0 |
| E2E #412 | 2 | 0 |
| #409 | 29 | 0 |
| #414 | 15 | 0 |
| #415 | 11 | 0 |
| #416 | 29 | **2** — `Chamfer 1`, beide Aufrufe |
| #419 | 66 | **2** — `Extrude 2`, `Extrude 15` |

**Neuer Befund (#419), abgelegt als #451:** `Extrude 2` und `Extrude 15` werden geschrieben,
angenommen und dann vom Kernel abgelehnt: „sketch has no closed regions to
extrude". Beide stehen NICHT in `KNOWN_GAPS` dieser Suite, beide sind ueber
`result.idMap` direkt beschriftet (nicht abgeleitet). Nicht diagnostiziert.

### 6. Das KONKAV-Urteil war falsch — gemessen, und repariert (v0.47.2)

`edge_cutter_frame` hat ZWEI Gruende, einen harten Cutter abzulehnen, und hat
beide `Concave` genannt:

1. die zwei MIXED-Quadranten-Sonden — sie lesen die Form des Dihedrals;
2. `node.sdf(inner) > −0,5·reach` — der Rollkugel-Mittelpunkt liegt `reach`
   hinter BEIDEN Flaechen und damit ausserhalb des Koerpers.

(2) sagt nichts ueber die Kante, sondern etwas ueber die GROESSE des Cutters.
Am selben, unveraenderten konvexen Aussenkanten-Dihedral gemessen
(`f449_edge_cutter_gate_separates_concave_from_oversized`):

| Geometrie | 0,5 mm | 1 mm | 1,5 mm | 2 mm | 3 mm | 5 mm |
|---|---|---|---|---|---|---|
| 3 mm dicke Platte | OK | OK | OK | OK | **abgelehnt** | **abgelehnt** |
| 20 mm dicke Platte | OK | OK | OK | OK | OK | OK |
| echte Innenkante | **abgelehnt** | **abgelehnt** | — | — | — | **abgelehnt** |

Ein Urteil, das bei unveraenderter Geometrie mit `reach` kippt, ist kein
Konvexitaets-Urteil. Die Konvexitaets-Sonde fragt jetzt ZUERST (sie misst nur
die Ecke innerhalb `eps`, nicht wie weit das Material dahinter reicht), die
Passt-der-Cutter-Sonde danach, und ihr Ergebnis heisst `LargerThanMaterial`.

Das Gate **entscheidet bitgleich wie vorher** — dieselbe Menge wird angenommen,
dieselbe abgelehnt, nur der Grund stimmt. Die Schutz-Eigenschaft (Gate-Fail →
kein harter Cutter, nie falsche Geometrie) bleibt unveraendert, `id_hash`es
bewegen sich nicht, also kein `NAMING_SCHEMA_VERSION`-Bump.

Am echten Dokument: #416s `Chamfer 1` ist eine 5-mm-Fase auf einer **3 mm
dicken** Platte (Nachbarnormalen (−1, 0, 0) und (0, 0, 1) — eine gewoehnliche
Aussenkante). Meldung vorher „sie ist KONKAV", jetzt „die Fase (5 mm) ist
groesser als das Material an dieser Kante". Koerperzahl und Fehlerzahl
unveraendert; das Feature geht weiterhin nicht (P3 in
`docs/FILLET-CUTTER-PLAN.md` bleibt offen und ist NICHT dieser Fall).

### 7. Methodik-Lehre

Der bisherige Merksatz war „Ground Truth kommt von Onshape, nie von uns"
(#412). Er reicht nicht: **Onshape beantwortet zwei verschiedene Fragen, und
der bequemere Endpunkt beantwortet die falsche.** Fuer Ausdehnungen gilt ab
jetzt `evBox3d(tight: true)` oder die Tessellierung, und ein Pin, dessen
Begruendung nicht nachgemessen wird, verrottet auch dann, wenn er rot bleibt.

## Update 2026-07-29 (11) — zwei gleiche Meldungen, zwei verschiedene Luecken (#451, v0.47.3)

Update (10) §5 legte zwei Namen ab, die den Import sauber passieren und danach
vom Kernel abgelehnt werden — `Extrude 2` und `Extrude 15` des
#419-Dokuments, beide mit **derselben** Meldung („sketch has no closed regions
to extrude") und beide undiagnostiziert. Das Ticket nannte drei Hypothesen:
(a) das Profil-Query loest auf eine leere Region auf, (b) die Skizze traegt
keine geschlossene Region, (c) `resolveRegionFaces` greift daneben.

Gemessen ist es **zweimal (b)** — und die zwei (b) haben nichts miteinander zu
tun. Dass eine gemeinsame Meldung eine gemeinsame Ursache nahelegt, war die
achte widerlegte Eingangsthese dieser Serie.

### 1. Die Sonde: was jede der beiden Skizzen wirklich enthaelt

Nach dem Import `listSketchRegions` auf der jeweiligen Traeger-Skizze, dazu die
Onshape-Query dekodiert und die Uebersetzungs-Diagnostik gelesen:

| | `Extrude 2` | `Extrude 15` |
|---|---|---|
| Onshape-Query | `qSketchRegion(id, filterInnerLoops)` — ganze Skizze | `qCompressed(...)`, dekodiert vollstaendig |
| Traeger-Skizze | `Sketch 2` (`F2ySW2Y4xxbAo0M_1`) | `Sketch 12` (`FHk6N0R8R7Mprgx_13`) |
| Regionen bei uns | 0 | 0 |
| Nicht uebersetzt | `BTCurveGeometryEllipse-1189` ×1 | nichts |

`Sketch 2` traegt fuenf **Konstruktions**linien, einen Punkt — und genau eine
Nicht-Konstruktionskurve: eine **Ellipse**. `translateOnshapeSketch` las Linien
und Kreise; die Ellipse fiel in den `unhandled`-Zaehler. Beim Kernel kam damit
nichts an, was eine Flaeche umschliesst. (a) und (c) scheiden aus: die Query
waehlt gar keine einzelne Region, sie nennt die ganze Skizze.

`Sketch 12` traegt tatsaechlich **eine einzige Linie**, und das ist kein
Uebersetzungsverlust — Onshape hat dort nur diese eine Kurve. Der dekodierte
Regionenrand nennt sie mit `side: 1`, und alles Weitere im selben
TOPOLOGY-Block sind Kanten des **Koerpers**, auf dem die Skizze steht:
`CAP_EDGE`, zwei `SWEPT_EDGE`, ein `MERGE`.

### 2. Reparatur (nur fuer die belegte Ursache): die Ellipse

Der Kernel hat `sketch_ellipse` seit DL-49 (geschlossener Quad-Bezier-Pfad,
10 µm Toleranz). Es fehlte **ein Zweig im Web-Importer**, an zwei Stellen im
Gleichschritt:

- `onshapeSketch.ts` — `BTCurveGeometryEllipse` → `sketch_ellipse`. Onshapes
  `radius` IST die grosse Halbachse und liegt auf `xDir`/`yDir`, `minorRadius`
  steht senkrecht darauf; das ist genau FieldDrafts
  `(radius_x, radius_y, rotation)`-Frame, also eine Umbenennung plus ein
  `atan2`. Die Id-Vergabe ist **drei** Entities (Mitte, **Fokus**, Ellipse) —
  PlaneGCS parametrisiert eine Ellipse ueber einen Fokus, und der Kernel legt
  ihn zwischen Mitte und Kurve an. Zwei statt drei haetten jede spaetere Id
  verschoben.
- `onshapeImport.ts::sketchEntityVertices` — der geflachte Rand, gegen den die
  Region-Aufloesung „links dieser Kante" misst. Er laeuft in der **eigenen
  Haendigkeit** der Ellipse (`clockwise`), dieselbe Lehre wie bei den
  gespiegelten Vollkreisen (#446).

Elliptische **Boegen** bleiben bewusst offen: FieldDraft hat keinen, und eine
gebogene Ellipse zur ganzen zu schliessen erfaende Flaeche, die niemand
gezeichnet hat. Sie werden als `BTCurveGeometryEllipse-1189(arc)` gemeldet.

### 3. Was `Extrude 2` wirklich ist — und warum die SDF es nicht zeigt

`Sketch 2` hat jetzt **eine** Region: 438,6 mm² (π·a·b = 438,42 mm², der
Bezier-Pfad liegt 0,04 % darueber), AABB ±13,481 × [−1,302; 19,402] mm — die
Ellipse mit a = 13,4885, b = 10,3459 mm um (0; 9,05), um 2,34° gekippt.

Onshapes eigene Tiefe fuer dieses Extrude ist `0.01 mm`, **symmetrisch**: ein
±5-µm-Ritz, keine Tasche. Entsprechend:

- die AABB des geschnittenen Koerpers bewegt sich **nicht**;
- `bodySdfEval` liest **bitgleich** mit und ohne das Feature — die SDF traegt
  einen Schnitt unterhalb ihrer eigenen Bautoleranz (10 µm) nicht;
- die B-Rep traegt ihn: der 25,4 × 25,4 × 18,1-mm-Block geht von
  6 Flaechen / 12 Kanten / 8 Ecken auf 7 / 14 / 12.

Vier dieser sieben Flaechen kommen mit **unverbundenem** Aussenring heraus
(`openFaces` 0 → 4). Das ist die ehrliche Antwort des Kernels auf einen
Schnitt, der duenner ist als seine Toleranz — der Import ist treu, das Artefakt
liegt dahinter. Gepinnt ist es exakt, damit eine spaetere Verbesserung als
Pin-Bewegung sichtbar wird.

**Kein `NAMING_SCHEMA_VERSION`-Bump:** der Kernel wurde nicht angefasst, die
Aenderung ist rein im Web-Importer.

### 4. Was bleibt: eine Region, die Onshape gegen die TRAEGERFLAECHE schliesst

Onshape schliesst eine Sketch-Region gegen den Rand der **Flaeche**, auf der
die Skizze liegt: ein Strich quer ueber eine Planflaeche teilt sie in zwei
waehlbare Regionen. FieldDrafts Regionenfindung kennt nur die Sketch-Kurven
selbst — eine einzelne Linie umschliesst nichts.

Das ist ein **Modell**-Unterschied, keine fehlende Uebersetzung: kein
Kurventyp fehlt, keine Query bleibt unlesbar. Zu schliessen nur, indem die
Traegerflaeche mit ins Arrangement geht, was auch die Wand-Buchhaltung
(`qWall(...)`) beruehrt. Abgelegt als **#452**, auf diesem Dokument haengt
genau ein Feature daran.

### 5. Zahlen, gemessen

| | vorher (v0.47.2) | nachher (v0.47.3) |
|---|---:|---:|
| Regen-Fehler #419 | **2** (`Extrude 2`, `Extrude 15`) | **1** (`Extrude 15`) |
| Import-Fehler #419 | 169 Meldungen / 88 Features | 169 / 88 |
| Koerperzahl #419 | 18 | 18 |
| Szenen-AABB #419 | x max 71,481 | 71,481 |
| `Revolve 1` Profil | `qRegions(6, [6, 8, 9])` | unveraendert |
| `Extrude 4` Profil | `qRegions(6, [7, 9, 12])` | unveraendert |
| `Sketch 3` Regionen | 15 | 15 |

`Sketch 3` traegt drei Ellipsen und bewegt sich **trotzdem nicht** — es sind
elliptische BOEGEN (`BTMSketchCurveSegment`), die weiterhin offen bleiben. Die
scharf gepinnten Flaechen von `Revolve 1` (47,57 / 62,65 mm²) und die
Regionen-Indizes stehen deshalb bitgleich. Das war nicht geplant, sondern
gemessen; ein Volltreffer auf die eine Skizze, die es brauchte.

### 6. Methodik-Lehre

**Eine gemeinsame Fehlermeldung ist kein gemeinsamer Befund.** Beide Features
lasen wortgleich „sketch has no closed regions to extrude"; die eine Ursache
war ein fehlender Zweig von zwanzig Zeilen, die andere ein Modell-Unterschied
mit eigenem Ticket. Der Zensus, der sie zusammen ablegte (Update (10) §5), war
richtig — er gruppiert nach Symptom, und genau deshalb muss die Diagnose
danach jeden Namen einzeln aufmachen.

## Update 2026-07-29 (12) — die 0,1675 mm sind eine RUNDUNG, die wir nicht schneiden (#450)

Ticket #450 hatte eine Spur mitbekommen — „zuerst die Endbedingung von
`Extrude 9`, dann den Keilwinkel" — und ausdruecklich dazugeschrieben, dass
sie NICHT vermessen ist. Sie war auch falsch. `Extrude 9` ist exakt; die
0,1675 mm sind der Unterschied zwischen einer scharfen Keilspitze und einer
gerundeten.

### 1. `Extrude 9` trifft auf 1,8e-6 mm — drei unabhaengige Quellen

Die Keilspitze liegt bei **z = −73,16882881**. Das sagen drei Antworten, von
denen keine von einer anderen abhaengt:

| Quelle | Wert (mm) |
|---|---:|
| Onshapes Sketch-3D-Antwort (`onshape-416-sketches3d.json`, der Eckpunkt selbst) | −73,16882880644965 |
| Onshapes eigene FLAECHEN-Ebenen (`evSurfaceDefinition` fuer `qCreatedBy(Extrude 9)`), geschnitten | −73,16882881 |
| unsere SDF (`bodySdfEval`, Bisektion der Saeule x = 3, y = −10,1) | −73,1688271 |

Der Sketch-Ursprung von `Sketch 6` sitzt bei (3; −10,1; −65) — runde Zahlen,
weil der Mate-Connector auf der Mitte einer Kante liegt. Die Skizze traegt ein
Viereck (0;0) → (−48,4946;0) → (−44,8889;2,4573) → (0;8,1688), und
−65 − 8,168828806 = −73,168828806. Der Extrude selbst kann `z min` gar nicht
bewegen: seine Normale ist ±y (die Kappen liegen bei y = −12,6 und y = −7,6,
zusammen 5,0 mm = Onshapes `depth` bei `symmetric` — unsere Lesart „±depth/2"
ist damit gegen das Dokument bestaetigt). Gemessen: **d(z min)/d(depth) = 0**,
bitgleich bei halbierter Tiefe.

### 2. Onshape kommt nur deshalb nicht so tief: die Spitze ist GERUNDET

Onshapes tiefster VERTEX liegt bei −72,97326 — ueber seinem eigenen
`z min` = −73,00127. Der Tiefpunkt gehoert also keiner Ecke, sondern
**SPLINE-Flaechen**, und die stammen laut `qCreatedBy` von:

- `Fillet 3` (`FWgfYj1eBr1J6Hd_1`), r = **1,3647572 mm**, `crossSection: CURVATURE`
  — die mittlere Blend-Flaeche, `z min` −73,00127404;
- `Fillet 2` (`FsxE0XiOVUgMBWN_2`), r = 1,2071068 mm — die zwei Eckstuecke an
  den Enden, −73,00131 / −73,00128.

Die Rechnung stimmt bis auf die Tangenten: die Spitzenkante ist die Kante
zwischen der Aussenwand x = 3 und der langen Keilunterseite
(Normale (−0,1262; 0; −0,9920)); der Rollkugel-Mittelpunkt fuer r = 1,3647572
liegt bei z = −71,61942246, und exakt dort endet die Blend-Flaeche nach oben
(gemessene maxCorner z = −71,61942246). Es ist also genau dieser Fillet.

### 3. Wir schneiden ihn nicht — und zwar KEIN einziger Fillet dieses Dokuments

Der Import ist an dieser Stelle nicht schuld: `Fillet 3` schreibt drei
`fillet_edge`-Aufrufe, und der dritte,

    {"blend_radius":1.3647571568806205,"edge":"qCommonEdge(qWall(12, 9), qWall(12, 12))"}

loest auf **genau eine** Kante auf — 4,99986 mm lang, an (3; −10,1; −73,16883),
also die Spitzenkante — und `listFeatures` meldet **keinen** Regenerationsfehler.
Trotzdem ist das Feld unveraendert:

- `setFeatureSuppressed(15, true)` → AABB und SDF **bitgleich**;
- `blend_radius` auf 5 mm gedreht → **bitgleich**;
- alle zwoelf `fillet_edge`-Aufrufe unterdrueckt → **bitgleich**.

Die Ursache steht im Kernel, nicht im Web: `csg::edge_cutter_frame` hat ein
P1-Gate `cdot.abs() > 2e-2 → NonOrthogonal` (`docs/FILLET-CUTTER-PLAN.md` §4/§5).
An dieser Kante ist `cdot = n_a·n_b = −0,126219` (Innenwinkel 82,75°), also
abgelehnt. Der Rueckfallpfad ist der alte `smin`-MaskedBlend — und der braucht
eine Boolean-Fuge zwischen `feature_a` und `feature_b`. Beide Waende gehoeren
hier demselben Feature (`qWall(12, 9)` und `qWall(12, 12)`), es gibt keine Fuge,
und damit passiert nichts: das ist woertlich der #406-Mechanismus, nur an einer
schiefen statt an einer Wuerfelkante.

### 4. Was ein Cutter brauchte — und was danach uebrig bleibt

Mit r = 1,3647572 an dieser Kante gerechnet (2D im xz-Schnitt, Ecke bei
(3; −73,1688288)):

| Querschnitt | tiefster Punkt (mm) | Abstand zu Onshape |
|---|---:|---:|
| scharf (heute) | −73,1688288 | 0,1675 zu tief |
| Kreis (Rolling Ball, unser Cutter) | −72,98418 | 0,0171 zu flach |
| Konik rho = 0,5 | −72,99508 | 0,0062 zu flach |
| Onshape (`CURVATURE`) | −73,00127404 | — |

Ein kreisfoermiger Cutter holt also **rund 90 %** der Abweichung und laesst
0,0171 mm stehen, deren Ursache dann benannt ist: Onshapes Querschnitt ist
kruemmungsstetig, unserer ein Kreisbogen. Der Importer sagt das bereits
(`Fillet 3: CURVATURE-Querschnitt → kreisfoermig uebernommen`) — er sagt es nur
bisher folgenlos, weil gar nichts geschnitten wurde.

### 5. Methodik-Lehre

**Eine zu WEIT reichende Grenze kann fehlendes Wegnehmen sein.** Alle bisherigen
#416-Runden haben nach fehlendem MATERIAL gesucht; hier fehlt eine
Materialabnahme. Und sie fehlt lautlos: die Query loest auf, das Feature
regeneriert fehlerfrei, der Zensus ist sauber — nur die Geometrie bewegt sich
nicht. Ein Feature, das nichts tut und nichts sagt, ist von einem Feature, das
funktioniert, nur an der Geometrie zu unterscheiden.

### 6. Behoben: EINE Box, in der Winkelhalbierenden aufgespannt (v0.47.4)

Der Plan (`docs/FILLET-CUTTER-PLAN.md` §5) hatte fuer P2 ein schiefes
Parallelepiped aus drei Slab-Boxen vorgesehen. Das braucht es nicht. Rechnet man
im Rahmen der WINKELHALBIERENDEN `m = normalize(n_a + n_b)` statt in den beiden
Flaechennormalen, so liegen die zwei Beruehrpunkte der Rollkugel symmetrisch bei

    m = −r·sin²φ / cos φ,   s = ±r·sin φ        (φ = halbe Dieder, cos 2φ = n_a·n_b)

und EINE orientierte Box von dieser Sehne bis ueber die Kante hinaus enthaelt die
wegzuschneidende Kappe exakt: ihre innere Flaeche IST die Sehne des Bogens, liegt
also ueberall dort im Ball, wo Material ist, und kann keine Stufe schneiden; ihre
seitlichen Flaechen sind gar nicht bindend, weil Material jenseits der
Beruehrpunkte hinter der Sehne liegt.

An der rechtwinkligen Kante ist das die ANDERE HAELFTE der alten `r × r`-Eck-Box
— der Unterschied zwischen beiden ist das Kreissegment, und das zieht der
Zylinder ohnehin wieder ab. Gleiche Geometrie, gleiche Zahl von Blaettern,
gleiche Sub-Ids: **kein `NAMING_SCHEMA_VERSION`-Bump**, und alle 1376 Kernel- und
48 Slow-Tests liefen unveraendert durch.

Die Fase erbt es mit einer Zeile Trigonometrie (`inset = d·sin φ` statt `d/√2`),
weil ihre beiden Rueckspruenge ebenfalls gleich weit auf der Winkelhalbierenden
liegen. Neu ist nur eine Entartungs-Schranke `|n_a·n_b| ≤ 0,98`: jenseits davon
stehen die Flaechen fast flach oder auf Messerschneide, die Sehne laeuft mit
`1/cos φ` davon.

### 7. Zahlen, gemessen

| | vorher (v0.47.3) | nachher (v0.47.4) | Onshape |
|---|---:|---:|---:|
| tiefster Punkt des Keils (SDF-Bisektion) | −73,16883 (scharf) | **−72,98413** | −73,00127 |
| Rollkugel-Rechnung fuer r = 1,3647572 | — | −72,98418 | — |
| Abstand zu Onshape | 0,1675 zu tief | **0,0171 zu flach** | — |
| `sceneAabb` z min | −73,16883 | −73,16883 | — |
| Koerperzahl / uebrige fuenf Grenzen | 1 / exakt | 1 / exakt | — |
| Import-Fehler #416 | Chamfer 2, Fillet 2, Fillet 5 | unveraendert | — |
| Regen-Fehler #416 | 2× Chamfer 1 | unveraendert | — |

Der Rest von 0,0171 mm hat eine eigene, gemessene Ursache und ein eigenes
Ticket: Onshapes Querschnitt ist `CURVATURE` (kruemmungsstetig), unserer ein
Kreisbogen — **#453**.

`sceneAabb` bewegt sich NICHT und wird es nie: `CsgNode::aabb` gibt bei
`Difference` den linken Operanden zurueck, ein harter Cutter ist eine
`Difference`, also ist die Szenen-Box fuer jede Rundung STRUKTURELL blind. Der
z-min-Pin der #416-Suite misst deshalb seit diesem Ticket die SDF: erst ein
0,5-mm-Zensus ueber die ganze Grundflaeche (wo ist ueberhaupt noch Material unter
−72,95?), dann Bisektion in der gefundenen Saeule. Fuenf der sechs Grenzen duerfen
weiter aus `sceneAabb` kommen, weil dort nichts geschnitten wird — aber das ist
ein Befund, keine Eigenschaft der Box.

### 8. Zweite Methodik-Lehre

**Eine lose Box kann eine Verbesserung genauso verstecken wie eine falsche.**
Update (10) hat gelernt, dass Onshapes lose Box keine Ground Truth ist; hier
war UNSERE lose. Nach dem Fix lief die #416-Suite gruen durch, ohne dass sich
eine einzige Zahl bewegt haette — der Pin fragte `sceneAabb`, und die kann eine
Subtraktion nicht sehen. Ein Test, der ueber eine konservative Schranke misst,
kann nur nach EINER Seite Auskunft geben, und in welche Richtung das ist, muss
dastehen.

## Update 2026-07-29 (13) — der Zaun steht, `Extrude 15` steht trotzdem nicht (#452)

Ticket #452 hatte eine These: eine Sketch-Region, die Onshape gegen die
TRAEGERFLAECHE schliesst, hat bei uns keinen Rand, und zu schliessen sei das
nur, indem die Berandung dieser Flaeche als zusaetzliche, nicht-extrudierbare
Zaunkurven mit ins Arrangement geht. Der erste Teil stimmt und ist jetzt gebaut.
Der zweite — dass DAMIT `Extrude 15` faellt — stimmt nicht: das Feature haengt
an einer zweiten, davon unabhaengigen Luecke, die vor dem Zaun liegt.

### 1. Die Messung vorher, reproduziert

| | Wert |
|---|---:|
| Regen-Zensus #419 | **1** (`Extrude 15`), Meldung „sketch has no closed regions to extrude" |
| Import-Fehler | 169 Meldungen / 88 Features |
| Koerperzahl | 18 |
| `sceneAabb` x max | 71,4809 |
| Import-Dauer | 56,4 s |

Von den 17 Skizzen des Dokuments stehen **elf** auf einer echten B-Rep-Flaeche
(`planeFaces = 1`); die uebrigen sechs auf einer Ursprungsebene oder einem
Mate-Connector, die beide UNENDLICH sind und deshalb keinen Rand haben.

### 2. Was der Zaun ist — und was er ausdruecklich nicht ist

Eine Skizze auf einer Flaeche bekommt in `SketchFeature::execute` die
projizierte Berandung dieser Flaeche als zusaetzliche Sub-Calls angehaengt,
markiert mit `fence: true`. Die drei Sorten Sub-Call stehen damit in einer
Tabelle:

| | Regionenfindung | extrudierbares Profil |
|---|---|---|
| normal | ja | ja |
| `construction` | nein | nein |
| `fence` | ja | **nein** |

Zwei Entscheidungen tragen das Ganze:

**Der Zaun kann SCHLIESSEN, nie SCHNEIDEN** (`trim_fence_to_open_area`). Vor
dem Arrangement wird jede Zaunkurve an den gezeichneten Kurven aufgeteilt und
jedes Stueck verworfen, das durch eine Flaeche des NUR-GEZEICHNETEN
Arrangements laeuft. Ohne das waere der Preis dieser Aenderung enorm gewesen:
`Sketch 4` (44 × 44 mm Platte, 15 Regionen) steht auf einer 38 × 38 mm grossen
Wand, deren Rand mitten durch die Platte laeuft — ungetrimmt zerfiel die Skizze
in 19 Regionen, drei alte in je zwei Stuecke geschnitten (255 → 173,31 + 81,69;
237 → 159,81 + 77,19; 1337,62 → 738,18 + 599,44). Getrimmt bleibt **jede alte
Region bitgleich**, in derselben Reihenfolge; hinzu kommt nur, was vorher gar
nicht berandet war. Das ist kein Komfort, sondern eine Korrektheitsfrage: die
38 × 38-Wand ist UNSERE Zerlegung, nicht Onshapes — Onshape faltet koplanare
Flaechen zu einer (#443) — und eine Naht, die es nur bei uns gibt, darf keine
Region schneiden.

**Die Traegerflaeche ist der koplanare FLICKEN, nicht die eine benannte
Teilflaeche.** `resolve_carrier_fence` sammelt jede ebene Flaeche desselben
Koerpers, die in der Skizzenebene liegt, und entfernt die Naehte zwischen
ihnen: eine Kurve, die auf BEIDEN Seiten Flickenmaterial hat, ist innen und
kein Zaun. Auf `Sketch 12` sind das zwei Wandstuecke (x ∈ [−1,5; 1,5] und
x ∈ [1,5; 33,6]), die zu EINEM Rechteck x ∈ [−1,5; 33,6] × y ∈ [−22,25; 22,25]
zusammenwachsen — genau der `MERGE`, den Onshapes Ebenen-Query nennt.

Eine Region, deren AEUSSERE Schleife nur aus Zaunkurven besteht, ist keine
Region: das ist die Traegerflaeche selbst, die die Skizze nirgends gezeichnet
hat, und sie ist als FLAECHEN-Profil (`qWall(…)` / `qFace(…)`, der #442-Pfad)
ohnehin und besser benennbar.

### 3. Was sich bewegt hat

Auf der aufgezeichneten #419-Wiedergabe (`onshape419_full_import.json`, 49
Kernel-Aufrufe) gewinnen drei Skizzen je EINE Region, und zwar an Index 0, was
alle folgenden um eins verschiebt:

| Skizze | vorher | nachher |
|---|---|---|
| 16 | 2 (1391,94 / 1449,94) | 3 (**37,94** / 1391,94 / 1449,94) |
| 26 | 1 (1936,0) | 2 (**711,38** / 1936,0) |
| 30 (= `Sketch 4`) | 14 | 15 (**676,85** vorangestellt) |
| 38 | 5 | 6 (**348,23** vorangestellt) |
| 43 | 1 (60,5) | 2 (**1408,63** / 60,5) |
| 6 (Mate-Connector) | 15 | 15, bitgleich |

Die Indizes in der Aufzeichnung wurden mitgezogen (sieben Aufrufe), weil die
Datei laut ihrem eigenen Kopf die Folge ist, die der IMPORTER erzeugt. Dass
danach der Offene-Flaechen-Zensus `[0,0,0,0,0,0,2,0,0,1,4,0]` wieder exakt
herauskommt, ist die Gegenprobe zum Trimmen: haette der Zaun irgendwo
geschnitten, brachte keine Umnummerierung diese zwoelf Zahlen zurueck.

### 4. Und trotzdem: `Extrude 15` faellt nicht

`Sketch 12` hat nach dem Zaun **sechs** Zaunkurven und **null** Regionen. Der
Grund steht in den Zahlen:

    Traeger (Zaun, 2D):  x ∈ [−1,5; 33,6]   y ∈ [−22,25; 22,25]
    gezeichnete Linie:   (−68,53; 23,05) → (−36,43; −16,83)

Die Linie liegt 35 mm neben ihrer eigenen Traegerflaeche. Sie kann dort nichts
schliessen, und der Zaun ist daran unschuldig: die 2D-Koordinaten der Linie
sind **bitgleich Onshapes eigene Sketch-Koordinaten** (aus `pntX/pntY`,
`dirX/dirY` und `startParam/endParam` nachgerechnet: (−68,52731558; 23,05) und
(−36,42731558; −16,832859459) — genau das, was im Sub-Call steht). Es wurde
also gar keine In-Plane-Abbildung angewandt.

Das ist die #415-Rueckfallebene, und sie greift zu Recht:
`measureOnshapeSketchFrame` braucht DREI nicht-kollineare 2D↔3D-Paare, um
Drehung und Verschiebung zu schaetzen. `Sketch 12` besteht aus EINER Linie, also
aus zwei Punkten, also kollinear — das Gleichungssystem ist entartet, die
Funktion liefert `null` statt eines geratenen Frames, und der Payload bleibt
woertlich stehen. Onshapes eigene 3D-Antwort sagt, wo die Linie wirklich liegt:
Weltpunkte (−13,011; 74,627; 23,55) → (−13,011; 42,527; −16,833).

`Extrude 15` haette also auch mit Region ein Loch an der falschen Stelle
gefraest. Die Luecke, an der es haengt, ist nicht die Regionenfindung, sondern
der Sketch-FRAME.

### 5. Was ein Frame aus ZWEI Punkten braucht

Kollinear heisst nicht unbestimmt. Ist die Ebene bekannt (und sie ist es — die
Traegerflaeche liefert Normale und Ursprung), so bestimmen zwei Korrespondenzen
die In-Plane-Drehung bis auf eine SPIEGELUNG an der Linie selbst. Fuer die Lage
der Linie ist diese Spiegelung gegenstandslos — genau das Argument, mit dem
#442 den Ein-Punkt-Fall konzentrischer Kreise geloest hat („die Drehung ist
nicht bloss unbekannt, sie ist gegenstandslos").

Hier ist sie es aber NICHT ganz: Onshape sagt zu seiner Region `side: 1`, also
LINKS der Kurvenrichtung, und links/rechts kippt unter der Spiegelung. Welche
der beiden Haelften der Traegerflaeche gemeint ist, haengt damit an der
Haendigkeit — und die ist aus zwei Punkten nicht messbar. Eine angenommene
Haendigkeit waere geraten, und ein geratener Halbraum ist bei einem REMOVE ein
Loch an der falschen Stelle.

Zwei Auswege stehen offen, beide messend statt ratend:

- Die Haendigkeit an den Skizzen desselben Dokuments ablesen, deren Frame
  vermessen WERDEN kann, und pruefen, ob sie fuer flaechengebundene Skizzen
  einheitlich ist. Ist sie es, ist die Uebernahme eine Induktion aus einer
  Messung, keine Annahme.
- Die uebrigen Randkanten von Onshapes Region-Query verwerten. `Extrude 15`
  nennt neben der Linie eine `CAP_EDGE` von `FbKE2kiRYXa1rde_13.opExtrude` und
  eine `MERGE`-Kante — beide gehoeren zu einem BENANNTEN Feature. Welche der
  zwei Haelften an dessen Wandstueck grenzt, ist eine Messung an unserer
  eigenen Geometrie. `regionBoundary` verwirft solche Kanten heute; sie
  aufzuheben ist die generische Fassung dieses Falls und trifft auch die 26
  weiteren Region-Queries dieses Dokuments, die Sketch-Kurven mit Koerperkanten
  mischen.

### 6. Methodik-Lehre

**Ein fehlender Rand kann eine fehlende LAGE sein.** Der Zaun war die richtige
Antwort auf die richtige Frage — die Region fehlt wirklich, und sie fehlt aus
genau dem Grund, den das Ticket nennt. Nur macht sie das Feature nicht ganz:
was fehlt, ist auch der Frame, und das faellt erst auf, wenn man nach dem
Einbau nicht die Meldung, sondern die KOORDINATEN nachrechnet. Die Meldung war
bis zuletzt dieselbe.

Dass die #451-Lehre („eine gemeinsame Fehlermeldung ist kein gemeinsamer
Befund") hier eine Stufe tiefer noch einmal gilt, ist kein Zufall: eine
Fehlermeldung nennt den Ort des Abbruchs, nie die Ursache.

## Update 2026-07-29 (14) — der Frame aus zwei Punkten, und die dritte Schicht darunter (#452)

Update (13) hat zwei messende Wege aus der Frame-Luecke benannt und Weg 2 (die
verworfenen Koerperkanten der Region-Query) als die generische Fassung. Diese
Runde ist beiden nicht gefolgt, und zwar aus einem gemessenen Grund: beide
loesen die HAENDIGKEIT, und die Haendigkeit ist nicht, woran `Extrude 15`
haengt. Unter der Frame-Luecke liegt eine dritte, und sie liegt vor beiden.

### 1. Gebaut: der Frame aus zwei kollinearen Stuetzpunkten

`measureOnshapeSketchFrame` braucht drei nicht-kollineare 2D↔3D-Paare, weil es
Drehung und Verschiebung FREI schaetzt. Nimmt man die Ebene dazu — und die
Traegerflaeche liefert sie —, bleiben von sechs Freiheitsgraden drei, und eine
Richtung plus ein Punkt bestimmen alle drei:

    θ = atan2(e_y, e_x) − atan2(d_y, d_x)

mit `d` Onshapes 2D-Richtung und `e` derselben Richtung in Weltkoordinaten, auf
FieldDrafts In-Plane-Achsen projiziert. Nicht bestimmt ist die HAENDIGKEIT: die
Spiegelung an eben dieser Geraden bildet beide Punkte auf sich ab, jede
Gegenprobe an ihnen ist gegen sie blind. Angenommen wird `det = +1`, und das
ist keine Raterei, sondern die Regel, die dieselbe Datei seit #446 an ihren
Daten misst: ueber die fuenf Dokumente mit 3D-Antwort tragen 29 Sketches einen
aufloesbaren Frame, 26 davon mit `det = +1`, und die drei Ausreisser sind
FLAECHEN-Fehlaufloesungen, die sich selbst melden. Der Import schreibt die
Annahme in die Meldung.

Gemessen: der Pfad greift auf `Sketch 4` des #416-Dokuments (fuenf kollineare
Stuetzpunkte). Alle 48 Slow-Tests inklusive der sechs scharfen #416-Grenzen
bleiben unveraendert.

### 2. Warum er `Sketch 12` trotzdem ablehnt — Onshape gegen Onshape

Die Gegenprobe (jeder gemeldete 3D-Punkt muss aus seinem 2D-Punkt rekonstruiert
werden) schlaegt an, und das ist der Befund dieser Runde:

| | 2D-Payload (`startParam`/`endParam`) | Onshapes 3D-Antwort |
|---|---:|---:|
| Laenge der Linie | 51,1962 mm | 51,5867 mm |
| Anfangspunkt, v bzw. z | 23,050 | 23,550 |
| Endpunkt, v bzw. z | −16,832859459 | −16,832859459 |

Die u-Beziehung stimmt fuer BEIDE Punkte exakt (`u + y = 6,1 mm`, auf 1e-17).
Nur der ANFANGSPUNKT liegt in Onshapes 3D-Antwort um exakt 0,5 mm weiter, und
damit ist die Linie dort 0,3905 mm laenger als ihr eigener Payload sagt.
Zwischen zwei Punktepaaren mit verschiedenem Abstand gibt es keine starre
Abbildung — der Rest der Anpassung ist 0,3057 mm, und die Funktion lehnt ab
statt zu verschieben.

Das ist kein Einzelfall, sondern eine FAMILIE. Ueber alle fuenf Dokumente
gemessen (2D-Abstand gegen 3D-Abstand, je Punktepaar):

| Dokument | Sketches nicht starr | Betrag |
|---|---:|---|
| #419 | 8 | 0,39 / 1,00 / 1,08 / 1,24 / 1,70 / 2,21 / 21,0 / 21,0 mm |
| #409 | 3 | je 50,0 mm |
| #416, #415, #414 | 0 | — |

Alle uebrigen sind exakt starr (0 bis 2e-12 mm). Die Fixture-Paare sind also in
sich stimmig; die Abweichung gehoert diesen Sketches. `measureOnshapeSketchFrame`
lehnt sie laengst ab, nur nannte die Meldung bisher „zu wenige oder kollineare
Stuetzpunkte" — eine ganz andere Ursache. Sie nennt jetzt den Betrag.

### 3. Die dritte Schicht: die Ebene selbst liegt 17 mm daneben

Und selbst mit perfektem In-Plane-Frame faende `Extrude 15` nicht statt, wo
Onshape fraest. Zum Zeitpunkt des Imports gemessen, gegen den Frame, den
`qWall(44, 35)` aufloest:

    outOfPlane = 16,983 mm

Die aufgeloeste FieldDraft-Ebene ist nicht Onshapes Sketch-Ebene, sondern eine
parallele 17 mm daneben — die Fehlerklasse, die `alignSketchFrame` sonst als
„die aufgeloeste FieldDraft-Ebene liegt X mm neben Onshapes Sketch-Ebene"
meldet und die hier stumm blieb, weil es ohne messbaren Frame gar nicht bis zu
dieser Pruefung kam. Ein Extrude auf dieser Ebene schneidet 17 mm neben dem
Ziel, gleich welche Haelfte der Region gewaehlt wird.

Damit fallen beide Wege aus §5 von Update (13) als NAECHSTER Schritt aus: sie
entscheiden, WELCHE Haelfte gemeint ist. Die Haelfte ist nicht das Problem.

### 4. Was dennoch bewiesen ist: der Zaun sitzt richtig

Projiziert man Onshapes eigene 3D-Endpunkte in den aufgeloesten Traeger-Frame,
landet die Linie bei

    u = 1,5000   →   u = 33,6000

und das sind, auf die vierte Stelle, die beiden senkrechten Zaunkurven des
koplanaren Flickens aus Update (13): seine NAHT bei 1,5 und seine rechte Kante
bei 33,6. Die Linie spannt den Traeger also genau von der einen zur anderen —
das ist die Region, die Onshapes Query mit `CAP_EDGE` und `MERGE` beschreibt,
und der Zaun aus (13) schliesst sie. Nur die Ebene, auf der das geschieht, ist
17 mm verschoben.

### 5. Woran es jetzt haengt

In dieser Reihenfolge, weil jede Schicht die naechste voraussetzt:

1. **Die Ebene.** `qWall(44, 35)` muss die Flaeche treffen, auf der Onshape die
   Skizze hat. 16,983 mm ist kein Rundungsfehler, sondern eine andere Flaeche.
   Solange sie falsch ist, ist alles darueber gegenstandslos.
2. **Die Lage in der Ebene.** Mit richtiger Flaeche ist zu entscheiden, ob
   Onshapes 3D-Antwort oder sein 2D-Payload die Linie beschreibt — sie
   widersprechen sich um 0,5 mm. Die 3D-Antwort ist die Ground Truth dieses
   Repos und traegt zusaetzlich die Endpunkte, die den Traeger wirklich
   erreichen; der Payload ist es, was der Sketch-Uebersetzer heute liest.
3. **Die Haelfte.** Erst hier greifen die beiden Wege aus (13) §5.

### 6. Methodik-Lehre

**Eine Luecke, die man geschlossen hat, kann die naechste erst sichtbar
machen.** Der Zaun (13) war noetig, um zu sehen, dass die Linie neben ihrer
Flaeche liegt; der Zwei-Punkte-Frame war noetig, um zu sehen, dass Onshapes
zwei Antworten sich widersprechen; und erst dessen Ablehnung fuehrte zu der
Messung, die die Ebene um 17 mm daneben zeigt. Drei Runden, drei Ursachen, eine
Fehlermeldung — die von Anfang an dieselbe war.

Das ist kein Argument gegen das schrittweise Vorgehen, sondern eines fuer die
Reihenfolge: gemessen wurde jedes Mal ZUERST, und jede Schicht steht seither
mit einer Zahl da statt mit einer Vermutung.

## Update 2026-07-29 (15) — die Ebene ist richtig BENANNT und falsch GELEGT (#452 → #454)

Update (14) hat die Reihenfolge festgelegt: erst die Ebene, dann die Lage
darin, dann die Haelfte. Diese Runde hat die Ebene vermessen. Sie ist kein
Namens-Problem — die beiden bekannten `qWall`-Mechanismen treffen beide nicht —
sondern ein Lage-Problem, dessen Ursache vier Features weiter oben sitzt und
ein eigenes Ticket bekommen hat (**#454**).

### 1. Elfte Widerlegung: es ist KEIN `qWall`-Fehler

Verdaechtigt waren (a) region-eingeschraenkte Profile ohne Entity-Provenienz
(`SKETCH-REGION-IMPRINT-PLAN` §5, letzter Punkt) und (b) der
`POSITIONAL_WALL_FLAG` aus #445, der positionell zurueckgefallene Wand-Tabellen
fuer `qWall` strukturell unauffindbar macht. Gemessen trifft keiner von beiden:

| | Ergebnis |
|---|---|
| `qWall(44, 35)` (`Extrude 5`) | **1** Fläche, `sub = 35`, `POSITIONAL_WALL_FLAG` **nicht** gesetzt |
| `qWall(47, 35)` (`Extrude 14`) | **1** Fläche, `sub = 35`, Flag nicht gesetzt |
| Lage beider | x = +3,972, Normale (−1, 0, 0) — **koplanar** |

Beide Subs sind echte Entity-Ordinale, keine positionellen Indizes. Und die
beiden Waende sind GENAU das Paar, das Onshapes Ebenen-Query als `MERGE`
nennt (`FRLqXQ83tJHM7Lj_13` = `Extrude 5`, `FbKE2kiRYXa1rde_13` = `Extrude 14`,
beide aus der Kurve `pozkE6lurT5A.right` von `Sketch 11`). Die Flaechen-
Identitaet ist einwandfrei — es ist derselbe koplanare Flicken, den der Zaun aus
(13) ohnehin schon zu EINEM Rechteck zusammenfasst.

Der Nebenbefund gehoert notiert: der letzte Punkt von `SKETCH-REGION-IMPRINT-PLAN`
§5 („Region-eingeschraenkte Profile tragen noch keine Entity-Provenienz") ist
seit #424 ueberholt; `docs/ARCHITECTURE.md` sagt es bereits richtig.

### 2. Was stattdessen zutrifft: die Geometrie liegt 16,983 mm daneben

Onshapes Sketch-Ebene fuer `Sketch 12` ist x = −13,011 (aus seiner eigenen
3D-Antwort fuer die Linie). Unter allen **171 ebenen Flaechen** der 18 Koerper
gibt es dort **keine einzige** mit x-Normale; die naechsten liegen bei −15,405
und −17,405. Unsere beiden Waende liegen bei x = +3,972 — 16,983 mm daneben,
und das ist auf die dritte Stelle der `outOfPlane`-Betrag aus Update (14).

Die Kette dahinter ist end-to-end gemessen:

1. `Sketch 11` faellt bei `measureOnshapeSketchFrame` durch und wird WOERTLICH
   uebernommen. Sein FieldDraft-Frame-Ursprung liegt bei x = 16,983, Onshapes
   bei x ≈ 0 — die Geometrie landet um genau diesen Betrag verschoben, IN der
   Ebene (die Ebene selbst stimmt: `outOfPlane` = 1,6e-5 mm).
2. `Extrude 5` und `Extrude 14` stehen auf dieser Skizze, ihre Waende erben den
   Versatz.
3. `Sketch 12` steht auf genau diesen Waenden — seine Traegerflaeche ist damit
   16,983 mm verschoben.
4. `Extrude 15` fraest 16,983 mm neben dem Ziel, gleich was an Region und
   Frame noch repariert wird.

### 3. Warum `Sketch 11` durchfaellt — und dass es nicht muss

`measureOnshapeSketchFrame` verlangt, dass JEDER Stuetzpunkt auf 1e-6 mm
rekonstruiert wird. Verwirft man iterativ den groessten Ausreisser und passt
neu an, ergibt sich:

| Dokument | Skizze | Paare | verworfen | Rest | \|x\| / \|y\| |
|---|---|---:|---:|---:|---|
| #419 | Sketch 11 | 32 | **1** (1,035 mm) | 5,6e-14 mm | 1,000000000 / 1,000000000 |
| #419 | Sketch 20 | 19 | 5 | 4,2e-14 mm | 1,000000000 / 1,000000000 |
| #419 | Sketch 23 / 24 | 16 | 8 | — | 1,0184 / 1,1826 |
| #419 | Sketch 30 | 33 | 23 | — | 0,9952 / 0,9874 |
| #409 | Sketch 2 | 32 | 16 | — | 1,0000 / **1,1000** |
| #409 | Sketch 3 | 32 | 16 | — | 1,0000 / 1,0957 |

Die Familie zerfaellt damit sauber in zwei Klassen, und die Trennlinie ist die
ORTHONORMALITAET der ueberlebenden Basis:

- **(A) ausreisser-verseucht, aber starr**: `Sketch 11` und `Sketch 20`. Nach
  dem Verwerfen ist die Basis auf neun Stellen orthonormal — zehnfach
  ueberbestimmt, also eine Messung. Hier ist ein Frame da und wird verschenkt.
- **(B) wirklich nicht starr**: die uebrigen fuenf. Dort ueberlebt eine
  SKALIERUNG (`|y| = 1,1000` in #409 `Sketch 2`, auf vier Stellen glatt), und
  eine Skalierung ist kein Frame. Die Ablehnung ist dort richtig.

Das ist der Inhalt von **#454**: die Anpassung an die konsistente MEHRHEIT
legen und die verworfenen Punkte BENENNEN, statt die Toleranz aufzuweichen.
`Sketch 11` haengt an einem einzigen Punkt von 32.

### 4. Warum #452 darauf parkt

Die drei Schichten aus (14) §5 sind jetzt alle vermessen, und die unterste ist
nicht #452s: es ist eine familienweite Ursache ueber elf Skizzen in fuenf
Dokumenten, und ihre Reparatur bewegt Frames in Dokumenten, die mit `Extrude 15`
nichts zu tun haben. Sie gehoert unter eigene Messung und eigene Pins — daher
#454. Ist sie behoben, ist #452 wieder eine Frage von zwei Schichten, und beide
sind gebaut: der Zaun (0.48.0) und der Zwei-Punkte-Frame (0.49.0). Offen bleibt
dann nur noch die HAELFTE, mit den zwei Wegen aus (13) §5.

### 5. Methodik-Lehre

**Eine Query kann richtig benannt sein und trotzdem ins Leere zeigen.** Drei
Runden lang war die Vermutung, `qWall(44, 35)` treffe die falsche Flaeche —
zuletzt gestuetzt auf zwei bekannte, real existierende Fehlerklassen des
Wall-Namings. Gemessen trifft die Query GENAU die Flaeche, die Onshape meint;
sie liegt nur woanders. Der Unterschied ist nicht akademisch: gegen ein
Namens-Problem haette man am Query-Pfad gearbeitet, und dort ist nichts kaputt.

Was den Unterschied sichtbar gemacht hat, war eine einzige Frage in
Weltkoordinaten — „welche unserer 171 ebenen Flaechen liegt in Onshapes
Sketch-Ebene?" — mit der Antwort „keine". Ein Name laesst sich nur pruefen,
wenn man weiss, wonach er zeigen SOLL.
