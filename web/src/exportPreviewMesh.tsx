/**
 * Export-preview solid.
 *
 * Renders the exact merged export triangles (published by the export
 * dialog via `exportPreviewStore`) as a flat-shaded solid on layer 0.
 * On the WebGPU path the SDF body pass is suppressed while this is
 * active (see `viewportSdfOverlays.renderOverlayLayer`), so this mesh
 * takes the body's place; on the WebGL path the caller swaps the normal
 * body meshes for this one. Its own lights make it look like a solid
 * regardless of the surrounding scene (the SDF scene carries none).
 *
 * Returns `null` when no preview is published — the normal renderer
 * then owns the frame again.
 */

import { useEffect, useMemo, useSyncExternalStore } from "react";
import { useThree } from "@react-three/fiber";
import * as THREE from "three";
import { getExportPreview, subscribeExportPreview } from "./exportPreviewStore";

export function ExportPreviewMesh() {
  const preview = useSyncExternalStore(
    subscribeExportPreview,
    getExportPreview,
  );
  const invalidate = useThree((s) => s.invalidate);
  const geometry = useMemo(() => new THREE.BufferGeometry(), []);

  useEffect(() => () => geometry.dispose(), [geometry]);

  useEffect(() => {
    if (!preview) {
      invalidate(); // repaint the frame the normal renderer owns again
      return;
    }
    geometry.setAttribute(
      "position",
      new THREE.BufferAttribute(preview.positions, 3),
    );
    geometry.setIndex(new THREE.BufferAttribute(preview.indices, 1));
    // Flat shading derives face normals in-shader, but a normal buffer
    // keeps the material happy on every backend.
    geometry.computeVertexNormals();
    geometry.computeBoundingBox();
    geometry.computeBoundingSphere();
    invalidate();
  }, [geometry, preview, invalidate]);

  if (!preview) return null;

  return (
    <>
      <ambientLight intensity={0.6} />
      <directionalLight position={[200, 300, 200]} intensity={0.85} />
      <directionalLight position={[-150, -100, -200]} intensity={0.25} />
      <mesh geometry={geometry}>
        <meshStandardMaterial
          color="#b3bcc9"
          flatShading
          side={THREE.DoubleSide}
        />
      </mesh>
    </>
  );
}
