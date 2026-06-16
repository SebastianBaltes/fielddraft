import * as THREE from "three";
import { OVERLAY_LAYER } from "./viewportSdfOverlays";

// Sketch vertex DOTS as billboarded, screen-constant circle MESHES (ticket
// tdq2). The earlier THREE.Points / PointsNodeMaterial dots (tickets n6hg/b78p)
// render NOTHING in this app's three/webgpu overlay pass (`renderOverlayLayer`
// in viewportSdfOverlays.tsx) — confirmed live: a Points object exists in the
// scene, correct geometry, on the overlay layer, but draws nothing. Meshes DO
// render in that pass (the sketch region fill is one, ticket jjmv). So every dot
// is now a tiny CircleGeometry mesh.
//
// "Screen-constant" + "camera-facing" is solved exactly like the label sprites
// in viewportOverlays.tsx (`SketchLabels3D`): on each render we scale the dot so
// its diameter is a fixed number of CSS pixels regardless of zoom, and copy the
// camera orientation so the disk always faces the viewer. One InstancedMesh
// holds all vertices of a sketch; each instance is scaled per its own distance
// to the camera so a dot near the camera and one far away read the same size.

/** Segment count of the dot circle — smooth enough at any on-screen size. */
const DOT_SEGMENTS = 24;

/** The slice of the renderer we need — both WebGPU and WebGL expose `getSize`. */
interface RendererWithSize {
  getSize(target: THREE.Vector2): THREE.Vector2;
}

/**
 * A set of filled-circle vertex markers sharing one colour, drawn as a single
 * InstancedMesh on the OVERLAY_LAYER. `diameterPx` is the on-screen diameter in
 * CSS pixels, held constant across zoom via a per-render rescale (see `attach`).
 */
export class SketchDots {
  readonly mesh: THREE.InstancedMesh;
  // Classic MeshBasicMaterial (not the node variant) so the same dots render on
  // BOTH the WebGPU overlay pass and the classic WebGL fallback canvas — like
  // the committed sketch region fill (ticket jjmv).
  private readonly material: THREE.MeshBasicMaterial;
  private readonly diameterPx: number;
  /** World-space centre of each dot (3 floats per dot). */
  private readonly centers: Float32Array;
  private readonly count: number;
  // Scratch reused every render to avoid per-frame allocations.
  private readonly tmpMat = new THREE.Matrix4();
  private readonly tmpPos = new THREE.Vector3();
  private readonly tmpQuat = new THREE.Quaternion();
  private readonly tmpScale = new THREE.Vector3();
  private readonly camPos = new THREE.Vector3();

  /**
   * @param positions flat world-space xyz triples (as produced by
   *        `collectSketchSessionGeometry(...).points`).
   * @param colorHex  dot colour.
   * @param diameterPx on-screen diameter in CSS pixels (constant at any zoom).
   * @param renderOrder draw order — keep above the fat lines (≈998).
   */
  constructor(positions: number[], colorHex: number, diameterPx: number, renderOrder: number) {
    this.count = Math.floor(positions.length / 3);
    this.centers = new Float32Array(positions);
    this.diameterPx = diameterPx;

    // Unit-radius circle; the per-render rescale turns the radius into pixels.
    const geom = new THREE.CircleGeometry(1, DOT_SEGMENTS);
    this.material = new THREE.MeshBasicMaterial({
      color: colorHex,
      transparent: true,
      depthTest: false,
      depthWrite: false,
      side: THREE.DoubleSide,
    });
    this.mesh = new THREE.InstancedMesh(geom, this.material, Math.max(this.count, 1));
    this.mesh.frustumCulled = false;
    this.mesh.renderOrder = renderOrder;
    // `enable` (not `set`) so the dot draws in BOTH the base render pass and the
    // overlay-only pass (`camera.layers.set(OVERLAY_LAYER)`) of renderOverlayLayer
    // — exactly like the sketch lines/fill, which use `enable` too.
    this.mesh.layers.enable(OVERLAY_LAYER);
    this.mesh.count = this.count;

    // Orient + size every instance just before the GPU consumes this mesh, so
    // the dots stay camera-facing and screen-constant during a camera move.
    this.mesh.onBeforeRender = (renderer, _scene, camera) => {
      this.update(renderer as unknown as RendererWithSize, camera);
    };
  }

  /** Recolour in place (DOF status change) — no geometry churn. */
  setColor(colorHex: number): void {
    this.material.color.setHex(colorHex);
  }

  private update(renderer: RendererWithSize, camera: THREE.Camera): void {
    if (this.count === 0) return;
    const size = renderer.getSize(new THREE.Vector2());
    const viewportH = size.y || 1;
    const persp = camera as THREE.PerspectiveCamera;
    // World units per screen pixel at distance d (perspective): the view frustum
    // is 2·d·tan(fov/2) tall, spread over `viewportH` pixels. (Same derivation
    // as the label sprites in viewportOverlays.tsx.) For an ortho camera the
    // scale is depth-independent.
    const tanHalfFov = persp.isPerspectiveCamera
      ? Math.tan(persp.fov * 0.5 * THREE.MathUtils.DEG2RAD)
      : 0;
    const radiusPx = this.diameterPx * 0.5;
    camera.getWorldPosition(this.camPos);
    // Face the camera: a CircleGeometry lies in the XY plane facing +Z, so the
    // camera's own orientation aligns it to the view plane (billboard).
    this.tmpQuat.copy(camera.quaternion);

    for (let i = 0; i < this.count; i++) {
      this.tmpPos.set(this.centers[i * 3], this.centers[i * 3 + 1], this.centers[i * 3 + 2]);
      let radiusWorld: number;
      if (persp.isPerspectiveCamera) {
        const dist = this.camPos.distanceTo(this.tmpPos);
        const worldPerPx = (2 * dist * tanHalfFov) / viewportH;
        radiusWorld = radiusPx * worldPerPx;
      } else {
        const ortho = camera as THREE.OrthographicCamera;
        const worldPerPx = (ortho.top - ortho.bottom) / ortho.zoom / viewportH;
        radiusWorld = radiusPx * worldPerPx;
      }
      this.tmpScale.setScalar(radiusWorld);
      this.tmpMat.compose(this.tmpPos, this.tmpQuat, this.tmpScale);
      this.mesh.setMatrixAt(i, this.tmpMat);
    }
    this.mesh.instanceMatrix.needsUpdate = true;
  }

  dispose(): void {
    this.mesh.geometry.dispose();
    this.material.dispose();
    this.mesh.dispose();
  }
}

/** Convenience: build a {@link SketchDots} or `null` when there are no points. */
export function makeSketchDots(
  positions: number[] | null | undefined,
  colorHex: number,
  diameterPx: number,
  renderOrder: number,
): SketchDots | null {
  if (!positions || positions.length === 0) return null;
  return new SketchDots(positions, colorHex, diameterPx, renderOrder);
}
