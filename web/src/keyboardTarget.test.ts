import { describe, it, expect } from "vitest";
import { isTextEntryTarget } from "./keyboardTarget";

describe("isTextEntryTarget", () => {
  it("matches plain form text surfaces", () => {
    expect(isTextEntryTarget(document.createElement("input"))).toBe(true);
    expect(isTextEntryTarget(document.createElement("textarea"))).toBe(true);
    const ce = document.createElement("div");
    ce.setAttribute("contenteditable", "true");
    expect(isTextEntryTarget(ce)).toBe(true);
  });

  it("does not match an ordinary element", () => {
    expect(isTextEntryTarget(document.createElement("div"))).toBe(false);
    expect(isTextEntryTarget(null)).toBe(false);
  });

  // The script editor (Monaco) is NOT this helper's job: its EditContext
  // input is a focusable <div> that is neither TEXTAREA nor
  // contentEditable, so it reads as a non-text element here. Bare-key
  // shortcuts are kept out of it by gating on the active tab kind in
  // App.tsx, not by recognising the editor surface.
  it("does not match a Monaco-style EditContext div", () => {
    const editor = document.createElement("div");
    editor.className = "monaco-editor";
    const editContextInput = document.createElement("div"); // not contentEditable
    editor.appendChild(editContextInput);
    expect(isTextEntryTarget(editContextInput)).toBe(false);
  });
});
