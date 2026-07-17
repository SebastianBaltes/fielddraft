# LOG — Agent C („Hybrid"): Nahfeld-Distanz zur exakten Randrepräsentation

Forschungsfrage: Kann FieldDraft die (Nahfeld-)echte euklidische Distanz über
„Distanz zum gewobenen B-Rep-Proxy" (Faces/Kanten aus dem Weaver, tesseliert,
BVH-beschleunigt) zurückgewinnen, statt das analytische Feld selbst zu
reparieren? Harness: `lab/` (dependency-freies Rust, `~/.cargo/bin/cargo run
--release` im `lab/`-Verzeichnis; alle Zahlen unten von diesem Lauf,
reproduzierbar mit festen RNG-Seeds).

---

## Iteration 1 — Grounding, Literatur, Basis-Experimente (E1–E4)

### Codebasis-Grounding (Fakten)

- `crates/kernel/src/brep/tessellate.rs`: `tessellate_brep(brep, placement,
  tol_mm)` existiert bereits — der Proxy mit **kontrollierter
  Chordal-Toleranz** ist heute schon ein Kernel-Artefakt des Weavers.
- `crates/kernel/src/csg.rs`: `hard_combine` = `max(da,−db)` bzw. `min`;
  `blend_min` (smin_poly/chamfer) für Fillets. Das Feld ist die SSOT der
  Oberfläche — auch die geblendete Fläche IST die Nullstellenmenge des Felds.
- `crates/kernel/src/api/voxel.rs`: sampelt nur **Occupancy** (SDF<0), kein
  Distanzfeld — der Voxelpfad liefert heute keine Nahfeld-Distanz.
- Weaver läuft bereits **async pro Regen** (weaver.worker,
  placement-invarianter Cache-Key `importBodyBrep`) — die
  Invalidierungs-Maschinerie für einen Proxy existiert.

### Quellen

1. Jacobson, Kavan, Sorkine-Hornung (2013): *Robust Inside-Outside
   Segmentation using Generalized Winding Numbers*, SIGGRAPH/TOG.
   https://igl.ethz.ch/projects/winding-number/ — robustes Vorzeichen für
   defekte/offene Meshes (Soups, Löcher, Selbstschnitte).
2. Barill, Dickson, Schmidt, Levin, Jacobson (2018): *Fast Winding Numbers for
   Soups and Clouds*, SIGGRAPH. https://www.dgp.toronto.edu/projects/fast-winding-numbers/
   — O(log n)-Auswertung per Fernfeld-Multipol; macht Winding-Sign praktikabel.
3. Sawhney: *FCPW — Fastest Closest Points in the West*.
   https://github.com/rohan-sawhney/fcpw — Referenz-BVH für Punkt-Mesh-Queries,
   CPU-vektorisiert (Enoki) + **GPU-Backend (Slang)** — Beleg, dass
   BVH-Distanzqueries auch GPU-seitig praktikabel sind.
4. Zong et al. (2023): *P2M: A Fast Solver for Querying Distance from Point to
   Mesh Surface*, TOG. https://doi.org/10.1145/3592439 — schneller als
   BVH-Traversal durch Vorberechnung; Option, falls unser Query-Budget knapp wird.
5. Bærentzen, Aanæs (2005): *Signed Distance Computation Using the Angle
   Weighted Pseudonormal*, IEEE TVCG — Vorzeichen aus dem Closest-Point
   (Pseudonormalen-Test) für wasserdichte Meshes; libigl `signed_distance`
   implementiert genau das.
6. Frisken, Perry, Rockwood, Jones (2000): *Adaptively Sampled Distance
   Fields*, SIGGRAPH. https://dl.acm.org/doi/10.1145/344779.344899 — der
   klassische Gegenspieler (Gitter/Octree-Sampling): O(h)-Fehler an Kanten,
   genau das, was der exakte Proxy vermeidet.
7. OCCT `BRepExtrema_DistShapeShape` / `GeomAPI_ProjectPointOnSurf`:
   https://dev.opencascade.org/doc/refman/html/class_b_rep_extrema___dist_shape_shape.html
   — CAD-Kernel-Praxis: Distanz = Extrema über Sub-Shapes (Faces/Edges/
   Vertices) + Newton-Projektion auf Parameterflächen mit Startwert aus
   grober Diskretisierung. Genau die Zwei-Stufen-Architektur (diskreter
   Kandidat → analytisches Polish), die uns später offensteht.
8. nTop-Praxisreferenz (implicit CAD): Offsets „nie fehlschlagend", aber nur
   auf echten Distanzfeldern — https://www.ntop.com/resources/blog/how-implicits-succeed-where-b-reps-fail/

### Hypothesen (Iteration 1)

- **H1**: Das min/max-Schrankenfeld hat im Nahfeld konkaver Nähte relative
  Fehler bis Faktor √2. *(Folklore — wird sich als teils falsch, teils zu
  optimistisch herausstellen.)*
- **H2**: Distanz zum exakten Proxy-Mesh ist im Nahfeld maschinengenau, wo
  Faces planar sind; bei gekrümmten Faces ist der Fehler ≈ Sehnenfehler (sag)
  der Tesselation, also **O(tol_mm)** und zweiter Ordnung in der Segmentdichte.
- **H3**: BVH-Punkt-Mesh-Queries sind CPU-seitig für Meshing/Offset/Shell
  budgetierbar (≥10⁵ q/s), aber nicht für per-Frame-Raymarching.
- **H4**: Ein Blend d_near↔f an einer Schwelle ist marsch-sicher (keine
  Überschätzung), aber nicht 1-Lipschitz, außer das Blendband ist breit.

### Aufbau

`lab/src/main.rs`, f64, ohne Dependencies:
- Szene E1: A = Box he(20,10,10) @ Origin, B = Box he(12.5,10,12.5) @
  (12.5,10,0); A∖B = L-Prisma (Polygon 6-Eck, z∈[−10,10]); konkave Kante
  x=y=0. **Ground Truth analytisch**: exakte SDF des extrudierten Polygons
  (2D-Polygon-SDF + Slab-Kombination) — unabhängig vom Mesh.
- Proxy: handkonstruiertes exaktes L-Prisma-Mesh (20 Dreiecke).
- Punkt-Dreieck-Distanz nach Ericson (RTCD 5.1.5), BVH Median-Split, Leaf=4,
  Branch-Pruning per AABB-dist².
- Zylinder r=10, hz=10 für Tesselationsfehler (inskribiert, n_seg variiert).
- Metriken: max/mean |f−d_wahr|, max |d_wahr|/|f| (Schranken-Güte), |∇h| per
  zentrale Differenzen, Überschätzung h−d_wahr (Marsch-Sicherheit).

### Ergebnisse

**E1 — Validierung + Schrankenfeld (200k bzw. 100k Samples):**

- d_mesh vs. analytische Ground Truth: **max Abweichung 4.4·10⁻¹⁵ mm** —
  Punkt-Dreieck+BVH ist auf Maschinengenauigkeit exakt (planare Faces, 20
  Dreiecke). H2 (planarer Teil) bestätigt.
- Schrankenfeld f = max(dA,−dB), Nahfeld |d|<5 mm:
  max|f−d| = **3.64 mm**, mean 0.041 mm, **max d/f = 3.83**.
- Notch-Region außen (x,y>0): max|f−d| = **3.74 mm**, max d/f = **3.99**,
  worst p=(19.69, 4.84, −11.25).
- Innen nahe der konkaven Kante (Differenz-Darstellung!): Fehler **exakt 0**.

**Befund, der H1 kippt (wichtigste Einzelerkenntnis der Iteration):**
Die Fehlstruktur des harten Differenz-Felds ist NICHT primär „konkave Naht ≈
√2". Im Slab-Bereich ist die Notch-Außenregion sogar **exakt** (jeder Punkt im
ausgeräumten Quadranten hat ein perpendikulares Lot auf eine der beiden echten
Faces; min(x,y) IST dort die wahre Distanz). Der große Fehler sitzt an
**Phantom-Flächen**: Randstücke des Subtrahenden B, die außerhalb von A liegen
(hier: B ragt in z um 2.5 mm über die Kappen von A hinaus, und in y über
A hinaus). Dort misst −dB die Distanz zu einer Fläche, die im Ergebnis gar
nicht existiert. Am worst point: f = 1.25 mm, wahr ≈ 5.0 mm → **Faktor 4,
unbeschränkt** in der Geometrie (je weiter der Subtrahend übersteht, desto
schlimmer) — die Schranken-Degradation ist geometrie-, nicht nur
verschachtelungsgetrieben.

**E1b — Offset-Konsequenz (Iso f=δ=2 mm), nur Notch-Region:** wahre Distanz
der Isopunkte ∈ [2.0000, 2.0000] — im Slab-Notch wäre ein Offset via f−δ
exakt. *(Sackgasse/Ehrlichkeit: dieses erste Sampling war irreführend eng —
es bestätigte scheinbar Unbedenklichkeit; erst der globale Survey E1d in
Iteration 2 zeigt das wahre Ausmaß.)*

**E2 — Tesselationsfehler (Zylinder, inskribiert, Nahfeld |d|<4):**

| n_seg | tris | max err (mm) | mean err (mm) | sag-Prognose r(1−cos(π/n)) |
|---|---|---|---|---|
| 16 | 288 | 0.19215 | 0.08925 | 0.19215 |
| 32 | 576 | 0.04815 | 0.02243 | 0.04815 |
| 64 | 1152 | 0.01205 | 0.00564 | 0.01205 |
| 128 | 2304 | 0.00301 | 0.00141 | 0.00301 |
| 256 | 4608 | 0.00075 | 0.00035 | 0.00075 |
| 512 | 9216 | 0.00019 | 0.00009 | 0.00019 |

Der maximale Distanzfehler des Proxys **ist** der Sehnenfehler, auf alle
gezeigten Stellen genau (max err ≡ sag bis zur 5. Nachkommastelle), Konvergenz
O(1/n²) ⇒ **d_mesh-Fehler = tol_mm der Tesselation, direkt steuerbar** über
`tessellate_brep(…, tol_mm)`. Scharfe (gerade) Kanten und planare Faces sind
**exakt** (E1: 10⁻¹⁵) — der strukturelle Vorteil gegenüber jeder
Gitter-Redistanzierung (ADF/FSM: O(h)-Schmieren an Kanten). H2 voll bestätigt.

**E3 — Kosten (1 Thread, f64, naiver Median-Split-BVH):**

| tris | build ms | queries/s | ns/query |
|---|---|---|---|
| 2 176 | 0.3 | 785 388 | 1 273 |
| 20 992 | 3.7 | 338 935 | 2 950 |
| 206 848 | 41.4 | 139 765 | 7 155 |
| 1 028 096 | 231.8 | 53 063 | 18 845 |

Selbst dieser bewusst simple BVH schafft 10⁵–10⁶ q/s; SAH-Split, f32,
Vektorisierung (fcpw) oder P2M bringen erfahrungsgemäß ≥5–10×. Für einen
Meshing-Pass sind nur Oberflächen-nahe Zellen Mesh-Queries (128³-Grid ⇒ ~10⁵
Nahband-Samples ⇒ <1 s single-thread bei 200k tris). H3 bestätigt: CPU-seitig
locker budgetierbar; per-Frame-Raymarching (10⁶–10⁷ Steps/Frame) bleibt beim
Schrankenfeld.

**E4 — Blend d_near↔f, h = mix(sign(f)·d_mesh, f, smoothstep(τ_in,τ_out,|f|)):**

| Band [τ_in, τ_out] | max ∇h | max Überschätzung h−d_wahr |
|---|---|---|
| [2, 3] | 14.06 | 0.0000 mm |
| [2, 4] | 6.99 | 0.0000 mm |
| [2, 6] | 3.31 | 0.0000 mm |
| [2, 10] | 1.55 | 0.0000 mm |

**Beide Teile von H4 bestätigt**: (a) h überschätzt die wahre Distanz nie
(beide Zutaten sind Unterschätzer bzw. exakt) ⇒ **sphere-trace-sicher
unabhängig vom Band**; (b) 1-Lipschitz ist h im Blendband nicht — der
Gradient spike't mit ≈ Δ/Bandbreite (Δ = Felddiskrepanz bis 3.7 mm). Wer
Lipschitz braucht (Normalen aus ∇h im Band), braucht Band ≳ 4× der maximalen
Diskrepanz — oder nutzt im Band einfach durchgehend d_near (Nahfeld < τ_in
ist ohnehin exakt, Normale dort = exakte Mesh-Gradienten).

### Schlussfolgerung Iteration 1

Der Hybrid-Pfad steht auf empirisch solidem Boden: Proxy-Distanz ist im
Nahfeld exakt bis tol_mm (Kanten exakt), Kosten CPU-tauglich, Blend
marsch-sicher. Die Folklore-Fehlercharakteristik des Schrankenfelds war in
zwei Richtungen falsch: konkave Differenz-Notches sind (in-slab) exakt, dafür
sind Phantom-Flächen-Regionen **schlechter als √2 — Faktor ~4 in einer
trivialen Zwei-Box-Szene**.

---

## Iteration 2 — Repräsentationsabhängigkeit, globaler Iso-Survey, Tiefe 2

### Hypothesen

- **H5**: Der √2-Fall existiert, aber **innen** und **repräsentationsabhängig**:
  derselbe L-Körper als *Union* zweier überlappender Boxen hat innen an der
  konkaven Ecke Fehlerfaktor √2 (Phantom-Innenflächen der Operanden).
- **H6**: Ein Offset über f−δ ist global unbrauchbar: die Isofläche f=δ liegt
  in Phantom-Regionen um ein Mehrfaches von δ von der wahren Fläche entfernt.
- **H7**: Bei Verschachtelungstiefe 2 bleibt der Worst-Case-Faktor ähnlich
  (er ist geometrie-, nicht tiefengetrieben), aber die betroffene Fläche
  wächst.

### Ergebnisse

**E1c — L als UNION (innen, nahe konkaver Ecke):**
max|f−d| = 1.4495 mm, mean 0.093 mm, **max |d|/|f| = 1.4142 = √2 exakt**.
H5 bestätigt: das Schrankenfeld ist keine Funktion der Geometrie, sondern des
CSG-*Rezepts* — derselbe Körper, zwei Felder, zwei Fehlerbilder (Differenz:
außen Faktor 4 an Kappen-Phantomen, innen exakt; Union: innen √2, außen exakt).
Ein Konsument kann dem Feld also nie ansehen, wo es lügt. Der Proxy ist von
der Repräsentation unabhängig — er sieht nur das Ergebnis.

**E1d — globaler Iso-Survey f=δ=2 mm (50k Bisektions-Treffer im ganzen
Domain):** wahre Distanz der f=2-Punkte ∈ [2.0000, **11.9373**] mm — der
per f−δ konstruierte „Offset" läge lokal um **+497 %** daneben (worst
p=(13.7, 11.9, −10.5), im Phantom-Lappen über A). Das revidiert das
irreführend milde E1b endgültig: **Offset/Shell auf dem Schrankenfeld ist
nicht „ungenau", sondern strukturell falsch** — die Isofläche hat Lappen, die
gar keine Offsetfläche des Körpers mehr sind. H6 bestätigt, drastischer als
erwartet.

**E5 — Tiefe-2-Differenz (zweiter Notch):**
max|f−d| = 3.72 mm, mean 0.063 mm, max |d|/|f| = 3.97 (worst wieder an einer
Kappen-Phantom-Ecke). H7 bestätigt: Der Worst-Case-Faktor bleibt ~4 (gleiche
Phantom-Mechanik), die mittlere Fehlerdichte wächst mit jedem Operanden, der
über das Ergebnis hinaussteht. Tiefe an sich ist nicht der Treiber —
überstehende Operanden sind es. (Konsequenz: „Operanden knapp schneiden"
würde das Feld verbessern, ist aber Modellier-Disziplin, die man Nutzern
nicht abverlangen kann.)

### Architektur-Skizze für FieldDraft (Stand der Evidenz)

Nahfeld-Korrekturpfad **d_hybrid** pro Body:

1. **Proxy** = `tessellate_brep(weave(body), tol_mm)` — existiert beides
   schon. tol_mm ist DIE Genauigkeitsstellschraube (E2: Fehler ≡ tol).
   Proxy-BVH einmal pro Regen im Weaver-Worker mitbauen (Build-Kosten E3:
   Millisekunden bis ~0.2 s bei 1M tris — im Async-Worker unkritisch).
2. **Query**: d_near(p) = sign(f(p)) · dist_BVH(p) — **Vorzeichen aus dem
   Feld** (bei harten Booleans exakt; die Nullmenge von f IST die Fläche,
   auch bei Blends), Betrag aus dem Mesh. Kein Winding-Number nötig, solange
   das Feld die SSOT bleibt; GWN (Barill 2018) nur als Fallback für den Fall
   „Feld nicht auswertbar / Soup-Import".
   Konsistenz-Kaveat: nahe der Fläche können sign(f) und Mesh um ≤ tol_mm
   auseinanderliegen ⇒ Diskontinuität ≤ 2·tol_mm direkt an der Fläche —
   unterhalb der Toleranz, mit der der Proxy ohnehin definiert ist.
3. **Konsumenten**:
   - Offset/Shell (Modellier-Op): im Band |f| < δ + Puffer ausschließlich
     d_near verwenden ⇒ Isofläche exakt bis tol_mm (E1d-Problem verschwindet
     per Konstruktion). CPU-seitig (Mesher/Weaver) unproblematisch (E3).
   - Meshing: Nahband-Samples über d_near (exakte Kanten! E1), Fernfeld f.
   - Analytische Abfragen (Measure, Mindestwandstärke): direkt d_near;
     später OCCT-Stil Newton-Polish auf der analytischen Face (Quelle 7),
     Kandidat aus dem BVH.
   - **Renderer bleibt beim Schrankenfeld**: f ist Unterschätzer ⇒
     sphere-trace-korrekt (nur langsamer); E4 zeigt, dass auch ein
     Hybrid-Feld marsch-sicher wäre, aber BVH-Traversal im
     WGSL-Interpreter ist ein eigenes Projekt (fcpw-Slang beweist
     Machbarkeit, s. u. „offene Frage Offset-Rendering").
4. **Blend**: smoothstep-Band auf |f|; marsch-sicher bei jeder Bandbreite
   (E4). Wenn ∇-Stetigkeit gebraucht wird: Band ≥ 4× max. Felddiskrepanz
   oder Konsument bleibt komplett im Nahband (w=0).
5. **Invalidierung**: identisch zum heutigen Async-Weave-Zyklus — Proxy-BVH
   ist ein Artefakt desselben Workers, gleicher Cache-Key, gleiches
   Staleness-Fenster (bis Weave fertig: Fallback aufs Schrankenfeld =
   heutiges Verhalten, keine Regression).

**Offene Kern-Frage (Offset-Rendering):** Ein Offset-*Feature* muss auch
gerendert werden. Optionen: (a) BVH-Distanz als neuer Opcode mit
Storage-Buffer im WGSL-Interpreter (machbar, fcpw-GPU als Beleg; Kosten/Frame
unklar), (b) Offset-Ergebnis erneut weben und als Mesh rendern, (c) baked
Nahfeld-Volumen nur für Offset-Bodies. Muss experimentell entschieden werden.

### Wo der Ansatz bricht (ehrlich)

- **Weave-Fehlschläge** (gibt es heute real): kein Proxy ⇒ Fallback
  Schrankenfeld = Status quo. Der Hybrid-Pfad macht nichts schlechter,
  liefert aber dann auch keine Korrektur. Offset auf einem Body ohne Weave
  müsste als „unscharf" gekennzeichnet oder verweigert werden.
- **Gyroid/TPMS**: kein sauberes B-Rep, Weave ausgeschlossen (heute schon
  explizit exkludiert). Dort bleibt nur Feld-Seite (Agenten A/B/D).
- **Offene Shells**: sign(f) existiert nicht sinnvoll; unsigned d_mesh geht,
  signed nicht — GWN hilft nur bei „fast geschlossenen" Shells.
- **Fillet-/Blend-Zonen**: Der Proxy korrigiert nur, was der Weaver webt.
  Ob die heutigen Edge-Fillet-Zonen (Kapsel-Maske) im Weave-Output als
  gerundete Faces auftauchen, habe ich NICHT verifiziert — wenn nicht, misst
  d_near dort die Distanz zur ungefilleteten Kante (Fehler ≈ Fillet-Radius,
  also groß). Muss vor einer Implementierung geklärt werden.
- **Tesselations-Overstepping**: inskribierte Meshes überschätzen die Distanz
  außen um ≤ sag; ein streng konservativer Marsch bräuchte d_near − tol_mm.

### Bewertetes Gesamtbild nach 2 Iterationen

| Hypothese | Status | Kern-Zahl |
|---|---|---|
| Proxy-Distanz exakt bis tol_mm, Kanten exakt | **bestätigt** | 4.4e−15 mm (planar), max err ≡ sag (Zylinder) |
| Schrankenfeld-Fehler „√2 an konkaven Nähten" | **verworfen/präzisiert** | Differenz in-slab: exakt; Phantom-Flächen: Faktor 3.99; Union innen: √2 exakt |
| Offset via f−δ reparabel durch Skalierung | **verworfen** | Isofläche f=δ bis 5.97·δ von der Fläche entfernt (+497 %) |
| Hybrid-Blend marsch-sicher | **bestätigt** | Überschätzung 0.0000 mm bei allen Bändern |
| Hybrid-Blend 1-Lipschitz | **verworfen (Band-abhängig)** | ∇h bis 14.1 (Band 1 mm) bzw. 1.55 (Band 8 mm) |
| CPU-Kosten tragbar | **bestätigt** | 0.14–0.79 M q/s (20k–200k tris, 1 Thread, naiver BVH) |

### Nächste Experimente (Vorschlag fürs gemeinsame Harness)

1. **Echter Weave-Proxy statt Hand-Mesh**: `tessellate_brep` auf einer echten
   Kernel-Szene (Box−Box, Box∪Zylinder, Body mit Edge-Fillet) laufen lassen,
   d_mesh gegen `eval_opcodes`-Feld und Brute-Force-Ground-Truth messen —
   prüft insbesondere, ob Fillet-Zonen im Weave-Output geometrisch stimmen
   (Haupt-Implementierungsrisiko, s. o.).
2. **Offset-End-to-End**: Offset-Feature einmal über d_near statt f−δ
   evaluieren (CPU-Mesher-Pfad), Iso-Positionsfehler vorher/nachher messen —
   erwartete Verbesserung: von bis zu ~6·δ auf ≤ tol_mm.
3. **GPU-Frage**: Mikro-Benchmark BVH-Traversal im WGSL-Interpreter (ein
   Opcode, Storage-Buffer) auf realer Szene — entscheidet Option (a) vs.
   (b)/(c) fürs Offset-Rendering.
