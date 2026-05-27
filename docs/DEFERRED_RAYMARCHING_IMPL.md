# Deferred Raymarching — Implementierungsstand

Dieses Dokument beschreibt den tatsächlich implementierten Deferred-Raymarching-Renderer
auf Basis des Konzeptdokuments `DEFERRED_RAYMARCHING.md`. Nur was im Code steht, steht hier.

***

## Architektur-Überblick

Der Deferred Raymarcher kombiniert Hardware-Rasterisierung (Sichtbarkeitstest)
mit SDF-Sphere-Tracing (exakte Oberfläche). Pro Body werden Hüll-Meshes
rasterisiert, um per Pixel zu bestimmen, welche SDF-Terme evaluiert werden
müssen. Der Sphere-Tracer erzeugt dann das finale Bild.

### Dateien

| Datei                               | Rolle                                                            |
| ----------------------------------- | ---------------------------------------------------------------- |
| `web/src/sdfDeferredRenderer.ts`    | Pipeline-Init, alle Passes, Materialien, Body-Rebuild            |
| `web/src/sdfVmInterpreter.ts`       | GPU-bytecode VM — interpretiert Opcodes zur Laufzeit             |
| `web/src/sdfPipelineCommon.ts`      | Shared Buffers: Opcode-Packing, Material-Layout, Volume-Texturen |
| `web/src/sdfRenderer.ts`            | SDF-Primitive-Funktionen (WGSL), Opcode-Konstanten               |
| `web/src/viewportSdfOverlays.tsx`   | React-Integration: `SdfDeferredOverlay`-Komponente               |
| `web/src/pbrShading.ts`             | Cook-Torrance PBR (3 Directional Lights + Ambient)               |
| `web/src/materialBridge.ts`         | Kernel → Pipeline Material-Mapping                               |
| `crates/kernel/src/bvh.rs`          | `classify_for_hybrid()` → `HybridInfo` (Rust-seitig)             |
| `crates/kernel/src/sdf_compiler.rs` | CSG-Baum → flacher Opcode-Buffer (RPN)                           |

### Konstanten

| Konstante              | Wert        | Bedeutung                                            |
| ---------------------- | ----------- | ---------------------------------------------------- |
| `DEFERRED_MAX_OPCODES` | 4096        | Max. Opcodes im concatenated Buffer (uniform buffer) |
| `MAX_BODIES`           | 32          | Max. Bodies — begrenzt durch uint32-Bitmask          |
| `DEFERRED_MARCH_STEPS` | 512         | Max. Sphere-Trace-Schritte pro Pixel                 |
| `SURFACE_EPS`          | 0.002       | Konvergenz-Schwelle für Treffer                      |
| `MAX_MARCH_DIST`       | 3000        | Raymarching-Distanzlimit                             |
| `HULL_INFLATE`         | 1.01        | Aufblasfaktor für transparente Hulls (1%)            |
| `MAX_BITMASK_PIXELS`   | 4096 × 2160 | Maximale Viewport-Auflösung für Bitmask-Buffer       |
| `VM_MAX_OPCODES`       | 2048        | Per-Body Opcode-Limit im VM-Interpreter              |
| `MAX_VOLUME_TEXTURES`  | 4           | Max. 3D-Texturen für importierte Mesh-SDFs           |
| `NORMAL_EPS`           | 0.01        | Epsilon für Finite-Difference Normalen               |

***

## Hüll-Mesh-Klassifikation

Die Klassifikation erfolgt Rust-seitig in `classify_for_hybrid()` (`bvh.rs`).
Pro Body wird ein `HybridInfo` erzeugt:

```rust
pub struct HybridInfo {
    pub leaves: Vec<HybridLeafInfo>,
    pub sdf_regions: Vec<SdfRegionInfo>,   // AABB-Overlap-Regionen bei CSG-Ops
    pub needs_full_sdf: bool,              // true → nur AABB-Box, kein Leaf-Mesh
    pub is_union_only: bool,               // true → keine Differenz/Intersection im Baum
}

pub struct HybridLeafInfo {
    pub kind: String,        // "box", "sphere", "cylinder", ...
    pub aabb: [f32; 6],
    pub transform: [f32; 10], // [tx,ty,tz, qx,qy,qz,qw, sx,sy,sz]
    pub params: Vec<f32>,
    pub role: String,        // "leaf" oder "skip"
    pub contour: Option<ContourData>,
}
```

### Klassifikationsregeln (implementiert)

Die `classify_walk`-Funktion traversiert den `BoundedCsg`-Baum rekursiv:

| CSG-Knoten                                        | Verhalten                                                           |
| ------------------------------------------------- | ------------------------------------------------------------------- |
| `Union`                                           | Beide Kinder als Leaves weiterreichen                               |
| `Difference`                                      | Linkes Kind \= Leaf, rechtes Kind \= `role: "skip"`                 |
| `Intersection`                                    | Beide Kinder \= `role: "skip"`                                      |
| `SmoothUnion` / `SmoothDifference`                | Wie Union/Difference, plus AABB-Overlap-Region (expanded um Radius) |
| `MaskedBlend`                                     | Kapsel-AABB der Maskierungs-Linie als Overlap-Region                |
| `Twist` / `Bend` / `Displace` / `Shell` / `Morph` | `needs_full_sdf = true` — keine Leaf-Meshes möglich                 |
| `Extrude`                                         | Kontur-Extraktion → `HybridLeafInfo` mit `contour`-Daten            |
| `Revolve`                                         | Kontur-Extraktion → `HybridLeafInfo` mit `contour`-Daten            |

### Dual-Mesh-Zuordnung (TypeScript-seitig)

In `rebuildDeferredBodies()` (`sdfDeferredRenderer.ts`) werden pro Body
Meshes erzeugt:

| Body-Typ                | Opakes Mesh                               | Transparentes Mesh               | Bemerkung                                  |
| ----------------------- | ----------------------------------------- | -------------------------------- | ------------------------------------------ |
| `needs_full_sdf` Bodies | —                                         | BoxGeometry (AABB + 0.5 Padding) | Gyroid, Domain Warps, Shell etc.           |
| `box`                   | BoxGeometry (exakt)                       | —                                | Rein konvex, kein transparentes Mesh nötig |
| `sphere`                | SphereGeometry(r, 32, 24)                 | SphereGeometry × HULL\_INFLATE   |                                            |
| `cylinder`              | CylinderGeometry(r, r, h×2, 32)           | CylinderGeometry × HULL\_INFLATE |                                            |
| `torus`                 | TorusGeometry(R, r, 24, 48), rotateX(π/2) | TorusGeometry × HULL\_INFLATE    |                                            |
| `capsule`               | CapsuleGeometry                           | CapsuleGeometry × HULL\_INFLATE  |                                            |
| `cone`                  | CylinderGeometry(rBottom, rTop, h×2, 32)  | CylinderGeometry × HULL\_INFLATE |                                            |
| `extrude` / `revolve`   | ContourMesh (exakt)                       | ContourMesh × HULL\_INFLATE      | via Kontur-Daten aus Rust                  |

Leaves mit `role: "skip"` werden übersprungen.

Fallback: Wenn ein Body weder opake noch transparente Meshes erzeugt
(z.B. alle Leaves sind "skip"), aber Opcodes hat, wird eine AABB-Box
als transparentes Mesh verwendet.

***

## Pipeline

### DeferredPipelineState

```typescript
interface DeferredPipelineState {
  // Render Targets
  opaqueRT: WebGLRenderTarget;         // RGBA Float + DepthTexture(Float)
  transparentRT: WebGLRenderTarget;    // RGBA Float + DepthTexture(Float)
  transparentBackRT: WebGLRenderTarget; // RGBA HalfFloat, kein Depth

  // Scenes (je eine pro Raster-Pass + Shade-Pass)
  opaqueScene, transparentScene, transparentBackScene, shadeScene: Scene;
  shadeCamera: OrthographicCamera;

  // Materialien
  shadeMaterial: MeshBasicNodeMaterial;     // Fullscreen-Shade-Pass
  backFaceMaterial: MeshBasicNodeMaterial;  // Shared für alle Back-Face-Meshes

  // Opcode-Buffers (uniformArray)
  opcodeBuf, opcodeData: Float32Array;     // Concatenated Opcode-Stream
  offsetBuf, offsetData: Float32Array;     // [start, length] pro Body

  // Material-Buffer (StorageBuffer, read-only)
  materialData: Float32Array;              // 12 floats pro Body
  materialAttr: StorageBufferAttribute;
  materialBuf: storage(...).toReadOnly();

  // Selection
  selectionBuf: uniformArray;              // 1.0 = selektiert, 0.0 = nicht
  selectionData: Float32Array;

  // Volume-Texturen (importierte Mesh-SDFs)
  volumeBaseBuf: uniformArray;             // Base-Index pro Body
  volumeBaseData: Float32Array;
  volumeTextures: Data3DTexture[];

  // Bitmask (Atomic Storage Buffer)
  bitmaskAttr: StorageBufferAttribute;     // uint32 pro Pixel
  bitmaskBuf: storage(...).toAtomic();
  clearNode: computeNode;                  // atomicStore(0) per Compute

  // Uniforms
  camPosUniform: uniform(Vector3);
  viewMatU: uniform(Matrix4);
  widthU: uniform(number);
  showEdgesU: uniform(number);

  // Body-Mesh-Verwaltung
  bodyOpaqueMeshes: Map<number, Mesh[]>;
  bodyTransparentMeshes: Map<number, Mesh[]>;
  bodyBackMeshes: Map<number, Mesh[]>;
  bodyOpcodeRanges: Map<number, { start, length }>;

  // Viewport-Größe + Rebuild-Flag
  width, height: number;
  needsShadeRebuild: boolean;
}
```

### Render Targets

| RT                  | Format         | Depth               | Inhalt                                       |
| ------------------- | -------------- | ------------------- | -------------------------------------------- |
| `opaqueRT`          | RGBA Float     | DepthTexture(Float) | `vec4(worldPos, bodyIndex+1)`                |
| `transparentRT`     | RGBA Float     | DepthTexture(Float) | `vec4(worldPos, bodyIndex+1)`                |
| `transparentBackRT` | RGBA HalfFloat | keins               | `vec4(scalarCamDist, 0, 0, 1)`, MAX-Blending |

### Materialien (4 Material-Typen)

**G-Buffer Material** (`createGBufferMaterial`):

* Output: `vec4(worldPos, bodyIndex+1)`
* FrontSide, depthWrite + depthTest

**Transparent G-Buffer Material** (`createTransparentGBufferMaterial`):

* Output: `vec4(worldPos, bodyIndex+1)`
* Zusätzlich: `atomicOr(bitmaskBuf[pixelIdx], 1u << bodyIdx)`
* FrontSide, depthWrite + depthTest

**Back-Face Material** (`createBackFaceMaterial`):

* Output: `vec4(length(worldPos - camPos), 0, 0, 1)`
* BackSide, kein depthTest/Write
* `CustomBlending` mit `MaxEquation` + `OneFactor` — fernste Rückseite gewinnt

**Shade Material** (`createShadeMaterial`):

* Fullscreen-Quad, liest alle RTs + Bitmask
* Enthält den VM-Interpreter (`buildVmSdfEvalFn`)
* Evaluiert SDF, berechnet Normalen, Cook-Torrance PBR, Edge Detection, Selection, Wireframe

### Pipeline (5 Passes)

1. **Pass 0 — Compute Clear**: `atomicStore(bitmaskBuf[i], 0)` per Compute Shader
2. **Pass 1 — Opaque Raster**: → `opaqueRT` (worldPos + bodyTag), mit Depth Buffer
3. **Pass 2 — Transparent Front**: → `transparentRT` (worldPos + bodyTag, FrontSide) + `atomicOr(bitmaskBuf[pixelIdx], 1u << bodyIdx)`
4. **Pass 2b — Back-Face Collect**: → `transparentBackRT` (scalar camDist, BackSide, MAX-Blending)
5. **Pass 3 — Unified Shade**: Fullscreen Quad, liest alle RTs + `bitmaskBuf` → Screen

***

## Shade-Logik (Pass 3)

### Fallunterscheidung

1. **Weder opak noch transparent** → Discard (Hintergrund)
2. **Nur opak (hasOpaque && !hasTrans)** → Shade am Mesh-Punkt:
   * 6 SDF-Evaluationen für Finite-Difference Normale
   * Cook-Torrance PBR + Edge Detection + Selection + Wireframe
3. **Transparent vorhanden (hasTrans)** → Multi-Body Sphere-Trace:

### Sphere-Trace-Logik (Fall B)

1. **tMin**: World-Position aus `transparentRT` (nächste transparente Vorderseite)
2. **tMax**: `backDist - frontDist` aus `transparentBackRT`.
   Optional verengt: Liegt ein opakes Objekt eines *anderen* Bodies dahinter,
   wird `tMax = min(tMax, opaqueDist)`. Same-Body-Opaques werden per
   Bitmask-Check erkannt und ignoriert.
3. **March-Loop** (`DEFERRED_MARCH_STEPS` = 512 Iterationen):
   - Pro Schritt: iteriere über alle 32 möglichen Bodies
   - Für jeden Body mit gesetztem Bitmask-Bit:
     - Lade Opcode-Offset aus `offsetBuf`
     - Evaluiere SDF via VM-Interpreter
     - Optional: addiere Surface Displacement (trigonometrisch per Material)
     - Tracke minimale Distanz + closest Body
   - Bei `d < SURFACE_EPS` → Hit, Break
   - `t += max(minD, SURFACE_EPS)`
   - Bei `t > tMax` → Break (kein Treffer)
4. **Ergebnis**:
   - Hit + opakes Objekt eines anderen Bodies liegt näher → Shade am opaken Punkt
   - Hit → Shade am Trefferpunkt
   - Kein Hit + opakes Objekt eines anderen Bodies → Shade am opaken Punkt
   - Kein Hit + kein anderer opaker Body → Discard (Rille/Lücke)

### Shading

Nach dem Hit wird der Punkt geschadet:

1. **Normale**: Finite Differences auf der SDF (6 Evaluationen, eps \= 0.01)
2. **PBR**: Cook-Torrance BRDF mit 3 Directional Lights + Ambient
   * Lights: `[0.2, 0.3, 1.0] × 3.0`, `[-0.5, 0.1, 0.6] × 0.5`, `[0.0, -0.2, -1.0] × 0.3`
   * Ambient: 0.25
   * GGX Distribution, Schlick-GGX Geometry, Schlick Fresnel
3. **Edge Detection** (optional, per `showEdgesU`): 6 weitere SDF-Evaluationen
   in eps\=0.5 Abstand, Laplacian-artige Summe → Smoothstep \[0.1, 0.5] → Mix mit Grau (0.12)
4. **Selection Highlighting**: Selektierte Bodies → Mix mit Blau `(0.541, 0.706, 0.973)` × 35%
5. **Wireframe Mode**: Fresnel-Edge + Grid-Pattern (Gittergröße 10, Schwelle 0.03)
6. **Surface Displacement**: Per-Body trigonometrische Displacement via Material-Properties
   `sin(x·freq) · sin(y·freq·1.3) · sin(z·freq·0.7) · depth`

***

## Material-System

Pro Body werden 12 floats im StorageBuffer gespeichert (`MAT_STRIDE = 12`):

| Offset | Feld           | Default                               |
| ------ | -------------- | ------------------------------------- |
| 0–2    | Color R/G/B    | Body-Farbpalette (8 Farben, zyklisch) |
| 3      | Roughness      | 0.35                                  |
| 4      | Metalness      | 0.0                                   |
| 5–7    | Emissive R/G/B | 0, 0, 0                               |
| 8      | Opacity        | 1.0                                   |
| 9      | Wireframe      | 0 (aus)                               |
| 10     | Displace Depth | 0                                     |
| 11     | Displace Freq  | 0                                     |

Materialien werden über `materialBridge.ts` aus dem Kernel geladen:
Feature-Typ `"material"` → `MaterialProps` → in den StorageBuffer geschrieben.

***

## SDF-Evaluation (VM-Interpreter)

Die SDF-Auswertung erfolgt durch `buildVmSdfEvalFn()` (`sdfVmInterpreter.ts`):

* **Stack-VM**: 8-tiefer Wert-Stack (vs0–vs7) + 4-tiefer Positions-Stack (ps0–ps3)
* **Opcode-Format**: Linearer Float-Stream in Reverse Polish Notation
* **Program Counter**: Inkrementiert sequentiell durch den Opcode-Buffer
* **Base Offset**: Für Deferred-Modus — springt pro Body an die richtige Stelle im concatenated Buffer

### Opcode-Buffer-Layout

Alle Bodies' Opcodes werden in einem einzigen `uniformArray` hintereinander gepackt.
Eine Offset-Tabelle (`offsetBuf`, 2 floats pro Body: `[start, length]`) erlaubt
dem Shade-Pass, das richtige Snippet zu laden.

**Opcode Buffer**: `[Body 0 Opcodes...][Body 1 Opcodes...][Body 2 Opcodes...]`

**Offset Table**: Body 0 → `start=0, len=24` | Body 1 → `start=24, len=18` | …

Packing erfolgt in `packOpcodes()` (`sdfPipelineCommon.ts`).

### Volume-Texturen (Mesh-SDF)

Importierte Meshes werden als 3D-Texturen (SDF-Voxelgitter) in den Shader übergeben:

* Max. 4 Texturen gleichzeitig (`MAX_VOLUME_TEXTURES`)
* Format: `RedFormat`, `FloatType`, `LinearFilter`
* `volumeBaseBuf` speichert pro Body den Start-Index in die Textur-Liste
* Ausgewertet via `OP_PRIM_TEX3D` im VM-Interpreter (trilineare Interpolation)

Wenn sich die Anzahl der Volume-Texturen ändert, wird das Shade-Material
komplett neu gebaut (`needsShadeRebuild = true` → `rebuildShadeMaterialIfNeeded()`).

***

## Change Detection & Compilation

Die `SdfDeferredOverlay`-Komponente (`viewportSdfOverlays.tsx`) trackt Änderungen
über ein Fingerprinting-System:

* **Structural Fingerprint** (`structFp`): Body-Count + Body-IDs + Opcode-Längen + Volume-Counts
* **Data Fingerprint** (`dataFp`): Body-Versionen (Parameter-Änderungen)

| Änderung                 | Reaktion                                                           |
| ------------------------ | ------------------------------------------------------------------ |
| `structFp` ändert sich   | Voller Rebuild: neue Meshes + Shader-Kompilation                   |
| Nur `dataFp` ändert sich | Daten-Update: Opcodes + Offsets + Mesh-Transforms (kein Recompile) |
| Keine Änderung           | Nur Rendern                                                        |

Shader-Kompilation ist asynchron (`compileDeferredPipeline`). Während der
Kompilation wird ein Overlay ("Compiling shader…") angezeigt, und der Renderer
rendert nicht.

***

## THREE.js Patches

Die WebGPU-Spec erlaubt `GPUBufferBindingType.Storage` (read\_write) in Fragment-Shadern,
aber THREE.js r183 blockiert dies. Zwei Patches in `web/patches/three+0.183.2.patch`:

1. **WGSLNodeBuilder.getNodeAccess()**: Warnung bei Atomics in Nicht-Compute-Stages entfernt
2. **WebGPUBindingUtils**: Respektiert `binding.access` für alle Stages (nicht nur Compute)

***

## Harte Architektur-Limits

| Limit                 | Wert                     | Ursache                                                |
| --------------------- | ------------------------ | ------------------------------------------------------ |
| Max. Bodies           | 32                       | Per-Pixel Bitmask \= `uint32` → 32 Bits \= 32 Bodies   |
| Max. Opcodes          | 4096                     | `uniformArray` (WebGPU uniform buffer, 64KB Minimum)   |
| Brute-Force pro Pixel | O(bodies × march\_steps) | March-Loop iteriert über alle Bitmask-Bits pro Schritt |
| Max. Viewport         | 4096 × 2160              | Bitmask-Buffer voralloziert für diese Auflösung        |
| Max. Volume-Texturen  | 4                        | Hardcoded im VM-Interpreter                            |

Diese Limits sind architektonische Constraints des brute-force per-pixel Ansatzes,
nicht Tuning-Parameter.