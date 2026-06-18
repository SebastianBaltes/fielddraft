import { describe, it, expect, beforeEach } from "vitest";
import {
  logUserAction,
  getActionLog,
  clearActionLog,
  serializeActionLog,
  type ActionEntry,
} from "./userActionLog";

describe("userActionLog (Report-Issue action trace)", () => {
  beforeEach(() => clearActionLog());

  it("records semantic actions oldest-first with category/name/detail", () => {
    logUserAction("sketch", "enter", { plane: "qFront" });
    logUserAction("sketch", "tool", { tool: "sketch_line" });
    const log = getActionLog() as ActionEntry[];
    expect(log).toHaveLength(2);
    expect(log[0]).toMatchObject({
      kind: "action",
      cat: "sketch",
      name: "enter",
      detail: { plane: "qFront" },
    });
    expect(log[1]).toMatchObject({
      kind: "action",
      cat: "sketch",
      name: "tool",
      detail: { tool: "sketch_line" },
    });
    expect(log[0].t).toBeLessThanOrEqual(log[1].t);
  });

  it("omits an empty detail and timestamps every entry", () => {
    logUserAction("sketch", "exit");
    const e = getActionLog()[0] as ActionEntry;
    expect(e.detail).toBeUndefined();
    expect(typeof e.t).toBe("number");
  });

  it("getActionLog returns a copy — callers can't mutate the buffer", () => {
    logUserAction("doc", "load-project");
    const copy = getActionLog();
    copy.push({ t: 0, kind: "action", cat: "x", name: "y" });
    expect(getActionLog()).toHaveLength(1);
  });

  it("caps the buffer at the max entry count (ring buffer)", () => {
    for (let i = 0; i < 2200; i++) logUserAction("test", `n${i}`);
    const log = getActionLog();
    expect(log.length).toBeLessThanOrEqual(2000);
    // The newest entries survive; the oldest are dropped.
    expect((log[log.length - 1] as ActionEntry).name).toBe("n2199");
    expect(log.some((e) => (e as ActionEntry).name === "n0")).toBe(false);
  });

  it("serializes with metadata + the entries", () => {
    logUserAction("a", "b");
    logUserAction("c", "d");
    const out = serializeActionLog();
    expect(out.entryCount).toBe(2);
    expect(out.entries).toHaveLength(2);
    expect(typeof out.capturedAt).toBe("string");
    expect(out.spanMs).toBeGreaterThanOrEqual(0);
  });
});
