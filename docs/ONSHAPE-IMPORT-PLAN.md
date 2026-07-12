# Onshape-Import — Plan (SSOT)

Ziel: eigene Onshape-Projekte parametrisch nach FieldDraft holen — den Feature-Baum
und die Sketch-Features nachstellen, nicht nur ein totes Mesh importieren.

## Zugang

Onshape ist rein cloud-basiert, hat aber eine vollständig dokumentierte öffentliche
REST-API; der Web-Client ruft dieselben Endpunkte auf. Ein Netzwerk-Sniff wäre also
nur die Wieder-Entdeckung der dokumentierten API — unnötig und fragil.

Auth: API-Key (Access + Secret, HMAC-signiert) für die EIGENEN Dokumente. Kein
Session-Token-Sniff. Der Key lebt gitignored, nie im Repo, nie im Chat.

DWE-URL-Schema: `/api/.../d/{document}/w/{workspace}/e/{element}` (w = workspace,
v = version, m = microversion; jeweils 24-stellige IDs).

Relevante Endpunkte:
- `partstudios/.../features` — der Feature-Baum inkl. Sketches (BTM-Format).
- `partstudios/.../tessellatedfaces` / `tessellatededges` — Meshes/Kanten (Fallback).
- `partstudios/.../bodydetails` — B-Rep-Topologie.
- Translation-API — STEP/Parasolid-Export (asynchron), harter Fallback.
- FeatureScript-Eval — Snippet serverseitig ausführen; löst opake Query-IDs zu
  echter Geometrie auf. Der Schlüssel zum Referenz-Mapping (siehe unten).
- Glassworks — interaktiver API-Explorer zum Ausprobieren ohne Code.

## Datenmodell-Abbildung

### Sketch-Geometrie — bildet sich fast 1:1 ab

Onshape-Koordinaten sind in **Metern**, in der lokalen 2D-Ebene des Sketches →
×1000 für FieldDrafts (u,v)-Millimeter.

| Onshape | FieldDraft |
| --- | --- |
| `BTMSketchCurveSegment` + `BTCurveGeometryLine` (pnt + dir, startParam/endParam) | `sketch_line { start, end }` |
| `BTMSketchCurve` + `BTCurveGeometryCircle` (voller Kreis) | `sketch_circle { center, radius }` |
| `BTMSketchCurveSegment` + `BTCurveGeometryCircle` (Bogen) | `sketch_arc { center, radius, start_angle, sweep_angle }` |
| `BTMSketchPoint` (freistehend) | `sketch_point { at }` |
| `isConstruction: true` | `params.construction: true` |

Linien-Endpunkte werden NICHT als eigene `sketch_point`s emittiert — FieldDraft-Linien
tragen ihre Endpunkte selbst; geteilte Punkte werden über Coincident-Constraints
verbunden (genau wie in Onshape).

### Sketch-Constraints — bildet sich sauber ab

COINCIDENT / HORIZONTAL / VERTICAL / PARALLEL / PERPENDICULAR / TANGENT / CONCENTRIC /
EQUAL haben direkte `sketch_constraint_*`-Entsprechungen (keine Bemaßungswerte).
DISTANCE / ANGLE / RADIUS / DIAMETER sind Bemaßungen (Wert aus `BTMParameterQuantity`)
— Folgeschritt.

Der Knackpunkt: Onshape referenziert Entities per **String-`entityId`** ("Line1",
"Line1.start"), FieldDraft per **Integer-Entity-ID** aus `buildDraftSketchState`
(Origin=0, dann pro Sub-Call allokiert: line=start,end,line; circle=center,circle;
arc=center,start,end,arc; point=point). Der Übersetzer baut daher eine
`entityId → FieldDraft-id`-Karte anhand der bekannten Allokationsreihenfolge und
löst Constraints darüber auf.

### Feature-Parameter — unkritisch

`BTMParameterQuantity` ("25 mm"), `BTMParameterEnum`, `BTMParameterBoolean` →
Tiefen/Winkel/Op-Modi von `extrude`/`revolve`.

## Der harte Kern: Geometrie-REFERENZEN (Queries)

Feature-Parameter zeigen über `BTMParameterQueryList` → `BTMIndividualQuery` auf
Geometrie, per `queryString` (FeatureScript) oder opaken `deterministicIds`.
Diese IDs ergeben nur in Onshapes Kernel Sinn; FieldDraft hat sein eigenes
persistentes Naming (`qFace(idHash@fingerprint)`, `qCreatedBy(featureId)`, `qSketchLine`).

Brücke: FeatureScript-Eval liefert zu jeder ID die echte Geometrie (Position,
Normale, welche Sketch-Region) → daraus wird eine FieldDraft-Query per Position/
Fingerprint. Ohne diese Auflösungsschicht kein parametrischer Feature-Import.

## Staffelung

1. **Sketch-Geometrie** (dieser Spike) — Punkte/Linien/Kreise/Bögen → Sub-Calls,
   Meter→mm, Entity-ID-Karte. Beweist die 1:1-Abbildung und die Auth-Kette.
2. **Sketch-Constraints** — positionelle zuerst, dann Bemaßungen.
3. **Extrude/Revolve** — erstmals die Query-Auflösung (Sketch-Region → FieldDraft).
4. **Hybrid-Fallback** — was (noch) nicht parametrisch mappt, kommt per STEP/Mesh
   als gebackener Body rein. Import ist nie „geht nicht".
5. **Lange Reihe** — Fillet, Chamfer, Shell, Pattern, Hole, Sweep, Loft, Variablen,
   Mate-Connectors, Configurations. Manches ohne FieldDraft-Entsprechung.

## Dateien

- `web/src/onshape/onshapeSketch.ts` — Typen + `translateOnshapeSketch` (rein, testbar).
- `web/src/onshape/onshapeSketch.test.ts` — Vitest gegen realistische Fixtures.
- `web/scripts/onshapeFetch.mjs` — HMAC-signierter Fetch (Node), braucht nur den Key.
- `web/.onshape.local.json` — gitignored Key-Config (Vorlage:
  `web/.onshape.local.example.json`).
