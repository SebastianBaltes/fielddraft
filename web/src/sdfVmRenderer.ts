/**
 * Per-body forward SDF renderer using the GPU bytecode VM.
 *
 * Each body gets its own AABB box mesh (BackSide). The fragment shader
 * evaluates the VM opcode buffer and sphere-traces from the camera to
 * the box exit point. Depth is written per-hit so THREE.js composites
 * bodies via the hardware depth buffer.
 *
 * Zero multi-pass overhead — serves as a performance baseline against
 * the deferred pipeline.
 */

// @ts-nocheck — TSL's TypeScript surface in three r183 is intentionally loose.

import * as THREE from "three";
import {
  Break,
  Discard,
  Fn,
  If,
  Loop,
  cameraPosition,
  cameraProjectionMatrix,
  cameraViewMatrix,
  float,
  int,
  max,
  normalize,
  modelWorldMatrix,
  positionGeometry,
  property,
  uniform,
  vec3,
  vec4,
  uniformArray,
} from "three/tsl";
import { MeshBasicNodeMaterial } from "three/webgpu";

import { NORMAL_EPS } from "./sdfRenderer";
import { buildVmSdfEvalFn, VM_MAX_OPCODES } from "./sdfVmInterpreter";
import { BODY_COLORS } from "./sdfDeferredRenderer";

// ── Constants ───────────────────────────────────────────────────────

const MARCH_STEPS = 512;
const SURFACE_EPS = 0.002;
const AABB_PAD = 0.5;

// ── Types ───────────────────────────────────────────────────────────

export interface SdfVmBodyHandle {
  mesh: THREE.Mesh;
  material: MeshBasicNodeMaterial;
  opcodeBuf: ReturnType<typeof uniformArray>;
  opcodeData: Float32Array;
  opcodeLen: number;
}

// ── Body management ─────────────────────────────────────────────────

export function createSdfVmBody(
  opcodes: Float32Array,
  aabb: Float32Array,
  bodyIndex: number = 0,
): SdfVmBodyHandle {
  const opcodeData = new Float32Array(VM_MAX_OPCODES);
  opcodeData.set(opcodes.subarray(0, Math.min(opcodes.length, VM_MAX_OPCODES)));
  const opcodeLen = Math.min(opcodes.length, VM_MAX_OPCODES);

  const opcodeBuf = uniformArray(opcodeData, "float");
  const sceneSdf = buildVmSdfEvalFn(opcodeBuf);

  const [cr, cg, cb] = BODY_COLORS[bodyIndex % BODY_COLORS.length];
  const bodyColorU = uniform(new THREE.Vector3(cr, cg, cb));

  const hitDepthProp = property("float", "hitDepth");

  const colorNode = Fn(() => {
    const worldPos = modelWorldMatrix.mul(vec4(positionGeometry, 1)).xyz;
    const ro = cameraPosition;
    const rd = normalize(worldPos.sub(ro));
    const tMax = worldPos.sub(ro).length();

    const t = float(0).toVar();
    const hit = float(0).toVar();
    const hitPos = vec3(0).toVar();

    Loop(int(MARCH_STEPS), () => {
      const p = ro.add(rd.mul(t));
      const d = sceneSdf({ p });
      If(d.lessThan(float(SURFACE_EPS)), () => {
        hit.assign(1);
        hitPos.assign(p);
        Break();
      });
      t.addAssign(max(d, float(SURFACE_EPS)));
      If(t.greaterThan(tMax), () => {
        Break();
      });
    });

    const finalColor = vec3(0).toVar();
    If(hit.greaterThan(float(0.5)), () => {
      const eps = float(NORMAL_EPS);
      const ex = vec3(eps, 0, 0);
      const ey = vec3(0, eps, 0);
      const ez = vec3(0, 0, eps);
      const n = normalize(
        vec3(
          sceneSdf({ p: hitPos.add(ex) }).sub(sceneSdf({ p: hitPos.sub(ex) })),
          sceneSdf({ p: hitPos.add(ey) }).sub(sceneSdf({ p: hitPos.sub(ey) })),
          sceneSdf({ p: hitPos.add(ez) }).sub(sceneSdf({ p: hitPos.sub(ez) })),
        ),
      );

      const keyDir = normalize(vec3(0.4, 1, 0.5));
      const fillDir = normalize(vec3(-0.6, 0.2, 0.8));
      const rimDir = normalize(vec3(0.0, -0.3, -1.0));
      const dKey = max(n.dot(keyDir), float(0));
      const dFill = max(n.dot(fillDir), float(0));
      const dRim = max(n.dot(rimDir), float(0));
      const light = dKey.mul(0.45).add(dFill.mul(0.2)).add(dRim.mul(0.1)).add(0.3);
      finalColor.assign(bodyColorU.mul(light));

      const clipPos = cameraProjectionMatrix.mul(cameraViewMatrix).mul(vec4(hitPos, 1));
      hitDepthProp.assign(clipPos.z.div(clipPos.w));
    }).Else(() => {
      Discard();
    });

    return vec4(finalColor, 1);
  });

  const material = new MeshBasicNodeMaterial();
  material.outputNode = colorNode();
  material.depthNode = hitDepthProp;
  material.side = THREE.BackSide;
  material.depthWrite = true;
  material.depthTest = true;
  material.toneMapped = false;

  const minX = aabb[0], minY = aabb[1], minZ = aabb[2];
  const maxX = aabb[3], maxY = aabb[4], maxZ = aabb[5];
  const sx = maxX - minX + AABB_PAD * 2;
  const sy = maxY - minY + AABB_PAD * 2;
  const sz = maxZ - minZ + AABB_PAD * 2;
  const cx = (minX + maxX) * 0.5;
  const cy = (minY + maxY) * 0.5;
  const cz = (minZ + maxZ) * 0.5;

  const geometry = new THREE.BoxGeometry(sx, sy, sz);
  const mesh = new THREE.Mesh(geometry, material);
  mesh.position.set(cx, cy, cz);
  mesh.frustumCulled = false;

  return { mesh, material, opcodeBuf, opcodeData, opcodeLen };
}

export function updateSdfVmBody(
  handle: SdfVmBodyHandle,
  opcodes: Float32Array,
  aabb: Float32Array,
): SdfVmBodyHandle {
  const newLen = Math.min(opcodes.length, VM_MAX_OPCODES);

  handle.opcodeData.fill(0);
  handle.opcodeData.set(opcodes.subarray(0, newLen));
  handle.opcodeLen = newLen;

  const minX = aabb[0], minY = aabb[1], minZ = aabb[2];
  const maxX = aabb[3], maxY = aabb[4], maxZ = aabb[5];
  handle.mesh.position.set(
    (minX + maxX) * 0.5,
    (minY + maxY) * 0.5,
    (minZ + maxZ) * 0.5,
  );

  const sx = maxX - minX + AABB_PAD * 2;
  const sy = maxY - minY + AABB_PAD * 2;
  const sz = maxZ - minZ + AABB_PAD * 2;
  const geom = handle.mesh.geometry as THREE.BoxGeometry;
  const gp = geom.parameters;
  if (
    Math.abs(gp.width - sx) > 0.01 ||
    Math.abs(gp.height - sy) > 0.01 ||
    Math.abs(gp.depth - sz) > 0.01
  ) {
    geom.dispose();
    handle.mesh.geometry = new THREE.BoxGeometry(sx, sy, sz);
  }

  return handle;
}

export function disposeSdfVmBody(handle: SdfVmBodyHandle): void {
  handle.mesh.geometry.dispose();
  handle.material.dispose();
}
