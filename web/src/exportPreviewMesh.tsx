/**
 * Export-preview solid.
 *
 * Renders the exact merged export triangles (published by the export
 * dialog via `exportPreviewStore`) as a flat-shaded solid on the
 * dedicated PREVIEW_LAYER. The SDF path draws that layer AFTER the body
 * shade pass, so the mesh can crossfade transparently OVER the
 * SDF-rendered solid (the dialog's opacity slider); a checkbox overlays
 * the triangle wireframe. Its own lights (also on PREVIEW_LAYER) make it
 * read as a solid regardless of the surrounding scene.
 *
 * Returns `null` when no preview is published — the normal renderer then
 * owns the frame again.
 */

import { useEffect, useMemo, useSyncExternalStore } from "react";
import { useThree } from "@react-three/fiber";
import * as THREE from "three";
import {
  getExportPreview,
  getExportPreviewOpacity,
  getExportPreviewWireframe,
  subscribeExportPreview,
} from "./exportPreviewStore";
import { PREVIEW_LAYER } from "./viewportLayers";

/** Put an object (mesh / light) on the preview layer only. */
const onPreviewLayer = (o: THREE.Object3D | null) => {
  o?.layers.set(PREVIEW_LAYER);
};

export function ExportPreviewMesh() {
  const preview = useSyncExternalStore(
    subscribeExportPreview,
    getExportPreview,
  );
  const opacity = useSyncExternalStore(
    subscribeExportPreview,
    getExportPreviewOpacity,
  );
  const wireframe = useSyncExternalStore(
    subscribeExportPreview,
    getExportPreviewWireframe,
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
    geometry.computeVertexNormals();
    geometry.computeBoundingBox();
    geometry.computeBoundingSphere();
    invalidate();
  }, [geometry, preview, invalidate]);

  // Repaint on every crossfade/wireframe change (the SDF path is
  // demand-driven; a state change alone won't trigger a frame).
  useEffect(() => {
    invalidate();
  }, [opacity, wireframe, invalidate]);

  if (!preview) return null;

  return (
    <>
      <ambientLight ref={onPreviewLayer} intensity={0.6} />
      <directionalLight
        ref={onPreviewLayer}
        position={[200, 300, 200]}
        intensity={0.85}
      />
      <directionalLight
        ref={onPreviewLayer}
        position={[-150, -100, -200]}
        intensity={0.25}
      />
      <mesh ref={onPreviewLayer} geometry={geometry} renderOrder={1}>
        <meshStandardMaterial
          color="#b3bcc9"
          flatShading
          side={THREE.DoubleSide}
          transparent={opacity < 1}
          opacity={opacity}
          depthWrite={opacity > 0.98}
        />
      </mesh>
      {wireframe && (
        <mesh ref={onPreviewLayer} geometry={geometry} renderOrder={2}>
          <meshBasicMaterial
            color="#1e293b"
            wireframe
            transparent
            opacity={Math.max(opacity, 0.35)}
            depthWrite={false}
          />
        </mesh>
      )}
    </>
  );
}
