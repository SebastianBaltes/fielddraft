//! Lowering vom gelösten Sketch zum [`Sketch2d`]-Profilbaum:
//! `build_sketch2d_from_sub_calls` (+ raw-Variante), Regionen-Queries,
//! Spline-/Slot-Lowering, Mirror-Lowering und Revolve-Achsen-Strip.

use fielddraft_protocol::{FeatureCall, FeatureId};

use crate::sketch2d::{ellipse_quad_beziers, Sketch2d, Sketch2dPrimitive, SketchSegment};

use super::loops::{infer_closed_loops, ChainElement, LOOP_JOIN_TOL};
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

/// Drop closed-Path boundary segments that lie ON the revolve axis
/// (sketch x = 0). The half-profile of a body of revolution is
/// conventionally closed by an edge along the axis; revolving makes
/// that edge interior, not surface — keeping it would leave a phantom
/// zero-distance "wall" along the whole axis (a 2 mm shell then leaves
/// a solid rod standing in the middle, and the weave grows a
/// degenerate radius-0 cylinder face). Dropping the segments keeps the
/// even-odd parity for every sample at r ≥ 0 — a +x ray cast from
/// r ≥ 0 never crosses x = 0 — while the distance magnitude now comes
/// from the real wall.
pub fn strip_revolve_axis_segments(sketch: Sketch2d) -> Sketch2d {
    const AXIS_EPS: f32 = 1e-4;
    fn on_axis(seg: &SketchSegment) -> bool {
        let max_abs_x = match seg {
            SketchSegment::Line { start, end } => start[0].abs().max(end[0].abs()),
            SketchSegment::QuadBezier { p0, p1, p2 } => {
                p0[0].abs().max(p1[0].abs()).max(p2[0].abs())
            }
            // A circular arc can't lie on a straight line.
            SketchSegment::Arc { .. } => return false,
        };
        max_abs_x <= AXIS_EPS
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
        Sketch2d::Union(a, b) => Sketch2d::Union(
            Box::new(strip_revolve_axis_segments(*a)),
            Box::new(strip_revolve_axis_segments(*b)),
        ),
        other => other,
    }
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
/// K8.13: variant that takes raw JSON sub-call values
/// (the format stored in FeatureCall params). Parses each
/// into a FeatureCall and delegates.
pub fn build_sketch2d_from_sub_calls_raw(raw: &[serde_json::Value]) -> Sketch2d {
    let calls: Vec<FeatureCall> = raw
        .iter()
        .filter_map(|v| {
            let type_name = v.get("type")?.as_str()?.to_string();
            let id_str = v.get("id")?.as_str().unwrap_or("0");
            let id = FeatureId::from_u32(id_str.parse::<u32>().unwrap_or(0));
            let params = v.get("params").cloned().unwrap_or(serde_json::json!({}));
            Some(FeatureCall::new(type_name, id, params))
        })
        .collect();
    build_sketch2d_from_sub_calls(&calls)
}

/// Grid density for region detection when resolving a profile query
/// (`qRegion`/`qRegions`/`qAllRegions`). Matches `listSketchRegions`' default
/// so the region INDEX the UI emitted (from the kernel's own flood-fill order)
/// addresses the same region here.
pub const PROFILE_REGION_RESOLUTION: u32 = 128;

/// Resolve a profile query against the sketch table to a `(Sketch2d, frame)`,
/// or `None` when it doesn't resolve to a non-empty profile (ticket dhjn).
///
/// Honoured query forms — all addressing one sketch feature:
///   * `qCreatedBy(sketch_id)` — the WHOLE sketch (every closed region).
///   * `qAllRegions(sketch_id)` — same as above, explicit.
///   * `qRegion(sketch_id, i)` — only the i-th connected region.
///   * `qRegions(sketch_id, [i, j, …])` — only the listed regions.
///
/// Region indices come from [`crate::sketch_regions::find_regions`] (its
/// flood-fill order), which is exactly what `listSketchRegions` exposes to the
/// UI — so the index the user picked addresses the same region here. The
/// returned profile is the EXACT CSG of the selected region(s)
/// ([`crate::sketch_regions::restrict_sketch_to_points`]), holes included.
pub fn resolve_profile_query(
    query: &str,
    sketches: &std::collections::HashMap<FeatureId, SketchData>,
) -> Option<(Sketch2d, SketchFrame)> {
    use crate::queries::QueryExpr;
    let expr = crate::queries::parse_query(query).ok()?;
    let (sketch_id, indices): (FeatureId, Option<Vec<u32>>) = match expr {
        QueryExpr::CreatedBy(id) | QueryExpr::AllRegions(id) => (id, None),
        QueryExpr::Region(id, i) => (id, Some(vec![i])),
        QueryExpr::Regions(id, list) => (id, Some(list)),
        _ => return None,
    };
    let sketch_data = sketches.get(&sketch_id)?;
    let full = build_sketch2d_from_sub_calls(&sketch_data.sub_calls);
    if matches!(full, Sketch2d::Empty) {
        return None;
    }
    let sketch = match indices {
        None => full,
        Some(indices) => {
            // Map each requested index to its region's guaranteed-interior
            // point, then restrict the profile to the branches containing them.
            let regions =
                crate::sketch_regions::find_regions(&full, PROFILE_REGION_RESOLUTION);
            let points: Vec<[f32; 2]> = indices
                .iter()
                .filter_map(|&i| regions.iter().find(|r| r.index == i))
                .map(|r| r.interior)
                .collect();
            if points.is_empty() {
                return None;
            }
            let restricted =
                crate::sketch_regions::restrict_sketch_to_points(&full, &points);
            if matches!(restricted, Sketch2d::Empty) {
                return None;
            }
            restricted
        }
    };
    Some((sketch, sketch_data.frame))
}

pub fn build_sketch2d_from_sub_calls(sub_calls: &[FeatureCall]) -> Sketch2d {
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
            for segments in loops {
                let node = Sketch2d::Prim(Sketch2dPrimitive::Path {
                    segments,
                    closed: true,
                });
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
        let prim = match call.type_name.as_str() {
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
                Some(Sketch2dPrimitive::Rectangle {
                    center,
                    half_extents: [half_x, half_y],
                })
            }
            "sketch_circle" => {
                let d = match SketchCircleDef::parse(&call.params) {
                    Ok(d) => d,
                    Err(_) => continue,
                };
                if d.radius <= 0.0 {
                    continue;
                }
                Some(Sketch2dPrimitive::Circle {
                    center: d.center,
                    radius: d.radius,
                })
            }
            "sketch_arc" => {
                let d = match SketchArcDef::parse(&call.params) {
                    Ok(d) => d,
                    Err(_) => continue,
                };
                if d.radius <= 0.0 || d.sweep_angle.abs() < 1e-6 {
                    continue;
                }
                Some(Sketch2dPrimitive::Sector {
                    center: d.center,
                    radius: d.radius,
                    start_angle: d.start_angle,
                    sweep_angle: d.sweep_angle,
                })
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
                Some(Sketch2dPrimitive::Path {
                    segments: segs,
                    closed: true,
                })
            }
            "sketch_slot" => {
                let d = match SketchSlotDef::parse(&call.params) {
                    Ok(d) => d,
                    Err(_) => continue,
                };
                match slot_path_segments(d.center_a, d.center_b, d.radius) {
                    Some(segments) => Some(Sketch2dPrimitive::Path {
                        segments,
                        closed: true,
                    }),
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
                Some(Sketch2dPrimitive::Polygon { vertices: verts })
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
                Some(Sketch2dPrimitive::Path {
                    segments: segs,
                    closed: d.closed,
                })
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
                    Some(verts) => Some(Sketch2dPrimitive::Polygon { vertices: verts }),
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
        let Some(prim) = prim else { continue };
        let node = Sketch2d::Prim(prim);
        acc = Some(match acc {
            None => node,
            Some(prev) => Sketch2d::Union(Box::new(prev), Box::new(node)),
        });
    }
    acc.unwrap_or(Sketch2d::Empty)
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
fn slot_path_segments(
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
