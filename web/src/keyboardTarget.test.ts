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

  // Regression: Monaco's EditContext input is a focusable <div> that is
  // neither TEXTAREA nor contentEditable. Bare-key shortcuts (1/2/3/…)
  // were swallowed while typing in the FieldSkript editor because the
  // guard only checked tag/contentEditable.
  it("matches any element inside a Monaco editor (incl. EditContext div)", () => {
    const editor = document.createElement("div");
    editor.className = "monaco-editor";
    const editContextInput = document.createElement("div"); // not contentEditable
    editor.appendChild(editContextInput);
    expect(isTextEntryTarget(editContextInput)).toBe(true);
    expect(isTextEntryTarget(editor)).toBe(true);
  });
});
