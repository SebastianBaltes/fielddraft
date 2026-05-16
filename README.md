# FieldDraft — Projektbeschreibung

![image](assets/logo.png)

> © 2026 Sebastian Leonardo Baltes, Deutschland. Alle Rechte vorbehalten.
> Dieses Dokument einschließlich aller Inhalte, Konzepte und technischen Beschreibungen ist urheberrechtlich geschützt.
> Jede Vervielfältigung, Verbreitung, öffentliche Zugänglichmachung oder sonstige Verwertung – ganz oder teilweise – bedarf
> der vorherigen schriftlichen Zustimmung des Rechteinhabers. Vertraulich – nicht zur Weitergabe bestimmt.

## 1. Kurzfassung

**FieldDraft** ist ein browserbasierter, parametrischer 3D‑Modeller, der sich in Bedienung und Funktionsumfang an **Onshape** orientiert, technisch aber einen radikal anderen Weg geht: Statt eines klassischen B‑Rep/NURBS‑Kernels (OpenCASCADE, Parasolid, ACIS) setzt FieldDraft auf einen **SDF‑basierten Geometriekern** (Signed Distance Fields / Implicit Modeling).

Zielanwendung ist der **FDM/SLA‑3D‑Druck** in einem Bauraum von typischerweise bis zu 300 × 300 × 300 mm mit praxisrelevanter Präzision von ca. 0,1 mm.

FieldDraft ist bewusst **kein** vollständiger Onshape‑Ersatz für zerspanenden Maschinenbau, sondern ein **vereinfachter Onshape‑Clone** für den Maker‑ und Hardware‑Startup‑Workflow: genug Parametrik und Constraint‑Sketching, um Prototypen, Gehäuse, Halterungen und Mechanik‑Teile sauber zu konstruieren — ohne die Altlasten der CNC‑Welt mitzuschleppen.

Der Name **FieldDraft** beschreibt das Produkt wörtlich: ein **Draft**‑Tool (technisches Zeichnen / Konstruieren), das im **Field** (Distanzfeld) rechnet.

---

## Dokumentationsstruktur

Diese README ist das **Einstiegsdokument** und beschreibt die Vision, Zielgruppe, Scope, Lizenz und die harten Architektur‑Grenzen. Details liegen in separaten Dateien, damit der Pair‑Programming‑Agent nur das laden muss, was er gerade braucht:

| Datei | Inhalt | Wann lesen |
|---|---|---|
| **`README.md`** (dieses Dokument) | Vision, Zielgruppe, Funktionsumfang, Dateiformate, Lizenz, harte Scope‑Grenzen | Einmal beim Onboarding |
| [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) | Technische Architektur (Kernel §4.1, Frontend §4.2, Memory‑Brücke §4.3, Solver §4.4, Variablen §4.5, Feature‑as‑Function §4.6, Backend §4.7) + offene architektonische Risiken (§11) | Bei Kernel‑ oder Frontend‑Arbeit |
| [`docs/ROADMAP.md`](docs/ROADMAP.md) | K‑Slices, Release‑Tiers (T0–T3), Backend‑Meilensteine (M11–M13), Phase 3 (P1–P5) | Bei Slice‑Planung |
| [`docs/LITERATURE.md`](docs/LITERATURE.md) | Kuratierte Pflichtlektüre pro Themengebiet (Dual Contouring, QEF, SDF‑Primitive, BVH, PlaneGCS, wgpu, …) | Beim Implementieren eines Algorithmus — nur den relevanten Abschnitt |
| [`docs/STATUS.md`](docs/STATUS.md) | Ist‑Zustand aller Slices (`done`/`partial`/`open`), Test‑Zahlen, Empfehlung zur weiteren Entwicklung | Bei Status‑Fragen; wird häufig aktualisiert |
| [`DEPLOY.md`](DEPLOY.md) | Self‑Hosting‑Anleitung (Docker Compose, MariaDB, Production‑Deployment) | Beim Deployment |
| [`CLAUDE.md`](CLAUDE.md) | Agent‑spezifische Arbeitsanweisungen (Telegram‑Kommunikation, Build & Test, Doc‑Map) | Automatisch bei Session‑Start geladen |
| [`onshape-ux.md`](onshape-ux.md) | Vollständige Onshape‑Paritäts‑Analyse für die Sketch‑UX | Vor K6b und UX‑Arbeit am Sketcher |
| [`docs/BUSINESS.md`](docs/BUSINESS.md) | Lizenzmodell, Preisstrategie, Zielgruppen‑Positionierung, Kommerzialisierung | Nicht Teil der technischen Dokumentation |

Querverweise auf „§4.x" zeigen auf `docs/ARCHITECTURE.md`, „§10" auf `docs/ROADMAP.md`, „§11" auf `docs/ARCHITECTURE.md` (dort als eigener Abschnitt), „§16.x" auf `docs/LITERATURE.md`. §2, §3, §5–§9, §12–§15 leben in dieser README.

---

## Quick Start

### Entwicklung (lokal, Browser‑only)

Der komplette Modeller läuft im Browser ohne Backend. Gast‑Modus nutzt `localStorage` als einzige Persistenz.

```bash
# 1. Web‑Dependencies installieren (einmalig)
just install

# 2. WASM‑Kernel bauen und Vite‑Dev‑Server starten
just dev
```

Der Dev‑Server läuft dann auf `http://localhost:5173`. **Benötigt einen WebGPU‑fähigen Browser** (Chromium stabil, Safari stabil, Firefox teilweise).

### Tests

```bash
just test            # Rust‑Unit‑Tests + Vitest (schneller lokaler Lauf)
just test-rust       # nur cargo test --workspace
just test-web        # nur Vitest
just test-e2e        # Playwright E2E (headed Chromium mit echtem WebGPU)
just install-e2e     # einmalig: Playwright‑Chromium herunterladen
```

### Backend (optional, für Auth + Projekt‑Sync)

Der Server ist **optional** — der Modeller funktioniert ohne. Für Auth, Projekt‑Dashboard und Sync:

```bash
# Variante A: Docker Compose (komplett mit MariaDB)
docker compose up --build -d
# Services: db (MariaDB :3306), server (Go :8080), web (Vite :5173)

# Variante B: Server manuell starten (MariaDB muss laufen)
cd server && go run .
```

Umgebungsvariablen (mit Defaults für Dev): `DB_DSN=fielddraft:fielddraft@tcp(db:3306)/fielddraft`, `PORT=8080`, `SESSION_TTL=168h`.

Vollständige Production‑Deployment‑Anleitung in [`DEPLOY.md`](DEPLOY.md).

### Builds

```bash
just build-wasm   # nur WASM‑Kernel bauen (schneller als voller Build)
just build        # voller Web‑Bundle (WASM + Vite)
just fmt          # Rust + Prettier
just clean        # alle Build‑Artefakte löschen
```

---

## 2. Motivation

Die zugrundeliegende Recherche hat gezeigt, dass es unter Linux für ein Hardware‑Startup mit Patent‑Ambitionen und schmalem Budget de facto kein ideales CAD gibt:

- **Onshape Free** — IP‑Vorveröffentlichung, damit patent­rechtlich toxisch.
- **Onshape Standard/Professional** — zu teuer in der Pre‑Seed‑Phase (~1.500 € / Nutzer / Jahr).
- **SolidWorks, Fusion, Alibre, Solid Edge** — kein nativer Linux‑Client.
- **Plasticity** — kein Light Mode / kein UI‑Scaling, keine Parametrik, keine 2D‑Ableitung.
- **FreeCAD / Ondsel** — UX‑Schwächen.
- **BricsCAD Mechanical** — teurer Einmalkauf, eher klassisches CAD.

Gleichzeitig zeigt die Analyse der benötigten Funktionen, dass für den **3D‑Druck‑Use‑Case** die Schwächen klassischer B‑Rep‑Kernel (TNP, co‑planare Booleans, Fillet‑Edge‑Cases, Single‑Thread‑Performance) **strukturell vermieden** werden können, wenn man auf einen SDF‑Kern umschwenkt:

- Boolesche Operationen sind in SDFs trivial und mathematisch robust (`min`, `max`, `−`) — sie kennen die topologischen Grenzfälle von B‑Rep‑Solvern (co‑planare Flächen, Zero‑Thickness‑Berührungen, TNP) strukturell nicht.
- Offsets sind eine einfache Konstanten‑Subtraktion `f(x) − d`, ohne Self‑Intersection‑Probleme.
- Garantierte Wasserdichtigkeit: Aus einem SDF‑Graphen lässt sich per Dual Contouring ein Mesh ableiten, das mathematisch garantiert keine Löcher oder Self‑Intersections enthält — der Slicer bekommt immer ein druckbares STL.
- Bei 0,1 mm Mesh‑Auflösung ist der „Potato‑Meshing"‑Effekt der klassischen Marching‑Cubes‑Variante für FDM‑Druck praktisch irrelevant, da eine 0,4‑mm‑Düse ohnehin keine perfekt scharfen Kanten druckt; Dual Contouring statt Marching Cubes erhält scharfe mechanische Kanten zusätzlich. Für die Viewport‑Ästhetik und den STL‑Export (wo das Mikro‑Rauschen des QEF‑Solvers sichtbar wird und die uniforme Dreiecksinflation flacher Flächen GPU‑Bandbreite frisst) adressiert **K3.5** das Problem über eine Provenance‑gesteuerte Mesh‑Optimierung: analytisch bekannte Flächen werden per Newton‑Iteration rauschfrei positioniert und topologisch per 2D‑Retriangulierung auf wenige große Dreiecke reduziert (siehe `docs/ARCHITECTURE.md` §4.1 und `docs/ROADMAP.md` K3.5).

FieldDraft zielt damit auf die Lücke: **Onshape‑artige UX, Plasticity‑artige Geschwindigkeit, FreeCAD‑artige Datenhoheit — nativ im Browser, offline‑fähig, unter Linux.**

## 3. Zielgruppe und Anwendungsfälle

**Primär:**
- Hardware‑Startups in der Prototyping‑Phase unter Linux.
- Maker, 3D‑Druck‑Enthusiasten, Robotik‑Hobbyisten.
- Konstrukteure, die Gehäuse, Halterungen, Gimbal‑Arme, Mounts und einfache Mechanik drucken wollen.

**Typische Aufgaben:**
- Hüllgeometrien von Zukaufteilen (Motoren, Platinen, Sensoren) manuell modellieren oder als STL/OBJ‑Referenz laden (STEP‑Import ist Ausblick, siehe §7).
- Booleans gegen importierte Referenzgeometrien, um passgenaue Kavitäten und Bohrungen zu erzeugen.
- Parametrische 2D‑Sketches mit Constraint‑Solver für exakte Maße.
- Export als STL/GLB (später ggf. 3MF) für den Slicer (Bambu Studio, PrusaSlicer, Cura, OrcaSlicer).

**Explizit außerhalb des Scopes (für das MVP):**
- Normgerechte 2D‑Zeichnungsableitung für Zerspaner.
- Spritzguss‑DFM (Entformungsschrägen, Schwindungsanalyse).
- Blechabwicklung.
- Vollwertige Assemblies mit kinematischen Gelenken (Mates) — Kinematik wird extern (z. B. in einer Three.js‑basierten Engine) gelöst.
- FEM‑Simulation.
- Class‑A‑Surfacing.

## 4. Technische Architektur

Die Architektur ist in einem eigenen Dokument beschrieben: **[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md)**. Dort liegen:

- **§4.1** Rust/WASM‑Kernel (CSG‑Graph mit Provenance, Dual‑Contouring‑Mesher, B‑Rep‑Proxy, Multi‑Body‑Isolation, Spatial Masks, Ghost Topology, BVH, 2D‑Kernel, K3.5 Mesh‑Optimierung, Export, Undo/Redo)
- **§4.2** React‑Three‑Fiber‑Frontend (Rendering, Picking via Raymarching‑Sensor, Snapping, generischer FeatureDialog, Auto‑Save)
- **§4.3** Memory‑Brücke (Zero‑Copy zwischen Rust und JS, inkrementeller Per‑Body‑Stream)
- **§4.4** 2D‑Constraint‑Solver (PlaneGCS + Adapter‑Schicht: InferenceEngine, DragController, DoF‑Preflight, Diagnostics)
- **§4.5** Variablen und Formel‑Engine (unit‑safe Expression‑Sprache, Parser im Kernel)
- **§4.6** Feature‑as‑Function (Registry, Schema‑Reflexion, Regeneration Worker, Queries, Determinismus)
- **§4.7** Backend‑Server (Go + MariaDB, spätere Phase)
- **§11** Offene architektonische Risiken (16 Punkte)

Der kurze Überblick: FieldDraft besteht aus einem **Rust/WASM‑Geometriekern** im Browser und einem **React‑Three‑Fiber/WebGPU‑Frontend**. Verbindung über eine **Zero‑Copy‑Memory‑Brücke**. Ein **Go‑Backend‑Server** für Auth und Sync ist optional — im MVP rein clientseitig, Persistenz in `localStorage`.

## 5. User‑ und Projektverwaltung (spätere Phase)

> **Hinweis:** User‑Accounts, ein serverseitiges Dashboard und öffentliche Galerien sind Teil der Backend‑Phase (nach K9). Im MVP gibt es genau einen impliziten „lokalen Workspace" im Browser, dessen gesamter Zustand in `localStorage` liegt. Die folgende Spezifikation beschreibt den Zielzustand, nicht den MVP‑Stand.

### 5.1 Accounts

- **Registrierung:** E‑Mail + Passwort, optional Verifikations‑E‑Mail in Phase 2.
- **Login / Logout:** Session‑basiert, Cookies.
- **Passwort‑Reset:** via E‑Mail‑Token (Phase 2).
- **Gast‑Modus:** Ohne Login nutzbar — das Projekt existiert dann nur lokal in `localStorage`. Beim ersten Login‑Vorgang wird optional angeboten, das lokale Projekt in den Account zu übernehmen.

### 5.2 Projekt‑Dashboard

Nach dem Login landet der User auf einem Dashboard mit:

- Liste aller eigenen Projekte (Thumbnail, Name, letzte Änderung, Sichtbarkeit).
- Button „Neues Projekt".
- Umbenennen, Duplizieren, Löschen.
- Suche und Filter (privat/öffentlich, zuletzt geändert).
- Galerie öffentlicher Projekte anderer User (nur Leserechte).

### 5.3 Sichtbarkeit und IP‑Schutz

Weil jedes Projekt standardmäßig **privat** angelegt wird, ist FieldDraft im Gegensatz zu Onshape Free patentrechtlich unkritisch — der User muss aktiv auf „öffentlich" umschalten, um etwas zu publizieren. Private Projekte liegen verschlüsselt auf dem Server (At‑Rest‑Encryption auf DB‑Ebene); eine optionale Ende‑zu‑Ende‑Verschlüsselung mit user‑gehaltetem Schlüssel ist als Phase‑3‑Feature eingeplant.

## 6. Funktionsumfang (Feature‑Parität zu Onshape)

Die UI‑Referenz orientiert sich eng an den Onshape‑Screenshots (`assets/onshape-part-editor.png`, `assets/onshape-sketch-editor.png`).

### 6.1 Part‑Editor (3D)

Die Tier‑Spalte ordnet jedes Feature einem Release‑Tier aus `docs/ROADMAP.md` zu — **T1** ist der Shippable‑Alpha‑Cut, **T2** ist der SDF‑USP („Reason to Exist"), **T3** ist die Feature‑Complete‑Vision.

| Funktion | Tier | Slice |
|---|---|---|
| Feature‑Tree | T1 | K7 |
| Part‑List / Outliner | T1 | K7 |
| Multiselect | T1 | K7 |
| Kamera‑ und Render‑Optionen | T1 | K7 |
| Ansichts‑Würfel | T1 | K7 |
| Undo / Redo (über Feature‑Tree) | T1 | K7 |
| Sketch (Einstieg in 2D) | T1 | K6b |
| Extrude (Add, Subtract, Intersect, New) | T1 | K6b/K7 |
| Revolve | T1 | K6b/K7 |
| Boolean (Union, Difference, Intersect — nur hart, kein `smin`) | T1 | K7 |
| Linear Pattern | T1 | K8 |
| Circular Pattern | T1 | K8 |
| Mirror | T1 | K8 |
| Transform (Move/Rotate/Scale) | T1 | K7 |
| Delete | T1 | K7 |
| Plane (Referenzebenen) | T1 | K6b |
| Variable / Expressions (Formel‑Engine) | T1 | K8 |
| STL‑Export (ASCII / Binary) | T1 | K7 |
| Maßband / Measure | T1 | K9 |
| Mate Connector (als Ankerpunkt, nicht Kinematik) | T1 | K9 |
| Fillet (via Spatial Mask + Ghost Topology) | T1 | K10 |
| Chamfer (via Spatial Mask + Ghost Topology) | T1 | K10 |
| Gyroid / TPMS‑Infill | **T2** | K11 |
| Procedural Noise / Displacement (Terminal Feature) | **T2** | K11 |
| Domain Deformation (Bend / Twist, Lipschitz‑korrigiert) | **T2** | K11 |
| Morph (Lerp zwischen SDFs) | **T2** | K11 |
| `smin`‑Blends (weiche Booleans) | **T2** | K11 |
| Thread (analytisches Gewinde‑Primitiv) | T3 | K12 |
| Sweep (via Mesh‑to‑SDF, gemeinsamer Pfad mit Mesh‑Import) | T3 | K13 |
| STL/OBJ‑Import als Referenzgeometrie | T3 | K9/K13 |
| Loft | T3+ | (nicht in K1–K13) |
| Curved Pattern | T3+ | (nicht in K1–K13) |

**Cut‑Linie T1 → T2.** T1 endet bei K10 (Fillets). Alles darunter in der Tabelle — Gyroid, Displacement, Domain Deformation, Morph, `smin`‑Blends — ist explizit *nicht* Teil der ersten shippable Version. Das ist eine bewusste Entscheidung, keine Vertagung aus Zeitgründen: T1 liefert ein **brauchbares** CAD, T2 liefert ein **differenziertes** CAD, und die Reihenfolge ist nicht tauschbar, weil die Spatial‑Mask‑Infrastruktur aus K10 von K11's `smin`‑Blends wiederverwendet wird.

### 6.2 Sketch‑Editor (2D)

| Funktion | MVP |
|---|---|
| Multiselect | ✅ |
| Line, Rectangle, Circle, Ellipse, Polygon, Spline, Point | ✅ |
| Project / Intersect mit Part oder Linie | ✅ |
| Toggle Construction (Hilfslinien) | ✅ |
| Trim, Split, Offset | ✅ |
| Mirror | ✅ |
| Linear / Circular Pattern | ✅ |
| Transform | ✅ |
| Import / Export: DXF, SVG, PNG, JPG | ✅ |
| Auto‑Inferencing beim Zeichnen (mit Shift‑Unterdrückung) | ✅ |
| Driving vs. Driven Dimensions (Auto‑Konversion bei Über‑Constraint) | ✅ |
| Constraint Manager Panel (Liste, Filter, Hover‑Highlight) | ✅ |
| **Constraints:** | |
| Dimension (Länge, Winkel, treibend oder getrieben) | ✅ |
| Coincident | ✅ |
| Concentric | ✅ |
| Parallel | ✅ |
| Tangent | ✅ |
| Horizontal / Vertical | ✅ |
| Orthogonal | ✅ |
| Equal | ✅ |
| Midpoint | ✅ |
| Symmetric | ✅ |
| Constraint Removal | ✅ |
| Drei‑Farben‑Status (blau = unter‑, schwarz = voll, rot = über‑constrained) | ✅ |

### 6.3 UX‑Vereinfachungen gegenüber Onshape

- **Strikt parametrischer Workflow als kanonischer Pfad.** Die Ground Truth jeder Änderung ist der Feature‑Tree plus die §6.4 beschriebenen generischen Dialoge: Klick auf Tool → Dialog/Tool‑Panel öffnet sich → Eingabemodus → Bestätigen, Baum rechnet neu. Das ist der *immer verfügbare* Pfad für jedes Feature — auch dann, wenn der User parallel eine implizite Geste benutzt. Ein naives Push/Pull‑Gizmo, das einen Upstream‑Sketch‑ oder Extrude‑Parameter rückwärts auflöst, bleibt bewusst draußen: die Abbildung „gezogene Distanz → welches numerische Feld wächst?" ist im allgemeinen Fall mehrdeutig und in Multi‑Body‑Szenen instabil. Der Weg zu Direct‑Manipulation‑artigen Interaktionen läuft in FieldDraft stattdessen über §6.4 (Zweigleisige Bedienung) und über das *Erzeugen neuer Features* statt das Rückwärts‑Lösen bestehender — etwa Face‑Push/Pull als neues `FaceOffset`‑Feature im Tree statt als Parameter‑Edit eines Upstream‑Extrudes.
- **Onshape‑Parität beim Sketchen.** Die Sketch‑UX orientiert sich explizit an Onshape, nicht an FreeCAD oder SolveSpace. Vier Verhaltensweisen bilden den Kern (technische Umsetzung in `docs/ARCHITECTURE.md` §4.4, Adapter‑Schicht): (a) **Auto‑Inferencing** — beim Zeichnen schlägt die InferenceEngine passende Constraints proaktiv vor (Coincident, Horizontal/Vertical, Parallel, Tangent, …) und fügt sie beim Klick automatisch ein; Shift unterdrückt den Vorschlag. (b) **Driving vs. Driven Dimensions** — eine Dimension, die den Sketch über‑constrainen würde, wird automatisch als Driven (grau, Nur‑Anzeige) eingefügt statt einen Fehler zu werfen; ein Klick schaltet sie auf Driving. (c) **Drag‑Stabilität ohne Topologie‑Kippen** — beim Ziehen eines Punkts bleibt der Sketch in derselben topologischen Konfiguration, ein Rechteck invertiert sich nicht, ein Dreieck klappt nicht um. (d) **Drei‑Farben‑Status** — Entities und Constraints sind blau (unter‑constrained), schwarz (voll constrained) oder rot (im Konflikt); das Constraint‑Manager‑Panel listet alle Constraints, filtert nach Status und highlightet beim Hover die betroffenen Entities im Viewport. Diese vier zusammen sind der Unterschied zwischen „FreeCAD‑Feeling" und „Onshape‑Feeling" — und sie sind explizit als MVP‑Anforderung formuliert, nicht als Nice‑to‑Have.

### 6.4 Zweigleisige Bedienung — Dialoge und Gesten

FieldDraft fährt die Bedienung bewusst **zweigleisig**: Der Feature‑Tree plus die generischen Dialoge (K6a/K6a.1) sind die **kanonische, immer verfügbare Bedienoberfläche** für jedes Feature — Tool klicken, Dialog/Tool‑Panel erscheint, User füllt Parameter aus, bestätigt, Tree wächst. Das ist der Pfad, auf dem *jedes* Feature sicher, vollständig und reproduzierbar bedient werden kann, inklusive der mächtigen Randoptionen, die sich nicht gestisch abbilden lassen (End‑Condition‑Variationen, Second‑End‑Position, Draft‑Winkel, Merge‑Scope, …). Die vollständige Onshape‑Paritäts‑Analyse dazu steht in `onshape-ux.md`.

Über dieser kanonischen Schicht liegt ein **experimenteller, impliziter Bedienpfad**, der ausgewählte Basis‑Features über Gesten direkt im Viewport erreichbar macht — ohne den Dialog je zu ersetzen. Die Absicht ist, den Onshape‑typischen „mehrere Klicks, um einen Sketch anzufangen"‑Schmerz abzubauen: Doppelklick auf eine Face startet den Sketch auf dieser Face implizit, Klicken in den leeren Raum beginnt direkt ein parametrisches Primitiv, Ziehen eines Gizmos auf einer Boolean‑Komponente manipuliert den Transform des Sub‑Körpers live.

Zwei Regeln halten die beiden Gleise konfliktfrei:

1. **Gesten erzeugen oder manipulieren exakt dieselben `FeatureCall`‑Einträge wie Dialoge.** Es gibt keinen parallelen Datenpfad — jede Geste serialisiert letztlich einen Dialog‑Edit, nur eben ohne modalen Dialog. Ein gestengetriebenes Feature ist im Tree nachher von einem dialog‑getriebenen Feature nicht unterscheidbar.
2. **Der Dialog bleibt als Tool‑Panel auch während gestischer Bedienung sichtbar, kontext‑aware.** Wer per Geste einen Extrude angestoßen hat, sieht daneben weiterhin das vollständige Extrude‑Tool‑Panel mit allen Parametern und kann dort die Optionen feinjustieren, die die Geste nicht abdeckt. Das Tool‑Panel ist also nicht nur Backup für Feinjustage, sondern **permanente Ground Truth für den State**, den die Geste manipuliert.

Die Gesten‑UX wird **mitgedacht, aber nicht im ersten Meilenstein erzwungen**. Sie erscheint in der Roadmap als eigener experimenteller Track (`docs/ROADMAP.md`, K‑Exp1), nicht als Blocker für K6b oder K7.

### 6.5 Sketch‑Sub‑Feature‑Tree

Jeder Sketch hat in FieldDraft einen **eigenen, vollständigen Feature‑Tree im Hintergrund** — keine Blackbox, sondern ein zweiter `Vec<FeatureCall>`‑Spine pro Sketch. Jedes Sketch‑Werkzeug (Line, Rectangle, Circle, Arc, Point, Dimension, Trim, Mirror, Offset, …) ist ein eigener Feature‑Typ mit Schema (K6b), jeder Klick während des Zeichnens wird als `FeatureCall` in diesen Sub‑Spine geschrieben. Der Sub‑Tree wird im Outliner **optional aufklappbar** unter dem Sketch‑Knoten des Part‑Studio‑Spines dargestellt.

Das hat drei konkrete Konsequenzen:

1. **Undo/Redo pro Sketch‑Schritt.** Statt „alles oder nichts" wie in Onshape kann der User gezielt einzelne Zeichen‑Operationen rückgängig machen, ohne die restlichen Schritte zu verlieren.
2. **Re‑play und Inspektion.** Der Sub‑Tree ist git‑diffbar wie der Haupt‑Tree und erlaubt Features wie „Sketch‑Timeline‑Scrubbing" oder „Sketch‑Fehler‑Lokalisierung".
3. **Einheitliche Infrastruktur.** Sketch‑Werkzeuge laufen durch denselben `FeatureRegistry`, dieselbe Schema‑Validation, denselben generischen Dialog‑Generator wie Part‑Studio‑Features.

**Vorab‑Klarstellung zum Solver‑Paradigma:** Der Sketch‑Sub‑Feature‑Tree **ändert nichts an PlaneGCS**. Der Solver bleibt **deklarativ und simultan** — er bekommt weiterhin die vollständige Entity‑ und Constraint‑„Suppe" und löst sie global in einem Rutsch (DogLeg/SQP). Der Sub‑Tree ist **keine prozedurale Solver‑Semantik**, sondern ein reiner **Aufzeichnungs‑ und Wiedergabe‑Mechanismus** über einem unveränderten deklarativen Solver.

## 7. Daten und Dateiformate

- **Nativ:** **JSON‑Dokument** mit dem kompletten Feature‑Tree, den Sketches, Variablen und Metadaten. Enthält ein `schemaVersion`‑Feld für zukünftige Migrationen. Im MVP ausschließlich in `localStorage`; sobald das Backend existiert, wird dasselbe Dokument byte‑identisch an den Server gesynct. Menschenlesbar, git‑diffbar, ohne Spezial‑Tools inspizierbar.
- **Persistenz‑Schichten:**
   - **`localStorage`** — einzige Persistenz im MVP. Auto‑Save nach jedem Edit, Wiederherstellung beim Reload. Limit ca. 5–10 MB pro Origin; bei Überschreitung warnt das UI und bietet den Export als `.fdraft`‑Datei an.
   - **Dateisystem** — Download/Upload als `.fdraft`‑Datei für Export, Backup und manuellen Transfer.
   - **Backend‑DB** (spätere Phase) — kanonischer Stand nach Sync, plus History‑Snapshots.
- **Export:**
   - **STL** (Ascii/Binary) — primäres Druckformat.
   - **GLB** (glTF 2.0) — für Web‑Engines, Renderer, Three.js‑Integrationen wie BeamDraft.
   - **OBJ** — Wavefront‑Format für Standard‑Mesh‑Toolchains.
   - **SVG/DXF** — Zwei Export‑Pfade. (1) **Einzelne Skizze:** Rechtsklick auf einen Sketch‑Knoten → „Als DXF exportieren". Die Skizze liegt im 2D‑Constraint‑Solver ohnehin als exakte Linien, Kreisbögen und Splines vor, der Export ist verlustfrei — damit sind Lasercutter und Wasserstrahlschneider abgedeckt. (2) 2D‑Schnitte und Projektionen aus dem 3D‑Modell für Fertigungs‑Zeichnungen, grob, nicht normgerecht (siehe §12).
   - **3MF** — Phase 2, mit Mehrkörper‑Support und Farben.
   - **STEP (Mechanical Core Export, via OCCT‑Batch)** — Späterer Milestone (siehe `docs/ROADMAP.md` M14+). Exportiert wird nicht das sichtbare SDF‑Bauteil, sondern der **harte Teil des Feature‑Trees** — analytische Primitive, Extrusionen, Revolves, Transforms, harte Booleans. Der Kernel serialisiert diesen Sub‑Baum und übergibt ihn an einen **OpenCASCADE‑Web‑Worker**, der das Modell als echte NURBS‑B‑Rep nachbaut und als lupenreine STEP‑Datei schreibt. OCCT läuft nicht im Echtzeit‑Render‑Pfad — reiner Batch‑Prozessor, nur bei Klick auf „Export STEP" aktiv. Der OCCT‑Payload (20–30 MB WASM) wird ausschließlich in den Worker geladen. **Was fehlt:** `smin`‑Blends, Fillets/Chamfers aus Spatial Masks (degradieren zu scharfen Kanten), Gyroid/TPMS, Procedural Noise/Displacement, Domain Deformation, Morph, Mesh‑to‑SDF‑Knoten. Der STEP ist das „mechanische Skelett" — genug für CNC‑Fräsen, Normteil‑Referenzen, CAM‑Pipelines, aber nicht bitgenau das gedruckte Bauteil.
- **Import:**
   - **STL, GLB, OBJ** als **visuelle Referenzgeometrie** für COTS‑Teile. Import‑Verhalten in drei Stufen:
     1. **Import & Darstellung:** Standardmäßig rohes Mesh an Three.js — null Rechenaufwand, keine SDF‑Konvertierung.
     2. **Messen & Snapping:** Three.js‑Raycaster kennt jedes Dreieck. Maßband‑Tool und Sketch‑Snapping greifen darauf zu.
     3. **Boolean (Opt‑in, „Convert to Solid"):** Nur auf expliziten Wunsch wird das Mesh via Mesh‑to‑SDF (BVH‑basiert, Winding‑Number/Pseudo‑Normal‑Innentest) in ein Distanzfeld überführt und damit ein echter Feature‑Tree‑Knoten. Vorher läuft ein Wasserdichtigkeits‑Check.
   - **Derselbe Mesh‑to‑SDF‑Knoten trägt auch das Sweep‑Feature** aus §6.1: Ein Sweep entlang eines 3D‑Pfades mit Twist wird vom Kernel als wasserdichtes Dreiecks‑Mesh erzeugt und über diesen Pfad in den CSG‑Baum eingehängt. Import und Sweep teilen sich Code, BVH und Test‑Suite.
   - **DXF, SVG** in den Sketch‑Editor.
   - **STEP (Ausblick, nicht im MVP):** Ein echter STEP‑Import würde B‑Rep‑Daten in ein SDF umwandeln, was wasserdichte Eingangs‑Meshes voraussetzt — in der Praxis sind COTS‑STEP‑Dateien oft fehlerhaft. Außerdem bedeutet OpenCASCADE im WASM‑Payload 20–30 MB zusätzlich. Der Weg wird erst angegangen, wenn das MVP steht.

## 8. Datenhoheit, Deployment und Lizenz

- **Client‑Only im MVP:** Alle Berechnungen (CSG‑Auswertung, Mesher, Solver, Picking‑Raymarch) laufen im Browser, Persistenz nach `localStorage`. Es gibt keinen Server — das Tool kann vollständig als statisches Bundle ausgeliefert werden (GitHub Pages, Netlify, beliebiger Webserver). Ein Backend wird in einer späteren Phase ergänzt (siehe `docs/ARCHITECTURE.md` §4.7 und `docs/ROADMAP.md`).
- **Tauri als Escape Hatch:** Falls der Browser‑Kontext an harte Grenzen stößt — 2–4 GB WASM‑RAM, eingeschränktes Multithreading über Web Worker, WebGPU‑Reifegrad in Firefox — ist ein Port nach Tauri vorgesehen. Das React/R3F‑Frontend bleibt bitgenau erhalten, und der Rust‑Kernel zieht aus WASM in den nativen Rust‑Prozess um: voller Prozess‑RAM, echtes Multithreading via `rayon`, direkter `wgpu`‑Zugriff auf Vulkan/Metal/DirectX. Tauri statt Electron, weil es den nativen Webview des Betriebssystems nutzt (App‑Größe ca. 5–10 MB statt 150 MB). Der Umstieg kostet Code‑Duplizierung nur an der Persistenz‑Schicht; Kernel, Solver, Feature‑Tree und UI bleiben unverändert.
- **Client‑Heavy, Server‑Light:** Sobald der Server existiert, übernimmt er nur Auth, Projekt‑CRUD und Sync — keine Geometrie‑Dienste.
- **Self‑Hostable:** Ein einzelnes statisches Go‑Binary + MariaDB‑Container reicht. `docker-compose.yml` im Repo startet Server, DB und Web‑Frontend mit einem Befehl — siehe [`DEPLOY.md`](DEPLOY.md).
- **IP‑Schutz:** Projekte sind standardmäßig privat; der Sichtbarkeits‑Schalter muss explizit umgelegt werden.
- **OSS‑Readiness als Architektur‑Disziplin.** Unabhängig davon, wann und in welcher Form FieldDraft unter einer freien Lizenz veröffentlicht wird, wird der Kernel von Anfang an so gebaut, dass er **OSS‑fähig bleibt**: Keine proprietären Abhängigkeiten außer PlaneGCS (LGPL‑2.1+, siehe `docs/ARCHITECTURE.md` §4.4), keine Secrets oder API‑Keys im Repository, jede Drittbibliothek sauber attributiert, keine Telemetrie ohne Opt‑in.
- **Lizenz: offen.** Das Lizenzmodell, die Preisstrategie, die Zielgruppen‑Positionierung und alles andere mit kommerziellem Bezug sind bewusst nicht Gegenstand dieses technischen Dokuments und sind in `docs/BUSINESS.md` dokumentiert.

### 8.1 IP‑Hygiene (agent‑relevante Regeln)

- **Keine Onshape‑Markenbegriffe in Produkt‑ oder API‑Namen.** „FeatureScript" ist eine Marke von PTC/Onshape. Die JS‑API aus `docs/ARCHITECTURE.md` §4.6 heißt deshalb bewusst `fd.*`, nicht `featureScript.*`. Vergleichbare Markenbegriffe aus Onshape (Part Studio, Feature Studio, Mate Connector als Term) werden in produktsichtbarer UI und in der API vermieden oder durch generische Begriffe ersetzt. Intern in Dokumentation ist der Verweis auf Onshape‑Patterns als Referenz (siehe `onshape-ux.md`) erlaubt — aber die User‑sichtbaren Namen sind eigenständig.
- **Kein Quellcode aus Onshape, FreeCAD, SolveSpace, OpenCASCADE.** PlaneGCS wird als externes Modul geladen und bleibt über die LGPL‑Regeln klar abgegrenzt. OpenCASCADE wird als externes Modul im Web Worker betrieben, nicht als Code‑Kopie. Neue Drittbibliotheken werden beim Einziehen mit Lizenz in `THIRD_PARTY.md` (oder als `Cargo.toml`‑Kommentar) dokumentiert.
- **Konzepte sind Stand der Technik, nicht Onshape‑Erfindungen.** Feature‑Trees gibt es seit Pro/ENGINEER (1987), parametrische Sketcher mit Constraint‑Solvern seit den 1990ern. Die Architektur‑Konzepte übernehmen die **UX‑Philosophie** bewährter CAD‑Tools, nicht deren Code.
- **Originäre FieldDraft‑Architektur.** Der SDF‑Kernel mit Provenance‑Tracking, die Ghost Topology, die Spatial Masks für Fillets/Chamfers, die Dual‑Path‑Evaluation (visual vs. logical), das Mesh‑to‑SDF‑Pattern für Sweep und Import in einem Knoten — das alles kommt in keinem existierenden CAD‑Produkt so vor. Diese Teile sind originäre Architektur und gehören FieldDraft.

## 9. Teststrategie

Tests werden **von Anfang an** mitentwickelt, nicht nachgezogen. Die Pyramide:

- **Rust‑Unit‑Tests** (`cargo test`) für den Geometriekern: SDF‑Primitive (3D und 2D), harte Booleans, CSG‑Graph mit Provenance‑Auswertung, BVH‑Korrektheit, Dual‑Contouring‑Goldfiles (Mesh‑Hash gegen Referenz), Manifold‑Check des DC‑Outputs, Multi‑Body‑Isolation, B‑Rep‑Proxy‑Ableitung aus Provenance, Polyline‑Approximation von Splines, Serde‑Roundtrips des Projektformats.
- **Go‑Tests** (Backend‑Phase, `go test ./...`): Auth‑Flows, Projekt‑CRUD, Sichtbarkeits‑Transitionen, Quotas, Rate‑Limits, Sync‑Endpoints gegen eine echte MariaDB‑Instanz.
- **Solver‑Property‑Tests** (`proptest`): Zufallsgenerierte Sketches gegen invariante Eigenschaften.
- **TypeScript‑Unit‑Tests** (`vitest`) für Frontend‑Logik: Projekt‑Store, Auto‑Save‑Debouncing, `localStorage`‑Wrapper, UI‑Reducer. Komponententests für R3F‑Szenen via `@react-three/test-renderer`.
- **WASM‑Contract‑Tests:** TypeScript testet die WASM‑Bindings gegen dokumentierte Verträge, damit ABI‑Drift früh auffällt.
- **Playwright‑E2E‑Tests** (headed Chromium mit echtem WebGPU): Auth‑Flows, Projekt‑CRUD, Auto‑Save, Offline/Online‑Sync, Sketch‑Constraint‑Workflow, Extrude und STL‑Export (Download byteweise gegen Goldfile), Visual‑Regression‑Screenshots.
- **Lokaler Testlauf:** `just test` führt Rust‑Tests und Vitest aus; `just test-e2e` startet die Playwright‑Suite separat.

## 10. Roadmap

Die Roadmap ist in einem eigenen Dokument beschrieben: **[`docs/ROADMAP.md`](docs/ROADMAP.md)**. Dort liegen:

- **Release‑Tiers** T0–T3 und T∞ mit harten Shipping‑Linien
- **Kernel‑Phase K1–K14** mit Planungstexten pro Slice (inklusive K3.5)
- **Experimentelle Parallel‑Slices** (K‑Exp1 Skelett‑First‑Renderpfad)
- **Backend‑Phase M11–M13** (Auth, Projekt‑CRUD, Hardening)
- **Phase 3 P1–P5** (Assemblies, Kinematik, R3F‑Export, Physik, FEM)
- **Spätere Meilensteine M14+** (STEP, Loft, Collaboration)

Für den **Ist‑Zustand** (done/partial/open pro Slice, aktuelle Empfehlung, Test‑Zahlen) siehe **[`docs/STATUS.md`](docs/STATUS.md)**.

## 11. Offene Risiken

Die offenen architektonischen Risiken sind in **[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) §11** dokumentiert. 16 Punkte von Meshing‑Qualität über Provenance‑Epsilon‑Stabilität, Adapter‑Layer‑Komplexität und Feature‑Registry‑Refactor‑Tiefe bis zu WebAssembly‑Speicherlimits und WebGPU‑Browser‑Support.

## 12. Harte Architektur‑Grenzen (permanente Scope‑Ausschlüsse)

Die folgenden drei Limitierungen sind keine Risiken im Sinne von §11, sondern **permanente, bewusst akzeptierte Architektur‑Grenzen**. Sie ergeben sich aus der Entscheidung für einen SDF‑basierten Kern und sind in diesem Kern nicht auflösbar, ohne einen vollständigen B‑Rep/NURBS‑Zweitkernel danebenzustellen — was den USP des Systems zerstören würde. Wer diese Features braucht, braucht nicht FieldDraft.

1. **Auto‑Surfacing des organischen SDF‑Outputs nach NURBS.** Die globale Fertigungskette (Spritzguss, CNC‑Fräserei, Blechbearbeitung, Toleranz‑Management in Mastercam/Edgecam) spricht STEP mit exakten NURBS‑Oberflächen. Ein Algorithmus, der aus einem Dual‑Contouring‑Mesh (oder direkt aus dem SDF) wieder tangentenstetige NURBS‑Patches rekonstruiert, ist eines der schwersten ungelösten Probleme der Computergrafik. FieldDraft versucht diesen Weg **bewusst nicht**. Stattdessen existieren zwei strikt getrennte Export‑Pfade (§7): *WYSIWYG‑Mesh* (STL/3MF/GLB — alles Sichtbare, inklusive Gyroid, Noise, smooth Blends) und *Mechanical Core STEP* (via OCCT‑Batch‑Worker auf dem harten Teil des Feature‑Trees — nur Primitive, Extrusionen/Revolves und harte Booleans). Der Mechanical‑Core‑Export gibt dem Fräser genau das, was er braucht: exakte Bohrmittelpunkte, harte Kanten zum Einmessen, saubere Primitive in einem Format, das jedes CAM‑System liest. Die strategische Positionierung bleibt **Direct Digital Manufacturing** — additive Fertigung, SDF‑direktes CAM, Maker‑Workflows.

2. **Normgerechte technische Zeichnungsableitung.** Verträge, Qualitätskontrolle und Werkstatt‑Kommunikation in klassischen Branchen laufen zu 90 % über 2D‑Zeichnungen nach DIN ISO 128. Eine saubere 2D‑Ableitung setzt voraus, dass der Kernel Silhouetten als analytische Vektor‑Kurven aus dem CSG‑Baum extrahieren kann — theoretisch machbar, aber bei verrundeten `smin`‑Übergängen und Ghost‑Topology‑Edges kaum in saubere DIN‑konforme Vektor‑Konturen zu übersetzen. Grobe 2D‑Schnitte und Projektionen für Lasercutter sind weiterhin möglich (§7 DXF/SVG‑Export), normgerechte Fertigungs‑Zeichnungen nicht.

3. **ISO‑Toleranzen, Passungen, Metrologie.** In SolidWorks oder Onshape definiert man eine Welle nicht als `10 mm`, sondern als `10.000 mm h7` — eine Passungstoleranz, die in der Qualitätskontrolle gegen eine exakte theoretisch‑mathematische Referenzfläche mit einer eindeutigen, persistenten ID gemessen wird. Die Toleranz‑/Metrologie‑Welt verlangt referenzierbare Flächen mit GD&T‑Annotationen (Form‑, Lage‑, Richtungs‑Toleranzen), die über den gesamten Produktlebenszyklus bis zur Laserscanner‑Messung bestehen bleiben. Dieses Datenmodell ist nicht mit dem Provenance‑basierten Proxy vereinbar und wird in FieldDraft nicht nachgebaut. Für Maker‑, Prototyping‑ und 3D‑Druck‑Toleranzen reicht die numerische Präzision des Kernels; für Serienfertigung mit SPC‑Qualitätskontrolle ist das Tool nicht gedacht.

**Positionierung:** Diese drei Ausschlüsse sind keine Mängel, sondern die Kehrseite der Architektur‑Stärken (unkaputtbare Booleans, native TPMS/Gyroid/Displacement, SDF‑direktes CAM, R3F‑Code‑Export). Marketing und Doku sollten FieldDraft konsequent als **„CAD für Direct Digital Manufacturing"** positionieren — Maker, Hardware‑Startups, Robotik‑Entwickler, Industriedesigner, 3D‑Druck‑Pioniere — und die klassische Maschinenbau‑Welt (Motorenblöcke, Spritzgussformen, Dreh‑/Frästeile mit ISO‑Passungen) explizit **nicht** als Zielgruppe.

## 13. Verwandtes / Inspiration

- **Onshape** — UX‑Referenz, Feature‑Tree‑Vorbild.
- **Shapr3D** — Vorbild für „Direkt auf Fläche skizzieren" ohne expliziten Sketch‑Button.
- **Plasticity** — Geschwindigkeit des Direct Modeling, Negativbeispiel für UI‑Zwang.
- **PlaneGCS (FreeCAD)** — 2D‑Constraint‑Solver‑Basis (siehe `docs/ARCHITECTURE.md` §4.4). Numerischer Optimizer mit mehreren Algorithmen, reicher Diagnostik und nativem Drag‑Stabilitäts‑Mechanismus.
- **SolveSpace / libslvs** — UX‑Gegenbeispiel und verworfene Solver‑Wahl: solider algebraischer Kern, aber dokumentiertes Lösungs‑Sprung‑Problem und eng verzahnte UI.
- **Manifold** — moderne, robuste Mesh‑Booleans; als potenzielle Ergänzung zum SDF‑Pfad.
- **nTopology / nTop** — kommerzielles SDF/Implicit‑CAD der Luft‑ und Raumfahrt, Beleg dafür, dass der Ansatz industrietauglich ist.
- **MagicaCSG, Dreams (PS)** — kreative SDF‑Modeller als UX‑Inspiration.
- **Fornjot, Truck** — Rust‑native B‑Rep‑Kernel, als Fallback oder Ergänzung interessant.

## 14. Namensgebung und Schwesterprojekt

- **FieldDraft** — Distanz‑**Field**‑basiertes CAD‑**Draft**‑Tool.
- **BeamDraft** — eigenständiges Laser‑Projektor‑Projekt, das denselben 2D‑Constraint‑Solver als Kern nutzt. Durch die gemeinsame geometrische Engine entsteht ein strategischer Hebel: Ein investierter Entwicklungs‑Euro dient zwei Produkten.

## 15. Domains

Die TLDs .eu, .de und .xyz sind frei.
FieldDraft.com ist bereits registriert („for sale" bei GoDaddy).
Markenrechtlich gibt es keine Eintragungen.
Keine Github‑ oder NPM‑Projekte mit diesem Namen.

## 16. Literatur

Die kuratierte Pflichtlektüre pro Themengebiet (Dual Contouring, QEF, SDF‑Primitive 2D/3D, BVH, CSG‑Provenance, PlaneGCS, wgpu, wasm‑bindgen, Sharp‑Feature‑Preservation, Polyline‑Approximation, Adaptive Heightfield‑Triangulierung) steht in **[`docs/LITERATURE.md`](docs/LITERATURE.md)**. Wer einen K‑Slice anfasst, liest den relevanten Abschnitt dort **vorher**.
