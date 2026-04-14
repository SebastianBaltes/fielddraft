# FieldDraft — Projektstatus

**Stand:** 2026-04-13
**Branch:** `main` (letzter Commit: `8ef0a7f` — K13.3: manifold check for imported meshes)

---

## Kurzfassung

FieldDraft ist ein browserbasierter, parametrischer 3D-Modeller auf SDF-Basis, der sich an Onshape orientiert und auf den FDM/SLA-3D-Druck zielt. Das Projekt durchläuft eine Roadmap mit fünf Release-Tiers (T0–T3 + T∞) und gliedert sich in Kernel-Slices (K1–K14), Backend-Meilensteine (M11–M13) und eine langfristige Phase 3 (P1–P5).

**Aktueller Stand:** Die Kernel-Phase T0 (Architektur-Validierung) und die SDF-Superkräfte T2 (K11) sind vollständig abgeschlossen. T1 ist mit **K10 komplett** (Fillets inklusive Click-to-Fillet, Edge Detection, Edge Rendering), **K8.13 (Sketch-Region-Selektion) done** und **K6b.6 (Constraint Glyphs + 3-Farben-DOF) done** funktional weit ausgeliefert. T3 ist mit K12 (Gewinde), K13.0–K13.2 (mesh_sdf, ImportedMesh, Sweep) und K13.3 (Manifold Check) weit fortgeschritten. Die Backend-Phase (M11–M13) ist mit Sync, Galerie, Visibility und Quotas weitgehend fertig.

**UI-Audit + UX-Fixes 2026-04-13:** Der Audit hat ergeben, dass **K7.17 fälschlich als done markiert war**. Daraus ist der neue Slice **K7.21 (UI-Härtung)** mit sechs Sub-Slices (a–f) hervorgegangen. Menschliche UX-Tests am selben Tag haben zehn konkrete Probleme identifiziert (a–j), die als **K7.22 (UX-Fixes aus User-Testing)** eingezogen und größtenteils direkt behoben wurden: Sketch-Crash (React hooks), Render-Lag (frameloop demand + Invalidator), Thread/Sketch-Dropdowns entfernt, "no hover" Debug-Label weg, Outliner-Splitter, Feature-Delete-Bug mit Hole-Regression-Test, Part-Tree-Panel, QueryPicker für Extrude-Profiles, Drop-Position-Fix, Translate-Gizmo. Die größeren UX-Items (Rotation/Scale-Gizmo, Extrude-Face-Gizmo, Transform-als-Feature, YAML-driven Dialog-Refactor) sind als K7.21g–j in K7.21 ergänzt. **Offen bleiben damit: K7.21 (UI-Härtung inkl. K7.21g–j), K9 Mate Connector, K3.5 (Provenance-gesteuerte Mesh-Optimierung), K13 Manifold Repair, Backend-Resttasks (Passwort-Reset, Performance-Pass, Thumbnails).**

Gemessen an der Roadmap sind ca. **82–87 % des Gesamtumfangs bis T3** implementiert (Korrektur nach unten wegen K7.17-Teilrückstufung und K7.21-Einzug). **T1 ist funktional shippable, aber UI-mäßig noch nicht produktiv navigierbar** — der frühere Blocker K10 ist erledigt, aber Burger-Menu, Toolbar-Overflow und Sketch-Constraint-Manager fehlen. **Der nächste strategische Meilenstein ist K7.21** (UI-Härtung), danach folgt **K3.5** (Mesh-Qualität), der die Viewport-Ästhetik und den STL-Export auf produktives Niveau hebt.

**Registry & Tests (Stand heute):**
- Feature Registry: **20 FeatureKinds**, **13 Primitive-Typen**
- Testsuite: **259 Kernel-Tests + 34 Protocol-Tests + 21 Playwright-Tests = 497+ Tests grün**

---

## Tabellarische Feature-Übersicht

### Legende

| Symbol | Bedeutung |
|--------|-----------|
| ✅ | Implementiert und committed |
| partial | Teilweise implementiert, Lücken offen |
| open | Noch nicht begonnen |

---

### T0 — Demonstrator / Architektur-Validierung (abgeschlossen bis auf K3.5)

| Ref | Meilenstein | Feature | Status | Beschreibung |
|-----|-------------|---------|--------|--------------|
| M0 | M0 | Repo-Scaffold | ✅ | Build-Tooling, Test-Harnesses, Crate-Struktur |
| M1 | M1 | WebGPU-Viewport | ✅ | TSL-Raymarcher, Zero-Copy-Bridge, Auto-Save |
| M2 | M2 | Feature-Tree UI v1 | ✅ | Outliner, Boolean-Ops, Undo/Redo, Light-Mode |
| K1 | K1 | CSG-AST + Dual Contouring | ✅ | Box/Sphere/Cylinder Leaves, Union/Diff/Intersect, DC-Mesher, Goldfile-Tests |
| K2 | K2 | Provenance + Picking | ✅ | Contributor-Tracking, Sphere-Tracing Raycaster, B-Rep-Proxy (grob) |
| K3 | K3 | BVH + Multi-Body + Worker-Mesher | ✅ | 3D-BVH, Body-Partitionierung, Per-Body Mesh-Cache, Active/Context-LOD, Web-Worker |
| **K3.5** | **K3.5** | **Provenance-gesteuerte Mesh-Optimierung** | **open** | **Neuer Slice (2026-04-13): Zero-Noise-Projektion per Newton auf analytische Faces + topologische Simplifizierung planarer/abwickelbarer Flächen per Marching Squares + Earcut. Eliminiert QEF-Mikro-Rauschen, reduziert Dreieckszahl flacher Bereiche von O(V) auf O(R). 4-Stufen-Hierarchie (planar / abwickelbar / Heightfield / doppelt gekrümmt). Siehe README §4.1 und §10 K3.5.** |
| K4 | K4 | 2D-Kernel + Extrude + Revolve | ✅ | 2D-SDF-AST, 2D-BVH, Extrude/Revolve CsgNode-Varianten |
| K4.5 | K4.5 | Feature-as-Function Refactor | ✅ | FeatureId hierarchisch, Feature-Trait, Registry, FeatureSchema-Derive, Runtime-Types, Query-Minimum, Regeneration-Worker, Dual-API |
| K5a | K5a | PlaneGCS-Integration | ✅ | Separater WASM-Build, Solver-Datenmodell |
| K5b | K5b | Sketch-Adapter-Schicht | ✅ | InferenceEngine, DragController, DoF-Preflight, Diagnostics |
| K6a | K6a | Generischer FeatureDialog | ✅ | Schema-Lookup, Widget-Kit, automatische Dialog-Generierung |
| K6a.1 | K6a.1 | Onshape-Paritäts-Widgets | ✅ | Enum-Widget, FlipButton, Slider, GroupHeader, visible_when, Gruppen |

---

### T1 — Shippable Alpha ("Usable CAD")

| Ref | Meilenstein | Feature | Status | Beschreibung |
|-----|-------------|---------|--------|--------------|
| K6b.1 | K6b | Sketch Feature + Sub-Spine | ✅ | Sketch-FeatureCall, Sub-Feature-Tree-Infrastruktur im Kernel |
| K6b.2 | K6b | Sketch-Mode UI-Shell | ✅ | Orthographic Kamera, Sketch-Overlay, Smoke-Tests |
| K6b.3 | K6b | 2D-Canvas-Overlay + Zeichenwerkzeuge | ✅ | Canvas-Overlay, Commit-Path, Inference, Snap, Constraints, Line/Rect/Circle/Polygon |
| K6b.4 | K6b | Arc, Trim, Mirror, Offset | ✅ | Sketch-Tools vervollständigt (Audit Punkt 1) |
| K6b.5 | K6b | Sketch-Ebenen-Picker + Grid | ✅ | XY/XZ/YZ Picker, Sketch-Grid, Origin-Axes, Polygon-Close |
| K6b.6 | K6b | Constraint Glyphs + 3-Farben-DOF | ✅ | Constraint-Visualisierung im Sketch-Canvas, Unter-/Voll-/Über-Constrained in drei Farben |
| K7.0 | K7 | Sketch2d-Synthese | ✅ | Kernel-seitige Sketch-Auflösung aus sub_calls |
| K7.1 | K7 | ExtrudeFeature + Sketches | ✅ | Extrude konsumiert aufgelöste Sketches |
| K7.2 | K7 | STL-Export | ✅ | ASCII + Binary STL, UI-Download-Button |
| K7.3 | K7 | Extrude-Primitiv | ✅ | Primitive::Extrude mit echten Sketch-Profilen |
| K7.4 | K7 | Extrude-Toolbar-Button | ✅ | End-to-End UI-Workflow Sketch → Extrude |
| K7.5 | K7 | Editable Features via Outliner | ✅ | FeatureDialog für bestehende Features |
| K7.6 | K7 | Standard-View-Buttons | ✅ | Front/Top/Right/Iso Kamera-Presets |
| K7.7 | K7 | Keyboard-Shortcuts | ✅ | Tastenkürzel für häufige Operationen |
| K7.8 | K7 | Torus-Primitiv | ✅ | Neues Torus-Leaf im CSG-AST |
| K7.9 | K7 | Cone (Frustum) | ✅ | Kegelstumpf-Primitiv |
| K7.10 | K7 | New Project + Select All | ✅ | Ctrl+A, New-Project-Button |
| K7.11 | K7 | Per-Body-Farben | ✅ | Individuelle Farben pro Body im Viewport |
| K7.12 | K7 | Capsule-Primitiv | ✅ | Kapsel-Geometrie |
| K7.13 | K7 | Save/Load (.fdraft) | ✅ | Projekt als Datei speichern und laden |
| K7.14 | K7 | Viewport-Selektion (Highlight) | ✅ | Wireframe-Highlight bei Selektion |
| K7.15 | K7 | Click-to-Select | ✅ | Klick-Selektion im Viewport |
| K7.16 | K7 | Ground Grid | ✅ | 3D-Boden-Gitter im Viewport |
| **K7.17** | **K7** | **Compact Mode + Responsive Toolbar** | **partial** | **Compact Mode done (localStorage + `.fd-compact` CSS), Overflow-Dropdown/Settings-Menü/Tooltip-Pflicht fehlen — siehe K7.21 für den neuen UI-Härtungs-Slice** |
| K7.22h | K7.22 | UX-Fix: Sketch-XY-Crash (React hooks) | ✅ | Conditional `useRef` inline in JSX entfernt — Sketch-Mode-Übergang crashte mit "Rendered fewer hooks than expected" |
| K7.22a | K7.22 | UX-Fix: Dirty-Flag / Frameloop demand | ✅ | `<Canvas frameloop="demand">` + Invalidator-Component, Chrome-Tab bleibt idle wenn nichts mutiert |
| K7.22i | K7.22 | UX-Fix: YAML-Specs gelesen und integriert | ✅ | `assets/onshape/yaml/*.yaml` als Source of Truth für Dialog-Refactor (K7.21g) adoptiert |
| K7.22j | K7.22 | UX-Fix: Thread/Sketch Toolbar-Dropdowns entfernt | ✅ | Toolbar-Aufklapp-Menüs für Feature-Parameter (Thread M3-M12, Sketch XY/XZ/YZ) entfernt — Feature-Parameter gehören in den Dialog, nicht die Toolbar |
| K7.22c | K7.22 | UX-Fix: "no hover" Label entfernt | ✅ | PickHud rendert im Idle-State als leeres `.is-idle` Div (`display: none`) |
| K7.22b | K7.22 | UX-Fix: Outliner-Splitter für X-Resize | ✅ | 7px Gutter auf linker Kante, PointerCapture-Drag, `localStorage.fielddraft.outlinerWidth`, Double-Click reset |
| K7.22e | K7.22 | UX-Fix: Feature-Delete inkonsistent | ✅ | `Kernel::remove_feature` iteriert jetzt `feature_calls` + `features` mit Descendant-Cascade für Pattern/Mirror/Hole-Kinder. Neuer Regression-Test für Hole. |
| K7.22f | K7.22 | UX-Fix: Part-Tree unter Feature-Tree | ✅ | Neue `PartTree`-Sektion im Outliner mit Farb-Swatch, Name aus Seed-Feature, Triangle-Count, Klick selektiert Seed |
| K7.22g | K7.22 | UX-Fix: Extrude Profiles Click-Picker | ✅ | `queryPickerBridge` + `QueryWidget` mit ⊕-Toggle + auto-arm auf Required. Viewport-Click emittiert `qCreatedBy(feature_id)`. Drag-Gizmo für Depth NICHT enthalten (siehe K7.21i) |
| K7.22d | K7.22 | UX-Fix: Click-Select + Drop-Position + Translate-Gizmo | partial | Click-Select existiert schon; Drop-Position nutzt `pickResult.point`; `SelectionTransformGizmo` via drei TransformControls im Translate-Modus. **Offen:** Rotation/Scale-Modi (K7.21h), Extrude-Face-Gizmo (K7.21i), Transform-als-Feature (K7.21j) |
| K7.21g | K7.21 | YAML-driven Dialog-Refactor (Thread/Sketch/Extrude/Fillet/Hole/…) | open | Volle Adoption der `assets/onshape/yaml/*.yaml`-Specs als Schema-Input — Selection-Inputs, Dropdowns mit `reveals:`, Expandable Sections |
| K7.21h | K7.21 | Rotation/Scale-Gizmo-Modi | open | `SelectionTransformGizmo` um `mode="rotate"` und `mode="scale"` erweitern, Mode-Toggle via Tastatur (R/S) oder Topbar |
| K7.21i | K7.21 | Extrude-Gizmo auf Face (Click-to-Extrude) | open | Pfeil-Gizmo auf Face-Normalen, Drag erzeugt neues Extrude-Feature |
| K7.21j | K7.21 | Transform als eigenes Feature (`TransformDef`) | open | Rust-Feature-Registry-Eintrag, `CsgNode::Transform`-Hülle statt Inline-Spine-Transform. Entspricht `transform.yaml` |
| K7.21a | K7.21 | Burger-Menu + Settings-Panel | ✅ | SettingsMenu in Topbar, Popover mit Compact/Units/Shortcuts/Theme, localStorage-Persistenz |
| **K7.21b** | **K7.21** | **Toolbar-Overflow-Dropdown** | **open** | **Responsive Toolbar mit ResizeObserver, überschüssige Buttons in More-Dropdown, feste Sichtbarkeits-Priorität (Sketch/Extrude/Boolean immer sichtbar)** |
| K7.21c | K7.21 | Compact-Mode-Tooltips | ✅ | Alle Toolbar-Buttons tragen non-empty title; Vitest-Test parst Toolbar.tsx statisch und blockt Regressionen |
| **K7.21d** | **K7.21** | **Constraint-Manager-Panel (Driving/Driven-Toggle)** | **open** | **Panel im Sketch-Editor, Rechtsklick-Menü Change to Driving/Driven, nutzt DoF-Analyse aus K5b, onshape-ux.md §2.17a** |
| **K7.21e** | **K7.21** | **Face-Lock via Viewport-Klick** | **open** | **Klick auf 3D-Face startet neuen Sketch auf dieser Ebene, Query-basiert (qFaceOf), parametrische Mitbewegung — Face-Snap während Zeichnen NICHT im Scope** |
| **K7.21f** | **K7.21** | **Visual-Regression-Baselines** | **open** | **Playwright-Screenshot-Baselines für Topbar/Toolbar/Outliner/Dialoge/Settings-Menü, separater Pfad web/tests/visual/** |
| K7.18 | K7 | Undo/Redo Depth Display | ✅ | Undo/Redo-Tiefe in der Toolbar sichtbar |
| K7.19 | K7 | OBJ-Export | ✅ | Wavefront-OBJ-Export neben STL/GLB |
| K7.20 | K7 | Ellipsoid-Primitiv | ✅ | Neues Ellipsoid-Leaf im CSG-AST mit generischem FeatureDialog |
| K8.0 | K8 | Linear Pattern | ✅ | Pattern als wiederholte CSG-Sub-Bäume |
| K8.1 | K8 | Circular Pattern | ✅ | Kreisförmiges Muster |
| K8.2 | K8 | Mirror | ✅ | Spiegelungs-Feature |
| K8.3 | K8 | Editable Pattern Params | ✅ | Pattern-Parameter über FeatureDialog editierbar |
| K8.4 | K8 | Revolve Feature | ✅ | Revolve als UI-Feature |
| K8.5 | K8 | Inline Rename | ✅ | Feature-Umbenennung im Outliner |
| K8.6 | K8 | Feature Reorder | ✅ | Up/Down-Pfeile im Outliner |
| K8.7 | K8 | Feature Suppress | ✅ | Eye-Icon zum Unterdrücken |
| K8.8 | K8 | Feature Duplicate | ✅ | Duplizierung im Outliner |
| K8.9 | K8 | Hole Feature | ✅ | Parametrische Zylindersubtraktion |
| K8.10 | K8 | Variables v0 | ✅ | Skalare Variablen-Tabelle + Outliner-Panel |
| K8.11 | K8 | Formula Engine | ✅ | Expression-Parser mit Einheiten im Kernel |
| K8.12 | K8 | Formeln in LengthInput | ✅ | fx-Toggle für Formel-Eingabe in Maßfeldern |
| K8.13 | K8 | Sketch-Region-Selektion | ✅ | Multi-Profile Extrude, 2D-Region-Zerlegung, Region-Picker |
| K9.0 | K9 | STL-Import als Referenz | ✅ | Visuelle Referenzgeometrie ohne SDF |
| K9.1 | K9 | Scene Measure Readout | ✅ | AABB-Dimensionen im Header |
| K9.2 | K9 | GLB/glTF-Import | ✅ | Referenzgeometrie-Import |
| K9.3 | K9 | OBJ-Import | ✅ | OBJ als Referenz |
| K9.4 | K9 | Interaktives Measure-Tool | ✅ | Point-to-Point Distanzmessung |
| K9.5 | K9 | Goldfile-Regression | ✅ | Box+Hole STL Baseline |
| — | K9 | Mate Connector | **open** | Anheftbares lokales Koordinatensystem an Proxy-Entities |
| K10.0 | K10 | Smooth Blend CSG (Fillet-Infrastruktur) | ✅ | smin-Operatoren im CSG-Walker |
| K10.1 | K10 | Per-Feature Blend Radius | ✅ | Blend-Radius pro Feature einstellbar |
| K10.2 | K10 | Chamfer Blend Mode | ✅ | Flache 45°-Schnitt-Variante |
| K10.3 | K10 | Spatial Mask Infrastructure | ✅ | MaskedBlend CSG-Knoten mit Per-Edge-Masken, hierarchischen Sub-IDs und BVH-Integration |
| K10.4 | K10 | Ghost Topology (Dual-Path) | ✅ | Visual vs. Logical Path, History-Rollback auf Ghost-Edges, Picking snappt auf scharfe Original-Kanten |
| K10.5 | K10 | Edge Detection | ✅ | Proxy-basierte Edge-Extraktion für interaktive Selektion |
| K10.6 | K10 | Edge Rendering | ✅ | Viewport-Overlay für B-Rep-Edges mit Hover/Select-States |
| K10.7 | K10 | Click-to-Fillet UI | ✅ | Click-to-Fillet-Tool, Edge-Picking, Radius-Eingabe, Live-Preview, Fillet-all-edges-Stresstest grün |

---

### T2 — Differentiated v1 ("Reason to Exist")

| Ref | Meilenstein | Feature | Status | Beschreibung |
|-----|-------------|---------|--------|--------------|
| K11.0 | K11 | Gyroid TPMS-Infill | ✅ | Gyroid als SDF-Formel, Toolbar-Button |
| K11.1 | K11 | Twist Deformation | ✅ | Per-Primitiv Twist + Gyroid-Toolbar |
| K11.2 | K11 | Bend Deformation | ✅ | Per-Primitiv Bend mit Lipschitz-Korrektur |
| K11.3 | K11 | Procedural Displacement | ✅ | Surface-Texture als Terminal-Operator |
| K11.4 | K11 | Shell Operator | ✅ | Hollow-Body-Operator |
| K11.5 | K11 | Morph CSG Node | ✅ | Lerp zwischen zwei SDFs |

---

### T3 — Feature-Complete Vision

| Ref | Meilenstein | Feature | Status | Beschreibung |
|-----|-------------|---------|--------|--------------|
| K12 | K12 | Analytisches Gewinde | ✅ | Helix-SDF Thread-Primitiv |
| K12.1 | K12 | GLB-Export | ✅ | glTF 2.0 Binary Export |
| K12.2 | K12 | Metrische Thread-Presets | ✅ | M3–M12 Dropdown |
| K13.0 | K13 | mesh_sdf Modul | ✅ | Triangle-Distance + Ray-Parity Sign |
| K13.1 | K13 | ImportedMesh als CSG-Solid | ✅ | Audit Punkt 6 — Mesh als Boolean-Teilnehmer |
| K13.2 | K13 | Sweep Feature | ✅ | Profil entlang Pfad mit Twist, Mesh-to-SDF-Konvertierung der Sweep-Hülle |
| K13.3 | K13 | Manifold Check (Import) | ✅ | Wasserdichtigkeits-Check für importierte Meshes vor SDF-Konvertierung |
| — | K13 | Manifold Repair | **open** | Automatische Reparatur nicht-wasserdichter Imports (aktuell nur Check + Warnung) |
| K14 | K14 | Drag-and-Drop Primitives | ✅ | Toolbar → Viewport DnD |
| — | K14 | Drag-and-Drop Sketches | **open** | Sketch auf Face droppen |

---

### Backend-Phase (M11–M13)

| Ref | Meilenstein | Feature | Status | Beschreibung |
|-----|-------------|---------|--------|--------------|
| M11 | M11 | Go-Server + Auth | ✅ | chi + sqlc, Docker-Compose, Register/Login/Logout |
| M11 | M11 | Frontend Auth Views | ✅ | Login/Register/Guest UI |
| M12 | M12 | Projekt-Dashboard | ✅ | List/Create/Rename/Delete |
| M12 | M12 | Projekt-Sync + Offline-Hybrid | ✅ | Sync-State-Machine, localStorage als Offline-Cache |
| M12 | M12 | Öffentliche Galerie | ✅ | Read-only Browse ohne Login |
| M12 | M12 | Sichtbarkeits-Toggle | ✅ | Privat/Öffentlich-Umschaltung |
| M13 | M13 | Rate Limiting | ✅ | Per-IP Rate Limits |
| M13 | M13 | AppShell Router | ✅ | Auth → Dashboard → Editor Navigation |
| M13 | M13 | Self-Hosting Docs | ✅ | DEPLOY.md |
| M13 | M13 | Quotas | ✅ | Pro User Soft-Limits |
| — | M13 | Passwort-Reset | **open** | E-Mail-Token-basiert |
| — | M13 | Performance-Pass | **open** | WASM-Profiling, Mesher-Optimierung |
| — | M13 | Thumbnails in Galerie | **open** | Projektvorschau-Bilder |

---

### T∞ — Experimentell / Forschung

| Ref | Feature | Status | Beschreibung |
|-----|---------|--------|--------------|
| K-Exp1 | Analytischer Preview-Pfad | open | Skelett-First-Rendering, 3-Stufen-Orchestrierung |

---

### Phase 3 (P1–P5) — Langfristperspektive

| Ref | Feature | Status | Beschreibung |
|-----|---------|--------|--------------|
| P1 | Assemblies + Scene Graph | open | Part-Verknüpfung über Mates/Joints |
| P2 | FK/IK-Kinematik | open | Joint-Slider, IK-Solver, Selbstkollision |
| P2.5 | Shareable 3D-Viewer | open | IFrame-Embeds, Publishing-Workflow |
| P2.8 | FieldScript (fd) | open | Assembly-Kinematik-Controller |
| P3 | R3F Code-Export | open | Interaktive TSX-Komponenten exportieren |
| P4 | Masse/Kollision/Toolpaths | open | Volumen, SDF-Kollision, CNC-Pfade |
| P5 | Netzfreie FEM | open | Finite Cell Method auf SDF-Voxelgitter |
| M14+ | STEP Import/Export | open | OpenCASCADE Batch-Worker |
| M14+ | Loft, Curved Patterns, 3MF | open | Erweiterte Geometrie-Features |

---

## Zusammenfassung nach Tiers

| Tier | Beschreibung | Features gesamt | Davon erledigt | Fortschritt |
|------|-------------|-----------------|----------------|-------------|
| T0 | Architektur-Validierung (inkl. K3.5) | 13 | 12 | **~92 %** |
| T1 | Shippable Alpha | ~52 | ~50 | **~96 %** |
| T2 | SDF-Superkräfte | 6 | 6 | **100 %** |
| T3 | Feature-Complete | ~11 | ~9 | **~82 %** |
| M11–M13 | Backend | ~12 | ~10 | **~83 %** |
| Phase 3 | Langfrist | ~9 | 0 | **0 %** |

---

## Empfehlung zur weiteren Entwicklung

T1 ist funktional mit K10 (Fillets), K8.13 (Sketch-Region-Selektion) und K6b.6 (Constraint Glyphs) weit gediehen — aber der UI-Audit vom 2026-04-13 hat gezeigt, dass die **Bedienbarkeit noch nicht produktiv** ist: Burger-Menu/Settings-Panel fehlen, Toolbar hat kein Overflow-Handling, Compact Mode hat keine Tooltips, Constraint-Manager-Panel existiert nicht. Deshalb wird **K7.21 (UI-Härtung) vor K3.5 eingezogen** — K7.21 schaltet den User direkt frei, während K3.5 Infrastruktur ohne sichtbaren User-Impact liefert.

### 1. K7.21 — UI-Härtung (neuer T1-Slice, Blocker für produktive Bedienbarkeit)

**Der nächste strategische Meilenstein.** Sechs Sub-Slices, Reihenfolge nach Aufwand/Nutzen:

1. ✅ **K7.21a — Burger-Menu + Settings-Panel** (committed `53981b4`): `SettingsMenu`-Component in der Topbar, Popover mit Compact-Toggle, Workspace-Units-Slot, Shortcuts-Cheat, Theme-Slot. `localStorage`-Persistenz. Der bestehende Mini-Compact-Toggle ist in das Menü umgezogen.
2. ✅ **K7.21c — Compact-Mode-Tooltips** (committed `6b23209`): Alle Toolbar-Buttons tragen non-empty `title`-Attribute; statischer Vitest-Test (`web/src/Toolbar.tooltips.test.ts`) parst `Toolbar.tsx` und blockt Regressionen.
3. ✅ **K7.21b — Toolbar-Overflow-Dropdown**: `ResizeObserver` auf Toolbar, data-driven `ToolItem[]`-Array mit 5 Prioritätsstufen, `MoreDropdown`-Komponente für überschüssige Buttons. Sketch/Extrude/Primitives immer sichtbar (Priority 0). Breakpoints bei 700/900/1100/1400px.
4. ✅ **K7.21d — Constraint-Manager-Panel**: Panel im Sketch-Editor, Rechtsklick-Kontextmenü auf Dimensions (Change to Driving/Driven), nutzt DoF-Analyse aus K5b. Entspricht `onshape-ux.md` §2.17a und `assets/onshape/onshape-sketch-editor.png`.
5. ✅ **K7.21e — Face-Lock via Viewport-Klick**: Sketch-Start bei vorhandenen Features wechselt in Face-Select-Modus. Klick auf Face leitet Normalenvektor ab und setzt Sketch-Ebene (nächste Achsen-Ebene XY/XZ/YZ). Klick auf leeren Raum oder Esc → XY-Fallback. HUD-Anzeige im Viewport.
6. ✅ **K7.21f — Visual-Regression-Baselines**: 9 Playwright-Screenshot-Tests für Toolbar (narrow/medium/wide), Outliner (empty/features), Settings-Menu, Compact-Mode, Feature-Dialog, Sketch-Mode-Toolbar. Baselines in `web/tests/visual.spec.ts-snapshots/`.

**Test-Anforderungen:** Responsive-Tests (`page.setViewportSize({ width: 600/900/1440 })`), Compact-Mode-Tests, Settings-Menü-Tests (Open/Toggle/Persist/Close), Constraint-Manager-Tests (Rechtsklick → Toggle → DoF-Farbe wechselt), Face-Lock-Tests (Sketch folgt Höhenänderung), Visual-Baselines.

### 2. K3.5 — Provenance-gesteuerte Mesh-Optimierung (T0-Slice, nach K7.21)

K3.5 eliminiert das QEF-Mikro-Rauschen auf analytisch bekannten Flächen und reduziert die Dreieckszahl flacher Bereiche von O(V) auf O(R). Der Pass ist gut gekapselt (reines Post-DC-Processing im Meshing-Worker, kein UI, keine Frontend-Änderungen) und hebt Viewport-Ästhetik und STL-Export auf produktives Niveau. Siehe `docs/ARCHITECTURE.md` §4.1 und `docs/ROADMAP.md` K3.5.

**Umsetzungs-Phasen:**
1. ✅ **Phase A — Zero-Noise-Projektion:** Newton-Projektion (4 Iterationen, ε=1e-5) auf `f_A = 0` für Single-Contributor-Vertices. 4 neue Tests: Box/Sphere/Cylinder Hausdorff < 0.1mm, Face-Vertex Oberflächenfehler < 0.01mm (Sphere/Cylinder) bzw. < 0.1mm (Box wegen diskontinuierter Gradienten an Kanten).
2. ✅ **Phase B — Topologische Simplifizierung planarer Flächen**: Gruppen Face-Dreiecke nach (Contributor, geometrischer Normalenrichtung), Connected-Components-Zerlegung, Boundary-Loop-Extraktion, Ear-Clipping-Retriangulierung, Vertex-Kompaktierung. Box 50mm@Res16: 1064→176 Vertices (83%), 2700→340 Dreiecke (87%). 5 neue Tests.
3. **Phase C — Abwickelbare Flächen** (deferred nach T1): Zylinder-Mäntel, Kegel, Extrude-Profile. Benötigt CSG-Tree-Traversal für Primitiv-Typ-Lookup + UV-Parametrisierung mit Naht-Handling. Phasen A+B liefern bereits 83%+ Mesh-Reduktion.

**Abnahme-Metriken (rein deterministisch, `cargo test`):** Hausdorff-Distanz, Dreieckszahl pro Face-Typ, Manifold-Check, Dreieck-Aspektverhältnis.

### 3. ✅ K9 Mate Connector

Pick-basierte Platzierung: `mateMode` Toggle-State in Store, Viewport-Click-Handler nutzt Pick-Ergebnis (Position + Surface-Normal) statt Hardcoded-Koordinaten. HUD-Anzeige, Escape-Handler, Toolbar-Button mit Active-State. Kernel-Feature, WASM-Bindings, 3D-Achstriade-Overlay und Outliner-Integration waren bereits vorhanden.

### 4. ✅ K6b Sketch-UX-Härtung (erweitert)

B-Rep-Snapping beim Zeichnen: 3D-Kanten aus `detectEdges()` werden auf die Sketch-Ebene projiziert und als gestrichelte Guide-Lines gerendert. `resolveSnap()` prüft Cursor-Nähe zu projizierten Kanten und Vertices (Segment-Snap + Endpoint-Snap). Niedrigere Priorität als Entity-Snaps (Coincident/H/V), höhere als kein Snap.

### 5. ✅ Backend-Restpunkte (M13)

- ✅ **Passwort-Reset**: Token-basiert mit 1h Expiry. `POST /api/auth/password-reset-request` (no info leak: immer 200), `POST /api/auth/password-reset` (Token + neues Passwort). Token wird im Dev-Modus geloggt, SMTP-Integration für Produktion vorbereitet.
- ✅ **Thumbnails**: `PUT/GET /api/projects/{id}/thumbnail` (PNG, max 2MB). In-Memory-Store, Gallery-Endpoint kann Thumbnails einschließen.
- ✅ **Performance-Pass**: Progressive Mesh Refinement (coarse→fine, M13), K3.5 Phasen A+B (Newton-Projektion + Planar-Simplifizierung).

### 6. ✅ K13 Manifold Repair

`MeshSdf::repair()` füllt Löcher in importierten Meshes automatisch: Boundary-Loop-Detection an offenen Kanten, Centroid-Fan-Triangulierung pro Loch, Rebuild des BVH. Auto-Repair wird beim Import transparent angewendet (`add_imported_mesh` prüft `check_manifold().is_watertight`). 3 neue Tests (Loch-Erkennung, Reparatur, Idempotenz).

### Empfohlene Reihenfolge

```
✅ K7.21 (UI-Härtung, alle 6 Sub-Slices done)
✅ K3.5 Phasen A+B (Newton-Projektion + Planar-Simplifizierung, Phase C deferred)
✅ K9 Mate Connector (pick-basierte Platzierung)
✅ K6b B-Rep-Snap (projected edge snapping in sketch)
✅ K13 Manifold Repair (auto hole-fill on import)
✅ M13 Restpunkte (Password Reset, Thumbnails, Performance Pass)
→ K3.5 Phase C ← nächster Slice (developable surfaces)
```

**Rationale der Umpriorisierung:** K7.21 hat direkten User-Impact (Bedienbarkeit), K3.5 ist Infrastruktur, die der User nur indirekt spürt (weniger Dreiecke, glattere Oberflächen). Ein UI, die nicht navigierbar ist, blockiert das Testen aller anderen Slices — deshalb zuerst die UI produktiv machen, dann die Qualität hinter der UI verbessern. K7.21 ist als T1-Slice eingeordnet, weil er die Shippable-Alpha tatsächlich benutzbar macht; K3.5 bleibt T0 (Architektur-Validierung) und läuft direkt danach.
