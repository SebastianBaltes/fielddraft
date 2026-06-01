import { describe, it, expect } from "vitest";
import { sucraseTranspiler, identityTranspiler } from "./transpile";
import {
  buildFdModule,
  compileBuild,
  makeBuildId,
  type ScriptHostDeps,
} from "../workerScriptHost";

const SOURCE = `import { fd } from "fielddraft";
export const definition = fd.feature({ name: "Slot", parameters: [] });
export default function build(context, id, def: { width: number }) {
  fd.opBox(context, id.sub("a"), { half_extents: [def.width, 1, 1] });
}`;

describe("sucraseTranspiler", () => {
  it("rewrites the fielddraft import into a require the host resolves", () => {
    const out = sucraseTranspiler(SOURCE);
    expect(out).toContain(`require('fielddraft')`);
    expect(out).not.toMatch(/^\s*import\s/m);
  });

  it("turns the default export into module.exports.default", () => {
    const out = sucraseTranspiler(SOURCE);
    expect(out).toContain("exports.default = build");
  });

  it("strips TypeScript type annotations", () => {
    expect(sucraseTranspiler(SOURCE)).not.toContain(": { width: number }");
  });

  it("produces a body the worker host can compile and run end-to-end", () => {
    // The real proof: sucrase output → compileBuild → fd.op* → commit.
    const calls: { id: string; type: string; def: string }[] = [];
    const deps: ScriptHostDeps = {
      commit(id, type, defJson) {
        calls.push({ id, type, def: defJson });
        return [id];
      },
    };
    const build = compileBuild(sucraseTranspiler(SOURCE), buildFdModule(deps));
    build({ _brand: "fd.Context" }, makeBuildId("12"), { width: 7 });
    expect(calls).toEqual([
      { id: "12.a", type: "box", def: JSON.stringify({ half_extents: [7, 1, 1] }) },
    ]);
  });
});

describe("identityTranspiler", () => {
  it("returns already-CommonJS bodies untouched", () => {
    const cjs = `const { fd } = require("fielddraft"); module.exports.default = () => {};`;
    expect(identityTranspiler(cjs)).toBe(cjs);
  });
});
