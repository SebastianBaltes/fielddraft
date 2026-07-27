//! Muster-Features: `linear_pattern`, `circular_pattern`, `mirror`.

use serde_json::Value;

use fielddraft_protocol::{FeatureId, MultiQuery};

use super::{Context, FeatureError, FeatureKind, FeatureSchema};
// ─── K8.0: Linear pattern ────────────────────────────────────────
//
// The first bona-fide parametric pattern feature. Given a
// source feature id, a direction vector, a count and a spacing
// distance, [`LinearPatternFeature`] emits `count - 1` extra
// copies of the source spine entry at offsets
// `i * spacing * unit_dir` for `i = 1..count`. The originals
// stay in place — Onshape semantics.
//
// Copies inherit the source's BoolOp, primitive payload and
// transform verbatim, with the offset added to
// `transform.translate`. Their feature ids are hierarchical
// children of the pattern feature (`pattern_id/1`,
// `pattern_id/2`, …) so provenance still resolves to "this
// clone came from the pattern".
//
// Known limits for this first cut:
//
// * Source must already be on the spine when the pattern
//   runs, i.e. the source call must appear *before* the
//   pattern call in `feature_calls`. Not enforced — a
//   missing source just errors out with `MissingParameter`.
// * Pattern-of-pattern isn't supported: the source lookup
//   walks `document.features` by id, and pattern children
//   are siblings of the pattern feature itself, not under it.

/// Resolve a `source` MultiQuery (a list of `qCreatedBy(id)` exprs)
/// into the set of source feature ids it references. Mirrors the
/// resolution path in [`BooleanFeature::execute`], but goes through
/// [`crate::queries::parse_query`] so malformed query strings surface
/// as a clean [`FeatureError::InvalidParameter`] rather than silently
/// dropping out. Order is preserved and duplicates are kept out.
fn resolve_source_feature_ids(
    mq: &MultiQuery,
    feature: &'static str,
) -> Result<Vec<FeatureId>, FeatureError> {
    use crate::queries::QueryExpr;
    let mut ids: Vec<FeatureId> = Vec::new();
    for q in mq.queries() {
        let expr = crate::queries::parse_query(q).map_err(|e| {
            FeatureError::InvalidParameter {
                feature,
                name: "source",
                message: format!("could not parse source query `{q}`: {e}"),
            }
        })?;
        match expr {
            QueryExpr::CreatedBy(id) | QueryExpr::FaceOf(id) => {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
            other => {
                return Err(FeatureError::InvalidParameter {
                    feature,
                    name: "source",
                    message: format!(
                        "source must reference a body via qCreatedBy(id); got `{other}`"
                    ),
                });
            }
        }
    }
    Ok(ids)
}

/// `linear_pattern` — replicate a source feature along a
/// direction vector.
#[derive(FeatureSchema)]
#[schema(type = "linear_pattern")]
pub struct LinearPatternDef {
    /// Bodies to replicate, as a list of `qCreatedBy(id)` queries.
    /// Each referenced source feature is replicated independently.
    #[param(name = "Source", entity_filter = "BODY")]
    pub source: ::fielddraft_protocol::MultiQuery,
    /// Direction vector in local frame. Doesn't have to be
    /// normalised — the executor divides by its length at
    /// apply time.
    pub direction: [f32; 3],
    /// Total number of instances *including* the source. Stored
    /// as an `f32` (not `u32`) so the generic FeatureDialog's
    /// number widget can edit it — the derive maps plain integer
    /// types to the `generic → unknown` widget and they'd never
    /// render. `execute` rounds to the nearest positive integer.
    /// `count = 1` is a no-op, `count = 5` means the pattern
    /// adds four offset copies.
    pub count: f32,
    /// Distance between consecutive instances in millimetres.
    pub spacing: ::fielddraft_protocol::Length,
}

pub struct LinearPatternFeature;

impl FeatureKind for LinearPatternFeature {
    fn type_name(&self) -> &'static str {
        LinearPatternDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        LinearPatternDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = LinearPatternDef::parse(def)?;
        let count = d.count.round().max(0.0) as u32;
        if count < 2 {
            // A one-instance pattern is a no-op. Don't error —
            // this is a useful parameter-sweep resting point.
            return Ok(());
        }
        let spacing_mm = d.spacing.to_mm();
        if spacing_mm <= 0.0 {
            return Err(FeatureError::InvalidParameter {
                feature: LinearPatternDef::TYPE_NAME,
                name: "spacing",
                message: format!("must be > 0 mm, got {spacing_mm}"),
            });
        }

        // Normalise the direction. A zero-length vector is
        // invalid — there's no axis to replicate along.
        let dx = d.direction[0];
        let dy = d.direction[1];
        let dz = d.direction[2];
        let len = (dx * dx + dy * dy + dz * dz).sqrt();
        if len < 1e-6 {
            return Err(FeatureError::InvalidParameter {
                feature: LinearPatternDef::TYPE_NAME,
                name: "direction",
                message: "must be a non-zero vector".to_string(),
            });
        }
        let unit = [dx / len, dy / len, dz / len];

        // Resolve the source bodies from the query list.
        let source_ids =
            resolve_source_feature_ids(&d.source, LinearPatternDef::TYPE_NAME)?;
        if source_ids.is_empty() {
            return Err(FeatureError::InvalidParameter {
                feature: LinearPatternDef::TYPE_NAME,
                name: "source",
                message: "must reference at least one source body".to_string(),
            });
        }

        // Replicate every referenced source body independently.
        for source_fid in &source_ids {
            let source_feature = ctx
                .features
                .iter()
                .find(|f| &f.id == source_fid)
                .ok_or(FeatureError::MissingParameter {
                    feature: LinearPatternDef::TYPE_NAME,
                    name: "source",
                })?
                .clone();

            // Emit `count - 1` offset copies. `i = 0` would be the
            // source itself, which is already on the spine.
            // Clones carry hierarchical child ids under the
            // pattern, namespaced by the source feature id so
            // multiple sources don't collide — `sync_feature_calls`
            // drops those via its parent filter so the wire form
            // stays authoritative.
            for i in 1..count {
                let i_f = i as f32;
                let offset = [
                    unit[0] * spacing_mm * i_f,
                    unit[1] * spacing_mm * i_f,
                    unit[2] * spacing_mm * i_f,
                ];
                let mut clone = source_feature.clone();
                clone.id = id.child(format!("{source_fid}_{i}"));
                clone.name = format!("{} {}", source_feature.name, i);
                clone.transform.translate = [
                    source_feature.transform.translate[0] + offset[0],
                    source_feature.transform.translate[1] + offset[1],
                    source_feature.transform.translate[2] + offset[2],
                ];
                ctx.features.push(clone);
            }
        }

        Ok(())
    }
}

// ─── K8.1: Circular pattern ──────────────────────────────────────
//
// Twin of the linear pattern: instead of offsetting clones by
// a constant translation along a direction vector, each clone is
// rotated around an `axis` through a `center` pivot by an
// angle that scales with its index. Default sweep is a full
// 360° divided evenly over `count` instances; arcs come in by
// overriding `angle_deg`.
//
// The math per clone at index `i ∈ 1..count`:
//   θ        = (i / count) · angle_deg · π / 180
//   q_delta  = axis_angle_to_quat(unit_axis, θ)
//   offset   = source.translate − center
//   clone.translate = q_delta · offset + center
//   clone.rotation  = q_delta · source.rotation
//
// This composes the pattern's rotation on top of whatever
// transform the source feature already carries, so you can
// chain circular-pattern a rotated box and every clone stays
// rotated relative to its own local axis.

/// `circular_pattern` — replicate a source feature around an
/// axis through a pivot point.
#[derive(FeatureSchema)]
#[schema(type = "circular_pattern")]
pub struct CircularPatternDef {
    /// Bodies to replicate, as a list of `qCreatedBy(id)` queries.
    /// Each referenced source feature is replicated independently.
    #[param(name = "Source", entity_filter = "BODY")]
    pub source: ::fielddraft_protocol::MultiQuery,
    /// Rotation axis in local frame. The executor normalises
    /// it; a zero vector errors out.
    pub axis: [f32; 3],
    /// Pivot point the axis passes through.
    #[param(default = [0.0, 0.0, 0.0])]
    pub center: [f32; 3],
    /// Total number of instances including the source. `f32`
    /// for dialog-widget compatibility — `execute` rounds to
    /// the nearest positive integer. See [`LinearPatternDef::count`]
    /// for the full rationale.
    pub count: f32,
    /// Total sweep in degrees. Defaults to a full 360° when
    /// omitted from the call.
    #[param(default = 360.0, unit = "\u{00b0}")]
    pub angle_deg: f32,
}

pub struct CircularPatternFeature;

impl FeatureKind for CircularPatternFeature {
    fn type_name(&self) -> &'static str {
        CircularPatternDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        CircularPatternDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = CircularPatternDef::parse(def)?;
        let count = d.count.round().max(0.0) as u32;
        if count < 2 {
            return Ok(());
        }
        // Normalise the axis.
        let ax = d.axis[0];
        let ay = d.axis[1];
        let az = d.axis[2];
        let axis_len = (ax * ax + ay * ay + az * az).sqrt();
        if axis_len < 1e-6 {
            return Err(FeatureError::InvalidParameter {
                feature: CircularPatternDef::TYPE_NAME,
                name: "axis",
                message: "must be a non-zero vector".to_string(),
            });
        }
        let unit = [ax / axis_len, ay / axis_len, az / axis_len];

        let source_ids =
            resolve_source_feature_ids(&d.source, CircularPatternDef::TYPE_NAME)?;
        if source_ids.is_empty() {
            return Err(FeatureError::InvalidParameter {
                feature: CircularPatternDef::TYPE_NAME,
                name: "source",
                message: "must reference at least one source body".to_string(),
            });
        }

        // Per-instance angle step.
        let step_rad =
            (d.angle_deg / count as f32) * std::f32::consts::PI / 180.0;

        for source_fid in &source_ids {
            let source_feature = ctx
                .features
                .iter()
                .find(|f| &f.id == source_fid)
                .ok_or(FeatureError::MissingParameter {
                    feature: CircularPatternDef::TYPE_NAME,
                    name: "source",
                })?
                .clone();

            for i in 1..count {
                let theta = step_rad * i as f32;
                let half = theta * 0.5;
                let s = half.sin();
                let q_delta = [unit[0] * s, unit[1] * s, unit[2] * s, half.cos()];

                // Rotate the source translate around the pivot.
                let offset = [
                    source_feature.transform.translate[0] - d.center[0],
                    source_feature.transform.translate[1] - d.center[1],
                    source_feature.transform.translate[2] - d.center[2],
                ];
                let rotated = crate::quat_rotate(q_delta, offset);
                let new_translate = [
                    rotated[0] + d.center[0],
                    rotated[1] + d.center[1],
                    rotated[2] + d.center[2],
                ];

                let mut clone = source_feature.clone();
                clone.id = id.child(format!("{source_fid}_{i}"));
                clone.name = format!("{} {}", source_feature.name, i);
                clone.transform.translate = new_translate;
                clone.transform.rotation =
                    crate::quat_multiply(q_delta, source_feature.transform.rotation);
                ctx.features.push(clone);
            }
        }

        Ok(())
    }
}

// ─── K8.2: Mirror ────────────────────────────────────────────────
//
// A single clone whose PLACEMENT is the source's placement composed
// with the reflection through the mirror plane.
//
// The first cut reflected only `transform.translate` and kept the
// rotation verbatim. That is not a mirror: it relocates the source's
// ORIGIN and leaves the body itself unturned, so an L-bracket came out
// pointing the same way as its original, and a sketch-based body — whose
// translate is the world origin — landed exactly ON TOP of its source.
// The Onshape importer had to refuse every `mirror` feature rather than
// emit that silent duplicate (`web/src/onshape/onshapeImport.ts`).
//
// The reflection through the plane `(p0, n)` is the affine map
//
//     x ↦ A·x + 2(n·p0)·n,    A = I − 2nnᵀ
//
// and `A` factors EXACTLY into the TRS form this kernel stores:
//
//     A = (−I) · R₁₈₀(n)
//
// because `R₁₈₀(n) = 2nnᵀ − I`. So the reflection is a rotation of 180°
// about the plane normal together with a UNIFORM scale of −1 — the
// quaternion is simply `(nₓ, n_y, n_z, 0)`.
//
// Uniform matters twice over. `compose_transforms` folds scale and
// rotation independently, which is exact only when the scale commutes
// with the rotation — it does here, so `reflection ∘ source` is exact
// rather than approximated. And |−1| = 1 makes the map an ISOMETRY:
// the mirrored body's SDF stays a true distance field, no Lipschitz
// slack, no march damping. Downstream, `min_scale()` takes magnitudes
// (see `fielddraft_protocol::Transform::min_scale`), the AABB mapping
// unions all eight transformed corners, and the mesher derives its
// winding from the field's gradient — so a negative scale needs no
// special case in any of them.

/// `mirror` — reflect a source feature's translate through a
/// plane.
#[derive(FeatureSchema)]
#[schema(type = "mirror")]
pub struct MirrorDef {
    /// Bodies to mirror, as a list of `qCreatedBy(id)` queries.
    /// Each referenced source feature is mirrored independently.
    #[param(name = "Source", entity_filter = "BODY")]
    pub source: ::fielddraft_protocol::MultiQuery,
    /// Mirror plane, given as a face/datum query (e.g. `qFace(<hash>)`
    /// or `origin_plane_xy`). Its world-space frame supplies both the
    /// plane normal and a point on the plane.
    #[param(name = "Plane", entity_filter = "FACE")]
    pub plane: ::fielddraft_protocol::Query,
}

pub struct MirrorFeature;

impl FeatureKind for MirrorFeature {
    fn type_name(&self) -> &'static str {
        MirrorDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        MirrorDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = MirrorDef::parse(def)?;

        // Resolve the mirror plane from the face/datum query. Its
        // world-space frame gives both the plane normal and a point
        // on the plane.
        let plane_expr = crate::queries::parse_query(d.plane.as_str()).map_err(|e| {
            FeatureError::InvalidParameter {
                feature: MirrorDef::TYPE_NAME,
                name: "plane",
                message: format!("could not parse plane query: {e}"),
            }
        })?;
        let frame = ctx.resolve_plane_frame(&plane_expr).ok_or(
            FeatureError::InvalidParameter {
                feature: MirrorDef::TYPE_NAME,
                name: "plane",
                message: "plane query resolved to no datum and no face".to_string(),
            },
        )?;
        let n = frame.normal;
        let plane_point = frame.origin;

        let source_ids =
            resolve_source_feature_ids(&d.source, MirrorDef::TYPE_NAME)?;
        if source_ids.is_empty() {
            return Err(FeatureError::InvalidParameter {
                feature: MirrorDef::TYPE_NAME,
                name: "source",
                message: "must reference at least one source body".to_string(),
            });
        }

        // The plane normal must be a unit vector — `A = I − 2nnᵀ` is a
        // reflection only then, and `resolve_plane_frame` is not the only
        // producer of these frames. A degenerate normal would silently
        // scale the clone instead of mirroring it.
        let n_len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if !(n_len > 1e-6) {
            return Err(FeatureError::InvalidParameter {
                feature: MirrorDef::TYPE_NAME,
                name: "plane",
                message: "mirror plane has a degenerate normal".to_string(),
            });
        }
        let n = [n[0] / n_len, n[1] / n_len, n[2] / n_len];

        // The reflection as a TRS placement (see the block comment):
        // rotate 180° about `n`, scale by −1 uniformly, then translate by
        // `2(n·p0)·n` so the plane itself stays fixed.
        let n_dot_p0 = n[0] * plane_point[0] + n[1] * plane_point[1] + n[2] * plane_point[2];
        let reflection = ::fielddraft_protocol::Transform {
            translate: [
                2.0 * n_dot_p0 * n[0],
                2.0 * n_dot_p0 * n[1],
                2.0 * n_dot_p0 * n[2],
            ],
            rotation: [n[0], n[1], n[2], 0.0],
            scale: [-1.0, -1.0, -1.0],
        };

        for source_fid in &source_ids {
            let source_feature = ctx
                .features
                .iter()
                .find(|f| &f.id == source_fid)
                .ok_or(FeatureError::MissingParameter {
                    feature: MirrorDef::TYPE_NAME,
                    name: "source",
                })?
                .clone();

            let mut clone = source_feature.clone();
            clone.id = id.child(format!("{source_fid}_1"));
            clone.name = format!("{} mirror", source_feature.name);
            // `reflection ∘ source`: the source is placed first, then the
            // whole thing is reflected — the source's own rotation and
            // scale are carried through instead of being dropped.
            clone.transform =
                crate::transform::compose_transforms(&reflection, &source_feature.transform);
            ctx.features.push(clone);
        }

        Ok(())
    }
}
