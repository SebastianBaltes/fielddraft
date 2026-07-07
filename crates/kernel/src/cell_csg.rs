//! Cell-local CSG specialization — the fidget-style "shortened tape".
//!
//! For a spatial query region (an octree cell) most branches of a large
//! assembly's CSG tree provably cannot influence the field: a union member
//! whose distance lower bound over the cell exceeds the rest's upper bound
//! never wins the `min`, a subtracted tool far from the cell never carves.
//! [`CellCsg::specialize`] rewrites the tree for one cell by dropping such
//! branches and returns conservative field bounds `[lo, hi]` over the cell
//! as a by-product. Children of the cell re-specialize from the parent's
//! (already shrunken) tree, so the per-sample cost inside a cell becomes
//! proportional to the *locally visible* complexity instead of the whole
//! assembly — the same amortisation `fidget` gets from interval-arithmetic
//! tape shortening, built here on `BoundedCsg`'s per-node AABBs and the
//! kernel's Lipschitz machinery instead of interval ops.
//!
//! ## Soundness
//!
//! Pruning preserves the field **value exactly** and the **contributor set
//! for any band `eps ≤ margin`** on the cell:
//!
//! * Bounds come from one centre evaluation per kept opaque subtree plus a
//!   Lipschitz ball: `f(p) ∈ [f(c) − L·r, f(c) + L·r]` with `r` the cell
//!   half-diagonal and `L = 1/relax` the body's Lipschitz allowance (the
//!   same `march_relax` assumption the octree's `Empty` bracket and the
//!   sphere tracer already rely on — Twist/Bend share the pre-existing
//!   blind spot documented at [`crate::sdf_compiler::march_relax`]).
//! * A branch is dropped only when its bound clears the keeper's bound by
//!   more than `margin (+ blend radius for smooth ops)`, so it can neither
//!   change the value nor enter a contributor band of width `≤ margin`.
//! * A far branch whose AABB gap to the cell already exceeds the keeper's
//!   upper bound is dropped **without evaluating it at all** (`f ≥
//!   dist(p, aabb)` outside the AABB — the same property
//!   [`BoundedCsg::sdf`]'s point pruning uses). This is what keeps
//!   specialization cheap on thousand-part assemblies.
//!
//! Nodes that remap the query point (Twist/Bend), shells with opened faces
//! (retained-boundary fields reach past their AABBs) and MaskedBlend are
//! treated as opaque: they are never rewritten inside, only dropped as a
//! whole when a parent rule allows it.
//!
//! The specialization is **mode-specific**: bounds are evaluated in the
//! [`EvalMode`] passed to [`SpecCell`], and the resulting tree must be
//! queried in that same mode.

use crate::bvh::BoundedCsg;
use crate::csg::{Aabb, BlendMode, Contributors, EvalMode};

/// Extra dominance a prune decision must have beyond the mathematical
/// bound — absorbs f32 rounding in the centre evals (units: mm).
const PRUNE_SLACK: f32 = 1e-4;

/// Query-region descriptor for one specialization pass.
#[derive(Debug, Clone, Copy)]
pub struct SpecCell {
    pub aabb: Aabb,
    /// Contributor bands queried against the specialized tree must use
    /// `eps ≤ margin`.
    pub margin: f32,
    /// The [`EvalMode`] the specialized tree will be queried in (bounds
    /// are evaluated in this mode).
    pub mode: EvalMode,
    /// `1 / march_relax(body)` — the Lipschitz allowance of the body's
    /// field (`≥ 1`).
    pub inv_relax: f32,
    center: [f32; 3],
    /// `half-diagonal · inv_relax` — the Lipschitz ball radius.
    ball: f32,
}

impl SpecCell {
    pub fn new(aabb: Aabb, margin: f32, mode: EvalMode, inv_relax: f32) -> Self {
        let size = aabb.size();
        let half_diag =
            0.5 * (size[0] * size[0] + size[1] * size[1] + size[2] * size[2]).sqrt();
        Self {
            aabb,
            margin,
            mode,
            inv_relax,
            center: [
                0.5 * (aabb.min[0] + aabb.max[0]),
                0.5 * (aabb.min[1] + aabb.max[1]),
                0.5 * (aabb.min[2] + aabb.max[2]),
            ],
            ball: half_diag * inv_relax,
        }
    }
}

/// A cell-specialized view of a [`BoundedCsg`]. `Full` marks a subtree
/// kept verbatim (evaluation delegates to the original node); the
/// combinator variants re-appear only where pruning happened somewhere
/// below them. Building views instead of cloned `BoundedCsg` trees keeps
/// specialization allocation-light (Extrude/Revolve leaves carry deep 2D
/// BVHs that must not be cloned per octree cell).
#[derive(Debug)]
pub enum CellCsg<'a> {
    Full(&'a BoundedCsg),
    Union {
        left: Box<CellCsg<'a>>,
        right: Box<CellCsg<'a>>,
    },
    Difference {
        left: Box<CellCsg<'a>>,
        right: Box<CellCsg<'a>>,
    },
    Intersection {
        left: Box<CellCsg<'a>>,
        right: Box<CellCsg<'a>>,
    },
    SmoothUnion {
        left: Box<CellCsg<'a>>,
        right: Box<CellCsg<'a>>,
        radius: f32,
        blend: BlendMode,
    },
    SmoothDifference {
        left: Box<CellCsg<'a>>,
        right: Box<CellCsg<'a>>,
        radius: f32,
        blend: BlendMode,
    },
    Morph {
        left: Box<CellCsg<'a>>,
        right: Box<CellCsg<'a>>,
        t: f32,
    },
    Displace {
        inner: Box<CellCsg<'a>>,
        depth: f32,
        freq: f32,
    },
    /// Only shells **without** opened faces descend (an open shell's
    /// retained field is evaluated under an open-branch set, which the
    /// prune bounds cannot see).
    Shell {
        inner: Box<CellCsg<'a>>,
        thickness: f32,
    },
    Offset {
        inner: Box<CellCsg<'a>>,
        delta: f32,
    },
}

/// A specialized tree plus its conservative field bounds over the cell.
#[derive(Debug)]
pub struct CellSpec<'a> {
    pub tree: CellCsg<'a>,
    pub lo: f32,
    pub hi: f32,
}

impl<'a> CellCsg<'a> {
    /// Wrap a full body tree, unspecialized.
    pub fn full(body: &'a BoundedCsg) -> Self {
        CellCsg::Full(body)
    }

    /// Specialize this (possibly already specialized) tree for `cell`.
    /// Sound to call repeatedly for nested cells with non-increasing
    /// margins.
    pub fn specialize(&self, cell: &SpecCell) -> CellSpec<'a> {
        match self {
            CellCsg::Full(node) => specialize_node(node, cell),
            CellCsg::Union { left, right } => {
                combine_union(left.specialize(cell), right.specialize(cell), cell, 0.0, None)
            }
            CellCsg::SmoothUnion { left, right, radius, blend } => combine_union(
                left.specialize(cell),
                right.specialize(cell),
                cell,
                *radius,
                Some(*blend),
            ),
            CellCsg::Difference { left, right } => {
                // No AABB on an overlay node — the eval-free gap drop
                // already ran on the first specialization pass.
                combine_difference(
                    left.specialize(cell),
                    0.0,
                    || right.specialize(cell),
                    cell,
                    0.0,
                    None,
                )
            }
            CellCsg::SmoothDifference { left, right, radius, blend } => combine_difference(
                left.specialize(cell),
                0.0,
                || right.specialize(cell),
                cell,
                *radius,
                Some(*blend),
            ),
            CellCsg::Intersection { left, right } => {
                combine_intersection(left.specialize(cell), right.specialize(cell), cell)
            }
            CellCsg::Morph { left, right, t } => {
                combine_morph(left.specialize(cell), right.specialize(cell), *t)
            }
            CellCsg::Displace { inner, depth, freq } => {
                combine_displace(inner.specialize(cell), *depth, *freq)
            }
            CellCsg::Shell { inner, thickness } => {
                combine_shell(inner.specialize(cell), *thickness)
            }
            CellCsg::Offset { inner, delta } => combine_offset(inner.specialize(cell), *delta),
        }
    }

    /// Signed distance on the specialized tree. Must be queried in the
    /// mode the tree was specialized for.
    pub fn sdf(&self, p: [f32; 3], mode: EvalMode) -> f32 {
        match self {
            CellCsg::Full(node) => node.sdf(p, mode),
            CellCsg::Union { left, right } => left.sdf(p, mode).min(right.sdf(p, mode)),
            CellCsg::Difference { left, right } => {
                left.sdf(p, mode).max(-right.sdf(p, mode))
            }
            CellCsg::Intersection { left, right } => {
                left.sdf(p, mode).max(right.sdf(p, mode))
            }
            CellCsg::SmoothUnion { left, right, radius, blend } => {
                let da = left.sdf(p, mode);
                let db = right.sdf(p, mode);
                match mode {
                    EvalMode::Visual => crate::csg::blend_min(da, db, *radius, *blend),
                    EvalMode::Logical => da.min(db),
                }
            }
            CellCsg::SmoothDifference { left, right, radius, blend } => {
                let da = left.sdf(p, mode);
                let db = right.sdf(p, mode);
                match mode {
                    EvalMode::Visual => -crate::csg::blend_min(-da, db, *radius, *blend),
                    EvalMode::Logical => da.max(-db),
                }
            }
            CellCsg::Morph { left, right, t } => {
                (1.0 - t) * left.sdf(p, mode) + t * right.sdf(p, mode)
            }
            CellCsg::Displace { inner, depth, freq } => {
                let d = inner.sdf(p, mode);
                let disp = *depth
                    * (p[0] * freq).sin()
                    * (p[1] * freq * 1.3).sin()
                    * (p[2] * freq * 0.7).sin();
                d - disp
            }
            CellCsg::Shell { inner, thickness } => {
                let d = inner.sdf(p, mode);
                crate::csg::shell_wall_sdf(d, d, *thickness)
            }
            CellCsg::Offset { inner, delta } => inner.sdf(p, mode) - delta,
        }
    }

    /// Provenance-aware evaluation on the specialized tree. Mirrors
    /// [`BoundedCsg::evaluate`] arm for arm; `eps` must be `≤` the margin
    /// the tree was specialized with, and `mode` must match.
    pub fn evaluate(&self, p: [f32; 3], eps: f32, mode: EvalMode) -> (f32, Contributors) {
        match self {
            CellCsg::Full(node) => node.evaluate(p, eps, mode),
            CellCsg::Union { left, right } => {
                let (da, ca) = left.evaluate(p, eps, mode);
                let (db, cb) = right.evaluate(p, eps, mode);
                crate::bvh::combine_min(da, ca, db, cb, eps)
            }
            CellCsg::Difference { left, right } => {
                let (da, ca) = left.evaluate(p, eps, mode);
                let (db, cb) = right.evaluate(p, eps, mode);
                crate::bvh::combine_max(da, ca, -db, cb, eps)
            }
            CellCsg::Intersection { left, right } => {
                let (da, ca) = left.evaluate(p, eps, mode);
                let (db, cb) = right.evaluate(p, eps, mode);
                crate::bvh::combine_max(da, ca, db, cb, eps)
            }
            CellCsg::SmoothUnion { left, right, radius, blend } => {
                let (da, ca) = left.evaluate(p, eps, mode);
                let (db, cb) = right.evaluate(p, eps, mode);
                match mode {
                    EvalMode::Visual => {
                        let d = crate::csg::blend_min(da, db, *radius, *blend);
                        let mut c = ca;
                        c.extend(cb);
                        (d, c)
                    }
                    EvalMode::Logical => crate::bvh::combine_min(da, ca, db, cb, eps),
                }
            }
            CellCsg::SmoothDifference { left, right, radius, blend } => {
                let (da, ca) = left.evaluate(p, eps, mode);
                let (db, cb) = right.evaluate(p, eps, mode);
                match mode {
                    EvalMode::Visual => {
                        let d = -crate::csg::blend_min(-da, db, *radius, *blend);
                        let mut c = ca;
                        c.extend(cb);
                        (d, c)
                    }
                    EvalMode::Logical => crate::bvh::combine_max(da, ca, -db, cb, eps),
                }
            }
            CellCsg::Morph { left, right, t } => {
                let (da, ca) = left.evaluate(p, eps, mode);
                let (db, cb) = right.evaluate(p, eps, mode);
                let d = (1.0 - t) * da + t * db;
                let mut c = ca;
                c.extend(cb);
                (d, c)
            }
            CellCsg::Displace { inner, depth, freq } => {
                let (d, c) = inner.evaluate(p, eps, mode);
                let disp = *depth
                    * (p[0] * freq).sin()
                    * (p[1] * freq * 1.3).sin()
                    * (p[2] * freq * 0.7).sin();
                (d - disp, c)
            }
            CellCsg::Shell { inner, thickness } => {
                // open_faces is empty by construction → d_ret = d, c_ret = c.
                let (d, c) = inner.evaluate(p, eps, mode);
                crate::bvh::combine_max(d, c, -(d + *thickness), c.mapped_inner(), eps)
            }
            CellCsg::Offset { inner, delta } => {
                let (d, c) = inner.evaluate(p, eps, mode);
                (d - delta, c)
            }
        }
    }
}

/// Minimum distance between `cell` and `aabb` (0 when they overlap,
/// `INFINITY` when `aabb` is the empty sentinel). Field values inside the
/// cell are `≥` this gap for any subtree bounded by `aabb`.
fn aabb_gap(cell: Aabb, aabb: Aabb) -> f32 {
    if aabb.is_empty() || cell.is_empty() {
        return f32::INFINITY;
    }
    let mut sum = 0.0f32;
    for i in 0..3 {
        let g = (aabb.min[i] - cell.max[i]).max(cell.min[i] - aabb.max[i]).max(0.0);
        sum += g * g;
    }
    sum.sqrt()
}

/// Specialize one original `BoundedCsg` node for `cell`.
fn specialize_node<'a>(node: &'a BoundedCsg, cell: &SpecCell) -> CellSpec<'a> {
    match node {
        BoundedCsg::Union { left, right, .. } => {
            spec_union_like(left, right, cell, 0.0, None)
        }
        BoundedCsg::SmoothUnion { left, right, radius, mode, .. } => {
            spec_union_like(left, right, cell, *radius, Some(*mode))
        }
        BoundedCsg::Difference { left, right, .. } => combine_difference(
            specialize_node(left, cell),
            aabb_gap(cell.aabb, right.aabb()),
            || specialize_node(right, cell),
            cell,
            0.0,
            None,
        ),
        BoundedCsg::SmoothDifference { left, right, radius, mode, .. } => combine_difference(
            specialize_node(left, cell),
            aabb_gap(cell.aabb, right.aabb()),
            || specialize_node(right, cell),
            cell,
            *radius,
            Some(*mode),
        ),
        BoundedCsg::Intersection { left, right, .. } => combine_intersection(
            specialize_node(left, cell),
            specialize_node(right, cell),
            cell,
        ),
        BoundedCsg::Morph { left, right, t, .. } => {
            combine_morph(specialize_node(left, cell), specialize_node(right, cell), *t)
        }
        BoundedCsg::Displace { inner, depth, freq, .. } => {
            combine_displace(specialize_node(inner, cell), *depth, *freq)
        }
        BoundedCsg::Shell { inner, thickness, open_faces, .. } if open_faces.is_empty() => {
            combine_shell(specialize_node(inner, cell), *thickness)
        }
        BoundedCsg::Offset { inner, delta, .. } => {
            combine_offset(specialize_node(inner, cell), *delta)
        }
        // Opaque: leaves, sketch carriers, warps, masked blends, open
        // shells, Empty. Bounds via one centre eval + Lipschitz ball,
        // sharpened by the AABB gap where the cell is outside.
        _ => opaque_bounds(node, cell),
    }
}

fn opaque_bounds<'a>(node: &'a BoundedCsg, cell: &SpecCell) -> CellSpec<'a> {
    if matches!(node, BoundedCsg::Empty) {
        return CellSpec { tree: CellCsg::Full(node), lo: f32::INFINITY, hi: f32::INFINITY };
    }
    let d_c = node.sdf(cell.center, cell.mode);
    let gap = aabb_gap(cell.aabb, node.aabb());
    let lo = (d_c - cell.ball).max(if gap > 0.0 { gap } else { f32::NEG_INFINITY });
    let hi = d_c + cell.ball;
    CellSpec { tree: CellCsg::Full(node), lo, hi }
}

/// Union / SmoothUnion over two ORIGINAL subtrees: try to drop the
/// AABB-farther side without evaluating it at all, else fall through to
/// the bound-based combine.
fn spec_union_like<'a>(
    left: &'a BoundedCsg,
    right: &'a BoundedCsg,
    cell: &SpecCell,
    radius: f32,
    blend: Option<BlendMode>,
) -> CellSpec<'a> {
    let gap_left = aabb_gap(cell.aabb, left.aabb());
    let gap_right = aabb_gap(cell.aabb, right.aabb());
    let (near, far, far_gap, near_is_left) = if gap_left <= gap_right {
        (left, right, gap_right, true)
    } else {
        (right, left, gap_left, false)
    };
    let blend_margin = blend_zone_margin(radius, blend, cell.mode);
    let no_prune = blend.is_some() && cell.mode == EvalMode::Visual;
    let s_near = specialize_node(near, cell);
    // Eval-free drop: the far side's field is ≥ its AABB gap everywhere
    // in the cell; if that already clears the near side's upper bound (+
    // margins), the far subtree is invisible here.
    if !no_prune && far_gap > s_near.hi + cell.margin + blend_margin + PRUNE_SLACK {
        return s_near;
    }
    let s_far = specialize_node(far, cell);
    let (s_left, s_right) =
        if near_is_left { (s_near, s_far) } else { (s_far, s_near) };
    combine_union(s_left, s_right, cell, radius, blend)
}

/// The blend-zone width a smooth op adds to the prune margin, and by
/// which its Visual-mode lower bound undershoots the hard min.
fn blend_zone_margin(radius: f32, blend: Option<BlendMode>, _mode: EvalMode) -> f32 {
    match blend {
        Some(_) => radius.max(0.0),
        None => 0.0,
    }
}

fn combine_union<'a>(
    s_left: CellSpec<'a>,
    s_right: CellSpec<'a>,
    cell: &SpecCell,
    radius: f32,
    blend: Option<BlendMode>,
) -> CellSpec<'a> {
    let blend_margin = blend_zone_margin(radius, blend, cell.mode);
    let margin = cell.margin + blend_margin + PRUNE_SLACK;
    // Visual-mode smooth ops merge BOTH sides' contributors
    // unconditionally (no eps gate — see the SmoothUnion arm of
    // [`BoundedCsg::evaluate`]), so a dropped branch would vanish from
    // the contributor set even when it cannot touch the value. Keep
    // both sides; their subtrees are still specialized.
    let no_prune = blend.is_some() && cell.mode == EvalMode::Visual;
    if !no_prune {
        if s_right.lo > s_left.hi + margin {
            return s_left;
        }
        if s_left.lo > s_right.hi + margin {
            return s_right;
        }
    }
    let lo_hard = s_left.lo.min(s_right.lo);
    let hi = s_left.hi.min(s_right.hi);
    // Visual smooth blends undershoot the hard min by ≤ blend_margin.
    let lo = match (blend, cell.mode) {
        (Some(_), EvalMode::Visual) => lo_hard - blend_margin,
        _ => lo_hard,
    };
    let tree = match blend {
        None => CellCsg::Union { left: Box::new(s_left.tree), right: Box::new(s_right.tree) },
        Some(b) => CellCsg::SmoothUnion {
            left: Box::new(s_left.tree),
            right: Box::new(s_right.tree),
            radius,
            blend: b,
        },
    };
    CellSpec { tree, lo, hi }
}

/// Difference / SmoothDifference. The right operand arrives as a lazy
/// closure plus its AABB gap so the eval-free drop can fire before any
/// work happens on it.
fn combine_difference<'a>(
    s_left: CellSpec<'a>,
    gap_right: f32,
    spec_right: impl FnOnce() -> CellSpec<'a>,
    cell: &SpecCell,
    radius: f32,
    blend: Option<BlendMode>,
) -> CellSpec<'a> {
    let blend_margin = blend_zone_margin(radius, blend, cell.mode);
    let margin = cell.margin + blend_margin + PRUNE_SLACK;
    // Visual-mode smooth ops merge contributors unconditionally — no
    // pruning there (see combine_union).
    let no_prune = blend.is_some() && cell.mode == EvalMode::Visual;
    // Eval-free drop: right ≥ gap on the cell ⇒ −right ≤ −gap. If
    // left's lower bound clears that by the margin, the tool cannot
    // carve here (nor enter the contributor band).
    if !no_prune && gap_right > 0.0 && s_left.lo + gap_right > margin {
        return s_left;
    }
    let s_right = spec_right();
    if !no_prune && s_left.lo + s_right.lo > margin {
        return s_left;
    }
    let lo_hard = s_left.lo.max(-s_right.hi);
    let hi = s_left.hi.max(-s_right.lo);
    let lo = match (blend, cell.mode) {
        (Some(_), EvalMode::Visual) => lo_hard - blend_margin,
        _ => lo_hard,
    };
    let tree = match blend {
        None => CellCsg::Difference {
            left: Box::new(s_left.tree),
            right: Box::new(s_right.tree),
        },
        Some(b) => CellCsg::SmoothDifference {
            left: Box::new(s_left.tree),
            right: Box::new(s_right.tree),
            radius,
            blend: b,
        },
    };
    CellSpec { tree, lo, hi }
}

fn combine_intersection<'a>(
    s_left: CellSpec<'a>,
    s_right: CellSpec<'a>,
    cell: &SpecCell,
) -> CellSpec<'a> {
    let margin = cell.margin + PRUNE_SLACK;
    // max(a, b): the side whose upper bound stays below the other's
    // lower bound (by the margin) never wins nor ties.
    if s_right.hi < s_left.lo - margin {
        return s_left;
    }
    if s_left.hi < s_right.lo - margin {
        return s_right;
    }
    CellSpec {
        tree: CellCsg::Intersection {
            left: Box::new(s_left.tree),
            right: Box::new(s_right.tree),
        },
        lo: s_left.lo.max(s_right.lo),
        hi: s_left.hi.max(s_right.hi),
    }
}

fn combine_morph<'a>(s_left: CellSpec<'a>, s_right: CellSpec<'a>, t: f32) -> CellSpec<'a> {
    // Robust for any t: bounds over all endpoint combinations.
    let combos = [
        (1.0 - t) * s_left.lo + t * s_right.lo,
        (1.0 - t) * s_left.lo + t * s_right.hi,
        (1.0 - t) * s_left.hi + t * s_right.lo,
        (1.0 - t) * s_left.hi + t * s_right.hi,
    ];
    let lo = combos.iter().copied().fold(f32::INFINITY, f32::min);
    let hi = combos.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    CellSpec {
        tree: CellCsg::Morph {
            left: Box::new(s_left.tree),
            right: Box::new(s_right.tree),
            t,
        },
        lo,
        hi,
    }
}

fn combine_displace<'a>(s_inner: CellSpec<'a>, depth: f32, freq: f32) -> CellSpec<'a> {
    let amp = depth.abs();
    CellSpec {
        lo: s_inner.lo - amp,
        hi: s_inner.hi + amp,
        tree: CellCsg::Displace { inner: Box::new(s_inner.tree), depth, freq },
    }
}

fn combine_shell<'a>(s_inner: CellSpec<'a>, thickness: f32) -> CellSpec<'a> {
    // wall(d) = max(d, −(d + t)) — decreasing to d = −t/2, then increasing.
    let wall = |d: f32| d.max(-(d + thickness));
    let (lo_d, hi_d) = (s_inner.lo, s_inner.hi);
    let lo = if lo_d <= -0.5 * thickness && -0.5 * thickness <= hi_d {
        -0.5 * thickness
    } else {
        wall(lo_d).min(wall(hi_d))
    };
    let hi = wall(lo_d).max(wall(hi_d));
    CellSpec {
        lo,
        hi,
        tree: CellCsg::Shell { inner: Box::new(s_inner.tree), thickness },
    }
}

fn combine_offset<'a>(s_inner: CellSpec<'a>, delta: f32) -> CellSpec<'a> {
    CellSpec {
        lo: s_inner.lo - delta,
        hi: s_inner.hi - delta,
        tree: CellCsg::Offset { inner: Box::new(s_inner.tree), delta },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::csg::CsgNode;
    use fielddraft_protocol::{FeatureId, Primitive};

    fn leaf(id: u32, primitive: Primitive) -> CsgNode {
        CsgNode::Leaf { id: FeatureId::from_u32(id), primitive }
    }

    fn sphere(id: u32, center: [f32; 3], radius: f32) -> CsgNode {
        leaf(id, Primitive::Sphere { center, radius })
    }

    fn boxp(id: u32, center: [f32; 3], half: [f32; 3]) -> CsgNode {
        leaf(id, Primitive::Box { center, half_extents: half })
    }

    /// Deterministic pseudo-random f32 in [0, 1) from integer coords.
    fn hash01(seed: u32) -> f32 {
        let mut x = seed.wrapping_mul(0x9E37_79B9).wrapping_add(0x85EB_CA6B);
        x ^= x >> 16;
        x = x.wrapping_mul(0x7FEB_352D);
        x ^= x >> 15;
        (x & 0xFFFFFF) as f32 / 16_777_216.0
    }

    /// A 3×3×3 grid of alternating boxes/spheres unioned together, a
    /// couple of subtracted tools, one smooth union and an offset — a
    /// small "assembly" exercising every prunable combinator.
    fn assembly() -> BoundedCsg {
        let mut node: Option<CsgNode> = None;
        let mut id = 1u32;
        for ix in 0..3 {
            for iy in 0..3 {
                for iz in 0..3 {
                    let c = [ix as f32 * 10.0, iy as f32 * 10.0, iz as f32 * 10.0];
                    let prim = if (ix + iy + iz) % 2 == 0 {
                        sphere(id, c, 3.0)
                    } else {
                        boxp(id, c, [2.5, 2.5, 2.5])
                    };
                    id += 1;
                    node = Some(match node {
                        None => prim,
                        Some(acc) => CsgNode::Union(Box::new(acc), Box::new(prim)),
                    });
                }
            }
        }
        let mut tree = node.unwrap();
        // Two subtracted tools.
        tree = CsgNode::Difference(
            Box::new(tree),
            Box::new(sphere(100, [10.0, 10.0, 10.0], 2.0)),
        );
        tree = CsgNode::Difference(
            Box::new(tree),
            Box::new(boxp(101, [20.0, 0.0, 0.0], [1.5, 1.5, 1.5])),
        );
        // One smooth union off to the side.
        tree = CsgNode::SmoothUnion(
            Box::new(tree),
            Box::new(sphere(102, [0.0, 20.0, 20.0], 4.0)),
            1.5,
            BlendMode::Fillet,
        );
        BoundedCsg::from_csg(&tree)
    }

    /// Specialized evaluation must be bit-identical to the full tree —
    /// values AND contributor sets — for every eps ≤ margin, across many
    /// cells and modes.
    #[test]
    fn specialized_eval_matches_full_tree() {
        let body = assembly();
        let root = body.aabb().expanded(2.0);
        let size = root.size();
        let n = 5; // 5×5×5 cells over the root box
        let mut checked = 0usize;
        for mode in [EvalMode::Logical, EvalMode::Visual] {
            for cx in 0..n {
                for cy in 0..n {
                    for cz in 0..n {
                        let min = [
                            root.min[0] + size[0] * cx as f32 / n as f32,
                            root.min[1] + size[1] * cy as f32 / n as f32,
                            root.min[2] + size[2] * cz as f32 / n as f32,
                        ];
                        let max = [
                            root.min[0] + size[0] * (cx + 1) as f32 / n as f32,
                            root.min[1] + size[1] * (cy + 1) as f32 / n as f32,
                            root.min[2] + size[2] * (cz + 1) as f32 / n as f32,
                        ];
                        let cell_aabb = Aabb { min, max };
                        let margin = 0.8;
                        let cell = SpecCell::new(cell_aabb, margin, mode, 1.0);
                        let spec = CellCsg::full(&body).specialize(&cell);
                        // Bounds sanity + equivalence at jittered points.
                        for i in 0..12u32 {
                            let seed = (cx * 25 + cy * 5 + cz) as u32 * 101 + i;
                            let p = [
                                min[0] + (max[0] - min[0]) * hash01(seed * 3),
                                min[1] + (max[1] - min[1]) * hash01(seed * 3 + 1),
                                min[2] + (max[2] - min[2]) * hash01(seed * 3 + 2),
                            ];
                            for eps in [0.0, 0.25, margin] {
                                let (d_full, c_full) = body.evaluate(p, eps, mode);
                                let (d_spec, c_spec) = spec.tree.evaluate(p, eps, mode);
                                assert_eq!(
                                    d_full.to_bits(),
                                    d_spec.to_bits(),
                                    "value mismatch at {p:?} eps={eps} mode={mode:?}"
                                );
                                assert_eq!(
                                    c_full.as_slice(),
                                    c_spec.as_slice(),
                                    "contributor mismatch at {p:?} eps={eps} mode={mode:?}"
                                );
                                assert!(
                                    d_full >= spec.lo - 1e-3 && d_full <= spec.hi + 1e-3,
                                    "bounds violated: {d_full} not in [{}, {}]",
                                    spec.lo,
                                    spec.hi
                                );
                                checked += 1;
                            }
                        }
                    }
                }
            }
        }
        assert!(checked > 5000, "test exercised too few points: {checked}");
    }

    /// Nested re-specialization (parent cell → child cell) must stay
    /// equivalent — the octree inheritance pattern.
    #[test]
    fn nested_specialization_matches_full_tree() {
        let body = assembly();
        let parent_aabb = Aabb { min: [-5.0, -5.0, -5.0], max: [15.0, 15.0, 15.0] };
        let parent = SpecCell::new(parent_aabb, 1.0, EvalMode::Logical, 1.0);
        let spec_parent = CellCsg::full(&body).specialize(&parent);
        // Child = one octant, smaller margin.
        let child_aabb = Aabb { min: [-5.0, -5.0, -5.0], max: [5.0, 5.0, 5.0] };
        let child = SpecCell::new(child_aabb, 0.5, EvalMode::Logical, 1.0);
        let spec_child = spec_parent.tree.specialize(&child);
        for i in 0..64u32 {
            let p = [
                -5.0 + 10.0 * hash01(i * 3),
                -5.0 + 10.0 * hash01(i * 3 + 1),
                -5.0 + 10.0 * hash01(i * 3 + 2),
            ];
            for eps in [0.0, 0.5] {
                let (d_full, c_full) = body.evaluate(p, eps, EvalMode::Logical);
                let (d_spec, c_spec) = spec_child.tree.evaluate(p, eps, EvalMode::Logical);
                assert_eq!(d_full.to_bits(), d_spec.to_bits(), "value mismatch at {p:?}");
                assert_eq!(c_full.as_slice(), c_spec.as_slice(), "contributors at {p:?}");
            }
        }
    }

    /// The eval-free far-branch drop must actually shrink the tree: a
    /// cell around one grid corner of the 27-part assembly keeps only a
    /// handful of leaves.
    #[test]
    fn far_branches_are_dropped() {
        let body = assembly();
        let cell_aabb = Aabb { min: [-4.0, -4.0, -4.0], max: [4.0, 4.0, 4.0] };
        let cell = SpecCell::new(cell_aabb, 0.5, EvalMode::Logical, 1.0);
        let spec = CellCsg::full(&body).specialize(&cell);

        fn count_leaves(t: &CellCsg<'_>) -> usize {
            match t {
                CellCsg::Full(n) => count_bounded(n),
                CellCsg::Union { left, right }
                | CellCsg::Difference { left, right }
                | CellCsg::Intersection { left, right }
                | CellCsg::SmoothUnion { left, right, .. }
                | CellCsg::SmoothDifference { left, right, .. }
                | CellCsg::Morph { left, right, .. } => {
                    count_leaves(left) + count_leaves(right)
                }
                CellCsg::Displace { inner, .. }
                | CellCsg::Shell { inner, .. }
                | CellCsg::Offset { inner, .. } => count_leaves(inner),
            }
        }
        fn count_bounded(n: &BoundedCsg) -> usize {
            match n {
                BoundedCsg::Union { left, right, .. }
                | BoundedCsg::Difference { left, right, .. }
                | BoundedCsg::Intersection { left, right, .. }
                | BoundedCsg::SmoothUnion { left, right, .. }
                | BoundedCsg::SmoothDifference { left, right, .. }
                | BoundedCsg::MaskedBlend { left, right, .. }
                | BoundedCsg::Morph { left, right, .. } => {
                    count_bounded(left) + count_bounded(right)
                }
                BoundedCsg::Twist { inner, .. }
                | BoundedCsg::Bend { inner, .. }
                | BoundedCsg::Displace { inner, .. }
                | BoundedCsg::Shell { inner, .. }
                | BoundedCsg::Offset { inner, .. } => count_bounded(inner),
                BoundedCsg::Empty => 0,
                _ => 1,
            }
        }

        let full = count_bounded(&body);
        let kept = count_leaves(&spec.tree);
        assert_eq!(full, 30, "assembly should have 30 leaves, got {full}");
        assert!(
            kept <= 10,
            "cell near one corner should keep ≤ 10 of 30 leaves, kept {kept}"
        );
    }
}
