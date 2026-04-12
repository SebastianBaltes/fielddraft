# FieldDraft — Entwicklungsstand (2026-04-12)

## Rohdaten

- **Erster Commit**: 2026-04-09 17:14
- **Letzter Commit**: 2026-04-12 ~12:00
- **100 Commits** in ~67 Stunden Wallclock (3 Arbeitstage)
- **Aktive Pair-Coding-Zeit**: ~30–35 h

## Feature-Umfang

### Primitive (12 Typen)
Box, Sphere, Cylinder, Capsule, HexPrism, Cone, Torus, Thread,
Extrude, Revolve, Hole, Gyroid

### SDF-Modifikatoren (8 pro Feature)
Blend radius (Fillet), Chamfer size, Twist rate, Bend rate,
Displace depth, Displace freq, Shell thickness, Boolean Op

### Pattern-Typen (3)
Linear, Circular, Mirror

### Sketch-Tools (5)
Point, Line, Rectangle, Circle, Polygon (mit close-on-first-vertex)

### Feature-Operationen
Rename, Reorder (up/down), Suppress, Duplicate, Delete

### Export
STL (binary), GLB (glTF 2.0)

### Import
STL als Referenz, GLB/glTF als Referenz

### Parametrics
Document-level Variables, Formula Engine (=width/2 + 5),
LengthInput mit fx-Toggle

### Viewport
Per-Body Colors (8-Farben-Palette), Selection Wireframe,
Click-to-Select, Ground Grid, Standard Views (Front/Top/Right/Iso)

### Sketch Editor
Adaptive Grid + Origin Axes, Interactive Polygon Drawing,
Constraint Solver (H/V/Coincident)

### Project Management
New / Save (.fdraft) / Load / Autosave (localStorage)

### Keyboard Shortcuts (16)
B/S/C/T/H Primitive, Del/Esc Selection, Ctrl+Z/Y Undo/Redo,
Ctrl+A/D/S All/Duplicate/Save, 1/2/3/0 Views, Arrows Nudge,
F1 Help Overlay

## Architektur

- **Kernel**: Rust/WASM, 15.000+ Zeilen
- **Protocol**: Serde-basiertes Wire-Format
- **Frontend**: React + React Three Fiber + WebGPU
- **Mesher**: Web Worker mit Dual Contouring
- **Solver**: PlaneGCS (2D Constraints)

## Test-Suite

- **254 Kernel-Tests** (Rust native)
- **30 Protocol-Tests**
- **20 Playwright E2E-Tests** (headed Chromium + WebGPU)
- **177 Vitest Unit-Tests** (TS)
- **Total: 481 Tests**

## Roadmap-Status

| Tier | Status | Beschreibung |
|---|---|---|
| **T0** | ✅ | Demonstrator / Architektur-Validierung |
| **T1** | ✅ | Shippable Alpha (Sketch→Extrude→Boolean→Pattern→Fillet→Variable→STL) |
| **T2** | ✅ | SDF-Superkräfte (Gyroid, Twist, Bend, Displacement, Chamfer, Shell) |
| **T3** | 🟡 | Thread (K12), GLB Export (K12.1), mesh_sdf Infra (K13.0) |

## 16 registrierte FeatureKinds

box, sphere, cylinder, capsule, hex_prism, cone, torus, thread,
extrude, revolve, hole, gyroid, linear_pattern, circular_pattern,
mirror, sketch
