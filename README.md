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

- Boolesche Operationen sind in SDFs trivial und mathematisch robust (`min`, `max`, `−`) — sie kennen die topologischen Grenzfälle von B‑Rep‑Solvern (co‑planare Flächen, Zero‑Thickness‑Berührungen, TNP) strukturell nicht.
- Offsets sind eine einfache Konstanten‑Subtraktion `f(x) − d`, ohne Self‑Intersection‑Probleme.
- Garantierte Wasserdichtigkeit: Aus einem SDF‑Graphen lässt sich per Dual Contouring ein Mesh ableiten, das mathematisch garantiert keine Löcher oder Self‑Intersections enthält — der Slicer bekommt immer ein druckbares STL.
- Bei 0,1 mm Mesh‑Auflösung ist der „Potato‑Meshing”‑Effekt der klassischen Marching‑Cubes‑Variante für FDM‑Druck praktisch irrelevant, da eine 0,4‑mm‑Düse ohnehin keine perfekt scharfen Kanten druckt; Dual Contouring statt Marching Cubes erhält scharfe mechanische Kanten zusätzlich.

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

Verantwortlich für alles, was Mathematik und Zustand ist — läuft ohne DOM‑Zugriff im Linear Memory von WebAssembly. Der Kernel ist kein reiner SDF‑Evaluator, sondern ein **hybrider Kern** aus drei eng gekoppelten Schichten: ein **CSG‑Graph mit Provenance‑Tracking** (mathematische Wahrheit), ein **Dual‑Contouring‑Mesher** (skalierbare Anzeigegeometrie) und ein daraus abgeleiteter **B‑Rep‑Proxy** (topologische Selektions‑ und Snapping‑Schicht). Diese Drei‑Schichten‑Architektur ist zwingend, weil reines Raymarching auf CSG‑Bäumen über mehrere Bodies und komplexe Sketches nicht skaliert (Instruction‑Cache‑Kollaps der GPU, siehe §11) und ein klassischer B‑Rep‑Kernel im WASM‑Payload zu groß und an Booleschen Grenzfällen zu fragil wäre.

- **Feature‑Tree / CSG‑Graph als Ground Truth:** Der Kernel speichert den parametrischen Baum aus Sketches, Extrusionen, Revolves, Booleans, Patterns, Transformationen, Variablen und Parametern als annotierten DAG. Jeder Leaf‑Knoten (Primitiv oder Sketch‑Extrusion) hat eine eindeutige `node_id`; jeder innere Knoten beschreibt eine Operation (`Union`, `Difference`, `Intersection`, `Transform`, `Pattern`). Geometrische Features werden **nicht** über Koordinaten, sondern über Provenance‑Rezepte aus diesen IDs referenziert (siehe B‑Rep‑Proxy weiter unten). Damit ist das Topological Naming Problem (TNP) für Booleans strukturell gelöst — ohne separates Genealogy‑Naming‑Schema.

- **Multi‑Body‑Isolation:** Jeder eigenständige Körper im Part Studio besitzt seinen **eigenen** CSG‑Graphen, eigenen BVH und eigenen Mesh‑Cache. Es gibt keine globale „Monster‑Gleichung", die alle Bodies verknüpft. Zwei Bodies werden mathematisch nur dann gekoppelt, wenn der User explizit eine Boolean‑Operation zwischen ihnen ausführt. Ohne diese Isolation würde die SDF‑Auswertung bei mehreren Bodies an der Instruction‑Cache‑Grenze der GPU und am asymptotischen O(N) der naiven Punkt‑Auswertung kollabieren.

- **SDF‑Auswertung mit Provenance:** Jede Punkt‑Auswertung `f(p)` liefert nicht nur den Distanzwert, sondern zusätzlich die Liste der **Contributors** — die Leaf‑IDs, deren Distanzfeld am Punkt `p` aktiv den Wert dominiert (innerhalb eines Epsilons ε). Die Auswertung von `min(f_A, f_B)` propagiert beide IDs, sobald `|f_A − f_B| < ε`; sonst nur die ID des dominanten Terms. Aus dieser Contributor‑Menge entsteht der B‑Rep‑Proxy. Boolesche Operationen bleiben mathematisch trivial: Union = `min`, Intersection = `max`, Difference = `max(f_A, −f_B)` — alle hart.

- **Fillets und Chamfers via räumlicher Maskierung:** Kanten‑Operationen (Fillet, Chamfer, organischer Smooth‑Blend) werden **nie global** auf den CSG‑Baum angewandt, sondern lokal über eine **Spatial Mask** — eine unsichtbare Zylinder‑ oder Kapsel‑SDF, die der Kernel entlang der selektierten B‑Rep‑Edge (aus dem Proxy, siehe weiter unten) legt. Innerhalb der Maske ersetzt der Operator das harte `min`/`max` durch den weichen Gegenpart: Fillet als Quadrat‑Summe der Distanzen `r − √(max(r−f_A, 0)² + max(r−f_B, 0)²)`, Chamfer als lineare Mittelung `min(min(f_A, f_B), (f_A + f_B − c)·0.5^½)`, organischer Blend als `smin`. Außerhalb der Maske bleibt das Feld bit‑identisch zum harten Baum, der Operator fällt transparent auf den harten Unteroperator zurück. Der Mechanismus ist für emergente Kanten (CSG‑Schnitte zweier Körper) und intrinsische Kanten (z. B. die vertikale Kante eines extrudierten Quaders) **einheitlich** — es gibt genau einen Code‑Pfad. 2D‑Verrundungen im Sketcher laufen nicht über diesen Pfad, sondern werden vom Solver als explizite Linien‑/Kreisbogen‑Segmente geführt (siehe §4.4), was den 3D‑Baum sauber hält.

- **Ghost Topology (zweigeteilter Evaluator):** Smooth Blends und Fillet‑Masken zerstören die Provenance in der Übergangszone — auf der verrundeten Oberfläche ist weder `f_A = 0` noch `f_B = 0`, die Kante zwischen beiden Contributors existiert mathematisch nicht mehr, und der B‑Rep‑Proxy liefert dort keine Face/Edge/Vertex‑Zerlegung. Damit Picking und Snapping trotzdem funktionieren, fährt der Kernel den CSG‑Baum in **zwei Modi**: Der **visual path** (für Mesher und Render‑Buffer) wertet alle `smin`‑Operatoren und Spatial Masks voll aus und erzeugt die weichen Übergänge. Der **logical path** (für Picking, Snapping und die B‑Rep‑Proxy‑Konstruktion) degradiert `smin → min` und überspringt alle Fillet‑/Chamfer‑Masken — für die Sensorik besteht das Bauteil ausschließlich aus harten Grundkörpern und scharfen Original‑Kanten. Das ist kein zweiter Baum und kein zweites Mesh, sondern ein Flag am CSG‑Walker; die Implementierung kostet einen zusätzlichen Codepfad pro verrundendem Operator, keine doppelte Datenstruktur. Jede verrundete Kante behält damit ihre ursprüngliche Contributor‑Identität, und der User snappt beim Skizzieren auf die virtuelle scharfe Original‑Ecke — funktional äquivalent zu den „Virtual Sharps" klassischer B‑Rep‑Systeme, nur als Nebeneffekt der Dual‑Path‑Evaluation statt als separat gepflegtes Datenmodell.

- **History‑Rollback bei Skizzen auf Ghost‑Edges:** Klickt der User auf eine virtuelle scharfe Kante, die durch ein nachgelagertes Fillet/Chamfer erzeugt wurde, darf das neue Feature (Skizze, Bohrung, Extrude) nicht hinter dem Fillet in die Historie gehängt werden — sonst läge sein Verankerungspunkt auf einer Kante, die im visual path nicht existiert. Stattdessen zieht der Feature‑Tree den Einfüge‑Zeitpunkt automatisch **vor** den ältesten Fillet/Chamfer‑Knoten, dessen Spatial Mask die Contributor‑IDs der geklickten Ghost‑Edge berührt, fügt das neue Feature dort ein und rechnet die Historie bis zum Ende neu. Die Bohrung wird dadurch vom nachgelagerten Fillet automatisch mit‑verrundet, so wie der User es erwartet. Dieser Rollback ist die logische Konsequenz der Ghost Topology und wird vom Feature‑Tree als stiller Schritt ausgeführt — kein Modal, keine User‑Bestätigung.

- **BVH als Pflicht‑Basistechnologie, nicht Optimierung:** Jeder CSG‑Knoten trägt eine axis‑aligned Bounding Box (AABB), die bei jeder Parameter‑Änderung des Sub‑Baums neu berechnet wird. Mesher und Picking traversieren den Graphen ausschließlich über diese BVH; ganze Subtrees werden weggeschnitten, sobald `f_AABB(p) > Schrittweite`. Komplexität sinkt damit von O(N) auf O(log N). Ein Bauteil mit 500 Bohrungen evaluiert pro Sample‑Punkt nur die zwei oder drei tatsächlich relevanten Loch‑Knoten, nicht alle 500. Ohne BVH ist das System bei realistischen Bauteilen nicht nutzbar — sie ist deshalb von Anfang an Pflicht‑Bestandteil des Kernels, nicht eine spätere Optimierung.

- **2D‑Kernel mit 2D‑BVH (immer, ohne Ausnahme):** Skizzen sind ihre eigene 2D‑SDF‑Welt: Linien, Kreisbögen, Polygone und Offset‑Regionen werden als 2D‑SDF‑Bäume gespeichert, mit identischer CSG‑Semantik (`min`/`max` über 2D‑Primitive) und einem dedizierten 2D‑BVH. **Jede** Skizze bekommt diesen BVH, auch triviale — keine Sonderfälle, keine adaptive Wahl. Alternative Strategien wie 2D‑SDF‑Baking in Float32‑Texturen oder Domain Repetition via Modulo werden bewusst **nicht** verwendet: Texturen kosten VRAM, blockieren parametrische Updates und verlieren bei jeder Maßänderung ihren Cache; Domain Repetition funktioniert nur für regelmäßige Muster und passt nicht in das parametrische CAD‑Modell. Die Extrude‑ und Revolve‑Knoten heben das fertige 2D‑Feld in den 3D‑Raum.

- **Splines als Polylinien (im 3D‑Kernel):** Splines (B‑Splines, Bézier) und Kreisbögen werden im 2D‑Solver (siehe §4.4) weiterhin exakt als Kurven gehalten — der Constraint‑Solver braucht die mathematische Form, und auch die Sketch‑Visualisierung im 2D‑Editor zeigt sie exakt. Für die SDF‑Auswertung im 3D‑Kernel werden sie aber in **Polylinien‑Approximationen** mit konfigurierbarer Sehnenfehler‑Toleranz (Default 0,05 mm — feiner als die FDM‑Druckauflösung) zerlegt. Das hält den 2D‑SDF‑AST geschlossen über wenigen einfachen Primitiv‑Typen (Linie, Kreisbogen) und erspart dem Mesher exotische Spline‑SDFs mit transzendenten Distanzformeln, die sich gegen analytisches Provenance‑Tracking nur schwer verteidigen lassen.

- **Dual Contouring als Pflicht‑Renderpfad (nicht Export‑only):** Der Kernel meshet jeden Body kontinuierlich in einem Web Worker und hält das aktuelle Mesh im VRAM gecacht. Bei jeder Parameter‑Änderung im CSG‑Sub‑Baum eines Body wird **nur dieser Body** neu gemesht; alle anderen Bodies bleiben unangetastet. Dual Contouring (statt Marching Cubes) ist Pflicht, weil es scharfe mechanische Kanten erhält und Hermite‑Daten aus den SDF‑Gradienten direkt nutzt. Inaktive Bodies bleiben als statische Meshes im Kontext; aktive Bodies (gerade in Bearbeitung) werden inkrementell re‑meshed. Manifold‑Verletzungen sind ein bekannter DC‑Edge‑Case und werden in einem Post‑Processing‑Pass repariert, siehe §11.

- **B‑Rep‑Proxy aus Provenance:** Aus der Contributor‑Analyse über das gemeshte Volumen entsteht ein dünner topologischer Layer: **Faces** sind zusammenhängende Mesh‑Regionen mit identischem dominanten Contributor; **Edges** sind die Punktmengen, an denen exakt zwei Contributors simultan den Distanz‑Nullwert bestimmen (Schnittkurven zwischen zwei Leaf‑Feldern); **Vertices** sind Punkte mit ≥3 gleichzeitig aktiven Contributors. Edge‑Sample‑Punkte werden zu Polylinien geclustert und auf die mathematisch exakte Schnittkurve der beteiligten Sub‑SDFs zurückprojiziert (Newton‑Iteration entlang `f_A = f_B = 0`), damit die finalen Snap‑Koordinaten nicht auf den ungenauen Mesh‑Vertices liegen. Dieser Proxy ist das einzige Zielobjekt jeder Selektion und jedes Snappings im Frontend. Faces, Edges und Vertices werden über ihre Provenance‑Rezepte gespeichert (z. B. `Edge = Intersection(Node_12, Node_15)`), nicht über Koordinaten — bei Re‑Evaluation des Baums wandern die Bezüge automatisch mit.

- **Export:** STL (Ascii/Binary) und GLB (glTF 2.0) werden direkt aus dem ohnehin vorhandenen Body‑Mesh‑Cache erzeugt — kein separater Export‑Meshing‑Lauf nötig. Später STEP via dediziertem B‑Rep‑Exporter (M14+).

- **Konstanten‑Cache und Undo/Redo:** Da der CSG‑Graph die Ground Truth ist, kommt Undo/Redo praktisch geschenkt — man spult den Baum einen Schritt zurück, der BVH wird inkrementell invalidiert, und nur die betroffenen Bodies werden neu gemesht.

### 4.2 React‑Three‑Fiber‑Frontend (TypeScript)

Verantwortlich für Darstellung, Interaktion und Gizmos. Das UI wird komplett in **React** gebaut; die 3D‑Szene über **React Three Fiber** (`@react-three/fiber`) mit Unterstützung durch **`@react-three/drei`** für Helper (Kamera, Gizmos, Ansichts‑Würfel, Controls). R3F wickelt Three.js deklarativ in den React‑Komponentenbaum ein, sodass Viewport‑State, Outliner, Panels und Feature‑Tree dieselbe State‑Management‑Welt teilen.

- **Rendering:** **Klassische Rasterisierung** der vom Kernel gelieferten Body‑Meshes über die WebGPU‑Pipeline (kein WebGL‑Fallback). **Kein globales Raymarching im Viewport.** Begründung: Sphere Tracing eines verschachtelten CSG‑AST mit mehreren Bodies sprengt den Instruction‑Cache der GPU und führt zu Register Spilling — ein Bauteil mit 50 Features rendert auf der vorhandenen Hardware nicht mehr in Echtzeit. Stattdessen streamt der Kernel nach jeder Parameter‑Änderung neue Vertex/Index‑Buffer pro betroffenem Body an die GPU, und Three.js zeichnet sie wie jedes andere statische Mesh.
- **Kamera‑Steuerung:** Orbit/Pan/Zoom, Ansichts‑Würfel oben rechts zum Ausrichten auf Standardansichten.
- **Picking via Raymarching‑Sensor:** Raymarching wird nicht eliminiert, sondern auf eine spezifische Aufgabe reduziert: **ein einzelner Strahl pro Mausklick**. Die Kamera schießt den Strahl in den Kernel‑CSG‑Graphen, BVH‑beschleunigtes Sphere Tracing findet den ersten Treffpunkt, und der Kernel liefert die Contributor‑Liste an diesem Punkt zurück. Aus den Contributors wird der zugehörige Face/Edge/Vertex des B‑Rep‑Proxys (siehe §4.1) ermittelt. Bei einer komplexen Szene bleibt das selbst mit tausenden Features im Sub‑Millisekunden‑Bereich, weil pro Klick eben nur ein Strahl gerechnet wird, nicht acht Millionen. Mesh‑basiertes Picking gegen die Dreiecke wird **nicht** verwendet — das Mesh ist nur Anzeigelayer und liegt nicht auf den mathematisch exakten Oberflächen. Der Picking‑Ray läuft dabei auf dem **logical path** des Evaluators (§4.1, Ghost Topology): smooth Blends werden zu harten Booleans degradiert und Fillet‑/Chamfer‑Masken übersprungen. Der Strahl trifft damit die virtuelle scharfe Original‑Kante an der Stelle, an der der User visuell eine verrundete Fläche sieht — und der Cursor snappt zuverlässig auf die Contributor‑IDs, die der harte CSG‑Baum an diesem Punkt hinterlassen hat.
- **Gizmos:** Translate, Rotate, Scale und Mirror‑Achse für ganze Feature‑Tree‑Knoten. **Keine** Push/Pull‑Gizmos auf einzelnen Faces — der Workflow ist strikt parametrisch, geändert wird ausschließlich über Skizzen, Maße und die Parameter‑Tabelle (siehe §6.3).
- **Snapping:** Snaps fangen ausschließlich gegen den B‑Rep‑Proxy aus §4.1 — Faces, Edges, Edge‑Mittelpunkte, Vertices, Achsen, Mate‑Connectors. Niemals gegen die Dreiecke des gerenderten Meshes (das Mesh ist eine ungenaue Hülle). Die Snap‑Punkte werden bei Bedarf aus den Contributor‑Rezepten des Proxys analytisch nachberechnet, sodass sie exakt auf den mathematischen Oberflächen liegen, nicht auf den Mesh‑Vertices. Da der Proxy über den logical path der Ghost Topology konstruiert wird, liefert Snapping auch auf verrundeten Bauteilen die scharfen Original‑Edges — die aus klassischem CAD bekannten „Virtual Sharps" fallen in dieser Architektur als Nebeneffekt an, ohne ein zweites Datenmodell zu benötigen.
- **Auto‑Save im Browser:** Jede Änderung am Feature‑Tree erzeugt einen Dirty‑Marker; nach einer kurzen Debounce‑Phase (ca. 500 ms) wird der aktuelle Projektzustand als JSON in `localStorage` persistiert. Beim Seiten‑Reload wird der letzte lokale Stand automatisch wiederhergestellt. Ein späterer Sync an einen Backend‑Server ist vorgesehen, aber nicht Teil des MVPs — die `localStorage`‑Schicht ist zunächst die einzige Persistenz.

### 4.3 Memory‑Brücke

Keine JSON‑Serialisierung zwischen Rust und JS auf dem Geometrie‑Pfad. Der Rust‑Mesher legt Vertices, Normals und Indices **pro Body** als fortlaufende Blöcke im WASM Linear Memory ab; JavaScript erzeugt `Float32Array`‑ und `Uint32Array`‑Views direkt auf diese Speicherbereiche und bindet sie an `THREE.BufferGeometry`‑Instanzen. Damit landet jedes Mesh ohne Kopiervorgang auf der GPU.

Die Brücke ist außerdem **inkrementell**: Bei jeder Parameter‑Änderung, die einen Body invalidiert, streamt der Kernel ausschließlich für diesen Body neue Buffer ans Frontend (über einen Web‑Worker‑Channel), und das Frontend tauscht die `BufferGeometry` dieses Bodies atomar aus. Alle anderen Bodies bleiben unberührt — keine Re‑Allocation, kein GPU‑Upload für inaktiven Kontext. Alte Buffer werden verworfen, sobald der nächste Render‑Frame durch ist; die Lebenszeit‑Verwaltung läuft über Reference‑Counts im Rust‑Kernel und kontrollierte `free()`‑Calls aus dem JS‑Wrapper.

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

> **Hinweis:** Das MVP läuft komplett clientseitig, persistiert wird ausschließlich in `localStorage`. Ein Backend‑Server wird erst nach Abschluss der Kernel‑Phase (siehe Roadmap, nach K9) eingeführt. Der folgende Abschnitt beschreibt die geplante Architektur dieser späteren Phase — inklusive MariaDB, Auth und Projekt‑CRUD.

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

> **Hinweis:** User‑Accounts, ein serverseitiges Dashboard und öffentliche Galerien sind ebenfalls erst Teil der Backend‑Phase (nach K9). Im MVP gibt es genau einen impliziten „lokalen Workspace" im Browser, dessen gesamter Zustand in `localStorage` liegt. Die folgende Spezifikation beschreibt den Zielzustand, nicht den MVP‑Stand.

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
| Fillet (via Spatial Mask + Ghost Topology) | ✅ | |
| Chamfer (via Spatial Mask + Ghost Topology) | ✅ | |
| Thread (analytisches Gewinde‑Primitiv) | ✅ | |
| Gyroid / TPMS‑Infill | ✅ | |
| Procedural Noise / Displacement (Terminal Feature) | ✅ | |
| Domain Deformation (Bend / Twist, Lipschitz‑korrigiert) | ✅ | |
| Morph (Lerp zwischen SDFs) | ✅ | |
| Sweep (via Mesh‑to‑SDF, gemeinsamer Pfad mit Mesh‑Import) | ✅ | |
| Loft | ❌ | M14+ |
| Curved Pattern | ❌ | M14+ |

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

- **Kein expliziter „Neue Skizze”‑Button.** Statt User zu zwingen, zuerst eine Ebene zu definieren, reicht es, das Sketch‑Tool zu aktivieren und direkt auf eine Face im Viewport zu klicken. Der Picking‑Raymarch (siehe §4.2) liefert den Treffpunkt, die Provenance liefert das zugehörige Face des B‑Rep‑Proxys, und der Kernel verankert die neue Sketch‑Ebene **strukturell an diesem Face** (Auto‑Lock): wandert das Face später durch eine Parameter‑Änderung im Raum, wandert die Skizzenebene mit. Das funktioniert verlässlich für planare Faces aus Extrusions‑ und Revolve‑Knoten (Vorbild: Shapr3D). Klicks auf gekrümmte/organische Faces würden eine „unconstrained” Ebene erzeugen, die der User mit harten Maßen im 3D‑Raum verankern müsste — diese Variante ist im MVP bewusst noch nicht implementiert (siehe §11).
- **Strikt parametrischer Workflow.** Änderungen laufen **ausschließlich** über Skizzen, Bemaßungen und Variablen — bewusst kein Direct‑Manipulation‑Modus im MVP. Der B‑Rep‑Proxy aus §4.1 liefert zwar Faces, Edges und Vertices als selektierbare Objekte, aber ein Push/Pull‑Gizmo auf einer Face bräuchte einen konsistenten Rückweg von der gezogenen Distanz in das richtige numerische Feld eines Upstream‑Sketch‑ oder Extrude‑Knotens. Diese Abbildung ist im allgemeinen Fall mehrdeutig (welcher Parameter soll wachsen?) und in Multi‑Body‑Szenen instabil. Das Onshape‑Modell ist sauberer: Skizze → Extrusion → Maß ändern → Baum rechnet neu. Ein Direct‑Mode kann später nachgeliefert werden (M14+), sobald ein robustes Heuristik‑Modell für die Push/Pull‑zu‑Parameter‑Auflösung existiert.
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
     3. **Boolean (Opt‑in, „Convert to Solid"):** Nur auf expliziten Wunsch wird das Mesh via Mesh‑to‑SDF (BVH‑basiert, Winding‑Number/Pseudo‑Normal‑Innentest) in ein Distanzfeld überführt und damit ein echter Feature‑Tree‑Knoten, der in Booleans teilnehmen kann. Vorher läuft ein Wasserdichtigkeits‑Check (Manifold‑Prüfung); bei kaputten COTS‑Meshes warnt das UI, bevor Artefakte entstehen. Rationale: COTS‑Downloads von Thingiverse/GrabCAD haben oft Self‑Intersections, Zero‑Thickness‑Flächen und umgedrehte Normalen — ein pauschales Mesh‑to‑SDF bei jedem Import würde den WASM‑Speicher mit fehlerhaften Feldern fluten. Außerdem zieht man in der Praxis fast nie eine ganze COTS‑Platine als „Stempel" ab, sondern baut einfache Aussparungen drumherum. **Derselbe Mesh‑to‑SDF‑Knoten trägt auch das Sweep‑Feature aus §6.1:** Ein Sweep entlang eines 3D‑Pfades mit Twist wird vom Kernel zunächst mit klassischen Polygon‑Algorithmen als wasserdichtes Dreiecks‑Mesh erzeugt (das ist ein gelöstes Problem der Computergrafik) und dann über exakt diesen Mesh‑to‑SDF‑Pfad in den CSG‑Baum eingehängt. Import und Sweep teilen sich damit Code, BVH und Test‑Suite — die Investition in einen robusten Mesh‑to‑SDF‑Knoten bringt beide Features gleichzeitig und erspart dem Kernel einen dedizierten analytischen Sweep‑Operator, der für verdrehte 3D‑Pfade die Lipschitz‑Bedingung ohnehin verletzen würde.
   - **DXF, SVG** in den Sketch‑Editor.
   - **STEP (Ausblick, nicht im MVP):** Ein echter STEP‑Import würde B‑Rep‑Daten in ein SDF umwandeln (Mesh‑to‑SDF), was wasserdichte Eingangs‑Meshes voraussetzt — in der Praxis sind COTS‑STEP‑Dateien oft fehlerhaft. Außerdem bedeutet OpenCASCADE im WASM‑Payload 20–30 MB zusätzlich. Der Weg wird erst angegangen, wenn das MVP steht. Bis dahin lautet der Workaround: STEP extern (z. B. in FreeCAD) zu STL/OBJ konvertieren und als Referenz laden.

## 8. Datenhoheit, Deployment und Lizenz

- **Client‑Only im MVP:** Alle Berechnungen (CSG‑Auswertung, Mesher, Solver, Picking‑Raymarch) laufen im Browser, Persistenz nach `localStorage`. Es gibt keinen Server — das Tool kann vollständig als statisches Bundle ausgeliefert werden (GitHub Pages, Netlify, beliebiger Webserver). Ein Backend wird erst in einer späteren Phase ergänzt (siehe §4.6 und Roadmap).
- **Tauri als Escape Hatch:** Falls der Browser‑Kontext an harte Grenzen stößt — 2–4 GB WASM‑RAM, eingeschränktes Multithreading über Web Worker, WebGPU‑Reifegrad in Firefox — ist ein Port nach Tauri vorgesehen. Das React/R3F‑Frontend bleibt bitgenau erhalten, und der Rust‑Kernel zieht aus WASM in den nativen Rust‑Prozess um: voller Prozess‑RAM statt 2–4 GB, echtes Multithreading via `rayon` für paralleles Meshing mehrerer Bodies, direkter `wgpu`‑Zugriff auf Vulkan/Metal/DirectX. Tauri statt Electron, weil es den nativen Webview des Betriebssystems nutzt (App‑Größe ca. 5–10 MB statt 150 MB) und Rust‑Backend und Webview‑Frontend über einen nativen Message‑Bus koppelt, ohne N‑API‑Brücken oder Shared‑Memory‑Tricks. Der Umstieg kostet Code‑Duplizierung nur an der Persistenz‑Schicht (`localStorage` vs. Dateisystem); der Kernel, der Solver, der Feature‑Tree und die gesamte UI bleiben unverändert. Das MVP wird aber zuerst im Browser gebaut — Tauri ist eine Trumpfkarte für später, kein aktueller Entwicklungsstrang.
- **Client‑Heavy, Server‑Light (spätere Phase):** Sobald der Server existiert, übernimmt er nur Auth, Projekt‑CRUD und Sync — keine Geometrie‑Dienste, damit die Cloud‑Kosten minimal bleiben und Self‑Hosting trivial wird.
- **Self‑Hostable (spätere Phase):** Ein einzelnes statisches Go‑Binary + MariaDB‑Container reicht. `docker-compose.yml` im Repo startet dann Server, DB und Web‑Frontend mit einem Befehl.
- **IP‑Schutz:** Projekte sind standardmäßig privat; der Sichtbarkeits‑Schalter muss explizit umgelegt werden, um etwas zu publizieren. Damit ist FieldDraft — anders als Onshape Free — für Hardware‑Startups mit Patent‑Ambitionen unbedenklich.
- **Lizenzmodell:** Offen (noch zu entscheiden: AGPL oder Apache‑2.0 Core + kommerzielle Cloud‑Komponente).

## 9. Teststrategie

Tests werden **von Anfang an** mitentwickelt, nicht nachgezogen. Die Pyramide sieht so aus:

- **Rust‑Unit‑Tests** (`cargo test`) für den Geometriekern: SDF‑Primitive (3D und 2D), harte Booleans, CSG‑Graph mit Provenance‑Auswertung (Contributor‑Mengen für Single‑Face/Edge/Vertex‑Punkte), 3D‑BVH‑Korrektheit (BVH‑Cull‑Ergebnis identisch zur naiven O(N)‑Referenz für zufällige CSG‑Bäume), 2D‑BVH‑Korrektheit, Dual‑Contouring‑Goldfiles (Mesh‑Hash gegen Referenz für kanonische Bauteile), Manifold‑Check des DC‑Outputs auf kritischen Topologien, Multi‑Body‑Isolation (eine Body‑Änderung darf andere Body‑Meshes nicht invalidieren), B‑Rep‑Proxy‑Ableitung aus Provenance, Polyline‑Approximation von Splines mit Sehnenfehler‑Toleranz, Serde‑Roundtrips des Projektformats. Numerische Vergleiche mit Referenztabellen.
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
- **Visual‑Regression** via Playwright‑Screenshots für kritische Viewport‑States (frisch gemeshtes Bauteil, Hover‑Highlight auf Face/Edge/Vertex, Sketch‑Editor‑Snapshot).
- **Lokaler Testlauf:** Ein einzelner Befehl `just test` führt `cargo test` (Kernel), Vitest und Playwright nacheinander aus. Pro Commit grün oder nicht gemerged. Eine CI‑Pipeline (GitHub Actions o. ä.) wird bewusst erst aufgesetzt, sobald das Projekt auf GitHub landet.
- **Test‑Fixtures:** Ein Satz kanonischer Beispielprojekte (`fixtures/`) als Goldfiles für Regression und Bootstrapping neuer Tests.

## 10. Roadmap

Die Roadmap ist darauf ausgelegt, dass ich als Pair‑Programming‑Agent die Umsetzung in kleinen, testbaren Vertical Slices vorantreiben kann. Jeder Meilenstein endet mit grünen Tests, einem lauffähigen Artefakt und einem nachvollziehbaren Commit‑Verlauf. Keine „Big Bang”‑Phasen.

**Erledigt vor Architektur‑Wechsel (M0–M2) sowie K1, K2, K3 (Teile 1–3):**

- **M0:** Repo‑Scaffold, Build‑Tooling (`just`, `wasm-pack`, `npm`), Test‑Harnesses (`cargo test`, Vitest, Playwright), CI‑freier lokaler Workflow, `crates/kernel`, `crates/protocol`, `web/`.
- **M1:** Erste WebGPU‑Viewport‑Iteration mit TSL‑Raymarcher und festen Primitiven, Zero‑Copy‑Memory‑Brücke, debounced Auto‑Save in `localStorage`.
- **M2:** Feature‑Tree, Boolean‑Knoten, Transform‑Knoten, Undo/Redo, Outliner, Range‑Select, Light‑Mode‑CSS.
- **K1:** CSG‑AST in Rust (`crates/kernel/src/csg.rs`) mit Box/Sphere/Cylinder‑Leaves und Union/Difference/Intersection/Transform‑Inner‑Nodes; naives uniformes Dual Contouring (`crates/kernel/src/dc.rs`) inklusive Goldfile‑Test „Quader mit zwei Bohrungen”; Zero‑Copy‑Bridge für Positionen/Normalen/Indices via `meshPositionsPtr`/`meshIndicesPtr`; `Viewport.tsx` rendert jetzt eine `BufferGeometry` aus den Kernel‑Puffern, der TSL‑Raymarcher (`raymarch.ts`) bleibt als Referenz im Repo, ist aber nicht mehr in den Render‑Pfad eingebunden.
- **K2:** Provenance‑getragene `CsgNode::evaluate(p, eps) → (distance, Contributors)` mit stabilen `NodeId`s auf jedem Leaf (übernommen aus `FeatureId`), inline `Contributors`‑Set (Kapazität 4) und `ProxyElement`‑Klassifikation (Face / Edge / Vertex). Sphere‑Tracing‑Raycaster in `crates/kernel/src/raycast.rs` plus `Kernel::pick(origin, dir)` wasm‑bindgen‑Export mit JSON‑Antwort. Der DC‑Mesher taggt jeden Vertex mit Klassifikationsbyte + flachem Contributor‑Array (`MAX_CONTRIBUTORS` Slots, `u32::MAX`‑Sentinel); Zero‑Copy via `meshClassificationsPtr`/`meshContributorsPtr`. Im Frontend treibt `PickProbe` einen `THREE.Raycaster` über die Pointer‑Bewegung, ruft `kernel.pick()` auf, zeigt das Ergebnis in einem `PickHud`‑Overlay (Farbkodierung face/edge/vertex) und setzt einen kleinen Debug‑Marker an den Trefferpunkt. Tests: Contributor‑Klassifikation auf Face/Edge/Vertex‑Punkten, Difference‑Randkurve, Raycast‑Miss/Face‑Hits, Mesh‑Klassifikation auf Union(Quader, Zylinder).

**Zu K2 offen (bewusst verschoben):** Clusterung der Edge‑Vertices zu Polylinien und Newton‑Projektion auf die exakte Schnittkurve `f_A = f_B = 0` sind in K2 noch nicht umgesetzt. Die Sample‑basierte Klassifikation pro Mesh‑Vertex liefert den groben Proxy, der für Hover‑Debug‑Anzeigen reicht; präzises Snapping braucht diese beiden Nachbesserungen und wandert mit K6 (Sketch‑Editor) bzw. dediziertem K2.1‑Slice zurück in den Kernel.

- **K3 (Teil 1, BVH‑Pfad):** Dual‑Path‑Evaluator‑Infrastruktur über ein neues `EvalMode::{Visual, Logical}`‑Enum, das durch alle SDF‑ und Contributor‑Call‑Sites gefädelt ist — Mesher bekommt `Visual`, Raycaster/Picking bekommen `Logical`, beide Pfade sind bis K10 bit‑identisch. Parallel dazu ein `BoundedCsg`‑Wrapper (`crates/kernel/src/bvh.rs`), der die logische `CsgNode`‑Form einmal pro Mutation in eine query‑optimierte Baumform überführt: jeder Knoten trägt seine Welt‑AABB gecached, `CsgNode::Transform`‑Ketten werden beim Build in den `world_transform` jedes Leaves gefaltet (via neuer `compose_transforms` und `quat_multiply` in `lib.rs`), und die hot‑path‑Methoden `sdf(p, mode)` und `evaluate(p, eps, mode)` prunen Sub‑Bäume, deren AABB‑Lower‑Bound den aktuellen Akkumulator bereits dominiert (Union + Difference; Intersection bleibt naiv, weil die AABB dort keinen Upper Bound liefert). `rebuild_mesh_internal` und `Kernel::pick` bauen die `BoundedCsg` einmal und reichen sie an `dual_contour` bzw. `raycast`. Tests: Leaf‑Transform‑Flattening, Union/Difference‑AABB‑Semantik, Äquivalenz `BoundedCsg::sdf == CsgNode::sdf` auf gemischten Szenen, Pruning‑Korrektheit bei weit auseinander liegenden Bodies, `distance_point_to_aabb` innen/außen.

- **K3 (Teil 2, Multi‑Body‑Kernel):** Partitionierung des linearen Feature‑Spines in disjunkte Bodies über räumliche AABB‑Überlappung (`crates/kernel/src/body.rs`). Regeln: jede `Union` ohne Überlappung startet einen neuen Body, `Union`‑Features mit Überlappung fügen sich an (und verschmelzen ggf. mehrere Bodies zu einem, wenn sie brücken), `Difference`/`Intersection` hängen sich an den ersten überlappenden Body; orphane Diff/Inter werden stillschweigend verworfen. Body‑IDs sind stabil (`= min(feature_id)` pro Group), sodass die Frontend‑Seite Body‑Instanzen über Mutationen hinweg wiedererkennen kann. Der Kernel hält `Vec<Body>` mit per‑Body‑`Mesh` + `payload_hash` + `version`: nach jeder Mutation rebuildet `rebuild_mesh_internal` nur die Bodies, deren Hash sich tatsächlich geändert hat, und bumpt die per‑Body‑Version. Bodies, deren Seed aus dem Dokument verschwindet, werden verworfen. Die alte aggregierte `meshPositionsPtr`‑API bleibt funktional (Konkatenation mit Index‑Offsets pro Body), damit der Viewport unverändert weiterläuft; parallel ist eine neue per‑Body‑Oberfläche (`bodyCount`, `bodyId(i)`, `bodyVersion(i)`, `bodyPositionsPtr(i)`, `bodyNormalsPtr(i)`, `bodyIndicesPtr(i)`, `bodyClassificationsPtr(i)`, `bodyContributorsPtr(i)`) für den kommenden Frontend‑Split bereit. Tests: Partition‑Korrektheit (Single‑Body, zwei disjunkte Unions, überlappende Union, Difference‑Anbindung, orphane Diff, brückende Union, Feature‑Order‑Erhaltung, stabile Body‑ID), Payload‑Hash‑Stabilität, und die Schlüssel‑Assertion: eine Mutation an Body B bumpt nur Body B's Version, Body A bleibt unberührt.

- **K3 (Teil 3, Multi‑Body‑Frontend):** Die Aggregat‑Mesh‑Krücke aus Teil 2 ist weg. Der Kernel hat kein `mesh: Mesh`‑Feld mehr und keine aggregierten Pointer‑Accessoren; `meshVertexCount`/`meshIndexCount`/`meshTriangleCount` bleiben als billige Summen über alle Bodies erhalten (für Statusleisten und die Rust‑Tests). Der Viewport‑`KernelMesh`‑Component iteriert jetzt pro Frame über `kernel.bodyCount()`, hält eine `Map<body_id, { geometry, version }>` als React‑Ref und legt **pro Body genau eine `THREE.BufferGeometry`** an. Neue Bodies werden per `body*View(i)`‑Zero‑Copy gelesen, gesliced und als Attribute gebunden. Bestehende Bodies deren `bodyVersion(i)` sich nicht geändert hat, werden übersprungen — weder CPU‑ noch GPU‑Traffic. Geometrien von Bodies, die aus dem Kernel verschwinden (Seed gelöscht oder in einen Nachbarn gemergt), werden `dispose()`'d. React rendert nur dann ein neues `<mesh>`‑Array, wenn sich die Menge der Body‑IDs ändert; reine Versions‑Bumps mutieren nur die `BufferGeometry`, die R3F beim nächsten Paint automatisch aufpickt. Die `kernel.ts`‑Wrapper‑Klasse bekommt passend dazu `bodyCount`/`bodyId(i)`/`bodyVersion(i)`/`bodyVertexCount(i)`/`bodyIndexCount(i)`/`bodyTriangleCount(i)` sowie fünf `body*View(i)`‑Methoden. Damit ist die Multi‑Body‑Isolation jetzt **durchgängig** — CPU‑Partition, Per‑Body‑Cache, GPU‑Upload‑Skipping — ohne dass ein Body, der nicht angefasst wurde, in irgendeinem Pass der Pipeline auftaucht.

**Zu K3 offen (für die nächste Session):** Web‑Worker‑Mesher mit Transferable‑basiertem `ArrayBuffer`‑Hot‑Swap (Mesh‑Berechnung wandert aus dem Main‑Thread, damit ein Re‑Mesh bei 50+ Bodies die UI nicht einfriert), Active‑vs‑Context‑Body‑LOD (gerade bearbeitete Bodies bekommen feineres Grid als ruhender Kontext), und die Performance‑Assertion „≥50 Bodies, Re‑Meshing blockiert UI nicht“. Die Kernel‑ und Wrapper‑APIs aus Teilen 2–3 sind bereits so ausgelegt, dass nur noch der Worker‑Channel und ein Transferable‑Protokoll für die Buffer‑Slices pro Body fehlen — keine weiteren Rust‑Änderungen.

**Hinweis zum Architektur‑Wechsel:** Mit dem Übergang in die Kernel‑Phase wird der TSL‑Raymarcher‑Renderpfad aus M1 stillgelegt und durch einen Mesh‑basierten Renderpfad ersetzt. Begründung in §4.1, §4.2 und §11 — kurz: globales Raymarching skaliert nicht über mehrere Bodies und komplexe Sketches hinaus, und ohne einen B‑Rep‑Proxy aus Provenance lassen sich Faces/Edges/Vertices nicht zuverlässig selektieren. Die übrigen Artefakte aus M0–M2 (Repo, Tooling, Tests, Auto‑Save, Feature‑Tree, Outliner, Undo/Redo, CSS) bleiben unverändert in Verwendung.

---

### Kernel‑Phase (K1–K10): Hybrider SDF/Provenance/B‑Rep‑Kernel

Diese Phase ersetzt die alten Meilensteine M3–M8 vollständig. Sie baut den hybriden Kernel aus §4.1 in sieben aufeinander aufbauenden Slices auf, dann folgen drei nachgelagerte Feature‑Slices (K8–K10).

**K1 — Kernel‑Skelett und statisches Mesh‑Rendering**
- CSG‑AST in Rust (enum mit Box/Sphere/Cylinder als Leafs, Union/Difference/Intersection/Transform als Inner Nodes).
- Punkt‑Auswertung `f(p) → distance` ohne Provenance, ohne BVH (naiv O(N) — bewusst, um die Render‑Pipeline isoliert verifizieren zu können).
- Naives Dual Contouring auf einem uniformen Voxel‑Grid (grobe Auflösung, single‑threaded, kein Manifold‑Pass).
- Mesh‑Brücke nach Three.js: Vertex/Index‑Buffer als `Float32Array`/`Uint32Array`‑Views auf WASM Linear Memory.
- WebGPU‑Viewport rendert das Mesh wie jedes andere statische Geometrie‑Objekt; Kamera‑Controls bleiben aus M1 erhalten.
- TSL‑Raymarcher‑Renderpfad wird stillgelegt (Code bleibt vorerst für Referenz‑Tests im Repo, ist aber nicht mehr aktiv).
- Test: hartkodierte CSG‑Szene (Quader mit zwei Bohrungen) wird als Mesh gerendert; Snapshot‑Vergleich gegen Goldfile.

**K2 — Provenance, Picking, B‑Rep‑Proxy**
- Punkt‑Auswertung erweitert auf `(distance, contributors[])`: jeder Leaf‑Knoten hat eine `node_id`, `min`/`max`/`difference` propagieren die ID des dominanten Operanden bzw. beide IDs bei Gleichstand innerhalb ε.
- Picking‑Pfad: Mausklick → Kamera‑Strahl → Sphere‑Trace (vorerst ohne BVH) → Trefferpunkt → Contributor‑Auswertung.
- B‑Rep‑Proxy‑Konstruktion aus dem ohnehin gemeshten Volumen: Sample‑Pass über die Mesh‑Vertices, Klassifikation in Face (1 Contributor), Edge (2 Contributors), Vertex (≥3 Contributors), Clusterung benachbarter Edge‑Punkte zu Polylinien, Newton‑Projektion auf die exakte Schnittkurve `f_A = f_B = 0`.
- Frontend: Hover im Viewport hebt das getroffene Face/Edge/Vertex visuell hervor.
- Test: hartkodierte Union(Quader, Zylinder) — Klick auf die emergente Schnittkante liefert die korrekten zwei Leaf‑IDs und das richtige Edge‑Element des Proxys.

**K3 — BVH, Multi‑Body, asynchrones Meshing**
- 3D‑AABB pro CSG‑Knoten; BVH‑Konstruktion bei jeder Tree‑Mutation, BVH‑beschleunigte Punkt‑Auswertung sowohl für Mesher als auch für Picking.
- Multi‑Body: jeder eigenständige Körper bekommt einen eigenen CSG‑Graphen, eigenen BVH und eigenen Mesh‑Cache. Body‑übergreifende Booleans nur auf expliziten User‑Wunsch.
- Meshing wandert in einen Web Worker; Body‑Updates streamen über einen Channel ins Frontend, das die `BufferGeometry` dieses Bodies atomar austauscht (siehe §4.3).
- Active‑Body vs. Context‑Body‑LOD: aktive Bodies werden bei Parameter‑Änderung sofort neu gemesht, Context‑Bodies bleiben statisch im VRAM.
- Test: 50+ Primitive‑Bodies in einer Szene, Re‑Meshing eines einzelnen Bodies blockiert das UI nicht (Playwright‑Frame‑Rate‑Assertion), Picking liefert in <5 ms das korrekte Body/Face‑Paar.

**K4 — 2D‑Kernel, 2D‑BVH, Extrude, Revolve**
- 2D‑SDF‑AST: Linie, Kreisbogen, Polygon, Offset‑Region; Composing über `min`/`max` analog zu 3D.
- 2D‑BVH als Pflicht‑Pfad für **jede** Skizze, ohne adaptive Sonderfälle (kein Baking, kein Modulo).
- Splines werden im Solver (kommt in K5) exakt geführt; für die SDF‑Auswertung im 3D‑Kernel werden sie in Polylinien mit konfigurierbarer Sehnenfehler‑Toleranz (Default 0,05 mm) zerlegt. In K4 reicht ein Stub für die Polyline‑Approximation, da der Solver noch fehlt.
- Extrude‑ und Revolve‑Knoten heben das 2D‑Feld in den 3D‑Raum (`max(f_2D(x,y), |z| − h/2)` bzw. `f_2D(√(x²+z²), y)`).
- Skizzen werden in dieser Phase noch programmatisch in Rust‑Tests aufgebaut, keine UI.
- Test: extrudiertes 200‑Loch‑Polygon rendert; Picking trifft korrekt einzelne Loch‑Wände; 2D‑BVH‑Cull beschleunigt die Punkt‑Auswertung gegen die naive O(N)‑Referenz um ≥ 50× bei 200 Löchern.

**K5 — Constraint‑Solver (libslvs‑Portierung)**
- Portierung von `libslvs` nach Rust, Test‑Harness gegen die C‑Referenz inkl. Übernahme der Tests aus `solvespace/test`. Die portierten Tests müssen fehlerfrei laufen. Bei nicht behebbaren Stabilitätsproblemen Fallback auf `libslvs` in C++ via Emscripten/`cc` (siehe §4.4).
- Pflicht‑Constraints: Coincident, Parallel, Horizontal/Vertical, Dimension (Länge, Winkel), Tangent, Concentric, Equal, Midpoint, Symmetric.
- DoF‑Analyse und strukturierte Diagnostics (over/under‑constrained).
- `proptest`‑Suite für invariante Eigenschaften.
- Headless‑Smoke‑Test: programmatisch aufgebaute Sketches werden gelöst, ohne dass UI existiert.

**K6 — Sketch‑Editor‑UI**
- lies zum Beginn dieses Meilensteins onshape-ux.md
- Sketch‑Ebene via Klick auf Face: Picking‑Pfad aus K2 + Auto‑Lock an das Face des B‑Rep‑Proxys (siehe §6.3).
- Werkzeuge: Line, Rectangle, Circle, Arc, Point, Trim, Mirror, Offset.
- Vektor‑basiertes Snapping ausschließlich gegen den B‑Rep‑Proxy (Faces, Edges, Edge‑Mittelpunkte, Vertices) und gegen Sketch‑interne Punkte.
- Live‑Einbindung des Solvers aus K5: Bemaßungen und Constraints wirken direkt beim Zeichnen und beim Dragging.
- Fehleranzeige für over/under‑constrained Sketches am betroffenen Element.
- Playwright‑Tests für Zeichenwerkzeuge (Canvas‑Koordinaten‑Assertions).

**K7 — Feature‑Tree‑UI, Booleans, Export**
- Outliner und Feature‑Tree‑Panel im Frontend (baut auf der M2‑Implementierung auf).
- Boolean‑Knoten (Union/Difference/Intersection — **nur hart**, kein `smin`), Transform‑Knoten, Pattern‑Stub.
- Undo/Redo über Tree‑Navigation, integriert mit dem inkrementellen BVH‑Update aus K3.
- Vollständiger E2E‑Workflow: Sketch → Extrude → Boolean → Export.
- STL‑Ascii‑/Binary‑Export und GLB‑Export direkt aus den Body‑Mesh‑Caches (kein separater Export‑Mesher); Goldfile‑Tests gegen kanonische Bauteile.

**K8 — Patterns, Mirror, Variablen, Formel‑Engine**
- Linear/Circular Pattern, Mirror als Feature‑Tree‑Knoten — Patterns werden als wiederholte CSG‑Sub‑Bäume mit Transform‑Wrappern realisiert (keine Modulo‑Tricks im SDF, siehe §4.1).
- Vollwertige Formel‑Engine im Kernel (siehe §4.5): Expression‑Parser mit AST, Einheiten‑System (`mm`, `deg`, …), Funktionsbibliothek (`sin`, `sqrt`, `min`, …), Referenzen auf andere Variablen, Zyklen‑Erkennung, strukturierte Fehler‑Diagnostics. Maßfelder im Sketch‑Editor akzeptieren Formeln direkt (`=width/2 + 5 mm`).
- `proptest`‑Suite für Parser‑Roundtrip, Einheiten‑Arithmetik und Zyklen; Goldfiles für kanonische Onshape‑ähnliche Ausdrücke.
- Abhängigkeitsgraph‑basierte Re‑Evaluation des Baumes bei Variablen‑Änderung (nur betroffene Downstream‑Knoten).
- E2E‑Test: parametrisches Schrankelement aus den Assets.

**K9 — Referenz‑Import, Measure, Mate Connector**
- **STL/OBJ‑Import als visuelle Referenzgeometrie** (keine SDF‑Konvertierung, keine Booleans gegen das Referenzmesh).
- GLB‑Import für Demo‑ und Galerie‑Zwecke.
- Maßband‑Tool über den B‑Rep‑Proxy.
- Mate‑Connector als anheftbares lokales Koordinatensystem an Faces/Edges/Vertices des Proxys.
- Visual‑Regression‑Suite für den gesamten Part‑Editor.

**K10 — Fillets, Chamfers, Spatial Masks, Ghost Topology**
- Spatial‑Mask‑Operator im CSG‑AST: Zylinder‑/Kapsel‑SDF entlang einer selektierten Proxy‑Edge, weicher Auslauf an den Kapsel‑Enden. Innerhalb der Maske ersetzt der Operator das harte `min`/`max` durch die Fillet‑Formel (Quadrat‑Summe) oder die Chamfer‑Formel (lineare Mittelung); außerhalb fällt er transparent auf den harten Unteroperator zurück.
- Masken‑AABB wird als eigener Knoten im 3D‑BVH registriert, damit die Punkt‑Auswertung außerhalb der Masken‑Hülle den Mix‑Operator komplett überspringt und die Rechenlast logarithmisch mit der Masken‑Dichte skaliert (siehe §11 Risiko 6).
- Ghost Topology: Dual‑Path‑Flag am CSG‑Walker. Der *visual path* wertet `smin` und alle Spatial Masks voll aus und erzeugt die weichen Übergänge für Mesher und Render‑Buffer. Der *logical path* degradiert `smin → min` und überspringt alle Masken — Picking, Snapping und B‑Rep‑Proxy‑Konstruktion laufen ausschließlich auf diesem Pfad und sehen harte Original‑Kanten (siehe §4.1, §4.2).
- History‑Rollback beim Klick auf Ghost‑Edges: Der Feature‑Tree fügt neue Features automatisch **vor** den ältesten Fillet/Chamfer‑Knoten ein, dessen Maske die Contributor‑IDs der geklickten Edge berührt, und rechnet die Historie neu. Eine Bohrung auf einer verrundeten Kante wird damit vom nachgelagerten Fillet mit‑verrundet, ohne dass der User den Zeitstempel manuell verschieben muss.
- UI: Fillet‑ und Chamfer‑Tool in der Part‑Editor‑Toolbar, Einzelsegment‑Selektion auf dem Proxy, Radius‑/Distanz‑Eingabe mit Live‑Preview. Edge‑Chaining entlang tangentialer Nachbarn bleibt zunächst manuell über Mehrfach‑Selektion (siehe §11 Risiko 8).
- Tests: Goldfiles für Einzelkanten‑Fillets auf Würfel und Zylinder, Zimmerecken‑Regression als Baseline für den weichen Auslauf, Overlap‑Detection zwischen benachbarten Masken (Warnung bei Interferenz, siehe §11 Risiko 7), Ghost‑Topology‑Picking auf einem zweifach verrundeten Bauteil (Klick muss die scharfe Original‑Edge treffen, nicht die verrundete Oberfläche), History‑Rollback‑Test (Bohrung auf Ghost‑Edge wird vom Fillet automatisch mit‑verrundet).

**K11 — Native SDF‑Superkräfte**
- Gyroid / TPMS‑Infill als Leaf‑Knoten mit der Standardgleichung `sin(p.x)·cos(p.y) + sin(p.y)·cos(p.z) + sin(p.z)·cos(p.x)`, per Boolean mit dem Host‑Körper verschnitten. Parameter: Zellengröße, Wandstärke, optionale Clipping‑Domain. Kosten im Speicher: null Bytes — die gesamte Gitterstruktur lebt ausschließlich als Formel.
- Procedural Noise / Displacement als Terminal‑Operator: `f_result(p) = f_body(p) − noise(uv(p)) · tiefe`, mit Triplanar‑Projektion als UV‑loses Default‑Mapping und zylindrischer Projektion als Option für Handgriffe und Knöpfe. Der Operator ist im *logical path* als Terminal markiert und wird übersprungen — der B‑Rep‑Proxy sieht weiterhin die glatte Grundform, und das Snapping fängt auf der Ur‑Oberfläche.
- Domain Deformation (Bend, Twist) als Transform‑Knoten mit Lipschitz‑Korrektur: Der Raum wird vor der Auswertung verbogen, der zurückgegebene Distanzwert um den Verzerrungsfaktor skaliert, damit Raymarcher und Mesher nicht an falschen Metriken hängen bleiben.
- Morph (Lerp zwischen zwei SDFs): `f_morph = (1−t)·f_A + t·f_B` mit animierbarem `t`. Für `t ∈ {0, 1}` greift Ghost Topology ungebrochen; für Zwischenwerte ist der Knoten Terminal, weil beide Contributor‑Mengen in der Übergangszone verschmelzen.
- Tests: Gyroid‑Wasserdichtigkeit nach Dual Contouring, Displacement‑Terminal‑Verhalten (Snapping liefert glatte Ur‑Fläche auf gerändeltem Knopf), Twist‑Zahnrad‑Goldfile gegen Referenz, Morph‑Animation über 10 Zwischenschritte.

**K12 — Analytisches Gewinde‑Tool**
- Thread‑Primitiv als Leaf‑Knoten mit direkter mathematischer Gewinde‑Formel (metrisch M3–M64, Trapez, Zoll). Kosten: O(1) pro Sample‑Punkt. Kein Sweep entlang einer Helix‑Kurve — dieser Weg scheitert im SDF an der Metrik‑Verzerrung der Raum‑Rotation.
- Der B‑Rep‑Proxy behandelt das Thread‑Feature im *logical path* als glatten Referenzzylinder mit Kerndurchmesser. Snapping fängt auf Mittelachse, Stirnfläche und Steigungs‑Parameter, nicht auf den spiraligen Gewindeflanken des *visual path*.
- UI: Thread‑Tool auf Zylinder‑Face angewandt, Gewinde‑Dropdown (M3, M4, …), Richtung (innen/außen), Länge. Schraube und Mutter teilen sich dieselbe Formel mit umgekehrtem Vorzeichen.
- Tests: Wasserdichtigkeits‑Check, Schraube‑in‑Mutter‑Goldfile mit definierter Spielpassung, Stirnflächen‑Snap‑Test, Thread‑Boolean‑Kombination mit gefastem Zylinder.

**K13 — Mesh‑to‑SDF: Convert‑to‑Solid und Sweep**
- Mesh‑to‑SDF‑Knoten: BVH gegen Dreiecke, kürzeste Distanz plus Winding‑Number oder Pseudo‑Normal‑Check für Innen/Außen. Ein einziger Knotentyp, der sowohl den Import‑Convert‑to‑Solid‑Pfad aus §7 als auch das neue Sweep‑Feature trägt.
- Wasserdichtigkeits‑Check und optionaler Manifold‑Repair‑Pass vor der SDF‑Konvertierung; UI‑Warnung bei kaputten COTS‑Meshes.
- Sweep‑Feature: Der Kernel erzeugt unsichtbar im Hintergrund ein geschlossenes Dreiecks‑Mesh des Sweeps (Profil entlang Pfad, inklusive Twist) mit klassischen Polygon‑Algorithmen und hängt das Resultat als Mesh‑to‑SDF‑Knoten in den Baum. Pfad und Profil behalten ihre IDs im Feature‑Tree und bleiben nach‑bearbeitbar — der Sweep wird bei Änderung neu gebacken.
- Ghost Topology für Mesh‑to‑SDF‑Knoten: Da der Knoten keine „harte Alternative" hat, liefert der *logical path* für Picking die Dreiecke des zugrundeliegenden Input‑Meshes selbst zurück (sie sind mathematisch exakt zum Profil und Pfad verankert, im Gegensatz zum Dual‑Contouring‑Output der SDF‑Konvertierung).
- Tests: STL‑Import‑Rundlauf (Import → Boolean → Export, Goldfile), Sweep‑Goldfile entlang Kreisbogen und 3D‑Spline mit Twist, Performance‑Test gegen 100k‑Dreieck‑Input‑Mesh, Manifold‑Repair auf kaputtem COTS‑Download als Regressions‑Fixture.

---

*Ab hier beginnt die Backend‑Phase. Der clientseitige Modeller ist nach K9 funktional komplett; die folgenden drei Meilensteine ergänzen Server, Accounts und Sync, bevor M13 den Hardening‑Pass durchführt.*

**M11 — Backend‑Grundgerüst: Auth, User, leeres Projekt**
- `server/`‑Ordner mit Go + `chi` + `sqlc` + `golang-migrate` anlegen.
- `docker-compose.yml` mit MariaDB (Dev‑DB, persistiertes Volume), Server und Web. `just dev` startet jetzt den kompletten Stack.
- `users`‑Tabelle, Argon2id‑Hashing, Session‑Cookies.
- Endpunkte: `POST /api/auth/register`, `/login`, `/logout`, `GET /api/me`.
- Frontend: Registrierungs‑, Login‑, Logout‑Views (React‑Komponenten).
- `go test ./...` gegen echte MariaDB im CI; Integration‑Tests für Auth‑Flows, E2E‑Tests für die UI‑Pfade.

**M12 — Projekt‑CRUD, Sichtbarkeit, Sync**
- `projects`‑Tabelle, Endpunkte für Create/List/Read/Update/Delete.
- Dashboard: Projektliste, Neu, Umbenennen, Löschen, Sichtbarkeits‑Toggle.
- Öffentliche Galerie (read‑only View ohne Login).
- Sync‑State‑Machine (Clean, Dirty, Syncing, Error) im Frontend‑Store.
- `POST /api/projects/:id/snapshot` mit JSON‑Body (`Content-Encoding: gzip` optional auf HTTP‑Ebene).
- Gast‑Modus: der bisherige `localStorage`‑Stand wird beim ersten Login optional in den Account übernommen.
- Migration von reiner `localStorage`‑Persistenz auf Hybrid‑Modus: `localStorage` bleibt Offline‑Cache, Server wird kanonisch.
- E2E‑Tests für CRUD, Sichtbarkeits‑Transitionen, Reload‑Persistenz und Offline→Online‑Sync.

**M13 — Hardening und öffentliche Beta**
- Passwort‑Reset via E‑Mail.
- Quotas, Rate‑Limits‑Tuning.
- Performance‑Pass: WASM‑Profiling, Mesher‑Compute‑Paths, BVH‑Update‑Inkremente.
- Öffentliche Projektgalerie mit Thumbnails.
- Dokumentation, Self‑Host‑Anleitung.

---

### Phase 3 — Interaktions‑Export, Simulation, Physik

Diese Phase hebt FieldDraft vom reinen Konstruktionswerkzeug zur Entwicklungs‑Plattform für interaktive 3D‑Applikationen. Sie nutzt Fähigkeiten, die aus der SDF‑Architektur „gratis" abfallen — Kollisionsabfragen in O(1), netzfreie FEM, direkte Toolpath‑Generierung — und verpackt sie in User‑sichtbare Features. Die Reihenfolge ist grob priorisiert, aber weniger zwingend verkettet als K1–K13.

**P1 — Assemblies und Scene Graph**
- Paradigmenwechsel vom CSG‑Baum zum Scene Graph auf der Assembly‑Ebene: Jedes Einzelteil bleibt ein isolierter Part Studio mit eigenem CSG‑Baum, eigenem BVH und eigenem Mesh‑Cache. Die Assembly verknüpft diese Parts über Transformations‑Hierarchien und mechanische Freiheitsgrade (Mates, Joints).
- Mate‑Typen: Fixed, Revolute (1 Rotationsfreiheitsgrad), Prismatic (1 Translationsfreiheitsgrad), Cylindrical, Ball. Jeder Mate bindet zwei Mate‑Connectors aus den Part‑Studios, die in K9 eingeführt wurden.
- Hierarchische Bewegung: Dreht sich die Roboter‑Schulter, wandern Ellenbogen und Greifer als Kinder mit. Der Scene Graph ist parametrisch und debounced persistiert wie der Feature‑Tree.
- Tests: Goldfile‑Szenen für Roboterarm, Linearachse und Scherenmechanik; Mate‑Constraint‑Solver‑Test gegen Referenz‑Positionen.

**P2 — Kinematik: FK und IK**
- Forward Kinematics: Slider in der UI bewegen einzelne Joint‑Winkel, der Scene Graph rechnet die Transformations‑Kette.
- Inverse Kinematics: Der User gibt eine Zielkoordinate im Raum an, ein IK‑Solver im Rust‑Kernel berechnet die Joint‑Winkel. Joint‑Limits aus P1 werden als Constraints mitgegeben.
- Selbstkollisions‑Prüfung während der Bewegung: Die SDFs der beteiligten Parts liefern den Abstand zwischen beliebigen Punkten in O(1) — der IK‑Solver kann Pfade verwerfen, bei denen ein Part durch ein anderes hindurchschwingt.
- Tests: 6‑DoF‑Roboterarm greift eine Zielkiste, Kollision mit dem eigenen Sockel wird automatisch vermieden.

**P3 — Code‑First‑Export: React Three Fiber**
- Export‑Button erzeugt nicht nur ein STL, sondern ein Paket aus: optimierten Meshes (GLTF/DRACO), einer generierten `.tsx`‑Datei mit dem Scene Graph als R3F‑Komponentenbaum, Props für jeden Joint‑Winkel, und optional einem kleinen `leva`‑basierten Slider‑Panel.
- Beispiel‑Output: Ein `<RobotArm shoulderAngle={…} elbowAngle={…}/>`‑Root‑Element mit verschachtelten `<group rotation={…}>`‑Knoten, die die Kinematik aus P1/P2 nachbilden. Der Developer kann den Code direkt in eine eigene React‑App einbauen, ohne GLTF manuell animieren zu müssen.
- USP‑Formulierung: *FieldDraft exportiert Interaktion, nicht nur Geometrie.* Das ist der architektonische Hebel, der die Brücke vom CAD‑Werkzeug zum Web‑Native‑Entwicklungs‑Tool schlägt.
- Tests: Snapshot‑Test des generierten TSX‑Codes gegen Goldfiles, Laufzeit‑Test des exportierten Bundles in einer Dummy‑React‑App (Playwright).

**P4 — Physikalische Abfragen: Masse, Kollision, Toolpaths**
- Volumen und Massenschwerpunkt: Klassisch via Divergenztheorem auf dem gemesheten Anzeige‑Mesh berechnet — das SDF liefert kein exaktes Volumen ohne Monte‑Carlo‑Integration, aber das DC‑Mesh ist für Maker‑Präzision mehr als ausreichend. Dichte pro Material konfigurierbar, Ausgabe im Measure‑Panel.
- Kollisionserkennung als generisches O(1)‑Primitiv: Die SDF jedes Körpers beantwortet die Frage „liegt Punkt P im Inneren und wie tief?" direkt. Wird dem Assembly‑Modul als API freigegeben und ist die Grundlage für die IK‑Selbstkollisions‑Prüfung aus P2.
- SDF‑direkte CNC‑/Slicer‑Toolpaths: Werkzeugpfade als Iso‑Fläche `f(p) = werkzeugradius` — der Fräskopf fährt entlang dieser Iso‑Fläche, ohne Umweg über STEP oder G‑Code‑Nachbearbeitung. Im MVP als experimentelles CAM‑Modul für simple 2.5D‑Fräsvorgänge; aufwändige 5‑Achs‑Strategien bleiben explizit out of scope.
- Tests: Massenschwerpunkt‑Goldfile gegen analytisch berechnete Referenzkörper, Kollisions‑API‑Fuzz‑Test, Toolpath‑Visualisierung als Linien‑Overlay im Viewport.

**P5 — Netzfreie FEM via Finite Cell Method**
- Netzfreie Finite‑Element‑Simulation direkt auf einem regelmäßigen Voxel‑Gitter über dem SDF: Jede Zelle bekommt vom SDF einen Füllgrad (0 % bis 100 %), und die Belastungs‑Gleichungen werden auf diesem Gitter gelöst. Damit entfällt der fehleranfällige Tetraeder‑Mesher klassischer FEM‑Tools — komplexe Gyroid‑ und TPMS‑Strukturen aus K11 werden erst dadurch simulierbar.
- Lasten und Randbedingungen werden auf den B‑Rep‑Proxy gelegt (fixierte Face, aufgebrachte Kraft auf Face), damit sie parametrisch an den Modell‑IDs hängen und eine Parameter‑Änderung die Simulation‑Setups nicht invalidiert.
- Das Feature ist groß und steht bewusst am Ende der Phase. Realistisches Ziel: statische Linear‑Elastizität für einfache Leichtbau‑Parts, nicht Crash‑Simulation.
- Tests: Goldfile gegen analytische Referenzen (Biegebalken, Zugstab), Vergleich gegen eine CalculiX‑Rechnung für einen nicht‑trivialen Referenz‑Part.

---

**Spätere Meilensteine (M14+)**
- Loft, Curved Patterns, 3MF‑Export.
- **STEP‑Import** via OpenCASCADE im Web‑Worker oder `truck`‑Experiment, inklusive Mesh‑to‑SDF‑Konvertierung und Robustheits‑Pass gegen kaputte COTS‑Dateien. Bewusst weit hinten: erst wenn das MVP steht und es echte User‑Nachfrage gibt.
- **Direct‑Manipulation‑Modus** (Push/Pull auf B‑Rep‑Proxy‑Faces), sobald ein robustes Heuristik‑Modell für die Push/Pull‑zu‑Parameter‑Auflösung existiert.
- Kollaborative Sessions via CRDT (Yjs o. ä.).
- Optionale Ende‑zu‑Ende‑Verschlüsselung privater Projekte.
- STEP‑Export via B‑Rep‑Rekonstruktion.
- OAuth‑Login.

## 11. Offene Risiken

1. **Meshing‑Qualität an scharfen Kanten und Manifold‑Verletzungen:** Dual Contouring ist bekannt anspruchsvoll und erzeugt an Drei‑ und Vier‑Flächen‑Schnitten artefakt­anfällige Übergänge. Manifold‑Verletzungen (zwei Faces, die sich an einer einzigen Kante berühren) können beim STL‑Export Slicer‑Probleme erzeugen. Gegenmaßnahme: Manifold‑Variante des DC‑Algorithmus implementieren oder einen Post‑Processing‑Pass, der nicht‑manifolde Kanten entdeckt und durch Vertex‑Splitting auflöst. Goldfile‑Tests gegen kanonische Bauteile (z. B. drei orthogonal verschnittene Quader) erkennen Regressions früh.

2. **Provenance‑Epsilon‑Stabilität:** Die Klassifikation eines Punkts als „auf einer Edge" beruht auf `|f_A − f_B| < ε`. Ein zu großes ε liefert Falsch‑Positive (Faces werden fälschlich als Edges markiert), ein zu kleines verfehlt echte Edges in numerisch verrauschten Regionen. Gegenmaßnahme: ε proportional zur lokalen Mesh‑Auflösung wählen, nicht absolut; Property‑Tests, die für zufällige CSG‑Bäume die Konsistenz der Klassifikation gegen Referenz‑Berechnungen prüfen.

3. **Solver‑Portierung C++ → Rust:** Bei Solvern geht es um numerische Stabilität (Floating‑Point‑Epsilons, Konvergenz). Ein winziger Rundungsunterschied in Rust kann eine Skizze „over‑constrained" melden lassen, obwohl sie es nicht ist. Gegenmaßnahme: differential Fuzz‑Testing gegen die C‑Referenz und harte Goldfiles. **Fallback:** `libslvs` bleibt C++ und wird zusammen mit dem Rust‑Kernel in ein gemeinsames WASM‑Modul kompiliert (siehe §4.4).

4. **BVH‑Invalidierung und Re‑Meshing‑Kaskaden:** Eine Parameter‑Änderung an einem hoch referenzierten Variablen‑Knoten kann viele Bodies invalidieren. Auch wenn jeder Body‑Re‑Mesh asynchron im Web Worker läuft, summieren sich die Latenzen bei großen Baugruppen zu sichtbaren Wartezeiten, ähnlich dem Onshape‑„Regenerating"‑Spinner. Gegenmaßnahme: inkrementelles BVH‑Update statt Komplett‑Rebuild, Sub‑Tree‑Hashing zum Erkennen, ob ein Body wirklich invalidiert wurde, Priorisierung des aktiven Bodies vor Context‑Bodies. Akzeptanz: ≥1 s Wartezeit bei 100‑Body‑Szenen ist im MVP OK.

5. **Vertex Blending / Setbacks (Zimmerecke):** Eine Spatial Mask muss am Kantenende irgendwo aufhören. Hartes Abschneiden der Maske am Boden einer Zimmerecke verletzt die Lipschitz‑Bedingung — der Mesher erzeugt Löcher, Zacken oder doppelte Normalen. Weicher Auslauf hält das Feld stetig, lässt die Fase/Rundung aber organisch in die angrenzenden Flächen ausschmelzen, statt in einem messerscharfen Dreieck zu enden. FieldDraft wählt den weichen Auslauf als Default; scharfe Kanten‑Enden im Stil klassischer B‑Rep‑Kernel sind in dieser Architektur nicht erreichbar, und automatisch generierte CSG‑Schnittkörper als Ersatz sind bewusst verworfen (sie funktionieren nur für triviale, gerade Kanten und wären für den allgemeinen Fall kryptisch). User, die exakt geometrische Enden brauchen, müssen den Fasen‑/Stufenkörper manuell im Sketcher modellieren und per harter Boolean abziehen — das ist ein bewusster Workaround, keine System‑Automatik. Goldfile‑Tests fixieren den Auslauf‑Look als Regression‑Baseline.

6. **Masken‑Stacking‑Performance:** Jedes Fillet und jeder Chamfer fügt dem CSG‑Baum einen Masken‑Test plus Interpolations‑Operator hinzu. Bei 50 Verrundungen an einem Gehäuse läuft die Punkt‑Auswertung naiv linear mit der Anzahl der Masken. Gegenmaßnahme: Jede Masken‑SDF erhält eine eigene AABB im BVH — außerhalb der Masken‑Hülle wird der Mix‑Operator nicht ausgewertet, der Punkt fällt transparent auf den harten Unteroperator zurück. Die Rechenlast skaliert damit wieder logarithmisch mit der Anzahl räumlich nahe beieinanderliegender Masken, nicht mit der Gesamtanzahl im Baum.

7. **Masken‑Kollision benachbarter Fillets:** Überlappen zwei Fillet‑Masken räumlich (z. B. zwei parallele Kanten im Abstand von 3 mm mit Radius 2 mm), interferieren ihre Mix‑Operatoren unvorhersehbar an der Schnittstelle — mal gewinnt A, mal B, und in der Übergangszone entstehen Artefakte. Gegenmaßnahme: Overlap‑Detection beim Anlegen einer neuen Maske mit UI‑Warnung und Vorschlag zur Radius‑Reduktion; deterministische Interferenz‑Regel („innerer Operator gewinnt") als Fallback, wenn der User die Warnung ignoriert; Property‑Tests mit zufälligen Masken‑Paarungen gegen Referenz‑Auswertungen.

8. **Edge Chaining tangentialer Kanten (MVP deferred):** In Onshape wählt der User eine Kante, und das System findet automatisch alle tangential angrenzenden Kanten und zieht das Fillet als geschlossene Schleife herum. Im SDF‑Modell müsste die Spatial Mask einer durch mehrere CSG‑Schnitte erzeugten, gekrümmten 3D‑Kurve folgen — analytisch schwer, weil der exakte Kurvenverlauf nicht ohne eine aufwendige Edge‑Propagation entlang tangentialer Proxy‑Nachbarn vorliegt. MVP‑Ausweg: Fillet/Chamfer wirken nur auf einzelne selektierte Edge‑Segmente des Proxys; tangentiales Chaining wird durch Mehrfach‑Selektion simuliert. Automatisches Chaining ist ein Later‑Feature und wird erst angegangen, sobald die Proxy‑Edge‑Propagation robust steht.

9. **Sketch‑Ebenen auf gekrümmten Faces (deferred):** Klickt der User in der Zukunft auf eine gekrümmte Face (z. B. eine Revolve‑Außenseite), gibt es keine kanonische planare Ebene, an die sich die neue Skizze auto‑locken ließe. Die geplante Lösung über „unconstrained planes" mit harten Maßen zum 3D‑Origin ist in K6 noch nicht enthalten und wird erst implementiert, wenn der MVP steht und konkrete Anwendungsfälle es erzwingen.

10. **STEP‑Import als B‑Rep‑Mauer:** Mesh‑to‑SDF‑Konvertierung von STEP‑Daten setzt wasserdichte Meshes voraus — COTS‑STEP‑Dateien aus Fernost sind es oft nicht. OpenCASCADE in WASM kostet 20–30 MB Payload. Deshalb ist STEP‑Import bewusst aus K9 herausgeschoben (siehe M14+). Im MVP reicht STL/OBJ‑Referenz, bei Bedarf extern konvertiert.

11. **WebAssembly‑Speicherlimits:** 2–4 GB Linear Memory pro WASM‑Instanz reicht für Maker‑Bauteile sehr gut, könnte aber bei sehr großen Baugruppen mit hochaufgelösten Body‑Meshes ein Thema werden. Sparse‑Grids (z. B. OpenVDB‑Ansatz) im Mesher und Mesh‑Decimation für Context‑Bodies sind die Plan‑B‑Pfade. Konkrete Schätzung: ein typisches Maker‑Bauteil mit 100 k–300 k Dreiecken belegt unter 5 % des verfügbaren WASM‑RAMs.

12. **WebGPU‑Browser‑Support:** WebGPU ist in Chromium und Safari stabil, in Firefox teilweise. Da FieldDraft bewusst kein WebGL‑Fallback pflegt, ist die Browser‑Matrix enger. Gegenmaßnahme: klare System‑Requirements‑Seite und eine Feature‑Detection beim Ladezeitpunkt.

13. **Auto‑Save‑Konflikte:** Wenn derselbe User zwei Tabs parallel öffnet, können lokale Stände divergieren. Gegenmaßnahme: Last‑Writer‑Wins mit Warnung plus versionierte Snapshots im Backend, damit nichts verloren geht.

## 12. Harte Architektur‑Grenzen (permanente Scope‑Ausschlüsse)

Die folgenden drei Limitierungen sind keine Risiken im Sinne von §11, sondern **permanente, bewusst akzeptierte Architektur‑Grenzen**. Sie ergeben sich aus der Entscheidung für einen SDF‑basierten Kern und sind in diesem Kern nicht auflösbar, ohne einen vollständigen B‑Rep/NURBS‑Zweitkernel danebenzustellen — was den USP des Systems zerstören würde. Wer diese Features braucht, braucht nicht FieldDraft.

1. **STEP‑/NURBS‑Export für die klassische Fertigungsindustrie.** Die globale Fertigungskette (Spritzguss, CNC‑Fräserei in klassischen CAM‑Systemen, Blechbearbeitung, Toleranz‑Management in Mastercam/Edgecam) spricht genau eine Sprache: STEP mit exakten B‑Rep‑Flächen und NURBS‑Oberflächen. FieldDraft kann Meshes exportieren (STL, GLB, 3MF), aber kein STEP mit parametrischen Flächen. Ein „Auto‑Surfacing"‑Algorithmus, der aus einem SDF‑Mesh wieder tangentenstetige NURBS‑Patches rekonstruiert, ist eines der schwersten ungelösten Probleme der Computergrafik — die existierenden Lösungen liefern fragmentierte „Patchworks", die Industriedesigner in Sekunden als ungeeignet erkennen. FieldDraft geht diesen Weg bewusst **nicht**. Die strategische Alternative lautet: **Direct Digital Manufacturing** — additive Fertigung (FDM, SLA, SLS), SDF‑direktes CAM (siehe Phase 3, P4), Dentaltechnik‑ähnliche Workflows, bei denen das B‑Rep‑Format nie zwischengeschaltet wird.

2. **Normgerechte technische Zeichnungsableitung.** Verträge, Qualitätskontrolle und Werkstatt‑Kommunikation in klassischen Branchen laufen zu 90 % über 2D‑Zeichnungen nach DIN ISO 128: Silhouetten‑Projektionen, Schnittansichten, Bemaßungen mit Passungen, Oberflächenangaben. Eine saubere 2D‑Ableitung setzt voraus, dass der Kernel Silhouetten als analytische Vektor‑Kurven aus dem CSG‑Baum extrahieren kann (Raytracing der Flächen‑Gradienten, nicht der Mesh‑Dreiecke) — theoretisch machbar, aber bei verrundeten `smin`‑Übergängen und Ghost‑Topology‑Edges in der Praxis kaum in saubere DIN‑konforme Vektor‑Konturen zu übersetzen, weil der Unterschied zwischen visual path und logical path genau hier aufeinanderprallt. Grobe 2D‑Schnitte und Projektionen für Lasercutter sind weiterhin möglich (§7 DXF/SVG‑Export), normgerechte Fertigungs‑Zeichnungen nicht.

3. **ISO‑Toleranzen, Passungen, Metrologie.** In SolidWorks oder Onshape definiert man eine Welle nicht als `10 mm`, sondern als `10.000 mm h7` — eine Passungstoleranz, die in der Qualitätskontrolle gegen eine exakte theoretisch‑mathematische Referenzfläche mit einer eindeutigen, persistenten ID gemessen wird. Unsere Ghost Topology hält Original‑Kanten identifizierbar, aber die Toleranz‑/Metrologie‑Welt verlangt mehr: referenzierbare Flächen mit GD&T‑Annotationen (Form‑, Lage‑, Richtungs‑Toleranzen), die über den gesamten Produktlebenszyklus bis zur Laserscanner‑Messung bestehen bleiben. Dieses Datenmodell ist nicht mit dem Provenance‑basierten Proxy vereinbar und wird in FieldDraft nicht nachgebaut. Für Maker‑, Prototyping‑ und 3D‑Druck‑Toleranzen reicht die numerische Präzision des Kernels vollkommen; für Serienfertigung mit SPC‑Qualitätskontrolle ist das Tool nicht gedacht.

**Positionierung:** Diese drei Ausschlüsse sind keine Mängel, sondern die Kehrseite der Architektur‑Stärken (unkaputtbare Booleans, native TPMS/Gyroid/Displacement, SDF‑direktes CAM, R3F‑Code‑Export). Marketing und Doku sollten FieldDraft konsequent als **„CAD für Direct Digital Manufacturing"** positionieren — Maker, Hardware‑Startups, Robotik‑Entwickler, Industriedesigner, 3D‑Druck‑Pioniere — und die klassische Maschinenbau‑Welt (Motorenblöcke, Spritzgussformen, Dreh‑/Frästeile mit ISO‑Passungen) explizit **nicht** als Zielgruppe.

## 13. Verwandtes / Inspiration

- **Onshape** — UX‑Referenz, Feature‑Tree‑Vorbild.
- **Shapr3D** — Vorbild für „Direkt auf Fläche skizzieren" ohne expliziten Sketch‑Button.
- **Plasticity** — Geschwindigkeit des Direct Modeling, Negativbeispiel für UI‑Zwang.
- **SolveSpace** — 2D‑Solver‑Basis.
- **Manifold** — moderne, robuste Mesh‑Booleans; als potenzielle Ergänzung zum SDF‑Pfad.
- **nTopology / nTop** — kommerzielles SDF/Implicit‑CAD der Luft‑ und Raumfahrt, Beleg dafür, dass der Ansatz industrietauglich ist.
- **MagicaCSG, Dreams (PS)** — kreative SDF‑Modeller als UX‑Inspiration.
- **Fornjot, Truck** — Rust‑native B‑Rep‑Kernel, als Fallback oder Ergänzung interessant.

## 14. Namensgebung und Schwesterprojekt

- **FieldDraft** — Distanz‑**Field**‑basiertes CAD‑**Draft**‑Tool.
- **BeamDraft** — eigenständiges Laser‑Projektor‑Projekt, das denselben 2D‑Constraint‑Solver als Kern nutzt. Durch die gemeinsame geometrische Engine entsteht ein strategischer Hebel: Ein investierter Entwicklungs‑Euro dient zwei Produkten.

## 15. Domains

Die TLDs .eu, .de und .xyz sind frei.
FieldDraft.com ist bereits registriert („for sale“ bei GoDaddy). 
Markenrechtlich gibt es keine Eintragungen.
Keine Github- oder NPM-Projekte mit diesem Namen.

## 16. Literatur und Referenzen für Implementierungs‑Agenten

Diese Sektion ist Pflichtlektüre für jeden Pair‑Programming‑Agent, der an der Kernel‑Phase (K1–K10) arbeitet. Die folgenden Quellen sind kuratiert und decken die zentralen Datenstrukturen und Algorithmen ab. Wer einen K‑Slice anfasst, ohne die zugehörigen Abschnitte gelesen zu haben, baut zwangsläufig in die falsche Richtung.

### 16.1 Dual Contouring

- [Ju, Losasso, Schaefer, Warren — „Dual Contouring of Hermite Data" (SIGGRAPH 2002, PDF)](https://www.cs.rice.edu/~jwarren/papers/dualcontour.pdf) — Originalarbeit, definiert das Verfahren, das im FieldDraft‑Kernel zur Mesh‑Extraktion aus dem SDF‑Feld eingesetzt wird. Pflichtlektüre für alle, die am Vernetzer arbeiten.
- [Schaefer, Ju, Warren — „Manifold Dual Contouring" (TVCG 2007, PDF)](https://people.engr.tamu.edu/schaefer/research/dualsimp_tvcg.pdf) — Erweiterung mit Garantie für mannigfaltige Meshes auch unter adaptiver Vereinfachung; relevant, sobald wir Octree‑Adaption brauchen.
- [swiftcoder/isosurface (Rust crate)](https://github.com/swiftcoder/isosurface) — Wartungsaktive, dependency‑freie Rust‑Implementierung von Marching Cubes und Dual Contouring; gute Referenz für Datenstrukturen und Octree‑Layout.
- [nickgildea/fast_dual_contouring](https://github.com/nickgildea/fast_dual_contouring) — Praxiserprobte Variante ohne Octree, demonstriert eine flache Datenstruktur, die sich gut auf WASM portieren lässt.

### 16.2 Quadratic Error Function (QEF) Solver

- [BorisTheBrave — „Dual Contouring Tutorial"](https://www.boristhebrave.com/2018/04/15/dual-contouring-tutorial/) und [zugehöriger QEF‑Code](https://github.com/BorisTheBrave/mc-dc/blob/master/qef.py) — Die kanonische didaktische Quelle. Erklärt QEF‑Aufbau, SVD‑Lösung und Bias‑Term, jeweils mit lauffähigem Python.
- [Lindstrom — „Out‑of‑Core Simplification of Large Polygonal Models" (SIGGRAPH 2000, PDF)](http://evasion.imag.fr/Membres/Franck.Hetroy/Teaching/Geo3D/Articles/lindstrom2000.pdf) — Begründet die SVD‑basierte robuste QEF‑Minimierung, die bis heute Stand der Technik ist.
- [Matt Keeter — „QEF Explainer"](https://www.mattkeeter.com/projects/qef/) und [nickgildea/qef (SVD‑Implementierungen)](https://github.com/nickgildea/qef) — Kompakte numerische Erklärung plus C/GLSL/OpenCL‑Solver, an denen sich eine numerisch stabile Rust‑Variante gut spiegeln lässt.

### 16.3 SDF‑Primitive (3D)

- [Inigo Quilez — „3D Signed Distance Functions"](https://iquilezles.org/articles/distfunctions/) — De‑facto‑Referenz für exakte 3D‑SDF‑Formeln (Box, Zylinder, Kegel, Torus, Kapsel, …). Quelle der Wahrheit für alle Primitive im FieldDraft‑Kernel.
- [Inigo Quilez — „Raymarching Distance Fields"](https://iquilezles.org/articles/raymarchingdf/) — Hintergrund zu Distanzeigenschaften (Lipschitz‑Bound, exakte vs. begrenzende SDFs); wichtig, weil unser BVH und der Picking‑Raymarcher nur mit echten SDFs korrekt funktionieren.
- [Inigo Quilez — „SDF Bounding Volumes"](https://iquilezles.org/articles/sdfbounding/) — Beschreibt das Sketch‑Extrude‑ und Revolve‑Pattern (2D‑Profil → 3D‑Volumen) inklusive Bounding‑Box‑Ableitung, das genau dem FieldDraft‑Feature‑Tree entspricht.

### 16.4 SDF‑Primitive (2D) und Komposition

- [Inigo Quilez — „2D Distance Functions"](https://iquilezles.org/articles/distfunctions2d/) — Vollständige Sammlung exakter 2D‑SDFs (Linie, Kreis, Bogen, Ellipse, Polygon) mit Shadertoy‑Demos. Direkte Vorlage für den 2D‑Sketch‑Solver.
- [Inigo Quilez — „Distance + Gradient 2D SDF"](https://iquilezles.org/articles/distgradfunctions2d/) — Liefert analytische Gradienten zusammen mit der Distanz; entscheidend, damit Hermite‑Daten für Dual Contouring direkt aus dem 2D‑Profil propagiert werden können.

### 16.5 BVH‑Beschleunigung für SDF‑/CSG‑Trees

- [Inigo Quilez — „SDF Bounding Volumes"](https://iquilezles.org/articles/sdfbounding/) — Erklärt, wie man Subtrees mit AABBs ummantelt und Punktanfragen früh abbricht. Direkt anwendbar auf unseren CSG‑Tree.
- [PBR Book — Bounding Volume Hierarchies (3rd ed.)](https://pbr-book.org/3ed-2018/Primitives_and_Intersection_Acceleration/Bounding_Volume_Hierarchies) — Goldstandard für BVH‑Konstruktion (SAH, Traversal). Auch wenn das Buch ray‑zentriert ist, sind Aufbau und Speicher‑Layout 1:1 für Punktanfragen verwertbar.
- [Liktor et al. — „Synchronized‑Tracing of Implicit Surfaces" (arXiv 2304.09673)](https://arxiv.org/pdf/2304.09673) — Modernes Paper über räumliche Pruning‑Strukturen für CSG‑/Blobtree‑Auswertung; gute Referenz für Pruned‑Tree‑Auswertung pro Region.

### 16.6 CSG‑Provenance und Feature‑Labeling

- [Rossignac, Voelcker — „Active Zones in CSG" (ACM TOG 1989)](https://dl.acm.org/doi/abs/10.1145/49155.51123) — Klassiker; definiert „active zones" als räumliche Regionen, in denen ein Primitiv die Boolean‑Auswertung dominiert. Theoretisches Fundament für unser Contributor‑Tracking.
- [Hoffmann — „How Solid Is Solid Modeling?" (PDF)](https://www.cs.purdue.edu/cgvlab/www/resources/papers/Hoffmann-Workshop_on_App_Comp_geo-1996-How_SolidIs_Solid_Modeling.pdf) — Hoffmanns Übersicht zu Boundary Evaluation, Feature‑Persistenz und numerischer Robustheit. Kontextpapier für alle persistenten ID‑Schemata.
- [Matt Keeter — „Massively Parallel Rendering of Complex Closed‑Form Implicit Surfaces" (libfive Kernel‑Paper, PDF)](https://www.mattkeeter.com/research/kernels.pdf) und [libfive Repository](https://github.com/libfive/libfive) — Modernste implizite CAD‑Kernel‑Architektur in der Praxis; libfive trackt Sub‑Expression‑Dominanz pro Region und ist die beste lebende Referenz für FieldDraft.

### 16.7 SolveSpace / libslvs

- [solvespace/solvespace (GitHub)](https://github.com/solvespace/solvespace) und [include/slvs.h (Library‑API)](https://github.com/solvespace/solvespace/blob/master/include/slvs.h) — Quellcode des einzigen erprobten Open‑Source‑Constraint‑Solvers für parametrisches CAD; relevant, sobald der 2D‑Sketcher echte Constraints bekommt.
- [SolveSpace Reference (Westhues)](https://solvespace.com/ref.pl) — Westhues' originale Notizen zur Architektur (Newton‑Verfahren, symbolisches Algebra‑System, Jacobi‑Aufbau).

### 16.8 wgpu (Rust‑WebGPU‑Bindings)

- [wgpu.rs (offizielle Projektseite)](https://wgpu.rs/) und [docs.rs/wgpu](https://docs.rs/wgpu/) — Maßgebliche API‑Dokumentation für den Rust‑WebGPU‑Stack, den der FieldDraft‑Viewport nutzt.
- [sotrh — „Learn Wgpu"](https://sotrh.github.io/learn-wgpu/) — Das De‑facto‑Lehrbuch zur Bibliothek; deckt Pipelines, Bind Groups, Compute‑Shader und WASM‑Targeting ab.
- [W3C WebGPU Specification](https://www.w3.org/TR/webgpu/) und [W3C WGSL Specification](https://www.w3.org/TR/WGSL/) — Normative Quelle für Semantik von Buffers, Bind Groups und Shadern; bei jeder Unklarheit gilt die Spec, nicht der Wrapper.

### 16.9 wasm‑bindgen Zero‑Copy‑Patterns

- [The wasm‑bindgen Guide](https://rustwasm.github.io/docs/wasm-bindgen/) — Offizielle Dokumentation; insbesondere die Abschnitte über `js_sys::Float32Array::view` zeigen, wie man ohne Kopie auf den linearen WASM‑Speicher mappt.
- [rustwasm/wasm‑bindgen Issue #2456 — „need example of sharing memory"](https://github.com/rustwasm/wasm-bindgen/issues/2456) — Konkrete Beispiele zur Lebensdauer‑Verwaltung von Views (Achtung: kein `alloc` während ein View existiert), exakt das Pattern, das FieldDraft für Mesh‑Buffer braucht.

### 16.10 React Three Fiber + BufferGeometry

- [React Three Fiber — „Objects, Properties and Constructor Arguments"](https://r3f.docs.pmnd.rs/api/objects) — Offizielle R3F‑Doku zum `attach="attributes-position"`‑Pattern, mit dem externe Float32Arrays an `<bufferGeometry>` angebunden werden.
- [three.js — BufferGeometry (offizielle Doku)](https://threejs.org/docs/#api/en/core/BufferGeometry) und [Custom BufferGeometry Manual](https://threejs.org/manual/en/custom-buffergeometry.html) — Maßgebliche Beschreibung von Attributen, Indices, Bounding‑Spheres und Update‑Flags. Pflicht, bevor jemand `setAttribute` aufruft.

### 16.11 Sharp‑Feature‑Preservation in Dual Contouring

- [Ju et al. — „Dual Contouring of Hermite Data" (siehe §16.1)](https://www.cs.rice.edu/~jwarren/papers/dualcontour.pdf) — Originaldarstellung der scharfen Kanten über QEF‑Minimierung mit Edge‑Normalen.
- [Schaefer, Warren — „Dual Contouring: The Secret Sauce" (PDF)](https://people.eecs.berkeley.edu/~jrs/meshpapers/SchaeferWarren2.pdf) — Zusatzpapier mit Implementierungsdetails (Bias‑Term, Constraint Box, Feature‑Klassifikation), die im Originalpaper fehlen.
- [Ho et al. — „Cubical Marching Squares" (PDF)](https://www.csie.ntu.edu.tw/~cyy/publications/papers/Ho2005CMS.pdf) — Alternative Sicht auf adaptive Feature‑Erhaltung; nützlich für Heuristiken bei mehreren Kanten in einer Zelle.

### 16.12 Polyline‑Approximation von Splines

- [Cao, Wang — „Approximate Computation of Curves on B‑Spline Surfaces" (PDF)](https://www.cs.cornell.edu/~caosong/index_files/surface_cad08.pdf) — Beschreibt adaptive Subdivision unter Hausdorff‑Toleranz; Grundlage für chord‑error‑getriebenes Sampling.
- [Liutaio Mottola — „Calculating Arc Parameters" (Sagitta‑Formeln)](https://liutaiomottola.com/formulae/sag.htm) — Praktische Formelsammlung zu Sagitta/Versine; direkt verwendbar für Bogen‑zu‑Polyline‑Konvertierung im Sketch‑Modul.
- [Lee Mac — „Bulge Conversion Functions"](https://www.lee-mac.com/bulgeconversion.html) — Klassische Notizen zur Bulge/Sagitta‑Beziehung; nützlich, weil DXF‑Importe Bögen genau in dieser Form transportieren.

