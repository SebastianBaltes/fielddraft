/**
 * The API the MODEL reads — and a CONTRACT the compiler checks (ticket #347).
 *
 * This file is shown to the assistant verbatim: setup.ts imports it with `?raw`
 * and `expandRuntimeTypes` trims it down to the `@api-start`/`@api-end` section.
 * Everything outside those markers (this header, the import, the export below)
 * exists for the compiler only — the model never sees it.
 *
 * **Why an `interface` and not a hand-written string.** This used to be a
 * template literal full of signatures. `tsc` checked that it was a valid string;
 * nothing checked that it was TRUE. So it rotted, silently and in exactly the way
 * that looks like a model error: it promised `click()` and `setValue()`, which
 * nobody implemented, while the four UI functions that DO exist were invisible to
 * the model. The assistant read the description, called what it read, and failed.
 *
 * Now `setup.ts` builds the object under `const agentApi: FieldDraftAgentApi`, so
 * the three ways this can drift are build errors:
 *
 *   · described, not implemented → TS2741 "Property … is missing"
 *   · implemented, not described → TS2353 "… does not exist in type"
 *   · wrong signature            → TS2322 "Type … is not assignable"
 *
 * What the compiler still does NOT check are the EXAMPLES at the bottom: they are
 * comments. Copy them from code that actually ran — never write one from memory.
 *
 * `// @values <expr>` lines are filled with the document's real values before each
 * turn (expandRuntimeTypes in setup.ts). The annotation MUST be a line comment
 * directly above a `type X = string;` — inside a JSDoc block it silently does
 * nothing, which is what it did here for months.
 */

import type { ProposeResult } from "./scriptProposals";

// @api-start
// ── FieldDraft ────────────────────────────────────────────────────────────────
// Parametrischer CAD-Editor. Einheiten: MILLIMETER, Winkel in GRAD.
// Ein Dokument ist ein FEATURE-BAUM: jedes Feature erzeugt oder verändert
// Geometrie, in der Reihenfolge des Baums.

interface FeatureInfo {
  id: number;
  type: string;
  name: string;
  suppressed: boolean;
}

interface DocumentSummary {
  featureCount: number;
  bodyCount: number;
  features: FeatureInfo[];
  /** Feature-Ids */
  selection: number[];
  /** Feature-Id des Sketches, der gerade editiert wird — sonst null. */
  activeSketch: number | null;
  /** Der Wert kann eine FORMEL sein (String), nicht nur eine Zahl. */
  variables: { name: string; value: number | string }[];
  /** Bounding-Box in mm */
  sceneSize: [number, number, number];
}

/** Die Feature-Typen, die DIESES Dokument kennt — inklusive der Skript-Features
 *  des Nutzers, die es nur zur Laufzeit gibt. Nie einen festen Satz annehmen. */
// @values context.featureTypes
type FeatureType = string;

/** Die Variablen, die es JETZT gibt. */
// @values context.variableNames
type VariableName = string;

type ViewPreset =
  | "front"
  | "back"
  | "left"
  | "right"
  | "top"
  | "bottom"
  | "iso";

interface FieldDraftAgentApi {
  // ── Lesen ───────────────────────────────────────────────────────────────────

  /** Das ganze Dokument auf einen Blick. Billig — im Zweifel aufrufen. */
  getDocument(): DocumentSummary;

  /** Alle Feature-Typen, die dieses Dokument kennt. */
  listFeatureTypes(): FeatureType[];

  /** Das Parameter-Schema eines Typs (JSON-Schema-artig). IMMER lesen, bevor du
   *  addFeature aufrufst — die Feldnamen sind nicht ratbar. */
  schemaForType(type: FeatureType): unknown;

  /** Die aktuellen Parameter eines Features — eine KOPIE. Sie zu verändern bewirkt
   *  nichts; schreib sie mit updateFeature zurück. null, wenn es die Id nicht gibt. */
  getFeature(id: number): Record<string, unknown> | null;

  // ── Schreiben ───────────────────────────────────────────────────────────────

  /** Legt ein Feature an und gibt seine Id zurück. Der generische Weg — Primitive,
   *  Booleans, Muster, Transformationen und Skript-Features laufen alle hierüber.
   *  Wirft bei unbekanntem Typ oder falschen Parametern. */
  addFeature(type: FeatureType, params: Record<string, unknown>): number;

  /** Ersetzt die Parameter eines Features (VOLLE Wire-Form: getFeature lesen,
   *  ändern, ganzes Objekt zurückschreiben). */
  updateFeature(id: number, params: Record<string, unknown>): boolean;

  renameFeature(id: number, name: string): boolean;
  setFeatureSuppressed(id: number, suppressed: boolean): void;

  /** ZERSTÖREND. Nur nach ausdrücklicher Bestätigung des Nutzers. */
  removeFeature(id: number): void;

  /** Dokument-Variable setzen (in jedem Zahlenfeld als Formel nutzbar). */
  setVariable(name: VariableName, value: number): boolean;

  undo(): void;
  redo(): void;

  /** Schlägt ein FieldSkript-Feature vor (TypeScript). Es wird NICHT ausgeführt:
   *  die Quelle wird geprüft und dem NUTZER vorgelegt, der entscheidet.
   *  {ok:false, reason, line} → korrigieren und neu vorschlagen.
   *  {ok:true} → der Nutzer sieht das Skript; behaupte NICHT, es sei gelaufen.
   *  Erlaubter Import: nur "fielddraft". */
  proposeScript(name: string, source: string, intent: string): ProposeResult;

  // ── Auswahl & Ansicht ───────────────────────────────────────────────────────

  /** Ein Feature auswählen (null = Auswahl leeren). */
  select(id: number | null): void;

  /** Kamera auf eine Standardansicht. */
  setView(preset: ViewPreset): void;

  /** Zoomt so, dass das ganze Modell ins Bild passt. */
  fitView(): void;

  // ── Oberfläche lesen und bedienen ───────────────────────────────────────────
  // Sieht NUR DOM-Bedienelemente (Buttons, Felder, Listen) — NICHT den 3D-Viewport.
  // Szene, Auswahl und Maße kommen aus getDocument().
  //
  // Adressiert wird IMMER über den data-*-key, den readUIState liefert — nie über
  // CSS-Selektoren, nie über sichtbaren Text.

  /** Die sichtbaren Bedienelemente mit ihren data-*-keys. */
  readUIState(): Record<string, unknown>;

  /** Button/Element klicken. */
  click(key: string): void;

  /** Ein Eingabefeld füllen (löst input+change aus, wie eine echte Eingabe). */
  setValue(key: string, value: string): void;

  /** Eine Option in einem <select> wählen. */
  selectOption(key: string, option: string): void;

  /** Ein Element kurz hervorheben — zeigt dem Nutzer, wovon du redest. */
  highlightElement(key: string): void;
}

// ── Beispiele ─────────────────────────────────────────────────────────────────
// Aus laufendem Code kopiert. Der Compiler prüft sie NICHT — erfinde hier nichts.
//
// „Bau mir eine Grundplatte 80 × 60 × 5 mit einem 10er-Loch in der Mitte."
//   const types = listFeatureTypes();                 // welche Typen gibt es?
//   const boxSchema = schemaForType("box");           // wie heißen die Felder?
//   const plate = addFeature("box", { size: [80, 60, 5] });
//   const holeSchema = schemaForType("hole");
//   addFeature("hole", { diameter: 10, depth: 5 });
//   fitView();
//   ({ plate, types: types.length });                 // der letzte Ausdruck ist das Ergebnis
//
// „Mach die Platte 10 mm dicker."
//   const doc = getDocument();
//   const box = doc.features.find(f => f.type === "box");
//   const p = getFeature(box.id);                     // KOPIE
//   p.size = [p.size[0], p.size[1], p.size[2] + 10];  // ändern
//   updateFeature(box.id, p);                         // zurückschreiben
//
// „Was ist in dem Dokument?"
//   getDocument();                                    // einfach zurückgeben, nicht erzählen
//
// „Schalt den Draht-Modus ein." — erst schauen, dann klicken:
//   const ui = readUIState();                         // liefert die data-*-keys
//   click("view.wireframe");
//
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
//
//   proposeScript("Lochreihe", `
//   import { fd } from "fielddraft";
//
//   export const definition = fd.feature({
//     name: "Lochreihe",
//     icon: "⬡",
//     parameters: [
//       fd.real("count", { name: "Anzahl", default: 3 }),
//       fd.length("spacing", { name: "Abstand", default: 20 }),
//       fd.length("diameter", { name: "Durchmesser", default: 6 }),
//       fd.length("depth", { name: "Tiefe", default: 10 }),
//     ],
//   });
//
//   export default function build(context, id, def) {
//     const count = Number(def.count.value);
//     const spacing = Number(def.spacing.value);
//     const start = -((count - 1) * spacing) / 2;   // zentriert
//
//     for (let i = 0; i < count; i++) {
//       fd.opHole(context, id.sub("h" + i), {
//         diameter: Number(def.diameter.value),
//         depth: Number(def.depth.value),
//         center: [start + i * spacing, 0, 0],
//       });
//     }
//   }
//   `, "Erzeugt eine zentrierte, lineare Reihe von Bohrungen.");
//   // → liegt jetzt dem Nutzer vor. NICHT behaupten, es sei gelaufen.
// @api-end

export type { FieldDraftAgentApi, FeatureInfo, DocumentSummary };
