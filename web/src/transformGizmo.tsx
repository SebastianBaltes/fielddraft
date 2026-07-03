import { useEffect, useMemo, useRef } from "react";
import { useThree } from "@react-three/fiber";
import { TransformControls as ThreeTransformControls } from "three/examples/jsm/controls/TransformControls.js";
import * as THREE from "three";
import { useKernelStore, type StoreState } from "./store";
import type { FeatureSummary } from "./kernel";
import { OVERLAY_LAYER } from "./viewportLayers";

// ── Selection transform gizmo ────────────────────────────────────────
//
// The translate/rotate/scale TransformControls gizmo for the single
// selected feature, plus the render-invalidation helper component.

/** Feature kinds that are themselves a transform of another feature — the
 *  gizmo edits THEIR params in place instead of appending a new transform
 *  call onto a geometry feature (ticket p3vh). */
const TRANSFORM_FEATURE_KINDS = new Set(["translate", "rotate", "scale"]);

export function findSingleSelectedFeature(
  state: StoreState | null | undefined,
): FeatureSummary | undefined {
  if (!state || state.selection.size !== 1) return undefined;
  const first = state.selection.values().next().value;
  if (first === undefined) return undefined;
  return state.features.find((f) => f.id === first);
}

const NO_GIZMO_KINDS: ReadonlySet<string> = new Set([
  // extrude: owns its depth-arrow gizmo (ExtrudeDepthGizmo, ticket qqgo)
  "boolean",
  "sketch",
  "fillet_edge",
  "extrude",
  // mate_connector: a datum CSYS positioned by its anchor + offset params, not
  // a free transform — a translate/rotate/scale gizmo would be meaningless and
  // couldn't write back (ticket zcwn). It shows its own triad gizmo instead.
  "mate_connector",
]);

export function SelectionTransformGizmo() {
  const { store, state } = useKernelStore();
  const meshRef = useRef<THREE.Mesh>(null);
  const tcRef = useRef<ThreeTransformControls | null>(null);
  const syncedTranslateRef = useRef<[number, number, number] | null>(null);
  const gizmoThrottleRef = useRef<ReturnType<typeof setTimeout>>(undefined);
  const gizmoPendingRef = useRef(false);

  const camera = useThree((s) => s.camera);
  const gl = useThree((s) => s.gl);
  const scene = useThree((s) => s.scene);
  const defaultControls = useThree((s) => s.controls);
  const invalidate = useThree((s) => s.invalidate);

  const selectedFeature = useMemo(
    () => findSingleSelectedFeature(state),
    [state],
  );

  const gizmoMode = state?.gizmoMode ?? "translate";

  useEffect(() => {
    const mesh = meshRef.current;
    if (!mesh || !selectedFeature) {
      syncedTranslateRef.current = null;
      return;
    }
    const [tx, ty, tz] = selectedFeature.translate;
    const prev = syncedTranslateRef.current;
    if (!prev || prev[0] !== tx || prev[1] !== ty || prev[2] !== tz) {
      mesh.position.set(tx, ty, tz);
      syncedTranslateRef.current = [tx, ty, tz];
    }
    const [rx, ry, rz, rw] = selectedFeature.rotation;
    mesh.quaternion.set(rx, ry, rz, rw);
    const [scx, scy, scz] = selectedFeature.scale;
    mesh.scale.set(scx, scy, scz);
  }, [selectedFeature]);

  const commitRef = useRef<() => void>(() => {});
  commitRef.current = () => {
    const mesh = meshRef.current;
    if (!mesh || !store || !selectedFeature) return;
    gizmoPendingRef.current = false;
    const p = mesh.position;
    const q = mesh.quaternion;
    let sx = mesh.scale.x,
      sy = mesh.scale.y,
      sz = mesh.scale.z;
    if (gizmoMode === "translate") {
      syncedTranslateRef.current = [p.x, p.y, p.z];
    }
    if (gizmoMode === "scale" && selectedFeature.kind === "sphere") {
      const u = Math.max(Math.abs(sx), Math.abs(sy), Math.abs(sz));
      sx = u;
      sy = u;
      sz = u;
      mesh.scale.set(u, u, u);
    }
    // A selected TRANSFORM feature (Translate/Rotate/Scale row in the
    // outliner) edits ITS OWN params: `applyGizmo*` targets a geometry
    // feature and appends/merges a transform onto it, but a transform
    // feature never appears in the kernel's decoded spine, so the call
    // was a silent no-op (ticket p3vh — dragging the sphere's translate
    // gizmo moved nothing). Only the matching mode commits; dragging a
    // rotate handle on a Translate feature has no parameter to write.
    if (TRANSFORM_FEATURE_KINDS.has(selectedFeature.kind)) {
      let current: Record<string, unknown>;
      try {
        current = store.kernel.getFeatureCallParams(selectedFeature.id);
      } catch {
        return;
      }
      if (gizmoMode === "translate" && selectedFeature.kind === "translate") {
        store.updateFeatureParams(selectedFeature.id, {
          ...current,
          translate: [p.x, p.y, p.z],
        });
      } else if (gizmoMode === "rotate" && selectedFeature.kind === "rotate") {
        const e = new THREE.Euler().setFromQuaternion(q, "XYZ");
        store.updateFeatureParams(selectedFeature.id, {
          ...current,
          rotation: [e.x, e.y, e.z],
        });
      } else if (gizmoMode === "scale" && selectedFeature.kind === "scale") {
        store.updateFeatureParams(selectedFeature.id, {
          ...current,
          scale: [sx, sy, sz],
        });
      }
      return;
    }
    if (gizmoMode === "translate") {
      store.applyGizmoTranslate(selectedFeature.id, p.x, p.y, p.z);
    } else if (gizmoMode === "rotate") {
      store.applyGizmoRotate(selectedFeature.id, q.x, q.y, q.z, q.w);
    } else {
      store.applyGizmoScale(selectedFeature.id, sx, sy, sz);
    }
  };

  useEffect(() => {
    const mesh = meshRef.current;
    if (!mesh || !selectedFeature || !store) {
      if (tcRef.current) {
        tcRef.current.detach();
        scene.remove(tcRef.current.getHelper() as THREE.Object3D);
        tcRef.current.dispose();
        tcRef.current = null;
      }
      return;
    }

    const tc = new ThreeTransformControls(camera, gl.domElement);
    const helper = tc.getHelper() as THREE.Object3D;
    tc.attach(mesh);
    tc.setMode(gizmoMode);
    tc.setSize(0.5);
    helper.traverse((child: THREE.Object3D) => {
      child.layers.enable(OVERLAY_LAYER);
      // Ticket 7hk3: on the WebGL fallback the gizmo's FIRST program compile can
      // land before the classic WebGLRenderer's shader infrastructure is fully
      // warm (the manual-advance root renders the helper on the very first frame
      // it appears). That throws inside resolveIncludes and three caches the
      // failed program, so the gizmo silently never draws on the first
      // arm-and-select after load. Forcing the helper's materials to recompile
      // clears any cached failure on the next (warm) frame — a one-time no-op on
      // WebGPU and on every subsequent mount. Mirrors the manual recompile that
      // was verified to make the arrows appear.
      const mat = (child as THREE.Mesh).material;
      if (mat) {
        if (Array.isArray(mat)) mat.forEach((m) => (m.needsUpdate = true));
        else mat.needsUpdate = true;
      }
    });

    const onDragging = (e: { value: boolean }) => {
      if (defaultControls) (defaultControls as any).enabled = !e.value;
      // "B-Rep rebuild while dragging" (default off): suppress the
      // per-commit weave for the whole drag, catch up once on release.
      if (e.value) store.beginGizmoDrag();
      else store.endGizmoDrag();
    };
    const onChange = () => invalidate();
    const onObjectChange = () => {
      gizmoPendingRef.current = true;
      if (gizmoThrottleRef.current !== undefined) return;
      commitRef.current();
      gizmoThrottleRef.current = setTimeout(() => {
        gizmoThrottleRef.current = undefined;
        if (gizmoPendingRef.current) commitRef.current();
      }, 50);
    };

    tc.addEventListener("dragging-changed", onDragging as any);
    tc.addEventListener("change", onChange);
    tc.addEventListener("objectChange", onObjectChange);
    scene.add(helper);
    tcRef.current = tc;
    invalidate();

    return () => {
      clearTimeout(gizmoThrottleRef.current);
      gizmoThrottleRef.current = undefined;
      tc.removeEventListener("dragging-changed", onDragging as any);
      tc.removeEventListener("change", onChange);
      tc.removeEventListener("objectChange", onObjectChange);
      tc.detach();
      scene.remove(helper);
      tc.dispose();
      tcRef.current = null;
      // Unmounting mid-drag (feature deselected/deleted) must lift an
      // active B-Rep suppression; a no-op otherwise (idempotent).
      store.endGizmoDrag();
    };
  }, [
    selectedFeature?.id,
    gizmoMode,
    camera,
    gl,
    scene,
    defaultControls,
    invalidate,
    store,
  ]);

  useEffect(() => () => clearTimeout(gizmoThrottleRef.current), []);

  if (!selectedFeature || !store || NO_GIZMO_KINDS.has(selectedFeature.kind))
    return null;

  return (
    <mesh ref={meshRef} visible={false}>
      <boxGeometry args={[1, 1, 1]} />
    </mesh>
  );
}

export function Invalidator({
  ndc,
}: {
  ndc: { x: number; y: number; active: boolean };
}) {
  const invalidate = useThree((s) => s.invalidate);
  const { state } = useKernelStore();
  const meshedBodies = state?.meshedBodies;
  const sketchActive = state?.sketchSession.active ?? false;
  const sketchSubCalls = state?.sketchSession.subCalls;
  const selection = state?.selection;
  const features = state?.features;
  const visibleSketches = state?.visibleSketches;

  useEffect(() => {
    invalidate();
  }, [
    invalidate,
    meshedBodies,
    sketchActive,
    sketchSubCalls,
    selection,
    features,
    visibleSketches,
    ndc.x,
    ndc.y,
    ndc.active,
  ]);

  return null;
}
