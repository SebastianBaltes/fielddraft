# Agent A — Iteration 2, Experiment 3: 3D-Offset/Shell-Showdown auf echten Kernel-Feldern

Stand 2026-07-18. Auftrag aus `SYNTHESIS.md` §4 (Experiment 3): Platte−Zylinder
als kompiliertes Kernel-Feld, Outward-Offset δ ∈ {0.5, 2, 5} mm, Ansätze
roh / Ball-Extremum (A5) / Union-Pushdown (F) / 200³-FSM (A3), plus
Wandstärke-innen roh vs. Hopf–Lax (A4). Alle Rohdaten als CSV in diesem
Verzeichnis, Code als `offset_stack.patch`
(`crates/metricity-lab/src/bin/offset_stack.rs`, Lauf:
`cargo run -p metricity-lab --release --bin offset_stack -- <outdir>`).

## Quellen

- Iteration-1-Vorarbeit dieses Agenten (`../LOG.md`): Seitenselektivitäts-Satz
  (min exakt außen / max exakt innen), Hopf–Lax-2D (`exp2_hopflax.py`,
  Royston/Lee–Darbon–Osher), FSM-3D-Kostenanker (`exp3_fsm3d.rs`, Zhao 2005),
  Ball-Extremum-Hypothese A5 (Dilation/Erosion als Minkowski-Morphologie hat
  exaktes Zero-Set auf Pseudo-SDFs).
- Agent D, Bonus-Befund (`../../agent-d-numerics/LOG.md`): Dilation kommutiert
  mit Union — `min(dᵢ)−δ = min(dᵢ−δ)`; dual Erosion mit Intersection.
- Harness `crates/metricity-lab` (`../../HARNESS.md`): Field-Trait,
  GroundTruth (Punktwolke + Kompass-Raffinierung), Doppel-GT-Muster.

## Hypothesen

- **H1 (aus A1/F):** Auf der max-Wurzel (Differenz) ist der Outward-Offset
  roh strukturell falsch (Fan hinter der einspringenden Bohrungskante,
  13–16 % der Abfragedistanz); auf der min-Wurzel (Union) ist er exakt.
  Dual: inward exakt auf Differenz, kaputt auf Union.
- **H2 (A5):** Ball-Extremum-Dilation `g(x) = min_{‖y−x‖≤δ} f(y)` liefert die
  exakte Offsetfläche auch über Pseudo-SDFs; Diskretisierung über
  Richtungs-Samples + Verfeinerung; Frage: Evals/Query für < 1 % Fehler.
- **H3 (A2/A3):** FSM-Redistanzierung rekonstruiert den Fan, degradiert aber
  die beweisbar exakten Seiten auf O(h) (Seitenselektivitäts-Warnung).
- **H4 (A4):** Innentiefen unter Union-Nähten (Phantomflächen) sind roh
  massiv unterschätzt; punktweises Hopf–Lax auf dem echten Feld repariert das
  budget-skalierbar.

## Aufbau

Szenen (rohe `CsgNode`-Bäume gegen die Kernel-API, `EvalMode::Visual`):

- `plate_bore` = Difference(Box 80×20×50, Zylinder r=8, hh=14 bei x=28) —
  Durchgangsbohrung, dünnste Wand 4 mm (Bohrungswand ↔ x-Fläche),
  Werkzeug-Überstand über die Platte: 4 mm je Seite.
- `plate_boss` = Union(gleiche Platte, Zylinder r=12, y ∈ [9, 41]) —
  Boss auf dem Deckel, 1 mm eingesenkt (verdeckte Phantom-Deckelfläche).
- `plate_bore_fillet` = `plate_bore` mit MaskedBlend-Fillet am Bohrungsmund
  (r_blend=3, Masken-Tube r=6 um den Mundkreis, `is_difference`).

Ground Truth: für bore/boss **analytisch** (exakte f64-Distanz zur realen
Oberfläche: Boxflächen ∖ Mundscheiben, Bohrungs-/Boss-Mantel, Deckscheibe),
validiert gegen die Harness-GT (`gt_validation.csv`): bore max 1.9e-3 mm;
boss max 0.143 mm — der Ausreißer liegt am *Harness* (Kompass-Raffinierung
strandet am Graztangenten-Strahl zur konkaven Junction-Kreislinie; von Hand
nachgerechnet: Analytik 10.6276 mm zur Junction ist korrekt, Harness meldet
10.7702). Für die Fillet-Szene Harness-GT (dort keine Analytik; Unsicherheit
an konkaven Zonen entsprechend ~0.1 mm). Tape-Parität: max |visual − tape| =
0.0 auf `plate_bore` (harte Booleans; MaskedBlend-Tape divergiert bekanntlich
per #382-Fade, dort wurde nur das Visual-Feld gemessen).

Messgröße Isofläche: Punkte auf {g = 0} via Achsenlinien-March + Bisektion
(m = 40, ~4–9 k Punkte), Fehler e = |d_wahr(p)| − δ (signiert; + = Fläche zu
weit draußen = Phantom-Material). Kosten: Basisfeld-Evals/Query
(AtomicU64-Zähler) + µs/Query an den Isoflächenpunkten (CPU, single-point).

## Messwerte

### Haupttabelle Outward-Offset (`offset_iso.csv`)

| Szene | Ansatz | δ | mean | p95 | max | bias | Evals/Q | µs/Q |
|---|---|---|---|---|---|---|---|---|
| plate_bore | roh | 0.5 | 0.0002 | ~0 | **0.191** | +0.0002 | 1 | 0.02 |
| plate_bore | ball32×2 | 0.5 | 0.00004 | 0.00005 | 0.0095 | −0.00004 | 103 | 2.5 |
| plate_bore | fsm200 | 0.5 | 0.0099 | 0.017 | 0.220 | −0.008 | 0 (Build) | 0.04 |
| plate_bore | roh | 2 | 0.009 | ~0 | **0.775** | +0.009 | 1 | 0.02 |
| plate_bore | ball32×2 | 2 | 0.0014 | 0.009 | 0.091 | −0.0014 | 107 | 2.7 |
| plate_bore | fsm200 | 2 | 0.043 | 0.262 | 0.495 | −0.038 | 0 | 0.04 |
| plate_bore | roh | 5 | 0.013 | ~0 | **3.684** | +0.013 | 1 | 0.02 |
| plate_bore | ball32×2 | 5 | 0.0064 | 0.034 | 0.408 | −0.0064 | 110 | 2.7 |
| plate_bore | fsm200 | 5 | 0.105 | 0.406 | 0.708 | −0.096 | 0 | 0.04 |
| fillet | roh | 0.5 | 0.0002 | ~0 | 0.141 | +0.0002 | 1 | 0.08 |
| fillet | ball32×2 | 0.5 | 0.00002 | 0.00004 | 0.0059 | −0.00002 | 103 | 8.4 |
| fillet | roh | 2 | 0.0042 | 0.0002 | 0.747 | +0.0042 | 1 | 0.08 |
| fillet | ball32×2 | 2 | 0.0011 | 0.008 | 0.044 | −0.0011 | 109 | 8.8 |
| fillet | roh | 5 | 0.029 | 0.0007 | 4.211 | +0.029 | 1 | 0.08 |
| fillet | ball32×2 | 5 | 0.0068 | 0.036 | 0.429 | −0.0068 | 110 | 8.8 |

(ball32×2 = 32 Fibonacci-Richtungen + Kompass-Verfeinerung der 2 besten
Starts. „~0" = < 1e-5 mm.)

**Roh-Anatomie:** p95 ≈ 0 überall — der Fehler sitzt ausschließlich im
Fan-Keil am Bohrungsmund, dort aber strukturell: max ≈ 0.38·δ bei δ=0.5/2
(Theoriewert der scharfen Ersatz-Kante: (√2−1)·δ = 0.41·δ). Bei δ=5 kippt es
qualitativ: **die Bohrung wird zugedeckelt**. Der Offset `−(d_cyl+δ)`
erodiert den *Subtrahenden*, dessen Kappe nur 4 mm über der Platte liegt —
sobald δ den Werkzeug-Überstand übersteigt, endet das erodierte Loch als
Sackloch (gemessen max 3.68 mm; wahres Supremum am Lochzentrum
√(8²+5²)−5 = 4.43 mm). Der Roh-Fehler ist damit nicht nur 13–16 %, sondern
**repräsentationsabhängig unbeschränkt** (hängt vom Modellier-Detail
„wie weit überstand das Bohrwerkzeug" ab — dieselbe Klasse wie Agent Cs
Phantom-Befund) und **topologisch falsch**. Vorzeichen immer +
(Phantom-Material), nie konservativ.

**Fillet-Variante:** identisches Bild (roh 0.14/0.75/4.21) — die smin-Zone
ändert die Fehlerklasse nicht, und Ball-Dilation funktioniert auf dem
MaskedBlend-Pseudo-SDF unverändert (H2 auf Blends bestätigt). Nur die
Query-Kosten steigen mit dem teureren Basisfeld (8.8 statt 2.7 µs).

### Ball-Budget-Sweep, δ = 2 (`ball_sweep.csv`)

| dirs | starts | mean | p95 | max | max % δ | Evals/Q |
|---|---|---|---|---|---|---|
| 8 | 0 | 0.319 | 0.838 | 1.839 | 92 % | 9 |
| 8 | 1 | 0.0026 | 0.015 | 0.413 | 21 % | 45 |
| 16 | 1 | 0.0029 | 0.018 | 0.176 | 8.8 % | 57 |
| 32 | 1 | 0.0026 | 0.021 | 0.176 | 8.8 % | 67 |
| 16 | 4 | 0.0009 | 0.005 | 0.038 | **1.9 %** | 172 |
| 64 | 4 | 0.0007 | 0.005 | 0.053 | 2.7 % | 211 |
| 128 | 4 | 0.0008 | 0.004 | 0.037 | **1.8 %** | 274 |

- **mean/p95 < 1 % von δ ab ~45 Evals/Query** (8 Richtungen + 1
  Kompass-Start); reines Richtungs-Sampling ohne Verfeinerung ist wertlos
  (92 % max bei 8 dirs), die Verfeinerung trägt fast alles.
- **max < 1 % wurde NICHT erreicht** (Negativbefund): Plateau 1.8–3 % ab
  ~170 Evals. Restfehler sitzt an der Torus-Zone des Offset-Munds, wo die
  Richtungs-Landschaft nahe Gleichstand ist und der Kompass im falschen
  Becken strandet. Multi-Start (starts=4) ist der Hebel, mehr Rohrichtungen
  kaum (64/128 dirs ≈ 16 dirs bei gleichem starts).
- **Einseitig konservativ:** Sampling-Min ≥ wahres Min ⇒ die diskretisierte
  Offsetfläche liegt nie AUSSERHALB der wahren (bias durchweg ≤ 0,
  Unter-Dilation). Kein Phantom-Material, im Gegensatz zu roh.

### Union-Pushdown-Klassifikation (Ansatz c; in `offset_iso.csv`)

Algebraisch ist `min(dᵢ)−δ = min(dᵢ−δ)` (und ebenso für max) eine Identität —
der Gehalt des Rewrites ist mengentheoretisch: (A∪B)⊕B_δ = (A⊕B_δ)∪(B⊕B_δ)
gilt, (A∩B)⊕B_δ ⊊ (A⊕B_δ)∩(B⊕B_δ) nicht. Gemessen als 2×2 (roh, exakte
Leaves):

| Wurzel-Op | outward δ=2 | inward t=1.5 |
|---|---|---|
| Union (`plate_boss`) | **exakt** (max 2e-6 mm) | **kaputt** (max 9.39 mm, Phantom-Deckel) |
| Differenz (`plate_bore`) | **kaputt** (s. Haupttabelle) | **exakt** (max 2e-6 mm) |

Klassifikationsregel fürs Offset-Feature (Compile-Zeit am Tape/CSG-Baum):
Outward-Offset ist exakt, wenn auf dem Weg zur Wurzel nur Union-Knoten
liegen (Blätter exakte SDFs); Inward-Offset dual für
Intersection/Differenz. Gemischte Bäume: exakt genau auf der jeweils
„exakten Seite" jedes Knotens — die Entscheidung ist pro Feature statisch
ableitbar, Kosten 0. Der kaputte Union-inward-Fall ist per Ball-*Erosion*
reparierbar: max 9.39 → **0.041 mm** (104 Evals/Q).

### FSM 200³ auf dem echten Feld (`fsm_build.csv`, `fsm_side_selectivity.csv`)

Build (Domain AABB+8 mm, längste Achse 200 Knoten, h = 0.482 mm →
200×92×138 = 2.54 M Zellen): Sampling des Kernel-Felds 0.10 s (2.54 M
Evals, rayon), Band-Init 0.01 s, Sweeps 0.20 s (2 Sets konvergiert),
19 MB. Der Iteration-1-Kostenanker hält auch mit echtem Tape (das Feld ist
hier flach; tiefe Bäume verschieben die Kosten weiter Richtung Sampling).

Isoflächen (Haupttabelle): FSM ist bei JEDEM δ schlechter als Ball —
p95 0.26 mm bei δ=2 (13 % von δ), max 0.49–0.71 mm ≈ 1–1.5·h. Der
O(h)-Boden aus Godunov-Diskretisierung + trilinearem Resampling dominiert.

Feldfehler roh vs. FSM (12 k Uniform-Punkte, gegen analytische GT):

| Band | Seite | roh mean / max | FSM mean / max |
|---|---|---|---|
| nah (<2 mm) | außen | 0.0018 / 0.484 | 0.016 / 0.424 |
| nah | innen | **0.000000 / 0.000002** | 0.0055 / 0.200 |
| mittel (2–10) | außen | 0.021 / **5.871** | 0.105 / 0.848 |
| mittel | innen | **0.000000** | 0.013 / 0.229 |
| fern (>10) | außen | 0.037 / 3.285 | 0.219 / 0.906 |

**Seitenselektivitäts-Warnung bestätigt (H3):** FSM deckelt die
Fan-Katastrophe (5.9 → 0.85 mm), verschlechtert aber die beweisbar exakte
Innenseite von 0 auf bis zu 0.23 mm (0.47·h) und die exakten
Außen-Regionen auf mean 0.10–0.22 mm (Bias +, systematische
First-Order-Überschätzung). Ein FSM-Feld darf die exakten Zonen des Tapes
nie ersetzen, nur die als kaputt klassifizierten.

### Wandstärke innen: roh vs. Hopf–Lax (`inner_depth.csv`)

55 Punkte in der Platte unter dem Boss-Fuß (wahre Tiefen ~3–12 mm) und 45
Punkte in der 4-mm-Wand der Bohrung:

| Fall | Methode | mean | p95 | max | bias | Evals/Q | µs/Q |
|---|---|---|---|---|---|---|---|
| unter Boss | roh (\|f\|) | 4.363 | 8.514 | 8.514 | −4.363 | 1 | — |
| unter Boss | HL 32 dirs | 0.095 | 0.515 | 0.957 | +0.095 | 2283 | 54 |
| unter Boss | HL 64 dirs | 0.049 | 0.221 | 0.381 | +0.049 | 2128 | 51 |
| unter Boss | HL 128 dirs | 0.031 | 0.089 | 0.234 | +0.031 | 3118 | 76 |
| 4-mm-Wand (Diff) | roh | **0.000000** | 0 | 0 | −0 | 1 | — |
| 4-mm-Wand | HL 32 | 0.000000 | 0 | 0 | −0 | 105 | 2.6 |

- H4 bestätigt: roh misst unter dem Boss zur Phantom-Deckelfläche (bis
  −8.5 mm bei ~12 mm wahrer Tiefe = 71 % Unterschätzung); Hopf–Lax auf dem
  echten Feld repariert auf 0.03–0.10 mm mean. 64 Richtungen sind der Sweet
  Spot: GENAUER UND BILLIGER als 32 (bessere Ball-Maxima → weniger äußere
  Iterationen: 2128 vs 2283 Evals).
- **Vorzeichen-Warnung:** der HL-Bias ist positiv — das gesampelte
  Ball-Maximum unterschätzt das wahre Maximum, die Iteration überschießt
  die Tiefe. Für Wandstärke-*Warnungen* ist das die unsichere Seite;
  Richtungsbudget hoch genug wählen oder Schwellen um den gemessenen Bias
  (~1–3 % bei 64–128 dirs) konservativ setzen.
- An der dünnen Differenz-Wand ist roh exakt (Satz aus It. 1, jetzt am
  Kernel-Feld bestätigt) und HL konvergiert in 1 Iteration (105 Evals) auf
  denselben Wert — die Klassifikationsregel sagt vorher, wo HL überhaupt
  gebraucht wird.

### Pflicht-Metriken-Abdeckung

(1) Distanz-/Isoflächenfehler: oben. (2) |∇f|: nicht separat vermessen —
alle Felder hier sind stückweise exakte SDFs mit |∇f|=1 f.ü.
(Fußpunktfehler-Klasse, für Gradientenmetriken unsichtbar, vgl. Agent D);
Ball-Dilation ist als inf-Faltung 1-Lipschitz und einseitig konservativ —
sphere-trace-sicher. (3) Tiefen-Sweep: n.a. (eine Boolean-Ebene); die
Klassifikationsregel komponiert aber induktiv über den Satz aus It. 1.
(4) Kosten: Evals/Query + µs/Query in jeder Tabelle; FSM-Build separat.

## Schlussfolgerungen

1. **Roh-Offset (`f − δ`) ist auf der kaputten Seite nicht nur ungenau,
   sondern strukturell und topologisch falsch** — Fan-Fehler 0.38–0.41·δ an
   der einspringenden Kante, und sobald δ den Subtrahenden-Überstand
   übersteigt, werden Bohrungen zugedeckelt (repräsentationsabhängig
   unbeschränkt). Fehler immer auf der Phantom-Material-Seite.
2. **Ball-Extremum (A5) ist der Showdown-Sieger auf der kaputten Seite:**
   mean/p95 < 1 % von δ ab ~45–110 Feld-Evals/Query, einseitig konservativ,
   funktioniert unverändert auf MaskedBlend-Pseudo-SDFs und repariert dual
   (Erosion) auch Union-inward (9.39 → 0.041 mm). Negativbefund: max-Fehler
   plateaut bei 1.8–3 % von δ (~170–270 Evals, Torus-Zonen) — max < 1 %
   braucht einen besseren inneren Löser (z. B. Dual-Gradienten statt
   Kompass), nicht mehr Samples.
3. **Union-Pushdown/Klassifikation (F) ist gratis und scharf:** outward über
   Union und inward über Differenz/Intersection sind schon roh exakt
   (max 2e-6 mm gemessen); die Exaktheit ist zur Compile-Zeit am Baum
   entscheidbar. Das Offset-Feature sollte zuerst klassifizieren und nur im
   kaputten Fall zahlen.
4. **FSM ist für Offset-Isoflächen die falsche Waffe** (überall schlechter
   als Ball, O(h)-Boden 0.2–0.9 mm bei 0.48 mm Zellen, degradiert exakte
   Seiten — H3/Seitenselektivität jetzt in 3D am echten Feld belegt). Seine
   Nische: flächige Innenraum-Abfragen. Break-even gegen Hopf–Lax liegt bei
   ~1200 Abfragepunkten (Build 2.5 M Evals ≈ 1200 × 2100); Wandstärke-
   Heatmaps gewinnen mit FSM, Einzelmessungen mit HL.
5. **Hopf–Lax auf dem echten Kernel-Feld funktioniert** (erste 3D-Messung):
   71 % Tiefen-Unterschätzung → 0.03–0.10 mm für ~2100 Evals (~51 µs CPU);
   64 Richtungen dominieren 32. Achtung positiver Bias (Überschätzung) durch
   Sampling — bei Warnschwellen konservativ auslegen.

## Empfehlung für den Kernel (je Anwendungsfall)

- **Offset/Shell-Feature (Geometrie-Definition, Mesher-Pfad):** dreistufig.
  (i) Compile-Zeit-Klassifikation am `BoundedCsg`-Baum: exakte Fälle
  (outward∘Union, inward∘Diff/Intersection, exakte Leaves) → wörtlich
  `f − δ`, Kosten 0. (ii) Kaputte Fälle → Ball-Extremum-Feld mit
  ~16 Richtungen + 4 Kompass-Starts (~170 Evals/Query, p95 ≪ 1 %,
  konservativ); das ist ein reiner Feld-Operator und damit WGSL-portierbar,
  gehört aber wegen ~100× Eval-Kosten in Mesher/Abfrage, nicht ins
  Per-Pixel-Rendering. (iii) Für die Isoflächen-Politur der verbleibenden
  1–3 %-max-Zonen ggf. Agent Gs AD-Sampson bzw. ein Gradienten-basierter
  innerer Löser (offene Frage an Iteration 3).
- **Wandstärke/Innentiefen-Abfragen:** Einzelpunkte → Hopf–Lax 64 dirs
  (~2100 Evals, 51 µs); Heatmaps/Volumen → seitenselektives FSM on-demand
  (0.3 s / 19 MB auf dieser Szene), niemals als Ersatz-Renderfeld und nie
  auf den exakt klassifizierten Seiten.
- **Nicht tun:** `f − δ` auf Differenz-/Intersection-Außenseiten anbieten
  (deckelt Bohrungen); volles FSM als generisches „Reparatur-Feld";
  Richtungs-Sampling ohne lokale Verfeinerung (92 % Fehler).

## Reproduktion

    cargo run -p metricity-lab --release --bin offset_stack -- \
        docs/research/sdf-metricity/agent-a-pde/iter2

Laufzeit ~50 s (Release, 8 Threads via rayon für GT/Marching; FSM-Sweeps
single-thread). Deterministisch (SplitMix64-Seeds im Code).
