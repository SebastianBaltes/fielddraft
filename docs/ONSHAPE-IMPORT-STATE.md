# Onshape-Import — Fortschritts-Log

Arbeitslog zum Vier-Schritte-Plan (Query-System-Erweiterung + Import-Pipeline).
Plan-Kontext: docs/ONSHAPE-IMPORT-PLAN.md. Testmodell: „Kabelbox" (7 Sketches
auf 7 verschiedenen Ebenen), live unter den IDs in web/.onshape.local.json;
Roh-Features + dekodierte Queries als Fixture in web/src/onshape/fixtures/.

## Empirische Basis (2026-07-13, Live-API)

Onshapes queryString ist `query=qCompressed(1.0,"<payload>",id);` mit zwei
Payload-Kodierungen: Klartext-Dictionary (`%B5$…`) und zlib/base64
(`&<hexlen>$eJ…`). Dekodierte Struktur pro Sketch-Ebene der Kabelbox:

- Sketch 1, 3: Default-Ebene (`operationId=[Topplane]`, queryType DUMMY).
- Sketch 2, 6: `CAP_FACE` des Extrudes mit `isStart=T/F` — Rolle + Seite.
- Sketch 5: `SWEPT_FACE` des Extrudes, Herkunft = Sketch-Entity
  (`sketchEntityId="<wireId>.left"`, ORIGINAL_DEPENDENCY).
- Sketch 4, 8: `SWEPT_FACE`, disambiguiert per TOPOLOGY-Nachbarschaft
  (`SWEPT_EDGE` → TRUE_DEPENDENCY → `BLEND_FACE` des Fillets) und
  Boolean-`SPLIT` (Herkunfts-Operanden beider Extrudes).

Die drei Kernel-Lücken des Auftrags sind damit bestätigt: Rollen (Schritt 1),
stabile Herkunfts-Entity-Subs (Schritt 2), Schnitt/Nachbarschafts-Filter
(Schritt 3).

## Schritt 1 — FaceRole + qCreatedBy(op, ROLE) — ERLEDIGT (70bc032)

- `FaceRole` = WALL | CAP_TOP | CAP_BOTTOM | INNER_WALL | BLEND in
  queries.rs; `face_role(&Face)` als einzige Autorität (Blend via
  `HalfSpace::Blend`-Surface VOR den Sub-Bits geprüft — Fillet mintet sub 0).
- Parser/Display/Resolver + Web-Spiegel (queryAcceptedByFilter → FACE).
- Hash-neutral, keine Migration. b2c0472-Wiederbelebung geprüft: der
  entfernte Code war ein Feature-Level-Existenz-Evaluator, für die
  element-level Rollen-/Schnitt-Auflösung unbrauchbar — nicht wiederbelebt.

## Schritt 2 — Stabile Sketch-Entity-Ordinals — UMGESETZT

Implementiert: `Primitive::{Extrude,Revolve,Helix}.wall_subs` (protocol) →
`CsgNode`/`BoundedCsg`-Leaves (`Arc<Vec<u32>>`) → Konsumenten mappen über
`csg::map_wall_sub(ws, g)`: Contributor-Fns (extrude/revolve/helix ±retained),
Retained-Closures (csg+bvh, je 3), `branch_pairs::push_sketch_walls`,
`sketch_surfaces`-Collectors, `surface_registry` (Profile-Carriers +
Shell-Nachbarlogik jetzt positional-über-Tabelle statt `g±1`),
`wireframes` (Revolve/Helix-Corner-Kanten), `edge_walker::extrude_rim_corner`
(invers Tabelle→Position). Tabellenbau: `sketch/lower.rs`
(`build_sketch2d_traced` + `wall_sub_table`), Allokations-Autorität =
Registry-Replay (`replay_allocations`); `resolve_profile_query` liefert die
Trace als drittes Element; Revolve/Helix transformieren die Trace mit M mit
(`transform_traced_prims`, Rectangle→Polygon-Gruppen-Reorder [b,r,t,l]);
Revolve-Achsen-Strip über strip-aware Matching. Kein Hot-Path-Umbau: interne
Nummerierung bleibt positionell, Übersetzung nur an der Branch-Grenze.

Repräsentanten konkret: Line/Arc/Circle/Spline/Ellipse = Curve-Entity-ID
(min über G1-Gruppe); Slot = erste Punkt-Entity des Calls (voll G1 = 1 Wand);
Polygon/RegularPolygon = Vertex-Punkt-ID `first+k` je Kante k. Fallback
(ganze Tabelle None): sketch_path (SVG, nur Punkt-Entities), sketch_mirror
(Alias), Replay-Fehler, Überlauf ≥ 0x7FE.

Tests: 6 Unit (lower) + 3 E2E (features_tests): Wall-Subs = Line-Entity-IDs
im gewobenen B-Rep, qCreatedBy(op, ROLE) auf echtem Sweep, Heal-Test über
Entity-ID-Shift (Migrationsgarantie v1→v2). `NAMING_SCHEMA_VERSION = 2` in
brep/naming.rs dokumentiert.

### Entscheidungen

- **Repräsentant:** Sub-ID einer Mantelfläche = MIN der Solver-Curve-Entity-IDs
  (Line/Arc/Circle/Spline/…) über die Segmente ihrer G1-Randgruppe. Damit
  bleibt die ID stabil, wenn VOR dem Profil liegende Sub-Calls eingefügt/
  entfernt werden, und deterministisch, wenn eine G1-Gruppe mehrere Kanten
  umfasst (Line+Tangent-Arc-Kette, Slot).
- **Allokations-Autorität:** Replay der `SketchToolRegistry` über die
  Sub-Calls in einen Scratch-`SketchState`; pro Call werden die neu
  allozierten Curve-Entities abgelesen. KEIN vierter Lockstep — die
  `apply()`-Implementierungen selbst sind die Quelle.
- **Träger:** `Primitive::{Extrude,Revolve,Helix}.wall_subs: Option<Vec<u32>>`
  (serde default) → `BoundedSketch2d.wall_subs` → Konsumenten mappen
  positional→stabil über `wall_sub(g)`. Interne Nummerierung (Offsets im
  Sketch2d-Baum, sdf_with_edges) bleibt UNVERÄNDERT positionell — die
  Übersetzung passiert ausschließlich an der Branch-Grenze.
- **Berechnung** nach der Profil-Restriktion (qRegion!) im Feature-execute:
  instrumentiertes Lowering liefert (Prim, lokale Wall-IDs) in Baum-Reihenfolge;
  der (ggf. restrizierte) Baum wird per in-order strukturellem Matching
  dagegen gelegt; lokale Tabellen konkateniert = globale Tabelle.
- **Fallback = None (positionell, Status quo)** wenn irgendein Prim keine
  IDs liefert: sketch_path (SVG-Import, nur Punkt-Entities), sketch_mirror
  (Alias-Kollision der Kopie), ID-Überlauf ≥ 0x7FE. Kein stilles Falschbinden.
- **Nachbar-Arithmetik:** surface_registry.rs (RevolvedProfile
  extend_start/end via g±1) bleibt positionell und mappt erst beim Insert.

### Migration

- Hashes ändern sich nur für Mantelflächen von Sketch-Sweeps mit Tabelle.
- Persistierte Referenzen (`qFace(hash @ fingerprint)`) heilen über den
  N2/N3-Resolver (Fingerprint-Override); Migrationstest deckt das ab.
- `NAMING_SCHEMA_VERSION`-Konstante in brep/naming.rs eingeführt (Doku-Marke,
  Branch-Subs werden nicht persistiert — nur Query-Hashes, und die heilen).

## Schritt 3 — Query-AST — UMGESETZT

Neue Formen (queries.rs, klammer-bewusster rekursiver Parser
`split_top_level`, Display = kanonische Wire-Form, beliebig schachtelbar):
- `qWall(op, entity)` — Mantelfläche von Sweep `op` mit stabilem
  Entity-Sub `entity` (Rolle WALL erzwungen, Innenwände ausgeschlossen).
- `qIntersection(a, b)` — Mengen-Schnitt per id_hash.
- `qAdjacentTo(a, b)` — Flächen aus a, die eine B-Rep-Kante (Loop-Kanten)
  mit einer Fläche aus b teilen — FieldDrafts Gegenstück zu Onshapes
  TOPOLOGY-/SPLIT-Disambiguierung.
b2c0472 (Feature-Level-Evaluator) blieb unbrauchbar — element-level neu.
Web-Spiegel: alle drei Formen FACE-level in queryAcceptedByFilter.
Tests: Parse/Roundtrip (inkl. Nesting), Wall-by-Sub, Schnitt,
Blend-Nachbarschafts-Pick am synthetischen 3-Flächen-Body.

## Schritt 4 — Übersetzer + Pipeline + UI — IN ARBEIT

Scope-Erkenntnis: Die Kabelbox-DoD („jeder Sketch auf korrekter Fläche,
Basiskörper ändern → Ebenen wandern mit") erfordert, dass die referenzierten
KÖRPER existieren — der Import muss also neben den 7 Sketches auch die
Kabelbox-Features minimal mitbringen (Extrude new/add/remove, Shell,
Fillet, Boolean-SPLIT-Scope), sonst lösen die Ebenen-Queries ins Leere.
Der volle generische Feature-Import (alle End-Conditions etc.) bleibt
Folgearbeit.

Onshape-queryString-Format (empirisch): `query=qCompressed(1.0,"…",id);`
Payload `%…` = Klartext-BTON-Dictionary, `&<hexlen>$<b64>` = zlib.
BTON-Tokens: `M<n>` Map, `A<n>` Array, `S<len>$s` String,
`S<l1>.<l2>$s1s2` zweiteilig, `S-<ref>.<l>$suffix` Ref+Suffix,
`B<len>$Enum` + Wert-String, `R<hex>` String-Backref, `C<hex>` Klasse,
`D<±hex>` Zahl, `T`/`F` Bool. Relevante Felder: queryType (DUMMY /
SKETCH_ENTITY / CAP_FACE / SWEPT_FACE / BLEND_FACE / SPLIT / INTERSECT),
operationId (`<featureId>op<Kind>`), isStart, sketchEntityId
(`<wireId>.<entity>`), derivedFrom-Ketten, disambiguationData
(ORIGINAL_DEPENDENCY originals / TOPOLOGY entities).

## Offene Punkte / Lücken

- Übersetzer-Lücken (nach Pipeline schließen): Fillet-Arcs im Sketch,
  Bemaßungs-Constraints (DISTANCE/ANGLE/RADIUS/DIAMETER), Splines.
- Extrude/Revolve-Feature-Import selbst (Stufe 3 des Plans) — hier nur
  Sketches; Ebenen-Queries sind der Proof of Concept fürs Query-Mapping.
- OAuth statt Operator-API-Key (Konzept in server/onshape.go skizziert).
