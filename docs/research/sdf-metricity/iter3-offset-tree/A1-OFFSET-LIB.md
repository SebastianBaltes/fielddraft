# A1-Spike: Auswahl der 2D-Offset-Bibliothek (Phase A, offset_tree)

Teil von #384 / docs/OFFSET-TREE-PLAN.md §5 („2D-Offsetting per Bibliothek").
Experiment-Bin: `crates/metricity-lab/src/bin/a1_offset_lib_spike.rs`,
Messdaten: `a1_offset_lib.csv` (dieses Verzeichnis). Lauf:
`cargo run -p metricity-lab --release --bin a1_offset_lib_spike`.

**Empfehlung: `cavalier_contours` 0.7** — als einzige Kandidatin erfüllt sie
alle fünf Plan-Kriterien plus das nachgeschobene Audit-Kriterium
(Segment-Provenance, docs/OFFSET-TREE-AUDIT.md). Alle Messungen liegen in
der 1e-15-Klasse (Maschinengenauigkeit), drei Größenordnungen unter der
geforderten 1e-6-Klasse. Per Audit-Direktive ist der **Erosions-Fall (−δ)
der entscheidende** (Dilation bleibt vorerst f−δ) — genau dort (Insel
verschwindet, C-Profil spaltet, Loch wächst) ist cavalier_contours exakt.

## Kandidaten

* **cavalier_contours 0.7.0** (Rust-Port von CavalierContours, MIT/Apache-2.0,
  Release Jan 2026, aktiv gepflegt): Polylinien aus Linien- und
  Bulge-Bogensegmenten, `parallel_offset` (auch selbstschneidend),
  Booleans (Or/And/Not/Xor), `Shape`-API für Konturen mit Löchern. Pure Rust.
* **i_overlay 7.0.2** (iShape, MIT, sehr aktiv, 5.5M Downloads): robuste
  Polygon-Booleans + Outline-Offset, aber **nur Liniensegmente** — Bögen
  müssten geflattet werden. Pure Rust. Hier als Flatten-Referenz vermessen.
* **clipper2 0.6.0** (Bindings): wrappt `clipper2c-sys` (C++-Quellen via
  cc-Crate). `cargo check --target wasm32-unknown-unknown` scheitert am
  Build-Script (clang++-Toolchain für wasm nötig) → **praktisch
  disqualifiziert**; zudem nur Polygone und int64-Skalierung.
* **geo / geo-offset / geo-buffer**: keine Bogensegmente; geo-offset 0.4
  (Feb 2025, wenig genutzt), geo-buffer seit 2023 stale. Keine ernsthaften
  Kandidaten für Arc-Path-Profile.

## Kriterien-Tabelle

| Kriterium | cavalier_contours 0.7 | i_overlay 7.0 | clipper2 0.6 |
|---|---|---|---|
| 1. Bogen-Erhaltung (r±δ analytisch) | **ja, exakt**: Radialfehler ≤ 7.8e-16 (T1/T2), Bögen bleiben Bulge-Bögen (arc_vertices erhalten), bulge>1 (270°-Bogen) ok (T1b: ≤ 4.4e-16) | nein, flattet: Kreis→1024 Segmente ⇒ 4.7e-6 Fehler, 4096 ⇒ 3.0e-7 (T6) — 1e-6-Klasse nur mit ≥2k Segmenten je Kreis | nein (Polygone) |
| 2. wasm-tauglich | **ja**: pure Rust, Scratch-Crate `cargo check --target wasm32-unknown-unknown` grün | ja (pure Rust, grün) | **nein** (C++/cc-Build-Script scheitert) |
| 3. Topologiewechsel | **ja**: Insel r=0.3 bei δ=0.4 → 0 Loops; C-Profil → exakt 2 Loops (Flächen bis 2.2e-16 an der wahren Minkowski-Erosion inkl. Reflex-Ecken-Bump); Loch schließt bei Dilation δ>r (Shape-API) (T3/T4) | ja (Booleans robust), aber auf geflatteter Geometrie | ja, aber s. o. |
| 4. Profil-Booleans | **ja**: Or/And/Not/Xor mit Bogenkanten, Flächenfehler ≤ 4.4e-16, Halbkreiskante bleibt exakter Bogen (1.7e-16), innenliegender Abzug liefert Loch als neg_pline (T5) | ja, aber nur auf Polygonen | ja, nur Polygone |
| 5. f64 / Toleranzen / Robustheit | f64 generisch (`Real`), Epsilons explizit steuerbar (`pos_equal_eps` etc., Default 1e-5 absolut!); Offset kann Selbstschnitte (`handle_self_intersects`), Booleans verlangen selbstschnittfreie geschlossene Loops (Issues #72/#38) | f64-Frontend über int-Backend, sehr robust | int64-Skalierung, Genauigkeit = Skalenwahl |
| 6. Segment-Provenance (Audit) | **Booleans: direkt aus der API** (`BooleanResultPline.subslices`: `source_is_pline1` + Vertex-Index-Fenster; Endpunkte liegen exakt auf der Quelle, T8: 0.0). **Offset: exakt rekonstruierbar per Rückprojektion** — jedes Ergebnis-Segment liegt konstruktiv bei Abstand genau δ zu genau einem Quellsegment (Segment-Provenance) oder zu zwei (= Quell-Vertex → Join-Bogen); T8: 5 Segmente + 3 Join-Bögen korrekt klassifiziert, Distanzfehler 4.7e-16. Kein Fuzzy-Matching. Alternativ liegt die interne Offset-Pipeline öffentlich (`polyline::internal::pline_offset`: raw offset segs sind 1:1 zu Quellsegmenten indiziert) | **nein**: `outline()`/Booleans liefern nackte Punktlisten; nur Nearest-Match-Heuristik, zusätzlich verwaschen durch Flatten-Sagitta | nein (Punktlisten) |
| Performance (Regen-Budget) | 20-Segment-Profil: **~10 µs**/Offset; 1000-Segment-Profil: 5.4 ms Offset, 0.54 ms Union (T7) | Kreis-Offset 0.08–3.6 ms je nach Flatten-N | — |

Kernzahlen (Auszug aus a1_offset_lib.csv): t1_erode radial_err_max 2.2e-16,
t1b_dilate 4.4e-16, t2_erode_fillet 2.8e-16, t3_split_loop0 area_err 2.2e-16,
t4_hole_dilate area_err 3.6e-15, t5_union area_err 4.4e-16,
t8_offset_prov backproj_dist_err_max 4.7e-16, t6_flat_n1024 4.7e-6.

## Integrationshinweise für Phase A2

* **API:** `Polyline<f64>` + `PlineSource::parallel_offset(_opt)` /
  `boolean(_opt)`; für Profile mit Löchern/Inseln die `Shape`-API
  (`Shape::from_plines` mit CCW-Außenkonturen + CW-Löchern,
  `Shape::parallel_offset`). Booleans sind Pline-Paar-basiert — Multi-Loop-
  Profile (Loop-Listen aus der Regionenfindung) müssen loop-paarweise mit
  pos/neg-Buchführung komponiert werden; das ist der Hauptaufwand in A2.
* **Vorzeichen-Konvention:** positives Offset = links der Laufrichtung =
  **innen bei CCW-Loop**. Erosion um δ (der entscheidende Fall) =
  `parallel_offset(+δ)` auf CCW-Konturen; Dilation = −δ. `Shape` folgt
  derselben Konvention (empirisch verifiziert, T4).
* **Bulge ↔ sketch2d-Bogen:** `SketchSegment::Arc { center, radius,
  start_angle, sweep_angle }` → Vertex am Bogen-Startpunkt mit
  `bulge = tan(sweep_angle/4)` (CCW positiv), nächster Vertex am Endpunkt.
  Rückweg: aus (v1, v2, bulge) sind Sweep = 4·atan(bulge), Zentrum und
  Radius geschlossen rekonstruierbar (Formeln im Spike, `bulge_arc`).
  Bögen > 180° (bulge > 1) funktionieren gemessen exakt (T1b) — kein
  Splitting nötig. Volle Kreise brauchen ≥ 2 Vertexes.
* **f32→f64:** sketch2d/protocol speichert f32; offsetten in f64, Ergebnis
  zurück nach f32 quantisieren (Fehlerbudget 1e-7-Klasse kommt dann aus dem
  f32-Roundtrip, nicht aus der Bibliothek).
* **QuadBezier fehlt:** cavalier_contours kennt keine Béziers. Profile mit
  `SketchSegment::QuadBezier` entweder (a) vorab per Biarc/Arc-Spline auf
  Toleranz approximieren oder (b) in der ersten Ausbaustufe vom Einfalten
  ausschließen (f−δ stehen lassen). Empfehlung: (b) zuerst, (a) als Folge-
  Ticket — deckungsgleich mit der Audit-Einschränkung auf Erosions-Folds.
* **Orientierungs-Gotcha:** `BooleanOp::Not` kann die Ergebnis-Loop **CW**
  zurückgeben (T5: result_ccw=0). Nach jedem Boolean per `area()`-Vorzeichen
  normalisieren (`invert_direction_mut`), bevor weitergerechnet wird.
* **Toleranzen:** Die Default-Epsilons (`pos_equal_eps` 1e-5 u. a.) sind
  **absolut**, nicht relativ — für mm-Profile ok, bei sehr kleiner Geometrie
  über `PlineOffsetOptions`/`PlineBooleanOptions`/`ShapeOffsetOptions`
  mitskalieren.
* **Robustheits-Grenzen (Upstream-Issues):** Booleans erwarten geschlossene,
  selbstschnittfreie Eingaben (#72, #38); der Offset selbst kann Selbst-
  schnitte via `handle_self_intersects: true`. Loops aus der sketch2d-
  Regionenfindung erfüllen die Voraussetzung; degenerierte Nutzer-Profile
  vorher per `scan_for_self_intersect` abweisen. `collapsed_area_eps`
  setzen, um Null-Flächen-Loops nach Booleans zu unterdrücken.
* **Provenance-Rezept für `wall_subs`:** Booleans: `subslices` direkt
  konsumieren (Quell-Pline + Index-Fenster je Slice). Offset: Ergebnis-
  Segment-Mittelpunkt gegen die Quellsegmente rückprojizieren — Abstand ist
  konstruktiv exakt δ; ein eindeutiges Minimum = Quellsegment, zwei gleiche
  Minima = Join-Bogen am gemeinsamen Quell-Vertex (Ecken-Provenance). Damit
  bleibt die Sub-Face-Provenance je Wandsegment ohne Heuristik erhalten.
* **Dependency-Platzierung:** fürs Produkt kommt die Dependency ins
  Kernel-Crate (dort läuft `push_down_offsets`); dieser Spike hält sie
  bewusst nur in metricity-lab. Kernel kompiliert unverändert für
  wasm32-unknown-unknown (geprüft), cavalier_contours ebenso.
