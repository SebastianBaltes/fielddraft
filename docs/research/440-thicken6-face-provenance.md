# #440 — Woher `Thicken 6` seine Fläche nimmt, und warum wir sie nicht haben

Aufgemacht als Vorarbeit zu #438 und #439: beide fertig gemessenen Weave-Reparaturen
sind daran gescheitert, dass die Flächen-Query `qWall(23, 130)` des Imports danach auf
0 Flächen auflöst und `Thicken 6` übersprungen wird.

Dieses Dokument hält fest, was gemessen ist. Es enthält **keine** Reparatur.

Alle Zahlen kommen aus zwei `#[ignore]`-Sonden in
`crates/kernel/src/onshape_regression_tests.rs`:

    cargo nextest run --profile deep --run-ignored all \
        -E 'test(/onshape_440_/)' --no-capture

---

## (a) Was `Thicken 6` im Onshape-Original referenziert

SSOT ist die eingefrorene Feature-Liste `web/src/onshape/fixtures/onshape-419-features.json`.
Kein Netz, kein Konto nötig. `Thicken 6` ist Feature-Index 38, `FpBQazFCYSqdMLd_13`,
`featureStatus: OK`.

Sein `entities`-Query ist unkomprimiert lesbar (BTON, `%B5$`-Präfix) und dekodiert mit
`parseQueryDocument` aus `web/src/onshape/onshapeQuery.ts` zu genau einer Kette:

    SKETCH_ENTITY EDGE  2L94FTnhDuaQ.right   aus  FLZMozrL9HQb6CN_2 (= Sketch 3)
      → SWEPT_FACE      aus  FLX3OZA0PqgN51K_6 (= Revolve 1)
        → COPY          aus  FJSh1Xj9Ce9qqJO_12 (= Boolean 1, keepTools = True)

Die Geometrie dazu, direkt aus Sketch 3 (mm):

| Entität | Form |
|---|---|
| `2L94FTnhDuaQ.right` | Strecke x = 9,2542, y von −46,6773 bis −60,9273 (14,25 lang) |
| `GSq2DBgKgR8T` (Revolve-Achse) | Strecke x = 16,98916, y von −19,4773 bis −46,6773 |
| `2L94FTnhDuaQ.left` | Strecke x = 16,98916, y von −46,6773 bis −62,5273 — **kollinear zur Achse** |

Die Achse liegt also bei x = 16,98916; der Radius einer Sketch-Koordinate ist
`16,98916 − x`. Damit ist die referenzierte Fläche der **Zylindermantel mit Radius
16,98916 − 9,2542 = 7,7350 mm**, über die volle Linienlänge von 14,25 mm.

Unser Kernel baut genau den: Träger 94338 ist
`InfiniteCylinder { radius: 7.7349997 }` mit Identitäts-Placement, Achse = lokale y-Achse.
`qWall(23, 130)` = Branch 23·4096 + 130 = 94338, wobei Aufruf-Op 23 der `Revolve 1` des
Replays ist (Feature-Ids des Replays sind Array-Index + 1) und Entität 130 die
Solver-Id von `2L94FTnhDuaQ.right`.

Der `booleanScope` von `Thicken 6` ist der `SWEPT_BODY` von `FuEM5hJm4ZF6lHu_11`
(= `Extrude 16`) — deckungsgleich mit unserem aufgezeichneten
`targets: qCreatedBy(27)`. `Thicken 6` ist also ein **Schnitt mit einer dünnen
Zylinderschale**, kein Volumen-Feature.

Offen geblieben: Onshape führt `thickness1 = 0,1 mm`, `thickness = 5 mm`,
`thickness2 = 0 mm`. Unser Importer nimmt `thickness1`, also 0,1 mm. Welches Feld
Onshapes Thicken-UI tatsächlich anzeigt, ist aus der Aufzeichnung nicht entscheidbar —
eigener Prüfpunkt, nicht Teil dieses Befunds.

## (b) Die Prämisse des Tickets ist falsch — das Instrument hat gelogen

Das Ticket führt zwei Behauptungen, beide messbar unrichtig.

**„Kümmerfläche mit Flächeninhalt null."** Nein. Beide Kanten, die 94338 nennen, sind
geschlossene Vollkreise mit 48,6003 mm Umfang (= 2π · 7,735), bei y = 52,8273 und
y = 60,9273. Eine Fläche mit Flächeninhalt null hat keine 48-mm-Kreise.

**„on-boundary samples 0."** Diese Zahl stammt aus `carrier_coverage`, und
`carrier_coverage` linearisiert den Träger per zentraler Differenz im AABB-Mittelpunkt
zu einer **Tangentialebene** (`carrier_plane`) und rastert dann diese Ebene. Für einen
planaren Träger ist das exakt. Für einen Zylinder ist es bedeutungslos: eine Ebene
schneidet einen Zylinder in höchstens zwei Geraden, also liest jeder gekrümmte Träger
„0", egal wie viel von ihm Rand ist. Der Geschwister-Träger 94347 ist tatsächlich eine
Ebene (`Plane { normal: [0,1,0] }` bei y = 60,9273) — deshalb las er 13924 und 94338 las 0.

Das formenagnostische Gegenstück `carrier_coverage_curved` (Newton-Projektion eines
Volumengitters auf die eigene Nullmenge des Trägers) misst auf demselben Körper:

| Träger | projiziert | on-boundary | davon Contributor |
|---|---|---|---|
| 94338 (Zylinder r = 7,735) | 729 000 | **1040** | 1040 |
| 94347 (Ebene y = 60,9273) | 729 000 | 648 000 | 84 600 |

## (c) Wo die Fläche geblieben ist

Ehrliche Parametrisierung: (θ, y)-Gitter **auf dem Zylinder selbst**, 360 × 180,
r = 7,7349997, y ∈ [44, 62], eps = 1e−4 · Diagonale = 0,00779 mm.

    on-boundary   486
    verschüttet 42354
    außerhalb   21960

    Rand nur in 6 von 360 θ-Scheiben, y-Bereich 52,85 … 60,85

Wer verschüttet, Feature für Feature (eigene SDF < −eps):

| Feature (Replay-Index) | Proben | y-Bereich |
|---|---|---|
| `Extrude 25` (idx 13, Onshapes `Extrude 4`) | 28 674 | 52,83 … 60,93 |
| `Revolve 23` (idx 11, Onshapes `Revolve 1`) | 9 720 | < 46,677 |
| `Extrude 24` (idx 12, Onshapes `Extrude 3`) | 3 960 | > 60,93 |

Die 6 verbliebenen θ-Scheiben sind die **Tangentenlinie**, auf der `Extrude 4`s Block
(lokal x ∈ [−8; 7,735], z ∈ [−8; 8]) den Zylinder bei x = +7,735, z = 0 berührt. Das ist
kein Flächenstück, das ist eine Linie.

Und das eigentliche Loch — das Radialprofil unseres `Revolve 1` bei θ = 90°
(eine Ebene, die kein Extrude-Block erreicht), Schrittweite 0,05 mm:

    y 20,5 … 25,0   Material r ∈ [0; 10,400]
    y 26,5 … 46,0   Material r ∈ [0;  9,450]
    y 47,5 … 52,0   KEIN MATERIAL
    y 53,5 … 59,5   Material r ∈ [0;  7,700]

**Unser `Revolve 1` hat eine Lücke von 6,15 mm.**

In Onshape ist die zweite Profilregion von `Revolve 1` das Rechteck
x ∈ [9,2542; 16,9892] × y ∈ [−46,6773; −60,9273], also Radius 0 … 7,735 über die
**volle** Höhe y_lokal 46,677 … 60,927. Wir bauen nur deren untere Hälfte
(52,827 … 60,927). Getrennt wird sie von `q0PrimhP9B2a.top` (y = −52,8273) — der
Oberkante genau des Rechtecks, das `Extrude 4` extrudiert.

Damit schließt sich die Kette: Onshapes `Extrude 4`-Block deckt y 52,83 … 60,93 ab; die
Fläche, auf der Onshapes `Thicken 6` steht, ist deshalb das **Band y 46,68 … 52,83**
(6,15 mm hoch, ≈ 299 mm²) — und das ist exakt das Band, das unser `Revolve 1` nicht baut.

Der Mechanismus liegt im Übersetzer, nicht im Weaver:
`resolveProfiles` (`web/src/onshape/onshapeImport.ts`) bildet **eine** Onshape-Region
über `regionFaceIndex` auf **genau eine** atomare Imprint-Fläche ab. Zerlegt unsere
Imprint-Anordnung eine Onshape-Region in zwei atomare Flächen, fällt die zweite
lautlos weg: die Fehlmengen-Warnung greift nur bei `indices.size < regionQueries.length`,
und hier stehen 2 Queries 2 Indizes gegenüber. Sichtbar ist das an der Aufzeichnung —
`Revolve 1` bekommt `qRegions(6, [4, 7])`, `Extrude 4` bekommt `qRegions(6, [4, 5])`;
Region 4 ist in beiden dieselbe untere Lamelle, die obere hat niemand.

Onshapes `Extrude 4` wählt übrigens **drei** Regionen (die drei Streifen, in die
`2L94FTnhDuaQ.right` und `.left` das `q0PrimhP9B2a`-Rechteck teilen), wir zwei — dieselbe
Verlustart, nur an anderer Stelle.

## (d) Beide Richtungen des Tickets sind widerlegt

**Richtung 2 („unser Körper 9 ist unvollständig")** stimmt im Ergebnis, aber nicht in der
Begründung. Körper 9 fehlt Material — aber es gehört zu `Revolve 1`, einem Feature, das
**importiert**. Zwischen `Revolve 1` (Onshape-Index 31) und `Thicken 6` (38) werden nur
`Sketch 5` (34, kein Material) und `Extrude 17` (37) übersprungen. Kein zusätzlich
importiertes Feature könnte `Extrude 4`s Block aus y 52,83 … 60,93 entfernen; das freie
Band liegt bei 46,68 … 52,83, und dort fehlt schlicht unser eigener Revolve.

**Richtung 1 („der Importer soll unauflösbare Flächen melden und überspringen")** wäre die
falsche Reparatur. Die Fläche, die Onshape nennt, existiert wirklich. Sie zu überspringen
zementiert einen Verlust, statt ihn zu beheben.

## (e) Was das Überspringen heute kostet: nichts

Replay des Dokuments einmal vollständig, einmal mit **unterdrücktem** `Thicken 6`.

Wichtig, und beim ersten Versuch falsch gemacht: der Aufruf darf **nicht gelöscht**
werden. `Document::allocate_id` vergibt fortlaufend, ein entfernter Aufruf nummeriert
jedes spätere Feature um und richtet jedes spätere `qCreatedBy(n)` auf das falsche Ziel —
der Löschversuch meldete 12 → 11 Körper und einen geschrumpften Körper 9, reine
Artefakte. `set_feature_suppressed(29, true)` hält die Ids fest.

Ergebnis, alle 12 Körper:

    counts (v, e, f)   identisch für jeden Körper
    openFaces          [0,0,0,0,0,0,2,0,0,1,8,0]  →  [0,0,0,0,0,0,2,0,0,1,8,0]
    Volumen (64³)      identisch für alle 12, bis auf 10⁻³ mm³

Kontrolle, damit das kein blindes Instrument ist: `set_feature_suppressed(25, …)`
(= `Extrude 4`) bewegt Körper 9 von (26, 46, 25) auf (18, 35, 19).

**`Thicken 6` ändert heute keine einzige Zahl.** Der Schnitt mit der 0,1-mm-Schale auf
einem vollständig verschütteten Zylinder trifft nichts. Das Feature importiert, aber es
tut nichts.

## (f) Was daraus folgt

- Die echte Reparatur sitzt in `resolveProfiles` / `regionFaceIndex`: eine
  Onshape-Region muss auf **alle** atomaren Flächen abgebildet werden, die sie überdeckt,
  nicht auf die bestbewertete. Das ist ein eigener Umbau am #412-Kern und bewegt Pins in
  allen sechs Onshape-Suiten — eigenes Paket, eigene Abnahme.
- #438 und #439 verlieren durch ihre Bruchstelle **nachweislich nichts**: was sie
  abschalten, ist ein Feature ohne Wirkung. Ob sie deshalb landen dürfen, ist eine
  Produktentscheidung und steht in ihren eigenen Tickets.
- `carrier_coverage` darf nie wieder auf einen gekrümmten Träger angewendet werden.
  `carrier_coverage_curved` steht daneben und sagt im Kopfkommentar, warum.

---

## Reparatur (Option A) — der Region-Übersetzer, 2026-07-28, v0.42.1

Der User hat Option A gewählt: eine Onshape-Region bildet ab jetzt auf **alle**
atomaren Imprint-Flächen ab, die sie überdeckt.

### Was sich geändert hat

Der Umbau hängt an einer Beobachtung, die (f) noch nicht hatte: die Region-Query
trägt ihre **vollständige Rand-Topologie**, und `parseBton` liest sie seit #430
fehlerfrei. Eine `FACE`/`IMPRINT`-Query disambiguiert über TOPOLOGY zu einer
Liste von Imprint-KANTEN; jede Kante nennt ihre Basis-Sketchkurve und — wieder
über TOPOLOGY — die zwei Imprint-VERTICES, an denen sie beschnitten ist, jeder
als Schnitt benannter Kurven. Über alle sieben Fixture-Dokumente hat **jede**
Profil-Query der Form `FACE/IMPRINT` diese Disambiguierung; es gibt keinen Fall
ohne sie.

#412 las statt dessen einen **flachen Token-Scan** über alle Strings der Query.
Der kostete drei Dinge:

1. **Den Beschnitt.** Ohne ihn geht eine 920 mm lange Linie in voller Länge in
   die Bewertung, obwohl die Region nur 20 mm davon berandet.
2. **Die Unterscheidbarkeit.** `Extrude 4` (#419) wählt drei Streifen EINES
   Rechtecks; zwei davon nennen buchstäblich dieselben Kurven-Ids und
   unterscheiden sich nur im Beschnitt. Der flache Scan lieferte für beide
   dieselbe Liste — die Regionen fielen zusammen.
3. **Die Genauigkeit.** Der Scan sammelte auch Strukturwerte (`CAP_EDGE`) und
   Ids aus verschachtelten Vertex-Disambiguierungen als wären es Randkurven.
   Jeder unbekannte Name machte die ganze Query unauflösbar.

Neu, in zwei Dateien:

- `web/src/onshape/onshapeRegion.ts` liefert statt `entityIds: string[]` jetzt
  `edges: RegionBoundaryEdge[]` — Basis-Kurve plus die Kurvenmengen der beiden
  Trim-Vertices. Kanten, die nicht aus einer Sketchkurve stammen, fallen weg
  statt die Query zu vergiften.
- `web/src/onshape/onshapeRegionFaces.ts` (neu) bildet eine Region auf eine
  MENGE von Flächen ab. Zwei unabhängige Lesarten derselben Sondierung:
  - **Rangfolge** wählt den Anker (wie bisher: Deckungsanteil am eigenen Umfang
    × Anteil der Randkanten, die die Fläche berühren) — nur jetzt auf den
    beschnittenen Kanten, was die Bewertung erheblich schärft.
  - **Verschmelzung** wächst vom Anker: eine Onshape-Imprint-Kante hat auf jeder
    Seite **genau eine** Fläche. Wechselt die Fläche auf einer Seite, während man
    die Kante entlangläuft, liegen beide in derselben Onshape-Region — unsere
    Anordnung hat dort geschnitten, Onshapes nicht. Das ist positive Evidenz,
    kein Fehlen eines Trenners: es kann nicht in den ganzen Sketch auslaufen.
  Eine benannte Kante ZWISCHEN zwei Flächen ist Beweis, dass sie nicht dieselbe
  Region sind. Widerspricht sie der Verschmelzung, bleibt der Anker allein
  stehen und die Auflösung meldet `conflict`.

Zwei Fälle konnten vorher stumm falsch werden und sind es nicht mehr: der Anker
einer Region gehört ihr — keine andere Region darf ihn per Verschmelzung
abziehen —, und die Fehlmengen-Warnung zählt nicht mehr Flächen gegen Queries
(das schwieg genau beim #440-Fall), sondern benennt, was passiert ist:
unauflösbare Regionen, Anker-Kollisionen, Widersprüche, umstrittene Flächen.

### Nachher-Zahlen

**`Revolve 1`** (#419): `qRegions(6, [4, 7])` → **`qRegions(6, [4, 6, 7])`**.
Face 6 ist genau das Band x ∈ [9,25; 16,99] × y ∈ [−52,83; −46,68], also die
6,15-mm-Lücke aus (c). Das Radialprofil hat sie nicht mehr.

**`Extrude 4`** (#419): `qRegions(6, [4, 5])` → **`qRegions(6, [3, 4, 5])`** —
die drei Streifen, die Onshape wählt, statt zwei. Die Warnung ist ersatzlos weg.

**`Thicken 6`**: seine Fläche existiert jetzt. Die Query löst auf **genau eine**
Fläche auf, und die ist der Zylinder aus (a):

    Body 9, featureId 23 (Revolve 1), kind cylinder, radius 7,735 mm

Vorher war derselbe Träger nach (c) auf 486 von 64 800 Proben Rand — eine
Tangentenlinie, kein Flächenstück.

**`Thicken 6` schneidet trotzdem nichts** — und der Grund ist ein anderer als
#440s. Sein `booleanScope` ist `qCreatedBy(27)` = `Extrude 16`, und dessen Körper
trifft den Revolve in unserem Import überhaupt nicht: Body 10 (Features 27, 35)
liegt bei x ∈ [−78,4; −62,5], Body 9 (der Revolve) bei x ∈ [−72,5; −19,5], und
der Zylinder sitzt lokal bei y 46,68 … 60,93 ⇒ Welt-x −46,7 … −60,9. Der Beweis,
dass das nicht an Thicken liegt: **`Boolean 1` (Call 28) subtrahiert dieselben
zwei Körper und bewegt ebenfalls keine einzige Zahl.** Beide hängen an
`Extrude 16`s End-Condition `UP_TO_SURFACE` mit `oppositeDirection: true`, die
wir als gemessene Tiefe +15,85 mm übernehmen — eigener Prüfpunkt, eigene
Baustelle, nicht die Regionen-Auflösung.

Kontrolle, damit das kein blindes Instrument ist: `setFeatureSuppressed(25)`
(= `Extrude 4`) bewegt Body 9 von (26, 47, 24) auf (18, 35, 18).

### Pin-Bewegungen in den sechs Suiten

Körperzahlen und Fehlerzahlen: #419 12 Körper / 192 Meldungen unverändert,
#412 / #414 / #415 / #416 vollständig unverändert. Bewegt haben sich:

| Dokument | Feature | vorher | nachher | Warum besser |
|---|---|---|---|---|
| #419 | `Revolve 1` | `[4, 7]` | `[4, 6, 7]` | die 6,15-mm-Lücke, s. o. |
| #419 | `Extrude 4` | `[4, 5]` | `[3, 4, 5]` | Onshape wählt drei Streifen |
| #419 | `Extrude 20` | 9 Faces | 11 Faces | 5 Kollisionen → 3 |
| #419 | `Extrude 23` | 7 Faces | 8 Faces | 2 unauflösbare → 1 |
| #419 | `Extrude 49` | `[4, 8, 9, 10]` | `[2, 4, 8, 9]` | Beschnitt trennt die Streifen |
| #419 | `Revolve 7` | ganzer Sketch | `qRegion(6, 13)` | löst jetzt auf |
| #419 | `Extrude 44` | ganzer Sketch | Region | löst jetzt auf |
| #409 | `Extrude 4` | `[2, 3]` | `[1, 3]` | s. u. — die einzige harte |

Die #409-Bewegung ist die einzige, die etwas kostet, und sie ist die deutlichste
Korrektur. Sketch 3 ist ein 920×500-Rahmen: vier Leisten (Faces 0, 1, 3, 4) um
eine große Mittelplatte (Face 2). `Extrude 3` extrudiert die obere und untere
Leiste, `Extrude 4` die linke und rechte. Die erste Region-Query von `Extrude 4`
ist — mit Beschnitt gelesen — exakt das Rechteck x ∈ [0; 20] × y ∈ [20; 480], die
linke Leiste. Ohne Beschnitt gingen die beiden 920 mm langen Querlinien in voller
Länge ein, und die bestbewertete Fläche war die **880×460-Mittelplatte**, die
Onshape nie extrudiert. Der Import baute dort einen Körper, den es im Original
nicht gibt.

Der Preis: die beiden Leisten sind **disjunkt** und werden von denselben zwei
Querlinien berandet. `wall_sub_table` lehnt eine Tabelle mit wiederholten Ids ab
(zwei Wände mit einer Sub kollabierten zu einer Fläche), also fallen die Wände
auf positionale Subs zurück — `qWall(8, 45)` bindet nicht mehr, und `Mirror 2`
samt Mate-Connector stehen wieder in `KNOWN_GAPS` der #409-Suite. Gemessen:
vorher band `qWall(8, n)` für n = 6, 15, 36, 45; nachher für n = 0 … 7.

Das ist eine **Kernel-Lücke, keine Übersetzer-Lücke**: die richtige Reparatur ist
Wand-Benennung pro Rand-Schleife (#429s Gebiet). Die Alternative — das Profil
wieder aufzublasen — hieße, einen Mate-Connector funktionieren zu lassen, weil
der Körper unter ihm 44× zu groß ist.

### Was offen bleibt

- `Extrude 16`s `UP_TO_SURFACE` + `oppositeDirection` (s. o.) — blockiert
  `Boolean 1` UND `Thicken 6` im #419-Dokument.
- Wand-Benennung für disjunkte Mehrfach-Regionen (#409 `Mirror 2`).
- Der `thickness1`-vs-`thickness`-Punkt aus (a) ist weiterhin ungeprüft.
- Die zwei `#[ignore]`-Sonden in `crates/kernel/src/onshape_regression_tests.rs`
  laufen auf der eingefrorenen Aufzeichnung `testdata/onshape419_full_import.json`,
  die noch die ALTEN Profile trägt (`qRegions(6, [4, 7])`). Sie messen damit den
  Zustand vor dieser Reparatur. Wer sie wieder als Instrument braucht, muss die
  Aufzeichnung neu ziehen — dabei bewegen sich die dort gepinnten
  `open_faces`-Vektoren. Die Nachher-Messung oben steht statt dessen als
  dauerhafter Pin in `onshapeImport419E2E.slow.test.ts`.
