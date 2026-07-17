# Agent A („PDE") — Forschungslog

Level-Set-/PDE-Sicht auf die SDF-Metrik-Treue (Ticket #384). Alle Experimente in
diesem Verzeichnis; Pipeline-Grounding: `crates/kernel/src/csg.rs`
(`hard_combine`, `smin_poly`/`blend_min`, `sdf_open`), `sdf_compiler.rs`
(`march_relax`-Dämpfungen), `api/voxel.rs` (200³-Occupancy-Sampling, ein
WASM-Call, `EvalMode::Visual`).

---

## Iteration 1 — Redistancing-Grundlagen + 2D-Validierung an der konkaven Naht

### Quellen (gesichtet 2026-07-17)

1. **H.-K. Zhao (2005), „A fast sweeping method for eikonal equations", Math.
   Comp. 74, 603–627.** Gauss-Seidel-Sweeps in 2^n Orderings, Godunov-Upwind;
   Konvergenz in O(N) mit konstanter Sweep-Zahl für Distanzfunktionen;
   bewiesene Fehlerordnung **O(|h·log h|)** für die First-Order-Diskretisierung
   (der log-Faktor stammt aus den Rarefaction-Fans an Ecken).
   https://www.math.uci.edu/~zhao/homepage/research_files/FSM-triangle.pdf (Folgearbeit Triangular Meshes, Qian/Zhang/Zhao SINUM 2007).
2. **G. Russo, P. Smereka (2000), „A remark on computing distance functions",
   J. Comput. Phys. 163, 51–67.** Subcell-Fix: Upwind-Reinit OHNE Fix
   verschiebt das Zero-Level-Set; der Fix pinnt die Interface-Zellen über die
   Interface-Lage aus dem Ausgangsfeld → Zero-Set-Erhalt, 2. Ordnung nahe
   Interface. (Via ResearchGate/Übersicht in Min 2010.)
3. **C. Min (2010), „On reinitializing level set functions", JCP 229,
   2764–2772.** Bestätigt: Subcell-Fix ist der Standardweg für
   interface-erhaltendes Reinit; 2. Ordnung fürs Feld, für Krümmung zu wenig.
   http://math.ewha.ac.kr/~chohong/publications/article_15.pdf
4. **M. Sussman, P. Smereka, S. Osher (1994), JCP 114, 146–159.**
   PDE-Reinit ∂τφ = sgn(φ)(1−|∇φ|): iterativ, CFL-beschränkt, ohne Subcell-Fix
   bewegt es das Interface — für unsere Zwecke (einmalige on-demand
   Redistanzierung) den direkten Eikonal-Lösern unterlegen. Nur als Kontext.
5. **W.-K. Jeong, R. Whitaker (2008), „A fast iterative method for eikonal
   equations", SIAM J. Sci. Comput. 30, 2512–2534.** FIM: aktive Liste +
   Jacobi-artige Updates, GPU-freundlich, O(N); Basis der meisten
   GPU-Eikonal-Löser. (Übersicht + Verbesserungen: arXiv:2106.15869.)
6. **M. Detrixhe, F. Gibou, C. Min (2013), „A parallel fast sweeping method
   for the Eikonal equation", JCP 237, 46–55.** Cuthill-McKee-/Diagonal-
   Ordering macht FSM parallel ohne Mehr-Iterationen.
   http://math.ewha.ac.kr/~chohong/publications/article_23.pdf
7. **M. Royston, A. Pradhana, B. Lee, Y.T. Chow, W. Yin, J. Teran, S. Osher
   (2018), „Parallel redistancing using the Hopf–Lax formula", JCP 365, 7–17**
   (+ B. Lee, J. Darbon, S. Osher, M. Kang (2017), „Revisiting the
   redistancing problem using the Hopf–Lax formula", JCP 330, 268–281).
   **Punktweise, gitterfreie** Redistanzierung: d(x) = kleinstes t, s.d.
   min_{‖y−x‖≤t} φ(y) = 0; innere Minimierung per projiziertem
   Gradientenverfahren, äußere per Sekanten-Iteration. Kein Nachbar-Stencil →
   embarrassingly parallel, „seldom affected by the curse of dimensionality".
   → DIREKT auf unser Tape anwendbar (Iteration 2).
8. **R.I. Saye (2014), „High-order methods for computing distances to
   implicitly defined surfaces", CAMCoS 9(1), 107–141** (Code: Algoim,
   https://algoim.github.io/). Closest-Point via Newton, initialisiert aus
   Punktwolke der Fläche; Ordnung = Approximationsordnung des Implicits.
   Narrow-Band-High-Order-Redistancing ohne PDE-Sweep.
9. **Z. Marschner, S. Sellán, H.-T. D. Liu, A. Jacobson (2023), „Constructive
   Solid Geometry on Neural Signed Distance Fields", SIGGRAPH Asia 2023**
   (PDF gesichtet, S. 1–4). Taxonomie: **Exact SDF ⊋ Pseudo-SDF (eikonal
   a.e., aber Distanz-Property verletzt) ⊋ Conservative SDF (|f| ≤ |d|)**.
   min/max-CSG erzeugt Pseudo-SDFs; „eikonal property alone is insufficient" —
   |∇f|=1 fast überall gilt auch für unsere min/max-Felder, der Fehler liegt in
   der **Closest-Point-Property** (x − f(x)∇f(x) muss auf der Fläche liegen).
   Deren Fix ist ein Trainings-Loss (neural) — für unsere analytische Pipeline
   nicht direkt nutzbar, aber die Charakterisierung trägt.
10. **GPU-Distanztransformationen mit Seeds:** Rong/Tan 2006 (Jump Flooding,
    O(log n) Pässe, approximativ) und Cao et al. 2010 (PBA, exakt) brauchen
    **rasterisierte Seeds** der Fläche — d.h. sie lösen erst NACH einer
    Oberflächen-Diskretisierung; Genauigkeit der Seeds limitiert alles
    (Distanz zu Voxel-Zentren statt zur Fläche). Für uns nur als
    Beschleuniger einer Voxel-Stufe interessant, nicht als Genauigkeitsquelle.

### Theorie-Erwartung (Hypothesen für die Experimente)

- **T1:** min/max-Booleans über exakten Operanden-SDFs sind exakt überall
  AUSSER im „Fan-Keil" hinter konkaven Nähten (Region, deren Closest Point auf
  dem weggebooleanten Flächenstück liegt). Dort UNTERschätzt das Feld die
  Außendistanz (konservativ, Sphere-Trace-sicher, aber Offset/Shell falsch).
  Der wahre Wert im Keil ist die Distanz zur Nahtkante (Kegel/„rarefaction
  fan" der Eikonal-Gleichung).
- **T2:** Redistancing (FSM/FIM/FMM) mit gefrorenem Interface-Band
  rekonstruiert genau diesen Fan — Viskositätslösung = echte Distanz — und
  RUNDET die konkave Kante des Zero-Sets dabei NICHT (das Zero-Set steckt im
  gefrorenen Band; Kinks der Distanzfunktion liegen auf Medialachse + Fan-
  Zentrum und sind zulässige Viskositäts-Features). Erwarteter Restfehler:
  O(h) generisch, O(h·log h) im Fan (Zhao 2005).
- **T3:** Die Kantenrundung passiert erst beim RESAMPLING (tri-/bilineare
  Interpolation eines Gitterfelds kann den Kink des Feldes an der konkaven
  Kante nur mit O(h)-Fehler darstellen; auf glatten Flächenstücken O(h²)).
  D.h.: Gitter-Redistancing als *Feld-Ersatz* im Renderer kostet
  Kantenschärfe; als *Korrektur-/Abfragestufe* (Offset-Distanzen, Meshing,
  Queries) nicht.

### Experiment E1 — 2D, Union zweier Kreise, FSM-Redistancing

Skript: `exp1_fsm2d.py` (NumPy; Eikonal-Löser = monotones Godunov-Jacobi bis
Konvergenz — numerisch identischer Fixpunkt wie FSM, gleichzeitig genau das
Update-Muster eines GPU-Compute-Passes).

Setup: Kreise r=1 um (±0.55, 0), Domain [−2.4, 2.4]²; konkave Nahtecken bei
(0, ±0.835). Ground Truth analytisch (Distanz zu den retained Arcs bzw. zu den
Eckpunkten). Feld f = min(d1,d2) (unser `hard_combine`-Union). Metriken: Fehler
im Außenband 0<d≤0.5, Fan-Strahl-Test (Bisektion entlang Strahlen ab der Ecke),
Offset-Kontur-Lage f=δ, Zero-Set-Verschiebung nach Redistancing,
Konvergenzordnung über h.

### E1-Ergebnisse (Lauf 2026-07-17, `python3 exp1_fsm2d.py`)

**Erster Lauf war eine widerlegte Prämisse (dokumentiert als Negativbefund):**
Ich hatte den Außen-Fan an der konkaven Naht des *Union* erwartet —
`raw_ext_max = 0.0` auf allen Auflösungen: das min-Feld ist im **gesamten
Außenraum exakt**. Das ist kein Zufall, sondern ein Satz:

> **Mini-Satz (Exaktheits-Seite harter Booleans).** Seien d_A, d_B exakte
> SDFs beliebiger abgeschlossener Mengen. Für p ∉ A∪B ist
> min(d_A,d_B)(p) = dist(p, ∂(A∪B)) **exakt**.
> *Beweis:* O.B.d.A. d_A(p) ≤ d_B(p), q = Closest Point auf ∂A. Läge q in B˚,
> so schnitte die Strecke p→q die Fläche ∂B, also d_B(p) < d_A(p) —
> Widerspruch. Also q ∈ ∂(A∪B), und näher als min geht es nicht, da
> ∂(A∪B) ⊆ ∂A ∪ ∂B. ∎
> Dual (Komplemente): max(d_A,d_B) ist im **Innenraum** von A∩B exakt.
> Fehlregionen: min → nur INNEN (Closest Point liegt auf weggebooleantem
> Flächenstück), max/Differenz → nur AUSSEN (Fan-Keil hinter einspringenden
> Kanten). Konsistent mit Marschner et al. 2023, Fig. 7 (Pseudo-SDF-Isolinien
> weichen im Innern der Union ab).

**Praktische Sofort-Konsequenz für FieldDraft:** Nach jedem harten Boolean ist
genau EINE Seite des Felds metrisch exakt — Union: außen; Intersection und
`a.max(-b)`-Differenz: innen. D.h. Sphere-Tracing und Outward-Offset über
reinen Union-Bäumen sind exakt; kaputt sind (a) Innen-Distanzen des Union
(Shell!, Wandstärke), (b) Außen-Distanzen nach Differenz/Intersection
(Outward-Offset an einspringenden Kanten — der CAD-Normalfall: Tasche,
Bohrung), (c) alles im smin-Blendbereich.

**Messwerte (Band |d_true| ≤ 0.5; Fehler = |f − d_true| bzw. |FSM − d_true|):**

union (f = min): Außen exakt, Fehler nur innen; FSM-Redistancing (Band
eingefroren) repariert innen:

| N | h | raw innen max | FSM innen max | FSM außen max | Zero-Disp | Ecken-Disp |
|---|---|---|---|---|---|---|
| 65 | 0.075 | 0.1258 | 0.0152 | 0.0092 | 9.3e-4 | 1.1e-4 |
| 129 | 0.0375 | 0.1430 | 0.0101 | 0.0051 | 1.8e-4 | 4.9e-5 |
| 257 | 0.01875 | 0.1430 | 0.0066 | 0.0026 | 5.2e-5 | 1.6e-5 |
| 513 | 0.009375 | 0.1430 | 0.0049 | 0.0013 | 1.1e-5 | 1.2e-6 |

intersect (f = max, Lens): Innen exakt (raw_int_max = 0), Fehler außen im Fan;
Offset-Test: Lage der Kontur f = δ (δ=0.2) gegen wahre Offsetfläche,
Fan-Test: Feldwert auf wahrer Distanz 0.3 ab der Ecke:

| N | raw außen max | FSM außen max | Offset-Fehler roh | Offset-Fehler FSM | Fan roh | Fan FSM |
|---|---|---|---|---|---|---|
| 65 | 0.0513 | 0.0155 | 0.0314 | 0.0101 | 0.0386 | 0.0109 |
| 129 | 0.0543 | 0.0112 | 0.0314 | 0.0065 | 0.0386 | 0.0086 |
| 257 | 0.0557 | 0.0078 | 0.0314 | 0.0051 | 0.0386 | 0.0063 |
| 513 | 0.0557 | 0.0047 | 0.0314 | 0.0031 | 0.0386 | 0.0038 |

### Schlussfolgerungen Iteration 1

1. **T1 präzisiert und belegt:** Der Feldfehler harter Booleans ist
   h-unabhängig und seitenselektiv (union innen bis 0.143 ≈ Nahttiefe;
   intersect außen 0.056 ≈ 13–16 % der Abfragedistanz im Fan; Offsetfläche
   δ=0.2 liegt roh um 0.031 = 16 % von δ falsch).
2. **T2 belegt:** Eikonal-Redistancing mit gefrorenem Interface-Band
   rekonstruiert den Fan (Fehler ↓ um Faktor 4–12) und hält das Zero-Set bis
   O(h²) (1.1e-5 bei N=513); die konkave **Ecke wird nicht gerundet**
   (Ecken-Verschiebung 1.2e-6 — die Ecke liegt im gefrorenen Band).
3. **Konvergenzordnung:** glatte Regionen ~O(h) (mean-Fehler), am
   Rarefaction-Fan beobachtet ~O(h^0.6) — schwächer als Zhaos O(h·log h),
   bekanntes Verhalten an Quellpunkt-Singularitäten; Gegenmittel wäre
   factored eikonal (Fomel/Luo/Qian 2009) oder lokal-exakte Eckenbehandlung
   (Distanz-zur-Kante analytisch im Fan-Keil).
4. **Negativbefund:** Volles Redistancing VERSCHLECHTERT die beweisbar exakte
   Seite (union außen: 0 → 0.0013–0.0092 = O(h)-Diskretisierungsrauschen).
   Eine Korrektur muss **seitenselektiv** sein (nur die kaputte Seite
   ersetzen, die exakte Seite vom Tape lassen).

---

## Iteration 2 — Gitterfreie punktweise Redistanzierung (Hopf–Lax auf dem Tape) + 3D-Kosten

### Hypothese

**H-HL:** Die Hopf–Lax-Formulierung (Quellen 7) lässt sich als *punktweise*
Korrektur direkt auf unserem analytischen Tape auswerten — ohne Gitter, ohne
Nachbarn, GPU-parallel: d(x) = kleinstes t mit min_{‖y−x‖≤t} f(y) = 0.
Für 1-Lipschitz-Felder ist die Iteration t ← t + m(t) sicher (Beweis im
Skript-Kopf: für jeden Flächenpunkt z gilt ‖z−x‖ ≥ t + m(t)) — das ist
„Sphere-Tracing auf der echten Distanz", Konvergenz wie Sphere-Tracing, aber
jeder Schritt kostet eine Ball-Minimierung (projizierter Gradientenabstieg,
Multi-Start).

### Experiment E2 (`exp2_hopflax.py`)

Fälle: (A) Lens = max zweier Kreise, Testpunkte im Außen-Fan auf wahren
Distanzen 0.1/0.3/0.6 (analytische Truth); (B) zirkuläres smin
(`kernel::smin_poly`-Formel, r=0.3) als Union-Blend, Truth = Distanz zu einer
dichten Zero-Kontur-Punktwolke (4097²-Subgrid-Wurzeln, Cloud-Abstand ~1e-3).
Gradienten per zentraler Differenz (4 Evals) — auf dem echten Tape stünde
Forward-Mode (`sdf_dual.rs`), also ~¼ der gezählten Evals.

| Fall | Budget | Fehler roh (max) | Hopf–Lax max | Hopf–Lax mean | Evals/Punkt (mean/max) |
|---|---|---|---|---|---|
| lens | cheap (2 Starts, 12 Iter, tol 1e-3) | 0.0631 | 0.00269 | 4.2e-4 | 436 / 733 |
| lens | thorough (6 Starts, 30 Iter, tol 1e-4) | 0.0631 | 8.7e-5 | 3.4e-5 | 3557 / 5286 |
| smin | cheap | 0.0555 | 0.00165 | 2.7e-4 | 342 / 733 |
| smin | thorough | 0.0555 | 5.9e-4 | 9.6e-5 | 2678 / 6343 |

**Befund:** Punktweise Redistanzierung auf dem Tape funktioniert und ist
budget-skalierbar: Faktor ~25 Fehlerreduktion für ~430 Feld-Evals (mit
Dual-Gradienten realistisch ~100–150 Tape-Evals), Faktor ~700 für ~3500.
Sie repariert auch das smin-Feld (√2-Problem) ohne jede Strukturkenntnis.
Für Per-Pixel-Rendering zu teuer (Sphere-Tracing selbst braucht 30–100
Evals), aber genau richtig für: analytische Abfragen (Wandstärke, Messen),
Offset-/Shell-*Definition* an Meshing-Samplepunkten, Kalibrierung/Verifikation.

**Theorie-Bonus (Morphologie-Sicht, fürs Protokoll):** Dilation/Erosion als
Ball-Extremum g(x) = min_{‖y−x‖≤δ} f(y) hat ein **exaktes** Zero-Set
(Minkowski-Summe des Solids mit dem δ-Ball) unabhängig davon, ob f
Pseudo-SDF ist — es hängt nur vom Vorzeichen von f ab. Ein Offset-Feature,
das statt „d − δ" eine (approximierte) Ball-Minimierung auswertet, wäre
also geometrisch korrekt selbst über kaputten Distanzen. Kostenfrage offen
(→ Harness-Experiment).

### Experiment E3 — 3D-Kosten auf voxel.rs-Skala (`exp3_fsm3d.rs`, rustc -O)

Union zweier Kugeln, 200³ = 8·10⁶ Zellen (exakt die `VOXEL_CAP`-Skala von
`api/voxel.rs`), f32, sequentielles Gauss-Seidel-FSM (8 Orderings),
Single-Thread auf dieser Maschine:

- Feld-Sampling 200³: **0.03 s** (analytische Kugeln; das echte Tape ist
  teurer — `voxelizeOccupancy` existiert ja schon und ist der Kostenanker)
- Band-Init: **0.02 s**; Sweeps: **2.9 s für 8 Sweep-Sets**, aber
  **konvergiert praktisch nach 1 Set** (~0.36 s; maxch ab Set 2 nur noch
  ~2e-4 ≈ 0.01·h f32-Rauschen)
- Speicher: 2 × 32 MB (Feld + Distanz) + Band-Bits
- Fehler (Band |d|≤0.5): roh innen 0.1334 → FSM innen **0.0101 max /
  0.0022 mean**; roh außen 0 → FSM außen 0.0056 (= 0.23·h, wieder der
  Seitenselektivitäts-Punkt aus Iteration 1)

**Kostenfazit (Frage 3):** On-demand-Redistancing pro Body nach Feature-Regen
ist auf CPU-Niveau **~0.5–1 s single-thread bei 200³** (1–2 Sweep-Sets), also
mit Parallelisierung (Detrixhe-Ordering, 8 Kerne) bzw. GPU-FIM klar
Interaktiv-tauglich (<100 ms Größenordnung laut Literatur, Quelle 5/6).
Der dominante echte Kostenfaktor wäre das Tape-Sampling der 8M Zellen, nicht
der Eikonal-Löser — und genau dieses Sampling macht `voxelizeOccupancy`
heute schon in einem WASM-Call.

### Schlussfolgerungen Iteration 2

1. **H-HL belegt** (im 2D-Modell): punktweise, tape-native Redistanzierung
   mit kontrollierbarem Budget; die einzige gitterfreie Methode im gesamten
   gesichteten Methodenraum, die in unsere analytische Pipeline passt.
2. Gitter-Redistancing ist billig genug für „einmal pro Body nach Regen",
   liefert aber nur O(h)-Treue (bei 200³ auf 100 mm Teil: h ≈ 0.5 mm,
   Bandfehler ~0.1–0.5 h ≈ 0.05–0.25 mm) und rundet beim trilinearen
   RESAMPLING Feld-Kinks mit O(h) — als Ersatz-Renderfeld ungeeignet, als
   Abfrage-/Korrekturschicht (Meshing-Prioren, Wandstärke, Offset-Distanz
   im Fernfeld) geeignet.
3. Die Arbeitsteilung zeichnet sich ab: **exakte Seite vom Tape (Satz aus
   It. 1) + Nahband punktweise (Hopf–Lax/Closest-Point à la Saye) +
   Fernfeld/Innenraum vom redistanzierten Gitter.**

---

## Bewertete Hypothesen (Stand Ende Iteration 2)

- **A1 (belegt, Satz + Messung):** Harte Booleans sind seitenselektiv exakt
  (min: außen, max: innen); Fehler der kaputten Seite ist h-unabhängig und
  erreicht 13–16 % der Abfragedistanz (Fan) bzw. volle Nahttiefe (Union
  innen). Korrekturen müssen seitenselektiv sein, sonst zerstören sie
  Exaktheit (E1/E3-Negativbefund).
- **A2 (belegt):** Eikonal-Redistancing mit gefrorenem Band erhält Zero-Set
  (O(h²)) und konkave Kanten (keine Rundung; 1.2e-6 bei N=513) und
  rekonstruiert Fans; Ordnung O(h) glatt, ~O(h^0.6)–O(h log h) am Fan.
- **A3 (belegt):** On-demand-3D-Redistancing auf 200³ kostet ~0.4–1 s
  single-thread (Löser), Feld-Sampling dominiert auf echtem Tape;
  GPU/parallel < 100 ms realistisch (Literatur). Sweet Spot: einmal pro
  Body nach Feature-Regen, NICHT im Render-Pfad.
- **A4 (belegt im Modell):** Hopf–Lax punktweise auf dem Tape korrigiert
  Distanzen gitterfrei um Faktor 25–700 bei 100–3500 Tape-Evals/Punkt —
  für Abfragen/Offset-Samples, nicht per Pixel.
- **A5 (plausibel, ungetestet):** Offset/Shell als Ball-Extremum
  (morphologische Dilation/Erosion) hat beweisbar exaktes Zero-Set auch auf
  Pseudo-SDFs; Kosten/Approximierbarkeit im WGSL-Interpreter offen →
  nächstes Harness-Experiment.

## Vorschlag fürs gemeinsame Harness (metricity-lab)

**E-next:** Realer CAD-Fall auf dem echten Kernel-Tape: Platte − Zylinder
(einspringende Kante), optional + Kanten-Fillet (MaskedBlend). (1) Outward-
Offset δ ∈ {0.5, 2, 5} mm: Oberflächenlage roh vs. Ball-Min-Dilation (A5)
vs. 200³-FSM-Gitter (A3), Hausdorff gegen Referenz-BRep; (2) Wandstärke-
Abfrage innen roh vs. Hopf–Lax (A4); (3) Kosten in Tape-Evals und ms auf
dem WGSL-Interpreter. Damit sind alle vier Pflicht-Metriken des README
abgedeckt und die Hypothesen A1–A5 hängen an einem produktionsnahen Body.

