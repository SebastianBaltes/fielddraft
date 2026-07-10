/**
 * Offscreen project-thumbnail capture (ticket 14cff61).
 *
 * The live viewport renders through WebGPU (the SDF/deferred pipeline), whose
 * canvas cannot be read back reliably (`toDataURL`/`drawImage` came back blank
 * or froze). Instead we render the current bodies into a DEDICATED, throwaway
 * `THREE.WebGLRenderer` with `preserveDrawingBuffer: true` — a well-trodden,
 * reliable path for `canvas.toBlob` — fully isolated from the live device.
 *
 * The scene is rebuilt fresh from the kernel body tessellation (`meshedBodies`,
 * the SAME triangle data the `mesh`/`webgl` render path draws — booleans and
 * SDF already resolved), flat-shaded, lit, and framed with a fixed isometric
 * camera for a consistent gallery look. Everything is disposed afterwards.
 */
import * as THREE from "three";
import { BODY_COLORS } from "./bodyColors";

/** The minimal per-body triangle payload the thumbnail needs (a structural
 *  subset of `MeshedBody`). */
export interface ThumbnailBody {
  positions: Float32Array;
  normals: Float32Array;
  indices: Uint32Array;
}

/** Default square thumbnail edge in device pixels. */
const DEFAULT_SIZE = 512;

/**
 * Render the given bodies to a PNG `Blob`, or `null` when there is nothing to
 * draw or a WebGL context can't be created (headless/no-GPU). Never throws.
 */
export async function renderThumbnailBlob(
  bodies: readonly ThumbnailBody[],
  size = DEFAULT_SIZE,
): Promise<Blob | null> {
  const usable = bodies.filter(
    (b) => b.positions.length > 0 && b.indices.length > 0,
  );
  if (usable.length === 0) return null;

  const scene = new THREE.Scene();
  const disposables: { dispose(): void }[] = [];
  const bounds = new THREE.Box3();
  const tmpBox = new THREE.Box3();

  for (let i = 0; i < usable.length; i++) {
    const b = usable[i];
    const geom = new THREE.BufferGeometry();
    geom.setAttribute("position", new THREE.BufferAttribute(b.positions, 3));
    geom.setAttribute("normal", new THREE.BufferAttribute(b.normals, 3));
    geom.setIndex(new THREE.BufferAttribute(b.indices, 1));
    geom.computeBoundingBox();
    const [r, g, bl] = BODY_COLORS[i % BODY_COLORS.length];
    const mat = new THREE.MeshStandardMaterial({
      color: new THREE.Color(r, g, bl),
      roughness: 0.4,
      metalness: 0.0,
      side: THREE.DoubleSide,
      // The kernel mesh carries angle-averaged normals; flat-shade so hard
      // edges read crisp (same reasoning as MeshPreviewBody).
      flatShading: true,
    });
    scene.add(new THREE.Mesh(geom, mat));
    disposables.push(geom, mat);
    if (geom.boundingBox) {
      tmpBox.copy(geom.boundingBox);
      bounds.union(tmpBox);
    }
  }

  if (bounds.isEmpty()) {
    for (const d of disposables) d.dispose();
    return null;
  }

  // Fixed 3/4 isometric view framing the scene's bounding sphere.
  const center = bounds.getCenter(new THREE.Vector3());
  const radius = Math.max(
    bounds.getSize(new THREE.Vector3()).length() / 2,
    1e-3,
  );
  const cam = new THREE.PerspectiveCamera(35, 1, radius * 0.01, radius * 100);
  const dir = new THREE.Vector3(1, 0.75, 1).normalize();
  const dist = (radius / Math.sin((cam.fov * Math.PI) / 360)) * 1.15;
  cam.position.copy(center).addScaledVector(dir, dist);
  cam.up.set(0, 1, 0);
  cam.lookAt(center);
  cam.updateProjectionMatrix();

  // Ambient + a key/fill/back directional trio (directionals are parallel, so
  // only their direction — position minus the origin target — matters).
  scene.add(new THREE.AmbientLight(0xffffff, 0.55));
  const key = new THREE.DirectionalLight(0xffffff, 0.9);
  key.position.copy(dir);
  const fill = new THREE.DirectionalLight(0xffffff, 0.35);
  fill.position.set(-1, 0.4, 0.6);
  const back = new THREE.DirectionalLight(0xffffff, 0.25);
  back.position.set(-0.4, 0.8, -1);
  scene.add(key, fill, back);

  const canvas = document.createElement("canvas");
  canvas.width = size;
  canvas.height = size;
  let renderer: THREE.WebGLRenderer;
  try {
    renderer = new THREE.WebGLRenderer({
      canvas,
      antialias: true,
      alpha: true,
      preserveDrawingBuffer: true,
    });
  } catch {
    for (const d of disposables) d.dispose();
    return null;
  }
  renderer.setClearColor(0x000000, 0); // transparent — the card supplies the bg

  let blob: Blob | null = null;
  try {
    renderer.render(scene, cam);
    blob = await new Promise<Blob | null>((resolve) =>
      canvas.toBlob((out) => resolve(out), "image/png"),
    );
  } catch {
    blob = null;
  } finally {
    for (const d of disposables) d.dispose();
    renderer.dispose();
    renderer.forceContextLoss();
  }
  return blob;
}
