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
3. **Konditionale Feld-Sichtbarkeit im Schema:** Kennt das Schema/`widgets/schema.ts` schon ein
   `visible_if`/`enabled_if`? Falls nein, entweder Derive-Attribut ergänzen oder Web-seitig per
   Typ-Enum ausblenden. Zu prüfen vor P6.
4. **Bohrachsen-Vorzeichen:** Konvention +Z_local des Ziel-Frames „in den Body hinein"; bei
   Vertex/MC muss die Face-Normale konsistent nach außen zeigen, sonst bohrt das Loch vom
   Material weg. `opposite_direction` als Nutzer-Override; ggf. Auto-Sign über Body-Innentest.
5. **Through-all-AABB entlang schräger Achse:** Bei nicht-achsparalleler Bohrachse die
   Body-AABB konservativ entlang der Achse projizieren (+ε); Standard-AABB-Muster wiederverwenden.
6. **PEM® & Inch/ANSI-Normen:** Bewusst aus dem Erst-Scope gelassen (metrisch zuerst). Enum-Slots
   offen halten, aber keine ANSI-Tabellen in P1–P6.
7. **Norm-Daten-Korrektheit:** Die const-Tabellen müssen 1:1 aus ISO 261/273 transkribiert
   werden (Transkriptions-Guard-Test `tap≈nominal−pitch`), nicht aus Web-Snippets geraten — die
   §4.2-Werte sind Richtwerte zur Größenordnung.

### Quellen

- Onshape Help — Hole feature: https://cad.onshape.com/help/Content/hole.htm
- ISO 261 (metrische Regelgewinde-Steigungen), ISO 273 / ISO 20273 (Durchgangslöcher).
- Kernloch-Faustformel `tap_drill = Nenn-Ø − Steigung` (75 % Tragtiefe), verifiziert.
