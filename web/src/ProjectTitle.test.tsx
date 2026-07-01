import { describe, expect, it, beforeEach, afterEach, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { ProjectTitle } from "./ProjectTitle";
import type { KernelStore } from "./store";

/**
 * Ticket C — the inline-editable project title next to the logo. Clicking the
 * name swaps in a text input; Enter / blur commits via `store.renameProject`,
 * Escape cancels. Only the rename delegation matters here, so the store is a
 * stub exposing that one method.
 */

(
  globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root;

function makeStore(): {
  store: KernelStore;
  renameProject: ReturnType<typeof vi.fn>;
} {
  const renameProject = vi.fn();
  const store = { renameProject } as unknown as KernelStore;
  return { store, renameProject };
}

function render(store: KernelStore, name: string) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => {
    root.render(<ProjectTitle store={store} name={name} />);
  });
}

function click(testid: string) {
  const el = container.querySelector<HTMLElement>(`[data-testid="${testid}"]`)!;
  act(() => {
    el.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  });
}

function setValue(testid: string, value: string) {
  const el = container.querySelector<HTMLInputElement>(
    `[data-testid="${testid}"]`,
  )!;
  Object.getOwnPropertyDescriptor(
    HTMLInputElement.prototype,
    "value",
  )!.set!.call(el, value);
  act(() => {
    el.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

function key(testid: string, k: string) {
  const el = container.querySelector<HTMLElement>(`[data-testid="${testid}"]`)!;
  act(() => {
    el.dispatchEvent(
      new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true }),
    );
  });
}

function blur(testid: string) {
  const el = container.querySelector<HTMLElement>(`[data-testid="${testid}"]`)!;
  // React delegates `onBlur` to the bubbling `focusout` event, not the native
  // non-bubbling `blur`.
  act(() => {
    el.dispatchEvent(new FocusEvent("focusout", { bubbles: true }));
  });
}

beforeEach(() => {
  vi.restoreAllMocks();
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

describe("ProjectTitle", () => {
  it("shows the project name as a clickable button", () => {
    const { store } = makeStore();
    render(store, "Gearbox");
    const btn = container.querySelector('[data-testid="project-title"]');
    expect(btn?.textContent).toBe("Gearbox");
    expect(
      container.querySelector('[data-testid="project-title-input"]'),
    ).toBeNull();
  });

  it("clicking the name reveals an input seeded with the name", () => {
    const { store } = makeStore();
    render(store, "Gearbox");
    click("project-title");
    const input = container.querySelector<HTMLInputElement>(
      '[data-testid="project-title-input"]',
    );
    expect(input).not.toBeNull();
    expect(input!.value).toBe("Gearbox");
  });

  it("Enter commits the edited name via renameProject", () => {
    const { store, renameProject } = makeStore();
    render(store, "Gearbox");
    click("project-title");
    setValue("project-title-input", "Gearbox v2");
    key("project-title-input", "Enter");
    expect(renameProject).toHaveBeenCalledWith("Gearbox v2");
    // Back to the read-only button.
    expect(
      container.querySelector('[data-testid="project-title-input"]'),
    ).toBeNull();
  });

  it("blur commits the edited name", () => {
    const { store, renameProject } = makeStore();
    render(store, "Gearbox");
    click("project-title");
    setValue("project-title-input", "Renamed");
    blur("project-title-input");
    expect(renameProject).toHaveBeenCalledWith("Renamed");
  });

  it("Escape cancels without renaming", () => {
    const { store, renameProject } = makeStore();
    render(store, "Gearbox");
    click("project-title");
    setValue("project-title-input", "Discarded");
    key("project-title-input", "Escape");
    expect(renameProject).not.toHaveBeenCalled();
    // Input closed, original name still shown.
    expect(
      container.querySelector('[data-testid="project-title-input"]'),
    ).toBeNull();
    expect(
      container.querySelector('[data-testid="project-title"]')?.textContent,
    ).toBe("Gearbox");
  });
});
