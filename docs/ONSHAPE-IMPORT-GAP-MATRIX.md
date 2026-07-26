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
| #415 | 20 | Feature-Typen **OK** (thicken übersetzt), aber **Geometrie noch falsch**: die Sketch-Frames auf Wandflächen sind gegenüber Onshape um 90° verdreht → Bodenplatte und Deckelausschnitt liegen quer, X/Y der Ground-Truth-AABB vertauscht — s. Update 2026-07-26 (2) |
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
| mirror (Spiegelebenen-Query + entities) | M | 409,411,417,418,419 |
| circularPattern (+ linearPattern) (Achse/Richtung + Anzahl + entities) | M | 409,411,417,418,419 |
| ~~thicken~~ ERLEDIGT (Face-Query → `qFace`, Dicke 1/2, Richtung, Scope) | — | ~~415,417,418,419~~ |
| **Sketch-Frame auf Face-Ebenen** (FieldDraft leitet die u/v-Basis nur aus der Normalen ab, Onshape aus der Flächen-Parametrisierung → 90°-Verdrehung) | M–L | 415 (offen), vermutlich 414 und jedes Dokument mit Sketch auf Wandfläche |
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
4. **P3 Fundament Boolean + Variablen:** ERLEDIGT 2026-07-26 (s. u.). Schließt **kein** Ticket allein — #415 braucht noch `thicken` (P4).
5. **P4 Thicken:** ERLEDIGT 2026-07-26 (s. u.). Schließt **#415 NICHT** — dort ist
   ein neuer, allgemeinerer Blocker aufgetaucht (Sketch-Frame auf Wandflächen).
   Entblockt 417/418/419.
6. **P5 Patterns + Mirror** (circular/linear/mirror): schließt mit P1–P4 **#409** und **#419**.
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
