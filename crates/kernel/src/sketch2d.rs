//! Kernel-side sketch2d bindings.
//!
//! K7.3 moved the data types `Aabb2`, `Sketch2dPrimitive` and
//! `Sketch2d` into the protocol crate so `Primitive::Extrude`
//! can carry a sketch profile directly in the wire form. This
//! module re-exports them and keeps the runtime acceleration
//! structure [`BoundedSketch2d`] — a BVH-wrapped mirror of
//! [`Sketch2d`] that caches per-node AABBs and prunes boolean
//! branches the same way [`crate::bvh::BoundedCsg`] does in 3D.

pub use fielddraft_protocol::sketch2d::{
    distance_point_to_aabb2, Aabb2, Sketch2d, Sketch2dPrimitive,
};

/// BVH-wrapped mirror of [`Sketch2d`]. Every node carries its
/// precomputed [`Aabb2`], and [`BoundedSketch2d::sdf`] uses it
/// to prune boolean branches.
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

    /// Pruned signed-distance query. Matches [`Sketch2d::sdf`]
    /// for the same input tree up to f32 rounding, but
    /// short-circuits boolean branches whose AABB can't affect
    /// the running value. Same pruning rules as the 3D version
    /// in `bvh.rs`.
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
        assert!((c.sdf([5.0, 0.0])).abs() < 1e-6);
        assert!(c.sdf([0.0, 0.0]) < 0.0);
        assert!(c.sdf([10.0, 0.0]) > 0.0);
    }

    #[test]
    fn rectangle_sdf_signs_match_point_containment() {
        let r = Sketch2dPrimitive::Rectangle {
            center: [0.0, 0.0],
            half_extents: [3.0, 2.0],
        };
        assert!(r.sdf([0.0, 0.0]) < 0.0);
        assert!(r.sdf([5.0, 0.0]) > 0.0);
    }

    #[test]
    fn bounded_sketch_sdf_matches_naive_tree_on_a_washer() {
        let outer = Sketch2d::Prim(Sketch2dPrimitive::Circle {
            center: [0.0, 0.0],
            radius: 10.0,
        });
        let inner = Sketch2d::Prim(Sketch2dPrimitive::Circle {
            center: [0.0, 0.0],
            radius: 4.0,
        });
        let washer = Sketch2d::Difference(Box::new(outer), Box::new(inner));
        let bounded = BoundedSketch2d::from_sketch(&washer);
        for (px, py) in [(0.0, 0.0), (5.0, 0.0), (3.0, 0.0), (12.0, 0.0)] {
            let a = washer.sdf([px, py]);
            let b = bounded.sdf([px, py]);
            assert!((a - b).abs() < 1e-4, "{} != {}", a, b);
        }
    }
}
