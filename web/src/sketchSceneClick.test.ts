import { describe, it, expect } from "vitest";
import {
  routeSketchSceneClick,
  sceneClickMaySelectFeatures,
  type SketchSceneContext,
} from "./sketchSceneClick";

const ctx = (over: Partial<SketchSceneContext>): SketchSceneContext => ({
  sketchActive: false,
  planeQueryArmed: false,
  projectToolActive: false,
  ...over,
});

describe("routeSketchSceneClick", () => {
  it("no sketch session → default viewport routing", () => {
    expect(routeSketchSceneClick(ctx({ sketchActive: false }))).toBe("default");
  });

  it("no session: armed pickers / tools do not change the default routing", () => {
    // Outside a sketch a query picker is a normal feature-dialog picker; the
    // sketch context flags are irrelevant when no session is open.
    expect(
      routeSketchSceneClick(
        ctx({ sketchActive: false, planeQueryArmed: true }),
      ),
    ).toBe("default");
    expect(
      routeSketchSceneClick(
        ctx({ sketchActive: false, projectToolActive: true }),
      ),
    ).toBe("default");
  });

  it("sketch active, plane-query armed → plane-pick", () => {
    expect(
      routeSketchSceneClick(ctx({ sketchActive: true, planeQueryArmed: true })),
    ).toBe("plane-pick");
  });

  it("sketch active, Project tool on → project-pick", () => {
    expect(
      routeSketchSceneClick(
        ctx({ sketchActive: true, projectToolActive: true }),
      ),
    ).toBe("project-pick");
  });

  it("plane-query wins over the Project tool when both somehow hold", () => {
    expect(
      routeSketchSceneClick(
        ctx({
          sketchActive: true,
          planeQueryArmed: true,
          projectToolActive: true,
        }),
      ),
    ).toBe("plane-pick");
  });

  it("sketch active, no plane-query, no Project tool → inert", () => {
    // The polygon / line / circle / select etc. case: the scene is dead.
    expect(routeSketchSceneClick(ctx({ sketchActive: true }))).toBe("inert");
  });
});

describe("sceneClickMaySelectFeatures", () => {
  it("only true outside a sketch session", () => {
    expect(sceneClickMaySelectFeatures(ctx({ sketchActive: false }))).toBe(
      true,
    );
  });

  it("false in every sketch context — the session cannot be deselected by a scene click", () => {
    expect(
      sceneClickMaySelectFeatures(ctx({ sketchActive: true })),
    ).toBe(false);
    expect(
      sceneClickMaySelectFeatures(
        ctx({ sketchActive: true, planeQueryArmed: true }),
      ),
    ).toBe(false);
    expect(
      sceneClickMaySelectFeatures(
        ctx({ sketchActive: true, projectToolActive: true }),
      ),
    ).toBe(false);
  });
});
