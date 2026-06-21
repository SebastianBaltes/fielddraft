/**
 * ScalarFormulaInput regression test (ticket 6sjq).
 *
 * The bug: a literal numeric field and a formula field were two different
 * `<input>` elements, swapped on every keystroke as the typed text crossed the
 * number⇄formula boundary. Typing a variable name (`Welt`) into a field that
 * held a number remounted the input right after the first letter ("W"), which
 * dropped DOM focus — the user could never finish typing the name and the field
 * appeared to "close". The fix renders ONE persistent input for both modes.
 *
 * These tests assert the contract that makes the focus survive: the exact same
 * DOM element stays mounted across a literal→formula transition (so focus is
 * never lost), and a formula is committed as the kernel's `=…` wire form.
 */

import { describe, expect, it } from "vitest";
import { act } from "react";
import { createRoot } from "react-dom/client";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean })
  .IS_REACT_ACT_ENVIRONMENT = true;

import { ScalarFormulaInput } from "./widgets";

function setNativeValue(el: HTMLInputElement, value: string) {
  const proto = Object.getPrototypeOf(el) as object;
  const setter = Object.getOwnPropertyDescriptor(proto, "value")?.set;
  setter?.call(el, value);
  el.dispatchEvent(new Event("input", { bubbles: true }));
}

describe("ScalarFormulaInput", () => {
  it("keeps the same <input> element (and focus) across literal→formula", () => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    const root = createRoot(host);

    let committed: number | string | null = null;
    // A controlled cell: re-render with whatever was committed, mirroring how
    // the FeatureDialog feeds the new param value straight back in.
    let value: number | string = 50;
    const render = () =>
      act(() => {
        root.render(
          <ScalarFormulaInput
            value={value}
            unit="mm"
            ariaLabel="x"
            onCommit={(next) => {
              committed = next;
              value = next;
            }}
          />,
        );
      });
    render();

    const before = host.querySelector("input") as HTMLInputElement;
    before.focus();
    expect(document.activeElement).toBe(before);

    // Type "W" — the text now reads as a formula, which previously swapped the
    // element. Re-render with the committed value as React would.
    act(() => setNativeValue(before, "W"));
    render();

    const after = host.querySelector("input") as HTMLInputElement;
    // Same element instance ⇒ React did NOT remount ⇒ focus is retained.
    expect(after).toBe(before);
    expect(document.activeElement).toBe(after);
    expect(committed).toBe("=W");

    act(() => root.unmount());
    host.remove();
  });

  it("commits a plain number literal (with unit) as a number", () => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    const root = createRoot(host);

    let committed: number | string | null = null;
    act(() => {
      root.render(
        <ScalarFormulaInput
          value={10}
          unit="mm"
          ariaLabel="x"
          onCommit={(next) => {
            committed = next;
          }}
        />,
      );
    });

    const input = host.querySelector("input") as HTMLInputElement;
    act(() => setNativeValue(input, "25 mm"));
    expect(committed).toBe(25);

    act(() => root.unmount());
    host.remove();
  });

  it("renders a stored formula in its raw =… wire form", () => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    const root = createRoot(host);

    act(() => {
      root.render(
        <ScalarFormulaInput value="=Welt" unit="mm" onCommit={() => {}} />,
      );
    });

    const input = host.querySelector("input") as HTMLInputElement;
    expect(input.value).toBe("=Welt");
    expect(input.className).toContain("is-formula");

    act(() => root.unmount());
    host.remove();
  });
});
