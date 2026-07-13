/**
 * The API description the MODEL reads (ticket #347).
 *
 * This string is the most important one in the whole integration: the assistant
 * can only use what it reads here. It is deliberately hand-written rather than
 * generated — signatures alone teach a model nothing about WHEN to call what,
 * and the worked examples below move its behaviour more than any rule does.
 *
 * `@values` placeholders are filled with the document's real values before each
 * turn (expandRuntimeTypes in setup.ts), so the model sees the feature types and
 * variable names that actually exist right now instead of a guessed list.
 */

export const API_DESCRIPTION = `
// ── FieldDraft ────────────────────────────────────────────────────────────────
// Parametrischer CAD-Editor. Einheiten: MILLIMETER, Winkel in GRAD.
// Ein Dokument ist ein FEATURE-BAUM: jedes Feature erzeugt oder verändert
// Geometrie, in der Reihenfolge des Baums.

interface FeatureInfo { id: number; type: string; name: string; suppressed: boolean }

interface DocumentSummary {
  featureCount: number;
  bodyCount: number;
  features: FeatureInfo[];
  selection: number[];      // Feature-Ids
  activeSketch: number | null;
  variables: { name: string; value: number }[];
  sceneSize: [number, number, number];   // Bounding-Box in mm
}

// ── Lesen ─────────────────────────────────────────────────────────────────────

/** Das ganze Dokument auf einen Blick. Billig — im Zweifel aufrufen. */
getDocument(): DocumentSummary

/** Alle Feature-Typen, die DIESES Dokument kennen — inklusive der Skript-Features
 *  des Nutzers, die es nur zur Laufzeit gibt. Nie einen festen Satz annehmen.
 *  @values featureTypes */
listFeatureTypes(): string[]

/** Das Parameter-Schema eines Typs (JSON-Schema-artig). IMMER lesen, bevor du
 *  addFeature aufrufst — die Feldnamen sind nicht ratbar. */
schemaForType(type: string): unknown

/** Die aktuellen Parameter eines Features — eine KOPIE. Sie zu verändern bewirkt
 *  nichts; schreib sie mit updateFeature zurück. null, wenn es die Id nicht gibt. */
getFeature(id: number): Record<string, unknown> | null

// ── Schreiben ─────────────────────────────────────────────────────────────────

/** Legt ein Feature an und gibt seine Id zurück. Der generische Weg — Primitive,
 *  Booleans, Muster, Transformationen und Skript-Features laufen alle hierüber.
 *  Wirft bei unbekanntem Typ oder falschen Parametern. */
addFeature(type: string, params: Record<string, unknown>): number

/** Ersetzt die Parameter eines Features (VOLLE Wire-Form: getFeature lesen,
 *  ändern, ganzes Objekt zurückschreiben). */
updateFeature(id: number, params: Record<string, unknown>): boolean

renameFeature(id: number, name: string): boolean
setFeatureSuppressed(id: number, suppressed: boolean): void

/** ZERSTÖREND. Nur nach ausdrücklicher Bestätigung des Nutzers. */
removeFeature(id: number): void

/** Dokument-Variable setzen (in jedem Zahlenfeld als Formel nutzbar).
 *  @values variableNames */
setVariable(name: string, value: number): boolean

undo(): void
redo(): void

/** Schlägt ein FieldSkript-Feature vor (TypeScript). Es wird NICHT ausgeführt:
 *  die Quelle wird geprüft und dem NUTZER vorgelegt, der entscheidet.
 *  Rückgabe {ok:false, reason, line} → korrigieren und neu vorschlagen.
 *  Rückgabe {ok:true} → der Nutzer sieht das Skript; behaupte NICHT, es sei
 *  gelaufen. Erlaubter Import: nur "fielddraft". */
proposeScript(name: string, source: string, intent: string): { ok: boolean; status: string; reason?: string; line?: number }

// ── Auswahl & Ansicht ─────────────────────────────────────────────────────────

/** Ein Feature auswählen (null = Auswahl leeren). */
select(id: number | null): void

/** Kamera auf eine Standardansicht: "front" | "back" | "left" | "right" |
 *  "top" | "bottom" | "iso" */
setView(preset: string): void

/** Zoomt so, dass das ganze Modell ins Bild passt. */
fitView(): void

// ── Oberfläche lesen und bedienen ─────────────────────────────────────────────
// Sieht NUR DOM-Bedienelemente (Buttons, Felder, Listen) — NICHT den 3D-Viewport.
// Szene, Auswahl und Maße kommen aus getDocument().

readUIState(): UINode           // sichtbare Bedienelemente mit data-* Attributen
click(testId: string): void     // Button/Element per data-testid klicken
setValue(testId: string, value: string): void

// ── Beispiele ─────────────────────────────────────────────────────────────────

// „Bau mir eine Grundplatte 80 × 60 × 5 mit einem 10er-Loch in der Mitte."
const types = listFeatureTypes();                 // welche Typen gibt es?
const boxSchema = schemaForType("box");           // wie heißen die Felder?
const plate = addFeature("box", { size: [80, 60, 5] });
const holeSchema = schemaForType("hole");
addFeature("hole", { diameter: 10, depth: 5 });
fitView();
({ plate, types: types.length });                 // der letzte Ausdruck ist das Ergebnis

// „Mach die Platte 10 mm dicker."
const doc = getDocument();
const box = doc.features.find(f => f.type === "box");
const p = getFeature(box.id);                     // KOPIE
p.size = [p.size[0], p.size[1], p.size[2] + 10];  // ändern
updateFeature(box.id, p);                         // zurückschreiben

// „Was ist in dem Dokument?"
getDocument();                                    // einfach zurückgeben, nicht erzählen

// ── FieldSkript ───────────────────────────────────────────────────────────────
// Ein Skript-Feature ist TypeScript mit GENAU dieser Form. Halte dich daran —
// die Namen sind nicht ratbar:
//   · "parameters" ist ein ARRAY von Parameter-Buildern: fd.length("key", {…}),
//     fd.real("key", {…}), fd.angle, fd.bool, fd.str, fd.vec3, fd.enumParam.
//   · "build" ist der DEFAULT-Export.
//   · Werte kommen als Objekt: Number(def.spacing.value) — nicht def.spacing.
//   · Geometrie erzeugen die fd.op*-Funktionen: opBox, opSphere, opCylinder,
//     opExtrude, opRevolve, opHole, opFillet, opBoolean. Jede nimmt
//     (context, id, def) — und jede braucht eine EIGENE Id: id.sub("name").
//   · Erlaubter Import: nur "fielddraft".
//
// „Mach mir ein Feature, das eine Lochreihe erzeugt." → Skript vorschlagen:
proposeScript("Lochreihe", \`
import { fd } from "fielddraft";

export const definition = fd.feature({
  name: "Lochreihe",
  icon: "⬡",
  parameters: [
    fd.real("count", { name: "Anzahl", default: 3 }),
    fd.length("spacing", { name: "Abstand", default: 20 }),
    fd.length("diameter", { name: "Durchmesser", default: 6 }),
    fd.length("depth", { name: "Tiefe", default: 10 }),
  ],
});

export default function build(context, id, def) {
  const count = Number(def.count.value);
  const spacing = Number(def.spacing.value);
  const start = -((count - 1) * spacing) / 2;   // zentriert

  for (let i = 0; i < count; i++) {
    fd.opHole(context, id.sub("h" + i), {
      diameter: Number(def.diameter.value),
      depth: Number(def.depth.value),
      center: [start + i * spacing, 0, 0],
    });
  }
}
\`, "Erzeugt eine zentrierte, lineare Reihe von Bohrungen.");
// → liegt jetzt dem Nutzer vor. NICHT behaupten, es sei gelaufen.
`;
