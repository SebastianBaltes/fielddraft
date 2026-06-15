# Feature-Snapshot-Semantik — Plan (SSOT)

Status: **PHASEN 1a–1c + 2 UMGESETZT** · Owner: Kernel/Pipeline · Letzte Aktualisierung: 2026-06-15

## Fortschritt

- **Phase 1a (Kontakt-Merge entfernt):** `partition()` Add-Branch dockt nur noch
  über expliziten Link (`target_ids`); untagged Add → eigener Body. Produkt-Regel:
  berührende Solids verschmelzen NIE automatisch, nur per explizitem Boolean-Feature
  bzw. Merge-scope/Face-Owner. Intent-basiert (= Option B): Face-aufgebaute Features
  docken via Owner-Target, freie Features bleiben eigenständig.
  - Tests angepasst: `untagged_add_*` (body.rs) auf „bleibt separat"; neuer
    `tagged_add_docks_into_target_body`; Helix/Boss/Weave-Roundtrip nutzen jetzt
    expliziten Merge (`targets`/Boolean-Union); Vase-Helper + `vase_cutaway` leeren
    den ctx vor jedem Regen (Produktion nutzt pro Regen einen frischen Context —
    der alte Kontakt-Merge kaschierte einen Doppel-Regen-Helper-Bug).
  - Neuer Snapshot-Guard `boolean_forward_referenced_tool_does_not_carve` (lib.rs):
    Tool nach Boolean → kein Schnitt. Ersetzt den explorativen Repro-Test der
    Vorsession (falsche Richtung) + `dbg_boolean_load` entfernt.
- **Nebenbefund (separater Bug, evtl. Ticket 20260615-103929-9wxs „Boolean
  funktioniert nicht mehr"):** Ein Boolean-**Subtract mit Cone-Tool** schnitt selbst
  in korrekter Reihenfolge nicht (Box+Sphere-Subtract dagegen funktioniert). Nicht
  Teil der Merge/Snapshot-Arbeit — separat verifizieren.

- **Merge-Scope-Warnung (v0.6.139):** Feature-implizite Booleans
  (Extrude/Revolve/Helix/Sweep mit op=Add/Remove/Intersect) ohne gültigen
  Merge-Scope werden sichtbar gewarnt — analog Onshape.
  - „Gültiger Scope" = explizite Körperliste (`targets`) **oder** neue
    Checkbox **„Merge with all"** (`merge_with_all`, neuer Param an allen vier
    Features; Sweep bekam zusätzlich `targets`). „Merge with all" expandiert im
    `execute` zu expliziten target_ids = alle aktuellen Body-Seeds (kein
    Struct-/Partition-Umbau nötig; nutzt den targeted-dock-Pfad).
  - Kernel: `Context::feature_warnings` → `Kernel::feature_warnings` →
    `listFeatures.warning` (parallel zum bestehenden `error`-Plumbing). Helper
    `resolve_merge_scope` + `merge_scope_warning` in features.rs.
  - Web: Outliner-Label in `--color-warning` + Tooltip (`.row.has-warning`);
    `materialBridge` weist Warn-Körpern ein halbtransparentes rotes
    `WARNING_MATERIAL` zu (überschreibt User-Material). Checkbox erscheint
    automatisch über den schema-getriebenen Dialog.
  - Test: `add_feature_without_merge_scope_warns` (lib.rs).
  - OFFEN: no-scope **Remove/Intersect** dockt in `partition` noch per
    AABB-Fallback (geteilt mit dem expliziten Boolean-Feature) statt als
    eigenständiges rotes Preview — sauber erst mit dem Snapshot-Evaluator
    (Phase 2) bzw. wenn das Boolean-Feature seinen Scope an die Tools
    weitergibt. Der Add-Fall (Screenshot) ist vollständig korrekt.

- **Phase 1b — Boolean-Forward-Reference-Guard (v0.6.140, löst Ticket 9wxs):**
  Das explizite Boolean-Feature lehnt jetzt Tools/Targets ab, die vor ihm im
  Tree nicht existieren (Vorwärts-Referenz oder gelöscht) → klarer
  `FeatureError` (rotes Outliner-Label + Tooltip) statt stillem No-op. Damit
  ist „Subtract Cone aus Sphere hat keinen Effekt" erklärt: im Ticket stand
  der Cone (id 6) NACH dem Boolean (id 5). Match über `root_ancestor`, damit
  Child-ID-Features (Sweep) korrekt auflösen.
  - Tests: `ticket_9wxs_subtract_cone_from_sphere` (gute Reihenfolge schneidet;
    Forward-Ref → Error, kein Schnitt) — beantwortet „warum fangen Tests das
    nicht?". Cone-Achse ist +Y; korrekte Probe `[0,0,0]`.
  - OFFEN (Phase 1b-Rest): prefix-skopierte Resolution auch für
    `qFace`/`qCreatedBy` der übrigen Features.

- **Phase 1c — UI-Auswahlfilter + Reorder-Guard (v0.6.141):**
  - **Reorder-Guard (Kernel, SSOT):** `Kernel::reorder_feature` lehnt eine
    Umordnung ab, die eine Vorwärts-Referenz erzeugen würde (Feature vor eines
    seiner referenzierten Features ziehen). Helfer `created_by_references`
    sammelt alle `qCreatedBy(<id>)` aus den Params (rekursiv, MultiQuery-aware).
    Test `reorder_rejected_when_it_creates_a_forward_reference`.
  - **Picker-Filter (Web):** `ActivePicker.selectableFeatureIds` (Features VOR
    dem editierten); `MultiQueryWidget` berechnet die Menge aus
    `state.selection` und gibt sie dem Picker mit; `Viewport.tryQueryPick`
    lehnt qCreatedBy-Picks außerhalb ab (+ qFace via Contributor-Check). Damit
    bietet ein Boolean spätere Features gar nicht erst zur Auswahl an
    (Ur-Anforderung des Users).
  - OFFEN: visuelles Hover-Dimming später Geometrie + Pick-Reject-Feedback
    (Toast); `QueryWidget` (Single-Query: Sketch-Plane/Face) noch ohne Filter.

- **Phase 2 — Snapshot-Evaluator (v0.6.142):** Schlüssel-Erkenntnis beim Umbau:
  **`partition()` IST bereits der sequenzielle Left-Fold** (verarbeitet Features
  in Tree-Reihenfolge, eine Gruppe enthält beim Verarbeiten von Feature *i* nur
  Features < *i*) und gibt `Vec<BodyPartition>` aus, das alle ~30 Downstream-
  Stellen konsumieren. Ein Komplett-Rewrite zu einem separaten Body-State-
  Evaluator wäre also reine Code-Umschichtung ohne semantischen Gewinn — Phase 1
  hatte die eigentliche Arbeit (explizit-referenz-getriebenes Andocken, kein
  impliziter Union, Forward-Ref-Guards) bereits geleistet.
  - **Was Phase 2 konkret liefert:** Das explizite Boolean-Feature reicht seinen
    Merge-Scope an die Subtract/Intersect-**Tools** durch (`target_ids`), statt
    auf AABB-Raten zu setzen. Ein Subtract/Intersect mit Scope wirkt jetzt
    **nur** auf die explizit gewählten Zielkörper, nie auf einen zufällig
    überlappenden. Test `boolean_subtract_carves_only_the_explicit_target`.
  - **Snapshot-Invariante vollständig erfüllt:** Jedes Feature wirkt nur auf
    Bodies aus Features VOR ihm (Fold-Struktur), spätere Referenzen sind
    ausgeschlossen (Forward-Ref-Guard Boolean + Reorder-Guard + UI-Filter),
    impliziter Union ist weg.
  - **Bewusst BEIBEHALTEN:** Der untagged-Remove/Intersect-AABB-Fallback
    (un-skopierter Subtract dockt an den *früheren* Körper, auf dem er liegt).
    Das ist KEINE Snapshot-Verletzung (nur frühere Bodies, Left-Fold) und ist
    gewollte UX — das **Hole**-Feature und Face-Subtracts hängen daran (Loch
    bohrt den Körper, auf dem es platziert ist). Würde man es entfernen,
    bräuchte jedes Loch einen expliziten Zielkörper → schlechtere UX, kein
    Korrektheitsgewinn. No-scope-Fälle werden ohnehin gewarnt (siehe
    Merge-Scope-Warnung).
  - **Boolean-Sibling-Mutation (B1)** bleibt der partition-interne
    Kommunikationsweg (Tool trägt die op). Funktional korrekt; ein Umbau zu
    „Boolean trägt eigenen Knoten" wäre groß und ohne Nutzen.

## 1. Anforderung (die eine Invariante)

> **Jedes Feature wird gegen einen Snapshot des Szenenzustands ausgewertet, der genau
> die Features *vor* ihm im Feature-Tree enthält. Ein Feature darf ausschließlich
> Topologie referenzieren, die in diesem Snapshot existiert.**

Das ist die Standard-Semantik parametrischer CAD-Systeme (Onshape/SolidWorks/Fusion): Der
Feature-Tree ist ein sequenziell ausgeführtes Programm. Feature *i* liest den Zustand
`S(i)` = Ergebnis von Features `0..i`, produziert `S(i+1)`. Eine Referenz auf ein Feature
mit Tree-Index `≥ i` ist per Definition ungültig.

### Direkte Konsequenzen
- **UI:** Ein Boolean (oder jedes andere referenzierende Feature) darf in seiner
  Eingabe-Auswahl **nur** Bodies/Faces anbieten, die von Features *vor* ihm stammen.
  Der Cone, der nach dem Boolean im Tree steht, darf nicht auswählbar sein.
- **Kernel:** Eine dennoch gespeicherte Vorwärts-Referenz muss zu einem
  **Feature-Error** führen (Onshape-Semantik, bereits via `RegenErrors` unterstützt),
  nicht zu stiller, reihenfolge-ignorierender Verrechnung.

## 2. Root Cause — warum es heute bricht

Der Kernel baut Geometrie **nicht inkrementell**, sondern *global-deklarativ*:

1. `FeatureRegistry::regenerate()` (`crates/kernel/src/features.rs:925`) dekodiert alle
   Features in eine flache `Vec<DecodedFeature>`. Das **Boolean-`execute`
   (`features.rs:4131`) mutiert dabei *fremde* Features in-place** — es setzt `op` und
   `target_ids` seiner Tools.
2. `partition()` (`crates/kernel/src/body.rs:332`) gruppiert die komplette Liste nach
   `BoolOp` + `target_ids` + **AABB-Overlap**.
3. `weave_all()` (`body.rs:429`) verschweißt jede Partition; `rebuild_mesh_internal()`
   (`lib.rs:654`) packt sie zu Bodies, die der Renderer per WASM-Pointer liest.

Zwei Eigenschaften dieses Modells verletzen die Invariante:

- **(B1) Boolean mutiert Geschwister-Features.** Ein Feature greift in den dekodierten
  Zustand anderer Features ein, statt seine Operation lokal auszudrücken und auf den
  Snapshot anzuwenden. Ein Tool *vor* dem Boolean wird global umgeschrieben (egal an
  welcher Position); ein Tool *nach* dem Boolean existiert zur `execute`-Zeit noch nicht
  in `ctx.features` und kann gar nicht sauber behandelt werden → ambivalentes,
  positionsabhängiges Verhalten.
- **(B2) AABB-Overlap-Docking.** `Remove`/`Intersect`/untargeted-`Add` docken an *jeden*
  räumlich überlappenden Body — unabhängig von expliziter Referenz und Tree-Position
  („spooky action at a distance"). Reordering ändert nicht, *welche* Bodies betroffen
  sind, weil der Begriff „die Bodies, die an meiner Tree-Position existieren" im Modell
  nicht vorkommt.

**Fazit:** Kein Boolean-Bug, sondern ein Pipeline-Defizit. `partition`/`weave_all` ist ein
global-deklaratives Modell, wo ein sequenziell-inkrementelles gebraucht wird. Das erklärt
beide gemeldeten Symptome — Cone *nach* Boolean wird verrechnet **und** die Regression bei
Boolean *nach* Cone (in beiden Fällen entscheidet globales Grouping, nicht die Tree-Position).

## 3. Zielarchitektur — sequenziell-inkrementelle Auswertung

Ersetze das global-deklarative Modell durch einen **Left-Fold über den Feature-Tree**:

```
S(0) = ∅                                    // leerer Szenenzustand
für jedes Feature i in Tree-Reihenfolge (bis Rollback-Cursor):
    inputs = resolve(feature_i.refs, gegen S(i))   // NUR Snapshot S(i) sichtbar
    falls eine Referenz nicht in S(i) auflösbar → Feature-Error, S(i+1) = S(i)
    S(i+1) = apply(feature_i, inputs, S(i))         // konsumiert/erzeugt Bodies
S(n) = finaler Zustand → Renderer-Output
```

Kernpunkte:
- **Snapshot `S(i)` ist die einzige sichtbare Welt** für Feature *i*. Resolution
  (`qCreatedBy`, `qFace`, Tools, Targets) läuft ausschließlich gegen `S(i)`.
- **Explizite Referenzen sind die Wahrheit.** Kontakt/AABB darf nur eine *bewusste,
  dokumentierte* Regel sein (z.B. „neue, sich berührende Solids einer Union verschmelzen"),
  und auch die nur snapshot-skopiert. Kein implizites Cross-Tree-Docking mehr.
- **Konsum ist explizit.** Ein Boolean konsumiert seine Tool-Bodies aus `S(i)`
  (entfernt sie aus dem Zustand) und legt das Resultat ab — außer `keep_tools`.
- **Kein Feature mutiert ein anderes Feature.** Boolean drückt seine Operation lokal
  aus; die Wirkung entsteht beim `apply` auf `S(i)`, nicht durch In-place-Rewrite von
  `op`/`target_ids` fremder Features. (Entfernt B1.)

### Snapshot-Caching
Der bestehende `BrepCache` (`body.rs:462`) keyt bereits per Content-Hash einer
Feature-Slice. Wir keyen Snapshots deterministisch per Prefix `features[0..i]`:
`S(i)`-Weave wird einmal gebaut und sowohl für (a) die Resolution während Feature *i*
als auch (b) die UI-Auswahl beim Editieren von Feature *i* genutzt. Ersetzt den
LRU-Cache durch eine prefix-indizierte Struktur (`Vec<Snapshot>` parallel zur
Feature-Liste), invalidiert ab dem ersten geänderten Feature.

## 4. Phasen

Jede Phase ist einzeln shippbar und testbar. „Keine Backward Compatibility" gilt — wir
dürfen Partition-Modell, Boolean-`execute` und Persistenzformat brechen.

### Phase 0 — Reproduktion & Guard-Tests (Fundament)
- Failing Tests, die die Invariante festschreiben, *bevor* wir umbauen:
  - `T0.1` Cone *nach* Boolean als Tool → Boolean ignoriert ihn / Feature-Error,
    Cone bleibt eigenständiger Body.
  - `T0.2` (Regression) Boolean *nach* Cone, zweiter unbeteiligter Body mit AABB-Overlap →
    Boolean fasst nur explizit referenzierte Tools an, nicht den Überlappungs-Nachbarn.
  - `T0.3` Reorder eines Features hinter seine Referenz → Feature-Error.
- Repro-Projekt aus dem Ticket/Screenshot als `.fdraft`-Fixture ablegen.

### Phase 1 — Referenz-Integrität (Invariante + Enforcement)
Behebt die *Korrektheit* mit beschränktem Blast-Radius, ohne die Weave-Engine noch zu
ersetzen.
- **K1.1 Prefix-skopierte Resolution.** `Context` bekommt den aktuellen Feature-Index;
  alle Resolver (`resolve_topo`, `resolve_topo_owner`, `target_ids_from_queries`,
  Boolean `resolve_ids`, Face-Owner-Fallbacks) lösen nur gegen `features[0..i]` auf.
  Eine Referenz auf Index `≥ i` → `FeatureError` (gesammelt in `RegenErrors`).
- **K1.2 Boolean snapshot-treu.** Boolean-`execute` darf keine Features mit Index `≥ i`
  als Tool akzeptieren. (Übergangsweise bleibt der In-place-Rewrite, aber strikt auf den
  Prefix beschränkt — wird in Phase 2 entfernt.)
- **K1.3 Partition snapshot-treu.** `partition()` bleibt ein Left-Fold, aber
  AABB-Docking (B2) wird auf Bodies beschränkt, deren Seed-Feature-Index `< i` ist, und
  langfristig zugunsten expliziter Targets entschärft (Flag/Übergang in Phase 2).
- **UI1.4 Auswahl-Filter.** Neue Kernel-API `selectable_before(feature_id) -> [BodyRef/FaceRef]`
  (liefert die Topologie von `S(i)`). `MultiQueryWidget` (`web/src/widgets/widgets.tsx:995`)
  und der Viewport-Picker (`web/src/Viewport.tsx`, `elementQuery.ts`) filtern Auswahl und
  Hover gegen diese Liste. Beim Editieren von Feature *i* sind spätere Features nicht
  pickbar. Das ist die direkte Behebung von „Boolean bietet den Cone an".
- **UI1.5 Reorder-Guard.** `store.reorderFeature` (`web/src/store.ts`) / Drag-Drop im
  Outliner verbietet (oder markiert als Error) Reorders, die eine Referenz hinter ihren
  Nutzer schieben.

### Phase 2 — Snapshot-Evaluator (die saubere Engine)
Ersetzt B1/B2 vollständig.
- **K2.1 Zustands-Fold.** Neue Auswertung baut `S(i)` als explizite Body-Menge sequenziell
  auf. `apply(feature, S(i))` ersetzt das globale `partition()`-Grouping.
- **K2.2 Explizite Boolean-Semantik.** Boolean konsumiert Tools aus `S(i)`, erzeugt einen
  Resultat-Body; `keep_tools` steuert Konsum. Kein Geschwister-Mutieren mehr.
- **K2.3 Prefix-Snapshot-Cache** ersetzt den LRU-`BrepCache`; deterministisch, ab erster
  Änderung invalidiert; geteilt zwischen Regen, FieldSkript-`RegenScratch` und UI-Auswahl.
- **K2.4 Migration aller Call-Sites** von `weave_all`/`partition` (~30, Liste s. §5) auf
  die neue Snapshot-API. Pattern-/Sketch-Features, die heute Zwischengeometrie weben,
  ziehen `S(i)` aus dem Cache.

### Phase 3 — Output, ID-Stabilität, Aufräumen
- **K3.1 Body-Identität.** `seed_id()` (`body.rs:51`) bleibt stabil über den Fold
  (niedrigste Feature-ID der Partition). Verifizieren, dass Material-/Selektions-Lookups
  (`body_id`, `body_seed_feature_id`, `lib.rs:3019+`) unter dem neuen Modell stabil bleiben.
- **K3.2 Toten Code entfernen:** alte `partition`-AABB-Docking-Pfade, Boolean-In-place-
  Rewrite, LRU-`BrepCache`.
- **K3.3 Doku:** `docs/ARCHITECTURE.md` um das Snapshot-/Fold-Modell ergänzen.

## 5. Blast-Radius — Call-Sites von `partition`/`weave_all`

| Datei | Kontext | Zeilen |
|---|---|---|
| `body.rs` | kanonisches `weave_all`/`partition`, `BrepCache::ensure` | 332, 429, 508 |
| `lib.rs` | `rebuild_mesh_internal` (Hot-Path), Snapshot-BRep | 659, 718, 6906 |
| `features.rs` | `resolve_topo_owner`, Shell, viele Feature-`execute` | 381, 2367, 6158, 6638, 6665, 8552, 8624, 8773, 8894, 8961 |
| `sketch_features.rs` | Profil-/Revolve-Auflösung | 5151, 5209 |
| `script_runtime.rs` | FieldSkript `RegenScratch` live context | 402, 508 |
| `body.rs` (Tests) | ~25 Partition-Testfälle | 1265–1557 |

Hot-Path = `rebuild_mesh_internal`; Mid-Loop = Feature-`execute`; Script = `RegenScratch`.

## 6. Risiken & offene Fragen
- **Performance:** Sequenzieller Fold + Prefix-Cache statt einmaligem Global-Weave. Cache
  muss greifen, sonst O(n²) Weaves. → Prefix-Cache deterministisch, Invalidierung ab
  Änderungspunkt.
- **AABB-Auto-Merge:** Wird die „berührende neue Solids verschmelzen automatisch"-UX
  beibehalten? Falls ja, als bewusste, snapshot-skopierte Regel dokumentieren; falls nein,
  Union erfordert explizite Tools (sauberer, aber UX-Bruch). → **Entscheidung nötig.**
- **Pattern/Clone-IDs:** Hierarchische IDs (`u32::MAX`-Fallback, K8.0) im Fold prüfen.
- **FieldSkript:** `fd.q` muss gegen `S(i)` des Skript-Kontexts auflösen.

## 7. Definition of Done
- Phase-0-Tests grün; das Repro-Projekt verrechnet den Cone nicht mehr.
- Editieren eines Features bietet ausschließlich Topologie aus `S(i)` zur Auswahl.
- Vorwärts-Referenz / illegaler Reorder → sichtbarer Feature-Error, kein stiller Effekt.
- Reordering eines Features ändert das Ergebnis deterministisch gemäß neuer Position.
- `just test` grün; keine Regression an bestehenden Boolean-/Pattern-/Sketch-Tests.
