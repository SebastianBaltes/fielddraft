/**
 * Per-body GPU raymarching via opcode-to-WGSL transpiler.
 *
 * Instead of interpreting the opcode buffer at runtime on the GPU (which
 * would require branches and a stack in the fragment shader), we
 * transpile the flat opcode stream into straight-line WGSL at material
 * creation time.  The GPU sees a branchless SDF function with all
 * constants embedded, which the driver can optimise aggressively.
 *
 * The outer raymarching loop (sphere tracing, normals, lighting) is
 * written in Three.js TSL so it composes cleanly with the WebGPU
 * renderer's camera / depth / model-matrix handling.
 */

// @ts-nocheck — TSL's TypeScript surface in three r183 is intentionally
// loose; we keep this file un-typed like raymarch.ts.

import * as THREE from "three";
import {
  Break,
  Fn,
  If,
  Loop,
  abs,
  cameraPosition,
  float,
  int,
  max,
  min,
  modelWorldMatrix,
  normalize,
  positionGeometry,
  vec3,
  vec4,
  wgslFn,
} from "three/tsl";
import { MeshBasicNodeMaterial } from "three/webgpu";

// ── Opcode tags (mirror crates/kernel/src/sdf_compiler.rs) ──────────

const OP_PRIM_BOX       = 0x01;
const OP_PRIM_SPHERE    = 0x02;
const OP_PRIM_CYLINDER  = 0x03;
const OP_PRIM_TORUS     = 0x04;
const OP_PRIM_CONE      = 0x05;
const OP_PRIM_CAPSULE   = 0x06;
const OP_PRIM_HEX_PRISM = 0x07;
const OP_PRIM_ELLIPSOID = 0x08;
const OP_PRIM_CONST     = 0x09;
const OP_PRIM_GYROID    = 0x0A;
const OP_PRIM_THREAD    = 0x0B;

const OP_PRIM2D_CIRCLE  = 0x10;
const OP_PRIM2D_RECT    = 0x11;
const OP_PRIM2D_POLY    = 0x12;
const OP_PRIM2D_SECTOR  = 0x13;

const OP_UNION          = 0x20;
const OP_DIFFERENCE     = 0x21;
const OP_INTERSECT      = 0x22;
const OP_SMOOTH_UNION   = 0x23;
const OP_SMOOTH_DIFF    = 0x24;

const OP_SHELL          = 0x29;
const OP_MORPH          = 0x2A;
const OP_DISPLACE_VAL   = 0x2B;
const OP_EXTRUDE        = 0x2C;
const OP_REVOLVE_SCALE  = 0x2D;
const OP_OFFSET_2D      = 0x2E;

const OP_WARP_PUSH      = 0x30;
const OP_WARP_POP       = 0x31;
const OP_WARP_TWIST     = 0x32;
const OP_WARP_BEND      = 0x33;
const OP_WARP_TRANSFORM = 0x34;
const OP_WARP_REVOLVE   = 0x35;

const OP_MASKED_BLEND   = 0x40;

const OP_END            = 0xFF;

// ── WGSL helper functions ───────────────────────────────────────────

const worldToLocalFn = wgslFn(`
  fn fd_world_to_local(p: vec3f, translate: vec3f, quat: vec4f, scale: f32) -> vec3f {
    let c = p - translate;
    let qx = -quat.x;
    let qy = -quat.y;
    let qz = -quat.z;
    let qw = quat.w;
    let tx = 2.0 * (qy * c.z - qz * c.y);
    let ty = 2.0 * (qz * c.x - qx * c.z);
    let tz = 2.0 * (qx * c.y - qy * c.x);
    let r = vec3f(
      c.x + qw * tx + qy * tz - qz * ty,
      c.y + qw * ty + qz * tx - qx * tz,
      c.z + qw * tz + qx * ty - qy * tx,
    );
    return r / scale;
  }
`);

const sdfBoxFn = wgslFn(`
  fn fd_sdf_box(p: vec3f, h: vec3f) -> f32 {
    let q = abs(p) - h;
    let outside = length(max(q, vec3f(0.0)));
    let inside = min(max(q.x, max(q.y, q.z)), 0.0);
    return outside + inside;
  }
`);

const sdfSphereFn = wgslFn(`
  fn fd_sdf_sphere(p: vec3f, r: f32) -> f32 {
    return length(p) - r;
  }
`);

const sdfCylinderFn = wgslFn(`
  fn fd_sdf_cylinder(p: vec3f, r: f32, hh: f32) -> f32 {
    let radial = length(vec2f(p.x, p.z)) - r;
    let axial = abs(p.y) - hh;
    let outside = length(vec2f(max(radial, 0.0), max(axial, 0.0)));
    let inside = min(max(radial, axial), 0.0);
    return outside + inside;
  }
`);

const sdfTorusFn = wgslFn(`
  fn fd_sdf_torus(p: vec3f, major: f32, minor: f32) -> f32 {
    let q0 = length(vec2f(p.x, p.z)) - major;
    return length(vec2f(q0, p.y)) - minor;
  }
`);

const sdfConeFn = wgslFn(`
  fn fd_sdf_cone(p: vec3f, rb: f32, rt: f32, hh: f32) -> f32 {
    let r = length(vec2f(p.x, p.z));
    let t = clamp((p.y + hh) / (2.0 * hh), 0.0, 1.0);
    let r_at_y = rb * (1.0 - t) + rt * t;
    let dr = r - r_at_y;
    let da = abs(p.y) - hh;
    if (dr > 0.0 && da > 0.0) {
      return sqrt(dr * dr + da * da);
    }
    return max(dr, da);
  }
`);

const sdfCapsuleFn = wgslFn(`
  fn fd_sdf_capsule(p: vec3f, r: f32, hh: f32) -> f32 {
    let rv = length(vec2f(p.x, p.z));
    let yc = clamp(p.y, -hh, hh);
    let dy = p.y - yc;
    return sqrt(rv * rv + dy * dy) - r;
  }
`);

const sdfHexPrismFn = wgslFn(`
  fn fd_sdf_hex_prism(p: vec3f, apothem: f32, hh: f32) -> f32 {
    let qx = abs(p.x);
    let qz = abs(p.z);
    let d_hex = max(qx * 0.8660254 + qz * 0.5, qz) - apothem;
    let d_axial = abs(p.y) - hh;
    if (d_hex > 0.0 && d_axial > 0.0) {
      return sqrt(d_hex * d_hex + d_axial * d_axial);
    }
    return max(d_hex, d_axial);
  }
`);

const sdfEllipsoidFn = wgslFn(`
  fn fd_sdf_ellipsoid(p: vec3f, radii: vec3f) -> f32 {
    let r_min = min(radii.x, min(radii.y, radii.z));
    let q = p / radii;
    return (length(q) - 1.0) * r_min;
  }
`);

const sdfGyroidFn = wgslFn(`
  fn fd_sdf_gyroid(p: vec3f, cell_size: f32, thickness: f32) -> f32 {
    let s = p / cell_size;
    let f = sin(s.x) * cos(s.y) + sin(s.y) * cos(s.z) + sin(s.z) * cos(s.x);
    return abs(f) - thickness;
  }
`);

const sdfThreadFn = wgslFn(`
  fn fd_sdf_thread(p: vec3f, nom_r: f32, pitch: f32, depth: f32, hh: f32) -> f32 {
    let r = length(vec2f(p.x, p.z));
    let theta = atan2(p.z, p.x);
    let y_unwrapped = p.y - pitch * theta / 6.2831853;
    let phase = (y_unwrapped / pitch) * 6.2831853;
    let r_profile = nom_r - depth * 0.5 * (1.0 - cos(phase));
    let d_radial = r - r_profile;
    let d_axial = abs(p.y) - hh;
    return max(d_radial, d_axial);
  }
`);

const blendMinFn = wgslFn(`
  fn fd_blend_min(a: f32, b: f32, r: f32, mode: f32) -> f32 {
    if (mode < 0.5) {
      let h = clamp(0.5 + 0.5 * (b - a) / r, 0.0, 1.0);
      return b * (1.0 - h) + a * h - r * h * (1.0 - h);
    } else {
      return min(min(a, b), (a + b - r) * 0.70710678);
    }
  }
`);

const sdfCapsule3dFn = wgslFn(`
  fn fd_sdf_capsule_3d(p: vec3f, a: vec3f, b: vec3f, r: f32) -> f32 {
    let ab = b - a;
    let ap = p - a;
    let dot_ab = dot(ab, ab);
    var t = 0.0;
    if (dot_ab > 0.0) {
      t = clamp(dot(ap, ab) / dot_ab, 0.0, 1.0);
    }
    let d = ap - ab * t;
    return length(d) - r;
  }
`);

const sdfExtrudeFn = wgslFn(`
  fn fd_sdf_extrude(d2d: f32, z: f32, half_h: f32) -> f32 {
    let wz = abs(z) - half_h;
    let outside = length(vec2f(max(d2d, 0.0), max(wz, 0.0)));
    let inside = min(max(d2d, wz), 0.0);
    return outside + inside;
  }
`);

const sdfRect2dFn = wgslFn(`
  fn fd_sdf_rect_2d(p: vec2f, center: vec2f, half: vec2f) -> f32 {
    let q = abs(p - center) - half;
    let outside = length(max(q, vec2f(0.0)));
    let inside = min(max(q.x, q.y), 0.0);
    return outside + inside;
  }
`);

const sdfCircle2dFn = wgslFn(`
  fn fd_sdf_circle_2d(p: vec2f, center: vec2f, r: f32) -> f32 {
    return length(p - center) - r;
  }
`);

const ALL_INCLUDES = [
  worldToLocalFn, sdfBoxFn, sdfSphereFn, sdfCylinderFn,
  sdfTorusFn, sdfConeFn, sdfCapsuleFn, sdfHexPrismFn,
  sdfEllipsoidFn, sdfGyroidFn, sdfThreadFn,
  blendMinFn, sdfCapsule3dFn, sdfExtrudeFn,
  sdfRect2dFn, sdfCircle2dFn,
];

// ── Raymarching constants ───────────────────────────────────────────

const MAX_STEPS = 96;
const SURFACE_EPS = 0.001;
const NORMAL_EPS = 0.01;
const MAX_DIST = 1200;

// ── Opcode → WGSL transpiler ────────────────────────────────────────

function f(v: number): string {
  const s = v.toString();
  return s.includes(".") || s.includes("e") || s.includes("E")
    ? s
    : s + ".0";
}

function readTag(opcodes: Float32Array, pc: number): number {
  const buf = new Uint32Array(opcodes.buffer, opcodes.byteOffset + pc * 4, 1);
  return buf[0];
}

export function transpileOpcodes(opcodes: Float32Array): string {
  const vstack: string[] = [];
  const pstack: string[] = [];
  let point = "p";
  let varN = 0;
  let pc = 0;
  const lines: string[] = [];

  function nextVar(): string {
    return `v_${varN++}`;
  }
  function nextPt(): string {
    return `pt_${varN++}`;
  }
  function rd(): number {
    return opcodes[pc++];
  }
  function rd3(): [number, number, number] {
    const a = opcodes[pc], b = opcodes[pc + 1], c = opcodes[pc + 2];
    pc += 3;
    return [a, b, c];
  }
  function pushV(name: string) {
    vstack.push(name);
  }
  function popV(): string {
    return vstack.pop()!;
  }
  function emitTransformAndLocal(): string {
    const [tx, ty, tz] = rd3();
    const [qx, qy, qz] = [rd(), rd(), rd()];
    const qw = rd();
    const scale = rd();
    const localVar = nextPt();
    lines.push(
      `  let ${localVar} = fd_world_to_local(${point}, ` +
      `vec3f(${f(tx)}, ${f(ty)}, ${f(tz)}), ` +
      `vec4f(${f(qx)}, ${f(qy)}, ${f(qz)}, ${f(qw)}), ${f(scale)});`
    );
    return localVar;
  }

  while (pc < opcodes.length) {
    const tag = readTag(opcodes, pc);
    pc++;

    switch (tag) {
      // ── 3D primitives ─────────────────────────────────────
      case OP_PRIM_BOX: {
        const local = emitTransformAndLocal();
        const scale = opcodes[pc - 1]; // last read by emitTransformAndLocal
        const [hx, hy, hz] = rd3();
        const v = nextVar();
        lines.push(
          `  let ${v} = fd_sdf_box(${local}, vec3f(${f(hx)}, ${f(hy)}, ${f(hz)})) * ${f(scale)};`
        );
        pushV(v);
        break;
      }
      case OP_PRIM_SPHERE: {
        const local = emitTransformAndLocal();
        const scale = opcodes[pc - 1];
        const r = rd();
        const v = nextVar();
        lines.push(`  let ${v} = fd_sdf_sphere(${local}, ${f(r)}) * ${f(scale)};`);
        pushV(v);
        break;
      }
      case OP_PRIM_CYLINDER: {
        const local = emitTransformAndLocal();
        const scale = opcodes[pc - 1];
        const r = rd();
        const hh = rd();
        const v = nextVar();
        lines.push(`  let ${v} = fd_sdf_cylinder(${local}, ${f(r)}, ${f(hh)}) * ${f(scale)};`);
        pushV(v);
        break;
      }
      case OP_PRIM_TORUS: {
        const local = emitTransformAndLocal();
        const scale = opcodes[pc - 1];
        const major = rd();
        const minor = rd();
        const v = nextVar();
        lines.push(`  let ${v} = fd_sdf_torus(${local}, ${f(major)}, ${f(minor)}) * ${f(scale)};`);
        pushV(v);
        break;
      }
      case OP_PRIM_CONE: {
        const local = emitTransformAndLocal();
        const scale = opcodes[pc - 1];
        const rb = rd(), rt = rd(), hh = rd();
        const v = nextVar();
        lines.push(`  let ${v} = fd_sdf_cone(${local}, ${f(rb)}, ${f(rt)}, ${f(hh)}) * ${f(scale)};`);
        pushV(v);
        break;
      }
      case OP_PRIM_CAPSULE: {
        const local = emitTransformAndLocal();
        const scale = opcodes[pc - 1];
        const r = rd(), hh = rd();
        const v = nextVar();
        lines.push(`  let ${v} = fd_sdf_capsule(${local}, ${f(r)}, ${f(hh)}) * ${f(scale)};`);
        pushV(v);
        break;
      }
      case OP_PRIM_HEX_PRISM: {
        const local = emitTransformAndLocal();
        const scale = opcodes[pc - 1];
        const ap = rd(), hh = rd();
        const v = nextVar();
        lines.push(`  let ${v} = fd_sdf_hex_prism(${local}, ${f(ap)}, ${f(hh)}) * ${f(scale)};`);
        pushV(v);
        break;
      }
      case OP_PRIM_ELLIPSOID: {
        const local = emitTransformAndLocal();
        const scale = opcodes[pc - 1];
        const [rx, ry, rz] = rd3();
        const v = nextVar();
        lines.push(`  let ${v} = fd_sdf_ellipsoid(${local}, vec3f(${f(rx)}, ${f(ry)}, ${f(rz)})) * ${f(scale)};`);
        pushV(v);
        break;
      }
      case OP_PRIM_CONST: {
        const val = rd();
        const v = nextVar();
        lines.push(`  let ${v} = ${f(val)};`);
        pushV(v);
        break;
      }
      case OP_PRIM_GYROID: {
        const local = emitTransformAndLocal();
        const scale = opcodes[pc - 1];
        const cs = rd(), th = rd();
        const v = nextVar();
        lines.push(`  let ${v} = fd_sdf_gyroid(${local}, ${f(cs)}, ${f(th)}) * ${f(scale)};`);
        pushV(v);
        break;
      }
      case OP_PRIM_THREAD: {
        const local = emitTransformAndLocal();
        const scale = opcodes[pc - 1];
        const nr = rd(), pitch = rd(), depth = rd(), hh = rd();
        const v = nextVar();
        lines.push(`  let ${v} = fd_sdf_thread(${local}, ${f(nr)}, ${f(pitch)}, ${f(depth)}, ${f(hh)}) * ${f(scale)};`);
        pushV(v);
        break;
      }

      // ── 2D primitives ─────────────────────────────────────
      case OP_PRIM2D_CIRCLE: {
        const cx = rd(), cy = rd(), r = rd();
        const v = nextVar();
        lines.push(`  let ${v} = fd_sdf_circle_2d(vec2f(${point}.x, ${point}.y), vec2f(${f(cx)}, ${f(cy)}), ${f(r)});`);
        pushV(v);
        break;
      }
      case OP_PRIM2D_RECT: {
        const cx = rd(), cy = rd(), hx = rd(), hy = rd();
        const v = nextVar();
        lines.push(`  let ${v} = fd_sdf_rect_2d(vec2f(${point}.x, ${point}.y), vec2f(${f(cx)}, ${f(cy)}), vec2f(${f(hx)}, ${f(hy)}));`);
        pushV(v);
        break;
      }
      case OP_PRIM2D_POLY: {
        const n = rd();
        const nInt = Math.round(n);
        const v = nextVar();
        // Inline polygon SDF: accumulate min edge distance + winding
        lines.push(`  var ${v}_d = 1e30;`);
        lines.push(`  var ${v}_s = 1.0;`);
        const verts: [number, number][] = [];
        for (let i = 0; i < nInt; i++) {
          verts.push([rd(), rd()]);
        }
        const px = `${point}.x`;
        const py = `${point}.y`;
        for (let i = 0; i < nInt; i++) {
          const j = (i + nInt - 1) % nInt;
          const vi = verts[i], vj = verts[j];
          const eid = `${v}_e${i}`;
          lines.push(`  {`);
          lines.push(`    let ei = vec2f(${f(vj[0])} - ${f(vi[0])}, ${f(vj[1])} - ${f(vi[1])});`);
          lines.push(`    let wi = vec2f(${px} - ${f(vi[0])}, ${py} - ${f(vi[1])});`);
          lines.push(`    let ti = clamp(dot(wi, ei) / dot(ei, ei), 0.0, 1.0);`);
          lines.push(`    let di = wi - ei * ti;`);
          lines.push(`    ${v}_d = min(${v}_d, dot(di, di));`);
          lines.push(`    let c1 = ${py} >= ${f(vi[1])};`);
          lines.push(`    let c2 = ${py} < ${f(vj[1])};`);
          lines.push(`    let c3 = ei.x * wi.y > ei.y * wi.x;`);
          lines.push(`    if ((c1 && c2 && c3) || (!c1 && !c2 && !c3)) { ${v}_s = -${v}_s; }`);
          lines.push(`  }`);
        }
        lines.push(`  let ${v} = ${v}_s * sqrt(${v}_d);`);
        pushV(v);
        break;
      }
      case OP_PRIM2D_SECTOR: {
        const cx = rd(), cy = rd(), r = rd(), start = rd(), sweep = rd();
        const v = nextVar();
        // Inline sector SDF
        const mid = start + sweep * 0.5;
        const half = Math.abs(sweep) * 0.5;
        lines.push(`  {`);
        lines.push(`    let ${v}_dx = ${point}.x - ${f(cx)};`);
        lines.push(`    let ${v}_dy = ${point}.y - ${f(cy)};`);
        lines.push(`    let ${v}_r = sqrt(${v}_dx * ${v}_dx + ${v}_dy * ${v}_dy);`);
        lines.push(`    let ${v}_angle = atan2(${v}_dy, ${v}_dx);`);
        lines.push(`    var ${v}_rel = ${v}_angle - ${f(mid)};`);
        lines.push(`    if (${v}_rel > 3.1415927) { ${v}_rel -= 6.2831853; }`);
        lines.push(`    if (${v}_rel < -3.1415927) { ${v}_rel += 6.2831853; }`);
        lines.push(`    let ${v}_d_circ = ${v}_r - ${f(r)};`);
        lines.push(`    var ${v}_result = ${v}_d_circ;`);
        lines.push(`    if (abs(${v}_rel) > ${f(half)}) {`);
        const edgeAnglePos = start + sweep;
        const edgeAngleNeg = start;
        lines.push(`      var ${v}_ea = ${f(edgeAngleNeg)};`);
        lines.push(`      if (${v}_rel > 0.0) { ${v}_ea = ${f(edgeAnglePos)}; }`);
        lines.push(`      let ${v}_ex = ${f(cx)} + ${f(r)} * cos(${v}_ea);`);
        lines.push(`      let ${v}_ey = ${f(cy)} + ${f(r)} * sin(${v}_ea);`);
        lines.push(`      let ${v}_to_edge = sqrt((${point}.x - ${v}_ex) * (${point}.x - ${v}_ex) + (${point}.y - ${v}_ey) * (${point}.y - ${v}_ey));`);
        lines.push(`      ${v}_result = min(max(${v}_d_circ, 0.0), min(${v}_to_edge, ${v}_r));`);
        lines.push(`    }`);
        lines.push(`  }`);
        // Need the result outside the block
        lines.pop(); // remove closing brace
        lines.push(`    var ${v}_out = ${v}_result;`);
        lines.push(`  }`);
        // Actually, WGSL scoping is an issue. Let me restructure.
        // Re-emit without block scope.
        // Clear and redo:
        const idx = lines.lastIndexOf(`  {`);
        lines.splice(idx);
        const vd = `${v}_dx`;
        lines.push(`  let ${vd} = ${point}.x - ${f(cx)};`);
        lines.push(`  let ${v}_dy = ${point}.y - ${f(cy)};`);
        lines.push(`  let ${v}_r = sqrt(${vd} * ${vd} + ${v}_dy * ${v}_dy);`);
        lines.push(`  let ${v}_angle = atan2(${v}_dy, ${vd});`);
        lines.push(`  var ${v}_rel = ${v}_angle - ${f(mid)};`);
        lines.push(`  if (${v}_rel > 3.1415927) { ${v}_rel -= 6.2831853; }`);
        lines.push(`  if (${v}_rel < -3.1415927) { ${v}_rel += 6.2831853; }`);
        lines.push(`  let ${v}_d_circ = ${v}_r - ${f(r)};`);
        lines.push(`  var ${v} = ${v}_d_circ;`);
        lines.push(`  if (abs(${v}_rel) > ${f(half)}) {`);
        lines.push(`    var ${v}_ea = ${f(edgeAngleNeg)};`);
        lines.push(`    if (${v}_rel > 0.0) { ${v}_ea = ${f(edgeAnglePos)}; }`);
        lines.push(`    let ${v}_ex = ${f(cx)} + ${f(r)} * cos(${v}_ea);`);
        lines.push(`    let ${v}_ey = ${f(cy)} + ${f(r)} * sin(${v}_ea);`);
        lines.push(`    let ${v}_to_edge = sqrt((${point}.x - ${v}_ex) * (${point}.x - ${v}_ex) + (${point}.y - ${v}_ey) * (${point}.y - ${v}_ey));`);
        lines.push(`    ${v} = min(max(${v}_d_circ, 0.0), min(${v}_to_edge, ${v}_r));`);
        lines.push(`  }`);
        pushV(v);
        break;
      }

      // ── Binary operators ──────────────────────────────────
      case OP_UNION: {
        const b = popV(), a = popV();
        const v = nextVar();
        lines.push(`  let ${v} = min(${a}, ${b});`);
        pushV(v);
        break;
      }
      case OP_DIFFERENCE: {
        const b = popV(), a = popV();
        const v = nextVar();
        lines.push(`  let ${v} = max(${a}, -${b});`);
        pushV(v);
        break;
      }
      case OP_INTERSECT: {
        const b = popV(), a = popV();
        const v = nextVar();
        lines.push(`  let ${v} = max(${a}, ${b});`);
        pushV(v);
        break;
      }
      case OP_SMOOTH_UNION: {
        const r = rd(), mode = rd();
        const b = popV(), a = popV();
        const v = nextVar();
        lines.push(`  let ${v} = fd_blend_min(${a}, ${b}, ${f(r)}, ${f(mode)});`);
        pushV(v);
        break;
      }
      case OP_SMOOTH_DIFF: {
        const r = rd(), mode = rd();
        const b = popV(), a = popV();
        const v = nextVar();
        lines.push(`  let ${v} = -fd_blend_min(-${a}, ${b}, ${f(r)}, ${f(mode)});`);
        pushV(v);
        break;
      }

      // ── Unary / compositor ────────────────────────────────
      case OP_SHELL: {
        const thickness = rd();
        const d = popV();
        const v = nextVar();
        lines.push(`  let ${v} = abs(${d}) - ${f(thickness)};`);
        pushV(v);
        break;
      }
      case OP_MORPH: {
        const t = rd();
        const b = popV(), a = popV();
        const v = nextVar();
        lines.push(`  let ${v} = (1.0 - ${f(t)}) * ${a} + ${f(t)} * ${b};`);
        pushV(v);
        break;
      }
      case OP_DISPLACE_VAL: {
        const depth = rd(), freq = rd();
        const d = popV();
        const v = nextVar();
        lines.push(`  let ${v} = ${d} - ${f(depth)} * sin(${point}.x * ${f(freq)}) * sin(${point}.y * ${f(freq)} * 1.3) * sin(${point}.z * ${f(freq)} * 0.7);`);
        pushV(v);
        break;
      }
      case OP_EXTRUDE: {
        const half_h = rd(), scale = rd();
        const d2d = popV();
        const v = nextVar();
        lines.push(`  let ${v} = fd_sdf_extrude(${d2d}, ${point}.z, ${f(half_h)}) * ${f(scale)};`);
        pushV(v);
        break;
      }
      case OP_REVOLVE_SCALE: {
        const scale = rd();
        const d = popV();
        const v = nextVar();
        lines.push(`  let ${v} = ${d} * ${f(scale)};`);
        pushV(v);
        break;
      }
      case OP_OFFSET_2D: {
        const dist = rd();
        const d = popV();
        const v = nextVar();
        lines.push(`  let ${v} = ${d} - ${f(dist)};`);
        pushV(v);
        break;
      }

      // ── Domain warps ──────────────────────────────────────
      case OP_WARP_PUSH: {
        pstack.push(point);
        break;
      }
      case OP_WARP_POP: {
        point = pstack.pop()!;
        break;
      }
      case OP_WARP_TWIST: {
        const rate = rd();
        const pt = nextPt();
        lines.push(`  let ${pt}_angle = ${f(rate)} * ${point}.y;`);
        lines.push(`  let ${pt}_s = sin(${pt}_angle);`);
        lines.push(`  let ${pt}_c = cos(${pt}_angle);`);
        lines.push(`  let ${pt} = vec3f(${pt}_c * ${point}.x - ${pt}_s * ${point}.z, ${point}.y, ${pt}_s * ${point}.x + ${pt}_c * ${point}.z);`);
        point = pt;
        break;
      }
      case OP_WARP_BEND: {
        const rate = rd();
        const pt = nextPt();
        lines.push(`  let ${pt}_angle = ${f(rate)} * ${point}.x;`);
        lines.push(`  let ${pt}_s = sin(${pt}_angle);`);
        lines.push(`  let ${pt}_c = cos(${pt}_angle);`);
        lines.push(`  let ${pt} = vec3f(${pt}_c * ${point}.x - ${pt}_s * ${point}.y, ${pt}_s * ${point}.x + ${pt}_c * ${point}.y, ${point}.z);`);
        point = pt;
        break;
      }
      case OP_WARP_TRANSFORM: {
        const [tx, ty, tz] = rd3();
        const [qx, qy, qz] = [rd(), rd(), rd()];
        const qw = rd();
        const scale = rd();
        const pt = nextPt();
        lines.push(
          `  let ${pt} = fd_world_to_local(${point}, ` +
          `vec3f(${f(tx)}, ${f(ty)}, ${f(tz)}), ` +
          `vec4f(${f(qx)}, ${f(qy)}, ${f(qz)}, ${f(qw)}), ${f(scale)});`
        );
        point = pt;
        break;
      }
      case OP_WARP_REVOLVE: {
        const pt = nextPt();
        lines.push(`  let ${pt} = vec3f(sqrt(${point}.x * ${point}.x + ${point}.z * ${point}.z), ${point}.y, 0.0);`);
        point = pt;
        break;
      }

      // ── Masked blend ──────────────────────────────────────
      case OP_MASKED_BLEND: {
        const [p0x, p0y, p0z] = rd3();
        const [p1x, p1y, p1z] = rd3();
        const mask_r = rd(), blend_r = rd(), mode = rd();
        const is_diff = rd() > 0.5;
        const db = popV(), da = popV();
        const v = nextVar();
        lines.push(`  let ${v}_mask = fd_sdf_capsule_3d(${point}, vec3f(${f(p0x)}, ${f(p0y)}, ${f(p0z)}), vec3f(${f(p1x)}, ${f(p1y)}, ${f(p1z)}), ${f(mask_r)});`);
        if (is_diff) {
          lines.push(`  var ${v} = max(${da}, -${db});`);
          lines.push(`  if (${v}_mask < 0.0) { ${v} = -fd_blend_min(-${da}, ${db}, ${f(blend_r)}, ${f(mode)}); }`);
        } else {
          lines.push(`  var ${v} = min(${da}, ${db});`);
          lines.push(`  if (${v}_mask < 0.0) { ${v} = fd_blend_min(${da}, ${db}, ${f(blend_r)}, ${f(mode)}); }`);
        }
        pushV(v);
        break;
      }

      case OP_END:
        break;

      default:
        break;
    }

    if (tag === OP_END) break;
  }

  const result = vstack.length > 0 ? vstack[vstack.length - 1] : "1e30";
  let body = lines.join("\n") + "\n";
  body += `  return ${result};\n`;
  return `fn fd_eval_sdf(p: vec3f) -> f32 {\n${body}}\n`;
}

// ── Material creation ───────────────────────────────────────────────

export interface SdfBodyHandle {
  mesh: THREE.Mesh;
  material: MeshBasicNodeMaterial;
}

export function createSdfBodyMaterial(
  opcodes: Float32Array,
  aabb: Float32Array,
): SdfBodyHandle {
  const wgsl = transpileOpcodes(opcodes);
  const evalSdf = wgslFn(wgsl, ALL_INCLUDES);

  const sceneSdf = Fn(({ p }) => {
    return evalSdf({ p });
  });

  const sceneNormal = Fn(({ p }) => {
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
      const light = dKey.mul(0.45).add(dFill.mul(0.2)).add(dRim.mul(0.1)).add(ambient);
      const baseColor = vec3(0.55, 0.65, 0.85);
      finalColor.assign(baseColor.mul(light));
      finalAlpha.assign(1);
    });

    return vec4(finalColor, finalAlpha);
  });

  const material = new MeshBasicNodeMaterial();
  material.colorNode = colorNode();
  material.side = THREE.BackSide;
  material.transparent = true;
  material.depthWrite = false;
  material.depthTest = true;
  material.toneMapped = false;

  // AABB-sized box geometry
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

  return { mesh, material };
}

export function disposeSdfBody(handle: SdfBodyHandle): void {
  handle.mesh.geometry.dispose();
  handle.material.dispose();
}
