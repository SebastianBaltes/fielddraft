# REPORT — Lichen-Forschungssession (Branch `lichen-research`)

> ENTWURF — wird am Sessionende finalisiert. Fortlaufende Details in
> RESEARCH_LOG.md, aktueller Arbeitsstand in STATE.md.

## Zusammenfassung

*(ausstehend — Status der Erfolgskriterien mit Zahlen)*

Erfolgskriterien:
1. Gyroid-Kugel-Ausschnitt besteht 4-Metriken-Suite: **offen** (Manifold-Metrik
   ist der Blocker; Hausdorff/Aspect/Dreieckszahl bereits im Ziel)
2. Bestehende Suite grün: Web 1679/1679 ✓ (2026-07-13 03:09); Kernel-Lichen-
   Gates 33/33 ✓; volle Workspace-Suite am Ende erneut
3. REPORT.md: dieses Dokument

## Metriken vorher/nachher (Abnahmefall, Betreiber-Settings, release)

| Stand | open | nm | H(m→s) mm | H(s→m) mm | asp p99 | minang | pts/tris |
|---|---|---|---|---|---|---|---|
| Baseline v0.6.563 | 397 | 0 | 0.010 | 0.080 (p95 0.003) | 1.78 | 18.6° | 5746/11137 |
| v0.6.564 blatt-restr. Kugel | 312 | 0 | 0.026 | 0.090 (p95 0.003) | 1.78 | 18.6° | 5746/11222 |
| v0.6.565 Band 10° + Guard-Ausnahme + Ketten-Weld | 21 | 0 | 0.021 | 0.098 (p95 0.002) | 1.85 | 14.9° | 5738/11459 |
| Zwickel-Topologie (in Arbeit) | | | | | | | |

## Erforschte Ansätze

### Gelandet
- **Blatt-restringierte Zeugen-Kugel** (v0.6.564): restricted Delaunay je
  glattem Patch (Cheng–Dey–Levine); Regionen aus B-Rep-Flächen + Union-Find
  (Verschmelzung ohne trennende Kette), Naht-Band-Einfärbung immer aktiv.
  397→312, alle Mikro-Löcher entlang der Nähte geschlossen.
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

## Architektur der Lösung

*(ausstehend)*

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

## Offene Punkte / empfohlene nächste Schritte

*(ausstehend)*
