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
