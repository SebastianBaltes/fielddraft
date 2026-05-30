* Zu Technical Material soll es auch noch einen Namen als Eingabefeld geben. Materials sollen also in einer Oberfläche CRUD sein - statt fester Presets gibt es also vorgefertigte Materialien, die sich erweitern lassen
* Compile von Shadern lässt Browser einfriefen (auch in andern Tabs). Ich dachte, das sollte Async sein? Kann man das einfrieren verhindern?
*
* [https://github.com/timschmidt/csgrs](https://github.com/timschmidt/csgrs)
* [https://github.com/cheind/sdftoolbox](https://github.com/cheind/sdftoolbox)
* [https://github.com/kaist-visual-ai-group/odc](https://github.com/kaist-visual-ai-group/odc)
* [https://github.com/sgsellan/gpytoolbox](https://github.com/sgsellan/gpytoolbox)
* [https://crates.io/crates/delaunay](https://crates.io/crates/delaunay)





* Gyroid ist immer noch eine AABB. Das hattest du bereits mehrmals versucht zu fixen - jetzt müssen wird dem auf den Grund gehen!

FALSCH! KEINE ÜBERLAPPUNG NACH ==AABB== ODER ORT. Das beisst sich doch fundamental mit der Anforderung "Wirkt nur auf bestimmte Körper.

27.05. 23:00

Du hast Recht. Die ==AABB==-basierte Zuordnung ist fundamental falsch für Gyroid. Es darf **keine räumliche Überlappung** als Kriterium geben — die Zuordnung muss **rein über die Target-IDs** laufen:

* Gyroid hat Targets → wird **ausschließlich** in die Bodies dieser Targets eingefügt (als Intersect-Operation im CSG-Tree)
* Kein ==AABB==-Check, kein Overlap-Test, keine räumliche Logik
* Die ==AABB== des Gyroids selbst ist irrelevant, weil er mathematisch unendlich ist und nur durch die Intersection mit dem Target-Body begrenzt wird

Das heißt konkret:

1. **`csg.rs` ==AABB==**: Kann weg oder auf `[0;3]` — wird nie gebraucht
2. **`body.rs` Partitionierung**: Für Features mit `target_ids` → direkt die Target-Bodies nachschlagen, kein ==`aabb`==`s_overlap`-Fallback
3. **CSG-Tree**: Gyroid wird als Intersect-Node an den CSG-Tree des jeweiligen Target-Bodies angehängt

Soll ich das so umsetzen?

27.05. 23:00

Ja

27.05. 23:00

Ich lese die relevanten Stellen im Kernel.

Jetzt verstehe ich die Partitionierungslogik vollständig. Lass mich die `Feature`-Struct und `target_ids` anschauen.

Gut, jetzt sehe ich das vollständige Bild. Die `target_ids`-Logik in `body.rs:189-193` ist eigentlich korrekt — das Problem ist, dass bei `target_ids` nicht-leer der ==AABB== trotzdem die Body-==AABB== aufbläht. Lass mich den Fix machen.

Also `SpineFeature` \= `Feature` (alias). Die `target_ids` kommen also korrekt in der `Feature`-Struct an. Der Partitioner in `body.rs` bekommt sie.

Lass mich prüfen, ob der Bug vielleicht darin liegt, dass die `targets`-Query leer ist (User hat keine Targets ausgewählt), oder dass `qCreatedBy(...)` nicht matcht.

Jetzt ist mir das Problem klar. Zwei Fixes nötig:

1. **`body.rs`**: Gyroid muss in **alle** matching Target-Groups eingefügt werden (nicht nur die erste via `.find()`)
2. **`csg.rs`**: Gyroid-==AABB== auf Zero setzen als Safety-Net

Jetzt prüfe ich ob ==`Aabb`==`::ZERO` existiert oder ob ich es definieren muss.

Es gibt `EMPTY` aber kein `ZERO`. Ich verwende `EMPTY` — das passt besser weil eine Empty-==AABB== mit nichts überlappt.

Jetzt bauen und Tests laufen lassen.

Alle Tests grün. Jetzt WASM bauen und committen.

Fix gepusht als v0.3.75. Die Änderungen:

**`body.rs` — Partitionierung:**

* Features mit `target_ids` werden jetzt in **alle** passenden Body-Groups eingefügt (Loop über alle Groups statt `.find()` für nur die erste)
* Kein ==AABB==-Fallback wenn `target_ids` gesetzt ist

**`csg.rs` — ==AABB==:**

* Gyroid-==AABB== ist jetzt `EMPTY` (Infinity/-Infinity) statt der 2000er-Box — kann nie versehentlich per Overlap matchen

Damit wirkt Gyroid nur noch auf Bodies, deren Feature-IDs in den Targets stehen.

27.05. 23:00

Zählst du die Versionsnummer hoch? Weil ich immer noch v0.3.70 sehe. Auch nach Reload.