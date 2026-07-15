# Plan: Loft-Feature (Interpolation zwischen N Profil-Sketches → Solid)

**Status:** Design-Phase (2026-07-16, Ticket #385). Noch **kein** Code — dieses Dokument ist SSOT
für die gestaffelte Umsetzung in Folge-Turns. **Branch-Ziel:** `main`.

> **Warum jetzt.** Loft existiert im Kernel **nicht** (weder Op noch Web-Feature). Der Onshape-Import
> (#376) kann `featureType: "loft"` deshalb nicht übersetzen — das Ziel-Part (gelofteter C-Kanal in
> `web/src/onshape/fixtures/ticket339-features.json`, „Loft 1") degradiert zu simplen Extrudes. Loft
> ist ein eigener, substanzieller Arbeitsstrang: neue Kernel-Operation (SDF/CSG-Knoten + Opcode +
> Mesh/Weave-Anbindung), Web-Feature (`addLoft`) und ein Onshape-Übersetzer.

---

## 0. Ziel & Abgrenzung

**Ziel:** Ein Feature `loft`, das zwischen **N ≥ 2** Profil-Sketches (bzw. planaren B-Rep-Faces)
interpoliert und einen Solid erzeugt, der durch alle Profile läuft und an den Endprofilen mit
planaren Deckeln (Caps) schließt.

**In-Scope (v1):**
- N ≥ 2 Profile auf **parallelen Ebenen**, gestapelt entlang einer **gemeinsamen Achse** (die
  Profil-Ebenen-Normale). Das deckt den #376-C-Kanal und den 95 %-Onshape-Fall ab.
- **Lineare** Querschnitt-Interpolation (segmentweise zwischen benachbarten Profilen).
- Boolean-Integration (`New/Add/Remove/Intersect`) + Merge-Scope wie Extrude/Sweep.
- Render (sdf-vm-Sphere-Trace), Mesh (DC/Lichen) und Weave/B-Rep-Selektierbarkeit.

**Out-of-Scope (spätere Phasen / Tickets):**
- **Führungskurven** (guide curves) — eigene Phase P7.
- **Nicht-planare / gekippte** Profile (Profil-Ebenen nicht parallel) — braucht per-z-Framing wie
  Sweep; deferred.
- Tangenten-/Krümmungs-Randbedingungen an den Endprofilen (Onshape `LoftEndDerivativeType`).
- SURFACE-Loft (offene Fläche ohne Volumen) — v1 nur `bodyType == SOLID`.
- Periodischer/geschlossener Loft (letztes Profil → erstes zurück).

---

## 1. Design-Kern: Wie repräsentiert man einen Loft als SDF?

Das ist die schwierigste Entscheidung. Drei Optionen, ehrlich bewertet — Empfehlung am Ende.

### Vorbild-Mechanik im Kernel (existiert bereits)

Ein `CsgNode::Extrude { sketch, half_height, .. }` (`csg.rs:790`) wertet so aus (`csg.rs:1976`,
`sdf_extrude` `csg.rs:2199`): das **2D-Profil-Feld** `sketch.sdf_with_edges([x,y], eps)`
(`sketch2d.rs:214`) wird über die Höhe `z` **hart auf das Segment beklemmt**
(`max(d2d, |z|−half_height)` bzw. die exakte IQ-Ecken-Komposition in `OP_EXTRUDE`,
`sdf_compiler.rs:1294`). Loft ist **dieselbe Mechanik mit einem z-abhängigen 2D-Feld**.

Zusätzlich existiert bereits `CsgNode::Morph(a, b, t)` (`csg.rs:767`, eval `csg.rs:1648`:
`(1−t)·da + t·db`) samt Opcode `OP_MORPH` (`0x2A`, `sdf_compiler.rs:58`) und
`march_relax(Morph) = min(relax(a), relax(b))` (`sdf_compiler.rs:153`). Das ist ein **globaler
3D-Feld-Lerp**, nicht direkt ein Loft, aber der Beweis, dass Feld-Interpolation im Kernel schon lebt.

### Option (a) — z-beklemmtes 2D-Querschnitt-Morph  ★ EMPFOHLEN

An Höhe `z`: finde das **Bracket-Segment** `[z_k, z_{k+1}]` zwischen zwei benachbarten Profilen,
Parameter `t = (z − z_k)/(z_{k+1} − z_k) ∈ [0,1]`. Der interpolierte 2D-Querschnitt ist der
**Feld-Lerp** der beiden Profil-Felder:

    d2d(x,y,z) = (1−t)·sketch_k.sdf(x,y) + t·sketch_{k+1}.sdf(x,y)

Danach **hart in z beklemmen** auf `[z_0, z_{N−1}]` (Deckel wie Extrude). N > 2 ist **stückweise**:
jedes Segment ist ein eigener Lerp; an den inneren Profil-Ebenen ist das Feld C0-stetig (das Profil
selbst) und der Segmentwechsel unsichtbar.

- **Der große Gewinn — korrespondenzfrei.** Der Feld-Lerp interpoliert die **Level-Sets implizit**;
  er braucht **keine Punkt-Korrespondenz** zwischen Loops unterschiedlicher Vertex-Zahl (das
  klassische, schwierigste Loft-Problem). Ein Kreis (Circle-Prim) und ein Rechteck (Poly-Prim) mit
  völlig verschiedener Segmentzahl morphen ohne Reparametrisierung — der konvexe Mittel-Querschnitt
  ist einfach `{ (1−t)d_A + t d_B = 0 }`. Das ist der zentrale Grund, SDF statt B-Rep-Skinning zu
  wählen.
- **Machbarkeit im vorhandenen Kernel: hoch.** Wiederverwendet `sketch.sdf_with_edges` (Profil-Feld),
  die Extrude-z-Klemme (`OP_EXTRUDE`-Muster) und die Morph-Lerp-Idee (`OP_MORPH`). Kein Mesher-Umbau
  nötig, solange das SDF + die AABB korrekt sind.
- **Aufwand: mittel.** Ein neuer `CsgNode::Loft`-Variant + Opcode(s) + der bekannte ~7-Dateien-Ripple
  (s. P2). Kein neuer Meshing-Pfad.
- **Lipschitz-/Distanzfeld-Konsequenz (die reale Kosten, P5).** Bei festem `t` ist `(1−t)d_A + t d_B`
  als Konvexkombination zweier 1-Lipschitz-Felder **in (x,y) 1-Lipschitz**. ABER das Feld variiert
  **auch in z** (über `t` und über die Formänderung), also bekommt der 3D-Gradient einen `∂d/∂z`-Term
  `≈ |d_B − d_A| / (z_{k+1} − z_k)`. Ändern sich die Profile stark über eine kurze Höhe, wird
  `L > 1` → der Sphere-Trace **übersteigt** die Fläche und tunnelt → **grauer Render** (exakt der
  #382-Fehlermodus). Absicherung ist Pflicht (P5): `march_relax(Loft)` als Dämpfung aus der maximalen
  lateralen Profil-Verschiebung pro Höhe, plus ggf. Feld-Normalisierung. Der Lerp ist außerdem **kein
  exaktes** Distanzfeld (Betrag nur genähert) — für das Marschieren/Bracketing genügt eine gültige
  ≤1-Lipschitz-Unterschranke, nicht die exakte Distanz.

### Option (b) — Skinning zu Mesh/B-Rep, dann als `ImportedMesh`

Loops paarweise zu Ruled-Quads vernähen → Dreiecksnetz → `Primitive::ImportedMesh { mesh: MeshSdf }`
(`protocol/src/lib.rs:312`, nearest-tri + ray-parity SDF).

- **Pragmatisch**, liefert **exakte Regelflächen** zwischen den Profilen.
- **Verliert den SDF-Vorteil**: `ImportedMesh` ist ein schweres, nicht sauber Lipschitz-beschränktes
  Feld (Ray-Parity), regen-teuer, und die B-Rep-Faces sind Dreieckssuppe statt analytischer/
  parametrischer Flächen (keine saubere Face-Selektion).
- **Verlagert das Korrespondenzproblem** genau dorthin, wo es weh tut: die Skin-Quads **brauchen**
  Loop-Vertex-Matching bei unterschiedlicher Vertexzahl — das Problem, das Option (a) umgeht.
- Höchstens **pragmatischer Zwischenschritt**, falls Option (a) an einem konkreten Profilpaar
  scheitert. Nicht das Fundament.

### Option (c) — Hybrid

SDF-Morph (a) als Feld für Render/CSG/Mesh, **zusätzlich** pro Profilpaar Ruled-B-Rep-Patches für
die Selektion seeden (analog wie Extrude Deckel als retained Faces und Wände als nicht-analytische
Profilflächen führt). Für v1 **Overkill**; die Seitenflächen fallen in P3 ohnehin als
nicht-analytische Profilflächen aus dem Weave (wie Sweep-Wände), das reicht zunächst.

### Empfehlung

**Option (a).** Korrespondenzfrei (löst das härteste Loft-Problem gratis), wiederverwendet die
bewiesene Extrude-z-Klemme und den Morph-Lerp, braucht **keinen** Mesher-Umbau, ist parametrisch und
regen-billig. Die einzige echte Kosten — der z-Gradient kann `L > 1` verletzen — ist genau das, was
die verpflichtende Lipschitz-Phase P5 abfängt (`march_relax`-Dämpfung + optionale Normalisierung,
Vorbild: `REVOLVE_MARCH_RELAX`, `Displace`-Dämpfung `sdf_compiler.rs:167`, und die
`masked_blend_render_fade`-√2-Normalisierung aus #382, `sdf_compiler.rs:1794`).

---

## 2. Feld-Qualität ist eine PFLICHT-Phase, kein Nice-to-have (aus #382)

Der sdf-vm-Sphere-Trace nimmt `d` als gültige Distanzschranke. Ein Feld-Operator, der die
~1-Lipschitz-Eigenschaft verletzt (Sprünge/steile Gradienten entlang der Strahlen), lässt Strahlen
durch die Oberfläche **tunneln** → Render bricht zu grau. Bei Fillets war die Lösung
`masked_blend_render_fade` + √2-Normalisierung in `sdf_compiler.rs` plus eine **Lipschitz-Probe im
Test** (`ticket_382_..._lipschitz_probe`). `march_relax(MaskedBlend)` wurde dadurch wieder `1.0`.

**Loft erbt dieses Risiko direkt** (der z-Gradient des Morphs, §1a). Der Plan enthält deshalb **P5**
als Pflicht: eine 3D-Gradienten-Probe über das Loft-**Render**-Feld, die `|∇f| ≤ 1` (bzw. die
korrekte `march_relax`-Dämpfung) absichert — analog `ticket_382_filleted_field_lipschitz_probe`,
plus Headful-WebGPU-Sichtprüfung (Loft rendert, kein Grau/Tunneln).

---

## 3. Phasenplan

### P1 — Kernel-Datenstruktur + Feature-Enum + N-Profil-Resolve

**Ziel:** Ein `loft`-Feature parst N Profil-Referenzen, löst sie zu (Sketch2d, Achsen-Offset) auf und
pusht einen (zunächst evtl. nur AABB-korrekten) Platzhalter-Knoten.

**Betroffen / neu:**
- `crates/protocol/src/lib.rs` — neuer `Primitive::Loft { profiles: Vec<LoftSection>, center: [f32;3],
  wall_subs: Option<Vec<u32>> }`, `LoftSection { sketch: Sketch2d, offset: f32 }` (offset = Position
  entlang der lokalen Z/Loft-Achse). `Primitive::kind`/`type_label`-Arme (`lib.rs:507/566`).
- `crates/kernel/src/features/loft.rs` (**neu**) — `LoftDef` mit
  `#[derive(FeatureSchema)] #[schema(type = "loft")]`, Feld `profiles:
  fielddraft_protocol::MultiQuery` (`#[param(entity_filter = "SKETCHREGION, FACE")]`, wie
  `ExtrudeDef.profiles` `extrude.rs:98`), `operation: BoolOp`, `targets`, `merge_with_all`. Struct
  `LoftFeature: FeatureKind`.
- `crates/kernel/src/features/mod.rs` — `pub mod loft; pub use loft::*;` und
  `r.register(Box::new(LoftFeature))` in `with_builtins` (`mod.rs:329`).
- `crates/kernel/src/features/context_resolve.rs` — `resolve_loft_profiles(&[String]) ->
  Result<(Vec<LoftSection>, SketchFrame), String>`: jede Query via `resolve_profile_query`
  (`sketch/lower.rs:161`, Sketch-Regionen) bzw. `resolve_extrude_faces` (`context_resolve.rs:159`,
  planare Faces) auflösen; die gemeinsame Loft-Achse = Profil-Ebenen-Normale; Profile **nach
  Achsen-Koordinate sortieren**; `offset_k` = Projektion des Profil-Ursprungs auf die Achse relativ
  zum ersten Profil. Fehlerfälle: < 2 Profile, nicht-parallele Ebenen, leere Regionen.

**Tests (`features_tests.rs` / `sketch_features_tests.rs`):** Parse; Resolve von 2 koplanar-parallelen
Sketch-Regionen → 2 Sections mit korrekten Offsets; Fehler bei 1 Profil und bei gekippten Ebenen.

**Fertig, wenn:** `loft`-FeatureCall regeneriert ohne Panik, pusht `Primitive::Loft` mit korrekter
Section-Liste + Frame; Boolean/Merge-Scope-Warnungen wie Extrude.

### P2 — SDF/CSG-Knoten + Opcode(s) + Eval

**Ziel:** Das Loft-Feld auf allen drei Auswertungspfaden (CPU-Weave, Dual, Renderer-Opcodes) korrekt.

**Betroffen / neu:**
- `crates/kernel/src/csg.rs` — neuer `CsgNode::Loft { id, sections: Vec<(Box<Sketch2d>, f32)>,
  wall_subs }`. Eval (`sdf`/`sdf_open`, ~`csg.rs:1946`-Arm neben Extrude): z-Bracket → `t` → Lerp der
  beiden `sdf_with_edges`-Bänder → z-Klemme (`sdf_extrude`-Analog auf `[offset_0, offset_{N−1}]`).
  `aabb` (`csg.rs:2117`-Arm) = Union der Profil-AABBs, jeweils auf ihre Offset-Ebene gehoben.
  `Primitive::Loft` → `CsgNode::Loft` in `lower`/`from_primitive` (`csg.rs:967`ff). `all_finite`,
  `contributors`, Tree-Visitor/`with_open`-Arme (`csg.rs:1429/1496/1526/1556`), `id`-Arme
  (`csg.rs:1511/1564`).
- `crates/kernel/src/sdf_dual.rs` — Dual-Number-Eval (Gradient) analog `sdf_extrude_generic`
  (`sdf_dual.rs:155`).
- `crates/kernel/src/cell_csg.rs` — Intervall-/Cell-Eval-Arm (Bracketing fürs Octree; `combine_morph`
  `cell_csg.rs:564` als Vorbild für den t-Lerp).
- `crates/kernel/src/sdf_compiler.rs` — **`BoundedCsg::Loft`** + Emission. Zwei Wege möglich:
  (i) einen dedizierten `OP_WARP_LOFT` (wählt Segment aus `p.z`, setzt `t`) + zwei `emit_sketch`-Tapes
  + `OP_LOFT_MORPH` (2D-Lerp, wie `OP_MORPH` aber auf den 2D-Stack) + `OP_EXTRUDE`-artige z-Klemme;
  oder (ii) den Knoten als **N−1 gemorphte Extrude-Segmente per `OP_INTERSECT`/Segment-Maskierung**
  entfalten. Empfehlung: **(i)** — kompakter, kein Segment-Überzähl-Artefakt. Nächster freier Opcode
  nach `OP_WARP_SWEEP` (`0x38`): `OP_WARP_LOFT = 0x39`, `OP_LOFT_MORPH = 0x3A` (Konstanten +
  Interpreter-Arm `eval_opcodes` `sdf_compiler.rs:~1294`, WGSL/TSL-Codegen für den Renderer).
- `crates/kernel/src/step_classifier.rs`, `crates/kernel/src/brep/weaving.rs`,
  `crates/kernel/src/brep/extraction.rs`, `body.rs`, `bvh.rs` — der bekannte, vom Compiler erzwungene
  Match-Ripple (Vorbild: Ripple-Liste in `docs/THICKEN-SDF-PLAN.md` §1).

**Tests:** `sdf_parity_tests.rs` — CPU-`sdf` == Opcode-`eval_opcodes` an Stichproben; Zero-Set an den
Profil-Ebenen `z = offset_k` **gleich dem Profil** `sketch_k`; Slab-AABB korrekt; N=2 und N=3.

**Fertig, wenn:** SDF-Parität grün, Nullfläche an den Profilen exakt, AABB dicht.

### P3 — Mesh- und Weave-Anbindung

**Ziel:** Loft vermascht wasserdicht und liefert selektierbare Faces.

- **Mesh** (`kernel_core`/DC/Lichen): fällt **gratis** aus einem korrekten SDF + dichter AABB —
  primär verifizieren, nicht neu bauen. `march_relax(Loft)`-Eintrag (`sdf_compiler.rs:140`) setzen
  (Wert aus P5).
- **Weave/B-Rep:** Endprofil-Deckel als **retained planar Faces** (wie Extrude-Caps,
  `EXTRUDE_CAP_TOP/BOTTOM_SUB`, `sdf_compiler.rs:361`); die Loft-Seitenwand ist eine
  **nicht-analytische Profilfläche** und bleibt (wie Sweep-Wände, `sdf_compiler.rs:438` /
  Leaf-Pfad) beim generischen Traced-Face-Pfad. Innere Profil-Ebenen v1 **nicht** als Naht führen
  (glatter Durchlauf), es sei denn ein Profil ist eine echte Kante.

**Tests:** Euler F/E/V watertight für N=2/3; Deckel selektierbar (`qFace`); Headful-Mesh-AABB stimmt.

**Fertig, wenn:** wasserdichtes Mesh, Deckel pickbar, keine Phantom-Flächen.

### P4 — Web-Feature (`addLoft`) + UI + MultiQuery-N-Profile

**Ziel:** Loft in der App anlegbar.

- **Schema-getriebene UI:** `LoftDef::schema_json()` erzeugt den FeatureDialog automatisch
  (`profiles` als `MultiQuery` mit `entity_filter = SKETCHREGION, FACE` — der Viewport-Profil-Picker
  akkumuliert **N** Regionen/Faces in einer Pick-Session, exakt wie Extrude `k280`).
- `crates/kernel/src/api/features.rs` — optionaler `#[wasm_bindgen(js_name = addLoft)]`-Bequemlichkeits-
  Wrapper (analog `add_extrude` `features.rs:247` / `add_sweep` `features.rs:363`); der generische
  `kernel.addFeatureCall("loft", …)`-Pfad (`onshapeImport.ts:420`) genügt aber bereits.
- `web/src/store.ts` — `addLoft(profileQueries: string[])` (Muster `addExtrudeFaces` `store.ts:1027`).
- `web/src/kernel.ts` — Feature-Kind-Union um `"loft"` (`kernel.ts:289`).
- `web/src/Toolbar.tsx` (Button), `web/src/leafMeshFactory.ts` (`case "loft"`, `leafMeshFactory.ts:66`),
  `web/src/webglBodies.tsx` (`webglBodies.tsx:116`), `web/src/transformGizmo.tsx`
  (`transformGizmo.tsx:33`) — Feature-Kind in den jeweiligen Listen ergänzen.

**Tests (vitest):** Store legt `loft`-FeatureCall mit N Profil-Queries an; Regen liefert einen Body;
Outliner-Label.

**Fertig, wenn:** Nutzer wählt 2+ Sketch-Regionen → „Loft" → Solid erscheint, editierbar über den
Dialog.

### P5 — Lipschitz-Absicherung / Render-Qualität (PFLICHT, siehe §2)

**Ziel:** Der Loft rendert im sdf-vm-Sphere-Trace ohne Tunneln/Grau.

- `march_relax(Loft)` (`sdf_compiler.rs:140`) = `min` über die Kinder-Relaxe **und** einen
  Loft-Dämpfungsfaktor `1 / L_z`, wobei `L_z ≈ 1 + max_k( max_lateral_shift(sketch_k, sketch_{k+1}) /
  (offset_{k+1} − offset_k) )` — der worst-case-z-Gradient. Vorbild: `Displace`-Dämpfung
  (`sdf_compiler.rs:167`) und `REVOLVE_MARCH_RELAX`. Feld bleibt für Mesher/Weave **unverändert**
  (nur der Renderer-Schritt wird gedämpft — dieselbe Trennung wie bei Displace/Revolve; die
  Feld-Normalisierung verschlechtert nachweislich den Lichen-Mesher, `bvh.rs`-Befund 2026-07-13).
- Falls Dämpfung allein den Trace zu langsam macht: optionale √2-artige Render-Feld-Normalisierung
  im Emit-Pfad (Vorbild `masked_blend_render_fade` `sdf_compiler.rs:1794`), Zero-Set-erhaltend.
- **Regressionstest `ticket_385_loft_field_lipschitz_probe`** (analog
  `ticket_382_filleted_field_lipschitz_probe`): 3D-Gradienten-Probe über das kompilierte Render-Feld
  eines Test-Lofts (z. B. kleiner Kreis → großes Rechteck über kurze Höhe = worst case) sichert
  `|∇f| ≤ 1` bzw. dass `march_relax` die korrekte Dämpfung meldet.

**Fertig, wenn:** Probe-Test grün; Headful-WebGPU zeigt den Loft ohne Grau/Tunneln bei Streifwinkeln.

### P6 — Onshape-Loft-Übersetzer + #376-Entblockung

**Ziel:** Onshape `featureType: "loft"` importiert als FieldDraft-Loft.

- `web/src/onshape/onshapeImport.ts` — neuer Branch `if (feat.featureType === "loft")` (neben
  `"extrude"` `onshapeImport.ts:385`): den Array-Parameter **`sheetProfilesArray`**
  (`BTMParameterArray-2025`) lesen; jedes `items[].parameters[].sheetProfileEntities`
  (`BTMParameterQueryList-148`) ist die Profil-Query **eines** Querschnitts → via `profileSketchId` /
  `decodeAnyOpRef` (wie Extrude `onshapeImport.ts:386/415`) zu `qCreatedBy(<sketch>)`/Region auflösen.
  `operationType` → `operation`; `bodyType == SOLID` erforderlich (SURFACE → Warn+Skip); `guidesArray`
  / `LoftEndDerivativeType` v1 ignoriert (Warnung). Dann
  `kernel.addFeatureCall("loft", { display_name, profiles: "<q0>, <q1>, …", operation, … })`.
- Fixture liegt vor: `web/src/onshape/fixtures/ticket339-features.json` („Loft 1",
  `sheetProfilesArray` mit ≥ 2 `items`).

**Tests:** `web/src/onshape/onshapeImport.test.ts` — Loft-Feature aus der Fixture → ein `loft`-Call
mit N Profil-Queries; #376-Regression: der C-Kanal degradiert nicht mehr zu Extrudes.

**Fertig, wenn:** Der ticket339-Loft importiert als echter Loft; #376-Blocker entfernt.

### P7 — Führungskurven (später / optional)

`LoftDef.guides` (Edge-/Sketch-Queries). Der Querschnitt-Lerp wird durch die Guide-Verschiebung
**gewarpt** (die Guide gibt pro z den lateralen Versatz/Skalierung vor, statt reiner Linear-t). Nutzt
die Sweep-Framing-Infrastruktur (`path_provider`, `sweep_path::SweepPath`). Eigene Phase, eigenes
Ticket — erst nach P1–P6-Erfahrung.

---

## 4. Risiken & offene Fragen

1. **Loop-Korrespondenz vs. Feld-Mittel.** Der SDF-Lerp umgeht das Vertex-Matching, aber der
   Mittel-Querschnitt ist das implizite Feld-Mittel `{(1−t)d_A + t d_B = 0}`, **keine** klassische
   Regelfläche mit pointwise-linearen Vertices. Für gleiche Topologie (translatiert/skaliert, C-Kanal)
   sehr gut; bei stark verschiedenen Profilen kann die Mittel-Isofläche einschnüren oder der
   Feld-Betrag von der echten Distanz driften (Lipschitz). **Am #376-C-Kanal validieren, bevor
   zugesagt wird** (User-Direktive „echtes user-sichtbares Ergebnis prüfen").
2. **Lipschitz / z-Gradient (P5).** Starke Formänderung über kurze Höhe → `L > 1` → Tunneln/Grau
   (#382-Modus). Muss über `march_relax`-Dämpfung + optionale Normalisierung + Probe-Test abgesichert
   werden. Härtestes technisches Risiko.
3. **Nicht-planare / nicht-parallele Profile & Selbstdurchdringung.** v1 setzt parallele Ebenen +
   gemeinsame Achse voraus (früh mit klarer Fehlermeldung ablehnen). Gekippte Profile brauchen
   per-z-Framing (Sweep-Analog) — deferred. Sehr unterschiedliche Größen/Positionen können
   Selbstdurchdringung erzeugen; numerischer Guard statt algebraisch.
4. **Offset-Bestimmung der Achse** bei Profilen, deren Ebenen-Ursprünge nicht auf einer Geraden
   liegen (nur v1-parallel, aber lateral versetzt): Achse = gemeinsame Normale, `offset` = Projektion
   — laterale Versätze sind erlaubt und morphen mit; nur die **Normalen** müssen parallel sein.
5. **Innere Profile als Naht?** v1 lässt innere Profil-Ebenen glatt durchlaufen (keine B-Rep-Kante).
   Falls ein Nutzer dort eine Kante erwartet, spätere Option.

---

## 5. Validierung & Reihenfolge

Pro Phase: Kernel-Unit + Integration; SDF-Parität (CPU/Opcode); Weave-Wasserdichtheit (Euler F/E/V);
Headful-WebGPU-E2E (rendert, AABB stimmt, kein Grau); `tsc`/`vitest`; `cargo clippy`; `just
build-wasm` (WASM-Artefakt committen — laufender Dev-Server serviert sonst altes WASM).

| Schritt | Inhalt | Gate |
|--------|--------|------|
| P1 | `LoftDef`/`Primitive::Loft` + N-Profil-Resolve | Feature regeneriert, Sections korrekt |
| P2 | `CsgNode::Loft` + Opcodes + Eval (CPU/Dual/Cell/WGSL) | SDF-Parität, Zero-Set an Profilen exakt |
| P3 | Mesh/Weave | watertight, Deckel pickbar |
| P4 | Web-Feature + UI | Nutzer loftet 2+ Regionen → Solid |
| P5 | Lipschitz-Absicherung | Probe-Test grün, Headful kein Grau |
| P6 | Onshape-Übersetzer | ticket339-Loft importiert, #376 entblockt |
| P7 | Führungskurven | (später, eigenes Ticket) |

**Entscheidungs-Gates:** nach P2 (SDF-Repräsentation trägt am C-Kanal?), nach P5 (Feld
render-tauglich?), nach P6 (#376 wirklich entblockt — end-to-end am Import prüfen, nicht nur am Test).

---

## 6. Implementierungs-Schablonen (echte Vorbilder im Repo)

- **Feature-Struktur / Resolve:** `crates/kernel/src/features/extrude.rs` (`ExtrudeDef`, MultiQuery-
  Profile, Face- vs. Sketch-Pfad, Merge-Scope) und `features/sweep.rs` (`SweepDef`, modularer
  Path-Resolve, direkter `CsgNode`-Push mit `id.child("0")`).
- **Protocol-Primitive:** `Primitive::Extrude`/`Revolve`/`Helix` (`protocol/src/lib.rs:249/331/351`),
  `wall_subs`-Konvention.
- **CSG-Knoten + Eval:** `CsgNode::Extrude`/`Sweep`/`Morph` (`csg.rs:790/837/767`), `sdf_extrude`
  (`csg.rs:2199`), `extrude_retained_combine`.
- **Opcodes:** `OP_EXTRUDE` (z-Klemme, `sdf_compiler.rs:1294`), `OP_MORPH` (Lerp, `0x2A`),
  `OP_WARP_SWEEP` (`0x38`, letzter belegter Warp), `emit_sketch`/`emit_warp_transform`.
- **Lipschitz/Render:** `march_relax` (`sdf_compiler.rs:140`), `REVOLVE_MARCH_RELAX`,
  `Displace`-Dämpfung (`:167`), `masked_blend_render_fade` + √2 (`:1794`, #382).
- **Web/Onshape:** `kernel.addFeatureCall` (`onshapeImport.ts:420`), Extrude-Import-Branch
  (`onshapeImport.ts:385`), `store.addExtrudeFaces` (`store.ts:1027`), Fixture
  `onshape/fixtures/ticket339-features.json` („Loft 1").
