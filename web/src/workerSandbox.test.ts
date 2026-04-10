import { describe, expect, it } from "vitest";
import {
  FD_INNER_API_VERSION,
  freezeGlobals,
  hashDocumentJson,
  installInnerApi,
  mulberry32,
  reseedRandom,
  type SandboxScope,
} from "./workerSandbox";

/**
 * A mock scope that mirrors the shape of the real `globalThis` the
 * worker operates on. Only the properties the sandbox touches are
 * meaningful; the rest is there so TypeScript's shape check stays
 * happy.
 */
function mockScope(): SandboxScope & Record<string, unknown> {
  return {
    fetch: () => Promise.resolve(),
    setTimeout: (() => 0) as unknown,
    setInterval: (() => 0) as unknown,
    clearTimeout: () => {},
    clearInterval: () => {},
    Date: { now: () => 1234567890 } as unknown as DateConstructor,
    // A standalone Math object so we don't mutate the real one
    // across test cases.
    Math: { ...Math } as Math,
  };
}

describe("workerSandbox.freezeGlobals", () => {
  it("removes fetch so typeof fetch is 'undefined'", () => {
    const scope = mockScope();
    freezeGlobals(scope);
    expect("fetch" in scope).toBe(false);
  });

  it("removes the timer globals and their clearers", () => {
    const scope = mockScope();
    freezeGlobals(scope);
    expect("setTimeout" in scope).toBe(false);
    expect("setInterval" in scope).toBe(false);
    expect("clearTimeout" in scope).toBe(false);
    expect("clearInterval" in scope).toBe(false);
  });

  it("clamps Date.now to a zero stub", () => {
    const scope = mockScope();
    expect(scope.Date.now()).toBe(1234567890);
    freezeGlobals(scope);
    expect(scope.Date.now()).toBe(0);
  });

  it("is idempotent — calling twice is a no-op the second time", () => {
    const scope = mockScope();
    freezeGlobals(scope);
    freezeGlobals(scope);
    expect("fetch" in scope).toBe(false);
    expect(scope.Date.now()).toBe(0);
  });
});

describe("workerSandbox.reseedRandom", () => {
  it("is deterministic for the same seed", () => {
    const a = mockScope();
    const b = mockScope();
    reseedRandom(a, 42);
    reseedRandom(b, 42);
    const seqA = Array.from({ length: 5 }, () => a.Math.random());
    const seqB = Array.from({ length: 5 }, () => b.Math.random());
    expect(seqA).toEqual(seqB);
  });

  it("produces different sequences for different seeds", () => {
    const a = mockScope();
    const b = mockScope();
    reseedRandom(a, 1);
    reseedRandom(b, 2);
    const seqA = Array.from({ length: 5 }, () => a.Math.random());
    const seqB = Array.from({ length: 5 }, () => b.Math.random());
    expect(seqA).not.toEqual(seqB);
  });

  it("returns values in the standard [0, 1) range", () => {
    const scope = mockScope();
    reseedRandom(scope, 7);
    for (let i = 0; i < 100; i++) {
      const v = scope.Math.random();
      expect(v).toBeGreaterThanOrEqual(0);
      expect(v).toBeLessThan(1);
    }
  });

  it("can be called again to install a fresh PRNG without leaking state", () => {
    const scope = mockScope();
    reseedRandom(scope, 100);
    const first = scope.Math.random();
    reseedRandom(scope, 100);
    const second = scope.Math.random();
    expect(first).toBe(second);
  });
});

describe("workerSandbox.mulberry32", () => {
  it("advances one step at a time", () => {
    const rng = mulberry32(12345);
    const a = rng();
    const b = rng();
    expect(a).not.toBe(b);
  });

  it("has a full 32-bit output range over many draws", () => {
    const rng = mulberry32(999);
    let min = 1;
    let max = 0;
    for (let i = 0; i < 10_000; i++) {
      const v = rng();
      if (v < min) min = v;
      if (v > max) max = v;
    }
    // Empirical sanity check: after 10k draws we should see both
    // ends of the range populated well inside the quartiles.
    expect(min).toBeLessThan(0.05);
    expect(max).toBeGreaterThan(0.95);
  });
});

describe("workerSandbox.hashDocumentJson", () => {
  it("is deterministic", () => {
    const a = hashDocumentJson('{"name":"Test"}');
    const b = hashDocumentJson('{"name":"Test"}');
    expect(a).toBe(b);
  });

  it("distinguishes different documents", () => {
    const a = hashDocumentJson('{"name":"Alpha"}');
    const b = hashDocumentJson('{"name":"Beta"}');
    expect(a).not.toBe(b);
  });

  it("handles the empty string without overflowing", () => {
    const h = hashDocumentJson("");
    expect(Number.isFinite(h)).toBe(true);
    expect(h).toBeGreaterThanOrEqual(0);
    expect(h).toBeLessThanOrEqual(0xffffffff);
  });

  it("stays inside the 32-bit unsigned range for long inputs", () => {
    const long = "x".repeat(10_000);
    const h = hashDocumentJson(long);
    expect(h).toBeGreaterThanOrEqual(0);
    expect(h).toBeLessThanOrEqual(0xffffffff);
  });
});

describe("workerSandbox.installInnerApi", () => {
  it("installs globalThis.fd with the version marker", () => {
    const scope = mockScope();
    installInnerApi(scope);
    expect(scope.fd).toBeDefined();
    expect(scope.fd?.version).toBe(FD_INNER_API_VERSION);
  });

  it("log sink accumulates call arguments in order", () => {
    const scope = mockScope();
    const api = installInnerApi(scope);
    api.log("hello");
    api.log("world", 42);
    expect(api.logs.length).toBe(2);
    expect(api.logs[0]).toEqual(["hello"]);
    expect(api.logs[1]).toEqual(["world", 42]);
  });

  it("is replaceable — a second install wipes the old log buffer", () => {
    const scope = mockScope();
    const first = installInnerApi(scope);
    first.log("old entry");
    const second = installInnerApi(scope);
    expect(second.logs.length).toBe(0);
    expect(scope.fd).toBe(second);
  });

  it("ping returns 'pong: <msg>' for the Outer API round-trip probe", () => {
    const scope = mockScope();
    const api = installInnerApi(scope);
    expect(api.ping("hello")).toBe("pong: hello");
    expect(api.ping("")).toBe("pong: ");
  });

  it("echo returns its argument unchanged", () => {
    const scope = mockScope();
    const api = installInnerApi(scope);
    expect(api.echo(42)).toBe(42);
    expect(api.echo("hi")).toBe("hi");
    const payload = { a: 1, b: [2, 3] };
    expect(api.echo(payload)).toBe(payload);
  });
});

describe("workerSandbox integration: freeze + seed + install", () => {
  it("a worker-style init call leaves the scope in the fully locked shape", () => {
    const scope = mockScope();
    freezeGlobals(scope);
    reseedRandom(scope, hashDocumentJson('{"doc":"v1"}'));
    const api = installInnerApi(scope);

    expect("fetch" in scope).toBe(false);
    expect("setTimeout" in scope).toBe(false);
    expect(scope.Date.now()).toBe(0);
    expect(typeof scope.Math.random()).toBe("number");
    expect(scope.fd).toBe(api);
  });
});
