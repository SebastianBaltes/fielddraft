# Spezifikation der 3D-Meshing-Pipeline (Direkte 3D-Berechnung)

Dieses Dokument beschreibt die Architektur der 3D-Meshing-Pipeline für den Geometry-Kernel FieldDraft. Das Verfahren kombiniert die exakten Grenzrepräsentationen (BRep) mit der Topologie von Signed Distance Fields (SDF). Die nachfolgende Spezifikation bezieht sich auf die Triangulierung eines einzelnen, topologisch zusammenhängenden Objekts (Connected Component). Bei Vorliegen disjunkter Körperstrukturen wird die Pipeline pro zusammenhängender Komponente separat initiiert. Der Algorithmus vermeidet typische Fehlstellen der 3D-Delaunay-Triangulierung durch algorithmische Vorverarbeitung, deterministische Punktverteilung und topologisches Merging.

### Begriffsdefinitionen

* **BRep (Boundary Representation):** Die CAD-Darstellung eines Körpers durch exakte Grenzflächen und Kanten. Diese Kanten dienen der Pipeline als feste topologische Begrenzungen und Startlinien für den Meshing-Prozess.
* **SDF (Signed Distance Field):** Eine skalare Feldfunktion, die für jeden Punkt im 3D-Raum den kürzesten euklidischen Abstand zur Modelloberfläche liefert. Das Vorzeichen kodiert die Position relativ zum Volumen (z. B. negativ für das Innere des Körpers, positiv für das Äußere).
* *p*: Ein abgetasteter Punkt (Seed), der exakt auf der SDF-Oberfläche liegt (*f(p) \= 0*). Diese Menge an Punkten bildet die Vertices des finalen Dreiecksnetzes.
* *U(p)*: Der lokale zulässige Umkreis-Radius (verwandt mit der *Local Feature Size*; präzise definiert in Abschnitt 2) um den Punkt *p*. Dies definiert eine Kugel um *p*, in der die Oberfläche stetig verläuft, die Krümmung unter einem definierten Schwellenwert liegt und keine fremden Oberflächenzweige eindringen. Innerhalb dieses Radius *U(p)* sind die Oberflächenpunkte näherungsweise koplanar. Somit ist die euklidische 3D-Distanz in diesem Intervall eine zulässige Näherung der geodätischen Distanz.
* *G*: Der Nachbarschaftsgraph (ungerichtet), der die validierten topologischen Beziehungen aller Mesh-Punkte (Knoten) vor der eigentlichen Triangulierung definiert. Kanten in *G* repräsentieren gültige, euklidisch-nahe Verbindungen auf der Oberfläche.

### 1. Oberflächenabtastung und initiale Seed-Punkte

Die Konstruktion des Graphen erfordert eine Menge an Oberflächenpunkten *p*, die die Geometrie lückenlos abtasten. Die Initialisierung dieses Punktesets startet an definierten Begrenzungen:

* **Regelfall (BRep vorhanden):** Die diskretisierten BRep-Ecken und -Kanten fungieren als initiale Startpunkte.
* **Ausnahmefall (Parameterfreie Geometrien):** Bei vollständig geschlossenen oder unbegrenzten Flächen ohne BRep-Grenzen (z. B. einer Kugel oder einem Gyroid) wird mindestens ein initialer Seed-Punkt für das zusammenhängende Objekt benötigt. Dieser wird performant über einen Gradientenabstieg (Newton-Raphson-Verfahren) ausgehend von einer Startkoordinate in der Bounding Box ermittelt: *p\[n+1] \= p\[n] − f(p\[n]) · ∇f(p\[n])*. Der Punkt konvergiert iterativ auf die Iso-Oberfläche (*f(x) \= 0*).

#### Wachstumsalgorithmus (Waben-Ausbreitung)

Ausgehend von den initialen Startlinien oder Seed-Punkten wächst die Punktverteilung iterativ über die gesamte zusammenhängende Oberfläche. Die Identifikation neuer "innerer Oberflächenpunkte" erfolgt über ein lokales hexagonales Waben-Muster:

1. Für einen bestehenden validierten Punkt *p* wird die lokale Tangentialebene anhand der Oberflächennormalen *n \= ∇f(p)* definiert.
2. Zur Konstruktion des Sechsecks auf dem Umkreis mit Radius *U(p)* in dieser Ebene wird ein Referenz-Tangentenvektor *u* für die Orientierung definiert:
   * **Fortführung (Kantenbasiert):** Startet das Wachstum ausgehend von einer BRep-Kante oder einem bereits teilvernetzten Bereich, wird *u* kollinear zur bestehenden Kante oder Verbindungslinie ausgerichtet.
   * **Initialer Seed (Isoliert):** Startet das Wachstum an einem isolierten Seed-Punkt, wird *u* entlang der **maximalen Hauptkrümmungsrichtung** (*d\[max]*) ausgerichtet (abgeleitet aus der Eigenwertzerlegung der Hesse-Matrix des SDFs). Diese Orientierung erzwingt einen Kantenfluss, der der natürlichen geometrischen Form (z. B. einem Zylinder) folgt. Handelt es sich um eine planare oder sphärische Fläche (Nabelpunkt), wird ein arbiträrer, zu *n* orthogonaler Vektor gewählt.
3. Es werden sechs äquidistante Kandidatenpunkte auf dem Kreis in der Tangentialebene generiert.
4. Diese Kandidatenpunkte werden mittels Gradientenabstieg (SDF-Projektion) exakt auf die gekrümmte Iso-Oberfläche *f(x)\=0* fallengelassen.
5. **Konfliktauflösung:** Unterschreitet ein generierter Kandidatenpunkt den geforderten Toleranzabstand (abhängig von *U(p)*) zu bereits bestehenden Punkten *q* oder BRep-Kanten, wird er nicht verworfen, um Lücken in der Abtastung zu vermeiden. Stattdessen wird seine Position iterativ so korrigiert, dass er einen gleichmäßigen, ausbalancierten Abstand zwischen dem Ursprungspunkt *p* und den benachbarten Konfliktpunkten *q* aufweist. Nach dieser räumlichen Ausbalancierung wird der Punkt erneut exakt auf die Iso-Oberfläche projiziert und in die globale Punktmenge aufgenommen.
6. **Adjazenz-Registrierung:** Mit der Aufnahme in die globale Punktmenge werden alle sechs Waben-Eckpunkte (einschließlich der korrigierten Konfliktpunkte) als Knoten in einer dynamischen Adjazenzliste registriert. Die topologische Verbindung zwischen dem Ursprungspunkt *p* und seinen sechs Waben-Nachbarn sowie die Ring-Verbindungen zwischen den Waben-Eckpunkten selbst werden direkt als Kanten in diese Liste eingetragen. Über diese sechs frischen Nachbarn hinaus werden zusätzlich alle bereits existierenden Punkte innerhalb von *U(p)* als (ungerichtete) Nachbarn von *p* aufgenommen — insbesondere BRep-Ecken und -Kantenpunkte sowie Punkte fremder Waben, die beim Umlauf um eine gekrümmte Oberfläche oder beim Zusammentreffen zweier Wachstumsfronten in die Umgebung von *p* geraten; ohne diese Verknüpfung blieben an solchen Patch-Grenzen Lücken im Graphen. Jede Kante zu einem existierenden Punkt *q* wird jedoch vorab per SDF-Schnitttest validiert (Abschnitt 3, Schritt 4): Taucht das Liniensegment *p*–*q* ins Materialinnere ("hintere Wand", negatives SDF), wird die Verbindung verworfen — so werden räumlich nahe, aber topologisch getrennte Punkte (etwa auf der Rückseite einer dünnen Wand) gar nicht erst als Nachbarn eingetragen. Diese kontinuierlich befüllte Adjazenzliste bildet die strukturelle Basis für die Konstruktion des Nachbarschaftsgraphen *G*.

### 2. Algorithmische Konstruktionsvorschrift für den Radius *U(p)*

Der zulässige Radius *U(p)* an einem Oberflächenpunkt *p* wird definiert als das Minimum aus drei distinkten Limitierungen: der lokalen Krümmung, dem Abstand zu BRep-Grenzen und dem topologischen Reach (Distanz zur äußeren Medialachse). Die formale Konstruktionsvorschrift lautet:

*U(p) \= min(R\[curv], R\[BRep], R\[reach])*

Die drei Komponenten werden wie folgt berechnet:

1. *R\[curv]* **(Krümmungsbegrenzung):** Verhindert topologische Überschneidungen und zu große Planaritätsabweichungen in stark gekrümmten Bereichen. Die Berechnung erfolgt über die maximale Hauptkrümmung *κ\[max]* im Punkt *p*, abgeleitet aus der Hesse-Matrix des SDFs. Es gilt *R\[curv] \= α/κ\[max]*, wobei *α* einen benutzerdefinierten Toleranzfaktor für die maximale Sehnenabweichung darstellt.
2. *R\[BRep]* **(Distanz zu topologischen Grenzen):** Garantiert, dass generierte Dreiecke nicht über exakte CAD-Grenzen hinauswachsen. *R\[BRep]* ist die kürzeste euklidische Distanz von *p* zur nächsten BRep-Kante oder BRep-Ecke. Befindet sich *p* selbst auf einer Kante, wird die Distanz zu den nächsten benachbarten Diskretisierungspunkten dieser Kante aus dem Graphen *G* gemessen.
3. *R\[reach]* **(Topologische Filterung und äußerer Reach):** Ermittelt die Distanz zu Fremdbranchen der Oberfläche mittels eines nach außen gerichteten Marching-Verfahrens mit Fußpunkt-Verfolgung (Footpoint Tracking).
   * **Initialisierung:** Ein Suchstrahl wird entlang der nach außen zeigenden Oberflächennormalen *n \= ∇f(p)* definiert: *x(t) \= p + t · n*.
   * **Iteration:** Für schrittweise steigendes *t > 0* wird der projizierte Fußpunkt *q(t)* auf der Iso-Oberfläche berechnet: *q(t) \= x(t) − f(x(t)) · ∇f(x(t))*.
   * **Abbruchbedingung:** Solange die Abweichung des Fußpunkts vom Ursprung minimal ist (*||q(t) − p|| \< ε*), liegt keine Fremdbranche vor. Springt der Fußpunkt signifikant über die Toleranz *ε* hinaus, wurde die äußere Medialachse gekreuzt (z. B. Spalt zu einer anderen Fläche). Das entsprechende *t* definiert *R\[reach]*.
   * **Hinweis zur inneren Medialachse:** Ein Marching in das Material hinein (*t \< 0*) entfällt zur Performance-Optimierung vollständig. Die strukturelle Begrenzung durch das Materialinnere (Thin-Wall-Problem) wird stattdessen in Abschnitt 3 durch den SDF-Sichtlinien-Test abgefangen. Dies maximiert *U(p)* auf dünnen, planaren Wänden.

Die über diesen Radius und das Waben-Muster generierte Punktverteilung kann optional durch abstoßende Partikel-Relaxation (auf *f(x)\=0* beschränkt) homogenisiert werden, um numerische Entartungen im finalen Netz präventiv aufzulösen.

### 3. Konstruktion des Nachbarschaftsgraphen *G*

Vor der Triangulierung wird der ungerichtete Nachbarschaftsgraph *G* aller validierten Oberflächenpunkte finalisiert. Dieser Prozess konsolidiert die während des Wachstums erfassten Adjazenzlisten, integriert zwingend die exakten CAD-Grenzen und filtert Kurzschlüsse durch das Volumen:

1. **BRep-Ecken (Vertices):** Die exakten Eckpunkte der Boundary Representation werden direkt als initiale Knoten in den Graphen *G* eingefügt.
2. **BRep-Kanten (Edges):** Die eindimensionalen Begrenzungen werden vorab unterteilt (diskretisiert). Die Unterteilung erfolgt so, dass ein definierter Schwellwinkel der geometrischen Krümmung zwischen benachbarten Segmenten nicht überschritten wird. Die resultierenden Diskretisierungspunkte sowie ihre linearen Nachbarschaftsbeziehungen entlang der BRep-Kante werden als feste Knoten und Kanten in *G* registriert.
3. **Innere Oberflächenpunkte:** Die identifizierten und gefilterten Oberflächenpunkte *p* werden als Knoten in *G* eingefügt. Die im Wachstumsalgorithmus (Abschnitt 1, Schritt 6) kontinuierlich erfasste Adjazenzliste — inklusive Waben-Ringverbindungen und der dort bereits registrierten Innerhalb-*U(p)*-Nachbarschaften — wird direkt als initiale Kantenmenge übernommen. Beim Wachstum noch nicht erfasste Paare innerhalb der *U(p)*-Radien werden hier lediglich ergänzt, nicht neu abgeleitet.
4. **SDF-Sichtlinien-Test (Line-of-Sight):** Jede Kante — aus der Adjazenzliste (Abschnitt 1, Schritt 6) wie auch neu ergänzt — zwischen zwei Knoten *p[1]* und *p[2]* wird final validiert. Verworfen wird sie genau dann, wenn das euklidische Liniensegment *p[1]*–*p[2]* durch das Materialinnere ("hintere Wand", negatives SDF-Volumen) verläuft, ohne dabei gültige tangentiale Nachbarn zu treffen. Der Test besteht aus einem lokalen Primärkriterium an den Endpunkten und einer Absicherung entlang der Strecke:
   * **Abgangsrichtung (Primärkriterium, an beiden Enden):** Entscheidend ist, ob die Kante einen Endpunkt *tangential* verlässt (gültiger Nachbar) oder *ins Material eintaucht* (hintere Wand). Mit der Einheitsrichtung *d = (p[2]−p[1])/|p[1]−p[2]|* misst das Skalarprodukt *cos θ = n(p[1]) · d* (Oberflächennormale *n = ∇f*) genau das: bei einem tangentialen Nachbarn ist *cos θ ≈ 0*, bei einer hinteren Wand zeigt *d* entgegen der Außennormalen, also *cos θ ≈ −1*. Diskretisiert entspricht das einem kleinen Vortast-Schritt *h* von *p[1]* aus, dessen Vorzeichen/Betrag über *f(p[1] + h·d)* ausgewertet wird. Der Test läuft **von beiden Endpunkten** (*p[1]→p[2]* und *p[2]→p[1]*), da eine Kante tangential starten und erst nahe des anderen Endpunkts eintauchen kann.
   * **Krümmungsskalierte Schwelle:** Das bloße Vorzeichen genügt nicht. Auf einer *konvexen* Fläche liegt die Sehne zwischen zwei Oberflächenpunkten stets leicht innen — der kleine Schritt liefert dort *f(p[1] + h·d) ≈ −h · κ(p[1]) · L/2* (mit *L = |p[1]−p[2]|*), also ein legitimes, rein krümmungsbedingtes Negativ. Verworfen wird deshalb erst, wenn der Wert *deutlich* negativer ausfällt, als die lokale Krümmung erklärt: *f(p[1] + h·d) < −h · (κ(p[1])·L/2 + Marge)*. Die Hauptkrümmung *κ(p[1])* liegt aus der *R[curv]*-Berechnung (Abschnitt 2, Schritt 1) bereits vor.
   * **Backstop (Stützstellen entlang der Strecke):** Startet und endet eine lange Kante tangential, durchquert aber *dazwischen* ein Innenstück, greift kein Endentest. Als Absicherung wird der SDF an einigen Stützstellen entlang *p[1]*–*p[2]* (mindestens am Mittelpunkt *m = 0.5 · (p[1] + p[2])*) abgetastet und die Kante verworfen, sobald der Wert an einer Stelle relativ zur lokalen Skala signifikant negativ wird (*f < −ε*). Der Endpunkt-Test ist der billige, hochselektive Primärfilter; die Stützstellen sind das Netz darunter.
   * **Interpretation:** Im Kern ist dies ein Tangentialebenen-Seiten-Test — die bereits berechneten Normalen liefern *cos θ* direkt, ohne die SDF überhaupt abzutasten. Das SDF-Vortasten ist die diskretisierte Variante, die zusätzlich ein *Zurückkrümmen* der Oberfläche einfängt, und dient daher als Zweitsignal. Kanten, die eine dieser Prüfungen nicht bestehen, werden nicht in *G* aufgenommen bzw. aus *G* entfernt.

Die spätere Prüfung von Dreieckskanten und -knoten im Mesher erfolgt ausschließlich durch Iteration über die Kanten von *G*; fehlerhafte Verbindungen im 3D-Raum sind damit bereits vor der Triangulierung ausgeschlossen.

### 4. Berechnung der euklidischen 3D-Umkreisbedingung

Durch die Filterung in den Abschnitten 2 und 3 wird sichergestellt, dass alle für eine Kante relevanten Nachbarpunkte innerhalb von *U(p)* liegen und sich näherungsweise auf derselben koplanaren Flächenbranche befinden. Dies eliminiert die Notwendigkeit einer 2D-Projektion auf eine lokale Tangentialebene.

Die Umkreisbedingung wird direkt im *ℝ³* berechnet:

1. Gegeben sind drei Punkte *A, B, C ∈ ℝ³*.
2. Es wird der 3D-Mittelpunkt *M* sowie der Radius *R* des Kreises berechnet, der durch *A, B* und *C* verläuft.
3. Es erfolgt eine Iteration über alle Punkte *D*, die im Graphen mit *A, B* oder *C* verknüpft sind.
4. Die Umkreisbedingung gilt als verletzt, sofern für mindestens ein *D* die euklidische Distanz *dist(M, D) \< R* ist.

Die verbleibende Abweichung von der exakten geodätischen Metrik ist damit vernachlässigbar und erzeugt keine topologischen Fehler bei der Delaunay-Knotenauswahl.

### 5. Advancing-Front Delaunay-Mesher

Nach der Punktgenerierung und der Graphen-Initialisierung wird der Triangulierungs-Algorithmus ausgeführt. Dieser agiert als gerichtete Graphentraversierung unter Verwendung einer Half-Edge-Datenstruktur.

**Phase A: Initialisierung (Seed-Dreieck)**

1. Auswahl eines unverbundenen Startpunkts *A*.
2. Ermittlung des euklidisch nächsten Nachbarn *B* innerhalb des Graphen *G*.
3. Ermittlung eines dritten Punktes *C* aus der gemeinsamen Nachbarschaft von *A* und *B* in *G* mit folgenden Kriterien:
   * *C* erfüllt die leere 3D-Umkreisbedingung für das Dreieck *(A,B,C)*.
   * Die Abstandssumme *dist(A,C) + dist(B,C)* ist minimal.
4. Instanziierung des initialen Dreiecks. Die Knotenreihenfolge wird konsistent gegen den Uhrzeigersinn (Counter-Clockwise, CCW) orientiert, determiniert durch den lokalen SDF-Gradienten (Oberflächennormale).

**Phase B: Definition der Front**

* Die "Front" bezeichnet das Set aller gerichteten Halbkanten, die zum aktuellen Ausführungszeitpunkt exakt einem Dreieck (Face) angehören.
* Eine Halbkante *A → B* verbleibt in der Front-Menge, bis ihr gerichtetes Gegenstück *B → A* durch ein angrenzendes Dreieck generiert wird.

**Phase C: Traversierung (Advancing Front)**

Solange die Front-Menge nicht leer ist:

1. Entnahme einer Front-Kante *A → B*.
2. Iteration über die verknüpften Nachbarn in *G* auf der unverbundenen Halbebene der Kante.
3. Identifikation des optimalen Zielpunkts *C* (Apex):
   * *C* erfüllt die leere 3D-Umkreisbedingung bezüglich des lokalen Nachbarschaftsgraphen *G*.
   * *C* minimiert die Distanzfunktion *dist(A,C) + dist(B,C)*.
4. Erzeugung des neuen Dreiecks *(B, A, C)*. Diese Knotenreihenfolge garantiert die topologische Fortsetzung der Flächenorientierung.

**Phase D: Topologisches Merging**

* Der identifizierte Punkt *C* kann den Zustand "isoliert" besitzen oder bereits einem anderen Dreieck der Struktur angehören (etwa beim Aufeinandertreffen zweier Fronten). Der Algorithmus differenziert hierbei nicht; topologische Lücken werden durch Referenzierung derselben Vertex-ID automatisch geschlossen.
* Die initial entnommene Front-Kante *A → B* wird aus der Front-Menge gelöscht.
* Die neu erstellten Halbkanten *A → C* und *C → B* werden analysiert: Existiert ihr Gegenstück im System, wird die Kante als verbundene Kante registriert. Fehlt das Gegenstück, wird die Halbkante der Front-Menge hinzugefügt.

**Phase E: Terminierung**

* Das Verfahren terminiert deterministisch, sobald die Front-Menge den Zustand leer (`size == 0`) erreicht.
* Das Endergebnis ist ein komplett geschlossenes, überlappungsfreies und 2-mannigfaltiges (manifoldes) Dreiecksnetz der jeweiligen Objektkomponente.

**Garantie & Vorbedingungen (Invariante).** Geschlossenheit und Naht-Konsistenz des Ergebnisses sind garantiert, sofern:

1. die Fläche **orientierbar** ist — für SDF-Ränder stets erfüllt, die Orientierung folgt aus *∇f*;
2. *G* die lokalen (restringierten) **Delaunay-Kanten** enthält (Adjazenz-Registrierung, Abschnitt 1, Schritt 6);
3. die Abtastung die **lfs-Bedingung** erfüllt (*U ≤ reach*).

Degenerationen (ko-sphärische/ko-planare Punkte) werden durch den **symmetrischen Tiebreak** — die über beide Kantenrichtungen identische Minimierung von *dist(A,C) + dist(B,C)* — konsistent aufgelöst.