//! Sketch2d data types + math.
//!
//! K7.3 moves the data types `Aabb2`, `Sketch2dPrimitive` and
//! `Sketch2d` (plus their pure-math impls for `sdf` and `aabb`)
//! from `fielddraft-kernel` into the protocol crate so that
//! `Primitive::Extrude` can carry a sketch profile directly in
//! the wire form. The kernel keeps the BVH-wrapped
//! `BoundedSketch2d` — that's a runtime acceleration structure,
//! not part of the persisted shape.
//!
//! Polygon SDF uses Inigo Quilez's even-odd-ray-cast formulation
//! from <https://iquilezles.org/articles/distfunctions2d/>. It's
//! exact for arbitrary simple polygons (convex or concave, as
//! long as edges don't cross) and handles both signs.

use serde::{Deserialize, Serialize};

/// 2D axis-aligned bounding box. `EMPTY` carries inverted
/// extents so `union(empty, x) == x` is a free no-op.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Aabb2 {
    pub min: [f32; 2],
    pub max: [f32; 2],
}

impl Aabb2 {
    pub const EMPTY: Self = Self {
        min: [f32::INFINITY; 2],
        max: [f32::NEG_INFINITY; 2],
    };

    pub fn is_empty(&self) -> bool {
        self.min[0] > self.max[0] || self.min[1] > self.max[1]
    }

    pub fn union(a: Self, b: Self) -> Self {
        if a.is_empty() {
            return b;
        }
        if b.is_empty() {
            return a;
        }
        Self {
            min: [a.min[0].min(b.min[0]), a.min[1].min(b.min[1])],
            max: [a.max[0].max(b.max[0]), a.max[1].max(b.max[1])],
        }
    }

    /// Grow the box by `margin` on every side. Negative margins
    /// shrink; an over-shrunk box becomes empty.
    pub fn expanded(&self, margin: f32) -> Self {
        if self.is_empty() {
            return *self;
        }
        Self {
            min: [self.min[0] - margin, self.min[1] - margin],
            max: [self.max[0] + margin, self.max[1] + margin],
        }
    }

    pub fn size(&self) -> [f32; 2] {
        [self.max[0] - self.min[0], self.max[1] - self.min[1]]
    }
}

/// Distance from a point to the surface of a 2D AABB. Zero
/// inside, Euclidean distance outside — same Lipschitz-1 bound
/// semantics as the 3D `distance_point_to_aabb`.
pub fn distance_point_to_aabb2(p: [f32; 2], aabb: Aabb2) -> f32 {
    if aabb.is_empty() {
        return f32::INFINITY;
    }
    let dx = (aabb.min[0] - p[0]).max(0.0).max(p[0] - aabb.max[0]);
    let dy = (aabb.min[1] - p[1]).max(0.0).max(p[1] - aabb.max[1]);
    (dx * dx + dy * dy).sqrt()
}

/// Filled 2D primitive. Every variant is negative strictly inside
/// its region and positive outside, so they compose under the
/// usual boolean min/max rules.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Sketch2dPrimitive {
    /// Filled disk: `‖p − center‖ − radius`.
    Circle { center: [f32; 2], radius: f32 },
    /// Axis-aligned filled rectangle. `half_extents` is per-axis.
    Rectangle {
        center: [f32; 2],
        half_extents: [f32; 2],
    },
    /// Closed simple polygon. Winding is irrelevant — the
    /// even-odd crossing count recovers the interior sign. The
    /// polygon must have at least three vertices; fewer yields
    /// `+∞` so it degenerates to an empty region gracefully.
    Polygon { vertices: Vec<[f32; 2]> },
    /// Circular sector (pie wedge). `start_angle` and
    /// `sweep_angle` are in radians. The sector fills from
    /// `start_angle` over `sweep_angle` counter-clockwise.
    Sector {
        center: [f32; 2],
        radius: f32,
        start_angle: f32,
        sweep_angle: f32,
    },
    /// Filled contour built from analytical curve segments
    /// (lines, quadratic béziers, circular arcs). When `closed`,
    /// the segment chain is treated as a closed loop and the SDF
    /// is signed via the even-odd rule; otherwise it is the
    /// unsigned distance to the open curve.
    ///
    /// The distance *magnitude* is always the exact analytical
    /// minimum over all segments — no CPU flattening. Only the
    /// inside/outside *sign* of a closed path uses a polyline
    /// approximation (see [`PATH_FLATTEN_SAMPLES`]).
    Path {
        segments: Vec<SketchSegment>,
        closed: bool,
    },
}

/// Number of straight sub-segments each curved segment is
/// flattened into when recovering the inside/outside sign of a
/// closed [`Sketch2dPrimitive::Path`], and the count the GPU
/// opcode compiler uses to emit a curve-free `OP_PRIM2D_POLY`.
/// The distance *magnitude* on the CPU/B-Rep path always stays
/// the exact analytical value — this sampling only feeds the
/// even-odd sign test and the GPU renderer (DL-46 / DL-45).
pub const PATH_FLATTEN_SAMPLES: usize = 24;

/// Tangent-alignment threshold for grouping G1-continuous path
/// segments into a single boundary branch (cos ≈ 0.999 ≈ 2.5°).
pub const PATH_G1_COS_THRESHOLD: f32 = 0.999;

/// Maximum boundary groups that can tie within `eps` at one query
/// point. A sketch corner ties exactly two boundary groups; richer
/// ties (three branches at a point) are vanishingly rare in 2D, so the
/// headroom is generous while keeping [`SketchEdgeBand`] inline & `Copy`
/// on the hot evaluate path — no heap allocation per SDF sample.
pub const MAX_TIED_EDGES: usize = 6;

/// Signed distance plus every boundary group whose boundary lies within
/// `eps` of the closest one — the 2D analogue of the kernel's analytic
/// half-space contributor band (`expand_evaluate_leaf` / `combine_min`).
///
/// A sweep ([`crate::Primitive::Extrude`] / `Revolve`) lifts this band
/// into 3D contributors, so a *genuine* sweep edge (two adjoining sketch
/// boundary groups meeting at a corner, or a wall meeting a cap) reports
/// **both** colliding branches — exactly what the B-Rep both-branches
/// gate needs. The earlier hard `sdf_with_edge` argmin returned a single
/// group, which made every real sweep edge invisible to that gate
/// (DL-65). Inline + `Copy`; no heap on the evaluate hot path.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SketchEdgeBand {
    /// Signed distance to the sketch boundary (negative inside).
    pub distance: f32,
    groups: [u32; MAX_TIED_EDGES],
    len: u8,
}

impl SketchEdgeBand {
    /// A band carrying just a distance, no groups yet.
    pub fn new(distance: f32) -> Self {
        Self {
            distance,
            groups: [0; MAX_TIED_EDGES],
            len: 0,
        }
    }

    /// A band with a single boundary group.
    pub fn single(distance: f32, group: u32) -> Self {
        let mut b = Self::new(distance);
        b.push(group);
        b
    }

    /// The tied boundary groups (deduplicated, insertion order).
    pub fn groups(&self) -> &[u32] {
        &self.groups[..self.len as usize]
    }

    /// Insert `group` if not already present (drops past capacity — a
    /// sign of too large an `eps`, not real geometry).
    pub fn push(&mut self, group: u32) {
        if self.groups().contains(&group) {
            return;
        }
        if (self.len as usize) < MAX_TIED_EDGES {
            self.groups[self.len as usize] = group;
            self.len += 1;
        }
    }

    /// Shift every group id up by `by` — used to offset a boolean
    /// right-child's branch ids past the left child's `edge_count`,
    /// matching [`Sketch2d::sdf_with_edge`]'s global numbering. Public so
    /// the kernel's `BoundedSketch2d` mirror can reuse it.
    pub fn offset(mut self, by: u32) -> Self {
        for i in 0..self.len as usize {
            self.groups[i] += by;
        }
        self
    }

    /// Boolean-min fold of two boundary bands with the same `eps`-tie
    /// rule as the kernel's `combine_min`: within `eps` both sides are
    /// tied and their groups merge; otherwise the nearer side wins.
    pub fn combine_min(a: SketchEdgeBand, b: SketchEdgeBand, eps: f32) -> SketchEdgeBand {
        if (a.distance - b.distance).abs() <= eps {
            let mut out = SketchEdgeBand::new(a.distance.min(b.distance));
            for &g in a.groups() {
                out.push(g);
            }
            for &g in b.groups() {
                out.push(g);
            }
            out
        } else if a.distance < b.distance {
            a
        } else {
            b
        }
    }

    /// Boolean-max fold (intersection / difference), same tie rule as
    /// [`Self::combine_min`] but keeping the larger distance.
    pub fn combine_max(a: SketchEdgeBand, b: SketchEdgeBand, eps: f32) -> SketchEdgeBand {
        if (a.distance - b.distance).abs() <= eps {
            let mut out = SketchEdgeBand::new(a.distance.max(b.distance));
            for &g in a.groups() {
                out.push(g);
            }
            for &g in b.groups() {
                out.push(g);
            }
            out
        } else if a.distance > b.distance {
            a
        } else {
            b
        }
    }
}

impl Sketch2dPrimitive {
    pub fn sdf(&self, p: [f32; 2]) -> f32 {
        match self {
            Self::Circle { center, radius } => {
                let dx = p[0] - center[0];
                let dy = p[1] - center[1];
                (dx * dx + dy * dy).sqrt() - *radius
            }
            Self::Rectangle {
                center,
                half_extents,
            } => {
                let qx = (p[0] - center[0]).abs() - half_extents[0];
                let qy = (p[1] - center[1]).abs() - half_extents[1];
                let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
                let inside = qx.max(qy).min(0.0);
                outside + inside
            }
            Self::Polygon { vertices } => sdf_polygon(p, vertices),
            Self::Sector {
                center,
                radius,
                start_angle,
                sweep_angle,
            } => sdf_sector(p, *center, *radius, *start_angle, *sweep_angle),
            Self::Path { segments, closed } => sdf_path(p, segments, *closed),
        }
    }

    /// Number of distinct boundary branches this primitive
    /// contributes. For a [`Self::Path`] this is the number of
    /// G1-continuous segment groups (a smooth spline counts as
    /// one branch; each tangent discontinuity / corner splits).
    pub fn edge_count(&self) -> u32 {
        match self {
            Self::Circle { .. } => 1,
            Self::Rectangle { .. } => 4,
            Self::Polygon { vertices } => vertices.len() as u32,
            Self::Sector { .. } => 3,
            Self::Path { segments, .. } => {
                if segments.is_empty() {
                    0
                } else {
                    g1_group_segments(segments, PATH_G1_COS_THRESHOLD)
                        .iter()
                        .copied()
                        .max()
                        .map(|g| g + 1)
                        .unwrap_or(0)
                }
            }
        }
    }

    /// SDF value and index of the closest boundary branch. For a
    /// [`Self::Path`] the returned index is the *G1 group* of the
    /// analytically-closest segment, not the raw segment index.
    pub fn sdf_with_edge(&self, p: [f32; 2]) -> (f32, u32) {
        match self {
            Self::Circle { center, radius } => {
                let dx = p[0] - center[0];
                let dy = p[1] - center[1];
                ((dx * dx + dy * dy).sqrt() - *radius, 0)
            }
            Self::Rectangle {
                center,
                half_extents,
            } => {
                let qx = (p[0] - center[0]).abs() - half_extents[0];
                let qy = (p[1] - center[1]).abs() - half_extents[1];
                let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
                let inside = qx.max(qy).min(0.0);
                // Branch order: +x, -x, +y, -y. The dominant axis
                // (larger signed gap) picks the side; ties break to
                // x first, matching DL-32's X→Y axis order.
                let edge = if qx >= qy {
                    if p[0] >= center[0] { 0 } else { 1 }
                } else if p[1] >= center[1] {
                    2
                } else {
                    3
                };
                (outside + inside, edge)
            }
            Self::Polygon { vertices } => sdf_polygon_with_edge(p, vertices),
            Self::Sector {
                center,
                radius,
                start_angle,
                sweep_angle,
            } => {
                let d = sdf_sector(p, *center, *radius, *start_angle, *sweep_angle);
                let pieces =
                    sector_boundary_distances(p, *center, *radius, *start_angle, *sweep_angle);
                let mut best = 0u32;
                for g in 1..SECTOR_GROUPS {
                    if pieces[g] < pieces[best as usize] {
                        best = g as u32;
                    }
                }
                (d, best)
            }
            Self::Path { segments, closed } => {
                if segments.is_empty() {
                    return (f32::INFINITY, 0);
                }
                // Index of the analytically-closest segment. Ties
                // break to the lower segment index (DL-32 arena
                // order) so classification is deterministic on the
                // medial axis between two groups.
                let (best_d, best_i) = path_min_distance(p, segments);
                // G1 group of the winner, counted incrementally — no
                // per-eval `g1_group_segments` allocation.
                let mut group = 0u32;
                for i in 1..=best_i {
                    if !g1_continuous(&segments[i - 1], &segments[i], PATH_G1_COS_THRESHOLD) {
                        group += 1;
                    }
                }
                let signed = if *closed && path_is_inside(p, segments) {
                    -best_d
                } else {
                    best_d
                };
                (signed, group)
            }
        }
    }

    /// Signed distance plus **every** boundary group within `eps` of the
    /// closest one (the eps-banded sibling of [`Self::sdf_with_edge`]).
    /// At a sketch corner two adjoining boundary groups tie, so the band
    /// carries both; on a flat span only the one nearest group survives.
    /// This is what lets a sweep report both branches at a genuine edge.
    pub fn sdf_with_edges(&self, p: [f32; 2], eps: f32) -> SketchEdgeBand {
        match self {
            Self::Circle { .. } => SketchEdgeBand::single(self.sdf(p), 0),
            Self::Rectangle {
                center,
                half_extents,
            } => {
                let (cx, cy) = (center[0], center[1]);
                let (hx, hy) = (half_extents[0], half_extents[1]);
                // Side order matches `sdf_with_edge`: 0=+x, 1=−x, 2=+y, 3=−y.
                let sides = [
                    ([cx + hx, cy - hy], [cx + hx, cy + hy]),
                    ([cx - hx, cy - hy], [cx - hx, cy + hy]),
                    ([cx - hx, cy + hy], [cx + hx, cy + hy]),
                    ([cx - hx, cy - hy], [cx + hx, cy - hy]),
                ];
                let dists = sides.map(|(a, b)| dist_point_seg(p, a, b));
                let min_d = dists.iter().copied().fold(f32::INFINITY, f32::min);
                let mut band = SketchEdgeBand::new(self.sdf(p));
                for (i, &d) in dists.iter().enumerate() {
                    if (d - min_d).abs() <= eps {
                        band.push(i as u32);
                    }
                }
                band
            }
            Self::Polygon { vertices } => {
                let n = vertices.len();
                if n < 3 {
                    return SketchEdgeBand::new(f32::INFINITY);
                }
                // Edge j connects vertex j → i, matching `sdf_polygon_with_edge`.
                let mut min_d = f32::INFINITY;
                let mut j = n - 1;
                for i in 0..n {
                    min_d = min_d.min(dist_point_seg(p, vertices[j], vertices[i]));
                    j = i;
                }
                let mut band = SketchEdgeBand::new(sdf_polygon(p, vertices));
                let mut j = n - 1;
                for i in 0..n {
                    if (dist_point_seg(p, vertices[j], vertices[i]) - min_d).abs() <= eps {
                        band.push(j as u32);
                    }
                    j = i;
                }
                band
            }
            // Sector: arc (group 0) + the two radial flanks (1 = start,
            // 2 = end), matching `sdf_with_edge` and `edge_count() == 3`.
            // At the two arc ends the arc and one flank tie inside `eps`,
            // so a pie-wedge corner reports both branches — exactly what a
            // sweep needs to weave the corner edge.
            Self::Sector {
                center,
                radius,
                start_angle,
                sweep_angle,
            } => {
                let pieces =
                    sector_boundary_distances(p, *center, *radius, *start_angle, *sweep_angle);
                let min_d = pieces.iter().copied().fold(f32::INFINITY, f32::min);
                let mut band = SketchEdgeBand::new(self.sdf(p));
                for (g, d) in pieces.iter().enumerate() {
                    if d.is_finite() && d - min_d <= eps {
                        band.push(g as u32);
                    }
                }
                band
            }
            Self::Path { segments, closed } => {
                let n = segments.len();
                if n == 0 {
                    return SketchEdgeBand::new(f32::INFINITY);
                }
                // Single exact pass: store each segment's distance once (the
                // old code solved every closest-point three times per eval).
                // Segments whose lower bound cannot land within `eps` of the
                // running minimum are skipped and keep +∞ — they can never
                // enter the band.
                let mut stack = [f32::INFINITY; PATH_STACK_SEGMENTS];
                let mut heap: Vec<f32>;
                let dists: &mut [f32] = if n <= PATH_STACK_SEGMENTS {
                    &mut stack[..n]
                } else {
                    heap = vec![f32::INFINITY; n];
                    &mut heap[..]
                };
                let mut min_d = f32::INFINITY;
                for (i, seg) in segments.iter().enumerate() {
                    if seg.distance_lower_bound(p) > min_d + eps {
                        continue;
                    }
                    let d = seg.unsigned_distance(p);
                    dists[i] = d;
                    if d < min_d {
                        min_d = d;
                    }
                }
                let signed = if *closed && path_is_inside(p, segments) {
                    -min_d
                } else {
                    min_d
                };
                let mut band = SketchEdgeBand::new(signed);
                // G1 group ids derived incrementally — no per-eval allocation.
                let mut group = 0u32;
                for i in 0..n {
                    if i > 0
                        && !g1_continuous(&segments[i - 1], &segments[i], PATH_G1_COS_THRESHOLD)
                    {
                        group += 1;
                    }
                    if dists[i] - min_d <= eps {
                        band.push(group);
                    }
                }
                band
            }
        }
    }

    /// Retained-boundary field: the NEGATED unsigned distance to the
    /// non-excluded boundary groups — the 2D kernel of the retained-boundary
    /// shell erosion. Always ≤ 0: the field treats everything as "material
    /// extended through the openings", which keeps it CONTINUOUS across the
    /// opened face (a parity-based sign would jump right where the opening's
    /// rim edge lives, killing the edge tracer's activity band there). The
    /// shell wall `max(d, −(d_ret + t))` caps it with the true outer surface,
    /// so the zero set is unchanged. `is_excluded` receives global group ids;
    /// `base` is this primitive's first group id in the owning sketch tree
    /// (see [`Sketch2d::edge_count`]). Only a `Path` has removable groups;
    /// for every other primitive the single boundary IS the surface, so
    /// exclusion is ignored (the wall stays closed there).
    pub fn sdf_retained(
        &self,
        p: [f32; 2],
        base: u32,
        is_excluded: &mut dyn FnMut(u32) -> bool,
    ) -> f32 {
        match self {
            Self::Path { segments, .. } => {
                let mut any = false;
                let (dist, _) = path_retained_distance(p, segments, |g| {
                    let ex = is_excluded(base + g);
                    any |= ex;
                    ex
                });
                if !any {
                    return self.sdf(p);
                }
                -dist
            }
            _ => self.sdf(p),
        }
    }

    /// Eps-banded sibling of [`Self::sdf_retained`]: the negated unsigned
    /// distance plus the retained groups within `eps` of it (excluded groups
    /// never enter the band).
    pub fn sdf_with_edges_retained(
        &self,
        p: [f32; 2],
        eps: f32,
        base: u32,
        is_excluded: &mut dyn FnMut(u32) -> bool,
    ) -> SketchEdgeBand {
        match self {
            Self::Path { segments, .. } => {
                let n = segments.len();
                if n == 0 {
                    return SketchEdgeBand::new(f32::INFINITY);
                }
                let mut stack = [f32::INFINITY; PATH_STACK_SEGMENTS];
                let mut heap: Vec<f32>;
                let dists: &mut [f32] = if n <= PATH_STACK_SEGMENTS {
                    &mut stack[..n]
                } else {
                    heap = vec![f32::INFINITY; n];
                    &mut heap[..]
                };
                let mut min_d = f32::INFINITY;
                let mut group = 0u32;
                let mut any = false;
                for (i, seg) in segments.iter().enumerate() {
                    if i > 0
                        && !g1_continuous(&segments[i - 1], &segments[i], PATH_G1_COS_THRESHOLD)
                    {
                        group += 1;
                    }
                    if is_excluded(base + group) {
                        any = true;
                        continue;
                    }
                    if seg.distance_lower_bound(p) > min_d + eps {
                        continue;
                    }
                    let d = seg.unsigned_distance(p);
                    dists[i] = d;
                    if d < min_d {
                        min_d = d;
                    }
                }
                if !any {
                    return self.sdf_with_edges(p, eps).offset(base);
                }
                let mut band = SketchEdgeBand::new(-min_d);
                let mut group = 0u32;
                for i in 0..n {
                    if i > 0
                        && !g1_continuous(&segments[i - 1], &segments[i], PATH_G1_COS_THRESHOLD)
                    {
                        group += 1;
                    }
                    if dists[i].is_finite() && dists[i] - min_d <= eps {
                        band.push(base + group);
                    }
                }
                band
            }
            _ => self.sdf_with_edges(p, eps).offset(base),
        }
    }

    pub fn aabb(&self) -> Aabb2 {
        match self {
            Self::Circle { center, radius } => Aabb2 {
                min: [center[0] - radius, center[1] - radius],
                max: [center[0] + radius, center[1] + radius],
            },
            Self::Rectangle {
                center,
                half_extents,
            } => Aabb2 {
                min: [center[0] - half_extents[0], center[1] - half_extents[1]],
                max: [center[0] + half_extents[0], center[1] + half_extents[1]],
            },
            Self::Polygon { vertices } => {
                let mut bb = Aabb2::EMPTY;
                for v in vertices {
                    bb = Aabb2::union(
                        bb,
                        Aabb2 {
                            min: *v,
                            max: *v,
                        },
                    );
                }
                bb
            }
            Self::Sector {
                center, radius, ..
            } => {
                // Conservative: full circle AABB.
                Aabb2 {
                    min: [center[0] - radius, center[1] - radius],
                    max: [center[0] + radius, center[1] + radius],
                }
            }
            Self::Path { segments, .. } => {
                let mut bb = Aabb2::EMPTY;
                for seg in segments {
                    bb = Aabb2::union(bb, seg.aabb());
                }
                bb
            }
        }
    }
}

/// Signed distance to a [`Sketch2dPrimitive::Path`]. The distance
/// magnitude is the exact analytical minimum over all segments;
/// for a `closed` path the sign comes from the exact even-odd
/// crossing test [`path_is_inside`] — fully analytic, no sampling.
/// Public so the kernel's CPU opcode interpreter evaluates path
/// records through the very same code as the logical CSG SDF.
pub fn sdf_path(p: [f32; 2], segments: &[SketchSegment], closed: bool) -> f32 {
    if segments.is_empty() {
        return f32::INFINITY;
    }
    let (unsigned, _) = path_min_distance(p, segments);
    if !closed {
        return unsigned;
    }
    if path_is_inside(p, segments) {
        -unsigned
    } else {
        unsigned
    }
}

/// Exact minimum unsigned distance over the path's **retained** boundary:
/// segments whose G1 group (per [`g1_group_segments`], computed on the fly
/// without allocating) is NOT excluded. Returns the closest retained foot
/// point alongside the distance so dual-number callers can lift the field to
/// first order. Returns `(f32::INFINITY, p)` when every group is excluded.
///
/// This is the open-shell erosion kernel (retained-boundary semantics): the
/// shell's cavity is `sign(full path) · retained distance ≤ −t`, so the wall
/// keeps a constant thickness `t` up to the opened face instead of tapering
/// to a knife edge there. The *sign* deliberately stays with the full closed
/// path — see `CsgNode::Shell`.
pub fn path_retained_distance(
    p: [f32; 2],
    segments: &[SketchSegment],
    mut is_excluded: impl FnMut(u32) -> bool,
) -> (f32, [f32; 2]) {
    let mut best = f32::INFINITY;
    let mut best_cp = p;
    let mut group = 0u32;
    for (i, seg) in segments.iter().enumerate() {
        if i > 0 && !g1_continuous(&segments[i - 1], seg, PATH_G1_COS_THRESHOLD) {
            group += 1;
        }
        if is_excluded(group) {
            continue;
        }
        if seg.distance_lower_bound(p) >= best {
            continue;
        }
        let (cp, _) = seg.closest_point_and_tangent(p);
        let dx = p[0] - cp[0];
        let dy = p[1] - cp[1];
        let d = (dx * dx + dy * dy).sqrt();
        if d < best {
            best = d;
            best_cp = cp;
        }
    }
    (best, best_cp)
}

/// Exact minimum unsigned distance over all segments, plus the index of the
/// analytically-closest one (ties break to the lower index, DL-32). Segments
/// whose [`SketchSegment::distance_lower_bound`] cannot beat the running
/// minimum skip the exact closest-point solve. Public so the kernel's
/// dual-number mirror shares the same pruned argmin.
pub fn path_min_distance(p: [f32; 2], segments: &[SketchSegment]) -> (f32, usize) {
    let mut best = f32::INFINITY;
    let mut best_i = 0usize;
    for (i, seg) in segments.iter().enumerate() {
        if seg.distance_lower_bound(p) >= best {
            continue;
        }
        let d = seg.unsigned_distance(p);
        if d < best {
            best = d;
            best_i = i;
        }
    }
    (best, best_i)
}

/// Even-odd crossing test over the **exact** contour — no flattening. Each
/// segment is decomposed into y-monotone pieces (a quadratic Bézier splits at
/// its single y-extremum, an arc at the cardinal angles `π/2 + kπ`); on a
/// monotone piece the polygon half-open rule `(ya > py) != (yb > py)`
/// transfers verbatim and the crossing parameter is the unique root of
/// `y(t) = py`, solved in closed form. Segments whose conservative y-span
/// does not straddle `p.y` cannot toggle the parity and are skipped. Public
/// so the kernel's dual-number mirror and the GPU SDF-VM (which ports this
/// per-record) share the same sign test.
///
/// **Seam robustness (#392).** The half-open rule is only consistent at a
/// segment seam if *both* adjoining segments classify the shared vertex on the
/// same side of `p.y`. Two consecutive arcs of the SAME circle recompute their
/// shared seam endpoint from different angle representations (e.g. angle `0` on
/// one side, `2π` on the other), whose `sin` can round to opposite sides of a
/// symmetry line — the crossing was then double- or zero-counted, flipping the
/// sign of the 2D distance on a null line. [`merge_cocircular_runs`] fuses such
/// runs into one arc so the fragile interior seam vanishes; the crossing math
/// itself is untouched, so every other configuration is bit-for-bit unchanged.
pub fn path_is_inside(p: [f32; 2], segments: &[SketchSegment]) -> bool {
    let merged = merge_cocircular_runs(segments);
    let segments: &[SketchSegment] = merged.as_deref().unwrap_or(segments);
    let mut crossings = 0u32;
    for seg in segments {
        let (y_min, y_max) = seg.y_span_conservative();
        if p[1] < y_min || p[1] > y_max {
            continue;
        }
        crossings += match seg {
            SketchSegment::Line { start, end } => line_crossings(p, *start, *end),
            SketchSegment::QuadBezier { p0, p1, p2 } => quad_crossings(p, *p0, *p1, *p2),
            SketchSegment::Arc {
                center,
                radius,
                start_angle,
                sweep_angle,
            } => arc_crossings(p, *center, *radius, *start_angle, *sweep_angle),
        };
    }
    crossings % 2 == 1
}

/// Fuse maximal runs of consecutive arcs that lie on the SAME circle and sweep
/// the same direction into a single arc, so their shared (non-cardinal) seam
/// vanishes from the even-odd crossing test (#392). Wraps around a closed loop.
/// Returns `None` when nothing merges — the overwhelmingly common case — so the
/// hot SDF path pays only an O(n) scan and no allocation. A mirror of
/// `offset_fold::merge_cocircular_runs`; also applied at GPU/CGAL lowering
/// (`emit_path_record`) so every evaluator sees the same normalised contour.
pub fn merge_cocircular_runs(segments: &[SketchSegment]) -> Option<Vec<SketchSegment>> {
    let n = segments.len();
    if n < 2 {
        return None;
    }
    // Cheap pre-scan: is there ANY adjacent (incl. wrap-around) co-circular
    // same-direction arc pair? If not, skip the allocation entirely.
    let mergeable = |i: usize, j: usize| -> bool {
        matches!(
            (&segments[i], &segments[j]),
            (SketchSegment::Arc { .. }, SketchSegment::Arc { .. })
        ) && cocircular_same_dir(&segments[i], &segments[j])
    };
    if !(0..n).any(|i| mergeable(i, (i + 1) % n)) {
        return None;
    }
    // Build the fused contour. Walk once; extend the current arc while the next
    // segment is co-circular and same-direction with a combined |sweep| ≤ 2π.
    let mut out: Vec<SketchSegment> = Vec::with_capacity(n);
    for seg in segments {
        match (out.last_mut(), seg) {
            (
                Some(SketchSegment::Arc { center: ca, radius: ra, sweep_angle: wa, .. }),
                SketchSegment::Arc { center: cb, radius: rb, sweep_angle: wb, .. },
            ) if arcs_cocircular_same_dir(*ca, *ra, *wa, *cb, *rb, *wb) => {
                *wa = (*wa + *wb).clamp(-std::f32::consts::TAU, std::f32::consts::TAU);
            }
            _ => out.push(seg.clone()),
        }
    }
    // Wrap-around: if the last and first entries fused into one circle, fold the
    // tail arc's sweep into the head and drop the tail.
    if out.len() >= 2 {
        let last = out.len() - 1;
        if let (
            SketchSegment::Arc { center: cb, radius: rb, sweep_angle: wb, .. },
            SketchSegment::Arc { center: ca, radius: ra, sweep_angle: wa, .. },
        ) = (&out[last], &out[0])
        {
            if arcs_cocircular_same_dir(*ca, *ra, *wa, *cb, *rb, *wb) {
                let add = *wb;
                if let SketchSegment::Arc { start_angle: sa, sweep_angle: wa, .. } = &mut out[0] {
                    // The tail arc ENDS where the head STARTS, i.e. it spans
                    // [head_start − add, head_start]. Folding its sweep into the
                    // head must therefore also rotate the head's start back by
                    // `add`, or the fused arc covers the wrong angular range.
                    *sa -= add;
                    *wa = (*wa + add).clamp(-std::f32::consts::TAU, std::f32::consts::TAU);
                }
                out.pop();
            }
        }
    }
    (out.len() != n).then_some(out)
}

fn cocircular_same_dir(a: &SketchSegment, b: &SketchSegment) -> bool {
    match (a, b) {
        (
            SketchSegment::Arc { center: ca, radius: ra, sweep_angle: wa, .. },
            SketchSegment::Arc { center: cb, radius: rb, sweep_angle: wb, .. },
        ) => arcs_cocircular_same_dir(*ca, *ra, *wa, *cb, *rb, *wb),
        _ => false,
    }
}

fn arcs_cocircular_same_dir(
    ca: [f32; 2],
    ra: f32,
    wa: f32,
    cb: [f32; 2],
    rb: f32,
    wb: f32,
) -> bool {
    let tol = (ra.abs() * 1e-5).max(1e-6);
    let same_circle =
        (ca[0] - cb[0]).abs() <= tol && (ca[1] - cb[1]).abs() <= tol && (ra - rb).abs() <= tol;
    let same_dir = (wa >= 0.0) == (wb >= 0.0);
    same_circle && same_dir && (wa + wb).abs() <= std::f32::consts::TAU + 1e-4
}

/// +x-ray crossing count (0 or 1) of a straight edge — the classic polygon
/// half-open rule.
fn line_crossings(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> u32 {
    if (a[1] > p[1]) != (b[1] > p[1]) {
        let x_cross = a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
        if p[0] < x_cross {
            return 1;
        }
    }
    0
}

/// +x-ray crossing count of a quadratic Bézier, exact. The curve's y is a
/// parabola in `t` with at most one interior extremum; splitting there gives
/// y-monotone pieces on which the half-open endpoint rule applies and the
/// crossing root is unique.
fn quad_crossings(p: [f32; 2], p0: [f32; 2], p1: [f32; 2], p2: [f32; 2]) -> u32 {
    let ay = (p0[1] - 2.0 * p1[1] + p2[1]) as f64;
    let by = 2.0 * (p1[1] - p0[1]) as f64;
    let cy = p0[1] as f64;
    let py = p[1] as f64;

    let y_at = |t: f64| (ay * t + by) * t + cy;
    let x_at = |t: f64| {
        let ax = (p0[0] - 2.0 * p1[0] + p2[0]) as f64;
        let bx = 2.0 * (p1[0] - p0[0]) as f64;
        (ax * t + bx) * t + p0[0] as f64
    };

    // Piece boundaries: [0, t_extremum, 1] when the parabola's apex falls
    // strictly inside the parameter range, [0, 1] otherwise.
    let mut bounds = [0.0_f64, 1.0, 1.0];
    let mut nb = 2;
    if ay.abs() > 1e-12 {
        let t_ext = -by / (2.0 * ay);
        if t_ext > 0.0 && t_ext < 1.0 {
            bounds[1] = t_ext;
            bounds[2] = 1.0;
            nb = 3;
        }
    }

    let mut count = 0u32;
    for w in bounds[..nb].windows(2) {
        let (ta, tb) = (w[0], w[1]);
        // Exact endpoint y for the outer boundaries so chained segments see
        // bitwise-identical junction values (the half-open rule depends on it).
        let ya = if ta == 0.0 { p0[1] as f64 } else { y_at(ta) };
        let yb = if tb == 1.0 { p2[1] as f64 } else { y_at(tb) };
        if (ya > py) == (yb > py) {
            continue;
        }
        let t = monotone_quad_root(ay, by, cy - py, ta, tb);
        if (p[0] as f64) < x_at(t) {
            count += 1;
        }
    }
    count
}

/// The unique root of `ay·t² + by·t + c0 = 0` inside the y-monotone piece
/// `[ta, tb]` whose endpoint values straddle zero (callers guarantee this).
fn monotone_quad_root(ay: f64, by: f64, c0: f64, ta: f64, tb: f64) -> f64 {
    if ay.abs() < 1e-12 {
        return (-c0 / by).clamp(ta, tb);
    }
    let disc = (by * by - 4.0 * ay * c0).max(0.0);
    // Citardauq pairing avoids cancellation: q = −(b + sign(b)·√disc)/2,
    // roots are q/a and c/q.
    let q = -0.5 * (by + by.signum() * disc.sqrt());
    let r0 = q / ay;
    let r1 = if q.abs() > 1e-300 { c0 / q } else { r0 };
    let eps = 1e-9;
    if r0 >= ta - eps && r0 <= tb + eps {
        r0.clamp(ta, tb)
    } else {
        r1.clamp(ta, tb)
    }
}

/// +x-ray crossing count of a circular arc, exact. The swept angular
/// interval is split at the y-extrema `π/2 + kπ` into monotone pieces; the
/// sweep direction is irrelevant for parity, so the interval is walked in
/// ascending angle.
fn arc_crossings(p: [f32; 2], center: [f32; 2], radius: f32, start: f32, sweep: f32) -> u32 {
    let cx = center[0] as f64;
    let cy = center[1] as f64;
    let r = radius as f64;
    let py = p[1] as f64;
    let px = p[0] as f64;

    // Full circle: the two horizontal-line intersections, counted directly
    // (strict comparison ⇒ a tangent ray crosses zero times, matching the
    // monotone half-open rule).
    if (sweep as f64).abs() >= std::f64::consts::TAU {
        let dy = py - cy;
        if dy.abs() >= r {
            return 0;
        }
        let half = (r * r - dy * dy).sqrt();
        return u32::from(px < cx + half) + u32::from(px < cx - half);
    }

    let s0 = start as f64;
    let s1 = start as f64 + sweep as f64;
    let lo = s0.min(s1);
    let hi = s0.max(s1);
    let y_at = |th: f64| cy + r * th.sin();
    let x_at = |th: f64| cx + r * th.cos();

    let mut count = 0u32;
    let mut ta = lo;
    // First y-extremum angle strictly above `lo`.
    let mut k = ((lo - std::f64::consts::FRAC_PI_2) / std::f64::consts::PI).floor() + 1.0;
    loop {
        let bp = std::f64::consts::FRAC_PI_2 + k * std::f64::consts::PI;
        let tb = bp.min(hi);
        let ya = y_at(ta);
        let yb = y_at(tb);
        if (ya > py) != (yb > py) {
            // sin(θ*) = (py−cy)/r has two branches per period; the piece lies
            // between adjacent extrema, so its midpoint's cos sign picks one.
            let v = ((py - cy) / r).clamp(-1.0, 1.0);
            let base = v.asin();
            let mid = 0.5 * (ta + tb);
            let mut th = if mid.cos() >= 0.0 {
                base
            } else {
                std::f64::consts::PI - base
            };
            th += ((ta - th) / std::f64::consts::TAU).ceil() * std::f64::consts::TAU;
            if px < x_at(th) {
                count += 1;
            }
        }
        if tb >= hi {
            break;
        }
        ta = tb;
        k += 1.0;
    }
    count
}

/// Split a segment into y-monotone pieces: a quadratic Bézier at its single
/// y-apex (exact de Casteljau subdivision), an arc at the cardinal angles
/// `π/2 + kπ` it sweeps across; lines are already monotone. The union of the
/// pieces is the original curve, so distance is unaffected. The GPU SDF-VM
/// stores path records pre-split this way, so its per-record crossing test
/// is the plain half-open endpoint rule with one closed-form root — no inner
/// subdivision loop in the shader.
pub fn split_y_monotone(seg: &SketchSegment) -> Vec<SketchSegment> {
    match seg {
        SketchSegment::Line { .. } => vec![*seg],
        SketchSegment::QuadBezier { p0, p1, p2 } => {
            let ay = (p0[1] - 2.0 * p1[1] + p2[1]) as f64;
            if ay.abs() < 1e-12 {
                return vec![*seg];
            }
            let t = (p0[1] - p1[1]) as f64 / ay;
            if !(1e-6..=1.0 - 1e-6).contains(&t) {
                return vec![*seg];
            }
            // De Casteljau at t — exact subdivision in f64.
            let lerp = |a: [f32; 2], b: [f32; 2]| {
                [
                    (a[0] as f64 + (b[0] as f64 - a[0] as f64) * t) as f32,
                    (a[1] as f64 + (b[1] as f64 - a[1] as f64) * t) as f32,
                ]
            };
            let q0 = lerp(*p0, *p1);
            let q1 = lerp(*p1, *p2);
            let m = lerp(q0, q1);
            vec![
                SketchSegment::QuadBezier { p0: *p0, p1: q0, p2: m },
                SketchSegment::QuadBezier { p0: m, p1: q1, p2: *p2 },
            ]
        }
        SketchSegment::Arc {
            center,
            radius,
            start_angle,
            sweep_angle,
        } => {
            // Walk the swept interval in ascending angle (parity and distance
            // are direction-free) and cut at every y-extremum strictly inside.
            let s0 = *start_angle as f64;
            let s1 = s0 + *sweep_angle as f64;
            let lo = s0.min(s1);
            let hi = s0.max(s1).min(lo + std::f64::consts::TAU);
            let mut pieces = Vec::new();
            let mut ta = lo;
            let mut k =
                ((lo - std::f64::consts::FRAC_PI_2) / std::f64::consts::PI).floor() + 1.0;
            loop {
                let bp = std::f64::consts::FRAC_PI_2 + k * std::f64::consts::PI;
                let tb = bp.min(hi);
                if tb - ta > 1e-9 {
                    pieces.push(SketchSegment::Arc {
                        center: *center,
                        radius: *radius,
                        start_angle: ta as f32,
                        sweep_angle: (tb - ta) as f32,
                    });
                }
                if tb >= hi {
                    break;
                }
                ta = tb;
                k += 1.0;
            }
            pieces
        }
    }
}

/// Do `prev` and `cur` sweep into the SAME carrier surface? A boundary group
/// becomes ONE B-Rep face, and a face has one surface: a straight segment
/// sweeps to a plane, a circular arc to a cylinder, a Bézier to a free-form
/// carrier. Tangency alone does not make two segments one face — a line
/// tangent to a fillet arc is a plane meeting a cylinder.
///
/// Same-kind pairs still fuse (that is what the tangency test is for):
/// collinear lines share a plane, co-circular arcs share a cylinder, and a
/// Bézier chain is one free-form carrier. Arcs must additionally agree on
/// their circle — two tangent arcs of different radius are two cylinders.
/// (`lift_path_group` re-verifies both conditions before it lifts an
/// analytic carrier, so this predicate only has to avoid fusing what could
/// never lift.)
fn same_carrier(prev: &SketchSegment, cur: &SketchSegment) -> bool {
    match (prev, cur) {
        (SketchSegment::Line { .. }, SketchSegment::Line { .. }) => true,
        (SketchSegment::QuadBezier { .. }, SketchSegment::QuadBezier { .. }) => true,
        (
            SketchSegment::Arc { center: ca, radius: ra, .. },
            SketchSegment::Arc { center: cb, radius: rb, .. },
        ) => {
            let tol = (ra.abs() * 1e-5).max(1e-6);
            (ca[0] - cb[0]).abs() <= tol
                && (ca[1] - cb[1]).abs() <= tol
                && (ra - rb).abs() <= tol
        }
        _ => false,
    }
}

/// Are `prev → cur` tangent at their shared endpoint (shared point, aligned
/// tangents)? The geometric half of [`g1_continuous`], without the carrier
/// test — so a TANGENT carrier change (line → fillet arc) still answers
/// `true`. That is exactly the group boundary whose swept seam is a
/// tangential one, which no SSI can seed (a double root has no sign change)
/// and which the weave therefore has to construct; see
/// [`tangential_group_breaks`].
pub fn tangent_at_joint(prev: &SketchSegment, cur: &SketchSegment, cos_threshold: f32) -> bool {
    let prev_end = prev.end_point();
    let cur_start = cur.start_point();
    let dx = cur_start[0] - prev_end[0];
    let dy = cur_start[1] - prev_end[1];
    if dx * dx + dy * dy >= 1e-6 {
        return false;
    }
    let t_prev = prev.tangent_at_end();
    let t_cur = cur.tangent_at_start();
    t_prev[0] * t_cur[0] + t_prev[1] * t_cur[1] > cos_threshold
}

/// Are `prev → cur` G1-continuous (tangent) AND carried by the same swept
/// surface? The pairwise predicate behind [`g1_group_segments`], usable
/// incrementally so the SDF hot path can derive group ids without allocating
/// the full vec.
fn g1_continuous(prev: &SketchSegment, cur: &SketchSegment, cos_threshold: f32) -> bool {
    tangent_at_joint(prev, cur, cos_threshold) && same_carrier(prev, cur)
}

/// The joints of a CLOSED boundary chain where the group changes *smoothly* —
/// tangent, but a different carrier surface (line → arc, arc → line, …).
/// Returns `(segment index i, group of i−1, group of i, joint point)`: the
/// swept image of that point is the seam between the two wall faces, and it
/// is the one seam an SSI-based weave provably cannot find. A CREASE joint
/// (non-tangent) is absent — those seams intersect transversally and seed
/// normally.
pub fn tangential_group_breaks(
    segments: &[SketchSegment],
    cos_threshold: f32,
) -> Vec<(usize, u32, u32, [f32; 2])> {
    let n = segments.len();
    if n < 2 {
        return Vec::new();
    }
    let groups = g1_group_segments(segments, cos_threshold);
    let mut out = Vec::new();
    for i in 0..n {
        let prev = (i + n - 1) % n;
        if groups[i] == groups[prev] {
            continue;
        }
        if tangent_at_joint(&segments[prev], &segments[i], cos_threshold) {
            out.push((i, groups[prev], groups[i], segments[i].start_point()));
        }
    }
    out
}

/// Distance from `p` to the axis-aligned box `[lo, hi]` (zero inside).
fn aabb2_distance(p: [f32; 2], lo: [f32; 2], hi: [f32; 2]) -> f32 {
    let dx = (lo[0] - p[0]).max(p[0] - hi[0]).max(0.0);
    let dy = (lo[1] - p[1]).max(p[1] - hi[1]).max(0.0);
    (dx * dx + dy * dy).sqrt()
}

/// Stack capacity for per-segment distance buffers on the SDF hot path —
/// real sketch paths are far smaller; larger ones fall back to a heap vec.
const PATH_STACK_SEGMENTS: usize = 64;

/// Signed distance to a simple polygon (Inigo Quilez, 2D
/// Sector (pie wedge) SDF. The sector spans from `start` over
/// `sweep` radians CCW. Inside the angular range the distance
/// is `|p−c| − r`; outside the angular range it's the distance
/// to the nearer radial edge.
fn sdf_sector(
    p: [f32; 2],
    center: [f32; 2],
    radius: f32,
    start: f32,
    sweep: f32,
) -> f32 {
    let dx = p[0] - center[0];
    let dy = p[1] - center[1];
    let dist = (dx * dx + dy * dy).sqrt();
    let angle = dy.atan2(dx);

    // Normalise angle relative to sector start.
    let half = sweep * 0.5;
    let mid = start + half;
    let mut rel = angle - mid;
    // Wrap to [-PI, PI].
    while rel > std::f32::consts::PI {
        rel -= std::f32::consts::TAU;
    }
    while rel < -std::f32::consts::PI {
        rel += std::f32::consts::TAU;
    }

    let d_circle = dist - radius;

    if rel.abs() <= half {
        // Inside the angular range — pure radial distance.
        d_circle
    } else {
        // Outside angular range — distance to the nearer
        // radial edge (a line from center at start or end).
        let edge_angle = if rel > 0.0 {
            start + sweep
        } else {
            start
        };
        let ex = edge_angle.cos();
        let ey = edge_angle.sin();
        // Project (dx, dy) onto the edge direction.
        let proj = dx * ex + dy * ey;
        let proj_clamped = proj.clamp(0.0, radius);
        let closest = [
            center[0] + ex * proj_clamped,
            center[1] + ey * proj_clamped,
        ];
        let ddx = p[0] - closest[0];
        let ddy = p[1] - closest[1];
        let d_edge = (ddx * ddx + ddy * ddy).sqrt();
        // Outside the sector: positive. Max with the radial
        // distance ensures the exterior is consistently > 0.
        d_edge.max(d_circle.max(0.0))
    }
}

/// Boundary branches a [`Sketch2dPrimitive::Sector`] contributes:
/// `0` = the arc, `1` = the radial flank at `start_angle`, `2` = the
/// radial flank at `start_angle + sweep_angle`. Matches
/// [`Sketch2dPrimitive::edge_count`], which has always reserved three
/// slots — only the *attribution* used to collapse them onto group 0.
pub const SECTOR_GROUPS: usize = 3;

/// A closed sector's sweep is a full turn to within this angle, i.e. the
/// two radial flanks coincide and lie strictly inside the disk — the
/// wedge degenerates to a plain circle with no flank branches.
const SECTOR_FULL_SWEEP_EPS: f32 = 1e-6;

/// Does `sweep` close the sector into a full disk? The SINGLE definition,
/// shared by the branch attribution here and the weaver's wall lift
/// (`brep::weaving::sketch_surfaces::collect_prim_walls`). If those two
/// ever disagreed at the boundary, one side would register a flank carrier
/// the other never reports as a branch — and a branch with a carrier but no
/// field (or the reverse) is exactly what bails a body to the octree.
pub fn sector_is_full_turn(sweep: f32) -> bool {
    sweep.abs() >= std::f32::consts::TAU - SECTOR_FULL_SWEEP_EPS
}

/// Is `sweep` too small to bound any area at all? Shared for the same
/// reason as [`sector_is_full_turn`].
pub fn sector_is_degenerate(radius: f32, sweep: f32) -> bool {
    !(radius > 0.0) || sweep.abs() < 1e-6
}

/// Unsigned distance from `p` to each of a sector's three boundary
/// pieces, in [`SECTOR_GROUPS`] order (arc, start flank, end flank).
/// A piece that is not part of the boundary — both flanks of a
/// full-turn sector, every piece of a degenerate (r ≤ 0) sector —
/// yields `f32::INFINITY`, so an `argmin` over the result never names it.
///
/// Purely analytic: point-to-arc is the radial projection with an
/// endpoint fallback outside the angular span, point-to-flank is the
/// ordinary point-to-segment distance. No sampling, no tolerance on the
/// geometry itself (`SECTOR_FULL_SWEEP_EPS` only classifies the
/// full-turn degeneracy).
pub fn sector_boundary_distances(
    p: [f32; 2],
    center: [f32; 2],
    radius: f32,
    start: f32,
    sweep: f32,
) -> [f32; SECTOR_GROUPS] {
    if sector_is_degenerate(radius, sweep) {
        return [f32::INFINITY; SECTOR_GROUPS];
    }
    let dx = p[0] - center[0];
    let dy = p[1] - center[1];
    let dist = (dx * dx + dy * dy).sqrt();

    // ── group 0: the arc ────────────────────────────────────────────
    let half = sweep.abs() * 0.5;
    let mid = start + sweep * 0.5;
    let full_turn = sector_is_full_turn(sweep);
    let mut rel = dy.atan2(dx) - mid;
    while rel > std::f32::consts::PI {
        rel -= std::f32::consts::TAU;
    }
    while rel < -std::f32::consts::PI {
        rel += std::f32::consts::TAU;
    }
    let end = start + sweep;
    let arc_end_point = |a: f32| [center[0] + radius * a.cos(), center[1] + radius * a.sin()];
    let d_arc = if full_turn || rel.abs() <= half {
        // The radial projection lands ON the arc.
        (dist - radius).abs()
    } else {
        // Outside the angular span — the nearest arc point is an end.
        let da = dist_2d(p, arc_end_point(start));
        let db = dist_2d(p, arc_end_point(end));
        da.min(db)
    };
    if full_turn {
        return [d_arc, f32::INFINITY, f32::INFINITY];
    }

    // ── groups 1/2: the radial flanks, centre → rim ─────────────────
    let flank = |a: f32| {
        let (ex, ey) = (a.cos(), a.sin());
        let t = (dx * ex + dy * ey).clamp(0.0, radius);
        let cx = center[0] + ex * t;
        let cy = center[1] + ey * t;
        dist_2d(p, [cx, cy])
    };
    [d_arc, flank(start), flank(end)]
}

/// Plain 2D Euclidean distance (not squared — see `point_seg_dist2`
/// elsewhere for the squared convention).
fn dist_2d(a: [f32; 2], b: [f32; 2]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    (dx * dx + dy * dy).sqrt()
}

/// distance functions). Walks every edge, tracks the minimum
/// squared edge distance, and recovers the sign from an even-
/// odd crossing count. Fewer than three vertices yields `+∞`.
fn sdf_polygon(p: [f32; 2], v: &[[f32; 2]]) -> f32 {
    let n = v.len();
    if n < 3 {
        return f32::INFINITY;
    }
    let mut d = {
        let dx = p[0] - v[0][0];
        let dy = p[1] - v[0][1];
        dx * dx + dy * dy
    };
    let mut s = 1.0_f32;
    let mut j = n - 1;
    for i in 0..n {
        let ex = v[j][0] - v[i][0];
        let ey = v[j][1] - v[i][1];
        let wx = p[0] - v[i][0];
        let wy = p[1] - v[i][1];
        let ee = ex * ex + ey * ey;
        let t = if ee > 0.0 {
            ((wx * ex + wy * ey) / ee).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let bx = wx - ex * t;
        let by = wy - ey * t;
        let d_candidate = bx * bx + by * by;
        if d_candidate < d {
            d = d_candidate;
        }
        let c1 = p[1] >= v[i][1];
        let c2 = p[1] < v[j][1];
        let c3 = ex * wy > ey * wx;
        if (c1 && c2 && c3) || (!c1 && !c2 && !c3) {
            s = -s;
        }
        j = i;
    }
    s * d.sqrt()
}

/// Polygon SDF with closest-edge tracking. Same algorithm as
/// `sdf_polygon` but also returns the index of the edge that
/// produced the minimum distance (edge `j` connects vertex `j`
/// to vertex `i = (j+1) % n`). Ties keep the first (lower-index)
/// edge, matching DL-32's deterministic arena-order tie-break.
fn sdf_polygon_with_edge(p: [f32; 2], v: &[[f32; 2]]) -> (f32, u32) {
    let n = v.len();
    if n < 3 {
        return (f32::INFINITY, 0);
    }
    let mut d = {
        let dx = p[0] - v[0][0];
        let dy = p[1] - v[0][1];
        dx * dx + dy * dy
    };
    let mut best_edge = 0u32;
    let mut s = 1.0_f32;
    let mut j = n - 1;
    for i in 0..n {
        let ex = v[j][0] - v[i][0];
        let ey = v[j][1] - v[i][1];
        let wx = p[0] - v[i][0];
        let wy = p[1] - v[i][1];
        let ee = ex * ex + ey * ey;
        let t = if ee > 0.0 {
            ((wx * ex + wy * ey) / ee).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let bx = wx - ex * t;
        let by = wy - ey * t;
        let d_candidate = bx * bx + by * by;
        if d_candidate < d {
            d = d_candidate;
            best_edge = j as u32;
        }
        let c1 = p[1] >= v[i][1];
        let c2 = p[1] < v[j][1];
        let c3 = ex * wy > ey * wx;
        if (c1 && c2 && c3) || (!c1 && !c2 && !c3) {
            s = -s;
        }
        j = i;
    }
    (s * d.sqrt(), best_edge)
}

/// Logical 2D CSG tree. No cached AABBs, no flattened
/// transforms: those belong to the kernel's `BoundedSketch2d`
/// BVH mirror.
///
/// Serde uses the externally-tagged default shape rather than
/// the internally-tagged one on [`Sketch2dPrimitive`] because
/// the boolean variants are tuple-shaped, and `#[serde(tag =
/// "kind")]` only supports struct-form variants.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Sketch2d {
    Empty,
    Prim(Sketch2dPrimitive),
    Union(Box<Sketch2d>, Box<Sketch2d>),
    Difference(Box<Sketch2d>, Box<Sketch2d>),
    Intersection(Box<Sketch2d>, Box<Sketch2d>),
    /// Shift the zero-level-set of an inner sketch by a
    /// constant distance: positive `distance` grows the region
    /// outwards, negative erodes inwards. Equivalent to
    /// `inner.sdf(p) - distance`.
    Offset {
        inner: Box<Sketch2d>,
        distance: f32,
    },
}

impl Sketch2d {
    /// Naive signed distance — walks every branch. The kernel's
    /// BVH wrapper uses this as the ground truth it has to
    /// agree with.
    pub fn sdf(&self, p: [f32; 2]) -> f32 {
        match self {
            Self::Empty => f32::INFINITY,
            Self::Prim(prim) => prim.sdf(p),
            Self::Union(a, b) => a.sdf(p).min(b.sdf(p)),
            Self::Difference(a, b) => a.sdf(p).max(-b.sdf(p)),
            Self::Intersection(a, b) => a.sdf(p).max(b.sdf(p)),
            Self::Offset { inner, distance } => inner.sdf(p) - distance,
        }
    }

    /// World-space AABB over the whole tree. Same conservative
    /// rules as the 3D version: Union takes the hull, Difference
    /// keeps the left operand, Intersection keeps the left,
    /// Offset expands the inner by `distance`.
    pub fn aabb(&self) -> Aabb2 {
        match self {
            Self::Empty => Aabb2::EMPTY,
            Self::Prim(prim) => prim.aabb(),
            Self::Union(a, b) => Aabb2::union(a.aabb(), b.aabb()),
            Self::Difference(a, _) => a.aabb(),
            Self::Intersection(a, _) => a.aabb(),
            Self::Offset { inner, distance } => inner.aabb().expanded(*distance),
        }
    }

    /// Total number of boundary branches across the whole tree.
    /// Boolean children's branch ids are concatenated, so the
    /// right child's ids start after the left child's count.
    pub fn edge_count(&self) -> u32 {
        match self {
            Self::Empty => 0,
            Self::Prim(prim) => prim.edge_count(),
            Self::Union(a, b) | Self::Difference(a, b) | Self::Intersection(a, b) => {
                a.edge_count() + b.edge_count()
            }
            Self::Offset { inner, .. } => inner.edge_count(),
        }
    }

    /// SDF value and globally-unique branch index across the tree.
    /// Boolean children's branch indices are offset so the right
    /// child's branches start after the left child's
    /// [`Self::edge_count`]. On the medial axis (`da == db`) the
    /// tie breaks to the left child, matching DL-32's deterministic
    /// arena/index order.
    pub fn sdf_with_edge(&self, p: [f32; 2]) -> (f32, u32) {
        match self {
            Self::Empty => (f32::INFINITY, 0),
            Self::Prim(prim) => prim.sdf_with_edge(p),
            Self::Union(a, b) => {
                let (da, ea) = a.sdf_with_edge(p);
                let (db, eb) = b.sdf_with_edge(p);
                if da <= db {
                    (da, ea)
                } else {
                    (db, eb + a.edge_count())
                }
            }
            Self::Difference(a, b) => {
                let (da, ea) = a.sdf_with_edge(p);
                let (db, eb) = b.sdf_with_edge(p);
                let neg_db = -db;
                if da >= neg_db {
                    (da, ea)
                } else {
                    (neg_db, eb + a.edge_count())
                }
            }
            Self::Intersection(a, b) => {
                let (da, ea) = a.sdf_with_edge(p);
                let (db, eb) = b.sdf_with_edge(p);
                if da >= db {
                    (da, ea)
                } else {
                    (db, eb + a.edge_count())
                }
            }
            Self::Offset { inner, distance } => {
                let (d, e) = inner.sdf_with_edge(p);
                (d - distance, e)
            }
        }
    }

    /// Eps-banded sibling of [`Self::sdf_with_edge`]: the signed distance
    /// plus every boundary group within `eps` of the closest boundary,
    /// with the same global branch numbering (right-child ids offset past
    /// the left child's [`Self::edge_count`]). Boolean nodes fold with the
    /// `eps`-tie rule so a boundary crossing between two primitives also
    /// reports both branches — the 2D mirror of the kernel's analytic
    /// `combine_min`/`expand_evaluate_leaf` contributor band (DL-65).
    pub fn sdf_with_edges(&self, p: [f32; 2], eps: f32) -> SketchEdgeBand {
        match self {
            Self::Empty => SketchEdgeBand::new(f32::INFINITY),
            Self::Prim(prim) => prim.sdf_with_edges(p, eps),
            Self::Union(a, b) => {
                let ba = a.sdf_with_edges(p, eps);
                let bb = b.sdf_with_edges(p, eps).offset(a.edge_count());
                SketchEdgeBand::combine_min(ba, bb, eps)
            }
            Self::Difference(a, b) => {
                let ba = a.sdf_with_edges(p, eps);
                let mut bb = b.sdf_with_edges(p, eps).offset(a.edge_count());
                bb.distance = -bb.distance;
                SketchEdgeBand::combine_max(ba, bb, eps)
            }
            Self::Intersection(a, b) => {
                let ba = a.sdf_with_edges(p, eps);
                let bb = b.sdf_with_edges(p, eps).offset(a.edge_count());
                SketchEdgeBand::combine_max(ba, bb, eps)
            }
            Self::Offset { inner, distance } => {
                let mut band = inner.sdf_with_edges(p, eps);
                band.distance -= distance;
                band
            }
        }
    }

    /// [`Self::sdf`] with excluded boundary groups removed from the distance
    /// (sign stays with the full tree) — the retained-boundary shell erosion,
    /// threaded through the sketch booleans with the same global group
    /// numbering as [`Self::sdf_with_edges`].
    pub fn sdf_retained(&self, p: [f32; 2], is_excluded: &mut dyn FnMut(u32) -> bool) -> f32 {
        self.sdf_retained_at(p, 0, is_excluded)
    }

    fn sdf_retained_at(
        &self,
        p: [f32; 2],
        base: u32,
        is_excluded: &mut dyn FnMut(u32) -> bool,
    ) -> f32 {
        match self {
            Self::Empty => f32::INFINITY,
            Self::Prim(prim) => prim.sdf_retained(p, base, is_excluded),
            Self::Union(a, b) => a
                .sdf_retained_at(p, base, is_excluded)
                .min(b.sdf_retained_at(p, base + a.edge_count(), is_excluded)),
            Self::Difference(a, b) => a
                .sdf_retained_at(p, base, is_excluded)
                .max(-b.sdf_retained_at(p, base + a.edge_count(), is_excluded)),
            Self::Intersection(a, b) => a
                .sdf_retained_at(p, base, is_excluded)
                .max(b.sdf_retained_at(p, base + a.edge_count(), is_excluded)),
            Self::Offset { inner, distance } => {
                inner.sdf_retained_at(p, base, is_excluded) - distance
            }
        }
    }

    /// Eps-banded sibling of [`Self::sdf_retained`]; band ids are global
    /// (already offset), excluded groups never enter the band.
    pub fn sdf_with_edges_retained(
        &self,
        p: [f32; 2],
        eps: f32,
        is_excluded: &mut dyn FnMut(u32) -> bool,
    ) -> SketchEdgeBand {
        self.sdf_with_edges_retained_at(p, eps, 0, is_excluded)
    }

    fn sdf_with_edges_retained_at(
        &self,
        p: [f32; 2],
        eps: f32,
        base: u32,
        is_excluded: &mut dyn FnMut(u32) -> bool,
    ) -> SketchEdgeBand {
        match self {
            Self::Empty => SketchEdgeBand::new(f32::INFINITY),
            Self::Prim(prim) => prim.sdf_with_edges_retained(p, eps, base, is_excluded),
            Self::Union(a, b) => {
                let ba = a.sdf_with_edges_retained_at(p, eps, base, is_excluded);
                let bb =
                    b.sdf_with_edges_retained_at(p, eps, base + a.edge_count(), is_excluded);
                SketchEdgeBand::combine_min(ba, bb, eps)
            }
            Self::Difference(a, b) => {
                let ba = a.sdf_with_edges_retained_at(p, eps, base, is_excluded);
                let mut bb =
                    b.sdf_with_edges_retained_at(p, eps, base + a.edge_count(), is_excluded);
                bb.distance = -bb.distance;
                SketchEdgeBand::combine_max(ba, bb, eps)
            }
            Self::Intersection(a, b) => {
                let ba = a.sdf_with_edges_retained_at(p, eps, base, is_excluded);
                let bb =
                    b.sdf_with_edges_retained_at(p, eps, base + a.edge_count(), is_excluded);
                SketchEdgeBand::combine_max(ba, bb, eps)
            }
            Self::Offset { inner, distance } => {
                let mut band = inner.sdf_with_edges_retained_at(p, eps, base, is_excluded);
                band.distance -= distance;
                band
            }
        }
    }
}

// ── Sketch segments (analytical curves) ─────────────────────────────

/// A single open curve segment with an *analytical* unsigned
/// distance — no CPU flattening (DL-46, supervisor decision #1).
///
/// Three variants only: `Line` (exact), `QuadBezier` (exact via
/// the Cardano/Quilez depressed-cubic closest-point solver) and
/// `Arc` (a circular arc; exact). Cubic béziers are deliberately
/// absent: any cubic input upstream must be subdivided into
/// quadratics at construction time (no `CubicBezier` variant).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SketchSegment {
    /// Straight line from `start` to `end`.
    Line { start: [f32; 2], end: [f32; 2] },
    /// Quadratic Bézier: B(t) = (1−t)²p0 + 2(1−t)t·p1 + t²·p2.
    QuadBezier {
        p0: [f32; 2],
        p1: [f32; 2],
        p2: [f32; 2],
    },
    /// Circular arc centred at `center`, radius `radius`, spanning
    /// from `start_angle` over `sweep_angle` radians (CCW positive).
    Arc {
        center: [f32; 2],
        radius: f32,
        start_angle: f32,
        sweep_angle: f32,
    },
}

impl SketchSegment {
    /// Unsigned Euclidean distance from `p` to the nearest point
    /// on this curve segment. Always ≥ 0, zero on the curve.
    pub fn unsigned_distance(&self, p: [f32; 2]) -> f32 {
        let c = self.closest_point(p);
        let dx = p[0] - c[0];
        let dy = p[1] - c[1];
        (dx * dx + dy * dy).sqrt()
    }

    /// Closest point on the curve to `p`. **Analytic** for every
    /// variant — in particular `QuadBezier` solves the depressed
    /// cubic in closed form, so the parameter `t*` it returns is
    /// exact (no sampling). SA-2's dual block relies on this.
    pub fn closest_point(&self, p: [f32; 2]) -> [f32; 2] {
        match self {
            Self::Line { start, end } => closest_on_line_seg(p, *start, *end),
            Self::QuadBezier { p0, p1, p2 } => closest_on_quad_bezier(p, *p0, *p1, *p2),
            Self::Arc {
                center,
                radius,
                start_angle,
                sweep_angle,
            } => closest_on_arc(p, *center, *radius, *start_angle, *sweep_angle),
        }
    }

    /// Closest point on the curve to `p`, **plus the unit curve tangent there**.
    /// The tangent is exact (it comes from the curve's analytic derivative at the
    /// closest parameter), so its 90°-rotation is a clean surface normal even when
    /// `p` lies exactly on the curve — unlike `∇‖p−closest‖`, which is `0/0` on
    /// the curve. This is what lets a sketch-sweep wall present a *signed* field
    /// with a well-defined gradient on its own boundary (DL-77).
    pub fn closest_point_and_tangent(&self, p: [f32; 2]) -> ([f32; 2], [f32; 2]) {
        match self {
            Self::Line { start, end } => {
                (closest_on_line_seg(p, *start, *end), self.tangent_at_start())
            }
            Self::QuadBezier { p0, p1, p2 } => closest_quad_bezier_pt_tangent(p, *p0, *p1, *p2),
            Self::Arc {
                center,
                radius,
                start_angle,
                sweep_angle,
            } => {
                let c = closest_on_arc(p, *center, *radius, *start_angle, *sweep_angle);
                // Tangent = ±perp(radial), oriented by the sweep sign.
                let sign = if *sweep_angle >= 0.0 { 1.0 } else { -1.0 };
                let rx = c[0] - center[0];
                let ry = c[1] - center[1];
                (c, normalize([-sign * ry, sign * rx]))
            }
        }
    }

    /// Conservative bounding box containing the entire curve.
    pub fn aabb(&self) -> Aabb2 {
        match self {
            Self::Line { start, end } => Aabb2 {
                min: [start[0].min(end[0]), start[1].min(end[1])],
                max: [start[0].max(end[0]), start[1].max(end[1])],
            },
            Self::QuadBezier { p0, p1, p2 } => Aabb2 {
                min: [p0[0].min(p1[0]).min(p2[0]), p0[1].min(p1[1]).min(p2[1])],
                max: [p0[0].max(p1[0]).max(p2[0]), p0[1].max(p1[1]).max(p2[1])],
            },
            Self::Arc {
                center,
                radius,
                start_angle,
                sweep_angle,
            } => aabb_arc(*center, *radius, *start_angle, *sweep_angle),
        }
    }

    pub fn start_point(&self) -> [f32; 2] {
        match self {
            Self::Line { start, .. } => *start,
            Self::QuadBezier { p0, .. } => *p0,
            Self::Arc {
                center,
                radius,
                start_angle,
                ..
            } => arc_point(*center, *radius, *start_angle),
        }
    }

    pub fn end_point(&self) -> [f32; 2] {
        match self {
            Self::Line { end, .. } => *end,
            Self::QuadBezier { p2, .. } => *p2,
            Self::Arc {
                center,
                radius,
                start_angle,
                sweep_angle,
            } => arc_point(*center, *radius, *start_angle + *sweep_angle),
        }
    }

    /// The same curve traversed in the opposite direction:
    /// `reversed().start_point() == end_point()` and vice versa.
    /// Used by the loop-inference pass that chains free sketch
    /// segments into closed contours regardless of the direction
    /// each element was drawn in.
    pub fn reversed(&self) -> Self {
        match self {
            Self::Line { start, end } => Self::Line {
                start: *end,
                end: *start,
            },
            Self::QuadBezier { p0, p1, p2 } => Self::QuadBezier {
                p0: *p2,
                p1: *p1,
                p2: *p0,
            },
            Self::Arc {
                center,
                radius,
                start_angle,
                sweep_angle,
            } => Self::Arc {
                center: *center,
                radius: *radius,
                start_angle: *start_angle + *sweep_angle,
                sweep_angle: -*sweep_angle,
            },
        }
    }

    /// Position on the curve at parameter `t ∈ [0, 1]`. This is
    /// the actual curve point (distinct from `closest_point`),
    /// used to flatten a segment into a polyline.
    pub fn point_at(&self, t: f32) -> [f32; 2] {
        match self {
            Self::Line { start, end } => [
                start[0] + (end[0] - start[0]) * t,
                start[1] + (end[1] - start[1]) * t,
            ],
            Self::QuadBezier { p0, p1, p2 } => {
                let s = 1.0 - t;
                [
                    s * s * p0[0] + 2.0 * s * t * p1[0] + t * t * p2[0],
                    s * s * p0[1] + 2.0 * s * t * p1[1] + t * t * p2[1],
                ]
            }
            Self::Arc {
                center,
                radius,
                start_angle,
                sweep_angle,
            } => arc_point(*center, *radius, *start_angle + t * *sweep_angle),
        }
    }

    /// Cheap lower bound on [`Self::unsigned_distance`] (always ≤ the true
    /// value). Lets a multi-segment query skip the exact closest-point solve
    /// for segments that provably cannot beat the running minimum — the
    /// B-Rep octree fires millions of path SDF evals, and without this every
    /// one of them paid a full cubic solve per Bézier segment.
    pub fn distance_lower_bound(&self, p: [f32; 2]) -> f32 {
        match self {
            Self::Line { start, end } => aabb2_distance(
                p,
                [start[0].min(end[0]), start[1].min(end[1])],
                [start[0].max(end[0]), start[1].max(end[1])],
            ),
            // The curve lies inside its control-point hull, so the
            // control-point AABB distance is a valid lower bound.
            Self::QuadBezier { p0, p1, p2 } => aabb2_distance(
                p,
                [
                    p0[0].min(p1[0]).min(p2[0]),
                    p0[1].min(p1[1]).min(p2[1]),
                ],
                [
                    p0[0].max(p1[0]).max(p2[0]),
                    p0[1].max(p1[1]).max(p2[1]),
                ],
            ),
            // Distance to the full circle bounds the distance to any arc of it.
            Self::Arc { center, radius, .. } => {
                let dx = p[0] - center[0];
                let dy = p[1] - center[1];
                ((dx * dx + dy * dy).sqrt() - radius).abs()
            }
        }
    }

    /// Conservative `[y_min, y_max]` containing the whole curve (control-point
    /// hull for Béziers, full circle for arcs). Used to skip segments that
    /// cannot straddle a horizontal ray in the even-odd sign test.
    fn y_span_conservative(&self) -> (f32, f32) {
        match self {
            Self::Line { start, end } => (start[1].min(end[1]), start[1].max(end[1])),
            Self::QuadBezier { p0, p1, p2 } => (
                p0[1].min(p1[1]).min(p2[1]),
                p0[1].max(p1[1]).max(p2[1]),
            ),
            Self::Arc { center, radius, .. } => (center[1] - radius, center[1] + radius),
        }
    }

    /// Unit tangent at t=0 (start of the curve).
    pub fn tangent_at_start(&self) -> [f32; 2] {
        let raw = match self {
            Self::Line { start, end } => [end[0] - start[0], end[1] - start[1]],
            Self::QuadBezier { p0, p1, .. } => [p1[0] - p0[0], p1[1] - p0[1]],
            Self::Arc {
                start_angle,
                sweep_angle,
                ..
            } => {
                let sign = if *sweep_angle >= 0.0 { 1.0 } else { -1.0 };
                let (st, ct) = start_angle.sin_cos();
                [-sign * st, sign * ct]
            }
        };
        normalize(raw)
    }

    /// Exact unit tangent at parameter `t` (forward direction). Closed-form
    /// derivative per segment kind — central differencing here is NOT an
    /// option: adaptive spline lowering produces millimetre-short segments
    /// whose f32 point differences are pure noise at usable step sizes.
    pub fn tangent_at(&self, t: f32) -> [f32; 2] {
        let raw = match self {
            Self::Line { start, end } => [end[0] - start[0], end[1] - start[1]],
            Self::QuadBezier { p0, p1, p2 } => {
                // B'(t) = 2(1−t)(p1−p0) + 2t(p2−p1)
                let mt = 1.0 - t;
                [
                    2.0 * (mt * (p1[0] - p0[0]) + t * (p2[0] - p1[0])),
                    2.0 * (mt * (p1[1] - p0[1]) + t * (p2[1] - p1[1])),
                ]
            }
            Self::Arc {
                start_angle,
                sweep_angle,
                ..
            } => {
                let sign = if *sweep_angle >= 0.0 { 1.0 } else { -1.0 };
                let a = start_angle + t * sweep_angle;
                let (st, ct) = a.sin_cos();
                [-sign * st, sign * ct]
            }
        };
        normalize(raw)
    }

    /// Unit tangent at t=1 (end of the curve).
    pub fn tangent_at_end(&self) -> [f32; 2] {
        let raw = match self {
            Self::Line { start, end } => [end[0] - start[0], end[1] - start[1]],
            Self::QuadBezier { p1, p2, .. } => [p2[0] - p1[0], p2[1] - p1[1]],
            Self::Arc {
                start_angle,
                sweep_angle,
                ..
            } => {
                let sign = if *sweep_angle >= 0.0 { 1.0 } else { -1.0 };
                let (st, ct) = (start_angle + sweep_angle).sin_cos();
                [-sign * st, sign * ct]
            }
        };
        normalize(raw)
    }
}

fn normalize(raw: [f32; 2]) -> [f32; 2] {
    let len = (raw[0] * raw[0] + raw[1] * raw[1]).sqrt();
    if len < 1e-12 {
        [1.0, 0.0]
    } else {
        [raw[0] / len, raw[1] / len]
    }
}

/// G1-continuity grouping: consecutive segments that share an
/// endpoint with aligned tangents (dot product > `cos_threshold`)
/// receive the same group index. Returns one group id per input
/// segment. Segments in the same group share a BranchId sub-index,
/// so a smooth spline collapses to one B-Rep edge and only corners
/// (tangent discontinuities) split into separate ids.
pub fn g1_group_segments(segments: &[SketchSegment], cos_threshold: f32) -> Vec<u32> {
    let n = segments.len();
    if n == 0 {
        return vec![];
    }
    let mut group = vec![0u32; n];
    let mut current_group = 0u32;
    group[0] = current_group;
    for i in 1..n {
        if !g1_continuous(&segments[i - 1], &segments[i], cos_threshold) {
            current_group += 1;
        }
        group[i] = current_group;
    }
    group
}

/// Per-segment G1 group ids as a lazy iterator — the allocation-free sibling
/// of [`g1_group_segments`], for callers that walk segments once (the
/// kernel's dual-number retained-distance mirror).
pub fn segment_groups(segments: &[SketchSegment]) -> impl Iterator<Item = u32> + '_ {
    let mut group = 0u32;
    (0..segments.len()).map(move |i| {
        if i > 0 && !g1_continuous(&segments[i - 1], &segments[i], PATH_G1_COS_THRESHOLD) {
            group += 1;
        }
        group
    })
}

/// Signed distance (and its 2D gradient) from `p` to the **carrier** of one
/// G1 path group: the group's segments, extended past both chain ends along
/// their end tangents so the field stays smooth where the neighbouring
/// groups take over. Without the extension the signed distance flips
/// erratically beyond the chain ends (the closest point clamps to an
/// endpoint and the side classification degenerates) — exactly where the
/// edges to the adjacent groups live.
///
/// Sign convention: negative on the **material** side of the owning closed
/// path (`ccw` = its winding, material left of the traversal direction —
/// the same convention as `collect_sketch_walls`' outward normals). The
/// gradient is the outward unit normal at the foot point.
///
/// `segments` must be in path order (the order `g1_group_segments` sees).
/// For a group that wraps a closed path's seam the two extension rays sit
/// at the wrap joint instead of the geometric ends; distances *on* the
/// group are unaffected, only the carrier's far extension differs.
///
/// `extend_start`/`extend_end` select the carrier shape past each chain end:
/// `true` extends along the end tangent (smooth field across the shared
/// corner with the adjacent group → crisp miter edge), `false` keeps the
/// plain closest-point field whose offset level set rounds the endpoint with
/// an arc ("stadium") — the retained-boundary shell erosion surface where
/// the neighbouring group was opened.
pub fn group_signed_distance_and_normal(
    segments: &[SketchSegment],
    ccw: bool,
    p: [f32; 2],
    extend_start: bool,
    extend_end: bool,
) -> (f32, [f32; 2]) {
    debug_assert!(!segments.is_empty());
    let mut best_d2 = f32::INFINITY;
    let mut best_cp = p;
    let mut best_tan = [1.0f32, 0.0];

    macro_rules! consider {
        ($cp:expr, $tan:expr) => {{
            let cp = $cp;
            let dx = p[0] - cp[0];
            let dy = p[1] - cp[1];
            let d2 = dx * dx + dy * dy;
            if d2 < best_d2 {
                best_d2 = d2;
                best_cp = cp;
                best_tan = $tan;
            }
        }};
    }

    for s in segments {
        // Same lower-bound prune as `path_min_distance`: skip the exact
        // closest-point solve when the segment's conservative bound cannot
        // beat the running best (adaptive spline lowering emits dozens of
        // short segments; this is the hot call of the octree edge route).
        let lb = s.distance_lower_bound(p);
        if lb * lb >= best_d2 {
            continue;
        }
        let (cp, tan) = s.closest_point_and_tangent(p);
        consider!(cp, tan);
    }

    // Tangent-ray extensions. The traversal direction stays path-forward on
    // both rays (start ray runs BACKWARD from the first point), so the
    // outward-normal sign convention is seamless across the chain ends.
    if let (true, Some(first)) = (extend_start, segments.first()) {
        let a = first.start_point();
        let ta = first.tangent_at_start();
        let u = -((p[0] - a[0]) * ta[0] + (p[1] - a[1]) * ta[1]);
        if u > 0.0 {
            consider!([a[0] - u * ta[0], a[1] - u * ta[1]], ta);
        }
    }
    if let (true, Some(last)) = (extend_end, segments.last()) {
        let b = last.end_point();
        let tb = last.tangent_at_end();
        let v = (p[0] - b[0]) * tb[0] + (p[1] - b[1]) * tb[1];
        if v > 0.0 {
            consider!([b[0] + v * tb[0], b[1] + v * tb[1]], tb);
        }
    }

    // Outward normal from the foot-point tangent: rotate −90° for CCW
    // (material on the left), +90° for CW.
    let n_out = if ccw {
        [best_tan[1], -best_tan[0]]
    } else {
        [-best_tan[1], best_tan[0]]
    };
    let dist = best_d2.sqrt();
    // The per-segment closed-form foot points carry ~1e-5 absolute error
    // (f32 cubic solve); any closer than that the direction `p − cp` is pure
    // float noise, so the tangent-derived normal takes over — it is the
    // exact gradient ON the curve and first-order correct beside it. The
    // signed value is the projection onto that normal (≈ ±dist, exact for
    // true normal feet).
    let near = 1e-4_f32.max(1e-6 * (p[0].abs() + p[1].abs()));
    if dist < near {
        let d = (p[0] - best_cp[0]) * n_out[0] + (p[1] - best_cp[1]) * n_out[1];
        return (d, n_out);
    }
    let dir = [(p[0] - best_cp[0]) / dist, (p[1] - best_cp[1]) / dist];
    let sign = if dir[0] * n_out[0] + dir[1] * n_out[1] >= 0.0 {
        1.0
    } else {
        -1.0
    };
    (sign * dist, [sign * dir[0], sign * dir[1]])
}

// ── Ellipse decomposition (DL-49 Option B) ──────────────────────────

/// Decompose an ellipse — or an elliptic arc — into a chain of
/// tangent-continuous [`SketchSegment::QuadBezier`] segments
/// (DL-49 Nachtrag, Option B). Ellipses are first-class CAD
/// primitives but need not be *exact*: rather than an iterative
/// elliptic distance/dual solver, they ride the verified quadratic
/// Bézier machinery, so accuracy becomes a pure tessellation knob.
///
/// An ellipse is the affine image `M·u + center` of the unit
/// circle, with `M = R(rotation)·diag(radius_x, radius_y)`. Affine
/// maps send tangent lines to tangent lines and preserve their
/// intersection, so the quadratic control point of each sub-arc is
/// simply the affine image of the unit-circle tangent intersection
/// — exact (no sampling), and the resulting segments are G1 at every
/// join. `g1_group_segments` therefore collapses a full ellipse into
/// a single B-Rep edge.
///
/// `segments == 0` selects an automatic count from `tolerance_mm`
/// via the per-sub-arc sagitta bound ε ≈ R·Δ⁴/128 (R = larger
/// semi-axis). `start_angle`/`sweep_angle` are in radians in the
/// *unrotated* ellipse frame (CCW positive). A full sweep
/// (|sweep| ≥ 2π) yields a closed loop whose last endpoint coincides
/// with the first.
#[allow(clippy::too_many_arguments)]
pub fn ellipse_quad_beziers(
    center: [f32; 2],
    radius_x: f32,
    radius_y: f32,
    rotation: f32,
    start_angle: f32,
    sweep_angle: f32,
    segments: u32,
    tolerance_mm: f32,
) -> Vec<SketchSegment> {
    let rx = radius_x.abs();
    let ry = radius_y.abs();
    if rx <= 0.0 || ry <= 0.0 || sweep_angle.abs() < 1e-6 {
        return vec![];
    }

    // Pick the sub-arc count.
    let n = if segments > 0 {
        segments.max(1)
    } else {
        let r = rx.max(ry);
        let tol = tolerance_mm.max(1e-4);
        // ε ≈ R·Δ⁴/128 ⇒ Δ ≤ (128·tol/R)^¼
        let delta_max = (128.0 * tol / r).powf(0.25).clamp(1e-3, std::f32::consts::FRAC_PI_2);
        (sweep_angle.abs() / delta_max).ceil() as u32
    }
    .max(1);

    let (sin_phi, cos_phi) = rotation.sin_cos();
    // Affine image of a point on the unit circle.
    let map = |u: [f32; 2]| -> [f32; 2] {
        let lx = rx * u[0];
        let ly = ry * u[1];
        [
            center[0] + cos_phi * lx - sin_phi * ly,
            center[1] + sin_phi * lx + cos_phi * ly,
        ]
    };

    let step = sweep_angle / n as f32;
    let mut segs = Vec::with_capacity(n as usize);
    for i in 0..n {
        let a0 = start_angle + step * i as f32;
        let a1 = start_angle + step * (i + 1) as f32;
        let (s0, c0) = a0.sin_cos();
        let (s1, c1) = a1.sin_cos();
        // Unit-circle tangent-line intersection (control point):
        // solve X·u(a0)=1, X·u(a1)=1 ⇒ X = [s1−s0, c0−c1]/sin(a1−a0).
        let det = (a1 - a0).sin();
        let u1 = if det.abs() < 1e-9 {
            // Degenerate sub-arc: fall back to the chord midpoint.
            [0.5 * (c0 + c1), 0.5 * (s0 + s1)]
        } else {
            [(s1 - s0) / det, (c0 - c1) / det]
        };
        segs.push(SketchSegment::QuadBezier {
            p0: map([c0, s0]),
            p1: map(u1),
            p2: map([c1, s1]),
        });
    }

    // Full sweep: snap the seam exactly closed. `sin(TAU)` in f32 is
    // a few ×1e-7, not zero, so the last endpoint would land just off
    // the start — and an even-odd ray cast aligned with that seam
    // (e.g. the major axis of an axis-aligned ellipse) then double-
    // counts the crossing and flips the inside/outside sign. Forcing
    // `last.p2 == first.p0` removes the degeneracy at its source.
    if sweep_angle.abs() >= std::f32::consts::TAU - 1e-4 {
        let first = segs[0].start_point();
        if let Some(SketchSegment::QuadBezier { p2, .. }) = segs.last_mut() {
            *p2 = first;
        }
    }
    segs
}

// ── Line segment ────────────────────────────────────────────────────

/// Unsigned distance from `p` to the line segment `a→b`. Used by the
/// per-edge contributor banding in [`Sketch2dPrimitive::sdf_with_edges`].
fn dist_point_seg(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let c = closest_on_line_seg(p, a, b);
    ((p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2)).sqrt()
}

fn closest_on_line_seg(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    let ex = b[0] - a[0];
    let ey = b[1] - a[1];
    let len_sq = ex * ex + ey * ey;
    if len_sq < 1e-12 {
        return a;
    }
    let wx = p[0] - a[0];
    let wy = p[1] - a[1];
    let t = ((wx * ex + wy * ey) / len_sq).clamp(0.0, 1.0);
    [a[0] + ex * t, a[1] + ey * t]
}

// ── Quadratic Bézier (analytic closest point) ───────────────────────

/// Closest point on a quadratic Bézier via the closed-form
/// depressed-cubic root method (Quilez/Cardano): minimising
/// `|P−B(t)|²` gives a cubic in `t` whose real roots are the
/// stationary parameters. We solve it exactly in f64, clamp to
/// `[0,1]`, and pick the nearest among the interior roots and the
/// two endpoints. No sampling — this is fully analytic.
fn closest_on_quad_bezier(p: [f32; 2], a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> [f32; 2] {
    // B(t) = a + L·t + M·t² where L = 2(b−a), M = a−2b+c
    let mx = (a[0] - 2.0 * b[0] + c[0]) as f64;
    let my = (a[1] - 2.0 * b[1] + c[1]) as f64;
    let lx = (2.0 * (b[0] - a[0])) as f64;
    let ly = (2.0 * (b[1] - a[1])) as f64;
    let wx = (p[0] - a[0]) as f64;
    let wy = (p[1] - a[1]) as f64;

    // d/dt|P−B(t)|² = 0  →  2(M·M)t³ + 3(L·M)t² + (L·L − 2W·M)t − W·L = 0
    let mm = mx * mx + my * my;
    let lm = lx * mx + ly * my;
    let ll = lx * lx + ly * ly;
    let wm = wx * mx + wy * my;
    let wl = wx * lx + wy * ly;

    let (roots, count) = solve_cubic_real(2.0 * mm, 3.0 * lm, ll - 2.0 * wm, -wl);

    let eval = |t: f32| -> [f32; 2] {
        let s = 1.0 - t;
        [
            s * s * a[0] + 2.0 * s * t * b[0] + t * t * c[0],
            s * s * a[1] + 2.0 * s * t * b[1] + t * t * c[1],
        ]
    };
    let dist_sq =
        |pt: [f32; 2]| -> f32 { (p[0] - pt[0]).powi(2) + (p[1] - pt[1]).powi(2) };

    let mut best = a;
    let mut best_d = dist_sq(a);
    let dc = dist_sq(c);
    if dc < best_d {
        best = c;
        best_d = dc;
    }
    for r in roots.iter().take(count) {
        let t = *r as f32;
        if t > 1e-6 && t < 1.0 - 1e-6 {
            let pt = eval(t);
            let d = dist_sq(pt);
            if d < best_d {
                best = pt;
                best_d = d;
            }
        }
    }
    best
}

/// Closest point on a quadratic Bézier **and its unit tangent** there. Mirrors
/// [`closest_on_quad_bezier`] but tracks the winning parameter `t*` so it can
/// return the analytic derivative `B'(t*) = L + 2·M·t*` (normalised) — an exact
/// curve tangent usable even on the curve, where `∇‖p−B‖` degenerates (DL-77).
fn closest_quad_bezier_pt_tangent(
    p: [f32; 2],
    a: [f32; 2],
    b: [f32; 2],
    c: [f32; 2],
) -> ([f32; 2], [f32; 2]) {
    let mx = (a[0] - 2.0 * b[0] + c[0]) as f64;
    let my = (a[1] - 2.0 * b[1] + c[1]) as f64;
    let lx = (2.0 * (b[0] - a[0])) as f64;
    let ly = (2.0 * (b[1] - a[1])) as f64;
    let wx = (p[0] - a[0]) as f64;
    let wy = (p[1] - a[1]) as f64;

    let mm = mx * mx + my * my;
    let lm = lx * mx + ly * my;
    let ll = lx * lx + ly * ly;
    let wm = wx * mx + wy * my;
    let wl = wx * lx + wy * ly;
    let (roots, count) = solve_cubic_real(2.0 * mm, 3.0 * lm, ll - 2.0 * wm, -wl);

    let eval = |t: f32| -> [f32; 2] {
        let s = 1.0 - t;
        [
            s * s * a[0] + 2.0 * s * t * b[0] + t * t * c[0],
            s * s * a[1] + 2.0 * s * t * b[1] + t * t * c[1],
        ]
    };
    let dist_sq = |pt: [f32; 2]| -> f32 { (p[0] - pt[0]).powi(2) + (p[1] - pt[1]).powi(2) };
    // Tangent B'(t) = L + 2·M·t.
    let tangent = |t: f64| normalize([(lx + 2.0 * mx * t) as f32, (ly + 2.0 * my * t) as f32]);

    let mut best = a;
    let mut best_t = 0.0_f64;
    let mut best_d = dist_sq(a);
    let dc = dist_sq(c);
    if dc < best_d {
        best = c;
        best_t = 1.0;
        best_d = dc;
    }
    for r in roots.iter().take(count) {
        let t = *r as f32;
        if t > 1e-6 && t < 1.0 - 1e-6 {
            let pt = eval(t);
            let d = dist_sq(pt);
            if d < best_d {
                best = pt;
                best_t = *r;
                best_d = d;
            }
        }
    }
    (best, tangent(best_t))
}

// ── Circular arc (analytic closest point) ───────────────────────────

fn arc_point(center: [f32; 2], radius: f32, angle: f32) -> [f32; 2] {
    let (s, c) = angle.sin_cos();
    [center[0] + radius * c, center[1] + radius * s]
}

/// Closest point on a circular arc. The nearest point on the full
/// circle is the radial projection of `p`; if its angle falls
/// inside `[start, start+sweep]` that point is the answer,
/// otherwise the nearer endpoint wins. Fully analytic.
fn closest_on_arc(
    p: [f32; 2],
    center: [f32; 2],
    radius: f32,
    start: f32,
    sweep: f32,
) -> [f32; 2] {
    let dx = p[0] - center[0];
    let dy = p[1] - center[1];
    let len = (dx * dx + dy * dy).sqrt();
    let proj_angle = dy.atan2(dx);
    if len > 1e-12 && angle_in_arc(proj_angle, start, sweep) {
        return arc_point(center, radius, proj_angle);
    }
    // Degenerate (p == center) or outside the angular span: pick
    // the nearer endpoint deterministically (start wins ties).
    let ps = arc_point(center, radius, start);
    let pe = arc_point(center, radius, start + sweep);
    let ds = (p[0] - ps[0]).powi(2) + (p[1] - ps[1]).powi(2);
    let de = (p[0] - pe[0]).powi(2) + (p[1] - pe[1]).powi(2);
    if ds <= de {
        ps
    } else {
        pe
    }
}

fn aabb_arc(center: [f32; 2], radius: f32, start: f32, sweep: f32) -> Aabb2 {
    let ps = arc_point(center, radius, start);
    let pe = arc_point(center, radius, start + sweep);
    let mut bb = Aabb2 {
        min: [ps[0].min(pe[0]), ps[1].min(pe[1])],
        max: [ps[0].max(pe[0]), ps[1].max(pe[1])],
    };
    // Cardinal extrema (0, π/2, π, 3π/2) that fall within the arc.
    for k in 0..4 {
        let a = k as f32 * std::f32::consts::FRAC_PI_2;
        if angle_in_arc(a, start, sweep) {
            let pt = arc_point(center, radius, a);
            bb = Aabb2::union(bb, Aabb2 { min: pt, max: pt });
        }
    }
    bb
}

// ── Angle utilities ─────────────────────────────────────────────────

fn normalize_angle_positive(a: f32) -> f32 {
    let a = a % std::f32::consts::TAU;
    if a < 0.0 {
        a + std::f32::consts::TAU
    } else {
        a
    }
}

fn angle_in_arc(theta: f32, start: f32, sweep: f32) -> bool {
    if sweep.abs() >= std::f32::consts::TAU {
        return true;
    }
    let ccw = normalize_angle_positive(theta - start);
    if sweep > 0.0 {
        ccw <= sweep
    } else {
        (std::f32::consts::TAU - ccw) <= -sweep
    }
}

// ── Cubic equation solver ───────────────────────────────────────────

/// Solve `ax³ + bx² + cx + d = 0` over the reals. Returns the
/// roots and their count. Used by the analytic quad-bézier
/// closest-point solver (Cardano's method on the depressed cubic).
fn solve_cubic_real(a: f64, b: f64, c: f64, d: f64) -> ([f64; 3], usize) {
    let mut roots = [0.0_f64; 3];

    if a.abs() < 1e-12 {
        if b.abs() < 1e-12 {
            if c.abs() < 1e-12 {
                return (roots, 0);
            }
            roots[0] = -d / c;
            return (roots, 1);
        }
        let disc = c * c - 4.0 * b * d;
        if disc < 0.0 {
            return (roots, 0);
        }
        let sq = disc.sqrt();
        roots[0] = (-c + sq) / (2.0 * b);
        roots[1] = (-c - sq) / (2.0 * b);
        return (roots, if disc < 1e-12 { 1 } else { 2 });
    }

    let b1 = b / a;
    let c1 = c / a;
    let d1 = d / a;

    let p = c1 - b1 * b1 / 3.0;
    let q = d1 - b1 * c1 / 3.0 + 2.0 * b1 * b1 * b1 / 27.0;
    let offset = -b1 / 3.0;

    let disc = -4.0 * p * p * p - 27.0 * q * q;

    if disc > 1e-10 {
        let r = (-p / 3.0).sqrt();
        let cos_arg = (-q / (2.0 * r * r * r)).clamp(-1.0, 1.0);
        let theta = cos_arg.acos() / 3.0;
        let tau3 = std::f64::consts::TAU / 3.0;
        roots[0] = 2.0 * r * theta.cos() + offset;
        roots[1] = 2.0 * r * (theta - tau3).cos() + offset;
        roots[2] = 2.0 * r * (theta + tau3).cos() + offset;
        (roots, 3)
    } else if disc.abs() <= 1e-10 {
        if p.abs() < 1e-10 {
            roots[0] = offset;
            (roots, 1)
        } else {
            roots[0] = 3.0 * q / p + offset;
            roots[1] = -3.0 * q / (2.0 * p) + offset;
            (roots, 2)
        }
    } else {
        let s = q * q / 4.0 + p * p * p / 27.0;
        let sq = s.sqrt();
        let u = (-q / 2.0 + sq).cbrt();
        let v = (-q / 2.0 - sq).cbrt();
        roots[0] = u + v + offset;
        (roots, 1)
    }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod path_tests {
    use super::*;

    fn square_path(closed: bool) -> Sketch2dPrimitive {
        // Unit square, CCW, from (-1,-1) to (1,1).
        Sketch2dPrimitive::Path {
            segments: vec![
                SketchSegment::Line {
                    start: [-1.0, -1.0],
                    end: [1.0, -1.0],
                },
                SketchSegment::Line {
                    start: [1.0, -1.0],
                    end: [1.0, 1.0],
                },
                SketchSegment::Line {
                    start: [1.0, 1.0],
                    end: [-1.0, 1.0],
                },
                SketchSegment::Line {
                    start: [-1.0, 1.0],
                    end: [-1.0, -1.0],
                },
            ],
            closed,
        }
    }

    /// Dense-flattening reference for the even-odd parity (the algorithm the
    /// analytic [`path_is_inside`] replaced, at a much higher sample count).
    fn flattened_is_inside(p: [f32; 2], segments: &[SketchSegment], steps: usize) -> bool {
        let mut inside = false;
        for seg in segments {
            let n = match seg {
                SketchSegment::Line { .. } => 1,
                _ => steps,
            };
            let mut a = seg.point_at(0.0);
            for k in 1..=n {
                let b = seg.point_at(k as f32 / n as f32);
                if (a[1] > p[1]) != (b[1] > p[1]) {
                    let x_cross = a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
                    if p[0] < x_cross {
                        inside = !inside;
                    }
                }
                a = b;
            }
        }
        inside
    }

    /// Grid-parity harness: the analytic crossing test must agree with the
    /// dense reference everywhere except within `guard` of the contour
    /// (where the flattened reference itself is wrong by the chord error).
    fn assert_inside_parity(segments: &[SketchSegment], lo: [f32; 2], hi: [f32; 2], guard: f32) {
        let n = 64;
        for iy in 0..=n {
            for ix in 0..=n {
                let p = [
                    lo[0] + (hi[0] - lo[0]) * ix as f32 / n as f32,
                    lo[1] + (hi[1] - lo[1]) * iy as f32 / n as f32,
                ];
                if path_min_distance(p, segments).0 < guard {
                    continue;
                }
                assert_eq!(
                    path_is_inside(p, segments),
                    flattened_is_inside(p, segments, 512),
                    "parity mismatch at {p:?}",
                );
            }
        }
    }

    #[test]
    fn analytic_inside_matches_flattened_reference_on_quad_contour() {
        // A wavy closed blob of quadratic Béziers — apexes both inside and
        // outside the parameter range, so the monotone split is exercised.
        let segments = vec![
            SketchSegment::QuadBezier { p0: [-2.0, 0.0], p1: [-2.5, 2.5], p2: [0.0, 2.0] },
            SketchSegment::QuadBezier { p0: [0.0, 2.0], p1: [2.5, 1.5], p2: [2.0, 0.0] },
            SketchSegment::QuadBezier { p0: [2.0, 0.0], p1: [1.5, -2.5], p2: [0.0, -2.0] },
            SketchSegment::QuadBezier { p0: [0.0, -2.0], p1: [-2.0, -1.5], p2: [-2.0, 0.0] },
        ];
        assert_inside_parity(&segments, [-3.5, -3.5], [3.5, 3.5], 0.02);
    }

    #[test]
    fn analytic_inside_matches_flattened_reference_on_arc_contour() {
        use std::f32::consts::{FRAC_PI_2, PI};
        // Stadium: two horizontal lines capped by semicircular arcs, the
        // right one swept CCW, the left one CW (negative sweep) — parity
        // must not care about traversal direction.
        let segments = vec![
            SketchSegment::Line { start: [-1.0, -1.0], end: [1.0, -1.0] },
            SketchSegment::Arc {
                center: [1.0, 0.0],
                radius: 1.0,
                start_angle: -FRAC_PI_2,
                sweep_angle: PI,
            },
            SketchSegment::Line { start: [1.0, 1.0], end: [-1.0, 1.0] },
            SketchSegment::Arc {
                center: [-1.0, 0.0],
                radius: 1.0,
                start_angle: FRAC_PI_2,
                sweep_angle: -PI,
            },
        ];
        assert_inside_parity(&segments, [-3.0, -2.0], [3.0, 2.0], 0.02);
    }

    #[test]
    fn analytic_inside_full_circle_arc() {
        let segments = vec![SketchSegment::Arc {
            center: [0.5, -0.25],
            radius: 1.5,
            start_angle: 0.3,
            sweep_angle: std::f32::consts::TAU,
        }];
        assert_inside_parity(&segments, [-2.0, -2.5], [3.0, 2.0], 0.02);
    }

    /// The axis-stripped revolve profile: an OPEN chain whose implicit
    /// closure would run along x = 0. Every revolved sample has x ≥ 0, and
    /// the +x ray test never needs the missing on-axis edge — interior
    /// points (between the chain and the axis) must read inside.
    #[test]
    fn open_axis_stripped_profile_parity() {
        let segments = vec![
            SketchSegment::Line { start: [0.0, 2.0], end: [1.5, 2.0] },
            SketchSegment::QuadBezier { p0: [1.5, 2.0], p1: [2.5, 0.0], p2: [1.5, -2.0] },
            SketchSegment::Line { start: [1.5, -2.0], end: [0.0, -2.0] },
        ];
        assert!(path_is_inside([0.5, 0.0], &segments), "interior near axis");
        assert!(path_is_inside([1.9, 0.0], &segments), "interior at belly");
        assert!(!path_is_inside([2.6, 0.0], &segments), "outside the belly");
        assert!(!path_is_inside([0.5, 2.5], &segments), "above the rim");
    }

    #[test]
    fn closed_square_path_sign_and_magnitude() {
        let sq = square_path(true);
        assert!((sq.sdf([0.0, 0.0]) - -1.0).abs() < 1e-5);
        assert!((sq.sdf([3.0, 0.0]) - 2.0).abs() < 1e-5);
        assert!(sq.sdf([1.0, 0.0]).abs() < 1e-5);
        assert!((sq.sdf([0.9, 0.0]) - -0.1).abs() < 1e-5);
    }

    #[test]
    fn open_line_path_is_unsigned() {
        let path = Sketch2dPrimitive::Path {
            segments: vec![SketchSegment::Line {
                start: [0.0, 0.0],
                end: [10.0, 0.0],
            }],
            closed: false,
        };
        assert!(path.sdf([5.0, 3.0]) >= 0.0);
        assert!((path.sdf([5.0, 3.0]) - 3.0).abs() < 1e-5);
        assert!((path.sdf([5.0, -4.0]) - 4.0).abs() < 1e-5);
        assert!(path.sdf([5.0, 0.0]).abs() < 1e-5);
    }

    #[test]
    fn empty_path_is_infinite() {
        let path = Sketch2dPrimitive::Path {
            segments: vec![],
            closed: true,
        };
        assert!(path.sdf([0.0, 0.0]).is_infinite());
        assert_eq!(path.edge_count(), 0);
    }

    /// Acceptance gate: analytic quad-bézier distance must match a
    /// dense 10k-sample reference to ~1e-4 over several curves and
    /// query points. This proves `closest_point` is analytic, not
    /// flattened.
    #[test]
    fn quad_bezier_distance_matches_dense_reference() {
        let curves = [
            ([0.0, 0.0], [5.0, 10.0], [10.0, 0.0]),
            ([-3.0, 2.0], [1.0, -8.0], [6.0, 4.0]),
            ([0.0, 0.0], [10.0, 0.0], [10.0, 10.0]),
            ([2.0, 2.0], [2.0, 2.0], [2.0, 2.0]), // degenerate point
        ];
        let queries = [
            [5.0, 5.0],
            [5.0, 0.0],
            [0.0, 0.0],
            [-5.0, -5.0],
            [12.0, 3.0],
            [3.3, 1.1],
        ];
        let mut max_err = 0.0_f32;
        for (p0, p1, p2) in curves {
            let seg = SketchSegment::QuadBezier { p0, p1, p2 };
            for q in queries {
                let analytic = seg.unsigned_distance(q);
                // Dense reference.
                const N: usize = 10_000;
                let mut ref_d = f32::INFINITY;
                for k in 0..=N {
                    let t = k as f32 / N as f32;
                    let pt = seg.point_at(t);
                    let d = ((q[0] - pt[0]).powi(2) + (q[1] - pt[1]).powi(2)).sqrt();
                    ref_d = ref_d.min(d);
                }
                let err = (analytic - ref_d).abs();
                max_err = max_err.max(err);
                assert!(
                    err < 1e-3,
                    "quad analytic {analytic} vs dense {ref_d} (err {err}) for q={q:?}"
                );
            }
        }
        // The analytic solver should beat the discretisation error
        // of even a 10k-sample reference comfortably.
        assert!(max_err < 1e-3, "max quad distance error {max_err}");
    }

    #[test]
    fn closed_path_with_quad_bezier_arc() {
        let path = Sketch2dPrimitive::Path {
            segments: vec![
                SketchSegment::Line {
                    start: [-1.0, 0.0],
                    end: [1.0, 0.0],
                },
                SketchSegment::Line {
                    start: [1.0, 0.0],
                    end: [1.0, 1.0],
                },
                SketchSegment::QuadBezier {
                    p0: [1.0, 1.0],
                    p1: [0.0, 2.0],
                    p2: [-1.0, 1.0],
                },
                SketchSegment::Line {
                    start: [-1.0, 1.0],
                    end: [-1.0, 0.0],
                },
            ],
            closed: true,
        };
        assert!(path.sdf([0.0, 0.5]) < 0.0);
        assert!(path.sdf([0.0, 1.4]) < 0.0);
        assert!(path.sdf([5.0, 0.5]) > 0.0);
        assert!(path.sdf([0.0, 3.0]) > 0.0);
        assert!(path.sdf([0.0, 0.0]).abs() < 1e-4);
    }

    #[test]
    fn arc_distance_is_exact() {
        // Quarter circle, radius 5, centre origin, 0 → π/2.
        let seg = SketchSegment::Arc {
            center: [0.0, 0.0],
            radius: 5.0,
            start_angle: 0.0,
            sweep_angle: std::f32::consts::FRAC_PI_2,
        };
        // On the arc.
        assert!(seg.unsigned_distance([0.0, 5.0]) < 1e-4);
        // Radially outside at 45°: distance = |p|-r.
        let s = std::f32::consts::FRAC_1_SQRT_2;
        assert!((seg.unsigned_distance([8.0 * s, 8.0 * s]) - 3.0).abs() < 1e-4);
        // Outside the sweep (angle 180°): nearest is the (0,5) end.
        let expected = ((3.0_f32).powi(2) + 25.0).sqrt();
        assert!((seg.unsigned_distance([-3.0, 0.0]) - expected).abs() < 1e-3);
    }

    #[test]
    fn g1_grouping_smooth_arc_is_one_branch_corner_splits() {
        // Three G1-continuous quad segments approximating a smooth
        // arc: the end tangent of each matches the start tangent of
        // the next, so they collapse to a single branch.
        let smooth = Sketch2dPrimitive::Path {
            segments: vec![
                SketchSegment::QuadBezier {
                    p0: [0.0, 0.0],
                    p1: [1.0, 1.0],
                    p2: [2.0, 1.0],
                },
                // start tangent = (2,1)-(1,1)=(1,0) == prev end tangent.
                SketchSegment::QuadBezier {
                    p0: [2.0, 1.0],
                    p1: [3.0, 1.0],
                    p2: [4.0, 0.0],
                },
            ],
            closed: false,
        };
        // Verify the join really is G1.
        let segs = match &smooth {
            Sketch2dPrimitive::Path { segments, .. } => segments.clone(),
            _ => unreachable!(),
        };
        let g = g1_group_segments(&segs, PATH_G1_COS_THRESHOLD);
        assert_eq!(g[0], g[1], "G1 join should share a group");
        assert_eq!(smooth.edge_count(), 1);

        // Insert a sharp corner: a line that leaves at a different
        // tangent → the corner splits into a second branch.
        let with_corner = Sketch2dPrimitive::Path {
            segments: vec![
                segs[0],
                segs[1],
                SketchSegment::Line {
                    start: [4.0, 0.0],
                    end: [4.0, -3.0],
                },
            ],
            closed: false,
        };
        assert_eq!(with_corner.edge_count(), 2, "corner adds a branch");
    }

    #[test]
    fn collinear_lines_collapse_square_corners_split() {
        let collinear = Sketch2dPrimitive::Path {
            segments: vec![
                SketchSegment::Line {
                    start: [0.0, 0.0],
                    end: [1.0, 0.0],
                },
                SketchSegment::Line {
                    start: [1.0, 0.0],
                    end: [2.0, 0.0],
                },
                SketchSegment::Line {
                    start: [2.0, 0.0],
                    end: [3.0, 0.0],
                },
                SketchSegment::Line {
                    start: [3.0, 0.0],
                    end: [4.0, 0.0],
                },
            ],
            closed: false,
        };
        assert_eq!(collinear.edge_count(), 1);
        assert_eq!(square_path(true).edge_count(), 4);
    }

    #[test]
    fn path_sdf_with_edge_maps_to_group() {
        let sq = square_path(true);
        // Clearly outside the bottom edge → group 0.
        let (_, e) = sq.sdf_with_edge([0.0, -3.0]);
        assert_eq!(e, 0);
        // Clearly outside the right edge → group 1.
        let (_, e) = sq.sdf_with_edge([3.0, 0.0]);
        assert_eq!(e, 1);
    }

    #[test]
    fn path_sdf_with_edge_corner_tie_break_is_deterministic() {
        // A point on the medial diagonal past the (1,-1) corner is
        // equidistant to the bottom (group 0) and right (group 1)
        // edges. The tie must break to the lower segment index (0).
        let sq = square_path(true);
        let (_, e) = sq.sdf_with_edge([3.0, -3.0]);
        assert_eq!(e, 0, "corner medial-axis tie breaks to lower index");
        // Repeated calls are stable.
        for _ in 0..8 {
            let (_, e2) = sq.sdf_with_edge([3.0, -3.0]);
            assert_eq!(e2, 0);
        }
    }

    // ── Ellipse decomposition (DL-49 Option B) ──────────────────────

    /// True signed-ish radial test: how far the decomposed boundary
    /// deviates from the analytic ellipse, sampled densely. Returns
    /// the max |distance-to-Path| over points exactly on the ellipse.
    fn ellipse_boundary_error(
        center: [f32; 2],
        rx: f32,
        ry: f32,
        rot: f32,
        segs: u32,
        tol: f32,
    ) -> f32 {
        let path = Sketch2dPrimitive::Path {
            segments: ellipse_quad_beziers(center, rx, ry, rot, 0.0, TAU, segs, tol),
            closed: true,
        };
        let (sp, cp) = rot.sin_cos();
        let mut max_err = 0.0_f32;
        const N: usize = 2000;
        for k in 0..N {
            let a = TAU * k as f32 / N as f32;
            let (s, c) = a.sin_cos();
            let lx = rx * c;
            let ly = ry * s;
            // Point exactly on the analytic ellipse.
            let p = [
                center[0] + cp * lx - sp * ly,
                center[1] + sp * lx + cp * ly,
            ];
            max_err = max_err.max(path.sdf(p).abs());
        }
        max_err
    }

    const TAU: f32 = std::f32::consts::TAU;

    #[test]
    fn ellipse_auto_tolerance_is_tight() {
        // Auto segment count must keep the boundary within a few×
        // the requested 10 µm tolerance for a range of ellipses.
        for (rx, ry, rot) in [
            (10.0, 10.0, 0.0),       // circle
            (20.0, 5.0, 0.0),        // axis-aligned, eccentric
            (15.0, 8.0, 0.7),        // rotated
            (3.0, 30.0, -1.2),       // tall, rotated
        ] {
            let err = ellipse_boundary_error([2.0, -1.0], rx, ry, rot, 0, 0.01);
            assert!(err < 0.05, "ellipse ({rx},{ry},{rot}) boundary err {err} mm too large");
        }
    }

    #[test]
    fn ellipse_more_segments_reduce_error() {
        let coarse = ellipse_boundary_error([0.0, 0.0], 20.0, 5.0, 0.0, 4, 0.0);
        let fine = ellipse_boundary_error([0.0, 0.0], 20.0, 5.0, 0.0, 32, 0.0);
        assert!(fine < coarse, "more segments must reduce error ({fine} !< {coarse})");
    }

    #[test]
    fn ellipse_is_a_single_g1_edge() {
        // A smooth closed ellipse is one B-Rep edge: the segment
        // joins are all G1 (and the wrap-around close is too).
        let segs = ellipse_quad_beziers([0.0, 0.0], 12.0, 7.0, 0.3, 0.0, TAU, 16, 0.0);
        let path = Sketch2dPrimitive::Path {
            segments: segs,
            closed: true,
        };
        assert_eq!(path.edge_count(), 1, "ellipse must be a single edge");
    }

    #[test]
    fn ellipse_sign_inside_outside() {
        let path = Sketch2dPrimitive::Path {
            segments: ellipse_quad_beziers([0.0, 0.0], 10.0, 4.0, 0.0, 0.0, TAU, 0, 0.01),
            closed: true,
        };
        assert!(path.sdf([0.0, 0.0]) < 0.0, "centre is inside");
        assert!(path.sdf([8.0, 0.0]) < 0.0, "inside along major axis");
        assert!(path.sdf([0.0, 3.0]) < 0.0, "inside along minor axis");
        assert!(path.sdf([11.0, 0.0]) > 0.0, "outside past major axis");
        assert!(path.sdf([0.0, 5.0]) > 0.0, "outside past minor axis");
    }

    #[test]
    fn ellipse_degenerate_inputs_yield_no_segments() {
        assert!(ellipse_quad_beziers([0.0, 0.0], 0.0, 5.0, 0.0, 0.0, TAU, 0, 0.01).is_empty());
        assert!(ellipse_quad_beziers([0.0, 0.0], 5.0, 0.0, 0.0, 0.0, TAU, 0, 0.01).is_empty());
        assert!(ellipse_quad_beziers([0.0, 0.0], 5.0, 5.0, 0.0, 0.0, 0.0, 0, 0.01).is_empty());
    }

    #[test]
    fn elliptic_arc_endpoints_lie_on_ellipse() {
        // A partial sweep: endpoints of the chain must sit exactly on
        // the analytic ellipse (open arc, not closed).
        let rx = 9.0;
        let ry = 4.0;
        let start = 0.4;
        let sweep = 2.0;
        let segs = ellipse_quad_beziers([1.0, 1.0], rx, ry, 0.0, start, sweep, 8, 0.0);
        let on_ellipse = |a: f32| [1.0 + rx * a.cos(), 1.0 + ry * a.sin()];
        let first = segs.first().unwrap().start_point();
        let last = segs.last().unwrap().end_point();
        let e0 = on_ellipse(start);
        let e1 = on_ellipse(start + sweep);
        assert!((first[0] - e0[0]).hypot(first[1] - e0[1]) < 1e-4);
        assert!((last[0] - e1[0]).hypot(last[1] - e1[1]) < 1e-4);
    }

    // ── #392: seam-robust even-odd parity on symmetry lines ────────────

    /// A full circle split into two semicircular arcs whose shared seams lie
    /// exactly on the horizontal symmetry line `y = 0` (the arc endpoints are
    /// (±r, 0)). A query row exactly on `y = 0` runs straight through both
    /// seams — the historically fragile case where one arc re-derived the seam
    /// endpoint from angle `0` (`sin = 0`) and the other from angle `2π`
    /// (`sin ≈ +3.5e-7·r`), landing on opposite sides of the ray, so the seam
    /// was counted twice or not at all and the 2D distance sign flipped.
    fn two_semicircle_circle() -> Vec<SketchSegment> {
        use std::f32::consts::PI;
        vec![
            // Upper half: angle 0 → π, endpoints (r,0) and (−r,0).
            SketchSegment::Arc { center: [0.0, 0.0], radius: 2.0, start_angle: 0.0, sweep_angle: PI },
            // Lower half: angle π → 2π, back to (r,0).
            SketchSegment::Arc { center: [0.0, 0.0], radius: 2.0, start_angle: PI, sweep_angle: PI },
        ]
    }

    #[test]
    fn merge_cocircular_runs_fuses_two_semicircles_into_a_circle() {
        // The normalisation pass must collapse the two-arc circle into one
        // full-sweep arc (no interior seam), and leave a non-co-circular path
        // untouched (returns None → no allocation on the hot path).
        let merged = merge_cocircular_runs(&two_semicircle_circle()).expect("should fuse");
        assert_eq!(merged.len(), 1, "two co-circular arcs must fuse into one");
        match merged[0] {
            SketchSegment::Arc { sweep_angle, .. } => {
                assert!((sweep_angle.abs() - std::f32::consts::TAU).abs() < 1e-4);
            }
            _ => panic!("fused segment must stay an arc"),
        }
        // A path without a co-circular arc pair does not merge — the pass
        // returns None so nothing is allocated on the hot SDF path.
        let no_merge = vec![
            SketchSegment::Line { start: [0.0, 0.0], end: [1.0, 0.0] },
            SketchSegment::Arc { center: [1.0, 1.0], radius: 1.0, start_angle: -1.5, sweep_angle: 1.5 },
            SketchSegment::Line { start: [1.0, 2.0], end: [0.0, 2.0] },
        ];
        assert!(merge_cocircular_runs(&no_merge).is_none());
    }

    #[test]
    fn seam_on_symmetry_line_arc_arc_signs_correct() {
        let segs = two_semicircle_circle();
        // Interior points ON the seam line y = 0 must be inside (negative sdf).
        for x in [-1.5f32, -0.5, 0.0, 0.5, 1.5] {
            let p = [x, 0.0];
            assert!(path_is_inside(p, &segs), "interior seam point {p:?} classified outside");
            assert!(sdf_path(p, &segs, true) < 0.0, "sdf sign wrong at interior seam {p:?}");
        }
        // Exterior points ON the seam line must be outside (positive sdf).
        for x in [-3.0f32, -2.5, 2.5, 3.0] {
            let p = [x, 0.0];
            assert!(!path_is_inside(p, &segs), "exterior seam point {p:?} classified inside");
            assert!(sdf_path(p, &segs, true) > 0.0, "sdf sign wrong at exterior seam {p:?}");
        }
    }

    #[test]
    fn seam_band_around_symmetry_line_is_stable() {
        // Sweep a dense band of query rows straddling y = 0 by tiny f64 steps —
        // interior rows must all classify inside, exterior rows all outside.
        // Before the co-circular run-merge, rows exactly on the seam flipped
        // sign sporadically.
        let segs = two_semicircle_circle();
        for i in -200i32..=200 {
            let y = (i as f64) * 1e-9;
            assert!(
                path_is_inside([0.0f32, y as f32], &segs),
                "seam-band interior flip at y={y:e} (i={i})",
            );
            assert!(
                !path_is_inside([5.0f32, y as f32], &segs),
                "seam-band exterior flip at y={y:e} (i={i})",
            );
        }
    }

    #[test]
    fn seam_on_symmetry_line_arc_line_signs_correct() {
        use std::f32::consts::PI;
        // Half-disk: a semicircular arc (top) closed by a straight diameter
        // (bottom) — an arc↔line seam whose endpoints (±r, 0) sit exactly on
        // the symmetry line y = 0. Each segment keeps its own endpoints, so the
        // per-segment half-open rule already classifies this seam consistently.
        let segs = vec![
            SketchSegment::Arc { center: [0.0, 0.0], radius: 2.0, start_angle: 0.0, sweep_angle: PI },
            SketchSegment::Line { start: [-2.0, 0.0], end: [2.0, 0.0] },
        ];
        // Row exactly on the diameter seam: interior between the caps, exterior
        // beyond them.
        for x in [-1.5f32, -0.5, 0.0, 0.5, 1.5] {
            assert!(path_is_inside([x, 0.0], &segs), "arc↔line seam interior {x} outside");
        }
        for x in [-3.0f32, 3.0] {
            assert!(!path_is_inside([x, 0.0], &segs), "arc↔line seam exterior {x} inside");
        }
        // A hair above the diameter is inside the half-disk; a hair below is
        // outside — the sign is stable straight through the seam.
        for x in [-1.0f32, 0.0, 1.0] {
            assert!(path_is_inside([x, 1e-6], &segs), "just above diameter must be inside");
            assert!(!path_is_inside([x, -1e-6], &segs), "just below diameter must be outside");
        }
    }

    #[test]
    fn wraparound_fold_rotates_start_angle() {
        use std::f32::consts::PI;
        // Half-disk (upper semicircle + diameter) whose contour STARTS in the
        // middle of the arc run: Arc [π/4 → π], the diameter line, then Arc
        // [0 → π/4] closing back to the start. The last and first segments are
        // co-circular, so the wrap-around fold fires. The fused head must span
        // [0, π] — folding only the sweep (keeping start π/4) would cover
        // [π/4, 5π/4] instead and misclassify the whole upper half.
        let r = 2.0f32;
        let segs = vec![
            SketchSegment::Arc { center: [0.0, 0.0], radius: r, start_angle: PI / 4.0, sweep_angle: 3.0 * PI / 4.0 },
            SketchSegment::Line { start: [-r, 0.0], end: [r, 0.0] },
            SketchSegment::Arc { center: [0.0, 0.0], radius: r, start_angle: 0.0, sweep_angle: PI / 4.0 },
        ];
        let merged = merge_cocircular_runs(&segs).expect("wrap pair must fuse");
        assert_eq!(merged.len(), 2);
        match merged[0] {
            SketchSegment::Arc { start_angle, sweep_angle, .. } => {
                assert!(start_angle.abs() < 1e-6, "start must rotate back to 0, got {start_angle}");
                assert!((sweep_angle - PI).abs() < 1e-6, "sweep must be π, got {sweep_angle}");
            }
            _ => panic!("head must stay an arc"),
        }
        // (0, 1) is inside the upper half-disk, (0, −1) is outside.
        assert!(path_is_inside([0.0, 1.0], &segs), "interior of half-disk misclassified");
        assert!(!path_is_inside([0.0, -1.0], &segs), "below the diameter must be outside");
    }

    #[test]
    fn clockwise_cocircular_arcs_also_fuse_and_classify() {
        use std::f32::consts::PI;
        // Same circle, both halves drawn CLOCKWISE (negative sweep): the merge
        // must still fuse them (same direction) and parity stays correct.
        let segs = vec![
            SketchSegment::Arc { center: [0.0, 0.0], radius: 2.0, start_angle: 0.0, sweep_angle: -PI },
            SketchSegment::Arc { center: [0.0, 0.0], radius: 2.0, start_angle: -PI, sweep_angle: -PI },
        ];
        assert_eq!(merge_cocircular_runs(&segs).map(|m| m.len()), Some(1));
        for x in [-1.5f32, 0.0, 1.5] {
            assert!(path_is_inside([x, 0.0], &segs), "cw-arc interior {x} misclassified");
        }
        for x in [-3.0f32, 3.0] {
            assert!(!path_is_inside([x, 0.0], &segs), "cw-arc exterior {x} misclassified");
        }
    }

    // ── #423: a Sector's three boundary branches ────────────────────────

    /// A pie wedge has three analytic boundary pieces, and `edge_count()`
    /// has always reserved three group slots. Until #423 the attribution
    /// collapsed all of them onto group 0 ("single-group simplification"),
    /// which left an extruded sector's walls out of the weaver's half-space
    /// map and sent the whole body to the octree. Group order: 0 = arc,
    /// 1 = the flank at `start_angle`, 2 = the flank at `start + sweep`.
    #[test]
    fn sector_reports_arc_and_both_flanks_as_distinct_groups() {
        use std::f32::consts::FRAC_PI_2;
        // Quarter wedge in the first quadrant, radius 10, centred at origin.
        let sector = Sketch2dPrimitive::Sector {
            center: [0.0, 0.0],
            radius: 10.0,
            start_angle: 0.0,
            sweep_angle: FRAC_PI_2,
        };
        assert_eq!(sector.edge_count(), 3);

        // Mid-arc (45°) → the arc.
        let m = 10.0 * std::f32::consts::FRAC_1_SQRT_2;
        assert_eq!(sector.sdf_with_edge([m, m]).1, 0, "mid-arc is group 0");
        // Halfway along the +x flank (start_angle = 0) → group 1.
        assert_eq!(sector.sdf_with_edge([5.0, 0.0]).1, 1, "+x flank is group 1");
        // Halfway along the +y flank (start + sweep = 90°) → group 2.
        assert_eq!(sector.sdf_with_edge([0.0, 5.0]).1, 2, "+y flank is group 2");
    }

    /// At a pie wedge's rim corner the arc and the adjoining flank tie, so
    /// the eps-band carries BOTH — that is what lets an extruded sector
    /// weave the corner edge where its cylinder wall meets its flank plane.
    #[test]
    fn sector_corner_band_carries_arc_and_flank() {
        use std::f32::consts::FRAC_PI_2;
        let sector = Sketch2dPrimitive::Sector {
            center: [0.0, 0.0],
            radius: 10.0,
            start_angle: 0.0,
            sweep_angle: FRAC_PI_2,
        };
        // The rim end of the start flank: (10, 0) is on the arc AND on the flank.
        let band = sector.sdf_with_edges([10.0, 0.0], 1e-3);
        let mut groups = band.groups().to_vec();
        groups.sort_unstable();
        assert_eq!(groups, vec![0, 1], "arc + start flank tie at the rim corner");
    }

    /// A full-turn sector IS a circle: its two flanks coincide and lie
    /// strictly inside the disk, so they are not boundary branches at all.
    /// Everything must report the arc, or an extruded full sector would
    /// grow two phantom wall branches through its own interior.
    #[test]
    fn full_turn_sector_has_only_the_arc_branch() {
        let disk = Sketch2dPrimitive::Sector {
            center: [1.0, -2.0],
            radius: 4.0,
            start_angle: 0.3,
            sweep_angle: std::f32::consts::TAU,
        };
        let d = sector_boundary_distances([1.0, -2.0], [1.0, -2.0], 4.0, 0.3, std::f32::consts::TAU);
        assert!(d[0].is_finite(), "the arc stays a branch");
        assert!(!d[1].is_finite() && !d[2].is_finite(), "no flank branches on a full turn");
        for p in [[5.0f32, -2.0], [1.0, 2.0], [-3.0, -2.0]] {
            assert_eq!(disk.sdf_with_edge(p).1, 0, "{p:?} must report the arc");
        }
    }

    /// The flank distances are genuine point-to-SEGMENT distances (centre →
    /// rim), not point-to-infinite-line: beyond the rim the nearest flank
    /// point is its endpoint. Without that a far-away point would be
    /// attributed to a flank the sector's boundary never reaches there.
    #[test]
    fn sector_flank_distance_is_segment_not_line() {
        use std::f32::consts::FRAC_PI_2;
        // Far out along +x, well past the rim at (10, 0).
        let d = sector_boundary_distances([30.0, 0.0], [0.0, 0.0], 10.0, 0.0, FRAC_PI_2);
        // Flank 1 ends at (10, 0) ⇒ distance 20, not 0 (the infinite line).
        assert!((d[1] - 20.0).abs() < 1e-4, "flank is a segment: {}", d[1]);
        // The arc is nearer (radial distance 20 as well, tie) — the point is
        // outside either way; what matters is that the flank is not 0.
        assert!(d[1] > 1.0);
    }
}
