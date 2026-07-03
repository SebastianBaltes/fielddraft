/**
 * Tool-name → controller registry (architecture review, Befund 2).
 *
 * `SketchCanvas2d` resolves the active controller here instead of running
 * if-chains over tool names. Select mode (no tool) and the constraint tools
 * (`sketch_constraint_*`, incl. the Dimension/Coincident sentinels) don't
 * live in the record — they are resolved explicitly by the canvas because
 * their dispatch happens at a different point of the pointer flow.
 */

import type { SketchToolController } from "./toolTypes";
import { DRAW_TOOL_CONTROLLERS } from "./drawTools";
import { lineTool } from "./lineTool";
import { splineTool } from "./splineTool";
import { arcTool } from "./arcTool";
import { circle3pTool, rect3pTool } from "./threePointTools";

export const SKETCH_TOOL_REGISTRY: Record<string, SketchToolController> = {
  // Controllers register here as the migration proceeds (steps 2–5).
  ...DRAW_TOOL_CONTROLLERS,
  sketch_line: lineTool,
  sketch_spline: splineTool,
  sketch_arc: arcTool,
  sketch_circle_3p: circle3pTool,
  sketch_rectangle_3p: rect3pTool,
};

/** The controller for a drawing/editing tool name, or null while the tool
 *  is still handled by the canvas's legacy if-chains. */
export function controllerForTool(
  tool: string | null,
): SketchToolController | null {
  if (tool === null) return null;
  return SKETCH_TOOL_REGISTRY[tool] ?? null;
}
