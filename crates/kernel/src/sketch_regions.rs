//! Sketch region detection via an imprint arrangement (#412).
//!
//! The atomic imprint faces of a sketch — the set of ALL its curves (open trim
//! lines included) subdivides the plane — come from
//! [`crate::sketch_arrangement`]. This module is the thin adapter that exposes
//! them as [`SketchRegion`] metadata and resolves a click point to a region
//! index, both consumed by the WASM API (`listSketchRegions`,
//! `regionQueryAtPoint`) and the profile-query lowering.

use fielddraft_protocol::sketch2d::Aabb2;
use fielddraft_protocol::FeatureCall;
use serde::Serialize;

/// One atomic imprint face of a sketch profile.
#[derive(Debug, Clone, Serialize)]
pub struct SketchRegion {
    /// Zero-based region index in the deterministic arrangement order.
    pub index: u32,
    /// Approximate centroid in sketch-plane coordinates. NB: for a non-convex
    /// region (a ring) the centroid can fall in the HOLE, so it is NOT a
    /// reliable inside point — use [`SketchRegion::interior`] for membership.
    pub centroid: [f32; 2],
    /// A point GUARANTEED to lie inside the region (outside any holes). Used to
    /// address this one region when restricting an extrude/revolve profile.
    pub interior: [f32; 2],
    /// Axis-aligned bounding box of the region's outer boundary.
    pub aabb: Aabb2,
    /// Approximate area in mm².
    pub area: f32,
}

/// Analyse a sketch's raw sub-calls and return all atomic imprint faces, in the
/// deterministic order [`crate::sketch_arrangement::arrange`] defines.
pub fn find_regions(sub_calls: &[FeatureCall]) -> Vec<SketchRegion> {
    crate::sketch_arrangement::arrange(sub_calls)
        .into_iter()
        .enumerate()
        .map(|(i, f)| SketchRegion {
            index: i as u32,
            centroid: f.centroid,
            interior: f.interior,
            aabb: f.aabb,
            area: f.area,
        })
        .collect()
}

/// Index of the atomic region containing the sketch-space point `p`, or `None`
/// when `p` is outside every region. The returned index addresses the same
/// region a `qRegion(sketch_id, i)` query resolves to (both go through the same
/// arrangement order). When holes overlap the click, the smallest-area region
/// wins (the most specific face).
pub fn region_index_at_point(sub_calls: &[FeatureCall], p: [f32; 2]) -> Option<u32> {
    let regions = crate::sketch_arrangement::arrange(sub_calls);
    let mut best: Option<(u32, f32)> = None;
    for (i, f) in regions.iter().enumerate() {
        if f.profile.sdf(p) < 0.0 {
            match best {
                Some((_, area)) if area <= f.area => {}
                _ => best = Some((i as u32, f.area)),
            }
        }
    }
    best.map(|(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fielddraft_protocol::{FeatureCall, FeatureId};
    use serde_json::json;

    fn id(n: u32) -> FeatureId {
        FeatureId::from_u32(n)
    }
    fn line(n: u32, a: [f32; 2], b: [f32; 2]) -> FeatureCall {
        FeatureCall::new("sketch_line", id(n), json!({ "start": a, "end": b }))
    }
    fn circle(n: u32, c: [f32; 2], r: f32) -> FeatureCall {
        FeatureCall::new("sketch_circle", id(n), json!({ "center": c, "radius": r }))
    }
    /// A closed rectangle drawn as four lines.
    fn rect_lines(base: u32, lo: [f32; 2], hi: [f32; 2]) -> Vec<FeatureCall> {
        vec![
            line(base, [lo[0], lo[1]], [hi[0], lo[1]]),
            line(base + 1, [hi[0], lo[1]], [hi[0], hi[1]]),
            line(base + 2, [hi[0], hi[1]], [lo[0], hi[1]]),
            line(base + 3, [lo[0], hi[1]], [lo[0], lo[1]]),
        ]
    }

    #[test]
    fn empty_sketch_produces_no_regions() {
        assert!(find_regions(&[]).is_empty());
    }

    #[test]
    fn single_circle_produces_one_region() {
        let regions = find_regions(&[circle(1, [0.0, 0.0], 10.0)]);
        assert_eq!(regions.len(), 1);
        assert!(regions[0].area > 200.0); // π·10² ≈ 314
    }

    #[test]
    fn rectangle_with_internal_divider_splits_into_two() {
        // (a) A rectangle plus one internal separator line → 2 regions.
        let mut calls = rect_lines(1, [0.0, 0.0], [20.0, 10.0]);
        calls.push(line(5, [10.0, 0.0], [10.0, 10.0])); // vertical divider
        let regions = find_regions(&calls);
        assert_eq!(regions.len(), 2, "divider splits the rectangle");
        // Each half is inside exactly its own region.
        assert!(region_index_at_point(&calls, [5.0, 5.0]).is_some());
        assert!(region_index_at_point(&calls, [15.0, 5.0]).is_some());
        assert_ne!(
            region_index_at_point(&calls, [5.0, 5.0]),
            region_index_at_point(&calls, [15.0, 5.0]),
            "the two halves are distinct regions"
        );
    }

    #[test]
    fn t_junction_line_divides_and_overshoot_adds_nothing() {
        // (b) The divider crosses the rectangle and OVERSHOOTS past both edges.
        // The two crossings are genuine T-junctions (they meet the edges'
        // interiors, not corners), so the rectangle splits in two; the two
        // overshooting stubs hang free and must not add regions of their own.
        let mut calls = rect_lines(1, [0.0, 0.0], [20.0, 10.0]);
        calls.push(line(5, [10.0, -7.0], [10.0, 17.0]));
        let regions = find_regions(&calls);
        assert_eq!(regions.len(), 2, "split in two; the stubs bound no area");
        assert_ne!(
            region_index_at_point(&calls, [5.0, 5.0]),
            region_index_at_point(&calls, [15.0, 5.0]),
        );
        // Beside a free-hanging stub there is no region at all.
        assert_eq!(region_index_at_point(&calls, [10.5, -3.0]), None);
    }

    #[test]
    fn nested_rectangle_imprints_a_frame_and_an_inner_face() {
        // A rectangle fully inside another → frame + inner face, NOT one merged
        // region. Verifies the containment/hole handling on straight edges.
        let mut calls = rect_lines(1, [0.0, 0.0], [40.0, 40.0]);
        calls.extend(rect_lines(10, [10.0, 10.0], [30.0, 30.0]));
        let regions = find_regions(&calls);
        assert_eq!(regions.len(), 2, "frame + inner");
        let inner = region_index_at_point(&calls, [20.0, 20.0]).unwrap();
        let frame = region_index_at_point(&calls, [5.0, 20.0]).unwrap();
        assert_ne!(inner, frame);
        let faces = crate::sketch_arrangement::arrange(&calls);
        // The frame really has a hole where the inner face sits.
        assert!(faces[frame as usize].profile.sdf([20.0, 20.0]) > 0.0);
        assert!(faces[frame as usize].profile.sdf([5.0, 20.0]) < 0.0);
    }

    #[test]
    fn a_picked_circle_region_keeps_its_exact_curved_profile() {
        // An un-cut curved source is re-emitted as its exact segment, so a
        // picked disc extrudes to a true cylinder rather than a polygon.
        let calls = vec![circle(1, [0.0, 0.0], 10.0)];
        let faces = crate::sketch_arrangement::arrange(&calls);
        assert_eq!(faces.len(), 1);
        // Sample the profile around the rim: an exact circle is within f32
        // noise of radius 10 everywhere, a 64-gon would cut the corners.
        for k in 0..32 {
            let a = std::f32::consts::TAU * (k as f32) / 32.0;
            let p = [9.99 * a.cos(), 9.99 * a.sin()];
            assert!(
                faces[0].profile.sdf(p) < 0.0,
                "just inside the rim must be inside at angle {a}"
            );
        }
    }

    #[test]
    fn dangling_curve_does_not_divide() {
        // (c) A line that ends FREE in the interior does not separate — the
        // flood runs around its tip, so the rectangle stays one region.
        let mut calls = rect_lines(1, [0.0, 0.0], [20.0, 10.0]);
        calls.push(line(5, [10.0, 0.0], [10.0, 6.0])); // stops short of the top edge
        let regions = find_regions(&calls);
        assert_eq!(regions.len(), 1, "a dangling divider does not imprint");
    }

    #[test]
    fn nested_circles_give_ring_and_inner() {
        // (d) Concentric circles → the annular ring AND the inner disc as two
        // distinct regions (the biggest behaviour change vs the old union model).
        let calls = vec![circle(1, [0.0, 0.0], 20.0), circle(2, [0.0, 0.0], 10.0)];
        let regions = find_regions(&calls);
        assert_eq!(regions.len(), 2, "ring + inner disc");
        // The inner point sits only in the inner disc; a ring point only in the ring.
        let inner = region_index_at_point(&calls, [0.0, 0.0]).unwrap();
        let ring = region_index_at_point(&calls, [15.0, 0.0]).unwrap();
        assert_ne!(inner, ring);
        // The ring's profile must carry the hole; the inner disc must be solid.
        let ring_face = &crate::sketch_arrangement::arrange(&calls)[ring as usize];
        assert!(ring_face.profile.sdf([15.0, 0.0]) < 0.0, "in the annulus");
        assert!(ring_face.profile.sdf([0.0, 0.0]) > 0.0, "hole preserved");
        let inner_face = &crate::sketch_arrangement::arrange(&calls)[inner as usize];
        assert!(inner_face.profile.sdf([0.0, 0.0]) < 0.0, "inner disc solid");
    }

    #[test]
    fn ordering_is_deterministic_and_stable() {
        // (e) Two runs of the same geometry produce identical region metadata.
        let mut calls = rect_lines(1, [0.0, 0.0], [30.0, 10.0]);
        calls.push(line(5, [10.0, 0.0], [10.0, 10.0]));
        calls.push(line(6, [20.0, 0.0], [20.0, 10.0]));
        let a = find_regions(&calls);
        let b = find_regions(&calls);
        assert_eq!(a.len(), 3);
        for (ra, rb) in a.iter().zip(b.iter()) {
            assert_eq!(ra.aabb.min, rb.aabb.min);
            assert!((ra.area - rb.area).abs() < 1e-3);
        }
        // Order is left-to-right by aabb.min.x (all share min.y = 0).
        assert!(a[0].aabb.min[0] <= a[1].aabb.min[0]);
        assert!(a[1].aabb.min[0] <= a[2].aabb.min[0]);
    }

    #[test]
    fn a_closed_path_contour_is_arranged_like_any_other_loop() {
        // `sketch_path` (SVG import, glyphs, B-spline tool) contributes its
        // contour chain to the arrangement — two nested contours imprint into
        // a frame plus an inner face.
        let calls = vec![FeatureCall::new(
            "sketch_path",
            id(1),
            json!({ "contours": [
                { "start": [0.0, 0.0], "closed": true, "segments": [
                    { "kind": "line", "to": [40.0, 0.0] },
                    { "kind": "line", "to": [40.0, 40.0] },
                    { "kind": "line", "to": [0.0, 40.0] }
                ]},
                { "start": [10.0, 10.0], "closed": true, "segments": [
                    { "kind": "line", "to": [30.0, 10.0] },
                    { "kind": "line", "to": [30.0, 30.0] },
                    { "kind": "line", "to": [10.0, 30.0] }
                ]}
            ]}),
        )];
        let regions = find_regions(&calls);
        assert_eq!(regions.len(), 2, "outer frame + inner face");
        assert_ne!(
            region_index_at_point(&calls, [20.0, 20.0]),
            region_index_at_point(&calls, [5.0, 20.0]),
        );
    }

    #[test]
    fn mirror_and_offset_sketches_report_no_regions() {
        // `sketch_mirror` / `sketch_offset` rewrite the accumulated profile
        // TREE, so an arrangement over the raw entities would omit their
        // effect. Such sketches report NO regions on purpose — the caller then
        // uses the whole-sketch profile instead of a wrong sub-region.
        let mut calls = rect_lines(1, [0.0, 0.0], [10.0, 10.0]);
        assert_eq!(find_regions(&calls).len(), 1, "plain rectangle arranges");
        calls.push(FeatureCall::new("sketch_mirror", id(9), json!({ "axis": 0 })));
        assert!(
            find_regions(&calls).is_empty(),
            "a mirrored sketch declines region subdivision"
        );
    }

    #[test]
    fn two_separate_rectangles_are_two_regions() {
        let mut calls = rect_lines(1, [-30.0, 0.0], [-20.0, 10.0]);
        calls.extend(rect_lines(10, [20.0, 0.0], [30.0, 10.0]));
        assert_eq!(find_regions(&calls).len(), 2);
    }
}
