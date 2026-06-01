* Struct Feature: id und name sind sicher allgemeine Eigenschaften, aber primitive, transform, blend\_radius, chamfer\_size und weitere ... das sind spezifische Eigenschaften, die nur bestimmte Features benötigen. Es ist nicht akzeptable, einen Basis-Typ zu haben, der sämtliche Datenfelder aller Features enthält, da diese später auch zur Laufzeit über eine Skriptsprache definiert werden können (FieldSkript).
* FieldSkript - bitte lies dazu die Doku bei Onshape [https://cad.onshape.com/FsDoc/](https://cad.onshape.com/FsDoc/) und erstelle ein Konzept, wie wir das in FieldDraft nachimplementieren können. Insbesondere die Meta-Auszeichnung der Fieldskript-Parameter und das Query-System für Körper und BRep. Als Sprache bietet sich Typescript an. Eine CRUD Oberfläche für FieldSkripte und ein ausgewachsener Editor sollte existieren (was nimmt man da - Monarch?)
* Scale Feature verwendet, Scale einer Box damit geändert, Scale Feature gelöscht \=> Box bleibt gescaled.
* Selektion von BRep - Edges und Flächen
* Sketches in der Szene sollten selektierbar sein (indem man auf eine der Objekte im Sketch klickt, die ja in der 3D Szene zu sehen sind entsprechend Projeziert im 3D-Raum, wenn der Sketch visible ist)
* Sketch soll als Detail-Eingabefeld "Sketch Plane" haben, d.h. ähnlich der Körperauswahl in anderen Features kann man hierbei eine Plane auswählen, z.b. die Seitenfläche eines Würfels, oder eine der Default Geometry Planes, oder einen Mate Connector.
* Extrude klappt nicht (man kann einen Sketch nicht als Ziel auswählen in der UI)
* Zu Technical Material soll es auch noch einen Namen als Eingabefeld geben. Materials sollen also in einer Oberfläche CRUD sein - statt fester Presets gibt es also vorgefertigte Materialien, die sich erweitern lassen
* Compile von Shadern lässt Browser einfriefen (auch in andern Tabs). Ich dachte, das sollte Async sein? Kann man das einfrieren verhindern?
* Bei Gyroid auf Kugel ist das erzeugte BRep offenkundig falsch
* Dann zu den Primitiven: Ich hätte gerne noch Thread und ein "Follow Oriented Path" also ein Twist, welche auf Sketch2d operieren. Du findest die Doku dazu in docs.bak/onshape-ux.md und in entsprechenden assets/onshape/yaml und assets/onshape/screenshot
* Sketch ist absolut unvollständig implementiert!
* \- Selekt von Objekten (Linie, Punkt, Kreis etc) soll durch Klick oder Aufzieh-Selektions-Rechteck funktionieren, gibt es nicht
* \- Arc funktioniert nicht: Die Erwartung ist, dass es wie Line ziehen durch MausDown, MausDrag, MausUp einen Kreisausschnitt mit Radius der Liniezieht und einen Kontrollpunkt für den Mittelpunkt, der anschließend verschoben werden kann.
* \- Trim funktioniert nicht: Man soll auf ein Objekt (also z.b. Linie) klicken und sie dadurch löschen.
* \- Mirror, Offset funktionieren nicht.
* \- Polygon ist überflüssig (dafür gibt es Line). Statt der aktuellen Funktion ist ein N-Eck die bessere Variante.
* \- Spline, Bezier fehlt.
* [https://github.com/timschmidt/csgrs](https://github.com/timschmidt/csgrs)
* [https://github.com/cheind/sdftoolbox](https://github.com/cheind/sdftoolbox)
* [https://github.com/kaist-visual-ai-group/odc](https://github.com/kaist-visual-ai-group/odc)
* [https://github.com/sgsellan/gpytoolbox](https://github.com/sgsellan/gpytoolbox)
* [https://crates.io/crates/delaunay](https://crates.io/crates/delaunay)