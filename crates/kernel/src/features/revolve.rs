//! `revolve` — Sketch-Profil um eine Achse rotieren, inkl. der
//! 2D-Achsen-Ausrichtungs-Mathematik (`revolve_axis_alignment`,
//! `transform_sketch_2d`).

use serde_json::Value;

use fielddraft_protocol::{BoolOp, FeatureId, Primitive, Transform};

use crate::csg::{DecodedFeature, PrimitiveModifiers};

use super::merge_scope::{resolve_merge_scope, target_ids_from_queries};
use super::{Context, FeatureError, FeatureKind, FeatureSchema, optional_display_name, optional_rotation, optional_scale};
// ─── K8.4: Revolve ───────────────────────────────────────────────
//
// Twin of ExtrudeFeature — consumes a sketch profile, but sweeps
// it around the local Y axis instead of translating it along Z.
// Lowers to Primitive::Revolve, which DecodedFeature::from_parts
// wraps in a CsgNode::Revolve. The sketch is interpreted in
// (r, y) coordinates: `x` becomes the radial distance from
// the Y axis. Authoring convention is `x ≥ 0`; a sketch that
// straddles the axis produces a valid but geometrically silly
// body (the centreline sweeps through itself).

/// `revolve` — Rust-native feature that sweeps a sketch profile
/// around the local Y axis.
#[derive(FeatureSchema)]
#[schema(type = "revolve")]
pub struct RevolveDef {
    /// Sketch profile this revolve consumes. Accepts the WHOLE sketch
    /// (`qCreatedBy(sketch_id)` / `qAllRegions`) or a single closed region
    /// of it (`qRegion`/`qRegions`) — ticket dhjn. The `SKETCHREGION` entity
    /// filter tells the viewport's profile picker to resolve a clicked filled
    /// region to a `qRegion` query instead of the whole sketch.
    #[param(entity_filter = "SKETCHREGION")]
    pub profiles: ::fielddraft_protocol::Query,

    /// Boolean op used to fold the revolved body into the running
    /// feature spine. Defaults to [`BoolOp::Add`] (Onshape-style).
    #[param(default = ::fielddraft_protocol::BoolOp::Add)]
    pub operation: BoolOp,

    /// Revolve axis, as a query on a single straight line. Accepts a
    /// B-Rep edge (`qEdge(hash)`) or a sketch line (`qSketchLine(sketch,
    /// line)`) — both resolve to a world-space line the profile sweeps
    /// around. REQUIRED (ticket ni56): an empty / unresolvable / off-plane
    /// axis makes the revolve invalid (no body) with a warning. There is no
    /// implicit local-Y fallback.
    #[param(default = ::fielddraft_protocol::Query::new(""), entity_filter = "EDGE,SKETCHLINE")]
    pub axis: ::fielddraft_protocol::Query,

    /// Wall thickness for hollowing the revolved body. `0` (the
    /// default) leaves it solid; any positive value lowers to a
    /// [`CsgNode::Shell`] (`|sdf| − thickness`) via
    /// [`PrimitiveModifiers`], producing a thin-walled closed shell.
    #[param(name = "Shell thickness", default = ::fielddraft_protocol::Length::mm(0.0))]
    pub shell_thickness: ::fielddraft_protocol::Length,

    /// Merge scope: explicit target bodies for the boolean op, as
    /// `qCreatedBy(<seed_feature>)` queries. Empty + `merge_with_all` off =
    /// no merge (standalone body; warned for Add/Remove/Intersect).
    #[param(name = "Merge scope", default = ::fielddraft_protocol::MultiQuery::new(""), entity_filter = "BODY")]
    pub targets: ::fielddraft_protocol::MultiQuery,

    /// Merge with all (Onshape): fold the body into EVERY body it touches
    /// instead of an explicit scope. Off by default — touching solids never
    /// merge implicitly; opt in here or name targets above.
    #[param(name = "Merge with all", default = false)]
    pub merge_with_all: bool,
}

pub struct RevolveFeature;

impl FeatureKind for RevolveFeature {
    fn type_name(&self) -> &'static str {
        RevolveDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        RevolveDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = RevolveDef::parse(def)?;

        // Resolve the `profiles` query against the sketch table.
        // Unlike ExtrudeFeature, we don't fall back to a legacy
        // half-extent stand-in: Revolve needs a real sketch to
        // produce any geometry at all. Ticket dhjn: the query may name
        // the WHOLE sketch (`qCreatedBy`) OR a single closed region of it
        // (`qRegion`/`qRegions`/`qAllRegions`) — `resolve_profile_query`
        // restricts the profile to the picked region(s) in either case.
        //
        // Ticket ni56: an incomplete revolve must produce NO body and read as
        // invalid with a clear warning — never a garbage solid. A missing /
        // unresolvable profile is exactly that, so we warn and bail out
        // (return Ok without pushing a feature) instead of erroring out.
        let Some((sketch, sketch_frame)) = crate::sketch::resolve_profile_query(
            d.profiles.as_str(),
            &ctx.sketches,
        ) else {
            ctx.warn_feature(
                id,
                "Revolve unvollständig: kein gültiges Profil ausgewählt — kein Körper."
                    .to_string(),
            );
            return Ok(());
        };
        let name = optional_display_name(def, || format!("Revolve {id}"));
        let rotation = optional_rotation(def);
        let scale = optional_scale(def);

        // C1: same frame→world composition as ExtrudeFeature.
        let base_transform = crate::compose_transforms(
            &sketch_frame.to_transform(),
            &Transform {
                translate: [0.0, 0.0, 0.0],
                rotation,
                scale,
            },
        );

        // Axis query → revolve around the chosen in-plane line. We
        // resolve the picked line (B-Rep edge OR sketch line) to a world
        // segment, project it into the profile's sketch frame, and apply
        // the rigid 2D map `M` that carries that line onto the canonical
        // local-Y axis (`x = 0`). The profile is pre-multiplied by `M`
        // (so the existing revolve-around-Y machinery is reused verbatim)
        // and the inverse `M⁻¹` is post-composed into the placement, so
        // the body lands exactly where the profile was drawn — only the
        // axis of revolution moves.
        //
        // Ticket ni56: an axis is REQUIRED. There is no legacy implicit
        // local-Y fallback anymore — an empty / unresolvable / off-plane axis
        // makes the revolve invalid (no body) with a clear warning, instead of
        // silently producing a body around a guessed axis (the old behaviour
        // also caused the y2xm "tail" while picking an axis: a transient regen
        // with a not-yet-resolved axis built a degenerate solid).
        let axis_2d = match classify_revolve_axis(ctx, &sketch_frame, d.axis.as_str()) {
            RevolveAxis::Valid(a, b) => (a, b),
            RevolveAxis::Missing => {
                ctx.warn_feature(
                    id,
                    "Revolve unvollständig: keine Achse ausgewählt — kein Körper."
                        .to_string(),
                );
                return Ok(());
            }
            RevolveAxis::OffPlane => {
                ctx.warn_feature(
                    id,
                    "Revolve-Achse liegt nicht in der Sketch-Ebene — kein Körper."
                        .to_string(),
                );
                return Ok(());
            }
        };

        let (a, b) = axis_2d;
        let (sketch, transform) = {
            let (m, m_inv) = revolve_axis_alignment(a, b);
            let aligned = transform_sketch_2d(sketch.clone(), &m);
            // The revolve SDF folds every query to radius √(x²+z²) ≥ 0 and
            // tests the profile there, so a profile that the axis-alignment
            // lands on the NEGATIVE-x side is never reached → an EMPTY solid
            // (tickets a204/ifn2: the SDF/deferred show only the B-Rep edges,
            // never a surface; only the WebGL lathe mesh — independent of the
            // SDF — looked right). Reversing the axis direction flips the
            // profile onto +x: the SAME revolve line, hence the SAME world
            // solid (the matching M⁻¹ places it back), so pick whichever
            // orientation lands the profile on +x.
            if crate::sketch2d::BoundedSketch2d::from_sketch(&aligned)
                .aabb()
                .max[0]
                <= 0.0
            {
                let (m2, m_inv2) = revolve_axis_alignment(b, a);
                (
                    transform_sketch_2d(sketch, &m2),
                    crate::compose_transforms(&base_transform, &m_inv2),
                )
            } else {
                (aligned, crate::compose_transforms(&base_transform, &m_inv))
            }
        };

        // The axis edge of a half-profile is interior after revolving,
        // not surface — strip it so distances near the axis are real
        // (shell would otherwise keep a phantom rod along the axis). Runs
        // AFTER `M`, so the picked axis line (now at `x = 0`) is the one
        // that gets stripped.
        let sketch = crate::sketch::strip_revolve_axis_segments(sketch);

        let target_ids = resolve_merge_scope(
            ctx,
            target_ids_from_queries(&d.targets),
            d.merge_with_all,
        );
        if let Some(w) = ctx.merge_scope_warning(d.operation, &target_ids) {
            ctx.warn_feature(id, w);
        }
        ctx.features.push(DecodedFeature::from_parts(
            id.clone(),
            name,
            d.operation,
            Primitive::Revolve {
                sketch,
                // The revolve axis is now expressed by aligning the
                // profile + placement (see above), so the primitive's own
                // centre is always the local origin.
                center: [0.0, 0.0, 0.0],
            },
            transform,
            PrimitiveModifiers {
                shell_thickness: d.shell_thickness.to_mm().max(0.0),
                ..PrimitiveModifiers::NONE
            },
            target_ids,
        ));
        Ok(())
    }
}

/// Classification of a revolve's `axis` query against its profile plane
/// (ticket ni56). Only [`RevolveAxis::Valid`] yields a body; the other two
/// variants make the revolve invalid (no body) with a warning.
#[derive(Debug, Clone, Copy, PartialEq)]
enum RevolveAxis {
    /// Axis resolved to an in-plane segment, given in the sketch frame's
    /// local 2D coordinates as `(a, b)`.
    Valid([f32; 2], [f32; 2]),
    /// No axis query, or the query couldn't be resolved to a world segment
    /// (e.g. an empty field or a dangling edge/line reference).
    Missing,
    /// The axis resolved but lies off the profile plane (ticket dnfe): the
    /// profile can't rotate around a line it isn't coplanar with.
    OffPlane,
}

/// Resolve a revolve's `axis` query and classify it against the profile plane.
/// An empty or unresolvable query → [`RevolveAxis::Missing`]; an off-plane
/// segment → [`RevolveAxis::OffPlane`]; otherwise the in-plane 2D endpoints.
fn classify_revolve_axis(
    ctx: &mut Context<'_>,
    sketch_frame: &crate::sketch::SketchFrame,
    axis_query: &str,
) -> RevolveAxis {
    let Some((w0, w1)) = crate::queries::parse_query(axis_query)
        .ok()
        .and_then(|expr| ctx.resolve_axis_world_segment(&expr))
    else {
        return RevolveAxis::Missing;
    };
    if axis_in_sketch_plane(sketch_frame, w0, w1) {
        RevolveAxis::Valid(
            sketch_frame.world_to_local_2d(w0),
            sketch_frame.world_to_local_2d(w1),
        )
    } else {
        RevolveAxis::OffPlane
    }
}

/// Whether a world-space axis segment lies IN a sketch's plane — both endpoints
/// within a length-relative tolerance of the plane. A revolve/helix axis must
/// be coplanar with the profile (the profile rotates/winds around it); an
/// off-plane pick is rejected (ticket dnfe).
pub(crate) fn axis_in_sketch_plane(
    frame: &crate::sketch::SketchFrame,
    w0: [f32; 3],
    w1: [f32; 3],
) -> bool {
    let (n, o) = (frame.normal, frame.origin);
    let dist = |p: [f32; 3]| {
        ((p[0] - o[0]) * n[0] + (p[1] - o[1]) * n[1] + (p[2] - o[2]) * n[2]).abs()
    };
    let d = [w1[0] - w0[0], w1[1] - w0[1], w1[2] - w0[2]];
    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    let tol = (len * 1e-3).max(1e-3);
    dist(w0) <= tol && dist(w1) <= tol
}

/// A rigid 2D map `p ↦ (c·x − s·y + tx, s·x + c·y + ty)` with rotation
/// angle `θ = atan2(s, c)` and unit `(c, s) = (cos θ, sin θ)`. Built by
/// [`revolve_axis_alignment`] to carry a chosen axis line onto the
/// canonical local-Y axis before revolving; applied to the profile by
/// [`transform_sketch_2d`].
#[derive(Debug, Clone, Copy)]
pub(crate) struct Rigid2d {
    c: f32,
    s: f32,
    tx: f32,
    ty: f32,
}

impl Rigid2d {
    pub(crate) fn apply(&self, p: [f32; 2]) -> [f32; 2] {
        [
            self.c * p[0] - self.s * p[1] + self.tx,
            self.s * p[0] + self.c * p[1] + self.ty,
        ]
    }
    /// The rotation angle baked into this map (radians); arcs and
    /// sectors add it to their start angle.
    fn angle(&self) -> f32 {
        self.s.atan2(self.c)
    }
}

/// Build the alignment pair for a revolve whose axis is the in-plane
/// segment `a → b` (sketch-frame 2D coordinates).
///
/// Returns `(M, M_inv)` where `M` is the rigid 2D map that sends the
/// axis line onto `x = 0` with its direction pointing `+y`, and `M_inv`
/// is the inverse as a 3D [`Transform`] (a rotation about `+Z` plus an
/// in-plane translation) that undoes `M` after the revolve so the body
/// lands back where the profile was drawn. A degenerate (zero-length)
/// segment yields the identity pair.
pub(crate) fn revolve_axis_alignment(a: [f32; 2], b: [f32; 2]) -> (Rigid2d, Transform) {
    let dx = b[0] - a[0];
    let dy = b[1] - a[1];
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-6 {
        return (
            Rigid2d { c: 1.0, s: 0.0, tx: 0.0, ty: 0.0 },
            Transform::IDENTITY,
        );
    }
    // Unit axis direction. The rotation R = [[dy,-dx],[dx,dy]] sends this
    // direction to (0, 1), so c = cos θ = dy, s = sin θ = dx.
    let (dxu, dyu) = (dx / len, dy / len);
    let (c, s) = (dyu, dxu);
    // Translate so the rotated axis line sits on x = 0; slide along the
    // axis (y) is irrelevant to a revolve, so leave ty = 0.
    let tx = -(c * a[0] - s * a[1]);
    let m = Rigid2d { c, s, tx, ty: 0.0 };

    // M_inv = R⁻¹(q − t): rotation by −θ about +Z, translation −R⁻¹·t.
    let theta = m.angle();
    let half = -0.5 * theta;
    let rotation = [0.0, 0.0, half.sin(), half.cos()];
    // R⁻¹·t with t = (tx, 0): (c·tx, −s·tx); negate for the translation.
    let m_inv = Transform {
        translate: [-c * tx, s * tx, 0.0],
        rotation,
        scale: [1.0, 1.0, 1.0],
    };
    (m, m_inv)
}

/// Apply a rigid 2D map to every point of a [`Sketch2d`] tree. Rotation
/// turns axis-aligned rectangles into general quads, so a [`Rectangle`]
/// is converted to a [`Polygon`] of its four (transformed) corners;
/// circles are rotation-invariant about their centre, and arc/sector
/// start angles pick up the map's rotation.
///
/// [`Rectangle`]: ::fielddraft_protocol::Sketch2dPrimitive::Rectangle
/// [`Polygon`]: ::fielddraft_protocol::Sketch2dPrimitive::Polygon
pub(crate) fn transform_sketch_2d(
    sketch: ::fielddraft_protocol::Sketch2d,
    m: &Rigid2d,
) -> ::fielddraft_protocol::Sketch2d {
    use ::fielddraft_protocol::{Sketch2d, Sketch2dPrimitive, SketchSegment};
    let theta = m.angle();
    let seg = |s: SketchSegment| -> SketchSegment {
        match s {
            SketchSegment::Line { start, end } => SketchSegment::Line {
                start: m.apply(start),
                end: m.apply(end),
            },
            SketchSegment::QuadBezier { p0, p1, p2 } => SketchSegment::QuadBezier {
                p0: m.apply(p0),
                p1: m.apply(p1),
                p2: m.apply(p2),
            },
            SketchSegment::Arc {
                center,
                radius,
                start_angle,
                sweep_angle,
            } => SketchSegment::Arc {
                center: m.apply(center),
                radius,
                start_angle: start_angle + theta,
                sweep_angle,
            },
        }
    };
    match sketch {
        Sketch2d::Empty => Sketch2d::Empty,
        Sketch2d::Prim(prim) => Sketch2d::Prim(match prim {
            Sketch2dPrimitive::Circle { center, radius } => Sketch2dPrimitive::Circle {
                center: m.apply(center),
                radius,
            },
            Sketch2dPrimitive::Rectangle { center, half_extents } => {
                // Rotated rect is no longer axis-aligned → emit the four
                // corners as a polygon.
                let (cx, cy) = (center[0], center[1]);
                let (hx, hy) = (half_extents[0], half_extents[1]);
                Sketch2dPrimitive::Polygon {
                    vertices: vec![
                        m.apply([cx - hx, cy - hy]),
                        m.apply([cx + hx, cy - hy]),
                        m.apply([cx + hx, cy + hy]),
                        m.apply([cx - hx, cy + hy]),
                    ],
                }
            }
            Sketch2dPrimitive::Polygon { vertices } => Sketch2dPrimitive::Polygon {
                vertices: vertices.into_iter().map(|v| m.apply(v)).collect(),
            },
            Sketch2dPrimitive::Sector {
                center,
                radius,
                start_angle,
                sweep_angle,
            } => Sketch2dPrimitive::Sector {
                center: m.apply(center),
                radius,
                start_angle: start_angle + theta,
                sweep_angle,
            },
            Sketch2dPrimitive::Path { segments, closed } => Sketch2dPrimitive::Path {
                segments: segments.into_iter().map(seg).collect(),
                closed,
            },
        }),
        Sketch2d::Union(a, b) => Sketch2d::Union(
            Box::new(transform_sketch_2d(*a, m)),
            Box::new(transform_sketch_2d(*b, m)),
        ),
        Sketch2d::Difference(a, b) => Sketch2d::Difference(
            Box::new(transform_sketch_2d(*a, m)),
            Box::new(transform_sketch_2d(*b, m)),
        ),
        Sketch2d::Intersection(a, b) => Sketch2d::Intersection(
            Box::new(transform_sketch_2d(*a, m)),
            Box::new(transform_sketch_2d(*b, m)),
        ),
        Sketch2d::Offset { inner, distance } => Sketch2d::Offset {
            inner: Box::new(transform_sketch_2d(*inner, m)),
            distance,
        },
    }
}
