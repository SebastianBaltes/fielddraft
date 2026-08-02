//! SDF compiler: [`BoundedCsg`] → flat opcode buffer for GPU raymarching.
//!
//! The compiler traverses the BVH-wrapped CSG tree in postfix order and
//! emits a compact `Vec<f32>` that a GPU stack machine can interpret.
//! Each body gets its own opcode buffer.  2D sketch primitives (for
//! Extrude/Revolve) are inlined directly into the 3D stream — the
//! shader evaluates them at `(point.x, point.y)` in the local frame
//! established by a preceding `WarpTransform`.
//!
//! A CPU interpreter ([`eval_opcodes`]) mirrors the GPU logic 1:1 so
//! that every opcode path can be tested without a GPU.

use fielddraft_protocol::{BakedVolume, HalfSpace, Primitive, Transform};

use crate::bvh::BoundedCsg;
use crate::csg::{Aabb, BlendMode};
use crate::sketch2d::{BoundedSketch2d, Sketch2dPrimitive, SketchSegment};
use crate::quat_rotate;

// ── Opcode tags ────────────────────────────────────────────────────
//
// Encoded as `f32::from_bits(tag)` in the buffer.  The GPU shader
// reads the f32, does `bitcast<u32>`, and dispatches on the tag.

// 3D primitives — push SDF(point) onto value stack.
// Transform (translate 3 + quat 4 + scale 3 = 10 floats) is embedded.
pub const OP_PRIM_BOX: u32       = 0x01; // +11: transform(8) + half_extents(3)
pub const OP_PRIM_SPHERE: u32    = 0x02; // +9:  transform(8) + radius(1)
pub const OP_PRIM_CYLINDER: u32  = 0x03; // +10: transform(8) + radius(1) + half_h(1)
pub const OP_PRIM_TORUS: u32     = 0x04; // +10: transform(8) + major_r(1) + minor_r(1)
pub const OP_PRIM_CONE: u32      = 0x05; // +11: transform(8) + r_bottom(1) + r_top(1) + half_h(1)
pub const OP_PRIM_CAPSULE: u32   = 0x06; // +10: transform(8) + radius(1) + half_h(1)
pub const OP_PRIM_HEX_PRISM: u32 = 0x07; // +10: transform(8) + apothem(1) + half_h(1)
pub const OP_PRIM_ELLIPSOID: u32 = 0x08; // +11: transform(8) + radii(3)
pub const OP_PRIM_CONST: u32     = 0x09; // +1:  constant distance
pub const OP_PRIM_GYROID: u32    = 0x0A; // +10: transform(8) + cell_size(1) + thickness(1)
pub const OP_PRIM_THREAD: u32    = 0x0B; // +12: transform(8) + nom_r + pitch + depth + half_h
pub const OP_PRIM_TEX3D: u32     = 0x0C; // +15: tex_idx(1) + transform(8) + aabb_min(3) + aabb_max(3)
pub const OP_PRIM_HALFSPACE: u32 = 0x0D; // +15: kind(1) + transform(10) + params(4) — one analytic half-space face (retained-boundary shell)
pub const OP_PRIM_CUSTOM: u32    = 0x0E; // +15+2N: transform(10) + inv_lipschitz(1) + half_extents(3) + op_count(1) + ops(2*N, record = code(1) + payload(1)); user expression tape, field = max(expr·inv_L, box)

// 2D primitives — evaluate at (point.x, point.y), push onto stack.
pub const OP_PRIM2D_CIRCLE: u32  = 0x10; // +3: center(2) + radius(1)
pub const OP_PRIM2D_RECT: u32    = 0x11; // +4: center(2) + half_extents(2)
pub const OP_PRIM2D_POLY: u32    = 0x12; // +1+2N: vertex_count(1) + vertices(2*N)
pub const OP_PRIM2D_SECTOR: u32  = 0x13; // +5: center(2) + radius(1) + start(1) + sweep(1)
pub const OP_PRIM2D_PATH: u32    = 0x14; // +2+7N: seg_count(1) + closed(1) + records(7*N), record = kind(1) + payload(6); every record is y-monotone

// Binary operators — pop 2, push 1.
pub const OP_UNION: u32          = 0x20; // +0
pub const OP_DIFFERENCE: u32     = 0x21; // +0
pub const OP_INTERSECT: u32      = 0x22; // +0
pub const OP_SMOOTH_UNION: u32   = 0x23; // +2: radius(1) + mode(1)
pub const OP_SMOOTH_DIFF: u32    = 0x24; // +2: radius(1) + mode(1)

// Unary / compositor operators.
pub const OP_SHELL: u32          = 0x29; // +1: thickness — pops 1, closed inward shell max(d, −(d+t))
pub const OP_MORPH: u32          = 0x2A; // +1: t — pops 2, pushes lerp
pub const OP_DISPLACE_VAL: u32   = 0x2B; // +2: depth + freq — pops 1, uses point
pub const OP_EXTRUDE: u32        = 0x2C; // +3: half_height + scale + cap_mask(1=top,2=bottom retained) — pops d2d, uses point.z
pub const OP_REVOLVE_SCALE: u32  = 0x2D; // +1: scale — pops d2d, multiplies by scale
pub const OP_OFFSET_2D: u32      = 0x2E; // +1: distance — pops 1, pushes d - distance
pub const OP_NEG: u32            = 0x2F; // +0: pops 1, pushes −v — retained path fields are −unsigned distance

// Domain warps — modify the point register.
pub const OP_WARP_PUSH: u32      = 0x30; // +0: save point to point stack
pub const OP_WARP_POP: u32       = 0x31; // +0: restore point from point stack
pub const OP_WARP_TWIST: u32     = 0x32; // +1: rate — rotate XZ by rate*y
pub const OP_WARP_BEND: u32      = 0x33; // +1: rate — rotate XY by rate*x
pub const OP_WARP_TRANSFORM: u32 = 0x34; // +10: translate(3) + quat(4) + scale(3)
pub const OP_WARP_REVOLVE: u32   = 0x35; // +0: point ← (√(x²+z²), y, 0)
pub const OP_WARP_HELIX: u32     = 0x36; // +7: pitch + hand + turns + y_anchor + taper + r_inner + start_angle — point ← (r−taper·(y−y0), w, ell)
pub const OP_HELIX_SCALE: u32    = 0x37; // +1: scale (world·lipschitz, baked at emission) — pops d2d, composes with point.z (end caps), multiplies
pub const OP_WARP_SWEEP: u32     = 0x38; // +4 header (N, closed, total_len, twist) + N×10 (point3, n3, b3, arclen); nearest spine point → point ← (twist-rotated u, v, w_cap); compose via OP_HELIX_SCALE

// #385 P2: loft cross-section morph accumulator. Pops the 2D field `d_j` of one
// section AND the running accumulator `acc`, pushes `acc + hat_j(point.z)·d_j`
// (the partition-of-unity tent weight `crate::csg::loft_hat`, centred z). One
// op per section; a leading `OP_PRIM_CONST 0` seeds `acc`, a trailing
// `OP_EXTRUDE` applies the symmetric z-clamp — see `emit_node`'s Loft arm.
//
// TAPE-LAYOUT DECISION (plan §P2 grants latitude "kleinste ehrliche Kodierung"):
// the plan sketched a two-opcode `OP_WARP_LOFT` (bracket select) + `OP_LOFT_MORPH`
// (2D lerp) pair for the N=2 case. For a STATIC tape over N profiles the bracket
// cannot be selected by a jump, so the honest encoding is a partition-of-unity
// accumulator: every section's field is weighted by its tent basis and summed —
// exactly the bracket lerp over any interior segment (Σ hat_j ≡ 1). That folds
// bracket-selection AND the lerp into this ONE compositor; per-section reframing
// reuses the existing `OP_WARP_TRANSFORM` (0x34), and the z-clamp reuses
// `OP_EXTRUDE` (0x2C). So a single new opcode (0x39) suffices; the reserved
// `OP_WARP_LOFT` (0x3A) is intentionally NOT defined.
pub const OP_LOFT_MORPH: u32     = 0x39; // +3: z_prev + z_self + z_next — pops d_j + acc, pushes acc + hat·d_j

// Masked blend — pops 2 (da, db), evaluates capsule mask, pushes 1.
pub const OP_MASKED_BLEND: u32   = 0x40; // +var: n + n×point(3) + mask_r+blend_r+mode+is_diff

// offset_tree Phase B (#384): Kanal-Korrekturkörper an einem dualen
// Offset-Stopp — pops den naiven Stopp-Wert d, pusht die korrigierte
// Distanz (CPU-Referenz: `offset_channel::channel_apply`).
// Header (41): n_arcs(1) + concave(1) + delta(1) + trust(1) + gate(1) +
//   bound_c(3) + bound_r(1) + end_planes(4×4 = 16; #384 §8f, je Slot
//   [nx,ny,nz,c] mit s₃ₖ = ⟨x,n̂ₖ⟩ − cₖ, neutraler Slot [0,0,0,−1e30]) +
//   face_planes(4×4 = 16; #384 §8j, IDENTISCHES Slot-Format und derselbe
//   `min` — die Quelle unterscheidet sich: das dritte Gate begrenzt ENTLANG
//   der Naht, das vierte QUER dazu gegen die nächste stützende Fremdfläche).
// je Bogen (27): c(3) u(3) v(3) r(1) sweep(1; −1 = Segment) bc(3) br(1)
//   m1s(3) m2s(3) m1e(3) m2e(3).
pub const OP_ARC_CHANNEL: u32    = 0x41; // +41+27N

// offset_tree Phase C4 (#384): Rossignac–Requicha-Vertex-Kugel an einer
// Triple-Junction — pops den Stopp-Wert d, pusht die korrigierte Distanz
// (CPU-Referenz: `offset_channel::vertex_apply`).
// Payload (16): concave(1) + delta(1) + trust(1) + gate(1) + v(3) +
//   g1(3) + g2(3) + g3(3).
pub const OP_VERTEX_SPHERE: u32  = 0x42; // +16

// offset_tree Phase 2 (#398): Rand-Fillet an den ∂P-Reflexkanten des
// Kontakt-Prismas — pops den naiven (Prisma-)Stopp-Wert g, pusht die
// Nearest-Feature-korrigierte Distanz (CPU-Referenz:
// `offset_rim::apply_rims_f32`; f64-Vertrag: `offset_rim::apply_rims`).
// Header (2): n_features(1) + concave(1; 0/1, homogen je Stopp).
// je Feature (17 = RIM_STRIDE): a(3) b(3) n_a(3) n_b(3) n_c(3) delta(1) trust(1);
//   ein Record = f32-Projektion EINES `offset_rim::RimFeature` (ohne concave).
// Wird in Phase 2 NICHT im Regen emittiert (nur Tests konstruieren Blöcke) —
// das ausgelieferte Feld bleibt bit-identisch; Phase 4 schaltet die Emission
// scharf, sobald alle Evaluator-Spiegel (WGSL/TSL/CGAL/fidget) stehen.
pub const OP_RIM_FILLET: u32     = 0x43; // +2+17N

// offset_tree FC Phase 4 (#398): GEMISCHTER Rand-Fillet-Block mit BOGEN-Spines —
// pops den naiven (Prisma-)Stopp-Wert g, pusht die Nearest-Feature-korrigierte
// Distanz über Segment- UND Bogen-Records (CPU-Referenz:
// `offset_rim::apply_rims_arc_f32`; f64-Vertrag: `offset_rim::apply_rims_arc`).
// Wird NUR emittiert, wenn ein Stopp eine konvexe Bogen-∂P-Kante trägt; dann
// wandern ALLE Features (auch die geraden, als Segment-Records `sweep < 0`) in
// diesen Block, und OP_RIM_FILLET entfällt für den Stopp (Feldneutralität: rein
// gerade Kontakte emittieren weiter 0x43, byte-identisch).
// Header (2): n_features(1) + concave(1; 0/1, homogen je Stopp).
// je Feature (20 = RIM_ARC_STRIDE): c(3) u(3) v(3) n_a(3) n_b(3) r(1) sweep(1)
//   wall_sign(1) delta(1) trust(1). Segment (sweep<0): c=Start, u=Einheitsricht.,
//   r=Länge, n_b=konstante Wandnormale. Bogen (sweep>0): c=Zentrum, (u,v)=Basis,
//   r=Radius, Wandnormale RADIAL = wall_sign·(q−c)/r (n_b ungenutzt).
pub const OP_RIM_FILLET_ARC: u32 = 0x44; // +2+20N

// offset_tree §8i (#384): **Basis-Register eines Offset-Stopps.** Payload-los;
// pops den naiven Stopp-Wert d₀, merkt ihn im Skalar-Register `base` und pusht
// ihn unverändert zurück (stack-neutral — `base ← top of stack`). Alle
// folgenden OP_ARC_CHANNEL/OP_VERTEX_SPHERE desselben Stopps gaten gegen
// `base` statt gegen den laufenden Stack-Wert; damit ist die Ein-Term-Schranke
// `G` global und die Term-Reihenfolge feldwert-neutral (CPU-Referenz:
// `offset_channel::stop_apply`).
//
// EIN Register genügt für beliebige Verschachtelung: der Opcode steht
// unmittelbar vor dem Term-Lauf seines Stopps, und ein innerer Stopp im
// naiven Teilbaum ist vollständig abgearbeitet, bevor der äußere sein
// `base` setzt. Emittiert wird er nur, wenn der Stopp überhaupt Kanäle
// oder Vertex-Kugeln trägt.
pub const OP_STOP_BASE: u32      = 0x45; // +0

// offset_tree §8j (#384): **Grenzwert-Klemme eines Offset-Stopps.** Ein
// Payload-Float (`concave`); pops den korrigierten Stopp-Wert und pusht ihn,
// nach unten (konkav) bzw. oben (konvex) durch das Grenzwert-Register `lim`
// geklemmt. `lim` wird von `OP_STOP_BASE` auf −1e30 zurückgesetzt und von
// jedem OP_ARC_CHANNEL/OP_VERTEX_SPHERE mit `δ − min(de, h)` bzw. `δ − ρ`
// hochgezogen (nur innerhalb des Trust-Radius).
//
// Warum: jeder Naht-/Ecken-Punkt liegt auf `∂Ω`, also ist `de` eine OBERE
// Schranke von `dist(x,∁Ω)` und `δ − de` eine gültige UNTERE Schranke des
// Zielfelds. Die Terme selbst liefern nur obere Schranken (`min`) — genau
// deshalb konnte eine WEITER entfernte Naht den `min` gewinnen und über eine
// nähere hinweg Material behaupten. Die Klemme hebt den Wert höchstens bis
// zur Wahrheit an; sie kann keinen Über-Claim erzeugen (CPU-Referenz:
// `offset_channel::stop_apply`).
pub const OP_STOP_LIMIT: u32     = 0x46; // +1

pub const OP_END: u32            = 0xFF;

// ── Compiled output ────────────────────────────────────────────────

/// Flat opcode buffer compiled from a single body's CSG tree.
#[derive(Debug, Clone)]
pub struct CompiledSdf {
    pub opcodes: Vec<f32>,
    pub aabb: Aabb,
    pub volumes: Vec<BakedVolume>,
    /// Conservative raymarch STEP factor (≤1) for this body — the renderer does
    /// `t += max(minD · march_relax, ε)`. Geometry-safe: it scales the STEP,
    /// never the field, so the zero-set / shell wall thickness stay exact
    /// (a field scale, as gyroid/helix bake in, would corrupt a downstream
    /// Shell's `−(d+t)`). The min over the body's features (Ticket 1wnm).
    pub march_relax: f32,
}

impl CompiledSdf {
    pub fn opcode_count(&self) -> usize {
        self.opcodes.len()
    }
}

// ── Compiler ───────────────────────────────────────────────────────

struct CompileCtx {
    opcodes: Vec<f32>,
    volumes: Vec<BakedVolume>,
}

const MESH_VOLUME_RESOLUTION: u32 = 64;

/// Compile a body's `BoundedCsg` tree into a flat opcode buffer.
pub fn compile_body(node: &BoundedCsg) -> CompiledSdf {
    let mut ctx = CompileCtx {
        opcodes: Vec::with_capacity(256),
        volumes: Vec::new(),
    };
    emit_node(node, &mut ctx, &crate::csg::OpenBranchSet::EMPTY);
    ctx.opcodes.push(f32::from_bits(OP_END));
    CompiledSdf {
        opcodes: ctx.opcodes,
        aabb: node.aabb(),
        volumes: ctx.volumes,
        march_relax: march_relax(node),
    }
}

/// Conservative raymarch STEP factor for a body's tree (see
/// [`CompiledSdf::march_relax`]): `1 / L` with `L = lipschitz_bound(node)`,
/// the compile-time Lipschitz net (#384, SYNTHESIS §6 Stufe 2). The factor
/// scales only the STEP, never the field — zero set and shell wall thickness
/// stay exact (Ticket 1wnm).
pub(crate) fn march_relax(node: &BoundedCsg) -> f32 {
    1.0 / lipschitz_bound(node)
}

/// Largest coordinate-radius `√(x² + z²)` a point of `aabb` can have —
/// the twist warp's lever arm (its Jacobian stretch grows with that radius).
fn max_radius_xz(aabb: &crate::csg::Aabb) -> f32 {
    if aabb.is_empty() {
        return 0.0;
    }
    let rx = aabb.max[0].abs().max(aabb.min[0].abs());
    let rz = aabb.max[2].abs().max(aabb.min[2].abs());
    (rx * rx + rz * rz).sqrt()
}

/// `√(x² + y²)`-analogue of [`max_radius_xz`] for the bend warp.
fn max_radius_xy(aabb: &crate::csg::Aabb) -> f32 {
    if aabb.is_empty() {
        return 0.0;
    }
    let rx = aabb.max[0].abs().max(aabb.min[0].abs());
    let ry = aabb.max[1].abs().max(aabb.min[1].abs());
    (rx * rx + ry * ry).sqrt()
}

/// Worst-case Lipschitz bound of the compiled LOFT render field (#385 P5).
///
/// At a fixed height `z` the cross-section is the tent-weighted lerp
/// `Σ_k w_k(z)·d_k` of the section 2D profile fields — a convex combination of
/// 1-Lipschitz fields (`∂f/∂a + ∂f/∂b = 1`, both ≥ 0), so it is 1-Lipschitz
/// **in (x, y)**. The extra slope is purely axial: over an interior bracket
/// `[z_k, z_{k+1}]` the field is `(1−t)·d_k + t·d_{k+1}` with
/// `t = (z − z_k)/Δz_k`, hence
///   `∂f/∂z = (d_{k+1} − d_k) / Δz_k`.
/// `|d_{k+1} − d_k|` is the lateral gap between the two profiles' level sets;
/// combined with the unit in-plane gradient the full 3D gradient norm is
/// `√(1² + (∂f/∂z)²) ≤ 1 + |∂f/∂z|`, so
///   `L_z = 1 + max_k( max_lateral_shift(k, k+1) / Δz_k )`.
/// The linear `1 + …` upper bound is used (matches the `Displace` arm's
/// additive convention and is conservative vs. the quadrature norm).
///
/// `max_lateral_shift` is estimated CONSERVATIVELY from the two sections' 2D
/// AABBs re-expressed in the shared loft-local frame (each section's `map`
/// applied forward, exactly as [`crate::csg::loft_local_aabb`] lifts them):
/// per axis the larger corner displacement `max(|Δmin|, |Δmax|)`, combined in
/// quadrature. This captures both a lateral TRANSLATION of the profile and a
/// change of its EXTENT (a small circle growing into a large rectangle over a
/// short height — the documented worst case that tunnels). It is exactly 0
/// when consecutive profiles are coincident (`Δmin = Δmax = 0` on both axes)
/// → `L_z = 1.0`, i.e. no damping.
///
/// Over-conservativity is acceptable by design: over-damping only slows the
/// trace, under-damping tunnels (grey wall, #382/#385 §2 — "lieber zu stark
/// dämpfen als tunneln"). Known honesty gap: two profiles that share an AABB
/// but differ in shape (plus-sign vs. filled square) report shift 0 here while
/// their interior fields can differ by up to ~half the extent — but that is a
/// boundary that does NOT slide laterally, so it does not move the surface a
/// ray hits, and is not the render-tunnelling case P5 targets. Flagged, not
/// fixed. FIELD-NEUTRAL: this only feeds `march_relax` (the renderer STEP
/// factor); the emitted tape / field value is byte-identical to before.
fn loft_lipschitz_bound(sections: &[crate::bvh::LoftBoundedSection]) -> f32 {
    // Loft-local 2D AABB of one section: its pruned sketch AABB with the rigid
    // `map` (a general in-plane rotation + XY-translation, unit scale for a
    // loft — for an anti-parallel section the rotation is a π-flip about an
    // in-plane axis, which still maps z=0 corners to z=0, so the dropped `r[2]`
    // stays zero) applied forward — the own → common direction, mirroring
    // `loft_local_aabb`.
    let local_bb = |s: &crate::bvh::LoftBoundedSection| -> Option<([f32; 2], [f32; 2])> {
        let bb2 = s.sketch.aabb();
        if bb2.is_empty() {
            return None;
        }
        let corners = [
            [bb2.min[0], bb2.min[1]],
            [bb2.max[0], bb2.min[1]],
            [bb2.min[0], bb2.max[1]],
            [bb2.max[0], bb2.max[1]],
        ];
        let mut mn = [f32::INFINITY; 2];
        let mut mx = [f32::NEG_INFINITY; 2];
        for c in corners {
            let scaled = [c[0] * s.map.scale[0], c[1] * s.map.scale[1], 0.0];
            let r = quat_rotate(s.map.rotation, scaled);
            let p = [r[0] + s.map.translate[0], r[1] + s.map.translate[1]];
            for a in 0..2 {
                mn[a] = mn[a].min(p[a]);
                mx[a] = mx[a].max(p[a]);
            }
        }
        Some((mn, mx))
    };

    let mut worst_rate = 0.0f32;
    for w in sections.windows(2) {
        let dz = (w[1].z - w[0].z).abs();
        // Ascending, distinct planes are guaranteed by the resolver (it rejects
        // two profiles on the same plane); guard the division anyway so a
        // degenerate node can never emit NaN/∞ into the render step.
        if dz <= 1e-6 {
            continue;
        }
        let (Some((amn, amx)), Some((bmn, bmx))) = (local_bb(&w[0]), local_bb(&w[1])) else {
            continue;
        };
        let mut sq = 0.0f32;
        for a in 0..2 {
            let d = (amn[a] - bmn[a]).abs().max((amx[a] - bmx[a]).abs());
            sq += d * d;
        }
        let shift = sq.sqrt();
        worst_rate = worst_rate.max(shift / dz);
    }
    1.0 + worst_rate
}

/// Compile-time upper bound on the Lipschitz constant of the COMPILED render
/// field of `node` (#384, SYNTHESIS §2 Lösung B / §6 Stufe 2). Replaces the
/// former per-feature guess constants (`REVOLVE_MARCH_RELAX = 0.7` etc.) with
/// derived formulas that compose along the tree: warps multiply, additive
/// displacements add, boolean branches take the winner. Every arm carries its
/// derivation — no magic numbers.
///
/// Semantics: sphere marching with step `f·(1/L)` is safe when the field
/// never overestimates the true distance by more than `L`. A field built from
/// exact leaf distances composed through the ops below satisfies
/// `f(p) ≤ L · d_true(p)` with the returned `L`.
///
/// Honest overconservativity (documented, measured in
/// `metricity-lab --bin stage2_relax_net`):
/// * The bound is ONE scalar per body — a Twist on one union branch damps
///   marching over the whole body, even where rays never approach the twisted
///   part (spatial co-localisation not implemented; SYNTHESIS §5 Exp 4 —
///   after Stufe 1, smin is neutral here, so the chain_16-style 3.5× blowup
///   cannot occur; the residual cost is bounded by the worst single feature).
/// * Nested Chamfer chains converge to `L∞ = 1/(√2−1) ≈ 2.414` even when the
///   chamfer zones are spatially disjoint (real L there ≈ √2).
pub(crate) fn lipschitz_bound(node: &BoundedCsg) -> f32 {
    match node {
        // ── 1-Lipschitz sources ────────────────────────────────────
        // Leaf primitives: exact local SDFs; anisotropic placement scale is
        // conservative by construction (localise divides per-axis, the result
        // is multiplied by min(scale) → per-axis gradient min(scale)/scale_i
        // ≤ 1, an UNDER-estimate).
        // Gyroid leaf: field `(|g| − t)·s·0.2887` with `g = sin·cos + …`;
        // each partial of g is `a·cos − b·sin` with a² + b² ≤ 2, so
        // ‖∇g‖ ≤ √6 and L ≤ 0.2887·√6 = 1/√2 < 1 (0.2887 = 1/(2√3)).
        // Extrude: exact analytic 2D profile distance (OP_PRIM2D_PATH carries
        // the analytic segments — sdf_path is the exact minimum) combined
        // with the cap distance via exact box-composition; × min-scale as
        // above.
        // Revolve: the meridian map `(x,y,z) → (√(x²+z²), y)` is 1-Lipschitz
        // (reverse triangle inequality on the xz-plane), and every profile
        // boundary point at r ≥ 0 IS a surface point of the solid with the
        // same (r, y) — so `d2d ∘ meridian` never overestimates the true 3D
        // distance, inside or outside. The former blanket 0.7
        // (`REVOLVE_MARCH_RELAX`, Ticket 1wnm: "spline profile field can be
        // non-Lipschitz") is obsolete: the tape's 2D path distance is the
        // exact analytic minimum, not a spline approximation. Verified by
        // trace measurement (stage2_relax_net: 0 penetrations at full steps).
        // Helix: `OP_HELIX_SCALE` bakes `min_scale · lipschitz` into the
        // FIELD at emission — already normalised.
        // Sweep: the RMF frame lookup is gated at build time (κ·r < 1
        // pre-flight gate); within the gate the mapping stays contractive on
        // the tube the profile occupies. Known honesty gap: near the gate
        // limit the stretch approaches 1/(1−κ·r) and is NOT reflected here —
        // same behaviour as before (factor 1.0), flagged in #384.
        BoundedCsg::Empty
        | BoundedCsg::Leaf { .. }
        | BoundedCsg::Extrude { .. }
        | BoundedCsg::Revolve { .. }
        | BoundedCsg::Helix { .. }
        | BoundedCsg::Sweep { .. } => 1.0,
        // #385 P5: the loft morph is NOT 1-Lipschitz in z — the axial gradient
        // `(d_{k+1} − d_k)/Δz_k` lifts the bound to
        // `L_z = 1 + max_k(max_lateral_shift / Δz_k)` (see `loft_lipschitz_bound`
        // for the derivation + conservative shift estimate). This feeds ONLY
        // `march_relax` (renderer STEP damping, same as Displace/Revolve); the
        // emitted FIELD is byte-identical, so the mesher/weave are untouched.
        // Coincident consecutive profiles → shift 0 → `L_z = 1` (no damping).
        BoundedCsg::Loft { sections, .. } => loft_lipschitz_bound(sections),

        // ── Hard booleans: winner branch ───────────────────────────
        // min/max are sup-norm contractions — the gradient is the winning
        // child's gradient (SYNTHESIS §1: no amplification over depth).
        BoundedCsg::Union { left, right, .. }
        | BoundedCsg::Difference { left, right, .. }
        | BoundedCsg::Intersection { left, right, .. } => {
            lipschitz_bound(left).max(lipschitz_bound(right))
        }

        // ── Blends ─────────────────────────────────────────────────
        // Fillet: the Circular-CD smin (`csg::smin_poly`, v0.16.2) is a
        // CONVEX combination of its inputs (∂f/∂a + ∂f/∂b = 1, both ≥ 0) →
        // L = max(L_a, L_b), stack-neutral (SYNTHESIS §5 Exp 1; the old
        // quadratic form grew in quadrature, √(1+m) per m nested smins).
        // Chamfer: `min(a, b, (a+b−r)/√2)` — the cut-plane term has gradient
        // `(∇a+∇b)/√2`, so L = max(L_a, L_b, (L_a+L_b)/√2); for unit inputs
        // that is √2 (the chamfer face between acute walls genuinely
        // overestimates: growth rate `2·cos(θ/2)/√2` exceeds 1 for θ < 90°).
        // Chamfer chains converge: L ← (L+1)/√2 has fixed point 1/(√2−1).
        BoundedCsg::SmoothUnion { left, right, mode, .. }
        | BoundedCsg::SmoothDifference { left, right, mode, .. } => {
            let ll = lipschitz_bound(left);
            let lr = lipschitz_bound(right);
            match mode {
                BlendMode::Fillet => ll.max(lr),
                BlendMode::Chamfer => {
                    ll.max(lr).max((ll + lr) * std::f32::consts::FRAC_1_SQRT_2)
                }
            }
        }
        BoundedCsg::MaskedBlend { left, right, blend_mode, .. } => {
            // #382/#384: `csg::masked_blend_combine` cross-fades hard → CD
            // blend with blended ≤ hard pointwise, so the FILLET field never
            // overestimates distance — neutral. The CHAMFER blend inside the
            // mask tube is the same cut-plane construction as above and
            // inherits its (L_a+L_b)/√2 term.
            let ll = lipschitz_bound(left);
            let lr = lipschitz_bound(right);
            match blend_mode {
                BlendMode::Fillet => ll.max(lr),
                BlendMode::Chamfer => {
                    ll.max(lr).max((ll + lr) * std::f32::consts::FRAC_1_SQRT_2)
                }
            }
        }
        // Morph `f = (1−t)·a + t·b` is a fixed convex combination:
        // L = (1−t)·L_a + t·L_b (≤ max of the two).
        BoundedCsg::Morph { left, right, t, .. } => {
            let tt = t.clamp(0.0, 1.0);
            (1.0 - tt) * lipschitz_bound(left) + tt * lipschitz_bound(right)
        }

        // ── Domain warps: composition multiplies ───────────────────
        // Twist `T(p) = (R(rate·y)·p_xz, y)`: the Jacobian is the xz-rotation
        // plus the column ∂T/∂y = (rate·R'·p_xz, 1) with ‖rate·R'·p_xz‖ =
        // |rate|·r. Operator norm ≤ 1 + |rate|·r (Galin et al. 2020; proof:
        // ‖Ju‖² ≤ (√(u_x²+u_z²) + |u_y|·|rate|·r)² + u_y² ≤ (1 + |rate|·r)²).
        // r is bounded by the largest xz-radius the twisted SURFACE can have:
        // the twist preserves each point's xz-radius, so the surface stays
        // within the inner child's AABB radius. L(f∘T) ≤ L_f · L_T.
        BoundedCsg::Twist { inner, rate, .. } => {
            let r = max_radius_xz(&inner.aabb());
            lipschitz_bound(inner) * (1.0 + rate.abs() * r)
        }
        // Bend `B(p) = (R(rate·x)·p_xy, z)` — same structure with the lever
        // arm in the xy-plane: L_B ≤ 1 + |rate|·max‖p_xy‖ over the inner
        // child's AABB.
        BoundedCsg::Bend { inner, rate, .. } => {
            let r = max_radius_xy(&inner.aabb());
            lipschitz_bound(inner) * (1.0 + rate.abs() * r)
        }

        // ── Additive displacement: constants add ───────────────────
        // `f − depth·sin(x·f)·sin(1.3·y·f)·sin(0.7·z·f)`: the displacement's
        // gradient is bounded per axis by |depth·freq|·{1, 1.3, 0.7}, so its
        // norm by |depth·freq|·√(1+1.3²+0.7²) ≈ 1.783·|depth·freq|, and
        // L_total ≤ L_inner + that term (gradients of sums add). The factor
        // is NOT baked into the field — field normalisation measurably harms
        // the Lichen mesher (813 → 1634 open edges, Befund 2026-07-13 in
        // bvh.rs); it is only reported to the renderer as step damping.
        BoundedCsg::Displace { inner, depth, freq, .. } => {
            let grad_norm = (1.0f32 + 1.3 * 1.3 + 0.7 * 0.7).sqrt();
            lipschitz_bound(inner) + (depth * freq).abs() * grad_norm
        }

        // ── L-neutral unaries ──────────────────────────────────────
        // Shell `max(d, −(d+t))` is a winner branch of two fields with the
        // child's L; Offset `d − δ` shifts the value, not the gradient (and
        // a level-set offset of an L-Lipschitz field never overestimates by
        // more than L: dist(p, {f=c}) ≥ (f(p)−c)/L).
        BoundedCsg::Shell { inner, .. }
        | BoundedCsg::InnerWall { inner, .. }
        | BoundedCsg::Offset { inner, .. } => lipschitz_bound(inner),

        // offset_tree Phase B: der Kanal-Term ist ein min/max-Gewinner aus
        // 1-Lipschitz-Quellen (Distanz-zur-Kurve minus Konstante, Ebenen-
        // Dots mit Einheitsvektoren, ±d + Konstante) — Winner-Branch wie
        // die harten Booleans, komponiert neutral (Plan §6).
        BoundedCsg::ArcChannel { inner, .. } => lipschitz_bound(inner).max(1.0),
    }
}

fn op(tag: u32) -> f32 {
    f32::from_bits(tag)
}

/// Stride EINES Rand-Fillet-Records im `OP_RIM_FILLET`-Block (offset_tree
/// Phase 2, #398): `a(3) b(3) n_a(3) n_b(3) n_c(3) delta(1) trust(1)`.
pub const RIM_STRIDE: usize = 17;

/// Hängt EINEN `OP_RIM_FILLET`-Block (Header + Records) an `out` an — analog
/// zur Kanal-Emission, aber NICHT im Regen-Pfad aufgerufen (Phase 2: nur
/// Tests). `concave` ist homogen je Offset-Stopp (Header). Der Decode-Spiegel
/// ist der `OP_RIM_FILLET`-Arm in [`eval_opcodes`].
pub fn encode_rim_block(concave: bool, recs: &[crate::offset_rim::RimRecord], out: &mut Vec<f32>) {
    out.push(op(OP_RIM_FILLET));
    out.push(recs.len() as f32);
    out.push(if concave { 1.0 } else { 0.0 });
    for r in recs {
        out.extend_from_slice(&r.a);
        out.extend_from_slice(&r.b);
        out.extend_from_slice(&r.n_a);
        out.extend_from_slice(&r.n_b);
        out.extend_from_slice(&r.n_c);
        out.push(r.delta);
        out.push(r.trust);
    }
}

/// Stride EINES gemischten Arc-Rand-Fillet-Records im `OP_RIM_FILLET_ARC`-Block:
/// `c(3) u(3) v(3) n_a(3) n_b(3) r(1) sweep(1) wall_sign(1) delta(1) trust(1)`.
pub const RIM_ARC_STRIDE: usize = 20;

/// Hängt EINEN `OP_RIM_FILLET_ARC`-Block (Header + gemischte Segment-/Bogen-
/// Records) an `out` an. Decode-Spiegel ist der `OP_RIM_FILLET_ARC`-Arm in
/// [`eval_opcodes`].
pub fn encode_rim_arc_block(
    concave: bool,
    recs: &[crate::offset_rim::RimArcRecord],
    out: &mut Vec<f32>,
) {
    out.push(op(OP_RIM_FILLET_ARC));
    out.push(recs.len() as f32);
    out.push(if concave { 1.0 } else { 0.0 });
    for r in recs {
        out.extend_from_slice(&r.c);
        out.extend_from_slice(&r.u);
        out.extend_from_slice(&r.v);
        out.extend_from_slice(&r.n_a);
        out.extend_from_slice(&r.n_b);
        out.push(r.r);
        out.push(r.sweep);
        out.push(r.wall_sign);
        out.push(r.delta);
        out.push(r.trust);
    }
}

fn blend_mode_f32(m: BlendMode) -> f32 {
    match m {
        BlendMode::Fillet => 0.0,
        BlendMode::Chamfer => 1.0,
    }
}

fn emit_node(node: &BoundedCsg, ctx: &mut CompileCtx, open: &crate::csg::OpenBranchSet<'_>) {
    let out = &mut ctx.opcodes;
    match node {
        BoundedCsg::Empty => {
            out.push(op(OP_PRIM_CONST));
            out.push(f32::MAX);
        }
        BoundedCsg::Leaf {
            id,
            primitive,
            world_transform,
            ..
        } => {
            if open.has_feature(id.as_u32_or_root().unwrap_or(0)) {
                emit_leaf_retained(id, primitive, world_transform, ctx, open);
            } else {
                emit_leaf_primitive(primitive, world_transform, ctx);
            }
        }
        BoundedCsg::Union { left, right, .. } => {
            emit_node(left, ctx, open);
            emit_node(right, ctx, open);
            ctx.opcodes.push(op(OP_UNION));
        }
        BoundedCsg::Difference { left, right, .. } => {
            emit_node(left, ctx, open);
            emit_node(right, ctx, open);
            ctx.opcodes.push(op(OP_DIFFERENCE));
        }
        BoundedCsg::Intersection { left, right, .. } => {
            emit_node(left, ctx, open);
            emit_node(right, ctx, open);
            ctx.opcodes.push(op(OP_INTERSECT));
        }
        BoundedCsg::SmoothUnion {
            left,
            right,
            radius,
            mode,
            ..
        } => {
            emit_node(left, ctx, open);
            emit_node(right, ctx, open);
            let out = &mut ctx.opcodes;
            out.push(op(OP_SMOOTH_UNION));
            out.push(*radius);
            out.push(blend_mode_f32(*mode));
        }
        BoundedCsg::SmoothDifference {
            left,
            right,
            radius,
            mode,
            ..
        } => {
            emit_node(left, ctx, open);
            emit_node(right, ctx, open);
            let out = &mut ctx.opcodes;
            out.push(op(OP_SMOOTH_DIFF));
            out.push(*radius);
            out.push(blend_mode_f32(*mode));
        }
        BoundedCsg::Twist { inner, rate, .. } => {
            out.push(op(OP_WARP_PUSH));
            out.push(op(OP_WARP_TWIST));
            out.push(*rate);
            emit_node(inner, ctx, open);
            ctx.opcodes.push(op(OP_WARP_POP));
        }
        BoundedCsg::Bend { inner, rate, .. } => {
            out.push(op(OP_WARP_PUSH));
            out.push(op(OP_WARP_BEND));
            out.push(*rate);
            emit_node(inner, ctx, open);
            ctx.opcodes.push(op(OP_WARP_POP));
        }
        BoundedCsg::Displace {
            inner, depth, freq, ..
        } => {
            emit_node(inner, ctx, open);
            let out = &mut ctx.opcodes;
            out.push(op(OP_DISPLACE_VAL));
            out.push(*depth);
            out.push(*freq);
        }
        BoundedCsg::Offset { inner, delta, .. } => {
            // `Offset` is `child − δ`. `OP_OFFSET_2D` is dimension-agnostic on the
            // distance stack (pops 1, pushes `d − δ`), so it serves the 3D node too.
            emit_node(inner, ctx, open);
            let out = &mut ctx.opcodes;
            out.push(op(OP_OFFSET_2D));
            out.push(*delta);
        }
        // #384 D2: InnerWall is a No-Op on the distance tape (it only rewrites
        // provenance, which the tape does not track) — emit the child unchanged.
        // This keeps the GPU-TSL and CGAL-C++ backends untouched.
        BoundedCsg::InnerWall { inner, .. } => emit_node(inner, ctx, open),
        BoundedCsg::ArcChannel { inner, channels, verts, rims, .. } => {
            // offset_tree Phase B: naiver Stopp zuerst, dann je Kanal EIN
            // OP_ARC_CHANNEL (pops d, pusht korrigiert) — sequenziell wie
            // `offset_channel::channels_apply`. Phase C4: danach je
            // Vertex-Kugel EIN OP_VERTEX_SPHERE. Phase 4 (#398):
            // zuletzt je Rim-Block EIN OP_RIM_FILLET (`apply_rims_f32`).
            // §8i: davor EIN payload-loses OP_STOP_BASE — es friert den naiven
            // Stopp-Wert d₀ im Basis-Register ein, gegen das alle Gates dieses
            // Stopps rechnen (CPU-Referenz `offset_channel::stop_apply`).
            emit_node(inner, ctx, open);
            let out = &mut ctx.opcodes;
            if !channels.is_empty() || !verts.is_empty() {
                out.push(op(OP_STOP_BASE));
            }
            for ch in channels {
                out.push(op(OP_ARC_CHANNEL));
                out.push(ch.arcs.len() as f32);
                out.push(if ch.concave { 1.0 } else { 0.0 });
                out.push(ch.delta);
                out.push(ch.trust);
                out.push(ch.gate);
                out.extend_from_slice(&ch.bound_c);
                out.push(ch.bound_r);
                // #384 §8f: die vier End-Halbräume (drittes Gate).
                for g in &ch.end_planes {
                    out.extend_from_slice(g);
                }
                // #384 §8j: die vier Fremdflächen-Halbräume (viertes Gate).
                for g in &ch.face_planes {
                    out.extend_from_slice(g);
                }
                for a in &ch.arcs {
                    out.extend_from_slice(&a.c);
                    out.extend_from_slice(&a.u);
                    out.extend_from_slice(&a.v);
                    out.push(a.r);
                    out.push(a.sweep);
                    out.extend_from_slice(&a.bc);
                    out.push(a.br);
                    out.extend_from_slice(&a.m1s);
                    out.extend_from_slice(&a.m2s);
                    out.extend_from_slice(&a.m1e);
                    out.extend_from_slice(&a.m2e);
                }
            }
            for vs in verts {
                out.push(op(OP_VERTEX_SPHERE));
                out.push(if vs.concave { 1.0 } else { 0.0 });
                out.push(vs.delta);
                out.push(vs.trust);
                out.push(vs.gate);
                out.extend_from_slice(&vs.v);
                out.extend_from_slice(&vs.gates[0]);
                out.extend_from_slice(&vs.gates[1]);
                out.extend_from_slice(&vs.gates[2]);
            }
            // #384 §8j: die Grenzwert-Klemme steht NACH allen Termen und VOR
            // den Rim-Fillets (Spiegel von `bvh::evaluate_open`).
            if !channels.is_empty() || !verts.is_empty() {
                out.push(op(OP_STOP_LIMIT));
                let concave = channels
                    .first()
                    .map(|c| c.concave)
                    .unwrap_or_else(|| verts[0].concave);
                out.push(if concave { 1.0 } else { 0.0 });
            }
            for block in rims {
                // GENAU eine der beiden Listen ist gefüllt: rein gerade Kontakte
                // → OP_RIM_FILLET (byte-identisch); Bogen-Präsenz → der gemischte
                // OP_RIM_FILLET_ARC (Segmente + Bögen in EINER Nearest-Feature-
                // Auswahl, FC Phase 4).
                if !block.recs.is_empty() {
                    encode_rim_block(block.concave, &block.recs, out);
                }
                if !block.arc_recs.is_empty() {
                    encode_rim_arc_block(block.concave, &block.arc_recs, out);
                }
            }
        }
        BoundedCsg::Shell {
            inner,
            thickness,
            open_faces,
            ..
        } => {
            // Retained-boundary shell: wall = max(d, −(d_ret + t)). The closed
            // case folds to the single-subtree OP_SHELL; an opened shell
            // compiles the inner subtree TWICE — the second copy under the
            // opened branches (its leaves emit retained-boundary fields) —
            // and combines with the existing stack ops:
            //   push d; push d_ret; OFFSET(−t) → d_ret + t; DIFFERENCE → wall.
            emit_node(inner, ctx, open);
            if open_faces.is_empty() {
                let out = &mut ctx.opcodes;
                out.push(op(OP_SHELL));
                out.push(*thickness);
            } else {
                let merged = crate::csg::OpenBranchSet::push(open, open_faces);
                emit_node(inner, ctx, &merged);
                let out = &mut ctx.opcodes;
                out.push(op(OP_OFFSET_2D));
                out.push(-*thickness);
                out.push(op(OP_DIFFERENCE));
            }
        }
        BoundedCsg::Morph {
            left, right, t, ..
        } => {
            emit_node(left, ctx, open);
            emit_node(right, ctx, open);
            let out = &mut ctx.opcodes;
            out.push(op(OP_MORPH));
            out.push(*t);
        }
        BoundedCsg::MaskedBlend {
            left,
            right,
            mask_polyline,
            mask_radius,
            blend_radius,
            blend_mode,
            is_difference,
            ..
        } => {
            emit_node(left, ctx, open);
            emit_node(right, ctx, open);
            let out = &mut ctx.opcodes;
            out.push(op(OP_MASKED_BLEND));
            // Count-prefixed polyline: n then n×[x,y,z]. A 1-point
            // polyline is a sphere, 2 points a single capsule, etc.
            out.push(mask_polyline.len() as f32);
            for pt in mask_polyline {
                out.extend_from_slice(pt);
            }
            out.push(*mask_radius);
            out.push(*blend_radius);
            out.push(blend_mode_f32(*blend_mode));
            out.push(if *is_difference { 1.0 } else { 0.0 });
        }
        BoundedCsg::Extrude {
            id,
            sketch,
            half_height,
            world_transform,
            wall_subs,
            ..
        } => {
            let excl = open
                .has_feature(id.as_u32_or_root().unwrap_or(0))
                .then_some((id, open, wall_subs.as_deref().map(|v| v.as_slice())));
            let mut cap_mask = 3.0_f32;
            if let Some((id, open, _)) = excl {
                if open.contains_sub(id, crate::csg::BranchId::EXTRUDE_CAP_TOP_SUB) {
                    cap_mask -= 1.0;
                }
                if open.contains_sub(id, crate::csg::BranchId::EXTRUDE_CAP_BOTTOM_SUB) {
                    cap_mask -= 2.0;
                }
            }
            out.push(op(OP_WARP_PUSH));
            emit_warp_transform(world_transform, out);
            emit_sketch(sketch, out, 0, excl);
            out.push(op(OP_EXTRUDE));
            out.push(*half_height);
            out.push(world_transform.min_scale());
            out.push(cap_mask);
            out.push(op(OP_WARP_POP));
        }
        BoundedCsg::Revolve {
            id,
            sketch,
            world_transform,
            wall_subs,
            ..
        } => {
            let excl = open
                .has_feature(id.as_u32_or_root().unwrap_or(0))
                .then_some((id, open, wall_subs.as_deref().map(|v| v.as_slice())));
            out.push(op(OP_WARP_PUSH));
            emit_warp_transform(world_transform, out);
            out.push(op(OP_WARP_REVOLVE));
            emit_sketch(sketch, out, 0, excl);
            out.push(op(OP_REVOLVE_SCALE));
            out.push(world_transform.min_scale());
            out.push(op(OP_WARP_POP));
        }
        BoundedCsg::Helix {
            id,
            sketch,
            pitch,
            turns,
            left_handed,
            taper,
            start_angle,
            y_anchor,
            r_inner,
            world_transform,
            wall_subs,
            ..
        } => {
            // Helical unwrap into the profile frame, then the shared 2D
            // sketch eval, then the per-point Lipschitz normalisation —
            // mirrors `crate::helix_profile_point` (and the web GPU
            // interpreter's OP_WARP_HELIX / OP_HELIX_SCALE arms).
            let excl = open
                .has_feature(id.as_u32_or_root().unwrap_or(0))
                .then_some((id, open, wall_subs.as_deref().map(|v| v.as_slice())));
            out.push(op(OP_WARP_PUSH));
            emit_warp_transform(world_transform, out);
            out.push(op(OP_WARP_HELIX));
            out.push(pitch.max(1e-3));
            out.push(if *left_handed { -1.0 } else { 1.0 });
            out.push(turns.max(0.0));
            out.push(*y_anchor);
            out.push(*taper);
            out.push(*r_inner);
            out.push(*start_angle);
            emit_sketch(sketch, out, 0, excl);
            // Lipschitz scale is a per-feature constant (see
            // `helix_profile_point`) — bake it into the multiplier.
            let tau = std::f32::consts::TAU;
            let p_eff = pitch.max(1e-3);
            let rr = r_inner.max(p_eff / tau * 0.25).max(1e-3);
            let q = p_eff / (tau * rr);
            let lip = 1.0 / (2.25 + q * q + taper * taper).sqrt();
            out.push(op(OP_HELIX_SCALE));
            out.push(world_transform.min_scale() * lip);
            out.push(op(OP_WARP_POP));
        }
        BoundedCsg::Sweep { id, sketch, path, world_transform, .. } => {
            // Nearest-spine-point unwrap into the profile's (u, v) frame, the
            // shared 2D sketch eval, then the IQ extrude cap-compose (reuses
            // OP_HELIX_SCALE — point.z carries the cap coordinate w). Mirrors
            // `crate::sweep_path::SweepPath::sdf`.
            // Sweeps keep positional wall subs (no table).
            let excl = open
                .has_feature(id.as_u32_or_root().unwrap_or(0))
                .then_some((id, open, None));
            out.push(op(OP_WARP_PUSH));
            emit_warp_transform(world_transform, out);
            out.push(op(OP_WARP_SWEEP));
            out.push(path.points.len() as f32);
            out.push(if path.closed { 1.0 } else { 0.0 });
            out.push(path.total_len);
            out.push(path.twist);
            for i in 0..path.points.len() {
                let p = path.points[i];
                let f = &path.frames[i];
                out.extend_from_slice(&[p[0], p[1], p[2]]);
                out.extend_from_slice(&[f.n[0], f.n[1], f.n[2]]);
                out.extend_from_slice(&[f.b[0], f.b[1], f.b[2]]);
                out.push(path.arclen[i]);
            }
            emit_sketch(sketch, out, 0, excl);
            out.push(op(OP_HELIX_SCALE));
            out.push(world_transform.min_scale());
            out.push(op(OP_WARP_POP));
        }
        BoundedCsg::Loft {
            sections,
            half_span,
            world_transform,
            ..
        } => {
            // Tape layout (see OP_LOFT_MORPH): bring the point into loft-local
            // coordinates, seed the accumulator with 0, then for each section
            // reframe into its own sketch plane (OP_WARP_TRANSFORM = the rigid
            // `map`), evaluate its 2D field, and fold it in with its tent weight
            // (OP_LOFT_MORPH). Finally the symmetric IQ z-clamp (OP_EXTRUDE,
            // cap_mask=3 — loft shell-opening is P3, so caps are always
            // retained) and the world scale. Every section is evaluated
            // unconditionally, matching the CPU `sdf_loft`.
            //
            // Loft shell-retained opening is not modelled in P2, so the wall
            // subs / open-branch set are ignored here (cap_mask stays 3).
            out.push(op(OP_WARP_PUSH));
            emit_warp_transform(world_transform, out);
            out.push(op(OP_PRIM_CONST));
            out.push(0.0);
            let n = sections.len();
            for (j, s) in sections.iter().enumerate() {
                let z_prev = if j == 0 { s.z } else { sections[j - 1].z };
                let z_next = if j + 1 == n { s.z } else { sections[j + 1].z };
                out.push(op(OP_WARP_PUSH));
                emit_warp_transform(&s.map, out);
                emit_sketch(&s.sketch, out, 0, None);
                out.push(op(OP_WARP_POP));
                out.push(op(OP_LOFT_MORPH));
                out.push(z_prev);
                out.push(s.z);
                out.push(z_next);
            }
            out.push(op(OP_EXTRUDE));
            out.push(*half_span);
            out.push(world_transform.min_scale());
            out.push(3.0); // both caps retained
            out.push(op(OP_WARP_POP));
        }
    }
}

// ── Transform helpers ──────────────────────────────────────────────

fn emit_warp_transform(t: &Transform, out: &mut Vec<f32>) {
    out.push(op(OP_WARP_TRANSFORM));
    out.extend_from_slice(&t.translate);
    out.extend_from_slice(&t.rotation);
    out.extend_from_slice(&t.scale);
}

fn emit_transform_with_center(t: &Transform, center: &[f32; 3], out: &mut Vec<f32>) {
    let center_scaled = [
        center[0] * t.scale[0],
        center[1] * t.scale[1],
        center[2] * t.scale[2],
    ];
    let center_world = quat_rotate(t.rotation, center_scaled);
    out.push(t.translate[0] + center_world[0]);
    out.push(t.translate[1] + center_world[1]);
    out.push(t.translate[2] + center_world[2]);
    out.extend_from_slice(&t.rotation);
    out.extend_from_slice(&t.scale);
}

fn emit_transform(t: &Transform, out: &mut Vec<f32>) {
    out.extend_from_slice(&t.translate);
    out.extend_from_slice(&t.rotation);
    out.extend_from_slice(&t.scale);
}

// ── Shell open-face surface kinds (mirror in `web/src/sdfRenderer.ts`'s
//    `fd_open_gate`). Each open face is a fixed 15-float record so both VMs can
//    pre-skip the operand block and loop at a constant stride:
//    `kind(1) + transform(10) + params(4, zero-padded)`. ──
pub(crate) const OPEN_KIND_PLANE: f32 = 0.0;
pub(crate) const OPEN_KIND_CYLINDER: f32 = 1.0;
pub(crate) const OPEN_KIND_SPHERE: f32 = 2.0;
pub(crate) const OPEN_KIND_CONE: f32 = 3.0;
pub(crate) const OPEN_KIND_TORUS: f32 = 4.0;
/// A non-analytic surface (TPMS / profile) — never emitted as a retained face.
pub(crate) const OPEN_KIND_NEVER: f32 = 5.0;

/// Retained-boundary leaf: `max` over the primitive's half-space faces with
/// the opened ones removed (the GPU mirror of `csg::leaf_sdf_retained`). Each
/// retained face is one `OP_PRIM_HALFSPACE` record folded with
/// `OP_INTERSECT`; an all-faces-open leaf degenerates to `−∞` (no retained
/// boundary → everything is cavity).
fn emit_leaf_retained(
    id: &fielddraft_protocol::FeatureId,
    prim: &Primitive,
    world_transform: &Transform,
    ctx: &mut CompileCtx,
    open: &crate::csg::OpenBranchSet<'_>,
) {
    let Some(hs) = crate::csg::primitive_halfspaces(prim) else {
        // No half-space split (Sphere-as-mesh etc. never reach here; TPMS /
        // imported meshes have nothing to retain) — exclusion is ignored.
        emit_leaf_primitive(prim, world_transform, ctx);
        return;
    };
    let retained: Vec<_> =
        hs.faces().iter().filter(|f| !open.contains_sub(id, f.sub)).collect();
    if retained.len() == hs.faces().len() {
        emit_leaf_primitive(prim, world_transform, ctx);
        return;
    }
    let out = &mut ctx.opcodes;
    if retained.is_empty() {
        out.push(op(OP_PRIM_CONST));
        out.push(f32::NEG_INFINITY);
        return;
    }
    for (i, f) in retained.iter().enumerate() {
        // Face frame: world ∘ translate(center + local_offset), the same DL-20
        // fold the weaver uses.
        let translate = [
            hs.center[0] + f.local_offset[0],
            hs.center[1] + f.local_offset[1],
            hs.center[2] + f.local_offset[2],
        ];
        let face_xf = crate::compose_transforms(
            world_transform,
            &Transform { translate, ..Transform::IDENTITY },
        );
        emit_halfspace_record(&f.half_space, &face_xf, out);
        if i > 0 {
            out.push(op(OP_INTERSECT));
        }
    }
}

/// One `OP_PRIM_HALFSPACE` record: kind + transform(10) + params(4).
fn emit_halfspace_record(surface: &HalfSpace, placement: &Transform, out: &mut Vec<f32>) {
    let kind = match surface {
        HalfSpace::Plane { .. } => OPEN_KIND_PLANE,
        HalfSpace::InfiniteCylinder { .. } => OPEN_KIND_CYLINDER,
        HalfSpace::Sphere { .. } => OPEN_KIND_SPHERE,
        HalfSpace::Cone { .. } => OPEN_KIND_CONE,
        HalfSpace::Torus { .. } => OPEN_KIND_TORUS,
        HalfSpace::Gyroid { .. } => OPEN_KIND_NEVER,
        HalfSpace::RevolvedProfile { .. } => OPEN_KIND_NEVER,
        HalfSpace::Blend { .. } => OPEN_KIND_NEVER,
    };
    out.push(op(OP_PRIM_HALFSPACE));
    out.push(kind);
    out.extend_from_slice(&placement.translate);
    out.extend_from_slice(&placement.rotation);
    out.extend_from_slice(&placement.scale);
    // Four param slots, zero-padded; layout matches the WGSL mirror.
    let params = match surface {
        HalfSpace::Plane { normal, offset } => [normal[0], normal[1], normal[2], *offset],
        HalfSpace::InfiniteCylinder { radius } | HalfSpace::Sphere { radius } => {
            [*radius, 0.0, 0.0, 0.0]
        }
        HalfSpace::Cone { half_angle } => [*half_angle, 0.0, 0.0, 0.0],
        HalfSpace::Torus { major_radius, minor_radius } => {
            [*major_radius, *minor_radius, 0.0, 0.0]
        }
        HalfSpace::Gyroid { .. } | HalfSpace::RevolvedProfile { .. } | HalfSpace::Blend { .. } => {
            [0.0; 4]
        }
    };
    out.extend_from_slice(&params);
}

/// Raw local SDF of one analytic half-space face, mirroring
/// `csg::sdf_halfspace` and the WGSL `fd_halfspace_local`. The caller scales
/// by the placement's min-scale, the same convention as every other leaf
/// primitive opcode.
fn halfspace_local(local: [f32; 3], kind: f32, p: [f32; 4]) -> f32 {
    if kind < 0.5 {
        // Plane: normal = (p0, p1, p2), offset = p3.
        local[0] * p[0] + local[1] * p[1] + local[2] * p[2] - p[3]
    } else if kind < 1.5 {
        // Infinite cylinder, radius p0, axis Y.
        (local[0] * local[0] + local[2] * local[2]).sqrt() - p[0]
    } else if kind < 2.5 {
        // Sphere, radius p0.
        (local[0] * local[0] + local[1] * local[1] + local[2] * local[2]).sqrt() - p[0]
    } else if kind < 3.5 {
        // Cone, half-angle p0, apex at origin, axis Y.
        let rho = (local[0] * local[0] + local[2] * local[2]).sqrt();
        let (sin_a, cos_a) = p[0].sin_cos();
        rho * cos_a - local[1].abs() * sin_a
    } else if kind < 4.5 {
        // Torus, major p0, minor p1.
        let rho = (local[0] * local[0] + local[2] * local[2]).sqrt();
        let q0 = rho - p[0];
        (q0 * q0 + local[1] * local[1]).sqrt() - p[1]
    } else {
        // Non-analytic (TPMS / profile) — never emitted as retained faces.
        f32::INFINITY
    }
}

// ── Leaf primitives ────────────────────────────────────────────────

fn emit_leaf_primitive(prim: &Primitive, t: &Transform, ctx: &mut CompileCtx) {
    let out = &mut ctx.opcodes;
    match prim {
        Primitive::Box {
            center,
            half_extents,
        } => {
            out.push(op(OP_PRIM_BOX));
            emit_transform_with_center(t, center, out);
            out.extend_from_slice(half_extents);
        }
        Primitive::Sphere { center, radius } => {
            out.push(op(OP_PRIM_SPHERE));
            emit_transform_with_center(t, center, out);
            out.push(*radius);
        }
        Primitive::Cylinder {
            center,
            radius,
            half_height,
        } => {
            out.push(op(OP_PRIM_CYLINDER));
            emit_transform_with_center(t, center, out);
            out.push(*radius);
            out.push(*half_height);
        }
        Primitive::Torus {
            center,
            major_radius,
            minor_radius,
        } => {
            out.push(op(OP_PRIM_TORUS));
            emit_transform_with_center(t, center, out);
            out.push(*major_radius);
            out.push(*minor_radius);
        }
        Primitive::Cone {
            r_bottom,
            r_top,
            half_height,
        } => {
            out.push(op(OP_PRIM_CONE));
            emit_transform(t, out);
            out.push(*r_bottom);
            out.push(*r_top);
            out.push(*half_height);
        }
        Primitive::Capsule {
            radius,
            half_height,
        } => {
            out.push(op(OP_PRIM_CAPSULE));
            emit_transform(t, out);
            out.push(*radius);
            out.push(*half_height);
        }
        Primitive::HexPrism {
            apothem,
            half_height,
        } => {
            out.push(op(OP_PRIM_HEX_PRISM));
            emit_transform(t, out);
            out.push(*apothem);
            out.push(*half_height);
        }
        Primitive::Ellipsoid { radii } => {
            out.push(op(OP_PRIM_ELLIPSOID));
            emit_transform(t, out);
            out.extend_from_slice(radii);
        }
        Primitive::Gyroid {
            cell_size,
            thickness,
        } => {
            out.push(op(OP_PRIM_GYROID));
            emit_transform(t, out);
            out.push(*cell_size);
            out.push(*thickness);
        }
        Primitive::Thread {
            nominal_radius,
            pitch,
            depth,
            half_height,
        } => {
            out.push(op(OP_PRIM_THREAD));
            emit_transform(t, out);
            out.push(*nominal_radius);
            out.push(*pitch);
            out.push(*depth);
            out.push(*half_height);
        }
        // #385 P2: Loft (like Extrude/Revolve/Helix/Sweep) is emitted from its
        // own `BoundedCsg::Loft` arm in `emit_node` (OP_LOFT_MORPH + OP_EXTRUDE),
        // never as a leaf primitive — `from_parts` lowers `Primitive::Loft` to a
        // `CsgNode::Loft`, so this leaf fallback is unreachable for it and just
        // keeps the emitter total well-defined.
        Primitive::Extrude { .. }
        | Primitive::Revolve { .. }
        | Primitive::Helix { .. }
        | Primitive::Loft { .. } => {
            out.push(op(OP_PRIM_CONST));
            out.push(0.0);
        }
        Primitive::ImportedMesh { mesh } => {
            let tex_idx = ctx.volumes.len() as f32;
            let volume = mesh.bake_volume(MESH_VOLUME_RESOLUTION);
            out.push(op(OP_PRIM_TEX3D));
            out.push(tex_idx);
            emit_transform(t, out);
            out.extend_from_slice(&volume.aabb_min);
            out.extend_from_slice(&volume.aabb_max);
            ctx.volumes.push(volume);
        }
        Primitive::CustomSdf { ops, lipschitz, half_extents, .. } => {
            out.push(op(OP_PRIM_CUSTOM));
            emit_transform(t, out);
            out.push(1.0 / lipschitz.max(f32::MIN_POSITIVE));
            out.extend_from_slice(half_extents);
            out.push(ops.len() as f32);
            for o in ops {
                let (code, payload) = expr_op_record(o);
                out.push(code);
                out.push(payload);
            }
        }
    }
}

/// Fixed-width (code, payload) record of one [`SdfExprOp`] in the GPU
/// buffer. Codes are plain small floats (not bit-cast tags) — the inner
/// expression interpreter dispatches on `int(code)`. Mirrored by
/// `web/src/sdfVmInterpreter.ts` and [`eval_expr_record`].
pub(crate) fn expr_op_record(op: &fielddraft_protocol::SdfExprOp) -> (f32, f32) {
    use fielddraft_protocol::SdfExprOp as E;
    match op {
        E::X => (0.0, 0.0),
        E::Y => (1.0, 0.0),
        E::Z => (2.0, 0.0),
        E::Const { v } => (3.0, *v),
        E::Add => (4.0, 0.0),
        E::Sub => (5.0, 0.0),
        E::Mul => (6.0, 0.0),
        E::Div => (7.0, 0.0),
        E::Neg => (8.0, 0.0),
        E::Min => (9.0, 0.0),
        E::Max => (10.0, 0.0),
        E::Abs => (11.0, 0.0),
        E::Sqrt => (12.0, 0.0),
        E::Sin => (13.0, 0.0),
        E::Cos => (14.0, 0.0),
        E::Floor => (15.0, 0.0),
        E::Exp => (16.0, 0.0),
        E::Ln => (17.0, 0.0),
        E::Pow => (18.0, 0.0),
        E::Atan2 => (19.0, 0.0),
        E::Asin => (20.0, 0.0),
        E::Acos => (21.0, 0.0),
    }
}

/// CPU mirror of the GPU inner expression interpreter: evaluate one
/// (code, payload) record stream at the LOCAL point. Must match the
/// TSL dispatch in `sdfVmInterpreter.ts` op for op.
fn eval_expr_records(records: &[f32], local: [f32; 3]) -> f32 {
    const MAX_STACK: usize = fielddraft_protocol::SDF_EXPR_MAX_STACK;
    let mut stack = [0.0f32; MAX_STACK];
    let mut sp = 0usize;
    let mut i = 0usize;
    while i + 1 < records.len() {
        let code = records[i] as i32;
        let payload = records[i + 1];
        i += 2;
        // Leaf pushes.
        let pushed = match code {
            0 => Some(local[0]),
            1 => Some(local[1]),
            2 => Some(local[2]),
            3 => Some(payload),
            _ => None,
        };
        if let Some(v) = pushed {
            if sp >= MAX_STACK {
                return 1.0e6;
            }
            stack[sp] = v;
            sp += 1;
            continue;
        }
        // Unary ops.
        if matches!(code, 8 | 11 | 12 | 13 | 14 | 15 | 16 | 17 | 20 | 21) {
            if sp == 0 {
                return 1.0e6;
            }
            let a = stack[sp - 1];
            stack[sp - 1] = match code {
                8 => -a,
                11 => a.abs(),
                12 => a.max(0.0).sqrt(),
                13 => a.sin(),
                14 => a.cos(),
                15 => a.floor(),
                16 => a.exp(),
                17 => {
                    if a > 0.0 {
                        a.ln()
                    } else {
                        -1.0e6
                    }
                }
                20 => a.clamp(-1.0, 1.0).asin(),
                21 => a.clamp(-1.0, 1.0).acos(),
                _ => unreachable!(),
            };
            continue;
        }
        // Binary ops.
        if sp < 2 {
            return 1.0e6;
        }
        let b = stack[sp - 1];
        let a = stack[sp - 2];
        sp -= 1;
        stack[sp - 1] = match code {
            4 => a + b,
            5 => a - b,
            6 => a * b,
            7 => a / b,
            9 => a.min(b),
            10 => a.max(b),
            18 => {
                if a > 0.0 {
                    (a.ln() * b).exp()
                } else {
                    0.0
                }
            }
            19 => a.atan2(b),
            _ => return 1.0e6,
        };
    }
    if sp == 1 { stack[0] } else { 1.0e6 }
}

// ── 2D sketch inlining ────────────────────────────────────────────

/// `excl` carries the owning sweep's feature id + opened branch set; `base` is
/// this subtree's first global boundary-group id (right-boolean children start
/// past the left child's `edge_count`), matching the kernel's retained walk.
fn emit_sketch(
    sketch: &BoundedSketch2d,
    out: &mut Vec<f32>,
    base: u32,
    excl: Option<(
        &fielddraft_protocol::FeatureId,
        &crate::csg::OpenBranchSet<'_>,
        Option<&[u32]>,
    )>,
) {
    match sketch {
        BoundedSketch2d::Empty => {
            out.push(op(OP_PRIM_CONST));
            out.push(f32::MAX);
        }
        BoundedSketch2d::Prim { primitive, .. } => {
            emit_sketch_primitive(primitive, out, base, excl);
        }
        BoundedSketch2d::Union { left, right, .. } => {
            emit_sketch(left, out, base, excl);
            emit_sketch(right, out, base + left.edge_count(), excl);
            out.push(op(OP_UNION));
        }
        BoundedSketch2d::Difference { left, right, .. } => {
            emit_sketch(left, out, base, excl);
            emit_sketch(right, out, base + left.edge_count(), excl);
            out.push(op(OP_DIFFERENCE));
        }
        BoundedSketch2d::Intersection { left, right, .. } => {
            emit_sketch(left, out, base, excl);
            emit_sketch(right, out, base + left.edge_count(), excl);
            out.push(op(OP_INTERSECT));
        }
        BoundedSketch2d::Offset {
            inner, distance, ..
        } => {
            emit_sketch(inner, out, base, excl);
            out.push(op(OP_OFFSET_2D));
            out.push(*distance);
        }
    }
}

fn emit_sketch_primitive(
    prim: &Sketch2dPrimitive,
    out: &mut Vec<f32>,
    base: u32,
    excl: Option<(
        &fielddraft_protocol::FeatureId,
        &crate::csg::OpenBranchSet<'_>,
        Option<&[u32]>,
    )>,
) {
    match prim {
        Sketch2dPrimitive::Circle { center, radius } => {
            out.push(op(OP_PRIM2D_CIRCLE));
            out.extend_from_slice(center);
            out.push(*radius);
        }
        Sketch2dPrimitive::Rectangle {
            center,
            half_extents,
        } => {
            out.push(op(OP_PRIM2D_RECT));
            out.extend_from_slice(center);
            out.extend_from_slice(half_extents);
        }
        Sketch2dPrimitive::Polygon { vertices } => {
            out.push(op(OP_PRIM2D_POLY));
            out.push(vertices.len() as f32);
            for v in vertices {
                out.extend_from_slice(v);
            }
        }
        Sketch2dPrimitive::Sector {
            center,
            radius,
            start_angle,
            sweep_angle,
        } => {
            out.push(op(OP_PRIM2D_SECTOR));
            out.extend_from_slice(center);
            out.push(*radius);
            out.push(*start_angle);
            out.push(*sweep_angle);
        }
        Sketch2dPrimitive::Path { segments, closed } => {
            // Retained-boundary exclusion (opened shell faces): the field is
            // the NEGATED unsigned distance to the retained groups — an OPEN
            // path record (unsigned) + NEG. Continuous through the opening;
            // see `Sketch2dPrimitive::sdf_retained`.
            if let Some((id, open, ws)) = excl {
                let groups = fielddraft_protocol::sketch2d::g1_group_segments(
                    segments,
                    fielddraft_protocol::sketch2d::PATH_G1_COS_THRESHOLD,
                );
                // The opened-face set carries STABLE subs; map the
                // positional group before asking it (wall_subs table).
                let sub_of = |g: u32| crate::csg::map_wall_sub(ws, base + g);
                if groups.iter().any(|g| open.contains_sub(id, sub_of(*g))) {
                    let retained: Vec<SketchSegment> = segments
                        .iter()
                        .zip(&groups)
                        .filter(|(_, g)| !open.contains_sub(id, sub_of(**g)))
                        .map(|(s, _)| s.clone())
                        .collect();
                    emit_path_record(&retained, false, out);
                    out.push(op(OP_NEG));
                    return;
                }
            }
            emit_path_record(segments, *closed, out);
        }
    }
}

/// One `OP_PRIM2D_PATH` record. The stream carries the analytic segments
/// themselves — exact distance and exact even-odd parity on the GPU. (The
/// former polyline flattening closed the contour with a synthetic edge; for
/// an axis-stripped revolve profile that edge lay ON the axis and Shell
/// inflated it into a phantom 2 mm rod.) Records are pre-split y-monotone so
/// the shader's crossing test is the plain half-open endpoint rule with one
/// closed-form root.
fn emit_path_record(segments: &[SketchSegment], closed: bool, out: &mut Vec<f32>) {
    // #392: fuse consecutive same-circle arcs first, so the fragile interior
    // seam (two arc endpoints of one circle re-derived from different angles,
    // rounding to opposite sides of a symmetry line) never reaches the GPU/CGAL
    // even-odd test. This is the SAME normalisation the CPU `path_is_inside`
    // applies, so every evaluator classifies the same contour. A no-op unless a
    // co-circular run is present, so ordinary profiles are byte-for-byte
    // unchanged.
    let merged = fielddraft_protocol::sketch2d::merge_cocircular_runs(segments);
    let segments: &[SketchSegment] = merged.as_deref().unwrap_or(segments);
    let records: Vec<SketchSegment> = segments
        .iter()
        .flat_map(fielddraft_protocol::sketch2d::split_y_monotone)
        .collect();
    out.push(op(OP_PRIM2D_PATH));
    out.push(records.len() as f32);
    out.push(if closed { 1.0 } else { 0.0 });
    for seg in &records {
        match seg {
            SketchSegment::Line { start, end } => {
                out.push(0.0);
                out.extend_from_slice(start);
                out.extend_from_slice(end);
                out.extend_from_slice(&[0.0, 0.0]);
            }
            SketchSegment::QuadBezier { p0, p1, p2 } => {
                out.push(1.0);
                out.extend_from_slice(p0);
                out.extend_from_slice(p1);
                out.extend_from_slice(p2);
            }
            SketchSegment::Arc {
                center,
                radius,
                start_angle,
                sweep_angle,
            } => {
                out.push(2.0);
                out.extend_from_slice(center);
                out.push(*radius);
                out.push(*start_angle);
                out.push(*sweep_angle);
                out.push(0.0);
            }
        }
    }
}

// ── CPU interpreter (test mirror of the GPU shader) ────────────────

const VSTACK_SIZE: usize = 8;
const PSTACK_SIZE: usize = 4;

/// Evaluate the compiled opcode buffer at a world-space point.
/// This is a 1:1 mirror of the WGSL stack machine and exists
/// solely so that every opcode path can be tested without a GPU.
pub fn eval_opcodes(opcodes: &[f32], world_point: [f32; 3]) -> f32 {
    eval_opcodes_with_volumes(opcodes, world_point, &[])
}

pub fn eval_opcodes_with_volumes(
    opcodes: &[f32],
    world_point: [f32; 3],
    volumes: &[BakedVolume],
) -> f32 {
    let mut vs = [0.0f32; VSTACK_SIZE];
    let mut vsp: usize = 0;
    let mut ps = [[0.0f32; 3]; PSTACK_SIZE];
    let mut psp: usize = 0;
    let mut pt = world_point;
    let mut pc: usize = 0;
    // #384 §8i: Basis-Register eines Offset-Stopps — der naive Stopp-Wert d₀,
    // gesetzt von `OP_STOP_BASE`, gelesen von OP_ARC_CHANNEL/OP_VERTEX_SPHERE.
    // Ein wohlgeformtes Tape schreibt es immer, bevor ein Term es liest.
    let mut base: f32 = 0.0;
    // #384 §8j: Grenzwert-Register eines Offset-Stopps — `max_j(δ − de_j)`
    // in konkaver Orientierung; `OP_STOP_BASE` setzt es zurück,
    // `OP_STOP_LIMIT` wendet es an (Spiegel: `offset_channel::stop_apply`).
    let mut lim: f32 = -crate::offset_channel::END_PLANE_OFF;

    macro_rules! push_v {
        ($v:expr) => {{
            vs[vsp] = $v;
            vsp += 1;
        }};
    }
    macro_rules! pop_v {
        () => {{
            vsp -= 1;
            vs[vsp]
        }};
    }
    macro_rules! rd {
        () => {{
            let v = opcodes[pc];
            pc += 1;
            v
        }};
    }
    macro_rules! rd3 {
        () => {{
            let a = opcodes[pc];
            let b = opcodes[pc + 1];
            let c = opcodes[pc + 2];
            pc += 3;
            [a, b, c]
        }};
    }

    while pc < opcodes.len() {
        let tag = opcodes[pc].to_bits();
        pc += 1;

        match tag {
            // ── 3D primitives ──────────────────────────────────
            OP_PRIM_BOX => {
                let (local, scale) = read_transform_and_localise(opcodes, &mut pc, pt);
                let he = rd3!();
                push_v!(sdf_box_local(local, he) * scale);
            }
            OP_PRIM_SPHERE => {
                let (local, scale) = read_transform_and_localise(opcodes, &mut pc, pt);
                let r = rd!();
                push_v!(sdf_sphere_local(local, r) * scale);
            }
            OP_PRIM_CYLINDER => {
                let (local, scale) = read_transform_and_localise(opcodes, &mut pc, pt);
                let r = rd!();
                let hh = rd!();
                push_v!(sdf_cylinder_local(local, r, hh) * scale);
            }
            OP_PRIM_TORUS => {
                let (local, scale) = read_transform_and_localise(opcodes, &mut pc, pt);
                let major = rd!();
                let minor = rd!();
                push_v!(sdf_torus_local(local, major, minor) * scale);
            }
            OP_PRIM_CONE => {
                let (local, scale) = read_transform_and_localise(opcodes, &mut pc, pt);
                let rb = rd!();
                let rt = rd!();
                let hh = rd!();
                push_v!(sdf_cone_local(local, rb, rt, hh) * scale);
            }
            OP_PRIM_CAPSULE => {
                let (local, scale) = read_transform_and_localise(opcodes, &mut pc, pt);
                let r = rd!();
                let hh = rd!();
                push_v!(sdf_capsule_local(local, r, hh) * scale);
            }
            OP_PRIM_HEX_PRISM => {
                let (local, scale) = read_transform_and_localise(opcodes, &mut pc, pt);
                let ap = rd!();
                let hh = rd!();
                push_v!(sdf_hex_prism_local(local, ap, hh) * scale);
            }
            OP_PRIM_ELLIPSOID => {
                let (local, scale) = read_transform_and_localise(opcodes, &mut pc, pt);
                let radii = rd3!();
                push_v!(sdf_ellipsoid_local(local, radii) * scale);
            }
            OP_PRIM_CONST => {
                let v = rd!();
                push_v!(v);
            }
            OP_PRIM_GYROID => {
                let (local, scale) = read_transform_and_localise(opcodes, &mut pc, pt);
                let cs = rd!();
                let th = rd!();
                push_v!(sdf_gyroid_local(local, cs, th) * scale);
            }
            OP_PRIM_THREAD => {
                let (local, scale) = read_transform_and_localise(opcodes, &mut pc, pt);
                let nr = rd!();
                let pitch = rd!();
                let depth = rd!();
                let hh = rd!();
                push_v!(sdf_thread_local(local, nr, pitch, depth, hh) * scale);
            }
            OP_PRIM_HALFSPACE => {
                let kind = rd!();
                let (local, scale) = read_transform_and_localise(opcodes, &mut pc, pt);
                let params = [rd!(), rd!(), rd!(), rd!()];
                push_v!(halfspace_local(local, kind, params) * scale);
            }
            OP_PRIM_CUSTOM => {
                let (local, scale) = read_transform_and_localise(opcodes, &mut pc, pt);
                let inv_l = rd!();
                let he = rd3!();
                let n = rd!() as usize;
                let records = &opcodes[pc..pc + 2 * n];
                pc += 2 * n;
                let expr = eval_expr_records(records, local) * inv_l;
                let d = expr.max(sdf_box_local(local, he));
                push_v!(d * scale);
            }

            // ── 2D primitives ──────────────────────────────────
            OP_PRIM2D_CIRCLE => {
                let cx = rd!();
                let cy = rd!();
                let r = rd!();
                let dx = pt[0] - cx;
                let dy = pt[1] - cy;
                push_v!((dx * dx + dy * dy).sqrt() - r);
            }
            OP_PRIM2D_RECT => {
                let cx = rd!();
                let cy = rd!();
                let hx = rd!();
                let hy = rd!();
                let qx = (pt[0] - cx).abs() - hx;
                let qy = (pt[1] - cy).abs() - hy;
                let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
                let inside = qx.max(qy).min(0.0);
                push_v!(outside + inside);
            }
            OP_PRIM2D_POLY => {
                let n = rd!() as usize;
                let mut verts = Vec::with_capacity(n);
                for _ in 0..n {
                    let vx = rd!();
                    let vy = rd!();
                    verts.push([vx, vy]);
                }
                push_v!(sdf_polygon_2d([pt[0], pt[1]], &verts));
            }
            OP_PRIM2D_SECTOR => {
                let cx = rd!();
                let cy = rd!();
                let r = rd!();
                let start = rd!();
                let sweep = rd!();
                push_v!(sdf_sector_2d([pt[0], pt[1]], [cx, cy], r, start, sweep));
            }
            OP_PRIM2D_PATH => {
                let n = rd!() as usize;
                let closed = rd!() != 0.0;
                let mut segs = Vec::with_capacity(n);
                for _ in 0..n {
                    let kind = rd!();
                    let a = rd!();
                    let b = rd!();
                    let c = rd!();
                    let d = rd!();
                    let e = rd!();
                    let f = rd!();
                    segs.push(match kind as u32 {
                        0 => SketchSegment::Line { start: [a, b], end: [c, d] },
                        1 => SketchSegment::QuadBezier { p0: [a, b], p1: [c, d], p2: [e, f] },
                        _ => SketchSegment::Arc {
                            center: [a, b],
                            radius: c,
                            start_angle: d,
                            sweep_angle: e,
                        },
                    });
                }
                push_v!(fielddraft_protocol::sketch2d::sdf_path(
                    [pt[0], pt[1]],
                    &segs,
                    closed,
                ));
            }

            // ── Binary operators ───────────────────────────────
            OP_UNION => {
                let b = pop_v!();
                let a = pop_v!();
                push_v!(a.min(b));
            }
            OP_DIFFERENCE => {
                let b = pop_v!();
                let a = pop_v!();
                push_v!(a.max(-b));
            }
            OP_INTERSECT => {
                let b = pop_v!();
                let a = pop_v!();
                push_v!(a.max(b));
            }
            OP_SMOOTH_UNION => {
                let r = rd!();
                let mode = rd!();
                let b = pop_v!();
                let a = pop_v!();
                push_v!(blend_min_eval(a, b, r, mode));
            }
            OP_SMOOTH_DIFF => {
                let r = rd!();
                let mode = rd!();
                let b = pop_v!();
                let a = pop_v!();
                push_v!(-blend_min_eval(-a, b, r, mode));
            }

            // ── Unary / compositor operators ───────────────────
            OP_SHELL => {
                // Closed inward shell. Opened shells compile to two subtrees
                // + OFFSET(−t) + DIFFERENCE instead (retained-boundary
                // erosion), so this op carries only the thickness.
                let thickness = rd!();
                let d = pop_v!();
                push_v!(d.max(-(d + thickness)));
            }
            OP_MORPH => {
                let t = rd!();
                let b = pop_v!();
                let a = pop_v!();
                push_v!((1.0 - t) * a + t * b);
            }
            OP_LOFT_MORPH => {
                // #385 P2: pop this section's 2D field `d_j` and the running
                // accumulator, push `acc + hat_j(point.z)·d_j` (centred z).
                let z_prev = rd!();
                let z_self = rd!();
                let z_next = rd!();
                let dj = pop_v!();
                let acc = pop_v!();
                let w = crate::csg::loft_hat(pt[2], z_prev, z_self, z_next);
                push_v!(acc + w * dj);
            }
            OP_DISPLACE_VAL => {
                let depth = rd!();
                let freq = rd!();
                let d = pop_v!();
                let disp = depth
                    * (pt[0] * freq).sin()
                    * (pt[1] * freq * 1.3).sin()
                    * (pt[2] * freq * 0.7).sin();
                push_v!(d - disp);
            }
            OP_EXTRUDE => {
                let half_h = rd!();
                let scale = rd!();
                let cap_mask = rd!() as u32;
                let d2d = pop_v!();
                if cap_mask == 3 {
                    // Both caps retained: exact-corner composition (IQ).
                    let wz = pt[2].abs() - half_h;
                    let outside = (d2d.max(0.0).powi(2) + wz.max(0.0).powi(2)).sqrt();
                    let inside = d2d.max(wz).min(0.0);
                    push_v!((outside + inside) * scale);
                } else {
                    // Opened cap(s): plain max over the retained terms
                    // (mirrors `csg::extrude_retained_combine`).
                    let mut d = d2d;
                    if cap_mask & 1 != 0 {
                        d = d.max(pt[2] - half_h);
                    }
                    if cap_mask & 2 != 0 {
                        d = d.max(-pt[2] - half_h);
                    }
                    push_v!(d * scale);
                }
            }
            OP_REVOLVE_SCALE => {
                let scale = rd!();
                let d = pop_v!();
                push_v!(d * scale);
            }
            OP_HELIX_SCALE => {
                let scale = rd!();
                // pt[2] carries the signed end-overshoot chord stashed
                // by OP_WARP_HELIX; compose like `crate::helix_compose`.
                let d2 = pop_v!();
                let ell = pt[2];
                let outside = (d2.max(0.0)).hypot(ell.max(0.0));
                let inside = d2.max(ell).min(0.0);
                push_v!((outside + inside) * scale);
            }
            OP_OFFSET_2D => {
                let dist = rd!();
                let d = pop_v!();
                push_v!(d - dist);
            }
            OP_NEG => {
                let v = pop_v!();
                push_v!(-v);
            }

            // ── Domain warps ───────────────────────────────────
            OP_WARP_PUSH => {
                ps[psp] = pt;
                psp += 1;
            }
            OP_WARP_POP => {
                psp -= 1;
                pt = ps[psp];
            }
            OP_WARP_TWIST => {
                let rate = rd!();
                let angle = rate * pt[1];
                let (s, c) = angle.sin_cos();
                let px = pt[0];
                pt[0] = c * px - s * pt[2];
                pt[2] = s * px + c * pt[2];
            }
            OP_WARP_BEND => {
                let rate = rd!();
                let angle = rate * pt[0];
                let (s, c) = angle.sin_cos();
                let px = pt[0];
                pt[0] = c * px - s * pt[1];
                pt[1] = s * px + c * pt[1];
            }
            OP_WARP_TRANSFORM => {
                let translate = rd3!();
                let quat = [rd!(), rd!(), rd!(), rd!()];
                let scale = [rd!(), rd!(), rd!()];
                pt = world_to_local_inline(pt, translate, quat, scale);
            }
            OP_WARP_REVOLVE => {
                let r = (pt[0] * pt[0] + pt[2] * pt[2]).sqrt();
                pt = [r, pt[1], 0.0];
            }
            OP_WARP_HELIX => {
                let pitch = rd!();
                let hand = rd!();
                let turns = rd!();
                let y_anchor = rd!();
                let taper = rd!();
                let r_inner = rd!();
                let start_angle = rd!();
                // Mirrors `crate::helix_profile_point` — continuous
                // sweep-parameter clamp; the signed end-overshoot chord
                // rides along in pt[2] (2D sketch ops ignore z) until
                // OP_HELIX_SCALE composes it in.
                let tau = std::f32::consts::TAU;
                let r = (pt[0] * pt[0] + pt[2] * pt[2]).sqrt();
                let theta = pt[2].atan2(pt[0]);
                let th = hand * theta - start_angle;
                let n = ((pt[1] - y_anchor) / pitch - th / tau).round();
                let t_n = th + tau * n;
                let t_end = tau * turns;
                let t_c = t_n.clamp(0.0, t_end);
                let w = pt[1] - pitch * t_c / tau;
                let rp = r - taper * (pt[1] - y_anchor);
                let rr = r_inner.max(pitch / tau * 0.25).max(1e-3);
                let l_t = (t_n - t_end * 0.5).abs() - t_end * 0.5;
                let ell = l_t.signum()
                    * r.min(rr)
                    * (l_t.abs().min(std::f32::consts::PI) * 0.5).sin();
                pt = [rp, w, ell];
            }
            OP_WARP_SWEEP => {
                // Mirror of `SweepPath::cross_section`: nearest spine point,
                // project into the interpolated (n, b) frame → (u, v); the cap
                // coordinate w rides along in pt[2] until OP_HELIX_SCALE.
                use crate::vec3f::{cross, dot, normalize, sub};
                let n = rd!() as usize;
                let closed = rd!() > 0.5;
                let total_len = rd!();
                let twist = rd!();
                let mut pts = Vec::with_capacity(n);
                let mut nrm = Vec::with_capacity(n);
                let mut bnm = Vec::with_capacity(n);
                let mut arc = Vec::with_capacity(n);
                for _ in 0..n {
                    pts.push(rd3!());
                    nrm.push(rd3!());
                    bnm.push(rd3!());
                    arc.push(rd!());
                }
                let seg_count = if closed { n } else { n - 1 };
                let mut best_d2 = f32::INFINITY;
                let mut best_seg = 0usize;
                let mut best_t = 0.0f32;
                for si in 0..seg_count {
                    let a = pts[si];
                    let b = pts[(si + 1) % n];
                    let ab = sub(b, a);
                    let denom = dot(ab, ab);
                    let t = if denom > 1e-12 {
                        (dot(sub(pt, a), ab) / denom).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    let foot = [a[0] + ab[0] * t, a[1] + ab[1] * t, a[2] + ab[2] * t];
                    let d = sub(pt, foot);
                    let d2 = dot(d, d);
                    if d2 < best_d2 {
                        best_d2 = d2;
                        best_seg = si;
                        best_t = t;
                    }
                }
                let i0 = best_seg;
                let i1 = (best_seg + 1) % n;
                let ab = sub(pts[i1], pts[i0]);
                let foot = [
                    pts[i0][0] + ab[0] * best_t,
                    pts[i0][1] + ab[1] * best_t,
                    pts[i0][2] + ab[2] * best_t,
                ];
                let lerp = |a: [f32; 3], b: [f32; 3]| {
                    normalize([
                        a[0] + (b[0] - a[0]) * best_t,
                        a[1] + (b[1] - a[1]) * best_t,
                        a[2] + (b[2] - a[2]) * best_t,
                    ])
                };
                let nf = lerp(nrm[i0], nrm[i1]);
                let bf = lerp(bnm[i0], bnm[i1]);
                let d = sub(pt, foot);
                let mut u = dot(d, nf);
                let mut v = dot(d, bf);
                let s_foot = arc[i0] + (arc[i1] - arc[i0]) * best_t;
                let w = if closed {
                    f32::NEG_INFINITY
                } else {
                    let at_end = (best_seg == 0 && best_t <= 1e-6)
                        || (best_seg == seg_count - 1 && best_t >= 1.0 - 1e-6);
                    if at_end {
                        let taxis = normalize(cross(nf, bf));
                        dot(d, taxis).abs()
                    } else {
                        -s_foot.min(total_len - s_foot)
                    }
                };
                // Twist: un-rotate (u, v) by θ = twist · s/L (mirror of
                // SweepPath::cross_section).
                if twist != 0.0 && total_len > 1e-6 {
                    let theta = -twist * s_foot / total_len;
                    let (sn, cs) = theta.sin_cos();
                    let (nu, nv) = (cs * u - sn * v, sn * u + cs * v);
                    u = nu;
                    v = nv;
                }
                pt = [u, v, w];
            }

            // ── Masked blend ───────────────────────────────────
            OP_MASKED_BLEND => {
                let n = rd!() as usize;
                let mut polyline: Vec<[f32; 3]> = Vec::with_capacity(n);
                for _ in 0..n {
                    polyline.push(rd3!());
                }
                let mask_r = rd!();
                let blend_r = rd!();
                let mode = rd!();
                let is_diff = rd!() > 0.5;
                let db = pop_v!();
                let da = pop_v!();
                let mask_d = crate::csg::sdf_polyline_tube(pt, &polyline, mask_r);
                let blended = if is_diff {
                    -blend_min_eval(-da, db, blend_r, mode)
                } else {
                    blend_min_eval(da, db, blend_r, mode)
                };
                let hard = crate::csg::hard_combine(is_diff, da, db);
                push_v!(crate::csg::masked_blend_combine(mask_d, blended, hard, blend_r));
            }

            // ── Basis-Register eines Offset-Stopps (#384 §8i) ────
            // `base ← top of stack`, stack-neutral (pop + push desselben
            // Werts). Spiegel: `offset_channel::stop_apply` friert d₀ ein.
            OP_STOP_BASE => {
                let d = pop_v!();
                base = d;
                lim = -crate::offset_channel::END_PLANE_OFF;
                push_v!(d);
            }

            // ── Grenzwert-Klemme eines Offset-Stopps (#384 §8j) ────
            OP_STOP_LIMIT => {
                let concave = rd!() > 0.5;
                let d = pop_v!();
                push_v!(if concave { d.max(lim) } else { d.min(-lim) });
            }

            // ── Kanal-Korrekturkörper (offset_tree Phase B) ────
            // 1:1-Spiegel von `offset_channel::channel_apply` auf dem
            // Opcode-Strom (inkl. des exakten Bounding-Sphere-Skips —
            // feldwert-identisch, reine Kostenoptimierung).
            OP_ARC_CHANNEL => {
                const ARC_STRIDE: usize = 27;
                let n = rd!() as usize;
                let concave = rd!() > 0.5;
                let delta = rd!();
                let trust = rd!();
                let gate_w = rd!();
                let bc = rd3!();
                let br = rd!();
                // #384 §8f: drittes Gate — vier affine End-Halbräume,
                // `s₃ = min_k(⟨x,n̂ₖ⟩ − cₖ)` (Spiegel von `end_plane_gate`).
                let mut s3 = f32::INFINITY;
                for _ in 0..4 {
                    let n = rd3!();
                    let c = rd!();
                    s3 = s3.min(pt[0] * n[0] + pt[1] * n[1] + pt[2] * n[2] - c);
                }
                // #384 §8j: viertes Gate — vier Fremdflächen-Halbräume,
                // `s₄ = min_k(⟨x,n̂ₖ⟩ − cₖ)` (Spiegel von `face_plane_gate`).
                let mut s4 = f32::INFINITY;
                for _ in 0..4 {
                    let n = rd3!();
                    let c = rd!();
                    s4 = s4.min(pt[0] * n[0] + pt[1] * n[1] + pt[2] * n[2] - c);
                }
                let arc_base = pc;
                pc += n * ARC_STRIDE;
                let d = pop_v!();
                let v3 = |i: usize| [opcodes[i], opcodes[i + 1], opcodes[i + 2]];
                let dist3 = |a: [f32; 3], b: [f32; 3]| {
                    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2))
                        .sqrt()
                };
                let dbs = dist3(pt, bc) - br;
                // #384 §8j: der Skip muss auch den `lim`-Beitrag neutral
                // halten — `dbs ≥ trust` beweist `de ≥ trust`.
                if (trust - dbs <= if concave { -d } else { d }) && dbs >= trust {
                    push_v!(d);
                    continue;
                }
                // argmin über die Bögen.
                let mut best = f32::INFINITY;
                let mut best_q = pt;
                let mut best_m1 = [0.0f32; 3];
                let mut best_m2 = [0.0f32; 3];
                for k in 0..n {
                    let b = arc_base + k * ARC_STRIDE;
                    let abc = v3(b + 11);
                    let abr = opcodes[b + 14];
                    if dist3(pt, abc) - abr >= best {
                        continue;
                    }
                    let c = v3(b);
                    let u = v3(b + 3);
                    let v = v3(b + 6);
                    let r = opcodes[b + 9];
                    let sweep = opcodes[b + 10];
                    let (de, q, t) = if sweep < 0.0 {
                        // Segment c → c + u·r.
                        let len = r.max(1e-12);
                        let t = (((pt[0] - c[0]) * u[0]
                            + (pt[1] - c[1]) * u[1]
                            + (pt[2] - c[2]) * u[2])
                            / len)
                            .clamp(0.0, 1.0);
                        let q = [
                            c[0] + u[0] * (t * len),
                            c[1] + u[1] * (t * len),
                            c[2] + u[2] * (t * len),
                        ];
                        (dist3(pt, q), q, t)
                    } else {
                        let dc = [pt[0] - c[0], pt[1] - c[1], pt[2] - c[2]];
                        let x = dc[0] * u[0] + dc[1] * u[1] + dc[2] * u[2];
                        let y = dc[0] * v[0] + dc[1] * v[1] + dc[2] * v[2];
                        let rho = (x * x + y * y).sqrt();
                        let arc_point = |phi: f32| -> [f32; 3] {
                            let (s, cs) = phi.sin_cos();
                            [
                                c[0] + r * (u[0] * cs + v[0] * s),
                                c[1] + r * (u[1] * cs + v[1] * s),
                                c[2] + r * (u[2] * cs + v[2] * s),
                            ]
                        };
                        if rho < 1e-9 {
                            let q = arc_point(0.0);
                            (dist3(pt, q), q, 0.0)
                        } else {
                            let mut phi = y.atan2(x);
                            if phi < 0.0 {
                                phi += std::f32::consts::TAU;
                            }
                            if phi <= sweep {
                                let q = arc_point(phi);
                                (dist3(pt, q), q, phi / sweep)
                            } else {
                                let q0 = arc_point(0.0);
                                let q1 = arc_point(sweep);
                                let d0 = dist3(pt, q0);
                                let d1 = dist3(pt, q1);
                                if d0 <= d1 {
                                    (d0, q0, 0.0)
                                } else {
                                    (d1, q1, 1.0)
                                }
                            }
                        }
                    };
                    if de < best {
                        best = de;
                        best_q = q;
                        let nl = |a: [f32; 3], bb: [f32; 3]| -> [f32; 3] {
                            let w = [
                                a[0] + (bb[0] - a[0]) * t,
                                a[1] + (bb[1] - a[1]) * t,
                                a[2] + (bb[2] - a[2]) * t,
                            ];
                            let nn = (w[0] * w[0] + w[1] * w[1] + w[2] * w[2]).sqrt();
                            if nn > 1e-9 {
                                [w[0] / nn, w[1] / nn, w[2] / nn]
                            } else {
                                a
                            }
                        };
                        best_m1 = nl(v3(b + 15), v3(b + 21));
                        best_m2 = nl(v3(b + 18), v3(b + 24));
                    }
                }
                if !best.is_finite() {
                    push_v!(d);
                    continue;
                }
                let dq = [pt[0] - best_q[0], pt[1] - best_q[1], pt[2] - best_q[2]];
                let s1 = dq[0] * best_m1[0] + dq[1] * best_m1[1] + dq[2] * best_m1[2];
                let s2 = dq[0] * best_m2[0] + dq[1] * best_m2[1] + dq[2] * best_m2[2];
                // §8i: Gate gegen das Basis-Register (naiver Stopp-Wert d₀).
                let gate = if concave { gate_w - base } else { gate_w + base };
                // Rollen-Trennung (C4): Geometrie-Klemme vs. Add-Gate —
                // 1:1-Spiegel von `channel_apply`.
                let w_geo = s1.min(s2).min(s3).min(s4).min(best - delta).min(trust - best);
                let w = w_geo.min(gate);
                // #384 §8j: Beitrag zum Grenzwert-Register.
                if best < trust {
                    lim = lim.max(delta - best.min(s4 + delta));
                }
                push_v!(if concave {
                    if w > 0.0 {
                        d.min(-w)
                    } else if w_geo <= 0.0 {
                        d.min(-w_geo)
                    } else {
                        d
                    }
                } else {
                    d.max(w)
                });
            }

            // ── Vertex-Kugel (offset_tree Phase C4) ────
            // 1:1-Spiegel von `offset_channel::vertex_apply`.
            OP_VERTEX_SPHERE => {
                let concave = rd!() > 0.5;
                let delta = rd!();
                let trust = rd!();
                let gate_w = rd!();
                let v = rd3!();
                let g1 = rd3!();
                let g2 = rd3!();
                let g3 = rd3!();
                let d = pop_v!();
                let dq = [pt[0] - v[0], pt[1] - v[1], pt[2] - v[2]];
                let rho = (dq[0] * dq[0] + dq[1] * dq[1] + dq[2] * dq[2]).sqrt();
                // #384 §8j: Beitrag zum Grenzwert-Register (v liegt auf ∂Ω).
                if rho < trust {
                    lim = lim.max(delta - rho);
                }
                if trust - rho <= if concave { -d } else { d } {
                    push_v!(d);
                    continue;
                }
                let s1 = dq[0] * g1[0] + dq[1] * g1[1] + dq[2] * g1[2];
                let s2 = dq[0] * g2[0] + dq[1] * g2[1] + dq[2] * g2[2];
                let s3 = dq[0] * g3[0] + dq[1] * g3[1] + dq[2] * g3[2];
                // §8i: Gate gegen das Basis-Register (naiver Stopp-Wert d₀).
                let gate = if concave { gate_w - base } else { gate_w + base };
                // Rollen-Trennung (C4) — 1:1-Spiegel von `vertex_apply`.
                let w_geo = s1.min(s2).min(s3).min(rho - delta).min(trust - rho);
                let w = w_geo.min(gate);
                push_v!(if concave {
                    if w > 0.0 {
                        d.min(-w)
                    } else if w_geo <= 0.0 {
                        d.min(-w_geo)
                    } else {
                        d
                    }
                } else {
                    d.max(w)
                });
            }

            // ── Rand-Fillet (offset_tree Phase 2, #398) ────
            // 1:1-Spiegel von `offset_rim::apply_rims_f32` auf dem Opcode-Strom:
            // Nearest-Feature-Komposition (kleinste Kanten-Distanz gewinnt, bei
            // Gleichstand der ERSTE Kandidat), δ−de konkav / de−δ konvex, mit den
            // drei Masken der f64-Referenz (Trust; End-Caps t∈[0,1] strikt mit
            // cap_eps 1e-6; Wand-Halbräume mit eps 1e-9; ungenutzte n_c=[0;3]
            // → Maske true). Pops g, pusht die korrigierte Distanz.
            OP_RIM_FILLET => {
                let n = rd!() as usize;
                let concave = rd!() > 0.5;
                let rim_base = pc;
                pc += n * RIM_STRIDE;
                let g = pop_v!();
                let v3 = |i: usize| [opcodes[i], opcodes[i + 1], opcodes[i + 2]];
                let dot3 = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
                let dist3 = |a: [f32; 3], b: [f32; 3]| {
                    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
                };
                let mut best_de = f32::INFINITY;
                let mut best_corr = 0.0f32;
                for k in 0..n {
                    let b = rim_base + k * RIM_STRIDE;
                    let ra = v3(b);
                    let rb = v3(b + 3);
                    let n_a = v3(b + 6);
                    let n_b = v3(b + 9);
                    let n_c = v3(b + 12);
                    let delta = opcodes[b + 15];
                    let trust = opcodes[b + 16];
                    // closest_t: geklemmter Lotfußpunkt q, ungeklemmter Parameter t.
                    let d = [rb[0] - ra[0], rb[1] - ra[1], rb[2] - ra[2]];
                    let len2 = dot3(d, d);
                    let (de, q, t) = if len2 < 1e-24 {
                        (dist3(pt, ra), ra, 0.0)
                    } else {
                        let t_raw = ((pt[0] - ra[0]) * d[0]
                            + (pt[1] - ra[1]) * d[1]
                            + (pt[2] - ra[2]) * d[2])
                            / len2;
                        let tc = t_raw.clamp(0.0, 1.0);
                        let q = [ra[0] + d[0] * tc, ra[1] + d[1] * tc, ra[2] + d[2] * tc];
                        (dist3(pt, q), q, t_raw)
                    };
                    if de > trust {
                        continue;
                    }
                    let seg = dist3(rb, ra);
                    if seg > 1e-9 && (t < -1e-6 || t > 1.0 + 1e-6) {
                        continue;
                    }
                    let dq = [pt[0] - q[0], pt[1] - q[1], pt[2] - q[2]];
                    let side = |nn: [f32; 3]| -> bool {
                        if dist3(nn, [0.0; 3]) < 0.5 {
                            return true; // ungenutzte dritte Wand
                        }
                        let s = dot3(dq, nn);
                        if concave {
                            s <= 1e-9
                        } else {
                            s >= -1e-9
                        }
                    };
                    if !(side(n_a) && side(n_b) && side(n_c)) {
                        continue;
                    }
                    // Nearest-Feature: strikt kleineres de gewinnt (Gleichstand
                    // → erster Kandidat, wie `apply_rims`).
                    if de < best_de {
                        best_de = de;
                        best_corr = if concave { delta - de } else { de - delta };
                    }
                }
                push_v!(if best_de.is_finite() {
                    if concave {
                        g.min(best_corr)
                    } else {
                        g.max(best_corr)
                    }
                } else {
                    g
                });
            }

            // ── Gemischter Bogen-Rand-Fillet (FC Phase 4, #398) ────
            // 1:1-Spiegel von `offset_rim::apply_rims_arc_f32`: Nearest-Feature
            // über Segment- UND Bogen-Records (kleinstes de gewinnt, Gleichstand
            // → erster). Segment (sweep<0) wie OP_RIM_FILLET, Bogen (sweep>0) mit
            // Radialnormale wall_sign·(q−c)/r und Sweep-End-Cap. Pops g.
            OP_RIM_FILLET_ARC => {
                let n = rd!() as usize;
                let concave = rd!() > 0.5;
                let base = pc;
                pc += n * RIM_ARC_STRIDE;
                let g = pop_v!();
                let v3 = |i: usize| [opcodes[i], opcodes[i + 1], opcodes[i + 2]];
                let dot3 = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
                let dist3 = |a: [f32; 3], b: [f32; 3]| {
                    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
                };
                let mut best_de = f32::INFINITY;
                let mut best_corr = 0.0f32;
                for k in 0..n {
                    let b = base + k * RIM_ARC_STRIDE;
                    let c = v3(b);
                    let u = v3(b + 3);
                    let vv = v3(b + 6);
                    let n_a = v3(b + 9);
                    let n_b = v3(b + 12);
                    let r = opcodes[b + 15];
                    let sweep = opcodes[b + 16];
                    let wall_sign = opcodes[b + 17];
                    let delta = opcodes[b + 18];
                    let trust = opcodes[b + 19];
                    // foot: (de, q, in_cap) + Radialnormale n_b_eff.
                    let cap_eps = 1e-6f32;
                    let (de, q, in_cap, n_b_eff) = if sweep < 0.0 {
                        let len = r.max(1e-12);
                        let s = dot3([pt[0] - c[0], pt[1] - c[1], pt[2] - c[2]], u);
                        let t = s / len;
                        let sc = s.clamp(0.0, len);
                        let q = [c[0] + u[0] * sc, c[1] + u[1] * sc, c[2] + u[2] * sc];
                        (dist3(pt, q), q, !(t < -cap_eps || t > 1.0 + cap_eps), n_b)
                    } else {
                        let dc = [pt[0] - c[0], pt[1] - c[1], pt[2] - c[2]];
                        let x = dot3(dc, u);
                        let y = dot3(dc, vv);
                        let mut phi = y.atan2(x);
                        if phi < 0.0 {
                            phi += std::f32::consts::TAU;
                        }
                        let in_cap = phi <= sweep + cap_eps;
                        let pc2 = phi.clamp(0.0, sweep);
                        let (sn, cs) = pc2.sin_cos();
                        let q = [
                            c[0] + r * (u[0] * cs + vv[0] * sn),
                            c[1] + r * (u[1] * cs + vv[1] * sn),
                            c[2] + r * (u[2] * cs + vv[2] * sn),
                        ];
                        let d = [q[0] - c[0], q[1] - c[1], q[2] - c[2]];
                        let nn = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-30);
                        let n_b_eff = [wall_sign * d[0] / nn, wall_sign * d[1] / nn, wall_sign * d[2] / nn];
                        (dist3(pt, q), q, in_cap, n_b_eff)
                    };
                    if !in_cap || de > trust {
                        continue;
                    }
                    let dq = [pt[0] - q[0], pt[1] - q[1], pt[2] - q[2]];
                    let side = |nn: [f32; 3]| -> bool {
                        if dist3(nn, [0.0; 3]) < 0.5 {
                            return true;
                        }
                        let s = dot3(dq, nn);
                        if concave {
                            s <= 1e-9
                        } else {
                            s >= -1e-9
                        }
                    };
                    if !(side(n_a) && side(n_b_eff)) {
                        continue;
                    }
                    if de < best_de {
                        best_de = de;
                        best_corr = if concave { delta - de } else { de - delta };
                    }
                }
                push_v!(if best_de.is_finite() {
                    if concave {
                        g.min(best_corr)
                    } else {
                        g.max(best_corr)
                    }
                } else {
                    g
                });
            }

            OP_PRIM_TEX3D => {
                let tex_idx = rd!() as usize;
                let (local, scale) = read_transform_and_localise(opcodes, &mut pc, pt);
                let aabb_min = rd3!();
                let aabb_max = rd3!();
                if let Some(vol) = volumes.get(tex_idx) {
                    push_v!(vol.sample_trilinear(local) * scale);
                } else {
                    push_v!(f32::MAX);
                }
                let _ = (aabb_min, aabb_max);
            }

            OP_END => break,
            _ => break,
        }
    }

    if vsp > 0 {
        vs[vsp - 1]
    } else {
        f32::MAX
    }
}

// ── Inline SDF helpers (mirror what the WGSL shader does) ──────────

fn read_transform_and_localise(
    buf: &[f32],
    pc: &mut usize,
    pt: [f32; 3],
) -> ([f32; 3], f32) {
    let translate = [buf[*pc], buf[*pc + 1], buf[*pc + 2]];
    let quat = [buf[*pc + 3], buf[*pc + 4], buf[*pc + 5], buf[*pc + 6]];
    let scale = [buf[*pc + 7], buf[*pc + 8], buf[*pc + 9]];
    *pc += 10;
    let local = world_to_local_inline(pt, translate, quat, scale);
    let min_s = scale[0].min(scale[1]).min(scale[2]);
    (local, min_s)
}

fn world_to_local_inline(
    p: [f32; 3],
    translate: [f32; 3],
    quat: [f32; 4],
    scale: [f32; 3],
) -> [f32; 3] {
    let centered = [
        p[0] - translate[0],
        p[1] - translate[1],
        p[2] - translate[2],
    ];
    let qx = -quat[0];
    let qy = -quat[1];
    let qz = -quat[2];
    let qw = quat[3];
    let t = [
        2.0 * (qy * centered[2] - qz * centered[1]),
        2.0 * (qz * centered[0] - qx * centered[2]),
        2.0 * (qx * centered[1] - qy * centered[0]),
    ];
    let rotated = [
        centered[0] + qw * t[0] + qy * t[2] - qz * t[1],
        centered[1] + qw * t[1] + qz * t[0] - qx * t[2],
        centered[2] + qw * t[2] + qx * t[1] - qy * t[0],
    ];
    [rotated[0] / scale[0], rotated[1] / scale[1], rotated[2] / scale[2]]
}

fn sdf_box_local(p: [f32; 3], half: [f32; 3]) -> f32 {
    let q = [
        p[0].abs() - half[0],
        p[1].abs() - half[1],
        p[2].abs() - half[2],
    ];
    let outside = (q[0].max(0.0).powi(2) + q[1].max(0.0).powi(2) + q[2].max(0.0).powi(2)).sqrt();
    let inside = q[0].max(q[1]).max(q[2]).min(0.0);
    outside + inside
}

fn sdf_sphere_local(p: [f32; 3], r: f32) -> f32 {
    (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt() - r
}

fn sdf_cylinder_local(p: [f32; 3], r: f32, hh: f32) -> f32 {
    let radial = (p[0] * p[0] + p[2] * p[2]).sqrt() - r;
    let axial = p[1].abs() - hh;
    let outside = radial.max(0.0).hypot(axial.max(0.0));
    let inside = radial.max(axial).min(0.0);
    outside + inside
}

fn sdf_torus_local(p: [f32; 3], major: f32, minor: f32) -> f32 {
    let q0 = (p[0] * p[0] + p[2] * p[2]).sqrt() - major;
    (q0 * q0 + p[1] * p[1]).sqrt() - minor
}

fn sdf_cone_local(p: [f32; 3], rb: f32, rt: f32, hh: f32) -> f32 {
    let r = (p[0] * p[0] + p[2] * p[2]).sqrt();
    let t = ((p[1] + hh) / (2.0 * hh)).clamp(0.0, 1.0);
    let r_at_y = rb * (1.0 - t) + rt * t;
    // Project the radial gap onto the inclined wall normal so the field is
    // the true perpendicular distance (Lipschitz-1); cylinder (rb == rt) → 1.
    // Mirrors `fd_sdf_cone` in web/src/sdfRenderer.ts — this mirror had
    // DRIFTED from the shader (missing wall_scale, caught by the B7 parity
    // test): for a 45° cone the unprojected gap overestimates by ~40%.
    let drr = rb - rt;
    let slant_len = (drr * drr + (2.0 * hh) * (2.0 * hh)).sqrt();
    let wall_scale = if slant_len > 1e-9 { (2.0 * hh) / slant_len } else { 1.0 };
    let dr = (r - r_at_y) * wall_scale;
    let da = p[1].abs() - hh;
    if dr > 0.0 && da > 0.0 {
        (dr * dr + da * da).sqrt()
    } else {
        dr.max(da)
    }
}

fn sdf_capsule_local(p: [f32; 3], r: f32, hh: f32) -> f32 {
    let rv = (p[0] * p[0] + p[2] * p[2]).sqrt();
    let yc = p[1].clamp(-hh, hh);
    let dy = p[1] - yc;
    (rv * rv + dy * dy).sqrt() - r
}

fn sdf_hex_prism_local(p: [f32; 3], apothem: f32, hh: f32) -> f32 {
    let qx = p[0].abs();
    let qz = p[2].abs();
    let d_hex = (qx * 0.866_025_4 + qz * 0.5).max(qz) - apothem;
    let d_axial = p[1].abs() - hh;
    if d_hex > 0.0 && d_axial > 0.0 {
        (d_hex * d_hex + d_axial * d_axial).sqrt()
    } else {
        d_hex.max(d_axial)
    }
}

fn sdf_ellipsoid_local(p: [f32; 3], radii: [f32; 3]) -> f32 {
    let r_min = radii[0].min(radii[1]).min(radii[2]);
    let q = [p[0] / radii[0], p[1] / radii[1], p[2] / radii[2]];
    let len = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2]).sqrt();
    (len - 1.0) * r_min
}

fn sdf_gyroid_local(p: [f32; 3], cell_size: f32, thickness: f32) -> f32 {
    let s = cell_size;
    let x = p[0] / s;
    let y = p[1] / s;
    let z = p[2] / s;
    let f = x.sin() * y.cos() + y.sin() * z.cos() + z.sin() * x.cos();
    (f.abs() - thickness) * s * 0.2887
}

fn sdf_thread_local(
    p: [f32; 3],
    nominal_radius: f32,
    pitch: f32,
    depth: f32,
    half_height: f32,
) -> f32 {
    let r = (p[0] * p[0] + p[2] * p[2]).sqrt();
    let theta = p[2].atan2(p[0]);
    let y_unwrapped = p[1] - pitch * theta / std::f32::consts::TAU;
    let phase = (y_unwrapped / pitch) * std::f32::consts::TAU;
    let r_profile = nominal_radius - depth * 0.5 * (1.0 - phase.cos());
    let d_radial = r - r_profile;
    let d_axial = p[1].abs() - half_height;
    // Lipschitz normalization: |∇f| peaks at sqrt(1 + (π·depth/pitch)²) because
    // the helix folds y into r_profile's phase; divide by it so the field is a
    // conservative ≤1 distance and the sphere-march never overshoots the groove
    // walls (cf. Gyroid 0.2887 / Cone wall_scale). See sdf_dual::Thread.
    let lip = (1.0 + (std::f32::consts::PI * depth / pitch.max(1e-6)).powi(2)).sqrt();
    d_radial.max(d_axial) / lip
}

fn sdf_polygon_2d(p: [f32; 2], verts: &[[f32; 2]]) -> f32 {
    if verts.len() < 3 {
        return f32::MAX;
    }
    let n = verts.len();
    let mut d = f32::MAX;
    let mut sign = 1.0f32;
    let mut j = n - 1;
    for i in 0..n {
        let vi = verts[i];
        let vj = verts[j];
        let e = [vj[0] - vi[0], vj[1] - vi[1]];
        let w = [p[0] - vi[0], p[1] - vi[1]];
        let t = (w[0] * e[0] + w[1] * e[1]) / (e[0] * e[0] + e[1] * e[1]);
        let t = t.clamp(0.0, 1.0);
        let dx = w[0] - e[0] * t;
        let dy = w[1] - e[1] * t;
        let dd = dx * dx + dy * dy;
        if dd < d {
            d = dd;
        }
        // Winding number (crossing test)
        let c1 = p[1] >= vi[1];
        let c2 = p[1] < vj[1];
        let c3 = e[0] * w[1] > e[1] * w[0];
        if (c1 && c2 && c3) || (!c1 && !c2 && !c3) {
            sign = -sign;
        }
        j = i;
    }
    sign * d.sqrt()
}

fn sdf_sector_2d(
    p: [f32; 2],
    center: [f32; 2],
    radius: f32,
    start_angle: f32,
    sweep_angle: f32,
) -> f32 {
    let dx = p[0] - center[0];
    let dy = p[1] - center[1];
    let r = (dx * dx + dy * dy).sqrt();
    let angle = dy.atan2(dx);
    let mid = start_angle + sweep_angle * 0.5;
    let half = sweep_angle.abs() * 0.5;
    let mut rel = angle - mid;
    while rel > std::f32::consts::PI {
        rel -= std::f32::consts::TAU;
    }
    while rel < -std::f32::consts::PI {
        rel += std::f32::consts::TAU;
    }
    let d_circ = r - radius;
    if rel.abs() <= half {
        d_circ
    } else {
        let edge_angle = if rel > 0.0 {
            start_angle + sweep_angle
        } else {
            start_angle
        };
        let ex = center[0] + radius * edge_angle.cos();
        let ey = center[1] + radius * edge_angle.sin();
        let to_edge = ((p[0] - ex).powi(2) + (p[1] - ey).powi(2)).sqrt();
        let to_center = r;
        d_circ.max(0.0).min(to_edge.min(to_center))
    }
}

fn blend_min_eval(a: f32, b: f32, r: f32, mode: f32) -> f32 {
    if mode < 0.5 {
        // Fillet — delegates to `csg::smin_poly` (#384 Circular-CD smin) so
        // the tape stays byte-consistent with the CPU/dual/fidget family.
        crate::csg::smin_poly(a, b, r)
    } else {
        // Chamfer
        a.min(b).min((a + b - r) * std::f32::consts::FRAC_1_SQRT_2)
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::csg::{CsgNode, DecodedFeature, EvalMode, PrimitiveModifiers};
    use fielddraft_protocol::{BoolOp, FeatureId, Primitive, Transform};

    /// Displace ist nicht 1-Lipschitz (L ≈ 1 + |depth·freq|·√(1+1.3²+0.7²)) —
    /// der Renderer bekommt eine Schritt-Dämpfung 1/L, das FELD bleibt
    /// unnormalisiert (Feld-Normalisierung macht den Lichen-Mesher messbar
    /// schlechter, siehe bvh.rs-Befund 2026-07-13).
    #[test]
    fn displace_march_relax_dampens_by_lipschitz_bound() {
        let inner = Box::new(BoundedCsg::Empty);
        let node = |depth: f32, freq: f32| BoundedCsg::Displace {
            inner: inner.clone(),
            depth,
            freq,
            aabb: crate::csg::Aabb::EMPTY,
        };
        // depth 0.3 / freq 1.5 → L = 1 + 0.45·√3.18 ≈ 1.8025 → relax ≈ 0.5548
        let r = march_relax(&node(0.3, 1.5));
        let l = 1.0 + 0.45 * (1.0f32 + 1.69 + 0.49).sqrt();
        assert!((r - 1.0 / l).abs() < 1e-4, "relax={r}, erwartet {}", 1.0 / l);
        // Nulltiefe → keine Störung → volle Schrittweite.
        assert_eq!(march_relax(&node(0.0, 5.0)), 1.0);
        // Vorzeichen egal (|depth·freq|).
        assert!(march_relax(&node(-0.3, 1.5)) < 0.6);
    }

    /// #384 Stufe 2: hergeleitete Lipschitz-Schranken statt Pauschalen.
    #[test]
    fn lipschitz_net_derived_bounds() {
        use crate::csg::BlendMode;
        use crate::sketch2d::{Sketch2d, Sketch2dPrimitive};

        let boxp = |id: u32, half: [f32; 3]| CsgNode::Leaf {
            id: FeatureId::from_u32(id),
            primitive: Primitive::Box { center: [0.0; 3], half_extents: half },
        };
        let relax = |n: &CsgNode| march_relax(&BoundedCsg::from_csg(n));

        // Revolve: Meridian-Map + exakte analytische 2D-Distanz → 1-Lipschitz.
        // Die frühere 0.7-Pauschale (REVOLVE_MARCH_RELAX) ist obsolet.
        let revolve = CsgNode::Revolve {
            wall_subs: None,
            id: FeatureId::from_u32(1),
            sketch: Box::new(Sketch2d::Prim(Sketch2dPrimitive::Rectangle {
                center: [5.0, 0.0],
                half_extents: [2.0, 4.0],
            })),
        };
        assert_eq!(relax(&revolve), 1.0);

        // Twist: L = 1 + |rate|·r_max mit r_max = max √(x²+z²) über die
        // Kind-AABB (Box halb [30,50,10] → r = √(900+100)).
        let rate = std::f32::consts::FRAC_PI_2 / 100.0;
        let twist = CsgNode::Twist(Box::new(boxp(1, [30.0, 50.0, 10.0])), rate);
        let expect = 1.0 / (1.0 + rate * (900.0f32 + 100.0).sqrt());
        let r = relax(&twist);
        assert!((r - expect).abs() < 1e-4, "twist relax={r}, erwartet {expect}");

        // Bend: Hebelarm in xy (Box halb [50,8,8] → r = √(2500+64)).
        let bend = CsgNode::Bend(Box::new(boxp(1, [50.0, 8.0, 8.0])), rate);
        let expect = 1.0 / (1.0 + rate * (2500.0f32 + 64.0).sqrt());
        let r = relax(&bend);
        assert!((r - expect).abs() < 1e-4, "bend relax={r}, erwartet {expect}");

        // Fillet (Circular-CD, Konvexkombination) bleibt neutral …
        let fillet = CsgNode::SmoothUnion(
            Box::new(boxp(1, [10.0; 3])),
            Box::new(boxp(2, [10.0; 3])),
            3.0,
            BlendMode::Fillet,
        );
        assert_eq!(relax(&fillet), 1.0);

        // … Chamfer nicht: Schnittebenen-Term (∇a+∇b)/√2 → L = √2.
        let chamfer = |inner: CsgNode| {
            CsgNode::SmoothUnion(
                Box::new(inner),
                Box::new(boxp(9, [10.0; 3])),
                3.0,
                BlendMode::Chamfer,
            )
        };
        let c1 = chamfer(boxp(1, [10.0; 3]));
        let r = relax(&c1);
        assert!(
            (r - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6,
            "chamfer relax={r}"
        );
        // Verschachtelte Chamfer konvergieren (L ← (L+1)/√2, Fixpunkt
        // 1/(√2−1) ≈ 2.414) statt unbegrenzt zu wachsen.
        let mut node = boxp(1, [10.0; 3]);
        for _ in 0..12 {
            node = chamfer(node);
        }
        let r = relax(&node);
        assert!(r > 1.0 / 2.4143 - 1e-4, "chamfer-Kette relax={r} unter Fixpunkt");
        assert!(r < std::f32::consts::FRAC_1_SQRT_2, "chamfer-Kette relax={r} zu lasch");

        // Stapelung: Twist ∘ Displace multipliziert/addiert entlang des Pfads
        // (Warp × (1 + Displace-Term)) statt min() der Einzelwerte.
        let displaced = CsgNode::Displace(Box::new(boxp(1, [30.0, 50.0, 10.0])), 0.3, 1.5);
        let stacked = CsgNode::Twist(Box::new(displaced), rate);
        let l_disp = 1.0 + 0.45 * (1.0f32 + 1.69 + 0.49).sqrt();
        // Displace bläht die AABB um depth auf — Hebelarm entsprechend größer.
        let bb = BoundedCsg::from_csg(&CsgNode::Displace(
            Box::new(boxp(1, [30.0, 50.0, 10.0])),
            0.3,
            1.5,
        ))
        .aabb();
        let rx = bb.max[0].abs().max(bb.min[0].abs());
        let rz = bb.max[2].abs().max(bb.min[2].abs());
        let l_twist = 1.0 + rate * (rx * rx + rz * rz).sqrt();
        let expect = 1.0 / (l_disp * l_twist);
        let r = relax(&stacked);
        assert!((r - expect).abs() < 1e-4, "stack relax={r}, erwartet {expect}");
    }

    fn box_feature(id: u32, op: BoolOp, center: [f32; 3], half: [f32; 3]) -> DecodedFeature {
        DecodedFeature::from_parts(
            FeatureId::from_u32(id),
            format!("Box {id}"),
            op,
            Primitive::Box {
                center,
                half_extents: half,
            },
            Transform::IDENTITY,
            PrimitiveModifiers::NONE,
            vec![],
        )
    }

    fn sphere_feature(id: u32, center: [f32; 3], radius: f32) -> DecodedFeature {
        DecodedFeature::from_parts(
            FeatureId::from_u32(id),
            format!("Sphere {id}"),
            BoolOp::New,
            Primitive::Sphere { center, radius },
            Transform::IDENTITY,
            PrimitiveModifiers::NONE,
            vec![],
        )
    }

    fn translated_feature(
        id: u32,
        op: BoolOp,
        primitive: Primitive,
        translate: [f32; 3],
    ) -> DecodedFeature {
        DecodedFeature::from_parts(
            FeatureId::from_u32(id),
            format!("F{id}"),
            op,
            primitive,
            Transform {
                translate,
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0, 1.0, 1.0],
            },
            PrimitiveModifiers::NONE,
            vec![],
        )
    }

    fn assert_sdf_match(bvh: &BoundedCsg, compiled: &CompiledSdf, points: &[[f32; 3]], tol: f32) {
        for &p in points {
            let expected = bvh.sdf(p, EvalMode::Visual);
            let got = eval_opcodes(&compiled.opcodes, p);
            // Same-sign infinities agree (an emptied body reads +∞ on both
            // paths; the literal difference would be NaN).
            if expected.is_infinite() && got.is_infinite() && expected.signum() == got.signum() {
                continue;
            }
            assert!(
                (expected - got).abs() < tol,
                "mismatch at {p:?}: bvh={expected} compiled={got} (tol={tol})",
            );
        }
    }

    #[test]
    fn helix_opcode_stream_matches_tree_eval() {
        // Thread-like helix (rect profile r∈[8,10], pitch 2, 5 turns)
        // unioned onto a core cylinder — GPU/CPU opcode parity against
        // the BVH tree eval, including the Lipschitz normalisation.
        let features = vec![
            DecodedFeature::from_parts(
                FeatureId::from_u32(1),
                "Core".into(),
                BoolOp::New,
                Primitive::Cylinder { center: [0.0, 0.0, 0.0], radius: 8.2, half_height: 7.0 },
                Transform::IDENTITY,
                PrimitiveModifiers::NONE,
                vec![],
            ),
            DecodedFeature::from_parts(
                FeatureId::from_u32(2),
                "Helix".into(),
                BoolOp::Add,
                Primitive::Helix {
                    sketch: fielddraft_protocol::Sketch2d::Prim(
                        fielddraft_protocol::Sketch2dPrimitive::Rectangle {
                            center: [9.0, 0.0],
                            half_extents: [1.0, 0.6],
                        },
                    ),
                    center: [0.0, 0.0, 0.0],
                    pitch: 2.0,
                    turns: 5.0,
                    left_handed: false,
                    taper: 0.0,
                    // Non-zero so the parity check exercises the
                    // start-angle path of the opcode stream too.
                    start_angle: 0.6,
                    wall_subs: None,
                },
                Transform::IDENTITY,
                PrimitiveModifiers::NONE,
                vec![],
            ),
        ];
        let bvh = BoundedCsg::from_csg(&CsgNode::from_decoded(&features));
        let compiled = compile_body(&bvh);
        let points = [
            [9.0, 0.0, 0.0],
            [9.0, 1.0, 0.0],
            [0.0, 0.5, 9.0],
            [9.0, 2.0, 0.0],
            [9.0, 12.0, 0.0],
            [11.5, 0.0, 0.0],
            [0.0, 0.0, 0.0],
            [4.0, 3.3, 4.0],
            [-9.0, 5.1, 0.2],
            // End-region seam pairs (the vtyh fix lives here).
            [-9.0, -1.0, 0.05],
            [-9.0, -1.0, -0.05],
            [-9.0, 11.0, 0.05],
            [-9.0, 11.0, -0.05],
        ];
        assert_sdf_match(&bvh, &compiled, &points, 1e-4);
    }

    #[test]
    fn sweep_opcode_stream_matches_tree_eval() {
        // Disk r=5 swept along a gentle 3D wave — opcode VM (the GPU mirror)
        // vs the BoundedCsg tree eval, exercising the nearest-segment unwrap,
        // interpolated RMF frame, and the end-cap composition.
        let path: Vec<[f32; 3]> = (0..=10)
            .map(|i| {
                let x = i as f32 * 5.0;
                [x, (x * 0.03).sin() * 5.0, (x * 0.02).cos() * 3.0]
            })
            .collect();
        let node = CsgNode::Sweep {
            id: FeatureId::from_u32(1),
            sketch: Box::new(fielddraft_protocol::Sketch2d::Prim(
                fielddraft_protocol::Sketch2dPrimitive::Circle {
                    center: [0.0, 0.0],
                    radius: 5.0,
                },
            )),
            path,
            closed: false,
            // Non-zero twist so the parity check exercises the rotation path.
            twist: 1.5,
        };
        let bvh = BoundedCsg::from_csg(&node);
        let compiled = compile_body(&bvh);
        let points = [
            [0.0, 0.0, 3.0],
            [25.0, 0.0, 0.0],
            [25.0, 10.0, 0.0],
            [50.0, 0.0, 3.0],
            [52.0, 0.0, 3.0],
            [10.0, 8.0, 5.0],
            [40.0, -6.0, -2.0],
        ];
        assert_sdf_match(&bvh, &compiled, &points, 1e-3);
    }

    #[test]
    fn single_box_at_origin() {
        let features = vec![box_feature(
            1,
            BoolOp::New,
            [0.0, 0.0, 0.0],
            [10.0, 10.0, 10.0],
        )];
        let bvh = BoundedCsg::from_csg(&CsgNode::from_decoded(&features));
        let compiled = compile_body(&bvh);
        let points = [
            [0.0, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [15.0, 0.0, 0.0],
            [5.0, 5.0, 5.0],
            [-10.0, -10.0, -10.0],
            [20.0, 20.0, 20.0],
        ];
        assert_sdf_match(&bvh, &compiled, &points, 1e-4);
    }

    #[test]
    fn box_with_offset_center() {
        let features = vec![box_feature(
            1,
            BoolOp::New,
            [5.0, -3.0, 7.0],
            [4.0, 4.0, 4.0],
        )];
        let bvh = BoundedCsg::from_csg(&CsgNode::from_decoded(&features));
        let compiled = compile_body(&bvh);
        let points = [
            [5.0, -3.0, 7.0],
            [9.0, -3.0, 7.0],
            [0.0, 0.0, 0.0],
            [5.0, 1.0, 11.0],
            [10.0, 10.0, 10.0],
        ];
        assert_sdf_match(&bvh, &compiled, &points, 1e-4);
    }

    #[test]
    fn sphere_translated() {
        let features = vec![translated_feature(
            1,
            BoolOp::New,
            Primitive::Sphere {
                center: [0.0, 0.0, 0.0],
                radius: 8.0,
            },
            [10.0, 20.0, 30.0],
        )];
        let bvh = BoundedCsg::from_csg(&CsgNode::from_decoded(&features));
        let compiled = compile_body(&bvh);
        let points = [
            [10.0, 20.0, 30.0],
            [18.0, 20.0, 30.0],
            [0.0, 0.0, 0.0],
            [10.0, 20.0, 38.0],
        ];
        assert_sdf_match(&bvh, &compiled, &points, 1e-4);
    }

    #[test]
    fn union_of_two_boxes() {
        let features = vec![
            box_feature(1, BoolOp::New, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
            box_feature(2, BoolOp::New, [30.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
        ];
        let bvh = BoundedCsg::from_csg(&CsgNode::from_decoded(&features));
        let compiled = compile_body(&bvh);
        let points = [
            [0.0, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [15.0, 0.0, 0.0],
            [30.0, 0.0, 0.0],
            [20.0, 0.0, 0.0],
            [50.0, 50.0, 50.0],
        ];
        assert_sdf_match(&bvh, &compiled, &points, 1e-4);
    }

    #[test]
    fn difference_box_minus_sphere() {
        let features = vec![
            box_feature(1, BoolOp::New, [0.0, 0.0, 0.0], [20.0, 20.0, 20.0]),
            {
                let mut f = sphere_feature(2, [10.0, 10.0, 10.0], 8.0);
                f.op = BoolOp::Remove;
                f
            },
        ];
        let bvh = BoundedCsg::from_csg(&CsgNode::from_decoded(&features));
        let compiled = compile_body(&bvh);
        let points = [
            [0.0, 0.0, 0.0],
            [10.0, 10.0, 10.0],
            [20.0, 0.0, 0.0],
            [15.0, 5.0, 5.0],
            [-10.0, -10.0, -10.0],
        ];
        assert_sdf_match(&bvh, &compiled, &points, 1e-4);
    }

    #[test]
    fn cylinder_and_torus() {
        let features = vec![
            translated_feature(
                1,
                BoolOp::New,
                Primitive::Cylinder {
                    center: [0.0, 0.0, 0.0],
                    radius: 5.0,
                    half_height: 15.0,
                },
                [0.0, 0.0, 0.0],
            ),
            translated_feature(
                2,
                BoolOp::New,
                Primitive::Torus {
                    center: [0.0, 0.0, 0.0],
                    major_radius: 10.0,
                    minor_radius: 3.0,
                },
                [0.0, 0.0, 0.0],
            ),
        ];
        let bvh = BoundedCsg::from_csg(&CsgNode::from_decoded(&features));
        let compiled = compile_body(&bvh);
        let points = [
            [0.0, 0.0, 0.0],
            [5.0, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [0.0, 15.0, 0.0],
            [13.0, 0.0, 0.0],
        ];
        assert_sdf_match(&bvh, &compiled, &points, 1e-3);
    }

    #[test]
    fn extruded_rectangle() {
        use crate::sketch2d::{Sketch2d, Sketch2dPrimitive};

        let sketch = Sketch2d::Prim(Sketch2dPrimitive::Rectangle {
            center: [0.0, 0.0],
            half_extents: [5.0, 3.0],
        });
        let csg = CsgNode::Extrude { wall_subs: None,
            id: FeatureId::from_u32(1),
            sketch: Box::new(sketch),
            half_height: 2.0,
        };
        let bvh = BoundedCsg::from_csg(&csg);
        let compiled = compile_body(&bvh);
        let points = [
            [0.0, 0.0, 0.0],
            [5.0, 0.0, 0.0],
            [0.0, 3.0, 0.0],
            [0.0, 0.0, 2.0],
            [5.0, 3.0, 2.0],
            [7.0, 0.0, 0.0],
            [10.0, 10.0, 10.0],
        ];
        assert_sdf_match(&bvh, &compiled, &points, 1e-4);
    }

    #[test]
    fn extruded_washer_ring() {
        use crate::sketch2d::{Sketch2d, Sketch2dPrimitive};

        let outer = Sketch2d::Prim(Sketch2dPrimitive::Circle {
            center: [0.0, 0.0],
            radius: 10.0,
        });
        let inner = Sketch2d::Prim(Sketch2dPrimitive::Circle {
            center: [0.0, 0.0],
            radius: 4.0,
        });
        let washer = Sketch2d::Difference(Box::new(outer), Box::new(inner));
        let csg = CsgNode::Extrude { wall_subs: None,
            id: FeatureId::from_u32(1),
            sketch: Box::new(washer),
            half_height: 3.0,
        };
        let bvh = BoundedCsg::from_csg(&csg);
        let compiled = compile_body(&bvh);
        let points = [
            [0.0, 0.0, 0.0],
            [7.0, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [3.0, 0.0, 0.0],
            [0.0, 0.0, 3.0],
            [7.0, 0.0, 3.0],
        ];
        assert_sdf_match(&bvh, &compiled, &points, 1e-3);
    }

    /// #392: a circle drawn as two semicircular arcs has an interior seam on
    /// the y = 0 symmetry line. The co-circular run-merge (applied on both the
    /// CPU `path_is_inside` and the opcode-lowering `emit_path_record`) must
    /// keep the analytic BVH field and the compiled tape agreeing in sign for
    /// query points sitting exactly on that seam plane — the fragile
    /// double/zero-count case. Also checks the sign is correct (negative
    /// inside), not merely that the two mirrors agree on a wrong value.
    #[test]
    fn extruded_two_arc_circle_seam_plane_signs_match() {
        use crate::sketch2d::{Sketch2d, Sketch2dPrimitive};
        use std::f32::consts::PI;

        let sketch = Sketch2d::Prim(Sketch2dPrimitive::Path {
            segments: vec![
                SketchSegment::Arc { center: [0.0, 0.0], radius: 5.0, start_angle: 0.0, sweep_angle: PI },
                SketchSegment::Arc { center: [0.0, 0.0], radius: 5.0, start_angle: PI, sweep_angle: PI },
            ],
            closed: true,
        });
        let csg = CsgNode::Extrude {
            wall_subs: None,
            id: FeatureId::from_u32(1),
            sketch: Box::new(sketch),
            half_height: 4.0,
        };
        let bvh = BoundedCsg::from_csg(&csg);
        let compiled = compile_body(&bvh);

        // Points exactly on the sketch seam line y = 0 (interior and exterior),
        // at a few z inside the extrude.
        let seam_pts = [
            [0.0f32, 0.0, 0.0],
            [3.0, 0.0, 0.0],
            [-3.0, 0.0, 2.0],
            [4.5, 0.0, -2.0],
            [7.0, 0.0, 0.0],  // outside the disk
            [-8.0, 0.0, 1.0], // outside the disk
        ];
        assert_sdf_match(&bvh, &compiled, &seam_pts, 1e-3);

        // Sign must be correct, not just mutually consistent.
        for &(p, inside) in &[
            ([0.0f32, 0.0, 0.0], true),
            ([3.0, 0.0, 0.0], true),
            ([-4.5, 0.0, 0.0], true),
            ([7.0, 0.0, 0.0], false),
            ([-8.0, 0.0, 0.0], false),
        ] {
            let v = eval_opcodes(&compiled.opcodes, p);
            assert_eq!(v < 0.0, inside, "wrong sign at seam point {p:?}: {v}");
        }
    }

    #[test]
    fn revolved_rectangle() {
        use crate::sketch2d::{Sketch2d, Sketch2dPrimitive};

        let sketch = Sketch2d::Prim(Sketch2dPrimitive::Rectangle {
            center: [8.0, 0.0],
            half_extents: [2.0, 5.0],
        });
        let csg = CsgNode::Revolve { wall_subs: None,
            id: FeatureId::from_u32(1),
            sketch: Box::new(sketch),
        };
        let bvh = BoundedCsg::from_csg(&csg);
        let compiled = compile_body(&bvh);
        let points = [
            [8.0, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [6.0, 0.0, 0.0],
            [0.0, 0.0, 8.0],
            [0.0, 6.0, 0.0],
            [0.0, 0.0, 0.0],
        ];
        assert_sdf_match(&bvh, &compiled, &points, 1e-3);
    }

    #[test]
    fn axis_touching_revolve_tape_matches_field_and_analytic_truth() {
        // #390: rectangle u ∈ [0,30], v ∈ [−20,20] revolved around Y →
        // cylinder r=30, |y| ≤ 20. Built via `from_parts` like the
        // product, so the axis-strip invariant holds; field and tape
        // must agree AND carry no phantom zero-set on the axis.
        use crate::sketch2d::{Sketch2d, Sketch2dPrimitive};
        let feature = DecodedFeature::from_parts(
            FeatureId::from_u32(1),
            "revolve".to_string(),
            BoolOp::New,
            Primitive::Revolve {
                sketch: Sketch2d::Prim(Sketch2dPrimitive::Rectangle {
                    center: [15.0, 0.0],
                    half_extents: [15.0, 20.0],
                }),
                center: [0.0, 0.0, 0.0],
                wall_subs: None,
            },
            Transform::IDENTITY,
            PrimitiveModifiers::NONE,
            vec![],
        );
        let bvh = BoundedCsg::from_csg(&CsgNode::from_decoded(&[feature]));
        let compiled = compile_body(&bvh);
        let points = [
            [0.0, 0.0, 0.0],   // dead centre on the axis
            [0.0, 15.0, 0.0],  // on the axis near the top cap
            [0.5, 0.0, 0.0],   // just off the axis
            [15.0, 0.0, 0.0],  // mid-wall
            [30.0, 0.0, 0.0],  // outer surface
            [0.0, 25.0, 0.0],  // outside, above the cap on the axis
            [35.0, 0.0, 0.0],  // outside radially
        ];
        assert_sdf_match(&bvh, &compiled, &points, 1e-3);
        // Analytic truth on the axis: caps dominate (−20), the phantom
        // pre-fix value was ~0.
        let f0 = eval_opcodes(&compiled.opcodes, [0.0, 0.0, 0.0]);
        assert!((f0 - (-20.0)).abs() < 1e-3, "axis centre: {f0} != -20");
        let f1 = eval_opcodes(&compiled.opcodes, [0.0, 15.0, 0.0]);
        assert!((f1 - (-5.0)).abs() < 1e-3, "axis near cap: {f1} != -5");
    }

    #[test]
    fn mixed_scene_matches_bvh() {
        let features = vec![
            box_feature(1, BoolOp::New, [0.0, 0.0, 0.0], [20.0, 20.0, 20.0]),
            {
                let mut f = sphere_feature(2, [10.0, 10.0, 10.0], 8.0);
                f.op = BoolOp::Remove;
                f
            },
            box_feature(3, BoolOp::New, [-30.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
        ];
        let bvh = BoundedCsg::from_csg(&CsgNode::from_decoded(&features));
        let compiled = compile_body(&bvh);
        let points = [
            [0.0, 0.0, 0.0],
            [20.0, 0.0, 0.0],
            [10.0, 10.0, 10.0],
            [-30.0, 0.0, 0.0],
            [-60.0, 0.0, 0.0],
            [100.0, 100.0, 100.0],
            [15.0, 5.0, 5.0],
            [-4.0, -4.0, -4.0],
        ];
        assert_sdf_match(&bvh, &compiled, &points, 1e-3);
    }

    #[test]
    fn twist_warp() {
        let f = DecodedFeature::from_parts(
            FeatureId::from_u32(1),
            "Box 1".into(),
            BoolOp::New,
            Primitive::Box {
                center: [0.0, 0.0, 0.0],
                half_extents: [5.0, 20.0, 5.0],
            },
            Transform::IDENTITY,
            PrimitiveModifiers { twist_rate: 0.03, ..PrimitiveModifiers::NONE },
            vec![],
        );
        let features = vec![f];
        let bvh = BoundedCsg::from_csg(&CsgNode::from_decoded(&features));
        let compiled = compile_body(&bvh);
        let points = [
            [0.0, 0.0, 0.0],
            [5.0, 0.0, 0.0],
            [0.0, 10.0, 0.0],
            [3.0, 5.0, 3.0],
            [10.0, 10.0, 10.0],
        ];
        assert_sdf_match(&bvh, &compiled, &points, 1e-3);
    }

    #[test]
    fn shell_operator() {
        let f = DecodedFeature::from_parts(
            FeatureId::from_u32(1),
            "Sphere 1".into(),
            BoolOp::New,
            Primitive::Sphere {
                center: [0.0, 0.0, 0.0],
                radius: 10.0,
            },
            Transform::IDENTITY,
            PrimitiveModifiers { shell_thickness: 1.0, ..PrimitiveModifiers::NONE },
            vec![],
        );
        let features = vec![f];
        let bvh = BoundedCsg::from_csg(&CsgNode::from_decoded(&features));
        let compiled = compile_body(&bvh);
        let points = [
            [0.0, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [9.5, 0.0, 0.0],
            [15.0, 0.0, 0.0],
        ];
        assert_sdf_match(&bvh, &compiled, &points, 1e-3);
    }

    #[test]
    fn shell_operator_curved_opening() {
        // A sphere body shelled with its own spherical face opened: the curved
        // gate must compile identically to the BVH path (validates the per-kind
        // open-surface VM record + `open_gate_local`/`fd_open_gate` mirror).
        use fielddraft_protocol::{HalfSpace, Transform};
        let sphere = CsgNode::from_decoded(&[DecodedFeature::from_parts(
            FeatureId::from_u32(1),
            "Sphere 1".into(),
            BoolOp::New,
            Primitive::Sphere { center: [0.0, 0.0, 0.0], radius: 10.0 },
            Transform::IDENTITY,
            PrimitiveModifiers::NONE,
            vec![],
        )]);
        let shell = CsgNode::Shell(
            Box::new(sphere),
            1.0,
            vec![crate::csg::OpenSurface {
                branch: crate::csg::BranchId::from_feature_sub(
                    &FeatureId::from_u32(1),
                    0,
                )
                .as_u64(),
                surface: HalfSpace::Sphere { radius: 10.0 },
                placement: Transform::IDENTITY,
            }],
        );
        let bvh = BoundedCsg::from_csg(&shell);
        let compiled = compile_body(&bvh);
        let points = [
            [0.0, 0.0, 0.0],
            [9.5, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [0.0, 9.5, 0.0],
            [5.0, 5.0, 0.0],
        ];
        assert_sdf_match(&bvh, &compiled, &points, 1e-3);
    }

    #[test]
    fn empty_tree_compiles() {
        let bvh = BoundedCsg::from_csg(&CsgNode::from_decoded(&[]));
        let compiled = compile_body(&bvh);
        let d = eval_opcodes(&compiled.opcodes, [0.0, 0.0, 0.0]);
        assert!(d > 1e30);
    }

    #[test]
    fn compiled_opcode_count_is_reasonable() {
        let features = vec![
            box_feature(1, BoolOp::New, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
            sphere_feature(2, [0.0, 0.0, 0.0], 5.0),
        ];
        let bvh = BoundedCsg::from_csg(&CsgNode::from_decoded(&features));
        let compiled = compile_body(&bvh);
        // box: 1 tag + 13 payload = 14 (scale 1→3: +2)
        // sphere: 1 tag + 11 payload = 12 (scale 1→3: +2)
        // union: 1
        // end: 1
        // total = 28
        assert_eq!(compiled.opcode_count(), 28);
    }
}
