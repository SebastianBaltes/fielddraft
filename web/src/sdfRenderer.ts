/**
 * SDF opcode constants and WGSL primitive functions.
 *
 * Shared by the VM interpreter (sdfVmInterpreter.ts) and both renderers
 * (sdfVmRenderer.ts, sdfDeferredRenderer.ts).
 */

// @ts-nocheck — TSL's TypeScript surface in three r183 is intentionally
// loose; we keep this file un-typed.

import {
  wgslFn,
} from "three/tsl";

// ── Opcode tags (mirror crates/kernel/src/sdf_compiler.rs) ──────────

export const OP_PRIM_BOX       = 0x01;
export const OP_PRIM_SPHERE    = 0x02;
export const OP_PRIM_CYLINDER  = 0x03;
export const OP_PRIM_TORUS     = 0x04;
export const OP_PRIM_CONE      = 0x05;
export const OP_PRIM_CAPSULE   = 0x06;
export const OP_PRIM_HEX_PRISM = 0x07;
export const OP_PRIM_ELLIPSOID = 0x08;
export const OP_PRIM_CONST     = 0x09;
export const OP_PRIM_GYROID    = 0x0A;
export const OP_PRIM_THREAD    = 0x0B;
export const OP_PRIM_TEX3D     = 0x0C;

export const OP_PRIM2D_CIRCLE  = 0x10;
export const OP_PRIM2D_RECT    = 0x11;
export const OP_PRIM2D_POLY    = 0x12;
export const OP_PRIM2D_SECTOR  = 0x13;

export const OP_UNION          = 0x20;
export const OP_DIFFERENCE     = 0x21;
export const OP_INTERSECT      = 0x22;
export const OP_SMOOTH_UNION   = 0x23;
export const OP_SMOOTH_DIFF    = 0x24;

export const OP_SHELL          = 0x29;
export const OP_MORPH          = 0x2A;
export const OP_DISPLACE_VAL   = 0x2B;
export const OP_EXTRUDE        = 0x2C;
export const OP_REVOLVE_SCALE  = 0x2D;
export const OP_OFFSET_2D      = 0x2E;

export const OP_WARP_PUSH      = 0x30;
export const OP_WARP_POP       = 0x31;
export const OP_WARP_TWIST     = 0x32;
export const OP_WARP_BEND      = 0x33;
export const OP_WARP_TRANSFORM = 0x34;
export const OP_WARP_REVOLVE   = 0x35;

export const OP_MASKED_BLEND   = 0x40;

export const OP_END            = 0xFF;

// ── WGSL helper functions ───────────────────────────────────────────

export const worldToLocalFn = wgslFn(`
  fn fd_world_to_local(p: vec3f, translate: vec3f, quat: vec4f, scale: vec3f) -> vec3f {
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

export const sdfBoxFn = wgslFn(`
  fn fd_sdf_box(p: vec3f, h: vec3f) -> f32 {
    let q = abs(p) - h;
    let outside = length(max(q, vec3f(0.0)));
    let inside = min(max(q.x, max(q.y, q.z)), 0.0);
    return outside + inside;
  }
`);

export const sdfSphereFn = wgslFn(`
  fn fd_sdf_sphere(p: vec3f, r: f32) -> f32 {
    return length(p) - r;
  }
`);

export const sdfCylinderFn = wgslFn(`
  fn fd_sdf_cylinder(p: vec3f, r: f32, hh: f32) -> f32 {
    let radial = length(vec2f(p.x, p.z)) - r;
    let axial = abs(p.y) - hh;
    let outside = length(vec2f(max(radial, 0.0), max(axial, 0.0)));
    let inside = min(max(radial, axial), 0.0);
    return outside + inside;
  }
`);

export const sdfTorusFn = wgslFn(`
  fn fd_sdf_torus(p: vec3f, major: f32, minor: f32) -> f32 {
    let q0 = length(vec2f(p.x, p.z)) - major;
    return length(vec2f(q0, p.y)) - minor;
  }
`);

export const sdfConeFn = wgslFn(`
  fn fd_sdf_cone(p: vec3f, rb: f32, rt: f32, hh: f32) -> f32 {
    let r = length(vec2f(p.x, p.z));
    let t = clamp((p.y + hh) / (2.0 * hh), 0.0, 1.0);
    let r_at_y = rb * (1.0 - t) + rt * t;
    // Project the radial gap onto the inclined wall normal so the field is the
    // true perpendicular distance (Lipschitz-1). Cylinder (rb == rt) -> 1.
    let drr = rb - rt;
    let slant_len = sqrt(drr * drr + (2.0 * hh) * (2.0 * hh));
    let wall_scale = select(1.0, (2.0 * hh) / slant_len, slant_len > 1e-9);
    let dr = (r - r_at_y) * wall_scale;
    let da = abs(p.y) - hh;
    if (dr > 0.0 && da > 0.0) {
      return sqrt(dr * dr + da * da);
    }
    return max(dr, da);
  }
`);

export const sdfCapsuleFn = wgslFn(`
  fn fd_sdf_capsule(p: vec3f, r: f32, hh: f32) -> f32 {
    let rv = length(vec2f(p.x, p.z));
    let yc = clamp(p.y, -hh, hh);
    let dy = p.y - yc;
    return sqrt(rv * rv + dy * dy) - r;
  }
`);

export const sdfHexPrismFn = wgslFn(`
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

export const sdfEllipsoidFn = wgslFn(`
  fn fd_sdf_ellipsoid(p: vec3f, radii: vec3f) -> f32 {
    let r_min = min(radii.x, min(radii.y, radii.z));
    let q = p / radii;
    return (length(q) - 1.0) * r_min;
  }
`);

export const sdfGyroidFn = wgslFn(`
  fn fd_sdf_gyroid(p: vec3f, cell_size: f32, thickness: f32) -> f32 {
    let s = p / cell_size;
    let f = sin(s.x) * cos(s.y) + sin(s.y) * cos(s.z) + sin(s.z) * cos(s.x);
    // Normalize by Lipschitz bound (2*sqrt(3)/cell_size) to get a proper distance field.
    return (abs(f) - thickness) * cell_size * 0.2887;
  }
`);

export const sdfThreadFn = wgslFn(`
  fn fd_sdf_thread(p: vec3f, nom_r: f32, pitch: f32, depth: f32, hh: f32) -> f32 {
    let r = length(vec2f(p.x, p.z));
    let theta = atan2(p.z, p.x);
    let y_unwrapped = p.y - pitch * theta / 6.2831853;
    let phase = (y_unwrapped / pitch) * 6.2831853;
    let r_profile = nom_r - depth * 0.5 * (1.0 - cos(phase));
    let d_radial = r - r_profile;
    let d_axial = abs(p.y) - hh;
    // Lipschitz normalization. The helix unwraps y into r_profile's phase, so
    // ∂r_profile/∂y = π·depth/pitch and |∇f| peaks at sqrt(1 + (π·depth/pitch)²)
    // — the raw Chebyshev field OVERESTIMATES distance and the sphere-march
    // overshoots the groove walls (the banding seen on Thread-subtractions,
    // worst looking down the axis). Dividing by that exact bound yields a
    // conservative ≤1 distance, same idea as the Gyroid 0.2887 / Cone wall_scale
    // normalizations. eps_pitch guards a degenerate pitch→0 thread.
    let lip = sqrt(1.0 + pow(3.14159265 * depth / max(pitch, 1e-6), 2.0));
    return max(d_radial, d_axial) / lip;
  }
`);

export const blendMinFn = wgslFn(`
  fn fd_blend_min(a: f32, b: f32, r: f32, mode: f32) -> f32 {
    if (mode < 0.5) {
      let h = clamp(0.5 + 0.5 * (b - a) / r, 0.0, 1.0);
      return b * (1.0 - h) + a * h - r * h * (1.0 - h);
    } else {
      return min(min(a, b), (a + b - r) * 0.70710678);
    }
  }
`);

export const sdfCapsule3dFn = wgslFn(`
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

export const sdfExtrudeFn = wgslFn(`
  fn fd_sdf_extrude(d2d: f32, z: f32, half_h: f32) -> f32 {
    let wz = abs(z) - half_h;
    let outside = length(vec2f(max(d2d, 0.0), max(wz, 0.0)));
    let inside = min(max(d2d, wz), 0.0);
    return outside + inside;
  }
`);

export const sdfRect2dFn = wgslFn(`
  fn fd_sdf_rect_2d(p: vec2f, center: vec2f, half: vec2f) -> f32 {
    let q = abs(p - center) - half;
    let outside = length(max(q, vec2f(0.0)));
    let inside = min(max(q.x, q.y), 0.0);
    return outside + inside;
  }
`);

export const sdfCircle2dFn = wgslFn(`
  fn fd_sdf_circle_2d(p: vec2f, center: vec2f, r: f32) -> f32 {
    return length(p - center) - r;
  }
`);

export const sdfSector2dFn = wgslFn(`
  fn fd_sdf_sector_2d(p: vec2f, cx: f32, cy: f32, r: f32, start_angle: f32, sweep_angle: f32) -> f32 {
    let dx = p.x - cx;
    let dy = p.y - cy;
    let dist = sqrt(dx * dx + dy * dy);
    let angle = atan2(dy, dx);
    let mid = start_angle + sweep_angle * 0.5;
    let half_a = abs(sweep_angle) * 0.5;
    var rel = angle - mid;
    if (rel > 3.1415927) { rel -= 6.2831853; }
    if (rel < -3.1415927) { rel += 6.2831853; }
    let d_circ = dist - r;
    if (abs(rel) <= half_a) {
      return d_circ;
    }
    var ea = start_angle;
    if (rel > 0.0) { ea = start_angle + sweep_angle; }
    let ex = cx + r * cos(ea);
    let ey = cy + r * sin(ea);
    let to_edge = sqrt((p.x - ex) * (p.x - ex) + (p.y - ey) * (p.y - ey));
    return min(max(d_circ, 0.0), min(to_edge, dist));
  }
`);

// ── Shared constant ─────────────────────────────────────────────────

export const NORMAL_EPS = 0.01;
