/**
 * Shared GPU bytecode VM interpreter for SDF evaluation.
 *
 * Builds a fixed TSL expression tree that interprets the opcode buffer
 * at runtime.  Used by sdfVmRenderer and sdfDeferredRenderer.
 */

// @ts-nocheck — TSL's TypeScript surface in three r183 is intentionally loose.

import {
  Break,
  Fn,
  If,
  Loop,
  abs,
  clamp,
  float,
  int,
  max,
  min,
  sqrt,
  step,
  texture,
  vec2,
  vec3,
  vec4,
  wgslFn,
  uniformArray,
} from "three/tsl";

import * as THREE from "three";

import {
  OP_PRIM_BOX, OP_PRIM_SPHERE, OP_PRIM_CYLINDER, OP_PRIM_TORUS,
  OP_PRIM_CONE, OP_PRIM_CAPSULE, OP_PRIM_HEX_PRISM, OP_PRIM_ELLIPSOID,
  OP_PRIM_CONST, OP_PRIM_GYROID, OP_PRIM_THREAD, OP_PRIM_TEX3D,
  OP_PRIM2D_CIRCLE, OP_PRIM2D_RECT, OP_PRIM2D_POLY, OP_PRIM2D_SECTOR,
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
} from "./sdfRenderer";

export const MAX_VOLUME_TEXTURES = 4;

export const VM_MAX_OPCODES = 2048;

export const bitcastTagFn = wgslFn(`
  fn fd_bitcast_tag(v: f32) -> i32 {
    return i32(bitcast<u32>(v));
  }
`);

export const floatToIntFn = wgslFn(`
  fn fd_f2i(v: f32) -> i32 {
    return i32(v + 0.5);
  }
`);

export function buildVmSdfEvalFn(
  opBuf: ReturnType<typeof uniformArray>,
  options?: {
    withBaseOffset?: boolean;
    volumeTextures?: THREE.Data3DTexture[];
    withVolumeBaseIdx?: boolean;
  },
) {
  const useBaseOffset = options?.withBaseOffset ?? false;
  const volTextures = options?.volumeTextures ?? [];
  const useVolumeBaseIdx = options?.withVolumeBaseIdx ?? false;
  return Fn(({ p: pIn, baseOffset }: any) => {
    const pc = useBaseOffset ? int(baseOffset).toVar() : int(0).toVar();

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

      }).ElseIf(tag.equal(int(OP_PRIM_TEX3D)), () => {
        const texIdxRaw = rd();
        const { local, scale: s } = emitTransformAndLocal();
        const aminx = rd(), aminy = rd(), aminz = rd();
        const amaxx = rd(), amaxy = rd(), amaxz = rd();
        const uvw = local.sub(vec3(aminx, aminy, aminz))
          .div(vec3(amaxx, amaxy, amaxz).sub(vec3(aminx, aminy, aminz)));
        const dist = float(1e10).toVar();
        if (volTextures.length > 0) {
          const texIdx = floatToIntFn({ v: texIdxRaw });
          const lookupIdx = useVolumeBaseIdx
            ? texIdx.add(int(volumeBaseIdx))
            : texIdx;
          for (let i = 0; i < volTextures.length; i++) {
            If(lookupIdx.equal(int(i)), () => {
              dist.assign(texture(volTextures[i], uvw).x);
            });
          }
        }
        pushV(dist.mul(s));

      }).ElseIf(tag.equal(int(OP_PRIM2D_CIRCLE)), () => {
        const cx = rd(), cy = rd(), r = rd();
        pushV(sdfCircle2dFn({ p: vec2(ptx, pty), center: vec2(cx, cy), r }));
      }).ElseIf(tag.equal(int(OP_PRIM2D_RECT)), () => {
        const cx = rd(), cy = rd(), hx = rd(), hy = rd();
        pushV(sdfRect2dFn({ p: vec2(ptx, pty), center: vec2(cx, cy), half: vec2(hx, hy) }));
      }).ElseIf(tag.equal(int(OP_PRIM2D_POLY)), () => {
        const nRaw = rd();
        const nInt = floatToIntFn({ v: nRaw });
        const vertStart = pc.toVar();
        pc.addAssign(nInt.mul(2));

        const dSq = float(1e30).toVar();
        const wSign = float(1).toVar();
        const ii = int(0).toVar();
        Loop(int(32), () => {
          If(ii.greaterThanEqual(nInt), () => { Break(); });
          const jj = ii.add(nInt).sub(1).mod(nInt);
          const vix = opBuf.element(vertStart.add(ii.mul(2)));
          const viy = opBuf.element(vertStart.add(ii.mul(2)).add(1));
          const vjx = opBuf.element(vertStart.add(jj.mul(2)));
          const vjy = opBuf.element(vertStart.add(jj.mul(2)).add(1));
          const eix = vjx.sub(vix);
          const eiy = vjy.sub(viy);
          const wix = ptx.sub(vix);
          const wiy = pty.sub(viy);
          const dotWE = wix.mul(eix).add(wiy.mul(eiy));
          const dotEE = eix.mul(eix).add(eiy.mul(eiy));
          const ti = clamp(dotWE.div(dotEE), 0, 1);
          const dix = wix.sub(eix.mul(ti));
          const diy = wiy.sub(eiy.mul(ti));
          dSq.assign(min(dSq, dix.mul(dix).add(diy.mul(diy))));
          const crossVal = eix.mul(wiy).sub(eiy.mul(wix));
          const c1 = step(viy, pty);
          const c2 = float(1).sub(step(vjy, pty));
          const c3 = step(float(0), crossVal);
          const flipAmt = c1.mul(c2).mul(c3).add(
            float(1).sub(c1).mul(float(1).sub(c2)).mul(float(1).sub(c3)),
          );
          wSign.assign(wSign.mul(float(1).sub(flipAmt.mul(2))));
          ii.addAssign(1);
        });
        pushV(wSign.mul(sqrt(dSq)));
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
