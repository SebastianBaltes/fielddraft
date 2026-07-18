# B0 — Messreihe „konkave Shell-Naht" (#384, offset_tree Phase B)

Stand 2026-07-19. Schließt die in `docs/OFFSET-TREE-PLAN.md` §6/§9 als
„bisher NICHT gemessen" markierte Lücke: der konkave Naht-Fall unter
Erosion/Innen-Offset (Shell-Innenwand). Der Spike (LOG.md, Teil B) hatte
nur den konvexen Überschuss-Keil (+δ über Schnitt) quantifiziert.

Binary: `crates/metricity-lab/src/bin/b0_concave_seam.rs`
(`cargo run -p metricity-lab --release --bin b0_concave_seam`).
Rohdaten: `b0_concave_seam.csv` (26 Fälle). Laufzeit ~2 min.

## Kernfrage und Antwort vorweg

**Frage:** Ist das heutige Feld (nach Pushdown + Blatt-Einfalten, exakt
der Baum, den Phase B vorfindet) an konkaven Nähten nullmengen-exakt —
oder gibt es echte Zero-Set-/Vorzeichenfehler, die einen konkaven
Kanal-Korrekturkörper erfordern?

**Antwort (zweigeteilt):**

1. **Alle Pfade, auf denen die Erosion ein exaktes Blatt erreicht, sind
   nullmengen-exakt** — L-Extrude (Fold), Box-minus-Box (−δ exakt durch
   Difference, Subtrahend f−δ am Blatt), Dieder per Intersection,
   Box-Shell in beiden Bauarten: Nullmengen-Abstand ≤ 2.2e-5 mm
   (f32-Rauschklasse), **null** Vorzeichen-Fehlklassifikationen über
   ~230 000 Messpunkte. Dort gibt es nur den bekannten
   **Metrik-/Fußpunktfehler** der max/min-Naht (|f−d| ≈ (1−sin(α/2))·r
   im Abstand r zur Naht, s. u.) — für Rendering und Mengen-Semantik
   harmlos, kein Korrekturkörper nötig.
2. **Der duale Stopp-Fall −δ an Union (`notch_union`) hat einen ECHTEN
   Nullmengenfehler:** die Wurzel bleibt `f − δ` = min(f_A, f_B) + |δ|,
   dessen Nullmenge ist (A↓δ) ∪ (B↓δ) statt (A∪B)↓δ — an der konkaven
   Naht **fehlt der Kanal-Sliver**. Gemessene Sliver-Tiefe folgt der
   Vorhersage **δ·(1/sin(α/2) − 1)** auf 1–4 % (α = Öffnungswinkel des
   Luftkeils): bei α=60°, δ=2 mm fehlen 1.99 mm Material; bei 90°
   0.82 mm. **Genau dort (und nur dort) braucht Phase B den konkaven
   Kanal-Korrekturkörper (Union-Fall, Material ADDIEREN).**

## Aufbau

### Fixtures (IST-Feld)

Alle Fixtures laufen durch die Produktions-Normalisierung
`push_down_offsets().fold_offset_leaves()` — identisch zu
`body.rs::build_bounded_for_body_rounded` (die Fixtures haben weder
Edge-Fillets noch Global-Round, die dortigen Vorstufen sind
No-Ops). Die erwartete Baumform wird je Fall per Assertion geprüft
(vollständig gefaltet / Dilations-Offset am Blatt / Stopp an der Wurzel).

| Fixture | Baum | δ bzw. t [mm] | Pipeline-Ergebnis |
|---|---|---|---|
| `l_extrude` | Offset(Extrude(L-Polygon, h=12), −δ), Reflex-Ecke (12,12) | 0.5 / 2 / 5 | Fold: Profil↓δ (Bogen an der Reflex-Ecke) + h−δ |
| `pocket` | Offset(Difference(Box 60×40×30, Box 24×14×24 durch den Deckel), −δ) | 0.5 / 2 / 5 | −δ exakt durch Difference; Subtrahend dilatiert als `Offset(Leaf, +δ)` = f−δ |
| `wedge_int` | Offset(Intersection(Box, Box@Rz), −δ), Deckflächen-Dieder α | α ∈ {60…170}° bei δ=2; δ ∈ {0.5, 5} bei 90° | −δ exakt durch Intersection, Blätter gefaltet |
| `notch_union` | Offset(Union(Box, Box@Rz), −δ), Luftkeil-Öffnung α | wie `wedge_int` | **STOPP**: Wurzel bleibt `Offset(Union, −δ)` = f−δ |
| `shell/expand` | Difference(Box 50×36×24, Offset(Box, −t)) — so baut Phase D Shell | t = 0.5 / 2 / 5 | Fold: Kavität = geschrumpfte Box |
| `shell/field` | CsgNode::Shell(Box, t) = max(d, −(d+t)) — heutiger Modifier | t = 0.5 / 2 / 5 | unverändert |

Dieder-Fixtures messen in einem ±25/±20-mm-Fenster um die Naht; die
Boxen (200×100×100) sind so groß, dass dort nur die beiden Deckflächen
leben.

### Ground Truth

Brute-Force-GT des metricity-lab-Harness (`GroundTruth`, Gitter 96,
Shell t=0.5 Gitter 200, Richtungs-Raffinierung an), gebaut auf einem
**analytischen, vorzeichen-exakten f64-Referenzfeld des WAHR erodierten
Solids** je Fixture (Erosions-Mengenidentitäten: (A∩B)↓δ = A↓δ ∩ B↓δ;
(A∖B)↓δ = A↓δ ∖ B↑δ; Extrude↓δ = Extrude(Profil↓δ, h−δ) mit exaktem
Polygon-SDF; Union-Dieder: Material ⇔ dist(p, Luftkeil-Kegel) ≥ δ).
Damit misst |f−d| die Summe aus Nullmengen-Verschiebung und
Metrikfehler gegen die **wahre** Erosionsfläche, nicht gegen die
Feld-eigene Nullmenge.

### Metriken je Fall

* |f−d| getrennt nach **Naht-Band** (≤ 1 mm von der wahren Naht-Kurve:
  Bogenwand/Mundrand/Keilkante/Innenkanten, analytisch), **Naht-Region**
  (≤ δ+1 mm) und **Fernfeld**; Band/Region gezielt besampelt
  (Rejection, je ~2000 Punkte), dazu 2000 uniforme + 3000
  oberflächennahe Punkte.
* **Nullmengen-Abstand:** Oberflächen-Punktwolke des IST-Felds
  (Bisektion, ≤ 2400 Punkte) → |d_wahr| je Punkt (`zs_*`-Spalten).
* **Vorzeichen-Fehlklassifikation** (`mis_*`): Feld- vs. GT-Vorzeichen,
  inkl. maximaler wahrer Tiefe eines Fehlers; Fehler außerhalb der
  Naht-Region wären Konstruktionsfehler (überall 0 — Kreuzvalidierung
  der Referenzfelder).
* **‖∇f‖ im Band** (zentrale Differenzen, h = 1e-3·Modell) für die
  Taxonomie Normfehler vs. Fußpunktfehler (SYNTHESIS §1).

## Ergebnisse

### Nullmengen-exakte Familien (kein Korrekturkörper nötig)

max |f−d| im Naht-Band / Nullmengen-Abstand max (mm):

| Fixture | δ/t=0.5 | 2 | 5 | zs max (alle δ) | mis |
|---|---|---|---|---|---|
| `l_extrude` | 0.003 | 0.017 | 0.022 | **3e-6** | 0 |
| `pocket` | 0.246 | 0.278 | 0.265 | **6e-6** | 0 |
| `shell/expand` | 0.00004 | 0.00004 | 0.00004 | **4e-6** | 0 |
| `shell/field` | 0.059 | 0.278 | 0.288 | **4e-6** | 0 |

* **`shell/expand` ist überall exakt** (f32-Rauschen): Box↓t ist eine
  exakte geschrumpfte Box, und max(d_box, −d_kavität) zweier exakter
  SDFs ist an dieser Geometrie das exakte SDF. Die Phase-D-Expansion
  repariert also nicht nur die Nullmenge, sondern (bei Box-Shells) auch
  die Metrik komplett.
* **`shell/field` (heutiger Modifier)** hat an den Innenkanten den
  Level-Set-Metrikfehler ≈ 0.29·r (r = Abstand zur Innenkante, 90°:
  Faktor 1−sin 45°) — Nullmenge und Vorzeichen exakt. Der Faktor 0.288
  bei t=5 im 1-mm-Band bestätigt die Gesetzmäßigkeit.
* **`pocket`:** Band-Fehler 0.25–0.28 (gleiche 0.29·r-Klasse am
  Mundrand); im Fernfeld wächst der bekannte Differenz-Fan auf bis zu
  2.9 mm (13–16 %-Klasse aus der Baseline) — Metrik, nicht Menge.
* **`l_extrude`:** p95 im Band 2e-6 mm; die Ausreißer bis 0.022 liegen
  in der dokumentierten GT-Kompass-Restfehlerklasse an Ecken-Tälern
  (HARNESS.md: ≤ 2.4e-4·Modellgröße ≈ 0.01 mm, konkave Bogen-Ecken
  etwas darüber), nicht im Feld: mean 4.5e-5, Nullmenge 3e-6.

### Dieder-Sweep: Fehler als Funktion des Konkav-Winkels

δ = 2 mm; „pred" = analytische Vorhersage.

**`wedge_int`** (Intersection, exakter Pushdown — reiner Metrikfehler,
pred = Faktor (1−sin(α/2)) je mm Naht-Abstand):

| α | band max |f−d| | zs max | pred·1mm |
|---|---|---|---|
| 60° | 0.461 | 1.7e-5 | 0.500 |
| 90° | 0.234 | 9e-6 | 0.293 |
| 120° | 0.125 | 9e-6 | 0.134 |
| 150° | 0.031 | 9e-6 | 0.034 |
| 170° | 0.002 | 2.2e-5 | 0.004 |

Nullmenge exakt für jedes α; der Metrikfehler skaliert linear mit dem
Abstand zur Kante (Fernfeld bis 17 mm bei α=60° in 34 mm Abstand — der
Rarefaction-Fan wirkt in jeder Entfernung, March-Sicherheit ist davon
unberührt: Unterschätzung, Bias durchweg negativ).

**`notch_union`** (Union, dualer STOPP-Fall — echter Nullmengenfehler,
pred = Sliver-Tiefe δ·(1/sin(α/2) − 1)):

| α | δ | zs max | mis-Tiefe max | pred | Band-Bias |
|---|---|---|---|---|---|
| 60° | 2 | **1.989** | 1.728 | 2.000 | +0.254 |
| 90° | 2 | **0.821** | 0.764 | 0.828 | +0.139 |
| 120° | 2 | **0.303** | 0.249 | 0.309 | +0.049 |
| 150° | 2 | **0.065** | 0.058 | 0.071 | +0.009 |
| 170° | 2 | **0.003** | 0.003 | 0.008 | +0.001 |
| 90° | 0.5 | 0.075¹ | **0.193** | 0.207 | +0.031 |
| 90° | 5 | **1.953** | 1.703 | 2.071 | +0.454 |

¹ Bei δ=0.5 ist die IST-Oberflächenwolke (Gitterweite ~0.55 mm) zu grob
für die Sliver-Spitze; die Vorzeichen-Fehlklassifikation (mis-Tiefe
0.193 ≈ pred 0.207) ist dort die belastbare Schranke.

Der Fehler ist **fehlendes Material** (Bias positiv, Feld meldet Luft,
wo wahr Material ist): die Nullmenge von min+|δ| ist die Union der
einzeln erodierten Kinder, der Sliver zwischen scharfer Feld-Ecke und
wahrem Erosions-Bogen (Radius δ um die Naht) fehlt. 488 von ~9000
Punkten sind bei α=60° falsch klassifiziert, bis in 1.73 mm wahre
Tiefe.

### Taxonomie: Fußpunktfehler, nicht Normfehler

‖∇f‖ im Naht-Band weicht im Mittel um ≤ 0.05 von 1 ab (`notch_union`:
0.002–0.007) — das Feld ist lokal ein perfektes SDF der **falschen**
Fläche (Fußpunktfehler nach SYNTHESIS §1). Konsequenz bestätigt:
gradientenbasierte Korrekturen (Sampson etc.) sind hier blind; nur die
strukturelle Korrektur (Kanal-Körper) hilft. Die grad-max-Ausreißer
(0.29–0.99) sind die Differenzen-Schablone quer über die Naht-Falte
bzw. die 0.5-mm-Wand, keine Feldeigenschaft.

## Konsequenz für den Phase-B-Scope

1. **Konkave Kanal-Korrekturkörper sind NUR am dualen Stopp-Fall −δ an
   Union nötig** (und per Shell-Dualität überall, wo eine Erosion über
   einer Union stehen bleibt — d. h. Shell/Offset auf einem
   Multi-Feature-Body mit konkaven Feature-Nähten). Additionsfall:
   Kanal = Bogen-Sweep um die Naht-Kurve, Material **hinzufügen**
   (Union), Größenordnung δ·(1/sin(α/2) − 1) — bei üblichen 90°-Nähten
   ≈ 0.41·δ, bei spitzen Kehlen (60°) ≈ 1.0·δ. Ohne Korrektur ist eine
   2-mm-Shell-Innenwand an einer 90°-Naht lokal 0.8 mm zu dick
   (Kavität zu klein) — kein Rendering-Artefakt, sondern falsche
   Geometrie in Mesh/Export.
2. **Difference-/Intersection-Erosionen und Blatt-Folds brauchen KEINEN
   konkaven Korrekturkörper** — ihre konkaven Nähte sind
   nullmengen-exakt; was bleibt, ist der bekannte, march-sichere
   Metrikfehler (Unterschätzung ≤ (1−sin(α/2))·r), den die
   Nicht-Ziele-Doktrin des Plans (keine Innen-Betragsgarantie) bereits
   abdeckt.
3. Die Winkelabhängigkeit liefert das **Pruning-Kriterium gratis**: bei
   α ≥ 150° (fast tangentiale Nähte) liegt der Sliver unter 0.07·δ —
   Kanal-Körper dort können unterhalb der Zieltoleranz entfallen
   (Phase C, Toleranz-Staffelung Render 0.01 mm / Export 0.001 mm
   gegen δ·(1/sin(α/2) − 1) prüfen).
4. Für **Phase D** bestätigt `shell/expand` vs. `shell/field`: die
   Expansion S ∖ Offset(S, −t) ist dem heutigen Feld-Shell auch
   metrisch überlegen (Box-Shell: exakt statt 0.29·r-Fehler an den
   Innenkanten) — zusätzlich zum eigentlichen Ziel, dass die Erosion
   dann durch die Boolean-Regeln läuft.

## Limitierungen (ehrlich)

* **Prismatische, gerade Nähte.** Alle Naht-Kurven sind Geraden bzw.
  achsparallele Bögen; der Arc-Spline-Fit-Aspekt von Phase B (gekrümmte
  Schnittkurven, Normalkegel-Gating) ist hier nicht mitgemessen — dafür
  bleibt die Steinmetz-Szene des Spikes maßgeblich.
* **Nullmengen-Auflösung ~0.5 mm Gitter:** `zs_*` unterschätzt
  Sliver < 1 Gitterzelle (δ=0.5-Fall); dort ist `mis_max_depth` die
  Schranke. GT-Kompass-Resttail ≤ ~0.02 mm an konkaven Ecken-Tälern
  (l_extrude-Band-Maxima) liegt über der f32-Rauschklasse, aber zwei
  Größenordnungen unter den gemessenen Effekten.
* **Keine smin/MaskedBlend-Stopps gemessen** (Offset über gefilletetem
  Körper bleibt f−δ, Plan §9) — B0 belegt nur die harten Booleans.
* Der Union-Dieder misst den **lokalen** Zwei-Flächen-Fall; Überlappung
  mehrerer Normalkegel (enge Features, großes δ) bleibt Phase-C-Witness-
  Thema.
* Punktmengen deterministisch (Seed 4242), aber Band-Sampling per
  Rejection — Zählspalten (`band_n` etc.) variieren leicht je Fixture.

## Verweise

* CSV: `b0_concave_seam.csv` (Spalten: Band/Region/Fernfeld-Stats,
  `zs_*`, `mis_*`, `grad_band_*`, `pred_mm`).
* Plan: `docs/OFFSET-TREE-PLAN.md` §6 (Phase B), §9 (offene Punkte —
  „Konkaver Kanal ungemessen" ist hiermit geschlossen).
* Theorie: SYNTHESIS §1 (Taxonomie), §9 (Doktrin); Rossignac &
  Requicha §2.6 (Kanal-Flächen).
