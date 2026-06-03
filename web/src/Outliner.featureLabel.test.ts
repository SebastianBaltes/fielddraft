import { describe, it, expect } from "vitest";
import { kindGlyph, prettifyFeatureName } from "./Outliner";

// A FieldSkript instance reports its `kind` as the script id (e.g. "script_1").
// The display name + icon live in the script record, surfaced via
// `state.scriptTypes`. Regression guard for the tree showing "? script_1 1"
// instead of "⬡ My Feature 1".
const SCRIPT_TYPES = [{ id: "script_1", name: "My Feature", icon: "⬡" }];

describe("kindGlyph", () => {
  it("uses the script's icon for a FieldSkript instance kind", () => {
    expect(kindGlyph("script_1", SCRIPT_TYPES)).toBe("⬡");
  });

  it("falls back to '?' for an unknown kind with no matching script type", () => {
    expect(kindGlyph("script_1", [])).toBe("?");
    expect(kindGlyph("totally_unknown")).toBe("?");
  });

  it("still resolves built-in feature glyphs", () => {
    // built-ins resolve via FEATURE_TYPE_META, not scriptTypes
    expect(kindGlyph("sketch", SCRIPT_TYPES)).not.toBe("?");
  });
});

describe("prettifyFeatureName", () => {
  it("substitutes the script's display name and keeps the instance suffix", () => {
    expect(prettifyFeatureName("script_1 1", "script_1", SCRIPT_TYPES)).toBe("My Feature 1");
    expect(prettifyFeatureName("script_1 7", "script_1", SCRIPT_TYPES)).toBe("My Feature 7");
  });

  it("uses the bare display name when the instance name equals the kind", () => {
    expect(prettifyFeatureName("script_1", "script_1", SCRIPT_TYPES)).toBe("My Feature");
  });

  it("returns the raw name when no script type matches and no built-in label exists", () => {
    expect(prettifyFeatureName("script_1 1", "script_1", [])).toBe("script_1 1");
  });
});
