//! Lowering vom gelösten Sketch zum [`Sketch2d`]-Profilbaum:
//! `build_sketch2d_from_sub_calls` (+ raw-Variante), Regionen-Queries,
//! Spline-/Slot-Lowering, Mirror-Lowering und Revolve-Achsen-Strip.

use fielddraft_protocol::{FeatureCall, FeatureId};

use crate::sketch2d::{ellipse_quad_beziers, Sketch2d, Sketch2dPrimitive, SketchSegment};

use super::loops::{infer_closed_loops, ChainElement, LOOP_JOIN_TOL};
use fielddraft_protocol::sketch2d::{g1_group_segments, PATH_G1_COS_THRESHOLD};
use super::spline_math::{bspline_quad_beziers, catmull_rom_quad_beziers};
use super::frame::SketchFrame;
use super::tools::{
    build_sketch_path, regular_polygon_vertices, SketchArcDef, SketchCircleDef,
    SketchEllipseDef, SketchLineDef, SketchMirrorDef, SketchOffsetDef, SketchPathDef,
    SketchPolygonDef, SketchRectangleDef, SketchRegularPolygonDef, SketchSlotDef,
    SketchSplineDef, SplineMode,
};
use super::SketchData;
// ─── K7.0: Sketch2d synthesis ───────────────────────────────────
//
// The regen pipeline stashes a [`SketchData`] per sketch feature
// in [`Context::sketches`]. For downstream features (extrude,
// revolve, ...) to consume a sketch as a 2D profile, we need a
// translation from the committed sub-spine into a [`Sketch2d`]
// tree the existing CSG pipeline can lower to a 3D volume.
//
// First cut only handles the *composite* geometric tools that
// carry "this is a closed region" intent: `sketch_rectangle` and
// `sketch_circle`. Free `sketch_point` and `sketch_line` calls
// are ignored — they don't implicitly form a region without an
// inference pass that clusters connected segments into closed
// loops, and that's K7.1 work. Multiple region primitives from
// one sketch are unioned together so a sketch with several
// overlapping shapes lowers to one filled area.

/// Drop profile boundary pieces that lie ON the revolve axis
/// (sketch x = 0). The half-profile of a body of revolution is
/// conventionally closed by an edge along the axis; revolving makes
/// that edge interior, not surface — keeping it would leave a phantom
/// zero-distance "wall" along the whole axis (#390: a phantom
/// zero-set the sphere tracer hits as a thin needle, a 2 mm shell
/// leaves a solid rod standing in the middle, and the weave grows a
/// degenerate radius-0 cylinder face). Dropping the segments keeps the
/// even-odd parity for every sample at r ≥ 0 — a +x ray cast from
/// r ≥ 0 never crosses x = 0 (`line_crossings` needs `p.x < x_cross`
/// and an on-axis wall has `x_cross = 0`) — while the distance
/// magnitude now comes from the real walls at r > 0. This is the
/// exact revolved SDF: the surface of the solid of revolution is the
/// revolution of `∂Ω ∩ {r > 0}` only, and for a query at r ≥ 0 the
/// nearest point of a revolved circle always lies in the same
/// meridian half-plane, so "2D distance to the off-axis boundary"
/// IS the 3D distance. The strip only removes zero-set pieces and
/// never bends the meridian map, so the "meridian map is 1-Lipschitz"
/// proof behind `march_relax = 1.0` for Revolve is untouched.
///
/// Handles every profile form a revolve can see (#390):
/// - `Path { closed: true }`: on-axis segments are filtered out.
/// - `Rectangle` / `Polygon` / `Sector` with an on-axis boundary
///   edge: lowered to the equivalent closed `Path` (via
///   [`revolve_prim_boundary`]) with the on-axis segments dropped.
///   Prims WITHOUT an on-axis edge stay untouched, so off-axis
///   revolves (measured exact, ≤ 2.7e-4 mm) are bit-identical.
/// - Booleans and `Offset` recurse into BOTH children: a subtracted
///   region's axis edge is just as phantom (it would put a zero-set
///   needle inside the bore), and an inward `Offset` of an unstripped
///   profile would grow the phantom wall into a phantom tube.
///
/// All SDF mirrors (CSG-tree eval, compiled tape, dual-number eval,
/// TSL/WGSL interpreter via the tape) consume the stripped `Sketch2d`,
/// so field/tape parity is preserved by construction.
pub fn strip_revolve_axis_segments(sketch: Sketch2d) -> Sketch2d {
    fn on_axis(seg: &SketchSegment) -> bool {
        segment_on_revolve_axis(seg, REVOLVE_AXIS_EPS)
    }

    match sketch {
        Sketch2d::Prim(Sketch2dPrimitive::Path {
            segments,
            closed: true,
        }) => {
            let kept: Vec<SketchSegment> = segments.into_iter().filter(|s| !on_axis(s)).collect();
            Sketch2d::Prim(Sketch2dPrimitive::Path {
                segments: kept,
                closed: true,
            })
        }
        Sketch2d::Prim(prim) => {
            match revolve_prim_boundary(&prim) {
                Some(chain) if chain.iter().any(|(s, _)| on_axis(s)) => {
                    let kept: Vec<SketchSegment> = chain
                        .into_iter()
                        .filter(|(s, _)| !on_axis(s))
                        .map(|(s, _)| s)
                        .collect();
                    Sketch2d::Prim(Sketch2dPrimitive::Path {
                        segments: kept,
                        closed: true,
                    })
                }
                _ => Sketch2d::Prim(prim),
            }
        }
        Sketch2d::Union(a, b) => Sketch2d::Union(
            Box::new(strip_revolve_axis_segments(*a)),
            Box::new(strip_revolve_axis_segments(*b)),
        ),
        Sketch2d::Difference(a, b) => Sketch2d::Difference(
            Box::new(strip_revolve_axis_segments(*a)),
            Box::new(strip_revolve_axis_segments(*b)),
        ),
        Sketch2d::Intersection(a, b) => Sketch2d::Intersection(
            Box::new(strip_revolve_axis_segments(*a)),
            Box::new(strip_revolve_axis_segments(*b)),
        ),
        Sketch2d::Offset { inner, distance } => Sketch2d::Offset {
            inner: Box::new(strip_revolve_axis_segments(*inner)),
            distance,
        },
        other => other,
    }
}

/// Tolerance for "this boundary edge lies ON the revolve axis". Shared
/// by [`strip_revolve_axis_segments`] and the strip-aware trace
/// matching in [`wall_sub_table`].
pub(crate) const REVOLVE_AXIS_EPS: f32 = 1e-4;

/// The boundary of a fixed-shape prim as a closed segment chain, each
/// segment tagged with the prim's OWN boundary-group index (the
/// numbering `sdf_with_edge` uses). `None` for prim kinds that either
/// are handled directly (`Path`) or cannot carry a straight on-axis
/// edge (`Circle`). Shared between [`strip_revolve_axis_segments`] and
/// the conversion-aware trace matching in [`wall_sub_table`] — both
/// must lower identically or the traced wall sources desynchronise
/// from the stripped profile.
pub(crate) fn revolve_prim_boundary(
    prim: &Sketch2dPrimitive,
) -> Option<Vec<(SketchSegment, u32)>> {
    match prim {
        Sketch2dPrimitive::Rectangle {
            center: [cx, cy],
            half_extents: [hx, hy],
        } => {
            // CCW chain; group order of `Rectangle::sdf_with_edge` is
            // 0=+x, 1=−x, 2=+y, 3=−y.
            let (l, r, b, t) = (cx - hx, cx + hx, cy - hy, cy + hy);
            Some(vec![
                (SketchSegment::Line { start: [l, b], end: [r, b] }, 3),
                (SketchSegment::Line { start: [r, b], end: [r, t] }, 0),
                (SketchSegment::Line { start: [r, t], end: [l, t] }, 2),
                (SketchSegment::Line { start: [l, t], end: [l, b] }, 1),
            ])
        }
        Sketch2dPrimitive::Polygon { vertices } => {
            if vertices.len() < 3 {
                return None;
            }
            // Edge id = start-vertex index (`sdf_polygon_with_edge`).
            Some(
                (0..vertices.len())
                    .map(|i| {
                        let j = (i + 1) % vertices.len();
                        (
                            SketchSegment::Line {
                                start: vertices[i],
                                end: vertices[j],
                            },
                            i as u32,
                        )
                    })
                    .collect(),
            )
        }
        Sketch2dPrimitive::Sector {
            center,
            radius,
            start_angle,
            sweep_angle,
        } => {
            let end_angle = start_angle + sweep_angle;
            let p0 = [
                center[0] + radius * start_angle.cos(),
                center[1] + radius * start_angle.sin(),
            ];
            let p1 = [
                center[0] + radius * end_angle.cos(),
                center[1] + radius * end_angle.sin(),
            ];
            // Group indices are positional only — sector traces carry a
            // single source id, so `traced_group_ids` falls back anyway.
            Some(vec![
                (SketchSegment::Line { start: *center, end: p0 }, 0),
                (
                    SketchSegment::Arc {
                        center: *center,
                        radius: *radius,
                        start_angle: *start_angle,
                        sweep_angle: *sweep_angle,
                    },
                    1,
                ),
                (SketchSegment::Line { start: p1, end: *center }, 2),
            ])
        }
        Sketch2dPrimitive::Circle { .. } | Sketch2dPrimitive::Path { .. } => None,
    }
}

/// Is this segment ON the revolve axis (sketch x = 0)? Shared between
/// [`strip_revolve_axis_segments`] and the strip-aware prim matching in
/// [`wall_sub_table`] — both must filter identically or the traced wall
/// sources desynchronise from the stripped profile.
pub(crate) fn segment_on_revolve_axis(seg: &SketchSegment, eps: f32) -> bool {
    let max_abs_x = match seg {
        SketchSegment::Line { start, end } => start[0].abs().max(end[0].abs()),
        SketchSegment::QuadBezier { p0, p1, p2 } => {
            p0[0].abs().max(p1[0].abs()).max(p2[0].abs())
        }
        // A circular arc can't lie on a straight line.
        SketchSegment::Arc { .. } => return false,
    };
    max_abs_x <= eps
}

/// Lower a spline definition to its analytic segment chain —
/// shared by the open-spline region arm and the loop-inference
/// pass below.
pub(crate) fn lower_spline_segments(d: &SketchSplineDef) -> Vec<SketchSegment> {
    let pts: Vec<[f32; 2]> = d
        .points
        .chunks(2)
        .filter(|c| c.len() == 2)
        .map(|c| [c[0], c[1]])
        .collect();
    if pts.len() < 2 {
        return Vec::new();
    }
    match d.mode {
        SplineMode::Interpolate => catmull_rom_quad_beziers(&pts, d.closed, SPLINE_LOWER_TOL),
        SplineMode::Control => {
            let cp: Vec<[f64; 2]> = pts.iter().map(|p| [p[0] as f64, p[1] as f64]).collect();
            bspline_quad_beziers(&cp, d.closed, SPLINE_LOWER_TOL)
        }
    }
}

/// Max deviation (mm) of the quad-Bézier lowering from the exact
/// spline. The lowered segments ARE the kernel geometry (SDF, mesher,
/// weave, GPU stream all consume them), so this is a true geometric
/// tolerance, not a display setting. 0.02 mm is far below feature
/// scale and weave tolerances while keeping segment counts small —
/// the count scales with curvature and only as tol^(−1/3).
pub const SPLINE_LOWER_TOL: f64 = 0.02;


/// Walk a sketch's committed sub_calls and assemble a
/// [`Sketch2d`] tree of every closed-region primitive they
/// contain. Returns [`Sketch2d::Empty`] when the sub-spine has
/// no region-forming entries — downstream features should treat
/// that as "no profile available" and fail fast.
/// Parse raw JSON sub-call values (the format stored in a sketch feature's
/// `sub_calls` param) into [`FeatureCall`]s. Shared by the region arrangement
/// and the profile lowering.
pub fn parse_raw_sub_calls(raw: &[serde_json::Value]) -> Vec<FeatureCall> {
    raw.iter()
        .filter_map(|v| {
            let type_name = v.get("type")?.as_str()?.to_string();
            let id_str = v.get("id")?.as_str().unwrap_or("0");
            let id = FeatureId::from_u32(id_str.parse::<u32>().unwrap_or(0));
            let params = v.get("params").cloned().unwrap_or(serde_json::json!({}));
            Some(FeatureCall::new(type_name, id, params))
        })
        .collect()
}

/// K8.13: variant that takes raw JSON sub-call values
/// (the format stored in FeatureCall params). Parses each
/// into a FeatureCall and delegates.
pub fn build_sketch2d_from_sub_calls_raw(raw: &[serde_json::Value]) -> Sketch2d {
    build_sketch2d_from_sub_calls(&parse_raw_sub_calls(raw))
}

/// Resolve a profile query against the sketch table to a `(Sketch2d, frame)`,
/// or `None` when it doesn't resolve to a non-empty profile (ticket dhjn, #412).
///
/// Honoured query forms — all addressing one sketch feature:
///   * `qCreatedBy(sketch_id)` — the WHOLE sketch (lowered union profile, with
///     its per-entity provenance trace).
///   * `qAllRegions(sketch_id)` — same as above, explicit.
///   * `qRegion(sketch_id, i)` — only the i-th atomic imprint face.
///   * `qRegions(sketch_id, [i, j, …])` — the union of the listed faces.
///
/// Region indices come from [`crate::sketch_arrangement::arrange`] (its
/// deterministic order), which is exactly what `listSketchRegions` exposes to
/// the UI — so the index the user picked addresses the same face here. A
/// region-restricted profile is the EXACT contour of the selected face(s),
/// holes included, but carries no provenance trace yet (positional wall subs).
pub fn resolve_profile_query(
    query: &str,
    sketches: &std::collections::HashMap<FeatureId, SketchData>,
) -> Option<(Sketch2d, SketchFrame, Vec<TracedPrim>)> {
    use crate::queries::QueryExpr;
    let expr = crate::queries::parse_query(query).ok()?;
    let (sketch_id, indices): (FeatureId, Option<Vec<u32>>) = match expr {
        QueryExpr::CreatedBy(id) | QueryExpr::AllRegions(id) => (id, None),
        QueryExpr::Region(id, i) => (id, Some(vec![i])),
        QueryExpr::Regions(id, list) => (id, Some(list)),
        _ => return None,
    };
    let sketch_data = sketches.get(&sketch_id)?;
    match indices {
        None => {
            // Whole sketch — the lowered union profile, with its provenance trace.
            let (full, traced) = build_sketch2d_traced(&sketch_data.sub_calls);
            if matches!(full, Sketch2d::Empty) {
                return None;
            }
            Some((full, sketch_data.frame, traced))
        }
        Some(indices) => {
            // Imprint atomic faces (#412): select the requested faces from the
            // arrangement and union their EXACT profiles. Region-restricted
            // profiles carry no stable provenance trace (the atomic-face path
            // is a fresh contour), so wall subs fall back to positional.
            let faces = crate::sketch_arrangement::arrange(&sketch_data.sub_calls);
            let mut acc = Sketch2d::Empty;
            for &i in &indices {
                if let Some(f) = faces.get(i as usize) {
                    acc = match acc {
                        Sketch2d::Empty => f.profile.clone(),
                        prev => Sketch2d::Union(Box::new(prev), Box::new(f.profile.clone())),
                    };
                }
            }
            if matches!(acc, Sketch2d::Empty) {
                return None;
            }
            Some((acc, sketch_data.frame, Vec::new()))
        }
    }
}

pub fn build_sketch2d_from_sub_calls(sub_calls: &[FeatureCall]) -> Sketch2d {
    build_sketch2d_traced(sub_calls).0
}

/// One entry per primitive the lowering emits, in tree DFS (= emission)
/// order — the provenance record the stable wall-sub table is built from.
#[derive(Debug, Clone)]
pub struct TracedPrim {
    /// Exact clone of the primitive that entered the tree.
    pub prim: Sketch2dPrimitive,
    /// Where each boundary group's geometry came from; `None` when the
    /// lowering couldn't attribute stable ids (sketch_path, mirror copy)
    /// — a table containing such a prim falls back to positional subs.
    pub sources: Option<TraceSources>,
}

/// Stable-source attribution of one primitive's boundary.
#[derive(Debug, Clone)]
pub enum TraceSources {
    /// One stable id per LOCAL boundary group, in group order — the
    /// fixed-group prims (Rectangle 4, Circle/Sector 1, Polygon n).
    PerGroup(Vec<u32>),
    /// One source per PATH SEGMENT; the per-group representative (min id
    /// over the group's segments) is derived at table-build time, after
    /// any segment filtering (revolve axis strip) has settled.
    PerSegment(Vec<Option<u32>>),
}

/// Entity-allocation record of one sub-call, from the registry replay.
struct CallAlloc {
    /// `next_entity_id` before the call — the id of its FIRST entity.
    first: u32,
    /// Curve entities (line/circle/arc/ellipse/bspline) the call
    /// allocated, in allocation order. Empty for pure-point tools
    /// (slot, polygon) and failed/unknown calls.
    curves: Vec<u32>,
}

/// Replay the sub-spine through the REAL tool registry into a scratch
/// [`SketchState`] and record what each call allocates. The `apply()`
/// implementations are the single allocation authority (the same code
/// the regeneration runs), so this can never drift from the kernel —
/// unlike a third hand-maintained allocation table.
fn replay_allocations(sub_calls: &[FeatureCall]) -> Vec<CallAlloc> {
    use fielddraft_solver::Entity;
    let registry = super::tools::SketchToolRegistry::with_builtins();
    let mut state = fielddraft_solver::SketchState::with_origin();
    let mut out = Vec::with_capacity(sub_calls.len());
    for call in sub_calls {
        let first = state.next_entity_id;
        let before = state.entities.len();
        let applied = registry
            .get(&call.type_name)
            .map(|t| t.apply(&mut state, &call.id, &call.params));
        let mut curves = Vec::new();
        if let Some(Ok(())) = applied {
            for e in &state.entities[before..] {
                match e {
                    Entity::Line { id, .. }
                    | Entity::Circle { id, .. }
                    | Entity::Arc { id, .. }
                    | Entity::Ellipse { id, .. }
                    | Entity::BSpline { id, .. } => curves.push(id.0),
                    _ => {}
                }
            }
        }
        out.push(CallAlloc { first, curves });
    }
    out
}

/// Push every primitive of an untraceable subtree (sketch_path lowering,
/// mirror copy) so the in-order matching in [`wall_sub_table`] stays
/// aligned — with `sources: None`, which poisons the table (positional
/// fallback) if any of these prims survives into the profile.
fn push_untraced(tree: &Sketch2d, trace: &mut Vec<TracedPrim>) {
    match tree {
        Sketch2d::Empty => {}
        Sketch2d::Prim(p) => trace.push(TracedPrim { prim: p.clone(), sources: None }),
        Sketch2d::Union(a, b)
        | Sketch2d::Difference(a, b)
        | Sketch2d::Intersection(a, b) => {
            push_untraced(a, trace);
            push_untraced(b, trace);
        }
        Sketch2d::Offset { inner, .. } => push_untraced(inner, trace),
    }
}

/// [`build_sketch2d_from_sub_calls`] plus the per-primitive provenance
/// trace (in emission order) that [`wall_sub_table`] consumes.
pub fn build_sketch2d_traced(sub_calls: &[FeatureCall]) -> (Sketch2d, Vec<TracedPrim>) {
    let allocs = replay_allocations(sub_calls);
    let curve0 = |idx: usize| allocs.get(idx).and_then(|a| a.curves.first().copied());
    let mut trace: Vec<TracedPrim> = Vec::new();
    // K7.1 loop inference: free `sketch_line`s and open `sketch_spline`s
    // form no region on their own, but a chain of them whose endpoints
    // coincide (the snap tools guarantee exact coincidence) encloses an
    // area exactly like a polygon/closed-spline does. Gather the
    // chainable elements, stitch closed loops, and splice each loop into
    // the accumulator at the position of its first member so the
    // order-sensitive modifiers (mirror, offset) keep their semantics.
    let mut chainable: Vec<ChainElement> = Vec::new();
    for (idx, call) in sub_calls.iter().enumerate() {
        if call
            .params
            .get("construction")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            continue;
        }
        match call.type_name.as_str() {
            "sketch_line" => {
                if let Ok(d) = SketchLineDef::parse(&call.params) {
                    let len = ((d.end[0] - d.start[0]).powi(2)
                        + (d.end[1] - d.start[1]).powi(2))
                    .sqrt();
                    if len > LOOP_JOIN_TOL {
                        chainable.push(ChainElement {
                            call_index: idx,
                            segments: vec![SketchSegment::Line {
                                start: d.start,
                                end: d.end,
                            }],
                            source: curve0(idx),
                        });
                    }
                }
            }
            "sketch_spline" => {
                if let Ok(d) = SketchSplineDef::parse(&call.params) {
                    if !d.closed {
                        let segments = lower_spline_segments(&d);
                        if !segments.is_empty() {
                            chainable.push(ChainElement {
                                call_index: idx,
                                segments,
                                source: curve0(idx),
                            });
                        }
                    }
                }
            }
            // Ticket 6b41: a `sketch_arc` is an open curve whose two endpoints
            // can close a contour together with lines/splines/other arcs (e.g. a
            // diameter line + a semicircular arc → a half-disc). Offer it to the
            // chainer as a single `Arc` segment so that loop is detected as a
            // closed region. A full-circle arc (|sweep| ≈ 2π) is already closed
            // on its own, so it stays out of the chainer and is lowered to a
            // `Sector` below; likewise a lone open arc that joins no loop falls
            // through to the `Sector` (pie-wedge) lowering via `consumed`.
            "sketch_arc" => {
                if let Ok(d) = SketchArcDef::parse(&call.params) {
                    let nearly_full =
                        (d.sweep_angle.abs() - std::f32::consts::TAU).abs() < 1e-3;
                    if d.radius > 0.0 && d.sweep_angle.abs() > 1e-6 && !nearly_full {
                        chainable.push(ChainElement {
                            call_index: idx,
                            segments: vec![SketchSegment::Arc {
                                center: d.center,
                                radius: d.radius,
                                start_angle: d.start_angle,
                                sweep_angle: d.sweep_angle,
                            }],
                            source: curve0(idx),
                        });
                    }
                }
            }
            _ => {}
        }
    }
    let (mut inferred_loops, consumed) = infer_closed_loops(&chainable);

    let mut acc: Option<Sketch2d> = None;
    for (idx, call) in sub_calls.iter().enumerate() {
        if let Some(loops) = inferred_loops.remove(&idx) {
            for (segments, sources) in loops {
                let prim = Sketch2dPrimitive::Path { segments, closed: true };
                trace.push(TracedPrim {
                    prim: prim.clone(),
                    sources: Some(TraceSources::PerSegment(sources)),
                });
                let node = Sketch2d::Prim(prim);
                acc = Some(match acc {
                    None => node,
                    Some(prev) => Sketch2d::Union(Box::new(prev), Box::new(node)),
                });
            }
        }
        if consumed.contains(&idx) {
            continue;
        }
        // Construction geometry is reference-only: it participates in the
        // constraint solve (the TS side still builds solver entities for
        // it) but never contributes to the extrudable region. The flag
        // is a plain param so it costs nothing in the tool schemas.
        if call
            .params
            .get("construction")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            continue;
        }
        let prim: Option<(Sketch2dPrimitive, Option<TraceSources>)> = match call
            .type_name
            .as_str()
        {
            "sketch_rectangle" => {
                let d = match SketchRectangleDef::parse(&call.params) {
                    Ok(d) => d,
                    Err(_) => continue,
                };
                let (min_x, max_x) = sort_pair(d.corner_a[0], d.corner_b[0]);
                let (min_y, max_y) = sort_pair(d.corner_a[1], d.corner_b[1]);
                let half_x = 0.5 * (max_x - min_x);
                let half_y = 0.5 * (max_y - min_y);
                if half_x <= 0.0 || half_y <= 0.0 {
                    continue;
                }
                let center = [0.5 * (min_x + max_x), 0.5 * (min_y + max_y)];
                // Group order 0..3 = +x, −x, +y, −y (`sdf_with_edges`);
                // the tool allocates its four line entities in the order
                // bottom, right, top, left → [right, left, top, bottom].
                let sources = allocs
                    .get(idx)
                    .filter(|a| a.curves.len() == 4)
                    .map(|a| {
                        TraceSources::PerGroup(vec![
                            a.curves[1],
                            a.curves[3],
                            a.curves[2],
                            a.curves[0],
                        ])
                    });
                Some((
                    Sketch2dPrimitive::Rectangle {
                        center,
                        half_extents: [half_x, half_y],
                    },
                    sources,
                ))
            }
            "sketch_circle" => {
                let d = match SketchCircleDef::parse(&call.params) {
                    Ok(d) => d,
                    Err(_) => continue,
                };
                if d.radius <= 0.0 {
                    continue;
                }
                let sources =
                    curve0(idx).map(|c| TraceSources::PerGroup(vec![c]));
                Some((
                    Sketch2dPrimitive::Circle {
                        center: d.center,
                        radius: d.radius,
                    },
                    sources,
                ))
            }
            "sketch_arc" => {
                let d = match SketchArcDef::parse(&call.params) {
                    Ok(d) => d,
                    Err(_) => continue,
                };
                if d.radius <= 0.0 || d.sweep_angle.abs() < 1e-6 {
                    continue;
                }
                // Sector keeps the single-group simplification.
                let sources =
                    curve0(idx).map(|c| TraceSources::PerGroup(vec![c]));
                Some((
                    Sketch2dPrimitive::Sector {
                        center: d.center,
                        radius: d.radius,
                        start_angle: d.start_angle,
                        sweep_angle: d.sweep_angle,
                    },
                    sources,
                ))
            }
            "sketch_ellipse" => {
                let d = match SketchEllipseDef::parse(&call.params) {
                    Ok(d) => d,
                    Err(_) => continue,
                };
                if d.radius_x <= 0.0 || d.radius_y <= 0.0 {
                    continue;
                }
                // Option B: decompose into a closed Path of quadratic
                // Béziers (G1-continuous ⇒ one B-Rep edge). 10 µm
                // default tolerance; `segments` overrides the count.
                let segs = ellipse_quad_beziers(
                    d.center,
                    d.radius_x,
                    d.radius_y,
                    d.rotation,
                    0.0,
                    std::f32::consts::TAU,
                    d.segments.max(0.0) as u32,
                    0.01,
                );
                if segs.is_empty() {
                    continue;
                }
                // The whole ellipse is ONE solver entity — every segment
                // (and thus every derived group) points at it.
                let sources = curve0(idx)
                    .map(|c| TraceSources::PerSegment(vec![Some(c); segs.len()]));
                Some((
                    Sketch2dPrimitive::Path {
                        segments: segs,
                        closed: true,
                    },
                    sources,
                ))
            }
            "sketch_slot" => {
                let d = match SketchSlotDef::parse(&call.params) {
                    Ok(d) => d,
                    Err(_) => continue,
                };
                match slot_path_segments(d.center_a, d.center_b, d.radius) {
                    Some(segments) => {
                        // The slot tool allocates only its two centre
                        // POINTS — anchor the (single, fully-G1) boundary
                        // on the first of them: unique per call, stable
                        // in the shared entity-id space.
                        let sources = allocs.get(idx).map(|a| {
                            TraceSources::PerSegment(vec![
                                Some(a.first);
                                segments.len()
                            ])
                        });
                        Some((
                            Sketch2dPrimitive::Path { segments, closed: true },
                            sources,
                        ))
                    }
                    None => continue,
                }
            }
            "sketch_polygon" => {
                let d = match SketchPolygonDef::parse(&call.params) {
                    Ok(d) => d,
                    Err(_) => continue,
                };
                let verts: Vec<[f32; 2]> = d
                    .vertices
                    .chunks(2)
                    .filter(|c| c.len() == 2)
                    .map(|c| [c[0], c[1]])
                    .collect();
                if verts.len() < 3 {
                    continue;
                }
                // Polygon group k = the edge leaving vertex k; the tool
                // allocates one POINT per vertex in order → anchor edge k
                // on vertex k's point id (`first + k`).
                let sources = allocs.get(idx).map(|a| {
                    TraceSources::PerGroup(
                        (0..verts.len() as u32).map(|k| a.first + k).collect(),
                    )
                });
                Some((Sketch2dPrimitive::Polygon { vertices: verts }, sources))
            }
            "sketch_spline" => {
                // Closed splines (and open ones no loop consumed) lower
                // to a Path; the quad-Bézier fit lives in
                // [`lower_spline_segments`], shared with loop inference.
                let d = match SketchSplineDef::parse(&call.params) {
                    Ok(d) => d,
                    Err(_) => continue,
                };
                let segs = lower_spline_segments(&d);
                if segs.is_empty() {
                    continue;
                }
                let sources = curve0(idx)
                    .map(|c| TraceSources::PerSegment(vec![Some(c); segs.len()]));
                Some((
                    Sketch2dPrimitive::Path {
                        segments: segs,
                        closed: d.closed,
                    },
                    sources,
                ))
            }
            "sketch_path" => {
                // An explicit line+quad path of one or more contours,
                // combined with even-odd hole semantics into a subtree.
                // Union the whole subtree into the accumulator (it may
                // itself contain Difference nodes for holes).
                let d = match SketchPathDef::parse(&call.params) {
                    Ok(d) => d,
                    Err(_) => continue,
                };
                let Some(node) = build_sketch_path(&d) else {
                    continue;
                };
                // SVG paths allocate only point entities — no per-segment
                // curve provenance. Trace the prims untraced so the
                // in-order matching stays aligned (table → positional).
                push_untraced(&node, &mut trace);
                acc = Some(match acc {
                    None => node,
                    Some(prev) => Sketch2d::Union(Box::new(prev), Box::new(node)),
                });
                continue;
            }
            "sketch_regular_polygon" => {
                let d = match SketchRegularPolygonDef::parse(&call.params) {
                    Ok(d) => d,
                    Err(_) => continue,
                };
                match regular_polygon_vertices(d.center, d.radius, d.sides, d.inscribed) {
                    Some(verts) => {
                        // Same vertex-point anchoring as `sketch_polygon` —
                        // the tool allocates one point per synthesised
                        // vertex in the same order.
                        let sources = allocs.get(idx).map(|a| {
                            TraceSources::PerGroup(
                                (0..verts.len() as u32).map(|k| a.first + k).collect(),
                            )
                        });
                        Some((Sketch2dPrimitive::Polygon { vertices: verts }, sources))
                    }
                    None => continue,
                }
            }
            "sketch_mirror" => {
                // Mirror the accumulated tree so far across an axis.
                let d = match SketchMirrorDef::parse(&call.params) {
                    Ok(d) => d,
                    Err(_) => continue,
                };
                if let Some(ref tree) = acc {
                    let mirrored = mirror_sketch2d(tree, d.axis as u32);
                    // The mirror copy shares its source entities with the
                    // original — aliased subs would fold two distinct
                    // walls onto one branch. Trace untraced instead
                    // (whole-sketch positional fallback).
                    push_untraced(&mirrored, &mut trace);
                    acc = Some(Sketch2d::Union(
                        Box::new(acc.take().unwrap()),
                        Box::new(mirrored),
                    ));
                }
                continue;
            }
            "sketch_offset" => {
                // Offset the accumulated profile by inflating its SDF.
                // Positive = grow (outward), negative = shrink (inward).
                // The `Sketch2d::Offset` node carries the distance through
                // to evaluation (`sketch2d.rs`) and SDF compilation
                // (`sdf_compiler.rs`), so the whole profile so far is
                // wrapped in one offset node.
                let d = match SketchOffsetDef::parse(&call.params) {
                    Ok(d) => d,
                    Err(_) => continue,
                };
                if acc.is_some() {
                    acc = Some(Sketch2d::Offset {
                        inner: Box::new(acc.take().unwrap()),
                        distance: d.distance,
                    });
                }
                continue;
            }
            _ => None,
        };
        let Some((prim, sources)) = prim else { continue };
        trace.push(TracedPrim { prim: prim.clone(), sources });
        let node = Sketch2d::Prim(prim);
        acc = Some(match acc {
            None => node,
            Some(prev) => Sketch2d::Union(Box::new(prev), Box::new(node)),
        });
    }
    (acc.unwrap_or(Sketch2d::Empty), trace)
}

/// Mirror a Sketch2d tree across an axis.
/// axis=0 → mirror across X (flip Y), axis=1 → across Y (flip X).
fn mirror_sketch2d(tree: &Sketch2d, axis: u32) -> Sketch2d {
    match tree {
        Sketch2d::Empty => Sketch2d::Empty,
        Sketch2d::Prim(prim) => Sketch2d::Prim(mirror_prim(prim, axis)),
        Sketch2d::Union(a, b) => Sketch2d::Union(
            Box::new(mirror_sketch2d(a, axis)),
            Box::new(mirror_sketch2d(b, axis)),
        ),
        Sketch2d::Difference(a, b) => Sketch2d::Difference(
            Box::new(mirror_sketch2d(a, axis)),
            Box::new(mirror_sketch2d(b, axis)),
        ),
        Sketch2d::Intersection(a, b) => Sketch2d::Intersection(
            Box::new(mirror_sketch2d(a, axis)),
            Box::new(mirror_sketch2d(b, axis)),
        ),
        Sketch2d::Offset { inner, distance } => Sketch2d::Offset {
            inner: Box::new(mirror_sketch2d(inner, axis)),
            distance: *distance,
        },
    }
}

fn mirror_prim(prim: &Sketch2dPrimitive, axis: u32) -> Sketch2dPrimitive {
    let flip = |p: [f32; 2]| -> [f32; 2] {
        if axis == 0 {
            [p[0], -p[1]]
        } else {
            [-p[0], p[1]]
        }
    };
    match prim {
        Sketch2dPrimitive::Circle { center, radius } => Sketch2dPrimitive::Circle {
            center: flip(*center),
            radius: *radius,
        },
        Sketch2dPrimitive::Rectangle {
            center,
            half_extents,
        } => Sketch2dPrimitive::Rectangle {
            center: flip(*center),
            half_extents: *half_extents,
        },
        Sketch2dPrimitive::Polygon { vertices } => Sketch2dPrimitive::Polygon {
            vertices: vertices.iter().map(|v| flip(*v)).collect(),
        },
        Sketch2dPrimitive::Sector {
            center,
            radius,
            start_angle,
            sweep_angle,
        } => {
            // Mirror the angular range.
            let new_start = if axis == 0 {
                -start_angle - sweep_angle
            } else {
                std::f32::consts::PI - start_angle - sweep_angle
            };
            Sketch2dPrimitive::Sector {
                center: flip(*center),
                radius: *radius,
                start_angle: new_start,
                sweep_angle: *sweep_angle,
            }
        }
        Sketch2dPrimitive::Path { segments, closed } => Sketch2dPrimitive::Path {
            segments: segments.iter().map(|s| mirror_segment(s, axis)).collect(),
            closed: *closed,
        },
    }
}

/// Reflect a single sketch segment about the mirror axis. `axis
/// == 0` flips Y (reflect across the X-axis), `axis == 1` flips X.
/// Reflection reverses orientation, so an `Arc`'s sweep sign flips
/// and its start angle maps accordingly (θ → −θ for axis 0, θ →
/// π−θ for axis 1).
fn mirror_segment(seg: &SketchSegment, axis: u32) -> SketchSegment {
    let flip = |p: [f32; 2]| -> [f32; 2] {
        if axis == 0 {
            [p[0], -p[1]]
        } else {
            [-p[0], p[1]]
        }
    };
    match seg {
        SketchSegment::Line { start, end } => SketchSegment::Line {
            start: flip(*start),
            end: flip(*end),
        },
        SketchSegment::QuadBezier { p0, p1, p2 } => SketchSegment::QuadBezier {
            p0: flip(*p0),
            p1: flip(*p1),
            p2: flip(*p2),
        },
        SketchSegment::Arc {
            center,
            radius,
            start_angle,
            sweep_angle,
        } => {
            let new_start = if axis == 0 {
                -start_angle
            } else {
                std::f32::consts::PI - start_angle
            };
            SketchSegment::Arc {
                center: flip(*center),
                radius: *radius,
                start_angle: new_start,
                sweep_angle: -sweep_angle,
            }
        }
    }
}

fn sort_pair(a: f32, b: f32) -> (f32, f32) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

/// Build the closed stadium boundary for a centre-to-centre slot:
/// two parallel straight edges between the end caps, joined by two
/// outward-bulging semicircles. Returns `None` for a degenerate slot
/// (coincident centres or non-positive radius).
///
/// Winding (consistent CCW for r·n on the +n side first):
/// Line(A+r·n → B+r·n); Arc capping B; Line(B−r·n → A−r·n); Arc
/// capping A. Each cap is a semicircle (`sweep_angle = −π`) starting
/// at its first boundary point's angle so the cap bulges away from
/// the opposite centre — verified by the build test's bounding box.
pub(crate) fn slot_path_segments(
    center_a: [f32; 2],
    center_b: [f32; 2],
    radius: f32,
) -> Option<Vec<SketchSegment>> {
    let ax = center_a[0] as f64;
    let ay = center_a[1] as f64;
    let bx = center_b[0] as f64;
    let by = center_b[1] as f64;
    let r = radius as f64;
    let dx = bx - ax;
    let dy = by - ay;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-6 || r < 1e-6 {
        return None;
    }
    // Unit axis A→B and its left normal n = (−u.y, u.x).
    let ux = dx / len;
    let uy = dy / len;
    let nx = -uy;
    let ny = ux;
    // Four boundary points.
    let a_plus = [(ax + r * nx) as f32, (ay + r * ny) as f32]; // A+r·n
    let b_plus = [(bx + r * nx) as f32, (by + r * ny) as f32]; // B+r·n
    let b_minus = [(bx - r * nx) as f32, (by - r * ny) as f32]; // B−r·n
    let a_minus = [(ax - r * nx) as f32, (ay - r * ny) as f32]; // A−r·n
                                                                // Cap B starts at B+r·n (angle of +n); sweep −π takes it through
                                                                // +u (outside, away from A) to B−r·n.
    let cap_b_start = (ny as f32).atan2(nx as f32);
    // Cap A starts at A−r·n (angle of −n); sweep −π takes it through
    // −u (outside, away from B) to A+r·n.
    let cap_a_start = (-ny as f32).atan2(-nx as f32);
    let pi = std::f32::consts::PI;
    Some(vec![
        SketchSegment::Line {
            start: a_plus,
            end: b_plus,
        },
        SketchSegment::Arc {
            center: [bx as f32, by as f32],
            radius,
            start_angle: cap_b_start,
            sweep_angle: -pi,
        },
        SketchSegment::Line {
            start: b_minus,
            end: a_minus,
        },
        SketchSegment::Arc {
            center: [ax as f32, ay as f32],
            radius,
            start_angle: cap_a_start,
            sweep_angle: -pi,
        },
    ])
}

// ─── stable wall-sub table (Onshape-Import Schritt 2) ────────────────────

/// Build the positional-group → stable-sub table for a profile tree.
///
/// Walks `tree` in the same in-order DFS the global boundary-group
/// numbering uses (`sdf_with_edges`' `edge_count` offsets) and matches
/// every `Prim` leaf, in order, against the lowering trace — the tree may
/// be a pruned SUBSEQUENCE of the trace (region restriction) and its Path
/// prims may have been axis-stripped (revolve profiles); both are handled.
/// The concatenated per-prim id lists form the table.
///
/// Returns `None` — positional fallback, never a half-translated table —
/// when any surviving prim lacks stable sources (sketch_path, mirror,
/// replay failure), a prim can't be matched, or an id would collide with
/// the reserved sub range (caps at `0x7FE`/`0x7FF`, inner-wall flag).
pub fn wall_sub_table(tree: &Sketch2d, traced: &[TracedPrim]) -> Option<Vec<u32>> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    collect_wall_subs(tree, traced, &mut cursor, &mut out)?;
    if out
        .iter()
        .any(|&s| s >= crate::csg::BranchId::EXTRUDE_CAP_BOTTOM_SUB)
    {
        return None;
    }
    // Two walls that shared a sub would collapse into ONE branch (and one
    // face). A table that can't name every wall distinctly is no table.
    let mut seen: Vec<u32> = out.clone();
    seen.sort_unstable();
    seen.dedup();
    if seen.len() != out.len() {
        return None;
    }
    Some(out)
}

fn collect_wall_subs(
    tree: &Sketch2d,
    traced: &[TracedPrim],
    cursor: &mut usize,
    out: &mut Vec<u32>,
) -> Option<()> {
    match tree {
        Sketch2d::Empty => Some(()),
        Sketch2d::Prim(p) => {
            while *cursor < traced.len() {
                let t = &traced[*cursor];
                *cursor += 1;
                if let Some(ids) = prim_wall_ids(p, t) {
                    out.extend(ids?);
                    return Some(());
                }
            }
            None
        }
        Sketch2d::Union(a, b)
        | Sketch2d::Difference(a, b)
        | Sketch2d::Intersection(a, b) => {
            collect_wall_subs(a, traced, cursor, out)?;
            collect_wall_subs(b, traced, cursor, out)
        }
        Sketch2d::Offset { inner, .. } => collect_wall_subs(inner, traced, cursor, out),
    }
}

/// Does trace entry `t` describe final prim `p`? Outer `None` = no match
/// (try the next trace entry); `Some(inner)` = matched, where inner
/// `None` = matched but unattributable (poison → positional fallback).
fn prim_wall_ids(p: &Sketch2dPrimitive, t: &TracedPrim) -> Option<Option<Vec<u32>>> {
    // Exact match — the common case.
    if &t.prim == p {
        return Some(traced_group_ids(p, t.sources.as_ref()));
    }
    // Strip-aware match: a revolve profile's closed Path may have lost
    // its on-axis segments (`strip_revolve_axis_segments`). Filter the
    // traced Path with the SAME predicate and compare again.
    if let (
        Sketch2dPrimitive::Path { segments: fs, closed: fc },
        Sketch2dPrimitive::Path { segments: ts, closed: tc },
    ) = (p, &t.prim)
    {
        if fc == tc && fs.len() < ts.len() {
            let kept: Vec<&SketchSegment> = ts
                .iter()
                .filter(|s| !segment_on_revolve_axis(s, REVOLVE_AXIS_EPS))
                .collect();
            if kept.len() == fs.len() && kept.iter().zip(fs.iter()).all(|(a, b)| *a == b) {
                let sources = match &t.sources {
                    Some(TraceSources::PerSegment(src)) if src.len() == ts.len() => {
                        let filtered: Vec<Option<u32>> = ts
                            .iter()
                            .zip(src.iter())
                            .filter(|(s, _)| !segment_on_revolve_axis(s, REVOLVE_AXIS_EPS))
                            .map(|(_, id)| *id)
                            .collect();
                        Some(TraceSources::PerSegment(filtered))
                    }
                    other => other.clone(),
                };
                return Some(traced_group_ids(p, sources.as_ref()));
            }
        }
    }
    // Conversion-aware match (#390): an axis-touching Rectangle /
    // Polygon / Sector was lowered to a closed Path with its on-axis
    // edges dropped (`strip_revolve_axis_segments`). Re-run the SAME
    // conversion on the traced prim and compare the kept chain; the
    // traced per-GROUP sources are remapped to per-SEGMENT sources via
    // the chain's original-group tags.
    if let Sketch2dPrimitive::Path { segments: fs, closed: true } = p {
        if let Some(chain) = revolve_prim_boundary(&t.prim) {
            if chain
                .iter()
                .any(|(s, _)| segment_on_revolve_axis(s, REVOLVE_AXIS_EPS))
            {
                let kept: Vec<&(SketchSegment, u32)> = chain
                    .iter()
                    .filter(|(s, _)| !segment_on_revolve_axis(s, REVOLVE_AXIS_EPS))
                    .collect();
                if kept.len() == fs.len()
                    && kept.iter().zip(fs.iter()).all(|((a, _), b)| a == b)
                {
                    let sources = match &t.sources {
                        Some(TraceSources::PerGroup(ids))
                            if ids.len() == t.prim.edge_count() as usize =>
                        {
                            Some(TraceSources::PerSegment(
                                kept.iter().map(|(_, g)| Some(ids[*g as usize])).collect(),
                            ))
                        }
                        // Anything else (e.g. a Sector's single source
                        // id) is unattributable → poison to the
                        // positional fallback, never a mis-named wall.
                        _ => None,
                    };
                    return Some(traced_group_ids(p, sources.as_ref()));
                }
            }
        }
    }
    None
}

/// Reduce a matched prim's trace sources to one stable id per LOCAL
/// boundary group (group order = the prim's `sdf_with_edges` numbering).
/// `None` = unattributable.
fn traced_group_ids(
    p: &Sketch2dPrimitive,
    sources: Option<&TraceSources>,
) -> Option<Vec<u32>> {
    let sources = sources?;
    let group_count = p.edge_count() as usize;
    match sources {
        TraceSources::PerGroup(ids) => {
            (ids.len() == group_count).then(|| ids.clone())
        }
        TraceSources::PerSegment(src) => {
            let Sketch2dPrimitive::Path { segments, .. } = p else {
                return None;
            };
            if src.len() != segments.len() {
                return None;
            }
            let groups = g1_group_segments(segments, PATH_G1_COS_THRESHOLD);
            let mut ids: Vec<Option<u32>> = vec![None; group_count];
            for (g, s) in groups.iter().zip(src.iter()) {
                let slot = ids.get_mut(*g as usize)?;
                // Canonical representative: the MIN curve entity id over
                // the group's segments — deterministic when a G1 group
                // spans several sketch edges (line + tangent arc chain).
                *slot = match (*slot, *s) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (None, other) => other,
                    (keep, None) => keep,
                };
            }
            // A group with NO attributed segment poisons the prim: a
            // half-attributed table must not silently mis-name walls.
            let any_hole = ids
                .iter()
                .zip(0..group_count)
                .any(|(id, _)| id.is_none());
            if any_hole {
                return None;
            }
            Some(ids.into_iter().map(|i| i.unwrap()).collect())
        }
    }
}
