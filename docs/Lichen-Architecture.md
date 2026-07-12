# Lichen — Architektur, Erkenntnisse, Irrwege, offene Punkte

Dieses Dokument ist der lebende Begleiter zu `docs/Mesher-Konzept.md`. Das
Konzept-Dokument beschreibt das *Soll* (die Spezifikation der direkten
3D-Meshing-Pipeline); dieses Dokument beschreibt das *Ist*: wie Lichen
tatsächlich gebaut ist, welche Konzept-Teile sich bewährt haben, welche
ersetzt wurden, was wir dabei gelernt haben — und wo die offenen Baustellen
liegen. Stand: 2026-07-12 (v0.6.545).

---

## 1. Zweck und Einordnung

**Lichen** ist der experimentelle Debug-Oberflächen-Mesher von FieldDraft:
er trianguliert direkt im 3D aus **SDF + B-Rep**, ohne Volumengitter
(Marching Cubes o.ä.) und ohne globales (u,v)-Chart. Er lebt neben dem
PASM-Mesher (Hybrid-CDT) und dem CGAL-Mesher-Service (bestes Export-Mesh)
als **Explorationspfad**: Ziel ist ein einziger, chartloser Algorithmus, der
planare, gekrümmte und implizit-chartlose Flächen (Gyroid) mit demselben
Code wasserdicht vermascht. Erreichbar über **Debug ▸ Mesher: Lichen…**.

Dateien:

| Datei | Rolle |
|---|---|
| `crates/kernel/src/lichen.rs` | Wachstum (§1/§2 des Konzepts), Kanten-Resampling, `MeshQuality`, Orientierung, Pack-Format |
| `crates/kernel/src/lichen_tri.rs` | Allgemeine (chartlose) Pro-Flächen-Triangulierung: Stern-Konsens, Umkugel (Gabriel), Wächter, Vervollständigung, Loch-Füller |
| `crates/kernel/src/api/lichen.rs` | Orchestrierung + WASM-Entry `lichenMesh`: Seeds, Ketten, Flächen-Klassifikator, Feld-Streuung, Chart-CDT-Pfad, General-Pfad, Tests |
| `crates/kernel/src/mesher_cdt.rs` | Geteilter robuster CDT-Kern (spade) — planarer und periodischer Chart-Pfad, Loch-Polygon-Füllung |
| `web/src/MesherLichenDialog.tsx`, `LichenOverlay.tsx`, `lichenDebugStore.ts` | Dialog (Optionen + Qualitäts-Readout), Overlay (Punkte, U(p)-Kreise, Dreiecke, Defekt-Kanten), Store |

**Flat-Format** (WASM → Web): `[nPoints, nTris, nFallbackTris, points×9
(x,y,z,sdf,nx,ny,nz,U,flag), indices×3, nFaceRec, faceRec×4 (pointIdx,n),
nOpenEdge, (a,b)…, nWeakTri, (a,b,c)…, Qualitäts-Block×6 (boundary,
nonmanifold, degenerate, components, euler, orientViolations)]`.
Punkt-`flag`: 1 = Oberflächen-Partikel, 2 = Kantenzug, 3 = Ecke.

Zwei Triangulierungs-Pfade, im Dialog schaltbar (`General tri`):

* **Chart-Pfad** (default): pro B-Rep-Fläche `triangulate_planar_face`
  (Newell-Frame) bzw. `triangulate_periodic_face` (Zylinder/Kugel/Kegel über
  periodischen Chart). Robust, wasserdicht auf Box/Zylinder/Halbkugel.
  Seit v0.6.545 fällt jede chart-unfähige Fläche **pro Fläche auf die
  General-Triangulierung zurück** (Vollwickel-Kugel, Torus, Gyroid,
  loop-lose Flächen) — der Chart-Modus ist damit ein Hybrid „Chart wo
  möglich, chartlos wo nötig".
* **General-Pfad** (`lichen_tri.rs`): chartlos, Stern-Konsens + Umkugel.
  Das eigentliche Forschungsziel; die planaren Flächen sind dabei der
  Referenz-Testfall (dort gibt es mit dem CDT eine bekannte richtige Antwort).

---

## 2. Pipeline im Ist-Zustand

### 2.1 Seeds und Ketten (Konzept §1/§3.1–3.2)

1. **Ecken (flag 3):** B-Rep-Vertices, in Welt transformiert.
   **Mikro-Feature-Kollaps:** Ecken näher beieinander als der Min.
   Kantenabstand teilen sich EINEN Mesh-Vertex (Cluster-Repräsentant).
   Solche Häufungen entstehen bei streifenden Verschneidungen (r=30-Bohrung
   trifft Box-Kante bei 29.998) — Topologie unterhalb der Mesh-Auflösung,
   die sonst U(Ecke) auf µm drückt und Verdichtungs-Kaskaden auslöst.
   Mikro-Kanten innerhalb eines Clusters und Mikro-Ringe (Umfang <
   4·minSpacing) entfallen ersatzlos.
2. **Kanten (flag 2):** `resample_edge_by_curvature` arbeitet auf der
   **Original-Polyline** der B-Rep-Tessellierung (nicht auf einem
   Pre-Sampling, s. Irrweg I-1) in drei Pässen:
   * Pass 1 — Emission bei akkumulierter Bogenlänge ≥ h ODER akkumuliertem
     Knickwinkel ≥ α; Winkel-Emission respektiert `min_edge_spacing`;
     Trailing-Remnants am Kettenende werden getilgt.
   * Pass 2 — **Grading ≤ 1.5** zwischen Nachbarsegmenten, mit
     Ecken-Rand-Bedingungen (Pass A sammelt die Endabstände aller an einer
     Ecke mündenden Ketten) UND der harten **Überdeckungs-Bedingung**
     `min(dl,d) + min(d,dr) ≥ d` (ohne Floor — Geometrie feiner als
     minSpacing erzwingt lokal feinere Punkte).
   * Pass 3 — Bogenpositionen → Punkte (Interpolation auf der Polyline).
   * Geschlossene Ringe: ≥4 Punkte (Degenerations-Schutz) und
     **Ring-Naht-Kohärenz** (Mini-Restsegment am Nahtstoß wird entfernt,
     bis die Überdeckungsregel an der Naht hält).
3. **U(p) der Ketten-Punkte:** `u` = Abstand zum nächsten Ketten-Nachbarn
   (Anzeige-Kreis endet an der Kante), und — entscheidend — **`space` =
   echter Ketten-Nachbarabstand** (nicht h!). `space` steuert Stern-Horizont,
   Grading-Start des Wachstums und den Konflikt-Test (s. Irrweg I-6).
4. **Invariante (Test-Assertion in jeder Pipeline-Fahrt):** jedes
   Ketten-Segment ist von den U-Kreisen seiner Endpunkte überdeckt:
   `u_a + u_b ≥ d`.

### 2.2 Flächen, Klassifikator, verwaiste Flächen

* Pro analytischer B-Rep-Fläche wird ein `FaceSurf` (HalfSpace + transform +
  placement) gesichert; Rand-Loops entstehen durch Verkettung der
  Kanten-Ketten in Loop-Reihenfolge (geteilte globale Indizes → wasserdichte
  Nähte zwischen Flächen).
* **`face_of`-Klassifikator** (die „harte Kanten-Wand"): Welt-Punkt → Fläche
  mit kleinstem `|sdf_halfspace|`; `None` jenseits von 0.5·h. Wird vom
  Wachstum (on_face), von der Feld-Streuung und vom Owner-Stitching genutzt.
* **Verwaiste Branch-Flächen (F2a):** der Weave lässt Flächen aus, deren
  Trim-Nähte er droppt (DL-71 skippt non-Lipschitz-Gruppen). Der Träger
  steht trotzdem im `branch_halfspace_map`; Rand-Loops werden aus den
  angrenzenden Kanten-Ketten verkettet, kettenlose Flächen per
  **Feld-Streuung** besät (Gitter über die Szenen-Hülle + Newton auf den
  Träger, behalten wo `|f_body| ≈ 0` und `face_of` die eigene Region
  bestätigt; skalen-korrekt — Mini-Kugel mit placement-scale 0.02!).

### 2.3 Wachstum (Konzept §1, `grow_surface`)

Pro Fläche, Seeds = Rand-Punkte (mit Flächen-Normale und echtem
Rand-`space`) bzw. Streu-Punkte. BFS-Front, pro Punkt 6 hexagonale
Kandidaten im Radius `r = min(h, R_curv, R_reach)`, Referenzrichtung
entlang der Hauptkrümmung `d_max`:

* **Krümmung von der TRÄGERFLÄCHE** (`surf_curv` aus dem
  Halfspace-Normalenfeld), nie vom Body-∇f (s. Irrweg I-5).
* **Grading ≤ 1.5** gegenüber dem eigenen `space`.
* Kandidaten-Kaskade mit **Schritt-Halbierung** (≤5 Stufen) statt
  Verwerfen bei: SDF-Fehlprojektion (`|f| > 0.2·rr`), Sichtlinien-Verstoß,
  Kanten-Wand-Treffer, Redundanz. Die **Kaskaden-Bremse** hält `space` des
  neuen Punkts am vollen Radius (halbierte Punkte gebären sonst halbierte
  Kinder).
* **Harte Kanten-Wand `on_face`:** (a) Ziel liegt auf der eigenen
  Trägerfläche (`own_dist ≤ max(0.15·rr, 0.02·h)`), (b) **Ketten-Vorhang**:
  der Schritt p→cp kreuzt keine Kanten-Kette (2D-Test im
  robust_grad-Tangentialrahmen, Segment-Spatial-Hash).
* **Globale Konflikt-Quelle:** `existing = &all` — Punkte ALLER bisherigen
  Flächen zählen als Konflikt (tangentiale Nähte wachsen sonst
  0,04-mm-Doppelblätter → Folds).
* **§1.5-Konflikt-Rebalancing:** ein zu naher Kandidat wird nicht verworfen,
  sondern iterativ von den Konflikt-Nachbarn weggedrückt (Ziel ≈ 0.85·r),
  neu projiziert, und nur bei echtem Platzmangel verworfen.

### 2.4 U(p)-Komponenten (Konzept §2)

* `R_curv = α_rad/κ_max` — Krümmung per Shape-Operator (FD über das
  Trägerflächen-Normalenfeld, δ = 0.1·h geklemmt).
* `R_reach` — SDF-Außenmarsch mit Fußpunkt-Verfolgung (Fußpunkt springt >
  reach_tol → Medialachse gekreuzt).
* `R_BRep` fließt NICHT mehr in `space` ein (die harte Wand sichert die
  Kante; Dichte muss an Kanten nicht schrumpfen) — nur noch in den
  **Anzeige**-Radius `u = min(space, brep_dist)`.
* Floor = `min_edge_spacing` (NICHT 0.2·h, s. Irrweg I-4).

### 2.5 General-Triangulierung (`lichen_tri.rs`)

Pro Fläche, chartlos:

1. **Sterne:** pro Punkt Nachbarn im dilatierten Horizont
   `eff_r = 2.5 · max(space über die Nachbarschaft)` sammeln (symmetrische
   Zeugen-Mengen an Dichte-Übergängen), in die Tangentialebene projizieren,
   2D-**Constrained**-Delaunay (spade; lokale Ketten-Segmente als
   Constraints), Stern des Zentrums extrahieren → Stimme pro Dreieck.
2. **Nachbar-Filter beim Sammeln:**
   * **Cocone** (Amenta/Bern; vom Betreiber unabhängig erdacht):
     `|⟨d,n⟩| ≤ sin(2.5·α, clamp 15–60°)·|d|` — fremde Blätter
     (Gyroid-Gegenwand) liegen im Doppelkegel um die Normale und werden nie
     verbunden.
   * **SDF-Sichtlinie mit Normalen-Knick-Toleranz:** `|f(mid)| ≤
     max(0.3·L·θ, 0.1·space)` mit θ = Winkel der Endpunkt-Normalen — die
     erlaubte Eintauchtiefe ist exakt die Sagitta (L·θ/8-Ordnung). Parallele
     Normalen ≈ 0 erlaubt (fängt Durchquerungs-Sehnen), kippende Normalen
     erlauben genau die Kugel-Sehne.
   * **Regionen-Sichtlinie** (Doppelkappen-Problem): der auf die
     Body-Oberfläche projizierte Sehnen-Mittelpunkt muss `own_near` der
     eigenen Trägerfläche sein — Vorder→Rückkappen-Sehnen der in einer Wand
     steckenden Mini-Kugel sind aus Kugel-Sicht LEGITIME Sehnen
     (Normalen/Sagitta prinzipiell blind), projizieren aber auf die Wand.
3. **Akzeptanz** (Modus `auto`/`vote`/`gabriel`/`hybrid`):
   * `vote`: Dreieck in allen 3 Sternen (`vote_min = 3`).
   * `gabriel`: **Umkugel-Leere** — kleinste Kugel durch die 3 Ecken
     (Umkreis = Äquator) frei von anderen Punkten; Tie-Zeugen
     (`|d²−r²| ≤ 2e-4·r²`) → deterministischer **Fächer-Tie-Break** (Fächer
     vom kleinsten Index in zyklischer Winkelordnung ums Umkreiszentrum).
     **Umkreis-Deckel r ≤ 5·cell** (Perf UND Sinn: nie lokales Delaunay).
   * `auto` (Default): planar/abwickelbar (Plane/Zylinder/Kegel) → vote,
     doppelt gekrümmt → gabriel. Empirie: freie Kugeln sind mit gabriel
     wasserdicht (vote: 20–127 Löcher), die Box ist mit vote bei ~100
     offenen Kanten (gabriel: ~1200 — exakte Hex-Ties, Fächer-Partner ist
     oft kein Stern-Kandidat).
4. **Wächter nach der Abstimmung** (alle deterministisch):
   Owner-Stitching für Voll-Ketten-Dreiecke (Zentroid-`face_of`),
   Ketten-Kreuzungs-Verbot (2D in der Dreiecks-Ebene), fremde Ketten-Kanten
   als globale Wände, **Ketten-Kanten-Einzelnutzung** (Rand = genau 1
   Dreieck pro Fläche), **Gegenseiten-Pflicht** innerer Kanten (2 Dreiecke
   einer Kante auf Gegenseiten; tangential-ambivalente nie droppen).
5. **Kandidaten-Vervollständigung** (Teil des Basis-Algorithmus, KEIN
   Füller): offene Nicht-Ketten-Kanten werden geschlossen, wenn ein
   Umkugel-verifiziertes Dreieck existiert (billige Filter zuerst, ≤6
   Kandidaten verifiziert, ≤8 Runden bis Fixpunkt); das
   **Dreiecks-Loch-Prädikat** akzeptiert den Schließer eines exakten
   3er-Lochs auch ohne Umkugel (topologisch eindeutig — Gabriel lehnt an
   Dichte-Übergängen legitim fehlende Dreiecke ab).
6. **Loch-Füller** (Option, NACH der Diagnose): offene Kanten → gerichtete
   Loops (geometrischer Walker: schärfste Linkskurve), einfache Loops ≤32
   im Newell-Frame per CDT füllen; kapazitäts-bewusst; danach fremde Wände
   + Einzelnutzung erneut durchsetzen.
7. **Flächen-Treue:** Zentroid jedes Dreiecks muss auf der Oberfläche
   liegen (Sagitta-Toleranz aus dem Normalen-Knick) UND auf die eigene
   Trägerfläche projizieren.
8. **Orientierung:** Winding an der Flächen-Normale, dann global
   `orient_consistent` (BFS über Manifold-Kanten; **Fold-Auflösung**: pro
   Konflikt-Paar fliegt deterministisch das Dreieck mit größerem Umkreis,
   ≤4 Runden — ehrliches Loch statt falschem Fold).
9. **Diagnose-first (Betreiber-Prinzip):** alles wird GEZÄHLT und im
   Overlay gezeigt (offene Kanten rot, Fast-Konsens-Kandidaten orange,
   Qualitäts-Block im Dialog), nichts still übermalt. Der alte
   Advancing-Front starb genau an stillen Qualitäts-Gates.

---

## 3. Abweichungen vom Konzept-Dokument

| Konzept | Ist | Grund |
|---|---|---|
| §5 Advancing-Front-Mesher | **ERSETZT** durch Stern-Konsens/Umkugel (General) bzw. CDT (Chart) | Greedy Front sammelt an fast-cozirkulären Hex-Quads widersprüchliche Diagonalen-Münzwürfe ein → Stalls, Löcher; komplett entfernt (v0.6.485) |
| §3 globaler Graph *G* | Kein expliziter Graph; Nachbarschaft entsteht implizit pro Stern (Spatial-Hash + Filter) | Der Graph war an den alten Front gekoppelt; die Filter (§3.4) leben als Sichtlinie/Cocone/Region weiter |
| §1.6 Adjazenz-Registrierung | Implizit über Stern-Horizont `eff_r` | dito |
| §2 `U(p) = min(curv, BRep, reach)` | `space` OHNE R_BRep (harte Wand ersetzt Dichte-Schrumpf an Kanten); `u` (Anzeige) MIT R_BRep | Uniformere Abtastung, Graph bleibt an Kanten zusammenhängend |
| §4 Umkreis „im Graphen verknüpfte D" | Umkugel-Scan über Spatial-Hash + Fächer-Tie-Break + 5·cell-Deckel | Graph existiert nicht mehr; Ties sind real (Hex-Muster) und brauchen den deterministischen Break |
| Waben-Konfliktauflösung §1.5 | Umgesetzt (Rebalancing), zusätzlich Schritt-Halbierung für Sub-h-Features | Konzept sah nur Ausbalancieren vor; Sub-h-Features brauchten den halben Schritt |

---

## 4. Feine Details, die man wissen muss

* **`u` vs. `space`:** `u` ist NUR Anzeige (Kreis endet an der B-Rep-Kante),
  `space` ist die funktionale Größe (Horizont, Grading, Konflikte). Wer die
  U(p)-Kreise im Overlay ansieht, sieht also nicht direkt den
  Stern-Horizont (der ist `2.5 · max(space der Nachbarschaft)` — deutlich
  größer).
* **`sdf_gradient` ist EXAKT null auf planaren Flächen** (Abs-Dual bei 0);
  Newton konvergiert genau dorthin. JEDER Konsument braucht
  `lichen::robust_grad` (FD-Fallback) — der [0,0,1]-Default hat historisch
  die halbe Pipeline vergiftet (s. Irrweg I-3).
* **Flächen-Normalen der Sterne kommen von der Trägerfläche** (exakt),
  nicht aus `LPoint.n` — Rausch-/Default-Normalen machten den Cocone auf 4
  von 6 Box-Flächen blind (leere Sterne, 0 Dreiecke, „72 Löcher" waren in
  Wahrheit ein halb leeres Netz).
* **Ketten-Punkte gehören BEIDEN Nachbarflächen** → Dreieck aus 3
  Ketten-Punkten entsteht in beiden Sternläufen; Dedup über sortierte
  Tripel + Owner-Stitching sind Pflicht.
* **Ein Sliver aus 3 aufeinanderfolgenden Kettenpunkten hat sein Zentroid
  AUF der Kante** — ein SDF-Klassifikator kann ihn nicht zuordnen; die
  Ketten-Kanten-Einzelnutzung ist der richtige Mechanismus, nicht ein
  Klassifikator-Tuning.
* **Perf:** Kanten-Segmente leben in einem Spatial-Hash (Zelle h);
  `brep_dist` ist auf h gedeckelt; `nearest_pt` ist radius-beschränkt
  (Zelle h/4). Bei „zu langsam" IMMER zuerst Phasen-Stoppuhren
  (`LICHEN_PERF`, nur nativ!) — der 55×-Fresser war eine 5-Zeilen-Stelle
  (Umkreisradius-Explosion fast-kollinearer Gabriel-Kandidaten), nicht die
  Architektur.
* **Kein `std::time` im Kernel-Produktionspfad** — `Instant::now` panict
  auf wasm32; ein Panic in einer `&mut`-WASM-Methode vergiftet das
  wasm-bindgen-Borrow-Flag dauerhaft (Session tot bis Reload). `lichenMesh`
  ist deshalb `catch_unwind`-gesichert.
* **Suite-Hinweise:** Web-Vitest immer aus `web/` starten (Root sammelt
  Playwright-Specs ein). `LICHEN_DEBUG_GROW/TRI/FACES/CHAIN/ORIENT`,
  `LICHEN_ACCEPT`, `LICHEN_VOTE_MIN`, `LICHEN_COCONE_DEG`, `LICHEN_NO_FILL`
  sind die Mess-Hebel.
* **App-Version im Titel prüfen**, bevor Screenshots interpretiert werden —
  Betreiber-Screenshots eines alten Builds haben schon einmal einen bereits
  gefixten Stand „widerlegt".

## 5. Irrwege (vermessen und verworfen — nicht wiederholen)

* **I-1 Uniformes Kanten-Pre-Sampling (0.15·h):** zerstörte kleine, stark
  gekrümmte Kanten (6-mm-Kreis bei h=10 → 4 Punkte, bevor der Winkel-Test
  messen konnte). → Knickwinkel auf der Original-Polyline akkumulieren.
* **I-2 Eigener Advancing-Front (§5):** stallte an Feature-Kollisionen und
  produzierte Löcher, weil Hex-Wachstum den maximal entarteten
  Delaunay-Fall (cozirkuläre Quads) serienmäßig liefert. Komplett entfernt.
* **I-3 Body-∇f als Normalen-/Krümmungsquelle:** (a) exakt 0 auf planaren
  Flächen (Abs-Dual) → Default-Normalen überall; (b) misst an B-Rep-Kanten
  den KNICK als Krümmung (Mikro-Spacing-Bänder, 15× Überdichte); (c)
  rauscht bei kleinem FD-Fenster (κ≈1 statt 0.125 am Zylinder-Mantel).
  → robust_grad + Trägerflächen-Normalenfeld.
* **I-4 0.2·h-Floor des Wachstumsradius:** verwarf die gesamte
  Krümmungs-Verfeinerung auf kleinen Teilen (6-mm-Teil bei h=10: genau EIN
  Innenpunkt). → Floor = min_edge_spacing.
* **I-5 Distanz-argmin als Kanten-Wand:** an tangentialen Verschneidungen
  (Gyroid ∩ Box-Ebene fast parallel) ein Münzwurf — beide Flächen über
  mm-Streifen gleich nah, fast jeder Kandidat gefressen. Eine bloße
  Mit-Minimal-Toleranz wurde vermessen und verworfen (Demo 148→229 +
  non-manifold). → eigene-Fläche-Test + Ketten-Vorhang.
* **I-6 Ketten-Punkte mit `space = h`:** ein 0.2-dichter Kettenpunkt
  „beanspruchte" 10 mm und erklärte ganze Mikro-Patches für redundant
  (Wachstum auf Wand-Flächen tot). → space = echter Nachbarabstand.
* **I-7 SoS-Jitter (Simulation of Simplicity):** verschlechterte die Box
  von 103 auf 1762 Löcher — spade entscheidet exakte Hex-Ties selbst
  konsistent; Jitter macht daraus asymmetrische Fast-Ties, die am
  Stern-Horizont pro Stern verschieden kippen.
* **I-8 Pauschale Sichtlinien-Toleranz (0.15·L):** kann Durchquerung
  (f<0 durch Material) nicht von legitimer Kugel-Sehne (f<0, Tiefe L²/8r)
  unterscheiden. → Toleranz aus dem Normalen-Knick (Sagitta).
* **I-9 Normalen/Sagitta gegen Doppelkappen:** prinzipiell blind — die
  Sehne Vorderkappe→Rückkappe ist aus Trägerflächen-Sicht legitim. →
  Regionen-Sichtlinie (projizierter Mittelpunkt muss auf der eigenen
  Fläche landen).
* **I-10 vote≥2 statt 3:** schlechter (108 statt 72 auf der Box) — der
  Konsens lebt von der Einstimmigkeit.
* **I-11 AABB-Gate gegen Union-Fusion (DL-73-Kontext):** falscher Stich —
  vor dem Beschuldigen eines räumlichen Gates verifizieren, dass sich die
  Solids wirklich berühren. (Lehre gilt generell.)
* **I-12 Radius-relative Projektions-Toleranz (bis v0.6.543):** die
  Wachstums-Akzeptanz `|f| ≤ 0.2·rr` skalierte mit dem Wachstumsradius —
  bei h=10 waren 2 mm „auf der Oberfläche"; Newton stallt an Distanzfeld-
  Graten genau dort → Luft-Punkte neben dem Körper, die zusätzlich die
  Stern-Nachbarschaften vergifteten (Demo 165→30 boundary nach dem Fix).
  → absolutes Oberflächen-Gate `surf_tol = max(5e-4·h, 1e-4)` mit
  Nach-Projektion.
* **I-13 Lokale Frame-Einheiten in Distanz-Schwellen (bis v0.6.544):**
  `face_of`/`own_dist` verglichen `sdf_halfspace`-Werte verschiedener
  Flächen ROH — ein Träger mit placement-Skala 0.02 (Mini-Kugel = Kugel
  r50 × 0.02) liefert 50× aufgeblasene Distanzen; argmin und alle
  Schwellen wirkten pro Fläche verschieden. → immer auf Welt-mm skalieren.
* **I-14 `on_face`-Floor 0.02·h (bis v0.6.544):** 0.2 mm Toleranz bei h=10
  machte die harte Kanten-Wand für Sub-h-Features porös — die Gyroid-Wand
  wuchs bis 0.2 mm neben ihren Träger quer über das Band der Mini-Kugel;
  ihre Übertreter blockierten als xgrid-Konflikte die echte Besiedlung.
  → Floor 2e-3·h.
* **I-15 Case 1 des periodischen Charts ohne Vollwickel-Schutz (bis
  v0.6.544):** eine geschlossene Kugel mit zwei kleinen, NICHT windenden
  Loch-Ringen (Wand steckt in der Kugel) wickelt die volle Periode — Case 1
  hat keine Naht-Maschinerie, am Chart-Schnitt blieb ein leerer Meridian-
  Streifen, den die Chart-Hülle mit langen degenerierten Welt-Sehnen
  überbrückte (Betreiber-Bild „Spalt in der Kugel"). → u-Überdeckungs-
  Wächter (keine Lücke ≥ 5% der Periode → `None` → chartloser Fallback).

## 6. Lessons Learned (Arbeitsweise)

* **„X Löcher" IMMER gegen tris-pro-Fläche gegenprüfen** — 0-tris-Flächen
  erzeugen 0 Löcher; ein halb leeres Netz sieht in der Loch-Statistik
  besser aus als ein fast fertiges.
* **Diagnose vor Filter:** Zähler + Overlay (rote offene Kanten, orange
  Fast-Konsens) haben jede Wurzel in Minuten gezeigt; stille Gates haben
  den alten Front getötet.
* **Ablation nutzen:** Demo-Varianten (ohne Gyroid / ohne Mikro-Features)
  isolieren Ursachen sofort.
* **Screenshots + Dialog-Zähler des Betreibers = Repro-Gold** — aber
  Versions-Badge prüfen.
* **Gates mit Betreiber-Settings fahren** (5°/0.2), nicht nur mit den
  gemütlichen 20°-Defaults — die Fold-Front war NUR bei dichten Settings
  sichtbar.
* **Erst billig filtern/sortieren/deckeln, dann teuer verifizieren** —
  Umkugel-Scans im Debug-Build sind teuer.
* **Gates können False Positives sein — Prämisse prüfen, nicht nur das
  Assert:** der Halbkugel-Watertight-Test war seit langem grün, obwohl die
  Kugelkappe NIE gemesht wurde: `add_primitive(TAG_REMOVE)` hängt ohne
  `targets` kein CSG an — der Test baute zwei getrennte Bodies, und nur
  die geschlossene Box erfüllte χ=2. Aufgeflogen erst, als der neue
  Fallback die freie Kugel plötzlich meshte und „den grünen Test brach".
  Konsequenz: bei Gate-Brüchen nach einem Fix zuerst fragen, ob der Test
  je das gemessen hat, was sein Name behauptet (verwandt mit „X Löcher
  gegen tris-pro-Fläche gegenprüfen").

## 7. Dedizierte Tests (Stand v0.6.543)

Aktive Gates (`crates/kernel/src/api/lichen.rs`):

* `lichen_box_mesh_is_watertight`, `lichen_cylinder_mesh_is_watertight`,
  `lichen_hemisphere_mesh_is_watertight` — Chart-Pfad geschlossen (χ=2).
* `lichen_general_free_spheres_gabriel_watertight_no_fallbacks` — freie
  Kugeln r1/r8/r50, General-Pfad, OHNE Füller.
* `lichen_general_tri_cylinder_is_watertight`,
  `lichen_general_tri_box_smoke_no_nonmanifold`.
* `lichen_demo_project_{chart,general}_quality` — Betreiber-Demo (Kugel∩
  Gyroid, Bohrung, Mikro-Features) mit Ceiling-Gates (0 nm / 0 degen /
  0 orient; boundary-Budget).
* `lichen_demo_user_settings_orientation_clean` — dichte Betreiber-Settings
  (5°, 0.2), 0 Folds (teuerster Test, ~4 min).
* `growth_refines_below_h_on_small_curved_part`, `lichen_gyroid_seam_rings_run_clean`,
  Format-/Normalen-Tests.
* Überdeckungs-Invariante `u_a+u_b ≥ d` als Assertion in jeder Testfahrt.

`#[ignore]`-WIP (die ehrliche Watertight-Front):
`lichen_general_tri_box_is_watertight`, `lichen_demo_project_is_watertight`.

## 8. Offene Punkte

*(Erledigt am 2026-07-12: Luft-Punkte im Wachstum → I-12/v0.6.544;
Kugel-Meridian-Spalt + „Gyroid ohne Mesh bei General-tri-aus" →
I-13/I-14/I-15 + Chartlos-Fallback/v0.6.545. Demo-Messstand: chart 15,
general 36 offene Kanten, beide 0 nm / 0 degen / 0 orient.)*

1. **Stern-Projektions-Entartung auf stark gekrümmten Flächen** (Betreiber-
   Repro curv 1°/minEdge 0.1): Innen-Sterne der r=1-Kugel haben Horizonte
   von mehreren RADIANT Bogen — die Tangential-Projektion ist dort entartet,
   Innen- und Ring-Sterne wählen verschiedene Partner (78 chain↔surf-Löcher
   Ring→Kappen-Innen). Dazu der unaufgeklärte **Widerspruch: warum wachsen
   auf der r=1-Kugel bei curv=1° Punkte mit space≈2 statt α/κ≈0.017?**
   (space-Kette prüfen; das früher beobachtete leere ~1-mm-Band war
   vermutlich I-14 — nach dem Fix neu vermessen.)
2. **Rest-Löcher an Dichte-Übergängen/Gyroid-Nähten:** chart 15 / general
   36 offene Kanten auf dem Demo (`lichen_demo_project_is_watertight`
   bleibt #[ignore]-WIP). Umkugel-Bedingung auf dem Gyroid vielerorts
   verletzt (Betreiber-Bild 5) — vermutlich Punktverteilung, s. R-A1/R-A3.
3. **Torus (doppelt periodisch)** hat kein Chart — läuft jetzt über den
   General-Fallback, aber ohne dediziertes Gate.
4. **Wachstums-Überdichte / CDT-Verwurf randnaher Partikel** (Chart-Pfad):
   usedV≈384 von 879 auf der Box — kosmetisch, aber Dichte-Feinschliff
   steht aus.
5. **Wurzel-Panic der WASM-Borrow-Vergiftung** (v0.6.542) nicht
   reproduziert — Konsolen-Zeile ÜBER dem Aliasing-Fehler nötig; evtl.
   Geometrie-Edit während der Mesher lief. `catch_unwind`-Muster ggf. auf
   andere `&mut`-Entries ausweiten.
6. **`own_near`-Schwelle (Regionen-Sichtlinie) ist frame-lokal:**
   `sdf_halfspace ≤ 0.05·h` in LOKALEN Einheiten — für die 0.02-skalierte
   Mini-Kugel effektiv 0.01 mm (streng, korrekt), für skala-1-Flächen
   0.5 mm (lax). Funktioniert derzeit, ist aber dieselbe Fehlerklasse wie
   I-13; sauber wäre eine Spacing-relative Welt-Toleranz.
7. **Export-Perspektive:** seit dem CGAL-Service ist Lichen fürs
   Export-Mesh evtl. obsolet — er bleibt der Explorationspfad für
   chartloses direktes 3D-Meshing.

---

## 8b. Analyse-Durchbruch: die fehlende Säule ist die Manifold-Extraktion (2026-07-12)

Auf Betreiber-Wunsch wurde der **pure Basis-Algorithmus** (Umkugel-Graph +
Delaunay, ohne Chart-Optimierung, ohne Loch-Füller) systematisch vermessen,
BEVOR etwas geändert wurde. Werkzeuge: `zz_basis_matrix` (Körper × Akzeptanz-
Modi) und die Loch-Forensik `LICHEN_WHY` (für jede offene Kante: welcher
Wächter blockiert das fehlende Dreieck?).

**Befund 1 — die Grundlage trägt.** Box, Zylinder, freie Kugel und Torus sind
bei 5° in JEDEM Akzeptanz-Modus wasserdicht (0 offen / 0 nm / 0 orient), ohne
jede Reparaturstufe. Der Basis-Algorithmus ist nicht das Problem.

**Befund 2 — Verfeinern hilft NICHT.** Auf dem Demo-Projekt steigen die
offenen Kanten mit feinerer Abtastung (minEdge 0.1 → 0.02: 93 → 442). Die
naive ε-Sampling-Hypothese („mehr Punkte heilen es") ist damit **widerlegt**.

**Befund 3 (die Wurzel) — Kapazitäts-Konflikte mit GÜLTIGEN Blockern.** Für
~85 % aller offenen Kanten existiert ein kugel-gültiger dritter Punkt; er
wird von der Kanten-KAPAZITÄT blockiert (die Nachbarkante trägt schon zwei
Dreiecke) — und die blockierenden Dreiecke sind **selbst gültig**. Es gibt
also systematisch MEHR gültige Dreiecke, als in ein 2-Manifold passen.
Greedy akzeptieren + Kapazität klemmen ist strukturell falsch: aus dem
Kandidaten-Komplex muss eine FLÄCHE **extrahiert** werden. Das ist exakt
Cocones dritter Schritt (Manifold-Extraktion), den wir nie hatten — nicht
ein Prädikat- und nicht ein Sampling-Problem.

**Konsequenz (v0.6.548):** `lichen_tri::extract_manifold` — Sheet-Walk über
gerichtete Halbkanten; an jeder Front-Kante gewinnt der Punkt mit **kleinstem
Sweep-Winkel um die Kantenachse** (Drehung durch die Luft = Ball-Pivoting-
Regel). Pivotiert wird über ALLE Punkte im Horizont, nicht nur über
Stern-Kandidaten (die sind nachweislich unvollständig — genau deshalb gab es
die Vervollständigungs-Passe). Ergebnis: per Konstruktion 2-manifold und
konsistent orientiert; Ketten-Kanten sind Flächenrand, die Front endet dort.
**Box, Zylinder, Kugel und Torus sind damit wasserdicht OHNE Kandidaten-
Vervollständigung, OHNE Kapazitäts-Klemme und OHNE Loch-Füller** (Gate
`lichen_walk_extraction_watertight_on_smooth_bodies`).

Dazu die **Zeugen-Umkugel** (`accept: "witness"`, Betreiber-Idee „umgekehrtes
Raymarching"): das Kugelzentrum wird per 1D-Newton entlang der Dreiecks-
Normalen auf f = 0 geschoben — das ist das Prädikat des *restricted*
Delaunay (und damit des Amenta/Bern-Beweises), während unsere Gabriel-Kugel
(kleinste Kugel) strenger ist und auf gekrümmten Flächen legitime Dreiecke
ablehnt. Auf planaren Flächen degeneriert sie exakt zu Gabriel.

**Offen:** Das Demo-Projekt (Gyroid + tangentiale Mikro-Features) profitiert
noch nicht (43 → 43 im gabriel-Walk, 26 im vote-Walk). Dort greift die
theoretische Grenze: an tangentialen Kontakten geht lfs → 0, ε-Sampling
verlangte unendliche Dichte. Nächster Schritt ist deshalb NICHT mehr
Abtastung, sondern der verallgemeinerte **Mikro-Feature-Kollaps**
(Sub-Resolution-Topologie kollabieren, wie schon an Ecken-Clustern) —
plus der Rückbau des Wächter-Turms, der mit der Extraktion großteils
redundant wird.

## 8c. Globaler Walk + ε-Sampling (v0.6.549)

**Befund 4 — die Restlöcher sind NAHT-SCHLITZE, keine Sampling-Defekte.** Die
offenen Kanten der per-Fläche-Extraktion bilden **geschlossene Loops** (Gyroid
ein 31-Kanten-Loop, Bohrung 11+13, alle Knoten Grad 2). Ursache: wo der Weave
die Trim-Naht droppt (non-Lipschitz-Gruppen, DL-71), existiert keine geteilte
Punkt-Kette — beide Nachbarflächen enden mit freiem Rand, ihre Randpunkte sind
VERSCHIEDEN, im Netz bleibt ein Schlitz. Weder ein anderes Prädikat noch mehr
Punkte können das schließen: **die Flächen-Zerlegung selbst ist der Defekt**
(bestätigt: feineres h ändert nichts, die Dichte ist gar nicht h-getrieben).

**Globaler Walk** (`globalWalk`, `lichen_tri::triangulate_global_walk`): EIN
Netz über die gesamte Punktwolke, keine Flächen-Zerlegung. An echten
B-Rep-Kanten liegen die Ketten-Punkte ohnehin in beiden Flächen (geteilte
Indizes) — der Walk läuft hinüber und reproduziert die Kante exakt (der
Sweep-durch-die-Luft trifft zwangsläufig zuerst den Nachbarflächen-Punkt).
Ketten-Kanten bleiben Kreuzungs-Verbot (Feature-Linien), sind aber **keine
Wände** mehr. Der Cocone-Kegel muss dafür weit sein (60–80°), sonst blockiert
er die Kanten selbst; die Blatt-Trennung leisten Sichtlinie + Zentroid-Test.
**Gate `lichen_global_walk_watertight_on_smooth_bodies`: Box, Zylinder, Kugel
χ=2 und TORUS χ=0** — der Torus (im Chart-Pfad nie lösbar, doppelt periodisch)
fällt als Nebenprodukt ab, und die Genus-1-Topologie wird selbst erkannt.

**ε-Sampling gegen die lfs** (`lfsFactor`, `lichen::reach_inner`): Das
Konzept (§2.3) ließ den Marsch INS MATERIAL „zur Performance-Optimierung" weg
und überließ dünne Wände dem Sichtlinien-Test. Das ist die zentrale falsche
Annahme (R-A2): die local feature size, gegen die das Umkugel-Theorem
formuliert ist, ist die Distanz zur Medialachse — **innen wie außen**. Auf der
1-mm-Gyroid-Wand ist lfs ≈ 0,5 mm bei Spacing 0,2 (ε = 0,4 statt ≤ 0,1): die
leere Umkugel **erreicht das Gegenblatt**. Die Flächen-Zerlegung hat das nur
maskiert; der globale Walk legt es offen. Erkennung der Medialachse über die
**Abflachung des Distanzfelds** (`−f(p − t·n) < 0.9·t`) statt über
Fußpunkt-Verfolgung — die bleibt auf dünnen Wänden unter `reach_tol` und
detektiert gar nichts. Gate: 1-mm-Wand 103 → 33458 Punkte.

**Offen:** Das Demo-Projekt bleibt im globalen Walk offen (relativer Randanteil
sinkt mit ε-Sampling von 29 % auf 11 %, absolut noch nicht dicht). Die
verbleibende Front ist damit sauber isoliert: **tangentiale Kontakte**
(Mini-Kugel in der Wand, Gyroid ∩ Box fast parallel), wo lfs → 0 geht und
kein ε die Bedingung mehr erfüllen kann. Dafür ist der verallgemeinerte
**Mikro-Feature-Kollaps** die theoretisch richtige Antwort (Sub-Resolution-
Topologie kollabieren statt abtasten), nicht mehr Dichte.

## 8d. Kollaps-Hypothese widerlegt, Feature-Protection, Walk-Grenze (v0.6.550)

**Befund 5 — der Mikro-Feature-Kollaps war die falsche Fährte.** Vor dem Bau
gemessen (`zz_duplicate_sheet_stats`): auf dem Demo gibt es **NULL** Punktpaare
näher als 0,5·space — weder mit gleich- noch mit gegenläufiger Normale. Die
globale Konflikt-Quelle des Wachstums (xgrid, I-6-Serie) verhindert
Doppelblätter längst; es gibt schlicht nichts zu kollabieren. Hypothese
verworfen, ohne eine Zeile Produktionscode dafür geschrieben zu haben.

**Befund 6 — die Medialachse läuft in jede scharfe Kante.** Das ε-Sampling in
seiner ersten Fassung zerstörte den Würfel (263 → 19 097 Punkte, 3 565 offene
Kanten): der Bisektor zweier Nachbarflächen **berührt** die gemeinsame Kante,
also geht lfs dort gegen 0 und die Verdichtung ist unbegrenzt. Genau deshalb
*protecten* Cheng/Dey/Ramos scharfe Features. Fix: `reach_inner` zählt nur
noch echte **Gegenblätter** (Fußpunkt-Normale antiparallel, `dot < −0.3`);
scharfe Kanten sind über ihre B-Rep-Ketten ohnehin exakt abgetastet. Zweiter
Fehler derselben Fassung: der Marsch gab bei „nichts gefunden" seinen Cap
zurück, wodurch ε **systematisch jede** Fläche refinete — jetzt bindet die
Bedingung nur bei tatsächlich gefundener Medialachse (Marsch-Cap `h/ε`).
Gate: dünne Wand 103 → 9 279 Punkte, **dicker Würfel punktgleich**.

**Befund 7 — die Grenze des greedy Walks.** Auf der Demo sind 92 % der offenen
Kanten des globalen Walks vom Typ „**voll gültiges Dreieck, aber Halbkante
belegt**" (Forensik `LICHEN_WHY`): der Walk legt sich selbst lahm. Best-first-
Front (kleinster Umkreis zuerst statt FIFO) und das Verbot von Rogue-Seeds
(ein Startdreieck darf keine schon benutzte Kante berühren) beheben das auf
allen glatten Körpern, **nicht** auf der Demo — dort bleibt der Randanteil bei
~17 % und **skaliert mit der Netzgröße, unabhängig von der Abtastung** (ε von
0,5 bis 0,15 und minEdge bis 0,02 vermessen: keine Konvergenz).

**Damit ist die nächste Front präzise benannt:** nicht Abtastung, nicht
Prädikat, nicht Kollaps — sondern die **Extraktion selbst**. Der greedy
Sheet-Walk ist eine Approximation von Cocones Extraktions-Schritt; die echte
Fassung arbeitet nicht greedy, sondern über den **globalen Kandidaten-Komplex**
(alle restricted-Delaunay-Dreiecke) mit **Umbrella-/Zirkularordnungs-Pruning**
pro Vertex. Das ist der letzte fehlende Baustein des Programms.

## 8e. Normalen-Guard: die prinzipielle Lücke des Cocone-Filters (v0.6.551)

**Befund 8 (Betreiber-Idee, vermessen und bestätigt).** Der Cocone-Filter prüft
die **Richtung** zum Nachbarn: `|⟨d, n_i⟩| ≤ sin(θ)·|d|` — der Nachbar muss
fast in der Tangentialebene liegen. Bei zwei **fast parallelen Blättern** im
Abstand *t* ist eine Sehne quer über die Wand zu einem lateral um *L*
entfernten Punkt aber nahezu tangential (Winkel ≈ `atan(t/L)`, für *L ≫ t*
beliebig klein). **Cocone kann diese Kante prinzipiell nicht sehen** — erst
recht nicht mit dem weiten Kegel (60–80°), den der globale Walk für scharfe
Kanten braucht. Das ist exakt die Gyroid-∩-Wand-Situation.

Das verlässliche Kriterium ist der Winkel zwischen den **Normalen der beiden
Endpunkte**: quer durch eine dünne Wand sind sie ~antiparallel, egal wie
tangential die Sehne liegt. Die Abtast-Bedingung garantiert, dass sich die
Normale pro Schritt um höchstens den Krümmungswinkel α dreht; bei einem
Horizont von ~2,5 Schritten ist `k·α` die natürliche Schwelle
(implementiert: 6·α, geklemmt 30–75°, Env `LICHEN_NORMAL_GUARD_DEG`).

**Kanten-Sonderbehandlung** (der Kern der Idee): an einem Knick ist *die eine*
Normale bedeutungslos (der Mittelwert zeigt in die Winkelhalbierende). Geprüft
wird deshalb über die **Normalen-MENGE** je Punkt — eine pro angrenzender
Fläche (Flächenpunkte tragen eine, Kantenpunkte zwei, Ecken ≥3; die Mengen
existierten längst als `point_faces` fürs Overlay). Die Kante ist gültig, wenn
**ein Paar** `(n_i ∈ N(i), n_j ∈ N(j))` innerhalb der Schwelle liegt: an einer
Box-Kante passt die Normale der gemeinsamen Fläche, quer durch eine dünne Wand
passt keine.

**Messung (1-mm-Platte, globaler Walk):** ohne Guard **111 Wand-Durchquerungen**
und **χ = −67** (34 falsche Henkel quer durch die Platte); mit Guard **0**
Durchquerungen, χ = 0. Glatte Körper bleiben unverändert (Box/Zylinder/Kugel
χ=2, Torus χ=0) — die Normalen-Mengen halten die scharfen Kanten. Gate
`lichen_thin_plate_global_walk_is_watertight` prüft genau diese Eigenschaft und
fällt ohne den Guard um.

**Unverändert offen:** die Platte ist damit noch nicht *dicht* (556 Rand-Kanten)
— das ist die Walk-Sackgasse aus §8d, nicht der Guard. Die Trennung der beiden
Probleme ist jetzt aber sauber: **Blatt-Trennung = gelöst (Normalen-Guard),
Extraktion = offen (Umbrella-Pruning statt greedy Walk).**

## 8f. Ketten-Sliver, Forensik-Korrektur, revidierte Diagnose (v0.6.552)

**Befund 9 — der Sliver mit Umkreisradius NULL.** Auf der dünnen Platte
blockierte ein einziger Dreieckstyp die Fächer *sämtlicher* Ketten-Punkte: der
**Sliver aus drei aufeinanderfolgenden Kettenpunkten** (kollinear,
Kantenlänge ~0,1 mm). Sein Umkreisradius rechnete numerisch zu **0** — der
Entartungs-Test war ABSOLUT (`den < 1e-20`) und greift bei kleinen Koordinaten
nicht. Damit galt der Sliver in **jeder** Umkreis-Sortierung als „bester"
Kandidat (Seed-Wahl, Front-Priorität, Konflikt-Auflösung) und saturierte die
Ketten-Kanten, bevor die echten Fächer geschlossen waren. Fixes: **relativer**
Entartungs-Test (Gram-Determinante gegen `|ab|²·|ac|²`) + Degenerations-Guard
in `valid` (`sin(Winkel) > 0,01`). Platte: degenerierte Dreiecke 2 → 0,
Komponenten 4 → 2, χ 0 → 2.

**Befund 10 — die Forensik log (Selbstkorrektur).** `LICHEN_WHY` zählte an
einer offenen Kante das **bereits vorhandene** Dreieck als „gültigen
Kandidaten" und meldete daraufhin „voll gültig → Walk-Sackgasse". Die gesamte
§8d-Diagnose („92 % Sackgassen, die Extraktion ist der Engpass") war damit ein
**Artefakt**. Nach der Korrektur (Apexe der existierenden Dreiecke
ausschließen) ergibt die Platte ein völlig anderes Bild:

| Ursache | Anteil |
|---|---|
| Normalen-Guard blockt | 465 |
| Ketten-Kreuzungs-Verbot | 72 |
| Sliver | 3 |
| Walk-Sackgasse | **0** |

**Die Wächter sind der Engpass, nicht die Extraktion.** Insbesondere ist der
neue Normalen-Guard (§8e) an scharfen Kanten zu streng: seine Schwelle
(6·α, geklemmt 30–75°) erlaubt keinen Dreieck-Übergang, dessen Endpunkt-
Normalen 90° auseinander stehen — an einer Box-Kante trifft das jedes Dreieck
zwischen zwei *Innen*-Punkten benachbarter Flächen. Die Normalen-MENGE rettet
nur Dreiecke, an denen ein Ketten-Punkt beteiligt ist. **Nächster Schritt:**
den Guard feature-bewusst machen (ein Paar darf über eine gemeinsame
B-Rep-Kante hinweg inkompatibel sein, wenn das Dreieck die Kante nicht kreuzt)
statt ihn zu lockern — und dann das Ketten-Kreuzungs-Verbot (72) prüfen.

**Methodische Lehre:** Ein Diagnose-Werkzeug ist selbst Code und kann lügen.
Die „Walk-Sackgasse" hat eine ganze Runde Arbeit (best-first Front, Rogue-Seed-
Verbot, Flip-Kaskade) in eine Richtung gelenkt, die nicht der Engpass war —
die Fixes sind einzeln korrekt und bleiben, aber sie waren nicht die Wurzel.
Vor der nächsten Optimierung: **Forensik gegen einen Fall mit bekannter
Antwort prüfen.**

## 8g. Dünne Platte wasserdicht — beide Wurzeln lagen in der Punktverteilung (v0.6.553)

Nach der Forensik-Korrektur (§8f) zeigte die dünne Platte (1 mm dick,
Gegenblätter 1 mm auseinander, nur 1 mm breite Seitenfläche) ein klares Bild:
465 × Normalen-Guard, 72 × Ketten-Kreuzung, **0 × Walk-Sackgasse**. Das
Nachfassen an konkreten Kanten führte auf **zwei Wurzeln — beide in der
Punktverteilung**, keine im Prädikat und keine in der Extraktion.

**Wurzel A — die Ketten folgten dem ε-Sampling nicht.** Auf der Platte blieb
die B-Rep-Kette bei **1,75 mm**, während die Flächen per lfs auf **0,26 mm**
verfeinert wurden (Faktor 7). Ein Ketten-Punkt sitzt dann als grober
Fremdkörper in einer dichten Punktwolke: sein Delaunay-Fächer ist entartet,
die leere Umkugel **jedes** Dreiecks mit ihm enthält Flächen-Punkte — und der
einzige Kandidat, den die Umkugel überhaupt noch zulässt, liegt auf der
**Gegenseite der Platte**. Dort greift dann korrekt der Normalen-Guard, und
die Kante bleibt offen. Der Guard war also nicht die Ursache, sondern der
*letzte* Wächter, der den Schaden sichtbar machte. Fix: `build_chain` bekommt
ein lfs-begrenztes `h` (ε-Sampling auch auf den Ketten).

**Wurzel B — die erste Punktreihe stand zu dicht an der Kante.** Die
Schritt-Halbierung setzte sie auf den **halben** Abstand: 0,14 mm neben einer
Kette, deren eigene Nachbarn 0,48 mm entfernt sind. Solche Punkte liegen **in
der Umkugel der Kanten-Dreiecke** und machen sie ungültig — die schmale
Seitenfläche der Platte blieb deshalb komplett ungemesht. Fix:
**Ketten-Clearance** im Wachstum (`brep_dist ≥ 0,7 · r_eff`) — die Fläche muss
von ihrer Kante mit dem vollen Spacing „abstehen".

**Ergebnis:** Platte 556 → 540 (Sliver-Fix) → 326 (Ketten-lfs) → **0** offene
Kanten, χ = 2, 0 Wand-Durchquerungen, 1 Komponente — ohne Chart, ohne
Kandidaten-Vervollständigung, ohne Loch-Füller. Gate
`lichen_thin_plate_global_walk_is_watertight` prüft alles davon.

**Das Muster über alle Befunde hinweg:** jede vermeintliche Prädikat- oder
Extraktions-Schwäche war am Ende eine **Sampling-Verletzung**. Die Reihenfolge
der Säulen aus dem Cocone-Programm stimmt also — Säule 1 (ε-Sample) trägt
alles, und sie muss für **jede** Punktquelle gelten: Wachstum, **Ketten**, und
den Abstand zwischen beiden.

Demo (Gyroid + tangentiale Mikro-Features): 814 → 651 offene Kanten. Dort geht
lfs an tangentialen Kontakten gegen 0 — die verbliebene, theoretisch harte
Front.

## 8h. Wächter-Rückbau: der Turm fällt (v0.6.554)

Mit der korrigierten Forensik ließ sich der Wächter-Turm zum ersten Mal
gezielt abtragen. Demo (globaler Walk, ε=0,5): **814 → 303 offene Kanten**.

**Befund 11 — das Ketten-Kreuzungs-Verbot cullte falsch.** Sein Nähe-Cull
nutzte `cell` = das **größte Spacing der gesamten Punktwolke**; im globalen
Walk ist das `h` (Zentimeter). Damit wurden Ketten-Segmente quer durch den
ganzen Körper in die Dreiecks-Ebene projiziert und „kreuzten" dort scheinbar.
Größter Einzelblocker der Demo (279 von 655). Fix: echter 3D-Punkt-Segment-
Abstand zum Dreiecks-Zentroid.

**Befund 12 — der Normalen-Guard war zu streng.** `6·α ≈ 30°` lehnt auf dem
**Gyroid** legitime Nachbarn ab (dort drehen sich benachbarte Flächen-Normalen
schon um ~35°). Für die Blatt-Trennung ist diese Strenge gar nicht nötig: das
Gegenblatt einer dünnen Wand steht **~antiparallel** und wird von jeder
Schwelle unter 180° sicher erwischt. Neue Schwelle: **90° („gleiche
Halbkugel")** — die prinzipielle Grenze, denn eine gültige Flächenkante
verbindet nie zwei Punkte, deren Normalen in entgegengesetzte Halbräume
zeigen. Die dünne Platte bleibt bei 0 Wand-Durchquerungen.

**Befund 13 — der Cocone-Kegel ist ERSATZLOS entfallen.** Er prüft die
Richtung zum Nachbarn gegen die **gemittelte** Punkt-Normale — die an jeder
B-Rep-Kante bedeutungslos ist (sie zeigt in die Winkelhalbierende) — und
lehnte damit legitime Kanten-Dreiecke ab. Was er leisten sollte (fremde
Blätter fernhalten), leistet der Normalen-Guard exakt und ohne
Kanten-Kollateralschaden. **Damit ist der erste Wächter des Turms gefallen,
nicht ersetzt, sondern gestrichen.**

Der globale Walk braucht heute nur noch: leere (Zeugen-)Umkugel,
Normalen-Guard, Sichtlinie/Zentroid auf der Oberfläche, Ketten-Kreuzungs-
Verbot, Degenerations-Guard. Sichtlinie und Ketten-Verbot sind die nächsten
Kandidaten für dieselbe Prüfung.

## 8i. Die irreduzible Front: Sub-Resolution-Kontakte (v0.6.555)

Die Demo steht nach dem Wächter-Rückbau bei **302 offenen Kanten** (von 814).
Die Forensik zerlegt sie so:

| Ursache | Anteil |
|---|---|
| Normalen-Guard blockt | 134 (davon **70 im 150–180°-Band = KORREKTE** Ablehnungen) |
| nur nicht-leere Kugeln | 88 |
| Ketten-Kreuzung | 46 |
| Walk-Sackgasse | 28 |
| Sichtlinie/Zentroid | 20 |

**Befund 14 — die beiden größten Posten haben dieselbe Wurzel.** Gemessen
(`zz_duplicate_sheet_stats`): **18 Punktpaare mit gegenläufigen Normalen liegen
näher als EIN Spacing** beieinander (engstes 0,56·space). Bei ε-Sampling mit
ε = 0,5 müsste das Gegenblatt **vier** Spacings entfernt sein — dort greift
aber der `minEdgeSpacing`-Boden. Jedes Dreieck, das einen dieser Punkte
enthält, hat seinen Gegenblatt-Partner **in der Umkugel** ⇒ kein Kandidat ist
gültig („nicht-leere Kugeln"), und der einzige, den die Umkugel noch zuließe,
liegt auf dem Gegenblatt ⇒ der Normalen-Guard lehnt ihn korrekt ab. Beide
Symptome, eine Ursache.

**Befund 15 — Verfeinern hilft nachweislich nicht.** minEdge 0,1 → 0,01:
302 → 827 offene Kanten. Das ist die **tangentiale Annäherung** (Mini-Kugel in
der Gyroid-Wand), wo lfs → 0 geht: **kein endliches Spacing** erfüllt die
Abtastbedingung mehr. Die Theorie sagt das exakt voraus — die
ε-Sample-Garantie ist gegen lfs formuliert, und lfs ist dort null.

**Damit ist die Front theoretisch scharf und die Antwort ist nicht mehr
Abtastung, sondern KOLLAPS:** Wo zwei Blätter einander näher kommen als die
Auflösung, ist die Topologie dazwischen sub-resolution und muss zusammenfallen
(so wie wir es an Ecken-Clustern längst tun). Der frühere Kollaps-Versuch
scheiterte an der falschen Detektion (er suchte *Duplikate* — davon gibt es
null); die richtige Detektion ist die **Gegenblatt-Nähe** (gegenläufige
Normalen, Abstand < Spacing), und sie ist jetzt vermessen und benannt.

Nebenbei gefixt: das **FD-Fenster des Shape-Operators** hing fest an der
globalen Ziel-Kantenlänge (0,1·h = 1 mm bei h = 10) und war damit auf feinen
Features blind — eine Gyroid-Wand ist selbst nur 1 mm dick. Jetzt zweistufig:
grob schätzen, Radius ableiten, mit daran gekoppeltem Fenster nachmessen.

## 8j. Der Keil: warum Strahl-Marsch die lfs nicht messen kann (v0.6.556)

**Befund 16 — der äußere Reach war blind wie der innere.** Die lfs-Bedingung
griff auf dem Gyroid gar nicht, obwohl `lfsFactor` gesetzt war: `reach()`
(außen) nutzte noch die **Fußpunkt-Verfolgung** des Konzepts, die erst
anschlägt, wenn der Fußpunkt weiter als `reach_tol` (1 mm) springt — ein
**0,17-mm-Spalt** zwischen zwei Gyroid-Blättern bleibt darunter. Exakt derselbe
Fehler, den `reach_inner` in v0.6.549 losgeworden ist. Fix: `reach_outer_flat`
über die Abflachung des Distanzfelds.

**Befund 17 — der Mindestabstand brach die ε-Garantie.** Der
`minEdgeSpacing`-Boden klemmte das Spacing auf 0,1, während lfs dort ≤ 0,04
verlangt. Die lfs-Bedingung ist aber die **Voraussetzung** des
Umkugel-Theorems, kein Komfort: ein Boden, der sie bricht, erzeugt genau den
gemessenen Zustand — Blattabstand 0,17 mm bei 0,2 mm Spacing, jedes Dreieck hat
den Gegenblatt-Punkt in seiner Umkugel, **kein Kandidat ist gültig**. Die lfs
hebelt den Boden jetzt lokal aus (dieselbe Regel gilt längst für die
Ketten-Überdeckung).

**Befund 18 (der eigentliche, harte Kern) — der KEIL.** Auch mit korrektem
Strahl-Marsch bleibt die Verletzung bestehen. Die Vermessung der
Gegenblatt-Paare zeigt warum: der Partner liegt **nicht entlang der Normalen**,
sondern quer versetzt (Δ ≈ (−0,17; +0,07; −0,05) bei |Δ| = 0,19). Es ist ein
**Keil**: zwei Flächen, die sich unter kleinem Winkel nähern. Ein Strahl-Marsch
entlang ±n kann ihn **strukturell nie finden** — die Medialachse eines Keils
liegt in der Winkelhalbierenden, nicht auf der Normalen.

**Vermessen und verworfen:** das Spacing am Abstand zum nächsten
Gegenblatt-Punkt der **Punktwolke** zu ziehen (die einzige Messung, die einen
Keil sehen *kann*). Ergebnis: Box 0 → 3 offene Kanten, Demo 295 → **532**. Die
lokale Verfeinerung **kaskadiert** und reißt genau die Dichte-Übergänge auf, an
denen die Sterne uneins werden. Im Code als Irrweg dokumentiert.

**Damit ist die Anforderung präzise:** eine keil-korrekte Abtastung braucht ein
**global konsistentes, Lipschitz-beschränktes Dichtefeld** (`space(x)` mit
|∇space| ≤ const, aus der lfs *aller* Blätter gebildet) — keine lokale
Nachkorrektur. Das ist der klassische *sizing field* der Delaunay-Refinement-
Literatur und der nächste Baustein. Alternativ bleibt der Kollaps der
Sub-Resolution-Topologie.

Demo: 302 → 295. Alle Gates unverändert (Box/Zylinder/Kugel χ=2, Torus χ=0,
dünne Platte χ=2 mit 0 Wand-Durchquerungen).

## 8k. Die Divergenz: warum ε-Sampling an tangentialen Kontakten scheitern MUSS (v0.6.557)

**Befund 19 — der Marsch konnte den Spalt gar nicht sehen.** Der Reach-Marsch
startete bei `0,02·cap` = **0,4 mm** (h=10, ε=0,5) und kann damit einen
0,17-mm-Spalt prinzipiell nicht auflösen: er meldet seinen *ersten Schritt* als
lfs, und das Spacing landet exakt auf 0,5·0,4 = 0,2 — genau der gemessene Wert
(Blattabstand 0,17 bei Spacing 0,19). Der Startschritt ist jetzt ein Parameter.

**Und dann die entscheidende Messung.** Mit einem feinen Start (0,05·r) *löst*
der Marsch die Spalte auf — das ist die physikalisch korrekte Messung. Das
Ergebnis:

| Variante | Demo (offene Kanten) | Punkte | Laufzeit |
|---|---|---|---|
| grober Start (heute) | **295** | 2 807 | ~20 s |
| feiner Start + Auflösungs-Boden | 619 | 6 390 | 2,5 min |
| feiner Start + freier Boden | — | — | **>15 min, kein Ende** |

**Die korrekte Messung divergiert.** An einem tangentialen Kontakt geht lfs → 0
und die geforderte Dichte → ∞. Das ist keine Implementierungsschwäche, sondern
**exakt die Aussage der Theorie**: die ε-Sample-Garantie ist gegen lfs
formuliert, und an einem Sub-Resolution-Kontakt existiert **keine endliche
gültige Abtastung**.

**Zwei Irrwege, dieselbe Ursache** (beide im Code dokumentiert):

1. **Feiner Startschritt** (dieser Abschnitt) — Demo 295 → 619.
2. **Keil-Spacing aus der Punktwolke** (§8j) — Box 0 → 3, Demo 295 → 532.

Beide verfeinern **lokal** und erzeugen dabei abrupte Dichte-Sprünge, an denen
Sterne und Umkugeln auseinanderlaufen. Die Lehre ist scharf und gilt für jede
weitere Verfeinerungs-Idee:

> **Verfeinerung ohne ein global konsistentes, Lipschitz-beschränktes
> Dichtefeld schadet mehr, als sie nützt.**

**Damit ist der nächste Baustein eindeutig benannt** — und es ist der letzte
strukturelle: ein **Sizing-Field** `σ(x)` mit
`σ ≤ min(h, α/κ, ε·lfs)` und `|∇σ| ≤ K` (K ≈ 0,3), global relaxiert und *dann*
als Vorgabe für Ketten UND Wachstum verwendet (statt lokal nachzukorrigieren).
Wo σ unter die Auflösungsgrenze fällt, ist die Topologie sub-resolution und
gehört kollabiert, nicht abgetastet.

## 8l. Sizing-Field: der letzte strukturelle Baustein (v0.6.558)

Zweimal war vermessen worden, dass **lokale** Verfeinerung das Netz
verschlechtert, obwohl sie geometrisch korrekter ist (§8j: Keil-Spacing aus der
Punktwolke → Box 0→3; §8k: feiner Reach-Marsch → Demo 295→619). Die Diagnose
war beide Male dieselbe: abrupte Dichte-Sprünge, an denen Sterne und Umkugeln
auseinanderlaufen. `lichen::SizeField` macht es **global**:

1. **Roh-Größe** an Stützstellen: `min(h, α/κ, ε·lfs)` — mit **feinem** Marsch
   (der grobe meldet seinen ersten Schritt und sieht kleine Spalte nie).
2. **Lipschitz-Relaxation:** `σ_i = min_j (raw_j + K·|p_i − p_j|)`, K = 0,3,
   iterativ bis Fixpunkt (σ fällt monoton → terminiert).
3. **Auswertung:** `σ(x) = min_j (σ_j + K·|x − p_j|)` — automatisch
   K-Lipschitz auch *zwischen* den Stützstellen.
4. **Boden = `minEdgeSpacing`** (Auflösungsgrenze). Ohne ihn divergiert das
   Feld an tangentialen Kontakten, wo lfs → 0.

Die Pipeline läuft dafür **zweimal** (Option `sizingField`, Dialog-Checkbox):
erst grob für die Stützstellen-Wolke, dann mit dem Feld für **Ketten UND
Wachstum** — beide Punktquellen folgen jetzt derselben Vorgabe.

**Messung** (Demo, globaler Walk, ε = 0,5, minEdge 0,03):

| | Punkte | Dreiecke | offene Kanten | Rate |
|---|---|---|---|---|
| ohne Feld | 4 683 | 8 809 | 623 | 7,1 % |
| **mit Feld** | 27 639 | 53 982 | 1 398 | **2,6 %** |

Die **relative** Defektrate fällt also deutlich — das Feld tut, was es soll.
Die absolute Zahl wächst, weil das Netz 6× größer wird. Alle glatten Körper
bleiben unverändert wasserdicht (Box/Zylinder/Kugel χ=2, Torus χ=0, dünne
Platte χ=2 mit 0 Wand-Durchquerungen).

**Damit ist das Cocone-Programm vollständig implementiert:**

| Säule | Zustand |
|---|---|
| 1. ε-Sample (Punktverteilung) | Sizing-Field, Lipschitz-geglättet, für Ketten + Wachstum |
| 2. Prädikat (restricted Delaunay) | Zeugen-Umkugel + Normalen-Guard |
| 3. Manifold-Extraktion | Sheet-Walk (Ball-Pivoting-Regel), global |

Und die verbleibende Front ist keine Lücke im Programm mehr, sondern seine
**bewiesene Grenze**: an tangentialen Kontakten geht lfs → 0, dort existiert
keine endliche gültige Abtastung. Die Antwort dort ist der **Kollaps der
Sub-Resolution-Topologie** — die einzige verbliebene Aufgabe.

## 9. Kritische Reflexion (2026-07-12)

Bewertung des Konzepts mit dem Wissen aus Sampling-Theorie
(ε-Sampling/lfs, Amenta–Bern, Cocone/Crust, restricted Delaunay) und der
bisherigen Empirie. Ohne Anspruch auf Vollständigkeit.

### Was sollte GARANTIERT funktionieren — und warum

* **R-G1 Restricted-Delaunay auf ε-Sample:** Die Theorie (Amenta/Bern) gibt
  eine echte Garantie: ist die Abtastung ein ε-Sample der Fläche bzgl. der
  local feature size (lfs = Distanz zur Medialachse, INNEN wie außen) mit
  ε ≲ 0.1, dann ist die restricted-Delaunay-Triangulierung homöomorph zur
  Fläche und wasserdicht. Der Betreiber-Eindruck „das reine
  Umkugel-Verfahren funktioniert, braucht aber extrem hohe Auflösung" ist
  exakt diese Bedingung: die Garantien gelten erst deutlich unterhalb der
  Krümmungs- und Reach-Skala. Freie Kugeln mit Gabriel wasserdicht = der
  empirische Beleg, dass der Kern stimmt, WENN die Abtastung stimmt.
* **R-G2 Planare Flächen mit Rand-Constraints:** 2D-CDT auf exakter Ebene
  ist ein gelöstes Problem — dass der General-Pfad auf der Box gegen den
  CDT konvergieren muss, ist die richtige Referenz.
* **R-G3 Konsistente Orientierung** auf einem manifold Ergebnis folgt aus
  ∇f — unproblematisch.

### Fragwürdige Aspekte des Konzepts

* **R-F1 „Umkugel-Bedingung nur gegen Graph-Nachbarn" (§4)** ist KEIN
  Delaunay: das Konzept prüft die leere Umkugel nur gegen die in *G*
  verknüpften Punkte. Der heutige Code prüft immerhin räumlich
  (Spatial-Hash) — aber mit dem 5·cell-Deckel und dem Horizont wird das
  Prädikat wieder lokal. Ein lokal leerer Umkreis ist global nicht leer,
  wenn der Zeuge außerhalb des Horizonts liegt; genau daraus entstehen die
  „Kanten zu fernen Punkten"-Defekte: **fehlende nahe Punkte im
  Kandidaten-Set und ein zu weiter Horizont sind duale Fehlerquellen
  desselben Lokalitäts-Kompromisses.**
* **R-F2 Stern-Konsens ist ein Symptom-Konstrukt:** vote=3 heilt die
  Uneindeutigkeit der Tangential-Projektionen, ERZEUGT aber das duale
  Problem — legitime Dreiecke, die ein Stern (wegen Horizont, Cocone,
  Projektion) nicht sieht, fallen still raus → Löcher, die dann die
  Vervollständigung + Füller wieder schließen müssen. Die Kaskade
  vote → Wächter → Vervollständigung → Füller → Nach-Wächter ist ein
  Indiz, dass die Grund-Primitive (Punktverteilung + EIN Akzeptanzprädikat)
  die Invariante nicht selbst trägt. Vergleich: Cocone-artige Verfahren
  brauchen genau EIN Prädikat, weil ihre Sampling-Voraussetzung hart ist.
* **R-F3 Tangential-Projektion mit Horizont ≫ Krümmungsradius** ist
  mathematisch undefiniert (Punkte jenseits des Äquators projizieren
  chaotisch). Der Stern-Horizont `2.5·max(space)` ist NICHT
  krümmungsgekappt — auf der r=1-Kugel mit groben Nachbarn wird er zu
  mehreren Radiant Bogen. Jede Zutat ist einzeln plausibel; zusammen
  verletzt sie die Voraussetzung der Projektion.
* **R-F4 `auto`-Modusweiche (vote vs. gabriel je Trägertyp)** ist ein
  Eingeständnis, dass keines der beiden Prädikate allein trägt. Sie bricht
  an Übergängen: eine Kette zwischen planarer und gekrümmter Fläche wird
  von zwei verschiedenen Akzeptanz-Regimen bedient.
* **R-F5 Empirische Konstanten-Dichte:** 0.45/0.8/0.85·r, 0.2·rr, 0.15·rr,
  0.3·L·θ, 0.1·space, 2.5·space, 5·cell, 2e-4·r², 15–60°-Clamp … Jede ist
  einzeln vermessen, aber ihr Zusammenspiel ist nicht beweisbar; jede neue
  Geometrie kann eine neue Kante des Parameterraums treffen. Das ist der
  strukturelle Grund für die lange Fix-Serie.

### Fixes, die (auch) Symptome bekämpfen

* **R-S1 Loch-Füller + Kandidaten-Vervollständigung** reparieren, was
  Punktverteilung + Akzeptanz liegen lassen. Das Dreiecks-Loch-Prädikat ist
  topologisch sauber; der CDT-Loop-Füller ist Kosmetik über einer
  Sampling-Schwäche.
* **R-S2 Fold-Auflösung in `orient_consistent`** (größerer Umkreis fliegt)
  entfernt real falsche Geometrie — dass sie überhaupt entsteht, zeigt,
  dass die Seiten-/Sichtlinien-Wächter das Blatt-Trennungs-Problem nicht
  vollständig lösen.
* **R-S3 Schritt-Halbierung + Rebalancing + Grading** sind drei
  Mechanismen für dieselbe Aufgabe (lokal korrekte Dichte). Eine
  Punktverteilung, die die Abtastbedingung KONSTRUKTIV garantiert (z.B.
  echte Poisson-Disk auf der Fläche mit r(p) = min(α/κ, lfs-Anteil)),
  würde alle drei ersetzen.

### Möglicherweise falsche Annahmen

* **R-A1 „Das Hex-Wachstum liefert eine meshbare Verteilung."** Es liefert
  fast-cozirkuläre Quads (maximal entartet fürs Delaunay-Prädikat) und an
  Fronten-Kollisionen genau die Lückenbreiten (0.8r < Lücke <
  dreieckstauglich), an denen alles Downstream kämpft. Die Alternative
  (unstrukturierte Poisson-Disk-Dichte OHNE Hex-Muster) würde die
  Tie-Problematik an der Wurzel entfernen — Ties sind dann Maß Null.
* **R-A2 „reach nur nach außen genügt; die innere Medialachse fängt die
  Sichtlinie ab" (§2.3-Hinweis).** Die Doppelkappen-Serie hat gezeigt, dass
  genau der Innen-Reach fehlt: dünne Wand = innere Medialachse nah, und
  alle Sehnen-Tests waren nacheinander blind (Sichtlinie → Sagitta →
  Region). Die Regionen-Sichtlinie ist der dritte Patch für dieselbe
  fehlende Größe. **ε-Sampling ist gegen lfs definiert, nicht gegen den
  Außen-Reach.**
* **R-A3 „space ≤ α/κ ist durch die Konstruktion garantiert."** Offener
  Widerspruch (Punkt 8.1): Innenpunkte der r=1-Kugel mit space≈2 bei
  curv=1°. Solange die Kette (local_radius → Grading → Kaskaden-Bremse →
  space-Zuweisung) das nicht nachweislich einhält, stehen ALLE
  daraus abgeleiteten Größen (Horizont, Cocone-Winkel, Sagitta-Toleranzen)
  auf Sand. **Das ist der am stärksten verdächtige Einzelpunkt.**
* **R-A4 „Die 3D-Umkreisbedingung approximiert die geodätische" (§4/§68).**
  Gilt nur unter der lfs-Bedingung — die der Code nirgends erzwingt oder
  auch nur misst. Es gibt keinen Sampling-Audit („ist diese Punktmenge ein
  ε-Sample?"), obwohl er billig wäre und JEDE Downstream-Diagnose
  vorhersagen würde.

### Was klare, eindeutige Test-Signale liefern würde

1. **Sampling-Audit als Testprädikat:** für jeden Punkt `space·κ(p) ≤ α`
   und `space ≤ c·lfs(p)` (lfs numerisch über beidseitigen Reach) messen
   und VERLETZUNGEN zählen — vor jeder Triangulierung. These: die
   Verletzungs-Karte deckt sich mit der Loch-/Fold-Karte. (Bestätigt →
   Punktverteilung reparieren; widerlegt → Akzeptanz reparieren.)
2. **`|f(p)| ≤ tol_abs` über ALLE emittierten Punkte** als hartes Gate
   (Betreiber-Bild 1 wäre nie durchgekommen).
3. **General-vs-CDT-Differenz auf planaren Flächen** (gleiche Punktmenge):
   symmetrische Dreiecks-Differenz muss → 0; jedes Differenz-Dreieck ist
   ein eindeutiger, lokalisierter Fehlerfall.
4. **Kugel-Konvergenztest:** freie Kugel über α ∈ {20°,10°,5°,2°,1°} —
   watertight muss für ALLE gelten, nicht nur für die Gate-Settings;
   Hausdorff-Distanz zur Idealkugel muss ~α² fallen.
5. **Umkugel-Zeugen-Gegenprobe:** für jede akzeptierte Kante die 3D-nächsten
   k Punkte listen; liegt einer davon nicht im Kandidaten-Set des Sterns,
   ist der Horizont/Filter der Täter (unterscheidet R-F1 von R-F3).
