// Backend-aware overlay line materials.
//
// FieldDraft renders two ways: the primary WebGPU pipeline (SDF passes) and a
// classic THREE.WebGLRenderer fallback (render mode `webgl`) for machines
// without a usable WebGPU adapter. The two backends are now mounted in two
// SEPARATE R3F roots (Viewport.tsx WebGLViewport vs the WebGPU <Scene>).
//
// Node materials (`Line2NodeMaterial`, `SpriteNodeMaterial`, … from
// `three/webgpu`) carry no classic GLSL source — `material.vertexShader` is
// undefined. Handing one to the classic `THREE.WebGLRenderer` crashes inside
// `resolveIncludes` ("Cannot read properties of undefined (reading 'replace')",
// tickets n6rs/gsoj). So every overlay that draws a "fat" highlight line must
// pick its material by backend: the fat instanced `Line2NodeMaterial` on
// WebGPU, a plain `THREE.LineBasicMaterial` on WebGL — exactly the split
// BRepEdgeOverlay already used for the base wireframe (ticket jnej).
//
// This module is the ONE place that split lives, so the hover-outline, the
// selected-edge stroke and the body-highlight wireframe stay consistent and no
// future overlay re-introduces the crash by hand.
import { LineSegments2 } from "three/examples/jsm/lines/webgpu/LineSegments2.js";
import { LineSegmentsGeometry } from "three/examples/jsm/lines/LineSegmentsGeometry.js";
import { Line2NodeMaterial } from "three/webgpu";
import * as THREE from "three";

/** `true` when `gl` is the WebGPU renderer, `false` for the classic
 *  `THREE.WebGLRenderer`. Tolerates the `unknown`-typed R3F `gl`. */
export function isWebGPURenderer(gl: unknown): boolean {
  return (gl as { isWebGPURenderer?: boolean }).isWebGPURenderer === true;
}

export interface FatLineStyle {
  color: number;
  /** Screen-space width in px (honoured on WebGPU's instanced line shader;
   *  the WebGL `LineBasicMaterial` always rasterises 1 px, ticket jnej). */
  linewidth: number;
  depthTest?: boolean;
}

/** A matched material PAIR for one overlay line. Exactly one is ever rendered,
 *  decided per object by {@link makeFatLine} — but both are created up front so
 *  the owning component can dispose them in a single effect without caring
 *  which backend is live. */
export interface FatLineMaterials {
  /** Fat instanced material — rendered ONLY on the WebGPU backend. */
  node: Line2NodeMaterial;
  /** Plain classic material — rendered ONLY on the WebGL backend. */
  classic: THREE.LineBasicMaterial;
}

export function createFatLineMaterials(style: FatLineStyle): FatLineMaterials {
  const depthTest = style.depthTest ?? false;
  return {
    node: new Line2NodeMaterial({
      color: style.color,
      linewidth: style.linewidth,
      worldUnits: false,
      depthTest,
      depthWrite: false,
      transparent: true,
    }),
    classic: new THREE.LineBasicMaterial({
      color: style.color,
      depthTest,
      depthWrite: false,
      transparent: true,
    }),
  };
}

/** Build a renderable line-segments object from a flat `[x,y,z, …]` segment
 *  buffer, choosing the geometry + material that the live backend can actually
 *  render: `LineSegments2` + `Line2NodeMaterial` on WebGPU, classic
 *  `THREE.LineSegments` + `LineBasicMaterial` on WebGL. The caller disposes the
 *  returned geometry; the materials are shared (disposed by their owner). */
export function makeFatLine(
  positions: ArrayLike<number>,
  mats: FatLineMaterials,
  webgpu: boolean,
): { object: THREE.Object3D; geometry: { dispose(): void } } {
  const buf =
    positions instanceof Float32Array
      ? positions
      : new Float32Array(positions);
  if (webgpu) {
    const geometry = new LineSegmentsGeometry();
    geometry.setPositions(buf);
    return { object: new LineSegments2(geometry, mats.node), geometry };
  }
  const geometry = new THREE.BufferGeometry();
  geometry.setAttribute("position", new THREE.Float32BufferAttribute(buf, 3));
  return { object: new THREE.LineSegments(geometry, mats.classic), geometry };
}
