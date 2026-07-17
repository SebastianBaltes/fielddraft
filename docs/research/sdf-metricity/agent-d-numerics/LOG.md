# Agent D (Numerik) — Forschungslog

Richtung: Feld als Funktion. Gradientennorm-Korrekturen, Normalisierungs-
operatoren, Fehlerfortpflanzung über Verschachtelungstiefe, Praktikabilität
für die Tape-Pipeline (Bytecode/WGSL).

Alle Experimente in 2D (Kreise): die untersuchten Formeln sind dimensions-
unabhängig, Boolean-Nähte sind lokal 2D (Codim-2-Kanten im 3D ↔ Ecken im 2D).
Ground Truth analytisch (getrimmte Bögen + Ecken) bzw. via 4·10⁵-fach
bisektierter Randabtastung + KD-Tree (Chord-Fehler ≪ 10⁻⁸). Skripte: `lib.py`,
`exp1_smooth_corrections.py`, `exp2_hard_seam.py`, `exp2b_boolean_gaps.py`.

---

## Iteration 1 — Gradientennorm-Korrekturen vs. die zwei Fehlerklassen

### Quellen

- P. D. Sampson (1982): Fitting conic sections to “very scattered” data.
  CGIP 18 — Ursprung der first-order-Distanz f/|∇f|.
- G. Taubin (1991): Estimation of planar curves, surfaces, and nonplanar
  space curves defined by implicit equations. IEEE TPAMI 13(11) —
  d₁ = |f|/|∇f|; (1994) Distance approximations for rasterizing implicit
  curves, ACM TOG 13(1) — Distanz k-ter Ordnung als Wurzel eines Polynoms
  in den Ableitungen.
- A. Biswas, V. Shapiro (2004): Approximate distance fields with
  non-vanishing gradients. Graphical Models 66(3) — Normalisierungsordnung m
  als lokales Qualitätsmaß eines ADF nahe des Randes.
- V. L. Rvachev (1963 ff., R-Funktionen); V. Shapiro (2007): Semi-analytic
  geometry with R-functions. Acta Numerica — Normalisierung 1. Ordnung
  w/|∇w|, R-Funktionen erhalten Normalisierungsordnung.
- A. Belyaev, P.-A. Fayolle (2013): Signed Lp-distance fields. CAD 45(2) —
  glatte Distanz-Approximanten mit steuerbarer Normalisierungsordnung.

### Hypothesen

- H1: f/|∇f| (Sampson) korrigiert Skalen-/Normfehler (|∇f| ≠ 1) auf
  1. Ordnung; Fehler O(d²·κ) nahe glatter Flächen. Taubin 2. Ordnung → O(d³).
- H2: An harten min/max-Nähten ist |∇f| = 1 fast überall → JEDE
  gradientennorm-basierte Korrektur lässt den Boolean-Fehler vollständig
  stehen (Fußpunkt-Fehler ≠ Skalenfehler).
- H3 (naiv, VOR der Messung): min-Union unterschätzt außen im Schattenkegel
  konkaver Ecken um 1 − cos(γ/2).

### Aufbau

E1: (A) algebraischer Kreis f = (r²−R²)/(2R); (B) skaliertes SDF (r−R)/√2
(Modell des unnormalisierten smin-Visual-Pfads); (C) smin_poly-Union zweier
fast-tangentialer Kreise (gap 0.02, r_blend 0.25), GT via Randabtastung.
Verglichen: f roh, Sampson (FD-Gradient h=1e-5), Taubin 2. Ordnung (Quadratik
entlang der Normalen), iterierte Normalisierung k=2,3 (geschachtelte FD).

E2/E2b: harte Booleans zweier Kreise R=1: Union außen, Union innen (Tiefe),
Intersection außen (Linse), Difference außen (Horn/Sichel). Punkte im
Normalkegel der Ecke, ρ ∈ [10⁻³, 0.3].

### Ergebnisse (Zahlen)

E1-A (algebraischer Kreis, Fehler entlang Normalstrahl, log-log-Ordnung):

    f roh:    err(d=0.01)=5.0e-5, err(d=0.3)=4.5e-2, Ordnung ≈ 2.00
    Sampson:  err(d=0.01)=5.0e-5, err(d=0.3)=3.5e-2, Ordnung ≈ 1.96
    Taubin2:  err(d=0.01)=2.6e-13, err(d=0.3)=7.8e-10, Ordnung ≈ 2.05 (!)
    iter2:    wie Sampson (keine Verbesserung)

  Überraschung: beim algebraischen Kreis ist f selbst schon 2.-Ordnung-genau
  (f = d + d²/2R), Sampson bringt dort NICHTS Zusätzliches (gleiche Ordnung,
  ähnliche Konstante). Taubin2 ist beim Kreis fast exakt (die Quadratik
  entlang der Normalen IST die Geometrie).

E1-B (globaler Skalenfehler 1/√2): Sampson exakt bis Maschinengenauigkeit
  (3e-13). Erwartbar, aber wichtig: der √2-Faktor des smin-Feldes ist ein
  reiner Normfehler → Wurzel-Normalisierung genügt VOLLSTÄNDIG für diesen
  Fehleranteil.

E1-C (fast-tangentiale smin-Union, Außen-Nahfeld, max|err| pro |d|-Band):

    Band          f roh     Sampson   Taubin2   iter2     iter3
    [0.001,0.02)  8.4e-3    3.3e-4    6.7e-5    3.2e-4    3.3e-4
    [0.02,0.05)   2.1e-2    2.1e-3    1.1e-3    2.1e-3    3.6e-3
    [0.05,0.1)    3.4e-2    5.4e-3    3.1e-3    6.6e-3    1.2e-2

  → Sampson: 10–25× Fehlerreduktion im Nahfeld; Taubin2 nochmal ~5× am Rand;
  iterierte Normalisierung k≥2 bringt gegenüber k=1 NICHTS (k=3 wird durch
  geschachtelte FD-Ableitungen und Fernfeld-Divergenz schlechter). Bemerkung:
  am Neck ist |∇f| ≈ 0.41–0.87 < 1 (fast-ANTIparallele Zweig-Gradienten),
  nicht √2 — die √2-Zone liegt bei fast-KOINZIDENTEN Flächen (→ Iteration 2).

E2 (min-Union außen): H3 WIDERLEGT — gemessener max. Relativfehler 0.0000
  über alle Überlapptiefen. Grund: dist(x, A∪B) = min(dist(x,A), dist(x,B))
  ist für Außenpunkte eine exakte Mengen-Identität. Der Union-Fehler sitzt
  NUR innen.

E2b-1 (Intersection außen, Schattenkegel der Linsen-Ecke):

    cx     γ [°]    pred 1−cos(γ/2)   gemessen max
    0.30   34.9     0.0461            0.0456
    0.50   60.0     0.1340            0.1337
    0.70   88.9     0.2859            0.2841
    0.90  128.3     0.5641            0.5597
    0.98  157.0     0.8010            0.7979

  → Modell f = ρ·cos(γ/2) auf der Kegel-Bisektrix bestätigt auf <1%.
  Fehler ist LINEAR in ρ (Relativfehler konstant): er verschwindet NICHT
  nahe der Fläche — er ist im ganzen Kegel proportional präsent.

E2b-2 (Union innen, cx=0.7): max. relative Tiefen-Unterschätzung 58%;
  Shell-Innenfläche {f=−t}: wahre Tiefe statt t:

    t=0.1 → max 0.148 (+48% Wanddicke am Neck)
    t=0.2 → max 0.326 (+63%)
    t=0.3 → max 0.691 (+130%)

  → Shells über Boolean-Nähten werden systematisch ZU DICK (Material
  kriecht in den Neck). Erosion kommutiert nicht mit Union.

E2b-3 (Difference, Horn-Vorfeld): f ≈ cos(γ/2)·d_true bestätigt (<3% Abw.).
  Überraschung: flache Anschnitte (externe Fast-Tangenz) sind HARMLOS
  (γ klein). Der Adversar ist die dünnwandige Sichel (interne Fast-Tangenz):

    Wanddicke 0.50 → γ=165.6° → f/d = 0.106
    Wanddicke 0.25 → γ=173.2° → f/d = 0.050
    Wanddicke 0.19 → γ=174.2° → f/d = 0.046

  → Feld kollabiert im Horn-Vorfeld auf wenige % der wahren Distanz
  („Grey-Wall“-Mechanismus für harte Differenzen; march_relax hilft hier
  nicht mehr, es ist kein Überschreitungs- sondern ein Kriech-Problem).

E2-FD (Tie-Flächen-Statistik, Union cx=0.7, Punkte am Naht-Band):

    h_FD    min|g_fd|   max (f/|g_fd|)/d_true
    1e-6    1.0000      1.000
    1e-4    0.716       1.397
    1e-2    0.715       1.399

  → Zentrale Differenzen mitteln am Naht-Band beide Äste: |g|=cos(γ/2)<1,
  f/|g| ÜBERSCHREITET die wahre Distanz um bis zu 1/cos(γ/2) — obwohl f
  dort exakt ist. FD-Sampson ist als Sphere-Trace-Schrittweite an JEDER
  min/max-Naht unsicher; Forward-AD (Winner-Branch, wie eval) zeigt das
  Problem nicht (|g|=1 a.e.).

### Schlussfolgerung Iteration 1

Der Feldfehler zerfällt in zwei fundamental verschiedene Klassen:

1. NORM-Fehler (|∇f| ≠ 1): smin-Zonen, Skalierung, algebraische Felder.
   → Sampson f/|∇f| korrigiert 1. Ordnung (10–25×), Taubin2 weitere ~5×,
   Iteration k≥2 lohnt nicht.
2. FUSSPUNKT-Fehler (|∇f| = 1, aber Fußpunkt liegt auf weggeschnittenem
   Rand): Intersection/Difference außen (Schattenkegel, rel. Fehler
   1−cos(γ/2), im Sichel-Fall >95%), Union innen (Shell +130% Wanddicke).
   → KEINE gradientennorm-basierte Methode kann das sehen, denn lokal ist
   das Feld ein perfektes SDF der falschen (ungetrimmten) Fläche.
   Normalisierungsordnung à la Biswas/Shapiro ist hier die falsche Brille:
   sie misst Treue am Rand, der Fehler sitzt im Fernfeld des Kegels.

---

## Iteration 2 — smin-Exaktheit, Offsets, Fehlerfortpflanzung über Tiefe

Skripte: `exp3_smin_exactness_offsets.py`, `exp4_depth.py`.

### Hypothesen

- H4: smin_poly ist zur EIGENEN Nullfläche außen exakt bis auf die Blend-Zone;
  Überschätzung (Sphere-Trace-Gefahr) tritt nur bei fast-parallelen
  Zweig-Gradienten auf, Grenzfall koinzidenter Flächen mit Faktor √2.
- H5: Über Verschachtelungstiefe N kombiniert der smin-Fehler in QUADRATUR
  (die blend_min-Gewichte (w_a,w_b) sind ein Einheitsvektor), nicht
  multiplikativ: worst case Θ(√N), nicht (√2)^N.

### Ergebnisse E3 (smin-Union cx=0.6, r=0.25, GT zur eigenen Nullfläche)

Fehlerkarte f − d_true: außen min −4.9e-6 — bei cx=0.6 füllt der Fillet den
Neck, praktisch KEINE Unterschätzung außen (die −3.4e-2 aus E1-C gehören zur
fast-tangentialen Konfiguration, wo die Blend-Zone den Neck nicht abdeckt);
**max +4.3e-2 ÜBERSCHÄTZUNG, f/d bis 1.209**. Zone: äußerer Rand der Blend-Zone auf der
Tie-Achse (d1≈d2≈r, Fillet weit weg). Sweep über Konfigurationen:

    cx=0.95 (flacher Schnitt):  max f/d = 1.00–1.0013
    cx=0.8:                     max f/d = 1.00–1.12
    cx=0.6:                     max f/d = 1.17–1.26
    cx=0.3 (tiefer Überlapp):   max f/d = 1.356–1.376  → Richtung √2

  → Die Überschätzung wächst mit der Gradienten-Parallelität der Zweige;
  der koinzidente Grenzfall ist √2. **MASKED_BLEND_MARCH_RELAX = 0.75 ist
  im Grenzfall zu lax (0.75·√2 = 1.06 > 1)**; sicher wäre 1/√2 ≈ 0.707
  für EINEN smin.

Offset-Isokonturen {F=δ}, wahre Distanz darauf (soll δ):

    δ       roh [min,max]        Wurzel-Sampson [min,max]
    0.05    [0.0423, 0.0500]     [0.0489, 0.0504]    (Fehler −15% → −2.2%)
    0.1     [0.0841, 0.1000]     [0.0949, 0.1015]
    0.2     [0.1663, 0.2000]     [0.1749, 0.2052]
    0.4     [0.3751, 0.4000]     [0.3294, 0.4000]    (Sampson SCHLECHTER!)
    −0.1    [−0.1000, −0.0867]   [−0.1020, −0.0980]

  → Wurzel-Sampson verbessert NAHE Offsets deutlich (Neck-Unterschätzung ist
  Normfehler, |∇f|<1 dort), verschlechtert aber FERNE Offsets (die Korrektur
  gehört in ein Vertrauensband |f| ≲ r_blend, außerhalb ausblenden).

### Ergebnisse E4 (Tiefe)

P1 — koinzidenter Stack, m identische Kreise, r=0.3 (Vorhersage √m):

    m    pred √m   max|∇f|   max f/d   Sampson-Restfehler
    2    1.414     1.414     1.414     8.8e-2
    4    2.000     2.000     2.000     1.5e-1
    8    2.828     2.828     2.828     1.9e-1
    16   4.000     4.000     4.000     2.25e-1

  → **√m-Gesetz exakt bestätigt** (Lipschitz UND Überschätzungsfaktor).
  Überraschung: Sampson-Restfehler ≠ 0 — außerhalb der Blend-Zone ist das
  Feld das SDF des UNaufgeblähten Kreises (|∇f|=1, konstanter Offsetfehler
  d* = r(1−1/√m), für m=16: 0.225 ✓ gemessen 2.25e-1). Der koinzidente
  Stack erzeugt also BEIDE Fehlerklassen: Normfehler innen (Sampson fixt),
  Fußpunktfehler außen (Sampson blind).

P2 — gestaffelt-adversarial (jeder neue Zweig trifft den akkumulierten Wert):
  Rekursion L_k = (L_{k-1}+1)/√2, Fixpunkt √2+1 ≈ 2.414. Gemessen:
  1.414, 1.707, 1.914, … 2.383 (Tiefe 11) → bestätigt, BESCHRÄNKT.

P3 — realistische Kette (N Kreise, Abstand 1.2, r=0.25):
  max|∇f| = 1.237/1.239/1.236 für N=2/4/8 → **keinerlei Akkumulation**,
  Blend-Zonen sind disjunkt, der Gewinner-Zweig lokalisiert.

P4 — 40 Zufallskonfigurationen je N:
  N=2: median 1.13, max 1.41; N=4: median 1.42, max 1.83;
  N=8: median 1.54, max 2.02 (√8=2.83) → deutlich unter dem √N-Grenzfall.

Formale Aussage (empirisch gestützt): für min/max gilt die Kontraktion
|min fᵢ − min dᵢ| ≤ maxᵢ|fᵢ − dᵢ| — harte Booleans VERSTÄRKEN Eingangsfehler
nie, sie fügen nur ihren eigenen Mengen-Gap hinzu (Winner-Branch-Propagation).
smin verstärkt in Quadratur: Konjektur worst case L ≤ √(m+1) bei m
geschachtelten smins (koinzident erreicht ihn; Staffelung bleibt darunter;
Zufall weit darunter). Fehler über Tiefe: Θ(√Tiefe) worst case, O(1) typisch.

### Schlussfolgerung Iteration 2

Die Tiefenangst „(√2)^N“ ist unbegründet: Quadratur-Kombination → √N nur im
koinzidenten Grenzfall, Sättigung im Normalfall. ABER: der koinzidente Fall
ist real (gemusterte/gestapelte Features auf gemeinsamer Basisfläche) und
schlägt mit √m durch — ein pauschales relax 0.75 reicht dort nicht.

---

## Iteration 3 — Zweikanal-AD, Wurzel-Sampson, Trace-Sicherheit

Skript: `exp5_root_sampson_tracing.py`. Dual-Number-Forward-AD als Prototyp
des Zweikanal-Evals im Bytecode-Interpreter: jede Op propagiert (val, grad)
analytisch — min/max via Winner-Branch, smin via blend_min_weights. Kein
Schrittweiten-Parameter, keine FD-Artefakte.

### Korpus-Messung (außen, d_true ∈ (10⁻³, 1)): max F/d_true (>1 = Schranken-
verletzung), nötiger relax = 1/max:

    Feld                roh            AD-Sampson     nötig relax
    a) smin moderat     1.208          1.205          0.83
    b) smin tief        1.366          1.361          0.73
    c) koinzident m=4   2.000          2.000          0.50
    d) harte Union      1.000          1.000          1.00
    e) Sichel-Diff      1.000          1.000          1.00
    f) Linse            1.000          1.000          1.00

Kernbefund: **Wurzel-Sampson senkt die maximale ÜBERSCHÄTZUNG praktisch
gar nicht** — am Ort des Maximums (Blend-Zonen-Rand bzw. außerhalb) ist
|∇f| ≈ 1. Die Sphere-Trace-Sicherheit ist ein Fußpunktfehler-Problem.
(Der scheinbare Widerspruch zu Iteration 1 löst sich auf: Sampson
verbessert die FEHLERGRÖSSE |f−d| dort, wo |∇f|<1 — Neck, Offsets —
nicht die Überschätzungs-RATIO fürs Marching.)

FD vs AD: FD-Sampson erzeugt an min/max-Tie-Flächen ZUSÄTZLICHE
Überschätzung bis 1/cos(γ/2) (≈1.4 gemessen, E2), die AD nicht hat; bei
glatten Zonen sind beide gleich. FD kostet 2·dim Extra-Evals (6 im 3D),
AD ≈ 2–4× Arithmetik pro Op (4 Register statt 1 im WGSL-Interpreter).

### Trace-Experimente (tol 1e-4)

    e) Sichel-Horn-Kegel:  roh 66 Schritte ok (Kriechen, kein Overshoot;
       typisch wären ~10); Sampson identisch (|∇f|=1) — Grey-Wall-Klasse.
    b) smin-Neck:          roh 1 Schritt, PENETRATION −7.2e-2!
       relax 0.75 → ok (3 Schritte). AD-Sampson allein: PENETRATION −5.3e-2.
    c) koinzident m=4:     roh PENETRATION −0.30; **relax 0.75 PENETRATION
       −0.10** (der heutige Wert versagt); AD-Sampson+0.75 ok (6 Schritte);
       sicher per Tabelle: relax 0.50 = 1/√m.

### Schlussfolgerung Iteration 3

1. Für Schrittweiten-Sicherheit ist die richtige Stellschraube ein
   TIEFEN-BEWUSSTES march_relax: 1/√(1+m) mit m = maximale Zahl
   geschachtelter smin-Ops entlang eines Tape-Pfads (zur Compile-Zeit aus
   dem Baum ablesbar, 0 Laufzeitkosten). Deckt den bewiesenen worst case;
   heutiges flaches 0.75 ist für m≥2 nachweislich unsicher, für m=1 mit
   tiefem Überlapp grenzwertig (nötig 0.707–0.73).
2. Wurzel-Sampson (einmalig am Wurzelknoten, AD-Gradient) ist die richtige
   Stellschraube für OFFSET-/ISOFLÄCHEN-GENAUIGKEIT im Nahfeld
   (−15% → −2% beim δ=0.05-Offset), muss aber vertrauensbandiert werden
   (|f| ≲ r_blend) und ersetzt kein relax.
3. Fußpunktfehler-Klasse (Intersection-/Differenz-Schattenkegel, Sichel-
   Kriechen, Union-Innentiefe → Shell +130%) ist mit Feld-Postprocessing
   jeder Ordnung UNERREICHBAR — lokal ist das Feld ein perfektes SDF der
   falschen Fläche; alle Ableitungen stimmen mit der falschen Geometrie
   überein. Hier helfen nur strukturelle Wege: (a) exakte algebraische
   Rewrites — Dilation kommutiert mit Union (Offset δ>0 durch min-Knoten
   PUSHEN: min(dᵢ)−δ = min(dᵢ−δ), verlustfrei), Erosion kommutiert mit
   Intersection; (b) Nahfeld-Korrektur gegen explizite Naht-Geometrie
   (Richtung Agent C) oder Redistancing (Agent A).

---

## Gesamtbild (Agent-D-Perspektive)

Zwei orthogonale Fehlerklassen mit disjunkten Gegenmitteln:

| Klasse | Diagnose | Beispiele | Gegenmittel | erreichbar |
|---|---|---|---|---|
| Normfehler (|∇f| ≠ 1) | AD-Gradientnorm | smin-Zonen (0.41–√m), Skalierung, algebraische Felder | Wurzel-Sampson (AD, vertrauensbandiert); Taubin2 für Meshing-Feinschliff | Fehler ↓ 10–25× (Ordnung bleibt 2 wg. Krümmung; Taubin2 → Ordnung 3) |
| Fußpunktfehler (|∇f| = 1, falscher Fußpunkt) | NICHT lokal diagnostizierbar | max/Diff-Schattenkegel (1−cos(γ/2), Sichel bis 95% Kollaps), Union-Innentiefe (Shell +130%), smin-Blend-Rand (bis √m) | Compile-Zeit: Offset-Pushdown, tiefen-bewusstes relax 1/√(1+m); strukturell: Naht-Geometrie/Redistancing | Sicherheit garantierbar; Genauigkeit nur strukturell |

Kostenbild Tape-Pipeline: Zweikanal-AD (val+grad) = 4 Register/Op im
WGSL-Interpreter, ~2–4× Arithmetik, KEINE Extra-Evals, keine FD-Artefakte —
klar der FD-Variante (6 Extra-Evals im 3D + Tie-Flächen-Overshoots)
vorzuziehen. Tiefen-bewusstes relax: reine Compile-Zeit-Analyse.

