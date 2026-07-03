/**
 * Default per-body part colours (index = bodyIndex % length). Single source
 * of truth for every render path: the SDF material buffers
 * (sdfPipelineCommon/materialBridge), the classic-WebGL bodies
 * (webglBodies) and the screen-space CSG compositor pass.
 */
export const BODY_COLORS: [number, number, number][] = [
  [0.55, 0.65, 0.85],
  [0.65, 0.85, 0.55],
  [0.85, 0.55, 0.65],
  [0.85, 0.75, 0.5],
  [0.55, 0.8, 0.8],
  [0.75, 0.55, 0.85],
  [0.85, 0.85, 0.55],
  [0.55, 0.7, 0.7],
];
