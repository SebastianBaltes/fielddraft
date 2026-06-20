# Warum nicht Onshape?

Eine laufende Sammlung konkreter Reibungspunkte mit Onshape, die FieldDraft besser machen will. Jeder Eintrag beschreibt das Problem, wie es reproduziert wurde und was wir stattdessen anstreben. Neue Punkte unten anhängen.

1. Free \= alle Rechte an Modellen abtreten. Paid Plan \= €1410 / Jahr / Dev
2. Filets / Booleans brechen sehr schnell die Geometrien (Mikroskopische Ecken / Dreiecke, die nicht ganz passen) x weitere Operationen \=> Invalid
3. Viereck \=> Extrude \=> Chamfer \=> OBJ Export \=> jede einzelne Face als eigenes Sub-Objekt / Non-Manifold Export
4. Import SVG, als Stamp verwenden \=> non Manifold