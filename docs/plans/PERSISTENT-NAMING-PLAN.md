# Persistentes Naming — Provenienz + Healing statt kippender Räumlichkeit

Stand: 2026-07-07. Plan-SSOT für den Umbau der Face-/Edge-/Vertex-Identität
(ARCHITECTURE-ROADMAP Punkt 4). Auslöser: „face not found" nach harmlosen
Edits; schlimmer noch der stille Fehlbind-Fall (s. Befund B3).

## Befund (gegen den Code verifiziert, 2026-07-07)

Identität heute: `id_hash = fnv1a(sortierte Branch-u32s ++ spatial_index)`
(`naming.rs::hash_signature`). Die konstruktive Hälfte ist solide: `BranchId`
= Feature-UUID (20 bit) + Sub-Rolle (12 bit) ist geometrieunabhängig; reine
Parameter-Edits erhalten die Signaturen byte-identisch. Pattern-Instanzen
bekommen hierarchische Kind-FeatureIds → distinkte Branches; Koinzidenz
erzeugt wegen der No-Face-Merging-Policy ebenfalls keine geteilten
Identitäten. **Mehrdeutigkeit entsteht praktisch nur bei Multi-Region-
Branches (F0-1: ein Branch zerfällt in mehrere Grenzregionen) und echter
Symmetrie.**

Die Bruchstellen:

- **B1 — Vergabe kippt:** `spatial_indices` sortiert gleich-signierte
  Gruppen per Welt-Zentroid-Sweep X→Y→Z mit **absolutem** ε=1e-9 und
  Arena-Index als Tie-Break. µm-Verschiebungen und Weave-Reihenfolge
  (Kanten-Membership-Ordnung in `materialize_faces`) kippen die Sortierung.
- **B2 — Auflösung heilt nie:** `resolve_query_topo` (queries.rs) und alle
  `context_resolve.rs`-Pfade matchen ausschließlich exakt auf `id_hash`.
  Das vorhandene Stufe-2-Healing (`naming::resolve`, Fingerprints,
  DL-34-Schwellen) wird **nur von Tests** aufgerufen.
- **B3 — Stilles Fehlbinden:** Kippt der Sweep zwischen zwei Faces derselben
  Signatur, existieren beide alten Hashes weiter — sie zeigen nur auf die
  **vertauschten** Entities. Exakter Hash-Match bindet dann kommentarlos an
  die falsche Face. Das ist gefährlicher als „face not found".
- **B4 — Referenzen tragen keine Vergleichsdaten:** Features persistieren
  `qFace(<id_hash>)`-Strings (MultiQuery). Der Fingerprint (Centroid, Size,
  Tangente) aus `TopoRef` wird **nicht** persistiert (DL-35 vertagt) — im
  Produktionspfad hätte Healing gar kein Alt-Wissen zum Vergleichen.

## Leitentscheidungen

1. **Zwei Schichten, sauber getrennt.**
   - *Schicht 1 (Weave-deterministisch):* `id_hash` bleibt eine reine
     Funktion des gewobenen B-Rep. Der Toggle-Invarianz-Kontrakt
     (Seed-Route ↔ Octree-Route byte-identisch, `assert_gate`) bleibt
     unangetastet. Kein History-/Generationen-Zustand in der ID-Berechnung.
   - *Schicht 2 (Dokument-persistent):* Die **Referenz** (im Feature-Param)
     trägt neben dem Hash den Fingerprint und heilt beim Auflösen. Identität
     kippt selten; Referenzen überleben das Kippen.
2. **Provenienz zuerst, Raum zuletzt.** Die Basis-Signatur bleibt
   Branch-Provenienz. Der räumliche Disambiguator bleibt (Multi-Region
   braucht ihn), wird aber skalenrelativ und surface-frame-bewusst — Kippen
   nur noch bei Edits in der Größenordnung der Region-Abstände, nicht bei µm.
3. **Kein stilles Fehlbinden — je Kandidat ein Score.** `naming::resolve`
   bewertet ALLE Kandidaten einheitlich (Fingerprint-Ähnlichkeit +
   Hash-Bonus). Ein Hash-Treffer mit schlechtem Fingerprint verliert gegen
   einen Nicht-Treffer mit exzellentem Fingerprint → `Healed` statt falsch
   gebunden. Ohne persistierten Fingerprint (alte/kurze Referenzform) bleibt
   exakter Hash-Match das einzige Kriterium — wie heute, nur ehrlich
   dokumentiert.
4. **Selbstheilende Dokumente.** Löst eine Referenz per Healing auf, wird
   die kanonische Form (frischer Hash + frischer Fingerprint) in den
   Feature-Param zurückgeschrieben. Frühe Dev-Phase, keine Kompat-Pflicht:
   aggressiv heilen, klar loggen.
5. **Koinzidenz-Klassen: geprüft, für Identität NICHT nötig.** Die
   No-Face-Merging-Policy hält Identität pro Branch stabil; die Registry
   wirkt bereits in der Weave-Kanonikalisierung. Die Roadmap-Vokabel
   („nicht überdeckter Teil von Branch X in Klasse K") wird durch
   Branch + Region-Disambiguator + Fingerprint abgedeckt. Sollte
   Face-Merging je kommen, dockt der Klassen-Repräsentant in
   `base_signature_hash` an — Andockstelle dokumentiert, nicht gebaut.

## Phasen (je Phase: Tests grün → Commit → Push)

### N0 — Rote Handle-Resolve-Gates (Testmatrix zuerst)

Neue Testfamilie `naming_gate` (analog `classification_gate`), Szenarien
mit `#[ignore]`-Dokumentation wo heute rot:

- `gate_swap_no_misbind`: Zwei-Region-Branch (kreuzende Boxen / U-Profil),
  µm-Verschiebung kippt den Sweep → Referenz bindet an die geometrisch
  selbe Region (`Healed` ok), NIE still an die andere. Heute rot (B3).
- `gate_region_split_heals`: Wand referenziert, kreuzendes Feature spaltet
  sie in zwei Regionen → Auflösung `Healed` auf das Stück mit dem alten
  Centroid oder ehrlich `Ambiguous` — nicht `Missing`, nicht falsch. Heute
  rot (B2).
- `gate_ring_face_identity`: Durchgangsbohrung in referenzierte Face →
  Identität bleibt exakt (Inner-Loops sind nicht Teil der Signatur;
  Single-Member-Gruppe behält Index 0). Erwartet grün — festnageln.
- `gate_resize_exact`: Bestands-Verhalten (Resize → Hash stabil) aus
  naming.rs-Tests in die Gate-Familie spiegeln.
- Toggle-Invarianz: alle Szenarien über `assert_gate`-Mechanik (Seed ↔
  Octree identische Handles) laufen lassen.

### N1 — Vergabe robuster (Schicht 1, naming.rs)

- ε der Zentroid-Ordnung skalenrelativ (`brep::tol`, B-Rep-AABB-Diagonale)
  statt absolut 1e-9.
- Multi-Member-Ordnung: primär quantisierte relative Größe, dann
  Zentroid-Sweep im **Surface-Frame** (Ebene: 2D-Koordinaten, Zylinder:
  (θ,z)) statt Welt-XYZ, wo die Fläche eines hergibt; Welt-Sweep als
  Fallback. Arena-Tie-Break nur noch für echte Symmetrie, als solcher
  dokumentiert.
- `naming::resolve` auf Ein-Score-Modell (Leitentscheidung 3) umbauen;
  DL-34-Schwellen bleiben.

### N2 — Fingerprint-tragende Referenzen (DL-35 einlösen)

- Query-String-Form erweitert: `qFace(<hash> @ <cx> <cy> <cz> ; <size>
  [; <tx> <ty> <tz>])` (Whitespace tolerant; Kurzform `qFace(<hash>)`
  bleibt gültig = exakt-only). Parser (`parse_query`) + `Display`
  round-trippen die Langform; `MultiQuery::queries()` splittet weiterhin
  nur auf Top-Level-Kommas → keine Kommas im fp-Block, Trenner sind
  Spaces/`;`/`@`.
- Pick-JSON (`api/brep.rs`, `script_runtime.rs`) liefert den Fingerprint
  mit; `web/src/elementQuery.ts` baut die Langform (Element-Query-Contract
  wird bewusst erweitert — Konsumenten behandeln Strings opak).
- `fd.q.face/edge/vertex` reichen die Langform durch (Handles tragen
  `idHash` + `fingerprint`).

### N3 — Healing im Produktionspfad

- `resolve_query_topo`: Face/Edge/Vertex-Arme nutzen bei vorhandenem
  Fingerprint das Ein-Score-Resolve; Rückgabe transportiert den
  `ResolutionState`. `Healed` → Ergebnis + Heal-Notiz; `Ambiguous`/
  `Missing` → präziser `FeatureError` mit Kandidaten-Fingerprints.
- `context_resolve.rs`-Pfade (extrude faces, up-to-face, thicken, shell,
  fillet/chamfer, vertex) auf die neue Auflösung umstellen — eine
  gemeinsame Hilfsfunktion statt sieben Hash-Scan-Kopien.

### N4 — Selbstheilung + Sichtbarkeit

- Regeneration schreibt geheilte Referenzen kanonisch in die Params zurück
  (Document dirty, Undo-fähig); Heal-Ereignisse landen im Build-Report
  (Frontend: dezenter Hinweis statt Silent-Magie).
- Playwright-E2E: Referenz überlebt Sketch-Edit + Region-Split sichtbar.
- Release-Note + Versions-Bump je Landung; ARCHITECTURE-ROADMAP Punkt 4
  auf „umgesetzt" nachführen.

## Nicht-Ziele

- Kein Topology-DAG / Generationen-Timeline / Semantic Decay (Diskussions-
  Doc bleibt Ideenspeicher; widerspräche Toggle-Invarianz in Schicht 1).
- Kein Face-Merging, keine Änderung der Weave-Topologie.
- `evaluate_query`/`QueryCache` (Feature-Level-Ebene) bleiben unangetastet;
  ihr Ausbau ist ein separates Tail-Item.

## Risiken

- **Fingerprint-Drift bei legitimen Groß-Edits:** Score-Schwellen (DL-34)
  entscheiden Healed vs. Ambiguous; Gate-Tests dimensionieren sie an realen
  Szenarien, nicht an Wunschwerten.
- **Format-Erweiterung der Query-Strings:** alle Parser-Konsumenten
  (Kernel, Web-Filter `queryAcceptedByFilter`, FieldSkript) müssen die
  Langform tolerieren — N2 landet nur mit rundum grünen Suites.
- **Write-Back und Undo:** Heilung während Regeneration darf keine
  Undo-Schleifen erzeugen — Write-Back als Teil des regulären
  Param-Update-Pfads, nicht als Seiteneffekt im Weave.
