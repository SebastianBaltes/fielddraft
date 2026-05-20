/**
 * Baked 3D Distance Field SDF renderer.
 *
 * Pre-computes the SDF on a 3D grid via compute shader, stores it in a
 * Storage3DTexture (R32Float), then raymarches with O(1) trilinear
 * texture lookups per step — independent of CSG complexity.
 *
 * Progressive refinement: 64³ → 128³ → 256³ as the scene settles.
 *
 * Trade-off vs. transpiled/VM modes: loses sharp edges at low res,
 * uses more GPU memory (8–64 MB/body), but per-step cost is constant.
 */

// @ts-nocheck — TSL's TypeScript surface in three r183 is intentionally loose.

import * as THREE from "three";
import {
  Break,
  Fn,
  If,
  Loop,
  abs,
  cameraPosition,
  clamp,
  float,
  int,
  instanceIndex,
  max,
  min,
  modelWorldMatrix,
  normalize,
  positionGeometry,
  texture3D,
  textureStore,
  uint,
  uniformArray,
  uvec3,
  vec2,
  vec3,
  vec4,
  wgslFn,
} from "three/tsl";
import { MeshBasicNodeMaterial, Storage3DTexture } from "three/webgpu";

import {
  OP_PRIM_BOX, OP_PRIM_SPHERE, OP_PRIM_CYLINDER, OP_PRIM_TORUS,
  OP_PRIM_CONE, OP_PRIM_CAPSULE, OP_PRIM_HEX_PRISM, OP_PRIM_ELLIPSOID,
  OP_PRIM_CONST, OP_PRIM_GYROID, OP_PRIM_THREAD,
  OP_PRIM2D_CIRCLE, OP_PRIM2D_RECT, OP_PRIM2D_SECTOR,
  OP_UNION, OP_DIFFERENCE, OP_INTERSECT, OP_SMOOTH_UNION, OP_SMOOTH_DIFF,
  OP_SHELL, OP_MORPH, OP_DISPLACE_VAL, OP_EXTRUDE, OP_REVOLVE_SCALE,
  OP_OFFSET_2D,
  OP_WARP_PUSH, OP_WARP_POP, OP_WARP_TWIST, OP_WARP_BEND,
  OP_WARP_TRANSFORM, OP_WARP_REVOLVE,
  OP_MASKED_BLEND, OP_END,
  worldToLocalFn, sdfBoxFn, sdfSphereFn, sdfCylinderFn,
  sdfTorusFn, sdfConeFn, sdfCapsuleFn, sdfHexPrismFn,
  sdfEllipsoidFn, sdfGyroidFn, sdfThreadFn,
  blendMinFn, sdfCapsule3dFn, sdfExtrudeFn,
  sdfRect2dFn, sdfCircle2dFn, sdfSector2dFn,
  MAX_STEPS, SURFACE_EPS, NORMAL_EPS, MAX_DIST,
} from "./sdfRenderer";

// ── Constants ───────────────────────────────────────────────────────

const VM_MAX_OPCODES = 2048;

export type BakeResolution = 64 | 128 | 256;

const bitcastTagFn = wgslFn(`
  fn fd_bitcast_tag(v: f32) -> i32 {
    return i32(bitcast<u32>(v));
  }
`);

// ── VM interpreter for compute shader ───────────────────────────────
// Reuses the same If/ElseIf stack-machine interpreter as sdfVmRenderer,
// but runs inside a compute dispatch instead of a fragment shader.

function buildComputeVmEvalFn(opBuf: ReturnType<typeof uniformArray>) {
  return Fn(({ p: pIn }: { p: any }) => {
    const pc = int(0).toVar();

    const vs0 = float(0).toVar(), vs1 = float(0).toVar();
    const vs2 = float(0).toVar(), vs3 = float(0).toVar();
    const vs4 = float(0).toVar(), vs5 = float(0).toVar();
    const vs6 = float(0).toVar(), vs7 = float(0).toVar();
    const vsp = int(0).toVar();

    const ps0x = float(0).toVar(), ps0y = float(0).toVar(), ps0z = float(0).toVar();
    const ps1x = float(0).toVar(), ps1y = float(0).toVar(), ps1z = float(0).toVar();
    const ps2x = float(0).toVar(), ps2y = float(0).toVar(), ps2z = float(0).toVar();
    const ps3x = float(0).toVar(), ps3y = float(0).toVar(), ps3z = float(0).toVar();
    const psp = int(0).toVar();

    const ptx = pIn.x.toVar();
    const pty = pIn.y.toVar();
    const ptz = pIn.z.toVar();

    const rd = () => {
      const v = opBuf.element(pc).toVar();
      pc.addAssign(1);
      return v;
    };

    const pushV = (val: any) => {
      If(vsp.equal(0), () => { vs0.assign(val); })
        .ElseIf(vsp.equal(1), () => { vs1.assign(val); })
        .ElseIf(vsp.equal(2), () => { vs2.assign(val); })
        .ElseIf(vsp.equal(3), () => { vs3.assign(val); })
        .ElseIf(vsp.equal(4), () => { vs4.assign(val); })
        .ElseIf(vsp.equal(5), () => { vs5.assign(val); })
        .ElseIf(vsp.equal(6), () => { vs6.assign(val); })
        .ElseIf(vsp.equal(7), () => { vs7.assign(val); });
      vsp.addAssign(1);
    };

    const readVs = (idx: any) => {
      const out = float(0).toVar();
      If(idx.equal(0), () => { out.assign(vs0); })
        .ElseIf(idx.equal(1), () => { out.assign(vs1); })
        .ElseIf(idx.equal(2), () => { out.assign(vs2); })
        .ElseIf(idx.equal(3), () => { out.assign(vs3); })
        .ElseIf(idx.equal(4), () => { out.assign(vs4); })
        .ElseIf(idx.equal(5), () => { out.assign(vs5); })
        .ElseIf(idx.equal(6), () => { out.assign(vs6); })
        .ElseIf(idx.equal(7), () => { out.assign(vs7); });
      return out;
    };

    const popV = () => {
      vsp.subAssign(1);
      return readVs(vsp);
    };

    const pushP = (x: any, y: any, z: any) => {
      If(psp.equal(0), () => { ps0x.assign(x); ps0y.assign(y); ps0z.assign(z); })
        .ElseIf(psp.equal(1), () => { ps1x.assign(x); ps1y.assign(y); ps1z.assign(z); })
        .ElseIf(psp.equal(2), () => { ps2x.assign(x); ps2y.assign(y); ps2z.assign(z); })
        .ElseIf(psp.equal(3), () => { ps3x.assign(x); ps3y.assign(y); ps3z.assign(z); });
      psp.addAssign(1);
    };

    const popP = () => {
      psp.subAssign(1);
      const rx = float(0).toVar(), ry = float(0).toVar(), rz = float(0).toVar();
      If(psp.equal(0), () => { rx.assign(ps0x); ry.assign(ps0y); rz.assign(ps0z); })
        .ElseIf(psp.equal(1), () => { rx.assign(ps1x); ry.assign(ps1y); rz.assign(ps1z); })
        .ElseIf(psp.equal(2), () => { rx.assign(ps2x); ry.assign(ps2y); rz.assign(ps2z); })
        .ElseIf(psp.equal(3), () => { rx.assign(ps3x); ry.assign(ps3y); rz.assign(ps3z); });
      ptx.assign(rx);
      pty.assign(ry);
      ptz.assign(rz);
    };

    const emitTransformAndLocal = () => {
      const tx = rd(), ty = rd(), tz = rd();
      const qx = rd(), qy = rd(), qz = rd(), qw = rd();
      const scaleVal = rd();
      const local = worldToLocalFn({
        p: vec3(ptx, pty, ptz),
        translate: vec3(tx, ty, tz),
        quat: vec4(qx, qy, qz, qw),
        scale: scaleVal,
      }).toVar();
      return { local, scale: scaleVal };
    };

    Loop(int(VM_MAX_OPCODES), () => {
      const rawTag = opBuf.element(pc).toVar();
      pc.addAssign(1);
      const tag = bitcastTagFn({ v: rawTag });

      If(tag.equal(int(OP_PRIM_BOX)), () => {
        const { local, scale: s } = emitTransformAndLocal();
        const hx = rd(), hy = rd(), hz = rd();
        pushV(sdfBoxFn({ p: local, h: vec3(hx, hy, hz) }).mul(s));
      }).ElseIf(tag.equal(int(OP_PRIM_SPHERE)), () => {
        const { local, scale: s } = emitTransformAndLocal();
        const r = rd();
        pushV(sdfSphereFn({ p: local, r }).mul(s));
      }).ElseIf(tag.equal(int(OP_PRIM_CYLINDER)), () => {
        const { local, scale: s } = emitTransformAndLocal();
        const r = rd(), hh = rd();
        pushV(sdfCylinderFn({ p: local, r, hh }).mul(s));
      }).ElseIf(tag.equal(int(OP_PRIM_TORUS)), () => {
        const { local, scale: s } = emitTransformAndLocal();
        const major = rd(), minor = rd();
        pushV(sdfTorusFn({ p: local, major, minor }).mul(s));
      }).ElseIf(tag.equal(int(OP_PRIM_CONE)), () => {
        const { local, scale: s } = emitTransformAndLocal();
        const rb = rd(), rt = rd(), hh = rd();
        pushV(sdfConeFn({ p: local, rb, rt, hh }).mul(s));
      }).ElseIf(tag.equal(int(OP_PRIM_CAPSULE)), () => {
        const { local, scale: s } = emitTransformAndLocal();
        const r = rd(), hh = rd();
        pushV(sdfCapsuleFn({ p: local, r, hh }).mul(s));
      }).ElseIf(tag.equal(int(OP_PRIM_HEX_PRISM)), () => {
        const { local, scale: s } = emitTransformAndLocal();
        const apothem = rd(), hh = rd();
        pushV(sdfHexPrismFn({ p: local, apothem, hh }).mul(s));
      }).ElseIf(tag.equal(int(OP_PRIM_ELLIPSOID)), () => {
        const { local, scale: s } = emitTransformAndLocal();
        const rx = rd(), ry = rd(), rz = rd();
        pushV(sdfEllipsoidFn({ p: local, radii: vec3(rx, ry, rz) }).mul(s));
      }).ElseIf(tag.equal(int(OP_PRIM_CONST)), () => {
        pushV(rd());
      }).ElseIf(tag.equal(int(OP_PRIM_GYROID)), () => {
        const { local, scale: s } = emitTransformAndLocal();
        const cell_size = rd(), thickness = rd();
        pushV(sdfGyroidFn({ p: local, cell_size, thickness }).mul(s));
      }).ElseIf(tag.equal(int(OP_PRIM_THREAD)), () => {
        const { local, scale: s } = emitTransformAndLocal();
        const nom_r = rd(), pitch = rd(), depth = rd(), hh = rd();
        pushV(sdfThreadFn({ p: local, nom_r, pitch, depth, hh }).mul(s));

      }).ElseIf(tag.equal(int(OP_PRIM2D_CIRCLE)), () => {
        const cx = rd(), cy = rd(), r = rd();
        pushV(sdfCircle2dFn({ p: vec2(ptx, pty), center: vec2(cx, cy), r }));
      }).ElseIf(tag.equal(int(OP_PRIM2D_RECT)), () => {
        const cx = rd(), cy = rd(), hx = rd(), hy = rd();
        pushV(sdfRect2dFn({ p: vec2(ptx, pty), center: vec2(cx, cy), half: vec2(hx, hy) }));
      }).ElseIf(tag.equal(int(OP_PRIM2D_SECTOR)), () => {
        const cx = rd(), cy = rd(), r = rd();
        const start_angle = rd(), sweep_angle = rd();
        pushV(sdfSector2dFn({ p: vec2(ptx, pty), cx, cy, r, start_angle, sweep_angle }));

      }).ElseIf(tag.equal(int(OP_UNION)), () => {
        const b = popV(), a = popV();
        pushV(min(a, b));
      }).ElseIf(tag.equal(int(OP_DIFFERENCE)), () => {
        const b = popV(), a = popV();
        pushV(max(a, b.negate()));
      }).ElseIf(tag.equal(int(OP_INTERSECT)), () => {
        const b = popV(), a = popV();
        pushV(max(a, b));
      }).ElseIf(tag.equal(int(OP_SMOOTH_UNION)), () => {
        const r = rd(), mode = rd();
        const b = popV(), a = popV();
        pushV(blendMinFn({ a, b, r, mode }));
      }).ElseIf(tag.equal(int(OP_SMOOTH_DIFF)), () => {
        const r = rd(), mode = rd();
        const b = popV(), a = popV();
        pushV(blendMinFn({ a: a.negate(), b, r, mode }).negate());

      }).ElseIf(tag.equal(int(OP_SHELL)), () => {
        const thickness = rd();
        const d = popV();
        pushV(abs(d).sub(thickness));
      }).ElseIf(tag.equal(int(OP_MORPH)), () => {
        const t = rd();
        const b = popV(), a = popV();
        pushV(float(1).sub(t).mul(a).add(t.mul(b)));
      }).ElseIf(tag.equal(int(OP_DISPLACE_VAL)), () => {
        const dpth = rd(), freq = rd();
        const d = popV();
        const sn = Fn(() => {
          const sx = float(ptx.mul(freq)).toVar();
          const sy = float(pty.mul(freq).mul(1.3)).toVar();
          const sz = float(ptz.mul(freq).mul(0.7)).toVar();
          return float(0).add(sx.sin().mul(sy.sin()).mul(sz.sin()));
        });
        pushV(d.sub(dpth.mul(sn())));
      }).ElseIf(tag.equal(int(OP_EXTRUDE)), () => {
        const half_h = rd(), s = rd();
        const d2d = popV();
        pushV(sdfExtrudeFn({ d2d, z: ptz, half_h }).mul(s));
      }).ElseIf(tag.equal(int(OP_REVOLVE_SCALE)), () => {
        const s = rd();
        pushV(popV().mul(s));
      }).ElseIf(tag.equal(int(OP_OFFSET_2D)), () => {
        const dist = rd();
        pushV(popV().sub(dist));

      }).ElseIf(tag.equal(int(OP_WARP_PUSH)), () => {
        pushP(ptx, pty, ptz);
      }).ElseIf(tag.equal(int(OP_WARP_POP)), () => {
        popP();
      }).ElseIf(tag.equal(int(OP_WARP_TWIST)), () => {
        const rate = rd();
        const angle = rate.mul(pty);
        const sn = angle.sin(), cs = angle.cos();
        const oldX = ptx.toVar();
        ptx.assign(cs.mul(oldX).sub(sn.mul(ptz)));
        ptz.assign(sn.mul(oldX).add(cs.mul(ptz)));
      }).ElseIf(tag.equal(int(OP_WARP_BEND)), () => {
        const rate = rd();
        const angle = rate.mul(ptx);
        const sn = angle.sin(), cs = angle.cos();
        const oldX = ptx.toVar();
        ptx.assign(cs.mul(oldX).sub(sn.mul(pty)));
        pty.assign(sn.mul(oldX).add(cs.mul(pty)));
      }).ElseIf(tag.equal(int(OP_WARP_TRANSFORM)), () => {
        const tx = rd(), ty = rd(), tz = rd();
        const qx = rd(), qy = rd(), qz = rd(), qw = rd();
        const s = rd();
        const local = worldToLocalFn({
          p: vec3(ptx, pty, ptz),
          translate: vec3(tx, ty, tz),
          quat: vec4(qx, qy, qz, qw),
          scale: s,
        }).toVar();
        ptx.assign(local.x);
        pty.assign(local.y);
        ptz.assign(local.z);
      }).ElseIf(tag.equal(int(OP_WARP_REVOLVE)), () => {
        const r = ptx.mul(ptx).add(ptz.mul(ptz)).sqrt();
        ptx.assign(r);
        ptz.assign(0);

      }).ElseIf(tag.equal(int(OP_MASKED_BLEND)), () => {
        const p0x = rd(), p0y = rd(), p0z = rd();
        const p1x = rd(), p1y = rd(), p1z = rd();
        const maskR = rd(), blendR = rd(), mode = rd();
        const isDiff = rd();
        const db = popV(), da = popV();
        const maskD = sdfCapsule3dFn({
          p: vec3(ptx, pty, ptz),
          a: vec3(p0x, p0y, p0z),
          b: vec3(p1x, p1y, p1z),
          r: maskR,
        });
        const tMask = float(0).step(maskD);
        const uSmooth = blendMinFn({ a: da, b: db, r: blendR, mode });
        const uSharp = min(da, db);
        const dSmooth = blendMinFn({ a: da.negate(), b: db, r: blendR, mode }).negate();
        const dSharp = max(da, db.negate());
        const uResult = uSmooth.mix(uSharp, tMask);
        const dResult = dSmooth.mix(dSharp, tMask);
        pushV(uResult.mix(dResult, isDiff));

      }).ElseIf(tag.equal(int(OP_END)), () => {
        Break();
      });
    });

    const result = float(1e30).toVar();
    If(vsp.greaterThan(0), () => {
      result.assign(readVs(vsp.sub(1)));
    });
    return result;
  });
}

// ── Compute bake kernel ─────────────────────────────────────────────

interface BakeKernel {
  computeNode: any;
  opcodeBuf: ReturnType<typeof uniformArray>;
  opcodeData: Float32Array;
  aabbBuf: ReturnType<typeof uniformArray>;
  aabbData: Float32Array;
  volumeTex: Storage3DTexture;
  res: BakeResolution;
}

function createBakeKernel(res: BakeResolution): BakeKernel {
  const opcodeData = new Float32Array(VM_MAX_OPCODES);
  const opcodeBuf = uniformArray(opcodeData, "float");

  const aabbData = new Float32Array(6);
  const aabbBuf = uniformArray(aabbData, "float");

  const volumeTex = new Storage3DTexture(res, res, res);
  volumeTex.type = THREE.FloatType;
  volumeTex.format = THREE.RedFormat;
  volumeTex.internalFormat = "r32float";

  const sdfEval = buildComputeVmEvalFn(opcodeBuf);

  const bakeKernelFn = Fn(({ volumeTex, aabbBuf }: any) => {
    const resU = uint(res);
    const ix = instanceIndex.mod(resU);
    const iy = instanceIndex.div(resU).mod(resU);
    const iz = instanceIndex.div(resU.mul(resU));
    const coord = uvec3(ix, iy, iz);

    const aabbMinX = aabbBuf.element(0);
    const aabbMinY = aabbBuf.element(1);
    const aabbMinZ = aabbBuf.element(2);
    const aabbMaxX = aabbBuf.element(3);
    const aabbMaxY = aabbBuf.element(4);
    const aabbMaxZ = aabbBuf.element(5);

    const cellSize = vec3(
      aabbMaxX.sub(aabbMinX).div(float(res)),
      aabbMaxY.sub(aabbMinY).div(float(res)),
      aabbMaxZ.sub(aabbMinZ).div(float(res)),
    );

    const worldPos = vec3(
      aabbMinX.add(ix.toFloat().add(0.5).mul(cellSize.x)),
      aabbMinY.add(iy.toFloat().add(0.5).mul(cellSize.y)),
      aabbMinZ.add(iz.toFloat().add(0.5).mul(cellSize.z)),
    );

    const dist = sdfEval({ p: worldPos });

    textureStore(volumeTex, coord, vec4(dist, 0, 0, 0)).toWriteOnly();
  });

  const computeNode = bakeKernelFn({ volumeTex, aabbBuf })
    .compute(res * res * res, [64]);

  return { computeNode, opcodeBuf, opcodeData, aabbBuf, aabbData, volumeTex, res };
}

// ── Volume raymarching material ─────────────────────────────────────

function createVolumeMaterial(
  volumeTex: Storage3DTexture,
  aabbBuf: ReturnType<typeof uniformArray>,
): MeshBasicNodeMaterial {
  const colorNode = Fn(() => {
    const worldPos = modelWorldMatrix.mul(vec4(positionGeometry, 1)).xyz;
    const ro = cameraPosition;
    const rd = normalize(worldPos.sub(ro));

    const aabbMinX = aabbBuf.element(0);
    const aabbMinY = aabbBuf.element(1);
    const aabbMinZ = aabbBuf.element(2);
    const aabbMaxX = aabbBuf.element(3);
    const aabbMaxY = aabbBuf.element(4);
    const aabbMaxZ = aabbBuf.element(5);
    const aabbMin = vec3(aabbMinX, aabbMinY, aabbMinZ);
    const aabbMax = vec3(aabbMaxX, aabbMaxY, aabbMaxZ);
    const aabbSize = aabbMax.sub(aabbMin);

    const t = float(0).toVar();
    const hit = float(0).toVar();
    const hitPos = vec3(0).toVar();

    Loop(int(MAX_STEPS), () => {
      const p = ro.add(rd.mul(t));
      const uvw = p.sub(aabbMin).div(aabbSize);
      const clamped = clamp(uvw, 0, 1);
      const inside = clamped.sub(uvw).length().lessThan(0.001);

      const d = float(0).toVar();
      If(inside, () => {
        d.assign(texture3D(volumeTex, uvw).x);
      }).Else(() => {
        d.assign(float(1.0));
      });

      If(d.lessThan(SURFACE_EPS), () => {
        hit.assign(1);
        hitPos.assign(p);
        Break();
      });
      t.addAssign(max(d, float(SURFACE_EPS)));
      If(t.greaterThan(MAX_DIST), () => {
        Break();
      });
    });

    const finalColor = vec3(0).toVar();
    const finalAlpha = float(0).toVar();
    If(hit.greaterThan(0.5), () => {
      const eps = NORMAL_EPS;
      const ex = vec3(eps, 0, 0);
      const ey = vec3(0, eps, 0);
      const ez = vec3(0, 0, eps);

      const sampleAt = Fn(({ p }: { p: any }) => {
        const uv = p.sub(aabbMin).div(aabbSize);
        return texture3D(volumeTex, clamp(uv, 0, 1)).x;
      });

      const n = normalize(vec3(
        sampleAt({ p: hitPos.add(ex) }).sub(sampleAt({ p: hitPos.sub(ex) })),
        sampleAt({ p: hitPos.add(ey) }).sub(sampleAt({ p: hitPos.sub(ey) })),
        sampleAt({ p: hitPos.add(ez) }).sub(sampleAt({ p: hitPos.sub(ez) })),
      ));

      const key = normalize(vec3(0.4, 1, 0.5));
      const fill = normalize(vec3(-0.6, 0.2, 0.8));
      const rim = normalize(vec3(0.0, -0.3, -1.0));
      const dKey = max(n.dot(key), float(0));
      const dFill = max(n.dot(fill), float(0));
      const dRim = max(n.dot(rim), float(0));
      const ambient = float(0.3);
      const light = dKey.mul(0.45).add(dFill.mul(0.2)).add(dRim.mul(0.1)).add(ambient);
      const baseColor = vec3(0.55, 0.65, 0.85);
      finalColor.assign(baseColor.mul(light));
      finalAlpha.assign(1);
    });

    return vec4(finalColor, finalAlpha);
  });

  const material = new MeshBasicNodeMaterial();
  material.outputNode = colorNode();
  material.side = THREE.BackSide;
  material.transparent = true;
  material.depthWrite = false;
  material.depthTest = false;
  material.toneMapped = false;

  return material;
}

// ── Public API ──────────────────────────────────────────────────────

export interface SdfBakedBodyHandle {
  mesh: THREE.Mesh;
  material: MeshBasicNodeMaterial;
  bakeKernels: Map<BakeResolution, BakeKernel>;
  currentRes: BakeResolution;
  pendingRefine: ReturnType<typeof setTimeout> | null;
  opcodes: Float32Array;
  aabb: Float32Array;
}

export function createSdfBakedBody(
  opcodes: Float32Array,
  aabb: Float32Array,
): SdfBakedBodyHandle {
  console.time("sdf-baked:create");

  const kernel64 = createBakeKernel(64);
  kernel64.opcodeData.set(opcodes.subarray(0, Math.min(opcodes.length, VM_MAX_OPCODES)));

  const margin = 2.0;
  const expandedAabb = new Float32Array(6);
  expandedAabb[0] = aabb[0] - margin;
  expandedAabb[1] = aabb[1] - margin;
  expandedAabb[2] = aabb[2] - margin;
  expandedAabb[3] = aabb[3] + margin;
  expandedAabb[4] = aabb[4] + margin;
  expandedAabb[5] = aabb[5] + margin;
  kernel64.aabbData.set(expandedAabb);

  const material = createVolumeMaterial(kernel64.volumeTex, kernel64.aabbBuf);

  const minX = expandedAabb[0], minY = expandedAabb[1], minZ = expandedAabb[2];
  const maxX = expandedAabb[3], maxY = expandedAabb[4], maxZ = expandedAabb[5];
  const sx = maxX - minX;
  const sy = maxY - minY;
  const sz = maxZ - minZ;
  const cx = (minX + maxX) * 0.5;
  const cy = (minY + maxY) * 0.5;
  const cz = (minZ + maxZ) * 0.5;

  const geometry = new THREE.BoxGeometry(sx, sy, sz);
  const mesh = new THREE.Mesh(geometry, material);
  mesh.position.set(cx, cy, cz);
  mesh.frustumCulled = false;

  const bakeKernels = new Map<BakeResolution, BakeKernel>();
  bakeKernels.set(64, kernel64);

  console.timeEnd("sdf-baked:create");

  return {
    mesh,
    material,
    bakeKernels,
    currentRes: 64,
    pendingRefine: null,
    opcodes: opcodes.slice(),
    aabb: expandedAabb,
  };
}

export function bakeVolume(
  handle: SdfBakedBodyHandle,
  renderer: THREE.WebGPURenderer,
  res: BakeResolution = 64,
): void {
  let kernel = handle.bakeKernels.get(res);
  if (!kernel) {
    kernel = createBakeKernel(res);
    handle.bakeKernels.set(res, kernel);
  }

  kernel.opcodeData.fill(0);
  kernel.opcodeData.set(
    handle.opcodes.subarray(0, Math.min(handle.opcodes.length, VM_MAX_OPCODES)),
  );
  kernel.aabbData.set(handle.aabb);

  console.time(`sdf-baked:bake@${res}`);
  renderer.compute(kernel.computeNode);
  console.timeEnd(`sdf-baked:bake@${res}`);

  handle.currentRes = res;
}

export function scheduleRefinement(
  handle: SdfBakedBodyHandle,
  renderer: THREE.WebGPURenderer,
  invalidate: () => void,
): void {
  if (handle.pendingRefine !== null) {
    clearTimeout(handle.pendingRefine);
  }

  if (handle.currentRes < 128) {
    handle.pendingRefine = setTimeout(() => {
      bakeVolume(handle, renderer, 128);
      invalidate();

      handle.pendingRefine = setTimeout(() => {
        bakeVolume(handle, renderer, 256);
        invalidate();
        handle.pendingRefine = null;
      }, 300);
    }, 100);
  } else if (handle.currentRes < 256) {
    handle.pendingRefine = setTimeout(() => {
      bakeVolume(handle, renderer, 256);
      invalidate();
      handle.pendingRefine = null;
    }, 300);
  }
}

export function updateSdfBakedBody(
  handle: SdfBakedBodyHandle,
  opcodes: Float32Array,
  aabb: Float32Array,
  renderer: THREE.WebGPURenderer,
  invalidate: () => void,
): SdfBakedBodyHandle {
  handle.opcodes = opcodes.slice();

  const margin = 2.0;
  handle.aabb[0] = aabb[0] - margin;
  handle.aabb[1] = aabb[1] - margin;
  handle.aabb[2] = aabb[2] - margin;
  handle.aabb[3] = aabb[3] + margin;
  handle.aabb[4] = aabb[4] + margin;
  handle.aabb[5] = aabb[5] + margin;

  const minX = handle.aabb[0], minY = handle.aabb[1], minZ = handle.aabb[2];
  const maxX = handle.aabb[3], maxY = handle.aabb[4], maxZ = handle.aabb[5];
  handle.mesh.position.set(
    (minX + maxX) * 0.5,
    (minY + maxY) * 0.5,
    (minZ + maxZ) * 0.5,
  );
  const sx = maxX - minX;
  const sy = maxY - minY;
  const sz = maxZ - minZ;
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

  bakeVolume(handle, renderer, 64);
  scheduleRefinement(handle, renderer, invalidate);

  return handle;
}

export function disposeSdfBakedBody(handle: SdfBakedBodyHandle): void {
  if (handle.pendingRefine !== null) {
    clearTimeout(handle.pendingRefine);
    handle.pendingRefine = null;
  }
  handle.mesh.geometry.dispose();
  handle.material.dispose();
  for (const kernel of handle.bakeKernels.values()) {
    kernel.volumeTex.dispose();
  }
  handle.bakeKernels.clear();
}
