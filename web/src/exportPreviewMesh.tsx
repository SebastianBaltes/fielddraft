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
import { useState } from "react";

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
  // Ticket cdb25e1e: material.wireframe is IGNORED by the WebGPURenderer
  // (no line fill-mode in WebGPU) — the checkbox showed nothing in the SDF
  // render modes. The wireframe is real LineSegments geometry instead,
  // rebuilt whenever the preview mesh changes.
  const [wireGeo, setWireGeo] = useState<THREE.WireframeGeometry | null>(
    null,
  );

  useEffect(() => () => geometry.dispose(), [geometry]);
  useEffect(
    () => () => {
      wireGeo?.dispose();
    },
    [wireGeo],
  );

  useEffect(() => {
    if (!preview) {
      setWireGeo(null);
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
    setWireGeo(new THREE.WireframeGeometry(geometry));
    invalidate();
  }, [geometry, preview, invalidate]);

  // Ticket cdb25e1e: manage the solid's material imperatively. `transparent`
  // stays ALWAYS true — toggling it per opacity value rebuilds the WebGPU
  // pipeline and made the slider act like on/off; a constant alpha-blend
  // pipeline gives smooth intermediate opacities. Only the depthWrite flip
  // (opaque-ish ⇒ write depth) needs a pipeline update.
  const solidMat = useMemo(
    () =>
      new THREE.MeshStandardMaterial({
        color: "#b3bcc9",
        flatShading: true,
        side: THREE.DoubleSide,
        transparent: true,
        polygonOffset: true,
        polygonOffsetFactor: -2,
        polygonOffsetUnits: -4,
      }),
    [],
  );
  useEffect(() => () => solidMat.dispose(), [solidMat]);
  useEffect(() => {
    solidMat.opacity = opacity;
    const depthWrite = opacity > 0.98;
    if (solidMat.depthWrite !== depthWrite) {
      solidMat.depthWrite = depthWrite;
      solidMat.needsUpdate = true;
    }
  }, [opacity, solidMat]);

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
      {/* The preview solid coincides with the SDF surface, so it must WIN
          the depth test against the body depth the shade pass wrote — since
          that depth is the exact surface (ticket iw6f removed the former
          0.1% push-back), the mesh z-fights and vanishes at opacity < 1
          without a bias of its own (ticket rfm6). polygonOffset pulls the
          preview fragments toward the camera; the wireframe gets a slightly
          stronger pull so it also beats the preview solid. */}
      <mesh
        ref={onPreviewLayer}
        geometry={geometry}
        material={solidMat}
        renderOrder={1}
      />
      {wireframe && wireGeo && (
        <lineSegments ref={onPreviewLayer} geometry={wireGeo} renderOrder={2}>
          <lineBasicMaterial
            color="#1e293b"
            transparent
            opacity={Math.max(opacity, 0.35)}
            depthWrite={false}
          />
        </lineSegments>
      )}
    </>
  );
}
