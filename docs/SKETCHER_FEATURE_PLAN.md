# Sketcher-Feature-Plan

Stand: 2026-06-06. Vergleich des FieldDraft-Sketchers gegen Onshape und FreeCAD,
mit exakter Beschreibung jedes Features, seinem aktuellen Umsetzungsstand und einem
Umsetzungsplan. Dieses Dokument ist die Single Source of Truth für den Ausbau des
2D-Sketchers bis zur Vollständigkeit gegenüber den beiden Referenz-CAD-Sketchern.

Eine Erweiterung gilt erst dann als **vollständig**, wenn sie

1. im Kernel (falls nötig) implementiert ist,
2. in der UI vollständig bedienbar ist (Toolbar/Palette/Dialog, kein Konsolen-Only),
3. mit Unit-Tests **inklusive Grenzfällen** abgedeckt ist, und
4. einen grünen Playwright-E2E auf echter WebGPU besitzt, der den interaktiven Pfad
   beweist.

Statuslegende:

- **✅ Vollständig** — Kernel (falls nötig) + UI + Unit-Tests + E2E vorhanden.
- **🟡 Unvollständig** — existiert teilweise (z. B. Kernel-Stub ohne UI, oder UI ohne
  Tests/Grenzfälle). Genaue Lücke ist jeweils benannt.
- **🔴 Fehlend** — gar nicht vorhanden.

---

## 1. Bereits umgesetzte Features (✅ Vollständig)

Diese Features sind in Kernel + UI implementiert und getestet. Sie sind die Baseline
und werden nicht erneut angefasst, außer ein neues Feature baut darauf auf.

| Feature | Umsetzung |
|---|---|
| Punkt | `sketch_point`, UI-Tool, voll |
| Linie | `sketch_line`, UI-Tool, voll |
| Rechteck (Corner) | `sketch_rectangle` (corner_a/corner_b → 4 Punkte + 4 Linien + 4 H/V-Constraints) |
| Kreis (Center+Radius) | `sketch_circle`, UI-Tool, voll |
| Polygon (freie Vertices) | `sketch_polygon` (flaches Vertex-Array), UI-Tool, voll |
| Bogen (3-Klick: center→start→end) | `sketch_arc`, UI-Tool, voll |
| Trim (inkl. Composites) | `sketchTrim.ts`, UI-Modus, Unit + E2E; Rechteck/Polygon-Kanten trimmbar |
| Drag-to-Edit (Grips) | `sketchGrips.ts`: Linienenden, Kreis-/Bogen-Center+Radius+Sweep, Rechteck-Ecken, Polygon-Vertices; re-solved bei Drop |
| Coincident-Constraint | `sketch_constraint_coincident` |
| Horizontal / Vertikal | `sketch_constraint_horizontal` / `_vertical` |
| Parallel / Perpendicular | `sketch_constraint_parallel` / `_perpendicular` |
| Distance-Maß (driven/reference) | `sketch_constraint_distance`, editierbarer Wert |
| Angle-Maß (driven/reference) | `sketch_constraint_angle` |
| Equal | `sketch_constraint_equal` (Längen/Radien) |
| Tangent | `sketch_constraint_tangent` |
| Midpoint | `sketch_constraint_midpoint` |
| Symmetric | `sketch_constraint_symmetric` (2 Punkte + Achse) |
| Radius / Diameter-Maß | `sketch_constraint_radius` / `_diameter`, driven/reference |
| Construction-Geometrie | `construction: true`-Flag, gedämpft gestrichelt, aus Extrude-Profil ausgeschlossen |
| Snapping | Endpoint, Midpoint, Origin (10 mm), Grid (Ctrl) |
| Inferenz / Auto-Constraints | Echtzeit-Vorschläge (H/V/parallel/tangent/perp/concentric, gerankt) |
| DoF-Farbstatus | blau (unter-) / grün (voll-) / rot (überbestimmt) pro Entity |
| Grid + Origin-Achsen | adaptiv (1/2/5/10 mm), Snap-to-Grid |
| Undo/Redo | Session-Sub-Call-Liste + Kernel-Snapshots |
| Touch (Pinch-Zoom) | Zwei-Finger-Zoom an Pinch-Mitte |
| Sketch-Ebene (Face-Lock + Plane-Query) | Face-Pick-Modus, `qCreatedBy(origin_plane_xy)` default, 3D-Frame |
| Live-Maßlabels beim Drag | Distanz-/Winkellabels während Ziehen |

---

## 2. Unvollständig umgesetzte Features (🟡)

Existieren teilweise; die jeweils exakt benannte Lücke schließt die Erweiterung.

### 2.1 Ellipse
**Stand:** `sketch_ellipse` ist im Kernel registriert (`crates/kernel/src/sketch_features.rs`),
legt aber **nur den Center-Punkt** an — keine Bogensegment-Generierung. Das Tool ist
**nicht in der UI-Allowlist** (`SKETCH_TOOL_KEYS` in `web/src/Toolbar.tsx`) und damit
unerreichbar. Kein Solver-Support für Ellipsen-Constraints.
**Lücke:** Vollständige Ellipsen-Geometrie (Polylinien-Approximation oder echte
Ellipsenkurve) + UI-Tool (Center → semi_major → semi_minor → rotation, per Drag) +
`buildDraftSketchState`-Expansion + Tests + E2E.

### 2.2 Mirror (Sketch)
**Stand:** `sketch_mirror` existiert als Build-Time-Stub (Geometrie wird erst beim
`Sketch2d`-Build dupliziert+gespiegelt). **Nicht interaktiv**, keine Achsenauswahl in
der UI über den festen `axis: 0|1` hinaus, kein Solver-Feedback, gespiegelte Geometrie
ist nicht editierbar.
**Lücke:** Interaktiver Mirror — Auswahl von Quell-Entities + Spiegelachse (beliebige
Linie/Konstruktionslinie, nicht nur X/Y), Erzeugung gespiegelter, einzeln editierbarer
Sub-Calls (analog zur Trim-Dekomposition), UI-Tool, Tests, E2E.

### 2.3 Offset (Sketch)
**Stand:** `sketch_offset` existiert als Build-Time-Stub (SDF-Wrapping beim Build).
**Nicht interaktiv**, keine UI über den festen `distance` hinaus, kein editierbares
Ergebnis.
**Lücke:** Interaktiver Offset — Auswahl einer Kette/Kurve + Distanz (Drag oder
Numerik, innen/außen), Erzeugung versetzter editierbarer Sub-Calls, UI-Tool, Tests, E2E.

### 2.4 Concentric-Constraint
**Stand:** Im Kernel als Constraint-Typ vorhanden, aber **nicht in der
Constraint-Palette** verdrahtet (`web/src/sketchConstraints.ts`).
**Lücke:** Palettenregel „2 Kreise/Bögen → Concentric", Apply-Pfad,
`buildDraftSketchState`-Zweig, Unit-Test, E2E.

### 2.5 Polygon — reguläres n-Eck
**Stand:** `sketch_polygon` nimmt nur freie Vertices. Es gibt **kein** Tool für ein
regelmäßiges n-Eck (inscribed/circumscribed um einen Center+Radius), wie in beiden
Referenzen Standard.
**Lücke:** UI-Tool „Regular Polygon" (Center → Radius, Seitenzahl-Eingabe, inscribed/
circumscribed), das die N Vertices berechnet und als `sketch_polygon` emittiert; Tests, E2E.

### 2.6 DoF-Anzeige als Zähler
**Stand:** nur Farbcodierung pro Entity; **kein numerischer „N DoF verbleibend"-Zähler**
wie in Onshape/FreeCAD.
**Lücke:** DoF-Zähler-Anzeige im Sketch-HUD (verbleibende Freiheitsgrade, „fully
constrained"-Status), aus dem Solver-Diagnosepfad gespeist; Test + E2E-Assertion.

### 2.7 Split (explizit)
**Stand:** Kurven lassen sich nur **implizit über Trim** teilen. Kein dediziertes
Split-Werkzeug, das eine Kurve an einem Klickpunkt in zwei Teile zerlegt, ohne etwas zu
entfernen.
**Lücke:** Split-Tool (Klick auf Kurve → zwei coincidente Teilkurven am Klickpunkt),
auf der Trim-Infrastruktur aufbauend; Tests, E2E.

### 2.8 External/Projected Geometry (Use/Project)
**Stand:** Projizierte 3D-Kanten werden nur als **gestrichelte Guides angezeigt**, sind
aber **nicht in den Sketch übernehmbar/referenzierbar** (kein „Use"/„Project"-Tool).
**Lücke:** Tool, das eine projizierte Kante als (Konstruktions-)Sketch-Geometrie
übernimmt, die von Constraints referenziert werden kann; Tests, E2E.

---

## 3. Fehlende Features (🔴)

Gar nicht vorhanden. Exakte Beschreibung jeweils inklusive UI-Erwartung.

### 3.1 Spline / B-Spline
Interpolierende und Kontrollpunkt-B-Splines. Größte funktionale Lücke; ohne Splines sind
organische Profile nicht zeichenbar. **Umfang:** Kernel-Kurventyp (B-Spline, Grad 3,
Knotenvektor) inkl. Tessellation für `Sketch2d`-Build; UI-Tool (Klickfolge zum Setzen
der Kontroll-/Interpolationspunkte, Doppelklick beendet); Drag-Grips für jeden
Kontrollpunkt; Solver-Anbindung mindestens für Endpunkt-Coincidence; Tests inkl.
Grenzfälle (2 Punkte = Linie, geschlossener Spline, kollineare Punkte); E2E.

### 3.2 Slot (Langloch)
Center-to-center-Slot und 3-Punkt-Slot: zwei Halbkreise + zwei tangentiale Linien als
geschlossene Region. **Umfang:** Composite (analog Rechteck) das in 2 Bögen + 2 Linien +
Tangenten-/Equal-Constraints expandiert; UI-Tool (Drag center→center, dann Breite); Tests
inkl. Grenzfällen (Nulllänge, Breite 0); E2E.

### 3.3 Sketch-Fillet
Verrundung der Ecke zwischen zwei sich treffenden Sketch-Kurven durch einen tangentialen
Bogen, die Originalkurven werden auf die Tangentenpunkte gekürzt. **Umfang:** reine
TS-Geometrie auf der Trim-Infrastruktur (zwei Kurven + Radius → Bogen + zwei gekürzte
Kurven + 2 Tangenten-Constraints); UI-Modus (zwei Kurven anklicken oder gemeinsame Ecke,
Radius-Eingabe); Tests inkl. Grenzfälle (paralleler/antiparalleler Fall, Radius zu groß,
Kurven schneiden sich nicht); E2E.

### 3.4 Sketch-Chamfer
Wie Fillet, aber gerade Fase statt Bogen (zwei Kurven gekürzt + Verbindungslinie).
**Umfang:** analog 3.3 mit Linie statt Bogen, Distanz-Eingabe; Tests inkl. Grenzfälle; E2E.

### 3.5 Extend
Verlängert eine offene Kurve bis zur nächsten Schnittkante. **Umfang:** Gegenstück zu
Trim auf derselben Intersection-Infrastruktur; UI-Modus (Klick auf das zu verlängernde
Ende); Tests inkl. Grenzfälle (kein Schnittpunkt vorhanden, mehrere Kandidaten); E2E.

### 3.6 Sketch-Pattern (linear / circular)
Lineare und kreisförmige Wiederholung ausgewählter Sketch-Entities innerhalb des
Sketches. **Umfang:** Auswahl + Richtung/Anzahl/Abstand (linear) bzw. Center/Anzahl/Winkel
(circular); Erzeugung kopierter, einzeln editierbarer Sub-Calls; UI-Dialog; Tests inkl.
Grenzfälle (Anzahl 1, 0°/360°, negative Distanz); E2E.

### 3.7 Copy / Move / Transform (Sketch)
Verschieben, Rotieren, Kopieren ausgewählter Sketch-Entities. **Umfang:** Auswahl +
Transform (Translation/Rotation, optional Kopie); UI-Modus; Tests inkl. Grenzfälle
(leere Auswahl, Identitätstransform); E2E.

### 3.8 Box-Select / Marquee
Aufziehen eines Rechtecks zur Mehrfachauswahl (links→rechts = enclosing, rechts→links =
crossing, wie in beiden Referenzen). **Umfang:** Marquee-Interaktion im Canvas,
Hit-Test gegen Entity-Bounding/Schnitt, Integration ins Selektionsmodell; Tests inkl.
Grenzfälle (leere Box, Box außerhalb); E2E.

### 3.9 Fix/Lock-Constraint
Fixiert einen Punkt/eine Entity an ihrer aktuellen Position (entfernt Freiheitsgrade
ohne Maß). **Umfang:** Constraint-Typ im Solver + Kernel-Registrierung,
Palettenregel, Apply-Pfad, `buildDraftSketchState`-Zweig; Tests; E2E.

### 3.10 Point-on-object-Constraint
Zwingt einen Punkt auf eine Kurve (ohne Coincidence mit einem Endpunkt). **Umfang:**
Solver- + Kernel-Constraint, Palettenregel „1 Punkt + 1 Kurve → Point-on-object",
Apply-Pfad; Tests; E2E.

### 3.11 Live-Numerikeingabe beim Zeichnen
Während des Zeichnens Länge/Radius/Winkel direkt eintippen (Tab zwischen Feldern), wie in
Onshape. Erzeugt sofort das passende getriebene Maß. **Umfang:** Eingabe-HUD am Cursor
während aktivem Drawing-Tool, Tastatur-Commit, automatische Maß-Sub-Calls; Tests inkl.
Grenzfälle (ungültige Eingabe, Esc bricht ab); E2E.

### 3.12 Konflikt-/Redundanz-Diagnose
Über die rote Farbe hinaus: **welche** Constraints konfliktieren/redundant sind benennen
(wie FreeCAD), mit Option zum Entfernen. **Umfang:** Auswertung des Solver-Diagnosepfads,
HUD-Liste der konfliktierenden Constraint-IDs, „entfernen"-Aktion; Tests; E2E.

### 3.13 Constraint-/Element-Listen-Panel
Panel, das alle Sketch-Entities und Constraints auflistet, anwählbar/löschbar macht (wie
FreeCAD „Elements"/„Constraints"). **Umfang:** Panel-Komponente (eigene CSS-Datei unter
`web/src/styles/components/`), Sync mit Selektionsmodell, Löschen einzelner Constraints;
Tests; E2E.

### 3.14 Primitiv-Varianten
Rechteck (Center, 3-Punkt), Kreis (3-Punkt), wie in beiden Referenzen. **Umfang:** je
Variante ein UI-Tool/Modus, der dieselben Kernel-Sub-Calls emittiert; Tests inkl.
Grenzfälle; E2E. (Regular Polygon ist separat unter 2.5.)

### 3.15 Text
Text als Sketch-Geometrie (ShapeString-Äquivalent), zu Konturen vektorisiert. **Umfang:**
Font→Glyph-Konturen→Sub-Calls (oder Polylinien), UI-Dialog (Text, Größe, Font);
Tests; E2E. *(Hoher Aufwand, niedrige Priorität.)*

### 3.16 SVG/DXF-Import als Sketch *(neu)*
Import von Standard-2D-Vektorformaten (**SVG** und **DXF**) als Sketch-Geometrie.
**Umfang:**
- Parser für SVG-Pfade (line/polyline/polygon/rect/circle/ellipse/path mit L/H/V/C/S/Q/T/A/Z)
  und DXF-Entities (LINE, LWPOLYLINE, POLYLINE, CIRCLE, ARC, ELLIPSE, SPLINE).
- Mapping auf vorhandene Sub-Calls (`sketch_line`, `sketch_arc`, `sketch_circle`,
  ggf. `sketch_polygon`/Spline), Einheiten- und Y-Achsen-Konvertierung (SVG y-down →
  Sketch y-up), Skalierung in mm.
- UI: Import-Button/Dropzone im Sketch-Modus, Datei-Auswahl, Vorschau/Platzierung,
  optionaler Construction-Toggle für Importe.
- Tests inkl. Grenzfälle (leere Datei, unbekannte Entities → übersprungen mit Hinweis,
  Bézier→Bogen/Polyline-Approximation, große Koordinaten, verschachtelte Transforms in SVG).
- E2E: Import einer Beispiel-SVG und einer Beispiel-DXF, Assertion der erzeugten Sub-Calls.

---

## 4. Umsetzungsplan

### 4.1 Vorgehen

Jede Erweiterung wird von einem **frischen Subagenten** umgesetzt. Der Hauptagent
(dieser Loop) übernimmt Reihenfolge, Abnahme und Nacharbeit:

1. Hauptagent gibt dem Subagenten den exakten Feature-Auftrag inkl. aller stehenden
   Konventionen (siehe 4.3) und der relevanten Dateien.
2. Subagent implementiert Kernel (falls nötig) + UI + Unit-Tests (mit Grenzfällen) +
   Playwright-E2E, baut WASM bei Versionsänderung neu, lässt die Suite grün.
3. Hauptagent **nimmt ab**: prüft Code-Qualität, UI-Vollständigkeit, Testabdeckung
   (insb. Grenzfälle), führt die Tests selbst aus. Bei Mängeln → Nacharbeit durch
   denselben oder einen neuen Subagenten.
4. Erst nach grüner Abnahme: Versions-Bump (`web/package.json`), WASM-Rebuild synchron,
   Commit **und** Push (stehende Konvention), Statusänderung in diesem Dokument auf ✅.

Die Umsetzung erfolgt **sequenziell**, nicht parallel: Fast alle Features fassen dieselben
zentralen Dateien an (`web/src/Toolbar.tsx`, `web/src/sketchSession.ts`, die Kernel-Tool-
Registry, `web/src/sketchConstraints.ts`). Parallele Subagenten würden hier kollidieren.
Nach jedem Feature wird committed, damit der nächste Subagent auf sauberem Stand aufsetzt.

### 4.2 Reihenfolge (geringster Aufwand / höchster Nutzen zuerst)

Begründung der Reihenfolge: erst die billigen Wins und UX-Grundlagen, dann die mittleren
Editier-Tools auf vorhandener Infrastruktur, zuletzt die großen Brocken (Spline, Text).

| # | Feature | Abschnitt | Aufwand | Abhängigkeit |
|---|---|---|---|---|
| 1 | Concentric-Constraint in UI | 2.4 | XS | — |
| 2 | Fix/Lock-Constraint | 3.9 | S | — |
| 3 | Point-on-object-Constraint | 3.10 | S | — |
| 4 | Box-Select / Marquee | 3.8 | M | — |
| 5 | DoF-Zähler | 2.6 | S | — |
| 6 | Reguläres n-Eck | 2.5 | S | — |
| 7 | Primitiv-Varianten (Rect/Circle) | 3.14 | M | — |
| 8 | Ellipse fertigstellen | 2.1 | M | — |
| 9 | Slot (Langloch) | 3.2 | M | Bogen (vorhanden) |
| 10 | Sketch-Fillet | 3.3 | M | Trim-Infra |
| 11 | Sketch-Chamfer | 3.4 | S | #10 |
| 12 | Extend | 3.5 | M | Trim-Infra |
| 13 | Split (explizit) | 2.7 | S | Trim-Infra |
| 14 | Mirror interaktiv | 2.2 | M | — |
| 15 | Offset interaktiv | 2.3 | M | — |
| 16 | Sketch-Pattern (linear/circular) | 3.6 | M | — |
| 17 | Copy/Move/Transform | 3.7 | M | — |
| 18 | Live-Numerikeingabe | 3.11 | L | — |
| 19 | Constraint-/Element-Panel | 3.13 | M | — |
| 20 | Konflikt-/Redundanz-Diagnose | 3.12 | M | #19 |
| 21 | External/Projected Geometry | 2.8 | L | — |
| 22 | SVG/DXF-Import | 3.16 | L | — |
| 23 | Spline / B-Spline | 3.1 | XL | — |
| 24 | Text | 3.15 | XL | #23 (Konturen) |

### 4.3 Stehende Konventionen für jeden Subagenten

- **Keine Backward Compatibility** — Breaking Changes erwünscht, keine Compat-Shims.
- **Versions-Bump + WASM-Rebuild synchron** bei jeder Änderung (siehe unten).
- **CSS** lebt in `web/src/styles/` (Light Mode, jede Schriftgröße ≥ 1rem, `fd-`-Präfix
  Top-Level-Klasse + natives Nesting, eine CSS-Datei pro Komponente, keine Inline-Styles
  außer dynamische Werte; fehlendes Token zuerst in `tokens.css`).
- **Tests** brauchen node 22 (`source ~/.nvm/nvm.sh && nvm use 22`).
- **E2E real-GPU** auf HTTPS-Port (siehe Memory `reference_playwright_realgpu`),
  Temp-Config nach Lauf löschen.
- **WASM-Rebuild:** `cd crates/kernel && FIELDDRAFT_VERSION="$(node -p "require('../../web/package.json').version")" wasm-pack build --target web --out-dir ../../web/src/wasm --out-name fielddraft_kernel`, dann `git add -f web/src/wasm/fielddraft_kernel_bg.wasm`.
- **Entity-ID-Allokationsreihenfolge** in `buildDraftSketchState` muss die Kernel-
  Allokation byteweise spiegeln (sonst korrumpieren Constraint-Referenzen).
- **docs/ARCHITECTURE.md** bei strukturellen Änderungen mitpflegen.
- `assets/screenshots/sprengfass-*.png` **nie** committen.

### 4.4 Definition of Done je Feature

- [ ] Kernel-Op implementiert (falls nötig) + Rust-Tests grün (`cargo test --workspace`)
- [ ] UI vollständig bedienbar (Tool/Palette/Dialog), keine Konsolen-Only-Funktion
- [ ] Unit-Tests inkl. **Grenzfällen** grün (`cd web && npm test`)
- [ ] Playwright-E2E auf echter WebGPU grün
- [ ] Versions-Bump + WASM-Rebuild synchron
- [ ] Vom Hauptagenten abgenommen
- [ ] Commit **und** Push
- [ ] Status in diesem Dokument auf ✅ gesetzt

---

## 5. Fortschritt

| # | Feature | Status | Version | Commit |
|---|---|---|---|---|
| 1 | Concentric-Constraint in UI | ✅ | 0.5.42 | 1049f68 |
| 2 | Fix/Lock-Constraint | ✅ | 0.5.43 | 35fb4f8 |
| 3 | Point-on-object-Constraint | ✅ | 0.5.44 | b2bd7b7 |
| 4 | Box-Select / Marquee | ✅ | 0.5.45 | c8f6057 |
| 5 | DoF-Zähler | ✅ | 0.5.46 | 749236b |
| 6 | Reguläres n-Eck | ✅ | 0.5.47 | ecc1a1a |
| 7 | Primitiv-Varianten (Rect/Circle) | 🔴 offen | — | — |
| 8 | Ellipse fertigstellen | 🔴 offen | — | — |
| 9 | Slot (Langloch) | 🔴 offen | — | — |
| 10 | Sketch-Fillet | 🔴 offen | — | — |
| 11 | Sketch-Chamfer | 🔴 offen | — | — |
| 12 | Extend | 🔴 offen | — | — |
| 13 | Split (explizit) | 🔴 offen | — | — |
| 14 | Mirror interaktiv | 🔴 offen | — | — |
| 15 | Offset interaktiv | 🔴 offen | — | — |
| 16 | Sketch-Pattern (linear/circular) | 🔴 offen | — | — |
| 17 | Copy/Move/Transform | 🔴 offen | — | — |
| 18 | Live-Numerikeingabe | 🔴 offen | — | — |
| 19 | Constraint-/Element-Panel | 🔴 offen | — | — |
| 20 | Konflikt-/Redundanz-Diagnose | 🔴 offen | — | — |
| 21 | External/Projected Geometry | 🔴 offen | — | — |
| 22 | SVG/DXF-Import | 🔴 offen | — | — |
| 23 | Spline / B-Spline | 🔴 offen | — | — |
| 24 | Text | 🔴 offen | — | — |
