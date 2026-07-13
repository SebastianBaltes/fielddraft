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
| Schutzkorridor (in Arbeit) | | | | | | | |

## Erforschte Ansätze

### Gelandet
- **Blatt-restringierte Zeugen-Kugel** (v0.6.564): restricted Delaunay je
  glattem Patch (Cheng–Dey–Levine); Regionen aus B-Rep-Flächen + Union-Find
  (Verschmelzung ohne trennende Kette), Naht-Band-Einfärbung immer aktiv.
  397→312, alle Mikro-Löcher entlang der Nähte geschlossen.

### Verworfen (vermessen)
- Host-only-Kontamination: 312→384 — unabhängige Blatt-Triangulierungen
  verheddern sich im überfüllten Naht-Keil.
- Gabriel-Fallback bei Zeugen-Newton-Versagen: →478 — Kugeln ohne
  Flächen-Zentrum lassen falsche Dreiecke zu.
- faceGuard-Dreiecks-Gate: wirkungslos auf den Nähten (386).
- Globales minEdgeSpacing-Anheben: 155 offene, aber untersampelt
  (H(s→m) 0.26 > 0.15) — bestätigt aber die Crowding-Diagnose.

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
