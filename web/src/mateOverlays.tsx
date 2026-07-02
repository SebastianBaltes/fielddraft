import { useEffect, useMemo, useRef } from "react";
import { useFrame, useThree } from "@react-three/fiber";
import * as THREE from "three";
import { isWebGPURenderer } from "./overlayLineMaterials";
import { useKernelStore } from "./store";
import { OVERLAY_LAYER } from "./viewportSdfOverlays";
import {
  orthonormalMateFrame,
  buildMateGizmo,
  disposeMateGizmo,
} from "./mateGizmo";
import {
  enumerateConnectionPoints,
  mateFrameFor,
} from "./mateConnectionPoints";
import { setMateInferenceSnap } from "./mateInferenceBridge";
import { getHoveredElementOfKind } from "./brepPickBridge";

// ── Assembly parts + mate connectors ─────────────────────────────────
//
// The assembly part meshes, the resolved mate-connector gizmos and the
// mate-connector inference (hover preview) overlay.

function AssemblyPartBody({
  body,
  isSelected,
}: {
  body: {
    positions: Float32Array;
    normals: Float32Array;
    indices: Uint32Array;
  };
  isSelected: boolean;
}) {
  const geometry = useMemo(() => {
    const geom = new THREE.BufferGeometry();
    geom.setAttribute("position", new THREE.BufferAttribute(body.positions, 3));
    geom.setAttribute("normal", new THREE.BufferAttribute(body.normals, 3));
    geom.setIndex(new THREE.BufferAttribute(body.indices, 1));
    geom.computeBoundingSphere();
    return geom;
  }, [body.positions, body.normals, body.indices]);

  useEffect(() => {
    return () => geometry.dispose();
  }, [geometry]);

  return (
    <mesh geometry={geometry}>
      <meshStandardMaterial
        color={isSelected ? "#6366f1" : "#94a3b8"}
        side={THREE.DoubleSide}
      />
    </mesh>
  );
}

export function AssemblyParts() {
  const { state } = useKernelStore();
  const assembly = state?.assembly;
  if (!assembly?.active || assembly.document.parts.length === 0) return null;

  return (
    <>
      {assembly.document.parts.map((part) => {
        const rot = part.transform.rotate.map(
          (d: number) => (d * Math.PI) / 180,
        ) as [number, number, number];
        return (
          <group
            key={part.id}
            position={part.transform.translate}
            rotation={rot}
          >
            {part.meshBodies.map((body, i: number) => (
              <AssemblyPartBody
                key={`${part.id}-body-${i}`}
                body={body}
                isSelected={part.id === assembly.selectedPartId}
              />
            ))}
          </group>
        );
      })}
    </>
  );
}

export function MateConnectorOverlay() {
  const { store, state } = useKernelStore();
  const webgpu = isWebGPURenderer(useThree((s) => s.gl));
  const groupRef = useRef<THREE.Group>(null);

  useEffect(() => {
    const group = groupRef.current;
    if (!group) return;
    const clear = () => {
      while (group.children.length > 0) {
        const child = group.children[0] as THREE.Group;
        group.remove(child);
        disposeMateGizmo(child);
      }
    };
    clear();
    if (!store || !state || state.mateConnectorsHidden) return;
    // C2: the kernel resolves each connector's CSYS from its anchor/orientation
    // queries during regeneration; we render the full gizmo it hands back.
    const connectors = store.kernel.resolvedMateConnectors();
    for (const c of connectors) {
      const frame = orthonormalMateFrame({
        origin: c.origin,
        z: c.normal,
        x: c.x_axis,
      });
      if (!frame) continue;
      // OVERLAY_LAYER so the deferred renderer's overlay pass draws it — the
      // body pass only renders DATUM/OVERLAY layers, never plain layer 0.
      group.add(
        buildMateGizmo(frame, { size: 14, layer: OVERLAY_LAYER, webgpu }),
      );
    }
    return clear;
  }, [store, state?.features, state?.mateConnectorsHidden, webgpu]);

  return <group ref={groupRef} />;
}

// ─── Mate-connector inference (hover preview) ────────────────────
//
// While the Mate Connector tool is active, this overlay snaps a translucent
// preview gizmo to the connection point nearest the cursor (vertices, edge
// midpoints, planar face centres) and renders small dot markers for the points
// nearby — the Onshape inference experience. The snapped point + its chosen
// orientation are published to `mateInferenceBridge` so a viewport click can
// place the connector there.
//
// Orientation (option b): each point carries one CSYS per adjacent face. The
// default follows the face under the cursor (move across an edge → the
// connector's +Z flips to that side's normal); `R` cycles the variants
// manually until the cursor moves to a different point.

const MATE_DOT_RADIUS_MM = 0.9;
const MATE_DOT_COLOR = 0x4a4a3a;
const MATE_SNAP_DOT_COLOR = 0xf5c542;
/** Snap/preview only when the nearest point is within this cursor radius. */
const MATE_SHOW_RADIUS_PX = 130;
/** Points within this cursor radius get a visible dot marker. */
const MATE_DOT_RADIUS_PX = 75;
const MATE_MAX_DOTS = 256;

export function MateInferenceOverlay({
  ndc,
}: {
  ndc: { x: number; y: number; active: boolean };
}) {
  const { store, state } = useKernelStore();
  const camera = useThree((s) => s.camera);
  const size = useThree((s) => s.size);
  const invalidate = useThree((s) => s.invalidate);
  const webgpu = isWebGPURenderer(useThree((s) => s.gl));
  const mateMode = state?.mateMode === true;

  const groupRef = useRef<THREE.Group>(null);
  const gizmoRef = useRef<THREE.Group | null>(null);
  const lastGizmoKey = useRef("");
  // Per-anchor reorient cursor (R-cycled, ×90° about +Z). Resets when the
  // cursor moves to a different connection point.
  const variantRef = useRef<{ anchor: string | null; reorient: number }>({
    anchor: null,
    reorient: 0,
  });
  // Shift LOCKS the inference: while held, the snapped connector stays put even
  // as the cursor moves (Onshape parity).
  const shiftLockRef = useRef(false);

  const dotsRef = useRef<THREE.InstancedMesh | null>(null);
  if (!dotsRef.current) {
    const g = new THREE.SphereGeometry(MATE_DOT_RADIUS_MM, 8, 6);
    const m = new THREE.MeshBasicMaterial({
      depthTest: false,
      depthWrite: false,
      transparent: true,
      opacity: 0.9,
    });
    const inst = new THREE.InstancedMesh(g, m, MATE_MAX_DOTS);
    inst.frustumCulled = false;
    inst.renderOrder = 998;
    inst.count = 0;
    // Deferred mode only renders the OVERLAY/DATUM layers over the body.
    inst.layers.enable(OVERLAY_LAYER);
    dotsRef.current = inst;
  }

  const bodyFingerprint = useMemo(() => {
    if (!store || !mateMode) return "";
    const n = store.kernel.bodyCount();
    let fp = String(n);
    for (let i = 0; i < n; i++)
      fp += `:${store.kernel.bodyId(i)}.${store.kernel.bodyVersion(i)}`;
    return fp;
    // state in deps so a regen (new features) re-enumerates.
  }, [store, mateMode, state]);

  const candidates = useMemo(
    () => (store && mateMode ? enumerateConnectionPoints(store.kernel) : []),
    [store, mateMode, bodyFingerprint],
  );

  // Mount the dot instances once.
  useEffect(() => {
    const grp = groupRef.current;
    const dots = dotsRef.current;
    if (grp && dots && dots.parent !== grp) grp.add(dots);
  }, []);

  // R cycles the orientation variant while the tool is active. Capture phase +
  // stopImmediatePropagation so it wins over global view shortcuts during the
  // (transient) tool mode.
  useEffect(() => {
    if (!mateMode) return;
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      if (
        t &&
        (t.isContentEditable ||
          t.tagName === "INPUT" ||
          t.tagName === "TEXTAREA" ||
          t.tagName === "SELECT")
      )
        return; // don't hijack typing
      if (e.key === "r" || e.key === "R") {
        variantRef.current.reorient = (variantRef.current.reorient + 1) % 4;
        e.preventDefault();
        e.stopImmediatePropagation();
        invalidate();
      }
    };
    const onShift = (e: KeyboardEvent) => {
      if (e.key !== "Shift") return;
      shiftLockRef.current = e.type === "keydown";
      invalidate();
    };
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("keydown", onShift);
    window.addEventListener("keyup", onShift);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("keydown", onShift);
      window.removeEventListener("keyup", onShift);
      shiftLockRef.current = false;
    };
  }, [mateMode, invalidate]);

  // Dispose GPU resources on unmount.
  useEffect(
    () => () => {
      setMateInferenceSnap(null);
      if (gizmoRef.current) disposeMateGizmo(gizmoRef.current);
      const dots = dotsRef.current;
      if (dots) {
        dots.geometry.dispose();
        (dots.material as THREE.Material).dispose();
      }
    },
    [],
  );

  const dummy = useMemo(() => new THREE.Object3D(), []);
  const proj = useMemo(() => new THREE.Vector3(), []);
  const view = useMemo(() => new THREE.Vector3(), []);
  const snapColor = useMemo(() => new THREE.Color(MATE_SNAP_DOT_COLOR), []);
  const dotColor = useMemo(() => new THREE.Color(MATE_DOT_COLOR), []);

  useFrame(() => {
    const grp = groupRef.current;
    const dots = dotsRef.current;
    if (!grp || !dots) return;

    const clearGizmo = () => {
      if (gizmoRef.current) {
        grp.remove(gizmoRef.current);
        disposeMateGizmo(gizmoRef.current);
        gizmoRef.current = null;
        lastGizmoKey.current = "";
      }
    };

    if (!mateMode || !ndc.active || candidates.length === 0) {
      grp.visible = false;
      dots.count = 0;
      clearGizmo();
      setMateInferenceSnap(null);
      return;
    }
    grp.visible = true;

    // Shift locks the inference: keep the current snap + gizmo + dots frozen
    // while the user moves the cursor (the bridge snap persists too).
    if (shiftLockRef.current && gizmoRef.current) return;

    const w = size.width;
    const h = size.height;
    camera.updateMatrixWorld();
    const cx = (ndc.x * 0.5 + 0.5) * w;
    const cy = (0.5 - ndc.y * 0.5) * h;

    // Project every candidate; track the nearest in-front one.
    const sx: number[] = new Array(candidates.length);
    const sy: number[] = new Array(candidates.length);
    const front: boolean[] = new Array(candidates.length);
    let best = -1;
    let bestD = Infinity;
    for (let i = 0; i < candidates.length; i++) {
      const c = candidates[i];
      view.set(c.world[0], c.world[1], c.world[2]).applyMatrix4(
        camera.matrixWorldInverse,
      );
      front[i] = view.z < 0; // camera looks down -Z in view space
      proj.set(c.world[0], c.world[1], c.world[2]).project(camera);
      sx[i] = (proj.x * 0.5 + 0.5) * w;
      sy[i] = (0.5 - proj.y * 0.5) * h;
      if (!front[i]) continue;
      const d = Math.hypot(sx[i] - cx, sy[i] - cy);
      if (d < bestD) {
        bestD = d;
        best = i;
      }
    }
    const snapIndex = best >= 0 && bestD <= MATE_SHOW_RADIUS_PX ? best : -1;

    // Dot markers: points near the cursor (+ the snapped one).
    let n = 0;
    for (let i = 0; i < candidates.length && n < MATE_MAX_DOTS; i++) {
      if (!front[i]) continue;
      const d = Math.hypot(sx[i] - cx, sy[i] - cy);
      if (d > MATE_DOT_RADIUS_PX && i !== snapIndex) continue;
      const c = candidates[i];
      dummy.position.set(c.world[0], c.world[1], c.world[2]);
      dummy.scale.setScalar(i === snapIndex ? 1.7 : 1);
      dummy.updateMatrix();
      dots.setMatrixAt(n, dummy.matrix);
      dots.setColorAt(n, i === snapIndex ? snapColor : dotColor);
      n++;
    }
    dots.count = n;
    dots.instanceMatrix.needsUpdate = true;
    if (dots.instanceColor) dots.instanceColor.needsUpdate = true;

    if (snapIndex < 0) {
      clearGizmo();
      setMateInferenceSnap(null);
      return;
    }

    const cand = candidates[snapIndex];
    const vr = variantRef.current;
    if (vr.anchor !== cand.anchorQuery) {
      vr.anchor = cand.anchorQuery;
      vr.reorient = 0;
    }
    // Secondary reference: the hovered face if it is one of the anchor's
    // adjacent faces (so moving across an edge flips the secondary axis to
    // that side / picks a vertex's +Z), else the first deterministic ref.
    let secRef = cand.secondaryRefs[0] ?? null;
    const hf = getHoveredElementOfKind("face");
    if (hf) {
      const want = `qFace(${hf.idHash})`;
      const m = cand.secondaryRefs.find((r) => r.faceQuery === want);
      if (m) secRef = m;
    }
    const secNormal = secRef ? secRef.normal : null;
    const reorient = vr.reorient;
    const f3 = mateFrameFor(cand, secNormal, reorient, false);

    setMateInferenceSnap({
      anchorQuery: cand.anchorQuery,
      t: cand.t,
      secondary: secRef ? secRef.faceQuery : null,
      reorient,
      world: cand.world,
    });

    const key = `${cand.anchorQuery}|${secRef?.faceQuery ?? ""}|${reorient}`;
    if (key !== lastGizmoKey.current) {
      clearGizmo();
      const frame = orthonormalMateFrame({
        origin: cand.world,
        z: f3.z,
        x: f3.x,
      });
      if (frame) {
        const giz = buildMateGizmo(frame, {
          size: 11,
          ghost: true,
          layer: OVERLAY_LAYER,
          webgpu,
        });
        grp.add(giz);
        gizmoRef.current = giz;
      }
      lastGizmoKey.current = key;
    }
  });

  return <group ref={groupRef} />;
}
