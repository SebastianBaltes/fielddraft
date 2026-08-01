# Hole-Feature nach Onshape-Vorbild — Plan (#404)

> SSOT für den Umbau des Hole-Features. Reine Recherche/Planung — kein Code committed.
> Bindende Konvention: physische Maße als `::fielddraft_protocol::Length`, Winkel als
> `::fielddraft_protocol::Angle`, **nie** nacktes `f32` (ein f32/Length-Mismatch war die
> Ursache von #405). Jedes Web-Zahlenfeld über `NumberInput.tsx`. Remove ohne Scope → Warnung.

## 1. Warum überhaupt

Ticket #404 (User, bindend): „Das Hole Feature ist falsch implementiert. … Es benötigt als
Ziel einen Mate Connector, oder einen Punkt auf einem Sketch, um ein Loch eines Standard-
(z.B. M6) und individuellen Maßes zu erzeugen. Das ist ein umfangreiches Feature, bitte
VOLLSTÄNDIG entsprechend der Onshape Vorlage umsetzen."

**Ist-Zustand** (`crates/kernel/src/features/hole.rs`): Das heutige `HoleDef` ist ein
verkappter Subtract-Zylinder mit genau vier Feldern — `diameter: Length`, `depth: Length`,
`translate: [f32;3]` und `targets: MultiQuery` (Scope). `execute` senkt es auf einen einzigen
`Primitive::Cylinder` mit `BoolOp::Remove` unter dem Kind-Id `<hole>/0` ab
(`hole.rs:88-104`). Die wasm-API `add_hole(diameter_mm, depth_mm, tx, ty, tz)`
(`crates/kernel/src/api/features.rs:463`) und `store.addHole(diameterMm=6, depthMm=40)`
(`web/src/store.ts:657`) nehmen **eine Weltkoordinate** entgegen — keine Zielselektion, kein
Loch-Typ, keine Norm, keine Endbedingung, keine Bohrspitze. Der Toolbar-Knopf
(`web/src/Toolbar.tsx:1293`) legt stumpf ein Ø6×40-Loch im Ursprung an.

Gegenüber der Onshape-Vorlage fehlt praktisch alles außer dem nackten Zylinder-Subtrakt.
Der Umbau ist ein Breaking Change am Feature-Schema — laut `CLAUDE.md` erwünscht.

## 2. Onshape-Referenzverhalten (aus der Doku)

Quelle: Onshape Help, „Hole" (https://cad.onshape.com/help/Content/hole.htm). Der Dialog ist
in Blöcke gegliedert; Reihenfolge und Gruppierung unten spiegeln den Onshape-Dialog.

### 2.1 Dialog-Gruppierung (Reihenfolge wie in Onshape)

1. **Measurement system** — `Inch` | `Metric` (steuert, welche Normreihen die Dropdowns zeigen).
2. **Hole style** (Geometrie-Grundform, gegenseitig ausschließend):
   `Simple` · `Counterbore` · `Countersink`.
3. **Hole type** (Bohrungs-Zweck, eigenes Dropdown):
   `Drilled` · `Clearance` · `Tapped` · `PEM®`.
4. **Locations** — Punkt-/Mate-Connector-Selektion (Mehrfachauswahl, Box-Select).
5. **Merge scope** — welche Parts geschnitten werden.
6. **Start** — `Start from part` | `Start from sketch plane` | `Start from selected plane` (mit Entity/Mate-Connector-Feld).
7. **Termination** (Endbedingung) — `Blind` | `Up to next` | `Up to entity` | `Through all`.
8. **Tip angle** — `118°` | `135°` | `Flat` | `Custom`.
9. **Tolerances** (ausklappbar, pro Maß) + **Name** (Auto/manuell).

### 2.2 Parameter-Katalog

| Parameter | Typ | Gilt für | Bemerkung |
|---|---|---|---|
| `measurementSystem` | Enum `inch\|metric` | alle | Filtert die Größen-Dropdowns |
| `style` | Enum `simple\|counterbore\|countersink` | alle | Geometrie-Grundform |
| `type` | Enum `drilled\|clearance\|tapped\|pem` | alle | Zweck; schaltet Größen-/Fit-Felder |
| `locations` | Query-Liste (Punkte/MC) | alle | Sketch-Punkte, Vertices, Kreiszentren, Spline-/Linien-Enden, Mate Connectors; Box-Select |
| `mergeScope` | Query-Liste (Bodies) | alle | Welche Parts werden gebohrt |
| **Größe/Durchmesser** | | | |
| `drillSize` | Enum (Norm-Dropdown) | drilled | Bohrer-Nenngröße |
| `standardSize` | Enum (M2…M64) | clearance/tapped | z.B. `M6` |
| `fit` | Enum `close\|normal\|loose` (+ASME-Varianten) | clearance | Durchgangsloch-Passung → ISO-273-Wert |
| `diameter` | **Length** | drilled/simple | Direktes Maß, override der Norm |
| `tapType` | Enum `straight\|straight_pipe\|tapered_pipe` | tapped | metrisch: `straight` |
| `threadClass` | Enum + Toggle | tapped | Gewindeklasse (kosmetisch) |
| `tappedDepth` | **Length** | tapped/blind | Gewindetiefe |
| `tapClearance` | **Length** | tapped/blind | Sackloch-Auslauf unter dem Gewinde |
| **Counterbore** | | | |
| `cboreDiameter` | **Length** | counterbore | Senk-Ø |
| `cboreDepth` | **Length** | counterbore | Senktiefe |
| **Countersink** | | | |
| `csinkDiameter` | **Length** | countersink | Senk-Ø (oben) |
| `csinkAngle` | **Angle** | countersink | Kegelwinkel (typ. 82°/90°) |
| **Endbedingung** | | | |
| `endType` | Enum `blind\|up_to_next\|up_to_entity\|through_all` | alle | |
| `depth` | **Length** | blind | Bohrtiefe |
| `upToEntity` | Query | up_to_entity | Ziel-Face/MC |
| `offset` | **Length** | up_to_next/entity | „Offset from tip" (optional) |
| `oppositeDirection` | bool (`opposite_direction`-UI) | alle | Bohrrichtung umdrehen |
| **Bohrspitze** | | | |
| `tipAngle` | Enum `118\|135\|flat\|custom` | drilled/blind | Kegel am Bohrgrund |
| `tipAngleCustom` | **Angle** | tip=custom | |
| `depthToTip` | bool | blind | Tiefe bis Spitze vs. bis Schulter (voll-Ø) |
| `name` | String (optional) | alle | Auto-generiert aus den Settings |

### 2.3 Semantische Kernpunkte

- **Ein Feature, viele Löcher:** Alle in `locations` selektierten Punkte erzeugen im *einen*
  Hole-Feature identische Löcher gleichzeitig (jeder Punkt = eine Bohrung, gleiche Parameter).
- **Zielselektion = Punkt + Achse:** Ein Sketch-Punkt liefert Position + die Sketch-Ebenen-
  Normale als Bohrachse; ein Mate Connector liefert Ursprung + dessen +Z. Onshape bohrt entlang
  dieser Achse in die durch `oppositeDirection` gewählte Richtung.
- **Tapped ist kosmetisch:** Onshape schneidet **kein** echtes Wendel-Gewinde. Die reale
  Geometrie eines Tapped-Holes ist der **Kernloch-Zylinder** (tap drill = Nenn-Ø − Steigung,
  ~75 % Tragtiefe); das Gewinde ist Annotation/Callout, kein Volumen.
- **Bohrspitze:** Ein realer Bohrer hinterlässt am Blind-Grund einen Kegel (118°/135°).
  `depthToTip` entscheidet, ob `depth` bis zur Kegelspitze oder bis zur letzten
  voll-durchmessrigen Stelle (Schulter) misst. `Flat` = Flachboden (kein Kegel).
- **Through all:** Loch geht durch alle Parts im Scope — geometrisch ein Zylinder, dessen Länge
  die AABB des Ziel-Bodys entlang der Bohrachse sicher überspannt.

## 3. Gap-Analyse Ist vs. Onshape

| Aspekt | Ist (heute) | Onshape-Soll | Δ |
|---|---|---|---|
| Loch-Grundform | nur Zylinder | simple/cbore/csink | **fehlt** cbore/csink |
| Loch-Typ | — | drilled/clearance/tapped/pem | **fehlt** komplett |
| Norm-Größen (M6…) | — | ISO-Dropdown → Maß-Tabelle | **fehlt** (keine Normtabellen im Repo) |
| Zielselektion | Weltkoordinate `translate` | Sketch-Punkte + Mate Connectors | **falsch** — Kern des Tickets |
| Bohrachse | fix lokal Y | aus Punkt/MC-Normale | **fehlt** |
| Mehrfach-Löcher | 1 pro Feature | N Punkte → N Löcher | **fehlt** |
| Endbedingung | nur feste Tiefe | blind/up-to/through-all | **fehlt** |
| Bohrspitze | flach | 118°/135°/flat/custom | **fehlt** |
| Start-Ebene | implizit | 3 Modi | **fehlt** |
| Scope-Warnung | vorhanden (`merge_scope_warning`) | ✓ | ok — behalten |

`translate: [f32;3]` verletzt außerdem die Length-Konvention und entfällt zugunsten der
Punkt-Auflösung.

## 4. Ziel-Design

### 4.1 Kernel-Datenmodell (`HoleDef`-Schema, `#[derive(FeatureSchema)]`)

Der Schema-Derive (`crates/features-macros/src/lib.rs`) unterstützt bereits alle nötigen
Typen: `f32`, `[f32;3]`, `bool`, `String`, `BoolOp`, `Length`, `Angle`, `Query`, `MultiQuery`
und **beliebige `Serialize+Deserialize`-Enums** über das generische `"enum"`-Vokabular
(`variants_json`), mit `ui_hint = "horizontal_enum" | "opposite_direction" | "slider"`. Damit
sind Loch-Typ-Dropdowns, Segmented-Tabs und der ±-Flip ohne Derive-Erweiterung darstellbar.

Neue Enums (kernel-seitig, `#[derive(Serialize, Deserialize)]`):
`HoleStyle {Simple, Counterbore, Countersink}`, `HoleType {Drilled, Clearance, Tapped}`
(PEM zunächst weggelassen), `HoleEndType {Blind, UpToNext, UpToEntity, ThroughAll}`,
`TipAngle {A118, A135, Flat, Custom}`, `ClearanceFit {Close, Normal, Loose}`,
`StartMode {Part, SketchPlane, SelectedPlane}`, `MetricSize`-Enum (M2…M64) **oder** ein
String-`standardSize` mit Tabellen-Lookup (Enum bevorzugt: der Dialog rendert es als Dropdown).

`HoleDef` (Kernfelder; alle Maße `Length`, Winkel `Angle`):

    style: HoleStyle
    hole_type: HoleType
    standard_size: MetricSize            // Norm-Dropdown, treibt Kern-/Clearance-Ø
    fit: ClearanceFit                    // nur clearance
    diameter_override: Option<Length>    // custom-Maß überschreibt Norm
    // Counterbore
    cbore_diameter: Length
    cbore_depth: Length
    // Countersink
    csink_diameter: Length
    csink_angle: Angle
    // Tapped
    tapped_depth: Length
    tap_clearance: Length
    // Endbedingung
    end_type: HoleEndType
    depth: Length
    up_to_entity: Query                  // entity_filter FACE
    end_offset: Length
    opposite_direction: bool             // ui_hint opposite_direction
    // Bohrspitze
    tip: TipAngle
    tip_custom: Angle
    depth_to_tip: bool
    // Ziel + Scope
    start_mode: StartMode
    start_plane: Query                   // nur SelectedPlane
    locations: MultiQuery                // entity_filter VERTEX (Punkte/MC) — MEHRERE
    targets: MultiQuery                  // Scope, entity_filter BODY (behalten)

`translate` **entfällt** (durch `locations` ersetzt).

### 4.2 Normtabellen (ISO, Rust-const)

Neue Datei `crates/kernel/src/features/hole_iso.rs` mit const-Tabellen. Quellen (bindend im
Doc-Kommentar zu zitieren): **ISO 261** (metrische Regelgewinde-Steigungen), **ISO 273 /
ISO 20273** (Durchgangslöcher close/medium/coarse), Kernloch = Nenn-Ø − Steigung (75 %
Tragtiefe, bestätigt).

Datenmodell:

    struct MetricThread { nominal_mm: f32, coarse_pitch_mm: f32, tap_drill_mm: f32,
                          clear_close_mm: f32, clear_normal_mm: f32, clear_loose_mm: f32 }
    const ISO_METRIC: &[MetricThread] = &[ … ];

Richtwerte (aus ISO 261/273; **bei Implementierung 1:1 aus der Norm zu transkribieren, nicht
erfinden** — hier nur zur Größenordnung): M3 p0.5 tap2.5 clear 3.2/3.4/3.6 · M4 p0.7 tap3.3
4.3/4.5/4.8 · M5 p0.8 tap4.2 5.3/5.5/5.8 · M6 p1.0 tap5.0 6.4/6.6/7.0 · M8 p1.25 tap6.8
8.4/9.0/10.0 · M10 p1.5 tap8.5 10.5/11.0/12.0 · M12 p1.75 tap10.2 13.0/13.5/14.5 · M16 p2.0
tap14.0 17.0/17.5/18.5 · M20 p2.5 tap17.5 21.0/22.0/24.0 · M24 p3.0 tap21.0 25.0/26.0/28.0.
Ein Kernel-Unit-Test prüft die Formel `tap_drill ≈ nominal − pitch` je Zeile (Transkriptions-
Guard). Die effektive Bohr-Ø-Ableitung: `drilled/simple`→`diameter_override.or(nominal)`,
`clearance`→`clear_*[fit]`, `tapped`→`tap_drill`.

### 4.3 Geometrie-Erzeugung (CSG)

Pro Location wird ein **Cutter-Solid** gebaut und mit `BoolOp::Remove` gegen `targets`
verrechnet (wie heute, aber Stapel statt Einzelzylinder). Konvention aus dem Repo (Fillet/Loft):
dedizierte CSG-Knoten bevorzugen (`CsgNode::Extrude`/Revolve-Profil) statt generischer
Leaf-Verpackung; jede Location bekommt eine eigene Kind-Id `<hole>/<i>` (heutiges `<hole>/0`-
Muster, `hole.rs:80`), damit `sync_feature_calls` sie beim nächsten Sync verwirft und die
`hole`-FeatureCall die SSOT bleibt.

Cutter-Aufbau je Stil (alles entlang der lokalen Bohrachse +Z_local des Ziel-Frames):

- **Simple/Drilled:** Schaft-Zylinder `Ø = drill_dia`, Länge = `effektive_tiefe`. Bei `blind`
  + Kegelspitze (118°/135°): unten ein Kegel (Revolve-Profil oder Cone-Primitive) mit
  Halbwinkel `90°−tip/2`. `depth_to_tip` verschiebt den Zylinder/Kegel-Übergang.
- **Counterbore:** Schaft + koaxialer weiterer Zylinder oben `Ø=cbore_dia`, Tiefe `cbore_depth`
  (Union der beiden Cutter, dann Remove).
- **Countersink:** Schaft + oben ein Kegelstumpf (Revolve-Profil) `Ø=csink_dia`, Winkel
  `csink_angle`.
- **Clearance/Tapped:** wie Simple, nur andere Ø-Ableitung (s. 4.2). Gewinde bleibt kosmetisch
  (keine Wendel).
- **Through all:** Schaftlänge = AABB-Spanne des Bodys entlang der Bohrachse + Sicherheits-ε,
  in beide Richtungen (Strahl durch AABB) — analog zum bestehenden `resolve`-AABB-Muster.

Der Cutter wird durch den Ziel-Frame (Origin+Basis) als `Transform` platziert — die
Bohrachse ist immer +Z_local, `opposite_direction` negiert sie.

### 4.4 Zielauflösung (Query → Frame)

Die Auflösung existiert bereits und wird wiederverwendet:
`Context::resolve_mate_anchor` (`crates/kernel/src/features/context_resolve.rs:899`) liefert für
`QueryExpr::Vertex`/`Edge`/`EdgeBetween` eine `MateAnchorFrame { origin, primary, auto_secondary }`.
- **Mate Connector / Vertex:** `origin` = Punkt, Bohrachse = `primary` bzw. bei Vertex die
  `auto_secondary`-Face-Normale (Onshape: „normal to the face"). MC werden schon heute via
  `qFace/qEdge/qVertex`-Anchor angelegt (`store.addMateConnectorAt`, `store.ts:989`).
- **Sketch-Punkt:** *Offene Frage — s. §7.* Sketch-Punkte leben im geteilten Entity-ID-Raum
  (`web/src/sketchGrips.ts` `entityIdForGrip`). Zu klären: ob ein Sketch-Punkt als B-Rep-Vertex
  auftaucht (dann `qVertex(id)` + `resolve_mate_anchor` direkt) oder ob eine dedizierte
  `qSketchPoint(sketch, entity)`-Query + Auflösung (Position im Sketch-Frame, Normale =
  Sketch-Ebenen-Normale via `face_plane_frame`/`SketchFrame`) nötig ist.
- **Face-Ziel/Start-Ebene/up_to_entity:** `Context::resolve_face_frame`
  (`context_resolve.rs:44`) liefert die `SketchFrame`.

`locations` als `MultiQuery` → klammer-bewusster Top-Level-Split (bestehendes
`MultiQuery`-Muster) → je Ausdruck ein Frame → je Frame ein Cutter.

### 4.5 Scope & Warnung

Unverändert das Muster aus heute/`thicken.rs`: `target_ids_from_queries(&d.targets)` +
`ctx.merge_scope_warning(BoolOp::Remove, &ids)` → `ctx.warn_feature`. Leerer Scope = Warnung,
bohrt nichts (Onshape-Semantik: Scope immer explizit).

## 5. UI-Plan

- **Toolbar-Flow wie #401:** Klick auf den Hole-Knopf öffnet **sofort** den `FeatureDialog`
  (schema-getrieben), Cancel verwirft das Pending-Feature. Kein blindes Anlegen mehr
  (`Toolbar.tsx:1293` wird von `store.addHole()` auf „Dialog öffnen" umgestellt).
- **Dialog schema-getrieben:** Der `FeatureDialog` (`web/src/widgets/FeatureDialog.tsx`)
  rendert bereits `QueryWidget`/`MultiQueryWidget` (`:574`) und Enum-Segmented-Tabs. Gruppierung
  nach §2.1: Stil/Typ als `horizontal_enum`-Tabs oben, dann `locations`-Picker
  (`entity_filter="VERTEX"`, Mehrfach), Größen-Dropdown, konditionale Felder je Typ, Termination,
  Tip-Angle, Scope. Konditionale Sichtbarkeit (z.B. `cbore_*` nur bei Counterbore) — s. §7, ob
  das Schema `visible_if` schon kennt oder ergänzt werden muss.
- **Picker:** Punkt-/MC-Selektion über den bestehenden Query-Picker (One-Shot,
  `deactivateOnPick`); Mehrfachauswahl sammelt in die `MultiQuery`.
- **Zahlenfelder:** ausnahmslos `NumberInput.tsx` (Ø, Tiefen, Winkel als Length/Angle mit Einheit).
- **wasm-API:** `add_hole` wird von der flachen `(dia,depth,tx,ty,tz)`-Signatur
  (`api/features.rs:463`) auf einen **JSON-Def-Push** umgestellt (wie andere schema-getriebene
  Features), damit Web nur noch die Schema-Defaults schickt und der Dialog den Rest treibt.

## 6. Phasenplan

Jede Phase einzeln committbar + testbar. **Gate-Definition (bindend, Lehre aus #402/#405):**
Kernel-Tests laufen über die **echte API-Kette** `add_feature_call` → `body_sdf_eval` /
`body_brep_faces`, **nicht** nur auf Resolver-Ebene. Length-Felder werden mit `{value,unit}`
gepusht, nie als f32.

- **P1 — Kleinster nutzbarer Kern (Simple/Drilled auf Sketch-Punkten, Blind + Through all).**
  Neues `HoleDef`-Schema mit `style=Simple`, `standard_size` (metrisch), `diameter_override`,
  `end_type ∈ {Blind, ThroughAll}`, `locations` (Vertex/MC-Query, **mehrere**), `targets`.
  Normtabelle `hole_iso.rs` (Nenn-Ø + Kernloch/Clearance-Grundgerüst). Zielauflösung über
  `resolve_mate_anchor`. Mehrfach-Löcher (N Punkte → N Cutter). *Gate:* `add_feature_call("hole",
  …)` auf einen Box-Body mit 2 gepickten Vertices → `body_brep_faces` zeigt 2 zylindrische
  Bohrungen; Through-all durchbohrt; leerer Scope → Warnung. Toolbar öffnet Dialog (#401-Flow).
- **P2 — Loch-Typen & Normen (Drilled/Clearance/Tapped + ISO-Größen-Dropdown).**
  Voll ausgebaute `hole_iso.rs` (M2…M24 mind., Transkriptions-Guard-Test), `fit`-Passungen,
  Ø-Ableitung je Typ, kosmetisches Tapped (Kernloch-Geometrie). *Gate:* M6-Clearance-normal
  liefert Ø6.6; M6-Tapped liefert Ø5.0; Test über die API-Kette.
- **P3 — Counterbore & Countersink (Stil-Stapel).** Cutter-Stapel (Union) mit `CsgNode`-
  Knoten/Revolve-Profil. *Gate:* `body_brep_faces` zeigt die Senk-Stufe/den Kegel; Maße stimmen.
- **P4 — Bohrspitze & Tiefen-Semantik (Tip 118/135/flat/custom, depth_to_tip).**
  Kegel am Blind-Grund; `depth_to_tip`-Umschaltung. *Gate:* Blind-Tiefe bis Spitze vs. Schulter
  geometrisch verifiziert.
- **P5 — Endbedingungen up_to_next / up_to_entity & Start-Modi.** Face-Frame-Auflösung
  (`resolve_face_frame`), `end_offset`, `opposite_direction`, `start_mode`. *Gate:* Loch endet
  an der gewählten Face + Offset.
- **P6 — UI-Feinschliff:** konditionale Feld-Sichtbarkeit, Auto-Name, Sketch-Punkt-Picker-UX,
  Box-Select. *Gate:* Dialog zeigt nur relevante Felder je Typ/Stil.

P1 ist die minimal nutzbare Einheit, die das Kern-Ticket-Anliegen (Punkt-Ziel statt
Weltkoordinate, Norm-Größe M6, mehrere Löcher) bereits erfüllt.

### P1 — Umgesetzt (Stand dieser Iteration)

**Entscheidung Sketch-Punkt-Referenzierung (Frage §7.1, empirisch geklärt):** Ein freier
Sketch-Punkt taucht **nicht** als B-Rep-Vertex auf — nur Solid-Ränder (Extrude/Revolve)
erzeugen Vertices, ein loser Punkt trägt nichts zum gewebten Körper bei. `qVertex` kann ihn
also gar nicht adressieren. Stattdessen: **neue Query `qSketchPoint(sketch_id, entity_id)`**,
analog zum bereits ausgelieferten `qSketchLine`. Aufgelöst in
`Context::resolve_mate_anchor` gegen `Context::sketches[sketch_id].state.entities` (Punkt →
`SketchFrame`-Lift → Origin; Bohrachse = Sketch-Ebenen-Normale). Warum stabil unter
Regeneration/Reordering: `sketch_id` ist die Feature-eigene ID (nicht positionsbasiert), und
`entity_id` ist der Solver-`EntityId` aus dem **geteilten Kernel↔Web-Entity-ID-Raum** — der
Kernel (`SketchState::with_origin`, Origin=0, Punkte ab 1) und das Web
(`buildDraftSketchState`/`buildGripIndex`, id ab 1) allozieren deterministisch identisch aus
der Sub-Call-Reihenfolge. (Fragil nur gegenüber Umsortieren INNERHALB des Sketch — dieselbe
Eigenschaft, die das gesamte Sketch-Grip-System schon hat; nicht schlechter.)

Mate Connectors werden über `qCreatedBy(<mc_feature_id>)` referenziert: `resolve_mate_anchor`
prüft zuerst `ctx.mate_connectors` (Datum-Features, kein B-Rep) und liefert deren CSYS
(Origin + +Z) als Bohr-Frame, sonst Fallback auf die Face-Anchor-Auflösung.

**Geliefert:** `HoleDef` mit String-Enums (`style`/`hole_type`/`standard_size`/`end_type`,
je `variants=`-Liste, so erweitern P2/P3 ohne Wire-Bruch), Maße als `Length`; N Locations →
N Remove-Zylinder-Cutter unter Kind-IDs `<hole>/<i>`; `blind` (exakte Tiefe, kleine
Mündungs-ε außerhalb des Materials) + `through_all` (Cutter spannt die Body-Vertex-AABB
entlang der Bohrachse). Bohrrichtung standardmäßig in den Körper (−Normale), `opposite_direction`
kippt. ISO-Tabelle `hole_iso.rs` (M2–M12, Transkriptions-Guard `tap≈nenn−steigung`).
wasm-`add_hole(standard_size, diameter_mm, end_type, depth_mm, locations, targets)` schreibt
die Length-Wire-Form. Web: `store.addHole()` öffnet den schema-getriebenen Dialog sofort
(#401-Flow); `locations`-Picker (`entity_filter="VERTEX,SKETCHPOINT"`) via Viewport-Raycast
auf publizierte Sketch-Punkt-Pickables → `qSketchPoint(...)`.

**Bewusst offen (P2+):** Counterbore/Countersink, Clearance/Tapped + Fit, Bohrspitze,
up_to_next/up_to_entity, Start-Modi, konditionale Feld-Sichtbarkeit, Sketch-Punkt-Hover-
Highlight + Box-Select. `diameter_override` ist in P1 als `diameter` (aktiv nur bei
`standard_size=custom`) umgesetzt; die getrennte Override-über-Norm-Semantik kommt mit P2.

### P2 — Umgesetzt (v0.52.0)

**Normtabelle.** `hole_iso.rs` von 9 auf **17 Zeilen** erweitert: M1.6 · M2 · M2.5 · M3 · M3.5 ·
M4 · M5 · M6 · M8 · M10 · M12 · M14 · M16 · M18 · M20 · M22 · M24. Spalten wie geplant
(Nenn-Ø, ISO-261-Regelsteigung, Kernloch, ISO-273 close/normal/loose). Die Werte wurden
gegen drei unabhängige Quellen abgeglichen, weil sie sich widersprachen: eine populäre
Tabelle (Fractory) weicht in mehreren Zeilen von ISO 273 ab (M6 normal 6.5 statt 6.6,
M12 13.5/14.5 → 14.0/15.0, M24 25/26/28 → 26/28/30). Übernommen wurde die
ISO-273-/ASME-B18.2.8-Reihe (AmesWeb + engineeringhardware, beide deckungsgleich mit den
bereits in P1 transkribierten M2…M12-Zeilen — die blieben also unverändert). M18 = 19/20/21
und M22 = 23/24/26 sind die neu belegten Zeilen.

**Guard-Tests** (`hole_iso::tests`, 6 statt 3): der bestehende `tap ≈ nenn − steigung`
(±0.1 mm) plus neu — Zeilen streng aufsteigend **und** Key == Nenn-Ø (Copy-Paste-Schutz),
`tap < nenn < clearance_close` (die Invariante, die die drei Typen überhaupt unterscheidbar
macht), und ein Fit-Spalten-Lookup inkl. Fallback.

**Ø-Ableitung.** `effective_diameter_mm` dispatcht jetzt auf `hole_type`:
`drilled`→Nenn-Ø, `clearance`→`clearance_mm(fit)`, `tapped`→Kernloch. `standard_size=custom`
(oder ein unbekannter Key aus einem alten Dokument) nimmt weiterhin die freie `diameter` —
und zwar **vor** dem Typ-Dispatch, damit ein Custom-Maß nie von einer Norm überschrieben wird.

**Schema.** Neues Feld `fit: String` (`close|normal|loose`, Default `normal`), direkt nach
`hole_type`. `standard_size`-Variantenliste auf 17+custom erweitert. Beides sind additive
`variants=`-Erweiterungen — kein Wire-Bruch. Der Dialog ist schema-getrieben: beide Dropdowns
erscheinen automatisch, ohne Web-Änderung. Die wasm-`addHole`-Signatur bekam
`hole_type` + `fit` vorangestellt (`store.addHole()` ruft weiterhin argumentlos mit Defaults).

**Gate gemessen** (`hole_type_and_fit_select_the_iso_bore_diameter`, über
`add_feature_call("hole", …)` → `body_sdf_eval`): Der Bohrungsradius wird per Bisektion des
kompilierten SDF-Tapes quer zur Achse bestimmt (48 Schritte, ~1e-5 mm) — gemessen wird also
die Geometrie, die der Nutzer bekommt, nicht die Absicht des Resolvers. Ergebnis, alle
innerhalb 2e-3 mm: M6 drilled **Ø6.000**, clearance close/normal/loose **Ø6.400 / 6.600 /
7.000**, tapped **Ø5.000**; M3 tapped Ø2.500, M12 clearance-loose Ø14.500, M16 drilled Ø16.000;
`custom` liefert 7.7 bzw. 3.3 unabhängig von Typ und Passung. Zusätzlich
`hole_clearance_bore_is_a_cylinder_face_of_the_iso_radius`: `body_brep_faces` webt für das
M6-Clearance-Loch eine `surfaceKind = "cylinder"`-Fläche mit `radius = 3.3` — die Bohrung ist
ein echter Zylinder, nicht nur ein Loch im Feld.

### P3 — Umgesetzt (v0.53.0)

**Kein neuer Opcode — sondiert, nicht vermutet.** Vor dem ersten Strich Code wurde geprüft, was
an Kegel-Primitiven existiert: `Primitive::Cone { r_bottom, r_top, half_height }` (Kegelstumpf
um die lokale +Y-Achse, `r_top = 0` = spitzer Kegel) ist **schon in allen Evaluatoren** —
CPU-Tape (`sdf_eval.rs`), BVH/Fold (`csg.rs`, `offset_fold.rs`), Dual (`sdf_dual.rs`), Fidget
(`fidget_bridge.rs`), TSL (`web/src/sdfRenderer.ts::sdfConeFn` + `sdfVmInterpreter.ts`),
STEP/OCCT (`step_commands.rs`, `occt.worker.ts`) — und über `primitive_halfspaces` sogar als
analytische `HalfSpace::Cone`-Fläche webbar. Der Stil-Stapel braucht damit **null**
Evaluator-Arbeit; die P1-Lehre des Fillet-Cutters ist eingehalten. Kein Golden-Lauf nötig
(rein additive Feature-Arbeit, kein Evaluator angefasst).

**Geometrie.** Pro Location wird ein Stapel koaxialer Remove-Cutter erzeugt (Entfernen einer
Vereinigung == Vereinigung der Entfernungen). Alle Teile laufen durch eine gemeinsame
Platzierungsfunktion `axial_span(origin, dir, t_lo, t_hi)` mit `t = 0` an der Ankerfläche,
positiv ins Material:

- **Schaft** wie bisher, Kind-Id `<hole>/<i>`.
- **Counterbore** `<hole>/<i>c`: Zylinder `Ø = cbore_diameter` von `-ε` bis `cbore_depth`.
  Der Radius wird auf `≥` Schaftradius geklemmt (ein schmalerer „Senk"-Zylinder fräst nichts).
- **Countersink** `<hole>/<i>s`: `Primitive::Cone` von `-ε` bis `csink_depth`, mit
  `csink_depth = (R_senk − R_bohrung) / tan(Winkel/2)`. Der Mündungs-Überstand verlängert den
  Kegel **entlang seiner eigenen Wand** (`r_bottom = R_senk + ε·tan(Winkel/2)`) statt ihn flach
  zu kappen — nur so bleibt der Kreis auf der Fläche exakt der eingetragene Durchmesser.
  `csink_angle` ist wie in Onshape der **volle Öffnungswinkel**, nie der halbe.

**Neue Felder** (alle `Length`, der Winkel `Angle`): `cbore_diameter` (11 mm),
`cbore_depth` (6 mm), `csink_diameter` (12 mm), `csink_angle` (90°). Bewusst **keine**
Norm-Auto-Ableitung aus `standard_size`: DIN 974-1 (Senkungen für Zylinderschrauben) und
ISO 15065 (Senkungen für Senkschrauben) sind eigene Tabellen, und geraten wird hier nichts —
das Auto-Ausfüllen gehört zu P6/UI. Entartungen werden mit **benanntem** Parameterfehler
abgelehnt: Senk-Ø ≤ Bohrungs-Ø, Winkel außerhalb (0°, 180°), nicht-positive Maße.

**Gate gemessen.** Counterbore Ø12 × 4 mm über einer blinden M6-Ø6 × 12 mm-Bohrung, gebohrt
von der Oberseite eines Blocks (Skizzenebene == Deckfläche, damit die Mündung auf einer echten
Freifläche sitzt): SDF-Bisektion liefert **Ø12.000** im Sitz (2 mm tief), **Ø6.000** im Schaft
(6 mm tief), die Stufe bei **4.000 mm** und den blinden Grund bei **12.000 mm** (alle < 2e-3 mm
Abweichung). `body_brep_faces` webt den Sitz als analytische `cylinder`-Fläche mit `radius 6.0`.
Countersink Ø12 über derselben Bohrung, für **90°, 82° und 120°**: der Radius folgt in allen
drei Fällen exakt `6 − t·tan(Winkel/2)` (geprüft bei t = 0.25 / 0.5 / 1.0 mm), unterhalb der
Kegeltiefe steht wieder der Ø6-Schaft, und `body_brep_faces` meldet eine `cone`-Fläche mit
`halfAngle` = 0.7854 / 0.7156 / 1.0472 rad (= 45° / 41° / 60°, also genau der halbe
Öffnungswinkel).

**Nebenbefund (pre-existing, NICHT von P3 verursacht) — Geschwister-Cutter teilen sich einen
Branch.** Beim Messen fiel auf, dass `body_brep_faces` vom Stapel nur die Flächen des
**zuletzt** gepushten Cutters führt: der Ø6-Schaft unterhalb des Sitzes taucht nicht als
eigene Fläche auf. Ursache ist `FeatureId::branch_identity`, das für alle
`FeatureId::child`ren eines Features dasselbe `(feature, instance)`-Paar liefert — dieselbe
Faltung, die auf `BranchId::CUTTER_AUX_ROUND` bereits dokumentiert ist (der Fillet-Cutter löst
sie über reservierte Sub-Bänder in `cutter_sub`). Gegenprobe, empirisch:

- **zwei Hole-Features** mit je einer Bohrung → 4 Bohrungs-Flächen (beide Bohrungen gewebt);
- **ein Hole-Feature** mit denselben zwei Locations → 2 Flächen (nur eine Bohrung gewebt).

Der Defekt existiert also schon seit P1 (Mehrfach-Löcher) und betrifft **nur** das analytische
Flächen-Set — das entfernte Volumen, das Mesh und das SDF sind in beiden Fällen identisch
(alle SDF-Proben oben bestätigen die volle Stapel-Geometrie). Praktisch heißt das: die zweite
Bohrung bzw. die Schaftwand unter einer Senkung lässt sich derzeit nicht anklicken/messen.
Gepinnt in `hole_sibling_cutters_share_one_branch`. Die Behebung ist eine Entscheidung am
BranchId-Schema (freies Sub-Band wie beim Fillet vs. Instanz-Ordinal `~i` — letzteres kollidiert
mit Pattern-Kopien, s. `branch_identity`-Doku) und gehört bewusst NICHT in eine Feature-Phase.

### P4 — Umgesetzt (v0.54.0)

**Geometrie.** Der Bohrgrund eines **blinden** Lochs bekommt einen `Primitive::Cone`
(`r_bottom = Bohrradius`, `r_top = 0`, also eine echte Spitze) als weiteren Remove-Cutter unter
`<hole>/<i>t` — wieder ohne neuen Opcode. Kegelhöhe `h = R / tan(Winkel/2)`; `tip` ist wie
`csink_angle` der **volle** Öffnungswinkel. Damit fallen genau die Zahlen der Bohrer-Tabellen:
118° → `h = 0.3005·Ø`, 135° → `h = 0.2071·Ø`. `through_all` bekommt **keine** Spitze (ein Loch,
das durchgeht, hat keinen Grund).

**Neue Felder:** `tip` (`a118` · `a135` · `flat` · `custom`, Default **a118** — Onshapes
Voreinstellung, also ein Verhaltenswechsel gegenüber P1–P3, wo der Grund flach war),
`tip_custom: Angle` (118°) und `depth_to_tip: bool` (false).

**Tiefen-Semantik.** `depth_to_tip = false` (Default, die Zeichnungs-Konvention): `depth`
erreicht die **Schulter**, die Spitze hängt darunter — das Loch ist am Ende `h` tiefer als die
eingetragene Zahl. `depth_to_tip = true`: `depth` erreicht die **Spitze**, der volle Durchmesser
endet `h` früher. Ist die Tiefe kürzer als die Spitze selbst, erreicht das Loch nie den vollen
Durchmesser — das wird mit einem benannten `depth`-Fehler abgelehnt statt still zu einem
Kegelstumpf zu werden.

**Gate gemessen** (SDF-Bisektion auf der Bohrachse, M6-Ø6, Tiefe 12 mm):
`flat` → Grund bei **12.000 mm**. `a118` → Spitze bei **13.803 mm** (= 12 + 3/tan 59°),
`a135` → **13.243**, `custom 100°` → **14.518**; bei 11.9 mm steht überall noch der volle
**Ø6.000**, auf halber Kegelhöhe **≈ Ø3**. `body_brep_faces` meldet jeweils eine `cone`-Fläche
mit `halfAngle` = 59° / 67.5° / 50°. Die Umschaltung selbst
(`hole_depth_to_tip_switches_which_end_the_depth_names`): mit derselben 12-mm-Eingabe und
demselben 118°-Anschliff liegt die Spitze einmal bei **13.803**, einmal bei **12.000** — die
Differenz ist auf 2e-3 mm genau **eine** Kegelhöhe; bei 11.9 mm ist die schulter-bezogene
Bohrung noch voll Ø6, die spitzen-bezogene schon verjüngt (Ø < 5).

**Folgeanpassungen:** `hole_without_scope_warns_and_drills_nothing` erwartet jetzt 3 statt 2
Körper (der ungescopte Cutter-Stapel ist Schaft **plus** Spitze); die P3-Gates setzen
`tip: "flat"`, damit sie genau eine Sache prüfen (und weil der zuletzt gepushte Cutter wegen des
Branch-Folds die `cone`-Fläche im Weave stellt — s. P3-Nebenbefund).

### P5 — Umgesetzt (v0.55.0)

**Ein gemeinsames Achsen-Koordinatensystem.** Alle vier Endbedingungen und alle drei
Start-Modi reduzieren sich auf zwei Zahlen pro Location: `t_start` (wo die Bohrung beginnt)
und `t_end` (wo sie aufhört), gemessen in mm entlang der Bohrrichtung ab dem Anker. Jedes
Cutter-Teil läuft weiterhin durch `axial_span`; neu ist nur, dass Schaft, Senkung und
Kegelsenkung ihren Nullpunkt bei `t_start` statt hart bei 0 haben. Bei
`start_mode = sketch_plane` (Default) ist `t_start = 0` — die P1–P4-Geometrie ist damit
unverändert, was die bestehenden Gates bestätigen.

**Neue Felder:** `start_mode` (`sketch_plane` · `part` · `selected_plane`, Default
`sketch_plane`), `start_plane: Query` (entity_filter FACE), `up_to_entity: Query`
(entity_filter FACE) und `end_offset: Length` (0 mm, **vorzeichenbehaftet**: positiv = tiefer
in Bohrrichtung, negativ = kurz vor der gefundenen Fläche). `end_type` hatte die Varianten
`up_to_next`/`up_to_entity` seit P1 in der Liste — bis hierher verhielten sie sich still wie
`blind`. Jetzt tun sie, was sie heißen.

**Ebenen-Auflösung — keine neue Maschinerie.** `Context::resolve_plane_frame` gab es schon
(Datum-Ebene → Mate Connector → B-Rep-Fläche, in genau dieser Reihenfolge und aus genau den in
#409 dokumentierten Gründen). Für eine Endbedingung ist die Fläche-Sprosse aber zu weich:
`face_plane_frame` beantwortet auch eine GEKRÜMMTE Fläche — mit deren Schwerpunkt und lokalem
+Z. Für einen Sketch ist das ein brauchbarer Notnagel, für „die Bohrung endet an DIESER Fläche"
ist es eine erfundene Ebene. Deshalb neu: `Context::resolve_planar_frame` — dieselbe Leiter,
aber die B-Rep-Sprosse zählt nur für `SurfaceGeometry::Analytic { HalfSpace::Plane }`. Die
gemeinsamen Datum-/Connector-Sprossen sind in `datum_or_connector_frame` herausgezogen, damit
beide Varianten dieselbe Reihenfolge benutzen statt sie zweimal zu behaupten.

**`up_to_next` und `start_mode = part` lesen das Feld, nicht die Absicht.** Beide brauchen eine
Materialgrenze, die in keinem Parameter steht. Gelesen wird sie auf dem lebenden Feld von allem
bisher Modellierten — `CsgNode::from_decoded(&ctx.features)`, dasselbe Muster, das der
Chamfer-Dieder-Test schon fährt — und zwar **vor** dem ersten gepushten Cutter, sodass eine
Bohrung niemals an sich selbst oder an ihrer Geschwister-Bohrung endet. Der Marsch
(`first_boundary`) ist ein Sphere-Trace: die Schrittweite ist der Feldwert selbst, kann also
per SDF-Eigenschaft keine Wand überspringen, mit einem Boden von 0.01 mm (sonst bliebe der
Lauf an einer Fläche stehen) und einem harten Budget von 40 000 Schritten gegen streifende
Achsen. Der gefundene Vorzeichenwechsel wird auf 1e-4 mm einbisektiert — deshalb landet
`up_to_next` **auf** der Fläche und nicht innerhalb eines Sampling-Rasters daneben.

**Bohrspitze nur noch bei `blind`.** Bisher galt „alles außer through_all bekommt eine Spitze".
Das ist für `up_to_next`/`up_to_entity` falsch: beide enden auf einer Fläche, die der Nutzer
benannt hat, und ein Kegel dort stößt genau durch sie hindurch. Onshape zeigt den Anschliff
ebenfalls nur bei Blind (§2.2). Ein unbekannter `end_type` aus einem neueren Dokument liest
sich als `blind` — die einzige Lesart, die nicht mehr wegschneiden kann als die eingetragene
Zahl.

**Bewusste Abweichung: `through_all` ignoriert den Start.** Der Durchgangs-Cutter spannt
weiterhin die volle Körper-Ausdehnung in BEIDE Richtungen ab dem Anker (P1-Verhalten). Der
Start-Modus verschiebt nur, wovon Tiefen gemessen werden — und `through_all` misst keine. Die
Senkungen wandern trotzdem mit `t_start` mit, denn die haben eine Tiefe.

**Gates gemessen** (alle über `add_feature_call("hole", …)` → `body_sdf_eval`, Bisektion auf
der Achse, Toleranz 2e-3 mm):

- *up_to_entity:* 20-mm-Block, von oben auf die per `query`-String gepickte **Bodenfläche**
  gebohrt. `end_offset = −5` → Grund bei **15.000 mm**, `−2` → **18.000 mm**, `0` → die Bohrung
  bricht durch (bei z = −19.99 offen). `depth` steht dabei durchgehend auf absurden 999 mm und
  wird nachweislich ignoriert. Der Grund ist bei Default-`tip = a118` **flach** — ein Kegel
  hätte ihn auf 16.803 gelegt.
- *up_to_next:* 5-mm-Platte (z ∈ [−5, 0]) über einem Block (z ∈ [−30, −20]), **beide im
  Scope**. Die Bohrung durchdringt die Platte und lässt den Block ganz; dieselbe Datei mit
  `through_all` bohrt den Block sehr wohl (Kontrollprobe, dass die Aussage die Endbedingung
  misst und nicht einen Scope-Zufall). Mit `end_offset = −1` bleibt exakt **4.000 mm** Platte
  über dem Bohrgrund stehen.
- *start_mode = part:* Block 10 mm UNTER der Skizzenebene (z ∈ [−30, −10]), blind 6 mm.
  `sketch_plane` schneidet nichts (der Cutter hängt in der Luft), `part` öffnet die Bohrung an
  der Deckfläche und legt den Grund auf **16.000 mm** unter der Skizzenebene (= 10 mm Abstand
  + 6 mm Tiefe).
- *start_mode = selected_plane:* Platte A (z ∈ [−10, 0]) über Block B (z ∈ [−30, −15]) mit
  5 mm Luft, **nur B im Scope**. Von A's Bodenfläche aus gestartet läuft die 12-mm-Bohrung bis
  **22.000 mm** und nimmt 7 mm aus B; von der Skizzenebene aus endet sie bei 12 mm in der Luft
  und B bleibt unberührt.
- *Fehlerfälle, benannt statt still:* leeres `up_to_entity`, eine zur Bohrachse **parallele**
  Ebene (`qCreatedBy(origin_plane_yz)` bzw. `_xz` als Startebene) und eine **gekrümmte** Fläche
  liefern je einen `InvalidParameter`-Fehler mit dem Feldnamen darin.

Testzahlen: Kernel 1508 → **1513** grün (5 neue), Web 2072 unverändert, `tsc -b` sauber.

### P6 — Umgesetzt (v0.56.0)

**Konditionale Sichtbarkeit: Derive-Attribut, weil die Web-Seite es längst konnte.** §7.3 fragte,
ob das Schema ein `visible_if` kennt. Sondiert statt vermutet — und die Antwort war überraschend:
`web/src/widgets/schema.ts` liest `visible_when: {param, equals}` **seit K6a.1**, `isVisible` ist
implementiert, und `FeatureDialog.tsx:298` filtert damit. Es gab nur keinen Produzenten: der
Derive konnte das Feld nicht emittieren, also kam es ausschließlich aus handgeschriebenen
Fixtures. Gewählt wurde deshalb **das additive Derive-Attribut** (nicht Web-seitiges Ausblenden
per Typ-Enum): Ein `if (featureType === "hole" && key === "cbore_depth")` im Web wäre eine zweite
Wahrheit über das Schema, die bei jedem neuen Feld nachgezogen werden müsste; das Attribut steht
dort, wo das Feld steht.

    #[param(name = "Counterbore Ø", …, visible_when = "style=counterbore")]

Grammatik: Bedingungen per `,` sind **UND**-verknüpft, Werte per `|` innerhalb einer Bedingung
**ODER**. `"end_type=up_to_next|up_to_entity"` und `"end_type=blind, tip=custom"` sind beide nötig
— ohne UND hinge das freie Spitzenmaß am `through_all`-Loch weiter im Dialog. Der Spec-String wird
**zur Compile-Zeit** geprüft (Tippfehler = Build-Fehler, nicht ein Feld, das still nie erscheint).
Wire-Form ist konsequent eine **Liste** von Bedingungen; die alte Ein-Objekt-Form ist ersatzlos
weg (kein Compat-Shim, CLAUDE.md), `equals` akzeptiert Skalar oder Array.

Annotiert sind 13 Felder: `fit` (nur clearance), `diameter` (nur custom-Größe), `start_plane`
(nur selected_plane), `depth` · `tip` · `depth_to_tip` (nur blind), `tip_custom` (blind UND
custom), `up_to_entity` (nur up_to_entity), `end_offset` (up_to_next ODER up_to_entity), die
beiden `cbore_*` (nur counterbore) und die beiden `csink_*` (nur countersink).

*Gate gemessen* (`hole_dialog_shows_only_the_fields_that_apply`): der Test wertet die Regel in
Rust **auf dem echten `schema_json()`** aus, mit derselben Semantik wie `isVisible` in TS, und
vergleicht die Schlüsselmenge exakt — plus die Gegenprobe, dass jede Bedingung einen Parameter
nennt, den es auch gibt. Fünf Konfigurationen: Default-M6-blind zeigt 11 Felder (kein fit, kein
freies Ø, keine Senkmaße, kein Up-to-Ziel); clearance+custom+through_all zeigt `fit` und
`diameter`, aber **weder `depth` noch den Anschliff**; counterbore+up_to_next zeigt `end_offset`
und die zwei Senkmaße, keine Tiefe und keine Startebene; countersink+selected_plane+up_to_entity
zeigt `start_plane`, `up_to_entity`, `end_offset` und das Kegelpaar; und die UND-Probe:
`blind + tip=custom` zeigt `tip_custom`, `through_all + tip=custom` blendet den **ganzen**
Spitzen-Block aus. Web-seitig prüfen drei neue `isVisible`-Tests Array-ODER, Mehrfach-UND und
das Zurückweisen der alten Objektform.

**Auto-Name.** Ein Loch heißt jetzt nach dem, was es bohrt: `M6 tapped`, `M8 clearance
counterbore ×2`, `Ø7.7 drilled countersink` — statt `Hole 7`. Die Regel (`hole::auto_name`) steht
neben dem Schema, das sie erzeugt, und `synth_name_for_call` fragt sie; ein vom Nutzer gesetzter
Name gewinnt weiterhin. Bewusst nur Größe + Typ + Stil (+ `×N` ab zwei Bohrungen): das sind die
Merkmale, an denen man zwei Löcher im Baum unterscheidet — Endbedingung und Anschliff schlägt man
nach, sie sind keine Identität. Die Größe folgt derselben Entscheidung wie
`effective_diameter_mm`, kann also nie eine Norm behaupten, die die Geometrie nicht benutzt hat.
*Gate:* `hole_names_itself_after_what_it_drills` über `add_feature_call` → `list_features`.

**Sketch-Punkt-Picker-UX.** Hover-Highlight und Box-Select, beide auf vorhandenen Mustern:

- *Hover:* exakt die fs23-Brücke für Sketch-LINIEN, eine Entitätsart weiter
  (`setHoveredSketchPoint`/`subscribe…` neben `setHoveredSketchLine`). Der Viewport publiziert bei
  Pointer-Move den Punkt unter dem Cursor, sobald ein Picker mit `SKETCHPOINT`-Filter scharf ist;
  das Overlay zeichnet ihn als 1.6-fach großen Punkt in der Auswahlfarbe über den normalen
  (`makeSketchDots`, RenderOrder 999 wie die gehoverte Linie). Ohne das ist ein Bohrpunkt ein
  8-Pixel-Ziel ohne jede Rückmeldung.
- *Box-Select:* kein neues Selektions-Framework — die vorhandene Rechteck-Geste liefert `project`
  und `ndcRect` bereits fertig; neu ist die reine Funktion `sketchPointsInRect` in `boxSelect.ts`
  neben `elementsInRect`/`bodiesInRect` und ein Zweig in `handlePointerUp`, der bei scharfem
  Punkt-Picker die gefundenen `qSketchPoint(…)` einzeln an `picker.onPick` gibt statt die Auswahl
  zu ändern. Der `MultiQueryWidget`-Picker akkumuliert synchron und deaktiviert sich nicht nach
  einem Pick, also reicht ein Aufruf je Punkt. Fenster/Kreuzen fallen zusammen — ein Punkt hat
  keine Ausdehnung.

**Senkmaß-Vorbelegung nach Norm: NICHT ausgeliefert — die Quellen widersprechen sich.** Der
Auftrag war explizit an eine 1:1-Transkription gebunden. Recherchiert wurden vier Quellen:

| M | AmesWeb (Ø/Tiefe) | EngineersBible DIN 912 | neue-physik DIN 974-1 | schraube-mutter DIN 974 |
|---|---|---|---|---|
| M3 | 6.50 / 3.00 | 6.5 / 3.5 | 6.5 / 3.4 | 6.00 / 2.40 |
| M6 | 11.25 / 6.00 | 11.0 / 6.8 | 11.0 / 6.4 | 11.00 / 4.70 |
| M8 | 14.25 / 8.00 | 15.0 / 8.8 | 15.0 / 8.6 | 15.00 / 6.00 |
| M12 | 19.25 / 12.00 | 20.0 / 12.8 | 20.0 / 12.6 | 20.00 / 8.00 |
| M24 | 37.50 / 24.00 | 40.0 / 25.0 | 40.0 / 24.8 | 40.00 / 13.80 |

Der **Durchmesser** ist gut belegt: zwei Quellen sind auf der DIN-974-1-Reihe-1 zeilenweise
deckungsgleich, eine dritte in 9 von 10 Zeilen (M3 6.0 statt 6.5 — vermutlich Reihe 1 vs. 2).
AmesWeb fährt eine ganz andere (ASME-nahe) Reihe und ist wie bei ISO 273 in P2 nicht verwendbar.
Die **Tiefe** dagegen liefert vier Quellen mit vier Werten pro Zeile, weil sie von der
Schraubenkopfhöhe der jeweiligen Norm abhängt (DIN 912 vs. ISO 4762 vs. „Kopfhöhe + Zugabe"). Beim
Countersink ist es schlimmer: ISO 15065 (Dc M6 = 12.60–12.85) und DIN 74 (d2 M6 = 13.7) sind
verschiedene Reihen für verschiedene Kopfformen, und die ISO-Zahlen ließen sich nur aus einem
Suchtreffer greifen, nicht aus dem Normtext.

Ein halber Auto-Fill — richtiger Ø, erfundene Tiefe — wäre schlechter als ein ehrlicher fester
Default: er sieht normkonform aus. Die Senkmaße bleiben daher bei den P3-Vorgaben (11 / 6 mm bzw.
12 mm / 90°) und **manuell**. Nachzuholen, sobald der DIN-974-1- bzw. ISO-15065-Normtext selbst
vorliegt — dann mit Transkriptions-Guard wie bei `hole_iso`.

Testzahlen: Kernel 1513 → **1515** grün, Web 2072 → **2082** grün, `tsc -b` sauber.

## 7. Offene Fragen / Risiken

1. **Sketch-Punkt-Referenzierung (größtes Risiko, blockiert P1-Zielselektion):** Taucht ein
   Sketch-Punkt als B-Rep-Vertex auf (→ `qVertex` + `resolve_mate_anchor` reicht) oder braucht
   es eine neue `qSketchPoint(sketch, entity)`-Query samt Auflösung (Position im `SketchFrame`,
   Normale = Sketch-Ebenen-Normale)? Muss vor P1 empirisch geklärt werden (Sketch anlegen,
   Entity-ID greifen, Query-Resolver testen). Fallback für P1: nur Mate-Connector-/Vertex-Ziele,
   Sketch-Punkte in P1.5 nachziehen.
2. **Tapped kosmetisch vs. real:** Empfehlung wie Onshape — kosmetisch (Kernloch-Volumen,
   Gewinde nur als Callout/Annotation). Echtes Wendel-Gewinde ist teuer und für die
   Fertigungs-Semantik unnötig. Callout-Annotation (Measure-Overlay?) ist optional/spät.
3. ~~**Konditionale Feld-Sichtbarkeit im Schema:**~~ **Geklärt (P6):** die TS-Seite konnte
   `visible_when` schon seit K6a.1 (Parser + `isVisible` + Dialog-Filter), es fehlte nur der
   Produzent. Gelöst mit dem additiven Derive-Attribut `#[param(visible_when = "…")]`, UND/ODER
   fähig und compile-zeit-geprüft — nicht Web-seitig per Typ-Enum, das wäre eine zweite Wahrheit
   über das Schema gewesen.
4. **Bohrachsen-Vorzeichen:** Konvention +Z_local des Ziel-Frames „in den Body hinein"; bei
   Vertex/MC muss die Face-Normale konsistent nach außen zeigen, sonst bohrt das Loch vom
   Material weg. `opposite_direction` als Nutzer-Override; ggf. Auto-Sign über Body-Innentest.
5. **Through-all-AABB entlang schräger Achse:** Bei nicht-achsparalleler Bohrachse die
   Body-AABB konservativ entlang der Achse projizieren (+ε); Standard-AABB-Muster wiederverwenden.
6. **PEM® & Inch/ANSI-Normen:** Bewusst aus dem Erst-Scope gelassen (metrisch zuerst). Enum-Slots
   offen halten, aber keine ANSI-Tabellen in P1–P6.
7. **Norm-Daten-Korrektheit:** Die const-Tabellen müssen 1:1 aus ISO 261/273 transkribiert
   werden (Transkriptions-Guard-Test `tap≈nominal−pitch`), nicht aus Web-Snippets geraten — die
   §4.2-Werte sind Richtwerte zur Größenordnung. **Offen geblieben (P6):** die Senkungs-Tabellen
   (DIN 974-1 / ISO 15065) haben diese Hürde NICHT genommen — vier Quellen, vier Tiefen pro Zeile,
   zwei unvereinbare Countersink-Reihen. Die Senkmaße bleiben manuell, s. den P6-Abschnitt.

### Quellen

- Onshape Help — Hole feature: https://cad.onshape.com/help/Content/hole.htm
- ISO 261 (metrische Regelgewinde-Steigungen), ISO 273 / ISO 20273 (Durchgangslöcher).
- Kernloch-Faustformel `tap_drill = Nenn-Ø − Steigung` (75 % Tragtiefe), verifiziert.
