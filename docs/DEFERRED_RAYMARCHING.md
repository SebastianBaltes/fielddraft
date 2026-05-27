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
(**HULL_INFLATE = 0.1%**) aufgeblasen, um sicherzustellen, dass die
tessellierte Polygon-Oberfläche die mathematische SDF-Fläche an allen
Stellen überdeckt — insbesondere an Silhouetten-Kanten, wo die
Polygon-Sehne kürzer ist als der Kreisbogen:

- Ein Quader-Term bekommt ein leicht aufgeblasenes Quader-Mesh.
- Ein Kugel-Term bekommt eine SphereGeometry (32 Segmente) × 1.001.
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

- Welche SDF-Terme für diesen Pixel relevant sind (über die Bitmask)
- Den Startpunkt des Rays (Front-Face-Tiefe = tMin)
- Den Endpunkt des Rays (siehe tMax-Bestimmung unten)

### Per-Pixel Body-ID-Buffer

Durch die Rasterisierung aller Hüll-Meshes entsteht pro Pixel ein **Set der
relevanten SDF-Terme**. Dieses Set wird in einem atomaren Storage Buffer
als gepackte Body-IDs gespeichert: 2 × `uint32` pro Pixel, jeweils mit
16-Bit Count und 16-Bit Body-IDs. Jeder transparente Fragment-Shader
registriert seine Body-ID per `atomicAdd` (Count) + `atomicOr` (ID-Slot).
So werden bis zu 2 transparente Bodies pro Pixel gesammelt, nicht nur der
vorderste.

Die SDF-Auswertung folgt der CSG-Baumstruktur: Werden die Terme A und B durch
`Difference(A, B)` verknüpft, evaluiert der Raymarcher `max(sdf_A, -sdf_B)`.
Die CSG-Beziehungen sind statisch bekannt und werden als linearer
Opcode-Stream (Stack-VM in RPN) an die GPU übergeben.

### tMax-Bestimmung

Der Endpunkt des Rays (tMax) wird hybrid bestimmt:

1. **Back-Face RT** (immer verfügbar für transparente Pixel): Die Rückseiten
   aller transparenten Hüll-Meshes werden mit `BackSide` in ein separates RT
   gerendert. Das Material gibt die skalare Kamera-Distanz aus
   (`length(worldPos - camPos)`). Durch **MAX-Blending** (`blendEquation =
   MaxEquation`) gewinnt automatisch die fernste Rückseite über alle
   überlappenden Hulls. tMax ergibt sich als `backDist - frontDist`.

2. **Opaque Depth** (optional, engerer Bound): Liegt ein opakes Objekt eines
   *anderen* Bodies dahinter, ist dessen Tiefe aus Pass 1 ein engerer Bound.
   `tMax = min(backFaceTMax, opaqueDist)`. Same-Body-Opaques werden ignoriert
   (sonst würden Rillen/Lücken volumetrischer Bodies zugefüllt).

### Korrektheit

Das zentrale Korrektheitskriterium: An jedem Pixel, an dem ein transparentes
Hüll-Mesh getroffen wird, muss dessen Opcode-Snippet **alle** SDF-Terme
enthalten, die für die korrekte Oberflächenberechnung in diesem Raumbereich
nötig sind. Außerhalb des Hüll-Meshes ist per Definition der opake Körper
(oder der Hintergrund) die korrekte Oberfläche.

Zusätzlich gilt für den Opaque-Fallback bei March-Miss: Findet der Raymarcher
keine SDF-Oberfläche, wird nur auf ein opakes Objekt eines *anderen* Bodies
zurückgefallen. Ein Same-Body-Miss bedeutet, dass der Ray durch eine
Rille/Lücke des volumetrischen Bodies ging — hier wird discardet, damit der
Hintergrund sichtbar bleibt.

### Performance-Garantien

1. **Kein leerer Raum**: Der Raymarcher startet erst am Hüll-Mesh, nicht an
   der Kamera. Typische Einsparung: 50–90% der Marching-Schritte.
2. **Minimale SDF-Terme**: Pro Pixel werden nur die tatsächlich relevanten
   Terme evaluiert, nicht der gesamte CSG-Baum der Szene.
3. **Einfache Körper sind fast kostenlos**: Durch die Hull-Inflation
   startet der Ray knapp außerhalb der Oberfläche. Sphere-Tracing konvergiert
   in 1–3 Schritten (+ 6 für Normale) = 7–9 SDF-Evaluationen pro Pixel.
4. **Hardware-Culling**: Die GPU verwirft transparente Hüll-Meshes hinter
   opaken Oberflächen automatisch per Z-Test (Early-Z).
5. **Transparenz ohne Sorting**: Volumetrische Bodies (Gyroid, Thread)
   benötigen keine spezielle Transparenz-Behandlung — Rays die keine
   SDF-Oberfläche treffen werden einfach discardet.
6. **Enges March-Intervall**: Durch die Back-Face RT ist tMax exakt an die
   Hull-Geometrie gebunden, nicht an eine konservative AABB-Diagonale.

### Dual-Mesh-Klassifikation

Jeder Body liefert mindestens ein Hüll-Mesh — opak, transparent oder beides.
Die Klassifikation erfolgt TypeScript-seitig beim Mesh-Aufbau und richtet
sich nach der Geometrie des Leaf-Primitives:

| Body-Typ | Opakes Mesh | Transparentes Mesh |
|---|---|---|
| Box | BoxGeometry (exakt) | — |
| Sphere | SphereGeometry (exakt) | SphereGeometry (inflated) |
| Cylinder | CylinderGeometry (exakt) | CylinderGeometry (inflated) |
| Torus | TorusGeometry (exakt, rotateX π/2) | TorusGeometry (inflated) |
| Capsule | CapsuleGeometry (exakt) | CapsuleGeometry (inflated) |
| Cone | CylinderGeometry (exakt) | CylinderGeometry (inflated) |
| Extrude/Revolve | ContourMesh (exakt) | ContourMesh (inflated) |
| Gyroid (needs_full_sdf) | — | BoxGeometry (AABB) |
| Domain Warps (needs_full_sdf) | — | BoxGeometry (AABB) |

Regeln:
- Jeder Body braucht mindestens ein Mesh (opak oder transparent).
- **Box**: Nur opak — rein konvex, keine Silhouetten-Clipping-Gefahr.
- **Alle anderen Primitiven**: Opak (exakte Geometrie) + transparent
  (inflated × HULL_INFLATE). Der Sphere-Tracer konvergiert vom inflated
  Rand in 1–3 Schritten zur exakten Fläche.
- **Gyroid / Domain Warps**: Nur transparent (AABB-Box), da keine exakte
  Mesh-Geometrie darstellbar.

Vorteile gegenüber nur-inflated Meshes:
- **Kein Inflations-Halo**: Opake Meshes liefern exakte Silhouetten.
  Am Rand, wo nur das transparente Mesh den Pixel trifft, findet der
  Sphere-Tracer die SDF-Fläche oder discardet (dünner, kaum sichtbarer Rand).
- **Gyroid-Durchsicht**: Gyroid-Löcher zeigen das opake Objekt dahinter,
  weil die opake Schicht (RT1) die Information bewahrt.
- **Weniger SDF-Arbeit**: Für opak-only Pixel (Fall A) entfällt der
  Sphere-Trace komplett — nur 6 SDF-Evals für die Normale.

### Three-RT + Body-ID-Buffer-Implementierung

Die Implementierung nutzt drei Render Targets plus einen
Fragment-Shader-Atomic Body-ID-Buffer:

- **RT1 (Opaque)**: Nicht-inflated Hüll-Meshes → `vec4(worldPos, bodyIndex+1)`.
  Standard-Depth-Test, nächstes opakes Objekt gewinnt.
- **RT2 (Transparent Front)**: Inflated/Volume Hüll-Meshes, FrontSide →
  `vec4(worldPos, bodyIndex+1)`. Standard-Depth-Test, nächste Vorderseite = tMin.
- **RT3 (Back-Face)**: Selbe Hulls, BackSide → `vec4(scalarCamDist, 0, 0, 1)`.
  MAX-Blending, kein Depth-Buffer. Fernste Rückseite = tMax-Basis.
- **Body-ID-Buffer**: 2 × `uint32` pro Pixel, gepackte 16-Bit Body-IDs + Count.
  Fragment-Shader registriert Body-IDs per `atomicAdd` + `atomicOr`.

#### Pipeline (5 Passes)

    Pass 0:  Compute Clear    → atomicStore(bodyIdBuf[i], 0) für alle Pixel (2 Wörter pro Pixel)
    Pass 1:  Opaque Raster    → RT1 (worldPos + bodyTag), Depth Buffer
    Pass 2:  Transparent Front → RT2 (worldPos + bodyTag, FrontSide)
             + Body-ID-Insertion in bodyIdBuf
    Pass 2b: Back-Face Collect → RT3 (scalar camDist, BackSide, MAX-Blending)
    Pass 3:  Unified Shade     → Fullscreen Quad liest RT1, RT2, RT3, bodyIdBuf

#### Shade-Logik (Pass 3)

- **Fall A** (`!hasTrans && hasOpaque`): 6 SDF-Evals für Normale + Lighting.
- **Fall B** (`hasTrans`): Multi-Body Sphere-Trace von tMin (RT2 Front-Face)
  bis tMax (RT3 Back-Face-Distanz minus Front-Distanz, optional verengt durch
  Opaque-Depth eines anderen Bodies). Evaluiert nur die 1–2 tatsächlich im
  Body-ID-Buffer eingetragenen Bodies. Der nächste Treffer (min distance)
  wird geshaded.
  Hit → Shade. Kein Hit + hasOpaque (anderer Body) → Fall A.
  Kein Hit + kein anderer opaker Body → Discard.
- **Fall C** (`!hasOpaque && !hasTrans`): Hintergrund (Discard).

#### THREE.js Patches für Fragment-Shader Atomics

Die WebGPU-Spec erlaubt `GPUBufferBindingType.Storage` (read_write) in
Fragment-Shadern, aber THREE.js r183 blockiert dies an zwei Stellen:

1. **WGSLNodeBuilder.getNodeAccess()**: Warnte bei Atomics in
   Nicht-Compute-Stages. Patch: Warnung entfernt (Logik war bereits korrekt).
2. **WebGPUBindingUtils**: Erzwang `ReadOnlyStorage` für alle
   Nicht-Compute-Stages. Patch: Respektiert `binding.access` für alle Stages.

Patches liegen als `patch-package` in `web/patches/three+0.183.2.patch`
und werden via `postinstall` automatisch angewendet.

---

## Kapitel 2: GPU-Referenz

### Stack-VM für SDF-Auswertung

Die SDF-Auswertung erfolgt durch eine Stack-basierte Virtuelle Maschine
(Stack-VM), die einen linearen Opcode-Stream interpretiert:

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
```

### Concatenated Opcode Buffer

Alle Opcode-Snippets aller Bodies werden hintereinander in einem einzigen
Storage Buffer (`StorageBufferAttribute`) abgelegt. Eine Offset-Tabelle (`[start, length]` pro Body)
erlaubt dem Shade-Pass, das richtige Snippet zu laden:

```
Opcode Buffer:
  [Snippet 0: Box-Opcodes...] [Snippet 1: Sphere-Opcodes...]
  [Snippet 2: Difference(Box,Sphere)-Opcodes...] ...

Offset Table:
  Body 0 → offset=0,   length=12
  Body 1 → offset=12,  length=10
  ...
```

### Unified Lighting

Alle Pixel — opak und transparent — durchlaufen denselben Shade-Pass.
Die Normale wird per Finite Differences auf der SDF berechnet:

```
n = normalize(vec3(
  sdf(p + (eps,0,0)) - sdf(p - (eps,0,0)),
  sdf(p + (0,eps,0)) - sdf(p - (0,eps,0)),
  sdf(p + (0,0,eps)) - sdf(p - (0,0,eps))
))
```

---

## Kapitel 3: Implementierung in FieldDraft

### Konstanten

| Konstante | Wert | Zweck |
|-----------|------|-------|
| `DEFERRED_MAX_OPCODES` | 65536 | Max. Opcodes im concatenated Buffer |
| `MAX_BODIES` | 256 | Max. Bodies (Buffer-Allokation, frei erhöhbar) |
| `DEFERRED_MARCH_STEPS` | 512 | Max. Sphere-Trace-Schritte pro Pixel |
| `SURFACE_EPS` | 0.002 | Konvergenz-Schwelle für Treffer |
| `HULL_INFLATE` | 1.001 | Aufblasfaktor für transparente Hulls |
| `MAX_PIXELS` | 4096 × 2160 | Max. Viewport-Auflösung für Body-ID-Buffer |
| `BODY_ID_WORDS_PER_PIXEL` | 2 | uint32-Wörter pro Pixel im Body-ID-Buffer |

### Dateien

| Datei | Rolle |
|-------|-------|
| `web/src/sdfDeferredRenderer.ts` | Pipeline-State, alle Passes, Materialien |
| `web/src/sdfVmInterpreter.ts` | `buildVmSdfEvalFn()` mit `baseOffset` |
| `web/src/sdfHybridRenderer.ts` | `createPrimitiveGeometry()`, `applyLeafTransform()`, `BODY_COLORS` |
| `web/src/Viewport.tsx` | `SdfDeferredOverlay` Komponente |
| `web/src/SettingsMenu.tsx` | RenderMode `"sdf-deferred"` |
| `crates/kernel/src/bvh.rs` | `classify_for_hybrid()` → `HybridLeafInfo` |
| `crates/kernel/src/sdf_compiler.rs` | CSG → RPN Opcode-Kompilierung |

### DeferredPipelineState

Siehe `DEFERRED_RAYMARCHING_IMPL.md` für die vollständige State-Struktur.
Kern-Elemente:

- 3 Render Targets (Opaque, Transparent Front, Back-Face)
- Body-ID-Buffer: `StorageBufferAttribute` (2 × uint32 pro Pixel, atomic)
- Opcode + Offset Buffers: `StorageBufferAttribute` (read-only storage buffer)
- Material StorageBuffer: 12 floats pro Body
- Volume-Texturen: bis zu 4 `Data3DTexture`

### Render Targets

| RT | Format | Depth | Inhalt |
|----|--------|-------|--------|
| opaqueRT | RGBA32F + DepthTexture | ja | `vec4(worldPos, bodyIndex+1)` |
| transparentRT | RGBA32F + DepthTexture | ja | `vec4(worldPos, bodyIndex+1)` |
| transparentBackRT | RGBA16F | nein | `vec4(scalarCamDist, 0, 0, 1)`, MAX-Blending |

### Materialien

**G-Buffer Material** (opak): `outputNode = vec4(worldPos, bodyIndex+1)`.
FrontSide, depthWrite + depthTest.

**Transparent G-Buffer Material**: Wie G-Buffer, zusätzlich
Body-ID-Insertion per `atomicAdd` + `atomicOr` in den Body-ID-Buffer.

**Back-Face Material**: `outputNode = vec4(length(worldPos - camPos), 0, 0, 1)`.
BackSide, kein depthTest/Write, `CustomBlending` mit `MaxEquation` +
`OneFactor` — fernste Rückseite gewinnt.

### Pipeline (5 Passes)

**Pass 0 — Compute Clear**: `atomicStore(bodyIdBuf[i], 0)` per Compute (2 Wörter pro Pixel).

**Pass 1 — Opaque Raster**: Nicht-inflated Hüll-Meshes → opaqueRT.
Standard-Depth-Test, nächstes opakes Objekt gewinnt.

**Pass 2 — Transparent Front**: Inflated Hüll-Meshes (FrontSide) →
transparentRT. Standard-Depth-Test + Body-ID-Insertion in bodyIdBuf.
Nächste Vorderseite = tMin.

**Pass 2b — Back-Face Collect**: Selbe Hulls (BackSide) → transparentBackRT.
MAX-Blending auf skalare Kamera-Distanz, kein Depth-Buffer. Fernste
Rückseite gewinnt.

**Pass 3 — Unified Shade**: Fullscreen Quad liest alle RTs + Body-ID-Buffer.

### Shade-Logik (Pass 3)

**Fall C** (`!hasOpaque && !hasTrans`): Discard (Hintergrund).

**Fall A** (`hasOpaque && !hasTrans`): Opak-only. Shade am Mesh-Punkt:
6 SDF-Evals für Normale + Lighting. Kein March nötig.

**Fall B** (`hasTrans`): Multi-Body Sphere-Trace.

1. **tMin**: World-Position aus RT2 (nächste transparente Vorderseite).
2. **tMax**: `backDist - frontDist` aus RT3 (fernste Rückseite minus
   Vorderseiten-Abstand zur Kamera). Optional verengt durch Opaque-Depth
   eines *anderen* Bodies aus RT1: `tMax = min(tMax, opaqueDist)`.
   Same-Body-Opaques werden ignoriert.
3. **March**: Loop über `DEFERRED_MARCH_STEPS`. Pro Schritt: evaluiere
   SDF für die 1–2 Bodies aus dem Body-ID-Buffer, finde minimale Distanz.
   Treffer bei `d < SURFACE_EPS`.
4. **Ergebnis**:
   - Hit → Shade am Trefferpunkt.
   - Kein Hit + Opaque eines anderen Bodies → Shade am opaken Punkt (Fall A).
   - Kein Hit + kein anderer opaker Body → Discard (Rille/Lücke).

### Body-Aufbau (rebuildDeferredBodies)

Pro Body werden Opcode-Snippets in den concatenated Buffer kopiert und
Hüll-Meshes erzeugt:

- **Box**: Nur opakes Mesh (BoxGeometry, exakt).
- **Sphere/Cylinder/Torus/Capsule/Cone/Extrude/Revolve**: Opakes Mesh
  (exakte Geometrie) + transparentes Mesh (inflated × HULL_INFLATE) +
  Back-Face Mesh (inflated, BackSide, shared backFaceMaterial).
- **Gyroid / needs_full_sdf**: Nur transparentes + Back-Face Mesh
  (BoxGeometry über AABB).

---

## Kapitel 4: Skalierung

### Warum tausende Bodies kein Problem sind

Die Deferred-Architektur entkoppelt die Szenen-Komplexität (Gesamtzahl Bodies)
von den Per-Pixel-Kosten:

1. **Rasterisierung als Vorfilter**: Die GPU rasterisiert Hüll-Meshes in
   Hardware-Geschwindigkeit. Z-Culling, Frustum-Culling und Occlusion
   eliminieren unsichtbare Bodies vollständig — sie erzeugen weder Fragments
   noch Body-ID-Einträge.

2. **Per-Pixel O(present)**: Der Sphere-Tracer evaluiert nur die 1–2 Bodies,
   die tatsächlich am Pixel eingetragen sind (aus dem Body-ID-Buffer), nicht
   die gesamte Szene. Ein Pixel mit 2 überlappenden Bodies kostet gleich viel,
   egal ob die Szene 10 oder 10.000 Bodies hat.

3. **Opake Pixels sind fast kostenlos**: Für opak-only Pixel (kein
   transparenter Overlap) entfällt der March-Loop komplett — nur 6 SDF-Evals
   für die Finite-Difference Normale. Das betrifft den Großteil aller Pixel.

4. **Transparente Regionen sind screen-space-begrenzt**: Fillets, Booleans
   und Objektränder betreffen nur schmale Streifen in Screen-Space. Selbst bei
   vielen Bodies bleibt der Anteil der Pixel, die tatsächlich marchen müssen,
   gering.

### Verbleibende Bottlenecks

| Bottleneck                  | Aktuell                | Lösung                                           |
| --------------------------- | ---------------------- | ------------------------------------------------ |
| Transparente Bodies/Pixel   | Max. 2                 | Overflow-Liste (Storage Buffer, nicht impl.)     |
| ~~Opcode-Buffer~~           | ~~4096 floats (uniform)~~ | Gelöst: Storage Buffer, 65536 floats          |
| Body-ID-Buffer Speicher     | 2 × uint32 × Pixel     | Skaliert mit Viewport, nicht mit Body-Count      |
| Shader-Kompilationszeit     | Steigt mit VM-Opcodes  | Betrifft nur Rebuild, nicht Render-Performance   |
| Mesh-Verwaltung (CPU-seitig) | Map pro Body           | Linear skalierbar                                |
