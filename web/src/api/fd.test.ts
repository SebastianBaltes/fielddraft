import { describe, expect, it } from "vitest";
import { createFdApi } from "./fd";
import type { Kernel, FeatureSummary } from "../kernel";

/**
 * Minimal fake kernel for testing the FdApi façade without
 * loading the WASM binary. Only implements the three methods
 * `FdApi` touches; everything else is left out and TypeScript
 * won't let the test call the missing bits by accident because
 * the cast is tightly scoped.
 */
function fakeKernel(): Kernel {
  const stubSchemas: Record<string, unknown> = {
    box: {
      type: "box",
      params: [
        {
          key: "half_extents",
          name: "Half extents",
          type: "vec3",
          required: true,
        },
      ],
    },
  };
  const features: FeatureSummary[] = [
    {
      id: 1,
      name: "Box 1",
      op: "union",
      kind: "box",
      translate: [0, 0, 0],
      scale: 1,
    },
  ];
  return {
    listFeatureTypes: () => ["box", "cylinder", "extrude", "sphere"],
    schemaForType: (name: string) => {
      const s = stubSchemas[name];
      if (!s) throw new Error(`unknown type: ${name}`);
      return s;
    },
    listFeatures: () => features,
  } as unknown as Kernel;
}

describe("createFdApi", () => {
  it("listFeatureTypes passes through the kernel result", () => {
    const api = createFdApi(fakeKernel());
    expect(api.listFeatureTypes()).toEqual([
      "box",
      "cylinder",
      "extrude",
      "sphere",
    ]);
  });

  it("schemaForType parses the kernel result into a widget-aware schema", () => {
    const api = createFdApi(fakeKernel());
    const schema = api.schemaForType("box");
    expect(schema.type).toBe("box");
    expect(schema.params).toHaveLength(1);
    expect(schema.params[0].widget).toBe("vec3");
    expect(schema.params[0].key).toBe("half_extents");
  });

  it("schemaForType propagates kernel errors for unknown types", () => {
    const api = createFdApi(fakeKernel());
    expect(() => api.schemaForType("torus")).toThrow(/unknown type/);
  });

  it("listFeatures returns the kernel's feature-spine summaries", () => {
    const api = createFdApi(fakeKernel());
    const features = api.listFeatures();
    expect(features).toHaveLength(1);
    expect(features[0].name).toBe("Box 1");
    expect(features[0].kind).toBe("box");
  });
});
