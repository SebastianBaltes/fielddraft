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

| Datei                                    | Rolle                                                            |
| ---------------------------------------- | ---------------------------------------------------------------- |
| `web/src/sdfDeferredRenderer.ts`         | Pipeline-Init, alle Passes, Body-Rebuild (voll typisiert)        |
| `web/src/sdfDeferredRenderer.shaders.ts` | TSL-Shader-Builder: G-Buffer-/Back-Face-/Shade-Material, Body-ID-Buffer |
| `web/src/sdfVmInterpreter.ts`            | GPU-bytecode VM — interpretiert Opcodes zur Laufzeit             |
| `web/src/sdfPipelineCommon.ts`           | Shared Buffers: Opcode-Packing, Material-Layout, Volume-Texturen, March-Konstanten, CameraUniformSet |
| `web/src/sdfPipelineCommon.shaders.ts`   | Geteilte TSL-Blöcke: `emitUnionMarch`, `emitSceneDepthWrite`, `makeDisplacedSdfEval`, `shadeSdfHit` |
| `web/src/sdfRenderer.ts`                 | SDF-Primitive-Funktionen (WGSL), Opcode-Konstanten               |
| `web/src/leafMeshFactory.ts`             | `createPrimitiveGeometry`, `leafGeoKey`, `applyLeafTransform` (Leaf-Hüll-Meshes) |
| `web/src/bodyColors.ts`                  | `BODY_COLORS` Part-Palette (SSOT)                                |
| `web/src/viewportSdfOverlays.tsx`        | React-Integration: `SdfDeferredOverlay` + `useSdfPipelineOverlay`-Hook |
| `web/src/pbrShading.ts`                  | Cook-Torrance PBR (3 Directional Lights + Ambient)               |
| `web/src/materialBridge.ts`              | Kernel → Pipeline Material-Mapping                               |
| `crates/kernel/src/bvh.rs`               | `classify_for_hybrid()` → `HybridInfo` (Rust-seitig)             |
| `crates/kernel/src/sdf_compiler.rs`      | CSG-Baum → flacher Opcode-Buffer (RPN)                           |

### Konstanten

| Konstante                 | Wert        | Bedeutung                                                  |
| ------------------------- | ----------- | ---------------------------------------------------------- |
| `SDF_MAX_OPCODES`         | 16384       | Max. Opcodes im concatenated Buffer (sdfPipelineCommon)    |
| `DEFERRED_MAX_BODIES`     | 256         | Max. Bodies für Buffer-Pre-Allokation                      |
| `SDF_MARCH_STEPS`         | 512         | Max. Sphere-Trace-Schritte pro Pixel (sdfPipelineCommon)   |
| `SURFACE_EPS`             | 0.002       | Konvergenz-Schwelle für Treffer (sdfPipelineCommon)        |
| `MARCH_START_BACKOFF`     | 1.0 mm      | March-Start-Rückversatz vor dem exakten Hüll-Mesh (vlhp); Hulls werden NICHT mehr aufgeblasen |
| `AABB_HULL_PAD`           | 0.5 mm      | Padding der AABB-Fallback-Hülle (sdfPipelineCommon)        |
| `MAX_PIXELS`              | 4096 × 2160 | Maximale Viewport-Auflösung für Body-ID-Buffer             |
| `BODY_ID_WORDS_PER_PIXEL` | 2           | uint32-Wörter pro Pixel im Body-ID-Buffer                  |
| `VM_MAX_OPCODES`          | 512         | Per-Body Opcode-Limit im VM-Interpreter (~35 Primitives)   |
| `MAX_VOLUME_TEXTURES`     | 4           | Max. 3D-Texturen für importierte Mesh-SDFs                 |
| `NORMAL_EPS`              | 0.01        | Epsilon für Finite-Difference Normalen                     |

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

  // Opcode-Buffers (StorageBuffer, read-only)
  opcodeAttr: StorageBufferAttribute;
  opcodeBuf, opcodeData: Float32Array;     // Concatenated Opcode-Stream
  offsetAttr: StorageBufferAttribute;
  offsetBuf, offsetData: Float32Array;     // [start, length] pro Body

  // Material-Buffer (StorageBuffer, read-only)
  materialData: Float32Array;              // 12 floats pro Body
  materialAttr: StorageBufferAttribute;
  materialBuf: storage(...).toReadOnly();

  // Selection
  selectionBuf: uniformArray;              // 1.0 = selektiert, 0.0 = nicht
  selectionData: Float32Array;

  // Volume-Texturen (importierte Mesh-SDFs, StorageBuffer)
  volumeBaseAttr: StorageBufferAttribute;
  volumeBaseBuf;                           // Base-Index pro Body
  volumeBaseData: Float32Array;
  volumeTextures: Data3DTexture[];

  // Body-ID Buffer (Atomic Storage Buffer, 2 uint32 pro Pixel)
  bodyIdAttr: StorageBufferAttribute;      // packed body IDs + count
  bodyIdBuf: storage(...).toAtomic();
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
* Zusätzlich: Packed Body-ID Insertion via `atomicAdd` + `atomicOr` in den `bodyIdBuf`
* FrontSide, depthWrite + depthTest

**Back-Face Material** (`createBackFaceMaterial`):

* Output: `vec4(length(worldPos - camPos), 0, 0, 1)`
* BackSide, kein depthTest/Write
* `CustomBlending` mit `MaxEquation` + `OneFactor` — fernste Rückseite gewinnt

**Shade Material** (`createShadeMaterial`):

* Fullscreen-Quad, liest alle RTs + Body-ID-Buffer
* Enthält den VM-Interpreter (`buildVmSdfEvalFn`)
* March-Loop iteriert nur über die 1–2 tatsächlich vorhandenen Bodies pro Pixel (statt über alle Bodies)
* Evaluiert SDF, berechnet Normalen, Cook-Torrance PBR, Edge Detection, Selection, Wireframe

### Pipeline (5 Passes)

1. **Pass 0 — Compute Clear**: `atomicStore(bodyIdBuf[i], 0)` per Compute Shader (2 Wörter pro Pixel)
2. **Pass 1 — Opaque Raster**: → `opaqueRT` (worldPos + bodyTag), mit Depth Buffer
3. **Pass 2 — Transparent Front**: → `transparentRT` (worldPos + bodyTag, FrontSide) + Body-ID-Insertion in `bodyIdBuf`
4. **Pass 2b — Back-Face Collect**: → `transparentBackRT` (scalar camDist, BackSide, MAX-Blending)
5. **Pass 3 — Unified Shade**: Fullscreen Quad, liest alle RTs + `bodyIdBuf` → Screen

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
   Body-ID-Vergleich erkannt und ignoriert.
3. **March-Loop** (`DEFERRED_MARCH_STEPS` = 512 Iterationen):
   - Pro Schritt: evaluiere nur die 1–2 Bodies aus dem Body-ID-Buffer
   - Für jeden Body (via `evalBodySdf`-Helper):
     - Lade Opcode-Offset aus `offsetBuf`
     - Evaluiere SDF via VM-Interpreter
     - Optional: addiere Surface Displacement (trigonometrisch per Material)
     - Tracke minimale Distanz + closest Body
   - Zwei unrollte Aufrufe: `transId0` (immer), `transId1` (nur wenn `transCount >= 2`)
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

Alle Bodies' Opcodes werden in einem einzigen Storage Buffer hintereinander gepackt.
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

Die Overlay-Komponenten (`viewportSdfOverlays.tsx`) tracken Änderungen
über ein dreistufiges Fingerprinting-System:

* **Volume Fingerprint** (`volumeFp`): Gesamtzahl der Volume-Texturen — nur Änderungen hier erzwingen Shader-Recompile (Volume-Texturen sind in den Shader eingebaut)
* **Structural Fingerprint** (`structFp`): Body-Count + Body-IDs + Opcode-Längen + Volume-Counts
* **Data Fingerprint** (`dataFp`): Body-Versionen (Parameter-Änderungen)

| Änderung                 | Reaktion (VM Overlay)                                              | Reaktion (Deferred Overlay)                    |
| ------------------------ | ------------------------------------------------------------------ | ---------------------------------------------- |
| `volumeFp` ändert sich   | Voller Rebuild + Shader-Kompilation                                | Rebuild + Shader-Kompilation (via needsShadeRebuild) |
| `structFp` ändert sich   | Daten-Update: Opcodes + Offsets (kein Recompile)                   | Voller Rebuild: neue Meshes + Kompilation      |
| Nur `dataFp` ändert sich | Daten-Update: Opcodes + Offsets                                    | Daten-Update: Opcodes + Offsets + Mesh-Transforms |
| Keine Änderung           | Nur Rendern                                                        | Nur Rendern                                    |

Das Hinzufügen/Entfernen von Bodies oder Ändern von Opcodes löst im VM-Modus nur ein schnelles Buffer-Update aus, keinen Shader-Recompile — es sei denn, die Anzahl der Volume-Texturen ändert sich.

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

## Per-Pixel Body-ID-Buffer

### Encoding

Statt einer Bitmask (altes Konzept: 1 Bit pro Body, max 32) speichert der
Body-ID-Buffer 2 × `uint32` pro Pixel mit gepackten 16-Bit Body-IDs:

| Wort   | Bits 31–16       | Bits 15–0          |
| ------ | ---------------- | ------------------- |
| Word 0 | Count (0, 1, 2+) | Body-ID 0 (1-based) |
| Word 1 | (reserviert)     | Body-ID 1 (1-based) |

Body-IDs sind 1-based (0 = leer). Pro Frame werden dichte Indizes vergeben
(keine UUIDs), sodass 16 Bit für bis zu 65534 Bodies reichen.

### Atomare Schreiblogik (Fragment-Shader)

Jeder transparente Fragment-Shader fügt seine Body-ID per lock-free Atomics ein:

1. `atomicAdd(word0, 0x10000)` — inkrementiert Count im oberen Halbwort, gibt alten Wert zurück
2. Aus dem alten Wert wird `oldCount = (old >> 16) & 0xFFFF` extrahiert
3. `oldCount == 0` → `atomicOr(word0, encodedId)` — ID in unteres Halbwort von Word 0
4. `oldCount == 1` → `atomicOr(word1, encodedId)` — ID in unteres Halbwort von Word 1
5. `oldCount >= 2` → Drop (kein Platz, Overflow wird ignoriert)

Kein CAS nötig: Jedes ID-Slot-Halbwort wird genau einmal von 0 auf einen
Wert ≠ 0 gesetzt. `atomicOr` mit einem Wert ≤ 0xFFFF lässt das Count-Feld
in Word 0 unverändert.

### Auslesen (Shade-Pass)

    word0      = atomicLoad(bodyIdBuf[pixelIdx * 2])
    word1      = atomicLoad(bodyIdBuf[pixelIdx * 2 + 1])
    transCount = min((word0 >> 16) & 0xFFFF, 2)
    transId0   = word0 & 0xFFFF
    transId1   = word1 & 0xFFFF

Die March-Loop evaluiert dann `transId0` (immer, wenn `transCount >= 1`) und
`transId1` (wenn `transCount >= 2`) — O(present) statt O(max).

### Overflow

Die aktuelle Implementierung unterstützt max. 2 transparente Bodies pro Pixel.
Der dritte und weitere Bodies werden ignoriert. In der Praxis reicht das,
weil transparente Überlappung (Fillets, Booleans, Ränder) typischerweise nur
2 Bodies betrifft. Eine Overflow-Liste (zusätzlicher Storage Buffer) ist
architektonisch vorgesehen, aber nicht implementiert.

***

## Harte Architektur-Limits

| Limit                     | Wert                    | Ursache                                                      |
| ------------------------- | ----------------------- | ------------------------------------------------------------ |
| Max. Bodies (Szene)       | 256 (Buffer-Allokation) | `MAX_BODIES` Konstante, frei erhöhbar                        |
| Max. Bodies (per Pixel)   | 65534                   | 16-Bit Body-ID-Encoding (1-based, 0 = leer)                 |
| Transparente Bodies/Pixel | 2                       | 2 Slots im Body-ID-Buffer (Overflow nicht implementiert)     |
| Max. Opcodes              | 16384                   | `StorageBufferAttribute` (WebGPU storage buffer)             |
| Max. Opcodes/Body (VM)    | 512                     | GPU-Loop-Limit im VM-Interpreter (~35 Primitives/Body)       |
| March-Kosten pro Pixel    | O(present × steps)      | Loop evaluiert nur die 1–2 tatsächlich vorhandenen Bodies    |
| Max. Viewport             | 4096 × 2160             | Body-ID-Buffer voralloziert für diese Auflösung              |
| Max. Volume-Texturen      | 4                       | Hardcoded im VM-Interpreter                                  |

`MAX_BODIES` ist ein Buffer-Allokations-Limit, kein algorithmisches. Die
per-Pixel-Kosten sind unabhängig von der Gesamtzahl der Bodies in der Szene.