# Deferred Raymarching — Architekturkonzept

## Kapitel 1: Das abstrakte Konzept

### Problemstellung

Ein SDF-Renderer (Signed Distance Field) berechnet für jeden Pixel eines Bildes
die Oberfläche mathematisch definierter Körper durch iteratives Marschieren
entlang eines Sichtstrahls (Sphere Tracing). Die Kosten pro Pixel sind
proportional zu:

- **Anzahl der Marching-Schritte** (typisch 64–96)
- **Komplexität der SDF-Auswertung** pro Schritt (Anzahl der CSG-Terme)

Der Großteil dieser Rechenzeit wird verschwendet: Der Strahl marschiert durch
leeren Raum, und an Pixeln, an denen ein einfacher Quader liegt, wird dieselbe
teure SDF-Schleife durchlaufen wie an Pixeln mit komplexen Bohrungen oder
Fillets.

### Die Grundidee

Die klassische Dreiecks-Rasterisierung löst das Sichtbarkeitsproblem
(welches Objekt ist an einem Pixel vorne?) in Hardware-Geschwindigkeit.
SDF-Raymarching löst das Oberflächenproblem (wie sieht die mathematisch
exakte Oberfläche aus?) mit beliebiger Präzision.

**Deferred Raymarching** kombiniert beide: Die Rasterisierung wird als
reine Vorverarbeitung genutzt, um pro Pixel exakt zu bestimmen, *wo* und
*mit welchen SDF-Termen* der Raymarcher arbeiten muss. Der Raymarcher
selbst erzeugt dann das finale Bild — auch für einfache Körper, dort aber
mit minimal einem einzigen Auswertungsschritt.

### Hüll-Meshes

Jeder SDF-Term (oder jede Gruppe zusammengehöriger Terme) bekommt ein
**Hüll-Mesh**: ein Dreiecksnetz, das den Raumbereich, in dem der Term
gilt, vollständig umschließt. Hüll-Meshes werden um einen kleinen Faktor
(**HULL_INFLATE = 2%**) aufgeblasen, um sicherzustellen, dass die
tessellierte Polygon-Oberfläche die mathematische SDF-Fläche an allen
Stellen überdeckt — insbesondere an Silhouetten-Kanten, wo die
Polygon-Sehne kürzer ist als der Kreisbogen:

- Ein Quader-Term bekommt ein leicht aufgeblasenes Quader-Mesh.
- Ein Kugel-Term bekommt eine SphereGeometry (32 Segmente) × 1.02.
- Ein Fillet-Term (smin-Übergangszone) bekommt ein Mesh, das entlang der
  betroffenen Kante extrudiert ist und den Blend-Radius einschließt.
- Ein Gyroid-Term bekommt ein Mesh (AABB-Box + Padding), das das gesamte
  Volumen des Gyroids umschließt.

### Opak und Transparent

Hüll-Meshes werden in zwei Klassen eingeteilt:

**Opake Hüll-Meshes** repräsentieren Raumbereiche, in denen ein einziger
SDF-Term die Oberfläche vollständig bestimmt. Da die Meshes leicht
aufgeblasen sind (HULL_INFLATE), liegt der Startpunkt des Rays knapp
*außerhalb* der mathematischen Oberfläche. Der Sphere-Tracer konvergiert
in 1–3 Schritten von der aufgeblasenen Mesh-Position zur echten
SDF-Oberfläche. Für volumetrische Bodies (Gyroid, Thread) werden Rays,
die keine SDF-Oberfläche finden, verworfen (discard) — so entstehen
korrekte Durchsichten und Transparenzen.

**Transparente Hüll-Meshes** repräsentieren Raumbereiche, in denen mehrere
SDF-Terme zusammenwirken müssen (Booleans, Fillets, Domain Warps). An Pixeln,
die von transparenten Meshes getroffen werden, muss der Raymarcher die
zugehörigen SDF-Terme tatsächlich evaluieren. Das transparente Mesh definiert
dabei:

- Welche SDF-Terme für diesen Pixel relevant sind (über die Mesh-ID)
- Den Startpunkt des Rays (Front-Face-Tiefe des Hüll-Meshes)
- Den Endpunkt des Rays (Tiefe des nächsten opaken Objekts dahinter)

### Per-Pixel SDF-Term-Set

Durch die Rasterisierung aller Hüll-Meshes entsteht pro Pixel ein **Set der
relevanten SDF-Terme**. Dieses Set ist minimal: Es enthält nur die Terme,
deren Hüll-Meshes den Pixel tatsächlich treffen. Der Raymarcher baut die
SDF-Auswertungsfunktion zur Laufzeit aus genau diesen Termen zusammen.

Die Reihenfolge der Auswertung folgt dabei der CSG-Baumstruktur: Werden
die Terme A und B durch `Difference(A, B)` verknüpft, evaluiert der
Raymarcher `max(sdf_A, -sdf_B)`. Die CSG-Beziehungen sind statisch bekannt
(sie ändern sich nur bei Topologie-Änderungen) und werden als linearer
Opcode-Stream an die GPU übergeben.

### Korrektheit

Das zentrale Korrektheitskriterium: An jedem Pixel, an dem ein transparentes
Hüll-Mesh getroffen wird, muss dessen Opcode-Snippet **alle** SDF-Terme
enthalten, die für die korrekte Oberflächenberechnung in diesem Raumbereich
nötig sind. Außerhalb des Hüll-Meshes ist per Definition der opake Körper
(oder der Hintergrund) die korrekte Oberfläche.

### Performance-Garantien

1. **Kein leerer Raum**: Der Raymarcher startet erst am Hüll-Mesh, nicht an
   der Kamera. Typische Einsparung: 50–90% der Marching-Schritte.
2. **Minimale SDF-Terme**: Pro Pixel werden nur die tatsächlich relevanten
   Terme evaluiert, nicht der gesamte CSG-Baum der Szene.
3. **Einfache Körper sind fast kostenlos**: Durch die 2% Hull-Inflation
   startet der Ray knapp außerhalb der Oberfläche. Sphere-Tracing konvergiert
   in 1–3 Schritten (+ 6 für Normale) = 7–9 SDF-Evaluationen pro Pixel.
4. **Hardware-Culling**: Die GPU verwirft transparente Hüll-Meshes hinter
   opaken Oberflächen automatisch per Z-Test (Early-Z).
5. **Transparenz ohne Sorting**: Volumetrische Bodies (Gyroid, Thread)
   benötigen keine spezielle Transparenz-Behandlung — Rays die keine
   SDF-Oberfläche treffen werden einfach discardet.

---

## Kapitel 2: GPU-Implementierung

### Pipeline-Architektur

Die Rendering-Pipeline besteht aus drei sequentiellen Passes:

```
Pass 1: OPAQUE RASTER
  Input:  Opake Hüll-Meshes (Vertex + Index Buffers)
  Output: Color Buffer, Depth Buffer (t_max), Object-ID Buffer

Pass 2: TRANSPARENT COLLECT
  Input:  Transparente Hüll-Meshes
  Reads:  Depth Buffer aus Pass 1 (für Z-Test)
  Output: Per-Pixel Term-ID Set (Storage Buffer), t_min Buffer

Pass 3: UNIFIED RAYMARCH (Compute Shader)
  Input:  t_min, t_max, Term-ID Set, Opcode Buffer, Camera
  Output: Final Color Buffer, Final Depth Buffer
```

### Pass 1: Opaque Raster

Standard-Rasterisierung mit Z-Buffer. Jedes opake Hüll-Mesh schreibt:

- **Depth Buffer** (`depth32float`): Die Tiefe der nächsten opaken Oberfläche.
  Dieser Wert wird zum `t_max` für den Raymarcher — weiter als diese Tiefe
  muss kein Strahl marschieren, weil ein opakes Objekt dahinter liegt.
- **ID Buffer** (`r32uint`): Die Object-ID des opaken Meshes. Der Raymarcher
  nutzt diese ID, um das zugehörige Opcode-Snippet zu laden.

Der Fragment-Shader dieses Passes ist trivial: Er gibt die Object-ID aus.
Beleuchtung wird erst im Raymarching-Pass berechnet (Unified Lighting).

### Pass 2: Transparent Collect

Die transparenten Hüll-Meshes werden gegen den Z-Buffer aus Pass 1 gerastert.
Konfiguration: **Z-Test AN, Z-Write AUS**. Transparente Meshes, die komplett
hinter einem opaken Objekt liegen, werden von der Hardware per Early-Z
verworfen — ohne jegliche Shader-Kosten.

Der Fragment-Shader sammelt die Term-IDs aller transparenten Meshes, die einen
Pixel treffen, in einem Per-Pixel Storage Buffer. Gleichzeitig wird der
Tiefenwert der Front-Face des vordersten transparenten Meshes als `t_min`
gespeichert.

#### Per-Pixel Term-ID Encoding

Für typische CAD-Szenen mit wenigen räumlichen Überlappungen reicht ein
kompaktes Encoding in einem einzelnen `u32` pro Pixel:

```
Bit-Layout: [CC | IIIIIIIIII | IIIIIIIIII | IIIIIIIIII]
             2b      10b          10b          10b

CC:   2-Bit Counter (0–3): Anzahl belegter Slots
I×3:  3 × 10-Bit Object-IDs (bis zu 1024 verschiedene Terme)
```

Der Fragment-Shader nutzt `atomicAdd` auf den Counter, um sich einen Slot zu
reservieren, und `atomicOr`, um die ID in den richtigen 10-Bit-Bereich zu
schreiben. Da der Counter als Guard fungiert, gibt es keine Race Conditions.

Für Szenen mit mehr als 3 überlappenden transparenten Bereichen pro Pixel kann
ein Overflow-Mechanismus ergänzt werden (Linked-List-Extension über einen
separaten Storage Buffer). Für CAD ist das selten nötig.

#### t_min-Erfassung

Der `t_min`-Wert (Startpunkt des Raymarching) ergibt sich aus dem Z-Buffer der
Front-Faces der transparenten Hüll-Meshes. Dafür gibt es zwei Optionen:

- **Separater Depth-Buffer** für Pass 2 (einfach, erfordert zweites
  Depth-Attachment)
- **atomicMin** auf einen `r32float`-Storage-Buffer (flexibler, kein
  zweites Attachment nötig, aber Floating-Point-Atomics erfordern
  Bitcast-Tricks: `atomicMin` auf `u32(bitcast<u32>(depth))` funktioniert,
  weil IEEE 754 Floats für positive Werte dieselbe Ordnung wie Integers haben)

### Pass 3: Unified Raymarch

Ein Compute Shader verarbeitet jeden Pixel des Bildes. Pro Pixel liest er:

1. Den **Opaque-ID** aus Pass 1 (oder "kein Treffer" wenn kein opakes Mesh)
2. Den **t_max** aus dem Depth Buffer von Pass 1
3. Die **Term-ID-Liste** aus dem Storage Buffer von Pass 2
4. Den **t_min** aus Pass 2 (oder 0 wenn kein transparenter Treffer)

#### Fall A: Nur opaker Treffer (kein transparentes Mesh)

Der Raymarcher lädt das Opcode-Snippet der Opaque-ID, evaluiert die SDF einmal
am Trefferpunkt, berechnet die Normale per Finite Differences (6 weitere
SDF-Evaluationen), und schreibt Farbe + Tiefe. Kosten: **7 SDF-Evaluationen
pro Pixel** (1 Oberfläche + 6 Normale), unabhängig von der Szenen-Komplexität.

#### Fall B: Transparenter Treffer

Der Raymarcher baut die SDF-Auswertungsfunktion aus den Term-IDs zusammen.
Der Strahl startet bei `t_min` und marschiert bis maximal `t_max`.
Die Schrittzahl ist durch das enge `[t_min, t_max]`-Intervall dramatisch
reduziert gegenüber einem naiven Ansatz (Start bei Kamera, Ende bei MAX_DIST).

#### Fall C: Kein Treffer

Pixel ohne opaken oder transparenten Treffer werden mit der Hintergrundfarbe
gefüllt. Kosten: null SDF-Evaluationen.

### Stack-VM für SDF-Auswertung

Die SDF-Auswertung im Compute Shader erfolgt durch eine Stack-basierte
Virtuelle Maschine (Stack-VM), die einen linearen Opcode-Stream interpretiert:

```
Opcode-Stream (Reverse Polish Notation):
  [PUSH_BOX, transform, params]  → evaluiert Box-SDF, pusht Distanz
  [PUSH_SPHERE, transform, params] → evaluiert Sphere-SDF, pusht Distanz
  [OP_DIFFERENCE]                → poppt 2 Distanzen, pusht max(a, -b)
  [OP_END]                       → finales Ergebnis liegt auf Stack

Hardware-Eigenschaften:
  - Stack lebt in GPU-Registern (kein Speicherzugriff)
  - Opcode-Stream wird linear gelesen (perfektes Prefetching)
  - Keine Rekursion, keine Pointer, keine Branches über Datenstrukturen
  - Kann optional via Workgroup Shared Memory vorladen
```

### Concatenated Opcode Buffer

Alle Opcode-Snippets aller Terme werden hintereinander in einem einzigen
Storage Buffer abgelegt. Eine Offset-Tabelle (ein Array von `[start, length]`
pro Term-ID) erlaubt dem Compute Shader, das richtige Snippet zu laden:

```
Opcode Buffer:
  [Snippet 0: Box-Opcodes...] [Snippet 1: Sphere-Opcodes...]
  [Snippet 2: Difference(Box,Sphere)-Opcodes...] ...

Offset Table:
  Term 0 → offset=0,   length=12
  Term 1 → offset=12,  length=10
  Term 2 → offset=22,  length=24
  ...
```

Der Compute Shader setzt den Program Counter der Stack-VM auf den Offset des
gewünschten Snippets und evaluiert bis `OP_END`.

### Unified Lighting

Da alle Pixel — opak und transparent — durch denselben Compute Shader laufen,
gibt es genau einen Beleuchtungs-Code-Pfad. Die Normale wird überall per
Finite Differences auf der SDF berechnet:

```
n = normalize(vec3(
  sdf(p + (eps,0,0)) - sdf(p - (eps,0,0)),
  sdf(p + (0,eps,0)) - sdf(p - (0,eps,0)),
  sdf(p + (0,0,eps)) - sdf(p - (0,0,eps))
))
```

Vorteile:
- Konsistente Beleuchtung über die gesamte Szene
- Raytraced Shadows möglich (Schattenstrahl mit derselben SDF)
- Keine Duplikation von Shading-Code zwischen Mesh- und SDF-Pfad

### Speicherbedarf

Für eine Viewport-Auflösung von 1920×1080:

| Buffer | Format | Größe |
|--------|--------|-------|
| Depth (Pass 1) | `depth32float` | 7.9 MB |
| Opaque-ID (Pass 1) | `r32uint` | 7.9 MB |
| Term-ID Set (Pass 2) | `r32uint` | 7.9 MB |
| t_min (Pass 2) | `r32float` | 7.9 MB |
| Final Color (Pass 3) | `rgba8unorm` | 7.9 MB |
| Final Depth (Pass 3) | `r32float` | 7.9 MB |
| **Gesamt** | | **~48 MB** |

Dazu: Opcode Buffer + Offset Table (typisch <100 KB für CAD-Szenen).

---

## Kapitel 3: Implementierung in FieldDraft

### Bestandsaufnahme: Was bereits existiert

| Baustein | Status | Datei |
|----------|--------|-------|
| Stack-VM SDF-Interpreter | Fertig | `sdfVmInterpreter.ts` |
| VM_MAX_OPCODES = 2048 | Fertig | `sdfVmInterpreter.ts` |
| Opcode-Kompilierung (CSG → RPN) | Fertig | `sdf_compiler.rs` |
| 14 Primitiv-Opcodes + 6 CSG-Ops + 6 Warps | Fertig | `sdf_compiler.rs`, `sdfRenderer.ts` |
| CSG-Baum-Klassifikation (Hybrid) | Fertig | `bvh.rs: classify_for_hybrid()` |
| Primitive Mesh Factory | Fertig | `sdfHybridRenderer.ts` |
| Clip-Material (AABB-Discard) | Fertig | `sdfHybridRenderer.ts` |
| Compute Dispatch + StorageTexture | Fertig | `sdfHybridComputeRenderer.ts` |
| Blit-Composite (Fullscreen Quad) | Fertig | `sdfHybridComputeRenderer.ts` |
| Camera-Encoding (16-Float uniformArray) | Fertig | `sdfBatchmarch.ts` |
| Per-Body Farb-Palette (8 Farben) | Fertig | `meshMaterial.ts` |

### Was neu gebaut werden muss

#### 3.1 Render-Mode `sdf-deferred`

`web/src/SettingsMenu.tsx`: RenderMode-Typ erweitern um `"sdf-deferred"`.

`web/src/Viewport.tsx`: Neue Komponente `SdfDeferredOverlay`, analog zu den
bestehenden Overlay-Komponenten. Routing:
```
{renderMode === "sdf-deferred" && <SdfDeferredOverlay />}
```

#### 3.2 Hüll-Mesh-Klassifikation (Rust)

`crates/kernel/src/bvh.rs`: Die bestehende `classify_for_hybrid()` liefert
bereits `HybridLeafInfo` (meshbare Primitive) und `SdfRegionInfo`
(Overlap-AABBs). Für `sdf-deferred` muss sie erweitert werden:

```rust
pub struct DeferredMeshInfo {
    pub opaque_meshes: Vec<OpaqueMeshInfo>,
    pub transparent_meshes: Vec<TransparentMeshInfo>,
    pub opcode_snippets: Vec<OpcodeSnippet>,
}

pub struct OpaqueMeshInfo {
    pub term_id: u32,
    pub kind: PrimitiveKind,      // box, sphere, cylinder, ...
    pub transform: [f32; 8],      // position + quaternion + scale
    pub params: Vec<f32>,         // primitive-spezifische Parameter
}

pub struct TransparentMeshInfo {
    pub term_id: u32,
    pub mesh_kind: TransparentMeshKind,  // fillet_extrusion, boolean_zone, ...
    pub aabb: [f32; 6],                  // Fallback bis exakte Meshes existieren
    pub transform: [f32; 8],
}

pub struct OpcodeSnippet {
    pub term_id: u32,
    pub opcodes: Vec<f32>,        // RPN-Opcode-Stream für diesen Term
}
```

Die Klassifikation folgt dem CSG-Baum:

| CSG-Knoten | Opake Meshes | Transparente Meshes |
|------------|-------------|---------------------|
| Leaf(Box) | 1× BoxMesh | — |
| Leaf(Sphere) | 1× IkosaederMesh | — |
| Union(A, B) | Meshes von A + B | — |
| Difference(A, B) | Meshes von A (mit Clip) | 1× Overlap-Zone (A∩B) |
| SmoothUnion(A, B, r) | Meshes von A + B (mit Clip) | 1× Blend-Zone (A∩B expanded um r) |
| Twist/Bend/Shell | — | 1× Volumen-Mesh (gesamter Subtree) |
| Extrude(Sketch) | — (oder DC-Mesh) | 1× Extrusions-Volumen |

Jede transparente Zone bekommt ihr eigenes Opcode-Snippet, das den
**vollständigen CSG-Subtree** enthält, der für korrekte SDF-Auswertung in
dieser Zone nötig ist.

#### 3.3 WASM API

`crates/kernel/src/lib.rs`: Neuer Endpoint:

```rust
#[wasm_bindgen(js_name = bodyDeferredInfoJson)]
pub fn body_deferred_info_json(&self, i: usize) -> String
```

Gibt `DeferredMeshInfo` als JSON zurück. Wird nur bei `bodyVersion`-Änderung
aufgerufen.

#### 3.4 TypeScript: Pipeline-State

`web/src/sdfDeferredRenderer.ts` (neues Modul):

```typescript
export interface DeferredPipelineState {
  // Pass 1: Opaque Raster
  opaqueRT: THREE.WebGPURenderTarget;    // Color + Depth
  opaqueIdTex: StorageTexture;           // R32Uint Object-ID
  opaqueScene: THREE.Scene;
  opaqueCamera: THREE.PerspectiveCamera;

  // Pass 2: Transparent Collect
  termIdBuffer: StorageTexture;          // R32Uint Per-Pixel Term-ID Set
  tMinBuffer: StorageTexture;            // R32Float t_min

  // Pass 3: Unified Raymarch
  opcodeBuffer: ReturnType<typeof uniformArray>;
  opcodeData: Float32Array;
  offsetTable: ReturnType<typeof uniformArray>;
  offsetData: Float32Array;
  cameraBuf: ReturnType<typeof uniformArray>;
  cameraData: Float32Array;
  marchNode: any;                        // Compute Node

  // Blit
  blitScene: THREE.Scene;
  blitCamera: THREE.OrthographicCamera;
  blitMesh: THREE.Mesh;

  width: number;
  height: number;
}
```

#### 3.5 Pass 1: Opaque Raster

Rendering in ein `WebGPURenderTarget` mit Depth-Attachment:

```typescript
const opaqueRT = new THREE.WebGPURenderTarget(width, height, {
  depthTexture: new THREE.DepthTexture(width, height, THREE.FloatType),
});
```

Opake Meshes verwenden ein Material, das nur die Object-ID ausgibt:

```typescript
const idMaterial = new MeshBasicNodeMaterial();
idMaterial.colorNode = vec4(float(termId) / 1024.0, 0, 0, 1);
idMaterial.depthWrite = true;
idMaterial.depthTest = true;
```

Die Geometrie-Erzeugung nutzt die bestehende Primitive Mesh Factory aus
`sdfHybridRenderer.ts` (Box → BoxGeometry, Sphere → SphereGeometry, etc.).

#### 3.6 Pass 2: Transparent Collect

Transparente Hüll-Meshes werden gegen den Z-Buffer aus Pass 1 gerastert.
In Phase 1 verwenden transparente Meshes BoxGeometry über der Overlap-AABB
als Fallback. Spätere Phasen ersetzen diese durch enganliegende Meshes.

Der Fragment-Shader schreibt per `atomicOr` in den Term-ID Storage Buffer.
Für die `t_min`-Erfassung wird ein separater Storage Buffer mit
`atomicMin` (via Bitcast-Trick für Floats) verwendet.

Die Herausforderung in Three.js/TSL: `atomicOr` auf Storage Buffern ist
in TSL r183 noch nicht direkt exponiert. Mögliche Wege:

- **Custom WGSL via `wgslFn()`**: Inline-WGSL-Funktion für die atomare
  Operation, aufgerufen aus dem TSL-Fragment-Shader
- **Compute-Pass statt Fragment-Pass**: Transparente Meshes ebenfalls
  rasterisieren, aber die ID-Collection als separaten Compute-Dispatch
  implementieren, der die Depth-Textur liest

#### 3.7 Pass 3: Unified Raymarch (Compute Shader)

Der Compute Shader folgt dem Muster aus `sdfBatchmarch.ts`, erweitert um
die Per-Pixel Term-ID-Auswertung:

```typescript
const marchKernel = Fn(({ opcBuf, offsetBuf, termIdTex,
                          tMinTex, depthTex, camBuf, outColor, outDepth }) => {
  const w = uint(width);
  const px = instanceIndex.mod(w);
  const py = instanceIndex.div(w);

  // Term-IDs lesen und entpacken
  const packed = textureLoad(termIdTex, uvec2(px, py)).x;
  const count = packed.shr(30).bitAnd(3);
  const id0 = packed.bitAnd(0x3FF);
  const id1 = packed.shr(10).bitAnd(0x3FF);
  const id2 = packed.shr(20).bitAnd(0x3FF);

  // t_min und t_max lesen
  const tMin = textureLoad(tMinTex, uvec2(px, py)).x;
  const tMax = textureLoad(depthTex, uvec2(px, py)).x;

  // Ray bauen (Camera-Encoding wie sdfBatchmarch.ts)
  const { origin: ro, dir: rd } = buildRay({ px, py, camBuf });

  // Fall A: Nur opak, kein transparenter Treffer
  If(count.equal(0), () => {
    // 1 SDF-Eval + 6 für Normale → Farbe schreiben
  });

  // Fall B: Transparente Terme → Raymarch [tMin, tMax]
  // VM evaluiert Opcode-Snippet ab offsetBuf[termId]
});
```

#### 3.8 Concatenated Opcode Buffer

Alle Opcode-Snippets werden CPU-seitig in einen großen `Float32Array`
konkateniert. Die Offset-Tabelle wird als zweiter `uniformArray` übergeben:

```typescript
function buildConcatenatedOpcodes(
  snippets: OpcodeSnippet[]
): { opcodeData: Float32Array; offsetData: Float32Array } {
  let totalLen = 0;
  for (const s of snippets) totalLen += s.opcodes.length;

  const opcodeData = new Float32Array(totalLen);
  const offsetData = new Float32Array(snippets.length * 2); // [offset, length] pro Snippet

  let cursor = 0;
  for (let i = 0; i < snippets.length; i++) {
    offsetData[i * 2] = cursor;
    offsetData[i * 2 + 1] = snippets[i].opcodes.length;
    opcodeData.set(snippets[i].opcodes, cursor);
    cursor += snippets[i].opcodes.length;
  }

  return { opcodeData, offsetData };
}
```

Die bestehende `buildVmSdfEvalFn(opBuf)` bekommt einen zusätzlichen
`baseOffset`-Parameter, damit der Program Counter nicht bei 0, sondern beim
Snippet-Offset startet.

#### 3.9 Viewport-Komponente

```typescript
function SdfDeferredOverlay() {
  const pipelineRef = useRef<DeferredPipelineState | null>(null);
  const { store } = useKernelStore();

  useFrame(({ gl, camera, invalidate }) => {
    const renderer = gl as THREE.WebGPURenderer;
    const k = store!.kernel();

    // Pipeline initialisieren (einmalig oder bei Resize)
    if (!pipelineRef.current) {
      pipelineRef.current = initDeferredPipeline(width, height);
    }
    const pipeline = pipelineRef.current;

    // Bodies aktualisieren (bei Version-Änderung)
    for (let i = 0; i < k.bodyCount(); i++) {
      const info = k.bodyDeferredInfo(i);
      updateDeferredBody(pipeline, info);
    }

    // Camera-Uniforms aktualisieren
    updateDeferredCamera(pipeline, camera);

    // Pass 1: Opaque Raster
    renderer.setRenderTarget(pipeline.opaqueRT);
    renderer.render(pipeline.opaqueScene, camera);

    // Pass 2: Transparent Collect
    // (Compute oder Fragment — siehe 3.6)

    // Pass 3: Unified Raymarch
    renderer.compute(pipeline.marchNode);

    // Blit
    renderer.setRenderTarget(null);
    renderer.render(pipeline.blitScene, pipeline.blitCamera);
  });
}
```

### Phasenplan

#### Phase 1: Opaque-Only Pipeline

Nur Pass 1 + Pass 3 (ohne transparente Meshes). Alle Bodies als opake
Hüll-Meshes rendern. Der Compute Shader evaluiert die SDF einmal pro Pixel
für Normalen und Beleuchtung. Keine Boolean-Korrektheit, aber:

- Validiert die Offscreen-RT → Compute → Blit Pipeline
- Validiert Camera-Encoding und Ray Construction
- Validiert Concatenated Opcode Buffer + Offset Table
- Vergleichbar mit bestehendem `sdf-hybrid` für Union-only Szenen

#### Phase 2: Transparent Collect + Raymarch

Pass 2 hinzufügen. Transparente Hüll-Meshes (zunächst als BoxGeometry über
Overlap-AABBs) sammeln Term-IDs. Der Compute Shader marschiert in
`[t_min, t_max]` mit den korrekten Opcode-Snippets. Boolean-Korrektheit
für Difference, Intersection, SmoothUnion/Diff.

#### Phase 3: Enganliegende Hüll-Meshes

BoxGeometry-Fallbacks durch enganliegende Meshes ersetzen:

- Fillet-Zonen: Extrusion entlang der betroffenen Kante
- Boolean-Zonen: Konvexe Hülle der Schnittregion
- Domain Warps: Mesh-Approximation des gewarpten Volumens

#### Phase 4: Optimierungen

- Workgroup Shared Memory für Opcode-Blöcke
- Adaptive Step-Count (weniger Steps für kleine `[t_min, t_max]`-Intervalle)
- Half-Resolution für transparente Regionen + Upscale im Blit
- Raytraced Shadows (Schattenstrahl mit derselben SDF)

### Kritische Dateien

| Datei | Änderung |
|-------|----------|
| `web/src/sdfDeferredRenderer.ts` | **NEU**: Pipeline-State, Passes, Blit |
| `web/src/SettingsMenu.tsx` | RenderMode `"sdf-deferred"` |
| `web/src/Viewport.tsx` | `SdfDeferredOverlay` Komponente |
| `web/src/sdfVmInterpreter.ts` | `baseOffset`-Parameter für Snippet-Lookup |
| `web/src/kernel.ts` | `bodyDeferredInfo()` Accessor |
| `crates/kernel/src/bvh.rs` | `classify_for_deferred()` + `DeferredMeshInfo` |
| `crates/kernel/src/lib.rs` | WASM API `bodyDeferredInfoJson()` |
| `crates/kernel/src/sdf_compiler.rs` | Snippet-Kompilierung pro CSG-Subtree |

### Verifikation

```
Phase 1:
  - Union(Box, Sphere): Beide als opake Meshes, korrekte Beleuchtung
  - Kamera drehen: Kein Z-Fighting, korrekte Verdeckung
  - F2 HUD: "sdf-deferred", Framezeit < sdf-vm

Phase 2:
  - Difference(Box, Sphere): Bohrung korrekt sichtbar
  - SmoothUnion(Box, Sphere, 5mm): Fillet-Zone korrekt gerendert
  - Mehrere Bodies: Korrekte Verdeckung zwischen Bodies

Phase 3:
  - Fillet-Zonen: Enganliegendes Mesh statt Box, weniger verschwendete Pixel
  - Performance-Vergleich: sdf-deferred vs sdf-hybrid-vm

Phase 4:
  - Benchmark: Shared Memory vs. Uniform Buffer
  - Schatten-Demo: Raytraced Soft Shadows
```
