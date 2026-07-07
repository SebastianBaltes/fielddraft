# Hybrid-Meshing: PASM-Partikel + B-Rep-Kanten (Plan-SSOT)

Ticket nwmi. Ziel: ein **kantenexaktes, manifoldes, wasserdichtes** Dreiecksnetz
aus (1) der SDF (glatte Flächen) und (2) den expliziten B-Rep-Kanten (scharfe
C1-Diskontinuitäten). Grundlage: *Particle-Based Anisotropic Surface Meshing*
(Zhong, Guo, Wang, Lévy, Sun, Liu, Mao — SIGGRAPH 2013,
`docs/Literatur/PASM_SIGGRAPH_2013.pdf`), im Folgenden **PASM**.

## 0. Kurzfassung des Papers (was wir übernehmen)

- **Energie statt Ad-hoc-Kräfte.** Partikelpaare tragen eine Gauß-Energie
  `E_ij = exp(−‖x̄_i−x̄_j‖² / 4σ²)` in einem (nie explizit konstruierten)
  „Embedding Space", in dem die Wunsch-Metrik isotrop ist. Mit der Metrik
  `M = QᵀQ` am Paar-Mittelpunkt wird daraus im Original-Raum
  `E_ij ≈ exp(−(x_i−x_j)ᵀ M_ij (x_i−x_j) / 4σ²)` und die Kraft
  `F̃_ij = Q_ij (x_i−x_j) / 2σ² · E_ij` (Paper Gl. 12/21). Wichtig: `Q_ij`
  (nicht `M_ij` — der ASPH-Fehler) und die Rück-Rotation `V_i` (Gl. 23).
- **Ein globales σ:** `σ = 0.3 · √(|Ω̄|/n)` mit `|Ω̄| = ∫√det M ds`
  (metrische Fläche), Kernel-Cutoff bei `5σ` (ANN-Nachbarschaft).
- **Optimierung:** L-BFGS auf der Summenenergie; Gradienten in die
  Tangentialebene projiziert (Gl. 27); Partikel nach jedem Schritt auf die
  Fläche reprojiziert. Konvergiert in ~100 Iterationen ohne
  Populations-Kontrolle (kein Insert/Delete nötig — Hauptvorteil gegenüber
  Bossen–Heckbert/Shimada-Bubbles).
- **Metrik:** `M = [v_min v_max n] · diag(s₁², s₂², 0) · [·]ᵀ` aus den
  Hauptkrümmungen (Gl. 30), bzw. isotrop krümmungs-gradiert
  `ρ(x) = (|κ_min|+|κ_max|)/2` (Fig. 9); alternativ das 6D-Embedding
  `φ(x) = [x, s·n(x)]` (Lévy–Bonneel, Abschn. 4.4) — Krümmungsadaption ohne
  explizite Metrik.
- **Vernetzung:** Restricted Anisotropic Voronoi Diagram (Du–Wang-Definition)
  über der finalen Punktmenge, Dual = Dreiecksnetz; Topologie-Kontrolle über
  Zusammenhangskomponenten (Yan et al. 2009).

**Was das Paper NICHT löst (unsere Lücke):** scharfe Kanten/Ecken. Alle
Testflächen sind glatt; Ränder werden nur „zurückprojiziert". An C1-Knicken
ist (a) die Krümmungsmetrik undefiniert, (b) die Tangential-Projektion
instabil (Normale springt), (c) das RVD-Dual legt Dreiecke ÜBER die Kante
(Chamfer). Genau dafür haben wir die B-Rep-Kanten.

## 1. Architektur-Entscheid: Skelett zuerst, Fläche danach

Der Hybrid behandelt das Modell als **geschichtete Dimension**:

    Ecken (0D, B-Rep-Vertices)   → FIX. Exakte Position aus dem Weave.
    Kanten (1D, B-Rep-Kurven)    → 1D-Partikelketten AUF der Kurve;
                                    Endpunkte = die fixen Ecken.
    Flächen (2D, SDF-Regionen)   → PASM-Partikel, begrenzt durch die Ketten.

Erst wird das **Kanten-Skelett** diskretisiert (Bogenlängen-Sampling der
exakten Kurven, dann 1D-Relaxation ENTLANG der Kurve mit derselben
Gauß-Energie — Kettenabstände folgen so demselben σ/Metrik-Regime wie die
Fläche). Dann relaxieren die Flächen-Partikel im Feld der (nun fixen)
Skelett-Partikel. Damit ist Kantenexaktheit **konstruktiv** garantiert statt
emergent erhofft: jede Netzkante auf einer Modellkante ist eine Sehne der
exakten Kurve, jede Modell-Ecke ist ein Netz-Vertex.

### Antwort auf Kernfrage 1 — Feature Constraints

Beides, in klarer Rollenteilung — aber anders als naiv gedacht:

- **Einrasten (Snapping) ist der Bootstrap, nicht der Betriebsmodus.** Die
  Skelett-Partikel werden nicht durch Relaxation „eingefangen", sondern
  direkt auf den Kurven ERZEUGT (wir haben die Kurven; Einfangen wäre ein
  Rückschritt in die Heuristik). Das vorhandene Feature-Snapping
  (mesher_points, Normalen-Proben) bleibt als Fallback für Bodies ohne
  B-Rep-Kanten (Partial-Weave-Lücken).
- **Barrieren für die Flächen-Partikel.** Ein Flächen-Partikel darf eine
  Kanten-Kette nicht überqueren, sonst wandern Samples zwischen zwei Faces
  und das RVD-Dual erzeugt Kanten-überspannende Dreiecke. Umsetzung billig
  und robust über die **Face-Zugehörigkeit statt Geometrie-Tests**: jedes
  Flächen-Partikel kennt seinen Branch/seine Region (Contributor-Attribution
  der SDF, existiert); nach jedem Relaxations-Schritt wird ein Partikel,
  dessen Attribution wechselt, auf seine letzte gültige Position
  zurückgesetzt (Halbschritt-Bisektion). Das ist derselbe Mechanismus wie
  die `both_branches_active`-Trims des Weavers — kein neuer
  Geometrie-Apparat.
- **Repulsion wirkt ÜBER die Kante hinweg trotzdem** (Skelett-Partikel
  stoßen Flächen-Partikel beidseitig ab): das erzeugt den sauberen
  „Offset-Streifen" beidseits der Kante, den ein gutes CAD-Netz an
  Features zeigt (strukturierte erste Dreiecksreihe).

### Antwort auf Kernfrage 2 — Metrik an Diskontinuitäten

Der Kern-Trick: **Die Krümmung wird nie AN der Kante ausgewertet, sondern
immer einseitig auf dem glatten Blatt.**

1. **Flächen-Partikel** leben je auf genau einem Face (Attribution). Die
   Metrik `M(x)` wird aus der Hesse-Matrix des zugehörigen GLATTEN Feldes
   berechnet — bei analytischen Trägern (Ebene/Zylinder/Kegel/Kugel/Torus/
   Revolve-Profil) in **geschlossener Form** (κ₁, κ₂ exakt, kein Sampling
   über den Knick), beim Gyroid via Dual-Hessian des Blatt-Feldes `f∓t`
   (glatt, da f≠0 auf dem Blatt). Die C1-Diskontinuität liegt IMMER auf der
   Kante — also außerhalb jedes Auswertepunkts.
2. **Skelett-Partikel** brauchen keine Flächen-Metrik: ihre 1D-Dichte folgt
   der KURVEN-Geometrie — Bogenlängen-Sampling mit Sehnen-Toleranz
   (Sagitta ≤ tol, identisch zur Adaptive-Trace-Toleranz des Weavers) und
   als Untergrenze der von beiden angrenzenden Faces geforderten Dichte
   (`h_edge = min(h_faceA, h_faceB, h_sagitta)`), damit die erste
   Dreiecksreihe nicht degeneriert.
3. **Clamping wie im Paper implizit:** `s₁,s₂` (Streckfaktoren) werden auf
   `[1, s_max]` begrenzt (Paper nutzt bis 100; wir starten konservativ
   `s_max = 8`) und das Metrikfeld Laplace-geglättet (Paper zitiert Alliez
   et al. für genau das) — Glättung PRO FACE, nie über Kanten hinweg.

Ergebnis: „κ = ∞ an der Kante" ist schlicht kein auswertbarer Zustand mehr —
die Kante ist Randbedingung (1D-Skelett), nicht Metrik-Quelle.

### Antwort auf Kernfrage 3 — Triangulation

**Per-Face Constrained Delaunay (CDT) im Parameterraum, mit dem Skelett als
Constraint-Polygon.** Begründung gegen die Alternativen:

- **PASM-RVD/RDT (global):** prinzipiell die Paper-Lösung, aber (a) das
  RVD-Clipping braucht ein Hintergrund-Mesh bzw. exakte Prädikate auf der
  Fläche — großer Apparat; (b) es garantiert NICHT, dass Modellkanten
  Netzkanten werden (genau unser Problem); (c) Topologie-Kontrolle
  („Komponenten müssen Discs sein") ist ein eigenes Fass. Als globaler
  Fallback später denkbar, nicht als Fundament.
- **Advancing Front (global auf der SDF):** startet zwar elegant an unseren
  exakten Kanten, aber Front-Kollisionsbehandlung auf gekrümmten impliziten
  Flächen ist der fehleranfälligste Teil der Literatur; ohne Parametrisierung
  schwer robust zu bekommen. Nur für nicht parametrisierbare Faces als
  Phase-3-Option.
- **Per-Face CDT (gewählt):** Unsere Faces sind per Konstruktion
  Branch-Regionen mit analytischem Träger. JEDER Träger außer dem Gyroid hat
  eine natürliche 2D-Parametrisierung (Ebene trivial; Zylinder/Kegel
  abrollbar; Kugel lokal stereographisch pro Region; Torus (θ,φ);
  Revolve-Profil (θ, s); Extrude-Wand (s, z)) — exakt die Karten, die
  `ssi.rs::ParamA` schon implementiert. Im Parameterraum: Skelett-Ketten der
  Face-Loops als **Constraint-Kanten**, Flächen-Partikel als Innenpunkte →
  `spade` (pure Rust, wasm-tauglich, CDT + Bulk-Load) trianguliert; die
  Anisotropie der Metrik wird durch eine metrik-gerechte Verzerrung der
  Parameterkoordinaten VOR der CDT angenähert (affine Squash pro Face reicht
  für moderate s_max; Paper-Fig. 8 zeigt dieselbe Squash-Idee als Q′=√M).
  Zurück im 3D sind die Loops identische Punktfolgen für beide
  angrenzenden Faces → Nähte passen exakt aufeinander → **wasserdicht per
  Konstruktion**, manifold solange jede Face-Region ein Disc-/Annulus-Loop
  ist (Loops liefert der Weave; Annulus = äußerer+innere Loops, CDT kann
  Löcher nativ).
- **Gyroid-Blätter** (parametrierlos, aber glatt und randlos bzw. nur von
  Rim-Loops berandet): Phase 3. Optionen: (a) lokale Tangential-CDT +
  Merge, (b) Advancing Front vom Rim-Skelett, (c) PASM-RVD nur für diese
  Faces. Bis dahin liefert der bestehende MDC/EMC-Pfad dort das Netz
  (hybrides Stitching am Rim-Loop, dessen Punkte ja fixiert sind).

## 2. Warum das trägt (Bewertung)

- PASM löst exakt unser Restproblem der Flächen-Verteilung (das
  Debug-Mesher-Experiment hat die Grenzen der Ad-hoc-Repulsion gezeigt:
  Stabilität nur mit Clamps, keine Krümmungsadaption, kein
  Konvergenzkriterium). Energie + L-BFGS + σ-Disziplin ersetzt drei
  Heuristik-Regler (Strength/Damping/Radius) durch EINEN Parameter (n bzw.
  Ziel-Kantenlänge).
- Die B-Rep-Kanten machen aus PASMs größter Schwäche (Features) einen
  Nicht-Punkt: Kanten sind Randbedingungen, keine Detektionsaufgabe.
- Wasserdichtigkeit entsteht strukturell (geteilte Loop-Ketten), nicht durch
  Toleranz-Schweißen.
- Risiken: (a) Partial-Weave-Bodies ohne vollständige Loops → Fallback auf
  Feature-Snap-Heuristik + MDC (heutiger Stand, kein Rückschritt); (b)
  Parametrisierungs-Verzerrung bei extremer Anisotropie → s_max-Clamp; (c)
  Gyroid-Faces erst Phase 3.

## 3. Phasenplan

- **P1 — Skelett & Constraints im Partikelsystem** *(diese Session)*:
  B-Rep-Kanten bogenlängen-sampeln (Ecken fix), als fixe Partikel in die
  Relaxation; Attribution-Barriere; Debug-Mesher-Schalter „B-Rep-Skelett";
  Export-Flag pro Punkt (Fläche/Kante/Ecke) fürs Overlay.
- **P2 — PASM-Energie**: Gauß-Kernel (σ=0.3√(|Ω̄|/n), 5σ-Cutoff via
  Grid-Hash), isotrop krümmungs-gradiert (ρ aus geschlossenen κ der Träger;
  Gyroid via Dual-Hessian), Tangential-L-BFGS (zunächst: Gradient-Descent
  mit Armijo reicht praktisch, L-BFGS als Folge-Feinschliff).
- **P3 — CDT-Vernetzung**: spade einbinden; Ebenen-Faces zuerst (Box-Deckel
  end-to-end wasserdicht), dann abrollbare Träger, dann Kugel/Torus/Profil;
  Naht-Kontrakt-Tests (jede Loop-Kette exakt einmal je Face-Seite).
- **P4 — Anisotropie & Gyroid**: volle M=QᵀQ-Kräfte (Gl. 21), Squash-CDT;
  Gyroid-Faces via Tangential-CDT oder Front; MDC-Stitching entfällt.
  - *P4a umgesetzt (v0.6.435)*: periodische Träger (Zylinder/Kegel/Kugel-
    Bänder) via `triangulate_periodic_face` — Unwrap + Windungszahl,
    Naht-Schnitt mit GETEILTEN Steiner-Punkt-Indizes in beiden Kopien
    (strukturelle Verschweißung), Polkappen (Windung 1) bewusst abgelehnt.
    Zwei Robustheits-Fundamente daraus: Dreiecks-Harvest kombinatorisch
    (Flood-Fill, Parität flippt an Constraint-Kanten — Zentroid-Tests
    rauschen an Naht-Slivern) und 0,25·h-Boundary-Clearance für Partikel
    (exakt-auf-Constraint splittet einseitig und öffnet die Kette).
    Zylinder end-to-end geschlossen/2-Manifold.
  - *P4b umgesetzt (v0.6.436)*: Revolve-Träger — v = Bogenlänge der
    Nächster-Punkt-Projektion auf die offset-korrigierte Profilkette
    (`ssi::profile_pieces` → (r,y)-Polyline). Punkte AUF der Fläche
    projizieren eindeutig; Faltungs-Guard (v-Sprung > ¼ Kettenlänge →
    Face bleibt unvermascht). Rohr-Meilenstein: Rechteck-Revolve ∩ Box
    end-to-end wasserdicht (Torus, Euler=0); Außen-/Innenwand desselben
    Branch trennen sich rein übers Chart, Deckel als Annuli.
  - *P4c umgesetzt (v0.6.437)*: Polkappen-Fächer — bei einer windenden
    Loop mit bekanntem Pol (Sphere/Cone/Revolve-Achsberührung) wird ein
    Pol-Ring aus Aux-Punkten knapp vor dem Pol erzeugt, Loop↔Ring läuft
    rekursiv durch den Band-Pfad, der Ring schließt per Dreiecks-Fächer
    zum einzelnen Pol-Vertex (nie zwei Pol-Ecken pro Dreieck). Ring-
    Auflösung aus der Welt-Bogenlänge. Kugelkappe ∩ Box: Euler=2.
  - *P4d offen*: Anisotropie (M=QᵀQ), Gyroid-Blätter (Tessellated-Faces),
    Zonen über den Pol (nicht-windende Loops, Pol im Inneren — derzeit
    unerkannt, hinterlässt ein Loch am Pol).
- **P5 — Export-Integration**: Ersetzt MT/MDC im Export-Dialog als
  Qualitätsstufe „CAD (kantenexakt)"; Survey-Harness
  (ticket_survey_all_methods) erweitert um Hausdorff/Winkel-Histogramm wie
  Paper Abschn. 4.5.

## 4. Offene Punkte

- Loop-Vollständigkeit im Partial-Weave (Gyroid-Bodies): P3 setzt
  geschlossene Face-Loops voraus; wo sie fehlen → Face fällt auf MDC-Patch
  zurück (Stitching am vorhandenen Teil-Skelett).
- Metrik-Glättung pro Face (Laplace über Partikel-Nachbarschaftsgraph) —
  Parameter noch zu kalibrieren.
- RVD/RDT als späterer globaler Qualitäts-Vergleichspfad (Paper-Metriken
  G_min/θ_min reproduzieren).
