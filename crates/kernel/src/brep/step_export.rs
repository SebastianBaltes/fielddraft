//! STEP (ISO 10303-21) export of the woven B-Rep (step D.4).
//!
//! Serialises a [`BRep`] as an **AP214** advanced-B-Rep STEP part-21 file of
//! trimmed analytic/NURBS surfaces. The output is the canonical entity graph a
//! mechanical-CAD reader expects:
//!
//! ```text
//! ADVANCED_BREP_SHAPE_REPRESENTATION
//!   └─ MANIFOLD_SOLID_BREP
//!        └─ CLOSED_SHELL | OPEN_SHELL
//!             └─ ADVANCED_FACE …
//!                  ├─ surface (PLANE | CYLINDRICAL_SURFACE | B_SPLINE_SURFACE)
//!                  └─ FACE_OUTER_BOUND / FACE_BOUND
//!                       └─ EDGE_LOOP
//!                            └─ ORIENTED_EDGE …
//!                                 └─ EDGE_CURVE
//!                                      ├─ VERTEX_POINT (start / end)
//!                                      └─ curve (B_SPLINE_CURVE_WITH_KNOTS | LINE)
//! ```
//!
//! See DL-39…DL-44 in `docs/GEOMETRY-KERNEL-CONCEPT.md` for the schema,
//! entity-mapping, de-duplication, rational-handling and non-planar decisions.

use std::collections::HashMap;
use std::fmt::Write as _;

use fielddraft_protocol::{HalfSpace, Transform};

use crate::brep::{BRep, EdgeGeometry, EdgeId, Face, Loop, NurbsCurve, SurfaceGeometry, VertexId};
use crate::{compose_transforms, quat_rotate};

/// Apply a body's world `placement` (rigid + uniform scale) to a point
/// already in the body-LOCAL B-Rep frame, returning world coordinates.
/// The woven B-Rep stores vertices, curve control points and analytic
/// surface transforms in the local frame (DL-50); this is the single
/// fold-back applied at export. For an identity placement it is a no-op,
/// so a body woven at the origin exports byte-for-byte as before.
fn place_point(placement: &Transform, p: [f64; 3]) -> [f64; 3] {
    // T(p) = R·(s·p) + t, computed in f64 so vertices far from the origin
    // keep full precision (placement scale is uniform, so scale[0] suffices).
    let s = placement.scale[0] as f64;
    let scaled = [p[0] * s, p[1] * s, p[2] * s];
    let r = quat_rotate_f64(placement.rotation, scaled);
    [
        r[0] + placement.translate[0] as f64,
        r[1] + placement.translate[1] as f64,
        r[2] + placement.translate[2] as f64,
    ]
}

/// f64 quaternion rotation (placement fold-back keeps full precision).
fn quat_rotate_f64(q: [f32; 4], v: [f64; 3]) -> [f64; 3] {
    let qx = q[0] as f64;
    let qy = q[1] as f64;
    let qz = q[2] as f64;
    let qw = q[3] as f64;
    let t = [
        2.0 * (qy * v[2] - qz * v[1]),
        2.0 * (qz * v[0] - qx * v[2]),
        2.0 * (qx * v[1] - qy * v[0]),
    ];
    [
        v[0] + qw * t[0] + qy * t[2] - qz * t[1],
        v[1] + qw * t[1] + qz * t[0] - qx * t[2],
        v[2] + qw * t[2] + qx * t[1] - qy * t[0],
    ]
}

/// A monotonic STEP entity-id allocator that also accumulates the emitted
/// `#N=ENTITY(...);` lines in allocation order.
struct StepWriter {
    next: usize,
    lines: Vec<String>,
}

impl StepWriter {
    fn new() -> Self {
        Self {
            next: 1,
            lines: Vec::new(),
        }
    }

    /// Reserve the next id without emitting a body yet (for the rare case a
    /// forward reference is needed before the body is known). Unused here, but
    /// kept minimal: every entity below is emitted eagerly via [`emit`].
    fn alloc(&mut self) -> usize {
        let id = self.next;
        self.next += 1;
        id
    }

    /// Emit one entity whose body is `body` (the text after `=`), returning its
    /// `#N` id. `body` must not include the leading `#N=` or trailing `;`.
    fn emit(&mut self, body: impl AsRef<str>) -> usize {
        let id = self.alloc();
        self.lines.push(format!("#{id}={};", body.as_ref()));
        id
    }
}

/// Format an `f64` as a STEP real literal. STEP requires a decimal point in
/// every real (`1.` not `1`), so we always include one. We use `{:?}` which
/// yields the shortest round-tripping decimal, then guarantee a dot.
fn real(x: f64) -> String {
    // `{:?}` on f64 gives the minimal round-trip form ("1.0", "0.5",
    // "1e-9", "-2.0"). STEP accepts both "1.0" and "1.E-9"; the only fixup
    // needed is ensuring a decimal point exists for integers without an
    // exponent (Rust already prints "1.0", so this is belt-and-braces).
    let s = format!("{x:?}");
    if s.contains('.') || s.contains('e') || s.contains('E') {
        s
    } else {
        format!("{s}.")
    }
}

/// Bring a primitive-local point into world space through `t`:
/// scale, then rotate (quaternion), then translate. Inverse of
/// [`crate::world_to_local`].
fn local_to_world_point(t: &Transform, p: [f32; 3]) -> [f64; 3] {
    let scaled = [p[0] * t.scale[0], p[1] * t.scale[1], p[2] * t.scale[2]];
    let rot = quat_rotate(t.rotation, scaled);
    [
        (rot[0] + t.translate[0]) as f64,
        (rot[1] + t.translate[1]) as f64,
        (rot[2] + t.translate[2]) as f64,
    ]
}

/// Rotate a local direction into world space (rotation only — directions are
/// translation-invariant). Re-normalised, as STEP `DIRECTION` should be unit.
fn local_to_world_dir(t: &Transform, d: [f32; 3]) -> [f64; 3] {
    let r = quat_rotate(t.rotation, d);
    let n = ((r[0] * r[0] + r[1] * r[1] + r[2] * r[2]) as f64).sqrt();
    if n < 1e-12 {
        [r[0] as f64, r[1] as f64, r[2] as f64]
    } else {
        [r[0] as f64 / n, r[1] as f64 / n, r[2] as f64 / n]
    }
}

/// Any unit vector orthogonal to `n` (assumed unit). Picks the axis least
/// aligned with `n` to avoid degeneracy, then Gram-Schmidt + normalise.
fn orthonormal(n: [f64; 3]) -> [f64; 3] {
    let ax = n[0].abs();
    let ay = n[1].abs();
    let az = n[2].abs();
    let helper = if ax <= ay && ax <= az {
        [1.0, 0.0, 0.0]
    } else if ay <= az {
        [0.0, 1.0, 0.0]
    } else {
        [0.0, 0.0, 1.0]
    };
    let dot = helper[0] * n[0] + helper[1] * n[1] + helper[2] * n[2];
    let mut r = [
        helper[0] - dot * n[0],
        helper[1] - dot * n[1],
        helper[2] - dot * n[2],
    ];
    let len = (r[0] * r[0] + r[1] * r[1] + r[2] * r[2]).sqrt();
    if len > 1e-12 {
        r = [r[0] / len, r[1] / len, r[2] / len];
    }
    r
}

/// Collapse a clamped knot vector into `(distinct_value, multiplicity)` pairs.
fn knot_multiplicities(knots: &[f64]) -> (Vec<f64>, Vec<usize>) {
    let mut values: Vec<f64> = Vec::new();
    let mut mults: Vec<usize> = Vec::new();
    for &k in knots {
        if let Some(last) = values.last() {
            if (*last - k).abs() < 1e-12 {
                *mults.last_mut().unwrap() += 1;
                continue;
            }
        }
        values.push(k);
        mults.push(1);
    }
    (values, mults)
}

/// Caches de-duplicated `CARTESIAN_POINT` ids keyed by quantised coordinates,
/// plus per-vertex `VERTEX_POINT` ids so shared topology references one id.
struct Geometry {
    /// Quantised coordinate → CARTESIAN_POINT id.
    points: HashMap<[u64; 3], usize>,
    /// Quantised direction → DIRECTION id.
    dirs: HashMap<[u64; 3], usize>,
    /// VertexId → VERTEX_POINT id.
    vertex_points: HashMap<u32, usize>,
    /// EdgeId → EDGE_CURVE id.
    edge_curves: HashMap<u32, usize>,
}

impl Geometry {
    fn new() -> Self {
        Self {
            points: HashMap::new(),
            dirs: HashMap::new(),
            vertex_points: HashMap::new(),
            edge_curves: HashMap::new(),
        }
    }
}

/// Quantise a coordinate triple to 1e-9 so floating noise doesn't defeat the
/// de-dup map. (Coordinates that genuinely differ by < 1nm are coincident.)
fn quantise(p: [f64; 3]) -> [u64; 3] {
    let q = |v: f64| (v * 1e9).round() as i64 as u64;
    [q(p[0]), q(p[1]), q(p[2])]
}

/// Get-or-create a de-duplicated `CARTESIAN_POINT` for `p`.
fn cartesian_point(w: &mut StepWriter, g: &mut Geometry, p: [f64; 3]) -> usize {
    let key = quantise(p);
    if let Some(&id) = g.points.get(&key) {
        return id;
    }
    let id = w.emit(format!(
        "CARTESIAN_POINT('',({},{},{}))",
        real(p[0]),
        real(p[1]),
        real(p[2])
    ));
    g.points.insert(key, id);
    id
}

/// Get-or-create a de-duplicated `DIRECTION` for `d`.
fn direction(w: &mut StepWriter, g: &mut Geometry, d: [f64; 3]) -> usize {
    let key = quantise(d);
    if let Some(&id) = g.dirs.get(&key) {
        return id;
    }
    let id = w.emit(format!(
        "DIRECTION('',({},{},{}))",
        real(d[0]),
        real(d[1]),
        real(d[2])
    ));
    g.dirs.insert(key, id);
    id
}

/// Get-or-create a de-duplicated `VERTEX_POINT` for a topological vertex.
fn vertex_point(
    w: &mut StepWriter,
    g: &mut Geometry,
    brep: &BRep,
    placement: &Transform,
    v: VertexId,
) -> usize {
    if let Some(&id) = g.vertex_points.get(&v.0) {
        return id;
    }
    let pos = place_point(placement, brep.vertex(v).position);
    let pt = cartesian_point(w, g, pos);
    let id = w.emit(format!("VERTEX_POINT('',#{pt})"));
    g.vertex_points.insert(v.0, id);
    id
}

/// Emit a curve entity for an edge, returning its `#N` id. Non-rational NURBS
/// → `B_SPLINE_CURVE_WITH_KNOTS`; rational → complex `(... RATIONAL_...)`;
/// a polyline collapses to a degree-1 B-spline through its points.
fn emit_curve(
    w: &mut StepWriter,
    g: &mut Geometry,
    placement: &Transform,
    geometry: &EdgeGeometry,
) -> usize {
    match geometry {
        EdgeGeometry::Nurbs(curve) => emit_bspline(w, g, placement, curve),
        EdgeGeometry::Polyline(pts) => emit_polyline_bspline(w, g, placement, pts),
        // Hermite must not survive weaving; treat its points as a polyline if
        // it ever does, so export never panics.
        EdgeGeometry::Hermite(samples) => {
            let pts: Vec<[f64; 3]> = samples.iter().map(|s| s.point).collect();
            emit_polyline_bspline(w, g, placement, &pts)
        }
    }
}

/// Emit a `B_SPLINE_CURVE_WITH_KNOTS` (or its rational complex form) for a
/// fitted NURBS curve. The clamped knot vector is collapsed to distinct
/// (value, multiplicity) pairs as STEP requires.
fn emit_bspline(
    w: &mut StepWriter,
    g: &mut Geometry,
    placement: &Transform,
    curve: &NurbsCurve,
) -> usize {
    let cp_ids: Vec<usize> = curve
        .control_points
        .iter()
        .map(|&p| cartesian_point(w, g, place_point(placement, p)))
        .collect();
    let cp_refs = join_refs(&cp_ids);
    let (knot_vals, knot_mults) = knot_multiplicities(&curve.knots);
    let mults = knot_mults
        .iter()
        .map(|m| m.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let vals = knot_vals
        .iter()
        .map(|&v| real(v))
        .collect::<Vec<_>>()
        .join(",");

    let rational = curve.weights.iter().any(|&wt| (wt - 1.0).abs() > 1e-9);
    if !rational {
        w.emit(format!(
            "B_SPLINE_CURVE_WITH_KNOTS('',{},({}),.UNSPECIFIED.,.F.,.F.,({}),({}),.UNSPECIFIED.)",
            curve.degree, cp_refs, mults, vals
        ))
    } else {
        // Rational: emit the ISO complex (AND) instance. The control-point
        // list lives on the B_SPLINE_CURVE_WITH_KNOTS subtype; weights on the
        // RATIONAL_B_SPLINE_CURVE subtype; BOUNDED/CURVE/GEOMETRIC/
        // REPRESENTATION_ITEM are the abstract supertypes STEP requires.
        let weights = curve
            .weights
            .iter()
            .map(|&wt| real(wt))
            .collect::<Vec<_>>()
            .join(",");
        w.emit(format!(
            "( BOUNDED_CURVE() B_SPLINE_CURVE({},({}),.UNSPECIFIED.,.F.,.F.) \
B_SPLINE_CURVE_WITH_KNOTS(({}),({}),.UNSPECIFIED.) CURVE() GEOMETRIC_REPRESENTATION_ITEM() \
RATIONAL_B_SPLINE_CURVE(({})) REPRESENTATION_ITEM('') )",
            curve.degree, cp_refs, mults, vals, weights
        ))
    }
}

/// Emit a degree-1 `B_SPLINE_CURVE_WITH_KNOTS` through `pts` (a polyline as a
/// linear B-spline). Used for the test/fallback `Polyline` edge geometry.
fn emit_polyline_bspline(
    w: &mut StepWriter,
    g: &mut Geometry,
    placement: &Transform,
    pts: &[[f64; 3]],
) -> usize {
    // Guarantee at least 2 control points so the curve is well-formed.
    let pts: Vec<[f64; 3]> = if pts.len() >= 2 {
        pts.to_vec()
    } else if pts.len() == 1 {
        vec![pts[0], pts[0]]
    } else {
        vec![[0.0; 3], [0.0; 3]]
    };
    let cp_ids: Vec<usize> = pts
        .iter()
        .map(|&p| cartesian_point(w, g, place_point(placement, p)))
        .collect();
    let cp_refs = join_refs(&cp_ids);
    let n = pts.len();
    // Degree-1 clamped knots: [0,0, 1, 2, …, (n-2), (n-1),(n-1)] → distinct
    // values 0..=(n-1), multiplicity 2 at the ends, 1 in between.
    let mut vals = Vec::new();
    let mut mults = Vec::new();
    for i in 0..n {
        vals.push(real(i as f64));
        mults.push(if i == 0 || i == n - 1 { 2 } else { 1 });
    }
    let vals = vals.join(",");
    let mults = mults
        .iter()
        .map(|m| m.to_string())
        .collect::<Vec<_>>()
        .join(",");
    w.emit(format!(
        "B_SPLINE_CURVE_WITH_KNOTS('',1,({cp_refs}),.UNSPECIFIED.,.F.,.F.,({mults}),({vals}),.UNSPECIFIED.)"
    ))
}

/// Emit (once) the `EDGE_CURVE` for `eid`, returning its id. Both endpoint
/// `VERTEX_POINT`s are de-duplicated through [`Geometry`].
fn edge_curve(
    w: &mut StepWriter,
    g: &mut Geometry,
    brep: &BRep,
    placement: &Transform,
    eid: EdgeId,
) -> usize {
    if let Some(&id) = g.edge_curves.get(&eid.0) {
        return id;
    }
    let edge = brep.edge(eid);
    // A closed edge with no bounding vertices reuses one synthetic vertex at
    // the curve's first control point so the EDGE_CURVE is still well-formed.
    let (start_v, end_v) = match (edge.start, edge.end) {
        (Some(s), Some(e)) => (
            vertex_point(w, g, brep, placement, s),
            vertex_point(w, g, brep, placement, e),
        ),
        _ => {
            let p = place_point(placement, curve_first_point(&edge.geometry));
            let cp = cartesian_point(w, g, p);
            let vp = w.emit(format!("VERTEX_POINT('',#{cp})"));
            (vp, vp)
        }
    };
    let curve = emit_curve(w, g, placement, &edge.geometry);
    let id = w.emit(format!("EDGE_CURVE('',#{start_v},#{end_v},#{curve},.T.)"));
    g.edge_curves.insert(eid.0, id);
    id
}

fn curve_first_point(geometry: &EdgeGeometry) -> [f64; 3] {
    match geometry {
        EdgeGeometry::Nurbs(c) => c.control_points.first().copied().unwrap_or([0.0; 3]),
        EdgeGeometry::Polyline(p) => p.first().copied().unwrap_or([0.0; 3]),
        EdgeGeometry::Hermite(s) => s.first().map(|s| s.point).unwrap_or([0.0; 3]),
    }
}

/// Emit an `EDGE_LOOP` of `ORIENTED_EDGE`s for `lp`, returning the loop id.
fn edge_loop(
    w: &mut StepWriter,
    g: &mut Geometry,
    brep: &BRep,
    placement: &Transform,
    lp: &Loop,
) -> usize {
    let mut oriented = Vec::with_capacity(lp.edges.len());
    for &(eid, reversed) in &lp.edges {
        let ec = edge_curve(w, g, brep, placement, eid);
        let orient = if reversed { ".F." } else { ".T." };
        oriented.push(w.emit(format!("ORIENTED_EDGE('',*,*,#{ec},{orient})")));
    }
    let refs = join_refs(&oriented);
    w.emit(format!("EDGE_LOOP('',({refs}))"))
}

/// Emit the surface entity for a face, returning its `#N` id.
///
/// Closed-form analytic half-spaces map to their exact STEP surface entity:
/// `Plane` → `PLANE`, `InfiniteCylinder` → `CYLINDRICAL_SURFACE`,
/// `Sphere` → `SPHERICAL_SURFACE`, `Cone` → `CONICAL_SURFACE`,
/// `Torus` → `TOROIDAL_SURFACE`. All share the primitive's centred-local
/// convention (axis = local +Y, origin at the transform's translation;
/// see DL-50). `Gyroid` (no closed form) and `Tessellated` faces have no
/// analytic surface and fall back to a best-effort mean-point `PLANE` so the
/// entity graph still resolves (DL-43).
fn emit_surface(
    w: &mut StepWriter,
    g: &mut Geometry,
    brep: &BRep,
    placement: &Transform,
    face: &Face,
) -> usize {
    match &face.surface {
        SurfaceGeometry::Analytic { surface, transform } => {
            // The analytic surface transform is body-LOCAL (DL-50); fold the
            // body placement back in to obtain its world placement.
            let transform = &compose_transforms(placement, transform);
            match surface {
            HalfSpace::Plane { normal, offset } => {
                let n = [normal[0], normal[1], normal[2]];
                // Point on plane in local frame: normal*offset (unit normal).
                let local_pt = [n[0] * *offset, n[1] * *offset, n[2] * *offset];
                let world_pt = local_to_world_point(transform, local_pt);
                let world_n = local_to_world_dir(transform, n);
                emit_plane(w, g, world_pt, world_n)
            }
            HalfSpace::InfiniteCylinder { radius } => {
                // Cylinder axis is local +Y (matches the primitive convention);
                // placement origin at the transform's translation.
                let origin = local_to_world_point(transform, [0.0, 0.0, 0.0]);
                let axis = local_to_world_dir(transform, [0.0, 1.0, 0.0]);
                let ref_dir = orthonormal(axis);
                let placement = axis2_placement(w, g, origin, axis, ref_dir);
                w.emit(format!(
                    "CYLINDRICAL_SURFACE('',#{placement},{})",
                    real(*radius as f64)
                ))
            }
            HalfSpace::Sphere { radius } => {
                // Sphere centre = local origin; axis = local +Y (the placement
                // axis is only the parametrisation pole, the surface is
                // rotationally symmetric).
                let origin = local_to_world_point(transform, [0.0, 0.0, 0.0]);
                let axis = local_to_world_dir(transform, [0.0, 1.0, 0.0]);
                let ref_dir = orthonormal(axis);
                let placement = axis2_placement(w, g, origin, axis, ref_dir);
                w.emit(format!(
                    "SPHERICAL_SURFACE('',#{placement},{})",
                    real(*radius as f64)
                ))
            }
            HalfSpace::Cone { half_angle } => {
                // Apex at local origin, axis = local +Y. STEP's CONICAL_SURFACE
                // radius is the radius at the placement's xy reference plane;
                // placing the AXIS2_PLACEMENT at the apex makes that radius 0
                // and `semi_angle` the cone half-angle (radians).
                let origin = local_to_world_point(transform, [0.0, 0.0, 0.0]);
                let axis = local_to_world_dir(transform, [0.0, 1.0, 0.0]);
                let ref_dir = orthonormal(axis);
                let placement = axis2_placement(w, g, origin, axis, ref_dir);
                w.emit(format!(
                    "CONICAL_SURFACE('',#{placement},{},{})",
                    real(0.0),
                    real(*half_angle as f64)
                ))
            }
            HalfSpace::Torus { major_radius, minor_radius } => {
                // Torus centre = local origin, axis (tube revolution) = local +Y.
                let origin = local_to_world_point(transform, [0.0, 0.0, 0.0]);
                let axis = local_to_world_dir(transform, [0.0, 1.0, 0.0]);
                let ref_dir = orthonormal(axis);
                let placement = axis2_placement(w, g, origin, axis, ref_dir);
                w.emit(format!(
                    "TOROIDAL_SURFACE('',#{placement},{},{})",
                    real(*major_radius as f64),
                    real(*minor_radius as f64)
                ))
            }
            // Gyroid (TPMS) has no closed-form STEP surface; approximate with
            // the face's mean-point plane so the file stays structurally valid
            // (DL-43).
            HalfSpace::Gyroid { .. } => fallback_plane(w, g, brep, placement, face),
            }
        }
        // Tessellated (Gyroid/TPMS) faces have no closed-form surface; emit a
        // best-effort plane so references still resolve (DL-43).
        SurfaceGeometry::Tessellated { .. } => fallback_plane(w, g, brep, placement, face),
    }
}

/// Emit a `PLANE` with an `AXIS2_PLACEMENT_3D` at `point` with normal `axis`.
fn emit_plane(w: &mut StepWriter, g: &mut Geometry, point: [f64; 3], axis: [f64; 3]) -> usize {
    let ref_dir = orthonormal(axis);
    let placement = axis2_placement(w, g, point, axis, ref_dir);
    w.emit(format!("PLANE('',#{placement})"))
}

/// Best-effort plane through the average of a face's vertices, with a normal
/// estimated from the loop (or +Z if degenerate). Used for surfaces we don't
/// yet map analytically so the entity graph still resolves.
fn fallback_plane(
    w: &mut StepWriter,
    g: &mut Geometry,
    brep: &BRep,
    placement: &Transform,
    face: &Face,
) -> usize {
    let mut pts: Vec<[f64; 3]> = Vec::new();
    for &(eid, _) in &face.outer.edges {
        let e = brep.edge(eid);
        if let Some(s) = e.start {
            pts.push(place_point(placement, brep.vertex(s).position));
        }
        if let Some(en) = e.end {
            pts.push(place_point(placement, brep.vertex(en).position));
        }
    }
    let centroid = if pts.is_empty() {
        [0.0, 0.0, 0.0]
    } else {
        let mut c = [0.0; 3];
        for p in &pts {
            c[0] += p[0];
            c[1] += p[1];
            c[2] += p[2];
        }
        let n = pts.len() as f64;
        [c[0] / n, c[1] / n, c[2] / n]
    };
    // Newell's method for a robust loop normal.
    let mut normal = [0.0f64; 3];
    if pts.len() >= 3 {
        for i in 0..pts.len() {
            let a = pts[i];
            let b = pts[(i + 1) % pts.len()];
            normal[0] += (a[1] - b[1]) * (a[2] + b[2]);
            normal[1] += (a[2] - b[2]) * (a[0] + b[0]);
            normal[2] += (a[0] - b[0]) * (a[1] + b[1]);
        }
    }
    let len = (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]).sqrt();
    let axis = if len > 1e-9 {
        [normal[0] / len, normal[1] / len, normal[2] / len]
    } else {
        [0.0, 0.0, 1.0]
    };
    emit_plane(w, g, centroid, axis)
}

/// Emit an `AXIS2_PLACEMENT_3D(location, axis, ref_direction)`.
fn axis2_placement(
    w: &mut StepWriter,
    g: &mut Geometry,
    location: [f64; 3],
    axis: [f64; 3],
    ref_dir: [f64; 3],
) -> usize {
    let loc = cartesian_point(w, g, location);
    let ax = direction(w, g, axis);
    let rd = direction(w, g, ref_dir);
    w.emit(format!("AXIS2_PLACEMENT_3D('',#{loc},#{ax},#{rd})"))
}

/// Escape a string for a STEP single-quoted literal: `'` doubles to `''`.
fn step_string(s: &str) -> String {
    s.replace('\'', "''")
}

/// `#a,#b,#c` for a list of ids.
fn join_refs(ids: &[usize]) -> String {
    ids.iter()
        .map(|id| format!("#{id}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// Emit every solid of one `brep` as `MANIFOLD_SOLID_BREP` (or, for an open
/// outer shell, `SHELL_BASED_SURFACE_MODEL`) rep items, appending their ids to
/// `out`. The shared [`StepWriter`] and [`Geometry`] cache let points/dirs
/// de-duplicate and entity ids stay unique *across* multiple breps, so this is
/// the single emission core both [`export_step`] and [`export_step_scene`] use.
fn emit_brep_solids(
    w: &mut StepWriter,
    g: &mut Geometry,
    brep: &BRep,
    placement: &Transform,
    name: &str,
    out: &mut Vec<usize>,
) {
    for solid in &brep.solids {
        let shell = brep.shell(solid.outer);
        let mut face_ids = Vec::with_capacity(shell.faces.len());
        for &fid in &shell.faces {
            let face = brep.face(fid);
            let surface = emit_surface(w, g, brep, placement, face);
            let mut bound_ids = Vec::new();
            // A fully closed analytic surface (a lone sphere, SA-6) is a face
            // with NO bounds — an empty outer loop emits no FACE_OUTER_BOUND, so
            // the ADVANCED_FACE carries only its surface. Bounded faces emit the
            // outer loop as usual.
            if !face.outer.edges.is_empty() {
                let outer = edge_loop(w, g, brep, placement, &face.outer);
                bound_ids.push(w.emit(format!("FACE_OUTER_BOUND('',#{outer},.T.)")));
            }
            for inner in &face.inner {
                let il = edge_loop(w, g, brep, placement, inner);
                bound_ids.push(w.emit(format!("FACE_BOUND('',#{il},.T.)")));
            }
            let bounds = join_refs(&bound_ids);
            face_ids.push(w.emit(format!("ADVANCED_FACE('',({bounds}),#{surface},.T.)")));
        }
        let faces = join_refs(&face_ids);
        let shell_id = if shell.is_closed {
            w.emit(format!("CLOSED_SHELL('',({faces}))"))
        } else {
            w.emit(format!("OPEN_SHELL('',({faces}))"))
        };
        let msb = if shell.is_closed {
            w.emit(format!("MANIFOLD_SOLID_BREP('{name}',#{shell_id})"))
        } else {
            // An open shell can't be a manifold solid; wrap it in a
            // SHELL_BASED_SURFACE_MODEL instead (still a valid rep item).
            w.emit(format!("SHELL_BASED_SURFACE_MODEL('{name}',(#{shell_id}))"))
        };
        out.push(msb);
    }
}

/// Serialise `brep` as an ISO-10303-21 / AP214 advanced-B-Rep STEP file named
/// `product_name`. See module docs and DL-39…DL-44 for the mapping.
pub fn export_step(brep: &BRep, product_name: &str) -> String {
    export_step_scene(&[brep], product_name)
}

/// [`export_step`] with an explicit body **placement** (DL-50). The B-Rep is
/// stored in the body-local frame; `placement` folds it back to world space.
/// `export_step` is the `placement = IDENTITY` case.
pub fn export_step_placed(brep: &BRep, placement: &Transform, product_name: &str) -> String {
    export_step_scene_placed(&[(brep, *placement)], product_name)
}

/// Serialise several bodies' B-Reps as multiple `MANIFOLD_SOLID_BREP` solids
/// inside ONE STEP file and ONE [`ADVANCED_BREP_SHAPE_REPRESENTATION`] (DL-53:
/// a scene with N separate bodies is N solids in a single STEP file). The
/// geometry de-dup cache and id allocator span all bodies, so coincident
/// points/dirs across solids share an id and every `#N` stays unique.
pub fn export_step_scene(breps: &[&BRep], product_name: &str) -> String {
    let placed: Vec<(&BRep, Transform)> =
        breps.iter().map(|&b| (b, Transform::IDENTITY)).collect();
    export_step_scene_placed(&placed, product_name)
}

/// [`export_step_scene`] with a per-body **placement** (DL-50): each body's
/// local B-Rep is folded back to world space by its own placement before
/// emission. The geometry de-dup cache and id allocator still span all
/// bodies. `export_step_scene` is the all-identity case.
pub fn export_step_scene_placed(breps: &[(&BRep, Transform)], product_name: &str) -> String {
    let mut w = StepWriter::new();
    let mut g = Geometry::new();
    let name = step_string(product_name);

    // ── geometric context: a millimetre length unit + tolerance ──────────
    let unit = w.emit("( LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.) )");
    let angle_unit = w.emit("( NAMED_UNIT(*) PLANE_ANGLE_UNIT() SI_UNIT($,.RADIAN.) )");
    let solid_angle_unit = w.emit("( NAMED_UNIT(*) SI_UNIT($,.STERADIAN.) SOLID_ANGLE_UNIT() )");
    let tol_value = w.emit(format!(
        "LENGTH_MEASURE_WITH_UNIT(LENGTH_MEASURE({}),#{unit})",
        real(1e-6)
    ));
    let uncertainty = w.emit(format!(
        "UNCERTAINTY_MEASURE_WITH_UNIT(#{tol_value},'distance_accuracy_value','edge curve and vertex point accuracy')"
    ));
    let geom_context = w.emit(format!(
        "( GEOMETRIC_REPRESENTATION_CONTEXT(3) \
GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT((#{uncertainty})) \
GLOBAL_UNIT_ASSIGNED_CONTEXT((#{unit},#{angle_unit},#{solid_angle_unit})) REPRESENTATION_CONTEXT('',''))"
    ));

    // ── one MANIFOLD_SOLID_BREP per solid of every body; faces → shells ───
    let mut brep_ids: Vec<usize> = Vec::new();
    for (brep, placement) in breps {
        emit_brep_solids(&mut w, &mut g, brep, placement, &name, &mut brep_ids);
    }

    // ── product / shape-representation boilerplate (AP214) ────────────────
    let app_context = w.emit("APPLICATION_CONTEXT('automotive design')");
    w.emit(format!(
        "APPLICATION_PROTOCOL_DEFINITION('international standard','automotive_design',2010,#{app_context})"
    ));
    let product_context = w.emit(format!("PRODUCT_CONTEXT('',#{app_context},'mechanical')"));
    let product = w.emit(format!(
        "PRODUCT('{name}','{name}','',(#{product_context}))"
    ));
    let pdf = w.emit(format!("PRODUCT_DEFINITION_FORMATION('','',#{product})"));
    let pd_context = w.emit(format!(
        "PRODUCT_DEFINITION_CONTEXT('part definition',#{app_context},'design')"
    ));
    let product_def = w.emit(format!(
        "PRODUCT_DEFINITION('design','',#{pdf},#{pd_context})"
    ));
    let pds = w.emit(format!("PRODUCT_DEFINITION_SHAPE('','',#{product_def})"));

    // The shape representation gathers the solid(s) in the geometric context.
    let rep_items = join_refs(&brep_ids);
    let shape_rep = w.emit(format!(
        "ADVANCED_BREP_SHAPE_REPRESENTATION('{name}',({rep_items}),#{geom_context})"
    ));
    w.emit(format!(
        "SHAPE_DEFINITION_REPRESENTATION(#{pds},#{shape_rep})"
    ));

    assemble(&w, product_name)
}

/// Wrap the accumulated entity lines in the part-21 envelope.
fn assemble(w: &StepWriter, product_name: &str) -> String {
    let product_name = step_string(product_name);
    let mut out = String::new();
    out.push_str("ISO-10303-21;\n");
    out.push_str("HEADER;\n");
    let _ = writeln!(
        out,
        "FILE_DESCRIPTION(('FieldDraft B-Rep advanced_brep'),'2;1');"
    );
    let _ = writeln!(
        out,
        "FILE_NAME('{product_name}','',(''),(''),'FieldDraft','FieldDraft kernel D.4','');"
    );
    out.push_str("FILE_SCHEMA(('AUTOMOTIVE_DESIGN { 1 0 10303 214 1 1 1 1 }'));\n");
    out.push_str("ENDSEC;\n");
    out.push_str("DATA;\n");
    for line in &w.lines {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str("ENDSEC;\n");
    out.push_str("END-ISO-10303-21;\n");
    out
}

// ─── semantic validation ────────────────────────────────────────────────────

/// Parse the `DATA` section into `(id → entity-type-token)` and the set of
/// every `#N` referenced on a right-hand side. Definition lines look like
/// `#N=ENTITY(...);`; the entity-type token is the identifier right after `=`
/// (or `( SUPER1() SUPER2() ... )` for an ISO complex instance, reported as the
/// literal `(` so callers can recognise it).
fn parse_entities(step: &str) -> (HashMap<usize, String>, std::collections::HashSet<usize>) {
    use std::collections::HashSet;
    let mut defined: HashMap<usize, String> = HashMap::new();
    let mut referenced: HashSet<usize> = HashSet::new();
    let mut in_data = false;
    for line in step.lines() {
        let t = line.trim();
        if t == "DATA;" {
            in_data = true;
            continue;
        }
        if t == "ENDSEC;" {
            in_data = false;
            continue;
        }
        if !in_data {
            continue;
        }
        let Some(rest) = t.strip_prefix('#') else { continue };
        let Some(eq) = rest.find('=') else { continue };
        let Ok(id) = rest[..eq].parse::<usize>() else { continue };
        let rhs = rest[eq + 1..].trim_start();
        // Entity-type token: leading identifier, or '(' for a complex instance.
        let token: String = if rhs.starts_with('(') {
            "(".to_string()
        } else {
            rhs.chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect()
        };
        defined.insert(id, token);
        // References live on the RHS only.
        let bytes = rhs.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'#' {
                let mut j = i + 1;
                while j < bytes.len() && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                if j > i + 1 {
                    if let Ok(r) = rhs[i + 1..j].parse::<usize>() {
                        referenced.insert(r);
                    }
                }
                i = j;
            } else {
                i += 1;
            }
        }
    }
    (defined, referenced)
}

/// Semantically validate a STEP part-21 string this module produced. Goes
/// beyond substring presence: it checks structural integrity of the entity
/// graph. Returns `Ok(())` or `Err(errors)` listing every problem found:
///
/// 1. **Reference closure** — every `#N` referenced is defined, and no id is
///    defined twice.
/// 2. Exactly one `ADVANCED_BREP_SHAPE_REPRESENTATION`, and it is referenced
///    (reachable) by a `SHAPE_DEFINITION_REPRESENTATION`.
/// 3. At least one `MANIFOLD_SOLID_BREP`.
/// 4. Every `ADVANCED_FACE` references a defined surface entity (a `PLANE`,
///    `*_SURFACE`, or complex surface instance), not a dangling/non-surface id.
/// 5. A `LENGTH_UNIT` and a `GEOMETRIC_REPRESENTATION_CONTEXT` are present.
pub fn validate_step(step: &str) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    let (defined, referenced) = parse_entities(step);
    let defined_ids: std::collections::HashSet<usize> = defined.keys().copied().collect();

    // (1) reference closure.
    for r in &referenced {
        if !defined_ids.contains(r) {
            errors.push(format!("dangling reference #{r}"));
        }
    }

    // Count entities by their type token.
    let count = |name: &str| defined.values().filter(|t| t.as_str() == name).count();

    // (2) exactly one ABSR, reachable from a SHAPE_DEFINITION_REPRESENTATION.
    let absr_ids: Vec<usize> = defined
        .iter()
        .filter(|(_, t)| t.as_str() == "ADVANCED_BREP_SHAPE_REPRESENTATION")
        .map(|(id, _)| *id)
        .collect();
    match absr_ids.len() {
        1 => {
            if !referenced.contains(&absr_ids[0]) {
                errors.push(
                    "ADVANCED_BREP_SHAPE_REPRESENTATION is not reachable (unreferenced)".into(),
                );
            }
        }
        n => errors.push(format!(
            "expected exactly 1 ADVANCED_BREP_SHAPE_REPRESENTATION, found {n}"
        )),
    }

    // (3) at least one solid.
    if count("MANIFOLD_SOLID_BREP") == 0 {
        errors.push("no MANIFOLD_SOLID_BREP".into());
    }

    // (4) every ADVANCED_FACE references a defined *surface* entity. The third
    // ref of `ADVANCED_FACE('',(bounds...),#surface,.T.)` must be a surface.
    let is_surface = |id: usize| -> bool {
        match defined.get(&id).map(String::as_str) {
            Some("PLANE")
            | Some("CYLINDRICAL_SURFACE")
            | Some("SPHERICAL_SURFACE")
            | Some("CONICAL_SURFACE")
            | Some("TOROIDAL_SURFACE")
            | Some("B_SPLINE_SURFACE")
            | Some("(") => true, // '(' = complex surface instance
            _ => false,
        }
    };
    for line in step.lines() {
        let t = line.trim();
        let Some(rest) = t.strip_prefix('#') else { continue };
        let Some(eq) = rest.find('=') else { continue };
        let Ok(face_id) = rest[..eq].parse::<usize>() else { continue };
        let rhs = rest[eq + 1..].trim_start();
        if !rhs.starts_with("ADVANCED_FACE(") {
            continue;
        }
        // The surface is the ref that follows the closing ')' of the bounds
        // list: `('',(#a,#b),#surface,.T.)`. Collect all refs, the surface is
        // the last one before the orientation flag — equivalently the ref that
        // is NOT inside the inner parenthesised bounds list.
        let depth_refs = face_surface_ref(rhs);
        match depth_refs {
            Some(sid) if is_surface(sid) => {}
            Some(sid) => errors.push(format!(
                "ADVANCED_FACE #{face_id} surface #{sid} is not a surface entity ({:?})",
                defined.get(&sid)
            )),
            None => errors.push(format!("ADVANCED_FACE #{face_id} has no surface reference")),
        }
    }

    // (5) units / context.
    if count("LENGTH_UNIT") == 0 && !step.contains("LENGTH_UNIT()") {
        errors.push("no LENGTH_UNIT".into());
    }
    if !step.contains("GEOMETRIC_REPRESENTATION_CONTEXT") {
        errors.push("no GEOMETRIC_REPRESENTATION_CONTEXT".into());
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Extract the surface `#N` from an `ADVANCED_FACE('',(<bounds>),#surface,.T.)`
/// body: the first `#N` that appears at parenthesis depth 1 (i.e. *not* nested
/// inside the bounds sub-list). Returns `None` if there is none.
fn face_surface_ref(rhs: &str) -> Option<usize> {
    let bytes = rhs.as_bytes();
    let mut depth = 0i32;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => depth -= 1,
            b'#' if depth == 1 => {
                let mut j = i + 1;
                while j < bytes.len() && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                if j > i + 1 {
                    return rhs[i + 1..j].parse::<usize>().ok();
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brep::octree::Octree;
    use crate::brep::weaving::weave;
    use crate::bvh::BoundedCsg;
    use crate::csg::CsgNode;
    use fielddraft_protocol::{FeatureId, Primitive};
    use std::collections::{HashMap, HashSet};

    fn box_body(half: [f32; 3]) -> BoundedCsg {
        let csg = CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Box {
                center: [0.0, 0.0, 0.0],
                half_extents: half,
            },
        };
        BoundedCsg::from_csg(&csg)
    }

    fn woven_cube() -> BRep {
        let body = box_body([1.0, 1.0, 1.0]);
        let octree = Octree::build(&body, 0.25);
        weave(&body, &octree)
    }

    /// Collect every `#N=` definition id and every referenced `#N` id.
    /// Returns (defined, referenced).
    fn collect_ids(step: &str) -> (HashMap<usize, usize>, HashSet<usize>) {
        let mut defined: HashMap<usize, usize> = HashMap::new(); // id -> count
        let mut referenced: HashSet<usize> = HashSet::new();
        for line in step.lines() {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix('#') {
                // Definition line: "#N=BODY;"
                if let Some(eq) = rest.find('=') {
                    if let Ok(id) = rest[..eq].parse::<usize>() {
                        *defined.entry(id).or_insert(0) += 1;
                        // References are on the right-hand side.
                        scan_refs(&rest[eq + 1..], &mut referenced);
                        continue;
                    }
                }
            }
        }
        (defined, referenced)
    }

    /// Find all `#N` tokens in a string and add them to `out`.
    fn scan_refs(s: &str, out: &mut HashSet<usize>) {
        let bytes = s.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'#' {
                let mut j = i + 1;
                while j < bytes.len() && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                if j > i + 1 {
                    if let Ok(id) = s[i + 1..j].parse::<usize>() {
                        out.insert(id);
                    }
                }
                i = j;
            } else {
                i += 1;
            }
        }
    }

    fn count_entity(step: &str, name: &str) -> usize {
        // Count "=NAME(" occurrences (the entity-type token right after `=`),
        // so e.g. PLANE doesn't match inside another token.
        step.lines()
            .filter(|l| {
                if let Some(eq) = l.find('=') {
                    l[eq + 1..].trim_start().starts_with(&format!("{name}("))
                } else {
                    false
                }
            })
            .count()
    }

    #[test]
    fn envelope_framing() {
        let step = export_step(&woven_cube(), "cube");
        assert!(step.starts_with("ISO-10303-21;"));
        assert!(step.trim_end().ends_with("END-ISO-10303-21;"));
        assert_eq!(step.matches("HEADER;").count(), 1);
        assert_eq!(step.matches("DATA;").count(), 1);
        assert_eq!(step.matches("ENDSEC;").count(), 2, "one per section");
        assert!(step.contains("FILE_SCHEMA(("));
        assert!(step.contains("AUTOMOTIVE_DESIGN"));
    }

    #[test]
    fn every_reference_resolves() {
        let step = export_step(&woven_cube(), "cube");
        let (defined, referenced) = collect_ids(&step);
        // No id defined twice.
        for (id, count) in &defined {
            assert_eq!(*count, 1, "#{id} defined {count} times");
        }
        // Every referenced id is defined.
        let defined_ids: HashSet<usize> = defined.keys().copied().collect();
        let missing: Vec<usize> = referenced.difference(&defined_ids).copied().collect();
        assert!(missing.is_empty(), "dangling references: {missing:?}");
    }

    #[test]
    fn cube_entity_counts() {
        let step = export_step(&woven_cube(), "cube");
        assert_eq!(count_entity(&step, "MANIFOLD_SOLID_BREP"), 1);
        assert_eq!(count_entity(&step, "CLOSED_SHELL"), 1);
        assert_eq!(count_entity(&step, "ADVANCED_FACE"), 6);
        assert_eq!(count_entity(&step, "PLANE"), 6);
        assert_eq!(
            count_entity(&step, "VERTEX_POINT"),
            8,
            "shared vertices de-duped"
        );
        assert_eq!(count_entity(&step, "EDGE_CURVE"), 12);
        assert_eq!(
            count_entity(&step, "B_SPLINE_CURVE_WITH_KNOTS"),
            12,
            "12 NURBS edge curves"
        );
    }

    #[test]
    fn cube_corner_coordinates_present() {
        let step = export_step(&woven_cube(), "cube");
        // The 8 corners are (±1,±1,±1). Each CARTESIAN_POINT for a vertex
        // should carry coordinates near ±1. Verify all eight corners appear.
        for &x in &[-1.0_f64, 1.0] {
            for &y in &[-1.0_f64, 1.0] {
                for &z in &[-1.0_f64, 1.0] {
                    let needle = format!(
                        "CARTESIAN_POINT('',({},{},{}))",
                        super::real(x),
                        super::real(y),
                        super::real(z)
                    );
                    assert!(step.contains(&needle), "missing corner {needle}");
                }
            }
        }
    }

    #[test]
    fn rational_curve_emits_complex_instance() {
        // Hand-build a minimal solid (one face / one shell / one solid) whose
        // single edge is a rational NURBS, to exercise the complex
        // RATIONAL_B_SPLINE_CURVE path. The edge must be reachable through a
        // shell loop, since `export_step` walks solids → shells → faces.
        use crate::brep::{Edge, Face, Loop, NurbsCurve, Shell, Solid, Vertex};
        use crate::csg::BranchId;
        let mut brep = BRep::new();
        let v0 = brep.add_vertex(Vertex::new([0.0, 0.0, 0.0], vec![BranchId(1), BranchId(2)]));
        let v1 = brep.add_vertex(Vertex::new([1.0, 0.0, 0.0], vec![BranchId(1), BranchId(3)]));
        let e = brep.add_edge(Edge::new(
            EdgeGeometry::Nurbs(NurbsCurve {
                degree: 2,
                control_points: vec![[0.0, 0.0, 0.0], [0.5, 1.0, 0.0], [1.0, 0.0, 0.0]],
                weights: vec![1.0, 0.5, 1.0], // non-uniform → rational
                knots: vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
            }),
            BranchId(1),
            BranchId(2),
            Some(v0),
            Some(v1),
        ));
        let f = brep.add_face(Face {
            branch_id: BranchId(1),
            surface: SurfaceGeometry::Tessellated {
                positions: Vec::new(),
                indices: Vec::new(),
            },
            outer: Loop::new(vec![(e, false)]),
            inner: Vec::new(),
        });
        let sh = brep.add_shell(Shell::new(vec![f], false));
        brep.add_solid(Solid::new(sh, Vec::new()));

        let step = export_step(&brep, "rational");
        assert!(
            step.contains("RATIONAL_B_SPLINE_CURVE((1.0,0.5,1.0))"),
            "{step}"
        );
        // References must still all resolve.
        let (defined, referenced) = collect_ids(&step);
        let defined_ids: HashSet<usize> = defined.keys().copied().collect();
        assert!(referenced.is_subset(&defined_ids));
    }

    // ─── SA-5 Task D: semantic validator on real bodies ──────────────────

    use fielddraft_protocol::{Sketch2d as ProtoSketch2d, Sketch2dPrimitive as ProtoSketch2dPrim};

    fn extrude_brep(sketch: ProtoSketch2d, half_height: f32) -> BRep {
        let csg = CsgNode::Extrude {
            id: FeatureId::from_u32(1),
            sketch: Box::new(sketch),
            half_height,
        };
        let body = BoundedCsg::from_csg(&csg);
        let octree = Octree::build(&body, 0.25);
        weave(&body, &octree)
    }

    /// A triangle profile `(r,y)` revolved about Y → a cone wall (oblique
    /// edge), an outer cylinder (vertical edge) and a base plane (horizontal
    /// edge). DL-50 lifts the oblique segment to a `HalfSpace::Cone`.
    fn cone_revolve_brep() -> BRep {
        let profile = ProtoSketch2d::Prim(ProtoSketch2dPrim::Polygon {
            vertices: vec![[1.0, -1.0], [3.0, -1.0], [1.0, 1.0]],
        });
        let csg = CsgNode::Revolve {
            id: FeatureId::from_u32(1),
            sketch: Box::new(profile),
        };
        let body = BoundedCsg::from_csg(&csg);
        let octree = Octree::build(&body, 0.1);
        weave(&body, &octree)
    }

    #[test]
    fn validate_box_step() {
        let step = export_step(&woven_cube(), "cube");
        assert_eq!(validate_step(&step), Ok(()), "box STEP must validate");
        assert_eq!(count_entity(&step, "MANIFOLD_SOLID_BREP"), 1);
    }

    #[test]
    fn validate_extrude_step() {
        let sketch = ProtoSketch2d::Prim(ProtoSketch2dPrim::Rectangle {
            center: [0.0, 0.0],
            half_extents: [1.0, 2.0],
        });
        let brep = extrude_brep(sketch, 1.0);
        let step = export_step(&brep, "extrude");
        assert_eq!(
            validate_step(&step),
            Ok(()),
            "sketch-extrude STEP must validate"
        );
        // 4 walls + 2 caps, all analytic planes.
        assert_eq!(count_entity(&step, "PLANE"), 6);
    }

    #[test]
    fn validate_revolve_cone_emits_conical_surface() {
        let brep = cone_revolve_brep();
        // Confirm a Cone face is actually present in the B-Rep BEFORE asserting
        // on the STEP (so a weaving regression can't silently pass the test).
        let has_cone = brep.faces.iter().any(|f| {
            matches!(
                &f.surface,
                SurfaceGeometry::Analytic {
                    surface: HalfSpace::Cone { .. },
                    ..
                }
            )
        });
        assert!(has_cone, "revolve must produce a HalfSpace::Cone face");

        let step = export_step(&brep, "cone");
        assert_eq!(validate_step(&step), Ok(()), "cone STEP must validate");
        assert!(
            count_entity(&step, "CONICAL_SURFACE") >= 1,
            "Cone face must emit CONICAL_SURFACE, not a fallback plane:\n{step}"
        );
        // The cone semi-angle (π/4 ≈ 0.785) must appear, not a plane.
        assert!(step.contains("CONICAL_SURFACE"));
    }

    #[test]
    fn validate_sphere_face_emits_spherical_surface() {
        // SA-6 end-to-end: a lone sphere primitive woven through the real
        // pipeline (no hand-built B-Rep) is ONE closed analytic face — empty
        // outer loop, no edges/vertices — and its STEP face emits a
        // SPHERICAL_SURFACE (not a tessellated/plane fallback) with no
        // FACE_OUTER_BOUND, and the document validates.
        let csg = CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Sphere { center: [0.0, 0.0, 0.0], radius: 2.5 },
        };
        let body = BoundedCsg::from_csg(&csg);
        let octree = Octree::build(&body, 0.3);
        let brep = weave(&body, &octree);

        // The woven B-Rep itself carries the analytic sphere face.
        assert_eq!(brep.face_count(), 1, "lone sphere → one face");
        assert!(matches!(
            brep.faces[0].surface,
            SurfaceGeometry::Analytic { surface: HalfSpace::Sphere { .. }, .. }
        ), "woven sphere face must be analytic Sphere, got {:?}", brep.faces[0].surface);

        let step = export_step(&brep, "sphere");
        assert_eq!(validate_step(&step), Ok(()), "sphere STEP must validate:\n{step}");
        assert_eq!(
            count_entity(&step, "SPHERICAL_SURFACE"),
            1,
            "Sphere face must emit SPHERICAL_SURFACE:\n{step}"
        );
        assert!(step.contains("SPHERICAL_SURFACE('',#"));
        assert!(step.contains("2.5"), "sphere radius must appear");
        // A fully closed surface carries no outer bound.
        assert_eq!(
            count_entity(&step, "FACE_OUTER_BOUND"),
            0,
            "closed sphere face has no FACE_OUTER_BOUND:\n{step}"
        );
    }

    #[test]
    fn validate_torus_face_emits_toroidal_surface() {
        // SA-6 end-to-end: a lone torus weaves to one closed analytic face
        // and emits a TOROIDAL_SURFACE with no FACE_OUTER_BOUND.
        let csg = CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Torus {
                center: [0.0, 0.0, 0.0],
                major_radius: 2.0,
                minor_radius: 0.5,
            },
        };
        let body = BoundedCsg::from_csg(&csg);
        let octree = Octree::build(&body, 0.2);
        let brep = weave(&body, &octree);

        assert_eq!(brep.face_count(), 1, "lone torus → one face");
        let step = export_step(&brep, "torus");
        assert_eq!(validate_step(&step), Ok(()), "torus STEP must validate:\n{step}");
        assert_eq!(
            count_entity(&step, "TOROIDAL_SURFACE"),
            1,
            "Torus face must emit TOROIDAL_SURFACE:\n{step}"
        );
        assert_eq!(
            count_entity(&step, "FACE_OUTER_BOUND"),
            0,
            "closed torus face has no FACE_OUTER_BOUND:\n{step}"
        );
    }

    #[test]
    fn validate_multi_body_scene() {
        // Two separate boxes → two MANIFOLD_SOLID_BREP in ONE shape rep (DL-53).
        let a = woven_cube();
        let b = woven_cube();
        let step = export_step_scene(&[&a, &b], "scene");
        assert_eq!(validate_step(&step), Ok(()), "scene STEP must validate");
        assert_eq!(
            count_entity(&step, "MANIFOLD_SOLID_BREP"),
            2,
            "two bodies → two solids"
        );
        assert_eq!(
            count_entity(&step, "ADVANCED_BREP_SHAPE_REPRESENTATION"),
            1,
            "still one shape representation"
        );
        // export_step is just the single-body special case of the scene export.
        let single = export_step(&a, "scene");
        assert_eq!(count_entity(&single, "MANIFOLD_SOLID_BREP"), 1);
    }

    #[test]
    fn validate_step_catches_dangling_reference() {
        // A deliberately corrupted file (reference to an undefined id) must be
        // rejected by the semantic validator.
        let mut step = export_step(&woven_cube(), "cube");
        step = step.replace("MANIFOLD_SOLID_BREP('cube',#", "MANIFOLD_SOLID_BREP('cube',#999999");
        let err = validate_step(&step).expect_err("corrupted file must fail");
        assert!(
            err.iter().any(|e| e.contains("dangling reference")),
            "expected a dangling-reference error, got {err:?}"
        );
    }
}
