# STATE — Lichen-Forschungssession (Branch `lichen-research`)

Stand: 2026-07-13, ~10:00 — **Sessionziel erreicht**, Abschlussphase.

## Status der Erfolgskriterien

1. **Gyroid-Kugel-Ausschnitt besteht die 4-Metriken-Suite: ERFÜLLT.**
   Aktives Gate `gyroid_cutout_acceptance_four_metrics` (api/lichen.rs):
   open=0, nm=0, orient=0, χ=2, H(m→s) 0.021 ≤ 0.05, H(s→m) 0.098 ≤ 0.15
   (p95 0.0022 ≤ 0.02), tris 11468 ∈ [7000,23000], asp p99 1.85 ≤ 4,
   minang 14.9° ≥ 10°. Robust h=6/10/14, deterministisch.
2. **Suiten grün**: Kernel release 1082/1082, Lichen-Gates 37/37, Workspace
   release 1159/1159, Web 1679/1679. Debug-Workspace-Lauf läuft noch
   (Ergebnis in RESEARCH_LOG nachtragen).
3. **REPORT.md**: final (Zusammenfassung, Metriken, Ansätze inkl. verworfener,
   Architektur, Härtung, offene Punkte).

## Commits (alle gepusht auf origin/lichen-research)

- `7069613` Messwerk + Fixture (Abnahmefall = Betreiber-.fdraft byte-genau)
- `b54e861` v0.6.564 blatt-restringierte Zeugen-Kugel (397→312)
- `98732ae` v0.6.565 Naht-Band 10° + Guard-Ausnahme + Ketten-Weld (312→21)
- `c02366e` v0.6.566 Kreuzungs-Fixes + Naht-Stitching (21→0, χ=2)
- `e9650ae` Abnahme-Gate aktiv
- `7f51255` v0.6.567 adversariale Härtung (Matrix, 4 Fixes, 3 neue Gates)

## Verbleibende Abschluss-Schritte

1. Debug-Workspace-Lauf abwarten, Ergebnis in Log/Report eintragen.
2. `just build-wasm`, damit der laufende Dev-Server des Betreibers den
   Stand im verlinkten Projekt zeigt (WASM ist gitignored, nicht committen).
3. Memory/Abschlussmeldung.

## Für die Fortsetzung (falls neuer Kontext)

- Alle Erkenntnisse: RESEARCH_LOG.md (chronologisch) + REPORT.md (final).
- Offene Arbeitslinien: Warp-Bodies (Feature-Linien ohne B-Rep), tangentiale
  Küsse (Soll-Definition), Sliver an Streifnähten, Renderer-Displace-
  Lipschitz (bvh.rs-Befund), Architecture-Doku-Nachtrag.
- Nichts auf main gemerged — Betreiber entscheidet über Merge.
