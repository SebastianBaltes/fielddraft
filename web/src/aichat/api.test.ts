/**
 * The description the MODEL is served (ticket #347).
 *
 * The compiler checks that api.ts is TRUE (setup.ts binds the implementation to
 * `FieldDraftAgentApi`). It cannot check what the model actually READS — that is
 * a string, produced by expandRuntimeTypes from the same file. This test covers
 * the gap between the two:
 *
 *  · the @api-start/@api-end markers really trim the file (no imports, no header)
 *  · `// @values` really expands to the document's live values — it did NOT for
 *    months, because the annotation sat inside a JSDoc block where the library's
 *    regex could never match it, and a silent no-op looks exactly like a working
 *    one from the outside.
 */

import { describe, expect, it } from "vitest";
import { expandRuntimeTypes } from "baseaichat";
import apiSource from "./api.ts?raw";

const description = () =>
  expandRuntimeTypes(apiSource, {
    featureTypes: ["box", "hole", "meineLochreihe"],
    variableNames: ["breite", "hoehe"],
  });

describe("the API description the model reads", () => {
  it("expands @values to the types that exist right now", () => {
    const out = description();
    expect(out).toContain('"box"');
    expect(out).toContain('"meineLochreihe"'); // the user's own script feature
    expect(out).toContain('"breite"');
  });

  it("leaves no @values annotation behind", () => {
    // A leftover annotation means the expansion silently did nothing — the model
    // would read the word "@values" and learn precisely nothing from it.
    expect(description()).not.toContain("@values");
  });

  it("shows only the marked section — no header, no imports", () => {
    const out = description();
    expect(out).not.toContain("import type");
    expect(out).not.toContain("@api-start");
    expect(out).not.toContain("export type");
  });

  it("teaches the UI functions that actually exist", () => {
    const out = description();
    // The names the model may call (setup.ts maps them onto the library's
    // clickElement/fillInput). The old description promised these two while
    // nobody implemented them — the assistant could not press a button.
    expect(out).toContain("click(key: string): void");
    expect(out).toContain("setValue(key: string, value: string): void");
  });
});
