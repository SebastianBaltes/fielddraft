import { describe, it, expect, vi } from "vitest";
import {
  makeBuildId,
  buildFdModule,
  compileBuild,
  makeScriptHost,
  type ScriptHostDeps,
} from "./workerScriptHost";

/** A fake `commit` that records every op and returns a deterministic handle. */
function recordingDeps(): ScriptHostDeps & {
  calls: { id: string; type: string; def: string }[];
} {
  const calls: { id: string; type: string; def: string }[] = [];
  return {
    calls,
    commit(id, type, defJson) {
      calls.push({ id, type, def: defJson });
      return [id];
    },
  };
}

/** A CJS build that emits one box under `id.sub("a")`. */
const BOX_SCRIPT = `
const { fd } = require("fielddraft");
module.exports.definition = fd.feature({ name: "Slot", parameters: [] });
module.exports.default = function build(context, id, def) {
  fd.opBox(context, id.sub("a"), { half_extents: [def.w, 1, 1], center: [0, 0, 0] });
};
`;

describe("makeBuildId", () => {
  it("dots sub-ids and stringifies to the raw id", () => {
    const id = makeBuildId("10");
    expect(String(id)).toBe("10");
    expect(String(id.sub("bore"))).toBe("10.bore");
    expect(String(id.sub("bore").sub("ring"))).toBe("10.bore.ring");
  });
});

describe("buildFdModule", () => {
  it("forwards fd.opBox to commit with the built-in type_name and def", () => {
    const deps = recordingDeps();
    const mod = buildFdModule(deps);
    const fd = mod.fd as Record<string, (...a: unknown[]) => { id: string }>;
    const handle = fd.opBox({ _brand: "fd.Context" }, makeBuildId("3.a"), {
      half_extents: [1, 2, 3],
    });
    expect(deps.calls).toEqual([
      { id: "3.a", type: "box", def: JSON.stringify({ half_extents: [1, 2, 3] }) },
    ]);
    expect(handle.id).toBe("3.a");
  });

  it("maps each op to its kernel built-in", () => {
    const deps = recordingDeps();
    const fd = buildFdModule(deps).fd as Record<string, (...a: unknown[]) => unknown>;
    fd.opSphere({}, makeBuildId("1"), {});
    fd.opCylinder({}, makeBuildId("2"), {});
    fd.opHole({}, makeBuildId("3"), {});
    fd.opFillet({}, makeBuildId("4"), {});
    fd.opBoolean({}, makeBuildId("5"), {});
    expect(deps.calls.map((c) => c.type)).toEqual([
      "sphere",
      "cylinder",
      "hole",
      "fillet_edge",
      "boolean",
    ]);
  });

  it("still exposes the S1 authoring builders for top-level evaluation", () => {
    const fd = buildFdModule(recordingDeps()).fd as Record<string, unknown>;
    expect(typeof fd.feature).toBe("function");
    expect(typeof fd.length).toBe("function");
  });

  it("fails loudly on the not-yet-implemented query / call surface", () => {
    const fd = buildFdModule(recordingDeps()).fd as {
      call: () => void;
      q: { createdBy: () => void; evaluate: () => void };
    };
    expect(() => fd.call()).toThrow(/later FieldSkript stage/);
    expect(() => fd.q.createdBy()).toThrow(/S3 query system/);
    expect(() => fd.q.evaluate()).toThrow(/S3 query system/);
  });
});

describe("compileBuild", () => {
  it("evaluates a CJS body and returns its default build()", () => {
    const deps = recordingDeps();
    const build = compileBuild(BOX_SCRIPT, buildFdModule(deps));
    build({ _brand: "fd.Context" }, makeBuildId("7"), { w: 5 });
    expect(deps.calls).toHaveLength(1);
    expect(deps.calls[0]).toMatchObject({ id: "7.a", type: "box" });
    expect(JSON.parse(deps.calls[0].def).half_extents).toEqual([5, 1, 1]);
  });

  it("rejects importing anything but the fielddraft module", () => {
    const bad = `const x = require("node:fs"); module.exports.default = () => {};`;
    expect(() => compileBuild(bad, buildFdModule(recordingDeps()))).toThrow(
      /only import "fielddraft"/,
    );
  });

  it("rejects a body with no callable default export", () => {
    const noDefault = `module.exports.definition = {};`;
    expect(() => compileBuild(noDefault, buildFdModule(recordingDeps()))).toThrow(
      /no callable default build/,
    );
  });
});

describe("makeScriptHost", () => {
  it("compiles on sync and dispatches the right build on run", () => {
    const deps = recordingDeps();
    const host = makeScriptHost(deps);
    host.sync([{ id: "slot", compiledJs: BOX_SCRIPT }]);
    host.run("slot", "42", JSON.stringify({ w: 9 }));
    expect(deps.calls).toEqual([
      {
        id: "42.a",
        type: "box",
        def: JSON.stringify({ half_extents: [9, 1, 1], center: [0, 0, 0] }),
      },
    ]);
  });

  it("throws for an unknown script id", () => {
    const host = makeScriptHost(recordingDeps());
    host.sync([]);
    expect(() => host.run("missing", "1", "{}")).toThrow(/no compiled build/);
  });

  it("skips records with empty compiledJs (untranspiled)", () => {
    const deps = recordingDeps();
    const host = makeScriptHost(deps);
    host.sync([{ id: "slot", compiledJs: "" }]);
    expect(() => host.run("slot", "1", "{}")).toThrow(/no compiled build/);
  });

  it("recompiles the table on each sync", () => {
    const deps = recordingDeps();
    const host = makeScriptHost(deps);
    host.sync([{ id: "a", compiledJs: BOX_SCRIPT }]);
    host.sync([{ id: "b", compiledJs: BOX_SCRIPT }]);
    expect(() => host.run("a", "1", "{}")).toThrow(/no compiled build/);
    host.run("b", "2", "{}");
    expect(deps.calls).toHaveLength(1);
  });

  it("defaults def to {} when paramsJson is empty", () => {
    const deps = recordingDeps();
    const host = makeScriptHost(deps);
    const script = `
      const { fd } = require("fielddraft");
      module.exports.default = (context, id, def) => {
        fd.opBox(context, id, { got: def });
      };
    `;
    host.sync([{ id: "s", compiledJs: script }]);
    host.run("s", "1", "");
    expect(JSON.parse(deps.calls[0].def)).toEqual({ got: {} });
  });
});

describe("fd.op handle from real commit shape", () => {
  it("returns the first created handle id", () => {
    const deps: ScriptHostDeps = {
      commit: vi.fn(() => ["9.boss", "9.boss.fillet"]),
    };
    const fd = buildFdModule(deps).fd as Record<string, (...a: unknown[]) => { id: string }>;
    const handle = fd.opBox({}, makeBuildId("9"), {});
    expect(handle.id).toBe("9.boss");
  });
});
