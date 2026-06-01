# B-Rep-Feature — Die Vision des Users

> **Was dieses Dokument ist.** Eine aufbereitete Rekonstruktion des Bildes, das **der User** (Sebastian) vom B-Rep-Feature gezeichnet hat — destilliert aus seinen eigenen Chat-Nachrichten im Zeitraum **2026-05-30 bis 2026-06-01**. Es gibt bewusst *seine* Sichtweise und seine Entscheidungen wieder, nicht die des Agenten. Wörtliche Zitate sind als Blockquote mit Datum gekennzeichnet. Die technische Umsetzung steht in `GEOMETRY-KERNEL-CONCEPT.md` (Quelle der Wahrheit für das *Wie*); dieses Dokument hält das *Warum* und die Leitplanken fest.

---

## 1. Der Wendepunkt: Backtracking nach zwei harten Lektionen

Der ursprüngliche Geometrie-Kernel-Ansatz (naive Half-Spaces als SDF + analytische Spline-Distanzen) lief in zwei fundamentale Wände. Der User hat daraus nicht ein Workaround abgeleitet, sondern einen bewussten Richtungswechsel.

> „Was ich gelernt habe:
> * Splines und Bezier müssen in lineare oder quadratische Teile segmentiert werden (Unlösbarkeit 5. Grad)
> * Half-Spaces naiv explodieren als SDF (512 OP Codes Limit)
>
> Wir brauchen einen anderen Ansatz. Wie bei Backtracking machen wir einen Schritt zurück, profitieren vom Gelernten und gehen einen Schritt in eine andere Richtung."
> — *2026-05-31*

Die beiden Lektionen im Detail, in seinen Worten:

- **Splines/Bezier sind analytisch nicht distanzierbar.** Konsequenz: Kantenzug im Rust-Kernel bilden.
  > „Analytisch ist der Abstand nicht bestimmbar bei Splines / Bezier, daher Kantenzug bilden im Rust-Kernel. Das begrenzt natürlich die Auflösung und Genauigkeit für CAD, weil bei FieldDraft die SDF die zentrale Struktur ist — das ist traurig, aber kein absoluter Blocker." — *2026-05-30*

- **Naive Half-Spaces sprengen das SDF-VM-Limit.**
  > „Wir können sowieso nur 512 — oder meinetwegen 10.000 — Leaf-Nodes, d.h. Halbflächen oder Sketch-Linienstücke … das ist für CAD ein Blocker. Dann ist dieser Weg tot." — *2026-05-30*

---

## 2. Was ein echtes CAD wirklich braucht (der Boden, auf den zurückgekehrt wird)

Statt den toten Weg zu flicken, kehrt der User bewusst zum bewährten `main`-Stand zurück und baut das B-Rep-Ziel **darauf** neu auf.

> „Was brauchen wir in einem echten CAD? Vor allem Sketches (approximierte), auf die Extrude, Rotate, Helix und Sweep angewendet werden (⇒ Sketch-Körper)." — *2026-05-31*

Sein Bild des vorhandenen, funktionierenden Fundaments:

1. **Sketch-Körper** als Kern: Sketches + Extrude / Rotate / Helix / Sweep.
2. **Projektion ins 2D des Sketches.** Für Extrude/Rotate/Helix (sofern Selbstüberschneidung von vornherein unterbunden ist) kann das 3D-SDF als Projektion in den 2D-Raum des Sketches abgefragt werden — über ein **vorverarbeitetes, gecachtes BVH** (Erzeugung ~log N, Abfrage ~log N), das nur die tatsächlich einflussreichen Terme liefert. Sweep braucht ein komplexeres 3D-BVH.
3. **SDF Deferred Renderer**, der pro Pixel auf opak/transparent klassifizierten Hüllmeshes operiert und daraus ableitet: die Liste der einflussreichen Bodies (⇒ SDF-Terme/VM) und das Iterationsintervall auf der Z-Achse.
4. **Effizientes Meshing** der Sketch-Körper zur Erzeugung der Hüllmeshes.
5. Darauf angewendet: **SDF-Transformationen** (Bool, Gyroid, Raumfeld, …).

> „Das war der Stand (auf main), den wir vor dem … Geometrie-Kernel-Konzept hatten." — *2026-05-31*

---

## 3. Das eigentliche Ziel: B-Reps effizient — und auf main übertragbar

Der User trennt sauber zwischen *Ziel* und *gescheitertem Weg*. Das Ziel des neuen Konzepts bleibt gültig; nur der Unterbau wechselt.

> „Das eigentliche Ziel des neuen Geometrie-Kernel-Konzeptes war, B-Reps effizient umzusetzen. Dazu hatten wir einen Octree, duale Zahlen und in den Raum ausstrahlende Halfspace-IDs verwendet und damit NURBS abgeleitet.
>
> Das können wir genauso auf den ursprünglichen (main) Stand anwenden. Wir müssen nur das Sketch-SDF analog erweitern um die Line-Segment-ID und Duale Zahlen. Dann haben wir den aus dem Octree abgeleiteten B-Rep." — *2026-05-31*

**Kernidee:** Die bewährte Octree→Extraction→Weaving→Naming-Pipeline bleibt 3D und unverändert. Neu ist nur, dass die **Sketch-SDF** zusätzlich **Segment-ID + dualen Gradienten** trägt, die über die Extrude/Revolve-Projektion ins 3D-Feld gehoben werden. So füttert man den vorhandenen 3D-Apparat mit getaggten, sketch-erzeugten Flächen — kein separater 2D-B-Rep.

### Spätere Option: parametrische B-Reps „erster Art"

> „Als spätere mögliche Option könnten wir einen Teil der B-Reps direkt aus den Sketch-Körpern erstellen (mittels eigener Algorithmen) und mit den Octree-B-Reps mergen. Vorteil … dass die parametrisch erzeugten B-Reps perfekt sind und daneben die aus dem Octree abgeleiteten B-Reps als B-Reps 2. Art existieren. Nachteil ist, dass das Mergen aufwendig sein könnte — andererseits haben wir bereits ein Verfahren für Naming/ID der Kanten und Flächen, … und über das SDF wissen wir, ob eine Kante im Inneren eines Körpers verschwindet." — *2026-05-31*

---

## 4. Die zentrale Architekturentscheidung: lokal vor global

Hier liegt der wichtigste konzeptionelle Beitrag des Users. Auf die Agenten-Empfehlung, alles 3D im Weltraum zu rechnen („2D ist nur Tagging"), hält er bewusst dagegen — mit dem Argument **lokaler Cache + Änderungshäufigkeit**.

> „Was für die 2D-Betrachtung spricht: 2D-SDF (BVH) aus Sketch kann gecached werden und muss nur neu berechnet werden, wenn der Sketch selbst sich ändert. Auf dem Sketch aufbauende Operationen wie Extrude sind dann billig und führen zu keiner Änderung der SDF, also z.B. **Echtzeit-Dragging des Extrude**.
>
> Die Tatsache, dass jeder Körper seinen eigenen 2D-Raum hat, ist sogar ein Vorteil: Affine Transformationen sollten die körpereigene SDF nicht verändern ⇒ keine Neuberechnung einer BVH. Die globalen 3D-Koordinaten müssen nur entsprechend in den lokalen Raum der jeweiligen Körper transformiert werden, um die globale SDF zu erhalten." — *2026-05-31*

Daraus folgt sein Leitsatz fürs gesamte Feature:

> „**B-Reps sind lokale Phänomene in Bezug auf Bodies.**" — *2026-05-31*

Und die explizite Akzeptanz des Preises dafür:

> „Zu 2 Codepfade: Ja genau, die hätte man dann, aber ich glaube, dass das sinnvoll ist. Hauptargument ist eben global vs. lokal und die Änderungshäufigkeit. Globale Features sind teurer und können sehr von lokal beschleunigten profitieren." — *2026-05-31*

> Das Konzept dient ausdrücklich **nicht nur** Booleans: „Auch Rendering, Meshing, Raum-Transformationen, Gyroid und vor allem B-Reps mittels Octree." Zur Deckel-Problematik der Extrude-Kappen ist er zuversichtlich: „Ich bin mir sicher, dass sich dafür eine einfache geometrische Lösung findet, im Koordinatensystem des Sketch-Körpers." — *2026-05-31*

**Diese Vision wurde später zur Korrektur DL-50 → DL-56:** B-Rep wird körperlokal gewoben, `Body.placement` erst bei der Konsumierung angewandt. Der User hat das World-Space-Baking explizit als Fehler markiert:

> „Das ist genau die falsche Entscheidung. Das ist ein Quick Win gegenüber einer sauberen Lösung, die wir jetzt nacharbeiten müssen." — *2026-06-01*

---

## 5. Booleans und Körper-Trennung

> „Bisher haben wir boolesche Operationen ganz SDF überlassen. Das geht, wenn man körperweise agiert." — *2026-05-31*

Klare, an OnShape orientierte Entscheidung zur Körper-Trennung (kein Cross-Body-Trimming):

> „Solange Körper nicht explizit über Features verbunden sind, sind sie trotz räumlicher Überschneidung als **getrennte Körper** zu behandeln. Das ist explizit so gewünscht und auch Standard OnShape." — *2026-05-31*

---

## 6. Fillets, Chamfers — was (noch) kein B-Rep wird

> „Es ist vorerst korrekt, durch SMin erzeugte **Verrundungen nicht als B-Rep-Flächen** abzubilden. Grund: Fillets sind meist nachgelagerte Features. Anders bei **Chamfers** — dort ist eine Kantenbildung wünschenswert, aber auch das können wir erst mal auf einen zukünftigen Meilenstein verschieben." — *2026-05-31*

---

## 7. Sketch-Primitive: was rein muss

> „Ellipsen sollten ebenfalls enthalten sein (ist Standard in CAD), müssen aber nicht zwangsläufig exakt sein … Splines und Bezier sind aber enthalten? … die Schwelle können wir ggf. anpassen später." — *2026-05-31*

Das wurde zur Entscheidung **Ellipse → Quad-Bézier-Zerlegung** (approximiert, Schwelle justierbar). Splines/Bezier sind Pflicht.

---

## 8. Visualisierung: analytische Kanten sichtbar und hoverbar

Der auslösende Bug-Report und die daraus folgende klare Anforderung:

> „Wird das B-Rep visualisiert? Wenn ich eine Box in der Szene habe, sehe ich keine explizit als Linien gezeichnete Kanten. Wenn ich mit der Maus über die Kanten fahre, werden diese nicht gehighlighted." — *2026-06-01*

> „Die **analytischen Kanten sollen auf jeden Fall visualisiert und hoverbar** sein." — *2026-06-01*

Seine eigene, präzise Unterscheidung „alt vs. analytisch" — die Definition, die den Cleanup-Auftrag begründet:

> „Die abgeleiteten ‚alten' Kanten — damit meinst du Kanten, die **aus dem Mesh abgeleitet** wurden, während die **analytischen Kanten** einerseits **generativ aus den Sketches / Grundformen wie Box** und andererseits **analytisch aus dem Octree / NURBS-Algorithmus** abgeleitet wurden?" — *2026-06-01*

> „Dann ist der ‚alte' Ansatz obsolet und muss entfernt werden. Bitte räume den Code entsprechend auf." — *2026-06-01*

---

## 9. Querschnitt: Arbeitsweise und Qualitätsmaßstab des Users

Diese Leitplanken zog der User wiederholt über das gesamte Feature — sie sind Teil seines Bildes davon, *wie* das Feature entstehen soll:

- **Selbstständige, tragfähige Lösungen statt Quick Wins.**
  > „Suche selbstständig nach einer Lösung, und erweitere das GEOMETRY-KERNEL-CONCEPT.md dann um einen Abschnitt mit Problem und gewählter Lösung. Wichtig ist, dass es saubere und tragfähige Lösungen sind — im Zweifel wird die aufwändigere Lösung dem Quick Win bevorzugt." — *2026-05-30*

- **Sketch-Segmente als Leaf-Nodes in der SDF-Pipeline** (seine eigene Architektur-Vermutung, die sich bestätigt hat):
  > „Ich vermute … dass die SketchSegmente als LeafNodes abgebildet werden müssen, dass es neue SDF-VM-Ops dafür geben muss, und dass die entsprechenden Nodes dann als Datenstrukturen in die SDF-Pipeline gegeben werden müssen." — *2026-05-30*

- **Der Feature-Agent ist „die Wahrheit"** für den neu aufgesetzten Code:
  > „Weil du der Agent bist, der das Feature neu aufsetzt, ist dein Code ‚die Wahrheit'." — *2026-05-30*

- **Autonom durcharbeiten, nur bei echten Unentscheidbarkeiten fragen:**
  > „Mach erst die Ellipse-Zerlegung, und dann arbeite einfach durch, außer du hast Fragen, die du nicht entscheiden kannst." — *2026-05-31*

- **Neuer Branch von aktuellem `main`, Umsetzung an Subagenten delegiert, Review durch den Hauptagenten:**
  > „Ich schlage vor, main zu pullen und dann einen neuen Feature-Branch von main aufzusetzen … Die anschließende Umsetzung Subagenten zu überlassen und deren Arbeit zu kontrollieren erscheint mir wieder optimal." — *2026-05-31*

---

## 10. Das Bild in einem Satz

> Ein echtes CAD baut auf **Sketch-Körpern** (Extrude/Revolve/Helix/Sweep über approximierten Sketches). Splines/Bezier werden segmentiert, naive Half-Spaces sind tot. Das **B-Rep-Ziel** bleibt: aus Octree + dualen Zahlen + ausstrahlenden (Segment-)IDs werden NURBS-Flächen, Kanten und Vertices analytisch extrahiert — aber **körperlokal gerechnet und gecacht**, weil B-Reps lokale Phänomene sind und lokale Caches Echtzeit-Editing erlauben. Getrennte Körper bleiben getrennt (OnShape-Stil), Fillets bleiben vorerst tesselliert, und die analytischen Kanten müssen **sichtbar und hoverbar** sein.
