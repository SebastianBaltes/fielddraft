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
