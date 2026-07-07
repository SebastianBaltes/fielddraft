# Hybrid-Meshing: PASM-Partikel + B-Rep-Kanten (Plan-SSOT)

Ticket nwmi. Ziel: ein **kantenexaktes, manifoldes, wasserdichtes** Dreiecksnetz
aus (1) der SDF (glatte Flächen) und (2) den expliziten B-Rep-Kanten (scharfe
C1-Diskontinuitäten). Grundlage: *Particle-Based Anisotropic Surface Meshing*
(Zhong, Guo, Wang, Lévy, Sun, Liu, Mao — SIGGRAPH 2013,
`docs/Literatur/PASM_SIGGRAPH_2013.pdf`), im Folgenden **PASM**.

## Status & Wiedereinstieg (Stand 2026-07-07, Arbeit pausiert)

**Erreicht: P1–P4c, v0.6.433–437.** Vier Referenzkörper laufen end-to-end
durch die komplette Kette Skelett → PASM-Relaxation → per-Face-CDT und
liefern GESCHLOSSENE, 2-manifolde, kantenexakte Netze (jede Kante exakt 2×,
korrekte Euler-Charakteristik):

| Körper             | prüft                                                   | Test (`api/export.rs`)                                   |
| ------------------ | ------------------------------------------------------- | -------------------------------------------------------- |
| Box                | planare Faces, geteilte Ketten-Slots                    | `mesher_points_cdt_box_is_watertight_manifold` (Euler 2) |
| Zylinder           | periodisches Band, Naht-Verschweißung                   | `..._cylinder_...` (Euler 2)                             |
| Revolve-Rohr ∩ Box | Revolve-Chart, 2 Wände auf EINEM Branch, Annulus-Deckel | `..._revolve_tube_...` (Euler 0, Torus)                  |
| Kugelkappe ∩ Box   | Polkappen-Fächer                                        | `..._sphere_cap_...` (Euler 2)                           |

**Code-Landkarte:**

* `crates/kernel/src/mesher_cdt.rs` — CDT-Kern (`spade`):
  `triangulate_uv` (uv-generisch, kombinatorischer Parität-Harvest,
  0,25·h-Boundary-Clearance), `triangulate_planar_face` (Newell-Frame),
  `triangulate_periodic_face` (Unwrap + Windungszahl; Fälle: Patch /
  Polkappe via Pol-Ring+Fächer+Band-Rekursion / Band mit geteilten
  Naht-Steiner-Punkten). Unit-Tests im selben File.
* `crates/kernel/src/api/export.rs::mesher_points` — Pipeline-Verdrahtung:
  Skelett-Sammlung (`corner_slot`/`edge_chain`), Partikel-Attribution über
  Contributor-Sets, Face-Loop-Assembly, Chart-Bau pro Träger (Plane /
  Zylinder / Kegel / Kugel / RevolvedProfile inkl. Profil-Polyline +
  Bogenlängen-Projektion + Faltungs-Guard), `poles`-Liste, Aux-Punkt-Emission
  (Flag 2 hinter dem Skelett-Block).
* `crates/kernel/src/mesher_points.rs` — Partikel-Loop: 1/d² + PASM-Gauß-
  Kernel (σ\=r/5, Konvergenz-Abbruch), Skelett-Anker, Feature-Snap-Fallback.
* `crates/kernel/src/brep/ssi.rs` — `profile_pieces` jetzt `pub(crate)`
  (liefert die offset-korrigierte Profilkette fürs Revolve-Chart).
* UI: Debug ▸ Mesher, Checkboxen „B-Rep skeleton", „PASM kernel (Gaussian)",
  „CDT faces (planar + abrollbar)"; Overlay färbt Ecken rot, Ketten/Naht
  orange.

**Die drei tragenden Robustheits-Entscheide** (bei Weiterarbeit nicht
aufweichen): (1) Wasserdichtigkeit ist STRUKTURELL — Nachbar-Faces teilen
dieselben globalen Ketten-/Steiner-Indizes, nie Toleranz-Schweißen.
(2) Dreiecks-Klassifikation KOMBINATORISCH (Flood-Fill, Parität flippt an
Constraint-Kanten) — Zentroid-Tests rauschen an fast-kollinearen Nähten.
(3) Partikel \< 0,25·h Chart-Abstand zur Boundary verwerfen — exakt-auf-
Constraint splittet einseitig und öffnet die geteilte Kette.

**Arbeitsstand im Working Tree (UNCOMMITTET — zuerst abschließen, §5.1):**

v0.6.461 ist vorbereitet, aber weder committet noch im Browser verifiziert.
Es liegen ZWEI getrennte Arbeiten gemischt im Working Tree:

1. **Mesher-PASM-Dialog (dieses Ticket nwmi):** Kernel-Option
   `MesherPointOptions::target_edge_length` (JSON `targetEdgeLength`,
   `mesher_points.rs`) — bei Wert > 0 leitet `api/export.rs::mesher_points`
   daraus ab: `max_depth` (Seed-Blattzelle knapp unter h, Clamp 1..10) und
   `interaction_radius = h` (\= 5σ-Gauß-Cutoff, σ \= h/5). Neuer Kernel-Test
   `mesher_points_pasm_target_edge_length_derives_resolution` (Box nur über
   h → geschlossen, Euler 2). Web: neuer Dialog `web/src/MesherPasmDialog.tsx`
   (**UNTRACKED**) + `web/src/styles/components/mesher-pasm-dialog.css`
   (**UNTRACKED**), Menüpunkt Debug ▸ „Mesher PASM…" (`SettingsMenu.tsx`),
   Persistenz + Projekt-Debug-Roundtrip (`projectDebug.ts`, Tests vorhanden),
   `mesherDebugStore.targetEdgeLength`. Beide Mesher-Dialoge publizieren in
   DENSELBEN `mesherDebugStore` → gegenseitiger Ausschluss ist implementiert
   und getestet. Version 0.6.461 + Release-Note sind bereits eingetragen.
2. **Export-Preview-Fix (Server-Ticket cdb25e1e — ANDERES Thema, eigener
   Commit!):** `web/src/exportPreviewMesh.tsx` — der WebGPURenderer
   ignoriert `material.wireframe` → echtes `THREE.WireframeGeometry` +
   `lineSegments`; und der Opacity-Slider wirkte als An/Aus, weil das
   Umschalten von `transparent` die Pipeline neu baute → Material jetzt
   imperativ, `transparent` konstant true, nur der depthWrite-Flip setzt
   `needsUpdate`. Braucht noch: eigenen Versions-Bump 0.6.462, eigene
   Release-Note, Ticket-Kommentar + state\=resolved.

**Empfohlene Reihenfolge zum Abschluss** (Detail-Anleitungen in §5; das
Paper muss dafür NICHT erneut gelesen werden — alle benötigten Gleichungen
stehen in §5.4, das PDF liegt unter `docs/Literatur/PASM_SIGGRAPH_2013.pdf`):

1. §5.1 Arbeitsstand testen, sichten, in ZWEI Commits committen + pushen.
2. §5.2 „Zone über den Pol" schließen (kleiner Korrektheits-Fix, P4d-1).
3. §5.3 P5 Export-Integration „CAD (kantenexakt)" — liefert Nutzerwert mit
   der fertigen isotropen Pipeline. Anisotropie ändert später nur die
   Punktverteilung, nicht den Export-Kontrakt, darum Export ZUERST.
4. §5.4 P4d-2 Anisotropie (Paper-Kern, größter Qualitätshebel, schwerste
   Mathematik — deshalb nach dem Export).
5. §5.5 P4d-3 Gyroid-Blätter (darf offen bleiben; Fallback greift).

**Unabhängig offen:** Ticket pzvt (Hover-Kamera-Desync, Analyse im
Ticket-Ordner `tickets/open/20260707-001324-pzvt`; Fix-Idee „pick what you
see": SDF-Pass publiziert Render-Kamera-Snapshot, Picks projizieren durch
diesen).

## 0. Kurzfassung des Papers (was wir übernehmen)

* **Energie statt Ad-hoc-Kräfte.** Partikelpaare tragen eine Gauß-Energie
  `E_ij = exp(−‖x̄_i−x̄_j‖² / 4σ²)` in einem (nie explizit konstruierten)
  „Embedding Space", in dem die Wunsch-Metrik isotrop ist. Mit der Metrik
  `M = QᵀQ` am Paar-Mittelpunkt wird daraus im Original-Raum
  `E_ij ≈ exp(−(x_i−x_j)ᵀ M_ij (x_i−x_j) / 4σ²)` und die Kraft
  `F̃_ij = Q_ij (x_i−x_j) / 2σ² · E_ij` (Paper Gl. 12/21). Wichtig: `Q_ij`
  (nicht `M_ij` — der ASPH-Fehler) und die Rück-Rotation `V_i` (Gl. 23).
* **Ein globales σ:** `σ = 0.3 · √(|Ω̄|/n)` mit `|Ω̄| = ∫√det M ds`
  (metrische Fläche), Kernel-Cutoff bei `5σ` (ANN-Nachbarschaft).
* **Optimierung:** L-BFGS auf der Summenenergie; Gradienten in die
  Tangentialebene projiziert (Gl. 27); Partikel nach jedem Schritt auf die
  Fläche reprojiziert. Konvergiert in \~100 Iterationen ohne
  Populations-Kontrolle (kein Insert/Delete nötig — Hauptvorteil gegenüber
  Bossen–Heckbert/Shimada-Bubbles).
* **Metrik:** `M = [v_min v_max n] · diag(s₁², s₂², 0) · [·]ᵀ` aus den
  Hauptkrümmungen (Gl. 30), bzw. isotrop krümmungs-gradiert
  `ρ(x) = (|κ_min|+|κ_max|)/2` (Fig. 9); alternativ das 6D-Embedding
  `φ(x) = [x, s·n(x)]` (Lévy–Bonneel, Abschn. 4.4) — Krümmungsadaption ohne
  explizite Metrik.
* **Vernetzung:** Restricted Anisotropic Voronoi Diagram (Du–Wang-Definition)
  über der finalen Punktmenge, Dual \= Dreiecksnetz; Topologie-Kontrolle über
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
Endpunkte \= die fixen Ecken.
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

* **Einrasten (Snapping) ist der Bootstrap, nicht der Betriebsmodus.** Die
  Skelett-Partikel werden nicht durch Relaxation „eingefangen", sondern
  direkt auf den Kurven ERZEUGT (wir haben die Kurven; Einfangen wäre ein
  Rückschritt in die Heuristik). Das vorhandene Feature-Snapping
  (mesher\_points, Normalen-Proben) bleibt als Fallback für Bodies ohne
  B-Rep-Kanten (Partial-Weave-Lücken).
* **Barrieren für die Flächen-Partikel.** Ein Flächen-Partikel darf eine
  Kanten-Kette nicht überqueren, sonst wandern Samples zwischen zwei Faces
  und das RVD-Dual erzeugt Kanten-überspannende Dreiecke. Umsetzung billig
  und robust über die **Face-Zugehörigkeit statt Geometrie-Tests**: jedes
  Flächen-Partikel kennt seinen Branch/seine Region (Contributor-Attribution
  der SDF, existiert); nach jedem Relaxations-Schritt wird ein Partikel,
  dessen Attribution wechselt, auf seine letzte gültige Position
  zurückgesetzt (Halbschritt-Bisektion). Das ist derselbe Mechanismus wie
  die `both_branches_active`-Trims des Weavers — kein neuer
  Geometrie-Apparat.
* **Repulsion wirkt ÜBER die Kante hinweg trotzdem** (Skelett-Partikel
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

Ergebnis: „κ \= ∞ an der Kante" ist schlicht kein auswertbarer Zustand mehr —
die Kante ist Randbedingung (1D-Skelett), nicht Metrik-Quelle.

### Antwort auf Kernfrage 3 — Triangulation

**Per-Face Constrained Delaunay (CDT) im Parameterraum, mit dem Skelett als
Constraint-Polygon.** Begründung gegen die Alternativen:

* **PASM-RVD/RDT (global):** prinzipiell die Paper-Lösung, aber (a) das
  RVD-Clipping braucht ein Hintergrund-Mesh bzw. exakte Prädikate auf der
  Fläche — großer Apparat; (b) es garantiert NICHT, dass Modellkanten
  Netzkanten werden (genau unser Problem); (c) Topologie-Kontrolle
  („Komponenten müssen Discs sein") ist ein eigenes Fass. Als globaler
  Fallback später denkbar, nicht als Fundament.
* **Advancing Front (global auf der SDF):** startet zwar elegant an unseren
  exakten Kanten, aber Front-Kollisionsbehandlung auf gekrümmten impliziten
  Flächen ist der fehleranfälligste Teil der Literatur; ohne Parametrisierung
  schwer robust zu bekommen. Nur für nicht parametrisierbare Faces als
  Phase-3-Option.
* **Per-Face CDT (gewählt):** Unsere Faces sind per Konstruktion
  Branch-Regionen mit analytischem Träger. JEDER Träger außer dem Gyroid hat
  eine natürliche 2D-Parametrisierung (Ebene trivial; Zylinder/Kegel
  abrollbar; Kugel lokal stereographisch pro Region; Torus (θ,φ);
  Revolve-Profil (θ, s); Extrude-Wand (s, z)) — exakt die Karten, die
  `ssi.rs::ParamA` schon implementiert. Im Parameterraum: Skelett-Ketten der
  Face-Loops als **Constraint-Kanten**, Flächen-Partikel als Innenpunkte →
  `spade` (pure Rust, wasm-tauglich, CDT + Bulk-Load) trianguliert; die
  Anisotropie der Metrik wird durch eine metrik-gerechte Verzerrung der
  Parameterkoordinaten VOR der CDT angenähert (affine Squash pro Face reicht
  für moderate s\_max; Paper-Fig. 8 zeigt dieselbe Squash-Idee als Q′\=√M).
  Zurück im 3D sind die Loops identische Punktfolgen für beide
  angrenzenden Faces → Nähte passen exakt aufeinander → **wasserdicht per
  Konstruktion**, manifold solange jede Face-Region ein Disc-/Annulus-Loop
  ist (Loops liefert der Weave; Annulus \= äußerer+innere Loops, CDT kann
  Löcher nativ).
* **Gyroid-Blätter** (parametrierlos, aber glatt und randlos bzw. nur von
  Rim-Loops berandet): Phase 3. Optionen: (a) lokale Tangential-CDT +
  Merge, (b) Advancing Front vom Rim-Skelett, (c) PASM-RVD nur für diese
  Faces. Bis dahin liefert der bestehende MDC/EMC-Pfad dort das Netz
  (hybrides Stitching am Rim-Loop, dessen Punkte ja fixiert sind).

## 2. Warum das trägt (Bewertung)

* PASM löst exakt unser Restproblem der Flächen-Verteilung (das
  Debug-Mesher-Experiment hat die Grenzen der Ad-hoc-Repulsion gezeigt:
  Stabilität nur mit Clamps, keine Krümmungsadaption, kein
  Konvergenzkriterium). Energie + L-BFGS + σ-Disziplin ersetzt drei
  Heuristik-Regler (Strength/Damping/Radius) durch EINEN Parameter (n bzw.
  Ziel-Kantenlänge).
* Die B-Rep-Kanten machen aus PASMs größter Schwäche (Features) einen
  Nicht-Punkt: Kanten sind Randbedingungen, keine Detektionsaufgabe.
* Wasserdichtigkeit entsteht strukturell (geteilte Loop-Ketten), nicht durch
  Toleranz-Schweißen.
* Risiken: (a) Partial-Weave-Bodies ohne vollständige Loops → Fallback auf
  Feature-Snap-Heuristik + MDC (heutiger Stand, kein Rückschritt); (b)
  Parametrisierungs-Verzerrung bei extremer Anisotropie → s\_max-Clamp; (c)
  Gyroid-Faces erst Phase 3.

## 3. Phasenplan

* **P1 — Skelett & Constraints im Partikelsystem** *(diese Session)*:
  B-Rep-Kanten bogenlängen-sampeln (Ecken fix), als fixe Partikel in die
  Relaxation; Attribution-Barriere; Debug-Mesher-Schalter „B-Rep-Skelett";
  Export-Flag pro Punkt (Fläche/Kante/Ecke) fürs Overlay.
* **P2 — PASM-Energie**: Gauß-Kernel (σ\=0.3√(|Ω̄|/n), 5σ-Cutoff via
  Grid-Hash), isotrop krümmungs-gradiert (ρ aus geschlossenen κ der Träger;
  Gyroid via Dual-Hessian), Tangential-L-BFGS (zunächst: Gradient-Descent
  mit Armijo reicht praktisch, L-BFGS als Folge-Feinschliff).
* **P3 — CDT-Vernetzung**: spade einbinden; Ebenen-Faces zuerst (Box-Deckel
  end-to-end wasserdicht), dann abrollbare Träger, dann Kugel/Torus/Profil;
  Naht-Kontrakt-Tests (jede Loop-Kette exakt einmal je Face-Seite).
* **P4 — Anisotropie & Gyroid**: volle M\=QᵀQ-Kräfte (Gl. 21), Squash-CDT;
  Gyroid-Faces via Tangential-CDT oder Front; MDC-Stitching entfällt.
  * *P4a umgesetzt (v0.6.435)*: periodische Träger (Zylinder/Kegel/Kugel-
    Bänder) via `triangulate_periodic_face` — Unwrap + Windungszahl,
    Naht-Schnitt mit GETEILTEN Steiner-Punkt-Indizes in beiden Kopien
    (strukturelle Verschweißung), Polkappen (Windung 1) bewusst abgelehnt.
    Zwei Robustheits-Fundamente daraus: Dreiecks-Harvest kombinatorisch
    (Flood-Fill, Parität flippt an Constraint-Kanten — Zentroid-Tests
    rauschen an Naht-Slivern) und 0,25·h-Boundary-Clearance für Partikel
    (exakt-auf-Constraint splittet einseitig und öffnet die Kette).
    Zylinder end-to-end geschlossen/2-Manifold.
  * *P4b umgesetzt (v0.6.436)*: Revolve-Träger — v \= Bogenlänge der
    Nächster-Punkt-Projektion auf die offset-korrigierte Profilkette
    (`ssi::profile_pieces` → (r,y)-Polyline). Punkte AUF der Fläche
    projizieren eindeutig; Faltungs-Guard (v-Sprung > ¼ Kettenlänge →
    Face bleibt unvermascht). Rohr-Meilenstein: Rechteck-Revolve ∩ Box
    end-to-end wasserdicht (Torus, Euler\=0); Außen-/Innenwand desselben
    Branch trennen sich rein übers Chart, Deckel als Annuli.
  * *P4c umgesetzt (v0.6.437)*: Polkappen-Fächer — bei einer windenden
    Loop mit bekanntem Pol (Sphere/Cone/Revolve-Achsberührung) wird ein
    Pol-Ring aus Aux-Punkten knapp vor dem Pol erzeugt, Loop↔Ring läuft
    rekursiv durch den Band-Pfad, der Ring schließt per Dreiecks-Fächer
    zum einzelnen Pol-Vertex (nie zwei Pol-Ecken pro Dreieck). Ring-
    Auflösung aus der Welt-Bogenlänge. Kugelkappe ∩ Box: Euler\=2.
  * *P4d offen*: Anisotropie (M\=QᵀQ), Gyroid-Blätter (Tessellated-Faces),
    Zonen über den Pol (nicht-windende Loops, Pol im Inneren — derzeit
    unerkannt, hinterlässt ein Loch am Pol).
* **P5 — Export-Integration**: Ersetzt MT/MDC im Export-Dialog als
  Qualitätsstufe „CAD (kantenexakt)"; Survey-Harness
  (ticket\_survey\_all\_methods) erweitert um Hausdorff/Winkel-Histogramm wie
  Paper Abschn. 4.5.

## 4. Offene Punkte

* Loop-Vollständigkeit im Partial-Weave (Gyroid-Bodies): P3 setzt
  geschlossene Face-Loops voraus; wo sie fehlen → Face fällt auf MDC-Patch
  zurück (Stitching am vorhandenen Teil-Skelett).
* Metrik-Glättung pro Face (Laplace über Partikel-Nachbarschaftsgraph) —
  Parameter noch zu kalibrieren.
* RVD/RDT als späterer globaler Qualitäts-Vergleichspfad (Paper-Metriken
  G\_min/θ\_min reproduzieren).

## 5. Abschluss-Anleitung für den Nachfolge-Agenten

Diese Anleitung ist so geschrieben, dass sie ohne Sessionkontext ausführbar
ist. Durchgehend gilt: nach jedem abgeschlossenen Schritt committen UND
pushen; pro Commit Versions-Bump in `web/package.json` + Eintrag OBEN in
`web/src/releaseNotes.ts` (typografische „…"-Quotes, NIE ASCII-`"` — ein
ASCII-Quote bricht den JS-String und damit den Build; der commit-msg-Hook
lehnt `feat|fix|perf … (vX.Y.Z)` ohne passenden Release-Note-Eintrag ab).

### 5.1 Zuerst: Arbeitsstand verifizieren & committen

1. `cargo test --workspace` — grün sein müssen insbesondere die
   `mesher_points_cdt_*`-Tests (Box/Zylinder/Revolve-Rohr/Kugelkappe) und
   der neue `mesher_points_pasm_target_edge_length_derives_resolution`.
2. `cd web && npm test` (Node 22!).
3. Sichtprüfung im Browser: `just dev`. GOTCHA: nach Kernel-Änderungen
   IMMER `just build-wasm` — ein laufender Vite-Server serviert sonst
   STALE WASM; Kernel-Version im Browser via `window.__fdKernel.version()`.
   Debug ▸ „Mesher PASM…" auf den vier Referenzkörpern (Box / Zylinder /
   Revolve-Rohr∩Box / Kugelkappe∩Box, per UI schnell nachgebaut): Ecken
   rot, Ketten orange, Drahtgitter geschlossen, Punkte-/Dreiecks-Readout
   plausibel, h halbieren ⇒ ≈ 4× Dreiecke. WebGPU rendert erst nach einem
   Fenster-Resize; „Compiling shader…" ist KEIN Freeze.
4. **Commit 1** (nwmi, v0.6.461): alles AUSSER `exportPreviewMesh.tsx` —
   inkl. der beiden untracked Dateien (`MesherPasmDialog.tsx`,
   `mesher-pasm-dialog.css`). Subject z.B. `feat(mesher): Debug ▸ Mesher
   PASM… — Ein-Parameter-Dialog (targetEdgeLength) (v0.6.461)`.
5. **Commit 2** (cdb25e1e, v0.6.462): `exportPreviewMesh.tsx` + Bump auf
   0.6.462 + eigene Release-Note (fix, scope export: Wireframe in
   WebGPU-Renderern sichtbar, Opacity-Slider stufenlos). Danach das
   Server-Ticket cdb25e1e auflösen: Kommentar in `ticket_comments`
   (MariaDB-Container `fielddraft-db-1`; `author_type='dev'`, `internal=0`,
   `po_reviewed=1`, `author_id=NULL`, `id=REPLACE(UUID(),'-','')`,
   Commit-Link `https://github.com/SebastianBaltes/fielddraft/commit/<hash>`)
   und `state='resolved'`.

### 5.2 P4d-1 — „Zone über den Pol" schließen

*Symptom:* Liegt ein Pol des Trägers (Kugel / Kegel / Revolve mit
Achsberührung) IM Inneren eines Faces, dessen Loops NICHT um die Periode
winden (Beispiel: Kugel, von einer seitlich versetzten Box geschnitten),
bleibt am Pol ein Loch — der P4c-Polkappen-Pfad springt nur bei windenden
Loops an.

*Erkennung* (in `export.rs`, dort wo `poles` befüllt und die Faces den
Chart-Fällen zugewiesen werden): Ein Pol gehört zum Face, wenn (a) der
Pol-Punkt der Face-Region attribuiert ist — dieselben Contributor-/
Aktivitäts-Checks wie bei der Partikel-Attribution, KEINE
Toleranz-Geometrie — und (b) keine Loop des Faces Netto-Windung trägt.
Plausibilisierung: u-Spannweite der abgewickelten Loop ≈ volle Periode.

*Umsetzung mit vorhandener P4c-Maschinerie:* Pol-Ring aus Aux-Punkten knapp
vor dem Pol einfügen (Ring-Auflösung aus der Welt-Bogenlänge, existiert).
Der Ring IST eine windende Loop → das Face zerfällt in (1) die Zone
zwischen Original-Loop und Ring — das ist der bekannte Band-Fall mit
geteilten Naht-Steiner-Punkten — und (2) die Polkappe Ring → Fächer →
Pol-Vertex (nie zwei Pol-Ecken pro Dreieck). Keine neue Topologie-Logik
erfinden.

*DoD:* neuer Kernel-Test „Kugel ∩ seitlich versetzte Box" in
`api/export.rs` nach dem Muster der vier bestehenden: jede Kante exakt 2×,
Euler \= 2. Danach Sichtprüfung + eigener Commit.

### 5.3 P5 — Export-Integration „CAD (kantenexakt)"

Ziel: das PASM-Netz als Export-Qualitätsstufe neben den bestehenden
Verfahren; Fallback bleibt MDC, wenn das Netz nicht geschlossen zustande
kommt.

1. **Kernel-API:** `mesher_points` liefert das Netz schon heute (Rückgabe:
   `out[0]` \= Punktzahl N, `out[1]` \= Dreieckszahl T, dann N Punkte à 9
   Floats — exaktes Layout in `export.rs` nachlesen, die Tests nutzen
   Stride 9 —, dann T·3 Indizes). Für den Export einen schlanken Pfad
   ergänzen (z.B. `exportMeshCad(targetEdgeLength)` als eigener
   `wasm_bindgen`-Einstieg), der NUR Positionen + Indizes zurückgibt, ohne
   Debug-Overlay-Ballast.
2. **Geschlossenheits-Gate im Kernel:** nach dem Bau `edge_use` zählen
   (Muster aus den Tests). Irgendeine Kante ≠ 2× oder unvermaschte Faces
   (Faltungs-Guard, fehlende Loops, Gyroid) ⇒ Ergebnis als „nicht
   geschlossen" markieren; der Export fällt dann auf MDC zurück und der
   Dialog zeigt eine Warnung (Muster: bestehende thin\_cells-/
   Selbstschnitt-Warnungen im Export-Dialog).
3. **UI:** `ExportDialog.tsx` — neue Qualitätsstufe „CAD (kantenexakt)" mit
   genau EINEM Parameter h (Ziel-Kantenlänge, `NumberInput` — nie
   `<input type=number>`), Default 2 mm. Preview über den bestehenden
   `exportPreviewStore`-Pfad (der Wireframe-Fix aus cdb25e1e zeigt das
   Netz dann in allen Renderern korrekt). STL-/3MF-Writer existieren —
   nur die Mesh-Quelle wechselt.
4. **Survey-Harness:** `ticket_survey_all_methods` um die Paper-§4.5-
   Metriken erweitern: Hausdorff-Schätzung (Dreiecke sampeln → max |SDF|),
   Winkel-Histogramm, Dreiecksqualität `G = 2√3·S/(p·h_max)`
   (Frey–Borouchaki: S Fläche, p Halbumfang, h\_max längste Kante),
   `G_min`, `G_avg`, `θ_min`, `θ_avg`, `%<30°`. Vergleichslauf MT vs. MDC
   vs. CAD auf den vier Referenzkörpern + Vase, Zahlen in die
   Commit-Message.

*DoD:* Export eines Box- und eines Revolve-Projekts als STL aus der App,
Geschlossenheits-Gate greift nachweislich (Gyroid-Body → MDC-Fallback +
Warnung); Survey-Zahlen dokumentiert; Commit + Push + Release-Note.

### 5.4 P4d-2 — Anisotropie (Paper §3.2 exakt umsetzen)

Der schwierigste Rest. Alle Gleichungsnummern \= PASM-Paper. Die zwei
dokumentierten IRRWEGE zuerst — beide sehen plausibel aus und degradieren
messbar (Paper Fig. 4): (1) `M_ij` statt `Q_ij` in der Kraft (der
ASPH-Fehler, Gl. 24); (2) die Jacobian-Pseudo-Inverse (Gl. 25). Es gilt
ausschließlich Gl. 21.

1. **Metrik pro Partikel** (in `export.rs` pro Träger, geschlossen — nie
   über die Kante sampeln): Hauptkrümmungen κ₁, κ₂ mit Richtungen v₁, v₂:
   Ebene 0, 0; Zylinder 1/r längs Umfang, 0 längs Achse; Kegel
   cos(Halbwinkel)/r längs Umfang (r \= Achsabstand), 0 längs Erzeugender;
   Kugel 1/r, 1/r; Revolve: Profilrichtung \= Krümmung der
   (r,y)-Profilkette (Winkeländerung ÷ Bogenlänge, über 2–3 Segmente
   geglättet), Umfangsrichtung \= n\_r/r (Meusnier; n\_r \=
   Radialkomponente der Flächennormale, r \= Achsabstand — deckt Zylinder/
   Kegel/Kugel als Spezialfälle ab).
2. **Streckfaktoren aus der Sag-Toleranz:** Richtungs-Ziellänge
   `h_k = √(8·ε/|κ_k|)` (Sehnen-Sagitta ≤ ε), geclampt auf `[h/s_max, h]`
   mit `s_max = 8`; ε \= dieselbe Sag-Toleranz wie beim Skelett-Sampling.
   Metrik `M = [v₁ v₂ n]·diag((h/h₁)², (h/h₂)², 0)·[·]ᵀ` (Gl. 30 mit
   `s_k = h/h_k`); `Q = √M = [v₁ v₂ n]·diag(h/h₁, h/h₂, 0)·[·]ᵀ`.
   NIE über eine SVD von M gehen (bei isotroper Metrik sind die
   Eigenvektoren mehrdeutig, Paper §5.1) — die Wurzel immer direkt im
   Krümmungs-Frame bilden.
3. **Kraft (Gl. 21):** für ein Nachbarpaar i, j auf DEMSELBEN Face:
   Paar-Mittelpunkt auf den Träger projizieren, dort M/Q auswerten;
   `F̃_ij = Q_ij·(x_i−x_j)/(2σ²) · exp(−(x_i−x_j)ᵀ·M_ij·(x_i−x_j)/(4σ²))`.
   Summieren (Gl. 22), in die Tangentialebene projizieren (`F − (F·n)·n`,
   Gl. 27), Schritt, Reprojektion auf die Fläche (Fit-Mechanik existiert),
   Attribution-Barriere unverändert lassen.
4. **σ und Nachbarschaft:** global `σ = 0.3·√(|Ω̄|/n)` mit
   `|Ω̄| ≈ Σ_i √det(M_i)·A_i` (2×2-Tangentialteil; A\_i \= Flächenanteil
   des Seeds). Nachbarsuche bleibt das Euklid-Grid, aber Radius auf
   `5σ·s_max` erweitern und die Kandidaten metrisch prunen (Paper §4.1,
   letzter Absatz). Der heutige isotrope Stand (σ \= h/5, Cutoff h) bleibt
   als Spezialfall `M = I` erhalten.
5. **Glättung:** Metrikfeld pro Face mit 1–2 Laplace-Schritten über den
   Partikel-Nachbarschaftsgraphen glätten, NIE über Face-Grenzen.
6. **Squash-CDT** (`mesher_cdt.rs`): pro Face mittlere Chart-Metrik
   `M̄_c = mean(Jᵀ·M·J)` (J \= Chart-Jacobian, 2×2), `A = √M̄_c`; ALLE
   Chart-Koordinaten (Ketten, Naht-Steiner, Innenpunkte) vor der CDT mit A
   transformieren — 3D-Positionen und Index-Kontrakte bleiben unberührt,
   nur die Triangulierungs-Kombinatorik ändert sich. Affin pro Face reicht
   für `s_max ≤ 8` (Paper Fig. 8: dieselbe Idee als Q′ \= √M).
7. **Skelett-Dichte nachziehen:** Kettenabstand
   `h_edge = min(h_faceA, h_faceB, h_sagitta)` lokal entlang der Kurve —
   die erste Dreiecksreihe neben der Kante darf nicht gröber sein, als die
   angrenzende Fläche verlangt.

*DoD:* langgestreckter Zylinder oder Vase: deutlich weniger Dreiecke als
isotrop bei gleicher Sag-Toleranz, gestreckte Dreiecke in der flachen
Richtung; Struktur-Invarianten (jede Kante 2×, Euler) unverändert grün;
Winkel-Histogramm NACH der Q-Transformation eng (Survey-Metriken aus §5.3
wiederverwenden).

### 5.5 P4d-3 — Gyroid-Blätter (optional, zuletzt)

Tessellated-Faces haben kein analytisches Chart. Optionen unverändert:
(a) lokale Tangential-CDT + Merge, (b) Advancing Front vom Rim-Skelett,
(c) PASM-RVD nur für diese Faces; Blatt-Metrik via Dual-Hessian von
`f∓t`. Solange offen: das Geschlossenheits-Gate aus §5.3 sorgt dafür, dass
solche Bodies sauber auf MDC zurückfallen — kein stiller Qualitätsverlust.
Das Gyroid ist dabei nur der erste Vertreter des allgemeinen Falls
„Fläche ohne Chart" — das Gesamtkonzept dafür steht in §6.

### 5.6 Betriebs-Gotchas (Kurzreferenz)

* **Stale WASM:** ein laufender Vite-Server rebuildet die Kernel-WASM
  nicht — nach jeder Kernel-Änderung `just build-wasm` + Hard-Reload.
* **Node 22** für alle Web-Tests/-Builds.
* **Release-Notes:** typografische „…"-Quotes, Eintrag oben, der Hook
  erzwingt es.
* **NumberInput** für jedes Zahlenfeld, nie `<input type=number>`.
* **Docs:** keine Code-Fences für Plain-Text, Diagramme als Mermaid.
* **Auto-Commit + Push** nach jeder getesteten Änderung, Version je Commit.
* **WebGPU** rendert erst nach einem Fenster-Resize; Canvas-/Hover-Tests
  brauchen einen SICHTBAREN Tab (Hintergrund-Tab friert rAF ein).
* **Die drei Robustheits-Entscheide** (oben im Status-Abschnitt) nie
  aufweichen: strukturelle Wasserdichtigkeit (geteilte Indizes, nie
  Toleranz-Schweißen), kombinatorischer Parität-Harvest, 0,25·h-Clearance.

## 6. Allgemeiner Fall: Flächen ohne B-Rep-Chart (Custom SDF, Blends, TPMS)

Die per-Face-CDT setzt einen analytischen Träger mit 2D-Parametrisierung
voraus. Es gibt und wird Flächen geben, die das nicht haben:

* **heute schon:** Gyroid/TPMS-Blätter (Tessellated-Faces, §5.5);
* **absehbar:** Custom-SDF-Features (User-Formeln/-Programme), glatte
  Blends/Fillets (Smooth-Ops ohne C1-Kante), Warp/Bend-Deformationen
  (zerstören das Chart des Trägers, nicht aber die Fläche).

Wichtig dabei: **„kein Chart" heißt NICHT „keine B-Rep-Kanten".** Der
SSI-Tracer arbeitet auf SDF-Feldern (so wurden die Gyroid-Kanten gefunden) —
Schnittkanten zwischen einer Custom-SDF-Fläche und anderen Flächen sind
weiterhin numerisch tracebar. Was fehlt, ist nur die Parametrisierung der
Fläche selbst. Nur ein Body ganz ohne Weave (reiner glatter Custom-SDF ohne
Boolean-Kanten) hat gar kein Skelett — der läuft wie heute über MDC/EMC
bzw. PASM + Feature-Snap-Fallback, kein Rückschritt.

**Tragende Invariante für den Misch-Fall:** Die Grenze zwischen einer
Chart-Region und einer SDF-only-Region ist selbst eine B-Rep-Kanten-Loop
(der „Rim"), und deren Kettenpunkte sind geteilte globale Skelett-Slots.
Ein Part kann also einen B-Rep-Teil und einen SDF-only-Teil haben und
trotzdem strukturell wasserdicht + manifold werden — beide Seiten müssen
nur EXAKT dieselben Rim-Vertex-Indizes als Rand verwenden. Wasserdichtigkeit
bleibt Konstruktion, nie Toleranz-Schweißen.

**Fallback-Leiter pro Face (Qualität absteigend, Naht-Kontrakt identisch):**

* **F1 — Chart-CDT** (heutiger Pfad): analytischer Träger + geschlossene
  Loops → kantenexakt, beste Qualität.
* **F2 — PASM ohne Chart:** Die PARTIKEL-Verteilung ist bereits chart-frei
  (sie braucht nur SDF-Gradient, Reprojektion und Attribution — keine
  Parametrisierung). Die Punktqualität ist also NICHT degradiert; nur die
  Vernetzung braucht einen Ersatz: lokale Tangential-CDT + Merge,
  Advancing Front vom Rim-Skelett, oder Restricted Delaunay über der
  Punktmenge (§5.5-Optionen, gelten allgemein).
* **F3 — MDC-Patch mit Reißverschluss:** Octree-Rasterung (MDC/EMC,
  existiert) NUR über den Zellen, die der Region attribuiert sind; den
  Patch-Rand mit ~0,25·h Clearance VOR dem Rim stoppen; den Ring-Spalt
  zwischen Patch-Randpolylinie und Rim-Kette als Streifen triangulieren
  (Band-Maschinerie-Idee aus P4a/P4c: zwei Polylinien, ein Zipper), wobei
  die Rim-Seite die geteilten Skelett-Indizes trägt. Ergebnis: Innenfläche
  in MDC-Qualität (per Definition nicht schlechter als Standard-DC/MC),
  Naht exakt und strukturell dicht. Octree-Blattgröße ≈ h wählen, damit
  die Zipper-Dreiecke nicht degenerieren.

**Gate bleibt der Wächter:** Solange F2/F3 nicht implementiert sind, sorgt
das Geschlossenheits-Gate aus §5.3 dafür, dass jeder Body mit
SDF-only-Faces KOMPLETT auf MDC zurückfällt — funktional korrekt, nur ohne
Kantenexaktheit. F3 ist danach der empfohlene erste Ausbauschritt (nutzt
nur Vorhandenes: MDC, Attribution, Band-Triangulierung); F2 ist die
Qualitätsstufe darüber. Voraussetzung beider: geschlossene Rim-Loops
(Partial-Weave-Lücken ⇒ weiterhin Voll-Fallback) und konsistente
Orientierung Patch↔Zipper↔Nachbarfaces.