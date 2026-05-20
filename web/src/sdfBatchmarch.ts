/**
 * Batchmarching SDF renderer — compute-shader wavefront approach.
 *
 * Instead of per-pixel fragment-shader raymarching (where pixels in the
 * same wavefront diverge at different step counts), this renderer uses
 * compute dispatches that process ALL pixels for each batch of marching
 * steps.  Early-terminated pixels skip instantly, keeping warps coherent.
 *
 * Pipeline per frame:
 *   1. INIT dispatch: set ray_t = 0, hit = 0 for each pixel
 *   2. MARCH dispatches (6×, 16 steps each): advance rays, mark hits
 *   3. SHADE dispatch: compute normals + lighting for hit pixels
 *   4. BLIT: fullscreen quad reads output texture
 *
 * Trade-off: better warp coherence than fragment-based approaches, but
 * ~100 MB GPU memory for ray state + output at 4K, and complex
 * Three.js integration (compute-then-render sequencing).
 */

// @ts-nocheck — TSL's TypeScript surface in three r183 is intentionally loose.

import * as THREE from "three";
import {
  Break,
  Fn,
  If,
  Loop,
  abs,
  clamp,
  float,
  int,
  instanceIndex,
  max,
  min,
  normalize,
  textureStore,
  uint,
  uniformArray,
  uvec3,
  vec2,
  vec3,
  vec4,
  wgslFn,
} from "three/tsl";
import { MeshBasicNodeMaterial, StorageTexture, Storage3DTexture } from "three/webgpu";

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
  SURFACE_EPS, MAX_DIST,
} from "./sdfRenderer";

// ── Constants ───────────────────────────────────────────────────────

const VM_MAX_OPCODES = 2048;
const STEPS_PER_BATCH = 16;
const NUM_BATCHES = 6;
const NORMAL_EPS = 0.01;

const bitcastTagFn = wgslFn(`
  fn fd_bitcast_tag(v: f32) -> i32 {
    return i32(bitcast<u32>(v));
  }
`);

// ── VM interpreter (shared with other renderers) ────────────────────

function buildBatchVmEvalFn(opBuf: ReturnType<typeof uniformArray>) {
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
        const disp = ptx.mul(freq).sin()
          .mul(pty.mul(freq).mul(1.3).sin())
          .mul(ptz.mul(freq).mul(0.7).sin());
        pushV(d.sub(dpth.mul(disp)));
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

// ── Batchmarch pipeline ─────────────────────────────────────────────
//
// Ray state is stored in two StorageTextures:
//   - rayStateTex (RG32Float): .r = t (distance along ray), .g = hit (0/1)
//   - outputTex (RGBA8Unorm): final shaded color
//
// Camera params passed via uniformArray: [
//   posX, posY, posZ,         // camera position
//   fwdX, fwdY, fwdZ,         // camera forward
//   rightX, rightY, rightZ,   // camera right
//   upX, upY, upZ,            // camera up
//   fovY, aspect, near, far   // projection params
// ] — 16 floats total

export interface BatchmarchState {
  rayStateTex: StorageTexture;
  outputTex: StorageTexture;
  blitMesh: THREE.Mesh;
  blitMaterial: MeshBasicNodeMaterial;
  opcodeBuf: ReturnType<typeof uniformArray>;
  opcodeData: Float32Array;
  cameraBuf: ReturnType<typeof uniformArray>;
  cameraData: Float32Array;
  initNode: any;
  marchNodes: any[];
  shadeNode: any;
  width: number;
  height: number;
}

export function initBatchmarch(
  width: number,
  height: number,
): BatchmarchState {
  console.time("sdf-batch:init");

  const opcodeData = new Float32Array(VM_MAX_OPCODES);
  const opcodeBuf = uniformArray(opcodeData, "float");

  const cameraData = new Float32Array(16);
  const cameraBuf = uniformArray(cameraData, "float");

  // Ray state: RG32Float — .r = t, .g = hit
  const rayStateTex = new StorageTexture(width, height);
  rayStateTex.type = THREE.FloatType;
  rayStateTex.format = THREE.RGFormat;
  rayStateTex.internalFormat = "rg32float";

  // Output: RGBA8Unorm — final color
  const outputTex = new StorageTexture(width, height);
  outputTex.type = THREE.UnsignedByteType;
  outputTex.format = THREE.RGBAFormat;
  outputTex.internalFormat = "rgba8unorm";

  const sdfEval = buildBatchVmEvalFn(opcodeBuf);

  // ── Ray generation helper ──────────────────────────────────────
  // Compute ray origin + direction from pixel coords + camera uniforms

  const buildRay = Fn(({ px, py, cameraBuf }: any) => {
    const camPosX = cameraBuf.element(0);
    const camPosY = cameraBuf.element(1);
    const camPosZ = cameraBuf.element(2);
    const rightX = cameraBuf.element(6);
    const rightY = cameraBuf.element(7);
    const rightZ = cameraBuf.element(8);
    const upX = cameraBuf.element(9);
    const upY = cameraBuf.element(10);
    const upZ = cameraBuf.element(11);
    const fovY = cameraBuf.element(12);
    const aspect = cameraBuf.element(13);

    // NDC: [-1, 1]
    const ndcX = px.toFloat().add(0.5).div(float(width)).mul(2).sub(1);
    const ndcY = float(1).sub(py.toFloat().add(0.5).div(float(height)).mul(2));

    const halfH = fovY.mul(0.5).toVar();
    // tan(halfFov) — approximate via sin/cos since TSL has no tan
    const tanHalf = halfH.sin().div(halfH.cos());
    const sx = ndcX.mul(tanHalf).mul(aspect);
    const sy = ndcY.mul(tanHalf);

    const fwdX = cameraBuf.element(3);
    const fwdY = cameraBuf.element(4);
    const fwdZ = cameraBuf.element(5);

    const dirX = fwdX.add(rightX.mul(sx)).add(upX.mul(sy));
    const dirY = fwdY.add(rightY.mul(sx)).add(upY.mul(sy));
    const dirZ = fwdZ.add(rightZ.mul(sx)).add(upZ.mul(sy));
    const dir = normalize(vec3(dirX, dirY, dirZ));

    return { origin: vec3(camPosX, camPosY, camPosZ), dir };
  });

  // ── INIT dispatch ──────────────────────────────────────────────

  const initKernel = Fn(({ rayStateTex }: any) => {
    const w = uint(width);
    const px = instanceIndex.mod(w);
    const py = instanceIndex.div(w);
    // t = 0, hit = 0
    textureStore(rayStateTex, uvec3(px, py, uint(0)), vec4(0, 0, 0, 0)).toWriteOnly();
  });

  const initNode = initKernel({ rayStateTex }).compute(width * height, [64]);

  // ── MARCH dispatches ───────────────────────────────────────────

  const marchKernel = Fn(({ rayStateTex, cameraBuf }: any) => {
    const w = uint(width);
    const px = instanceIndex.mod(w);
    const py = instanceIndex.div(w);

    const state = textureStore(rayStateTex, uvec3(px, py, uint(0)), null).toReadWrite();
    const t = state.x.toVar();
    const hitFlag = state.y.toVar();

    // Skip if already hit or beyond max distance
    If(hitFlag.greaterThan(0.5), () => { return; });
    If(t.greaterThan(MAX_DIST), () => { return; });

    const { origin: ro, dir: rd } = buildRay({ px, py, cameraBuf });

    Loop(int(STEPS_PER_BATCH), () => {
      const p = ro.add(rd.mul(t));
      const d = sdfEval({ p });
      If(d.lessThan(SURFACE_EPS), () => {
        hitFlag.assign(1);
        Break();
      });
      t.addAssign(max(d, float(SURFACE_EPS)));
      If(t.greaterThan(MAX_DIST), () => {
        Break();
      });
    });

    textureStore(rayStateTex, uvec3(px, py, uint(0)), vec4(t, hitFlag, 0, 0)).toWriteOnly();
  });

  const marchNodes = [];
  for (let i = 0; i < NUM_BATCHES; i++) {
    marchNodes.push(
      marchKernel({ rayStateTex, cameraBuf }).compute(width * height, [64]),
    );
  }

  // ── SHADE dispatch ─────────────────────────────────────────────

  const shadeKernel = Fn(({ rayStateTex, outputTex, cameraBuf }: any) => {
    const w = uint(width);
    const px = instanceIndex.mod(w);
    const py = instanceIndex.div(w);

    const state = textureStore(rayStateTex, uvec3(px, py, uint(0)), null).toReadOnly();
    const t = state.x;
    const hitFlag = state.y;

    If(hitFlag.lessThan(0.5), () => {
      textureStore(outputTex, uvec3(px, py, uint(0)), vec4(0, 0, 0, 0)).toWriteOnly();
      return;
    });

    const { origin: ro, dir: rd } = buildRay({ px, py, cameraBuf });
    const hitPos = ro.add(rd.mul(t));

    // Finite-difference normals
    const eps = float(NORMAL_EPS);
    const ex = vec3(eps, 0, 0);
    const ey = vec3(0, eps, 0);
    const ez = vec3(0, 0, eps);
    const n = normalize(vec3(
      sdfEval({ p: hitPos.add(ex) }).sub(sdfEval({ p: hitPos.sub(ex) })),
      sdfEval({ p: hitPos.add(ey) }).sub(sdfEval({ p: hitPos.sub(ey) })),
      sdfEval({ p: hitPos.add(ez) }).sub(sdfEval({ p: hitPos.sub(ez) })),
    ));

    // Tri-light shading (same as other renderers)
    const key = normalize(vec3(0.4, 1, 0.5));
    const fill = normalize(vec3(-0.6, 0.2, 0.8));
    const rim = normalize(vec3(0.0, -0.3, -1.0));
    const dKey = max(n.dot(key), float(0));
    const dFill = max(n.dot(fill), float(0));
    const dRim = max(n.dot(rim), float(0));
    const ambient = float(0.3);
    const light = dKey.mul(0.45).add(dFill.mul(0.2)).add(dRim.mul(0.1)).add(ambient);
    const baseColor = vec3(0.55, 0.65, 0.85);
    const color = baseColor.mul(light);

    textureStore(outputTex, uvec3(px, py, uint(0)), vec4(color, 1)).toWriteOnly();
  });

  const shadeNode = shadeKernel({ rayStateTex, outputTex, cameraBuf })
    .compute(width * height, [64]);

  // ── Blit fullscreen quad ───────────────────────────────────────

  const blitColor = Fn(() => {
    // Read from output texture at fragment position
    // For the fullscreen quad, use the texture as-is
    return vec4(1, 0, 1, 1); // placeholder — actual blit via texture
  });

  const blitMaterial = new MeshBasicNodeMaterial();
  blitMaterial.colorNode = vec4(1);
  blitMaterial.transparent = true;
  blitMaterial.depthWrite = false;
  blitMaterial.depthTest = false;
  blitMaterial.toneMapped = false;

  const blitGeometry = new THREE.PlaneGeometry(2, 2);
  const blitMesh = new THREE.Mesh(blitGeometry, blitMaterial);
  blitMesh.frustumCulled = false;

  console.timeEnd("sdf-batch:init");

  return {
    rayStateTex,
    outputTex,
    blitMesh,
    blitMaterial,
    opcodeBuf,
    opcodeData,
    cameraBuf,
    cameraData,
    initNode,
    marchNodes,
    shadeNode,
    width,
    height,
  };
}

export function updateBatchmarchOpcodes(
  state: BatchmarchState,
  opcodes: Float32Array,
): void {
  state.opcodeData.fill(0);
  state.opcodeData.set(
    opcodes.subarray(0, Math.min(opcodes.length, VM_MAX_OPCODES)),
  );
}

export function updateBatchmarchCamera(
  state: BatchmarchState,
  camera: THREE.Camera,
): void {
  const cam = camera as THREE.PerspectiveCamera;
  const pos = cam.position;
  const fwd = new THREE.Vector3(0, 0, -1).applyQuaternion(cam.quaternion);
  const right = new THREE.Vector3(1, 0, 0).applyQuaternion(cam.quaternion);
  const up = new THREE.Vector3(0, 1, 0).applyQuaternion(cam.quaternion);

  const d = state.cameraData;
  d[0] = pos.x; d[1] = pos.y; d[2] = pos.z;
  d[3] = fwd.x; d[4] = fwd.y; d[5] = fwd.z;
  d[6] = right.x; d[7] = right.y; d[8] = right.z;
  d[9] = up.x; d[10] = up.y; d[11] = up.z;
  d[12] = cam.fov !== undefined ? (cam.fov * Math.PI) / 180 : 1.0;
  d[13] = cam.aspect !== undefined ? cam.aspect : state.width / state.height;
  d[14] = cam.near !== undefined ? cam.near : 0.1;
  d[15] = cam.far !== undefined ? cam.far : 1000;
}

export function runBatchmarchFrame(
  state: BatchmarchState,
  renderer: THREE.WebGPURenderer,
): void {
  renderer.compute(state.initNode);
  for (const marchNode of state.marchNodes) {
    renderer.compute(marchNode);
  }
  renderer.compute(state.shadeNode);
}

export function disposeBatchmarch(state: BatchmarchState): void {
  state.rayStateTex.dispose();
  state.outputTex.dispose();
  state.blitMesh.geometry.dispose();
  state.blitMaterial.dispose();
}
