# Experiment-Harness: `crates/metricity-lab`

Gemeinsame Messbank des Forschungsprogramms (siehe `README.md` in diesem
Verzeichnis). Alle Experimente der Forschungsagenten laufen gegen dieses
Crate — gleiche Ground Truth, gleiche Metriken, gleicher Korpus, damit
Ergebnisse zwischen den Agenten vergleichbar sind.

Läufe:

    cargo run -p metricity-lab --release -- list          # Korpus anzeigen
    cargo run -p metricity-lab --release -- all           # kompletter Baseline-Lauf → results/baseline/
    cargo run -p metricity-lab --release -- <szene>       # Einzelszene → results/scenes/<szene>.json
    cargo test -p metricity-lab                           # Ground-Truth-Validierung + Smoke

Nützliche Flags: `--quick` (weniger Punkte/Bench-Evals), `--grid N`
(Ground-Truth-Gitter), `--points N`, `--seed S`, `--out DIR`,
`--no-bench`. Immer `--release` — die Ground Truth ist Brute Force.

## Begriffe und Konventionen

- **Feld** ([`Field`]-Trait, `src/field.rs`): `p ∈ ℝ³ → f(p)`, `f < 0`
  innen, Nullmenge = Oberfläche. `f64`-Schnittstelle; die Kernel-Felder
  rechnen intern in `f32`.
- **Ground Truth** = exakte euklidische Distanz **zur Nullmenge des
  Feldes**. Für min/max/smin-Felder fällt die Nullmenge mit der
  gewollten Oberfläche zusammen; der gemessene Fehler ist genau der
  Metrikverlust. (Wer eine Variante mit *verschobener* Nullmenge baut,
  misst gegen die GT der Referenz — siehe unten.)
- **Modellgröße** = größte AABB-Ausdehnung der Szene (~100–150 mm im
  Korpus). Alle Bänder/Toleranzen sind relativ dazu; Fehlerwerte in den
  Reports sind absolut in mm.
- **Bänder**: Nahfeld |d| < 2 % der Modellgröße, Mittelfeld 2–10 %,
  Fernfeld > 10 %; jeweils getrennt nach innen (d < 0) / außen.
- **Vorzeichen des Fehlers**: `err = f − d_wahr`. Positiv = Feld
  überschätzt die Distanz (gefährlich für Sphere-Tracing/Offsets),
  negativ = unterschätzt (konservativ, aber langsam/verzerrt).

## Ground Truth: Verfahren und Validierung

`GroundTruth::build(field, aabb_min, aabb_max, &GtOptions)` —
zweistufig (`src/ground_truth.rs`):

1. **Oberflächen-Punktwolke**: dichte Achsen-Linien in allen drei
   Richtungen über dem gepolsterten AABB (`grid`² Linien je Richtung);
   jeder Vorzeichenwechsel wird per Bisektion auf die Nullstelle
   verfeinert. Vorzeichen sind auch bei metrisch verzerrten Feldern
   exakt, darum ist die Punktwolke exakt auf der Oberfläche.
   Nearest-Neighbor über einen k-d-Tree (kiddo).
2. **Richtungs-Raffinierung** pro Abfrage (`GroundTruth::distance`):
   der NN-Treffer ist nur eine obere Schranke (Fehler O(h), im Nahfeld
   bis ~h/2 tangential). Deshalb wird die Distanz als Minimum der
   *ersten* Nullstelle über Strahlrichtungen minimiert (Kompass-Suche
   auf der Richtungskugel, 8 Richtungen, Seeds aus 4 NN-Punkten + der
   Gradientenrichtung; Nullstellen per March + Bisektion). Das Segment
   zum nächsten Oberflächenpunkt kreuzt die Fläche nicht vorher — die
   erste Nullstelle in optimaler Richtung IST die exakte Distanz.

**Validierungsstatus** (`tests/ground_truth_validation.rs`, Pflicht;
Gitter bewusst nur 64, die Genauigkeit muss aus der Raffinierung
kommen; 1500 Punkte je Fall, inkl. |d| bis ~1e-4·Größe):

| Fall | max. Fehler (relativ zur Modellgröße) |
|---|---|
| Kugel r=40 | 1.3e-7 |
| Box 80×50×30 (scharfe Kanten/Ecken) | 8.0e-5 |
| rotierte Box (0.7 rad, schiefe Achse) | 2.4e-4 |
| exakte Offset-Fläche (Kugel 30 + Offset 10) | 1.3e-7 |

Vertragsgrenze ist 1e-3 der Modellgröße; real liegt die GT ein bis vier
Größenordnungen darunter. Fehlerbudget dominiert von Ecken-Tälern der
Kompass-Suche + `f32`-Feldrauschen; glatte Flächen sind praktisch exakt.

## Eigene Feld-Variante messen (Rezept für Forschungsagenten)

Eine Variante ist irgendein `fn([f64; 3]) -> f64` — als `FnField`
verpackt oder als eigener `Field`-Impl. Typischer Experimentcode
(eigenes Testfile oder Binary im eigenen Agent-Verzeichnis, gegen
`metricity_lab` als Dependency):

    use metricity_lab::{CsgField, FnField, GroundTruth, GtOptions};
    use metricity_lab::metrics::{distance_errors, gradient_report,
        sample_near_surface, sample_uniform};
    use metricity_lab::scenes;

    let scene = scenes::scene_by_name("tangent_spheres_m25p0pct").unwrap();
    let ist = CsgField::visual(&scene.node);          // IST-Feld des Kernels

    // Ground Truth gegen die REFERENZ-Nullmenge bauen (hier: das
    // IST-Feld; Varianten mit identischer Nullmenge teilen sich die GT).
    let a = scene.node.aabb();
    let (lo, hi) = ([a.min[0] as f64, a.min[1] as f64, a.min[2] as f64],
                    [a.max[0] as f64, a.max[1] as f64, a.max[2] as f64]);
    let gt = GroundTruth::build(&ist, lo, hi, &GtOptions::default());

    // Punktmengen (deterministisch, Seeds mitpublizieren!).
    let mut pts = sample_uniform(lo, hi, 6000, 42);
    let near = sample_near_surface(&gt, 6000, 0.05, 43);
    pts.extend_from_slice(&near);

    // Die eigene Variante:
    let variante = FnField(|p| meine_korrektur(p));
    let (report, rows) = distance_errors(&gt, &variante, &pts);
    let grad = gradient_report(&variante, gt.model_size, &pts, &near, 44);

Wichtige Regeln:

- **Nullmenge verschoben?** Wenn die Variante die Oberfläche selbst
  bewegt (nicht nur die Metrik), misst `distance_errors` gegen die GT
  der Referenz die Summe aus Flächenverschiebung und Metrikfehler.
  Dann zusätzlich eine GT auf der Variante selbst bauen und beide
  Reports nebeneinanderlegen (so behandelt der Runner das kompilierte
  Tape, s. u.).
- **Kosten**: `metrics::cost_report` misst ns/Eval; jede Korrektur muss
  ihre Kosten gegen die Baseline-Spalten `csg_ns_per_eval` /
  `tape_ns_per_eval` ausweisen (Pflicht-Metrik 4 der Charta).
- **Tiefen-Sweeps**: `scenes::deep_csg(n)` / `scenes::smin_chain(n)`
  sind die parametrischen Familien; Fehler-über-Tiefe wie in
  `sweep_hard_depth.csv` berichten.
- `GroundTruth::distance` ist teuer (~10³–10⁴ Feld-Evals pro Punkt).
  Punktmengen einmal ziehen, GT-Werte cachen (`rows` aus
  `distance_errors` enthält `(d_wahr, err)` pro Punkt).

## Szenen bauen

`src/scenes.rs`. Szenen sind rohe `CsgNode`-Bäume direkt gegen die
Kernel-API — kein Feature-System, keine Persistenz, dadurch exakt
reproduzierbar. Eigene Szenen: `CsgNode`-Konstruktion nachahmen
(`Leaf`+`FeatureId::from_u32`, `Union`/`Difference`/`SmoothUnion`/
`MaskedBlend`/`Twist`/…) und ein `Scene {name, description, node,
gt_grid}` zurückgeben. `gt_grid > 0` überschreibt das GT-Gitter (nötig,
wenn Features dünner als ~2 Gitterzellen sind — Faustregel
`grid ≳ 2·Modellgröße/Feature-Dicke`).

Korpus (Namen siehe `-- list`): Kalibrierszenen (`sphere`, `box`,
`rot_box`), dünne Wände (`thin_wall_*`, Box-minus-Box-Schale, Wandstärke
5 %…0.5 %), fast tangentiale Booleans (`tangent_spheres_*`, Abstand
+10 %…−25 % des Radius), tiefe CSG-Bäume (`deep_csg_<N>`, N ≤ 64),
smin-Kaskaden (`smin_chain_<N>`), Domain-Warps (`twist_box`, `bend_bar`,
`twist_thin_wall`), `gyroid_box` (nicht-1-Lipschitz-Leaf) und
`masked_blend_edge` (Kanten-Fillet-Pfad). Twist/Bend/Gyroid sind direkt
in der Kernel-API erreichbar (`CsgNode::Twist`/`Bend`,
`Primitive::Gyroid`) und daher im Korpus enthalten.

## Reports lesen

`results/<lauf>/scenes/<szene>.json` (voll) + `summary.csv` (eine Zeile
je Szene). Spalten der Summary:

- `near/mid/far × out/in × mean/p95/max/bias` — Distanzfehler |f −
  d_wahr| in mm; `bias` ist der mittlere *signierte* Fehler (s. o.).
- `grad_dev_*` — Statistik von |‖∇f‖ − 1| (zentrale Differenzen,
  h = 1e-3·Modellgröße). Das JSON enthält zusätzlich das
  ‖∇f‖-Histogramm (25 Bins über [0, 2.5]).
- `seam_jump_p95/max` — ‖∇f(p) − ∇f(p+δ)‖ über Nahfeld-Paare
  (|δ| = 2e-3·Modellgröße): Gradienten-Sprünge an Nahtstellen. Glatte
  Felder ≈ 0; harte min/max-Nähte springen O(1) (bis 2 = Normalen
  antiparallel).
- `csg_ns_per_eval` / `tape_ns_per_eval` — Kosten pro Eval: direkter
  `CsgNode::sdf` (Visual) vs. kompiliertes Tape (`eval_opcodes`,
  CPU-Referenz des GPU-Interpreters).
- `tape_max_abs_diff` — max. |csg_visual − tape| über die Messpunkte.
  Divergiert das Tape (> 1e-3·Modellgröße, aktuell nur beim
  MaskedBlend-Render-Fade aus #382), enthält das Szenen-JSON unter
  `tape_distance` einen eigenen Report gegen die Tape-eigene GT.
- Sweeps: `sweep_hard_depth.csv` / `sweep_smin_chain.csv` — Nahfeld-
  und Gradient-Fehler über der Verschachtelungstiefe N.

## Baseline (Stand v0.16.0, `results/baseline/`)

Voller Lauf über den Korpus mit dem IST-Feld (`EvalMode::Visual`),
Gitter 96 (dünne Wände 192), 12 000 Punkte/Szene, Seed 4242. Die
Zahlen, gegen die jede Verbesserung antritt — Details und alle Szenen in
`results/baseline/summary.csv`. Kernbefunde:

**1. Exakte Zonen bestätigt.** Kugel/Box/rotierte Box, dünne Wände im
Nahfeld, Union-*Außenseiten* (auch fast tangential, Spalt ≥ 0) und der
MaskedBlend-Fillet im Nahfeld sind praktisch exakt (Fehler ~1e-6 mm =
`f32`-Rauschen). Das harte min/max ist außen an konvexen Übergängen
kein Problem — der Schaden sitzt woanders:

**2. Konkave Nähte harter Booleans zerstören die INNEN-Distanz.**
Überlappende Kugeln (`tangent_spheres_m*`): außen weiter exakt, innen
misst |min| zur *verdeckten* (weggebooleanten) Fläche statt zur realen
Oberfläche — Fehler bis 0.45 mm (−1 % Überlappung) bzw. 0.72 mm nahfeld
/ ~8 mm fernfeld (−25 %), Bias positiv (Feld meldet zu wenig Tiefe).
Gradienten-Sprünge an den Nähten erreichen ‖Δ∇f‖ ≈ √2 (Box exakt
1.4142) bis 1.93 (dünne Wand, fast antiparallele Normalen).

**3. Fehler wächst monoton mit der CSG-Tiefe** (`sweep_hard_depth.csv`,
Nahfeld außen): mean 3e-6 mm (N=1) → 2.5e-3 (N=8) → 1.6e-2 mm mit
Ausreißern bis 1.75 mm (N=64); innen ähnlich (max 1.2 mm). Kein
Plateau — jede Ebene stapelt Schranken auf Schranken. Kosten linear:
6.5 ns/Eval (N=1) → 535 ns (N=64, `CsgNode::sdf`).

**4. smin (zirkuläres `smin_poly`) ist nahe der Fläche gut, in der
Naht-Tiefe nicht.** Kaskaden (`sweep_smin_chain.csv`): Nahfeld-p95
0.05 mm (N=2) → 0.4 mm (N=16, r_blend=6 mm), max ~0.9 mm ≈ der
theoretische Undershoot (√2−1)·r ≈ 2.5 mm bleibt unerreicht, wächst
aber mit der Kettenlänge; ‖∇f‖ weicht bis 0.84 von 1 ab (p95 ~0.1) —
die Blendzone hat Flachstellen (‖∇f‖ ≪ 1), nicht nur die √2-Spitze.

**5. Der #382-Render-Fade des Tapes ist ein globaler ÷√2.**
`masked_blend_edge` ist die einzige Szene mit Tape-Divergenz
(`tape_max_abs_diff` = 18.3 mm): das kompilierte Feld unterschätzt
JEDE Distanz um den Faktor 1−1/√2 ≈ 29.3 % (gemessen: Fernfeld mean
6.9 mm bei ~23 mm wahrer Distanz, Nahfeld max 0.47 mm) — Zero-Set
identisch, Metrik global gestaucht. Für Offset/Shell auf dem
Render-Feld wäre das fatal; der Mesher-Pfad (Visual) ist davon frei.

**6. Warps und Gyroid sind erwartbar nicht-metrisch.** Twist 90°:
Nahfeld-p95 0.10 mm, Mittelfeld bis 1.9 mm; Bend 60°: 0.30/3.3 mm;
Gyroid-Leaf (mit eingebautem Lipschitz-Squash): mean |‖∇f‖−1| = 0.37,
Nahfehler ~0.8 mm p95.

**7. Kosten-Referenz:** CPU-Tape (`eval_opcodes`) ist auf dem Korpus
durchweg 1.2–2.3× LANGSAMER als der direkte `CsgNode::sdf`-Eval
(z. B. Kugel 6.6 vs 17.5 ns; deep_csg_64 535 vs 674 ns; 1M Evals ≈
0.5–0.7 s bei Tiefe 64). Der Tape-Vorteil liegt auf der GPU, nicht in
der CPU-Referenz — Korrektur-Operatoren sollten ihre Kosten gegen
BEIDE Spalten ausweisen.

## Offset/Shell-Property-Harness (#384)

`crates/metricity-lab/tests/offset_fuzz.rs` — ein seeded,
reproduzierbarer Property-Harness (kein cargo-fuzz, gewöhnliche
`#[test]`s mit dem in-crate `SplitMix64`; der Seed steckt in jeder
Fehlermeldung). Er würfelt degenerierte Offset/Shell-Szenen und prüft je
Szene drei Eigenschaftsklassen.

**Familien** (Szenen-Enum, je `OFFSET_FUZZ_ITERS` Szenen — Default 32):
`RotatedOffsetBox` (rotierte Naht, beide Vorzeichen), `RotatedUnionOffset`
(Erosion über Dieder-Union, optional global rotiert), `TangentialContact`
(Kugel-auf-Box-Fläche bzw. koplanare Boxen + Erosion), `SmallFeatureVsLargeDelta`
(|δ| nahe/über halber Kante → Kollaps als Grenzfall), `MultiFaceCorner`
(konvexe ≥4-Flächen-Spitze aus Boolean-Kette + Erosion), `StackedDualStops`
(Difference-Dilation + Union-Erosion, Offset über Offset),
`ClosedShellBoolean` (geschlossene Shell auf Boolean), `OffsetShellCombo`
(Dilation über geschlossener Shell).

**Properties.** P1 — nach jeder Pipelinestufe werden die vier
`offset_invariants::check_*` DIREKT aufgerufen (nicht via debug_assert);
jede Verletzung ist ein Befund. P2 — `synthesize_offset_channels_reported`
+ `prune_dead_channels` panikt nie, das Feld ist überall endlich, ein Punkt
weit außen ist positiv (Erosions-Kollaps ist gültige Geometrie, kein
Befund). P3 (Kern) — der `CapabilityReport` wird gegen die Ground Truth
gemessen: für jeden Feld-Oberflächenpunkt `p` ist `|d_base(p) − δ|` die
Distanz zur WAHREN Offset-Fläche (`d_base` = GT-Abstand zum Grundkörper,
Doppel-GT). `exact`/`tolerance_bounded` müssen die Schranke halten,
`conservative` trägt keine.

**Schwellen — empirisch geeicht, nicht geraten.** Der GT-Boden wird an
bekannten EXAKTEN Fällen kalibriert. Die glatte Kugel-Dilation gibt
~1e-7·Modellgröße; maßgeblich ist aber ein Panel rotierter Box-DILATIONEN
(~2e-4·Modellgröße p99), denn AUSSEN-Punkte der Rundungspatches gegen
scharfe Box-ECKEN sind die schwerste GT-Messlage (Punkt-Features, Kompass-
Stranding). P3 erzwingt das ROBUSTE **p99** (nicht max): vereinzeltes
GT-Stranding lebt im Extrem-Schwanz, eine echte VERTEILTE Abweichung hebt
das p99. Schwelle: `exact` = `3·sharp_rel·model_size` (Boden ≈4e-4),
`tolerance_bounded` = zusätzlich `+ channel_tol`. Max wird zusätzlich
berichtet.

**Reproduktion.** Ein Failure nennt `[Familie seed=N params]`. Default-
Lauf: `cargo test -p metricity-lab --test offset_fuzz`. Langer Lauf:
`OFFSET_FUZZ_ITERS=320 cargo test -p metricity-lab --test offset_fuzz
offset_shell_property_fuzz -- --nocapture`. Eine Einzelszene baut man mit
`generate(Family::X, seed)`.

**Aufgedeckte Befunde** (echte, verifizierte Pipeline-/Vertrags-Lücken;
NICHT im Zuge des Harness gefixt, je als `#[ignore]`-Minimalrepro
festgehalten, damit der CI-Gate grün bleibt):

- **F1 (BEHOBEN)** — `Offset(closed_shell, δ<0)` brach Pipeline-Invariante 4:
  `push_down` schob die Erosion durch die shell-expandierte `Difference` und
  dilatierte den `InnerWall`-Subtrahenden; da `offset_can_push_through`
  InnerWall ablehnte, umschloss ein `Offset` den InnerWall → er saß nicht mehr
  direkt als `Difference.r`, und die Höhle faltete nie. Fix: InnerWall ist
  feld-transparent, der Offset kommutiert jetzt hindurch
  (`Offset(InnerWall(X), δ)` ≡ `InnerWall(Offset(X, δ))`) — Erosion über
  geschlossener Shell faltet damit exakt. Regressionstest:
  `offset_invariants_tests::pos_erosion_over_closed_shell_keeps_innerwall_and_folds`;
  die Familie `OffsetShellCombo` würfelt wieder beide Vorzeichen.
- **F2 (BEHOBEN)** — DEGENERIERTE Union-Erosion (koplanarer/tangentialer
  Flächenkontakt) wich in der Nullmenge stark ab (bis >13 mm, p99/mean im
  mm-Bereich), Status blieb fälschlich `exact`/`tolerance_bounded`.
  `min(A,B)−δ` unterschätzt an der submergierten gemeinsamen Fläche die wahre
  Tiefe; an einem zero-thickness-Kontakt findet die SSI-Nahtsuche keine
  abdeckende Nahtkurve, und ein Arc-Spline-Kanal (1D-Kurve) kann den flächigen
  2D-Grenzbereich ohnehin nicht korrigieren. **Fix (Ehrlichkeits-Heuristik,
  #394):** der Kernel erkennt den koinzidenten, gegenläufig orientierten
  (anti-parallel), flächig überlappenden Carrier-Flächen-Kontakt rein
  strukturell aus den Halfspace-Karten
  (`offset_channel::coplanar_interface_present`) und meldet den Stopp ehrlich
  als `conservative` (feld-identisch, reiner Report — Kandidat 1). SAUBERE,
  transversale Union-Erosion (Dieder-Unionen, Kugel-auf-Box) erodiert
  weiterhin metrisch exakt und bleibt unangetastet. Damit erzwingt P3 nun
  BEIDE Union-Erosions-Familien (`RotatedUnionOffset`, `TangentialContact`)
  HART; die `p3_enforced`-Ausnahme und der `#[ignore]`-Minimalrepro sind
  entfernt. **P3-Robustheit:** der Gate fordert seither BEIDE Schranken —
  p99 UND Mittelwert über der Zusage. Grund: die GT-Kompass-Raffinierung
  strandet vereinzelt an scharfen konkaven Naht-End-Triple-Junctions (s.
  „Bekannte Limitationen"), was p99 grid-instabil zwischen ~0 und mehreren mm
  springen lässt, während das Mittel ~1e-4·size bleibt. Ein echter
  Vertragsbruch ist dagegen flächig (hoher p99 UND hoher Mittelwert), F2 selbst
  wird ohnehin schon vom Kernel degradiert. Die Doppel-Schranke verhindert
  also, dass isoliertes GT-Stranding als Lüge quittiert wird, ohne einen echten
  Befund zu verstecken.

## Bekannte Limitationen des Harness

- **GT misst gegen die Nullmenge des Feldes**, nicht gegen eine
  CAD-Soll-Geometrie. Flächenverschiebungen einer Variante brauchen den
  Doppel-GT-Vergleich (s. o.).
- Die Kompass-Raffinierung ist ein lokales Verfahren mit mehreren
  Seeds; an Ecken-/Kanten-Tälern bleibt ein Restfehler (beobachtet
  ≤ 2.4e-4 der Modellgröße, Validierungs-Tabelle). Wer < 1e-4 braucht,
  erhöht `grid` und prüft gegen analytische Fälle nach.
- Erste-Nullstellen-Suche marcht mit Schrittweite ≤ ½ GT-Gitterzelle:
  Features deutlich dünner als eine Gitterzelle können übersprungen
  werden → `gt_grid` an die dünnste Wand koppeln (Faustregel oben);
  unterhalb ~0.2 % der Modellgröße wird der Aufbau unpraktikabel teuer.
- Innen/außen kommt vom Feld-Vorzeichen; ein Feld mit *falschem*
  Vorzeichen (kaputte Variante) macht die signierte GT unbrauchbar —
  die Validierungstests decken das nicht ab.
- `f32`-Kernel-Felder begrenzen alles auf ~1e-5·Koordinate; die
  `f64`-Messschicht fügt nichts hinzu, kann das aber nicht heilen.
- Kostenmessung ist CPU-single-thread (Wall-Clock, release); sie ist
  ein *relatives* Maß zwischen Feldern, kein GPU-Timing des
  WGSL-Interpreters.
- Gyroid/CustomSdf skalieren ihr Feld leaf-intern (Lipschitz-Trick);
  die Baseline misst das Produktfeld — Aussagen betreffen dort das
  Gesamtfeld, nicht den Boolean-Operator allein.
