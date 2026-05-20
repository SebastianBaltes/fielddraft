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
  Fn,
  If,
  Loop,
  abs,
  cameraPosition,
  clamp,
  cos,
  float,
  int,
  max,
  min,
  mix,
  modelWorldMatrix,
  normalize,
  positionGeometry,
  sin,
  sqrt,
  step,
  vec2,
  vec3,
  vec4,
  wgslFn,
  uniformArray,
} from "three/tsl";
import { MeshBasicNodeMaterial } from "three/webgpu";

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
  ALL_INCLUDES,
  MAX_STEPS, SURFACE_EPS, NORMAL_EPS, MAX_DIST,
} from "./sdfRenderer";

// ── VM constants ────────────────────────────────────────────────────

const VM_MAX_OPCODES = 2048;
const VM_VSTACK = 8;
const VM_PSTACK = 4;

// ── Bitcast helper (tag bytes are denormal f32 — must not flush) ────

const bitcastTagFn = wgslFn(`
  fn fd_bitcast_tag(v: f32) -> i32 {
    return i32(bitcast<u32>(v));
  }
`);

// ── VM interpreter (TSL expression tree) ────────────────────────────
//
// This builds a fixed expression tree that the WebGPU pipeline compiles
// once.  The opcode buffer lives in a uniformArray that can be updated
// per-frame without recompilation.
//
// Design constraints:
// - TSL has no `switch` — we use If/ElseIf chains
// - uniformArray.element(runtimeNode) supports runtime indexing
// - All rd() results use .toVar() to guarantee evaluation order
// - Point/value stacks are individual .toVar() variables (not arrays)

function buildVmEvalFn(opBuf: ReturnType<typeof uniformArray>) {
  return Fn(({ p: pIn }: { p: any }) => {

    // Program counter
    const pc = int(0).toVar();

    // Value stack (8 deep)
    const vs0 = float(0).toVar();
    const vs1 = float(0).toVar();
    const vs2 = float(0).toVar();
    const vs3 = float(0).toVar();
    const vs4 = float(0).toVar();
    const vs5 = float(0).toVar();
    const vs6 = float(0).toVar();
    const vs7 = float(0).toVar();
    const vsp = int(0).toVar();

    // Point stack (4 deep) — stored as individual xyz components
    const ps0x = float(0).toVar(), ps0y = float(0).toVar(), ps0z = float(0).toVar();
    const ps1x = float(0).toVar(), ps1y = float(0).toVar(), ps1z = float(0).toVar();
    const ps2x = float(0).toVar(), ps2y = float(0).toVar(), ps2z = float(0).toVar();
    const ps3x = float(0).toVar(), ps3y = float(0).toVar(), ps3z = float(0).toVar();
    const psp = int(0).toVar();

    // Current evaluation point
    const ptx = pIn.x.toVar();
    const pty = pIn.y.toVar();
    const ptz = pIn.z.toVar();

    // Helper: read one float from opcode buffer and advance pc
    const rd = () => {
      const v = opBuf.element(pc).toVar();
      pc.addAssign(1);
      return v;
    };

    // Helper: push to value stack
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

    // Helper: read from value stack at index
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

    // Helper: pop from value stack
    const popV = () => {
      vsp.subAssign(1);
      return readVs(vsp);
    };

    // Helper: push to point stack
    const pushP = (x: any, y: any, z: any) => {
      If(psp.equal(0), () => { ps0x.assign(x); ps0y.assign(y); ps0z.assign(z); })
        .ElseIf(psp.equal(1), () => { ps1x.assign(x); ps1y.assign(y); ps1z.assign(z); })
        .ElseIf(psp.equal(2), () => { ps2x.assign(x); ps2y.assign(y); ps2z.assign(z); })
        .ElseIf(psp.equal(3), () => { ps3x.assign(x); ps3y.assign(y); ps3z.assign(z); });
      psp.addAssign(1);
    };

    // Helper: pop from point stack → restore ptx/pty/ptz
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

    // Helper: read transform (8 floats) and compute local point
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

    // ── Main interpreter loop ─────────────────────────────────────

    // OP_END is always present in the opcode buffer from the kernel,
    // so the loop terminates via Break on OP_END rather than a length guard.
    Loop(int(VM_MAX_OPCODES), () => {
      const rawTag = opBuf.element(pc).toVar();
      pc.addAssign(1);
      const tag = bitcastTagFn({ v: rawTag });

      // ── 3D primitives (0x01–0x0B) ────────────────────────────

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

      // ── 2D primitives (0x10–0x13) ────────────────────────────

      }).ElseIf(tag.equal(int(OP_PRIM2D_CIRCLE)), () => {
        const cx = rd(), cy = rd(), r = rd();
        pushV(sdfCircle2dFn({ p: vec2(ptx, pty), center: vec2(cx, cy), r }));

      }).ElseIf(tag.equal(int(OP_PRIM2D_RECT)), () => {
        const cx = rd(), cy = rd(), hx = rd(), hy = rd();
        pushV(sdfRect2dFn({ p: vec2(ptx, pty), center: vec2(cx, cy), half: vec2(hx, hy) }));

      // OP_PRIM2D_POLY (0x12) skipped — variable-length operand not
      // feasible in fixed interpreter without polygon vertex limit.
      // Polygon extrudes fall back to transpiled mode.

      }).ElseIf(tag.equal(int(OP_PRIM2D_SECTOR)), () => {
        const cx = rd(), cy = rd(), r = rd();
        const start_angle = rd(), sweep_angle = rd();
        pushV(sdfSector2dFn({ p: vec2(ptx, pty), cx, cy, r, start_angle, sweep_angle }));

      // ── Binary operators (0x20–0x24) ─────────────────────────

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

      // ── Unary / compositor (0x29–0x2E) ───────────────────────

      }).ElseIf(tag.equal(int(OP_SHELL)), () => {
        const thickness = rd();
        const d = popV();
        pushV(abs(d).sub(thickness));

      }).ElseIf(tag.equal(int(OP_MORPH)), () => {
        const t = rd();
        const b = popV(), a = popV();
        pushV(mix(a, b, t));

      }).ElseIf(tag.equal(int(OP_DISPLACE_VAL)), () => {
        const dpth = rd(), freq = rd();
        const d = popV();
        const disp = sin(ptx.mul(freq))
          .mul(sin(pty.mul(freq).mul(1.3)))
          .mul(sin(ptz.mul(freq).mul(0.7)));
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

      // ── Domain warps (0x30–0x35) ─────────────────────────────

      }).ElseIf(tag.equal(int(OP_WARP_PUSH)), () => {
        pushP(ptx, pty, ptz);

      }).ElseIf(tag.equal(int(OP_WARP_POP)), () => {
        popP();

      }).ElseIf(tag.equal(int(OP_WARP_TWIST)), () => {
        const rate = rd();
        const angle = rate.mul(pty);
        const sn = sin(angle), cs = cos(angle);
        const oldX = ptx.toVar();
        ptx.assign(cs.mul(oldX).sub(sn.mul(ptz)));
        ptz.assign(sn.mul(oldX).add(cs.mul(ptz)));

      }).ElseIf(tag.equal(int(OP_WARP_BEND)), () => {
        const rate = rd();
        const angle = rate.mul(ptx);
        const sn = sin(angle), cs = cos(angle);
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
        const r = sqrt(ptx.mul(ptx).add(ptz.mul(ptz)));
        ptx.assign(r);
        ptz.assign(0);

      // ── Masked blend (0x40) ──────────────────────────────────

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
        const tMask = step(float(0), maskD);
        const uSmooth = blendMinFn({ a: da, b: db, r: blendR, mode });
        const uSharp = min(da, db);
        const dSmooth = blendMinFn({ a: da.negate(), b: db, r: blendR, mode }).negate();
        const dSharp = max(da, db.negate());
        const uResult = mix(uSmooth, uSharp, tMask);
        const dResult = mix(dSmooth, dSharp, tMask);
        pushV(mix(uResult, dResult, isDiff));

      // ── End / unknown ────────────────────────────────────────

      }).ElseIf(tag.equal(int(OP_END)), () => {
        Break();
      });

      // Unknown tags: skip (do nothing, pc already advanced past tag)
    });

    // Return top of value stack (or MAX if empty)
    const result = float(1e30).toVar();
    If(vsp.greaterThan(0), () => {
      result.assign(readVs(vsp.sub(1)));
    });
    return result;
  });
}

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

  const sceneSdf = buildVmEvalFn(opcodeBuf);

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
      t.addAssign(d);
      If(t.greaterThan(MAX_DIST), () => {
        Break();
      });
    });

    const finalColor = vec3(0).toVar();
    const finalAlpha = float(0).toVar();
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

  // Always just update the buffer — no topology check needed
  console.time("sdf-vm:bufferUpdate");
  handle.opcodeData.fill(0);
  handle.opcodeData.set(opcodes.subarray(0, newLen));
  handle.opcodeLen = newLen;
  console.timeEnd("sdf-vm:bufferUpdate");

  // Update AABB geometry if needed
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
