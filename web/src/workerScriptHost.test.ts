import { describe, it, expect, vi } from "vitest";
import {
  makeBuildId,
  buildFdModule,
  compileBuild,
  makeScriptHost,
  scriptRecordsFrom,
  type ScriptHostDeps,
  type QueryHandle,
} from "./workerScriptHost";

/**
 * A fake `commit`/`resolveQuery` pair that records every call. `commit`
 * returns a deterministic handle; `resolveQuery` records the canonical
 * query string and yields a fixed handle list so passthrough is testable.
 */
function recordingDeps(): ScriptHostDeps & {
  calls: { id: string; type: string; def: string }[];
  queries: string[];
} {
  const calls: { id: string; type: string; def: string }[] = [];
  const queries: string[] = [];
  return {
    calls,
    queries,
    commit(id, type, defJson) {
      calls.push({ id, type, def: defJson });
      return [id];
    },
    resolveQuery(query) {
      queries.push(query);
      return [
        { kind: "face", idHash: "42", contributors: [query] },
      ] satisfies QueryHandle[];
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

describe("scriptRecordsFrom", () => {
  it("parses a document's scripts into id + compiledJs records", () => {
    const doc = JSON.stringify({
      scripts: [
        { id: "widget", compiledJs: "module.exports.default = function(){};", source: "…" },
        { id: "slot", compiledJs: "//js" },
      ],
    });
    expect(scriptRecordsFrom(doc)).toEqual([
      { id: "widget", compiledJs: "module.exports.default = function(){};" },
      { id: "slot", compiledJs: "//js" },
    ]);
  });

  it("yields [] for a script-free document, a missing field, or malformed JSON", () => {
    expect(scriptRecordsFrom(JSON.stringify({ features: [] }))).toEqual([]);
    expect(scriptRecordsFrom("{}")).toEqual([]);
    expect(scriptRecordsFrom("not json")).toEqual([]);
  });

  it("drops entries missing a string id or compiledJs", () => {
    const doc = JSON.stringify({
      scripts: [{ id: "ok", compiledJs: "x" }, { id: 7 }, { compiledJs: "y" }],
    });
    expect(scriptRecordsFrom(doc)).toEqual([{ id: "ok", compiledJs: "x" }]);
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

  it("fd.q throws when no resolver is injected (bare default)", () => {
    // No third arg → the bare default resolver, which must not no-op.
    const fd = buildFdModule(recordingDeps()).fd as {
      q: { createdBy: (t: string) => unknown };
    };
    expect(() => fd.q.createdBy("3.a")).toThrow(
      /requires the kernel query resolver/,
    );
  });

  it("fd.q.createdBy builds qCreatedBy(<id>) and passes the resolver result through", () => {
    const deps = recordingDeps();
    const resolved: QueryHandle[] = [
      { kind: "edge", idHash: "7", contributors: ["3.a"] },
    ];
    const fd = buildFdModule(deps, undefined, () => resolved).fd as {
      q: { createdBy: (t: unknown) => QueryHandle[] };
    };
    const out = fd.q.createdBy({ id: "3.a" });
    expect(out).toBe(resolved);
  });

  it("fd.q.createdBy emits the exact canonical string for a handle and a raw id", () => {
    const deps = recordingDeps();
    const fd = buildFdModule(deps, undefined, deps.resolveQuery).fd as {
      q: { createdBy: (t: unknown) => QueryHandle[] };
    };
    fd.q.createdBy({ id: "3.a" });
    fd.q.createdBy("9.boss");
    expect(deps.queries).toEqual(["qCreatedBy(3.a)", "qCreatedBy(9.boss)"]);
  });

  it("fd.q.faceOf builds qFaceOf(<id>)", () => {
    const deps = recordingDeps();
    const fd = buildFdModule(deps, undefined, deps.resolveQuery).fd as {
      q: { faceOf: (t: unknown) => QueryHandle[] };
    };
    fd.q.faceOf({ id: "12" });
    expect(deps.queries).toEqual(["qFaceOf(12)"]);
  });

  it("fd.q.edgeBetween builds the two-arg qEdgeBetween(a, b)", () => {
    const deps = recordingDeps();
    const fd = buildFdModule(deps, undefined, deps.resolveQuery).fd as {
      q: { edgeBetween: (a: unknown, b: unknown) => QueryHandle[] };
    };
    fd.q.edgeBetween({ id: "1" }, "2");
    expect(deps.queries).toEqual(["qEdgeBetween(1, 2)"]);
  });

  it("fd.q.face builds qFace(<idHash>) from a raw id_hash and a QueryHandle", () => {
    const deps = recordingDeps();
    const fd = buildFdModule(deps, undefined, deps.resolveQuery).fd as {
      q: { face: (h: unknown) => QueryHandle[] };
    };
    fd.q.face("12345");
    fd.q.face({ kind: "face", idHash: "678", contributors: [] });
    expect(deps.queries).toEqual(["qFace(12345)", "qFace(678)"]);
  });

  it("fd.q.edge builds qEdge(<idHash>) from a raw id_hash and a QueryHandle", () => {
    const deps = recordingDeps();
    const fd = buildFdModule(deps, undefined, deps.resolveQuery).fd as {
      q: { edge: (h: unknown) => QueryHandle[] };
    };
    fd.q.edge("42");
    fd.q.edge({ kind: "edge", idHash: "99", contributors: [] });
    expect(deps.queries).toEqual(["qEdge(42)", "qEdge(99)"]);
  });

  it("fd.q.vertex builds qVertex(<idHash>) from a raw id_hash and a QueryHandle", () => {
    const deps = recordingDeps();
    const fd = buildFdModule(deps, undefined, deps.resolveQuery).fd as {
      q: { vertex: (h: unknown) => QueryHandle[] };
    };
    fd.q.vertex("7");
    fd.q.vertex({ kind: "vertex", idHash: "8", contributors: [] });
    expect(deps.queries).toEqual(["qVertex(7)", "qVertex(8)"]);
  });

  it("fd.q.face passes the resolver result through unchanged", () => {
    const resolved: QueryHandle[] = [
      { kind: "face", idHash: "5", contributors: [] },
    ];
    const fd = buildFdModule(recordingDeps(), undefined, () => resolved).fd as {
      q: { face: (h: unknown) => QueryHandle[] };
    };
    expect(fd.q.face("5")).toBe(resolved);
  });

  it("fd.q.evaluate passes the raw canonical string through unchanged", () => {
    const deps = recordingDeps();
    const fd = buildFdModule(deps, undefined, deps.resolveQuery).fd as {
      q: { evaluate: (q: string) => QueryHandle[] };
    };
    const out = fd.q.evaluate("qFaceOf(3) && qCreatedBy(7)");
    expect(deps.queries).toEqual(["qFaceOf(3) && qCreatedBy(7)"]);
    expect(out).toEqual([
      { kind: "face", idHash: "42", contributors: ["qFaceOf(3) && qCreatedBy(7)"] },
    ]);
  });

  it("fd.call default resolves a built-in via commit", () => {
    const deps = recordingDeps();
    const fd = buildFdModule(deps).fd as {
      call: (...a: unknown[]) => { id: string };
    };
    const handle = fd.call({ _brand: "fd.Context" }, makeBuildId("3.b"), "box", {
      half_extents: [1, 1, 1],
    });
    expect(deps.calls).toEqual([
      {
        id: "3.b",
        type: "box",
        def: JSON.stringify({ half_extents: [1, 1, 1] }),
      },
    ]);
    expect(handle.id).toBe("3.b");
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

  it("a build() can query the geometry it just emitted via fd.q", () => {
    const deps = recordingDeps();
    const script = `
      const { fd } = require("fielddraft");
      module.exports.default = function build(context, id, def) {
        const box = fd.opBox(context, id.sub("a"), { half_extents: [1, 1, 1] });
        const faces = fd.q.createdBy(box);
        fd.opBox(context, id.sub("b"), { picked: faces.length });
      };
    `;
    const build = compileBuild(script, buildFdModule(deps, undefined, deps.resolveQuery));
    build({ _brand: "fd.Context" }, makeBuildId("7"), {});
    // The query was reached with the box's committed id (handle.id === "7.a").
    expect(deps.queries).toEqual(["qCreatedBy(7.a)"]);
    // …and the resolved handles flowed back into the second op's def.
    expect(JSON.parse(deps.calls[1].def)).toEqual({ picked: 1 });
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

  it("fd.call commits a built-in (script → built-in)", () => {
    const deps = recordingDeps();
    const host = makeScriptHost(deps);
    const script = `
      const { fd } = require("fielddraft");
      module.exports.default = function build(context, id, def) {
        fd.call(context, id.sub("b"), "box", { half_extents: [1, 1, 1] });
      };
    `;
    host.sync([{ id: "caller", compiledJs: script }]);
    host.run("caller", "1", "{}");
    expect(deps.calls).toHaveLength(1);
    expect(deps.calls[0]).toMatchObject({ id: "1.b", type: "box" });
  });

  it("fd.call recurses into another script in pure JS (script → script)", () => {
    const deps = recordingDeps();
    const host = makeScriptHost(deps);
    const leaf = `
      const { fd } = require("fielddraft");
      module.exports.default = function build(context, id, def) {
        fd.opBox(context, id.sub("box"), { half_extents: [def.w, 1, 1] });
      };
    `;
    const caller = `
      const { fd } = require("fielddraft");
      module.exports.default = function build(context, id, def) {
        fd.call(context, id.sub("inner"), "leaf", { w: def.w });
      };
    `;
    host.sync([
      { id: "leaf", compiledJs: leaf },
      { id: "caller", compiledJs: caller },
    ]);
    host.run("caller", "5", JSON.stringify({ w: 3 }));
    expect(deps.calls).toHaveLength(1);
    expect(deps.calls[0]).toMatchObject({ id: "5.inner.box", type: "box" });
    expect(JSON.parse(deps.calls[0].def).half_extents).toEqual([3, 1, 1]);
  });

  it("detects a self-cycle (A → A)", () => {
    const host = makeScriptHost(recordingDeps());
    const self = `
      const { fd } = require("fielddraft");
      module.exports.default = function build(context, id, def) {
        fd.call(context, id.sub("again"), "self", def);
      };
    `;
    host.sync([{ id: "self", compiledJs: self }]);
    expect(() => host.run("self", "1", "{}")).toThrow(/cycle/);
  });

  it("detects mutual recursion (A → B → A)", () => {
    const host = makeScriptHost(recordingDeps());
    const a = `
      const { fd } = require("fielddraft");
      module.exports.default = function build(context, id, def) {
        fd.call(context, id.sub("b"), "b", def);
      };
    `;
    const b = `
      const { fd } = require("fielddraft");
      module.exports.default = function build(context, id, def) {
        fd.call(context, id.sub("a"), "a", def);
      };
    `;
    host.sync([
      { id: "a", compiledJs: a },
      { id: "b", compiledJs: b },
    ]);
    expect(() => host.run("a", "1", "{}")).toThrow(/cycle/);
  });
});

describe("fd.op handle from real commit shape", () => {
  it("returns the first created handle id", () => {
    const deps: ScriptHostDeps = {
      commit: vi.fn(() => ["9.boss", "9.boss.fillet"]),
      resolveQuery: vi.fn(() => []),
    };
    const fd = buildFdModule(deps).fd as Record<string, (...a: unknown[]) => { id: string }>;
    const handle = fd.opBox({}, makeBuildId("9"), {});
    expect(handle.id).toBe("9.boss");
  });
});
