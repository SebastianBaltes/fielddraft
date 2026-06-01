import { describe, it, expect } from "vitest";
import { feature, length, bool } from "./dsl";
import { createRecord, reviseRecord, identityTranspiler } from "./record";

const definition = feature({
  name: "Slot",
  parameters: [length("width", { default: 10 }), bool("through", { default: true })],
});

const SOURCE = `export const definition = fd.feature({ /* ... */ });
export default function build() {}`;

describe("createRecord", () => {
  it("builds a v1 record with schema extracted from the definition", () => {
    const rec = createRecord({ id: "slot", source: SOURCE, definition });
    expect(rec.id).toBe("slot");
    expect(rec.name).toBe("Slot"); // from definition.name
    expect(rec.version).toBe(1);
    expect(rec.schema.type).toBe("slot");
    expect(rec.schema.params.map((p) => p.key)).toEqual(["width", "through"]);
  });

  it("defaults to the identity transpiler (compiledJs === source)", () => {
    const rec = createRecord({ id: "slot", source: SOURCE, definition });
    expect(rec.compiledJs).toBe(SOURCE);
    expect(identityTranspiler(SOURCE)).toBe(SOURCE);
  });

  it("uses an injected transpiler when provided", () => {
    const rec = createRecord({
      id: "slot",
      source: SOURCE,
      definition,
      transpile: (s) => `/*compiled*/\n${s}`,
    });
    expect(rec.compiledJs.startsWith("/*compiled*/")).toBe(true);
  });

  it("name override wins over definition.name", () => {
    const rec = createRecord({ id: "slot", source: SOURCE, definition, name: "Keyway" });
    expect(rec.name).toBe("Keyway");
  });
});

describe("reviseRecord", () => {
  it("bumps version and recompiles, preserving the id", () => {
    const v1 = createRecord({ id: "slot", source: SOURCE, definition });
    const nextDef = feature({
      name: "Slot",
      parameters: [length("width", { default: 12 })],
    });
    const v2 = reviseRecord(v1, { source: "// edited", definition: nextDef });
    expect(v2.id).toBe("slot");
    expect(v2.version).toBe(2);
    expect(v2.source).toBe("// edited");
    expect(v2.schema.params).toHaveLength(1);
    // v1 is untouched (pure).
    expect(v1.version).toBe(1);
    expect(v1.schema.params).toHaveLength(2);
  });
});
