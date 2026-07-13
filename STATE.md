# STATE — Lichen-Forschungssession (Branch `lichen-research`)

Stand: 2026-07-13, ~02:15

## Auftrag (Definition of Done)

1. Testfall **Gyroid-Kugel-Ausschnitt** besteht die automatisierte Suite mit vier
   Metriken: Hausdorff (beidseitig), Dreieckszahl, Manifold, Aspect Ratio.
2. Bestehende Suite vollständig grün.
3. `REPORT.md` mit ehrlichen Vorher/Nachher-Zahlen.

## Der Abnahme-Testfall (KANONISCH)

Vom Betreiber als .fdraft geliefert (identisch mit `demo_project_kernel()` in
`crates/kernel/src/api/lichen.rs`):

- Kugel r=50 ∩ Gyroid(cell 10, thickness 1) ∖ Zylinder(r=30, h=120),
  dann **Box-Intersect** half_extents (3,4,3) @ (7.970, 22.314, 29.870)
  → nur ein 6×8×6-mm-Ausschnitt bleibt (deshalb „Ausschnitt“ — schnell testbar),
- plus Marker-Kugel r=1 @ (5.211, 22.114, 29.248) als Union (Betreiber-Punkt #894,
  Loch auf der Mini-Kugel-Kappe).
- Betreiber-Mesher-Settings (project.json → debug.lichen):
  `targetEdgeLength 10, grow, curvAngleDeg 2, minEdgeSpacing 0.1, maxPoints 50000,
  generalTri, accept witness, extractWalk, globalWalk, lfsFactor 0.1, sizingField,
  faceGuard false, fillHoles false` (kein `refine` — v0.6.563-Refinement separat messen).

### WICHTIG: Gyroid-Semantik (Irrweg vermieden)

`Primitive::Gyroid`-SDF = `(|f(p/s)| − thickness) · s · 0.2887`.
**`thickness` ist DIMENSIONSLOS** (f-Wertebereich ±1,5), nicht mm; räumliche
Periode = `2π·cell_size` ≈ 62,8 mm bei cell 10. thickness=1 ⇒ ~82 % Material,
dicke Wände — KEIN dünnwandiges Gitter. Eine „repräsentative Mini-Fixture r=15“
ist damit sinnlos (< ¼ Periode); der Abnahmefall ist das Demo-Projekt selbst.

## Was funktioniert

- `crates/kernel/src/lichen_metrics.rs`: Hausdorff beidseitig (mesh→surface via
  Bary-Samples aufs SDF; surface→mesh via Newton-Projektion + TriGrid),
  AspectStats, deterministischer LCG. 3 Selbst-Tests grün.
- Test-Infrastruktur in `api/lichen.rs`: `scene_bodies`, `mesh_of`, `measure`,
  `print_report`, Baseline-Runner `zz_gyroid_baseline` (env-steuerbar).

## Offen / Nächste Schritte

1. Fixture-Validierung von der falschen r=15-Annahme auf das Demo-Projekt
   umstellen (Ausschnitt-Region proben: Luft UND Material, Marker = Material).
2. Baseline auf Demo mit Betreiber-Settings messen (± `refine`).
3. Abnahme-Gate als nicht-ignorierter Test (Schwellen aus Baseline + Theorie).
4. Iterationsschleife auf Defekten (worst_surface_point zur Lokalisierung).

## Bekannter Kontext aus Vorarbeit (Commits 9dc3883 u. davor)

- v0.6.563: Delaunay-Refinement (Boissonnat–Oudot) hinter `refine`/`refineRho`;
  ρ=1.0 Default (No-op auf glatten Körpern, füllt Betreiber-Fünfeck-Loch);
  Refinement braucht Ketten-Clearance, sonst zerlegt es die Box (543 offene Kanten).
- Demo-Defekt-Stand laut Memory: chart 15 / general 36 offene Boundary-Kanten.
- Theorie (Recherche): Zeugen-Kugel = surface Delaunay ball; CBP
  (Edelsbrunner–Shah) ist die exakte Manifold-Bedingung; loose ε-sample
  ε≈0.1–0.16 bzgl. lfs ⇒ Homöomorphie + Hausdorff O(ε²·lfs);
  CGAL-Kriterien: Winkel ≤30°, radius_bound, distance_bound.
