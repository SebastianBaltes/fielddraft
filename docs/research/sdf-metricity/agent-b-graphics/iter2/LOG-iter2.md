# Agent B — Iteration 2, Experiment 1: Circular-CD-smin-Tausch REAL im Kernel

Datum 2026-07-18. Auftrag aus SYNTHESIS.md §4.1: den in Iteration 1 nur am
2D-Modell bestätigten Formeltausch `smin_poly` → IQ Circular-CD **im echten
Kernel** durchführen und das Rollout-Gate messen: Zero-Set-Parität,
Feldmetriken, Trace-Verhalten inkl. ÷√2-Frage, Mesher-Regression.

## Quellen

- Iteration-1-Befund H1 (`../LOG.md`): Formel
  `f = min(a,b) − (r/2)·(1 + h − √(1 − h·(h−2)))`, `h = max(r − |a−b|, 0)/r`
  (I. Quílez, „Smooth Minimum", 2024, Circular-CD-Familie; Zero-Set identisch
  zum zirkulär-geometrischen smin, Konvexkombination ⇒ 1-Lipschitz).
- Harness `crates/metricity-lab` (`../../HARNESS.md`), GT validiert ≤ 2.4e-4 rel.
- Kernel-Stand: Worktree auf `aa9dcc47` (v0.16.0 + Forschungs-Docs).

## Hypothese

Der Tausch ändert die Nullmenge NICHT (Hausdorff 0), macht das Feld strikt
1-Lipschitz und überschätzungsfrei, erlaubt volle Trace-Schritte ohne
÷√2-Fade und bricht keine Mesher-/Weave-/Fillet-Tests. Bekanntes Risiko:
Bisektrix-Unterschätzung ≤ 0.293·r (Feldwerte, nicht Geometrie) —
NICHTLOKAL, da das CD-Band der Slab |a−b| < r bis ins Unendliche ist.

## Aufbau

Patch (`circ_cd.patch`) an ALLEN vier CPU-Spiegeln des zirkulären smin,
damit Wert-, Gradienten- und Interval-Pfad konsistent bleiben:

1. `crates/kernel/src/csg.rs::smin_poly` (Visual-/Mesher-Pfad) — neue
   `smin_circ_cd`, Laufzeit-Schalter `csg::smin_circ_cd_enabled()`
   (Env `FD_SMIN_CIRC_CD`, Default = CD). Schalter NUR fürs Experiment;
   ein Produktions-Rollout tauscht hart (spart den Atomic-Load pro Knoten).
2. `crates/kernel/src/sdf_compiler.rs::blend_min_eval` (Bytecode-Tape =
   CPU-Referenz des WGSL-Interpreters) — delegiert im Fillet-Zweig an
   `csg::smin_poly`, byte-konsistent.
3. `crates/kernel/src/sdf_dual.rs::smin_poly_generic` (Dual-Number-Pfad,
   Weave-Gradienten) — generisches CD-Gegenstück `smin_circ_cd_generic`.
4. `crates/kernel/src/fidget_bridge.rs::smin_poly` (Interval-DC-Tree).

Zusätzlich Schalter `sdf_compiler::mb_fade_div_sqrt2_enabled()`
(Env `FD_MB_FADE_SQRT2`), um den globalen ÷√2 des
`masked_blend_render_fade` (#382) für Messungen abschaltbar zu machen.
Chamfer-Modus unangetastet (CD-Gegenstück weiterhin offen). Der WGSL/TSL-
Mirror (`web/src/sdfVmInterpreter.ts`) ist für dieses CPU-Experiment außen
vor, MUSS bei einem Rollout aber synchron gezogen werden.

Messbinary: `crates/metricity-lab/src/bin/iter2_circ_cd.rs` (Kopie:
`iter2_circ_cd.rs.txt`). Schalter werden nur phasenweise sequentiell
umgelegt (rayon-sicher). Seeds im Code (42/43/777/4242/0xB001^0x384,
Seam-Probe 0x5EA0386).

## Messwerte

### 1. Zero-Set-Parität alt/neu (`parity.csv`)

Dichte Achsen-Linien-Scans (48² Linien × 3 Richtungen, 2048 Samples,
Bisektion 48 Iter) auf dem echten `CsgNode::sdf`-Visual-Feld; Nullstellen-
Matching je Linie (Hausdorff-artige Maximalabweichung); Vorzeichen-Parität
an 50 000 Zufallspunkten je Szene:

| Szene | max. Nullstellen-Abweichung | Roots alt/neu | unmatched | Sign-Mismatch |
|---|---|---|---|---|
| masked_blend_edge | 0.000002 mm | 7784 / 7784 | 0 | 0 / 50000 |
| smin_chain_02 | 0.000026 mm | 9152 / 9152 | 0 | 0 / 50000 |
| smin_chain_16 | 0.000025 mm | 7838 / 7838 | 0 | 0 / 50000 |

Szenen ohne smin-Knoten (tangent_spheres p10/p01/p00/m01/m25, thin_wall
5 %/0.5 %): max |f_alt − f_neu| = **0.000e0** über je 50 000 Punkte —
bit-identisch; der Tausch fasst harte Booleans nicht an.

**Iteration-1-Erwartung „identisch (0.00)" am echten Kernel bestätigt**
(Restabweichung ≤ 2.6e-5 mm ≈ 10⁻⁷ der Modellgröße = f32/Bisektions-
Rauschen).

### 2. Feldmetriken (`field_bands.csv`, `field_grad_over.csv`)

GT auf dem Alt-Feld gebaut (Nullmengen identisch), 6000 uniform + 6000
near-surface Punkte, beide Varianten gegen dieselben d_wahr.

**Überschätzung max(f − d_wahr, 0) über Außenpunkte — die Rollout-relevante
Trace-Sicherheitsgröße:**

| Szene | alt max / p99 | neu max / p99 |
|---|---|---|
| smin_chain_02 | 0.851 / 0.382 mm | **0.000005 / 0.000003 mm** |
| smin_chain_16 | 0.927 / 0.498 mm | **0.000027 / 0.000016 mm** |
| masked_blend_edge (visual) | 0.000004 mm | 0.000004 mm |

**‖∇f‖-Statistik (zentrale Differenzen, h = 1e-3·Modellgröße):**

| Szene | alt max / p99 | neu max / p99 |
|---|---|---|
| smin_chain_02 | 1.222 / 1.134 | **1.0001 / 1.0000** |
| smin_chain_16 | 1.216 / 1.152 | **1.0002 / 1.0000** |
| masked_blend_edge (visual) | 1.040 / 1.000 | 5.314 / 1.000 (!) → Befund 3 |

Erwartung „max ≈ 1.0" bestätigt; die Ketten liegen alt winkelbedingt bei
1.22 statt des theoretischen Worst-Case 1.366 (deckt sich mit Iteration-1
E2B ≈ 1.25). CD ist bis in die 4. Nachkommastelle 1-Lipschitz.

**|f − d_wahr| nach Bändern (Auszug, mm; bias = signierter Mittelwert):**

| Szene/Band | alt mean/p95/max (bias) | neu mean/p95/max (bias) |
|---|---|---|
| chain_16 near_out | 0.061/0.379/0.927 (+0.061) | 0.128/0.619/1.000 (−0.128) |
| chain_16 mid_out | 0.016/0.099/0.748 (+0.016) | 0.508/1.496/1.763 (−0.508) |
| chain_16 far_out | 0.00001/0.00002/0.009 (−0.0) | **0.751/1.832/2.721 (−0.751)** |
| mbe near_out | 3e-6/3e-6/0.0002 (−0.0) | 0.006/0.0002/0.382 (−0.006) |
| mbe mid_out | 4e-5/0.0003/0.0012 (−0.0) | 0.028/0.030/1.692 (−0.028) |
| mbe far_out | 0.0012/0.0053/0.063 (−0.0012) | 0.006/0.0055/1.586 (−0.006) |

Der Trade-off ist real und GRÖSSER als die 2D-Zahlen suggerierten: die
CD-Unterschätzung ist nicht auf die Naht-Nähe beschränkt (Slab bis ins
Fernfeld). Auf der 16er-Kette akkumuliert sie auf bis zu **2.72 mm
Fernfeld-Unterschätzung** (0.45·r_blend; sublinear in der Kettenlänge —
einzelner Blend bleibt ≤ 0.293·r —, alt war dort f32-exakt). Vorzeichen
durchweg negativ = konservativ: Traces bleiben korrekt (nur langsamer im
Slab), aber |f|-basierte Abfragen (Offset-Vorschau, Sizing) sehen dort
systematisch zu kleine Distanzen.

### 3. NEUER BEFUND — CD × MaskedBlend: Wertesprung an der Maskengrenze im Visual-Feld

`masked_blend_edge` visual: ‖∇f‖max alt 1.04 → neu **5.31**. Ursache
(`mask_jump_profile.csv`, Profil entlang der Wedge-Bisektrix (1,1,0)/√2 ab
Kante (12,20,0) durch den Tube-Rand bei t = mask_r = 15):

| Feld | max |Δf| über Δt = 0.002 mm |
|---|---|
| visual alt | 0.00142 mm (≈ stetig) |
| visual neu | **1.75877 mm** ≈ (1−1/√2)·r = 1.757 |
| tape alt (Crossfade+÷√2) | 0.00100 (Steigung ≈ 0.50) |
| tape neu (Crossfade) | 0.00162 (Steigung ≈ 0.81) |

Der Visual-Pfad schaltet an der Masken-Tube-Grenze HART zwischen `blended`
und `hard` (#382-Crossfade existiert nur im Tape). Das alte geometrische
smin ist LOKAL (blended ≡ hard außerhalb von Radius r um die Kante,
Schaltsprung ≈ 0). Das CD-smin weicht im GESAMTEN Slab |a−b| < r von min
ab — an der Tube-Grenze (15 mm ≫ r = 6 mm) steht damit ein echter Sprung
von (1−1/√2)·r. Nullmenge unberührt (der Sprung liegt fern der Fläche,
Parität = 0, Vorzeichen identisch); der Tape-/Render-Pfad bleibt durch den
Crossfade C1-stetig (Steigung 0.81 < 1 auch OHNE ÷√2). Konsequenz nur für
den Visual-Pfad: als Distanzfeld ist er an der Maskengrenze unstetig —
für Vorzeichen-Leser (Dual Contouring, Lichen-Sign-Sampling) egal
(empirisch: alle Mesher-Tests grün, s. Gate), für |f|-basierte Abfragen
über die Tube-Grenze hinweg nicht. Rollout-Empfehlung: den Tape-Crossfade
in den Visual-Pfad spiegeln (macht die Pfade zugleich konsistenter) oder
den Befund als bekannte Eigenschaft dokumentieren.

### 4. Sphere-Trace auf dem Tape (`trace.csv`, `trace_seam.csv`)

3000–4000 GT-treffende Rays je Szene (Ursprung Bounding-Kugel 1.6R;
GT-Erstschnitt: dichter Marsch 6000 Samples + Bisektion auf dem
Visual-Feld), Trace auf `eval_opcodes`, hit_eps 0.05 mm, Tunnel =
Konvergenz > 0.1 mm hinter dem wahren Schnitt oder Durchflug.
relax = 1 heißt VOLLE Schritte.

**Uniform gezielte Rays:**

| Szene | Politik | Tunnel | Schritte mean/p95 |
|---|---|---|---|
| smin_chain_02 | alt, volle Schritte | **1.77 %** | 6.5 / 14 |
| smin_chain_02 | alt, relax 0.75 | 0 % | 10.2 / 19 |
| smin_chain_02 | **neu, volle Schritte** | **0 %** | 6.9 / 14 |
| smin_chain_16 | alt, volle Schritte | **2.93 %** | 10.4 / 24 |
| smin_chain_16 | alt, relax 0.75 | 0 % | 15.5 / 33 |
| smin_chain_16 | **neu, volle Schritte** | **0 %** | 11.3 / 27 |
| masked_blend_edge | alt, ÷√2 (heutiger Stand) | 0 % | 14.0 / 29 |
| masked_blend_edge | alt, OHNE ÷√2 | 0 % | 9.1 / 21 |
| masked_blend_edge | neu, ÷√2 | 0 % | 14.1 / 30 |
| masked_blend_edge | **neu, OHNE ÷√2** | **0 %** | **9.2 / 21** |
| tangent_spheres_m01 (hart, Kontrolle) | alle | 0 % | 8.3 / 19 |

**Seam-gezielt** (4000 Rays auf den Junction-Ring gejittert, Ball r=6 —
der #386-Stressfall):

| Politik | Tunnel | Schritte mean/p95 |
|---|---|---|
| alt, ÷√2, relax 1 (heute) | 0 % | 15.3 / 36 |
| alt, ÷√2, relax 0.75 | 0 % | 21.3 / 49 |
| alt, OHNE ÷√2 | 0 % | 10.0 / 26 |
| **neu, OHNE ÷√2** | **0 %** | **10.6 / 27** |

Lesart:
- Das Alt-Feld tunnelt mit vollen Schritten auf den smin-Ketten REAL
  (1.8–2.9 % der Rays, am echten Kernel) — CD eliminiert das vollständig
  bei +4–9 % Schritten (konservativere Werte im Slab).
- Auf dem 90°-Kanten-Fillet tunnelt auch das Alt-Feld ohne ÷√2 nicht
  (geom-smin hat bei 90° ‖∇f‖ = 1; der Iteration-1-2D-Worst-Case 85 %
  gilt für spitze Winkel γ→30°, die diese Szene nicht enthält —
  **Negativbefund zur Übertragbarkeit der 2D-Zahl**, die Ketten-Szenen
  liefern den realen Beleg fürs Tunneln).
- Der heutige ÷√2 kostet ~50 % mehr Schritte (14.0 vs 9.2 uniform,
  15.3 vs 10.6 seam). Mit CD ist die Variante OHNE ÷√2 in allen
  gemessenen Konfigurationen tunnelfrei UND per Konstruktion sicher
  (1-Lipschitz-Blend; Fade-Restterm gemessen 0.81 < 1). Gegenüber der
  Politik ÷√2+relax0.75 ist neu-ohne-÷√2 **2.0× schneller** (21.3→10.6).

### 5. Kosten (`cost.csv`, 1M Evals single-thread, INKL. Schalter-Branch)

| Szene | csg alt→neu (ns/Eval) | tape alt→neu |
|---|---|---|
| masked_blend_edge | 31.5 → 31.3 | 58.0 → 57.7 |
| smin_chain_02 | 16.2 → 15.8 | 40.2 → 44.3 (+10 %) |
| smin_chain_16 | 159.9 → 208.8 (+31 %) | 355.2 → 417.5 (+18 %) |

CD kostet pro smin-Knoten mehr (÷r, abs, plus Atomic-Load des
Experiment-Schalters); ein harter Tausch entfernt den Load. Auf der GPU
dominiert die eingesparte Schrittzahl (−35 % ggü. ÷√2, −50 % ggü.
÷√2+relax).

### 6. Mesher-Regressions-Gate (`tests_before.txt`, `tests_after.txt`)

`cargo test --workspace --release --no-fail-fast`, identisches Binary,
Formel per Env-Schalter:

- **Referenz (Alt-Formel, `FD_SMIN_CIRC_CD=0`): 1219 passed, 0 failed.**
- **CD (Default): 1218 passed, 1 failed.**

Der EINZIGE Fehlschlag: `csg::tests::apply_global_round_rounds_a_union_edge`
— kein Mesher-Test, sondern eine Wert-Semantik-Assertion am Probe-Punkt
(10,10,0): dort ist da=0, db=−10, |a−b|=10 ≫ r=3 (außerhalb des CD-Bands
⇒ CD ≡ min = −10 exakt). Der Alt-Wert −10.34 < −10 entstand durch die
UNBESCHRÄNKTE Innen-Inflation des geometrischen smin (ub = r−db wächst mit
der Tiefe) — exakt der Metrikfehler, den CD beseitigt. Der Test kodierte
also den Defekt als Soll. Fix im Patch enthalten: Probe-Punkt auf die
tatsächliche konkave Naht (−1,11,0) mit da=db=1 im Arc-Bereich — besteht
unter BEIDEN Formeln (alt 0.17 < 1, neu 0.12 < 1). Mit dem Fix:
**voller Workspace grün unter CD** (finaler Lauf s. tests_after.txt).

Gezielt geprüft (Namen mit fillet/mesh/weave/lichen/golden/smin/blend,
220 Tests): **alle grün unter CD**, darunter die komplette
Lichen-Watertight-Familie (`lichen_*_watertight`, adversarial matrix,
gyroid, thin_plate, demo-Projekte), `mesher_points_cdt_*_watertight`,
Weave-/BRep-/Fillet-Pfade. **Die Bisektrix-Unterschätzung (≤ 0.293·r
Feldwerte, Fernfeld-akkumuliert bis 0.45·r) schlägt NICHT auf
Mesh-Sizing/offene Kanten durch** — plausibel, weil Unterschätzung das
Sizing nur verdichtet (kleinere |f| ⇒ konservativere Zellen), nie Löcher
reißt; die Nullmenge selbst ist paritätisch (Befund 1). Separate
Golden-/Snapshot-Mesh-Checks außerhalb des Workspace-Suites existieren
nicht (CGAL-Golden lebt serverseitig und hängt am SDF-Vorzeichen, das
unverändert ist).

## Schlussfolgerung

1. **H1 hält am echten Kernel.** Zero-Set-Parität exakt (≤ 2.6e-5 mm),
   ‖∇f‖max 1.22 → 1.0002, Überschätzung 0.93 mm → ~0 (f32-Rauschen),
   Tunnelrate mit vollen Schritten 1.8–2.9 % → 0 %, ÷√2 verzichtbar
   (−35…−50 % Schritte). Harte Booleans bit-unverändert.
2. **Ehrliche Negativbefunde:** (a) Die 2D-85 %-Tunnelzahl aus Iteration 1
   überträgt sich NICHT auf das 90°-Fillet — Tunneln des Alt-Felds ist
   real, aber winkelabhängig (Ketten ja, 90°-Kante nein). (b) Die
   CD-Unterschätzung ist nichtlokal und akkumuliert über Ketten bis
   0.45·r im Fernfeld, wo das Alt-Feld exakt war. (c) NEU entdeckt: am
   MaskedBlend reißt CD im VISUAL-Pfad einen (1−1/√2)·r-Wertesprung an
   der Masken-Tube-Grenze auf (Tape/Render bleibt stetig). (d) CPU-Kosten
   +18–31 % auf smin-lastigen Szenen (mit Schalter-Overhead).
3. **Rollout-Gate: BESTANDEN UNTER AUFLAGEN.**
   - Auflage 1: Test-Probe-Punkt-Fix `apply_global_round_rounds_a_union_edge`
     mitnehmen (im Patch; der alte Punkt kodierte den Metrikdefekt).
   - Auflage 2: WGSL/TSL-Mirror (`web/src/sdfVmInterpreter.ts`) im selben
     Commit tauschen — sonst rendert die GPU ein anderes Feld als die
     CPU-Referenz.
   - Auflage 3: MaskedBlend-Visual-Sprung adressieren (Crossfade in den
     Visual-Pfad spiegeln ODER als bekannte Eigenschaft dokumentieren;
     Mesher empirisch unbeeinträchtigt).
   - Auflage 4: ÷√2-Fade und Rest-relax erst NACH dem Mirror-Tausch
     entfernen; Chamfer-Modus behält bis zum CD-Gegenstück seine heutige
     Dämpfungspolitik.
   - Für Offset/Shell/Sizing gilt weiterhin: CD liefert Under-Bounds —
     magnitudentreue Nahfeld-Distanz bleibt Sache der Iteration-2-
     Experimente 2/3 (Proxy/Rewrites), CD ersetzt sie nicht.

## Dateien

- `circ_cd.patch` — Kernel-Patch (4 Spiegel + Schalter + Test-Fix).
- `parity.csv`, `field_bands.csv`, `field_grad_over.csv`, `trace.csv`,
  `trace_seam.csv`, `mask_jump_profile.csv`, `cost.csv` — Messdaten.
- `tests_before.txt` / `tests_after.txt` — Gate-Läufe.
- `iter2_circ_cd.rs.txt` — Messbinary (läuft als
  `cargo run -p metricity-lab --release --bin iter2_circ_cd -- <outdir> [probe]`).
