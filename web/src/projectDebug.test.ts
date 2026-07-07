import { beforeEach, describe, expect, it } from "vitest";
import {
  applyProjectDebug,
  collectProjectDebug,
  getDebugDialogs,
  parseProjectDebug,
  setMesherDialogOpen,
  setMesherPasmDialogOpen,
  setSdfPlaneDialogOpen,
  subscribeDebugDialogs,
} from "./projectDebug";
import { getPartAabbVisible, setPartAabbVisible } from "./partAabbStore";

describe("projectDebug", () => {
  beforeEach(() => {
    localStorage.clear();
    applyProjectDebug(undefined); // closed dialogs, overlay off
  });

  it("collect returns undefined when no debug tool is active", () => {
    expect(collectProjectDebug()).toBeUndefined();
  });

  it("round-trips open dialogs + settings + overlay through a project blob", () => {
    localStorage.setItem(
      "fielddraft.debug.sdfPlane",
      JSON.stringify({ point: [7.5, 21.9, 29.35], halfExtent: 6 }),
    );
    localStorage.setItem(
      "fielddraft.debug.mesher",
      JSON.stringify({ maxDepth: 7, partOpacity: 0.5 }),
    );
    setSdfPlaneDialogOpen(true);
    setMesherDialogOpen(true);
    setPartAabbVisible(true);

    const blob = collectProjectDebug()!;
    expect(blob.sdfPlaneOpen).toBe(true);
    expect(blob.sdfPlane).toMatchObject({ halfExtent: 6 });
    expect(blob.mesherOpen).toBe(true);
    expect(blob.partAabbVisible).toBe(true);

    // Fresh session: nothing open, other settings in the slots.
    applyProjectDebug(undefined);
    localStorage.setItem("fielddraft.debug.sdfPlane", JSON.stringify({ halfExtent: 99 }));
    expect(getDebugDialogs().sdfPlaneOpen).toBe(false);
    expect(getPartAabbVisible()).toBe(false);

    // Apply the (validated) blob — dialogs open, slots restored, overlay on.
    const before = getDebugDialogs().revision;
    applyProjectDebug(parseProjectDebug(JSON.parse(JSON.stringify(blob))));
    expect(getDebugDialogs().sdfPlaneOpen).toBe(true);
    expect(getDebugDialogs().mesherOpen).toBe(true);
    expect(getDebugDialogs().revision).toBe(before + 1);
    expect(getPartAabbVisible()).toBe(true);
    expect(
      JSON.parse(localStorage.getItem("fielddraft.debug.sdfPlane")!),
    ).toMatchObject({ halfExtent: 6 });
  });

  it("apply(undefined) closes everything (state is reproducible both ways)", () => {
    setSdfPlaneDialogOpen(true);
    setPartAabbVisible(true);
    applyProjectDebug(undefined);
    expect(getDebugDialogs().sdfPlaneOpen).toBe(false);
    expect(getPartAabbVisible()).toBe(false);
  });

  it("parseProjectDebug rejects junk without breaking the project", () => {
    expect(parseProjectDebug(null)).toBeUndefined();
    expect(parseProjectDebug("x")).toBeUndefined();
    expect(parseProjectDebug({})).toBeUndefined();
    expect(parseProjectDebug({ sdfPlaneOpen: "yes" })).toBeUndefined();
    expect(parseProjectDebug({ sdfPlane: [1, 2] })).toBeUndefined();
    expect(parseProjectDebug({ mesherOpen: true })).toEqual({ mesherOpen: true });
  });

  it("open-flag changes notify subscribers (dirty tracking hook)", () => {
    let fired = 0;
    const unsub = subscribeDebugDialogs(() => fired++);
    setSdfPlaneDialogOpen(true);
    setSdfPlaneDialogOpen(true); // no-op — must not fire
    setSdfPlaneDialogOpen(false);
    expect(fired).toBe(2);
    unsub();
  });

  it("round-trips the Mesher PASM dialog + settings", () => {
    localStorage.setItem(
      "fielddraft.debug.mesherPasm",
      JSON.stringify({ targetEdgeLength: 3.5, iterations: 60 }),
    );
    setMesherPasmDialogOpen(true);

    const blob = collectProjectDebug()!;
    expect(blob.mesherPasmOpen).toBe(true);
    expect(blob.mesherPasm).toMatchObject({ targetEdgeLength: 3.5 });

    applyProjectDebug(undefined);
    expect(getDebugDialogs().mesherPasmOpen).toBe(false);

    applyProjectDebug(parseProjectDebug(JSON.parse(JSON.stringify(blob))));
    expect(getDebugDialogs().mesherPasmOpen).toBe(true);
    expect(
      JSON.parse(localStorage.getItem("fielddraft.debug.mesherPasm")!),
    ).toMatchObject({ targetEdgeLength: 3.5, iterations: 60 });
  });

  it("Mesher and Mesher PASM are mutually exclusive (shared overlay store)", () => {
    setMesherDialogOpen(true);
    setMesherPasmDialogOpen(true);
    expect(getDebugDialogs().mesherOpen).toBe(false);
    expect(getDebugDialogs().mesherPasmOpen).toBe(true);

    setMesherDialogOpen(true);
    expect(getDebugDialogs().mesherOpen).toBe(true);
    expect(getDebugDialogs().mesherPasmOpen).toBe(false);

    // A blob claiming both keeps only the PASM dialog.
    applyProjectDebug({ mesherOpen: true, mesherPasmOpen: true });
    expect(getDebugDialogs().mesherOpen).toBe(false);
    expect(getDebugDialogs().mesherPasmOpen).toBe(true);
  });
});
