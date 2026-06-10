# UX-Presets — Maus- und Tastatursteuerung anderer CAD-Programme

Stand: 2026-06-10. Konzept aus Ticket 20260610-085906-evq2: Anwender bringen
Steuerungs-Gewohnheiten aus ihrem bisherigen Programm mit. Ein Preset-System
soll die wichtigsten Schemata per Setting abbilden: **Three.js-Editor,
Onshape, FreeCAD, Fusion 360, TinkerCAD** (plus FieldDraft-Default).

## Ist-Zustand

- Kamera: `@react-three/drei`-`OrbitControls` in `web/src/viewportCamera.tsx`
  mit den drei-Standardbindings (LMB Orbit, RMB Pan, Wheel Zoom) und festen
  Speeds. Im Sketch-Modus eine zweite, rotationsfreie Instanz.
- Selektion (seit evq2): Klick selektiert Vertex/Kante/Fläche
  (Spezifischeres gewinnt: Vertex > Kante > Fläche), zweiter Klick auf die
  selektierte Fläche eskaliert zum Körper. Shift/Ctrl/Cmd = Multi-Select.
  Entscheidungslogik zentral in `resolveElementClick`
  (`web/src/elementQuery.ts`).
- Settings: einzelne `localStorage`-Keys (`fielddraft.*`), UI in
  `web/src/SettingsMenu.tsx`. Es gibt noch kein Bündel-/Preset-Konzept.

## Vergleich der Steuerschemata

Orbit/Pan/Zoom der unterstützten Programme (Maus, Default-Konfiguration):

| Programm | Orbit | Pan | Zoom | Selektions-Idiom |
|---|---|---|---|---|
| FieldDraft (heute) | LMB-Drag | RMB-Drag | Wheel | Klick = Element, 2. Klick Fläche = Körper |
| Three.js-Editor | LMB-Drag | RMB-Drag (bzw. 2-Finger) | Wheel | Klick = Objekt (keine Sub-Elemente) |
| Onshape | RMB-Drag | MMB-Drag (oder Ctrl+RMB) | Wheel | Klick = Element, Doppelklick = Kette/Fläche, RMB-Menü |
| FreeCAD (CAD-Navigation) | MMB-Drag | MMB+LMB oder Space | Wheel | Klick = Element, Ctrl = Multi |
| Fusion 360 | Shift+MMB-Drag | MMB-Drag | Wheel | Klick = Element, Klick-Halten = Tiefen-Auswahlmenü |
| TinkerCAD | RMB-Drag | MMB-Drag | Wheel | Klick = Objekt, Drag = Box-Select |

Wichtigste Konfliktachse: **Wer bekommt die linke Maustaste?** FieldDraft und
der Three.js-Editor orbiten mit LMB; alle klassischen CAD-Systeme reservieren
LMB für Selektion/Box-Select und orbiten mit RMB/MMB. Das Preset entscheidet
also primär über die Button-Zuordnung der `OrbitControls` und darüber, ob
LMB-Drag eine Box-Selektion (später) oder eine Kamerafahrt ist.

## Zielarchitektur

Mermaid-Überblick:

```mermaid
flowchart LR
    SM[SettingsMenu\nPreset-Dropdown] -->|fielddraft.controlsPreset| CP[controlsPreset.ts\nPreset-Registry]
    CP -->|mouseButtons, speeds| VC[viewportCamera.tsx\nOrbitControls]
    CP -->|Modifier, Idiome| VP[Viewport.tsx\nClick-Handling]
    CP -->|Shortcuts| KT[keyboardTarget.ts]
```

1. **Preset-Registry** `web/src/controlsPreset.ts`: ein Objekt pro Preset
   mit drei Abschnitten —
   - `camera`: `mouseButtons` (`{LEFT, MIDDLE, RIGHT}` auf
     `THREE.MOUSE.ROTATE|PAN|DOLLY` oder `null`), `touches`,
     `rotateSpeed/panSpeed/zoomSpeed`, Modifier-Kombinationen
     (z. B. Fusion: Orbit nur bei Shift+MMB → `enableRotate` dynamisch).
   - `selection`: Multi-Select-Modifier (Shift vs. Ctrl), ob der
     Körper-Eskalationsklick aktiv ist, später Box-Select-Verhalten.
   - `keys`: Tastatur-Kürzel-Overrides (z. B. FreeCAD-Viewcube-Tasten),
     angedockt an das bestehende `keyboardTarget`/`viewCommands`-System.
2. **Persistenz**: ein Key `fielddraft.controlsPreset` (Werte: `fielddraft`,
   `threejs`, `onshape`, `freecad`, `fusion`, `tinkercad`), gelesen wie die
   übrigen Settings in `SettingsMenu.tsx`; Default `fielddraft`.
3. **Anwendung**: `viewportCamera.tsx` liest das Preset und setzt
   `mouseButtons`/Speeds der OrbitControls; `Viewport.tsx` fragt die
   Selektions-Idiome ab, statt Modifier hart zu codieren. Die
   Klick-Entscheidung bleibt in `resolveElementClick` — sie bekommt das
   Preset als Parameter, sobald sich Presets dort unterscheiden.

## Umsetzungsphasen

1. **Phase 1 — Kamera-Bindings** (kleinster nützlicher Schritt):
   Preset-Registry + Settings-Dropdown + `mouseButtons`-Mapping. Damit sind
   Onshape/FreeCAD/TinkerCAD-Umsteiger bereits arbeitsfähig; LMB wird dort
   frei für reine Selektion.
2. **Phase 2 — Selektions-Idiome**: Modifier und Eskalationsverhalten pro
   Preset (z. B. Onshape-Doppelklick), Box-Select für TinkerCAD/Three.js.
3. **Phase 3 — Tastatur**: Shortcut-Tabellen pro Preset über
   `keyboardTarget.ts`; Konflikte mit App-Shortcuts (Sketch-Tools) auflösen.

Phase 1 ist bewusst klein gehalten und kann als eigenes Ticket umgesetzt
werden, sobald die Priorisierung es hergibt.
