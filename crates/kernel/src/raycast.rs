//! Sphere-tracing raycaster for the CSG tree.
//!
//! K2 scope: a single ray per mouse click, no BVH yet (O(N) SDF
//! evaluation per step), no bounding-box rejection. The raycaster
//! terminates on `|f(p)| < surface_eps`, then calls
//! [`CsgNode::evaluate`] once at the hit point to recover the
//! contributor set — which gives the caller back the face/edge/vertex
//! classification of the B-Rep proxy element it just clicked on.
//!
//! Later slices will tighten this up:
//!   * K3 adds BVH-accelerated stepping.
//!   * K6+ will Newton-refine edge hits onto the mathematically exact
//!     intersection curve so snaps don't land on mesh-level drift.

use crate::bvh::BoundedCsg;
use crate::csg::{Contributors, EvalMode, ProxyElement};

/// Result of a successful raycast.
#[derive(Debug, Clone, PartialEq)]
pub struct RayHit {
    pub point: [f32; 3],
    /// Distance from the ray origin along the (normalised) direction.
    pub t: f32,
    pub contributors: Contributors,
    pub element: ProxyElement,
}

/// Tunables for the raycaster.
#[derive(Debug, Clone, Copy)]
pub struct RaycastParams {
    /// Distance at which the ray is considered to have hit the
    /// surface. A few tenths of a millimetre is typical for the
    /// 300 mm build volume.
    pub surface_eps: f32,
    /// Epsilon handed to `evaluate()` at the hit point to recover
    /// contributors. Chosen slightly larger than `surface_eps` so
    /// genuine intersection curves are classified as edges even when
    /// the sphere trace overshoots them by a fraction.
    pub contributor_eps: f32,
    pub max_steps: u32,
    pub max_distance: f32,
}

impl Default for RaycastParams {
    fn default() -> Self {
        Self {
            surface_eps: 0.05,
            contributor_eps: 0.25,
            max_steps: 256,
            max_distance: 5000.0,
        }
    }
}

fn normalize(v: [f32; 3]) -> Option<[f32; 3]> {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len < 1e-12 {
        None
    } else {
        Some([v[0] / len, v[1] / len, v[2] / len])
    }
}

/// Sphere-trace a ray through `node` and return the first hit, if any.
///
/// The ray direction is normalised internally — callers can hand in
/// any non-zero vector. A zero-length direction or an origin that's
/// already deep inside a solid beyond the trace budget returns
/// `None`.
///
/// K3: walks the BVH-wrapped form so the per-step SDF uses cached
/// AABBs and pruned branches. Both the surface test and the
/// contributor lookup use [`EvalMode::Logical`] — picking always
/// wants the unsmoothed B-Rep-proxy ground truth.
pub fn raycast(
    node: &BoundedCsg,
    origin: [f32; 3],
    direction: [f32; 3],
    params: RaycastParams,
) -> Option<RayHit> {
    let dir = normalize(direction)?;
    let mut t: f32 = 0.0;
    for _ in 0..params.max_steps {
        let p = [
            origin[0] + dir[0] * t,
            origin[1] + dir[1] * t,
            origin[2] + dir[2] * t,
        ];
        let d = node.sdf(p, EvalMode::Logical);
        if d.abs() <= params.surface_eps {
            let (_, contributors) =
                node.evaluate(p, params.contributor_eps, EvalMode::Logical);
            let element = contributors.element();
            if element == ProxyElement::None {
                return None;
            }
            return Some(RayHit {
                point: p,
                t,
                contributors,
                element,
            });
        }
        // `d` may be negative if the origin happened to start inside
        // a solid — in that case we step toward the surface by the
        // absolute distance, which at worst walks a non-optimal
        // number of iterations but always converges.
        let step = d.abs().max(params.surface_eps * 0.5);
        t += step;
        if t > params.max_distance {
            break;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::csg::CsgNode;
    use fielddraft_protocol::{BoolOp, Feature, FeatureId, Primitive, Transform};

    fn bounded(features: &[Feature]) -> BoundedCsg {
        BoundedCsg::from_csg(&CsgNode::from_features(features))
    }

    fn box_feature(id: u32, center: [f32; 3], half: [f32; 3]) -> Feature {
        Feature {
            id: FeatureId::from_u32(id),
            name: format!("Box {id}"),
            op: BoolOp::Union,
            primitive: Primitive::Box {
                center,
                half_extents: half,
            },
            transform: Transform::IDENTITY,
            blend_radius: 0.0,
            chamfer_size: 0.0,
            twist_rate: 0.0,
            bend_rate: 0.0,
            displace_depth: 0.0,
            displace_freq: 0.0,
        }
    }

    fn cyl_feature(id: u32, center: [f32; 3], radius: f32, half_height: f32) -> Feature {
        Feature {
            id: FeatureId::from_u32(id),
            name: format!("Cyl {id}"),
            op: BoolOp::Union,
            primitive: Primitive::Cylinder {
                center,
                radius,
                half_height,
            },
            transform: Transform::IDENTITY,
            blend_radius: 0.0,
            chamfer_size: 0.0,
            twist_rate: 0.0,
            bend_rate: 0.0,
            displace_depth: 0.0,
            displace_freq: 0.0,
        }
    }

    #[test]
    fn ray_misses_when_aimed_away() {
        let tree =
            bounded(&[box_feature(1, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0])]);
        let hit = raycast(
            &tree,
            [0.0, 100.0, 0.0],
            [0.0, 1.0, 0.0], // straight up, away from the box
            RaycastParams::default(),
        );
        assert!(hit.is_none());
    }

    #[test]
    fn ray_from_above_hits_box_top_as_face() {
        let tree =
            bounded(&[box_feature(1, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0])]);
        let hit = raycast(
            &tree,
            [0.0, 100.0, 0.0],
            [0.0, -1.0, 0.0],
            RaycastParams::default(),
        )
        .expect("box should be hit");
        // Surface eps default is 0.05 mm, hit should be at y≈10.
        assert!((hit.point[1] - 10.0).abs() < 0.2, "hit at y={}", hit.point[1]);
        assert_eq!(hit.element, ProxyElement::Face);
        assert_eq!(hit.contributors.as_slice(), &[FeatureId::from_u32(1)]);
    }

    #[test]
    fn ray_zero_direction_returns_none() {
        let tree =
            bounded(&[box_feature(1, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0])]);
        assert!(raycast(
            &tree,
            [0.0, 100.0, 0.0],
            [0.0, 0.0, 0.0],
            RaycastParams::default(),
        )
        .is_none());
    }

    #[test]
    fn ray_through_cylinder_cap_returns_single_contributor() {
        // Union of a box and a cylinder sticking out the top. A ray
        // from directly above the cylinder axis hits the cylinder's
        // top cap well above the box, so only the cylinder should
        // show up as a contributor.
        let features = vec![
            box_feature(1, [0.0, 0.0, 0.0], [20.0, 20.0, 20.0]),
            cyl_feature(2, [0.0, 0.0, 0.0], 10.0, 30.0),
        ];
        let tree = bounded(&features);
        let hit = raycast(
            &tree,
            [0.0, 200.0, 0.0],
            [0.0, -1.0, 0.0],
            RaycastParams::default(),
        )
        .expect("should hit the cylinder cap");
        assert!(
            (hit.point[1] - 30.0).abs() < 0.3,
            "expected cap at y≈30, got {}",
            hit.point[1],
        );
        assert_eq!(hit.contributors.as_slice(), &[FeatureId::from_u32(2)]);
        assert_eq!(hit.element, ProxyElement::Face);
    }

    #[test]
    fn ray_honours_max_distance_budget() {
        let tree =
            bounded(&[box_feature(1, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0])]);
        let tight = RaycastParams {
            max_distance: 10.0,
            ..Default::default()
        };
        let hit = raycast(&tree, [0.0, 100.0, 0.0], [0.0, -1.0, 0.0], tight);
        assert!(hit.is_none(), "should not reach the box within 10 mm");
    }
}
