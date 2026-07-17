# Synthese Iteration 1 — SDF-Distanzerhaltung unter Modellieroperationen

Stand 2026-07-18, nach Abschluss von fünf unabhängigen, gegenseitig nicht
verankerten Agenten (Harness + vier Forschungsrichtungen). Primärquellen sind
die `LOG.md` der Agenten-Verzeichnisse und die Baseline unter
`results/baseline/`; dieses Dokument bewertet und ordnet.

## 1. Das Problem ist präziser als „Booleans zerstören die Distanz"

Alle vier Richtungen konvergieren unabhängig auf dieselbe Struktur:

**Der Feldfehler zerfällt in zwei orthogonale Klassen** (Agent D, exp1/exp5):

1. **Normfehler** — |∇f| ≠ 1, das Feld zeigt in die richtige Richtung, aber
   mit falschem Maßstab. Quelle: smin-Zonen (bis √m bei m gestapelten Blends),
   Skalierung, algebraische Felder. **Lokal korrigierbar** (Sampson f/|∇f|,
   10–25× Reduktion im Nahband; oder an der Wurzel vermeiden, s. Circular-CD).
2. **Fußpunktfehler** — |∇f| = 1, das Feld ist lokal ein *perfektes SDF der
   falschen Fläche* (Phantom-Flächen des Subtrahenden, verdeckte Flächen im
   Union-Inneren). **Für jede gradientenbasierte Korrektur beliebiger Ordnung
   unsichtbar** (gemessen: Sampson ändert dort exakt nichts). Nur nichtlokale
   Verfahren helfen (Redistancing, B-Rep-Proxy, strukturelle Tape-Rewrites).

**Harte Booleans sind seitenselektiv exakt** (Agent A, mit Beweis + Messung;
unabhängig bestätigt von B algebraisch und D als Mengen-Identität):

- `min` (Union) ist im **gesamten Außenraum exakt** — die „Union unterschätzt
  außen im Schattenkegel"-Intuition ist falsch (A und D haben sie beide als
  Ausgangshypothese getestet und verworfen, Fehler 0.0 auf allen Auflösungen).
- `max` (Intersection/Differenz) ist dual im **Innenraum exakt**.
- Kaputt sind genau die dualen Seiten: **Union innen** (Fehler = Nahttiefe,
  h-unabhängig; Baseline: bis 58 % Tiefen-Unterschätzung, Shell-Innenfläche
  bei +130 % Soll-Wanddicke am Neck) und **Differenz/Intersection außen** im
  Rarefaction-Fan hinter einspringenden Kanten (13–16 % der Abfragedistanz —
  der CAD-Normalfall Tasche/Bohrung; dünnwandige Differenz-Hörner kollabieren
  bis f ≈ 0.05·d_wahr).
- Konsequenz: **Korrekturen müssen seitenselektiv sein.** Volles Redistancing
  verschlechtert die exakte Seite (0 → 0.23·h, in 2D und 3D gemessen).

**Die √2-Folklore ist doppelt falsch** (C, D): An Phantom-Flächen ist der
Fehler unbeschränkt repräsentationsabhängig (Faktor ~4 an trivialer
Box−Box-Szene), und smin-Stapel wachsen in Quadratur auf √m (nicht auf √2
gedeckelt, aber auch nicht (√2)^N). Kein globaler Skalierungsfaktor —
weder ÷√2 noch ein anderer — kann das Feld reparieren; `f − δ`-Offsets
erzeugen strukturell falsche Isoflächen (+497 % Distanzfehler, Phantom-Lappen,
Erosionsdefekt 22–59 % von δ superlinear an der Union-Naht).

**Fehler über CSG-Tiefe** (D exp4, Harness-Sweep): harte min/max sind
Kontraktionen in der Sup-Norm (Winner-Branch, keine Verstärkung; Baseline
misst monotones, aber flaches Wachstum durch Naht-Akkumulation). smin-Ketten
wachsen in Quadratur, realistische (räumlich getrennte) Ketten sättigen bei
~1.24. Das Schreckgespenst „exponentielle Degradation über Tiefe" existiert
nicht; die Tiefe ist beherrschbar, die *Nähte* sind das Problem.

## 2. Bewerteter Lösungsraum

Bewertung je Ansatz: **Exaktheit** (was wird wie gut repariert), **Kosten**
(Lauf-/Bauzeit, Speicher), **Integrierbarkeit** (Passung zu Tape/WGSL-Mirror/
Weave-Pipeline). Evidenzgrad: gemessen / hergeleitet / plausibel.

### A. Nullstellen-treue Blends: Circular-CD-smin (Agent B) — ⭐ stärkster Einzelbefund

Quílez' Circular-CD-Variante hat **exakt dieselbe Nullstellenmenge** wie unser
`smin_poly` (Hausdorff 0.00 über 16 Konfigurationen — identische
Fillet-Geometrie), ist aber als Konvexkombination überall 1-Lipschitz:
|∇f| = 1.000, Überschätzung 0, stabil bis Tiefe 12. Trace-Simulation des
#386-Szenarios: 0 % Tunnel bei 4.0 Schritten mit *vollen* Schritten — 2.7×
schneller als die heutige ÷√2+0.75-Politik.

- Exaktheit: eliminiert die Normfehler-Klasse an Blends an der Wurzel;
  Feld*werte* auf der Naht-Bisektrix weichen ≤ 0.293·r ab (Unterschätzung —
  sicher, aber mesher-sizing-relevant).
- Kosten: identische Größenordnung pro Eval; ÷√2-Fade und
  `MASKED_BLEND_MARCH_RELAX` entfallen ersatzlos.
- Integrierbarkeit: Drop-in in `csg.rs::smin_poly`/`blend_min_eval` + WGSL/TSL-
  Mirror; Chamfer-Modus braucht ein eigenes CD-Gegenstück.
- **Offenes Risiko (einziges):** Lichen-Mesher-Regression durch die
  Bisektrix-Unterschätzung — Iteration-2-Experiment.

### B. Sichere Schrittweiten ohne Pauschal-Dämpfung (Agenten B, D)

Zwei komplementäre, gemessene Mechanismen ersetzen die Ad-hoc-`march_relax`:

- **Segment-lokale Lipschitz-Schranken** (Galin 2020; B H3): λ pro Opcode als
  Einzeiler aus vorhandenen Größen (Twist: 1+|τ|·max‖p‖ …), akkumuliert im
  Interpreter. Twist∘Box: 16.6 statt 47.6 Schritte, 0 % Tunnel (2.9×).
- **Tiefenbewusstes Relax als Sicherheitsnetz** (D H3): heutiges flaches 0.75
  ist *nachweislich unsicher* (koinzidenter m=4-Stack penetriert −0.10);
  sicher ist 1/√(1+m) mit m = maximale smin-Schachtelung des Tape-Pfads,
  zur Compile-Zeit aus dem `BoundedCsg`-Baum — 0 Laufzeitkosten. Wird durch
  Ansatz A weitgehend obsolet, bleibt Netz für Chamfer/Displace/Exoten.

Exaktheit: betrifft nur Trace-Sicherheit, nicht Feldbeträge. Kosten: null bis
negativ (weniger Schritte). Integrierbarkeit: hoch — `march_relax`-Mechanismus
existiert schon, wird präzisiert statt neu erfunden.

### C. Nahfeld-Wahrheit aus dem B-Rep-Proxy (Agent C)

Distanzabfrage gegen das tesselierte Weave-B-Rep (`tessellate_brep`):
im Nahfeld **exakt bis tol_mm, mit exakten scharfen Kanten** — dem
O(h)-Kanten-Schmieren jedes Gitters überlegen. Hybrid-Blend Feld↔Proxy im Band
überschießt nie (sphere-trace-sicher), Gradient-Spikes nur wenn Band < 4×
Feld-Diskrepanz. BVH-Punkt-Mesh-Queries CPU-tauglich.

- Exaktheit: repariert **beide** Fehlerklassen im Nahfeld, einschließlich der
  gradienten-blinden Fußpunktfehler; einzige gemessene Quelle magnitudentreuer
  Offsets/Shells nahe der Fläche.
- Kosten: BVH-Aufbau pro Body nach Regen (asynchron im Weaver-Worker denkbar);
  Query-Kosten CPU-seitig gemessen tragbar, GPU-Pfad offen.
- Integrierbarkeit: mittel — braucht fertiges Weave (Latenz nach Regen,
  vgl. #386-Gotcha „Fillet-Query löst spät auf"); CPU-Abfragen (Messen,
  Wandstärke, Offset-Samples, Mesher-Seeds) sofort, Render-Pfad später.
- **Offenes Risiko:** erscheinen Edge-Fillet-Zonen als gerundete Faces im
  Weave-Output? (Iteration-2-Experiment.)

### D. Seitenselektives Voxel-Redistancing on-demand (Agent A)

Fast Sweeping auf 200³ (`api/voxel.rs`-Skala): 0.4 s single-thread, 64 MB,
Fehler innen 0.133 → 0.010 max. Eikonal-Löser rundet konkave Kanten **nicht**
(Zero-Set-Drift O(h²), Kink bleibt); die gefürchtete Rundung entsteht erst beim
trilinearen **Resampling** (O(h)).

- Exaktheit: repariert Fußpunktfehler flächig, aber nur bis O(h)-Abfragefehler;
  muss seitenselektiv angewandt werden (sonst verschlechtert es die exakte
  Seite auf 0.23·h).
- Kosten: einmal pro Body nach Feature-Regen; Sampling des echten Tapes
  dominiert (macht `voxelizeOccupancy` heute schon in einem Call).
- Integrierbarkeit: hoch für **Abfrage-/Korrekturschicht** (Wandstärkenanalyse,
  Offset-Vorschau, innere Distanzen); ungeeignet als Ersatz-Renderfeld.
- Gegenüber C: schlechter an Kanten (O(h) vs exakt), besser im tiefen
  Inneren/Fernfeld und ohne Weave-Abhängigkeit.

### E. Punktweise Tape-Redistanzierung: Hopf–Lax (Agent A)

Gitterfrei, nachbarfrei, d(x) über iterierte Ball-Minima des Feldes; repariert
auch das √2-Feld ohne Strukturkenntnis (0.056 → 0.0016). ~440 Feld-Evals pro
Punkt („cheap"-Modus), mit `sdf_dual.rs`-Gradienten ~¼ davon.

- Exaktheit: hoch, beweisbar sicher iterierbar. Kosten: zu teuer pro Pixel,
  richtig für **einzelne analytische Abfragen** (Messen, Wandstärke,
  Offset-Stützpunkte). Integrierbarkeit: hoch (nur Feld-Evals nötig).

### F. Strukturelle Tape-Rewrites: Offset/Shell (Agenten A, D)

- **Dilation kommutiert mit Union** (D, exakt, gratis): `min(dᵢ)−δ =
  min(dᵢ−δ)` — Outward-Offset darf durch min-Knoten gepusht werden (dual:
  Erosion durch max). Reiner Compilezeit-Rewrite, deckt „Offset auf
  Union-of-Parts" verlustfrei ab.
- **Ball-Extremum-Offset** (A H5, plausibel/ungetestet): morphologische
  Dilation min über Ball hat beweisbar exaktes Zero-Set auch auf Pseudo-SDFs;
  WGSL-Kosten offen.
- Merksatz aus B H5: Außen-Offset über Union und Innen-Offset über
  Intersection sind *schon heute exakt*; kaputt sind nur die dualen Fälle.
  Ein Offset-Feature kann also zur Compile-Zeit entscheiden, ob es exakt ist.

### G. Normalisierung als Metrik-Politur: AD-Sampson, vertrauensbandiert (Agent D)

f/|∇f| mit **AD-Gradient** (Zweikanal-(val,grad)-Eval durchs Tape; FD ist an
min/max-Tie-Flächen nachweislich kaputt — mittelt Äste, macht f/|g| bis 1.4×
zu groß, wo f exakt ist). 10–25× Nahband-Verbesserung bei Normfehlern; blind
für Fußpunktfehler; als Trace-Schranke ungültig; muss auf |f| ≲ r_blend
bandiert werden (bei großem δ wird Sampson schlechter als roh).

- Rolle: Politur für Mesher-Projektion und Offset-Genauigkeit in Blend-Zonen,
  **nachdem** A–F die strukturellen Fehler adressiert haben. AD-Infrastruktur
  (val,grad) nützt zusätzlich B (λ-Akkumulator) und E (¼ Evals).

### Verworfen / negativ (dokumentiert, nicht wieder anfassen)

- Globale Skalierung (÷√2 o. ä.) als Reparatur — strukturell unmöglich (C, D).
- `f − δ` als Offset auf Nicht-SDF — nicht durch Skalierung heilbar (C).
- Volles (nicht seitenselektives) Redistancing — verschlechtert exakte
  Seiten (A).
- Sampson/Normalisierung als Sicherheits- oder Fußpunkt-Korrektur (D);
  iterierte Normalisierung k≥2 (divergiert im Fernfeld, D).
- Rvachev-R-Funktionen roh (L bis 3.3, unsicher; B H4).
- Radius-Fade statt Crossfade im MaskedBlend (bringt nichts, B H2).
- Flaches `march_relax` 0.75 als Sicherheitsgarantie (D, Gegenbeispiel).
- FD-Gradienten für jede metrische Korrektur (D H5).

## 3. Empfohlene Zielarchitektur (Hypothese für Iteration ≥ 2)

Drei Schichten, entlang der Fehler-Taxonomie:

1. **Feld-Hygiene an der Wurzel** (billig, sofort): Circular-CD-smin (A),
   segment-lokale λ + tiefenbewusstes Relax-Netz (B), Offset-Rewrites (F).
   Ergebnis: Renderfeld ist überall Under-Bound mit |∇f| ≤ 1, volle Schritte,
   kein globales ÷√2; Normfehler-Klasse weitgehend eliminiert.
2. **Nahfeld-Wahrheit** (magnitudentreue Offsets/Shells/Abfragen): B-Rep-Proxy
   (C) als primäre Quelle, Hopf–Lax (E) für Punkte ohne Weave, Voxel-FSM (D)
   für innere/flächige Abfragen. Seitenselektiv einsetzen — nur dort
   korrigieren, wo die Baseline-Fakten Fehler ausweisen.
3. **Politur** (Mesher/Analytik): vertrauensbandiertes AD-Sampson (G) auf der
   gemeinsamen (val,grad)-Infrastruktur.

Offene Architekturfragen: GPU-Tauglichkeit des Proxy-Pfads; Fillet-Zonen im
Weave; Chamfer-CD-Gegenstück; Verhalten der λ-Akkumulation unter
verschachtelten Warps.

## 4. Iteration 2 — beschlossene Experimente (gemeinsames Harness)

Alle auf `crates/metricity-lab` (Ground Truth validiert ≤ 2.4e-4 rel.):

1. **Circular-CD-Tausch real** (aus B): Patch in `smin_poly`/`blend_min_eval`,
   messen: Zero-Crossing-Parität alt/neu am Korpus, Trace-Schritte/Tunnelraten
   inkl. MaskedBlend-Fade ohne ÷√2, **Lichen-Mesher-Regression** (offene
   Kanten) wegen Bisektrix-Unterschätzung — die Rollout-Gate-Frage.
2. **Echter Weave-Proxy** (aus C): `tessellate_brep` auf realen Kernel-Szenen
   (Box−Box, Box∪Zylinder, **Body mit Edge-Fillet**) gegen `eval_opcodes` und
   Brute-Force-GT — klärt das Fillet-Zonen-Risiko.
3. **Offset-Stack 3D** (aus A): Platte−Zylinder, Outward-Offset δ ∈
   {0.5, 2, 5}: roh vs Ball-Dilation (A5) vs Union-Rewrite (F) vs 200³-FSM;
   Wandstärke innen roh vs Hopf–Lax; Kosten in Tape-Evals/ms.
4. **Relax-Sicherheit + AD** (aus D): 1/√(1+m) vs 0.75 auf koinzidenten
   Stacks (m=2..8) und echten Fillets (Penetrationsrate, Schrittzahl-
   Regression); Zweikanal-AD in `eval_opcodes` als CPU-Referenz,
   validiert an Offset/Shell-Isoflächen-Genauigkeit (2D-Prognose −15 % → −2 %).

## 5. Bezug zu #384 (Opus-Hypothesen) und #386

- Opus' Karte der verlierenden Ops: bestätigt und präzisiert (seitenselektiv).
- Voxel-FSM on-demand: bestätigt machbar (0.4 s/64 MB), aber als
  Abfrageschicht, nicht Renderfeld; seitenselektiv nötig — sonst schädlich.
- „PDE-Reinit verworfen": in der Pauschalform überholt — gitterbasiert als
  Renderfeld ja (Resampling rundet), als Korrekturschicht nein (A2/A3).
- „Einzigartiger Gewinn echter Redistanzierung = betragskorrekte Offset/Shell":
  bestätigt, mit konkreten Zahlen und drei konkurrierenden Wegen (C/E/F).
- #386 (graue Wand): B quantifiziert das rohe smin-Feld im Szenario mit 85 %
  Tunnelrate; die heutige Politik verhindert Tunneln bereits — konsistent mit
  dem Memory-Befund, dass die Wand *kein* Overstep-Problem ist. Der
  Circular-CD-Tausch entschärft die Klasse trotzdem strukturell.
