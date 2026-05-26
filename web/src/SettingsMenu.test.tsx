import { describe, expect, it, beforeEach, vi } from "vitest";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { SettingsMenu, loadSettings } from "./SettingsMenu";
import type { SettingsState } from "./SettingsMenu";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean })
  .IS_REACT_ACT_ENVIRONMENT = true;

function setup() {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);

  let currentSettings: SettingsState = {
    compact: false,
    units: "mm",
    theme: "light",
    renderMode: "sdf-deferred",
    clickCycleGizmo: true,
  };

  const onChange = vi.fn((next: SettingsState) => {
    currentSettings = next;
    act(() => {
      root.render(
        <SettingsMenu
          store={null}
          state={null}
          settings={currentSettings}
          onSettingsChange={onChange}
        />,
      );
    });
  });

  act(() => {
    root.render(
      <SettingsMenu
        store={null}
        state={null}
        settings={currentSettings}
        onSettingsChange={onChange}
      />,
    );
  });

  return {
    container,
    root,
    onChange,
    get settings() {
      return currentSettings;
    },
    cleanup() {
      act(() => root.unmount());
      container.remove();
    },
  };
}

function click(el: Element) {
  act(() => {
    el.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  });
}

function openSettingsDialog(container: HTMLElement) {
  click(container.querySelector('[data-testid="settings-menu-trigger"]')!);
  click(container.querySelector('[data-testid="menu-settings"]')!);
}

describe("SettingsMenu", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it("renders the burger button", () => {
    const t = setup();
    const trigger = t.container.querySelector(
      '[data-testid="settings-menu-trigger"]',
    );
    expect(trigger).toBeTruthy();
    expect(trigger!.textContent).toContain("☰");
    t.cleanup();
  });

  it("opens popover on click and closes on second click", () => {
    const t = setup();
    const trigger = t.container.querySelector(
      '[data-testid="settings-menu-trigger"]',
    )!;

    expect(
      t.container.querySelector('[data-testid="settings-menu-popover"]'),
    ).toBeNull();

    click(trigger);
    expect(
      t.container.querySelector('[data-testid="settings-menu-popover"]'),
    ).toBeTruthy();

    click(trigger);
    expect(
      t.container.querySelector('[data-testid="settings-menu-popover"]'),
    ).toBeNull();
    t.cleanup();
  });

  it("has menu items for New, Open, Save, Import, Export", () => {
    const t = setup();
    click(t.container.querySelector('[data-testid="settings-menu-trigger"]')!);

    expect(
      t.container.querySelector('[data-testid="menu-new-project"]'),
    ).toBeTruthy();
    expect(
      t.container.querySelector('[data-testid="menu-open"]'),
    ).toBeTruthy();
    expect(
      t.container.querySelector('[data-testid="menu-save"]'),
    ).toBeTruthy();
    expect(
      t.container.querySelector('[data-testid="menu-import"]'),
    ).toBeTruthy();
    expect(
      t.container.querySelector('[data-testid="menu-export"]'),
    ).toBeTruthy();
    t.cleanup();
  });

  it("opens settings dialog from menu", () => {
    const t = setup();
    openSettingsDialog(t.container);

    expect(
      t.container.querySelector('[data-testid="settings-dialog"]'),
    ).toBeTruthy();
    t.cleanup();
  });

  it("toggles compact mode in settings dialog", () => {
    const t = setup();
    openSettingsDialog(t.container);

    const toggle = t.container.querySelector(
      '[data-testid="setting-compact-toggle"]',
    )!;
    expect(toggle.textContent).toBe("Off");

    click(toggle);
    expect(t.onChange).toHaveBeenCalledWith(
      expect.objectContaining({ compact: true }),
    );
    expect(localStorage.getItem("fielddraft.compact")).toBe("true");

    const toggleAfter = t.container.querySelector(
      '[data-testid="setting-compact-toggle"]',
    )!;
    expect(toggleAfter.textContent).toBe("On");
    t.cleanup();
  });

  it("switches units in settings dialog", () => {
    const t = setup();
    openSettingsDialog(t.container);

    click(t.container.querySelector('[data-testid="setting-units-inch"]')!);
    expect(t.onChange).toHaveBeenCalledWith(
      expect.objectContaining({ units: "inch" }),
    );
    expect(localStorage.getItem("fielddraft.units")).toBe("inch");

    click(t.container.querySelector('[data-testid="setting-units-mm"]')!);
    expect(localStorage.getItem("fielddraft.units")).toBe("mm");
    t.cleanup();
  });

  it("switches theme in settings dialog", () => {
    const t = setup();
    openSettingsDialog(t.container);

    click(t.container.querySelector('[data-testid="setting-theme-dark"]')!);
    expect(t.onChange).toHaveBeenCalledWith(
      expect.objectContaining({ theme: "dark" }),
    );
    expect(localStorage.getItem("fielddraft.theme")).toBe("dark");
    t.cleanup();
  });

  it("expands and collapses shortcuts in settings dialog", () => {
    const t = setup();
    openSettingsDialog(t.container);

    expect(
      t.container.querySelector('[data-testid="shortcuts-table"]'),
    ).toBeNull();

    click(
      t.container.querySelector('[data-testid="setting-shortcuts-toggle"]')!,
    );
    expect(
      t.container.querySelector('[data-testid="shortcuts-table"]'),
    ).toBeTruthy();

    click(
      t.container.querySelector('[data-testid="setting-shortcuts-toggle"]')!,
    );
    expect(
      t.container.querySelector('[data-testid="shortcuts-table"]'),
    ).toBeNull();
    t.cleanup();
  });

  it("closes menu on Escape", () => {
    const t = setup();
    click(t.container.querySelector('[data-testid="settings-menu-trigger"]')!);
    expect(
      t.container.querySelector('[data-testid="settings-menu-popover"]'),
    ).toBeTruthy();

    act(() => {
      window.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
      );
    });

    expect(
      t.container.querySelector('[data-testid="settings-menu-popover"]'),
    ).toBeNull();
    t.cleanup();
  });

  it("closes settings dialog via close button", () => {
    const t = setup();
    openSettingsDialog(t.container);

    expect(
      t.container.querySelector('[data-testid="settings-dialog"]'),
    ).toBeTruthy();

    click(
      t.container.querySelector('[data-testid="settings-dialog-close"]')!,
    );

    expect(
      t.container.querySelector('[data-testid="settings-dialog"]'),
    ).toBeNull();
    t.cleanup();
  });
});

describe("loadSettings", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it("returns defaults when localStorage is empty", () => {
    const s = loadSettings();
    expect(s).toEqual({
      compact: false,
      units: "mm",
      theme: "light",
      renderMode: "sdf-deferred",
    });
  });

  it("reads stored values", () => {
    localStorage.setItem("fielddraft.compact", "true");
    localStorage.setItem("fielddraft.units", "inch");
    localStorage.setItem("fielddraft.theme", "dark");

    const s = loadSettings();
    expect(s).toEqual({
      compact: true,
      units: "inch",
      theme: "dark",
      renderMode: "sdf-deferred",
    });
  });

  it("ignores invalid values and falls back to defaults", () => {
    localStorage.setItem("fielddraft.units", "furlongs");
    localStorage.setItem("fielddraft.theme", "neon");

    const s = loadSettings();
    expect(s.units).toBe("mm");
    expect(s.theme).toBe("light");
  });
});
