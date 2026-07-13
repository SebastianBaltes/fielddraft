//! M1 lateral trim — **analytic half-space wedge**: bound a thickened wall to
//! exactly the picked face by intersecting the offset band with the wall's own
//! angular sector, expressed as a `Primitive::Extrude` of a wedge polygon.
//!
//! ## Why analytic, not a swept mesh
//!
//! FieldDraft is an SDF kernel: a Bézier wall already *has* an exact field
//! ([`Field::ExtrudeWall`](crate::sdf_dual) — the DL-77 cap-free signed profile,
//! built on the protocol's quadratic-only sketch segments). The earlier cut
//! materialised the face's edge-loop as a triangle-mesh prism
//! (`Primitive::ImportedMesh`) and trimmed with it. That threw away the kernel's
//! defining property — the mesh-SDF *sign* (Fast Winding Number) degrades to
//! O(triangles) for the near-surface query points that cluster exactly along the
//! trim seam, so every octree cell on the seam paid a per-cell mesh sweep and the
//! weave never finished. It is also conceptually wrong: extrude/thicken on a
//! quadratic profile must compose as an SDF, never a mesh.
//!
//! ## What actually bounds one wall
//!
//! An extrude side wall = one G1-continuous run of the profile contour swept along
//! the axis. Its B-Rep patch is a 4-edge Kantenzug: two **vertical corner edges**
//! (the profile corners extruded) plus the top/bottom profile curves at
//! `z = ±half_height`. So the wall's angular extent is bounded by exactly two
//! planes — one through each corner, perpendicular to the wall's boundary tangent
//! there (containing the extrude axis) — and its z-extent by the two cap planes.
//!
//! Those four planes are precisely a vertical **prism over a wedge polygon**:
//! clip a large square by the two corner half-planes (giving the angular sector),
//! then `Primitive::Extrude { sketch: wedge, half_depth_z: half_height }` adds the
//! cap planes for free. The trim is then `Intersection(band, wedge_prism)` — all
//! analytic, O(1) per sample, resolution-independent, no BVH / winding number.
//!
//! The corners and their tangents are read straight off the face's woven
//! edge-loop, so the G1 grouping the weave already performed is honoured for free
//! (no need to re-derive it from the sketch).
//!
//! ## Scope (M1 first cut)
//!
//! The only **bounded** non-analytic (`Tessellated`) faces the weave produces are
//! **extrude side walls** (a revolve is 360° → closed; a non-half-space leaf —
//! sphere / torus — is closed). So the trim runs for an `Extrude` carrier only;
//! a closed carrier (no edge-loop) falls through to the plain M0 band.

use fielddraft_protocol::{
    FeatureId, Primitive, Sketch2d, Sketch2dPrimitive, SketchSegment, Transform,
};

use crate::brep::{BRep, FaceId};
use crate::csg::CsgNode;

/// A picked face's edge-loops sampled into closed **body-local** polylines: one
/// outer boundary plus zero or more hole boundaries. Each polyline omits its
/// closing duplicate (`pts[0] != pts[last]`); a closed (boundary-free) face
/// yields an empty `outer`.
#[derive(Debug, Clone, Default)]
pub struct FaceLoops {
    pub outer: Vec<[f64; 3]>,
    pub inner: Vec<Vec<[f64; 3]>>,
}

impl FaceLoops {
    /// A face with no real boundary (closed surface) → nothing to trim.
    pub fn is_closed(&self) -> bool {
        self.outer.len() < 3
    }
}

/// Sample the picked face's loops into body-local polylines (the frame the B-Rep
/// is woven in — DL-56). Walks each [`Loop`](crate::brep::Loop) in edge order,
/// honouring the per-edge `reversed` flag, and welds consecutive duplicate join
/// points between edges and at the closing wrap.
pub fn sample_face_loops(brep: &BRep, fid: FaceId) -> FaceLoops {
    let face = brep.face(fid);
    let outer = sample_loop(brep, &face.outer);
    let inner = face.inner.iter().map(|l| sample_loop(brep, l)).collect();
    FaceLoops { outer, inner }
}

fn sample_loop(brep: &BRep, lp: &crate::brep::Loop) -> Vec<[f64; 3]> {
    let mut pts: Vec<[f64; 3]> = Vec::new();
    for &(eid, reversed) in &lp.edges {
        let mut seg = crate::brep::edge_sampling::sample_edge(&brep.edge(eid).geometry);
        if reversed {
            seg.reverse();
        }
        for p in seg {
            if pts.last().is_none_or(|q| dist2(*q, p) > WELD_EPS2) {
                pts.push(p);
            }
        }
    }
    // Drop the closing duplicate (the loop wraps back onto its start).
    if pts.len() > 1 && dist2(pts[0], *pts.last().unwrap()) <= WELD_EPS2 {
        pts.pop();
    }
    pts
}

/// Squared weld tolerance for consecutive loop points (body-local mm²).
const WELD_EPS2: f64 = 1e-12;

/// Build the lateral-trim node for a picked face whose source carrier is an
/// extrude, in the **band-local** frame (ready for `Intersection(band, trim)`).
///
/// * `carrier` — the source feature's single-primitive carrier (pre-remap); only
///   an `Extrude` (optionally wrapped in one affine `Transform`) is trimmed, any
///   other carrier returns `None` (closed → plain M0 band).
/// * `band_world` — the carrier-local → world placement of the band node
///   ([`OffsetBand::world`](crate::brep::face_offset::OffsetBand::world)).
/// * `placement` — the source body's body-local → world placement (folds the
///   sampled loops to world).
/// * `loops` — the picked face's [`FaceLoops`] (body-local).
/// * `out` / `inn` — the world-mm band offsets along ±normal (size the wedge).
/// * `trim_id` — feature id the wedge leaf is keyed under.
///
/// Returns `None` when there is nothing to trim (non-extrude carrier, a closed
/// face, or a loop whose two corners could not be recovered); otherwise a
/// `Transform(extrude-local → band-local, Extrude(wedge))` node.
pub fn lateral_trim_node(
    carrier: &CsgNode,
    band_world: &Transform,
    placement: &Transform,
    loops: &FaceLoops,
    out: f32,
    inn: f32,
    trim_id: &FeatureId,
) -> Option<CsgNode> {
    if loops.is_closed() {
        return None;
    }
    let (inner_xf, half_height, sketch) = extrude_carrier(carrier)?;
    let walls = profile_walls(sketch);
    if walls.is_empty() {
        return None;
    }
    // The extrude's own local frame in world: band(carrier-local→world) ∘ the
    // carrier's affine wrapper (extrude-local → carrier-local). Then `inner_xf`
    // alone maps extrude-local → band-local (the frame the trim must live in).
    let extrude_world = crate::compose_transforms(band_world, &inner_xf);

    // Fold the picked loop into the extrude-local profile plane (XY) — its z
    // (the extrude axis) is irrelevant to *which wall* it is.
    let el_xy: Vec<[f64; 2]> = loops
        .outer
        .iter()
        .map(|p| {
            let pw = local_to_world(placement, [p[0] as f32, p[1] as f32, p[2] as f32]);
            let pl = crate::world_to_local(&extrude_world, pw);
            [pl[0] as f64, pl[1] as f64]
        })
        .collect();
    if el_xy.len() < 3 {
        return None;
    }

    // Identify which profile wall the picked loop lies on (its XY points sit on
    // that wall's contour — robust even when the woven loop's order is degenerate)
    // and read its two corners + into-wall tangents straight off the quadratics.
    let wall = best_match_wall(&walls, &el_xy)?;
    let (c0, t0) = (wall.c0, wall.t0);
    let (c1, t1) = (wall.c1, wall.t1);

    // Wedge polygon = a generous square clipped by the two corner half-planes.
    let reach = el_xy
        .iter()
        .map(|p| p[0].abs().max(p[1].abs()))
        .fold(0.0_f64, f64::max)
        + (out + inn) as f64;
    let s = reach * 3.0 + 10.0;
    let square = vec![[-s, -s], [s, -s], [s, s], [-s, s]];
    let poly = clip_halfplane(&clip_halfplane(&square, c0, t0), c1, t1);
    if poly.len() < 3 {
        return None;
    }
    let vertices: Vec<[f32; 2]> = poly.iter().map(|p| [p[0] as f32, p[1] as f32]).collect();

    let leaf = CsgNode::Leaf {
        id: trim_id.child("lat_trim".to_string()),
        primitive: Primitive::Extrude {
            sketch: Sketch2d::Prim(Sketch2dPrimitive::Polygon { vertices }),
            half_depth_z: half_height,
            center: [0.0, 0.0, 0.0],
            wall_subs: None,
        },
    };
    Some(CsgNode::Transform(inner_xf, Box::new(leaf)))
}

/// Peel an extrude carrier (optionally under one affine `Transform`) into its
/// extrude-local → carrier-local placement, half-height, and 2D profile. `None`
/// for anything that is not a lone extrude.
fn extrude_carrier(node: &CsgNode) -> Option<(Transform, f32, &Sketch2d)> {
    match node {
        CsgNode::Extrude { half_height, sketch, .. } => {
            Some((Transform::IDENTITY, *half_height, sketch))
        }
        CsgNode::Transform(t, inner) => match inner.as_ref() {
            CsgNode::Extrude { half_height, sketch, .. } => Some((*t, *half_height, sketch)),
            _ => None,
        },
        _ => None,
    }
}

/// One side wall of the extrude: a maximal G1-continuous run of profile segments
/// between two corners, with each corner's in-plane tangent pointing **into** the
/// wall (the cut-plane normal — the kept half-plane is `(x − corner)·t ≥ 0`).
struct Wall {
    c0: [f64; 2],
    t0: [f64; 2],
    c1: [f64; 2],
    t1: [f64; 2],
    segs: Vec<SketchSegment>,
}

/// Split a closed profile contour into its side walls at every G1 break (a join
/// where the incoming and outgoing unit tangents diverge past
/// [`PATH_G1_COS_THRESHOLD`]) — the same grouping the weave uses to emit one face
/// per wall. A smooth (break-free) or non-`Path` profile yields no walls (→ the
/// caller falls back to the untrimmed M0 band).
fn profile_walls(sketch: &Sketch2d) -> Vec<Wall> {
    let Sketch2d::Prim(Sketch2dPrimitive::Path { segments, closed }) = sketch else {
        return Vec::new();
    };
    let n = segments.len();
    if !*closed || n == 0 {
        return Vec::new();
    }
    // Join after segment `i` is a corner when its tangents are not G1-aligned.
    let cos = fielddraft_protocol::sketch2d::PATH_G1_COS_THRESHOLD as f64;
    let is_corner = |i: usize| {
        let a = segments[i].tangent_at_end();
        let b = segments[(i + 1) % n].tangent_at_start();
        (a[0] * b[0] + a[1] * b[1]) as f64 <= cos
    };
    let breaks: Vec<usize> = (0..n).filter(|&i| is_corner(i)).collect();
    if breaks.len() < 2 {
        return Vec::new(); // smooth loop / single corner → no isolable wall
    }

    let mut walls = Vec::with_capacity(breaks.len());
    for w in 0..breaks.len() {
        let start = (breaks[w] + 1) % n; // first segment after this corner
        let end = breaks[(w + 1) % breaks.len()]; // last segment up to the next corner
        let mut segs = Vec::new();
        let mut i = start;
        loop {
            segs.push(segments[i]);
            if i == end {
                break;
            }
            i = (i + 1) % n;
        }
        let first = segs.first().unwrap();
        let last = segs.last().unwrap();
        let c0 = f2(first.start_point());
        let c1 = f2(last.end_point());
        let t0 = f2(first.tangent_at_start()); // out of c0, into the wall
        let te = last.tangent_at_end();
        let t1 = [-(te[0] as f64), -(te[1] as f64)]; // out of c1, into the wall
        walls.push(Wall { c0, t0, c1, t1, segs });
    }
    walls
}

/// Pick the wall whose contour the picked loop's XY points lie on — the one
/// minimising the summed unsigned distance from the loop points to the wall's
/// segments. `None` if the wall set is empty or a wall is degenerate (coincident
/// corners).
fn best_match_wall<'a>(walls: &'a [Wall], el_xy: &[[f64; 2]]) -> Option<&'a Wall> {
    walls
        .iter()
        .filter(|w| (w.c0[0] - w.c1[0]).powi(2) + (w.c0[1] - w.c1[1]).powi(2) > 1e-12)
        .min_by(|a, b| fit_cost(a, el_xy).total_cmp(&fit_cost(b, el_xy)))
}

/// Summed nearest-segment unsigned distance from the loop points to a wall.
fn fit_cost(w: &Wall, el_xy: &[[f64; 2]]) -> f64 {
    el_xy
        .iter()
        .map(|p| {
            let q = [p[0] as f32, p[1] as f32];
            w.segs
                .iter()
                .map(|s| s.unsigned_distance(q) as f64)
                .fold(f64::INFINITY, f64::min)
        })
        .sum()
}

fn f2(p: [f32; 2]) -> [f64; 2] {
    [p[0] as f64, p[1] as f64]
}

/// Sutherland–Hodgman clip of a convex polygon by the half-plane
/// `(p − c)·t ≥ 0` (the side the unit normal `t` points to).
fn clip_halfplane(poly: &[[f64; 2]], c: [f64; 2], t: [f64; 2]) -> Vec<[f64; 2]> {
    let f = |p: [f64; 2]| (p[0] - c[0]) * t[0] + (p[1] - c[1]) * t[1];
    let m = poly.len();
    let mut out: Vec<[f64; 2]> = Vec::with_capacity(m + 2);
    for i in 0..m {
        let a = poly[i];
        let b = poly[(i + 1) % m];
        let (fa, fb) = (f(a), f(b));
        if fa >= 0.0 {
            out.push(a);
        }
        if (fa >= 0.0) != (fb >= 0.0) {
            let s = fa / (fa - fb);
            out.push([a[0] + s * (b[0] - a[0]), a[1] + s * (b[1] - a[1])]);
        }
    }
    out
}

/// Local → world point transform: `s · R · p + t` (mirror of [`world_to_local`]).
fn local_to_world(t: &Transform, p: [f32; 3]) -> [f32; 3] {
    let s = [p[0] * t.scale[0], p[1] * t.scale[1], p[2] * t.scale[2]];
    let r = crate::quat_rotate(t.rotation, s);
    [r[0] + t.translate[0], r[1] + t.translate[1], r[2] + t.translate[2]]
}

fn dist2(a: [f64; 3], b: [f64; 3]) -> f64 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Even-odd point-in-polygon for the wedge tests.
    fn inside(poly: &[[f32; 2]], p: [f32; 2]) -> bool {
        let mut c = false;
        let n = poly.len();
        let mut j = n - 1;
        for i in 0..n {
            let (a, b) = (poly[i], poly[j]);
            if (a[1] > p[1]) != (b[1] > p[1]) {
                let x = a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
                if p[0] < x {
                    c = !c;
                }
            }
            j = i;
        }
        c
    }

    fn wedge_polygon(node: &CsgNode) -> Vec<[f32; 2]> {
        match node {
            CsgNode::Transform(_, inner) => match inner.as_ref() {
                CsgNode::Leaf {
                    primitive:
                        Primitive::Extrude {
                            sketch: Sketch2d::Prim(Sketch2dPrimitive::Polygon { vertices }),
                            ..
                        },
                    ..
                } => vertices.clone(),
                other => panic!("expected an Extrude(Polygon) leaf, got {other:?}"),
            },
            other => panic!("expected a Transform-wrapped wedge, got {other:?}"),
        }
    }

    /// The four-segment blob profile from the M1 end-to-end test.
    fn blob_sketch() -> Sketch2d {
        let qb = |p0: [f32; 2], p1: [f32; 2], p2: [f32; 2]| SketchSegment::QuadBezier { p0, p1, p2 };
        Sketch2d::Prim(Sketch2dPrimitive::Path {
            segments: vec![
                qb([3.0, 0.0], [4.0, 4.0], [0.0, 3.0]),
                qb([0.0, 3.0], [-4.0, 4.0], [-3.0, 0.0]),
                qb([-3.0, 0.0], [-4.0, -4.0], [0.0, -3.0]),
                qb([0.0, -3.0], [4.0, -4.0], [3.0, 0.0]),
            ],
            closed: true,
        })
    }

    fn blob_carrier() -> CsgNode {
        CsgNode::Extrude { wall_subs: None,
            id: FeatureId::from_u32(1),
            sketch: Box::new(blob_sketch()),
            half_height: 2.0,
        }
    }

    /// The blob has four corners → four walls, each a single quadratic.
    #[test]
    fn profile_walls_split_at_every_g1_break() {
        let walls = profile_walls(&blob_sketch());
        assert_eq!(walls.len(), 4, "four G1 breaks → four side walls");
        for w in &walls {
            assert_eq!(w.segs.len(), 1, "each blob wall is one quadratic");
        }
    }

    /// Picking the loop on wall-1 (the (3,0)→(0,3) quadratic) builds a wedge that
    /// holds that wall's midpoint and excludes the three neighbouring walls.
    #[test]
    fn wedge_isolates_one_walls_angular_sector() {
        // A loop lying on wall-1's contour (body-local == extrude-local here).
        let outer = vec![
            [3.0, 0.0, -2.0],
            [3.0, 0.0, 2.0],
            [2.75, 2.75, 2.0], // ≈ the quadratic's midpoint
            [0.0, 3.0, 2.0],
            [0.0, 3.0, -2.0],
            [2.75, 2.75, -2.0],
        ];
        let loops = FaceLoops { outer, inner: vec![] };
        let node = lateral_trim_node(
            &blob_carrier(),
            &Transform::IDENTITY,
            &Transform::IDENTITY,
            &loops,
            1.0,
            0.5,
            &FeatureId::from_u32(2),
        )
        .expect("a bounded extrude wall is trimmable");
        let poly = wedge_polygon(&node);

        assert!(inside(&poly, [2.1, 2.1]), "wall-1's own midpoint is inside the wedge");
        assert!(!inside(&poly, [2.1, -2.1]), "wall-4 (shares corner (3,0)) is excluded");
        assert!(!inside(&poly, [-2.1, 2.1]), "wall-2 (shares corner (0,3)) is excluded");
        assert!(!inside(&poly, [-2.1, -2.1]), "the opposite wall-3 is excluded");
    }

    #[test]
    fn extrude_carrier_peels_a_wrapping_transform_and_height() {
        let raw = blob_carrier();
        let (xf, h, _) = extrude_carrier(&raw).expect("a lone extrude peels");
        assert_eq!(xf.translate, [0.0, 0.0, 0.0]);
        assert_eq!(h, 2.0);

        let placed = CsgNode::Transform(
            Transform { translate: [3.0, 0.0, 0.0], ..Transform::IDENTITY },
            Box::new(raw),
        );
        let (xf, h, _) = extrude_carrier(&placed).expect("a wrapped extrude is still an extrude");
        assert_eq!(xf.translate, [3.0, 0.0, 0.0]);
        assert_eq!(h, 2.0);

        let leaf = CsgNode::Leaf {
            id: FeatureId::from_u32(2),
            primitive: Primitive::Sphere { center: [0.0, 0.0, 0.0], radius: 1.0 },
        };
        assert!(extrude_carrier(&leaf).is_none(), "a sphere carrier is not trimmable");
    }

    #[test]
    fn clip_halfplane_keeps_the_normal_side() {
        let square = vec![[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];
        // Keep x ≥ 0 (normal +x through the origin).
        let clipped = clip_halfplane(&square, [0.0, 0.0], [1.0, 0.0]);
        assert!(clipped.iter().all(|p| p[0] >= -1e-9), "no point left of the cut");
        assert!(clipped.iter().any(|p| (p[0]).abs() < 1e-9), "the cut edge is retained");
    }
}
