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
  const stubSketchSchemas: Record<string, unknown> = {
    sketch_point: {
      type: "sketch_point",
      params: [
        {
          key: "at",
          name: "At",
          type: "vec2",
          required: true,
        },
      ],
    },
    sketch_line: {
      type: "sketch_line",
      params: [
        { key: "start", name: "Start", type: "vec2", required: true },
        { key: "end", name: "End", type: "vec2", required: true },
      ],
    },
  };
  const features: FeatureSummary[] = [
    {
      id: 1,
      name: "Box 1",
      op: "new",
      kind: "box",
      translate: [0, 0, 0],
      rotation: [0, 0, 0, 1],
      scale: [1, 1, 1],
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
    listSketchToolTypes: () => ["sketch_line", "sketch_point"],
    schemaForSketchToolType: (name: string) => {
      const s = stubSketchSchemas[name];
      if (!s) throw new Error(`unknown sketch tool: ${name}`);
      return s;
    },
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

  it("listSketchToolTypes passes through the kernel result", () => {
    const api = createFdApi(fakeKernel());
    expect(api.listSketchToolTypes()).toEqual(["sketch_line", "sketch_point"]);
  });

  it("schemaForSketchToolType parses a sketch-tool schema with a vec2 widget", () => {
    const api = createFdApi(fakeKernel());
    const schema = api.schemaForSketchToolType("sketch_point");
    expect(schema.type).toBe("sketch_point");
    expect(schema.params).toHaveLength(1);
    expect(schema.params[0].widget).toBe("vec2");
    expect(schema.params[0].key).toBe("at");
  });

  it("schemaForSketchToolType propagates kernel errors for unknown sketch tools", () => {
    const api = createFdApi(fakeKernel());
    expect(() => api.schemaForSketchToolType("sketch_torus")).toThrow(
      /unknown sketch tool/,
    );
  });
});
