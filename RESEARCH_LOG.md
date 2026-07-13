# RESEARCH_LOG — Lichen-Session

Format: Zeitstempel · Hypothese/Frage · Ergebnis · Entscheidung.

## 2026-07-13 01:30 — Session-Start (nach Kontext-Reset)

- Vorherige Turns: Metrik-Modul `lichen_metrics.rs` gebaut (Hausdorff beidseitig,
  Aspect, deterministisch), Gyroid-Fixture r=15 + Validierung + Baseline-Runner
  in `api/lichen.rs` (uncommitted). Recherche-Agenten hatten geliefert:
  Zeugen-Kugel ≙ surface Delaunay ball, CBP als exakte Manifold-Bedingung,
  loose ε-sampling (Boissonnat–Oudot) mit ε≈0.1–0.16, CGAL-Kriterien.
- Betreiber-Hinweis: Abnahme-Testfall = Projekt 7d9701… (.fdraft hochgeladen).
  Befund: **identisch mit `demo_project_kernel()`** — inkl. Box-Ausschnitt
  (3,4,3)@(7.97,22.31,29.87) und Marker-Kugel r=1. „Ausschnitt“ im DoD ist
  wörtlich: der Testfall IST der kleine Ausschnitt → schnell genug für CI.
  Zusätzlich liefert die Datei die Betreiber-Settings (u.a. curvAngleDeg 2,
  lfsFactor 0.1, sizingField true, accept witness).

## 2026-07-13 02:00 — Fixture-Validierung schlägt fehl → Gyroid-Semantik

- Hypothese: Fixture r=15 enthält dünnwandiges Gyroid (1-mm-Wand, ~30 % Material).
- Messung: 82 % Material, tiefster Innenpunkt −2,89 mm, nur 2 Vorzeichenwechsel
  auf 30 mm. Code-Lektüre `sdf_eval.rs:325`: SDF = `(|f(p/s)|−t)·s·0.2887` —
  **t dimensionslos, Periode 2π·s ≈ 62,8 mm**. thickness=1 ⇒ |f|<1 ⇒ dicke Wände.
- Konsequenz: r=15-Fixture (< ¼ Periode) ist NICHT repräsentativ; meine
  Validierungs-Annahme („Luft > 700“) war falsch begründet. Der ehrliche
  Testfall ist das Demo-Projekt selbst (Ausschnitt = klein = schnell).
- Entscheidung: `gyroid_sphere_kernel(r)`-Fixture streichen; Validierung,
  Morphologie-Probe und Baseline auf `demo_project_kernel()` umstellen;
  Hausdorff-Sampling-Box = Ausschnitt-AABB ∪ Marker-AABB + Rand.
- Notiz (nicht diese Session): Feature-Doku sagt „Distance between repeating
  cells in mm“ — stimmt nicht mit der Implementierung (Periode 2π·cell) überein.
  Semantik-Änderung würde bestehende Projekte brechen; nur dokumentieren.
