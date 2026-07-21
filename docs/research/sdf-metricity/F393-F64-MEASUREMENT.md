# F393 — Empirische f32-Präzisions-Messreihe (Phase 1: Messung)

Ticket #393. Frage: Hält der durchgehend in f32 rechnende FieldDraft-Kernel
das Export-Toleranzbudget von **0.001 mm** bei ~300 mm Bauraum, oder braucht
es einen f64-Pfad (Export, Fold-Arithmetik, Kanäle, SSI, Messwerkzeuge)?

Vorgehen laut Ticket: **vor** jeder Umstellung empirisch messen, welche Pfade
real unter das Budget fallen — statt pauschal auf f64 umzustellen. Diese Phase
misst ausschließlich; sie ändert **keinen** Produktionscode.

## 1. Mess-Infrastruktur

Alle Messungen liegen in einem Test-Harness des Mess-Crates, kein
Produktionscode wurde angefasst:

- `crates/metricity-lab/tests/f393_f64_precision.rs` — der komplette Harness.
  Enthält:
  - einen **f64-Spiegel-Evaluator** (`mirror_sdf` / `mirror_primitive`), der
    die f32-Formeln 1:1 transkribiert, nur in f64: Primitive aus
    `kernel/src/sdf_eval.rs::sdf_primitive_generic` (Box, Sphere, Cylinder,
    Torus, Cone, mit purer `sqrt` wie die f32-`SdfSqrt`), die Transform-Algebra
    aus `kernel/src/transform.rs` (`quat_inverse_rotate`, `world_to_local`,
    `min_scale`) und die Boolean-/Offset-/Shell-Arme aus
    `kernel/src/csg.rs::sdf_open` (`Union = min`, `Difference = max(a, −b)`,
    `Intersection = max`, `Offset = inner − δ`, `Shell = shell_wall_sdf`).
    Der Spiegel ist algorithmisch identisch — er misst rein die
    Arithmetik-Breite, keinen Algorithmus-Unterschied.
  - Tests `m1_tree_eval_precision`, `m2_tape_vs_tree`,
    `m3_construction_arithmetic` (schnell, CI-tauglich, laufen in ~0.2 s) sowie
    `m1_tree_eval_precision_full` und `m4_export_vertices` (`#[ignore]`,
    langlaufend).
- Wiederverwendet aus dem bestehenden Harness: `metricity_lab::field::Field`,
  `metricity_lab::ground_truth::{GroundTruth, GtOptions}` (Brute-Force-f64-
  Distanz zur Nullmenge) und `metricity_lab::util::SplitMix64` (deterministisch,
  feste Seeds).

Metrik-Isolation: Beide Evaluatoren bekommen **denselben** f32-gerundeten Punkt
`p_f32`. Der f32-Pfad sieht real nie mehr als f32-Koordinaten (Mesher-Grid und
Ray-March sind f32), also isoliert `|f32(p_f32) − f64(p_f32)|` genau die
Arithmetik-Breite. Die davon getrennte **Eingangs-Quantisierung** (der Fehler,
den schon das Runden der Koordinate auf f32 einbringt) ist analytisch
`pos · 2⁻²⁴` und wird separat ausgewiesen.

Weil die Nullmenge eines gültigen SDF mit der Soll-Oberfläche zusammenfällt und
das Feld ~Lipschitz-1 ist, entspricht ein Feldwert-Unterschied von ε mm direkt
einer Oberflächen-Verschiebung von ε mm. `|f32 − f64|` ist damit unmittelbar der
Oberflächenfehler in Millimetern.

## 2. Kern-Zahlen

Budget durchgängig **1.0e−3 mm**. Fehler in mm, `p99`/`max` über Oberflächen-
bzw. Vertex-Stichproben.

### M1 — Kernel-Baum-Eval (f32 `CsgNode::sdf` vs f64-Spiegel)

| Bedingung | p50 | p99 | max |
|---|---|---|---|
| (a) Box 10 mm @Ursprung | 0 | 0 | 0 |
| (a) Box 100 mm @Ursprung | 0 | 0 | 0 |
| (a) Box 300 mm @Ursprung | 0 | 0 | 0 |
| (b) Box 300 mm @pos 0 | 0 | 0 | 0 |
| (b) Box 300 mm @pos 300 | 0 | 7.6e−6 | 7.6e−6 |
| (b) Box 300 mm @pos 1000 | 0 | 0 | 0 |
| (c) Bool-Kette Tiefe 1 @pos 300 | 0 | 7.6e−6 | 7.6e−6 |
| (c) Bool-Kette Tiefe 5 @pos 300 | 0 | 7.6e−6 | 7.6e−6 |
| (c) Bool-Kette Tiefe 10 @pos 300 | 0 | 7.6e−6 | 7.6e−6 |
| (c) Bool-Kette Tiefe 20 @pos 300 | 0 | 7.6e−6 | 7.6e−6 |
| (d) Transform-Kaskade Tiefe 1 | 1.7e−6 | 5.9e−6 | 7.6e−6 |
| (d) Transform-Kaskade Tiefe 5 | 3.7e−6 | 1.6e−5 | 2.4e−5 |
| (d) Transform-Kaskade Tiefe 10 | 5.7e−6 | 3.1e−5 | 3.6e−5 |
| (d′) Transform-Kaskade T1 @pos 1000 | 1.7e−6 | 6.0e−6 | 8.1e−6 |
| (d′) Transform-Kaskade T5 @pos 1000 | 3.6e−6 | 1.6e−5 | 2.3e−5 |
| (d′) Transform-Kaskade T10 @pos 1000 | 6.2e−6 | 3.0e−5 | 4.4e−5 |
| (e) Shell 100 mm Wand 0.5 mm @pos 300 | 0 | 0 | 0 |
| (e) Shell 300 mm Wand 0.5 mm @pos 300 | 0 | 7.6e−6 | 7.6e−6 |
| (e) Shell 300 mm Wand 1 mm @pos 300 | 0 | 7.6e−6 | 7.6e−6 |

Vollmatrix-Lauf (grid = 72, ~12 000 Punkte je Bedingung) bestätigt die Zahlen;
die Transform-Kaskade T10 landet dort bei p99 = 2.8e−5, max = 3.3e−5 mm.

**Worst-Case p99 über alle M1-Bedingungen: 3.1e−5 mm → ~32× Budget-Reserve.**

Beobachtungen:
- Modellgröße allein (a) und große Translation allein (b) erzeugen **keine**
  messbare Eval-Streuung (Werte exakt 0 oder ein einzelnes f32-ULP von
  7.6e−6 mm = 2⁻¹⁷). Die achsparallele Box-Arithmetik `|x−c|−h` löscht sauber
  aus; die große Verschiebung geht durch `world_to_local` als eine einzige
  Subtraktion, deren Rundung an der Oberfläche wieder verschwindet.
- **Boolean-Tiefe (c) akkumuliert nichts.** min/max sind exakte Selektionen,
  kein arithmetischer Fehler — Tiefe 1 und Tiefe 20 sind ununterscheidbar
  (7.6e−6 mm). Das ist ein wichtiges Negativ-Ergebnis: tiefe CSG-Bäume sind
  für sich genommen präzisionsneutral.
- **Der einzige echte Akkumulator ist die Transform-Kaskade (d).** Jede Ebene
  ist ein eigenes `world_to_local` (Quaternion-Rotation + Subtraktion +
  Division); der Fehler wächst mit der Tiefe (T1 ≈ 6e−6 → T10 ≈ 3e−5 mm),
  ~√-artig. Selbst bei 10 verschachtelten Transforms plus 1000 mm Offset bleibt
  p99 bei 3e−5 mm — Faktor ~32 unter Budget.
- Dünne Wände (e, Shell 0.5 mm) zeigen **keine** Auslöschungs-Katastrophe: die
  Shell ist `max(d, −(d+t))`, kein Differenz-zweier-großer-Zahlen.

Getrennt ausgewiesen: die **Koordinaten-Quantisierung** ist der eigentliche
größte f32-Einzeleffekt bei 300 mm Bauraum — bei Position 1000 mm rundet schon
die Eingangskoordinate um `1000 · 2⁻²⁴ ≈ 6.0e−5 mm`, bei 300 mm um ≈ 1.8e−5 mm.
Das ist ein Eigenschaft der f32-**Eingabe** (Grid/March), nicht des Evals, und
liegt weiterhin ~17× unter Budget.

### M2 — Opcode-Tape (f32) vs Baum-Eval

`[truth]` = Tape gegen f64-Spiegel, `[tapeδ]` = Tape gegen direkten f32-Baum-Eval
(isoliert den reinen Tape-Zusatzverlust durch andere Assoziierung / gespeicherte
Zwischenwerte).

| Szene | truth p99 | truth max | tapeδ p99 | tapeδ max |
|---|---|---|---|---|
| Box 300 mm @Ursprung | 0 | 0 | 0 | 0 |
| Box 300 mm @pos 300 | 7.6e−6 | 7.6e−6 | 0 | 0 |
| Box 300 mm @pos 1000 | 0 | 0 | 0 | 0 |
| Bool-Kette T10 @pos 300 | 7.6e−6 | 7.6e−6 | 3.8e−6 | 7.6e−6 |
| Transform-Kaskade T10 @pos 1000 | 3.7e−5 | 3.9e−5 | 4.2e−5 | 4.6e−5 |
| Shell 300 mm Wand 0.5 @pos 300 | 7.6e−6 | 7.6e−6 | 0 | 0 |

**Worst tape-truth p99: 3.7e−5 mm.** Das Tape kostet gegenüber dem Baum-Eval
praktisch nichts (tapeδ meist 0, im Kaskaden-Fall ≈ 1 ULP mehr). Das Tape
verletzt das Budget also nicht zusätzlich — es liegt auf demselben ~3e−5-Niveau
wie der Baum-Eval.

### M3 — Konstruktions-Arithmetik (offset_fold)

Der Fold-Pfad konstruiert Parameter über `offset_fold::shrink_param` =
`offset_scalar_inward` = schlicht `v − δ` in f32 (Radien, Halbhöhen,
Halbausdehnungen). Das 2D-Profil-Erodieren läuft über `cavalier_contours`, das
**intern in f64 rechnet** (die Region-Typen im Fold-Pfad sind `Polyline<f64>`,
`crates/kernel/src/offset_fold.rs`) — dort entsteht also kein f32-Verlust.

Einzel-Subtraktion `v − δ`:

| v (mm) | δ = 0.1 | δ = 0.5 | δ = 2.0 |
|---|---|---|---|
| 5 | 9.5e−8 | 0 | 0 |
| 25 | 3.8e−7 | 0 | 0 |
| 75 | 1.5e−6 | 0 | 0 |
| 150 | 6.1e−6 | 0 | 0 |

Kaskadiert (N sequentielle f32-Subtraktionen von v = 150):

| N | \|Δ\| |
|---|---|
| 1 | 6.5e−6 |
| 5 | 3.2e−5 |
| 10 | 6.5e−5 |
| 20 | 1.1e−4 |

**Worst single 6.1e−6 mm, worst kaskadiert (20 Offsets) 1.1e−4 mm** — Faktor ~9
unter Budget selbst bei absurd tiefen Offset-über-Offset-Stapeln. Der Fehler des
einzelnen konstruierten Parameters ist ~½ ULP des Ergebnisses (bei 150 mm ≈ 6e−6);
er summiert sich linear über gestapelte Offsets.

### M4 — DC-Export-Vertices vs f64-Grundwahrheit

DC-Mesh (`dc::dual_contour`) eines 300-mm-Modells, Vertex-Positionen gegen zwei
Ground Truths: **total** = Vertex → wahre f64-Fläche (f32-Eval- **plus**
Diskretisierungsfehler); **diskret** = Vertex → f32-Nullmenge (auf die der
DC-Newton-Schritt projiziert, also reines Diskretisierungs-/Newton-Residuum).
Die Differenz total − diskret ist der f32-Eval-Beitrag (≈ M1).

| Modell (res) | total p99 | total max | diskret p99 | diskret max |
|---|---|---|---|---|
| Box 300 mm (96) | 3.5e−4 | 1.6e−3 | 3.4e−4 | 1.6e−3 |
| Box 300 mm @pos 1000 (96) | 3.6e−4 | 1.6e−3 | 3.5e−4 | 1.6e−3 |
| Bool-Kette T10 300 mm (96) | 5.5e−2 | 5.5e−2 | 5.5e−2 | 5.5e−2 |

(Shell 300 mm Wand 2 mm @ res 128 lieferte ein **leeres** Mesh: die 2-mm-Wand ist
schmaler als die ~2.3-mm-Zellen — ein Auflösungs-/Topologie-Effekt, kein
Präzisionsproblem. Dünne Wände brauchen Grid-Auflösung ≥ Wandstärke, orthogonal
zur f32-Frage.)

**Das entscheidende Negativ-Ergebnis:** total ≈ diskret in **allen** Fällen. Der
f32-Eval-Beitrag (total − diskret) ist ~1e−5 mm und damit im Rauschen — die
Diskretisierung dominiert den Export-Fehler vollständig. Schon die Box überschreitet
im max (1.6e−3 mm) das 0.001-mm-Budget rein durch DC-Diskretisierung; die
Boolean-Kette liegt mit 5.5e−2 mm um **Faktor > 50** darüber (scharfe Feature- Kanten,
die DC bei res 96 nicht auflöst). Die absoluten Box-Werte (~3.5e−4 mm) liegen nahe
dem Mess-Boden der Ground-Truth-Kompass-Suche und sind eher obere Schranken; die
Boolean-Kette dagegen ist unzweideutig reale Diskretisierung.

Ein f64-Export-Pfad würde an diesen Zahlen **nichts** ändern: er könnte den
~1e−5-mm-Eval-Beitrag entfernen, aber der budget-bestimmende Term ist die
Meshing-Diskretisierung (3e−4 bis 5e−2 mm).

## 3. Verdikte je Pfad gegen das 0.001-mm-Budget

| Pfad | Worst p99 | Verdikt |
|---|---|---|
| M1 Kernel-Baum-Eval (f32) | 3.1e−5 mm | **Unter Budget, ~32× Reserve.** |
| — davon Boolean-Tiefe | 7.6e−6 mm | Präzisionsneutral (min/max exakt). |
| — davon Transform-Kaskade | 3.1e−5 mm | Einziger Akkumulator, hält Budget bis T10. |
| — Koordinaten-Quantisierung (Input) | 6.0e−5 mm @1000 mm | Unter Budget, ~17× Reserve. |
| M2 Opcode-Tape (f32) | 3.7e−5 mm | **Unter Budget**, kein Zusatzverlust ggü. Baum. |
| M3 Konstruktions-Arithmetik | 1.1e−4 mm (20× Offset) | **Unter Budget**, ~9× Reserve; cavalier ist f64. |
| M4 DC-Export gesamt | 3.5e−4 … 5.5e−2 mm | **Budget verletzt — aber durch DISKRETISIERUNG, nicht f32-Eval.** |

Kein Eval-/Konstruktions-Pfad verletzt das Budget. Der einzige Pfad, der das
Budget reißt (M4), reißt es aus einem Grund, den f64 nicht behebt.

## 4. Empfehlung

**Option (d): Nichts an der f32-Arithmetik tun — das Budget hält, wo f32 rechnet.**

Die Messung stützt weder den ursprünglichen Review-Vorschlag eines f64-Export-
Pfades noch die weitergehende f64-Umstellung von Fold/Kanal/SSI/Messwerkzeugen:

- Der f32-Kernel-Eval (M1) hält das Budget mit ~32× Reserve; selbst die
  pathologischste gemessene Lage (Transform-Kaskade T10 + 1000 mm) bleibt bei
  3e−5 mm. Tiefe CSG-Bäume (der Kern der #384-Offset-Baum-Sorge) akkumulieren
  gar nichts.
- Das Opcode-Tape (M2) kostet keine zusätzliche Stelle.
- Die Konstruktions-Arithmetik (M3) ist ½-ULP-klein; cavalier_contours rechnet
  ohnehin f64.
- Der Export (M4) verletzt das Budget, aber der bestimmende Term ist die
  **Meshing-Diskretisierung**, nicht die f32-Feldauswertung. Ein f64-Eval-/
  Export-Pfad würde den ~1e−5-mm-Eval-Anteil entfernen und den 3e−4…5e−2-mm-
  Diskretisierungsanteil unangetastet lassen — also den Fehler praktisch nicht
  senken. Wer die 0.001 mm im Export wirklich will, muss an der
  **Vermaschung** ansetzen (feinere/adaptive Auflösung, Feature-Snap an
  scharfen Kanten — genau die bestehenden Mesher-Projekte), nicht an der
  Zahlenbreite. Der bereits existierende **CGAL-Server-Mesher rechnet ohnehin in
  C++-double** und ist der richtige Hebel für hochgenauen Export.

Falls später doch ein einzelner f32-Effekt stört, ist der **einzige** empirisch
belegte Kandidat die Koordinaten-Quantisierung großer Translationen (Option (b),
lokale Koordinatensysteme pro Body) — sie ist mit 6e−5 mm @1000 mm der größte
f32-Einzelbeitrag. Aber auch sie hält das Budget mit ~17× Reserve; eine
Umstellung wäre heute verfrüht. Option (a) (f64-Konstruktion + f32-Tape) und (c)
(gezielte Einzel-Fixes) adressieren Fehlerquellen, die die Messung nicht
nachweisen kann — sie sind durch die Daten **nicht** gestützt.

Kurz: f32 reicht überall, wo der Kernel rechnet. Genau das ist das Ergebnis.

## 5. Was NICHT gemessen wurde (und warum)

- **M5 (SSI / Kanal-Synthese / Naht-Seeds):** nicht quantitativ gemessen. Der
  SSI-/Edge-Tracing-Pfad (`brep/`) und die Arc-Spline-Kanäle
  (`offset_channel.rs`) haben keine geschlossene f64-Spiegelform, die sich mit
  vertretbarem Aufwand 1:1 aufbauen ließe; ein ehrlicher Vergleich hätte einen
  kompletten f64-Nachbau der SSI-Continuation erfordert. Indizien aus den
  Nachbar-Messungen: Der Kanal-Toleranz-Beitrag (`CHANNEL_TOL_RENDER`) ist ein
  bewusst gesetztes Budget im ~1e−2-mm-Bereich (siehe `offset_fuzz.rs`) und
  damit ohnehin größer als jeder hier gemessene f32-Eval-Fehler — die f32-Frage
  ist dort nicht der limitierende Faktor. Sollte SSI-Genauigkeit später
  kritisch werden, ist ein dedizierter f64-Spiegel der Continuation der
  nächste Schritt.
- **Sketch-getragene Primitive (Extrude/Revolve/Helix/Sweep, ImportedMesh):**
  Der f64-Spiegel deckt die analytischen Primitive (Box/Sphere/Cylinder/Torus/
  Cone) ab. Die sketch-/mesh-getragenen Arme rufen im f32-Pfad ohnehin einen
  2D-Sketch-Kern (`sketch2d`) bzw. Mesh-Distanz auf, der keine generische
  Primitiv-Form hat; ein Spiegel hätte den gesamten 2D-Kern duplizieren müssen.
  Da der 3D-Zusammenbau (Extrude-Combine, Revolve-Radius) dieselben min/max/
  sqrt-Bausteine nutzt wie die gemessenen Primitive, ist kein abweichendes
  f32-Verhalten zu erwarten; explizit gemessen ist es aber nicht.
- **SmoothUnion/SmoothDifference/MaskedBlend (Fillets):** nicht gemessen. Die
  Blend-Operatoren (`blend_min`) sind im Visual-Mode aktiv und haben ein
  eigenes, geometrie-definierendes Radius-Budget; der f32-Eval-Fehler ist dort
  nachrangig gegenüber der Blend-Radius-Semantik selbst (vgl. #386).
- **Nicht-uniforme Skalierung:** Die Szenen nutzen `scale = [1,1,1]`. Nicht-
  uniforme Skalierung verzerrt die Metrik ohnehin systematisch (die Transform-
  Arme multiplizieren mit `min_scale`), was ein Metrik- und kein
  Präzisionsthema ist.

## 6. Test-Ergebnis-Zeilen

Schnelllauf (`cargo test -p metricity-lab --test f393_f64_precision`):

test m1_tree_eval_precision ... ok
test m2_tape_vs_tree ... ok
test m3_construction_arithmetic ... ok
test result: ok. 3 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.22s

Langlauf (`… -- --ignored --nocapture`, release):

test m1_tree_eval_precision_full ... ok
test m4_export_vertices ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 3 filtered out; finished in 1.21s

Roh-Logs der Läufe: `/tmp/f393_m1.txt`, `/tmp/f393_m2.txt`, `/tmp/f393_m3.txt`,
`/tmp/f393_m1_full.txt`, `/tmp/f393_m4.txt`.
