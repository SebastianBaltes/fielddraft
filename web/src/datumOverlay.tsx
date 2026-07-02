import { useEffect, useRef } from "react";
import { useThree } from "@react-three/fiber";
import * as THREE from "three";
import { useKernelStore } from "./store";
import { DATUM_LAYER } from "./viewportSdfOverlays";
import { ELEMENT_SELECTION_COLOR } from "./viewportOverlayShared";

// ── Datum planes + origin ────────────────────────────────────────────
//
// The three default datum planes with labels, the origin marker and the
// analytic datum ray pick.

const DATUM_PLANE_SIZE = 200;
const DATUM_PLANE_OPACITY = 0.08;
const DATUM_EDGE_OPACITY = 0.35;
const DATUM_COLOR = 0x3b82f6;

const DATUM_PLANES: {
  id: string;
  rotation: [number, number, number];
  label: string;
}[] = [
  { id: "front", rotation: [0, 0, 0], label: "Front" },
  { id: "top", rotation: [-Math.PI / 2, 0, 0], label: "Top" },
  { id: "right", rotation: [0, Math.PI / 2, 0], label: "Right" },
];

const DATUM_LABEL_POS: [number, number, number] = [
  -DATUM_PLANE_SIZE / 2 + 22,
  DATUM_PLANE_SIZE / 2 - 10,
  0.5,
];

function makeLabelMaterial(text: string): THREE.MeshBasicMaterial {
  const canvas = document.createElement("canvas");
  const ctx = canvas.getContext("2d")!;
  const font = "bold 64px sans-serif";
  ctx.font = font;
  const metrics = ctx.measureText(text);
  const w = Math.ceil(metrics.width) + 8;
  const h = 80;
  canvas.width = w;
  canvas.height = h;
  ctx.font = font;
  ctx.fillStyle = "#3b82f6";
  ctx.globalAlpha = 0.6;
  ctx.textBaseline = "top";
  ctx.fillText(text, 0, 4);
  const tex = new THREE.CanvasTexture(canvas);
  tex.minFilter = THREE.LinearFilter;
  tex.magFilter = THREE.LinearFilter;
  return new THREE.MeshBasicMaterial({
    map: tex,
    transparent: true,
    depthTest: false,
    side: THREE.DoubleSide,
  });
}

const datumLabelCache: Record<
  string,
  { material: THREE.MeshBasicMaterial; aspect: number }
> = {};
function getDatumLabel(text: string) {
  if (!datumLabelCache[text]) {
    const mat = makeLabelMaterial(text);
    const aspect =
      (mat.map as THREE.CanvasTexture).image.width /
      (mat.map as THREE.CanvasTexture).image.height;
    datumLabelCache[text] = { material: mat, aspect };
  }
  return datumLabelCache[text];
}

const datumEdgesSource = new THREE.PlaneGeometry(
  DATUM_PLANE_SIZE,
  DATUM_PLANE_SIZE,
);
const datumEdgesGeometry = new THREE.EdgesGeometry(datumEdgesSource);
datumEdgesSource.dispose();

/** Pick radius (mm) around the origin marker — generously larger than the
 *  1.5 mm sphere so the origin stays clickable. */
const DATUM_ORIGIN_PICK_RADIUS = 6;

/**
 * Datum under a viewport ray (ticket r36f): "origin" when the ray passes
 * within {@link DATUM_ORIGIN_PICK_RADIUS} of the world origin, else the
 * front-most visible default plane the ray hits inside its rendered quad.
 * Returns null when the ray misses everything (or the datums are hidden).
 * The plane quads live on the world axes: front = z 0, top = y 0,
 * right = x 0, each ±DATUM_PLANE_SIZE/2.
 */
export function pickDatumAtRay(
  origin: THREE.Vector3,
  dir: THREE.Vector3,
  hidden: ReadonlySet<string>,
): string | null {
  const o = [origin.x, origin.y, origin.z];
  const d = [dir.x, dir.y, dir.z];
  if (!hidden.has("origin")) {
    const t = -(o[0] * d[0] + o[1] * d[1] + o[2] * d[2]);
    if (t > 0) {
      const dist = Math.hypot(
        o[0] + t * d[0],
        o[1] + t * d[1],
        o[2] + t * d[2],
      );
      if (dist <= DATUM_ORIGIN_PICK_RADIUS) return "origin";
    }
  }
  const half = DATUM_PLANE_SIZE / 2;
  const planes: { id: string; axis: number }[] = [
    { id: "front", axis: 2 },
    { id: "top", axis: 1 },
    { id: "right", axis: 0 },
  ];
  let best: string | null = null;
  let bestT = Infinity;
  for (const { id, axis } of planes) {
    if (hidden.has(id)) continue;
    if (Math.abs(d[axis]) < 1e-9) continue;
    const t = -o[axis] / d[axis];
    if (t <= 0 || t >= bestT) continue;
    const u = o[(axis + 1) % 3] + t * d[(axis + 1) % 3];
    const v = o[(axis + 2) % 3] + t * d[(axis + 2) % 3];
    if (Math.abs(u) > half || Math.abs(v) > half) continue;
    best = id;
    bestT = t;
  }
  return best;
}

export function DatumOverlay() {
  const { state } = useKernelStore();
  const hidden = state?.hiddenDatums ?? new Set<string>();
  const selectedDatum = state?.selectedDatum ?? null;
  const invalidate = useThree((s) => s.invalidate);

  const originVisible = !hidden.has("origin");
  const visiblePlanes = DATUM_PLANES.filter((p) => !hidden.has(p.id));

  useEffect(() => {
    invalidate();
  }, [hidden, selectedDatum, invalidate]);

  // Ticket p8dp: the deferred shade quad's opaque miss-fill paints over
  // anything the main-scene (layer-0) pass drew underneath it, so in the SDF
  // path datums must be re-drawn AFTER the shade. Ticket 1m8g: do that on a
  // DEDICATED datum layer (not the shared OVERLAY_LAYER) so renderOverlayLayer
  // can draw datums with the body depth still intact — a solid in front
  // occludes the datum behind it — separately from the B-Rep edges, which
  // x-ray through solids on OVERLAY_LAYER. `enable` (not `set`) keeps datums on
  // layer 0 too, so the plain WebGL path still draws them with no extra work.
  // Re-runs whenever the visible set changes so freshly mounted meshes get it.
  const groupRef = useRef<THREE.Group>(null);
  useEffect(() => {
    groupRef.current?.traverse((o) => o.layers.enable(DATUM_LAYER));
  }, [originVisible, visiblePlanes.length, selectedDatum]);

  if (!originVisible && visiblePlanes.length === 0) return null;

  return (
    <group ref={groupRef}>
      {originVisible && (
        <mesh>
          <sphereGeometry args={[1.5, 16, 16]} />
          <meshBasicMaterial
            color={
              selectedDatum === "origin" ? ELEMENT_SELECTION_COLOR : DATUM_COLOR
            }
            transparent
            opacity={selectedDatum === "origin" ? 1 : 0.6}
            depthTest={false}
          />
        </mesh>
      )}
      {visiblePlanes.map((plane) => (
        <group key={plane.id} rotation={plane.rotation}>
          <mesh>
            <planeGeometry args={[DATUM_PLANE_SIZE, DATUM_PLANE_SIZE]} />
            <meshBasicMaterial
              color={
                selectedDatum === plane.id
                  ? ELEMENT_SELECTION_COLOR
                  : DATUM_COLOR
              }
              transparent
              opacity={
                selectedDatum === plane.id
                  ? DATUM_PLANE_OPACITY * 2
                  : DATUM_PLANE_OPACITY
              }
              side={THREE.DoubleSide}
              depthWrite={false}
            />
          </mesh>
          <lineSegments geometry={datumEdgesGeometry}>
            <lineBasicMaterial
              color={
                selectedDatum === plane.id
                  ? ELEMENT_SELECTION_COLOR
                  : DATUM_COLOR
              }
              transparent
              opacity={selectedDatum === plane.id ? 0.9 : DATUM_EDGE_OPACITY}
            />
          </lineSegments>
          <mesh
            position={DATUM_LABEL_POS}
            material={getDatumLabel(plane.label).material}
            scale={[12 * getDatumLabel(plane.label).aspect, 12, 1]}
          >
            <planeGeometry args={[1, 1]} />
          </mesh>
        </group>
      ))}
    </group>
  );
}
