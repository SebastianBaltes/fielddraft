# FieldDraft Architecture

FieldDraft is a browser-based, parametric 3D modeler for FDM/SLA 3D printing. It uses a **Signed Distance Field (SDF)** geometry kernel instead of traditional B-Rep/NURBS, compiled from Rust to WebAssembly and rendered via WebGPU ray marching.

```mermaid
graph TB
    subgraph Browser
        UI["React UI\n(Zustand Store)"]
        Renderer["Three.js\nWebGPU SDF Raymarch"]
        subgraph Workers["Web Workers"]
            Mesher["Mesher\n(Dual Contouring)"]
            OCCT["OCCT\n(STEP I/O)"]
        end
        Kernel["Kernel\n(Rust / WASM)"]

        UI <--> Renderer
        UI --> Kernel
        Kernel -- "opcodes" --> Renderer
        Kernel <--> Mesher
        Kernel <--> OCCT
    end

    Kernel -. "optional" .-> Server["Go Server\n(Auth, Sync)"]
    Server --> DB["MariaDB"]
```

---

## Table of Contents

1. [Technology Stack](#1-technology-stack)
2. [Repository Layout](#2-repository-layout)
3. [Rust Kernel](#3-rust-kernel)
   - [Protocol Crate](#31-protocol-crate)
   - [Solver Crate](#32-solver-crate)
   - [Features-Macros Crate](#33-features-macros-crate)
   - [Kernel Crate](#34-kernel-crate)
4. [Web Frontend](#4-web-frontend)
   - [State Management](#41-state-management)
   - [WASM Bridge](#42-wasm-bridge)
   - [Rendering Pipeline](#43-rendering-pipeline)
   - [Sketch System](#44-sketch-system)
   - [Import/Export](#45-importexport)
   - [CSS Architecture](#46-css-architecture)
5. [Go Server](#5-go-server)
6. [Build System & Deployment](#6-build-system--deployment)
7. [Testing Strategy](#7-testing-strategy)
8. [Key Data Flows](#8-key-data-flows)

---

## 1. Technology Stack

| Layer | Technology | Purpose |
|-------|-----------|---------|
| Geometry Kernel | Rust → WebAssembly (wasm-pack) | SDF evaluation, CSG tree, Dual Contouring mesh |
| Frontend | React 19, TypeScript 6, Vite 8 | UI components, state management |
| 3D Rendering | Three.js 0.183 + @react-three/fiber 9 | WebGPU ray marching, mesh display |
| Constraint Solver | @salusoft89/planegcs (WASM) | 2D sketch geometric constraint solving |
| CAD Import | opencascade.js 2.0, occt-import-js | STEP/IGES file import via B-Rep tessellation |
| Backend (optional) | Go 1.26, MariaDB 11 | Authentication, project sync, snapshots |
| Testing | cargo test, Vitest, Playwright | Unit (Rust/TS), integration, E2E with real WebGPU |

**Key design choice:** No hand-written shaders. All GPU code is generated dynamically via Three.js TSL (Texture Shading Language) node graphs.

---

## 2. Repository Layout

```
fielddraft/
├── crates/
│   ├── protocol/          # Wire types & document schema (Rust)
│   ├── solver/            # Constraint solver data model (Rust)
│   ├── features-macros/   # Proc-macro code generation (Rust)
│   └── kernel/            # SDF kernel, CSG, meshing, WASM bindings (Rust)
├── web/
│   ├── src/
│   │   ├── wasm/          # Auto-generated WASM output (wasm-pack)
│   │   ├── solver/        # PlaneGCS integration (TypeScript)
│   │   ├── widgets/       # Feature dialog, schema parser
│   │   ├── styles/        # CSS architecture (tokens → components)
│   │   ├── api/           # Backend API client
│   │   └── *.ts/tsx       # Application modules (~50 files)
│   ├── tests/             # Playwright E2E tests
│   └── package.json       # v0.3.55
├── server/
│   └── main.go            # Optional Go backend
├── Cargo.toml             # Workspace: 4 crates, AGPL-3.0
├── justfile               # Developer command surface
├── docker-compose.yml     # MariaDB + Go + Vite
└── DEPLOY.md              # Production deployment guide
```

---

## 3. Rust Kernel

The kernel is a Cargo workspace with four crates. The compilation target is `wasm32-unknown-unknown` via wasm-pack, with release profile: `opt-level = 3`, fat LTO, single codegen unit.

### 3.1 Protocol Crate

**`crates/protocol`** — Shared wire types between kernel and frontend.

Defines the document schema:

```
Document
├── feature_calls: Vec<FeatureCall>    # Authoritative serialization format
│   └── FeatureCall { type_name, id: FeatureId, params: Value }
├── features: Vec<SpineFeature>        # In-memory spine (skip-serialized)
├── next_feature_id: u32
└── variables: HashMap<String, f64>    # Formula engine variable table
```

**FeatureId** supports flat (`"42"`) and hierarchical (`"3.op.mask_0"`) identifiers.

**Primitive enum** — 13 geometric shape types:

| Primitive | Description |
|-----------|-------------|
| `Box`, `Sphere`, `Cylinder`, `Torus`, `Cone`, `Capsule`, `HexPrism`, `Ellipsoid` | Direct SDF evaluation |
| `Extrude` | 2D sketch profile swept along Z |
| `Revolve` | 2D sketch profile revolved around Y |
| `Gyroid` | TPMS lattice (`\|f\| - thickness`) |
| `Thread` | Metric helix |
| `ImportedMesh` | CSG-participating triangle mesh (passthrough storage) |

**Typed parameters** (K4.5.4f): `Length` (mm/cm/m/in → mm), `Angle` (deg/rad), `Query`, `MultiQuery`, `BoolOp`.

**Additional types:** `Sketch2d`, `Sketch2dPrimitive`, `MeshSdf`, `BakedVolume`, `Transform` (translate + quaternion + per-axis scale).

### 3.2 Solver Crate

**`crates/solver`** — Data model for 2D constraint sketching. This crate is **data-only** — it defines wire types but contains no solver logic. The actual constraint solving happens in TypeScript via PlaneGCS WASM.

```
SketchState
├── entities: Vec<Entity>           # Point, Line, Circle, Arc, Origin
├── constraints: Vec<Constraint>    # 12 types (see below)
├── next_entity_id: u32
└── next_constraint_id: u32

SketchDiagnostics
├── converged: bool
├── dof: i32                        # +ve = under, 0 = full, -ve = over
├── conflicting: Vec<ConstraintId>
├── redundant: Vec<ConstraintId>
├── partially_redundant: Vec<ConstraintId>
└── malformed: Vec<ConstraintId>
```

**Constraint types:** Coincident, Parallel, Horizontal, Vertical, Perpendicular, Distance, Angle, Tangent, Concentric, Equal, Midpoint, Symmetric.

### 3.3 Features-Macros Crate

**`crates/features-macros`** — Procedural macro `#[derive(FeatureSchema)]`.

Generates from annotated structs:
1. `const TYPE_NAME: &'static str` — registry key
2. `fn type_name() → &'static str`
3. `fn schema_json() → Value` — JSON schema with types, defaults, UI hints
4. `fn parse(def: &Value) → Result<Self, FeatureError>` — validated parameter extraction

Field annotations: `#[param(name = "...", default = expr, ui_hint = "slider"|"opposite_direction"|"horizontal_enum", unit = "m"|"°")]`.

### 3.4 Kernel Crate

**`crates/kernel`** — The core geometry engine. Compiled as `cdylib` (WASM) + `rlib` (Rust tests).

#### Feature System (`features.rs`)

Central dispatch for the parametric modeling timeline:

```rust
trait FeatureKind: Send + Sync {
    fn type_name(&self) -> &'static str;
    fn schema_json(&self) -> Value;
    fn execute(&self, ctx: &mut Context, id: &FeatureId, def: &Value) -> Result<(), FeatureError>;
}
```

`FeatureRegistry::with_builtins()` registers ~24 feature kinds: primitives, patterns (linear/circular/mirror), transforms (translate/rotate/scale), sketch, extrude, revolve, sweep, boolean, fillet, shell, material. `regenerate()` walks feature calls in document order, resolves formula variables, and dispatches to each feature's `execute()`.

`Context` holds the mutable document reference, solved sketch data (`HashMap<FeatureId, SketchData>`), and collected edge fillets.

#### CSG Tree (`csg.rs`)

The `CsgNode` enum (16 variants) represents the constructive solid geometry tree:

- **Leaf:** `Leaf { id, primitive }` — terminal SDF evaluation
- **Binary ops:** `Union`, `Difference`, `Intersection`, `SmoothUnion`, `SmoothDifference`
- **Domain warps:** `Twist` (rotate XZ by rate*y), `Bend` (rotate XY by rate*x), `Displace` (sine-product bump)
- **Modifiers:** `Shell` (`|sdf| - thickness`), `Morph` (lerp between SDFs)
- **Profile-based:** `Extrude { sketch, half_height }`, `Revolve { sketch }`
- **Masked blend:** Capsule-masked smooth transition between two SDFs

`CsgNode::from_features()` folds the spine linearly: each feature becomes a leaf, wrapped in its transform and domain warps, then folded into the accumulator via the feature's boolean operation.

**Contributors:** Each CSG evaluation tracks up to 4 `NodeId`s (leaves dominating the SDF within epsilon), classified as `ProxyElement`: Face (1 contributor), Edge (2), Vertex (3+). This provides approximate B-Rep-like topology from a pure SDF representation — accuracy depends on mesh resolution and breaks down at sharp feature transitions.

#### BVH & Bounded CSG (`bvh.rs`)

`BoundedCsg` mirrors `CsgNode` but adds precomputed world-space AABBs at every node and collapses transform chains at leaves. This eliminates intermediate transform nodes from the hot evaluation path and enables spatial culling.

#### SDF Compiler (`sdf_compiler.rs`)

Compiles the `BoundedCsg` tree into a flat opcode buffer for GPU consumption:

```
compile_body(node: &BoundedCsg) → CompiledSdf { opcodes: Vec<f32>, aabb, volumes }
```

**Opcode set** (35 opcodes, postfix encoding):

| Category | Opcodes |
|----------|---------|
| 3D Primitives | BOX, SPHERE, CYLINDER, TORUS, CONE, CAPSULE, HEX_PRISM, ELLIPSOID, CONST, GYROID, THREAD, TEX3D |
| 2D Primitives | CIRCLE, RECT, POLY, SECTOR |
| Binary Ops | UNION, DIFFERENCE, INTERSECT, SMOOTH_UNION, SMOOTH_DIFF |
| Modifiers | SHELL, MORPH, DISPLACE_VAL, EXTRUDE, REVOLVE_SCALE, OFFSET_2D |
| Domain Warps | WARP_PUSH, WARP_POP, WARP_TWIST, WARP_BEND, WARP_TRANSFORM, WARP_REVOLVE |
| Masked Blend | MASKED_BLEND |
| Control | END |

Leaf primitives encode: opcode tag + transform (3 translate + 4 quaternion + 3 scale) + shape parameters. This flat buffer is uploaded to the GPU as a storage buffer and interpreted by the VM shader at runtime. Typical per-body sizes: single primitive = 14–18 floats, 5 primitives + operators = ~100 floats, complex 20-primitive body = ~400 floats.

#### Multi-Body Partitioning (`body.rs`)

Features are grouped into independent solid bodies by AABB overlap analysis:

- **Union:** Overlapping AABBs coalesce into one body (merge if multi-hit)
- **Difference/Intersect:** Attach to first overlapping body; drop if none
- Feature order within a body always matches document order (deterministic)

Each `Body` caches: compiled SDF opcodes, mesh, topology, payload hash (for incremental meshing), and a version counter for GPU buffer reuse.

#### Meshing (`dc.rs`, `mdc.rs`)

**Dual Contouring** (`dc.rs`) generates triangle meshes from the SDF:
- Voxelizes the body's AABB at configurable resolution
- Narrow-band optimization: only processes voxels near the surface
- Per-vertex classification (face/edge/vertex) from contributor analysis

**Marching Cubes** variant (`mdc.rs`) available as alternative mesher.

#### Additional Modules

| Module | Purpose |
|--------|---------|
| `sketch2d.rs` | BVH-wrapped 2D sketch profiles for SDF evaluation |
| `sketch_features.rs` | Sketch feature execution, `SketchToolRegistry` with ~20 sketch tools |
| `sketch_regions.rs` | Flood-fill region detection for extrude profile selection |
| `expr.rs` | Formula expression evaluator for parametric variables |
| `export.rs` | STL export (ASCII + binary) |
| `export_3mf.rs` | 3MF export with per-body colors |
| `queries.rs` | Geometry reference queries (`qCreatedBy`, `qFaceOf`) |
| `raycast.rs` | Ray-mesh intersection for viewport picking |
| `topology.rs` | B-Rep-like edge/face tracking derived from SDF contributors |
| `mesh_sdf.rs` | Triangle mesh as SDF primitive (imported geometry) |
| `edge_detect.rs` | Sharp edge detection / silhouette extraction |
| `step_classifier.rs` | Step-by-step feature classification for STEP export |

#### WASM API (`lib.rs`)

The `Kernel` struct is the main entry point exported to JavaScript via `wasm-bindgen`:

- **Document I/O:** `load_json`, `to_json`
- **Feature mutations:** `add_primitive`, `add_boolean`, `duplicate_feature`, `remove_feature`, `reorder_feature`, `nudge_feature`, `set_feature_op`, `set_feature_rotation`, `set_feature_scale`
- **Variables:** `set_variable`, `remove_variable`, `list_variables`
- **Multi-body access:** `body_count`, per-body position/normal/index/classification buffer pointers
- **SDF access:** `body_sdf_opcodes_ptr` (flat opcode buffer), `body_sdf_aabb` (world bounds)
- **Mesh control:** `set_mesh_resolution`, `set_context_mesh_resolution`, `set_active_body_id`
- **Undo/redo:** `can_undo`, `can_redo`, `undo()`, `redo()` (kernel holds document snapshots)
- **Introspection:** `list_feature_types`, `schema_for_type`, `list_features`, `get_feature_call_params`

Mesh data is exposed as **zero-copy typed array views** into WASM linear memory (pointers must be re-acquired after any mutating call due to potential memory reallocation).

---

## 4. Web Frontend

### 4.1 State Management

**`store.ts`** — Central state via a custom pub-sub store (consumed by React's `useSyncExternalStore`).

**Key state slices:**

| Slice | Type | Purpose |
|-------|------|---------|
| `features` | `FeatureSummary[]` | Feature spine mirror from kernel |
| `selection` | `Set<number>` | Selected feature IDs |
| `meshedBodies` | `MeshedBody[]` | Per-body mesh cache (from worker) |
| `canUndo/canRedo` | `boolean` | Undo/redo availability |
| `sketchSession` | `SketchSessionState` | Active sketch draft |
| `variables` | `{name, value}[]` | Document-level scalar variables |
| `assembly` | `AssemblyState` | Multi-body assembly data |
| `rollbackIndex` | `number` | Feature timeline position |
| `syncState` | `string` | Server sync state (clean/dirty/syncing/error/offline) |

**Undo/redo** is implemented in the WASM kernel (document snapshots, 64-deep stack). The store only caches availability flags and calls `kernel.undo()` / `kernel.redo()`.

**Mutation flow:**
1. Store method calls kernel mutation (e.g., `kernel.addBox()`)
2. `commit()` refreshes all derived state (features, undo flags, variables, scene bounds)
3. `syncActiveBody()` updates LOD hint for the selected body
4. `autosave.schedule()` debounces localStorage write (500ms)
5. `scheduleWorkerRebuild()` triggers mesh regeneration in worker
6. Listeners notified → React re-renders

**Persistence** (`persistence.ts`): Auto-save to `localStorage["fielddraft.project.v1"]` with 500ms debounce. UI state (folders, visible sketches, hidden datums, outliner width) persisted separately in individual keys.

### 4.2 WASM Bridge

**`kernel.ts`** — TypeScript wrapper around the Rust WASM kernel.

**Worker architecture** (`mesher.worker.ts`):
- Main thread kernel: document state + undo history + picking (sync meshing disabled)
- Worker kernel: independent copy with Dual Contouring enabled
- Communication: `{type: "rebuild", documentJson, resolutions, activeBodyId}` → `{type: "bodies", bodies: [{id, positions, normals, indices, ...}]}`
- TypedArrays transferred (zero-copy) via structured clone
- Progressive refinement: initial coarse pass (half-res), then auto-refine to full resolution
- Coalescing: `workerInflight` flag prevents backlog of stale mesh requests

### 4.3 Rendering Pipeline

FieldDraft implements **two active SDF rendering modes**, both built on Three.js TSL and WebGPU. There are no hand-written shader files — all GPU code is generated dynamically.

#### SDF-VM Mode (`sdfVmRenderer.ts`)

Single-pass fullscreen ray marching:

```mermaid
graph LR
    A["Screen Quad"] --> B["Reconstruct Ray"] --> C["Sphere-Trace\nAll Bodies"] --> D["PBR Shade"]
```

- Fullscreen `PlaneGeometry(2, 2)` with orthographic camera
- GPU bytecode VM interprets opcode stream at shader invocation time
- Per-body SDF evaluation, closest hit wins — **brute-force: every body evaluated at every pixel**
- **Hard limits: max 8 bodies, 16384 opcodes total (storage buffer), 512 march steps, 512 opcodes/body (VM loop).** These are architectural constraints of the brute-force per-pixel approach, not tuning parameters.

#### SDF-Deferred Mode (`sdfDeferredRenderer.ts`)

Three-pass hybrid rasterization + deferred ray marching:

```mermaid
graph TD
    subgraph "Pass 1: Opaque G-Buffer"
        P1["Rasterize primitive meshes\n(FrontSide)"] --> P1out["Output: world position\n+ body tag"]
    end

    subgraph "Pass 2: Transparent G-Buffer"
        P2a["Rasterize inflated hulls\n(scale 1.01)"]
        P2b["Back-face distances\n(MAX blend)"]
        P2a --> P2out["Output: position + body tag\n+ exit depth"]
        P2b --> P2out
    end

    subgraph "Pass 3: Unified Shading"
        P3["Fullscreen quad reads\nall G-buffers"] --> P3b{"Transparent\nregion?"}
        P3b -- "Yes" --> P3c["Sphere-trace via VM"]
        P3b -- "No" --> P3d["Use rasterized hit"]
        P3c --> PBR["PBR Shading"]
        P3d --> PBR
    end

    P1out --> P3
    P2out --> P3
```

Simple primitives (box, sphere, cylinder, torus, cone, capsule) get rasterized opaque meshes for speed. Complex geometry (gyroid, boolean regions) is ray-marched through transparent hull volumes. **Hard limits: max 256 bodies (buffer allocation), 16384 opcodes total (storage buffer), 2 transparent bodies/pixel.** Per-pixel cost is O(present), independent of total body count.

#### Opcode Interpretation

**Runtime VM** (`sdfVmInterpreter.ts`): TSL function interprets opcodes at runtime from a storage buffer. 8-slot value stack + 4-slot position stack. Single shader handles any opcode sequence up to 512 opcodes/body (~35 primitives). Used by both VM and Deferred modes. `sdfRenderer.ts` provides the shared opcode constants and WGSL primitive functions.

#### Material System

Per-body PBR materials stored in a GPU storage buffer (12 floats/body stride):

```
[colorR, colorG, colorB, roughness, metalness, emissiveR, emissiveG, emissiveB,
 opacity, wireframe, displaceDepth, displaceFreq]
```

**PBR shading** (`pbrShading.ts`): Cook-Torrance BRDF with metallic workflow. 3 directional lights, GGX distribution, Schlick-Beckmann visibility, Fresnel. Normals computed via central differences on the SDF.

**Material resolution** (`materialBridge.ts`): CAD "Material" features target bodies via query selectors (`qCreatedBy(featureId)`). Resolved each frame, written to the GPU material buffer.

#### Synchronization (`viewportSdfOverlays.tsx`)

Per-frame fingerprinting drives the rebuild/update cycle with three fingerprints:

- `volumeFp` (total volume texture count) → shader rebuild (VM mode only — volume textures are baked into shader)
- `structFp` (topology hash: body count, IDs, opcode lengths, volume counts) → full mesh rebuild (deferred) or data update (VM)
- `dataFp` (data hash: body versions) → opcode/volume buffer upload only
- Material application runs every frame regardless

Adding/removing bodies or changing opcodes triggers only a fast buffer update, not a shader recompile (unless volume texture count changes).

#### Rendering Parameters

| Parameter | Value |
|-----------|-------|
| Max march steps | 512 (VM/Deferred) |
| Surface epsilon | 0.002 mm |
| Normal epsilon | 0.01 mm |
| Max ray distance | 1200–3000 mm |
| Hull inflate factor | 1.01 (deferred) |
| Build volume | 600 mm cubic |

### THREE.js Patches

The WebGPU spec allows `GPUBufferBindingType.Storage` (read_write) in fragment shaders, but Three.js r183 blocks this. Two patches in `web/patches/three+0.183.2.patch` (applied via `patch-package` postinstall):

- **`WGSLNodeBuilder.getNodeAccess()`** — Removes the warning that rejects atomics in non-compute stages, allowing `READ_WRITE` access in fragment shaders
- **`WebGPUBindingUtils`** — Respects `binding.access` for all shader stages, not just compute. Without this, storage buffers in fragment shaders are always downgraded to `ReadOnlyStorage`.

Both patches are required for the deferred renderer's per-pixel atomic body-tag buffer. Upstream PR: `mrdoob/three.js#33626` — once merged into a Three.js release, the patch file and `patch-package` dependency can be removed.

### 4.4 Sketch System

Three-layer architecture for 2D parametric constraint sketching:

```mermaid
graph TD
    Canvas["Canvas UI\n(SketchCanvas2d.tsx)"]
    Session["Session State Machine\n(sketchSession.ts)"]
    Inference["inferenceEngine.ts\nReal-time constraint suggestions"]
    PlaneGCS["planegcs.ts\nPlaneGCS WASM wrapper"]
    Drag["dragController.ts\nLive drag with continuation"]
    DoF["dofPreflight.ts\nDoF tally"]
    Diag["diagnosticsPipeline.ts\nPer-entity state coloring"]
    KernelSketch["Kernel Integration\n(sketch_features.rs)"]
    Extrude["Profile Extrusion\n(sketch2d.rs, sketch_regions.rs)"]

    Canvas -- "pointer events,\nsnap resolution" --> Session
    Session -- "sub-calls:\nFeatureCall[]" --> Solver

    subgraph Solver["Solver Integration (solver/)"]
        Inference
        PlaneGCS
        Drag
        DoF
        Diag
    end

    Solver --> KernelSketch
    KernelSketch -- "SketchToolRegistry\n→ SketchState" --> Extrude
```

**Data flow:**

1. **User draws** on 2D canvas overlay → pointer events emitted
2. **Snap inference** (`inferenceEngine.ts`): Checks proximity to endpoints, midpoints, origin; detects horizontal/vertical/parallel alignment. Priority-sorted candidates.
3. **Sub-calls accumulated** in `sketchSession.subCalls` (geometry + constraints as `FeatureCall[]`)
4. **Solver replay** (`planegcs.ts`): Translates entities/constraints to PlaneGCS format, runs DogLeg solver, reads back solved positions + diagnostics
5. **Drag controller**: Snapshots sketch, overwrites dragged point each frame, re-solves. PlaneGCS' numerical continuation keeps hard constraints satisfied.
6. **Diagnostics pipeline**: Maps solver output to per-entity visual states (blue = underconstrained, black = fully constrained, red = overconstrained)
7. **Commit**: `kernel.addSketch(planeQuery, subCalls)` stores solved state in `Context::sketches`
8. **Extrude/Revolve** features reference sketch via query, evaluate 2D SDF profile from `Sketch2d`

**Select-mode editing (P0.1–P0.3):** With a sketch open and no drawing tool active, the overlay is in *select mode*. Left-clicks hit-test committed geometry (`sketchHitTest.ts`) and toggle it in `sketchSession.selection`. The toolbar palette offers the constraints/dimensions applicable to the current pick (`sketchConstraints.ts`): geometric constraints (P0.1) and dimensions seeded from the current measurement (P0.2, editable in the constraint manager). Supported geometric picks include horizontal/vertical (one line), parallel/perpendicular (two lines), coincident (two points), equal (two curves), tangent (line + curve), **midpoint** (a point + a line — pins the point at the line's centre) and **symmetric** (two points + an axis line — mirrors them across it). Midpoint and symmetric have no single PlaneGCS primitive matching FieldDraft's entity shapes, so `planegcs.ts` maps them onto `p2p_symmetric_ppp` (line endpoints symmetric about the point ⇒ point is their midpoint) and `p2p_symmetric_ppl` respectively. A single circle or arc also takes a **radius** or **diameter** dimension (seeded from the current measurement, editable like distance/angle); these map to PlaneGCS' `circle_radius`/`arc_radius` and `circle_diameter`/`arc_diameter`, dispatched by entity kind. Diameter stores the diameter value so the annotation round-trips while the solver pins `2·radius`. Selected entities expose draggable grip handles (`sketchGrips.ts`, P0.3): line endpoints, free points, a circle's centre + radius rim, rectangle corners (axis-aligned resize against the fixed diagonal corner), polygon vertices and an arc's centre + radius. Dragging a grip rewrites the sub-call coordinate live and re-solves on drop, so existing constraints reassert themselves. All edits route through the same solve-and-replace pipeline as drawing.

**Camera gestures under the overlay:** The 2D overlay sits on top of the WebGPU canvas with `pointer-events: auto`, so it would otherwise swallow every camera gesture. Mouse zoom (wheel) and pan (right/middle drag) are *forwarded*: the overlay re-dispatches the event onto the WebGPU `<canvas>` so OrbitControls keeps driving the orthographic camera underneath (the camera state is published back to the overlay via `sketchCameraBridge.ts`). Touch is handled the same way for the two-finger pinch: `SketchCanvas2d` tracks live touch pointers and, once a second finger lands, aborts any in-progress single-finger draft and holds a lockout until every finger lifts, synthesising wheel events at the pinch midpoint to reuse that same dolly path. Single-finger touch (select/draw/grip-drag) and the entire mouse path are untouched — the pinch branches are gated on `pointerType === "touch"` plus two active fingers. Two-finger pan is a deliberate follow-up.

**Entity types:** Point, Line, Circle, Arc, Origin (fixed at 0,0).

**Drawing tools:** `sketch_point`, `sketch_line`, `sketch_rectangle`, `sketch_circle`, `sketch_polygon`, `sketch_arc`.

**Profile-operation tools:** `sketch_mirror` and `sketch_offset` act on the whole accumulated profile rather than drawing a new entity. They are applied through the tool dialog (axis for mirror, distance for offset) plus an Apply button, or by clicking the canvas while the tool is active — the click appends the operation sub-call with the dialog's current parameters. The actual geometry transform happens at commit time in `build_sketch2d_from_sub_calls` (`sketch_features.rs`): mirror unions the accumulated `Sketch2d` tree with a reflected copy (`mirror_sketch2d`); offset wraps the tree in a `Sketch2d::Offset` node whose `distance` inflates (positive) or insets (negative) the SDF in evaluation and SDF compilation. Because these ops carry no solver entity, they do not appear in the live 2D overlay — only the committed 3D body reflects them.

**Trim:** unlike the others, trim is *not* a kernel tool. It is an interactive editing mode: with the Trim button active, a canvas click cuts the curve under the cursor at its nearest intersections with the rest of the sketch and drops the picked piece — a line clicked on an overhang shortens to the crossing, a line clicked between two crossings splits into two, a full circle becomes the complementary arc, an arc shortens or splits like a line. The geometry is a pure, DOM-free module, `sketchTrim.ts`: `intersectCurves` (line/circle/arc pairwise), `nearestTrimTarget` (which sub-call was clicked) and `trimCurveAt` (the param-space cut), composed by `computeTrim`. It reads solved coordinates straight from the sub-call params and emits ordinary `sketch_line` / `sketch_arc` sub-calls — the kernel needs no trim primitive. `store.trimSketchAt` splices the 0–2 replacement pieces in via `replaceSketchSubCallAt` (the general form of the delete reducer, which rewrites or drops downstream constraint refs around the changed entity-id range) and re-solves. Constraints that referenced the trimmed entity are dropped, since the curve they pinned no longer exists. Composite primitives (rectangle, polygon) are skipped as trim *targets* — they expand to several edges from one sub-call — but still act as cutters.

**Arc gesture:** `sketch_arc` is a three-click *centre → start → end* gesture. The first click sets the centre, the second fixes radius and start angle, the third sweeps the arc; a live preview tracks the cursor between clicks. The pure geometry lives in `sketchArc.ts` (`buildArcSubCall`, `unwrapSweep`, `sampleArc`) so it is unit-testable in isolation. All angles are sketch-space (CCW from +x, Y-up) to match `sketchGrips.ts` and the kernel `SketchArcDef`; `unwrapSweep` keeps the sweep continuous across the ±180° seam so the gesture can wind toward a full turn. Rendering samples the arc into a polyline via `sampleArc` rather than `ctx.arc`, which would mirror the arc on the Y-down canvas.

**Construction geometry:** Any drawing sub-call may carry a `construction: true` param (toggled per element from its sub-spine row). Construction geometry is reference-only: `buildDraftSketchState` still builds a full solver entity for it (so constraints can reference it), but the kernel's `build_sketch2d_from_sub_calls` skips any construction-flagged call up front, so it never contributes to the extrudable region. The 2D overlay renders it muted and dashed (`COLOR_CONSTRUCTION` / `--color-construction`). The flag is a plain param — it round-trips through the solve writeback untouched and costs nothing in the tool schemas.

**Critical invariant:** Entity ID allocation order in TypeScript must exactly match kernel allocation. `buildDraftSketchState()` replicates this deterministically. Any mismatch silently corrupts constraint references.

### 4.5 Import/Export

#### STEP Export

```mermaid
graph LR
    A["Feature Tree"] --> B["StepCommand[]"] --> C["OCCT Worker"] --> D["opencascade.js\nB-Rep"] --> E["STEP File"]
```

The OCCT worker (`occt.worker.ts`) loads opencascade.js (~25 MB WASM) and executes CSG operations (box, sphere, cylinder, cone, torus, hexprism, extrude, revolve, union/difference/intersect, transform) as B-Rep geometry via OCCT's `BRepAlgoAPI`. Only root shapes (not consumed operands) are written to STEP via `STEPControl_Writer`.

#### STEP Import

```mermaid
graph LR
    A["STEP File"] --> B["OCCT Worker"] --> C["occt-import-js\nTessellation"] --> D["ImportedMesh"] --> E["Kernel"]
```

Configurable deflection (0.02 mm linear, 0.1 rad angular). AP242 fallback extracts `COMPLEX_TRIANGULATED_FACE` entities via regex parser for convergent geometry.

#### STL/OBJ

- **Rust kernel** (`export.rs`): STL ASCII + binary export, normals via cross product
- **TypeScript** (`meshIO.ts`): STL binary import (parser), OBJ import/export

#### 3MF

- **Rust kernel** (`export_3mf.rs`): Multi-body 3MF with per-body color palette (8 colors)
- **TypeScript** (`threemf.ts`): 3MF roundtrip preserving slicer metadata (Bambu, Prusa, Cura, OrcaSlicer)

### 4.6 CSS Architecture

Hierarchical, token-driven CSS with native nesting:

```
styles/
├── tokens.css         # Design tokens: colors, spacing, typography, radii, shadows
├── base.css           # Browser resets, html/body defaults
├── index.css          # Master entry point (@import all modules)
├── layout/
│   └── shell.css      # App shell container
└── components/
    ├── toolbar.css    # .fd-toolbar { ... }
    ├── outliner.css   # .fd-outliner { ... }
    ├── settings-menu.css
    ├── feature-dialog.css
    ├── viewport.css
    └── ...            # One file per component
```

- No BEM, no CSS-in-JS — native CSS nesting
- Each component has one top-level class with `fd-` prefix (`.fd-toolbar`, `.fd-outliner`, ...)
- Child selectors are short and unprefixed, scoped by nesting
- Custom properties from `tokens.css` used extensively
- Light mode only
- Minimum font size: 1rem

**Responsive layout.** Below the 768px phone breakpoint the docked Outliner
becomes an off-canvas drawer (absolutely positioned in `.main`, parked at
`translateX(100%)`), slid in by a 44px hamburger in the topbar and dismissed
via a scrim; the viewport then owns the full width. The `useMediaQuery` hook
(`useMediaQuery.ts`, kept in sync with the `768px` CSS literal via the
`MOBILE_QUERY` constant) lets `Outliner` drop its splitter-driven inline width
on mobile so the CSS drawer width wins. Two gotchas, both load-bearing: a
`box-shadow: var(--x, <fallback with commas>)` declaration placed before
nested rules makes lightningcss silently swallow those nested rules, so the
drawer states use explicit selectors and a plain shadow; and a CSS `transform`
*transition* freezes off-canvas under the always-mounted WebGPU render loop's
per-frame style recalc, so the drawer toggles instantly with no transition.

The same 768px breakpoint also up-sizes interactive controls to a comfortable
touch target. The `--touch-target` token (≈44px) raises the toolbar `.tool`
buttons (lifting their desktop `max-height` cap) and the bottom-tab strip's
add button, tabs, labels and carets — all otherwise ~32–40px. These rules live
in plain `@media (max-width: 768px)` blocks at the foot of `toolbar.css` and
`bottom-tabs.css` and stay scoped to mobile (a desktop E2E guards that the
toolbar keeps its compact height).

---

## 5. Go Server

**`server/main.go`** — Optional backend for multi-user project sync.

The application runs purely in the browser with localStorage persistence. The Go server adds:

- **Authentication:** Argon2id password hashing (t=1, m=64MB, p=4), HttpOnly session cookies (7-day TTL, SameSite=Lax)
- **Project CRUD:** Create, read, update, delete projects (50 MB document limit, 100 project quota/user)
- **Visibility:** Private or public projects; public gallery endpoint
- **Snapshots:** Frozen read-only copies for sharing (`/api/v/{id}`)
- **Thumbnails:** Per-project PNG upload/serve (2 MB limit)
- **Rate limiting:** Per-IP token bucket (60 burst, 1 req/sec refill)
- **Password reset:** 1-hour token, CLI-logged (SMTP integration TBD)

**API endpoints** (16 routes): `/api/auth/*` (register/login/logout/reset), `/api/me`, `/api/projects/*` (CRUD + publish + thumbnail), `/api/gallery`, `/api/v/{id}`.

Currently uses in-memory store; MariaDB integration planned.

---

## 6. Build System & Deployment

### Justfile (Developer Commands)

| Command | Description |
|---------|-------------|
| `just install` | npm install web dependencies |
| `just build-wasm` | Compile Rust kernel to WASM (`wasm-pack build --target web`) |
| `just dev` | Build WASM + start Vite dev server (:5173) |
| `just build` | Full production bundle (WASM + Vite) |
| `just test` | Rust unit tests + Vitest |
| `just test-rust` | `cargo test --workspace` |
| `just test-web` | Build WASM + Vitest |
| `just test-e2e` | Playwright E2E (headed Chromium + real WebGPU) |
| `just fmt` | cargo fmt + prettier |
| `just clean` | Remove all build artifacts |

### Docker Deployment

```yaml
services:
  db:     MariaDB 11 (port 3306)
  server: Go server (Dockerfile, port 8080)
  web:    Vite dev server (port 5173)
```

Server Dockerfile: Multi-stage `golang:1.26-alpine` build + `alpine:3` runtime. CGO disabled, static binary.

Environment: `DB_DSN` (MariaDB connection), `PORT` (default 8080), `SESSION_TTL` (default 168h).

---

## 7. Testing Strategy

### Unit Tests

- **Rust:** `cargo test --workspace` — kernel logic, CSG operations, SDF evaluation, meshing, export
- **TypeScript:** Vitest — solver integration, inference engine, drag controller, diagnostics pipeline, persistence, worker RPC, 3MF roundtrip, tooltips, settings

### E2E Tests (Playwright)

Runs headed Chromium with real WebGPU (no headless — requires GPU):

| Test Suite | Coverage |
|------------|----------|
| `smoke.spec.ts` | Full feature stack: add/undo/redo/delete, boolean ops, patterns, sketches, revolve, rename, reorder, suppress, duplicate, variables, formulas, fillet, STL/STEP/3MF export, STEP import, persistence |
| `renderer.spec.ts` | SDF renderer mode switching (VM ↔ Deferred), shader compilation, body count preservation, multi-primitive rendering |
| `visual.spec.ts` | Screenshot regression at 3 viewports (600/900/1440px), 2% pixel diff tolerance |

Configuration: 1 worker (serial), 30s timeout, trace on failure, Vite dev server auto-started.

---

## 8. Key Data Flows

### Feature Creation (Add a Box)

```mermaid
graph TD
    Click["User clicks 'Box' in Toolbar"]
    Store["store.addPrimitive('box')"]
    WASM["kernel.addBox(op, translate, dimensions)\n(WASM: regenerate + mesh)"]

    Click --> Store --> WASM --> Commit

    subgraph Commit["commit()"]
        Refresh["Refresh: features, canUndo,\nvariables, sceneSize"]
        Sync["syncActiveBody()\n→ kernel.setActiveBodyByFeature()"]
        Save["autosave.schedule(kernel.toJson())\n500ms debounce → localStorage"]
        Worker["scheduleWorkerRebuild()\n→ Worker: Dual Contouring"]
        Notify["notify listeners\n→ React re-renders"]
    end

    Worker --> WorkerExec["Worker: kernel.loadJson()\n→ mesh → transferrable buffers"]
    WorkerExec --> MeshUpdate["store.meshedBodies updated\n→ React re-renders"]
    MeshUpdate --> Viewport

    subgraph Viewport["viewportSdfOverlays: fingerprint changed"]
        Collect["Collect opcode buffers from kernel"]
        Upload["Upload to GPU (uniform buffer)"]
        Compile["Compile VM pipeline\n(if topology changed)"]
        Render["Render frame\n(ray march + PBR shade)"]
        Collect --> Upload --> Compile --> Render
    end
```

### Document → GPU Pipeline

```mermaid
graph TD
    Calls["Document.feature_calls[]"]
    Spine["Document.features[]\n(SpineFeature spine)"]
    CSG["CsgNode\n(boolean tree)"]
    BVH["BoundedCsg\n(AABB-annotated,\ntransforms collapsed)"]
    Partition["Per-body BoundedCsg"]
    Compiled["CompiledSdf\n{ opcodes: Vec&lt;f32&gt;, aabb, volumes }"]
    GPU["GPU uniform buffer"]
    Screen["Per-pixel SDF evaluation\n→ sphere trace → PBR shade → screen"]

    Calls -- "FeatureRegistry.regenerate()" --> Spine
    Spine -- "CsgNode::from_features()" --> CSG
    CSG -- "BoundedCsg::from_csg()" --> BVH
    BVH -- "partition()\ngroup by AABB overlap" --> Partition
    Partition -- "compile_body()" --> Compiled
    Compiled -- "Transfer to JS\nvia WASM pointer" --> GPU
    GPU -- "VM interpreter" --> Screen
```

### Sketch → Extrude

```mermaid
graph TD
    Draw["User draws on SketchCanvas2d"]
    SubCalls["sketchSession.subCalls[] accumulated\n(sketch_line, sketch_circle,\nsketch_constraint_*, ...)"]
    Solve["PlaneGCS solve\n(async, WASM worker)"]
    Solved["Solved SketchState\n(positions updated, diagnostics)"]
    Commit["kernel.addSketch(planeQuery, subCalls)\nSketchToolRegistry.apply()\n→ ctx.sketches[id]"]
    Extrude["User adds Extrude feature\n(references sketch via query)"]
    Resolve["features.rs: Resolve sketch\n→ Sketch2d → Primitive::Extrude"]
    CsgNode["CsgNode::Extrude\n{ sketch, half_height }"]
    Body["3D solid body"]

    Draw -- "pointer events +\nsnap inference" --> SubCalls
    SubCalls --> Solve
    Solve -- "Translate entities/constraints\n→ DogLeg solver" --> Solved
    Solved --> Commit
    Commit --> Extrude
    Extrude --> Resolve
    Resolve --> CsgNode
    CsgNode -- "BoundedCsg, compile,\nmesh, render" --> Body
```
