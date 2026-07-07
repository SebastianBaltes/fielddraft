//! General **field-offset** thicken — the SDF counterpart to the per-primitive
//! analytic [`crate::brep::face_thicken`].
//!
//! Where `face_thicken` reconstructs an analytic offset-solid pair for the four
//! "toy" surface families (sphere / cylinder / torus / cone), this module makes
//! a thin wall from *any* closed carrier field by composing two general
//! operators:
//!
//! ```text
//!     band = Shell(Offset(carrier, out), out + in)
//! ```
//!
//! `Offset(carrier, out)` dilates the carrier outward by `out`; shelling it
//! inward by `out + in` leaves the wall between `carrier + out` (outer surface)
//! and `carrier − in` (inner surface) — exactly the thickened wall. It reuses
//! the proven Shell thin-wall weave and its inner-wall (`0x800`) face
//! classification (DL-75) and the general `Offset` operator, with **no**
//! thicken-specific face machinery and no dual-offset branch collision (there is
//! a single carrier instance inside one Shell).
//!
//! For an analytic carrier the band folds back to the same concentric analytic
//! faces the per-primitive path emits (`Sphere(r+out)` / `Sphere(r−in)`, …) via
//! the branch-map `Offset`/`Shell` arms — so analytic exactness survives as an
//! automatic fast-path, not a special case. The reach the analytic path lacked
//! — a **closed non-analytic** face (a revolved or swept blob) — is the new
//! capability: its curved walls weave as selectable `Tessellated` faces.
//!
//! ## Scope (M0)
//!
//! Closed carriers only — a genuinely closed surface (sphere, full torus,
//! fully-revolved / closed-profile sweep) needs no lateral trim, so the whole
//! offset shell *is* the wall. Open / partially-revolved faces (a 90° cylinder
//! wedge, an extrude side wall, a surface patch) need the lateral bound that
//! arrives with M1; until then they stay on the analytic path or are deferred.

use fielddraft_protocol::{FeatureId, Transform};

use crate::csg::CsgNode;

/// One general field-offset wall: the band node in its local frame plus the
/// local → world placement the caller wraps it in (mirrors
/// [`crate::brep::face_thicken::ThickenSolid::world`]).
#[derive(Debug, Clone)]
pub struct OffsetBand {
    /// `Shell(Offset(carrier, out), out + in)` in the carrier's local frame.
    pub node: CsgNode,
    /// Carrier-local → world placement (`placement ∘ feature.transform`).
    pub world: Transform,
}

/// Reduce a source feature's node to a **single closed primitive carrier** — one
/// `Leaf` / `Extrude` / `Revolve`, descending through (and preserving) any
/// affine `Transform` wrappers. Returns `None` for a composite node (a boolean
/// of several leaves), whose general offset is deferred past M0: offsetting the
/// whole boolean would thicken every surface, not the picked face's carrier.
pub fn single_primitive_carrier(node: &CsgNode) -> Option<CsgNode> {
    match node {
        CsgNode::Leaf { .. } | CsgNode::Extrude { .. } | CsgNode::Revolve { .. } => {
            Some(node.clone())
        }
        CsgNode::Transform(t, inner) => {
            single_primitive_carrier(inner).map(|c| CsgNode::Transform(*t, Box::new(c)))
        }
        _ => None,
    }
}

/// Composite-aware carrier lookup: for a feature whose node is a **boolean**
/// of several single-primitive subtrees (a FieldSkript-built feature, a
/// thicken wall's own `Difference`), descend the boolean arms, collect every
/// candidate single-primitive subtree (with its `Transform` wrappers
/// preserved), and pick the one whose surface actually holds the picked
/// face: the candidate whose |SDF| is smallest across the face's loop
/// samples (feature-local). Blanket-offsetting the whole boolean would
/// thicken *every* surface; the probe pins the one carrier the face lives
/// on — the sidedness-probe pattern of the classification plan, evaluated
/// at KNOWN on-face points.
///
/// Returns `None` when no candidate's worst-sample residual clears `tol`
/// (e.g. the face lives on a `Shell`/`Offset` wrapper — a thin-wall solid
/// has two sheets and no single closed carrier; the caller reports that
/// honestly).
pub fn face_carrier(
    node: &CsgNode,
    samples_local: &[[f32; 3]],
    tol: f32,
) -> Option<CsgNode> {
    if let Some(c) = single_primitive_carrier(node) {
        return Some(c);
    }
    if samples_local.is_empty() {
        return None;
    }
    let mut candidates: Vec<CsgNode> = Vec::new();
    collect_carrier_candidates(node, &mut candidates);
    let mut best: Option<(f32, CsgNode)> = None;
    for cand in candidates {
        let worst = samples_local
            .iter()
            .map(|&p| cand.sdf(p, crate::csg::EvalMode::Logical).abs())
            .fold(0.0_f32, f32::max);
        // Strictly-better keeps the pick deterministic when two coincident
        // leaves both hold the face (either is a valid carrier).
        if worst <= tol && best.as_ref().is_none_or(|(b, _)| worst < *b) {
            best = Some((worst, cand));
        }
    }
    best.map(|(_, c)| c)
}

/// Collect every single-primitive subtree of a boolean composition,
/// preserving the `Transform` wrappers on the path down (a candidate must
/// evaluate in the same feature-local frame as the whole node).
fn collect_carrier_candidates(node: &CsgNode, out: &mut Vec<CsgNode>) {
    match node {
        CsgNode::Leaf { .. } | CsgNode::Extrude { .. } | CsgNode::Revolve { .. } => {
            out.push(node.clone());
        }
        CsgNode::Transform(t, inner) => {
            let mut sub = Vec::new();
            collect_carrier_candidates(inner, &mut sub);
            out.extend(
                sub.into_iter()
                    .map(|c| CsgNode::Transform(*t, Box::new(c))),
            );
        }
        CsgNode::Union(a, b)
        | CsgNode::Difference(a, b)
        | CsgNode::Intersection(a, b) => {
            collect_carrier_candidates(a, out);
            collect_carrier_candidates(b, out);
        }
        // Shell / Offset / smooth ops / leaves of other kinds: no closed
        // single-primitive carrier to extract — skip (an empty candidate set
        // surfaces as the caller's honest error).
        _ => {}
    }
}

/// Re-key every primitive id in a carrier subtree to a fresh child of `base`.
/// The carrier is *cloned* from the source feature's spine, so without this its
/// faces would carry the **source** feature's ids and a pick on the wall would
/// resolve to the source body, not the thicken feature. Re-keying under the
/// thicken id keeps selection/TNP pointing at the wall's own feature while the
/// wall still hangs off the source `qFace` (regeneration-stable). Only the
/// single-primitive shapes [`single_primitive_carrier`] can return appear here;
/// any other node is passed through unchanged.
pub fn remap_leaf_ids(node: CsgNode, base: &FeatureId) -> CsgNode {
    let mut counter = 0u32;
    remap_rec(node, base, &mut counter)
}

fn remap_rec(node: CsgNode, base: &FeatureId, counter: &mut u32) -> CsgNode {
    let mut fresh = || {
        let n = *counter;
        *counter += 1;
        base.child(format!("c{n}"))
    };
    match node {
        CsgNode::Leaf { primitive, .. } => CsgNode::Leaf { id: fresh(), primitive },
        CsgNode::Extrude { sketch, half_height, .. } => {
            CsgNode::Extrude { id: fresh(), sketch, half_height }
        }
        CsgNode::Revolve { sketch, .. } => CsgNode::Revolve { id: fresh(), sketch },
        CsgNode::Transform(t, inner) => {
            CsgNode::Transform(t, Box::new(remap_rec(*inner, base, counter)))
        }
        other => other,
    }
}

/// Build the general thicken band for a closed carrier. `out` / `inn` are world
/// millimetres grown along +normal / −normal (both `≥ 0`); `world` is the
/// carrier-local → world placement (its uniform scale renormalises the offsets
/// to world mm, exactly as the analytic path does). Returns an `Err` for a
/// non-uniform or degenerate placement scale (the offset of a closed field under
/// anisotropic scale is not a uniform wall).
pub fn offset_band(carrier: CsgNode, out: f32, inn: f32, world: Transform) -> Result<OffsetBand, String> {
    let s = uniform_scale(&world)?;
    let (out_l, in_l) = (out / s, inn / s);
    let node = CsgNode::Shell(
        Box::new(CsgNode::Offset(Box::new(carrier), out_l)),
        out_l + in_l,
        Vec::new(),
    );
    Ok(OffsetBand { node, world })
}

/// The uniform world-scale magnitude of a placement (local length × `s` = world
/// length). The general offset, like the analytic one, only stays a uniform-
/// thickness wall under uniform scale — an anisotropic scale would warp the band
/// — so a non-uniform or zero scale is an honest `Err`.
fn uniform_scale(world: &Transform) -> Result<f32, String> {
    let s = world.scale;
    let uniform = (s[0] - s[1]).abs() < 1e-4 * s[0].abs().max(1.0)
        && (s[1] - s[2]).abs() < 1e-4 * s[1].abs().max(1.0);
    if !uniform {
        return Err(format!(
            "face has a non-uniform scale ({:.3}, {:.3}, {:.3}) — its field offset is not a \
             uniform-thickness wall; apply a uniform scale or thicken before scaling",
            s[0], s[1], s[2]
        ));
    }
    if s[0].abs() < 1e-6 {
        return Err("face placement scale is degenerate (zero) — cannot thicken".to_string());
    }
    Ok(s[0])
}

#[cfg(test)]
mod tests {
    use super::*;
    use fielddraft_protocol::{FeatureId, Primitive};

    fn leaf(id: u32) -> CsgNode {
        CsgNode::Leaf {
            id: FeatureId::from_u32(id),
            primitive: Primitive::Sphere { center: [0.0, 0.0, 0.0], radius: 5.0 },
        }
    }

    #[test]
    fn single_carrier_accepts_a_lone_leaf_and_preserves_wrapping_transforms() {
        assert!(single_primitive_carrier(&leaf(1)).is_some());
        let placed = CsgNode::Transform(
            Transform { translate: [1.0, 2.0, 3.0], ..Transform::IDENTITY },
            Box::new(leaf(1)),
        );
        match single_primitive_carrier(&placed) {
            Some(CsgNode::Transform(t, inner)) => {
                assert_eq!(t.translate, [1.0, 2.0, 3.0]);
                assert!(matches!(*inner, CsgNode::Leaf { .. }));
            }
            other => panic!("expected the transform to be preserved, got {other:?}"),
        }
    }

    #[test]
    fn single_carrier_rejects_a_composite_node() {
        let composite = CsgNode::Union(Box::new(leaf(1)), Box::new(leaf(2)));
        assert!(
            single_primitive_carrier(&composite).is_none(),
            "a boolean carrier is deferred past M0"
        );
    }

    #[test]
    fn offset_band_is_shell_of_offset_and_renormalises_scale() {
        let band = offset_band(leaf(1), 0.8, 0.6, Transform::IDENTITY).unwrap();
        match &band.node {
            CsgNode::Shell(inner, thickness, open) => {
                assert!(open.is_empty(), "a closed band has no open faces");
                assert!((*thickness - 1.4).abs() < 1e-6, "thickness = out + in");
                assert!(matches!(**inner, CsgNode::Offset(_, d) if (d - 0.8).abs() < 1e-6));
            }
            other => panic!("expected Shell(Offset(..)), got {other:?}"),
        }

        // A 2× uniform scale halves the local offsets so the world wall is still
        // 0.8 / 0.6 mm (the band node lives in the carrier-local frame).
        let scaled = Transform { scale: [2.0, 2.0, 2.0], ..Transform::IDENTITY };
        let band2 = offset_band(leaf(1), 0.8, 0.6, scaled).unwrap();
        match &band2.node {
            CsgNode::Shell(inner, thickness, _) => {
                assert!((*thickness - 0.7).abs() < 1e-6, "local thickness halved under 2× scale");
                assert!(matches!(**inner, CsgNode::Offset(_, d) if (d - 0.4).abs() < 1e-6));
            }
            other => panic!("expected Shell(Offset(..)), got {other:?}"),
        }
    }

    #[test]
    fn remap_rekeys_primitive_ids_under_the_thicken_feature() {
        let base = FeatureId::from_u32(7);
        let placed = CsgNode::Transform(
            Transform { translate: [1.0, 0.0, 0.0], ..Transform::IDENTITY },
            Box::new(leaf(2)),
        );
        match remap_leaf_ids(placed, &base) {
            CsgNode::Transform(_, inner) => match *inner {
                CsgNode::Leaf { id, .. } => {
                    assert_eq!(id.parent().map(|p| p.to_string()), Some("7".to_string()));
                    assert_eq!(id.as_u32_or_root(), Some(7), "wall resolves to the thicken feature");
                }
                other => panic!("expected a re-keyed leaf, got {other:?}"),
            },
            other => panic!("expected the transform preserved, got {other:?}"),
        }
    }

    #[test]
    fn face_carrier_picks_the_leaf_the_face_lives_on() {
        // Difference of a big sphere and an off-centre box: samples on the
        // sphere surface must pick the sphere subtree, samples on the box
        // wall must pick the (transform-wrapped) box.
        let boxed = CsgNode::Transform(
            Transform { translate: [4.0, 0.0, 0.0], ..Transform::IDENTITY },
            Box::new(CsgNode::Leaf {
                id: FeatureId::from_u32(2),
                primitive: Primitive::Box {
                    center: [0.0, 0.0, 0.0],
                    half_extents: [2.0, 2.0, 2.0],
                },
            }),
        );
        let composite = CsgNode::Difference(Box::new(leaf(1)), Box::new(boxed));

        // Points on the sphere r=5 (away from the box).
        let on_sphere = [[-5.0, 0.0, 0.0], [0.0, 5.0, 0.0], [0.0, 0.0, 5.0]];
        match face_carrier(&composite, &on_sphere, 1e-3) {
            Some(CsgNode::Leaf { primitive: Primitive::Sphere { .. }, .. }) => {}
            other => panic!("sphere samples must pick the sphere, got {other:?}"),
        }

        // Points on the box wall x = 2 (world x = 6 via the +4 transform).
        let on_box = [[6.0, 0.0, 0.0], [6.0, 1.0, 0.5], [6.0, -1.0, -0.5]];
        match face_carrier(&composite, &on_box, 1e-3) {
            Some(CsgNode::Transform(_, inner)) => {
                assert!(matches!(*inner, CsgNode::Leaf { primitive: Primitive::Box { .. }, .. }));
            }
            other => panic!("box samples must pick the transformed box, got {other:?}"),
        }

        // Points on NEITHER surface → no carrier, honest None.
        let off = [[20.0, 0.0, 0.0]];
        assert!(face_carrier(&composite, &off, 1e-3).is_none());

        // A Shell owner exposes no closed single carrier.
        let shelled = CsgNode::Shell(Box::new(leaf(1)), 1.0, vec![]);
        assert!(
            face_carrier(&shelled, &on_sphere, 1e-3).is_none(),
            "a thin-wall owner has no single closed carrier"
        );
    }

    #[test]
    fn offset_band_rejects_non_uniform_scale() {
        let aniso = Transform { scale: [2.0, 1.0, 1.0], ..Transform::IDENTITY };
        assert!(offset_band(leaf(1), 0.8, 0.6, aniso).is_err());
    }
}
