# FieldDraft — Architektur: Kernel, Rendering & B-Rep

> Architekturdoku auf Basis eines Code-Reviews (Stand 2026-06-01, `agents/fielddraft-a-dev`,
> Version v0.4.2). Schwerpunkt: **Geometrie-Kernel** (Rust→WASM) und **Shader/Rendering**
> (WebGPU/TSL). Sie beantwortet drei Leitfragen:
>
> 1. **Welche Arten von Objekten werden wie in der Szene dargestellt?** → §2, §3, §4
> 2. **Wie verläuft die Interaktion mit Objekten?** → §6
> 3. **Wie werden B-Reps abgeleitet und dargestellt?** → §5
>
> Alle Verweise sind als `datei:zeile` angegeben und im Editor klickbar. Diese Doku
> beschreibt den Ist-Zustand des Codes; Designabsicht und -historie stehen in
> [`GEOMETRY-KERNEL-CONCEPT.md`](GEOMETRY-KERNEL-CONCEPT.md) und
> [`BREP-FEATURE-VISION.md`](BREP-FEATURE-VISION.md).

---

## 1. System-Topologie

FieldDraft ist ein parametrischer, SDF-zentrierter CAD-Editor. Eine **einzige Quelle der
Wahrheit** — das Feature-Document — wird im Rust-Kernel gehalten, nach WASM kompiliert und
vom Web-Frontend über drei verschiedene Repräsentationen konsumiert: als **SDF-Bytecode**
(Echtzeit-Rendering), als **analytisches B-Rep** (Kanten, Picking, STEP-Export) und optional
als **Dreiecksnetz** (Mesh-Modus, Export).

```
┌──────────────────────────────────────── Rust-Workspace (crates/) ───────────────────────────┐
│                                                                                              │
│   protocol/         kernel/                                          solver/   features-     │
│   ┌─────────┐       ┌──────────────────────────────────────────┐    (GCS-    macros/        │
│   │Document │  ───► │ Kernel  (lib.rs)  #[wasm_bindgen]          │     Stub)   (proc-macro)   │
│   │Feature  │       │  ├─ document: Document                     │                            │
│   │Primitive│       │  ├─ bodies: Vec<Body>      (Partitionen)   │                            │
│   │Sketch2d │       │  ├─ packed: Vec<f32>       (Feature-SoA)   │                            │
│   │BoolOp   │       │  └─ undo/redo: Vec<Document>               │                            │
│   └─────────┘       │                                            │                            │
│                     │   CSG-Baum ──► SDF (sdf_compiler.rs, sdf_dual.rs)                       │
│                     │            └─► B-Rep (brep/*.rs)   └─► Mesh (dc.rs, mdc.rs, mt.rs)      │
│                     └──────────────────────────────────────────┘                            │
└──────────────────────────────────────────────│──────────────────────────────────────────────┘
                                                │ wasm-bindgen (JSON + Zero-Copy-Views)
┌───────────────────────────────────────────── web/src/ ──────────────────────────────────────┐
│                                                                                              │
│   store.ts (Zustand-Store, Single-Instance Kernel)                                           │
│      ├─ kernel.ts        (typed WASM-Wrapper)                                                 │
│      ├─ mesher.worker.ts (Off-Thread DC-Meshing, workerRpc.ts)                               │
│      └─ React-UI: App ─ Toolbar ─ Outliner ─ Viewport ─ FeatureDialog ─ SketchCanvas2d       │
│                                                                                              │
│   Rendering (WebGPU via three.js TSL):                                                       │
│      sdfDeferredRenderer.ts   ← Produktion (Raster + selektives Raymarching)                 │
│      sdfVmRenderer.ts         ← reines Fullscreen-Raymarching                                │
│      Viewer.tsx / exportR3F   ← Mesh-Pfad (react-three-fiber, Snapshots/Export)             │
│      sdfVmInterpreter.ts      ← WGSL-Bytecode-Interpreter (von beiden SDF-Pfaden genutzt)   │
│      viewportOverlays.tsx     ← B-Rep-Kanten, Sketch, Gizmos, Datums                         │
└──────────────────────────────────────────────────────────────────────────────────────────────┘
```

**Kernel-Crates** (`crates/`):

| Crate | Rolle |
|-------|-------|
| `protocol` | Serialisierbare Wire-Typen (`Document`, `Feature`, `Primitive`, `Sketch2d`, `BoolOp`), MC-Tabellen, Mesh-SDF. Kein Verhalten. |
| `kernel` | Die gesamte Geometrie-Engine: CSG, SDF-Compiler, B-Rep, Meshing, Raycast, WASM-Bindings. |
| `solver` | Constraint-Solver-Stub (der produktive 2D-Solver ist PlaneGCS-WASM im Frontend, §6.3). |
| `features-macros` | Proc-Macro zur deklarativen Feature-Definition (Schema + Generierung). |

---

## 2. Das Objektmodell im Kernel

### 2.1 Die Hierarchie: Document → Feature → Body

Die **einzige persistente Datenstruktur** ist das `Document` (`protocol/src/lib.rs:987`):

```rust
pub struct Document {
    pub schema_version: u32,
    pub features: Vec<Feature>,          // In-Memory-Spine (nicht serialisiert)
    pub feature_calls: Vec<FeatureCall>, // Kanonische Wire-Form (serialisiert)
    pub next_feature_id: u32,
    pub variables: BTreeMap<String, f32>,// benannte Parameter / Maße
}
```

Der `Kernel` (`kernel/src/lib.rs:534`) umhüllt das Document und hält abgeleitete,
gecachte Repräsentationen:

```rust
#[wasm_bindgen]
pub struct Kernel {
    pub document: Document,
    pub packed: Vec<f32>,          // Zero-Copy-Feature-Buffer für Renderer (SoA, 16 f32/Feature)
    pub undo: Vec<Document>,       // Voll-Snapshots
    pub redo: Vec<Document>,
    pub bodies: Vec<Body>,         // Multi-Body-Partitionen mit Mesh-/B-Rep-Cache
    pub mesh_version: u32,
    pub edge_fillets: Vec<EdgeFillet>,
}
```

**Wichtig — das Modell ist ein _linearer Spine_, kein Baum.** Features sind eine flache,
geordnete Liste (`Vec<Feature>`); jedes Feature faltet sich von links nach rechts in einen
Akkumulator (Start `+∞` = leerer Raum). Die _Baum_-Struktur (CSG) entsteht erst bei der
Auswertung (§2.4). Die UI (Outliner) spiegelt genau diese lineare Liste.

Ein `Feature` (`protocol/src/lib.rs:638`) ist ein Spine-Eintrag:

```rust
pub struct Feature {
    pub id: FeatureId,
    pub name: String,
    pub op: BoolOp,              // Union / Difference / Intersect — wie es sich einfaltet
    pub primitive: Primitive,    // die Kern-3D-Form
    pub transform: Transform,    // affine Platzierung (translate + quat + scale)
    pub blend_radius: f32,       // K10.0: Fillet (smooth-min)
    pub chamfer_size: f32,       // K10.2: Chamfer
    pub twist_rate: f32,         // K11.1: Domain-Warp
    pub bend_rate: f32,          // K11.2: Domain-Warp
    pub displace_depth: f32,     // K11.3: Oberflächen-Displacement
    pub shell_thickness: f32,    // Shell-Operator (aushöhlen)
    pub target_ids: Vec<FeatureId>, // explizites Body-Targeting für Difference/Intersect
}
```

#### Zwei-Schichten-Modell: `FeatureCall` (Quelle) vs. `Feature` (dekodierte IR)

Das `Feature`-Struct ist bewusst ein **denormalisiertes „Envelope"** (so der Code-Kommentar,
`protocol/src/lib.rs:636`): _jedes_ Feature trägt _alle_ Modifier-Felder physisch, auch eine
simple Box hat ein `twist_rate`/`chamfer_size`/`shell_thickness`. Bei nicht zutreffenden steht
schlicht `0.0` — serdeseitig per `#[serde(default, skip_serializing_if = "is_zero")]` aus dem
JSON ausgeblendet (`lib.rs:653` ff.), auswertungsseitig als _bedingte_ CSG-Wrapper interpretiert
(`blend_radius>0` → `SmoothUnion`, `twist_rate≠0` → `CsgNode::Twist`, `shell_thickness>0` →
`CsgNode::Shell`, …). Es sind also keine Feature-_Typen_, sondern optionale Operatoren auf dem
Primitiv des Features. Die Feld-Klassen:

| Klasse | Felder | Charakter |
|--------|--------|-----------|
| allgemein | `id`, `name` | jedes Feature |
| quasi-allgemein | `op`, `transform`, `primitive` | im aktuellen Modell hat jedes `Feature` genau eins davon; `primitive` ist faktisch der **Diskriminator** der Art |
| spezifisch / Modifier | `blend_radius`, `chamfer_size`, `twist_rate`, `bend_rate`, `displace_depth`, `displace_freq`, `shell_thickness`, `target_ids` | optional, **default `0`**, greifen nur wenn ≠ 0 |

Daneben — und das ist der eigentliche Punkt — gibt es die **polymorphe Zielform** `FeatureCall`
(`protocol/src/lib.rs:713`): `{ type_name, id, params: Value, suppressed }`. Das ist die
„Feature-as-Function"-Wire-Form (K4.5). Hier gehören feature-spezifische Eigenschaften
eigentlich hin: `type_name` wählt einen `FeatureKind` aus der Registry, `params` trägt nur die
Parameter, die dieser Typ wirklich braucht.

Die Arbeitsteilung ist eindeutig:

- **`feature_calls` ist autoritativ** und das _einzige_, was serialisiert wird; `Document.features`
  ist `#[serde(skip_serializing)]` (`lib.rs:993`). Laden rehydriert den Spine via
  `FeatureRegistry::regenerate` (`features.rs:281`): jedes `FeatureKind::execute` erzeugt die
  `Feature`-Einträge (`bootstrap_features_from_calls`, `lib.rs:1075`).
- **`Feature` ist die dekodierte Laufzeit-IR.** Die gesamte Geometrie-Pipeline (csg `from_features`,
  body `partition`, bvh, raycast, sdf_compiler, dc/mdc/mt, queries, B-Rep, STEP) liest _ausschließlich_
  den dekodierten `Feature`-Spine (~180 Call-Sites in 11 Dateien), nie das schemalose `params`-JSON.
  Der Spine ist damit **kein totes Legacy-Skelett, sondern load-bearing** — er ist die getypte
  Zwischenform, ohne die man `params` an jeder Geometrie-Stelle neu dekodieren müsste.

> **Bekannte Altlast (Refactor-Kandidat).** Die Migration ist nicht ganz abgeschlossen: Ein paar
> ältere WASM-Mutatoren (`add_primitive` `lib.rs:2027`, `nudge_feature` und die — von der Web-App
> nicht mehr aufgerufenen — `set_feature_{translate,rotation,scale,transform}`) schreiben noch
> _direkt_ in den `Feature`-Spine. Das erzwingt eine **bidirektionale Sync**: `sync_feature_calls`
> (`lib.rs:608`, pro Mutation) rekonstruiert die `feature_calls` aus dem gebackenen Spine via
> `FeatureCall::from_spine_feature` (`protocol/src/lib.rs:766`, ~190 Zeilen `json!`-Boilerplate,
> samt `is_spine_type`/`__derived_*_skip__`-Markern). Stellt man jene Mutatoren auf den
> `push_feature_call`-Pfad um (den `add_extrude` & Co. längst nutzen), wird `feature_calls` die
> einzige Quelle der Wahrheit und die gesamte Reverse-Ableitung ist löschbar (netto negativer LOC).
> Der echte Gizmo-Drag läuft bereits call-first (`applyGizmo*`, `viewportOverlays.tsx:259`), die
> frühere Perf-Sorge entfällt also. Subtilität: Transform-Anwendung bei Nicht-Spine-Primitiven
> (Extrude/Revolve) und Nudge-Akkumulation werden derzeit von der Reverse-Sync absorbiert und
> brauchen beim Umbau eine bewusste Designentscheidung.

### 2.2 Bodies: Multi-Body-Partitionierung

`body.rs:partition()` zerlegt den linearen Spine in **disjunkte Solids**:

- Ein **Union**-Feature startet immer einen neuen Body (Seed).
- **Difference/Intersect** hängt an einen bestehenden Body an — primär über `target_ids`,
  sonst per AABB-Overlap-Heuristik.
- Jeder Body bekommt eine **stabile ID** aus der kleinsten enthaltenen Feature-ID
  (`body.rs:61`).

Bodies bleiben bewusst getrennt — **kein Cross-Body-Trimming** (OnShape-artig, vgl.
[`GEOMETRY-KERNEL-CONCEPT.md`](GEOMETRY-KERNEL-CONCEPT.md), Memo „bodies-stay-separate").
Jeder `Body` cached unabhängig Mesh und B-Rep (§5.4, §7).

### 2.3 Primitive: die geometrischen Grundobjekte

Alle „Arten von Objekten" leben im `Primitive`-Enum (`protocol/src/lib.rs:240`). Jede Variante
hat ein numerisches Kind-Tag (`KIND_BOX=0 … KIND_REVOLVE=4 …`, `protocol/src/lib.rs:348`) für
das Shader-Dispatch:

| Variante | Felder | Analytisch? | SDF |
|----------|--------|-------------|-----|
| `Box` | center, half_extents | ✓ (6 Halbräume) | exakte Quader-SDF |
| `Sphere` | center, radius | ✓ | `\|p−c\| − r` |
| `Cylinder` | center, radius, half_height | ✓ (2 Kappen + Mantel) | |
| `Cone` | r_bottom, r_top, half_height | ✓ | |
| `Torus` | center, major/minor_radius | ✓ | |
| `Capsule` | radius, half_height | ✓ | |
| `HexPrism` | apothem, half_height | ✓ | |
| `Ellipsoid` | radii[3] | ✓ (skaliert) | |
| `Thread` | nominal_radius, pitch, depth, half_height | teilweise | prozedural |
| `Gyroid` | cell_size, thickness | ✗ | TPMS, prozedural |
| `Extrude` | **sketch: Sketch2d**, half_depth_z, center | teilweise | 2D-SDF ⊗ Z (§3) |
| `Revolve` | **sketch: Sketch2d**, center | teilweise | 2D-SDF um Achse (§3) |
| `ImportedMesh` | mesh: Box<MeshSdf> | ✗ | trilineares Volumen-Texture-Lookup |

Die SDF jeder Primitive wird in Rust von `sdf_primitive(primitive, p) -> f32`
(`lib.rs:213`) ausgewertet; dieselbe Mathematik existiert dreifach gespiegelt: CPU-Rust,
WGSL-Shader (§4) und generisch über Dualzahlen (§2.5).

### 2.4 Booleans, Modifier & der CSG-Baum

Drei Boolesche Operationen (`BoolOp`, `protocol/src/lib.rs:407`): `Union`, `Difference`,
`Intersect`. Aus dem linearen Spine wird via `CsgNode::from_features()` (`csg.rs:598`) ein
binärer **CSG-Baum** gefaltet. `CsgNode` (`csg.rs:492`) ist das eigentliche Rechen-Modell:

```rust
pub enum CsgNode {
    Empty,
    Leaf { id, primitive },
    Transform(Transform, Box<CsgNode>),
    Union / Difference / Intersection (Box, Box),
    SmoothUnion / SmoothDifference (Box, Box, f32, BlendMode),   // K10.0 Fillet
    Twist(Box, f32),                  // K11.1  XZ-Rotation ∝ y
    Bend(Box, f32),                   // K11.2  XY-Rotation ∝ x
    Displace(Box, f32, f32),          // K11.3  Sinus-Produkt-Muster
    Shell(Box, f32),                  // aushöhlen auf Wandstärke
    Morph(Box, Box, f32),             // K11.5  lineare Interpolation
    MaskedBlend { … },                // K10.3  lokaler Fillet auf Region
    Extrude { id, sketch, half_height },
    Revolve { id, sketch },
}
```

Das CSG ist also **mehr als Primitive + Booleans**: Es trägt _Modifier_ (Twist/Bend/Displace/
Shell/Morph als Domain-Warps), _Smooth-Booleans_ (Fillet/Chamfer als smooth-min) und die
_gehobenen Sketches_ (Extrude/Revolve als eigene Knoten, nicht als Leaf-Wrapper).

Für Auswertung und Meshing wird der `CsgNode` zu `BoundedCsg` (`bvh.rs:68`) angereichert —
strukturgleich, aber mit vorberechneten **AABBs** pro Knoten und **eingefalteten Transforms**
(BVH zum Pruning). `BoundedCsg::evaluate(p, eps, mode)` ist die zentrale SDF-Auswertung; sie
liefert nicht nur den Distanzwert, sondern im `EvalMode::Logical` auch die **beitragenden
Branches** — die Grundlage von Picking (§6.1) und B-Rep-Klassifikation (§5).

### 2.5 SDF-Auswertung in drei Formen

1. **CPU-Skalar** — `sdf_features` / `BoundedCsg::evaluate`, für Raycast, Meshing,
   B-Rep-Octree-Sampling.
2. **Dualzahlen** — `sdf_dual.rs`: `sdf_primitive_generic<T: DualNum>` (`sdf_dual.rs:40`)
   ist generisch über `DualSVec64<3>` (nalgebra) und liefert **exakte analytische Gradienten**
   (Oberflächennormalen) in einem Durchlauf. Genutzt von der B-Rep-Projektion (§5.2). Echte
   Kurven (Extrude/Revolve/ImportedMesh) fallen auf finite Differenzen zurück.
3. **GPU-Bytecode** — `sdf_compiler.rs` kompiliert den `BoundedCsg`-Baum zu einem flachen
   `Vec<f32>`-Opcode-Buffer (`CompiledSdf`, `sdf_compiler.rs:78`), den der WGSL-Interpreter
   ausführt (§4.2). CPU- und GPU-Interpreter (`eval_opcodes`) sind 1:1 gespiegelt.

---

## 3. Sketches: 2D-Geometrie und ihre Hebung ins 3D

Ein 2D-Sketch ist ein eigener Boolescher Baum (`Sketch2d`, `protocol/src/sketch2d.rs`):

```rust
pub enum Sketch2d {
    Empty,
    Prim(Sketch2dPrimitive),
    Union / Difference / Intersection (Box, Box),
    Offset { inner: Box, distance: f32 },
}
pub enum Sketch2dPrimitive {
    Circle { center, radius },
    Rectangle { center, half_extents },
    Polygon { vertices: Vec<[f32;2]> },
    Sector { center, radius, start_angle, sweep_angle },
    Path { segments: Vec<SketchSegment>, closed },  // analytische Kurven (Linien, Bögen, Béziers)
}
```

Im Kernel wird `Sketch2d` zu `BoundedSketch2d` (`sketch2d.rs:23`) mit pro-Knoten-`Aabb2`
für Query-Pruning. Die **Even-Odd-Füllregel** und die per-Segment-Branch-IDs (für stabile
Benennung der gehobenen Wände) entstehen hier; Pfad-Segmente leben _innerhalb_ eines `Path`-
Knotens (Even-Odd-Füllung), nicht als unabhängige min-kombinierte Leaves.

**Hebung ins 3D** (`lib.rs:248` / CSG-Knoten):

- **Extrude**: `d_3d = sqrt(max(d_xy,0)² + max(\|z\|−h, 0)²)` — die 2D-SDF wird mit dem
  Z-Slab kombiniert. Ergibt Deck-/Bodenkappen (Ebenen) + Wände pro Sketch-Boundary.
- **Revolve**: 2D-SDF ausgewertet bei `(r, y)` mit `r = √(x²+z²)` — Rotation um die
  Center-Achse.

Bei der B-Rep-Extraktion (§5) werden geradlinige Sketch-Boundaries zu analytischen Ebenen,
Kreise zu Zylindern; Béziers/Sektoren fallen auf tesselierte Faces zurück.

---

## 4. Darstellung in der Szene: Rendering & Shader

> **Leitfrage 1 (Teil B): Wie werden die Objekte dargestellt?** Drei Renderpfade, alle
> WebGPU über die three.js-TSL-Schicht (WGSL als inline-TS-Strings). Der Renderpfad ist
> per `renderMode` umschaltbar (`Viewport.tsx:96`).

| Modus | Datei | Technik | Einsatz |
|-------|-------|---------|---------|
| `sdf-deferred` | `sdfDeferredRenderer.ts` | Rasterung von Proxy-Meshes + selektives Raymarching | **Produktion** (Default) |
| `sdf-vm` | `sdfVmRenderer.ts` | reines Fullscreen-Sphere-Tracing aller Bodies | Debug/Referenz, „pixelgenau" |
| `mesh` | `Viewer.tsx`, `exportR3F.ts` | react-three-fiber, getesselte Dreiecke + `meshStandardMaterial` | Snapshots, Export |

In allen SDF-Moden wird die **Geometrie nicht als Mesh hochgeladen**, sondern als
SDF-Bytecode mathematisch im Fragment-Shader ausgewertet. Der Mesh-Modus ist der einzige,
der das DC-Netz des Kernels rastert.

### 4.1 SDF-Daten auf der GPU

Gemeinsame Buffer-Logik in `sdfPipelineCommon.ts`:

- **Opcode-Buffer** (`packOpcodes`, `sdfPipelineCommon.ts:65`): alle Body-Bytecodes
  back-to-back konkateniert in einem `StorageBuffer`; eine **Offset-Tabelle** `[start,length]`
  pro Body. Quelle ist `kernel.bodySdfOpcodesView(i)` (Zero-Copy-WASM-View, kopiert beim
  Hochladen).
- **Material-Buffer** (`MAT_STRIDE = 12` f32/Body: Farbe, Roughness, Metalness, Emissive,
  Opacity, Wireframe, Displace). `createMaterialBuffers`.
- **Volume-Texturen** (bis zu 4 `Data3DTexture`): trilineare SDF-Volumina für `ImportedMesh`.

### 4.2 Die SDF-VM (WGSL-Bytecode-Interpreter)

Kern beider SDF-Pfade ist `buildVmSdfEvalFn()` (`sdfVmInterpreter.ts:64`) — eine TSL-Funktion
`fn(p, baseOffset, volumeBaseIdx) -> f32`, die den Bytecode als **Stack-Maschine** abarbeitet:

- **Value-Stack** (8 tief): skalare SDF-Distanzen.
- **Position-Stack** (4 tief): 3D-Punkte für Domain-Warps (Twist/Bend/Transform pushen ein
  verzerrtes `p`, evaluieren das Kind, poppen).
- **Reverse-Polish**: `[Box-SDF push] [Sphere-SDF push] [OP_DIFFERENCE]` → `max(a, −b)`.

Opcodes (Konstanten in `sdfRenderer.ts:15`, gespiegelt aus `sdf_compiler.rs`): Primitive
`OP_PRIM_BOX=0x01 … OP_PRIM_TEX3D=0x0C`, 2D-Primitive `OP_PRIM2D_*=0x10…`, Booleans
`OP_UNION=0x20 … OP_SMOOTH_UNION=0x23`, Sweeps `OP_EXTRUDE=0x2C`/`OP_REVOLVE_SCALE=0x2D`,
Warps `OP_WARP_PUSH/TWIST/BEND/TRANSFORM=0x30…`, `OP_MASKED_BLEND=0x40`, `OP_END=0xFF`.
Eine `Loop(VM_MAX_OPCODES=512)` läuft die Opcodes ab; jeder Opcode liest seine Parameter
(Transform-Quat/Scale, Halb-Extents …) inline aus dem Buffer.

### 4.3 Sphere-Tracing

Beide SDF-Pfade marchieren denselben Strahl (max. 512 Schritte, `SURFACE_EPS≈0.002`):
`p = ro + rd·t`, `minD = min über Bodies( evalBodySdf(p) )`, `t += max(minD, eps)`, Abbruch
bei `minD < eps` (Treffer) oder `t > maxDist`. Der Unterschied liegt darin, _welche_ Bodies
pro Pixel ausgewertet werden (siehe Deferred unten).

### 4.4 Der Deferred-Renderer (Produktionspfad)

`sdfDeferredRenderer.ts` kombiniert Hardware-Rasterung (Sichtbarkeit) mit Bytecode-Raymarching
(exakte Oberfläche). Statt pro Pixel _alle_ Bodies zu tracen, rastert er pro Body ein
**Proxy-Mesh** (exakte `BoxGeometry` für Quader; um 1 % aufgeblähte Hüllen für Sphere/Cylinder/
Torus/…; AABB-Box für `needs_full_sdf`-Bodies wie Gyroid/Warps) und marchiert nur innerhalb der
getroffenen Hülle, nur gegen die 1–2 Bodies, die ein **Body-ID-Buffer** dort meldet. Welche
Primitive mesh-fähig sind, klassifiziert der Kernel via `HybridInfo` (`bvh.rs`), abgefragt
in `rebuildDeferredBodies` (`sdfDeferredRenderer.ts:751`).

**Pass-Folge** (`renderDeferredFrame`, `sdfDeferredRenderer.ts:987`):

| Pass | Ziel | Inhalt |
|------|------|--------|
| 0 Clear | `bodyIdBuf` (atomic) | Body-ID-Zähler auf 0 |
| 1 Opaque | `opaqueRT` + DepthTex | `vec4(worldPos, bodyIndex+1)`, FrontSide |
| 2 Transparent-Front | `transparentRT` | wie Opaque, FrontSide; **+ atomisches Einfügen** der Body-IDs (Zähler+ID, max 2/Pixel) |
| 2b Transparent-Back | `transparentBackRT` (HalfFloat, **kein Depth**) | skalare Kameradistanz, BackSide, MAX-Blending |
| 3 Shade | Canvas (Fullscreen-Quad) | liest alle RTs + Body-ID-Buffer, schattiert |

Der **Shade-Pass** (`sdfDeferredRenderer.ts:312`) unterscheidet drei Fälle: (A) nur Opaque →
direkt am Mesh-Punkt schattieren, Normale aus 6 SDF-Evals; (B) Transparent → Sphere-Tracing
vom Front-Punkt (`tMin`) bis Back-Distanz (`tMax`), nur gegen die 1–2 Body-IDs aus dem Buffer;
(C) Hintergrund → verwerfen.

**Kritisch für die B-Rep-Darstellung:** Der Shade-Pass **schreibt Canvas-Tiefe** aus dem
geraymarchten Oberflächenpunkt (`sdfDeferredRenderer.ts:544`):
`depth = viewZToPerspectiveDepth(viewZ, near, far)`. Dadurch können die nachfolgend gezeichneten
B-Rep-Kanten (§5.5) korrekt von SDF-Flächen verdeckt werden („visible"-Modus) — ohne diese
Tiefe lägen Kanten immer obenauf. Genau dieser Mechanismus wurde in v0.4.1 ergänzt (Commit
cef47ff).

### 4.5 PBR-Shading

`pbrShading.ts:pbrShade()` — Cook-Torrance/GGX, metallic Workflow (`F0 = mix(0.04, baseColor,
metalness)`), Schlick-Fresnel, drei fest verdrahtete Direktionallichter + flacher Ambient
(0.25), kein IBL. Zusätzliche Effekte im Shade-Pass: Kantendetektion (Laplacian aus 6 SDF-Evals
bei `eps=0.5`, graue Overlay-Linien), Selektions-Highlight (Mix nach Blau), Wireframe-Muster
(Fresnel+Grid), per-Material-Displacement.

---

## 5. B-Rep: Ableitung und Darstellung

> **Leitfrage 3.** Aus der impliziten SDF/CSG-Darstellung wird ein _explizites_
> Boundary-Representation-Modell (Vertices/Edges/Faces/Shells/Solids) abgeleitet — analytisch,
> topologisch robust und mit **stabiler Benennung**. Aller Code liegt in
> `crates/kernel/src/brep/` plus `dc.rs`/`mdc.rs` (Meshing) und den Naming-Helfern.

Der zentrale Trick ist **Branch-Tracking**: Jede beitragende Halbraum-Fläche eines Primitivs
bekommt eine `BranchId` (`csg.rs`), abgeleitet aus `FeatureId` + Halbraum-Sub-Index — also rein
aus der CSG-_Struktur_, nicht aus der Geometrie. Eine Box exportiert 6 Branches (ihre 6 Ebenen).
Eine Kante ist die Kollision von **genau 2** Branches, ein Vertex von **≥3**. Weil die IDs nicht
geometrisch sind, überleben sie Parameteränderungen — die Grundlage gegen das
*Topological-Naming-Problem* (§5.6).

### 5.1 Pipeline-Überblick

Einstieg: `Body::ensure_brep()` (`body.rs`, lazily, gecached) →
`Octree::build()` → `weave()`:

```
BoundedCsg
   │
   ▼  ① Octree (octree.rs)        — adaptives Sampling, Zellen klassifizieren:
   │                                 Face (Vorzeichenwechsel), Edge (2 Branches), Vertex (≥3)
   ▼  ② Projection (projection.rs) — Newton/Taubin: Seed-Punkt exakt auf die Oberfläche ziehen
   │                                 (Gradient via Dualzahlen, §2.5)
   ▼  ③ Extraction (extraction.rs) — Edge-Punkt = Schnitt zweier Tangentialebenen (3×3-Solve);
   │                                 Vertex = LSQ/SVD über ≥3 Ebenen; Phantom-Filter
   ▼  ④ Edge-Walker (edge_walker.rs)— Punkte nach Branch-Signatur gruppieren, globale Primitive
   │                                 (Linie/Kreis) fitten oder verketten; an Vertices snappen
   ▼  ⑤ Weaving (weaving.rs)        — Hermite→NURBS-Edges; Branch→Halbraum-Lookup für Faces;
   │                                 Loops ordnen (reflex-aware); Shell/Solid bauen
   ▼
  BRep  (Vertices, NURBS-Edges, analytische/tesselierte Faces, Shell, Solid)
```

### 5.2 ① Octree + ② Projektion

`octree.rs`: Vom AABB des Bodys (1 % Marge) rekursiv in Oktanten unterteilt. Pro Zelle werden
8 Ecken + Zentrum via `BoundedCsg::evaluate(EvalMode::Logical)` gesampelt → Menge distinkter
`BranchId`s + Vorzeichenwechsel. Klassifikation (`CellClass`, `octree.rs:55`): **Empty**
(verworfen), **Face** (Vorzeichenwechsel, ≤1 Branch), **Edge** (genau 2 Branches), **Vertex**
(≥3). Eine **Lipschitz-Schranke** (`f_min > Zelldiagonale` ⇒ keine Oberfläche möglich) fängt
dünne Rippen, die ein reiner Vorzeichentest verfehlen würde. Abbruch bei `min_cell_size` oder
`max_depth`. Ergebnis: dünn besetzter Baum klassifizierter Leaf-Zellen — auflösungsrobust (ein
sauberer Würfel liefert immer 12 Edge- und 8 Vertex-Cluster).

`projection.rs:project_to_surface()` zieht einen Seed-Punkt per **Taubin-normalisiertem
Newton-Schritt** `p ← p − (f/\|∇f\|²)·∇f` exakt auf die Oberfläche (1–4 Iterationen; bei echter
euklidischer SDF ist ein Schritt exakt). Die Normale kommt aus dem Gradienten der letzten
nicht-degenerierten Iterierten — wichtig an C⁰-Knicken (Box-Flächen liefern auf der Kante
Gradient 0).

### 5.3 ③ Extraktion + ④ Edge-Walker

`extraction.rs`: Pro Edge-Zelle werden die 2 Branches sauber projiziert und ihre
Tangentialebenen geschnitten — ein 3×3-System aus zwei Ebenengleichungen + Tangentenrichtung
(`extract_edge_point`, `extraction.rs:306`). Pro Vertex-Zelle: Least-Squares-Schnitt von ≥3
Ebenen via SVD (`extract_vertex`, `extraction.rs:429`). Ein **Phantom-Filter** (zellrelative
`\|f\|`-Schranke) verwirft falsche Treffer auf der Medialachse/in Voronoi-Gräben.

`edge_walker.rs:build_wireframe()` (`edge_walker.rs:689`) macht aus den ungeordneten Punkten ein
Drahtgitter: Vertices nach Branch-Menge deduplizieren; Edge-Punkte pro `(BranchA, BranchB)`-
Signatur gruppieren; **global ein Primitiv fitten** — Linie (PCA) oder Kreis (Kåsa-Fit, robust
gegen Achs-Kontamination, DL-51/52) — sonst greedy mit Lückentrennung verketten; Endpunkte an
deduplizierte Vertices snappen. Ergebnis: `BRep` mit Vertices + **Hermite-Edges** (Punkt+Tangente
pro Sample), Faces/Shells noch leer.

### 5.4 ⑤ Weaving → fertiges B-Rep

`weaving.rs:weave()` (`weaving.rs:852`) vollendet das B-Rep:

1. **Hermite → NURBS** (`hermite_to_nurbs`, `weaving.rs:65`): jede Edge-Kette wird zu einer
   stückweise-kubischen NURBS-Kurve (Bézier-Konkatenation, Sehnenlängen-Parametrisierung,
   geklammerter Knotenvektor, C⁰-Joins).
2. **Branch→Halbraum-Map** (`branch_halfspace_map`, `weaving.rs:127`): den `BoundedCsg`-Baum
   ablaufen und pro Leaf die analytischen Halbräume sammeln (`HashMap<u32,(HalfSpace,Transform)>`).
   Extrude/Revolve heben dafür ihre 2D-Sketch-Wände analytisch (gerade→Ebene, Kreis→Zylinder);
   gekrümmte Boundaries → tesselierte Faces.
3. **Faces bauen**: Edges pro Branch gruppieren, Oberfläche nachschlagen
   (`SurfaceGeometry::Analytic{ surface, transform }` oder `Tessellated`), Loops per
   **reflex-aware Turn-Angle-Walk** (DL-58) ordnen (Außenloop CCW von außen, Löcher umgekehrt).
4. **Shell + Solid**: `is_closed` (wasserdicht) ⇔ jede Edge wird von genau zwei Face-Loops
   referenziert. Sonderfall kantenlose Flächen (eine Vollkugel) als Single-Face-Shell.

Die B-Rep-Datenstrukturen (`brep/mod.rs`) folgen einem Index-Arena-Muster
(`VertexId/EdgeId/FaceId/ShellId/SolidId` als `u32`-Newtypes):

```rust
struct Vertex { position: [f64;3], branches: Vec<BranchId> }          // ≥3 Branches, sortiert
enum  EdgeGeometry { Hermite(..), Nurbs(NurbsCurve), Polyline(..) }
struct Edge   { geometry, signature: (BranchId, BranchId), start/end: Option<VertexId> }
enum  SurfaceGeometry { Analytic { surface: HalfSpace, transform }, Tessellated { .. } }
struct Face   { branch_id, surface, outer: Loop, inner: Vec<Loop> }
struct Shell  { faces: Vec<FaceId>, is_closed: bool }
struct Solid  { outer: ShellId, inner: Vec<ShellId> }
```

Die `signature` einer Edge / der `branch_id` einer Face ist die **stabile, nicht-geometrische
Identität** — Resize einer Box ändert die Geometrie, aber nicht die Branch-IDs.

### 5.5 Darstellung des B-Rep im Viewport

Das gewobene B-Rep wird **nicht** als Mesh getesselt, sondern als analytische Kanten dargestellt:

1. **Kernel-Export** — `Kernel::body_brep_edges(i)` (`lib.rs:2804`) sampelt jede Edge-Geometrie
   (`edge_sampling::sample_edge`) zu einer **world-space-Polyline** (Body-local + `placement`
   gefaltet, DL-56) und liefert JSON-Array `{a, b, points[]}`, wobei `a/b` die
   Signatur-Branch-IDs sind — dieselben, die `pick()` in `contributors`/`branches` meldet.
2. **Web-Konsum** — `viewportOverlays.tsx:443` ruft `bodyBrepEdges(i)` für alle Bodies, baut
   eine gemeinsame `LineBasicMaterial`-Geometrie. Bewusst **render-mode-unabhängig**: das
   analytische B-Rep lebt auf dem Main-Thread-Kernel auch in den SDF-Modi, deshalb erscheinen
   Kanten dort ebenso (`viewportOverlays.tsx:430`).
3. **Verdeckung** — drei Modi (`BRepEdgeMode`, `SettingsMenu.tsx`): `visible` (Kanten
   `depthTest=true` gegen die in §4.4 geschriebene SDF-Tiefe → korrekt verdeckt), `xray`
   (`clearDepth()` vor dem Zeichnen → immer sichtbar), `none`. Die Layer-Logik steckt in
   `viewportSdfOverlays.tsx:renderOverlayLayer` und `viewportOverlays.tsx:BRepEdgeOverlay`.
4. **Hover** — eine gehoverte Kante leuchtet, wenn beide Signatur-Branches `a,b` in den
   `pick.branches` enthalten sind (`viewportOverlays.tsx:485`).

`Kernel::body_brep_counts(i)` (`lib.rs:2777`) liefert zusätzlich V/E/F/Shell/Solid-Zähler +
Watertight-Flag für die UI.

### 5.6 Stabile Benennung (Topological Naming)

`brep/naming.rs` erzeugt deterministische semantische IDs aus Branch-Signaturen plus einem
**Spatial-Index** (lexikografische Zentroid-Ordnung als Tie-Break, DL-32):
`face_id(branch, idx)`, `edge_id(a, b, idx)`, `vertex_id(branches, idx)` — gehasht mit **FNV-1a**
(reproduzierbar über Maschinen/Rust-Versionen, _nicht_ SipHash). Ein geometrischer Fingerprint
(Zentroid, Sehnenlänge, Mid-Tangente) dient als zweite Stufe zur Auflösung bei Topologie-
Änderungen. So bleibt „fillet diese Kante" über Rebuilds hinweg gültig.

### 5.7 STEP-Export

`brep/step_export.rs:export_step_placed()` (aufgerufen via `Kernel::body_step_export`,
`lib.rs:2836`, bzw. `sceneStepExport` für alle Bodies in einer Datei) schreibt ISO-10303-21
AP214: `MANIFOLD_SOLID_BREP` → `CLOSED_SHELL` → `ADVANCED_FACE` (analytisch: `PLANE`,
`CYLINDRICAL_SURFACE`, …; sonst tesseliert), `EDGE_CURVE` mit `B_SPLINE_CURVE_WITH_KNOTS` für
NURBS, `VERTEX_POINT`. Das `placement` des Bodys wird erst beim Export eingefaltet (DL-50/56) —
reine Verschiebung bleibt ein B-Rep-Cache-Hit.

---

## 6. Interaktion mit Objekten

> **Leitfrage 2.** Auswahl, Hover, Sketch-Editor, Feature-Erstellung/-Bearbeitung, Kamera.

### 6.1 Picking: vom Klick zur Selektion

FieldDraft nutzt **kein GPU-ID-Buffer-Picking**, sondern delegiert an den Kernel-Raycaster:

```
Pointer-Event (Viewport.tsx:137 pickAtMouse)
   │  NDC + Kamera (pickCameraBridge.ts)
   ▼  kernel.pick(origin, direction)               (kernel.ts:404)
   ▼  raycast.rs:raycast()  — Sphere-Tracing gegen die CSG-SDF (raycast.rs:77)
   │     Treffer → Normale (zentrale Differenz), Klassifikation
   ▼  PickHit { point, normal, t, element: face|edge|vertex,
   │            contributors: [FeatureId], branches: [BranchId] }
   ▼  Viewport.tsx:307 handleSelectionClick → store.selectOnly / toggleSelect / selectRangeTo
```

`element` ergibt sich aus der Zahl der beitragenden Branches am Trefferpunkt (mehrere → Kante).
`contributors` sind Root-Feature-IDs (für Selektion), `branches` die per-Face-Branch-IDs (für
Edge-Matching, §5.5). Liegt ein FeatureDialog im „Click-to-Pick"-Modus, fängt
`queryPickerBridge.ts` den Klick ab und wandelt ihn in einen stabilen Query-String (§6.4).

### 6.2 Selektions- und Hover-Modell

Im Store (`store.ts`): `selection: ReadonlySet<number>` (Feature-IDs) + `selectionAnchor`
(für Shift-Range) + `highlightedBodyIndex` (LOD-Hinweis). Im 3D-Viewport ist die Selektions-
Granularität der **Body/das Feature**; einzelne Faces/Edges werden über `queryLabels.ts` adressiert
(`qCreatedBy(id)`, `qFaceOf()`, `qEdgeBetween(a,b)`), nicht selektiert. **Hover** ist rein visuell
(kein persistenter State): `PickProbe` (`Viewport.tsx:58`) aktualisiert pro Frame ein React-`pickResult`
für HUD, Snap-Hints und Edge-Highlight.

### 6.3 Der Sketch-Editor

Beim Betreten eines Sketches (`store.enterSketch(plane)`) wird `sketchSession` aktiv
(`sketchSession.ts`): gesperrte `planeQuery`, gewähltes Tool, Draft-Subcalls. Gezeichnet wird auf
einem **2D-Canvas-Overlay** über dem WebGPU-Viewport (`SketchCanvas2d.tsx`), mit
`sketchCameraBridge.ts` für 2D↔Screen-Transforms. DOF-Färbung: unterbestimmt (blau) →
voll bestimmt (grün) → überbestimmt (rot).

Der **Constraint-Solver** ist PlaneGCS (WASM) über einen dünnen TS-Wrapper
(`solver/planegcs.ts`): FieldDraft-`Entity`/`Constraint` → PlaneGCS-Primitive →
`wrapper.solve()` → Parameter zurücklesen, samt `SketchDiagnostics` (konfliktäre/redundante
Constraints). Live-Dragging stabilisiert `solver/dragController.ts` (PlaneGCS-`temporary`-Flag);
`inferenceEngine.ts` schlägt beim Zeichnen Constraints vor (Snap auf Punkt/Linie/Senkrechte);
`dofPreflight.ts`/`diagnosticsPipeline.ts` zählen Freiheitsgrade und übersetzen Diagnose-Tags.

### 6.4 Features erstellen & bearbeiten

`Toolbar.tsx` rendert Primitiv- und Sketch-Buttons (inkl. Drag-&-Drop
`dataTransfer "fielddraft/primitive"`). Bearbeitet wird über **einen generischen**
`widgets/FeatureDialog.tsx`, der pro Parameter ein Widget nach `schema.params[].widget`
dispatcht (number, vec2/3, length, angle, enum, color, bool, query, multi_query, slider,
group/tabs — `widgets/schema.ts`). Conditional-Visibility (`visible_when`) und Query-Widgets
(Click-to-Pick via `queryPickerBridge`) inklusive. `fieldscript.ts` ist davon getrennt: ein
deterministisches Mini-Skripting für Assembly-Kinematik (`new Function(ctx, params, src)`).

**Edit-Flow**: Outliner-/Selektionsklick → `store.editFeatureId(id)` → Dialog mit Schema +
aktuellen Kernel-Parametern → `onChange` (lokaler Draft) → Commit `store.updateFeature(id, def)`
→ Kernel regeneriert. Schemata stammen aus `features-macros` (Rust-Proc-Macro) bzw.
`fd.schemaForFeatureId(id)`.

### 6.5 Outliner

`Outliner.tsx` zeigt die **flache** Feature-Liste (kein Baum — der Spine _ist_ linear, §2.1):
pro Zeile Glyph, Name, Bool-Op-Badge, Sichtbarkeits-Auge, optionale Sketch-Sub-Zeilen.
**Folders** (`store.ts`) sind eine reine Anzeige-Gruppierung (localStorage, nicht kernelseitig).
Zeilen-Selektion spiegelt `state.selection` und synchronisiert das aktive Body-LOD
(`setActiveBodyByFeature`).

### 6.6 State-Architektur & Re-Evaluation

Der `KernelStore` (`store.ts`, Zustand) hält die **einzige Kernel-Instanz** und mutiert State
über `commit()` (komplettes Rewrite + Listener-Notify + debounced Autosave nach localStorage).
DC-Meshing läuft off-thread in `mesher.worker.ts` (eigene WASM-Instanz, `workerRpc.ts`), mit
Coalescing (in-flight → `workerRebuildQueued`) und Sync-Fallback. **Re-Eval-Kette**: Mutation →
Kernel-Methode → `commit()` (Version++) → `refreshMeshCache()` postet Rebuild → Worker liefert
Mesh → `state.meshedBodies` → React rendert Viewport. Wichtig: In den SDF-Modi ist `meshedBodies`
leer (DC nur im Mesh-Modus aktiv) — SDF-Bytecode und B-Rep kommen direkt vom Main-Thread-Kernel.

### 6.7 Kamera & Ansicht

`viewportCamera.tsx`: OrbitControls (`CameraRig`) + orthografische Sketch-Kamera;
Preset-Ansichten front/top/right/iso; `CameraQuaternionBridge` publiziert die Kamera-Quaternion
an die ViewCube-UI und an `publishPickCamera()` (für §6.1). `viewCommands.ts` animiert Presets
(Hotkeys 1/2/3/0). Overlays (`viewportOverlays.tsx`): PickHud, SketchOverlay3D,
SelectionTransformGizmo, BRepEdgeOverlay, DatumOverlay.

---

## 7. Caching & Re-Evaluation (Querschnitt)

Jeder `Body` cached unabhängig:

- **Mesh** (`body.rs`): per-Body-`version`-Zähler, der nur bei echtem Remesh hochzählt.
  Affine-Fast-Path: reine Translation → GPU-Matrix statt Remesh. LOD: aktiver Body
  `active_mesh_resolution=48`, Kontext-Bodies `24`.
- **B-Rep** (`Body::brep`): lazily gewoben, gecached per Payload-Key; **reine Platzierung
  (Move) ist ein Cache-Hit** — das `placement` wird erst beim Konsum (Edge-Sampling/STEP)
  gefaltet (DL-50/56). Geometrie-Änderung → Key ändert sich → Neu-Weben.
- **SDF-Bytecode**: vom `sdf_compiler` aus dem `BoundedCsg` erzeugt, vom Frontend via
  `bodySdfOpcodesView` abgeholt.

Undo/Redo arbeitet mit Voll-Snapshots des `Document` (`Kernel::undo/redo`).

---

## 8. Datei-Landkarte

**Kernel (`crates/kernel/src/`)**

| Datei | Rolle |
|-------|-------|
| `lib.rs` | `Kernel`-Struct + alle `#[wasm_bindgen]`-Exporte (Mutatoren, Views, B-Rep/STEP-Queries) |
| `body.rs` | Body-Partitionierung, Mesh-/B-Rep-Cache, `ensure_brep` |
| `csg.rs` | `CsgNode`-Enum, Feature→CSG-Faltung |
| `bvh.rs` | `BoundedCsg` (AABB-annotiert), `evaluate`, `HybridInfo` für Renderer |
| `sdf_compiler.rs` | CSG → GPU-Opcode-Buffer (`CompiledSdf`), CPU-Interpreter |
| `sdf_dual.rs` | Dualzahl-SDF für exakte Gradienten |
| `expr.rs` | Ausdrucks-/Variablen-Auswertung (Maße) |
| `features.rs` | Feature-Definitionen/Registry (größte Datei) |
| `sketch2d.rs`, `sketch_regions.rs`, `sketch_features.rs` | 2D-Sketch-Modell, Regionen, Sketch-Features |
| `raycast.rs`, `queries.rs` | SDF-Raycast (Picking), Geometrie-Queries |
| `dc.rs`, `mdc.rs`, `mt.rs` | Dual Contouring / Manifold-DC / Marching-Tetrahedra (Meshing) |
| `brep/octree.rs` | ① adaptiver Octree + Zellklassifikation |
| `brep/projection.rs` | ② Newton/Taubin-Projektion |
| `brep/extraction.rs` | ③ Edge-/Vertex-Solve + Phantom-Filter |
| `brep/edge_walker.rs` | ④ Drahtgitter: Fit/Chaining/Snapping |
| `brep/weaving.rs` | ⑤ NURBS-Fit + Face-Weaving + Shell/Solid |
| `brep/mod.rs` | B-Rep-Datenstrukturen (Arena) |
| `brep/naming.rs` | Stabile topologische IDs (FNV-1a) |
| `brep/edge_sampling.rs` | Edge-Geometrie → Polyline (Viewport/STEP) |
| `brep/step_export.rs` | ISO-10303-21-Export |

**Web (`web/src/`)**

| Datei | Rolle |
|-------|-------|
| `store.ts` | Zustand-Store, Single-Kernel, Re-Eval, Selektion |
| `kernel.ts` | typisierter WASM-Wrapper (`pick`, Views, Stride-Konstanten) |
| `mesher.worker.ts`, `workerRpc.ts` | Off-Thread-Meshing |
| `sdfDeferredRenderer.ts` | Produktions-Renderer (Raster + Raymarch, Depth-Write) |
| `sdfVmRenderer.ts`, `sdfVmInterpreter.ts` | Fullscreen-Raymarch + WGSL-Bytecode-VM |
| `sdfPipelineCommon.ts` | Opcode-/Material-/Volume-Buffer |
| `pbrShading.ts` | Cook-Torrance-PBR |
| `Viewport.tsx`, `viewportSdfOverlays.tsx`, `viewportOverlays.tsx` | Szene, Renderpfad-Auswahl, B-Rep-/Sketch-/Gizmo-Overlays |
| `viewportCamera.tsx`, `viewCommands.ts`, `pickCameraBridge.ts` | Kamera, Presets, Pick-Bridge |
| `SketchCanvas2d.tsx`, `sketchSession.ts`, `sketchCameraBridge.ts` | 2D-Sketch-Editor |
| `solver/*` | PlaneGCS-Wrapper, Drag, Inferenz, DOF, Diagnose |
| `widgets/FeatureDialog.tsx`, `widgets/schema.ts` | generische Feature-UI |
| `Toolbar.tsx`, `Outliner.tsx`, `App.tsx` | UI-Shell |
| `queryLabels.ts`, `queryPickerBridge.ts` | stabile Geometrie-Referenzen, Click-to-Pick |
| `Viewer.tsx`, `exportR3F.ts`, `stepExport.ts`, `threemf.ts` | Mesh-Pfad & Export |

---

## 9. Glossar

- **SDF** — Signed Distance Field; implizite Geometrie als Vorzeichen-Distanz-Funktion.
- **CSG** — Constructive Solid Geometry; Booleaner Baum aus Primitiven.
- **Branch / BranchId** — eine Halbraum-Fläche eines Primitivs; rein strukturelle ID
  (FeatureId + Sub-Index), Basis der stabilen Benennung.
- **B-Rep** — Boundary Representation; explizite Topologie (Vertices/Edges/Faces/Shells/Solids).
- **Weaving** — der Schritt, der aus klassifizierten Octree-Zellen ein fertiges B-Rep „webt".
- **Dual numbers** — automatische Differentiation für exakte SDF-Gradienten/Normalen.
- **SDF-VM** — Stack-Maschine, die kompilierten SDF-Bytecode im WGSL-Shader interpretiert.
- **Deferred-Pfad** — Rasterung von Proxy-Hüllen + selektives Raymarching im Shade-Pass.
- **Body** — disjunkter Solid (Partition des Feature-Spine); cached Mesh + B-Rep getrennt.
- **GCS** — Geometric Constraint Solver (PlaneGCS) für 2D-Sketches.
- **DL-xx** — „Design Log"-Einträge in [`GEOMETRY-KERNEL-CONCEPT.md`](GEOMETRY-KERNEL-CONCEPT.md).
</content>
</invoke>
