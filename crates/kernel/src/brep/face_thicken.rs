//! Derive a thin-shell solid from a **curved** analytic B-Rep face's surface.
//!
//! This is the kernel half of "thicken a face" — the curved counterpart to
//! [`crate::brep::face_profile`] (which extrudes a *planar* face). Onshape's
//! Thicken takes a curved face's analytic surface and offsets it along its
//! normal by a wall thickness to make a thin solid that follows the curvature.
//!
//! ## Reduction (same trick as planar extrude)
//!
//! For the analytic surfaces whose normal offset is a pure parameter shift, a
//! thickened wall is just the **difference of two coaxial offset solid
//! primitives**, so it reuses the whole solid pipeline (SDF, weave, booleans)
//! for free — exactly like a planar face extrude reused [`crate::Primitive::Extrude`]:
//!
//! - `Sphere(r)`      → `Sphere(r+out) − Sphere(r−in)`         (radial normal)
//! - `Cylinder(r)`    → `Cyl(r+out, h) − Cyl(r−in, h)`         (radial normal;
//!   the axial extent `h` is recovered from the face's rim loops)
//! - `Torus(R, r)`    → `Torus(R, r+out) − Torus(R, r−in)`     (tube-radial normal)
//! - `Cone(α)`        → `Frustum(r±Δ) − Frustum(r∓Δ)`          (slant normal;
//!   a *normal* offset `t` is a *radial* shift `Δ = t/cos α` of both rim radii,
//!   leaving the half-angle unchanged — a parallel cone. The frustum's two rim
//!   radii + axial span are recovered from the rim loops; DL-76's apex-correct
//!   cone weave makes those rims exist and the offset frustums weave watertight.)
//!
//! Closed surfaces (sphere, torus) need no boundary at all. A cylinder or cone
//! wall needs its axial span, read off the rim loops in surface-local `y`, plus a
//! full-revolution check (a cut / partial wall would be overshot by a difference
//! of *full* solids, so it is deferred).
//!
//! ## Coordinate spaces
//!
//! The returned [`ThickenSolid::outer`]/`inner` primitives live in the face's
//! **surface-local** frame (the frame in which [`HalfSpace`] is centred), and
//! [`ThickenSolid::world`] maps that frame to world (`placement ∘
//! face.transform`). The caller wraps the `Difference` of the two primitives in
//! `world` as the feature placement — so a single-face thicken keeps a
//! separable placement (DL-56 move-cache).
//!
//! ## Scaled placements
//!
//! The offsets are given in world millimetres but the offset solids are built in
//! the surface-local frame, so a uniform body scale `s` is divided out
//! (`δ_local = t / s`) — a thicken reads the same wall thickness whatever the
//! body's scale. A *non-uniform* scale is rejected: the offset of these surfaces
//! under anisotropic scale is not an analytic primitive.
//!
//! ## Up-to-face targets
//!
//! Instead of a fixed thickness the wall can grow up to a coaxial **target**
//! face: [`offset_to_target`] measures the signed world-mm normal gap between a
//! source surface and a coaxial target of the *same* analytic kind (concentric
//! sphere, coaxial cylinder, coaxial torus) and turns it into the `(outer, inner)`
//! offsets [`thicken_face`] already consumes — so the whole reduction above is
//! reused verbatim. The gap's sign picks the direction (target outside → outward
//! wall, inside → inward). Cones pair with coaxial same-half-angle cones (their
//! parallel surfaces have the constant gap `δ_apex·sin α`). A non-coaxial or
//! type-mismatched target is rejected (its normal gap is not a constant offset,
//! so the wall would not be a difference of two coaxial primitives).
//!
//! ## Partial (cut) walls
//!
//! A cut cylinder or cone wall (a half-pipe) is bounded by a [`SectorTrim`]:
//! the rim azimuths reveal the angular gap, and the caller intersects the
//! full-revolution difference with an extruded [`Sector`] prism over the
//! wall's own interval. Partial spheres/tori remain out of scope (their cut
//! boundary is not an axial wedge).
//!
//! ## Out of scope — each returns a descriptive `Err`
//!
//! Planar faces (use Extrude with `symmetric`), tessellated/TPMS faces,
//! partial spheres/tori.

use fielddraft_protocol::{HalfSpace, Primitive, Transform};

use crate::brep::{BRep, FaceId, Loop, SurfaceGeometry};

/// The two coaxial offset solids whose difference is the thickened wall, plus
/// the world placement that maps their surface-local frame to world.
#[derive(Debug, Clone)]
pub struct ThickenSolid {
    /// Outer (grown along +normal) solid in surface-local coordinates.
    pub outer: Primitive,
    /// Inner (shrunk along −normal) solid — subtracted to leave the wall.
    pub inner: Primitive,
    /// Surface-local → world placement (`placement ∘ face.transform`).
    pub world: Transform,
    /// Angular trim for a PARTIAL (cut) cylinder / cone wall: `None` for a
    /// full revolution. The caller intersects the wall with the sector prism
    /// this describes (see [`SectorTrim`]), so a half-pipe thickens to a
    /// half-tube instead of overshooting to the full ring.
    pub sector: Option<SectorTrim>,
}

/// The angular wedge a partial revolution wall is trimmed to, in the same
/// surface-local frame as [`ThickenSolid::outer`]/`inner` (azimuth measured
/// around the local +Y axis, `θ = atan2(z, x)`). Realised as a
/// [`Primitive::Extrude`] of a [`Sketch2dPrimitive::Sector`] whose extrude
/// axis is rotated onto the surface-local +Y.
#[derive(Debug, Clone, Copy)]
pub struct SectorTrim {
    /// Wall start azimuth (radians, `atan2(z, x)` in the surface frame).
    pub start_angle: f32,
    /// CCW angular extent of the wall (radians, `< τ`).
    pub sweep_angle: f32,
    /// Radial reach of the trim prism — comfortably beyond the outer solid.
    pub radius: f32,
    /// Axial slab centre (surface-local `y`).
    pub center_y: f32,
    /// Axial half-height of the prism — with slack beyond the wall's rims,
    /// so the intersection never shaves the tube's own caps.
    pub half_height: f32,
}

/// Build the offset-solid pair for a curved analytic face. `outer_off` grows
/// the surface along +normal (outside the source solid), `inner_off` shrinks it
/// along −normal; both are world millimetres and `≥ 0`. Returns a descriptive
/// `Err` for planar, tessellated, partial-revolution, non-uniformly-scaled or
/// degenerate faces.
pub fn thicken_face(
    brep: &BRep,
    face_id: FaceId,
    placement: &Transform,
    outer_off: f32,
    inner_off: f32,
) -> Result<ThickenSolid, String> {
    let SurfaceGeometry::Analytic { surface, transform } = &brep.face(face_id).surface else {
        return Err("face is tessellated (TPMS / non-analytic) — cannot thicken".to_string());
    };
    // Surface-local → world. The face transform places the centred HalfSpace
    // into the body, the body placement into the world. The cone arm folds its
    // axial centre into `world` (a `Primitive::Cone` is always origin-centred),
    // so it is mutable.
    let mut world = crate::features::transforms::compose_placement(placement, transform);

    // Eikonal-renormalise the offsets to world millimetres. The offset solids
    // are built in the surface-local frame and then placed by `world`, which may
    // carry a body scale `s`: a local radius delta `δ` shows up as `s·δ` in the
    // model. So a *world* wall thickness `t` is a *local* shift `t/s`. Without
    // this, a 2×-scaled body would render a 2 mm thicken as a 4 mm wall.
    //
    // A *non-uniform* scale is rejected: the normal offset of a sphere (or any
    // of these surfaces) under anisotropic scale is an offset ellipsoid, not an
    // analytic primitive — the whole parameter-shift reduction breaks. Honest
    // error beats silently-wrong geometry.
    let (outer_off, inner_off) = {
        let s = uniform_scale(&world)?;
        (outer_off / s, inner_off / s)
    };

    let mut sector: Option<SectorTrim> = None;
    let (outer, inner) = match surface {
        HalfSpace::Plane { .. } => {
            return Err(
                "planar face — use Extrude (Symmetric) to thicken a flat face".to_string()
            );
        }
        HalfSpace::Sphere { radius } => {
            let r = *radius;
            guard_inner(r, inner_off)?;
            (
                Primitive::Sphere { center: [0.0, 0.0, 0.0], radius: r + outer_off },
                Primitive::Sphere { center: [0.0, 0.0, 0.0], radius: r - inner_off },
            )
        }
        HalfSpace::Torus { major_radius, minor_radius } => {
            let r = *minor_radius;
            guard_inner(r, inner_off)?;
            (
                Primitive::Torus {
                    center: [0.0, 0.0, 0.0],
                    major_radius: *major_radius,
                    minor_radius: r + outer_off,
                },
                Primitive::Torus {
                    center: [0.0, 0.0, 0.0],
                    major_radius: *major_radius,
                    minor_radius: r - inner_off,
                },
            )
        }
        HalfSpace::InfiniteCylinder { radius } => {
            let r = *radius;
            guard_inner(r, inner_off)?;
            // The HalfSpace cylinder is infinite along local Y; the face's rim
            // loops give the real axial slab to bound the tube. A partial
            // (cut) wall additionally reports its angular interval, realised
            // below as a sector trim.
            let (cy, hh, span) = cylinder_axial_extent(brep, face_id, transform)?;
            sector = span.map(|(start_angle, sweep_angle)| SectorTrim {
                start_angle,
                sweep_angle,
                radius: r + outer_off + 0.5 * r.max(1.0),
                center_y: cy,
                half_height: hh * 1.5 + 1.0,
            });
            (
                Primitive::Cylinder {
                    center: [0.0, cy, 0.0],
                    radius: r + outer_off,
                    half_height: hh,
                },
                Primitive::Cylinder {
                    center: [0.0, cy, 0.0],
                    radius: r - inner_off,
                    half_height: hh,
                },
            )
        }
        HalfSpace::Cone { half_angle } => {
            // The face transform's origin is the cone apex (DL-76); the rim loops
            // give the axial slab and the two rim radii. A *normal* offset `t` is
            // a *radial* shift `Δ = t / cos α` of both rims (parallel cone, same
            // half-angle).
            let (cy, hh, r_bottom, r_top, span) = cone_frustum_extent(brep, face_id, transform)?;
            let cos_a = half_angle.cos().abs().max(1e-4);
            let d_out = outer_off / cos_a;
            let d_in = inner_off / cos_a;
            // The narrower rim limits how far the wall can move inward.
            guard_inner(r_bottom.min(r_top), d_in)?;
            // A `Primitive::Cone` is origin-centred (no `center` field), so fold
            // the frustum's axial centre into the placement: translate by `cy`
            // along the surface-local axis, then map to world.
            world = crate::features::transforms::compose_placement(
                &world,
                &Transform { translate: [0.0, cy, 0.0], ..Transform::IDENTITY },
            );
            // The frustum is re-centred by the fold above, so the sector trim
            // (same folded frame) sits at axial centre 0.
            sector = span.map(|(start_angle, sweep_angle)| SectorTrim {
                start_angle,
                sweep_angle,
                radius: r_bottom.max(r_top) + d_out + 0.5 * r_bottom.max(r_top).max(1.0),
                center_y: 0.0,
                half_height: hh * 1.5 + 1.0,
            });
            (
                Primitive::Cone {
                    r_bottom: r_bottom + d_out,
                    r_top: r_top + d_out,
                    half_height: hh,
                },
                Primitive::Cone {
                    r_bottom: r_bottom - d_in,
                    r_top: r_top - d_in,
                    half_height: hh,
                },
            )
        }
        HalfSpace::Gyroid { .. } => {
            return Err("gyroid / TPMS face cannot be thickened analytically".to_string());
        }
        // The thicken result must be a Primitive pair; a revolved profile
        // group has no primitive counterpart (yet).
        HalfSpace::RevolvedProfile { .. } => {
            return Err("revolved profile face cannot be thickened analytically".to_string());
        }
    };
    Ok(ThickenSolid { outer, inner, world, sector })
}

/// Reject an inward offset that would consume the surface (radius ≤ 0): the
/// inner wall would invert / self-intersect.
fn guard_inner(radius: f32, inner_off: f32) -> Result<(), String> {
    if radius - inner_off <= 1e-4 {
        return Err(format!(
            "inward thickness {inner_off} ≥ surface radius {radius} — wall would self-intersect"
        ));
    }
    Ok(())
}

/// Verify a surface-local → world placement carries a **uniform**, non-degenerate
/// scale and return its signed magnitude `s` (local length × `s` = world length).
/// The analytic normal-offset reduction only holds under uniform scale — an
/// anisotropic scale turns these surfaces' offsets into non-primitive (ellipsoid)
/// shapes — so a non-uniform scale is an honest `Err` rather than silently-wrong
/// geometry.
fn uniform_scale(world: &Transform) -> Result<f32, String> {
    let s = world.scale;
    let uniform = (s[0] - s[1]).abs() < 1e-4 * s[0].abs().max(1.0)
        && (s[1] - s[2]).abs() < 1e-4 * s[1].abs().max(1.0);
    if !uniform {
        return Err(format!(
            "face has a non-uniform scale ({:.3}, {:.3}, {:.3}) — its normal offset is not an \
             analytic primitive; apply a uniform scale or thicken before scaling",
            s[0], s[1], s[2]
        ));
    }
    if s[0].abs() < 1e-6 {
        return Err("face placement scale is degenerate (zero) — cannot thicken".to_string());
    }
    Ok(s[0])
}

/// A source/target analytic surface lifted to world space for the up-to-face
/// mode: its [`HalfSpace`] (with surface-local params), its surface-local → world
/// placement, and the uniform world scale magnitude.
struct WorldSurface {
    kind: HalfSpace,
    world: Transform,
    /// World scale magnitude (`local_radius × s_abs = world_radius`).
    s_abs: f32,
}

/// Lift a face's analytic surface to world space (see [`WorldSurface`]). Errors
/// for tessellated faces and non-uniform / degenerate scales.
fn world_surface(brep: &BRep, face_id: FaceId, placement: &Transform) -> Result<WorldSurface, String> {
    let SurfaceGeometry::Analytic { surface, transform } = &brep.face(face_id).surface else {
        return Err("face is tessellated (TPMS / non-analytic) — cannot use as a thicken target".to_string());
    };
    let world = crate::features::transforms::compose_placement(placement, transform);
    let s_abs = uniform_scale(&world)?.abs();
    Ok(WorldSurface { kind: surface.clone(), world, s_abs })
}

/// Compute the `(outer_off, inner_off)` world-millimetre offsets that grow a
/// `source` face's surface up to a coaxial `target` face — the up-to-face thicken
/// mode. Both surfaces must be the **same** analytic kind and coaxial; the signed
/// normal gap decides the direction (target outside → outward wall, inside →
/// inward). Supported: sphere, cylinder, torus, cone (same half-angle). A type
/// mismatch, a non-coaxial target or a coincident target each returns a
/// descriptive `Err`.
///
/// The returned offsets are world millimetres, so the caller feeds them straight
/// into [`thicken_face`] (which renormalises them by the source's own scale) — the
/// outer/inner solid then reaches exactly the target surface.
pub fn offset_to_target(
    src_brep: &BRep,
    src_face: FaceId,
    src_placement: &Transform,
    tgt_brep: &BRep,
    tgt_face: FaceId,
    tgt_placement: &Transform,
) -> Result<(f32, f32), String> {
    let src = world_surface(src_brep, src_face, src_placement)?;
    let tgt = world_surface(tgt_brep, tgt_face, tgt_placement)?;
    let gap = coaxial_gap(&src, &tgt)?;
    if gap.abs() < 1e-4 {
        return Err(
            "target face is coincident with the source surface — there is no wall to fill".to_string(),
        );
    }
    if gap > 0.0 {
        Ok((gap, 0.0)) // target outside the source → grow outward
    } else {
        Ok((0.0, -gap)) // target inside → grow inward
    }
}

/// The signed world-mm normal gap `target − source` between two coaxial analytic
/// surfaces of the same kind. Errors when the kinds differ, the half-angles of a
/// cone pair differ, or the two surfaces are not coaxial.
fn coaxial_gap(src: &WorldSurface, tgt: &WorldSurface) -> Result<f32, String> {
    match (&src.kind, &tgt.kind) {
        (HalfSpace::Sphere { radius: rs }, HalfSpace::Sphere { radius: rt }) => {
            let rs_w = rs * src.s_abs;
            let rt_w = rt * tgt.s_abs;
            let tol = 1e-3 * rs_w.max(rt_w).max(1.0);
            if dist(src.world.translate, tgt.world.translate) > tol {
                return Err(
                    "source and target spheres are not concentric — up-to-face needs a coaxial target"
                        .to_string(),
                );
            }
            Ok(rt_w - rs_w)
        }
        (HalfSpace::InfiniteCylinder { radius: rs }, HalfSpace::InfiniteCylinder { radius: rt }) => {
            let rs_w = rs * src.s_abs;
            let rt_w = rt * tgt.s_abs;
            require_coaxial_axis(src, tgt, 1e-3 * rs_w.max(rt_w).max(1.0))?;
            Ok(rt_w - rs_w)
        }
        (
            HalfSpace::Torus { major_radius: rmaj_s, minor_radius: rs },
            HalfSpace::Torus { major_radius: rmaj_t, minor_radius: rt },
        ) => {
            let rmaj_s_w = rmaj_s * src.s_abs;
            let rmaj_t_w = rmaj_t * tgt.s_abs;
            let tol = 1e-3 * rmaj_s_w.max(rmaj_t_w).max(1.0);
            require_coaxial_axis(src, tgt, tol)?;
            if dist(src.world.translate, tgt.world.translate) > tol
                || (rmaj_s_w - rmaj_t_w).abs() > tol
            {
                return Err(
                    "source and target tori are not coaxial with an equal major radius".to_string(),
                );
            }
            Ok(rt * tgt.s_abs - rs * src.s_abs)
        }
        (HalfSpace::Cone { half_angle: aa }, HalfSpace::Cone { half_angle: ab }) => {
            // Two coaxial cones with the SAME half-angle are parallel
            // surfaces; the constant normal gap is the axial apex offset
            // times sin α (radial gap Δr = δ·tan α at any height, normal gap
            // = Δr·cos α). Different half-angles → non-constant gap → no
            // uniform wall; honest error.
            if (aa - ab).abs() > 1e-3 {
                return Err(
                    "source and target cones have different half-angles — their gap is not a                      constant wall thickness"
                        .to_string(),
                );
            }
            // Axes parallel, same opening direction, and colinear.
            let ds = crate::quat_rotate(src.world.rotation, [0.0, 1.0, 0.0]);
            let dt = crate::quat_rotate(tgt.world.rotation, [0.0, 1.0, 0.0]);
            let dot = ds[0] * dt[0] + ds[1] * dt[1] + ds[2] * dt[2];
            if dot < 1.0 - 1e-3 {
                return Err(
                    "source and target cone axes are not aligned — up-to-face needs a coaxial                      same-direction target"
                        .to_string(),
                );
            }
            let scale_hint = dist(src.world.translate, tgt.world.translate).max(1.0);
            require_coaxial_axis(src, tgt, 1e-3 * scale_hint)?;
            // Apexes are the surface-frame origins (DL-76); the surface
            // frame's +Y points from the base toward the apex, so a target
            // apex FURTHER ALONG +Y than the source's means a larger target
            // radius at every height → outward gap (verified empirically by
            // `cone_up_to_face_measures_the_parallel_gap`).
            let w = [
                tgt.world.translate[0] - src.world.translate[0],
                tgt.world.translate[1] - src.world.translate[1],
                tgt.world.translate[2] - src.world.translate[2],
            ];
            let delta_apex = w[0] * ds[0] + w[1] * ds[1] + w[2] * ds[2];
            Ok(delta_apex * aa.sin().abs())
        }
        (HalfSpace::Cone { .. }, _) | (_, HalfSpace::Cone { .. }) => Err(
            "source and target must both be cones for a conical up-to-face wall".to_string(),
        ),
        (HalfSpace::Plane { .. }, _) | (_, HalfSpace::Plane { .. }) => Err(
            "planar faces are not a curved thicken source/target — use Extrude".to_string(),
        ),
        _ => Err(
            "source and target faces must be the same coaxial analytic kind \
             (sphere/cylinder/torus) for up-to-face"
                .to_string(),
        ),
    }
}

/// Require two surfaces' local-Y axes to be the same world line (parallel and
/// colinear within `tol`). Used for the cylinder/torus up-to-face targets.
fn require_coaxial_axis(src: &WorldSurface, tgt: &WorldSurface, tol: f32) -> Result<(), String> {
    let ds = crate::quat_rotate(src.world.rotation, [0.0, 1.0, 0.0]);
    let dt = crate::quat_rotate(tgt.world.rotation, [0.0, 1.0, 0.0]);
    let dot = ds[0] * dt[0] + ds[1] * dt[1] + ds[2] * dt[2];
    if dot.abs() < 1.0 - 1e-3 {
        return Err(
            "source and target axes are not parallel — up-to-face needs a coaxial target".to_string(),
        );
    }
    // Perpendicular distance between the two axis lines: the component of the
    // centre-to-centre vector orthogonal to the (unit) axis direction.
    let ps = src.world.translate;
    let pt = tgt.world.translate;
    let w = [pt[0] - ps[0], pt[1] - ps[1], pt[2] - ps[2]];
    let wd = w[0] * ds[0] + w[1] * ds[1] + w[2] * ds[2];
    let perp = [w[0] - wd * ds[0], w[1] - wd * ds[1], w[2] - wd * ds[2]];
    let off = (perp[0] * perp[0] + perp[1] * perp[1] + perp[2] * perp[2]).sqrt();
    if off > tol {
        return Err(
            "source and target axes are offset — up-to-face needs a coaxial target".to_string(),
        );
    }
    Ok(())
}

/// Euclidean distance between two points.
fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Recover a cylindrical wall's axial centre + half-height (surface-local `y`)
/// from its rim loops, plus the wall's angular interval when it is a PARTIAL
/// (cut) revolution (`None` for a full one). The partial interval feeds the
/// [`SectorTrim`] so the wall difference is bounded to the actual cut.
fn cylinder_axial_extent(
    brep: &BRep,
    face_id: FaceId,
    surf: &Transform,
) -> Result<(f32, f32, Option<(f32, f32)>), String> {
    // body-local → surface-local (the frame the HalfSpace is centred in).
    let inv = crate::inverse(surf);
    let face = brep.face(face_id);

    let mut lo = f32::INFINITY;
    let mut hi = f32::NEG_INFINITY;
    let mut thetas: Vec<f32> = Vec::new();
    let mut sample_loop = |lp: &Loop| {
        for &(edge_id, _reversed) in &lp.edges {
            for p in crate::brep::edge_sampling::sample_edge(&brep.edge(edge_id).geometry) {
                let pl = place_point(&inv, [p[0] as f32, p[1] as f32, p[2] as f32]);
                lo = lo.min(pl[1]);
                hi = hi.max(pl[1]);
                // Azimuth around the cylinder axis (local Y); the seam line all
                // shares one θ but the rims must sweep the full circle.
                thetas.push(pl[2].atan2(pl[0]));
            }
        }
    };
    sample_loop(&face.outer);
    for hole in &face.inner {
        sample_loop(hole);
    }

    if !lo.is_finite() || !hi.is_finite() {
        return Err("cylindrical face has no boundary rim to size the wall".to_string());
    }
    let hh = (hi - lo) * 0.5;
    if hh <= 1e-4 {
        return Err("cylindrical face is degenerate (zero axial height)".to_string());
    }
    let span = revolution_span(&mut thetas)?;
    Ok(((lo + hi) * 0.5, hh, span))
}

/// Recover a conical wall's frustum parameters — axial centre + half-height and
/// the two rim radii (`r_bottom` at the lower `y`, `r_top` at the upper) — in the
/// **apex-local** surface frame (the cone's transform origin is its apex, DL-76),
/// and verify the wall does not straddle the apex. A PARTIAL (cut) wall
/// additionally reports its angular interval for the [`SectorTrim`].
/// Returns `(center_y, half_height, r_bottom, r_top, span)`.
fn cone_frustum_extent(
    brep: &BRep,
    face_id: FaceId,
    surf: &Transform,
) -> Result<(f32, f32, f32, f32, Option<(f32, f32)>), String> {
    // body-local → surface-local (apex-centred frame the HalfSpace is in).
    let inv = crate::inverse(surf);
    let face = brep.face(face_id);

    let mut lo = f32::INFINITY;
    let mut hi = f32::NEG_INFINITY;
    let mut r_at_lo = 0.0_f32;
    let mut r_at_hi = 0.0_f32;
    let mut thetas: Vec<f32> = Vec::new();
    let mut sample_loop = |lp: &Loop| {
        for &(edge_id, _reversed) in &lp.edges {
            for p in crate::brep::edge_sampling::sample_edge(&brep.edge(edge_id).geometry) {
                let pl = place_point(&inv, [p[0] as f32, p[1] as f32, p[2] as f32]);
                let r = (pl[0] * pl[0] + pl[2] * pl[2]).sqrt();
                if pl[1] < lo {
                    lo = pl[1];
                    r_at_lo = r;
                }
                if pl[1] > hi {
                    hi = pl[1];
                    r_at_hi = r;
                }
                thetas.push(pl[2].atan2(pl[0]));
            }
        }
    };
    sample_loop(&face.outer);
    for hole in &face.inner {
        sample_loop(hole);
    }

    if !lo.is_finite() || !hi.is_finite() {
        return Err("conical face has no boundary rim to size the wall".to_string());
    }
    // Both rims must sit on the same nappe — a wall that spanned the apex (apex at
    // surface-local y = 0) would be overshot by a difference of full cones.
    if lo < -1e-4 && hi > 1e-4 {
        return Err("conical face spans the apex — cannot thicken as a single frustum".to_string());
    }
    let hh = (hi - lo) * 0.5;
    if hh <= 1e-4 {
        return Err("conical face is degenerate (zero axial height)".to_string());
    }
    let span = revolution_span(&mut thetas)?;
    Ok(((lo + hi) * 0.5, hh, r_at_lo, r_at_hi, span))
}

/// Classify the rim azimuths: a full revolution (no large gap) → `None`; a
/// partial (cut) wall with ONE clear angular gap → `Some((start, sweep))`,
/// the wall's own angular interval (the complement of the gap). The gap
/// endpoints come from the samples of the wall's axial boundary edges, which
/// sit exactly on the cut planes, so the interval is as accurate as the
/// tracer's refinement.
fn revolution_span(thetas: &mut [f32]) -> Result<Option<(f32, f32)>, String> {
    if thetas.len() < 3 {
        return Err("cylindrical face boundary is too sparse to verify revolution".to_string());
    }
    thetas.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = thetas.len();
    // Largest interior gap and the wrap-around gap between last and first.
    let mut max_gap = std::f32::consts::TAU - (thetas[n - 1] - thetas[0]);
    let mut gap_end = thetas[0]; // wall starts where the gap ends
    let mut wrap = true;
    for i in 0..n - 1 {
        let g = thetas[i + 1] - thetas[i];
        if g > max_gap {
            max_gap = g;
            gap_end = thetas[i + 1];
            wrap = false;
        }
    }
    // ~34°: comfortably above the ~22.5° spacing of a 16-sample rim, well below
    // the ≥90° wedge a real cut would remove.
    if max_gap <= 0.6 {
        return Ok(None); // full revolution
    }
    let sweep = std::f32::consts::TAU - max_gap;
    let start = if wrap { thetas[0] } else { gap_end };
    Ok(Some((start, sweep)))
}

/// Apply a [`Transform`] (scale → rotate → translate) to a point. Mirrors the
/// mapping in [`crate::brep::face_profile`] / `face_plane_frame`.
fn place_point(t: &Transform, p: [f32; 3]) -> [f32; 3] {
    let scaled = [p[0] * t.scale[0], p[1] * t.scale[1], p[2] * t.scale[2]];
    let r = crate::quat_rotate(t.rotation, scaled);
    [r[0] + t.translate[0], r[1] + t.translate[1], r[2] + t.translate[2]]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_revolution_accepts_a_dense_circle() {
        // 24 samples around a full circle → max gap ≈ 15° < threshold.
        let mut thetas: Vec<f32> = (0..24)
            .map(|i| (i as f32 / 24.0) * std::f32::consts::TAU - std::f32::consts::PI)
            .collect();
        assert_eq!(revolution_span(&mut thetas).unwrap(), None);
    }

    #[test]
    fn revolution_span_reports_a_quarter_wedge_gap() {
        // A circle missing a 90° wedge: samples over [0°, 270°] inclusive —
        // the wall spans 270°, the gap is the remaining 90°.
        let mut thetas: Vec<f32> = (0..=18)
            .map(|i| (i as f32 / 24.0) * std::f32::consts::TAU)
            .collect();
        let (start, sweep) = revolution_span(&mut thetas).unwrap().expect("partial");
        assert!((start - 0.0).abs() < 1e-5, "wall starts at 0°, got {start}");
        assert!(
            (sweep - 0.75 * std::f32::consts::TAU).abs() < 1e-5,
            "wall sweeps 270°, got {sweep}"
        );
    }

    #[test]
    fn revolution_span_handles_a_wrap_straddling_wall() {
        // Wall over [−45°, +45°] (crossing the ±π seam is the gap's job
        // here: samples close around 0, the gap spans the far side).
        let mut thetas: Vec<f32> = (0..=12)
            .map(|i| -std::f32::consts::FRAC_PI_4 + (i as f32 / 12.0) * std::f32::consts::FRAC_PI_2)
            .collect();
        let (start, sweep) = revolution_span(&mut thetas).unwrap().expect("partial");
        assert!(
            (start + std::f32::consts::FRAC_PI_4).abs() < 1e-5,
            "wall starts at −45°, got {start}"
        );
        assert!(
            (sweep - std::f32::consts::FRAC_PI_2).abs() < 1e-5,
            "wall sweeps 90°, got {sweep}"
        );
    }

    use crate::brep::octree::Octree;
    use crate::brep::weaving::weave;
    use crate::bvh::BoundedCsg;
    use crate::csg::CsgNode;
    use fielddraft_protocol::FeatureId;

    /// Weave a body and find its first face carrying the given surface test.
    fn find_face(
        brep: &BRep,
        pred: impl Fn(&HalfSpace) -> bool,
    ) -> Option<FaceId> {
        (0..brep.faces.len()).map(|i| FaceId(i as u32)).find(|&fid| {
            matches!(
                &brep.face(fid).surface,
                SurfaceGeometry::Analytic { surface, .. } if pred(surface)
            )
        })
    }

    #[test]
    fn partial_cylinder_wall_thickens_with_sector_trim() {
        // Half-pipe: a cylinder with its x < 0 half cut away. The outer wall
        // is a 180° partial revolution — before the sector trim this was a
        // descriptive error; now it thickens to a half-tube.
        let cyl = CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Cylinder {
                center: [0.0, 0.0, 0.0],
                radius: 10.0,
                half_height: 10.0,
            },
        };
        let cut = CsgNode::Leaf {
            id: FeatureId::from_u32(2),
            primitive: Primitive::Box {
                center: [-20.0, 0.0, 0.0],
                half_extents: [20.0, 25.0, 25.0],
            },
        };
        let body = BoundedCsg::from_csg(&CsgNode::Difference(Box::new(cyl), Box::new(cut)));
        let octree = Octree::build(&body, 0.5);
        let brep = weave(&body, &octree);
        let wall = find_face(&brep, |hs| matches!(hs, HalfSpace::InfiniteCylinder { .. }))
            .expect("half-pipe must have a cylindrical wall face");

        let s = thicken_face(&brep, wall, &Transform::IDENTITY, 2.0, 0.0)
            .expect("partial wall must thicken");
        let t = s.sector.expect("partial wall must carry a sector trim");
        // The kept wall spans the x ≥ 0 half: θ ∈ [−90°, +90°].
        assert!(
            (t.sweep_angle - std::f32::consts::PI).abs() < 0.05,
            "half-pipe sweeps ~180°, got {}",
            t.sweep_angle
        );
        assert!(
            (t.start_angle + std::f32::consts::FRAC_PI_2).abs() < 0.05,
            "wall starts at ~−90°, got {}",
            t.start_angle
        );

        // Field-level acceptance: material inside the kept half-wall,
        // nothing on the cut-away side.
        let node = crate::features::thicken::thicken_diff_node(
            &FeatureId::from_u32(9),
            0,
            &s,
        );
        let solid =
            BoundedCsg::from_csg(&CsgNode::Transform(s.world, Box::new(node)));
        let f_wall = solid.sdf([11.0, 0.0, 0.0], crate::csg::EvalMode::Logical);
        assert!(f_wall < 0.0, "wall material expected at (11,0,0), f = {f_wall}");
        let f_cut = solid.sdf([-11.0, 0.0, 0.0], crate::csg::EvalMode::Logical);
        assert!(f_cut > 0.0, "no material expected at (−11,0,0), f = {f_cut}");
        // Axial caps intact: nothing far above the tube.
        let f_above = solid.sdf([11.0, 12.0, 0.0], crate::csg::EvalMode::Logical);
        assert!(f_above > 0.0, "no material above the tube, f = {f_above}");
    }

    #[test]
    fn cone_up_to_face_measures_the_parallel_gap() {
        // Two coaxial frustums with the SAME half-angle (slope 0.5): B's rims
        // sit 2 mm radially outside A's, i.e. a normal gap of 2·cos α.
        let mk = |id: u32, r_b: f32, r_t: f32| CsgNode::Leaf {
            id: FeatureId::from_u32(id),
            primitive: Primitive::Cone { r_bottom: r_b, r_top: r_t, half_height: 5.0 },
        };
        let body_a = BoundedCsg::from_csg(&mk(1, 10.0, 5.0));
        let body_b = BoundedCsg::from_csg(&mk(2, 12.0, 7.0));
        let brep_a = weave(&body_a, &Octree::build(&body_a, 0.4));
        let brep_b = weave(&body_b, &Octree::build(&body_b, 0.4));
        let face_a = find_face(&brep_a, |hs| matches!(hs, HalfSpace::Cone { .. }))
            .expect("cone A wall");
        let face_b = find_face(&brep_b, |hs| matches!(hs, HalfSpace::Cone { .. }))
            .expect("cone B wall");

        let (outer, inner) = offset_to_target(
            &brep_a,
            face_a,
            &Transform::IDENTITY,
            &brep_b,
            face_b,
            &Transform::IDENTITY,
        )
        .expect("coaxial same-angle cones must measure a gap");
        // Δr = 2, slope tan α = 0.5 → cos α = 1/√1.25 → normal gap ≈ 1.7889.
        let expect = 2.0 / (1.0_f32 + 0.25).sqrt();
        assert!(
            (outer - expect).abs() < 0.05 && inner == 0.0,
            "outward gap ≈ {expect}, got ({outer}, {inner})"
        );

        // The reverse direction grows inward by the same amount.
        let (outer_rev, inner_rev) = offset_to_target(
            &brep_b,
            face_b,
            &Transform::IDENTITY,
            &brep_a,
            face_a,
            &Transform::IDENTITY,
        )
        .expect("reverse gap");
        assert!(
            outer_rev == 0.0 && (inner_rev - expect).abs() < 0.05,
            "inward gap ≈ {expect}, got ({outer_rev}, {inner_rev})"
        );
    }

    #[test]
    fn place_point_round_trips_through_inverse() {
        let t = Transform {
            translate: [3.0, -2.0, 1.0],
            rotation: crate::transform::euler_xyz_to_quat(10.0, 25.0, -40.0),
            scale: [1.0, 1.0, 1.0],
        };
        let p = [1.5, 0.5, -0.7];
        let there = place_point(&t, p);
        let back = place_point(&crate::inverse(&t), there);
        for k in 0..3 {
            assert!((back[k] - p[k]).abs() < 1e-4, "axis {k}: {} vs {}", back[k], p[k]);
        }
    }
}
