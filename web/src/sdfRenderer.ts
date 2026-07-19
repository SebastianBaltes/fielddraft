/**
 * SDF opcode constants and WGSL primitive functions.
 *
 * Shared by the VM interpreter (sdfVmInterpreter.ts) and both renderers
 * (sdfVmRenderer.ts, sdfDeferredRenderer.ts).
 */

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
export const OP_PRIM_HALFSPACE = 0x0D;
export const OP_PRIM_CUSTOM    = 0x0E;

export const OP_PRIM2D_CIRCLE  = 0x10;
export const OP_PRIM2D_RECT    = 0x11;
export const OP_PRIM2D_POLY    = 0x12;
export const OP_PRIM2D_SECTOR  = 0x13;
export const OP_PRIM2D_PATH    = 0x14;

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
export const OP_NEG            = 0x2F;

export const OP_WARP_PUSH      = 0x30;
export const OP_WARP_POP       = 0x31;
export const OP_WARP_TWIST     = 0x32;
export const OP_WARP_BEND      = 0x33;
export const OP_WARP_TRANSFORM = 0x34;
export const OP_WARP_REVOLVE   = 0x35;
export const OP_WARP_HELIX     = 0x36;
export const OP_HELIX_SCALE    = 0x37;
export const OP_WARP_SWEEP     = 0x38;

export const OP_MASKED_BLEND   = 0x40;
// offset_tree Phase B (#384): Kanal-Korrekturkörper an einem dualen
// Offset-Stopp — pops den naiven Stopp-Wert, pusht die korrigierte
// Distanz. Header 9 Floats + 27 je Bogen (Spiegel von
// `kernel/src/offset_channel.rs::channel_apply`).
export const OP_ARC_CHANNEL    = 0x41;
// offset_tree Phase C4 (#384): Vertex-Kugel an einer Triple-Junction.
// Payload (16): concave, delta, trust, gate, v(3), g1(3), g2(3), g3(3).
export const OP_VERTEX_SPHERE  = 0x42;

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

// Raw local SDF of one analytic half-space face for OP_PRIM_HALFSPACE,
// mirroring the kernel's `csg::sdf_halfspace` / `sdf_compiler::halfspace_local`
// (retained-boundary shell faces). `kind` selects the surface; `p` holds up to
// four params (layout per kind, zero-padded):
//   0 plane: normal=(p.x,p.y,p.z), offset=p.w
//   1 infinite cylinder (axis Y): radius=p.x
//   2 sphere: radius=p.x
//   3 cone (apex origin, axis Y): half_angle=p.x
//   4 torus: major=p.x, minor=p.y
//   5 non-analytic (never emitted as a retained face)
// The caller scales by the placement's min-scale, like every other primitive.
export const halfspaceLocalFn = wgslFn(`
  fn fd_halfspace_local(local: vec3f, kind: f32, p: vec4f) -> f32 {
    if (kind < 0.5) {
      return dot(local, vec3f(p.x, p.y, p.z)) - p.w;
    } else if (kind < 1.5) {
      return length(vec2f(local.x, local.z)) - p.x;
    } else if (kind < 2.5) {
      return length(local) - p.x;
    } else if (kind < 3.5) {
      let rho = length(vec2f(local.x, local.z));
      return rho * cos(p.x) - abs(local.y) * sin(p.x);
    } else if (kind < 4.5) {
      let rho = length(vec2f(local.x, local.z));
      let q0 = rho - p.x;
      return length(vec2f(q0, local.y)) - p.y;
    }
    return 1e30;
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
      // Fillet — Circular-CD smin (#384), matching the Rust kernel's
      // csg::smin_poly: min(a,b) - (r/2)*(1 + h - sqrt(1 - h*(h-2))) with
      // h = max(r - |a-b|, 0)/r. Same zero set as the old circular-
      // geometrical form, but 1-Lipschitz everywhere (never overestimates).
      if (r <= 0.0) { return min(a, b); }
      let h = max(r - abs(a - b), 0.0) / r;
      return min(a, b) - r * 0.5 * (1.0 + h - sqrt(1.0 - h * (h - 2.0)));
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

// ── OP_PRIM2D_PATH segment helpers ──────────────────────────────────
//
// GPU ports of the analytic path machinery in
// `fielddraft_protocol::sketch2d` (`sdf_path` / `path_is_inside`). Each
// record in the opcode stream is a y-MONOTONE piece (the kernel pre-splits
// quads at their y-apex and arcs at the cardinal angles), so every crossing
// helper is the plain polygon half-open endpoint rule plus one closed-form
// root — no subdivision loops in the shader.

export const pathSegDistSqFn = wgslFn(`
  fn fd_path_seg_dist_sq(p: vec2f, a: vec2f, b: vec2f) -> f32 {
    let e = b - a;
    let w = p - a;
    let t = clamp(dot(w, e) / max(dot(e, e), 1e-12), 0.0, 1.0);
    let d = w - e * t;
    return dot(d, d);
  }
`);

export const pathSegCrossingFn = wgslFn(`
  fn fd_path_seg_crossing(p: vec2f, a: vec2f, b: vec2f) -> i32 {
    if ((a.y > p.y) == (b.y > p.y)) { return 0; }
    let x_cross = a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x);
    return select(0, 1, p.x < x_cross);
  }
`);

// Closest distance² to a quadratic Bézier via the closed-form
// depressed-cubic root method (Quilez/Cardano) — mirrors
// `closest_on_quad_bezier` + `solve_cubic_real` in f32. Takes the running
// minimum and returns the (possibly improved) minimum so a convex-hull
// lower bound can skip the cubic entirely (the GPU twin of
// `SketchSegment::distance_lower_bound` on the CPU hot path). Every root
// candidate is Newton-polished once and evaluated ON the curve, so f32
// Cardano noise can only overestimate, never underestimate — sphere
// tracing stays safe.
export const pathQuadDistSqFn = wgslFn(`
  fn fd_path_quad_dist_sq(p: vec2f, a: vec2f, b: vec2f, c: vec2f, best_in: f32) -> f32 {
    let lo = min(min(a, b), c);
    let hi = max(max(a, b), c);
    let dv = max(max(lo - p, p - hi), vec2f(0.0));
    if (dot(dv, dv) >= best_in) { return best_in; }
    let M = a - 2.0 * b + c;
    let L = 2.0 * (b - a);
    let W = p - a;
    let mm = dot(M, M);
    let lm = dot(L, M);
    let ll = dot(L, L);
    let wm = dot(W, M);
    let wl = dot(W, L);
    // d/dt|p−B(t)|² = 0 → 2mm·t³ + 3lm·t² + (ll − 2wm)·t − wl = 0
    let A = 2.0 * mm;
    let B = 3.0 * lm;
    let C = ll - 2.0 * wm;
    let D = -wl;
    var roots = vec3f(0.0);
    if (abs(A) < 1e-9) {
      if (abs(B) < 1e-9) {
        if (abs(C) >= 1e-9) {
          roots.x = -D / C;
        }
      } else {
        let disc = C * C - 4.0 * B * D;
        if (disc >= 0.0) {
          let sq = sqrt(disc);
          roots = vec3f((-C + sq) / (2.0 * B), (-C - sq) / (2.0 * B), 0.0);
        }
      }
    } else {
      let b1 = B / A;
      let c1 = C / A;
      let d1 = D / A;
      let pp = c1 - b1 * b1 / 3.0;
      let qq = d1 - b1 * c1 / 3.0 + 2.0 * b1 * b1 * b1 / 27.0;
      let off = -b1 / 3.0;
      let disc = -4.0 * pp * pp * pp - 27.0 * qq * qq;
      if (disc > 0.0) {
        let r = sqrt(-pp / 3.0);
        let cosArg = clamp(-qq / (2.0 * r * r * r), -1.0, 1.0);
        let theta = acos(cosArg) / 3.0;
        let tau3 = 2.0943951023931953;
        roots = vec3f(
          2.0 * r * cos(theta),
          2.0 * r * cos(theta - tau3),
          2.0 * r * cos(theta + tau3),
        ) + vec3f(off);
      } else {
        let s = max(qq * qq / 4.0 + pp * pp * pp / 27.0, 0.0);
        let sq = sqrt(s);
        let u0 = -qq / 2.0 + sq;
        let v0 = -qq / 2.0 - sq;
        let u = sign(u0) * pow(abs(u0), 1.0 / 3.0);
        let v = sign(v0) * pow(abs(v0), 1.0 / 3.0);
        roots = vec3f(u + v + off, 0.0, 0.0);
      }
    }
    let da = p - a;
    let dc = p - c;
    var best = min(dot(da, da), dot(dc, dc));
    // Unused root slots are 0 -> they re-evaluate the t=0 endpoint, which
    // is already in best. Harmless, and skipping branches keeps the loop flat.
    for (var i = 0; i < 3; i = i + 1) {
      var t = roots[i];
      let g = ((A * t + B) * t + C) * t + D;
      let gp = (3.0 * A * t + 2.0 * B) * t + C;
      if (abs(gp) > 1e-12) { t = t - g / gp; }
      t = clamp(t, 0.0, 1.0);
      let s1 = 1.0 - t;
      let bt = a * s1 * s1 + b * 2.0 * s1 * t + c * t * t;
      let d = p - bt;
      best = min(best, dot(d, d));
    }
    return min(best_in, best);
  }
`);

// +x-ray crossing count (0/1) of a y-monotone quadratic Bézier piece —
// mirrors `quad_crossings`/`monotone_quad_root` for pre-split records.
export const pathQuadCrossingFn = wgslFn(`
  fn fd_path_quad_crossing(p: vec2f, a: vec2f, b: vec2f, c: vec2f) -> i32 {
    if ((a.y > p.y) == (c.y > p.y)) { return 0; }
    let ay = a.y - 2.0 * b.y + c.y;
    let by = 2.0 * (b.y - a.y);
    let c0 = a.y - p.y;
    var t: f32;
    if (abs(ay) < 1e-9) {
      t = -c0 / by;
    } else {
      let disc = max(by * by - 4.0 * ay * c0, 0.0);
      var sgn = sign(by);
      if (sgn == 0.0) { sgn = 1.0; }
      let q = -0.5 * (by + sgn * sqrt(disc));
      let r0 = q / ay;
      var r1 = r0;
      if (abs(q) > 1e-30) { r1 = c0 / q; }
      t = select(r1, r0, r0 >= -1e-4 && r0 <= 1.0 + 1e-4);
    }
    t = clamp(t, 0.0, 1.0);
    let s = 1.0 - t;
    let x = s * s * a.x + 2.0 * s * t * b.x + t * t * c.x;
    return select(0, 1, p.x < x);
  }
`);

// Distance² to a circular arc — radial projection if the angle falls inside
// the sweep, else the nearer endpoint (mirrors `closest_on_arc`).
export const pathArcDistSqFn = wgslFn(`
  fn fd_path_arc_dist_sq(p: vec2f, c: vec2f, r: f32, a0: f32, sweep: f32) -> f32 {
    let TAU = 6.283185307179586;
    let d = p - c;
    let theta = atan2(d.y, d.x);
    var ccw = (theta - a0) % TAU;
    if (ccw < 0.0) { ccw += TAU; }
    var in_arc = abs(sweep) >= TAU;
    if (sweep > 0.0) { in_arc = in_arc || ccw <= sweep; }
    else { in_arc = in_arc || (TAU - ccw) <= -sweep; }
    if (in_arc && dot(d, d) > 1e-24) {
      let dd = length(d) - r;
      return dd * dd;
    }
    let ps = c + r * vec2f(cos(a0), sin(a0));
    let pe = c + r * vec2f(cos(a0 + sweep), sin(a0 + sweep));
    let ws = p - ps;
    let we = p - pe;
    return min(dot(ws, ws), dot(we, we));
  }
`);

// +x-ray crossing count (0/1) of a y-monotone arc piece — mirrors
// `arc_crossings` for pre-split records: half-open endpoint rule, then the
// unique sin-branch picked by the piece midpoint's cos sign.
export const pathArcCrossingFn = wgslFn(`
  fn fd_path_arc_crossing(p: vec2f, c: vec2f, r: f32, a0: f32, sweep: f32) -> i32 {
    let PI = 3.141592653589793;
    let TAU = 6.283185307179586;
    let ta = min(a0, a0 + sweep);
    let tb = max(a0, a0 + sweep);
    let ya = c.y + r * sin(ta);
    let yb = c.y + r * sin(tb);
    if ((ya > p.y) == (yb > p.y)) { return 0; }
    let v = clamp((p.y - c.y) / r, -1.0, 1.0);
    var th = asin(v);
    if (cos(0.5 * (ta + tb)) < 0.0) { th = PI - th; }
    th += ceil((ta - th) / TAU) * TAU;
    return select(0, 1, p.x < c.x + r * cos(th));
  }
`);

// ── Shared constant ─────────────────────────────────────────────────

export const NORMAL_EPS = 0.01;
