# FieldDraft — Literatur und Referenzen für Implementierungs‑Agenten

> **Diese Datei.** Kuratierte Pflichtlektüre pro Themengebiet. Wer einen K‑Slice anfasst, liest den relevanten Abschnitt hier **vorher**. Der Rest der Dokumentation ist aufgeteilt in `README.md` (Vision), `docs/ARCHITECTURE.md` (§4 + §11), `docs/ROADMAP.md` (§10) und `docs/STATUS.md` (Ist‑Zustand).

Die folgenden Quellen sind kuratiert und decken die zentralen Datenstrukturen und Algorithmen ab. Wer einen K‑Slice anfasst, ohne die zugehörigen Abschnitte gelesen zu haben, baut zwangsläufig in die falsche Richtung.

## §16.1 Dual Contouring

- [Ju, Losasso, Schaefer, Warren — „Dual Contouring of Hermite Data" (SIGGRAPH 2002, PDF)](https://www.cs.rice.edu/~jwarren/papers/dualcontour.pdf) — Originalarbeit, definiert das Verfahren, das im FieldDraft‑Kernel zur Mesh‑Extraktion aus dem SDF‑Feld eingesetzt wird. Pflichtlektüre für alle, die am Vernetzer arbeiten.
- [Schaefer, Ju, Warren — „Manifold Dual Contouring" (TVCG 2007, PDF)](https://people.engr.tamu.edu/schaefer/research/dualsimp_tvcg.pdf) — Erweiterung mit Garantie für mannigfaltige Meshes auch unter adaptiver Vereinfachung; relevant, sobald wir Octree‑Adaption brauchen.
- [swiftcoder/isosurface (Rust crate)](https://github.com/swiftcoder/isosurface) — Wartungsaktive, dependency‑freie Rust‑Implementierung von Marching Cubes und Dual Contouring; gute Referenz für Datenstrukturen und Octree‑Layout.
- [nickgildea/fast_dual_contouring](https://github.com/nickgildea/fast_dual_contouring) — Praxiserprobte Variante ohne Octree, demonstriert eine flache Datenstruktur, die sich gut auf WASM portieren lässt.
- [mapbox/earcut](https://github.com/mapbox/earcut) — Schneller 2D‑Polygon‑Triangulator mit Loch‑Unterstützung, Laufzeit O(R log R). Referenz‑Implementierung für die 2D‑Retriangulierung planarer und abwickelbarer Flächen in **K3.5** (Hybrid‑DC). Die Earcut‑Heuristik ist nicht optimal, liefert aber robust gute Dreiecke und ist in Rust trivial zu portieren.

## §16.2 Quadratic Error Function (QEF) Solver

- [BorisTheBrave — „Dual Contouring Tutorial"](https://www.boristhebrave.com/2018/04/15/dual-contouring-tutorial/) und [zugehöriger QEF‑Code](https://github.com/BorisTheBrave/mc-dc/blob/master/qef.py) — Die kanonische didaktische Quelle. Erklärt QEF‑Aufbau, SVD‑Lösung und Bias‑Term, jeweils mit lauffähigem Python.
- [Lindstrom — „Out‑of‑Core Simplification of Large Polygonal Models" (SIGGRAPH 2000, PDF)](http://evasion.imag.fr/Membres/Franck.Hetroy/Teaching/Geo3D/Articles/lindstrom2000.pdf) — Begründet die SVD‑basierte robuste QEF‑Minimierung, die bis heute Stand der Technik ist.
- [Matt Keeter — „QEF Explainer"](https://www.mattkeeter.com/projects/qef/) und [nickgildea/qef (SVD‑Implementierungen)](https://github.com/nickgildea/qef) — Kompakte numerische Erklärung plus C/GLSL/OpenCL‑Solver, an denen sich eine numerisch stabile Rust‑Variante gut spiegeln lässt.

## §16.3 SDF‑Primitive (3D)

- [Inigo Quilez — „3D Signed Distance Functions"](https://iquilezles.org/articles/distfunctions/) — De‑facto‑Referenz für exakte 3D‑SDF‑Formeln (Box, Zylinder, Kegel, Torus, Kapsel, …). Quelle der Wahrheit für alle Primitive im FieldDraft‑Kernel.
- [Inigo Quilez — „Raymarching Distance Fields"](https://iquilezles.org/articles/raymarchingdf/) — Hintergrund zu Distanzeigenschaften (Lipschitz‑Bound, exakte vs. begrenzende SDFs); wichtig, weil unser BVH und der Picking‑Raymarcher nur mit echten SDFs korrekt funktionieren.
- [Inigo Quilez — „SDF Bounding Volumes"](https://iquilezles.org/articles/sdfbounding/) — Beschreibt das Sketch‑Extrude‑ und Revolve‑Pattern (2D‑Profil → 3D‑Volumen) inklusive Bounding‑Box‑Ableitung, das genau dem FieldDraft‑Feature‑Tree entspricht.

## §16.4 SDF‑Primitive (2D) und Komposition

- [Inigo Quilez — „2D Distance Functions"](https://iquilezles.org/articles/distfunctions2d/) — Vollständige Sammlung exakter 2D‑SDFs (Linie, Kreis, Bogen, Ellipse, Polygon) mit Shadertoy‑Demos. Direkte Vorlage für den 2D‑Sketch‑Solver.
- [Inigo Quilez — „Distance + Gradient 2D SDF"](https://iquilezles.org/articles/distgradfunctions2d/) — Liefert analytische Gradienten zusammen mit der Distanz; entscheidend, damit Hermite‑Daten für Dual Contouring direkt aus dem 2D‑Profil propagiert werden können.

## §16.5 BVH‑Beschleunigung für SDF‑/CSG‑Trees

- [Inigo Quilez — „SDF Bounding Volumes"](https://iquilezles.org/articles/sdfbounding/) — Erklärt, wie man Subtrees mit AABBs ummantelt und Punktanfragen früh abbricht. Direkt anwendbar auf unseren CSG‑Tree.
- [PBR Book — Bounding Volume Hierarchies (3rd ed.)](https://pbr-book.org/3ed-2018/Primitives_and_Intersection_Acceleration/Bounding_Volume_Hierarchies) — Goldstandard für BVH‑Konstruktion (SAH, Traversal). Auch wenn das Buch ray‑zentriert ist, sind Aufbau und Speicher‑Layout 1:1 für Punktanfragen verwertbar.
- [Liktor et al. — „Synchronized‑Tracing of Implicit Surfaces" (arXiv 2304.09673)](https://arxiv.org/pdf/2304.09673) — Modernes Paper über räumliche Pruning‑Strukturen für CSG‑/Blobtree‑Auswertung; gute Referenz für Pruned‑Tree‑Auswertung pro Region.

## §16.6 CSG‑Provenance und Feature‑Labeling

- [Rossignac, Voelcker — „Active Zones in CSG" (ACM TOG 1989)](https://dl.acm.org/doi/abs/10.1145/49155.51123) — Klassiker; definiert „active zones" als räumliche Regionen, in denen ein Primitiv die Boolean‑Auswertung dominiert. Theoretisches Fundament für unser Contributor‑Tracking.
- [Hoffmann — „How Solid Is Solid Modeling?" (PDF)](https://www.cs.purdue.edu/cgvlab/www/resources/papers/Hoffmann-Workshop_on_App_Comp_geo-1996-How_SolidIs_Solid_Modeling.pdf) — Hoffmanns Übersicht zu Boundary Evaluation, Feature‑Persistenz und numerischer Robustheit. Kontextpapier für alle persistenten ID‑Schemata.
- [Matt Keeter — „Massively Parallel Rendering of Complex Closed‑Form Implicit Surfaces" (libfive Kernel‑Paper, PDF)](https://www.mattkeeter.com/research/kernels.pdf) und [libfive Repository](https://github.com/libfive/libfive) — Modernste implizite CAD‑Kernel‑Architektur in der Praxis; libfive trackt Sub‑Expression‑Dominanz pro Region und ist die beste lebende Referenz für FieldDraft.

## §16.7 PlaneGCS (FreeCADs Geometric Constraint Solver)

- [FreeCAD/src/Mod/Sketcher/App/planegcs (GitHub)](https://github.com/FreeCAD/FreeCAD/tree/main/src/Mod/Sketcher/App/planegcs) — Original‑C++‑Quellcode des Solvers als Teil von FreeCAD. Pflichtlektüre für die FFI‑Definition in K5a und für das Verständnis der Constraint‑Primitives, auf die die Adapter‑Schicht mappt (siehe ARCHITECTURE.md §4.4).
- [@salusoft89/planegcs (npm)](https://www.npmjs.com/package/@salusoft89/planegcs) und [salusoft89/planegcs (GitHub)](https://github.com/salusoft89/planegcs) — Emscripten‑WASM‑Build von PlaneGCS mit TypeScript‑Bindings und JSON‑basierter Primitiv‑API. Der Build wiegt ca. 881 kB gepackt und ist der Referenz‑Integrationspfad für die separate‑WASM‑Variante aus ARCHITECTURE.md §4.4. Das Repo enthält auch die begleitende Thesis zur WASM‑Portierung mit Details zu den Solver‑Algorithmen (DogLeg, Levenberg‑Marquardt, BFGS, SQP), zum `temporary`‑Flag‑Mechanismus für Drag‑Stabilität und zur Diagnostik‑API — Pflichtlektüre vor dem Anfassen von K5a.
- **MIT „CAD in 1 Hour"** (im `salusoft89/planegcs`‑Repo verlinkt) — Kompakte Einführung in Sketch‑Solver‑Architektur und Adapter‑Layer‑Patterns; dient als zusätzliche Referenz für die Test‑Szenarien in K5b.
- [solvespace/solvespace (GitHub)](https://github.com/solvespace/solvespace) und [SolveSpace Reference (Westhues)](https://solvespace.com/ref.pl) — Die frühere Solver‑Wahl, aus UX‑Gründen verworfen. Bleibt als historische Referenz und als Quelle für die Standard‑2D‑Constraint‑Semantik interessant.

## §16.8 wgpu (Rust‑WebGPU‑Bindings)

- [wgpu.rs (offizielle Projektseite)](https://wgpu.rs/) und [docs.rs/wgpu](https://docs.rs/wgpu/) — Maßgebliche API‑Dokumentation für den Rust‑WebGPU‑Stack, den der FieldDraft‑Viewport nutzt.
- [sotrh — „Learn Wgpu"](https://sotrh.github.io/learn-wgpu/) — Das De‑facto‑Lehrbuch zur Bibliothek; deckt Pipelines, Bind Groups, Compute‑Shader und WASM‑Targeting ab.
- [W3C WebGPU Specification](https://www.w3.org/TR/webgpu/) und [W3C WGSL Specification](https://www.w3.org/TR/WGSL/) — Normative Quelle für Semantik von Buffers, Bind Groups und Shadern; bei jeder Unklarheit gilt die Spec, nicht der Wrapper.

## §16.9 wasm‑bindgen Zero‑Copy‑Patterns

- [The wasm‑bindgen Guide](https://rustwasm.github.io/docs/wasm-bindgen/) — Offizielle Dokumentation; insbesondere die Abschnitte über `js_sys::Float32Array::view` zeigen, wie man ohne Kopie auf den linearen WASM‑Speicher mappt.
- [rustwasm/wasm‑bindgen Issue #2456 — „need example of sharing memory"](https://github.com/rustwasm/wasm-bindgen/issues/2456) — Konkrete Beispiele zur Lebensdauer‑Verwaltung von Views (Achtung: kein `alloc` während ein View existiert), exakt das Pattern, das FieldDraft für Mesh‑Buffer braucht.

## §16.10 React Three Fiber + BufferGeometry

- [React Three Fiber — „Objects, Properties and Constructor Arguments"](https://r3f.docs.pmnd.rs/api/objects) — Offizielle R3F‑Doku zum `attach="attributes-position"`‑Pattern, mit dem externe Float32Arrays an `<bufferGeometry>` angebunden werden.
- [three.js — BufferGeometry (offizielle Doku)](https://threejs.org/docs/#api/en/core/BufferGeometry) und [Custom BufferGeometry Manual](https://threejs.org/manual/en/custom-buffergeometry.html) — Maßgebliche Beschreibung von Attributen, Indices, Bounding‑Spheres und Update‑Flags. Pflicht, bevor jemand `setAttribute` aufruft.

## §16.11 Sharp‑Feature‑Preservation in Dual Contouring

- [Ju et al. — „Dual Contouring of Hermite Data" (siehe §16.1)](https://www.cs.rice.edu/~jwarren/papers/dualcontour.pdf) — Originaldarstellung der scharfen Kanten über QEF‑Minimierung mit Edge‑Normalen.
- [Schaefer, Warren — „Dual Contouring: The Secret Sauce" (PDF)](https://people.eecs.berkeley.edu/~jrs/meshpapers/SchaeferWarren2.pdf) — Zusatzpapier mit Implementierungsdetails (Bias‑Term, Constraint Box, Feature‑Klassifikation), die im Originalpaper fehlen.
- [Ho et al. — „Cubical Marching Squares" (PDF)](https://www.csie.ntu.edu.tw/~cyy/publications/papers/Ho2005CMS.pdf) — Alternative Sicht auf adaptive Feature‑Erhaltung; nützlich für Heuristiken bei mehreren Kanten in einer Zelle.

## §16.12 Polyline‑Approximation von Splines

- [Cao, Wang — „Approximate Computation of Curves on B‑Spline Surfaces" (PDF)](https://www.cs.cornell.edu/~caosong/index_files/surface_cad08.pdf) — Beschreibt adaptive Subdivision unter Hausdorff‑Toleranz; Grundlage für chord‑error‑getriebenes Sampling.
- [Liutaio Mottola — „Calculating Arc Parameters" (Sagitta‑Formeln)](https://liutaiomottola.com/formulae/sag.htm) — Praktische Formelsammlung zu Sagitta/Versine; direkt verwendbar für Bogen‑zu‑Polyline‑Konvertierung im Sketch‑Modul.
- [Lee Mac — „Bulge Conversion Functions"](https://www.lee-mac.com/bulgeconversion.html) — Klassische Notizen zur Bulge/Sagitta‑Beziehung; nützlich, weil DXF‑Importe Bögen genau in dieser Form transportieren.

## §16.13 Adaptive Heightfield‑Triangulierung

- [Duchaineau et al. — „ROAMing Terrain: Real‑time Optimally Adapting Meshes" (IEEE Vis 1997, PDF)](https://www.llnl.gov/graphics/ROAM/roam.pdf) — Originalarbeit zu Real‑time Optimally Adapting Meshes; die klassische Referenz für adaptive Heightfield‑Triangulierung, die Dreiecke anhand eines lokalen Fehler‑Kriteriums (Displacement‑Gradient, Hausdorff‑Distanz) verfeinert. Grundlage für das Stufe‑3‑Gewinde‑Meshing in K3.5/K12 — der periodische Mittelteil eines metrischen Gewindes wird einmal gemesht und per Index‑Offset wiederverwendet, der Gewindeauslauf wird individuell verfeinert.
- **Restricted Quadtree Triangulation (RQT)** — Verwandte Technik aus der Terrain‑Literatur: Ein Quadtree über dem UV‑Raum wird adaptiv verfeinert, die Triangulierung garantiert per Restriction‑Regel T‑Junction‑Freiheit zwischen benachbarten Zellen unterschiedlicher Tiefe. Einfacher zu implementieren als ROAM, liefert für analytische Heightfields (Gewinde, periodische Displacement‑Profile) vergleichbare Qualität. Praktische Einführung z. B. in Pajarola, „Large Scale Terrain Visualization Using the Restricted Quadtree Triangulation" (IEEE Vis 1998).
