import { afterEach, describe, expect, it, vi } from "vitest";
import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import { useMediaQuery } from "./useMediaQuery";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean })
  .IS_REACT_ACT_ENVIRONMENT = true;

/**
 * jsdom ships no `matchMedia`, so we install a controllable fake that
 * records its listener and lets the test flip the match and fire `change`.
 */
function installMatchMedia(initial: boolean) {
  let matches = initial;
  let listener: (() => void) | null = null;
  const mql = {
    get matches() {
      return matches;
    },
    addEventListener: (_type: string, cb: () => void) => {
      listener = cb;
    },
    removeEventListener: () => {
      listener = null;
    },
  };
  window.matchMedia = vi.fn().mockReturnValue(mql) as unknown as typeof window.matchMedia;
  return {
    set(next: boolean) {
      matches = next;
      listener?.();
    },
    hasListener: () => listener !== null,
  };
}

/** Render the hook into a throwaway root and expose its latest value. */
function renderQuery(query: string) {
  const container = document.createElement("div");
  const root = createRoot(container);
  let value = false;
  function Probe() {
    value = useMediaQuery(query);
    return null;
  }
  act(() => {
    root.render(createElement(Probe));
  });
  return {
    get value() {
      return value;
    },
    unmount: () => act(() => root.unmount()),
  };
}

afterEach(() => {
  // @ts-expect-error — tear the fake back down between tests.
  delete window.matchMedia;
  vi.restoreAllMocks();
});

describe("useMediaQuery", () => {
  it("returns false when matchMedia is unavailable", () => {
    const probe = renderQuery("(max-width: 768px)");
    expect(probe.value).toBe(false);
    probe.unmount();
  });

  it("reflects the initial match", () => {
    installMatchMedia(true);
    const probe = renderQuery("(max-width: 768px)");
    expect(probe.value).toBe(true);
    probe.unmount();
  });

  it("updates when the query result changes", () => {
    const ctl = installMatchMedia(false);
    const probe = renderQuery("(max-width: 768px)");
    expect(probe.value).toBe(false);
    act(() => ctl.set(true));
    expect(probe.value).toBe(true);
    probe.unmount();
  });

  it("detaches its listener on unmount", () => {
    const ctl = installMatchMedia(true);
    const probe = renderQuery("(max-width: 768px)");
    expect(ctl.hasListener()).toBe(true);
    probe.unmount();
    expect(ctl.hasListener()).toBe(false);
  });
});
