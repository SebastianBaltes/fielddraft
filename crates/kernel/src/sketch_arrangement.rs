//! Imprint region arrangement (#412).
//!
//! Onshape identifies extrude/revolve profiles as **imprint regions**: the set
//! of ALL sketch curves — including open trim lines that cut through an area,
//! and overlapping / nested closed profiles — subdivides the sketch plane into
//! **atomic faces**, each individually selectable.
//!
//! FieldDraft's old region model (`sketch_regions::find_regions` on an
//! SDF-union flood-fill of the *lowered* `Sketch2d`) could not do this: the
//! lowering already dropped open separator lines, and nested/overlapping
//! closed profiles merged into one flood-fill region. This module computes a
//! true planar arrangement over the RAW sub-calls (which still carry every
//! curve) and returns the bounded atomic faces, each with an EXACT profile.
//!
//! See `docs/SKETCH-REGION-IMPRINT-PLAN.md` for the full design.

use fielddraft_protocol::sketch2d::{Aabb2, Sketch2d, Sketch2dPrimitive, SketchSegment};
use fielddraft_protocol::FeatureCall;
use std::f64::consts::TAU;

use crate::sketch::tools::{
    PathSegmentDef, SketchArcDef, SketchCircleDef, SketchEllipseDef, SketchLineDef,
    SketchPathDef, SketchPolygonDef, SketchRectangleDef, SketchRegularPolygonDef,
    SketchSlotDef, SketchSplineDef,
};

/// One atomic imprint face of a sketch: an EXACT closed profile plus the
/// metadata region-pickers need. Faces are returned in a deterministic order
/// (see [`arrange`]).
#[derive(Debug, Clone)]
pub struct AtomicFace {
    /// Exact 2D profile of this face (outer loop minus immediate holes).
    pub profile: Sketch2d,
    /// A point GUARANTEED inside this face (outside its holes).
    pub interior: [f32; 2],
    /// Approximate centroid (outer-loop vertex average).
    pub centroid: [f32; 2],
    /// Bounding box of the outer loop.
    pub aabb: Aabb2,
    /// Signed area of the face (outer minus holes), always ≥ 0.
    pub area: f32,
    /// Total length of the face's BOUNDARY — outer loop plus every hole loop —
    /// measured on the flattened polylines. It is what tells a big ambient face
    /// apart from a small specific one when a caller only knows a few of the
    /// bounding curves: the same named curve accounts for a large FRACTION of a
    /// plate's boundary and a negligible one of the sheet around it (#412).
    pub perimeter: f32,
    /// Per-primitive provenance of [`profile`](Self::profile), in the tree's
    /// DFS (= emission) order — outer loop first, then each hole. This is what
    /// lets a region-restricted extrude carry STABLE, entity-attributed wall
    /// subs instead of positional ones, so `qWall(op, entity)` binds a face on
    /// a picked region exactly as it does on a whole-sketch profile (#424).
    pub traces: Vec<crate::sketch::TracedPrim>,
}

/// Endpoint weld tolerance (mm). Snapped / solver-coincident endpoints are
/// bit-identical; this only absorbs float noise.
const WELD_TOL: f64 = 1e-4;
/// Faces below this area (mm²) are tracing artifacts, not regions.
const MIN_AREA: f64 = 1e-4;

// ── entity → exact boundary segments ─────────────────────────────────

/// One flattened straight micro-piece with its source-segment provenance and
/// the PARAMETER RANGE it covers on that source (`0 … 1` over the whole
/// source, in the source's own direction).
///
/// The range is what lets [`face_profile`] hand back an exact CUT arc instead
/// of the chords it was flattened into: a run of consecutive micro-pieces of
/// one source spans `[t0 … t1]`, and [`sub_segment`] restricts the analytic
/// curve to exactly that span (#445).
#[derive(Clone)]
struct Piece {
    a: [f64; 2],
    b: [f64; 2],
    src: usize,
    t0: f64,
    t1: f64,
}

/// Collect every non-construction entity's exact boundary segments, each
/// tagged with the SKETCH CURVE ENTITY it came from (`None` = the tool has no
/// per-segment curve entity to name, e.g. the point-only polygon/slot/path
/// tools) and with whether it is a FENCE curve (#452). The ids come from the
/// same registry replay the whole-sketch provenance trace uses
/// ([`crate::sketch::replay_allocations`]), so the two paths can never drift
/// apart.
///
/// LOCKSTEP: for a multi-curve tool the emission order below must match the
/// tool's `apply()` allocation order. Today that is only `sketch_rectangle`
/// (bottom, right, top, left in both) — anything else emits one curve or none.
fn collect_sources(
    sub_calls: &[FeatureCall],
) -> (Vec<SketchSegment>, Vec<Option<u32>>, Vec<bool>) {
    let allocs = crate::sketch::replay_allocations(sub_calls);
    let mut out: Vec<SketchSegment> = Vec::new();
    let mut src_ids: Vec<Option<u32>> = Vec::new();
    let mut src_fence: Vec<bool> = Vec::new();
    for (ci, call) in sub_calls.iter().enumerate() {
        let fence = crate::sketch::is_fence_call(call);
        let curves: &[u32] = allocs.get(ci).map(|a| a.curves.as_slice()).unwrap_or(&[]);
        // Everything this call pushes gets attributed at the end of the arm.
        let before = out.len();
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
                    if dist2(d.start, d.end) > WELD_TOL * WELD_TOL {
                        out.push(SketchSegment::Line {
                            start: d.start,
                            end: d.end,
                        });
                    }
                }
            }
            "sketch_arc" => {
                if let Ok(d) = SketchArcDef::parse(&call.params) {
                    if d.radius > 0.0 && d.sweep_angle.abs() > 1e-6 {
                        out.push(SketchSegment::Arc {
                            center: d.center,
                            radius: d.radius,
                            start_angle: d.start_angle,
                            sweep_angle: d.sweep_angle,
                        });
                    }
                }
            }
            "sketch_circle" => {
                if let Ok(d) = SketchCircleDef::parse(&call.params) {
                    if d.radius > 0.0 {
                        out.push(SketchSegment::Arc {
                            center: d.center,
                            radius: d.radius,
                            start_angle: 0.0,
                            sweep_angle: std::f32::consts::TAU,
                        });
                    }
                }
            }
            "sketch_rectangle" => {
                if let Ok(d) = SketchRectangleDef::parse(&call.params) {
                    let (lo_x, hi_x) = sort2(d.corner_a[0], d.corner_b[0]);
                    let (lo_y, hi_y) = sort2(d.corner_a[1], d.corner_b[1]);
                    if hi_x - lo_x > 0.0 && hi_y - lo_y > 0.0 {
                        let c = [[lo_x, lo_y], [hi_x, lo_y], [hi_x, hi_y], [lo_x, hi_y]];
                        for i in 0..4 {
                            out.push(SketchSegment::Line {
                                start: c[i],
                                end: c[(i + 1) % 4],
                            });
                        }
                    }
                }
            }
            "sketch_polygon" => {
                if let Ok(d) = SketchPolygonDef::parse(&call.params) {
                    push_polygon(&mut out, &d.vertices);
                }
            }
            "sketch_regular_polygon" => {
                if let Ok(d) = SketchRegularPolygonDef::parse(&call.params) {
                    if let Some(verts) =
                        crate::sketch::regular_polygon_vertices(d.center, d.radius, d.sides, d.inscribed)
                    {
                        let flat: Vec<f32> = verts.iter().flat_map(|v| [v[0], v[1]]).collect();
                        push_polygon(&mut out, &flat);
                    }
                }
            }
            "sketch_ellipse" => {
                if let Ok(d) = SketchEllipseDef::parse(&call.params) {
                    if d.radius_x > 0.0 && d.radius_y > 0.0 {
                        out.extend(fielddraft_protocol::sketch2d::ellipse_quad_beziers(
                            d.center,
                            d.radius_x,
                            d.radius_y,
                            d.rotation,
                            0.0,
                            std::f32::consts::TAU,
                            d.segments.max(0.0) as u32,
                            0.01,
                        ));
                    }
                }
            }
            "sketch_spline" => {
                if let Ok(d) = SketchSplineDef::parse(&call.params) {
                    out.extend(crate::sketch::lower::lower_spline_segments(&d));
                }
            }
            "sketch_slot" => {
                if let Ok(d) = SketchSlotDef::parse(&call.params) {
                    if let Some(segs) =
                        crate::sketch::lower::slot_path_segments(d.center_a, d.center_b, d.radius)
                    {
                        out.extend(segs);
                    }
                }
            }
            "sketch_path" => {
                if let Ok(d) = SketchPathDef::parse(&call.params) {
                    for c in &d.contours {
                        let mut prev = c.start;
                        for seg in &c.segments {
                            match seg {
                                PathSegmentDef::Line { to } => {
                                    out.push(SketchSegment::Line { start: prev, end: *to });
                                    prev = *to;
                                }
                                PathSegmentDef::Quad { ctrl, to } => {
                                    out.push(SketchSegment::QuadBezier {
                                        p0: prev,
                                        p1: *ctrl,
                                        p2: *to,
                                    });
                                    prev = *to;
                                }
                            }
                        }
                        // Close the loop if the chain does not already land on
                        // `start` — the same implicit closing the lowering adds.
                        if c.closed && dist2(prev, c.start) > WELD_TOL * WELD_TOL {
                            out.push(SketchSegment::Line { start: prev, end: c.start });
                        }
                    }
                }
            }
            // `sketch_mirror` / `sketch_offset` transform the accumulated
            // PROFILE TREE, not individual entities, so they have no boundary
            // curves of their own to arrange. [`arrange`] refuses such sketches
            // outright (see `has_unarrangeable`) rather than silently dropping
            // their geometry.
            _ => {}
        }
        // Attribute this call's freshly pushed segments. One curve entity
        // (line, arc, circle, ellipse, spline) names ALL of them — an ellipse
        // and a spline are one entity split into many beziers. Several curves
        // (rectangle) map positionally, which the LOCKSTEP note above pins
        // down. No curve at all (polygon, slot, path) means no stable name.
        let pushed = out.len() - before;
        for k in 0..pushed {
            src_ids.push(match curves.len() {
                0 => None,
                1 => Some(curves[0]),
                n if n == pushed => Some(curves[k]),
                _ => None,
            });
            src_fence.push(fence);
        }
    }
    (out, src_ids, src_fence)
}

fn push_polygon(out: &mut Vec<SketchSegment>, flat: &[f32]) {
    let verts: Vec<[f32; 2]> = flat
        .chunks(2)
        .filter(|c| c.len() == 2)
        .map(|c| [c[0], c[1]])
        .collect();
    if verts.len() >= 3 {
        for i in 0..verts.len() {
            out.push(SketchSegment::Line {
                start: verts[i],
                end: verts[(i + 1) % verts.len()],
            });
        }
    }
}

/// Flatten one source segment to a polyline (≥ 2 points, f64).
fn flatten(seg: &SketchSegment) -> Vec<[f64; 2]> {
    match seg {
        SketchSegment::Line { start, end } => vec![f2(*start), f2(*end)],
        SketchSegment::Arc {
            center,
            radius,
            start_angle,
            sweep_angle,
        } => {
            let (cx, cy) = (center[0] as f64, center[1] as f64);
            let r = *radius as f64;
            let s = *start_angle as f64;
            let w = *sweep_angle as f64;
            // ~64 subdivisions per full turn, at least 2.
            let n = ((w.abs() / TAU * 64.0).ceil() as usize).max(2);
            let mut pts: Vec<[f64; 2]> = (0..=n)
                .map(|i| {
                    let t = s + w * (i as f64 / n as f64);
                    [cx + r * t.cos(), cy + r * t.sin()]
                })
                .collect();
            // A full turn must close EXACTLY. `sweep` is an f32 and a full
            // circle's TAU rounds slightly past 2pi, so the last sample would
            // overshoot the first by ~1e-6 — enough for the closing chord to
            // properly CROSS the opening one, which would then be split as a
            // real intersection and cost the arc its exact reconstruction.
            if w.abs() >= TAU - 1e-6 {
                let first = pts[0];
                *pts.last_mut().unwrap() = first;
            }
            pts
        }
        SketchSegment::QuadBezier { p0, p1, p2 } => {
            let n = 16usize;
            (0..=n)
                .map(|i| {
                    let t = i as f64 / n as f64;
                    let mt = 1.0 - t;
                    [
                        mt * mt * p0[0] as f64 + 2.0 * mt * t * p1[0] as f64 + t * t * p2[0] as f64,
                        mt * mt * p0[1] as f64 + 2.0 * mt * t * p1[1] as f64 + t * t * p2[1] as f64,
                    ]
                })
                .collect()
        }
    }
}

// ── planarization ────────────────────────────────────────────────────

/// Spatial-hash vertex pool: welds near-identical points to one id.
struct VertexPool {
    positions: Vec<[f64; 2]>,
    cells: std::collections::HashMap<(i64, i64), Vec<usize>>,
}
impl VertexPool {
    fn new() -> Self {
        Self {
            positions: Vec::new(),
            cells: std::collections::HashMap::new(),
        }
    }
    fn id_for(&mut self, p: [f64; 2]) -> usize {
        let cx = (p[0] / WELD_TOL).round() as i64;
        let cy = (p[1] / WELD_TOL).round() as i64;
        for dx in -1..=1 {
            for dy in -1..=1 {
                if let Some(bucket) = self.cells.get(&(cx + dx, cy + dy)) {
                    for &id in bucket {
                        let q = self.positions[id];
                        if (q[0] - p[0]).hypot(q[1] - p[1]) <= WELD_TOL {
                            return id;
                        }
                    }
                }
            }
        }
        let id = self.positions.len();
        self.positions.push(p);
        self.cells.entry((cx, cy)).or_default().push(id);
        id
    }
}

/// Proper intersection parameter `t` on segment (p→q) with (a→b), plus the
/// point — `None` when they don't cross within the open interval on BOTH.
fn seg_intersection(
    p: [f64; 2],
    q: [f64; 2],
    a: [f64; 2],
    b: [f64; 2],
) -> Option<(f64, [f64; 2])> {
    let r = [q[0] - p[0], q[1] - p[1]];
    let s = [b[0] - a[0], b[1] - a[1]];
    let denom = r[0] * s[1] - r[1] * s[0];
    if denom.abs() < 1e-12 {
        return None; // parallel / collinear — handled by endpoint-on-seg split
    }
    let qp = [a[0] - p[0], a[1] - p[1]];
    let t = (qp[0] * s[1] - qp[1] * s[0]) / denom;
    let u = (qp[0] * r[1] - qp[1] * r[0]) / denom;
    let eps = 1e-9;
    if !(t > eps && t < 1.0 - eps && u > eps && u < 1.0 - eps) {
        return None;
    }
    let x = [p[0] + t * r[0], p[1] + t * r[1]];
    // A crossing within weld tolerance of ANY of the four endpoints is a shared
    // vertex (or float noise at a seam), not an interior cut.
    for e in [p, q, a, b] {
        if (x[0] - e[0]).hypot(x[1] - e[1]) <= WELD_TOL {
            return None;
        }
    }
    Some((t, x))
}

/// Parameter of the projection of `x` onto segment (p→q) if `x` lies on the
/// segment's INTERIOR within weld tol, else `None`.
fn point_on_seg_t(p: [f64; 2], q: [f64; 2], x: [f64; 2]) -> Option<f64> {
    let d = [q[0] - p[0], q[1] - p[1]];
    let len2 = d[0] * d[0] + d[1] * d[1];
    if len2 < WELD_TOL * WELD_TOL {
        return None;
    }
    let t = ((x[0] - p[0]) * d[0] + (x[1] - p[1]) * d[1]) / len2;
    if t <= 1e-9 || t >= 1.0 - 1e-9 {
        return None;
    }
    let proj = [p[0] + t * d[0], p[1] + t * d[1]];
    if (proj[0] - x[0]).hypot(proj[1] - x[1]) <= WELD_TOL {
        let len = len2.sqrt();
        if t * len > WELD_TOL && (1.0 - t) * len > WELD_TOL {
            return Some(t);
        }
    }
    None
}

// ── DCEL face extraction ─────────────────────────────────────────────

struct Edge {
    u: usize,
    v: usize,
    src: usize,
    /// Parameter range on [`Self::src`], oriented `u → v`.
    t0: f64,
    t1: f64,
}

impl Edge {
    /// The source and its parameter range as traversed by half-edge `h`
    /// (`h & 1 == 1` walks `v → u`, which reverses the range).
    fn walk(edges: &[Edge], h: usize) -> (usize, f64, f64) {
        let e = &edges[h >> 1];
        if h & 1 == 1 {
            (e.src, e.t1, e.t0)
        } else {
            (e.src, e.t0, e.t1)
        }
    }
}

/// A bounded (positive-area) face of the arrangement.
struct RawFace {
    /// Ordered outer-boundary vertex ids (welded pool ids).
    verts: Vec<usize>,
    /// Per boundary step, the HALF-EDGE traversed (`edge = h >> 1`,
    /// direction `h & 1`). Cancelling a half-edge against its twin is what
    /// turns a set of atomic faces into the boundary of their union
    /// ([`union_loops`]).
    half_edges: Vec<usize>,
    area: f64,
}

/// Build the arrangement and return its bounded faces + the vertex pool.
///
/// `fence` marks, per source, whether it is a FENCE curve (#452) — the carrier
/// face's own boundary rather than something the sketch drew. A fence curve may
/// **close** a region, never **cut** one, so before anything else the fence is
/// trimmed to the area the drawn curves leave open ([`trim_fence_to_open_area`]).
fn build_faces(
    sources: &[SketchSegment],
    fence: &[bool],
) -> (Vec<RawFace>, Vec<[f64; 2]>, Vec<Edge>) {
    // 1. Flatten every source into straight pieces tagged with the source id
    //    AND the parameter span each piece covers on it.
    let mut pieces: Vec<Piece> = Vec::new();
    for (si, seg) in sources.iter().enumerate() {
        let poly = flatten(seg);
        let n = poly.len().saturating_sub(1);
        for (k, w) in poly.windows(2).enumerate() {
            if dist2f(w[0], w[1]) > WELD_TOL * WELD_TOL {
                pieces.push(Piece {
                    a: w[0],
                    b: w[1],
                    src: si,
                    t0: k as f64 / n as f64,
                    t1: (k + 1) as f64 / n as f64,
                });
            }
        }
    }
    let pieces = trim_fence_to_open_area(pieces, fence);
    faces_from_pieces(pieces)
}

/// Drop every fence piece that runs through area the DRAWN curves already
/// bound (#452).
///
/// Without this a fence would SUBDIVIDE what the sketch itself encloses, and
/// that is wrong twice over. Semantically: the carrier's edge is a boundary
/// only where the sketch has none of its own — the whole point of the fence is
/// to close what is open, and a drawn rectangle is not open. Practically: our
/// carrier face need not be Onshape's. Onshape folds coplanar faces into ONE
/// (#443) and we do not, so a sketch whose curves reach past OUR piece of that
/// plane would be cut at a seam Onshape's model has nowhere — measured on
/// #419's `Sketch 4`, whose 44 × 44 plate stands on a 38 × 38 wall and came
/// apart into 19 regions instead of 15. Trimmed, the fence can only ADD regions
/// where there were none, never split one that exists.
///
/// „Already bound" means: inside any BOUNDED face of the drawn-only
/// arrangement. That includes the inside of a hole (a hole is a bounded face
/// too), which is right — the drawn curves fence it either way.
///
/// The test is per SUB-PIECE, and the fence is cut against the drawn pieces
/// first: a fence line that enters and leaves a drawn region has to lose only
/// the stretch INSIDE it. Judged whole, a line crossing a rectangle would be
/// kept or dropped by its midpoint alone — which split #419's `Sketch 43`
/// region in two, the very thing this function exists to prevent.
fn trim_fence_to_open_area(pieces: Vec<Piece>, fence: &[bool]) -> Vec<Piece> {
    let is_fence = |p: &Piece| fence.get(p.src).copied().unwrap_or(false);
    if !pieces.iter().any(is_fence) {
        return pieces;
    }
    let (drawn, fence_pieces): (Vec<Piece>, Vec<Piece>) =
        pieces.into_iter().partition(|p| !is_fence(p));
    if drawn.is_empty() {
        return fence_pieces;
    }
    let (faces, verts, _) = faces_from_pieces(drawn.clone());
    let polys: Vec<Vec<[f64; 2]>> = faces
        .iter()
        .map(|f| f.verts.iter().map(|&i| verts[i]).collect())
        .collect();
    let mut out = drawn;
    out.extend(
        split_pieces_at(&fence_pieces, &out, false)
            .into_iter()
            .filter(|p| {
                let mid = [(p.a[0] + p.b[0]) * 0.5, (p.a[1] + p.b[1]) * 0.5];
                !polys.iter().any(|poly| point_in_poly(mid, poly))
            }),
    );
    out
}

/// Split every piece of `targets` where a piece of `against` crosses it or
/// ends on its interior — the planarization cut, shared by the arrangement
/// itself (`targets == against`) and by the fence trim, which cuts fence
/// pieces against the DRAWN ones only.
fn split_pieces_at(targets: &[Piece], against: &[Piece], same: bool) -> Vec<Piece> {
    let mut sub_pieces: Vec<Piece> = Vec::new();
    for (i, pi) in targets.iter().enumerate() {
        let mut cuts: Vec<f64> = Vec::new();
        for (j, pj) in against.iter().enumerate() {
            if same && i == j {
                continue;
            }
            if let Some((t, _)) = seg_intersection(pi.a, pi.b, pj.a, pj.b) {
                cuts.push(t);
            }
            for &x in &[pj.a, pj.b] {
                if let Some(t) = point_on_seg_t(pi.a, pi.b, x) {
                    cuts.push(t);
                }
            }
        }
        if cuts.is_empty() {
            sub_pieces.push(pi.clone());
            continue;
        }
        cuts.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let d = [pi.b[0] - pi.a[0], pi.b[1] - pi.a[1]];
        // Local piece parameter → global source parameter.
        let src_t = |t: f64| pi.t0 + t * (pi.t1 - pi.t0);
        let mut prev = pi.a;
        let mut prev_t = 0.0;
        for &t in &cuts {
            if t - prev_t < 1e-9 {
                continue;
            }
            let cp = [pi.a[0] + t * d[0], pi.a[1] + t * d[1]];
            if dist2f(prev, cp) > WELD_TOL * WELD_TOL {
                sub_pieces.push(Piece {
                    a: prev,
                    b: cp,
                    src: pi.src,
                    t0: src_t(prev_t),
                    t1: src_t(t),
                });
                prev = cp;
                prev_t = t;
            }
        }
        if dist2f(prev, pi.b) > WELD_TOL * WELD_TOL {
            sub_pieces.push(Piece {
                a: prev,
                b: pi.b,
                src: pi.src,
                t0: src_t(prev_t),
                t1: pi.t1,
            });
        }
    }
    sub_pieces
}

/// Planarize a piece soup into the arrangement's bounded faces.
fn faces_from_pieces(pieces: Vec<Piece>) -> (Vec<RawFace>, Vec<[f64; 2]>, Vec<Edge>) {
    // 2. Planarize: split every piece at intersections + interior T-junctions.
    let sub_pieces = split_pieces_at(&pieces, &pieces, true);

    // 3. Weld endpoints → graph edges (dedup unordered vertex pairs).
    let mut pool = VertexPool::new();
    let mut edges: Vec<Edge> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for sp in &sub_pieces {
        let u = pool.id_for(sp.a);
        let v = pool.id_for(sp.b);
        if u == v {
            continue;
        }
        let key = (u.min(v), u.max(v));
        if !seen.insert(key) {
            continue;
        }
        edges.push(Edge {
            u,
            v,
            src: sp.src,
            t0: sp.t0,
            t1: sp.t1,
        });
    }
    let verts = pool.positions.clone();

    // 4. DCEL traversal (half-edge h: edge h>>1, dir h&1; 0 = u→v, 1 = v→u).
    let tail = |h: usize| {
        let e = &edges[h >> 1];
        if h & 1 == 1 {
            e.v
        } else {
            e.u
        }
    };
    let head = |h: usize| {
        let e = &edges[h >> 1];
        if h & 1 == 1 {
            e.u
        } else {
            e.v
        }
    };
    // Departure angle at the tail.
    let angle = |h: usize| {
        let t = verts[tail(h)];
        let hd = verts[head(h)];
        (hd[1] - t[1]).atan2(hd[0] - t[0])
    };
    let mut outgoing: std::collections::HashMap<usize, Vec<usize>> = std::collections::HashMap::new();
    for h in 0..edges.len() * 2 {
        outgoing.entry(tail(h)).or_default().push(h);
    }
    for list in outgoing.values_mut() {
        list.sort_by(|&a, &b| angle(a).partial_cmp(&angle(b)).unwrap());
    }

    let mut visited = vec![false; edges.len() * 2];
    let mut faces: Vec<RawFace> = Vec::new();
    for start in 0..edges.len() * 2 {
        if visited[start] {
            continue;
        }
        let mut face_verts: Vec<usize> = Vec::new();
        let mut face_halfs: Vec<usize> = Vec::new();
        let mut h = start;
        let mut ok = false;
        for _ in 0..=edges.len() * 2 {
            visited[h] = true;
            face_verts.push(tail(h));
            face_halfs.push(h);
            // next(h): clockwise-most turn after twin(h) at head(h).
            let twin = h ^ 1;
            let list = match outgoing.get(&head(h)) {
                Some(l) if !l.is_empty() => l,
                _ => break,
            };
            let idx = match list.iter().position(|&x| x == twin) {
                Some(i) => i,
                None => break,
            };
            h = list[(idx + list.len() - 1) % list.len()];
            if h == start {
                ok = true;
                break;
            }
            if visited[h] {
                break;
            }
        }
        if !ok || face_verts.len() < 3 {
            continue;
        }
        let area = signed_area(&face_verts, &verts);
        if area > MIN_AREA {
            faces.push(RawFace {
                verts: face_verts,
                half_edges: face_halfs,
                area,
            });
        }
    }
    (faces, verts, edges)
}

/// Boundary loops of the UNION of several atomic faces (#424).
///
/// Every bounded face is traversed counter-clockwise, so an arrangement edge
/// shared by two of them is walked ONCE IN EACH DIRECTION. Dropping those
/// mutually-cancelling half-edge pairs leaves exactly the half-edges that
/// border the union from outside — the classic half-edge cancellation, and an
/// EXACT construction, not an approximation: no geometry is recomputed, only
/// selected.
///
/// Why the union needs its own boundary at all: the shared edge is INTERIOR to
/// the union and therefore produces no wall face. Unioning the two closed
/// contours instead (the previous behaviour) put the shared sketch curve into
/// the profile TWICE, which made [`crate::sketch::wall_sub_table`] see a
/// duplicate sub-id and drop the whole table — the entity-named walls of both
/// regions degraded to positional subs, and `qWall(op, entity)` could never
/// bind again (measured on #419: features 24 and 25).
///
/// The surviving half-edges form closed loops by construction (every vertex
/// keeps equal in- and out-degree), so they are re-threaded with the SAME
/// clockwise-most-turn rule [`build_faces`] uses.
fn union_loops(
    selected: &[usize],
    faces: &[RawFace],
    edges: &[Edge],
    verts: &[[f64; 2]],
) -> Vec<RawFace> {
    let mut keep: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for &i in selected {
        for &h in &faces[i].half_edges {
            keep.insert(h);
        }
    }
    // Drop every half-edge whose twin also borders the selection: that edge
    // has selected faces on BOTH sides, so it lies inside the union.
    let interior: Vec<usize> = keep.iter().copied().filter(|h| keep.contains(&(h ^ 1))).collect();
    for h in interior {
        keep.remove(&h);
    }
    if keep.is_empty() {
        return Vec::new();
    }

    let tail = |h: usize| if h & 1 == 1 { edges[h >> 1].v } else { edges[h >> 1].u };
    let head = |h: usize| if h & 1 == 1 { edges[h >> 1].u } else { edges[h >> 1].v };
    let angle = |h: usize| {
        let t = verts[tail(h)];
        let hd = verts[head(h)];
        (hd[1] - t[1]).atan2(hd[0] - t[0])
    };
    // The turn rule needs EVERY half-edge at a vertex, not just the kept ones:
    // "clockwise-most after the twin" is defined against the full local star,
    // and skipping the interior spokes would turn the wrong corner where three
    // regions meet. Candidates are filtered to `keep` while stepping instead.
    let mut outgoing: std::collections::HashMap<usize, Vec<usize>> = std::collections::HashMap::new();
    for h in 0..edges.len() * 2 {
        outgoing.entry(tail(h)).or_default().push(h);
    }
    for list in outgoing.values_mut() {
        list.sort_by(|&a, &b| angle(a).partial_cmp(&angle(b)).unwrap());
    }

    let mut visited: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut out: Vec<RawFace> = Vec::new();
    let mut starts: Vec<usize> = keep.iter().copied().collect();
    starts.sort_unstable();
    for start in starts {
        if visited.contains(&start) {
            continue;
        }
        let mut loop_verts: Vec<usize> = Vec::new();
        let mut loop_halfs: Vec<usize> = Vec::new();
        let mut h = start;
        let mut closed = false;
        for _ in 0..=keep.len() {
            visited.insert(h);
            loop_verts.push(tail(h));
            loop_halfs.push(h);
            let Some(list) = outgoing.get(&head(h)) else { break };
            let Some(idx) = list.iter().position(|&x| x == (h ^ 1)) else { break };
            // Walk the star clockwise from the twin until a KEPT half-edge
            // turns up — interior spokes are passed over, not turned into.
            let mut next = None;
            for k in 1..=list.len() {
                let cand = list[(idx + list.len() - k) % list.len()];
                if keep.contains(&cand) {
                    next = Some(cand);
                    break;
                }
            }
            let Some(n) = next else { break };
            h = n;
            if h == start {
                closed = true;
                break;
            }
            if visited.contains(&h) {
                break;
            }
        }
        if !closed || loop_verts.len() < 3 {
            continue;
        }
        let area = signed_area(&loop_verts, verts);
        if area > MIN_AREA {
            out.push(RawFace {
                verts: loop_verts,
                half_edges: loop_halfs,
                area,
            });
        }
    }
    out
}

// ── public entry ─────────────────────────────────────────────────────

/// Everything the arrangement of one sketch yields, kept together so both
/// public entries ([`arrange`] and [`arrange_union`]) run the SAME pipeline —
/// a region index therefore means the same face to both.
struct Arrangement {
    sources: Vec<SketchSegment>,
    src_ids: Vec<Option<u32>>,
    /// Per source segment: does it come from a FENCE sub-call (#452)?
    src_fence: Vec<bool>,
    faces: Vec<RawFace>,
    edges: Vec<Edge>,
    verts: Vec<[f64; 2]>,
    polys: Vec<Vec<[f64; 2]>>,
    /// Immediate children (holes) per raw face.
    children: Vec<Vec<usize>>,
}

impl Arrangement {
    fn build(sub_calls: &[FeatureCall]) -> Option<Self> {
        if has_unarrangeable(sub_calls) {
            return None;
        }
        let (sources, src_ids, src_fence) = collect_sources(sub_calls);
        if sources.is_empty() {
            return None;
        }
        let (faces, verts, edges) = build_faces(&sources, &src_fence);
        if faces.is_empty() {
            return None;
        }
        // Face polygons (coords) for containment tests.
        let polys: Vec<Vec<[f64; 2]>> = faces
            .iter()
            .map(|f| f.verts.iter().map(|&i| verts[i]).collect())
            .collect();
        // Immediate-parent per face: smallest-area face strictly containing it.
        let n = faces.len();
        let mut parent: Vec<Option<usize>> = vec![None; n];
        for i in 0..n {
            let pt = interior_of_poly(&polys[i]);
            let mut best: Option<usize> = None;
            for j in 0..n {
                if i == j || faces[j].area <= faces[i].area {
                    continue;
                }
                if point_in_poly(pt, &polys[j]) {
                    match best {
                        Some(b) if faces[b].area <= faces[j].area => {}
                        _ => best = Some(j),
                    }
                }
            }
            parent[i] = best;
        }
        let children: Vec<Vec<usize>> = (0..n)
            .map(|j| (0..n).filter(|&i| parent[i] == Some(j)).collect())
            .collect();
        Some(Self { sources, src_ids, src_fence, faces, edges, verts, polys, children })
    }

    /// Does this face's OUTER boundary carry at least one DRAWN (non-fence)
    /// curve? A fence curve — the carrier face's own boundary, projected into
    /// the sketch plane (#452) — may CLOSE a region the user's curves leave
    /// open, but it never bounds one on its own.
    ///
    /// The face whose outer loop is pure fence is the carrier face itself
    /// (minus whatever the sketch drew on it): the sketch says nothing about
    /// it, and it is already addressable — better named — as a FACE profile
    /// (`qWall(…)` / `qFace(…)`, the #442 path). Handing it out here as well
    /// would add a region to EVERY face-mounted sketch and renumber all the
    /// others, for a selection that is a duplicate.
    ///
    /// A pure-fence face nested INSIDE a drawn one is a different matter: it
    /// is a hole of the carrier (there is no material there), and it stays a
    /// hole — this filter only decides who is a REGION of its own.
    fn has_drawn_boundary(&self, f: &RawFace) -> bool {
        f.half_edges
            .iter()
            .any(|&h| !self.src_fence[self.edges[h >> 1].src])
    }

    fn profile_of(&self, face: &RawFace) -> (Sketch2d, crate::sketch::TracedPrim) {
        face_profile(face, &self.sources, &self.src_ids, &self.verts, &self.edges)
    }

    /// Sort key of one boundary loop: ascending `aabb.min.y`, then
    /// `aabb.min.x`, then area — the SAME rule [`Self::atomic_faces`] orders
    /// regions by.
    ///
    /// The order of the loops decides their boundary-loop ORDINAL (#424), and
    /// an ordinal is part of a wall's persistent name. Left to the order they
    /// arrive in it would be a traversal artefact: [`union_loops`] starts at
    /// the lowest kept half-edge index, and `children` lists holes by raw face
    /// index — both change the moment the arrangement is built in a different
    /// order, which would silently re-name every wall of a multi-contour
    /// profile. Keying off the geometry makes the ordinal a property of the
    /// SHAPE, the same reasoning [`rotate_to_g1_break`] applies inside one loop.
    fn loop_key(&self, f: &RawFace) -> (f64, f64, f64) {
        let poly: Vec<[f64; 2]> = f.verts.iter().map(|&i| self.verts[i]).collect();
        let bb = poly_aabb_f(&poly);
        (bb.min[1] as f64, bb.min[0] as f64, f.area)
    }

    /// Assemble one [`AtomicFace`] from a set of outer loops plus the holes
    /// they enclose. `outer_area` is the loops' gross area.
    ///
    /// Tree shape is `Difference(Difference(Union(o1, o2, …), h1), h2)`, whose
    /// DFS order is o1, o2, …, h1, h2 — the order
    /// [`crate::sketch::wall_sub_table`] walks in, so the trace list is pushed
    /// in exactly that order and the two sequences stay aligned by
    /// construction. Both groups are put in [`Self::loop_key`] order first, so
    /// that DFS position — the wall's boundary-loop ordinal — is geometric.
    fn assemble(&self, outer: &[RawFace], holes: &[usize]) -> Option<AtomicFace> {
        let mut order: Vec<&RawFace> = outer.iter().collect();
        order.sort_by(|a, b| {
            self.loop_key(a).partial_cmp(&self.loop_key(b)).unwrap()
        });
        let mut holes: Vec<usize> = holes.to_vec();
        holes.sort_by(|&a, &b| {
            self.loop_key(&self.faces[a])
                .partial_cmp(&self.loop_key(&self.faces[b]))
                .unwrap()
        });
        let holes = &holes[..];
        let mut profile = Sketch2d::Empty;
        let mut traces = Vec::new();
        let mut outer_polys: Vec<Vec<[f64; 2]>> = Vec::new();
        let mut outer_area = 0.0;
        for lo in order {
            let (p, t) = self.profile_of(lo);
            if matches!(p, Sketch2d::Empty) {
                continue;
            }
            profile = match profile {
                Sketch2d::Empty => p,
                prev => Sketch2d::Union(Box::new(prev), Box::new(p)),
            };
            traces.push(t);
            outer_polys.push(lo.verts.iter().map(|&i| self.verts[i]).collect());
            outer_area += lo.area;
        }
        if matches!(profile, Sketch2d::Empty) {
            return None;
        }
        let mut hole_polys: Vec<&Vec<[f64; 2]>> = Vec::new();
        let mut hole_area = 0.0;
        for &c in holes {
            let (hole, hole_trace) = self.profile_of(&self.faces[c]);
            if !matches!(hole, Sketch2d::Empty) {
                profile = Sketch2d::Difference(Box::new(profile), Box::new(hole));
                traces.push(hole_trace);
                hole_polys.push(&self.polys[c]);
                hole_area += self.faces[c].area;
            }
        }
        let area = (outer_area - hole_area).max(0.0);
        if area < MIN_AREA {
            return None;
        }
        let perimeter: f64 = outer_polys
            .iter()
            .chain(hole_polys.iter().copied())
            .map(|p| closed_poly_len(p))
            .sum();
        // The interior point must lie in the LARGEST outer loop: a disjoint
        // selection has several, and a point from the small one would mislead
        // every containment test built on it.
        let biggest = outer_polys
            .iter()
            .max_by(|a, b| {
                signed_area_poly(a).partial_cmp(&signed_area_poly(b)).unwrap()
            })
            .expect("non-empty profile has an outer loop");
        let interior = interior_excluding(biggest, &hole_polys, &profile);
        let mut aabb = poly_aabb(&outer_polys[0]);
        for p in &outer_polys[1..] {
            let a = poly_aabb(p);
            for c in 0..2 {
                aabb.min[c] = aabb.min[c].min(a.min[c]);
                aabb.max[c] = aabb.max[c].max(a.max[c]);
            }
        }
        Some(AtomicFace {
            profile,
            interior,
            centroid: poly_centroid(biggest),
            aabb,
            area: area as f32,
            perimeter: perimeter as f32,
            traces,
        })
    }

    /// The atomic faces in the deterministic public order, each paired with
    /// its RAW face index (what [`union_loops`] needs).
    fn atomic_faces(&self) -> Vec<(usize, AtomicFace)> {
        let mut out: Vec<(usize, AtomicFace)> = (0..self.faces.len())
            .filter(|&i| self.has_drawn_boundary(&self.faces[i]))
            .filter_map(|i| {
                let f = std::slice::from_ref(&self.faces[i]);
                self.assemble(f, &self.children[i]).map(|a| (i, a))
            })
            .collect();
        // Deterministic order — geometry-derived, independent of scan order.
        out.sort_by(|a, b| {
            (a.1.aabb.min[1], a.1.aabb.min[0], a.1.area)
                .partial_cmp(&(b.1.aabb.min[1], b.1.aabb.min[0], b.1.area))
                .unwrap()
        });
        out
    }
}

/// Compute the bounded atomic imprint faces of a sketch from its raw
/// sub-calls, in a deterministic order (ascending `aabb.min.y`, then
/// `aabb.min.x`, then `area`).
pub fn arrange(sub_calls: &[FeatureCall]) -> Vec<AtomicFace> {
    let Some(arr) = Arrangement::build(sub_calls) else {
        return Vec::new();
    };
    arr.atomic_faces().into_iter().map(|(_, f)| f).collect()
}

/// The profile of the UNION of the atomic faces at the given [`arrange`]
/// indices — ONE region with ONE boundary, not a union of closed contours
/// (#424).
///
/// This is what a multi-region selection (`qRegions(sketch, [i, j])`) must
/// extrude. The shared boundary between two selected regions lies INSIDE the
/// result and is dropped by [`union_loops`], so it never enters the profile,
/// never mints a wall branch nobody can see, and — the reason this exists —
/// never makes the same sketch curve appear twice in the wall-sub table.
///
/// `None` when the sketch has no arrangement or the indices select nothing.
pub fn arrange_union(sub_calls: &[FeatureCall], indices: &[u32]) -> Option<AtomicFace> {
    let arr = Arrangement::build(sub_calls)?;
    let public = arr.atomic_faces();
    let selected: Vec<usize> = indices
        .iter()
        .filter_map(|&i| public.get(i as usize).map(|(raw, _)| *raw))
        .collect();
    if selected.is_empty() {
        return None;
    }
    if selected.len() == 1 {
        // Nothing to cancel — reuse the atomic face verbatim so a single-region
        // pick cannot drift from `arrange`.
        return public
            .into_iter()
            .find(|(raw, _)| *raw == selected[0])
            .map(|(_, f)| f);
    }
    let outer = union_loops(&selected, &arr.faces, &arr.edges, &arr.verts);
    if outer.is_empty() {
        return None;
    }
    // Holes of the union: every hole of a selected face that was not itself
    // selected. A selected hole is filled by its own selection, so it stops
    // being a hole — and its boundary already cancelled in `union_loops`.
    let mut holes: Vec<usize> = Vec::new();
    for &i in &selected {
        for &c in &arr.children[i] {
            if !selected.contains(&c) && !holes.contains(&c) {
                holes.push(c);
            }
        }
    }
    arr.assemble(&outer, &holes)
}

/// Does the sub-spine contain a tool the arrangement cannot represent? `mirror`
/// and `offset` rewrite the accumulated PROFILE TREE rather than contributing
/// boundary curves, so an arrangement over the raw entities would silently omit
/// the mirrored copy / the offset shift. Such sketches report NO regions, which
/// makes the caller fall back to the whole-sketch profile (`qCreatedBy`) —
/// visibly coarser, never geometrically wrong. See plan doc §5.
fn has_unarrangeable(sub_calls: &[FeatureCall]) -> bool {
    sub_calls
        .iter()
        .any(|c| matches!(c.type_name.as_str(), "sketch_mirror" | "sketch_offset"))
}

/// Reconstruct one face's outer boundary as an exact closed `Path`.
///
/// The boundary walk is grouped into RUNS: maximal stretches of consecutive
/// steps that come from the same source and whose parameter spans are
/// contiguous. A straight source's run is emitted step by step (a line is
/// already exact, and splitting it keeps the per-step provenance one-to-one);
/// a CURVED source's run is emitted as ONE exact analytic segment restricted
/// to the run's parameter span ([`sub_segment`]).
///
/// That restriction is the whole of #445. Before it, a curved source counted as
/// exact only when it was used WHOLE and UNCUT; a circle that a profile's own
/// boundary cuts — the ordinary shape of a multi-region extrude — decayed into
/// its ~50 flattening chords. That cost the geometry (50 planes where Onshape
/// has one cylinder) and, because each chord became its own G1 wall carrying
/// the same sketch entity, it cost the NAMES: 50 occurrences of one curve blow
/// past [`crate::csg::BranchId::OCCURRENCE_MAX`], `wall_sub_table` refuses the
/// table, and every wall of the sweep falls back to a positional sub. A cut arc
/// is still one arc, so it is still one wall with one name.
///
/// Returns the path together with its [`crate::sketch::TracedPrim`] — one
/// stable sketch-entity id per emitted SEGMENT, in the same order, so the
/// picked region's walls carry the same names a whole-sketch profile's do.
fn face_profile(
    face: &RawFace,
    sources: &[SketchSegment],
    src_ids: &[Option<u32>],
    verts: &[[f64; 2]],
    edges: &[Edge],
) -> (Sketch2d, crate::sketch::TracedPrim) {
    let n = face.verts.len();
    // Per boundary step: source id and parameter span, in TRAVERSAL direction.
    let steps: Vec<(usize, f64, f64)> = (0..n)
        .map(|k| Edge::walk(edges, face.half_edges[k]))
        .collect();
    // A run continues into step `k` when it stays on the same source and picks
    // its parameter up exactly where the previous step left off. Both cut
    // points and flattening joints reproduce the parameter bit-for-bit, so the
    // test needs no tolerance beyond float noise.
    //
    // On a CLOSED source the parameter is cyclic, so `1` and `0` are the same
    // point and a run may cross that seam. It routinely does: a circle is
    // parametrised from angle 0, which is an ordinary interior point of any
    // stretch the boundary happens to use — without the wrap a plain semicircle
    // would come back as two arcs meeting at a seam nothing in the geometry
    // asked for (exactly the fragile interior seam `merge_cocircular_runs`
    // exists to repair).
    let closed_src = |s: usize| match sources[s] {
        SketchSegment::Arc { sweep_angle, .. } => {
            (sweep_angle.abs() as f64) >= TAU - 1e-6
        }
        _ => false,
    };
    let continues = |k: usize| {
        let prev = steps[(k + n - 1) % n];
        let cur = steps[k];
        if prev.0 != cur.0 {
            return false;
        }
        if (prev.2 - cur.1).abs() <= PARAM_EPS {
            return true;
        }
        closed_src(cur.0) && (prev.2 - cur.1).abs() >= 1.0 - PARAM_EPS
    };
    // Start the walk at a run BREAK so a run is never split across the seam.
    // No break at all means the whole loop is one run (a lone circle).
    let start = (0..n).find(|&k| !continues(k));
    let mut segments: Vec<SketchSegment> = Vec::new();
    let mut seg_src: Vec<Option<u32>> = Vec::new();
    let mut emit_run = |src: usize, first_step: usize, last_step: usize| {
        if matches!(sources[src], SketchSegment::Line { .. }) {
            // Straight: keep one segment per boundary step, with the welded
            // vertices verbatim. Merging collinear steps would drop the
            // one-to-one provenance the trace is built on.
            for k in first_step..=last_step {
                let step = k % n;
                let a = verts[face.verts[step]];
                let b = verts[face.verts[(step + 1) % n]];
                segments.push(SketchSegment::Line {
                    start: [a[0] as f32, a[1] as f32],
                    end: [b[0] as f32, b[1] as f32],
                });
                seg_src.push(src_ids[src]);
            }
        } else {
            // The run's span is ACCUMULATED, not read off its last step: each
            // step contributes its own signed parameter length, so a run that
            // crossed the seam of a closed source comes out as `0.6 → 1.3`
            // rather than folding back to `0.6 → 0.3`.
            let ta = steps[first_step % n].1;
            let tb = (first_step..=last_step)
                .map(|k| {
                    let s = steps[k % n];
                    s.2 - s.1
                })
                .sum::<f64>()
                + ta;
            segments.push(sub_segment(&sources[src], ta, tb));
            seg_src.push(src_ids[src]);
        }
    };
    match start {
        None => emit_run(steps[0].0, 0, n - 1),
        Some(start) => {
            let mut k = 0usize;
            while k < n {
                let first = start + k;
                let mut last = first;
                while last + 1 < start + n && continues((last + 1) % n) {
                    last += 1;
                }
                emit_run(steps[first % n].0, first, last);
                k += last - first + 1;
            }
        }
    }
    // Cut points live on the flattening CHORDS, so a reconstructed arc starts
    // a sagitta away from the neighbour that cut it. Close those junctions
    // analytically (#445) — see [`stitch_junctions`].
    stitch_junctions(&mut segments);
    if segments.is_empty() {
        return (
            Sketch2d::Empty,
            crate::sketch::TracedPrim {
                prim: Sketch2dPrimitive::Path { segments: Vec::new(), closed: true },
                sources: None,
            },
        );
    }
    rotate_to_g1_break(&mut segments, &mut seg_src);
    let prim = Sketch2dPrimitive::Path {
        segments,
        closed: true,
    };
    let trace = crate::sketch::TracedPrim {
        prim: prim.clone(),
        sources: Some(crate::sketch::TraceSources::PerSegment(seg_src)),
    };
    (Sketch2d::Prim(prim), trace)
}

/// How close two parameter values must be to count as the same point on a
/// source curve. Both flattening joints and cut parameters are produced by the
/// same arithmetic on both sides of a junction, so the only slack needed is
/// float noise.
const PARAM_EPS: f64 = 1e-9;

/// Restrict an analytic source segment to the parameter span `[ta … tb]`,
/// where `0 … 1` runs along the source's own direction (#445).
///
/// `tb < ta` is legal and yields the REVERSED piece — the boundary walk is
/// always counter-clockwise, and a curve the walk meets against its own
/// direction has to come back that way, or the emitted path would zig-zag and
/// invent G1 breaks where the geometry is smooth.
fn sub_segment(seg: &SketchSegment, ta: f64, tb: f64) -> SketchSegment {
    // Whole and forward — hand the source back untouched, so the (very
    // common) uncut case stays bit-for-bit what it always was.
    if (ta).abs() <= PARAM_EPS && (tb - 1.0).abs() <= PARAM_EPS {
        return *seg;
    }
    match seg {
        SketchSegment::Line { start, end } => {
            let at = |t: f64| {
                [
                    (start[0] as f64 + t * (end[0] - start[0]) as f64) as f32,
                    (start[1] as f64 + t * (end[1] - start[1]) as f64) as f32,
                ]
            };
            SketchSegment::Line {
                start: at(ta),
                end: at(tb),
            }
        }
        SketchSegment::Arc {
            center,
            radius,
            start_angle,
            sweep_angle,
        } => {
            let s = *start_angle as f64;
            let w = *sweep_angle as f64;
            SketchSegment::Arc {
                center: *center,
                radius: *radius,
                start_angle: (s + w * ta) as f32,
                sweep_angle: (w * (tb - ta)) as f32,
            }
        }
        // Quadratic Bézier restriction via the blossom: the sub-curve on
        // [a … b] has control points f(a,a), f(a,b), f(b,b) with
        // f(u,v) = (1−u)(1−v)·p0 + [(1−u)v + u(1−v)]·p1 + uv·p2. Symmetric in
        // its arguments, so it reverses for free when b < a.
        SketchSegment::QuadBezier { p0, p1, p2 } => {
            let blossom = |u: f64, v: f64| {
                let (w0, w1, w2) = (
                    (1.0 - u) * (1.0 - v),
                    (1.0 - u) * v + u * (1.0 - v),
                    u * v,
                );
                [
                    (w0 * p0[0] as f64 + w1 * p1[0] as f64 + w2 * p2[0] as f64) as f32,
                    (w0 * p0[1] as f64 + w1 * p1[1] as f64 + w2 * p2[1] as f64) as f32,
                ]
            };
            SketchSegment::QuadBezier {
                p0: blossom(ta, ta),
                p1: blossom(ta, tb),
                p2: blossom(tb, tb),
            }
        }
    }
}

/// Close the junctions of a reconstructed loop ANALYTICALLY (#445).
///
/// The arrangement cuts curves on their flattening CHORDS, so a cut point sits
/// up to one sagitta inside the true curve (≈ 5 µm on an r = 4.5 mm circle at
/// 64 chords per turn). Reconstructing the exact arc therefore leaves a gap of
/// that size against the neighbour that cut it — and a gap on a CLOSED path is
/// not cosmetic: `path_is_inside` counts even-odd crossings per segment, so a
/// horizontal band as tall as the gap can be counted twice or not at all
/// (the failure mode #392 was about).
///
/// The remedy is the project's standing rule for coincidence — solve it, do
/// not sample it. Line∩line, line∩circle and circle∩circle all have closed
/// forms; the root nearest the flattened estimate is the meeting point, and
/// BOTH neighbours are moved onto it. A junction that already agrees (every
/// junction of an all-straight or uncut profile) is left untouched, so this
/// changes nothing that was not broken.
fn stitch_junctions(segments: &mut [SketchSegment]) {
    let n = segments.len();
    if n < 2 {
        return;
    }
    for i in 0..n {
        let j = (i + 1) % n;
        let a = f2(segments[i].end_point());
        let b = f2(segments[j].start_point());
        if dist2f(a, b) <= PARAM_EPS * PARAM_EPS {
            continue;
        }
        let hint = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
        let Some(meet) = analytic_meet(&segments[i], &segments[j], hint) else {
            continue;
        };
        set_end_point(&mut segments[i], meet);
        set_start_point(&mut segments[j], meet);
    }
}

/// Where two adjoining boundary segments TRULY meet, from their analytic
/// supports (infinite line / full circle) and the flattened estimate `hint`.
/// `None` = no closed form applies (a Bézier is involved, the supports are
/// parallel or the circles miss each other) — the junction then keeps its
/// flattened point.
fn analytic_meet(
    a: &SketchSegment,
    b: &SketchSegment,
    hint: [f64; 2],
) -> Option<[f64; 2]> {
    use SketchSegment::*;
    match (a, b) {
        (Line { start, end }, Line { start: s2, end: e2 }) => {
            line_line(f2(*start), f2(*end), f2(*s2), f2(*e2))
        }
        (Line { start, end }, Arc { center, radius, .. })
        | (Arc { center, radius, .. }, Line { start, end }) => line_circle(
            f2(*start),
            f2(*end),
            f2(*center),
            *radius as f64,
            hint,
        ),
        (
            Arc { center, radius, .. },
            Arc {
                center: c2,
                radius: r2,
                ..
            },
        ) => circle_circle(f2(*center), *radius as f64, f2(*c2), *r2 as f64, hint),
        // A Bézier's own endpoint IS exact — snap the straight neighbour to it
        // rather than inventing a meeting point.
        (QuadBezier { .. }, _) => Some(f2(a.end_point())),
        (_, QuadBezier { .. }) => Some(f2(b.start_point())),
    }
}

/// Intersection of the two INFINITE lines through `(p→q)` and `(r→s)`.
fn line_line(p: [f64; 2], q: [f64; 2], r: [f64; 2], s: [f64; 2]) -> Option<[f64; 2]> {
    let d1 = [q[0] - p[0], q[1] - p[1]];
    let d2 = [s[0] - r[0], s[1] - r[1]];
    let denom = d1[0] * d2[1] - d1[1] * d2[0];
    let scale = (d1[0].hypot(d1[1])) * (d2[0].hypot(d2[1]));
    if scale < 1e-12 || denom.abs() < 1e-9 * scale {
        return None;
    }
    let t = ((r[0] - p[0]) * d2[1] - (r[1] - p[1]) * d2[0]) / denom;
    Some([p[0] + t * d1[0], p[1] + t * d1[1]])
}

/// The root of the INFINITE line `(p→q)` on the circle `(c, r)` nearest `hint`.
fn line_circle(
    p: [f64; 2],
    q: [f64; 2],
    c: [f64; 2],
    r: f64,
    hint: [f64; 2],
) -> Option<[f64; 2]> {
    let d = [q[0] - p[0], q[1] - p[1]];
    let len2 = d[0] * d[0] + d[1] * d[1];
    if len2 < 1e-18 {
        return None;
    }
    // Foot of the perpendicular from the centre, then ± the half-chord.
    let t_foot = ((c[0] - p[0]) * d[0] + (c[1] - p[1]) * d[1]) / len2;
    let foot = [p[0] + t_foot * d[0], p[1] + t_foot * d[1]];
    let h2 = r * r - dist2f(foot, c);
    if h2 <= 0.0 {
        // Tangent (or a miss within float noise): the foot is the best answer.
        return Some(foot);
    }
    let dt = (h2 / len2).sqrt();
    let cands = [
        [p[0] + (t_foot - dt) * d[0], p[1] + (t_foot - dt) * d[1]],
        [p[0] + (t_foot + dt) * d[0], p[1] + (t_foot + dt) * d[1]],
    ];
    Some(nearest(&cands, hint))
}

/// The intersection of two circles nearest `hint`; `None` when they are
/// concentric / cocircular or do not reach each other.
fn circle_circle(
    c1: [f64; 2],
    r1: f64,
    c2: [f64; 2],
    r2: f64,
    hint: [f64; 2],
) -> Option<[f64; 2]> {
    let d = (c2[0] - c1[0]).hypot(c2[1] - c1[1]);
    if d < 1e-12 {
        return None; // concentric — cocircular runs share their endpoint anyway
    }
    let a = (d * d + r1 * r1 - r2 * r2) / (2.0 * d);
    let h2 = r1 * r1 - a * a;
    if h2 <= 0.0 {
        return None;
    }
    let h = h2.sqrt();
    let ux = (c2[0] - c1[0]) / d;
    let uy = (c2[1] - c1[1]) / d;
    let m = [c1[0] + a * ux, c1[1] + a * uy];
    let cands = [
        [m[0] - h * uy, m[1] + h * ux],
        [m[0] + h * uy, m[1] - h * ux],
    ];
    Some(nearest(&cands, hint))
}

fn nearest(cands: &[[f64; 2]], hint: [f64; 2]) -> [f64; 2] {
    *cands
        .iter()
        .min_by(|a, b| {
            dist2f(**a, hint)
                .partial_cmp(&dist2f(**b, hint))
                .unwrap()
        })
        .unwrap()
}

/// Move a segment's START onto `p`. An arc turns by the (tiny) angle between
/// its old start and `p`, keeping its sweep DIRECTION and its other end fixed.
fn set_start_point(seg: &mut SketchSegment, p: [f64; 2]) {
    match seg {
        SketchSegment::Line { start, .. } => *start = [p[0] as f32, p[1] as f32],
        SketchSegment::QuadBezier { .. } => {}
        SketchSegment::Arc {
            center,
            start_angle,
            sweep_angle,
            ..
        } => {
            let want = (p[1] - center[1] as f64).atan2(p[0] - center[0] as f64);
            let delta = wrap_pi(want - *start_angle as f64);
            *start_angle = (*start_angle as f64 + delta) as f32;
            *sweep_angle = (*sweep_angle as f64 - delta) as f32;
        }
    }
}

/// Move a segment's END onto `p`; the mirror of [`set_start_point`].
fn set_end_point(seg: &mut SketchSegment, p: [f64; 2]) {
    match seg {
        SketchSegment::Line { end, .. } => *end = [p[0] as f32, p[1] as f32],
        SketchSegment::QuadBezier { .. } => {}
        SketchSegment::Arc {
            center,
            start_angle,
            sweep_angle,
            ..
        } => {
            let want = (p[1] - center[1] as f64).atan2(p[0] - center[0] as f64);
            let end = *start_angle as f64 + *sweep_angle as f64;
            *sweep_angle = (*sweep_angle as f64 + wrap_pi(want - end)) as f32;
        }
    }
}

/// Fold an angle difference into `(−π … π]` — a junction only ever moves by a
/// sagitta, so the wrapped value is the intended one.
fn wrap_pi(a: f64) -> f64 {
    let mut a = a % TAU;
    if a > std::f64::consts::PI {
        a -= TAU;
    } else if a <= -std::f64::consts::PI {
        a += TAU;
    }
    a
}

/// Rotate a CLOSED boundary loop so it starts at a G1 BREAK (#424).
///
/// [`fielddraft_protocol::sketch2d::g1_group_segments`] walks a path linearly
/// and never joins the last segment back to the first, so a wall that happens
/// to straddle the loop's seam is cut into TWO boundary groups — two branches,
/// two coplanar faces, and (when both pieces come from the same sketch curve)
/// two groups wanting the same sub-id, which makes
/// [`crate::sketch::wall_sub_table`] refuse the whole table.
///
/// Where the seam falls is an artefact of where the DCEL traversal started, so
/// this is not a cosmetic ordering question: it decides how many walls the
/// solid has. Starting the loop at a real crease makes the decomposition a
/// property of the GEOMETRY alone. A loop that is smooth all the way round (a
/// circle) has no break and stays as it is — one group either way.
fn rotate_to_g1_break(segments: &mut Vec<SketchSegment>, srcs: &mut Vec<Option<u32>>) {
    use fielddraft_protocol::sketch2d::{g1_continuous, PATH_G1_COS_THRESHOLD};
    let n = segments.len();
    if n < 2 {
        return;
    }
    let Some(start) = (0..n).find(|&i| {
        !g1_continuous(&segments[(i + n - 1) % n], &segments[i], PATH_G1_COS_THRESHOLD)
    }) else {
        return;
    };
    if start == 0 {
        return;
    }
    segments.rotate_left(start);
    srcs.rotate_left(start);
}

// ── geometry helpers ─────────────────────────────────────────────────

fn f2(p: [f32; 2]) -> [f64; 2] {
    [p[0] as f64, p[1] as f64]
}
fn dist2(a: [f32; 2], b: [f32; 2]) -> f64 {
    let dx = (a[0] - b[0]) as f64;
    let dy = (a[1] - b[1]) as f64;
    dx * dx + dy * dy
}
fn dist2f(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)
}
fn sort2(a: f32, b: f32) -> (f32, f32) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

fn signed_area_poly(poly: &[[f64; 2]]) -> f64 {
    let mut acc = 0.0;
    for i in 0..poly.len() {
        let p = poly[i];
        let q = poly[(i + 1) % poly.len()];
        acc += p[0] * q[1] - q[0] * p[1];
    }
    acc / 2.0
}

fn signed_area(loop_ids: &[usize], verts: &[[f64; 2]]) -> f64 {
    let mut acc = 0.0;
    for i in 0..loop_ids.len() {
        let p = verts[loop_ids[i]];
        let q = verts[loop_ids[(i + 1) % loop_ids.len()]];
        acc += p[0] * q[1] - q[0] * p[1];
    }
    acc / 2.0
}

fn point_in_poly(p: [f64; 2], poly: &[[f64; 2]]) -> bool {
    let mut inside = false;
    let n = poly.len();
    let mut j = n - 1;
    for i in 0..n {
        let a = poly[i];
        let b = poly[j];
        if (a[1] > p[1]) != (b[1] > p[1]) {
            let x = a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
            if p[0] < x {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

/// A point guaranteed inside a simple polygon (centroid if convex-ish, else a
/// grid scan).
fn interior_of_poly(poly: &[[f64; 2]]) -> [f64; 2] {
    let c = {
        let mut s = [0.0, 0.0];
        for p in poly {
            s[0] += p[0];
            s[1] += p[1];
        }
        [s[0] / poly.len() as f64, s[1] / poly.len() as f64]
    };
    if point_in_poly(c, poly) {
        return c;
    }
    let bb = poly_aabb_f(poly);
    scan_inside(&bb, |p| point_in_poly(p, poly)).unwrap_or(c)
}

/// A point inside `outer` but outside every hole, verified against the exact
/// profile SDF.
fn interior_excluding(
    outer: &[[f64; 2]],
    holes: &[&Vec<[f64; 2]>],
    profile: &Sketch2d,
) -> [f32; 2] {
    let inside = |p: [f64; 2]| {
        point_in_poly(p, outer)
            && holes.iter().all(|h| !point_in_poly(p, h))
            && profile.sdf([p[0] as f32, p[1] as f32]) < 0.0
    };
    let bb = poly_aabb_f(outer);
    if let Some(p) = scan_inside(&bb, inside) {
        return [p[0] as f32, p[1] as f32];
    }
    let c = interior_of_poly(outer);
    [c[0] as f32, c[1] as f32]
}

/// Scan a grid over `bb`; return the first cell centre satisfying `pred`.
fn scan_inside(bb: &Aabb2, pred: impl Fn([f64; 2]) -> bool) -> Option<[f64; 2]> {
    let n = 41usize;
    let w = (bb.max[0] - bb.min[0]) as f64;
    let h = (bb.max[1] - bb.min[1]) as f64;
    for iy in 0..n {
        for ix in 0..n {
            let p = [
                bb.min[0] as f64 + (ix as f64 + 0.5) / n as f64 * w,
                bb.min[1] as f64 + (iy as f64 + 0.5) / n as f64 * h,
            ];
            if pred(p) {
                return Some(p);
            }
        }
    }
    None
}

fn poly_aabb_f(poly: &[[f64; 2]]) -> Aabb2 {
    let mut bb = Aabb2::EMPTY;
    for p in poly {
        bb = Aabb2::union(
            bb,
            Aabb2 {
                min: [p[0] as f32, p[1] as f32],
                max: [p[0] as f32, p[1] as f32],
            },
        );
    }
    bb
}
fn poly_aabb(poly: &[[f64; 2]]) -> Aabb2 {
    poly_aabb_f(poly)
}
/// Length of a CLOSED polyline (the wrap-around edge included).
fn closed_poly_len(poly: &[[f64; 2]]) -> f64 {
    if poly.len() < 2 {
        return 0.0;
    }
    (0..poly.len())
        .map(|i| dist2f(poly[i], poly[(i + 1) % poly.len()]).sqrt())
        .sum()
}
fn poly_centroid(poly: &[[f64; 2]]) -> [f32; 2] {
    let mut s = [0.0f64, 0.0];
    for p in poly {
        s[0] += p[0];
        s[1] += p[1];
    }
    [(s[0] / poly.len() as f64) as f32, (s[1] / poly.len() as f64) as f32]
}
