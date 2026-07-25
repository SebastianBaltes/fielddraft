# Sketch-Region Imprint-Subdivision — Plan & Design

Status: in Arbeit (#412 P0-Folgearbeit). SSOT für die Regionen-Semantik im
Sketch-Modell. Ziel: FieldDrafts Kernel identifiziert Extrude-/Revolve-/Loft-
Profile wie Onshape als **Imprint-Regionen** — die Menge ALLER Sketch-Kurven
(auch offener Trennlinien, überlappender und verschachtelter Profile) zerlegt
die Sketch-Ebene in **atomare Faces**, die einzeln adressierbar sind.

## 1. Ausgangslage (verifiziert, v0.29.0)

Es gab bis hierher **zwei** divergierende Regionen-Modelle.

### 1a. Kernel — SDF-Flood-Fill (`crates/kernel/src/sketch_regions.rs`)

`find_regions(&Sketch2d, resolution)` wertet die 2D-SDF des **gelowerten**
Profilbaums auf einem Gitter aus und flutet zusammenhängende Innenzellen
(SDF < 0) zu Regionen. Konsumenten:

- `api/sketch.rs::list_sketch_regions` / `region_query_at_point` (WASM-API
  `listSketchRegions` / `regionQueryAtPoint`).
- `sketch/lower.rs::resolve_profile_query` → Extrude (`features/extrude.rs`),
  Revolve (`features/revolve.rs`), Loft (`features/context_resolve.rs`) über
  `qRegion(s,i)` / `qRegions(s,[…])` / `qAllRegions(s)` / `qCreatedBy(s)`.
- `restrict_sketch_to_points` schnitt die Profil-CSG auf die Union-Branches
  zurück, deren SDF am Regionen-Innenpunkt negativ ist.

**Fundamentale Lücke:** `Sketch2d` ist der gelowerte CSG-Profilbaum. Das
Lowering (`sketch/lower.rs::build_sketch2d_traced` + `sketch/loops.rs`) wirft
**offene Trennlinien weg**: freie `sketch_line`/offene Splines werden nur dann
Teil des Profils, wenn ihre Endpunkte zu einer **geschlossenen Schleife**
ketten (`infer_closed_loops`). Eine interne Trennlinie, die ein bestehendes
Face zerschneidet, aber selbst keine eigene Schleife bildet, verschwindet
spurlos. Verschachtelte/überlappende geschlossene Profile verschmelzen im
Flood-Fill zu EINER Region. Für das #412-Dokument (Blech-Platte, durch
Trennlinien in Platten-Faces zerteilt) findet der Kernel darum nur **1 Region**.

**Konsequenz fürs Design:** die Imprint-Zerlegung MUSS auf den **rohen
sub_calls** laufen (die die Trennlinien noch enthalten), nicht auf dem
gelowerten `Sketch2d`. Beide WASM-Einstiege haben die sub_calls bereits zur
Hand; `resolve_profile_query` bekommt sie über `SketchData.sub_calls`.

### 1b. Web — DCEL-Arrangement (`web/src/sketchRegions.ts`)

Für die interaktive Shade-Overlay-Anzeige im Sketch-Editor (`sketchOutline.ts`
→ `closedRegionsCached`). Baut aus den sub_calls einen planaren Graphen:
intrinsisch geschlossene Primitive (Kreis, Rechteck, …) werden direkt gefüllt;
offene Kurven (Linien, Bögen, offene Splines) werden per Endpunkt verschweißt,
an **T-Junctions** gesplittet (`planarizeEdges`) und die beschränkten Faces per
Standard-DCEL-Traversierung extrahiert (`loopsFromEdges`).

**Grenzen des Web-Finders:** (a) Er arrangiert **geschlossene Primitive nicht
gegeneinander** — zwei konzentrische Kreise werden als zwei überlappende
Füllungen (nonzero-Regel) gezeichnet, NICHT als Ring + Innenfläche. (b) Er
splittet nur an Endpunkt-auf-Kante-T-Junctions, nicht an **allgemeinen
Kurven-Schnittpunkten** (zwei sich kreuzende Linien mitten im Feld). Er ist
also *näher* an Imprint als der Kernel, aber ebenfalls kein volles Arrangement.

### 1c. Interaktives Teil-Region-Extrudieren heute

Klick in eine gefüllte Region → `Viewport.tsx::sketchRegionAtClick` →
`kernel.regionQueryAtPoint(sketchId,u,v)` → speichert `qRegion(s,i)` im
Extrude/Revolve-`profiles`-Feld. Der Index `i` kommt aus der Flood-Fill-
Reihenfolge; er ist **nicht edit-stabil** (Scan-Reihenfolge des Gitters) und
adressiert bisher nur ganze Union-Regionen — eine echte Teil-Fläche (Ring,
Platte) war gar nicht adressierbar, weil der Kernel sie nie fand.

## 2. Ziel-Semantik (Imprint / Arrangement)

Die **Randkurven ALLER Sketch-Entities** (Linien, Bögen, Kreise → Vollkreis-
Bögen, Rechtecke/Polygone → Kantenzüge, Ellipsen/Splines → Bézier-Ketten;
Konstruktionsgeometrie ausgenommen) bilden gemeinsam ein planares Arrangement.
Die beschränkten Faces dieses Arrangements sind die **atomaren Imprint-Faces**.

- Eine geschlossene Kurve **in** einer Fläche (Kreis im Rechteck) prägt sich
  ein: Rechteck → Rahmen (Rechteck ∖ Kreis) + Scheibe.
- Verschachtelte Kreise → **Ring** (Difference) + **Innenscheibe**. (Größte
  sichtbare Verhaltensänderung ggü. dem alten Union-Flood-Fill.)
- Sich kreuzende/überlappende Profile → Linsen-Faces.
- Eine Trennlinie, die zwei Region-Ränder verbindet, zerteilt (Onshape-Imprint).
- Eine **Dangling-Kurve**, die frei im Inneren endet, zerteilt **nicht** — das
  Flutfeld läuft um die freie Spitze herum, beide Seiten bleiben verbunden.
  (Entspricht Onshape: nur Kurven, die Ränder verbinden, imprinten.)

Die alte Union-Semantik bleibt als **Aggregat** erhalten: eine MultiQuery
`qRegions(s,[i,j,…])` extrudiert mehrere atomare Faces zusammen; berührende
extrudierte Solids verschmelzen weiterhin NICHT implizit (Regel „No implicit
Boolean merge").

## 3. Gewähltes Design

**EINE Wahrheit im Kernel.** Der Kernel bekommt ein echtes planares
Arrangement über den rohen sub_calls (`crates/kernel/src/sketch_arrangement.rs`).
`find_regions` / `region_index_at_point` / die Profil-Auflösung konsumieren es.
Der Web-Finder bleibt vorerst als reines **Overlay-Shading** bestehen
(kein Query-Konsument), wird aber langfristig ebenfalls auf die Kernel-Wahrheit
gezogen (Kandidat: `listSketchRegions`-Polygone rendern). Kein zweites
Regionen-Modell in Query-Pfaden.

### 3a. Algorithmus

1. **Sammeln:** je Entity die exakten Rand-`SketchSegment`e (Line/Arc/
   QuadBezier). Kreis → ein Vollkreis-`Arc` (sweep 2π). Rechteck → 4 Lines.
   Polygon → n Lines. Ellipse/Spline/Path → ihre Bézier-/Segment-Ketten.
   Freie Linien/Bögen/offene Splines ebenso — **alles** wird Kante, keine
   Sonderrolle für „Trennlinie vs. Füllung".
2. **Flatten (nur Topologie):** jedes Segment in eine Polyline; jede
   Mikro-Gerade trägt die Quelle (Source-Segment-Index) mit. Exaktheit kommt
   NICHT aus der Flatten-Auflösung, sondern aus der Rekonstruktion (3c).
3. **Planarisieren:** alle Mikro-Geraden paarweise schneiden, an Schnitt-
   punkten splitten, Endpunkte tolerant verschweißen (Spatial-Hash) → PSLG.
4. **DCEL-Faces:** ausgehende Halbkanten je Vertex CCW sortieren,
   `next(h)` = im-Uhrzeigersinn-nächste Kante nach `twin(h)`; Faces mit
   **positiver** Fläche behalten (die unbeschränkte Außenfläche läuft CW).
   (Portierung von `loopsFromEdges`.)
5. **Containment-Nesting:** für jede positive Face die unmittelbar enthaltenen
   Faces bestimmen; atomare Face = Face **minus** ihre direkten Kinder. Ring:
   Außenscheibe minus Innenscheibe; Innenscheibe bleibt eigene Face.
6. **Ordnung (deterministisch, edit-stabiler als Flood-Fill):** aufsteigend
   nach `(aabb.min.y, aabb.min.x, area)` des atomaren Faces. Begründung: rein
   geometrie-abgeleitet und reproduzierbar; unabhängig von Gitter-Scan-Reihen-
   folge. **Grenze:** Indizes sind NICHT über beliebige Sketch-Edits stabil —
   fügt man ein Face hinzu, können Nachbar-Indizes rutschen. Das ist für den
   Query-Pfad unkritisch, weil `regionQueryAtPoint` zur Auswahl-Zeit über den
   **Innenpunkt** auflöst (nicht über einen persistierten Index), und der
   Onshape-Import ebenfalls über Innenpunkte matcht.

### 3b. Dangling / T-Junction / offene Kurven

Fallen automatisch aus dem Arrangement:
- **T-Junction** (Linie endet auf anderer Linie): der Schnittpunkt-Split in (3)
  erzeugt dort einen echten Graph-Vertex → die zwei Faces trennen sauber.
- **Dangling** (Linie endet frei im Inneren): ihre freie Spitze ist ein
  Grad-1-Vertex; die Face-Traversierung läuft an ihr hin und zurück (die Kante
  wird in beiden Richtungen begangen), erzeugt also KEINE Trennung — die Fläche
  bleibt eine Face. (Wie Onshape.)

### 3c. Exakte Profil-Rekonstruktion (keine Facettierung der Regelfälle)

Je Quelle wird mitgeführt, ob sie **geschnitten** wurde und wie viele
Graph-Kanten sie nach Verschweißen/Dedup noch stellt (`SourceUse`). Liegen
**alle** überlebenden Kanten einer un-geschnittenen krummen Quelle auf dem Rand
dieser Face, wurde sie GANZ verbraucht → das exakte `SketchSegment`
(Arc/QuadBezier) wird einmal emittiert. Nur ein **tatsächlich geschnittenes**
krummes Segment (selten: eine Trennlinie, die einen Kreis kreuzt) erscheint als
Mikro-Linien-Kette (facettiert). Damit sind alle Regelfälle (Kreis, Ring,
Rechteck-Platte, #412) **exakt**: eine extrudierte Kreis-Region bleibt ein
exakter Zylinder, kein Prisma. Das Profil einer atomaren Face =
`Path{closed:true}` des Außenrands, minus (`Difference`) der Kind-Ränder.

**Gotcha (gefunden & gefixt):** `sweep_angle` ist ein `f32`; ein Vollkreis-TAU
liegt ~1.7e-7 rad **über** 2π. Der letzte Flatten-Punkt schoss dadurch über den
ersten hinaus, sodass die Schluss-Sehne die Anfangs-Sehne echt KREUZTE — der
Kreis galt als „geschnitten" und verlor seine exakte Rekonstruktion. Fix:
Vollkreis-Bögen schließen im Flatten **exakt** (letzter Punkt := erster), und
`seg_intersection` verwirft Kreuzungen, die innerhalb der Schweiß-Toleranz an
einem der vier Endpunkte liegen (geteilter Vertex statt echtem Schnitt).

## 4. Verhaltensänderungen (bewusst)

- **Verschachtelte/überlappende geschlossene Profile** werden jetzt in atomare
  Faces zerlegt (Ring + Innenscheibe statt einer verschmolzenen Region). Alte
  Kernel-Tests, die hier „1 Region" erwarteten (`sketch_regions.rs`-Unit-Tests
  `ring_with_hole_produces_one_region`, konzentrische Fälle), werden bewusst auf
  die neue, feinere Semantik angepasst.
- `restrict_sketch_to_points` (Union-Branch-Rückschnitt) entfällt — Profile
  kommen direkt aus dem Arrangement.
- `find_regions`/`region_index_at_point` nehmen jetzt sub_calls statt `Sketch2d`.

## 4a. Onshape-Region-Query → atomare Face (Web-Seite)

Beim Live-Test an #412 kam ein **zweiter, bis dahin unsichtbarer Fehler** zum
Vorschein. Onshape benennt eine Region über **nur zwei ihrer Randkanten**
(z.B. `<rectA>.right` + `<rectB>.bottom`). Der bisherige Web-Löser
(`regionInteriorPoint`) nahm den **Schwerpunkt der Eckpunkte dieser Kanten** als
Innenpunkt — der liegt im Allgemeinen **NICHT** in der gemeinten Region, sondern
irgendwo in der Fläche dazwischen. Solange der Kernel nur 1 Region fand, fiel
das nicht auf (alles kollabierte ohnehin auf `qRegion(1,0)`).

Gemessen am #412-Dokument: die vier Onshape-Auswahlpunkte landeten bei
(-1551,1351), (-1635,1186), (-1260,1636), (993,1717) — drei davon mitten in der
großen Restfläche, nur einer in einer Platte.

**Gewählte Lösung (`regionFaceIndex`):** nicht raten, sondern **an den benannten
Kanten sondieren**. Je Kante beidseitig knapp neben der Kantenmitte abfragen
(`regionQueryAtPoint`) — das liefert genau die zwei angrenzenden Faces: die
spezifische Face, die die Kante berandet, und die große Umgebungs-Face. Da die
Umgebungs-Face an JEDE benannte Kante grenzt, wäre ein Schnitt der Kandidaten-
mengen immer die Umgebungs-Face; gewählt wird deshalb die **spezifischste, d.h.
flächenkleinste** angrenzende Face (Flächen aus `listSketchRegions`).

Ergebnis am #412-Dokument: `qRegions(1, [1, 2, 3, 4])` — die vier Platten, 1:1
zur Onshape-Auswahl, ohne Reduktions-Warnung.

**Grenze (dokumentiert, nicht still):** eine Auswahl der Umgebungs-Face selbst
ist so nicht ausdrückbar und würde auf eine ihrer Nachbar-Faces fallen. Der
saubere Endzustand wäre, die Face direkt aus Onshapes Imprint-Regionen-Benennung
abzuleiten statt aus Geometrie — Folgearbeit.

**Extrude-MultiQuery:** mehrere Faces werden zu EINEM `qRegions(s,[i,j,…])`
zusammengefasst, nicht komma-verkettet zu mehreren `qRegion`-Termen — der
Extrude wertet für den Sketch-Pfad nur die **erste** Profile-Query aus, ein
komma-verketteter Ausdruck hätte also nur die erste Platte extrudiert.

## 5. Grenzen / offene Punkte

- **Geschnittene krumme Segmente** (Trennlinie kreuzt Kreis/Ellipse/Spline)
  werden in ihrem Face facettiert (nur die Randstücke, mit Flatten-Auflösung).
  Un-geschnittene Kurven bleiben exakt. Onshape-Import #412 ist rein
  geradlinig, daher nicht betroffen. Exakte Sub-Bogen-/Sub-Bézier-Rekonstruktion
  ist als Folgearbeit möglich (Sub-Range-Winkel für Arc; de-Casteljau für Bézier).
- Robuste Schnittarithmetik ist O(M²) über die Mikro-Kanten; für reale Sketches
  (≤ einige hundert Mikro-Kanten) unkritisch. Bentley-Ottmann wäre die Skalier-
  Folgearbeit, falls je nötig.
- Der Web-Overlay-Finder ist noch nicht auf die Kernel-Wahrheit vereinheitlicht
  (er shadet nur, konsumiert keine Query). Vereinheitlichung ist Folgearbeit.
- **`sketch_mirror` / `sketch_offset`**: beide schreiben den akkumulierten
  PROFIL-BAUM um, statt eigene Randkurven beizusteuern. Ein Arrangement über die
  rohen Entities würde die gespiegelte Kopie bzw. die Offset-Verschiebung still
  unterschlagen. Solche Sketches melden deshalb bewusst **null Regionen**
  (`has_unarrangeable`) — der Aufrufer nutzt dann das Gesamt-Sketch-Profil
  (`qCreatedBy`): gröber, aber nie geometrisch falsch. `sketch_path` ist dagegen
  voll unterstützt (Konturen gehen als Segmentketten ins Arrangement).
- Region-eingeschränkte Profile tragen **noch keine Entity-Provenienz**; ihre
  Wall-Subs fallen auf positional zurück (Gesamt-Sketch-Profile behalten die
  volle Trace). Stabile Entity-Subs für Imprint-Teilflächen sind Folgearbeit.
