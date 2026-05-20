/**
 * K7.21a: SettingsMenu unit tests.
 *
 * Tests cover:
 * - Popover opens/closes on burger click
 * - Compact toggle persists to localStorage
 * - Units segmented control persists
 * - Theme segmented control persists
 * - Shortcuts section expand/collapse
 * - Close on Escape
 * - loadSettings reads from localStorage correctly
 */

import { describe, expect, it, beforeEach, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
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
    renderMode: "dc",
  };

  const onChange = vi.fn((next: SettingsState) => {
    currentSettings = next;
    // Re-render with new settings
    act(() => {
      root.render(
        <SettingsMenu settings={currentSettings} onSettingsChange={onChange} />,
      );
    });
  });

  act(() => {
    root.render(
      <SettingsMenu settings={currentSettings} onSettingsChange={onChange} />,
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

describe("SettingsMenu", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it("renders the burger button", () => {
    const t = setup();
    const trigger = t.container.querySelector('[data-testid="settings-menu-trigger"]');
    expect(trigger).toBeTruthy();
    expect(trigger!.textContent).toContain("\u2630"); // hamburger char
    t.cleanup();
  });

  it("opens popover on click and closes on second click", () => {
    const t = setup();
    const trigger = t.container.querySelector('[data-testid="settings-menu-trigger"]')!;

    // Initially closed
    expect(t.container.querySelector('[data-testid="settings-menu-popover"]')).toBeNull();

    // Open
    click(trigger);
    expect(t.container.querySelector('[data-testid="settings-menu-popover"]')).toBeTruthy();

    // Close
    click(trigger);
    expect(t.container.querySelector('[data-testid="settings-menu-popover"]')).toBeNull();
    t.cleanup();
  });

  it("toggles compact mode and persists to localStorage", () => {
    const t = setup();
    const trigger = t.container.querySelector('[data-testid="settings-menu-trigger"]')!;
    click(trigger);

    const toggle = t.container.querySelector('[data-testid="setting-compact-toggle"]')!;
    expect(toggle.textContent).toBe("Off");

    // Turn on
    click(toggle);
    expect(t.onChange).toHaveBeenCalledWith(
      expect.objectContaining({ compact: true }),
    );
    expect(localStorage.getItem("fielddraft.compact")).toBe("true");

    // The re-render shows "On"
    const toggleAfter = t.container.querySelector('[data-testid="setting-compact-toggle"]')!;
    expect(toggleAfter.textContent).toBe("On");
    t.cleanup();
  });

  it("switches units and persists to localStorage", () => {
    const t = setup();
    click(t.container.querySelector('[data-testid="settings-menu-trigger"]')!);

    const inchBtn = t.container.querySelector('[data-testid="setting-units-inch"]')!;
    click(inchBtn);

    expect(t.onChange).toHaveBeenCalledWith(
      expect.objectContaining({ units: "inch" }),
    );
    expect(localStorage.getItem("fielddraft.units")).toBe("inch");

    // Switch back to mm
    const mmBtn = t.container.querySelector('[data-testid="setting-units-mm"]')!;
    click(mmBtn);
    expect(localStorage.getItem("fielddraft.units")).toBe("mm");
    t.cleanup();
  });

  it("switches theme and persists to localStorage", () => {
    const t = setup();
    click(t.container.querySelector('[data-testid="settings-menu-trigger"]')!);

    const darkBtn = t.container.querySelector('[data-testid="setting-theme-dark"]')!;
    click(darkBtn);

    expect(t.onChange).toHaveBeenCalledWith(
      expect.objectContaining({ theme: "dark" }),
    );
    expect(localStorage.getItem("fielddraft.theme")).toBe("dark");
    t.cleanup();
  });

  it("expands and collapses the shortcuts section", () => {
    const t = setup();
    click(t.container.querySelector('[data-testid="settings-menu-trigger"]')!);

    // Shortcuts collapsed by default
    expect(t.container.querySelector('[data-testid="shortcuts-table"]')).toBeNull();

    // Expand
    click(t.container.querySelector('[data-testid="setting-shortcuts-toggle"]')!);
    expect(t.container.querySelector('[data-testid="shortcuts-table"]')).toBeTruthy();

    // Collapse
    click(t.container.querySelector('[data-testid="setting-shortcuts-toggle"]')!);
    expect(t.container.querySelector('[data-testid="shortcuts-table"]')).toBeNull();
    t.cleanup();
  });

  it("closes on Escape keydown", () => {
    const t = setup();
    click(t.container.querySelector('[data-testid="settings-menu-trigger"]')!);
    expect(t.container.querySelector('[data-testid="settings-menu-popover"]')).toBeTruthy();

    act(() => {
      window.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
      );
    });

    expect(t.container.querySelector('[data-testid="settings-menu-popover"]')).toBeNull();
    t.cleanup();
  });
});

describe("loadSettings", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it("returns defaults when localStorage is empty", () => {
    const s = loadSettings();
    expect(s).toEqual({ compact: false, units: "mm", theme: "light", renderMode: "dc" });
  });

  it("reads stored values", () => {
    localStorage.setItem("fielddraft.compact", "true");
    localStorage.setItem("fielddraft.units", "inch");
    localStorage.setItem("fielddraft.theme", "dark");

    const s = loadSettings();
    expect(s).toEqual({ compact: true, units: "inch", theme: "dark", renderMode: "dc" });
  });

  it("ignores invalid values and falls back to defaults", () => {
    localStorage.setItem("fielddraft.units", "furlongs");
    localStorage.setItem("fielddraft.theme", "neon");

    const s = loadSettings();
    expect(s.units).toBe("mm");
    expect(s.theme).toBe("light");
  });
});
