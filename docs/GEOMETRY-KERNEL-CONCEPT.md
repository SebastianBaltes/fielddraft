# FieldDraft Geometrie-Kernel-Konzept

> **Status 2026-05-31 — Architektur-Pivot (siehe DL-46).** Dieser Doc stammt vom Branch
> `agents/fielddraft-a` und enthält den vollständigen Design-Log DL-1…45. Die **Half-Space-only-Leaf-Node-Architektur**
> (Abschnitt „Halbraum-Architektur": *„Das System kennt keine geschlossenen 3D-Körper … die Basisbausteine sind
> ausschließlich unendliche Halbräume"*) ist **überholt** — naive Half-Spaces sprengen die SDF (512-Opcode-Cap,
> ganzer Baum pro Pixel) und skalieren nicht auf CAD-Größe. Die neue Richtung auf `feat/sketch-sdf-brep`:
> **lokal gecachte Per-Body-SDF (Sketch-Körper, 2D-BVH) als billige Basis, der teure globale Octree-/Boolean-Schritt
> nur in Body-Überlappungszonen.** Was **gültig bleibt:** die Rolle von SDF/B-Rep, Duale Zahlen, die
> Octree→Extraction→Weaving-Pipeline, persistentes Naming (D.1) und STEP-Export (D.4) — sie werden auf
> Sketch-Körper statt auf Half-Space-Bäume angewandt. Details: `docs/plans/SKETCH-SDF-BREP-PLAN.md` + DL-46.

FieldDraft ist ein hybrider CAD-Kernel, der die Robustheit von Signed Distance Fields (SDFs) mit der industriellen Standard-Topologie von Boundary Representations (B-Rep) vereint.

**Designprinzip: Analytische Exaktheit statt Approximation.** Im aktuellen FieldDraft werden Sketch-Kurven (Splines, Ellipsen, Bezier) vor der Extrusion in Liniensegmente approximiert. Das neue Konzept eliminiert diese Approximation vollständig: Alle Kurven werden als analytische SDF-Primitiven (mit exakter Distanzberechnung und exaktem Gradient via Duale Zahlen) ausgewertet und fließen als exakte Splines und Trimmed Surfaces in das B-Rep ein. Die Geometrie ist analytisch exakt innerhalb der Octree-Auflösung — von der Sketch-Definition bis zum STEP-Export.

## Rolle von SDF und B-Rep

**SDF** bildet die Basis und wird benötigt für:

* Rendering der Körper (Raymarching)
* Operationen auf Körper: Boolean, SMin, Rotation, Sweep, Extrude, Loft, Spiral
* Mesh-Export (3D-Druck via Dual Contouring / Marching Cubes)

**B-Rep** wird aus SDF und Half-Space CSG abgeleitet und benötigt für:

* Konstruktionsprozess: Flächen selektieren und extrudieren, Kanten selektieren und mit Fillets abrunden
* Visuelle und selektierbare Darstellung der Kanten und Flächen
* Projektion von Körper-Kanten in Sketches (Ebene)
* Ableitung des Schnittumrisses (Intersection) von Körpern mit Sketches (Ebene)
* STEP-Export (NURBS-fähige Flächen; Gyroids/TPMS werden als tessellierte Approximation exportiert)

**STEP-Import** ist unabhängig von B-Rep: Importierte STEP-Dateien werden tesselliert und als MeshSDF (Volumen-3D-Textur) oder Render-Only-Mesh eingehängt.

***

## Halbraum-Architektur (Half-Space CSG)

Das System kennt keine geschlossenen 3D-Körper (wie Würfel oder Zylinder) mit intrinsischen Kanten. Die Basisbausteine (Leaf Nodes) des Systems sind ausschließlich **unendliche, weiche, C∞-stetige Flächen**. Alle scharfen Kanten entstehen erst durch Boolesche Operationen zwischen diesen Flächen.

### Leaf-Node-Kategorien

**B-Rep-fähige Leaf Nodes** (C∞-stetig, Branch-Tracking-fähig):

* Unendliche Ebenen, Röhren (Zylindermäntel), Sphären, Kegel, Tori
* Gyroids und andere TPMS (Triply Periodic Minimal Surfaces)
* Atomare 2D-Skizzen, bestehend aus Segmenten von Linien, Ellipsen, Splines oder Bezier-Kurven, auch konkav und mit Löchern

**Reine SDF Leaf Nodes** (nicht B-Rep-fähig, aber für SDF-Operationen zugänglich):

* Fraktale (Mandelbulb, Sierpinski) — unendliche Topologie verhindert B-Rep-Extraktion
* Importierte Meshes, die in SDF konvertiert wurden (Volumen-3D-Textur)

**Render-Only Leaf Nodes:**

* Importierte Meshes, die unverarbeitet und read-only angezeigt werden

### Primitive durch Halbräume

Geschlossene Körper werden aus unendlichen Basisflächen zusammengesetzt:

* **Würfel** \= `max(Ebene_+x, Ebene_-x, Ebene_+y, Ebene_-y, Ebene_+z, Ebene_-z)`
* **Zylinder** \= `max(UnendlicherZylindermantel, Ebene_oben, Ebene_unten)`
* **Extrusion** \= `max(f_2D(p_x, p_y), p_z - höhe, -p_z)`

Die Kanten eines Würfels werden so nicht durch intrinsische Geometrie erzeugt, sondern durch Branch-Kollisionen zwischen den sechs Ebenen-Halbräumen — identisch zum Mechanismus, der auch CSG-Verschneidungen zwischen verschiedenen Körpern findet.

### Rotationskörper

2D-Profile werden durch zylindrische Koordinatentransformation zu 3D-Körpern:

```
f_3D(x, y, z) = f_2D(sqrt(x² + y²), z)
```

Dabei wird eine vertikale 2D-Linie zum Zylinder, eine horizontale zur Ebene, eine schräge zum Kegel, ein Kreis zum Torus.

### Konkave Skizzen und Löcher

Atomare 2D-Sketch-Nodes berechnen intern:

1. **Unsigned Distance:** Minimum der Abstände zu allen Segmenten (Linie, Bezier, Kreisbogen etc.)
2. **Vorzeichen:** Even-Odd-Rule (Raycasting / Winding Number) — ein Strahl vom Punkt ins Unendliche zählt Kreuzungen mit dem Pfad. Ungerade \= innen (negativ), gerade \= außen (positiv).

Löcher werden automatisch korrekt behandelt: Ein Punkt im Loch kreuzt die Loch-Grenze (1×) und die Außen-Grenze (1×) \= 2 Kreuzungen (gerade) → außen.

***

## Kanten-Monopol durch Boolesche Logik

Jede einzelne Kante im gesamten System — sei es die Fase eines Sweep-Körpers, die Außenkante einer Extrusion oder die Verschneidung eines Gyroid-Infills — entsteht mathematisch vollkommen gleichberechtigt: durch die Kollision weicher Basisflächen mittels `min` (Union) oder `max` (Intersection/Difference).

Es gibt keine zwei Klassen von Kanten (native vs. generierte), die nachträglich zusammengenäht werden müssten. Das eliminiert das klassische "Stitching"-Problem von B-Rep-Kerneln.

***

## Duale Auswertung und Branch-Tracking

Der Kernel wertet den Raum mit **Dualen Zahlen 1. Ordnung** (Crate `num-dual`) aus. Für die B-Rep-Extraktions-Pipeline (Phasen 1–4) wird die 2. Ableitung (Hesse-Matrix) nicht benötigt. Zukünftige Features (krümmungsadaptive Octree-Verfeinerung, Medial-Axis-Analyse) können Krümmungsinformation erfordern — in diesem Fall liefern Finite Differences auf den exakten Gradienten gute Schätzungen, ohne dass Hyper-Dual-Zahlen im gesamten Evaluierungspfad nötig werden.

Jeder ausgewertete Punkt liefert simultan:

* **Distanz** f(p) — Realteil der dualen Zahl
* **Gradient** ∇f(p) — Dualteil, exakter Oberflächen-Normalenvektor
* **Branch-ID** — welche Basisfläche für diesen Raumabschnitt dominant ist

### Branch-Tracking

Bei jeder `min`/`max`-Operation wird nicht nur der Wert, sondern auch die ID des dominierenden Operanden weitergereicht. Bei einem Würfel aus 6 Ebenen meldet ein Punkt nahe der Vorderseite z.B. Branch-ID `Ebene_+z`, ein Punkt nahe der Oberseite `Ebene_+y`.

Bei atomaren 2D-Skizzen meldet der Node die ID des **nächstgelegenen Segments** (z.B. `Linie_12` oder `Spline_13`), nicht nur die Sketch-ID.

### Verwerfungen im SDF-Raum

Jede `min`/`max`-Operation teilt den Raum in Regionen, in denen jeweils ein Operand dominiert. Die Grenzfläche zwischen diesen Regionen — wo `a(p) = b(p)` gilt — ist die **Verwerfung** (Voronoi-Trennfläche).

Eigenschaften der Verwerfung:

* Sie verläuft zwingend exakt durch die physische Kante des Körpers (dort gilt `a(p) = b(p) = 0`)
* Sie erstreckt sich als unendliche Fläche nach außen (in den leeren Raum) und nach innen (Medial Axis / Skelett)
* Abseits der Verwerfung (selbst bei δ \= 10⁻¹⁰) ist das Feld absolut stabil
* Auf der Verwerfung selbst springt der Gradient diskontinuierlich (C¹-Unstetigkeit)

**Konsequenz:** Gradientenabstieg kann Kanten nicht direkt finden — in Region A zeigt der Gradient auf Oberfläche A, in Region B auf Oberfläche B. Kein Gradient zeigt jemals auf die Kante selbst. Stattdessen nutzt der Algorithmus die stabilen Flanken beiderseits der Verwerfung.

### Warum nicht smin für scharfe Kanten?

`smin` (Smooth Minimum) macht das Feld global C∞-stetig, erzeugt aber:

* Einen extrem schmalen Krümmungs-Spike (proportional zu 1/k), der Gradientenabstieg destabilisiert
* Eine physische Abrundung (Fillet mit Radius ≈ k) — die exakte Kante existiert nicht mehr
* Verschmierte Branch-IDs in der Übergangszone

**Regelung:** Außerhalb der Blend-Zone (wo `|a - b| > k`) dominiert klar ein Operand — dort wird dessen Branch-ID weitergereicht. Innerhalb der k-Zone (wo `|a - b| ≤ k`) werden die Branch-IDs beider Operanden zu einer **neuen, gemeinsamen Blend-ID** verschmolzen. Der Octree erkennt diese Zone als einen einzigen Branch und generiert dort keine scharfe Kante. Die Übergangszone wird im B-Rep als eine einzige, glatte Freiformfläche (NURBS-Patch) extrahiert.

***

## Vorverarbeitung (Phase 0)

### G1-Preprocessing

Bevor ein 2D-Sketch in den SDF-Baum eingehängt wird, werden alle Verbindungspunkte zwischen Segmenten auf **G1-Stetigkeit** (geometrische Tangentenstetigkeit) geprüft:

```
Wenn 1.0 - (v̂₁ · v̂₂) < ε    →    G1-stetig (kein Knick)
```

Dabei sind v̂₁ und v̂₂ die **normalisierten** Tangentenvektoren am Verbindungspunkt.

**Problem ohne Preprocessing:** Zwei tangential verbundene Segmente (z.B. Linie → Kreisbogen) haben verschiedene Branch-IDs. Die Voronoi-Trennfläche zwischen ihnen existiert im SDF-Raum, und der Octree würde dort fälschlicherweise eine Kante suchen. Da die Normalen an dieser Stelle identisch sind (n\_a \= n\_b), wird das Kreuzprodukt zum Nullvektor → Singularität.

**Lösung:** G1-stetige Segmente werden zu einer **Piecewise Curve** unter einer gemeinsamen Branch-ID verschmolzen. Die Verwerfung verschwindet, der Octree sieht einen durchgehenden Branch und generiert korrekterweise keine Kante.

### Offset-Routing (Shell-Operation)

Offsets werden **an die Leaf Nodes durchgereicht**, nicht am Wurzelknoten subtrahiert:

```
Richtig:   max(Ebene_A - d, Ebene_B - d)     →  Kanten bleiben scharf
Falsch:    max(Ebene_A, Ebene_B) - d          →  Kanten runden sich ab
```

Die unendlichen Ebenen verschieben sich jeweils um d entlang ihrer Normalen. Das `max` schneidet sie danach wieder scharfkantig zusammen. Die Verwerfung wandert mit, bleibt aber eine perfekte 0-Radius-Kante.

**Einschränkung bei nicht-planaren Leaves:** Leaf-Level-Offset funktioniert sauber für Ebenen, Zylindermäntel und Sphären (überall konstanter Gradientenbetrag). Bei Gyroids oder TPMS ist `f(p) - d` äquivalent zum Root-Level-Offset — die Unterscheidung ist irrelevant, da die Fläche selbst keine Kanten hat. Wenn ein Gyroid mit Begrenzungsebenen kombiniert ist (`max(Gyroid, Planes)`), erhalten nur die **Begrenzungs-Leaves** den Offset, nicht der Gyroid selbst — es sei denn, der Nutzer will explizit die Gyroid-Isofläche verschieben.

**Shell** (Hohlkörper mit Wandstärke t):

```
f_shell = max(Außenhaut(+d_out an Leaves), -Innenhaut(-d_in an Leaves))
```

### Domain Warps (Twist, Bend, Taper)

Domain Warps verformen den Raum vor der SDF-Auswertung:

```
f_twisted(p) = f_original(warp(p))
```

**Kanten-Monopol bleibt erhalten:** Warps erzeugen keine neuen `min`/`max`-Operationen. Bestehende Branch-Kollisionen werden mitverformt, aber nicht zerstört. Der Octree findet die verformten Kanten über dieselben Branch-ID-Wechsel.

**Gradienten bleiben exakt:** Die Kettenregel der Dualen Zahlen (`num-dual`) multipliziert die Jacobi-Matrix des Warps automatisch mit dem Gradienten des Originals. Der resultierende Gradient ∇f(p) steht exakt senkrecht auf der verformten Oberfläche — die Tangentialebenen für Kanten- und Vertex-Berechnung stimmen perfekt.

**Metrische Verzerrung:** Der Warp zerstört die euklidische Metrik des Distanzfeldes. Ein Wert von f(p) \= 5.0 bedeutet nicht mehr, dass die Oberfläche 5mm entfernt ist. Der Gradientenbetrag |∇f(p)| ist nicht mehr 1. Die Projektionsformel in Phase 2 schätzt die Entfernung dann falsch ein — deshalb ist Phase 2 als iterativer Solver ausgelegt (Newton-Raphson-Schleife), der bei gewarpten Feldern in 2–4 Iterationen konvergiert.

***

## B-Rep-Extraktion: Der Algorithmus

Ein **Körper** ist eine durch Boolesche Operationen verbundene Menge von Leaf Nodes. Es kann mehrere Körper in der Szene geben. Jeder Körper wird in einem eigenen Octree unabhängig von den anderen verarbeitet — das ermöglicht Parallelisierung (Multithreading) bei Baugruppen.

### Phase 1: Räumliche Isolierung (Octree)

Der Raum wird systematisch abgetastet, um Oberflächen und Verwerfungen zu lokalisieren.

1. Eine Bounding Box (AABB) des Körpers wird als Octree iterativ unterteilt
2. An den 8 Eckpunkten jeder Zelle wird das SDF mit Dualen Zahlen ausgewertet: `eval(p) → (f, ∇f, Branch_ID)`
3. Jede Zelle wird klassifiziert:

| Typ          | Bedingung                                              | Bedeutung                           |                 |                                                                             |
| ------------ | ------------------------------------------------------ | ----------------------------------- | --------------- | --------------------------------------------------------------------------- |
| **Leer**     | Vorzeichen alle gleich, Branch-IDs alle gleich, UND \` | f\_min                              | > diag(Zelle)\` | Garantiert kein Feature → verwerfen                                         |
| **Fläche**   | Vorzeichen wechselt, Branch-IDs alle gleich            | Glatte, stetige Oberfläche          |                 |                                                                             |
| **Kante**    | Branch-IDs unterschiedlich (Vorzeichen beliebig)       | Verwerfung durchschneidet die Zelle |                 |                                                                             |
| **Unsicher** | Vorzeichen und Branch-IDs alle gleich, ABER \`         | f\_min                              | ≤ diag(Zelle)\` | Oberfläche *könnte* durchlaufen, ohne Ecken zu berühren → weiter verfeinern |

Die **Lipschitz-Bedingung** in der vierten Zeile ist der Schutz gegen das Nyquist-Problem: Für echte SDFs (|∇f| ≤ 1) kann die Funktion innerhalb einer Zelle höchstens um die Zelldiagonale variieren. Ist der kleinste Absolutwert an den Ecken kleiner als die Diagonale, kann ein Nulldurchgang (\= Oberfläche) im Zellinneren existieren, obwohl kein Vorzeichenwechsel an den Ecken sichtbar ist. Typisches Szenario: dünne Rippen, schmale Nuten, nahe parallele Flächen.

1. Zellen werden iterativ verfeinert bis zu einer definierten **Auflösungsgrenze** (z.B. 0.1 mm). Features unterhalb dieser Schwelle sind explizit außerhalb der Systemgarantie. Für praktische Fertigungsverfahren (3D-Druck: \~0.2 mm Schichthöhe, CNC: \~0.01 mm Werkzeugtoleranz) ist eine Grenze von 0.1 mm oder feiner vollkommen ausreichend.

### Phase 2: Iterative Oberflächen-Projektion

In identifizierten Kanten-Zellen wird die Rasterung verlassen und in exakte Vektormathematik gewechselt.

1. Zwei Abtastpunkte (p\_A, p\_B) werden genommen, die zu den beiden kollidierenden Branches gehören
2. Jeder Punkt wird iterativ per Gradient auf seine zugehörige Null-Oberfläche projiziert:

```
repeat:
    p ← p - f(p)/|∇f(p)|² · ∇f(p)
until |f(p)| < ε
```

Die Division durch |∇f(p)|² ist die **Taubin-Normalisierung** — sie korrigiert die Distanz bei nicht-euklidischen Feldern (Gyroids, Domain Warps) "on the fly".

**Konvergenz:** Bei echten euklidischen Distanzfeldern (|∇f| \= 1) ist ein einziger Schritt exakt. Bei gewarpten oder nicht-normierten Feldern (Twist, Bend, Gyroid) konvergiert die Schleife dank der exakten Gradienten aus den Dualen Zahlen typischerweise in 2–4 Iterationen auf Maschinengenauigkeit. Abbruchkriterium: |f(p)| \< ε (z.B. 10⁻⁸).

1. Der normalisierte Gradient am projizierten Punkt definiert die exakte Tangentialebene.

### Phase 3: Kanten- und Vertex-Extraktion

**Kanten** (Kollision von 2 Branches):

1. Die Tangentialebenen von Branch A (Normale n\_a) und Branch B (Normale n\_b) werden geschnitten
2. Kantenrichtung (Tangente): `t = n_a × n_b`
3. Der Zell-Mittelpunkt wird senkrecht auf die Schnittgerade projiziert → exakter Kantenpunkt p\_edge

**Vertices** (Kollision von ≥3 Branches):

1. Tangentialebenen für alle beteiligten Branches aufspannen
2. Das resultierende lineare Gleichungssystem (3×3 oder überbestimmt via SVD) lösen
3. Der singuläre Schnittpunkt ist der exakte Eckpunkt in 3D

**Phantom-Filter:**

Nicht jede Branch-ID-Kollision ist eine echte Kante. Der projizierte Kantenpunkt p\_edge wird ein letztes Mal in den AST gegeben:

* |f(p\_edge)| \< ε → **echte Oberflächenkante** → behalten
* f(p\_edge) ≪ 0 → interne Medial Axis (Skelett) → **verwerfen**
* f(p\_edge) ≫ 0 → externer Voronoi-Graben → **verwerfen**

### Phase 3.5: Topologie-Rekonstruktion (Edge Walker)

Phase 3 produziert pro Octree-Zelle einen Kantenpunkt mit Richtung. Bevor Splines gefittet werden können, müssen diese Punkte zu **geordneten Kantenketten** zusammengesetzt werden.

**Kanten-Signatur:** Jede Kante zwischen zwei Halbräumen hat eine eindeutige Signatur: das Paar `(Branch_A, Branch_B)` (kanonisch sortiert).

**Der Algorithmus:**

1. **Vertex-Identifikation:** Finde alle Octree-Zellen mit ≥3 kollidierenden Branch-IDs. Berechne dort die exakten Vertices (Phase 3). Diese Vertices sind die Start- und Endknoten des Kantengraphen.
2. **Edge Tracing (Der Walk):**
   * Starte an einem Vertex. Dieser Vertex verbindet mehrere Kanten mit verschiedenen Signaturen.
   * Wähle eine Signatur, z.B. `(Branch_A, Branch_B)`.
   * Suche in den 26 direkten Nachbarzellen des Octrees nach der Zelle, die exakt diese beiden IDs als Kollision meldet.
   * Gehe in diese Zelle, speichere den dort berechneten Kantenpunkt und seine Tangente in der geordneten Liste.
   * Von dieser Zelle aus suche wieder in deren Nachbarschaft nach der nächsten Zelle mit derselben Signatur `(Branch_A, Branch_B)`. Die Zelle, aus der man gerade kommt, wird ignoriert.
   * Wiederhole, bis eine Zelle mit ≥3 Branches erreicht wird. Das ist der End-Vertex.
3. **Geschlossene Kanten:** Wenn der Walk zum Start-Vertex zurückkehrt (z.B. ein kreisförmiger Schnitt), entsteht eine geschlossene Kantenkurve ohne Vertices.

**Robustheit:** Der Walk funktioniert zuverlässig, solange die Octree-Auflösung fein genug ist, dass sich keine Kanten mit derselben Signatur in derselben Zelle kreuzen — was bei stetigen Basisflächen mathematisch kaum möglich ist.

### Phase 4: B-Rep Weaving

**Edges:**

Die geordneten Kantenketten aus Phase 3.5 werden als Stützstellen für **kubische Hermite-Splines** genutzt. Da an jedem Punkt Position und exakte Tangente t bekannt sind, interpoliert der Spline fehlerfrei auf der wahren analytischen Kante. Diese Hermite-Splines lassen sich trivial in NURBS-Kurven konvertieren.

**Wireframe:**

Die Enden der Splines werden an den berechneten Vertices zusammengeführt. Das Ergebnis ist ein geschlossenes Kantennetzwerk.

**Faces (Trimmed Surfaces):**

1. Die originalen analytischen Parametrisierungen der NURBS-fähigen Basisflächen (Ebenen, Zylinder, Sphären, Kegel, Tori) werden aus Phase 0 geladen
2. Das geschlossene Kantennetzwerk wird auf diese Flächen projiziert und definiert die **Trim Boundaries**
3. Alles außerhalb der Boundaries wird verworfen
4. **Gyroids und TPMS** haben keine NURBS-Parametrisierung. Ihre Faces werden als tessellierte Approximation (aus dem Mesh-Pfad) ins B-Rep eingehängt. Für STEP-Export werden sie als Triangulated Surfaces geschrieben.

Das Resultat ist ein topologisch **wasserdichtes B-Rep-Volumenmodell**. NURBS-fähige Flächen werden exakt in STEP/IGES exportiert; Gyroid-Flächen als Tessellation.

***

## Dirty-Tracking und B-Rep-Cache

Die B-Rep-Extraktion (Phasen 1–4) ist rechenintensiv. Sie darf nur für Körper ausgeführt werden, die sich tatsächlich geändert haben.

### Invalidierungseinheit

Die Invalidierungseinheit ist der **Körper** (Body). Eine Änderung ist in der Regel ein Feature im Feature-Tree (Hinzufügen, Entfernen, Parameter-Änderung). Da jeder Körper aus einer definierten Teilmenge des Feature-Spines besteht, lässt sich präzise bestimmen, welche Körper von einer Feature-Änderung betroffen sind.

### Mechanismus

Jeder Body speichert einen **Payload-Hash** über seine Feature-Parameter und CSG-Struktur. Bei einer Mutation wird der Hash neu berechnet:

* **Hash unverändert** → B-Rep aus Cache verwenden, keine Neuberechnung
* **Hash geändert** → Body als dirty markieren, B-Rep-Extraktion anstoßen

Reine Transformations-Änderungen (Translation, Rotation eines ganzen Körpers) erfordern keine B-Rep-Neuberechnung — das B-Rep wird transformiert, nicht neu erzeugt.

### Analogie zum bestehenden Mesh-Cache

Das bestehende System nutzt bereits dieses Muster für die Mesh-Pipeline: `Body` in `body.rs` speichert `payload_hash` und `geometry_hash`. Mesh wird nur bei Hash-Änderung neu berechnet, reine Transforms werden als `transform_delta` an die GPU weitergereicht. Der B-Rep-Cache arbeitet nach identischem Prinzip — Mesh und B-Rep können denselben Dirty-Trigger teilen, werden aber unabhängig voneinander berechnet.

***

## Topologische Stabilität (Persistent Naming)

### Das Problem

Parametrisches CAD erfordert **stabile Referenzierbarkeit**: Ein Fillet, das auf "diese Kante" gelegt wurde, muss nach 200 Feature-Änderungen noch dieselbe Kante referenzieren. In klassischen B-Rep-Kerneln (Parasolid, OpenCASCADE) erhalten Kanten und Flächen arbiträre IDs (`Edge_42`), die bei jedem Rebuild neu vergeben werden. Das Wiederfinden der "richtigen" Kante nach einer Parameteränderung — das **Topological Naming Problem (TNP)** — ist das schwerste ungelöste Problem der CAD-Branche.

### Natürlicher Vorteil der Halbraum-Architektur

FieldDrafts Branch-Ancestry-Ansatz hat einen **strukturellen Vorteil** gegenüber klassischen Kerneln: Eine Kante ist keine willkürliche Entität, sondern entsteht deterministisch als Kollision zweier Halbräume. Ihre Identität lässt sich direkt aus ihrer Entstehungsgeschichte ableiten — nicht aus rekonstruierter Topologie.

Die Identitätskette:

```
Feature (UUID) → Branch (Halbraum-Leaf) → Edge (Branch-Kollision)
```

Bei rein parametrischen Änderungen (Radius 10 → 10.001) ändern sich weder Features noch Branches. Die Kanten-Identität ist **automatisch stabil**, ohne jede Heuristik. Das deckt den Großteil der Alltagsfälle ab.

### Stufe 1: Semantische Identität für Faces, Edges und Vertices

Jede topologische Entität erhält einen deterministischen Namen aus ihren Eltern-Branches.

**Ebene 1 — Feature-ID:** Jedes Feature im Feature-Tree bekommt bei Erstellung eine unveränderliche UUID.

**Ebene 2 — Branch-ID:** Ein Feature expandiert intern zu Halbräumen. Die Branch-ID kombiniert Feature-UUID mit der lokalen Halbraum-Rolle:

```
Branch_ID = Hash(Feature_UUID, "Plane_+Z")
```

**Ebene 3 — Topologische Signaturen:**

| Entität    | Entstehung                                    | Signatur                                                  |
| ---------- | --------------------------------------------- | --------------------------------------------------------- |
| **Face**   | Eine Halbraum-Fläche, getrimmt durch Nachbarn | `Face_ID = Branch_ID`                                     |
| **Edge**   | Kollision zweier Halbräume                    | `Edge_ID = Hash(sort(Branch_A, Branch_B))`                |
| **Vertex** | Kollision von ≥3 Halbräumen                   | `Vertex_ID = Hash(sort(Branch_A, Branch_B, Branch_C, …))` |

Face-Naming ist das einfachste: Jede Fläche im B-Rep gehört zu genau einem Branch (der unendlichen Basisfläche, getrimmt durch die Kantenschleifen). `Face_ID = Branch_ID` ist automatisch stabil. Wenn ein Branch durch eine CSG-Operation in mehrere disjunkte Flächen zerfällt (z.B. ein Loch teilt eine Ebene), wird Spatial Disambiguation analog zu Edges angewendet.

**Spatial Disambiguation (bei Multiplizität):** Wenn mehrere Entitäten dieselbe Signatur besitzen (z.B. eine Ebene schneidet beide Schenkel eines U-Profils → zwei Edges mit identischem `Hash(Branch_A, Branch_B)`), wird der 3D-Mittelpunkt (Centroid) berechnet und entlang einer stabilen Achse (X → Y → Z) sortiert. Jede Entität erhält einen Index:

```
Edge_ID = Hash(Branch_A, Branch_B, SpatialIndex)
```

**Referenzierung durch Features:** Ein Fillet-Feature speichert die `Edge_ID` seiner Zielkante, ein Sketch-Placement die `Face_ID` seiner Trägerfläche. Beim Rebuild sucht der Kernel im neuen B-Rep nach Entitäten mit demselben Hash. Da Branches aus Feature-UUIDs abgeleitet werden, bleibt der Hash über Parameteränderungen hinweg stabil — solange die beteiligten Features nicht gelöscht werden.

### Stufe 2: Geometrischer Fingerabdruck und Zustandsmaschine

Für die Fälle, in denen der reine Hash nicht ausreicht (Topology Split, Merge, Spatial Reordering), wird die Identifizierung um einen geometrischen Fingerabdruck erweitert.

**Bei Referenzierung** (Nutzer klickt eine Kante an) speichert das System zusätzlich zum Hash:

* Approximativer Centroid (3D-Schwerpunkt der Kante)
* Approximative Kantenlänge
* Haupttangente am Mittelpunkt

**Beim Rebuild** vergleicht der Kernel die gespeicherten Fingerprints mit den neu generierten Kanten gleicher Signatur. Daraus ergibt sich ein **Confidence Score** (0.0–1.0), der die Zustandsmaschine steuert:

| Zustand       | Score    | Verhalten                                                                                                                   |
| ------------- | -------- | --------------------------------------------------------------------------------------------------------------------------- |
| **Resolved**  | > 0.95   | Automatisches Mapping. Feature baut sich normal auf.                                                                        |
| **Healed**    | 0.7–0.95 | Feature baut sich auf, wird in der UI gelb markiert ("Referenz automatisch angepasst").                                     |
| **Ambiguous** | 0.4–0.7  | Feature stoppt. UI hebt Kandidaten-Kanten farblich hervor, Nutzer bestätigt per Klick.                                      |
| **Missing**   | \< 0.4   | Referenz verloren. Feature schlägt fehl. Ghost-Geometrie aus dem B-Rep-Cache wird rot/transparent im Viewport eingeblendet. |

**Graceful Degradation:** Features, die auf mehrere Kanten referenzieren (z.B. Fillet auf 4 Kanten), arbeiten partiell weiter, wenn nur einzelne Referenzen fehlen — statt als Ganzes zu scheitern.

**Semantische Diagnostik:** Da Kanten-IDs an den AST gebunden sind, kann der Kernel bei fehlenden Referenzen präzise Ursachen benennen ("Zylinder und Ebene schneiden sich nicht mehr" statt generischem "Reference Error").

### Zukünftige Erweiterungen (nicht im aktuellen Scope)

* **Topological Neighborhood Matching:** Nachbar-Vertices, angrenzende Faces und Valence als zusätzliche Matching-Dimension
* **Temporal Continuity:** Bipartite Graph Matching zwischen alter und neuer Topologie über mehrere Rebuilds hinweg (Identity Memory Layer)
* **Semantic Decay:** Halbwertszeit-basierte Garbage Collection für historische Topologie-Records statt hartem Löschen
* **Constraint-basierte Identität:** Semantische Beschreibung ("die obere horizontale Kante des Features X") statt rein geometrischer Hashes

***

## Bekannte Einschränkungen

1. **Auflösungsgrenze (Thin Features):** Die B-Rep-Extraktion garantiert nur Features oberhalb der definierten Octree-Auflösungsgrenze (z.B. 0.1 mm). Die Lipschitz-basierte Verfeinerung (Phase 1) stellt sicher, dass der Octree in der Nähe von Oberflächen automatisch verfeinert — auch bei dünnen Rippen ohne Vorzeichenwechsel an den Zellecken. Features unterhalb der Auflösungsgrenze können aber prinzipbedingt übersehen werden. Für praktische Fertigungsverfahren (3D-Druck, CNC) ist dies kein Problem.
2. **AST-Tiefe und Performance:** Die Halbraum-Expansion erhöht die Opcode-Anzahl pro Primitiv (Würfel: 1 Opcode → \~11). Duale Zahlen verursachen ca. 3–4× Overhead gegenüber reinem f64 pro Auswertung. Bei Modellen mit hunderten Features summiert sich beides. Existierende Mitigationen: **(a)** Der `sdf_compiler` kompiliert den AST bereits in einen flachen Bytecode-Instruktionspuffer (Stack-Machine) — keine Pointer-Verfolgung oder Baumtraversierung zur Laufzeit, gute Cache-Lokalität. **(b)** Der BVH (`BoundedCsg`) umhüllt CSG-Teilbäume mit AABBs und überspringt irrelevante Branches — effektive Kosten pro Punkt: O(log N + k) statt O(N). **(c)** Per-Body-Isolation: Assembly-Größe beeinflusst die Per-Point-Kosten nicht, da jeder Körper seinen eigenen Evaluierungspfad hat. **(d)** GPU-seitig können Composite-Opcodes (z.B. `OP_PRIM_BOX` als Optimierung für die max-of-6-planes-Pattern) beibehalten werden, auch wenn der CPU-Kernel reine Halbräume verwendet.
3. **Fillets/Chamfers:** Analytisch perfekte Abrundungen erfordern explizite Einfügung neuer Halbräume (Zylinder für Kanten, Kugeln für Ecken) in den AST — das ist nicht trivial und bläht den Baum auf.
4. **Fraktale und B-Rep:** Fraktale haben unendliche Topologie (unendlich viele Kanten, unendliche Oberfläche). Sie können gerendert und per Mesh exportiert, aber nicht als B-Rep extrahiert werden.
5. **Gyroids/TPMS und STEP:** Gyroids haben keine NURBS-Parametrisierung. Im B-Rep werden ihre Faces als tessellierte Approximation dargestellt. STEP-Export schreibt sie als Triangulated Surfaces — nicht als analytische Flächen.
6. **Domain Warps und Distanzmetrik:** Twist, Bend und Taper zerstören die euklidische Metrik des SDF. Die B-Rep-Projektion (Phase 2) muss iterativ arbeiten (2–4 Schritte statt One-Shot). Die Korrektheit der Gradienten ist durch Duale Zahlen garantiert.

***

## Analyse: Erforderliche Änderungen an der aktuellen Codebasis

Die folgende Analyse vergleicht die bestehende FieldDraft-Architektur mit dem neuen Kernel-Konzept und identifiziert die notwendigen Umbauten auf High-Level-Ebene.

### 1. Primitive → Halbraum-Dekomposition

**Aktuell:** 13 geschlossene Primitiven (`Primitive` enum in `crates/protocol/src/lib.rs`): Box, Sphere, Cylinder, Torus, Cone, Capsule, HexPrism, Ellipsoid, Gyroid, Thread, ImportedMesh, plus Extrude/Revolve als 2D→3D-Lifting. Jedes Primitiv ist ein fertiger Körper mit intrinsischen Kanten.

**Neu:** Keine geschlossenen Primitiven. Leaf Nodes sind ausschließlich unendliche C∞-Flächen (Ebene, Zylindermantel, Sphäre, Kegel, Torus-Ring) oder atomare 2D-Skizzen. Ein Würfel entsteht aus 6 Ebenen-Halbräumen via `max`.

**Änderungen:**

* `Primitive` enum ersetzen durch `HalfSpace` enum (Plane, InfiniteCylinder, Sphere, Cone, Torus, Gyroid, AtomicSketch, …)
* Feature-zu-Halbraum-Expansion: Nutzer-Primitive ("Würfel hinzufügen") werden intern in den entsprechenden Halbraum-Baum expandiert (6 Ebenen + 5× `max`)
* `CsgNode` in `crates/kernel/src/csg.rs` anpassen: Leaf-Knoten referenzieren Halbräume statt geschlossene Primitive
* `BoundedCsg` (BVH in `crates/kernel/src/bvh.rs`) muss mit den expandierten Bäumen umgehen

### 2. Sketch-System → Analytische Kurven-SDFs

**Aktuell:** `Sketch2dPrimitive` kennt nur Circle, Rectangle, Polygon, Sector (`crates/protocol/src/sketch2d.rs`). Kurven (Splines, Bezier, Ellipsen) werden im Frontend als Polygon-Vertices approximiert und als `Polygon`-Primitiv an den Kernel geschickt. Das Frontend (`web/src/SketchCanvas2d.tsx`, `sketchSession.ts`) bietet nur sketch\_line, sketch\_point, sketch\_rectangle, sketch\_circle, sketch\_polygon, sketch\_arc als Tools.

**Neu:** Analytische SDF-Auswertung für Line, Ellipse, Bezier, Spline — mit exakter Distanz und exaktem Gradient via Duale Zahlen. Keine Linien-Approximation. Jedes Segment liefert eine eigene Branch-ID.

**Änderungen:**

* Neue 2D-SDF-Primitiven: `Line`, `EllipticArc`, `QuadraticBezier`, `CubicBezier`, `BSpline` mit analytischer Distanzberechnung
* Segment-basierte Branch-IDs statt Sketch-ID: Jedes Segment im Sketch meldet seine eigene ID
* G1-Preprocessing als neuer Vorverarbeitungsschritt (`sketch2d.rs`): Tangentenstetigkeit an Segmentübergängen prüfen, G1-stetige Segmente unter gemeinsamer Branch-ID verschmelzen
* Frontend-Sketch-Tools erweitern um Spline- und Bezier-Werkzeuge
* GPU-Opcodes: Neue `OP_PRIM2D_*` für analytische Kurven (Bezier, Spline) im Shader

### 3. Gradient-Berechnung → Duale Zahlen

**Aktuell:** Keine automatische Differentiation. Gradienten werden ausschließlich per **Finite Differences** berechnet (6 zusätzliche SDF-Auswertungen pro Normal, `crates/kernel/src/dc.rs` Funktion `sdf_normal`, ε \= max(cell\_size × 0.25, 1e-4)). Auf der GPU ebenfalls Finite Differences (`NORMAL_EPS = 0.01`).

**Neu:** Duale Zahlen 1. Ordnung (`num-dual` Crate) für exakte analytische Gradienten auf CPU-Seite. Jede SDF-Auswertung liefert simultan `(f, ∇f)` ohne zusätzliche Samples.

**Änderungen:**

* `num-dual` als Dependency zum Kernel-Crate hinzufügen
* Alle SDF-Evaluierungsfunktionen auf generischen Typ `T: DualNum<f64>` umstellen (Wert + 3 Dual-Komponenten für ∂/∂x, ∂/∂y, ∂/∂z)
* `sdf_normal()` in `dc.rs` ersetzen durch Extraktion des Dualteils
* GPU-Shader: Bleiben bei Finite Differences (Duale Zahlen sind eine CPU-Technik). Keine Änderung an `sdfVmInterpreter.ts` oder `sdfRenderer.ts` nötig
* Performance-Implikation: 1 SDF-Auswertung mit Dual statt 7 Auswertungen mit FD (1 Wert + 6 Offsets). Netto schneller trotz \~3-4× Overhead pro Auswertung

### 4. Branch-Tracking → Halbraum-Branch-IDs

**Aktuell:** `Contributors`-System existiert bereits (`crates/kernel/src/csg.rs`, Struct mit 4 Slots `[NodeId; 4]`). Trackt welche **Feature-IDs** (geschlossene Primitive) an einem Punkt dominieren. Klassifizierung: 1 Contributor \= Face, 2 \= Edge, ≥3 \= Vertex. Wird für Mesh-Vertex-Classification und Picking genutzt.

**Neu:** Branch-IDs tracken nicht Feature-IDs, sondern **Halbraum-Leaf-IDs** — also die individuellen unendlichen Flächen. Ein Würfel hat 6 Branch-IDs statt einer Feature-ID. Zusätzlich: Bei atomaren 2D-Skizzen meldet jedes Segment seine eigene Sub-ID.

**Änderungen:**

* `Contributors` refactorn: Statt `FeatureId` werden `BranchId` (\= Halbraum-Leaf-ID) gespeichert
* Kapazität erweitern: 4 Slots könnten bei Vertex-Erkennung (≥3 Branches) knapp werden, ggf. auf 6-8 erweitern
* `evaluate()` in `csg.rs` anpassen: Bei `min`/`max` nicht die Feature-ID, sondern die Branch-ID des dominierenden Halbraums weitergeben
* Sketch-Nodes: Branch-ID-Propagation auf Segment-Ebene statt Sketch-Ebene
* Picking (`raycast.rs`) und Mesh-Classification (`dc.rs`, `mdc.rs`) auf neue Branch-ID-Semantik umstellen

### 5. B-Rep-Extraktion → Analytische Octree-Pipeline (NEU)

**Aktuell:** Keine echte B-Rep-Datenstruktur. Topologie wird **aus dem Mesh abgeleitet** (post-hoc): Planar-Face-Simplification, Edge/Vertex-Detection via Contributors auf DC-Mesh-Vertices (`dc.rs` Zeilen 235-323). Das ist ein Proxy, kein analytisches B-Rep. Die API (`bodyFaceCount`, `bodyEdgeCount`, `bodyTopologyJson`) liefert Mesh-basierte Approximationen.

**Neu:** Eigenständige B-Rep-Extraktion via Octree + analytische Projektion + Hermite-Spline-Fitting (Phasen 1-4). Produziert echte NURBS-Trimmed-Surfaces, unabhängig vom Mesh.

**Änderungen:**

* **Neues Subsystem** (`brep.rs` oder `crates/brep/`): Octree-Builder, analytische Projektion (Taubin-Normalisierung), Kanten-/Vertex-Extraktion, Phantom-Filter, Hermite-Spline-Fitting
* B-Rep-Datenstrukturen: `Vertex`, `Edge` (NURBS-Kurve), `Face` (Trimmed Surface), `Shell`, `Solid` — analog zu OpenCASCADE-Topologie
* Octree-Zell-Klassifizierung: Nutzt das neue Branch-ID-System aus Punkt 4
* Entkopplung von Mesh: B-Rep-Extraktion läuft parallel zum Meshing, nicht daraus abgeleitet
* Bestehende Topology-API (`bodyFaceCount`, `bodyEdgeCount` etc.) umstellen von Mesh-Proxy auf echtes B-Rep

### 6. STEP-Export → Aus B-Rep statt aus Features

**Aktuell:** `step_commands.rs` mappt Features direkt auf OCCT-Kommandos (box, sphere, cylinder, union, difference, …). Der OCCT-Worker (`web/src/occt.worker.ts`) baut daraus B-Rep-Geometrie via OpenCASCADE. Das funktioniert nur für die bekannten Primitiv-Typen.

**Neu:** STEP-Export aus dem analytischen B-Rep (Trimmed NURBS Surfaces). OCCT-Mapping über Feature-Spine wird obsolet.

**Änderungen:**

* B-Rep-Output direkt als STEP serialisieren (eigener Writer oder via OCCT als Trimmed-Surface-Input)
* `step_commands.rs` entfällt langfristig oder wird zum Fallback für einfache Fälle
* STEP-Import: Kann bestehen bleiben (Tessellation → Render-Only oder MeshSDF)

### 7. GPU-Opcode-System

**Aktuell:** 39 Opcodes für geschlossene Primitive (`OP_PRIM_BOX`, `OP_PRIM_SPHERE`, …), Booleans, Warps. Stack-Machine-VM in `sdf_compiler.rs` (Rust) und `sdfVmInterpreter.ts` (GPU).

**Neu:** Opcodes für Halbräume statt geschlossene Primitive. Neue 2D-Kurven-Opcodes. Die VM-Architektur (Stack-Machine) bleibt, der Befehlssatz ändert sich.

**Änderungen:**

* Neue Opcodes: `OP_HALFSPACE_PLANE`, `OP_HALFSPACE_INF_CYLINDER`, `OP_HALFSPACE_SPHERE`, `OP_HALFSPACE_CONE`, `OP_HALFSPACE_TORUS`
* Neue 2D-Opcodes: `OP_PRIM2D_LINE`, `OP_PRIM2D_ELLIPTIC_ARC`, `OP_PRIM2D_QUAD_BEZIER`, `OP_PRIM2D_CUBIC_BEZIER`
* Alte geschlossene Primitiv-Opcodes entfallen langfristig (Box, Cylinder etc. werden zu Halbraum-Kompositionen kompiliert)
* `sdf_compiler.rs`: `emit_node()` expandiert Feature-Primitive in Halbraum-Opcode-Sequenzen
* `sdfVmInterpreter.ts`: Neue Opcode-Handler im GPU-VM
* `sdfRenderer.ts`: Neue WGSL-Hilfsfunktionen für Halbraum-SDFs und analytische 2D-Kurven
* Performance-Überlegung: Ein Würfel wird von 1 Opcode (OP\_PRIM\_BOX) zu \~11 Opcodes (6 Planes + 5 max). AST-Tiefe und Opcode-Budgets prüfen

### 8. Shell/Offset-Operation

**Umgesetzt (D.2, siehe DL-36).** Der alte globale Shell `abs(sdf) - thickness` auf dem Wurzelknoten ist entfernt. Shell wird jetzt als `Difference(inner, apply_offset(inner, -t))` beim Bau gesenkt: Jede Leaf-Primitive wird individuell um `d` versetzt (negatives `d` erodiert nach innen), dann erst boolesch verknüpft → scharfe Kanten bleiben erhalten.

**Änderungen (erledigt):**

* `apply_offset(node, d)` AST-Pass in `csg.rs` (klont den Baum, versetzt jede Leaf-Primitive-Dimension um `d`, Clamp auf `OFFSET_FLOOR`)
* Shell-Senkung in `CsgNode::from_features`; `CsgNode::Shell`, `BoundedCsg::Shell` und `OP_SHELL` (Rust-Emitter + TS-Interpreter) vollständig gelöscht
* GPU: kein Shell-Opcode mehr — die bestehende `OP_DIFF`-Maschinerie trägt den Shell. Offset-Maße fließen durch die vorhandenen Primitive-Opcodes

### 9. Meshing-Pipeline

**Aktuell:** Manifold Dual Contouring mit QEF (`dc.rs`, `mdc.rs`). Finite-Difference-Normalen. Per-Vertex Contributors.

**Neu:** Meshing bleibt als unabhängiger Pfad erhalten (3D-Druck-Export). Die Umstellung auf Duale Zahlen verbessert die Normal-Berechnung. Branch-IDs ersetzen Feature-Contributors.

**Änderungen:**

* `sdf_normal()` durch Dual-Number-Gradient ersetzen → exaktere Normalen, weniger SDF-Evaluierungen
* QEF-Solver profitiert von exakten Gradienten (bessere Feature-Preservation)
* Contributor-Tracking auf Branch-IDs umstellen (Punkt 4)
* Mesh-Pipeline selbst (MC-Lookup, Cycle-Detection, QEF) bleibt strukturell erhalten

### 10. Domain Warps in der B-Rep-Pipeline

**Aktuell:** Twist und Bend als Domain Warps implementiert (`CsgNode::Twist`, `CsgNode::Bend`). Verformen den Punkt vor der SDF-Auswertung. GPU-seitig: `OP_WARP_TWIST`, `OP_WARP_BEND`.

**Neu:** Domain Warps bleiben funktional identisch. Die Umstellung auf Duale Zahlen liefert automatisch korrekte Gradienten (Kettenregel über die Jacobi-Matrix des Warps). Die iterative Projektion in Phase 2 kompensiert die metrische Verzerrung.

**Änderungen:**

* SDF-Evaluierung mit Dualen Zahlen propagiert Warps automatisch korrekt (keine manuelle Jacobi-Berechnung)
* Phase 2 der B-Rep-Extraktion: Newton-Raphson-Schleife statt One-Shot bei gewarpten Körpern (2–4 Iterationen)
* Kein Umbau der Warp-Architektur selbst nötig — nur die Konsumenten (Projektion, Meshing) müssen mit nicht-euklidischen Feldern umgehen

### 11. Persistent Naming

**Aktuell:** Keine persistente Topologie-Identifizierung. Features referenzieren Topologie implizit über Mesh-basierte Indizes, die bei jedem Rebuild instabil sind.

**Neu:** Deterministische Benennung aller topologischen Entitäten (Faces, Edges, Vertices) durch Branch-Ancestry-Hashes. Geometrische Fingerprints und Confidence-basierte Zustandsmaschine für Robustheit bei topologischen Änderungen.

**Änderungen:**

* Branch-IDs aus Punkt 4 bilden die Basis: `Face_ID = Branch_ID`, `Edge_ID = Hash(Branch_A, Branch_B)`, `Vertex_ID = Hash(Branch_A, Branch_B, Branch_C, …)`
* Spatial Disambiguation: Centroid-basierte Sortierung bei Multiplizität (mehrere Entitäten mit identischer Signatur)
* Feature-Referenzen (Fillet → Edge, Sketch-Placement → Face) speichern Hash + geometrischen Fingerabdruck (Centroid, Länge, Tangente)
* Confidence-basierte Zustandsmaschine (Resolved/Healed/Ambiguous/Missing) im Rebuild-Pfad
* Ghost-Geometrie aus B-Rep-Cache für fehlgeschlagene Referenzen im Viewport
* Semantische Diagnostik: Ursachen für fehlende Referenzen aus dem AST ableiten

### Zusammenfassung: Prioritäten und Abhängigkeiten

```
              ┌──────────────────────────────────────┐
              │ 1. Halbraum-Primitive + 7. GPU-Opcodes│  Grundlage
              └──────────────────┬───────────────────┘
                                 │
                ┌────────────────┼────────────────┐
                ▼                ▼                ▼
     ┌──────────────────┐ ┌───────────┐ ┌─────────────────┐
     │ 2. Analytische   │ │ 3. Duale  │ │ 4. Branch-IDs   │
     │    Sketch-Kurven │ │    Zahlen │ │    (Halbräume)  │
     └────────┬─────────┘ └─────┬─────┘ └────────┬────────┘
              │                 │                 │
              └─────────────────┼─────────────────┘
                                ▼
                      ┌──────────────────────┐
                      │ 5. B-Rep-Extraktion  │  Neues Subsystem
                      │    (Octree-Pipeline) │
                      └──────────┬───────────┘
                                 │
                    ┌────────────┼─────────────────────┐
                    ▼            ▼            ▼        ▼
         ┌────────────────┐ ┌────────────┐ ┌──────┐ ┌──────────────┐
         │ 6. STEP-Export │ │ 8. Shell/  │ │ 10.  │ │ 11. Persistent│
         │    aus B-Rep   │ │    Offset  │ │ Warp │ │     Naming    │
         └────────────────┘ └────────────┘ └──────┘ └──────────────┘
```

**Phase A** (Fundament): Punkte 1, 3, 4, 7 — Halbraum-Primitiven, Duale Zahlen, Branch-ID-Refactoring, GPU-Opcodes. Diese vier sind eng gekoppelt: Ohne GPU-Opcodes für Halbräume kann nichts gerendert werden.

**Phase B** (Analytik): Punkt 2 — Analytische Sketch-Kurven. Benötigt Phase A (Duale Zahlen für exakte Gradienten, Branch-IDs für Segment-Tracking).

**Phase C** (B-Rep): Punkt 5 — Die eigentliche B-Rep-Extraktion (Octree, Projektion, Edge Walker, Weaving). Benötigt Phase A+B. Größter Neuentwicklungsaufwand.

**Phase D** (Integration): Punkte 6, 8, 10, 11 — STEP-Export, Shell-Semantik, Domain-Warp-Projektion, Persistent Naming. Persistent Naming benötigt funktionierendes B-Rep (Phase C) und Branch-IDs (Phase A). Die semantische Kanten-/Flächen-Identität (Hash-basiert) ist ein natürliches Nebenprodukt der Branch-ID-Architektur und entsteht mit geringem Mehraufwand. Die Confidence-Zustandsmaschine und Ghost-Geometrie werden implementiert, sobald die B-Rep-Pipeline reale Feature-Referenzen bedienen muss.

### Implementierungsschritte (inkrementell, System bleibt nach jedem Schritt lauffähig)

Phase A ist zu groß für einen einzelnen Arbeitsschritt. Die folgende Tabelle zerlegt die Phasen in inkrementelle Schritte, die jeweils additiv sind — `just test` (Rust + Vitest) muss nach jedem Schritt grün bleiben.

#### Phase A: Fundament

| Schritt | Beschreibung                                                                                                                                                                                                                 | Betroffene Dateien (Kern)                                  | Testkriterium                                                                           |
| ------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------- | --------------------------------------------------------------------------------------- |
| **A.1** | `num-dual` als Dependency hinzufügen. SDF-Evaluierung generisch auf `T: DualNum<f64>` umstellen. Bestehende Primitive (Box, Sphere etc.) sollen mit Dual Numbers funktionieren. `sdf_normal()` durch Dual-Gradient ersetzen. | `Cargo.toml`, `csg.rs`, `dc.rs`, `mdc.rs`                  | `cargo test` grün. Bestehende Meshes identisch.                                         |
| **A.2** | Neues `HalfSpace` enum (Plane, InfiniteCylinder, Sphere, Cone, Torus, Gyroid) **neben** den bestehenden Primitiven hinzufügen. SDF-Evaluierung für alle Halbraum-Typen implementieren.                                       | `protocol/src/lib.rs`, `csg.rs` (neuer Eval-Pfad)          | `cargo test` grün. Neue Unit-Tests für Halbraum-SDFs.                                   |
| **A.3** | `Contributors` → `BranchId` Refactoring. Kapazität von 4 auf 6–8 Slots erweitern. `evaluate()` liefert Branch-IDs (Halbraum-Leaf-IDs) statt Feature-IDs. Picking und Mesh-Classification umstellen.                          | `csg.rs`, `dc.rs`, `mdc.rs`, `raycast.rs`                  | `cargo test` grün. Picking funktioniert weiterhin.                                      |
| **A.4** | Feature-zu-Halbraum-Expansion: Box → 6 Planes + max, Cylinder → Tube + 2 Planes + max, etc. Alte Primitiv-Typen als Convenience-Layer beibehalten, intern auf Halbräume expandiert.                                          | `csg.rs`, `body.rs`, `sdf_compiler.rs`                     | `cargo test` grün. Rendering identisch (gleiche Geometrie, andere interne Darstellung). |
| **A.5** | GPU-Opcodes für Halbräume: `OP_HALFSPACE_PLANE`, `OP_HALFSPACE_INF_CYLINDER`, etc. in Compiler, GPU-VM und WGSL-Shader. Alte Composite-Opcodes (OP\_PRIM\_BOX) als Optimierung beibehalten.                                  | `sdf_compiler.rs`, `sdfVmInterpreter.ts`, `sdfRenderer.ts` | `npm test` grün. GPU-Rendering identisch.                                               |
| **A.6** | Alte geschlossene Primitive entfernen (sofern alle durch Halbraum-Expansion abgedeckt). Tests und Dokumentation aufräumen.                                                                                                   | `protocol/src/lib.rs`, `csg.rs`, Tests                     | `just test` grün. Kein toter Code.                                                      |

#### Phase B: Analytische Sketch-Kurven

| Schritt | Beschreibung                                                                                                                                                                                           | Betroffene Dateien (Kern)                                                        | Testkriterium                                                                    |
| ------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | -------------------------------------------------------------------------------- | -------------------------------------------------------------------------------- |
| **B.1** | Neue 2D-SDF-Primitiven: `Line`, `EllipticArc`, `QuadraticBezier`, `CubicBezier` mit analytischer Distanzberechnung und Dual-Number-Gradient.                                                           | `sketch2d.rs`, `protocol/src/sketch2d.rs`                                        | Unit-Tests für Distanz und Gradient jedes Kurventyps.                            |
| **B.2** | Segment-basierte Branch-IDs: Jedes Sketch-Segment meldet eigene Branch-ID statt Sketch-ID. G1-Preprocessing: Tangentenstetigkeit prüfen, G1-stetige Segmente unter gemeinsamer Branch-ID verschmelzen. | `sketch2d.rs`, `csg.rs`                                                          | Extrusion eines Rechteck-Sketchs produziert korrekte Branch-IDs an den 4 Kanten. |
| **B.3** | GPU-Opcodes für analytische 2D-Kurven: `OP_PRIM2D_LINE`, `OP_PRIM2D_QUAD_BEZIER`, `OP_PRIM2D_CUBIC_BEZIER`. Frontend-Sketch-Tools um Spline/Bezier-Werkzeuge erweitern.                                | `sdf_compiler.rs`, `sdfVmInterpreter.ts`, `sdfRenderer.ts`, `SketchCanvas2d.tsx` | GPU-Rendering von Bezier-Skizzen korrekt.                                        |

#### Phase C: B-Rep-Extraktion

| Schritt | Beschreibung                                                                                                                                                                | Betroffene Dateien (Kern)                       | Testkriterium                                                                            |   |                                  |
| ------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------- | ---------------------------------------------------------------------------------------- | - | -------------------------------- |
| **C.1** | B-Rep-Datenstrukturen: `Vertex`, `Edge` (NURBS-Kurve), `Face` (Trimmed Surface), `Shell`, `Solid`. Neues Crate oder Modul `brep`.                                           | `crates/brep/` oder `crates/kernel/src/brep.rs` | Kompiliert. Datenstrukturen instanziierbar.                                              |   |                                  |
| **C.2** | Octree-Builder (Phase 1): Zell-Klassifizierung (Leer/Fläche/Kante/Unsicher), Lipschitz-Verfeinerung, Auflösungsgrenze.                                                      | `brep/octree.rs`                                | Unit-Test: Würfel (6 Planes + max) → Octree findet 12 Kanten-Zellen und 8 Vertex-Zellen. |   |                                  |
| **C.3** | Analytische Projektion (Phase 2) + Kanten-/Vertex-Extraktion (Phase 3) + Phantom-Filter.                                                                                    | `brep/projection.rs`, `brep/extraction.rs`      | Unit-Test: Projizierte Kantenpunkte liegen auf Oberfläche (                              | f | \< ε). Phantom-Kanten gefiltert. |
| **C.4** | Edge Walker (Phase 3.5): Topologie-Rekonstruktion. Geordnete Kantenketten von Vertex zu Vertex.                                                                             | `brep/edge_walker.rs`                           | Unit-Test: Würfel → 12 Kantenketten, jeweils von Vertex zu Vertex.                       |   |                                  |
| **C.5** | B-Rep Weaving (Phase 4): Hermite-Spline-Fitting, Wireframe-Assembly, Trimmed Surfaces.                                                                                      | `brep/weaving.rs`                               | Unit-Test: Würfel → wasserdichtes B-Rep mit 6 Faces, 12 Edges, 8 Vertices.               |   |                                  |
| **C.6** | Integration: Bestehende Topology-API (`bodyFaceCount`, `bodyEdgeCount`, `bodyTopologyJson`) von Mesh-Proxy auf echtes B-Rep umstellen. Dirty-Tracking/B-Rep-Cache anbinden. | `body.rs`, `kernel.ts`                          | `just test` grün. Topology-API liefert B-Rep-basierte Werte.                             |   |                                  |

#### Phase D: Integration

| Schritt | Beschreibung                                                                                                                                 | Testkriterium                                      |
| ------- | -------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------- |
| **D.1** | Persistent Naming: Branch-Ancestry-Hashes für Faces/Edges/Vertices. Spatial Disambiguation. Feature-Referenzen speichern Hash + Fingerprint. | Fillet auf Würfelkante überlebt Parameteränderung. |
| **D.2** | Shell/Offset: Leaf-Level-Offset statt Root-Level `abs(sdf) - d`.                                                                             | Shell-Operation erhält scharfe Kanten.             |
| **D.3** | Domain Warps: Iterative Projektion (Phase 2) für gewarpte Felder (2–4 Newton-Raphson-Schritte).                                              | Twist auf Würfel → korrekte B-Rep-Kanten.          |
| **D.4** | STEP-Export: B-Rep-Output als Trimmed NURBS Surfaces serialisieren.                                                                          | STEP-File importierbar in externem CAD-Viewer.     |

***

## Designentscheidungen während der Implementierung

Dieser Abschnitt ist ein **fortlaufendes Protokoll** der Entscheidungen, die beim Schreiben des ursprünglichen Konzepts noch nicht überblickt werden konnten. Jeder Eintrag dokumentiert *Problem → Optionen → gewählte Lösung → Begründung*. Leitlinie: saubere, tragfähige Lösungen vor Quick Wins; im Zweifel die aufwändigere Variante.

### DL-1: Gefüllte 2D-Skizzen aus Kurvensegmenten — Even-Odd-Knoten statt unabhängiger Leaf-Nodes (Schritt B.3)

**Problem.** Das Konzept beschreibt analytische Sketch-Kurven (Punkt 2, Schritte B.1–B.3) und das atomare Sketch-Modell (Abschnitt „Konkave Skizzen und Löcher": unsigned-Distanz = min über Segmente, Vorzeichen = Even-Odd). Es legt aber nicht fest, **als welche Datenstruktur** ein aus mehreren Segmenten (Linie/Bezier/Bogen) bestehender, *füllbarer* Skizzenzug in die SDF-Pipeline eingehängt wird. Die naheliegende Vermutung „jedes Segment ist ein eigener Leaf-Node, per `min` verknüpft" trägt nicht: Das `min` unbesigneter Segment-Distanzen ist stets ≥ 0 → kein gefülltes Inneres, und eine vorzeichenbehaftete Halbraum-Verknüpfung (`max`) funktioniert nur für *konvexe* Skizzen.

**Optionen.**
1. *Segmente als unabhängige CSG-Leaves* (`min`-verknüpft). Verworfen: kein Füll-Vorzeichen, nur konvex.
2. *Segmente als vorzeichenbehaftete Halbebenen, `max`-verknüpft.* Verworfen: nur konvex, kein Loch-/Konkav-Support.
3. *Ein Pfad-Knoten, der min-Distanz + Even-Odd kapselt.* **Gewählt.**

**Gewählte Lösung.** Neue Variante `Sketch2dPrimitive::Path { segments: Vec<SketchSegment>, closed: bool }` als einzelnes 2D-Leaf im Sketch-CSG-Baum:
- **Distanz-Magnitude:** exakt analytisch, `min` über alle Segmente (`SketchSegment::unsigned_distance`).
- **Vorzeichen:** Even-Odd-Strahltest über die zu einer Polyline geflättete *geschlossene* Kontur (`PATH_FLATTEN_SAMPLES = 24` Sub-Segmente pro gekrümmtem Segment; Linien nutzen ihre zwei Endpunkte). Nur das Vorzeichen nutzt die Polyline-Näherung — der Betrag bleibt exakt.
- **GPU/VM:** Ein Composite-Opcode `OP_PRIM2D_PATH` (0x1D) führt min + Even-Odd in einer variabel langen Schleife aus (analog zu `OP_PRIM2D_POLY`). Zusätzlich existieren standalone Einzel-Segment-Opcodes `OP_PRIM2D_LINE/QUAD_BEZIER/CUBIC_BEZIER/ELLIPTIC_ARC` (0x1A–0x1E) als Bausteine und für künftige *offene* Konstruktionskurven.
- **Pro-Segment-Branch-IDs bleiben erhalten:** `Path::sdf_with_edge` liefert den G1-gruppierten Index des nächstgelegenen Segments; `CsgNode::Extrude/Revolve::evaluate()` macht daraus `BranchId::from_feature_sub(feature, segment_index)` (Caps auf eigenen Sub-Indizes). Das ist die direkte Analogie zur Box→6-Halbräume-Expansion aus Punkt 1/Schritt A.4 — die Branch-Granularität ist hier das Segment.

**Begründung.** Das Even-Odd-Vorzeichen ist zwingend eine *pfad-globale* Operation (es zählt Strahlkreuzungen über die ganze geschlossene Kontur) und der einzige Weg, konkave Skizzen und Löcher korrekt zu behandeln. Es kann nicht aus lokalen, unabhängigen Segment-SDFs emergieren. Branch-Tracking (für Picking, Meshing, Phase-C-B-Rep) läuft im `evaluate()`-Pfad, **nicht** im Rendering-Opcode-VM — die GPU braucht nur den SDF-Wert, daher ist der kompakte Composite-Opcode dort korrekt und effizient. Verifiziert durch Test `extruded_path_tracks_per_segment_branch_ids` (csg.rs): extrudierter Pfad aus 3 Linien + 1 Bezier → 4 distinkte Segment-Branch-IDs + 2 Cap-IDs.

### DL-2: B-Rep als Modul im `kernel`-Crate statt eigenständiges Crate (Schritt C.1)

**Problem.** Das Konzept lässt offen: „Neues Crate oder Modul `brep`" (Schritte C.1–C.6, Punkt 5).

**Optionen.**
1. *Eigenes Crate `crates/brep`.* Sauberere Kapselung, aber: Die Extraktion (C.2+) braucht die SDF-Auswertung des Kernels (`sdf_dual`, `BranchId`, `BoundedCsg`, `csg`), und die Topology-API-Integration (C.6) ruft B-Rep aus `body.rs` auf → `kernel` würde von `brep` abhängen, das wiederum von `kernel` → **Zyklus**.
2. *Modul `crates/kernel/src/brep/` im Kernel-Crate.* **Gewählt.**

**Gewählte Lösung.** B-Rep ist ein Modul-Verzeichnis im Kernel-Crate (`crates/kernel/src/brep/mod.rs`, später Submodule `octree.rs`, `projection.rs`, `extraction.rs`, `edge_walker.rs`, `weaving.rs`). Datenstrukturen sind ein **Index-Arena-B-Rep** (Newtype-Handles `VertexId/EdgeId/FaceId/ShellId/SolidId` indizieren flache `Vec`-Arenen im `BRep`-Container) — kein `Rc`/`RefCell`, klonbar, serde-serialisierbar.

**Begründung.** Der Zyklus aus Option 1 wäre nur über ein künstliches drittes Crate oder Trait-Inversion auflösbar — unnötige Komplexität in früher Dev-Phase. Ein Modul vermeidet ihn vollständig und gibt der Extraktion direkten Zugriff auf die Kernel-internen SDF-Typen. Eine spätere Extraktion in ein eigenes Crate bleibt möglich, falls die Build-Zeiten es erzwingen.

### DL-3: Octree-Zell-Klassifizierung — Surface-Proximity-Guard und auflösungsrobustes Testkriterium (Schritt C.2)

**Problem A — Medial-Axis-Artefakte.** Die Phase-1-Klassifizierungstabelle definiert eine **Kante** als „Branch-IDs unterschiedlich" und einen **Vertex** als ≥3 kollidierende Branches. Naiv angewandt liefert das falsche Features *tief im Inneren* eines Körpers: Im Zentrum eines Würfels ziehen alle 6 Flächen-Halbräume am Inradius gleich → eine Zelle dort meldet 6 Branches und würde als (spuriöser) Vertex klassifiziert. Das ist die Medial Axis (Skelett), keine Oberflächentopologie. Der Würfel ergäbe 8 echte + 1 falschen Vertex.

**Gewählte Lösung.** **Surface-Proximity-Guard:** Edge-/Vertex-Klassifizierung wird zusätzlich an `f_min ≤ diag` gekoppelt (kleinster Eckpunkt-Absolutwert ≤ Zelldiagonale — die Lipschitz-Klammer). Ein echtes Feature überspannt zwingend das Null-Level-Set, also liegt mindestens ein Eckpunkt innerhalb einer Diagonale der Oberfläche. Multi-Branch-Ties mit `f_min > diag` sind Medial-Axis-/Voronoi-Artefakte und werden wie *Leer* verworfen. Damit ergibt der Würfel exakt 12 Kanten + 8 Vertices. (Dies ist die Operationalisierung des im Abschnitt „Verwerfungen im SDF-Raum" beschriebenen Phantom-Problems bereits auf Octree-Ebene, vor der eigentlichen Phantom-Filterung in Phase 3.)

**Problem B — „12 Kanten-Zellen, 8 Vertex-Zellen" ist auflösungsabhängig.** Ein bis zur Auflösungsgrenze (z.B. 0.1 mm) verfeinerter Octree erzeugt *hunderte* roher Kanten-/Vertex-Zellen pro Würfelkante (Würfel @ 0.25 → 792 Kanten- + 112 Vertex-Zellen). Eine Assertion auf rohe Zellzahlen wäre brüchig und auflösungsabhängig.

**Gewählte Lösung.** Das Testkriterium prüft **distinkte topologische Signaturen** statt roher Zellzahlen: Anzahl eindeutiger kanonischer Branch-Paare der Kanten-Zellen (= 12 für den Würfel) und eindeutiger sortierter Branch-Mengen der Vertex-Zellen (= 8). Das ist auflösungsunabhängig und beschreibt die wahre Würfeltopologie. API: `Octree::distinct_edge_signatures()` / `distinct_vertex_branch_sets()`.

**Weitere C.2-Tunables (Standardwerte, dokumentiert):** Sampling an 8 Ecken + Zentrum (Zentrum reichert nur die Branch-Menge an; `sign_change`/`f_min` über die 8 Ecken); `eps = diag · 0.5` für `evaluate`; `min_cell_size = 0.1`, `max_depth = 8` (Hang-Schutz); Root = Körper-AABB mit kleinem Margin. `Uncertain` ist ein interner Verfeinerungszustand, kein gespeichertes Leaf.

### DL-4: Per-Branch-Startpunkte für die Projektion — Voronoi-eindeutige Ecke + Sole-Contributor-Präferenz (Schritt C.3)

**Problem.** Phase 2 projiziert je einen Punkt pro kollidierendem Branch auf dessen Null-Oberfläche, um die Tangentialebene (`n_a`/`n_b` bzw. `n_i`) zu gewinnen. Dafür braucht jeder Branch einen Startpunkt, der **eindeutig in seinem Voronoi-Gebiet** liegt. Auf der Verwerfung selbst sind beide Branches gleichzeitig aktiv, der Gradient ist mehrdeutig (am C⁰-Knick) und kollabiert sogar auf Null — dort gestartet liefert die Newton-Iteration keine wohldefinierte Flächennormale.

**Optionen.**
1. *Zell-Mittelpunkt für alle Branches verwenden.* Verworfen: liegt typischerweise nahe der Verwerfung → mehrdeutige Normale.
2. *Beliebige Ecke, die den Branch enthält.* Verworfen: kann eine Kanten-/Eck-Ecke sein, an der mehrere Branches aktiv sind → kein sauberer Tangentenraum.
3. *8 Ecken + Zentrum mit `EvalMode::Logical` abtasten; pro Branch die beste Ecke wählen.* **Gewählt.**

**Gewählte Lösung.** Für jeden Branch `X` werden die 8 Zell-Ecken + Zentrum mit `evaluate(.., Logical)` ausgewertet. Eine Probe „gehört" `X`, wenn ihr Contributor-Slice `X` enthält. Auswahlpriorität: (a) Proben, bei denen `X` **alleiniger** Contributor ist (`slice.len() == 1` → sauberster Tangentenraum, klar in `X`' Gebiet), schlagen geteilte Proben kategorisch; (b) innerhalb einer Klasse gewinnt das kleinste `|f|` (nächster an der Oberfläche → bester Newton-Seed). Diese Probe wird via Phase 2 projiziert. Findet sich für einen Branch **keine** Probe (oder divergiert die Projektion), wird die ganze Zelle als nicht-extrahierbar übersprungen — es wird **nie** ein Phantom erzeugt.

**Begründung.** Die Sole-Contributor-Präferenz garantiert, dass der Startpunkt im Inneren des Branch-Halbraums (weg von der Verwerfung) liegt, wo der Gradient die saubere Flächennormale ist; das kleinste `|f|` minimiert die Newton-Schritte. Beides zusammen ist robuster als jede mittelpunkts- oder einzelproben-basierte Heuristik und kommt ohne zusätzliche Feld-Auswertungen jenseits der ohnehin nötigen 9 Proben aus.

### DL-5: ε_phantom relativ zur Zellgröße statt absoluter Projektions-Toleranz (Schritt C.3)

**Problem.** Der Phantom-Filter wertet `f(p_edge)` bzw. `f(p_vert)` erneut aus und behält den Punkt nur bei `|f| < ε_phantom`. Welcher Wert für `ε_phantom`? Die Projektions-Toleranz (`1e-6`) ist hier **falsch**: Die Ebenen-Schnitt-Linearisierung (Phase 3) sitzt bei *gekrümmten* Flächen prinzipiell nicht exakt bei `f = 0` — der lineare Schnittpunkt der Tangentialebenen weicht um einen Betrag in der Größenordnung der Zellkrümmung·Zellgröße ab. Ein `ε_phantom = 1e-6` würde fast jede echte Kante an gekrümmten Flächen fälschlich als Phantom verwerfen.

**Optionen.**
1. *`ε_phantom = ε_projection` (1e-6).* Verworfen: verwirft echte Kanten an gekrümmten Flächen.
2. *Fester absoluter Wert (z.B. 0.05 mm).* Verworfen: auflösungsabhängig falsch — zu groß für feine Zellen, zu klein für grobe.
3. *`ε_phantom = Faktor · Zelldiagonale`.* **Gewählt** (`PHANTOM_EPS_FACTOR = 0.25`).

**Gewählte Lösung.** `ε_phantom = 0.25 · diag(Zelle)`. Die Toleranz skaliert mit der Zellgröße, denn genau diese begrenzt den Linearisierungsfehler des Tangentialebenen-Schnitts. Der Faktor 0.25 (ein Viertel der Diagonale) ist großzügig genug, um echte Features an realistisch gekrümmten Flächen zu behalten, und eng genug, um interne Medial-Axis-Punkte (`f ≪ 0`) und externe Voronoi-Gräben (`f ≫ 0`) — die typischerweise um ≫ eine halbe Zelle daneben liegen — zuverlässig zu verwerfen. (Komplementär zum Surface-Proximity-Guard aus DL-3, der grobe Medial-Axis-Ties bereits auf Octree-Ebene aussortiert; der Phantom-Filter fängt die feineren Fälle nach der analytischen Projektion ab.)

### DL-6: Vertex-Least-Squares via Normalengleichungen + SVD (nalgebra) statt Cramer (Schritt C.3)

**Problem.** Ein Vertex (≥3 Branches) ist der Schnittpunkt von ≥3 Tangentialebenen `n_i · x = n_i · p_i` — ein überbestimmtes System, das im Least-Squares-Sinn zu lösen ist. Die Konzept-Vorgabe nennt „3×3 oder überbestimmt via SVD". Frage: eigene Cramer-Lösung der Normalengleichungen oder eine vorhandene SVD?

**Optionen.**
1. *Cramer/Determinante auf den 3×3-Normalengleichungen `AᵀA x = Aᵀb`, ohne neue Dependency.* Tragfähig, aber der Singularitäts-Guard über `det ≈ 0` ist numerisch schwächer als Singulärwerte.
2. *SVD über `nalgebra`.* **Gewählt** — `nalgebra = "0.33"` ist bereits Workspace-Dependency (genutzt von `sdf_dual.rs`).

**Gewählte Lösung.** Die Normalengleichungen `AᵀA = Σ n_i n_iᵀ`, `Aᵀb = Σ n_i (n_i · p_i)` werden als `Matrix3`/`Vector3` akkumuliert und per `Matrix3::svd().solve()` gelöst. Singularitäts-Guard: der kleinste Singulärwert muss `≥ 1e-9` sein (sonst pinnen die Ebenen keinen eindeutigen Punkt → Vertex verwerfen). Die analoge Kanten-Lösung (genau 2 Ebenen + die `t·x = t·c`-Gleichung, die auf der Schnittgeraden den zu `c` nächsten Punkt wählt) ist exakt 3×3 und wird per `try_inverse()` mit `|det| ≥ 1e-9`-Guard gelöst.

**Begründung.** Keine neue Crate-Dependency nötig (DL-Regel erfüllt), und SVD liefert den numerisch sauberen Rang-Defekt-Test über Singulärwerte statt über die schlechter konditionierte Determinante — die robustere Variante, wie vom Projekt-Prinzip „kein Quick-Win" gefordert. Die Normalengleichungen halten das System bei beliebig vielen Branches auf festen 3×3 (kein dynamisch dimensioniertes `DMatrix`), was Allokationen vermeidet.

### DL-7: Zwischen-Output-Struct `EdgePoint` (f64) statt sofortiger `Edge`-Konstruktion (Schritt C.3)

**Problem.** Phase 3 produziert *pro Octree-Kanten-Zelle* genau einen Punkt + Tangente + Signatur. Der `mod.rs`-Typ `Edge` ist dagegen eine *ganze* topologische Kante (mit `EdgeGeometry`-Kette, `start`/`end`-Vertices). Ein `Edge` lässt sich hier noch nicht bilden — das Verketten zu Ketten und das Spline-Fitting sind C.4/C.5.

**Gewählte Lösung.** Ein schlankes `pub struct EdgePoint { position: [f64;3], tangent: [f64;3], signature: (BranchId, BranchId) }` als Phase-3-Zwischenprodukt. `f64` passend zu den B-Rep-Geometrietypen (das Feld evaluiert intern in f32, gespeichert wird in f64). Vertices dagegen *werden* direkt als `mod.rs`-`Vertex` erzeugt, da ein Vertex bereits ein vollständiges topologisches 0-Zell ist.

**Begründung.** Saubere Phasentrennung: C.3 liefert rohe, ungeordnete Stützstellen; der Edge-Walker (C.4) konsumiert `Vec<EdgePoint>` und verkettet sie anhand der Signaturen zu `Edge`-Ketten. `EdgePoint` nicht zu Ketten zu verknüpfen oder Splines zu fitten ist explizit Scope-Grenze von C.3.

### DL-8: Projektions-Parameter (ε, max_iter) und Behandlung nicht-extrahierbarer Zellen (Schritt C.3)

**Problem.** Phase 2 braucht eine Abbruch-Toleranz `ε` und ein Iterations-Cap; Phase 3 muss entscheiden, was bei Zellen passiert, die keinen sauberen Output liefern.

**Gewählte Lösung.** `PROJECT_EPS = 1e-6` (f32-Oberflächentoleranz, gemäß Konzept), `PROJECT_MAX_ITER = 16`. Begründung Cap: euklidische SDFs konvergieren in 1 Schritt, gewarpte Felder in 2–4 (Konzept Z.217); 16 ist ein großzügiger Hang-Schutz. **Nicht-extrahierbare Zellen** (kein Per-Branch-Startpunkt, degenerierte Tangente `|n_a × n_b| ≈ 0`, singuläres Gleichungssystem, oder durchgefallener Phantom-Filter) liefern `None` und werden **stillschweigend übersprungen** — niemals wird ein degeneriertes/Phantom-Feature erzeugt. Zusatz-Detail der Projektion: min/max-komponierte SDFs (Box, CSG) haben **exakt auf einer Fläche** einen kollabierenden Dual-Number-Gradienten (C⁰-Knick → `∇f = 0`). Da der Konvergenz-Schritt genau dort landet, merkt sich `project_to_surface` die letzte *gesunde* Normale (vom letzten Off-Surface-Iterat) und gibt diese zurück, statt fälschlich `None` zu liefern.

### DL-9: B-Rep-Gradient in `EvalMode::Logical` statt `Visual` (Schritt C.3)

**Problem.** Phase 2/3 beziehen Feldwert und Gradient über `octree::surface_gradient`, das in C.2 als dünner Re-Export von `sdf_dual::sdf_gradient` angelegt wurde. `sdf_gradient` ist jedoch hart auf `EvalMode::Visual` verdrahtet (der Default für Mesher/Rendering, mit *weichen* smin/smax-Blends). Der restliche B-Rep-Pfad — Octree-Klassifizierung (DL-3) und Per-Branch-Sampling (DL-4) — wertet dagegen durchgängig in `EvalMode::Logical` aus (harte CSG, wohldefinierte Branches), weil Kanten konzeptuell nur in der harten Interpretation existieren. Die Projektion und der Phantom-Filter liefen damit über den falschen Modus.

**Optionen.**
1. *Visual beibehalten.* Verworfen: in einer aktiven Blend-Zone (Fillet, Radius ≈ k) würde die Newton-Iteration auf die **verrundete** Fläche projizieren und als Normale den geglätteten Blend-Gradienten liefern statt der harten Branch-Halbraum-Normale — inkonsistent mit der Logical-Klassifizierung, die die Zelle überhaupt erst als Kante markiert hat. Der Phantom-Filter testete zudem den Visual-Feldwert.
2. *`surface_gradient` modusbewusst machen und in `Logical` auswerten.* **Gewählt.**

**Gewählte Lösung.** `sdf_dual::sdf_gradient` wird in eine modus-parametrierte `sdf_gradient_mode(node, p, mode)` refaktoriert; `sdf_gradient` bleibt als `Visual`-Wrapper für alle bestehenden Render-/Mesh-Aufrufer unverändert. `octree::surface_gradient` ruft nun `sdf_gradient_mode(.., EvalMode::Logical)`. Damit projiziert C.3 auf dieselbe harte Feld-Interpretation, die der Octree klassifiziert.

**Begründung.** Konsistenz des gesamten B-Rep-Pfads auf `Logical`. Für blend-freie Geometrie (alles, was C.1–C.3 testet: einzelne Box-Leaves, hartes `max`-Difference) sind beide Modi **bitidentisch** — der Fix ändert kein aktuelles Testverhalten, schließt aber eine latente Korrektheits-Naht, die sonst beim ersten Zusammentreffen von Blends und B-Rep-Extraktion (spätestens Phase D / C.6) auftreten würde. Sauberer Fix vor Quick-Win-Weglassen, gemäß Projekt-Leitlinie.

### DL-10: Edge Walker liefert eine Wireframe-`BRep` (Vertices + Edges, Faces vertagt) (Schritt C.4)

**Problem.** Phase 3.5 muss die ungeordnete `Vec<EdgePoint>` + Vertex-Punktwolke aus Phase 3 in Topologie überführen. Offen war, *welchen* Rückgabetyp `build_wireframe` produziert: nur geordnete Ketten (`Vec<Vec<HermiteSample>>`), oder bereits eine befüllte `BRep`?

**Optionen.**
1. *Rohe Ketten zurückgeben, BRep erst in C.5/C.6 zusammenbauen.* Verworfen: die Vertex-Dedup und das Endpunkt-Snapping müssen ohnehin in C.4 passieren (sie brauchen Vertices und Ketten gleichzeitig); das Ergebnis ohne `BRep`-Hülle weiterzureichen würde diese Zuordnung an C.5 verlieren.
2. *Eine `BRep` mit gefüllten `vertices` + `edges`, `faces`/`shells`/`solids` leer.* **Gewählt.**

**Gewählte Lösung.** `build_wireframe(body, octree) -> BRep` gibt ein Drahtgittermodell zurück: deduplizierte `Vertex`es plus ein `Edge` pro Kette mit `EdgeGeometry::Hermite(geordnete Samples)` und aufgelösten `start`/`end`-`VertexId`s. Faces/Shells/Solids bleiben leer — das ist C.6 (Weaving). Der Spline-Fit (`Hermite → Nurbs`) ist C.5; C.4 liefert bewusst die rohen Hermite-Ketten, damit C.5 unabhängig fitten kann.

**Begründung.** Die `BRep` ist der natürliche, schon vorhandene Container (C.1) und macht das Wireframe sofort über `validate()`/`vertex_count()`/`edge_count()` prüfbar. Leere Face-Arena ist legitim (`validate()` akzeptiert sie). Keine Zwischen-Typen, kein Re-Mapping — gemäß „Keine Backward Compatibility / sauberer Code".

### DL-11: Greedy-räumlicher Walk statt wörtlichem Octree-26-Nachbarn-Walk (Schritt C.4)

**Problem.** Das Konzept beschreibt den Walk als Sprung von Zelle zu Zelle über die 26 direkten Octree-Nachbarn, immer der Signatur `(A,B)` folgend. Das erfordert einen Zell-Adjazenz-Index (räumliches Hashing der Leaf-Zellen auf ihr Gitter), den der Octree (C.2) nicht bereitstellt.

**Optionen.**
1. *26-Nachbarn-Index nachrüsten.* Ein `HashMap` von Gitter-Koordinate → Zelle, dazu die Nachbar-Iteration. Korrekt, aber: der Octree ist adaptiv (Zellen verschiedener Größe), ein sauberer 26-Nachbar-Begriff existiert nur auf gleicher Tiefe — der Index wäre aufwändig und fragil.
2. *Greedy-Nearest-Neighbour-Walk im Raum innerhalb einer Signatur-Gruppe.* **Gewählt.**

**Gewählte Lösung.** Pro Signatur-Gruppe wird ein Startpunkt am räumlichen Extremum (kleinstes `x`, dann `y`, dann `z`) gewählt und wiederholt der nächste noch ungenutzte Punkt angehängt. Das ist **äquivalent** zum 26-Nachbarn-Walk, *solange die Signatur lokal eindeutig ist* — genau die Robustheits-Annahme, die das Konzept selbst macht (zwei Kanten gleicher Signatur kreuzen sich nicht in derselben Zelle).

**Begründung.** Kein Adjazenz-Index nötig, robust gegen die adaptive Octree-Tiefe, und unter der konzepteigenen Annahme verhaltensgleich. Die quadratische Nearest-Neighbour-Suche ist für die ~Dutzend bis Hundert Punkte pro Kante unkritisch; ein k-d-Baum wäre vorzeitige Optimierung.

### DL-12: Ketten-Split-Heuristik über medianer lokaler Punktdichte (Schritt C.4)

**Problem.** Zwei räumlich getrennte Teilkanten können dieselbe Signatur `(A,B)` tragen (z.B. zwei parallele Schnitte derselben zwei Flächen). Ein einzelner Greedy-Walk würde sie über die große Lücke hinweg fälschlich zu einer Kette verschweißen.

**Optionen.**
1. *Feste absolute Lücken-Schwelle.* Verworfen: hängt von der Modellskala und der Octree-Auflösung ab.
2. *Lücke relativ zur lokalen Punktdichte.* **Gewählt.**

**Gewählte Lösung.** Der Walk bricht die aktuelle Kette ab und startet eine neue, sobald der Nearest-Neighbour-Abstand `CHAIN_GAP_FACTOR = 3.0 ×` die **mediane** lokale Punktdichte (Median der strikt positiven Nearest-Neighbour-Abstände der Gruppe) übersteigt.

**Begründung.** Entlang einer einzelnen Kante sitzen aufeinanderfolgende Samples etwa einen Octree-Zellabstand auseinander (Dichte ≈ konstant), während die Lücke zu einer *disjunkten* Teilkante mindestens in der Größenordnung der Feature-Größe liegt — viele Mediane breit. Der Median (statt Mittelwert) ist robust gegen einzelne große Lücken, die ihn sonst aufblähen würden. Faktor 3 lässt gewöhnliches Sampling-Jitter passieren und trennt erst echte Sprünge.

### DL-13: Vertex-Dedup nach Branch-Menge + Zentroid (Schritt C.4)

**Problem.** Phase 3 liefert *einen* `Vertex` pro Vertex-*Zelle*; um eine Würfelecke herum melden viele Zellen dasselbe Branch-Tripel (beim 2³-Würfel @0.25: 40 Roh-Vertices für 8 Ecken). Diese müssen zu je einem topologischen Vertex kollabiert werden.

**Optionen.**
1. *Dedup nach räumlicher Nähe (Clustering der Positionen).* Verworfen: braucht eine Distanzschwelle und kann nahe Ecken fälschlich verschmelzen.
2. *Dedup nach kanonischer Branch-Menge.* **Gewählt.**

**Gewählte Lösung.** Roh-Vertices werden über ihre `Vertex::branches` (von `Vertex::new` bereits sortiert+dedupliziert) gruppiert; die finale Position ist der **Zentroid** aller Zell-Positionen derselben Branch-Menge. Ein sauberer Würfel kollabiert exakt auf 8 Vertices.

**Begründung.** Die Branch-Menge ist die exakte topologische Identität einer Ecke (3 kollidierende Halbräume = genau eine Ecke), auflösungs-unabhängig und ohne magische Distanzschwelle. Der Zentroid mittelt das Sub-Pixel-Rauschen der Einzelzellen-Lösungen heraus.

### DL-14: Vertex-Snapping-Toleranz relativ zur Ketten-Punktdichte (Schritt C.4)

**Problem.** Die Endpunkte einer Kette müssen den begrenzenden Vertices zugeordnet werden. Der letzte Edge-Sample sitzt aber ~eine Zelle *vor* der echten Ecke, denn die Ecke ist eine Vertex-Zelle (≥3 Branches), keine Edge-Zelle (2 Branches) — zwischen letztem Sample und Vertex klafft eine systematische Lücke.

**Optionen.**
1. *Exakte Übereinstimmung verlangen.* Unmöglich — die Lücke ist immer da.
2. *Snapping innerhalb einer dichte-relativen Toleranz, nur auf Kandidaten-Vertices mit Branch-Menge ⊇ {A,B}.* **Gewählt.**

**Gewählte Lösung.** Kandidaten-Vertices einer Kette `(A,B)` sind genau die deduplizierten Vertices, deren Branch-Menge `{A,B}` enthält. Jeder Ketten-Endpunkt snappt auf den nächsten Kandidaten innerhalb `SNAP_FACTOR = 2.5 ×` der Ketten-Punktdichte.

**Begründung.** Die zu überbrückende Lücke ist ~eine Punktdichte (eine Zelle); 2.5 deckt sie plus den Halb-Zellen-Versatz des Vertex-Zentroids ab, bleibt aber deutlich unter einer ganzen Kantenlänge, sodass nie die *andere* Ecke derselben Kante erwischt wird. Die Branch-Mengen-Filterung schließt geometrisch nahe, aber topologisch fremde Vertices aus.

### DL-15: Closed-Loop-Erkennung über zusammentreffende Ketten-Enden (Schritt C.4)

**Problem.** Eine kreisförmige Kante (z.B. Zylinder-in-Box-Schnitt) hat keine begrenzenden Vertices — sie ist eine geschlossene Schleife (`start = end = None`). Sie muss von einer offenen Kante mit fehlgeschlagenem Snapping unterschieden werden.

**Optionen.**
1. *Immer offen lassen, wenn kein Vertex snappt.* Verworfen: eine echte Schleife bliebe fälschlich „offen mit `None`-Enden" ohne semantische Loop-Markierung — und eine kurze Kante, deren beide Enden zufällig keinen Vertex finden, würde als Loop fehlklassifiziert.
2. *Loop nur, wenn kein Kandidat snappt **und** die beiden Ketten-Enden räumlich zusammentreffen.* **Gewählt.**

**Gewählte Lösung.** Eine Kette ist eine geschlossene Schleife, wenn (a) weder Start- noch End-Snapping einen Vertex findet, (b) die Kette ≥3 Samples hat und (c) der Abstand ihrer beiden Enden `≤ LOOP_FACTOR = 2.5 ×` Punktdichte ist. Andernfalls bleiben die (ggf. `None`-) Snap-Ergebnisse als offene Enden stehen.

**Begründung.** Die Kombination „keine Kandidaten-Vertices" (eine glatte Kreiskante kollidiert nur mit 2 Branches, erzeugt also nie eine ≥3-Branch-Ecke) und „Enden treffen sich" ist das eindeutige Signatur einer Schleife. Die `≥3`-Sample-Bedingung verhindert, dass ein degenerierter Zwei-Punkt-Stub als Loop durchgeht.

### DL-16: Tangenten-Orientierung entlang der Walk-Richtung (Schritt C.4)

**Problem.** Die Edge-Tangente aus Phase 3 ist `t = n_a × n_b` — vorzeichen-mehrdeutig, sie kann von Zelle zu Zelle kippen. Ein Hermite-Spline mit uneinheitlich orientierten Tangenten oszilliert.

**Optionen.**
1. *Tangenten roh übernehmen.* Verworfen: kippende Vorzeichen brechen den Spline-Fit (C.5).
2. *Jede Tangente auf die lokale Walk-Vorwärtsrichtung ausrichten.* **Gewählt.**

**Gewählte Lösung.** Beim Aufbau der geordneten Kette wird jede Tangente, deren Skalarprodukt mit der lokalen Vorwärtsrichtung (Segment zum nächsten Punkt; beim letzten Punkt das vorherige Segment) negativ ist, gespiegelt. Die Hermite-Kette ist damit monoton start→end orientiert.

**Begründung.** Eine konsistent orientierte Tangentenkette ist Voraussetzung für einen sauberen kubischen Hermite-/NURBS-Fit (C.5) und macht die Kette per Konstruktion entlang ihrer variierenden Achse monoton (so vom C.4-Test geprüft).

### DL-17: Edge-Punkt-Dedup vor dem Chaining (Schritt C.4)

**Problem.** Phase 3 liefert einen Edge-Punkt pro Edge-*Zelle*; entlang einer **geraden** Kante projizieren mehrere benachbarte Zellen aber auf *denselben* analytischen Punkt. Die Roh-Gruppe ist dadurch voller exakter Duplikate (beim Würfel @0.25: 44 Punkte je Kante, viele koinzident). Folge: der naive Median der Nearest-Neighbour-Abstände wird durch die Null-Abstände der Duplikate auf 0 gezogen — alle Schwellen kollabieren, und der Walk produzierte 0 Kanten.

**Optionen.**
1. *Median über alle Roh-Punkte.* Verworfen — exakt der Bug: Median = 0 wegen koinzidenter Punkte.
2. *Duplikate vor dem Chaining zu Repräsentanten verschmelzen; Dichte aus den distinkten Überlebenden.* **Gewählt.**

**Gewählte Lösung.** Pro Signatur-Gruppe werden Punkte näher als `eps = 0.25 ×` (Median der **strikt positiven** Nearest-Neighbour-Abstände) zu einem Repräsentanten zusammengefasst (gemittelte Position + vorzeichen-ausgerichtet gemittelte Tangente). Die charakteristische Punktdichte wird anschließend auf den duplikatfreien Überlebenden neu berechnet und speist Gap-/Snap-/Loop-Schwellen.

**Begründung.** `eps` leitet sich selbst-skalierend aus der Octree-Auflösung ab (kein externer Parameter), die strikt-positive Median-Definition ist robust gegen den Koinzidenz-Bug, und die Tangenten-Mittelung mit Vorzeichen-Ausrichtung glättet das Per-Zelle-Rauschen, bevor die Kette gebaut wird.

### DL-18: Half-Space-Aufzählung als Single Source of Truth, Hot-Path-Refactor für Box/Cylinder (Schritt C.5)

**Problem.** Der Weaver braucht pro Branch die analytische `HalfSpace`-Fläche. Die Branch→Flächen-Zerlegung lebte bisher *nur* inline in `expand_evaluate_leaf` (heißer SDF-Pfad). Eine zweite, separat gepflegte Kopie im Weaver würde unweigerlich auseinanderdriften (z. B. bei einer Normalen-Reihenfolge-Änderung).

**Optionen.**
1. *Zweite Aufzählung im Weaver, Doku-Querverweis "keep in sync".* Verworfen — driftanfällig.
2. *Aufzählungsfunktion `primitive_halfspaces` neben `expand_evaluate_leaf` in `csg.rs`, und den heißen Pfad zero-cost daraus speisen.* **Gewählt** — für Box/Cylinder vollständig, da deren Sub-Faces reine Half-Spaces im zentrierten Frame sind.

**Gewählte Lösung.** `primitive_halfspaces(prim) -> Option<PrimitiveHalfSpaces>` gibt `center` plus bis zu sechs `HalfSpaceFace { sub, half_space }` in einem **stack-allokierten Fixed-Array** (kein Heap) zurück. `expand_evaluate_leaf` konsumiert das für `Box`/`Cylinder` direkt (identische Mathematik, keine Allokation). Der `Cone`-Pfad behält seine bestehende Wand-SDF (`r − r_at_y`, abgeschnittener Kegel) im Hot Path; `primitive_halfspaces` liefert für Cone die Kappen als Ebenen und die Wand als `HalfSpace::Cone` (unendlicher Apex) nur für den Weaver. Diese Divergenz ist dokumentiert, weil ein Umbau der Cone-Wand das numerische Verhalten ändern würde — out of scope für den Würfel-Pfad.

**Begründung.** Eine einzige Quelle für Box/Cylinder (die für C.5 relevante Würfel-Acceptance), keine Heap-Kosten im Hot Path (Fixed-Array), und für Cone ein ehrlicher, dokumentierter Kompromiss statt eines riskanten Verhaltensänderungs-Refactors.

### DL-19: Branch→(HalfSpace,Transform)-Lookup per BoundedCsg-Walk (Schritt C.5)

**Problem.** Es existiert **kein** Reverse-Lookup `BranchId → Fläche`. Der Weaver braucht ihn aber, um jeder Face ihre analytische Surface zuzuordnen.

**Optionen.**
1. *Reverse-Map am `BranchId`/`Contributors` anhängen.* Verworfen — diese Typen sind protokollnah und sollen nicht aufgebläht werden.
2. *Map zur Laufzeit aus dem `BoundedCsg`-Baum aufbauen.* **Gewählt.**

**Gewählte Lösung.** `branch_halfspace_map(body) -> HashMap<u32, (HalfSpace, Transform)>` traversiert den `BoundedCsg` rekursiv, ruft pro Leaf `primitive_halfspaces` (DL-18) auf und trägt für jede Sub-Face `BranchId::from_feature_sub(id, sub).as_u32()` ein. Boolean-/Modifier-Knoten werden in die Kinder hinein durchgereicht.

**Begründung.** Die Map wird nur einmal pro Weave gebaut, lebt komplett im `weaving`-Modul und hält `BranchId`/`HalfSpace`/`Transform` unverändert. Da die Geometrie aus `primitive_halfspaces` stammt, ist sie deckungsgleich mit dem Branch-Tracking des SDF-Pfads.

### DL-20: Primitive-`center` als lokale Pre-Translation in die Face-Transform (Schritt C.5)

**Problem.** Der Primitive-`center`-Offset steckt **nicht** im `world_transform` — er wird intern als `lp = p − center` angewandt. Die auf einer Face gespeicherte `Transform` muss aber das volle Local→World abbilden, sonst säße die analytische Fläche falsch.

**Optionen.**
1. *`center` ignorieren* — falsch für verschobene Primitive.
2. *`center` als Translation in den `world_transform` komponieren.* **Gewählt** — aber Reihenfolge beachten.

**Gewählte Lösung.** Die Half-Space ist im zentrierten Frame definiert (`p = lp + center`). `center` ist also eine Translation im *lokalen* Frame, die **vor** dem `world_transform` wirkt: `face_transform = compose_transforms(world_transform, translate(center))` (`compose_transforms(parent, child)` = `parent ∘ child`, child zuerst). Beim zentrierten Würfel ist `center = 0` ⇒ Identität.

**Begründung.** Spiegelt exakt die Hot-Path-Semantik `world_to_local(world_transform, p)` gefolgt von `lp = p − center` und ist damit konsistent mit der SDF-Auswertung.

### DL-21: Chord-Length-Magnitude für die kubischen Bézier-Tangenten (Schritt C.5)

**Problem.** Die Hermite-Tangenten aus dem Edge-Walk sind **Einheitsvektoren** (nur Richtung). Ein kubischer Bézier braucht jedoch Tangenten-*Beträge*: `b1 = p_i + m_i/3`, `b2 = p_{i+1} − m_{i+1}/3`. Mit Einheitsbetrag (≈ 1) entstünden auf langen Segmenten gestauchte, auf kurzen überschwingende Kurven.

**Optionen.**
1. *Einheitsbetrag belassen.* Verworfen — kein Bezug zur Segmentlänge.
2. *Catmull-Rom-artige Skalierung mit Nachbarsegment-Längen.* Mehraufwand, Vorteil bei dieser bereits dichten, monotonen Sample-Kette gering.
3. *Pro Segment beide Tangenten mit der **Sehnenlänge** dieses Segments skalieren.* **Gewählt.**

**Gewählte Lösung.** `m_i = unit(t_i) · ‖p_{i+1} − p_i‖` und `m_{i+1} = unit(t_{i+1}) · ‖p_{i+1} − p_i‖`. Für ein gerades Segment mit achsenparallelen Tangenten ergeben sich exakt die Drittelpunkte (`b1 = p_i + chord/3`, `b2 = p_{i+1} − chord/3`) ⇒ kollineare Kontrollpunkte, geometrisch eine Linie.

**Begründung.** Selbst-skalierend (kein Parameter), erzeugt für gerade Kanten beweisbar kollineare Kontrollpunkte (im Unit-Test verifiziert) und vermeidet Überschwinger, da der Tangentenbetrag der lokalen Segmentlänge folgt.

### DL-22: Piecewise-Bézier → eine geklemmte NURBS mit C⁰-Innenstößen (Schritt C.5)

**Problem.** `s` aufeinanderfolgende Hermite-Segmente liefern `s` kubische Béziers. Diese müssen zu *einer* `NurbsCurve` (degree 3) zusammengesetzt werden, unter Wahrung der Invarianten `weights.len() == control_points.len()` und `knots.len() == control_points.len() + degree + 1`.

**Optionen.**
1. *Je Segment eine eigene NURBS-Kurve.* Verworfen — eine Kante ist topologisch eine Kurve.
2. *Eine geklemmte NURBS, geteilte Bézier-Endpunkte einmal gespeichert, Innenstöße mit Multiplizität = degree (C⁰).* **Gewählt.**

**Gewählte Lösung.** `3·s + 1` Kontrollpunkte (geteilte Endpunkte einmal). Knotenvektor: Endmultiplizität `degree + 1 = 4`, jeder innere Segmentstoß bei Parameter `1, 2, …` mit Multiplizität `degree = 3` (C⁰-Bézier-Stoß). Länge = `(degree+1) + (s−1)·degree + (degree+1) = 3s + 5 = n + degree + 1`. ✓

**Begründung.** C⁰-Innenstöße sind die korrekte Wahl, weil die Segmente an den Sample-Punkten nur Positions-, nicht zwingend Krümmungsstetigkeit teilen; das Format ist direkt STEP/IGES-exportierbar und die Invarianten sind per Konstruktion (und `debug_assert`) erfüllt.

### DL-23: Loop-Ordering per Shared-Vertex-Walk (Schritt C.5)

**Problem.** Die Kanten einer Branch-Fläche liegen unsortiert vor; eine `Loop` braucht sie aber in Umlaufreihenfolge mit korrektem `reversed`-Flag.

**Optionen.**
1. *Geometrische Winkel-Sortierung in der Flächenebene.* Komplexer, ebenenabhängig.
2. *Topologischer Walk über geteilte Vertices.* **Gewählt.**

**Gewählte Lösung.** Start mit Kante 0 (vorwärts `start→end`), dann wiederholt die unbenutzte Kante suchen, deren `start` *oder* `end` dem aktuellen Walk-Vertex gleicht; `reversed = true`, wenn der Stoß an ihrem `end` liegt. Schließt der Walk auf den allerersten Vertex zurück, ist die `Loop` gültig; sonst (offene Kette/Verzweigung) `None` ⇒ Fallback auf die unsortierte Vorwärts-Liste, damit die Face trotzdem erfasst wird.

**Begründung.** Rein topologisch (keine Ebenen-Projektion), robust gegen beliebige Discovery-Reihenfolge und gemischte Kantenorientierung; beim Würfel ergibt jede Branch-Fläche genau einen geschlossenen 4-Kanten-Zyklus (Unit-Test).

### DL-24: Wasserdicht-Kriterium „jede Kante genau zweimal referenziert" (Schritt C.5)

**Problem.** `Shell::is_closed` soll genau dann `true` sein, wenn die Shell eine geschlossene Mannigfaltigkeit ist.

**Optionen.**
1. *Euler-Charakteristik `V − E + F = 2` prüfen.* Notwendig, aber nicht hinreichend (erkennt z. B. nicht-mannigfaltige Konfigurationen schlechter).
2. *Jede Kante muss von genau zwei Face-Loops referenziert werden.* **Gewählt.**

**Gewählte Lösung.** Über alle Faces (Outer + Inner Loops) wird pro `EdgeId` ein Nutzungszähler geführt; `is_closed = uses.all(== 2)` (und Shell nicht leer). Beim Würfel teilt sich jede der 12 Kanten genau zwei der 6 Faces ⇒ wasserdicht.

**Begründung.** Die 2-Mannigfaltigkeits-Bedingung ist das direkte, lokale Kriterium für eine geschlossene Oberfläche und im Test pro Kante überprüfbar — präziser als die globale Euler-Formel allein.

### DL-25: Nicht-analytische / Sketch-Branches → Tessellated-Fallback (Schritt C.5)

**Problem.** Nicht jede Branch hat eine analytische `HalfSpace` (Sphere ist ungesplittet, Torus/Gyroid haben hier keinen Per-Face-Split; Extrude/Revolve sind Sketch-Sweeps ohne Lookup-Eintrag).

**Optionen.**
1. *Solche Faces überspringen.* Verworfen — bräche die Wasserdichtigkeit (Kanten dann nur 1× referenziert).
2. *Face mit `SurfaceGeometry::Tessellated` (zunächst leer) anlegen.* **Gewählt.**

**Gewählte Lösung.** Fehlt ein Lookup-Eintrag, bekommt die Face eine `Tessellated`-Surface (vorerst leere `positions`/`indices`; das Mesh wird in einem späteren Schritt aus dem DC-Pfad eingehängt — vgl. Konzept Phase 4, Punkt 4). Die Topologie (Loop, Edge-Sharing) bleibt vollständig, sodass die Shell-Bilanz konsistent ist. Der Würfel-Pfad ist vollständig analytisch (alle 6 Faces `Analytic { Plane }`).

**Begründung.** Hält das B-Rep topologisch vollständig und wasserdicht, auch wenn die Surface-Geometrie für TPMS/Sketch-Flächen noch nachgereicht wird; kein Sonderfall im Topologie-Code.

### DL-26: NURBS-Endpunkte auf die topologischen Vertices klemmen (Schritt C.5)

**Problem.** Die Edge-Walk-Samples enden ~eine Octree-Zelle *vor* der echten Ecke (die Ecke ist eine Vertex-Zelle, keine Edge-Zelle). Die NURBS-Endkontrollpunkte (= erste/letzte Sample-Position) lägen damit neben dem `start`/`end`-Vertex — das Wireframe wäre nicht exakt geschlossen.

**Optionen.**
1. *Samples roh übernehmen.* Verworfen — Endpunkte ≠ Vertices, Lücke an jeder Ecke.
2. *Erste/letzte Sample-Position vor dem Fitting auf die aufgelöste Vertex-Position setzen.* **Gewählt.**

**Gewählte Lösung.** Vor `hermite_to_nurbs` wird `samples.first().point` auf `vertex(start).position` und `samples.last().point` auf `vertex(end).position` gesetzt (nur falls der jeweilige Vertex existiert; geschlossene Loops ohne Vertex bleiben unberührt). Da die NURBS geklemmt ist, fallen die Endkontrollpunkte exakt auf die Vertices.

**Begründung.** Die Vertices sind die topologisch maßgeblichen, aus dem Branch-Tripel-Zentroid berechneten Positionen; die NURBS muss auf sie interpolieren, damit benachbarte Kanten an gemeinsamen Vertices C⁰-stetig zusammenstoßen und die Shell wasserdicht ist (im Würfel-Test verifiziert: Endkontrollpunkte == Vertex-Positionen).

### DL-27: Lazy `Body.topology`-Cache mit Invalidierung am Remesh (Schritt C.6)

**Problem.** Die WASM-Topologie-API soll echte B-Rep-Zählungen (Vertex/Edge/Face/Shell/Solid) liefern. Das Weben ist teuer (Octree-Build + Edge-Walking + NURBS-Fitting) und darf den heißen Edit-Pfad (`rebuild_mesh_internal` läuft bei *jeder* Mutation) nicht belasten. Gleichzeitig muss eine gecachte Topologie ungültig werden, sobald sich die Geometrie des Bodys ändert.

**Optionen.**
1. *B-Rep eager bei jedem Remesh mitberechnen.* Verworfen — verteuert jede Mutation, obwohl die Topologie nur bei expliziter Abfrage gebraucht wird.
2. *Pro Abfrage neu weben, kein Cache.* Verworfen — wiederholte Abfragen (z. B. Statusleiste) würden jedes Mal neu weben.
3. *Lazy `Option<BRep>`-Feld auf `Body`, befüllt bei erster Abfrage, invalidiert beim Remesh.* **Gewählt.**

**Gewählte Lösung.** `Body` bekommt `topology: Option<BRep>` (Default `None`). `Body::ensure_topology(&mut self, features)` webt bei `None` einmalig und cached. In `rebuild_mesh_internal` wird `topology` exakt dort auf `None` gesetzt, wo ein Body neu gemesht wird: im vollen Remesh-Zweig und im Affine-Fast-Path (der Body bewegt sich in Weltkoordinaten — das gecachte, weltraum-bezogene B-Rep wäre veraltet, auch wenn die Zählungen gleich bleiben). Der Cache-Reuse-Zweig (`payload_hash` + Auflösung identisch) übernimmt `prev` verbatim und behält damit das gewobene B-Rep. Die Abfragemethode braucht `&mut self` (von wasm_bindgen erlaubt).

**Begründung.** Folgt exakt der bestehenden `payload_hash`/`version`-Dirty-Logik der Mesh-Caches: ein verbatim wiederverwendeter Body behält Mesh *und* Topologie, ein neu gemeshter verliert beide. Der heiße Pfad bleibt billig, wiederholte Abfragen sind O(1).

### DL-28: Body-relative Octree-Auflösung für das Topologie-Weben (Schritt C.6)

**Problem.** `Octree::build` nimmt eine absolute `min_cell_size` (Default 0.1 mm). Ein winziger Body (Submillimeter) würde damit gar nicht aufgelöst, ein riesiger (Meter-Skala) liefe in den `max_depth`-Cap und/oder würde extrem grob abgetastet — beides liefert falsche oder leere Topologie.

**Optionen.**
1. *Absolute 0.1 mm beibehalten.* Verworfen — nicht skaleninvariant.
2. *`min_cell_size` aus der Body-AABB ableiten, geklemmt.* **Gewählt.**

**Gewählte Lösung.** `topology_min_cell_size(aabb)` = `(größte_Ausdehnung / 32).clamp(1e-3, 10.0)`. Divisor 32 zielt auf ~32 Zellen über die längste Body-Kante (genug, damit die zwei Flächen einer Kante getrennt registrieren, aber innerhalb des `max_depth=8`-Caps von 256³). Floor `1e-3` mm und Ceil `10.0` mm verhindern, dass eine degenerierte (Null-Größe) oder astronomisch große AABB den Octree in den Tiefen-Cap treibt oder hängt; eine leere AABB fällt auf den Floor zurück.

**Begründung.** Skaleninvariante Abtastung ohne neue Hänge-Risiken. Der Würfel-Test (Kantenlänge 2, `min_cell` = 0.0625, Tiefe ~5) liefert weiterhin exakt 8/12/6.

### DL-29: Panik-Sicherheit beim Weben + Smooth-Body-Limitierung (Schritt C.6)

**Problem.** Die Topologie-Extraktion darf bei *beliebiger* Nutzergeometrie niemals paniken. `hermite_to_nurbs` assertet `samples.len() >= 2`; ein degeneriertes Edge-Point-Cluster könnte prinzipiell mit einem einzelnen Sample durch `build_wireframe` rutschen und die Assertion auslösen.

**Optionen.**
1. *Assertion in `hermite_to_nurbs` entfernen / aufweichen.* Verworfen — die Funktion hat eine echte Vorbedingung, die anderswo Bugs aufdeckt.
2. *Am Aufrufer in `weave` Kanten mit < 2 Samples überspringen (als Hermite belassen).* **Gewählt.**

**Gewählte Lösung.** Die NURBS-Hochstufungsschleife in `weave` überspringt jede Hermite-Kante mit `samples.len() < 2` (`continue`) und lässt sie als harmlose Hermite-Geometrie stehen, statt der Assertion eine unterdimensionierte Kette zu füttern. Verifiziert durch einen Test: ein einzelner Sphere-Body und ein CSG-Body (Box minus Sphere) weben ohne Panik.

**Bekannte Einschränkung.** Feature-basiertes B-Rep erfasst nur *scharfe* Topologie. Glatte Bodies ohne scharfe Kanten (z. B. eine einzelne Kugel) liefern null/nahezu-null Vertices/Edges/Faces — das ist korrekt und dokumentiert, kein Fehler.

### DL-30: Eine JSON-Methode `bodyTopologyCounts` statt mehrerer Getter (Schritt C.6)

**Problem.** Wie soll die B-Rep-Topologie über die WASM-Grenze exponiert werden, ohne den bestehenden `bodyVertexCount` (= *Mesh*-Vertices, load-bearing für den Mesh-Buffer-Transfer in `mesher.worker.ts`/`kernel.ts`) zu verändern?

**Optionen.**
1. *Mehrere numerische Getter (`bodyBrepVertexCount`, …).* Verworfen — jeder Aufruf müsste den Lazy-Cache befüllen (mehrfache `&mut self`-Aufrufe), und das API wächst bei jeder neuen Kennzahl.
2. *Eine Methode `bodyTopologyCounts(&mut self, i) -> String` (JSON).* **Gewählt.**

**Gewählte Lösung.** `bodyTopologyCounts` webt-oder-wiederverwendet `bodies[i].topology` und gibt `{"vertices","edges","faces","shells","solids","watertight"}` als JSON zurück (`watertight` = `is_closed` der äußeren Shell). Ein einziger cache-befüllender Aufruf, beliebig erweiterbar. Der TS-Wrapper `Kernel.bodyTopologyCounts(i)` `JSON.parse`t das. `bodyVertexCount` bleibt unangetastet und semantisch getrennt (Mesh- vs. B-Rep-Vertices).

**Begründung.** Ein JSON-Objekt hält das API erweiterbar und vermeidet mehrfaches Cache-Befüllen; die klare Trennung von `bodyVertexCount` verhindert, dass der für den Mesh-Transfer kritische Pfad versehentlich umgewidmet wird.

### DL-31: Deterministischer FNV-1a-Hash statt `DefaultHasher` für persistente IDs (Schritt D.1)

**Problem.** Stufe-1-IDs (`Edge_ID`, `Vertex_ID`, `Face_ID`) werden von einem Feature *persistiert* (auf Platte, über Maschinen- und Rust-Versionsgrenzen hinweg). Sie müssen byteweise reproduzierbar sein: dieselbe Branch-Signatur muss heute und nach einem Rebuild auf einer anderen Maschine denselben `u64` liefern.

**Optionen.**
1. *`std::collections::hash_map::DefaultHasher` (SipHash).* Verworfen — zufällig geschlüsselt bzw. nicht versionsstabil; das Ergebnis kann sich zwischen Läufen, Maschinen und Rust-Releases ändern. Für persistierte IDs fatal.
2. *Eigener, fester FNV-1a (64-Bit).* **Gewählt.**

**Gewählte Lösung.** `fnv1a_64(bytes)` mit kanonischem Offset-Basis `0xcbf29ce484222325` und Prime `0x100000001b3`. Die Signatur-Worte (`u32`) werden in fester Little-Endian-Bytereihenfolge plus angehängtem `spatial_index` gehasht (`hash_signature`). Ein Regressionstest pinnt `fnv1a_64(b"fielddraft") == 0x3ee262286177cce2` und `fnv1a_64(b"") == OFFSET_BASIS`, sodass ein versehentliches Algorithmus-Drift sofort auffällt.

**Begründung.** FNV-1a ist winzig, allokationsfrei und konstruktionsfest — exakt das, was eine persistente, plattformunabhängige ID braucht. Kollisionsresistenz gegen Angreifer ist hier irrelevant (interne Identität, keine Sicherheitsgrenze).

### DL-32: Spatial Disambiguation — Achsenreihenfolge X→Y→Z mit Arena-Index-Tie-Break (Schritt D.1)

**Problem.** Mehrere Entitäten können dieselbe Branch-Signatur besitzen (z. B. eine Ebene schneidet beide Schenkel eines U-Profils → zwei Edges mit identischem `(Branch_A, Branch_B)`). Sie brauchen unterscheidbare, *stabile* IDs.

**Optionen.**
1. *Reihenfolge nach Arena-Index (Entdeckungsreihenfolge).* Verworfen — nicht stabil: ändert sich, wenn der Weaver Entitäten in anderer Reihenfolge erzeugt.
2. *Centroid-Sortierung entlang einer festen Achsenreihenfolge.* **Gewählt.**

**Gewählte Lösung.** Innerhalb einer kollidierenden Gruppe wird nach 3D-Centroid lexikografisch X→Y→Z sortiert (`cmp_axis_order`, mit Epsilon `1e-9` gegen Float-Jitter auf der führenden Achse), letzter Tie-Break ist der ursprüngliche Arena-Index. Jede Entität erhält einen `spatial_index` (0,1,2,…), der in den Hash gefaltet wird. Entitäten mit eindeutiger Signatur behalten `spatial_index = 0`. Ein einfacher Würfel hat 12 eindeutige Edge-Signaturen und 8 eindeutige Vertex-Mengen → alle Index 0.

**Begründung.** Geometrische (Centroid-)Ordnung ist gegenüber der Weaver-Erzeugungsreihenfolge invariant und damit über Rebuilds stabil; der Arena-Index als finaler Tie-Break garantiert eine totale, deterministische Ordnung selbst bei exakt deckungsgleichen Centroiden.

### DL-33: Fingerprint-Kantenlänge über das Kontrollpolygon (Schritt D.1)

**Problem.** Der geometrische Fingerprint braucht eine „Kantenlänge". Eine Edge kann als Polyline, Hermite-Kette oder NURBS-Kurve vorliegen; eine NURBS-Bogenlänge erfordert numerische Integration.

**Optionen.**
1. *Exakte NURBS-Bogenlänge (Gauß-Quadratur).* Verworfen — Overkill für einen Ähnlichkeits-Score, der nur grobe Verhältnisse braucht.
2. *Sehnenlänge der Punktkette / NURBS-Kontrollpolygon-Länge.* **Gewählt.**

**Gewählte Lösung.** `edge_points` extrahiert für jede Repräsentation eine geordnete Punktkette (Polyline-Punkte, Hermite-Sample-Punkte, NURBS-Kontrollpunkte). `edge_centroid_length` summiert die Sehnenlängen. Für die kubischen Hermite-Fits, die FieldDraft erzeugt, interpoliert das Kontrollpolygon die Endpunkte und folgt der Kurve eng genug für einen Fingerprint. Die Haupttangente am Mittelpunkt kommt aus der zentralen Sehne (`edge_mid_tangent`).

**Begründung.** Der Fingerprint ist ein Heilungs-Hinweis, kein Maß für die Fertigung — eine robuste, allokationsarme Approximation genügt und ist über alle drei Geometrie-Varianten einheitlich.

### DL-34: Confidence-Metrik & Zustands-Schwellen für die Referenzauflösung (Schritt D.1)

**Problem.** `resolve` muss einen Score in `[0,1]` liefern, der (a) einen reinen Parameterwechsel (Branch-Hash identisch, Geometrie verschoben) als `Resolved` einstuft und (b) eine umbenannte/umsortierte Entität ohne Hash-Treffer noch als `Healed` retten kann.

**Optionen.**
1. *Nur Hash-Match (binär).* Verworfen — keine Heilung bei Topology-Split/Reordering möglich.
2. *Nur Fingerprint-Distanz.* Verworfen — ein reiner Resize verschiebt den Centroid, würde unter 0.95 fallen, obwohl die Branch-Identität eindeutig stabil ist.
3. *Gewichtete Mischung Hash-Bonus + Fingerprint-Ähnlichkeit.* **Gewählt.**

**Gewählte Lösung.** `fingerprint_similarity` mischt drei normierte Terme: Centroid-Distanz exponentiell gedämpft mit einer aus der Entitätsgröße abgeleiteten Längenskala (so bleibt der Term bei proportionalem Resize hoch), Größenverhältnis `min/max`, und — nur für Edges — Tangenten-Ausrichtung `|cos θ|` (orientierungs-insensitiv). Gewichte: Edge 0.4/0.4/0.2, Face/Vertex 0.5/0.5. Ein **Hash-Treffer** ist in FieldDrafts Architektur *starke* Evidenz (Branch-IDs stammen aus Feature-UUIDs, nicht aus Geometrie), daher: Score = `0.9 + 0.1·Ähnlichkeit` (geklemmt) — ein uniformer Resize überschreitet bequem 0.95, ein Hash-Treffer mit völlig unähnlicher Geometrie fällt aber auf `Healed` zurück. Ohne Hash-Treffer: reiner Fingerprint-Score über alle gleichartigen Entitäten. Schwellen (aus dem Konzept): `>0.95 Resolved`, `0.7–0.95 Healed`, `0.4–0.7 Ambiguous`, `<0.4 Missing`. Unter der `Missing`-Schwelle wird kein Handle zurückgegeben (Match zu schwach zum Handeln), aber der Score bleibt sichtbar. `resolve_set` löst eine Referenzmenge unabhängig auf → partieller Erfolg (Graceful Degradation) für Mehrkanten-Features.

**Begründung.** Die Mischung respektiert den strukturellen Vorteil der Halbraum-Architektur (Hash dominiert) und liefert trotzdem einen geometrischen Rückfallpfad für die harten TNP-Fälle — exakt die im Konzept beschriebene Zwei-Stufen-Strategie.

### DL-35: `TopoRef` im Kernel, Feature-Einbettung verschoben; D.1-Scope = Naming, nicht Fillets (Schritt D.1)

**Problem.** Das Konzept formuliert die D.1-Abnahme als „ein Fillet auf einer Würfelkante überlebt einen Parameterwechsel". Fillets sind aber laut Konzept (*Bekannte Einschränkungen* #3) ein explizit nicht-trivialer, *out-of-scope* AST-Feature; es existiert noch kein konsumierendes Feature, in das eine `TopoRef` eingebettet werden könnte.

**Optionen.**
1. *Fillet-AST + Feature-Einbettung jetzt mitbauen.* Verworfen — riesiger, separater Scope; würde D.1 überladen.
2. *Naming-/Auflösungs-Infrastruktur im Kernel implementieren, Feature-Einbettung verschieben.* **Gewählt.**

**Gewählte Lösung.** Das neue Modul `crates/kernel/src/brep/naming.rs` enthält die vollständige Stufe-1+2-Infrastruktur (`fnv1a_64`, `edge_id`/`face_id`/`vertex_id`, `Fingerprint`, `TopoRef`, `ResolutionState`, `resolve`, `resolve_set`). `TopoRef` ist serde-serialisierbar, sodass ein Feature es *könnte* persistieren. Die D.1-Abnahme wird auf Kernel-Ebene realisiert: Eine `TopoRef` auf eine Würfelkante (und -fläche) überlebt einen Resize (`half=1.0` → `1.5`) und löst mit Score > 0.95 auf die geometrisch korrespondierende Entität auf. Die Einbettung in ein echtes `Feature`/Fillet bleibt aufgeschoben, bis ein referenzierendes Feature existiert.

**Begründung.** Saubere Trennung: D.1 liefert die durable, vollständig getestete Identitäts- und Auflösungsschicht, ohne sich am noch nicht existierenden Fillet-AST zu verheben. Sobald ein Fillet-Feature gebaut wird, kann es `TopoRef` direkt aufnehmen — die API ist dafür bereits geformt.

### DL-36: Shell als Leaf-Offset-Differenz statt Wurzel-`abs`-Band (Schritt D.2)

**Problem.** Der alte Shell-Operator war `inner.sdf().abs() - thickness`, ausgewertet auf dem Wurzelknoten (`CsgNode::Shell`, `BoundedCsg::Shell`, GPU-Opcode `OP_SHELL`). Auf dem zusammengesetzten CSG-Feld angewandt, *rundet* `abs(sdf)` jede durch `min`/`max` geformte Kante: Das CSG-Feld ist nahe einer Kante keine echte euklidische Distanz, also liegt die `abs`-Bandfläche dort kreisförmig statt scharf. Eine geschalte L-Form verlor an der einspringenden Kante ihre scharfe Innenwand.

**Gewählte Lösung — algebraische Identität.** `Shell(inner, t) ≡ Difference(inner, apply_offset(inner, -t))`: der Körper minus seiner nach innen erodierten Kopie ist eine nach innen gerichtete Hohlwand der Dicke `t` mit *scharfen* Kanten. `apply_offset(d)` ist ein Baum-Walk (modelliert nach `apply_edge_fillets`), der **jede Leaf-Primitive-Dimension** um `d` versetzt, statt das Wurzelfeld zu verschieben. Weil jede Leaf-Primitive rigide wandert (ihr eigenes SDF bleibt eine echte Distanz), bleibt eine durch `min`/`max` zweier Leaves geformte Dieder-Kante scharf. Der Shell wird in `from_features` beim Bau gesenkt; danach wurden `CsgNode::Shell`, `BoundedCsg::Shell` und `OP_SHELL` (Rust-Emitter + TS-Interpreter) **vollständig entfernt** — kein Compat-Shim, kein toter Opcode (CLAUDE.md: keine Backward Compatibility). Der freigewordene Opcode-Wert `0x29` bleibt als bewusste Numerierungslücke ungenutzt; die Konstanten sind explizit. Reuse statt Neubau: Die `Difference`-Maschinerie (SDF, Dual-Eval, Contributor-Tracking, GPU `OP_DIFF`) trägt den Shell ohne neue Infrastruktur.

**Per-Primitive-Offset-Tabelle.** `apply_offset` wächst nur oberflächendefinierende Maße um `d`: `Box` jede Halbachse; `Sphere`/`Cylinder`/`Cone`/`Capsule`/`HexPrism`/`Thread`/`Ellipsoid` die jeweiligen Radien/Halbhöhen; `Torus` nur `minor_radius` (der `major_radius` ist die Schlauch-Mittellinie = Position, keine Oberfläche); `Gyroid` nur `thickness` (`cell_size` ist eine Periode). Positionen/Zentren, `pitch`, `cell_size` und `major_radius` bleiben unverändert. `ImportedMesh` hat kein analytisches Maß und geht unverändert durch (echter Mesh-Offset ist Zukunftsarbeit).

**Erosions-Clamp.** Jede versetzte Dimension wird auf einen kleinen positiven Boden `OFFSET_FLOOR = 1e-4` geklemmt (`(dim + d).max(OFFSET_FLOOR)`). Erosion über null hinaus (Wanddicke > Körpermaß) würde sonst eine negative Dimension und damit ein vorzeichen-invertiertes SDF erzeugen (Innen/Außen vertauscht). Mit dem Clamp degeneriert die erodierte Kopie stattdessen zu einem verschwindend-kleinen, aber gültigen Festkörper → keine Kavität (massiver Körper), nie ein invertierter.

**Inward-only-Semantik.** Der neue Shell ist *einseitig nach innen* (`Difference(inner, erode)`), während das alte `abs(sdf) - t` ein *symmetrisches* Band der halben Breite `t` um die Originalfläche legte (Wand zur Hälfte außen, zur Hälfte innen, Außenmaß wuchs um `t`). Gewählt wurde inward-only, weil es die übliche CAD-Shell-Konvention ist (Außenmaße bleiben erhalten, der Hohlraum entsteht innen) und die Identität sich exakt auf eine `Difference` abbilden lässt.

**Extrude/Revolve-Approximation (bekannte Einschränkung).** Für `Extrude`/`Revolve` wird nur die Tiefe außerhalb der Ebene (`half_height`) versetzt; das 2D-Skizzenprofil in der Ebene bleibt unverändert. Ein korrekter Shell einer Extrusion bräuchte einen echten 2D-Kurven-Offset der Skizze — das ist ein eigenständiges Offset-Problem und Zukunftsarbeit. `FeatureId`s werden auf der erodierten Kopie erhalten (Provenienz überlebt den Offset).

**Geteilte Branch-ID von Innen-/Außenfläche (Notiz, out-of-scope).** Weil `apply_offset` die `FeatureId`s der Leaves unverändert lässt, teilen sich die resultierende Innen- und Außenwand eines Shells dieselbe Branch-ID. Das sauber zu trennen ist eine B-Rep-of-Shells-Frage (eigene Identität pro Wandseite) und liegt außerhalb des D.2-Scopes; hier genügt, dass die Provenienz auf das ursprüngliche Feature zurückführt.

**Test-Diskriminierung.** Eine einzelne `Box` ist innen bereits scharf (Chebyshev-SDF) und kann Leaf- von Wurzel-Shell *nicht* unterscheiden. Der Abnahme-Test `shell_keeps_sharp_edges` nutzt daher eine L-Form (Union zweier Boxen) mit einer durch `min` geformten einspringenden Kante: Am analytischen Eckpunkt der Wand `(10,-10)` liefert der Leaf-Shell ≈ 0 (Punkt liegt scharf auf der Wandfläche), während der gerundete Wurzel-Shell `|inner.sdf|-t = |0|-t = -t` ausgibt (behauptet, der Punkt liege `t` mm tief im Material — der charakteristische Rundungsfehler). Die Differenz von genau `t` an der Kante beweist die Schärfe; ein Kontrollpunkt abseits der Ecke `(9,-9)` zeigt Übereinstimmung beider Formeln.

### DL-37: Offset-Vorzeichenregel für subtrahierte Operanden (Schritt D.2)

**Problem.** Der `apply_offset`-Pass schiebt jedes Leaf-Primitiv um `d` nach außen. Naiv wurde `d` an *beide* Kinder jedes Binäroperators durchgereicht. Für `Difference(A, B) = A − B` (und analog `SmoothDifference`, `MaskedBlend` mit `is_difference`) ist das geometrisch falsch: Wächst der Volumenkörper (`+d`), muss das subtrahierte Werkzeug `B` *schrumpfen*, damit seine Wand nach außen dicker wird; erodiert der Körper (`−d`), muss die Kavität *wachsen*. Mit gleichem Vorzeichen an beiden Kindern schrumpft beim Erodieren auch das Loch — die Wand um ein Loch würde dünner statt dicker.

**Optionen.**
1. *Gleiches Vorzeichen an allen Kindern* (Konzept-Wortlaut „leaf.offset += d" wörtlich). Verworfen: für Subtraktionen falsch; im aktuellen Shell-Pfad (Single-Primitive-/Union-Leaf) latent, aber `apply_offset` ist eine wiederverwendbare öffentliche Funktion.
2. *Vorzeichen am subtrahierten Operanden invertieren.* **Gewählt.**

**Gewählte Lösung.** `Difference(a, b)` → `Difference(a.apply_offset(d), b.apply_offset(-d))`; `SmoothDifference` analog; `MaskedBlend` flippt `b` nur, wenn `is_difference`. Union/Intersection/SmoothUnion/Morph behalten gleiches Vorzeichen. Test `apply_offset_flips_sign_for_subtracted_operand`: `Difference(box10, box3)` um 2 erodiert wächst die Kavität auf `[±5]` (Punkt `(4,0,0)` wird leer) statt auf `[±1]` zu schrumpfen.

**Begründung.** Korrekter Offset eines CSG-Baums: Die Hülle eines subtrahierten Bereichs ist Teil der Körperoberfläche mit umgekehrter Orientierung; ein nach außen gerichteter Körper-Offset bewegt diese Hülle in den subtrahierten Bereich hinein. Die Regel macht `apply_offset` für beliebige Bäume korrekt — Voraussetzung für spätere Wiederverwendung (Offset-Features, korrekte Shells gelochter Körper) — und kostet nichts im häufigen Single-Primitive-Shell-Fall.

### DL-38: Iterative Kanten-/Vertex-Verfeinerung für gewarpte Felder (Schritt D.3)

**Untersuchung (Messung vor jeder Änderung).** Ein um die lokale Y-Achse getwisteter 2×2×2-Würfel (`CsgNode::Twist`, Octree `min_cell = 0.25`) durch die bestehende Pipeline (Phase 1 → Phase 2 Newton-Projektion → Phase 3 One-Shot-Tangentenebenen-Schnitt). Maximaler Oberflächenabstand `|surface_gradient(body, p).0|` der extrahierten Punkte:

| Twist-Rate | One-Shot max|f_Kante| | One-Shot max|f_Vertex| |
|---|---|---|
| 0.0 (planar) | 0.000000 | 0.000004 |
| 0.1 | 0.031675 | 0.016885 |
| 0.2 | 0.021180 | 0.066794 |
| 0.4 | 0.042871 | 0.020598 |

Befund: Topologie blieb unter dem Warp erhalten (in allen Fällen 12 Kanten-Signaturen / 8 Vertex-Branch-Sets), aber die Punkte lagen materiell *neben* der Oberfläche — bis ~0.067, drei Größenordnungen über dem planaren ε und weit über jeder sinnvollen B-Rep-Toleranz (der Würfel-Test fordert `< 1e-3`). Ursache wie vermutet: Die Tangentenebenen werden an Zell-Ecken (in der jeweiligen Voronoi-Region) abgegriffen, also *fern* der wahren gekrümmten Kante; der einmalige lineare 3×3-Schnitt approximiert die gewarpte Kante nur erster Ordnung.

**Entscheidung.** Iterative äußere Verfeinerung **hinzugefügt** (Evidenz oben rechtfertigt sie eindeutig), beschränkt auf `extraction.rs`. `projection.rs`/`mod.rs`/`octree.rs`/`sdf_dual.rs` unverändert.

**Algorithmus.** Nach dem initialen One-Shot-Schätzwert `p`:
1. **Konvergenztest:** Liegt `p` bereits auf allen beitragenden Flächen (`|f(p)| < REFINE_EPS = 1e-4`), Schleife überspringen — der planare Fall konvergiert in 0 Extraschritten und bleibt exakt & schnell.
2. **Near-Edge-Re-Seeding:** Pro beitragender Branch einen Startpunkt *nahe `p`* wählen, der eindeutig in der Voronoi-Region dieser Branch liegt: um `delta = 0.25 · Zelldiagonale` entlang einer In-Tangentialebenen-Richtung versetzen. Bei Kanten ist das `±(t × n_i)` (steht senkrecht zur Kantentangente, weg vom Fault); bei Vertices fehlt eine einzelne Tangente, daher wird eine orthonormale In-Plane-Basis `(u, v)` aus `n_i` gebaut und `±u, ±v` geprobt. Der Seed mit `branch` als (möglichst alleinigem) Contributor und kleinstem `|f|` gewinnt; dann Phase-2-Newton-Projektion → Normale *an der wahren Kante*.
3. **Re-Solve:** Lineares 3×3-System (Kante) bzw. SVD-LSQ (Vertex) erneut lösen, der freie In-Tangenten-Freiheitsgrad jetzt am *aktuellen* `p` verankert (nicht mehr am Zellzentrum), sodass die Lösung entlang der gekrümmten Kante zu `p` wandert.
4. Wiederholen bis Konvergenz oder Iterationsobergrenze.

**Iterationsobergrenze.** `REFINE_MAX_ITER = 8` (Konzept nennt 2–4; 8 ist großzügiger Hang-Guard). Konvergenzkriterium `REFINE_EPS = 1e-4` auf `|f(p)|`.

**Harte Akzeptanz-Schranke (`ACCEPT_EPS = 1e-3`).** Der Tangentenebenen-Schnitt ist eine Erster-Ordnung-Konstruktion; bei starker Krümmung stagniert der Restfehler einzelner Zellen oberhalb `REFINE_EPS`. Statt einen ungenauen Punkt zu emittieren, wird jede Zelle mit Restfehler `≥ ACCEPT_EPS` **verworfen**. Da jede topologische Kante/jeder Vertex von vielen Octree-Zellen überdeckt wird, bleiben die vollständigen Überdeckungen (12 Kanten / 8 Vertices) erhalten, während *jeder zurückgegebene Punkt* garantiert auf der gewarpten Oberfläche liegt. Liegt innerhalb der zellrelativen Phantom-Toleranz (DL-5).

**Akzeptanz-Toleranz im Test.** `ACCEPT_EPS_TEST = 5e-3` — kleiner Schlupf über der `1e-3`-Extraktor-Schranke, der die f32-Rundung der erneuten `surface_gradient`-Auswertung im Assert selbst abfängt. Gewählt, weil der Extraktor bei `< 1e-3` gatet, die Re-Evaluierung in f32 aber zusätzliche ~1e-4-Drift einbringt.

**Ergebnis (nach Verfeinerung + Gate).** Alle Raten 0.1/0.2/0.4 sowie ein starker Twist 0.6 (`min_cell 0.2`) und ein Bend 0.3: 12 Kanten-Signaturen, 8 Vertex-Sets, max|f| jeweils `< 1e-3` (gemessen 0.00098 / 0.00053 / 0.00073 / 0.00051 / 0.00094). Kantentangenten unit. Akzeptanztests `twisted_cube_extracts_correct_brep_edges`, `strongly_twisted_cube_extracts_correct_brep_edges`, `bent_cube_extracts_correct_brep_edges`.

### DL-39: AP214-Schema und Part-21-Boilerplate (Schritt D.4)

**Entscheidung.** STEP-Export schreibt eine **ISO-10303-21 / AP214**-Datei (`FILE_SCHEMA(('AUTOMOTIVE_DESIGN { 1 0 10303 214 1 1 1 1 }'))`). AP214 (Automotive Design) ist der für solide/B-Rep-Geometrie verbreitetste der „advanced_brep"-Application-Protocols und wird von praktisch jedem CAD-Reader (FreeCAD, OpenCascade, SolidWorks, Fusion) akzeptiert; das hier emittierte Entity-Set (`MANIFOLD_SOLID_BREP`, `ADVANCED_FACE`, `B_SPLINE_CURVE_WITH_KNOTS`, …) ist gemeinsamer Nenner von AP203/AP214/AP242, daher schema-portabel. Der HEADER trägt `FILE_DESCRIPTION`, `FILE_NAME` (mit dem `product_name`) und `FILE_SCHEMA`; der DATA-Block die Entity-Instanzen. Mindest-Produkt-Boilerplate: `APPLICATION_CONTEXT` → `APPLICATION_PROTOCOL_DEFINITION`, `PRODUCT(_CONTEXT)` → `PRODUCT_DEFINITION_FORMATION`/`_CONTEXT`/`PRODUCT_DEFINITION` → `PRODUCT_DEFINITION_SHAPE` → `SHAPE_DEFINITION_REPRESENTATION` → `ADVANCED_BREP_SHAPE_REPRESENTATION`, sodass ein Reader von der Produktwurzel bis zum Solid navigieren kann. Geometrischer Kontext: ein komplexer `GEOMETRIC_REPRESENTATION_CONTEXT` mit `(LENGTH_UNIT)`=Millimeter, Radiant- und Steradiant-Einheiten plus `UNCERTAINTY_MEASURE_WITH_UNIT` (1e-6). Implementiert in `crates/kernel/src/brep/step_export.rs`.

### DL-40: Entity-Graph-Mapping B-Rep → STEP advanced_brep

**Entscheidung.** Die index-arena `BRep` wird 1:1 auf den kanonischen advanced-B-Rep-Graphen abgebildet: `ADVANCED_BREP_SHAPE_REPRESENTATION` → je `Solid` ein `MANIFOLD_SOLID_BREP` → dessen `outer` Shell ein `CLOSED_SHELL` (bei `is_closed`) bzw. `OPEN_SHELL` (sonst, dann via `SHELL_BASED_SURFACE_MODEL` statt `MANIFOLD_SOLID_BREP`) → je `Face` ein `ADVANCED_FACE((bounds), surface, .T.)` → `outer` Loop als `FACE_OUTER_BOUND`, jeder `inner` Loop als `FACE_BOUND`, je Loop ein `EDGE_LOOP` aus `ORIENTED_EDGE`s (Orientierung `.T.`/`.F.` aus dem `reversed`-Flag) → je `Edge` genau ein `EDGE_CURVE(VERTEX_POINT_start, VERTEX_POINT_end, curve, .T.)` → `EdgeGeometry::Nurbs` als `B_SPLINE_CURVE_WITH_KNOTS`, `Vertex` als `VERTEX_POINT`. IDs werden über einen monotonen Zähler (`StepWriter`) eager vergeben und sofort emittiert; Vorwärtsreferenzen sind in STEP legal. Plane-Flächen werden zu `PLANE(AXIS2_PLACEMENT_3D(point, axis, ref_dir))`: der lokale Plane-Punkt `normal·offset` (Einheitsnormale ⇒ `(normal·offset)·normal − offset = 0`) wird via `transform` (Scale→Quat-Rotate→Translate) in Weltkoordinaten gebracht, die Normale per Quaternion gedreht und renormiert, die Referenzrichtung als beliebiger orthonormaler Vektor gewählt.

### DL-41: De-Duplikation von CARTESIAN_POINT / DIRECTION / VERTEX_POINT / EDGE_CURVE

**Entscheidung.** Eine geteilte Topologie (eine Kante grenzt an zwei Flächen; ein Vertex an drei Kanten/Flächen) darf nicht mehrfach instanziiert werden. `Geometry` hält vier Maps: Koordinaten-quantisiert (auf 1e-9 gerundet → robust gegen f64-Rauschen) → `CARTESIAN_POINT`-ID; Richtung-quantisiert → `DIRECTION`-ID; `VertexId` → `VERTEX_POINT`-ID; `EdgeId` → `EDGE_CURVE`-ID. Damit referenzieren alle Loops, die eine Kante teilen, dieselbe `EDGE_CURVE`-Instanz, und ein geteilter Eck-Vertex liefert genau ein `VERTEX_POINT`. Für den Würfel ergibt das exakt 8 `VERTEX_POINT` (statt 24 = 6 Flächen × 4) und 12 `EDGE_CURVE` (statt 24). Geschlossene Kanten ohne Start/End-Vertex (`start`/`end` = `None`) bekommen einen synthetischen Vertex am ersten Kontrollpunkt, sodass die `EDGE_CURVE` wohlgeformt bleibt.

### DL-42: Nicht-rationale vs. rationale B-Splines

**Entscheidung.** Sind alle Gewichte einer `NurbsCurve` ≈ 1 (Toleranz 1e-9 — der C.5-Weaver erzeugt ausschließlich Gewicht-1-Splines, DL-22), wird eine schlichte `B_SPLINE_CURVE_WITH_KNOTS(degree, (cps), .UNSPECIFIED., .F., .F., (mults), (knots), .UNSPECIFIED.)` emittiert; kein rationaler Wrapper nötig und korrekt. Weicht ein Gewicht ab, wird die ISO-Komplex-Instanz `( BOUNDED_CURVE() B_SPLINE_CURVE(...) B_SPLINE_CURVE_WITH_KNOTS(...) CURVE() GEOMETRIC_REPRESENTATION_ITEM() RATIONAL_B_SPLINE_CURVE((weights)) REPRESENTATION_ITEM('') )` geschrieben (AND-Subtyp-Kombination). Der geklammerte Knotenvektor wird per `knot_multiplicities` in `(distinct value, multiplicity)`-Paare kollabiert, wie STEP es verlangt. `Polyline`-Kanten (Test-/Degenerat-Fallback) werden als Grad-1-`B_SPLINE_CURVE_WITH_KNOTS` durch ihre Punkte emittiert; `Hermite` darf post-weave nicht auftreten, wird aber defensiv wie eine Polyline behandelt, damit der Export nie paniced. Reals werden via `{:?}` (kürzeste Round-Trip-Form) mit garantiertem Dezimalpunkt formatiert (`1.0`, nicht `1`).

### DL-43: Nicht-planare / tessellierte Flächen — vorerst Best-Effort-Plane

**Entscheidung & Limitation.** Der Cube-Pfad (alle `HalfSpace::Plane`) wird voll korrekt als `PLANE` exportiert; `InfiniteCylinder` wird als `CYLINDRICAL_SURFACE` (Achse lokales +Y, Radius) abgebildet. `Sphere`/`Cone`/`Torus`/`Gyroid` sowie `SurfaceGeometry::Tessellated` (TPMS) haben **noch kein** dediziertes STEP-Surface-Mapping; sie fallen auf eine Best-Effort-`PLANE` durch den Flächen-Schwerpunkt mit Newell-Normale zurück. Damit bleibt die Datei strukturell gültig (alle Referenzen auflösbar, jeder Reader kann sie öffnen), aber die nicht-planaren Trimmflächen sind geometrisch nur approximiert. Sauberes `SPHERICAL_SURFACE`/`CONICAL_SURFACE`/`TOROIDAL_SURFACE` und ein echtes `B_SPLINE_SURFACE` für TPMS sind ein Folge-Schritt. (Frontend-Download-UI ebenfalls Folge-Schritt: WASM-Export `bodyStepExport` + `web/src/kernel.ts`-Wrapper sind vorhanden, der Datei-Download-Button noch nicht verdrahtet.)

### DL-44: Referenz-Auflösungs-Test als Korrektheits-Proxy für „CAD-importierbar"

**Entscheidung.** Da hier kein externer CAD-Viewer läuft, wird die strukturelle Validität rigoros im Rust-Test geprüft. Der wichtigste Test parst den Text, sammelt alle definierten `#N=`-IDs und alle referenzierten `#N`-IDs und assertiert: (1) keine ID doppelt definiert, (2) `referenced ⊆ defined` (keine baumelnden Referenzen). Eine STEP-Datei, deren Entity-Graph vollständig auflösbar ist und das advanced_brep-Schema einhält, ist die belastbarste hier mögliche Stellvertreter-Garantie für „in einem CAD-Viewer importierbar". Ergänzend für den gewobenen Würfel (`box_body([1,1,1])`, Octree `0.25`, `weave`): genau 1 `MANIFOLD_SOLID_BREP`, 1 `CLOSED_SHELL`, 6 `ADVANCED_FACE`, 6 `PLANE`, 8 `VERTEX_POINT` (De-Dup-Beweis), 12 `EDGE_CURVE`, 12 `B_SPLINE_CURVE_WITH_KNOTS`; die 8 Würfelecken (±1,±1,±1) erscheinen als `CARTESIAN_POINT`. Der getwistete Würfel (NURBS-Kanten aus D.3) wird ebenfalls auf vollständige Referenz-Auflösung getestet. Smoke-Test `body_step_export_box_is_valid_advanced_brep` über die WASM-API-Methode `Kernel::body_step_export`.

### DL-45: Szenengesteuerte 2D-Kurven-Shader-Variante (WebGPU-Crash-Fix)

**Problem.** Die in B.3a (Commit `6432d7b`) ergänzten 2D-Kurven-Handler im geteilten SDF-Interpreter-Shader (Cubic-Bézier-Nullstellensuche, Elliptic-Arc-Sampling, Path-Crossing-Loops) blähen das WGSL so stark auf, dass der Shader-Compiler mancher GPU-Treiber bei der Pipeline-Erstellung abstürzt. Chrome meldet das als Device-Lost mit `reason: unknown` bzw. „A valid external Instance reference no longer exists" — kein sauberer Compile-Error, sondern der GPU-Prozess stirbt. Betroffen waren **beide** SDF-Renderer (`sdf-vm` und der Default `sdf-deferred`); der MDC-Mesher und die Vor-B.3a-Version liefen fehlerfrei → klare Regression im SDF-Shader, nicht in der Umgebung.

**Entscheidung.** Der SDF-Interpreter kompiliert die schweren Kurven-Handler nur noch dann, wenn die Szene tatsächlich Kurven-Opcodes enthält. `buildVmSdfEvalFn` bekommt eine Option `include2dCurves` (Default `true` = sicherer Superset; jede nicht verdrahtete Stelle fällt aufs korrekte, schwere Verhalten zurück, nie auf Garbage). Bei `false` fehlen der Kurven-Dispatch-Block **und** die zugehörigen WGSL-Funktionen — der Shader ist so schlank wie vor B.3a. `streamUses2dCurves` (in `sdfPipelineCommon.ts`) scannt den gepackten Opcode-Stream per `Uint32`-View auf die Kurven-Tags `0x1A–0x1E`; weil Opcode-Tags als bitcast-u32 (winzige Denormals) abgelegt sind und echte Float-Parameter nie in diesen Wertebereich fallen, gibt es keine False-Negatives. Beide Renderer wählen die Variante anhand des Scans; der Deferred-Pfad startet bewusst schlank und rekompiliert den Shade-Shader on-demand, sobald (bzw. solange) Kurven auftauchen (`needsShadeRebuild`-Trigger bei Kurven-Wechsel).

**Limitation (offene Baustelle).** Der Fix umgeht das Problem nur für reine 3D-Szenen. Sobald Bézier-/Spline-/Arc-Sketches tatsächlich in der Szene sind, wird der volle Kurven-Shader kompiliert und kann auf schwachen GPUs weiterhin den Compiler sprengen. Die saubere Lösung — die Kurven-Auswertung aus dem monolithischen Interpreter-Shader herauslösen (z. B. Precompute/Bake der Kurven-SDF in ein Volume, oder Aufteilen in mehrere kleinere Pipelines) — ist ein Folge-Schritt.

### DL-46: Lokal-gecachte Per-Body-SDF als Basis, globaler Octree nur in Überlappungszonen (Architektur-Pivot)

**Kontext.** Backtrack aus der Half-Space-only-Architektur (siehe Masthead oben). Zwei harte Lektionen erzwangen den Pivot: (1) Splines/Bézier haben keinen geschlossenen Abstand (Kubik ⇒ Quintic, in Radikalen unlösbar) → Kurven müssen im Kernel in lineare/quadratische Segmente zerlegt werden. (2) Naive Half-Spaces explodieren als SDF — der flache Opcode-VM hat eine Speicher-Decke (512 Opcodes/Body) **und** eine Compute-Decke (ganzer Baum pro Pixel pro March-Schritt). Beides verschiebt sich durch 512→10k nur, statt zu skalieren. Damit ist der Half-Space-only-VM als CAD-Renderer tot.

**Entscheidung.** Die tragende Achse ist nicht „2D vs. 3D", sondern **lokal vs. global + Änderungsfrequenz**. Daraus folgt eine **Hierarchie**:

1. **Lokale Per-Body-Repräsentation (billig, gecacht, seltene Invalidierung).** FieldDraft-Geometrie ist fast vollständig **Sketch-basiert** (Sketch + Extrude/Revolve/Helix/Sweep). Die 2D-Sketch-SDF samt **2D-BVH** (`BoundedSketch2d`) wird **einmal pro Sketch** gebaut und nur invalidiert, wenn sich der **Sketch** ändert — die *seltene* Operation. Die *häufigen* Operationen (Extrude-Höhe ziehen, Body verschieben/rotieren/skalieren, Echtzeit-Dragging) berühren die lokale SDF **nicht**: die globale SDF entsteht durch **Transformation des Query-Punkts in den lokalen Body-Raum**, nicht durch Umrechnen der lokalen Struktur. „Jeder Body hat seinen eigenen 2D-Raum" ist damit ein **Feature**, kein Mangel — affine Platzierung ⇒ keine BVH-Neuberechnung.
   - **Metrik-Feinheit:** Bei starrer Transformation + uniformer Skalierung bleibt die SDF eine echte Distanz (nur Skalenfaktor korrigieren). Bei nicht-uniformer Skalierung/Scherung bleibt das *lokale Feld* unverändert (valides Inside/Outside-Orakel), aber die globale Metrik wird verzerrt → Normale braucht die Inverse-Transponierte der Jacobi-Matrix. **Dieselbe** Dual-Number-Kettenregel, die den 2D-Gradienten durch Extrude/Revolve hebt, trägt auch diese Platzierungs-Jacobi — affine Platzierung ist nur ein weiteres Glied derselben Kette.
   - **Per-Body-B-Rep-Flächen sind lokal** und analytisch/gecacht ableitbar (Extrude-Mantel = ruled surface des Segments; **Deckel** = Ebenen z=0/z=h im Body-Koordinatensystem, trivial; Revolve-Patches = Kegel/Zylinder/Torus). Das ist der „direkte parametrische" Pfad — durch diese Entscheidung **zentral**, nicht „spätere Option".

2. **Globaler Schritt (teuer, lokalisiert) — Octree als Interaktions-Löser, nicht Allzweck-Extraktor.** Der Octree/Boolean ist nicht mehr monolithisch über die ganze Szene, sondern läuft **nur in Body-Überlappungszonen** (der Deferred-Renderer kennt sie schon per-Pixel, ≤2 Bodies) und **konsumiert** die billigen lokalen Per-Body-Flächen. Genau hier sind die genuin cross-Body-Phänomene: **Trim-Kurven** scharfer Booleans (3D-Raumkurven ohne Urbild in einer einzelnen Sketch-Ebene) und **smin-Blend-Regionen** (neue Fläche, die von beiden Feldern gleichzeitig in einer Umgebung abhängt — keine getrimmte Vereinigung lokaler Flächen). Das ist „global teuer, profitiert massiv von lokal beschleunigt".

**Zweck (breiter als nur Booleans).** Die lokale Beschleunigung dient nicht nur dem Boolean, sondern Rendering, Meshing, Raum-Transformationen, Gyroids und B-Rep gleichermaßen.

**Konsequenz für die Brücke.** Segment-ID + Duale Zahlen sind weiterhin der Mechanismus, der die Sketch-SDF mit der Octree-Pipeline verbindet — sie tragen primär den **lokalen** Pfad (analytisches Lifting 2D-Entity→3D-Entity, z. B. Revolve: 2D-Punkt → 3D-Kreis, auf der Achse → Punkt; 2D-Kante → 3D-Flächen-Patch). Zwei Codepfade (lokal-analytisch + global-Octree) sind **gewollt**: sie entsprechen exakt dem Lokal/Global-Split, den die Änderungsfrequenz erzwingt.

**Verworfen.** Ein separater, rein-2D-B-Rep im Sketch-Space (ohne den globalen 3D-Schritt) — er kann Deckel/Z-Struktur, Inter-Body-Trims und smin-Blends nicht ausdrücken; die zusammengesetzte Festkörper-Topologie ist unausweichlich 3D.

### DL-47: `BranchId(u32)` als Contributor-ID, `feature_collapsed`-Grenze Mesh↔B-Rep (Phase 0 / SA-0a)

**Kontext.** Phase 0 portiert das B-Rep-Substrat von `agents/fielddraft-a` auf `feat/sketch-sdf-brep` (von `main`). `main` und der agents-Branch teilen einen Ahnen, der `Contributors`/`EvalMode`/`combine_min·max`/`BoundedCsg::evaluate` bereits hatte, divergierten dann aber: `main` nutzt `pub type NodeId = FeatureId` (String-artig, nicht `Copy`, **kein Sub-Index**), der agents-Branch `BranchId(u32)` mit Sub-Index-Packing + `expand_evaluate_leaf` (Box/Cylinder/Cone werden in ihre Halbflächen mit Sub-IDs expandiert).

**Entscheidung 1 — `BranchId(u32)` statt `NodeId = FeatureId`.** `NodeId` ersatzlos entfernt. `BranchId(pub u32)` packt `feature_id << SUB_BITS | (sub & mask)`. **`SUB_BITS = 12`** (statt agents' 4): 4096 Sub-IDs pro Feature — nötig, weil Sketch-Segmente (Kreis-Approx ≥24, beliebige Polygone) die 16 von 4 Bit sofort sprengen würden; bleiben 20 Bit ≈ 1 M Feature-IDs. `Contributors` hält jetzt `[BranchId; 8]` und ist wieder `Copy` (kein `.clone()`-Zwang mehr). Grund: nachgelagerte Phasen brauchen einen Sub-Index pro Primitiv-Fläche bzw. pro Sketch-Segment, den ein String-`FeatureId` nicht tragen kann; die zu portierenden `brep/`-Module sprechen ohnehin `BranchId`.

**Entscheidung 2 — `Contributors::feature_collapsed()` zieht die Grenze Mesh-Proxy ↔ analytischer B-Rep.** Die Per-Fläche-Zerlegung (`expand_evaluate_leaf`) würde die Mesh-Semantik verändern: der Mittel-Vertex einer einzelnen Box sähe 3 koinzidente Flächen → `Vertex` statt `Face`. Daher `feature_collapsed()` (Sub-Index strippen, nach Feature dedupen), angewandt in `mdc.rs`/`rfta/mod.rs` **vor** der Per-Vertex-Klassifikation und vor dem flachen `u32`-Contributor-Buffer (der wieder `feature_id()` speichert). Netto: DC/MDC/RFTA/Topology verhalten sich exakt wie auf `main`; die feineren Per-Fläche-`BranchId`s leben nur in `evaluate`/`expand_evaluate_leaf` für den B-Rep-Weaver. **Das ist die Trennlinie:** der Mesh-Pfad bleibt feature-granular, der analytische B-Rep-Pfad wird fläche-granular. SA-3 (lokaler Per-Body-B-Rep) ist der Ort, an dem die feinere Granularität auf der Geometrie-Seite ankommt.

**Entscheidung 3 — `HalfSpace` wandert ins `protocol`-Crate.** Der analytische Flächentyp (Plane/InfiniteCylinder/Sphere/Cone/Torus/Gyroid), den der Weaver an Faces hängt und der serde-round-trippt, gehört in das geteilte Crate (wie auf agents), nicht in den Kernel.

**Konsequenz (Breaking Change, bewusst) — Picking-JSON.** `pick()` in `lib.rs` emittiert jetzt gepackte `BranchId`-u32 (`feature << 12 | sub`) statt blanker Feature-IDs. Per-Fläche-Präzision ist für B-Rep-Flächenselektion erwünscht. **Frontend-Hinweis (offen, schlägt erst beim WASM-Rebuild durch):** Wer im JS per Feature-ID hervorhebt, muss `>> 12` entpacken. Track A ist Rust-only, WASM noch nicht neu gebaut ⇒ latent.

**Bewahrt von `main`:** Gyroid-`target_ids`-Partitionierung, nicht-uniforme `scale`/`min_scale()`, keep-bodies-separate, `CsgNode::Shell` (D.2-`apply_offset` *nicht* portiert — späterer Schritt). **Gate grün:** `cargo test --workspace` (Kernel 368/0), neuer Substrat-Test `expand_evaluate_leaf_box_face_and_edge_subfaces`, wasm32-Build ok.

### DL-48: Zwei „Topology"-Systeme nebeneinander; Feature-B-Rep als `Body.brep` (Phase 0 / SA-0b)

**Kontext.** SA-0b portierte die additiven B-Rep-Module (`sdf_dual.rs`, `brep/{mod,octree,extraction,edge_walker,weaving,naming,projection,step_export}.rs`) auf das SA-0a-Substrat. Dabei kollidierte der Name `Body::ensure_topology`/`Body::topology`: auf `main` ist das bereits belegt durch eine **mesh-abgeleitete** `topology::BodyTopology`, die live ans Frontend verdrahtet ist (`bodyTopologyJson`/`bodyFaceCount` → `mesher.worker.ts` + `viewportOverlays.tsx`).

**Entscheidung.** Die beiden Systeme **koexistieren** vorerst getrennt, statt eines zu ersetzen:
- **Mesh-abgeleitete `topology::BodyTopology`** (`Body.topology`) — bleibt unverändert, versorgt die Frontend-Overlays.
- **Feature-basierter `brep::BRep`** (neues Feld `Body.brep: Option<BRep>`, `Body::ensure_brep(features)`, body-relative `topology_min_cell_size` ≈ 32 Zellen über die größte AABB-Ausdehnung, geclamped `[1e-3, 10] mm`). Cache wird bei Body-Neubau/Remesh auf `None` invalidiert. Neue wasm-Exports: `bodyBrepCounts`, `bodyStepExport` (der agents-Name `bodyTopologyCounts` wurde **nicht** wiederverwendet, weil dieser JS-Name hier die Mesh-Topology meint).

**Offen (Folge-Entscheidung):** Ob die Mesh-Topology-Overlay langfristig zugunsten des Feature-B-Rep entfällt — bewusst NICHT unter „keine Backward Compatibility" angenommen, weil das Frontend sie aktiv konsumiert und es außerhalb des SA-0b-Scopes lag.

**Weitere Detail-Entscheidungen.** (1) `CsgNode::Shell` musste in zwei erschöpfende `match`-Bäume eingefügt werden, die agents nicht kannte: `sdf_dual.rs::eval_bounded_d3` (duales `abs(inner)-thickness` als manueller Vorzeichen-Flip auf `.re`, um die Ableitung zu erhalten — `ComplexField::abs` ist auf dem konkreten `DualSVec64<3>` nicht in Scope) und `weaving.rs::collect_halfspaces` (Shell in den `Twist|Bend|Displace`-Recurse-Arm gefaltet). (2) `bvh::distance_point_to_aabb` → `pub(crate)` (von `sdf_dual.rs` benötigt). (3) `num_dual` 0.11 passte ohne Anpassung.

**Hinweis für SA-1 (Sketch-Segment-IDs).** Der Weaver dekomponiert analytische Flächen über `weaving.rs::collect_halfspaces` → `branch_halfspace_map`, gekeyt per `BranchId::from_feature_sub(id, f.sub)` aus `csg::primitive_halfspaces`. **Nur Box/Cylinder/Cone liefern heute Half-Spaces; `Extrude`/`Revolve` liefern KEINEN analytischen Eintrag** → ihre Flächen fallen im Weaver auf tesselliierte Geometrie zurück, und sie emittieren nur einen einzelnen `from_feature(id)`-Contributor (kein Sub-Index). SA-1 muss daher: (a) per-Segment-`from_feature_sub(id, seg)`-Contributors aus der Sketch-SDF liefern (`sketch.sdf_with_edge`/`edge_count`-Analog) und (b) einen `Extrude|Revolve`-Arm in `collect_halfspaces` (bzw. eine Per-Segment-Flächen-Map) ergänzen. Extraction/Edge-Walker partitionieren bereits rein nach der *Menge* der an einem Punkt kollidierenden `BranchId`s — sobald Contributors Per-Segment-Sub-IDs tragen, splittet der bestehende Weave Segmentgrenzen ohne strukturelle Änderung in separate B-Rep-Kanten. Außerdem ist in `sdf_dual.rs` ein `#[cfg(any())]`-Block (`mod sketch_segment_duals_sa1` + `sketch_segment_distance_generic`) verbatim für SA-1 hinterlegt (depend. auf SA-1s `SketchSegment`-Typ); SA-1 entfernt die `#[cfg(any())]`-Gates beim Aktivieren.

### DL-49: `SketchSegment {Line, QuadBezier, Arc}`, G1-Gruppen als Kanten-Identität, analytischer Quad-Abstand (Phase 1 / SA-1)

**Kontext.** SA-1 gab der 2D-Sketch-SDF Per-Segment-Identität und führte native analytische Quadratik-Bézier-Segmente ein (Entscheidung #1, freigegeben). Portiert von `agents/fielddraft-a` (`Sketch2dPrimitive::Path { segments, closed }`, `sdf_with_edge`/`edge_count`-Threading durch den BVH-CSG-Baum), aber mit drei bewussten Abweichungen.

**Entscheidung 1 — `SketchSegment = { Line, QuadBezier, Arc }` als Kernel-Primitive.** Der agents-Branch hatte `{Line, EllipticArc, QuadraticBezier, CubicBezier}`. SA-1 verwirft `EllipticArc` (war Newton+Sampling, *nicht* exakt) zugunsten eines **zirkulären `Arc`** (center/radius/start/sweep) mit exaktem analytischem Closest-Point (radiale Projektion, sonst näherer Endpunkt). Damit gilt durchweg „Line/Arc exakt, QuadBezier analytisch".

**Nachtrag (Supervisor-Entscheidung, Option B): Ellipsen & Splines via Zerlegung.** Ellipsen sind CAD-Standard und sollen als First-Class-Sketch-Primitive existieren — aber *müssen nicht exakt* sein. Statt eines iterativen Ellipsen-Solvers (Option A: Quartic/Newton + neue Dual-Implementierung) werden Ellipse/Elliptic-Arc **bei der Konstruktion in QuadBezier-Segmente zerlegt** (wie kubische Splines, Entscheidung 2). Dasselbe Verfahren deckt Splines/CubicBezier ab. Vorteile: **null** neuer Distanz-/Dual-/GPU-Code (reitet auf der verifizierten QuadBezier-Maschinerie), Genauigkeit ist ein Tessellierungs-Parameter, B-Rep-Identität bleibt sauber (G1-Gruppierung → *eine* Ellipsen-Kante). Ehrlicher Haken: extrudierte Ellipsenwand wird im STEP-Export eine B-Spline-Approximation, keine analytisch „echte" elliptische Fläche — für CAD-Interop normal, exakte elliptische STEP-Flächen bei Bedarf später als Upgrade (Option A, gut isolierbar, da die UI/API-Form schon steht). Die Zerlegung gehört konstruktionsseitig (`protocol`/`sketch_features`), nicht in die Dual-Kette — SA-2 bleibt davon unberührt.

*Implementiert (2026-05-31):* `ellipse_quad_beziers(center, rx, ry, rotation, start_angle, sweep_angle, segments, tolerance_mm)` in `protocol/sketch2d.rs`. Kerntrick: eine Ellipse ist das **affine Bild** `M·u + center` des Einheitskreises (`M = R(rotation)·diag(rx,ry)`). Affine Abbildungen bilden Tangenten auf Tangenten ab und erhalten ihren Schnittpunkt, also ist der QuadBezier-Kontrollpunkt jedes Teilbogens schlicht das affine Bild des Einheitskreis-Tangenten-Schnittpunkts `X = [s1−s0, c0−c1]/sin(a1−a0)` — exakt, ohne Sampling, und G1-stetig an jeder Naht (`g1_group_segments` ⇒ *eine* Kante). `segments == 0` ⇒ automatische Anzahl aus der Sagitta-Schranke ε ≈ R·Δ⁴/128 (Default-Toleranz 10 µm). **Naht-Snap:** bei vollem Sweep wird `last.p2 := first.p0` exakt gesetzt — `sin(2π)` in f32 ist ein paar ×1e-7, nicht null, sonst zählt ein Even-Odd-Strahl entlang der Naht (z. B. Hauptachse einer achsparallelen Ellipse) die Kreuzung doppelt und kippt das Innen/Außen-Vorzeichen. Tool: `sketch_ellipse` (`center`, `radius_x`, `radius_y`, `rotation`=0, `segments`=0=auto) → geschlossener `Path`. Elliptic-Arcs (offener `Path`) sind über die `start_angle`/`sweep_angle`-Parameter des Helpers bereits abgedeckt; ein eigenes Arc-Tool/UI folgt bei Bedarf.

**Entscheidung 2 — kein `CubicBezier`.** Kubische Béziers haben keinen geschlossenen Abstand (Quintic). Sie müssen *upstream bei der Konstruktion* in Quadratiken unterteilt werden. Heute existiert keine Cubic-Quelle (`sketch_features.rs` erzeugt nur Circle/Rect/Sector/Polygon), also gibt es noch keine Unterteilungs-Stelle; der Vertrag steht am Enum dokumentiert.

**Entscheidung 3 — G1-Kontinuität definiert die Kanten-Identität (nicht das einzelne Segment).** `g1_group_segments` (Schwelle `PATH_G1_COS_THRESHOLD = 0.999`, ≈2,5°) kollabiert aufeinanderfolgende tangentenstetige Segmente in **eine** Branch-/Kanten-ID; nur Tangenten-Sprünge (Ecken) splitten. `edge_count()` = Anzahl G1-Gruppen, `sdf_with_edge` liefert den **Gruppen**-Index. Folge: ein glatter Spline = **eine** B-Rep-Kante, eine Polygon-Ecke = Split. Das ist die CAD-korrekte Semantik und der Schlüssel, damit SA-3 die Extrude-Wand pro Segment-*Gruppe* (nicht pro Roh-Segment) in Flächen splittet.

**Quad-Solver.** `closest_on_quad_bezier` (`protocol/sketch2d.rs`): Minimierung von `|P−B(t)|²` für `B(t)=a+Lt+Mt²` ergibt die kubische `2(M·M)t³+3(L·M)t²+(L·L−2W·M)t−W·L=0`, geschlossen gelöst per `solve_cubic_real` (Cardano, f64), dann nächster Kandidat unter Innen-Roots (auf (0,1) geclamped) + zwei Endpunkten. Kein Sampling. Gate-Test vs. 10k-Sample-Referenz < 1e-3 (Schranke ist der Referenz-Diskretisierungsfehler, der Solver ist f32-exakt).

**GPU bleibt kurvenfrei (DL-45/46).** `Path` lowert in `sdf_compiler.rs` via `flatten_path_to_polyline` auf `OP_PRIM2D_POLY` — KEIN neuer Kurven-Opcode, CPU-Mirror unverändert. Analytische Segmente nur für CPU-SDF + B-Rep; der GPU-Renderer sieht das geflattenete Polygon.

**3D-Contributors.** `CsgNode`/`BoundedCsg::Extrude·Revolve::evaluate` rufen `sdf_with_edge` und falten den Gruppen-Index in `from_feature_sub(id, group)`. **Group 0 ≡ `from_feature(id)`** by construction → bestehende Single-Contributor-Tests für Circle/Rectangle (Gruppe 0) bleiben unverändert grün.

**Tie-Break (DL-32).** Deterministisch: gleichweit → niedrigerer Segment-Index; im Bool-Baum → linkes Kind; Rechteck-Achsensplit → X zuerst.

**Hinweis SA-2.** Der `#[cfg(any())]`-Block in `sdf_dual.rs` referenziert noch die ALTEN Variantennamen (`QuadraticBezier`/`CubicBezier`/`EllipticArc`). Beim Aktivieren auf `QuadBezier`/`Arc` umbenennen und den `CubicBezier`-Test streichen. `SketchSegment::closest_point(&self,[f32;2])->[f32;2]` ist analytisch für Quad und bereit. **Gate grün:** Kernel 436, Protocol 52, wasm32-Build ok.

### DL-50: Analytische Extrude/Revolve-Flächen + platzierungs-bewusster B-Rep-Cache (Phase 2 / SA-3)

**Kontext.** SA-1 gab der Extrude/Revolve-SDF Per-Segment-Identität (DL-49), aber `weaving.rs::collect_halfspaces` hatte für `BoundedCsg::Extrude·Revolve` einen leeren Arm — die Wände/Caps fielen auf `SurfaceGeometry::Tessellated` zurück. SA-3 schließt diese Lücke: jede analytisch darstellbare Sketch-Boundary-Gruppe wird in eine 3D-`HalfSpace` gehoben, ausgerichtet auf exakt die BranchIds, die der Octree sah.

**Entscheidung 1 — Cap-Split in der Contributor-Markierung.** Die Extrude-SDF ist `max(d2, |z|−hh)`; der dominierende Term benennt die nächste Fläche. Bisher trug der f32-Pfad *jeden* Punkt — auch Cap-Punkte — mit der nächsten *Wand*-Gruppe, sodass Top/Bottom-Cap keine eigenständigen Flächen waren. Neu: `extrude_contributor_sub(d2, z, hh, group)` (csg.rs) vergibt bei dominierendem Cap-Term reservierte Sub-IDs `BranchId::EXTRUDE_CAP_TOP_SUB = 0xFFF` / `EXTRUDE_CAP_BOTTOM_SUB = 0xFFE` (Top bei `z ≥ 0`). Wand-Gruppen sind immer klein (`< edge_count ≪ 0xFFE`), also kollisionsfrei. Beide f32-Eval-Pfade (`CsgNode::Extrude` in csg.rs, `BoundedCsg::Extrude` in bvh.rs) teilen denselben Helper; `* world_transform.min_scale()` bleibt unverändert. **Der Dual-Pfad (sdf_dual.rs) bleibt unberührt** — Gradienten sind vom Tagging unabhängig. Revolve hat keine Caps (jede Boundary-Gruppe ist ein revolviertes Profilsegment), daher dort kein Split.

**Entscheidung 2 — analytische Flächen pro Boundary-Gruppe.** `collect_sketch_walls` (weaving.rs) läuft den `BoundedSketch2d`-Baum mit *demselben* Gruppen-Index-Offset wie `sdf_with_edge` (rechtes Kind um `left.edge_count()` versetzt) und liefert pro Gruppe `(group_sub, SketchWall)`. **Extrude:** Caps → `Plane{[0,0,±1], offset=hh}` (Vorzeichen wie Box-Top/Bottom, `p·n−offset`); gerade Wand (Rechteck-Seite, Polygon-Kante) → 2D-Außennormale auf `[nx,ny,0]` gehoben; Circle (Gruppe 0) → `InfiniteCylinder{radius}` (2D-Center als lokale Vor-Translation gefaltet, DL-20). **Revolve** (Profil in `(r,y)`, Achse = y): konstantes `r` (vertikale Linie) → `InfiniteCylinder{radius}`; konstantes `y` (horizontale Linie) → `Plane{[0,±1,0]}` (y-Ebene als Translation); schräge Linie → `Cone{half_angle=atan(|dr|/|dy|)}` mit Apex-Platzierung `y = offset/ny` via Transform. **Außennormale = Richtung steigender SDF**, exakt wie Box-Flächen; Polygon-Winding (Signed-Area) bestimmt das Vorzeichen, der Even-Odd-Sign macht beide Wicklungen tragbar.

**Dokumentierte Grenze.** Gekrümmte Gruppen werden NICHT analytisch gehoben und fallen korrekt auf `Tessellated` zurück: Extrude-`QuadBezier`/`Arc`-Path-Gruppen + `Sector` (Sektor mischt Radiallinien + Bogen in *eine* Gruppe 0); Revolve-`Circle` (würde Torus-Platzierung brauchen) und alle gekrümmten Profile. Auch `Sketch2d::Offset` wird übersprungen (Offset verschiebt die Boundary; korrekte Re-Ableitung pro Primitivtyp ist brüchig) — bewusst konservativ. Reine Geraden-`Path`-Gruppen sind theoretisch hebbar, aber die G1-Gruppierung kann kollineare Segmente mergen, daher ebenfalls übersprungen.

**Entscheidung 3 — B-Rep-Cache auf vollen `payload_hash` geschlüsselt, NICHT platzierungsinvariant.** Der Task fragte nach einem Cache, der reine Platzierungsänderungen (Move/Transform) übersteht. **Befund (Design-Frage, bewusst NICHT „platzierungsinvariant" implementiert):** Die gewobenen Faces backen die Welt-Pose *zweifach* — sowohl die Octree-Vertex-Positionen (der Octree sampelt den welt-platzierten `BoundedCsg`) als auch die analytischen Face-`Transform`s (`= world_transform`, das die Feature-Transform via `BoundedCsg::from_csg` enthält). Ein reiner Cache-Reuse nach einem Move würde die Geometrie in der *alten* Pose rendern. Daher ist eine platzierungsinvariante Wiederverwendung mit der heutigen Architektur **falsch** — sie bräuchte entweder einen lokal-frame-gewobenen B-Rep + separater Welt-Transform-Schicht beim Konsum (STEP-Export/Counts), oder ein Re-Stamping der Vertex-Positionen. Stattdessen: `Body.brep: Option<(u64, BRep)>` geschlüsselt auf `hash_body_payload` (Geometrie **und** Platzierung). `ensure_brep` webt nur neu, wenn der Key abweicht — *selbstheilend*: korrigiert nebenbei einen latenten Bug, bei dem der Affine-Fast-Path (`detect_uniform_translation`) in `rebuild_bodies` `brep = None` umging und so einen stale-Pose-B-Rep behielt. Ein echter platzierungsinvarianter Cache bleibt ein sauber isolierbares Upgrade (lokaler Weave + Transform beim Export), falls B-Rep-Recompute je heiß wird.

**Gate grün (SA-3).** Kernel-Lib 449 passed / 0 failed, Protocol 58 passed / 0 failed, clippy fehlerfrei. Neue Gate-Tests in weaving.rs: Rechteck-Extrude → 8 V / 12 E / 6 F, alle analytische Planes, watertight; L-Polygon-Extrude (6 Ecken) → watertight, 8 Flächen (6 Wände + 2 Caps), alle Planes; Revolve eines achsversetzten Rechteck-Profils → watertight, Seiten 2× `InfiniteCylinder` + 2× `Plane` (kein `Tessellated`). **Befund zu V/E/F:** L-Polygon meldet V=10/E=110, Revolve V=0/E=4672 — die Topologie ist korrekt watertight (jede Kante genau 2×, `validate()==Ok`, Shell `is_closed`) und die Flächenzahl stimmt; die hohen Kantenzahlen sind ein Artefakt des generischen Octree-Edge-Walkers (viele kleine Kanten-Segmente statt einer gemergten topologischen Kante), kein Fehler der analytischen Zerlegung. Revolve V=0 ist korrekt: ein revolvierter Quader hat keine scharfen Eck-*Punkte*, seine 4 Face-Trennkanten sind volle Kreise ohne Vertex. Eine Kanten-Konsolidierung (G1-Merge auf B-Rep-Ebene) ist ein separater Schritt, kein SA-3-Scope.

### DL-51: Per-Signatur-Globalfit (Linie/Kreis) konsolidiert fragmentierte Kanten (Phase 3.5 / SA-3b)

**Kontext.** DL-50 stellte fest, dass der generische Octree→Wireframe-Edge-Walker Nicht-Box-Körper *über-segmentiert*: L-Polygon-Extrude meldete 18 korrekte Kanten-*Signaturen*, aber 110 Kanten bei res 0.10 (und *mehr* bei feinerer Auflösung), Revolve V=0/E=4672. **Ursache (vom Supervisor diagnostiziert, hier verifiziert):** Die Signatur-Klassifikation ist korrekt — innerhalb *einer* Signatur-Gruppe bilden die Edge-Punkte aber zerklüftete (lückige) Spatial-Runs statt eines sauberen 1D-Laufs. Die Box ist sauber, weil `BoundedCsg::Leaf` über `expand_evaluate_leaf` verlustfrei pro Half-Space klassifiziert; Extrude/Revolve nutzen `sketch.sdf_with_edge` → `sdf_polygon_with_edge`, dessen Sub-Cell-Contributor-Klassifikation entlang der Kante Löcher/Dithering hat. `chain_group` (Greedy-NN, Split bei Lücke > `spacing·CHAIN_GAP_FACTOR`) zerlegt darum jede *eine* topologische Kante in viele Ketten; feinere Auflösung → feinere Löcher → mehr Fragmente.

**Entscheidung — globaler Primitiv-Fit pro Signatur-Gruppe (`edge_walker.rs::build_wireframe`).** Für jede Gruppe deduplizierter Edge-Punkte wird *vor* dem Greedy-Chaining versucht: (1) **Linien-Fit** (Total-Least-Squares: Centroid + dominanter Eigenvektor der Kovarianz via `nalgebra::SymmetricEigen`). Max-Perp-Residuum ≤ `spacing·FIT_RESIDUAL_FACTOR` (=1.5) ⇒ *eine* gerade Kante: Punkte nach Projektion auf die Linienrichtung sortiert, *eine* orientierte Hermite-Kette (`orient_chain`), Vertex-Snapping unverändert. (2) Sonst **Kreis-Fit** (PCA-Best-Fit-Ebene aus den zwei dominanten Eigenvektoren, dann algebraischer Kåsa-Fit in der Ebene; Residuum = max |dist−r| plus Planaritäts-Check). Erfolg ⇒ *eine* geschlossene Schleife (`start=end=None`, sortiert nach Winkel um den Zentrum). (3) Sonst (weder Linie noch Kreis, z. B. zwei disjunkte parallele Kanten mit geteilter Signatur, DL-11) → **unveränderter `chain_group`-Fallback** (Gap-Splitting bleibt für den echten Mehr-Kanten-Fall). Die Box ist unberührt: ihr einzelner sauberer Lauf fittet eine Linie → eine Kante → weiterhin 12.

**Nebenfix — Dedup-`eps` Span-relativ statt NN-statistisch.** `dedup_points` zog das Merge-`eps` bisher aus der *Median*-NN-Distanz. Eine gerade Extrude-Wandkante stapelt aber viele Octree-Zellen entlang der Sweep-Achse, die teils auf den *exakt* selben Analytik-Punkt projizieren (und teils auf zwei nur durch Float-Rauschen ~1e-8 getrennte) — der Median kollabiert dann auf 0 (>50% Null-Distanzen), das Min auf das Rauschen. Beide sind als `eps` unbrauchbar und führten dazu, dass `build_wireframe` solche Gruppen via `spacing<=0`→`continue` *stillschweigend verwarf* (zwei vertikale L-Kanten gingen verloren). Neu: `eps = group_span · 1e-3` (Bounding-Diagonale), robust gegen die Duplikat-Anzahl — weit unter jedem echten Sample-Schritt (`span/Zellen_pro_Kante`), weit über Float-Rauschen.

**Ergebnis V/E/F (res wie Gate-Tests).** Box 8 V / 12 E / 6 F (unverändert). Rechteck-Extrude 8 V / 12 E / 6 F. **L-Polygon-Extrude 10 V / 18 E / 8 F** (von 110 E) — exakt die 18 topologischen Kanten (6 vertikal + 6 oben + 6 unten), watertight, `validate()==Ok`, alle 8 Flächen analytische Planes. **Revolve-Tube 0 V / 4 E / 4 F** (von 4672 E) — die 4 Face-Trennkanten als geschlossene Kreise (`start=end=None`), 2 Cylinder + 2 Plane, watertight.

**Dokumentierte Grenze — Reflex-Ecke (Extraktions-Limit, NICHT Edge-Walker).** Das L-Polygon erreicht 10 statt 12 Vertices: an der einzigen *konkaven* Ecke (1,1) klassifiziert Phase 3 (`extraction.rs`) bei dieser Auflösung **keine** `Vertex`-Zelle (die Drei-Branch-Kollision wird dort nicht detektiert; `raw_vertex_cells near (1,1) = 0`), also fehlen die Top-/Bottom-Vertices der Reflex-Kante. Alle 18 Kanten existieren trotzdem und die Shell schließt (`order_loop` fällt für die zwei Reflex-Flächen auf die ungeordnete Kantenliste zurück). Dies ist ein Phase-3-Extraktions-Limit (außerhalb des SA-3b-Scopes: Signatur/Klassifikation unverändert) — bewusst als erreichte Zahl `assert`et + dokumentiert, statt die Assertion stillschweigend abzuschwächen.

**Gate grün (SA-3b).** Kernel-Lib 453 passed / 0 failed, Protocol 58 passed / 0 failed, clippy fehlerfrei (Vorbestands-Warnings ok). Neue Unit-Tests in edge_walker.rs: `fit_line_consolidates_fragmented_collinear_points`, `fit_line_rejects_two_parallel_edges` (Fallback-Pfad), `fit_circle_consolidates_co_circular_points`, `fit_circle_rejects_a_straight_line`. Bestehende Gates grün: `cube_weaves_watertight_brep`, `cube_yields_12_edge_signatures…`, `cube_wireframe_has_12_edges_8_vertices`, `difference_body_wireframe_on_surface`, `chain_group_splits_on_large_gap`.

### DL-52: Within-Body-Booleans — robuster Kreis-Fit + Phantom-Signatur-Filter (Phase 3.5 / SA-4a)

**Kontext.** DL-51 konsolidierte fragmentierte Kanten für Sketch-Sweeps. Die echten Within-Body-Booleans sind aber `Difference`/`Intersect`, die in eine Basis schneiden (zwei `Union` sind immer getrennte Bodies). Empirie auf dem Octree→Edge-Walker→Weave-Pfad: `Difference(Box − Rechteck-Notch)` = 12 V / 16 E / 8 F watertight (gut). `Difference(Box − zylindrischer Bohrung, Achse Y, r=0.5)` = 8 V / **108 E** / 7 F: die beiden Trim-Kreise (Bohrungswand trifft +y/−y-Box-Fläche) waren massiv über-segmentiert (sollte je *ein* Kreis sein). `Intersect(Box, Box)` war bereits korrekt (8 V / 12 E / 6 F).

**Ursache (empirisch verifiziert, nicht geraten).** Zwei verschränkte Phase-3-Extraktions-Defekte bei *gekrümmten* Difference-Flächen:
1. **Kontaminierter Trim-Kreis-Fit.** Die echten Trim-Punkte liegen sauber auf y=±1, r=0.5. Aber Octree-Zellen, die den Saum überspannen, reichen ein bis zwei Zellen tief in die reine Wand hinein; ihre Ecken-Contributor-Vereinigung trägt die Trim-Signatur, also emittiert `extract_edge_point` dort *auf der Wand* liegende Punkte bei *anderem* y (≈0.70–0.90). Die Gruppe ist damit **nicht koplanar** (y∈[0.70,1.0]), der bestehende `fit_circle` (strikte Planarität gegen den Centroid) scheitert → Fallback auf Gap-Chaining → Fragmente. (Bei res 0.10: 188 von 340 Punkten auf dem echten Ring y=1, Rest Kontaminationsband.)
2. **Phantom-Signaturen.** Zellen auf der Zylinderwand nahe den x-/z-Box-Flächen erben über das Contributor-`eps` der Difference (`combine_max(box, −cyl, eps)`) eine Box-Flächen-Branch, obwohl die Wand (r=0.5) diese Flächen physisch nie trifft. Es entstehen Gruppen wie `(Box-x-Fläche, Zyl-Wand)`, deren Punkte exakt auf der Wand (r=0.5001–0.5008), aber nirgends nahe x=±1 liegen — kein echter Kanten-Saum.

**Entscheidung 1 — Outlier-robuster Kreis-Fit (`edge_walker.rs::fit_circle`).** Statt strikter Globalplanarität: PCA-Normale wie bisher, aber die **Ebenen-Offset** über *Mode-Seeking* entlang der Normalen (dichteste Ebene innerhalb `tol`), nicht Centroid/Median — der Kontaminations-Tail zieht beide weg, die dichteste Ebene ist der echte Ring. Inliers = Punkte ≤ `tol` von dieser Ebene. Kåsa-Fit nur auf Inliers. Akzeptiert nur, wenn Inliers (a) ≥ `CIRCLE_INLIER_FRACTION`=0.4 der Gruppe, (b) den vollen Ring abdecken (größte Winkellücke inkl. Wrap < `CIRCLE_MAX_ANGULAR_GAP`=π/2), (c) Radial-Residuum ≤ `tol`. Eine Teilbogen-/Streucluster-Gruppe scheitert an (a) oder (b) und fällt auf Gap-Chaining zurück.

**Entscheidung 2 — Phantom-Signatur-Filter (`edge_walker.rs::chain_on_both_surfaces`).** Ein echter B-Rep-Saum zwischen zwei *analytischen* Flächen liegt auf **beiden** Flächen. Nach dem Fit/Chaining wird jede konsolidierte Kette gegen die analytischen Flächen *beider* Branches (aus `weaving::branch_halfspace_map`, `sdf_halfspace` im jeweils inversen Face-Transform) geprüft: **alle** Sample-Punkte müssen ≤ `spacing` von beiden Flächen liegen, sonst wird die Kette verworfen. Branches ohne analytische Fläche (tesselierte Sketch-Sweeps, DL-25/DL-50) werden nicht geprüft (nie verworfen). Der Filter sitzt *nach* dem Fit (nicht auf der Rohgruppe), damit der Kreis-Fit erst die echten Ring-Inliers extrahiert, bevor geprüft wird — sonst würde der Kontaminations-Tail die Majorität kippen und den echten Ring fälschlich verwerfen.

**Entscheidung 3 — Filter unter Domain-Warp deaktiviert (`body_has_warp`).** Bei Twist/Bend/Displace beschreibt `branch_halfspace_map` das *un-gewarpte* Primitiv (die gespeicherte Analytik-Fläche ist eine bewusste Näherung). Der Filter würde dann *jede* gewarpte Kante verwerfen (Punkte liegen weit neben der flachen Ebene). Daher: enthält der Body einen Warp-Knoten, bleibt `halfspaces` leer → Filter ist No-op. (Fix für die Regression `body_step_export_twisted_box_refs_resolve`.)

**Ergebnis V/E/F (res 0.25).** `Difference(Box − Notch)` 12 V / 16 E / 8 F watertight (unverändert, keine Regression). `Difference(Box − Bohrung)` 8 V / **14 E** / 7 F watertight (von 108 E): die 12 Box-Kanten + **2 saubere Trim-Kreise** (je *eine* geschlossene Schleife `start=end=None`, 118 Samples → 1 Kante); Bohrungswand-Face genau von 2 Kreis-Kanten berandet. `Intersect(Box, Box)` 8 V / 12 E / 6 F watertight (unverändert). Watertight = jede Kante in genau 2 Face-Loops.

**Dokumentierte Grenze.** Der zugrunde liegende Phase-3-Defekt (Contributor-`eps`-Leak auf gekrümmten Difference-Wänden, der sowohl die Trim-Kontamination als auch die Phantom-Signaturen erzeugt) ist **nicht** an der Quelle behoben — er wird in Phase 3.5 robust *maskiert* (dichtester-Ebene-Inlier-Fit + analytischer Zwei-Flächen-Membership-Filter). Das ist korrekt und stabil für analytisch berandete Booleans (Box/Zylinder/Kegel), setzt aber voraus, dass beide Saum-Flächen eine `HalfSpace`-Repräsentation haben. Ein Saum zwischen zwei *tesselierten* gekrümmten Flächen (z. B. Sphere−Sphere, beide ohne Analytik-Eintrag) wird vom Filter nicht geprüft und könnte weiterhin über-segmentieren; eine echte Quell-Korrektur (saubere Edge-Zell-Klassifikation an gekrümmten Difference-Säumen) bleibt ein separater Phase-1/3-Schritt.

**Gate grün (SA-4a).** Kernel-Lib 456 passed / 0 failed (453 Basis + 3 neue), Protocol 58 passed / 0 failed. Clippy in den geänderten Dateien (`edge_walker.rs`, `weaving.rs`) warnungsfrei (vorbestehende Protocol-Clippy-Fehler + Kernel-Warnings stammen aus früheren SA-Phasen, nicht aus diesem Task). Neue Gate-Tests in weaving.rs: `difference_box_minus_notch_weaves_watertight` (16 E / 8 F, keine Schleifen), `difference_box_minus_cylindrical_drill_consolidates_trim_circles` (14 E / 7 F, Wand-Branch von genau 2 geschlossenen Kreis-Kanten berandet), `intersect_two_boxes_weaves_watertight_overlap_box` (12 E / 6 F).

### DL-53: Körper bleiben getrennt ohne explizite Feature-Verbindung; smin-Verrundungen sind keine B-Rep-Flächen (SA-4 Scope-Festlegung)

**Kontext.** SA-4 war ursprünglich als „globaler Interaktions-Schritt (Booleans/Blends)" angelegt. Nach der Within-Body-Boolean-Korrektur (DL-52) bleiben zwei Teilfragen, die Produktintention statt Technik sind. Beide hier verbindlich entschieden (Nutzer, 2026-05-31).

**Entscheidung 1 — Cross-Body-Trimming: NEIN (Baugruppen-Modell).** Solange zwei Körper nicht *explizit* über ein Feature verbunden sind (gemeinsame `Union`-Wurzel bzw. eine `Difference`/`Intersect`, die per `target_ids`/AABB an einen Body andockt — siehe `body.rs::partition`), bleiben sie auch bei räumlicher Überschneidung **getrennte Körper**. Jeder Body baut seinen B-Rep unabhängig; die Durchdringungszone wird *nicht* geometrisch aufgelöst, gemeinsame Innenwände bleiben bestehen. Das entspricht dem OnShape-Standard (separate Solids einer Baugruppe) und dem bestehenden `partition()` (Test `overlapping_unions_stay_separate`). Es wird **kein** globaler Octree-/Boolean-Schritt über Body-Grenzen hinweg gebaut. STEP-Export = mehrere Solids.

**Entscheidung 2 — smin-Verrundungen bleiben tesselierte Flächen, kein eigener Blend-B-Rep.** Durch `smin`/`SmoothUnion`/`MaskedBlend` erzeugte Fillet-Übergänge bekommen *keine* eigene Blend-BranchId und werden *nicht* zu analytischen B-Rep-Flächen rekonstruiert (Status quo: Tessellated, geometrisch korrekt & wasserdicht). Begründung: Fillets sind in der CAD-Praxis meist *nachgelagerte* Features auf bereits existierenden Kanten, nicht primäre Modellflächen. **Chamfer** (`chamfer_union`, planarer Übergang) *sollte* perspektivisch echte Kanten/Flächen bilden — das ist aber ebenfalls auf einen **zukünftigen Meilenstein** vertagt, nicht Teil von SA-4.

**Konsequenz.** SA-4 ist mit DL-52 (Within-Body-Booleans) inhaltlich abgeschlossen; 4b (Cross-Body) und 4c (Blend-Flächen) sind bewusst *nicht* implementiert. Nächster Schritt ist SA-5 (Naming + STEP-Validierung).

### DL-54: B-Rep-Cache ist platzierungs-abhängig — affiner Move erzwingt Re-Weave (kein analytischer Transform-Fast-Path), vertagt

**Kontext.** Frage des Nutzers (2026-06-01): Sind die B-Reps körperlokal, und sind damit affine Transformationen auf den Körpern performant? **Befund, hier verbindlich festgehalten:** Die B-Reps *sind* körperlokal (jeder `Body` webt + cached seinen eigenen `brep: Option<(u64, BRep)>` via `ensure_brep`, body-relative Octree-Auflösung per `topology_min_cell_size`, DL-28; Cache-Isolation zwischen Körpern). **Aber** affine Transforms sind für die exakte B-Rep *nicht* billig: Der Cache-Key ist der volle `payload_hash` (Geometrie **und** Platzierung, DL-50), weil sowohl Octree-Vertices als auch analytische Face-`Transform`s die Welt-Pose backen. Jeder Move/Rotate ⇒ neuer Key ⇒ **voller Re-Weave** (Octree-Rebuild + `weave`). Der einzige existierende affine Fast-Path ist der **Mesh**-Pfad für reine Translation (`geometry_hash` gleich, `transform_hash` verschieden ⇒ `transform_delta` als GPU-Matrix, kein Remesh) — das betrifft die *Darstellung*, nicht die B-Rep.

**Entscheidung — vertagt, nicht jetzt umgesetzt.** Eine echte Performance-Lösung wäre, die B-Rep in **körperlokalen** Koordinaten zu weben und die Placement-`Transform` erst beim Konsum (STEP-Export, Counts, Rendering) anzuwenden; ein reiner Rigid-Move würde dann nur die lokale Pose-Schicht aktualisieren und die gecachte lokale B-Rep analytisch transformieren (Vertices + Ebenen/Zylinder-Frames mitdrehen) statt neu zu weben. Bewusst aufgeschoben: nicht auf dem kritischen Pfad, B-Rep-Recompute ist heute nicht heiß, und der lokale Weave ist sauber als isoliertes Upgrade nachrüstbar (vgl. DL-50, letzter Absatz). Wird reaktiviert, falls B-Rep-Recompute bei interaktivem Verschieben spürbar wird.

### DL-55: SA-5 — analytische STEP-Flächen (Sphere/Cone/Torus), Mehr-Körper-Szenenexport, semantischer Validator; Naming an der Sketch-SDF-Pipeline verifiziert

**Kontext.** SA-5 = „Naming + STEP-Validierung". Naming (`brep/naming.rs`) und STEP-Export (`brep/step_export.rs`) waren portiert und unit-getestet, aber (a) nur gegen Primitiv-Würfel validiert und (b) der STEP-Export hob nur `Plane`/`InfiniteCylinder` analytisch — `Sphere`/`Cone`/`Torus` fielen auf `fallback_plane` zurück, obwohl DL-50 analytische `Cone`-Flächen (schräge Revolve-Profile) erzeugt. Das war der eigentliche Fidelity-Verlust.

**Entscheidung 1 — analytische STEP-Flächen geschlossen.** `emit_surface` hebt jetzt `HalfSpace::Sphere → SPHERICAL_SURFACE`, `Cone → CONICAL_SURFACE` (AXIS2_PLACEMENT im Apex ⇒ Referenzradius `0.0`, `semi_angle = half_angle`), `Torus → TOROIDAL_SURFACE`. Alle folgen der Zylinder-Konvention (Achse = lokales +Y, Ursprung = Transform-Translation). `Gyroid` (kein Closed-Form) und `Tessellated` bleiben bewusst auf `fallback_plane`.

**Entscheidung 2 — Mehr-Körper-Szenenexport (DL-53-konform).** Die Pro-Solid-Emission ist in `emit_brep_solids(w, g, brep, name, out)` faktorisiert; `export_step` = `export_step_scene(&[brep], name)`, und `export_step_scene(breps, name)` emittiert N Körper als N `MANIFOLD_SOLID_BREP` in *einer* `ADVANCED_BREP_SHAPE_REPRESENTATION` mit gemeinsamem `StepWriter`/`Geometry`-Cache (Punkte/Richtungen deduplizieren über Solids hinweg, IDs bleiben eindeutig). WASM: `sceneStepExport()` webt alle Körper (`ensure_brep`) und exportiert die Szene. Entspricht DL-53 (getrennte Körper = mehrere Solids in einer Datei).

**Entscheidung 3 — semantischer Validator statt externem Viewer.** Kein FreeCAD/pythonOCC in der Umgebung; das D.4-Abnahmekriterium („importierbar in externem CAD-Viewer") ist hier nicht automatisierbar und bleibt ein manueller/CI-Schritt. Stattdessen `pub fn validate_step(&str) -> Result<(), Vec<String>>`: prüft (1) Referenz-Abschluss, (2) genau eine erreichbare `ADVANCED_BREP_SHAPE_REPRESENTATION`, (3) ≥1 `MANIFOLD_SOLID_BREP`, (4) jede `ADVANCED_FACE` referenziert eine *echte* Flächen-Entität (Surface-Ref auf Klammer-Tiefe 1, nicht in der Bounds-Liste), (5) Einheiten/Kontext vorhanden. Gate-Tests laufen damit auf Box, Sketch-Extrude, **Revolve-Cone** (bestätigt `CONICAL_SURFACE` end-to-end aus dem echten Revolve-Weaver), Sphere und Mehr-Körper-Szene.

**Befund A — Naming an Sketch-SDF-Körpern: Flächen stabil, Kanten topologie-abhängig.** Ein `TopoRef` auf eine analytische Seitenwand eines Rechteck-Extrude-Körpers überlebt eine **Sketch-Breitenänderung** mit `Resolved` (Score > 0.95) — die Wand-BranchId leitet sich aus Feature-UUID + Sub-ID ab, also Hash-stabil. Kanten: Ein Re-Weave bei geänderter **Extrude-Tiefe** kann die Topologie umstrukturieren (12 → 10 Kanten: zwei Wand-Cap-Ränder werden zu geschlossenen Loop-Kanten konsolidiert, zwei Wand-Wand-Vertikalen entfallen). Für Kanten mit *persistentem* Branch-Paar (Wand-Wand-Vertikale) löst `TopoRef` `Resolved` (>0.95); umstrukturierte Branch-Paare erreichen nur das `Ambiguous`-Fingerprint-Band (~0.63, kein Hash-Treffer). **Das ist eine Weaver-Topologiestabilitäts-Grenze, kein Naming-Defekt** — Naming ist genau dann stabil, wenn die zugrundeliegende Topologie stabil ist. Ehrlich als Test-Doc-Kommentar festgehalten (kein gefakter Test).

**Befund B — Sphere-Weaver-Lücke (offener Folgeschritt).** `primitive_halfspaces` liefert für Kugeln `None`; eine Kugel nimmt mit *einer* Ganz-Primitiv-BranchId teil und ihre Wand wird `Tessellated` gewoben — es entsteht *keine* analytische `HalfSpace::Sphere`-Face. Daher ist die `SPHERICAL_SURFACE`-**Emission** über eine handgebaute B-Rep validiert (gleiches Muster wie `rational_curve_emits_complex_instance`); der end-to-end-Pfad Kugel → analytische Sphere-Face ist ein separater Weaver-Schritt. **Der Cone hingegen ist end-to-end durch den Revolve-Weaver erzeugt.**

**Gate grün (SA-5, unabhängig verifiziert).** Kernel-Lib 463 passed / 0 failed (189.64s), Protocol 58 passed / 0 failed (1 ignored), clippy `-D warnings` fehlerfrei. `brep/step_export.rs` (+6 Tests), `brep/naming.rs` (+2 Extrude-Tests), `lib.rs` (`sceneStepExport`), `body.rs` (`brep_cached`).

### DL-56: Korrektur von DL-50 — B-Rep in körperlokalen Koordinaten gewoben, Platzierung erst beim Konsum (kein World-Space-Baking mehr)

**Kontext & Kurskorrektur.** DL-50 (Entscheidung 3) und DL-54 hatten den B-Rep bewusst in **Welt**-Koordinaten gewoben und den Cache auf den vollen `payload_hash` (Geometrie + Platzierung) geschlüsselt — jeder Move erzwang einen vollen Re-Weave. Der Nutzer hat das (2026-06-01) als das identifiziert, was es war: ein Quick-Win, der den eigentlichen Zweck eines körperlokalen B-Rep verfehlt. Diese DL **ersetzt** DL-50/Entsch.3 und DL-54: Der B-Rep wird jetzt **körperlokal** gewoben, die Platzierung als separate Schicht erst beim Konsum angewandt.

**Datenmodell-Befund (entscheidend).** Ein `Body` ist eine *ephemere Laufzeit-Partition* (`feature_indices`-Liste), **kein** persistiertes Dokument-Entity; `Feature.transform` ist die Welt-Platzierung des Primitivs. Daher *kein* persistiertes `Body.placement`-Feld (das würde nicht ins Modell passen und Frontend-/Serialisierungs-Umbau erzwingen) — stattdessen ein **Laufzeit**-Feld.

**Entscheidung 1 — Laufzeit-`Body.placement: Transform`.** Nicht serialisiert (`Body` ist nur `Debug+Clone`). `compute_body_placement(features, indices)` = Transform des **Anker-Features** `indices[0]` (die Seed-Union, die den Body startet), beschränkt auf **rigide + uniforme Skalierung** (`scale = [min_scale; 3]`, Rotation + Translation des Ankers). Nicht-uniforme Skalierung des Ankers bleibt im *lokalen* Weave (beliebige Affine ist dort unkritisch); so bleibt `placement` in der Klasse, unter der Zylinder/Kegel/Kugel/Torus geschlossen sind.

**Entscheidung 2 — lokaler Weave + platzierungs-invarianter Cache-Key.** `BoundedCsg::from_csg_with_carry(csg, carry)` (neu; `from_csg` = `carry = IDENTITY`); `build_bounded_for_body_local(features, indices, placement)` startet mit `inverse(placement)` als Carry → jedes Leaf-`world_transform` wird zu `inverse(placement) ∘ feature.transform`, Octree sampelt die *lokale* SDF, analytische Faces + Vertices kommen platzierungsfrei heraus. `inverse(&Transform)` (lib.rs, rigide + uniform: `R⁻¹`, `1/s`, `−R⁻¹·(t/s)`, mit Round-Trip-Test). Neuer Schlüssel `hash_body_local_payload` = Geometrie-Hash (aus `hash_body_payload_split`) + je Feature `inverse(placement) ∘ feature.transform`. **Invarianz** (algebraisch verifiziert): bei reiner Verschiebung um `D` shiften `placement.translate` und jede `feature.transform.translate` um dasselbe `D`, sodass das komponierte lokale Transform `R⁻¹·((tᵢ−t₀)/s)` ist — `D` kürzt sich, Key bleibt konstant.

**Entscheidung 3 — Platzierung am Konsum-Rand.** `placement` wird bei *jedem* `ensure_brep`-Aufruf billig neu berechnet (O(features), außerhalb des Stale-Gates); der Weave ist auf den lokalen Key gegated. Folge: reine Bewegung ⇒ Key unverändert ⇒ **Cache-Hit, kein Re-Weave**, nur `placement` aktualisiert. **STEP-Export** (`export_step_placed`/`export_step_scene_placed`, `place_point`) komponiert `placement ∘ face.transform` und faltet die Platzierung auf B-Rep-Vertices/Curve-CPs/Tessellated-Koords zurück → Welt-Output **byte-äquivalent** zum alten World-Weave. **Naming**-Fingerprints liegen jetzt im *lokalen* Frame (über Rebuilds konsistent → move-stabiler, hilft Befund A aus DL-55). **Counts** frame-invariant. **Mesh-Pfad bleibt unberührt** (SDF-Dual-Contouring in Welt, `transform_delta`-Translations-Fast-Path unverändert) — B-Rep und Mesh sind unabhängige Konsumenten.

**Gate grün (DL-56, unabhängig verifiziert).** Kernel-Lib 466 passed / 0 failed (177s), Protocol 58/0 (1 ignored), clippy `-D warnings` fehlerfrei. Schlüssel-Tests: `move_reuses_local_brep_cache` (Key unverändert **und** Pointer-Identität der gecachten B-Rep ⇒ kein Re-Weave) und `step_world_equivalence_under_move` (Box am Ursprung vs. um `D` verschoben → exportierte CARTESIAN_POINTs differieren exakt um `D`). Bestehende Tests unverändert (Primitiv-Tests nutzen Identity, lokal == Welt). Dateien: `lib.rs` (`inverse`, WASM-Placement-Threading, 3 Tests), `bvh.rs` (`from_csg_with_carry`), `body.rs` (`placement`-Feld, `compute_body_placement`, `hash_body_local_payload`, `build_bounded_for_body_local`, reworked `ensure_brep`), `brep/step_export.rs` (`place_point`, Placement durch die Emit-Kette, `*_placed`-Einstiege).

### DL-57: SA-6 — Kugel/Torus erzeugen analytische B-Rep-Flächen; kantenlose geschlossene Analytik-Face (schließt DL-55 Befund B)

**Kontext.** DL-55 Befund B: `primitive_halfspaces` lieferte für Kugeln `None` → die Kugelwand wurde `Tessellated` gewoben, keine analytische `HalfSpace::Sphere`-Face; `SPHERICAL_SURFACE` war nur über eine handgebaute B-Rep validiert. SA-6 schließt den end-to-end-Pfad.

**Entscheidung 1 — Sphere- und Torus-Arme.** `primitive_halfspaces` (csg.rs) bekommt je einen Arm: `Sphere{center,radius} → eine Face HalfSpace::Sphere{radius}` (sub 0, `center` gefaltet), `Torus{center,major,minor} → eine Face HalfSpace::Torus{…}`. `expand_evaluate_leaf` nimmt beide in den generischen `Box|Cylinder`-Arm auf (bei `len==1` taggt die Max-über-Faces-Schleife trivial sub 0). Branch-Tagging läuft über den f32-Pfad (`Octree::build` → `expand_evaluate_leaf`); der Dual-Pfad (`sdf_dual.rs`) ist am Tagging nicht beteiligt, daher unverändert.

**Entscheidung 2 — kantenlose geschlossene Analytik-Face.** Ein einzelnes geschlossenes Analytik-Primitiv hat *einen* Branch → **null Kanten-Zellen** (Kanten brauchen ≥2-Branch-Kollisionen), und der kantenbasierte Face-Loop erzeugte zuvor *gar keine* Face (leere B-Rep — kein Tessellated-Fallback, sondern Nichts). Fix in `weave` (weaving.rs): **nur wenn `brep.edges.is_empty()`** wird je Halfspace-Branch (der nicht ohnehin Kanten hat) eine Face mit **leerem Outer-Loop** emittiert (vollständig geschlossene, randlose Fläche); `is_closed = (face_ids.len() == 1)`. Der Guard `edges.is_empty()` stellt sicher, dass Box/Cylinder/Extrude/Revolve (immer mit Kanten) komplett unberührt bleiben; weggeklippte Branches in Mehr-Feature-Körpern werden zusätzlich übersprungen. STEP-Export lässt für randlose Faces den `FACE_OUTER_BOUND` weg → `ADVANCED_FACE` trägt nur die Surface (ISO-Standardform einer geschlossenen Fläche); der Validator akzeptiert das (prüft nur die Surface-Ref).

**Gate grün (SA-6, unabhängig verifiziert).** Kernel-Lib 470 passed / 0 failed (178s), Protocol 58/0 (1 ignored), clippy fehlerfrei. End-to-end-Tests: lone Sphere → 1 analytische `HalfSpace::Sphere`-Face / 0 Kanten / 0 Vertices / geschlossene Shell → `export_step` emittiert genau ein `SPHERICAL_SURFACE` (kein Fallback-Plane), `validate_step` ok; analog Torus → `TOROIDAL_SURFACE`; off-origin-Center faltet in die Face-Transform (DL-20); alle bestehenden Kugel-Mesh-Tests bleiben grün. `validate_sphere_face_emits_spherical_surface` ist von handgebaut auf end-to-end umgestellt. Dateien: `csg.rs`, `brep/weaving.rs`, `brep/step_export.rs`.

### DL-58: Reflex-bewusste Face-Loop-Ordnung (Turning-Angle) + Mehrfach-Loops (Outer + Holes) im Weaver

**Kontext.** `order_loop` (alt) war ein gieriger Shared-Vertex-Walk: am ersten Vertex, an dem ≥3 Face-Kanten zusammentreffen (Reflex-Ecke; oder eine Face, die ein Loch berührt), wählte das `break` die *erste* passende Kante in Iterationsreihenfolge — geometrisch falsch → falscher Ring oder `None` → kompletter Fallback auf eine **ungeordnete** `Loop` (alle `reversed=false`). Mehrfach-Loops (Outer + Löcher) wurden nie gebildet (`inner: Vec::new()` hart). Würfel liefen nur, weil konvexe Faces genau 2 Kanten/Vertex haben.

**Entscheidung 1 — Turning-Angle-Walk.** `order_loop` → `order_loops(brep, edge_ids, surface) -> Option<FaceLoops{outer, inner}>`. Die Face-Normale kommt aus `surface_normal_at(&SurfaceGeometry, p)`: Plane → rotierte Plane-Normale (konstant), Cylinder/Cone → radiale Richtung von der lokalen +Y-Achse zum Punkt, Sphere → Mittelpunkt→Punkt, Torus → nächster Punkt auf dem Mittenkreis→Punkt; Tessellated/Gyroid → `None` (Fallback auf den gierigen Walk). Am aktuellen Vertex sitzt der Walk mit der *umgekehrten Eingangs-Tangente* `back` (in die Tangentialebene projiziert); unter allen unbenutzten inzidenten Kanten wird die mit dem **größten CCW-Drehwinkel** (in `[0,2π)`) von `back` zur Ausgangs-Tangente gewählt — die nächste Kante im Gegenuhrzeigersinn, die das Face-Innere konsistent auf einer Seite hält und auch an Reflex-Ecken korrekt abbiegt. Tangenten kommen aus den Kurven-Samples (NURBS-Control-Points / Polyline / Hermite), Fallback auf die gerade Sehne.

**Entscheidung 2 — Mehrfach-Loops + Outer/Inner-Klassifikation.** Selbst-schließende Kreis-Kanten (start=end=None, z.B. Trim-Kreise) sind je ein eigener Ein-Kanten-Ring; übrige Vertex-begrenzte Kanten werden iterativ in Ringe geordnet (jeder verbleibende ungenutzte Seed startet einen neuen Ring). Klassifikation: **planar** über projizierte vorzeichenbehaftete Fläche in der Tangentialebene — größter Betrag = Outer; Outer wird auf CCW (Fläche > 0) gedreht, Löcher auf CW. **Gekrümmt** (Cylinder/Cone/Sphere/Torus) über die Ring-Ausdehnung (Summe der Sehnenlängen / Control-Polygon-Span) — längster Ring = Outer (DL-58). `flip_orientations` dreht beim Umkehren eines Rings die Per-Eintrag-`reversed`-Flags mit, damit Head-to-Tail erhalten bleibt.

**Entscheidung 3 — Konnektivitäts-Garantie + lauter Fallback.** Neuer Helper `Loop::is_connected_ring(&[Edge])`: jeder Eintrag-Endvertex == nächster-Start, letzter schließt auf den ersten (leerer Loop vakuum-ok; Ein-Kanten-Kreis-Loop ok). `weave` `debug_assert!`t, dass erfolgreich geordnete Outer-/Inner-Loops verbundene Ringe sind. Wenn `order_loops` `None` liefert (degenerierte Topologie), bleibt der ungeordnete Fallback — aber **laut geloggt** (`eprintln!`), nicht still.

**Ehrlicher Befund — Octree-Weaver vs. Reflex-Extrude.** Ein L-Polygon-Extrude (Reflex-Ecke bei (1,1)) liefert die **Cap-Face nicht** als sauberen Ring: bei grober Auflösung (0.1/0.05) fehlt der Reflex-*Vertex* (Phase-3 klassifiziert die 3-Branch-Kollision dort nicht), bei feiner (0.025) ist der Vertex da, aber eine *Rand-Kante* der Cap fehlt (5 statt 6 Kanten). Das ist eine **Phase-3-Extraktions-Lücke vor der Ordnung**, nicht ein Ordnungs-Fehler — `order_loops` gibt korrekt `None` (kein Fake-Ring), die Face fällt in den geloggten Fallback. Der Reflex-Ordnungs-Algorithmus wird daher an einer **handgebauten** vollständigen Reflex-Hexagon-Geometrie verifiziert (`order_loops_reflex_polygon_is_one_connected_ring`) — er threadet den Reflex-Vertex und gewinnt einen einzelnen CCW-Ring. Der Box-minus-Zylinder-Durchbruch dagegen wird vom Weaver **sauber** als Face-mit-Loch produziert: die beiden durchbohrten ±y-Planar-Faces bekommen je Outer-Rechteck + genau einen inneren Kreis-Loop (die Face wird **nicht** gesplittet), die Bohrwand bekommt zwei Trim-Kreise (einer Outer, einer Inner) → STEP emittiert je 1 `FACE_OUTER_BOUND` + 1 `FACE_BOUND` für die gelochten Faces.

**Gate grün (#22, unabhängig verifiziert).** Kernel-Lib 474 passed / 0 failed (175s), Protocol 58/0 (1 ignored), clippy `--workspace --all-targets -D warnings` fehlerfrei. Dateien: `brep/mod.rs` (`Loop::is_connected_ring`), `brep/weaving.rs` (`order_loops` + Helfer + Tests).

### DL-59: Viewport-Kanten kommen aus dem analytischen B-Rep; mesh-abgeleiteter Topologie-Pfad ersatzlos entfernt

**Kontext.** Es existierten **zwei** mesh-abgeleitete Kanten-Pfade fürs Frontend: (1) `edge_detect.rs` (`detectEdges`, K10.6 — Contributor-Wechsel entlang der DC-Mesh-Dreiecke) und (2) `topology.rs` (`extract_topology` → `bodyTopologyJson` + `bodyFace*`/`bodyEdge*`-Queries). Beide lasen Kanten *vom Dreiecksnetz ab* → auflösungsabhängig und nur über den OVERLAY_LAYER im SDF-Pass sichtbar (im Mesh-Modus gar nicht). Das echte analytische B-Rep (woven, körperlokal, DL-56) war hingegen reines Backend (nur STEP-Export + Counts) und wurde **nie visualisiert**. Der User hat den alten Ansatz als obsolet eingestuft → entfernen, durch die analytischen Kanten ersetzen, hoverbar machen.

**Entscheidung 1 — analytische Kanten als Welt-Polylinien.** Neues `brep/edge_sampling.rs`: `sample_edge(&EdgeGeometry) -> Vec<[f64;3]>`. Woven Edges sind `EdgeGeometry::Nurbs` (degree-3 clamped Bézier-Kette aus `hermite_to_nurbs`); gesampelt wird per **rationalem de Casteljau pro Bézier-Span** (16 Segmente/Span; C⁰-Join einmalig emittiert). Polyline/Hermite sind defensive Pass-throughs. Neues WASM-Binding `bodyBrepEdges(i)` → JSON-Array `{a,b,points[]}`: Punkte im körperlokalen Frame gesampelt und per `placement` (rigid+uniform) auf Welt gefaltet — exakt wie der STEP-Export (DL-56). `a`/`b` sind das kanonische Branch-Signatur-Paar (`BranchId::as_u32()`), **dieselben** Werte, die `pick` in `contributors` liefert.

**Entscheidung 2 — Hover via Signatur-Match (kein eigener Hover-Kernel).** Der Pick-/Hover-Pfad (`raycast.rs` + CSG-`ProxyElement`) ist von der Topologie unabhängig und bleibt. Das Frontend hebt die Kante hervor, deren Signatur `{a,b}` beide in `pick.contributors` enthält (nur wenn `element == "edge"`). Damit leuchtet exakt die analytische Edge auf, die auch STEP exportiert.

**Entscheidung 3 — alten Pfad ersatzlos löschen (no-compat).** Gelöscht: `edge_detect.rs`, `topology.rs`, `Body::topology` + `ensure_topology`, alle WASM-Bindings `detectEdges`/`bodyTopologyJson`/`bodyFaceCount|Contributor|Area|Centroid|Normal|TriangleCount|TrianglesPtr`/`bodyEdgeCount|ContributorA|ContributorB|PointCount|PointsPtr`. Web: `EdgeOverlay` entfernt, `BRepEdgeOverlay` auf `bodyBrepEdges` umgestellt (+ Hover-Highlight), `topologyJson` aus `MesherBodyPayload`/Store, K6b-Sketch-Snap (`SketchCanvas2d`) auf analytische Edges migriert. Das Overlay liegt jetzt auf `enable(OVERLAY_LAYER)` (behält Layer 0) → in **Mesh- und SDF-Modus** sichtbar; Material `depthTest:false` (Wireframe-Overlay immer sichtbar, Tiefen-Occlusion ist offene Politur).

**Gate grün (#26–#30, unabhängig verifiziert).** Kernel-Lib cargo check + clippy `-D warnings` clean, neue `edge_sampling`-Tests grün; Vitest 205/207 (die 2 Failures = vorbestehende `SettingsMenu/loadSettings`); Web-`tsc -b` identische Fehleranzahl (16) mit/ohne Änderung → **null neue Typfehler** (die 16 sind vorbestehend: WebGPURenderer-Typen, Test-Dateien). Dateien: `brep/edge_sampling.rs` (neu), `brep/mod.rs`, `lib.rs` (`bodyBrepEdges`), `body.rs`; Web: `kernel.ts`, `viewportOverlays.tsx`, `Viewport.tsx`, `SketchCanvas2d.tsx`, `mesher.worker.ts`, `store.ts`.

**Offen.** Tiefen-korrekte Kanten-Occlusion im Mesh-Modus (aktuell alle Kanten on-top). Die Phase-3-Reflex-Extraktionslücke (Task #25) ist mit **DL-60** geschlossen — fehlende Reflex-Ecken werden jetzt analytisch rekonstruiert und damit auch gezeichnet.

### DL-60: Reflex-Vertices analytisch rekonstruieren (Phase-3-Lücke an konkaven Ecken)

**Problem.** Die Octree-Klassifikation (`brep/octree.rs::classify`) taggt eine Zelle nur dann als `Vertex`, wenn dort **≥3 Branches gleichzeitig aktiv** sind (`f_min ≤ diag` als Oberflächennähe-Gate). An einer **konvexen** Ecke treffen die Halbräume am Apex zusammen → alle 3 liegen nah am Null-Level → `Vertex`. An einer **reflexen (konkaven)** Ecke öffnet sich der Körper über 180°: innen dominiert die jeweils nächste Wand, der dritte Halbraum ist nur in einer quasi-maßnullen Region aktiv. Die Eckzelle wird daher als 2-Branch-`Edge` fehlklassifiziert, die Ecke fehlt im Vertex-Set. Folge am L-Prisma (6-Ecken-Profil, 1 reflexe Ecke): nur 10 von 12 Ecken, und `order_loops` fiel für jede an der Lücke hängende Fläche auf seinen ungeordneten Fallback zurück. Symmetrisch verschärft `SNAP_FACTOR=2.5`: eine konvexe Ecke, deren Linien-Fit-Endpunkt ~3 Zellen zu kurz endet, verfehlt den (existierenden) Vertex ebenfalls knapp und bleibt dangling.

**Entscheidung — kantenbasierte Rekonstruktion + Re-Snap (Option B), kein feineres Sampling.** Statt die Klassifikations-Heuristik zu verfeinern (auflösungsabhängig, fragil), wird die Ecke aus der **bereits detektierten Topologie** wiederhergestellt. Neuer Post-Pass `reconcile_dangling_endpoints` in `edge_walker.rs`, nach dem Verdrahten der Edges:

1. **Stage 1 — Re-Snap an existierenden Vertex.** Jeder dangling Endpunkt sucht den nächsten Vertex, dessen Branch-Set die Edge-Signatur `{a,b}` **enthält**, innerhalb `RECONCILE_TOL_FACTOR=4×` Spacing. Branch-Set-Containment (nicht nur Distanz) hält den Match topologisch korrekt → fängt die knapp verfehlten konvexen Ecken sauber ein.
2. **Stage 2 — fehlende Ecke rekonstruieren.** Verbleibende dangling Enden werden räumlich geclustert; wo **≥3 Edges mit ≥3 verschiedenen Branches** zusammentreffen, wird am Cluster-Zentrum eine **synthetische `Vertex`-Zelle** gebildet und an `extract_vertex` übergeben — unverändert, inkl. Ebenen-Schnitt-LSQ + Refinement + Acceptance-/Phantom-Gate. Das Gate weist Cluster ab, die keine echte Ecke sind (kein Quick-Win-Hack). Der gelöste Punkt ist der **analytische** Schnitt der inzidenten Flächen — am L-Prisma exakt `(1,1,±1)`, nicht der zellverkürzte Endpunkt-Schwerpunkt.

**Ergebnis.** L-Prisma: 12/12 Ecken, 18 Kanten, 8 analytische Flächen, wasserdicht; `order_loops` braucht **keinen** Fallback mehr. Keine Änderung an der `extract_vertex`-Kernlogik (nur Wiederverwendung), kein neuer Sampling-Pfad. Test `l_polygon_extrude_weaves_watertight_eight_planes` verschärft: Vertex-Count 12 + Positions-Assertion der zwei rekonstruierten Reflex-Ecken. Ein sauberer Körper (Würfel) hat keine dangling Enden → Pass ist ein No-op (keine Regression).

### DL-61: Fat-`Feature`-Spine auflösen — dekodierte IR ist der offene `CsgNode`-Baum, nicht ein Feld-Union-Struct

**Problem.** `protocol::Feature` (die dekodierte Laufzeit-„Spine"-IR) ist ein **Feld-Union über alle Built-in-Features**: `primitive`, `transform`, `blend_radius`, `chamfer_size`, `twist_rate`, `bend_rate`, `displace_depth`, `displace_freq`, `shell_thickness`, `target_ids`. Jedes neue Feature, das einen neuen Modifier braucht, müsste ein weiteres Feld an diesem zentralen Struct ergänzen. Das ist mit dem geplanten **FieldSkript** (laufzeit-/skriptdefinierte Features, Onshape-FeatureScript-Analog) grundsätzlich unvereinbar: ein zur Laufzeit definiertes Feature kann nicht in ein zur Compile-Zeit geschlossenes Feld-Set passen. Features müssen dynamisch behandelt werden.

**Befund — die Modifier-Felder sind bereits reine `from_features`-Plumbing.** Die Feldkommentare im Struct sagen es wörtlich („when non-zero, `from_features` wraps this feature's leaf in `CsgNode::Twist`/`Bend`/`Displace`/`Shell`"). In `csg.rs::from_features` (≈667–697) werden genau diese Skalare unmittelbar in **kompositionelle `CsgNode`-Wrapper** entpackt — `Twist(leaf, rate)`, `Bend`, `Displace`, `Shell`, und die Junction-Blends `SmoothUnion`/`SmoothDifference(blend_radius, mode)`. Die Felder sind **write-once-read-once**: von `execute` geschrieben, ~30 Zeilen später von `from_features` gelesen, danach nie wieder. Der einzige zweite Leser ist der Geometrie-Hash in `body.rs:291–298` (Cache-Key), der exakt dieselben Felder hasht.

**Schlüssel-Einsicht — die offene IR existiert schon.** `CsgNode`/`BoundedCsg` ist bereits der dynamische, kompositionelle Baum (`Leaf`, `Union`/`Difference`/`Intersection`, `SmoothUnion`/`SmoothDifference`, `Twist`, `Bend`, `Displace`, `Shell`, `Morph`, `MaskedBlend`, `Extrude`, `Revolve`) **mit Provenienz** (jeder `Leaf` trägt eine `BranchId`/`FeatureId`, csg.rs:32/495). Der GPU-Compiler (`sdf_compiler::compile_body`) konsumiert `BoundedCsg`, **nicht** `Feature`; CPU-Eval, B-Rep und STEP hängen alle hinter `from_features`. Modifiers sind also bereits Knoten — der Fat-Struct ist eine redundante denormalisierte Zwischenstufe davor.

**Entscheidung — `Feature` zu einem dünnen Envelope um einen offenen `CsgNode` reduzieren.** Kein Feld-Union mehr. Die dekodierte Feature-Repräsentation wird:

```rust
pub struct DecodedFeature {     // ersetzt das Fat-`Feature`
    pub id: FeatureId,
    pub name: String,
    pub op: BoolOp,             // Combine-mit-Akkumulator (Partitioner-Granularität)
    pub blend: BlendSpec,       // blend_radius/chamfer → generische Junction-Spec
    pub node: CsgNode,          // ← OFFEN: primitive + transform + alle Modifier als Knoten
    pub target_ids: Vec<FeatureId>,
}
```

Alles **feature-spezifische** wandert in den offenen `node`; auf dem Envelope bleibt nur, was **jedes** Feature hat (Identität, Combine-Semantik, Targeting). `op`/`blend`/`target_ids` bleiben oben, weil der **Partitioner** (Body-Zuordnung) auf Feature- nicht Knoten-Granularität arbeitet.

**Konsequenz für die Ausführung.** `FeatureKind::execute` (Rust-Built-ins **und** FieldSkript-Interpreter) emittiert künftig direkt einen `node`-Subtree (gemeinsamer Helper `build_leaf_node(primitive, transform, modifiers)` zieht die heutige `from_features`-Wrapping-Logik an den Emit-Ort). `from_features` schrumpft zum trivialen Fold (`f.node` per `f.op`/`blend`/Targeting kombinieren). Ein FieldSkript-Feature emittiert einen **beliebigen** Subtree → vollständig dynamisch, der zentrale Struct muss für neue Feature-Typen **nie** wieder geändert werden. Das ist exakt das FeatureScript-Modell: Features sind node-emittierende Funktionen über einen Context, kein geschlossenes Datenschema.

**Stufenplan (test-getrieben, jede Stufe grün).**
1. **A — Konsolidierung (verhaltensneutral):** `build_leaf_node`-Helper extrahieren, der die Per-Leaf-Wrapping-Logik aus `from_features` kapselt. Keine Signaturänderung, reiner Refactor.
2. **B — Envelope-Umbau:** `Feature` → `DecodedFeature{node}`, Modifier-Felder löschen; `execute` füllt `node` über den Helper; `from_features` wird Trivial-Fold; `body.rs`-Hash hasht den `node`-Subtree statt Einzelfelder; STEP/Naming-Reads von `.primitive`/`.transform` lesen aus `node.Leaf`. Mechanisch, aber groß (~141 Reads, 7 Dateien). `FeatureCall` (Wire-Form) bleibt unberührt.
3. **C — FieldSkript (später):** Feature-Typ, dessen `execute` den `node` aus Skriptquelle baut.

**Korrektur einer früheren Einschätzung.** Die Aussage „`Feature` ist load-bearing und nicht entfernbar" galt nur **für die heutige Pipeline-Form**. Sie wird nicht ersatzlos gelöscht, sondern in den bereits existierenden offenen `CsgNode` **aufgelöst** — die korrekte, durable Antwort auf die FieldSkript-Anforderung.

**Stufe A erledigt** (Commit `d717258`, 2026-06-01): `CsgNode::build_leaf_node(&Feature) -> (CsgNode, f32, BlendMode)` aus `from_features` extrahiert, verhaltensneutral, `cargo test --workspace` grün.

#### DL-61 Nachtrag — Befund aus dem Scope-Mapping (2026-06-01): Stufe B ist ein **Cross-Crate-Umzug**, kein Feld-Tausch

Das Scope-Mapping vor Stufe B förderte eine harte Randbedingung zutage, die der ursprüngliche Stufenplan übersah:

- **Crate-Grenze.** `Feature`, `Document`, `Primitive` liegen in **protocol**; `CsgNode` in **kernel**; **protocol hat keine kernel-Abhängigkeit** (Richtung kernel→protocol). Ein `node: CsgNode`-Feld in einem protocol-`Feature` ist damit **physisch unmöglich**. Der dekodierte, node-tragende Typ **muss in kernel leben**.
- **`Document.features` ist tief verdrahtet:** `#[serde(skip_serializing)]` (reine Laufzeit, Persistenz safe ✅), aber **192 kernel-Zugriffe**, ~20 wasm-Mutatoren (`setFeatureTranslate/Op/Transform`, `nudgeFeature`, …), GPU-Packing (`pack_feature`/`pack_features` lesen Felder direkt), web-Getter (`listFeatures`→Outliner/Overlays), und der **Reverse-Sync** `from_spine_feature` (protocol:766–974), der jeden Modifier liest, um `FeatureCall`-Params zu rekonstruieren. Die spine-first-wasm-Mutatoren sind genau die Treiber dieses Reverse-Syncs.

**Reframing — was *muss* dynamisch werden:** nicht das protocol-`Feature` (ein statisch-bekannter Built-in-Dekodier-Puffer ist akzeptabel), sondern der **kernel-seitige dekodierte Spine-Eintrag**. Dort konvergieren Built-in und FieldSkript: ein Feature-Beitrag = `{op, blend, node: CsgNode, target_ids}`. Ein FieldSkript emittiert diesen Eintrag **direkt** (es hat kein `primitive`-Feld); ein Built-in baut ihn via `build_leaf_node`. Damit gehört `DecodedFeature` nach **kernel**.

**Revidierte Stufung (Entscheidung steht beim User — siehe unten):**
- **B-inkrementell (empfohlen):** kernel-seitigen `DecodedFeature{op, blend, node, target_ids, id, name}` einführen, von `regenerate`/`build_leaf_node` erzeugt; die **reinen Konsumenten** (Mesher-Hash `body.rs`, STEP `step_classifier`, `sdf_compiler`, GPU-Pack) auf `DecodedFeature` umstellen. protocol-`Feature` bleibt vorerst als Built-in-Dekodier-Puffer + Mutations-Puffer. **Entkoppelt die offene IR von der wasm-Mutations-API und von der Persistenz** → mittleres, sicheres Inkrement, das FieldSkript bereits freischaltet.
- **B-voll (Folge-Arbeit):** spine-first-wasm-Mutatoren auf **call-first** (editiere `feature_calls`, regeneriere), Reverse-Sync `from_spine_feature`/`sync_feature_calls` löschen, `Document.features` ganz entfernen (Document trägt nur noch `feature_calls`). Das ist die saubere Endform, aber groß und berührt die wasm-API.
- **C — FieldSkript-Feature-Typ** (wie gehabt, später).
