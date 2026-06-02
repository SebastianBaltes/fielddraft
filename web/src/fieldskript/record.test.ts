import { describe, it, expect } from "vitest";
import { createRecord, reviseRecord } from "./record";

const SOURCE = `import { fd } from "fielddraft";
export const definition = fd.feature({
  name: "Slot",
  parameters: [fd.length("width", { default: 10 }), fd.bool("through", { default: true })],
});
export default function build(context, id, def: { width: number }) {
  fd.opBox(context, id, { half_extents: [def.width, 1, 1] });
}`;

describe("createRecord", () => {
  it("builds a v1 record with schema derived from the source's definition", () => {
    const rec = createRecord({ id: "slot", source: SOURCE });
    expect(rec.id).toBe("slot");
    expect(rec.name).toBe("Slot"); // from the derived definition.name
    expect(rec.version).toBe(1);
    expect(rec.schema.type).toBe("slot");
    expect(rec.schema.params.map((p) => p.key)).toEqual(["width", "through"]);
  });

  it("defaults to the sucrase transpiler (ESM/TS → CommonJS)", () => {
    const rec = createRecord({ id: "slot", source: SOURCE });
    // ESM in, CommonJS out: the import becomes a require the worker host
    // resolves, the default export becomes module.exports.default, and
    // the TypeScript annotation is stripped.
    expect(rec.compiledJs).toContain(`require('fielddraft')`);
    expect(rec.compiledJs).toContain("exports.default = build");
    expect(rec.compiledJs).not.toMatch(/^\s*import\s/m);
    expect(rec.compiledJs).not.toContain(": { width: number }");
  });

  it("uses an injected transpiler when provided", () => {
    // The injected transpiler must yield valid CJS exporting a
    // definition, since evalDefinition runs against its output.
    const compiled =
      'const { fd } = require("fielddraft");\nmodule.exports.definition = fd.feature({ name: "Injected", parameters: [fd.bool("on")] });\nmodule.exports.default = () => {};';
    const rec = createRecord({
      id: "slot",
      source: SOURCE,
      transpile: () => compiled,
    });
    expect(rec.compiledJs).toBe(compiled);
    expect(rec.name).toBe("Injected");
    expect(rec.schema.params.map((p) => p.key)).toEqual(["on"]);
  });

  it("name override wins over the derived definition.name", () => {
    const rec = createRecord({ id: "slot", source: SOURCE, name: "Keyway" });
    expect(rec.name).toBe("Keyway");
  });
});

describe("reviseRecord", () => {
  it("bumps version and re-derives the schema, preserving the id", () => {
    const v1 = createRecord({ id: "slot", source: SOURCE });
    const nextSource = `import { fd } from "fielddraft";
export const definition = fd.feature({ name: "Slot", parameters: [fd.length("width", { default: 12 })] });
export default function build(){}`;
    const v2 = reviseRecord(v1, { source: nextSource });
    expect(v2.id).toBe("slot");
    expect(v2.version).toBe(2);
    expect(v2.source).toBe(nextSource);
    expect(v2.schema.params).toHaveLength(1);
    // v1 is untouched (pure).
    expect(v1.version).toBe(1);
    expect(v1.schema.params).toHaveLength(2);
  });
});
