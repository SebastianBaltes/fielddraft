# FieldDraft wird eingestellt

Stand 2026-10-21. Letzte Version: 0.66.4.

> **English summary.** FieldDraft was a browser-based parametric CAD modeller
> built on a signed-distance-field (SDF) kernel instead of a classical B-Rep.
> After six months of work it ran into three dead ends that share one root
> cause: Onshape models could not be imported reliably, fillets never became
> satisfactory, and Boolean combinations of SDFs do not yield a true SDF. The
> conclusion was that a full B-Rep kernel is the way out. The open-source
> project [SolidCraft](https://github.com/tangency-labs/solidcraft) (Rust,
> B-Rep, MIT/Apache-2.0) is already going down exactly that road and is much
> further along, so FieldDraft is discontinued. The code stays available as
> a reference; see [SOLIDCRAFT-COMPARISON.md](SOLIDCRAFT-COMPARISON.md) for a
> detailed comparison and for what might be carried over.

## 1. Die Ausgangsidee

FieldDraft sollte ein vereinfachter, Onshape-ähnlicher Modeller für den
3D-Druck sein, der statt auf einen B-Rep-Kernel (OpenCASCADE, Parasolid) auf
ein **Distanzfeld** setzt. Die Wette war: Booleans auf SDFs scheitern nie
(`min`/`max`), Offsets und Shells sind trivial (`f − r`), Rendering per
Raymarching braucht keine Tessellierung, und Lattices wie Gyroid-Infill
fallen nebenbei ab. Die Randdarstellung (Kanten, Flächen, Namen), die ein
parametrisches CAD für Picking, Referenzen und Export braucht, sollte aus dem
Feld rekonstruiert werden.

## 2. Die drei Sackgassen

### 2.1 Booleans liefern kein echtes SDF

Ein Distanzfeld ist nur dann ein Distanzfeld, wenn sein Wert der echte
Abstand zur Oberfläche ist. Die Forschung in
[research/sdf-metricity/SYNTHESIS.md](research/sdf-metricity/SYNTHESIS.md)
(#384) hat präzise gezeigt, wo das bricht:

- Harte Booleans sind nur **seitenselektiv** exakt: `min` (Union) stimmt im
  Außenraum, `max` (Intersection/Differenz) im Innenraum. Union innen und
  Differenz außen — also genau Tasche und Bohrung, der CAD-Normalfall — sind
  falsch, gemessen bis 58 % Tiefen-Unterschätzung bzw. 13–16 % der
  Abfragedistanz.
- Ein Teil der Fehler sind **Fußpunktfehler**: Das Feld ist dort lokal das
  perfekte SDF einer *falschen* Fläche (Phantomflächen des Subtrahenden).
  Jede gradientenbasierte Korrektur ist dafür blind; helfen würden nur
  nichtlokale Verfahren wie Redistancing oder ein B-Rep-Proxy.
- Kein globaler Faktor repariert das Feld, und `f − δ`-Offsets erzeugen
  strukturell falsche Isoflächen.

Damit fallen genau die Versprechen, für die das SDF gewählt wurde: Offset,
Shell und Fillet setzen ein metrisches Feld voraus — und das gibt es nach dem
ersten Boolean nicht mehr garantiert.

### 2.2 Fillets wurden nie zufriedenstellend

- Der feldbasierte `smin`-Blend wirkte an Kanten eines einzelnen Primitivs gar
  nicht (#406) und verschliff zusätzlich die Ecken kugelig (#383).
- Der Umbau auf **analytische harte Cutter**
  ([FILLET-CUTTER-PLAN.md](FILLET-CUTTER-PLAN.md)) kam bis Phase 2
  (beliebiger Diederwinkel); Phasen 3–6 (Ecken, gekrümmte Kanten, Ketten)
  blieben offen.
- Jeder Fillet hängt an einer Kante, die im SDF gar nicht existiert, sondern
  erst vom Weaver rekonstruiert wird. Jeder Fehler in dieser Rekonstruktion
  schlägt auf den Fillet durch.

### 2.3 Onshape-Import nicht fehlerfrei

Der parametrische Import über die Onshape-REST-API ist breit gekommen: Der
BTON-Decoder liest alle 697 Testdokument-Strukturen, elf Feature-Typen,
Variablen, Muster und Mate-Connector-Ebenen werden übernommen. Fehlerfrei
wurde er nie. [ONSHAPE-IMPORT-GAP-MATRIX.md](ONSHAPE-IMPORT-GAP-MATRIX.md)
führt auf knapp 4 000 Zeilen die Abweichungen pro Dokument: Rotationen um
90°, Versätze, fehlende Typen. Der Kern des Problems: Onshapes Queries
referenzieren **B-Rep-Topologie** (Flächen, Kanten, deren Entstehungs-
Historie). FieldDraft musste diese Topologie aus dem Feld nachbauen und über
eigene Branch-IDs benennen — eine Übersetzung zwischen zwei Welten, die an
jeder Naht Fehler produziert.

## 3. Die gemeinsame Ursache

Alle drei Sackgassen sind Kosten derselben Entscheidung. FieldDraft rechnet
ein **implizites Feld** und muss die Randdarstellung nachträglich
zurückgewinnen: Kanten weben, Flächen klassifizieren, Namen über Branch-IDs
vergeben, Meshes mit Feature-Snap erzeugen. Ein B-Rep-Kernel hat Kanten,
Flächen und ihre Historie **strukturell** — Fillets rollen an einer echten
Kante, Referenzen zeigen auf echte Flächen, und Onshape-Queries lassen sich
direkt abbilden. Er kämpft dafür mit anderen Problemen (Robustheit von
Schnittkurven, Toleranzen), aber das sind die bekannten, lösbaren Probleme
der Branche, nicht strukturelle.

Das Fazit war daher: **Ein vollständiger B-Rep-Kernel ist der Ausweg.**

## 4. Warum nicht umbauen, sondern einstellen

Im Oktober 2026 ist [SolidCraft](https://github.com/tangency-labs/solidcraft)
erschienen: ein Open-Source-CAD in Rust, B-Rep-basiert (truck, abgelöst durch
den SMLib-Port solidcore), mit eigenem Constraint-Solver, History-basiertem
Topological Naming, Desktop-, Web-, CLI- und MCP-Zugang, unter MIT/Apache-2.0
und mit einer großen Community. Es geht genau den Weg, zu dem FieldDraft
selbst gekommen war, und ist in fast allen Bereichen bereits deutlich weiter.
Einen zweiten B-Rep-Modeller daneben zu bauen, hätte keinen Mehrwert.

Die ausführliche Gegenüberstellung steht in
[SOLIDCRAFT-COMPARISON.md](SOLIDCRAFT-COMPARISON.md).

## 5. Was bleibt

Einiges aus FieldDraft ist unabhängig vom Ausgang interessant:

- **SDF-Metrizitätsforschung** ([research/sdf-metricity/](research/sdf-metricity/)):
  Normfehler vs. Fußpunktfehler, seitenselektive Exaktheit harter Booleans —
  relevant für jedes SDF-CAD (libfive, fidget, ImplicitCAD).
- **Implizite Features**: Gyroid-/TPMS-Infill und Formel-SDFs, FieldDrafts
  einziges echtes Alleinstellungsmerkmal gegenüber SolidCraft.
- **Onshape-BTON-Decoder und Query-Mapping** als Dokumentation, wie Onshape
  Features und Referenzen intern ablegt.
- **Trackball-Kamera** mit Upright-Modus und mitlaufendem Pivot.
- Viele Erfahrungen zu WebGPU/Three.js-Raymarching, Meshing (Dual Contouring,
  Surface Nets, Lichen, CGAL) und agentengetriebener Entwicklung.

Welche davon sich konkret in SolidCraft einbringen lassen und welche
Hürden es gibt (Clean-Room-Regel gegenüber GPL/LGPL, Verbot von Daten aus
kommerziellen CAD-Produkten, Lizenz MIT/Apache-2.0), beschreibt Abschnitt 6
der Vergleichsanalyse.
