# Synthese — SDF-Distanzerhaltung unter Modellieroperationen

Stand 2026-07-18. Abschnitte 1–5: Iteration 1 (fünf unabhängige, gegenseitig
nicht verankerte Agenten: Harness + vier Forschungsrichtungen). Abschnitte 6–8:
Iteration 2 (vier Experimente am echten Kernel, in isolierten Worktrees) und
die daraus folgende Integrationsempfehlung. Primärquellen sind die `LOG.md` /
`LOG-iter2.md` der Agenten-Verzeichnisse und `results/baseline/`; dieses
Dokument bewertet und ordnet.

## 1. Das Problem ist präziser als „Booleans zerstören die Distanz"

Alle vier Richtungen konvergieren unabhängig auf dieselbe Struktur:

**Der Feldfehler zerfällt in zwei orthogonale Klassen** (Agent D, exp1/exp5):

1. **Normfehler** — |∇f| ≠ 1, das Feld zeigt in die richtige Richtung, aber
   mit falschem Maßstab. Quelle: smin-Zonen (bis √m bei m gestapelten Blends),
   Skalierung, algebraische Felder. **Lokal korrigierbar** (Sampson f/|∇f|,
   10–25× Reduktion im Nahband; oder an der Wurzel vermeiden, s. Circular-CD).
2. **Fußpunktfehler** — |∇f| \= 1, das Feld ist lokal ein *perfektes SDF der
   falschen Fläche* (Phantom-Flächen des Subtrahenden, verdeckte Flächen im
   Union-Inneren). **Für jede gradientenbasierte Korrektur beliebiger Ordnung
   unsichtbar** (gemessen: Sampson ändert dort exakt nichts). Nur nichtlokale
   Verfahren helfen (Redistancing, B-Rep-Proxy, strukturelle Tape-Rewrites).

**Harte Booleans sind seitenselektiv exakt** (Agent A, mit Beweis + Messung;
unabhängig bestätigt von B algebraisch und D als Mengen-Identität):

* `min` (Union) ist im **gesamten Außenraum exakt** — die „Union unterschätzt
  außen im Schattenkegel"-Intuition ist falsch (A und D haben sie beide als
  Ausgangshypothese getestet und verworfen, Fehler 0.0 auf allen Auflösungen).
* `max` (Intersection/Differenz) ist dual im **Innenraum exakt**.
* Kaputt sind genau die dualen Seiten: **Union innen** (Fehler \= Nahttiefe,
  h-unabhängig; Baseline: bis 58 % Tiefen-Unterschätzung, Shell-Innenfläche
  bei +130 % Soll-Wanddicke am Neck) und **Differenz/Intersection außen** im
  Rarefaction-Fan hinter einspringenden Kanten (13–16 % der Abfragedistanz —
  der CAD-Normalfall Tasche/Bohrung; dünnwandige Differenz-Hörner kollabieren
  bis f ≈ 0.05·d\_wahr).
* Konsequenz: **Korrekturen müssen seitenselektiv sein.** Volles Redistancing
  verschlechtert die exakte Seite (0 → 0.23·h, in 2D und 3D gemessen).

**Die √2-Folklore ist doppelt falsch** (C, D): An Phantom-Flächen ist der
Fehler unbeschränkt repräsentationsabhängig (Faktor \~4 an trivialer
Box−Box-Szene), und smin-Stapel wachsen in Quadratur auf √m (nicht auf √2
gedeckelt, aber auch nicht (√2)^N). Kein globaler Skalierungsfaktor —
weder ÷√2 noch ein anderer — kann das Feld reparieren; `f − δ`-Offsets
erzeugen strukturell falsche Isoflächen (+497 % Distanzfehler, Phantom-Lappen,
Erosionsdefekt 22–59 % von δ superlinear an der Union-Naht).

**Fehler über CSG-Tiefe** (D exp4, Harness-Sweep): harte min/max sind
Kontraktionen in der Sup-Norm (Winner-Branch, keine Verstärkung; Baseline
misst monotones, aber flaches Wachstum durch Naht-Akkumulation). smin-Ketten
wachsen in Quadratur, realistische (räumlich getrennte) Ketten sättigen bei
\~1.24. Das Schreckgespenst „exponentielle Degradation über Tiefe" existiert
nicht; die Tiefe ist beherrschbar, die *Nähte* sind das Problem.

## 2. Bewerteter Lösungsraum

Bewertung je Ansatz: **Exaktheit** (was wird wie gut repariert), **Kosten**
(Lauf-/Bauzeit, Speicher), **Integrierbarkeit** (Passung zu Tape/WGSL-Mirror/
Weave-Pipeline). Evidenzgrad: gemessen / hergeleitet / plausibel.

### A. Nullstellen-treue Blends: Circular-CD-smin (Agent B) — ⭐ stärkster Einzelbefund

Quílez' Circular-CD-Variante hat **exakt dieselbe Nullstellenmenge** wie unser
`smin_poly` (Hausdorff 0.00 über 16 Konfigurationen — identische
Fillet-Geometrie), ist aber als Konvexkombination überall 1-Lipschitz:
|∇f| \= 1.000, Überschätzung 0, stabil bis Tiefe 12. Trace-Simulation des
\#386-Szenarios: 0 % Tunnel bei 4.0 Schritten mit *vollen* Schritten — 2.7×
schneller als die heutige ÷√2+0.75-Politik.

* Exaktheit: eliminiert die Normfehler-Klasse an Blends an der Wurzel;
  Feld*werte* auf der Naht-Bisektrix weichen ≤ 0.293·r ab (Unterschätzung —
  sicher, aber mesher-sizing-relevant).
* Kosten: identische Größenordnung pro Eval; ÷√2-Fade und
  `MASKED_BLEND_MARCH_RELAX` entfallen ersatzlos.
* Integrierbarkeit: Drop-in in `csg.rs::smin_poly`/`blend_min_eval` + WGSL/TSL-
  Mirror; Chamfer-Modus braucht ein eigenes CD-Gegenstück.
* **Offenes Risiko (einziges):** Lichen-Mesher-Regression durch die
  Bisektrix-Unterschätzung — Iteration-2-Experiment.

### B. Sichere Schrittweiten ohne Pauschal-Dämpfung (Agenten B, D)

Zwei komplementäre, gemessene Mechanismen ersetzen die Ad-hoc-`march_relax`:

* **Segment-lokale Lipschitz-Schranken** (Galin 2020; B H3): λ pro Opcode als
  Einzeiler aus vorhandenen Größen (Twist: 1+|τ|·max‖p‖ …), akkumuliert im
  Interpreter. Twist∘Box: 16.6 statt 47.6 Schritte, 0 % Tunnel (2.9×).
* **Tiefenbewusstes Relax als Sicherheitsnetz** (D H3): heutiges flaches 0.75
  ist *nachweislich unsicher* (koinzidenter m\=4-Stack penetriert −0.10);
  sicher ist 1/√(1+m) mit m \= maximale smin-Schachtelung des Tape-Pfads,
  zur Compile-Zeit aus dem `BoundedCsg`-Baum — 0 Laufzeitkosten. Wird durch
  Ansatz A weitgehend obsolet, bleibt Netz für Chamfer/Displace/Exoten.

Exaktheit: betrifft nur Trace-Sicherheit, nicht Feldbeträge. Kosten: null bis
negativ (weniger Schritte). Integrierbarkeit: hoch — `march_relax`-Mechanismus
existiert schon, wird präzisiert statt neu erfunden.

### C. Nahfeld-Wahrheit aus dem B-Rep-Proxy (Agent C)

Distanzabfrage gegen das tesselierte Weave-B-Rep (`tessellate_brep`):
im Nahfeld **exakt bis tol\_mm, mit exakten scharfen Kanten** — dem
O(h)-Kanten-Schmieren jedes Gitters überlegen. Hybrid-Blend Feld↔Proxy im Band
überschießt nie (sphere-trace-sicher), Gradient-Spikes nur wenn Band \< 4×
Feld-Diskrepanz. BVH-Punkt-Mesh-Queries CPU-tauglich.

* Exaktheit: repariert **beide** Fehlerklassen im Nahfeld, einschließlich der
  gradienten-blinden Fußpunktfehler; einzige gemessene Quelle magnitudentreuer
  Offsets/Shells nahe der Fläche.
* Kosten: BVH-Aufbau pro Body nach Regen (asynchron im Weaver-Worker denkbar);
  Query-Kosten CPU-seitig gemessen tragbar, GPU-Pfad offen.
* Integrierbarkeit: mittel — braucht fertiges Weave (Latenz nach Regen,
  vgl. #386-Gotcha „Fillet-Query löst spät auf"); CPU-Abfragen (Messen,
  Wandstärke, Offset-Samples, Mesher-Seeds) sofort, Render-Pfad später.
* **Offenes Risiko:** erscheinen Edge-Fillet-Zonen als gerundete Faces im
  Weave-Output? (Iteration-2-Experiment.)

### D. Seitenselektives Voxel-Redistancing on-demand (Agent A)

Fast Sweeping auf 200³ (`api/voxel.rs`-Skala): 0.4 s single-thread, 64 MB,
Fehler innen 0.133 → 0.010 max. Eikonal-Löser rundet konkave Kanten **nicht**
(Zero-Set-Drift O(h²), Kink bleibt); die gefürchtete Rundung entsteht erst beim
trilinearen **Resampling** (O(h)).

* Exaktheit: repariert Fußpunktfehler flächig, aber nur bis O(h)-Abfragefehler;
  muss seitenselektiv angewandt werden (sonst verschlechtert es die exakte
  Seite auf 0.23·h).
* Kosten: einmal pro Body nach Feature-Regen; Sampling des echten Tapes
  dominiert (macht `voxelizeOccupancy` heute schon in einem Call).
* Integrierbarkeit: hoch für **Abfrage-/Korrekturschicht** (Wandstärkenanalyse,
  Offset-Vorschau, innere Distanzen); ungeeignet als Ersatz-Renderfeld.
* Gegenüber C: schlechter an Kanten (O(h) vs exakt), besser im tiefen
  Inneren/Fernfeld und ohne Weave-Abhängigkeit.

### E. Punktweise Tape-Redistanzierung: Hopf–Lax (Agent A)

Gitterfrei, nachbarfrei, d(x) über iterierte Ball-Minima des Feldes; repariert
auch das √2-Feld ohne Strukturkenntnis (0.056 → 0.0016). \~440 Feld-Evals pro
Punkt („cheap"-Modus), mit `sdf_dual.rs`-Gradienten \~¼ davon.

* Exaktheit: hoch, beweisbar sicher iterierbar. Kosten: zu teuer pro Pixel,
  richtig für **einzelne analytische Abfragen** (Messen, Wandstärke,
  Offset-Stützpunkte). Integrierbarkeit: hoch (nur Feld-Evals nötig).

### F. Strukturelle Tape-Rewrites: Offset/Shell (Agenten A, D)

* **Dilation kommutiert mit Union** (D, exakt, gratis): `min(dᵢ)−δ =
  min(dᵢ−δ)` — Outward-Offset darf durch min-Knoten gepusht werden (dual:
  Erosion durch max). Reiner Compilezeit-Rewrite, deckt „Offset auf
  Union-of-Parts" verlustfrei ab.
* **Ball-Extremum-Offset** (A H5, plausibel/ungetestet): morphologische
  Dilation min über Ball hat beweisbar exaktes Zero-Set auch auf Pseudo-SDFs;
  WGSL-Kosten offen.
* Merksatz aus B H5: Außen-Offset über Union und Innen-Offset über
  Intersection sind *schon heute exakt*; kaputt sind nur die dualen Fälle.
  Ein Offset-Feature kann also zur Compile-Zeit entscheiden, ob es exakt ist.

### G. Normalisierung als Metrik-Politur: AD-Sampson, vertrauensbandiert (Agent D)

f/|∇f| mit **AD-Gradient** (Zweikanal-(val,grad)-Eval durchs Tape; FD ist an
min/max-Tie-Flächen nachweislich kaputt — mittelt Äste, macht f/|g| bis 1.4×
zu groß, wo f exakt ist). 10–25× Nahband-Verbesserung bei Normfehlern; blind
für Fußpunktfehler; als Trace-Schranke ungültig; muss auf |f| ≲ r\_blend
bandiert werden (bei großem δ wird Sampson schlechter als roh).

* Rolle: Politur für Mesher-Projektion und Offset-Genauigkeit in Blend-Zonen,
  **nachdem** A–F die strukturellen Fehler adressiert haben. AD-Infrastruktur
  (val,grad) nützt zusätzlich B (λ-Akkumulator) und E (¼ Evals).

### Verworfen / negativ (dokumentiert, nicht wieder anfassen)

* Globale Skalierung (÷√2 o. ä.) als Reparatur — strukturell unmöglich (C, D).
* `f − δ` als Offset auf Nicht-SDF — nicht durch Skalierung heilbar (C).
* Volles (nicht seitenselektives) Redistancing — verschlechtert exakte
  Seiten (A).
* Sampson/Normalisierung als Sicherheits- oder Fußpunkt-Korrektur (D);
  iterierte Normalisierung k≥2 (divergiert im Fernfeld, D).
* Rvachev-R-Funktionen roh (L bis 3.3, unsicher; B H4).
* Radius-Fade statt Crossfade im MaskedBlend (bringt nichts, B H2).
* Flaches `march_relax` 0.75 als Sicherheitsgarantie (D, Gegenbeispiel).
* FD-Gradienten für jede metrische Korrektur (D H5).

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
   Stacks (m\=2..8) und echten Fillets (Penetrationsrate, Schrittzahl-
   Regression); Zweikanal-AD in `eval_opcodes` als CPU-Referenz,
   validiert an Offset/Shell-Isoflächen-Genauigkeit (2D-Prognose −15 % → −2 %).

## 5. Iteration 2 — Ergebnisse (echter Kernel, echte Tapes)

Alle vier Experimente aus §4 wurden durchgeführt (Artefakte: `agent-*/iter2/`,
je LOG-iter2.md + Patch + CSVs; Patches NICHT gemergt, liegen als `*.patch`).

### Exp 1 — Circular-CD-Tausch: Rollout-Gate BESTANDEN, unter Auflagen

Patch in allen vier CPU-Spiegeln (`csg.rs::smin_poly`,
`sdf_compiler.rs::blend_min_eval`, `sdf_dual.rs`, `fidget_bridge.rs`).
Gemessen: Zero-Set-Parität ≤ 2.6e-5 mm (smin-freie Szenen bit-identisch);
‖∇f‖ 1.22 → 1.0001; Überschätzung → 0; Trace mit vollen Schritten ohne ÷√2
tunnelfrei und 1.5× schneller als heute (2.0× vs ÷√2+relax). **Das Alt-Feld
tunnelt real** (chain\_02 1.77 %, chain\_16 2.93 % bei vollen Schritten).
Mesher-Gate: alle 220 fillet/mesh/weave/lichen/golden-Tests grün — die
Bisektrix-Unterschätzung schlägt NICHT auf Mesh-Sizing durch. Einziger
Testbruch kodierte die Alt-Innen-Inflation als Soll (Fix im Patch).
Auflagen: WGSL/TSL-Mirror im selben Commit; MaskedBlend-Visual-Pfad hat mit
CD einen harten Wertesprung (1−1/√2)·r an der Masken-Tube-Grenze (Nullmenge
unberührt — Crossfade spiegeln oder dokumentieren); Chamfer behält Dämpfung
bis zum CD-Gegenstück; CPU-Kosten +18–31 % auf smin-lastigen Szenen.
Neu: CD-Unterschätzung ist nichtlokal (chain\_16-Fernfeld bis 0.45·r
konservativ zu kurz, wo alt exakt war — für |f|-Abfragen relevant).

### Exp 2 — Weave-Proxy: Fillet-Risiko BESTÄTIGT, Ausweg gefunden

Die Fillet-Zonen-Frage ist mit NEIN beantwortet, dreifach belegt: der
Produktions-Weave wendet Edge-Fillets gar nicht an (11/11 planare Faces am
gefilleteten Body); der Direkt-Weave webt zwar 1 Blend-Face, aber
`tessellate_brep` verweigert („procedural blend carrier", dazu
Runout-Loop-Fehler); der scharfe Proxy liegt im Fillet-Band exakt um
r·(√2−1) daneben, tol-unabhängig. Zusätzlich: `tessellate_brep` scheitert
auch fillet-frei an durchstoßenden Unions; Genauigkeits-Boden 0.123 mm aus
der Naht-Polyline. **Ausweg gemessen:** das DC/MDC-Mesh des gefilleteten
Felds als Proxy — Fillet-Band mean \~0.001 mm, 3 Größenordnungen besser als
Feld/Tape. Auf tesselierbaren Weaves gilt Nahfeld-Fehler ≡ tol (bis
0.0076 mm); Phantom-/Innen-Fehler 2–3.5 mm → 0.01–0.04 mm. Korrektur an
Iteration 1: „Hybrid-Blend überschätzt nie" bricht bei Fillets (hartes Feld
dort lokal Überschätzer); ∇-Spikes stammen primär aus
Vorzeichen-Inkonsistenz, nicht Bandbreite.

### Exp 3 — Offset/Shell-Showdown: Ball-Extremum gewinnt, Klassifikation gratis

`plate_bore` (Differenz) mit analytischer f64-Ground-Truth: roh `f−δ` ist
topologisch falsch — Fan-Fehler (√2−1)·δ bestätigt, und ab δ > Überstand des
Subtrahenden wird die Bohrung zugedeckelt (Phantom-Material, unbeschränkt).
**Ball-Extremum-Dilation:** mean \< 1 % von δ ab \~45 Evals/Query, einseitig
konservativ, funktioniert unverändert auf MaskedBlend-Pseudo-SDFs;
Negativbefund: max-Fehler plateaut 1.8–3 % (Torus-Zonen). **2×2-Pushdown
gemessen:** Union-outward + Differenz-inward roh exakt (2e-6), duale Fälle
kaputt (Union-innen bis 9.4 mm) — zur Compile-Zeit entscheidbar. **FSM in 3D
bestätigt seitenschädlich** (exakte Seiten 0 → 0.1–0.23 mm) und für
Offset-Isoflächen überall schlechter als Ball; Nische Wandstärke-Heatmaps
(Break-even \~1200 Punkte). Wandstärke-Einzelpunkte: Hopf–Lax 64 dirs
(71 % Unterschätzung → 0.03–0.1 mm, \~51 µs, 64 dirs dominiert 32 in
Genauigkeit UND Kosten; Achtung positiver Sampling-Bias). Nebenbefund:
Harness-Kompass-GT überschätzt an konkaven Junction-Kreisen bis 0.14 mm.

### Exp 4 — Relax-Sicherheit + AD: Status quo undicht, beide Verdicts positiv

**Der heutige Kernel dämpft SmoothUnion gar nicht** — realistische
smin-Ketten tunneln heute 0.6–1.8 % der Rays bis 0.57 mm. Flaches 0.75:
m\=4-Stack 24.3 % Penetration (max 1.77 mm), m\=8 42–46 %; tiefenbewusstes
1/√(1+m): 0 %. MaskedBlend ist dank ÷√2 stapelfest (Neutralität bestätigt).
Negativbefund: Pfadzählung überkonservativ auf getrennten Ketten (chain\_16:
3.5× Schritte) — braucht Blendzonen-Ko-Lokalisierung, falls ohne CD-Tausch
produktiv. **Zweikanal-AD** (`sdf_tape_ad.rs`-Prototyp): Wertparität 1.9e-6,
Kosten 1.0–2.2× eines Evals (FD: 7–8×); FD-Tie-Artefakt in 3D bis 1.93×
(AD exakt 1.000); bandiertes Wurzel-Sampson: Offset-Iso −13.7 % → −0.6 %,
Shell −12.7 % → −1.0 %, exakt blind für Fußpunktfehler (Taxonomie bestätigt).

## 6. Finale Empfehlung: Kernel-Integrationspfad

Reihenfolge nach Nutzen/Risiko; jede Stufe unabhängig shipbar und durch
Messdaten gedeckt. Ausgangs-Patches liegen in `agent-*/iter2/*.patch`.

**Stufe 1 — Circular-CD-smin (fix + perf zugleich).** Tausch in den vier
CPU-Spiegeln + WGSL/TSL-Mirror (`sdfVmInterpreter.ts`) in EINEM Commit;
MaskedBlend-Visual-Crossfade spiegeln (Wertesprung-Auflage); danach ÷√2-Fade
und `MASKED_BLEND_MARCH_RELAX` entfernen. Behebt reale Tunnel im heutigen
SmoothUnion-Pfad und macht Traces 1.5–2× schneller. Gate war grün (220
Mesher-Tests), ein Alt-Semantik-Test wird mitgefixt. Chamfer: Dämpfung
behalten, CD-Gegenstück als Folgearbeit.

**Stufe 2 — Lipschitz-Netz zur Compile-Zeit.** `lipschitz_sq_bound` aus dem
Exp-4-Patch als Sicherheitsnetz für Chamfer/Morph/Displace/Exoten (ersetzt
flache Konstanten); für smin nach Stufe 1 neutral. Vor Aktivierung auf
smin-Pfaden ohne Stufe 1: Blendzonen-Ko-Lokalisierung ergänzen.

*Umgesetzt (v0.16.3, `sdf_compiler::lipschitz_bound`):* `march_relax = 1/L`
mit hergeleiteten, komponierenden Schranken — Twist/Bend `L·(1+|rate|·r_max)`
(Hebelarm aus der Kind-AABB), Displace additiv `+|depth·freq|·√3.18`, Chamfer
(auch MaskedBlend-Chamfer) `max(L_a, L_b, (L_a+L_b)/√2)`, Fillet-CD und harte
Booleans neutral (max), Morph Konvex-Lerp. **Revolve ist als 1-Lipschitz
hergeleitet** (Meridian-Map = Umkehr-Dreiecksungleichung + exakte analytische
2D-Pfaddistanz) — die 0.7-Pauschale aus Ticket 1wnm war unnötig, empirisch
bestätigt (max |f|/|d| \= 1.0001, 0 Tunnel bei vollen Schritten, \~39 %
weniger Schritte). Messung `stage2_relax_net`: twist\_box 5.9 % → 0 %,
bend\_bar 16.2 % → 0 %, twist\_thin\_wall 3.8 % → 0 %, chamfer\_plates
5.3 % → 0 % Penetration (Schrittzahl dort \~1.6–2×); gyroid\_box bleibt bei
relax 1 tunnelfrei (Feld-Normalisierung 0.2887 \= 1/(2√3), L ≤ √6/(2√3)
\= 1/√2). Ko-Lokalisierung erwies sich als unnötig: smin ist nach Stufe 1
neutral, Chamfer-Ketten konvergieren gegen L∞ \= 1/(√2−1) ≈ 2.414. Ehrliche
Rest-Überkonservativität: EIN Skalar pro Körper (der schlechteste Zweig dämpft
den ganzen Body) und der Chamfer-Ketten-Fixpunkt auch bei räumlich getrennten
Fasen; Sweep-Krümmungsstreckung (1/(1−κ·r) nahe am Gate) weiterhin ungedeckt
(Faktor 1.0 wie zuvor).

**Stufe 3 — Offset/Shell korrekt.** (a) Compile-Zeit-2×2-Klassifikation:
exakte Fälle (Union-outward, Differenz-inward) wörtlich `f−δ`; (b) kaputte
Fälle im Abfrage-/Mesher-Pfad via Ball-Extremum (\~16 dirs × 4 Starts,
\~170 Evals/Query) — nie per Pixel; (c) `f−δ` auf Differenz-außen nirgends
mehr anbieten (topologisch falsch). Wandstärke: Hopf–Lax-64 punktweise,
seitenselektives FSM nur für Heatmaps.

**Stufe 4 — Zweikanal-AD als Infrastruktur.** `sdf_tape_ad.rs` produktiv
(CPU zuerst): exakte Gradienten für Mesher-Projektion, bandiertes Sampson
(Offset-Politur), λ-Akkumulation, Hopf–Lax-Beschleunigung. FD-Gradienten in
metrischen Pfaden abschaffen. WGSL-Version (4 Register/Slot) separat.

**Stufe 5 — Nahfeld-Wahrheitsschicht (Forschung → Produkt).** Kurzfristig:
DC/MDC-Mesh als Proxy, wo das Fillet-Gate zieht (O(0.2 mm)). Mittelfristig:
Blend-Face-Tesselation in `tessellate_brep` (Spike, s. §7), dann Weave-Proxy
mit Fehler ≡ tol als Quelle für Messen/Offset-Samples/Mesher-Seeds.

Nicht tun (jetzt dreifach belegt): globale Skalierungen als Reparatur,
`f−δ` auf Nicht-SDF-Seiten, nicht-seitenselektives Redistancing als
Feld-Ersatz, FD-Gradienten für Metrik-Korrekturen.

## 7. Offene Punkte für Iteration 3

1. **Blend-Face-Tesselations-Spike** in `tessellate_brep` (inkl.
   Runout-Loop-Topologie) — hebt den Weave-Proxy auf Fehler ≡ tol im
   Fillet-Band (Erwartung r·(√2−1) → ≤ tol) und adressiert nebenbei die
   durchstoßende-Union-Verweigerung.
2. **MaskedBlend-Visual-Crossfade** unter CD (Wertesprung-Auflage aus Exp 1).
3. **Chamfer-CD-Gegenstück** (Formel-Herleitung + gleiche Gates).
4. **Blendzonen-Ko-Lokalisierung** für das Lipschitz-Netz (nur nötig, wenn
   Stufe 2 vor/ohne Stufe 1 aktiviert wird).
5. Harness-GT an konkaven Junction-Kreisen härten (0.14-mm-Befund aus Exp 3).

## 8. Bezug zu #384 (Opus-Hypothesen) und #386 — Stand Iteration 1

* Opus' Karte der verlierenden Ops: bestätigt und präzisiert (seitenselektiv).
* Voxel-FSM on-demand: bestätigt machbar (0.4 s/64 MB), aber als
  Abfrageschicht, nicht Renderfeld; seitenselektiv nötig — sonst schädlich.
* „PDE-Reinit verworfen": in der Pauschalform überholt — gitterbasiert als
  Renderfeld ja (Resampling rundet), als Korrekturschicht nein (A2/A3).
* „Einzigartiger Gewinn echter Redistanzierung \= betragskorrekte Offset/Shell":
  bestätigt, mit konkreten Zahlen und drei konkurrierenden Wegen (C/E/F).
* \#386 (graue Wand): B quantifiziert das rohe smin-Feld im Szenario mit 85 %
  Tunnelrate; die heutige Politik verhindert Tunneln bereits — konsistent mit
  dem Memory-Befund, dass die Wand *kein* Overstep-Problem ist. Der
  Circular-CD-Tausch entschärft die Klasse trotzdem strukturell.

## 9. Strukturelle Konsequenz: Offset/Shell als Baum-Transformation

Entstanden im Betreiber-Dialog nach Iteration 2 (2026-07-18) — nicht aus den
Experimenten, sondern aus drei berechtigten Einwänden gegen deren
Konsequenzen-Teil: (a) Mesher sind keine Ground Truth (dünne Features →
Löcher/Ausfransungen), (b) Voxel/Octree erreichen auf Bauteilgröße keine
CAD-Auflösung (0.05–0.001 mm), (c) ein persistenter B-Rep-Rückfall wäre
„Parasolid nachprogrammieren". Die Experimente lieferten die Grenzen des
Feldes; dieser Abschnitt beantwortet, was man stattdessen baut.

### Doktrin

Das Feld ist Renderer und Innen/Außen-Klassifikator — **kein Messgerät**.
Vorzeichen + Nullstellenmenge sind immer wahr; 1-Lipschitz ist überall
erreichbar (CD-smin + Lipschitz-Netz; harte min/max sind ohnehin 1-Lipschitz
und unterschätzen nur → march-sicher). Der Betrag ist unter CSG prinzipiell
unrettbar (Fußpunktfehler sind nichtlokal). Also: **Betrag-konsumierende
Operationen (Offset, Shell, …) fragen nie das Feld, sondern werden zur
Regenerationszeit strukturell wegkompiliert, bevor das Feld entsteht.**

### `offset_tree`: (CSG-Baum, Weave-Adjazenz) → CSG-Baum

Offset/Shell als Compiler-Pass auf Baum-Ebene (Makro-Expansion je Regen; das
Feature speichert nur δ). Ausgabe ist wieder ein nativer Baum → kompiliert
zum Tape, rendert, boolt und fillet wie jeder Körper. Der Weave wird dabei
nur als *Entscheidungstabelle* gelesen (Kanten-Adjazenz + Konvexität), nie
als Geometrieträger behalten — kein persistentes B-Rep, kein Vernähen.

1. **Blätter analytisch offsetten** — die Trägerklassen sind
   offset-abgeschlossen: Ebene→Ebene, Zylinder/Kugel→r±δ, Kegel→Spitze
   verschoben, Torus→Rohrradius±δ. **Sketch-Familie ist die gute Nachricht:**
   Extrude = exakte Extrude-Formel über exaktem 2D-Profil-SDF, Offset =
   Extrude des 2D-offsetteten Profils (2D-Offsetting ist gelöst,
   Clipper-Klasse); Revolve kommutiert mit Offset (nächster Punkt liegt in
   der Meridian-Halbebene → 3D-Distanz = 2D-Profildistanz in (r,z)). Twist/
   Bend bleiben ausgenommen (keine Isometrie — exakter Offset existiert dort
   bei niemandem, auch nicht in Parasolid).
2. **Vertauschbarkeit ist genau das 2×2-Ergebnis:** exakt für Union⊕außen
   und Differenz⊖innen (Subtrahend bekommt den *entgegengesetzten* Offset);
   die dualen Fälle differieren **exakt lokalisiert** um Kanal-Körper entlang
   der Schnittkanten (außen über Schnitt: scharfe statt Rolling-Ball-Kante,
   zu viel Material; innen über Union: fehlendes Material an der konkaven
   Naht). Korrektur = endliche Liste von Kanten-/Ecken-Korrekturkörpern, je
   Konvexität addiert/abgezogen — **Rossignac & Requicha 1986** („Offsetting
   operations in solid modelling"); der Weave liefert die Klassifikation,
   die dort noch symbolisch hergeleitet wurde.
3. **1D-Ökonomie schlägt 3D:** Die Korrekturkörper sind Kanal-Flächen um
   Raumkurven (Distanz-zu-Kurve, für allgemeine Quadrik-Schnitte nicht
   closed-form) — aber eine 1D-Kurve auf µm zu approximieren (Arc-Spline)
   kostet eine Handvoll Segmente mit closed-form-Distanz. Das
   3D-Auflösungsproblem, das Voxel/Mesh disqualifiziert, existiert für
   Kurven nicht.
4. **Nicht Parasolid:** Übernommen wird nur der leichte Teil
   (Kanten aufzählen/klassifizieren, Korrekturkörper einsetzen). Der harte
   Parasolid-Kern — wasserdichtes Vernähen, tolerantes Modellieren,
   NURBS-SSI — entfällt, weil das Ergebnis implizit bleibt und die Booleans
   das Feld gratis auflöst. Ground Truth bleibt die Feature-Historie.

### Grenzen (ehrlich)

* Hartes Restproblem: **Topologiewechsel bei großem δ** (Flächen sterben,
  Bohrungen schließen sich *korrekt*) — die Expansion muss das erkennen,
  sonst emittiert sie tote Korrekturkörper. Für den CAD-Brotfall (Shell mit
  Wandstärke ≪ Bauteilgröße) sind die Korrekturen klein, lokal, gutartig.
* Der expandierte Baum ist wieder CSG: Nullstellenmenge exakt, Nahfeld
  gesund, Fernfeld-Betrag mit den üblichen min/max-Grenzen; Offset-auf-Offset
  expandiert erneut (Baumwachstum).
* Einordnung der Feld-Werkzeuge aus §2/§6: Ball-Extremum bleibt das korrekte
  *Orakel*/Validierungswerkzeug (einziger beweisbar richtiger
  Punkt-Klassifikator auf kaputtem Feld), seitenselektives FSM bleibt
  Mess-Nische (Wandstärke-Heatmaps) — beide sind **keine** Antwort für
  Live-Rendering oder Geometriebildung.

**Vorgeschlagener Iteration-3-Kern:** `offset_tree`-Spike (Fälle 1–2 auf
Extrude/Revolve-Blättern + Kanal-Korrektur an einer Beispiel-Kante,
Ground-Truth-Vergleich via metricity-lab) — vor der
Blend-Face-Tesselation aus §7, falls Offset/Shell fürs Produkt zentral ist.
Ergänzend geparkt: #389 (Render-Modus „sdf-vm (dual)", Zweikanal-AD durchs
Tape für lokale Schrittweiten + exakte Normalen).

**Spike DURCHGEFÜHRT (2026-07-18) — Konzept trägt.** Volle Messreihen in
`iter3-offset-tree/LOG.md` (+ CSVs, Prototyp
`metricity-lab/src/bin/iter3_offset_tree.rs`). Verdikte: **(A)** Extrude
eines Path-Profils und achsenfernes Revolve sind heute schon exakte SDFs
(≤ 2.7e-4 mm; Meridian-Identität am Kernel verifiziert); nicht exakt sind
nur 2D-Booleans im Profil, `Sketch2d::Offset` (f−δ) und — neuer
Kernel-Befund — achsberührendes Revolve (Phantom-Nullmenge auf der Achse).
**(B)** Das wörtliche §9-Keil-Rezept konvergiert NICHT (Restfehler ~0.27·δ
bei schiefem Dihedral); mit **Normalkegel-Gating am nächsten Kurvenpunkt**
konvergiert der Fehler δ-unabhängig auf die Kurventoleranz: ≤ 0.001 mm mit
128 Bögen gesamt, ~720 ns/Eval; Arc-Splines brauchen ~8× weniger Elemente
als Polylinien. Außenfeld march-sicher. **(C)** Die R&R-Zerlegung ist als
Mengen-Identität für jedes δ exakt (Topologiewechsel automatisch korrekt);
Restarbeit: Blatt-Parameter-Kollaps (parametrisch/Clipper, billig),
Tote-Körper-Pruning (Intervall + Ball-Orakel-Witness), Weave-Refresh nur für
Folge-Feature-Adjazenz. Konvexitätsbeweis für C2 gilt konvex; kaskadierte
Offsets und nichtkonvexe Kegel-Überlappungen sind die offenen Ecken.