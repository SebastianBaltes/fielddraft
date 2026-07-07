import { describe, expect, it } from "vitest";
import { act } from "react";
import { createRoot } from "react-dom/client";
import {
  NumberInput,
  parseRelaxedNumber,
  clampNumber,
} from "./NumberInput";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean })
  .IS_REACT_ACT_ENVIRONMENT = true;

function setNativeValue(el: HTMLInputElement, value: string) {
  const proto = Object.getPrototypeOf(el) as object;
  const setter = Object.getOwnPropertyDescriptor(proto, "value")?.set;
  setter?.call(el, value);
  el.dispatchEvent(new Event("input", { bubbles: true }));
}

describe("parseRelaxedNumber", () => {
  it("returns null for empty / intermediate / invalid input", () => {
    for (const s of ["", "   ", "-", "+", ".", "1e", "abc", "Infinity"]) {
      expect(parseRelaxedNumber(s)).toBeNull();
    }
  });

  it("parses valid and mid-typing-but-numeric forms", () => {
    expect(parseRelaxedNumber("0")).toBe(0);
    expect(parseRelaxedNumber("-5")).toBe(-5);
    expect(parseRelaxedNumber("1.")).toBe(1); // trailing dot still reads as 1
    expect(parseRelaxedNumber(".5")).toBe(0.5);
    expect(parseRelaxedNumber("1e3")).toBe(1000);
    expect(parseRelaxedNumber("  2 ")).toBe(2);
  });

  it("accepts a decimal comma", () => {
    expect(parseRelaxedNumber("0,3")).toBe(0.3);
    expect(parseRelaxedNumber("-1,25")).toBe(-1.25);
  });
});

describe("clampNumber", () => {
  it("clamps to the bounds", () => {
    expect(clampNumber(7, { min: 0, max: 5 })).toBe(5);
    expect(clampNumber(-2, { min: 0, max: 5 })).toBe(0);
    expect(clampNumber(3, { min: 0, max: 5 })).toBe(3);
  });
  it("rounds when integer", () => {
    expect(clampNumber(2.7, { integer: true })).toBe(3);
    expect(clampNumber(2.7, { min: 5, integer: true })).toBe(5);
  });
});

// ── component behaviour ──────────────────────────────────────────────
function mount(props: Parameters<typeof NumberInput>[0]) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  const render = (p: Parameters<typeof NumberInput>[0]) =>
    act(() => root.render(<NumberInput {...p} />));
  render(props);
  const input = host.querySelector("input") as HTMLInputElement;
  return {
    input,
    render,
    cleanup: () => {
      act(() => root.unmount());
      host.remove();
    },
  };
}

describe("NumberInput", () => {
  it("allows a lone '-' mid-typing without committing or resetting", () => {
    let committed: number | null = null;
    const { input, cleanup } = mount({
      value: 4,
      onChange: (n) => (committed = n),
    });
    act(() => input.focus());
    act(() => setNativeValue(input, "-"));
    // The field keeps "-", and nothing was committed (no reset to 0).
    expect(input.value).toBe("-");
    expect(committed).toBeNull();
    act(() => setNativeValue(input, "-5"));
    expect(committed).toBe(-5);
    cleanup();
  });

  it("commits a decimal comma value", () => {
    let committed: number | null = null;
    const { input, cleanup } = mount({
      value: 0,
      onChange: (n) => (committed = n),
    });
    act(() => input.focus());
    act(() => setNativeValue(input, "0,3"));
    expect(committed).toBe(0.3);
    cleanup();
  });

  it("reverts to the last value when blurred while unparseable", () => {
    let committed: number | null = null;
    const { input, cleanup } = mount({
      value: 7,
      onChange: (n) => (committed = n),
    });
    act(() => input.focus());
    act(() => setNativeValue(input, "-"));
    act(() => input.blur());
    expect(input.value).toBe("7"); // reverted
    expect(committed).toBeNull(); // never committed a bogus value
    cleanup();
  });

  it("clamps to [min,max] and rounds integers on commit", () => {
    let committed: number | null = null;
    const { input, cleanup } = mount({
      value: 6,
      min: 3,
      max: 12,
      integer: true,
      onChange: (n) => (committed = n),
    });
    act(() => input.focus());
    act(() => setNativeValue(input, "1")); // below min
    expect(committed).toBe(3);
    act(() => setNativeValue(input, "2.7")); // rounds to 3
    expect(committed).toBe(3);
    cleanup();
  });

  it("commitOnChange=false commits only on blur, not per keystroke", () => {
    const calls: number[] = [];
    const { input, cleanup } = mount({
      value: 10,
      commitOnChange: false,
      onChange: (n) => calls.push(n),
    });
    act(() => input.focus());
    act(() => setNativeValue(input, "42"));
    expect(calls).toEqual([]); // nothing yet
    act(() => input.blur());
    expect(calls).toEqual([42]);
    cleanup();
  });
});
