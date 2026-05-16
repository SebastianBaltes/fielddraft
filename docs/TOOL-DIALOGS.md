# FieldDraft — Tool-Dialog-Spezifikationen (rekonstruiert aus Onshape‑YAMLs)

> **Diese Datei.** Authoritativer Referenz-Aufbau jedes Feature-Dialogs, rekonstruiert aus den YAML-Specs in `assets/onshape/yaml/*.yaml` (Onshape-Beobachtung, 2026-04) und ergänzt um Onshape-Domänenwissen, wo die YAMLs Lücken lassen. Diese Datei löst die fehlerhafte, ältere `onshape-ux.md` als Source of Truth für die **Dialog-Struktur** ab. UX-Philosophie und Sketch-Paritäts-Szenarien bleiben vorerst in `onshape-ux.md`.
>
> **Lesen, bevor du einen Feature-Dialog anfasst.** Jede Abweichung von diesen Specs braucht eine ausdrückliche Begründung — sonst landet man wieder in dem „zwei Dropdowns im Toolbar"-Problem, den der Agent zuletzt gefixt hat.
>
> **Status-Legende.**
> - ✅ *implementiert* — Schema und Widget-Kit decken den Dialog ab
> - 🚧 *teilweise* — Grundgerüst steht, Feinheiten fehlen (siehe Unterabschnitt)
> - ⏳ *offen* — Feature ist im Kernel noch nicht angelegt
>
> **Terminologie.** Ein *Selection-Input* ist ein Click‑Focus-Feld, das auf einen Viewport‑Pick wartet und in FieldDraft als Query gespeichert wird. *Tabs* sind horizontale Enum-Rows im Dialog-Header. *Modes* (New/Add/Remove/Intersect) sind Booleans auf einer inneren `operation`-Dropdown-Zeile, die bei aktivem Add/Remove/Intersect ein `merge_scope`-Selection-Input einblendet.

---

## Gemeinsame Muster

Alle Feature-Dialoge folgen demselben Gerüst, das ein einzelner generischer `FeatureDialog`-React-Component aus einem JSON-Schema rendert:

```
┌───────────────────────────────────────────────────┐
│  Header                                           │
│    [ Name‑Textfeld ]    [ Tab1 | Tab2 | Tab3 ]    │ ← optionale Header‑Tabs
│                                  [✓] [✗]          │ ← OK/Cancel oben rechts
├───────────────────────────────────────────────────┤
│  Body                                             │
│    • Selection‑Inputs (Click‑Focus auf Feld →     │
│      nächster Viewport‑Klick füllt die Query)     │
│    • Dropdowns mit `reveals:`‑Konditional‑Fields  │
│    • Expandable Sections (Checkbox‑Group) die     │
│      bei Aktivierung ihre Kinder einblenden       │
│    • visible_when‑Ausdrücke auf Parametern        │
├───────────────────────────────────────────────────┤
│  Footer                                           │
│    [ Diagnostic‑Tools ]          [ ? Help ]       │
└───────────────────────────────────────────────────┘
```

**Mode-Row Pattern** (für Features, die einem bestehenden Body etwas hinzufügen/abziehen: Extrude, Revolve, Sweep, Loft, Thicken, Enclose, Boolean, Linear/Circular Pattern, Mirror):

```
Mode:  [ New | Add | Remove | Intersect ]
         │      │      │        │
         │      │      │        └─ mode == intersect → merge_scope (Selection-Input, visible_if "Merge with all: False")
         │      │      └─ mode == remove → merge_scope
         │      └─ mode == add → merge_scope
         └─ mode == new → keine Targets, Ergebnis wird ein eigenständiger Body
```

- **New** = Feature erzeugt einen neuen, eigenständigen Body. Keine Ziel-Body-Referenz, kein impliziter Union‑Merge.
- **Add** = Boolean-Union: das Feature-Ergebnis wird mit den referenzierten Target-Bodies verschmolzen. Erfordert mindestens eine Target-Query. Standardmäßig aktiviert Onshape den Checkbox `merge_with_all`, der alle räumlich überlappenden Bodies einsammelt; deaktiviert man ihn, erscheint ein `merge_scope`-Selection-Input zur expliziten Auswahl.
- **Remove** = Boolean-Difference: das Feature wird aus den Target-Bodies herausgeschnitten.
- **Intersect** = Boolean-Intersect.

**Das ist das Pattern für explizite Booleans.** Es ersetzt den früheren impliziten "Union-Features mit überlappender AABB mergen automatisch"‑Ansatz, der in K7.22 aus dem FieldDraft‑Kernel gestrichen wurde.

**Name-Field** hat überall denselben Aufbau: `Text-Input` mit Default-Wert `<FeatureType> <N>` (z. B. "Extrude 1"), editierbar, taucht im Outliner als Row-Label auf.

**OK/Cancel** sitzen immer oben rechts als grüner Check / roter X — niemals unten. Commit auf OK, Rollback auf Cancel oder Esc.

---

## Kategorie 1 — Sketch und Sketch-basierte Features

### Sketch (`sketch.yaml`)

Einstieg in den 2D-Editor. Das Dialog-Schema ist in FieldDraft ein Sonder­fall: beim Klick auf **OK** startet nicht ein Commit, sondern die Sketch-Session mit dem ausgewählten Sketch-Plane. Die vollständige Dialog-Struktur ist aber dieselbe wie bei anderen Features.

**Header**
- `name_field` — Text-Input, Default "Sketch 1"
- `actions` — OK (grüner Check, "Finalize sketch") / Cancel (roter X)

**Body**
- `sketch_plane` — Selection-Input, Placeholder "Sketch plane", **Status `Active/Highlight`** (d. h. das Feld ist beim Öffnen des Dialogs bereits click-armed, ein Klick im Viewport füllt es), `mate_connector_invokable: true` (erlaubt auch Mate‑Connector‑Origins als Plane-Referenz)
  - Sub-Widgets im Feld: History-Icon (Parameter-History-Dropdown), Clear-Button (✗) nur sichtbar wenn Feld gefüllt
- **Options** (drei Checkboxes)
  - `disable_imprinting` — Default `false`, Checkbox-Text "Disable imprinting"
  - `show_constraints` — Default `true`, Hotkey `Shift+C` (global), Checkbox-Text "Show constraints"
  - `show_expressions` — Default `false`, zeigt Variablennamen/Formeln statt nur der Werte
  - `show_errors` — Default `true`, highlightet über-definierte oder konfliktierende Constraints

**Footer**
- `sketch_diagnostics` — Dropdown-Button ("Sketch diagnostics", Pencil-with-Magnifier-Icon) mit Menüeinträgen:
  - `profile_inspector` — öffnet ein Sub-Panel mit Loose-Ends-Liste (`No loose ends found`-Success-Status, Previous/Next-Navigation, Close-Icon, Help-Icon)
  - `constraint_manager` — öffnet ein Sub-Panel mit:
    - Header: Close-Icon, Filters-Dropdown, Toggle "Automatically select constraints" (Default true)
    - Icon-Gruppen für Constraint-Typen: `[Coincident, Concentric, Parallel, Tangent, Horizontal, Vertical, Perpendicular, Equal, Midpoint, Normal, Pierce, Symmetric, Fix, Equal curvature]`
    - Icon-Gruppen für Modes: `[Internal, External, In-context]`
    - Icon-Gruppen für Statuses: `[Driven, Solved, Overconstrained]`
    - Navigation: Tabs `[Sort by constraint, Sort by entity]`, Search-Input
    - Actions: "Delete all"-Button (disabled bei leerer Auswahl)
- `help_link` — Link-Icon (?), Tooltip "Help"

**FieldDraft-Status:** 🚧 Sketch-Plane-Selection-Input fehlt Click-Focus (K7.21e). Constraint-Manager fehlt komplett (K7.21d). Profile-Inspector ist optional, niedrige Priorität.

---

### Extrude (`extrude.yaml`)

**Header**
- `name_field` — Text-Input, Default "Extrude 1"
- `tabs` — `[Solid (active), Surface, Thin]`
- `actions` — OK / Cancel

**Body (Tab „Solid")**

- **Mode-Row** (inline Enum, nicht Header-Tab): `[New, Add, Remove, Intersect]`
- `faces_and_regions` — Selection-Input "Faces and sketch regions to extrude", mate_connector_invokable
- `end_condition` — Dropdown mit `reveals:`
  - **Blind** → reveals `depth`, `symmetric`
  - **Symmetric** → reveals `depth`; deaktiviert "Second end position"
  - **Up_to_next** → reveals nichts
  - **Up_to_face** → reveals `up_to_face_entity` (Selection-Input), `offset_distance_checkbox`
  - **Up_to_part** → reveals `up_to_part_entity` (Selection-Input), `offset_distance_checkbox`
  - **Up_to_vertex** → reveals `up_to_vertex_entity` (Selection-Input), `offset_distance_checkbox`
  - **Through_all** → reveals nichts
- **Direction** — Expandable-Section; wenn aktiviert, erscheint `direction_entity` (Selection-Input, mate_connector_invokable)
- **Starting offset** — Checkbox-Group; wenn aktiviert, erscheinen:
  - `type` — Dropdown `[Blind, Entity]`
  - `offset_distance` — Distance-Input, visible_if "Type: Blind"
  - `offset_entity` — Selection-Input, visible_if "Type: Entity"
  - `opposite_direction` — Toggle
- **Draft** — Checkbox-Group; wenn aktiviert:
  - `draft_angle` — Angle-Input, Default "3 deg"
  - `opposite_direction` — Toggle
- **Second end position** — Checkbox-Group, visible_if "Symmetric: False"; wenn aktiviert:
  - `second_end_condition` — Dropdown, gleicher Optionssatz wie `end_condition`
  - `second_depth` — Distance-Input, visible_if "Second end condition: Blind"
  - `second_draft` — Checkbox-Group mit `second_draft_angle`, `second_draft_opposite`
- **Merge scope** (bei Mode Add/Remove/Intersect):
  - `merge_with_all` — Checkbox, Default `true`
  - `merge_scope` — Selection-Input, visible_if "Merge with all: False"

**Body (Tab „Surface")** — wie Solid, aber ohne top-level `Draft`, mit surface-spezifischen End-Condition-Sonderfällen.

**Body (Tab „Thin")** — fügt Wandstärken-Parameter hinzu:
- `mid_plane` — Checkbox
- `thickness_type` — Dropdown `["One direction", "Symmetric", "Two directions"]`
- `thickness_1` — Distance-Input
- `thickness_2` — Distance-Input, visible_if "Type: Two directions"

**Footer**
- `diagnostic_tools` — Button (Pencil-Magnifier, "Show analysis")
- `help` — Button
- `rollback_status` — Progress-Bar am Dialog-Boden (interaktiv)

**FieldDraft-Status:** 🚧 Solid-Tab: Mode-Row existiert als BoolOp (horizontal_enum). End-Condition-Dropdown mit reveals ist im FeatureDialog grundsätzlich unterstützt (K6a.1). Profiles-Picker arbeitet per Click-Focus (K7.22g). **Fehlt:** Surface/Thin-Tabs, Direction/Starting-Offset/Draft/Second-End-Position Expandable Sections, Merge-Scope-Logik (gebraucht sobald die Mode-Row auch Add/Remove/Intersect als eigenständige Booleans liefern soll).

---

### Revolve (`revolve.yaml`)

**Header**
- `name_field` — Text-Input, Default "Revolve 1"
- `tabs` — `[Solid (active), Surface, Thin]`
- `actions` — OK / Cancel

**Body (Tab „Solid")**
- **Modes:** `[New, Add, Remove, Intersect]`
- `faces_and_sketch_regions` — Selection-Input
- `revolve_axis` — Selection-Input
- `full_revolve` — Checkbox, Default `true`; wenn `false`:
  - `revolve_type` — Dropdown:
    - **Blind** → reveals `revolve_angle`, `symmetric`, `second_end_position`
    - **Symmetric** → reveals `revolve_angle`; Note: "Splits total angle equally; disables Second end position"
    - **Up_to_next** / **Up_to_face** / **Up_to_part** / **Up_to_vertex** — wie bei Extrude
  - `revolve_angle` — Angle-Input, Default "30 deg"
  - `second_end_position` — Checkbox-Group mit `second_revolve_type` (Dropdown, "Same as revolve_type") und `second_revolve_angle` (Angle-Input, visible_if "Type: Blind")
- **Merge scope** (Mode Add/Remove/Intersect): wie Extrude

**Body (Tab „Surface")**
- **Modes:** `[New, Add]` (kein Remove/Intersect)
- `edges_and_curves` — Selection-Input "Edges and sketch curves to revolve"
- `revolve_axis` — Selection-Input
- `full_revolve` — Checkbox, Default `true`
- Hinweis: Surface wird oft für Konstruktionsgeometrie genutzt.

**Body (Tab „Thin")**
- **Modes:** `[New, Add, Remove, Intersect]`
- Zusätzlich: `mid_plane` (Checkbox), `thickness_type` (Dropdown "One direction"/"Symmetric"/"Two directions"), `thickness_1` (Distance-Input, Default "5 mm"), `thickness_2` (Distance-Input, visible_if "Type: Two directions")

**FieldDraft-Status:** 🚧 Solid-Tab ist über `addRevolveFromLatestSketch` erreichbar, aber ohne Dialog (Hart-verdrahtete Achse, kein Angle-Picker). Tabs und Revolve-Type-Dropdown fehlen.

---

### Sweep (`sweep.yaml`)

**Header**
- `name_field` — Text-Input, Default "Sweep 1"
- `tabs` — `[Solid (active), Surface, Thin]`

**Body (Tab „Solid")**
- **Modes:** `[New, Add, Remove, Intersect]`
- `faces_and_regions` — Selection-Input "Faces and sketch regions to sweep"
- `sweep_path` — Selection-Input, mate_connector_invokable
- `profile_control` — Dropdown "Profile control":
  - **None** → reveals `twist_section`, `scale_section`
  - **Keep_profile_orientation** → reveals `twist_section`, `scale_section`
  - **Lock_profile_faces** → reveals `faces_to_lock` (Selection-Input), `twist_section`, `scale_section`
  - **Lock_profile_direction** → reveals `direction_selection`, `twist_section`, `scale_section`
- **Twist** — Checkbox-Group; wenn aktiviert:
  - `twist_type` — Dropdown:
    - **Turns** → `revolutions_input`
    - **Angle** → `twist_angle_input`
    - **Pitch** → `pitch_distance_input`
- **Scale** — Checkbox-Group; wenn aktiviert:
  - `scale_factor` — Number-Input, Default `1.0`
- `trim_ends` — Checkbox, Default `false`
- **Merge scope** (Mode Add/Remove/Intersect): wie Extrude

**Body (Tab „Surface")** — wie Solid, aber für Surfaces.

**Body (Tab „Thin")**
- `mid_plane` — Checkbox
- `thickness_type` — Dropdown "One direction"/"Symmetric"/"Two directions"
- `thickness_1`, `thickness_2`

**FieldDraft-Status:** 🚧 Kernel hat K13.2 Sweep-Feature (Profil + Pfad + Twist), aber kein FeatureDialog — das Sweep-Feature wird nur über Tests angesteuert. Volle Dialog-Implementierung gehört zu K13-Abschluss.

---

### Loft (`loft.yaml`)

**Header**
- `name_field` — Text-Input, Default "Loft 1"
- `tabs` — `[Solid (active), Surface, Thin]`

**Body (Tab „Solid")**
- **Modes:** `[New, Add, Remove, Intersect]`
- `profiles` — Selection-List, "Ordered list of sections", mate_connector_invokable (Reihenfolge wichtig)
- **Start/End Conditions**:
  - `start_condition` — Dropdown:
    - **None** — keine Reveals
    - **Normal_to_profile** → `start_magnitude`
    - **Tangent_to_profile** → `start_magnitude`
    - **Match_tangent** → `adjacent_faces` (Selection-Input), `start_magnitude`
    - **Match_curvature** → `adjacent_faces`, `start_magnitude`
    - **Normal_direction** → `direction_selection`, `start_magnitude`
    - **Tangent_direction** → `direction_selection`, `start_magnitude`
  - `end_condition` — Dropdown mit derselben Options-Logik wie `start_condition`
- **Guides and continuity** — Checkbox-Group; wenn aktiviert:
  - `guides` — Selection-List
  - `continuity` — Dropdown pro Guide, Optionen `[G0, G1, G2]`
- **Path** — Checkbox-Group; wenn aktiviert:
  - `path_entities` — Selection-Input "Edges, curves and sketches"
  - `section_count` — Number-Input, Default `5`
- **Connections** — Expandable-Section:
  - `match_connections` — Checkbox
  - `connections_list` — Selection-List, visible_if `match_connections: True`
- **Merge scope** (Mode Add/Remove/Intersect): wie Extrude

**Body (Tab „Surface")**
- **Modes:** `[New, Add]`
- `profiles`, `trim_profiles` (Checkbox), `trim_guides` (Checkbox, visible_if "Guides active")

**Body (Tab „Thin")**
- **Modes:** `[New, Add, Remove, Intersect]`
- `mid_plane`, `thickness_1`, `thickness_2` (visible_if "Mid plane: False"), `trim_profiles`, `trim_ends`

**Footer**
- `isocurves` — Checkbox "Show isocurves" (Dialog-Footer-Option)
- `diagnostic_tools`, `help`

**FieldDraft-Status:** ⏳ Nicht in der Roadmap bis T3+.

---

### Thicken (`thicken.yaml`)

**Header**
- `name_field` — Text-Input, Default "Thicken 1"

**Body**
- **Modes:** `[New, Add, Remove, Intersect]`
- `entities_to_thicken` — Selection-Input "Faces or surfaces to thicken"
- `mid_plane` — Checkbox, Default `false`
  - wenn `true`: `thickness` (Distance-Input "Thickness")
  - wenn `false`: `thickness_1`, `thickness_2`, `opposite_direction` (Toggle-Button)
- **Direction** — Expandable-Section (optional override für Face-Normal):
  - `direction_entity` — Selection-Input "Direction"
- **Options**
  - `keep_tools` — Checkbox "Keep tools", Default `false`
- **Merge scope** (Mode Add/Remove/Intersect): wie Extrude

**FieldDraft-Status:** ⏳ Nicht im Kernel.

---

### Helix / Thread (`helix.yaml`)

Onshape trennt Helix (Kurvenbildung) und Thread (Gewinde-Primitiv) nicht so sauber wie FieldDraft. Die YAML bezieht sich auf das Helix-Feature, das eine Spirale als Curve oder als extrudierten Solid erzeugt. FieldDraft hat stattdessen ein analytisches `thread`-Primitiv (K12).

**Header**
- `name_field` — Text-Input, Default "Helix 1"
- `tabs` — `[Cylinder_Cone (active), Axis, Circle]`
- `actions` — OK / Cancel

**Body — Common Parameters** (gelten in allen Tabs)
- `input_type` — Dropdown:
  - **Turns** → reveals `revolutions_field`
  - **Pitch** → reveals `pitch_field`
  - **Turns_and_pitch** → reveals `revolutions_field`, `pitch_field`
- `direction` — Dropdown `[Clockwise, Counter_clockwise]`
- **Options**
  - `show_profiles` — Checkbox "Show start and end profiles", Default `false`

**Body (Tab „Cylinder_Cone")**
- `reference` — Selection-Input "Conical or cylindrical face", mate_connector_invokable
- `opposite_direction` — Icon-Button (Flip)

**Body (Tab „Axis")**
- `helix_axis` — Selection-Input "Helix axis"
- `start_condition` — Dropdown:
  - **Start_angle** → `angle_input`, `start_radius_input`
  - **Start_point** → `point_selection` (Selection-Input, mate_connector_invokable)

**Body (Tab „Circle")**
- `circular_edge` — Selection-Input "Circular edge"
- `end_condition` — Dropdown:
  - **Height** → `height_input`
  - **End_point** → `point_selection`

**FieldDraft-Status:** 🚧 Kernel hat ein `thread`-Primitiv (`ThreadDef`) mit `nominal_radius`, `pitch`, `depth`, `half_height`. Der aktuelle Dialog zeigt die vier Zahlen direkt, ohne Tabs/Dropdowns. Für echte Onshape-Parität: Tab-Row implementieren + `input_type`-Dropdown, dann das Preset-Dropdown aus der Toolbar entfernt (bereits in K7.22j geschehen). Voller Helix-Dialog gehört zu K7.21g.

---

### Enclose (`enclose.yaml`)

**Header**
- `name_field` — Text-Input, Default "Enclose 1"

**Body**
- **Modes:** `[New, Add, Remove, Intersect]`
- `entities_to_enclose` — Selection-Input "Entities", "Surfaces and planes to enclose"
- **Options**
  - `keep_tools` — Checkbox, Default `false`
- **Merge scope** (Mode Add/Remove/Intersect)

**FieldDraft-Status:** ⏳ Nicht im Kernel.

---

## Kategorie 2 — Modifying Tools

### Fillet (`fillet.yaml`)

**Header**
- `name_field` — Text-Input, Default "Fillet 1"
- `tabs` — `[Edge (active), Full_round]`

**Body (Tab „Edge")**
- `entities_to_fillet` — Selection-Input "Entities to fillet"
- `measurement` — Dropdown:
  - **Radius** → reveals `control`, `asymmetric_checkbox`, `partial_fillet_checkbox`, `variable_fillet_checkbox`, `overflow_checkbox`, `smooth_corners_checkbox`
  - **Width** → reveals `control`, `tangent_propagation_checkbox` (vereinfacht die UI signifikant)
- `control` — Dropdown:
  - **Distance** → reveals `radius_input`
  - **Conic** → reveals `radius_input`, `rho_input`
  - **Curvature** → reveals `radius_input`, `magnitude_input`
- **Checkbox-Expansions** (alle Checkboxen können zusätzliche Felder einblenden):
  - **Asymmetric** → reveals `second_radius_input`
  - **Partial fillet** → reveals:
    - `bounds_type` — Dropdown `[Offset, Position]`
    - `start_position` — Distance-Input
    - `second_bound` — Checkbox, reveals `end_position_input`
  - **Variable fillet** → reveals:
    - `vertices_selection` — Selection-Input "Vertices and points on edges"
    - `smooth_transition` — Checkbox, Default `true`
- **Options**
  - `tangent_propagation` — Checkbox, Default `true`
  - `allow_edge_overflow` — Checkbox, Default `false`
  - `smooth_fillet_corners` — Checkbox, Default `true`

**Body (Tab „Full_round")**
- `side_face_set_1` — Selection-Input "Side face set 1"
- `center_face_set` — Selection-Input "Center face set"
- `side_face_set_2` — Selection-Input "Side face set 2"
- **Options**
  - `tangent_propagation` — Checkbox, Default `true`
  - `allow_edge_overflow` — Checkbox, Default `false`

**FieldDraft-Status:** 🚧 Kernel-Fillet (K10) funktioniert mit einem einzigen Radius-Input über Spatial Masks pro Edge. Tabs, Measurement-Dropdown, Partial/Variable-Fillet, Full-Round-Tab fehlen.

---

### Chamfer (`chamfer.yaml`)

**Header**
- `name_field` — Text-Input, Default "Chamfer 1"

**Body**
- `entities_to_chamfer` — Selection-Input "Entities to chamfer"
- `measurement` — Dropdown:
  - **Offset** — "Measures distance from the imaginary intersection point"
  - **Tangent** — "Measures distance along the adjacent faces"
- `chamfer_type` — Dropdown:
  - **Equal_distance** → reveals `distance_input`
  - **Two_distances** → reveals `distance_1_input`, `distance_2_input`, `flip_direction_button`
  - **Distance_and_angle** → reveals `distance_input`, `angle_input`, `flip_direction_button`
- **Numerical inputs** (je nach Chamfer-Type)
  - `distance` — Distance-Input "Distance"
  - `distance_1`, `distance_2` — Distance-Inputs
  - `angle` — Angle-Input, Default "45 deg"
  - `flip_direction` — Icon-Button (Opposite-Arrow)
- **Options**
  - `tangent_propagation` — Checkbox, Default `true`
  - `direction_overrides` — Selection-List, "Allows flipping direction for specific edges in a set"

**FieldDraft-Status:** 🚧 Kernel-Chamfer ist über `ChamferDef` im Registry, Dialog rendert die Felder aber flach ohne Dropdown/Tab-Struktur.

---

### Draft (`draft.yaml`)

**Header**
- `name_field` — Text-Input, Default "Draft 1"
- `tabs` — `[Neutral_plane (active), Parting_line]`

**Body (Tab „Neutral_plane")**
- `pull_direction` — Selection-Input "Pull direction", optional (defaults to Neutral Plane normal)
- `neutral_plane` — Selection-Input "Neutral plane"
- `entities_to_draft` — Selection-Input "Entities to draft"
- `draft_angle` — Angle-Input, Default "3 deg", mit Flip-Toggle (Icon-Button)
- **Options**
  - `tangent_propagation` — Checkbox, Default `true`
  - `reapply_fillet` — Checkbox, Default `false`

**Body (Tab „Parting_line")**
- `pull_direction` — Selection-Input
- `parting_lines` — Selection-Input
- `entities_to_draft` — Selection-Input
- `sides_selection` — Dropdown:
  - **One_sided** → reveals `draft_angle_1`
  - **Symmetric** → reveals `draft_angle_1` (gilt für beide Seiten)
  - **Two_sided** → reveals `draft_angle_1`, `draft_angle_2`
- `draft_angle_1`, `draft_angle_2` — Angle-Inputs mit Flip-Toggles
- **Options**
  - `parting_line_propagation` — Checkbox, Default `true` (umbenannt von Tangent Propagation)
  - `reapply_fillet` — Checkbox, Default `false`

**FieldDraft-Status:** ⏳ Nicht im Kernel.

---

### Shell (`shell.yaml`)

**Header**
- `name_field` — Text-Input, Default "Shell 1"

**Body**
- `hollow_toggle` — Checkbox "Hollow", Default `false`
  - wenn `true` (Hollow): Selection-Input-Label wird zu "Parts to hollow"
  - wenn `false`: Label wird zu "Faces to remove"
- `selection_box` — Selection-Input mit dynamischem Label (siehe `hollow_toggle`)
- `thickness_control` — Composite-Input:
  - `thickness_input` — Distance-Input "Shell thickness", Default "2.5 mm"
  - `thickness_slider` — Slider, dynamische Range
  - `opposite_direction` — Icon-Button (Flip-Arrow, "Opposite direction")

**FieldDraft-Status:** ⏳ Nicht im Kernel (shell_thickness ist ein Feld in SpineFeature, aber ohne dedicated Feature).

---

## Kategorie 3 — Patterns und Transform

### Linear Pattern (`linear-pattern.yaml`)

**Header**
- `name_field` — Text-Input, Default "Linear pattern 1"
- `pattern_type` — Dropdown (oben, nicht Tab):
  - **Part_pattern** — entities_label: "Entities to pattern", zeigt Boolean-Modes
  - **Feature_pattern** — entities_label: "Features to pattern", reveals `reapply_features_checkbox`
  - **Face_pattern** — entities_label: "Faces to pattern"

**Body**
- **Modes:** `[New, Add, Remove, Intersect]` — nur visible_if `Type: Part_pattern`
- **Direction 1** (Pflicht)
  - `direction_input` — Selection-Input "Direction"
  - `distance` — Distance-Input "Distance", Default "25 mm"
  - `instance_count` — Number-Input "Instance count", Default "2"
  - `centered` — Checkbox "Centered", Default `false`
  - `flip_direction` — Icon-Button (Opposite-Arrow)
- **Second direction** — Checkbox-Group; wenn aktiviert:
  - `direction_input`, `distance`, `instance_count`, `centered`, `flip_direction` (gleiche Felder wie Direction 1)
- **Options**
  - `reapply_features` — Checkbox, visible_if `Type: Feature_pattern`, Default `true`
  - **Skip instances** — Checkbox; wenn aktiviert: `instances_to_skip` (Selection-Input "Instances to skip")
  - **Merge scope** (visible_if Mode Add/Remove/Intersect): wie Extrude

**FieldDraft-Status:** 🚧 Kernel hat Linear-Pattern-Feature. Dialog-Gerüst existiert, aber ohne pattern_type-Dropdown und ohne Second-Direction-Support.

---

### Circular Pattern

Onshape's Circular Pattern folgt demselben Grundmuster wie Linear Pattern, aber statt Direction/Distance:
- `pattern_type` — `[Part_pattern, Feature_pattern, Face_pattern]`
- **Rotation axis** — Selection-Input
- **Angular spacing** — Angle-Input
- **Instance count** — Number-Input
- **Direction equal spacing** — Checkbox (Default `true`, verteilt Instanzen gleichmäßig)
- Sonst wie Linear Pattern

**FieldDraft-Status:** 🚧 Kernel hat Circular-Pattern. Dialog wie Linear-Pattern.

---

### Mirror (`mirror.yaml`)

**Header**
- `name_field` — Text-Input, Default "Mirror 1"
- `mirror_type` — Dropdown "Mirror type":
  - **Part_mirror** — entities_label: "Entities to mirror", zeigt Boolean-Modes
  - **Feature_mirror** — entities_label: "Features to mirror"
  - **Face_mirror** — entities_label: "Faces to mirror"

**Body**
- **Modes:** `[New, Add, Remove, Intersect]` — visible_if `Type: Part_mirror`
- `entities` — Selection-Input (dynamisches Label: Entities / Features / Faces)
- `mirror_plane` — Selection-Input "Mirror plane"
- **Options**
  - `reapply_features` — Checkbox, visible_if `Type: Feature_mirror`, Default `true`
  - **Merge scope** (visible_if `Type: Part_mirror`): wie Extrude

**FieldDraft-Status:** 🚧 Kernel-Mirror existiert. Dialog rendert Mirror-Plane als Query-Input. Mirror-Type-Dropdown und Mode-Row fehlen.

---

### Transform (`transform.yaml`)

**Header**
- `name_field` — Text-Input, Default "Transform 1"

**Body**
- `entities_to_transform` — Selection-Input "Entities to transform or copy"
- `transform_mode` — Dropdown:
  - **Translate_by_line** → `direction_selection`, `distance_input`
  - **Translate_by_distance** → `direction_selection`, `distance_input`
  - **Translate_by_XYZ** → `dx_input`, `dy_input`, `dz_input`
  - **Transform_by_mate_connectors** → `from_mate_connector`, `to_mate_connector`
  - **Rotate** → `axis_selection`, `angle_input`
  - **Copy_in_place** → keine Reveals (immer Copy)
  - **Scale** → `scale_uniformly_checkbox`, `scale_input`, `point_or_connector`
- **Mode-spezifische Details**
  - **Scale-Logik**:
    - `scale_uniformly` — Checkbox, Default `true`
    - uniform fields: `scale_factor`
    - non-uniform fields (visible_if `scale_uniformly: False`): `scale_x`, `scale_y`, `scale_z`
    - `point_reference` — Selection-Input "Point or mate connector", Pflicht für Scaling
- **Options**
  - `copy_part` — Checkbox "Copy part", Default `false` (implizit `true` für Copy_in_place)

**FieldDraft-Status:** ⏳ Transform-als-eigenes-Feature fehlt. Aktuell ist `translate/rotation/scale` in jeder `SpineFeature` eingebacken — das Direct-Manipulation-Gizmo editiert diese inline. K7.21j zieht das in ein eigenes `TransformDef`-Feature um.

---

## Kategorie 4 — Boolean und Konstruktion

### Boolean (`boolean.yaml`)

**Header**
- `name_field` — Text-Input, Default "Boolean 1"
- `tabs` — `[Union (active), Subtract, Intersect]`

**Body (Tab „Union")**
- `tools` — Selection-Input "Tools", "Parts to join together"

**Body (Tab „Subtract")**
- `tools` — Selection-Input "Tools", "Parts to use as cutters"
- `targets` — Selection-Input "Targets", "Parts to be cut"
- **Offset** — Checkbox-Group; wenn aktiviert:
  - `offset_all` — Checkbox "Offset all", Default `false`
  - `faces_to_offset` — Selection-Input, visible_if "Offset all: False"
  - `offset_distance` — Distance-Input
  - `reapply_fillet` — Checkbox

**Body (Tab „Intersect")**
- `tools` — Selection-Input "Tools", "Parts to find intersection of"

**Common options** (alle Tabs)
- `keep_tools` — Checkbox "Keep tools", Default `false`

**FieldDraft-Status:** ⏳ Explicit Boolean-Feature fehlt. Das ersetzt den entfernten Toolbar-OP-Switch in K7.22 (K7.21j follow-up). Kritisch für Merges, die der User manuell fordert.

---

### Split (`split.yaml`)

**Header**
- `name_field` — Text-Input, Default "Split 1"
- `tabs` — `[Part (active), Face]`

**Body (Tab „Part")**
- `entities_to_split` — Selection-Input "Parts, surfaces, or curves to split"
- `split_tool` — Selection-Input "Entity to split with"
- **Options**
  - `keep_tools` — Checkbox, Default `false`
  - `trim_to_face` — Checkbox "Trim to face boundaries"
  - `keep_both_sides` — Checkbox, Default `true`

**Body (Tab „Face")**
- `faces_to_split` — Selection-Input "Faces to split"
- `split_tool` — Selection-Input "Entities to split with"
- **Edge projection options** — Expandable-Section:
  - `projection_type` — Dropdown:
    - **Normal_to_target** → keine Reveals
    - **Direction** → `direction_selection`, `use_sketch_plane_checkbox`
  - `keep_tool_entities` — Checkbox "Keep tool surfaces and curves", Default `false`

**FieldDraft-Status:** ⏳ Nicht im Kernel.

---

### Plane (`plane.yaml`)

**Header**
- `name_field` — Text-Input, Default "Plane 1"

**Body**
- `plane_type` — Dropdown:
  - **Offset** → `entities`, `offset_distance`, `flip_normal`
  - **Plane_point** → `entities`, `point_selection`, `flip_normal`
  - **Line_angle** → `entities`, `line_selection`, `angle_input`, `flip_normal`
  - **Point_normal** → `point_selection`, `line_selection`, `flip_normal`
  - **Three_point** → `points_selection`, `flip_normal`
  - **Mid_plane** → `entities`, `flip_normal`
  - **Curve_point** → `curve_selection`, `point_selection`, `flip_normal`, `flip_alignment`
  - **Tangent** → `surface_selection`, `point_or_plane`, `flip_normal`, `flip_alignment`
- **Numerical inputs**
  - `offset_distance` — Distance-Input "Offset distance", Default "25 mm"
  - `angle` — Angle-Input "Angle", Default "0 deg"
- **Options**
  - `flip_normal` — Checkbox "Flip normal"
  - `flip_alignment` — Checkbox "Flip alignment", visible_if `Type: [Curve_point, Tangent]`

**FieldDraft-Status:** ⏳ Kernel hat nur die drei Origin-Planes (XY/XZ/YZ). Offset- und andere Plane-Typen sind für K7.21e-Follow-up.

---

### Mate Connector (`mate-connector.yaml`)

Vollwertige Version. Es gibt auch eine implizite Mate-Connector-Sub-Dialog-Variante in `implicit-mate-connector.yaml`, die innerhalb anderer Dialoge auftaucht, wenn ein Selection-Input `mate_connector_invokable: true` ist.

**Header**
- `name_field` — Text-Input, Default "Mate connector 1"

**Body**
- `mode_selection` — Dropdown:
  - **On_entity** → `origin_selection` (1 Selection-Input)
  - **Between_entities** → `origin_selection_1`, `origin_selection_2`
- **Alignment options**
  - **Realign** — Checkbox; wenn aktiviert:
    - `primary_axis_entity` — Selection-Input "Primary axis entity"
    - `secondary_axis_entity` — Selection-Input "Secondary axis entity"
- **Transformation options**
  - **Move** — Checkbox; wenn aktiviert:
    - **Translation**: `x_input`, `y_input`, `z_input` (Distance-Inputs, Default "0 mm")
    - **Rotation**: `axis_dropdown` (Dropdown "Rotate about" `[X, Y, Z]`), `angle_input` (Angle-Input "Angle", Default "0 deg")
- **Logic options**
  - **Owner entity** — Checkbox, Default `true`; wenn aktiviert:
    - `owner_selection` — Selection-Input "Select owner entity"

**Implicit Mate Connector** (Sub-Dialog innerhalb anderer Dialoge):
- Erscheint als Icon-Popover, wenn der User im Hover über einem Selection-Input (der `mate_connector_invokable: true` ist) einen Entity-Pick anbietet.
- Close-Button (X) in der Header-Bar
- Header: Titel "Mate connector"
- Inputs:
  - `origin_type` — Enum-Dropdown "Origin type" (Default "On entity")
  - `origin_entity` — Selection-Input "Origin entity" (Beispielwert "Edge of Sketch 3")
  - `realign` — Checkbox; wenn aktiviert: `primary_axis`, `secondary_axis` (Selection-Inputs)
  - `move` — Checkbox; wenn aktiviert: Quantity-Inputs für `translationX/Y/Z`, `rotationType` (Enum "Rotate about X"), `rotation` (Angle-Input "0 deg")
  - Axis-Controls: `flipPrimary` (Icon-Button), `secondaryAxisType` (Icon-Button "Reorient secondary axis")
- Footer: Help-Link mit URL-Anker `#mateconnector`

**FieldDraft-Status:** ⏳ K9-Mate-Connector fehlt noch im Outliner-Dialog. Kernel-seitig existiert `addMateConnectorAtPick`, aber der FeatureDialog ist nicht angeschlossen.

---

### Hole (`hole.yaml`)

**Header**
- `type_summary` — Label (z. B. `ø 6.528 mm ↧ 12 mm`)
- `measurement_system` — Segmented-Control:
  - **Inch** → standards `[ANSI]`
  - **Metric** (active) → standards `[ISO, DIN]`
- `hole_form_tabs` — Tabs:
  - **Simple** (active)
  - **Counterbore** → reveals `cbore_dia`, `cbore_depth`
  - **Countersink** → reveals `csink_dia`, `csink_angle`

**Body**
- **Primary parameters**
  - `locations` — Selection-Input "Sketch points and mate connectors"
  - `merge_scope` — Selection-Input "Merge scope"
- `hole_style` — Dropdown:
  - **Drilled** → `diameter`, `point_angle`
  - **Clearance** → `standard`, `size`, `fit`, `clearance_dia`
  - **Tapped** → `standard`, `size`, `thread_pitch`, `thread_class`, `tap_drill_dia`
- **Standard selection** (visible_if `Style: [Clearance, Tapped]`)
  - `standard` — Dropdown (ISO/DIN/ANSI)
  - `size` — Dropdown (M3/M4/…)
  - `fit` — Dropdown (visible_if `Style: Clearance`)
- **Details**
  - `diameter` — Distance-Input "Hole diameter"
  - `point_angle` — Angle-Input "Point angle", Options `["118 deg", "135 deg", "Flat"]`
  - `cbore_dia`, `cbore_depth` — Distance-Inputs
  - `csink_dia`, `csink_angle`
- `termination` — Dropdown:
  - **Blind** → `depth`
  - **Through_all** → keine Reveals
  - **Up_to_next** → keine Reveals
  - **Up_to_entity** → `target_entity`, `offset_from_tip`
- `start_logic` — Dropdown "Start plane":
  - **Start_from_sketch_plane**
  - **Start_from_part**
  - **Start_from_face** → `start_face_selection`

**Footer**
- `show_custom_clearance` — Checkbox
- `opposite_direction` — Icon-Button (Flip)

**FieldDraft-Status:** 🚧 Kernel hat `HoleDef` mit `diameter` und `depth`. Dialog rendert diese beiden Felder. Alles andere (Hole-Style, Standard-Dropdowns, Counterbore/Countersink-Tabs, Termination-Dropdown) fehlt.

---

## Kategorie 5 — Primitives (FieldDraft-spezifisch)

Onshape hat keine „Box/Sphere/Cylinder"-Primitive — parametrische Volumen entstehen dort ausschließlich über Sketch + Extrude/Revolve. FieldDraft bietet Primitive als Direct-Manipulation-Shortcut an. Die Dialog-Struktur folgt trotzdem dem Onshape-Muster:

**Header**
- `name_field` — Text-Input, Default "<Kind> <N>"
- **Mode-Tabs (K7.22/9)** — `[New, Add, Remove, Intersect]` (horizontal_enum-Rendering der `op: BoolOp`)
- `actions` — OK / Cancel

**Body (nur Size/Shape-Parameter, kein Center/Translate/Blend/Chamfer/Twist/Bend/Displace/Shell)**
- **Box** — `half_extents[3]` (Distance-Inputs für X, Y, Z)
- **Sphere** — `radius` (Distance-Input)
- **Cylinder** — `radius`, `half_height`
- **Cone** — `radius_bottom`, `radius_top`, `half_height`
- **Torus** — `major_radius`, `minor_radius`
- **Capsule** — `radius`, `half_height`
- **Hex Prism** — `apothem`, `half_height`
- **Ellipsoid** — `radii[3]`
- **Thread** — `nominal_radius`, `pitch`, `depth`, `half_height` (besser: zukünftig ein Tab-Dialog wie Helix, siehe K7.21g)

**Merge Scope** (bei Mode Add/Remove/Intersect):
- `merge_with_all` — Checkbox, Default `true`
- `merge_scope` — Selection-Input "Target parts", visible_if "Merge with all: False"

**Footer**
- `help` — Button

**Hidden in K7.22 (über `HIDDEN_PRIMITIVE_PARAM_KEYS` in `schema.ts`):**
- `center`, `translate` — gehören zum Transform-Feature (K7.21j)
- `op` — wird durch die Mode-Row repräsentiert, nicht als separates Feld
- `blend_radius`, `chamfer_size` — gehören zum Fillet/Chamfer-Feature
- `twist_rate`, `bend_rate` — gehören zum Deform-Feature (K11)
- `displace_depth`, `displace_freq` — gehören zum Displacement-Feature (K11)
- `shell_thickness` — gehört zum Shell-Feature

**FieldDraft-Status:** 🚧 Hidden-Filter in `schema.ts` ist aktiv (K7.22/9). Mode-Row via BoolOp rendert bereits als horizontal_enum-Tabs. Merge-Scope-Logik fehlt — das ist der K7.21j-Folgeschritt.

---

## Anhang: Widget-Kit-Mapping

Für die generische FeatureDialog-Infrastruktur (`web/src/widgets/widgets.tsx`) gibt es folgenden Mapping zwischen YAML-Widget-Typen und FieldDraft-Widgets:

| YAML-Typ | FieldDraft-Widget | Notizen |
|---|---|---|
| `Text-Input` | `StringInput` (nicht explizit benannt) | Für `name_field` und allgemeine Strings |
| `Distance-Input` | `LengthInput` | Unit-aware, Formel-support aus §4.5 |
| `Angle-Input` | `AngleInput` | deg/rad-toggleable |
| `Number-Input` | `LengthInput` (dimensionless) | Wenn integer: Slider mit step=1 |
| `Checkbox` | `BoolCheckbox` | |
| `Icon-Button` / Flip-Button | `FlipButton` | `ui_hint: "opposite_direction"` |
| `Toggle` | `BoolCheckbox` oder `FlipButton` | Kontext-abhängig |
| `Dropdown` | `EnumWidget` (dispatched) | Mit `reveals:`-Logic über `visible_when` |
| `Dropdown` mit `ui_hint: horizontal_enum` | `EnumWidget` als Tab-Row | Für Mode-Selectors (New/Add/Remove/Intersect) |
| `Selection-Input` | `QueryWidget` (Click-Focus) | Ist in K7.22g click-to-pick; leerer Required → auto-armed |
| `Selection-List` | `QueryListWidget` | ⏳ fehlt — derzeit fallback auf Textarea |
| `Slider` | `SliderWidget` | `ui_hint: "slider"` + min/max/step |
| `Segmented-Control` | `EnumWidget` (horizontal_enum) | |
| `Checkbox-Group` | `GroupHeader` + children | Parent-Checkbox treibt `drivingParam` |
| `Expandable-Section` | `GroupHeader` (Chevron-only) | Wie Checkbox-Group, aber ohne Checkbox |
| `Composite-Input` | Inline Row | Mehrere Inputs nebeneinander, nur in Shell |

---

## Tracking-Tabelle

Was ist in FieldDraft aktuell als Dialog implementiert gegenüber den YAML-Specs?

| Tool | Schema im Kernel | Dialog im Frontend | Vollständig laut YAML |
|---|---|---|---|
| Sketch | ✅ | 🚧 plane-selection als Text | ❌ (Constraint-Manager, Diagnostics fehlen) |
| Extrude | ✅ | 🚧 Mode-Row, Profiles-Picker | ❌ (Surface/Thin Tabs, Direction/Draft/Second-End fehlen) |
| Revolve | ✅ | ⏳ ohne Dialog | ❌ |
| Sweep | ✅ (K13.2) | ⏳ | ❌ |
| Loft | ⏳ | ⏳ | ❌ |
| Thicken | ⏳ | ⏳ | ❌ |
| Helix/Thread | ✅ (thread-primitive) | 🚧 flache Felder | ❌ (Tabs fehlen) |
| Enclose | ⏳ | ⏳ | ❌ |
| Fillet | ✅ | 🚧 nur Radius | ❌ (Tabs, Measurement, Variable-Fillet fehlen) |
| Chamfer | ✅ | 🚧 nur Distance | ❌ (Chamfer-Type-Dropdown fehlt) |
| Draft | ⏳ | ⏳ | ❌ |
| Shell | ⏳ | ⏳ | ❌ |
| Linear Pattern | ✅ | 🚧 | ❌ (Second-Direction, Skip-Instances fehlen) |
| Circular Pattern | ✅ | 🚧 | ❌ |
| Mirror | ✅ | 🚧 | ❌ |
| Transform | ⏳ (als Feature) | ⏳ | ❌ |
| Boolean | ⏳ | ⏳ | ❌ |
| Split | ⏳ | ⏳ | ❌ |
| Plane | 🚧 (nur Origin-Planes) | ⏳ | ❌ |
| Mate Connector | 🚧 (addMateConnectorAtPick) | ⏳ | ❌ |
| Hole | ✅ | 🚧 nur diameter/depth | ❌ (Hole-Style, Termination-Dropdowns fehlen) |
| Primitives | ✅ | 🚧 (hidden-Filter K7.22/9) | Merge-Scope folgt |

**Priorisierung für K7.21g (YAML-driven Dialog-Refactor):** Sketch → Extrude → Fillet → Hole → Revolve → Helix/Thread → Pattern/Mirror → Transform/Boolean (das sind die meistgenutzten Tools). Loft/Sweep/Shell/Draft/Split folgen in späteren Slices, wenn die jeweiligen Kernel-Features existieren.
