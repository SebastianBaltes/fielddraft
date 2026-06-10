# Sketch in 3D — Umbauplan (Tickets yx6a + bxoo)

SSOT für den Umbau des Sketch-Workflows. Stand: 2026-06-10, ab v0.6.40.

## Ziel (aus den Tickets)

- **yx6a:** Sketch ist ein Feature wie jedes andere, mit Property **Sketch
  Plane** (Query: Default-Plane `qCreatedBy(origin_plane_*)` oder
  `qFace(...)`). Kein Plane-Pick-Modus VOR dem Feature. Die
  Sketch-Tool-/Sub-Call-Liste ist eine Unterkomponente der Sketch-Detail-View.
- **bxoo:** Keine separate 2D-Sketch-View. Die normale 3D-Szene mit normaler
  Kamera bleibt aktiv (Pan/Zoom/Rotate, Punkt-/Kanten-/Flächen-Selektion).
  Eine unsichtbare Sketch-Ebene ist am Anker orientiert (z.B. Würfel-Face);
  gezeichnete Objekte werden in dieser Ebene **im 3D-Raum** dargestellt.
  Die Feature-Toolbar wird wie bisher durch die Sketch-Tools ersetzt.
  Onshape-Referenz: Sketch-Dialog "Sketch plane: Face of Extrude 2",
  blauer Kreis liegt sichtbar in der Ebene der Quaderoberseite.

## Was schon da ist (Verifiziert, Stand v0.6.39)

- Kernel akzeptiert beliebige Plane-Queries im sketch-FeatureCall
  (`sketch_features.rs:2309-2329`): `origin_plane_*` → `datum_plane_frame`,
  `qFace(...)`/`qCreatedBy(...)` → `resolve_face_frame` → `face_plane_frame`.
  Schema deklariert `plane` bereits als query-Param mit entityFilter FACE
  (`sketch_features.rs:2216-2234`).
- `addSketch(planeQuery, subCalls)` / `replaceSketch(...)` nehmen beliebige
  Plane-Queries. `editSketch` round-trippt die Plane seit v0.6.40.
- QueryWidget + queryPickerBridge (`widgets.tsx:861-928`,
  `queryPickerBridge.ts`) liefern den Onshape-artigen Pick-ins-Feld-Flow
  (Vorbild: Extrude-Faces-Param).
- 3D-Rendering committeter Sketche existiert: `collectSketchLineSegments`
  (`viewportOverlays.tsx:61-…`, visibleSketches) — projiziert (u,v) über
  `parseSketchPlane` in den Weltraum. Heute nur origin-Planes; muss den
  SketchFrame des Kernels nutzen.
- Face-Hover liefert präzise `qFace(idHash)` in jedem Render-Modus
  (brepPickBridge; Mesh-Raycast + analytischer `pick()`-Fallback).

## Phasen

### Phase 1 — Plane als Feature-Property (yx6a, kleinster Schnitt)
1. Sketch-Button legt SOFORT eine Session an (Default-Plane XY oder zuletzt
   gehoverte Face), KEIN facePickMode mehr. `facePickMode`-State + Banner
   ("Click a face to set sketch plane") entfernen; `enterSketchFromFacePick`/
   `normalToPlaneQuery` durch echte `qFace(hash)`-Erfassung ersetzen (Hover
   aus brepPickBridge, nicht nur Normale).
2. Sketch-Detail-View (Outliner): Plane-Query-Picker-Feld (QueryWidget,
   entityFilter FACE — Default-Planes brauchen zusätzlich Klickbarkeit der
   Datum-Plane-Quads → Query `qCreatedBy(origin_plane_*)`).
   Outliner.tsx:282-285 (Sketch vom Detail-Edit ausgeschlossen) aufheben:
   Plane-Änderung über `updateFeatureParams` bzw. in der Session über
   Session-State.
3. Sub-Call-Liste (Outliner.tsx:638-688) als eigene Komponente
   (`SketchSubCalls`) extrahieren; in Session-Panel UND Sketch-Detail-View
   rendern.
4. Kernel-seitig nichts nötig (Queries funktionieren); Label-Formatierung
   `qFace(...)` → "Face of <Feature>" in queryLabels.ts ergänzen.

### Phase 2 — Sketch-Geometrie im 3D-Raum (bxoo, Rendering)
1. Kernel exportiert den aufgelösten SketchFrame der AKTIVEN Session-Plane
   (origin + x_axis + y_axis + normal) — neues wasm-API
   `resolveSketchPlaneFrame(planeQuery) -> Frame` (resolve_face_frame +
   datum_plane_frame existieren).
2. Neuer 3D-Layer `SketchSessionOverlay3D` (Three.js, analog
   visibleSketches-Layer, aber aus dem Session-State): committed sub_calls
   + Live-Draft + Selektion + Grips als Fat Lines/Punkte im Frame
   (world = origin + u·x_axis + v·y_axis). `parseSketchPlane`-Hardcoding
   durch den Kernel-Frame ersetzen (gilt auch für visibleSketches).
3. Region-Shading (7x4v) als transluzente Meshes in der Ebene
   (sketchRegions-Polygone → earcut → Mesh im Frame).

### Phase 3 — Input in 3D (bxoo, Interaktion)
1. Maus → Sketch-Koordinaten: Ray-Plane-Schnitt (Kamera-Ray ∩ SketchFrame)
   statt `screenToSketch`. Zentrale Helper-Funktion; alle Tool-Handler von
   SketchCanvas2d portieren (Line/Rect/Circle/Arc/Spline/Move/Marquee/Grips).
2. Kamera: CameraRig-sketchActive-Zweig ENTFERNEN — normale Kamera +
   OrbitControls bleiben. Optional (Onshape-like): beim Sketch-Start einmalig
   Kamera senkrecht auf die Ebene animieren, danach frei.
3. Snapping/Pick-Toleranzen: Screen-Space-Toleranzen (px) bleiben, aber
   Distanzen im Screen-Raum messen (Punkte über Kamera projizieren).
4. SketchCanvas2d bleibt übergangsweise NUR noch für: Live-Längen-Input-Pille
   (DOM, an projizierter Linienmitte), Labels/Badges. Grid: Three.js-Grid in
   der Sketch-Ebene (statt Canvas-Grid), gedimmt.
5. Selektion von Punkten/Kanten/Flächen der 3D-Szene bleibt während der
   Session aktiv (für Projektion/Referenzen + Plane-Picker).

### Phase 4 — Aufräumen
- sketchCameraBridge (top-down-Annahme) entfernen oder auf Frame-Basis
  generalisieren; tote 2D-Pfade löschen; ARCHITECTURE.md aktualisieren;
  Playwright-Sketch-E2E anpassen (reference_playwright_realgpu).

## Landminen
- SketchCanvas2d ist groß (~3400 Zeilen) und trägt ALLE Tools — Phase 3
  tool-weise portieren, nicht big-bang.
- Hidden-Tab-rAF + WebGPU-Compile-Slowdowns machen Browser-Verifikation
  zäh — Unit-Tests für Ray-Plane/Frame-Mathe zuerst (pure Module).
- Sketch auf schräger Face: face_plane_frame-Seed-Konvention bestimmt die
  (u,v)-Orientierung — Extrude-Profile (face_profile.rs) nutzen dieselbe;
  beim Frame-API keine NEUE Konvention einführen.
- fsmf-Fix (v0.6.39, +V nach unten im 2D-Mapping) wird mit Phase 3 obsolet,
  bis dahin nicht erneut anfassen.
