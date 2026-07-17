# Agent B (Graphics) — Forschungslog: SDF-Metriktreue

Richtung: Grafik-/SDF-Literatur — korrigierte Booleans, smin-Varianten,
Lipschitz-Steuerung (Segment Tracing, Interval/Affine Arithmetic, libfive/Fidget),
Feld-Normalisierung f/|∇f|.

Arbeitsverzeichnis: `docs/research/sdf-metricity/agent-b-graphics/`.
Alle Experimente: Python 3 + numpy/scipy, Skripte hier im Verzeichnis.

---

## Iteration 1 — smin-Varianten: Distanzfehler, Lipschitz, Sicherheit

### Quellen

- I. Quílez, „Smooth Minimum" (2024er Komplett-Rewrite mit Normalisierung,
  Kernel-Taxonomie, CD/DD-Familien, Circular + Circular-Geometrical),
  https://iquilezles.org/articles/smin/ — abgerufen 2026-07-17.
  Kernaussagen: (a) alle Varianten der **CD-Familie** („clamped difference",
  Kernel über h = max(k−|a−b|,0)/k) sind **garantiert Unterschätzer** der
  Distanz; (b) die **Circular-Geometrical**-Variante (= FieldDrafts
  `smin_poly`, IQs „round union") **überschätzt** Distanz in vielen Regionen
  („raymarcher will go faster than the Speed of Light"); (c) DD-Familie:
  Gradient = Konvexkombination der Input-Gradienten.
- E. Galin, E. Guérin, A. Paris, A. Peytavie, „Segment Tracing Using Local
  Lipschitz Bounds", Computer Graphics Forum 39(2) / Eurographics 2020.
  https://hal.science/hal-02507361 , Code: https://github.com/aparis69/Segment-Tracing
  — lokale Lipschitz-Schranke pro Ray-SEGMENT statt global; pro Primitiv und
  Operator wird eine Schranke λ(segment) hergeleitet; Schrittweite t += d/λ.
- M. Aydinlilar, C. Zanni, „Forward inclusion functions for ray-tracing
  implicit surfaces", Computers & Graphics 2023.
  https://inria.hal.science/hal-04129922 — asymmetrische Inklusionsfunktionen,
  exakt am Query-Punkt, linear/quadratisch; bottom-up über den Ausdrucksbaum
  (≙ unser Tape) komponierbar.
- Barbier et al., „Lipschitz Pruning: Hierarchical Simplification of
  Primitive-Based SDFs", CGF 2025 (https://onlinelibrary.wiley.com/doi/10.1111/cgf.70057) —
  Lipschitz-Schranken pro CSG-Teilbaum zum Pruning; bestätigt „Lipschitz-Metadaten
  pro Knoten" als tragfähiges Architekturmuster.
- Rvachev-Funktionen / V. Shapiro, „Semi-analytic geometry with R-functions"
  (2007): R-Booleans sind C^k-stetig, aber **nicht** distanzerhaltend;
  Normalisierung „bis Ordnung 1" (|∇f|=1 auf dem Rand) via f/|∇f| ist dort
  Standardtechnik (Taubin/Sampson-Distanz erster Ordnung).

### Ausgangslage im Code (Grounding)

- `crates/kernel/src/csg.rs::smin_poly` (Z. 2398) = IQ **Circular Geometrical**
  („round union"): `max(r, min(a,b)) − ‖(max(r−a,0), max(r−b,0))‖`.
  Doku dort: exakte Kreisbogen-Distanz im Arc-Bereich, √2-Lipschitz.
- Render-Pfad dämpft mit `masked_blend_render_fade` (÷√2 global auf das
  MaskedBlend-Feld, sdf_compiler.rs Z. 1826) + `MASKED_BLEND_MARCH_RELAX 0.75`;
  Mesher-Pfad (`EvalMode::Visual`) unnormalisiert.
- `march_relax` (sdf_compiler.rs Z. 164) ist ein GLOBALER Faktor pro Body
  (min über Features) — genau die „globale Über-Dämpfung" aus der Fragestellung.

### Hypothese H1 (Kern-Hypothese dieser Iteration)

IQs **Circular (CD)**-Variante
`f = min(a,b) − (k/2)·(1 + h − √(1 − h(h−2)))`, `h = max(k−|a−b|,0)/k`
hat in der (a,b)-Ebene **exakt dieselbe Nullstellenmenge** wie FieldDrafts
`smin_poly` mit r = k (also identische Fillet-GEOMETRIE bei jedem
Flächenwinkel), ist aber eine Konvexkombinations-Blende
(∂f/∂a + ∂f/∂b = 1, beide ∈ [0,1]) ⇒ **1-Lipschitz für 1-Lipschitz-Inputs
⇒ nie Überschätzung ⇒ sphere-trace-sicher ohne ÷√2 und ohne march_relax**.

Handrechnung (vor dem Experiment): Parametrisiere den Kreisbogen
a = k(1−cos φ), b = k(1−sin φ). Dann ist |a−b| = k(cos φ − sin φ),
1−h = cos φ − sin φ, √(1−h(h−2)) = √(2−(1−h)²) = sin φ + cos φ, und
g = (k/2)(2 − 2cos φ) = a = min(a,b) ⇒ f_CD = 0 auf genau dem Kreis
(k−a)² + (k−b)² = k², d. h. der Nullmenge von `smin_poly`. Gewichte:
∂f/∂a = 1 − (1/2)(1 + (1−h)/√(2−(1−h)²)) ∈ [0,1], Summe mit ∂f/∂b = 1.

Erwarteter Trade-off: `smin_poly` ist im Arc-Bereich EXAKTE Distanz zum
Fillet-Kreis (gut für Offset/Shell nahe der Naht), CD-circular unterschätzt
dort ⇒ H1 löst (a) Schrittweiten-Sicherheit, nicht automatisch (b)
Nahfeld-Metrik. Beides quantifizieren.

### Experiment E1 (`exp1_smin_variants.py`)

Aufbau: 2D, analytische Felder, Ground-Truth-Distanz = Distanz zur
bisektions-verfeinerten Nullkontur der jeweiligen Variante selbst
(KDTree über Konturpunkte, Feingrid 2000² auf [−5,5]², 12 Bisektionsschritte).
Konfigurationen:
1. Union zweier Halbebenen, Normalenwinkel γ ∈ {30°, 60°, 90°, 120°, 150°}
   (γ klein = Gradienten fast parallel = √2-Worst-Case).
2. Intersection (smax) derselben Halbebenen (konvexe Kante).
3. Smooth-Difference −smin(−a, b) (konkaver Kanten-Fillet, #386-Szenario).
4. Union zweier Kreise (R=2, Abstand 3, r=1).
Varianten: hard-min, chamfer (FieldDraft), circ_geom (= `smin_poly`),
circ_cd (IQ Circular, k_int = r), quad_cd, cubic_cd, root (DD), exp, R0
(Rvachev a+b−√(a²+b²)).
Metriken (README-Pflichtmetriken 1, 2, 4):
- Zero-Set-Identität circ_geom ↔ circ_cd (Hausdorff über Konturwolken).
- max |∇f| (zentrale Differenzen, h = 10⁻⁴), 99.9-Perzentil.
- Sicherheit: max (f − d_true) und max f/d_true über {f > 0, d_true > 0.02}.
- Nahfeld: max |f − sd_true| über |sd_true| < 0.5r bzw. < 0.1r.
- Marsch-Effizienz: mittleres f/d_true außen (1.0 = perfekt).

### Messergebnisse E1 (`results_e1.txt`, voller Dump dort)

**Methodik-Sackgasse (dokumentiert):** Die erste Kontur-Extraktion verlor
Nullstellen, wenn eine Null-MEMBRAN (Schnittflächen-Fortsetzung bei sdiff)
exakt durch Gitterknoten lief (`f==0` am Knoten ⇒ `a·b<0` schlägt fehl) —
das erzeugte einen Phantom-Hausdorff von 0.304 bei sdiff_90. Fix:
asymmetrischer Sub-Zellen-Shift des Extraktionsgitters. Lehre fürs Harness:
Ground-Truth-Konturen nie auf achsen-symmetrischen Gittern extrahieren.

**Befund 1 — Zero-Set-Identität bestätigt (Kernresultat).**
Hausdorff(circ_geom, circ_cd) = **0.00** (unter Bisektions-Präzision ~10⁻⁴)
auf ALLEN 16 Konfigurationen (Union/Intersect/SDiff × 30–150°, Kreis-Union);
zusätzlich kein einziger Vorzeichen-Mismatch auf 900²-Grids. Der Tausch
`smin_poly` → IQ-Circular-CD ändert die Geometrie NICHT — weder Fillet-Bogen
noch Fernfeld (out-of-band sind beide = min).

**Befund 2 — der aktuelle `smin_poly` (circ_geom) ist quantifiziert unsicher.**
max |∇f| = √(1+cos γ) exakt wie Theorie: **1.366 @ γ=30°**, 1.225 @ 60°,
1.000 @ 90°. Überschätzung der wahren Distanz zur eigenen Fläche bis
**Faktor 1.366** (max f−d_true = 0.92·r bei intersect_30 schon im ±2-Fenster).
Der Chamfer-Modus ist GENAUSO betroffen (1.366 @ 30°) — der ÷√2-Fade
adressiert bisher nur den MaskedBlend-Pfad, nicht SmoothUnion/-Diff generell.
Kreis-Union: |∇f| 1.13, Überschätzung 1.10×.

**Befund 3 — circ_cd ist in JEDER Konfiguration exakt 1-Lipschitz und
überschätzungsfrei.** |∇f|max = 1.000 (alle 16 Configs), over_ratio = 1.000,
over_abs = 0.0000. Marsch-Effizienz (mittleres f/d_true außen) 0.85–0.98 —
gegenüber dem heutigen effektiven Faktor des Render-Pfads
(÷√2 · march_relax 0.75 ⇒ ~0.53) ein Gewinn von grob 1.6–1.8× pro Schritt.
quad_cd/cubic_cd ebenso sicher (theoretisch klar: Konvexkombinations-
Gewichte), aber ohne exakten Kreisbogen-Querschnitt.

**Befund 4 — Trade-off Nahfeld-Metrik ist winkelabhängig, nicht pauschal.**
|f − sd_true| im Band |sd|<0.5r: bei γ=90° ist circ_geom fast exakt (0.003)
vs. circ_cd 0.146 (≈ (1−1/√2)r·Konstante — die CD-Band-Unterschätzung).
Bei γ=30° kehrt es sich um: circ_geom 0.183, circ_cd **0.017**. D. h. die
„exakte Bogen-Distanz" von circ_geom gilt nur nahe senkrechter Flächen;
an scharfen/stumpfen Winkeln ist das konservative Feld auch metrisch besser.
Tief im Inneren (Shell-relevant): circ_geom wächst ×√2 mit der Tiefe entlang
der Naht-Bisektrix (unbeschränkter Fehler), circ_cd-Fehler bleibt ≤ 0.293r
(Band-Offset konstant). Rvachev R0 unnormalisiert: L bis 3.3,
Überschätzung 4.6 — als direkte Boolean-Alternative disqualifiziert;
R-Funktionen nur mit nachgeschalteter Normalisierung (→ Iteration 2) sinnvoll.

### Schlussfolgerung Iteration 1

H1 **bestätigt**: Der Formeltausch `smin_poly`/`blend_min_eval` (+ TSL-Spiegel)
auf IQ-Circular-CD liefert identische Geometrie bei strikt 1-Lipschitz-Feld ⇒
`masked_blend_render_fade`s ÷√2 und `MASKED_BLEND_MARCH_RELAX 0.75` werden
für den smin-Anteil obsolet (der Fade-Term `(blended−hard)·∇w` ist separat zu
prüfen — mit 1-Lipschitz-Inputs beiderseits sollte der Cross-Fade mit
w∈[0,1] plus kleinem ∇w-Restterm deutlich unter 1.06 fallen). Chamfer-Modus
braucht eine eigene konservative Variante (z. B. clamp der Chamfer-Ebene in
CD-Form: `min(a,b) − max(k−|a−b|,0)·c`-Familie; offen).
Achtung Nicht-Lokalität: CD-Band ist der Slab |a−b|<k bis ins Unendliche —
Feldwerte (nicht Geometrie!) ändern sich entlang der Bisektrix auch fern der
Naht um bis zu 0.293r (Unterschätzung). Mesher-Sizing-Felder sehen dort
kleinere |f| (vgl. Displace-Befund in bvh.rs) — vor einem Mesher-Rollout am
Lichen-Harness gegenmessen.

---

## Iteration 2 — Lipschitz-Steuerung, Trace-Simulation, Sampson-Korrektur

### Quellen (zusätzlich)

- M. J. Keeter, „Massively Parallel Rendering of Complex Closed-Form Implicit
  Surfaces", ACM TOG 39(4) / SIGGRAPH 2020, https://www.mattkeeter.com/research/mpr/
  — Kernmechanik: Intervall-Arithmetik AUF DEM TAPE (Region → Wertintervall),
  leere Regionen überspringen + Tape-Reduktion (bis 2 Größenordnungen kürzer);
  braucht nur C⁰, KEINE Distanz-Garantien — rendert per Regionen-Rejektion
  statt Sphere-Trace. libfive/Fidget sind dieselbe Familie.
- Galin et al. 2020 (s. o.): Schrittweite t += d/λ(Segment) mit pro-Primitiv/
  pro-Operator hergeleiteter LOKALER Lipschitz-Schranke über das Ray-Segment;
  Operatoren: Union→max der Kind-Schranken, Blend→gewichtete Summe,
  Warp→×Jacobian-Schranke. Genau das „pro Opcode Metadaten"-Muster.

### Experiment E2 (`exp2_trace_depth_sampson.py`, Ausgabe `results_e2.txt`)

**A — Sphere-Trace-Simulation des #386-Szenarios** (4000 Rays in den konkaven
Luftkeil der Union γ=30°, Ground-Truth-Schnitt via dichte Abtastung+Bisektion;
Tunnel = konvergiert > 0.01 hinter dem wahren Schnitt oder gar nicht):

| Feld/Politik | Tunnel-Rate | Schritte mean/p95 |
|---|---|---|
| `smin_poly` roh, volle Schritte | **85.25 %** | 1.8 / 2 |
| ÷√2 (#382), volle Schritte | 0 % | 5.8 / 7 |
| ÷√2 + relax 0.75 (heutiger Stand) | 0 % | 10.7 / 12 |
| **circ_cd, volle Schritte** | **0 %** | **4.0 / 5** |

Konvexe Kante grazing (A2, intersect 30°): geom roh 24.1 % Tunnel;
heutige Politik 12.3 Schritte; circ_cd 7.0 Schritte, 0 % Tunnel.
⇒ circ_cd ist sicher UND ~2.7× (konkav) bzw. ~1.75× (grazing) schneller als
die heutige ÷√2+0.75-Dämpfung. Die 85 % Roh-Tunnelrate quantifiziert, warum
#386 als „graue Wand" erschien.

**B — Verschachtelungstiefe** (smin-Union-Kette aus N Kreisen, r=0.6):
geom: |∇f|max ≈ 1.25, Überschätzung 1.167 — konstant über N=2…12 (der Fehler
ist WINKEL-getrieben, nicht tiefen-getrieben; keine Kompoundierung in dieser
gutartigen Kette). circ_cd: exakt 1.000 / 1.000 für ALLE N — die
Konvexkombinations-Eigenschaft überlebt beliebige Verschachtelung
(theoretisch klar, hier bis N=12 gemessen).

**C — Sampson-Korrektur f/|∇f|** (Finite-Differenzen-Gradient, h=10⁻⁴):

| Fall | over roh→S | near50 roh→S | near10 roh→S |
|---|---|---|---|
| circ_geom union30 | 1.366→1.363 | 0.183→0.106 | 0.0362→**0.0037** |
| rvachev_r0 union60 | 0.994→**1.271** | 0.970→0.136 | 0.185→0.027 |
| twist(0.35)+box | 1.414→1.251 | 0.196→0.152 | 0.0395→**0.0556 (!)** |
| hard_min 2 Kreise | 1.000→1.000 | 0.105→0.105 | 0.013→0.013 |

Interpretation:
1. Sampson repariert Nahfeld-Metrik von SKALIERTEN/verzerrten C¹-Feldern
   um 5–10× (circ_geom 10×, Rvachev 7×) — R-Funktionen werden erst mit
   Normalisierung metrisch brauchbar (klassisches Shapiro-Resultat,
   hier quantifiziert).
2. Sampson ist KEIN Sicherheitsmechanismus: over bleibt >1.25, bei Rvachev
   macht es ein sicheres Feld sogar UNSICHER (0.994→1.271). Marsch-Sicherheit
   und Nahfeld-Metrik sind orthogonale Probleme.
3. Beim Twist-Warp verschlechtert FD-Sampson das enge Band (0.0395→0.0556):
   Gradienten-Sprünge (Box-Kanten unterm Warp) + FD-Kreuz über die
   Diskontinuität → Ausreißer. Hypothese für Iteration 3: exakter
   AD-Gradient (Zweikanal-Tape) statt FD behebt das teilweise; an
   C⁰-Knicken bleibt f/|∇f| prinzipiell heikel.
4. **Negativresultat (wichtig):** Beim harten min ändert Sampson NICHTS
   (|∇f|=1 fast überall) — der Boolean-Schranken-Fehler ist NICHTLOKAL
   (Information über die weggeschnittene Randfläche fehlt punktweise).
   Keine punktweise Normalisierung kann Offset/Shell über harten Booleans
   reparieren. Deckt sich mit der Literatur: Keeter/libfive verzichten
   deshalb GANZ auf Distanzsemantik und rendern über Intervall-Rejektion.

### Strukturelle Analyse Offset/Shell über Booleans (Algebra, dann E2d)

Dilatation distribuiert über Vereinigung: `min(a,b) − δ = min(a−δ, b−δ)` —
die AUSSEN-Offset-Fläche einer harten Union ist mit exakten Leaf-SDFs also
bereits GEOMETRISCH EXAKT (ebenso Innen-Offset einer Intersection, dual).
Kaputt sind genau: Innen-Offset/Shell-Innenwand über Union (Erosion der
Union ⊋ Union der Erosionen) und Außen-Offset über Intersection/Difference.
Das ist der nichtlokale Kern von Punkt (b) der Forschungsfrage. → E2d misst
die Größe des Defekts.

### E2d — Erosions-Defekt quantifiziert (`exp2d_erosion_defect.py`, `results_e2d.txt`)

Zwei Kreise R=1.5, Zentren (±0.8, 0), Innen-Offset δ: Hausdorff zwischen
{min = −δ} (was das Feld liefert) und wahrer Erosion {sd_true = −δ}:

| δ | Hausdorff | in % von δ |
|---|---|---|
| 0.2 | 0.044 | 22 % |
| 0.4 | 0.113 | 28 % |
| 0.6 | 0.255 | 42 % |
| 0.8 | 0.473 | 59 % |

Größter Defekt exakt auf der Naht-Bisektrix (x=0) — Shell-Innenwände über
Union-Nähten werden strukturell zu stark ausgeräumt, superlinear in δ.
Punktweise unheilbar (siehe C.4); Korrektur braucht nichtlokale Information
(Redistancing / BREP-Proxy — Spur von Agent A/C) oder Render-seitig
Intervall-Wurzelsuche auf dem Ray (Keeter-Muster, keine Metrik nötig).

---

## Iteration 3 — Zweikanal-AD-Tape und Segment-Tracing (`exp3_tape_ad_segment.py`)

### Hypothesen

- H3a: Der Twist-Nahfeld-Rückschritt von FD-Sampson (E2 C.3) ist ein
  Finite-Differenzen-Artefakt; exakter forward-AD-Gradient (Wert+Gradient-
  Kanal durchs Tape, Warp-Jacobians per Kettenregel) behebt ihn.
- H3b: Segment-lokale Lipschitz-Schranken (Galin 2020) aus pro-Opcode-
  Metadaten erreichen Sicherheit OHNE globale Dämpfung und mit deutlich
  weniger Schritten.

### Ergebnisse (`results_e3.txt`)

**A — AD- vs. FD-Sampson auf twist(0.35)∘box:** identisch bis zur 4.
Nachkommastelle (over 1.251, near50 0.1524, near10 0.0556 beide).
**H3a WIDERLEGT** — der Rückschritt ist kein FD-Artefakt, sondern
prinzipiell: f/|∇f| korrigiert mit dem LOKALEN Gradienten-Ast; nahe
C¹-Knicken (Box-Ecken unterm Warp) gehört der nächste Oberflächenpunkt zu
einem anderen Ast. Sampson ist nur dort verlässlich, wo das Feld auf dem Weg
zur Fläche glatt ist. (Der AD-Kanal bleibt trotzdem wertvoll: exakte
Normalen, Kosten ~3–4× ALU, s. Synthese.)

**B — Segment-Tracing vs. globale Dämpfung** (3000 Rays, twist(0.35)∘box,
λ(seg) = 1 + |τ|·max(‖p₀‖,‖p₁‖) als pro-Opcode-Regel, 2 Fixpunkt-Iterationen
pro Schritt):

| Politik | Tunnel | Schritte mean/p95 |
|---|---|---|
| naiv volle Schritte | 22.9 % | 9.3 / 23 |
| global 1/L = 0.276 | 0 % | 47.6 / 90 |
| **segment-lokal** | **0 %** | **16.6 / 34** |

**H3b BESTÄTIGT:** sicher UND 2.9× weniger Schritte als die globale
Dämpfung. Das ist die direkte Blaupause für `march_relax`-Ablösung bei
Twist/Bend/Displace/Revolve: die Schranke ist eine Ein-Zeilen-Formel aus
Größen, die der jeweilige Warp-Opcode ohnehin kennt (Radius zur Achse,
depth·freq, Profil-Abstand).

---

## Iteration 4 — MaskedBlend-Fade unter circ_cd (`exp4_masked_blend_fade.py`, `results_e4.txt`, `results_e4b.txt`)

Frage: Fallen nach dem circ_cd-Tausch ÷√2 UND `MASKED_BLEND_MARCH_RELAX`?
2D-Analogon des Kernel-Fades (Masken-Disc 1.5r, smoothstep-Band r,
f = hard + (blended−hard)·w, OHNE ÷√2), Winkel-Sweep 20°–170°:

- geom-Blend (heutige Formel, roh): |∇f| bis **1.50**, Überschätzung bis
  1.47 (sdiff_170). ÷√2 drückt das auf ≈1.06 — deckt sich mit der
  Kernel-Messung aus #386.
- **cd-Blend: für Flächenwinkel-Konfigurationen bis 120° ist over = 1.000**;
  erst extreme Winkel überschätzen (150°: 1.027, 170°: 1.110 bei sdiff;
  union_20: 1.246). Der verbleibende Übeltäter ist der Fade-Term
  `(blended−hard)·∇w` selbst: bei fast parallelen Gradienten reicht die
  smin-Abweichung (~0.29r) räumlich weit über den Masken-Tubus hinaus
  (Band |a−b|<r wird bei kleinem γ räumlich breit, ∝ 1/sin(γ/2)), und der
  Fade muss sie über ein FESTES Band abbauen → steil.
- Alternative „Radius-Fade" (r_eff = r·w statt Feld-Crossfade): mit cd
  praktisch gleichauf (max 1.247), mit geom DEUTLICH schlechter (bis 1.59) —
  kein Gewinn, Crossfade kann bleiben.

Konsequenz: ÷√2 kann mit cd entfallen; eine Rest-Dämpfung nur für den
MaskedBlend braucht es bei scharfen Dihedralen weiterhin, aber (a) milder
(0.8 statt heute effektiv ÷√2·0.75 ≈ 0.53) und (b) am besten RÄUMLICH LOKAL:
der Opcode kennt `mask_d` bereits — außerhalb des Fade-Bands (mask_d > 0)
ist das Feld exakt das harte cd-Feld (L = 1, volle Schritte), nur im Band
lokal dämpfen. Das ist Segment-Tracing-lite mit null zusätzlichen Evals.

---

## Synthese (Antwort auf die Kernfrage, Stand nach 4 Iterationen)

(a) **Sichere Schrittweiten ohne globale Über-Dämpfung:**
1. smin-Formeltausch auf IQ-Circular-CD: identische Geometrie (Hausdorff 0
   auf 16 Konfigurationen), strikt 1-Lipschitz, komposition-stabil (N=12
   gemessen), 1.75–2.7× schnellere Traces als die heutige Politik bei 0 %
   Tunnel. Chamfer-Modus braucht ein CD-Gegenstück (offen, konstruierbar).
2. Für Warps: segment-lokale λ aus pro-Opcode-Metadaten (Galin) — 2.9×
   schneller als globales relax, 0 % Tunnel; Formeln pro Opcode trivial.
3. MaskedBlend: lokale Dämpfung nur im Fade-Band via vorhandenem mask_d.

(b) **Betragskorrekte Nahfeld-Distanz für Offset/Shell:**
- Über glatten verzerrten Feldern: Sampson f/|∇f| bringt 5–10× im Nahband,
  ABER nicht an C¹-Knicken und nie als Sicherheitsmechanismus.
- Über harten Booleans ist der Fehler NICHTLOKAL (Sampson wirkungslos;
  Erosions-Defekt 22–59 % von δ, superlinear). Außen-Offset über Union und
  Innen-Offset über Intersection sind dagegen algebraisch EXAKT — eine
  Offset-Korrektheits-Klassifikation pro Tape-Knoten (Vorzeichen von δ ×
  Boolean-Typ) wäre ein ehrlicher erster Schritt: die kaputten Fälle
  erkennen und dem Nutzer/Meshern signalisieren bzw. an einen
  nichtlokalen Pfad (Proxy/Redistancing, andere Agenten-Spuren) delegieren.


