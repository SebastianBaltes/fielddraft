# Namen für abgeleitete Geometrie — Entwurf (#424 / #429)

**Frage:** Wie bekommt Geometrie, die erst durch eine Operation entsteht — die
Kante nach einer Boolean, die Fläche einer Muster-Kopie, der Rand einer
Verrundung, das Bruchstück einer geteilten Fläche — einen **stabilen Namen**?

Heute kann FieldDraft nur benennen, was sich auf eine **Sketch-Entity** oder
eine **Deckfläche** zurückführen lässt. Alles andere lehnt der Onshape-Übersetzer
mit Klartext-Meldung ab (und das ist richtig so — siehe §1).

SSOT der Lückenklassen: `docs/ONSHAPE-IMPORT-GAP-MATRIX.md`.
Bestehende Naming-Infrastruktur: `crates/kernel/src/brep/naming.rs`,
`crates/kernel/src/queries.rs`, `crates/kernel/src/csg.rs` (`BranchId`).

---

## 1. Warum nicht raten

Vor #425 hat der Kanten-Dekoder eine Muster-Kopie ignoriert und die
entsprechende Kante des **Originals** verrundet. In #414 trugen `Fillet 1` und
`Fillet 2` dadurch die identische aufgelöste Query: zwei Verrundungen auf
derselben Kante, eine davon frei erfunden. Eine Kante ohne Entsprechung wird
deshalb **abgelehnt**, nicht auf eine ähnlich aussehende gelegt. Jeder Vorschlag
unten muss diese Eigenschaft erhalten.

---

## 2. Wo FieldDraft steht — gemessen, nicht vermutet

FieldDrafts Namensschema ist **generativ**: ein Name ist kein vergebener
Bezeichner, sondern die Signatur der beteiligten Halbräume.

| Entität | Name |
| --- | --- |
| Fläche | `hash(branch_id, spatial_index)` |
| Kante | `hash(sort(branch_a, branch_b), spatial_index)` |
| Ecke | `hash(sort(branches…), spatial_index)` |

`BranchId` = 20 Bit Feature-Id + 12 Bit Sub-Id. Die Sub-Id ist bei einer
Sketch-Wand die **Sketch-Entity-Id** der erzeugenden Kurve (Schema v2), bei
einer Deckfläche eine reservierte Konstante, beim Shell-Innenraum ein Flag-Bit.

Das ist strukturell eine starke Position: für eine reine Parameteränderung sind
die Signaturen byteweise identisch, ganz ohne Heuristik. Der Preis: **wer keine
Branch hat, hat keinen Namen.**

### 2.1 Was heute schon benennbar ist (gemessen 2026-07-27)

Zwei Quader, per Boolean vereinigt (`crates/kernel/src/lib_tests.rs`-Sonde):

    b0 E12 sig=[1:4 2:0] hash=14516246793604892097
    qCommonEdge(qCreatedBy(1), qCreatedBy(2)) -> 4 Kanten
    qEdgeBetween(1, 2)                        -> dieselben 4 Kanten

**Die Verschmelzungskante hat bereits einen stabilen Namen.** Onshapes `MERGE`
ist damit keine Kernel-Lücke, sondern eine Übersetzer-Lücke — der Dekoder muss
die zwei Flächen benennen, deren Schnitt gemeint ist.

### 2.2 Was heute keinen Namen hat

**Muster-Kopien — der härteste Befund.** *(Behoben in v0.39.2, #431 — siehe
Schritt 1 unten. Der Befund bleibt als Messprotokoll stehen.)* Ein Quader,
`linear_pattern` mit Anzahl 3:

    b0 f3 feat=1 sub=0 branch=4096 hash=6025886176426920981
    b1 f3 feat=1 sub=0 branch=4096 hash=6025886176426920981
    b2 f3 feat=1 sub=0 branch=4096 hash=6025886176426920981
    qCreatedBy(1) -> 18 Elemente, davon 6 verschiedene id_hash
    qCreatedBy(2) -> 0   (das Muster-Feature selbst besitzt nichts)

Die drei Kopien tragen **identische Branch-Ids und identische id_hashes**. Der
Grund steht in `BranchId::from_feature`: es ruft `FeatureId::as_u32_or_root()`,
und das faltet die hierarchische Klon-Id `P.«src»_i` auf ihren
**Wurzel-Vorfahren** zurück. Die Instanz-Nummer `i` fällt weg. `spatial_index`
rettet hier nichts: er wird **pro B-Rep** vergeben, und jede Kopie ist ein
eigener Körper — also überall 0.

Das ist nicht nur eine fehlende Fähigkeit, sondern eine **Verletzung der
Eindeutigkeitszusage von `qFace(hash)`**: derselbe Hash bezeichnet drei
verschiedene Flächen. Genau das hat in #414 die erfundene Verrundung erzeugt.

Nachgemessen wurde außerdem, **warum** die Kopien nicht auseinanderzuhalten
waren: das B-Rep eines Körpers wird **body-LOKAL** gewoben (`Body::ensure_brep`
teilt die `placement` heraus, damit ein reiner Move den Weave-Cache nicht
invalidiert). Die drei Kopien sind damit byteweise identisch — auch ihre
Fingerprint-Schwerpunkte (alle drei meldeten `[-10, 0, 0]` für dieselbe Wand).
Nur die `placement` unterscheidet sie. Jede Disambiguierung muss also durch die
Weltlage gehen; die Geometrie im B-Rep gibt nichts her.

**Verrundungsflächen.** `BlendProvenance.branch` ist als „Fillet-Feature-Id,
Sub 0" definiert, `FaceRole::Blend` existiert, `qCreatedBy(f, BLEND)` ist
parsebar. Gemessen an einem Absatz mit Verrundung auf einer Nahtkante liefert
`qCreatedBy(fillet, BLEND)` 0. *(Der Schluss daraus — „die Fläche fehlt" —
war falsch; Schritt 2 unten hat nachgemessen: die Fläche ist da, ihr fehlt die
Rolle.)*

**Koplanare Verschmelzung.** Zwei bündig aneinanderstoßende Körper behalten in
FieldDraft **zwei** Flächen (`feat=1 sub=4` und `feat=2 sub=0`); Onshape faltet
sie zu **einer**, die es `MERGE(a, b)` nennt. Eine Onshape-Fläche entspricht
hier also einer *Menge* von FieldDraft-Flächen.

**Geteilte Flächen.** Zerlegt eine Boolean eine Wand in mehrere Stücke, tragen
alle dieselbe Branch; unterschieden werden sie nur durch `spatial_index` —
eine **Sortierung nach Schwerpunkt**, die sich verschiebt, sobald sich die
Geometrie verschiebt.

### 2.3 Zahlen

`#419`-Kanten-Angaben nach Onshapes Wurzel-`queryType` (aus #425, unverändert):

| Form | Anzahl | FieldDraft heute |
| --- | ---: | --- |
| `INTERSECT` mit unbenannter Fläche | 61 | eine der Flächen hat keinen Namen |
| `COPY` | 22 | **kein Name** — Kopien kollabieren |
| `MERGE` | 20 | Kernel **kann** es (`qCommonEdge`), Übersetzer nicht |
| `BLEND_EDGE` / `BLEND_FACE` | 14 | Name entworfen, Fläche fehlt |
| `SPLIT` | 4 | nur `spatial_index` — positionell |

---

## 3. Was andere Kernel tun

Die Literatur trennt zwei Familien; die Übersicht von Farjana & Han (2018)
sortiert sie so:

- **Historie + Namensabgleich** (Kripac 1997; Chen/Hoffmann „generic naming",
  1995/96; Marcheix). Jede Entität bekommt einen Namen aus dem erzeugenden
  Feature, und ein **Protokoll von Split-/Merge-Ereignissen** schreibt fort,
  was aus ihr geworden ist. Beim Regenerieren wird der alte Name gegen das neue
  Modell gematcht, Mehrdeutigkeit über die **Nachbarschaft** aufgelöst.
  Raghothama & Shapiro haben formal gezeigt, dass das im Allgemeinen nicht
  lösbar ist — die Verfahren sind Heuristiken mit gutem Erwartungswert.
- **Zellmodell** (Bidarra & Bronsvoort). Eine Boolean **löscht nichts**: das
  Modell bleibt eine nicht-mannigfaltige Zellzerlegung, in der jede Fläche
  jedes Features als Zellwand erhalten bleibt und nur als „innen/außen"
  umklassifiziert wird. Der Name eines Features überlebt damit jede spätere
  Operation, weil die Zelle überlebt.

FieldDraft steht **näher am Zellmodell als an der Historie**: der
CSG-/Halbraum-Baum ist die Zellzerlegung, und `BranchId` ist der Zellname. Das
ist der Grund, warum die Verschmelzungskante schon heute benennbar ist (§2.1) —
und die Richtschnur für alles Folgende: **Namen werden nicht protokolliert,
sie werden aus der Erzeugung abgeleitet.** Ein Split-/Merge-Log wäre ein
Fremdkörper.

---

## 4. Entwurf

Drei Ergänzungen, alle innerhalb der bestehenden generativen Regel. Keine
Heuristik, kein Historien-Log, keine Geometrie-Ähnlichkeit.

```mermaid
flowchart TD
    A["Was für Geometrie ist gemeint?"] --> B{"entsteht sie aus<br/>einem Sketch-Segment<br/>oder einer Deckfläche?"}
    B -- ja --> C["qWall / qCreatedBy(op, ROLE)<br/>— vorhanden"]
    B -- nein --> D{"ist sie eine<br/>Muster-Kopie?"}
    D -- ja --> E["E1: Instanz-Ordinal<br/>in der BranchId"]
    D -- nein --> F{"ist sie eine<br/>Blend-Fläche?"}
    F -- ja --> G["E2: Blend-Träger als<br/>echte B-Rep-Fläche"]
    F -- nein --> H["E3: Kombinatoren —<br/>qUnion, qAdjacentTo,<br/>qCommonEdge"]
```

### E1 — Muster-Instanzen bekommen ein Ordinal in der Branch

`BranchId` ist 20 Bit Feature + 12 Bit Sub. Eine Muster-Kopie ist **dieselbe
Sub-Struktur an einer anderen Stelle der Feature-Hierarchie** — also gehört das
Instanz-Ordinal in den **Feature-Teil**, nicht in den Sub-Teil (der Sub-Teil
trägt die Sketch-Entity und muss das behalten).

Vorschlag: `BranchId` wird von `u32` auf `u64` verbreitert —
`feature (20) | instance (12) | sub (12) | reserve (20)`. `instance = 0` ist das
Original; die *i*-te Kopie eines Musters trägt `instance = i`. Die Zuweisung ist
deterministisch aus der Klon-Id (`FeatureId::child("«src»_i")`), also aus dem
Modell und nicht aus der Geometrie.

Query-Form: `qInstance(pattern_id, i, <sub-query>)` — „dasselbe wie
`<sub-query>`, aber an der *i*-ten Kopie von `pattern_id`". Onshapes `COPY`
trägt genau diese beiden Angaben.

Das ist ein **Breaking Change am Wire-Format** (`branchId` in
`bodyBrepFaces`, alle persistierten `qFace(hash)`). Das ist erwünscht: es
repariert eine Eindeutigkeitszusage, die heute nachweislich verletzt ist (§2.2).

**Vorleistung, unabhängig davon nützlich — erledigt in v0.39.2 (#431):**
`spatial_index` **körperübergreifend** vergeben statt pro B-Rep. Dann
kollidieren die Kopien nicht mehr still — sie werden nach Schwerpunkt
durchnumeriert und `qFace(hash)` ist wieder eindeutig. Das ist aber ein
*positioneller* Name; er überlebt keine Umsortierung der Kopien (Richtung
umdrehen, Abstand negieren). Als Übergang tauglich, als Ziel nicht — das Ziel
bleibt E1.

### E2 — Der Blend-Träger wird eine echte B-Rep-Fläche

> **Nachtrag nach Schritt 2 (unten):** Der hier formulierte Verdacht ist
> widerlegt. Beim geraden Absatz gibt es **keinen** `MaskedBlend`, also auch
> kein `register_blend_carrier` und kein scheiterndes Lookup — der harte Cutter
> webt die Verrundungsfläche als gewöhnlichen Zylindermantel unter der
> Fillet-Feature-Id. E2 heißt damit nicht mehr „Fläche bauen", sondern **„der
> vorhandenen Fläche ihre Rolle geben"**; die Query-Formen unten bleiben
> unverändert richtig.

`BlendProvenance` trägt für den smin-Pfad bereits alles Nötige: `branch`
(Fillet-Feature, Sub 0), `wall_a`, `wall_b` — dort funktioniert der Träger
(`fillet_blend_weaves_as_its_own_analytic_face`). Steht die Fläche auf beiden
Pfaden unter einer Rolle, folgt der Rest ohne neues Vokabular:

- `qCreatedBy(fillet, BLEND)` — die Verrundungsfläche selbst (Onshape:
  `BLEND_FACE`).
- `qCommonEdge(qCreatedBy(fillet, BLEND), <nachbar>)` — ihr Rand (Onshape:
  `BLEND_EDGE`).

### E3 — Kombinatoren statt neuer Basisnamen

Für `MERGE` und `SPLIT` braucht es **keinen** neuen Grundnamen, sondern die
Fähigkeit, mehrere vorhandene zu kombinieren:

- **`MERGE` (Fläche):** Onshapes verschmolzene Fläche ist die Vereinigung
  zweier FieldDraft-Flächen. Das Query-Vokabular hat `qIntersection`, aber
  **keine Vereinigung** — `qUnion(a, b)` fehlt und ist der ganze Bedarf.
- **`MERGE` (Kante):** ist `qCommonEdge(a, b)` und funktioniert bereits.
- **`SPLIT`:** `qAdjacentTo(a, b)` ist genau dafür dokumentiert („nach einem
  Boolean-Split das richtige Bruchstück über die Nachbarschaft wählen") und
  vorhanden. Was fehlt, ist der Übersetzer-Zweig, der Onshapes
  `SPLIT`-Disambiguierung darauf abbildet.
- **`INTERSECT` mit unbenannter Fläche (61×):** löst sich zum großen Teil von
  selbst auf, sobald E1/E2 stehen — die „unbenannte" Fläche ist in der Regel
  eine Muster-Kopie oder eine Blend-Fläche.

### Was ausdrücklich NICHT vorgeschlagen wird

- **Kein Split-/Merge-Protokoll.** Es wäre ein zweiter, konkurrierender
  Wahrheitsträger neben dem CSG-Baum — genau die Doppelbuchführung, an der die
  Historien-Verfahren scheitern.
- **Keine geometrische Ähnlichkeitssuche als *Erstauflösung*.** Der
  Fingerprint in `TopoRef` bleibt, was er ist: Heilung, wenn ein Hash nach
  einer Änderung nicht mehr trifft — nicht das Mittel, um überhaupt erst einen
  Namen zu erfinden.
- **Kein Aufweichen der Ablehnungen.** Was nicht abbildbar ist, wird benannt
  abgelehnt (§1).

---

## 5. Reihenfolge

| Schritt | Inhalt | Voraussetzung |
| --- | --- | --- |
| 0 | **Erledigt (v0.39.1):** Mehrfach-Region-Profile bekommen die Kontur ihrer Vereinigung; geschlossene Konturen starten am G1-Knick | — |
| 1 | **Erledigt (v0.39.2, #431):** `spatial_index` körperübergreifend — `qFace(hash)` wieder eindeutig | — |
| 2 | **Erledigt (2026-07-28):** Messen, warum der Blend-Träger keine Fläche wird — Antwort: er wird gar nicht gebaut, und die Fläche fehlt nicht (§ „Schritt 2" unten) | — |
| 3 | **Kernel-Teil erledigt (v0.41.0):** E2 — Rolle für die Cutter-Verrundungs-/Fasenfläche (§ „Schritt 3" unten); offen bleibt der Übersetzer-Zweig `BLEND_EDGE` | 2 |
| 4 | `qUnion(a, b)` + Übersetzer-Zweig `MERGE` (Fläche) | — |
| 5 | Übersetzer-Zweig `SPLIT` auf `qAdjacentTo` | — |
| 6 | **Erledigt (v0.40.0, #429):** E1: `BranchId` auf u64, Instanz-Ordinal, `qInstance(...)` | 1 |

Schritte 1–5 sind je für sich abgeschlossen und messbar. Schritt 6 war der
Breaking Change; er ist vorgezogen worden und in v0.40.0 ausgeliefert (§ „Schritt 6"
unten). Offen bleiben damit Schritt 3 (E2) und E3 (Schritte 4–5).

### Schritt 2 — wie er ausgeführt wurde (2026-07-28)

Sonde: Würfel `he = 10`, `fillet_edge` mit `r = 3` auf der +X/+Y-Kante, also
eine **gerade konvexe** Kante. Test:
`schritt2_hard_cutter_fillet_face_exists_but_has_no_blend_role`.

**Der Verdacht oben war falsch, und zwar strukturell.** Seit v0.26.0
(#383/#406) läuft genau diese Kantenklasse über den **harten Cutter**
(`body − (Eck-Box − Tangentialzylinder)`). Gemessen: **0** `MaskedBlend`-Knoten
im `BoundedCsg`. `register_blend_carrier` wird also nie aufgerufen; sein
`wall_a`/`wall_b`-Lookup kann gar nicht scheitern. Der smin-Pfad, auf dem der
Träger tatsächlich lebt, funktioniert und ist seit c-M2 grün
(`fillet_blend_weaves_as_its_own_analytic_face`, gekrümmter Rand).

**Die Verrundungsfläche fehlt nicht.** Der Weave liefert sie:

    face feat=2 sub=0 kind=cylinder  role=WALL  r=3.0  Achse=[7, 7, 0]
    face feat=2 sub=4 kind=plane     role=WALL          (Cutter-Box-Stirn)
    face feat=2 sub=5 kind=plane     role=WALL          (Cutter-Box-Stirn)
    qCreatedBy(2)        -> 3
    qCreatedBy(2, BLEND) -> 0

Radius und Achse sind exakt die der Rollkugel (`he − r = 7`). Die Fläche
gehört dem Fillet-Feature und ist über `qFace(hash)` längst selektierbar —
**was fehlt, ist die Rolle.** `face_role` erkennt `BLEND` allein an der
Halbraum-Klasse `HalfSpace::Blend`, und die entsteht nur auf dem smin-Pfad;
ein Zylindermantel bei Sub 0 liest als `Wall`.

**Latente Namenskollision, mitgemessen.** `2.cut_box` und `2.cut_cyl` sind
beides `FeatureId::child`-Kinder des Fillets und falten auf dieselbe
`branch_identity() == (2, 0)`. Beide Teilkörper registrieren ihre Halbräume
unter `feat=2` ab Sub 0, der Zylindermantel überschreibt darum die +X-Ebene der
Cutter-Box. Heute **folgenlos** — jene Box-Ebene liegt nie an der Oberfläche —
aber genau die Klasse Kollision, die Schritt 1 und E1 gerade beseitigt haben.
Der Test pinnt sie, damit Schritt 3 sie mit auflöst.

**Folge für Schritt 3.** Nicht „Blend-Fläche bauen", sondern: die
Cutter-Teilkörper bekommen getrennte, reservierte Sub-Bänder (nach dem Vorbild
von `EXTRUDE_CAP_TOP_SUB`), der Mantel-Sub liest als `FaceRole::Blend`, und der
Chamfer-Bevel bekommt dieselbe Behandlung — Onshape nennt auch ihn
`BLEND_FACE`. Erst danach hat der Übersetzer-Zweig etwas, worauf er abbilden
kann.

---

### Schritt 3 / E2 — wie er ausgeführt wurde (v0.41.0)

**Die Rolle kommt aus dem Namen, nicht aus der Fläche.** Der smin-Pfad
erkennt sein Blend an der Halbraum-Klasse `HalfSpace::Blend`; der harte Cutter
hat keine solche Klasse — sein Blend ist ein gewöhnlicher Zylinder bzw. eine
gewöhnliche Ebene. Ihn an der *Geometrie* zu erkennen (Radius vergleichen,
Nachbarschaft prüfen) wäre die Ähnlichkeitssuche, die §4 ausschließt. Also
trägt ihn das Sub-Feld: `BranchId::BLEND_SUB` (`0x7FD`), vergeben von
`csg::cutter_sub`, gelesen von `queries::face_role`.

Welche Fläche das ist, folgt aus der Konstruktion, nicht aus einer Messung:

| Cutter | Blend-Fläche | warum |
| --- | --- | --- |
| Fillet | `cut_cyl` Sub 0 | Sub 0 des Zylinders ist sein Mantel, und der Mantel **ist** die Rollkugel-Fläche |
| Chamfer | `cut_bevel` Sub 1 | die Box steht im Frame `(m, s, w)` mit `m` = äußere Winkelhalbierende, also ist `−X` die dem Körper zugewandte Ebene |

**Die Kollision aus Schritt 2 ist mitbehoben.** Jeder Teilkörper bekommt sein
eigenes Band (`CUTTER_AUX_ROUND` / `_BOX` / `_BEVEL`), sodass der
Zylindermantel keine Box-Ebene mehr verdrängt. Beide Teilkörper behalten
dieselbe `branch_identity()` — sie gehören ja beide dem Fillet; getrennt werden
sie eine Ebene tiefer.

**Ein Ort, nicht drei.** Das Re-Keying sitzt in `BranchId::from_feature_sub`.
Der Sub kommt zwar aus `primitive_halfspaces` (der SSOT der Flächenzerlegung),
aber jeder Leser eines Branch-Namens — Weave, Seed-Paare, Hot-SDF-Attribution,
Queries — geht durch diese eine Funktion. Ein Remap in `collect_halfspaces`
allein hätte drei Lockstep-Stellen erzeugt.

`NAMING_SCHEMA_VERSION` ging 3 → **4**: Cutter-Flächen und die Kanten, die sie
berühren, bekommen neue `id_hash`es (schmaler als Bump 3, dieselbe
Heilungs-Zusage).

**Gemessen.** `hard_cutter_fillet_face_is_a_named_blend` und
`hard_cutter_chamfer_face_is_a_named_blend`: `qCreatedBy(f, BLEND)` bindet
genau eine Fläche, mit Rollkugel-Radius und -Achse bzw. auf der
Winkelhalbierenden; die Bänder der Teilkörper überschneiden sich nicht.
Kernel-Suite 1363/1363, Web 2026/2026, alle sechs Onshape-E2E-Suiten
unverändert grün.

**Nebenbefund mit Wirkung.** Der Übersetzer emittiert
`qCreatedBy(<fillet>, BLEND)` **schon heute** — `onshapeImport.ts:675`,
als `qAdjacentTo`-Term, um zwei sonst gleiche Wände auseinanderzuhalten. Diese
Query band bisher null Flächen, die Disambiguierung war also wirkungslos. Sie
wirkt ab jetzt; in den gepinnten Korpus-Zahlen schlägt sich das (noch) nicht
nieder.

**Offen aus Schritt 3:** der Übersetzer-Zweig für `BLEND_EDGE` — Onshapes Rand
einer Verrundungsfläche, in FieldDraft
`qCommonEdge(qCreatedBy(f, BLEND), <nachbar>)`. Die Kernel-Seite steht damit
bereit.

### Schritt 1 — wie er ausgeführt wurde (v0.39.2, #431)

`spatial_index` ist jetzt **gestempelter Zustand** des B-Reps
(`BRep::spatial`, drei zu den Arenen parallele `Vec<u32>`) statt einer
Größe, die jeder Leser für sich neu ausrechnet. Genau **ein** Schreiber:
`brep::naming::stamp_scope`. Gestempelt wird an drei Stellen —
`weaving::finish_weave` (Einzelkörper, der Trichter aller Weave-Routen),
`BrepCache::ensure` (der zusammengesetzte Prefix ist der Scope) und
`Kernel::ensure_named_breps` (der Dokument-Scope, den `bodyBrepFaces` /
`bodyBrepEdges` / `bodyBrepVertices` / `resolveQuery` / der Query-Healer
benutzen). Nebeneffekt: `TopoRef::from_face` rechnete die Indextabelle bisher
bei **jedem** Aufruf neu — in einer Schleife über alle Flächen also O(F²)
inklusive Polylinien-Kopien. Jetzt ist der Lesezugriff O(1).

Die Nummerierung ist **zweistufig**, damit der Schritt keinen Namen bewegt,
der schon eindeutig war:

1. **Zwischen Körpern** — die an einer Signaturgruppe beteiligten Körper
   werden nach dem mittleren **Welt**-Schwerpunkt ihrer eigenen Mitglieder
   sortiert (X → Y → Z, Körperindex als Tie-Break).
2. **Innerhalb eines Körpers** — wie bisher nach dem **körperlokalen**
   Schwerpunkt. Das hält den Stempel invariant gegen Verschieben *und Drehen*
   eines Körpers und lässt jedes Dokument, dessen Körper keine Branch-Signatur
   teilen, byteweise gleich benannt.

**Gemessen (`lib_tests.rs`, Quader + `linear_pattern` Anzahl 3):**

| | vorher | nachher |
| --- | ---: | ---: |
| Flächen gesamt | 18 | 18 |
| verschiedene `id_hash` | **6** | **18** |
| `qFace(hash)` je Hash | 1 von 3 Kandidaten | genau 1 |

Der Körper an `x = 0` behält seine alten Hashes; nur die Kopien werden neu
nummeriert. Alle sechs Onshape-E2E-Suiten blieben unverändert grün
(`#419`: `bodyCount() == 12`, 107 `KNOWN_GAPS`; `#409`: 19 `KNOWN_GAPS`) —
im Korpus teilt kein Körperpaar eine Branch-Signatur.

**Was Schritt 1 ausdrücklich NICHT leistet:** der Name ist positionell. Er
ist reproduzierbar (gleicher Aufbau → gleiche Hashes) und unempfindlich gegen
unbeteiligte Körper (beides als Test gepinnt), aber er wandert, sobald die
Kopien ihre Reihenfolge tauschen — Musterrichtung umdrehen, Abstand negieren,
Kopie in der Mitte einfügen. Dafür ist E1 da.

---

### Schritt 6 / E1 — wie er ausgeführt wurde (v0.40.0, #429)

**Was zuerst gemessen wurde.** Eine Sonde auf einem 3er-Linearmuster eines
Quaders vergleicht die 18 Flächen-`id_hash`es bei Richtung `+x` gegen `−x`.
Vor E1 ergab das eine exakte Block-Umkehr `[A|B|C] → [C|B|A]`: nicht nur die
Kopien tauschten die Namen, auch das **Original** übernahm die Namen von
Kopie 2. Ein Folge-Feature auf einer Fläche des Originals hätte nach einem
Richtungswechsel still auf einer Kopie gesessen. Der Test dazu ist
`pattern_copy_names_survive_a_direction_flip`.

Nebenbefund, der den Testaufbau korrigierte: `spacing < 0` ist **keine**
Umkehrung — `LinearPatternFeature::execute` lehnt es ab (`must be > 0 mm`),
der Regen-Loop schluckt den Fehler und es bleibt **ein** Körper (gemessen:
`bodies=1 faces=6`). Die Umkehrung läuft über die Richtung.

**Die eigentliche Ursache war eine andere als im Entwurf oben.** §4/E1
vermutete, `BranchId::from_feature` falte die hierarchische Klon-Id über
`as_u32_or_root()` auf den Wurzel-Vorfahren zurück. Das stimmt als
Mechanismus, ist hier aber gar nicht wirksam geworden: die Klon-Id **erreicht
den Branch überhaupt nicht**. Ein Muster baut seine Kopie als
`source_feature.clone()` und setzt nur `DecodedFeature::id` um — die Geometrie
liegt aber in `DecodedFeature::node`, und jede id-tragende `CsgNode`-Variante
hält ihre **eigene** `FeatureId`, die des Originals. Gemessen 2026-07-28:
Klon-Ids `2.1~1` / `2.1~2`, gewobene Branch-Wörter aber dreimal
`1/0/0 … 1/0/5`. Der Fix ist `CsgNode::rebrand`, das den Klon-Teilbaum auf die
Klon-Identität umschreibt (inkl. der Branch-Wörter in `Shell`-Öffnungen und
`MaskedBlend`-Provenienz); der Match ist absichtlich erschöpfend.

**Layout.** `BranchId` ist jetzt `u64` mit
`feature (20) | instance (12) | sub (12)` — 44 signifikante Bits. Bewusst
nicht mehr: das gepackte Wort quert die wasm-Grenze als **JSON-Zahl**
(`branchId`, `a`/`b` in `bodyBrepEdges`), und JavaScript-Ganzzahlen sind nur
unter 2^53 exakt. Wer ein Feld weiter aufbohren will, muss zuerst den Draht
auf Strings umstellen. `NAMING_SCHEMA_VERSION` ging 2 → **3**: das Branch-Wort
im Hash wuchs von 4 auf 8 Byte, also hat sich **jeder** `id_hash` geändert.

**Der Feature-Teil trägt die QUELL-Id, nicht die Muster-Id** — eine Abweichung
vom Entwurf oben, und zwar gemessen. Ein Muster darf mehrere Quell-Features
auf einmal kopieren (`resolve_source_feature_ids` liest eine `MultiQuery`).
Trüge der Feature-Teil die Muster-Id, bekämen der Klon von Feature 1 und der
von Feature 2 beide `(P, i, sub)`. Gegenprobe mit zwei Quell-Quadern: unter
dem Muster-Id-Schema fallen 36 Branch-Wörter auf **24** zusammen, unter dem
Quell-Id-Schema bleiben es 36 (`a_two_source_pattern_keeps_its_sources_apart`).
Eine Kopie unterscheidet sich vom Original damit in **genau einem** Feld.

**Query-Form: `qInstance(i, <sub-query>)` — zwei Argumente, nicht drei.** Der
Entwurf skizzierte `qInstance(pattern_id, i, …)`. Die Muster-Id ist nach der
Messung oben aber gar nicht Teil des Namens; ein Argument mitzuführen, das
kein Branch kodiert, wäre ein Argument, das nichts tut. Die Auflösung ist rein
mechanisch: das Ergebnis der Unterabfrage hernehmen, auf jedem Branch seiner
Signatur das Ordinal tauschen, nachschlagen. Keine Geometrie, keine Nähe,
kein Raten — `qInstance(7, …)` auf ein 3er-Muster liefert **nichts** statt der
nächstbesten Kopie (§1).

**Was E1 NICHT leistet.** Zwei *verschiedene* Muster über *dieselbe* Quelle
prägen beide `(Quelle, i, sub)` und kollidieren. 12 Bit haben keinen Platz für
die Muster-Id; das aufzulösen braucht einen breiteren Draht als eine
JS-Zahl. Das steht so in `FeatureId::branch_identity` und ist ausgesprochen,
statt still falsch beantwortet zu werden.

---

## 6. Woran gemessen wird

Jeder Schritt wird gegen die sechs Onshape-E2E-Suiten gemessen
(`web/src/onshape/*.slow.test.ts`), und die Pins werden mitgezogen — die
Namenslisten `KNOWN_GAPS` und `expect(kernel.raw.bodyCount())` sind Assertions
in **beide** Richtungen. Zusätzlich gilt für alles, was Blends betrifft:

> **Keine zwei Blends dürfen dieselbe aufgelöste Kanten-Query tragen.**

So sah die erfundene Verrundung aus (#414), und der #419-E2E prüft es seit
v0.39.1 explizit.

---

## Quellen

- Kripac, J.: *A mechanism for persistently naming topological entities in
  history-based parametric solid models*, CAD 29(2), 1997.
- Chen, X.; Hoffmann, C. M.: *On editability of feature-based design*, CAD 27(12),
  1995 — „generic naming".
- Capoyleas, V.; Chen, X.; Hoffmann, C. M.: *Generic naming in generative,
  constraint-based design*, CAD 28(1), 1996.
- Bidarra, R.; Bronsvoort, W. F.: *A feature-based solution to the persistent
  naming problem*, Computer-Aided Design & Applications 2(1–4), 2005 —
  Zellmodell.
- Raghothama, S.; Shapiro, V.: *Topological framework for part families*, 2002 —
  Formalisierung und Unlösbarkeitsargument.
- Farjana, S. H.; Han, S.: *Mechanisms of persistent identification of
  topological entities in CAD systems: A review*, Alexandria Eng. J., 2018.
- In-Repo-Vorarbeit: `docs/Gemini-GPT-Diskussion-Topological-Naming-Problem.md`,
  `docs/BREP-CLASSIFICATION-PLAN.md`.
