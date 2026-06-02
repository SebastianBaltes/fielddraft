import { describe, it, expect } from "vitest";
import { evalDefinition } from "./evalDefinition";
import { sucraseTranspiler } from "./transpile";

describe("evalDefinition", () => {
  it("round-trips real ESM/TS source through sucrase and reads the definition", () => {
    const SOURCE = `import { fd } from "fielddraft";
export const definition = fd.feature({ name: "Slot", parameters: [fd.length("width", { default: 10 }), fd.bool("through", { default: true })] });
export default function build(context, id, def: { width: number }) { fd.opBox(context, id, { half_extents: [def.width, 1, 1] }); }`;
    const def = evalDefinition(sucraseTranspiler(SOURCE));
    expect(def.name).toBe("Slot");
    // `build()` references `fd.opBox`, which the authoring module lacks —
    // but extraction never calls build(), so it does not break.
    expect(def.parameters.map((p) => p.key)).toEqual(["width", "through"]);
  });

  it("reads the definition from a hand-written CommonJS body", () => {
    const cjs = `const { fd } = require("fielddraft");
module.exports.definition = fd.feature({ name: "Hole", parameters: [fd.length("depth", { default: 5 })] });`;
    const def = evalDefinition(cjs);
    expect(def.name).toBe("Hole");
    expect(def.parameters.map((p) => p.key)).toEqual(["depth"]);
  });

  it("throws when the body imports a forbidden specifier", () => {
    const cjs = `const fs = require("node:fs");
module.exports.definition = { name: "X", parameters: [] };`;
    expect(() => evalDefinition(cjs)).toThrow(/only import "fielddraft"/);
  });

  it("throws when the body exports no definition", () => {
    const cjs = `module.exports.default = function build() {};`;
    expect(() => evalDefinition(cjs)).toThrow(/no valid `definition`/);
  });

  it("throws when the definition is malformed (no parameters array)", () => {
    const cjs = `module.exports.definition = { name: "X" };`;
    expect(() => evalDefinition(cjs)).toThrow(/no valid `definition`/);
  });
});
