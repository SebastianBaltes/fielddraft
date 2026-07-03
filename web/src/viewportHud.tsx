import { useEffect, useMemo } from "react";
import * as THREE from "three";
import { useKernelStore } from "./store";
import type { PickResult } from "./kernel";

// ── Reference meshes + pick HUD ──────────────────────────────────────

function MeshOverlayEntry({
  data,
}: {
  data: {
    positions: Float32Array;
    normals: Float32Array;
    indices: Uint32Array;
  };
}) {
  const geometry = useMemo(() => new THREE.BufferGeometry(), []);

  useEffect(() => {
    return () => {
      geometry.dispose();
    };
  }, [geometry]);

  useEffect(() => {
    geometry.setAttribute(
      "position",
      new THREE.BufferAttribute(data.positions, 3),
    );
    geometry.setAttribute("normal", new THREE.BufferAttribute(data.normals, 3));
    geometry.setIndex(new THREE.BufferAttribute(data.indices, 1));
    geometry.computeBoundingBox();
    geometry.computeBoundingSphere();
  }, [geometry, data]);

  return (
    <mesh geometry={geometry}>
      <meshStandardMaterial
        color="#8888cc"
        transparent
        opacity={0.4}
        depthWrite={false}
        side={THREE.DoubleSide}
      />
    </mesh>
  );
}

export function ReferenceMesh() {
  const { store, state } = useKernelStore();

  const overlays = useMemo(() => {
    if (!store || !state) return [];
    return store.getMeshOverlayFeatures();
  }, [store, state?.features]);

  const refMesh = state?.referenceMesh;

  if (overlays.length === 0 && !refMesh) return null;

  return (
    <>
      {refMesh ? <MeshOverlayEntry data={refMesh} /> : null}
      {overlays.map((o) => (
        <MeshOverlayEntry key={o.id} data={o.data} />
      ))}
    </>
  );
}

// Invisible pick-state probe. The on-screen face/edge id readout was a
// debug overlay and is gone; this element stays in the DOM (visually
// hidden) only as a programmatic hook for the hover-pick e2e smoke test.
export function PickHud({ result }: { result: PickResult | null }) {
  return (
    <div
      className="pick-hud"
      data-testid="fielddraft-pick-hud"
      data-pick-hit={result?.hit ? "true" : "false"}
      aria-hidden="true"
    />
  );
}
