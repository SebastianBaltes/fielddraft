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

/// One flattened straight micro-piece with its source-segment provenance.
#[derive(Clone)]
struct Piece {
    a: [f64; 2],
    b: [f64; 2],
    src: usize,
}

/// Collect every non-construction entity's exact boundary segments, each
/// tagged with the SKETCH CURVE ENTITY it came from (`None` = the tool has no
/// per-segment curve entity to name, e.g. the point-only polygon/slot/path
/// tools). The ids come from the same registry replay the whole-sketch
/// provenance trace uses ([`crate::sketch::replay_allocations`]), so the two
/// paths can never drift apart.
///
/// LOCKSTEP: for a multi-curve tool the emission order below must match the
/// tool's `apply()` allocation order. Today that is only `sketch_rectangle`
/// (bottom, right, top, left in both) — anything else emits one curve or none.
fn collect_sources(sub_calls: &[FeatureCall]) -> (Vec<SketchSegment>, Vec<Option<u32>>) {
    let allocs = crate::sketch::replay_allocations(sub_calls);
    let mut out: Vec<SketchSegment> = Vec::new();
    let mut src_ids: Vec<Option<u32>> = Vec::new();
    for (ci, call) in sub_calls.iter().enumerate() {
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
        }
    }
    (out, src_ids)
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
}

/// A bounded (positive-area) face of the arrangement.
struct RawFace {
    /// Ordered outer-boundary vertex ids (welded pool ids).
    verts: Vec<usize>,
    /// Per boundary step, the source-segment id of the edge traversed.
    edge_srcs: Vec<usize>,
    area: f64,
}

/// Build the arrangement and return its bounded faces + the vertex pool.
/// Per-source bookkeeping the exact-profile reconstruction needs.
struct SourceUse {
    /// Was this source cut by an intersection / T-junction?
    split: bool,
    /// How many graph edges (post-weld, post-dedup) carry this source — the
    /// number of boundary steps a face must use to have consumed it WHOLE.
    edges: usize,
}

fn build_faces(sources: &[SketchSegment]) -> (Vec<RawFace>, Vec<[f64; 2]>, Vec<SourceUse>) {
    // 1. Flatten every source into straight pieces tagged with the source id.
    let mut pieces: Vec<Piece> = Vec::new();
    let mut src_micro_count = vec![0usize; sources.len()];
    for (si, seg) in sources.iter().enumerate() {
        let poly = flatten(seg);
        src_micro_count[si] = poly.len().saturating_sub(1);
        for w in poly.windows(2) {
            if dist2f(w[0], w[1]) > WELD_TOL * WELD_TOL {
                pieces.push(Piece {
                    a: w[0],
                    b: w[1],
                    src: si,
                });
            }
        }
    }

    // 2. Planarize: split every piece at intersections + interior T-junctions.
    let mut src_was_split = vec![false; sources.len()];
    let mut sub_pieces: Vec<Piece> = Vec::new();
    for i in 0..pieces.len() {
        let pi = &pieces[i];
        let mut cuts: Vec<f64> = Vec::new();
        for (j, pj) in pieces.iter().enumerate() {
            if i == j {
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
        src_was_split[pi.src] = true;
        cuts.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let d = [pi.b[0] - pi.a[0], pi.b[1] - pi.a[1]];
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
            });
        }
    }

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
        edges.push(Edge { u, v, src: sp.src });
    }
    let mut src_edge_count = vec![0usize; sources.len()];
    for e in &edges {
        src_edge_count[e.src] += 1;
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
        let mut face_srcs: Vec<usize> = Vec::new();
        let mut h = start;
        let mut ok = false;
        for _ in 0..=edges.len() * 2 {
            visited[h] = true;
            face_verts.push(tail(h));
            face_srcs.push(edges[h >> 1].src);
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
                edge_srcs: face_srcs,
                area,
            });
        }
    }
    let uses = (0..sources.len())
        .map(|i| SourceUse {
            split: src_was_split[i],
            edges: src_edge_count[i],
        })
        .collect();
    (faces, verts, uses)
}

// ── public entry ─────────────────────────────────────────────────────

/// Compute the bounded atomic imprint faces of a sketch from its raw
/// sub-calls, in a deterministic order (ascending `aabb.min.y`, then
/// `aabb.min.x`, then `area`).
pub fn arrange(sub_calls: &[FeatureCall]) -> Vec<AtomicFace> {
    if has_unarrangeable(sub_calls) {
        return Vec::new();
    }
    let (sources, src_ids) = collect_sources(sub_calls);
    if sources.is_empty() {
        return Vec::new();
    }
    let (faces, verts, src_use) = build_faces(&sources);
    if faces.is_empty() {
        return Vec::new();
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

    // Build each atomic face: outer profile minus immediate child profiles.
    let mut out: Vec<AtomicFace> = Vec::new();
    for i in 0..n {
        let (outer, outer_trace) =
            face_profile(&faces[i], &sources, &src_ids, &verts, &src_use);
        if matches!(outer, Sketch2d::Empty) {
            continue;
        }
        let mut profile = outer;
        // DFS order of `Difference(Difference(outer, h1), h2)` is outer, h1,
        // h2 — the same order [`crate::sketch::wall_sub_table`] walks the tree
        // in, so pushing each hole's trace as it is folded in keeps the two
        // sequences aligned by construction.
        let mut traces = vec![outer_trace];
        let mut hole_polys: Vec<&Vec<[f64; 2]>> = Vec::new();
        for &c in &children[i] {
            let (hole, hole_trace) =
                face_profile(&faces[c], &sources, &src_ids, &verts, &src_use);
            if !matches!(hole, Sketch2d::Empty) {
                profile = Sketch2d::Difference(Box::new(profile), Box::new(hole));
                traces.push(hole_trace);
                hole_polys.push(&polys[c]);
            }
        }
        let child_area: f64 = children[i].iter().map(|&c| faces[c].area).sum();
        let area = (faces[i].area - child_area).max(0.0);
        if area < MIN_AREA {
            continue;
        }
        let perimeter: f64 = std::iter::once(&polys[i])
            .chain(hole_polys.iter().copied())
            .map(|p| closed_poly_len(p))
            .sum();
        let interior = interior_excluding(&polys[i], &hole_polys, &profile);
        let aabb = poly_aabb(&polys[i]);
        let centroid = poly_centroid(&polys[i]);
        out.push(AtomicFace {
            profile,
            interior,
            centroid,
            aabb,
            area: area as f32,
            perimeter: perimeter as f32,
            traces,
        });
    }

    // Deterministic order — geometry-derived, independent of scan order.
    out.sort_by(|a, b| {
        (a.aabb.min[1], a.aabb.min[0], a.area)
            .partial_cmp(&(b.aabb.min[1], b.aabb.min[0], b.area))
            .unwrap()
    });
    out
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

/// Reconstruct one face's outer boundary as an exact closed `Path` (or an
/// equivalent `Union` of sub-paths). Line-sourced edges stay exact; a curved
/// source used fully & un-cut is emitted as its exact `SketchSegment`; a curved
/// source that was cut is emitted as its constituent straight micro-lines.
///
/// Returns the path together with its [`crate::sketch::TracedPrim`] — one
/// stable sketch-entity id per emitted SEGMENT, in the same order, so the
/// picked region's walls carry the same names a whole-sketch profile's do.
fn face_profile(
    face: &RawFace,
    sources: &[SketchSegment],
    src_ids: &[Option<u32>],
    verts: &[[f64; 2]],
    src_use: &[SourceUse],
) -> (Sketch2d, crate::sketch::TracedPrim) {
    // Count how many boundary steps each source contributes.
    let mut counts: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    for &s in &face.edge_srcs {
        *counts.entry(s).or_insert(0) += 1;
    }
    let mut segments: Vec<SketchSegment> = Vec::new();
    let mut seg_src: Vec<Option<u32>> = Vec::new();
    let mut emitted_full: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let n = face.verts.len();
    for step in 0..n {
        let src = face.edge_srcs[step];
        let a = verts[face.verts[step]];
        let b = verts[face.verts[(step + 1) % n]];
        let curved = !matches!(sources[src], SketchSegment::Line { .. });
        // A curved source that was never cut AND whose every surviving graph
        // edge lies on this face's boundary was consumed WHOLE — re-emit it as
        // the exact analytic segment, once (a picked disc stays a true circle;
        // order and direction are irrelevant to a closed Path's even-odd SDF).
        if curved
            && !src_use[src].split
            && src_use[src].edges > 0
            && src_use[src].edges == *counts.get(&src).unwrap_or(&0)
        {
            if emitted_full.insert(src) {
                segments.push(sources[src]);
                seg_src.push(src_ids[src]);
            }
            continue;
        }
        segments.push(SketchSegment::Line {
            start: [a[0] as f32, a[1] as f32],
            end: [b[0] as f32, b[1] as f32],
        });
        seg_src.push(src_ids[src]);
    }
    if segments.is_empty() {
        return (
            Sketch2d::Empty,
            crate::sketch::TracedPrim {
                prim: Sketch2dPrimitive::Path { segments: Vec::new(), closed: true },
                sources: None,
            },
        );
    }
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
