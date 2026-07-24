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

Alles andere fällt in den Sammel-Zweig (:594) → `errors: "Feature-Typ '<x>' wird noch nicht importiert"` (Feature übersprungen, Import läuft weiter).

## Vorkommen aller Feature-Typen (Summe über 13 Docs)

| Feature | Anz. | Status | FieldDraft-Kernel |
| --- | ---: | --- | --- |
| extrude | 238 | OK | vorhanden |
| newSketch | 225 | OK | vorhanden |
| assignVariable | 119 | FEHLT | Kernel HAT setVariable (api/variables.rs) — nur Wiring |
| fillet | 88 | OK | vorhanden |
| chamfer | 38 | FEHLT | KEIN Edge-Chamfer-Feature (nur SDF-BlendMode::Chamfer) |
| booleanBodies | 27 | FEHLT | Kernel HAT boolean — nur Wiring |
| thicken | 25 | FEHLT | Kernel HAT thicken — nur Wiring |
| mateConnector | 22 | FEHLT | Kernel HAT mate_connector — nur Wiring (Referenzgeo) |
| revolve | 20 | FEHLT | Kernel HAT revolve — nur Wiring (Achsen-Query) |
| mirror | 19 | FEHLT | Kernel HAT mirror — nur Wiring |
| circularPattern | 16 | FEHLT | Kernel HAT circular_pattern — nur Wiring |
| loft | 16 | OK (SOLID) | 6× SURFACE werden übersprungen |
| transform | 11 | FEHLT | kein Body-Transform-Feature |
| deleteBodies | 10 | FEHLT | kein Gegenstück (kann als No-op/Suppress laufen) |
| hole | 9 | FEHLT | Kernel HAT hole — nur Wiring (Plan: docs/HOLE-ONSHAPE-PLAN.md) |
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
- hole style: C_SINK 4, SIMPLE 5, (unspez.) 6.
- loft bodyType: SOLID 10 (ok), **SURFACE 6 (übersprungen** — alle in #418).
- Ausdrücke: 6004 literal-parsebar, **153 Variablen/Formeln** (`#Brettstaerke`, `3 mm+#HoleExtraGap`, …) — `mmOf` kann nur Literale, Variablenbezüge fallen auf Defaults zurück → falsche Maße.

## Gap-Matrix (Zeile = Ticket, Zelle = Zustand)

`OK` = alle Feature-Typen unterstützt · Feature-Liste = fehlende Typen (Anzahl)

| Ticket | Feats | Zustand |
| --- | ---: | --- |
| #410 | 18 | **Struktur korrekt** (Shell-Container hohl + Deckel, SDF-verifiziert), aber Import blockiert den Main-Thread ~4–5 min; Fillet-Reste (CAP_EDGE/CAP_FACE, qCommonEdge löst nichts auf) — P0-Livetest 2026-07-24 |
| #412 | 2  | Feature-Typen OK, aber **Geometrie falsch**: Extrude referenziert 4 Sketch-Regionen (qCompressed → SKETCH_ENTITY), Importer extrudiert den ganzen Sketch → ein Block statt Platten — P0-Livetest 2026-07-24 |
| #413 | 6  | **Struktur korrekt** (SDF-verifiziert: Wanne, 2-mm-Wände); Rest: SWEPT_/CAP_EDGE-Fillets übersprungen, Geschwister-Kanten nicht unterscheidbar — P0-Livetest 2026-07-24 |
| #414 | 27 | chamfer(1), hole(1) |
| #416 | 20 | chamfer(2), revolve(1) |
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
| chamfer (Edge-Chamfer-Feature exponieren; interim: als fillet nähern) | M–L | 409,414,416,417,418,419 |
| revolve (Profil + Achsen-Query) | S–M | 411,416,417,418,419 |
| hole + printedHole (Platzierungs-Query + Style; Plan da) | M | 409,414,417,419,421 |
| mirror (Spiegelebenen-Query + entities) | M | 409,411,417,418,419 |
| circularPattern (+ linearPattern) (Achse/Richtung + Anzahl + entities) | M | 409,411,417,418,419 |
| thicken (Face-Query + Dicke) | S–M | 415,417,418,419 |
| sweep (Profil + Pfad-Query) | M | 418,420 |
| helix | S–M | 420,421 |
| mateConnector (Referenzgeo; ggf. reine No-op-Aufnahme) | S | 417,421 |
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

## Empfohlene Paket-Reihenfolge

1. **P0 Quick Wins verifizieren:** ERLEDIGT 2026-07-24 — Ergebnis ernüchternd: nur #413 strukturell korrekt. Neue P0-Folgearbeiten: (a) **Sketch-Region-Query-Auflösung** (qCompressed → sketchEntityIds → Region), ohne die #412 falsch bleibt und vermutlich viele weitere Dokumente stille Falschgeometrie liefern; (b) **#410-Import-Freeze** profilen/fixen; (c) App-Bug #422 ERLEDIGT (v0.28.1, c4f0c619 — view-lose Projekte starten als Iso-Fit auf frische CSG-Bounds, Alt-Pose wird vergessen).
2. **P1 Chamfer + Hole:** schließt **#414** komplett; entblockt zusätzlich 409/416/417/418/419.
3. **P2 Revolve:** mit P1 schließt das **#416**; entblockt 411/417/418/419.
4. **P3 Fundament Boolean + Variablen:** booleanBodies-Wiring + assignVariable→setVariable + var-fähiges `mmOf`. Prerequisite für fast alles; mit P1/P2 nun in Reichweite: 415,419,420.
5. **P4 Thicken:** schließt mit P3 **#415**.
6. **P5 Patterns + Mirror** (circular/linear/mirror): schließt mit P1–P4 **#409** und **#419**.
7. **P6 Sweep + Helix + mateConnector + deleteBodies(No-op) + UP_TO_NEXT:** schließt **#420** und **#421**.
8. **P7 Long-Tail 417/418/411:** transform, cPlane, projectCurves, laserJoint, importDerived, enclose, SURFACE-Loft, threadCreator. Die drei Mega-/Spezialdokumente; zuletzt, weil teuer und wenig Streuwirkung.

Kern-Einsicht: 11 der 26 fehlenden Typen sind **reine Übersetzer-Verdrahtung** — der Kernel kann revolve/sweep/helix/hole/thicken/boolean/circular_pattern/linear_pattern/mirror/mate_connector/Variablen bereits. Echte Kernel-Neuarbeit brauchen nur chamfer (Edge-Feature), transform, cPlane, projectCurves, SURFACE-loft, laserJoint, importDerived, enclose.
