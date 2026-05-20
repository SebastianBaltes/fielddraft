/**
 * P2.5 — Read-only shareable viewer.
 *
 * Lightweight component that loads a published snapshot and renders it
 * in a rotating orbit view without the editor UI. Intended for embedding
 * via iframe or direct URL.
 */

import { useEffect, useMemo, useState } from "react";
import { Canvas } from "@react-three/fiber";
import * as THREE from "three";
import { OrbitControls } from "@react-three/drei";
import { Kernel } from "./kernel";

interface ViewerProps {
  snapshotId: string;
  serverUrl?: string;
}

interface ViewerBody {
  positions: Float32Array;
  normals: Float32Array;
  indices: Uint32Array;
}

export function Viewer({ snapshotId, serverUrl = "" }: ViewerProps) {
  const [bodies, setBodies] = useState<ViewerBody[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const resp = await fetch(`${serverUrl}/api/v/${snapshotId}`);
        if (!resp.ok) {
          setError(`Snapshot not found (${resp.status})`);
          setLoading(false);
          return;
        }
        const snap = await resp.json();
        const docJson = typeof snap.document === "string"
          ? snap.document
          : JSON.stringify(snap.document);

        const kernel = await Kernel.create();
        kernel.loadJson(docJson);

        if (cancelled) return;
        const count = kernel.bodyCount();
        const result: ViewerBody[] = [];
        for (let i = 0; i < count; i++) {
          result.push({
            positions: kernel.bodyPositionsView(i).slice(),
            normals: kernel.bodyNormalsView(i).slice(),
            indices: kernel.bodyIndicesView(i).slice(),
          });
        }
        setBodies(result);
      } catch (e) {
        if (!cancelled) setError(String(e));
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => { cancelled = true; };
  }, [snapshotId, serverUrl]);

  if (loading) {
    return <div className="fd-viewer-loading">Loading model...</div>;
  }
  if (error) {
    return <div className="fd-viewer-error">{error}</div>;
  }

  return (
    <div className="fd-viewer">
      <Canvas
        camera={{ position: [200, 150, 200], fov: 45, near: 1, far: 4000 }}
      >
        <ambientLight intensity={0.6} />
        <directionalLight position={[200, 300, 200]} intensity={0.8} />
        {bodies.map((body, i) => (
          <ViewerBody key={i} body={body} />
        ))}
        <OrbitControls enableDamping dampingFactor={0.1} />
        <gridHelper args={[400, 20, "#d4d4d8", "#e4e4e7"]} />
      </Canvas>
    </div>
  );
}

function ViewerBody({ body }: { body: ViewerBody }) {
  const geom = useMemo(() => {
    const g = new THREE.BufferGeometry();
    g.setAttribute("position", new THREE.BufferAttribute(body.positions, 3));
    g.setAttribute("normal", new THREE.BufferAttribute(body.normals, 3));
    g.setIndex(new THREE.BufferAttribute(body.indices, 1));
    g.computeBoundingSphere();
    return g;
  }, [body]);

  useEffect(() => {
    return () => geom.dispose();
  }, [geom]);

  return (
    <mesh geometry={geom}>
      <meshStandardMaterial color="#6366f1" side={THREE.DoubleSide} />
    </mesh>
  );
}

export function generateEmbedSnippet(viewUrl: string): string {
  return `<iframe src="${viewUrl}" width="640" height="480" frameborder="0" allow="webgpu"></iframe>`;
}
