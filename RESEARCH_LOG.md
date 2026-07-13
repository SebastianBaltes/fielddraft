# RESEARCH_LOG — Lichen-Session

Format: Zeitstempel · Hypothese/Frage · Ergebnis · Entscheidung.

## 2026-07-13 01:30 — Session-Start (nach Kontext-Reset)

- Vorherige Turns: Metrik-Modul `lichen_metrics.rs` gebaut (Hausdorff beidseitig,
  Aspect, deterministisch), Gyroid-Fixture r=15 + Validierung + Baseline-Runner
  in `api/lichen.rs` (uncommitted). Recherche-Agenten hatten geliefert:
  Zeugen-Kugel ≙ surface Delaunay ball, CBP als exakte Manifold-Bedingung,
  loose ε-sampling (Boissonnat–Oudot) mit ε≈0.1–0.16, CGAL-Kriterien.
- Betreiber-Hinweis: Abnahme-Testfall = Projekt 7d9701… (.fdraft hochgeladen).
  Befund: **identisch mit `demo_project_kernel()`** — inkl. Box-Ausschnitt
  (3,4,3)@(7.97,22.31,29.87) und Marker-Kugel r=1. „Ausschnitt“ im DoD ist
  wörtlich: der Testfall IST der kleine Ausschnitt → schnell genug für CI.
  Zusätzlich liefert die Datei die Betreiber-Settings (u.a. curvAngleDeg 2,
  lfsFactor 0.1, sizingField true, accept witness).

## 2026-07-13 02:00 — Fixture-Validierung schlägt fehl → Gyroid-Semantik

- Hypothese: Fixture r=15 enthält dünnwandiges Gyroid (1-mm-Wand, ~30 % Material).
- Messung: 82 % Material, tiefster Innenpunkt −2,89 mm, nur 2 Vorzeichenwechsel
  auf 30 mm. Code-Lektüre `sdf_eval.rs:325`: SDF = `(|f(p/s)|−t)·s·0.2887` —
  **t dimensionslos, Periode 2π·s ≈ 62,8 mm**. thickness=1 ⇒ |f|<1 ⇒ dicke Wände.
- Konsequenz: r=15-Fixture (< ¼ Periode) ist NICHT repräsentativ; meine
  Validierungs-Annahme („Luft > 700“) war falsch begründet. Der ehrliche
  Testfall ist das Demo-Projekt selbst (Ausschnitt = klein = schnell).
- Entscheidung: `gyroid_sphere_kernel(r)`-Fixture streichen; Validierung,
  Morphologie-Probe und Baseline auf `demo_project_kernel()` umstellen;
  Hausdorff-Sampling-Box = Ausschnitt-AABB ∪ Marker-AABB + Rand.
- Notiz (nicht diese Session): Feature-Doku sagt „Distance between repeating
  cells in mm“ — stimmt nicht mit der Implementierung (Periode 2π·cell) überein.
  Semantik-Änderung würde bestehende Projekte brechen; nur dokumentieren.

## 2026-07-13 02:40 — Baseline + Orts-Forensik der offenen Kanten

- Baseline (Betreiber-Settings, release, 12 s): pts 5746, tris 11137,
  **397 offene Kanten**, 0 nonmanifold/orient/degeneriert, 1 Komponente,
  H(m→s) 0,010 mm, H(s→m) max 0,080 / p95 0,0035 mm, Aspect p99 1,78,
  min. Winkel 18,6°. → Die EINE scheiternde Metrik ist Manifold/offene Kanten;
  Hausdorff+Aspect sind schon gut, die Risse sind schmal.
- `refine:true, refineRho:1.0` (v0.6.563): **No-op** — identisch 397 offen.
  Bestätigt: kein Dichte-Problem, sondern Topologie/Vernähung.
- Forensik (`zz_gyroid_open_edge_clusters`): 397 Kanten in 15 Ketten-Clustern:
  - 258 auf der **Gyroid∩Zylinder**-Schnittkurve (über den ganzen Ausschnitt),
  - 72 auf **Marker∩Zylinder** (Marker r=1 @ Radialabstand 29,7 durchstößt r=30),
  - ~40 auf **Box∩Gyroid**-Nähten, Rest Mikro-Löcher auf Gyroid∩Zylinder.
- Hypothese H1: Alle Risse liegen auf SDF-Knick-Kurven (Zwei-Flächen-Schnitten),
  für die es KEINE B-Rep-Kette gibt — Gyroid ist vom B-Rep ausgeschlossen
  (`step_classifier.rs:220`). Ohne Kette: kein Ketten-Sampling, kein Vorhang,
  keine Pflichtpunkte auf der Naht → beide Blätter wachsen an den Knick heran
  und werden nicht vernäht. Subagent prüft die Ketten-Pipeline (läuft).

## 2026-07-13 03:40 — H1 präzisiert: Ketten EXISTIEREN, der Konsum scheitert

- Empirie (`zz_gyroid_brep_edges`): der Seed-Weaver liefert dem Body 16 Kanten
  — inkl. Gyroid∩Zylinder (977 Samples), Gyroid∩Box, Marker∩Zylinder. H1
  („keine Ketten") in der starken Form widerlegt; alle Nähte haben Ketten und
  werden als Pflichtpunkte (flag 2) eingespeist.
- Flag-Forensik: im Hauptcluster 106 offene KETTEN-Segmente (nur 1 Dreieck
  statt 2) + 123 Flächenpunkt-Risse parallel zur Naht → EINE Seite der Naht
  vernäht, die andere scheitert.
- `empty_why`-Instrumentierung (Diagnose-Zwilling von `empty`): zwei Ursachen:
  (a) Zeugen-Newton scheitert an Ketten-Slivern (degeneriert, unkritisch),
  (b) KONTAMINATION: an der fast-tangentialen Naht ist der Blattabstand
  (~0,04 mm) kleiner als der Punktabstand (~0,1 mm) — jede Zeugen-Kugel
  enthält Gegenblatt-Punkte, beide Blätter blockieren sich WECHSELSEITIG.
  Der Normalen-Guard kann tangentiale Blätter prinzipiell nicht trennen.

## 2026-07-13 04:10 — Iteration 1: blatt-restringierte Zeugen-Kugel (v0.6.564)

- Fix: Leere-Kugel-Prüfung zählt nur Punkte, die eine B-Rep-REGION mit dem
  Dreieck teilen (restricted Delaunay je glattem Patch, Cheng–Dey–Levine);
  Einfärbung+Regionen laufen immer, Flächen-Guard bleibt opt-in.
- Messung: 397 → **312** offene Kanten; ALLE Mikro-Löcher zu (15 → 4 Cluster);
  33/33 Lichen-Gates grün (Box/Zylinder/Kugel/Torus/Platte unverändert).
- VERWORFEN (vermessen, zurückgebaut):
  - Host-only-Kontamination (nur Wuchs-Fläche zählt): 312 → 384 — die dann
    unabhängigen Blatt-Triangulierungen verheddern sich im überfüllten Keil.
  - Gabriel-Fallback bei Newton-Versagen: 384 → 478 — Kugeln ohne
    Flächen-Zentrum lassen falsche Dreiecke zu.
  - faceGuard:true (korrekt gemessen nach JSON-Merge-Fix): wirkungslos (386).
- WERKZEUG-BUG gefunden: LICHEN_GY_EXTRA per String-Anhängen erzeugte
  Duplikat-Keys → serde-Fehler → DEFAULT-Optionen. Frühere faceGuard/refine-
  „Messungen" mit überschreibenden Keys waren Phantome. Jetzt echtes
  JSON-Merge im Baseline-Runner.
- Crowding-Hypothese bestätigt: minEdgeSpacing 0.3 → 155 offene Kanten (aber
  H(s→m) 0,26 = untersampelt). Das Sizing-Field kollabiert die Dichte im
  Naht-Keil (lfs→0 am Knick), Blätter verschachteln sich. → Nächster Schritt:
  SCHUTZKORRIDOR um Ketten (Cheng–Dey–Ramos protecting balls): keine
  Flächenpunkte, wo Blattabstand < Punktabstand; Korridor-Breite aus
  Keilwinkel der beiden Ketten-Normalen.

## 2026-07-13 04:50 — Ideen-Agent-Review des Korridor-Plans

- Web-Suite grün (1679/1679), REPORT.md-Gerüst angelegt.
- Ideen-Agent (read-only) bestätigt Diagnose, warnt vor drei Korridor-Fallen:
  (1) Lücken-Füllung (lichen.rs ~1054+, #895) füllt den Korridor wieder auf —
  muss gaten; (2) Marker∩Zylinder ist geschlossener Mikro-Ring r≈1 — Korridor
  mit sinθ-Klemme 0.05 (R≈2 mm) verschluckt die ganze Kappe; (3) R-Sprünge an
  der sinθ-Klemmgrenze = Dichtesprung-Defektquelle → R längs Kette glätten.
- Bessere Hebel laut Review (an Implementierungs-Agent weitergegeben):
  1. GEWICHTETE Zeugen-Kugel / hidden vertices (power distance, Cheng–Dey–
     Ramos): Punkte im Schutzradius w_q eines Kettenpunkts werden im Walk
     komplett ignoriert (weder Kandidat noch Kontaminator) — Korridor am
     Prädikat statt im Wachstum: kein Refill, kein Dichtesprung im Wachstum.
  2. NAHT-SCHLITZ-STITCHING als deterministischer Post-Pass: offene Ketten-
     Kanten haben genau eine belegte Halbkante (§8c: Grad-2-Loops); dritten
     Punkt auf dem Gegenblatt suchen, Sliver verwerfen, crosses_chain achten.
     Nur additiv → metrik-sicher. Kandidat für den Rest nach Prädikat-Fix.
- Metrik-Risiken: Korridor gefährdet H(s→m)+Aspect (Untersampling/Randsprung),
  Snap-auf-Kette gefährdet H, Stitching nur Sliver-Risiko, weighted ~neutral.

## 2026-07-13 06:20 — Iteration 3 (Implementierungs-Agent): 312 → 21 (v0.6.565)

- KORRIDOR WIDERLEGT: Cheng–Dey–Ramos-Schutzkorridor voll implementiert und in
  13 Konfigurationen vermessen — nie besser als 287; ab β≥2 brechen H(m→s) und
  min-Winkel (Fächer über den Korridor slivern). Kern-Befund: reale Keilwinkel
  der Nähte sind sinθ ≥ 0.274 (16–24°) — die Blattverschachtelung ist ein
  RANDphänomen, nicht die Hauptursache. Korridor bleibt als Default-aus-
  Werkzeug (LICHEN_PROTECT_MODE=grow|hide, β/floor/ringdiv/lip per Env).
- DIE ECHTEN WURZELN (je per WHY-Forensik isoliert):
  1. Naht-Band-Einfärbung tang 25° → 10°: mit 25° trug jeder nahtnahe Punkt
     beide Regionen → Blatt-Restriktion blind („+0 fremdblatt-ignoriert").
     283 offene nach Fix (open-Verlauf 312→283, H(m→s) 0.026→0.012).
  2. Normalen-Guard-Ausnahme am Zwickel: 243/283 offene Kanten waren voll
     gültige Dreiecke, die NUR der 90°-Guard blockte (das Netz muss am spitz
     zulaufenden Gyroid∩Zylinder-Zwickel legitim >90° um die Spitze). Ausnahme
     nur bei STRIKT auf der Fläche liegender Sehne (0.25/0.5/0.75-Proben,
     |f| ≤ 0.15·min-spacing) — schließt zugleich das θ-skalierte Sichtlinien-
     Schlupfloch; dünne Platte bleibt geschützt. → 31 offene.
  3. Ketten-Verschweißung: Kettenpunkte VERSCHIEDENER B-Rep-Kanten in
     Sub-Resolution-Abstand (0.09 mm am Zwickel) teilen einen Mesh-Vertex
     (weld 1.5·minEdgeSpacing; 1.0→30, 1.5→21, 2.0→29 vermessen) +
     Kreuzungs-Veto-Ausnahme für Sub-Resolution-nahe fremde Ketten. → 21.
- Endstand: open 21, nm 0, orient 0, χ −4, H(m→s) 0.021, H(s→m) 0.098
  (p95 0.002), asp p99 1.85, minang 14.9°, 5738 pts. Suite 1078/1078,
  Lichen-Gates 33/33.
- Verworfen zusätzlich: Hidden-Vertex auf Finalstand (21→26), Host-only auch
  mit Korridor (351/303), Weld-Kollaps ohne Ersatzpunkt (U-Invariante reißt).
- Rest-21 = 4 Mikro-Zwickel, wo ≥3 fast-parallele Nähte konvergieren
  (größter @ [9.2, 26.2, 28.7]); Ablehnungen: 13 ketten-kreuzung, 5 nicht-
  leere Kugeln, 2 Walk-Sackgassen, 1 Guard. Empfehlung des Agenten: nicht
  Schwellen drehen (nicht-monoton), sondern topologisch — konvergierende
  Ketten am Zwickel bis zum echten Schnittpunkt verfolgen (Weave droppt den
  Verbindungs-Vertex, DL-71) bzw. off-vertex-schneidende Ketten-Segmente als
  EIN Kettenzug re-parametrisieren.

## 2026-07-13 09:30 — Iteration 4: 21 → 0 (v0.6.566, wasserdicht)

- Ansatz A (Segment-Segment-Verschweißung am Quasi-Schnitt) VOLL implementiert
  und WIDERLEGT: bei keinem Radius (1–3·weld_r) existiert ein echter
  Off-Vertex-Quasi-Schnitt — die nächsten Annäherungen fremder Ketten liegen
  immer an Segment-ENDEN (jenseits weld_r); die wenigen End-Snap-Einfügungen
  verschlechterten (21→23 bzw. 10→11). Code entfernt; der Phasen-Split
  (Ketten bauen → binden) bleibt als Ansatzstelle dokumentiert.
- DIE ECHTE WURZEL der 13 „ketten-kreuzung": neue Forensik (crossing_seg
  meldet das kreuzende Segment + Koordinaten) zeigt, dass das Veto fremde
  Nähte trifft, die 0,09–0,14 mm ÜBER der Dreiecksebene schweben — der Test
  PROJIZIERT jedes Segment im r_tri-Umkreis in die Ebene (r_tri = längste
  Kante ≫ Blattabstand). Kein Schneiden, reine Projektion.
  1. BLATT-RESTRIKTION im Kreuzungs-Veto (wie Zeugen-Kugel, Cheng–Dey–
     Levine): Kette ohne gemeinsame B-Rep-Region mit dem Dreieck kreuzt
     nicht (Region des Segments = gemeinsame Regionen seiner Endpunkte;
     ohne Info konservativ). 21 → 10.
  2. EBENEN-ABSTANDS-GATE: Segment ganz auf EINER Ebenenseite und weiter
     als 0,5·Spacing von der Ebene kann das Dreieck nicht durchstoßen
     (echte Auf-der-Fläche-Kreuzung liegt sagitta-nah, ≪0,5·Spacing) —
     fängt die Fälle, wo die Regionen-Restriktion mangels gemeinsamer
     Dreiecks-Region (gemischte Ecken am Drei-Flächen-Zwickel) blind ist.
- REST = Mikro-Loops (3–4 Kanten, alle Rand-Vertizes Grad 2): Schließ-
  Dreiecke voll orientierbar + strikt auf der Fläche, abgelehnt NUR vom
  Zeugen-Kriterium (Newton konvergiert am SDF-Knick nicht: „kontaminiert:
  0 Punkte"; bzw. Kugel im überfüllten Keil nie leer). NAHT-STITCHING als
  deterministischer Post-Pass (lichen_tri.rs, global walk): Loops ≤5 Kanten
  schließen, wenn jedes Dreieck `valid_geom` besteht (= valid OHNE leere
  Kugel: Degenerations-, Normalen-Guard, Sichtlinien θ-toleriert, Ketten-
  Kreuzung); 4-Loops über die erste manifold-freie Diagonale; additiv,
  belegt exakt die freien Halbkanten (Kill-Switch LICHEN_STITCH=0).
  Striktes |f|≤0.15·spacing im Stitch war zu streng: legitime Dreiecke
  WICKELN am Zwickel um die 90°-Box-Kante (Sehnen-Sagitta ~l/(2√2)).
- Messreihe: 21 (Start) → 23 (Segweld allein) → 10 (Blatt-Restriktion) →
  6 (+Stitching, strikt) → **0** (+Ebenen-Gate, Stitch=valid_geom).
  Endstand h=10: pts 5738, tris 11468, open 0, nm 0, orient 0, χ=2,
  H(m→s) 0,0213, H(s→m) 0,098 (p95 0,0022), asp p99 1,85, minang 14,9°;
  h=6 und h=14 ebenfalls open=0. Lichen-Gates 33/33, Suite 1078/1078.

## 2026-07-13 12:30 — Adversariale Härtung (Matrix aus 10 Fällen, 4 Klassen)

- MATRIX (`zz_adversarial_matrix`, ignored; Ist-Werte in der Doku des Tests):
  dünne Features (Gyroid-Wand 0,4 mm — thickness DIMENSIONSLOS, Wand =
  2·t·s/√3, die eigene Fixture-Validierung fing den Faktor-2-Irrtum;
  0,35-mm-Platte als Box-Differenz; 0,35-mm-Röhre), Domain-Warps
  (Displace-Kugel, Twist-Box), degenerierte Boolesche Kontakte (Linien-Kuss
  Zylinder@Box-Wand, Punkt-Kuss Kugel@Platte, koplanare Union), streifende
  Schnitte (Demo-Naht an ANDEREM Ort — erste Ortswahl hatte 0 % Material,
  ebenfalls von der Validierung gefangen; Kugel schneidet Ebene unter 3,6°).
- HÄRTUNG 1 — Ketten-Deckungs-Filter (koplanare Union 46 → 0 offene, χ=2):
  `global_chain_edges` wurde aus RING-Nachbarpaaren gebaut; bei „unordered
  fallback loops" des Weave (degenerierte koplanare Topologie) entstehen
  PHANTOM-Segmente zwischen nicht-benachbarten Ketten-Enden (gemessen: eine
  5,6-mm-Diagonale quer über eine ebene Fläche, Kreuzungs-Veto blockte jeden
  Fächer). Fix: nur Segmente, die konsekutiv auf einer abgetasteten
  B-Rep-Kanten-Kette liegen (`chain_seg_ok`) — für gesunde Loops beweisbar
  No-op (Gelenk-Vertizes sind Index-identisch).
- HÄRTUNG 2 — Regionen-Verschmelzung repariert (Punkt-Kuss 59 → 33):
  (a) Orientierungs-Gate: Flächen, deren nahe Punkte ANTIPARALLELE Normalen
  tragen (ndot < −0,7), verschmelzen nie — Rücken-an-Rücken-Blätter sind
  exakt das, was die Blatt-Restriktion trennen muss. (b) Berühr-Radius =
  1,5·min(space_i, space_j) statt nur space_i — der grobe Streu-Seed einer
  Orphan-Fläche (space=h) verschmolz sonst ALLE Flächen in 7,5 mm Umkreis.
- HÄRTUNG 3 — Stitch vervollständigt (dünne Gyroid-Wand 10 → 7):
  kanten-basierter Loop-Walk (Halbkanten-Konsum) statt Vertex-Nachfolger-Map
  (Grad-4-Zwickel-Vertizes ließen ZWEI Loops offen) + 5er-Loop-Fächer;
  weiterhin valid_geom-Pflicht je Dreieck, Manifold-Schutz via edge_use.
  Nebenwirkung positiv auch auf die WIP-Fälle: Displace 813→662,
  Twist 927→864, Linien-Kuss 452→430; alle (a)-Fälle byte-stabil.
- WIDERLEGT/ZURÜCKGEBAUT: DL-70-Normalisierung des Displace-Felds
  ((d−disp)/L, L≈1,8) in ALLEN Repräsentationen implementiert und vermessen:
  Mesher wird SCHLECHTER (813 → 1634 offene; Sizing verfeinert auf die
  kleineren |f|-Werte, Crowding verschärft). Für den RENDERER bleibt sie
  richtig (Marching übersteuert bis Faktor L) — Befund als Kommentar in
  bvh.rs, eigene Arbeitslinie nötig. BEHALTEN: fidget_bridge-Displace nutzte
  falsche Achsfaktoren (sin·sin·sin ohne 1.3/0.7 = ANDERE Fläche) — gefixt.
- NEUE AKTIVE GATES: `lichen_adv_coplanar_union_watertight`,
  `lichen_adv_graze_demo_alt_watertight` (Naht-Mechanik ist nicht auf den
  Abnahme-Ausschnitt überangepasst), `lichen_adv_thin_tube_watertight`
  (χ=0-Torus-Anker). WIP (ignored, präzise Diagnose):
  `lichen_adv_warp_bodies_wip` (kein B-Rep für Warp-Bodies → keine Ketten),
  `lichen_adv_tangent_kiss_wip` (Blattabstand 0 am Kontakt = prinzipiell
  sub-resolution).

## 2026-07-13 07:40 — Abnahme-Gate aktiv (Task #3)

- `gyroid_cutout_acceptance_four_metrics` als AKTIVER Test in api/lichen.rs:
  Manifold (open=0, nm=0, orient=0, deg=0, comp=1, χ=2 als Regressions-Anker),
  Hausdorff (m→s ≤ 0.05, s→m ≤ 0.15, p95 ≤ 0.02), Dreieckszahl-Band
  [7000, 23000], Aspect (p99 ≤ 4, minang ≥ 10°). Betreiber-Settings 1:1.
  Grün in release (12 s); Debug-Laufzeit wird vermessen.
- Iteration-4-Verifikation durch Orchestrator: open=0/χ=2 reproduziert,
  1078/1078 grün, Stitching-Review ok (Grad-2-Loops ≤5, valid_geom-Pflicht,
  Manifold-Schutz, Loops >5 bleiben ehrlich offen).

## 2026-07-13 09:50 — Härtungsphase abgeschlossen (v0.6.567)

- 10-Fälle-Matrix (dünn/Warp/degeneriert/streifend): 4× wasserdicht (a),
  3× kleine Defekte (b), 3× harte Grenze (c). Details REPORT.md.
- Vier Härtungs-Fixes gelandet (Phantom-Ketten-Filter chain_seg_ok,
  Regionen-Merge-Reparatur, Stitch-Loop-Walk + 5er-Fächer,
  fidget-Displace-Achsfaktoren); DL-70-Displace-Normalisierung implementiert,
  vermessen, ZURÜCKGEBAUT (Mesher 813→1634; Renderer-Arbeitslinie offen,
  Befund in bvh.rs).
- Drei neue aktive Gates (koplanar, graze-alt, thin-tube χ=0); 2 ignored-WIP
  mit Diagnose (Warp-Bodies, tangentiale Küsse).
- Fixture-Validierung fing zwei Fixture-Fehler des Härtungs-Agenten
  (Gyroid-Wandstärke 2ts/√3; Ort mit 0 % Material) — das Muster trägt.
- Verifikation Orchestrator: Lichen-Gates 37/37, Abnahme-Gate byte-identisch,
  kernel release 1082/1082, workspace release 1159/1159; voller
  Debug-Workspace-Lauf gestartet.

## 2026-07-13 10:20 — Abschluss-Verifikation

- Voller Debug-Workspace-Lauf: 0 failed (Kernel-Lib 1082/1082 in 712 s,
  alle übrigen Crates grün). Erfolgskriterium 2 vollständig erfüllt.
- WASM neu gebaut (just build-wasm) — der laufende Dev-Server zeigt den
  v0.6.567-Stand im Betreiber-Projekt 7d9701… (WASM gitignored, wie Policy).
- Session-DoD komplett: Abnahme-Gate aktiv+grün, Suiten grün, REPORT.md final.

## 2026-07-13 11:30 — Fortsetzung: Betreiber-Auftrag (Doku, Merge, offene Punkte)

- origin/main in lichen-research gemerged (86e93fb): Konflikte package.json
  (ours 0.6.567) + releaseNotes (Peer nutzte 0.6.544/545 parallel — gelöst
  als EIN Block pro Version mit beiden Entries, dokumentierte Lektion).
  Dabei ASCII-Quote-Breaker in eigener 567er-Note gefunden+gefixt (b6e8a13,
  dieselbe Falle wie Peers 537er). Post-Merge grün: 37 Gates, Abnahme-Gate,
  Web 1694/1694, tsc 0.
- RESEARCH (read-only Agent) Warp-Bodies: Twist{Box} ist NICHT space-filling
  → Oktree-Weave läuft, ABER unter Warp werden halfspaces+leaf_map geleert
  (edge_walker.rs:1731) → alle Nähte = zersplitterte Sample-Fits; zusätzlich
  nimmt Lichens face_surfs nur SurfaceGeometry::Analytic → keine Regionen,
  kein Wachstum. Design-Empfehlung:
  1. Twist/Bend: `WarpedHalfspace`-Field (Dual-Eval sdf_halfspace∘W⁻¹, W⁻¹
     existiert geschlossen in sdf_dual.rs:383) + SSI-Seeds aus dem
     UNGEWARPTEN inner-Baum, vorwärts durch W gemappt, trace_edge mit zwei
     WarpedHalfspace-Feldern; scope_supported_partial öffnen.
  2. Displace (Wert-Störung, nicht invertierbar): Feld-basierte
     Feature-Linien (Normalen-Sprung in der Punktwolke + Bisektion +
     bestehende build_chain/Weld-Bausteine).
  3. Beide teilen ein warp-bewusstes face_of (W⁻¹-Stufe zwischen placement
     und transform; für Displace ist argmin am selben Punkt korrekt).

## 2026-07-13 13:30 — Offene Punkte 4 + 1 gelandet (v0.6.568/569)

- v0.6.568 (3a078a0): Renderer-Displace-Lipschitz als march_relax-Schritt-
  dämpfung 1/L (eine Stelle, Muster REVOLVE_MARCH_RELAX; Design-Recherche:
  Feld-Normalisierung à la DL-70 schadet dem Mesher nur bei aktivem
  Sizing/lfs — Thread blieb unbestraft, weil sein Feld glatt ist und Sizing
  default-aus; die „Thread-vom-Lichen-ausgeschlossen"-These war FALSCH).
  Feld bitidentisch, Mesher beweisbar unberührt. Visuelle Abnahme offen.
- v0.6.569 (Warp-Paket, Implementierungs-Agent): Twist-Box 864→0 (χ=2!),
  Bend-Box auf Anhieb 0. Mechanik: DomainWarp{Twist,Bend} in sdf_dual
  (W geschlossen bzw. 1D-Newton, T = Eval-Formel), Field::WarpedAnalytic
  im DL-68-Tracer, scope_supported_partial öffnet Root-Warps (Seeds durch
  W gemappt), f64-Ecken-Politur NUR auf der Warp-Route (f32-refine ließ
  Bend-Witness durchs 2e-4-Aktivitätsband fallen — Forensik statt
  Band-Aufweichung), FaceSurf.warp macht face_of/own_dist/Einfärbung/
  Ketten-Normalen warp-bewusst (Normalen als Welt-FD — Zurückrotieren
  unterschlüge die Scherung). Neues aktives Gate warp_twist_and_bend.
  Verifiziert: Abnahme byte-identisch, 38 Gates, kernel 1088/1088.
- Scope-Grenzen (dokumentiert): Warps innerhalb Booleans, nicht-analytische
  Branches unter Warp, nicht-Identity-Placement (bestehende Kernel-
  Inkonsistenz), Displace (separates Paket: Feld-Feature-Linien).
- Doku-§ für das Warp-Paket in Lichen-Architecture.md steht noch aus.

## 2026-07-13 15:40 — Offener Punkt 2 (tangentiale Küsse): Sizing-Deckel WIDERLEGT

- Implementierungs-Agent fiel am Spend-Limit aus; Paket inline zu Ende
  vermessen. Mechanik (env-gated, Default AUS): Kuss-Deckel LICHEN_KISS_C —
  Gegenblatt-lfs-Beitrag ignorieren wenn lfs<c·floor ∧ antiparallel
  (reach_outer_counter liefert das Gegenblatt-dot) ∧ kettenlos
  (brep_dist>1.5); Eigen-Spacing = h ∧ α/κ (Sizing-Field ERSETZT die
  Wachstums-Klemme, local_radius wendet α/κ bei Some(sf) nicht mehr an —
  Krümmung muss also in den Deckel-Zweig).
- MESSUNG: Abnahme-Netz byte-identisch (Hash-Anker zz_acceptance_hash,
  auch bei c=1.0 — Bedingung (c) schützt Naht-Zonen). Punkt-Kuss
  33→22 (c=1.0, χ −17→−10), aber Laufzeit 74→191 s. Linien-Kuss
  430→**1558** (χ −480!): der r=h-Streifen neben ultradichter Umgebung
  ist die bekannte Dichtesprung-Defektklasse (I-16/§8j). → WIDERLEGT als
  Default; bleibt Mess-Werkzeug (Default 0.0).
- Fazit: Der Kuss braucht TOPOLOGIE-Behandlung (Sub-Resolution-Kontakt
  erkennen, Blätter als getrennte Komponenten meshen — Mikro-Feature-
  Kollaps-Familie §8k/§8l), keine Sizing-Kosmetik. WIP-Test-Doku
  aktualisiert; Werkzeuge bleiben: reach_outer_counter, LICHEN_KISS_DUMP,
  zz_acceptance_hash (Byte-Identitäts-Anker).
- Neutralität bei Default verifiziert: 38 Gates grün, thin_plate
  wert-identisch.

## 2026-07-13 — Displace-Paket: 662 → 0 (Regionen waren der Hebel, nicht Ketten)

- AUFTRAG: warp_displace_sphere (662 offene) via echte Ketten + Regionen,
  Bauplan analog Warp-Paket v0.6.569 (Field::DisplacedAnalytic, Seed-Weaver-
  Öffnung, displace-bewusste FaceSurf).
- BEFUND VOR ARBEITSBEGINN: die Matrix-Kugel hat 1 Face / 0 Kanten (BREP-
  Forensik) — für DIESEN Fall existieren gar keine Nähte. Die 662 offenen
  kamen komplett aus den REGIONEN: face_of/own_dist/On-Surface-Gates maßen
  gegen den UN-displacten Kugel-Träger (|disp| ≈ 0,3 mm als „Abstand"),
  Normalen ignorierten die Beulen (bis ~40° falsch → Krümmung/Sizing/Guards
  blind).
- SCHRITT 1 — FaceSurf.disp + carrier() (halfspace − disp, Welt-FD-Normalen;
  alle 12 Konsum-Stellen inkl. Orphan-/Streu-/Fallback-Pfad):
  **662 → 0**, χ=2, comp=1, H 0.0051/0.0040, p99 1.55, minang 22.6°.
  Punkte 5189 → 17401, Laufzeit 12 → ~85 s: EHRLICHE Krümmungs-Verfeinerung
  der Beulen (kein Crowding-Defekt; §8r-Verdacht „intraface, Regionen helfen
  nicht" WIDERLEGT — kein Sizing-Eingriff, I-21/I-24 unangetastet).
- SCHRITT 2 — Ketten für displaced Booleans (Bauplan-Teil 1+2):
  displaced_halfspace_value_grad (sdf_dual), Field::DisplacedAnalytic
  (edge_trace), root_displace (edge_walker), Seed-Weaver-Öffnung. Drei
  gemessene Stolpersteine:
  (a) both_branches_active attributiert Contributors im UN-displacten
      eps-Band → jeder Naht-Punkt „inactive"; Displace-Route prüft
      stattdessen On-Boundary |f_body| ≤ eps (Punkte liegen per
      Konstruktion auf beiden displacten Feldern).
  (b) Co-Komponenten-Dedup: un-displacte SSI-Seeds liegen 0,3 mm neben der
      displacten Polyline (near_tol 0,017) → ~60 Duplikat-Fragmente je
      Kante; Fix = project_to_edge-Projektion jedes Seeds vor dem Tracen.
  (c) MODELL-GRENZE (Geometrie, kein Bug): wo disp > 0 ist die Leaf-SDF
      außen EUKLIDISCH → runder Blend-Streifen statt scharfer Naht; die
      displacten Felder konkurrieren (√3−1)·disp AUSSERHALB des Bodys —
      Aktivitäts-Test verwirft korrekt. Displaced Booleans = piecewise
      scharfe Ketten (disp<0) + kettenlose Blend-Streifen.
- GATES NEU: lichen_adv_warp_displace_watertight (voller Assertions-Satz,
  ~85 s → #[ignore]d, manuell/CI via --ignored),
  hybrid_weaves_displaced_box_exact_topology_carved_regime (disp<0-Fixture
  x∈[8,16]/y∈[3,15]/z∈[−19,−11], sin-Fenster ohne Vorzeichenwechsel →
  exakte 8/12/6-Topologie, Kanten-Samples < 1e-6 auf beiden displacten
  Feldern), hybrid_displaced_box_mixed_sign_chain_samples_on_body (alle
  emittierten Samples auf dem realen Body). Scope-Test aktualisiert
  (Root-Displace im Partial-Scope, Twist-unter-Displace bleibt Oktree).
- REGRESSIONEN: Abnahme byte-identisch (zz_acceptance_hash
  6e620e39806d3db3 vor UND nach, gegen git-stash-Baseline gemessen);
  volle Matrix nach Schritt 1 wert-identisch in allen 10 übrigen Fällen
  (twist/bend 0/0, thin 7/0/0, tangent 430/33, coplanar/graze 0);
  38 Lichen-Gates grün; kernel release 1111/1111 grün (inkl. 2 neuer Seed-Weaver-Gates; Displace-Gate bleibt ignored/manuell).
- Doku: §8t in Lichen-Architecture.md, §8r Punkt 1 GESCHLOSSEN.

## 2026-07-13 20:30 — Displace-Ketten (v0.6.571) + Headful-Abnahme + Team-Koordination

- v0.6.571 (Merge c2acea7): warp_displace_sphere 662→0 (χ=2, H 0.005/0.004).
  ÜBERRASCHUNG: die 662 kamen komplett aus den REGIONEN (Kugel: 1 Face /
  0 Kanten) — FaceSurf maß Träger-Abstände gegen die UN-displacte Fläche;
  der §8r-Verdacht „intraface-Crowding, Regionen helfen nicht" hat sich
  NICHT bestätigt. FaceSurf.disp (carrier = halfspace − disp, 12 Stellen,
  Welt-FD-Normalen) + exakter Naht-Weave (DisplacedAnalytic, Seed-
  Projektion — ohne sie ~60 Duplikat-Fragmente/Kante —, On-Boundary-
  Aktivität statt Contributor-Test, f64-Politur). Modell-Grenze §8t:
  disp>0 ⇒ Leaf-SDF außen euklidisch ⇒ Blend-Streifen statt scharfer Naht.
  Damit ist die GESAMTE Warp-Familie geschlossen. Suite 1111/1111.
- Headful-WebGPU-Abnahme des Render-Fixes 0.6.568: PASS (0 Loch-Cluster
  Streifwinkel, Rim max 2 px, marchRelax 0.5548 exakt); dauerhafte Spec
  web/e2e/displace-march.spec.ts. Backlog-Fund: SDF-only-Commits triggern
  kein Repaint (Demand-Frameloop). Mess-Fallen im Spec-Header dokumentiert.
- KOORDINATION mit Peer `fielddraft` (arbeitet an Onshape + „tangentiale
  Nähte weben"): Er verschob seinen seed_weaver-Umbau, bis mein Displace-
  Stand auf main lag (jetzt frei, benachrichtigt), arbeitet solange
  web-only. Sein SSI-Befund (tangential = doppelte Nullstelle ohne
  Vorzeichenwechsel) bestätigt unsere Kuss-Diagnose; sein konstruktiver
  Naht-Ansatz (2D-Knick → Kette) wird auch unsere Kuss-Fälle mit Ketten
  versorgen. Zweimal Release-Note-Gates gerissen (ASCII-Quote erneut;
  vX.Y.Z-Muster im Text verboten) — beide sofort gefixt; LEKTION: Notes
  vor Commit IMMER tsc+vitest aus web/ laufen lassen, nicht verketten.

## 2026-07-13 — Sliver an streifenden Nähten: 5,2° → 11,8° (§8r Punkt 3 geschlossen)

- AUFTRAG: graze_sphere_plane minang ≥ 10° / asp p99 ≤ 4 ohne Verlust von
  Wasserdichtheit/Hausdorff; Kandidaten (1) Glätten+Reprojektion,
  (2) Kanten-Flips, (3) Kollaps.
- FLIPS ALLEIN WIDERLEGT (I-25): Lawson-Pass im globalen Walk voll
  implementiert und mit Ablehnungszählern vermessen — 0 von 462
  Sub-12°-Paaren (graze) bzw. 0 von 216 (twist) wären durch einen Flip
  besser geworden („schlechter"=alle): die Walk-Diagonale ist lokal
  optimal, die Alternative über drei fast-kollineare Kettenpunkte
  degeneriert. Code zurückgebaut.
- FORENSIK (LICHEN_SLIVER_STATS, Winkel-Histogramm + Worst-Dump): die
  Sliver sind KETTEN-FÄCHER — Basis = Ketten-Kante 0,1 mm
  (kurvatur-getrieben: curvAngleDeg 2° auf der Naht-Kurve r≈3,16),
  Apex = Flächenpunkt ~1 mm (Träger fast eben). Dichtesprung
  Kette↔Fläche ist intrinsisch (Kurven- ≠ Flächen-Krümmung) →
  Punktverteilungs-, kein Konnektivitätsproblem.
- FIX: `lichen_tri::sliver_repair` (Default-an, LICHEN_SLIVER=0,
  Schwelle LICHEN_SLIVER_DEG=12°), Post-Pass nach dem Walk in
  api/lichen.rs: (1) Rivara-Längste-Kante-Bisektion der Sliver
  (Newton-projizierter Mittelpunkt, Floor 2·Spacing,
  SEPARATIONS-GATE 0,3·l — ohne das Gate stall bei 6,5° durch
  Fast-Duplikat-Einfügungen 0,025 mm), (2) tangentiales Glätten mit
  STRIKTEM max-min-Winkel-Wächter (λ-Backtracking, region_ok=face_of,
  Sehnen-Sagitta-Check; Ketten/Ecken fix), (3) Lawson-Flips (nie über
  Ketten, Blatt-Restriktion, manifold-frei). χ/Orientierung/comp
  per Konstruktion erhalten; neue Punkte in all/point_faces nachgezogen.
- MESSREIHE graze: 5,2° (Baseline) → 6,5° (ohne Separations-Gate,
  37→18 s nach Guard-Reordering) → **11,8°/p99 3,09** (mit Gate, 12,5 s,
  H 0.0031/0.0026 UNVERÄNDERT, open 0, χ 2). Twist 9,0→12,0°
  (p99 3,31→3,17), Bend 11,6→12,5°, Coplanar 10,4→12,2° — alle übrigen
  Matrix-Fälle wert-identisch (thin 7/0/0, displace 0, tangent 430/33,
  graze_alt 12,4° unberührt: Schwelle 12° greift dort nicht).
- REGRESSIONEN: Abnahme-Gate grün und BYTE-IDENTISCH (zz_acceptance_hash
  6e620e39806d3db3 — Pass aktiviert sich nur unter 12°, Abnahme-minang
  14,9°); Lichen-Gates 39/39 (neu:
  `lichen_adv_graze_sphere_plane_watertight`, minang ≥ 10 / p99 ≤ 4);
  kernel release 1112/1112 (Workspace komplett grün).
- Doku: §8u in Lichen-Architecture.md, §8r Punkt 3 GESCHLOSSEN, I-25.

## 2026-07-13 23:00 — ALLE OFFENEN PUNKTE ABGESCHLOSSEN (v0.6.573, main 5bbed87)

- Sliver-Paket gelandet: sliver_repair (Rivara-Bisektion mit Separations-
  Gate + tangentiales Glätten mit max-min-Wächter + Lawson-Flips) —
  graze 5,2°→11,8°, twist 12,0°, bend 12,5°, coplanar 12,2°; Abnahme
  byte-identisch; I-25 (Flips als Erstmaßnahme widerlegt: Walk-Diagonale
  lokal optimal); §8u neu, §8r Punkt 3 geschlossen. 39 Gates, 1112/1112,
  Web 1717/1717.
- Damit sind ALLE vier offenen Punkte aus REPORT.md erledigt:
  1 Warp-Familie komplett wasserdicht (569/571), 2 Küsse ehrlich
  geschlossen (Topologie-Weg, I-24; Peer-Tangentialnähte werden Ketten
  liefern), 3 Sliver behoben (573), 4 Renderer-Displace gefixt+
  headful-abgenommen (568/571).
- Peer fielddraft hat parallel brep/tangential_seams.rs begonnen
  (konstruktiver Naht-Ansatz) — Koordination lief über zwei
  send_to_agent-Runden, kollisionsfrei sequenziert.
- Betreiber-Worktree-Änderungen (Backlog.md→Link-Ideen.md) auf Zuruf
  mit committet.

## 2026-07-13 — Defekt A + B (Export-Ticket faa42489): vier Wurzeln, „Befund 6" war es NICHT

- AUFTRAG: (A) `lfsFactor:0.1` + `sizingField:true` zerstört Polyeder (Würfel →
  acht geschlossene Klümpchen um die Ecken, alle sechs Flächen fehlen, χ=16);
  (B) U(p)-Überdeckungs-Invariante reißt auf Gyroid∩Box mit ε=0.
- ARBEITSHYPOTHESE DES AUFTRAGS (Wiederkehr von Befund 6: Medialachse läuft in
  jede scharfe Kante) **WIDERLEGT**: der Kanten-Guard in `reach_inner` arbeitet
  einwandfrei. Forensik: das Sizing-Field des Würfels ist völlig gesund
  (raw min 1,16 / p50 1,82 mm — kein Kollaps). Der Fehler lag hinter dem
  Sampling.
- WURZEL 1 (Defekt B): `sample_edge` liefert **16 doppelte Punkte** je
  Kettenende. Ein Null-Segment hat keine Richtung → `acos(0) = 90°` → der
  Knickwinkel-Akkumulator sah auf einer GERADEN Box-Kante bis zu sieben
  180°-Knicke (Summe 1800°) und emittierte einen Krümmungs-Punkt 0,097 mm hinter
  der Ecke. Genau dieses Mikro-Segment ist die U(p)-Lücke (reproduziert:
  `d=1.4679 ua=ub=0.1875`). Fix: Dedup mit AUFLÖSUNGS-Schwelle
  (`1e-3·min_spacing`) vor der Winkelmessung.
- WURZEL 2 (Zündung A): Raster-Rest `total mod h` = 0,089 mm auf der 20-mm-Kante
  (h=1,8101) → Grading-Kaskade (11 → 46 Ketten-Punkte) → `corner_gap` vergiftet
  alle Nachbarketten.
- WURZEL 3 (Wirkung A, der eigentliche Fix): `grow_surface` PLATZIERT mit
  `min(σ, 1.5·space)`, NIMMT aber gegen `r_eff = σ` an. Neben einer 0,089er-Kette
  landet jeder Kandidat bei 0,13 mm und fliegt als „redundant" (< 0,45·σ=0,81)
  wieder raus — das Wachstum steht STILL (gemessen: 578 Punkte, davon **0**
  Flächenpunkte). Fix: `r_eff = min(σ(cp), 1.5·space)` — beweisbar No-op auf
  feld-getriebenen Flächen (σ ist 0,3-Lipschitz), greift nur an Ketten.
- WURZEL 4: das Sizing-Field erfand bei `lfsFactor=0` ein eigenes ε=0,5 → andere
  Zieldichte als der grobe Lauf → Sprosse 0 der Export-Leiter riss auf
  (Platte+Bohrung 959 offene Kanten). Fix: `eps = lfs_factor`.
- VERWORFEN (I-26/I-27, je voll implementiert und vermessen): Gleichverteilung
  der Anker-Intervalle (Abnahme 0→4, Streif-Naht 0→209); Rest-Punkt streichen
  (koplanar 0→125, Streif 0→63); symmetrischer Gegenblatt-Guard im ÄUSSEREN
  Marsch (Streif-Naht 0→320 — der äußere Marsch ist an streifenden Nähten eine
  Verdichtungs-NOTWENDIGKEIT, keine -Quelle).
- FIXTURE-FALLE (die §8q-Lehre bestätigt sich zum vierten Mal): die erste
  `box_hole`-Fixture war eine QUER liegende Röhre (FieldDraft-Zylinder = y-Achse)
  mit 1-mm-Wänden — der Mesher verfeinerte zu Recht auf 95 000 Punkte. Der
  Marsch-Dump hat es gefangen, bevor daraus ein „Defekt" wurde.
- MESSREIHE: Würfel ε=0,1 578P/186Δ/χ16 → **1807P/3610Δ/χ2**, H(s→m) 10,4 → 0,000;
  Platte+Bohrung ε=0 11977P/959 offen → **2728P/0 offen/χ0**, ε=0,1 minang
  7,2° → 18,5°; 8-mm-Gyroid-Würfel ε=0 **PANIC** → 958Δ/χ2.
- REGRESSIONEN: Abnahme-Gate grün (open 0, χ 2, H(m→s) 0,0213, H(s→m) **0,0897**
  statt 0,0980, p95 0,0022, p99 1,85, minang 14,9° — alles gleich oder besser;
  Hash ändert sich wie freigegeben: 6e620e39806d3db3 → dab4fc245e12d0cf).
  42 Lichen-Gates grün (2 neue), Matrix: alle aktiven Gates dicht, Formgüte
  breit besser (minang 12° → 19–22° auf coplanar/graze/twist/bend); die beiden
  WIP-Fälle leicht schlechter (thin_gyroid_wall 7→11, tangent 430→448 / 33→39).
  Volle Suite release: kernel 1121, workspace 0 failed.
- EXPORT-LEITER: bleibt — aber aus einem ANDEREN Grund. Beide Sprossen sind jetzt
  korrekt, nur nicht gleich teuer (ε-Sampling fordert 0,5 mm auf einer 10-mm-Platte
  → Würfel 180 → 3610 Δ). ε=0 bleibt auf dem Gyroid-Ausschnitt offen. Die Leiter
  ist damit eine KOSTEN-, keine Korrektheitsfrage.
