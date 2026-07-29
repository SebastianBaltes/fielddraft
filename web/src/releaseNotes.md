# FieldDraft — Release Notes

Die SSOT der In-App-Release-Notes. REINER TEXT, kein Code: ein Anführungszeichen,
ein Backslash oder ein Semikolon im Text kann hier nichts kaputt machen (das war
früher anders — siehe releaseNotes.ts).

Format, eine Zeile je Eintrag:

    ## <version>
    - <kind>(<scope>) [#ticket …]: <text>

kind = feat | fix | perf. scope und #tickets sind optional. Folgezeilen, die
eingerückt sind, gehören zum Text davor (Umbruch erlaubt).

## 0.48.0
- feat(kernel) #452: Eine Skizze, die auf einer FLÄCHE steht, wird jetzt von deren Rand eingezäunt. Eine einzelne Linie quer über die Fläche schließt damit zwei Regionen — bisher schloss sie gar keine, weil FieldDraft Regionen allein aus den gezeichneten Kurven baute, und ein Extrude darauf brach mit „kein geschlossenes Profil“ ab. Der Zaun kann nur SCHLIESSEN, nie SCHNEIDEN: was die eigenen Kurven schon umranden, bleibt Stück für Stück wie es war, und in das extrudierbare Profil der ganzen Skizze geht der Zaun nicht ein. Die Trägerfläche ist dabei die ganze ebene Umgebung unter der Skizze, nicht die eine Teilfläche, in die unsere Zerlegung sie zufällig schneidet.

## 0.47.4
- fix(kernel) #450: Ein Kanten-Fillet rundet jetzt auch dort, wo die beiden Flächen NICHT im rechten Winkel zusammenstoßen — bisher tat es an einer solchen Kante schweigend nichts: keine Fehlermeldung, keine Änderung an der Geometrie. Dasselbe gilt für die Fase. Gemessen am #416-Dokument, wo Onshape die 82,75-Grad-Keilspitze eines Extrudes mit 1,36 mm rundet und wir sie scharf stehen ließen.

## 0.47.3
- fix(onshape) #451: Eine Ellipse in einer Onshape-Skizze wird jetzt übersetzt. Bisher fiel sie stillschweigend weg — eine Skizze, deren einzige Kurve eine Ellipse ist, kam damit ohne jede geschlossene Region beim Kernel an, und jedes Feature darauf brach erst bei der Regenerierung ab („kein geschlossenes Profil“). Elliptische BÖGEN bleiben offen und werden weiterhin gemeldet statt zu einer ganzen Ellipse geschlossen.

## 0.47.2
- fix(kernel) #449: Eine Fase, die breiter ist als die Wand, auf der sie sitzt, wird nicht mehr als „konkave Innenkante“ abgelehnt. Sie geht weiterhin nicht — aber die Meldung sagt jetzt, woran es liegt: der Fräser-Mittelpunkt läge hinter beiden Flächen. Im #416-Dokument betraf das eine 5-mm-Fase auf einer 3-mm-Platte, deren Kante eine ganz gewöhnliche Außenkante ist.

## 0.47.1
- fix(onshape) #447: Eine Skizze, die auf einem Mate-Connector MITTEN AUF einer Fläche sitzt, landet jetzt in Onshapes Ebene. Bisher las der Import statt der Fläche eine ihrer Randkurven — der Connector stand dann 90 Grad auf, oder sein Ursprung rutschte an dieser Kante entlang (im #419-Dokument exakt 90,0 Grad, im #409-Dokument 168,96 mm).

## 0.47.0
- feat(onshape) #446: Onshapes Angabe, auf WELCHER Seite einer Kante die gewählte Region liegt, wird jetzt für jede Skizze gelesen — bisher nur dort, wo ein Abgleich mit Onshapes 3D-Antwort sie bestätigen konnte. Ein Profil kann damit nicht mehr die Bohrung erwischen, um die es gezäunt ist.
- fix(onshape) #446: Ein gespiegelt kopierter Vollkreis behält beim Import seine Umlaufrichtung. Vorher lag die Region an ihm auf der falschen Seite: die zwei Schlüsselloch-Bohrungen im #419-Dokument wählten das Material um sich herum statt ihr eigenes Inneres.
- fix(onshape) #446: Sitzt die aufgelöste Ebene einer Skizze entgegengesetzt zu Onshapes, sagt der Import das jetzt. Das ist immer eine Fläche zu viel oder zu wenig — etwa die Rückseite einer dünnen Schale.

## 0.46.0
- feat(kernel) #445: Wenn ein Extrude seine Wände nicht benennen kann, sind sie jetzt auch nicht mehr auffindbar. Vorher lieferte eine Wand-Auswahl auf so einem Feature stillschweigend die falsche Fläche — die x-te in Reihenfolge statt der gemeinten Kurve.
- fix(kernel) #445: Damit kann ein Mate-Connector oder ein Fillet, der auf einer solchen Wand hängt, nicht mehr unbemerkt an der falschen Kante landen. Er meldet lieber, dass er nichts findet.

## 0.45.0
- feat(kernel) #445: Ein Kreis oder Bogen, den der Rand eines mehrregionalen Profils ANSCHNEIDET, bleibt jetzt ein exakter Bogen. Bisher zerfiel er in rund fünfzig Sehnen — aus einer Zylinderfläche wurden fünfzig Ebenen.
- fix(kernel) #445: Damit behalten die Wände eines solchen Extrudes ihre Namen. Vorher fielen ALLE Wände des Features auf eine Nummerierung nach Reihenfolge zurück, und eine Kanten-Auswahl darauf traf eine beliebige andere Wand.
- perf(kernel) #445: Der Import des #419-Dokuments wird dadurch dreimal schneller (16,6 s auf 5,4 s im Kernel-Replay): echte Zylinderflächen sind für die Flächenverwebung weniger Arbeit als fünfzig Facetten.

## 0.44.0
- feat(onshape) #442: Ein Extrude, das eine Wand oder Deckfläche eines früheren Features ausformt, wird jetzt auch so importiert. Bisher fiel der Import ersatzweise auf die ganze Skizze zurück und baute daraus einen Block, der halbe Nachbarflächen verschluckte.
- fix(onshape) #442: Eine Region-Auswahl überlebt jetzt einzelne Kurven, die Onshape zwar nennt, die die Skizze aber nicht mehr enthält. Fehlt die Hälfte des Randes, bleibt es bei der Absage.
- fix(onshape) #442: Eine Skizze auf einer Körperfläche sitzt jetzt genau dort, wo der Kernel sie hinlegt. Web und Kernel hatten zwei verschiedene Flächen-Mittelpunkte, was den Import um bis zu 7,5 mm verschob.
- fix(onshape) #442: Ein Mate-Connector, der auf der Kante zwischen zwei Flächen sitzt, wird jetzt gefunden. Vorher fiel die Skizze darauf und alles daran Hängende ersatzlos aus.
- fix(onshape) #442: Zwei konzentrische Kreise auf einer Körperfläche landen jetzt an Onshapes Stelle, obwohl ihre Drehung gar nicht messbar ist. Sie ist bei einer Kreisschar auch gegenstandslos.

## 0.43.3
- fix(onshape) #444: Gespiegelte Bögen einer Onshape-Skizze landen jetzt dort, wo sie hingehören. Onshape speichert einen Bogen nicht als Winkel IN der Skizze, sondern als Winkel in seinem eigenen kleinen Koordinatensystem — und eine Spiegelung dreht dieses System um. FieldDraft las die Winkel bisher, als wären es Skizzenwinkel: die gespiegelten Kopien eines Bogens erschienen dadurch an ihrem eigenen Spiegelbild, oft mehrere Millimeter daneben. Sichtbar wurde das erst danach: die Linien, die tangential an so einen Bogen anschließen, trafen ihn nicht mehr, die Umrisse fielen auf, und die Flächen, die sie einschließen sollten, gab es nicht. Im großen Testdokument betraf das drei von vier gleichen Schlüsselloch-Aussparungen einer Platte — die Skizze hatte 14 statt 15 Bereiche, zwei davon 0,03 mm² schmale Splitter, und die Extrusion überzog die ganze Platte statt nur ihren Rahmen. Jetzt trifft Onshapes Auswahl von 14 Bereichen genau 14 unserer Flächen, das Aufdicken „Thicken 7“ findet seine beiden Wände wieder und importiert, und die Zahl der Meldungen fällt von 179 auf 177.

## 0.43.2
- fix(onshape) #443: Eine Skizze, deren Ebene Onshape als EINE Deckfläche führt, wird jetzt auch dann importiert, wenn dieselbe Stelle bei uns aus mehreren Flächen besteht. Onshape fasst gleich hohe Deckflächen eines Extrudes zu einer Fläche zusammen; FieldDraft verschmilzt bewusst nichts, also traf dieselbe Ebenen-Angabe bei uns vier Flächen — und der Import brach dort mit „nicht eindeutig“ ab. Für eine EBENE ist das gar keine Mehrdeutigkeit: liegen alle Treffer in derselben Ebene, ist die Ebene bestimmt. Gemessen an den vier Deckflächen des großen Testdokuments weichen die Normalen erst in der neunten Nachkommastelle voneinander ab und die Höhen um 3,4 Nanometer — Flächen, die WIRKLICH verschieden liegen, werden weiterhin sichtbar abgelehnt. Im großen Testdokument importieren dadurch neun Features mehr (zwei Skizzen, sechs Extrusionen, eine Boolean), die Zahl der Meldungen fällt von 190 auf 179 und die der Körper steigt von 12 auf 15.

## 0.43.1
- fix(kernel) #424: Eine Skizzenkurve darf jetzt MEHRERE Wände eines Extrudes benennen. Wählt man zwei getrennte Bereiche einer Skizze — etwa die beiden schmalen Leisten eines Rahmens —, dann fassen dieselben zwei langen Linien beide Leisten ein. FieldDraft benannte eine Wand bisher allein nach der Kurve, die sie erzeugt hat; taucht dieselbe Kurve zweimal auf, war die Benennung nicht mehr eindeutig und wurde für das ganze Feature verworfen — auch für die sechs Wände, deren Kurve eindeutig ist. Alles, was auf einer solchen Wand aufbaut (eine Skizze darauf, ein Mate-Connector an ihrer Kante, ein Aufdicken), fand dann nichts mehr. Ein Wandname trägt jetzt zusätzlich, die wievielte Wand dieser Kurve gemeint ist; die Reihenfolge kommt aus der Geometrie (Konturen nach Lage und Größe, jede beginnend an einem Knick), nicht aus der Reihenfolge der Berechnung. In den Onshape-Testdokumenten kehren dadurch eine Spiegelung samt ihrem Mate-Connector zurück, und zwei weitere Wand-Verweise lösen wieder auf.
- perf(kernel) #424: Beim Weben eines Körpers wurde eine Naht-Nachbesserung auch dann versucht, wenn der Bereich, in dem die Naht liegt, dünner ist als das Raster, mit dem er abgetastet wird — sie konnte dort nichts finden und hat es trotzdem minutenlang versucht. Bei einer 0,12 mm dünnen Schalen-Subtraktion in einem der Testdokumente waren das 209 Sekunden pro Neuberechnung, dreimal hintereinander. Solche Nachbesserungen unterbleiben jetzt; der Körper behält die Topologie, die ohnehin schon feststand. Das Testdokument importiert wieder in 37 statt 1151 Sekunden.

## 0.43.0
- feat(kernel) #429: FieldDraft kann jetzt Flächen benennen, die erst durch eine Boolean entstehen — und damit auch die Kanten daran. Onshape faltet zwei bündig aneinanderstoßende Wände zu EINER Fläche zusammen und zerschneidet umgekehrt eine Wand in Bruchstücke; FieldDraft tut beides nicht und stand beim Import deshalb vor Verweisen, für die es keinen Namen hatte. Neu ist ein einziges Wort im Query-Vokabular, „qUnion“ — die Vereinigung zweier Auswahlen —, und damit lässt sich Onshapes verschmolzene Fläche als Vereinigung unserer eigenen sagen; ein Bruchstück wird über seine Nachbarn eingegrenzt, und der Rand einer Verrundung ist der Schnitt der Verrundungsfläche mit der Fläche daneben. In den Testdokumenten lösen dadurch 124 statt 101 Kanten-Verweise auf, alle 13 Verweise auf den Rand einer früheren Verrundung statt keinem, und 28 von 35 verschmolzenen bzw. geteilten Flächen tragen einen Namen. Wichtig dabei: unvollständig übersetzt wird nichts — fehlt einem Verschmelzungspartner der Name, wird die ganze Fläche mit Begründung abgelehnt, statt eine ähnlich aussehende zu nehmen. Am großen Testdokument importiert deshalb noch nichts Neues; die Fehlermeldungen verschieben sich aber sichtbar von „dafür haben wir keinen Namen“ zu „das Feature dahinter fehlt noch“.

## 0.42.2
- fix(onshape) #440: Eine Extrusion, die in Onshape „bis zur Fläche“ läuft und dabei die Richtung umkehrt, landete beim Import auf der falschen Seite ihrer Skizzenebene. Der Importer misst den Abstand zur Zielfläche selbst — dieses Maß trägt die Richtung bereits im Vorzeichen —, drehte es danach aber noch einmal um, weil Onshape zusätzlich einen Richtungspfeil führt. Die Angabe stand damit zweimal in derselben Rechnung, und der Körper wuchs um seine volle Tiefe über die Skizzenebene hinaus statt bis zum Ziel. In einem der Testdokumente steht so ein Körper 15,85 mm neben seinem Platz; alles, was danach mit ihm verrechnet wird, geht ins Leere — eine Subtraktion und ein Aufdicken verändern dort seit jeher keine einzige Zahl. Jetzt ist die Messung die Antwort: sie gilt in FieldDrafts eigener Skizzen-Normalen und braucht Onshapes Pfeil nicht mehr. Der Körper deckt sich mit Onshapes eigenem Maß auf 0,01 mm, und die beiden nachfolgenden Features schneiden wieder.

## 0.42.1
- fix(onshape) #440: Der Onshape-Import übernimmt eine ausgewählte Skizzen-Region jetzt vollständig. Onshape und FieldDraft zerlegen dieselben Skizzen-Kurven nicht überall gleich fein; wo FieldDraft feiner schneidet, besteht eine Onshape-Region aus mehreren unserer Teilflächen. Bisher wurde davon genau eine übernommen — die bestbewertete — und der Rest fiel wortlos weg. In einem der Testdokumente riss das eine 6,15 mm hohe Lücke in ein Drehteil, und die Zylinderwand, auf der ein späteres Aufdicken sitzt, entstand nie. Neu wird die Rand-Beschreibung der Region vollständig gelesen: jede berandende Kante samt der Punkte, an denen sie beschnitten ist. Damit lassen sich Regionen unterscheiden, die von genau denselben Kurven berandet werden und sich nur im Beschnitt unterscheiden — ein Extrude, das in Onshape drei Streifen eines Rechtecks wählt, bekommt jetzt drei statt zwei. Und eine Region wächst über eine Kante hinweg nur dann auf mehrere Teilflächen, wenn dieselbe Onshape-Kante beide berandet; widerspricht die Rand-Beschreibung dem, bleibt es bei der einen Fläche und es gibt eine Warnung. Die Meldung über verlorene Regionen zählt nicht mehr Flächen gegen Auswahlen, sondern benennt, was fehlt — vorher schwieg sie genau in dem Fall, um den es hier geht. In einem zweiten Testdokument extrudierte der Import dadurch bisher eine 880×460 mm große Mittelplatte, die es im Original gar nicht gibt; jetzt sind es die beiden 20 mm schmalen Leisten, die Onshape wirklich wählt.

## 0.42.0
- feat(issues) #441: Der Issues-Tab stellt Ticket-Beschreibungen und Kommentare jetzt als richtiges Markdown dar — Überschriften, Listen, Tabellen und Code-Blöcke sehen so aus, wie Agenten sie schreiben, statt als Fließtext-Wand mit rohen Sternchen und Rauten. Ein als mermaid markierter Codeblock wird zum Diagramm gerendert; ist die Diagramm-Syntax kaputt, erscheint an der Stelle einfach der rohe Code statt eines Absturzes. „#N“-Ticketverweise bleiben klickbar wie bisher, jetzt direkt aus dem Markdown heraus statt über einen separaten Autolinker. Die gerenderte HTML-Ausgabe läuft vor der Anzeige durch DOMPurify — eingefügtes Skript oder ein onerror-Attribut in einem Kommentar landet nie ausführbar im DOM.

## 0.41.1
- fix(onshape) #430: Der Onshape-Import versteht jetzt jede Kanten- und Flächen-Abfrage, die ein Onshape-Dokument mitbringt. Onshape schreibt seine Abfragen stark verkürzt: was schon einmal dastand, kommt nur noch als Rückverweis „nimm Nummer 26“. FieldDraft las diese Nummern als Verweise auf frühere Zeichenketten — das stimmt am Anfang jeder Abfrage und läuft danach schleichend aus dem Tritt, weil Onshape in Wahrheit auf frühere Werte zeigt, also auch auf ganze Listen und Objekte. Gemessen an allen 697 Abfragen der Testdokumente: 41 davon endeten in Feldern, die es bei Onshape gar nicht gibt, und in 325 blieb mindestens ein Verweis offen; jetzt sind es null und null. Sichtbar wird das an Verrundungen und Fasen, die Onshape über den Schnitt zweier Flächen benennt: 99 statt 78 der 206 Kanten des Testkorpus lassen sich übersetzen, und die Ausrede „Fläche nicht dekodierbar“ verschwindet ganz. Ein Dokument bekommt dadurch seine Verrundung zurück, zwei Fehlermeldungen weniger; der Rest hängt eine Stufe später an der Flächen-Zuordnung im Kernel, nicht mehr am Lesen der Abfrage. Erfundene Kanten sind dabei weiterhin ausgeschlossen: was sich nicht sauber lesen lässt, wird gemeldet und nicht geraten.

## 0.41.0
- feat(kernel): Verrundungen und Fasen haben jetzt einen eigenen Namen. Bisher unterschied FieldDraft eine Verrundungsfläche nicht von einer gewöhnlichen Wand: die Abfrage nach der Verrundung eines Features lieferte nichts, und der Onshape-Import benutzte genau diese Abfrage bereits, um zwei sonst gleich aussehende Wände auseinanderzuhalten — sie lief still ins Leere. Gemessen an einem Würfel mit verrundeter Kante: die Fläche war die ganze Zeit da, sauber gewoben, mit exakt dem Radius und der Achse der Rollkugel — ihr fehlte nur die Zuordnung. Jetzt trägt sie die Rolle „Verrundung“, und dasselbe gilt für die Schrägfläche einer Fase. Nebenbei behoben: die beiden Hilfskörper, aus denen eine Verrundung geschnitten wird, teilten sich einen Namensraum, sodass die Verrundungsfläche eine Fläche des Hilfsquaders verdrängte — folgenlos, aber dieselbe Art Verwechslung, die früher zu erfundenen Verrundungen geführt hat. Weil sich dadurch die Kennungen von Verrundungs- und Fasenflächen ändern, finden bestehende Entwürfe diese Flächen über die eingebaute Heilung wieder.

## 0.40.0
- feat(kernel) #429: Muster-Kopien haben jetzt einen eigenen, mitwandernden Namen. Bisher hing der Name einer Kopie daran, wo sie im Raum steht: FieldDraft nummerierte gleich benannte Flächen nach ihrem Schwerpunkt durch. Drehte man die Musterrichtung um, tauschten die Kopien ihre Namen — gemessen an einem Dreier-Muster kehrte sich die Reihenfolge exakt um, und sogar das Original übernahm die Namen der letzten Kopie. Ein Feature, das auf einer Fläche des Originals saß, wäre danach still auf einer Kopie gelandet. Jetzt trägt jede Kopie eine Instanz-Nummer, die aus dem Modell kommt und nicht aus der Geometrie: Richtung umdrehen, Abstand ändern, eine Kopie anhängen — der Name bleibt derselbe. Dazu gibt es die neue Abfrage „qInstance(i, …)“, mit der sich gezielt „dasselbe, aber an der i-ten Kopie“ benennen lässt; eine Kopie, die es nicht gibt, liefert nichts, statt auf die nächstbeste zu zeigen. Weil dafür die interne Namens-Signatur breiter wurde, ändern sich sämtliche Element-Kennungen einmalig — bestehende Entwürfe finden ihre Flächen und Kanten über die eingebaute Heilung wieder, und sie werden beim nächsten Speichern neu geschrieben. Was noch aussteht: zwei verschiedene Muster über dieselbe Quelle teilen sich weiterhin einen Namen.

## 0.39.8
- fix(kernel) #438: Derselbe Entwurf liefert jetzt bei jedem Programmstart dieselbe Kanten-Topologie. Der analytische Kanten-Weber fasst Eckpunkte zusammen, die dichter beieinanderliegen als eine Toleranz, die sich nach der Größe des Bauteils richtet — auf der 3 m langen Baugruppen-Schiene sind das gut drei Millimeter. Lagen mehrere Eckpunkte in diesem Umkreis, nahm er denjenigen, den seine interne Tabelle zufällig zuerst nannte, und diese Reihenfolge ist bei jedem Start eine andere. Gemessen an einem Körper des großen Baugruppen-Dokuments: mal 101, mal 102 Kanten nach dem Verschmelzen, etwa im Verhältnis eins zu eins über die Läufe verteilt — und mit den Kanten wanderten die stabilen Namen der Elemente, an denen Features hängen. Jetzt gewinnt immer der nächstgelegene Eckpunkt, bei Gleichstand der mit der kleineren Nummer. An den Ergebnissen selbst ändert sich nichts, sie sind nur nicht mehr dem Zufall überlassen.

## 0.39.7
- fix(kernel) #437: Weitere fehlende Kanten auf sehr langen Bauteilen kommen zurück. Der analytische Kanten-Weber kennt die Enden jedes Nahtstücks schon, bevor er es verfolgt: es sind die Punkte, an denen drei Flächen zusammentreffen. Seit 0.39.6 tastet er zwischen diesen Enden nach, wenn das grobe Raster ein Nahtstück übersprungen hat. Er holte sich diese Enden aber aus einer Liste, die zuvor nach Position ausgedünnt worden war — und deren Radius sich nach der Größe des ganzen Bauteils richtet, auf der 3 m langen Baugruppen-Schiene also gut drei Millimeter beträgt. Zwei Ecken, die näher beieinander liegen, galten dort als eine; die zweite verschwand samt der Information, zu welcher Naht sie gehört. Genau das traf die kurzen Nahtstücke, um die es geht: gemessen an einem Fall liegen die beiden Enden 1,6 mm auseinander, das Nachtasten bekam nur eines von beiden und unterblieb. Jetzt führt der Weber die Enden zusätzlich je Naht, ungekürzt — die Eckpunkt-Liste selbst bleibt unverändert, denn dort ist das Zusammenfassen richtig. Außerdem laufen Raster-Proben und Nachtast-Punkte nicht mehr in zwei getrennten Durchgängen, sondern in einer Liste, sodass die vorhandene Dubletten-Erkennung greift; damit entfällt die Einschränkung, dass nur völlig leer ausgegangene Nähte nachgetastet wurden. Gemessen am großen Baugruppen-Dokument: sechs weitere Kanten auf der Schiene (darunter zwei 19-mm-Flanken auf beiden Platten), zwei auf einem zweiten Körper, und eine Fläche mehr mit durchgängiger Berandung. Wasserdicht ist die Schiene damit noch nicht: der größere Teil der offenen Flächen hat eine andere Ursache, die nun an einem Minimalbeispiel nachgestellt und als eigener Befund festgehalten ist.

## 0.39.6
- fix(kernel) #436: Auf sehr langen Bauteilen fehlten einzelne Kanten — und damit die geschlossene Berandung der Flächen, die sie begrenzen. Der analytische Kanten-Weber sucht jede Naht, indem er die Schnittlinie zweier Flächen an 64 gleichmäßig verteilten Stellen abtastet; verteilt werden diese 64 Proben über den ganzen Körper. Auf der 3 m langen Baugruppen-Schiene liegt damit alle 49 mm eine Probe, während die tatsächlich vorhandenen Kantenstücke dort 10 bis 39 mm lang sind: Ein Rippenflansch oder eine Nutwange fiel komplett zwischen zwei Proben, wurde nie verfolgt und fehlte anschließend. Jetzt merkt der Weber, wenn eine Naht gar keine Kante geliefert hat, obwohl er ihre Eckpunkte längst kennt — dann tastet er gezielt zwischen diesen Eckpunkten nach, statt weiter über sie hinwegzuschreiten. Nähte, die schon vorher vollständig waren, bleiben unverändert. Gemessen am großen Baugruppen-Dokument: sechs zuvor fehlende Kanten kommen zurück, drei Flächen bekommen ihre geschlossene Berandung. Der Befund ist damit nicht erledigt — auf demselben Körper bleiben 25 Flächen ohne durchgängige Berandung, aus zwei weiteren Ursachen, die eigene Tickets bekommen haben. Neu ist, dass diese Zahl jetzt gemessen und festgenagelt ist, statt nur als Warnung durchzulaufen.

## 0.39.5
- fix(kernel) #435: Wer dasselbe Profil einmal extrudiert und einmal rotiert, bekommt den Körper jetzt sauber und schnell. Beide Formen berühren sich dabei entlang der gemeinsamen Profillinie — die extrudierte Wand ist genau die Tangentialebene des Rotationskörpers, sie schneiden sich also nicht, sondern schmiegen sich aneinander. Der Kanten-Sucher hielt das für eine Naht, fand aber keine Kurve zum Verfolgen und warf daraufhin den GANZEN Körper auf den langsamen Gitter-Weg, der dort statt einer Kante 35 winzige Schleifen von fünf Hundertstelmillimeter Größe hinterließ. Jetzt wird die Berührung an ihrer Bauart erkannt, bevor gesucht wird: kein Gitter, keine Streuschleifen, gleiche Flächen. Gemessen am großen Baugruppen-Dokument: der betroffene Aufruf braucht 0,9 statt 3,7 Sekunden, und der Körper hat 74 statt 110 Kanten — die 36 fehlenden waren allesamt Ausschuss.

## 0.39.4
- perf(kernel) #433: Das große Baugruppen-Dokument aus Onshape importiert in gut einem Drittel der Zeit — gemessen 107 Sekunden vorher, 36 Sekunden nachher. Die Zeit lag nicht verteilt über die 150 Features, sondern in sechs einzelnen Aufrufen: sie machten 89 Prozent des Imports aus, und in jedem davon steckten neun Zehntel in derselben Rechnung. Der Kanten-Weber sucht die Eckpunkte eines Körpers, indem er von jeder gefundenen Naht aus nachrechnet, wo sich drei Flächen treffen — und startete diese Rechnung 48 Mal je Flächen-Tripel, obwohl drei ebene Flächen sich nur in genau einem Punkt treffen können. Auf einem Körper waren das drei Millionen Rechnungen für 23 Ecken. Jetzt hört die Suche auf, sobald sie nichts Neues mehr findet. Dieselben Ecken, dieselbe Geometrie, dieselben Körper — nur ohne die Wiederholung. Spürbar überall dort, wo viele Features auf einem Körper stehen, nicht nur beim Import.

## 0.39.3
- perf(kernel) #426: Eine Boolean gegen einen langen Körper bleibt nicht mehr minutenlang stehen. Wenn der analytische Kanten-Weber einzelne Nähte nicht auflösen kann, holte er sie bisher aus einem Gitter über den GANZEN Körper — auf einer 3 m langen Baugruppen-Schiene waren das 1,5 Millionen Zellen und 14 Minuten, um zwei Nähte von 0,17 mm und 0,017 mm Breite zu flicken, die Zellen dieser Größe ohnehin nicht auflösen können. Jetzt wird das Gitter genau dort aufgespannt, wo die offenen Nähte liegen, und in deren Maßstab: derselbe Aufruf braucht 5 Sekunden statt 14 Minuten, und die Nähte werden dabei zum ersten Mal fein genug abgetastet. Ausgelöst wurde das in einem Onshape-Import; sichtbar wird es überall dort, wo ein Verschneiden großer Körper bisher scheinbar hängen blieb.

## 0.39.2
- fix(kernel) #431: Die Kopien eines Musters bekommen endlich eigene Flächen-Namen. Bisher trugen alle Kopien dieselben Namen wie das Original — ein Quader mit dreifachem linearem Muster hatte 18 Flächen, aber nur 6 verschiedene Namen. Wer eine Fläche einer Kopie ansprach (eine Skizze darauf, eine Fase an ihrer Kante, ein Bohrungs-Ziel), traf deshalb irgendeine der drei — meist die des Originals, und zwar lautlos. Genau so ist im Onshape-Testkorpus einmal eine Verrundung an einer Kante gelandet, an der in Onshape gar keine sitzt. Jetzt wird der Unterscheidungs-Index über ALLE Körper hinweg vergeben statt je Körper: 18 Flächen, 18 Namen, und jeder Name trifft genau eine Fläche. Namen, die schon vorher eindeutig waren, bleiben unverändert — ein Dokument ohne Muster merkt nichts davon. Offen bleibt, dass dieser Name die LAGE der Kopie beschreibt: dreht man die Musterrichtung um, tauschen die Kopien ihre Namen.

## 0.39.1
- fix(sketch) #424: Werden MEHRERE nebeneinanderliegende Skizzen-Regionen zusammen extrudiert, entsteht daraus jetzt EIN Bereich mit EINER Außenkontur — die gemeinsame Trennlinie liegt im Inneren und ist keine Wand mehr. Bisher wurden die geschlossenen Umrisse der Regionen einfach übereinandergelegt; die geteilte Linie kam dadurch doppelt vor, und weil zwei Wände nicht denselben Namen tragen dürfen, verlor der ganze Körper seine Wandnamen und wurde wieder durchnumeriert. Alles, was auf so einer Wand aufbaut — eine Skizze darauf, eine Wandstärke, eine Fase — fand sie danach nicht mehr. Zusätzlich beginnt eine geschlossene Kontur jetzt immer an einem Knick, damit eine glatte Wand nicht willkürlich an der Naht in zwei zerfällt. Gemessen an den Testdokumenten: im großen Baugruppen-Dokument importieren elf Features mehr (zwei Booleans, acht Extrusionen, eine Bohrung), im Regal-Dokument die Spiegelung samt ihrem Mate-Connector.

## 0.39.0
- feat(onshape) #425: Fasen und Verrundungen aus Onshape erkennen jetzt auch die senkrechte Kante zwischen zwei Wänden — bisher kannte der Import nur den Rand einer Deckfläche. Ebenso wichtig: Kanten, für die FieldDraft keinen Namen hat (eine Kante, die aus einer Boolean-Verschmelzung stammt, die Kante einer Muster-Kopie oder die einer bereits verrundeten Fläche), werden jetzt beim Namen genannt und übersprungen, statt still auf eine ähnlich aussehende Kante gelegt zu werden. Im Testkorpus wurden dadurch zwei Verrundungen erfunden, die auf der falschen Kante saßen; drei neue entstehen an der richtigen. Gemessen: von 206 Kanten-Angaben der Testdokumente sind 78 übersetzbar, die übrigen 128 melden ihre Onshape-Form im Klartext.

## 0.38.5
- fix(kernel) #428: Eine Boolean-Operation mit „Werkzeuge behalten“ lässt die Werkzeug-Körper jetzt wirklich stehen. Bisher wurde das Häkchen zwar gespeichert, aber nie gelesen: die Werkzeuge wurden ausnahmslos verbraucht und verschwanden kommentarlos aus dem Dokument. Im importierten Regal-Dokument aus Onshape kostete das die vier senkrechten Pfosten — das Modell maß dadurch 887 mm Höhe statt 1840 mm. Jetzt trifft es Onshapes Bereich auf den Millimeter, und drei Skizzen samt einer Extrusion, die auf den Pfosten aufbauen, importieren gleich mit.

## 0.38.4
- fix(onshape) #427: Liegt eine Skizze in Onshape auf einer Körperfläche, die FieldDraft zwar parallel, aber um einen Betrag versetzt auflöst, werden die 2D-Koordinaten jetzt trotzdem in FieldDrafts Achsen gedreht — der Versatz wird beziffert gemeldet, statt die Drehung mit zu verwerfen. Bisher landeten Onshapes Zahlen dabei roh auf den falschen Achsen: im Regal-Dokument stand eine Ebene dadurch um 90 Grad verdreht und 1072 mm neben ihrem Platz, und alles, was darauf aufbaute, hing 75 mm unter dem Boden. Jetzt steht es an seinem Platz.

## 0.38.3
- fix(sketch) #424: Eine Fläche, die aus einer ausgewählten Skizzen-REGION entstanden ist, trägt jetzt denselben Namen wie eine aus der ganzen Skizze: den der Kante, die sie gezeichnet hat. Bisher wurden die Wände solcher Körper einfach durchnumeriert — jede Bezugnahme auf „diese Wand“, etwa ein Sketch darauf oder eine Fase daran, ging deshalb ins Leere, sobald das Profil eine Region war. Beim Onshape-Import waren davon ganze Skizzen samt allem, was darauf aufbaut, betroffen.

## 0.38.2
- fix(onshape) #409 #419: Eine Extrusion, die in Onshape nicht auf der Skizzenebene beginnt, sondern auf einer davon versetzten Ebene, wird jetzt gemeldet statt still an der falschen Stelle gebaut. Bisher fiel der Versatz unter den Tisch — der Körper stand dann um genau diesen Betrag daneben, ohne dass irgendwo etwas dazu stand. Im Regal-Dokument war das ein Zylinder 100 mm zu hoch, im Schienen-Dokument betrifft es sieben Extrusionen.

## 0.38.1
- fix(onshape) #419: Ein Sketch, der auf einer schlicht gezeichneten Linie aufsetzt, wird jetzt gefunden. Bisher erkannte der Import die Kante nur an ihrer zusammengesetzten Schreibweise; eine einfache Linie fiel durchs Raster und der Sketch galt als „Ebene nicht übersetzbar“ — obwohl Onshapes Angabe längst verstanden war.
- fix(onshape) #419: Sketches auf der Mantelfläche eines Rotationskörpers werden importiert. Bisher wurde eine Seitenwand nur akzeptiert, wenn sie aus einer Extrusion stammte; bei einem gedrehten Körper brach die Kette dort ab, samt allem, was darauf aufbaute.
- fix(onshape) #419: Ein Mate-Connector, der auf dem Schwerpunkt einer geraden Linie sitzt, landet jetzt auf deren Mitte statt zu scheitern. Auf einer runden Kante bleibt er bewusst abgelehnt: dort liegt der Schwerpunkt im Zentrum und damit neben der Kante.

## 0.38.0
- feat(onshape) #409 #416 #419: Onshape-Sketches, die auf einem Mate-Connector liegen, werden jetzt importiert statt übersprungen — samt Connectors auf einer rohen Konstruktionslinie und auf dem Ursprung des Part Studios. Das war der Grund, warum bei manchen Dokumenten schon der erste Sketch fehlte und alles Nachfolgende mit ihm. Ein Mate-Connector taugt damit auch als Spiegelebene.
- fix(onshape) #409: Der Versatz eines Mate-Connectors wird ausgerechnet statt aus Onshapes Rohwert gelesen. Steht dort eine Formel wie „Höhe minus 3 cm", meldet Onshape schlicht 0 — das Regal stand dadurch 1,8 Meter zu tief.
- feat(onshape) #409 #419: Linear-, Kreis- und Spiegelmuster haben eigene Tests bekommen; Abstand und Anzahl kommen aus Onshapes Formel, nicht aus dem Rohwert.

## 0.37.2
- fix(kernel) #409 #419: Spiegeln spiegelt jetzt wirklich. Bisher wurde nur der Ursprung des Quell-Features an der Ebene gespiegelt, das Bauteil selbst aber nicht gedreht — eine Winkelplatte zeigte danach in dieselbe Richtung wie das Original, und ein aus einem Sketch gebauter Körper landete deckungsgleich auf seinem Original. Der Onshape-Import musste jede Spiegelung deshalb verweigern; das entfällt.

## 0.37.1
- perf(kernel) #423: Körper aus Kreissektor-Profilen (ein Bogen, der keine Kontur schließt — im Onshape-Import der Normalfall) werden wieder analytisch vermascht statt über den Octree. Der Deckel der Kabelbox brauchte dafür 5,9 Sekunden und lieferte eine entartete Kantenmenge; jetzt fällt beides weg. Nebenbei: das Seed-Bucketing im Weber macht aus 3,4 Sekunden reiner Buchhaltung einen Bruchteil davon.

## 0.37.0
- feat(onshape) #415 #414: Sketches auf Körperflächen landen nicht mehr um 90° verdreht — der Import misst Onshapes tatsächlichen Sketch-Frame an dessen eigener 3D-Antwort und rechnet die 2D-Koordinaten exakt in FieldDrafts Ebenen-Konvention um. Das Referenzteil aus #415 trifft jetzt Onshapes Bounding-Box exakt; die grobe Y-Abweichung in #414 ist damit ebenfalls weg.

## 0.36.0
- feat(onshape) #416 #417 #421: Onshape-Mate-Connectors (explizite und die impliziten, die ein Sketch als Unter-Feature mitbringt) werden als FieldDraft-Mate-Connectors importiert — inklusive Versatz, Flip und 90°-Ausrichtung; andere Winkel werden benannt gemeldet statt stillschweigend gerundet. Sketches AUF einem Mate-Connector brauchen noch zwei kleine Kernel-Erweiterungen und werden bis dahin benannt übersprungen.

## 0.35.0
- feat(onshape) #415: Onshape-Thicken (Fläche aufdicken) wird übersetzt — Wände landen exakt in der von Onshape gemessenen Stärke (±48,5 mm statt vorher ±46,5 mm im Referenzteil). Flächen-Verweise werden strikt aufgelöst: bindet eine Query auf null oder mehrere Flächen, wird sie benannt verworfen statt geraten.

## 0.34.0
- feat(onshape) #409 #415 #417: Onshape-Variablen und Formeln werden jetzt echt ausgewertet — assignVariable wird zu Dokument-Variablen, und Maß-Ausdrücke wie „3 mm+#HoleExtraGap" oder „(2·sqrt(#TelescopeWeight)/sqrt(3·pi)) cm" rechnen einheiten-bewusst (128 von 140 Korpus-Formeln korrekt). Vorher fielen solche Maße stillschweigend auf Vorgabewerte — die Geometrie sah fehlerfrei aus und war falsch. Nicht auswertbare Ausdrücke werden jetzt laut gemeldet, mit dem ersatzweise verwendeten Wert.
- feat(onshape) #415: Onshape-Boolean-Features (Vereinigen/Abziehen/Schneiden) werden übersetzt; nicht auflösbare Operanden werden benannt übersprungen statt geraten.

## 0.33.1
- fix(ui): Escape beendet jetzt auch den Fasen-, Fillet- und Mess-Modus (vorher blieb das Werkzeug aktiv und fing weiter Klicks ab).

## 0.33.0
- feat(onshape) #416: Onshape-Revolve-Features werden übersetzt — volle Umdrehung um eine Sketch-Linie (auch Konstruktionslinie), Profil-Regionen und Boolean-Scope wie beim Extrude. Teil-Umdrehungen, Flächen-Revolves und Nicht-Linien-Achsen werden mit klarer Meldung ausgelassen statt geraten.
- fix(onshape) #416: Sketches auf impliziten Mate-Connectors melden jetzt das fehlende Onshape-Konzept beim Namen statt einer kryptischen Query-Fehlermeldung.

## 0.32.0
- feat(ui) #414: Fasen-Werkzeug in der Toolbar — Kante anklicken erzeugt eine symmetrische Fase (chamfer_edge), gleicher Workflow wie beim Fillet; Maß im Feature-Dialog änderbar.
- feat(onshape) #414: Der Import übersetzt jetzt chamfer (EQUAL_OFFSETS exakt, OFFSET_ANGLE bei 45° mit Hinweis) und hole (Durchmesser wörtlich aus dem Dokument, Sackloch/durchgehend, mehrere Bohrpunkte). Nicht Unterstütztes (ungleiche Schenkel, Counterbore/Countersink, Bohrspitze, up-to-next) wird namentlich gemeldet statt still genähert.
- fix(onshape) #414: CAP_EDGE-Kanten-Queries (Onshape benennt den Kantenzug direkt) werden jetzt aufgelöst, und die Kappen-Seite folgt dem Extrude-Vorzeichen — schließt auch einen Teil der Fillet-Reste aus #410/#413.

## 0.31.2
- fix(kernel) #412: Dünne Bretter in großen Körpern rendern jetzt im Viewport — das Mesh-Gitter verfeinert sich automatisch, bis das dünnste Feature erfasst ist. Bisher leitete sich die Zellgröße allein aus der längsten Achse ab: ein 4-m-Regal aus 22-mm-Brettern wurde bei App-Auflösung komplett übersprungen und blieb ohne jede Fehlermeldung unsichtbar.

## 0.31.1
- fix(onshape) #412: Region-Auswahl wählt keine Phantom-Flächen mehr — eine Onshape-Query, die eine im Sketch nicht (mehr) existierende Kurve benennt, wird ehrlich verworfen (Warnung) statt durch eine Nachbar-Kurve ersetzt; beim Garagenregal wurde so ein 604×1500-mm-Block extrudiert, den Onshape nie gebaut hat. Auflösung jetzt über Boundary-Coverage-Scoring statt „kleinste angrenzende Fläche“; Teil-Auflösung bleibt erhalten (3 von 4 Platten statt Rückfall auf den ganzen Sketch).
- fix(onshape): Ein Extrude über den ungeteilten Gesamt-Sketch schreibt wieder qCreatedBy statt qRegion — die Regions-Nummerierung hatte die Wand-Referenzen späterer Features verschoben (Kabelbox: 3 Sketches übersprungen).

## 0.31.0
- feat(kernel) #414: Kanten-Fasen (Chamfer) als Kernel-Feature — chamfer_edge schneidet planare 45°-Fasen (EQUAL_OFFSETS) an geraden rechtwinkligen Kanten, wirkt in Rendering, Mesh, Picking und Voxel/Export und überlebt Speichern/Laden. UI-Werkzeug und Onshape-Übersetzung folgen.
- fix(kernel): Der Konvexitätstest der analytischen Kanten-Cutter konnte konvexe und konkave Kanten nicht unterscheiden — ein Fillet auf einer Innenkante hätte dort Material herausgeschnitten. Kanten werden jetzt über die gemischten Quadranten klassifiziert; konkave Kanten werden ehrlich abgelehnt.

## 0.30.0
- feat(kernel) #412: Sketch-Regionen folgen jetzt einem echten Imprint-Arrangement — sich schneidende oder verschachtelte Profile zerfallen in atomare Teilflächen, die einzeln wählbar und extrudierbar sind. Der Onshape-Import löst Regions-Auswahlen damit 1:1 auf: vier gewählte Platten ergeben vier Platten statt eines massiven Blocks, ohne Warnung.
- perf(onshape) #410: Der Import vermascht nicht mehr nach jedem Feature, sondern einmal am Ende — zusammen mit dem Weave-Cache aus 0.29.1 sinkt der Kabelbox-Import im Browser von mehreren Minuten Freeze auf gut 20 Sekunden.

## 0.29.1
- perf(kernel) #410: Beim Onshape-Import (und jeder Regeneration) wird jeder Körper nur noch einmal gewoben — ein persistenter per-Body-BRep-Cache ersetzt den bisherigen Cache-Schlüssel über die ganze Feature-Historie, der bei jedem neuen Feature sämtliche Körper von Grund auf neu webte (O(n²)).

## 0.29.0
- feat(onshape) #412: Der Onshape-Import übernimmt jetzt die im Extrude tatsächlich gewählten Sketch-Regionen — die komprimierten Regions-Queries werden dekodiert und auf FieldDraft-Regionen aufgelöst, statt stets den ganzen Sketch zu extrudieren. Kann FieldDrafts Regionenmodell eine Onshape-Auswahl nicht abbilden (Onshape trennt überlappende Profile per Imprint, FieldDraft verschmilzt sie), warnt der Import sichtbar („Onshape wählte N Regionen, FieldDraft findet dafür nur M“) statt still falsche Geometrie zu liefern.

## 0.28.1
- fix(ui) #422: Beim Öffnen eines Projekts ohne gespeicherte Ansicht (etwa direkt nach einem Onshape-Import) startet die Kamera jetzt als Iso-Ansicht, die das Modell formatfüllend rahmt — vorher erbte das neue Projekt still die Kameralage des zuvor geöffneten Projekts, die per Autosave sogar dauerhaft hineingeschrieben wurde, und wirkte danach „leer“. Zoom-to-fit holt die Kamera zudem verlässlich aus jeder Extremlage zurück.

## 0.28.0
- feat(kernel) #404: Bohrungs-Feature nach Onshape-Vorlage, Phase 1 — Bohrungen auf Sketch-Punkte und Mate Connectors setzen, mit ISO-Normgrößen (M2–M12, tap/close/free), freiem Durchmesser, Sackloch oder „durch alles“, und mehreren Bohrpunkten in einem Feature. Der Hole-Dialog öffnet direkt aus der Toolbar.

## 0.27.2
- fix(sdf) #405: „Round All“ wirkt jetzt wirklich — der Radius aus dem Dialog wurde seit Einführung des Features nie geparst, das Feature blieb dadurch still folgenlos. Jetzt rundet es alle harten Boolean-Kanten des Dokuments (Radius 0 = aus).

## 0.27.1
- fix(ui) #361: Das Admin-Modell-Dropdown im Assistent-Dialog sitzt jetzt in einer eigenen Zeile unter Titel und Schließen-Button statt eingequetscht daneben.

## 0.27.0
- feat(sdf) #401 #402: Thicken nach Onshape-Vorbild ausgebaut — ein Klick aufs Thicken-Tool öffnet jetzt direkt den Details-Dialog (Abbrechen verwirft das Feature wieder), und Thicken wirkt endlich auch auf planare Flächen: dort entsteht wie bei Extrude eine prismatische Aufdickung mit scharfen Kanten statt gar nichts. Neue Dialog-Parameter nach Onshape: Richtung umkehren, Merge scope und „Merge with all“.

## 0.26.0
- feat(sdf) #406 #383: Kanten-Fillets sind jetzt echte harte Cutter nach Onshape-Vorbild — an einer geraden konvexen Kante wird ein analytisch exakter Rundungskörper boolesch subtrahiert (zylindrische Rundung entlang der Kante, Ecken bleiben scharf). Damit funktioniert das Abrunden einer einzelnen Würfelkante überhaupt erstmals, und Fillets/Round All greifen jetzt sofort statt erst nach einer späteren Geometrie-Änderung.

## 0.25.1
- fix(ui) #400: Bricht man den Details-Dialog eines gerade per Werkzeug neu angelegten Features mit Abbrechen (oder Escape) ab, wird das Feature jetzt wieder entfernt — bestehende Features aus der Feature-Liste bleiben beim Abbrechen wie bisher erhalten.

## 0.25.0
- feat(sketch) #385 #376: Loft akzeptiert jetzt auch Profile auf einander zugewandten Ebenen (entgegengesetzte Normalen) — nötig u. a. für importierte Onshape-Lofts.

## 0.24.0
- feat(onshape) #385 #376: Onshape-Import übersetzt Loft-Features jetzt in echte FieldDraft-Lofts (Profil-Querschnitte aus sheetProfilesArray); Führungskurven werden in v1 noch ignoriert.

## 0.23.0
- feat(sketch) #385: Neues Loft-Feature — zwei oder mehr parallele Profil-Skizzen (oder planare Flächen) werden zu einem Körper überblendet; die Profile wählst du wie beim Extrude über den Profil-Picker im Dialog aus.
- fix(kernel) #385: Flächen-Auswahlen unterscheiden jetzt zuverlässig zwischen geometrisch identischen Körpern an verschiedenen Positionen — vorher konnten zwei Picks auf die Deckel zweier gleicher Boxen auf dieselbe Fläche zusammenfallen (etwa beim Loft: Fehlermeldung, die Profile lägen auf derselben Ebene).

## 0.22.0
- feat(sdf) #398 #384: Bogen-Kontakte sind jetzt vollwertig: Kontaktflächen mit runden Rändern (z. B. ein aufgesetztes Pillen- oder abgerundetes Profil) bekommen exakte Bogen-Korrekturkörper und einen Rand-Fillet, der gerade und runde Kanten in einem Zug verrundet. Nur nach innen gewölbte Bögen und Bohrungs-Ränder bleiben — wie einspringende Ecken — bewusst ungerundet und werden ehrlich mit Schranke gemeldet; diese Schranke erscheint jetzt auch dann, wenn an derselben Naht ein anderer Kontakt sauber schließt (vorher zu optimistisch).

## 0.21.0
- feat(sdf) #398 #384: Der Rand-Fillet an koplanaren Kontaktflächen greift jetzt auch bei Nicht-Box-Kontakten: gerade Kontaktrand-Kanten beliebiger (auch nicht-konvexer) Konturen werden sauber verrundet, jede Kontaktinsel bekommt ihren eigenen Saum. An einspringenden Ecken und Bogen-Kanten bleibt bewusst ein kleiner, ehrlich gemeldeter Rest stehen — die Genauigkeits-Angabe nennt dessen Schranke jetzt explizit, statt pauschal zu warnen.

## 0.20.0
- feat(sdf) #398 #384: Offset/Shell korrigieren koplanare Kontaktflächen jetzt auch zwischen Nicht-Box-Körpern: Bei bündig gedockten Extrudes (auch mit nicht-konvexen Profilen wie L-Formen) wird der Kontakt exakt verschnitten und pro Kontaktinsel ein eigener Korrekturkörper eingesetzt — der bisherige „Phantom-Schlitz“ an solchen Nahtstellen verschwindet, und die Genauigkeits-Angabe steigt von „konservativ“ auf „toleranz-beschränkt“. Kontakte mit Bogen-Konturen werden weiterhin ehrlich als Näherung gemeldet (folgt in einer späteren Phase).

## 0.19.2
- fix(sdf) #399: Ein Offset auf einen Body, der per Add-Docking aus mehreren Features besteht, erodierte bisher nur das im Ziel-Query genannte Feature — die angedockten Member blieben in Originalgröße stehen (Loch im Verbindungsbereich), und die Genauigkeits-Angabe behauptete fälschlich „exakt“. Der Offset erfasst jetzt immer den ganzen Body samt aller Member (wie es beim Abzieh-Pfad schon immer der Fall war), läuft regulär durch die Korrektur-Pipeline (Kontakt-Prisma + Rand-Fillet) und meldet die Genauigkeit ehrlich.

## 0.19.1
- fix(sdf) #392: Skizzen-Pfade aus mehreren Bögen desselben Kreises (z. B. ein aus zwei Halbkreisen gezeichneter Vollkreis) konnten exakt auf der Naht-Symmetrielinie sporadisch innen/außen verwechseln — sichtbar als vereinzelte Pixel-Artefakte auf Symmetrieebenen und als potenzieller Störer für Mesher-Seeds. Aufeinanderfolgende Bögen desselben Kreises werden jetzt beim Auswerten und beim GPU/Mesher-Lowering zu einem Bogen verschmolzen, sodass die fragile Naht gar nicht mehr existiert; alle Evaluatoren klassifizieren dieselbe normalisierte Kontur.

## 0.19.0
- feat(sdf) #398 #384: Rand-Fillets an koplanaren Kontaktflächen sind scharfgeschaltet: Wo bisher nach der Kontakt-Prisma-Korrektur ein δ-schmaler Sliver (Erosion) bzw. Keil (Dilation) am Kontaktrand stehen blieb (Restfehler bis ≈0,75·δ), rundet der neue OP_RIM_FILLET-Opcode diesen Rand jetzt metrisch exakt auf Radius δ — der Restfehler sinkt auf die Bau-Toleranz. Wirkt in allen Evaluatoren gleich (CPU, GPU-Renderer, Meshing, Ableitungen); die Capability-Meldung im Offset/Shell-Feature weist den verbesserten Rest ehrlich aus. Versetzte („staggered“) Kontakte, deren Randkanten konvex sind, werden bewusst nicht gefillett — dort bleibt die bisherige ehrliche Zusage bestehen, statt Material fälschlich aufzufüllen.

## 0.18.5
- fix(sdf) #397 #384: Die Ehrlichkeits-Erkennung für koplanare Kontakte am Dilations-Stopp ist jetzt vorzeichen-bewusst: Eine bündig ausgeschnittene Tasche aus Nicht-Box-Körpern (z. B. Extrude minus Extrude, flush cut) blieb bisher ganz ohne Warnung — der Phantom-Grat über dem Taschenmund lag unter einer zu guten Zusage. Der Kernel prüft nun zusätzlich die effektiven (Komplement-)Normalen und stuft solche Kontakte ehrlich auf „Näherung“ herab. Dünnwandige Shells sind über eine enge Koinzidenz-Toleranz sauber abgegrenzt und behalten ihre bisherige (bessere) Bewertung; Geometrie und Felder ändern sich nicht — es geht rein um die ehrliche Statusmeldung.

## 0.18.4
- fix(sdf) #395 #384: Das Kontakt-Prisma aus 0.18.3 korrigiert jetzt auch den Dilations-Fall (Komplement-Dualität): Bei positivem Offset oder Shell über einer Difference/Intersection mit koplanarer Kontaktfläche — der Klassiker ist die bündig ausgeschnittene Tasche (flush cut) — legte das naive Feld einen Phantom-Grat über den Taschenmund (bis über 6 mm Abweichung, teils unter falscher Zusage). Der Kernel hängt das Prisma dort als Schnitt-Constraint vor das +δ; das Prisma entfernt beweisbar nirgends echtes Material. Der Status ist „toleranzbeschränkt“ mit Restschranke ≈0,75·δ am Kontaktrand. Nicht exakt fassbare Kontakte (gekrümmt, Nicht-Box, tangential berührend) melden weiterhin ehrlich „Näherung“ — diese Meldung war beim Umbau kurz entfallen und ist wiederhergestellt.

## 0.18.3
- fix(sdf) #395 #384: Der in 0.18.2 nur ehrlich gemeldete Fehler an bündig gestapelten Körpern ist jetzt geometrisch behoben: Bei Erosion (negatives Offset, Shell) über einer Union mit koplanarer Kontaktfläche baut der Kernel ein Kontakt-Prisma über dem exakten Kontaktpolygon und hängt es als gewöhnlichen Union-Member vor das Offset — der flächige Phantom-Schlitz auf der submergierten Fläche (bisher bis über 8 mm Abweichung) verschwindet. Bei gleichem Querschnitt ist das Ergebnis metrisch exakt, sonst „toleranzbeschränkt“ mit bewiesener Restschranke ≈0,75·δ am Kontaktrand (der schmale Randsaum; Stufe 2 schließt ihn später). Das Prisma füllt beweisbar nie über die Union hinaus. Nicht exakt fassbare Kontakte (gekrümmt, Nicht-Box) bleiben ehrlich „Näherung“ wie bisher.

## 0.18.2
- fix(sdf) #394 #384: Der Maßhaltigkeits-Status von Offset/Shell log bei bündig gestapelten Körpern. Wenn zwei Körper einer Union sich koplanar oder tangential berühren (in CAD der Normalfall: flächig aufeinandergesetzte Teile), fand die Nahtsuche an der gemeinsamen Fläche keine Kurve — der Offset war dort nur eine Näherung (Abweichung bis über 13 mm), das Feature meldete aber weiterhin „exakt“ oder „toleranzbeschränkt“. Jetzt erkennt der Kernel den flächigen internen Kontakt strukturell und stuft ehrlich auf „Näherung“ herab; das Abzeichen und der Hinweis im Editor erscheinen. Saubere, quer verlaufende Nähte und getrennte Körper bleiben unverändert exakt bewertet. Die Geometrie selbst ändert sich nicht — es geht rein um die ehrliche Statusmeldung.

## 0.18.1
- fix(sdf) #384: Ein negatives Offset (Erosion) auf einem Körper mit geschlossener Shell ergab bisher an der Innenwand nur eine Näherung — die Höhle wurde beim internen Umbau nicht mit eingefaltet, die Wandstärke stimmte nach dem Offset nur ungefähr. Jetzt kommutiert das Offset korrekt durch die Innenwand-Struktur, die Höhle faltet exakt mit und die Wand bleibt maßhaltig. Aufgedeckt durch den neuen Property-Fuzz-Harness, der zufällige degenerierte Offset/Shell-Geometrien gegen die gemessene Ground Truth prüft.

## 0.18.0
- feat(sdf) #384: Offset- und Shell-Features zeigen jetzt an, wie maßhaltig ihr Ergebnis ist. Bisher blieb überall dort, wo die exakte Baum-Expansion an einer Stelle nicht möglich war, still eine Näherung („f−δ“) stehen — man erfuhr nicht, dass das Ergebnis dort nur ungefähr stimmt. Neu trägt jedes Offset/Shell-Feature einen Status: „exakt“ (vollständig analytisch gefaltet, keine Korrektur nötig), „toleranzbeschränkt“ (an Boolean-Nähten über Arc-Spline-Kanäle auf eine bekannte Toleranz genähert), „Näherung“ (irgendwo blieb f−δ stehen — die Nullmenge kann lokal abweichen, der Fehler ist nicht beschränkt) oder „nicht unterstützt“ (Twist/Bend im Scope, wird weiterhin abgelehnt). In der Baumansicht erscheint bei nicht-exakten Features ein Abzeichen mit Tooltip, im Feature-Editor ein Hinweisbanner mit den konkreten Gründen (z. B. „Profil enthält ein Bézier-Segment“, „Revolve-Profil berührt die Rotationsachse“ oder „Offset über einer Kanten-Fillet-Fläche“). Das Feld selbst bleibt unverändert — die Statuserhebung ist rein additiv.

## 0.17.3
- perf(sdf) #384: offset_tree Phase D2 — geschlossene Shell-Wände sind jetzt maßhaltig. Bisher wurde die Innenwand einer Shell über ein Näherungsfeld gelegt, das im Wandinneren (besonders an Ecken und Kehlen) um bis zu ~0,29·Wandstärke daneben lag; die Wand konnte dort dünner oder dicker wirken als eingestellt. Die Shell wird intern nun exakt als „Körper minus nach innen versetzter Körper" aufgebaut und erbt damit den präzisen Offset-Pfad — die Wandstärke stimmt bis auf Bruchteile eines Millimeters. Offene Shells (mit entfernten Flächen) bleiben unverändert. Zusätzlich: eine Shell auf einem mit Twist/Bend verdrehten Körper wird jetzt mit klarer Meldung abgelehnt, statt still eine falsche Wand zu erzeugen.

## 0.17.2
- fix(sdf) #384: offset_tree Phase D — das Offset-Feature ließ sich in der Baumansicht nicht bearbeiten: Es zeigte ein „?"-Symbol und beim Anwählen „No feature selected" statt der Parameter, sodass sich weder Ziel-Körper noch Betrag setzen ließen. Der Kernel meldete den Feature-Typ intern als „unbekannt"; jetzt wird „offset" korrekt aufgelöst, das Symbol erscheint und der Editor mit Körper-Auswahl und Betrag öffnet sich.

## 0.17.1
- fix(sdf) #384: offset_tree Phase D — ein Offset auf einem verdrehten oder gebogenen Körper (Twist/Bend) meldet jetzt einen klaren Feature-Fehler, statt still ein ungenaues Feld zu erzeugen. Eine Verdrehung ist keine Isometrie und hat keinen exakten Offset; wer sie offsetten will, entfernt sie oder setzt den Offset davor.

## 0.17.0
- feat(sdf) #384: offset_tree Phase D — neues Feature „Offset“. Wähle einen oder mehrere Körper und einen signierten Betrag; positiv wächst die Oberfläche nach außen (Dilation), negativ nach innen (Erosion). Der Offset ist kein Näherungsfeld mehr, sondern wird zur Bauzeit exakt in den Körper einexpandiert — dieselbe Maschinerie aus den Phasen A–C: der Betrag wandert an die Blätter, Erosionen falten sich exakt ein, und an den Boolean-Nähten korrigieren Kanalkörper die Distanz. Der Scope ist Pflicht (wie bei Remove/Intersect): ohne gewählten Zielkörper bleibt das Feature mit einem Hinweis leer, statt still nichts zu tun. Betrag als Millimeter-Eingabe im Dialog. (Shell nutzt diese Expansion noch nicht — folgt.)

## 0.16.8
- perf(sdf) #384: offset_tree Phase C — Topologie, Pruning und Weave-Refresh für die Offset-Expansion. Erodiert ein Offset ein Teil vollständig weg (Radius, Höhe oder 2D-Profil unter null), verschwindet es jetzt exakt statt Geister-Reste oder verfrühtes Löschen zu riskieren; eine Shell mit Wandstärke über der lokalen Materialdicke schließt sich korrekt. An Mehrkörper-Ecken (Triple-Junctions) schließen neue Rossignac-Requicha-Vertex-Kugeln den letzten Nullmengen-Fehler: an einer konkaven 90°-Raumecke sinkt der Eckenfehler von 1,38 mm (naiv) über 0,72 mm (nur Kanten-Kanäle) auf unter 0,03 mm bei δ=2. Dabei wurden drei latente Fehler der Phase-B-Kanäle gefunden und in allen fünf Auswertern behoben: die Naht-Verfolgung fiel bei achsparallelen Flächen still aus, die Kanal-Endkappen leckten bis zu δ Material über Ecken hinaus, und verkettete Kanäle konnten an Ecken Vorzeichen kippen. Picking sieht jetzt dasselbe korrigierte Feld wie der Renderer (Kanal-Flächen sind pickbar, Ray-Tiefe stimmt an Nähten), der B-Rep-Weave webt den expandierten Körper (Fillets auf offsetteten Kanten treffen echte Kanten), tote Korrekturkörper unter Nachbar-Material werden beweisbar feld-neutral entfernt, und die Toleranz-Staffel trennt Render- (0,01 mm) von Export-Qualität (0,001 mm).

## 0.16.7
- perf(sdf) #384: offset_tree Phase B — Kanal-Korrekturkörper an den dualen Offset-Stopps. Wo ein Offset nicht exakt durch die Booleans verteilt werden kann (Dilation über Schnitt/Differenz, Erosion über Vereinigung), wird das verbleibende „f − δ“ jetzt entlang der Schnittkanten korrigiert: der konvexe Überschuss-Keil (bis 0,414·δ) wird subtrahiert, der konkave Sliver fehlenden Materials (bis δ an spitzen Kehlen — eine 2-mm-Shell-Innenwand an einer 90°-Naht war lokal 0,8 mm zu dick) wird addiert. Die Naht-Kurven werden zur Regen-Zeit als Arc-Splines mit Normalkegel-Gating synthetisiert und in allen Auswertern gleich gerechnet (CPU, GPU-Raymarcher in beiden SDF-Modi, Mesher, CGAL-Server, Interval-DC-Export). Gemessen an der Steinmetz-Kante: Oberflächenfehler 0,002 mm mit 54 Bögen; wo keine Naht-Synthese möglich ist (Torus-/Gyroid-/Sweep-Nähte, tangentiale Nähte), bleibt ehrlich das bisherige Verhalten stehen. Nebenbefund des Pflicht-Golden-Checks: der CGAL-Server rechnete Fillets noch mit den alten Blend-Formeln von vor der 0.16.2-Umstellung — nachgezogen, alle 28 Golden-Szenen wieder bit-nah identisch.

## 0.16.6
- perf(sdf) #384: offset_tree Phase A — Erosions-Offsets werden jetzt analytisch in die Blätter eingefaltet. Ein nach innen gerichteter Offset auf Kugel, Zylinder, Box, Torus oder Kegel(-stumpf) wird zur Regen-Zeit durch dasselbe Primitiv mit geschrumpften Parametern ersetzt; bei Extrudes und achsenfernen Revolves wird das 2D-Profil geometrisch offsettiert (Profil-Booleans und Skizzen-Offsets werden dafür exakt aufgelöst, inklusive Wand-Identitäten). Das Feld ist damit wieder ein exaktes Distanzfeld statt „f − δ“: gemessene Außenfeld-Fehler bis 6,5 mm (Differenz-Profil-Fan) und Innenfehler bis 0,4 mm (Offset-Profil-Ridge) fallen auf Mess-Rauschen, und beim erodierten Vereinigungs-Profil sitzt die Oberfläche am Hals jetzt geometrisch richtig (vorher bis 4 mm daneben). Wo kein exakter Umbau existiert (Bézier-Profile, achsberührende Revolves, Verjüngung unter null), bleibt ehrlich das bisherige Verhalten stehen. B-Rep-Kanten, Feature-Snap und Face-Identitäten folgen der eingefalteten Fläche — der Weave-Baupfad fährt jetzt dieselbe Offset-Normalisierung wie Mesher und Renderer.

## 0.16.5
- fix(sdf) #390: Achsberührendes Revolve erzeugt keine Phantom-Oberfläche mehr auf der Rotationsachse. Profilkanten, die auf der Achse liegen (etwa die Schließkante eines Zylinder- oder Kugel-Halbprofils), sind nach der Rotation Körperinneres — das Distanzfeld behandelte sie aber weiterhin als Wand: Sphere-Tracing konnte eine unsichtbare „Nadel“ entlang der Achse treffen, und innen auf der Achse las das Feld 0 statt der echten Tiefe. Die Achs-Kanten werden jetzt für alle Profilformen (Rechteck, Polygon, Kreissektor, Pfad) und auch in booleschen Profilen (z. B. Bohrung bis zur Achse) entfernt; achsenferne Revolves bleiben bit-identisch exakt.

## 0.16.4
- perf(sdf) #384: Offset-Pushdown durch Union und Differenz — die beiden nachweislich exakten Fälle der Minkowski-Algebra (Aufdicken über Vereinigungen, Abtragen über Differenzen und Schnitte, wobei der Subtrahend den gegenläufigen Offset bekommt) werden jetzt zur Regen-Zeit als Baum-Umschreibung bis auf die Blätter durchgereicht. Dort sitzt der Offset auf exakten Feldern statt auf dem nur noch under-bound kombinierten Feld — die strukturelle Grundlage für analytisch gefaltete Offsets und Shell-Korrekturkörper aus dem „offset_tree“-Plan. Nullstellenmenge und Renderbild bleiben unverändert.

## 0.16.3
- perf(sdf) #384: Compile-Zeit-Lipschitz-Netz statt march_relax-Pauschalen — der Kernel leitet die Marsch-Schrittdämpfung jetzt pro Körper aus den Operationen her (Twist/Bend nach Verdrehrate und Hebelarm, Displace nach Amplitude·Frequenz, Chamfer analytisch ×1/√2) statt aus handgeschätzten Konstanten. Getwistete, gebogene und gefaste Körper tunneln damit nicht mehr (bis 16 % der Strahlen stachen zuvor durch), und Revolve-Körper rendern rund 40 % schneller, weil die alte 0,7-Pauschale nachweislich unnötig war.

## 0.16.2
- perf(sdf) #384: Neues zirkuläres Smooth-Minimum (Quílez Circular-CD) in Kernel und Renderer — gleiche Fillet-Geometrie, aber das Distanzfeld übertreibt nie mehr. Der Sphere-March kann dadurch volle Schritte gehen (bisher tunnelte er auf smin-Ketten real durch Oberflächen), die √2-Dämpfung des Kanten-Fillet-Render-Felds entfällt, und CPU, GPU und Mesher rechnen jetzt ein und dasselbe Feld.

## 0.16.1
- fix(sdf-vm) #386: Loop-Zähler-Härtung im GPU-Interpreter — der Masken-Blend zählt seine Polylinien-Schleife jetzt wie alle anderen Ops über einen echten Integer statt über einen Float, und eine leere Maske kann den Programmzähler nicht mehr aus dem Tritt bringen (Kandidat für die „graue Wand“ im sdf-vm-Modus).

## 0.16.0
- feat(onshape) #388: Aus Onshape importierte Projekte zeigen neben dem Titel einen Link zur Onshape-Originalseite und einen Reimport-Button, der nach Bestätigung den Feature-Baum frisch aus Onshape holt.

## 0.15.0
- feat(issues) #387: Schreibt ein Agent einen Ticket-Kommentar, steht im
  Kommentar-Kopf jetzt dezent, welcher Subagent die Arbeit tatsächlich gemacht
  hat — Modell und Effort, z.B. „claude-opus-4-8 · effort high“ (Tooltip mit
  Langform). Gemeint ist der Subagent, der Analyse/Implementierung wirklich
  durchgeführt hat, nicht der Hauptagent, der die Ergebnisse nur zusammenfasst.
  Rein additiv: Kommentare ohne diese Angabe (alle bisherigen) sehen aus wie
  vorher, und bei Nutzer-/System-Kommentaren erscheint die Angabe nie.

## 0.14.2
- fix(onshape) #376: Ein aus Onshape importiertes Projekt merkt sich jetzt auch
  die Onshape-Dokument-URL (`onshape.url`) im gespeicherten `.fdraft` — bisher
  trug `Project.onshape` nur die drei IDs, die URL ging beim Speichern/Laden
  verloren und war im Projekt-JSON nicht mehr sichtbar. „In Onshape öffnen“ in
  der Projektübersicht nutzt jetzt diese gespeicherte URL (mit Tooltip auf den
  Dokumentnamen) statt sie aus den IDs zu rekonstruieren.

## 0.14.1
- fix(fillet) #382: Im Render-Modus sdf-vm wird ein Kanten-Fillet jetzt sauber
  statt degeneriert/artefaktig dargestellt. Das gemischte Distanzfeld war am
  Fillet-Übergang mit Steigung bis √2 nicht mehr 1-Lipschitz, sodass der
  Sphere-Tracer die konkave Fillet-Fläche überschritt; das Render-Feld wird nun
  auf die Lipschitz-Konstante des Smin normalisiert (gleiche Nullfläche, |∇|≤1).

## 0.14.0
- feat(voxel) #379: Debug ▸ Voxel füllt die Szene jetzt nur noch, solange der
  Dialog offen ist — genau wie Debug ▸ Lichen/Mesher —, statt über einen
  eigenen Render-Modus geschaltet zu werden; die Voxel sind zudem mit dem
  Material des Objekts licht-schattiert statt flach selbstleuchtend.

## 0.13.1
- fix(fillet) #382: Sobald ein Kanten-Fillet im Dokument war, verschwand das Modell im Render-Modus sdf-vm fast komplett (grau), während der Mesh-Modus korrekt blieb. Der maskierte Blend schaltete an der Masken-Grenze hart um und riss so ein Loch (Sprung bis Blend-Radius) ins Distanzfeld — der Sphere-Trace tunnelte hindurch, der Mesh (nur Vorzeichen) merkte nichts. Der Render-Pfad blendet den Übergang jetzt stetig aus (Cross-Fade + gedämpfte Schrittweite); Mesh und B-Rep bleiben unverändert.

## 0.13.0
- feat(tickets) #381: Bild aus der Zwischenablage per Strg+V direkt in die Kommentar-Eingabe eines Tickets einfügen — landet als Anhang-Thumb, genau wie über den Büroklammern-Button (#372).

## 0.12.3
- fix(tickets) #377: Die Version-Spalte in der Ticket-Liste blieb leer, obwohl Tickets längst eine Version trugen — der Go-Server ließ das Feld in der Listen-Zusammenfassung (`ticketSummary`) weg. Die Version wird jetzt in jeder Listen-Antwort mitgeliefert.

## 0.12.2
- fix(voxel) #380: Der Voxel-Debug-Diff (Debug ▸ Voxel) samplete das ungefillette Feld — der Kanten-Fillet (#373) tauchte im Occupancy-Vergleich nicht auf. `voxelizeOccupancy` baut jeden Body jetzt über denselben gerundeten Builder wie Mesh- und SDF-Render-Pfad, der Fillet erscheint im Diff.

## 0.12.1
- fix(screenSpaceCsg) #370: Screen-Space-CSG (experimentelles Setting) komponiert keine Bodys mehr, deren Minuend aus mehreren aneinanderstoßenden Leaves besteht (z.B. zwei Extrudes auf demselben Profil) — sie waren großflächig durchsichtig gerendert (Boden-Grid schien durch die Deckfläche). Solche Bodys nutzen jetzt wieder den regulären, korrekten Mesh-Renderpfad.

## 0.12.0
- feat(aichat) #361: Admins können im Assistent-Dialog das OpenRouter-Modell per Dropdown im Panel-Header umschalten; andere Nutzer sehen kein Dropdown und bekommen weiter das erste erlaubte Modell.

## 0.11.1
- fix(aichat) #375: Assistent-Dialog aufgeräumt — kürzerer Placeholder „Frag mich…", Beispiel-Chips
  und das Bau-Beispiel im Begrüßungstext entfernt; Clear-Button zeigt jetzt ein Papierkorb-Icon statt
  eines Refresh-Symbols.

## 0.11.0
- feat(settings) #368: Der Settings-Dialog lässt sich jetzt wie alle anderen Dialoge per Kopfzeile verschieben.
- fix(projects) #374: Die Buttons „Import…“ und „Aus Onshape…“ im Projekte-Bereich sind jetzt wie „New project“ gestylt statt unstyled.

## 0.10.0
- feat(tickets) #365: Die Ticket-Liste zeigt jetzt eine eigene, sortierbare „Author"-Spalte mit dem aufgelösten Namen des Ticket-Erstellers statt der rohen User-ID.
- feat(tickets) #378: Der Titel eines Issues ist in der Detail-Ansicht nachträglich editierbar — nicht mehr nur der Autor, sondern auch Admins können den Titel per „Edit"-Button ändern.
- feat(tickets) #364: „#N"-Ticket-Referenzen im Issue-Text und in Kommentaren sind jetzt anklickbare Links, die das referenzierte Ticket direkt öffnen.

## 0.9.0
- feat(voxel) #369: Neuer Render-Modus „Voxel" füllt die Geometrie mit Würfeln in einer im Debug-Dialog (Debug ▸ Voxel…) einstellbaren Auflösung. Im Dialog lässt sich ein Snapshot speichern, die Geometrie ändern (z.B. Fillet hinzufügen), erneut snapshotten und der Unterschied zu einem gewählten Referenz-Snapshot anzeigen — hinzugekommene Voxel grün, entfernte rot, plus quantitative Auswertung (Anzahl hinzugekommen/entfernt/unverändert und Volumen-Delta). Dasselbe geht programmatisch über die Konsole via window.__fdVoxel (snapshot/diff).

## 0.8.0
- feat(tickets) #372: In der Ticket-Kommentarbox kannst du jetzt über einen Büroklammer-Button Dateien anhängen — sie erscheinen als Vorschau-Thumbnails (Bilder als Bild, andere Dateien als Endungs-Chip) und werden beim Absenden des Kommentars mit hochgeladen, genau wie bei „Report Issue…".

## 0.7.1
- fix(aichat) #371: Der Anhängen-Button im Assistent-Chat zeigt jetzt eine Büroklammer statt eines Bilderrahmen-Symbols — passender zur eigentlichen Funktion (Screenshot anhängen).

## 0.7.0
- fix(fillet) #350: „Kante runden" (Fillet) wirkt jetzt in ALLEN Ansichten — Mesh, SDF-VM und SDF-Deferred — und verändert auch das erzeugte Mesh. Bisher erreichte der Fillet nur den Auswahl-/Pick-Baum, aber nicht das Feld, das Mesher und Renderer tatsächlich auswerten, sodass sich sichtbar nichts änderte.
- feat(fillet) #350: Die Fillet-Details haben eine Debug-Checkbox „Fillet-Zone anzeigen", die die maskierte Zone — die Pill/Kapsel um die Kante, in der der Blend das Feld formt — als transparentes Overlay einblendet.

## 0.6.603
- fix(ui) #366: Der Löschen-Button in der Ticket-Detailansicht ist jetzt ein sauberer Icon-Button — Papierkorb-Symbol in Rot, ohne Rahmen/Hintergrund, mit Hover-Effekt und Zeiger-Cursor.

## 0.6.602
- fix(ui) #362: Der funktionslose „Share"-Menüpunkt ist entfernt.

## 0.6.601
- fix(kernel): Ein Dokument ohne fertigen Körper (z. B. ein Revolve, dem noch die Achse fehlt) legt nicht mehr den ganzen Kernel lahm: die Flächen-Abfrage kommt jetzt leer zurück, statt die Sitzung mit einem Absturz zu beenden.

## 0.6.600
- feat(tickets) #359: In der Ticket-Diskussion steht jetzt der Name der Person, die einen Statuswechsel ausgelöst oder einen Kommentar geschrieben hat — statt des generischen „System" bzw. „User". Rein automatische Vorgänge bleiben „System".

## 0.6.599
- fix(assistant) #360: Der FieldDraft-Assistent ist besser lesbar — alle Texte im Dialog sind jetzt mindestens 1rem groß und der graue Text hat mehr Kontrast. Der „Was ist in diesem Dokument?"-Vorschlag ist weg; „Grundplatte bauen" und „Wie skizziere ich?" bleiben.

## 0.6.598
- fix(tickets) #358: Der Löschen-Button in der Ticket-Detailansicht ist jetzt ein Papierkorb-Symbol am rechten Rand, klar abgesetzt von Bearbeiten und Status statt als weiterer Text-Button — die Sicherheitsabfrage bleibt.

## 0.6.597
- feat(release-notes) #357: Ticket-Nummern in den Release Notes (z.B. #347) sind jetzt anklickbar und öffnen das zugehörige Ticket in der Issues-Ansicht. Interne Kürzel ohne Ticket bleiben schlichter Text.

## 0.6.596
- fix(tickets) #357: Der Zustand eines Tickets wird wieder angezeigt, und das Status-Auswahlmenü zeigt die aktuellen Zustände (Triage, Needs info, In progress, Parked, Closed, Rejected) statt der alten. Grund war, dass die App noch das alte, umfangreichere Zustandsmodell kannte, während der Server längst auf das schlankere umgestellt war.
- feat(tickets) #357: Die Ticket-Liste hat eine Spalte "Version" — bei geschlossenen Tickets steht dort die Release, in der der Fix ausgeliefert wurde (sobald sie beim Schließen gesetzt wird).

## 0.6.595
- fix(tickets): Ticket-Zustände laufen jetzt über das vereinfachte IssueVault-Modell (ein State-Feld statt zweier). Für Melder ändern sich nur ein paar Namen — „erledigt" heißt jetzt einheitlich „geschlossen", Abgelehntes „rejected", und zurückgestellte Tickets („parked") tauchen nicht mehr bei jeder Antwort in der Bearbeitungsschlange auf.

## 0.6.594
- fix(build): Release Notes können den Build nicht mehr kaputt machen. Sie lagen bisher als Programmcode vor — ein einziges gerades Anführungszeichen in einem deutschen Satz beendete den Text-String und legte den kompletten Produktions-Build lahm (viermal passiert). Jetzt sind sie eine reine Textdatei: Anführungszeichen, Backslashes, Klammern — alles harmlos. Eine fehlerhafte Zeile kann die Anzeige auch nicht mehr leeren, sie fällt stattdessen in den Tests auf.
- fix(ui) #348 #349 #350: Ticket-Nummern in den Release Notes wurden mit doppeltem Rautezeichen angezeigt (##348).

## 0.6.593
- fix(build): Der Produktions-Build war kurzzeitig kaputt: in einem Release-Notes-Text stand ein gerades Anführungszeichen statt eines typografischen und zerriss damit den Text.
- perf(measure) #349: Der Measure-Dialog rechnet die Distanz nur noch neu, wenn sich Auswahl oder Geometrie ändern — vorher lief die teuerste Kombination (Kante zu Fläche: alle Kurvenpunkte gegen alle Dreiecke der Fläche) bei jedem Neuzeichnen des Dialogs erneut.

## 0.6.592
- fix(brep) #348: Eine Kante lief an einer runden Nut-Stirn S-förmig quer über die Rundung statt der Nutwand zu folgen. Wo eine Rundung eine Wand tangential berührt, liegen die Eckpunkte beider Nahtlinien nur wenige Millimeter auseinander; beim Abschluss einer Kante wurde der nächstgelegene Eckpunkt „voraus“ gegriffen, ohne zu prüfen, ob er überhaupt seitlich auf der Kurve liegt. Die Kante lief dann die eine Naht entlang und endete auf der anderen. Nebeneffekt: eine Fläche, die an dieser Falschkante zerbrach, schließt sich wieder.

## 0.6.591
- fix(fillet) #350: Ein Fillet blieb im Standard-Renderer unsichtbar — er war die ganze Zeit da. Der Renderer zeichnet die Körper als harte Meshes und rechnet die echte Feldgeometrie nur in den Zonen, die der Kernel ihm dafür meldet; für einen Kanten-Fillet war diese Zone auf den Überlapp der beiden beteiligten Körper beschnitten. Der Rundungswulst quillt an einer Schnittkante aber aus dem Schneidwerkzeug heraus, in Material, das das Werkzeug nie berührt. Im Beispiel des Tickets (Radius 10 an der Wand einer 8 mm breiten Nut) lagen 99,9 % der Rundung außerhalb der gerechneten Zone und wurden nie gezeichnet.

## 0.6.590
- fix(assistant) #347: Der Assistent kann die Oberfläche wirklich bedienen: Klicken und Felder füllen waren ihm beschrieben, aber nie implementiert — er rief treu auf, was er las, und lief in einen Fehler.
- fix(assistant) #347: Der Assistent kennt wieder die Feature-Typen und Variablen, die es im Dokument gerade wirklich gibt — statt einer Platzhalter-Zeile, die ihm nichts verriet.

## 0.6.589
- fix(mesher): Lichen: Polyeder werden mit ε-Abtastung wieder korrekt vermascht. Bisher zerfiel ein 20-mm-Würfel mit lfsFactor + Sizing-Field in acht Klümpchen um die Ecken — ein geschlossenes Netz, dem alle sechs Flächen fehlten. Vier Wurzeln, alle hinter dem Sampling (die naheliegende Vermutung „Medialachse an scharfen Kanten“ war es NICHT — der Kanten-Guard arbeitet einwandfrei): (1) Die Kanten-Abtastung lieferte doppelte Punkte; ein Null-Segment hat keine Richtung, also zählte der Knickwinkel-Akkumulator es als 90°-Knick — eine kerzengerade Box-Kante akkumulierte so bis zu 1800° und setzte Krümmungspunkte 0,1 mm hinter der Ecke. (2) Der Raster-Rest der Kanten-Abtastung (0,09 mm auf 20 mm) löste eine Grading-Kaskade aus, die sich über die Ecken auf alle Nachbarkanten vererbte. (3) Das Wachstum platzierte Punkte mit einem anderen Maß, als es sie annahm — neben einer solchen Mikro-Kette flog jeder Kandidat als „redundant“ wieder heraus und das Flächenwachstum stand still (null Flächenpunkte). (4) Das Sizing-Field erfand bei ausgeschaltetem ε eine eigene Zieldichte. Ergebnis: Würfel 186 → 3610 Dreiecke, χ=2, Hausdorff 0,000; Platte mit Bohrung 959 → 0 offene Kanten. Zwei neue Gates prüfen zwingend auch die FLÄCHEN-ÜBERDECKUNG — wasserdicht allein hätte den Klümpchen-Würfel durchgewinkt.
- fix(mesher): Lichen: die Überdeckungs-Invariante der Kanten-Abtastung hält wieder (sie war auf dünnen Wänden verletzt — im Produktivbetrieb ohne jede Warnung, weil die Prüfung nur in Tests läuft). Ursache waren dieselben doppelten Kettenpunkte: die Phantom-Knicke erzeugten ein Mikro-Segment, das die Abdeckungs-Kreise seiner Endpunkte nicht mehr überdeckten. Duplikate werden jetzt vor der Winkelmessung entfernt; die Invariante hält über alle Testkörper und Konfigurationen.

## 0.6.588
- fix(assistant) #347: Die Assistenten-Route ist gegen Missbrauch gehärtet. Sie war schon vorher nur für eingeloggte Nutzer offen (anonym: abgewiesen; gesperrte Konten verlieren ihre Sitzung), und die Modell-Liste war fest verdrahtet — neu sind drei Dinge: ein Tageskontingent pro Nutzer (die Minutenbremse hält einen Ausbruch auf, nicht einen geduldigen Skriptschreiber), eine Sperre gegen Anfragen, die eine fremde Seite im Namen deines Browsers abschickt, und die Möglichkeit, den API-Schlüssel aus einer Datei statt aus einer Umgebungsvariable zu lesen — die steht sonst in jeder Prozessliste.

## 0.6.587
- fix(assistant) #347: Der Assistent schreibt jetzt gültiges FieldSkript. In der Beschreibung, aus der er lernt, stand ein erfundenes Beispiel — Parameter als Objekt statt als Liste, falsche Werteform, falscher Export. Er hat es brav kopiert, und das Skript ließ sich nicht übersetzen. Jetzt steht dort die echte Signatur (Parameter-Liste, Default-Export, Werte über .value, fd.op*-Funktionen). Schlägt eine Übersetzung trotzdem fehl, räumt FieldDraft den angelegten Skript-Tab wieder weg, statt einen leeren zurückzulassen.
- fix(app) #347: Der Assistent erschien nicht und das Vorschaubild-Meshing brach ab: eine Konstante stand zwischen den Import-Zeilen, was die Hot-Reload-Transformation zerlegt hat.

## 0.6.586
- feat(assistant) #347: Der Assistent kann jetzt FieldSkript schreiben — starten darfst nur du. Wenn eine Geometrie parametrisch oder repetitiv genug ist, dass ein Skript besser passt als ein Stapel Einzel-Features, legt er dir den Quelltext vor: mit einem Satz, was er tut, und dem vollständigen Code. Erst dein Klick auf „Ausführen“ kompiliert und installiert ihn als Feature; Verwerfen ist der Standardweg. Der Quelltext wird vorher geprüft (kein window, kein fetch, kein eval, keine fremden Importe) — dass ein Skript im Kernel Geometrie erzeugen und verändern darf, bleibt aber seine Natur: rückgängig machst du es mit Strg+Z.
- fix(fieldskript) #347: Skripte laufen jetzt garantiert im strict mode. Bisher hing das an einem Detail des Transpilers; im laxen Modus hätte ein gewöhnlicher Funktionsaufruf Zugriff auf das globale Objekt gehabt — und damit an allen Beschränkungen vorbei, die für Skripte gelten sollen.

## 0.6.585
- fix(assistant) #347: Der Schutzwall um den KI-Assistenten ist nachgezogen: Die Chat-Bibliothek prüft den vom Modell geschriebenen Code seit ihrer Version 0.2.0 nicht mehr gegen eine Liste erlaubter Namen, sondern nur noch gegen verbotene Sprachkonstrukte — unbekannte Bezeichner gehen also durch. Damit wären die internen Debug-Schnittstellen der App (window.__fdStore, __fdKernel, __threeRenderer, …) für den Assistenten erreichbar gewesen, vorbei an der kuratierten API mit ihrer Prüfung, Regeneration und Undo-Semantik. Sie sind jetzt ausdrücklich gesperrt, und ein Test hält das fest.

## 0.6.584
- feat(assistant) #347: FieldDraft hat einen KI-Assistenten: ein Chat-Popup unten rechts, das Fragen zur Bedienung beantwortet UND das Modell tatsächlich baut — es liest den Feature-Baum, holt sich die echten Parameter-Schemata und legt Features an, auch die selbstgeschriebenen Skript-Features. Sein Code läuft im Browser hinter einer Allowlist-Prüfung und ausschließlich über dieselben Store-Aufrufe, die auch ein Klick auslöst — jede Änderung ist also ganz normal mit Strg+Z zurückzunehmen. Löschen führt er nur nach ausdrücklicher Bestätigung aus, und Skripte schreiben oder ausführen darf er bewusst nicht. Der Zugang läuft über einen serverseitigen Proxy, der den API-Schlüssel hält; ohne konfigurierten Schlüssel ist das Feature komplett aus.

## 0.6.583
- fix(tickets) #346: Tickets liegen jetzt ausschließlich in IssueVault — die beiden ungenutzten Ersatz-Ablagen (lokale Datenbank, Dateisystem) sind samt Umschalter entfernt. Das war nicht nur Aufräumen: ohne konfigurierten Zugang fiel der Server stillschweigend auf die lokale Ablage zurück und wirkte gesund, während jedes gemeldete Ticket dort landete, wo niemand hinsieht — jetzt scheitert er stattdessen laut. Nebenbei repariert: Admin-Warnungen mit Ticket-Bezug (bösartiges Ticket, zurückgehaltener Kommentar) konnten gar nicht gespeichert werden, weil die Datenbank auf ihre eigene, längst leere Ticket-Tabelle verwies.

## 0.6.582
- feat(onshape) #343: Ein Onshape-Import heißt jetzt wie das Onshape-DOKUMENT (also „Lötkolbenhalter“ statt „Onshape Import“) und merkt sich, woher er kam. Auf der Projektkarte stehen dafür zwei neue Aktionen: „Reimport“ holt den Feature-Baum frisch aus Onshape und ersetzt das Projekt damit (mit Rückfrage; Übersetzungsfehler werden gemeldet statt verschluckt), „In Onshape öffnen“ springt in genau das Part Studio, aus dem importiert wurde.
- fix(projects) #343: Projekte bekommen endlich ein echtes Vorschaubild. Bisher konnte die Aufnahme nur gelingen, wenn man zufällig im Mesh- oder WebGL-Modus arbeitete — in den SDF-Modi (die Voreinstellung) gibt es gar keine Dreiecke, und die Aufnahme zeichnete nichts. Jetzt vermascht sie ihr Dokument bei Bedarf selbst (einmalig, in einem eigenen Hintergrund-Prozess, grob aufgelöst) und zwar beim ÖFFNEN des Projekts, nicht erst beim Verlassen. Zwei weitere Fallen sind mit weg: die Aufnahme eines frisch angelegten Projekts wurde von der 30-Sekunden-Sperre verschluckt, und sie konnte die Geometrie des zuvor geöffneten Projekts erwischen — daher trugen frische Projekte das Bild ihres Vorgängers.

## 0.6.581
- fix(tickets) #345: Ein Ticket konnte durch einen einzigen schludrig geschriebenen Kommentar komplett unerreichbar werden („not found“ statt Detailansicht): steht in dessen Zusatzdaten eine 0 oder 1, wo ein Ja/Nein-Wert erwartet wird, scheiterte das Einlesen des GESAMTEN Tickets. Der Ticket-Speicher liest solche Werte jetzt nachsichtig (0/1 und „true“/„false“ zählen als Ja/Nein); geschrieben werden weiterhin ausschließlich echte Ja/Nein-Werte.

## 0.6.580
- feat(tickets) #345: Der Ticket-Zustand lässt sich jetzt direkt setzen: in der Issue-Ansicht steht neben „Edit“ ein Auswahlfeld „Set state“, dazu ein „Delete“-Knopf (mit Rückfrage). Als Admin sind ALLE Zustände wählbar — auch Sprünge, die der Ablauf-Graph nicht als Kante kennt (etwa Triage → Resolved); der Graph hält den automatischen Fluss User → PO → Dev sauber, soll aber den Betreiber nicht fesseln, und nur so kommt ein Ticket aus einer Sackgasse wieder heraus. Der Autor eines Tickets bekommt weiterhin genau sein sicheres Teilset (schließen, in den Backlog parken, entparken, wieder öffnen) — die vier Spezial-Knöpfe dafür sind im Auswahlfeld aufgegangen.

## 0.6.579
- fix(render) #344: Im SDF-Deferred-Renderer waren die Standard-Ebenen (Top/Front/Right) und die B-Rep-Kanten unsichtbar, während der SDF-VM-Renderer beides korrekt zeigte. Ursache: der Renderer schattiert in ein Zwischenbild und spielt dessen Tiefe danach auf die Leinwand zurück — diese Tiefe wurde aus dem Tiefen-Anhang des Zwischenbildes gelesen, und ein solcher Lesevorgang liefert unter WebGPU durchgehend 0. Damit stempelte das Zurückspielen die vorderste Tiefe über das ganze Bild, und alles, was danach gezeichnet wird (Ebenen, Kanten im Modus „nur sichtbare“), wurde vom Tiefentest verworfen. Die Tiefe reist jetzt als zweites Farb-Ziel mit — Ebenen und Kanten sind zurück und werden korrekt vom Körper verdeckt.

## 0.6.578
- fix(viewport) #1ca0124c: Die Anzeige „8 features 100.0 × 100.0 × 100.0 mm“ oben rechts ist entfallen — die Feature-Zahl steht ohnehin im Outliner, die Maße liefert die Part-AABB-Anzeige. Die Dreieckszahl im Mesh-Modus und Fehlermeldungen bleiben.

## 0.6.577
- feat(export) #faa42489: Der Mesh-Export kennt jetzt auch den Lichen-Mesher (SDF + B-Rep): B-Rep-Ecken und Kantenzüge setzen die Punkte, ein krümmungsadaptives Wachstum füllt die Flächen, ein globaler Manifold-Walk zieht daraus ein geschlossenes Netz. Sichtbar sind genau zwei Regler — die Ziel-Kantenlänge h (das Zellgrößen-Feld) und der Krümmungswinkel (Voreinstellung 10°, steuert die Feinheit gekrümmter Flächen). Alles andere ist im Kernel als Export-Preset festgezurrt: exakt die Konfiguration, unter der die Abnahme-Gates des Meshers wasserdicht sind, plus zwei harte Prüfungen (geschlossen UND die Fläche wirklich überdeckt) — besteht ein Körper sie nicht, fällt er automatisch auf die adaptive Route zurück, und der Bericht sagt, welche Route ihn geliefert hat. Der Lichen-Debug-Dialog mit allen Schrauben bleibt unverändert bestehen.
- feat(export) #6a0354e8: Der Mesh-Export zeigt seine Laufzeit — während des Laufs tickt die verstrichene Zeit mit (der Dialog sieht nicht mehr aus, als hinge er), und der Bericht nennt am Ende die Gesamtdauer. Gilt für alle Meshing-Methoden, auch die Server-Route.

## 0.6.576
- fix(viewport): B-Rep-Kanten werden standardmäßig nur noch gezeichnet, wo sie sichtbar sind (CAD-Konvention wie in Onshape/SolidWorks). Bisher war „X-Ray“ der Default: Rückseitenkanten schienen durch massives Material hindurch und lasen sich wie Risse im Körper — in der Displace-Abnahme wurden sie sogar als Fehlstellen mitgezählt. Der X-Ray-Modus bleibt im Einstellungsmenü erhalten (Durchblick/Debug); wer ihn schon einmal bewusst gewählt hat, behält seine Einstellung.

## 0.6.575
- fix(render): Änderungen, die nur das SDF-Programm betreffen (Displace an/aus, Blend-Radius, Shell-Dicke, Twist-Rate, Unterdrücken/Einblenden eines Features, neuer Körper), zeichnen den Viewport jetzt sofort neu — vorher blieb das alte Bild stehen, bis man die Kamera bewegte. Ursache war keine fehlende Invalidierung, sondern eine Cache-Lücke: Der Frame, der die Änderung ERKENNT, stößt Pipeline-Neubau und Shader-Kompilierung asynchron an und verlässt sich ohne zu zeichnen — seine „gesehen“-Marker (Fingerprints, Material, Selektion) hatte er aber schon geschrieben. Der Frame nach der Kompilierung fand deshalb alles „unverändert“ vor und blittete bei unbewegter Kamera erneut sein zwischengespeichertes Shade-Target, also exakt das Bild von vor der Änderung; nur eine Kamerabewegung brach den Cache. Die Änderung wird jetzt über die Kompilierungslücke hinweg gemerkt und erst nach einem tatsächlich gezeichneten Frame quittiert; der Schnellpfad für reine Overlay-Repaints (Sketch-Hover, Export-Vorschau) bleibt erhalten, und ein ruhendes Bild erzeugt weiterhin keine GPU-Last. Damit ist auch das vermeintliche „Körper wird während der Regeneration unsichtbar“ erledigt — das war dasselbe veraltete Standbild.
- fix(viewport): Die Namensschilder der Datum-Ebenen („Top“, „Front“, „Right“) werden jetzt von Körpern verdeckt, die davor liegen — bisher zeichneten sie ohne Tiefentest durch massives Material hindurch und sahen dort aus wie Löcher im Körper. Die Ebenen selbst waren schon korrekt tiefengetestet; der Ursprungs-Punkt bleibt bewusst durchscheinend, weil er mit 1,5 mm Radius sonst in fast jedem Modell unsichtbar im Material läge.

## 0.6.574
- fix(render): Änderungen, die nur das SDF-Programm betreffen (Displace an/aus, Blend-Radius, Shell-Dicke, Twist-Rate, Unterdrücken/Einblenden eines Features, neuer Körper), zeichnen den Viewport jetzt sofort neu — vorher blieb das alte Bild stehen, bis man die Kamera bewegte. Ursache: Der Frame, der die Änderung ERKENNT, stößt Pipeline-Neubau und Shader-Kompilierung asynchron an und verlässt sich ohne zu zeichnen — er hatte seine „gesehen"-Marker (Fingerprints, Material, Selektion) aber bereits geschrieben. Der Frame nach der Kompilierung fand daher alles „unverändert" vor, meldete dem Deferred-Renderer sceneDirty=false und blittete bei unbewegter Kamera erneut sein zwischengespeichertes Shade-Target — also exakt das Bild von vor der Änderung. Die Änderung wird jetzt über die Kompilierungslücke hinweg gemerkt und erst nach einem tatsächlich gezeichneten Frame quittiert; der Schnellpfad für reine Overlay-Repaints (Sketch-Hover, Export-Vorschau) bleibt erhalten, und ein ruhendes Bild erzeugt weiterhin keine GPU-Last. Damit verschwindet auch das vermeintliche „Körper wird während der Regeneration unsichtbar" — das war dasselbe veraltete Standbild.
- fix(viewport): Die Namensschilder der Datum-Ebenen („Top“, „Front“, „Right“) werden jetzt von Körpern verdeckt, die davor liegen — bisher zeichneten sie ohne Tiefentest durch massives Material hindurch und sahen dort aus wie Löcher im Körper. Die Ebenen selbst waren schon korrekt tiefengetestet; der Ursprungs-Punkt bleibt bewusst durchscheinend, weil er mit 1,5 mm Radius sonst in fast jedem Modell unsichtbar im Material läge.

## 0.6.573
- fix(mesher): Lichen: Sliver-Reparatur an streifenden Nähten und Ketten-Fächern (Streifschnitt 3,6°: min. Winkel 5,2° → 11,8°, Aspect p99 3,97 → 3,09; Twist-Box 9,0° → 12,0°, Bend und koplanare Vereinigung ebenfalls verbessert). Wurzel per Winkel-Histogramm-Forensik: Sliver sind Ketten-Fächer — 0,1-mm-Ketten-Kante an der Naht-Kurve gegen ~1-mm-Flächenpunkte als Apex, ein intrinsischer Dichtesprung. Neuer Post-Pass nach dem globalen Walk (Default an, Kill-Switch vorhanden): Rivara-Bisektion der längsten Nicht-Ketten-Kante je Sliver (Newton-Mittelpunkt, Separations-Gate gegen Fast-Duplikate — ohne das stallt die Reparatur bei 6,5°), tangentiales Glätten mit striktem max-min-Winkel-Wächter (Ketten und Ecken bleiben fix, Regionsgate, Sagitta-Sehnen-Prüfung), Lawson-Flips (nie über Ketten, Blatt-Restriktion). Wasserdichtheit, Orientierung und Hausdorff bleiben in allen Fällen erhalten; der Gyroid-Abnahmefall ist byte-identisch (Pass dort inaktiv). Kanten-Flips als Erstmaßnahme wurden vermessen und verworfen (0 von 462 Kanten verbesserbar — die Walk-Diagonale ist lokal optimal; erst nach Split und Glätten tragen Flips). Neues aktives Gate für den Streifschnitt.

## 0.6.572
- feat(kernel): Tangentiale Nähte: Ein abgerundetes Rechteck hat jetzt acht Seitenflächen statt einer. Bisher verschmolz FieldDraft eine Linie mit ihrem tangentialen Verrundungs-Bogen zu EINER Fläche — Flächen wurden nur an Knicken getrennt. Damit existierte „die linke Seite“ eines verrundeten Körpers gar nicht: man konnte dort weder skizzieren noch verrunden, und die ganze Mantelfläche fiel auf Tessellierung zurück. Der Grund war fundamental: die Naht zwischen Ebene und Zylinder ist tangential, und danach kann keine Flächen-Schnitt-Suche greifen (eine Berührung ist eine doppelte Nullstelle ohne Vorzeichenwechsel, und die Kurvenverfolgung wird dort singulär). Neu wird die Naht deshalb nicht gesucht, sondern KONSTRUIERT — sie ist exakt bekannt — und durchläuft danach dieselbe Beschneidung wie jede getracte Kante. Nebengewinn: verrundete Rechtecke und Langlöcher werden analytisch gewoben (exakte Ebenen und Zylinder) statt tesselliert.
- feat(onshape): Onshape-Import: Das Testmodell (Kabelbox) importiert jetzt VOLLSTÄNDIG — alle sieben Sketches liegen auf der Fläche, auf der Onshape sie gezeichnet hat, drei davon auf einer Seite des verrundeten Grundkörpers. Die Referenzen sind parametrisch: ändert man den Grundkörper, wandern die Sketch-Ebenen mit. Neu übersetzt werden außerdem „bis zur Fläche“-Extrudes gegen die Shell-Innenwand (neue Query-Form qWall(op, entity, INNER)) und Fillets auf Kappen-Kanten.

## 0.6.571
- feat(mesher): Lichen: Displace-Körper werden wasserdicht gemesht (Displace-Kugel 662 → 0 offene Kanten, χ=2, Hausdorff beidseitig ≤ 0,005 mm) — damit ist die gesamte Warp-Familie (Twist/Bend/Displace) geschlossen. Überraschender Kern-Befund: Die 662 offenen Kanten kamen komplett aus den REGIONEN, nicht aus fehlenden Ketten (die Kugel hat eine Fläche und null Kanten) — die Träger-Klassifikation maß Abstände gegen die UN-displacte Fläche und verwarf legitime Punkte als „0,3 mm daneben“. Fix: FaceSurf rechnet das Träger-Feld jetzt displace-bewusst (Halbraum − Beulen-Term, Normalen als Welt-Differenzenquotient); dazu webt der Kernel Root-Displace-Nähte exakt (Field::DisplacedAnalytic im DL-68-Tracer, SSI-Seeds per Newton auf die displacte Naht projiziert, f64-Ecken-Politur). Ehrlich dokumentierte Modell-Grenze: wo der Beulen-Term positiv ist, wird die Leaf-SDF außen euklidisch und die Naht zum runden Blend-Streifen — exakte Ketten-Topologie gilt im eingeschnittenen Regime, im gemischten liegen alle Samples nachweislich auf dem Body. Zusätzlich headful-WebGPU-Abnahme des Displace-Render-Fixes (Schrittdämpfung aus Release 0.6.568): 0 Marching-Löcher an Streifwinkeln, Silhouette artefaktfrei, marchRelax-Anker 0,5548 — als dauerhafte e2e-Spec (displace-march.spec.ts). Nebenbefund fürs Backlog: SDF-only-Commits (Displace an/aus) lösen im Demand-Frameloop kein Repaint aus. Abnahme-Gate byte-identisch, Suite 1111/1111.

## 0.6.570
- fix(onshape): Onshape-Sketch-Import — verrundete Ecken hielten nicht: Onshape bindet die ENDPUNKTE eines Fillet-Bogens an die angrenzenden Linien, der Übersetzer registrierte für Bögen und Kreise aber nur die Kurve selbst. Jede dieser Kopplungen ging still verloren, die Ecke fiel unter dem Solver auseinander. Jetzt werden Mittelpunkt, Start und Ende jeder Kurve mitgeführt. Außerdem neu übersetzt: alle Bemaßungen (Abstand, Länge, Winkel, Radius, Durchmesser — mit Einheiten-Umrechnung) sowie Mittelpunkt-, Fix- und Punkt-auf-Objekt-Constraints. An der Kabelbox steigt die Constraint-Übernahme damit von 22/36 auf 34/36 im Basis-Sketch (241/297 über alle sieben).
- feat(onshape): Onshape-Import — End-Conditions und Kanten: THROUGH_ALL wird jetzt an der Szene GEMESSEN statt pauschal genähert (der Schnitt räumt die Szenen-Diagonale, das Ergebnis ist identisch zum Durchbruch); UP_TO_SURFACE/UP_TO_FACE nimmt die Tiefe exakt aus der aufgelösten Zielfläche (gebacken — die Warnung sagt es); symmetrische und zweiseitige Extrudes werden übernommen, ein Draft-Winkel wird gemeldet statt still ignoriert. Fillets: Shell-Innenkanten (Boden trifft Innenwand) sind jetzt adressierbar — dafür unterscheidet das Rollen-Vokabular neu zwischen Innenwand und Innen-Cap (INNER_CAP_TOP/INNER_CAP_BOTTOM); nicht unterscheidbare Geschwister-Kanten werden zusammengefasst und gemeldet statt doppelt verrundet.

## 0.6.569
- feat(mesher): Lichen: Twist- und Bend-Körper werden wasserdicht gemesht (Twist-Box 864 → 0 offene Kanten, χ=2; Bend-Box auf Anhieb dicht). Gewarpte Körper hatten bisher nur ein Oktree-B-Rep mit zersplitterten Sample-Fit-Nähten und keine Regionen (Lichens Flächen-Klassifikator kannte nur analytische Träger). Jetzt: (1) Der Kernel webt Root-Twist/Bend exakt — SSI-Seeds aus dem ungewarpten Baum werden durch den Vorwärts-Warp gemappt (Twist geschlossen, Bend per 1D-Newton; der Rück-Warp ist die bestehende Eval-Formel) und der DL-68-Tracer verfolgt die gewarpten Kanten-Kurven (Helizes) über ein neues WarpedAnalytic-Feld mit exaktem Dual-Gradienten inkl. Warp-Jacobian; Ecken bekommen eine f64-Nachpolitur (die f32-Toleranz ließ den Bend-Ecken-Zeugen durchs Aktivitätsband fallen). (2) Lichens Flächen-Klassifikation, Wachstum, Einfärbung und Ketten-Normalen sind warp-bewusst (FaceSurf trägt die Warp-Kette; Normalen über Welt-Differenzen statt bloßem Zurückrotieren — sonst fehlt der Scher-Anteil). Neues aktives Gate für beide Warp-Fälle; Abnahme-Gate und alle bestehenden Ergebnisse byte-identisch; Warps innerhalb von Booleans und Displace laufen weiter über den Oktree-Fallback (Displace folgt separat).

## 0.6.568
- feat(onshape): Parametrischer Onshape-Import: „Aus Onshape…“ in der Projekt-Übersicht importiert ein Part Studio als FieldDraft-Projekt — Sketches (Geometrie + Constraints) samt ihrer EBENEN als parametrische Referenzen (Standard-Ebenen, Extrude-Caps) plus Extrude/Shell/Fillet in einer Erstübersetzung. Onshapes kodierte Creation-Queries (qCompressed/BTON) werden dekodiert und auf die neuen FieldDraft-Query-Formen abgebildet (qCreatedBy(op, ROLLE), qWall, qAdjacentTo, qCommonEdge); Mantelflächen tragen dafür jetzt stabile Sketch-Entity-Sub-IDs (Naming-Schema v2, bestehende Referenzen heilen über den Fingerprint). Bekannte Grenze: Sketches auf einer SEITE eines verrundeten Körpers lassen sich noch nicht platzieren — FieldDraft trennt Flächen nur an Knicken, ein abgerundetes Rechteck hat daher eine einzige Mantelfläche. Solche Fälle scheitern sichtbar im Dialog mit benannter Ursache — nie stiller XY-Fallback.
- fix(render): Displace-Körper: Sphere-Marching konnte die Beulen-Fläche überschreiten (Löcher/Artefakte bei Streifwinkeln), weil das Displace-Feld nicht 1-Lipschitz ist (L ≈ 1 + |depth·freq|·√(1+1.3²+0.7²)). Der Renderer bekommt jetzt eine Schrittweiten-Dämpfung 1/L über den bestehenden march-relax-Mechanismus (wie Revolve mit 0.7) — das Feld selbst bleibt bitidentisch unnormalisiert, denn die Feld-Normalisierung wurde in der Härtungs-Session vermessen und macht den Lichen-Mesher messbar schlechter (813 → 1634 offene Kanten; Befund in bvh.rs). Der Mesher ist von dieser Änderung beweisbar unberührt.

## 0.6.567
- fix(mesher): Lichen: adversariale Härtung gegen 10 Streit-Geometrien (dünne Features, Domain-Warps, degenerierte Boolesche Schnitte, streifende Verschneidungen). Vier echte Fixes: (1) Phantom-Ketten bei koplanaren Booleans — aus ungeordneten Fallback-Loops des Weave entstanden Diagonal-„Ketten“ quer über ebene Flächen, deren Kreuzungs-Veto legitime Dreiecke blockte (koplanare Vereinigung 46 → 0 offene Kanten, wasserdicht); jetzt zählen nur konsekutive Segmente echter Kanten-Ketten. (2) Regionen-Verschmelzung: Rücken-an-Rücken-Blätter (antiparallele Normalen) verschmelzen nie mehr, und der Berühr-Radius nutzt das Minimum beider Punktabstände statt des groben Streu-Seeds (Punkt-Kuss 59 → 33 offene). (3) Naht-Stitching läuft jetzt als kantenbasierter Loop-Walk über Halbkanten — Grad-4-Zwickel ließen vorher ganze Loops offen — und schließt auch 5er-Loops. (4) Displace in der fidget-Brücke nutzte als einzige Repräsentation die Achsfaktoren 1.3/0.7 nicht — war also eine andere Fläche. Drei neue aktive Gates: koplanare Vereinigung, Streif-Naht an anderem Ort (beweist: keine Überanpassung an den Abnahmefall), dünnes Rohr (χ=0-Torus-Anker). Ehrlich dokumentierte Grenzen als ignored-WIP mit Diagnose: Twist/Bend/Displace-Bodies (kein B-Rep → keine Ketten/Regionen), exakt tangentiale Küsse (Blattabstand 0 ist sub-resolution für jede Abtastung). Abnahme-Gate byte-identisch grün, Kernel-Suite 1082/1082.

## 0.6.566
- fix(mesher): Lichen: Gyroid-Abnahmefall WASSERDICHT — 21 → 0 offene Kanten (χ=2, 0 non-manifold, Hausdorff/Aspect/Winkel unverändert: H(m→s) 0,021 mm, H(s→m) 0,098 mm, Aspect p99 1,85, min. Winkel 14,9°; auch bei h=6 und h=14 dicht). Zwei Wurzeln, per Kreuzungs-Segment-Forensik isoliert: (1) Das Ketten-Kreuzungs-Veto projiziert jedes nahe Ketten-Segment in die Dreiecksebene — an Zwickeln konvergierender Nähte vetoten dadurch FREMDE Blätter, die 0,1–0,2 mm ÜBER dem Dreieck schweben, jeden legitimen Fächer (13 der letzten 21). Fix: Blatt-Restriktion wie bei der Zeugen-Kugel (Kette ohne gemeinsame B-Rep-Region mit dem Dreieck kreuzt nicht) plus Ebenen-Abstands-Gate (Segment ganz auf einer Seite und weiter als 0,5·Spacing von der Ebene kann nicht durchstoßen). (2) Die verbleibenden Mikro-Löcher (3–4-Kanten-Loops, Grad 2) waren voll orientierbare, strikt auf der Fläche liegende Schließ-Dreiecke, die NUR am Zeugen-Kugel-Kriterium scheiterten (Newton konvergiert am SDF-Knick nicht bzw. Kugel im überfüllten Keil nie leer — Abtast-Artefakte). Neuer deterministischer Naht-Stitching-Post-Pass: Mikro-Loops (≤5 Kanten) werden geschlossen, wenn jedes Dreieck die volle geometrische Gültigkeit (Degenerations-Guard, Normalen-Guard, Sichtlinien, Ketten-Kreuzung) besteht — nur das Zeugen-Kriterium entfällt; additiv und manifold-sicher. Segment-Segment-Verschweißung (Zwickel-Schnittpunkt-Hypothese) implementiert, vermessen, verworfen: es existieren keine Off-Vertex-Quasi-Schnitte. Lichen-Gates 33/33, Kernel-Suite 1078/1078 grün.

## 0.6.565
- fix(mesher): Lichen: Naht-Risse an fast-tangentialen Verschneidungen weitgehend geschlossen (Gyroid-Ausschnitt 312 → 21 offene Kanten, Hausdorff beidseitig ≤ 0,1 mm, Aspect p99 1,85). Drei Wurzeln, per WHY-Forensik isoliert: (1) Die Naht-Band-Einfärbung (25°-Tangential-Schwelle) gab JEDEM nahtnahen Punkt beide Regionen — die blatt-restringierte Zeugen-Kugel war dadurch wirkungslos; Schwelle jetzt 10° (die realen Keilwinkel liegen bei 16–24°). (2) Der 90°-Normalen-Guard blockte am Zwickel, wo die Gyroid-Wand spitz auf den Zylinder zuläuft, 243 voll gültige Dreiecke — das Netz muss dort legitim >90° um die Spitze laufen; strikte Ausnahme: >90°-Paare nur, wenn die Sehne strikt auf der Fläche liegt (schließt zugleich das θ-skalierte Sichtlinien-Schlupfloch; Platten-Querstiche fallen weiter durch). (3) Kettenpunkte VERSCHIEDENER B-Rep-Kanten im Abstand unterhalb der Auflösung (0,09 mm am Zwickel) teilen jetzt einen Mesh-Vertex (Verschweißung bei 1,5·minEdgeSpacing), und das Ketten-Kreuzungs-Veto ignoriert fremde Ketten, die näher als ihr eigenes Spacing an einer Dreiecks-Ecke vorbeilaufen. Der theoriegeleitete Schutzkorridor (Cheng–Dey–Ramos protecting balls) wurde vollständig implementiert und in 13 Konfigurationen vermessen — nie besser als 287 offene Kanten, bricht ab β≥2 Hausdorff und Winkel; bleibt als Default-aus-Werkzeug (LICHEN_PROTECT_MODE) erhalten. Volle Kernel-Suite 1078/1078 grün.

## 0.6.564
- fix(mesher): Lichen: blatt-restringierte Zeugen-Kugel (restricted Delaunay je glattem Patch, nach Cheng–Dey–Levine). An einer fast-tangentialen Naht (Gyroid∩Zylinder im Betreiber-Testfall) unterschreitet der Abstand der beiden Flächenblätter den Punktabstand — die Zeugen-Kugel jedes nahtnahen Dreiecks enthielt damit zwangsläufig Punkte des Gegenblatts, beide Blätter blockierten sich wechselseitig und die Naht blieb als Riss offen. Jetzt zählen nur Punkte, die eine B-Rep-Region mit dem Dreieck teilen, als Kontamination; Ketten-Punkte tragen beide Regionen und wirken weiter beidseitig, dünne Wände bleiben geschützt (beide Blätter derselben Fläche = dieselbe Region). Die Flächen-Einfärbung + Regionen-Verschmelzung laufen dafür immer (bisher nur bei aktivem Flächen-Guard). Gyroid-Ausschnitt: 397 → 312 offene Kanten, alle Mikro-Löcher entlang der Nähte geschlossen; Box/Zylinder/Kugel/Torus/dünne Platte unverändert wasserdicht.

## 0.6.563
- feat(mesher): Lichen: Delaunay-Refinement nach Boissonnat–Oudot (der Algorithmus hinter CGAL) — kein Punkt-Template mehr, sondern ein Kriterium. Die Zeugen-Kugel jedes Dreiecks (Zentrum auf der Fläche, durch die drei Ecken) misst, wie grob dort abgetastet ist; ist ihr Radius größer als die Ziel-Dichte, wird in ihr Zentrum ein Punkt eingefügt und neu vernetzt, bis kein Ball mehr zu groß ist. Der Hintergrund ist topologisch: ein reines Sechseck-Netz hat Euler-Charakteristik 0 und kann auf einer Kugel (χ=2) prinzipiell nicht schließen — dort braucht es exakt 12 Fünfecke; ein starres Waben-Muster schiebt diese unvermeidlichen Defekte an willkürliche Stellen, und genau dort reißen die Löcher. Messung: konvergiert sauber (1311 → 58 → 1 → 0 Einfügungen), füllt das vom Betreiber gemeldete punktleere Fünfeck, und ist auf Box/Zylinder/Kugel/Torus ein No-op — dort liefert das Wachstum bereits gute Dreiecke. Wichtig: das Refinement braucht dieselbe Ketten-Clearance wie das Wachstum (an einer B-Rep-Kante sind Dreiecke naturgemäß schmal, ihre Bälle daher groß — ohne die Regel ruinierte es die Box).

## 0.6.562
- fix(mesher): Lichen: das Wachstum ließ punktleere Löcher mitten auf gekrümmten Flächen stehen (Betreiber-Befund auf der Mini-Kugel: ein Fünfeck von 0,26 mm bei 0,11 mm Punktabstand). Drei Ursachen: (1) Der Ketten-Vorhang cullte nur nach dem Abstand zum Segment-MITTELPUNKT — eine lange Kante, die 1 mm neben der Fläche verläuft, blieb im Test, und die 2D-Projektion in die Tangentialebene ließ sie den 0,1-mm-Schritt scheinbar kreuzen; jetzt echter 3D-Strecken-Abstand. (2) Die Redundanz-Schwelle nahm das Spacing des GRÖBSTEN Nachbarn — an einem Dichte-Übergang verwarf sie damit Kandidaten mitten in der Lücke; mit Sizing-Field wird jetzt die bekannte Ziel-Dichte am Kandidaten benutzt. (3) Das Waben-Wachstum probiert nur SECHS feste Richtungen — eine Lücke, die keine davon trifft, bleibt für immer leer; ein Nachlauf wirft mit 12 Richtungen × zwei Radien und nimmt nur gut separierte Kandidaten (≥ 0,8·σ), kann also keine Überdichte erzeugen.

## 0.6.561
- fix(mesher): Lichen: die Optionen des Cocone-Stacks kamen im Kernel NIE an — das Overlay reichte Manifold-Extraktion, Globaler Walk, ε (lfs), Sizing-Field und Flächen-Guard nicht weiter, die Checkboxen waren also wirkungslos. Jetzt werden sie übergeben.
- feat(mesher): Lichen: Punkt-Inspektor — ALT+Klick im Viewport wählt den bildschirm-nächsten Mesh-Punkt (magenta markiert) und der Dialog zeigt seinen vollständigen Diagnose-Block: Position, Klasse (Ecke/Kante/Fläche), SDF-Wert, U(p), alle Flächen-Normalen (eine je angrenzender Fläche), Grad, sämtliche Nachbarn mit Abstand und Klasse sowie alle Defekte, die den Punkt berühren (offene Kanten, überlange Kanten, 2-Stimmen-Kandidaten). Ein Knopf kopiert den Block samt Settings in die Zwischenablage — damit lässt sich ein konkreter Punkt eindeutig benennen und besprechen.
- feat(mesher): Lichen: die drei Diagnose-Ebenen sind einzeln abschaltbar — Loch-Kanten (rot), 2-Stimmen-Kandidaten (orange) und überlange Kanten (violett) haben je eine eigene Checkbox.

## 0.6.560
- fix(mesher): Lichen: der Flächen-Guard hilft jetzt (Demo 295 → 283 offene Kanten, vorher verschlechterte er auf 393). Zwei Ursachen behoben: (1) REGIONEN-KORREKTHEIT — die Flächen-Träger sind unendlich, eine Ebene endet nicht an ihrer Kante; ein kantennaher Punkt der Deckfläche ist auch dem Träger der Seitenfläche nah und bekam fälschlich deren ID. Eine Zweitzuordnung ist nur dort sinnvoll, wo die beiden Träger am Punkt fast PARALLEL stehen — genau die tangentiale Verschneidung, für die der Guard existiert (an einer scharfen Kante trennt die Kette ohnehin). Mehrfach-Zuordnungen 445 → 176. (2) REGIONEN-VERSCHMELZUNG per Union-Find für Flächen, die sich berühren, aber keine gemeinsame Kette haben (gedroppte Trim-Nähte) — im Demo-Projekt greift sie nicht, die B-Rep ist dort vollständig; der Mechanismus bleibt als Sicherheitsnetz. Die dünne Platte bleibt wasserdicht mit 0 Wand-Durchquerungen.

## 0.6.559
- feat(mesher): Lichen: Flächen-Guard („Flächen-Einfärbung“, experimentell) — jeder Punkt trägt jetzt die MENGE der B-Rep-Flächen, auf denen er liegt (Punkte im Naht-Band zweier Flächen tragen zwei IDs), und ein Dreieck ist nur gültig, wenn seine drei Ecken eine gemeinsame Fläche haben. Das ist ein TOPOLOGISCHES Kriterium und sieht, was Normalen prinzipiell nicht sehen können: zwei Flächen, die sich TANGENTIAL nähern, haben ÄHNLICHE Normalen (nicht antiparallel), also greift dort kein normalen-basierter Guard. Messung bestätigt die These: die Blockaden des Normalen-Guards fallen von 133 auf 15 — der Flächen-Guard subsumiert ihn. Als harter Guard kostet er derzeit aber noch mehr, als er bringt (Demo 295 → 393 offene Kanten), weil die Flächen-Träger unendlich sind (Über-Zuordnung) und gedroppte Trim-Nähte Flächen ohne gemeinsame Punkte lassen. Default aus.

## 0.6.558
- feat(mesher): Lichen: Sizing-Field (Lipschitz) — der letzte strukturelle Baustein des Cocone-Programms. Zweimal hatte sich gezeigt, dass eine LOKALE Verfeinerung das Netz verschlechtert, obwohl sie geometrisch korrekter ist: sie erzeugt abrupte Dichte-Sprünge, an denen Sterne und Umkugeln auseinanderlaufen. Das Sizing-Field macht es global: erst grob abtasten, an dieser Wolke die feine local feature size messen (ein grober Marsch sieht kleine Spalte gar nicht), das Feld Lipschitz-glätten (|∇σ| ≤ 0,3) und die GESAMTE Abtastung — Kanten wie Wachstum — damit wiederholen. Wirkung auf dem Demo-Projekt: die relative Defektrate fällt bei feiner Auflösung von 7 % auf 2,6 %, alle glatten Körper bleiben wasserdicht.

## 0.6.557
- fix(mesher): Lichen: die Reach-Märsche bekommen ihren Startschritt jetzt als Parameter (statt fest 0,02·Cap). Damit ist vermessen, wo die Grenze der Abtastung liegt: ein feiner Start löst die 0,17-mm-Spalte zwischen zwei Gyroid-Blättern wirklich auf — divergiert aber an tangentialen Kontakten, wo die local feature size gegen null geht (das Demo-Projekt lief nach 15 Minuten noch immer; mit Auflösungs-Boden brach es auf 619 statt 295 offene Kanten ein). Genau das sagt die Theorie voraus: an einem Sub-Resolution-Kontakt gibt es keine endliche gültige Abtastung. Verfeinerung ohne ein global glattes (Lipschitz-beschränktes) Dichtefeld schadet mehr, als sie nützt — im Code als Irrweg dokumentiert.

## 0.6.556
- fix(mesher): Lichen: der ÄUSSERE Reach erkennt die Medialachse jetzt an der Abflachung des Distanzfelds (wie der innere seit einigen Versionen) — die alte Fußpunkt-Verfolgung schlug erst an, wenn der Fußpunkt weiter als die Reach-Toleranz (1 mm) springt, und war damit für einen 0,17-mm-Spalt zwischen zwei Gyroid-Blättern blind. Außerdem hebelt die lfs-Bedingung den Mindest-Kantenabstand jetzt lokal aus: sie ist die Voraussetzung des Umkugel-Theorems, kein Komfort — ein Boden, der sie bricht, erzeugt genau den Zustand, in dem kein einziges Dreieck mehr gültig ist.

## 0.6.555
- fix(mesher): Lichen: Krümmungs-Schätzung zweistufig — das FD-Fenster des Shape-Operators hing fest an der globalen Ziel-Kantenlänge (0,1·h = 1 mm bei h=10) und war damit auf feinen Features blind (eine Gyroid-Wand ist selbst nur 1 mm dick); jetzt wird erst grob geschätzt, daraus ein Radius abgeleitet und mit einem daran gekoppelten Fenster nachgemessen.

## 0.6.554
- fix(mesher): Lichen: Wächter-Rückbau im globalen Walk (Demo 814 → 303 offene Kanten). Drei Befunde: (1) Das Ketten-Kreuzungs-Verbot cullte weit entfernte Ketten nach dem GRÖSSTEN Spacing der gesamten Punktwolke — im globalen Walk sind das Zentimeter, also wurden Ketten quer durch den Körper in die Dreiecksebene projiziert und „kreuzten“ dort scheinbar (279 von 655 offenen Kanten); jetzt echter 3D-Abstand zum Dreieck. (2) Der Normalen-Guard war mit 30° zu streng — auf dem Gyroid drehen sich benachbarte Flächen-Normalen schon um ~35°. Für die Blatt-Trennung genügt die prinzipielle Grenze „gleiche Halbkugel“ (90°): das Gegenblatt einer dünnen Wand steht ~antiparallel und wird sicher erwischt. (3) Der Cocone-Kegel ist damit ERSATZLOS entfallen — er prüfte gegen die gemittelte Punkt-Normale, die an jeder B-Rep-Kante bedeutungslos ist, und lehnte legitime Kanten-Dreiecke ab. Die dünne Platte bleibt in allen Varianten wasserdicht mit 0 Wand-Durchquerungen.

## 0.6.553
- feat(mesher): Lichen: die dünne Platte ist wasserdicht — der schärfste Testfall (Gegenblätter 1 mm auseinander, eine nur 1 mm breite Seitenfläche) schließt jetzt vollständig (0 offene Kanten, χ=2, 0 Wand-Durchquerungen), ohne Chart, ohne Loch-Füller. Zwei Wurzeln: (1) die B-Rep-KANTEN folgten dem ε-Sampling nicht — auf der Platte blieb die Kette bei 1,75 mm, während die Flächen auf 0,26 mm verfeinert wurden; ein Ketten-Punkt sitzt dann als grober Fremdkörper in einer dichten Wolke und die leere Umkugel jedes Dreiecks mit ihm ist verletzt. (2) Die erste gewachsene Punktreihe stand auf HALBEM Abstand vor der Kante (0,14 mm neben einer Kette, deren eigene Nachbarn 0,48 mm entfernt sind) — solche Punkte liegen in der Umkugel der Kanten-Dreiecke und machten die schmale Seitenfläche unmeshbar. Jetzt: lfs-Abtastung auch auf den Ketten und eine Ketten-Clearance im Wachstum.

## 0.6.552
- fix(mesher): Lichen: Sliver aus drei aufeinanderfolgenden Kettenpunkten vergifteten den Manifold-Walk — ihr Umkreisradius rechnete numerisch zu NULL (der Entartungs-Test war absolut statt relativ), womit sie in jeder Umkreis-Sortierung als „bester“ Kandidat galten und die Fächer aller Kettenpunkte blockierten. Jetzt relativer Entartungs-Test plus ein Degenerations-Guard in der Gültigkeitsprüfung. Dünne Platte: degenerierte Dreiecke 2 → 0, Komponenten 4 → 2, Euler 0 → 2.
- fix(mesher): Lichen: Manifold-Walk kann Konflikte auflösen (Flip-Kaskade) — blockiert ein schlechteres Dreieck den vom Sweep eindeutig bestimmten Nachbarn, wird es verdrängt und die neu entstehenden offenen Kanten laufen nach. Außerdem Loch-Forensik korrigiert: sie zählte bisher das BEREITS VORHANDENE Dreieck als gültigen Kandidaten und meldete dadurch Sackgassen, wo keine waren.

## 0.6.551
- fix(mesher): Lichen: Normalen-Guard gegen Wand-Durchquerungen (Betreiber-Idee). Der bisherige Cocone-Filter prüft die RICHTUNG zum Nachbarn (muss ~tangential sein) — bei zwei fast parallelen Blättern im Abstand t ist eine Sehne quer über die Wand zu einem lateral entfernten Punkt aber nahezu tangential (Winkel ≈ atan(t/L)), er kann sie also prinzipiell nicht sehen. Verlässlich ist der Winkel zwischen den NORMALEN beider Endpunkte: quer durch die Wand sind sie ~antiparallel. Scharfe Kanten behalten ihre Sonderbehandlung — geprüft wird über die Normalen-MENGE je Punkt (eine pro angrenzender Fläche, Kantenpunkte tragen zwei). Messung an einer 1-mm-Platte: ohne Guard 111 Wand-Durchquerungen und χ = −67 (34 falsche Henkel quer durch die Platte), mit Guard 0.

## 0.6.550
- fix(mesher): Lichen: ε-Sampling respektiert jetzt scharfe Kanten (Feature-Protection). Die Medialachse eines Körpers läuft bis in jede scharfe Kante hinein (der Bisektor zweier Nachbarflächen berührt sie) — die naive lfs-Abtastung verdichtete dort ins Unendliche und zerstörte das Netz (Würfel 263 → 19.097 Punkte, 3.565 offene Kanten). Der Innen-Marsch zählt nur noch echte GEGENBLÄTTER (antiparallele Fußpunkt-Normale); Kanten sind durch ihre B-Rep-Ketten ohnehin exakt abgetastet. Außerdem bindet ε nur noch, wenn wirklich eine Medialachse gefunden wird (vorher refinete der Faktor systematisch jede Fläche). Ergebnis: dünne Wand 103 → 9.279 Punkte, dicker Würfel unverändert.
- fix(mesher): Lichen: der Manifold-Walk arbeitet die Front jetzt best-first ab (sicherstes Dreieck zuerst, kleinster Umkreis) statt FIFO, und Start-Dreiecke dürfen keine bereits benutzte Kante berühren — ein FIFO-Walk legte sich an unsicheren Stellen früh fest und blockierte sich die Halbkanten der später erreichten Regionen.

## 0.6.549
- feat(mesher): Lichen: Globaler Walk (Cocone über die GESAMTE Punktwolke statt pro B-Rep-Fläche) — Box, Zylinder, Kugel UND Torus sind damit ein geschlossenes 2-Manifold ohne Chart, ohne Kandidaten-Vervollständigung, ohne Loch-Füller; der Torus (im Chart-Pfad nie lösbar) fällt als Nebenprodukt ab und wird korrekt als Genus 1 erkannt (χ=0). Nötig war er, weil die per-Fläche-Zerlegung an gedroppten Trim-Nähten (Gyroid) prinzipiell Schlitze lässt: ohne geteilte Punkt-Kette endet jede Fläche mit freiem Rand. Analyse: die Restlöcher waren geschlossene Loops, keine Sampling-Defekte.
- feat(mesher): Lichen: ε-Sampling gegen die local feature size („ε (lfs-Abtastung)“) — die Punktdichte hält jetzt optional space ≤ ε·lfs ein, wobei lfs die Distanz zur Medialachse ist, INNEN wie außen. Das Konzept hatte den Marsch ins Material „zur Performance-Optimierung“ ausgelassen; die Analyse zeigt, dass genau das die Lücke ist: auf einer 1-mm-Wand erreicht die leere Umkugel sonst das Gegenblatt (die Flächen-Zerlegung hat das nur maskiert). Die Medialachse wird über die Abflachung des Distanzfelds erkannt (robust, im Gegensatz zur Fußpunkt-Verfolgung, die auf dünnen Wänden unter der Toleranz bleibt).

## 0.6.548
- feat(mesher): Lichen: Manifold-Extraktion (Sheet-Walk) und Zeugen-Umkugel — die Loch-Forensik hat gezeigt, dass ~85 % aller offenen Kanten NICHT am Kugel-Prädikat scheitern, sondern daran, dass eine Nachbarkante bereits von zwei ebenfalls GÜLTIGEN Dreiecken belegt war: es gibt mehr gültige Dreiecke, als in eine Fläche passen. Neue Option „Manifold-Extraktion (Walk)“ läuft die Fläche aus der Kandidaten-Menge heraus (Pivotieren um die Front-Kante) statt greedy zu akzeptieren und die Kapazität zu klemmen — Box, Zylinder, Kugel und Torus sind damit wasserdicht OHNE Kandidaten-Vervollständigung und OHNE Loch-Füller. Dazu der Akzeptanz-Modus „Zeugen-Umkugel (restricted)“: das Kugelzentrum wird per Newton auf die Fläche geschoben (restricted Delaunay statt kleinster Umkugel).

## 0.6.547
- feat(mesher): Lichen: Chart-Optimierung ist jetzt eine explizite Opt-in-Checkbox statt Default (Betreiber-Wunsch) — standardmäßig läuft das REINE chartlose General-Verfahren (Stern/Umkugel) für ALLE Flächen, auch planare (die sind das Testlabor des Algorithmus; der polierte CDT-Spezialpfad wiegt sonst in falscher Hoffnung). Die Checkbox „Chart-Optimierung“ ersetzt „General tri“ mit umgekehrtem Vorzeichen; Akzeptanz-Modus und Loch-Füller sind jetzt in beiden Modi sichtbar (der Chart-Modus nutzt sie für seine chartlosen Fallback-Flächen).

## 0.6.546
- feat(mesher): Lichen: Defekt-Overlay jetzt auch im Chart-Modus (Default) — offene Watertight-Kanten werden ROT gezeichnet (deckungsgleich mit dem Watertight-Zähler), Fast-Konsens-Kandidaten der Fallback-Flächen orange. Neu in beiden Pfaden: ÜBERLANGE Kanten (Länge > 3× lokales Punkt-Spacing) violett markiert plus Dialog-Zähler — die sichtbare Signatur lokaler Umkugel-Verletzungen („Kanten quer durchs Kugelinnere“, Betreiber-Beobachtung).

## 0.6.545
- perf(render) #853b80c5: SDF-Deferred kompiliert nur noch EIN G-Buffer-Shaderpaar statt eines pro Körper: die Body-ID reist jetzt als Geometrie-Attribut statt als in den Shader gebackene Konstante — der WGSL-Quelltext ist damit dokument-unabhängig, der Pipeline-Cache des Browsers greift beim nächsten Laden
- fix(mesher): Lichen: Meridian-Spalt in Kugeln geschlossen und Gyroid-Flächen auch ohne „General tri“ gemesht — geschlossene Träger mit kleinen Löchern (Kugel in einer Wand) wickeln die volle Chart-Periode, ohne dass ein Loop windet; der Chart hatte dort keine Naht und überbrückte den leeren Meridian mit langen degenerierten Sehnen. Solche Flächen (und Torus/Gyroid/loop-lose) fallen jetzt pro Fläche auf die chartlose Stern/Umkugel-Triangulierung zurück. Dazu Skalen-Korrektur des Flächen-Klassifikators (skalierte Träger lieferten 50× aufgeblasene Distanzen) und dichtere Kanten-Wand (Floor 0,02·h → 0,002·h). Demo-Messstand: chart 131→15 offene Kanten. Nebenbefund: der Halbkugel-Watertight-Test war ein False Positive (testete zwei getrennte Bodies) — jetzt echtes CSG und weiterhin grün.

## 0.6.544
- feat(sketch) #e5482a0a: Split funktioniert jetzt auch auf Splines: die Kurve wird am Klickpunkt in zwei Splines geteilt (Control-Splines exakt per Knoteninsertion), eine geschlossene Spline öffnet sich am Klick
- fix(viewport) #2ca6e21c: Mobile: Ein-Finger-Drag dreht nur noch die Szene und zieht kein Auswahl-Rechteck mehr auf — das Box-Select liegt jetzt auf Doppel-Tipp-und-Ziehen
- fix(mesher): Lichen: das Wachstum emittiert keine Luft-Punkte mehr neben dem Körper — die Projektions-Toleranz wuchs mit dem Wachstumsradius (bei h=10 bis zu 2 mm erlaubter Abstand); jetzt wird jeder Kandidat nachprojiziert und absolut gegen die Oberfläche geprüft (max |f| im Demo: 0,056 → 0,0001 mm). Nebeneffekt: deutlich weniger Loch-Kanten (Demo general 165→30), weil Luft-Punkte die Stern-Nachbarschaften vergifteten.

## 0.6.543
- fix(mesher): Lichen: Browser-Crash der 541er-Version behoben — die Performance-Stoppuhren nutzten std::time::Instant, das auf WASM nicht existiert und sofort panict („time not implemented on this platform“); der Panic vergiftete anschließend die ganze Session (Aliasing-Folgefehler, Auto-Save tot). Timings laufen jetzt nur noch nativ.

## 0.6.542
- fix(mesher): Lichen: Mesher-Panics vergiften nicht mehr die Session — ein Panic in einer WASM-&mut-Methode setzte das Borrow-Flag dauerhaft, jeder Folgeaufruf warf „recursive use of an object … unsafe aliasing“ und der Viewport war tot. lichenMesh fängt Panics jetzt ab und liefert ein leeres Ergebnis (der eigentliche Panic steht weiterhin in der Konsole).

## 0.6.541
- perf(mesher): Lichen 55× schneller bei feinen Settings (Betreiber-Befund „nicht mal tausend Dreiecke, wo andere Millionen schaffen“): fast-kollineare Umkugel-Kandidaten hatten explodierende Umkreisradien und der Punktfreiheits-Scan iterierte Milliarden Zellen (eine Gyroid-Fläche allein 241 s → 0,2 s; Umkreis > Nachbarschafts-Horizont wird jetzt abgelehnt statt gescannt — nie ein lokales Delaunay-Dreieck). Dazu Kanten-Segment-Spatial-Hash für Abstands- und Vorhang-Tests statt Voll-Scan und radius-beschränkte Nachbarsuche im Wachstum.

## 0.6.540
- fix(mesher): Lichen: Fold-Auflösung in der Orientierungs-Propagation — an tangentialen Nähten (Mini-Kugel in der Wand) überlebten Dreiecks-Folds alle seitenbasierten Wächter („Dreiecke durch die kleine Kugel“, 25 Orientierungs-Konflikte bei dichten Settings); Konflikte werden jetzt aufgelöst (pro Fold fliegt deterministisch das Dreieck mit größerem Umkreis — ehrliches Loch statt falscher Geometrie). Dazu Regionen-Zugehörigkeit in der Sichtlinie: der auf die Oberfläche projizierte Sehnen-Mittelpunkt muss auf der eigenen Trägerfläche liegen. Neues Test-Gate fährt exakt die dichten Betreiber-Settings mit.

## 0.6.539
- feat(sketch) #7ad7d649: Spline im Onshape-Look: Tangenten-Kontrollpunkte als weiße Ring-Punkte, Endpunkt-Legs dünn grau; Legs sind keine Snap-Ziele mehr (Endpunkte bleiben snapbar und constrainbar)

## 0.6.538
- perf(render) #40001a3a: SDF-Deferred cached das Shade-Ergebnis (Farbe + Tiefe) in einem Render-Target: reine Overlay-Invalidates (Sketch-Hover/-Drag, Gizmos) blitten den letzten Frame statt die komplette SDF neu zu marchen — Sketch-Interaktion ruckelt nicht mehr

## 0.6.537
- feat(issues): Ticket-Liste aufgeräumt: Datum mit Uhrzeit (ohne „Updated“-Präfix), Category-Spalte und -Filter entfernt (ungenutzt); Uhrzeit auch im Ticket-Detail und in Kommentaren

## 0.6.536
- fix(sketch) #1efd4bcb: Das Trim/Split-Hover-Highlight rendert jetzt auch bei aktivem Edit-Tool (vorher nur im Select-Modus); Arc-Highlights zeichnen nur die Bogen-Spanne statt des vollen Kreises

## 0.6.535
- feat(sketch) #5f0be545: Split funktioniert jetzt auch auf Kreisen (ein Klick setzt einen Vertex — der Kreis wird zum offenen 360°-Arc), Rechtecken und Polygonen (geklickte Kante wird geteilt, Composite zerfällt in Linien)
- feat(sketch) #1efd4bcb: Trim/Split highlighten das Ziel unter dem Cursor sofort orange statt eines hüpfenden Snap-Punkts
- feat(sketch) #90f9b34a: Eine im Outliner selektierte Default-Plane (Top/Front/Right) belegt beim Öffnen des Sketch-Tools die Sketch-Plane vor

## 0.6.534
- fix(mesher): Lichen: Sichtlinien-Toleranz aus dem NORMALEN-KNICK der Endpunkte statt pauschal (Kreisbogen-Sagitta L·θ/8) — die pauschale 0,15·L-Toleranz ließ Sehnen QUER DURCH die Kugelkappe zu (Betreiber-Screenshot „Kanten durch den Körper“), die genauso tief eintauchen wie legitime Sehnen auf der Kugel; parallele Endpunkt-Normalen erlauben jetzt ~0 Eintauchtiefe, kippende genau die Flächen-Sagitta. Demo: 107→59 offene Kanten.
- feat(sketch) #a243c50f #3095b041: Arcs sind jetzt vollwertige Sketch-Entities: Endpunkte als grabbare Grips (wie Onshape), Arc-Bögen sind snapbar und constrainbar (Point-on-Arc)
- feat(sketch) #b26062eb: Gezogene Punkte/Linien-Endpunkte snappen auf Kreise, Arcs und Linien und binden beim Loslassen persistent (Coincident bzw. Point-on-Object) — ein auf einen Kreisrand gezogener Endpunkt bewegt den Kreis jetzt mit
- fix(sketch) #3095b041: Beim Zeichnen gewinnt echte Geometrie (Kreis-/Arc-Rand, Linie) über Horizontal-/Vertikal-Hints im Snap-Radius

## 0.6.533
- fix(mesher): Lichen General-Tri: SDF-Sichtlinien-Prüfung (Konzept §1 Schritt 6) in Stern-Nachbarschaft und Dreiecks-Zentroid — Punkte über konkave Buchten hinweg sind in 3D nah, aber flächenfremd; solche Luft-Nachbarn spannten Speichen quer durch die Luft (Betreiber-Screenshot) und verdrängten im 2D-Delaunay der Sterne die echten Nachbarn (fehlende Dreiecke im Inneren). Demo: 123→107 offene Kanten, Sterne kleiner und schneller.

## 0.6.532
- fix(mesher): Lichen: Wachstum auf Mikro-Feature-Flächen lebt (Betreiber-Screenshot „Grow auf den flachen Seiten tot“) — drei Wurzeln behoben: Ketten-Punkte tragen jetzt ihr ECHTES Nachbar-Spacing statt h (ein 0,2-mm-dichter Randpunkt „beanspruchte“ sonst 10 mm und erklärte ganze Patches für redundant), die Kanten-Wand ist topologisch (Ziel auf eigener Trägerfläche + Ketten-Vorhang-Kreuzungstest statt Distanz-argmin, der an tangentialen Verschneidungen zum Münzwurf wurde), und das Wachstum sieht die Punkte ANDERER Flächen als Konflikt-Quelle (keine 0,04-mm-Doppelblätter an tangentialen Nähten mehr). Demo-Projekt: beste Werte bisher (123 offene Kanten, alle Strukturmetriken 0).

## 0.6.531
- feat(mesher): Lichen: freie Kugeln ALLER Größen (r=1/8/50) jetzt strikt wasserdicht ohne Fallbacks — Dreiecks-Loch-Prädikat in der Kandidaten-Vervollständigung (schließt ein Kandidat alle drei Kanten eines exakten 3er-Lochs, ist er die einzige Manifold-Vervollständigung; die Umkugel darf ihn an Dichte-Übergängen nicht mehr blockieren). Wachstum: Schritt-Halbierung bei Wand-Treffern (Sub-h-Features) mit Kaskaden-Bremse und symmetrischem Konflikt-Test gegen das Spacing des nächsten Bestandspunkts.

## 0.6.530
- feat(mesher): Lichen: Zylinder (Mantel + Kappen) im General-Pfad WASSERDICHT (χ=2, aktiver Test). Krümmung fürs Wachstum kommt jetzt von der glatten TRÄGERFLÄCHE statt vom Body-Feld — das Body-∇f maß an B-Rep-Kanten den Knick (Mikro-Spacing-Bänder) bzw. bei kleinem Fenster f32-Rauschen als Krümmung; die ~15-fache Überdichte ist weg (Box 12041→486 Dreiecke bei besserer Qualität, Mesher ~5× schneller). Rand-Seeds vererben ihr echtes Ketten-Spacing mit ≤×1,5-Grading; Fold-Elimination arbeitet seitenweise (auch non-manifolde Mehrfachbelegungen).

## 0.6.529
- fix(mesher): Lichen: Wurzel-Bug im Gradientenfeld behoben — der Dual-Gradient ist exakt AUF planaren Flächen null, wodurch alle Wachstums-Punkte die Default-Normale [0,0,1] bekamen: 4 von 6 Box-Flächen blieben komplett ungemesht, Krümmungsschätzung und Stern-Projektion waren vergiftet. Jetzt FD-Fallback (robust_grad), Stern-Normalen exakt aus der Trägerfläche, §1.5-Konflikt-Rebalancing beim Wachstum (Kandidaten werden ausbalanciert statt verworfen) und Gegenseiten-Pflicht innerer Kanten (Folds raus). Alle Testkörper jetzt voll gemesht mit 0 non-manifold, 0 degeneriert, 0 Orientierungs-Konflikten.

## 0.6.528
- feat(mesher): Lichen General-Tri: Umkugel-verifizierte Kandidaten-Vervollständigung — offene Kanten werden geschlossen, wenn das fehlende Delaunay-Dreieck die Umkugel-Prüfung besteht (Prädikat statt Heuristik, Teil des Basis-Algorithmus; deterministisch: kleinster Umkreisradius, Fixpunkt-Iteration mit Kapazitäts-, Ketten- und Degenerations-Wächtern). Box 128→98, Zylinder 434→345 offene Kanten (ohne Loch-Füller); der Füller ist jetzt kapazitäts-bewusst und kann keine non-manifolden Kanten mehr erzeugen.

## 0.6.527
- feat(mesher): Lichen General-Tri: freie Kugeln sind jetzt WASSERDICHT (χ=2) — ohne Rand-Topologie, ohne Loch-Füller: Feld-Streuung besät Flächen ohne Rand-Seeds (Gitter + skalen-korrekte Newton-Projektion), und die Umkugel-Akzeptanz trägt den doppelt gekrümmten Fall. Neuer Akzeptanz-Modus „Auto“ (Default): pro Fläche nach Trägertyp — doppelt gekrümmt → Umkugel, planar/abwickelbar → Stern-Konsens (Demo: 165→153 offene Kanten). Loch-Füller im Dialog abschaltbar, um den Basis-Algorithmus pur zu messen.

## 0.6.526
- feat(mesher): Lichen: verwaiste Branch-Flächen (vom Weave gedroppte Trim-Nähte, DL-71) werden rekonstruiert — Träger aus der Branch-Halfspace-Map, Rand-Loops durch Ketten-Verkettung, kettenlose Flächen per Feld-Streuung (Gitter + Newton-Projektion) besät. Loch-Loop-Walker biegt an Verzweigungen jetzt geometrisch ab (schärfste Linkskurve in der Tangentialebene) statt beliebig.

## 0.6.525
- fix(mesher): Lichen: Demo-Projekt-Befunde behoben — beide Triangulierungs-Pfade liefern jetzt 0 non-manifolde Kanten und global konsistente Orientierung. Ketten-Kanten-Einzelnutzung pro Fläche (Rand-Folds/Sliver entfernt, gilt auch nach dem Loch-Füller), fremde Ketten-Kanten als globale Wände, Ring-Naht-Kohärenz beim Kanten-Resampling (U(p)-Überdeckungslücke am Nahtstoß geschlossener Ringe), kreuzende Chart-Constraints crashen nicht mehr (Diagnose statt spade-Panic), Orientierungs-Propagation über Kanten-Nachbarschaft.

## 0.6.524
- feat(mesher): Lichen-Dialog zeigt Qualitäts-Kennzahlen des finalen Netzes: Manifold und Watertight als ✓/✗ mit Zählern, degenerierte Dreiecke, Komponenten, Euler-Charakteristik und Orientierungs-Verstöße (kernel-berechnet, beide Triangulierungs-Pfade). Das Betreiber-Demo-Projekt (Kugel∩Gyroid, Bohrung, Mikro-Features) ist jetzt Unit-Test-Fixture mit Qualitäts-Regressions-Gates; flächen-übergreifende Dreiecks-Duplikate des General-Pfads werden konsolidiert.

## 0.6.523
- feat(mesher): Lichen General-Tri im Dialog schaltbar: „General tri“-Checkbox plus Akzeptanz-Auswahl (Stern-Konsens / Umkugel mit deterministischem Fächer-Tie-Break / Hybrid). Diagnose-Overlay zeigt offene Loch-Kanten rot und Fast-Konsens-Kandidaten orange; der Cocone-Öffnungswinkel ist an den globalen Krümmungswinkel gekoppelt (Zylinder: 434 statt ~1000 Loch-Kanten).

## 0.6.522
- fix(mesher) #28a95b4e #0e9e8f40: Lichen: Auf geschlossenen Trägern (Kugel) entscheidet jetzt das Body-Feld statt der Loop-Parität, welche Chart-Region vermascht wird. Zwei Loops teilen eine Kugel in drei Regionen — die even-odd-Parität füllte den VERGRABENEN Annulus zwischen Naht-Ring und Bohrungs-Loop (innere Flächen), während die lebendige Kappe und das Bohrungs-Stück leer blieben. Der nicht-windende Chart-Fall prüft Kandidaten und Dreiecks-Zentroide jetzt per Liveness (Träger-Punkt auf dem Body): Kappe vermascht, vergrabene Dreiecke weg.

## 0.6.521
- fix(mesher) #e6b21b96: Lichen: Die Triangulierung nutzt jetzt die Grow-Punkte. Die CDT-Rand-Clearance war fix 0.25·h — bei krümmungsadaptiv dichtem Wachstum (Spacing ≪ h) galt damit jeder gewachsene Punkt als „zu nah am Rand“ und wurde verworfen; die Dreiecke überspannten die Flächen nur mit Rand-Ketten. Jetzt: Clearance pro Fläche aus dem lokalen Punkt-Spacing, plus eine relative Regel im CDT (kein freier Punkt näher als 10% der jeweiligen Constraint-Segmentlänge) gegen Sliver-Dreiecke, wenn dichte Punkte auf grobe Randsegmente treffen.

## 0.6.520
- fix(render) #d4fc002a #65cc6903: Der Z-Offset im „Visible only“-Modus wirkt jetzt wirklich — auf B-Rep-Kanten UND Lichen-Overlays. Ursache: polygonOffsetUnits zählt in Schritten des kleinsten Depth-Buffer-Deltas (~2⁻²³), die analytisch geschriebene SDF-Tiefe weicht von der Raster-Tiefe der Kanten aber um ~1e-5 ab — der bisherige Bias war ~100× zu klein, Kanten auf Flächen (z.B. der Ring um die Kugel) verloren den Tiefen-Test. Die Skala ist jetzt headful vermessen (Schwelle zwischen 50 und 200 Units) und auf Units = −200·Wert gesetzt: der Default 2 zeigt den Kugel-Ring vollständig, mit doppelter Marge.

## 0.6.519
- fix(mobile): Auch die Users- und Invites-Tabellen pannen jetzt horizontal, wenn sie nicht in die Fensterbreite passen (gleiches Muster wie die Issues-Tabelle): Scroll-Wrapper + Mindestbreite statt gequetschter Spalten.

## 0.6.518
- fix(mobile): Die Issues-Tabelle ist jetzt scrollbar, wenn sie nicht ins Fenster passt: Auf schmalen Screens pannt sie horizontal (die Spalten behalten ihre Mindestbreite, statt sich zu überlagern — Title kollabierte auf 0 und legte sich über Category); vertikal scrollt die Übersicht wie gehabt im eigenen Container.

## 0.6.517
- fix(mobile): Das Burger-Menü öffnet auf Phone-Breite nicht mehr aus dem Screen heraus: Es ist dort kein am Burger verankertes Popover mehr, sondern ein fixes Panel unter der Topbar über die volle Breite — scrollbar, wenn es höher als das Fenster ist; Submenus klappen inline auf (Akkordeon) statt seitlich hinauszulaufen. Die Debug-Dialoge (Mesher, Lichen/PASM, SDF-Plane) sind ebenfalls nie höher als das Fenster und scrollen ihren Inhalt.

## 0.6.516
- fix(mobile): Mobile Topbar entschlackt: Auf Phone-Breite (≤768px) bleiben nur Zurück-Pfeil, Burger-Menü, Projekt-Titel und User-Menü — Marke, Versions-Badge, Feature-/Dreiecks-/Größen-Anzeige und die Alarm-Glocke sind ausgeblendet; Fehlermeldungen bleiben sichtbar.
- fix(mobile): Die Seite selbst zoomt und scrollt auf Mobilgeräten nicht mehr: touch-action pan-x/pan-y verbietet dem Browser Pinch- und Doppeltipp-Zoom der Seite (Pinch gehört der 3D-Szene), overscroll-behavior stoppt Pull-to-Refresh/Rubber-Banding, und overflow:hidden auf html/body verhindert jedes Seiten-Scrollen — die App ist eine Single-Page-Fullscreen-Anwendung.

## 0.6.515
- feat(render): Der Z-Puffer-Offset ist jetzt direkt einstellbar: Bei „B-Rep edges: Visible only“ erscheint in den Settings das Feld „Z-Offset (Depth-Bias)“ (Default 2, 0 = aus). Der Wert steuert, wie stark Kanten und Debug-Overlays im Tiefen-Test zur Kamera gezogen werden (polygonOffset: Factor = −Wert, Units = −2·Wert) — vorher war der Bias fest auf 2 kodiert. Wirkt live auf B-Rep-Kanten und Lichen-Overlays.

## 0.6.514
- feat(render): Debug-Overlays verdeckungs-korrekt: Der Renderer-Modus „B-Rep edges: Visible only“ wirkt jetzt auch auf die Lichen-Overlays (Punkte, U(p)-Kreise, Normalen, Wireframe) — sie depth-testen gegen die Körper-Tiefe und ziehen ihre Fragmente per Depth-Bias (WebGPU depthBias über polygonOffset; die Fat-Lines und Punkt-Boxen rasterisieren Dreiecke) minimal zur Kamera, sodass Geometrie exakt auf der Fläche nicht z-fightet. Rückseitige Punkte/Kreise verschwinden hinter dem Körper; X-Ray bleibt der Default. Reiner Tiefen-Effekt — kein Pixel bewegt sich.
- feat(issues): Die Issue-Liste ist jetzt paginiert mit einstellbarer Seitengröße (10/25/50/100 oder „Alle“) — die gewählte Größe bleibt über Sitzungen erhalten. Unten zeigt eine Leiste die Gesamtzahl, den Seitengröße-Selektor und die Vor/Zurück-Navigation mit „Seite X von Y“.

## 0.6.513
- fix(tickets): Report-Submit scheiterte mit „signal timed out“: der 30-s-Timeout war für das IssueVault-Backend zu knapp — der Server lädt Screenshot, Anhänge und Projektdatei einzeln zum entfernten Store hoch. Timeout jetzt 120 s. Außerdem zeigt die fielddraft-a-Dev-Instanz nach Server-Rebuild wieder die projekt-weiten resolved Issues aller Installationen (der laufende Container war älter als das Sichtbarkeits-Feature).

## 0.6.512
- fix(mesher) #ee07b6f1: Lichen: Grow findet wieder Innenpunkte auf Teilen, die kleiner als h sind. Der Wachstumsradius war hart auf mindestens 0.2·h geklemmt (bei h=10 also 2 mm) — auf dem 6×8×6-mm-Beispielteil passte damit genau ein Innenpunkt, und die Krümmungs-Verfeinerung (R_curv ≈ 0.3 mm auf der Gyroid-Wand) kam nie zum Zug. Der Floor ist jetzt der Min. Kantenabstand (konsistent mit der Kanten-Abtastung): auf dem Ticket-Modell wachsen statt 1 nun ~1350 Innenpunkte, krümmungsadaptiv verteilt.

## 0.6.511
- feat(mesher) #2e7eab20: Lichen: Mikro-Features werden kollabiert statt umtanzt („andersrum“-Sicht auf die U(p)-Lücken). Drei Mechanismen: (1) B-Rep-Ecken näher beieinander als der Min. Kantenabstand teilen sich einen Mesh-Vertex (Cluster-Repräsentant) — solche Häufungen entstehen bei streifenden Verschneidungen; (2) Mikro-Kanten innerhalb eines Clusters und Mikro-Ringe (Umfang < 4·Min-Abstand) verschwinden ganz aus dem Mesh (Sub-Resolution-Topologie); (3) die Kanten-Abtastung tilgt Trailing-Remnants — das beliebig kleine Rest-Segment vor dem Kettenende (die eigentliche Quelle der µm-kleinen U(Ecke)-Kreise). Ecken-Umgebungen sind damit wieder ≥ Min-Abstand groß, und die Punktzahl sinkt statt zu explodieren (Ticket-Modell: 191 → 165 Punkte bei weiterhin lückenloser Überdeckung).

## 0.6.510
- fix(tickets) #95d0a7fa: Report-Dialog robuster gegen „nicht eingeloggt“-Fehler trotz sichtbarem Login: Bei einem 401 beim Absenden validiert die App die Session jetzt sofort neu (/api/me), sodass ein veraltetes „eingeloggt“-Badge ehrlich wird statt weiter zu lügen; der Submit hat außerdem einen 30-s-Timeout (kein ewiges „Sending…“ mehr bei totem Backend). Serverseitig werden transiente DB-Fehler bei der Session-Prüfung jetzt geloggt statt still als „nicht eingeloggt“ maskiert — sporadische 401 bei gültiger Session sind damit diagnostizierbar.

## 0.6.509
- fix(mesher) #2e7eab20: Lichen: Überdeckung schlägt Mindestabstand. An Mikro-Kanten (Ecken-Paare feiner als der Min. Kantenabstand, z.B. 0.017 mm) blieb neben dem winzigen Ecken-U ein 0.3-mm-Segment ungesplittet — Lücke zwischen den U-Kreisen. Das Grading prüft jetzt zusätzlich die harte Überdeckungs-Bedingung min(dl,d)+min(d,dr) ≥ d ohne Mindestabstands-Floor: Geometrie, die feiner als der Mindestabstand ist, erzwingt lokal feinere Punkte. Die Überdeckungs-Invariante läuft als Assertion in jeder Pipeline-Testfahrt mit.

## 0.6.508
- fix(mesher) #cec70614: Lichen: Das Kanten-Grading kennt jetzt Ecken-Rand-Bedingungen. Bisher wurde jede Kette isoliert graduiert — eine gerade Kante startete an der Ecke mit ~h, während U(Ecke) vom dichtesten Nachbarn stammt: die U-Kreise von Ecke und erstem Kettenpunkt berührten sich nicht (Lücke). Jetzt sammelt ein Vorlauf die End-Abstände aller an einer Ecke mündenden Ketten; das kleinste wird die Grading-Rand-Bedingung der Nachbarkanten — jede Kante wächst von der Ecke geometrisch (Faktor ≤1.5) auf h.

## 0.6.507
- fix(app) #24d81476: Release-Notes-Ansicht repariert: zwei Einträge (0.6.493/0.6.494) enthielten ein gerades Anführungszeichen im Text, das den String beendete — die Release-Notes-Datei war dadurch syntaktisch kaputt. Außerdem warnt der Tooltip von „Min. Kantenabstand“ im Lichen-Dialog jetzt explizit, dass Werte in der Größenordnung von h die Krümmungs-Verdichtung praktisch abschalten (sinnvoll: 0.01–0.1 mm).

## 0.6.506
- fix(mesher) #e1a10bcd: Lichen: Kanten-Abtastung mit Grading — benachbarte Segmentlängen wachsen höchstens um Faktor 1.5, statt an einer Ecken-Häufung abrupt von 0.01 mm auf h zu springen. Da U(p) eines Kettenpunkts der Abstand zum nächsten Nachbarn ist, überlappen sich die U(p)-Kreise benachbarter Punkte damit überall (U_a+U_b ≥ 2d/1.5 > d): Kanten sind vollständig von einander schneidenden Kreisen überdeckt, und im U-Kreis einer Ecke liegen ihre Ketten-Nachbarn.

## 0.6.505
- feat(mesher) #6d3f4a7f: Lichen: Neuer Parameter „Min. Kantenabstand (mm)“ (Default 0.01) — die winkelgetriebene Kanten-Verdichtung setzt keine Punkte näher als diesen Abstand, sehr kleine Kreise bekommen also nicht mehr die volle Winkel-Punktzahl. Geschlossene Kanten behalten unabhängig davon mindestens 4 Punkte, damit sie nicht entarten.
- fix(mesher) #0d94273b: Lichen: U(p) ist für Kanten- und Eckpunkte jetzt durch den Abstand zum nächsten Ketten-Nachbarn begrenzt (statt pauschal h) — der Anzeigekreis reicht nie über den Nachbarpunkt hinaus. Bei geschlossenen Ringen zählt auch das Schluss-Segment (das zudem jetzt in die R_BRep-Distanz eingeht).

## 0.6.504
- fix(account) #14a08041: Der „Keep your local work?“-Import-Banner nach dem Login erscheint nicht mehr in der lokalen Dev-Umgebung (Vite-Dev-Server) — dort ist das Gast-Projekt fast immer bewusste lokale Entwicklungsarbeit.

## 0.6.503
- fix(mesher): Lichen: Der Krümmungswinkel wirkt jetzt wirklich auf die Kanten-Dichte. Das Kanten-Resampling tastete die Kante erst uniform mit Schritt 0.15·h ab und dünnte dann per Winkel aus — kleine, stark gekrümmte Kanten (z.B. der 6-mm-Schnittkreis einer skalierten Kugel bei h=10) kollabierten dabei auf eine Handvoll Punkte, bevor der Winkel-Test überhaupt messen konnte. Jetzt wird der Knickwinkel direkt auf der krümmungsadaptiven B-Rep-Polyline akkumuliert; h unterteilt nur noch gerade Strecken.

## 0.6.502
- feat(mesher) #10df1b20: Lichen-Mesher M4: gekrümmte analytische Flächen (Zylinder, Kugel, Kegel) werden jetzt trianguliert — über den periodischen Chart mit Naht-Schnitt (geteilte Steiner-Indizes, unsichtbare Naht) und Pol-Kappen via Ring + Fächer, dieselbe Mechanik wie der Hybrid-PASM-Export. Ein Zylinder oder eine Halbkugel vermascht damit als geschlossenes, wasserdichtes 2-Manifold. Torus (doppelt periodisch) und Gyroid-Flächen (chartlos) bleiben vorerst Punktwolke.

## 0.6.501
- fix(brep) #20260710-163918-a26a: Skalierte Kugeln bekommen wieder vollständige BRep-Kanten: Die exakte Schnittkreis-Berechnung (Kugel×Ebene, Kugel×Kugel) ignorierte die Scale-Transform des Features — eine ×0.02 skalierte Kugel wurde mit ihrem unskalierten Radius (50 statt 1) geschnitten, der Seam an Box-Flächen fiel dadurch stillschweigend weg und die betroffene Fläche war nicht mehr komplett von Kanten umschlossen. Nicht-uniform skalierte Kugeln (Ellipsoide) nehmen jetzt den Parameterraum-Scan statt der geschlossenen Form.

## 0.6.500
- feat(issues): Die Issues-Liste zeigt jetzt erledigte (resolved) Tickets ALLER Installationen/Agenten des Projekts — resolved Tickets sind geteilt (sie wandern ohnehin nach main). Alle anderen Status bleiben auf die eigene Installation beschränkt. Serverseitig (IssueVault-Backend): resolved wird projekt-weit gelesen (scope:project), der Rest scope:own, zusammengeführt und dedupliziert. Mit dem issuevault-Agenten abgestimmt (der installations-gebundene Token darf projekt-weit lesen).

## 0.6.499
- feat(projects): Lokale Projekte speichern jetzt manuell statt kontinuierlich: die geöffnete .fdraft-Datei wird nur noch beim ausdrücklichen Speichern (Ctrl+S bzw. „Save“ im Menü) auf die Platte geschrieben — auf Chromium still in dieselbe Datei, sonst als Download. Der Arbeitsstand läuft weiterhin laufend in den lokalen Browser-Speicher (kein Datenverlust bei Reload), aber die Datei bleibt so lange unverändert, bis du speicherst. Das Menü zeigt den Dateinamen und einen Punkt, wenn es ungespeicherte Änderungen gibt; beim Schließen mit ungespeicherten lokalen Änderungen warnt der Browser. Damit ist Speichern in allen Browsern gleich (kein automatisches Mitschreiben der Datei mehr).

## 0.6.498
- feat(admin): Blob-Garbage-Collection für verwaiste content-addressed Assets — bewusst sicher und opt-in: der Admin-Endpoint POST /api/admin/blobs/gc läuft standardmäßig als Dry-Run (meldet, löscht nichts) und braucht dry_run=false zum echten Löschen. Ein Blob gilt nur als verwaist, wenn ihn KEINE Projekt-, Snapshot-, Thumbnail-, Mesh-Job- oder Ticket-Referenz und keine eingebettete Asset-Referenz mehr nennt (alle Quellen vereinigt, fail-closed bei jedem Fehler), und er älter als eine Grace-Period (Standard 48 h) ist. Zusätzlich räumt der Client verwaiste Blobs im lokalen IndexedDB-Store nach dem Laden auf.

## 0.6.497
- perf(projects): Die .fdraft-Projektdatei ist jetzt content-addressed und selbst-enthalten: importierte Meshes und hochgeladene Fonts liegen als hash-benannte Blob-Einträge (assets/…) im ZIP, das Projekt-Dokument (project.json) bleibt schlank und trägt nur die Hashes. Vorher waren Meshes als riesige JSON-Zahl-Arrays direkt im Dokument des ZIP eingebettet. Innerhalb einer Datei werden identische Assets dedupliziert. Ältere .fdraft-Dateien (mit eingebetteten Meshes) laden weiterhin unverändert.

## 0.6.496
- feat(projects) #14cff61: Vorschaubilder gelten jetzt für ALLE Projekte — Cloud UND lokale. Erzeugt werden sie beim Verlassen der Editor-Ansicht (und einmalig bei Neu-Anlage), nicht mehr bei jedem Speichern: das spart GPU/Upload und aktualisiert die Vorschau genau dann, wenn die Galerie sie zeigt. Cloud-Projekte laden ihr Thumbnail wie bisher vom Server; lokale Projekte speichern es in IndexedDB (pro Projekt) und die Übersicht zeigt es auf den „Auf diesem Gerät“-Karten. Weiterhin gedrosselt (höchstens 1×/30 s).

## 0.6.495
- feat(cloud): Cloud-Projekte speichern jetzt automatisch zum Server (Phase 3): periodisch (einstellbares Intervall, Standard 1 Minute — 0 schaltet es aus), beim Verlassen der Editor-Ansicht und beim Schließen/Neuladen des Tabs. Jeder Trigger feuert nur, wenn es ungespeicherte Änderungen gibt (kein unnötiger Traffic). Dank des schlanken Cloud-Dokuments (Phase 2) schickt der Sync nur das kleine Feature-Dokument, nicht die Mesh-Bytes. Das neue Intervall stellt man unter Einstellungen ▸ Cloud ein. Bisher wurde ein Cloud-Projekt nur beim Export gespeichert.

## 0.6.494
- fix(cloud): Veröffentlichte Projekte mit importierten Meshes oder hochgeladenen Fonts laden jetzt korrekt im öffentlichen Viewer. Seit dem schlanken Cloud-Speichern (Phase 2) liegen Assets als owner-authentifizierte Blobs — der unauthentifizierte Viewer hätte sie nicht laden können. Beim Publizieren wird jetzt das volle, selbst-enthaltene Dokument (Assets eingebettet) als Snapshot gespeichert, sodass der Viewer ohne Blob-Zugriff auskommt. Asset-freie Projekte sind unverändert.

## 0.6.493
- perf(cloud): Cloud-Speichern ist schlank geworden (Phase 2): Server-gebundene Projekte legen das Dokument klein ab (nur Features + Asset-Hashes); importierte Meshes und hochgeladene Fonts werden als content-addressed Blobs separat hochgeladen — mit Dedup (ein Blob wird nur einmal übertragen und gespeichert, auch über Projekte hinweg). Der Sync schickt beim Speichern nur noch das kleine Dokument statt megabyte-großer eingebetteter Meshes. Neue Server-Endpoints PUT/GET /api/blobs/{sha256} + /api/blobs/missing (content-adressiert, owner-authentifiziert). Bestehende (volle) Cloud-Dokumente laden unverändert und werden beim nächsten Speichern automatisch umgestellt. IndexedDB (lokal) und Server teilen sich denselben Externalisier-Code.

## 0.6.492
- fix(projects): Hochgeladene Fonts (Sketch-Text) überstehen jetzt einen Reload. Bisher lagen die Font-Bytes nur in einem flüchtigen In-Memory-Cache — nach einem Neuladen aus dem lokalen Arbeits-Store blieb der Text mit hochgeladener Schrift leer. Jetzt werden Fonts wie Meshes als content-addressed Blobs (sha256) im Arbeits-Store persistiert und beim Laden wiederhergestellt. Damit behandeln Server (P2) und `.fdraft` (P3) Meshes und Fonts einheitlich.

## 0.6.491
- perf(projects): Projekt-Persistenz: importierte Meshes werden nicht mehr roh (als riesige JSON-Zahl-Arrays) ins Projekt-Dokument geschrieben, sondern als content-addressed Blobs (sha256) ausgelagert; das Dokument bleibt dadurch klein. Der lokale Arbeits-Store wechselt von localStorage auf IndexedDB — kein 5–10-MB-Limit mehr (mesh-lastige Projekte sprengten bisher die localStorage-Quota, wodurch der Autosave still fehlschlug) und keine synchronen Schreib-Hänger. Bestehende Projekte werden beim ersten Laden einmalig migriert. Grundlage (Phase 1) für den kommenden Server-Blob-Store und das schlanke Cloud-Speichern.

## 0.6.490
- feat(projects) #14cff61: Projekt-Vorschaubilder: server-gebundene Projekte bekommen jetzt automatisch ein Thumbnail, das in der Projektübersicht auf der Karte erscheint. Erzeugt wird es beim Speichern zum Server über einen eigenen, unsichtbaren Offscreen-WebGL-Renderer (mit preserveDrawingBuffer) — bewusst getrennt vom Live-WebGPU-Renderer, dessen Canvas sich nicht zuverlässig auslesen ließ (leer/eingefroren). Die kleine Szene wird frisch aus den Kernel-Mesh-Daten gebaut (dieselben Dreiecke wie im mesh/webgl-Modus — Booleans und SDF bereits aufgelöst), flach schattiert und aus fixem Iso-Blick gerahmt. Gedrosselt (höchstens 1×/30 s) und best-effort — ein fehlendes Thumbnail schadet nie.

## 0.6.489
- feat(mesher): Debug-Mesher Lichen (Menü Debug ▸ Mesher: Lichen…): direkter SDF+BRep-Oberflächen-Mesher. Punkte wachsen als hexagonale Waben-Front pro BRep-Fläche über die Oberfläche — mit einer HARTEN Kanten-Wand: kein Punkt überspringt je eine BRep-Kante auf eine Nachbarfläche oder ins Leere, und der U(p)-Anzeigekreis bleibt stets innerhalb der Fläche. Die Triangulierung erfolgt pro Fläche über den robusten Constrained-Delaunay (spade), mit flächenübergreifend geteilten Rand-Ketten → das Netz einer Box ist ein geschlossenes, wasserdichtes 2-Manifold (kein Loch, kein Dreieck über eine Kante). Gekrümmte Flächen (Zylinder/Kugel/Torus) folgen als nächster Schritt. Das Debug-Overlay zeichnet die Punkte an ihrer echten Position (kein Z-Offset/-Versatz); die Sichtbarkeit regelt der x-ray-Layer wie beim PASM-Mesher-Overlay.

## 0.6.488
- feat(issues) #066230d9: Issues haben jetzt eine kurze Referenz „#N“ statt der langen UUID — angezeigt in der Liste (eigene, sortierbare Spalte) und im Detail-Kopf. Die interne ID bleibt der Speicher-/Routing-Schlüssel; die Nummer ist die menschen- und agentenfreundliche Kurzform (im lokalen Store fortlaufend, in Prod vom Issue-Server pro Projekt vergeben — mit dem issuevault-Agenten abgestimmt). Außerdem kann der Autor den State seines Tickets selbst steuern: schließen, ein geschlossenes wieder öffnen, und neu „Backlog“ = notiert, aber vom Agenten vorerst nicht bearbeiten (parken/entparken). Serverseitig als sichere Teilmenge der State-Machine (nur der Autor bzw. Admins), inkl. Enum-Migration und Backfill der Bestands-Nummern.

## 0.6.487
- fix(custom-sdf) #c50f3e15: Custom SDF ist jetzt editierbar. Zwei Probleme behoben: das Feature meldete intern die Art „unknown“, weshalb das Detail-Panel „Kein Feature selektiert“ zeigte statt eines Dialogs — die Formel blieb die Default-Kugel. Jetzt öffnet die Auswahl des Custom-SDF-Features einen Formel-Editor: ein mehrzeiliges Monospace-Feld mit Vokabular-Hinweis (Variablen x, y, z · Funktionen sin cos tan sqrt … · Konstanten pi, tau · Potenz ^). Syntaxfehler in der Formel werden als Feature-Fehler angezeigt. So kann man z.B. eine Gyroid-Formel eingeben: sin(x)·cos(y) + sin(y)·cos(z) + sin(z)·cos(x).

## 0.6.486
- fix(cgal) #335d153: CGAL-Export: scharfe, gekrümmte B-Rep-Kanten (z.B. ein Sichelrand) fransen nicht mehr aus. Zwei Ursachen behoben: die Schutz-Polylinien der Feature-Kanten wurden zu grob vereinfacht (0,1·cell → eine gekrümmte Kante wurde zu wenigen langen Geraden, deren Silhouette zackte) — jetzt 0,02·cell, also kurvenerhaltend; und die neue edge_distance-Kriterium (CGAL 6.0) begrenzt den Sehnenfehler der vermaschten Kante wie facet_distance für die Fläche. Ergebnis: der obere Sichelbogen wird sauber, das Netz bleibt wasserdicht/2-manifold. Sehr dünne Spitzen brauchen weiterhin eine kleinere Zellgröße.

## 0.6.485
- feat(issues) #a7b63b6: Die Issues-Liste hat jetzt klickbare, sortierbare Spaltenüberschriften (State, Title, Category, Updated). Ein Klick sortiert nach der Spalte, ein weiterer kehrt die Richtung um; die aktive Spalte zeigt einen Pfeil. Text-Spalten starten A→Z, „Updated“ neueste zuerst.

## 0.6.484
- feat(kernel): Fillet-Flächen weben jetzt sauber (c-M2): Die tangentialen Übergangsnähte eines Fillets (wo die Rundung ihre Trägerflächen G1-tangential verlässt) kann der numerische Tracer prinzipbedingt nicht abtasten — sie werden nun symbolisch als exakte Kurven eingesetzt. Für einen Fillet zwischen Zylinder und ebener Fläche (ein exakter Torus-Ring) sind das zwei koaxiale Kreise; das Ergebnis ist ein topologisch geschlossener, manifolder Körper mit einer echten Fillet-Fläche. Die verbleibenden Fälle (gerade Kanten mit Auslauf-Ecken) folgen später.

## 0.6.483
- feat(kernel): Neues Feature „Round All Edges“: rundet in einem Schritt jede Kante global mit einem Radius. Als Feld-Operator (SDF) kann die Rundung den Körper nie in einen ungültigen Zustand bringen, und zwischen ebenen Flächen ist sie ein exakter Kreisbogen. Gerundet werden die Kanten, an denen Features aneinanderstoßen; die Eigenkanten eines einzelnen Primitivs bleiben vorerst außen vor.

## 0.6.482
- fix(kernel): Fillets sind jetzt exakt kreisförmig: Der Rundungs-Übergang nutzt die zirkuläre smin-Form (echter Kreisbogen vom Radius r zwischen zwei ebenen Flächen) statt der bisherigen kubischen Polynom-Näherung — ein gefilletes Kantenprofil hat damit einen messbaren Radius. Nebenbei die zwei bisher auseinanderlaufenden smin-Familien vereinheitlicht (CPU-Kernel war kubisch, Tape/GPU/CGAL-Mesher quadratisch — beide behaupteten fälschlich „cubic“) und einen fehlenden Nulldivisions-Schutz (blend_radius 0) in der Tape-Familie ergänzt.

## 0.6.481
- feat(kernel): Fillets als eigenständige, referenzierbare Fläche (Fundament): Ein kantenselektiver Fillet erzeugt jetzt einen prozeduralen Blend-Träger mit stabiler, provenienz-basierter Identität (Fillet-Feature + die zwei Wand-Kanten) statt einer namenlosen Ersatzfläche. Das abgeleitete B-Rep folgt der echten Rundung statt der scharfen Phantomkante, und die Blend-Fläche ist damit auswählbar und benennbar — die Grundlage dafür, Fillet-Geometrie später sauber weiterzuverwenden (Projektion in Sketches u.a.). Auswertung konsistent über alle Backends (Dual-Zahlen, Tape/GPU, CGAL-Mesher).

## 0.6.480
- feat(kernel): Tangentialkontakt, Teil 2 — die konstruierten Profil-G1-Fälle: Gestapelte Revolves, deren Profile sich einen Endpunkt mit gleicher Tangente teilen (der Vasen-Stapel: Zylinder-Topf + Bogen-Kuppel), bekommen ihren Übergangs-Kreis jetzt symbolisch aus den exakten 2D-Sketchdaten (Profil-Kettenenden + Endtangenten) — RevolvedProfile-Wände paaren mit RevolvedProfile, Zylinder, Kegel und achsnormalen Ebenen.

## 0.6.479
- feat(kernel): Tangentialkontakt (Kontaktordnung 1) für koaxiale Rotationsflächen: Die Koinzidenz-Registry erkennt Tangentialkreise symbolisch auf Konstruktionsdaten (Kugel↔Zylinder, Kugel↔Kegel, Ebene↔Torus, Zylinder↔Torus, Kugel↔Torus) und der Weave materialisiert den EXAKTEN Kreis statt zu tracen — ein kugelgekappter Zylinder (abgerundeter Pfosten) webt jetzt sauber (vorher 241 Verletzungen auf der Octree-Route), ebenso die abgerundete Kegelspitze. Ein Kontributor-Rollenwechsel-Gate unterscheidet echte Nähte von Innen-Berührungen (einbeschriebene Kugel = keine Kante).
- fix(kernel): Aufwärts breiter werdende Kegelstümpfe (r_top > r_bottom) verloren auf der Seed-Route ALLE Kanten: primitive_halfspaces erzeugte einen negativen Halbwinkel, der das Steigungs-Vorzeichen der Kegel-SDF drehte — jede Rim-Projektion lief ins Leere. Der Halbwinkel ist jetzt immer positiv (die Öffnungsrichtung trägt der Apex-Offset).

## 0.6.478
- fix(kernel): Ring-Windung ohne intrinsische Autorität repariert: Punktur-Ringe, die um die Röhre eines Torus (oder durch eine Kugel) winden, haben die Fläche auf BEIDEN Seiten — uv-Shoelace und Flächenseite-Regel können sie nicht orientieren und drehten sie bisher gegen ihre schon korrekten Partner-Faces. Jetzt enthalten sich diese Ringe (und alle Ringe tessellierter Faces) beim intrinsischen Flip und übernehmen per Mehrheits-Parität das Komplement ihrer autoritativen Partner — Box ∪ Torus webt orientierungs-sauber (0 Manifold-Verletzungen) auf Octree- UND Hybrid-Route.

## 0.6.477
- feat(thicken): Thicken funktioniert jetzt auch auf Flächen von zusammengesetzten (Boolean-)Features — etwa FieldSkript-Körpern oder verketteten Thicken-Wänden: Der Träger der gepickten Fläche wird per Proben an den Face-Loop-Samples aus dem Boolean-Baum isoliert (nur DIE Fläche wächst, nicht jede Oberfläche des Booleans). Shell-/Offset-Owner ohne eindeutigen geschlossenen Träger melden weiterhin einen klaren Fehler.
- fix(extrude): Gekrümmte Fläche als Extrude-Profil: früher, präziser Fehler mit Handlungsanweisung (Thicken nutzen) statt des späten generischen Profil-Fehlers. Design-Entscheidung im Kernel-Konzept dokumentiert: Extrude bleibt planar, Normal-Wachstum gekrümmter Flächen ist Thicken.

## 0.6.476
- feat(kernel): Hybrid-Weave (Seed-Weaver Stufe C): Körper, deren Übergangs-Seams analytisch nicht auflösbar sind (z. B. Box ∪ Torus), bekommen jetzt ein VOLLSTÄNDIGES B-Rep statt einer offenen Schale — die parametrisch exakten Kanten der Operanden bleiben erhalten, nur die übersprungenen Seam-Paare werden aus dem Octree-Wireframe nachgefüllt (Endpunkte in die Seed-Ecken verschweißt, Branch-Sets vereinigt). Qualitätsboden: nie schlechter als der bisherige Voll-Octree-Fallback.

## 0.6.475
- fix(shell): Shell-Open-Faces erreichen jetzt auch Sweep-Körper: Beide SDF-Evaluatoren (BVH-Hot-Path und Picking-Pfad) gaten das geöffnete Profil-Segment im Querschnitt des Sweeps — Parität mit Extrude/Revolve/Helix, die den Retained-Pfad schon konnten.

## 0.6.474
- feat(thicken): Partielle (geschnittene) Zylinder- und Kegelwände lassen sich verdicken: Der Winkel-Spann der Wand wird aus den Rim-Kanten gelesen und die Voll-Rotations-Differenz mit einem extrudierten Sektor-Prisma auf den tatsächlichen Ausschnitt begrenzt — eine Half-Pipe wird zur halben Röhre statt zum Fehler. Außerdem: Up-to-Face akzeptiert jetzt Kegel-Ziele (koaxial, gleicher Halbwinkel — konstanter Wandabstand δ·sin α).

## 0.6.473
- fix(kernel): Koplanare ON-same-Überlappung webt manifold: Kreuzende Körper mit gleich hohen Deckeln doppel-deckten das Überlappungs-Quadrat (40 Orientierungs-Verletzungen). Neue Reparatur-Stufe im Weave: schlägt die Materialisierung fehl, partitioniert eine ±ε-Ownership-Probe die überlappenden Klassen-Regionen (kleinster enthaltender Member besitzt die Region), vergrabene Signatur-Branches verlieren die Kante, Flächen-innere Nähte fliegen. Nur bei nachgewiesenem Overlap aktiv — anstoßende Stapel (o2j9-Familie) bleiben byte-identisch.

## 0.6.472
- fix(kernel): DL-82 behoben: Bei feiner Octree-Auflösung erzeugte der Edge-Weave Geisterkanten quer über einen offenen Nutmund (der adaptive Seam-Tracer übersprang die Lücke in einem Schritt, alle Samples lagen auf den Rims). Der Body-Boundary-Filter testet jetzt symmetrisch (vergraben UND in der Luft) und probt bei getracten Chains zusätzlich die Chord-Mittelpunkte gegen das Sagitta-Budget — die Brücke fliegt, echte gekrümmte Kanten bleiben.

## 0.6.471
- feat(kernel): Persistentes Naming N4 — selbstheilende Dokumente: Beim Speichern (inkl. Auto-Save) werden alle gespeicherten Element-Referenzen gegen das aktuelle Modell re-kanonisiert — geheilte Referenzen bekommen frischen Hash + Fingerprint (kein Fingerprint-Verfall über viele Edits), alte hash-only Referenzen werden auf die heilbare Langform aufgerüstet. Unauflösbare Referenzen bleiben unverändert und melden ihren Fehler ehrlich am Feature.

## 0.6.470
- feat(kernel): Persistentes Naming N2/N3: Element-Referenzen tragen jetzt einen geometrischen Fingerprint (qFace(<hash> @ …), kanonische Form kommt fertig aus dem Kernel) und HEILEN beim Auflösen — eine gespeicherte Face-/Edge-/Vertex-Referenz überlebt topologische Edits, statt mit „face not found“ zu scheitern. Alle Feature-Pfade (Extrude auf Face, Up-to-Face, Thicken, Shell, Fillet, Mate-Anchor) laufen über eine gemeinsame healing-fähige Auflösung statt sieben Hash-Scan-Kopien.

## 0.6.469
- fix(kernel): Persistentes Naming N1: Referenz-Auflösung bewertet alle Kandidaten einheitlich (Fingerprint-Ähnlichkeit + Hash-Bonus, provenienz-gebunden auf die Signatur-Gruppe) — ein Index-Kipp bindet nicht mehr still an das falsche Element, gleich gute Kandidaten melden ehrlich „Ambiguous“. Räumliche Index-Vergabe jetzt mit gruppen-relativem ε statt absolutem 1e-9 (nm-Jitter kippt keine Identitäten mehr). Neue Handle-Resolve-Gate-Testfamilie (naming_gate.rs).

## 0.6.468
- fix(mate) #e94cf61: Mate-Connector-Gizmos (platzierte Connectoren und die Hover-Vorschau) erscheinen jetzt zoomstufen-unabhängig immer gleich groß (~2rem), statt mit der Zoomstufe mitzuwachsen. Die Arme werden pro Frame auf eine konstante Bildschirmgröße skaliert — dieselbe Screen-Space-Logik wie bei den übrigen Gizmos.

## 0.6.467
- fix(debug) #72a141f: Debug ▸ Part-AABB zeigt jetzt auch die Bounding-Box eines über die Parts-Liste selektierten Körpers. Ein Klick in der Parts-Liste markiert den Körper über den Highlight-Index, nicht über die Feature-Selektion — Overlay und Zahlen-Panel lasen aber nur letztere und blieben daher leer. Beide berücksichtigen nun zusätzlich den hervorgehobenen Part.

## 0.6.466
- fix(mesher) #0018657: Zahlenfelder, deren Änderung ein teures Neu-Weben auslöst (Mesher-, Mesher-PASM- und Export-Dialog), werten die Eingabe jetzt erst bei Verlassen des Feldes bzw. mit Enter aus. Vorher hing der Mesher schon während der Eingabe: Tippt man „0“ und dann „0,“, wurde der Zwischenwert auf das Minimum geklemmt und ein extrem feines Mesh gewoben, obwohl die Eingabe noch gar nicht fertig war.

## 0.6.465
- feat(mesher) #nwmi: Mesher PASM: Krümmungs-Anisotropie (Paper §3.2, opt-in Checkbox im PASM-Dialog). Die Gauß-Paarkraft wird mit der Flächenmetrik M=QᵀQ aus dem SDF-Formoperator verzerrt (Paper Gl. 21, Q_ij statt M_ij — der ASPH-Fehler wird vermieden): Partikel packen dichter, wo die Fläche krümmt, und gröber, wo sie flach ist. Am langen Zylinder messbar weniger Dreiecke bei gleicher Sag-Toleranz, gestreckte Dreiecke in der flachen Richtung, weiterhin wasserdicht/2-manifold. Bewusst opt-in (nicht im Export-Default), da der Formoperator pro Partikel die Vernetzung deutlich verlangsamt.

## 0.6.464
- feat(export) #nwmi: Export-Stufe „CAD (kantenexakt, PASM+B-Rep)“ — die Hybrid-Pipeline (Skelett → PASM-Relaxation → per-Face-CDT → DC-Fallback) ist jetzt ein wählbares Meshing-Verfahren im Export-Dialog neben MDC/MC/Surface-Nets. Die Zellgröße wird dabei als Ziel-Kantenlänge h gelesen. Ein Geschlossenheits-Gate fängt Körper ab, die die PASM-Kette nicht dicht bekommt (z.B. unvollständig gewebte Gyroide) und fällt für diesen Körper sauber auf das adaptive Verfahren zurück, sodass der Export nie ein Loch ausliefert.

## 0.6.463
- feat(mesher) #nwmi: Mesher PASM: Fallback für Flächen ohne Chart (Torus-/Gyroid-Träger, Custom-SDF) — die Region wird als Signed-DC-Octree-Patch vermascht, per Reißverschluss strukturell wasserdicht an die exakten B-Rep-Rim-Ketten genäht (geteilte Indizes, Loch-Füllung, Komponenten-Gate) und im Dialog orange dargestellt. Neuer Schritt-Selektor (Seeds → Skelett → Relax → CDT → Fallback) zeigt jede Pipeline-Stufe einzeln in der Szene; Kernel-Test: Torus ∩ Quader end-to-end geschlossen, 2-manifold, Euler 0.

## 0.6.462
- fix(export) #cdb25e1e: Export-Preview: Wireframe-Checkbox zeigt jetzt auch in den SDF-Rendermodi ein Drahtgitter (WebGPU ignoriert material.wireframe — das Gitter ist jetzt echte Linien-Geometrie), und der Deckkraft-Regler wirkt stufenlos statt als An/Aus (konstante Alpha-Blend-Pipeline statt Pipeline-Rebuild pro Reglerschritt).

## 0.6.461
- feat(mesher) #nwmi: Debug ▸ „Mesher PASM…“ — eigener Dialog für die Hybrid-PASM-Pipeline (B-Rep-Skelett → Gauß-Relaxation → per-Face-CDT) mit genau einem Auflösungsparameter: der Ziel-Kantenlänge h. Seed-Octree-Tiefe, Kernel-Radius (5σ = h), Ketten-/Naht-Abstand und CDT-Clearance leitet der Kernel daraus ab (neues targetEdgeLength in mesherPoints). Der generische Mesher-Dialog bleibt als Baukasten erhalten; beide schließen sich gegenseitig aus.

## 0.6.460
- fix(dev) #dc02f591: „Auto-reload on code change: Off“ schottet die App jetzt komplett von Code-Änderungen ab: Bisher blieben Hot-Module-Updates aktiv und setzten beim Editieren von Quellcode den UI-Zustand zurück (ohne Reload). Der Client meldet die Einstellung an den Dev-Server, der daraufhin sämtliche Hot-Payloads (Updates, Prune, Full-Reload) für diesen Tab verwirft — F5 lädt weiterhin den neuesten Stand, und das Umschalten wirkt sofort ohne Neustart.

## 0.6.459
- feat(projects) #14661fc4: Lokale Projekte erscheinen jetzt in der Projekt-Übersicht („On this device“) und lassen sich von dort wieder öffnen: „Open local project…“ nutzt die File-System-Access-API, merkt sich das FileHandle in IndexedDB, und der Autosave schreibt die .fdraft-Datei fortan direkt auf der Platte mit (gleiche Bytes wie „Download project“). „Forget“ entfernt einen Eintrag aus der Liste (die Datei bleibt). Auf Browsern ohne File-System-Access-API bleibt der klassische Datei-Dialog ohne Merken/Disk-Autosave.

## 0.6.458
- feat(app) #3032c29b: URL-Routing: Issues und Projekte haben jetzt URLs (#/issues, #/issues/<id>, #/projects, #/p/<projekt-id>, #/edit) — Browser-Zurück funktioniert (z. B. vom Issue-Detail zur Liste), und die URLs sind teil-/lesezeichenbar: ein direkt geladener #/p/<id>-Link öffnet das Server-Projekt, #/issues/<id> das Ticket.

## 0.6.457
- feat(ui) #6c09c11f: Burger-Menü unterscheidet lokal vs. Cloud: „Save“ heißt jetzt „Download project“ (lokale .fdraft-Datei, Ctrl+S), „Open…“ heißt „Open local project…“, und ein neuer Punkt „Save to cloud“ macht aus dem offenen lokalen Projekt ein Cloud-Projekt (legt es auf dem Server an und bindet es — ab dann greift der Autosave). Bei bereits gebundenen Projekten ist der Punkt mit Hinweis deaktiviert.

## 0.6.456
- feat(projects) #421c3610: Neuer „Import…“-Button neben „New project“ auf der Projekt-Seite: importiert ein lokales .fdraft als neues Server-Projekt (benannt nach der Datei) und öffnet es direkt gebunden.

## 0.6.455
- feat(tickets) #d22a909e: Tickets lassen sich wieder öffnen: Der Autor eines resolved-Tickets hat neben „Close issue“ jetzt einen „Reopen“-Button (setzt zurück auf in_progress — „der Fix reicht nicht“). Andere Zustandswechsel bleiben Admin/PO-Sache.

## 0.6.454
- fix(ui) #b5a60122: „Report Issue…“ ist zurück im Burger-Menü (mit sichtbarem F2-Shortcut) und aus dem User-Menü entfernt — das User-Menü ist wieder nur Identität + Log out.

## 0.6.453
- fix(tickets): „Report an issue“ scheiterte mit einem 500er, wenn in den ersten 80 Zeichen der Beschreibung ein Umlaut auf der Kürzungsgrenze lag: Der abgeleitete Titel wurde byteweise statt zeichenweise gekürzt, das halbe Multibyte-Zeichen war ungültiges UTF-8 und die Datenbank lehnte den Eintrag ab. Der Titel wird jetzt runen-sicher gekürzt.

## 0.6.452
- fix(viewport) #pzvt: Kanten-Hover trifft wieder, auch wenn der sichtbare Frame während langer Shader-Compiles eingefroren ist („Pick what you see“): Die SDF-Renderer publizieren die Kamera jedes tatsächlich gezeichneten Frames, und Edge-/Face-/Vertex-Picking projiziert gegen diesen Snapshot statt gegen die Live-Kamera. Bisher desyncte jede Kamera-Änderung im Compile-Fenster (View-Restore, kleine Orbits) Interaktion und Anzeige — das 8px-Kanten-Band verfehlte zuerst, Flächen schienen weiter zu funktionieren.

## 0.6.451
- feat(tickets): Links in Issue-Beschreibungen und Kommentaren sind jetzt klickbar (öffnen in neuem Tab) — z. B. die Commit-Links in Dev-Kommentaren.

## 0.6.450
- fix(ui) #3d0f20de: „Open…“ funktioniert jetzt auch von der Projekt-Übersicht aus: Das gewählte lokale .fdraft wird geladen und die App wechselt in die Modellier-Ansicht. Bisher lud die Datei unsichtbar in den Hintergrund-Workspace („es passiert nichts“); außerdem wird vorher die Server-Bindung gelöst, damit der Autosave das gebundene Server-Projekt nicht mit dem lokalen File überschreibt.
- fix(tickets) #ddd6d897: „Attach screenshot“ geht jetzt auch auf der Projekt-Übersicht. Der Capture hing bisher endlos an zwei Stellen: an den (noch leeren) 404-Projekt-Thumbnails und am Klonen der unsichtbar mitgemounteten 3D-Canvas. Kaputte Bilder und hidden-Bereiche werden jetzt aus dem Capture gefiltert, dazu ein 8-Sekunden-Sicherheitslimit.
- feat(tickets) #582dfe62: Angehängte Bilder in der Issue-Detailansicht zeigen jetzt ein Preview-Thumbnail (wie der Fenster-Screenshot) statt eines reinen Datei-Chips.

## 0.6.449
- fix(dev) #a020ac1e: „Auto-reload on code change: Off“ wirkt wieder. Vite 8 führt HMR-Listener über Promise.allSettled aus, wodurch der bisherige Abbrech-Trick (Exception im vite:beforeFullReload-Handler) still wirkungslos wurde — die Seite lud trotz Off-Schalter neu. Der Dev-Server schickt jetzt nie mehr ein rohes full-reload; der Client entscheidet selbst anhand der Einstellung.
- fix(ui) #376bf453: Burger-Menü: Dokument-Aktionen (New, Open, Save, Import, Export, Share, Undo, Redo, Debug) sind außerhalb der Projekt-Ansicht deaktiviert — auf der Projekt-Seite operierten sie bisher unsichtbar auf dem Hintergrund-Workspace.
- fix(ui) #e752b6f8: „Report Issue…“ und „Release Notes…“ sind aus dem Burger-Menü entfernt. Beides bleibt erreichbar: Issues über F2, Release Notes über Klick aufs Versions-Badge.

## 0.6.448
- fix(export) #63989a9e: Server-Projekt-Bindung überlebt jetzt einen Seiten-Reload: Bisher stellte das Autosave zwar das Dokument wieder her, die Sync-Bindung ging aber verloren — der CGAL-Server-Export meldete dann fälschlich „save to server first“, obwohl das Projekt auf dem Server lag. Die Bindung wird jetzt persistiert und beim Start gegen den Server verifiziert wiederhergestellt (gelöschte/fremde Projekte binden nicht).
- fix(projects) #63989a9e: Ein Server-Projekt mit defektem Dokument-Blob öffnete sich bisher STILL falsch: Die App wechselte in die Modellier-Ansicht, zeigte dort aber das vorherige lokale Dokument ungebunden an — mit demselben „save to server first“-Symptom. Jetzt bleibt die App auf der Projekt-Seite und zeigt eine Fehlermeldung.

## 0.6.447
- perf(export): CGAL-Server-Meshing: zell-lokale Opcode-Tape-Spezialisierung (C++-Gegenstück zu cell_csg.rs) — pro Octree-Zelle werden Boolean-Zweige, die das Feld dort beweisbar nicht beeinflussen, per 1-Lipschitz-Ball-Bound geprunt und die verkürzte Tape an Kindzellen vererbt; das Mesh_3-Oracle evaluiert nur noch die Zell-Tape. 512-Teile-Baugruppe: 2,2× Wall / 4,0× CPU (Tape 6656 → Ø 30 Records); bit-exakt verifiziert (check_spec-Harness, 7 Modelle × 20k Zufallspunkte, Abweichung 0). FD_NO_SPEC=1 schaltet ab.

## 0.6.446
- feat(export): Custom-SDF-Körper (Math-Expressions) sind jetzt auch über das Server-Meshing exportierbar: OP_PRIM_CUSTOM (0x0E, Expression-Stack-Machine) ist in den C++-CGAL-Interpreter portiert, das Ausschluss-Gate in fd-prepare entfällt. Golden-Szene (sin-displaced Sphere, 26 Szenen gesamt) und End-to-End-Verifikation (wasserdicht/2-manifold) grün.

## 0.6.445
- feat(kernel): Neues Feature „Custom SDF“ (ƒ in der Toolbar): eigene implizite Körper aus einer Math-Formel über x, y, z — z. B. TPMS-Gitter oder algebraische Flächen. Die Formel (sin, cos, sqrt, abs, min, max, exp, ln, atan2, pow, ^, pi/tau) wird einmal in eine Opcode-Tape kompiliert und identisch auf CPU (inkl. exakter Gradienten) und GPU ausgewertet; Lipschitz-Faktor normalisiert das Feld, eine lokale Box begrenzt den Körper. Custom-Bodies sind SDF-only (kein B-Rep), wie das Gyroid
- perf(kernel): Zell-lokale CSG-Baum-Spezialisierung im Kernel: B-Rep-Octree und adaptiver Narrow-Band-Mesher prunen pro Zelle Boolean-Zweige, die das Feld dort beweisbar nicht beeinflussen (fidget-artige Tape-Verkürzung auf dem eigenen Baum), und vererben den verkürzten Baum an die Kindzellen — Octree-Aufbau auf zusammengesetzten Körpern mehrfach schneller (30-Teile-Testbaugruppe: 3,9×), byte-identisches Ergebnis. Zusätzlich eps-bewusstes AABB-Pruning in der Contributor-Evaluation

## 0.6.444
- feat(export): CGAL-Server-Meshing versteht jetzt alle Kernel-Opcodes: HALFSPACE (geöffnete Shells), Helix und Sweep sind in den C++-Interpreter portiert, ein Masked-Blend-Format-Drift ist gefixt (count-präfixierte Polyline statt festem 2-Punkt-Kapsel-Layout). Der Golden-Test deckt alles mit 9 neuen Szenen ab (25 gesamt, 12.800 Checks). Dazu Douglas-Peucker-Dezimierung der Kanten-Schutz-Polylines im Mesher — eine Helix lieferte 102.000 Tracer-Punkte und würgte CGALs Feature-Protection ab. Helix+Vase end-to-end verifiziert: wasserdicht, 2-manifold, kantentreu (3,4 Mio. Dreiecke).

## 0.6.443
- feat(export): CGAL-Meshing Meilenstein M6 (3MF): Die Server-Methode „CGAL Mesh_3 (server)“ exportiert jetzt auch 3MF — das Server-STL wird clientseitig durch den Kernel-3MF-Encoder konvertiert (neues mesh3mfFromBuffers), das Format-Select ist für die Server-Route freigeschaltet. Damit sind alle Meilensteine des serverseitigen CGAL-Meshings (M1–M6) abgeschlossen.

## 0.6.442
- feat(deploy): CGAL-Meshing Meilenstein M5 (Prod-Deployment): mesher-Service in docker-compose.prod.yml (internes Netz, horizontal skalierbar via --scale), drittes GHCR-Image im Release-Workflow (Golden-Test läuft im Image-Build), Env-Tuning (MESH_WORKERS/CONCURRENCY/THREADS/TIMEOUT) in .env.prod.example, DEPLOY.md + ARCHITECTURE.md nachgeführt. Ansible-Deploy zieht das Image automatisch mit.

## 0.6.441
- feat(export): CGAL-Meshing Meilenstein M4 (Frontend): neue Export-Methode „CGAL Mesh_3 (server)“ im Export-Dialog — speichert das Projekt, startet den Server-Job, pollt den Status (Warteschlange/Server mesht/Download, mit Cancel), zeigt das STL als Preview in der Szene und lädt die unveränderten Server-Bytes herunter. Nur mit server-gespeichertem Projekt und STL-Format wählbar (sonst ausgegraut).

## 0.6.440
- feat(export): CGAL-Meshing Meilenstein M3 (Backend-Job-Queue): persistente mesh_jobs-Tabelle (überlebt Server-Restart, Boot-Recovery running→queued), Dispatcher-Worker-Pool mit atomarem Claim (replikasicher), Routen POST /api/projects/{id}/mesh → 202 jobId, Polling-Route, STL-Artefakt-Download (owner-gated, Content-addressed Blob), Abbruch-Route; max. 1 aktiver Job pro User. Integrationstest + Live-Roundtrip gegen den echten Mesher-Container verifiziert.

## 0.6.439
- feat(export): CGAL-Meshing Meilenstein M2 (Service-Container): Docker-Image (Debian-CGAL mit GMP/MPFR, TBB-Multithreading mit FD_MESH_THREADS-Deckel, Golden-Test als Pflicht-Build-Schritt) + Go-HTTP-Wrapper (POST /mesh → STL, Concurrency-Limit, Job-Timeout, Größenlimit) + Compose-Service „mesher“ im internen Netz.

## 0.6.438
- feat(export): Serverseitiges CGAL-Meshing, Meilenstein M1 (Kern-Pipeline): fd-prepare (natives Kernel-Binary: Dokument → SDF-Opcodes + B-Rep-Kanten-Polylines) + fd-cgal-mesh (natives CGAL Mesh_3 mit Kanten-Schutzkugeln, manifold-Kriterium, Außen-Orientierung, binäres STL). Box/Zylinder/Revolve-Rohr verifiziert wasserdicht/2-manifold mit exakter Kantentreue; Golden-Test Rust↔C++ fand und fixte zwei Interpreter-Drifts (Cone wall_scale, Shell inward). Noch ohne UI — Backend-Route und Export-Dialog-Option folgen.

## 0.6.437
- feat(debug) #nwmi: Hybrid-PASM P4c: Polkappen-Fächer — Faces mit einer windenden Loop um einen Pol/Apex (Kugelkappe, Kegelspitze, Revolve-Achsberührung) werden jetzt vernetzt: ein Pol-Ring aus Hilfspunkten knapp vor dem Pol, das Stück Loop↔Ring über den Band-Pfad, der Ring per Dreiecks-Fächer zum Pol geschlossen — jedes Fächer-Dreieck berührt den Pol genau einmal, nichts kollabiert. Ring-Auflösung aus der Welt-Bogenlänge (am Pol schrumpft der Umfang). Kugelkappe ∩ Box end-to-end geschlossen/2-Manifold (Euler=2).

## 0.6.436
- feat(debug) #nwmi: Hybrid-PASM P4b: Revolve-Träger im CDT-Mesher — die v-Achse des Charts ist die Bogenlänge der Nächster-Punkt-Projektion auf die offset-korrigierte Profilkette (ssi-Profile-Pieces). Rohr-Meilenstein grün: Rechteck-Revolve ∩ Box liefert end-to-end ein wasserdichtes Torus-Netz (Euler=0) — Außen- und Innenwand liegen auf demselben Branch und trennen sich rein über das Chart, die Deckel sind Annuli mit Loch-Loop. Faltungs-Guard: springt v entlang einer Loop um mehr als ¼ der Kettenlänge, bleibt die Face unvermascht.

## 0.6.435
- feat(debug) #nwmi: Hybrid-PASM P4a: CDT auf abrollbaren Trägern — Zylinder/Kegel/Kugel-Bänder werden ins (θ·r, v)-Chart abgewickelt und periodisch vernetzt; die Schnitt-Naht teilt ihre Steiner-Punkte zwischen beiden Kopien und ist damit strukturell verschweißt. Voller Zylinder jetzt end-to-end geschlossen/2-Manifold (Band + Deckel, Euler=2). Dreiecks-Klassifikation jetzt kombinatorisch (Flood-Fill mit Constraint-Parität) statt Zentroid-Test — robust gegen Naht-Sliver; randnahe Partikel (<0,25·h) werden vor dem Einfügen verworfen, damit kein einseitiger Constraint-Split die Naht öffnet.

## 0.6.434
- feat(debug) #nwmi: Hybrid-PASM-Meshing Phasen 2+3: „PASM kernel (Gaussian)“ — der Paar-Kernel des Papers (σ = Interaktionsradius/5, 5σ-Trunkierung, Konvergenz-Abbruch) ersetzt wahlweise den 1/d²-Monopol; „CDT faces (planar)“ — planare Faces werden per Constrained Delaunay (spade) in der Face-Ebene vernetzt, mit den B-Rep-Skelett-Loops als Constraints und den freien Partikeln als Innenpunkten. Erster End-to-End-Beweis: die Box liefert ein GESCHLOSSENES 2-Manifold-Netz (jede Kante exakt 2×, Euler V−E+F=2) — kantenexakt und wasserdicht per Konstruktion

## 0.6.433
- feat(debug) #nwmi: Hybrid-PASM-Meshing Phase 1 (Konzept-SSOT docs/HYBRID-PASM-MESHING-PLAN.md): Debug ▸ Mesher kann die gewebten B-Rep-Kanten als FIXES Partikel-Skelett einspannen — Ecken exakt gepinnt (rot), Kantenketten bogenlängen-gesampelt am Interaktionsradius (orange); sie stoßen die freien Flächen-Partikel ab, bewegen sich aber nie. Scharfe Features werden Randbedingung statt Detektionsaufgabe — die konstruktive Basis für kantenexaktes, wasserdichtes Meshing

## 0.6.432
- fix(viewport) #7cv2: Schmale Flächen waren nicht hoverbar (nur einzelne Kanten leuchteten): der Kanten-Pick-Radius war in NDC definiert und wuchs mit der Fenstergröße auf ~15px — breit genug, um eine schmale Seitenfläche von beiden Silhouetten her komplett abzudecken, sodass per Edge-vor-Face-Priorität nie die Fläche gewann. Kanten- (8px) und Vertex-Pick-Radius (11px) sind jetzt pixel-basiert und fenstergrößen-stabil

## 0.6.431
- feat(kernel): B-Rep: Vertex-Witnesses — Ecken (Tripelpunkte) werden VOR dem Kanten-Tracing analytisch gelöst und den Märschen als Snap-Ziele mitgegeben; ein Marsch endet exakt an der Ecke statt eine weggetrimmte Lücke zu überspringen (fundamentale Lösung des Gap-Leap-Problems). Schutzgitter: Aktivitäts-Gate über alle drei Flächenpaare, Spatprodukt-Rang-Gate gegen Phantom-Konkurrenzen entlang µm-gedrifteter Koplanar-Wände, Mikro-Stub-Filter; Branch-Union wie im Cluster-Pfad hält die Handle-Identität zur Octree-Route
- fix(camera) #zc9b: Zoom zum Mauszeiger (Ortho): der Pin-Offset wurde relativ zum Orbit-Pivot statt zur Kamera-Achse gerechnet — seit dem Onshape-Pivot-Raycast liegt der Pivot oft abseits der Blickachse, dann zoomte das Mausrad am Cursor vorbei (aufs Bildschirmzentrum). Jetzt bleibt der Punkt unterm Cursor exakt stehen

## 0.6.430
- fix(kernel) #7sp1: B-Rep: fehlende vertikale Kanten, wo eine große Zylinderwand eine kleine Box-Seitenfläche schneidet (Gyroid-Sliver). Drei Wurzelursachen behoben: (1) der SSI-Scan verwarf Schnitt-Zellen mit Eckpunkt knapp außerhalb der Overlap-Box — die Ecken-Maske wird jetzt um eine Zell-Diagonale erweitert (Seeds bleiben strikt in der Box); (2) die Co-Component-Dedup fraß parallel laufende Zwillings-Kanten — Radius von 0.1·diag auf 10× Sagitta-Toleranz verkleinert, Trim-Termini zählen zur Dedup-Polyline; (3) ein langer adaptiver Trace-Schritt konnte eine weggetrimmte Lücke komplett überspringen und zwei getrennte Kantenbögen überbrücken — neuer Mittelpunkts-Gap-Guard im March plus halbierte Seed-Routen-Schrittkappe

## 0.6.429
- feat(debug): Debug ▸ Mesher: „Feature snap (edges/corners)“ — Kanten/Ecken werden per Normalen-Proben erkannt, Punkte exakt auf den Schnitt der Tangentialebenen gezogen und dort verankert (Kantenpunkte gleiten 1D entlang der Kante, Ecken sind fixiert; beide stoßen weiter ab). Löst das systematische Abrunden harter Kanten durch isotrope Repulsion (0D/1D-Features haben Maß null); einstellbarer Feature-Winkel

## 0.6.428
- feat(kernel): Gyroid-Körper haben jetzt B-Rep-Kanten: die Boolean-Rim-Kurven (wo Box/Zylinder/Kugel-Flächen die Gyroid-Shell schneiden) werden über den octree-freien Seed-Pfad exakt getraced (SSI-Seeds + Dual-Field-Tracing) und als partielles Wireframe gewebt. Der Octree bleibt für Space-Filling-Körper weiterhin ausgeschlossen (kein DL-71-Stall); Helix/Sweep-Körper bleiben unverändert kantenlos

## 0.6.427
- feat(kernel): Kernel-Spike: Der SSI-Kurven-Seeder unterstützt jetzt Gyroide als Implicit-Operand (Ebene/Zylinder ∩ Gyroid) — Machbarkeitsnachweis für exakte Boolean-Kanten an TPMS-Körpern (Box∩Gyroid: 66 Rim-Kurven, ~9k Seeds, wenige ms). Noch nicht im B-Rep-Weave aktiv; Gyroid-Bodies bleiben vorerst ohne Kanten (DL-71)

## 0.6.426
- fix(debug): Debug ▸ Mesher: „Damping“ ist jetzt ein direktes Zahlenfeld statt eines Sliders

## 0.6.425
- fix(debug) #l45r: Debug ▸ Mesher: Punkte verklumpten trotz starker 1/d²-Repulsion in einer Ecke — Ursache war eine numerische Instabilität: der explizite Schritt konnte bei hoher „Strength“ einen Punkt komplett über seine Nachbarn hinweg und von der Oberfläche schleudern (wo „Only on surface“ ihn verwarf), während der symmetrische Cluster-Kern (Netto-Kraft ≈ 0) liegen blieb. Der Repulsions-Schritt wird jetzt auf die halbe Distanz zum nächsten Nachbarn begrenzt → stabil für JEDE Strength, Klumpen dehnen sich monoton zu gleichmäßigem Abstand

## 0.6.424
- fix(debug): Debug ▸ Mesher: Repulsions-Kraftgesetz ist jetzt durchgängig invers-quadratisch (∝ 1/d², wie gleich geladene Monopole) — nähere Punkte stoßen sich viel stärker ab, dadurch bleiben Cluster nicht bestehen und die Verteilung bleibt lokal und gleichmäßig statt global in die Ecken zu driften. Die „Weighting“-Auswahl (Linear/Inverse Square) entfällt, 1/d² ist das einzige Gesetz

## 0.6.423
- feat(debug) #yanv: Debug ▸ Mesher: UI an das Ein-Schleifen-Verfahren angepasst — die zwei alten Iterations-Felder durch ein einziges „Iterations“ ersetzt; neu: Repulsion-„Damping“ (Abkling-Faktor der Repulsion pro Iteration) und „Damping curve“ (Linear/Exponential) für das Annealing

## 0.6.422
- feat(debug) #yanv: Debug ▸ Mesher: Punktverteilung jetzt eine EINZIGE interleavte Schleife (Repulsion → Fit → Clamp pro Iteration) mit von stark nach null abklingender Repulsion, plus abschließendem Fit-to-Surface — die Punkte spreizen sich früh und liegen am Ende wirklich auf der Oberfläche (im Rahmen des Clamps)

## 0.6.421
- feat(debug) #yanv: Debug ▸ Mesher: Punktverteilung läuft jetzt als 2-Phasen-Iteration aus atomaren Schritten — Phase 1 pro Schritt Fit-on-Surface → Clamp-to-Cell (bis Target-ε oder Max-Iterationen), Phase 2 pro Schritt Repulsion → Fit → Clamp (für Repulsion-Iterationen); jeder Schritt nur, wenn die zugehörige Checkbox gesetzt ist

## 0.6.420
- feat(debug): Debug ▸ Mesher „Mesh points“ verbindet jetzt die angezeigten (gefilterten) Punkte entlang der Octree-Kantenbeziehungen zu Dreiecken — kein Dual Contouring, kein Vorzeichenwechsel-Test, Löcher/falsche Facetten sind bewusst erlaubt; das Ergebnis wird nur als Wireframe gezeichnet

## 0.6.419
- fix(debug) #jt76 #gjuu: Debug ▸ Mesher: die Filter „Neighbour sign change only“ und „Only on surface“ blenden die Normalen/Punkte jetzt sichtbar aus — der Kernel liefert pro Punkt ein shown-Flag, das alle Prozess-Filter berücksichtigt (vorher wirkten sie nur intern auf Fit/Repulsion/Mesh)
- fix(project) #4z37: Von Disk geöffnetes Projekt überlebt jetzt einen Reload: ein „Projekt öffnen“ persistiert sofort in localStorage und holt einen Deep-Link-Repro-Tab (?project=) aus dem Repro-Modus (echter Autosave, URL-Parameter entfernt), statt beim Reload leer zu sein

## 0.6.418
- feat(debug): Debug ▸ Mesher: Filter „Only on surface“ — Punkte, die die Oberfläche nach der Newton-Projektion nicht erreichen (in ihrer Zelle geclampt), fallen komplett aus dem Set: nicht gezeichnet, keine Repulsion, kein Mesh-Vertex

## 0.6.417
- feat(debug): Debug ▸ Mesher: „Mesh points (dual contour)“ — adaptives Dual Contouring über den gefilterten/projizierten/abgestoßenen Punkten (ein Vertex pro Octree-Blattzelle, Quads an vorzeichenwechselnden Minimal-Kanten, crack-frei per rekursivem cell/face/edge-Walk), optional mit 2:1-balanciertem Octree; Dreieckszahl im Dialog

## 0.6.416
- fix(export) #rfm6: Export-Vorschau: SDF ↔ Mesh-Crossfade blendet wieder graduell — nach dem jüngsten Tiefen-Fix verlor das exakt auf der Fläche liegende Preview-Mesh den Depth-Test gegen den SDF-Körper; es zieht sich jetzt per polygonOffset nach vorn

## 0.6.415
- feat(debug) #jq2t: Debug ▸ Mesher: iterative Punkt-Optimierer — „Fit on Surface“ (gedämpfte Newton-Projektion auf die SDF-Nullfläche) und „Repulsion inside Surface“ (tangentiale Relaxation mit Re-Pinning), mit Zell-Clamping (constrainToCell), Vorzeichenwechsel-Nachbar-Filter und Gewichtungsfunktion Linear/Inverse Square — komplett im Kernel (31k Punkte fitten in ~70 ms)

## 0.6.414
- feat(debug): Debug-Dialoge (SDF-Plane, Mesher, Part-AABB) persistieren im Projekt — ein gespeichertes Projekt oder Ticket öffnet mit exakt dem Debug-Setup (offene Dialoge + Parameter), mit dem es erfasst wurde
- feat(viewport): Projekt-Autosave per Dirty-Tracking: auch reine Kamera-Bewegungen und Debug-Dialog-Änderungen speichern jetzt (debounced); Debounce als Einstellung „Autosave-Debounce (s)“ (Default 5 s), beim Schließen des Tabs wird sofort geschrieben

## 0.6.413
- fix(render) #iw6f: SDF-Renderer: blickwinkelabhängige Parallaxe zwischen Solid und SDF-Plane behoben — die Szenen-Tiefe wurde 0,1 % hinter die Oberfläche geschoben (Kanten-Bias), was an flach geschnittenen Ebenen zur Sichel verstärkte; B-Rep-Kanten ziehen sich jetzt selbst per polygonOffset nach vorn

## 0.6.412
- fix(export) #4m67: Extended Marching Cubes: Feature-Zellen zelteten jedes Patch-Dreieck über alle drei Kanten auf den Feature-Vertex — jetzt nur noch über den Patch-Rand (Gyroid-Kugel: 12.650 → 24 non-manifold-Kanten, Abweichung 0,43 → 0,34 mm)
- fix(export) #4m67: Loch-Sicherheitsnetz auch für explizit gewählte Dense-Mesher: Manifold DC lieferte auf der Gyroid-Kugel 1.471 offene Kanten und 7,6 mm Ausreißer — bei Löchern ersetzt jetzt ein Marching-Tets-Netz das Ergebnis (im route-Label ausgewiesen)
- fix(export) #agco: Export-Verfahren „Marching cubes (robust)“ heißt jetzt ehrlich „Marching Tetrahedra (robust)“ — Titel und Beschreibung widersprachen sich

## 0.6.411
- fix(export) #tihw: Meshing-Verfahren „Reach for the Arcs“ entfernt — die sPSR-Rekonstruktion lieferte prinzipbedingt entartete Netze (625 Dreiecke, >5 mm Abweichung) und ist nicht rettbar

## 0.6.410
- feat(viewport) #pxff: Kamera-Blickwinkel (Position, Ausrichtung, Orbit-Ziel, Zoom) wird jetzt im Projekt gespeichert und beim Öffnen/Neuladen wiederhergestellt — statt immer auf die Default-Ansicht zu springen

## 0.6.409
- fix(debug) #0ly1: Mesher: die Surface-Farbschwelle wirkte kaum — die frühere graduelle, auf den Maximalwert normierte Farbrampe ließ oberflächennahe Samples fast immer grün wirken. Jetzt scharfe 3-Wege-Klassifikation: |SDF| ≤ Schwelle = grün, sonst blau (innen) / rot (außen). Der Schwellwert steuert das Grün nun direkt (kleiner Wert = wenig grün)

## 0.6.408
- feat(debug) #dbmy: Mesher-Debug erweitert: ein Parameter Surface ± für den Grün-/Surface-Schwellwert der Farbcodierung (|SDF| darunter = grün, darüber Rampe nach blau/rot) und ein Part-opacity-Slider, der den Solid durchscheinend macht — im transparenten Modus werden auch die innenliegenden Normalen gezeigt

## 0.6.407
- fix(debug) #dbmy: Mesher-Debug: Parameter Band guard in Band width umbenannt und mit Erklär-Tooltip versehen — er steuert die Narrow-Band-Breite der Octree-Unterteilung (nicht die Inside/Outside-Farbtrennung). Oberhalb ~1 gesättigt (1 bis 8 ohne sichtbaren Effekt), unter 1 engere Band, also weniger oberflächennahe Zellen/Normalen

## 0.6.406
- fix(ui) #cvii: Menüpunkt Debug ▸ Part-AABB war nach rechts eingerückt (ein unsichtbares EM-Space-Zeichen als Zustands-Platzhalter). Der Eintrag steht jetzt linksbündig wie die Nachbarn; der Aktiv-Zustand zeigt ein nachgestelltes Häkchen

## 0.6.405
- fix(camera) #q3bb: Umschalten Perspektive ⇄ Orthografisch springt nicht mehr: das Framing gleicht die Bildgröße jetzt um den tatsächlichen Blickmittelpunkt (Orbit-Target) ab statt um das Szenen-AABB-Zentrum — bei geschnittenen/versetzten Teilen wichen die auseinander. Zusätzlich erben die beim Umschalten neu erzeugten Kamera-Controls das letzte Orbit-Target, sodass ein Rotieren direkt nach dem Umschalten weiter um denselben Punkt dreht

## 0.6.404
- feat(camera) #ohtn: Kamera-Pivot wie in Onshape: Beim Start der Rotation wird ein Strahl vom Cursor in die Szene geschossen — trifft er Geometrie, dreht die Ansicht um genau diesen Punkt; trifft er ins Leere, um das Zentrum der sichtbaren Szene. Ersetzt die bisherige AABB-Sticky-Heuristik; man dreht jetzt um das, was unter dem Mauszeiger liegt (Maus + Touch)

## 0.6.403
- fix(dev) #y2b8: Auto-reload-Off-Unterdrückung robuster gemacht: statt sich auf die Listener-Reihenfolge zu verlassen (stopImmediatePropagation, brach wiederholt), umschließt der HMR-Socket-Patch jetzt Vites eigene message/close-Handler direkt und blendet full-reload-Payloads sowie den Server-Restart-Close aus, solange Auto-reload aus ist

## 0.6.402
- fix(ui) #35np: Feature-Details-Panel hat keinen Schlagschatten mehr — es ist in der Seitenleiste angedockt, kein schwebendes Fenster. Der Schatten bleibt den verschiebbaren/modalen Dialogen vorbehalten

## 0.6.401
- fix(debug) #5egt: Part-AABB-Panel: Schließen-× oben rechts ergänzt und das dekorative cyan-Kästchen vor dem Titel entfernt. Die Anzeige wechselt jetzt korrekt vom Leer-Hinweis auf die Maße des selektierten Parts (zusammen mit dem AABB-Fix ipnx)

## 0.6.400
- fix(kernel) #ipnx: Rotier-Pivot lag bei Körpern mit einem Gyroid-Intersect wieder im Ursprung: ein space-filling Werkzeug (Gyroid) meldet eine leere AABB, wodurch der Intersect die Body-AABB fälschlich auf leer kollabierte. Ein unbegrenztes Intersect-Werkzeug schränkt die Box jetzt nicht mehr ein — der Pivot folgt wieder dem sichtbaren Part

## 0.6.399
- fix(ui): Zahlenfelder sind jetzt durchgängig eine relaxte, text-basierte Eingabe (NumberInput) statt des nativen type=number: Zwischenzustände beim Tippen (führendes Minus, ein hängender Punkt, ein Dezimalkomma) sind erlaubt, kein Zurückspringen auf 0, kein aufgezwungener Spinner. Ersetzt alle rohen Number-Felder (Sketch-Werkzeuge, SDF-Plane, Mesher, Export, Sketch-Bemaßung)

## 0.6.398
- feat(debug): Debug ▸ Part-AABB: blendet die achsenparallele Bounding-Box des selektierten Parts als Wireframe im Viewport ein, plus ein Panel mit Größe (X × Y × Z) und Min/Max-Ecke. Nutzt die analytische, pipeline-gepflegte Body-AABB (eng nach Boolean-Schnitten)

## 0.6.397
- fix(kernel) #f88u: Body-AABB wird jetzt analytisch pro Feature durch die Pipeline gepflegt (wie das SDF): eine Remove-Operation zieht die Bound hinter die vom Werkzeug abgedeckte Schicht zurück, ein Intersect klippt sie auf das Werkzeug. Der Rotier-Pivot kommt aus dieser Body-AABB statt aus einer SDF-Abtastung — sauberer und ohne Sampling-Heuristik

## 0.6.396
- fix(viewport) #f88u: Sticky-Pivot-Verhalten wiederhergestellt: solange der Part ins Bild passt, folgt der Rotier-Pivot seinem (jetzt korrekten, engen) Mittelpunkt; ist so weit hineingezoomt, dass der Part nicht mehr ganz ins Bild passt, bleibt der Pivot an seiner letzten passenden Stelle statt auf einen Punkt außerhalb des Bildes zu springen

## 0.6.395
- fix(viewport) #f88u: Rotier-Pivot lag nach Boolean-Schnitten auf dem Zentrum des ursprünglichen Körpers (z.B. der ganzen Kugel) statt auf dem sichtbaren Rest. Der Pivot ist jetzt der Mittelpunkt der engen AABB des tatsächlich gerenderten Solids (aus dem SDF-Feld abgetastet), nicht mehr der konservativen CSG-Hülle

## 0.6.394
- fix(viewport) #f88u: Rotieren dreht jetzt immer um den Mittelpunkt der AABB des angezeigten Parts — die frühere „sticky pivot“-Logik (Pivot bleibt an der letzten passenden Stelle, wenn der Körper nicht ganz ins Bild passt) ist entfallen. Auch nach Verschieben/Zoomen zentriert ein Rotieren wieder auf den Part

## 0.6.393
- fix(dev) #0u31: Einstellung „Auto-reload on code change = off“ wird wieder respektiert: Vite 8 verschluckt (via Promise.allSettled) den Fehler, mit dem wir bisher den Full-Reload abbrachen — die Seite lud trotzdem neu (und verwarf das ungespeicherte Projekt). Neu wird die Vite-„full-reload“-Nachricht direkt am HMR-Socket abgefangen, bevor Vite sie verarbeitet

## 0.6.392
- feat(debug) #lbb3: Mesher-Debug-Ansicht verfeinert: die Normalen-Linien sind jetzt in der SDF-Farbe kodiert (innen/Oberfläche/außen) und haben eine einstellbare Stroke-Width; der Zell-Mittelpunkt ist ein runder Punkt (Durchmesser 3× Stroke-Width) in derselben Farbe. Neu: Checkbox „Near surface only“ mit Distanz-Eingabe blendet nur Octree-Zellen nahe der Oberfläche ein

## 0.6.391
- fix(viewport) #0m1l: Kamera lässt sich jetzt per Touch bedienen: ein Finger rotiert (nach kurzer Bewegung, ein Tippen selektiert weiterhin), zwei Finger zoomen (Pinch) und schieben (Pan). Vorher fingen Browser die Gesten ab und zoomten/verschoben die ganze Seite — die OnshapeControls hatten keine Touch-Verarbeitung, und dem Viewport fehlte touch-action:none samt user-scalable=no

## 0.6.390
- feat(ui): „Report Issue…“ ist jetzt auch im Haupt-Hamburger-Menü (neben Settings) — so lassen sich Fehler/Wünsche auch im nicht eingeloggten Zustand (Dev-Server) melden, nicht nur über das Benutzermenü

## 0.6.389
- fix(ui): App-Menü (Hamburger) lief auf schmalen/mobilen Bildschirmen rechts aus dem Viewport (Shortcuts und Submenu-Pfeil abgeschnitten). Auf Telefonen ist die Dropdown-Liste jetzt am rechten Rand des Buttons verankert (öffnet nach links) und maximal viewport-breit — sie bleibt vollständig sichtbar

## 0.6.388
- feat(debug): Neues Debug-Werkzeug „Mesher“ (Menü Debug ▸ Mesher…) als Gerüst für ein eigenes Meshing-Verfahren. Schritt 1: ein adaptiver Narrow-Band-Octree — jeder Blattzell-Mittelpunkt wird gegen SDF und Gradient ausgewertet und als Punkt gezeichnet (Farbe kodiert die SDF: innen blau, außen rot, an der Oberfläche grün) mit einer kurzen Linie entlang der Normale. Parameter: Max-Tiefe, Band-Breite, Normalen-Länge, Punktgröße; Zellzahl-Anzeige; Einstellungen in localStorage

## 0.6.387
- fix(viewport): Der Wechsel des Render-Shaders (WebGPU ⇄ WebGL) setzt die Kamera nicht mehr zurück: der Wechsel remountet den Canvas mit einer frischen Kamera auf der Standard-Ansicht — jetzt wird die Kamera-Pose (Position, Blickrichtung, Zoom, Orbit-Pivot) über den Wechsel hinweg erhalten

## 0.6.386
- fix(debug) #iwgq: SDF-Plane wird jetzt vom raymarching-Solid korrekt verdeckt und wandert nicht mehr blickwinkelabhängig: das Overlay-Quad lag auf dem Kanten-Layer, dessen Pass die Solid-Tiefe für den Röntgen-Effekt der B-Rep-Kanten löscht — dadurch schwebte die Ebene über dem Körper und zeigte einen Parallaxe-Versatz. Jetzt liegt sie (wie eine Datum-Ebene) auf dem Datum-Layer, der vor dem Tiefen-Löschen gezeichnet wird, sodass die grüne Null-Linie exakt auf der Körperoberfläche sitzt. Die SDF-Formeln (auch Gyroid) waren identisch — es war kein Feld-, sondern ein reiner Tiefen-/Rendering-Versatz. Nebenbei rendert die Ebene jetzt auch im WebGL-Modus

## 0.6.385
- feat(debug) #iepg: SDF-Plane-Dialog merkt sich seine Einstellungen (Punkt, Normale, Größe, Auflösung, Iso-Bänder, Step) in localStorage — sie überleben Schließen/Öffnen und Seiten-Reload

## 0.6.384
- feat(debug) #iepg: SDF-Plane-Dialog: die Schrittweite der Punkt-/Normalen-XYZ-Felder (Pfeiltasten/Spinner) ist über ein eigenes „Step“-Feld einstellbar — so lässt sich die Ebene grob oder fein durch das Modell schieben

## 0.6.383
- feat(debug) #iepg: SDF-Plane-Debug-Dialog erweitert: einstellbare Iso-Band-Anzahl (feinere Höhenlinien) und einstellbare Sampling-Auflösung der Farbkodierung; die Punkt-/Normalen-Felder sind breiter und ohne Labels; und der numerische SDF-Wert an der auf die Ebene projizierten Mausposition wird live im Dialog angezeigt

## 0.6.382
- feat(export): Neues Meshing-Verfahren MACET (Dietrich/Scheidegger, IEEE TVCG 2009): behält die Marching-Cubes-33-Topologie, verschiebt aber jede aktive Gitterkante so, dass sie senkrecht auf der Isofläche steht (Gradient- + Tangential-Transform mit dem analytischen ∇f) und findet den Schnitt per Bisektion neu — beseitigt schlanke/entartete Dreiecke und bleibt per Konstruktion watertight. Etwas langsamer (Nullstellensuche pro Kante). Verifiziert: Kugel geschlossen und ohne entartete Dreiecke

## 0.6.381
- feat(export): Drei neue Meshing-Verfahren im Export-Dialog: Marching Cubes 33 (Lewiner 2003 — topologisch korrekt, manifold & watertight für beliebige Felder, gute Wahl für Gyroid/TPMS), Dual Marching Cubes (Nielson 2004 — quad-dominant, wenige Slivers) und Extended Marching Cubes (Kobbelt 2001 — setzt in Zellen mit scharfer CSG-Kante einen QEF-Feature-Punkt und stellt so knackige Boolean-Kanten wieder her). MC33 & DMC sind per Konstruktion geschlossen (verifiziert auf Kugel und CSG-Schnitt)
- fix(build): Release-Notes-Datei ließ sich nicht mehr typprüfen: im vorigen SDF-Plane-Eintrag schloss ein ASCII-Anführungszeichen den String vorzeitig — auf ein typografisches „ “ korrigiert

## 0.6.380
- feat(debug): Debug-Werkzeug: Menü „Debug ▸ SDF-Plane…“ öffnet einen kleinen Dialog (Punkt + Normale + Größe). Solange er offen ist, wird eine Schnitt-Ebene im Raum angezeigt, die das exakte SDF-Feld der Szene farbcodiert (innen/außen, Iso-Distanz-Bänder, Null-Oberflächen-Linie) — zum Debuggen von Meshing-/Rasterungs-Artefakten direkt im Feld

## 0.6.379
- fix(export) #8l06: Surface-Nets-Export reißt keine Löcher mehr an Boolean-Schnittkanten: der QEM-Dezimierer der Bibliothek (baby_shark) zerriss einen an CSG-Nähten leicht non-manifolden Mesh in hunderte offene Kanten (statt zu dezimieren). Neu erkennt der Export das und weicht dort auf den loch-sicheren eigenen Dezimierer aus — der Mesh bleibt geschlossen und wird zudem stärker reduziert

## 0.6.378
- fix(export) #8l06: Export-Auflösung wird nicht mehr fälschlich gekappt, wenn ein Körper per Boolean-Intersect (z. B. ∩ Box) auf wenige Millimeter beschnitten ist: die Schnitt-AABB verengt sich jetzt auf den tatsächlichen Überlappungsbereich, sodass der beschnittene Bereich bei der gewünschten Zellgröße gemesht wird — an den Boolean-Kanten verschwinden dadurch die zellgroßen Zacken/Löcher (die SDF dort ist sauber, es war reine Unterauflösung)

## 0.6.377
- fix(export) #9ocx: Standard-Export (Adaptive) verschifft beim Gyroid keine Löcher mehr: der uniforme Fallback lieferte auf stark nicht-lipschitz Feldern bis zu 1471 offene Kanten + einen 17 mm daneben liegenden Ausreißer-Vertex. Neu greift ein garantiert lochfreies Marching-Tets-Netz, das den defektärmeren Mesh behält

## 0.6.376
- feat(export): Export-Dialog zeigt bei den dichten Meshern schon vor dem Meshen die erreichbare Zellgrößen-Untergrenze (Speicher-Budget) und eine Dreiecks-Abschätzung — so ist der Aufwand vorab sichtbar. Aktualisiert sich live beim Ändern von Zellgröße/Verfahren

## 0.6.375
- fix(export) #9ocx: Marching Cubes (robust) exportiert wieder watertight — ein Vorzeichenwechsel wurde je nach Zelle von unterschiedlichen Ecken interpoliert, sodass geteilte Kreuzungspunkte nicht verschmolzen (eine Kugel verlor 14.672 offene Kanten). Gyroid & Co. sind damit watertight ohne Löcher; an CSG-Schnittnähten bleiben wenige Non-Manifold-Kanten

## 0.6.374
- feat(account) #oyx8: Dev-Modus: automatischer Login als Seed-Admin (admin@local) beim Start des `vite dev`-Servers — Account-Features sind ohne manuelles Einloggen erreichbar. Nur im Dev-Browser aktiv (nicht in Prod, Tests oder e2e); Zugangsdaten via VITE_DEV_AUTOLOGIN_EMAIL/PASSWORD überschreibbar

## 0.6.373
- fix(viewport) #flzq: Fenster-Resize setzt die Kamera nicht mehr zurück — die aktuelle Ansicht (Position, Ausrichtung, Zoom) bleibt beim Ändern der Fenstergröße erhalten

## 0.6.372
- fix(export): Export-Dialog aufgeräumt: Spinner während des Meshens; die Body-Report-Tabelle ist transponiert (Attribute als Zeilen, ein Body je Spalte) und läuft nicht mehr horizontal über — die lange Mesh-Bezeichnung umbricht jetzt in der Zelle; die Vorschau-Einstellungen (Crossfade SDF↔Mesh, Wireframe) werden in localStorage gespeichert

## 0.6.371
- feat(export): Zwei bisher nur im Mesh-Viewer verfügbare Verfahren sind jetzt auch für den Export wählbar: Manifold DC (viewport) — das uniforme Manifold Dual Contouring des Viewports mit Planar-Face-Simplifikation; und Reach for the Arcs (Sellán et al. 2024) — Kugeln → Tangentenpunkte → Screened-Poisson-Rekonstruktion, robust auf unscharfen/unvollständigen Feldern (interne Auflösung gedeckelt). Damit deckt der Export-Dialog alle vorhandenen Rust-Mesher ab

## 0.6.370
- feat(export): Zwei weitere Export-Meshing-Verfahren im Dropdown: Uniform Dual Contouring (QEF) — dichtes Gitter mit QEF-Vertex-Platzierung: watertight by construction UND kantentreu (behält scharfe Kanten), nicht-1-Lipschitz-sicher, dann QEM-Decimation (der Allrounder für scharfkantige Körper). Und Marching cubes (robust) — Marching Tetrahedra: ambiguitätsfreie Fälle liefern watertight + 2-manifold by construction für JEDES Feld (die robuste Basislinie, u. a. watertight beim Gyroid, wo das Intervall-DC ein paar non-manifold-Kanten ließ). Die Report-Zeile zeigt jetzt das Verfahren als Klartext-Label

## 0.6.369
- feat(export): Export-Vorschau: ein Schieberegler blendet das Mesh-Solid transparent über den SDF-gerenderten Körper (Crossfade von SDF ↔ Mesh), und eine Checkbox zeigt das Mesh-Wireframe. So lassen sich Mesh und Original direkt überlagern und die Tessellierung inspizieren, solange der Export-Dialog offen ist

## 0.6.368
- feat(export): Viertes Export-Meshing-Verfahren: „Interval-octree DC (fidget)“. Übersetzt das Körperfeld in einen fidget-Ausdrucksbaum und meshet es per Intervall-Arithmetik-Octree + Manifold Dual Contouring: die Zell-Unterteilung wird per Intervall-Schranken auf dem echten Ausdruck zertifiziert (keine Lipschitz-Annahme → topologie-sicher für nicht-1-Lipschitz-Felder) und QEF bewahrt scharfe Kanten ohne B-Rep. Watertight auf glatten closed-form-Körpern und sehr genau (p95 ~0,01 mm); an scharfen/dichten Feature-Kanten hinterlässt das adaptive DC ein paar non-manifold-Kanten (Report zeigt es) — für garantiert watertight Surface Nets nutzen. Nur closed-form-Körper; Sketch-/Daten-Carrier fallen auf Surface Nets zurück

## 0.6.367
- fix(ui): Dialoge (Feature-, Export-, Report-Dialog, Measure-Panel) heben sich mit einem etwas kräftigeren, zweilagigen Schlagschatten (neues --shadow-dialog-Token) klarer vom Viewport ab; der Feature-Dialog hatte bisher gar keinen Schatten

## 0.6.366
- feat(export): Export-Dialog hat ein Dropdown für das Meshing-Verfahren. Neben dem bisherigen (Adaptive Dual Contouring) gibt es Surface Nets + Decimation: ein dichtes Gitter wertet jede Zelle aus (keine Lipschitz-Annahme) und findet so die Topologie nicht-1-Lipschitz-Geometrien (Gyroid, TPMS, dichte Helix-Windungen) zuverlässig; anschließend dezimiert QEM-Kantenkollaps das Mesh watertight-erhaltend. Zwei Umsetzungen wählbar: „library“ (fast-surface-nets + baby_shark) und „built-in“ (eigene, abhängigkeitsfreie Implementierung). Der Report zeigt die Dreiecks-Reduktion (dicht → dezimiert)

## 0.6.365
- feat(export): Export-Dialog zeigt eine Solid-Vorschau des gemeshten Objekts: nach „Mesh" wird im Viewport das exakte Export-Mesh (flat-shaded) statt des normalen Renderers angezeigt — solange der Dialog offen ist. Der Dialog ist jetzt non-modal und am Titelbalken verschiebbar, sodass man frei um das Mesh herum navigieren kann; beim Schließen übernimmt wieder der Original-Renderer

## 0.6.364
- fix(export) #0xir: Export-Dialog zweistufig: erst Zellgröße setzen und mit „Mesh" bestätigen, dann meshen und Speichern — das (bei Gyroiden/Blends teure) Meshing startet nicht mehr automatisch beim Öffnen und nicht bei jedem Tastendruck; Zellgröße ändern verwirft das alte Mesh und verlangt erneutes Bestätigen

## 0.6.363
- feat(export): Export-Meshing-Restposten: getrimmte Kugel-Patches tessellieren parametrisch (synthetisches Halbkreis-Profil speist die Rotations-Maschinerie), Selbstschnitt-Prüfung (Dreieck-Dreieck über Grid-Hash) und Dünnwand-Erkennung (Sub-Zellen-Features zwischen den Abtastebenen) im Export-Report mit Dialog-Warnungen; breiteres Narrow-Band für nicht-Lipschitz-Felder

## 0.6.362
- feat(export): Adaptives generisches Export-Meshing (Manifold Dual Contouring nach Schaefer/Ju/Warren auf Narrow-Band-Octree): topologie-erhaltendes Vertex-Clustering + konformes Sag-Refinement + Geisterkanten-Filter fürs Pinning — die hohle Vase exportiert mit 100k statt 1,5 Mio. Dreiecken, watertight, 0,07 mm Abweichung; Blend-Körper (SDF-only) 2.100 Dreiecke bei 0,05 mm

## 0.6.361
- fix(export): Generisches Export-Meshing gehärtet (Meshing Phase 1a/1b): MDC-Quad-Winding an scharfen Kanten an der Wurzel gefixt (geometrische Orientierung statt Achsen-Tabelle), Simplifikation deterministisch (kein Watertight-Würfeln mehr zwischen Läufen) mit sauberem Loch-Bridging, und B-Rep-Kanten-Pinning: Kanten-Vertices landen exakt auf den echten Kantenkurven

## 0.6.360
- feat(export): Parametrischer Export für Revolve-Körper: Spline-/Arc-Profile (RevolvedProfile-Carrier) tessellieren direkt aus dem B-Rep — die Vase exportiert mit 9.700 statt 1,5 Mio. Dreiecken; Kuppeln (Profil endet auf der Achse) über Pol-Fächer; generisches Sag-Refinement ersetzt die Winkel-Heuristik

## 0.6.359
- feat(export): Parametrischer Mesh-Export (Meshing Phase 2): analytisch weavbare Körper (Box/Zylinder/Kugel/Kegel/Torus-Booleans) tessellieren direkt aus dem B-Rep — watertight by construction, exakte Kanten, drastisch weniger Dreiecke (Zylinder: 252 statt 28.000); automatischer Fallback aufs generische Meshing, Report zeigt die Route pro Körper

## 0.6.358
- feat(export): STL/3MF-Export komplett neu: eigener toleranz-getriebener Mesh-Pass (Zellgröße einstellbar, unabhängig von der Viewport-Auflösung) mit Watertight-Validierung, Orientierungs-Reparatur und Qualitätsreport im neuen Export-Dialog

## 0.6.356
- fix(sketch): Einen selektierten Linien-Endpunkt oder eine Rechteck-Ecke zu ziehen verschiebt jetzt nur diesen Griff (bzw. hält die Gegenecke fest), statt die ganze Form zu verschieben — der Endpunkt ist ein eigenes Punkt-Entity, das der Drag-Planer nun als Griff der selektierten Kurve erkennt

## 0.6.355
- fix(kernel): Ein NaN aus dem Sketch-Solver kann den Kernel nicht mehr abstürzen lassen — der B-Rep-Weave prüft die Geometrie vorab und liefert bei nicht-endlichen Koordinaten ein leeres B-Rep statt zu paniken

## 0.6.354
- fix(step): STEP-Export und -Import können nicht mehr gegenseitig ihre Antworten kapern (Antworten werden per Request-Id zugeordnet statt per Nachrichtentyp); Fehler aus den Mesh-/Weave-/STEP-Workern werden gemeldet statt als leeres Ergebnis maskiert

## 0.6.353
- fix(viewport): Picking/Selektion kullt Punkte hinter der Kamera jetzt korrekt (Box-Select, Kanten-Snap, Vertex-Pick, Zoom-to-fit-Sichtbarkeit) — die NDC-Tiefenprüfung nutzte die WebGL- statt der WebGPU-Clip-Space-Konvention

## 0.6.352
- fix(sdf-vm) #yl0p: SDF VM rendert Körper jetzt auch in der orthografischen Ansicht (Default) geshadet — der Ray-Origin wurde mit WebGL- statt WebGPU-Clip-Space rekonstruiert und lag ~far hinter der Kamera

## 0.6.351
- fix(sketch) #mdye: Überstehende/überschießende Linien verhindern keine geschlossene Fläche mehr — ein Rechteck mit Fahnenmast-Überstand wird korrekt grau gefüllt (und ist extrudierbar)

## 0.6.349
- feat(accounts): Benutzerkonten mit Login/Logout — FieldDraft ist jetzt mehrbenutzerfähig
- feat(accounts): Registrierung nur per Einladungs-Link (Invite-only)
- feat(accounts): Anmeldepflicht in der ausgelieferten App; die Anmeldung bleibt über Reloads erhalten
- feat(invites): Einladungen im Admin-Bereich: Name, E-Mail und Einladungstext, mit teilbarem Registrierungs-Link
- feat(invites) #dqmh: Selbst-Registrierung über den Invite-Link mit vorausgefüllter E-Mail-Adresse
- feat(projects): Cloud-Projekte: Browser für eigene (privat/öffentlich) Projekte mit Filter — anlegen, umbenennen, duplizieren, löschen und Sichtbarkeit umschalten
- feat(projects): Öffentliche Galerie: freigegebene Projekte anderer ansehen und als Kopie öffnen
- feat(projects) #yoyz: Projekt-Titel in der Topbar per Klick editierbar
- feat(issues): Issue-Tracker im GitHub-Issues-Stil: Fehler und Wünsche per „Report Issue“ melden (inkl. Screenshot und Projekt), Issues-Liste mit Filter „nur meine“ und Status, Detailansicht mit Kommentar-Thread
- feat(admin): Admin: Benutzerverwaltung (Sperren/Entsperren) und ein Hinweis-Glöckchen für sicherheitsrelevante Ereignisse
- feat(ui): Neue Navigation: persistente Topbar, Tab-Übersicht (Projekte/Issues/Invites/Users) und ein 👤-Benutzermenü mit „Report Issue“
- fix(ui) #f5d8: Perf-Overlay togglet auf F9 (statt F2) und hat jetzt einen Schließen-Button

## 0.6.348
- fix(mate) #zcwn: Mate-Connector zeigt kein Translate/Rotate/Scale-Gizmo mehr — er wird über Anker & Offset positioniert, nicht per Freie-Transform

## 0.6.347
- feat(mate): Mate-Connector-Realignment: Origin entlang eigener X/Y/Z-Achsen verschieben (Offset)
- feat(mate): Mate-Connector zwischen zwei Elementen platzieren (2. Anker → Mittelpunkt)
- feat(mate): Mate-Connector einem Owner-Part zuordnen

## 0.6.346
- feat(extrude) #k280: Extrude: mehrere coplanare Flächen im Profiles-Feld auswählen — Profiles ist jetzt ein Multi-Picker (Flächen sammeln sich an), das separate Faces-Feld entfällt

## 0.6.345
- fix(release-notes): Versionsnummer in der Titelzeile als Link erkennbar (dezente Unterstreichung) — Klick öffnet die Release Notes

## 0.6.343
- feat(release-notes): Release Notes — pro Version erledigte Features & Fixes, geöffnet übers Menü oder per Klick aufs Versions-Badge, mit Filter nach Stichwort/Ticket

## 0.6.342
- feat(extrude) #20fc: Seitenfläche als Profil extrudieren — qFace im Profiles-Feld

## 0.6.341
- fix(extrude) #r2zx: „Profile half x/y" ausblenden + klare Fehlermeldung bei Face-Profil

## 0.6.340
- feat(sketch) #32az: Constraint-Tool-Kette — alle nacheinander geklickten Entities verketten

## 0.6.339
- fix(sdf-deferred) #c9ic: Extrude-Tiefenänderung aktualisiert den Solid

## 0.6.338
- fix(mate-gizmo): screen-space Stroke-Width statt 3D-Röhren

## 0.6.337
- fix(new-project) #vy3t: leere Szene nach „New" sofort räumen

## 0.6.336
- feat(mate): Shortcuts Ctrl+M / K / Shift

## 0.6.335
- feat(mate): CSYS-Redesign — Kante=Primärachse, Reorient-Toggle, Anker ohne Pflicht-Fläche

## 0.6.334
- fix(mate): Gizmo + Inferenz auf OVERLAY_LAYER — im Deferred-Modus sichtbar

## 0.6.333
- feat(mate): Connection-Point-Inferenz + Vertex/Kanten-Anker

## 0.6.332
- feat(mate): Mate-Connector-Gizmo — Triade + Drehkreis + gefüllter Sektor

## 0.6.331
- fix(ss-csg) #sa0k: SS-CSG-Tiefe ortho-tauglich — Kugel im Box-Inneren wieder verdeckt

## 0.6.330
- fix(ss-csg) #mlv3: WebGL Screen-Space-CSG nicht mehr dunkel — Live-Lichtrig + ACES/sRGB-Parität

## 0.6.328
- fix(brep-overlay) #1do9 #zhfy: Kanten sofort beim Laden zeigen — Demand-Mode-Repaint nach Async-Weave-Import

## 0.6.327
- fix(sdf-deferred) #87bf: Durchgangsloch in box-minus-Bohrung sichtbar — Box-Leaf marschieren bei Nicht-Union-Body

## 0.6.326
- fix(sdf-vm) #o6ia: Körper rendern wieder — setLayout-Eval revertiert

## 0.6.325
- feat(thicken): Thicken als Toolbar-Tool — Flächen anklicken wie Fillet

## 0.6.324
- fix(thicken) #iz3p: Feature-Kind klassifizieren — Detail-Panel zeigt Thicken-Params

## 0.6.323
- fix(sdf-vm) #21ws: VM-Shader-Compile von >1min auf ~210ms — SDF-Eval als eine WGSL-Funktion via setLayout

## 0.6.322
- fix(sdf-deferred) #g2ox: Durchgangsloch ortho see-through — March-Ray ortho-aware

## 0.6.321
- fix(settings): "(experimentell)" aus Screen-Space-CSG-Label entfernen

## 0.6.320
- feat(settings): UI-Tab in Gruppen ordnen + Screen-Space-CSG nur in WebGL zeigen

## 0.6.319
- fix(webgl) #dkaa: DC-Boolean-Mesh flach shaden — Schmier-Dreiecke an Hartkanten weg

## 0.6.318
- fix(sdf) #6yin: Körper wieder voll rendern statt gesliced — Single-Union-March wiederhergestellt

## 0.6.317
- feat(feature-dialog): edit boolean op + target on primitives

## 0.6.316
- feat(viewport): "View Normal to Face" Kontextmenü-Eintrag für planare Flächen (Ticket z0vk)

## 0.6.315
- feat(camera): Perspektive-Schalter in Settings; Default = Orthografisch (Ticket m62b)

## 0.6.314
- feat(extrude) #yx5h: One-sided + New defaults, render created body solid

## 0.6.313
- fix(sdf) #deeg #9ljc: separate Bodies nicht mehr implizit boolesch (min-Union) rendern — Nearest-Hit pro Body

## 0.6.312
- fix(weaver): Async-Weave-Import löst Re-Render aus — frisch erzeugte Körper zeigen sofort BRep-Kanten

## 0.6.311
- fix(sketch): Snap-Radius zoom-unabhängig (Pixel statt fix-mm) — Polygon schließt wieder

## 0.6.310
- fix(webgl-csg): SS-CSG-Pass blockierte Szenen-Render — Datum/Sketch wieder sichtbar + Render-Labels

## 0.6.309
- fix(webgl-csg): Nicht-konvexer Subtrahend (Torus) — 2-Layer-Depth-Peel gegen Durchscheinen (Ticket yt4o)

## 0.6.308
- feat(webgl-csg): Schnittflächen-Shading an Szenen-Lichtrig angleichen (Parität)

## 0.6.307
- feat(webgl-csg): Intersection (A∩B) im Screen-Space-CSG (Stage 1)

## 0.6.306
- feat(webgl-csg): Geshadetes A−B mit G-Buffer + Tiefen-Komposition (Stage 1)

## 0.6.305
- feat(webgl): Reine Unions parametrisch rendern statt lumpiges DC-Mesh

## 0.6.304
- feat(webgl-csg): Screen-Space-CSG Stage-1 (opt-in, experimentell) — Achse 3 Phase 3

## 0.6.303
- feat(brep-seed): Graceful Boolean-Seed-Weave statt All-or-Nothing-Octree-Fallback

## 0.6.302
- fix(brep-octree): Lipschitz-Guard mit march_relax skalieren — Revolve-Union nicht mehr degeneriert

## 0.6.301
- feat(deep-link): ClaudeDraft "open .fdraft" Tab-Loader (?project=)

## 0.6.300
- fix(report-dialog): Report-Window oben-links unter der Toolbar verankern

## 0.6.299
- fix(sdf-deferred): Identische exakte Meshes + Z-Offset statt Inflation

## 0.6.298
- fix(sdf-deferred): Konkave Speichen-Wellen per Normal-Offset statt Uniform-Scale

## 0.6.297
- feat(sdf-vm): marchRelax auch im VM-Raymarch anwenden

## 0.6.296
- fix(sketch-overlay): Hover gibt nicht mehr die noch gerenderten Basis-Linien frei

## 0.6.295
- fix(overlays): Hover-Edge/Face-Geometrie gedeferrt freigeben — sdf-vm Buffer-Race

## 0.6.294
- feat(camera): Zoom/Dolly zum Mauscursor hin

## 0.6.293
- feat(viewcube): Standard-Ansichten framen die Bounding-Sphere statt Ursprung

## 0.6.292
- feat(viewport): Boden-Gitter entfernt

## 0.6.291
- feat(camera): Onshape-Style unbegrenzte Rotation statt OrbitControls

## 0.6.290
- fix(webgl-lights): nicht-degeneriertes Light-Target + per-Frame View→World-Rotation

## 0.6.289
- feat(webgl-lights): WebGL-Lichter in Camera-Space wie SDF-Renderer

## 0.6.288
- feat(sketch) #matf #o6hx: Drag aller Entity-Typen + Mirror-Hover/Pick/GPU-Race

## 0.6.287
- feat(sketch) #uf62: Coincident-Tool typabhängig — Punkt/Kurve/Linie/Kreis

## 0.6.286
- feat(sketch) #fkig: Linien-Endpunkte snappen an Kreise/Bögen

## 0.6.285
- fix(sketch) #rxi2: Trim löscht keinen ganzen Kreis mehr bei <2 Schnittpunkten

## 0.6.284
- fix(detail) #6sjq: Variablenname in Zahlenfeld tippen schließt Detail nicht mehr

## 0.6.283
- fix(sidebar) #v1a7: Variables-Panel auch im Sketch sichtbar — rechte Sidebar identisch

## 0.6.282
- feat(measure) #me9w: Vertices + Flächen messbar, alle Distanz-Kombinationen

## 0.6.281
- fix(query) #2ukj: Profil-Query schließt eigene/spätere Output-Körper aus

## 0.6.280
- fix(sketch) #6b41: bogen-berandete Regionen (Linie+Arc) als Profil pickbar

## 0.6.279
- fix(extrude) #tgdg: Profil-Query je Region statt ganzem Sketch + Forward-Ref-Filter

## 0.6.278
- fix(sketch) #4anf: Szene-Klick kontextabhängig routen, Sketch bleibt offen

## 0.6.277
- fix(webgl) #n6rs #gsoj: keine Node-Materialien mehr im klassischen Renderer

## 0.6.276
- feat(sketch-project) #jcb7: assoziative B-Rep-Kanten-Projektion in den Sketch

## 0.6.275
- fix(revolve) #ni56: unvollständiger Revolve ist invalid + Warnung statt Garbage-Körper

## 0.6.274
- feat(variables) #buy9 #nlij: ein smartes Wert-Textfeld pro Variable — Einheit/Formel implizit, kein ƒx/Typ-Selektor

## 0.6.273
- feat(sketch-mirror) #ps21: Pick-to-apply statt „Enter wendet an"

## 0.6.272
- fix(sketch) #t11x: direkter Maus-Down+Drag greift Sketch-Punkt sofort

## 0.6.271
- fix(webgl-viewcube) #10cl: ViewCube-Klicks & View-Commands im WebGL-Modus

## 0.6.270
- fix(webgl-gizmo) #7hk3: Move-Gizmo im WebGL-Renderer sichtbar + interaktiv

## 0.6.269
- feat(revolve) #dhjn: Revolve-Profil per geschlossener Sketch-Region statt ganzem Sketch

## 0.6.268
- feat(webgl) #814w: allgemeines Picking/Selektion/Hover/Measure im WebGL-Modus

## 0.6.264
- fix(sdf-deferred): vlhp-Speichen — opake Proxy-Position per Gradient-Newton statt View-Ray polieren

## 0.6.263
- fix(sdf-deferred) #vlhp: Revolve-Unterseite-Speichen — opake Proxy-Position auf echten SDF polieren

## 0.6.262
- fix(sketch) #vwzj: Vertex-Dots beim Endpunkt-Drag nicht mehr neu erzeugen

## 0.6.261
- feat(measure) #stv8: entity-basiertes Measure-Tool mit schwebendem Panel

## 0.6.260
- fix(sdf-deferred) #sgzw: destruktiven Rebuild bis GPU-Idle deferren

## 0.6.259
- feat(transform): Move-Modus — Klick auf Körper legt/reused Translate-Feature

## 0.6.258
- feat(sketch): schwebende Tool-Hint + Mirror-axis-first + Move-click-drag

## 0.6.257
- fix(view) #xw51: ViewCube/View-Commands — Hit-Testing + Consumer ohne Controls

## 0.6.256
- fix(sketch) #pr8a: Rechteck-/Polygon-Kanten als Revolve-Achse wählbar

## 0.6.255
- fix(sketch) #ybnl: 3-Punkt-Kreis behält die 3 Punkte als verschiebbare Entities

## 0.6.254
- fix(sketch) #4peq: H/V/Parallel-Snap winkelbasiert statt distanzbasiert

## 0.6.253
- fix(sketch) #inej: Snap auf eine Linie erzeugt ein persistentes Constraint

## 0.6.252
- feat(sketch) #d9o9: neuer Sketch erscheint sofort als Feature

## 0.6.251
- fix(sketch) #rkez: Context-Menu „Confirm Sketch" schließt den Detail-Dialog

## 0.6.250
- feat(variables) #8ar9 #eil: Typ (Number/Length/Angle) + Einheiten

## 0.6.249
- feat(params) #x2m7: Variablen/Formeln in number- und vec-Feldern nutzbar

## 0.6.248
- feat(variables) #8ar9 #eil: Variablen umbenennbar

## 0.6.247
- fix(material/outliner) #o9ul: Bodies-Query filtert auf BODY + Detail-Header zeigt prettified Namen

## 0.6.246
- feat(report) #ttzl: Report-Dialog nicht-modal, verschiebbar und minimierbar

## 0.6.245
- fix(kernel) #c990: bodyHybridInfo darf bei Out-of-Range-Index nicht panicken

## 0.6.243
- fix(sdf-deferred) #sgzw: synchronen GPU-Dispose im Daten-Update deferren

## 0.6.242
- feat(shortcuts): F2 öffnet den Report-Issue-Dialog

## 0.6.240
- fix(sdf-deferred) #ceat #0b8m #a72n: Revolve-Proxy-Mesh schließen — Loch verursachte deferred Inside-out

## 0.6.239
- fix(dev) #4kky: Auto-Reload-Off greift jetzt auch bei Server-Neustart

## 0.6.238
- feat(controls) #8ikq #eil: FreeCAD-Preset ans Wiki angeglichen + SolidWorks ergänzt

## 0.6.237
- fix(sdf-deferred) #ceat #awq6: expliziter Per-Frame-Canvas-Clear gegen "Nachziehen"

## 0.6.236
- feat(controls) #8ikq #eil: Blender-Preset ergänzt

## 0.6.235
- feat(controls) #8ikq #eil: Three.js-Preset — Shift+LMB-Drag = Box-Select

## 0.6.234
- feat(controls) #8ikq #eil: Onshape als Default-Preset, FieldDraft-Preset entfernt

## 0.6.233
- feat(settings) #cztw: Toggle für Auto-Reload bei Codeänderung

## 0.6.232
- fix(camera) #xv9c: Welt-Gitter nicht mehr von der near-Plane abgeschnitten

## 0.6.231
- fix(camera) #s8l0: View Normal to Sketch Plane respektiert Kamera-UP

## 0.6.230
- fix(viewport) #qi81: Rechts-Drag öffnet kein Context Menu mehr — nur Rechts-Klick

## 0.6.229
- fix(sdf-deferred): Außen-Orientierung für alle Extrusionsarten generalisieren — Sweep/Helix/Revolve

## 0.6.228
- fix(sdf-deferred): Revolve-Mesh inside-out — Lathe-Winding+Normalen generisch nach außen zwingen

## 0.6.227
- perf(sdf-deferred): Revolve/full-SDF-Körper marschieren echtes Leaf-Mesh statt AABB-Box

## 0.6.226
- fix(sdf) #323i: Pipeline-Teardown deferred disposen — "Buffer used while destroyed"

## 0.6.225
- fix(sdf-deferred) #p8dp: Grid + Datum-Planes fehlten — auf OVERLAY_LAYER ziehen

## 0.6.224
- feat(sketch) #zadt: Hover-Highlight für Sketch-Entities im Select-Modus

## 0.6.223
- fix(outliner): Sketch-Detail-Header zeigt editierbaren Namen + ✓/✗ schließen mit einem Klick

## 0.6.222
- fix(mobile) #qh4l: drei identische ☰ entzerren — Panel-Chevron rechts, Toolbar zeigt aktives Tool

## 0.6.221
- feat(viewport,sketch): 3D-Rechtsklick-Kontextmenü + editierbare Bemaßung mit Einheiten

## 0.6.220
- feat(toolbar) #apr3: Toolbar wird auf Mobile-Fenstergröße zu einem Menü

## 0.6.219
- fix(kernel) #a204 #ifn2: achsen-ausgerichteter Revolve war ein LEERER SDF-Solid — Profil auf +x spiegeln

## 0.6.218
- fix(render) #ngj1 #a204 #ifn2: Revolve-Mesh bei achsen-ausgerichtetem Profil mit negativem x korrekt rendern

## 0.6.217
- fix(webgl) #6vdf: Revolve-Achse als Sketch-Linie im WebGL-Renderer pickbar

## 0.6.216
- fix(render) #fg9o: Achsen-Pick-Freeze im Default-Renderer (sdf-deferred) wirklich beheben — GPU-Disposal an onSubmittedWorkDone koppeln

## 0.6.215
- fix(sdf-vm) #fg9o: GPU-Ressourcen verzögert freigeben — kein Einfrieren nach Achsen-Pick

## 0.6.214
- feat(sketch) #7lxk: Dimension-Tool zeigt Maßlinien + Linien-Hover

## 0.6.213
- fix(sketch) #pgyf: Snap auf vorhandene Sketch-Linien (Punkt-Tool + Linien-Endpunkt)

## 0.6.212
- fix(sketch) #8vpe: Spline-Preview folgt dem Cursor während des Drags

## 0.6.211
- feat(sketch) #daa8: Line-Tool als verkettetes Rubber-Band (Polylinie)

## 0.6.210
- fix(sketch) #czqf: Mirror-Tool — Achsen-Linie per Klick wählbar + Hover-Highlight + Hinweis

## 0.6.209
- feat(sketch) #tqi4: Fillet/Chamfer per Einzelklick auf Eckpunkt — auch bei Polygon/N-Gon/Rechteck

## 0.6.208
- fix(sketch) #egia: Spline-Tool verlassen FINISHT die Spline statt sie zu verwerfen

## 0.6.207
- fix(sketch) #8vb5: N-Gon-Outline rendern — verschwindet nicht mehr beim Loslassen

## 0.6.206
- feat(report): Action-Log loggt jetzt ALLE Zeichen + Feldwerte (Dev-Phase, kein Datenschutz-Filter)

## 0.6.205
- fix(sketch) #94jk: keine BRep-Kanten/Flächen-Hover im Sketch-Modus

## 0.6.204
- feat(report): User-Aktions-Log fürs "Report Issue" — replaybare Schritt-Spur

## 0.6.203
- fix(query) #e9il: Single-Query-Picker nach Pick freigeben (one-shot) — kein "stuck"-Zustand mehr

## 0.6.202
- fix(render) #7kht: Szenen-Hintergrund wieder weiß — ACES-Tonemapping deaktiviert

## 0.6.201
- fix(query) #fs23: Klick im Szene-View bei fokussiertem Query-Feld selektiert statt Detail zu schließen

## 0.6.200
- fix(ui) #gqjt: redundante Feature-Titelzeile aus dem Detail-View entfernen

## 0.6.199
- feat(sketch) #zz1r: Constraints als Tools mit Labels in der Tool-Zeile, tool-first Workflow

## 0.6.198
- feat(ui) #f7xk: Detail-Header zeigt editierbaren Feature-Namen statt "Detail"

## 0.6.197
- fix(webgl) #jnej: BRep-Kanten im WebGL-Renderer darstellen

## 0.6.196
- fix(sketch) #imqf: committeter sichtbarer Sketch in Grau statt Blau

## 0.6.195
- feat(revolve/helix) #dnfe: Warnung wenn die Achse nicht in der Sketch-Ebene liegt

## 0.6.194
- feat(brep) #exus: Helix-End-Flächen selektierbar — Deckel als analytische Plane-Faces + Provenance

## 0.6.192
- fix(brep) #1t40: Helix/Sweep-Längskanten als glatte NURBS statt Polyline

## 0.6.191
- fix(camera) #lbzq: adaptive Near/Far-Clip — Körper verschwinden beim Wegzoomen nicht mehr

## 0.6.190
- feat(brep) #ifog #eil: konstruktive Helix- & Sweep-Kanten (Profil-Ecken durch die Bewegung tracen)

## 0.6.189
- feat(brep) #ifog #eil: konstruktive Revolve-Kanten — Profil-Ecken zu Kreisen tracen

## 0.6.188
- fix(brep) #ifog: Extrude-Seitenkanten bei schrägen Profilwänden (line_point-Determinante)

## 0.6.187
- feat(report) #uz5q: UI-State als ui_state.json im Ticket mitspeichern

## 0.6.186
- fix(sketch) #sc8g: committeter Sketch zeichnet auch Splines — schließt Spline-Loop

## 0.6.185
- fix(schema): entity_filter erreicht die Picker wieder (snake_case + multi_query)

## 0.6.184
- fix(picker): Query-Felder akzeptieren nur Picks ihres entity_filter

## 0.6.183
- feat(helix): Achsen-Query (Sketch-Linie ODER BRep-Kante) wie bei Revolve

## 0.6.182
- fix(webgl) #fdzj: sichtbarer Sketch zeigt auch Linien+Fill, nicht nur Eckpunkte

## 0.6.181
- feat(revolve) #b8qh: Achsen-Query (Sketch-Linie ODER BRep-Kante) statt fixer Y-Achse

## 0.6.180
- fix(webgl) #owia: Maus-Preset auch im WebGL-Viewport (CameraRig statt blanker OrbitControls)

## 0.6.179
- fix(webgl) #x941: Sketch-Live-Preview, Sketch-Undo, parametrische Helix

## 0.6.178
- fix(report) #478k: Ticket-Settings als settings.json + project.fdraft als echtes ZIP

## 0.6.177
- fix(webgl) #uzhr: Sketch-Linien sichtbar + Plane-Pick aus Szene im WebGL-Modus

## 0.6.176
- fix(render) #qz52: SDF-Deferred zeigt Geometrie wieder + weißer BG korrekt

## 0.6.175
- feat(project) #wu2v: .fdraft als ZIP-Container mit Asset-Persistenz

## 0.6.174
- feat(sketch) #vk8q: Constraint-Toolbar Onshape-Style mit Icons + unified Dimension

## 0.6.173
- feat(sketch) #yuaw: Sketch-Toolbar einzeilig — Import als Tool, DoF rechts

## 0.6.172
- fix(sketch) #tdq2: Eckpunkt-Dots sichtbar — Mesh-Kreise statt nicht-rendernder Points

## 0.6.171
- fix(render) #a2ps: SDF-Szenen-Hintergrund weiß statt grau

## 0.6.170
- feat(report): Per-Device-Settings als JSON ins Ticket aufnehmen

## 0.6.169
- feat(sketch) #7hgr: Session-Lifecycle — sticky gegen Szene-Klicks, Extrude/Revolve in Toolbar, Delete/New-Project-Reset

## 0.6.168
- fix(sketch) #g91j: leeres Text-Input löscht den Text im Edit-Modus

## 0.6.167
- fix(sketch) #3btx: Plane-Feld wie generisches Query-Input + Plane-Pick aus Szene

## 0.6.166
- feat(sketch) #xqp3: weißer Szenen-Hintergrund + runde Eckpunkt-Dots in doppelter Linienbreite

## 0.6.165
- feat(report) #xqll: Report-Issue ohne Title, kein ticket.json mehr

## 0.6.164
- feat(sketch) #87a1: Sketch-Details als generischer Detail-Dialog

## 0.6.163
- feat(sketch) #n6hg: Eckpunkte/Vertices als Punkte visualisieren

## 0.6.162
- fix(sketch) #o2y3: geladene Sketch-Objekte beim Editieren selektier/löschbar

## 0.6.161
- fix(sketch) #lycy: Text-Tool Feinschliff — leeres Feld pro Klick, kein Leer-Objekt, Tooltip

## 0.6.160
- feat(sketch): committeten Text in-scene re-editierbar

## 0.6.159
- feat(sketch): Fillet-Radius/Chamfer-Distanz post-hoc in-scene editierbar

## 0.6.158
- feat(sketch): n-gon/Pattern/Rotate-Parameter in-scene

## 0.6.157
- feat(webgl) #gcvz: Booleans im WebGL-Renderer via DC-Mesh + Material-Parität

## 0.6.156
- feat(sketch) #pqoe #eil: Fillet/Chamfer/Offset-Eingabe in-scene + Plan-Doc für restliche Felder

## 0.6.155
- feat(sketch) #pqoe #eil: Text-Eingabe + Parameter in-scene statt Toolbar

## 0.6.154
- feat(sketch) #pqoe #eil: Gizmos ausblenden während aktivem Query-Pick

## 0.6.153
- feat(sketch) #pqoe #eil: explizite Constraint-Buttons + Query-Pick akzeptiert Sketch-Feature aus Liste

## 0.6.152
- feat(sketch) #pqoe #eil: Geometry-Accordion (collapsed) + Tool-Dropdowns für Circle/Rect-Gruppen

## 0.6.151
- feat(sketch) #pqoe #eil: Sketch ist ein Feature wie jedes andere — Selektion = Edit, ✓/✗ im Detail-Panel

## 0.6.150
- fix(pick) #md1e: Klick auf committeten Sketch selektiert den Sketch, nicht die Datum-Plane dahinter

## 0.6.149
- feat(sketch) #jjmv: committете Sketch-Darstellung exakt wie Sketch-Modus — stärkere Füllung + doppelte Stroke-Breite

## 0.6.148
- feat(sdf) #1wnm: per-Body raymarch-Step-Relax-Mechanismus

## 0.6.147
- feat(sketch) #jjmv: committете Sketch-Regionen gefüllt + als Profil pickbar

## 0.6.146
- feat(snapshot): erster/freistehender Add ohne Scope = implizit "New", keine Warnung

## 0.6.145
- fix(webgl) #59c2 #r8b4: Part-Farben + Default-Planes + ViewCube-Sync im WebGL-Renderer

## 0.6.144
- feat(snapshot): Scope ist Pflicht für Remove/Intersect/Hole — kein impliziter AABB-Effekt, nur Warnung

## 0.6.143
- feat(snapshot): Hover-Dimming + Pick-Reject-Feedback für späteres-Feature-Geometrie

## 0.6.142
- feat(boolean): expliziter Merge-Scope schlägt AABB — Subtract/Intersect treffen nur die gewählten Zielkörper

## 0.6.141
- feat(snapshot): UI-Auswahlfilter + Reorder-Guard — Features referenzieren nur frühere Features

## 0.6.140
- fix(boolean) #9wxs: Forward-Reference-Tools/Targets → klarer Feature-Error statt stillem No-op

## 0.6.139
- feat(features): Merge-Scope-Warnung — Add/Remove/Intersect ohne gültigen Scope rot markiert

## 0.6.138
- fix(kernel): berührende Solids verschmelzen nicht mehr automatisch — Merge nur per explizitem Link

## 0.6.137
- feat(sweep): volles interaktives Drag-Handle-3D-Gizmo für Gizmo3d-Sweep-Pfade

## 0.6.136
- feat(sweep): Gizmo3d-Pfad per Toolbar — "Sweep 3D" Dropdown

## 0.6.135
- feat(sweep): Orthogonal-Sketches-Provider — Zwei-Ansichten-Raumkurve

## 0.6.134
- feat(sweep): EdgeChain-Pfad-Provider — Sweep entlang Modell-Kanten

## 0.6.133
- feat(sweep): offene Pfad-Skizzen — Sweep entlang A→B-Kurve

## 0.6.132
- feat(sweep): Twist — Profil rotiert entlang des Spines

## 0.6.131
- feat(sweep): GPU-VM-Opcodes — Sweep im sdf-deferred/vm sichtbar

## 0.6.130
- feat(sweep): Toolbar-Button + add_sweep-API — Sweeps per UI erzeugbar

## 0.6.129
- feat(sweep): Sketch2d-Pfad-Provider + getaggte PathSourceDef

## 0.6.128
- feat(sweep): SweepFeature auf gekrümmten Pfad via Gizmo3d-Provider

## 0.6.127
- feat(sweep): Gatekeeper-gegateter parametrischer Sweep-Leaf

## 0.6.126
- feat(kernel): CsgNode::Sweep — gekrümmter Sweep als echtes Kernel-Primitiv

## 0.6.125
- feat(kernel): modulare PathProvider-Schicht für Pfad-Quellen

## 0.6.124
- feat(kernel): curved-Sweep CPU-SDF — Nearest + Profil im RMF-Frame + Cap-Komposition

## 0.6.122
- feat(webgl): RMF-Sweep-Mesher + geteilter Shell-Assembler

## 0.6.121
- feat(kernel): Sweep-Gatekeeper — Capsule-BVH Pre-Flight für parametrische Sweeps

## 0.6.120
- feat(webgl): Twist/Bend als Domain-Warp-Meshes — Vertex-Deformation statt AABB

## 0.6.119
- feat(webgl): parametrische Helix-Meshes — Sweep-Profil statt AABB-Box

## 0.6.118
- fix(viewport): kein manuelles Resizen mehr nötig; StrictMode teilw. verschoben

## 0.6.117
- feat(render): funktionierender WebGL-Render-Pfad ohne WebGPU

## 0.6.116
- feat(render): Hybrid-Renderer Phase 2 — getrimmte Zylinder-Faces als Raster-Band

## 0.6.115
- feat(render): Hybrid-Renderer Phase 1 — planare B-Rep-Flächen als exakter Raster-Mesh

## 0.6.114
- feat(telemetry): GPU-Adapter + Steady-State-FPS-Telemetrie; Konzept adaptives Rendering

## 0.6.113
- fix(viewport): klare WebGPU-Fehlermeldung statt kaputtem WebGL-Fallback

## 0.6.112
- feat(sketch): Maß-/Constraint-/Snap-Labels als 3D-Billboards statt 2D-Overlay-Text

## 0.6.111
- feat(sketch): Drag-Previews, Hover, Snap-Ring, projizierte Kanten → 3D — Overlay zeichnet nur noch Text+Marquee

## 0.6.110
- feat(sketch): Selektions-Highlight + Grips → 3D-Scene-Geometrie

## 0.6.109
- fix(sketch): Overlay-HUD synchron zum Render-Frame zeichnen — kein Nachlaufen mehr von Snap/Hover/Grips/Selektion/Bemaßung

## 0.6.108
- feat(sketch): alle committeten Sketch-Geometrien als echte 3D-Scene-Geometrie — Overlay strokt keine Geometrie mehr

## 0.6.107
- feat(sketch): committed Kern-Geometrie als echte 3D-Scene-Geometrie statt 2D-Overlay-Reprojection

## 0.6.106
- feat(helix) #vfqh #15f5: Onshape-Parameter — Input-Modi, Height, Start-Winkel + Merge scope für Helix/Extrude/Revolve

## 0.6.105
- fix(helix) #vtyh: kontinuierlicher Sweep-Parameter-Clamp — kein degeneriertes Band am Helix-Ende

## 0.6.104
- feat(sketch): dezente Linien-Bemaßung mit Klick-zu-Edit statt mitfahrendem Eingabefeld

## 0.6.103
- perf(octree): Geschwister-Zellen teilen Corner-Evals über das 3×3×3-Gitter

## 0.6.102
- feat(kernel): Helix-Feature — Sketch-Profil helikal sweepen für echte Schraubgewinde

## 0.6.101
- feat(extrude): Sketch-Extrudes als Pending-Ghost mit Sketch-Plane-Anker

## 0.6.100
- feat(brep): F0-1 — Face pro zusammenhängender Grenzregion statt pro Branch

## 0.6.99
- feat(brep): asynchroner Overlay-Weave — Weaver-Worker webt off-thread, Main-Kernel importiert

## 0.6.98
- feat(extrude): Live-Extrude-Drag — Tiefe committet throttled während des Drags

## 0.6.97
- fix(viewport): B-Rep-Overlays während Gizmo-Drag komplett ausblenden

## 0.6.96
- feat(perf): BRep-Weave-Unterdrückung während Gizmo-Drags + Worker-Dauerunterdrückung

## 0.6.95
- feat(ssi): nicht-koaxiale RevolvedProfile-Paare im 2D-Scan + Tracer-Fixes

## 0.6.94
- perf(protocol): Lower-Bound-Pruning im Carrier-Closest-Point

## 0.6.93
- fix(brep): Extrude-Kreiswand-Carrier um 90° auf die Extrusionsachse gedreht

## 0.6.92
- feat(kernel): retained-boundary Open-Shell — konstante Wandstärke statt Messerkanten-Taper

## 0.6.91
- feat(brep): Shell-bewusster Seed-Weave — Kavitätskanten analytisch, Vase-Ringe exakt

## 0.6.90
- feat(brep): koaxialer SSI-Fall — Revolve-Kanten als exakte Kreise aus 2D-Profilschnitten

## 0.6.89
- feat(brep): HalfSpace::RevolvedProfile — Spline-Revolve-Wände analytisch in der Branch-Map

## 0.6.88
- feat(sdf-vm): Hemisphären-AO im SDF-Shading — Kavitäten lesbar statt Deckel-Illusion

## 0.6.87
- feat(kernel): adaptives Spline-Lowering — exakte Cubic-Béziers statt 16-Schritt-Sampling

## 0.6.85
- feat(sdf-vm): OP_PRIM2D_PATH — analytische Pfad-Segmente statt 2308-Vertex-Flattening

## 0.6.84
- fix(sdf-vm): POLY/SHELL-Loops im GPU-Interpreter dynamisch statt 32er-Cap

## 0.6.82
- fix(perf): Projekt-Load fror den Tab ein — Weave 6× schneller, Boot ohne Sync-Mesh/Weave, Weave-Cache über Regens

## 0.6.81
- fix(settings): Octree-Zellen-Overlay session-only — persistiertes 'true' fror den Tab beim Reload ein

## 0.6.80
- feat(brep): Phase 4 — Octree-Zellklassifikation auf Koinzidenz-Klassen

## 0.6.79
- fix(kernel): GPU-Path-Polygon schließt über die echte Lücke + Shell-Kind im Outliner

## 0.6.78
- fix(kernel): Revolve strippt Achsen-Segmente — Shell hinterlässt keinen Phantom-Stab

## 0.6.77
- feat(kernel): K7.1 Loop-Inferenz — freie Linien/offene Splines bilden Regionen

## 0.6.76
- feat(ux): Entf/Backspace löscht selektierte Sketch-Elemente

## 0.6.75
- fix(ux): Datum-Planes als Sketch-Plane pickbar + Query-Picker entschärft

## 0.6.68
- feat(viewport): BRep-Basiskanten 2.5px, Hover 3px

## 0.6.67
- feat(features) #sx5a #1rax: Onshape-artige Fehler-Toleranz im Regen-Loop + Extrude/Revolve ohne Selektion

## 0.6.66
- fix(viewport) #b91c: Selektions-/Hover-Darstellung Face-präzise statt Branch-weit

## 0.6.65
- feat(extrude) #x7af: Pending-Ghost bis zur Bestätigung + Negativ-Tiefe durch die Basisebene

## 0.6.64
- fix(viewport) #bqvz: Hover-Kanten 2.5px + helleres Orange, Face-Hover-Outline als Fat-Line auf WebGPU sichtbar

## 0.6.63
- fix(brep): Phase 3 voll — CoincidentSurface-Gate, On-Boundary-Aktivität, Interior-Fin-Beweis schließen Ticket o2j9

## 0.6.62
- fix(brep) #o2j9 #eil: Face-Extrude ankert exakt auf analytischer Ebene + robuste Koinzidenz-Kanonikalisierung

## 0.6.61
- fix(brep): Phasen 2+3 Koinzidenz-Kanonikalisierung — o2j9-Familie webt mannigfaltig

## 0.6.60
- feat(brep) #o2j9: Phase 1 Koinzidenz-Registry — orientierte Äquivalenzklassen, surface-generisch

## 0.6.59
- fix(brep): F0-2 Loop-Orientierung — Komplement-Parität + gesampelte Windung, Manifold-Konvention auf gekrümmten Flächen

## 0.6.55
- fix(brep) #px2o: Face-Extrude dockt an Eltern-Body statt fremder AABB-Überlappung

## 0.6.54
- fix(web) #dxrq: Sketch-Tools nach Plane-Pick funktionsfähig — One-Shot-Picker

## 0.6.53
- fix(web) #g5km: Sketch-Plane ohne Koordinatengitter

## 0.6.52
- fix(web) #zzl1: Extrude-Gizmo zeigt entlang Extrude-Achse statt Welt-äußerster Fläche

## 0.6.51
- fix(web) #mcvp: Kerben in fetten Highlight-Kanten durch kollineare Dezimierung

## 0.6.49
- feat(web) #bxoo: Sketch direkt in der 3D-Szene — normale Kamera, Projektion durchs Plane-Frame

## 0.6.48
- feat(web) #yx6a #bxoo: Sketch-Overlay nutzt echte Plane-Frames

## 0.6.46
- feat(web) #yx6a: Sketch startet sofort, Plane als editierbares Session-Property

## 0.6.45
- fix(web) #b9xz: Depth-Bias gegen Z-Fighting der Kanten im visible-only-Modus

## 0.6.44
- fix(web) #yiis: selektierte Sphere-/Torus-Face tintet das Primitiv orange

## 0.6.43
- feat(web) #yx6a #bxoo #hase: sketchPlaneFrame — web-seitige Plane-Frame-Auflösung + Ray-Plane-Input-Mathe

## 0.6.42
- fix(web) #6btt: selektierte Kanten 4px statt 2px

## 0.6.41
- feat(web) #yx6a #hase: Sketch-Plane im Detail-Panel editierbar

## 0.6.40
- fix(web) #yx6a #bxoo: editSketch round-trippt die Plane-Query + Sketch-in-3D-Plan

## 0.6.39
- fix(web) #fsmf: Sketch-2D-Overlay vertikal an die 3D-Kamera angeglichen

## 0.6.38
- fix(web) #c2ox: Line-Tool zeigt nur noch EIN Längenfeld — das blaue Live-Input

## 0.6.37
- fix(web,kernel) #p3vh: Transform-Feature-Gizmo committet in eigene Params; Längen-Einheit mm

## 0.6.36
- feat(web) #por8: Onshape-Selektion — runde Sphere-Tönung + vollflächiger Face-Fill

## 0.6.35
- fix(brep) #vyee: Query-Auflösung auf parametrischen Weave + robustes Face-Profil

## 0.6.34
- fix(brep) #oa9h: Face-Extrude-Profil per Endpunkt-Nähe zu Ringen ordnen

## 0.6.33
- fix(web) #bgvh: SVG-Import-Lexer für gepackte Pfaddaten

## 0.6.32
- fix(web) #qqgo: Extrude-Gizmo poliert — S/W-Pfeil, smooth Drag, echte Preview

## 0.6.31
- fix(web) #8f5z: B-Rep-Basiskanten als Fat-Lines rendern

## 0.6.30
- feat(web) #jn3t: Extrude-Tiefen-Gizmo mit Pfeil + transluzenter Slab-Vorschau

## 0.6.29
- fix(web) #981k: Extrude-Toolbar-Button wirkt auf selektierte Flächen

## 0.6.28
- feat(web) #7x4v: geschlossene Sketch-Konturen als Flächen schattiert + Wheel-Zoom bei aktivem Tool

## 0.6.27
- feat(web) #select #rectan #gle: Onshape-Multiselect + Rechteck-Selektion, Kugel-Highlight

## 0.6.26
- feat(web) #r36f: Viewport-Selektion in Seitenleiste spiegeln + Default-Geometrie selektierbar

## 0.6.25
- feat(web,server) #8onw: Attachments im Report-Issue-Dialog

## 0.6.24
- fix(web) #twvd: verdeckte Kanten nicht pickbar, Vertex-Punkte pixelgenau

## 0.6.23
- fix(web) #m0yw: Kanten-Hover heller im Selektions-Farbton

## 0.6.22
- feat(kernel,web): Onshape-Booleans — Add/New/Remove/Intersect statt Union/Difference/Intersect

## 0.6.21
- feat(web): Controls-Presets — Maus-/Tastatur-Schemata anderer CAD-Programme

## 0.6.20
- fix(brep) #9zlq: Path-Profile analytisch liften — gerade Extrude-Kanten

## 0.6.19
- feat(web) #evq2: Vertex-/Flächen-/Körper-Selektion im Viewport

## 0.6.18
- fix(brep) #g4rr: sub-band grazing seams weave exactly

## 0.6.17
- fix(web) #twxw: Kanten-Highlight folgt der Auswahl

## 0.6.16
- fix(web) #730r: Winkelfeld aus Line-Live-Input-HUD entfernen

## 0.6.15
- fix(web) #ijqe: ElementInfo-Readout selektierbar machen

## 0.6.14
- feat(brep) #el1x: parametric weave als Default an

## 0.6.13
- fix(web): edge hover highlight uses selection colour, 1px

## 0.6.12
- fix(web): selected edge as real 2px fat line, not invisible 1px

## 0.6.11
- fix(web): selected-edge highlight offset, now gold-orange 2px

## 0.6.10
- feat(web): close report dialog on submit, no confirmation step

## 0.6.9
- feat(web): capture full app window in issue tickets, not just viewport

## 0.6.8
- feat(web): in-app issue/feature ticket button

## 0.6.7
- fix(brep): split-on-spurious corner resolution for grazing primitives

## 0.6.6
- fix(brep): seed every active arc of a trimmed SSI component

## 0.6.5
- feat(brep): Phase 2b-iii — Seed-Pfad in weave_parametric verdrahtet

## 0.6.4
- feat(brep): Phase 2b-ii — build_seed_wireframe komplett

## 0.6.3
- feat(brep): Phase 2b-i — Seed-Weaver-Skelett + Scope-Gate

## 0.6.2
- feat(brep): Phase 2a — Aktiv-Intervall-Marsch im Tracer

## 0.6.1
- feat(brep): Phase 1b-ii — Branch-Pair-Seeds + SSI-Fixes

## 0.6.0
- feat(brep): Phase 1b-i — SSI-Komponentenzählung + Seeds

## 0.5.99
- feat(brep): Phase 1a — Branch-Paar-Aufzählung + AABB-Prune

## 0.5.98
- feat(brep): Phase 1a Fundament — statische CSG-Branch-Enumeration

## 0.5.97
- feat(brep): Differential-Komparator (Phase 0c Parametric-Weaving)

## 0.5.96
- feat(brep): Force-Pure-Octree-Modus (Phase 0b Parametric-Weaving)

## 0.5.95
- feat(brep): DL-24 Manifold-/Watertight-Invariante (Phase 0a Parametric-Weaving)

## 0.5.93
- feat(brep): DL-81 krümmungsadaptives Edge-Tracing in Produktion verdrahtet

## 0.5.92
- feat(brep): DL-81 krümmungsadaptives Edge-Tracing hinter Flag

## 0.5.91
- feat(brep): feature-adaptive octree — Face-Zellen gröber als Edge/Vertex

## 0.5.90
- fix(debug): Kanten-Overlay zeigt NURBS-Strukturpunkte statt Tessellierung

## 0.5.89
- feat(debug): Settings-Overlays für Kantenpunkte und Octree-Zellen

## 0.5.87
- fix(build): kernel-staleness guard compares vs on-disk WASM, not package.json

## 0.5.86
- fix(brep): DL-81 — drop phantom triple junction that kinks straight box edges

## 0.5.85
- fix(brep): DL-79 Kanten-Rescue + DL-80 geschlossene Loop-NURBS

## 0.5.79
- feat(web): live constraint solving during grip drag

## 0.5.78
- feat(sketch): point-on-spline solver constraint for open B-splines

## 0.5.77
- feat(sketch): make ellipse a first-class solver entity with tangent/point-on constraints

## 0.5.76
- fix(sketch): use authoritative solver dof() instead of JS estimate

## 0.5.72
- feat(sketch): editable sketch_path vertices + rounded SVG rects

## 0.5.71
- feat(sketch): G1 endpoint tangent legs for interpolation-mode splines

## 0.5.70
- feat(sketch): G1 tangent at B-spline endpoints via control-polygon legs

## 0.5.69
- feat: real cubic B-spline (control-point mode, de Boor, Boehm lowering)

## 0.5.68
- feat(web): filled glyph text from real fonts (opentype.js) replaces single-stroke

## 0.5.67
- feat: exact-Bézier SVG import + full transform stack + sketch_path primitive

## 0.5.66
- feat(web): chain/contour offset replaces single-curve offset

## 0.5.65
- feat(web): text as sketch geometry via a single-stroke font

## 0.5.64
- feat(web): interpolating spline tool

## 0.5.63
- feat(web): import SVG and DXF vector files as sketch geometry

## 0.5.62
- feat(web): use/project external geometry into the sketch

## 0.5.61
- feat(web): conflict/redundancy diagnostics in the constraints panel

## 0.5.60
- feat(web): selectable elements/constraints panel in the sketcher

## 0.5.59
- feat(web): live numeric input while drawing a line

## 0.5.58
- feat(web): move / copy / rotate selected sketch geometry

## 0.5.57
- feat(web): sketch linear & circular pattern

## 0.5.56
- feat(web): interactive sketch offset

## 0.5.55
- feat(web): interactive sketch mirror across a picked axis

## 0.5.54
- feat(web): sketch split — cut a curve in two at the click, keep both

## 0.5.53
- feat(web): sketch extend — lengthen a line to the nearest curve

## 0.5.52
- feat(web): sketch chamfer — bevel a corner between two lines

## 0.5.51
- feat(web): sketch fillet — round a corner between two lines

## 0.5.50
- feat(web): slot (stadium) tool

## 0.5.49
- feat(web): complete the ellipse tool UI

## 0.5.48
- feat(web): rectangle-center, rectangle-3point, circle-3point tools

## 0.5.47
- feat(web): regular polygon tool

## 0.5.46
- feat(web): live DoF counter in the sketch HUD

## 0.5.45
- feat(web): box-select / marquee in the sketcher

## 0.5.44
- feat(web): point-on-object constraint

## 0.5.43
- feat(web): fix/lock constraint pins a sketch point

## 0.5.42
- feat(web): concentric constraint in sketch palette

## 0.5.41
- feat(web): make rectangle & polygon edges trimmable

## 0.5.40
- feat(web): real interactive Trim for the 2D sketcher

## 0.5.39
- feat: construction (reference) geometry for the 2D sketcher

## 0.5.38
- feat: radius + diameter dimensions for circles/arcs

## 0.5.37
- feat(web): midpoint + symmetric constraints in the sketch palette

## 0.5.36
- feat(web): wire mirror + offset profile-operation tools

## 0.5.35
- feat(web): interactive arc drawing in the 2D sketch editor

## 0.5.33
- feat(web): two-finger pinch-to-zoom for the 2D sketch canvas

## 0.5.32
- feat(web): mobile touch hit-targets for toolbar + bottom tabs

## 0.5.31
- feat(web): mobile responsive Outliner drawer

## 0.5.30
- feat(web): grips for rectangle/polygon/arc geometry

## 0.5.29
- feat(web): P0.3 drag-edit committed sketch geometry via grips

## 0.5.27
- feat(web): editable dimensions — Length/Distance + Angle

## 0.5.26
- feat(shell): selectable inner-wall + opening B-Rep faces
- feat(web): reach + verify the sketch constraint palette

## 0.5.25
- feat(web): explicit constraint palette in the sketch editor

## 0.5.24
- feat(web): click-select committed sketch geometry in select mode

## 0.5.23
- feat(web): add sketch selection model for committed geometry

## 0.5.22
- feat(web): wire all sketch constraint types into the live solver

## 0.4.9
- feat(web): add Render loop demand/always toggle in Settings

## 0.4.8
- perf(web): drive WebGPU viewport on demand instead of always

## 0.4.6
- fix(web): unstick WebGPU swapchain present with a real pixel-size kick

## 0.4.5
- fix(web): stop shader-compile freeze + reliable present after compile

## 0.4.1
- feat: SDF shade pass writes canvas depth for B-Rep edge occlusion
