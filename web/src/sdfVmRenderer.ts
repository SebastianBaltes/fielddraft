/**
 * GPU Bytecode VM SDF renderer.
 *
 * A single fixed TSL expression tree interprets the opcode buffer at
 * runtime on the GPU.  Topology changes update only the uniform buffer
 * (~0.1 ms), never triggering shader recompilation.  The pipeline is
 * compiled once and reused across all bodies.
 *
 * Trade-off vs. transpiled mode: ~15-40% slower per pixel due to
 * branch divergence, but topology changes are instant.
 */

// @ts-nocheck — TSL's TypeScript surface in three r183 is intentionally
// loose; we keep this file un-typed like sdfRenderer.ts.

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
  modelWorldMatrix,
  normalize,
  positionGeometry,
  property,
  vec3,
  vec4,
  uniformArray,
} from "three/tsl";
import { MeshBasicNodeMaterial } from "three/webgpu";

import { MAX_STEPS, SURFACE_EPS, NORMAL_EPS, MAX_DIST } from "./sdfRenderer";
import { buildVmSdfEvalFn, VM_MAX_OPCODES } from "./sdfVmInterpreter";

// ── Material creation ───────────────────────────────────────────────

export interface SdfVmBodyHandle {
  mesh: THREE.Mesh;
  material: MeshBasicNodeMaterial;
  opcodeBuf: ReturnType<typeof uniformArray>;
  opcodeData: Float32Array;
  opcodeLen: number;
}

export function createSdfVmBody(
  opcodes: Float32Array,
  aabb: Float32Array,
): SdfVmBodyHandle {
  console.time("sdf-vm:create");

  const opcodeData = new Float32Array(VM_MAX_OPCODES);
  opcodeData.set(opcodes.subarray(0, Math.min(opcodes.length, VM_MAX_OPCODES)));
  const opcodeLen = Math.min(opcodes.length, VM_MAX_OPCODES);

  const opcodeBuf = uniformArray(opcodeData, "float");

  const sceneSdf = buildVmSdfEvalFn(opcodeBuf);

  const sceneNormal = Fn(({ p }: { p: any }) => {
    const ex = vec3(NORMAL_EPS, 0, 0);
    const ey = vec3(0, NORMAL_EPS, 0);
    const ez = vec3(0, 0, NORMAL_EPS);
    return normalize(
      vec3(
        sceneSdf({ p: p.add(ex) }).sub(sceneSdf({ p: p.sub(ex) })),
        sceneSdf({ p: p.add(ey) }).sub(sceneSdf({ p: p.sub(ey) })),
        sceneSdf({ p: p.add(ez) }).sub(sceneSdf({ p: p.sub(ez) })),
      ),
    );
  });

  const hitDepthProp = property('float', 'hitDepth');

  const colorNode = Fn(() => {
    const worldPos = modelWorldMatrix.mul(vec4(positionGeometry, 1)).xyz;
    const ro = cameraPosition;
    const rd = normalize(worldPos.sub(ro));
    const t = float(0).toVar();
    const hit = float(0).toVar();
    const hitPos = vec3(0).toVar();

    Loop(int(MAX_STEPS), () => {
      const p = ro.add(rd.mul(t));
      const d = sceneSdf({ p });
      If(d.lessThan(SURFACE_EPS), () => {
        hit.assign(1);
        hitPos.assign(p);
        Break();
      });
      t.addAssign(max(d.mul(0.8), float(SURFACE_EPS)));
      If(t.greaterThan(MAX_DIST), () => {
        Break();
      });
    });

    const finalColor = vec3(0).toVar();
    If(hit.greaterThan(0.5), () => {
      const n = sceneNormal({ p: hitPos });
      const key = normalize(vec3(0.4, 1, 0.5));
      const fill = normalize(vec3(-0.6, 0.2, 0.8));
      const rim = normalize(vec3(0.0, -0.3, -1.0));
      const dKey = max(n.dot(key), float(0));
      const dFill = max(n.dot(fill), float(0));
      const dRim = max(n.dot(rim), float(0));
      const ambient = float(0.3);
      const light = dKey
        .mul(0.45)
        .add(dFill.mul(0.2))
        .add(dRim.mul(0.1))
        .add(ambient);
      const baseColor = vec3(0.55, 0.65, 0.85);
      finalColor.assign(baseColor.mul(light));
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
  const margin = 2.0;
  const sx = maxX - minX + margin * 2;
  const sy = maxY - minY + margin * 2;
  const sz = maxZ - minZ + margin * 2;
  const cx = (minX + maxX) * 0.5;
  const cy = (minY + maxY) * 0.5;
  const cz = (minZ + maxZ) * 0.5;

  const geometry = new THREE.BoxGeometry(sx, sy, sz);
  const mesh = new THREE.Mesh(geometry, material);
  mesh.position.set(cx, cy, cz);
  mesh.frustumCulled = false;

  console.timeEnd("sdf-vm:create");

  return { mesh, material, opcodeBuf, opcodeData, opcodeLen };
}

export function updateSdfVmBody(
  handle: SdfVmBodyHandle,
  opcodes: Float32Array,
  aabb: Float32Array,
): SdfVmBodyHandle {
  const newLen = Math.min(opcodes.length, VM_MAX_OPCODES);

  console.time("sdf-vm:bufferUpdate");
  handle.opcodeData.fill(0);
  handle.opcodeData.set(opcodes.subarray(0, newLen));
  handle.opcodeLen = newLen;
  console.timeEnd("sdf-vm:bufferUpdate");

  const minX = aabb[0], minY = aabb[1], minZ = aabb[2];
  const maxX = aabb[3], maxY = aabb[4], maxZ = aabb[5];
  const margin = 2.0;
  const cx = (minX + maxX) * 0.5;
  const cy = (minY + maxY) * 0.5;
  const cz = (minZ + maxZ) * 0.5;
  handle.mesh.position.set(cx, cy, cz);

  const sx = maxX - minX + margin * 2;
  const sy = maxY - minY + margin * 2;
  const sz = maxZ - minZ + margin * 2;
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
