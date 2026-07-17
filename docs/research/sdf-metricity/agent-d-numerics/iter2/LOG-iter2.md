# Agent D — Iteration 2, Experiment 4: Tiefenbewusstes march_relax + Zweikanal-AD am echten Tape (3D)

Datum 2026-07-18. Auftrag aus `SYNTHESIS.md` §4 Nr. 4: die 2D-Numerik-Befunde
aus Iteration 1 (Quadratur-Wachstum √m, Schranke 1/√(1+m), AD-Winner-Branch,
vertrauensbandiertes Wurzel-Sampson) in 3D am **echten Bytecode-Tape**
(`eval_opcodes`) bestätigen oder widerlegen. Alles gegen die
metricity-lab-Ground-Truth (GT jeweils auf der Tape-EIGENEN Nullmenge —
kein Vorzeichen-Mismatch, Penetration ist exakt definiert).

## Quellen / Vorarbeit

- `agent-d-numerics/LOG.md` Iteration 1–3 (2D): √m-Gesetz (P1), FD-Tie-Artefakt
  E2-FD (f/|g_fd| bis 1.4·d_wahr), relax-0.75-Gegenbeispiel (m=4-Stack −0.10),
  Wurzel-Sampson-Offsets (−15 % → −2 % bei kleinem δ).
- `HARNESS.md` / `crates/metricity-lab` (GT validiert ≤ 2.4e-4 rel.).
- Kernel: `sdf_compiler.rs` (`march_relax`, `eval_opcodes`, #382-Render-Fade),
  `sdf_dual.rs` (Baum-Level-Dual-Eval — existiert, aber NICHT tape-fähig;
  der Tape-Prototyp hier ist die fehlende Ebene).

## Code (alles im Patch `relax_ad.patch`, git diff des Experiment-Worktrees)

- `crates/kernel/src/sdf_compiler.rs`: **Compile-Zeit-Analyse**
  `max_smin_nesting` (max. Zahl geschachtelter SmoothUnion/SmoothDifference
  je Tape-Pfad), `lipschitz_sq_bound` (rekursive L²-Schranke: smin = Quadratur
  `L_a²+L_b²`, min/max/Morph = Winner-max, MaskedBlend = neutral wegen
  #382-÷√2-Fade), `march_relax_depth_aware` = min(bisheriges `march_relax`,
  `1/√L²`). Reine Baumtraversierung, 0 Laufzeitkosten.
- `crates/kernel/src/sdf_tape_ad.rs` (neu): **Zweikanal-Forward-AD durch das
  Tape** — `eval_opcodes_dual`: Register (val, grad), Winner-Branch für
  min/max/Difference/Shell, Arc-Gewichte für smin (Fillet+Chamfer),
  volle MaskedBlend-Kette (inkl. Fade-Term (blended−hard)·∇w), Warps
  Transform/Twist/Bend über mitgeführte 3×3-Jacobi. Prototyp-Scope: 3D-Kern;
  2D-Profile/Helix/Sweep/Gyroid/Tex3D → `None`. CPU-Referenz, kein WGSL.
- `crates/metricity-lab/src/bin/iter2_relax_ad.rs`: Messlauf (dieser Report).

Reproduktion: `cargo run -p metricity-lab --release --bin iter2_relax_ad -- <outdir>`.
CSVs hier im Verzeichnis: `trace_adversarial.csv`, `trace_benign.csv`,
`sanity_ad.csv`, `fd_vs_ad_ties.csv`, `sampson_offsets.csv`, `ad_cost.csv`.

## Aufbau

- **Szenen adversarial:** koinzidente smin-Stacks `stack_coin_m{2,4,8}`
  (m SmoothUnion-Ops = m+1 Kugeln r=30 am selben Ort, r_blend=6; L-Vorhersage
  √(1+m)), fast-koinzident `stack_near_*` (Zentren ≤ 0.3 mm gejittert),
  `mb_stack_m04` (4 gestapelte MaskedBlends am selben Ort — Test der
  ÷√2-Neutralitäts-Vorhersage), `offset_smin` (tief überlappende SmoothUnion,
  3D-Gegenstück zur 2D-cx=0.6-Szene), plus Korpus `masked_blend_edge`
  (echtes Kanten-Fillet) und `smin_chain_04`.
- **Trace-Simulation:** 400 Rays/Szene/Politik von außen (0.35·Modell) auf
  GT-Oberflächenpunkte (2 % tangentialer Jitter), Schritt
  `t += max(f·relax, ε)`, Hit bei `f < 2e-4·Modell`; Penetration = GT-signierte
  Distanz des Hit-Punkts < −1e-3·Modell. Politiken: 1.0 (heutiges Tape für
  SmoothUnion-Bäume!), flach 0.75 (alte MASKED_BLEND-Politik), depth-aware
  `1/√(1+m)`. „misses" sind über alle Politiken identisch (Aiming-Artefakte
  streifender Starts, kein Tunneln — Politik-invariant).
- **AD-Validierung:** Wert-Parität + Gradient-vs-FD in glatten Zonen
  (`sanity_ad.csv`); Tie-Flächen-Ratio `(f/|g|)/d_wahr` auf Naht-Bändern;
  Offset-Isoflächen {F_var = δ} entlang AD-Normalen bisektiert, wahre Distanz
  des Treffpunkts via GT; Kosten-Benchmark 300 k Evals/Szene.

## MESSWERTE

### A) Penetration je relax-Politik (adversariale Stacks, 400 Rays)

| Szene | m | L² | relax 1.0: Pen-Rate (Tiefe mean/max mm) | 0.75 | depth-aware (Wert) |
|---|---|---|---|---|---|
| stack_coin_m02 | 2 | 3 | 39.1 % (0.85/2.53) | 19.6 % (0.65/0.99) | **0 %** (0.577) |
| stack_near_m02 | 2 | 3 | 37.2 % (0.61/2.43) | 13.8 % (0.51/0.99) | **0 %** (0.577) |
| stack_coin_m04 | 4 | 5 | 54.8 % (0.78/3.23) | 24.3 % (0.89/1.77) | **0 %** (0.447) |
| stack_near_m04 | 4 | 5 | 51.6 % (0.68/3.15) | 21.2 % (0.74/1.71) | **0 %** (0.447) |
| stack_coin_m08 | 8 | 9 | 64.4 % (0.93/3.99) | 42.4 % (0.75/2.44) | **0 %** (0.333) |
| stack_near_m08 | 8 | 9 | 65.7 % (1.04/3.73) | 46.2 % (0.82/2.19) | **0 %** (0.333) |
| mb_stack_m04 | 0* | 1 | **0 %** | 0 % | 0 % (1.0) |
| offset_smin | 1 | 2 | 0.6 % (0.17/0.17) | 0 % | 0 % (0.707) |
| smin_chain_04 | 3 | 4 | 0.9 % (0.32/0.57) | 0 % | 0 % (0.5) |
| masked_blend_edge | 0* | 1 | **0 %** | 0 % | 0 % (1.0) |

\* MaskedBlend zählt nicht: der #382-÷√2-Fade macht das Tape-Feld nachweislich
≤1-Lipschitz — **die Neutralitäts-Vorhersage ist bestätigt** (4 koinzident
gestapelte MaskedBlends, 0 Penetration bei vollen Schritten; Kosten des
kumulierten (1/√2)⁴-Shrinks: 16.2 statt 12.0 Schritte der nackten Kugel, +35 %).

**Kernbefunde:**
1. **Die 2D-Prognose ist in 3D bestätigt und VERSCHÄRFT**: flach 0.75
   penetriert den m=4-Stack in 20–24 % der Rays bis 1.77 mm; bei m=8 in
   42–46 % bis 2.44 mm. Fast-koinzident (0.3 mm Jitter) ist identisch zum
   koinzidenten Fall — der Adversar braucht keine exakte Koinzidenz.
2. **Der heutige Kernel ist auf reinen SmoothUnion-Bäumen undicht**:
   `march_relax` dämpft SmoothUnion gar nicht (= Politik 1.0) → schon die
   realistische `smin_chain`-Familie penetriert heute 0.6–1.8 % der Rays um
   0.1–0.57 mm (chain_02/04/08/16). Der MaskedBlend-Pfad ist dank #382 sicher;
   der generische SmoothUnion-Opcode hat diese Normalisierung nicht.
3. **1/√(1+m) ist dicht**: 0 Penetration über alle 10 Szenen × 400 Rays.

### B) Schrittzahl-Regression (gutartiger Korpus, Mittel über Hits)

| Szene | 1.0 | 0.75 | depth-aware |
|---|---|---|---|
| sphere / box / rot_box / thin_wall_2% / tangent_m25 / deep_csg_08 | 9.9–12.0 | 14.7–17.5 (+47 %) | **= 1.0-Spalte** (relax 1.0) |
| smin_chain_02 (m=1) | 11.8 (3 Pen!) | 17.2 | 18.5 (+8 % vs 0.75) |
| smin_chain_08 (m=7) | 8.2 (6 Pen!) | 12.7 | 31.0 (**2.4×** vs 0.75) |
| smin_chain_16 (m=15) | 7.4 (2 Pen!) | 11.8 | 41.5 (**3.5×** vs 0.75) |
| stack_coin_m08 | 3.7 (undicht) | 6.1 (undicht) | 18.6 (einzige dichte) |

**Ehrlicher Negativbefund:** auf smin-FREIEN Szenen ist depth-aware gratis
(= volle Schritte; flach 0.75 verschwendet dort konstant ~47 % Schritte). Auf
**räumlich getrennten** langen Ketten ist die reine Pfadzählung aber stark
überkonservativ: chain_16 bekommt relax 0.25 (Schranke √16), obwohl das reale
L ≈ 1.24 (Iteration-1 P3) — 3.5× mehr Schritte als 0.75, das dort empirisch
dicht ist. Die Schranke ist scharf nur im koinzidenten Fall. Verfeinerung
(nicht implementiert): m nur über smins zählen, deren Blendzonen sich räumlich
überlappen können (AABB-Test der Blend-Regionen zur Compile-Zeit) — trennt
Kette (m_eff=1) von Stack (m_eff=m).

### C) Zweikanal-AD: Sanity, Tie-Flächen, Kosten

Sanity (`sanity_ad.csv`, 3000 Punkte/Szene): Wert-Parität dual vs
`eval_opcodes` max 1.9e-6 mm; |g_AD − g_FD| median 0.0000, p95 0.0001 in
glatten Zonen — der Dual-Pfad rechnet dasselbe Feld.

FD-Tie-Flächen-Artefakt in 3D (`fd_vs_ad_ties.csv`, Naht-Bänder außen,
Ratio = (f/|g|)/d_wahr, > 1 = Schrankenverletzung):

| Szene | Gradient | max Ratio | p99 | Anteil > 1.05 |
|---|---|---|---|---|
| tangent_spheres_m25 | FD h=1.5 mm | **1.927** | 1.554 | 2.4 % |
| tangent_spheres_m25 | FD h=0.15 mm | 1.809 | 1.000 | 0.3 % |
| tangent_spheres_m25 | FD h=0.015 mm | 1.638 | 1.000 | 0.0 % |
| tangent_spheres_m25 | **AD** | **1.000** | 1.000 | 0 % |
| thin_wall 2 % | FD (alle h) | 1.413–1.414 | ≤1.331 | ≤3.5 % |
| thin_wall 2 % | **AD** | **1.000** | 1.000 | 0 % |

Das 2D-E2-FD-Ergebnis gilt in 3D, schärfer: kleineres h macht das Band
schmaler, aber das **Maximum bleibt** (1.6–1.9× statt 2D-1.4×, weil 3D-Nähte
spitzere Dieder erreichen). FD-Sampson ist als Trace-Schranke in 3D an jeder
min/max-Naht unsicher; AD (Winner-Branch) überschreitet nie.

### D) Vertrauensbandiertes Wurzel-Sampson auf Offset-Isoflächen

`sampson_offsets.csv`; relerr = (d_wahr(Isoflächenpunkt) − |δ|)/|δ|, min über
250 Normalen-Rays = schlimmste Stelle (Neck/Blendzone). r_blend = 6.

`offset_smin` (tief überlappende SmoothUnion — Normfehler-Szene):

| δ [mm] | roh min | Sampson min | bandiert min |
|---|---|---|---|
| 1 | **−13.7 %** | **−0.6 %** | −1.2 % |
| 2 | −14.0 % | −2.5 % | −2.5 % |
| 3 | −14.3 % | −5.4 % | −5.2 % |
| 6 | −11.3 % | −11.3 % | −11.3 % |
| 12 | −0.1 % | −0.1 % | −0.3 % |
| −2 (Shell-Innenfläche) | **−12.7 %** | **−1.0 %** | −1.0 % |

- 2D-Prognose „−15 % → −2 % bei kleinem δ" **bestätigt** (−13.7 % → −0.6 %),
  auch einwärts (Shell-Wanddicke).
- 2D-Prognose „Sampson wird bei großem δ ohne Bandierung SCHLECHTER"
  **in dieser 3D-Szene nicht reproduziert**: bei δ=12 ist |∇f| dort schon 1,
  unbandiert bleibt harmlos (−0.1 %). Die 2D-Verschlechterung kam aus
  |∇f|<1-Flachstellen weit außen, die diese Geometrie nicht hat. Bandierung
  bleibt billige Versicherung, war hier aber nicht nötig — und kostet bei
  δ=3–6 nichts (identisch zu unbandiert).
- Bei δ=6 (= r_blend) hilft nichts: die Isofläche verlässt die Blendzone,
  der Rest ist Fußpunktfehler.

`stack_coin_m04` (m=4-Stack — beide Fehlerklassen, wie 2D-P1):

| δ | roh | Sampson |
|---|---|---|
| 1, 2, −2 | −55.3 % (= 1 − 1/√5, exakt die L=√5-Zone) | **−0.0 %** |
| 3 | −55.3 % | −10.6 % (Bandzonen-Rand) |
| 6 | −55.3 % | −55.3 % (Fußpunktfehler: konstant 3.32 mm = r_b·(1−1/√5)) |
| 12 | −27.6 % | −27.6 % (derselbe konstante Absolutfehler) |

Exakt das 2D-P1-Bild: Sampson repariert die Norm-Zone vollständig und ist
für den konstanten Fußpunkt-Offsetfehler außerhalb der Blendzone blind.

### E) Kosten am echten Tape (300 k Evals, ns/Eval, single-thread release)

| Szene | Opcodes | 1 Eval | Dual-Eval | 1+6 FD | Dual/Eval | FD/Eval |
|---|---|---|---|---|---|---|
| stack_coin_m04 | 55 | 107 | 106 | 781 | **0.99×** | 7.33× |
| offset_smin | 25 | 39 | 40 | 308 | 1.04× | 7.90× |
| masked_blend_edge | 42 | 56 | 125 | 444 | 2.24× | 7.95× |
| deep_csg_08 | 105 | 96 | 185 | 702 | 1.92× | 7.31× |

Zweikanal-AD kostet am echten Tape **1.0–2.2×** eines Werte-Evals (Winner-
Branch-Propagation ist oft gratis; teuer sind nur MaskedBlend-Fade-Kette und
Transform-Jacobis) und ist damit **3.3–8× billiger als zentrale Differenzen**
— bei exakten statt artefaktbehafteten Gradienten. Die 2D-Schätzung
„2–4× Arithmetik" war zu pessimistisch.

## Schlussfolgerung

1. **√m/1/√(1+m) in 3D am echten Tape bestätigt.** Koinzidente und
   fast-koinzidente smin-Stacks penetrieren unter 0.75 massiv (bis 46 % der
   Rays, bis 2.4 mm); 1/√(1+m) ist über alle Szenen dicht. Neu gegenüber 2D:
   **der heutige Kernel dämpft SmoothUnion überhaupt nicht** (relax 1.0) —
   schon gewöhnliche smin-Ketten penetrieren heute messbar (0.6–1.8 %,
   bis 0.57 mm). Der MaskedBlend-Pfad ist dank #382-÷√2 gegen Stapelung
   robust (Vorhersage `L² ≤ max(L_a², L_b²)` gemessen bestätigt).
2. **Pfadzählung ist die richtige Sicherheits-, aber eine schlechte
   Performance-Politik für getrennte Ketten** (chain_16: 3.5× Schritte von
   0.75). Compile-Zeit-Verfeinerung über räumliche Blendzonen-Überlappung
   wäre der nächste Schritt — oder der Circular-CD-Tausch macht die
   Fillet-Klasse komplett 1-Lipschitz und lässt nur Chamfer/Displace/Exoten
   fürs Netz übrig.
3. **Zweikanal-AD funktioniert durchs komplette Tape** (Parität 2e-6,
   FD-Deckung in glatten Zonen), eliminiert das in 3D noch größere
   FD-Tie-Artefakt (1.93× vs exakt 1.0) und macht vertrauensbandiertes
   Wurzel-Sampson praktisch: Offset-/Shell-Isoflächen in Blendzonen
   −14 % → −1…−2.5 % bei ≤2.2× Eval-Kosten. Gegen Fußpunktfehler bleibt es
   erwartungsgemäß blind — konsistent mit der Zwei-Klassen-Taxonomie.

## Verdicts (Auftragsfragen)

1. **Gehört 1/√(1+m) in den Kernel?** Ja, als *Sicherheitsnetz* — es ist
   strikt besser als jede flache Politik (smin-frei: volle Schritte statt
   pauschal −25 %; koinzident: einzige dichte Politik; Status quo ist auf
   SmoothUnion-Bäumen nachweislich undicht). ABER: kommt der parallel
   getestete Circular-CD-smin-Tausch (Experiment 1), wird der Fillet-Anteil
   der Analyse obsolet (L=1 an der Wurzel schlägt jede Schrittdämpfung);
   dann bleibt `lipschitz_sq_bound` als Netz für Chamfer-Blends, Morph und
   künftige Nicht-1-Lipschitz-Ops sinnvoll, und die Kombination
   `march_relax_depth_aware` ist genau dafür schon die richtige Form
   (min über beide Mechanismen). Vor einem Produktiv-Einbau ohne CD-Tausch
   sollte die Ketten-Überkonservativität (Befund 2) über
   Blendzonen-Ko-Lokalisierung entschärft werden.
2. **Lohnt Zweikanal-AD als Kernel-Infrastruktur?** Ja. 1.0–2.2× Kosten
   (nicht 7–8× wie FD), exakte Winner-Branch-Gradienten ohne Tie-Artefakte,
   unmittelbarer Nutzen für: bandiertes Wurzel-Sampson (Offset/Shell/Mesher-
   Projektion in Blendzonen), λ-Akkumulation (Agent B), Hopf–Lax-Beschleuniger
   (Agent E-Pfad), GT-freie Normalen. Für WGSL heißt das 4 Register pro
   Stack-Slot — der CPU-Prototyp (`sdf_tape_ad.rs`) definiert die Semantik
   pro Opcode; 2D-Profile/Helix/Sweep fehlen noch (sauber als `None`
   abgegrenzt) und sind Fleißarbeit, keine Forschungsfrage.

## Limitationen

- Trace-Simulation misst Politiken am CPU-Spiegel `eval_opcodes`, nicht im
  WGSL-Renderer (identische Semantik per Konstruktion, aber kein GPU-Timing).
- „misses" (~5–20 % je Szene) sind Politik-invariante Aiming-Artefakte
  streifender Rays, kein Tunneln (identische Zählung über alle relax-Werte).
- Die Offset-Suche folgt AD-Normalen von Oberflächenpunkten; Regionen, die
  keine Normale eines Oberflächenpunkts trifft (tiefe Täler), sind
  unterrepräsentiert — die Worst-Case-Aussagen stützen sich auf die
  Neck-Region, die gut abgedeckt ist.
- `stack_near_*`-Jitter 0.3 mm ist klein gegen r_blend=6; größere Jitter
  interpolieren erwartbar Richtung Ketten-Verhalten (nicht gemessen).
