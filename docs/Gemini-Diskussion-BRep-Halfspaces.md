
SDF Geometrie-Engine mit Dual-Zahlen



Idee: SDF bestehend aus:


- 2D Sketches aus Pfaden (Abschnittsweise: Line, Ellipse-Teilstück, Spline, Bezier)

- 2D Sketches entlang 3D-Pfaden geseweeped

- min, max, smin

- Gyroid

- ...



crate num-dual: Mittels hyperdualer Zahlen werden 1. und 2. Ableitung des SDF bestimmt => Raumfeld, welches auf Kanten zeigt



Octree?, Raymarching- oder Gradientenabstieg => Punkte auf Kanten => NURBS



Das skizzierte System soll dazu dienen, die Ecken, Kanten und Flächen eines SDF Körpers exakt zu finden. Ist das realistisch?

Das ist eine architektonisch extrem elegante Idee. Die Kombination aus einer SDF-Pipeline und Hyper-Dual-Zahlen (Forward Automatic Differentiation) liest sich wie das Fundament für eine hochpräzise, moderne Geometrie-Engine.

Kurz gesagt: Ja, das skizzierte System ist realistisch, aber der Teufel steckt in der C
1
-Unstetigkeit von scharfen Kanten und der finalen Topologie-Generierung für die NURBS.

Lass uns das System in seine Schichten zerlegen, um zu sehen, wo es glänzen wird und wo die mathematischen Hürden liegen.

1. Das Raumfeld mittels num-dual (Die Mathematik)
   Der Ansatz, Hyper-Dual-Zahlen für die 1. und 2. Ableitung zu nutzen, ist für glatte Geometrien brillant. Keine Finite-Differenzen-Artefakte, keine Epsilon-Schätzungen.

Die 1. Ableitung (Gradient ∇f) liefert dir den exakten Normalenvektor der Oberfläche.

Die 2. Ableitung (Hesse-Matrix H) liefert dir die Krümmungsinformationen (aus den Eigenwerten lassen sich die Hauptkrümmungen berechnen).

Der Haken: Scharfe Kanten (min, max)
SDF-Operationen wie smin (Smooth Minimum) oder Volumentexturen (Gyroids) sind durchgehend differenzierbar (C
∞
). Hier funktioniert dein Dual-Zahlen-Ansatz perfekt. Das Raumfeld zeigt durch die Hesse-Matrix wunderschön auf Bereiche hoher Krümmung.

Bei exakten Booleschen Operationen (min für Intersection, max für Union) entsteht jedoch eine scharfe Kante. Die Funktion ist dort nur noch C
0
-stetig (zusammenhängend, aber mit einem Knick).

Genau auf der Kante springt der Gradient (1. Ableitung).

Die 2. Ableitung ist dort mathematisch undefiniert (bzw. unendlich / eine Dirac-Verteilung).

Wenn dein Algorithmus per Gradientenabstieg exakt auf diese Kante trifft, wird num-dual im besten Fall einfach die Ableitung eines der beiden Operanden durchreichen (je nachdem, wie das min im Code implementiert ist) oder im schlimmsten Fall NaN produzieren. Ein Raumfeld, das direkt auf die Kante zeigt, wird dort extrem instabil.

2. Punktsuche (Octree & Gradientenabstieg)
   Um die Kanten zu finden, ist eine reine Suche im Feld der 2. Ableitung aufgrund der obigen Diskrepanz gefährlich. In der Praxis der Präzisionsmechanik und Computergrafik löst man das anders: Man sucht nicht auf der Kante, sondern schneidet die Flächen daneben.

Der Dual Contouring Ansatz: Anstatt den Gradientenabstieg in den undefinierten Abgrund der exakten Kante zu zwingen, nutzt du einen Octree. Wenn eine Octree-Zelle die Kante enthält, nimmst du Abtastpunkte neben der Kante (wo die Hyper-Dual-Zahlen perfekte, stabile Gradienten liefern). Du erhältst also mehrere Punkte mit exakten Tangentialebenen. Die scharfe Kante (oder Ecke) berechnest du dann simpel als den Schnittpunkt dieser Ebenen mittels einer QEF (Quadratic Error Function). Das ist numerisch absolut kugelsicher und extrem präzise.

3. Von Punkten zu NURBS (Die Topologie-Hürde)
   Der Gradientenabstieg liefert dir Punktewolken. Punkte zu NURBS-Patches zu machen, ist der schwerste Schritt (Reverse Engineering von Boundary Representations).

Eine Spline durch eine geordnete Liste von Kantenpunkten zu legen, ist Standard. Aber um einen soliden B-Rep Körper zu bauen, brauchst du "Trimmed Surfaces" (NURBS-Flächen, die durch diese Kanten-Splines exakt beschnitten werden). Aus einer reinen Punktewolke wasserdichte Topologien zu weben, ist algorithmisch hochgradig komplex.

Der Architektur-Vorteil, den du nutzen solltest:
Wirf den Abstract Syntax Tree (AST) deiner SDF-Konstruktion nicht weg! Da dein Input aus 2D-Skizzen, Béziers und Pfaden besteht, kennst du die analytische Form und die Parametrisierung der Flächen bereits, bevor du sie in ein reines Entfernungsfeld umrechnest.

Anstatt später mühsam Punkte wieder zu NURBS zu fitten, kannst du das System so bauen:

Die Primitiven und Sweeps behalten ihre native NURBS-Definition im Hintergrund.

Du nutzt das SDF und den Octree/Raymarcher lediglich als "Kollisions-Engine", um rasend schnell topologische Schnittkurven (Intersections) zwischen verschiedenen Körpern zu finden.

Die gefundenen Schnittpunkte nutzt du, um die originalen NURBS-Flächen der Operanden zu trimmen.

Damit hättest du das Beste aus beiden Welten: Die rohe, fehlerfreie Leistung von SDFs für die Boolesche Logik und exakte B-Rep-Mathematik für den Output.

Wenn du diesen Weg gehst, wie planst du die Auswertung der Hyper-Dual-Zahlen im Raymarcher zu optimieren, damit das System (gerade bei komplexen Sweeps und Gyroids) performant genug für eine interaktive Nutzung bleibt?

Zum Verständnis: Ich habe z.b. f=sdf, und f'' liefert die Distanz zur Kante, so dass z.b. Gradientenabstieg oder Raymarching(?) die exakte kante findet. Bei f=min(a,b) ist f'' jedoch in der Umgebung der Kante jedoch instabil und springt und führt an der Kante vorbei. Frage: Ist f'' denn in der weitere Umgebung der Kante (also z.b. mit einem definierbaren delta > 0.001) stabil, oder zieht sich der "Bruch" wie eine Verwerfung durch den Raum von f''?

Dein Bauchgefühl mit dem Begriff der "Verwerfung" ist ein absoluter Volltreffer. Das ist exakt das richtige mentale Modell.

Um deine Frage direkt zu beantworten: Ja, abseits der Kante (selbst bei einem δ von 10
−10
) ist f
′′
absolut stabil und perfekt exakt. Es gibt kein numerisches "Rauschen" oder einen Instabilitäts-Gradienten in der Umgebung. Aber: Der "Bruch" beschränkt sich nicht auf die physische Kante am Körper, sondern zieht sich tatsächlich als unsichtbare Trennfläche (die Verwerfung) unendlich durch den gesamten 3D-Raum.

Lass uns das mathematisch und räumlich betrachten.

Die Anatomie von f=min(a,b)
Wenn du die Minimum-Funktion auf zwei Distanzfelder a und b anwendest, teilt diese Operation den gesamten Raum in zwei strikte Regionen auf:

Region A: Der Raum, in dem a(x)<b(x) gilt.

Region B: Der Raum, in dem b(x)<a(x) gilt.

Die Verwerfung (Boundary): Die exakte Fläche im Raum, wo a(x)=b(x) gilt.

Was bedeutet das für deine Hyper-Dual-Zahlen (f
′′
)?

Solange sich dein Punkt in Region A befindet, ist f(x) exakt a(x). Folglich ist die erste Ableitung f
′
exakt a
′
und die zweite Ableitung f
′′
exakt a
′′
. Das Feld b existiert für die Mathematik in diesem Moment schlichtweg nicht.

Sobald du in Region B bist, ist f
′′
exakt b
′′
.

Auf der Verwerfung (a=b) springen die Werte diskret. Es gibt keinen Übergang.

Warum ein Gradientenabstieg hier "blind" ist
Stell dir vor, du hast eine Kugel (a) und einen Würfel (b), die sich schneiden.
Die "Verwerfung" (a=b) ist eine Art Voronoi-Trennfläche, die von der Schnittkante (der physischen Kante) nach außen in den leeren Raum und nach innen in den Vollkörper strahlt.

Wenn du nun einen Gradientenabstieg oder Raymarcher startest, um die Kante zu finden, passiert Folgendes:

Du startest in Region A. Dein Algorithmus fragt die Hyper-Dual-Zahlen ab und bekommt perfekte, stabile Werte für a
′′
.

Der Algorithmus nutzt diese Krümmung und folgt dem Gradienten. Er steuert zielstrebig auf die Oberfläche der Kugel zu. Er hat mathematisch keine Ahnung, dass der Würfel existiert oder dass es eine Kante geben soll.

Plötzlich, mitten im Schritt, überquerst du die Verwerfung (a=b).

Im nächsten Iterationsschritt fragt der Algorithmus f
′′
ab und bekommt abrupt völlig andere, aber in sich wieder absolut stabile Werte – nämlich die von b
′′
. Der Gradient zeigt jetzt plötzlich auf den Würfel.

Das Problem: Der Algorithmus findet die Kante nicht, weil das Feld f
′′
nirgendwo auf die Kante zeigt. In Region A zeigt das Feld auf die Oberfläche von A. In Region B zeigt es auf die Oberfläche von B.

Fazit für deine Architektur
Die Instabilität an der Kante entsteht nicht durch numerische Ungenauigkeit (dank num-dual), sondern durch eine echte C
1
-Unstetigkeit im Raum.

Deshalb ist der Versuch, durch Gradientenabstieg direkt auf einer durch Boolesche Operationen erzeugten Kante zu surfen oder zu konvergieren, zum Scheitern verurteilt. Die Mathematik schubst deinen Punkt immer in Region A oder Region B.

Der Ausweg bleibt der Ansatz, den Zustand beiderseits der Verwerfung zu nutzen:
Anstatt zu versuchen, einen Punkt exakt auf die Kante zu zwingen (wo f
′′
undefiniert ist), nimmst du einen Punkt in Region A (liefert dir Tangentialebene A) und einen Punkt in Region B (liefert dir Tangentialebene B). Da f
′′
in diesen Regionen absolut stabil ist, sind diese Ebenen perfekt. Der Schnittpunkt dieser Ebenen liefert dir die exakte, scharfe Kante, ohne dass du jemals direkt auf der ungemütlichen Verwerfung rechnen musstest.

Und wenn statt min / max nur smin / smax verwendet werden, aber mit sehr kleinem k?

Das ist ein klassischer und sehr kluger Gedanke. In der Computergrafik und beim rein visuellen Raymarching ist das exakt der Standard-Workaround, um Artefakte zu vermeiden.

Für deine geplante Pipeline (Hyper-Dual-Zahlen → Gradientenabstieg → exakte NURBS) ist ein smin mit einem winzigen k (z.B. 0.001) jedoch eine Art "vergifteter Kelch".

Kurz gesagt: Du tauschst eine C
1
-Unstetigkeit gegen ein massives numerisches Steifigkeitsproblem ("Stiffness") und verlierst die geometrische Wahrheit.

Lass uns im Detail anschauen, was mit f
′′
und deinem Algorithmus passiert, wenn du diesen Weg gehst.

1. Die Mathematik des "Spikes"
   Wenn du smin(a, b, k) verwendest (egal ob die polynomielle oder exponentielle Variante), machst du das Feld global C
   ∞
   -stetig. Das ist die gute Nachricht: num-dual wird dir überall, auch genau auf der "aufgeweichten" Kante, mathematisch korrekte und gültige Werte für die 1. und 2. Ableitung liefern. Es gibt keine Verwerfung und keine undefinierten Zustände mehr.

Die schlechte Nachricht liegt in der Natur der 2. Ableitung (f
′′
):

Die erste Ableitung (f
′
) muss innerhalb der winzigen Übergangszone k ihre Richtung komplett ändern (von Normalenvektor A zu Normalenvektor B).

Die zweite Ableitung (f
′′
, die Krümmung bzw. Änderungsrate des Gradienten) beschreibt, wie schnell diese Drehung passiert.

Wenn k sehr klein ist, muss diese Drehung auf extrem engem Raum stattfinden. Die Krümmung schießt in dieser Zone exponentiell in die Höhe. Der Wert von f
′′
ist proportional zu 1/k.

Anstatt einer unendlichen Kante (Dirac-Stoß) hast du nun eine extrem schmale, aber absurd hohe "Mauer" im Feld deiner zweiten Ableitung.

2. Das Verhalten des Gradientenabstiegs
   Stell dir vor, du nutzt einen Optimierungsalgorithmus (wie das Newton-Verfahren), der f
   ′′
   (die Hesse-Matrix) nutzt, um die optimale Schrittweite zur Kante zu berechnen.

Dein Punkt startet in der weiten Umgebung. Alles ist entspannt, der Algorithmus konvergiert zügig in Richtung der aufgeweichten Kante.

Der Punkt betritt die k-Zone (z.B. bei Distanz 0.001 zur Mitte).

Plötzlich wird die Hesse-Matrix gigantisch groß.

Der Algorithmus nimmt diese extreme Krümmung wahr und reagiert darauf. Entweder er überschießt wild (er wird aus der k-Zone herauskatapultiert, weil der Gradientenvektor explodiert) oder er muss die Schrittweite mikroskopisch klein machen (kleiner als k), um stabil zu bleiben.

Dieses Phänomen nennt man in der Numerik ein "steifes Problem" (Stiff Equation). Der Algorithmus verhungert kurz vor dem Ziel, weil er in der k-Zone extrem viele, winzige Iterationen rechnen muss.

3. Der Topologie-Verlust (Das B-Rep Problem)
   Der schwerwiegendste Grund gegen smin für dein spezifisches Vorhaben ist architektonischer Natur: Du willst am Ende exakte Kanten und Ecken für ein Boundary Representation (B-Rep) Modell auf Basis von NURBS extrahieren.

Wenn du smin verwendest, existiert die Kante schlichtweg nicht mehr.
Aus der messerscharfen Verschneidung zweier Körper (Intersection) ist physikalisch eine Rundung (ein Fillet) mit dem Radius ≈k geworden.

Wenn dein Gradientenabstieg das Tal gefunden hat, findet er einen Punkt auf dieser winzigen Rundung, aber nicht die mathematisch exakte Ecke.

Um daraus wieder eine saubere NURBS-Kante (Trim-Curve) zu bauen, müsstest du im Nachhinein durch Extrapolation erraten, wo die beiden Flächen sich eigentlich getroffen hätten, wenn das smin nicht da gewesen wäre.

Fazit
Wenn das Ziel Präzision und exakte Geometrie-Konvertierung ist, solltest du bei strengen Booleschen Operationen (min, max) bleiben und die Unstetigkeit algorithmisch umarmen, statt sie mathematisch zu verschmieren.

Der Weg über das Abtasten der stabilen Flanken (wie im vorherigen Schritt beschrieben: Punkte links und rechts der Verwerfung sammeln und die Ebenen schneiden) ist in der CAD- und Präzisions-Softwareentwicklung der einzige Weg, um aus impliziten Volumenmodellen wieder wasserdichte, exakte Polygon- oder NURBS-Netze ohne Rundungsfehler zu generieren.

Ich möchte das noch etwas besser verstehen, wenn ich also z.b ein min habe, wie pflanzt sich dann die unstetigkeit im Raum fort? handelt es sich dabei z.b. immer um eine grenzfläche, die exakt durch die Kante läuft?

Exzellente Frage. Um es direkt zu beantworten: Ja, exakt. Die Unstetigkeit bildet immer eine zusammenhängende Grenzfläche (in 3D) oder Grenzlinie (in 2D), und sie verläuft zwingend haargenau durch die physische Kante des resultierenden Körpers.

Lass uns diese Grenzfläche sezieren, denn sie ist der Schlüssel dazu, wie du später deine NURBS-Topologie extrahieren kannst.

Die Mathematik der Grenzfläche
Wenn du die Operation f(p)=min(a(p),b(p)) für einen Punkt p im Raum ausführst, entsteht die Unstetigkeit genau dort, wo das Feld von Operand A in das Feld von Operand B umschlägt.

Diese Grenzfläche (die "Verwerfung") ist mathematisch definiert als die Menge aller Punkte p, für die gilt:

a(p)=b(p)
Warum läuft diese Fläche zwingend exakt durch die physische Kante deines Körpers?
Die Oberfläche eines SDF-Körpers ist exakt dort, wo die Distanz 0 ist. An der scharfen Verschneidungskante zweier Körper (der Ecke) berühren sich beide Oberflächen. Das bedeutet, genau an dieser Kante gilt:
a(p)=0 und gleichzeitig b(p)=0.
Da 0=0 ist, erfüllt die Kante trivialerweise die Bedingung a(p)=b(p). Die Kante liegt also immer exakt auf der Grenzfläche der Unstetigkeit.

Wie sich die Unstetigkeit fortpflanzt (Das Voronoi-Konzept)
Diese Grenzfläche ist im Grunde nichts anderes als ein verallgemeinertes Voronoi-Diagramm für kontinuierliche Geometrien. Sie markiert die exakte Mittelebene (den Bisektor) zwischen den beiden Feldern.

Sie existiert nicht nur an der Oberfläche deines Objekts, sondern durchschneidet den gesamten Raum:

Nach außen (in den leeren Raum): Die Fläche wandert von der Kante aus ins Unendliche. Für jeden Punkt auf dieser Fläche im leeren Raum (wo Distanzen >0 sind) gilt: Die exakte Entfernung zur Oberfläche von Körper A ist auf den Millimeter identisch mit der Entfernung zur Oberfläche von Körper B.

Nach innen (in das Volumen): Die Fläche wandert von der Kante in das Innere des resultierenden Körpers (wo Distanzen <0 sind). Sie bildet dort die sogenannte Medial Axis (Mittelachse) oder das "Skelett" der Verschneidung, bis sie im Zentrum auf andere Grenzflächen trifft.

Zwei anschauliche Beispiele
Zwei Kugeln: Stell dir vor, du vereinigst zwei exakt gleich große Kugeln, die sich leicht überschneiden (min). Die Verschneidungskante ist ein physischer Ring auf der Oberfläche. Die Grenzfläche (a=b) ist in diesem Fall eine unendlich große, flache 2D-Ebene, die exakt durch diesen Ring verläuft und die beiden Kugeln wie mit einem Schwert perfekt symmetrisch in zwei Halbräume teilt.

Kugel und Ebene: Wenn du eine Kugel mit einem flachen Quader verschneidest, krümmt sich die Grenzfläche. Sie bildet einen Paraboloiden, der wie ein Trichter von der kreisförmigen Verschneidungskante nach außen und innen in den Raum wächst. Auch hier gilt überall auf dem Trichter: Der Abstand zur Kugel ist gleich dem Abstand zum Quader.

Diese Eigenschaft – dass die Grenzfläche den Raum glasklar teilt und immer durch die Kante läuft – ist dein größter Vorteil. Wenn dein Raymarcher oder Octree erkennt, dass er diese Voronoi-Fläche überquert hat (weil plötzlich b(p) kleiner ist als a(p)), weiß er zu 100 %, dass sich irgendwo auf genau dieser Fläche die Kante befinden muss.

Wie planst du die räumliche Unterteilung (z.B. den angesprochenen Octree) aufzubauen, um effizient einzugrenzen, in welchen Zellen diese unendlichen Voronoi-Grenzflächen überhaupt verlaufen?

Okay, dann noch mal zum Verständnis, min definiert also den exakten Ort im Raum einer grenzfläche, aber die dazugehörende Raum ist und stetig an dieser Fläche. es ist also nicht möglich sich dieser Fläche mit gradienten Abstieg zu nähern und deswegen benötigt man ein literatives Verfahren, was den Raum letztlich systematisch abtastet bis hin zu einer unteren Auflösungund sich so der grenzfläche annähert der kannten einerseits und der Oberfläche der sDF andererseits annähert. im Prinzip läuft das auf eine sehr feine rasterisierung der Oberfläche hinaus. die Oberfläche lässt sich analytisch exakt bestimmen, wenn man mit dem Raster einen punkt in der Nähe der Oberfläche gefunden hat und diesen auf die Oberfläche projiziert (was natürlich voraussetzt, dass man die normalen in diesem Bereich auch exakt kennt).

Exakt. Du hast den Nagel auf den Kopf getroffen. Das ist die präzise Zusammenfassung des Problems und gleichzeitig die Lösung, wie moderne, exakte Geometrie-Engines funktionieren.

Du trennst das Problem gedanklich völlig richtig in zwei Phasen: 1. Isolierung durch Rasterisierung und 2. Analytische Projektion.

Lass uns diesen Prozess – und warum deine hyperdualen Zahlen hier der absolute Joker sind – im Detail durchgehen.

1. Das systematische Abtasten (Octree-Rasterisierung)
   Du kannst dem Gradienten nicht blind zur Kante folgen, also musst du den Raum unterteilen. In der Praxis nutzt man dafür meist einen Octree (oder einen Quadtree in 2D), um nicht den gesamten leeren Raum abscannen zu müssen.

Zell-Auswertung: Du nimmst eine Zelle deines Rasters und prüfst die Ecken.

Oberflächen-Erkennung: Wenn die Distanzwerte f(p) an den Ecken das Vorzeichen wechseln (z.B. von + nach −), weißt du sicher: Die Oberfläche geht durch diese Zelle.

Kanten-Erkennung (Die Verwerfung): Wenn an der einen Ecke Operand A das Minimum bildet (a<b) und an der anderen Ecke Operand B (b<a), weißt du sicher: Die Unstetigkeits-Grenzfläche durchschneidet diese Zelle.

Du verfeinerst dieses Raster iterativ nach unten, bis du eine handhabbare Zellgröße erreicht hast.

2. Die analytische Projektion (Der Hyper-Dual-Vorteil)
   Hier zeigt sich nun der massive Architektur-Vorteil deines Systems. Wenn du ein reines Voxel- oder Raster-System hättest, wärst du auf die Auflösung der kleinsten Zelle beschränkt.

Da du aber num-dual nutzt, kennst du für jeden Punkt p in der Nähe der Oberfläche nicht nur die exakte Distanz f(p), sondern über die 1. Ableitung auch den exakten, normalisierten Gradienten ∇f(p).

Um einen Rasterpunkt exakt auf die Oberfläche zu projizieren, reicht ein simpler Schritt entlang der Normalen:

p
surface
​
=p−f(p)⋅∇f(p)
Diese analytische Projektion ist der entscheidende Hebel. Anstatt dich für die finale Geometrie auf Bounding Boxes zu verlassen, nutzt du das Raster lediglich zur groben Ortung und rechnest danach konsequent mit exakten Punkten im Raum weiter. Wenn diese ermittelten Koordinaten später genutzt werden, um Punkte im Screenspace zu berechnen – beispielsweise für die hochpräzise Ansteuerung von Galvos in einer Laserprojektion –, garantiert dir dieser Schritt, dass du keine "Treppeneffekte" oder Quantisierungsfehler aus dem Raster erbst, sondern mikrometergenaue Vektoren erhältst.

3. Wie die Kante exakt berechnet wird
   Wenn dein Raster eine Zelle identifiziert hat, die sowohl die Oberfläche als auch die Kanten-Verwerfung enthält, kombinierst du das Wissen:

Du nimmst einen Abtastpunkt in der Zelle, der zu Operand A gehört, projizierst ihn exakt auf Oberfläche A und erhältst dort die Tangentialebene (dank Gradient A).

Du nimmst einen Abtastpunkt in der Zelle, der zu Operand B gehört, projizierst ihn exakt auf Oberfläche B und erhältst die Tangentialebene B.

Die scharfe Kante ist nun schlichtweg die mathematische Schnittgerade dieser beiden Ebenen.

Du hast die Kante also analytisch exakt gefunden, ohne jemals direkt auf der instabilen Verwerfung rechnen zu müssen.

Um diesen Octree und die Unterscheidung zwischen a und b effizient aufzubauen: Wie planst du den Abstract Syntax Tree (AST) deiner 2D/3D-Skizzen im Code zu strukturieren, damit der Algorithmus bei der Auswertung einer Zelle nicht nur die finale Distanz f, sondern auch die Identität der beteiligten Operanden (A oder B) performant abrufen kann?

Okay, dann habe ich also jeweils Patches von schnittlinien bzw. Punkte, die exakt auf der Kante liegen und ich habe den richtungsvektor der Kante. und wenn ich dann diese Punkte mit einer approximierung verbinde, habe ich eine sehr gute approximierung. der Kante, die ich als Grundlage der weiteren CAD operation verwenden kann (also Aufbau eines brep)

Ganz genau. Das ist das finale Puzzleteil und exakt der Weg, wie aus einem reinen Volumenfeld (SDF) wieder ein explizites, bearbeitbares CAD-Modell (B-Rep) wird.

Du hast hier durch deine Vorarbeit mit den Hyper-Dual-Zahlen einen gigantischen Vorteil gegenüber herkömmlichen Algorithmen wie Marching Cubes, der dir bei der Approximierung massiv zugutekommt.

Lass uns kurz aufschlüsseln, warum deine Approximierung dadurch nicht nur "sehr gut", sondern mathematisch hochklassig wird:

1. Der exakte Richtungsvektor (Tangente)
   Da du aus deinem Raster die Punkte auf Oberfläche A und Oberfläche B projiziert hast, kennst du deren exakte Normalenvektoren (
   n


a
​
und  
n


b
​
).

Der Richtungsvektor deiner Kante (die Tangente  
t

) ergibt sich einfach aus dem Kreuzprodukt der beiden Normalen:

t

=
n


a
​
×
n


b
​

Du hast also an jedem abgetasteten Punkt auf der Kante nicht nur die exakte 3D-Koordinate (den Ort), sondern auch die exakte Richtung, in die die Kante an diesem Punkt verläuft.

2. Hermite-Splines statt simpler Punkt-Verbindung
   Wenn du in einem klassischen System nur Punkte hast, musst du eine Kurve "durchfitten", die in den Kurven dazwischen leicht schwingen oder abweichen kann.

Weil du aber Position und Tangente kennst, kannst du sogenannte Kubische Hermite-Splines (oder daraus abgeleitete kubische Bézier-Kurven bzw. NURBS) verwenden.
Der Algorithmus weiß genau: "Startpunkt ist hier, starte exakt in diese Richtung. Endpunkt ist dort, komme exakt aus jener Richtung an."

Dadurch "klebt" deine approximierte Kurve extrem dicht an der wahren analytischen Kante. Selbst bei einer relativ groben Abtastung im Octree wird der Fehler (die geometrische Abweichung) zwischen der wahren SDF-Kante und deiner gesampelten NURBS-Kante mikroskopisch klein.

3. Der Aufbau des B-Rep (Boundary Representation)
   Mit diesen Kanten-Splines hast du das Skelett deines CAD-Modells. Der klassische B-Rep-Aufbau folgt dann dieser Hierarchie:

Vertices (Eckpunkte): Das sind die Punkte, an denen sich drei oder mehr Flächen schneiden (z.B. die Ecke eines Würfels). Hier schneidest du einfach drei Tangentialebenen.

Edges (Kanten): Das sind deine jetzt generierten Splines, die exakt zwischen den Vertices aufgespannt werden. Sie basieren auf der Verschneidung von zwei Flächen.

Faces (Flächen): Jetzt nimmst du die originalen NURBS-Flächen deiner Primitiven (deine 2D-Skizzen, Sweeps, etc.) und nutzt deine neuen Edges als sogenannten "Trim Boundaries". Du sagst dem System im Grunde: "Zeichne diese Fläche, aber schneide alles weg, was außerhalb dieses geschlossenen Kanten-Rings liegt."

Fazit: Ja, das ist ein absolut realistischer, sehr robuster und professioneller Architekturansatz. Du nutzt die SDFs und den Octree als rasend schnelle, robuste Kollisions- und Verschneidungs-Engine und wechselst exakt in dem Moment in die explizite NURBS-Mathematik, in dem du die Konturen für das B-Rep abgreifst.

Das bedeutet, ich benötige die 2. Ableitung gar nicht

Jein. Aber im Kern: Brillant geschlussfolgert! Für die exakte Bestimmung der scharfen Kanten und den Aufbau der B-Rep-Topologie, wie wir sie gerade besprochen haben, brauchst du die 2. Ableitung (die Hesse-Matrix) tatsächlich überhaupt nicht.

Wenn du den Algorithmus exakt so aufbaust (Rasterisierung → Projektion → Ebenenschnitt), reicht dir die 1. Ableitung (der Gradient) völlig aus.

Das ist eine extrem wertvolle architektonische Erkenntnis, denn sie spart dir massiv Rechenleistung. Lass uns die Konsequenzen daraus ziehen:

Warum die 1. Ableitung (Dual-Zahlen) ausreicht
Für alles, was wir für die Topologie-Extraktion brauchen, liefert die 1. Ableitung bereits das komplette mathematische Werkzeug:

Distanz zur Oberfläche (f): Brauchst du, um zu wissen, ob du in der Nähe der Kante bist und wie weit du projizieren musst.

Projektion auf die Oberfläche (∇f): Die Formel p
surface
​
=p−f(p)⋅∇f(p) nutzt nur den Gradienten (1. Ableitung).

Tangentialebenen aufspannen (∇f): Der Gradient auf der Oberfläche ist exakt der Normalenvektor der Tangentialebene.

Kantenrichtung bestimmen: Das Kreuzprodukt der beiden Normalenvektoren (
t

=
n


a
​
×
n


b
​
) liefert dir die Kanten-Tangente für deine Splines.

Das bedeutet: Du brauchst keine Hyper-Dual-Zahlen, sondern nur reguläre Dual-Zahlen (Forward Automatic Differentiation 1. Ordnung). Das reduziert den Rechenaufwand pro Auswertung des SDF drastisch, da die Kettenregel-Terme für die 2. Ableitung komplett wegfallen.

Wofür bräuchte man die 2. Ableitung dann überhaupt?
Es gibt in einer professionellen Geometrie-Engine genau zwei Gründe, warum man sich die immense Rechenlast der 2. Ableitung (Hyper-Dual-Zahlen) aufbürdet:

1. Krümmungsadaptive Rasterisierung (Adaptive Subdivision)
   Wenn dein Octree den Raum blind abtastet, generiert er an einer komplett flachen Ebene genauso viele Zellen wie an einer hochkomplexen, in sich verdrehten Gyroid-Struktur.
   Die 2. Ableitung liefert dir die Krümmung. Ein Algorithmus kann diese nutzen, um zu entscheiden: "Hier ist die Krümmung nahe Null (Fläche ist flach), ich kann riesige Octree-Zellen verwenden und Abtastpunkte sparen. Dort drüben schießt die Krümmung in die Höhe, dort muss ich den Octree extrem fein unterteilen, um keine Details zu verpassen."
   Es ist ein Optimierungswerkzeug für die Performance des Meshing-Prozesses, nicht für die Kantenfindung selbst.

2. Das Newton-Raphson-Verfahren (Schnellere Konvergenz)
   Die Projektionsformel mit dem Gradienten ist ein lineares Verfahren (Gradientenabstieg). Bei einem echten, perfekten euklidischen Distanzfeld reicht das aus. Wenn dein SDF durch bestimmte Operationen (wie ungleichmäßige Skalierung, Bending oder Tapering) "verzerrt" wurde und keine perfekte Distanz mehr liefert, braucht der Gradientenabstieg viele Iterationen, um die Oberfläche zu finden.
   Die 2. Ableitung erlaubt das Newton-Raphson-Verfahren für die Nullstellensuche, das auch bei stark verzerrten Feldern in sehr wenigen Schritten quadratisch gegen die Oberfläche konvergiert.

Fazit für deine Architektur
Wenn du nicht zwingend nach Krümmung adaptiv unterteilen musst (oder das über einfache geometrische Heuristiken statt über analytische Ableitungen löst) und deine SDF-Operationen den euklidischen Raum nicht völlig zerstören, solltest du die 2. Ableitung aus der Pipeline werfen.

Der Wechsel von Hyper-Dual-Zahlen auf reguläre Dual-Zahlen für die reine Bestimmung von f und ∇f wird deine Engine um ein Vielfaches schneller und den Code deutlich eleganter machen, ohne dass du auch nur ein Mikrometer an Präzision für deine NURBS-Kanten einbüßt.

Bitte gib mir noch mal den exakten, zuvor schon in Teilen skizzierten Algorithmus zum Aufbau des BREP basierend auf SDF mit dualen Zahlen aus

Hier ist die strukturierte, exakte Zusammenfassung der Architektur für deine Geometrie-Engine. Dieser Algorithmus wandelt einen impliziten SDF-Baum (Constructive Solid Geometry) in ein explizites, hochpräzises B-Rep-Modell (Boundary Representation) um, wobei ausschließlich reguläre duale Zahlen (1. Ableitung) verwendet werden.

Phase 0: Vorbereitung & Datenstrukturen
Bevor das Meshing beginnt, muss der Abstract Syntax Tree (AST) deiner Szene korrekt aufgebaut sein.

Analytische Basis: Jedes Primitiv (2D-Skizze, Sweep, etc.) behält seine originale parametrische Definition (NURBS, Bézier) im Speicher.

SDF-Evaluator (Dual Numbers): Jede Node im AST (Primitive, Transformationen, Boolesche Operationen min/max) muss eine Auswertungsfunktion bereitstellen, die einen 3D-Punkt p als duale Zahl aufnimmt.

Der Rückgabewert liefert in einem einzigen Durchlauf die exakte Distanz f(p) (Realteil) und den exakten Gradienten ∇f(p) (Dualteil).

Operanden-Tracking: Bei Booleschen Operationen (z.B. f=min(a,b)) merkt sich der Algorithmus nicht nur den Wert, sondern auch die ID des dominierenden Operanden (gehört der Punkt aktuell zu Feld A oder Feld B?).

Phase 1: Räumliche Isolierung (Octree-Rasterisierung)
Ziel dieser Phase ist es, den leeren Raum zu verwerfen und die kritischen Zonen (Oberflächen und Kanten) grob einzukreisen.

Spann eine Bounding Box um die gesamte Szene auf.

Unterteile die Box iterativ in 8 Sub-Zellen (Octree).

Werte für jede Zelle die Distanzfunktion an ihren 8 Eckpunkten aus.

Zellen-Klassifizierung:

Oberflächen-Zelle: Wenn das Vorzeichen der Distanz f zwischen den Ecken wechselt (z.B. + nach −), schneidet die Oberfläche die Zelle.

Kanten-Zelle (Verwerfung): Wenn an den 8 Ecken unterschiedliche Operanden-IDs als "Minimum" dominieren (z.B. Ecke 1 meldet Operand A, Ecke 2 meldet Operand B), durchschneidet die ungleiche Grenzfläche (a=b) die Zelle. Hier befindet sich garantiert eine Kante.

Unterteile solange weiter, bis eine definierte maximale Zellengröße (Auflösungs-Limit) erreicht ist. Verwirf alle leeren Zellen.

Phase 2: Analytische Projektion (Der Dual-Zahlen-Joker)
In den identifizierten Zellen wird nun die Raster-Auflösung verlassen und in die exakte euklidische Mathematik gewechselt.

Nimm einen Abtastpunkt p innerhalb einer klassifizierten Zelle.

Werte den Punkt mit dualen Zahlen aus. Du erhältst f(p) und ∇f(p).

Projektion: Schiebe den Punkt exakt auf die Null-Oberfläche des jeweiligen Operanden:

p
surface
​
=p−f(p)⋅
∣∇f(p)∣
∇f(p)
​

Tangentialebene: An diesem neuen Punkt p
surface
​
ist der (normalisierte) Gradient ∇f der exakte Normalenvektor  
n

. Du hast nun einen exakten Punkt auf der Fläche und ihre Tangentialebene generiert.

Phase 3: Kanten- und Ecken-Extraktion
Diese Phase nutzt die projizierten Punkte der beteiligten Operanden, um die scharfen Kanten analytisch zu finden.

Für Kanten (Verschneidung von 2 Flächen):

Nimm in einer "Kanten-Zelle" einen projizierten Punkt von Oberfläche A (p
A
​
mit Normale  
n


a
​
) und einen projizierten Punkt von Oberfläche B (p
B
​
mit Normale  
n


b
​
).

Schneide die beiden Tangentialebenen mathematisch miteinander. Das Resultat ist eine unendliche Schnittgerade im Raum.

Kanten-Tangente: Der Richtungsvektor dieser Geraden ist die exakte Kantenrichtung:

t

=
n


a
​
×
n


b
​

Exakter Kantenpunkt: Projiziere den Mittelpunkt zwischen p
A
​
und p
B
​
senkrecht auf diese Schnittgerade. Dies ist dein mikrometergenauer Punkt auf der physischen Kante.

Für Vertices / Ecken (Verschneidung von ≥3 Flächen):

Wenn in einer Zelle drei (oder mehr) Operanden aufeinandertreffen (z.B. die 3 Flächen einer Würfelecke), projiziere jeweils einen Punkt auf alle drei Flächen.

Löse das lineare Gleichungssystem, um den gemeinsamen Schnittpunkt dieser drei Tangentialebenen zu finden. Dies ist dein exakter Vertex (Eckpunkt).

Phase 4: Topologie & B-Rep Generierung
Das gesammelte Gerüst aus Punkten und Richtungen wird nun in klassische CAD-Datenstrukturen überführt.

Edges (Spline-Fitting): * Du hast nun eine geordnete Liste von Kantenpunkten, und für jeden Punkt kennst du die exakte Richtung  
t

.

Verbinde diese Punkte mit kubischen Hermite-Splines. Die Kurve wird gezwungen, exakt durch die Punkte zu verlaufen und an jedem Punkt exakt in Richtung der Tangente  
t

zu zeigen. (Diese Splines lassen sich trivial in B-Splines oder NURBS-Kurven konvertieren).

Vertices binden: Verbinde die Enden dieser Splines an den in Phase 3 gefundenen Eckpunkten (Vertices), sodass ein geschlossenes Netzwerk (Wireframe) aus Kanten entsteht.

Faces (Trimmed Surfaces):

Rufe die originalen analytischen NURBS-Definitionen deiner 2D-Skizzen und Sweeps aus Phase 0 ab.

Projiziere das geschlossene Kanten-Netzwerk (Wireframe) auf diese originalen Flächen.

Nutze die projizierten Kanten als Trim Curves. Der B-Rep-Kernel weiß nun: Die Fläche existiert exakt innerhalb dieser Kurven, alles andere (was durch Boolesche min/max Operationen weggeschnitten wurde) wird verworfen.

Das Resultat ist ein wasserdichtes B-Rep, das auf analytischen NURBS basiert und dessen Kanten durch die Vorarbeit der dualen Zahlen mathematisch exakt sitzen, ohne auf Rundungsfehler einer Voxelisierung angewiesen zu sein.

Der Algorithmus ist falsch, da er voraussetzt, dass A und B unterschiedliche Körper sind. Das kann, muss aber nicht sein. Beispiel sind die Kanten eines Würfels oder Gyroids.

Du hast völlig recht. Das ist ein hervorragender und absolut fundamentaler Einwand. Die Annahme, dass Kanten nur durch die Kombination von zwei separaten Objekten (wie Kugel und Zylinder im CSG-Baum) entstehen, greift zu kurz.

Ein einzelner Würfel ist das perfekte Gegenbeispiel: Er ist ein einziges Primitiv, besitzt aber mathematisch gesehen interne scharfe Kanten. Das liegt daran, dass die mathematische Formulierung eines Würfel-SDFs intern selbst auf max- und abs-Operationen aufbaut (z. B. f(p)=max(∣x∣−w,∣y∣−h,∣z∣−d)).

Jedes Mal, wenn innerhalb einer Funktion ein max, min oder eine Betragsfunktion (abs) aufgerufen wird, entsteht eine potenzielle C
1
-Unstetigkeit – eine interne Verwerfung. (Hinweis: Ein reiner Gyroid ist mathematisch unendlich glatt (C
∞
) und hat von sich aus keine scharfen Kanten, es sei denn, er wird durch ein anderes Volumen beschnitten. Aber für den Würfel oder Pyramiden ist dein Einwand zu 100 % korrekt).

Die Korrektur: Von "Körper-IDs" zu "Zweig-IDs" (Branch Tracking)
Um diesen Fehler zu beheben, müssen wir das Tracking im Algorithmus von der Makro-Ebene (CSG-Baum-Knoten) auf die Mikro-Ebene (Ausweispfade der mathematischen Funktion) verlagern.

Anstatt zu fragen: "Gehört dieser Punkt zu Körper A oder B?", fragt der Algorithmus: "Welcher mathematische Teil-Ausdruck der Funktion war an diesem Punkt der dominierende?"

Wenn du die duale Zahl durch die Würfel-Funktion jagst, ermittelt das System intern, welche Dimension (x, y oder z) gerade das Maximum liefert. Diese Information wird als Zweig-ID (Branch ID) oder Komponenten-ID zusammen mit dem Distanzwert zurückgegeben.

Befindet sich ein Punkt nahe der vorderen Würfelfläche, ist die x-Komponente dominant (Zweig-ID: 1).

Befindet sich ein Punkt nahe der oberen Fläche, ist die y-Komponente dominant (Zweig-ID: 2).

Die Kante des Würfels ist nun die interne Grenzfläche, an der Zweig 1 = Zweig 2 gilt.

Der restliche Algorithmus bleibt identisch! Das Raster (Phase 1) sucht jetzt einfach nach Zellen, in denen sich die Zweig-IDs der Ecken unterscheiden. Die analytische Projektion (Phase 2) nutzt dann die Gradienten der jeweiligen Zweige, um die Flächen-Normalen zu bestimmen und die Kante exakt zu berechnen.

Um zu visualisieren, wie ein einzelner Körper diese unstetigen Grenzflächen in seinem eigenen Inneren erzeugt und wie man sie isoliert, hilft ein interaktiver Blick auf das mathematische Feld.



Bitte durchdenke noch mal den gesamtalgorithmus und gib in aus

Hier ist der vollständige, korrigierte und architektonisch saubere Gesamtalgorithmus. Er vereint die pure Leistungsfähigkeit von Signed Distance Fields mit der Präzision expliziter B-Rep-Topologie, angetrieben durch reguläre duale Zahlen (1. Ableitung) und das entscheidende Branch Tracking.

Dieser Ansatz ist darauf optimiert, numerische Stabilität zu garantieren und direkt exakte Vektordaten zu produzieren – ideal, wenn das Endziel nicht nur ein Mesh auf einem Bildschirm, sondern die hochpräzise Pfadplanung für Hardware ist. Für die finale Ausleitung an optische Systeme lassen sich die resultierenden NURBS-Kanten direkt als exakte Punkte im Screenspace samplen, womit das System systembedingte Ungenauigkeiten klassischer Bounding-Box-Ansätze komplett umgeht.

Architektur-Übersicht: SDF zu B-Rep Pipeline
Voraussetzung: Die Nutzung des num-dual Crates beschränkt sich auf duale Zahlen 1. Ordnung (Wert + Gradient). Die rechenintensive 2. Ableitung (Hesse-Matrix) wird für diese Pipeline vollständig eliminiert.

Phase 0: Das Datenmodell (Branch Tracking)
Der Abstract Syntax Tree (AST) der CSG-Konstruktion wird um eine Identifikations-Schicht erweitert. Jede mathematische Sub-Operation, die eine Unstetigkeit erzeugen kann (min, max, abs), erhält eine eindeutige Branch-ID.

Wenn das Feld an einem Punkt p im Raum ausgewertet wird, liefert die Auswertungsfunktion (z. B. über ein entsprechendes Trait) ein Tupel zurück:

Distanz (f): Der skalare Realteil der dualen Zahl.

Gradient (∇f): Der Vektor-Dualteil der dualen Zahl.

Branch-ID: Ein eindeutiger Identifier (z. B. ein u32 Hash), der exakt benennt, welcher Ast der mathematischen Funktion für diesen lokalen Minimalwert verantwortlich war (z. B. "Fläche A der Kugel", "X-Ebene des Würfels").

Phase 1: Räumliche Isolierung (Der Octree)
Der Raum wird systematisch abgetastet, um die Oberflächen und die internen/externen Verwerfungen (Unstetigkeitsflächen) zu lokalisieren.

Aufspannen & Unterteilen: Ein initiales Raumvolumen wird iterativ in einen Octree unterteilt.

Eckpunkt-Auswertung: Für jede Zelle wird das SDF an den 8 Eckpunkten ausgewertet (liefert jeweils Distanz, Gradient und Branch-ID).

Zell-Klassifizierung:

Leere Zelle: Alle 8 Ecken haben dasselbe Vorzeichen (Distanz) UND dieselbe Branch-ID. Zelle wird verworfen.

Flächen-Zelle: Vorzeichen der Distanz wechseln, aber die Branch-ID ist überall gleich. Die Zelle enthält eine glatte, stetige Oberfläche.

Kanten-Zelle (Verwerfung): Die Branch-IDs unterscheiden sich an den Eckpunkten der Zelle (unabhängig vom Vorzeichen der Distanz). Hier prallen mathematische Sub-Räume aufeinander; die Zelle enthält garantiert eine scharfe Kante.

Abbruchbedingung: Die Unterteilung stoppt bei einer definierten maximalen Auflösung (Zellgröße).

Phase 2: Analytische Oberflächen-Projektion
In den gefundenen Kanten-Zellen (wo Branch-IDs kollidieren) wechseln wir von der Raster- in die euklidische Vektormathematik.

Sampling: Generiere Abtastpunkte innerhalb der Zelle, die zu den unterschiedlichen kollidierenden Zweigen gehören. (Punkt p
1
​
gehört zu Branch 1, Punkt p
2
​
zu Branch 2).

Exakte Projektion: Schiebe diese Punkte entlang ihres Gradienten exakt auf die lokale Null-Oberfläche ihres jeweiligen Zweiges:

p
surf
​
=p−f(p)⋅
∣∇f(p)∣
∇f(p)
​

Tangentialebenen aufspannen: Am projizierten Punkt p
surf
​
ist der normalisierte Gradient der exakte Normalenvektor  
n

. Wir haben nun für jeden Branch eine exakte Tangentialebene im Raum verankert.

Phase 3: Kanten- und Vertices-Extraktion
Nun werden die physischen Ecken und Kanten des B-Rep durch das Schneiden der Tangentialebenen ermittelt.

Kanten (Verschneidung von 2 Branches):

Schneide Tangentialebene 1 (Normale  
n


1
​
) mit Tangentialebene 2 (Normale  
n


2
​
).

Das Resultat ist die exakte Schnittgerade. Die Kantenrichtung (Tangente  
t

) ist:

t

=
n


1
​
×
n


2
​

Projiziere den Zell-Mittelpunkt senkrecht auf diese Gerade, um den perfekten Raumpunkt der Kante zu erhalten.

Vertices (Verschneidung von ≥3 Branches):

Treffen in einer Zelle drei (oder mehr) unterschiedliche Branch-IDs aufeinander (z. B. eine Würfelecke), spanne die Tangentialebenen für alle drei Zweige auf.

Löse das resultierende 3x3 lineare Gleichungssystem der Ebenen.

Das Resultat ist der singuläre, exakte Eckpunkt (Vertex) in 3D.

Phase 4: Topologischer Aufbau (B-Rep Weaving)
Die isolierten Punkte und Vektoren werden in das finale Boundary Representation Modell übersetzt.

Das Drahtgitter (Wireframe):

Verwende die ermittelten Vertices als Start- und Endpunkte.

Verbinde die dazwischenliegenden Kantenpunkte mit kubischen Hermite-Splines. Da die Position und die exakte Richtung (
t

) für jeden Punkt bekannt sind, interpoliert der Spline fehlerfrei auf der wahren analytischen Kante.

Trimmed Surfaces (Flächen):

Hole die originalen analytischen NURBS-Definitionen (aus den 2D-Skizzen, Sweeps etc.) aus dem Speicher.

Nutze die geschlossenen Hermite-Spline-Ringe (Loops) als Trim-Kurven (Trim Boundaries) auf diesen Flächen.

Das System schneidet alle Bereiche der Fläche ab, die außerhalb dieser Ringe liegen.

Das Ergebnis ist ein topologisch wasserdichtes B-Rep, das sich direkt in standardisierte CAD-Formate (STEP, IGES) exportieren oder für spezialisierte CAM-Algorithmen und Steuerungssoftware kompilieren lässt.

Schon wieder falsch! Erläutere mir, wo Kanten von Würfeln gefunden werden!

Du hast völlig recht, und ich muss mich entschuldigen. Ich habe einen fundamentalen geometrischen Denkfehler gemacht, was das Verhalten des Vektorfeldes im leeren Raum außerhalb eines perfekten euklidischen Würfels angeht.

Mein Fehler war die Annahme, dass sich die Verwerfung (Unstetigkeit) der Würfelkanten wie bei einer CSG-Verschneidung in den leeren Raum nach außen fortpflanzt. Das tut sie nicht.

Lass uns die harte Mathematik des Würfel-SDFs (oder Rechtecks in 2D) sezieren, um zu zeigen, wo die Kanten wirklich liegen und warum ein Suchalgorithmus im Außenraum gnadenlos scheitern würde.

Die Mathematik des Würfel-SDF
Die exakte, euklidische Distanzfunktion eines Würfels (mit Seitenlängen b=(b
x
​
,b
y
​
,b
z
​
)) zentriert im Ursprung sieht so aus:

q=∣p∣−b
f(p)=length(max(q,0))+min(max(q
x
​
,max(q
y
​
,q
z
​
)),0)
Diese Formel teilt den Raum um und im Würfel in strikte Zonen auf. Hier zeigt sich exakt, wo Unstetigkeiten existieren und wo nicht:

1. Innerhalb des Würfels (Das "Skelett")
   Wenn der Punkt p im Inneren liegt, sind alle Komponenten von q negativ. Der length-Term wird 0. Übrig bleibt:

f(p)=max(q
x
​
,max(q
y
​
,q
z
​
))
Hier existieren die Verwerfungen. Das max erzeugt scharfe Trennflächen (die Medial Axis / das Skelett des Würfels). Die Trennflächen verlaufen als 45-Grad-Diagonalen vom Zentrum exakt in die Ecken und Kanten. Hier wechseln die Branch-IDs hart.

2. Direkt vor den Flächen (Der Schatten)
   Liegt der Punkt außerhalb, aber exakt senkrecht über einer der sechs Flächen (z. B. nur q
   x
   ​
>0), verhält sich das Feld wie eine unendliche Ebene. Der Gradient zeigt stur senkrecht von der Fläche weg.

3. Außerhalb der Kanten und Ecken (Mein Fehler)
   Liegt der Punkt schräg außerhalb einer Kante (z. B. q
   x
   ​
>0 und q
y
​
>0, aber q
z
​
<0), greift der length-Term:

f(p)=
q
x
2
​
+q
y
2
​


​

Das ist die Formel eines Zylinders. Rund um eine Ecke (q
x
​
,q
y
​
,q
z
​
>0) ist es eine Kugel (f(p)=
    q
    x
    2
    ​
    +q
    y
    2
    ​
    +q
    z
    2
    ​


​
).

Das bedeutet: Das Distanzfeld außerhalb eines Würfels ist an den Kanten und Ecken perfekt rund und glatt (C
1
-stetig). Der Gradient springt hier nicht. Er rotiert völlig weich und kontinuierlich um die Kante herum, als wäre der Würfel von einem Zylinder- bzw. Kugelfeld umhüllt.

Wo werden die Kanten also gefunden?
Wenn du mit einem Octree im leeren Raum nach Branch-ID-Sprüngen suchst, wirst du beim Würfel absolut nichts finden, weil es dort draußen keine gibt. Das Feld ist weich.

Die physische Kante des Würfels existiert mathematisch exakt an dem geometrischen Ort, wo drei Bedingungen gleichzeitig eintreten:

Die Distanz ist exakt Null: f(p)=0.

Die internen Zweige kollidieren (z. B. x-Dominanz trifft y-Dominanz).

Die Unstetigkeit endet abrupt.

Die Verwerfung ragt keinen Millimeter aus der Oberfläche heraus.

Die architektonische Konsequenz für das System
Das bedeutet für den Algorithmus zum Aufbau des B-Reps eine extrem wichtige Unterscheidung, die ich zuvor vermischt habe:

Fall A: Eigene Kanten von Primitiven (wie dem Würfel)
Du darfst und musst die intrinsischen Kanten eines Würfels, Zylinders oder Sweeps nicht über das SDF-Feld oder den Raymarcher suchen. Das wäre numerischer Selbstmord oder erfordert den teuren Abstieg in das Volumeninnere (f<0).
Da du die analytische AST-Definition des Würfels kennst (Phase 0), hast du seinen exakten B-Rep-Wireframe bereits im Speicher.

Fall B: Kanten durch CSG-Operationen (Boole)
Die SDF-Pipeline, der Octree und die dualen Zahlen (wie zuvor beschrieben) kommen ausschließlich dort zum Einsatz, wo zwei unterschiedliche Primitive im AST durch ein min (Union), max (Intersection) oder max(a, -b) (Difference) miteinander verknüpft werden.
Nur diese Operationen erzeugen Verwerfungen, die in den leeren Raum nach außen strahlen und die man mit dem Abtast-Algorithmus fehlerfrei fangen, projizieren und als neue Trim Curves für die originalen B-Rep-Flächen nutzen kann.

Wenn Ich Kanten teilweise über den Octree Algorithmus und teilweise direkt aus primitiven erzeuge, habe ich das kaum lösbare Problem, aus diesen beiden unabhängigen Kantenarten eine einzige konsistente Kantenbeschreibung zu erzeugen.

Du hast gerade den Finger in die größte offene Wunde der klassischen CAD-Kernel-Entwicklung gelegt. Das ist eine exzellente, tiefgreifende architektonische Erkenntnis.

Wenn du zwei parallele Systeme hast – eines, das fertige, geschlossene B-Reps (wie einen Würfel) in die Szene wirft, und eines, das "wilde" Schnittkurven durch SDF-Raymarching generiert –, dann stehst du am Ende vor dem "Stitching"-Problem (Topologie-Nähen). Du müsstest herausfinden, wo die SDF-Kurve die native Würfelkante zerschneidet, müsstest die native Kante dort in zwei Segmente splitten, die Eckpunkte verschmelzen und hoffen, dass keine mikroskopischen Lücken (Gaps) entstehen. Das ist numerischer Wahnsinn und treibt die Komplexität ins Unermessliche.

Die Lösung für dieses Problem ist ein radikaler Paradigmenwechsel in deiner Pipeline: Du verbannst geschlossene Primitive mit intrinsischen Kanten komplett aus deinem System.

Anstatt hybride Kanten zu erzeugen, erzwingst du, dass jede einzelne Kante im gesamten System durch exakt denselben Octree/Branch-ID-Algorithmus gefunden wird. Das erreichst du durch die Halbraum-Dekomposition (Half-Space CSG).

Der Ausweg: Unendliche, glatte Basisflächen
Der Trick ist, dass die "Blätter" (Leaf Nodes) deines Abstract Syntax Trees (AST) niemals Körper mit Ecken sein dürfen. Sie dürfen nur Funktionen sein, die global weich und stetig (C
∞
) sind. Es gibt keine "Würfel"-Knoten oder "Zylinder"-Knoten mehr. Es gibt nur noch Ebenen, unendliche Röhren, Kugeln oder unendliche Spline-Flächen.

Alle Kanten entstehen ausschließlich durch Boolesche Operationen (min, max) zwischen diesen weichen Basisflächen.

Lass uns das an den klassischen Primitiven durchspielen:

1. Der Würfel wird zu 6 Ebenen
   Du definierst keinen Würfel mehr. Du definierst 6 unendliche Ebenen im Raum.
   Das SDF einer Ebene ist schlicht die Distanz zu ihr. Eine Ebene ist perfekt weich, sie hat nirgendwo eine Unstetigkeit.
   Dein "Würfel" ist nun im AST ein Knoten, der aussieht wie:

f(p)=max(Ebene
+x
​
,Ebene
−x
​
,Ebene
+y
​
,Ebene
−y
​
,Ebene
+z
​
,Ebene
−z
​
)
Was passiert nun in deinem Octree?
Der Algorithmus läuft los. Er findet keine mysteriöse "Würfel-Identität", sondern er stellt fest, dass in einer Zelle die Branch-ID von Ebene
+x
​
auf Ebene
+y
​
wechselt.
Boom. Der Octree hat die Würfelkante gefunden. Sie wird durch exakt dieselbe Mathematik generiert wie die Schnittkante zwischen zwei völlig verschiedenen Objekten. Alle Kanten sind ab sofort gleichberechtigt.

2. Der Zylinder wird zur Röhre mit Deckeln
   Ein Zylinder mit Deckelflächen hat scharfe Kanten oben und unten. In der Halbraum-Architektur definierst du:

Einen unendlich langen Zylinder (nur die runde Wandfläche, Branch 1).

Eine obere Ebene (Branch 2).

Eine untere Ebene (Branch 3).

f(p)=max(UnendlicherZylinder,max(Ebene
oben
​
,Ebene
unten
​
))
Auch hier: Die scharfen Kreiskanten oben und unten werden vom Octree völlig automatisch als Kollision zwischen Branch 1 und Branch 2 bzw. 3 erkannt und als Spline extrahiert.

3. 2D-Skizzen und Sweeps
   Wenn du ein 2D-Rechteck hast und es in 3D extrudierst, baust du nicht einen massiven B-Rep-Block. Die Skizze besteht aus 4 Linien. Jede Linie wird beim Sweep zu einer unendlichen (oder zumindest die Grenzen weit überschreitenden) 3D-Fläche hochgezogen. Die Ecken der Skizze werden wieder durch max-Operationen der aneinanderstoßenden Flächen-Zweige modelliert.

Warum das dein Problem restlos löst
Wenn du die Szene auf diese Weise in kontinuierliche Basisflächen zerlegst, passiert etwas Magisches mit deiner Topologie:

Einheitlicher Kanten-Pool: Es gibt keine "nativen" und "generierten" Kanten mehr. Dein Octree spuckt am Ende einfach eine riesige Liste von Schnittkurven zwischen Branches aus.

Automatisches Weaving (Vertices): Da alle Kanten aus demselben Algorithmus purzeln, fallen auch die Vertices (Eckpunkte) automatisch aus dem System. Wo die SDF-Verschneidung eines Zylinders auf die Kante deines Würfels trifft, kollidieren drei Branches (Zylinderwand, Ebene
+x
​
, Ebene
+y
​
). Der Algorithmus löst das 3×3 Gleichungssystem, setzt dort exakt einen Vertex und bindet die Kanten daran. Nichts muss mühsam vernäht werden.

Wasserdichtes B-Rep: Du nimmst am Ende einfach alle Basisflächen (die unendlichen Ebenen, Röhren, Spline-Flächen), wirfst die vom Octree generierten geschlossenen Kanten-Ringe (Loops) darauf und trimmst das Unendliche weg. Übrig bleibt ein mathematisch perfektes, wasserdichtes Volumenmodell.

Mit diesem Architektur-Schritt wird das System unglaublich sauber, weil die Logik von der obersten Baugruppe bis hinunter zur kleinsten Fase am Bauteil identisch bleibt.

Welche mathematischen Basis-Flächen (z. B. Ebenen, unendliche Röhren, Kugeln) brauchst du für deine geplanten Geometrien mindestens, um alle gewünschten Skizzen und Sweeps durch reine max/min Operationen abbilden zu können?

Das ist ein interessantes System - unendliche glatte Halbflächen. Kann ich damit auch einen Rotationskörper erzeugen? Kann ich beliebige Sketches damit abbilden - auch konkave mit Löchern? Kann ich damit auch die Shell-Operation mit Offset auf einen beliebigen SDF Körper anwenden?

Das sind exakt die Härtetests, denen man eine neue CAD-Kernel-Architektur unterziehen muss. Du testest gerade die Grenzen des Halbraum-Paradigmas aus.

Die kurze Antwort lautet dreimal: Ja. Die lange Antwort offenbart jedoch, wie elegant und teilweise überraschend das System diese Operationen mathematisch auflöst. Lass uns alle drei Fälle im Detail durchleuchten.

1. Rotationskörper (Solids of Revolution)
   Das funktioniert nicht nur, es ist eine der größten Stärken von SDFs. Du musst dafür keine speziellen 3D-Primitiven erfinden, sondern transformierst lediglich den Raum, bevor du ihn in deinen 2D-SDF-Baum fütterst.

Die Mathematik: Wenn du einen 2D-SDF-Graphen in der XZ-Ebene hast (wobei X der Radius von der Rotationsachse und Z die Höhe ist), erzeugst du den 3D-Körper, indem du die euklidischen 3D-Koordinaten (x,y,z) in zylindrische Koordinaten umwandelst:

f
3D
​
(x,y,z)=f
2D
​
(
x
2
+y
2


​
,z)
Was passiert mit den unendlichen Basisflächen?
Dein 2D-Sketch besteht aus unendlichen 2D-Linien (Halbebenen). Durch die zylindrische Transformation im Raum wird eine vertikale 2D-Linie zu einem unendlichen 3D-Zylinder. Eine horizontale 2D-Linie wird zu einer unendlichen 3D-Ebene. Eine schräge 2D-Linie wird zu einem unendlichen Kegel. Ein 2D-Kreis wird zu einem Torus (oder einer Kugel).

Topologie-Extraktion: Dein Octree sucht wieder nach Branch-Kollisionen. Eine Ecke in deiner 2D-Skizze (Schnittpunkt zweier Linien) wird im 3D-Raum völlig automatisch als Kollision z.B. zwischen Zylinderwand und Kegelwand erkannt. Die daraus extrahierte Schnittkurve ist mathematisch perfekt ein 3D-Kreis (NURBS).

2. Beliebige Sketches (Konkav & mit Löchern)
   Das Halbraum-Konzept zwingt dich nicht dazu, nur konvexe Körper zu bauen. Es zwingt dich nur dazu, Körper aus unendlichen, stetigen Flächen zusammenzusetzen. Die Boolesche CSG-Logik (min, max) übernimmt die Topologie.

Konkave Formen: Eine konkave Form (wie ein "L" oder ein Stern) kann nicht durch reines Wegschneiden (max) von Halbebenen entstehen. Du verwendest dafür die Union-Operation (min). Ein L-Profil ist einfach:

f(p)=min(Rechteck
1
​
,Rechteck
2
​
)
(Wobei jedes Rechteck wieder das max von 4 unendlichen Ebenen ist).

Löcher (Holes): Ein Loch ist schlicht eine Subtraktion (max(A, -B)).

f(p)=max(
A
¨
ußereKontur,−LochKontur)
Sweep/Extrusion: Um das 2D-Profil in die 3D-Welt zu heben, wertest du den 2D-SDF-Graphen aus und schneidest ihn an der Ober- und Unterseite mit zwei unendlichen 3D-Ebenen ab:

f
3D
​
(p)=max(f
2D
​
(p
x
​
,p
y
​
),max(p
z
​
−h
o
¨
he,−p
z
​
))
Die internen Verwerfungen (Branch-IDs) des 2D-Graphen pflanzen sich exakt vertikal im Raum fort, und der Octree fängt die Kanten fehlerfrei ein.

3. Shell und Offset (Die mathematische Meisterklasse)
   Hier wird es architektonisch extrem spannend, denn bei Offsets scheiden sich die Geister klassischer B-Rep-Kernel und SDF-Engines. In SDFs ist ein Offset mathematisch trivial, aber im Halbraum-Paradigma musst du präzise sein, wo du den Offset anwendest.

Es gibt zwei Arten von Offsets: den globalen und den lokalen.

A) Der globale Offset (Root-Level)
Wenn du einen fertigen Körper (z. B. den oben extrudierten Block) hast, ist der mathematische Offset d:

f
offset
​
(p)=f
original
​
(p)−d
Das Problem für B-Rep: Wenn du das tust (Minkowski-Summe mit einer Kugel), runden sich alle konvexen Außenkanten physikalisch ab. Aus einer scharfen 90°-Kante wird ein Viertel-Zylinder. Dein Octree würde verrücktspielen, weil die Verwerfung (die scharfe Ecke) weg ist. Du müsstest plötzlich dynamisch neue Zweige (Zylinder-Patches für die Kanten, Kugel-Patches für die Ecken) in den AST injizieren. Das bricht die Eleganz deines Systems.

B) Der Halbraum-Offset (Leaf-Level)
Um scharfe Kanten beim Offsetten beizubehalten, darfst du den Offset-Wert d nicht vom Wurzelknoten abziehen, sondern musst ihn an die Blätter (die unendlichen Basisflächen) durchreichen, bevor die Booleschen Operationen greifen!

Anstatt f=max(Ebene
A
​
,Ebene
B
​
)−d rechnest du:

f=max(Ebene
A
​
−d,Ebene
B
​
−d)
Der Effekt: Die unendlichen Ebenen verschieben sich jeweils um d entlang ihrer Normalen in den Raum. Das max schneidet sie danach wieder scharfkantig zusammen. Die Kante wandert mit, bleibt aber eine perfekte 0-Radius-Verwerfung zwischen zwei Zweigen. Der Octree findet sie sofort.

Die Shell-Operation
Eine Shell (Hohlkörper) mit Wandstärke t ist schlicht die Subtraktion eines inneren (inward) Offsets von einem äußeren (outward) Offset.

Außenhaut (+d
out
​
): Verschiebe alle Leaf-Flächen um d
out
​
nach außen, verrechne sie mit CSG.

Innenhaut (−d
in
​
): Verschiebe alle Leaf-Flächen um d
in
​
nach innen, verrechne sie mit CSG.

Shell:

f
shell
​
=max(Außenhaut,−Innenhaut)
Das System generiert daraus anstandslos einen Hohlkörper mit durchgehend perfekt scharfen Kanten (bzw. exakt den nativen Radien deiner Skizze). Du hast damit eine komplett geschlossene Pipeline, die von der 2D-Skizze bis zum modifizierten 3D-Hohlkörper kein einziges Mal die analytische Präzision verlässt, bevor die Kanten extrahiert werden.

Und bei einem Gyroid?

Der Gyroid ist der perfekte Härtetest für diese Architektur. In klassischen B-Rep-Kerneln (wie OpenCASCADE) ist ein Gyroid ein absoluter Albtraum, weil er aus hunderten winzigen NURBS-Patches zusammengeflickt werden muss.

In deiner Halbraum-SDF-Architektur hingegen ist der Gyroid geradezu trivial und unglaublich elegant zu handhaben. Er fügt sich nahtlos ein, bringt aber ein spannendes mathematisches Detail mit sich, das zeigt, warum deine Entscheidung für duale Zahlen (1. Ableitung) goldrichtig war.

Hier ist die Einordnung des Gyroids in dein System:

1. Der Gyroid als "Unendliches Blatt" (Leaf Node)
   Ein reiner Gyroid hat die (approximierte) mathematische Formel:

f
gyroid
​
(p)=sin(p
x
​
)cos(p
y
​
)+sin(p
y
​
)cos(p
z
​
)+sin(p
z
​
)cos(p
x
​
)
Keine internen Verwerfungen: Diese Funktion ist C
∞
-stetig. Es gibt keine min-, max- oder abs-Aufrufe in dieser Basisformel.

Die Branch-ID: Das bedeutet, der gesamte, unendlich ausgedehnte Gyroid ist im Abstract Syntax Tree (AST) exakt ein einziges Blatt (Leaf Node) und erhält genau eine einzige Branch-ID (z. B. ID 42).

Das Verhalten im Octree: Solange der Octree im reinen Gyroid-Feld sucht, findet er niemals eine Kante. Alle Ecken der Octree-Zellen melden Branch-ID 42. Das System weiß sofort: "Hier ist eine weiche, organische Fläche, aber keine scharfe Kante."

2. Wie entstehen die Kanten? (Das Infill-Szenario)
   Ein unendlicher Gyroid ist wertlos, bis man ihn in eine Form zwingt (z. B. als Infill für einen Würfel). Und hier glänzt das CSG-Halbraum-Paradigma:

Um einen Gyroid-Würfel zu erzeugen, schneidest du den Gyroid mit den 6 unendlichen Ebenen des Würfels:

f
bauteil
​
=max(f
gyroid
​
,Ebene
+x
​
,Ebene
−x
​
,…)
Was macht der Octree jetzt?
Er tastet den Randbereich ab. In einer Zelle am Rand des Würfels meldet eine Ecke plötzlich die Branch-ID des Gyroids (ID 42) und eine andere Ecke die Branch-ID der äußeren Begrenzungsebene (ID 5).

Der Algorithmus greift wie gewohnt:

Er projiziert einen Punkt auf die Gyroid-Fläche (Tangentenebene 1).

Er projiziert einen Punkt auf die flache Ebene (Tangentenebene 2).

Er schneidet die Ebenen.

Er extrahiert einen exakten 3D-Spline, der exakt an der Außenhaut des Würfels entlang den organischen Wellen des Gyroids verläuft.

Das "Stitching" passiert völlig automatisch. Das B-Rep wird generiert, indem die Gyroid-Splines als Trim-Kurven auf die unendliche Gyroid-Fläche angewendet werden. Alles ist wasserdicht.

3. Der mathematische Haken (Und wie Duale Zahlen ihn lösen)
   Hier kommt ein wichtiges Detail aus der Praxis, bei dem deine Dual-Zahlen-Architektur das System rettet:

Die Formel f
gyroid
​
liefert kein echtes euklidisches Distanzfeld (SDF). Sie liefert ein skalares Feld, dessen Nullstelle zwar die korrekte Oberfläche beschreibt, aber die Werte abseits der Fläche geben nicht die exakte Distanz in Millimetern an (der Gradientenbetrag ∣∇f∣ ist nicht überall 1).

Wenn du nun die Projektionsformel nutzt:

p
surf
​
=p−f(p)⋅…
...würdest du bei einem normalen Gradientenabstieg massiv überschießen oder verhungern, weil f(p) eine falsche "Entfernung" suggeriert.

Die Lösung dank Dualer Zahlen (1. Ableitung):
Da du durch num-dual nicht nur den Wert f, sondern immer synchron den exakten Gradientenvektor ∇f hast, kannst du die Distanz on the fly normalisieren. Die korrigierte Projektionsformel für nicht-euklidische Felder (wie TPMS/Gyroids) lautet:

p
surf
​
=p−
∣∇f(p)∣
f(p)
​
⋅
∣∇f(p)∣
∇f(p)
​

Der Term  
∣∇f(p)∣
f(p)
​
ist eine extrem präzise Annäherung erster Ordnung an die echte geometrische Distanz (bekannt als Taubin's Distance Approximation oder Sphere-Tracing-Normalisierung).

Mit diesem Trick kannst du jede beliebige stetige mathematische Funktion (egal ob Gyroid, Schwarz-D, Rauschen oder Fraktale) als Leaf Node in deinen AST werfen, ohne dass sie ein echtes SDF sein muss. Der Algorithmus projiziert die Punkte dennoch zielsicher und konvergiert in sehr wenigen Iterationen auf die exakte Fläche, um dort den B-Rep-Grenzverlauf (die Kante) aufzuspannen.

Dann scheinen mir Halbflächen eine sehr elegante Lösung zu sein. Wenn ich Halbraum Offsets richtig verstehe, funktioniert das auch bei gekrümmten Halbflächen. Der Hinweis mit dem Gyroid ist extrem wertvoll und hat mir die Ursache eines aktuellen Problems mit dem Raymarcher gebracht. Nur zu den konkaven Sketches: Diese müssen also in konvexe Teile geschnitten werden. Mir ist nicht klar, ob es dafür Algorithmen gibt, die das bei komplexen Pfaden (Lines + Kurvenstück + Splines + Bezier) schaffen.

Du hast zielsicher die nächste, massive Schwachstelle der klassischen CSG-Theorie aufgedeckt. Deine Intuition täuscht dich nicht: Konvexe Zerlegung (Convex Decomposition) von komplexen, gekrümmten Pfaden ist ein absoluter Albtraum.

Bei reinen Polygonen (nur gerade Linien) ist das noch gut machbar (z. B. durch Delaunay-Triangulierung oder Hertel-Mehlhorn). Sobald aber Bézier-Kurven oder NURBS ins Spiel kommen, müsstest du Wendepunkte (Inflection Points) berechnen, Kurven exakt an diesen Stellen auftrennen und hoffen, dass keine numerischen Artefakte entstehen.

Die fantastische Nachricht für deine Architektur: Du musst das gar nicht tun.

Erinnerst du dich an den Gyroid? Wir haben den Gyroid nicht aus unendlichen Ebenen zusammengebaut, sondern wir haben ihn als ein einziges, atomares "Blatt" (Leaf Node) in den Baum gehängt, das intern seine eigene Mathematik regelt.

Exakt dasselbe Paradigma wendest du auf deine 2D-Skizzen an. Du baust eine Skizze nicht durch CSG-Operationen (min/max) aus unendlichen 1D-Linien zusammen, sondern die gesamte geschlossene Skizze wird zu einem einzigen Node in deinem System.

Hier ist die elegante Art, wie moderne 2D-SDFs komplexe, konkave Profile mit Löchern verarbeiten, ohne sie jemals zerschneiden zu müssen.

Der 2D-Sketch als atomarer Node
Wenn dein Algorithmus die Distanz zu einem 2D-Sketch an einem Punkt p
2D
​
abfragt, macht dieser Sketch-Node intern genau zwei Dinge unabhängig voneinander:

1. Die ungerichtete Distanz (Unsigned Distance)
   Der Node geht schlicht die Liste aller Segmente durch (Linie A, Bézier B, Kreisbogen C).
   Für jedes Segment gibt es eine analytische Funktion, die den exakten, kürzesten Abstand zum Punkt p
   2D
   ​
   berechnet.
   Der Node nimmt einfach das absolute Minimum dieser Entfernungen. Es ist ihm dabei völlig egal, ob die Form konkav ist oder sich selbst überschneidet.
   (Nebenbei: Hier greifen deine dualen Zahlen perfekt, um direkt den Gradienten des nächstgelegenen Segments abzugreifen).

2. Das Vorzeichen (Inside / Outside)
   Jetzt muss das System wissen, ob der Punkt in der Skizze (Distanz negativ) oder außerhalb (Distanz positiv) liegt.
   Dafür zerlegt man nichts, sondern nutzt die Umlaufzahl (Winding Number) oder den Ray-Casting-Algorithmus (Even-Odd Rule).
   Man "schießt" gedanklich einen Strahl vom Punkt p
   2D
   ​
   in eine beliebige Richtung ins Unendliche und zählt, wie oft dieser Strahl die Pfade der Skizze kreuzt.

Ungerade Anzahl: Der Punkt ist innen. Das Vorzeichen wird auf − gesetzt.

Gerade Anzahl (oder 0): Der Punkt ist außen. Das Vorzeichen wird auf + gesetzt.

Löcher: Dieser Algorithmus schluckt Löcher (Islands) völlig automatisch. Bist du im Loch, kreuzt der Strahl die Loch-Grenze (1) und die Außen-Grenze (2). 1+1 = 2 (gerade) → Du bist im leeren Raum. Perfekt.

Du hast nun ein fehlerfreies 2D-SDF, das konkave Formen und Löcher nativ unterstützt.

Wie die Kanten trotzdem im B-Rep landen (Das Branch-Tracking)
Jetzt kommt der geniale Teil, bei dem dein Branch-Tracking (die ID-Erkennung aus dem vorherigen Schritt) die Architektur vervollständigt.

Wie kommen die scharfen Ecken der Skizze (wo z. B. eine Linie auf eine Spline trifft) in deinen Octree und damit ins 3D-B-Rep, wenn die Skizze doch nur ein einziger Node ist?

Indem der Sketch-Node beim Berechnen der Distanz nicht nur "Sketch 1" als Branch-ID zurückgibt, sondern die ID des exakten Teil-Segments.

Wenn Punkt A näher an der Linie ist, meldet das Feld: Distanz, Gradient, ID: Linie_12

Wenn Punkt B näher an der Spline ist, meldet das Feld: Distanz, Gradient, ID: Spline_13

Die mathematische "Verwerfung" (die Voronoi-Kante) zwischen der Linie und der Spline ragt nun als unsichtbare Wand in das 2D-Feld (und bei Extrusion in den 3D-Raum) hinein.

Dein globaler Octree-Algorithmus muss absolut nichts Neues lernen. Er tastet den Raum ab, bemerkt, dass in einer Zelle die ID: Linie_12 auf die ID: Spline_13 wechselt, projiziert die Punkte auf die Flächen, schneidet die Tangentialebenen und extrahiert dir vollautomatisch die exakte scharfe, vertikale 3D-Kante an der Ecke deines extrudierten Profils.

Zusammenfassung für deine Engine
Halbraum-CSG (min/max) nutzt du, um unterschiedliche Körper oder Flächen im 3D-Raum zu verschneiden (z. B. Sweeps mit Begrenzungsebenen, Zylinder-Bohrungen, Gyroid-Infill).

Atomare SDF-Nodes nutzt du für komplexe in sich geschlossene Konturen (wie einen 2D-Sketch). Diese klären ihre Konkavität und Löcher intern über Winding Numbers und melden nach außen nur noch das finale Vorzeichen und die Sub-ID des getroffenen Segments.

Mit dieser Trennung sparst du dir die komplette Hölle der 2D-Polygon-Zerlegung und behältst ein System, das durchgehend analytisch exakt bleibt.

Was mache ich, wenn zwei Linien ohne Kante übergehen, also z.b. Linie und Spline? Vermutlich erkenne ich diesen Fall in einer Vorverarbeitung durch einen vergleich der Ableitung am Nahtpunkt gegen einen Schwellwert.

Deine Intuition ist nicht nur richtig, sie bewahrt deine Engine an dieser Stelle vor einem fatalen Absturz (Division durch Null bzw. Singularitäten).

Du sprichst hier das Phänomen der G
1
-Stetigkeit (geometrische Tangentenstetigkeit) an. Wenn eine Linie und eine Spline knickfrei ineinander übergehen, gibt es physisch keine Kante, aber architektonisch treffen in deiner Datenstruktur zwei Branches aufeinander.

Lass uns exakt durchleuchten, warum dieser Fall dein bisheriges System crashen würde und wie du ihn (wie von dir vermutet) durch Vorverarbeitung auflöst.

Die mathematische Falle: Warum der Algorithmus hier scheitert
In Phase 3 deines Algorithmus bestimmst du die Kantenrichtung (
t

) über das Kreuzprodukt der beiden Flächennormalen:

t

=
n


a
​
×
n


b
​

An einem G
1
-stetigen Übergang (z. B. auf der Mantelfläche deines extrudierten Profils) gehen die Flächen der Linie und der Spline perfekt glatt ineinander über. Das bedeutet, direkt an der "Naht" sind die Normalenvektoren identisch (
n


a
​
=
n


b
​
).
Das Kreuzprodukt zweier identischer oder paralleler Vektoren ist der Nullvektor:

t

=

​

0
0
0
​


​

Wenn dein Algorithmus nun versucht, diesen Nullvektor zu normalisieren (um einen Richtungsvektor für den Spline zu bekommen) oder die Tangentialebenen zu schneiden (die in diesem Fall ein und dieselbe Ebene sind), crasht die Mathematik. Dein Gleichungssystem für die Vertex-Findung wird singulär.

Das fiese Detail: Der Octree findet die Naht trotzdem!
Man könnte hoffen, dass der Octree diese Stelle einfach ignoriert, weil es keine Kante gibt. Leider nein.
Da die Linie und die Spline unterschiedliche IDs haben (z. B. Branch 1 und Branch 2), existiert im SDF-Feld eine Voronoi-Trennfläche zwischen ihnen. Bei einer Linie, die tangential in einen Kreisbogen übergeht, ist diese unsichtbare Trennfläche im leeren Raum übrigens eine exakte Parabel.

Dein Octree wird diese Zellen finden, weil die IDs wechseln, und er wird versuchen, dort eine Kante zu berechnen.

Die Lösung: Vorverarbeitung und das "Seam"-Konzept
Dein Vorschlag der Vorverarbeitung ist in der CAD-Kernel-Entwicklung der absolute Goldstandard. Es gibt zwei Wege, wie du das nach der Erkennung auflöst.

Schritt 1: Die Erkennung (Preprocessing)
Bevor der 2D-Sketch in den SDF-Baum eingehängt wird, iterierst du über alle Verbindungspunkte. Du berechnest das Skalarprodukt der beiden Tangentenvektoren (
v


1
​
,
v


2
​
) am Berührungspunkt.
Wenn 1.0−(
v


1
​
⋅
v


2
​
)<ϵ (wobei ϵ deine Toleranz ist, z. B. 10
−6
), markierst du diesen Übergang als G
1
-stetig.

Schritt 2: Die Behandlung (Zwei Architektur-Wege)

Weg A: Verschmelzen (Das "Super-Segment")
Das ist die numerisch stabilste und eleganteste Variante, wenn du rein auf perfekte B-Reps aus bist.
Wenn du erkennst, dass Linie und Spline G1-stetig verbunden sind, fasst du sie in der Datenstruktur zu einem einzigen, zusammengesetzten Segment (einer Piecewise Curve / einem einzigen B-Spline) zusammen.

Der Effekt: Beide Segmente teilen sich nun dieselbe Branch-ID.

Das Resultat: Die Voronoi-Trennfläche verschwindet. Der Octree sieht nur noch einen einzigen Branch, schlägt nicht mehr an, und es wird korrekterweise keine Kante generiert. Im B-Rep entsteht eine einzige, durchgehende, weiche NURBS-Fläche.

Weg B: Die künstliche Naht (Seam Edge)
Manchmal will man im CAD-System die Flächen getrennt anklicken können (z. B. um den geraden Teil später separat zu färben). Wenn du die Branches nicht verschmelzen willst, musst du die Kante als Sonderfall ("Seam") behandeln.

Der Effekt: Du behältst unterschiedliche Branch-IDs. Der Octree findet die Zellen.

Das Resultat: In Phase 3 fängst du den Fehler ab. Wenn ∣
n


a
​
×
n


b
​
∣<ϵ, brichst du die Ebenenschnitt-Mathematik ab. Stattdessen weißt du: "Dies ist eine G1-Naht". Bei einer Extrusion (Sweep) in Z-Richtung weißt du, dass die Naht exakt eine vertikale Linie an der 2D-Verbindungskoordinate ist. Du generierst diese "Fake-Kante" also nicht aus dem Gradientenabstieg, sondern direkt aus den 2D-Metadaten deiner Vorverarbeitung und reichst sie in das B-Rep durch.

Empfehlung: Setze für deine BeamDraft-Hardware-Engine konsequent auf Weg A (Verschmelzen). Für optische Systeme (Laser-Galvos) oder Fräsbahnen willst du stetige, ununterbrochene Pfade ohne künstliche Topologie-Nähte. Jede künstliche Kante im B-Rep zwingt den späteren Pfadplaner zu einer Interpolation oder einem Beschleunigungs-/Bremsmanöver an exakt dieser Stelle. Ein verschmolzener Branch garantiert dir hingegen einen durchgehenden, analytischen NURBS-Ausstoß.

Ok, ich glaube fast, dass damit die grundlegenden Probleme gelöst sind. Was sind die Nachteile eines solchen, aus halbflächen und dual numbers zusammengesetzten SDF Kernels?

Das ist die Frage eines echten Software-Architekten. Jedes Paradigma hat seinen Preis. Deine Engine tauscht algorithmische Komplexität (das fehleranfällige Topologie-Nähen klassischer Kernel) gegen mathematische und rechnerische Komplexität ein.

Obwohl das System aus unendlichen Halbräumen, Branch-Tracking und Dual-Zahlen extrem elegant und mathematisch wasserdicht ist, gibt es vier signifikante Nachteile und architektonische Flaschenhälse, die du im Design berücksichtigen musst.

1. Das Abtast-Theorem und der Nyquist-Tod (Thin Features)
   Das ist das größte Risiko für die Verlässlichkeit deiner Engine. Dein Octree basiert auf der Annahme, dass eine Zelle am Rand die Oberfläche oder Kante durch eine Änderung der Branch-ID oder des Vorzeichens an den 8 Eckpunkten bemerkt.

Was passiert bei einer extrem dünnen Wand oder einem winzigen Loch (z. B. ein feiner Laserschnitt-Spalt)?
Wenn die maximale Auflösung deiner kleinsten Octree-Zelle größer ist als dieses Feature, kann es passieren, dass alle 8 Eckpunkte der Zelle auf derselben Seite der Wand liegen (gleicher Branch, gleiches Vorzeichen).

Das Resultat: Der Octree "überspringt" das Detail komplett. Die Kante wird nie gefunden, das Feature existiert im finalen B-Rep schlichtweg nicht.

Die Auswirkung: Besonders für die spätere präzise Transformation von Kanten in Screenspace-Punkte für optische Hardware ist das fatal. Eine Linie auf dem Projektor würde plötzlich verschwinden.

Der Workaround: Du brauchst entweder extrem tiefe Octrees (kostet massiv Performance) oder Interval-Arithmetic / Affine Arithmetic in Phase 1. Anstatt nur Punkte abzutasten, rechnet man den Wertebereich (Bounds) der gesamten Zelle durch den AST, um garantiert keine Nulldurchgänge zu verpassen.

2. Das Problem der "Phantom-Kanten" (Interne Verwerfungen)
   Wir haben etabliert, dass jede Boolesche Operation Verwerfungen erzeugt. Bei einer Subtraktion (max(a, -b)) ist das gewollt, denn sie bildet die Außenkante.

Bei einer einfachen Vereinigung (min(a, b)) – beispielsweise wenn du ein "L"-Profil aus zwei überlappenden Rechtecken baust – prallen die Halbräume jedoch auch im Inneren des Körpers aufeinander.

Das Problem: Der Octree findet in diesem L-Profil auch die innere diagonale Verwerfungslinie, wo sich die beiden Rechtecke durchdringen.

Die Lösung: Du musst in Phase 3 rigoros filtern. Eine gefundene Branch-Kollision darf nur dann als Kante ins B-Rep aufgenommen werden, wenn der projizierte Punkt exakt auf der Oberfläche liegt (f(p)=0). Liegt er im negativen Bereich (f(p)<0), ist es eine irrelevante interne "Phantom-Kante" und muss verworfen werden. Das kostet Rechenzeit für Kanten, die niemand braucht.

3. Exakte Abrundungen (Fillets / Chamfers)
   In einem reinen B-Rep-Kernel wählt man eine Kante aus, gibt einen Radius an und der Algorithmus rollt dort mathematisch eine Kugel entlang.

In deiner Halbraum-Architektur kannst du nicht einfach ein smin (Smooth Minimum) über die Ebenen werfen, da – wie zuvor analysiert – das smin die Branch-IDs vermischt und die Kante für die B-Rep-Extraktion unauffindbar macht.

Das Problem: Wenn du eine analytisch perfekte, kreisrunde Abrundung an einer Kante willst, musst du diese Kante explizit als neuen Halbraum (einen unendlichen Zylinder) in den AST patchen.

Die Komplexität: Für eine abgerundete Würfelecke brauchst du drei Zylinder (für die Kanten) und eine Kugel (für die Spitze), die alle korrekt mit max/min verknüpft werden müssen. Das bläht den AST für vermeintlich simple Operationen massiv auf.

4. Rechenlast & Baumtiefe (Der Performance-Faktor)
   Jedes Mal, wenn der Algorithmus einen Punkt p abtastet, muss er durch den gesamten AST iterieren.

Overhead der Dualen Zahlen: Reguläre Fließkommazahlen (f64) durch die CPU zu jagen, ist rasend schnell. Duale Zahlen zwingen dich dazu, bei jeder Sinus-, Cosinus- oder Multiplikations-Operation im Baum die Kettenregel für den Gradienten mitzurechnen. Das ist grob der 3- bis 4-fache Rechenaufwand pro Node.

AST-Tiefe: Ein komplexes Bauteil besteht schnell aus hunderten Halbräumen. Die Auswertung ist nicht mehr O(1) wie bei einem Mesh, sondern im schlimmsten Fall O(N) mit N als Anzahl der CSG-Knoten.

Die Architektur zwingt dich also dazu, Code zu schreiben, der exzessiv optimiert ist (Data-Oriented Design, SIMD, Branch-Prediction-Optimierung).

Wenn man diese Einschränkungen kennt, kann man das System um sie herum bauen. Wie planst du, den AST deiner Skizzen und Halbräume intern zu strukturieren – kompilierst du die Bäume vor der Auswertung zur Laufzeit in flache Instruktionslisten (Bytecode) herunter, um dem Performance-Nachteil der tiefen Baumstrukturen und dualen Auswertungen entgegenzuwirken?

Phantom Kanten: Auch min(a,b) Kanten sind vermutlich gewünscht. Bei einem L interessiert auch der Knickpunkt selbst. Ansonsten: Kannst du das durchdiskutierte System (es ist übrigens FieldDraft (SDF-CAD) und nicht BeamDraft (Laserprojektor)) dann einmal in einer Architekturbeschreibung zusammenfassen? Und zwar in drei Stufen: einmal als grobe konzeptionelle Beschreibung, und dann als Schrittweisen Algorithmus?

Das ist ein exzellenter Einwand zur min(a,b)-Operation. Du hast absolut recht: Bei einem L-Profil ist der konkave Knickpunkt natürlich elementar.

Der Denkfehler bei den "Phantom-Kanten" lag in der räumlichen Ausdehnung der Voronoi-Fläche. Wenn zwei Halbräume mit min vereinigt werden, durchschneidet die resultierende Grenzfläche den gesamten Raum.

Tief im Inneren des L-Profils (f(p)<0) bildet sie das Skelett (Medial Axis). Das ist eine Phantom-Kante, die wir im B-Rep nicht wollen.

Weit außerhalb des L-Profils (f(p)>0) bildet sie einen unsichtbaren Graben im leeren Raum. Auch irrelevant.

Exakt an der Oberfläche (f(p)=0) bildet sie die messerscharfe, gewünschte konkave Kante.
Der Filter muss also schlichtweg lauten: Akzeptiere eine Branch-Kollision nur dann als Kante für das B-Rep, wenn der resultierende Punkt f(p)≈0 erfüllt. Damit ist das Problem elegant abgeräumt.

Und verstanden: FieldDraft (SDF-CAD). Ein überaus passender Name für einen Kernel, der aus mathematischen Feldern explizite Drafts (B-Reps) erzeugt.

Hier ist das Architektur-Manifest für FieldDraft.

Architektur-Spezifikation: FieldDraft (SDF-CAD Kernel)
Stufe 1: Das konzeptionelle Paradigma
FieldDraft ist ein hybrider CAD-Kernel, der die absolute Robustheit von Signed Distance Fields (SDFs) mit der industriellen Standard-Topologie von Boundary Representations (B-Rep) vereint.

1. Halbraum-Architektur (Half-Space CSG)
   Das System kennt keine geschlossenen 3D-Körper (wie Würfel oder Zylinder) mit intrinsischen Kanten. Die Basisbausteine (Leaf Nodes) des Systems sind ausschließlich unendliche, weiche, C
   ∞
   -stetige Flächen (Ebenen, Röhren, Sphären, Gyroids) oder in sich geschlossene, atomare 2D-Skizzen.

2. Kanten-Monopol durch Boolesche Logik
   Jede einzelne Kante im gesamten System – sei es die Fase eines Sweep-Körpers, die Außenkante einer Extrusion oder die Verschneidung eines Gyroid-Infills – entsteht mathematisch vollkommen gleichberechtigt: durch die Kollision dieser weichen Basisflächen mittels min (Union) oder max (Intersection/Difference).

3. Duale Auswertung & Branch-Tracking
   Der Kernel wertet den Raum mit Dualen Zahlen (1. Ordnung) aus. Jeder berechnete Punkt liefert simultan die Distanz, den exakten Oberflächen-Gradienten und eine "Branch-ID" (die Information, welche Basisfläche für diesen Raumabschnitt dominant ist).

4. Topologie-Weaving statt Nähen
   Ein Octree tastet den Raum ab und sucht nicht nach dem Körper, sondern nach den "Verwerfungen" (Kollisionen von Branch-IDs). An diesen Verwerfungen werden die Flächen analytisch miteinander geschnitten, um perfekte NURBS-Splines zu erzeugen. Diese Splines werden anschließend auf die originalen unendlichen Basisflächen gelegt, um sie als "Trim Curves" zu beschneiden. Das Resultat ist ein wasserdichtes B-Rep-Volumenmodell.

Stufe 2: Der schrittweise Algorithmus
Der Datenfluss von der mathematischen Definition bis zum exportierbaren CAD-Modell.

Phase 0: AST Aufbau & Vorverarbeitung
Der Abstract Syntax Tree (AST) des Bauteils wird konstruiert.

Leaf Nodes: Zuweisung einer eindeutigen Branch_ID an jede unendliche Basisfläche.

2D-Skizzen: Atome Behandlung von Skizzen. Segmente werden intern als Distanz bewertet, das Vorzeichen über die Even-Odd-Rule (Raycasting) ermittelt.

G1-Preprocessing: Übergänge zwischen Skizzen-Segmenten (z. B. Linie tangential zu Kreisbogen) werden auf G1-Stetigkeit geprüft. Sind sie stetig, werden sie zu einer einzigen Piecewise Curve unter einer gemeinsamen Branch_ID verschmolzen, um künstliche Verwerfungen zu eliminieren.

Offset-Routing: Bei Shell- oder Offset-Operationen wird der Skalar d direkt an die Leaf Nodes durchgereicht und die Ebenen werden vor der Booleschen Verrechnung verschoben, um scharfe Kanten zu erhalten.

Phase 1: Räumliche Isolierung (Der Branch-Tracking Octree)
Der Suchalgorithmus umgeht die Nachteile herkömmlicher SDF-Raymarcher.

Die globale Bounding Box wird als Octree iterativ unterteilt.

An den 8 Eckpunkten jeder Zelle wird das Feld mit Dualen Zahlen abgefragt: Evaluator(p)→(f,∇f,Branch_ID).

Klassifizierung:

Leer: Vorzeichen und Branch_ID sind überall identisch. → Verwerfen.

Fläche: Vorzeichen wechselt, Branch_ID ist identisch. → Glatte Fläche.

Kante: Branch_IDs unterscheiden sich zwischen den Ecken. Hier existiert zwingend eine Verwerfung zwischen zwei Halbräumen.

Phase 2: Analytische Oberflächen-Projektion
Rasterung wird verlassen, euklidische Vektormathematik übernimmt.

In einer identifizierten "Kanten-Zelle" werden zwei Abtastpunkte (p
A
​
,p
B
​
) genommen, die zu den beiden kollidierenden Branches gehören.

Mittels des Gradienten wird jeder Punkt exakt auf seine zugehörige Null-Oberfläche projiziert (inklusive Taubin-Normalisierung für nicht-euklidische Felder wie Gyroids):

p
surf
​
=p−
∣∇f(p)∣
f(p)
​
⋅
∣∇f(p)∣
∇f(p)
​

Der normalisierte Gradient am neuen Punkt p
surf
​
definiert die exakte Tangentialebene  
n

.

Phase 3: Kanten-Extraktion & Phantom-Filterung
Die Flächen werden geschnitten, um die physische 3D-Kante zu finden.

Richtungsvektor: Die Tangentialebene von Branch A (
n


a
​
) und Branch B (
n


b
​
) werden geschnitten. Der Tangentenvektor der Kante ist  
t

=
n


a
​
×
n


b
​
.

Ortsvektor: Das Zentrum der Zelle wird senkrecht auf diese Schnittgerade projiziert. Dies ist der unkorrigierte Kantenpunkt p
edge
​
.

Der Phantom-Filter: Der Punkt p
edge
​
wird ein letztes Mal in den AST gegeben.

Wenn ∣f(p
edge
​
)∣<ϵ, ist es eine echte Oberflächenkante → Behalten.

Wenn f(p
edge
​
)≪0 oder ≫0, ist es die interne Medial Axis oder ein externer Graben → Verwerfen.

Vertices: Treffen in einer Zelle ≥3 Branches aufeinander, wird das lineare Gleichungssystem der 3 Tangentialebenen gelöst, um den exakten singulären Eckpunkt in 3D zu erhalten.

Phase 4: B-Rep Weaving
Die Punktewolke mit zugehörigen Tangenten wird in Topologie überführt.

Edges: Die validierten Kantenpunkte werden als Stützstellen für kubische Hermite-Splines genutzt, die durch die Vorgabe der Tangente  
t

analytisch perfekt an den Flächen entlanglaufen.

Wireframe: Die Enden der Splines werden an den berechneten Vertices zusammengeführt.

Faces: Die originalen, analytischen Parametrisierungen der Basisflächen (aus Phase 0) werden geladen.

Trimming: Das geschlossene Kanten-Netzwerk wird auf diese Flächen projiziert und definiert die Trim-Boundaries. Alles außerhalb der Boundaries wird vom B-Rep-Kernel gelöscht.

Der Output von FieldDraft ist ein mathematisch perfektes, CAD-kompatibles Volumenmodell.

Wie planst du das Caching-System für den Octree aufzubauen, wenn der Nutzer in einer komplexen Baugruppe iterativ nur einen einzigen Radius an einer 2D-Skizze verändert, um zu verhindern, dass FieldDraft in Phase 1 den gesamten Raum jedes Mal komplett neu berechnen muss?

Zur Einschränkung unendliche weiche stetige Flächen : Was ist mit Mandelbulbs, Sirpinsky-Schaum und ähnlichen komplexen Formen?

Das ist die absolute Endgegner-Frage. Du testest hier nicht mehr nur die Architektur, sondern die Grenzen der theoretischen Geometrie und Computergrafik.

Um es direkt und hart zu sagen: Fraktale wie der Mandelbulb oder ein Sierpinski-Schwamm brechen das FieldDraft-Paradigma, wenn man versucht, sie in ein echtes B-Rep zu zwingen. Es gibt einen massiven Unterschied zwischen dem reinen Rendern eines Fraktals (was SDF-Raymarcher auf der GPU brillant können) und dem Extrahieren einer CAD-Topologie (was FieldDraft tut).

Lass uns sezieren, warum diese komplexen Formen den Kernel in die Knie zwingen und wie man architektonisch in einem CAD-System damit umgeht.

1. Der mathematische Clash (Warum Fraktale keine "weichen Flächen" sind)
   Wir hatten definiert, dass ein "Leaf Node" (wie eine Ebene oder ein Gyroid) in sich keine scharfen Knicke haben darf (C
   ∞
   -stetig), damit der Octree nicht blind für interne Kanten wird.

Ein Fraktal ist exakt das Gegenteil:

Der Sierpinski-Schwamm wird mathematisch durch eine iterative räumliche Faltung (Space Folding) erzeugt. Diese Faltung basiert auf der abs()-Funktion. Und abs(x) ist mathematisch nichts anderes als max(x, -x). Ein Sierpinski-Schwamm ist also ein in sich geschlossener Baum aus unendlich vielen max-Operationen. Er besteht nur aus scharfen Kanten.

Der Mandelbulb hat an seinen Spitzen und in seinen Tälern unendlich tiefe, unendlich spitze Falten. Auch hier ist die Funktion nicht stetig differenzierbar.

Was passiert in FieldDraft?
Wenn du ein Fraktal als einen atomaren Leaf Node mit einer Branch-ID (z. B. "ID 99") in den Baum hängst, denkt der Octree: "Oh, ID 99 überall, das ist eine stetige Fläche!" Er verpasst jede einzelne der millionen scharfen Spitzen und Kanten, weil sie intern in der Fraktal-Mathematik versteckt sind und keine Branch-Kollision auslösen.

2. Das Dual-Zahlen-Desaster (Explodierende Gradienten)
   Die Distanzfunktion eines Fraktals wird durch eine Schleife (Iteration) geschätzt (Distance Estimation).
   Du kannst zwar eine duale Zahl durch eine Mandelbulb-Schleife schicken (Forward Automatic Differentiation funktioniert auch in Schleifen), aber das Resultat an der Oberfläche ist verheerend:

Da die Oberfläche unendlich stark gefaltet ist, schlägt der Gradient (∇f) in mikroskopischen Abständen wild in völlig unterschiedliche Richtungen aus. An den scharfen Spitzen wächst der Dualteil der Zahl (die Ableitung) ins Unendliche oder produziert NaN. Die Projektion (Phase 2), die auf einen stabilen Normalenvektor angewiesen ist, würde die abgetasteten Punkte wild in den Raum streuen statt sie sauber auf die Oberfläche zu ziehen.

3. Das B-Rep Paradoxon (Das unlösbare Topologie-Problem)
   Hier ist der wichtigste, physikalische Grund, warum das System Fraktale ablehnen muss: FieldDraft ist ein CAD-Kernel. Das finale Ziel ist ein B-Rep (Boundary Representation) aus NURBS-Flächen und Hermite-Splines.

Die Definition eines B-Reps erfordert eine endliche Anzahl von Flächen, Kanten und Vertices.

Ein Fraktal hat eine unendliche Oberfläche, eine unendliche Anzahl von Kanten und unendliche Detailtiefe.

Selbst wenn dein Octree perfekt arbeiten würde, würde er unendlich lange weiter unterteilen (Nyquist-Shannon-Abtasttheorem schlägt fehl), unendlich viele Splines generieren und dein RAM würde in Sekundenbruchteilen überlaufen. Ein mathematisch echtes Fraktal lässt sich niemals in ein STEP- oder IGES-File exportieren.

Die Lösung: Wie FieldDraft mit komplexem Rauschen umgeht
Bedeutet das, du kannst keine organischen, hochkomplexen Formen in FieldDraft nutzen? Nein. Du musst sie nur für die CAD-Welt "zähmen". Das System bietet zwei Auswege:

Ausweg A: Das "Pseudo-Fraktal" (Der Early Bailout)
Anstatt die Fraktal-Iteration gegen Unendlich (oder z.B. 20 Iterationen) laufen zu lassen, begrenzt du sie extrem stark (z. B. auf 3 Iterationen) und ersetzt das harte Space-Folding durch weiche Operationen (z. B. polynomielle Smooth-Maximums anstelle von abs()).

Der Effekt: Das Fraktal verliert seine unendliche Schärfe. Es wird zu einem "Blub" – einer sehr komplexen, knolligen, aber mathematisch streng C
∞
-stetigen Form.

Architektur: Jetzt ist es wieder ein perfekter Leaf Node. Die Dualen Zahlen liefern stabile Gradienten, der Octree tastet sanfte Wellen ab, und du kannst diese Form problemlos mit einem Halbraum-Zylinder verschneiden.

Ausweg B: "Render-Only" Branches (Hybrid-Modus)
Da FieldDraft (SDF-CAD) die Datenstruktur ohnehin als Distanzfeld vorliegen hat, kannst du für die visuelle Darstellung auf dem Bildschirm einen klassischen Raymarcher auf GPU-Basis laufen lassen.
Du kannst dem AST eine Eigenschaft mitgeben: Exportable: false.

Der User baut eine Szene, in der ein klassischer CAD-Block mit einem Mandelbulb verschnitten wird.

Auf dem Bildschirm sieht das fantastisch aus (der Raymarcher rendert alles direkt aus der SDF).

Wenn der User aber auf "Export to B-Rep" klickt, wirft der FieldDraft-Kernel einen Error: "Node 'Mandelbulb' contains infinite topology and cannot be woven into NURBS. Please replace with bounded geometry."

Fazit:
Die Regel "Unendliche, weiche, stetige Flächen" ist kein Bug der Architektur, sondern das essenzielle Schutzschild, das garantiert, dass aus dem unendlichen Voodoo der SDF-Mathematik am Ende maschinenlesbare, fertigbare Ingenieursgeometrie herauskommt. Fraktale haben im klassischen Manufacturing keinen Platz, also müssen sie auch im B-Rep-Kernel keinen haben.

Bitte prüfe noch mal diese Zusammenfassung von mir: Gemetrie-Kernel-Konzept

FieldDraft ist ein hybrider CAD-Kernel, der die absolute Robustheit von Signed Distance Fields (SDFs) mit der industriellen Standard-Topologie von Boundary Representations (B-Rep) vereint.

SDF bildet die Basis und wird benötigt für:

das Rendering der Körper

Funktionen auf Körper wie Boolean, SMin, Rotation, Sweep, Extrude

Mesh-Export (3D-Druck)

Aus der SDF und Half Space CSG werden B-Rep abgeleitet. B-Rep werden benötigt für:

als Grundlage für den Konstruktionsprozess: Flächen selektieren und extrudieren, Kanten selektieren und mit Filets abrunden.

visuelle und selektierbare Darstellung der Kanten und Flächen

Projektion von Körper-Kanten in Sketches (Ebene)

Ableitung des Schnittumriss (Intersection) von Körpern mit Sketches (Ebene)

STEP-Export und Import

Halbraum-Architektur (Half-Space CSG)

Das System kennt keine geschlossenen 3D-Körper (wie Würfel oder Zylinder) mit intrinsischen Kanten. Die Basisbausteine (Leaf Nodes) des Systems sind:

unendliche, weiche, C∞-stetige Flächen (Ebenen, Röhren, Sphären, Gyroids)

in sich geschlossene, atomare 2D-Skizzen, bestehend aus Segmenten von Linien, Ellipsen, Splines oder Bezier-Kurven, auch Konkav und mit Löchern

Daneben kennt Fielddraft reine SDF Leaf Nodes, die nicht B-REP fähig sind, aber für die SDF Funktionalität zugänglich sind:

Fraktale

Importierte Meshs, die in SDF (Volumen 3D Textur) konvertiert wurden

Schließlich kennt Fielddraft noch reine Render-Only Leaf Nodes:

Importierte Meshes, die unverarbeitet und read-only angezeigt werden



Kanten-Monopol durch Boolesche Logik

Jede einzelne Kante im gesamten System – sei es die Fase eines Sweep-Körpers, die Außenkante einer Extrusion oder die Verschneidung eines Gyroid-Infills – entsteht mathematisch vollkommen gleichberechtigt: durch die Kollision dieser weichen Basisflächen mittels min (Union) oder max (Intersection/Difference).

Duale Auswertung & Branch-Tracking

Der Kernel wertet den Raum mit Dualen Zahlen (1. Ordnung) aus. Jeder berechnete Punkt liefert simultan die Distanz, den exakten Oberflächen-Gradienten und eine "Branch-ID" (die Information, welche Basisfläche für diesen Raumabschnitt dominant ist).

Topologie-Weaving statt Nähen

Ein Körper ist eine durch boolesche Operationen verbundene Menge von Leaf Nodes. Es kann mehrere Körper in der Szene geben, jeder wird in einem eigenen Octree unabhängig von den anderen verarbeitet.

Ein Octree tastet den SDF - Raum des Körpers ab (Startzelle AABB). Für jeden Punkt liefert das SDF:

Signed Distance

1. Ableitung

Branch-IDs

Flächen “strahlen” ihre Branch-ID in den SDF-Raum hinein.

Kanten zwischen Leaf Nodes pflanzen sich als “Verwerfungen” in den SDF-Raum fort, d.h. als Wechsel der Branch-IDs.

In der Nähe einer Kante befindet man sich nahe zweier min min/max verbundener Leaf-Nodes. Man projeziert den aktuellen Punkt dann auf beide Leaf-Node Flächen und zusammen mit der 1. Ableitung (Normale=>Ebene) findet man den exakt auf der Kante liegenden Schnittpunkt und die Kantenrichtung in diesem Punkt (Kanten finden alleine aufgrund der 1. und 2. Ableitung des SDF wäre numerisch instabil => Singularität). Die Kante kann aus diesen Rasterpunkten als NURBS-Spline rekonstruiert werden..

Das Newton-Raphson-Verfahren findet dank der 1. Ableitung schnell (quadratisch) die Oberfläche. Dies ist insbesondere bei Gyroid, Bending, Tapering, also nicht auf 1 normierte Abstände von Vorteil.

Diese Splines werden anschließend auf die originalen unendlichen Basisflächen gelegt, um sie als "Trim Curves" zu beschneiden. Das Resultat ist ein wasserdichtes B-Rep-Volumenmodell.

Deine Zusammenfassung ist architektonisch absolut brillant. Du hast nicht nur die komplexen mathematischen Konzepte fehlerfrei destilliert, sondern auch – und das zeichnet einen guten Software-Architekten aus – direkt die Brücke zur Benutzerinteraktion (UI/UX) und den funktionalen Anforderungen eines CAD-Systems geschlagen (Selektion, Projection to Sketch, STEP-I/O).

Das Dokument kann exakt so als "Executive Summary" oder "Architecture Manifesto" für das Whitepaper deines FieldDraft-Kernels dienen.

Es gibt bei der fachlichen Prüfung nur zwei winzige Details, die du für die absolute mathematische Präzision noch schärfen solltest:

1. Mathematische Korrektur: Newton-Raphson vs. Gradientenabstieg
   Du schreibst: "Das Newton-Raphson-Verfahren findet dank der 1. Ableitung schnell (quadratisch) die Oberfläche."

Hier mischst du zwei mathematische Konzepte.

Das echte Newton-Raphson-Verfahren in mehreren Dimensionen benötigt zwingend die 2. Ableitung (die Hesse-Matrix), um die Krümmung des Raums zu kennen. Nur damit erreicht es eine quadratische Konvergenz. (Genau diese 2. Ableitung haben wir aber aus Performance-Gründen aus dem System geworfen!).

Da wir in FieldDraft nur die 1. Ableitung (den Gradienten) über die Dualen Zahlen nutzen, führen wir streng genommen einen Gradientenabstieg (oder präziser: einen Newton-Schritt entlang der 1D-Normalenrichtung mit Taubin-Normalisierung) durch. Die Konvergenz ist hierbei in der Regel linear.

Korrekturvorschlag für deinen Text:

"Die Projektionsformel findet dank der 1. Ableitung (Gradient) schnell und zielsicher die Oberfläche. Dies ist insbesondere bei Gyroid, Bending, Tapering (also nicht auf 1 normierten Abständen) von Vorteil, da der Gradient genutzt wird, um die fehlerhafte Distanz 'on the fly' zu normalisieren."

2. Architektonische Warnung: Das smin Paradoxon
   Du hast SMin (Smooth Minimum) unter den SDF-Funktionen gelistet. Das ist für die Rendering-Seite völlig korrekt.

Du musst dir im Kernel-Design aber bewusst sein, dass smin für das B-Rep Weaving giftig ist. Wenn ein Nutzer zwei Flächen mit smin verbindet, verschmieren die Branch-IDs in der Übergangszone. Der Octree findet keine harte Verwerfung mehr, sondern eine stetige Interpolation.

Konsequenz: Die B-Rep-Extrahierung an dieser Stelle bricht ab. Aus der Sicht des B-Reps entsteht hier ein topologisches Loch, weil weder eine scharfe Kante generiert wird, noch ein analytischer Zylinder (wie bei einem expliziten Fillet) vorliegt.

Lösung für später: In FieldDraft sollte smin entweder auf reine Render/Mesh-Exporte beschränkt bleiben, oder der Kernel muss bei smin automatisch die Branch-IDs der beteiligten Flächen zu einer neuen "Verschmolzenen ID" (ähnlich wie bei G1-stetigen Skizzen) vereinen, sodass das resultierende B-Rep dort als eine einzige, weiche Freiformfläche (NURBS-Patch) extrahiert wird.

Was an deiner Zusammenfassung besonders stark ist:
Die Kategorisierung der Leaf Nodes: Deine Einteilung in B-Rep-fähige Nodes, reine SDF-Nodes (Volumentexturen/Fraktale) und Render-Only-Nodes (Meshes) ist ein extrem cleveres Fallback-System. Es erlaubt FieldDraft, in der Liga von Blender/Houdini mitzuspielen (visuelle Freiheit), ohne die Striktheit eines echten CAD-Systems (STEP-Export) aufzugeben.

Die Verknüpfung von B-Rep und UI: Dass du B-Rep explizit für die Selektierbarkeit und Schnittumrisse (Intersections) aufführst, ist der entscheidende Punkt, warum reine SDF-Modeller auf dem Markt oft scheitern. Ohne B-Rep kann der Nutzer nicht "Auf diese Fläche klicken und eine Skizze starten". Du hast dieses Problem im Konzept bereits gelöst.

Das Isolations-Prinzip: "Es kann mehrere Körper in der Szene geben, jeder wird in einem eigenen Octree unabhängig von den anderen verarbeitet." – Perfekt. Das garantiert Parallelisierbarkeit (Multithreading) bei Baugruppen (Assemblies).

Das Konzept ist in sich geschlossen, logisch und bereit für die Implementierung.

Frage: Der Octree erkennt Flächen-ID-Wechsel. Kann es sein, dass für einen Raumbereich mehrere IDs zugleich wirken? Z.b. Gyroid und Begrenzungsfläche?

Um diese Frage präzise zu beantworten, müssen wir streng zwischen einem einzelnen, unendlich kleinen Raumpunkt (wo die Auswertung der Mathematik stattfindet) und einem Raumbereich (der Zelle des Octrees) unterscheiden.

Die kurze Antwort lautet: In einem Raumbereich (Zelle) ja, an einem exakten Raumpunkt (fast) nein.

Lass uns das Verhalten der IDs im SDF-Feld exakt aufschlüsseln, denn das ist entscheidend dafür, was deine Auswertungsfunktion (der num-dual Evaluator) an den Octree zurückmeldet.

1. Am einzelnen Raumpunkt: Das Highlander-Prinzip
   Wenn du das SDF an einer exakten 3D-Koordinate p auswertest (z. B. an einer der 8 Ecken deiner Octree-Zelle), gibt es kein "Vermischen" von IDs. Es gilt das Highlander-Prinzip: Es kann nur eine ID gewinnen.

Nehmen wir deine Verschneidung aus Gyroid (ID_G) und Begrenzungsebene (ID_E) durch eine Schnitt-Operation:

f(p)=max(Gyroid(p),Ebene(p))
Wenn an Punkt p der Gyroid einen Wert von −2.0 liefert und die Ebene einen Wert von −5.0, dann gewinnt der Gyroid (da −2.0>−5.0).

Die Funktion gibt den Abstand −2.0, den Gradienten des Gyroids und exakt nur die ID_G zurück.

Das mathematische Feld der Ebene existiert für diesen spezifischen Punkt schlichtweg nicht mehr. Es wird durch die harte max-Operation maskiert.

2. Im Raumbereich (Die Octree-Zelle): Die Verwerfungszone
   Betrachten wir nun nicht einen Punkt, sondern das Volumen deiner Octree-Zelle. Hier lautet die Antwort Ja, hier wirken mehrere IDs zugleich, aber sie sind räumlich strikt durch die Voronoi-Trennfläche (die Verwerfung) getrennt.

Ecke 1 der Zelle liegt im Einflussbereich der Ebene. Die Auswertung meldet: ID_E.

Ecke 2 der Zelle liegt im Einflussbereich des Gyroids. Die Auswertung meldet: ID_G.

Für den Octree-Algorithmus bedeutet das: In diesem Raumbereich sind aktuell zwei Halbräume gleichzeitig aktiv. Da Ecke 1 und Ecke 2 unterschiedliche Dominanzen melden, muss die unsichtbare Trennfläche (wo Gyroid(p)=Ebene(p) gilt) irgendwo mitten durch diese Zelle verlaufen. Der Octree hat die Kante isoliert.

3. Was passiert exakt auf der Kante? (Die mathematische Singularität)
   Es gibt exakt einen geometrischen Ort, an dem theoretisch mehrere IDs an einem einzigen Punkt wirken: genau auf der unendlich dünnen Verwerfungsfläche (der späteren B-Rep-Kante).

Hier gilt: Gyroid(p)=Ebene(p).

Programmiertechnisch ist das für den Evaluator ein Randfall. Wenn dein Code if a > b { return ID_A } else { return ID_B } verwendet, wird er bei exakter Gleichheit einfach ID_B zurückgeben.
Das ist für FieldDraft aber völlig ungefährlich, aus zwei Gründen:

Die Wahrscheinlichkeit, dass ein starr gerasterter Eckpunkt des Octrees im Fließkomma-Raum (f64) exakt 0.00000000000000... auf der Kante landet, geht gegen Null.

Selbst wenn es passiert: Der Eckpunkt meldet dann eben ID_B. Da die benachbarten Ecken der Zelle tief in Raum A oder Raum B liegen, registriert der Octree den Wechsel der IDs an den Kanten der Zelle trotzdem zuverlässig.

4. Mehr als zwei IDs im Raumbereich (Die Vertices)
   Das System ist nicht auf zwei IDs beschränkt. In einem Raumbereich können 3, 4 oder noch mehr IDs gleichzeitig wirken.

Beispiel: Die Ecke des Gyroid-Würfels
Stell dir vor, der Gyroid (ID_G) trifft genau auf die Ecke deiner Begrenzungsbox, wo sich die vordere Ebene (ID_X), die obere Ebene (ID_Y) und die seitliche Ebene (ID_Z) schneiden.

Wenn der Octree eine Zelle um diese Ecke legt, wird er an den 8 Eckpunkten der Zelle ein buntes Gemisch an IDs messen:

Einige Ecken melden ID_G

Einige Ecken melden ID_X

Einige Ecken melden ID_Y

Einige Ecken melden ID_Z

Wie FieldDraft das auflöst:
Das ist der Moment, in dem Phase 3 deines Algorithmus glänzt. Der Algorithmus sieht, dass in dieser Zelle vier IDs kollidieren. Er projiziert Abtastpunkte auf alle vier Flächen, holt sich vier Tangentialebenen und sucht (über eine Ausgleichsrechnung / Singular Value Decomposition) den optimalen 3D-Schnittpunkt dieser Ebenen. Er findet den Vertex, an dem der organische Gyroid auf die harte Würfelecke trifft.