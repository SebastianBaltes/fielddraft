# Iteration 3 — Forschungs-Spike „offset_tree" (#384, SYNTHESIS §9)

Stand 2026-07-18. Prototyp-Code:
`crates/metricity-lab/src/bin/iter3_offset_tree.rs`
(Aufruf `cargo run -p metricity-lab --release --bin iter3_offset_tree -- a|b|all`;
Ausgaben in diesem Verzeichnis). Alle Fehlerangaben in mm.

Primärquelle der Konstruktion: J. R. Rossignac & A. A. G. Requicha,
„Offsetting operations in solid modelling", *Computer Aided Geometric
Design* 3 (1986) 129–148. Relevante Aussagen (Original gesichtet,
faculty.cc.gatech.edu/~jarek/papers/Offsets.pdf):

* §2.1: S↑\*r = {p : d(p,S) ≤ r} (Ball-Dilation); Negativ-Offset über
  Komplement-Dualität S↓\*r = c\*((c\*S)↑\*r).
* §2.4 (Boolean-Identitäten, wörtlich):
  (A ∪\* B)↑\*r = (A↑\*r) ∪\* (B↑\*r) und
  (A ∩\* B)↓\*r = (A↓\*r) ∩\* (B↓\*r) sind **Gleichheiten**;
  (A −\* B)↓\*r = (A↓\*r) −\* (B↑\*r) (Subtrahend bekommt den
  **entgegengesetzten** Offset);
  (A ∩\* B)↑\*r ⊂ (A↑\*r) ∩\* (B↑\*r) und
  (A ∪\* B)↓\*r ⊃ (A↓\*r) ∪\* (B↓\*r) sind **echte Inklusionen** —
  exakt das 2×2-Ergebnis aus Exp 3.
* §2.6: ∂(S↑\*r) ⊂ n-Offset(∂S) = n-Offsets der Flächen ∪
  **Kanal-Flächen** (Kreis r im Normalenschnitt, entlang der singulären
  Kurven gesweept) ∪ Kugeln um singuläre Punkte. Ausdrücklich nur ein
  **Superset** der Randfläche — die Trimmung leistet die
  Punkt-Klassifikation (bei uns: die implizite min/max-Komposition).
* §3.1: Kanten werden als **PCC** (piecewise constant curvature =
  Arc-Splines) approximiert, die Kanal-Flächen als G1-gefügte
  Torus-/Zylinderstücke — die „1D-Ökonomie" von §9 ist wörtlich die
  R&R-Repräsentationsentscheidung von 1986.

Wichtige mengentheoretische Präzisierung, die im Spike tragend wird:
S↑\*r = S ∪ (getrimmte Flächen-Sweeps) ∪ (Kanten-Kanäle) ∪
(Ecken-Kugeln) ist als **Mengen-Identität für jedes r exakt** (jeder
Punkt mit d(p,S) ≤ r erreicht sein Minimum auf Fläche, Kante oder
Ecke). Nur die *Rand*-Beschreibung braucht Trimmung. Daraus folgt
unten sowohl die exakte Gating-Konstruktion (B) als auch die
Entwarnung beim Topologiewechsel (C).

---

## Teil A — Sind die Extrude/Revolve-Blätter heute echte SDFs?

### Methode

Fünf Szenen direkt gegen den echten Kernel (`CsgNode::Extrude` /
`CsgNode::Revolve`, `EvalMode::Visual`), gemessen mit dem
Standard-Runner der Messbank (GT-Gitter 96, 6000 uniform + 6000
oberflächennah, Seed 4242, Bench 200k Evals):

| Szene | Konstruktion | Hypothese aus §9 |
|---|---|---|
| `iter3_extrude_path_arcs` | Extrude eines EINZELNEN geschlossenen `Path` (konvexer Bogen r=15, konkave Kerbe r=5, scharfe Ecken), h=±15 | exakt |
| `iter3_extrude_bool_profile` | Extrude von `Sketch2d::Difference`(Rechteck, Randkreis) | 2D-Boolean-Seams liften nach 3D |
| `iter3_extrude_offset_profile` | Extrude von `Sketch2d::Offset`(L-Polygon, +4) | 2D-f−δ-Fehlerklasse liftet |
| `iter3_revolve_ring_arcs` | Revolve eines achsenFERNEN Pfad-Profils (u ∈ [18,32]) | exakt |
| `iter3_revolve_solid_axis` | Revolve eines achsBERÜHRENDEN Rechtecks (u ∈ [0,30]) | Achsen-Artefakt |

Zusätzlich (a) 2D-Messung derselben Profile direkt auf
`Sketch2d::sdf`-Ebene gegen eine 2D-Ray-Wahrheit (512 Richtungen,
geführter 1-Lipschitz-March + Bisektion) — isoliert den 2D-Anteil vom
3D-Lift; (b) für das achsberührende Revolve eine analytische
Zylinder-Wahrheit (`a5_axis_scan.csv`, `a5_analytic_bands.csv`).

### Ergebnisse (3D, GT-Bänder, max |f − d_wahr| in mm; `a_summary.csv`)

| Szene | near_out | near_in | mid_out | mid_in | far_out | far_in | ∇-dev p95 | tape-Diff |
|---|---|---|---|---|---|---|---|---|
| extrude_path_arcs | **0.00027** | 0.00001 | 0.0014 | 0.00001 | 0.0074 | 0.00001 | 3.0e-5 | < 5e-7 |
| extrude_bool_profile | **0.236** | 0.000005 | **2.131** | 0.000004 | **5.800** | 0.000004 | 2.3e-5 | < 5e-7 |
| extrude_offset_profile | 0.00009 | **0.038** | 0.0007 | **0.467** | 0.0029 | **1.534** | 2.8e-5 | < 5e-7 |
| revolve_ring_arcs | **0.000006** | 0.000007 | 0.000006 | 0.000006 | 0.000015 | 0.000005 | 7.3e-5 | < 5e-7 |
| revolve_solid_axis | 0.00006 | 0.000006 | 0.0015 | **2.677** | 0.0036 | **17.578** | 2.7e-5 | < 5e-7 |

(Die 1e-3..7e-3-Werte bei `extrude_path_arcs` mid/far_out sind der
dokumentierte GT-Boden an Ecken-Tälern der Kompass-Suche, ≤ 2.4e-4 der
Modellgröße — nicht das Feld.)

2D-Isolation derselben Profile (`a_2d_profile_errors.csv`, Ray-Wahrheit
512 Richtungen, |f| ≤ 8-Band, 1500 Punkte je Profil):

| Profil | innen mean/max | außen mean/max | Befund |
|---|---|---|---|
| path_slot_notch | 0.00001 / 0.0003 | 0.001 / 0.037¹ | **exakt** (f32-Rauschen) |
| bool_rect_minus_circle | 0.00001 / 0.0003 | **0.146 / 5.378** | Rarefaction-Fan hinter den einspringenden Biss-Ecken; f misst zur Phantom-Kreisfläche |
| offset_l_plus4 | **0.030 / 1.662** | 0.00002 / 0.0004 | f−δ-Ridge: innen an der Medialachse der einspringenden Offset-Ecke (4,4) meldet f zu wenig Tiefe |

¹ Ray-Wahrheits-Artefakt, nicht das Feld: an ecknächsten Außenpunkten
wächst der Richtungs-Diskretisierungsfehler der 512-Strahlen-Suche
linear (t(θ)-Knick am Vertex, err ≈ d·Δθ/2·tanψ ≈ 0.04 bei d≈6.4);
an glatt-nächsten Punkten ist er zweiter Ordnung. Verifiziert: die
Max-Fehler-Punkte liegen diagonal hinter den −30/±15-Ecken, und f dort
ist exakt hypot zur Ecke.

Achsberührendes Revolve, analytische Zylinder-Wahrheit
(`a5_axis_scan.csv`, `a5_analytic_bands.csv`): **f(Achse) = 0.000000
bei 20 mm wahrer Innentiefe** — die Achse ist eine Phantom-Nullmenge;
entlang der Radialen gilt innen f = −r bis r ≈ 10 (dort übernimmt die
echte Wand), außen ist ALLES exakt (mean 1e-6, max 6e-6 über 30k
Punkte); near_in ist bis auf Achsnähe exakt, max-Schaden far_in
19.130 mm.

### Verdikt A

1. **Extrude eines einzelnen geschlossenen Pfads ist heute schon ein
   exaktes SDF** — auch mit konvexen UND konkaven Bögen und scharfen
   Ecken, innen wie außen, inklusive Deckelkanten und -ecken (iq-
   opExtrusion-Kompositionsformel + exaktes Pfad-SDF). Kein Handlungs-
   bedarf.
2. **Revolve eines achsenfernen Profils ist exakt** (max 1.5e-5 über
   alle Bänder!) — die §9-Behauptung „3D-Distanz = 2D-Profildistanz in
   der Meridian-Halbebene" ist am echten Kernel bestätigt.
3. **Nicht exakt sind genau drei benennbare Fälle**, alle 2D-Ursachen,
   die der 3D-Lift verlustfrei konserviert:
   a) 2D-Boolean im Profil → Fan-/Phantomfehler NUR außen (innen
      bleibt max-exakt) — für `offset_tree` heißt das: Profil-Booleans
      vor dem Offsetten **geometrisch auflösen** (Clipper-Klasse),
      nicht feldweise f−δ hineinschieben;
   b) `Sketch2d::Offset` (f−δ) → Ridge-Fehler NUR innen (Nullmenge und
      Außenraum exakt!) — als Geometrie-Definition der Dilatation
      brauchbar, als Distanzquelle innen nicht;
   c) achsberührendes Revolve → Phantom-Nullmenge auf der Achse +
      f = −r im Achsbereich. Für `offset_tree` irrelevant (Offset
      liest das Nahfeld, das exakt ist), aber für Wandstärke/innere
      Distanzen ein realer, bisher undokumentierter Kernel-Defekt.
4. **Tape-Parität überall < 5e-7 mm** — der kompilierte Render-Pfad
   (CPU-Referenz `eval_opcodes`) evaluiert Pfad-Profile exakt wie das
   Visual-Feld; keine Bogen-Flattening-Divergenz (die 24-Sample-
   Flattening betrifft nur den Even-Odd-Sign/GPU-Poly-Fallback, nicht
   die CPU-Distanz).

### Theorie-Notiz (warum die Hypothese so lautet)

* Ein einzelner geschlossener `Path` liefert per Konstruktion ein
  exaktes 2D-SDF (exaktes analytisches Distanz-Minimum über alle
  Segmente + exakter Even-Odd-Sign, `sdf_path`/`path_is_inside` in
  `crates/protocol/src/sketch2d.rs`). Die Extrude-Formel des Kernels
  (`sdf_extrude`, iq `opExtrusion`) ist für ein exaktes d₂ beweisbar
  das exakte 3D-SDF der Prisma-Menge (alle vier Vorzeichen-Fälle).
* Revolve: `sdf([√(x²+z²), y])` — für Profile in u > 0 exakt, weil der
  nächste Punkt eines Rotationskörpers in der Meridian-Halbebene des
  Abfragepunkts liegt und die 3D-Distanz zum Breitenkreis (u,v) genau
  hypot(r−u, y−v) ist.
* Achsberührende Profile haben eine **Phantom-Randkante auf der
  Achse**: das 2D-Profil-SDF misst Distanz auch zum Achsen-Segment des
  Profilrands, das im 3D-Körper gar keine Fläche erzeugt. Erwartung:
  f ≈ −r nahe der Achse statt der wahren Innentiefe.
* 2D-Booleans/2D-Offset im Profil tragen exakt die bekannten
  Fehlerklassen aus §1 (Naht-Fußpunktfehler innen, Rarefaction-Fan
  außen, f−δ-Ridge) — die Extrude-Formel konserviert sie.

---

## Teil B — Kanal-Korrektur an einer gekrümmten Schnittkante

### Aufbau

„Offset nach außen über Schnitt": S = A ∩ B mit A = Zylinder r₁=30
entlang z (|z| ≤ 50), B = Zylinder r₂=20 entlang x (|x| ≤ 50) — beide
konvex, S konvex, Schnittkante = zwei Steinmetz-Loops
(x = ±√(r₁²−r₂²cos²θ), y = r₂cosθ, z = r₂sinθ; Kurvenlänge ≈ 130 mm je
Loop, nicht eben, nicht closed-form-distanzierbar). Kappen liegen fern
(|x|,|z| ≤ 30 < 50), ∂S besteht nur aus den zwei Mantel-Stücken + den
zwei Kanten-Loops.

**Ground Truth ist hier exakt** (nicht Brute-Force): für konvexes S
ist SDF(S↑δ) = SDF(S) − δ (Beweis elementar über Stützpunkte), innen
ist max(f_A, f_B) exakt (§1), außen gilt die Projektions-Identität

    dist(x,S) = min( [proj_A(x)∈B]·d_A, [proj_B(x)∈A]·d_B, dist(x,Kante) )

mit closed-form-Projektionen auf die Zylinder und einer dichten
Referenz-Polyline (200 000 Segmente je Loop) für den Kantenterm.
Validierung dreifach: gegen Dykstra-Projektion auf A ∩ B (unabhängiger
konvexer Projektionsalgorithmus) und gegen die Ball-Dilations-
Vorzeichenprobe aus dem Auftrag (`b_validation.csv`).

### Varianten

* **naiv**: max(f_A, f_B) − δ — Schnitt der offsetteten Blätter,
  scharfe Kante. Vorhersage (Halbraum-Modell): Überschuss-Keil bis
  δ·(1/cos(γ/2) − 1), mit γ = Normalenwinkel an der Kante; hier
  γ ∈ [48.2°, 90°] ⇒ max. Überschuss 0.414·δ (bei θ=90°, y=0).
* **C1**: naiv ∖ Keil mit Keil = {f_A>0 ∧ f_B>0 ∧ d_Kante>δ} — §9
  wörtlich („naiv ∪/∩ Kanal-Körper"), Gating über die
  Original-Körper. **Vorhersage: strukturell falsch** bei nicht
  orthogonalem Flächenwinkel: {außerhalb A ∧ außerhalb B} ist ein
  ECHTES Superset des Normalkegels der Kante; der Keil schneidet
  Material weg, dessen Fußpunkt auf einer Fläche liegt. Fehlertiefe
  im Halbraum-Modell δ·(1−sin γ) — hier bis 0.255·δ (bei θ=0, y=±20).
* **C2**: naiv ∖ Keil mit **Normalkegel-Gating**: am nächsten
  Kurvenpunkt q\* werden die analytischen Normalen n_A, n_B
  ausgewertet und der Keil über die dualen Kegelvektoren
  m_A ∝ n_A − (n_A·n_B)·n_B, m_B symmetrisch getrimmt:
  Keil = {⟨x−q\*, m̂_A⟩>0 ∧ ⟨x−q\*, m̂_B⟩>0 ∧ |x−q\*|>δ}.
  Das ist die korrekt getrimmte R&R-Konstruktion in Feldform; für
  konvexes S beweisbar exakt bis auf den Kurven-Approximationsfehler
  (x−q ∈ Normalkegel(q) ⇔ q = proj_S(x), Optimalitätsbedingung der
  konvexen Projektion). Alle Gating-Daten leben AUF der Kurve —
  „1D-Ökonomie" im Sinn von §9/R&R §3.1.

Kurven-Approximationen: uniforme θ-Polyline (N Segmente je Loop,
Chunk-Bounding-Spheres fürs Pruning) und Arc-Spline (N Bögen je Loop
durch Stützpunkt-Tripel, exakte closed-form Distanz zum Raumbogen).

### Metriken

* **Zero-Set-Fehler**: 4000 Strahlen (3000 kantennah + 1000 global)
  treffen die wahre Offset-Fläche (geführter March auf dem exakten
  1-Lipschitz-GT-Feld); je Variante wird die Varianten-Nullstelle im
  Fenster ±(1.2δ+0.5) um den wahren Treffer bisektiert und dort
  |f_true| ausgewertet — weil f_true ein exaktes SDF ist, ist das die
  exakte geometrische Abweichung der Varianten-Fläche
  (richtungsunabhängig). `misses` zählt Strahlen ohne
  Varianten-Nullstelle im Fenster.
* **Nahfeld**: |f − f_true| auf 4000 Punkten im Band Kante ± 0.5δ um
  den Kanal.
* **Kosten**: ns/Eval auf Band-Punkten (`b_costs.csv`).

### Ergebnisse

**GT-Validierung** (`b_validation.csv`): Projektions-Identität vs.
Dykstra max |Δ| = **2.96e-9 mm** über 20k Punkte (Dykstra ⌀ 7.7, max 72
Iterationen); Ball-Dilations-Vorzeichenprobe (1000 Richtungen × 4
Shells): 9/3000 Abweichungen, ALLE bei |f_true| ≤ 0.033 mm — reine
Sampling-Auflösung der Ballprobe, kein GT-Fehler. Die Ground Truth ist
damit auf ~1e-9 belastbar (f64-Labor).

**Kurven-Approximation** (`b_curve_approx.csv`, max. Abweichung von der
wahren Kurve, mm; N je Loop, Kurvenlänge ≈ 130 mm je Loop):

| N | Polyline | Arc-Spline |
|---|---|---|
| 16 | 0.485 | 0.0125 |
| 32 | 0.127 | 0.00182 |
| 64 | 0.0322 | 0.000232 |
| 128 | 0.00807 | 0.000029 |
| 256 | 0.00202 | 0.000004 |
| 512 | 0.000505 | — |
| 1024 | 0.000126 | — |

Polyline konvergiert O(N⁻²) (Sagitta), Arc-Spline O(N⁻³) — für die
0.001-mm-Klasse braucht die Polyline ~512 Segmente je Loop, der
Arc-Spline ~32–64 Bögen: **~8× weniger Elemente**, R&Rs PCC-
Entscheidung (§3.1) quantitativ bestätigt.

**Zero-Set-Fehler der Varianten** (δ=2, ~3370 auswertbare Strahlen;
Fehler = |f_true| am Varianten-Oberflächenpunkt, exakt weil f_true
exaktes SDF; `b_zeroset.csv` — δ=0.5 und δ=5 zeigen für c2 dieselben
Zahlen, die Korrektur ist **δ-unabhängig**):

| Variante | mean | p95 | max |
|---|---|---|---|
| naiv | 0.0319 | 0.222 | **0.818** (= 0.409·δ, Vorhersage 0.414·δ ✓) |
| c1_poly64 | 0.0157 | 0.089 | 0.552 |
| c1_poly1024 | 0.0122 | 0.082 | **0.543** — konvergiert NICHT (N-unabhängig) |
| c2_poly64 | 0.00263 | 0.0202 | 0.0287 |
| c2_poly128 | 0.00063 | 0.0048 | 0.0071 |
| c2_poly256 | 0.00016 | 0.0012 | **0.0018** |
| c2_poly512 | 0.00004 | 0.0003 | **0.00044** |
| c2_poly1024 | 0.00001 | 0.00008 | 0.00011 |
| c2_arc16 | 0.00056 | 0.0040 | 0.0093 |
| c2_arc32 | 0.00007 | 0.0006 | **0.0012** |
| c2_arc64 | 0.00001 | 0.00007 | **0.00014** |
| c2_arc128 | 0.000001 | 0.000009 | 0.000018 |

Der Zero-Set-Fehler von c2 ist in allen Stufen ≈ der Kurven-
Approximationsfehler — die Konstruktion selbst fügt nichts hinzu.
Die Maxima passen quantitativ zu den Flachmodell-Vorhersagen und
lokalisieren sich darüber: naiv 0.409·δ gemessen vs. 0.414·δ
vorhergesagt (γ=90°-Zone, θ=±90°); c1 0.27·δ gemessen vs.
δ(1−sin 48.2°)=0.255·δ vorhergesagt (y=±20-Zone) plus Krümmungsanteil.

Geltungsbereich: Der C2-Exaktheitsbeweis (Kegel-Test ⇔ Projektion)
nutzt die Konvexität von S. Für nichtkonvexe Körper bleibt C2 eine
lokal korrekte Konstruktion; an Stellen, wo sich Normalkegel
verschiedener Kanten überlappen (großes δ, enge Features), ist das
Restrisiko per Witness-Punkt + Ball-Extremum-Orakel zu prüfen
(→ Teil C).

**Nahfeld-Metrik** (Band Kante ± 0.5δ, δ=2; Split außen/innen via
Debug-Probe `bdbg`, c2_poly512):

* **Außen (March-Pfad): sauber.** mean 0.0024, p95 0.014, max 0.095,
  Bias −0.0023 (unterschätzend/konservativ); maximale ÜBERschätzung
  +0.00044 mm = exakt die Kurventoleranz ⇒ sphere-trace-sicher bis
  auf Kurventoleranz.
* **Innen: degradiert.** mean 0.49, max **3.25 mm** (bei 4.4 mm wahrer
  Tiefe): die Keil-Subtraktion stellt innen die klassische
  CSG-Differenz-Fußpunktfehlerklasse wieder her — das Feld misst zur
  Phantom-Kegelwand des abgezogenen Keils. Nullmenge und Vorzeichen
  bleiben exakt (alle Ausreißer-Punkte haben f_c2 < 0 wie f_true < 0).
  Das ist KEIN Konstruktionsfehler, sondern §9s eigene Konzession
  („der expandierte Baum ist wieder CSG") — jetzt mit Zahl.

**Kosten** (`b_costs.csv`, Band-Punkte, brute + Chunk-Pruning,
f64-Forschungscode):

| Feld | ns/Eval |
|---|---|
| naiv (2 Zylinder) | 8.5 |
| c2_arc16 (32 Bögen) | 288 |
| c2_arc64 (128 Bögen) | 718 |
| c2_poly256 (512 Segmente) | 977 |
| c2_poly1024 (2048 Segmente) | 2732 |

Einordnung: heutige Kernel-Tapes kosten 30–670 ns/Eval (HARNESS §7,
CPU); ein Kanal-Körper der 0.001-mm-Klasse (c2_arc64) liegt mit
~700 ns in der Klasse eines mittleren Tapes. Der Term ist zudem
räumlich lokal (Kanal-AABB) — mit Narrow-Band-Gating fällt er im
Fernfeld auf einen Chunk-Lower-Bound-Test zurück; die gemessenen
Zahlen sind die Worst-Case-Kosten IM Band.

### Zielerreichung Teil B

| Ziel | erreicht mit | Kosten |
|---|---|---|
| ≤ 0.01 mm | c2_arc16 (32 Bögen gesamt) bzw. c2_poly128 | ~290 ns/Eval |
| ≤ 0.001 mm | **c2_arc64 (128 Bögen gesamt)** bzw. c2_poly512 (1024 Segmente) | ~720 ns/Eval |

Messgrenze der Methode: ~14 % der Strahlen (identisch über alle
Varianten inkl. naiv) finden im ±(1.2δ+0.5)-Fenster keine
Varianten-Klammer — nach Stichproben tangentiale Streifschüsse
(Feld berührt 0 ohne Vorzeichenwechsel an den 240 Stützstellen);
die Statistik läuft über die verbleibenden ~3400 transversalen
Treffer. c2_poly16 verliert zusätzlich Strahlen durch die grob
verrutschte Röhre (Chord 0.48).

---

## Teil C — Topologiewechsel bei großem δ (Analyse, keine Implementierung)

### Die zentrale Entwarnung: Expansion ist für JEDES δ mengen-exakt

Die R&R-Zerlegung S↑δ = S ∪ Flächen-Sweeps ∪ Kanten-Kanäle ∪
Ecken-Kugeln ist eine **Mengen-Identität für jedes δ** — sie setzt
keinerlei Topologie-Stabilität voraus. In der impliziten Welt wird die
Vereinigung punktweise per min ausgewertet, d. h.:

* **„Bohrungen schließen sich korrekt" ist automatisch.** Wenn der
  δ-Kanal um die Bohrungskante die Bohrung überdeckt, ist das die
  richtige Menge; kein Detektor nötig, das min absorbiert es.
* **„Flächen sterben" ist für die Expansion harmlos.** Ein
  Korrekturkörper, dessen Beitrag vollständig von Nachbarn überdeckt
  wird, ist ein *toter, aber korrekter* Summand — Geometrie exakt,
  nur Eval-Kosten und Provenance verschmutzt.
* **Selbstschneidende Kanal-Spines sind kein Korrektheitsproblem:**
  der Kanal ist {dist(x, Kurve) ≤ δ} — als Menge auch bei κ·δ > 1
  wohl definiert (Vereinigung von Kugeln); nur eine
  *Flächen*-Parametrisierung des Kanals würde degenerieren, die
  brauchen wir nicht.
* Kontraktion/Shell läuft über die Komplement-Dualität (R&R §2.4) auf
  dieselbe Expansion des Komplement-Baums hinaus — dieselbe
  Mengen-Exaktheit, mit vertauschten Konvexitätsrollen.

Der eigentliche Inhalt des „Topologie-Problems" ist also NICHT
Korrektheit der Menge, sondern drei kleinere Fragen:

1. **Blatt-Parameter-Kollaps bei Kontraktion** (r−δ ≤ 0,
   Wandstärke ≤ 0, 2D-Profil verschwindet): rein parametrisch am
   Blatt erkennbar (Radius-/Dicken-Check; 2D-Profile: der
   Clipper-Offset liefert Topologiewechsel in 2D exakt und gratis).
   Billig, robust, keine Heuristik.
2. **Tote Korrekturkörper wegprunen** (Kosten/Provenance-Hygiene,
   nicht Korrektheit): Kandidat tot ⇔ sein Beitragsgebiet liegt in
   der Vereinigung der übrigen Terme. Billigster belastbarer
   Detektor: **Witness-Punkte auf dem Korrekturkörper-Rand +
   Ball-Extremum-Orakel** (das laut Exp 3 einzige beweisbar korrekte
   Punkt-Klassifikationswerkzeug auf kaputten Feldern) — einige Dutzend
   Punkte pro Kanal, je ~170 Evals. Intervall-Arithmetik auf dem
   expandierten Baum ist die schnellere Vorstufe (AABB des Kanals
   gegen garantierte Innen-Boxen der Nachbarn), entscheidet aber nur
   die klaren Fälle.
3. **Stale Adjazenz für NACHFOLGENDE Features**: wer auf S↑δ wieder
   Kanten sucht (Fillet, zweiter Offset), braucht den Weave des
   *Ergebnisses*; die alten Kantenlisten sind nach einem
   Topologiewechsel wertlos. Detektor hier = der Weave selbst — das
   ist der teure, aber ohnehin existierende Pfad. Kanal-Selbstschnitte
   und Kanal-Paar-Schnitte (Kurve-Kurve-Distanz < 2δ, 1D-BVH, billig)
   sind das richtige *Vorwarnsignal*, wann der Weave wirklich neu
   gebraucht wird.

### Ehrliche Einschätzung

* Häßlichkeitsgrad moderat, NICHT katastrophal — weil die Expansion
  selbst nie falsch wird. Der CAD-Brotfall (Shell/Offset mit
  δ ≪ Bauteilgröße) braucht Stufe 1 (parametrisch) und sonst nichts.
* Wirklich unangenehm sind kaskadierte Offsets auf Offsets
  (Baumwachstum + Kanal-auf-Kanal-Kanten, deren Spines schon
  Approximationen sind — R&R nennen exakt dieses Problem als Grund
  ihrer PCC-Entscheidung) und die Provenance-/Sub-Face-Zuordnung
  toter Flächen im Feature-UI.
* Der Fußpunkt bleibt: Twist/Bend-Blätter sind ausgenommen (keine
  Isometrie, exakter Offset existiert dort prinzipiell nicht) — wie
  in §9 bereits festgehalten.

---

## Negativbefunde / Gotchas dieses Spikes

1. **§9s Kanal-Rezept ist wörtlich genommen falsch.** „naiv ∖
   (Komplement A ∩ Komplement B ∩ Komplement Röhre)" (C1) konvergiert
   NICHT gegen die Wahrheit: {außerhalb A ∧ außerhalb B} ist bei
   nicht-orthogonalem Flächenwinkel ein echtes Superset des
   Normalkegels der Kante; der Keil frisst Material, dessen
   Distanz-Fußpunkt auf einer FLÄCHE liegt. Restfehler N-unabhängig
   ≈ δ·(1−sin γ) + Krümmungsanteil (gemessen 0.27·δ bei γ_min=48°).
   Die Korrektur MUSS über den Normalkegel am nächsten Kurvenpunkt
   getrimmt werden (C2) — das bleibt 1D-Ökonomie (Position + zwei
   Normalen je Stützstelle), ist aber ein anderer Körper als
   „Boolean aus A, B, Röhre".
2. **Die Keil-Subtraktion degradiert den INNEN-Betrag lokal** (max
   3.25 mm bei δ=2 im Kanal-Band) — Standard-CSG-Innen-
   Fußpunktfehler am Phantom-Kegel. Nullmenge exakt, Außen/March
   sicher. Wer innen Beträge braucht (Wandstärke), braucht weiter
   Hopf–Lax/Proxy — konsistent mit der §9-Doktrin „Feld ist kein
   Messgerät".
3. **`Sketch2d::Offset` ist f−δ** (`sketch2d.rs`, `Offset`-Arm) — als
   2D-„gelöstes Offsetting" im Sinne von §9 taugt es NUR für die
   Nullmenge/Außenraum; ein echter `offset_tree` sollte 2D-Profile
   geometrisch offsetten (Clipper-Klasse), sonst wird die
   2D-Innenmetrik ins 3D geliftet (gemessen: bis 1.53 mm far_in am
   Extrude).
4. **Achsberührende Revolve-Profile haben eine Phantom-Nullmenge auf
   der Achse** (f(Achse) = 0 bei 20 mm Innentiefe) — bisher
   undokumentierter Kernel-Befund; harmlos fürs Rendern (tief innen),
   giftig für jede |f|-Abfrage. Merkposten für die
   Blatt-Offset-Regeln: Achsen-Segmente des Profils erzeugen keine
   Fläche und dürfen beim Offsetten des Profils nicht mit-offsettet
   werden.
5. **Messmethodik:** (a) Die 2D-Ray-Wahrheit hat an ecknächsten
   Punkten einen LINEAREN Richtungs-Diskretisierungsfehler
   (≈ d·Δθ/2·tanψ, gemessen 0.037 bei 512 Richtungen) — an glatten
   Fußpunkten zweiter Ordnung; für Exaktheits-Zertifikate unter
   1e-3 ungeeignet, für 10⁻¹-Klasse-Fehler völlig ausreichend.
   (b) Dykstra liefert für INNEN-Punkte 0, die Projektions-Identität
   den signierten Wert — Validierungsvergleich nur außen (war
   zunächst als 19.6-mm-„Diskrepanz" fehlinterpretiert).
   (c) Shell-Sortierung von CSVs auf dieser Maschine: LC_ALL=C
   setzen, deutsches Locale zerlegt Dezimalpunkte.

## Quellen

* J. R. Rossignac, A. A. G. Requicha: *Offsetting operations in solid
  modelling.* Computer Aided Geometric Design 3 (1986) 129–148.
  (§2.1 Definitionen, §2.4 Boolean-Identitäten, §2.5 R_r/F_r-Rundung,
  §2.6 n-Offsets/Kanal-Flächen als Rand-Superset, §3.1 CSGO + PCC-
  Arc-Spline-Approximation.)
* I. Quilez: *distfunctions* (opExtrusion) — Grundlage von
  `sdf_extrude`.
* SYNTHESIS.md §1 (Fehler-Taxonomie), §5/Exp 3 (2×2-Pushdown,
  Ball-Extremum), §9 (Arbeitsprogramm).
