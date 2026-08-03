// SDF opcode interpreter — a 1:1 C++ port of the Rust CPU interpreter
// `eval_opcodes` in crates/kernel/src/sdf_compiler.rs.
//
// FieldDraft compiles each body's CSG tree to a flat `f32` opcode buffer
// (the "Data Shipping" flat AST). This header lets the C++/CGAL mesher
// evaluate that exact same SDF autonomously — no per-sample FFI callback
// into Rust. The two implementations MUST stay bit-for-bit equivalent;
// the Rust side has `assert_sdf_match` tests pinning `eval_opcodes` to the
// authoritative `BoundedCsg::sdf(.., EvalMode::Visual)`. Mirror any change
// there into here.
//
// Opcode tags and payload layouts are documented in sdf_compiler.rs.
#pragma once

// #384 §8y — Worte je Bogen-Record im OP_ARC_CHANNEL-Payload.
// War 27 (bis §8x); die drei neuen Worte sind e[0], e[1], d_e — die
// autoritative Bogen-Geometrie, die jede Laufzeit-Trigonometrie ersetzt.
// Muss mit sdf_compiler::ARC_STRIDE (Rust) und ARC_STRIDE
// (web/src/sdfVmInterpreter.ts) uebereinstimmen. tape_spec.h zieht die
// Konstante ueber seinen Include dieses Headers.
#ifndef FD_ARC_STRIDE
#define FD_ARC_STRIDE 30
#endif

#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <vector>

namespace fd {

// ── Opcode tags (must match sdf_compiler.rs) ───────────────────────
enum : uint32_t {
  OP_PRIM_BOX       = 0x01,
  OP_PRIM_SPHERE    = 0x02,
  OP_PRIM_CYLINDER  = 0x03,
  OP_PRIM_TORUS     = 0x04,
  OP_PRIM_CONE      = 0x05,
  OP_PRIM_CAPSULE   = 0x06,
  OP_PRIM_HEX_PRISM = 0x07,
  OP_PRIM_ELLIPSOID = 0x08,
  OP_PRIM_CONST     = 0x09,
  OP_PRIM_GYROID    = 0x0A,
  OP_PRIM_THREAD    = 0x0B,
  OP_PRIM_TEX3D     = 0x0C,
  OP_PRIM_HALFSPACE = 0x0D,
  OP_PRIM_CUSTOM    = 0x0E,

  OP_PRIM2D_CIRCLE  = 0x10,
  OP_PRIM2D_RECT    = 0x11,
  OP_PRIM2D_POLY    = 0x12,
  OP_PRIM2D_SECTOR  = 0x13,
  OP_PRIM2D_PATH    = 0x14,

  OP_UNION          = 0x20,
  OP_DIFFERENCE     = 0x21,
  OP_INTERSECT      = 0x22,
  OP_SMOOTH_UNION   = 0x23,
  OP_SMOOTH_DIFF    = 0x24,

  OP_SHELL          = 0x29,
  OP_MORPH          = 0x2A,
  OP_DISPLACE_VAL   = 0x2B,
  OP_EXTRUDE        = 0x2C,
  OP_REVOLVE_SCALE  = 0x2D,
  OP_OFFSET_2D      = 0x2E,
  OP_NEG            = 0x2F,

  OP_WARP_PUSH      = 0x30,
  OP_WARP_POP       = 0x31,
  OP_WARP_TWIST     = 0x32,
  OP_WARP_BEND      = 0x33,
  OP_WARP_TRANSFORM = 0x34,
  OP_WARP_REVOLVE   = 0x35,
  OP_WARP_HELIX     = 0x36,
  OP_HELIX_SCALE    = 0x37,
  OP_WARP_SWEEP     = 0x38,
  OP_LOFT_MORPH     = 0x39,

  OP_MASKED_BLEND   = 0x40,
  OP_ARC_CHANNEL    = 0x41,
  OP_VERTEX_SPHERE  = 0x42,
  OP_RIM_FILLET     = 0x43,
  OP_RIM_FILLET_ARC = 0x44,
  // #384 §8i: payload-loses Basis-Register eines Offset-Stopps
  // (base <- top of stack, stack-neutral).
  OP_STOP_BASE      = 0x45,
  // #384 §8j: Grenzwert-Klemme eines Offset-Stopps (+1 Float: concave).
  OP_STOP_LIMIT     = 0x46,
  // #384 §8o: untere Distanzschranke eines Offset-Stopps
  // (+7 Kopf-Floats + 11 je Randstueck, stack-neutral).
  OP_STOP_LB        = 0x47,

  OP_END            = 0xFF,
};

constexpr float FD_FLT_MAX = 3.40282347e38f;  // mirrors Rust f32::MAX
// #384 §8p: Schwelle "L ist verfuegbar" — das Register traegt entweder
// eine echte Stueck-Distanz oder exakt -1e30 (Spiegel von LB_AVAIL).
constexpr float FD_LB_AVAIL = -1.0e29f;
constexpr int VSTACK_SIZE = 8;
constexpr int PSTACK_SIZE = 4;

// Set when eval_opcodes hits an opcode this port does not implement —
// the evaluation result is unusable (FLT_MAX tail). Checked by the
// mesher to fail fast instead of refining against a garbage field.
inline bool& unknown_opcode_seen() {
  static bool seen = false;
  return seen;
}

inline void report_unknown_opcode(uint32_t tag, int pc) {
  if (!unknown_opcode_seen()) {
    std::fprintf(stderr,
                 "[sdf] ERROR: unknown opcode 0x%02X at pc=%d — "
                 "sdf_interp.h is out of sync with sdf_compiler.rs\n",
                 tag, pc);
  }
  unknown_opcode_seen() = true;
}

inline uint32_t bits_of(float f) {
  uint32_t u;
  std::memcpy(&u, &f, 4);
  return u;
}

inline float fmaxf_(float a, float b) { return a > b ? a : b; }
inline float fminf_(float a, float b) { return a < b ? a : b; }
inline float clampf_(float v, float lo, float hi) {
  return v < lo ? lo : (v > hi ? hi : v);
}
inline float sq_(float x) { return x * x; }

// ── Inline SDF helpers (mirror sdf_compiler.rs) ────────────────────

inline void world_to_local_inline(const float p[3], const float translate[3],
                                  const float quat[4], const float scale[3],
                                  float out[3]) {
  float centered[3] = {p[0] - translate[0], p[1] - translate[1],
                       p[2] - translate[2]};
  float qx = -quat[0], qy = -quat[1], qz = -quat[2], qw = quat[3];
  float t[3] = {
      2.0f * (qy * centered[2] - qz * centered[1]),
      2.0f * (qz * centered[0] - qx * centered[2]),
      2.0f * (qx * centered[1] - qy * centered[0]),
  };
  float rotated[3] = {
      centered[0] + qw * t[0] + qy * t[2] - qz * t[1],
      centered[1] + qw * t[1] + qz * t[0] - qx * t[2],
      centered[2] + qw * t[2] + qx * t[1] - qy * t[0],
  };
  out[0] = rotated[0] / scale[0];
  out[1] = rotated[1] / scale[1];
  out[2] = rotated[2] / scale[2];
}

// Reads translate(3)+quat(4)+scale(3) at *pc, advances *pc by 10,
// localises pt -> out, and returns min(scale) for distance rescaling.
inline float read_transform_and_localise(const float* buf, int* pc,
                                         const float pt[3], float out[3]) {
  float translate[3] = {buf[*pc], buf[*pc + 1], buf[*pc + 2]};
  float quat[4] = {buf[*pc + 3], buf[*pc + 4], buf[*pc + 5], buf[*pc + 6]};
  float scale[3] = {buf[*pc + 7], buf[*pc + 8], buf[*pc + 9]};
  *pc += 10;
  world_to_local_inline(pt, translate, quat, scale, out);
  return fminf_(fminf_(scale[0], scale[1]), scale[2]);
}

inline float sdf_box_local(const float p[3], const float half[3]) {
  float q[3] = {std::fabs(p[0]) - half[0], std::fabs(p[1]) - half[1],
                std::fabs(p[2]) - half[2]};
  float outside = std::sqrt(sq_(fmaxf_(q[0], 0.0f)) + sq_(fmaxf_(q[1], 0.0f)) +
                            sq_(fmaxf_(q[2], 0.0f)));
  float inside = fminf_(fmaxf_(fmaxf_(q[0], q[1]), q[2]), 0.0f);
  return outside + inside;
}

inline float sdf_sphere_local(const float p[3], float r) {
  return std::sqrt(p[0] * p[0] + p[1] * p[1] + p[2] * p[2]) - r;
}

inline float sdf_cylinder_local(const float p[3], float r, float hh) {
  float radial = std::sqrt(p[0] * p[0] + p[2] * p[2]) - r;
  float axial = std::fabs(p[1]) - hh;
  float outside = std::hypot(fmaxf_(radial, 0.0f), fmaxf_(axial, 0.0f));
  float inside = fminf_(fmaxf_(radial, axial), 0.0f);
  return outside + inside;
}

inline float sdf_torus_local(const float p[3], float major, float minor) {
  float q0 = std::sqrt(p[0] * p[0] + p[2] * p[2]) - major;
  return std::sqrt(q0 * q0 + p[1] * p[1]) - minor;
}

inline float sdf_cone_local(const float p[3], float rb, float rt, float hh) {
  float r = std::sqrt(p[0] * p[0] + p[2] * p[2]);
  float t = clampf_((p[1] + hh) / (2.0f * hh), 0.0f, 1.0f);
  float r_at_y = rb * (1.0f - t) + rt * t;
  // Project the radial gap onto the inclined wall normal (Lipschitz-1),
  // mirroring Rust `sdf_cone_local` (wall_scale drift caught by the golden
  // test: a 45-degree cone overestimated by ~40%).
  float drr = rb - rt;
  float slant_len = std::sqrt(drr * drr + (2.0f * hh) * (2.0f * hh));
  float wall_scale = slant_len > 1e-9f ? (2.0f * hh) / slant_len : 1.0f;
  float dr = (r - r_at_y) * wall_scale;
  float da = std::fabs(p[1]) - hh;
  if (dr > 0.0f && da > 0.0f)
    return std::sqrt(dr * dr + da * da);
  return fmaxf_(dr, da);
}

inline float sdf_capsule_local(const float p[3], float r, float hh) {
  float rv = std::sqrt(p[0] * p[0] + p[2] * p[2]);
  float yc = clampf_(p[1], -hh, hh);
  float dy = p[1] - yc;
  return std::sqrt(rv * rv + dy * dy) - r;
}

inline float sdf_hex_prism_local(const float p[3], float apothem, float hh) {
  float qx = std::fabs(p[0]);
  float qz = std::fabs(p[2]);
  float d_hex = fmaxf_(qx * 0.8660254f + qz * 0.5f, qz) - apothem;
  float d_axial = std::fabs(p[1]) - hh;
  if (d_hex > 0.0f && d_axial > 0.0f)
    return std::sqrt(d_hex * d_hex + d_axial * d_axial);
  return fmaxf_(d_hex, d_axial);
}

inline float sdf_ellipsoid_local(const float p[3], const float radii[3]) {
  float r_min = fminf_(fminf_(radii[0], radii[1]), radii[2]);
  float q[3] = {p[0] / radii[0], p[1] / radii[1], p[2] / radii[2]};
  float len = std::sqrt(q[0] * q[0] + q[1] * q[1] + q[2] * q[2]);
  return (len - 1.0f) * r_min;
}

inline float sdf_gyroid_local(const float p[3], float cell_size, float thickness) {
  float s = cell_size;
  float x = p[0] / s, y = p[1] / s, z = p[2] / s;
  float f = std::sin(x) * std::cos(y) + std::sin(y) * std::cos(z) +
            std::sin(z) * std::cos(x);
  return (std::fabs(f) - thickness) * s * 0.2887f;
}

inline float sdf_thread_local(const float p[3], float nominal_radius,
                              float pitch, float depth, float half_height) {
  const float TAU = 6.2831853f;
  float r = std::sqrt(p[0] * p[0] + p[2] * p[2]);
  float theta = std::atan2(p[2], p[0]);
  float y_unwrapped = p[1] - pitch * theta / TAU;
  float phase = (y_unwrapped / pitch) * TAU;
  float r_profile = nominal_radius - depth * 0.5f * (1.0f - std::cos(phase));
  float d_radial = r - r_profile;
  float d_axial = std::fabs(p[1]) - half_height;
  // Lipschitz normalization — mirrors Rust: |∇f| peaks at
  // √(1 + (π·depth/pitch)²) because the helix folds y into the phase.
  const float PI = 3.14159265f;
  float lip = std::sqrt(1.0f + sq_(PI * depth / fmaxf_(pitch, 1e-6f)));
  return fmaxf_(d_radial, d_axial) / lip;
}

inline float sdf_polygon_2d(const float p[2], const float* verts, int n) {
  if (n < 3) return FD_FLT_MAX;
  float d = FD_FLT_MAX;
  float sign = 1.0f;
  int j = n - 1;
  for (int i = 0; i < n; ++i) {
    float vix = verts[2 * i], viy = verts[2 * i + 1];
    float vjx = verts[2 * j], vjy = verts[2 * j + 1];
    float ex = vjx - vix, ey = vjy - viy;
    float wx = p[0] - vix, wy = p[1] - viy;
    float t = (wx * ex + wy * ey) / (ex * ex + ey * ey);
    t = clampf_(t, 0.0f, 1.0f);
    float dx = wx - ex * t, dy = wy - ey * t;
    float dd = dx * dx + dy * dy;
    if (dd < d) d = dd;
    bool c1 = p[1] >= viy;
    bool c2 = p[1] < vjy;
    bool c3 = ex * wy > ey * wx;
    if ((c1 && c2 && c3) || (!c1 && !c2 && !c3)) sign = -sign;
    j = i;
  }
  return sign * std::sqrt(d);
}

// ── Analytic path records (OP_PRIM2D_PATH) ─────────────────────────
//
// C++ port of the GPU path helpers in web/src/sdfRenderer.ts
// (fd_path_*), themselves ports of `fielddraft_protocol::sketch2d`
// (`sdf_path` / `path_is_inside`). Every record in the opcode stream is
// a y-MONOTONE piece (the kernel pre-splits quads at their y-apex and
// arcs at the cardinal angles), so each crossing helper is the plain
// polygon half-open endpoint rule plus one closed-form root.

inline float path_seg_dist_sq(const float p[2], const float a[2],
                              const float b[2]) {
  float ex = b[0] - a[0], ey = b[1] - a[1];
  float wx = p[0] - a[0], wy = p[1] - a[1];
  float t = clampf_((wx * ex + wy * ey) / fmaxf_(ex * ex + ey * ey, 1e-12f),
                    0.0f, 1.0f);
  float dx = wx - ex * t, dy = wy - ey * t;
  return dx * dx + dy * dy;
}

inline int path_seg_crossing(const float p[2], const float a[2],
                             const float b[2]) {
  if ((a[1] > p[1]) == (b[1] > p[1])) return 0;
  float x_cross = a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
  return p[0] < x_cross ? 1 : 0;
}

// Closest distance² to a quadratic Bézier via the closed-form
// depressed-cubic root method (Quilez/Cardano); every root candidate is
// Newton-polished once and evaluated ON the curve. Takes the running
// minimum so the convex-hull lower bound can skip the cubic.
inline float path_quad_dist_sq(const float p[2], const float a[2],
                               const float b[2], const float c[2],
                               float best_in) {
  float lox = fminf_(fminf_(a[0], b[0]), c[0]);
  float loy = fminf_(fminf_(a[1], b[1]), c[1]);
  float hix = fmaxf_(fmaxf_(a[0], b[0]), c[0]);
  float hiy = fmaxf_(fmaxf_(a[1], b[1]), c[1]);
  float dvx = fmaxf_(fmaxf_(lox - p[0], p[0] - hix), 0.0f);
  float dvy = fmaxf_(fmaxf_(loy - p[1], p[1] - hiy), 0.0f);
  if (dvx * dvx + dvy * dvy >= best_in) return best_in;
  float Mx = a[0] - 2.0f * b[0] + c[0], My = a[1] - 2.0f * b[1] + c[1];
  float Lx = 2.0f * (b[0] - a[0]), Ly = 2.0f * (b[1] - a[1]);
  float Wx = p[0] - a[0], Wy = p[1] - a[1];
  float mm = Mx * Mx + My * My;
  float lm = Lx * Mx + Ly * My;
  float ll = Lx * Lx + Ly * Ly;
  float wm = Wx * Mx + Wy * My;
  float wl = Wx * Lx + Wy * Ly;
  // d/dt|p−B(t)|² = 0 → 2mm·t³ + 3lm·t² + (ll − 2wm)·t − wl = 0
  float A = 2.0f * mm, B = 3.0f * lm, C = ll - 2.0f * wm, D = -wl;
  float roots[3] = {0.0f, 0.0f, 0.0f};
  if (std::fabs(A) < 1e-9f) {
    if (std::fabs(B) < 1e-9f) {
      if (std::fabs(C) >= 1e-9f) roots[0] = -D / C;
    } else {
      float disc = C * C - 4.0f * B * D;
      if (disc >= 0.0f) {
        float sq = std::sqrt(disc);
        roots[0] = (-C + sq) / (2.0f * B);
        roots[1] = (-C - sq) / (2.0f * B);
      }
    }
  } else {
    float b1 = B / A, c1 = C / A, d1 = D / A;
    float pp = c1 - b1 * b1 / 3.0f;
    float qq = d1 - b1 * c1 / 3.0f + 2.0f * b1 * b1 * b1 / 27.0f;
    float off = -b1 / 3.0f;
    float disc = -4.0f * pp * pp * pp - 27.0f * qq * qq;
    if (disc > 0.0f) {
      float r = std::sqrt(-pp / 3.0f);
      float cosArg = clampf_(-qq / (2.0f * r * r * r), -1.0f, 1.0f);
      float theta = std::acos(cosArg) / 3.0f;
      const float TAU3 = 2.0943951023931953f;
      roots[0] = 2.0f * r * std::cos(theta) + off;
      roots[1] = 2.0f * r * std::cos(theta - TAU3) + off;
      roots[2] = 2.0f * r * std::cos(theta + TAU3) + off;
    } else {
      float s = fmaxf_(qq * qq / 4.0f + pp * pp * pp / 27.0f, 0.0f);
      float sq = std::sqrt(s);
      float u0 = -qq / 2.0f + sq;
      float v0 = -qq / 2.0f - sq;
      float u = std::copysign(std::pow(std::fabs(u0), 1.0f / 3.0f), u0);
      float v = std::copysign(std::pow(std::fabs(v0), 1.0f / 3.0f), v0);
      roots[0] = u + v + off;
    }
  }
  float dax = p[0] - a[0], day = p[1] - a[1];
  float dcx = p[0] - c[0], dcy = p[1] - c[1];
  float best = fminf_(dax * dax + day * day, dcx * dcx + dcy * dcy);
  // Unused root slots are 0 → they re-evaluate the t=0 endpoint, which
  // is already in best. Harmless.
  for (int i = 0; i < 3; ++i) {
    float t = roots[i];
    float g = ((A * t + B) * t + C) * t + D;
    float gp = (3.0f * A * t + 2.0f * B) * t + C;
    if (std::fabs(gp) > 1e-12f) t -= g / gp;
    t = clampf_(t, 0.0f, 1.0f);
    float s1 = 1.0f - t;
    float btx = a[0] * s1 * s1 + b[0] * 2.0f * s1 * t + c[0] * t * t;
    float bty = a[1] * s1 * s1 + b[1] * 2.0f * s1 * t + c[1] * t * t;
    float dx = p[0] - btx, dy = p[1] - bty;
    best = fminf_(best, dx * dx + dy * dy);
  }
  return fminf_(best_in, best);
}

// +x-ray crossing count (0/1) of a y-monotone quadratic Bézier piece.
inline int path_quad_crossing(const float p[2], const float a[2],
                              const float b[2], const float c[2]) {
  if ((a[1] > p[1]) == (c[1] > p[1])) return 0;
  float ay = a[1] - 2.0f * b[1] + c[1];
  float by = 2.0f * (b[1] - a[1]);
  float c0 = a[1] - p[1];
  float t;
  if (std::fabs(ay) < 1e-9f) {
    t = -c0 / by;
  } else {
    float disc = fmaxf_(by * by - 4.0f * ay * c0, 0.0f);
    float sgn = by > 0.0f ? 1.0f : (by < 0.0f ? -1.0f : 1.0f);
    float q = -0.5f * (by + sgn * std::sqrt(disc));
    float r0 = q / ay;
    float r1 = std::fabs(q) > 1e-30f ? c0 / q : r0;
    t = (r0 >= -1e-4f && r0 <= 1.0f + 1e-4f) ? r0 : r1;
  }
  t = clampf_(t, 0.0f, 1.0f);
  float s = 1.0f - t;
  float x = s * s * a[0] + 2.0f * s * t * b[0] + t * t * c[0];
  return p[0] < x ? 1 : 0;
}

// Distance² to a circular arc — radial projection if the angle falls
// inside the sweep, else the nearer endpoint.
inline float path_arc_dist_sq(const float p[2], const float c[2], float r,
                              float a0, float sweep) {
  const float TAU = 6.283185307179586f;
  float dx = p[0] - c[0], dy = p[1] - c[1];
  float theta = std::atan2(dy, dx);
  float ccw = std::fmod(theta - a0, TAU);
  if (ccw < 0.0f) ccw += TAU;
  bool in_arc = std::fabs(sweep) >= TAU;
  if (sweep > 0.0f) in_arc = in_arc || ccw <= sweep;
  else in_arc = in_arc || (TAU - ccw) <= -sweep;
  float dd2 = dx * dx + dy * dy;
  if (in_arc && dd2 > 1e-24f) {
    float dd = std::sqrt(dd2) - r;
    return dd * dd;
  }
  float psx = c[0] + r * std::cos(a0), psy = c[1] + r * std::sin(a0);
  float pex = c[0] + r * std::cos(a0 + sweep);
  float pey = c[1] + r * std::sin(a0 + sweep);
  float wsx = p[0] - psx, wsy = p[1] - psy;
  float wex = p[0] - pex, wey = p[1] - pey;
  return fminf_(wsx * wsx + wsy * wsy, wex * wex + wey * wey);
}

// +x-ray crossing count (0/1) of a y-monotone arc piece: half-open
// endpoint rule, then the unique sin-branch picked by the piece
// midpoint's cos sign.
inline int path_arc_crossing(const float p[2], const float c[2], float r,
                             float a0, float sweep) {
  const float PI = 3.141592653589793f;
  const float TAU = 6.283185307179586f;
  float ta = fminf_(a0, a0 + sweep);
  float tb = fmaxf_(a0, a0 + sweep);
  float ya = c[1] + r * std::sin(ta);
  float yb = c[1] + r * std::sin(tb);
  if ((ya > p[1]) == (yb > p[1])) return 0;
  float v = clampf_((p[1] - c[1]) / r, -1.0f, 1.0f);
  float th = std::asin(v);
  if (std::cos(0.5f * (ta + tb)) < 0.0f) th = PI - th;
  th += std::ceil((ta - th) / TAU) * TAU;
  return p[0] < c[0] + r * std::cos(th) ? 1 : 0;
}

inline float sdf_sector_2d(const float p[2], const float center[2], float radius,
                           float start_angle, float sweep_angle) {
  const float PI = 3.14159265f;
  const float TAU = 6.2831853f;
  float dx = p[0] - center[0];
  float dy = p[1] - center[1];
  float r = std::sqrt(dx * dx + dy * dy);
  float angle = std::atan2(dy, dx);
  float mid = start_angle + sweep_angle * 0.5f;
  float half = std::fabs(sweep_angle) * 0.5f;
  float rel = angle - mid;
  while (rel > PI) rel -= TAU;
  while (rel < -PI) rel += TAU;
  float d_circ = r - radius;
  if (std::fabs(rel) <= half) {
    return d_circ;
  }
  float edge_angle = rel > 0.0f ? start_angle + sweep_angle : start_angle;
  float ex = center[0] + radius * std::cos(edge_angle);
  float ey = center[1] + radius * std::sin(edge_angle);
  float to_edge = std::sqrt(sq_(p[0] - ex) + sq_(p[1] - ey));
  float to_center = r;
  return fminf_(fmaxf_(d_circ, 0.0f), fminf_(to_edge, to_center));
}

inline float blend_min_eval(float a, float b, float r, float mode) {
  if (mode < 0.5f) {
    // Fillet — Circular-CD smin (#384 Stufe 1, v0.16.2), 1:1-Spiegel des
    // Rust-Kernels csg::smin_poly: min − (r/2)·(1 + h − sqrt(1 − h·(h−2)))
    // mit h = max(r − |a−b|, 0)/r. (Die vorige Form war das alte IQ
    // round-union — sie überschätzte Distanzen quer zur Naht; der
    // Golden-Check meldete die Drift auf den smooth_union-Szenen.)
    if (r <= 0.0f) return fminf_(a, b);
    float h = fmaxf_(r - std::fabs(a - b), 0.0f) / r;
    return fminf_(a, b) - r * 0.5f * (1.0f + h - std::sqrt(1.0f - h * (h - 2.0f)));
  }
  // Chamfer
  const float FRAC_1_SQRT_2 = 0.70710678f;
  return fminf_(fminf_(a, b), (a + b - r) * FRAC_1_SQRT_2);
}

// User expression tape (OP_PRIM_CUSTOM inner records) — 1:1 mirror of
// sdf_compiler.rs::eval_expr_records. Fixed-width (code, payload) records,
// codes are PLAIN SMALL FLOATS (not bit-cast tags); 1.0e6 is the shared
// error sentinel for stack under/overflow and malformed streams.
constexpr int FD_SDF_EXPR_MAX_STACK = 8;

inline float eval_expr_records(const float* records, int len,
                               const float local[3]) {
  float stack[FD_SDF_EXPR_MAX_STACK];
  int sp = 0;
  int i = 0;
  while (i + 1 < len) {
    int code = (int)records[i];
    float payload = records[i + 1];
    i += 2;
    // Leaf pushes.
    if (code >= 0 && code <= 3) {
      if (sp >= FD_SDF_EXPR_MAX_STACK) return 1.0e6f;
      stack[sp++] = code == 0   ? local[0]
                    : code == 1 ? local[1]
                    : code == 2 ? local[2]
                                : payload;
      continue;
    }
    // Unary ops.
    if (code == 8 || code == 11 || code == 12 || code == 13 || code == 14 ||
        code == 15 || code == 16 || code == 17 || code == 20 || code == 21) {
      if (sp == 0) return 1.0e6f;
      float a = stack[sp - 1];
      float r;
      switch (code) {
        case 8: r = -a; break;
        case 11: r = std::fabs(a); break;
        case 12: r = std::sqrt(fmaxf_(a, 0.0f)); break;
        case 13: r = std::sin(a); break;
        case 14: r = std::cos(a); break;
        case 15: r = std::floor(a); break;
        case 16: r = std::exp(a); break;
        case 17: r = a > 0.0f ? std::log(a) : -1.0e6f; break;
        case 20: r = std::asin(clampf_(a, -1.0f, 1.0f)); break;
        default: r = std::acos(clampf_(a, -1.0f, 1.0f)); break;
      }
      stack[sp - 1] = r;
      continue;
    }
    // Binary ops.
    if (sp < 2) return 1.0e6f;
    float b = stack[sp - 1];
    float a = stack[sp - 2];
    sp -= 1;
    float r;
    switch (code) {
      case 4: r = a + b; break;
      case 5: r = a - b; break;
      case 6: r = a * b; break;
      case 7: r = a / b; break;
      case 9: r = fminf_(a, b); break;
      case 10: r = fmaxf_(a, b); break;
      // Pow mirrors Rust exactly: exp(ln(a)·b), 0 for a ≤ 0.
      case 18: r = a > 0.0f ? std::exp(std::log(a) * b) : 0.0f; break;
      case 19: r = std::atan2(a, b); break;
      default: return 1.0e6f;
    }
    stack[sp - 1] = r;
  }
  return sp == 1 ? stack[0] : 1.0e6f;
}

// One retained analytic half-space face (OP_PRIM_HALFSPACE) — mirror of
// sdf_compiler.rs::halfspace_local. `kind` selects plane / infinite
// cylinder / sphere / cone / torus; params are zero-padded to 4 slots.
inline float halfspace_local(const float local[3], float kind,
                             const float p[4]) {
  if (kind < 0.5f) {
    // Plane: normal = (p0, p1, p2), offset = p3.
    return local[0] * p[0] + local[1] * p[1] + local[2] * p[2] - p[3];
  } else if (kind < 1.5f) {
    // Infinite cylinder, radius p0, axis Y.
    return std::sqrt(local[0] * local[0] + local[2] * local[2]) - p[0];
  } else if (kind < 2.5f) {
    // Sphere, radius p0.
    return std::sqrt(local[0] * local[0] + local[1] * local[1] +
                     local[2] * local[2]) -
           p[0];
  } else if (kind < 3.5f) {
    // Cone, half-angle p0, apex at origin, axis Y.
    float rho = std::sqrt(local[0] * local[0] + local[2] * local[2]);
    float sin_a = std::sin(p[0]), cos_a = std::cos(p[0]);
    return rho * cos_a - std::fabs(local[1]) * sin_a;
  } else if (kind < 4.5f) {
    // Torus, major p0, minor p1.
    float rho = std::sqrt(local[0] * local[0] + local[2] * local[2]);
    float q0 = rho - p[0];
    return std::sqrt(q0 * q0 + local[1] * local[1]) - p[1];
  }
  // Non-analytic (TPMS / profile) — never emitted as retained faces.
  return FD_FLT_MAX;
}

inline float dot3_(const float a[3], const float b[3]) {
  return a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
}

inline void normalize3_(float v[3]) {
  float len = std::sqrt(dot3_(v, v));
  if (len > 0.0f) {
    v[0] /= len;
    v[1] /= len;
    v[2] /= len;
  }
}

inline float sdf_capsule_3d(const float p[3], const float a[3], const float b[3],
                            float r) {
  float ab[3] = {b[0] - a[0], b[1] - a[1], b[2] - a[2]};
  float ap[3] = {p[0] - a[0], p[1] - a[1], p[2] - a[2]};
  float dot_ab = ab[0] * ab[0] + ab[1] * ab[1] + ab[2] * ab[2];
  float t = dot_ab > 0.0f
                ? clampf_((ap[0] * ab[0] + ap[1] * ab[1] + ap[2] * ab[2]) / dot_ab,
                          0.0f, 1.0f)
                : 0.0f;
  float dx = ap[0] - ab[0] * t;
  float dy = ap[1] - ab[1] * t;
  float dz = ap[2] - ab[2] * t;
  return std::sqrt(dx * dx + dy * dy + dz * dz) - r;
}

// ── Interpreter ────────────────────────────────────────────────────
//
// Evaluate the compiled opcode buffer at a world-space point.
// `volumes` (for OP_PRIM_TEX3D / ImportedMesh) are not yet wired into
// the CGAL path; the opcode still advances correctly and pushes
// FD_FLT_MAX, matching the Rust "missing volume" fallback.
inline float eval_opcodes(const float* opcodes, int n, float wx, float wy,
                          float wz) {
  float vs[VSTACK_SIZE];
  int vsp = 0;
  float ps[PSTACK_SIZE][3];
  int psp = 0;
  float pt[3] = {wx, wy, wz};
  int pc = 0;
  // #384 §8i: Basis-Register eines Offset-Stopps — der naive Stopp-Wert d0,
  // gesetzt von OP_STOP_BASE, gelesen von OP_ARC_CHANNEL/OP_VERTEX_SPHERE.
  // NICHT `base` nennen: mehrere Opcode-Arme haben ein lokales `int base = pc`,
  // das den Namen sonst still verschattet (und das Gate abschaltet).
  float stop_base = 0.0f;
  // #384 §8j: Grenzwert-Register — max_j(delta - de_j) in konkaver
  // Orientierung. OP_STOP_BASE setzt es zurueck, OP_STOP_LIMIT wendet es an.
  // Ebenfalls NICHT `lim` in den Opcode-Armen wiederverwenden.
  float stop_lim = -1.0e30f;
  // #384 §8o: L-Register — die untere Distanzschranke L(x) <= dist(x, komplement Omega)
  // aus der zugeschnittenen Vereinigungs-B-Rep. OP_STOP_BASE setzt es neutral,
  // OP_STOP_LB fuellt es, die konkaven Gates lesen es als zweite Gate-Basis.
  float stop_lb = -1.0e30f;

  auto push_v = [&](float v) { vs[vsp++] = v; };
  auto pop_v = [&]() { return vs[--vsp]; };
  auto rd = [&]() { return opcodes[pc++]; };

  while (pc < n) {
    uint32_t tag = bits_of(opcodes[pc]);
    pc += 1;

    switch (tag) {
      case OP_PRIM_BOX: {
        float local[3];
        float scale = read_transform_and_localise(opcodes, &pc, pt, local);
        float he[3] = {rd(), rd(), rd()};
        push_v(sdf_box_local(local, he) * scale);
        break;
      }
      case OP_PRIM_SPHERE: {
        float local[3];
        float scale = read_transform_and_localise(opcodes, &pc, pt, local);
        float r = rd();
        push_v(sdf_sphere_local(local, r) * scale);
        break;
      }
      case OP_PRIM_CYLINDER: {
        float local[3];
        float scale = read_transform_and_localise(opcodes, &pc, pt, local);
        float r = rd(), hh = rd();
        push_v(sdf_cylinder_local(local, r, hh) * scale);
        break;
      }
      case OP_PRIM_TORUS: {
        float local[3];
        float scale = read_transform_and_localise(opcodes, &pc, pt, local);
        float major = rd(), minor = rd();
        push_v(sdf_torus_local(local, major, minor) * scale);
        break;
      }
      case OP_PRIM_CONE: {
        float local[3];
        float scale = read_transform_and_localise(opcodes, &pc, pt, local);
        float rb = rd(), rt = rd(), hh = rd();
        push_v(sdf_cone_local(local, rb, rt, hh) * scale);
        break;
      }
      case OP_PRIM_CAPSULE: {
        float local[3];
        float scale = read_transform_and_localise(opcodes, &pc, pt, local);
        float r = rd(), hh = rd();
        push_v(sdf_capsule_local(local, r, hh) * scale);
        break;
      }
      case OP_PRIM_HEX_PRISM: {
        float local[3];
        float scale = read_transform_and_localise(opcodes, &pc, pt, local);
        float ap = rd(), hh = rd();
        push_v(sdf_hex_prism_local(local, ap, hh) * scale);
        break;
      }
      case OP_PRIM_ELLIPSOID: {
        float local[3];
        float scale = read_transform_and_localise(opcodes, &pc, pt, local);
        float radii[3] = {rd(), rd(), rd()};
        push_v(sdf_ellipsoid_local(local, radii) * scale);
        break;
      }
      case OP_PRIM_CONST: {
        float v = rd();
        push_v(v);
        break;
      }
      case OP_PRIM_GYROID: {
        float local[3];
        float scale = read_transform_and_localise(opcodes, &pc, pt, local);
        float cs = rd(), th = rd();
        push_v(sdf_gyroid_local(local, cs, th) * scale);
        break;
      }
      case OP_PRIM_THREAD: {
        float local[3];
        float scale = read_transform_and_localise(opcodes, &pc, pt, local);
        float nr = rd(), pitch = rd(), depth = rd(), hh = rd();
        push_v(sdf_thread_local(local, nr, pitch, depth, hh) * scale);
        break;
      }

      case OP_PRIM_HALFSPACE: {
        float kind = rd();
        float local[3];
        float scale = read_transform_and_localise(opcodes, &pc, pt, local);
        float prm[4] = {rd(), rd(), rd(), rd()};
        push_v(halfspace_local(local, kind, prm) * scale);
        break;
      }

      case OP_PRIM_CUSTOM: {
        // User expression tape: transform(10) + inv_lipschitz + half_extents(3)
        // + op_count + op_count×(code, payload). Field = max(expr·invL, box).
        float local[3];
        float scale = read_transform_and_localise(opcodes, &pc, pt, local);
        float inv_l = rd();
        float he[3] = {rd(), rd(), rd()};
        int nrec = (int)rd();
        const float* records = opcodes + pc;
        pc += 2 * nrec;
        float expr = eval_expr_records(records, 2 * nrec, local) * inv_l;
        float d = fmaxf_(expr, sdf_box_local(local, he));
        push_v(d * scale);
        break;
      }

      case OP_PRIM2D_CIRCLE: {
        float cx = rd(), cy = rd(), r = rd();
        float dx = pt[0] - cx, dy = pt[1] - cy;
        push_v(std::sqrt(dx * dx + dy * dy) - r);
        break;
      }
      case OP_PRIM2D_RECT: {
        float cx = rd(), cy = rd(), hx = rd(), hy = rd();
        float qx = std::fabs(pt[0] - cx) - hx;
        float qy = std::fabs(pt[1] - cy) - hy;
        float outside = std::sqrt(sq_(fmaxf_(qx, 0.0f)) + sq_(fmaxf_(qy, 0.0f)));
        float inside = fminf_(fmaxf_(qx, qy), 0.0f);
        push_v(outside + inside);
        break;
      }
      case OP_PRIM2D_POLY: {
        int np = (int)rd();
        // vertices live inline in the buffer at pc..pc+2*np
        const float* verts = &opcodes[pc];
        pc += 2 * np;
        float p2[2] = {pt[0], pt[1]};
        push_v(sdf_polygon_2d(p2, verts, np));
        break;
      }
      case OP_PRIM2D_SECTOR: {
        float cx = rd(), cy = rd(), r = rd(), start = rd(), sweep = rd();
        float p2[2] = {pt[0], pt[1]};
        float c2[2] = {cx, cy};
        push_v(sdf_sector_2d(p2, c2, r, start, sweep));
        break;
      }
      case OP_PRIM2D_PATH: {
        // Analytic path records — exact distance + exact even-odd
        // parity, mirroring `sketch2d::{sdf_path, path_is_inside}`.
        // Fixed 7-float records: kind(1) + payload(6), pre-split
        // y-monotone by the kernel.
        int nrec = (int)rd();
        bool closed = rd() != 0.0f;
        float p2[2] = {pt[0], pt[1]};
        float d_sq = FD_FLT_MAX;
        int crossings = 0;
        for (int r = 0; r < nrec; ++r) {
          float kind = rd();
          float f[6] = {rd(), rd(), rd(), rd(), rd(), rd()};
          if (kind == 0.0f) {  // Line: start(2) + end(2)
            d_sq = fminf_(d_sq, path_seg_dist_sq(p2, &f[0], &f[2]));
            if (closed) crossings += path_seg_crossing(p2, &f[0], &f[2]);
          } else if (kind == 1.0f) {  // QuadBezier: p0(2)+p1(2)+p2(2)
            d_sq = path_quad_dist_sq(p2, &f[0], &f[2], &f[4], d_sq);
            if (closed) crossings += path_quad_crossing(p2, &f[0], &f[2], &f[4]);
          } else {  // Arc: center(2) + radius + start + sweep
            d_sq = fminf_(d_sq, path_arc_dist_sq(p2, &f[0], f[2], f[3], f[4]));
            if (closed) crossings += path_arc_crossing(p2, &f[0], f[2], f[3], f[4]);
          }
        }
        float d = std::sqrt(d_sq);
        push_v(closed && (crossings % 2 == 1) ? -d : d);
        break;
      }

      case OP_UNION: {
        float b = pop_v(), a = pop_v();
        push_v(fminf_(a, b));
        break;
      }
      case OP_DIFFERENCE: {
        float b = pop_v(), a = pop_v();
        push_v(fmaxf_(a, -b));
        break;
      }
      case OP_INTERSECT: {
        float b = pop_v(), a = pop_v();
        push_v(fmaxf_(a, b));
        break;
      }
      case OP_SMOOTH_UNION: {
        float r = rd(), mode = rd();
        float b = pop_v(), a = pop_v();
        push_v(blend_min_eval(a, b, r, mode));
        break;
      }
      case OP_SMOOTH_DIFF: {
        float r = rd(), mode = rd();
        float b = pop_v(), a = pop_v();
        push_v(-blend_min_eval(-a, b, r, mode));
        break;
      }

      case OP_SHELL: {
        // Closed INWARD shell: max(d, -(d + t)) — mirrors Rust (the old
        // symmetric fabs(d) - t drifted; opened shells compile to
        // OFFSET(-t) + DIFFERENCE subtrees instead).
        float thickness = rd();
        float d = pop_v();
        push_v(fmaxf_(d, -(d + thickness)));
        break;
      }
      case OP_MORPH: {
        float t = rd();
        float b = pop_v(), a = pop_v();
        push_v((1.0f - t) * a + t * b);
        break;
      }
      case OP_LOFT_MORPH: {
        // #385 P2: pop this section's 2D field d_j and the running accumulator,
        // push acc + hat_j(pt.z)·d_j (centred z). loft_hat: rise 0→1 over
        // [z_prev,z_self], fall 1→0 over [z_self,z_next]; first/last
        // (z_prev==z_self / z_next==z_self) saturate to 1. Mirror of
        // `kernel/src/csg.rs::loft_hat`.
        float z_prev = rd(), z_self = rd(), z_next = rd();
        float dj = pop_v();
        float acc = pop_v();
        float rise = 1.0f;
        if (z_self > z_prev)
          rise = fminf_(fmaxf_((pt[2] - z_prev) / (z_self - z_prev), 0.0f), 1.0f);
        float fall = 1.0f;
        if (z_next > z_self)
          fall = fminf_(fmaxf_((z_next - pt[2]) / (z_next - z_self), 0.0f), 1.0f);
        push_v(acc + fminf_(rise, fall) * dj);
        break;
      }
      case OP_DISPLACE_VAL: {
        float depth = rd(), freq = rd();
        float d = pop_v();
        float disp = depth * std::sin(pt[0] * freq) *
                     std::sin(pt[1] * freq * 1.3f) *
                     std::sin(pt[2] * freq * 0.7f);
        push_v(d - disp);
        break;
      }
      case OP_EXTRUDE: {
        float half_h = rd(), scale = rd();
        uint32_t cap_mask = (uint32_t)rd();
        float d2d = pop_v();
        if (cap_mask == 3) {
          // Both caps retained: exact-corner composition (IQ).
          float wz_ = std::fabs(pt[2]) - half_h;
          float outside =
              std::sqrt(sq_(fmaxf_(d2d, 0.0f)) + sq_(fmaxf_(wz_, 0.0f)));
          float inside = fminf_(fmaxf_(d2d, wz_), 0.0f);
          push_v((outside + inside) * scale);
        } else {
          // Opened cap(s): plain max over the retained terms.
          float d = d2d;
          if (cap_mask & 1u) d = fmaxf_(d, pt[2] - half_h);
          if (cap_mask & 2u) d = fmaxf_(d, -pt[2] - half_h);
          push_v(d * scale);
        }
        break;
      }
      case OP_REVOLVE_SCALE: {
        float scale = rd();
        float d = pop_v();
        push_v(d * scale);
        break;
      }
      case OP_OFFSET_2D: {
        float dist = rd();
        float d = pop_v();
        push_v(d - dist);
        break;
      }
      case OP_NEG: {
        float v = pop_v();
        push_v(-v);
        break;
      }

      case OP_WARP_PUSH: {
        ps[psp][0] = pt[0];
        ps[psp][1] = pt[1];
        ps[psp][2] = pt[2];
        psp += 1;
        break;
      }
      case OP_WARP_POP: {
        psp -= 1;
        pt[0] = ps[psp][0];
        pt[1] = ps[psp][1];
        pt[2] = ps[psp][2];
        break;
      }
      case OP_WARP_TWIST: {
        float rate = rd();
        float angle = rate * pt[1];
        float s = std::sin(angle), c = std::cos(angle);
        float px = pt[0];
        pt[0] = c * px - s * pt[2];
        pt[2] = s * px + c * pt[2];
        break;
      }
      case OP_WARP_BEND: {
        float rate = rd();
        float angle = rate * pt[0];
        float s = std::sin(angle), c = std::cos(angle);
        float px = pt[0];
        pt[0] = c * px - s * pt[1];
        pt[1] = s * px + c * pt[1];
        break;
      }
      case OP_WARP_TRANSFORM: {
        float translate[3] = {rd(), rd(), rd()};
        float quat[4] = {rd(), rd(), rd(), rd()};
        float scale[3] = {rd(), rd(), rd()};
        float out[3];
        world_to_local_inline(pt, translate, quat, scale, out);
        pt[0] = out[0];
        pt[1] = out[1];
        pt[2] = out[2];
        break;
      }
      case OP_WARP_REVOLVE: {
        float r = std::sqrt(pt[0] * pt[0] + pt[2] * pt[2]);
        pt[0] = r;
        pt[1] = pt[1];
        pt[2] = 0.0f;
        break;
      }
      case OP_WARP_HELIX: {
        // Helical unwrap into the profile frame — mirror of the Rust arm /
        // helix_profile_point: continuous sweep-parameter clamp; the signed
        // end-overshoot chord rides along in pt[2] (2D sketch ops ignore z)
        // until OP_HELIX_SCALE composes it in.
        float pitch = rd(), hand = rd(), turns = rd(), y_anchor = rd();
        float taper = rd(), r_inner = rd(), start_angle = rd();
        const float TAU = 6.28318530717958647692f;
        const float PI_ = 3.14159265358979323846f;
        float r = std::sqrt(pt[0] * pt[0] + pt[2] * pt[2]);
        float theta = std::atan2(pt[2], pt[0]);
        float th = hand * theta - start_angle;
        float nn = std::round((pt[1] - y_anchor) / pitch - th / TAU);
        float t_n = th + TAU * nn;
        float t_end = TAU * turns;
        float t_c = clampf_(t_n, 0.0f, t_end);
        float w = pt[1] - pitch * t_c / TAU;
        float rp = r - taper * (pt[1] - y_anchor);
        float rr = fmaxf_(fmaxf_(r_inner, pitch / TAU * 0.25f), 1e-3f);
        float l_t = std::fabs(t_n - t_end * 0.5f) - t_end * 0.5f;
        float ell = std::copysign(1.0f, l_t) * fminf_(r, rr) *
                    std::sin(fminf_(std::fabs(l_t), PI_) * 0.5f);
        pt[0] = rp;
        pt[1] = w;
        pt[2] = ell;
        break;
      }
      case OP_HELIX_SCALE: {
        // Compose the 2D profile distance with the cap coordinate stashed
        // in pt[2] by OP_WARP_HELIX / OP_WARP_SWEEP (IQ extrude compose),
        // then apply the baked world·Lipschitz multiplier.
        float scale = rd();
        float d2 = pop_v();
        float ell = pt[2];
        float outside =
            std::sqrt(sq_(fmaxf_(d2, 0.0f)) + sq_(fmaxf_(ell, 0.0f)));
        float inside = fminf_(fmaxf_(d2, ell), 0.0f);
        push_v((outside + inside) * scale);
        break;
      }
      case OP_WARP_SWEEP: {
        // Mirror of SweepPath::cross_section: nearest spine point, project
        // into the interpolated (n, b) frame → (u, v); the cap coordinate w
        // rides along in pt[2] until OP_HELIX_SCALE. Records live at
        // opcodes[base + i*10]: point(3) + n(3) + b(3) + arclen.
        int n = (int)rd();
        bool closed = rd() > 0.5f;
        float total_len = rd();
        float twist = rd();
        int base = pc;
        pc += n * 10;
        auto rec = [&](int i) { return opcodes + base + i * 10; };
        int seg_count = closed ? n : n - 1;
        float best_d2 = FD_FLT_MAX;
        int best_seg = 0;
        float best_t = 0.0f;
        for (int si = 0; si < seg_count; ++si) {
          const float* a = rec(si);
          const float* b = rec((si + 1) % n);
          float ab[3] = {b[0] - a[0], b[1] - a[1], b[2] - a[2]};
          float ap[3] = {pt[0] - a[0], pt[1] - a[1], pt[2] - a[2]};
          float denom = dot3_(ab, ab);
          float t = denom > 1e-12f ? clampf_(dot3_(ap, ab) / denom, 0.0f, 1.0f)
                                   : 0.0f;
          float dx = ap[0] - ab[0] * t;
          float dy = ap[1] - ab[1] * t;
          float dz = ap[2] - ab[2] * t;
          float d2 = dx * dx + dy * dy + dz * dz;
          if (d2 < best_d2) {
            best_d2 = d2;
            best_seg = si;
            best_t = t;
          }
        }
        const float* a0 = rec(best_seg);
        const float* a1 = rec((best_seg + 1) % n);
        float foot[3] = {a0[0] + (a1[0] - a0[0]) * best_t,
                         a0[1] + (a1[1] - a0[1]) * best_t,
                         a0[2] + (a1[2] - a0[2]) * best_t};
        float nf[3] = {a0[3] + (a1[3] - a0[3]) * best_t,
                       a0[4] + (a1[4] - a0[4]) * best_t,
                       a0[5] + (a1[5] - a0[5]) * best_t};
        float bf[3] = {a0[6] + (a1[6] - a0[6]) * best_t,
                       a0[7] + (a1[7] - a0[7]) * best_t,
                       a0[8] + (a1[8] - a0[8]) * best_t};
        normalize3_(nf);
        normalize3_(bf);
        float d[3] = {pt[0] - foot[0], pt[1] - foot[1], pt[2] - foot[2]};
        float u = dot3_(d, nf);
        float v = dot3_(d, bf);
        float s_foot = a0[9] + (a1[9] - a0[9]) * best_t;
        float w;
        if (closed) {
          w = -FD_FLT_MAX;
        } else {
          bool at_end = (best_seg == 0 && best_t <= 1e-6f) ||
                        (best_seg == seg_count - 1 && best_t >= 1.0f - 1e-6f);
          if (at_end) {
            float taxis[3] = {nf[1] * bf[2] - nf[2] * bf[1],
                              nf[2] * bf[0] - nf[0] * bf[2],
                              nf[0] * bf[1] - nf[1] * bf[0]};
            normalize3_(taxis);
            w = std::fabs(dot3_(d, taxis));
          } else {
            w = -fminf_(s_foot, total_len - s_foot);
          }
        }
        // Twist: un-rotate (u, v) by θ = twist · s/L.
        if (twist != 0.0f && total_len > 1e-6f) {
          float theta = -twist * s_foot / total_len;
          float sn = std::sin(theta), cs = std::cos(theta);
          float nu = cs * u - sn * v;
          float nv = sn * u + cs * v;
          u = nu;
          v = nv;
        }
        pt[0] = u;
        pt[1] = v;
        pt[2] = w;
        break;
      }

      case OP_MASKED_BLEND: {
        // Count-prefixed mask polyline (n + n×[x,y,z]) — mirror of the Rust
        // arm: the mask is the min over consecutive capsules (1 point = a
        // sphere, 0 points = no mask).
        int n = (int)rd();
        int base = pc;
        pc += n * 3;
        float mask_r = rd();
        float blend_r = rd();
        float mode = rd();
        bool is_diff = rd() > 0.5f;
        float db = pop_v(), da = pop_v();
        float mask_d = FD_FLT_MAX;
        if (n == 1) {
          mask_d = sdf_capsule_3d(pt, opcodes + base, opcodes + base, mask_r);
        } else {
          for (int i = 0; i + 1 < n; ++i) {
            mask_d = fminf_(mask_d, sdf_capsule_3d(pt, opcodes + base + i * 3,
                                                   opcodes + base + (i + 1) * 3,
                                                   mask_r));
          }
        }
        // #382/#384: EIN Feld für alle Pfade — C1-Crossfade hart → CD-Blend
        // über die innere Tube-Schale (Spiegel von Rust
        // csg::masked_blend_combine; mask_d hier bereits signiert, d. h.
        // Distanz zur Polyline minus mask_r wie im Rust-Arm).
        float signed_mask = mask_d;  // sdf_capsule_3d liefert bereits dist − mask_r
        float blended = is_diff ? -blend_min_eval(-da, db, blend_r, mode)
                                : blend_min_eval(da, db, blend_r, mode);
        float hard = is_diff ? fmaxf_(da, -db) : fminf_(da, db);
        float band = fmaxf_(blend_r, 1e-3f);
        float xw = clampf_((-signed_mask) / band, 0.0f, 1.0f);
        float w = xw * xw * (3.0f - 2.0f * xw);
        push_v(hard + (blended - hard) * w);
        break;
      }

      case OP_STOP_BASE: {
        // #384 §8i: base <- top of stack (pop + push desselben Werts).
        // Spiegel von sdf_compiler.rs OP_STOP_BASE /
        // offset_channel::stop_apply.
        float d = pop_v();
        stop_base = d;
        stop_lim = -1.0e30f;
        // §8o: ohne OP_STOP_LB bleibt das L-Register neutral — das max im
        // konkaven Gate ist dann wirkungslos (Kontroll-Aequivalenz).
        stop_lb = -1.0e30f;
        push_v(d);
        break;
      }

      case OP_STOP_LB: {
        // #384 §8o: untere Distanzschranke des Stopps — 1:1-Spiegel von
        // kernel offset_lb::lb_value / sdf_compiler.rs OP_STOP_LB.
        // Kopf 7: n delta slack center(3) skip_r; je Stueck 14 Floats
        // (Wort 0 = kind: 0 = Dreieck a(3) b(3) c(3) nhat(3) emax, 1 =
        // rotiertes (r,z)-Segment o(3) ax(3) r0 z0 r1 z1 + 3 leere Worte).
        // #384 §8t: nhat/emax sind die vorberechnete Ebenen-Schranke; sie
        // erlauben den EXAKTEN Prune unten (Wert bit-identisch zum flachen
        // Lauf) und gelten nur innerhalb der Fernfeld-Kugel — deshalb steht
        // die Kugel-Pruefung zwingend vor der Schleife.
        const int kLb = 14;
        int n = (int)rd();
        float lb_delta = rd();
        float lb_slack = rd();
        float lc[3] = {rd(), rd(), rd()};
        float lb_skip = rd();
        int lb_base = pc;
        pc += n * kLb;
        float d = pop_v();
        float dcx = pt[0] - lc[0], dcy = pt[1] - lc[1], dcz = pt[2] - lc[2];
        float rc = sqrtf(dcx * dcx + dcy * dcy + dcz * dcz);
        if (d > lb_delta || rc > lb_skip) {
          stop_lb = -1.0e30f;
        } else {
          // §8t: Startwert FLT_MAX statt Rust-INFINITY. Beide sind sicher —
          // `best + emax` ist dann so gross, dass der Prune am ersten Stueck
          // nie feuert —, aber sie sind nicht bit-identisch zueinander; die
          // §8t-Zusage gilt je Auswerter (C++-Prune == C++-flach).
          float best = FD_FLT_MAX;
          for (int k = 0; k < n; ++k) {
            const float* a = opcodes + lb_base + k * kLb;
            float dd;
            if (a[0] < 0.5f) {
              // #384 §8t: exakter Prune (CPU: offset_lb::tri_cannot_beat) —
              // |nhat*(p-a)| >= best + emax beweist dist >= best, das Stueck
              // haette das min also nicht gesenkt.
              {
                const float* nh = a + 10;
                float hh = nh[0] * (pt[0] - a[1]) + nh[1] * (pt[1] - a[2]) +
                           nh[2] * (pt[2] - a[3]);
                if (fabsf(hh) >= best + a[13]) continue;
              }
              // Dreieck (CPU: offset_lb::tri_dist).
              const float* va = a + 1;
              const float* vb = a + 4;
              const float* vc = a + 7;
              float ba[3] = {vb[0] - va[0], vb[1] - va[1], vb[2] - va[2]};
              float cb[3] = {vc[0] - vb[0], vc[1] - vb[1], vc[2] - vb[2]};
              float ac[3] = {va[0] - vc[0], va[1] - vc[1], va[2] - vc[2]};
              float nor[3] = {
                  ba[1] * ac[2] - ba[2] * ac[1],
                  ba[2] * ac[0] - ba[0] * ac[2],
                  ba[0] * ac[1] - ba[1] * ac[0]};
              float nn = nor[0] * nor[0] + nor[1] * nor[1] + nor[2] * nor[2];
              auto seg2 = [&](const float* u, const float* v) {
                float e[3] = {v[0] - u[0], v[1] - u[1], v[2] - u[2]};
                float w[3] = {pt[0] - u[0], pt[1] - u[1], pt[2] - u[2]};
                float ll = e[0] * e[0] + e[1] * e[1] + e[2] * e[2];
                float t = ll > 1e-20f ? (w[0] * e[0] + w[1] * e[1] + w[2] * e[2]) / ll : 0.0f;
                if (t < 0.0f) t = 0.0f;
                if (t > 1.0f) t = 1.0f;
                float q[3] = {w[0] - e[0] * t, w[1] - e[1] * t, w[2] - e[2] * t};
                return q[0] * q[0] + q[1] * q[1] + q[2] * q[2];
              };
              float edge = seg2(va, vb);
              float e2 = seg2(vb, vc);
              if (e2 < edge) edge = e2;
              float e3 = seg2(vc, va);
              if (e3 < edge) edge = e3;
              if (nn <= 1e-24f) {
                dd = sqrtf(edge);
              } else {
                auto sg = [&](const float* e, const float* v) {
                  float cr[3] = {
                      e[1] * nor[2] - e[2] * nor[1],
                      e[2] * nor[0] - e[0] * nor[2],
                      e[0] * nor[1] - e[1] * nor[0]};
                  float w[3] = {pt[0] - v[0], pt[1] - v[1], pt[2] - v[2]};
                  return (cr[0] * w[0] + cr[1] * w[1] + cr[2] * w[2]) >= 0.0f ? 1.0f : -1.0f;
                };
                float ins = sg(ba, va) + sg(cb, vb) + sg(ac, vc);
                if (ins >= 2.0f) {
                  float h = nor[0] * (pt[0] - va[0]) + nor[1] * (pt[1] - va[1]) +
                            nor[2] * (pt[2] - va[2]);
                  dd = sqrtf(h * h / nn);
                } else {
                  dd = sqrtf(edge);
                }
              }
            } else {
              // Rotationsflaeche (CPU: offset_lb::rev_dist).
              const float* o = a + 1;
              const float* ax = a + 4;
              float r0 = a[7], z0 = a[8], r1 = a[9], z1 = a[10];
              float dv[3] = {pt[0] - o[0], pt[1] - o[1], pt[2] - o[2]};
              float t = dv[0] * ax[0] + dv[1] * ax[1] + dv[2] * ax[2];
              float rd3[3] = {dv[0] - ax[0] * t, dv[1] - ax[1] * t, dv[2] - ax[2] * t};
              float r = sqrtf(rd3[0] * rd3[0] + rd3[1] * rd3[1] + rd3[2] * rd3[2]);
              float ex = r1 - r0, ey = z1 - z0;
              float px = r - r0, py = t - z0;
              float ll = ex * ex + ey * ey;
              float sp = 0.0f;
              if (ll > 1e-20f) {
                sp = (px * ex + py * ey) / ll;
                if (sp < 0.0f) sp = 0.0f;
                if (sp > 1.0f) sp = 1.0f;
              }
              float qx = px - ex * sp, qy = py - ey * sp;
              dd = sqrtf(qx * qx + qy * qy);
            }
            if (dd < best) best = dd;
          }
          stop_lb = best < FD_FLT_MAX ? best - lb_slack : -1.0e30f;
        }
        push_v(d);
        break;
      }

      case OP_STOP_LIMIT: {
        // #384 §8j: d <- max(d, lim) konkav bzw. min(d, -lim) konvex.
        // Spiegel von sdf_compiler.rs OP_STOP_LIMIT /
        // offset_channel::stop_apply.
        bool concave = rd() > 0.5f;
        float d = pop_v();
        push_v(concave ? fmaxf_(d, stop_lim) : fminf_(d, -stop_lim));
        break;
      }

      case OP_ARC_CHANNEL: {
        // offset_tree Phase B (#384): Kanal-Korrekturkoerper an einem
        // dualen Offset-Stopp — 1:1-Spiegel des Rust-CPU-Interpreters
        // (sdf_compiler.rs OP_ARC_CHANNEL / offset_channel::channel_apply),
        // inklusive des exakten Bounding-Sphere-Skips. Header 44 Floats
        // (9 + 4x4 End-Halbraeume §8f + 4x4 Fremdflaechen-Halbraeume §8j
        // + 3 Modus/Addenden §8ac),
        // je Bogen FD_ARC_STRIDE:
        // c(3) u(3) v(3) r sweep(<0 = Segment) bc(3) br
        // m1s(3) m2s(3) m1e(3) m2e(3) e(2) d_e   (#384 §8y).
        const int kArc = FD_ARC_STRIDE;
        int n = (int)rd();
        bool concave = rd() > 0.5f;
        float delta = rd();
        float trust = rd();
        float gate_w = rd();
        float bc[3] = {rd(), rd(), rd()};
        float br = rd();
        // Drittes Gate: s3 = min_k(<x, n_k> - c_k) (offset_channel::
        // end_plane_gate). Neutraler Slot: n = 0, c = -1e30.
        float s3 = FD_FLT_MAX;
        for (int g = 0; g < 4; ++g) {
          float gn[3] = {rd(), rd(), rd()};
          float gc = rd();
          float v = pt[0] * gn[0] + pt[1] * gn[1] + pt[2] * gn[2] - gc;
          s3 = fminf_(s3, v);
        }
        // Viertes Gate: s4 = min_k(<x, n_k> - c_k) (offset_channel::
        // face_plane_gate). Gleicher Sentinel.
        float s4 = FD_FLT_MAX;
        for (int g = 0; g < 4; ++g) {
          float gn[3] = {rd(), rd(), rd()};
          float gc = rd();
          float v = pt[0] * gn[0] + pt[1] * gn[1] + pt[2] * gn[2] - gc;
          s4 = fminf_(s4, v);
        }
        // #384 §8ac / P5b-1A: Modus + beide Addenden, vor der Bogenliste.
        // Der Modus geht in den Feldwert nicht ein; die Addenden tun es, und
        // bei LegacyUnproven sind sie null.
        (void)rd();
        float a_seam = rd();
        float a_h = rd();
        int base = pc;
        pc += n * kArc;
        float d = pop_v();
        float dbs = std::sqrt(sq_(pt[0] - bc[0]) + sq_(pt[1] - bc[1]) +
                              sq_(pt[2] - bc[2])) - br;
        // §8j: der Skip muss auch den stop_lim-Beitrag neutral halten —
        // `dbs >= trust` beweist `de >= trust`.
        if ((trust - dbs <= (concave ? -d : d)) && dbs >= trust) {
          push_v(d);
          break;
        }
        float best = FD_FLT_MAX;
        float best_q[3] = {pt[0], pt[1], pt[2]};
        float best_m1[3] = {0, 0, 0};
        float best_m2[3] = {0, 0, 0};
        for (int k = 0; k < n; ++k) {
          const float* a = opcodes + base + k * kArc;
          float dbc = std::sqrt(sq_(pt[0] - a[11]) + sq_(pt[1] - a[12]) +
                                sq_(pt[2] - a[13]));
          if (dbc - a[14] >= best) continue;
          const float* c = a + 0;
          const float* u = a + 3;
          const float* v = a + 6;
          float r = a[9];
          float sweep = a[10];
          float de, q[3], t;
          if (sweep < 0.0f) {
            float len = fmaxf_(r, 1e-12f);
            float dp = (pt[0] - c[0]) * u[0] + (pt[1] - c[1]) * u[1] +
                       (pt[2] - c[2]) * u[2];
            t = clampf_(dp / len, 0.0f, 1.0f);
            q[0] = c[0] + u[0] * (t * len);
            q[1] = c[1] + u[1] * (t * len);
            q[2] = c[2] + u[2] * (t * len);
            de = std::sqrt(sq_(pt[0] - q[0]) + sq_(pt[1] - q[1]) +
                           sq_(pt[2] - q[2]));
          } else {
            float dc[3] = {pt[0] - c[0], pt[1] - c[1], pt[2] - c[2]};
            float x = dc[0] * u[0] + dc[1] * u[1] + dc[2] * u[2];
            float y = dc[0] * v[0] + dc[1] * v[1] + dc[2] * v[2];
            float rho = std::sqrt(x * x + y * y);
            if (rho < 1e-9f) {
              q[0] = c[0] + u[0] * r;
              q[1] = c[1] + u[1] * r;
              q[2] = c[2] + u[2] * r;
              t = 0.0f;
            } else {
              // #384 §8y: rein algebraisch — kein atan2, kein sin/cos.
              float e0 = a[27], e1 = a[28], d_e = a[29];
              float q0[3] = {c[0] + u[0] * r, c[1] + u[1] * r,
                             c[2] + u[2] * r};
              float q1[3] = {c[0] + r * (u[0] * e0 + v[0] * e1),
                             c[1] + r * (u[1] * e0 + v[1] * e1),
                             c[2] + r * (u[2] * e0 + v[2] * e1)};
              float av = y;
              float bv = x * e1 - y * e0;
              if (av >= 0.0f && bv >= 0.0f) {
                float cs = x / rho, sn = y / rho;
                q[0] = c[0] + r * (u[0] * cs + v[0] * sn);
                q[1] = c[1] + r * (u[1] * cs + v[1] * sn);
                q[2] = c[2] + r * (u[2] * cs + v[2] * sn);
                float dm = (x >= 0.0f) ? (y / (x + y)) : (1.0f + (-x) / (y - x));
                t = dm / d_e;
                t = t < 0.0f ? 0.0f : (t > 1.0f ? 1.0f : t);
              } else {
                float d0 = std::sqrt(sq_(pt[0] - q0[0]) + sq_(pt[1] - q0[1]) +
                                     sq_(pt[2] - q0[2]));
                float d1 = std::sqrt(sq_(pt[0] - q1[0]) + sq_(pt[1] - q1[1]) +
                                     sq_(pt[2] - q1[2]));
                if (d0 <= d1) {
                  q[0] = q0[0]; q[1] = q0[1]; q[2] = q0[2];
                  t = 0.0f;
                } else {
                  q[0] = q1[0]; q[1] = q1[1]; q[2] = q1[2];
                  t = 1.0f;
                }
              }
            }
            de = std::sqrt(sq_(pt[0] - q[0]) + sq_(pt[1] - q[1]) +
                           sq_(pt[2] - q[2]));
          }
          if (de < best) {
            best = de;
            best_q[0] = q[0]; best_q[1] = q[1]; best_q[2] = q[2];
            const float* m1s = a + 15;
            const float* m2s = a + 18;
            const float* m1e = a + 21;
            const float* m2e = a + 24;
            float m1[3], m2[3];
            for (int i = 0; i < 3; ++i) {
              m1[i] = m1s[i] + (m1e[i] - m1s[i]) * t;
              m2[i] = m2s[i] + (m2e[i] - m2s[i]) * t;
            }
            float n1 = std::sqrt(sq_(m1[0]) + sq_(m1[1]) + sq_(m1[2]));
            float n2 = std::sqrt(sq_(m2[0]) + sq_(m2[1]) + sq_(m2[2]));
            for (int i = 0; i < 3; ++i) {
              best_m1[i] = n1 > 1e-9f ? m1[i] / n1 : m1s[i];
              best_m2[i] = n2 > 1e-9f ? m2[i] / n2 : m2s[i];
            }
          }
        }
        if (best >= FD_FLT_MAX) {
          push_v(d);
          break;
        }
        float dq[3] = {pt[0] - best_q[0], pt[1] - best_q[1],
                       pt[2] - best_q[2]};
        float s1 = dq[0] * best_m1[0] + dq[1] * best_m1[1] +
                   dq[2] * best_m1[2];
        float s2 = dq[0] * best_m2[0] + dq[1] * best_m2[1] +
                   dq[2] * best_m2[2];
        // §8i: Gate gegen das Basis-Register (naiver Stopp-Wert d0).
        // §8o/§8p: ist L verfuegbar, gatet L - delta ALLEIN; sonst bleibt die
        // G-Kappe (Spiegel von offset_channel::concave_gate).
        float gate = concave ? (stop_lb > FD_LB_AVAIL ? stop_lb - delta
                                                      : gate_w - stop_base)
                             : gate_w + stop_base;
        // Rollen-Trennung (C4, CPU-Referenz channel_apply): Geometrie-
        // Klemme (Luft-Distanzschranke) vs. Add-Gate.
        float w_geo = fminf_(fminf_(fminf_(fminf_(s1, s2), s3), s4),
                             fminf_(best - delta, trust - best));
        float w = fminf_(w_geo, gate);
        // §8j: Beitrag zum Grenzwert-Register.
        if (best < trust) {
          // #384 §8ac: de_safe = fl(de + A_seam), h_safe = fl(fl(s4+delta) + A_h).
          stop_lim = fmaxf_(stop_lim,
                            delta - fminf_(best + a_seam, (s4 + delta) + a_h));
        }
        push_v(concave
                   ? (w > 0.0f ? fminf_(d, -w)
                               : (w_geo <= 0.0f ? fminf_(d, -w_geo) : d))
                   : fmaxf_(d, w));
        break;
      }

      case OP_VERTEX_SPHERE: {
        // offset_tree Phase C4 (#384): Rossignac-Requicha-Vertex-Kugel an
        // einer Triple-Junction — 1:1-Spiegel des Rust-CPU-Interpreters
        // (sdf_compiler.rs OP_VERTEX_SPHERE / offset_channel::vertex_apply).
        // Payload 16: concave delta trust gate v(3) g1(3) g2(3) g3(3).
        bool concave = rd() > 0.5f;
        float delta = rd();
        float trust = rd();
        float gate_w = rd();
        float v[3] = {rd(), rd(), rd()};
        float g1[3] = {rd(), rd(), rd()};
        float g2[3] = {rd(), rd(), rd()};
        float g3[3] = {rd(), rd(), rd()};
        float d = pop_v();
        float dq[3] = {pt[0] - v[0], pt[1] - v[1], pt[2] - v[2]};
        float rho = std::sqrt(sq_(dq[0]) + sq_(dq[1]) + sq_(dq[2]));
        // §8j: Beitrag zum Grenzwert-Register (v liegt auf dOmega).
        if (rho < trust) {
          stop_lim = fmaxf_(stop_lim, delta - rho);
        }
        if (trust - rho <= (concave ? -d : d)) {
          push_v(d);
          break;
        }
        float s1 = dq[0] * g1[0] + dq[1] * g1[1] + dq[2] * g1[2];
        float s2 = dq[0] * g2[0] + dq[1] * g2[1] + dq[2] * g2[2];
        float s3 = dq[0] * g3[0] + dq[1] * g3[1] + dq[2] * g3[2];
        // §8i: Gate gegen das Basis-Register (naiver Stopp-Wert d0).
        // §8o/§8p: ist L verfuegbar, gatet L - delta ALLEIN; sonst bleibt die
        // G-Kappe (Spiegel von offset_channel::concave_gate).
        float gate = concave ? (stop_lb > FD_LB_AVAIL ? stop_lb - delta
                                                      : gate_w - stop_base)
                             : gate_w + stop_base;
        // Rollen-Trennung (C4, CPU-Referenz vertex_apply).
        float w_geo = fminf_(fminf_(fminf_(s1, s2), fminf_(s3, rho - delta)),
                             trust - rho);
        float w = fminf_(w_geo, gate);
        push_v(concave
                   ? (w > 0.0f ? fminf_(d, -w)
                               : (w_geo <= 0.0f ? fminf_(d, -w_geo) : d))
                   : fmaxf_(d, w));
        break;
      }

      case OP_RIM_FILLET: {
        // offset_tree Phase 3 (#398): Rand-Fillet an ∂P-Kanten — 1:1-Spiegel
        // des Rust-CPU-Interpreters (sdf_compiler.rs OP_RIM_FILLET /
        // offset_rim::apply_rims_f32). Nearest-Feature-Komposition: strikt
        // kleinstes de gewinnt (Gleichstand → erster Kandidat), delta-de
        // konkav / de-delta konvex, mit den drei Masken der Referenz — Trust
        // (de <= trust), End-Caps (t in [0,1] strikt, cap_eps 1e-6, nur wenn
        // seg > 1e-9) und Wand-Halbraeume (eps 1e-9; ungenutzte n_c mit
        // Norm < 0.5 -> Maske true). Header n(1) concave(1), je Record 17
        // Floats: a(3) b(3) n_a(3) n_b(3) n_c(3) delta(1) trust(1).
        const int kRim = 17;
        int n = (int)rd();
        bool concave = rd() > 0.5f;
        int base = pc;
        pc += n * kRim;
        float g = pop_v();
        float best_de = FD_FLT_MAX;
        float best_corr = 0.0f;
        for (int k = 0; k < n; ++k) {
          const float* rec = opcodes + base + k * kRim;
          const float* ra = rec + 0;
          const float* rb = rec + 3;
          const float* n_a = rec + 6;
          const float* n_b = rec + 9;
          const float* n_c = rec + 12;
          float delta = rec[15];
          float trust = rec[16];
          // closest_t: geklemmter Lotfusspunkt q, ungeklemmter Parameter t.
          float d[3] = {rb[0] - ra[0], rb[1] - ra[1], rb[2] - ra[2]};
          float len2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
          float de, q[3], t;
          if (len2 < 1e-24f) {
            q[0] = ra[0]; q[1] = ra[1]; q[2] = ra[2];
            t = 0.0f;
            de = std::sqrt(sq_(pt[0] - ra[0]) + sq_(pt[1] - ra[1]) +
                           sq_(pt[2] - ra[2]));
          } else {
            float t_raw = ((pt[0] - ra[0]) * d[0] + (pt[1] - ra[1]) * d[1] +
                           (pt[2] - ra[2]) * d[2]) / len2;
            float tc = clampf_(t_raw, 0.0f, 1.0f);
            q[0] = ra[0] + d[0] * tc;
            q[1] = ra[1] + d[1] * tc;
            q[2] = ra[2] + d[2] * tc;
            de = std::sqrt(sq_(pt[0] - q[0]) + sq_(pt[1] - q[1]) +
                           sq_(pt[2] - q[2]));
            t = t_raw;
          }
          if (de > trust) continue;
          float seg = std::sqrt(sq_(rb[0] - ra[0]) + sq_(rb[1] - ra[1]) +
                                sq_(rb[2] - ra[2]));
          if (seg > 1e-9f && (t < -1e-6f || t > 1.0f + 1e-6f)) continue;
          float dq[3] = {pt[0] - q[0], pt[1] - q[1], pt[2] - q[2]};
          auto side = [&](const float* nn) -> bool {
            if (std::sqrt(sq_(nn[0]) + sq_(nn[1]) + sq_(nn[2])) < 0.5f)
              return true;  // ungenutzte dritte Wand
            float s = dq[0] * nn[0] + dq[1] * nn[1] + dq[2] * nn[2];
            return concave ? (s <= 1e-9f) : (s >= -1e-9f);
          };
          if (!(side(n_a) && side(n_b) && side(n_c))) continue;
          // Nearest-Feature: strikt kleineres de gewinnt (Gleichstand ->
          // erster Kandidat, wie apply_rims_f32).
          if (de < best_de) {
            best_de = de;
            best_corr = concave ? (delta - de) : (de - delta);
          }
        }
        if (best_de < FD_FLT_MAX) {
          push_v(concave ? fminf_(g, best_corr) : fmaxf_(g, best_corr));
        } else {
          push_v(g);
        }
        break;
      }

      case OP_RIM_FILLET_ARC: {
        // FC Phase 4 (#398): GEMISCHTER Rand-Fillet mit BOGEN-Spines — 1:1-Port
        // von offset_rim::apply_rims_arc_f32 / eval_opcodes OP_RIM_FILLET_ARC.
        // Nearest-Feature ueber Segment- (sweep<0) UND Bogen-Records (sweep>0).
        // Header n(1) concave(1); je Record 20 Floats: c(3) u(3) v(3) n_a(3)
        // n_b(3) r(1) sweep(1) wall_sign(1) delta(1) trust(1).
        const int kArc = 20;
        int n = (int)rd();
        bool concave = rd() > 0.5f;
        int base = pc;
        pc += n * kArc;
        float g = pop_v();
        float best_de = FD_FLT_MAX;
        float best_corr = 0.0f;
        for (int k = 0; k < n; ++k) {
          const float* rec = opcodes + base + k * kArc;
          const float* c = rec + 0;
          const float* u = rec + 3;
          const float* v = rec + 6;
          const float* n_a = rec + 9;
          const float* n_bc = rec + 12;  // Segment: konstante Wandnormale
          float r = rec[15];
          float sweep = rec[16];
          float wall_sign = rec[17];
          float delta = rec[18];
          float trust = rec[19];
          const float cap_eps = 1e-6f;
          float q[3];
          float de;
          bool in_cap;
          float n_b[3];
          if (sweep < 0.0f) {
            // Segment c -> c + u*r (u Einheitsrichtung, r Laenge).
            float len = r > 1e-12f ? r : 1e-12f;
            float s = (pt[0] - c[0]) * u[0] + (pt[1] - c[1]) * u[1] + (pt[2] - c[2]) * u[2];
            float t = s / len;
            float sc = clampf_(s, 0.0f, len);
            q[0] = c[0] + u[0] * sc;
            q[1] = c[1] + u[1] * sc;
            q[2] = c[2] + u[2] * sc;
            de = std::sqrt(sq_(pt[0] - q[0]) + sq_(pt[1] - q[1]) + sq_(pt[2] - q[2]));
            in_cap = !(t < -cap_eps || t > 1.0f + cap_eps);
            n_b[0] = n_bc[0]; n_b[1] = n_bc[1]; n_b[2] = n_bc[2];
          } else {
            float dc[3] = {pt[0] - c[0], pt[1] - c[1], pt[2] - c[2]};
            float x = dc[0] * u[0] + dc[1] * u[1] + dc[2] * u[2];
            float y = dc[0] * v[0] + dc[1] * v[1] + dc[2] * v[2];
            float phi = std::atan2(y, x);
            if (phi < 0.0f) phi += 6.28318530717958647692f;
            in_cap = phi <= sweep + cap_eps;
            float pc2 = clampf_(phi, 0.0f, sweep);
            float sn = std::sin(pc2), cs = std::cos(pc2);
            q[0] = c[0] + r * (u[0] * cs + v[0] * sn);
            q[1] = c[1] + r * (u[1] * cs + v[1] * sn);
            q[2] = c[2] + r * (u[2] * cs + v[2] * sn);
            de = std::sqrt(sq_(pt[0] - q[0]) + sq_(pt[1] - q[1]) + sq_(pt[2] - q[2]));
            float d[3] = {q[0] - c[0], q[1] - c[1], q[2] - c[2]};
            float nn = std::sqrt(sq_(d[0]) + sq_(d[1]) + sq_(d[2]));
            if (nn < 1e-30f) nn = 1e-30f;
            n_b[0] = wall_sign * d[0] / nn;
            n_b[1] = wall_sign * d[1] / nn;
            n_b[2] = wall_sign * d[2] / nn;
          }
          if (!in_cap || de > trust) continue;
          float dq[3] = {pt[0] - q[0], pt[1] - q[1], pt[2] - q[2]};
          auto side = [&](const float* nn) -> bool {
            if (std::sqrt(sq_(nn[0]) + sq_(nn[1]) + sq_(nn[2])) < 0.5f) return true;
            float s = dq[0] * nn[0] + dq[1] * nn[1] + dq[2] * nn[2];
            return concave ? (s <= 1e-9f) : (s >= -1e-9f);
          };
          if (!(side(n_a) && side(n_b))) continue;
          if (de < best_de) {
            best_de = de;
            best_corr = concave ? (delta - de) : (de - delta);
          }
        }
        if (best_de < FD_FLT_MAX) {
          push_v(concave ? fminf_(g, best_corr) : fmaxf_(g, best_corr));
        } else {
          push_v(g);
        }
        break;
      }

      case OP_PRIM_TEX3D: {
        // Volume sampling not yet wired into the CGAL path. Advance the
        // payload exactly (tex_idx + transform(10) + aabb_min(3) +
        // aabb_max(3)) and push the missing-volume fallback.
        (void)rd();  // tex_idx
        float local[3];
        (void)read_transform_and_localise(opcodes, &pc, pt, local);
        pc += 6;  // aabb_min(3) + aabb_max(3)
        push_v(FD_FLT_MAX);
        break;
      }

      case OP_END:
        goto done;
      default:
        // Unknown opcode = interpreter drift vs. sdf_compiler.rs. The
        // result field is garbage (FLT_MAX) from here on — say so loudly
        // ONCE instead of letting CGAL search forever for a surface
        // that does not exist.
        report_unknown_opcode(tag, pc - 1);
        goto done;
    }
  }
done:
  return vsp > 0 ? vs[vsp - 1] : FD_FLT_MAX;
}

}  // namespace fd
