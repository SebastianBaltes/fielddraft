//! 2D sketch kernel.
//!
//! This is the K4 companion to `csg.rs` and `bvh.rs`: a purely 2D
//! world of signed-distance primitives and CSG composition that
//! later gets lifted into the 3D tree via `CsgNode::Extrude` and
//! `CsgNode::Revolve`. The design mirrors the 3D side line-for-line:
//!
//!   * [`Sketch2d`] is the logical IR — diff-friendly, no cached
//!     bounding boxes, used by tests and the eventual JSON
//!     serialisation path (K5/K6).
//!   * [`BoundedSketch2d`] is the BVH-wrapped form built once per
//!     query batch, with a [`Aabb2`] cached at every node and the
//!     same Union-closer-first / Difference-far-enough pruning as
//!     the 3D [`crate::bvh::BoundedCsg`].
//!
//! K4 scope (the K5a/K5b solver hasn't landed yet):
//!
//!   * Region primitives only: [`Sketch2dPrimitive::Circle`],
//!     [`Sketch2dPrimitive::Rectangle`] and the general
//!     [`Sketch2dPrimitive::Polygon`]. Lines and arcs are still
//!     authored as "curves with zero thickness", which aren't
//!     useful operands for a min/max composition on their own —
//!     they'll come in K5/K6 once the solver lets us derive the
//!     bounded regions from user-drawn loops.
//!   * Offset regions (`Sketch2d::Offset`) compose with any inner
//!     sketch and dilate/erode it by a constant distance. This is
//!     the K4-sized stand-in for "thicken a curve" once we need
//!     to draw thin walls.
//!
//! Polygon SDF uses Inigo Quilez's even-odd-ray-cast formulation
//! from <https://iquilezles.org/articles/distfunctions2d/>. It's
//! exact for arbitrary simple polygons (convex or concave, as long
//! as edges don't cross) and handles both signs.

/// 2D axis-aligned bounding box. Semantics mirror
/// [`crate::csg::Aabb`]: `EMPTY` carries inverted extents so
/// `union(empty, x) == x` is a free no-op.
#[derive(Debug, Clone, Copy, PartialEq)]
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

/// Distance from a point to the surface of a 2D AABB. Zero inside,
/// Euclidean distance outside — same Lipschitz-1 bound semantics
/// as the 3D `distance_point_to_aabb`.
pub fn distance_point_to_aabb2(p: [f32; 2], aabb: Aabb2) -> f32 {
    if aabb.is_empty() {
        return f32::INFINITY;
    }
    let dx = (aabb.min[0] - p[0]).max(0.0).max(p[0] - aabb.max[0]);
    let dy = (aabb.min[1] - p[1]).max(0.0).max(p[1] - aabb.max[1]);
    (dx * dx + dy * dy).sqrt()
}

/// Filled 2D primitive. Every variant is negative strictly inside
/// its region and positive outside, so they compose under the usual
/// boolean min/max rules.
#[derive(Debug, Clone, PartialEq)]
pub enum Sketch2dPrimitive {
    /// Filled disk: `‖p − center‖ − radius`.
    Circle { center: [f32; 2], radius: f32 },
    /// Axis-aligned filled rectangle. `half_extents` is per-axis.
    Rectangle { center: [f32; 2], half_extents: [f32; 2] },
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

/// Signed distance to a simple polygon (Inigo Quilez, 2D distance
/// functions). Walks every edge, tracks the minimum squared edge
/// distance, and recovers the sign from an even-odd crossing count.
/// Fewer than three vertices yields `+∞`.
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

/// Logical 2D CSG tree — the sketch's equivalent of
/// [`crate::csg::CsgNode`]. No cached AABBs, no flattened
/// transforms: that's what [`BoundedSketch2d`] is for.
#[derive(Debug, Clone, PartialEq)]
pub enum Sketch2d {
    Empty,
    Prim(Sketch2dPrimitive),
    Union(Box<Sketch2d>, Box<Sketch2d>),
    Difference(Box<Sketch2d>, Box<Sketch2d>),
    Intersection(Box<Sketch2d>, Box<Sketch2d>),
    /// Shift the zero-level-set of an inner sketch by a constant
    /// distance: positive `distance` grows the region outwards,
    /// negative erodes inwards. Equivalent to
    /// `inner.sdf(p) - distance`.
    Offset {
        inner: Box<Sketch2d>,
        distance: f32,
    },
}

impl Sketch2d {
    /// Naive signed distance — walks every branch. Used by tests
    /// as the ground truth the BVH form must agree with.
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
    /// keeps the left operand, Intersection keeps the left, Offset
    /// expands the inner by `distance`.
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

/// BVH-wrapped mirror of [`Sketch2d`]. Every node carries its
/// precomputed [`Aabb2`], and [`BoundedSketch2d::sdf`] uses it to
/// prune boolean branches — exactly like [`crate::bvh::BoundedCsg`]
/// does in 3D.
#[derive(Debug, Clone)]
pub enum BoundedSketch2d {
    Empty,
    Prim {
        primitive: Sketch2dPrimitive,
        aabb: Aabb2,
    },
    Union {
        left: Box<BoundedSketch2d>,
        right: Box<BoundedSketch2d>,
        aabb: Aabb2,
    },
    Difference {
        left: Box<BoundedSketch2d>,
        right: Box<BoundedSketch2d>,
        aabb: Aabb2,
    },
    Intersection {
        left: Box<BoundedSketch2d>,
        right: Box<BoundedSketch2d>,
        aabb: Aabb2,
    },
    Offset {
        inner: Box<BoundedSketch2d>,
        distance: f32,
        aabb: Aabb2,
    },
}

impl BoundedSketch2d {
    pub fn from_sketch(sketch: &Sketch2d) -> Self {
        match sketch {
            Sketch2d::Empty => Self::Empty,
            Sketch2d::Prim(prim) => Self::Prim {
                primitive: prim.clone(),
                aabb: prim.aabb(),
            },
            Sketch2d::Union(a, b) => {
                let la = Self::from_sketch(a);
                let lb = Self::from_sketch(b);
                let aabb = Aabb2::union(la.aabb(), lb.aabb());
                Self::Union {
                    left: Box::new(la),
                    right: Box::new(lb),
                    aabb,
                }
            }
            Sketch2d::Difference(a, b) => {
                let la = Self::from_sketch(a);
                let lb = Self::from_sketch(b);
                let aabb = la.aabb();
                Self::Difference {
                    left: Box::new(la),
                    right: Box::new(lb),
                    aabb,
                }
            }
            Sketch2d::Intersection(a, b) => {
                let la = Self::from_sketch(a);
                let lb = Self::from_sketch(b);
                let aabb = la.aabb();
                Self::Intersection {
                    left: Box::new(la),
                    right: Box::new(lb),
                    aabb,
                }
            }
            Sketch2d::Offset { inner, distance } => {
                let li = Self::from_sketch(inner);
                let aabb = li.aabb().expanded(*distance);
                Self::Offset {
                    inner: Box::new(li),
                    distance: *distance,
                    aabb,
                }
            }
        }
    }

    pub fn aabb(&self) -> Aabb2 {
        match self {
            Self::Empty => Aabb2::EMPTY,
            Self::Prim { aabb, .. }
            | Self::Union { aabb, .. }
            | Self::Difference { aabb, .. }
            | Self::Intersection { aabb, .. }
            | Self::Offset { aabb, .. } => *aabb,
        }
    }

    /// Pruned signed-distance query. Matches [`Sketch2d::sdf`] for
    /// the same input tree up to f32 rounding, but short-circuits
    /// boolean branches whose AABB can't affect the running value.
    /// Same pruning rules as the 3D version in `bvh.rs`:
    ///
    /// * **Union**: visit the AABB-closer child first; the other
    ///   can be skipped once its AABB-distance exceeds the first
    ///   child's exact SDF.
    /// * **Difference**: skip the right operand when the left's
    ///   SDF already sits outside the right's AABB reach.
    /// * **Intersection**: no safe prune — the AABB is only a
    ///   lower bound on the child SDF, which doesn't help `max`.
    pub fn sdf(&self, p: [f32; 2]) -> f32 {
        match self {
            Self::Empty => f32::INFINITY,
            Self::Prim { primitive, .. } => primitive.sdf(p),
            Self::Union { left, right, .. } => {
                let dist_left = distance_point_to_aabb2(p, left.aabb());
                let dist_right = distance_point_to_aabb2(p, right.aabb());
                let (first_node, second_node, second_bound) = if dist_left <= dist_right {
                    (&**left, &**right, dist_right)
                } else {
                    (&**right, &**left, dist_left)
                };
                let d_first = first_node.sdf(p);
                if second_bound > 0.0 && d_first < second_bound {
                    d_first
                } else {
                    d_first.min(second_node.sdf(p))
                }
            }
            Self::Difference { left, right, .. } => {
                let d_left = left.sdf(p);
                let dist_right = distance_point_to_aabb2(p, right.aabb());
                if dist_right > 0.0 && d_left >= -dist_right {
                    d_left
                } else {
                    d_left.max(-right.sdf(p))
                }
            }
            Self::Intersection { left, right, .. } => left.sdf(p).max(right.sdf(p)),
            Self::Offset { inner, distance, .. } => inner.sdf(p) - distance,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── primitives ────────────────────────────────────────────────

    #[test]
    fn circle_sdf_zero_on_boundary_negative_inside_positive_outside() {
        let c = Sketch2dPrimitive::Circle {
            center: [0.0, 0.0],
            radius: 5.0,
        };
        assert!((c.sdf([5.0, 0.0])).abs() < 1e-5);
        assert!((c.sdf([0.0, -5.0])).abs() < 1e-5);
        assert!((c.sdf([0.0, 0.0]) + 5.0).abs() < 1e-5);
        assert!((c.sdf([10.0, 0.0]) - 5.0).abs() < 1e-5);
    }

    #[test]
    fn rectangle_sdf_interior_and_exterior() {
        let r = Sketch2dPrimitive::Rectangle {
            center: [0.0, 0.0],
            half_extents: [3.0, 2.0],
        };
        assert!((r.sdf([3.0, 0.0])).abs() < 1e-5);
        assert!((r.sdf([0.0, 2.0])).abs() < 1e-5);
        assert!((r.sdf([0.0, 0.0]) + 2.0).abs() < 1e-5);
        // Diagonal corner — external Euclidean distance.
        let expected = ((5.0_f32 - 3.0).powi(2) + (4.0_f32 - 2.0).powi(2)).sqrt();
        assert!((r.sdf([5.0, 4.0]) - expected).abs() < 1e-4);
    }

    #[test]
    fn polygon_sdf_signed_on_triangle() {
        let triangle = Sketch2dPrimitive::Polygon {
            vertices: vec![[0.0, 0.0], [10.0, 0.0], [0.0, 10.0]],
        };
        // Centroid is inside.
        assert!(triangle.sdf([3.0, 3.0]) < 0.0);
        // Corner is on the boundary.
        assert!(triangle.sdf([0.0, 0.0]).abs() < 1e-5);
        // Midpoint of the hypotenuse also on the boundary.
        assert!(triangle.sdf([5.0, 5.0]).abs() < 1e-4);
        // Outside the +x edge.
        assert!((triangle.sdf([15.0, 0.0]) - 5.0).abs() < 1e-4);
    }

    #[test]
    fn polygon_degenerate_vertices_are_graceful() {
        let p = Sketch2dPrimitive::Polygon {
            vertices: vec![[0.0, 0.0], [1.0, 0.0]],
        };
        assert!(p.sdf([5.0, 5.0]).is_infinite());
    }

    #[test]
    fn aabb_of_primitives_matches_extents() {
        assert_eq!(
            Sketch2dPrimitive::Circle {
                center: [1.0, 2.0],
                radius: 3.0
            }
            .aabb(),
            Aabb2 {
                min: [-2.0, -1.0],
                max: [4.0, 5.0]
            },
        );
        assert_eq!(
            Sketch2dPrimitive::Rectangle {
                center: [0.0, 0.0],
                half_extents: [5.0, 2.0]
            }
            .aabb(),
            Aabb2 {
                min: [-5.0, -2.0],
                max: [5.0, 2.0]
            },
        );
        assert_eq!(
            Sketch2dPrimitive::Polygon {
                vertices: vec![[0.0, 0.0], [4.0, 0.0], [2.0, 3.0]],
            }
            .aabb(),
            Aabb2 {
                min: [0.0, 0.0],
                max: [4.0, 3.0]
            },
        );
    }

    // ─── composition ───────────────────────────────────────────────

    fn circle(x: f32, y: f32, r: f32) -> Sketch2d {
        Sketch2d::Prim(Sketch2dPrimitive::Circle {
            center: [x, y],
            radius: r,
        })
    }

    #[test]
    fn difference_of_two_circles_gives_a_ring() {
        let ring = Sketch2d::Difference(Box::new(circle(0.0, 0.0, 10.0)), Box::new(circle(0.0, 0.0, 5.0)));
        // Between the circles → negative (inside the ring).
        assert!(ring.sdf([7.0, 0.0]) < 0.0);
        // At the ring centre → positive (we're in the hole).
        assert!(ring.sdf([0.0, 0.0]) > 0.0);
        // Far outside → positive.
        assert!(ring.sdf([50.0, 0.0]) > 0.0);
    }

    #[test]
    fn union_of_two_disjoint_circles_is_their_min() {
        let a = circle(0.0, 0.0, 5.0);
        let b = circle(100.0, 0.0, 5.0);
        let u = Sketch2d::Union(Box::new(a), Box::new(b));
        assert!(u.sdf([0.0, 0.0]) < 0.0);
        assert!(u.sdf([100.0, 0.0]) < 0.0);
        // Halfway between — outside both, distance is 45.
        assert!((u.sdf([50.0, 0.0]) - 45.0).abs() < 1e-4);
    }

    #[test]
    fn offset_dilates_and_erodes() {
        let base = circle(0.0, 0.0, 5.0);
        let dilated = Sketch2d::Offset {
            inner: Box::new(base.clone()),
            distance: 2.0,
        };
        // Original radius 5; dilated by 2 → effective radius 7.
        assert!((dilated.sdf([7.0, 0.0])).abs() < 1e-5);
        let eroded = Sketch2d::Offset {
            inner: Box::new(base),
            distance: -2.0,
        };
        // Effective radius 3.
        assert!((eroded.sdf([3.0, 0.0])).abs() < 1e-5);
    }

    // ─── BVH equivalence ───────────────────────────────────────────

    fn many_holes_sketch() -> Sketch2d {
        // 100×100 plate with a 10×10 grid of 2 mm holes spaced
        // 8 mm apart = 100 holes total. Matches the README's
        // "200-hole polygon" spirit without dragging cargo test
        // run time into the seconds.
        let plate = Sketch2d::Prim(Sketch2dPrimitive::Rectangle {
            center: [0.0, 0.0],
            half_extents: [50.0, 50.0],
        });
        let mut scene = plate;
        for gx in 0..10 {
            for gy in 0..10 {
                let cx = -36.0 + gx as f32 * 8.0;
                let cy = -36.0 + gy as f32 * 8.0;
                let hole = Sketch2d::Prim(Sketch2dPrimitive::Circle {
                    center: [cx, cy],
                    radius: 2.0,
                });
                scene = Sketch2d::Difference(Box::new(scene), Box::new(hole));
            }
        }
        scene
    }

    #[test]
    fn bvh_sdf_matches_naive_sdf_on_100_hole_scene() {
        let sketch = many_holes_sketch();
        let bvh = BoundedSketch2d::from_sketch(&sketch);

        // Sample a mix of deterministic locations: solid interior,
        // inside a hole, on a hole edge, outside the plate, and
        // a scatter of pseudo-random points across the AABB.
        let mut samples: Vec<[f32; 2]> = vec![
            [0.0, 0.0],
            [-36.0, -36.0], // hole centre
            [-34.0, -36.0], // hole surface
            [50.0, 0.0],    // plate edge
            [100.0, 0.0],   // far outside
            [-100.0, -100.0],
        ];
        // Deterministic pseudo-random scatter (LCG).
        let mut seed: u32 = 0xDEAD_BEEF;
        for _ in 0..200 {
            seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
            let x = ((seed as i32) as f32 / i32::MAX as f32) * 60.0;
            seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
            let y = ((seed as i32) as f32 / i32::MAX as f32) * 60.0;
            samples.push([x, y]);
        }

        let mut worst = 0.0_f32;
        for p in samples {
            let naive = sketch.sdf(p);
            let bvh_val = bvh.sdf(p);
            let delta = (naive - bvh_val).abs();
            if delta > worst {
                worst = delta;
            }
        }
        assert!(worst < 1e-3, "BVH drifted from naive by {worst}");
    }

    #[test]
    fn distance_to_aabb2_is_zero_inside_and_euclidean_outside() {
        let bb = Aabb2 {
            min: [-10.0, -10.0],
            max: [10.0, 10.0],
        };
        assert_eq!(distance_point_to_aabb2([0.0, 0.0], bb), 0.0);
        assert_eq!(distance_point_to_aabb2([10.0, 0.0], bb), 0.0);
        let d = distance_point_to_aabb2([13.0, 14.0], bb);
        assert!((d - 5.0).abs() < 1e-4, "{d}");
    }
}
