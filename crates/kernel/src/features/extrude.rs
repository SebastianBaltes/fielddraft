//! `extrude` — Sketch-Region- und Face-Extrusion.

use serde_json::Value;

use fielddraft_protocol::{BoolOp, FeatureId, Primitive, Transform};

use crate::csg::{DecodedFeature, PrimitiveModifiers};

use super::merge_scope::{resolve_merge_scope, target_ids_from_queries};
use super::{Context, FeatureError, FeatureKind, FeatureSchema, optional_display_name, optional_rotation, optional_scale, positive_f32};
// ─── ExtrudeDef (K4.5.8: Rust-native reference implementation) ──
//
// ExtrudeDef is the first K4.5 reference feature that exercises
// the full parameter vocabulary the K4.5.4f derive learned —
// `Query`, `Length`, `BoolOp`, `f32` and `[f32; 3]` all appear in
// one definition struct, so the §4.6 promise that "the derive
// covers the kernel runtime types" is demonstrated end-to-end
// rather than in unit-test fixtures only.
//
// ## Execute contract
//
// The protocol's `Primitive` enum doesn't carry an `Extrude`
// variant yet — adding one would ripple through the mesher, the
// DC pass, the BVH and the wire form, which is well out of
// scope for one slice. Instead, K4.5.8's executor **lowers** an
// ExtrudeDef into a plain `Primitive::Box`: the rectangular
// profile (half-width / half-height in X and Y) supplies the
// first two half-extents, and the extrude depth converted to
// millimetres via `Length::to_mm` supplies the Z half-extent
// (depth / 2, so the solid is centred on the extrude plane).
// The result is a real mesh-able feature that K3's dual-
// contouring pipeline handles exactly like any hand-added box
// — the difference is that the spine entry was produced by a
// FeatureCall routed through the registry, not by a direct
// `Kernel::add_primitive` call.
//
// ## What's intentionally simplified vs §4.6
//
// * **`profiles: Query` is accepted but not resolved.** K4.5.7
//   ships the query parser and a version cache, but there's no
//   sketch-producing feature upstream whose profile a query
//   could point at yet. The executor stores the query string
//   for round-trip fidelity and moves on. A later slice will
//   resolve the query into a real `Sketch2d` once sketch
//   features exist, at which point the `profile_half_x` /
//   `profile_half_y` fields can be removed in favour of the
//   sketch's own extent.
// * **Only the Z extrude direction is supported.** The §4.6
//   example includes `second_direction: bool` and
//   `depth2: Option<Length>`; `Option<T>` isn't a derive-
//   supported field type yet, and the two-sided extrude has no
//   semantic value while the mesh-level representation stays a
//   simple centred box. Both fields are deferred.
// * **Operation is `BoolOp`, not the full §4.6 "new / add /
//   remove / intersect" operation vocabulary.** The extra
//   "new" case (create a separate body) needs body-id
//   plumbing that's not yet wired through the registry path.

/// `extrude` — Rust-native feature that consumes a sketch
/// profile and lowers it to a 3D body.
///
/// K7.1 turns the `profiles` query into a real resolution
/// path: the executor parses the query, finds the referenced
/// sketch in [`Context::sketches`], synthesises a
/// [`crate::sketch2d::Sketch2d`] via
/// `build_sketch2d_from_sub_calls` and reads the profile
/// extents from the resulting AABB. The `profile_half_x` /
/// `profile_half_y` fields become optional fallbacks for
/// tests and for documents that point at a sketch the kernel
/// can't resolve yet.
///
/// Known shape-fidelity limitation: the current spine pipeline
/// has no `Primitive::Extrude` variant, so the lowering still
/// produces a `Primitive::Box` whose half-extents match the
/// sketch's bounding rectangle. A rectangle sketch is
/// pixel-perfect; a circle becomes its bounding box. Fixing
/// that is a later K7 slice which adds the Primitive variant
/// and teaches the mesher/BVH about it.
#[derive(FeatureSchema)]
#[schema(type = "extrude")]
pub struct ExtrudeDef {
    /// Sketch profile this extrude consumes. Evaluated against
    /// [`Context::sketches`] at regeneration time via
    /// [`crate::sketch::resolve_profile_query`]. Accepts the WHOLE
    /// sketch (`qCreatedBy(sketch_id)` / `qAllRegions`) or a single closed
    /// region of it (`qRegion`/`qRegions`) — ticket tgdg. The `SKETCHREGION`
    /// entity filter tells the viewport's profile picker to resolve a clicked
    /// filled region to a `qRegion` query instead of the whole sketch (it is
    /// the same picker path Revolve uses, ticket dhjn). Ticket 20fc: a planar
    /// B-Rep `qFace(...)` is accepted here too — picking a solid's side face as
    /// the "profile" routes through the face-extrude path, so the user need not
    /// know a separate field. Ticket k280: this is a `MultiQuery`, so several
    /// coplanar faces (or regions) accumulate in one pick session and extrude
    /// together — the single Onshape-style "faces and sketch regions to extrude"
    /// input. Every `qFace(...)` entry routes through the face-extrude path
    /// ([`Context::resolve_extrude_faces`], which unions their boundary loops);
    /// a sketch entry resolves via [`crate::sketch::resolve_profile_query`].
    #[param(entity_filter = "SKETCHREGION, FACE")]
    pub profiles: ::fielddraft_protocol::MultiQuery,

    /// Extrude depth along the local Z axis. Any supported
    /// length unit works; the executor converts to millimetres
    /// via [`Length::to_mm`] before lowering to a box.
    pub depth: ::fielddraft_protocol::Length,

    /// Fallback half-width for tests and for extrudes that
    /// don't yet point at a resolvable sketch. Ignored when
    /// the query successfully resolves to a non-empty sketch.
    #[param(default = 0.0)]
    pub profile_half_x: f32,

    /// Fallback half-height — same story as
    /// [`profile_half_x`].
    #[param(default = 0.0)]
    pub profile_half_y: f32,

    /// Boolean op used to fold the extruded body into the
    /// running feature spine. Defaults to [`BoolOp::New`]
    /// (ticket yx5h): a fresh extrude creates a separate body
    /// rather than merging into whatever it touches.
    #[param(default = ::fielddraft_protocol::BoolOp::New)]
    pub operation: BoolOp,

    /// Override for the extruded box origin in world space.
    /// When the sketch resolves, the X/Y components default to
    /// the sketch AABB centre; `center[2]` always supplies the
    /// extrude plane Z coordinate.
    #[param(default = [0.0, 0.0, 0.0])]
    pub center: [f32; 3],

    /// Wall thickness for hollowing the extruded body. `0` (the
    /// default) leaves it solid; any positive value lowers to a
    /// [`CsgNode::Shell`] (`|sdf| − thickness`) via
    /// [`PrimitiveModifiers`], producing a thin-walled closed shell.
    #[param(name = "Shell thickness", default = ::fielddraft_protocol::Length::mm(0.0))]
    pub shell_thickness: ::fielddraft_protocol::Length,

    /// Direction mode. `false` (default) is a one-sided (blind) extrude
    /// `[−depth_back, +depth]` along the plane normal — the Onshape default of
    /// growing outward from the profile plane (ticket yx5h). `true` grows the
    /// profile symmetrically by `±depth/2` about the plane.
    #[param(name = "Symmetric", default = false)]
    pub symmetric: bool,

    /// Backward depth for a two-sided (asymmetric) extrude, measured against the
    /// plane normal. Only consulted when `symmetric` is `false`; `0` (default)
    /// makes a pure blind extrude.
    #[param(name = "Back depth", default = ::fielddraft_protocol::Length::mm(0.0))]
    pub depth_back: ::fielddraft_protocol::Length,

    /// Merge scope: explicit target bodies for the boolean op, as
    /// `qCreatedBy(<seed_feature>)` queries. Empty falls back to face-owner
    /// docking (face extrudes); otherwise + `merge_with_all` off = no merge
    /// (standalone body; warned for Add/Remove/Intersect).
    #[param(name = "Merge scope", default = ::fielddraft_protocol::MultiQuery::new(""), entity_filter = "BODY")]
    pub targets: ::fielddraft_protocol::MultiQuery,

    /// Merge with all (Onshape): fold the body into EVERY body it touches
    /// instead of an explicit scope. Off by default — touching solids never
    /// merge implicitly; opt in here or name targets above.
    #[param(name = "Merge with all", default = false)]
    pub merge_with_all: bool,
}

pub struct ExtrudeFeature;

impl FeatureKind for ExtrudeFeature {
    fn type_name(&self) -> &'static str {
        ExtrudeDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        ExtrudeDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = ExtrudeDef::parse(def)?;

        // Length → mm at the model-unit boundary. The kernel's
        // native distance scale is millimetres, so every feature
        // that consumes a typed runtime length is responsible for
        // converting once, at the edge.
        let depth_mm = d.depth.to_mm();

        // Direction mode → (half-height, centre offset along the plane normal).
        // Symmetric (default) is ±|depth|/2 about the plane — the legacy sketch
        // behaviour (dz = 0). Blind / two-sided spans the sorted interval over
        // `{−back, +depth}` → a NEGATIVE depth extrudes in the −normal
        // direction (Onshape: the depth drag may cross the profile plane and
        // grow the body the other way, ticket x7af). Centre sits mid-span
        // along local Z (which the frame maps to the plane normal). The
        // `center[2]` shift is folded into the primitive below.
        let back_mm = d.depth_back.to_mm().max(0.0);
        let (half_depth_z, dz) = if d.symmetric {
            (depth_mm.abs() / 2.0, 0.0)
        } else {
            let lo = (-back_mm).min(depth_mm);
            let hi = (-back_mm).max(depth_mm);
            ((hi - lo) / 2.0, (hi + lo) / 2.0)
        };
        if half_depth_z <= 0.0 {
            return Err(FeatureError::InvalidParameter {
                feature: ExtrudeDef::TYPE_NAME,
                name: "depth",
                message: format!("extrude extent must be > 0 mm, got depth {depth_mm} / depth_back {back_mm}"),
            });
        }

        let name = optional_display_name(def, || format!("Extrude {id}"));
        let rotation = optional_rotation(def);
        let scale = optional_scale(def);
        let mods = PrimitiveModifiers {
            shell_thickness: d.shell_thickness.to_mm().max(0.0),
            ..PrimitiveModifiers::NONE
        };

        // ── Face-extrude path (P1) ──────────────────────────────────────────
        // `profiles` is the unified Onshape-style "faces and sketch regions"
        // input (ticket k280): every `qFace(...)` entry drives the face-extrude
        // path. Multiple coplanar faces accumulate in one pick session — their
        // boundary loops are projected into the shared plane, unioned, and swept
        // exactly like a sketch profile; the face frame places that profile back
        // onto the plane. Any face entry takes precedence over a sketch entry.
        let profile_qs: Vec<String> =
            d.profiles.queries().iter().map(|s| s.to_string()).collect();
        let face_queries: Vec<String> = profile_qs
            .iter()
            .filter(|q| {
                matches!(
                    crate::queries::parse_query(q),
                    Ok(crate::queries::QueryExpr::Face(_))
                )
            })
            .cloned()
            .collect();
        if !face_queries.is_empty() {
            let (sketch, frame) =
                ctx.resolve_extrude_faces(&face_queries).map_err(|message| {
                    FeatureError::InvalidParameter {
                        feature: ExtrudeDef::TYPE_NAME,
                        name: "profiles",
                        message,
                    }
                })?;
            let primitive = Primitive::Extrude {
                sketch,
                half_depth_z,
                center: [0.0, 0.0, dz],
                wall_subs: None,
            };
            let transform = crate::compose_transforms(
                &frame.to_transform(),
                &Transform { translate: [0.0, 0.0, 0.0], rotation, scale },
            );
            // Dock into the body that owns the picked face: an Add grows that
            // body (Onshape semantics), and the explicit target keeps the prism
            // from fusing with an unrelated AABB-overlapping body (ticket px2o).
            // An explicit merge scope (ticket 15f5) overrides the face owner.
            let mut target_ids = target_ids_from_queries(&d.targets);
            if target_ids.is_empty() {
                target_ids = ctx.face_owner_target_ids(&face_queries);
            }
            if target_ids.is_empty() && d.merge_with_all {
                target_ids = ctx.current_body_seed_ids();
            }
            if let Some(w) = ctx.merge_scope_warning(d.operation, &target_ids) {
                ctx.warn_feature(id, w);
            }
            ctx.features.push(DecodedFeature::from_parts(
                id.clone(),
                name,
                d.operation,
                primitive,
                transform,
                mods,
                target_ids,
            ));
            return Ok(());
        }

        // K7.3: resolve the `profiles` query against the
        // context's sketch table and lower to a real
        // `Primitive::Extrude` carrying the full Sketch2d
        // profile. The K4.5.8 fallback (Box stand-in) stays
        // for documents that don't yet point at a resolvable
        // sketch, so existing tests keep running unchanged.
        //
        // K7.22 follow-up: when the `profiles` query *does*
        // resolve to a sketch but the sketch has no closed
        // regions (e.g. the user drew lines/points but no
        // rectangle/circle), we used to fall through to the
        // legacy fallback and die on the `profile_half_x > 0`
        // guard with an unhelpful message. Distinguish the
        // three states explicitly: resolved-and-extrudable,
        // resolved-but-empty, no-resolve-use-fallback.
        let mut resolved_sketch: Option<::fielddraft_protocol::Sketch2d> = None;
        let mut sketch_trace: Vec<crate::sketch::TracedPrim> = Vec::new();
        let mut sketch_frame = crate::sketch::SketchFrame::IDENTITY;
        let mut sketch_was_found_but_empty = false;
        // Ticket tgdg: resolve `qCreatedBy`/`qAllRegions`/`qRegion`/`qRegions`
        // through the same path Revolve uses (ticket dhjn) so a single picked
        // region extrudes just that region (holes preserved), not the whole
        // sketch. `resolve_profile_query` returns `None` both for an
        // unresolvable query (→ legacy half-extent fallback) and for a sketch
        // that resolved but has no closed regions; distinguish those so the
        // "draw a closed region first" error still fires.
        // No face entries → treat the first profile entry as the sketch source
        // (a single sketch query — whole sketch or a region group — is the
        // common case; multi-region is already one `qRegions(...)` expression).
        let profile_str: &str = profile_qs.first().map(String::as_str).unwrap_or("");
        match crate::sketch::resolve_profile_query(
            profile_str,
            &ctx.sketches,
        ) {
            Some((sketch2d, frame, traced)) => {
                resolved_sketch = Some(sketch2d);
                sketch_trace = traced;
                sketch_frame = frame;
            }
            None => {
                // A `qCreatedBy(id)` that names a known sketch with no
                // extrudable region is the "found but empty" case.
                if let Ok(crate::queries::QueryExpr::CreatedBy(sketch_id))
                | Ok(crate::queries::QueryExpr::AllRegions(sketch_id)) =
                    crate::queries::parse_query(profile_str)
                {
                    if ctx.sketches.contains_key(&sketch_id) {
                        sketch_was_found_but_empty = true;
                    }
                }
            }
        }

        // K7.22 follow-up: emit a clear, user-facing error when
        // the caller *intended* to drive the extrude from a sketch
        // (profile_half_x/y both left at 0) but the sketch has no
        // extrudable regions. The legacy-API test path that passes
        // explicit non-zero half-extents still falls through to
        // the Box fallback below so existing callers keep working.
        if sketch_was_found_but_empty
            && d.profile_half_x == 0.0
            && d.profile_half_y == 0.0
        {
            return Err(FeatureError::InvalidParameter {
                feature: ExtrudeDef::TYPE_NAME,
                name: "profiles",
                message: "sketch has no closed regions to extrude — draw a rectangle, circle or polygon first".to_string(),
            });
        }

        // Ticket r2zx: the profile query named nothing extrudable (e.g. a solid
        // FACE or a whole-body `qCreatedBy(extrude_id)` was picked instead of a
        // sketch region) and no legacy half-extents were given. Fail with a
        // clear, actionable message instead of falling through to the box
        // fallback's cryptic "profile_half_x must be > 0".
        if resolved_sketch.is_none()
            && d.profile_half_x == 0.0
            && d.profile_half_y == 0.0
        {
            return Err(FeatureError::InvalidParameter {
                feature: ExtrudeDef::TYPE_NAME,
                name: "profiles",
                message: "select a closed sketch region or one or more coplanar planar faces to extrude".to_string(),
            });
        }

        // Direction mode folds into `center[2]` (along local Z / plane normal).
        let center = [d.center[0], d.center[1], d.center[2] + dz];
        let primitive = match resolved_sketch {
            Some(sketch) => {
                // Stable sketch-entity wall subs (Onshape-Import Schritt 2);
                // `None` = positional fallback (unattributable profile).
                let wall_subs = crate::sketch::wall_sub_table(&sketch, &sketch_trace);
                Primitive::Extrude {
                    sketch,
                    half_depth_z,
                    center,
                    wall_subs,
                }
            }
            None => {
                // Fallback: legacy hand-written half-extents.
                positive_f32(
                    d.profile_half_x,
                    ExtrudeDef::TYPE_NAME,
                    "profile_half_x",
                )?;
                positive_f32(
                    d.profile_half_y,
                    ExtrudeDef::TYPE_NAME,
                    "profile_half_y",
                )?;
                Primitive::Box {
                    center,
                    half_extents: [d.profile_half_x, d.profile_half_y, half_depth_z],
                }
            }
        };

        // C1: compose the sketch's frame→world placement (parent) with
        // the feature's own rotation/scale (child) so the profile — built
        // in local XY — sits on the real sketch plane. `origin_plane_xy`
        // and the legacy no-sketch path both yield the identity frame, so
        // existing extrudes are byte-for-byte unchanged.
        let transform = crate::compose_transforms(
            &sketch_frame.to_transform(),
            &Transform {
                translate: [0.0, 0.0, 0.0],
                rotation,
                scale,
            },
        );

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
            primitive,
            transform,
            mods,
            target_ids,
        ));
        Ok(())
    }
}
