# REPORT — Lichen-Forschungssession (Branch `lichen-research`)

Autonome Session 2026-07-13. Fortlaufende Details in RESEARCH_LOG.md,
Arbeitsstand in STATE.md. Commits `7069613` … `7f51255` (v0.6.564–567).

## Zusammenfassung

Der Abnahmefall **Gyroid-Kugel-Ausschnitt** (das Betreiber-Projekt byte-genau,
inkl. Betreiber-Mesher-Settings) ging von **397 offenen Kanten** auf **0** —
wasserdicht, χ=2, konsistent orientiert — bei gleichzeitig eingehaltenen
Hausdorff-, Dreieckszahl- und Formgüte-Zielen. Der Weg bestand aus vier
kleinen, einzeln vermessenen Korrekturen an der Zeugen-Kugel-Akzeptanz und
der Ketten-Behandlung (v0.6.564–566) plus adversarialer Härtung (v0.6.567);
zwei theoriegeleitete Großansätze (Schutzkorridor, Segment-Verschweißung)
wurden vollständig implementiert, vermessen und **verworfen** — die Messung
schlug die Theorie.

Erfolgskriterien:
1. Gyroid-Kugel-Ausschnitt besteht 4-Metriken-Suite: **ERFÜLLT** (v0.6.566) —
   aktives Gate `gyroid_cutout_acceptance_four_metrics`: Manifold (open=0,
   nm=0, orient=0, χ=2), Hausdorff (0.021/0.098 mm, Schwellen 0.05/0.15),
   Dreieckszahl 11468 ∈ [7000, 23000], Aspect p99 1.85 ≤ 4, minang 14.9° ≥ 10°.
   Robust bei h=6/10/14, deterministisch. Byte-identisch nach der Härtung.
2. Bestehende Suite grün: Kernel (release) 1082/1082 ✓, Lichen-Gates 37/37 ✓,
   Workspace (release) 1159/1159 ✓, Web-Vitest 1679/1679 ✓
   (Debug-Workspace-Lauf: siehe Abschluss-Abschnitt).
3. REPORT.md: dieses Dokument ✓

## Metriken vorher/nachher (Abnahmefall, Betreiber-Settings, release)

| Stand | open | nm | H(m→s) mm | H(s→m) mm | asp p99 | minang | pts/tris |
|---|---|---|---|---|---|---|---|
| Baseline v0.6.563 | 397 | 0 | 0.010 | 0.080 (p95 0.003) | 1.78 | 18.6° | 5746/11137 |
| v0.6.564 blatt-restr. Kugel | 312 | 0 | 0.026 | 0.090 (p95 0.003) | 1.78 | 18.6° | 5746/11222 |
| v0.6.565 Band 10° + Guard-Ausnahme + Ketten-Weld | 21 | 0 | 0.021 | 0.098 (p95 0.002) | 1.85 | 14.9° | 5738/11459 |
| **v0.6.566 Kreuzungs-Fixes + Naht-Stitching** | **0** (χ=2) | 0 | 0.021 | 0.098 (p95 0.002) | 1.85 | 14.9° | 5738/11468 |

Robustheit: open=0 auch bei h=6 und h=14; deterministisch (identische Läufe).

## Erforschte Ansätze

### Gelandet
- **Blatt-restringierte Zeugen-Kugel** (v0.6.564): restricted Delaunay je
  glattem Patch (Cheng–Dey–Levine); Regionen aus B-Rep-Flächen + Union-Find
  (Verschmelzung ohne trennende Kette), Naht-Band-Einfärbung immer aktiv.
  397→312, alle Mikro-Löcher entlang der Nähte geschlossen.
- **Kreuzungs-Veto-Korrekturen + Naht-Stitching** (v0.6.566): 21→0. Die
  letzten „Ketten-Kreuzungen" waren Projektions-Falsch-Positive (fremde
  Nähte 0,1 mm ÜBER der Dreiecksebene, in die Ebene projiziert): Blatt-
  Restriktion + Ebenen-Abstands-Gate im Veto (21→10). Rest = Mikro-Loops
  (≤5 Kanten, Grad-2-Rand), die nachweislich nur am Zeugen-Newton am
  SDF-Knick scheiterten: deterministischer Stitching-Post-Pass, jedes
  Dreieck muss die volle Geometrie-Gültigkeit (valid_geom inkl.
  Sichtlinien) bestehen — kein blindes Stopfen (10→0, χ=2).
- **Naht-Band 25°→10° + Normalen-Guard-Ausnahme + Ketten-Verschweißung**
  (v0.6.565): 312→21. (1) Mit 25°-Tangential-Schwelle trug jeder nahtnahe
  Punkt beide Regionen — die Blatt-Restriktion war blind; reale Keilwinkel
  16–24°. (2) 243/283 offene Kanten waren voll gültige Dreiecke, die nur der
  90°-Guard blockte (Netz muss am spitzen Zwickel legitim >90° um die Spitze);
  Ausnahme nur bei strikt auf der Fläche liegender Sehne — schließt zugleich
  das θ-skalierte Sichtlinien-Schlupfloch. (3) Kettenpunkte verschiedener
  B-Rep-Kanten in Sub-Resolution-Abstand teilen einen Mesh-Vertex
  (weld 1.5·minEdgeSpacing) + Kreuzungs-Veto-Ausnahme.

### Verworfen (vermessen)
- **Schutzkorridor um Ketten** (Cheng–Dey–Ramos protecting balls): voll
  implementiert (Keilwinkel→R, Ring-Deckel, Lipschitz-Glättung, Gates in
  Wachstum/Rebalance/Füllung/Refinement/Streuung, Hidden-Vertex-Variante),
  13 Konfigurationen — nie besser als 287; ab β≥2 brechen H(m→s)/minang.
  Kern-Erkenntnis: reale Keilwinkel sinθ ≥ 0.274 — Blattverschachtelung ist
  Randphänomen, die Hauptblocker waren Einfärbung + Guard. Bleibt als
  Default-aus-Werkzeug (LICHEN_PROTECT_MODE).
- Host-only-Kontamination: 312→384 (auch mit Korridor 351/303) — unabhängige
  Blatt-Triangulierungen verheddern sich im überfüllten Naht-Keil.
- Gabriel-Fallback bei Zeugen-Newton-Versagen: →478 — Kugeln ohne
  Flächen-Zentrum lassen falsche Dreiecke zu.
- faceGuard-Dreiecks-Gate: wirkungslos auf den Nähten (386).
- Globales minEdgeSpacing-Anheben: 155 offene, aber untersampelt
  (H(s→m) 0.26 > 0.15) — bestätigt aber die Crowding-Diagnose örtlich.
- Hidden-Vertex zusätzlich zum v0.6.565-Stand: 21→26.
- **Segment-Segment-Ketten-Verschweißung** (Zwickel-Schnittpunkt-Hypothese):
  voll implementiert (Ericson-Closest-Pair, Schnitt-Vertex in beide Ketten),
  4 Radien vermessen — es existieren keine Off-Vertex-Quasi-Schnitte; die
  echten Blocker waren Projektions-Falsch-Positive des Kreuzungstests.
  Implementierung entfernt, Phasen-Split der Ketten-Erzeugung bleibt.

## Architektur der Lösung

Lichen (Punkt-Wachstum → Zeugen-Kugel-Akzeptanz → globaler Walk) blieb
strukturell unangetastet; die Lösung besteht aus fünf chirurgischen
Ergänzungen, alle in `crates/kernel/src/lichen_tri.rs` bzw. `api/lichen.rs`:

1. **Blatt-restringierte Zeugen-Kugel** (restricted Delaunay je glattem
   Patch): Kontamination zählt nur von Punkten mit gemeinsamer B-Rep-Region;
   Regionen = Flächen ∪ Union-Find (keine Verschmelzung bei trennender Kette,
   nie bei antiparallelen Blättern), Naht-Band-Einfärbung (10°-Tangential-
   Schwelle) gibt nahtnahen Punkten die Zweitregion.
2. **Normalen-Guard mit strikter Auf-der-Fläche-Ausnahme**: >90°-Paare sind
   erlaubt, wenn die Sehne strikt auf der Fläche liegt (0.25/0.5/0.75-Proben,
   |f| ≤ 0.15·min-spacing) — nötig am spitzen Zwickel, schließt zugleich das
   θ-skalierte Sichtlinien-Schlupfloch; dünne Wände bleiben geschützt.
3. **Ketten-Hygiene**: Verschweißung fremder Kettenpunkte in Sub-Resolution-
   Abstand (1.5·minEdgeSpacing) zu einem Mesh-Vertex; Kreuzungs-Veto mit
   Blatt-Restriktion + Ebenen-Abstands-Gate (gegen Projektions-Falsch-
   Positive von Nähten über der Dreiecksebene) + Deckungs-Filter (nur
   konsekutive Segmente echter Kanten-Ketten — gegen Phantom-Diagonalen aus
   ungeordneten Weave-Fallback-Loops).
4. **Naht-Stitching-Post-Pass** (Kill-Switch LICHEN_STITCH=0): kantenbasierter
   Loop-Walk über die freien Halbkanten; Mikro-Loops ≤5 Kanten werden
   geschlossen, wenn jedes Dreieck `valid_geom` besteht (volle Gültigkeit
   inkl. Sichtlinien/Guard/Kreuzung, nur ohne Zeugen-Kugel — die dort
   nachweislich nur am SDF-Knick-Newton scheitert). Additiv, manifold-sicher,
   größere Loops bleiben ehrlich offen.
5. **Mess-/Forensik-Werk** (`lichen_metrics.rs` + ignored-Tests):
   deterministische beidseitige Hausdorff gegen die echte SDF, Fixture-
   Validierung, Open-Edge-Cluster/Flag-Forensik, `empty_why`
   (Kontaminatoren/Newton), Ketten-Dumps, JSON-Merge-Baseline-Runner.

Werkzeuge, die default-aus bleiben: Schutzkorridor
(`LICHEN_PROTECT_MODE=grow|hide` + β/floor/ringdiv/lip) — vollständig
implementiert, als Mesher-Verbesserung widerlegt, als Experimentier-Hebel
erhalten.

## Kern-Erkenntnisse

1. Der Abnahmefall („Ausschnitt") ist klein UND maximal fies: die Gyroid-Wand
   streift den Box-Ausschnitt fast-tangential; beide Problem-Nähte sind
   fast-tangentiale Zwei-Flächen-Schnitte.
2. `Primitive::Gyroid`: thickness ist DIMENSIONSLOS, Periode = 2π·cell_size —
   skalierte Ersatz-Fixtures haben eine andere Morphologie.
3. Die Nähte haben Ketten (Seed-Weaver); der Defekt liegt in der Akzeptanz:
   im Naht-Keil unterschreitet der Blattabstand den Punktabstand → jede
   Zeugen-Kugel enthält Gegenblatt-Punkte (wechselseitige Blockade). Der
   Normalen-Guard ist an tangentialen Nähten prinzipiell blind.
4. Werkzeug-Ehrlichkeit: Options-Merge per String-Anhängen erzeugte
   Duplikat-Keys → serde-Fehler → Default-Optionen (Phantom-Messungen).
   Messwerkzeuge müssen sich selbst beweisen (Fixture-Validierung, JSON-Merge).

## Härtung (adversariale Matrix, v0.6.567)

10 Fälle in 4 Klassen (`zz_adversarial_matrix`, Ist-Werte im Test-Doc):

- **Wasserdicht (Kat. a)**: dünne Platte (0,35 mm), dünnes Rohr (χ=0-Torus!),
  koplanare Vereinigung (nach Phantom-Ketten-Fix 46→0), Streif-Naht an
  anderem Ort (beweist: keine Überanpassung an den Abnahmefall). Drei davon
  als neue AKTIVE Gates.
- **Kleine Defekte (b)**: dünne Gyroid-Wand 7 offene (Zwickel-Loops >5),
  Punkt-Kuss 33 (nach Regionen-Merge-Fix, vorher 59), Streifschnitt 3,6°
  wasserdicht aber Sliver (minang 5,2°).
- **Harte Grenzen (c, ignored-WIP mit Diagnose)**: Twist/Bend/Displace-Bodies
  (kein B-Rep → keine Ketten/Regionen; Twist-Box reißt an helikalen Kanten),
  exakt tangentialer Linien-Kuss (Blattabstand 0 = sub-resolution; zudem
  Sizing-Kollaps lfs→0 → 200 s).

Härtungs-Fixes: Phantom-Ketten-Filter, Regionen-Merge-Reparatur
(Orientierungs-Gate + min-Radius), Stitch-Loop-Walk (Grad-4-Zwickel),
fidget-Displace-Achsfaktoren. Implementiert + vermessen + zurückgebaut:
DL-70-Normalisierung des Displace-Felds (macht den Mesher schlechter,
813→1634; für den Renderer weiterhin richtig — eigene Arbeitslinie,
Befund in bvh.rs dokumentiert).

## Offene Punkte / empfohlene nächste Schritte

1. **Warp-Bodies** (Twist/Bend/Displace): Feld-basierte Feature-Linien-
   Extraktion (Normalen-Diskontinuitäten) als Ketten-Ersatz, da diese Bodies
   beide Weave-Scopes verlassen. Displace zusätzlich: Wachstums-Crowding auf
   Beulen-Flächen ist intraface — Regionen helfen dort prinzipiell nicht.
2. **Exakt tangentiale Kontakte**: ehrliches Soll definieren (comp=2 beim
   Punkt-Kuss bzw. sauberer Pinch bei Linien-Kontakt) statt Wasserdichtheit
   zu erzwingen; Sizing-Kollaps (lfs→0 am Kontakt) deckeln.
3. **Sliver an streifenden Nähten** (minang ~5° bei 3,6°-Schnitt): derzeit
   der Preis der Wasserdichtheit; Kandidat: anisotropes Remeshing entlang
   der Naht als Nachlauf.
4. **Renderer-Arbeitslinie Displace-Lipschitz** (bvh.rs-Befund): Feld ist
   nicht 1-Lipschitz (L≈1.8) — Marching kann überschreiten; Normalisierung
   renderer-verifiziert einführen (für den Mesher nachweislich schädlich).
5. **Rest-Doku**: docs/Lichen-Architecture.md um die Sessionerkenntnisse
   ergänzen (Naht-Keil-Kapitel, Stitching, verworfene Ansätze) — Log/Report
   sind die Quelle.
