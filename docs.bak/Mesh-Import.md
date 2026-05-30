**Anforderung: Es muss auch den Obejtktyp "Mesh" geben, z.b. für importierte, nur als Mesh vorliegende Modelle.  Diese sollen genau wie alle anderen Objekte auch editierbar sein, d.h. boolean, Gyroid, Filets.**

Dass du auch importierte Polygon-Meshes als "Bürger erster Klasse" in deinen SDF-Baum aufnehmen willst (für `smin`, Booleans etc.), ist der logische und konsequente Schritt für ein modernes CAD-System.

Da du völlig richtig erkannt hast, dass die Umwandlung nur einmalig beim Import (offline) passieren muss, öffnet das die Tür für den absoluten Industriestandard in diesem Bereich: **Volume Textures (3D-Texturen)**.

Da die Stack-VM in deinem Raymarching-Shader extrem schnell sein muss, verbietet sich eine komplexe Dreiecks-Traversierung im Shader. Stattdessen "backen" wir das Mesh auf der CPU (in Rust) in ein Voxel-Gitter und laden dieses als 3D-Textur in den VRAM.

Hier ist die Architektur für diesen Mesh-zu-SDF Import-Workflow.

## 1. Die Datenstruktur: 3D-Texturen (WebGPU `texture3D`)

Anstatt das Mesh als Dreiecksliste an die GPU zu schicken, erzeugst du beim Import einen 3D-Datenblock.

* **Das Gitter:** Du legst ein unsichtbares, regelmäßiges Raster (z. B. $128 \times 128 \times 128$ oder $256 \times 256 \times 256$ Voxel) über die Bounding Box des importierten Meshes.
* **Der Inhalt:** In jedem "Voxel" (also jedem Pixel der 3D-Textur) speicherst du exakt einen Float-Wert: Die vorab berechnete *Signed Distance* von der Mitte dieses Voxels zur nächstgelegenen Dreiecksoberfläche.
* **VRAM-Kosten:** Eine 3D-Textur mit $256^3$ Voxeln im Format `R16Float` (was für SDFs meist ausreicht) belegt exakt 32 Megabyte. Für WebGPU ist das heutzutage ein Klacks.

## 2. Der Algorithmus (Die Import-Phase in Rust)

Das "Backen" des Meshes in das SDF-Gitter passiert beim Import und darf ein bis zwei Sekunden dauern. Es besteht aus drei mathematischen Schritten:

### Schritt A: Räumliche Indizierung (BVH)

Damit du nicht für jeden der Millionen Voxel den Abstand zu jedem der tausenden Dreiecke prüfen musst, baust du in Rust zuerst einen **BVH (Bounding Volume Hierarchy)** oder einen Kd-Baum für das importierte Mesh.

### Schritt B: Der absolute Abstand (Unsigned Distance)

Du iterierst nun in Rust über jeden Punkt deines Voxel-Gitters. Über den BVH findest du das nächstgelegene Dreieck in logarithmischer Zeit. Dann berechnest du den exakten euklidischen Abstand vom Voxel-Zentrum zum Dreieck (Punkt-zu-Dreieck-Distanz). Das ist dein Betrag.

### Schritt C: Das Vorzeichen-Problem (Inside / Outside)

Das ist der eigentlich schwierige Teil. Ist der Voxel innerhalb des Meshes (negative Distanz) oder außerhalb (positive Distanz)?

* **Der naive Weg (Raycasting):** Du schießt einen Strahl vom Voxel in eine beliebige Richtung. Schneidet der Strahl das Mesh in einer ungeraden Anzahl, bist du "drinnen". Bei einer geraden Anzahl "draußen" (Paritätsprüfung). *Nachteil:* STLs aus dem Internet haben oft Löcher (non-watertight). Da versagt dieser Ansatz kläglich.
* **Der robuste Weg (Generalized Winding Numbers):** Dies ist der Goldstandard (publiziert von Jacobson et al.). Man berechnet den Raumwinkel, unter dem das Mesh von dem Punkt aus erscheint. Ist er nahe $4 \pi$, bist du drinnen, nahe $0$ draußen. Das funktioniert verblüffend gut, selbst wenn das Mesh offene Kanten hat oder sich selbst durchdringt ("Polygon-Suppe").

## 3. Die Integration in die Shader-Pipeline

Sobald das Mesh als 3D-Textur vorliegt und im VRAM liegt, wird es in deiner Stack-VM zu einem extrem einfachen, fast kostenlosen Opcode.

### Der neue VM Opcode: `OP_PUSH_TEX3D`

Du fügst einen neuen Primitiv-Typ hinzu. Die Parameter für diesen Opcode im Bytecode-Stream sind:

* Die Textur-ID / Bind-Group (um die richtige 3D-Textur zu samplen).
* Die Bounding-Box / Transformationsmatrix (um die globale Koordinate in lokale Textur-Koordinaten $\[0, 1]$ umzurechnen).

### Die Hardware-Magie: Trilineare Interpolation

Wenn der Raymarcher läuft, landet er an einer Position `p`. Er rechnet `p` in lokale Textur-UVWs um und ruft auf:

`texture(mesh_sampler, uvw).r`

Jetzt passiert das Wichtigste: Die Grafikkarte nutzt ihre Hardware-Bilinearisierung (in 3D also *Trilineare Interpolation*), um den Wert *zwischen* den diskreten Voxel-Zentren zu interpolieren.

Das Ergebnis ist ein **kontinuierliches Feld**, das perfekt glatt ist. Der Raymarcher kann ganz normal Normalen per `Finite Differences` berechnen, als wäre es eine mathematische Formel. Deine Gyroids, Fillets und Booleans verhalten sich exakt so, wie du es erwartest.

**Zusammenfassung:** Das Hüll-Mesh-Konzept (aus unserer vorherigen Architektur) bleibt für dieses Objekt unverändert (z. B. ein einfacher Quader um die Bounding-Box). Aber anstatt in der VM eine harte Box zu berechnen, liest die VM im Raymarching-Schritt die Distanz aus der trilinear gefilterten 3D-Textur aus.