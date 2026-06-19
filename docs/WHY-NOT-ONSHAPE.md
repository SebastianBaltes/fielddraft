# Warum nicht Onshape?

Eine laufende Sammlung konkreter Reibungspunkte mit Onshape, die FieldDraft besser machen will. Jeder Eintrag beschreibt das Problem, wie es reproduziert wurde und was wir stattdessen anstreben. Neue Punkte unten anhängen.

## 1. OBJ-Export zerlegt einen Solid in eine Sub-Objekt-pro-Face-Wüste

**Beobachtet:** 2026-06-19

Einfacher Workflow: ein Viereck skizzieren, extrudieren, zwei Ecken per Chamfer abflachen, dann als Mesh (OBJ) exportieren. Ergebnis: Onshape schreibt **jede einzelne Face als eigenes Sub-Objekt** (`o`/`g`-Gruppe) in die OBJ-Datei — aus einem zusammenhängenden Solid werden Dutzende getrennte Objekte.

**Warum das nervt:**
- Downstream-Tools (Slicer, Game-Engines, Three.js-Loader) sehen statt eines Körpers viele lose Flächen — kein geschlossenes, wasserdichtes Mesh.
- Normalen-/Smoothing-Gruppen und Nachbarschaftsinformationen gehen verloren; man muss in der Zielanwendung erst wieder „Merge by distance" / „Join" machen.
- Schon ein triviales Teil (Viereck + 2 Chamfer) wird unbrauchbar segmentiert — das skaliert mit echter Geometrie zur Katastrophe.

**Was FieldDraft stattdessen will:** Ein exportierter Solid ist EIN Mesh-Objekt — eine zusammenhängende, indizierte, wasserdichte Vertex-/Face-Liste. Face-Grenzen sind eine Eigenschaft des B-Rep-Modells, nicht der exportierten Mesh-Topologie. Optionale Gruppierung nur, wenn der Nutzer sie explizit anfordert.
