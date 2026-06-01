# Plan: Sketch-projizierte SDF + Segment-IDs + Dual-Numbers → Octree-B-Rep

**Branch:** `feat/sketch-sdf-brep` (von `origin/main` @ 57c8d6f)
**Stand:** Entwurf zur Freigabe — 2026-05-31 (restrukturiert nach DL-46: lokal-vs-global)
**Vorgeschichte:** Backtrack aus dem `agents/fielddraft-a`-Konzept. Zwei Lektionen:
(1) Splines/Bézier haben keinen geschlossenen Abstand (Kubik ⇒ Quintic) → Segmentierung (linear/quadratisch) im Kernel.
(2) Naive Half-Spaces explodieren als SDF (512-Opcode-Cap pro Body + ganzer Baum pro Pixel).

---

## 1. Ziel & tragende Achse: lokal vs. global (DL-46)

FieldDraft-Geometrie ist fast vollständig **Sketch-basiert**: Sketches mit Extrude / Revolve /
Helix / Sweep. Die tragende Architektur-Achse ist **nicht „2D vs. 3D", sondern lokal vs. global +
Änderungsfrequenz**. Daraus folgt eine **Hierarchie**:

- **Lokale Per-Body-Repräsentation (billig, gecacht, seltene Invalidierung).** Die 2D-Sketch-SDF
  samt **2D-BVH** wird einmal pro Sketch gebaut und nur invalidiert, wenn sich der **Sketch** ändert.
  Häufige Operationen (Extrude-Höhe ziehen, Body verschieben/rotieren/skalieren, Echtzeit-Dragging)
  berühren die lokale SDF nicht — die globale SDF entsteht durch **Transformation des Query-Punkts in
  den lokalen Body-Raum**. „Eigener 2D-Raum pro Body" ist ein **Feature** (affine Platzierung ⇒ keine
  BVH-Neuberechnung). Per-Body-B-Rep-Flächen sind **lokal** und analytisch/gecacht ableitbar.
- **Globaler Schritt (teuer, lokalisiert).** Der Octree/Boolean ist **kein** Allzweck-Extraktor über
  die ganze Szene, sondern ein **Interaktions-Löser nur in Body-Überlappungszonen** (der
  Deferred-Renderer kennt sie schon per-Pixel, ≤2 Bodies). Er **konsumiert** die billigen lokalen
  Flächen und behandelt die genuin cross-Body-Phänomene: **Trim-Kurven** scharfer Booleans und
  **smin-Blend-Regionen**.

Das strategische Ziel — **effiziente, getaggte B-Reps** — wird damit zweistufig erreicht: lokal pro
Body (priorisiert, gecacht), global nur dort, wo Bodies interagieren.

## 2. Was auf `main` bereits existiert (kein Aufwand)

| Baustein | Ort | Status |
|---|---|---|
| Sketch2d-CSG-Baum (Circle/Rect/Polygon(even-odd)/Sector) | `crates/protocol/src/sketch2d.rs:81,286` | da |
| **2D-BVH mit AABB-Pruning** (`BoundedSketch2d`) — CPU, cachebar pro Sketch | `crates/kernel/src/sketch2d.rs:22,116` | da (~log N), **ohne Segment-IDs** |
| Extrude = 2D-Projektion + `sdf_extrude` | `crates/kernel/src/csg.rs:908,1197` | da |
| Revolve = `sketch.sdf(√(x²+z²), y)` | `crates/kernel/src/csg.rs:909` | da |
| Sweep (lowert auf Extrude via Rotation) | `crates/kernel/src/features.rs:967` | da, kein echter 3D-Pfad-Sweep |
| Helix/Screw für Sketches | — | **fehlt** (nur `Primitive::Thread`) |
| Deferred-Renderer: Hüllmesh-Cullung, Per-Pixel-Bodyliste (≤2), Z-Intervall | `web/src/sdfDeferredRenderer.ts` | da — liefert die **Überlappungszonen** gratis |
| Mesher (MDC/RFTA/CGAL) — unabhängig | `web/src/mesher.worker.ts` | da |
| GPU-VM (flach, `VM_MAX_OPCODES=512`) | `web/src/sdfVmInterpreter.ts:52` | da |

## 3. Präzisierung der Idee (wichtig)

Die C/D-Extraktion arbeitet auf einem 3D `BoundedCsg` und produziert einen **3D-B-Rep**. Sie ist
generisch über „eine SDF mit getaggten Leaves" (`Contributors`/`BranchId`). Der Brückenschritt ist
**nicht** „separater 2D-B-Rep", sondern die Lokal/Global-Hierarchie:

1. Sketch-SDF liefert pro Query-Punkt zusätzlich die **Segment-ID** (nächstes Segment) + **2D-Gradient**
   (Dual-Numbers). Lokal, gecacht, pro Sketch invalidiert.
2. Die Extrude/Revolve-Auswertung **expandiert** den Sketch-Körper in seine getaggten 3D-Flächen
   (eine Mantelfläche pro Sketch-Segment + Deckel-Ebenen z=0/z=h im **Body-lokalen** Koordinatensystem),
   jede mit einer aus der Segment-ID abgeleiteten `BranchId`; der 3D-Gradient entsteht per Kettenregel
   durch die Projektion **und** die affine Platzierungs-Jacobi (dasselbe Dual-Number-Machinery).
3. **Lokaler Per-Body-B-Rep (Phase 3, priorisiert):** Octree läuft auf **einem** getaggten Body und
   liefert dessen B-Rep (Branch-Kollisionen zwischen Mantelsegmenten + Deckeln ⇒ Kanten/Vertices).
   Ergebnis cachebar, invalidiert nur bei Sketch/Feature-Param-Änderung.
4. **Globaler Interaktions-Schritt (Phase 4):** Octree **nur in Überlappungszonen** zwischen Bodies,
   konsumiert die lokalen Flächen, erzeugt Trim-Kurven + smin-Blend-Flächen.

## 4. Warum der WebGPU-Crash auf `main` weg ist

Die schweren 2D-Kurven-Shader-Handler (`OP_PRIM2D_LINE/QUAD/CUBIC/PATH`) waren eine **B.3a-Ergänzung
nur auf `agents/fielddraft-a`**. `main` nutzt Circle/Rect/Polygon/Sector — kein Per-Pixel-Kurven-Solver,
kein Crash. **Kurven bleiben segmentiert** (heute Polygon; Quadratik-Segmente als Genauigkeits-Upgrade).

---

## 5. Zwei Tracks

- **Track A — Rust-B-Rep (strategisches Ziel, CPU-only, `cargo test`-verifizierbar).**
  Segment-IDs + Dual-Numbers in der Sketch-SDF → C/D portieren → **lokaler Per-Body-B-Rep** →
  **globaler Interaktions-Schritt** → getaggter 3D-B-Rep. **Vom Agenten verifizierbar.**
- **Track B — GPU-Sketch-SDF-Lokalisierung (Render-Skalierung, GPU-only, nur vom User verifizierbar).**
  2D-BVH-Traversierung im VM / Per-Segment-Lokalisierung, damit komplexe Sketches die 512-Decke /
  Pro-Pixel-Kosten nicht sprengen. Auf `main` ist der akute Crash bereits weg ⇒ **nachrangig.**

Track A ist Priorität (Ziel + verifizierbar). Track B ist separat planbar.

---

## 6. Phasenplan (Track A) — Subagenten-tauglich

### Phase 0 — C/D-Substrat auf `main` portieren (Voraussetzung)
**Aufgabe:** Die additiven B-Rep-Module aus `agents/fielddraft-a` als neue Dateien übernehmen und die
chirurgischen `csg.rs`/`bvh.rs`-Ergänzungen gegen mains divergierte `csg.rs` von Hand re-applizieren.
- Additiv (geringes Konfliktrisiko): `crates/kernel/src/brep/{mod,octree,extraction,edge_walker,weaving,naming,projection,step_export}.rs`, `crates/kernel/src/sdf_dual.rs`.
- Chirurgisch: `BranchId`, `Contributors`, `EvalMode`, `expand_evaluate_leaf`, `combine_min/max` in `csg.rs`/`bvh.rs` — reconcilen gegen mains Gyroid-Partitionierung (588df94/533a8ce), Non-uniform-Scale (12d77b5), keep-bodies-separate (338ae54).
- Dep: `num_dual` Crate.
**Output:** B-Rep-Pipeline kompiliert auf `feat/sketch-sdf-brep`.
**Acceptance:** Die portierten B-Rep-Tests grün (`cargo test -p kernel brep`), inkl. Box→B-Rep
(6 Faces, 8 Vertices, 12 Edges), getwisteter Würfel, STEP-Referenz-Auflösung. Restliche Suite grün.
**Risiko:** `csg.rs`-Reconcile. Mitigation: zuerst nur `csg.rs`/`bvh.rs` + ein Minimal-Box-Test, dann Rest.

### Phase 1 — Segment-IDs in der Sketch-SDF (lokales Tagging)
**Aufgabe:** Sketch-Primitive in adressierbare **Segmente** zerlegen und IDs vergeben.
- `Sketch2dPrimitive` so erweitern/auswerten, dass jede Kante (Polygon-Kante, Rechteck-Seite,
  Kreis-Bogen-Approx, Sektor-Kante) eine stabile **lokale Segment-ID** trägt.
- 2D-SDF-Query liefert zusätzlich `nearest_segment_id`. Mehrdeutigkeit auf der **Medial-Achse**
  (gleich weit zu zwei Segmenten) per Tie-Break analog **DL-32** (Achsenreihenfolge).
- `BranchId::from_feature_sub(extrude_feature, segment_id)` als Tag.
- Im 2D-BVH cachebar; invalidiert nur bei Sketch-Änderung.
**Output:** `sketch.sdf_tagged([x,y]) -> (f32, SegmentId)`.
**Acceptance:** Rust-Unit-Tests: nächste-Segment-Klassifikation pro Sektor eines Quadrats/Polygons korrekt; Tie-Break deterministisch.

### Phase 2 — Dual-Numbers in der 2D-SDF + Hebung durch Projektion & Platzierung
**Aufgabe:** 2D-Segmentabstände generisch über `T: DualNum` (analog `sdf_dual.rs` Layer A), und
Kettenregel durch Extrude (`d3 = sdf_extrude(d2, z)`), Revolve (`r=√(x²+z²)`) **und** die affine
Body-Platzierung (Jacobi; nicht-uniform/Scherung via Inverse-Transponierte).
- 2D-Segment-Duals: Linie, Quadratik-Bézier (analytisch, Grad-3-lösbar), Kreisbogen.
- `expand_evaluate_leaf`-Analog für Extrude/Revolve: pro Segment eine getaggte „Mantel"-Auswertung +
  Deckel-Ebenen (z=0/z=h, lokal) + 3D-Gradient.
**Output:** `BoundedCsg::evaluate` liefert für Sketch-Körper `(f32, Contributors)` mit Segment-`BranchId`s; `sdf_gradient` exakt (inkl. affiner Platzierung).
**Acceptance:** Gradient via Dual ≈ finite Differenz (Toleranz, auch unter Rotation+Skalierung); Contributors an einer Extrude-Mantelfläche = genau die eine Segment-ID, an der vertikalen Kante zweier Wände = beide, am Deckelrand = Segment + Deckel.

### Phase 3 — Lokaler Per-Body-B-Rep (priorisiert, cachebar)
**Aufgabe:** Die bestehende Extraktion auf **einen** Extrude/Revolve-Körper laufen lassen → dessen
eigener getaggter B-Rep, ohne Booleans.
- Branch→Surface-Lookup beim Weaving (`weaving.rs`): Segment-`BranchId` → erzeugte 3D-Fläche
  (Extrude-Mantel = ruled surface des Segments; Deckel = Ebene; Revolve = Kegel/Zylinder/Torus-Patch).
- Ergebnis als **Per-Body-Cache** (Key = Body-Identität + Sketch/Feature-Param-Hash), invalidiert nur
  bei Sketch/Feature-Änderung — *nicht* bei affiner Platzierung.
**Output:** getaggter, gecachter 3D-B-Rep pro Sketch-Körper.
**Acceptance:** Extrudiertes Rechteck → 6 Faces / 12 Edges / 8 Vertices; extrudiertes L-Polygon → korrekte Face/Edge-Zahl; revolviertes Profil → erwartete Patches; STEP-Referenz-Auflösung grün; Cache-Hit bei reiner Platzierungsänderung.

### Phase 4 — Globaler Interaktions-Schritt (Booleans/Blends, lokalisiert)
**Aufgabe:** Octree **nur in Body-Überlappungszonen**; konsumiert die lokalen Per-Body-Flächen und
erzeugt die cross-Body-Topologie.
- Überlappungszonen aus Body-AABBs/Hüllen bestimmen (CPU-Analog zur Per-Pixel-≤2-Body-Logik des Renderers).
- **Trim-Kurven** scharfer Booleans (`min`/`max`) + **smin-Blend-Flächen** (neue Fläche aus beiden Feldern).
- Lokale Flächen + globale Trims zu einer wasserdichten Shell verweben.
**Output:** getaggter 3D-B-Rep für eine Mehr-Body-Szene (z. B. zwei sich schneidende Extrudes; ein Body mit smin-Fillet).
**Acceptance:** Zwei orthogonale Extrudes → korrekte Schnittkanten/Vertices; smin-Verrundung → glatte Blend-Fläche mit korrekter Adjazenz; nur die Überlappungszone wird durch den Octree gerechnet (Assertion auf evaluierte Zellen).

### Phase 5 — Naming/Integration + STEP
**Aufgabe:** D.1-Naming (ist generisch ⇒ portierbar) auf Sketch-B-Reps (lokal + global); STEP-Export prüfen.
**Acceptance:** Stabile IDs über Parameteränderung (Rechteck größer ziehen ⇒ Kanten-Signaturen stabil); STEP-Datei referenz-auflösbar.

---

## 7. Entscheidungen (freigegeben 2026-05-31)

1. **Kurven-Genauigkeit:** ✅ Native **Quadratik-Bézier-Segmente** in `Sketch2dPrimitive` (analytischer Abstand, Grad-3-lösbar) in Phase 1/2.
2. **Port-Strategie Phase 0:** ✅ **Additive Dateien + chirurgisches `csg.rs`-Reapply** (kein git-Merge).
3. **Track B (GPU-Lokalisierung):** ✅ **Zurückgestellt**, bis Track A steht.
4. **Helix/Sweep:** ✅ Echter 3D-Pfad-Sweep = **separater Track, später**.

## 8. Subagenten-Schnitt (Vorschlag)

- **SA-0a:** `csg.rs`/`bvh.rs`-Reconcile + Minimal-Box-B-Rep-Test (Gate für alles Weitere).
- **SA-0b:** Restliche `brep/`-Module + `sdf_dual.rs` portieren, Tests grün.
- **SA-1:** Segment-IDs in Sketch-SDF (Rust) + Tie-Break + Tests.
- **SA-2:** 2D-Duals + Projektions-/Platzierungs-Kettenregel + Contributors-Tests.
- **SA-3:** Lokaler Per-Body-B-Rep + Weaving-Brücke Segment→3D-Fläche + Per-Body-Cache + Tests (Extrude/Revolve).
- **SA-4:** Globaler Interaktions-Schritt (Booleans/Blends in Überlappungszonen) + Tests.
- **SA-5:** Naming + STEP-Validierung.
Sequenziell mit Gates (jede Phase `cargo test`-grün, vom Hauptagenten kontrolliert). SA-0a/0b und SA-1
können teils parallel; SA-2→3→4→5 hängen voneinander ab.

## 9. Risiken

- **Phase 0 `csg.rs`-Reconcile** (divergierte main). → klein halten, Box-Test als Gate.
- **Medial-Achsen-Mehrdeutigkeit** der Segment-ID. → DL-32-Tie-Break, ist bewährt.
- **Revolve-Selbstüberschneidung** an der Achse (Profil muss x≥0). → Vorbedingung prüfen/erzwingen.
- **Ruled-Surface des Extrude-Kurvensegments** im Weaving als analytische Fläche. → für Linien-/Quadratik-Segmente handhabbar; Genauigkeit folgt der Segmentierung.
- **smin-Blend als B-Rep-Fläche** (Phase 4): keine getrimmte Vereinigung, neue Fläche aus zwei Feldern. → Octree-extrahiert in der Blend-Zone; Genauigkeit folgt der Octree-Auflösung.
- **Überlappungszonen-Bestimmung** (Phase 4): AABB/Hülle muss konservativ genug sein, sonst verpasste Trims. → konservativ inflaten, wie `HULL_INFLATE` im Renderer.
- **GPU (Track B)** kann der Agent nicht verifizieren — bleibt User-Test.
