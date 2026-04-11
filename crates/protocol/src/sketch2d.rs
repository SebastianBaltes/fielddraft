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
        }
    }
}

/// Signed distance to a simple polygon (Inigo Quilez, 2D
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
}
