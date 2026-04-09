# FieldDraft — Projektbeschreibung

## 1. Kurzfassung

**FieldDraft** ist ein browserbasierter, parametrischer 3D‑Modeller, der sich in Bedienung und Funktionsumfang an **Onshape** orientiert, technisch aber einen radikal anderen Weg geht: Statt eines klassischen B‑Rep/NURBS‑Kernels (OpenCASCADE, Parasolid, ACIS) setzt FieldDraft auf einen **SDF‑basierten Geometriekern** (Signed Distance Fields / Implicit Modeling).

Zielanwendung ist der **FDM/SLA‑3D‑Druck** in einem Bauraum von typischerweise bis zu 300 × 300 × 300 mm mit praxisrelevanter Präzision von ca. 0,1 mm.

FieldDraft ist bewusst **kein** vollständiger Onshape‑Ersatz für zerspanenden Maschinenbau, sondern ein **vereinfachter Onshape‑Clone** für den Maker‑ und Hardware‑Startup‑Workflow: genug Parametrik und Constraint‑Sketching, um Prototypen, Gehäuse, Halterungen und Mechanik‑Teile sauber zu konstruieren — ohne die Altlasten der CNC‑Welt mitzuschleppen.

Der Name **FieldDraft** beschreibt das Produkt wörtlich: ein **Draft**‑Tool (technisches Zeichnen / Konstruieren), das im **Field** (Distanzfeld) rechnet.

## 2. Motivation

Die zugrundeliegende Recherche (siehe `README.md`) hat gezeigt, dass es unter Linux für ein Hardware‑Startup mit Patent‑Ambitionen und schmalem Budget de facto kein ideales CAD gibt:

- **Onshape Free** — IP‑Vorveröffentlichung, damit patent­rechtlich toxisch.
- **Onshape Standard/Professional** — zu teuer in der Pre‑Seed‑Phase (~1.500 € / Nutzer / Jahr).
- **SolidWorks, Fusion, Alibre, Solid Edge** — kein nativer Linux‑Client.
- **Plasticity** — kein Light Mode / kein UI‑Scaling, keine Parametrik, keine 2D‑Ableitung.
- **FreeCAD / Ondsel** — UX‑Schwächen
- **BricsCAD Mechanical** — teurer Einmalkauf, eher klassisches CAD.

Gleichzeitig zeigt die Analyse der benötigten Funktionen, dass für den **3D‑Druck‑Use‑Case** die Schwächen klassischer B‑Rep‑Kernel (TNP, co‑planare Booleans, Fillet‑Edge‑Cases, Single‑Thread‑Performance) **strukturell vermieden** werden können, wenn man auf einen SDF‑Kern umschwenkt:

- Boolesche Operationen sind in SDFs trivial und mathematisch robust (`min`, `max`, `−`).
- Offsets sind eine einfache Konstanten‑Subtraktion `f(x) − d`.
- Raycasting erfolgt performant via Sphere Tracing direkt auf der Feldfunktion.
- Bei 0,1 mm Mesh‑Auflösung ist der „Potato‑Meshing“‑Effekt des Marching‑Cubes‑Algorithmus für FDM‑Druck praktisch irrelevant, da eine 0,4‑mm‑Düse ohnehin keine perfekt scharfen Kanten druckt.

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

FieldDraft besteht zunächst aus zwei Schichten: einem **Rust/WASM‑Geometriekern** im Browser und einem **React‑Three‑Fiber/WebGPU‑Frontend**. Die Verbindung zwischen Kernel und Frontend läuft über eine **Zero‑Copy‑Memory‑Brücke**. Ein **Go‑Backend‑Server** für Authentifizierung, Projektverwaltung und Synchronisation ist als spätere Phase vorgesehen — im MVP läuft alles rein clientseitig, der Projektzustand wird nach **`localStorage`** persistiert.

### 4.1 Rust/WASM‑Kernel (Backend im Browser)

Verantwortlich für alles, was Mathematik und Zustand ist — läuft ohne DOM‑Zugriff im Linear Memory von WebAssembly.

- **Feature‑Tree / Ground Truth:** Der Kernel speichert den parametrischen Baum aus Sketches, Extrusionen, Booleans, Patterns, Transformationen, Variablen und Parametern als **Genealogy Graph**. Kanten und Flächen werden **nicht** durchnummeriert, sondern über ihre Entstehungsgeschichte referenziert — das vermeidet das Topological Naming Problem (TNP) strukturell.
- **SDF‑Auswertung:** Jede Operation im Feature‑Tree trägt zu einer zusammengesetzten Distanzfunktion bei. Boolesche Operationen sind `min`/`max`/`−`, Patterns sind Modulo‑Transformationen des Raumes, Spiegelungen sind Vorzeichen­spiegelungen.
- **Lokale Modifikatoren (Fillets/Chamfers):** Realisiert über **parametrische SDF‑Masken**. Die Maske ist **keine** statische Box im Weltraum, sondern ein Feature‑Tree‑Knoten, dessen Geometrie zwingend an Vektoren der zugrundeliegenden Skizzen bzw. Extrusions‑Endpunkte gebunden ist. Wenn eine vorgelagerte Extrusion von 50 mm auf 100 mm wächst, wandert die Maske mit — sie rundet weiterhin die korrekte Kante ab, nicht plötzlich ein flaches Wandstück. Innerhalb der Maske wirkt `smin` (smooth minimum) für Fillets bzw. lineare Interpolation für Chamfers; außerhalb harte Booleans. Ein alternativer UX‑Modus („Kanten bemalen") ist angedacht, setzt aber eine robuste Kanten‑Identifikation voraus — siehe Risiko‑Abschnitt TNP.
- **Meshing:** Dual Contouring oder Surface Nets, um scharfe mechanische Kanten zu bewahren. Einzelne Körper werden **separat** gemesht — der Slicer kann überlappende Meshes selbst als einen Körper interpretieren, was Rechenzeit spart.
- **Export:** STL (Ascii/Binary), GLB (glTF 2.0), später STEP via Roundtrip‑Bridge oder einen dedizierten B‑Rep‑Exporter.
- **Konstanten‑Cache:** Da der Feature‑Tree die Ground Truth ist, kommt Undo/Redo praktisch geschenkt — man spult den Baum einen Schritt zurück und rechnet neu.

### 4.2 React‑Three‑Fiber‑Frontend (TypeScript)

Verantwortlich für Darstellung, Interaktion und Gizmos. Das UI wird komplett in **React** gebaut; die 3D‑Szene über **React Three Fiber** (`@react-three/fiber`) mit Unterstützung durch **`@react-three/drei`** für Helper (Kamera, Gizmos, Ansichts‑Würfel, Controls). R3F wickelt Three.js deklarativ in den React‑Komponentenbaum ein, sodass Viewport‑State, Outliner, Panels und Feature‑Tree dieselbe State‑Management‑Welt teilen.

- **Rendering:** React Three Fiber auf Three.js mit **WebGPU von Anfang an** (kein WebGL‑Fallback). Viewport, Beleuchtung, Materialien, Schatten.
- **Kamera‑Steuerung:** Orbit/Pan/Zoom, Ansichts‑Würfel oben rechts zum Ausrichten auf Standardansichten.
- **Raycasting:** Mausklicks treffen Geometrie; der Kernel liefert die zugehörige Feature‑ID aus dem Genealogy Graph zurück.
- **Gizmos:** Translate, Rotate, Scale und Mirror‑Achse für ganze Feature‑Tree‑Knoten. **Keine** Push/Pull‑Gizmos auf einzelnen SDF‑Flächen — der Workflow ist strikt parametrisch, geändert wird ausschließlich über Skizzen, Maße und die Parameter‑Tabelle (siehe §6.3).
- **Snapping:** Snaps fangen nicht auf dem (kartoffeligen) Mesh, sondern auf den **Vektordaten des Feature‑Trees** — Mittelpunkt eines Zylinders, Extrusions‑Punkt eines Sketches, Linienmittelpunkte etc.
- **WebGPU‑Raymarching:** Das SDF wird direkt per Fragment‑ bzw. Compute‑Shader in Echtzeit vorvisualisiert. Erst beim Export wird der teurere Mesher im Rust‑Kernel gestartet.
- **Auto‑Save im Browser:** Jede Änderung am Feature‑Tree erzeugt einen Dirty‑Marker; nach einer kurzen Debounce‑Phase (ca. 500 ms) wird der aktuelle Projektzustand als JSON in `localStorage` persistiert. Beim Seiten‑Reload wird der letzte lokale Stand automatisch wiederhergestellt. Ein späterer Sync an einen Backend‑Server ist vorgesehen, aber nicht Teil des MVPs — die `localStorage`‑Schicht ist zunächst die einzige Persistenz.

### 4.3 Memory‑Brücke

Keine JSON‑Serialisierung zwischen Rust und JS. Der Rust‑Mesher legt Vertices/Normals/Indices als fortlaufenden Block im WASM Linear Memory ab; JavaScript erzeugt einen `Float32Array`‑View direkt auf diesen Speicherbereich und übergibt ihn an eine `THREE.BufferGeometry`. Damit landet das Mesh ohne Kopiervorgang auf der GPU.

### 4.4 2D‑Constraint‑Solver

Der Solver ist das **verbindende Herzstück** und wird aus einem erprobten Open‑Source‑Projekt portiert:

- **Basis:** `libslvs` (SolveSpace) — algebraischer Solver mit Degree‑of‑Freedom‑Analyse, robust beim Mausziehen, kompakte Codebase, als Standalone‑Bibliothek entworfen.
- **Portierung:** C++ → Rust via moderner LLM‑unterstützter Transpilation, mit Unit‑Test‑Harness gegen die SolveSpace‑Referenz­implementierung. **Fallback‑Plan:** Sollte die Rust‑Portierung an numerischer Stabilität (Floating‑Point‑Epsilons, Konvergenz, Rundungsunterschiede) scheitern, wird `libslvs` stattdessen **nativ in C++ belassen** und gemeinsam mit dem Rust‑Kernel via Emscripten bzw. `cc`‑Crate in ein gemeinsames WASM‑Modul kompiliert. Das Interface zwischen Rust und C++ läuft dann über einen schmalen FFI‑Layer — kein Perfektionismus, aber robust.
- **Alternative/Ergänzung:** Wenn B‑Splines/NURBS‑Constraints später relevant werden, kommt ein iterativer Solver im Stil von PlanarGCS (FreeCAD) als zweite Strategie dazu — zerlegt via Graphen‑Dekomposition in starre Untergruppen.
- **Mehrfachnutzung:** Derselbe Rust‑Solver wird zusätzlich im Schwesterprojekt **BeamDraft** (laser‑projektions­basiertes Tool) eingesetzt.

### 4.5 Variablen und Formel‑Engine

Variablen sind **keine** bloßen Zahlwerte, sondern vollwertige **Formeln** — in UX und Mächtigkeit am Onshape‑Variablen­system (FeatureScript‑artige Expressions) orientiert. Jede Variable, jede Bemaßung und jeder numerische Parameter im Feature‑Tree kann durch einen Ausdruck statt einer Konstante definiert werden.

- **Expression‑Sprache:** Arithmetik (`+ − × ÷ ^`), Vergleichs‑ und Logikoperatoren, Funktionsbibliothek (`sin`, `cos`, `tan`, `asin`, `acos`, `atan`, `atan2`, `sqrt`, `abs`, `floor`, `ceil`, `round`, `min`, `max`, `mod`, `if`/ternär, `pi`, `e`). Referenzen auf andere Variablen per Name (`radius = diameter / 2`). Keine Schleifen, keine Seiteneffekte — rein funktional, damit die Re‑Evaluation deterministisch und zyklensicher bleibt.
- **Einheiten‑System (unit‑safe):** Werte tragen Einheiten (`mm`, `cm`, `m`, `in`, `ft`, `deg`, `rad`, dimensionslos). Der Parser prüft Dimensionskonsistenz zur Compile‑Zeit des Ausdrucks: `width + 30°` wird abgelehnt, `width + 5 mm` akzeptiert. Winkel‑Funktionen nehmen `deg`/`rad` korrekt entgegen. Das Ergebnis eines Ausdrucks ist immer eine typisierte Quantity, nicht nur ein Float.
- **Parser & Interpreter im Rust‑Kernel:** Die Expression wird im Kernel geparst (handgeschriebener Recursive‑Descent oder `pest`/`chumsky`), in einen typ‑ und einheitsgeprüften AST überführt und bei jedem Re‑Evaluate des Feature‑Trees ausgewertet. Keine JavaScript‑Eval‑Fallgrube. Fehler (Syntax, unbekannter Identifier, Zirkelreferenz, Einheiten‑Mismatch, Division durch Null) werden als strukturierte Diagnostics zurückgegeben und im UI am Ort der fehlerhaften Variable hervorgehoben.
- **Abhängigkeitsgraph:** Variablen bilden einen DAG; der Kernel topologisch‑sortiert vor jedem Rebuild und erkennt Zyklen (`a = b + 1`, `b = a * 2`) sofort. Änderungen propagieren nur durch die betroffenen Downstream‑Knoten, nicht durch den gesamten Baum.
- **UX‑Äquivalenz zu Onshape:** Variablen‑Panel mit Liste aller Variablen, Live‑Validierung beim Tippen, Inline‑Fehleranzeige, Autovervollständigung anderer Variablen­namen, Einheiten‑Anzeige rechts vom Wert. Direkt in Maßfeldern im Sketch‑Editor sind Formeln erlaubt: `=width/2 + 5 mm` statt nur `15`. Das ist das zentrale Werkzeug, um aus einem Einzelteil eine ganze Bauteil‑Familie zu machen (Schrankhöhe variieren, Motor‑Lochbild skalieren, Linsenradius an Fassung koppeln).
- **Serialisierung:** Im JSON‑Dokument stehen Variablen als `{ name, expression, unit? }`. Der evaluierte Wert wird **nicht** mitgespeichert — er entsteht beim Laden durch erneute Auswertung, was die Dokumente sauber diffbar hält.
- **Tests:** Expression‑Parser bekommt eine umfangreiche `proptest`‑Suite für Roundtrip (`parse→print→parse`), Einheiten‑Arithmetik, Fehler­fälle und Zyklenerkennung. Goldfiles für kanonische Formeln aus dem Onshape‑Vokabular.

### 4.6 Backend‑Server (spätere Phase, nicht im MVP)

> **Hinweis:** Das MVP läuft komplett clientseitig, persistiert wird ausschließlich in `localStorage`. Ein Backend‑Server wird erst nach Abschluss des Modelliers‑Kerns (siehe Roadmap, nach M8) eingeführt. Der folgende Abschnitt beschreibt die geplante Architektur dieser späteren Phase — inklusive MariaDB, Auth und Projekt‑CRUD.

Der Server wird bewusst schlank gehalten und hat nur wenige Aufgaben:

- **Tech‑Stack:** Go mit `chi` als HTTP‑Router (alternativ `net/http` direkt), `sqlc` für Compile‑Time‑geprüfte SQL‑Queries, `go-sql-driver/mysql` als Datenbank‑Treiber, `golang-migrate` für Schema‑Migrationen. **MariaDB in allen Umgebungen** — lokal für Dev in einem Docker‑Container (via `docker-compose up db`), in Production als Managed‑MariaDB/MySQL oder ebenfalls containerisiert. Dev/Prod‑Parität ohne DB‑Sonderfall.
- **Warum Go (und nicht Rust):** Der Server berechnet keine Geometrie, macht kein Meshing, kein WASM — die Rust‑Stärken werden hier nicht gebraucht. Go bietet dagegen deutlich schnellere Build‑ und Test‑Zyklen, ein pflegeleichteres Standard‑Ökosystem für HTTP/Auth/Sessions, eingebautes `pprof`‑Profiling und ein statisches Single‑Binary‑Deployment. Der Wire‑Contract zum Kernel läuft über ein JSON‑Schema, das aus dem Rust‑`protocol`‑Crate generiert wird — so können beide Seiten dieselbe Struktur typ‑scharf lesen, und der Server kann das Projektdokument für Migrationen, Suche und Thumbnails bei Bedarf sogar interpretieren.
- **Authentifizierung:** Klassisches E‑Mail + Passwort mit `argon2id`‑Hashing (`golang.org/x/crypto/argon2`). Session‑Cookies (HttpOnly, Secure, SameSite=Lax) anstelle von JWT, um CSRF/Replay‑Probleme zu minimieren. Optional OAuth (GitHub, Google) in einer späteren Phase.
- **User‑Modell:** `users(id, email, password_hash, display_name, created_at, last_seen_at)`.
- **Projekt‑CRUD (pro User):** `projects(id, owner_id, name, slug, visibility, document_blob, updated_at, created_at, size_bytes)`. Endpunkte: `POST/GET/PATCH/DELETE /api/projects`, `POST /api/projects/:id/snapshot` für Auto‑Save‑Synchronisation.
- **Sichtbarkeit:** Jedes Projekt hat einen Schalter `visibility ∈ {private, public}`. Private Projekte sind nur für den Owner sichtbar und werden nie öffentlich indiziert. Öffentliche Projekte erscheinen in einer durchsuchbaren Galerie und sind read‑only für Nicht‑Owner. Der Wechsel ist jederzeit umkehrbar.
- **Storage:** Das Projektdokument ist **JSON** (UTF‑8). Im Kernel via `serde_json` serialisiert, im Server als `JSON`‑Spalte in MariaDB gespeichert (intern LONGTEXT mit Validierung). Vorteile: menschenlesbar und direkt diffbar (auch in `git`), debuggbar ohne Spezial‑Tools, indizierbare Metadaten via **Generated Columns** + regulären Indizes (z. B. `name`, Feature‑Count, verwendete Node‑Typen aus dem Dokument extrahiert), saubere Schema‑Migrationen per Versionsfeld. Kompression passiert — falls nötig — transparent auf HTTP‑Ebene via `Content-Encoding: gzip`, nicht im Speicherformat. Kein Filesystem‑Zugriff, alles in der DB — vereinfacht Backups und Self‑Hosting drastisch.
- **Rate‑Limits:** Per IP und per User, um Auto‑Save‑Loops nicht zum Denial‑of‑Service werden zu lassen.
- **Quotas:** Pro User ein weiches Limit (z. B. 100 Projekte, 50 MB pro Projekt) — später über Rollen konfigurierbar.

## 5. User‑ und Projektverwaltung (spätere Phase)

> **Hinweis:** User‑Accounts, ein serverseitiges Dashboard und öffentliche Galerien sind ebenfalls erst Teil der Backend‑Phase (nach M8). Im MVP gibt es genau einen impliziten „lokalen Workspace" im Browser, dessen gesamter Zustand in `localStorage` liegt. Die folgende Spezifikation beschreibt den Zielzustand, nicht den MVP‑Stand.

### 5.1 Accounts

- **Registrierung:** E‑Mail + Passwort, optional Verifikations‑E‑Mail in Phase 2.
- **Login / Logout:** Session‑basiert, Cookies.
- **Passwort‑Reset:** via E‑Mail‑Token (Phase 2).
- **Gast‑Modus:** Ohne Login nutzbar — das Projekt existiert dann nur lokal in `localStorage`. Beim ersten Login‑Vorgang wird optional angeboten, das lokale Projekt in den Account zu übernehmen. (Im MVP ist der Gast‑Modus faktisch der einzige Modus, da kein Server existiert.)

### 5.2 Projekt‑Dashboard

Nach dem Login landet der User auf einem Dashboard mit:

- Liste aller eigenen Projekte (Thumbnail, Name, letzte Änderung, Sichtbarkeit).
- Button „Neues Projekt“.
- Umbenennen, Duplizieren, Löschen.
- Suche und Filter (privat/öffentlich, zuletzt geändert).
- Galerie öffentlicher Projekte anderer User (nur Leserechte).

### 5.3 Sichtbarkeit und IP‑Schutz

Weil jedes Projekt standardmäßig **privat** angelegt wird, ist FieldDraft im Gegensatz zu Onshape Free patentrechtlich unkritisch — der User muss aktiv auf „öffentlich“ umschalten, um etwas zu publizieren. Private Projekte liegen verschlüsselt auf dem Server (At‑Rest‑Encryption auf DB‑Ebene); eine optionale Ende‑zu‑Ende‑Verschlüsselung mit user‑gehaltetem Schlüssel ist als Phase‑3‑Feature eingeplant.

## 6. Funktionsumfang (Feature‑Parität zu Onshape)

Die UI‑Referenz orientiert sich eng an den Onshape‑Screenshots (`assets/onshape-part-editor.png`, `assets/onshape-sketch-editor.png`).

### 6.1 Part‑Editor (3D)

| Funktion | MVP | Later |
|---|---|---|
| Feature‑Tree | ✅ | |
| Part‑List / Outliner | ✅ | |
| Multiselect | ✅ | |
| Maßband / Measure | ✅ | |
| Kamera‑ und Render‑Optionen | ✅ | |
| Ansichts‑Würfel | ✅ | |
| Undo / Redo (über Feature‑Tree) | ✅ | |
| Sketch (Einstieg in 2D) | ✅ | |
| Extrude (Add, Subtract, Intersect, New) | ✅ | |
| Revolve | ✅ | |
| Boolean (Union, Difference, Intersect) | ✅ | |
| Linear Pattern | ✅ | |
| Circular Pattern | ✅ | |
| Mirror | ✅ | |
| Transform (Move/Rotate/Scale) | ✅ | |
| Delete | ✅ | |
| Plane (Referenzebenen) | ✅ | |
| Variable / Expressions | ✅ | |
| Mate Connector (als Ankerpunkt, nicht Kinematik) | ✅ | |
| Fillet (parametrisch, via SDF‑Masken an Skizzen gebunden) | ❌ | Phase 2 |
| Chamfer (parametrisch, via SDF‑Masken an Skizzen gebunden) | ❌ | Phase 2 |
| Sweep | ❌ | Phase 2 |
| Loft | ❌ | Phase 2 |
| Curved Pattern | ❌ | Phase 2 |

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
| **Constraints:** | |
| Dimension (Länge, Winkel) | ✅ |
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
| Error Display (over/under‑constrained) | ✅ |

### 6.3 UX‑Vereinfachungen gegenüber Onshape

- **Kein expliziter „Neue Skizze“‑Button.** Statt User zu zwingen, zuerst eine Ebene zu definieren, reicht es, das Sketch‑Tool zu aktivieren und direkt auf eine 3D‑Fläche zu klicken. Der Raycast liefert Schnittpunkt und Normale; Kernel und Frontend bauen die Sketch‑Ebene automatisch im Hintergrund (Vorbild: Shapr3D).
- **Strikt parametrischer Workflow.** Änderungen laufen **ausschließlich** über Skizzen, Bemaßungen und Variablen — bewusst kein Direct‑Manipulation‑Modus im MVP. Der Grund: In einem SDF gibt es mathematisch keine Flächen, Kanten oder Ecken, sondern nur ein Distanzfeld. Push/Pull auf „Flächen" würde eine robuste Topologie‑Extraktion aus dem Feld erfordern, inklusive einer Feature‑ID‑Berechnung im Raymarcher und eines konsistenten Rückwegs in den Feature‑Tree. Das ist lösbar, aber nicht MVP‑würdig. Stattdessen wird das Onshape‑Modell zitiert: Skizze → Extrusion → Maß ändern → Baum rechnet neu. Ein Direct‑Mode kann in einer späteren Phase nachgeliefert werden, sobald die Feature‑ID‑Infrastruktur im Shader existiert.
- **Respekt vor Augen und Tastatur:** vollwertiger Light/Dark/High‑Contrast‑Mode, Respekt der Default-Fontsize, konfigurierbare Keybindings. Das sind bewusste Lehren aus dem Plasticity‑Debakel im Kontext‑Chat.

## 7. Daten und Dateiformate

- **Nativ:** **JSON‑Dokument** mit dem kompletten Feature‑Tree, den Sketches, Variablen und Metadaten. Enthält ein `schemaVersion`‑Feld für zukünftige Migrationen. Wird im MVP ausschließlich in `localStorage` gehalten; sobald das Backend existiert, wird dasselbe Dokument byte‑identisch an den Server gesynct. Menschenlesbar, git‑diffbar, ohne Spezial‑Tools inspizierbar — ein bewusst wichtiger Vorteil gegenüber Binärformaten anderer CAD‑Tools.
- **Persistenz‑Schichten:**
   - **`localStorage`** — einzige Persistenz im MVP. Auto‑Save nach jedem Edit, Wiederherstellung beim Reload. Achtung: `localStorage` ist auf typischerweise 5–10 MB pro Origin limitiert; bei Überschreitung warnt das UI und bietet den Export als `.fdraft`‑Datei an. Ein Umstieg auf IndexedDB ist später nahtlos möglich, falls die Größenbudgets nicht mehr reichen.
   - **Dateisystem** — Download/Upload als `.fdraft`‑Datei für Export, Backup und manuellen Transfer.
   - **Backend‑DB** (spätere Phase) — kanonischer Stand nach Sync, plus History‑Snapshots.
- **Export:**
   - **STL** (Ascii/Binary) — primäres Druckformat.
   - **GLB** (glTF 2.0) — für Web‑Engines, Renderer, Three.js‑Integrationen wie BeamDraft.
   - **SVG/DXF** — 2D‑Schnitte und Projektionen für Lasercutter.
   - **3MF** — Phase 2, mit Mehrkörper‑Support und Farben.
   - **STEP (B‑Rep‑Export)** — Phase 3, über einen dedizierten B‑Rep‑Rekonstruktionspass auf den gemeshten/impliziten Daten.
- **Import:**
   - **STL, GLB, OBJ** als **visuelle Referenzgeometrie** für COTS‑Teile (Motoren, Platinen, Sensoren). Import‑Verhalten in drei Stufen:
     1. **Import & Darstellung:** Standardmäßig wird das Mesh roh an Three.js übergeben und auf die GPU geschoben — null Rechenaufwand, keine SDF‑Konvertierung, funktioniert auch mit kaputten Meshes. Ziel: Der User sieht das Teil, ohne dass das System crashen kann.
     2. **Messen & Snapping:** Obwohl das Mesh nicht Teil des SDF‑Kernels ist, kennt der Three.js‑Raycaster jedes Dreieck, jede Kante, jeden Vertex. Maßband‑Tool und Sketch‑Snapping greifen darauf zu und schicken die getroffenen Koordinaten als feste Vektoren an den Rust‑Kernel. Für den User fühlt sich das Referenzmesh normal an.
     3. **Boolean (Opt‑in, „Convert to Solid"):** Nur auf expliziten Wunsch wird das Mesh via Mesh‑to‑SDF (BVH‑basiert) in ein Distanzfeld überführt und damit ein echter Feature‑Tree‑Knoten, der in Booleans teilnehmen kann. Vorher läuft ein Wasserdichtigkeits‑Check (Manifold‑Prüfung); bei kaputten COTS‑Meshes warnt das UI, bevor Artefakte entstehen. Rationale: COTS‑Downloads von Thingiverse/GrabCAD haben oft Self‑Intersections, Zero‑Thickness‑Flächen und umgedrehte Normalen — ein pauschales Mesh‑to‑SDF bei jedem Import würde den WASM‑Speicher mit fehlerhaften Feldern fluten. Außerdem zieht man in der Praxis fast nie eine ganze COTS‑Platine als „Stempel" ab, sondern baut einfache Aussparungen drumherum.
   - **DXF, SVG** in den Sketch‑Editor.
   - **STEP (Ausblick, nicht im MVP):** Ein echter STEP‑Import würde B‑Rep‑Daten in ein SDF umwandeln (Mesh‑to‑SDF), was wasserdichte Eingangs‑Meshes voraussetzt — in der Praxis sind COTS‑STEP‑Dateien oft fehlerhaft. Außerdem bedeutet OpenCASCADE im WASM‑Payload 20–30 MB zusätzlich. Der Weg wird erst angegangen, wenn das MVP steht. Bis dahin lautet der Workaround: STEP extern (z. B. in FreeCAD) zu STL/OBJ konvertieren und als Referenz laden.

## 8. Datenhoheit, Deployment und Lizenz

- **Client‑Only im MVP:** Alle Berechnungen (SDF, Mesher, Solver, Raymarching) laufen im Browser, Persistenz nach `localStorage`. Es gibt keinen Server — das Tool kann vollständig als statisches Bundle ausgeliefert werden (GitHub Pages, Netlify, beliebiger Webserver). Ein Backend wird erst in einer späteren Phase ergänzt (siehe §4.6 und Roadmap).
- **Client‑Heavy, Server‑Light (spätere Phase):** Sobald der Server existiert, übernimmt er nur Auth, Projekt‑CRUD und Sync — keine Geometrie‑Dienste, damit die Cloud‑Kosten minimal bleiben und Self‑Hosting trivial wird.
- **Self‑Hostable (spätere Phase):** Ein einzelnes statisches Go‑Binary + MariaDB‑Container reicht. `docker-compose.yml` im Repo startet dann Server, DB und Web‑Frontend mit einem Befehl.
- **IP‑Schutz:** Projekte sind standardmäßig privat; der Sichtbarkeits‑Schalter muss explizit umgelegt werden, um etwas zu publizieren. Damit ist FieldDraft — anders als Onshape Free — für Hardware‑Startups mit Patent‑Ambitionen unbedenklich.
- **Lizenzmodell:** Offen (noch zu entscheiden: AGPL oder Apache‑2.0 Core + kommerzielle Cloud‑Komponente).

## 9. Teststrategie

Tests werden **von Anfang an** mitentwickelt, nicht nachgezogen. Die Pyramide sieht so aus:

- **Rust‑Unit‑Tests** (`cargo test`) für den Geometriekern: SDF‑Primitive, Booleans, Feature‑Tree‑Evaluation, Genealogy‑Graph, Meshing‑Algorithmen, Serde‑Roundtrips des Projektformats. Numerische Vergleiche mit Referenztabellen.
- **Go‑Tests** (spätere Backend‑Phase, `go test ./...`): Auth‑Flows, Projekt‑CRUD, Sichtbarkeits‑Transitionen, Quotas, Rate‑Limits, Sync‑Endpoints gegen eine echte MariaDB‑Instanz. `httptest.Server` für die HTTP‑Schicht, `testify` für Assertions, Table‑Driven Tests als Standard. Im MVP noch nicht aktiv, da kein Backend existiert.
- **Solver‑Property‑Tests** (`proptest`): Zufallsgenerierte Sketches werden gegen invariante Eigenschaften geprüft (z. B. „nach Setzen eines Parallel‑Constraints liegen die Liniensteigungen innerhalb ε identisch“).
- **TypeScript‑Unit‑Tests** (`vitest`) für Frontend‑Logik: Projekt‑Store, Auto‑Save‑Debouncing, `localStorage`‑Wrapper, UI‑Reducer, Command‑Dispatcher. Komponententests für R3F‑Szenen via `@react-three/test-renderer`.
- **WASM‑Contract‑Tests:** TypeScript testet die WASM‑Bindings gegen dokumentierte Verträge (Pointer‑Typen, Memory‑Layouts), damit ABI‑Drift früh auffällt.
- **Playwright‑E2E‑Tests:** End‑to‑End‑Szenarien im echten Browser (Chromium mit WebGPU‑Flag):
   - Registrierung, Login, Logout, Passwort falsch.
   - Projekt anlegen, umbenennen, duplizieren, löschen.
   - Sichtbarkeit umschalten und in der öffentlichen Galerie verifizieren.
   - Auto‑Save: Änderung machen, Tab neu laden, Zustand prüfen.
   - Offline/Online‑Sync: Netzwerk mocken, Edits machen, wieder online gehen, Server‑Stand prüfen.
   - Sketch‑Constraint‑Workflow: Rechteck zeichnen, bemaßen, Maß ändern, Viewport‑Snapshot.
   - Extrude und STL‑Export: Download‑Artefakt byteweise gegen Goldfile prüfen.
- **Visual‑Regression** via Playwright‑Screenshots für kritische Viewport‑States (Rohbauteil, gemeshtes Bauteil, Raymarching‑Preview).
- **Lokaler Testlauf:** Ein einzelner Befehl `just test` führt `cargo test` (Kernel), Vitest und Playwright nacheinander aus. Pro Commit grün oder nicht gemerged. Eine CI‑Pipeline (GitHub Actions o. ä.) wird bewusst erst aufgesetzt, sobald das Projekt auf GitHub landet.
- **Test‑Fixtures:** Ein Satz kanonischer Beispielprojekte (`fixtures/`) als Goldfiles für Regression und Bootstrapping neuer Tests.

## 10. Roadmap

Die Roadmap ist darauf ausgelegt, dass ich als Pair‑Programming‑Agent die Umsetzung in kleinen, testbaren Vertical Slices vorantreiben kann. Jeder Meilenstein endet mit grünen Tests, einem lauffähigen Artefakt und einem nachvollziehbaren Commit‑Verlauf. Keine „Big Bang“‑Phasen.

**Meilenstein M0 — Repo und Testgerüst**
- Lokales Git‑Repo (kein GitHub, kein Remote in dieser Phase).
- Monorepo mit `crates/kernel` (Rust→WASM), `crates/protocol` (Rust, JSON‑Schema‑Export), `web/` (Vite + TypeScript + React + React Three Fiber). Der `server/`‑Ordner wird erst in der Backend‑Phase angelegt.
- Build‑Tooling: `just`, `wasm-pack`, `npm` (kein pnpm).
- Vitest, Playwright und `cargo test` von Commit 1 an eingerichtet, lokal lauffähig via `just test`.
- `just dev` startet das Web‑Frontend mit einem Befehl — keine Datenbank, kein Docker‑Compose, keine CI‑Pipeline im MVP.

**Meilenstein M1 — WebGPU‑Viewport und erste Primitive**
- React Three Fiber mit WebGPU‑Renderer, Kamera, Ansichts‑Würfel (via `@react-three/drei`).
- Kernel liefert feste Primitive (Box, Sphere, Cylinder) als SDF.
- Raymarching‑Shader rendert das zusammengesetzte Feld in Echtzeit.
- Zero‑Copy‑Memory‑Brücke für SDF‑Parameter.
- Debounced Auto‑Save des Projektzustands nach `localStorage`; Wiederherstellung beim Reload.
- Snapshot‑Tests: gerendertes Bild gegen Goldfile.

**Meilenstein M2 — Feature‑Tree und Booleans**
- Feature‑Tree‑Datenstruktur im Kernel, Serde‑Roundtrip‑Tests.
- Boolean‑Nodes (Union, Difference, Intersect), Transform‑Nodes.
- Undo/Redo über Tree‑Navigation.
- UI: Part‑List/Outliner, Multiselect.
- E2E‑Test: zwei Primitive erstellen, subtrahieren, Ergebnis visuell prüfen.

**Meilenstein M3 — 2D‑Constraint‑Solver**
- Portierung von `libslvs` nach Rust, Test‑Harness gegen C‑Referenz. Bei Stabilitätsproblemen Fallback auf `libslvs` in C++ via Emscripten/`cc` (siehe §4.4).
- Auf jeden Fall benötigte Constraints: Coincident, Parallel, Horizontal/Vertical, Dimension (Länge, Winkel), Tangent, Concentric, Equal, Midpoint, Symmetric.
- DoF‑Analyse und strukturierte Diagnostics (over/under‑constrained).
- Property‑Tests für invariante Eigenschaften.
- Headless‑Smoke‑Test: programmatisch aufgebaute Sketches werden gelöst, ohne dass UI existiert.
- Portierung auch der Testfälle aus solvespace/test. Die portierten Tests müssen fehlerfrei laufen.

**Meilenstein M4 — 2D‑Sketcher (UI auf dem Solver)**
- Sketch‑Ebene via Klick auf Fläche.
- Werkzeuge: Line, Rectangle, Circle, Point, Trim, Mirror, Offset.
- Vektor‑basiertes Snapping auf Kernel‑Feature‑Daten.
- Live‑Einbindung des in M3 portierten Solvers: Bemaßungen und Constraints wirken direkt beim Zeichnen und beim Dragging.
- Fehleranzeige für over/under‑constrained Sketches am betroffenen Element.
- Playwright‑Tests für Zeichenwerkzeuge (Canvas‑Koordinaten‑Assertions).

**Meilenstein M5 — Extrude, Revolve, Meshing, STL‑Export**
- Extrude (Add, Subtract, Intersect, New), Revolve.
- Dual Contouring / Surface Nets im Kernel.
- STL‑Ascii‑ und Binary‑Export, Goldfile‑Tests.
- GLB‑Export via `gltf`‑Crate.
- Vollständiger E2E‑Workflow: Rechteck → extrudieren → exportieren → Download‑Datei verifizieren.

**Meilenstein M6 — Patterns, Mirror, Variablen**
- Linear/Circular Pattern, Mirror als Feature‑Tree‑Knoten.
- Vollwertige Formel‑Engine im Kernel (siehe §4.5): Expression‑Parser mit AST, Einheiten‑System (`mm`, `deg`, …), Funktionsbibliothek (`sin`, `sqrt`, `min`, …), Referenzen auf andere Variablen, Zyklen‑Erkennung, strukturierte Fehler‑Diagnostics. Maßfelder im Sketch‑Editor akzeptieren Formeln direkt (`=width/2 + 5 mm`).
- `proptest`‑Suite für Parser‑Roundtrip, Einheiten‑Arithmetik und Zyklen; Goldfiles für kanonische Onshape‑ähnliche Ausdrücke.
- Abhängigkeitsgraph‑basierte Re‑Evaluation des Baumes bei Variablen‑Änderung (nur betroffene Downstream‑Knoten).
- E2E‑Test: parametrisches Schrankelement aus den Assets.

**Meilenstein M7 — Parametrische Fillets und Chamfers**
- Feature‑Tree‑Knoten für Fillet/Chamfer, dessen SDF‑Maske zwingend an Skizzen‑Vektoren bzw. Extrusions‑Endpunkte gebunden ist.
- Re‑Evaluation der Maske bei Änderungen vorgelagerter Features, inklusive Goldfile‑Tests, die eine 50→100 mm‑Extrusion mit gleichbleibender abgerundeter Kante verifizieren.
- Kein Direct‑Modeling — die Fillet‑Kante wird durch Klick im Sketch bzw. Auswahl einer Feature‑ID referenziert.

**Meilenstein M8 — Referenz‑Import, Measure, Mate Connector**
- **STL/OBJ‑Import als visuelle Referenzgeometrie** (keine SDF‑Konvertierung, keine Booleans gegen das Referenzmesh).
- GLB‑Import für Demo‑ und Galerie‑Zwecke.
- Maßband‑Tool, Mate‑Connector als Ankerpunkt für Skizzen.
- Visual‑Regression‑Suite für den gesamten Part‑Editor.

---

*Ab hier beginnt die Backend‑Phase. Der clientseitige Modeller ist an diesem Punkt funktional komplett; die folgenden drei Meilensteine (M9–M11) ergänzen Server, Accounts und Sync, bevor M12 den Hardening‑Pass durchführt.*

**Meilenstein M9 — Backend‑Grundgerüst: Auth, User, leeres Projekt**
- `server/`‑Ordner mit Go + `chi` + `sqlc` + `golang-migrate` anlegen.
- `docker-compose.yml` mit MariaDB (Dev‑DB, persistiertes Volume), Server und Web. `just dev` startet jetzt den kompletten Stack.
- `users`‑Tabelle, Argon2id‑Hashing, Session‑Cookies.
- Endpunkte: `POST /api/auth/register`, `/login`, `/logout`, `GET /api/me`.
- Frontend: Registrierungs‑, Login‑, Logout‑Views (React‑Komponenten).
- `go test ./...` gegen echte MariaDB im CI; Integration‑Tests für Auth‑Flows, E2E‑Tests für die UI‑Pfade.

**Meilenstein M10 — Projekt‑CRUD und Sichtbarkeit**
- `projects`‑Tabelle, Endpunkte für Create/List/Read/Update/Delete.
- Dashboard: Projektliste, Neu, Umbenennen, Löschen, Sichtbarkeits‑Toggle.
- Öffentliche Galerie (read‑only View ohne Login).
- E2E‑Tests für alle CRUD‑ und Sichtbarkeits‑Transitionen.

**Meilenstein M11 — Sync: `localStorage` → Server**
- Sync‑State‑Machine (Clean, Dirty, Syncing, Error) im Frontend‑Store.
- `POST /api/projects/:id/snapshot` mit JSON‑Body (`Content-Encoding: gzip` optional auf HTTP‑Ebene).
- Gast‑Modus: der bisherige `localStorage`‑Stand wird beim ersten Login optional in den Account übernommen.
- Migration von reiner `localStorage`‑Persistenz auf Hybrid‑Modus: `localStorage` bleibt Offline‑Cache, Server wird kanonisch.
- Unit‑Tests für die State‑Machine, E2E‑Tests für Reload‑Persistenz und Offline→Online‑Sync.

---

**Meilenstein M12 — Hardening und öffentliche Beta**
- Passwort‑Reset via E‑Mail.
- Quotas, Rate‑Limits‑Tuning.
- Performance‑Pass: WASM‑Profiling, WebGPU‑Compute‑Paths.
- Öffentliche Projektgalerie mit Thumbnails.
- Dokumentation, Self‑Host‑Anleitung.

**Spätere Meilensteine (M13+)**
- Sweep, Loft, Curved Patterns, 3MF‑Export.
- **STEP‑Import** via OpenCASCADE im Web‑Worker oder `truck`‑Experiment, inklusive Mesh‑to‑SDF‑Konvertierung und Robustheits‑Pass gegen kaputte COTS‑Dateien. Bewusst weit hinten: erst wenn das MVP steht und es echte User‑Nachfrage gibt.
- **Direct‑Manipulation‑Modus** (Push/Pull auf SDF‑Flächen), sobald Feature‑ID‑Tagging im Raymarcher stabil läuft.
- Alternativer UX‑Modus „Fillet auf Kante malen" — benötigt vorher eine robuste Kanten‑Identifikation aus dem SDF.
- Kollaborative Sessions via CRDT (Yjs o. ä.).
- Optionale Ende‑zu‑Ende‑Verschlüsselung privater Projekte.
- STEP‑Export via B‑Rep‑Rekonstruktion.
- OAuth‑Login.

## 11. Offene Risiken

1. **Meshing‑Qualität an scharfen Kanten:** Dual Contouring ist bekannt anspruchsvoll und erzeugt artefaktanfällig. Prototyping muss früh klären, ob die Qualität für FDM ausreichend ist — bei 0,1 mm Auflösung ist der Toleranzspielraum groß, aber nicht unendlich.
2. **Solver‑Portierung C++ → Rust:** Bei Solvern geht es um numerische Stabilität (Floating‑Point‑Epsilons, Konvergenz). Ein winziger Rundungsunterschied in Rust kann eine Skizze „over‑constrained" melden lassen, obwohl sie es nicht ist. Gegenmaßnahme: differential‑Fuzz‑Testing gegen die C‑Referenz und harte Goldfiles. **Fallback:** `libslvs` bleibt C++ und wird zusammen mit dem Rust‑Kernel in ein gemeinsames WASM‑Modul kompiliert (siehe §4.4).
3. **Topological Naming Problem bei referenzierten Skizzen:** Der Genealogy Graph löst TNP für Booleans, **nicht** aber für den Fall, dass eine Folge‑Skizze auf „der Stirnfläche oben rechts" eines Bauteils sitzt und eine vorgelagerte Skizze diese Fläche strukturell wegkonstruiert (U → L). Gegenmaßnahme: Feature‑Tree‑Knoten referenzieren nicht „Flächen", sondern **Skizzen‑Features mit Seiten‑Label** aus dem Genealogy Graph. Wenn die Referenz bei Re‑Evaluation nicht auflösbar ist, wird der Folge‑Knoten sauber als „broken reference" markiert und im UI hervorgehoben — kein stiller Fehler, kein Crash.
4. **Feature‑ID‑Tagging im Raymarcher:** Damit ein User im Viewport eine „Fläche" oder „Kante" anklicken kann, muss der Shader zu jedem Oberflächen‑Treffer die dominierende Feature‑Tree‑ID zurückgeben. Das ist rechenintensiv und bis heute Forschungsgebiet. Im MVP bewusst eingeschränkt: Auswahl erfolgt primär über Feature‑Tree/Outliner und Skizzen‑Elemente, nicht über direktes Flächen‑Picking am Distanzfeld.
5. **Parametrische Fillets:** Eine Fillet‑Maske darf keine dumme Box im Weltraum sein — sie muss an Skizzen‑Vektoren gebunden bleiben, damit sie bei Änderungen vorgelagerter Extrusionen mitwandert. Das macht den Fillet‑Knoten deutlich komplexer als eine reine statische Maske und ist der Grund, warum Fillet/Chamfer erst ab M7 stehen.
6. **STEP‑Import als B‑Rep‑Mauer:** Mesh‑to‑SDF‑Konvertierung von STEP‑Daten setzt wasserdichte Meshes voraus — COTS‑STEP‑Dateien aus Fernost sind es oft nicht. OpenCASCADE in WASM kostet 20–30 MB Payload. Deshalb ist STEP‑Import bewusst aus M8 herausgeschoben (siehe M13+). Im MVP reicht STL/OBJ‑Referenz, bei Bedarf extern konvertiert.
7. **WebAssembly‑Speicherlimits:** 4 GB Linear Memory pro WASM‑Instanz reicht für Maker‑Bauteile sehr gut, könnte aber bei sehr hochauflösenden Voxel‑Feldern ein Thema werden. Sparse‑Grids (z. B. OpenVDB‑Ansatz) sind der Plan B.
8. **WebGPU‑Browser‑Support:** WebGPU ist 2026 in Chromium und Safari stabil, in Firefox teilweise. Da FieldDraft bewusst kein WebGL‑Fallback pflegt, ist die Browser‑Matrix enger. Gegenmaßnahme: klare System‑Requirements‑Seite und eine Feature‑Detection beim Ladezeitpunkt.
9. **Auto‑Save‑Konflikte:** Wenn derselbe User zwei Tabs parallel öffnet, können lokale Stände divergieren. Gegenmaßnahme: Last‑Writer‑Wins mit Warnung plus versionierte Snapshots im Backend, damit nichts verloren geht.

## 12. Verwandtes / Inspiration

- **Onshape** — UX‑Referenz, Feature‑Tree‑Vorbild.
- **Shapr3D** — Vorbild für „Direkt auf Fläche skizzieren" ohne expliziten Sketch‑Button.
- **Plasticity** — Geschwindigkeit des Direct Modeling, Negativbeispiel für UI‑Zwang.
- **SolveSpace** — 2D‑Solver‑Basis.
- **Manifold** — moderne, robuste Mesh‑Booleans; als potenzielle Ergänzung zum SDF‑Pfad.
- **nTopology / nTop** — kommerzielles SDF/Implicit‑CAD der Luft‑ und Raumfahrt, Beleg dafür, dass der Ansatz industrietauglich ist.
- **MagicaCSG, Dreams (PS)** — kreative SDF‑Modeller als UX‑Inspiration.
- **Fornjot, Truck** — Rust‑native B‑Rep‑Kernel, als Fallback oder Ergänzung interessant.

## 13. Namensgebung und Schwesterprojekt

- **FieldDraft** — Distanz‑**Field**‑basiertes CAD‑**Draft**‑Tool.
- **BeamDraft** — eigenständiges Laser‑Projektor‑Projekt, das denselben 2D‑Constraint‑Solver als Kern nutzt. Durch die gemeinsame geometrische Engine entsteht ein strategischer Hebel: Ein investierter Entwicklungs‑Euro dient zwei Produkten.

## 14. Domains

Die TLDs .eu, .de und .xyz sind frei.
FieldDraft.com ist bereits registriert („for sale“ bei GoDaddy). 
Markenrechtlich gibt es keine Eintragungen.
Keine Github- oder NPM-Projekte mit diesem Namen.

