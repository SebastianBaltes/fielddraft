import { useEffect, useMemo, useRef } from "react";
import { useFrame, useThree } from "@react-three/fiber";
import * as THREE from "three";
import { useKernelStore } from "./store";
import type { HybridLeafInfo } from "./kernel";
import { resolveMaterials, type MaterialProps } from "./materialBridge";
import {
  DEFAULT_ROUGHNESS,
  DEFAULT_METALNESS,
  LIGHTS,
  AMBIENT,
} from "./pbrShading";
import {
  BODY_COLORS,
  applyLeafTransform,
  createPrimitiveGeometry,
  buildWarpedLeafGeometry,
} from "./sdfDeferredRenderer";
import {
  bodyNeedsSubtractionPass,
  bodyNeedsIntersectionPass,
} from "./screenSpaceCsg";
import { collectAnalyticBodyMeshes, type BrepMeshKernel } from "./brepMesh";
import { useBodyFingerprint } from "./viewportOverlayShared";

// ── Classic-WebGL render path ────────────────────────────────────────
//
// Body meshes for the plain THREE.WebGLRenderer fallback: DC boolean
// meshes (MeshPreview), parametric leaf hulls (WebGLBodies), the analytic
// B-Rep tessellation (BRepMeshPreview) and the shared scene lights.

/**
 * SS-CSG-Skip-Prädikat: ein Body wird vom Screen-Space-CSG-Pass komponiert (und
 * darf daher NICHT zusätzlich als DC-Mesh/Hülle zeichnen), wenn er entweder eine
 * Subtraktion (A−B) oder eine binäre Intersection (A∩B) ist.
 */
function bodyHandledByScsPass(leaves: HybridLeafInfo[]): boolean {
  return bodyNeedsSubtractionPass(leaves) || bodyNeedsIntersectionPass(leaves);
}

// Material defaults for the classic-WebGL render paths (MeshPreview body fill +
// WebGLBodies leaf hulls). Aligned to the WebGPU/SDF path so the look matches
// across renderers (ticket gcvz): pbrShading.ts defaults roughness 0.35,
// metalness 0.0. Only applied to bodies WITHOUT an explicit user material.
const WEBGL_DEFAULT_ROUGHNESS = DEFAULT_ROUGHNESS;
const WEBGL_DEFAULT_METALNESS = DEFAULT_METALNESS;

/**
 * Scene lighting for the classic-WebGL render paths. Uses the SAME light setup
 * as the SDF (WebGPU) shader — `LIGHTS` / `AMBIENT` from pbrShading.ts as the
 * single source of truth. The SDF dirs/radiances are VIEW-space (camera-space),
 * so each frame we rotate every `dir` into world space with the camera's
 * orientation and place the directional there (target pinned to the world
 * origin), making the three.js light direction `normalize(position - target)`
 * track the camera (headlight-like) exactly like the SDF view-space `L`. Exact
 * pixel parity isn't expected (three's MeshStandardMaterial BRDF differs from
 * the SDF Cook-Torrance), only an identical light configuration.
 */
export function WebGLSceneLights() {
  const camera = useThree((s) => s.camera);
  const lightRefs = useRef<(THREE.DirectionalLight | null)[]>([]);

  // Per-light targets pinned to the WORLD origin. three computes a directional's
  // direction as `lightPos − targetPos`, so the target MUST be a separate object
  // at the origin: nesting it under the light (both at the same world position)
  // collapses the direction to (0,0,0) and the light contributes nothing.
  const targets = useMemo(() => LIGHTS.map(() => new THREE.Object3D()), []);
  const tmp = useMemo(() => new THREE.Vector3(), []);

  // The SDF dirs are VIEW-space; rotate them into world space by the camera
  // orientation each frame so the directionals follow the camera. The lights
  // stay DIRECT scene children — three's WebGLRenderer only collects lights it
  // finds while traversing the scene graph, and R3F's camera is NOT in that
  // graph, so parenting lights to the camera would leave the scene unlit.
  useFrame(() => {
    for (let i = 0; i < LIGHTS.length; i++) {
      const light = lightRefs.current[i];
      if (!light) continue;
      const d = LIGHTS[i].dir;
      tmp.set(d[0], d[1], d[2]).applyQuaternion(camera.quaternion);
      light.position.copy(tmp);
    }
  });

  return (
    <>
      <ambientLight intensity={AMBIENT} />
      {targets.map((t, i) => (
        <primitive key={`tgt-${i}`} object={t} />
      ))}
      {LIGHTS.map(({ radiance }, i) => (
        <directionalLight
          key={i}
          ref={(el) => {
            lightRefs.current[i] = el;
          }}
          intensity={radiance}
          target={targets[i]}
        />
      ))}
    </>
  );
}

/** Parametric leaf kinds that `createPrimitiveGeometry` (sdfDeferredRenderer)
 *  renders as an exact/near-exact analytic hull — far smoother than the general
 *  DC/SDF mesh. Ticket 55ea: a helix DC-meshes very coarsely; its parametric
 *  `buildHelixGeometry` (48 segments/turn) is the intended representation. */
const WEBGL_PARAMETRIC_KINDS = new Set([
  "box",
  "sphere",
  "cylinder",
  "cone",
  "torus",
  "capsule",
  "extrude",
  "revolve",
  "helix",
  "sweep",
]);

/**
 * Ticket 55ea/mdo0: should WebGL body `i` render from its parametric leaf hulls
 * (`WebGLBodies`) instead of the coarse boolean DC mesh (`MeshPreview`)? True
 * for a single parametric solid AND for a PURE UNION of parametric solids:
 *
 *   - Single primitive: its analytic hull IS the surface, far finer than DC.
 *   - Pure union (mdo0): two+ closed opaque hulls rendered with depth-testing
 *     show exactly the union boundary — the nearer solid's front face wins, and
 *     a face interior to the other solid is correctly occluded by that solid's
 *     front face. No screen-space CSG needed for UNION; the kernel already flags
 *     it via `is_union_only` (false for Difference/Intersection/Smooth/Masked).
 *
 * Gate: not `needs_full_sdf`, no residual SDF regions, at least one non-skip
 * leaf, all of a kind `createPrimitiveGeometry` handles, and — when multi-leaf —
 * `is_union_only`. Difference/Intersection/Smooth/SDF-region bodies fail and
 * keep the boolean-correct DC mesh (WebGLBodies does not composite non-unions).
 */
export function isWebglParametricBody(
  kernel: {
    bodyHybridInfo(i: number): {
      leaves: HybridLeafInfo[];
      sdf_regions?: unknown[];
      needs_full_sdf?: boolean;
      is_union_only?: boolean;
    };
  },
  i: number,
): boolean {
  try {
    const info = kernel.bodyHybridInfo(i);
    if (info.needs_full_sdf) return false;
    if (info.sdf_regions && info.sdf_regions.length > 0) return false;
    const real = info.leaves.filter((l) => l.role !== "skip");
    if (real.length === 0) return false;
    if (real.length > 1 && info.is_union_only !== true) return false;
    return real.every((l) => WEBGL_PARAMETRIC_KINDS.has(l.kind));
  } catch {
    return false;
  }
}

function MeshPreviewBody({
  body,
  mat,
  bodyIndex,
}: {
  body: {
    positions: Float32Array;
    normals: Float32Array;
    indices: Uint32Array;
  };
  mat: MaterialProps | undefined;
  bodyIndex: number;
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

  const fallback = BODY_COLORS[bodyIndex % BODY_COLORS.length];
  const color = mat
    ? new THREE.Color(mat.color[0], mat.color[1], mat.color[2])
    : new THREE.Color(fallback[0], fallback[1], fallback[2]);
  const emissive = mat
    ? new THREE.Color(mat.emissive[0], mat.emissive[1], mat.emissive[2])
    : undefined;

  return (
    <mesh geometry={geometry}>
      <meshStandardMaterial
        color={color}
        roughness={mat?.roughness ?? WEBGL_DEFAULT_ROUGHNESS}
        metalness={mat?.metalness ?? WEBGL_DEFAULT_METALNESS}
        emissive={emissive}
        opacity={mat?.opacity ?? 1}
        transparent={mat != null && mat.opacity < 1}
        wireframe={mat?.wireframe ?? false}
        side={THREE.DoubleSide}
        // Ticket dkaa: FLAT shade the Dual-Contouring boolean mesh. The DC
        // mesher emits ANGLE-AVERAGED vertex normals (a vertex on a box's hard
        // edge gets e.g. (0,0.71,-0.71) — the mean of the two faces it touches).
        // Smooth-shading those across a hard-edged solid bled dark gradient
        // triangles from every corner/edge across the flat faces (the WebGL
        // "render artifacts" the user saw — independent of projection and of
        // DoubleSide). Deriving per-face normals in the shader (flatShading)
        // ignores the bad smooth normals and renders the true facets, so hard
        // edges read crisp. Only the boolean/non-parametric DC mesh comes
        // through here; genuinely smooth parametric solids render via
        // WebGLBodies (analytic hull, smooth normals) and keep their shading.
        flatShading
      />
    </mesh>
  );
}

/**
 * @param parametricSplit Ticket 55ea: when set (the WebGL path), SKIP bodies
 *   that `isWebglParametricBody` flags — `WebGLBodies` renders those from their
 *   fine analytic hull instead of this coarse DC mesh. Off in `mesh` render
 *   mode, where the whole point is to show the DC mesher's output.
 * @param lights When false the caller provides shared `WebGLSceneLights` (so
 *   mounting MeshPreview + WebGLBodies together doesn't double the lighting).
 * @param skipSubtractionBodies Opt-in Screen-Space-CSG (v0.6.306): when set, SKIP
 *   every body that `bodyNeedsSubtractionPass` flags (a Difference with a real
 *   subtrahend). Those are drawn EXCLUSIVELY by `ScreenSpaceCsgPass`, so their
 *   coarse DC mesh must not also draw underneath. Off → exact current behaviour.
 */
export function MeshPreview({
  parametricSplit = false,
  lights = true,
  skipSubtractionBodies = false,
}: {
  parametricSplit?: boolean;
  lights?: boolean;
  skipSubtractionBodies?: boolean;
} = {}) {
  const { store, state } = useKernelStore();
  const bodies = state?.meshedBodies;
  const features = state?.features;

  const materials = useMemo(() => {
    if (!store || !features) return new Map<number, MaterialProps>();
    return resolveMaterials(store.kernel, features);
  }, [store, features]);

  if (!bodies || bodies.length === 0) return null;

  const rows = bodies
    .map((body, i) => ({ body, i }))
    .filter(({ i }) => {
      if (parametricSplit && store && isWebglParametricBody(store.kernel, i))
        return false;
      // SS-CSG: Difference-Bodys kommen NUR aus dem CSG-Pass, nicht doppelt als
      // DC-Mesh. bodyHybridInfo kann werfen → defensiv überspringen-Entscheidung.
      if (skipSubtractionBodies && store) {
        try {
          if (bodyHandledByScsPass(store.kernel.bodyHybridInfo(i).leaves))
            return false;
        } catch {
          /* im Zweifel zeichnen (heutiges Verhalten) */
        }
      }
      return true;
    });
  if (rows.length === 0) return null;

  return (
    <>
      {lights ? <WebGLSceneLights /> : null}
      {rows.map(({ body, i }) => (
        <MeshPreviewBody
          key={body.id}
          body={body}
          mat={materials.get(i)}
          bodyIndex={i}
        />
      ))}
    </>
  );
}

/**
 * Hybrid-Renderer, Phase 1 (`brep` render mode; docs/ADAPTIVE-RENDERING-PLAN.md
 * Achse 2): render every body whose analytic B-Rep surface is fully PLANAR
 * (boxes, prisms, planar boolean results) as an exact rasterized mesh — sharp
 * edges, exact normals, ZERO raymarching. Bodies with curved/implicit faces are
 * omitted here (they still show their analytic wireframe via BRepEdgeOverlay);
 * curved-face tessellation + SDF-residual compositing are the next phases.
 *
 * The triangulation is pure + unit-tested (`brepMesh.ts`); this component only
 * lifts it onto the GPU, rebuilding on the body fingerprint like the other
 * B-Rep overlays.
 */
export function BRepMeshPreview() {
  const { store } = useKernelStore();
  const fingerprint = useBodyFingerprint();

  const geometries = useMemo(() => {
    if (!store)
      return [] as { bodyIndex: number; geometry: THREE.BufferGeometry }[];
    let meshes;
    try {
      meshes = collectAnalyticBodyMeshes(
        store.kernel as unknown as BrepMeshKernel,
      );
    } catch {
      return [];
    }
    return meshes.map((m) => {
      const geometry = new THREE.BufferGeometry();
      geometry.setAttribute(
        "position",
        new THREE.BufferAttribute(m.positions, 3),
      );
      geometry.setAttribute("normal", new THREE.BufferAttribute(m.normals, 3));
      geometry.computeBoundingSphere();
      return { bodyIndex: m.bodyIndex, geometry };
    });
    // fingerprint drives the rebuild; store identity is stable within a session.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [store, fingerprint]);

  useEffect(() => {
    return () => {
      for (const g of geometries) g.geometry.dispose();
    };
  }, [geometries]);

  if (geometries.length === 0) return null;

  return (
    <>
      <ambientLight intensity={0.5} />
      <directionalLight position={[200, 400, 300]} intensity={0.8} />
      <directionalLight position={[-100, 200, -200]} intensity={0.3} />
      {geometries.map(({ bodyIndex, geometry }) => (
        <mesh key={bodyIndex} geometry={geometry}>
          <meshStandardMaterial color="#94a3b8" side={THREE.DoubleSide} />
        </mesh>
      ))}
    </>
  );
}

/**
 * WebGL render path body meshes (mode `webgl`; docs/ADAPTIVE-RENDERING-PLAN.md
 * Achse 3). Renders each body's PARAMETRIC hull meshes — box/sphere/cylinder/
 * cone/torus/capsule + extrude/revolve via `createPrimitiveGeometry` — with
 * classic `MeshStandardMaterial`, so the scene draws on a plain WebGLRenderer
 * (no WebGPU, no node materials). Bodies are shown individually (no boolean
 * compositing yet — that is the screen-space-CSG phase); helix/twist parametric
 * meshes and the B-Rep tessellation slot in here later.
 */
/**
 * @param onlyParametric Ticket 55ea: when set (the WebGL path WITH a DC mesh
 *   available), render ONLY bodies that `isWebglParametricBody` flags — the
 *   complement of what `MeshPreview parametricSplit` draws. When unset (the
 *   mesh-pending fallback) every body is drawn from its hulls.
 * @param lights When false the caller provides shared `WebGLSceneLights`.
 * @param skipSubtractionBodies Opt-in Screen-Space-CSG (v0.6.306): when set, SKIP
 *   every body that `bodyNeedsSubtractionPass` flags — those are composited
 *   exclusively by `ScreenSpaceCsgPass`. Used in the mesh-pending fallback so a
 *   Difference body doesn't briefly draw its non-boolean minuend hull underneath
 *   the CSG composite. Off → exact current behaviour.
 */
export function WebGLBodies({
  onlyParametric = false,
  lights = true,
  skipSubtractionBodies = false,
}: {
  onlyParametric?: boolean;
  lights?: boolean;
  skipSubtractionBodies?: boolean;
} = {}) {
  const { store, state } = useKernelStore();
  const fingerprint = useBodyFingerprint();
  // One material per body, so the WebGL path matches WebGPU: distinct part
  // colours (BODY_COLORS), user materials, and the translucent-red merge-scope
  // warning material all come through `resolveMaterials` (ticket 59c2).
  const matsRef = useRef<THREE.MeshStandardMaterial[]>([]);

  const meshes = useMemo(() => {
    for (const m of matsRef.current) m.dispose();
    matsRef.current = [];
    const out: THREE.Mesh[] = [];
    if (!store) return out;
    let count = 0;
    try {
      count = store.kernel.bodyCount();
    } catch {
      return out;
    }
    const userMats = state?.features
      ? resolveMaterials(store.kernel, state.features)
      : new Map<number, MaterialProps>();
    const materialForBody = (i: number): THREE.MeshStandardMaterial => {
      const um = userMats.get(i);
      const [r, g, b] = um ? um.color : BODY_COLORS[i % BODY_COLORS.length];
      const mat = new THREE.MeshStandardMaterial({
        color: new THREE.Color(r, g, b),
        side: THREE.DoubleSide,
        metalness: um ? um.metalness : WEBGL_DEFAULT_METALNESS,
        roughness: um ? um.roughness : WEBGL_DEFAULT_ROUGHNESS,
        emissive: um
          ? new THREE.Color(um.emissive[0], um.emissive[1], um.emissive[2])
          : new THREE.Color(0, 0, 0),
        transparent: um ? um.opacity < 1 : false,
        opacity: um ? um.opacity : 1,
        wireframe: um ? um.wireframe : false,
      });
      matsRef.current.push(mat);
      return mat;
    };
    for (let i = 0; i < count; i++) {
      // 55ea: in the split path render only the parametric-clean bodies; the
      // rest keep their boolean-correct DC mesh via MeshPreview.
      if (onlyParametric && !isWebglParametricBody(store.kernel, i)) continue;
      let info: { leaves: HybridLeafInfo[] };
      try {
        info = store.kernel.bodyHybridInfo(i);
      } catch {
        continue;
      }
      // SS-CSG an: Difference- UND Intersection-Bodys kommen ausschließlich aus
      // dem CSG-Pass.
      if (skipSubtractionBodies && bodyHandledByScsPass(info.leaves))
        continue;
      const mat = materialForBody(i);
      for (const leaf of info.leaves) {
        if (leaf.role === "skip") continue;
        if (leaf.warp) {
          // Twist/Bend: geometry is pre-warped into world space → identity.
          const geo = buildWarpedLeafGeometry(leaf);
          if (!geo) continue;
          out.push(new THREE.Mesh(geo, mat));
          continue;
        }
        const geo = createPrimitiveGeometry(leaf);
        if (!geo) continue;
        const mesh = new THREE.Mesh(geo, mat);
        applyLeafTransform(mesh, leaf);
        out.push(mesh);
      }
    }
    return out;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [store, fingerprint, state?.features, onlyParametric, skipSubtractionBodies]);

  useEffect(() => {
    return () => {
      for (const m of meshes) m.geometry.dispose();
    };
  }, [meshes]);
  useEffect(
    () => () => {
      for (const m of matsRef.current) m.dispose();
    },
    [],
  );

  if (meshes.length === 0) return null;
  return (
    <>
      {lights ? <WebGLSceneLights /> : null}
      {meshes.map((m, i) => (
        <primitive key={i} object={m} dispose={null} />
      ))}
    </>
  );
}
