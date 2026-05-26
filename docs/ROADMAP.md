# FieldDraft — Roadmap

> **Diese Datei.** Die geplanten K‑Slices (Kernel‑Phase), M‑Meilensteine (Backend‑Phase) und P‑Slices (Phase 3). Jeder Slice ist ein kleiner, testbarer Vertical Slice mit grünem `cargo test` am Ende und nachvollziehbarem Commit‑Verlauf. Der Rest der Dokumentation ist aufgeteilt in `README.md` (Vision), `docs/ARCHITECTURE.md` (§4 + §11), `docs/LITERATURE.md` (§16), `STATUS.md` (Ist‑Zustand) und `DEPLOY.md` (Self‑Hosting). Cross‑Refs auf „§4.x", „§16.x" zeigen auf ARCHITECTURE.md bzw. LITERATURE.md.

Die Roadmap ist darauf ausgelegt, dass der Pair‑Programming‑Agent die Umsetzung in kleinen, testbaren Vertical Slices vorantreiben kann. Jeder Meilenstein endet mit grünen Tests, einem lauffähigen Artefakt und einem nachvollziehbaren Commit‑Verlauf. Keine „Big Bang"‑Phasen.

## Release‑Tiers

Dieses Dokument beschreibt die FieldDraft‑**Vision**, nicht die erste shippable Version. Damit die Trennung zwischen „was zuerst rausgeht" und „was FieldDraft am Ende ausmacht" nicht in der Slice‑Liste untergeht, ordnet jeder K‑Slice genau einem von fünf **Release‑Tiers** zu. Die Tier‑Grenzen sind die harten Shipping‑Linien; innerhalb eines Tiers dürfen Slices umgeordnet werden, über Tier‑Grenzen hinweg nicht.

- **T0 — Demonstrator / Architektur‑Validierung.** K1–K3, K3.5, K4, K4.5, K5a, K5b, K6a, K6a.1. „Läuft", aber nicht user‑facing. Ziel: die SDF‑Architektur und die Feature‑as‑Function‑Pipeline end‑to‑end zum Laufen bringen, inklusive Solver‑Integration und generischem Dialog‑Generator. **Status: abgeschlossen bis auf K3.5** (neuer Slice, Planungsdetails siehe unten). Ein externer User kann das Tool an dieser Stelle nicht sinnvoll bedienen — es fehlen die Zeichen‑Werkzeuge und das Tree‑UI.
- **T1 — Shippable Alpha („Usable CAD").** K6b, K7, K8, K9, K10. **Cut‑Linie: der User kann einen Prototypen‑Gehäuse‑Entwurf mit verrundeten Kanten, Bohrungen, Patterns und Variablen aufbauen und als druckbares STL exportieren.** Ohne Fillets (K10) ist das Ergebnis für den Maker‑Einsatz nicht nutzbar — ein Gehäuse mit scharfen Kanten ist kein Gehäuse. Deshalb endet T1 explizit erst bei K10, nicht bei K9. Alles, was den SDF‑Hauptakku lädt (K11) oder Spezial‑Features sind (K12/K13), bleibt bewusst draußen. T1 ist der Punkt, an dem FieldDraft *ein* CAD ist — nicht *das* CAD.
- **T2 — Differentiated v1 („Reason to Exist").** K11. Hier lädt FieldDraft seinen Hauptakku auf: Gyroid/TPMS‑Infill, Procedural Noise/Displacement, Domain Deformation, Morph. Das sind die Features, die Onshape, Fusion, SolidWorks nicht haben, weil ihre B‑Rep‑Kernel sie nicht tragen — und der einzige Grund, FieldDraft über etablierte Tools zu wählen. Vor T2 ist FieldDraft technisch interessant, aber nicht differenziert. T2 ist die erste Version, die eine eigenständige Positionierung rechtfertigt.
- **T3 — Feature‑Complete Vision.** K12 (Analytisches Gewinde), K13 (Mesh‑to‑SDF für Sweep und Import), K14 (Drag'n'Drop), K15 (Input Map Presets), dann Phase 2 (Backend, Accounts, Assemblies) und Phase 3 (Kinematik, Physik, Code‑First‑Export). Mehrjahresperspektive.
- **T∞ — Forschung / experimentell.** K‑Exp1 (Skelett‑First‑Renderpfad) und weitere Parallel‑Slices. Kein Liefer‑Tier, sondern ein Ort, an dem Ideen reifen dürfen, ohne die Hauptlinie zu blockieren.

**Realitätsbezogener Zeithorizont.** Bei einem Entwickler plus Pair‑Programming‑Agent ist T1 als *mehrmonatige* Arbeit realistisch, T2 als *weitere mehrmonatige* Iteration, T3 in der Größenordnung von **1–2 Jahren**. Das Dokument ist damit keine „Road to MVP in N Wochen"‑Roadmap, sondern die Gesamtvision eines Produkts, das über mehrere Shipping‑Linien hinweg wächst.

**Was nicht im Tier‑Modell verschoben werden darf.** K10 (Fillets) bleibt in T1 — ohne Fillets kein usable CAD. K11 (SDF‑Superkräfte) bleibt in T2 und wird *nicht* nach T3 verschoben, um T1 schneller zu erreichen — ohne K11 gibt es keinen Grund für FieldDraft, existiert zu haben. Diese zwei Entscheidungen sind explizit getroffen und nicht mehr offen.

## Erledigte Slices

Der Ist‑Stand aller Slices steht in **`STATUS.md`** (tabellarische Übersicht mit `done`/`partial`/`open`‑Status pro Slice). Commit‑Level‑Details zu jedem erledigten Schritt stehen im `git log` (jeder Slice endet mit einem Commit mit Slice‑ID im Subject). Der frühere Execution‑Diary‑Abschnitt „§10.1 Erledigt" wurde aus diesem Dokument entfernt, weil er redundant zu STATUS.md und git log war.

**Kurzfassung des Ist‑Stands (Stand 2026‑04‑13):**
- **T0 komplett** bis auf K3.5 (neuer Slice, noch nicht umgesetzt).
- **T1 (Shippable Alpha) nahezu komplett** — K6b, K7, K8 inkl. K8.13, K9 teilweise, **K10 komplett** (Click‑to‑Fillet, Edge Detection/Rendering).
- **T2 (K11) komplett.**
- **T3** weit fortgeschritten: K12 done, K13.0–K13.3 done, K13 Manifold Repair offen, K14 teilweise.
- **Backend M11–M13** weitgehend fertig (Auth, Projekt‑CRUD, Sync, Galerie, Visibility, Quotas); offen: Passwort‑Reset, Performance‑Pass, Thumbnails.

Die noch offenen Slices haben unten volle Planungstexte. Die erledigten Slices sind durch ihre Implementierung im Code und durch die Commits dokumentiert.

---

## Kernel‑Phase (K1–K14): Hybrider SDF/Provenance/B‑Rep‑Kernel

Die Planungstexte für **alle** K‑Slices sind hier als Referenz erhalten — auch für erledigte Slices. Sie erklären das *Warum* jedes Slices und sind der Planungskontext, auf den zukünftige Slices (inkl. K3.5) aufbauen. Für den aktuellen Umsetzungsstand siehe `STATUS.md`.

### T0 · K1 — Kernel‑Skelett und statisches Mesh‑Rendering *(done)*
- CSG‑AST in Rust (enum mit Box/Sphere/Cylinder als Leafs, Union/Difference/Intersection/Transform als Inner Nodes).
- Punkt‑Auswertung `f(p) → distance` ohne Provenance, ohne BVH (naiv O(N) — bewusst, um die Render‑Pipeline isoliert verifizieren zu können).
- Naives Dual Contouring auf einem uniformen Voxel‑Grid, Mesh‑Brücke nach Three.js via Zero‑Copy‑Bridge.
- Test: hartkodierte CSG‑Szene (Quader mit zwei Bohrungen) wird als Mesh gerendert; Snapshot‑Vergleich gegen Goldfile.

### T0 · K2 — Provenance, Picking, B‑Rep‑Proxy *(done)*
- Punkt‑Auswertung erweitert auf `(distance, contributors[])`: jeder Leaf‑Knoten hat eine `node_id`, `min`/`max`/`difference` propagieren die ID des dominanten Operanden bzw. beide IDs bei Gleichstand innerhalb ε.
- Picking‑Pfad: Mausklick → Kamera‑Strahl → Sphere‑Trace (vorerst ohne BVH) → Trefferpunkt → Contributor‑Auswertung.
- B‑Rep‑Proxy‑Konstruktion aus dem ohnehin gemeshten Volumen: Face (1 Contributor), Edge (2 Contributors), Vertex (≥3 Contributors), Clusterung benachbarter Edge‑Punkte zu Polylinien.
- Test: Klick auf emergente Schnittkante einer Union(Quader, Zylinder) liefert die zwei korrekten Leaf‑IDs und das richtige Edge‑Element des Proxys.

### T0 · K3 — BVH, Multi‑Body, asynchrones Meshing *(done)*
- 3D‑AABB pro CSG‑Knoten; BVH‑Konstruktion bei jeder Tree‑Mutation, BVH‑beschleunigte Punkt‑Auswertung sowohl für Mesher als auch für Picking.
- Multi‑Body: jeder eigenständige Körper bekommt einen eigenen CSG‑Graphen, eigenen BVH und eigenen Mesh‑Cache.
- Meshing wandert in einen Web Worker; Body‑Updates streamen über einen Channel ins Frontend, das die `BufferGeometry` dieses Bodies atomar austauscht.
- Active‑Body vs. Context‑Body‑LOD: aktive Bodies werden bei Parameter‑Änderung sofort neu gemesht, Context‑Bodies bleiben statisch im VRAM.

### T0 · K3.5 — Provenance‑gesteuerte Mesh‑Optimierung *(neu, offen)*

Dieser Slice ist Infrastruktur, kein User‑Feature: Er baut im Meshing‑Worker einen deterministischen Post‑DC‑Pass, der die Dreieckszahl auf analytisch bekannten Flächen radikal reduziert und das Mikro‑Rauschen des QEF‑Solvers eliminiert. Logisch sitzt er zwischen K3 (asynchrones Meshing, abgeschlossen) und den Feature‑Slices K6b–K10 — er hebt die Qualität aller nachfolgenden Slices, ohne in die UI oder den Frontend‑Code zu greifen. Abhängigkeit: K2 (Provenance) und K3 (Worker‑Infrastruktur). Eingabe: SDF + CSG‑Baum + Provenance. Ausgabe: Vertex/Index‑Buffer — derselbe Kontrakt wie Standard‑DC, keine Frontend‑Änderungen. Siehe ARCHITECTURE.md §4.1 (Mechanismus), README.md §2 (Motivation Potato‑Meshing), ARCHITECTURE.md §11 Risiko 1 (Meshing‑Qualität).

- **Phase A — Zero‑Noise‑Projektion (Stufe 1, Grundlagen):** Im DC‑Pass werden Vertices mit genau einem dominanten Contributor per Provenance identifiziert und per Newton‑Iteration exakt auf die analytische Null‑Fläche `f_A = 0` des Contributors projiziert. Der QEF‑Solver wird für diese Vertices übersprungen — O(1) statt SVD, numerisch perfekt. Vertices in smin‑/Fillet‑Übergangszonen (Ghost Topology) fallen weiterhin auf Standard‑QEF zurück, weil sie keinen eindeutigen Contributor haben.
- **Phase B — Topologische Simplifizierung planarer Flächen (Stufe 1):** Dreiecke werden per Provenance den B‑Rep‑Faces zugeordnet (Face‑Gruppierung). Für jede planare Fläche wird im UV‑Parameterraum ein 2D‑Gitter aufgespannt, pro Gitterpunkt per SDF‑Auswertung klassifiziert (Contributor dominant ja/nein); Marching Squares extrahiert die Nulllinie als geschlossene Randkontur inklusive innerer Löcher (Bohrungen). Die groben Konturpunkte werden per Newton auf die exakte Schnittkurve `f_A = f_B = 0` zurückprojiziert; die inneren Vertices der Fläche werden verworfen; das Polygon wird per Earcut (mit Loch‑Unterstützung, O(R log R)) neu vernetzt. Rand‑Vertices zu benachbarten Flächen/Fillets bleiben unangetastet, damit das neue Mesh exakt und lückenlos an die komplexen Kanten und Fillets anschließt (keine T‑Junctions, keine Risse).
- **Phase C — Abwickelbare Flächen (Stufe 2):** Erkennung abwickelbarer Flächen (Zylinder‑Mäntel, Kegel, Extrude‑Profile) über den Contributor‑Typ. Isometrische Abwicklung in 2D, Marching‑Squares‑Randextraktion, Earcut, Rücktransformation in den 3D‑Raum. Nahtlinien‑Handling für geschlossene 360°‑Mäntel.
- **Interaktives Verhalten:** Die Simplifizierung läuft nur bei committed Parameter‑Änderungen. Während eines Live‑Drags bleibt das Standard‑DC‑Mesh aktiv.
- **Zeitbudget:** Der Pass geht auf das bestehende Meshing‑Zeitbudget aus K3. Asymptotisch gleich schnell oder schneller als Standard‑DC und refinanziert sich durch die drastisch reduzierte Dreieckszahl im VRAM‑Upload und der WebGPU‑Rasterisierung.
- **Explizit nicht im Scope:** Heightfield‑Meshing für Gewinde (Stufe 3, ROAM/Restricted Quadtree) wird zusammen mit **K12** implementiert. Adaptiver Octree für doppelt gekrümmte Flächen (Stufe 4, Schaefer‑Paper) bleibt eigener späterer Slice. Worker‑Pool ist Plan‑B für ARCHITECTURE.md §11 Risiko 14.
- **Testsuite (rein deterministisch, `cargo test`, keine UI):** Fixture‑Suite kanonischer Referenzkörper (Box, Zylinder mit Bohrung, Box mit Fillet, Union zweier Zylinder). Vier Metriken: (a) **Hausdorff‑Distanz** (`|SDF(v)| < ε` für analytische Flächen nach Newton‑Projektion), (b) **Dreieckszahl pro Face‑Typ** (planare Box‑Seite von ~5.000 auf einstellig), (c) **Manifold‑Check** (jede Kante exakt zwei Dreiecke), (d) **Dreieck‑Aspektverhältnis** (Sliver‑Detection).
- **Abnahme:** Alle vier Metriken grün auf der Referenz‑Suite; Standard‑DC‑Pfad bleibt als Fallback im Worker erhalten und wird während Live‑Drags aktiviert; K1/K2/K3/K4‑Goldfiles laufen unverändert grün.

### T1 · K3.6 — Dirty‑Tracking‑Fixes & Affine‑Transform‑Fastpath

- **Hash‑Bug‑Fix:** 7 geometrie‑beeinflussende Feature‑Felder (blend_radius, chamfer_size, twist_rate, bend_rate, displace_depth, displace_freq, shell_thickness) fehlten im Payload‑Hash → Änderungen an diesen Feldern triggerten kein Remesh.
- **Split‑Hash:** `hash_body_payload` aufgeteilt in `geometry_hash` (Primitiv + Modifier) und `transform_hash` (Translate/Rotation/Scale). Ermöglicht Erkennung reiner Transformations‑Änderungen.
- **Affine‑Fastpath:** Wenn `geometry_hash` unverändert und nur eine uniforme Translation erkannt wird, setzt der Kernel ein `transform_delta` statt ein volles Remesh auszulösen. Frontend wendet das Delta als GPU‑Matrix an (`mesh.position.set`).
- **WASM‑API:** `bodyHasTransformDelta(i)`, `bodyTransformDelta(i)` für die Frontend‑Brücke.

### T2 · K3.7 — Adaptives Dual Contouring + QEF *(offen)*

- Neues Modul `mdc.rs`: Adaptive Octree‑Konstruktion mit rekursiver Unterteilung bei Sign‑Changes, Pruning homogener Zellen.
- **QEF‑Solver:** 3×3 SVD‑basiert, akkumuliert Hermite‑Daten (Schnittpunkte + SDF‑Gradienten), optimale Vertex‑Platzierung statt Centroid.
- **Quad‑Emission:** ProcessCell/ProcessFace/ProcessEdge‑Traversierung nach Schaefer/Ju/Warren, T‑Junction‑Handling an Octree‑Level‑Grenzen.
- **Integration:** Dispatch auf `mdc_core` für Resolutions > 64, dichter DC‑Pfad bleibt für kleine Grids. Phase A/B/C Post‑Processing unverändert.
- **Ziel:** Scharfe Kanten bei Boxen, adaptive Auflösung (fein an Kanten, grob auf Flächen), weniger Dreiecke insgesamt.

### T3 · K3.8 — SDF‑CWF Export‑Mesher *(offen, langfristig)*

- Neues Modul `cwf.rs`: Implementierung des SDF‑CWF‑Verfahrens (Liu et al., CAD 2025) für Slicer‑/STEP‑Export‑Qualität.
- **Pipeline:** MC‑Seed → CVT + QEM‑Energie‑Optimierung → Power‑Diagram‑Triangulierung. L‑BFGS‑Solver, Feature‑Weights für Kantenerhaltung, abklingendes λ_CVT.
- **Qualität:** 0 Non‑Manifold‑Vertices, 0 Open‑Boundaries, optimale Dreiecksqualität.
- **Integration:** Export‑Button im Frontend triggert CWF‑Mesher im Worker. Nicht für Live‑Display gedacht.

### T0 · K4 — 2D‑Kernel, 2D‑BVH, Extrude, Revolve *(done)*
- 2D‑SDF‑AST: Linie, Kreisbogen, Polygon, Offset‑Region; Composing über `min`/`max` analog zu 3D.
- 2D‑BVH als Pflicht‑Pfad für **jede** Skizze, ohne adaptive Sonderfälle (kein Baking, kein Modulo).
- Splines im SDF‑Kernel als Polylinien‑Approximation (Default 0,05 mm Sehnenfehler).
- Extrude‑ und Revolve‑Knoten heben das 2D‑Feld in den 3D‑Raum.

### T0 · K4.5 — Feature‑as‑Function Refactor *(done)*

Strukturumbau ohne Verhaltens‑Break: Das bisherige K1/K2/K4‑Verhalten bleibt bit‑identisch, die Art und Weise, wie Features und Entities definiert und adressiert werden, wird auf §4.6 (Feature‑as‑Function) umgestellt. Hierarchische `FeatureId`, `FeatureKind`‑Trait + Registry, `FeatureSchema`‑Derive‑Proc‑Macro, `Vec<FeatureCall>` als kanonische JSON‑Wire‑Form, Regeneration Worker mit nativer JS‑Execution, Dual‑API‑Shells, Query‑System‑Minimum. Referenz‑Implementierung: `extrude` in beiden Pfaden.

### T0 · K5a — PlaneGCS‑Integration und Basis‑Constraints *(done)*
- Integration des PlaneGCS‑Solvers in den Kernel‑Workspace als eigenes Crate `crates/solver` (siehe ARCHITECTURE.md §4.4).
- FFI‑Layer: schmaler `extern "C"`‑Satz für `add_point`, `add_line`, `add_circle`, `add_arc`, `add_constraint`, `solve`, `get_dof`, `get_conflicting`, `get_redundant`, `get_partially_redundant`, `get_malformed`.
- Pflicht‑Constraints: Coincident, Parallel, Horizontal/Vertical, Perpendicular, Dimension, Tangent, Concentric, Equal, Midpoint, Symmetric.

### T0 · K5b — Sketch‑Adapter‑Schicht (UX‑Hebel) *(done)*
- **InferenceEngine, DragController, DoF‑Preflight, Diagnostics‑Pipeline** als vier Sub‑Komponenten der Adapter‑Schicht (ARCHITECTURE.md §4.4). Die Onshape‑artige Sketch‑UX lebt in dieser Schicht, nicht im Solver.

### T0 · K6a — Generischer FeatureDialog, Outer JS‑API, Widget‑Kit *(done)*

Frontend‑Seite der Feature‑as‑Function‑Architektur aus K4.5. Outer JS‑API als dünner `postMessage`‑Wrapper; generischer FeatureDialog als einziger React‑Component, der ein JSON‑Schema konsumiert; Widget‑Kit (`LengthInput`, `AngleInput`, `QueryPicker`, `EnumSelect`, `BoolCheckbox`, `TabGroup`, `ListInput`).

### T0 · K6a.1 — Onshape‑Paritäts‑Widgets im generischen FeatureDialog *(done)*

Schema‑Vokabular um Enum, UI‑Hints, Gruppen und `visible_when`‑Ausdrücke erweitert. Widget‑Kit: `EnumWidget`, `FlipButton`, `SliderWidget`, `GroupHeader` für kollabierbare Gruppen.

### T1 · K6b — Sketch‑Editor‑UI auf der neuen Infrastruktur *(weitgehend done, Härtung offen)*

- lies zum Beginn dieses Meilensteins `onshape-ux.md`
- Sketch‑Ebene via Klick auf Face: Picking‑Pfad aus K2 + Auto‑Lock an das Face des B‑Rep‑Proxys. Der Auto‑Lock wird als Query ausgedrückt, nicht als direkte Koordinate.
- Werkzeuge: Line, Rectangle, Circle, Arc, Point, Trim, Mirror, Offset — jedes Werkzeug ist ein Feature mit Schema, gerendert über den generischen FeatureDialog aus K6a.
- Vektor‑basiertes Snapping ausschließlich gegen den B‑Rep‑Proxy.
- Live‑Einbindung des Solvers aus K5a und der Adapter‑Schicht aus K5b.
- Drei‑Farben‑Status (blau/schwarz/rot), Constraint‑Manager‑Panel, Driving/Driven‑Auto‑Konversion.
- Playwright‑Tests für Zeichenwerkzeuge und für die Onshape‑Paritäts‑Szenarien.

**Offene Teile:** B‑Rep‑Snapping auf 3D‑Kanten/‑Vertices beim Skizzieren, vollständiges Constraint‑Manager‑Panel, Face‑Lock via Viewport‑Klick (statt nur Dialog).

### T1 · K7 — Feature‑Tree‑UI, Booleans, Export *(done)*
- Outliner und Feature‑Tree‑Panel im Frontend.
- Boolean‑Knoten (Union/Difference/Intersection — **nur hart**, kein `smin`), Transform‑Knoten, Pattern‑Stub.
- STL‑Ascii/Binary‑Export, GLB‑Export, OBJ‑Export direkt aus den Body‑Mesh‑Caches.

### T1 · K8 — Patterns, Mirror, Variablen, Formel‑Engine *(done)*
- Linear/Circular Pattern, Mirror als Feature‑Tree‑Knoten.
- Vollwertige Formel‑Engine im Kernel (siehe ARCHITECTURE.md §4.5): Expression‑Parser, Einheiten‑System, Funktionsbibliothek, Referenzen, Zyklen‑Erkennung.
- Abhängigkeitsgraph‑basierte Re‑Evaluation.

### T1 · K8.13 — Sketch‑Region‑Selektion für Extrude (Multi‑Profile) *(done)*
- **Problemstellung:** Ein Sketch enthält oft mehrere überlappende Profile, deren Schnittflächen eigenständige Regionen bilden. Der User muss gezielt einzelne, mehrere oder alle Regionen auswählen können (Onshape‑Modell).
- **2D‑Region‑Zerlegung im Kernel:** Der Sketch‑Commit‑Pfad erzeugt aus den geschlossenen Loops eine Planar‑Face‑Partition mit eindeutiger `RegionId`.
- **UI‑Selektion:** Region‑Picker im Extrude‑Dialog, Multi‑Select per Ctrl/Cmd‑Klick.
- **Query‑Integration:** `qAllRegions(sketch_id)`, `qRegion(sketch_id, region_id)`, `qRegions(sketch_id, [region_ids])`.

### T1 · K9 — Referenz‑Import, Measure, Mate Connector *(partial, Mate Connector offen)*
- **STL/OBJ/GLB‑Import als visuelle Referenzgeometrie** (keine SDF‑Konvertierung, keine Booleans). *done*
- Maßband‑Tool über den B‑Rep‑Proxy. *done*
- **Mate‑Connector** als anheftbares lokales Koordinatensystem an Faces/Edges/Vertices des Proxys. *offen*
- Visual‑Regression‑Suite für den gesamten Part‑Editor.

### T1 · K10 — Fillets, Chamfers, Spatial Masks, Ghost Topology *(done)*
- Spatial‑Mask‑Operator im CSG‑AST: Zylinder‑/Kapsel‑SDF entlang einer selektierten Proxy‑Edge. Innerhalb der Maske ersetzt der Operator das harte `min`/`max` durch die Fillet‑Formel oder Chamfer‑Formel; außerhalb fällt er transparent auf den harten Unteroperator zurück.
- **Hierarchische Sub‑IDs für Masken** (aus K4.5): `fillet_7.mask_0`, `fillet_7.mask_1`, …
- Masken‑AABB als eigener BVH‑Knoten — logarithmische Skalierung der Punkt‑Auswertung.
- **Ghost Topology (Dual‑Path):** Visual Path wertet alles aus, Logical Path degradiert `smin → min` und überspringt Masken.
- **History‑Rollback beim Klick auf Ghost‑Edges:** automatisches Einfügen neuer Features vor dem ältesten Fillet/Chamfer, dessen Maske die Contributor‑IDs berührt.
- UI: Click‑to‑Fillet‑Tool, Edge‑Selektion, Radius‑Live‑Preview.
- **Stress‑Test‑Anker „Fillet‑all‑edges":** Gehäuse mit 20+ Kanten, Performance‑ und Masken‑Interferenz‑Regression — grün.

### T2 · K11 — Native SDF‑Superkräfte *(done)*
- Gyroid / TPMS‑Infill als Leaf‑Knoten.
- Procedural Noise / Displacement als Terminal‑Operator.
- Domain Deformation (Bend, Twist) als Transform‑Knoten mit Lipschitz‑Korrektur.
- Morph (Lerp zwischen zwei SDFs).

### T3 · K12 — Analytisches Gewinde‑Tool *(done)*
- Thread‑Primitiv als Leaf‑Knoten mit direkter mathematischer Gewinde‑Formel (metrisch M3–M64).
- Der B‑Rep‑Proxy behandelt das Thread‑Feature im *logical path* als glatten Referenzzylinder.
- UI: Thread‑Tool auf Zylinder‑Face, Gewinde‑Dropdown (M3, M4, …), Richtung (innen/außen), Länge.

### T3 · K13 — Mesh‑to‑SDF: Convert‑to‑Solid und Sweep *(partial, Manifold Repair offen)*
- **Mesh‑to‑SDF‑Knoten:** BVH gegen Dreiecke, kürzeste Distanz plus Winding‑Number oder Pseudo‑Normal‑Check. *done (K13.0–K13.1)*
- **Sweep‑Feature** (Profil entlang Pfad mit Twist): Kernel erzeugt Zwischen‑Mesh, hängt es als Mesh‑to‑SDF‑Knoten in den Baum. *done (K13.2)*
- **Manifold‑Check** bei Import: Wasserdichtigkeits‑Prüfung, UI‑Warnung. *done (K13.3)*
- **Manifold Repair** bei Import: Automatische Reparatur kaputter COTS‑Meshes. *offen*
- **Qualitäts‑Bottleneck Fallback:** Falls Sweep‑Qualität bei Twist‑Pfaden nicht reicht, separater analytischer Sweep‑Operator für einfache Fälle (gerader Pfad, Kreisbogen, ebene Spline ohne Twist) als Notausgang dokumentiert.

### T3 · K14 — Drag'n'Drop für Primitive und Sketches *(partial, Sketch‑DnD offen)*

Für Primitive und Sketches wird Drag'n'Drop als primäre Interaktionsgeste eingeführt. Ziel: Brücke zwischen der spielerischen Bedienung von TinkerCAD und der parametrischen Strenge von Onshape.

- **Drag'n'Drop von Primitiven** (Box, Kugel, Zylinder, …): User zieht ein Primitiv‑Icon aus der Toolbar in den Viewport. Drop ins Leere → `FeatureCall` mit 3D‑Startkoordinate. Drop auf Face → Query‑Referenz `qFaceOf(...)`, parametrische Mitbewegung bei Höhenänderung. *done*
- **Drag'n'Drop für Sketches:** User zieht ein Sketch‑Icon auf eine Fläche. Funktional identisch zu „Neuer Sketch → Fläche auswählen" (K6b), aber als Geste statt Modal‑Workflow. *offen*
- **Technische Passung:** Rein Frontend — der Kernel‑Code bleibt unverändert, Drag'n'Drop ist eine alternative Art, wie das Frontend den JSON‑Befehl an den Kernel schickt.

### T3 · K7.17 — Responsive Toolbar, Settings‑Menü, Compact Mode *(partial — siehe K7.21)*

Geplant waren: Overflow‑Dropdown bei schmalem Viewport, globales Settings‑Menü mit Compact‑Mode‑Toggle (Icon‑only‑Toolbar), Tooltip‑Pflicht im Compact Mode, CSS‑basierte Umschaltung ohne JS‑Rerender.

**Tatsächlicher Stand (Audit 2026‑04‑13):** Nur der Compact‑Mode ist implementiert — `localStorage`‑Toggle, `.fd-compact`‑CSS‑Klasse, `display: none` auf Labels. **Nicht umgesetzt:** Overflow‑Dropdown (die Toolbar nutzt stattdessen `flex-wrap: wrap`, wodurch Buttons bei schmalen Viewports chaotisch stapeln), Settings‑Menü als Ganzes (nur ein Mini‑Toggle‑Button für Compact, kein Burger‑Menu, kein Panel), Tooltip‑Pflicht im Compact Mode (Buttons haben teilweise `title`‑Attribute, aber kein konsistentes Tooltip‑System). Die Slice wurde fälschlich als „done" markiert. Die offenen Teile sind in **K7.21** (UI‑Härtung) aufgebrochen und neu priorisiert.

### T1 · K7.22 — UX‑Fixes aus User‑Testing (2026‑04‑13) *(partial — Bug‑Fixes done, große Features offen)*

Menschliche UX‑Tests am 2026‑04‑13 haben zehn konkrete Probleme identifiziert, die teils Bug‑Fixes und teils neue Features sind. Der kleinere Anteil (Crashes, Perf, Dropdowns, Delete, Splitter, Part‑Tree, Query‑Picker, Drop‑Position, Translate‑Gizmo) wurde sofort behoben; die größeren Items (Rotation/Scale‑Gizmo, Extrude‑Face‑Gizmo, Transform‑als‑eigenes‑Feature, Sketch‑Drag, volle Thread/Sketch‑Dialoge auf YAML‑Spec‑Niveau) wandern als neue K7.21‑Items in die UI‑Härtung.

**Erledigte Sofort‑Fixes:**

- **(h) Sketch‑XY‑Crash (React "Rendered fewer hooks") — done.** `ref={useRef(...)}` inline in JSX in `web/src/Toolbar.tsx` erzeugte einen Conditional‑Hook, der beim Übergang non‑sketch → sketch‑mode gegen die Early‑Return‑Variante verhungerte. Ref entfernt, dead‑code.
- **(a) Render‑Lag: Canvas rendert nicht mehr pro Frame — done.** `<Canvas frameloop="demand">` gesetzt + neuer `Invalidator`‑Component in der Scene, der auf `state.meshedBodies`, `sketchSession.active`, `selection` und `ndc` reagiert und `invalidate()` explizit triggert. Chrome‑Tab bleibt idle, wenn nichts mutiert. OrbitControls und andere drei‑Helper triggern Invalidation intern.
- **(i) YAML‑Specs in `assets/onshape/yaml/*.yaml` als Source of Truth — adoption gestartet.** Dialog‑Specs für sketch/extrude/helix/transform/fillet/hole/plane/boolean/pattern/mirror/revolve/… gelesen. Kernaussage: Alle Onshape‑Feature‑Dialoge haben Selection‑Inputs mit Click‑Focus und Dropdowns mit `reveals:`‑Konditional‑Fields. Toolbar‑Aufklapp‑Menüs für Feature‑Parameter sind ein Anti‑Pattern.
- **(j) Thread/Sketch Toolbar‑Dropdowns entfernt — done.** `METRIC_THREADS` und `SKETCH_PLANES` Dropdowns weg. Click Thread → M6‑Default (editierbar via FeatureDialog), Click Sketch → XY‑Plane‑Default. Plane/Thread‑Preset‑Auswahl wandert in den FeatureDialog als ordentliche Enum‑Parameter (folgt mit K7.21g, volle YAML‑Dialog‑Adoption).
- **(c) "no hover" Label entfernt — done.** `PickHud` rendert im Idle‑State als leeres `.is-idle` Div (`display: none`) statt eines verwirrenden "no hover"‑Debug‑Texts.
- **(b) Outliner‑Splitter für X‑Resize — done.** 7 px Gutter auf der linken Kante des `.fd-outliner`, PointerDown/Move/Up‑Handler mit PointerCapture, Breite clamped auf `[220, 640]` px, persisted in `localStorage.fielddraft.outlinerWidth`, Double‑Click restore‑to‑default.
- **(e) Feature‑Delete inkonsistent — done + Regression‑Test.** Root‑Cause: `Kernel::remove_feature` iterierte nur `document.features` mit flat‑u32 Vergleich und verfehlte damit Features wie `hole`, deren Spine unter hierarchischen Child‑IDs (`42.0`) liegt. Fix: Scan sowohl `feature_calls` (authoritativ für non‑spine types) als auch `features` (inklusive `feature_id_is_descendant_of` für Cascade‑Delete von Pattern/Mirror/Hole‑Kinder). Neuer Test `remove_feature_deletes_hole_and_its_hierarchical_child_spine`.
- **(f) Part‑Tree unter Feature‑Tree — done.** Neue `PartTree`‑Sektion am unteren Ende des `.fd-outliner` mit einer Zeile pro Body aus `state.meshedBodies`, Farb‑Swatch nach Body‑Index, Name aus dem Seed‑Feature, Triangle‑Count, Klick selektiert das Seed‑Feature. Geunionte Körper erscheinen wie geplant als ein einziger Eintrag (die K3‑Partition mergt sie zu einem Body).
- **(g) Extrude Profiles Click‑Picker — done.** Neuer `queryPickerBridge` (Modul‑Singleton mit `register`/`subscribe`/`getActive`). `QueryWidget` im FeatureDialog hat jetzt einen Click‑Focus‑Toggle (⊕), auto‑armed bei leerem Required‑Field. Viewport‑Click prüft `getActiveQueryPicker()` vor der Standard‑Selection und konvertiert den Pick‑Treffer in einen `qCreatedBy(feature_id)`‑Query‑String. Für Extrude heißt das: Dialog öffnen → Click in Szene auf ein Sketch‑Feature → Profiles‑Feld gefüllt. Das in der YAML‑Spec geforderte Drag‑Gizmo für Extrude‑Depth ist NICHT enthalten (siehe K7.21g).
- **(d) Click‑Selektion + Translate‑Gizmo — partial.** Click‑Selektion funktionierte schon (existierte in `handleClick` via `store.selectOnly`). **Neu:** (1) Drop‑Position‑Fix — `store.addPrimitive(kind, op, translate?)` nimmt jetzt optionale Koordinaten, `handleDrop` im Viewport nutzt `pickResult.point` wenn verfügbar, statt alles am Origin zu stapeln. (2) `SelectionTransformGizmo` — drei `<TransformControls>` im Translate‑Modus an einem invisible Placeholder‑Mesh, positioniert auf `selectedFeature.translate`. `onMouseUp` commited via `kernel.setFeatureTranslate` + `nudgeSelected(0,0,0)` als Store‑Refresh.

**Offen (als neue Sub‑Slices in K7.21):**

- **K7.21g — YAML‑driven Dialog‑Refactor.** Thread‑Dialog auf `helix.yaml`‑Niveau (Cylinder/Axis/Circle‑Tabs mit `input_type` Dropdown für Turns/Pitch/Both), Sketch‑Dialog auf `sketch.yaml`‑Niveau (sketch_plane als Selection‑Input mit Active‑Highlight, Sketch‑diagnostics Footer, Show‑Constraints/Show‑Expressions/Show‑Errors‑Toggles), Extrude‑Dialog auf `extrude.yaml`‑Niveau (Solid/Surface/Thin‑Tabs, End‑Condition‑Dropdown mit `reveals:`, Direction/Starting‑Offset/Draft/Second‑End‑Position Expandable Sections, Merge‑Scope). Auch Fillet (Edge/Full_round‑Tabs), Hole (Simple/Counterbore/Countersink + Hole‑Style‑Dropdown), Plane (8 Plane‑Types), Revolve, Loft, Sweep. Volle Adoption der YAML‑Specs als Schema‑Input. Aufwand: ~3–5 Sessions. Nutzen: hoch — macht alle Features produktiv editierbar ohne Toolbar‑Shortcuts.
- **K7.21h — Rotation/Scale‑Gizmo‑Modi.** `SelectionTransformGizmo` kennt bisher nur Translate. Erweitern um Rotation (`mode="rotate"`) und Scale (`mode="scale"`) mit Mode‑Toggle (z. B. R/S‑Tastendrücke oder Topbar‑Buttons wie Onshape). Beim Commit: `setFeatureTransform` statt `setFeatureTranslate`. Aufwand: ~1 Session. Nutzen: mittel — rundet den Direct‑Manipulation‑Pfad ab.
- **K7.21i — Extrude‑Gizmo auf Face (Click‑to‑Extrude).** Wenn ein einzelnes Face selektiert ist, zeigt der Viewport einen Pfeil‑Gizmo entlang der Face‑Normalen. Drag erzeugt ein neues Extrude‑Feature (statt einen Upstream‑Parameter zurück aufzulösen). Entspricht §6.3 "Direct‑Manipulation läuft über neue Features, nicht Rückwärts‑Lösen". Aufwand: ~2–3 Sessions. Nutzen: hoch — zentrales Onshape‑Hebel.
- **K7.21j — Transform als eigenes Feature (`TransformDef`).** Aktuell ist `translate/rotation/scale` in jeder `SpineFeature` eingebacken. Onshape hat ein separates Transform‑Feature (`transform.yaml`: `entities_to_transform` Selection‑Input, `transform_mode` Dropdown mit Translate_by_line/Translate_by_XYZ/Rotate/Scale/Copy_in_place/…). FieldDraft‑Variante: neuer Rust‑Feature `TransformDef` + `TransformFeature` in der Registry, der seinen Target‑Body nicht mutiert, sondern in eine `CsgNode::Transform`‑Hülle wickelt. Der Translate‑Gizmo aus (d) wird als Shortcut dafür verwendet (statt die Spine‑Transform inline zu editieren). Aufwand: ~3–4 Sessions. Nutzen: hoch — alignment mit YAML‑Spec und Copy‑in‑place‑Funktionalität.
- **K14‑Rest — Drag‑and‑Drop für Sketches.** Ist in K14 als "offen" markiert. Gehört mit K7.21e zusammen — beide brauchen Face‑Selektion für den Sketch‑Start.

**Commit‑Log der Sofort‑Fixes:** siehe `git log` nach `22.04.2026`.

### T1 · K7.21 — UI‑Härtung (Burger‑Menu, Toolbar‑Overflow, Constraint‑Manager, Face‑Snap) *(neu, offen)*

Dieser Slice zieht die beim UI‑Audit vom 2026‑04‑13 identifizierten kritischen Lücken in einen eigenen T1‑Slice. Motivation: Die UI ist für den Demo‑Workflow (Sketch → Extrude → STL) bedienbar, aber **nicht produktiv navigierbar** — es fehlen Burger‑Menu und Settings‑Panel (Discoverability), Toolbar‑Overflow‑Handling (Responsive), Compact‑Mode‑Tooltips (Icon‑only ohne Beschriftung unbedienbar), Constraint‑Manager‑Panel (Driving/Driven‑Toggle per Rechtsklick, onshape‑ux.md §2.17a), Face‑Lock via Viewport‑Klick und visuelle Regressionstests. Der Slice ist **vor K3.5 einzuplanen**, weil K3.5 Infrastruktur ohne User‑Impact liefert, während K7.21 den User konkret freischaltet.

**Audit‑Bezug:** Die UI war bisher nur stichprobenartig gegen `onshape-ux.md` und `assets/onshape/*.png` geprüft. Der FeatureDialog und die Constraint‑Glyphen sind nachweislich aus den Screenshots abgeleitet und gut umgesetzt — das Topbar‑/Settings‑Chrome und der Sketch‑Constraint‑Manager sind es *nicht*. K7.21 holt diese Lücke auf.

**Sub‑Slices in Priorisierungs‑Reihenfolge:**

- **K7.21a — Burger‑Menu + Settings‑Panel.** Neues `SettingsMenu`‑React‑Component in der Topbar rechts: Burger‑Icon (☰) oder Zahnrad (⚙️) öffnet ein Popover/Dropdown mit globalen Einstellungen. Initialer Inhalt: Compact‑Mode‑Toggle (jetzt in Settings statt als einzelner Topbar‑Button), Workspace‑Units (mm/in/…) als Slot für später, Shortcuts‑Cheat‑Sheet als Read‑only‑Liste, Theme‑Slot (Dark Mode später). Einstellungen weiterhin in `localStorage` persistiert; `SettingsContext` liefert sie an die App‑Tree. Der bestehende `compact-toggle`‑Button in `App.tsx` wird entfernt und zieht in das Settings‑Menü um. Aufwand: ~1–2 Sessions. Nutzen: hoch — Discoverability.
- **K7.21b — Toolbar‑Overflow‑Dropdown.** Bei Viewport‑Breiten unter einem Schwellwert (Default ~900 px) verschiebt die Toolbar überschüssige Buttons automatisch in ein `···`/`More`‑Dropdown am rechten Rand. Sichtbare Tools werden nach fester Priorität sortiert (Sketch, Extrude, Boolean immer sichtbar; Patterns, Thread, Gyroid eher im Overflow). Die Umsetzung nutzt einen `ResizeObserver` auf dem Toolbar‑Container und berechnet pro Resize, welche Buttons Platz haben. Kein `flex-wrap: wrap` mehr — die Toolbar bleibt immer einzeilig, der Rest landet im Dropdown. Aufwand: ~2–3 Sessions. Nutzen: hoch — responsive Bedienbarkeit.
- **K7.21c — Compact‑Mode‑Tooltips (Quick Win).** Jeder Toolbar‑Button erhält verpflichtend ein `title`‑Attribut mit dem Tool‑Namen. Im Compact Mode (Labels versteckt) sind die Tooltips die **einzige** Beschriftung. Browser‑native `title`‑Tooltips reichen als initialer Aufwand; eine Custom‑Tooltip‑Component kann später folgen. Ergänzung: Tests prüfen, dass jeder `.tool`‑Button ein nicht‑leeres `title` hat. Aufwand: ~0,5 Sessions. Nutzen: mittel, aber extrem schneller Quick Win.
- **K7.21d — Constraint‑Manager‑Panel (Driving/Driven‑Toggle).** Neuer Tab oder Panel im Sketch‑Editor‑Chrome, der alle Constraints des aktiven Sketches listet (nutzt die Diagnostics‑Pipeline aus K5b). Jede Dimension hat ein Kontextmenü (Rechtsklick) mit „Change to Driving"/„Change to Driven" — die DoF‑Analyse aus `web/src/solver/dofPreflight.ts` liefert die Validierung, der Solver‑State wird nach dem Toggle neu solved. Driving = schwarz/editierbar, Driven = grau/schreibgeschützt (entspricht onshape‑ux.md §2.17a und `assets/onshape/onshape-sketch-editor.png`). Aufwand: ~2–3 Sessions. Nutzen: mittel — schließt eine sichtbare Sketch‑UX‑Lücke.
- **K7.21e — Face‑Lock via Viewport‑Klick.** Beim Start eines neuen Sketches (Toolbar‑Sketch‑Button oder Drag‑and‑Drop aus K14) wechselt der Viewport in einen Face‑Select‑Modus: Klick auf ein Face des 3D‑Modells setzt die Sketch‑Ebene, nicht nur auf XY/XZ/YZ aus dem Dropdown. Nutzt den bestehenden B‑Rep‑Proxy‑Picker aus K2/K10. Die Sketch‑Ebene wird weiterhin als Query (`qFaceOf(feature_id)`) persistiert, nicht als Koordinate — parametrische Mitbewegung bei Änderungen am Basiskörper. Aufwand: ~1 Session. Nutzen: mittel — natürlicher Sketch‑Einstieg.
- **K7.21f — Visual‑Regression‑Baselines.** Playwright‑Screenshot‑Baselines für Topbar, Toolbar (voll + compact), Outliner, FeatureDialog (Extrude, Fillet, Pattern), Sketch‑Editor‑Canvas, Settings‑Menü. Läuft in `web/tests/visual/` separat, nicht im Standard‑`just test`‑Pfad. Ziel: unbeabsichtigte CSS‑Drifts werden bei jedem Release als Diff sichtbar. Aufwand: ~2 Sessions. Nutzen: mittel — Drift‑Schutz, skaliert über zukünftige Slices.

**Test‑Anforderungen für K7.21:**
- Responsive‑Tests: Playwright mit `page.setViewportSize({ width: 600/900/1440 })` — Overflow‑Dropdown erscheint/verschwindet bei den richtigen Schwellwerten, Toolbar bleibt einzeilig.
- Compact‑Mode‑Tests: `localStorage.setItem('compact', 'true')`, Reload, alle Tools haben Tooltips, keine Labels sichtbar.
- Settings‑Menü: Öffnen, Compact toggeln, Persistenz nach Reload, Close per Esc und Click‑Outside.
- Constraint‑Manager: Rechtsklick auf Dimension → Kontextmenü → Toggle → DoF‑Farbe im Canvas wechselt korrekt, Solver liefert korrekte neue Lösung.
- Face‑Lock: Klick auf Face → Sketch‑Mode, Sketch‑Ebene ist parametrisch verknüpft (Test: Höhe des Basiskörpers ändern, Sketch wandert mit).
- Visual‑Regression‑Baselines in `web/tests/visual/`.

**Abnahme:** Alle sechs Sub‑Slices umgesetzt und getestet; ROADMAP‑ und STATUS‑Einträge wahrheitsgemäß aktualisiert; UI fühlt sich auf 1280×720, 900×600 und 600×800 (iPad‑artig) jeweils bedienbar an.

**Nicht im Scope:** B‑Rep‑Face‑Snap **beim Zeichnen** innerhalb eines Sketches (Snap auf Kanten/Vertices des 3D‑Modells während Linien gezogen werden) — das ist eine eigene K6b‑Härtung und hängt am vollen K9‑Proxy. K7.21e deckt nur den Face‑Lock bei **Sketch‑Start** ab, nicht das Snapping während des Zeichnens.

### T3 · K15 — Input Map Presets (konfigurierbare Maus‑/Tastatur‑Bedienung) *(offen)*

CAD‑Programme verwenden unterschiedliche Maus‑Tastenbelegungen für Orbit, Pan, Zoom, Selektion und Kontextmenü. Umsteiger müssen sich nicht umgewöhnen, wenn FieldDraft die gewohnte Belegung als Preset anbietet. Dieser Slice abstrahiert die gesamte Pointer‑Interaktion (nicht nur OrbitControls) hinter einem `InputMap`‑Objekt, das per Setting‑Dropdown umgeschaltet werden kann.

**Betroffene Interaktions‑Bereiche:**

| Aktion | Variiert zwischen Presets | Beispiel |
|---|---|---|
| **Orbit** | LMB (Three.js) vs. RMB (Onshape/Tinkercad) vs. MMB (Blender) vs. Shift+MMB (Fusion 360) | |
| **Pan** | RMB (Three.js) vs. MMB (Onshape/Fusion/Blender) vs. Shift+RMB (Tinkercad) | |
| **Zoom** | Wheel überall; Ctrl+MMB vertikal (Blender) als Alternative | |
| **Select** | LMB‑Klick überall, aber Drag‑Select‑Rechteck‑Trigger variiert | |
| **Context Menu** | RMB (Fusion 360, Blender) vs. nicht belegt (Three.js, Onshape, Tinkercad) | |
| **Gizmo‑Drag** | LMB überall, aber Modifier‑Konflikte mit Orbit bei LMB‑Orbit‑Presets | |

**Presets:**

- **Web / Three.js (Default):** Orbit=LMB, Zoom=Wheel/MMB, Pan=RMB oder LMB+Ctrl/Shift, Select=LMB‑Klick
- **Onshape:** Orbit=RMB, Pan=MMB, Zoom=Wheel, Select=LMB‑Klick/Drag‑Rect, Context=nicht belegt
- **Fusion 360:** Orbit=Shift+MMB, Pan=MMB, Zoom=Wheel, Select=LMB, Context=RMB
- **Blender:** Orbit=MMB, Pan=Shift+MMB, Zoom=Wheel oder Ctrl+MMB, Select=LMB, Context=RMB
- **Tinkercad:** Orbit=RMB, Pan=MMB oder Shift+RMB, Zoom=Wheel, Select=LMB

**Implementierung:**

1. **`InputMap`‑Typ** als zentrales Mapping‑Objekt: Für jede Aktion (Orbit, Pan, Zoom, Select, ContextMenu, GizmoDrag) definiert es Maus‑Button + Modifier‑Kombination + Touch‑Geste. Rein deklarativ, kein Event‑Handling.
2. **`InputMapRegistry`** mit den fünf Presets. Eigene Presets als Stretch Goal.
3. **OrbitControls‑Anbindung:** R3F‑OrbitControls haben `mouseButtons` (LEFT/MIDDLE/RIGHT → ROTATE/DOLLY/PAN) — der `InputMap` setzt diese Felder beim Mount/Wechsel.
4. **Viewport‑Click‑Dispatcher:** Der bestehende `onPointerDown`/`onClick`‑Handler in `Viewport.tsx` prüft gegen die aktive `InputMap`, ob der Klick als Select, ContextMenu oder ignoriert (weil Orbit/Pan) interpretiert wird.
5. **Gizmo‑Koordination:** TransformControls nutzen intern LMB für Drag. Bei Presets mit LMB=Orbit muss der Gizmo‑Hover‑State OrbitControls *vor* dem Drag deaktivieren (bereits implementiert via `dragging-changed`, muss aber mit dem neuen Dispatch zusammenspielen).
6. **Context‑Menu‑Infrastruktur:** Für Presets mit RMB=ContextMenu wird ein generisches Kontextmenü‑System benötigt (Feature‑Aktionen: Edit, Delete, Rename, Move Up/Down). Für Presets ohne ContextMenu bleibt RMB bei der alternativen Aktion (Orbit/Pan). Das ContextMenu selbst ist ein eigener Implementierungs‑Block innerhalb dieses Slices.
7. **Settings‑UI:** Dropdown im Settings‑Panel (K7.21a) unter „Navigation". Wert in `localStorage` persistiert. Default: „Web / Three.js" (Rückwärtskompatibilität).
8. **Touch‑Gesten:** Analog zur Maus‑Belegung — 1‑Finger=Orbit vs. Select hängt vom Preset ab. Stretch Goal, kann als Phase 2 innerhalb des Slices kommen.

**Abhängigkeiten:** K7.21a (Settings‑Panel für UI‑Integration). Kein Kernel‑Code betroffen.

**Tests:**
- Unit‑Tests für `InputMap`‑Auflösung: Gegeben Button+Modifier → erwartete Aktion.
- Playwright‑Tests pro Preset: Orbit/Pan/Select‑Geste erzeugt das erwartete Kamera‑Verhalten.
- Regressions‑Test: Default‑Preset verhält sich identisch zum bisherigen Verhalten.

---

## Experimentelle Parallel‑Slices (T∞)

Diese Einträge sind **experimentelle Parallel‑Slices**. Sie laufen nicht in der K1–K13‑Hauptlinie, blockieren keinen Meilenstein und werden aufgegriffen, wenn der kanonische Pfad aus den K‑Slices lauffähig ist und/oder wenn Forschungsfragen aus der UX‑Arbeit sie notwendig machen.

### T∞ · K‑Exp1 — Skelett‑First‑Renderpfad (`AnalyticalPreview`)

Hypothese: Der SDF‑Render‑Pfad (Dual Contouring im Worker) ist für Ausgabe‑Qualität optimal, aber für Live‑Drag‑Feedback nicht ideal. Ein paralleler, leichtgewichtiger **analytischer Preview‑Pfad** soll die Lücke schließen, ohne den bestehenden DC‑Pfad zu ersetzen. Drei Render‑Stufen statt zwei:

- **Stufe 0 — Analytisches Skelett (<1 ms pro Frame).** Kein DC‑Pass. Direkt aus den Feature‑Parametern berechnet: Extrude als Polyline‑Kontur + Deck/Boden, Revolve als parametrischer Zylindermantel. Edges aus dem B‑Rep‑Proxy als `LineSegments`. Für Features *vor* dem ersten Boolean exakt; *ab* dem ersten Boolean wird sie zur Ghost‑Topology (harte Schnittkanten aus dem logical path).
- **Stufe 1 — Grobes DC‑Mesh (10–50 ms).** Worker rechnet mit Resolution 16–24.
- **Stufe 2 — Feines DC‑Mesh (100–500 ms).** Volle Resolution, scharfe Kanten via QEF. Das ist der heutige Mesh‑Pfad aus K3.

**Geplante Komponenten:** Kernel‑API `analyticalBodyDescription(body)`, Frontend `AnalyticalPreview`‑Component als Wireframe‑Overlay, Stufen‑Orchestrierung (Stufe 0 sofort, Stufe 1 debounced, Stufe 2 nach ~200 ms Stille).

**Offene Fragen:** Kosten der analytischen Beschreibung ab dem ersten Boolean. Fillet‑Radius‑Drag im Skelett (Spatial Masks leben im Visual Path). Gizmo‑Drag auf Boolean‑Sub‑Komponenten.

**Nicht‑Ziele:** Ersetzt **nicht** K3, K6a/K6a.1, K6b. Zusätzliche Schicht, die bei Erfolg Plasticity‑artige Reaktivität liefert.

**Abbruch‑Kriterium:** Wenn Stufe 0 für Booleans zu aufwändig wird oder die Stufen‑Übergänge visuell nicht sauber werden, wird der Slice eingefroren und Reaktivität stattdessen über progressives Refinement **innerhalb** von Stufe 1 gelöst.

---

## Backend‑Phase (M11–M13)

*Ab hier beginnt die Backend‑Phase. Der clientseitige Modeller ist nach K9 funktional komplett; die folgenden drei Meilensteine ergänzen Server, Accounts und Sync.*

### M11 — Backend‑Grundgerüst: Auth, User, leeres Projekt *(done)*
- `server/`‑Ordner mit Go + `chi` + `sqlc` + `golang-migrate`.
- `docker-compose.yml` mit MariaDB, Server und Web.
- `users`‑Tabelle, Argon2id‑Hashing, Session‑Cookies.
- Endpunkte: `POST /api/auth/register`, `/login`, `/logout`, `GET /api/me`.
- Frontend: Registrierungs‑, Login‑, Logout‑Views.

### M12 — Projekt‑CRUD, Sichtbarkeit, Sync *(done)*
- `projects`‑Tabelle, Endpunkte für Create/List/Read/Update/Delete.
- Dashboard: Projektliste, Neu, Umbenennen, Löschen, Sichtbarkeits‑Toggle.
- Öffentliche Galerie (read‑only View ohne Login).
- Sync‑State‑Machine (Clean, Dirty, Syncing, Error) im Frontend‑Store.
- `POST /api/projects/:id/snapshot` mit JSON‑Body.
- Gast‑Modus: `localStorage`‑Stand wird beim ersten Login optional in den Account übernommen.
- Hybrid‑Modus: `localStorage` bleibt Offline‑Cache, Server wird kanonisch.

### M13 — Hardening und öffentliche Beta *(partial)*
- Rate Limiting, AppShell Router, Self‑Hosting Docs (`DEPLOY.md`), Quotas. *done*
- **Passwort‑Reset via E‑Mail.** *offen*
- **Performance‑Pass:** WASM‑Profiling, Mesher‑Compute‑Paths, BVH‑Update‑Inkremente. *offen*
- **Öffentliche Projektgalerie mit Thumbnails.** *offen*

---

## Phase 3 — Interaktions‑Export, Simulation, Physik

Diese Phase hebt FieldDraft vom reinen Konstruktionswerkzeug zur Entwicklungs‑Plattform für interaktive 3D‑Applikationen. Sie nutzt Fähigkeiten, die aus der SDF‑Architektur „gratis" abfallen — Kollisionsabfragen in O(1), netzfreie FEM, direkte Toolpath‑Generierung.

### P1 — Assemblies und Scene Graph
- Paradigmenwechsel vom CSG‑Baum zum Scene Graph auf der Assembly‑Ebene: Jedes Einzelteil bleibt ein isolierter Part Studio mit eigenem CSG‑Baum. Die Assembly verknüpft diese Parts über Transformations‑Hierarchien und mechanische Freiheitsgrade.
- Mate‑Typen: Fixed, Revolute, Prismatic, Cylindrical, Ball. Jeder Mate bindet zwei Mate‑Connectors aus K9.
- Hierarchische Bewegung, parametrisch und debounced persistiert wie der Feature‑Tree.
- Tests: Goldfile‑Szenen für Roboterarm, Linearachse, Scherenmechanik.

### P2 — Kinematik: FK und IK
- **Forward Kinematics:** Slider bewegen einzelne Joint‑Winkel.
- **Inverse Kinematics:** Zielkoordinate im Raum → IK‑Solver berechnet Joint‑Winkel. Joint‑Limits aus P1 als Constraints.
- **Selbstkollisions‑Prüfung:** Die SDFs der beteiligten Parts liefern den Abstand zwischen Punkten in O(1) — der IK‑Solver kann Pfade verwerfen, bei denen ein Part durch ein anderes schwingt.

### P2.5 — Shareable 3D‑Viewer und IFrame‑Embeds
- **Publishing‑Workflow:** JSON‑Snapshot in MariaDB → URL (`fielddraft.io/v/123`) oder `<iframe>`‑Snippet.
- **Leichtbau‑Viewer:** Viewport in einem „Read‑Only"‑Modus (R3F) — kein schwerer Geometrie‑Kernel im Worker nötig.
- **Interaktive Dokumentation:** Pitch‑Decks, Online‑Montageanleitungen — Betrachter dreht/wendet/bewegt Gelenke im IFrame (zusammen mit FieldScript aus P2.8).

### P2.8 — FieldScript (fd) für Assembly‑Kinematik
- **Integration:** Ein `fd`‑Skript im Assembly‑Kontext fungiert als Controller, greift direkt auf Mates und Joints zu.
- **UI‑Generierung:** Skript deklariert Parameter im Header (`angle`, `speed`, `color`). Daraus rendert das generische Dialog‑System (K6a) Steuer‑Elemente (Slider, Buttons).
- **Runtime:** Deterministische JS‑Runtime im Web Worker. Standard‑JavaScript, optional Standard‑Library für Mechanik‑Animationen (Sinus‑Oszillation, Schrittketten).

### P3 — Code‑First‑Export: React Three Fiber
- Export‑Button erzeugt nicht nur STL, sondern ein Paket: optimierte Meshes (GLTF/DRACO), generierte `.tsx`‑Datei mit Scene Graph als R3F‑Komponentenbaum, Props für jeden Joint‑Winkel, optional `leva`‑basiertes Slider‑Panel.
- Beispiel‑Output: `<RobotArm shoulderAngle={…} elbowAngle={…}/>` mit verschachtelten `<group rotation={…}>`‑Knoten.
- USP: *FieldDraft exportiert Interaktion, nicht nur Geometrie.*

### P4 — Physikalische Abfragen: Masse, Kollision, Toolpaths
- **Volumen und Massenschwerpunkt:** Divergenztheorem auf dem DC‑Mesh. Dichte pro Material konfigurierbar.
- **Kollisionserkennung als generisches O(1)‑Primitiv:** SDF beantwortet „liegt Punkt P im Inneren und wie tief?" direkt — Grundlage für IK‑Selbstkollisions‑Prüfung aus P2.
- **SDF‑direkte CNC‑/Slicer‑Toolpaths:** Werkzeugpfade als Iso‑Fläche `f(p) = werkzeugradius`. Im MVP als experimentelles CAM‑Modul für 2.5D‑Fräsen; 5‑Achs‑Strategien out of scope.

### P5 — Netzfreie FEM via Finite Cell Method
- Netzfreie Finite‑Element‑Simulation direkt auf einem regelmäßigen Voxel‑Gitter über dem SDF: Jede Zelle bekommt vom SDF einen Füllgrad (0 %–100 %), die Belastungs‑Gleichungen werden auf diesem Gitter gelöst. Ermöglicht Simulation komplexer Gyroid‑/TPMS‑Strukturen aus K11.
- Lasten und Randbedingungen werden auf den B‑Rep‑Proxy gelegt.
- Realistisches Ziel: statische Linear‑Elastizität für einfache Leichtbau‑Parts, nicht Crash‑Simulation.

---

## Spätere Meilensteine (M14+)

- **Loft, Curved Patterns, 3MF‑Export.**
- **STEP‑Import und Mechanical‑Core‑Export via OpenCASCADE (Batch‑Worker).** Beide Richtungen laufen über denselben OCCT‑Worker, nur auf explizite User‑Aktion aktiviert. Import: STEP → OCCT → Tessellation → Mesh‑to‑SDF‑Knoten (K13) mit Wasserdichtigkeits‑Check. Export: harter Teil des Feature‑Trees (Primitive, Extrusionen/Revolves, harte Booleans) → OCCT‑Befehlskette → NURBS‑B‑Rep → STEP. Alles Weiche (smin‑Blends, Spatial‑Mask‑Fillets, Gyroid/TPMS, Noise, Domain Deformation, Morph, Mesh‑to‑SDF) wird **nicht** übernommen und dem User transparent gemeldet. WASM‑Payload: 20–30 MB zusätzlich, ausschließlich im Worker. `truck` als reiner Rust‑Alternativ‑Kernel bleibt als Beobachtungs‑Kandidat.
- **Direct‑Manipulation‑Modus** (Push/Pull auf B‑Rep‑Proxy‑Faces), sobald ein robustes Heuristik‑Modell für Push/Pull‑zu‑Parameter‑Auflösung existiert.
- **Kollaborative Sessions** via CRDT (Yjs o. ä.).
- **Optionale Ende‑zu‑Ende‑Verschlüsselung** privater Projekte.
- **OAuth‑Login.**
