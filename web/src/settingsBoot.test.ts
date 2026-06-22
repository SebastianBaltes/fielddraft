import { describe, it, expect, beforeEach } from "vitest";
import { loadSettings, setBootSettings } from "./SettingsMenu";

// The ClaudeDraft "open ticket" deep-link stages a settings.json snapshot as a
// ONE-SHOT, in-memory override of the first loadSettings() (React mount) —
// never written to localStorage (origin-wide, would clobber the user's tabs).
describe("setBootSettings (deep-link override)", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it("overrides the next loadSettings() then reverts (one-shot)", () => {
    expect(loadSettings().renderMode).toBe("sdf-deferred"); // default

    setBootSettings({ renderMode: "webgl", controlsPreset: "freecad" });

    const first = loadSettings();
    expect(first.renderMode).toBe("webgl");
    expect(first.controlsPreset).toBe("freecad");

    // One-shot: the override is consumed, the next read is pure defaults again.
    expect(loadSettings().renderMode).toBe("sdf-deferred");
  });

  it("never writes the override to localStorage", () => {
    setBootSettings({ renderMode: "webgl" });
    loadSettings();
    expect(localStorage.getItem("fielddraft.renderMode")).toBeNull();
  });

  it("ignores unknown keys", () => {
    setBootSettings({ bogusKey: 42, renderMode: "sdf-vm" });
    const s = loadSettings() as unknown as Record<string, unknown>;
    expect(s.bogusKey).toBeUndefined();
    expect(s.renderMode).toBe("sdf-vm");
  });
});
