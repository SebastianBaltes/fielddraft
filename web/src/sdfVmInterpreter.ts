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
  acos,
  asin,
  atan,
  clamp,
  cos,
  exp,
  float,
  floor,
  int,
  log,
  max,
  min,
  round,
  sign,
  sin,
  sqrt,
  step,
  texture3D,
  vec2,
  vec3,
  vec4,
  wgslFn,
} from "three/tsl";

import * as THREE from "three";

import {
  OP_PRIM_BOX, OP_PRIM_SPHERE, OP_PRIM_CYLINDER, OP_PRIM_TORUS,
  OP_PRIM_CONE, OP_PRIM_CAPSULE, OP_PRIM_HEX_PRISM, OP_PRIM_ELLIPSOID,
  OP_PRIM_CONST, OP_PRIM_GYROID, OP_PRIM_THREAD, OP_PRIM_TEX3D,
  OP_PRIM2D_CIRCLE, OP_PRIM2D_RECT, OP_PRIM2D_POLY, OP_PRIM2D_SECTOR,
  OP_PRIM2D_PATH,
  OP_UNION, OP_DIFFERENCE, OP_INTERSECT, OP_SMOOTH_UNION, OP_SMOOTH_DIFF,
  OP_SHELL, OP_MORPH, OP_DISPLACE_VAL, OP_EXTRUDE, OP_REVOLVE_SCALE,
  OP_OFFSET_2D, OP_NEG, OP_PRIM_HALFSPACE, OP_PRIM_CUSTOM,
  OP_WARP_PUSH, OP_WARP_POP, OP_WARP_TWIST, OP_WARP_BEND,
  OP_WARP_TRANSFORM, OP_WARP_REVOLVE,
  OP_WARP_HELIX, OP_HELIX_SCALE, OP_WARP_SWEEP,
  OP_MASKED_BLEND, OP_END,
  worldToLocalFn, halfspaceLocalFn, sdfBoxFn, sdfSphereFn, sdfCylinderFn,
  sdfTorusFn, sdfConeFn, sdfCapsuleFn, sdfHexPrismFn,
  sdfEllipsoidFn, sdfGyroidFn, sdfThreadFn,
  blendMinFn, sdfCapsule3dFn, sdfExtrudeFn,
  sdfRect2dFn, sdfCircle2dFn, sdfSector2dFn,
  pathSegDistSqFn, pathSegCrossingFn, pathQuadDistSqFn, pathQuadCrossingFn,
  pathArcDistSqFn, pathArcCrossingFn,
} from "./sdfRenderer";

export const VM_MAX_OPCODES = 512;

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

const atan2Fn = wgslFn(`
  fn fd_atan2(y: f32, x: f32) -> f32 {
    return atan2(y, x);
  }
`);

export function buildVmSdfEvalFn(
  opBuf: { element: (index: any) => any },
  options?: {
    withBaseOffset?: boolean;
    volumeTextures?: THREE.Data3DTexture[];
    withVolumeBaseIdx?: boolean;
  },
) {
  const useBaseOffset = options?.withBaseOffset ?? false;
  const volTextures = options?.volumeTextures ?? [];
  const useVolumeBaseIdx = options?.withVolumeBaseIdx ?? false;
  const evalFn = Fn(({ p: pIn, baseOffset, volumeBaseIdx }: any) => {
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
      const sx = rd(), sy = rd(), sz = rd();
      const local = worldToLocalFn({
        p: vec3(ptx, pty, ptz),
        translate: vec3(tx, ty, tz),
        quat: vec4(qx, qy, qz, qw),
        scale: vec3(sx, sy, sz),
      }).toVar();
      return { local, scale: min(sx, min(sy, sz)) };
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

      }).ElseIf(tag.equal(int(OP_PRIM_CUSTOM)), () => {
        // User expression tape (mirror of `sdf_compiler::eval_expr_records`):
        // header = transform(10) + inv_lipschitz + half_extents(3) +
        // op_count, then op_count × (code, payload) fixed-width records.
        // The inner interpreter runs an own 8-deep operand stack
        // (es0..es7 + esp — same If-chain register trick as vs*/vsp;
        // TSL has no local arrays). Field = max(expr·invL, box).
        const { local, scale: s } = emitTransformAndLocal();
        const invL = rd();
        const hx = rd(), hy = rd(), hz = rd();
        const nRaw = rd();
        const nInt = floatToIntFn({ v: nRaw });
        const recStart = pc.toVar();
        pc.addAssign(nInt.mul(2));

        const es0 = float(0).toVar(), es1 = float(0).toVar();
        const es2 = float(0).toVar(), es3 = float(0).toVar();
        const es4 = float(0).toVar(), es5 = float(0).toVar();
        const es6 = float(0).toVar(), es7 = float(0).toVar();
        const esp = int(0).toVar();
        const pushE = (val: any) => {
          If(esp.equal(0), () => { es0.assign(val); })
            .ElseIf(esp.equal(1), () => { es1.assign(val); })
            .ElseIf(esp.equal(2), () => { es2.assign(val); })
            .ElseIf(esp.equal(3), () => { es3.assign(val); })
            .ElseIf(esp.equal(4), () => { es4.assign(val); })
            .ElseIf(esp.equal(5), () => { es5.assign(val); })
            .ElseIf(esp.equal(6), () => { es6.assign(val); })
            .ElseIf(esp.equal(7), () => { es7.assign(val); });
          esp.addAssign(1);
        };
        const popE = () => {
          esp.subAssign(1);
          const out = float(0).toVar();
          If(esp.equal(0), () => { out.assign(es0); })
            .ElseIf(esp.equal(1), () => { out.assign(es1); })
            .ElseIf(esp.equal(2), () => { out.assign(es2); })
            .ElseIf(esp.equal(3), () => { out.assign(es3); })
            .ElseIf(esp.equal(4), () => { out.assign(es4); })
            .ElseIf(esp.equal(5), () => { out.assign(es5); })
            .ElseIf(esp.equal(6), () => { out.assign(es6); })
            .ElseIf(esp.equal(7), () => { out.assign(es7); });
          return out;
        };

        const ii = int(0).toVar();
        Loop(nInt, () => {
          const base = recStart.add(ii.mul(2));
          const code = floatToIntFn({ v: opBuf.element(base) });
          const payload = opBuf.element(base.add(1)).toVar();
          // Leaf pushes: 0=X 1=Y 2=Z 3=Const.
          If(code.lessThan(int(4)), () => {
            const v = float(0).toVar();
            If(code.equal(int(0)), () => { v.assign(local.x); })
              .ElseIf(code.equal(int(1)), () => { v.assign(local.y); })
              .ElseIf(code.equal(int(2)), () => { v.assign(local.z); })
              .Else(() => { v.assign(payload); });
            pushE(v);
          }).ElseIf(
            // Unary: 8=Neg 11=Abs 12=Sqrt 13=Sin 14=Cos 15=Floor
            // 16=Exp 17=Ln 20=Asin 21=Acos.
            code.equal(int(8))
              .or(code.greaterThanEqual(int(11)).and(code.lessThanEqual(int(17))))
              .or(code.greaterThanEqual(int(20))),
            () => {
              const a = popE();
              const r = float(0).toVar();
              If(code.equal(int(8)), () => { r.assign(a.negate()); })
                .ElseIf(code.equal(int(11)), () => { r.assign(abs(a)); })
                .ElseIf(code.equal(int(12)), () => { r.assign(sqrt(max(a, 0.0))); })
                .ElseIf(code.equal(int(13)), () => { r.assign(sin(a)); })
                .ElseIf(code.equal(int(14)), () => { r.assign(cos(a)); })
                .ElseIf(code.equal(int(15)), () => { r.assign(floor(a)); })
                .ElseIf(code.equal(int(16)), () => { r.assign(exp(a)); })
                .ElseIf(code.equal(int(17)), () => {
                  If(a.greaterThan(0.0), () => { r.assign(log(a)); })
                    .Else(() => { r.assign(float(-1.0e6)); });
                })
                .ElseIf(code.equal(int(20)), () => { r.assign(asin(clamp(a, -1.0, 1.0))); })
                .Else(() => { r.assign(acos(clamp(a, -1.0, 1.0))); });
              pushE(r);
            },
          ).Else(() => {
            // Binary: 4=Add 5=Sub 6=Mul 7=Div 9=Min 10=Max 18=Pow 19=Atan2.
            const b = popE();
            const a = popE();
            const r = float(0).toVar();
            If(code.equal(int(4)), () => { r.assign(a.add(b)); })
              .ElseIf(code.equal(int(5)), () => { r.assign(a.sub(b)); })
              .ElseIf(code.equal(int(6)), () => { r.assign(a.mul(b)); })
              .ElseIf(code.equal(int(7)), () => { r.assign(a.div(b)); })
              .ElseIf(code.equal(int(9)), () => { r.assign(min(a, b)); })
              .ElseIf(code.equal(int(10)), () => { r.assign(max(a, b)); })
              .ElseIf(code.equal(int(18)), () => {
                If(a.greaterThan(0.0), () => { r.assign(exp(log(a).mul(b))); })
                  .Else(() => { r.assign(float(0)); });
              })
              .Else(() => { r.assign(atan2Fn({ y: a, x: b })); });
            pushE(r);
          });
          ii.addAssign(1);
        });
        const expr = popE();
        const boxD = sdfBoxFn({ p: local, h: vec3(hx, hy, hz) });
        pushV(max(expr.mul(invL), boxD).mul(s));

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
              dist.assign(texture3D(volTextures[i], uvw).x);
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
        Loop(nInt, () => {
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
      }).ElseIf(tag.equal(int(OP_PRIM2D_PATH)), () => {
        // Analytic path records — exact distance + exact even-odd parity,
        // mirroring `fielddraft_protocol::sketch2d::{sdf_path,
        // path_is_inside}`. Each record is a fixed 7-float, y-MONOTONE
        // piece: kind(1) + payload(6), kind 0 = line(start, end), 1 = quad
        // Bézier(p0, ctrl, p2), 2 = arc(center, r, start, sweep). An OPEN
        // chain (closed == 0) stays unsigned, like the CPU — crucially the
        // contour is NOT closed with a synthetic edge: an axis-stripped
        // revolve profile must not grow a membrane on the axis.
        const REC = 7;
        const nRaw = rd();
        const closedFlag = rd();
        const nInt = floatToIntFn({ v: nRaw });
        const recStart = pc.toVar();
        pc.addAssign(nInt.mul(REC));

        const p2d = vec2(ptx, pty).toVar();
        const dSq = float(1e30).toVar();
        const crossings = int(0).toVar();
        const ii = int(0).toVar();
        Loop(nInt, () => {
          const base = recStart.add(ii.mul(REC));
          const kind = floatToIntFn({ v: opBuf.element(base) });
          const f0 = opBuf.element(base.add(1)).toVar();
          const f1 = opBuf.element(base.add(2)).toVar();
          const f2 = opBuf.element(base.add(3)).toVar();
          const f3 = opBuf.element(base.add(4)).toVar();
          const f4 = opBuf.element(base.add(5)).toVar();
          If(kind.equal(int(0)), () => {
            const a = vec2(f0, f1), b = vec2(f2, f3);
            dSq.assign(min(dSq, pathSegDistSqFn({ p: p2d, a, b })));
            crossings.addAssign(pathSegCrossingFn({ p: p2d, a, b }));
          }).ElseIf(kind.equal(int(1)), () => {
            const f5 = opBuf.element(base.add(6));
            const a = vec2(f0, f1), b = vec2(f2, f3), c = vec2(f4, f5);
            dSq.assign(pathQuadDistSqFn({ p: p2d, a, b, c, best_in: dSq }));
            crossings.addAssign(pathQuadCrossingFn({ p: p2d, a, b, c }));
          }).Else(() => {
            const c = vec2(f0, f1);
            dSq.assign(min(dSq, pathArcDistSqFn({ p: p2d, c, r: f2, a0: f3, sweep: f4 })));
            crossings.addAssign(pathArcCrossingFn({ p: p2d, c, r: f2, a0: f3, sweep: f4 }));
          });
          ii.addAssign(1);
        });
        const dist = sqrt(dSq).toVar();
        If(closedFlag.greaterThan(float(0.5)).and(crossings.bitAnd(int(1)).equal(int(1))), () => {
          dist.assign(dist.negate());
        });
        pushV(dist);

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
        // CLOSED inward shell: wall = max(d, −(d + t)). Opened shells compile
        // to two subtrees + OFFSET(−t) + DIFFERENCE instead (retained-boundary
        // erosion, mirrors `csg::shell_wall_sdf`), so this op carries only the
        // thickness.
        const thickness = rd();
        const d = popV();
        pushV(max(d, d.add(thickness).negate()));
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
        const capMask = floatToIntFn({ v: rd() });
        const d2d = popV();
        // cap_mask 3 = both caps retained → exact-corner composition (IQ);
        // an opened cap drops its term: plain max over the retained ones
        // (mirrors `csg::extrude_retained_combine`).
        const res = float(0).toVar();
        If(capMask.equal(int(3)), () => {
          res.assign(sdfExtrudeFn({ d2d, z: ptz, half_h }));
        }).ElseIf(capMask.equal(int(1)), () => {
          res.assign(max(d2d, ptz.sub(half_h)));
        }).ElseIf(capMask.equal(int(2)), () => {
          res.assign(max(d2d, ptz.negate().sub(half_h)));
        }).Else(() => {
          res.assign(d2d);
        });
        pushV(res.mul(s));
      }).ElseIf(tag.equal(int(OP_REVOLVE_SCALE)), () => {
        const s = rd();
        pushV(popV().mul(s));
      }).ElseIf(tag.equal(int(OP_HELIX_SCALE)), () => {
        // pt.z carries the signed end-overshoot chord stashed by
        // OP_WARP_HELIX; compose like the kernel's `helix_compose`
        // (IQ extrude box), then apply the baked world·Lipschitz scale.
        const s = rd().toVar();
        const d2 = popV().toVar();
        const ell = ptz.toVar();
        const dPos = max(d2, 0).toVar();
        const ellPos = max(ell, 0).toVar();
        const outside = dPos.mul(dPos).add(ellPos.mul(ellPos)).sqrt();
        const inside = min(max(d2, ell), 0);
        pushV(outside.add(inside).mul(s));
      }).ElseIf(tag.equal(int(OP_OFFSET_2D)), () => {
        const dist = rd();
        pushV(popV().sub(dist));
      }).ElseIf(tag.equal(int(OP_NEG)), () => {
        pushV(popV().negate());
      }).ElseIf(tag.equal(int(OP_PRIM_HALFSPACE)), () => {
        // One analytic half-space face (retained-boundary shell): kind +
        // transform(10) + params(4); raw local SDF × min-scale, like every
        // other primitive opcode.
        const kind = rd();
        const { local, scale: s } = emitTransformAndLocal();
        const p0 = rd(), p1 = rd(), p2 = rd(), p3 = rd();
        pushV(halfspaceLocalFn({ local, kind, p: vec4(p0, p1, p2, p3) }).mul(s));

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
        const sx = rd(), sy = rd(), sz = rd();
        const local = worldToLocalFn({
          p: vec3(ptx, pty, ptz),
          translate: vec3(tx, ty, tz),
          quat: vec4(qx, qy, qz, qw),
          scale: vec3(sx, sy, sz),
        }).toVar();
        ptx.assign(local.x);
        pty.assign(local.y);
        ptz.assign(local.z);
      }).ElseIf(tag.equal(int(OP_WARP_REVOLVE)), () => {
        const r = ptx.mul(ptx).add(ptz.mul(ptz)).sqrt();
        ptx.assign(r);
        ptz.assign(0);

      }).ElseIf(tag.equal(int(OP_WARP_HELIX)), () => {
        // Helical unwrap into the profile's (r', w) frame — the GPU
        // mirror of the kernel's `helix_profile_point`. The nearest
        // pass is found by rounding, then its CONTINUOUS sweep
        // parameter is clamped to [0, 2π·turns] (ticket vtyh: clamping
        // a per-winding integer index instead jumped `w` across the
        // atan2 seam at the ends). The signed end-overshoot chord
        // rides along in pt.z (2D sketch ops ignore z) until
        // OP_HELIX_SCALE composes it in.
        const pitch = rd().toVar();
        const hand = rd().toVar();
        const turns = rd().toVar();
        const yAnchor = rd().toVar();
        const taper = rd().toVar();
        const rInner = rd().toVar();
        const startAngle = rd().toVar();
        const tau = float(6.2831853);
        const pi = float(3.14159265);
        const r = ptx.mul(ptx).add(ptz.mul(ptz)).sqrt().toVar();
        const theta = atan(ptz, ptx).toVar();
        const th = hand.mul(theta).sub(startAngle).toVar();
        const n = round(pty.sub(yAnchor).div(pitch).sub(th.div(tau))).toVar();
        const tN = th.add(tau.mul(n)).toVar();
        const tEnd = tau.mul(turns).toVar();
        const tC = clamp(tN, float(0), tEnd).toVar();
        const w = pty.sub(pitch.mul(tC).div(tau)).toVar();
        const rp = r.sub(taper.mul(pty.sub(yAnchor))).toVar();
        const rr = max(rInner, max(pitch.div(tau).mul(0.25), float(1e-3))).toVar();
        const lT = tN.sub(tEnd.mul(0.5)).abs().sub(tEnd.mul(0.5)).toVar();
        const ell = sign(lT)
          .mul(min(r, rr))
          .mul(min(lT.abs(), pi).mul(0.5).sin())
          .toVar();
        ptx.assign(rp);
        pty.assign(w);
        ptz.assign(ell);

      }).ElseIf(tag.equal(int(OP_WARP_SWEEP)), () => {
        // Nearest spine point → project into the interpolated (n, b) frame =
        // (u, v); the cap coordinate w rides in pt.z until OP_HELIX_SCALE.
        // GPU mirror of `SweepPath::cross_section` (verified in Rust against
        // the tree eval via `sweep_opcode_stream_matches_tree_eval`).
        const nInt = floatToIntFn({ v: rd() }).toVar();
        const closed = rd().toVar();
        const totalLen = rd().toVar();
        const twist = rd().toVar();
        const dataStart = pc.toVar();
        pc.addAssign(nInt.mul(10));

        const p = vec3(ptx, pty, ptz).toVar();
        const segCount = nInt.sub(1).toVar();
        If(closed.greaterThan(0.5), () => { segCount.assign(nInt); });

        const sample = (idx: ReturnType<typeof int>, off: number) =>
          vec3(
            opBuf.element(dataStart.add(idx.mul(10)).add(off)),
            opBuf.element(dataStart.add(idx.mul(10)).add(off + 1)),
            opBuf.element(dataStart.add(idx.mul(10)).add(off + 2)),
          );

        const bestD2 = float(1e30).toVar();
        const bestSeg = int(0).toVar();
        const bestT = float(0).toVar();
        const si = int(0).toVar();
        Loop(segCount, () => {
          const i1 = si.add(1).mod(nInt);
          const a = sample(si, 0);
          const b = sample(i1, 0);
          const ab = b.sub(a);
          const denom = max(ab.dot(ab), float(1e-12));
          const t = clamp(p.sub(a).dot(ab).div(denom), 0, 1).toVar();
          const dd = p.sub(a.add(ab.mul(t)));
          const d2 = dd.dot(dd);
          If(d2.lessThan(bestD2), () => {
            bestD2.assign(d2);
            bestSeg.assign(si);
            bestT.assign(t);
          });
          si.addAssign(1);
        });

        const i0 = bestSeg.toVar();
        const i1 = bestSeg.add(1).mod(nInt).toVar();
        const pa = sample(i0, 0);
        const pb = sample(i1, 0);
        const foot = pa.add(pb.sub(pa).mul(bestT));
        const nf = sample(i0, 3).add(sample(i1, 3).sub(sample(i0, 3)).mul(bestT)).normalize();
        const bf = sample(i0, 6).add(sample(i1, 6).sub(sample(i0, 6)).mul(bestT)).normalize();
        const d = p.sub(foot);
        const u = d.dot(nf).toVar();
        const v = d.dot(bf).toVar();
        const arc0 = opBuf.element(dataStart.add(i0.mul(10)).add(9));
        const arc1 = opBuf.element(dataStart.add(i1.mul(10)).add(9));
        const arcFoot = arc0.add(arc1.sub(arc0).mul(bestT));

        // Twist: un-rotate (u, v) by θ = twist·s/L (mirror of cross_section).
        const theta = twist.mul(arcFoot).div(max(totalLen, float(1e-6))).negate();
        const cs = theta.cos();
        const sn = theta.sin();
        const ur = cs.mul(u).sub(sn.mul(v));
        const vr = sn.mul(u).add(cs.mul(v));
        u.assign(ur);
        v.assign(vr);

        const w = float(-1e30).toVar();
        If(closed.lessThan(0.5), () => {
          const atStart = bestSeg.equal(int(0)).and(bestT.lessThanEqual(float(1e-6)));
          const atEnd = bestSeg.equal(segCount.sub(1))
            .and(bestT.greaterThanEqual(float(1).sub(float(1e-6))));
          If(atStart.or(atEnd), () => {
            w.assign(d.dot(nf.cross(bf).normalize()).abs());
          }).Else(() => {
            w.assign(min(arcFoot, totalLen.sub(arcFoot)).negate());
          });
        });
        ptx.assign(u);
        pty.assign(v);
        ptz.assign(w);

      }).ElseIf(tag.equal(int(OP_MASKED_BLEND)), () => {
        // DL-61: count-prefixed polyline tube. Read `n` points, then the
        // mask SDF is the min over capsules along consecutive segments
        // (a 2-point polyline reduces to the single-capsule case; a
        // 1-point polyline is a sphere).
        const n = rd().toVar();
        const firstX = rd().toVar(), firstY = rd().toVar(), firstZ = rd().toVar();
        const prevX = float(firstX).toVar();
        const prevY = float(firstY).toVar();
        const prevZ = float(firstZ).toVar();
        const maskD = float(1e30).toVar();
        // Single-point polyline → sphere at the first point (radius added
        // below once maskR is read). Handle via a zero-length capsule.
        If(n.lessThanEqual(int(1)), () => {
          maskD.assign(
            sqrt(
              ptx.sub(firstX).mul(ptx.sub(firstX))
                .add(pty.sub(firstY).mul(pty.sub(firstY)))
                .add(ptz.sub(firstZ).mul(ptz.sub(firstZ))),
            ),
          );
        });
        Loop(n.sub(1), () => {
          const cx = rd().toVar(), cy = rd().toVar(), cz = rd().toVar();
          const segD = sdfCapsule3dFn({
            p: vec3(ptx, pty, ptz),
            a: vec3(prevX, prevY, prevZ),
            b: vec3(cx, cy, cz),
            r: 0.0,
          });
          maskD.assign(min(maskD, segD));
          prevX.assign(cx);
          prevY.assign(cy);
          prevZ.assign(cz);
        });
        const maskR = rd(), blendR = rd(), mode = rd();
        const isDiff = rd();
        const db = popV(), da = popV();
        // #382: CONTINUOUS mask cross-fade (mirror of Rust
        // `masked_blend_render_fade`). `maskD` is the unsigned distance to the
        // polyline; the signed tube distance is `maskD - maskR` (< 0 inside).
        // A HARD `step` switch made the render field discontinuous (jump up to
        // `blendR`) → the sdf-vm sphere-trace tunnelled through the surface and
        // the model greyed out, while the mesh (sign-only DC) was fine. We
        // cross-fade `hard → blended` with a C1 smoothstep over a band of one
        // `blendR`, sitting in the tube's outer shell where blended≈hard, so the
        // fillet surface is preserved. `march_relax` damps the residual slope.
        // hardW: 1 outside the tube (→ sharp/hard), 0 deep inside (→ blended).
        const band = max(blendR, float(1e-3));
        const xw = clamp(maskR.sub(maskD).div(band), 0.0, 1.0).toVar();
        const blendW = xw.mul(xw).mul(float(3).sub(xw.mul(2)));
        const hardW = float(1).sub(blendW);
        const uSmooth = blendMinFn({ a: da, b: db, r: blendR, mode });
        const uSharp = min(da, db);
        const dSmooth = blendMinFn({ a: da.negate(), b: db, r: blendR, mode }).negate();
        const dSharp = max(da, db.negate());
        const uResult = uSmooth.mix(uSharp, hardW);
        const dResult = dSmooth.mix(dSharp, hardW);
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

  // NOTE (regression o6ia, reverts ticket 21ws's setLayout from 93bb212):
  // We deliberately do NOT call evalFn.setLayout(...) here. 93bb212 added a
  // layout so TSL would emit the eval as ONE WGSL `fn` (call sites = `call`)
  // instead of inlining the whole ~512-step opcode VM at every call site,
  // aiming to cut the >1min compile. On real hardware that did NOT pay off:
  //   - The driver cost is dominated by the body itself (VM_MAX_OPCODES ×
  //     MARCH_STEPS × ~40-branch switch), not by text multiplicity. Measured:
  //     with the layout the FULL eval never finished compiling even after
  //     200s, while a trivial-stub eval (same layout) compiled in ~45s — so
  //     the body, not the emit form, is the choke. The layout is neither the
  //     cause nor the cure of the slow compile.
  //   - Worse, the single-`fn` form — a function that does dynamic storage
  //     indexing inside its own 512-iteration loop AND is then called from
  //     inside the fragment's 512-step march loop (nested non-uniform loops)
  //     — is the shape ANGLE/GLES (the user's WebGPU "compatibility mode")
  //     mis-translates, yielding a body that compiles but renders black:
  //     ONLY the B-Rep edges remained, no shaded SDF surfaces (ticket o6ia).
  //     7ce9e19 (inlined form) was verified rendering; 93bb212 regressed it.
  // The inlined form is what every GPU path (incl. ANGLE) already accepted, so
  // we restore it. The legitimate compile-speed goal of 21ws must be pursued
  // by SHRINKING the shader (lower MARCH_STEPS/VM_MAX_OPCODES, fewer shade-pass
  // eval calls), not by toggling the emit form — tracked separately.
  return evalFn;
}
