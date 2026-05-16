# FieldDraft UX‑Spezifikation (Onshape‑Abgleich)

> **⚠ Teilweise veraltet — Dialog‑Strukturen siehe `docs/TOOL-DIALOGS.md`.**
>
> Dieses Dokument wurde im April 2026 als "fehlerhaft, lückenhaft, älter" identifiziert. Die **authoritative Dialog‑Struktur** pro Tool (Header, Tabs, Body, Parameter, Modes, Reveals, Footer) steht jetzt in **`docs/TOOL-DIALOGS.md`**, rekonstruiert aus den YAML‑Specs in `assets/onshape/yaml/*.yaml`. Wenn du einen Feature‑Dialog implementierst, lies TOOL‑DIALOGS.md **zuerst** — diese Datei hier ist nur noch als UX‑Philosophie‑Ergänzung nützlich (Sketch‑Paritäts‑Szenarien, Modifier‑Tasten, Drag‑Stabilität), nicht als Dialog‑Referenz. Die Feature‑Parameter‑Tabellen unten sind an mehreren Stellen unvollständig oder widersprechen den YAMLs.

Dieses Dokument beschreibt pro Tool, welche **Parameter**, **Freiheitsgrade** und **UX‑Verhalten** wir aus Onshape übernehmen, vereinfachen oder streichen — fokussiert auf den MVP‑Scope aus `README.md` §6. Es ersetzt nicht die Feature‑Tabellen in der README, sondern konkretisiert sie.

## Verhältnis zur README

Diese Spezifikation liegt logisch **zwischen** den README‑Feature‑Tabellen (§6.1/§6.2) und der eigentlichen Implementierung. Alle Architektur‑Entscheidungen (Kernel‑Schichten, Solver‑Wahl, Picking‑Pfad, Fillet‑Mechanismus) stehen in der README; hier geht es nur darum, wie der User die Tools **sieht, bedient und erwartet**. Zwei README‑Konzepte sind für die UX zentral und werden in diesem Dokument mehrfach referenziert:

- **B‑Rep‑Proxy** (README §4.1) — der aus Provenance abgeleitete topologische Layer mit Faces, Edges und Vertices. Er ist das **einzige** Ziel jedes Snappings und jeder Face/Edge‑Selektion im Part‑Editor. Wenn ein Tool im folgenden Text „Face anklicken" sagt, ist implizit immer eine Face des B‑Rep‑Proxys gemeint, nicht eine Dreiecks‑Fläche des gerenderten Meshes.
- **Ghost Topology** (README §4.1, §4.2) — der Dual‑Path‑Evaluator. Picking und Snapping laufen auf dem *logical path*, der `smin`‑Blends und Fillet‑/Chamfer‑Masken überspringt. Der User snappt damit auf die virtuelle scharfe Original‑Kante einer verrundeten Fläche, ohne dass ein zweites Datenmodell gepflegt werden müsste. Dieser Mechanismus ersetzt die „Virtual Sharps" klassischer B‑Rep‑Systeme.

Weiter relevant: Der 2D‑Constraint‑Solver ist **PlaneGCS** (README §4.4), nicht SolveSpace. Alle Sketch‑UX‑Erwartungen (Drag‑Stabilität, DoF‑Preflight, Auto‑Inferencing) werden von der Rust‑Adapter‑Schicht um PlaneGCS geliefert, nicht vom Solver‑Kern selbst. Das Frontend ist **React + React Three Fiber auf WebGPU** — keine direkten Three.js‑Szenen, keine imperative Viewport‑Logik.

## Legende

- **✅ übernehmen** — so wie in Onshape.
- **🔸 vereinfachen** — im Kern gleich, aber reduzierte Optionen.
- **➖ streichen** — in MVP nicht nötig.
- **➕ ergänzen** — über Onshape hinaus oder in README fehlt.
- **(?)** — hier bin ich unsicher, Screenshot oder Doku‑Check nötig.
- **FD** — wie FieldDraft es macht/machen sollte.

## Lesereihenfolge

1. Part‑Editor (§1): die 3D‑Tools, Reihenfolge nach Häufigkeit der Nutzung.
2. Sketch‑Editor (§2): 2D‑Zeichenwerkzeuge und Constraints.
3. Variablen & Formel‑Engine (§3).
4. Tool‑übergreifende UX‑Konventionen (§4): Selektion, Feedback, Shortcuts, Fehlerverhalten.
5. Abgleich mit README §6 (§5): Liste konkreter Änderungs­vorschläge.

---

## 1. Part‑Editor

### 1.1 Sketch (als Einstieg)

**Onshape‑Verhalten:**
- User klickt Sketch‑Tool, danach eine **Planebene oder planare Fläche** im Viewport.
- Ein grünes Häkchen oben schließt den Sketch ab, ein rotes X verwirft.
- Shortcut für Aktivierung: **SHIFT+S**

**Freiheitsgrade:**
- Ebenen‑Auswahl: Origin‑Planes (Front/Top/Right), erzeugte Planes, planare Bauteil‑Flächen, Mate Connectors (als orientierter Nullpunkt).
- Richtung der Sketch‑Y‑Achse: implizit aus der ausgewählten Ebene; in manchen Fällen manuell umschaltbar (?).

---

### 1.2 Extrude

**Dialog‑Struktur (aus `assets/3d-extrude.png`):**

Der Dialog ist zweifach getabbt: erst **Solid / Surface / Thin**, dann **New / Add / Remove / Intersect**. In FieldDraft fallen **Surface** und **Thin** raus (das sind B‑Rep‑Konzepte aus klassischen Kerneln, die im SDF‑Modell keine Entsprechung haben — siehe README §12 „harte Architektur‑Grenzen"). Der Solid‑Tab bleibt als einziger MVP‑Modus.

**Operation‑Type (NAIO‑Tabs):**
- **New** — erzeugt einen neuen Body.
- **Add** — schmilzt mit einem existierenden Body zusammen (Union, mit Merge‑Scope).
- **Remove** — zieht vom ausgewählten Target‑Body ab (Difference).
- **Intersect** — behält nur den Überlapp mit dem Target‑Body.

**Profil‑Auswahl:**
- Feld „Faces and sketch regions to extrude" akzeptiert **Sketch‑Regionen** (vom Regions­finder des Sketchers erzeugte geschlossene Flächen) und **planare Proxy‑Faces** bestehender Bodies.

**End Condition (Haupt‑Richtung) — Dropdown:**
- **Blind** (fester Depth‑Wert).
- **Symmetric** (gleich viel in beide Richtungen — ersetzt „Second end position").
- **Through all** (bis alles durchdrungen ist).
- **Up to face** (bis zu einer ausgewählten Proxy‑Face).
- **Up to entity** (bis zu einem ausgewählten Vertex oder Edge — ebenfalls Proxy‑Element).
- **Up to next** ist in Onshape **integriert im Dropdown** und bedeutet „bis zur nächsten Face in Richtung".

**Direction (optional, Checkbox):**
- Explizite Richtungs­vorgabe über eine Edge, Achse oder Mate‑Connector‑Z‑Achse. Ohne Checkbox gilt die Sketch‑Normale.

**Starting offset (optional, Checkbox):**
- Die Extrusion startet nicht auf der Sketch‑Ebene, sondern um einen Offset versetzt. Eigenes End‑Condition‑Dropdown und eigenes Depth‑Feld — funktional identisch zur Haupt‑End‑Condition, nur als Startpunkt.

**Symmetric (Checkbox):**
- Shortcut für symmetrische Extrusion um die Sketch‑Ebene.

**Draft (Checkbox):**
- Draft ist **direkt im Extrude‑Dialog** integriert (nicht separates Feature). Winkel + Innen/Außen‑Toggle für konische Seitenwände (Entformungsschrägen).

**Second end position (Checkbox):**
- Eine komplett eigene Gegenrichtung mit eigenem End‑Condition‑Dropdown, Depth‑Feld und eigener Draft‑Checkbox. Ermöglicht asymmetrische Extrusion in zwei Richtungen mit unterschiedlichen Werten und unterschiedlichen Draft‑Winkeln.

**Merge scope (implizit):**
- Bei Add/Remove bestimmt Onshape über Merge‑Scope‑Felder, welche Target‑Bodies betroffen sind. Im Screenshot nicht sichtbar, weil „New" aktiv.

**Live‑Preview:** Ja, Dialog rechts‑angedockt, Viewport‑Update in Echtzeit.

**Freiheitsgrade (Zusammenfassung):**
- Tab (Solid) × Operation‑Type (4) × Profil (1..n Regionen) × Direction (0/1) × End Condition (6) × Starting offset (0/1) × Symmetric (0/1) × Draft (0/1) × Second end position (0/1, mit eigenem End Condition + Depth + Draft).

**FD‑Empfehlung für MVP:**
- ✅ Nur **Solid**‑Tab, Surface/Thin entfallen dauerhaft (README §12).
- ✅ Alle vier NAIO‑Modi (New/Add/Remove/Intersect).
- ✅ Profil‑Multiselect (Sketch‑Regionen + Proxy‑Faces).
- ✅ End Condition: **Blind, Symmetric, Through all, Up to face, Up to entity**. „Up to face/entity" referenziert Ziel‑Elemente über den B‑Rep‑Proxy.
- 🔸 „Up to next" in MVP nicht notwendig — der User kann manuell die konkrete Ziel‑Face picken.
- ✅ Direction‑Checkbox mit expliziter Achsen­vorgabe.
- ✅ Starting offset (wird in SDF trivial als Box‑Subtraktion an der Startebene realisiert).
- ✅ Symmetric‑Checkbox.
- 🔸 **Draft im MVP behalten** — die README hat Draft bisher nicht explizit genannt, aber der Screenshot zeigt, dass es in Onshape im Haupt‑Dialog sitzt, nicht als Nebenfeature. Parametrische Draft‑Winkel lassen sich im SDF‑Kernel als lineare Profil‑Skalierung entlang der Extrusionsachse umsetzen; die UX‑Komplexität ist minimal, weil die Checkbox im bestehenden Dialog landet. Vorschlag: als Tracking‑Punkt in §5 für einen README‑K‑Slice ergänzen.
- ✅ Second end position (asymmetrischer Extrude mit zweiter End‑Condition/Depth/Draft).
- ✅ Merge scope bei Add/Remove.
- ➕ **Alle numerischen Felder akzeptieren Formeln** (`=width/2 + 5 mm`) — siehe §3.
- ➕ Einheit pro Feld respektiert die **Workspace Units** (§1.28).

**Shortcut:** per JSON‑Keybinding konfigurierbar (siehe §4.3).

---

### 1.3 Revolve

**Dialog‑Struktur (aus `assets/3d-revolve.png`):**

- **Solid / Surface / Thin**‑Tabs (wie bei Extrude — FD nutzt nur Solid).
- **New / Add / Remove / Intersect**‑Tabs.
- **Faces and sketch regions to revolve** — Profil‑Auswahl.
- **Revolve axis** — akzeptiert eine Sketch‑Construction‑Line, eine Proxy‑Edge, eine Referenz‑Achse oder **einen Mate Connector** (die Z‑Achse des Connectors wird zur Rotationsachse). Der Screenshot zeigt explizit einen Mate‑Connector als Achse.
- **Full revolve** Checkbox — default an, entspricht 360°. Abgeschaltet erscheint ein Angle‑Feld mit Blind/Symmetric‑Modi.

**Freiheitsgrade:** Profil × Operation (NAIO) × Achse × (Full vs. Winkel) × Richtung.

**FD‑Empfehlung:**
- ✅ Alle vier NAIO‑Modi.
- ✅ Achsen‑Quellen: Sketch‑Construction‑Line, Proxy‑Edge, Referenz‑Achse, **Mate Connector Z‑Achse**.
- ✅ Full‑Revolve‑Default, optional expliziter Winkel (Blind/Symmetric).
- ✅ Numerische Felder mit Formel‑Support und Workspace‑Unit‑Respekt.
- ➕ Fehlermeldung, wenn Profil die Rotationsachse schneidet (Self‑Intersection‑Warnung).

---

### 1.4 Boolean

**Dialog‑Struktur (aus `assets/3d-boolean.png`):**

- **Union / Subtract / Intersect**‑Tabs (kein NAIO‑Subtab hier — Boolean kombiniert per Definition existierende Bodies).
- **Tools** — die Tool‑Bodies (werden subtrahiert oder verschnitten).
- **Targets** — die Ziel‑Bodies (bekommen das Tool aufgedrückt).
- **Offset** (Checkbox) — die Tool‑Bodies werden vor dem Boolean um einen Offset erweitert oder geschrumpft. Typischer Anwendungsfall: eine Passung mit 0,1 mm Spiel direkt im Boolean erzeugen, ohne vorher ein separates Offset‑Feature zu bauen.
- **Offset all** (Checkbox) — Offset wirkt auf alle Faces des Tools. Ohne diese Option muss der User einzelne Faces über „Faces to offset" auswählen.
- **Offset distance** — Zahl mit Einheit.
- **Reapply fillet** (Checkbox) — wenn der Target‑Body Fillets hat, die durch die Boolean‑Operation verschwinden würden, werden sie danach automatisch erneut angewandt. In Onshape ist das eine Heuristik über die Feature‑Historie.
- **Keep tools** (Checkbox) — Tools bleiben nach der Operation als eigene Bodies erhalten.

**FD‑Empfehlung:**
- ✅ Union, Subtract, Intersect als explizite Feature‑Tree‑Knoten (parallel zu den inline‑Operations im Extrude/Revolve‑Dialog).
- ✅ **Keep tools**‑Option.
- ✅ **Offset** mit Offset‑Distance. Im SDF ist das mathematisch trivial — `f_tool_offset(p) = f_tool(p) − d` —, die UX‑Komplexität liegt nur im Dialog‑Layout.
- 🔸 **Offset all** vs. „Faces to offset" im MVP: nur „Offset all" umsetzen. Face‑spezifische Offsets setzen das Feature‑ID‑Tagging voraus, das bereits als offenes Thema in der README steht.
- ➖ **Reapply fillet** explizit **nicht** im MVP. In der SDF‑Architektur mit Ghost Topology wandert der Fillet‑Spatial‑Mask ohnehin mit den beteiligten Contributors mit — das Problem, dass Fillets nach einem nachgelagerten Boolean verschwinden, existiert in FieldDraft strukturell gar nicht. Damit entfällt das Feature ersatzlos.
- ➕ In SDF fallen klassische Boolean‑Fehlerfälle (Co‑Planarität, Mikro‑Lücken) strukturell weg — die UI kann den User getrost vor sich hin klicken lassen.

---

### 1.5 Linear Pattern

**Dialog‑Struktur (aus `assets/3d-linear-pattern.png`):**

- Oben ein Dropdown mit der **Pattern‑Quelle**: **Part pattern**, **Feature pattern**, **Face pattern**. Der Selektor bestimmt, was gemustert wird; je nach Wahl ändert sich das Entities‑Feld.
- **New / Add / Remove / Intersect**‑Tabs — wie beim Extrude, weil Pattern im Part‑Studio als Boolean‑Operation in die Szene hängt.
- **Entities to pattern** — Sammlung (Features, Parts oder Faces je nach Pattern‑Quelle).
- **Direction** — eine Kante, Achse oder Sketch‑Linie.
- **Distance** — Zahl mit Einheit.
- **Instance count** — ganze Zahl ≥ 2.
- **Centered** (Checkbox) — verteilt die Instanzen symmetrisch um die Original‑Position statt nur in Richtung des Direction‑Vektors.
- **Second direction** (collapsible) — eigene Achse, Distance, Count, Centered → 2D‑Raster.
- **Skip instances** (collapsible) — einzelne Instanzen im Preview anklicken und deaktivieren.

**Spacing‑Modi:** Der User hat bestätigt, dass Onshape zwei Modi für das Distance‑Feld kennt — **equal spacing** (Abstand pro Instanz) und **up to distance** (Gesamtlänge). Im MVP reicht der erste Modus.

**Freiheitsgrade:** Pattern‑Quelle × Operation (NAIO) × Entities × Direction × Distance × Count × Centered × (Second Direction optional) × (Skip‑List optional).

**FD‑Empfehlung:**
- ✅ Pattern‑Quelle im MVP: **Part pattern** und **Feature pattern**. Face pattern erst, wenn der B‑Rep‑Proxy selektive Face‑Operationen stabil liefert (README §11 Feature‑ID‑Tagging).
- ✅ NAIO‑Tabs wie beim Extrude.
- ✅ Direction + Distance + Count.
- ✅ **Centered**‑Option.
- ✅ **Second direction** (2D‑Raster).
- ✅ **Skip instances** (überraschend kleiner Mehraufwand — die Phantom‑Instanzen sind ohnehin im Live‑Preview angelegt, Skip ist nur ein Bitmask darüber). Im MVP halten.
- ➕ Distance‑Eingabe akzeptiert Formeln (`=pitch * 2`).
- ➕ „Up to distance"‑Modus kann Phase 2 bleiben, bis echte User‑Nachfrage existiert.

---

### 1.6 Circular Pattern

**Dialog‑Struktur (aus `assets/3d-circular-pattern.png`):**

- Pattern‑Quelle‑Dropdown **Part pattern / Feature pattern / Face pattern** (analog Linear Pattern).
- **New / Add / Remove / Intersect**‑Tabs.
- **Entities to pattern**.
- **Axis of pattern** — Edge, zylindrische Face, Referenz‑Achse oder Mate Connector.
- **Angle** — default 360°, Formel erlaubt.
- **Instance count** — default 4 im Screenshot.
- **Equal spacing** (Checkbox) — default aktiv. Verteilt Instanzen gleichmäßig über den Winkel. Ohne die Checkbox wird der Angle‑Wert als Winkel **pro Instanz** interpretiert statt als Gesamtwinkel.
- **Centered** (Checkbox) — verteilt den Gesamtwinkel symmetrisch um die Original‑Position.
- **Skip instances** (collapsible).

**Winkel‑Range:** Der User hat bestätigt, dass Onshape immer als **„from / to"** arbeitet — kein separater „Full"‑Modus. Angle = 360° und Equal spacing = an ist der Default für den vollen Kreis.

**FD‑Empfehlung:**
- ✅ Pattern‑Quelle (Part/Feature im MVP, Face später).
- ✅ NAIO‑Tabs.
- ✅ Axis aus Edge/Face/Mate Connector.
- ✅ Angle + Instance count + Equal spacing + Centered.
- ✅ **Skip instances** im MVP (Konsistenz mit Linear Pattern).

---

### 1.7 Mirror

**Dialog‑Struktur (aus `assets/3d-mirror.png`):**

- Quelle‑Dropdown **Part mirror / Feature mirror / Face mirror**.
- **New / Add / Remove / Intersect**‑Tabs.
- **Entities to mirror**.
- **Mirror plane** — planare Proxy‑Face oder Referenz‑Ebene.

**FD‑Empfehlung:**
- ✅ Quelle‑Dropdown (Part/Feature im MVP).
- ✅ NAIO‑Tabs.
- ✅ Plane‑Auswahl ausschließlich gegen Proxy‑Faces / Referenz‑Planes.
- ✅ In SDF ist Spiegelung eine reine Raumtransformation, extrem billig.

**Editierbarkeit gespiegelter Instanzen — wichtige Korrektur:** Entgegen früherer Annahmen sind in Onshape **gespiegelte Parts separat editierbar**. Die Regel folgt der **Position im Feature‑Tree**:

- Alle Features, die **vor** dem Mirror‑Knoten im Feature‑Tree stehen, wirken nur auf die Original‑Instanz und werden durch den Mirror auf das Spiegel­bild kopiert.
- Alle Features, die **nach** dem Mirror‑Knoten eingefügt werden, wirken **unabhängig** — sie können gezielt nur auf die Original‑Instanz, nur auf das Spiegel­bild, oder auf beide gleichzeitig angewandt werden, je nachdem, welches Body der User als Target selektiert.

Das ist architektonisch wichtig: Mirror ist kein reiner „kopiere jeden nachfolgenden Schritt automatisch"‑Operator, sondern ein Verzweigungs­punkt im Feature‑Tree, ab dem zwei unabhängige Bodies existieren, die die vorherige Historie teilen. Der User kann damit asymmetrische Details an einem sonst symmetrischen Bauteil anbringen (z. B. links eine Beschriftung, rechts eine Klemme), ohne den Mirror aufzulösen.

**FD‑Umsetzung in der SDF‑Architektur:** Der Mirror‑Knoten erzeugt einen neuen Body mit eigener `body_id` (README §4.1, Multi‑Body‑Isolation). Der Genealogy‑Graph hält fest, dass der neue Body eine Spiegel‑Kopie des Originals ist und bis zum Mirror‑Zeitpunkt identische Feature‑Historie teilt. Nachfolgende Features, die auf einen bestimmten Body gerichtet sind, treffen entweder nur den originalen, nur den gespiegelten oder (per Multi‑Select im Feature‑Dialog) beide Bodies. Das Picking läuft ohnehin body‑scoped; die UX bleibt konsistent mit allen anderen Multi‑Body‑Szenarien.

---

### 1.8 Transform

**Dialog‑Struktur (aus `assets/3d-transform.png`):**

- **Entities to transform or copy** — eine oder mehrere Bodies/Features.
- **Modus‑Dropdown** mit sieben Einträgen:
  1. **Translate by line** — Richtung aus einer Kante, Distanz als Zahl.
  2. **Translate by distance** — Abstand entlang einer vom User definierten Richtung (unabhängig von Geometrie).
  3. **Translate by XYZ** — ΔX / ΔY / ΔZ in Weltkoordinaten.
  4. **Transform by mate connectors** — zwei Mate Connectors aufeinander mappen (Rigid Alignment).
  5. **Rotate** — Achse + Winkel.
  6. **Copy in place** — dupliziert ohne zu verschieben.
  7. **Scale** — **existiert tatsächlich als eigener Modus** in Onshape.

Jeder Modus zeigt anschließend seine spezifischen Felder.

**FD‑Empfehlung:**
- ✅ **Alle sieben Modi übernehmen.** Die frühere Empfehlung, Scale zu streichen, war falsch — der Screenshot bestätigt, dass Scale ein regulärer Modus ist, und die README §6.1 listet Transform ohnehin explizit als „Move/Rotate/Scale" auf.
- ✅ Im SDF ist Scale trivial als uniformer oder achsenweiser Space‑Warp umzusetzen (`f_scaled(p) = f_orig(p / s) · min(s)` — mit Lipschitz‑Korrektur, analog zu Bend/Twist in README §4.1 und K11).
- ➕ **Warnung bei nicht‑uniformem Scale:** Anisotropes Scaling verzerrt Fillets und kann die Lipschitz‑Bedingung verletzen, wenn sie nicht sauber korrigiert wird. UI zeigt einen Hinweis, wenn der User unterschiedliche X/Y/Z‑Faktoren eingibt.
- ➕ **Copy‑Flag pro Modus:** außer in „Copy in place" und „Transform by mate connectors" sollte jeder Modus eine „Copy" Checkbox anbieten.

---

### 1.9 Delete (Parts / Features)

**Onshape‑Verhalten:**
- Delete‑Key auf einem Feature‑Tree‑Eintrag entfernt ihn inkl. aller abhängigen Features (mit Warnung).
- Separater „Delete Part"‑Feature existiert (?), um gezielt einen Körper aus dem Studio zu entfernen, während andere bleiben.

**FD‑Empfehlung:**
- ✅ Delete im Feature‑Tree ist Standard‑Operation.
- ➕ Broken‑Reference‑Handling (siehe §11 Risiken der README) muss hier sichtbar werden: wenn Delete referenzierte Sketches zerstört, werden Folge‑Features im Tree rot markiert, nicht stumm entfernt.

---

### 1.10 Plane

**Onshape‑Verhalten — Creation Modes:**
- **Offset** — Abstand von bestehender Ebene.
- **Angle** — Rotation um eine Kante, ausgehend von einer Referenz­fläche.
- **Three points** — drei Punkte im Raum.
- **Point + normal** — Punkt + Achse.
- **Mid plane** — Halbierende zwischen zwei Flächen.
- **Tangent** — tangential zu zylindrischer Fläche an einem Punkt.
- **Curve point** — senkrecht zu einer Kurve an einem Punkt.
- **Line + angle** — eine Linie als Achse, Winkel zu Referenz.

**Freiheitsgrade:** Mode × modus‑spezifische Parameter.

**FD‑Empfehlung für MVP:**
- ✅ **Offset, Three points, Angle, Mid plane** — das deckt >95% der Praxis.
- 🔸 Tangent/Curve‑point → Phase 2.
- ➕ Alle numerischen Eingaben akzeptieren Formeln.

---

### 1.11 Variable

**Dialog‑Struktur (aus `assets/3d-variable.png`):**

- Titel im Feature‑Tree: `#Box_Length = 20 cm` (Name‑Preview mit ausgewertetem Wert).
- **Source‑Tabs:**
  - **Assigned** — klassische Variable, Wert oder Expression.
  - **Measured** — der Wert wird aus einer Messung am Modell entnommen und bleibt daran gebunden (typisch: „Länge der Kante X", aktualisiert sich, wenn das Modell sich ändert).
  - **From import** — der Wert stammt aus einer externen Quelle (CSV/Spreadsheet).
- **Type‑Tabs** (im Assigned‑Modus): **Length / Angle / Number / Any** — erlaubt es, die erwartete Dimension explizit zu markieren, sodass der Expression‑Parser Einheitenfehler an der Quelle fängt.
- **Name** — alphanumerisch mit Underscore.
- **Value** — Expression oder Zahl mit Einheit.
- **Description** — freier Text, optional.

**Integration im Feature‑Tree:** Anders als in früheren Entwürfen dieses Dokuments vermutet, ist das Variable‑Management **kein separates Panel**, sondern Teil des regulären Feature‑Trees. Variablen sind Feature‑Nodes wie alle anderen, erscheinen chronologisch im Tree und werden über den normalen „New Feature"‑Weg erstellt. Referenzen aus anderen Feldern gehen über `#name`.

**FD‑Empfehlung:**
- ✅ Variable als Feature‑Tree‑Knoten (kein separates Panel).
- ✅ **Assigned** im MVP, mit allen vier Type‑Tabs (Length/Angle/Number/Any).
- 🔸 **Measured** — konzeptionell schön, aber braucht einen stabilen Draht zwischen Measure‑Tool und Variable‑Feature. Im MVP als Tracking‑Eintrag, nicht als Hard‑Requirement.
- 🔸 **From import** — Phase 2, kein MVP‑Bedarf.
- ➕ Inline‑Preview rechts vom Namen im Feature‑Tree (`#Box_Length = 20 cm`), identisch zu Onshape.

Die Expression‑Sprache, das Einheiten‑System und die Fehler‑Diagnostik stehen im dedizierten §3 dieses Dokuments und in README §4.5.

---

### 1.12 Mate Connector

**Dialog‑Struktur (aus `assets/3d-mate-connector.png`):**

- Obere Dropdown‑Auswahl: **On entity** (häufigster Modus — der Connector wird an einer Proxy‑Face/Edge/Vertex verankert). Weitere Modi existieren (z. B. „On mate connector", „On plane"), sind im MVP aber nachrangig.
- **Origin entity** — das Zielelement im Viewport (Face/Edge/Vertex aus dem B‑Rep‑Proxy).
- **Realign** (Checkbox) — erlaubt, die X/Y/Z‑Achsen des Connectors explizit umzuorientieren (Flip‑ und Rotations‑Buttons erscheinen, wenn aktiv).
- **Move** (Checkbox) — verschiebt den Connector relativ zum automatisch berechneten Ursprung (z. B. von der Face‑Mitte zur Ecke).
- **Owner entity** (Checkbox) — bindet den Connector explizit an einen bestimmten Body, falls das automatische Owner‑Resolving mehrdeutig ist.
- Ganz unten zwei kleine Icons (mit hoher Wahrscheinlichkeit „Flip X" und „Rotate 90°").

**Benannte vs. implizite Connectors:** Onshape zeigt **nur explizit erzeugte** Mate Connectors im Feature‑Tree. Implizite Connectors, die spontan als Teil eines anderen Features entstehen (z. B. ein Sketch‑Origin, das als Mate‑Reference benutzt wird), bleiben anonym und erscheinen nicht im Tree.

**Rolle im Part‑Studio:** FieldDraft hat kein Assembly, nutzt Mate Connectors aber als **stabile lokale Koordinatensysteme** für Plane‑Erzeugung (z. B. Plane aus Mate‑Connector‑XY), Pattern‑Achsen, Revolve‑Achsen und Sketch‑Einstiege. Die Onshape‑Semantik „zwei Mate Connectors aufeinander mappen" wird im Transform‑Feature (§1.8) durch den Modus *Transform by mate connectors* abgedeckt.

**FD‑Empfehlung:**
- ✅ Mate Connector als expliziter Feature‑Tree‑Eintrag, mit den vier oben genannten Controls (Realign, Move, Owner entity, Flip/Rotate‑Icons).
- ✅ Nur benannte Connectors im Tree; implizit entstandene bleiben anonym.
- ✅ Als Achse für Revolve und Circular Pattern freigegeben.
- ✅ Als Plane‑Quelle: „Plane aus Mate Connector XY/YZ/XZ".
- 🔸 „On mate connector"‑Modus (Connector auf Connector legen) erst, wenn der MVP steht.


---

### 1.13 Measure (Maßband)

**Korrektur gegenüber früheren Fassungen dieses Dokuments:** Der frühere Eintrag behauptete, Onshape habe „keinen separaten modalen Dialog". Das war falsch — `assets/3d-measure.png` und `assets/3d-measure-2.png` zeigen ein dediziertes **Measure‑Panel** mit eigener Titelzeile, Selection‑Feld und Ergebnis­anzeige.

**Dialog‑Struktur (aus `assets/3d-measure.png` / `3d-measure-2.png`):**

- **Panel‑Titel:** „Measure", mit Schließen‑X.
- **Select entities to measure** — Multi‑Select‑Feld, akzeptiert Proxy‑Edges, ‑Faces und ‑Vertices gemischt.
- **Measure type** — Dropdown (im Screenshot „Show all"). Weitere Optionen filtern die Ausgabe (nur Länge, nur Winkel, nur Fläche …).
- **Length unit** — Dropdown (Centimeter, Millimeter, Meter, Inch, …). Default folgt den **Workspace Units** (§1.28).
- **Angle unit** — Dropdown (Degree, Radian).
- **Reference coordinate system** (Checkbox) — bei aktiv werden Koordinaten bezogen auf ein auswählbares CS statt auf das Weltsystem angezeigt.
- **Ergebnis‑Ausgabe** am unteren Panel‑Rand: bei einem Element Länge bzw. Radius, bei zwei Edges Distanz + Winkel, bei drei selektierten Elementen mehrere Werte gleichzeitig (`Area: 4964.000 cm²`, `Length: 169.440 cm` im zweiten Screenshot).
- Unten rechts ein kleines `(x)`‑Icon — vermutlich „Insert as variable" (der Mess­wert landet als neue Measured‑Variable im Feature‑Tree; koppelt an den Measured‑Tab aus §1.11).

**Selection‑Verhalten:** Selektiert werden Proxy‑Elemente. Measure greift in FieldDraft auf die analytisch nachberechneten Contributor‑Rezepte des Proxys zu (siehe README §4.1), nicht auf die Mesh‑Dreiecke — die Zahlen sind damit auf numerische Präzision exakt, nicht durch das Dual‑Contouring begrenzt.

**FD‑Empfehlung:**
- ✅ Explizites Measure‑Panel mit genau diesem Aufbau übernehmen.
- ✅ **Zusätzlich** eine passive DoF‑/Koordinaten‑Anzeige in der unteren Statusleiste (das kostet nichts und hilft beim schnellen Navigieren).
- ✅ „Insert as variable"‑Button, der den aktuellen Messwert als Measured‑Variable in den Feature‑Tree hängt. Das ist die UI‑Seite des *Measured*‑Tabs aus §1.11 und macht Kopplungen wie „Bohrungs­durchmesser = gemessene Länge dieser Edge" möglich.
- ✅ Multi‑Select‑Semantik wie Onshape: beliebige Kombination aus Faces/Edges/Vertices, Measure zeigt alle sinnvollen Werte gleichzeitig.
- ➕ Reference‑Coordinate‑System‑Option im MVP als „World / Mate Connector auswählen" realisieren.
- ➕ Length/Angle‑Unit‑Dropdowns erben die Workspace‑Defaults, können aber per Measure overridet werden, ohne die Workspace‑Einstellung zu ändern.

**Shortcut:** per JSON‑Keybinding konfigurierbar (siehe §4.3).

---

### 1.14 Ansichts‑Würfel & Kamera

**Ansichts‑Würfel:**
- Oben rechts im Viewport. Klick auf Fläche → orthogonale Standardansicht. Klick auf Kante/Ecke → isometrische/achsenausgerichtete Ansicht. Drag → Orbit.

**View‑Menü (aus `assets/onshape-3d-view-menu.png`):**

Das View‑Menü hängt direkt am Ansichts‑Würfel‑Icon und enthält:

- **Isometric / Dimetric / Trimetric** — drei Standard‑3D‑Ansichten.
- **Graphics preferences…** — öffnet einen Sub‑Dialog mit Rendering‑Einstellungen (Shading‑Qualität, Anti‑Aliasing …).
- **Named views…** — Liste gespeicherter Kameraposen. Der User kann beliebige Ansichten als „My View 1" speichern und per Klick zurückspringen.
- **Previous view** — letzten Kamera‑State wiederherstellen (wie ein Cam‑Undo).
- **Zoom to fit** — alle Bodies sichtbar einpassen.
- **Zoom to window** — Rechteck im Viewport aufziehen, Kamera zoomt auf den Bereich.
- **Perspective view** — Toggle zwischen perspektivischer und orthogonaler Projektion.
- **Shaded with edges** (Sub‑Menü): `Shaded with edges`, `Shaded without edges`, `Unshaded`, `Translucent` — Rendering‑Modi.
- **Hidden edges removed** — Toggle für versteckte Kanten.
- **Tangent edges visible** — Toggle, ob tangentiale Kanten (an weichen Übergängen) sichtbar sind.
- **View in high quality** — Render‑Pass mit höherer Abtastrate.
- **Highlight boundary edges** — betont Kanten, die an nur einer Face hängen (nützlich für Debugging offener Meshes).
- **Section view…** — öffnet den Section‑View‑Dialog mit Plane + Gizmo zum Live‑Verschieben.

**Maus‑Defaults in Onshape:**
- **Linke Maustaste** — Multi‑Selektion und Gizmo‑Manipulation.
- **Mittlere Maustaste** — Kamera‑Pan.
- **Rechte Maustaste** — Rotation um den Bounding‑Box‑Mittelpunkt **der gerade sichtbaren Geometrie** (clip‑basiert, nicht Szene‑global). Das ist der überraschend gute Onshape‑Default.
- **Scroll** — Zoom.

**FD‑Empfehlung:**
- ✅ View Cube oben rechts.
- ✅ View‑Menü mit **allen** Einträgen oben (Isometric/Dimetric/Trimetric, Named Views, Previous view, Zoom to fit/window, Perspective, Shading‑Modi, Section view).
- ✅ Tangent‑Edges‑Toggle ist für die Ghost‑Topology‑Visualisierung wichtig: tangentiale Kanten sind im visual path oft nicht sichtbar, der User will sie aber beim Debugging einblenden können.
- ✅ **Maus‑Defaults wie Onshape** (links = Select, mitte = Pan, rechts = Orbit um Clip‑BBox‑Mittelpunkt). Preset‑Alternativen für Blender‑ und Plasticity‑User in den Settings.
- ✅ **Section View mit Gizmo** im MVP (vom User im offenen Punkte bestätigt).
- ✅ Named Views als JSON‑Persistenz im Projekt‑Dokument.
- 🔸 „View in high quality" und „Highlight boundary edges" sind Phase 2 — hübsch, aber nicht MVP‑kritisch.

---

### 1.15 Feature Tree / Outliner

**Onshape‑Verhalten:**
- Linke Panel‑Spalte.
- **Features‑Liste** chronologisch.
  - **Search** über dem Tree
  - **Default‑Geometry**‑Gruppe oben (Origin, Top/Front/Right Planes).
  - **Sketches** erscheinen als eigene Einträge, typischerweise vor dem Feature, das sie konsumiert.
  - **Rollback‑Bar**: man kann die Tree‑Historie temporär an einem Punkt einfrieren, um zu sehen, wie das Modell dort aussah, und dort neue Features einschieben.
  - **Reorder** per Drag & Drop, mit Gültigkeitsprüfung (Abhängigkeiten dürfen nicht rückwärts verletzt werden).
  - **Suppress/Unsuppress** per Rechtsklick oder Kontextmenü.
  - **Edit feature** per Doppelklick → öffnet den originalen Dialog, nur dieses Mal im Edit‑Modus.
  - **Rename** per Doppelklick auf den Namen.
- **Parts-Liste** separat (Ergebnis‑Körper).
  - View-Icon-Toggle
  - Supress-Icon-Toggle
  - **Rename** per Doppelklick auf den Namen.

**FD‑Empfehlung:**
- ✅ Alles übernehmen.
- ➕ **Broken‑Reference‑Highlighting** (siehe Risiko 3 in README): Features mit kaputten Vorgängern erscheinen rot mit Tooltip, der den fehlenden Referenz‑Pfad zeigt.
- ➕ **Variable‑Inline‑Preview**: Variablen‑Features zeigen den aktuell ausgewerteten Wert rechts neben dem Namen.
- 🔸 Rollback‑Bar im MVP als einfacher horizontaler Strich ohne Drag‑Animation — Hauptsache die Semantik stimmt.

---

### 1.16 Undo / Redo

**Onshape‑Verhalten:**
- `Ctrl+Z` / `Ctrl+Shift+Z` (oder `Ctrl+Y`).
- Stack ist pro Document, übersteht die Session (wird serverseitig gespeichert).

**FD‑Empfehlung:**
- ✅ Undo/Redo über Feature‑Tree‑Historie.
- 🔸 MVP: Undo‑Stack nur Session‑lokal (im Browser), nicht persistiert — genügt und ist billiger.
- ➕ Undo umfasst auch Variable‑Änderungen und Sichtbarkeits‑Toggles im Outliner, nicht nur Geometrie.

---

### 1.17 Hole

**Dialog‑Struktur (aus `assets/3d-hole.png`):**

Hole ist in Onshape ein **dediziertes Feature**, nicht eine Boolean‑Subtraktion mit einem handgebauten Zylinder. Der Dialog hat:

- **Unit‑Tabs** oben: **Inch / Metric** — bestimmt den Normen‑Satz für Drill‑Sizes und Standard‑Bohrungen.
- **Hole‑Typ‑Tabs:** **Simple / Counterbore / Countersink**.
- **Sketch points to place holes** — Multi‑Select von Sketch‑Punkten (ein Sketch mit n Punkten erzeugt n Bohrungen).
- **Merge scope** — welche Target‑Bodies durchbohrt werden.
- **Hole type** (Dropdown) — z. B. `Drilled`, `Tapped`, `Clearance`, `Through`. Je nach Typ ändert sich, welche Norm‑Tabelle gilt.
- **Drill size** (Dropdown) — im Screenshot `5`, entspricht metrischer 5er Bohrung.
- **Depth** (mit Icon) — im Screenshot `0.5 cm`.
- **Start plane** (Dropdown) — `Start from selected plane` oder vom Profil‑Sketch.
- **Hole start plane or mate connector** — optional, definiert die Start‑Ebene explizit.
- **Termination** (Dropdown) — `Blind`, `Through all`, `Up to face` etc., analog Extrude‑End‑Conditions.
- **Counterbore‑/Countersink‑spezifische Maße:**
  - Counterbore: Diameter, Depth der Senkung.
  - Countersink: Diameter, Angle (typisch 90° oder 82° für Inch).
- **Drill tip angle** — im Screenshot `118 deg`, der Standard‑Winkel einer HSS‑Bohrspitze; generiert einen konischen Boden, der dem echten gebohrten Loch entspricht.

**SDF‑Umsetzung in FieldDraft:** Hole ist konzeptionell ein **Makro** über den primitiven Kernel‑Operationen:

1. Ein Zylinder‑SDF (Durchmesser aus Drill size) wird als Tool vorbereitet, optional mit konischem Boden (Drill tip angle → Kegel‑SDF unterhalb des Zylinders).
2. Bei Counterbore wird ein zweiter, weiterer Zylinder darüber als Union angehängt; bei Countersink ein Kegel.
3. Das zusammengesetzte Tool wird per Pattern (Anzahl der Sketch‑Punkte) an jede Punkt‑Position verschoben.
4. Boolean‑Difference gegen die Target‑Bodies aus dem Merge‑Scope.

Der User sieht einen einzigen Feature‑Tree‑Eintrag „Hole 1" statt einer Kette aus Extrude‑Difference + Pattern; intern ist es eine Makro‑Expansion, die bei Parameter‑Änderung neu aufgebaut wird.

**FD‑Empfehlung:**
- ✅ Hole als dediziertes Feature im MVP — der Shortcut in die Praxis ist zu wertvoll, um User zu zwingen, Bohrungen manuell als Extrude‑Subtract zu modellieren.
- ✅ **Simple / Counterbore / Countersink** als drei Hole‑Typen.
- ✅ Metric‑Tab mit den Standard‑Drill‑Sizes (M3, M4, M5, M6, M8, M10, M12, M16, M20, M24) als Vorauswahl; Inch‑Tab mit UNC/UNF‑Pendant.
- 🔸 **Tapped holes** (Gewindebohrung mit Gewinde‑Profil im Loch) verweisen auf das Thread‑Feature (§1.26). Im Hole‑Dialog kann die Tapped‑Option das Thread‑Feature automatisch nachziehen.
- ✅ Merge scope wie bei Extrude.
- ✅ Termination wie bei Extrude (Blind, Through all, Up to face).
- ✅ Drill tip angle als optionales Feld (default 118°, abschaltbar für saubere Flachböden).
- ✅ Alle numerischen Felder mit Formel‑Support und Workspace‑Unit‑Respekt.

---

### 1.18 Fillet

**Dialog‑Struktur (aus `assets/3d-fillet.png`):**

Der Onshape‑Fillet‑Dialog ist sehr reich. FieldDraft übernimmt im MVP nur die harte Grundfunktion plus Tangent‑Propagation; die fortgeschrittenen Optionen sind Phase 2.

**MVP‑relevante Felder:**

- **Edge / Full round** — zwei Tabs. Edge ist die Standard­variante (Radius entlang einer selektierten Edge), Full round rundet eine dazwischenliegende Face komplett mit dem Wert aus den beiden angrenzenden Faces ab.
- **Entities to fillet** — Multi‑Select von Proxy‑Edges.
- **Tangent propagation** (Checkbox) — wenn aktiv, zieht das Fillet sich automatisch über tangential anschließende Edges weiter (= das Edge‑Chaining‑Problem aus README §11 Risiko 8). Der User klickt eine Edge und bekommt einen geschlossenen Fillet‑Zug als einen Knoten, statt jedes Segment einzeln markieren zu müssen.
- **Measurement** (Dropdown) — `Radius` als Default. Weitere Modi existieren (`Setback`, `Distance`), sind Phase 2.
- **Radius** — Zahl mit Einheit, Formel erlaubt.

**Phase‑2‑/Späte‑Felder (nicht MVP):**

- **Asymmetric / Second radius** — unterschiedliche Radien auf den beiden beteiligten Faces. Im SDF lösbar als zwei verschiedene Smooth‑Union‑Parameter.
- **Partial fillet** (mit Start/End position + Second bound) — Fillet läuft nicht über die gesamte Edge, sondern nur über ein definiertes Segment.
- **Variable fillet** (Vertices + Points on edges + Smooth transition) — Radius variiert entlang der Edge.
- **Allow edge overflow** — Fillet darf über die ursprüngliche Edge hinaus auf angrenzende Faces auslaufen.
- **Smooth fillet corners** — Vertex‑Blending an Eckpunkten (Zimmerecken‑Problem aus README §11 Risiko 5).
- **Corners to exclude** — bestimmte Vertices von der Ecken‑Verschmelzung ausnehmen.

**SDF‑Umsetzung (FieldDraft):** Spatial‑Mask‑Operator entlang der selektierten Proxy‑Edge, weicher Auslauf an den Kapsel‑Enden, innerhalb der Maske `smin`‑Blend, außerhalb harte Booleans (siehe README §4.1, §11 Risiko 5–7 und K10). Ghost Topology sorgt dafür, dass Picking/Snapping auf der virtuellen scharfen Original‑Edge bleiben.

**FD‑Empfehlung MVP:**
- ✅ **Edge‑Tab** mit Tangent‑Propagation, Radius, Multi‑Select.
- ✅ Tangent‑Propagation greift nur, wenn die Proxy‑Edge‑Nachbarschafts­analyse robust steht. Onshape‑Verhalten: der Check ist eine Toleranz‑Abfrage auf den Edge‑Tangentialvektor an den geteilten Vertices.
- ✅ Selektion ausschließlich über den Proxy (Contributor‑Rezepte, nicht Mesh‑Dreiecke).
- ➖ Im MVP **nicht**: Full round, Asymmetric, Partial, Variable, Allow edge overflow, Smooth corners, Corners to exclude. Die sind alle Phase 2 und werden spezifiziert, sobald K10 stabil läuft.
- ➕ **Overlap‑Warnung** (README §11 Risiko 7): wenn zwei benachbarte Fillet‑Spatial‑Masks räumlich interferieren, zeigt die UI eine dezente Warnung mit Radius‑Reduktions‑Vorschlag.

---

### 1.19 Chamfer

**Onshape‑Verhalten:**
- Strukturell identisch zum Fillet‑Dialog, nur mit **Distance** (statt Radius) und optionalem zweitem Distance‑Wert oder einem Winkel.
- Tangent propagation wie beim Fillet.
- Partial chamfer, Variable chamfer, Corners to exclude analog.

**SDF‑Umsetzung:** Derselbe Spatial‑Mask‑Pfad wie das Fillet, nur mit einem linearen Interpolations‑Operator statt `smin` — die Chamfer‑Formel `min(min(f_A, f_B), (f_A + f_B − c) · √(½))` aus README §4.1.

**FD‑Empfehlung MVP:**
- ✅ Edge‑Tab mit Tangent‑Propagation, Distance, Multi‑Select.
- ✅ Symmetrisch als Default; asymmetrischer Chamfer (zwei unterschiedliche Distanzen) Phase 2.
- ➖ Partial/Variable/Corner‑Exclude Phase 2.

---

### 1.20 Shell

**Dialog‑Struktur (aus `assets/3d-shell.png`):**

- **Hollow** (Checkbox) — wenn aktiv, wird der komplette Body ausgehöhlt, ohne dass eine Face entfernt wird (entspricht einem reinen Offset nach innen).
- **Faces to remove** — Multi‑Select von Proxy‑Faces. Diese werden nach dem Aushöhlen offen gelassen, sodass eine „offene Schale" entsteht. Typischer Fall: Gehäuse, bei dem der Deckel weggenommen ist.
- **Shell thickness** — Wandstärke, Zahl mit Einheit.

**SDF‑Umsetzung:** Die Shell‑Operation ist in der SDF‑Welt mathematisch sehr einfach — `f_shell(p) = max(f_body(p), −(f_body(p) + thickness))` erzeugt eine Hülle konstanter Dicke. Das Entfernen von Faces ist komplizierter, weil es ein Contributor‑basiertes Maskieren erfordert: die Shell‑Operation wird in den Regionen, wo der dominante Contributor eine der selektierten Faces ist, nicht angewandt.

**FD‑Empfehlung:**
- ✅ **Hollow**‑Modus (ohne Faces to remove) im MVP — das ist der triviale SDF‑Fall.
- 🔸 **Faces to remove** erst, wenn die Face‑ID‑Maskierung stabil aus dem B‑Rep‑Proxy fällt. Das ist direkt mit dem Feature‑ID‑Tagging‑Problem aus README §11 verkoppelt.
- ✅ Shell thickness mit Workspace‑Unit‑Respekt.

**Status:** Shell ist in der aktuellen README §6.1 **nicht explizit gelistet**. Mein Vorschlag für §5 (Abgleich): in den Feature‑Tabellen als **MVP‑Kandidat** ergänzen, mit Verweis auf diese Spezifikation.

---

### 1.21 Thicken

**Dialog‑Struktur (aus `assets/3d-thicken.png`):**

- **New / Add / Remove / Intersect**‑Tabs.
- **Faces and surfaces to thicken** — Multi‑Select von Proxy‑Faces.
- **Mid plane** (Checkbox) — symmetrisches Aufdicken in beide Richtungen.
- **Thickness 1** — Zahl mit Einheit.
- **Thickness 2** — zweite Richtung, default 0.
- **Keep tools** (Checkbox).

**FD‑Empfehlung:**
- 🔸 Thicken ist in Onshape primär für **Surface‑Bodies** relevant — Flächen, die keinen eigenen Volumen‑Anteil haben. Da FieldDraft keine Surface‑Bodies unterstützt (SDF kennt nur Volumina, nicht offene Flächen), ist Thicken im MVP **nicht nötig**.
- ➖ **Nicht im MVP.** Ein späterer Bedarf könnte entstehen, wenn FieldDraft irgendwann Import von offenen Meshes erlaubt, die der User als Start‑Geometrie verwenden will. Dann wäre Thicken die Brücke in den Solid‑Bereich.

---

### 1.22 Sweep

**Dialog‑Struktur (aus `assets/3d-sweep.png`):**

- **Solid / Surface / Thin**‑Tabs (wie Extrude — FD nutzt nur Solid).
- **New / Add / Remove / Intersect**‑Tabs.
- **Faces and sketch regions to sweep** — Profil‑Auswahl.
- **Sweep path** — Kurven‑Auswahl: Sketch‑Linien/Kurven, Proxy‑Edges oder eine 3D‑Helix (§1.24) als Pfad.
- **Profile control** (Dropdown) — `None` als Default. Weitere Modi: `Keep profile orientation`, `Twist with path`, `Align with guide rail`.
- **Twist** (Checkbox, collapsible):
  - **Twist type** (Dropdown) — `Turns`, `Angle`, `Length per turn`.
  - **Revolutions** / **Angle** — je nach Typ.
- **Scale** (Checkbox) — das Profil wird entlang des Pfades skaliert (konisch zulaufender Sweep).

**SDF‑Umsetzung (FieldDraft):** Sweep ist in der SDF‑Welt **nicht** analytisch sauber darstellbar, weil die Lipschitz‑Bedingung auf verdrehten 3D‑Pfaden verletzt wird. FieldDraft geht deshalb den Umweg über **Mesh‑to‑SDF** (README K13): Der Kernel erzeugt das Sweep als wasserdichtes Dreiecks‑Mesh mit klassischen Polygon‑Algorithmen (Profil entlang Pfad, mit optionalem Twist und Scale), hängt es als Mesh‑to‑SDF‑Knoten in den CSG‑Baum und nutzt dafür denselben Code‑Pfad wie der STL/OBJ‑Import.

**FD‑Empfehlung:**
- ✅ Sweep als MVP‑Feature, über den Mesh‑to‑SDF‑Pfad (README K13).
- ✅ Nur **Solid**‑Tab.
- ✅ Profile‑Auswahl + Sweep‑path.
- ✅ **Twist** (Turns, Angle) — trivial im Mesh‑Generator.
- ✅ **Scale** — linear entlang der Pfadlänge, einfacher zweiter Parameter.
- 🔸 **Profile control** im MVP nur mit `None` und `Keep profile orientation`. `Align with guide rail` (ein zweiter Kurven‑Input) ist Phase 2.
- ➕ **Sweep‑Resampling‑Toleranz** als Setting: weil das Mesh nachträglich in ein SDF überführt wird, muss die Sehnen­fehler‑Toleranz pro Sweep steuerbar sein (default 0,05 mm wie bei Splines).
- ➕ Bei Änderung des Pfades oder Profils wird das Sweep‑Mesh komplett neu „gebacken", siehe README K13.

---

### 1.23 Loft

**Dialog‑Struktur (aus `assets/3d-loft.png`):**

- **Solid / Surface / Thin**, **New / Add / Remove / Intersect**.
- **Profiles** (required) — zwei oder mehr Profile (Sketch‑Regionen oder planare Faces).
- **End conditions** (collapsible):
  - Start profile condition (None / Normal / Tangent to face …).
  - End profile condition (analog).
- **Guides and continuity** (Checkbox) — optional Guides‑Kurven, die den Loft‑Verlauf steuern.
- **Path** (Checkbox) — eine Zentral­kurve, an der die Profile aufgereiht werden.
- **Section count** — im Screenshot `5`, Anzahl der Zwischen­schnitte.
- **Connections** (Checkbox) — Punkt‑zu‑Punkt‑Zuordnung zwischen den Profilen manuell steuerbar.
- **Show isocurves** (Checkbox) — Visualisierungs­option.

**Status in FieldDraft:** Loft ist in der README §6.1 explizit als **M14+** markiert — also **nicht MVP**. Die SDF‑Umsetzung wäre wie beim Sweep ein Mesh‑Bake über einen Polygon‑Loft‑Algorithmus und dann Mesh‑to‑SDF. Der Aufwand liegt nicht in der Mathematik, sondern in der UX: Loft mit Guides, Connections und End‑Conditions ist eine der komplexesten Dialog‑Fächer überhaupt in Onshape.

**FD‑Empfehlung:**
- ➖ **Nicht im MVP**, wie in der README bereits festgelegt.
- 🔸 Ab M14+ lässt sich Loft über denselben Mesh‑to‑SDF‑Pfad wie Sweep realisieren (Code‑Sharing). Die Dialog‑UX bleibt die Hauptarbeit.

---

### 1.24 Helix

**Dialog‑Struktur (aus `assets/3d-helix.png`):**

- **Cylinder/Cone / Axis / Circle**‑Tabs oben — drei Modi, wie der User eine Helix definieren kann:
  - **Cylinder/Cone** — eine zylindrische oder konische Face picken, die Helix läuft auf der Manteloberfläche.
  - **Axis** — eine freie Achse + Radius.
  - **Circle** — einen Kreis im Sketch picken, der Durchmesser und Position definiert.
- **Conical or cylindrical face** — das Target‑Face.
- **Input type** (Dropdown) — `Turns`, `Pitch`, `Height`, `Revolutions`. Bestimmt, welche zwei der drei Variablen (Höhe, Umdrehungen, Steigung) vom User gesetzt werden; die dritte folgt aus den anderen.
- **Start condition** (Dropdown) — `Start angle` als Default.
- **Start angle** — z. B. `0 deg`.
- **End condition** (Dropdown) — `Height` / `Turns` / `Pitch`.
- **Target revolutions** / **Target height** / **Target pitch** — je nach End condition.
- **Clockwise / Counterclockwise** (Dropdown).
- **Show start and end profiles** (Checkbox) — zeigt kleine Markierungen für den Start‑ und Endpunkt der Helix.

**Was Helix ist (und was nicht):** Helix in Onshape liefert **eine 3D‑Kurve**, kein Volumen. Die Kurve wird typischerweise als Input für ein Sweep‑Feature benutzt, um eine Spirale aus einem Profil zu erzeugen (Federn, Schneckenförderer, Spiralfedern …). Für Schrauben­gewinde nutzt Onshape eher einen dedizierten Thread‑Shortcut (analytisch, ohne Helix).

**FD‑Empfehlung:**
- ✅ Helix als MVP‑Feature, das eine Kurve im Feature‑Tree ablegt.
- ✅ Mode **Cylinder/Cone** und **Axis**; **Circle**‑Modus Phase 2.
- ✅ Input type und End condition analog Onshape.
- ✅ Die Helix‑Kurve ist im Sketch‑Sinn eine Referenz‑Kurve (nicht editierbar, nur konsumierbar) und wird direkt an das Sweep‑Feature (§1.22) verfüttert.
- 🔸 Für **Schrauben­gewinde** nutzt FieldDraft das dedizierte **Thread**‑Feature (§1.26), weil der analytische Gewinde‑SDF numerisch stabiler ist als ein Sweep entlang einer Helix (README K12).

---

### 1.25 Curve Pattern

**Dialog‑Struktur (aus `assets/3d-curve-pattern.png`):**

- `Part pattern` (Dropdown, analog Linear/Circular).
- **Entities to pattern**.
- **Path to pattern along** — eine Kurve (Sketch, Helix, Proxy‑Edge).
- **Equal spacing** (Dropdown — oder „Distance").
- **Instance count**.
- **Orientation** (Dropdown) — `Tangent to curve`, `Align with world`, `Align with face`.
- **Skip instances**.

**Status in FieldDraft:** Curve Pattern steht in der README §6.1 explizit als **M14+**. Das ist konsistent — Curve Pattern ist das seltenste der drei Pattern‑Features und lässt sich per Sweep mit einem wiederholten Profil fast immer emulieren.

**FD‑Empfehlung:**
- ➖ **Nicht im MVP**.
- 🔸 Ab M14+ als wiederholte Transform‑Anwendung entlang der parametrisierten Kurve umsetzbar.

---

### 1.26 Thread

**Hintergrund:** Thread ist in Onshape ein dedizierter Shortcut für Schrauben­gewinde. In der README ist Thread als eigenständiges Feature in K12 spezifiziert — ein analytisches Gewinde‑Primitiv mit O(1) SDF‑Auswertung, **kein Sweep entlang einer Helix** (der Sweep‑Pfad würde an der Metrik‑Verzerrung der Raum‑Rotation scheitern).

**FD‑Dialog (abgeleitet von der README K12‑Spec, kein Onshape‑Screenshot vorliegend):**

- **Target face** — eine zylindrische Proxy‑Face auf der das Gewinde sitzen soll.
- **Direction** — Innen (für Muttern) oder Außen (für Schrauben).
- **Thread type** (Dropdown) — metrisch M3 / M4 / M5 / M6 / M8 / M10 / M12 / M16 / M20 / M24, Trapez, Zoll UNC/UNF.
- **Length** — Gewinde­länge entlang der Zylinder­achse.
- **Start offset** — Abstand vom Face‑Rand.
- **Right‑handed / Left‑handed**.

**Ghost Topology:** Im logical path wird das Thread als glatter Referenz­zylinder mit Kerndurchmesser behandelt — Snapping fängt auf Mittelachse, Stirnfläche und Steigungs­parameter, nicht auf den spiraligen Flanken des visual path (README K12).

**FD‑Empfehlung:**
- ✅ Thread als MVP‑Feature (README K12).
- ✅ Metric M3–M24 als Vorauswahl, Trapez und UNC/UNF als sekundäre Listen.
- ✅ Schraube ↔ Mutter teilen dieselbe Formel mit invertiertem Vorzeichen.
- ✅ Hole‑Dialog (§1.17) zeigt bei `Hole type = Tapped` automatisch die Thread‑Auswahl und verkettet die beiden Features.

---

### 1.27 Native SDF‑Superkräfte (Gyroid, Noise, Bend/Twist, Morph)

Diese Gruppe ist in Onshape **nicht vorhanden** (außer ansatzweise über FeatureScript‑Custom‑Features) und ist der primäre USP‑Bereich von FieldDraft. README K11 spezifiziert sie im Detail; hier nur die UX‑Seite.

**Gemeinsamkeiten der Dialoge:**
- Einziger gemeinsamer UX‑Pattern: **Terminal‑Feature‑Warnung** — weil diese Operatoren im logical path übersprungen werden, snappt der User auf die glatte Grundform, nicht auf die deformierte Oberfläche. Der Dialog zeigt einen kleinen Hinweis: „Picking snaps to the base surface, not the displaced one."

**Gyroid / TPMS‑Infill:**
- **Target body** — der Host‑Körper, in dem das Infill lebt.
- **Cell size** — Zellengröße.
- **Wall thickness** — Wandstärke.
- **Clipping domain** (optional) — Teil des Hosts ausnehmen.
- **Visual preview** im Viewport mit reduzierter Zellendichte für Performance.

**Procedural Noise / Displacement:**
- **Target face** oder **target body**.
- **Noise type** (Dropdown) — `Perlin`, `Simplex`, `Voronoi`, `Worley`.
- **Scale** / **Amplitude** / **Octaves** / **Seed**.
- **Projection** (Dropdown) — `Triplanar` als Default, `Cylindrical` als Option für Griffe und Knöpfe.

**Domain Deformation (Bend / Twist):**
- **Target body**.
- **Bend**: Axis + Start + End + Angle.
- **Twist**: Axis + Total angle + Start/End.
- **Lipschitz‑Korrektur‑Indikator** in der UI: wenn die Verzerrung die Korrektur an ihre Grenze bringt, zeigt das Panel eine Warnung.

**Morph:**
- **Source body A** / **Source body B**.
- **Blend parameter `t`** — Slider 0–1, animierbar.
- **Animate** (Checkbox) — speichert die Animation für den R3F‑Export (Phase 3, P3).

**FD‑Empfehlung:**
- ✅ Alle vier Features als MVP (README K11).
- ✅ Einheitliches Terminal‑Feature‑Warning‑Pattern im Dialog.
- ✅ Die Dialoge sind bewusst einfach gehalten, weil diese Features primär parametrisch‑generativ sind und wenige Sub‑Optionen brauchen.

---

### 1.28 Workspace Units

**Dialog‑Struktur (aus `assets/workspace-units.png`):**

Onshape hat einen dedizierten Workspace‑Units‑Dialog, der die Default‑Einheiten für das gesamte Projekt setzt. Drei Sektionen:

**Length:**
- Length default unit (Centimeter, Millimeter, Meter, Inch, Foot, …).
- Display decimals (0.123, 0.12, 0.1, 0.1234, …) — Anzahl der Nachkommastellen in der UI.
- Linear acceleration default unit (Meter per second squared, …).

**Angle:**
- Angle default unit (Degree, Radian).
- Display decimals.
- Angular velocity default unit (Degree per second, Radian per second, …).

**Mechanical:**
- Mass (Gram, Kilogram, Pound, …).
- Density (Gram per cubic centimeter, Kilogram per cubic meter, …).
- Force (Kilonewton, Newton, Pound‑force, …).
- Moment (Newton‑meter, Pound‑foot, …).
- Pressure (Pascal, kPa, MPa, psi, …).
- Energy (Joule, Calorie, BTU, …).

Jedes Feld hat zwei Dropdowns nebeneinander: die Einheit selbst und die Anzeige­präzision.

**Warum das kritisch ist:** Ohne Workspace‑Units muss der User in jedem einzelnen Dialog die Einheit mittippen, was bei einem CAD mit Dutzenden numerischen Feldern schnell nervig wird. Onshape setzt einen globalen Default, und jedes Feld übernimmt ihn — der User kann aber pro Feld override einer anderen Einheit tippen (`20 in` in einem metrischen Workspace wird korrekt konvertiert).

**FD‑Empfehlung:**
- ✅ **Workspace Units im MVP** — der Dialog gehört in die Projekt‑Einstellungen, erreichbar über einen Zahnrad‑Icon im Outliner‑Panel.
- ✅ Length, Angle, Mass als MVP‑Sektionen. Density/Force/Moment/Pressure/Energy werden erst in Phase 3 (P4, Physikalische Abfragen) relevant.
- ✅ Jede Zahl‑Eingabe im Kernel trägt eine Einheit; wird keine explizite Einheit getippt, wird die Workspace‑Default‑Einheit angenommen.
- ✅ Display decimals pro Einheit — die Anzahl der Nachkommastellen in der Anzeige (betrifft nicht die interne Präzision).
- ➕ **Status:** Workspace Units ist in der aktuellen README **nicht explizit gelistet**. Mein Vorschlag für §5 (Abgleich): in der README §4.5 (Formel‑Engine) oder §6 (Funktionsumfang) als expliziten MVP‑Punkt aufnehmen.

---

### 1.29 Context Menus (Rechtsklick)

**Face‑Select‑Kontextmenü (aus `assets/onshape-3d-face-select-context-menu.png`):**

Wenn der User im 3D‑Viewport eine Proxy‑Face anklickt und rechtsklickt, bietet Onshape ein reiches Kontextmenü mit direkten Feature‑Starts aus der Selektion heraus: Edit Extrude…, Edit Sketch…, Extrude, Offset plane, New sketch, Delete face, Hide face, Isolate, Select other, Create selection, Export as DXF/DWG, Zoom to selection, Section view…, Look normal to, View mates…, Add comment, Add parametric attribute.

Das ist kein Zusatz‑Feature, sondern die **zweite Haupt‑Interaktionsform** in Onshape — viele erfahrene User starten 80 % ihrer Operationen aus dem Kontextmenü statt aus der Toolbar, weil der Klickweg kürzer ist.

**Sketch‑Line‑Kontextmenü (aus `assets/onshape-sketch-line-select-context-menu.png`):**

Im Sketch‑Modus, Rechtsklick mit selektierter Linie:
- **Confirm Sketch 1** (Sketch bestätigen).
- **Copy sketch entities** / **Transform sketch entities**.
- **Construction** (selektierte Entities zu Hilfslinien toggeln).
- **Extrude…** / **Revolve…** — direkter Shortcut: Sketch beenden und sofort Extrude/Revolve mit der Sketch‑Region.
- **Hide other sketches** / **Hide all sketches**.
- **Curve/surface analysis…**.
- **Clear selection**.
- **Select** (Submenü): `Tangent connected edges`, `Equal length/radius edges`, `Parallel edges`, `Create selection…`.
- **Select other…**.
- **Add comment**.
- **Zoom to fit** / **Zoom to selection**.
- **View normal to sketch plane** / **Align view with vertical axis**.
- **Delete sketch entity**.

**Sketch‑Nothing‑Kontextmenü (aus `assets/onshape-sketch-nothing-select-context-menu.png`):**

Rechtsklick im leeren Bereich des Sketches: Confirm Sketch 1, Copy sketch, Show all, Curve/surface analysis, Select, Select other, Zoom to fit, View normal to sketch plane. Deutlich kürzer, weil ohne Selektions­kontext.

**Semantik „Select > Tangent connected edges":** Dieser Select‑Helper ist der UX‑Ausdruck der *Edge‑Chaining*‑Operation aus dem Fillet‑Dialog (§1.18 „Tangent propagation"). Der User kann die Multi‑Selektion vorbereiten und dann ein beliebiges Folge‑Tool darauf anwenden, nicht nur Fillet.

**FD‑Empfehlung:**
- ✅ **Kontextmenüs als erste‑Klasse‑Interaktionsform** im MVP.
- ✅ Face‑Select‑Kontextmenü mit mindestens: Edit Feature, New Sketch on this face, Extrude, Hide, Isolate, Select other, Zoom to selection, Look normal to, Measure, Delete feature.
- ✅ Sketch‑Kontextmenü mit mindestens: Confirm sketch, Construction toggle, Extrude…, Revolve…, Zoom to fit, Select → `Tangent connected`, Delete.
- ✅ Leere‑Sketch‑Kontextmenü als reduzierte Variante.
- ✅ **„Select > Tangent connected edges"** als Proxy‑Edge‑Propagation implementieren — derselbe Algorithmus, der vom Fillet‑Tool für Tangent propagation genutzt wird.
- ➕ **Kontextmenü‑Auswahl respektiert Multi‑Select:** mit mehreren selektierten Faces werden Operationen angeboten, die auf Multi‑Selection Sinn ergeben (Hide, Isolate, Extrude auf mehrere Profile).

---

## 2. Sketch‑Editor

### 2.1 Sketch‑Modus: allgemeine UX

**Onshape:**
- Eigener Modus mit eigener Toolbar.
- Farb‑Konvention für Sketch‑Entities:
  - **Blau** = under‑constrained (per Maus ziehbar).
  - **Schwarz** = fully‑constrained (alle Freiheitsgrade gebunden).
  - **Rot** = over‑constrained / Konflikt.
  - **Gestrichelt** = Construction (Hilfs‑Linien, nicht Teil von Extrude‑Profilen).
- Unten in der Statuszeile: „X degrees of freedom" oder „Fully constrained".
- Escape beendet aktuelles Tool, nicht den Sketch.
- Finish per grünem Häkchen in der Sketch‑Toolbar.

**FD‑Empfehlung:** ✅ übernehmen, inkl. Farbcodierung und DoF‑Anzeige. Die Farb‑Convention ist ein kritischer Teil der UX, den User aus Onshape instinktiv erwarten.

---

### 2.1a Drag‑Solving (Echtzeit‑Constraint‑Lösung beim Ziehen)

**Onshape‑Verhalten:**
- Under‑constrained Entities (blau) sind per Mausklick greifbar und ziehbar.
- Während des Ziehens wird der Constraint‑Solver **bei jedem Frame** aufgerufen. Alle anderen Entities, die über Constraints mit dem gezogenen Punkt verbunden sind, bewegen sich live mit.
- Die Lösung bleibt **topologisch stabil**: ein Rechteck invertiert sich nicht plötzlich, ein Kreis springt nicht auf die andere Seite einer Tangente. Onshape (und CATIA, SolidWorks) erreichen das, indem der Solver die aktuelle Geometrie als Startwert nimmt und die **nächstliegende** Lösung sucht, nicht irgendeine.
- Fully‑constrained Entities (schwarz) sind **nicht** ziehbar — der Cursor zeigt ein Schloss‑Icon beim Hover.
- Over‑constrained Entities (rot) sind ebenfalls nicht ziehbar.

**Technischer Hintergrund (PlaneGCS):**
- Beim Drag‑Start setzt die Adapter‑Schicht einen **temporären Constraint** (`temporary: true`) vom Typ Point‑on‑Point zwischen dem gezogenen Sketch‑Punkt und einem virtuellen „Cursor‑Punkt".
- Bei jeder Mausbewegung werden die Koordinaten des Cursor‑Punkts aktualisiert und `solve()` aufgerufen. PlaneGCS wechselt bei Vorhandensein temporärer Constraints automatisch auf den **SQP‑Algorithmus**, der den temporären Constraint als „weiche" Priorität behandelt: er versucht, ihn so gut wie möglich zu erfüllen, ohne harte Constraints zu verletzen.
- Beim Loslassen wird der temporäre Constraint entfernt und ein letztes `solve()` ohne temporäre Constraints ausgeführt, um die Geometrie in einen sauberen Zustand zu bringen.
- Durch die Nutzung des aktuellen Zustands als Startwert konvergiert der Solver in wenigen Iterationen (< 1 ms für typische Sketches) und springt nicht in eine andere Lösung.

**Bekannte Grenzfälle:**
- Bei schnellen Mausbewegungen (großer Sprung zwischen zwei Frames) kann der Solver dennoch in ein anderes Lösungs‑Bassin springen. Gegenmaßnahme: bei Sprüngen > Schwellwert (z. B. 50 px) interpolierte Zwischenschritte einfügen (2–3 Solve‑Calls auf dem Pfad).
- Bei hochgradig verbundenen Sketches (> 200 Constraints) kann die Solve‑Zeit über 16 ms steigen und den Frame‑Takt verfehlen. Gegenmaßnahme: Solver auf 30 Hz drosseln, letzten gelösten Zustand für Zwischenframes interpolieren.

**FD‑Empfehlung:**
- ✅ Live‑Drag‑Solving übernehmen — das ist eine der Kern‑Erwartungen an einen parametrischen Sketcher.
- ✅ Temporäre Constraints als Solver‑Mechanismus (PlaneGCS‑nativ).
- ➕ Für BeamDraft: Drag‑Toleranz auf Touch‑Input anpassen (größerer Snap‑Radius, 30‑Hz‑Drosselung statt 60 Hz).
- ➕ Frame‑Budget‑Monitor: wenn `solve()` > 8 ms dauert, UI‑Indikator „Solving…" statt stummem Lag.

---

### 2.2 Line

**Onshape:**
- Klick → Klick = ein Segment.
- Nächster Klick erzeugt anschließendes Segment (Kette).
- Esc oder Doppelklick beendet die Kette.
- **Auto‑Constraints während des Zeichnens**: beim Hovern/Ziehen werden Tangenten, Horizontalen, Vertikalen, Coincidents automatisch vorgeschlagen und beim Klick gesetzt — visuell durch Inferenz‑Icons angezeigt.
- Eingabe über Tastatur: während des Ziehens kann man X/Y‑Koordinaten oder Länge/Winkel eintippen.
- Shortcut: **L** (?).

**FD‑Empfehlung:** ✅ alles übernehmen. Auto‑Constraints sind eine der mächtigsten UX‑Mechaniken von Onshape und **unverzichtbar** für Tempo.

---

### 2.3 Rectangle

**Onshape‑Varianten:**
- **Corner rectangle** — zwei diagonale Punkte. Auto‑setzt Horizontal+Vertical Constraints auf die Kanten.
- **Center rectangle** — Mittelpunkt + Eckpunkt, setzt zusätzlich Symmetry‑Constraints.
- **3‑point rectangle** — zwei Punkte definieren eine Kante, dritter Punkt die Breite. Ermöglicht schräge Rechtecke.

**FD‑Empfehlung:** ✅ alle drei Varianten.

---

### 2.4 Circle

**Onshape‑Varianten:**
- **Center point** — Mittelpunkt + Radius.
- **3‑point** — Kreis durch drei Punkte.
- **Tangent** — tangential zu drei Kurven (?).

**FD‑Empfehlung:** ✅ Center point und 3‑point im MVP. Tangent → Phase 2.

---

### 2.4a Inline‑Dimensioning beim Zeichnen

*Gilt für Line, Rectangle, Circle, Arc und Polygon — zentral dokumentiert, um Dopplungen zu vermeiden.*

**Onshape‑Verhalten:**
- Während der User ein Zeichenwerkzeug aktiv hat und gerade eine Geometrie aufspannt (z. B. den zweiten Punkt einer Linie zieht, den Radius eines Kreises aufzieht, die Ecke eines Rechtecks zieht), kann er **jederzeit eine Zahl tippen**.
- Die Tastatureingabe wird automatisch in ein Dimensions‑Eingabefeld umgeleitet, das neben dem Cursor erscheint.
- Nach Bestätigung (Enter) wird die Geometrie auf den eingegebenen Wert gesetzt und ein entsprechender Dimension‑Constraint automatisch eingefügt.

**Verhalten pro Tool:**

| Tool | Tippen während des Ziehens | Resultat                                                                                                                    |
|---|---|-----------------------------------------------------------------------------------------------------------------------------|
| **Line** | Zahl | Setzt die Länge der aktuellen Linie. Richtung bleibt wie vom Cursor vorgegeben. Ein zweites Tab‑Feld erlaubt Winkeleingabe. |
| **Rectangle (Corner)** | Zahl → Tab → Zahl | Setzt Breite und Höhe. Zwei Eingabefelder erscheinen (Breite × Höhe).                                                       |
| **Rectangle (Center)** | Analog | Setzt halbe Breite und halbe Höhe ab Zentrum.                                                                               |
| **Circle (Center)** | Zahl | Setzt den Radius (oder Diameter — Toggle per Tab‑Taste, analog zum Dimension‑Tool).                                         |
| **Arc** | Zahl | Setzt den Radius des Bogens.                                                                                                |
| **Polygon** | Zahl | Setzt den Radius (inscribed/circumscribed je nach Toggle).                                                                  |

**Flow‑Detail:**
- Der Tastatur‑Listener ist immer aktiv, solange ein Zeichenwerkzeug läuft. Sobald ein Zeichen erkannt wird, das kein Shortcut ist (Ziffer, Punkt, Minus, `=` für Formeln), öffnet sich das Inline‑Eingabefeld.
- Das Feld akzeptiert Formeln (`=width/2 + 5 mm`) und Einheiten.
- Enter bestätigt und beendet die aktuelle Geometrie (bei Line: das aktuelle Segment, nicht die ganze Kette).
- Escape verwirft die Eingabe und kehrt zum freien Ziehen zurück.
- Tab wechselt zwischen den Feldern (Breite ↔ Höhe bei Rectangle, Länge ↔ Winkel bei Line).

**FD‑Empfehlung:**
- ✅ Inline‑Dimensioning für alle Zeichenwerkzeuge im MVP. **Das ist ein massiver Produktivitätsfaktor** — der User kann zeichnen und bemaßen in einem Schritt.
- ✅ Tab‑Wechsel zwischen Feldern.
- ✅ Formeln im Inline‑Feld.
- ✅ Länge/Radius/Winkel übernehmen

---

### 2.5 Ellipse

**Onshape:** Center + Semi‑Major‑Endpunkt + Semi‑Minor‑Endpunkt.

**FD‑Empfehlung:** ✅ übernehmen. Keine zusätzlichen Varianten.

---

### 2.6 Polygon

**Onshape:**
- **Regular polygon**: Center, Eckzahl (≥ 3), Radius (inscribed oder circumscribed toggle).
- Zeichnet gleichseitiges N‑Eck.

**FD‑Empfehlung:** ✅ übernehmen. Inscribed/Circumscribed Toggle ist wichtig für Schraubenschlüssel‑Kompatibilität (SW vs Eckabstand).

---

### 2.7 Spline

**Onshape‑Varianten:**
- **Control‑point spline** — Kontrollpunkte definieren die Form, die Kurve verläuft nicht notwendig durch die Punkte. Nachträgliches Verschieben einzelner Kontrollpunkte.
- **Fit‑point spline (interpolating)** — Kurve verläuft exakt durch die geklickten Punkte. Tangenten‑Handles an den Endpunkten für Richtungskontrolle.
- Onshape hat Fit-Point Spline (interpolierend) als Default — das ist der oberste Eintrag "Spline", der beim Klick auf den Toolbar-Button direkt aktiviert wird. "Spline control point" ist die Control-Point-Variante, und "Bezier" ist ein dritter Modus.

**FD‑Empfehlung:**
- 🔸 Im MVP als **Fit‑point spline** (interpolierend): User klickt Punkte, Kurve geht durch alle Punkte. Das ist intuitiver für Maker (die Punkte liegen dort, wo man sie hingesetzt hat).
- ✅ Nachbearbeitung via Kontrollpunkt‑Drag.
- ✅ **Tangent‑Constraints an Spline‑Endpunkten** — PlaneGCS unterstützt B‑Spline‑Constraints nativ (Point‑on‑Spline, Tangent‑at‑Endpoint). Im MVP realistisch: Tangent‑Constraint zwischen Spline‑Endpunkt und angrenzender Linie/Bogen. Das ermöglicht glatte Übergänge, ohne die Spline‑Form komplett zu constrainen.
- 🔸 Volle Spline‑Constraints (interne Kontrollpunkt‑Constraints, Krümmungs‑Stetigkeit) → Phase 2.
- ✅ Im Sketch‑Solver werden Splines mit ihren **parametrischen Koordinaten** (Kontrollpunkte als Solver‑Variablen) gehalten. Für die SDF‑Auswertung im 3D‑Kernel werden sie in Polylinien mit Sehnenfehler‑Toleranz (0,05 mm) zerlegt — das steht bereits in der README §4.1 und bleibt unverändert.

---

### 2.8 Point

**Onshape:** Ein isolierter Punkt in der Skizze, der als Constraint‑Referenz dient (Bohrungs­zentrum, Referenz­punkt). Keine Geometrie, aber DoF‑relevant.

**FD‑Empfehlung:** ✅ übernehmen.

---

### 2.9 Trim

**Onshape:**
- Aktiviert „Trim"‑Modus.
- Hover über eine Kurve → das Segment zwischen den nächsten Schnittpunkten wird gehighlightet.
- Klick → Segment gelöscht.
- Shortcut: **T** (?).

**FD‑Empfehlung:** ✅ exakt so. Trim ist eine der am häufigsten benutzten Operationen im Sketch.

---

### 2.10 Split

**Onshape:** Klick auf einen Schnittpunkt → teilt die Kurve dort in zwei Segmente. Nützlich, um einzelne Teil‑Segmente zu constrainen.

**FD‑Empfehlung:** ✅ übernehmen.

---

### 2.11 Offset

**Onshape:**
- Auswahl einer oder mehrerer Kurven (auch verkettet, z. B. geschlossenes Profil).
- Eingabe Offset‑Distanz (Formel erlaubt).
- Flip‑Toggle für Richtung.
- Offset‑Ergebnis bekommt automatische Distance‑Constraints zum Original.

**FD‑Empfehlung:** ✅ komplett übernehmen.

---

### 2.12 Mirror (Sketch)

**Onshape:**
- Auswahl Entities + Auswahl einer **Construction‑Line** als Mirror‑Achse.
- Gespiegelte Kopie mit automatischem **Symmetry Constraint** zum Original.

**FD‑Empfehlung:** ✅ übernehmen.

---

### 2.13 Linear / Circular Pattern (Sketch)

**Onshape:**
- Analog zu Part‑Pattern, aber innerhalb der Skizze.
- Linear: Richtung (Sketch‑Linie oder Construction) + Count + Spacing.
- Circular: Center‑Punkt + Count + Angle Range + Equal‑Spacing.

**FD‑Empfehlung:** ✅ übernehmen. Spacing‑Eingaben akzeptieren Formeln.

---

### 2.14 Transform (Sketch)

**Onshape:** Auswahl verschieben, rotieren, skalieren innerhalb der Skizze.

**FD‑Empfehlung:**
- ✅ übernehmen

---

### 2.15 Use / Intersect

**Onshape‑Terminologie‑Klarstellung:** Onshape nennt das Projektions‑Tool **„Use"** (nicht „Project"). Die frühere Bezeichnung „Project" in diesem Dokument war ein Missverständnis.

**Onshape:**
- **Use**: 3D‑Kanten, ‑Vertices oder ‑Punkte eines Bodies werden auf die Skizzen­ebene projiziert. Ergebnis ist eine Referenz‑Kurve im Sketch, die bei Änderung der Quelle **automatisch mitwandert**. Die projizierte Kurve ist read‑only im Sketch und dient als Constraint‑Referenz für neue Zeichnungen.
- **Intersect**: Kurven werden dort erzeugt, wo ein 3D‑Körper die Sketch‑Ebene schneidet. Anders als Use projiziert Intersect nicht, sondern berechnet einen echten Schnitt.

**FD‑Empfehlung:**
- ✅ Beide als zwei explizite Tools im Sketch‑Editor (Use und Intersect).
- ✅ Use‑Ergebnisse sind im Sketch als **externe Referenz** markiert (nicht editierbar, nur löschbar), damit der User nicht versehentlich projizierte Kurven verschiebt.
- ✅ Bei Änderung der Quelle wandert die Use‑Kurve automatisch mit — umgesetzt über den Genealogy‑Graph des B‑Rep‑Proxys: Die Sketch‑Entity referenziert die Proxy‑Edge über ihr Contributor‑Rezept, nicht über Koordinaten.
- ➕ Wenn die Quelle gelöscht oder strukturell zerstört wird (Broken‑Reference): Use‑Kurve wird rot markiert, der Sketch rechnet weiter, User sieht den Fehler am Ort der betroffenen Kurve.

---

### 2.16 Toggle Construction

**Onshape:**
- Umschalten zwischen normaler und Hilfslinie.
- Hilfslinien erscheinen **gestrichelt** und tragen nicht zum Extrude‑Profil bei.
- Shortcut: konfigurierbar via JSON‑Keybinding (§4.3).

**FD‑Empfehlung:** ✅ übernehmen. Kritisches Feature für Achsen, Symmetrie‑Linien, Hilfsgeometrie.

---

### 2.17 Dimension

**Onshape — ein einziges „Dimension"‑Tool mit kontextsensitivem Verhalten:**
- Pick 1 Linie → Länge.
- Pick 1 Kreis/Arc → Radius oder Diameter (Toggle per Klick oder Tab).
- Pick 2 Punkte → Distanz (senkrecht / horizontal / vertikal / direkte — abhängig davon, wo man das Label platziert).
- Pick 2 Linien → Winkel oder Parallel‑Distanz (abhängig davon, ob sie parallel sind).
- Pick Punkt + Linie → senkrechter Abstand.
- Klick setzt das Label‑Ziel.
- Popup‑Eingabefeld erscheint, in das man Wert und **Einheit** eintippt. Formel erlaubt.
- Eintrag `=#width/2` referenziert eine Variable.

**Freiheitsgrade:**
- Selection‑Typ × Dimension‑Typ × Label‑Position × Wert (Zahl oder Formel) × Einheit.

**Driving vs Driven:**
- Onshape kennt „driven dimensions" — Maße, die das Modell nicht steuern, sondern nur anzeigen, was gerade gilt. Wird automatisch so gesetzt, wenn ein normales Maß das System over‑constrainen würde.

**FD‑Empfehlung:**
- ✅ Single‑Tool‑Pattern komplett übernehmen. Das ist massiv besser als separate Tools für Linear/Radial/Angular.
- ✅ Formeln direkt im Eingabefeld (`=width/2 + 5 mm`).
- ✅ Driven‑Dimension‑Mode als automatischer Fallback bei over‑constrained (Details in §2.17a).
- ✅ Label‑Position ist persistent (Teil des Sketch‑State, nicht volatil).

---

### 2.17a Driving vs. Driven Dimensions — Detailspezifikation

**Onshape‑Verhalten im Detail:**

**Platzierung:**
- Wenn der User eine Dimension setzt, die den Sketch **über‑constrainen** würde, erkennt Onshape das **vor dem Solver‑Aufruf** (via DoF‑Analyse) und fügt die Dimension automatisch als **Driven** ein — statt einen Fehler zu werfen.
- Eine Driven Dimension erscheint **grau** statt schwarz und ist **nicht direkt editierbar**. Sie zeigt den aktuell berechneten Wert als Referenz an.
- Es gibt kein Modal, keine Fehlermeldung, keinen Bruch im Flow — die Dimension wird einfach grau gesetzt. Der User sieht sofort: „Ah, das System hat erkannt, dass dieser Wert schon durch andere Constraints bestimmt ist."

**Umschalten:**
- Rechtsklick auf eine Dimension → Kontextmenü → **„Change to Driving"** bzw. **„Change to Driven"**.
- Wenn eine Driven Dimension auf Driving umgeschaltet wird und das den Sketch über‑constrainen würde, markiert Onshape den Sketch rot (Over‑constrained). Der User muss dann eine andere Dimension entfernen oder auf Driven setzen.
- Der umgekehrte Weg (Driving → Driven) ist immer sicher und erhöht die DoF um 1.

**Visuelles Erscheinungsbild:**

| Zustand | Textfarbe | Editierbar | Bedeutung |
|---|---|---|---|
| Driving | Schwarz | ✅ Ja | Steuert die Geometrie. |
| Driven | Grau (heller, ggf. kursiv) | ❌ Nein (nur lesen) | Zeigt den resultierenden Wert. |
| Fehlerhaft | Rot | ✅ Ja | Über‑constrained oder nicht lösbar. |

**Technische Umsetzung (PlaneGCS):**

Die Pre‑Flight‑Prüfung vor dem Einfügen einer neuen Dimension läuft so ab:

1. Die Adapter‑Schicht zählt die aktuellen DoF des Sketches (PlaneGCS liefert das über die Jacobi‑Rang‑Analyse in `diagnose()`).
2. Wenn DoF = 0 (fully constrained), wird jede neue Driving Dimension den Sketch über‑constrainen → die Dimension wird automatisch als Driven eingefügt.
3. Wenn DoF > 0, wird die Dimension als Driving eingefügt, der Solver aufgerufen, und geprüft, ob die DoF um genau 1 gesunken sind. Falls nicht (die Dimension war redundant zu bestehenden Constraints), wird sie nachträglich auf Driven umgeschaltet.
4. Driven Dimensions werden **nicht** als Solver‑Constraints eingefügt, sondern als Post‑Solve‑Berechnungen: nach dem Solve‑Pass wertet die Adapter‑Schicht die geometrische Größe aus (Abstand, Winkel, Radius) und schreibt den Wert in das Dimension‑Objekt.

**FD‑Empfehlung:**
- ✅ Automatischer Driving/Driven‑Fallback bei Platzierung.
- ✅ Rechtsklick‑Toggle zwischen Driving und Driven.
- ✅ Grau/Schwarz‑Farbcodierung.
- ✅ Pre‑Flight‑DoF‑Check vor jedem Constraint‑Insert.
- ➕ Tooltip auf Driven Dimensions: „This dimension is determined by other constraints. Right‑click to change to Driving."

---

### 2.18 Constraints — vollständige Liste

**Onshape‑Liste (MVP‑relevant):**

| # | Constraint | Pick | Wirkung |
|---|---|---|---|
| 1 | **Coincident** | 2 Punkte / Punkt+Kurve | Punkte fallen zusammen / Punkt liegt auf Kurve. |
| 2 | **Concentric** | 2 Kreise/Bögen | gleicher Mittelpunkt. |
| 3 | **Parallel** | 2 Linien | parallel. |
| 4 | **Perpendicular** | 2 Linien | 90°. |
| 5 | **Tangent** | Linie+Kurve / Kurve+Kurve | tangential. |
| 6 | **Equal** | 2 Linien / 2 Kreise | gleiche Länge bzw. gleicher Radius. |
| 7 | **Horizontal** | 1 oder mehrere Linien | parallel zur Sketch‑X. |
| 8 | **Vertical** | 1 oder mehrere Linien | parallel zur Sketch‑Y. |
| 9 | **Midpoint** | Punkt + Linie | Punkt auf Mitte der Linie. |
| 10 | **Symmetric** | 2 Entities + Konstruktions­linie | symmetrisch um Achse. |
| 11 | **Fix** | beliebig | Entity wird fixiert (alle DoF entfernt). |
| 12 | **Collinear** | 2 Linien | liegen auf derselben unendlichen Geraden. |

**Auto‑Constraints während des Zeichnens:** Onshape schlägt permanent Horizontal/Vertical/Coincident/Tangent vor und setzt sie per Klick.

**UI‑Mechanik:** Constraints werden als kleine **Icons im Viewport** neben der betroffenen Geometrie angezeigt. Klick auf ein Icon selektiert den Constraint, Delete entfernt ihn.

**FD‑Empfehlung:**
- ✅ Alle 10 aus README §6.2 plus **Fix** und **Collinear** (beide vom User bestätigt; sollten in §6.2 nachgetragen werden — siehe §5.2 Abgleich).
- ✅ Auto‑Constraints während des Zeichnens (unverzichtbar). Details in §2.18a.
- ✅ Icon‑Display im Viewport.
- ✅ **Constraint‑Hover‑Highlight**: Hover über ein Constraint‑Icon markiert alle beteiligten Entities farbig — Onshape macht das so, FieldDraft auch.

---

### 2.18a Auto‑Inferencing (automatische Constraints beim Zeichnen)

**Onshape‑Verhalten:**
- Während der User ein Zeichenwerkzeug (Line, Rectangle, Circle, Arc, …) aktiv hat und den Cursor bewegt, prüft Onshape permanent, ob die aktuelle Cursorposition oder die entstehende Geometrie eine **Inferenz‑Beziehung** zu bestehenden Entities aufweist.
- **Visuelle Indikatoren:**
  - **Gestrichelte orangefarbene Linie** zwischen dem Cursor und dem Referenzpunkt, wenn eine Horizontal‑, Vertical‑ oder Alignment‑Inferenz aktiv ist.
  - **Constraint‑Icon** neben dem Cursor (z. B. ∥ für Parallel, ⊥ für Perpendicular, ○ für Coincident).
  - **Gelbes Highlight** auf dem Referenz‑Entity, zu dem die Inferenz führt.
  - **Snap‑Punkt**: der Cursor rastet sichtbar auf den inferierten Punkt ein (z. B. Endpunkt, Mittelpunkt, Kreiszentrum).
- **Beim Klick** wird der inferierte Constraint automatisch gesetzt — der User muss ihn nicht manuell hinzufügen.
- **Shift‑Taste** unterdrückt alle Inferenzen temporär (solange gehalten). Loslassen reaktiviert sie. Nützlich in dicht bebauten Sketches, wo ungewollte Inferenzen ständig dazwischenfunken.
- **Internes Inferencing** (innerhalb eines Composite‑Tools): Wenn der User ein Rechteck zeichnet, setzt Onshape automatisch Horizontal/Vertical auf die Kanten und Coincident auf die Ecken, ohne dass eine Inferenz‑Anzeige nötig ist — das sind strukturelle Constraints des Tools, nicht inferierte.

**Prioritätsregeln (rekonstruiert aus Onshape‑Verhalten, absteigend):**

| Priorität | Inferenz | Trigger |
|---|---|---|
| 1 | **Coincident auf Endpunkt** | Cursor < Snap‑Radius zu einem existierenden Endpunkt, Vertex oder Sketch‑Origin. |
| 2 | **Coincident auf Mittelpunkt** | Cursor < Snap‑Radius zum Mittelpunkt einer Linie oder eines Bogens. |
| 3 | **Point on Curve** | Cursor < Snap‑Radius zu einer Kurve (aber kein End‑/Mittelpunkt). |
| 4 | **Horizontal / Vertical** | Aktuelle Linie ist innerhalb ±3° zur Sketch‑X‑ oder Sketch‑Y‑Achse, oder der Cursor liegt vertikal/horizontal über einem Referenzpunkt (Origin, anderer Endpunkt). |
| 5 | **Parallel** | Aktuelle Linie ist innerhalb ±3° parallel zu einer existierenden Linie. |
| 6 | **Perpendicular** | Aktuelle Linie ist innerhalb ±3° senkrecht zu einer existierenden Linie. |
| 7 | **Tangent** | Cursor nähert sich einem Kreis/Bogen, und die aktuelle Geometrie steht tangential dazu. |
| 8 | **Equal Length** | (nicht automatisch inferiert — nur manuell.) |

**Proximity‑Schwelle:**
- Der Snap‑Radius ist in **Bildschirm‑Pixeln** definiert (nicht in mm), typischerweise 8–12 px. Damit skaliert er korrekt mit dem Zoom: bei starkem Hereinzoomen wird der Fangbereich in Modell‑Einheiten kleiner, was dem User mehr Kontrolle gibt.
- Wenn mehrere Inferenzen gleichzeitig innerhalb des Snap‑Radius liegen, gewinnt die höchstpriorisierte. Bei Gleichrang gewinnt die räumlich nächste.

**FD‑Empfehlung:**
- ✅ Vollständige Auto‑Inferencing‑Engine im MVP — **das ist das Feature, das den Sketcher von einem Zeichenprogramm zu einem CAD‑Tool macht.**
- ✅ Shift‑zum‑Unterdrücken.
- ✅ Visuelle Indikatoren (gestrichelte Linie + Icon + Highlight).
- 🔸 Im MVP: Coincident, Horizontal, Vertical, Parallel, Perpendicular, Midpoint als inferierte Constraints. Tangent‑Inferenz ist schwieriger (erfordert lokale Krümmungs‑Analyse) und kann Phase 2 sein.
- ➕ Konfigurierbare Snap‑Distanz in den Settings (Default 10 px).
- ➕ Für BeamDraft: Snap‑Radius auf 20–30 px erhöhen (Finger‑Touch statt Maus).

**Architektur‑Hinweis:** Die InferenceEngine ist **kein Solver‑Feature**, sondern eine eigene Komponente in der Adapter‑Schicht zwischen UI und Solver. Sie wird bei jeder Mausbewegung aufgerufen (≤ 1 ms Budget), prüft die aktuelle Geometrie gegen alle existierenden Entities im Sketch und liefert eine priorisierte Liste von Kandidaten‑Constraints zurück. Die UI zeigt den Top‑Kandidaten an. Beim Klick übergibt die Engine den Constraint an den Solver.

---

### 2.19 Constraint Removal

**Onshape:**
- Klick auf Constraint‑Icon → Delete.
- Oder: Selektiere Entity, sehe alle daran hängenden Constraints in einem Panel (?), lösche gezielt.

**FD‑Empfehlung:**
- ✅ Icon‑Klick + Delete.
- ➕ **Rechtsklick auf Entity → „Show constraints"** — öffnet eine kleine Liste aller beteiligten Constraints für gezieltes Löschen. Praktisch bei dicht bebauten Sketches.

---

### 2.20 Error Display (Over/Under‑constrained)

**Onshape‑Verhalten (korrigiert):**
- Statuszeile zeigt: „Fully constrained" / „N degrees of freedom" / „Over‑constrained".
- Entities werden entsprechend farbcodiert (blau/schwarz/rot).
- **Over‑constrained wird nicht verweigert, sondern nur visuell markiert.** Onshape ignoriert die eingefügten Konflikt‑Constraints für die eigentliche Solve‑Pass nicht — sie stehen trotzdem im Sketch, werden aber rot gezeichnet. Der User entscheidet, welchen Constraint er entfernt. Die Skizze bleibt editierbar, und der Solver liefert weiterhin eine (möglicherweise nicht alle Constraints erfüllende) Lösung.
- Bei nicht lösbarer Konvergenz: Fehlermeldung an der betroffenen Stelle, aber kein Blocker.

**FD‑Empfehlung:**
- ✅ Drei‑Farben‑Schema übernehmen.
- ✅ **Over‑constrained visuell markieren, nicht blockieren** — identisch zu Onshape.
- ✅ **Fix‑Suggestions** (Liste der konfliktverursachenden Constraints) als UI‑Unterstützung. PlaneGCS liefert diese Liste nativ über `getConflicting()`, `getRedundant()` und `getPartiallyRedundant()` — die Diagnostics‑Pipeline der Adapter‑Schicht aus README §4.4 bereitet die Rohdaten für die UI auf.
- ➕ **DoF‑Hint im Viewport**: under‑constrained Entities zeigen beim Hover kleine Pfeile in die Richtungen, in die sie noch ziehbar sind.

---

### 2.21 Constraint Manager

**Onshape‑Verhalten:**
- Der Constraint Manager ist ein **diagnostisches Panel**, das alle Constraints eines aktiven Sketches auflistet. Zugang über das „Show sketch diagnostic tools"‑Icon in der Sketch‑Toolbar.
- **Spalten/Informationen pro Constraint:** Typ‑Icon, beteiligte Entities (als Referenz), Status (solved / conflicting / redundant).
- **Filter:** nach Constraint‑Typ (z. B. nur Coincident anzeigen), nach Modus (Driving / Driven), nach Status (nur Fehler anzeigen).
- **Interaktion:**
  - Hover über einen Eintrag → die beteiligten Entities im Viewport werden **gelb** hervorgehoben, der Constraint selbst bekommt eine **Bounding‑Box**.
  - Klick → selektiert den Constraint (kann dann per Delete entfernt werden).
  - „Delete all"‑Button für gefilterte Liste.
  - „Zoom to selection"‑Button fokussiert die Kamera auf die betroffene Region.
- **„Automatically select constraints"**‑Toggle: wenn aktiv (Default), wird bei Klick auf einen Constraint im Panel automatisch die zugehörige Geometrie selektiert. Wenn deaktiviert, kann der User erst einen Region‑Select im Viewport machen und sieht dann nur die Constraints dieser Region im Manager.

**Warum das wichtig ist:**
- Bei Sketches mit 50+ Constraints (realistisch für ein Gehäuse‑Profil mit Rippen und Lochbild) ist visuelles Constraint‑Debugging über die kleinen Icons im Viewport allein nicht mehr praktikabel.
- Die häufigste Frustration in Onshape‑Foren ist: „Mein Sketch ist rot, und ich finde nicht, welcher Constraint das Problem verursacht." Der Constraint Manager mit Status‑Filter löst genau das.

**FD‑Empfehlung (aktualisiert — README §6.2 listet Constraint Manager jetzt als MVP):**
- ✅ **Im MVP.** Die README‑Feature‑Tabelle hat den Constraint Manager explizit als MVP‑Anforderung aufgenommen.
- ✅ Panel mit Liste, Filter nach Typ/Modus/Status, Hover‑Highlight der beteiligten Entities.
- ✅ „Delete all" auf gefilterter Liste, „Zoom to selection", „Automatically select constraints"‑Toggle.
- ✅ Rohdaten aus PlaneGCS' `getConflicting()`/`getRedundant()` über die Diagnostics‑Pipeline der Adapter‑Schicht (README §4.4).
- ➕ Zusätzlich zur Panel‑Ansicht: Rechtsklick auf Entity → „Show constraints" als kleines Inline‑Popover für schnelles Debugging ohne Panel‑Wechsel.

---

### 2.22 Sketch‑Beendigung und Under‑Constrained‑Verhalten

**Onshape‑Verhalten:**
- Der User beendet den Sketch per **grünem Häkchen** (Confirm) oder per Shortcut.
- **Rotes X** verwirft alle Änderungen seit dem letzten Sketch‑Öffnen.
- **Under‑constrained Sketches sind erlaubt.** Onshape erzwingt kein „Fully constrained", bevor ein Sketch bestätigt werden kann. Blaue (under‑constrained) Entities behalten ihre **aktuelle Position** zum Zeitpunkt der Bestätigung.
- Diese Position ist **nicht fixiert** — wenn der User den Sketch später wieder öffnet und einen neuen Constraint hinzufügt, können sich die ehemals blauen Entities verschieben. Das ist gewollt: der User soll iterativ constrainen können, ohne den Sketch in jedem Zwischenschritt vollständig definieren zu müssen.
- **Extrude/Revolve auf under‑constrained Sketches:** Erlaubt. Onshape verwendet die aktuelle Geometrie. Wenn der User später den Sketch ändert und dabei under‑constrained Entities verschiebt, aktualisiert sich das Extrude‑Ergebnis automatisch — was zu unerwarteten Ergebnissen führen kann. Onshape warnt nicht explizit davor, verlässt sich aber auf die blaue Farbcodierung als visuellen Hinweis.
- **Regionen‑Erkennung:** Beim Bestätigen berechnet Onshape die geschlossenen **Regionen** (Flächen) des Sketches — das sind die Flächen, die Extrude/Revolve als Profil verwenden können. Offene Konturen ergeben keine Regionen; der User sieht beim Extrude‑Versuch eine Fehlermeldung „Select one or more sketch regions".

**FD‑Empfehlung:**
- ✅ Under‑constrained Sketches erlauben und bestätigen lassen — **kein Zwang zu Fully Constrained.** Makers wollen schnell iterieren; ein Blocker hier killt den Flow.
- ✅ Aktuelle Positionen bei Bestätigung als „eingefrorener Zustand" in den 2D‑SDF‑Baum übernehmen.
- ➕ **Warnung statt Blocker:** Wenn der User einen under‑constrained Sketch bestätigt, zeigt FD einen dezenten Hinweis in der Statuszeile: „Sketch has N degrees of freedom — some entities may shift if you add constraints later." Kein Modal, kein Popup — nur Information.
- ➕ **Regionen‑Erkennung:** Der Rust‑Kernel berechnet geschlossene Regionen aus dem 2D‑SDF‑Baum (Region = zusammenhängendes Gebiet mit f(p) < 0). Diese Regionen sind die Input‑Flächen für Extrude/Revolve.
- ✅ Grünes Häkchen / Rotes X als UI‑Elemente in der Sketch‑Toolbar.

---

## 3. Variablen & Formel‑Engine

### 3.1 Onshape‑Modell

- **Variable** ist ein Feature‑Tree‑Eintrag mit drei Feldern:
  - **Name** (alphanumerisch, keine Sonderzeichen außer `_`).
  - **Value** — ein Expression mit Einheit, z. B. `50 mm`, `pi * radius`, `sqrt(2) * side`.
  - **Description** (optional, freier Text).
- Referenzierung in anderen Feldern per `#name`.
- Referenzierung einer anderen Variable innerhalb einer Expression per direkten Namen (`pi * radius`).

**FeatureScript‑Unit‑System:**
- Zahlen ohne Einheit sind dimensionslos.
- Zahlen mit Einheit sind typisiert: `mm`, `cm`, `m`, `in`, `ft`, `deg`, `rad`, …
- Arithmetik respektiert Dimensionen: `5 mm + 2 cm` = `25 mm`, aber `5 mm + 30°` ist ein Fehler.
- Winkel‑Funktionen (`sin`, `cos`) erwarten Winkel‑Dimension und liefern dimensionslose Werte.

### 3.2 FD‑Umsetzung

Siehe README §4.5. Hier nur die UX‑Ergänzungen.

**Integration im Feature‑Tree (Korrektur gegenüber früheren Fassungen):** Variablen sind **kein separates Panel**, sondern reguläre Einträge im Feature‑Tree. Sie werden über „New Variable" (toolbar oder Kontextmenü) erzeugt, erscheinen chronologisch zwischen anderen Features und zeigen im Tree‑Label ihren ausgewerteten Wert (`#Box_Length = 20 cm`). Siehe §1.11 für die Dialog‑Struktur inklusive Assigned/Measured/From‑Import‑Tabs.

**Inline‑Editing im Tree:**
- Doppelklick auf den Variable‑Eintrag öffnet den Dialog rechts, identisch zu anderen Features.
- Der Inline‑Wert im Tree‑Label aktualisiert sich sofort nach Commit.
- Drag‑to‑reorder ist kosmetisch möglich — die echten Abhängigkeiten stehen im Variablen‑DAG, nicht in der Tree‑Reihenfolge.

**Inline‑Usage in Feldern:**
- Jedes numerische Feld in jedem Dialog akzeptiert Formeln, erkennbar am `=`‑Prefix oder daran, dass der Parser keine reine Zahl sieht.
- Beispiel: Extrude‑Tiefe = `=height / 2 + 5 mm`.
- Referenzen auf Variablen über `#name` oder direkten Namen (beide akzeptiert).
- Nach Commit zeigt das Feld den ausgewerteten Wert; Klick/Fokus bringt die Expression zurück zum Editieren.
- **Workspace‑Default‑Einheiten** (§1.28) greifen, wenn keine explizite Einheit getippt wird.

**Autocomplete:**
- Beim Tippen in einem Expression‑Feld schlägt das System die Namen aller bisher definierten Variablen (die im Feature‑Tree **vor** dem aktuellen Feature stehen) vor.

**Fehleranzeige:**
- Syntax‑Fehler: rot unterstrichen, Hover‑Tooltip.
- Unbekannter Identifier: rot unterstrichen mit „Unknown variable: `radiuss`. Did you mean `radius`?".
- Einheit‑Mismatch: `Cannot add 5 mm and 30 deg (incompatible units)`.
- Zyklus: `Circular reference: a → b → a`.
- Division durch Null: `Division by zero at runtime` — als Laufzeitfehler, nicht Parser‑Fehler.
- Alle Diagnostics als strukturierte Objekte aus dem Rust‑Kernel, damit das Frontend sie lokal rendern kann.

**Measured Variables (Kopplung zu §1.13 Measure):** Im *Measured*‑Tab des Variable‑Dialogs verknüpft der User die Variable mit dem Ergebnis eines Measure‑Pass. Die Variable trägt dann das Contributor‑Rezept der gemessenen Entities, nicht einen toten Zahlenwert — Modell‑Änderungen, die die gemessene Kante verschieben, aktualisieren die Variable automatisch.

**Beschränkungen:**
- Variablen sind **Werte oder pure Expressions** — keine Funktions­definitionen, keine Schleifen, keine Seiteneffekte. Das ist bewusst so: die Expression‑Engine ist deterministisch und zyklensicher, weil sie rein funktional ist.

---

## 4. Tool‑übergreifende UX‑Konventionen

### 4.1 Selektion

- **Klick** = Single‑Select.
- **Shift+Klick** = Add to Selection.
- **Ctrl+Klick** = Toggle Selection (?).
- **Drag‑Box** = Rechteck‑Selektion, wie in Onshape (?).
- **Selection Filter** (?) — Toolbar‑Buttons „nur Flächen / nur Kanten / nur Vertices auswählen". Im MVP optional.

### 4.2 Feedback

- **Hover‑Highlight**: Entities werden beim Hover hellblau eingefärbt, inklusive Tooltip mit Typ und Name.
- **Pre‑select**: Ein Feature im Tree zu hovern markiert die davon erzeugten Entities im Viewport.
- **Status‑Leiste unten**: permanent aktuelle Maus‑Koordinate, Selektions‑Info, Mess‑Ergebnisse.

### 4.3 Shortcuts

Shortcuts sind in FieldDraft **vollständig konfigurierbar** über eine JSON‑Keybindings‑Datei. Es gibt keine fest verdrahteten Defaults in diesem Dokument — die Zuordnung wird am Ende der Implementierungs­phase auf Basis tatsächlicher Nutzungs­statistiken und Community‑Rückmeldungen festgelegt. Die README §6.3 beschreibt das Keybinding‑System als Teil der „Respekt vor Augen und Tastatur"‑UX‑Prinzipien.

**Design‑Regeln für das Keybinding‑JSON:**

- Jedes Kommando hat einen eindeutigen String‑ID (`sketch.new`, `feature.extrude`, `view.zoomToFit`, …).
- Shortcuts sind Kombinationen aus Modifier‑Keys (`Ctrl`, `Shift`, `Alt`) und einer Taste.
- Ein Shortcut kann mehreren Kontexten zugeordnet sein (global, nur im Sketch‑Modus, nur bei aktivem Feature‑Dialog).
- Kollisionen werden beim Laden erkannt und ans UI gemeldet.
- Der User kann Default‑Bindings per JSON überschreiben oder ergänzen, nicht den Kernel patchen.

**FD‑Empfehlung:**
- ✅ JSON‑basierte Keybindings von Anfang an, kein hardkodiertes Set.
- ✅ Onshape‑Defaults als **vorgeschlagene Startkonfiguration** mitliefern, aber explizit als Beispiel markiert, nicht als Pflicht.
- ✅ Einstellungs‑UI zum Editieren der Bindings ohne JSON‑Datei anzufassen.
- ✅ Presets für Onshape‑, Blender‑ und Plasticity‑Gewohnheiten als einmaliger Import.

### 4.4 Toolbar‑Struktur

- **Feature‑Toolbar** oben: Sketch · Plane · Extrude · Revolve · Boolean · Pattern · Mirror · Transform · Variable · Mate Connector · Measure.
- **Sketch‑Toolbar** (nur im Sketch‑Mode): Line · Rectangle · Circle · Ellipse · Polygon · Spline · Point · Dimension · Trim · Split · Offset · Mirror · Pattern · Transform · Project/Intersect · Construction · Constraints …
- Toolbars sind **rollenbasiert**: im Sketch‑Mode verschwindet die Feature‑Toolbar.

**FD‑Empfehlung:** ✅ genau dieses Rollen‑Modell.

### 4.5 Panels

- **Links:** Feature Tree (mit Default‑Geometry, Features, Parts — alle Variablen sind chronologische Einträge im Feature‑Tree, **kein separater Variablen‑Tab**). Darunter ein Parts‑Unterbereich mit View‑/Suppress‑Toggles und Rename.
- **Rechts:** Aktiv geöffneter Feature‑Dialog (statt Popup — Onshape nutzt Side‑Panels, das ist deutlich ruhiger als Modal‑Dialoge).
- **Rechts, zweite Spalte (optional):** Constraint Manager Panel im Sketch‑Modus (§2.21).
- **Unten:** Status‑Leiste mit DoF/Selektions‑Info/Koordinate/Unit‑Override.
- **Oben:** Toolbar + View‑Options + Ansichts‑Würfel.

### 4.5a Solver‑Status in der Statusleiste

**Onshape‑Verhalten:**
- Die Statusleiste zeigt im Sketch‑Modus permanent: **„Fully constrained"** oder **„N degrees of freedom"** oder **„Over‑constrained"**.
- Kein expliziter Solver‑Status — der User sieht nur das Ergebnis.

**FD‑Empfehlung:**
- ✅ DoF‑Anzeige in der Statusleiste wie Onshape.
- ➕ **Solver‑Performance‑Indikator (nur Debug‑Modus):** zeigt Solve‑Zeit in ms pro Frame. Nicht im normalen UI sichtbar, aber per Tastenkombination (z. B. Ctrl+Shift+P) einblendbar. Nützlich für Performance‑Regression‑Erkennung während der Entwicklung.
- ➕ **Solving‑Indikator bei Lag:** Wenn ein Solve‑Call > 16 ms dauert (Frame‑Budget überschritten), zeigt die Statusleiste kurz „Solving…" statt der DoF‑Zahl, damit der User den Lag als Solver‑Arbeit erkennt und nicht als UI‑Bug.

### 4.6 Fehler‑Verhalten

- Fehler blockieren **nie** den User stumm. Jeder Fehler hat:
  - Ort (welches Feature / welche Variable).
  - Klartext‑Meldung.
  - Optional: Vorschlag zur Behebung.
- Broken‑References werden im Tree rot markiert, das Modell rechnet trotzdem so weit es geht weiter.
- Over‑constrained Sketches verweigern den kaputten Constraint, ohne den ganzen Sketch zu invalidieren.

---

## 5. Abgleich mit README §6

Dieser Abschnitt ist vollständig neu geschrieben, nachdem README §6 zwischenzeitlich stark erweitert wurde (Fillet/Chamfer, Thread, Gyroid/TPMS, Procedural Noise, Domain Deformation, Morph, Sweep sind jetzt MVP — siehe K10–K13). Die Legende in den Tabellen ist:

- **✅** — steht in der README und matcht Onshape‑Praxis.
- **🔸** — steht in der README, braucht aber Konkretisierung.
- **➕** — steht **nicht** in der README, sollte dort ergänzt werden.
- **Spec →** — Verweis auf den konkretisierenden Abschnitt in diesem Dokument.

### 5.1 Part‑Editor (README §6.1)

| Feature | README §6.1 | Spec → | Anmerkung |
|---|---|---|---|
| Feature‑Tree | ✅ | §1.15 | Inkl. Rollback‑Bar, Rename, Suppress, Reorder. |
| Part‑List / Outliner | ✅ | §1.15 | |
| Multiselect | ✅ | §4.1 | Drag‑Box im MVP. |
| Measure (Maßband) | ✅ | §1.13 | **Korrektur:** dediziertes Panel, nicht nur passive Statuszeile. |
| Kamera‑ und Render‑Optionen | ✅ | §1.14 | Perspective/Orthogonal + Shading‑Modi + Named Views. |
| Ansichts‑Würfel | ✅ | §1.14 | |
| Undo / Redo | ✅ | §1.16 | |
| Sketch (Einstieg in 2D) | ✅ | §1.1 | „Direkt auf Face klicken" als UX‑Shortcut. |
| Extrude (Add/Subtract/Intersect/New) | ✅ | §1.2 | Präzisieren: End‑Conditions + Direction + Starting offset + Symmetric + Draft + Second end position. |
| Revolve | ✅ | §1.3 | Mate Connector als valide Achsen­quelle. |
| Boolean (Union/Difference/Intersect) | ✅ | §1.4 | **Offset** und **Keep tools** ergänzen; **Reapply fillet** nicht nötig. |
| Linear Pattern | ✅ | §1.5 | Pattern‑Quelle (Part/Feature/Face), Centered, Second direction, Skip instances. |
| Circular Pattern | ✅ | §1.6 | Full‑Default via Angle = 360° + Equal spacing. |
| Mirror | ✅ | §1.7 | **Korrektur:** Mirrored Parts sind separat editierbar (Features nach dem Mirror‑Knoten wirken unabhängig). |
| Transform (Move/Rotate/Scale) | ✅ | §1.8 | **Korrektur:** Scale ist Teil der MVP‑Modi (sieben Modi insgesamt). |
| Delete | ✅ | §1.9 | Broken‑Reference‑Highlighting statt stummem Löschen. |
| Plane (Referenzebenen) | ✅ | §1.10 | MVP: Offset, Three points, Angle, Mid plane, Point+Normal; später Tangent/Curve‑point. |
| Variable / Expressions | ✅ | §1.11, §3 | **Korrektur:** Feature‑Tree‑Eintrag, nicht separates Panel. Tabs: Assigned/Measured/From‑import × Length/Angle/Number/Any. |
| Mate Connector | ✅ | §1.12 | Realign/Move/Owner entity, nur benannte im Tree. |
| Fillet (via Spatial Mask + Ghost Topology) | ✅ | §1.18 | MVP: Edge‑Tab + Tangent propagation + Radius. Alle anderen Onshape‑Fillet‑Optionen explizit Phase 2. |
| Chamfer (via Spatial Mask + Ghost Topology) | ✅ | §1.19 | Analog Fillet, Distance statt Radius. |
| Thread (analytisches Gewinde‑Primitiv) | ✅ | §1.26 | Dediziertes Feature, kein Sweep entlang Helix. Verkettung mit Hole (Tapped). |
| Gyroid / TPMS‑Infill | ✅ | §1.27 | USP‑Feature, kein Onshape‑Äquivalent. |
| Procedural Noise / Displacement | ✅ | §1.27 | Terminal Feature, Picking auf Grundform. |
| Domain Deformation (Bend / Twist) | ✅ | §1.27 | Lipschitz‑korrigiert. |
| Morph (Lerp zwischen SDFs) | ✅ | §1.27 | |
| Sweep (via Mesh‑to‑SDF) | ✅ | §1.22 | MVP via K13‑Pfad; Twist, Scale, Profile control. |
| Loft | ❌ (M14+) | §1.23 | Bleibt M14+. |
| Curved Pattern | ❌ (M14+) | §1.25 | Bleibt M14+. |

**Nicht in README §6.1 aufgeführt, aber aus Screenshots als Onshape‑MVP‑Kandidaten identifiziert:**

| Feature | Status | Spec → | Empfehlung für README |
|---|---|---|---|
| **Hole** (Simple/Counterbore/Countersink, Drilled/Tapped) | ➕ fehlt | §1.17 | **Aufnehmen** als MVP‑Feature. Makro über bestehende Primitive (Zylinder + Pattern + Boolean), dialog‑seitig aber ein eigenständiges Tool mit Norm‑Tabelle. |
| **Shell** (Hollow + Faces to remove) | ➕ fehlt | §1.20 | **Aufnehmen** als MVP. Hollow‑Modus ist in SDF trivial; „Faces to remove" hängt am Feature‑ID‑Tagging und kann Phase 2 bleiben. |
| **Helix** (als 3D‑Kurve, Input für Sweep) | ➕ fehlt | §1.24 | **Aufnehmen** als MVP — Voraussetzung für Schnecken­förderer, Federn, Spiralfedern (nicht für Gewinde — das löst §1.26 Thread). |
| **Workspace Units** | ➕ fehlt | §1.28 | **Aufnehmen** als MVP — ohne das kippt die UX, weil jeder Dialog sonst manuelle Einheiten‑Angaben braucht. |
| **Kontextmenüs (Face‑Select, Sketch‑Select)** | ➕ fehlt | §1.29 | **Aufnehmen** als erste‑Klasse‑Interaktionsform im MVP, inkl. „Select → Tangent connected edges"‑Helper. |
| **Thicken** | — | §1.21 | **Nicht nötig** im MVP (keine Surface‑Bodies in FieldDraft). |
| **Surface / Thin** Tabs in Extrude/Revolve/Sweep/Loft | — | §1.2 ff. | **Dauerhaft weg** (B‑Rep‑Konzepte, nicht im SDF‑Modell). |
| **Reapply fillet** im Boolean‑Dialog | — | §1.4 | **Nicht nötig** — Ghost Topology verhindert das Problem strukturell. |

**Konkrete README‑Änderungs­vorschläge (§6.1):**

1. **Extrude‑Zeile** präzisieren: „End Conditions: Blind, Symmetric, Through all, Up to face, Up to entity. Second direction mit eigener Condition. Direction‑Override. Starting offset. Draft optional inline."
2. **Linear‑Pattern‑Zeile** präzisieren: „inkl. Direction 2 (2D‑Grid), Centered, Skip instances. Pattern‑Quelle Part/Feature im MVP; Face pattern Phase 2."
3. **Circular‑Pattern‑Zeile** präzisieren: „Angle default 360° + Equal spacing, Centered optional, Skip instances im MVP."
4. **Mirror‑Zeile** präzisieren: „Mirrored Bodies sind separat editierbar — Features nach dem Mirror‑Knoten wirken unabhängig auf Original oder Spiegelbild."
5. **Transform‑Zeile** bestätigen: Sieben Modi (Translate by line/distance/XYZ, Transform by mate connectors, Rotate, Copy in place, Scale). Die frühere Empfehlung, Scale zu streichen, ist zurückgenommen.
6. **Plane‑Zeile** präzisieren: MVP‑Modi Offset/Three points/Angle/Mid plane/Point+Normal.
7. **Boolean‑Zeile** präzisieren: mit Offset‑Option und Keep tools.
8. **Fillet/Chamfer‑Zeilen** präzisieren: MVP = Edge‑Tab + Tangent propagation + Radius/Distance. Alle anderen Onshape‑Optionen (Full round, Asymmetric, Partial, Variable, Allow edge overflow, Smooth corners) explizit Phase 2.
9. **Neue Zeilen einfügen:**
   - **Hole** (Simple/Counterbore/Countersink, metric/inch, Drilled/Tapped) — MVP.
   - **Shell** (Hollow im MVP; Faces to remove Phase 2) — MVP.
   - **Helix** (als 3D‑Kurve, Input für Sweep) — MVP.
   - **Workspace Units** — MVP.
   - **Kontextmenüs** — MVP (in §6.3 UX‑Prinzipien als eigener Punkt).

### 5.2 Sketch‑Editor (README §6.2)

| Feature | README §6.2 | Spec → | Anmerkung |
|---|---|---|---|
| Multiselect | ✅ | §4.1 | |
| Line | ✅ | §2.2 | |
| Rectangle | ✅ | §2.3 | Drei Varianten: Corner/Center/3‑point. |
| Circle | ✅ | §2.4 | Center/3‑point MVP; Tangent Phase 2. |
| Ellipse | ✅ | §2.5 | |
| Polygon | ✅ | §2.6 | Inscribed/Circumscribed‑Toggle. |
| Spline | ✅ | §2.7 | Fit‑Point als Default; Tangent‑Constraint am Endpunkt. |
| Point | ✅ | §2.8 | |
| Project / Intersect | ✅ | §2.15 | **Korrektur Terminologie:** Onshape nennt das Projection‑Tool „Use", nicht „Project". |
| Toggle Construction | ✅ | §2.16 | Gestrichelt. |
| Trim, Split, Offset | ✅ | §2.9–2.11 | |
| Mirror (Sketch) | ✅ | §2.12 | Auto‑Symmetry‑Constraint zum Original. |
| Linear / Circular Pattern (Sketch) | ✅ | §2.13 | |
| Transform (Sketch) | ✅ | §2.14 | |
| Import/Export DXF/SVG/PNG/JPG | ✅ | — | |
| Inline‑Dimensioning beim Zeichnen | ✅ (implizit) | §2.4a | Tippen einer Zahl während des Ziehens → Dimension wird gesetzt. |
| Auto‑Inferencing beim Zeichnen (Shift‑Unterdrückung) | ✅ | §2.18a | |
| Drag‑Solving (Live‑Constraint‑Lösung) | ✅ (implizit via §4.4 Adapter) | §2.1a | |
| Driving vs. Driven Dimensions | ✅ | §2.17a | |
| Constraint Manager Panel | ✅ | §2.21 | **Aktualisierung:** jetzt MVP (frühere Empfehlung „Phase 2" ist überholt). |
| Drei‑Farben‑Status | ✅ | §2.1, §2.20 | Blau/Schwarz/Rot + Braun/Orange Construction. |
| Under‑constrained erlaubt beim Sketch‑Finish | ✅ (implizit) | §2.22 | |
| Dimension (Länge, Winkel, treibend oder getrieben) | ✅ | §2.17 | Ein kontextsensitives Tool. |
| **Constraints:** | | | |
| Coincident | ✅ | §2.18 | |
| Concentric | ✅ | §2.18 | |
| Parallel | ✅ | §2.18 | |
| Tangent | ✅ | §2.18 | |
| Horizontal / Vertical | ✅ | §2.18 | |
| Orthogonal (= Perpendicular) | ✅ | §2.18 | Terminologie‑Hinweis. |
| Equal | ✅ | §2.18 | |
| Midpoint | ✅ | §2.18 | |
| Symmetric | ✅ | §2.18 | |
| **Fix** | ➕ fehlt | §2.18 | **Aufnehmen** (User‑Bestätigung). |
| **Collinear** | ➕ fehlt | §2.18 | **Aufnehmen** (User‑Bestätigung). |
| Constraint Removal | ✅ | §2.19 | + Rechtsklick‑„Show constraints". |

**Konkrete README‑Änderungs­vorschläge (§6.2):**

1. **Constraint‑Block** um **Fix** und **Collinear** erweitern (User hat beide bestätigt).
2. **Terminologie:** Die „Orthogonal"‑Zeile in der README sollte als Klarstellung einen Hinweis tragen: `Orthogonal (in Onshape: Perpendicular)` — beides ist dasselbe, aber User, die von Onshape kommen, suchen den Begriff.
3. **„Project / Intersect"** umbenennen in **„Use / Intersect"** — Onshape‑Terminologie.
4. **Neue Zeile einfügen:** „Inline‑Dimensioning: Zahl während des Zeichnens tippen → Dimension wird gesetzt" (ist der Produktivitäts­hebel von §2.4a).

### 5.3 UX‑Vereinfachungen (README §6.3)

Die README §6.3 listet bereits die vier Onshape‑Parität‑Kernpunkte (Auto‑Inferencing, Driving vs. Driven, Drag‑Stabilität, Drei‑Farben‑Status). Diese Spec konkretisiert sie in §2.1a, §2.17a, §2.18a und §2.20. Keine strukturellen Änderungen nötig.

**Kleine Ergänzungen:**

- **§6.3 neuer Punkt: Kontextmenüs** — „Rechtsklick ist in FieldDraft eine erste‑Klasse‑Interaktionsform, nicht bloß ein Backup‑Weg. Die wichtigsten Feature‑Starts sind aus dem Kontextmenü einer Proxy‑Face oder Sketch‑Selection erreichbar."
- **§6.3 neuer Punkt: Workspace Units** — „Jedes numerische Feld übernimmt die Workspace‑Default‑Einheit. Der User kann pro Feld eine abweichende Einheit mittippen, ohne die Workspace‑Einstellung zu ändern."

### 5.4 Variablen / Formel‑Engine (README §4.5)

Sehr gut abgedeckt. Zwei Ergänzungen:

- **Referenz­syntax** (`#name`) in §4.5 explizit dokumentieren.
- **Measured Variables** als Kopplung zum Measure‑Tool (§1.13 `(x)`‑Button) in §4.5 einweben — ein Measured‑Wert ist kein toter Float, sondern referenziert die gemessenen Proxy‑Elemente und aktualisiert sich bei Modell‑Änderungen.

### 5.5 Architektur‑Kopplungen, die implizit bleiben sollten

Ein paar Punkte sind bewusst **nicht** in die README §6‑Feature‑Tabellen eingearbeitet, weil sie Architektur‑Konsequenzen sind, keine Features:

- **Snapping gegen B‑Rep‑Proxy** (nicht Mesh‑Dreiecke) — steht in §4.1 und §4.2, reicht.
- **Ghost Topology für Picking auf Ghost‑Edges** — steht in §4.1, reicht.
- **Dual‑Path‑Evaluator** — steht in §4.1, reicht.
- **PlaneGCS Adapter‑Schicht** — steht in §4.4, reicht.

Diese UX‑Spec verweist mehrfach auf diese Konzepte, wiederholt sie aber nicht — das ist Absicht, um die Grenze zwischen „was der User sieht" und „wie der Kernel es baut" klar zu halten.

---

## 6. Geklärt und offene Restpunkte

Die meisten Unsicherheiten aus früheren Fassungen sind inzwischen beantwortet — entweder durch die Screenshots in `assets/` oder durch User‑Entscheidungen.

### 6.1 Geklärt

**Aus Screenshots (`assets/3d-*.png`, `assets/onshape-*.png`, `assets/workspace-units.png`):**
- Extrude‑Dialog‑Layout vollständig (Solid/Surface/Thin, NAIO, Starting offset, Second end position, Draft inline).
- Revolve mit Mate‑Connector als Achse und Full‑Revolve‑Toggle.
- Boolean mit Offset, Keep tools, Reapply fillet.
- Linear/Circular/Curve Pattern mit Part/Feature/Face‑Selektor und NAIO‑Tabs.
- Fillet‑Dialog ist weit umfangreicher als gedacht (Full round, Asymmetric, Partial, Variable, Edge overflow, Smooth corners).
- Hole als dediziertes Feature mit Simple/Counterbore/Countersink, metric/inch, Termination wie Extrude.
- Transform mit sieben Modi inkl. **Scale**.
- Helix als eigenständige Kurven‑Erzeugung (Input für Sweep).
- Shell und Thicken als eigenständige Features.
- View‑Menü mit Isometric/Dimetric/Trimetric, Named Views, Section view, Shading‑Modi.
- Measure als **dediziertes Panel** (nicht nur passive Statuszeile — die frühere Annahme war falsch).
- Mate‑Connector mit Realign/Move/Owner‑entity‑Controls.
- Variable als Feature‑Tree‑Eintrag mit Assigned/Measured/From‑import‑Tabs.
- Workspace‑Units als dedizierter Projekt‑Settings‑Dialog.
- Kontextmenüs als erste‑Klasse‑Interaktionsform (Face‑Select, Sketch‑Line‑Select, Sketch‑Nothing‑Select).

**Aus User‑Entscheidungen:**
- Circle‑Variante „Tangent" existiert (Phase 2).
- Over‑constrained wird in Onshape **ignoriert**, nicht verworfen — nur visuell markiert.
- Constraint‑Hover‑Highlight existiert.
- Construction‑Lines sind gestrichelt.
- Variable‑„Panel" ist Teil des Feature‑Trees, kein separates Panel.
- Shortcuts werden als konfigurierbares JSON‑Keybinding umgesetzt.
- Fix und Collinear kommen in die Constraint‑Liste.
- Rectangle‑Varianten: alle drei (Corner/Center/3‑point).
- Extrude MVP enthält auch „Up to face" (nicht nur Blind/Symmetric/Through all).
- Mirrored Parts sind separat editierbar — Features nach dem Mirror‑Knoten wirken unabhängig.
- Section View existiert mit Gizmo zum Live‑Verschieben.
- Mouse‑Defaults: links = Select, mitte = Pan, rechts = Orbit um Clip‑BBox‑Mittelpunkt.

### 6.2 Restliche offene Fragen

**Noch nicht eindeutig geklärt:**

1. **Plane‑Modi vollständige Liste** — der Screenshot zeigt keinen Plane‑Dialog; ich habe die gängigsten Modi aus dem Gedächtnis aufgeführt (Offset, Angle, Three points, Point+Normal, Mid plane, Tangent, Curve point, Line + angle). Wenn du Plane‑Modi willst, die darüber hinausgehen, wäre ein Screenshot hilfreich.
2. **Hole‑Norm‑Tabellen** — welche Tap‑Standards werden gewünscht (M‑Serie default, ggf. UNC/UNF für Inch‑Tab, ggf. Trapez, ggf. BSP/BSPT für Fluid‑Gewinde)? Im MVP reicht M3–M24 metric.
3. **Sweep `Profile control`‑Modi** im Detail — `Keep profile orientation` vs. `Twist with path` vs. `Align with guide rail`: welche davon im MVP?
4. **Helix `Circle`‑Modus** — ein Kreis im Sketch definiert Position und Durchmesser. Lohnt sich das im MVP, oder reichen Cylinder/Cone und Axis?
5. **Inline‑Variable‑Preview im Tree** — Onshape zeigt `#Name = Wert Einheit`. Soll FieldDraft auch den Typ‑Tab (Length/Angle/…) im Tree als Icon zeigen?
6. **Named Views‑Persistenz** — gehören Named Views in das Projekt‑Dokument (dann landen sie auch beim Export mit) oder in die User‑Settings (dann sind sie projekt­übergreifend)?
7. **Context‑Menu‑Umfang** — welche Einträge sollen im MVP wirklich drin sein? Der Onshape‑Screenshot zeigt 20+ Einträge; realistisch sind die in §1.29 gelisteten zehn bis zwölf.

Keine dieser Fragen blockiert den Start der Implementierung — sie lassen sich pro Feature‑Slice klären, wenn der Kernel dort ankommt.

