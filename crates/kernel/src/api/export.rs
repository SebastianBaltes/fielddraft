//! Export-API: STL (ASCII/binär), 3MF, STEP-Klassifikation und
//! STEP-Export.

use wasm_bindgen::prelude::*;

use crate::Kernel;
use crate::export;
use crate::export_3mf;
use crate::step_classifier;
use crate::step_commands;
#[wasm_bindgen]
impl Kernel {
    // ── Meshing Phase 0: tolerance-driven export meshing ─────────
    //
    // The mesh export no longer ships the viewport mesh: a session
    // builds every body at its own cell-size-derived resolution and
    // validates the result (watertight + measured SDF deviation).
    // Protocol: `exportMeshBegin(cellMm)` → N× `exportMeshBody(i)`
    // (the caller loops and reports progress) → `exportMeshStl()` /
    // `exportMesh3mf()` + `exportMeshReport()` → `exportMeshEnd()`.
    // Any document mutation between calls drops the session.

    /// Start an export meshing pass at `cell_size_mm`. Returns the
    /// number of bodies the caller must feed through
    /// [`Kernel::export_mesh_body`]. `options_json` carries the
    /// method-specific knobs beyond the cell size (camelCase, see
    /// [`crate::export_mesh::ExportMethodOptions`]); `""` = defaults.
    #[wasm_bindgen(js_name = exportMeshBegin)]
    pub fn export_mesh_begin(
        &mut self,
        cell_size_mm: f32,
        method: String,
        options_json: String,
    ) -> usize {
        let method = crate::export_mesh::MeshMethod::from_wire(&method);
        let options = crate::export_mesh::ExportMethodOptions::from_json(&options_json);
        self.export_mesh_begin_internal(cell_size_mm, method, options)
    }

    /// Debug: sample the scene SDF on a `resolution`×`resolution` grid over a
    /// plane centred at `(ox,oy,oz)` with `(nx,ny,nz)` normal, spanning
    /// ±`half_extent` in each in-plane axis. Row-major (v outer, u inner). The
    /// scene SDF is the min over all bodies of each body's exact mesher SDF
    /// (`EvalMode::Visual`) — the same field the meshers see — so the SDF-plane
    /// overlay shows exactly what is being contoured. Empty scene → all `MAX`.
    #[wasm_bindgen(js_name = sdfPlaneSample)]
    #[allow(clippy::too_many_arguments)]
    pub fn sdf_plane_sample(
        &self,
        ox: f32,
        oy: f32,
        oz: f32,
        nx: f32,
        ny: f32,
        nz: f32,
        half_extent: f32,
        resolution: u32,
    ) -> Vec<f32> {
        let res = (resolution.max(2)) as usize;
        // Orthonormal in-plane frame (u, v) from the normal.
        let mut n = [nx, ny, nz];
        let nl = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if nl < 1e-9 {
            n = [0.0, 0.0, 1.0];
        } else {
            n = [n[0] / nl, n[1] / nl, n[2] / nl];
        }
        let up = if n[1].abs() < 0.9 { [0.0, 1.0, 0.0] } else { [1.0, 0.0, 0.0] };
        let cross = |a: [f32; 3], b: [f32; 3]| {
            [
                a[1] * b[2] - a[2] * b[1],
                a[2] * b[0] - a[0] * b[2],
                a[0] * b[1] - a[1] * b[0],
            ]
        };
        let norm = |a: [f32; 3]| {
            let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt().max(1e-9);
            [a[0] / l, a[1] / l, a[2] / l]
        };
        let u = norm(cross(up, n));
        let v = cross(n, u);

        // Build each body's bounded CSG once; the scene field is their union.
        let bodies: Vec<crate::bvh::BoundedCsg> = self
            .bodies
            .iter()
            .map(|b| {
                crate::body::build_bounded_with_primitives(&self.decoded, &b.feature_indices).0
            })
            .collect();

        let step = if res > 1 { (2.0 * half_extent) / (res as f32 - 1.0) } else { 0.0 };
        let mut out = Vec::with_capacity(res * res);
        for j in 0..res {
            let tv = -half_extent + j as f32 * step;
            for i in 0..res {
                let tu = -half_extent + i as f32 * step;
                let p = [
                    ox + u[0] * tu + v[0] * tv,
                    oy + u[1] * tu + v[1] * tv,
                    oz + u[2] * tu + v[2] * tv,
                ];
                let mut d = f32::MAX;
                for b in &bodies {
                    d = d.min(b.sdf(p, crate::csg::EvalMode::Visual));
                }
                out.push(d);
            }
        }
        out
    }

    /// Debug ▸ Mesher: build the narrow-band adaptive octree over the scene
    /// (one point per leaf cell = its centre) and run the OPTIONAL stages of
    /// the debug meshing pipeline, all controlled by `options_json`
    /// ([`crate::mesher_points::MesherPointOptions`], camelCase; unknown
    /// fields ignored, malformed JSON → defaults):
    ///
    ///  1. jq2t point optimisers (processing filters, Newton "Fit on
    ///     Surface", tangential "Repulsion inside Surface", cell clamping),
    ///  2. `meshPoints`: adaptive dual contouring over the optimised points
    ///     (one vertex per leaf; quads at sign-changing minimal octree
    ///     edges), after an optional 2:1 balance pass (`balanceOctree`).
    ///
    /// Returns `[nPoints, nTris, nFallbackTris, points…, indices…]`: per
    /// leaf 9 floats `[x, y, z, sdf, nx, ny, nz, cellSize, shown]` with the
    /// FINAL point position (sdf/normal re-evaluated there) and a `shown`
    /// display flag (1 = passed the near-surface/sign-change filters), then
    /// 3 indices per triangle into the point list; the LAST `nFallbackTris`
    /// triangles are the §6-F3 fallback patches/zippers (chartless faces).
    /// Points failing the processing filters stay rigid and `shown = 0`;
    /// "only on surface" escapees are dropped from the point list entirely.
    #[wasm_bindgen(js_name = mesherPoints)]
    pub fn mesher_points(&mut self, options_json: String) -> Vec<f32> {
        let opts: crate::mesher_points::MesherPointOptions =
            serde_json::from_str(&options_json).unwrap_or_default();
        self.mesher_points_impl(opts, None)
    }

    /// Core of the PASM debug/export pipeline. `body_filter = None` meshes the
    /// WHOLE scene (the Debug ▸ Mesher overlay); `Some(i)` restricts every
    /// stage — octree extent, surface SDF, skeleton, particle attribution,
    /// per-face CDT — to body `i` alone, so the export path can produce one
    /// watertight mesh per body. The local `bodies` vec stays 1:1 with
    /// `self.bodies`, so branch attribution and face lookup share the same
    /// index space; `active` just selects which of them participate.
    pub(crate) fn mesher_points_impl(
        &mut self,
        mut opts: crate::mesher_points::MesherPointOptions,
        body_filter: Option<usize>,
    ) -> Vec<f32> {
        let bodies: Vec<crate::bvh::BoundedCsg> = self
            .bodies
            .iter()
            .map(|b| {
                crate::body::build_bounded_with_primitives(&self.decoded, &b.feature_indices).0
            })
            .collect();
        let active: Vec<usize> = match body_filter {
            Some(i) if i < bodies.len() => vec![i],
            Some(_) => return Vec::new(),
            None => (0..bodies.len()).collect(),
        };
        let Some((center, root_size)) = octree_root_of(&bodies, &active) else {
            return Vec::new();
        };
        // Mesher-PASM: ONE resolution parameter. `targetEdgeLength` (h, mm)
        // derives the seed octree depth (leaf cell size just below h → seed
        // spacing ≈ h in the surface band) and the interaction radius (= 5σ
        // truncation of the Gaussian kernel, σ = h/5).
        if opts.target_edge_length > 0.0 {
            let h = opts.target_edge_length;
            let depth = (root_size / h).log2().ceil() as i32;
            opts.max_depth = depth.clamp(1, 10) as u32;
            opts.interaction_radius = h;
        }
        let scene_sdf = |p: [f32; 3]| {
            let mut d = f32::MAX;
            for &bi in &active {
                d = d.min(bodies[bi].sdf(p, crate::csg::EvalMode::Visual));
            }
            d
        };
        let mut root = crate::mesher_points::build_octree(
            center,
            root_size,
            opts.max_depth,
            opts.band_guard,
            &scene_sdf,
        );
        // Balance also for the fallback patches: their DC connectivity spans
        // octree depth transitions exactly like the debug DC mesh.
        if (opts.mesh_points || (opts.cdt_faces && opts.fallback_patches)) && opts.balance_octree
        {
            crate::mesher_points::balance_2to1(&mut root, &scene_sdf);
        }
        let min_size_est = root_size / (1u32 << opts.max_depth.min(10)) as f32;
        let eps = (min_size_est * 0.05).clamp(1e-3, 1.0);
        let grad = |p: [f32; 3]| -> [f32; 3] {
            let gx = scene_sdf([p[0] + eps, p[1], p[2]]) - scene_sdf([p[0] - eps, p[1], p[2]]);
            let gy = scene_sdf([p[0], p[1] + eps, p[2]]) - scene_sdf([p[0], p[1] - eps, p[2]]);
            let gz = scene_sdf([p[0], p[1], p[2] + eps]) - scene_sdf([p[0], p[1], p[2] - eps]);
            let l = (gx * gx + gy * gy + gz * gz).sqrt();
            if l > 1e-12 {
                [gx / l, gy / l, gz / l]
            } else {
                [0.0, 0.0, 1.0]
            }
        };
        let leaves = crate::mesher_points::collect_leaves(&mut root, &grad);
        if leaves.is_empty() {
            return vec![0.0, 0.0, 0.0];
        }

        // ── B-Rep skeleton (Hybrid-PASM P1, ticket nwmi) ──
        // The woven edges become FIXED particle chains: corners exactly at the
        // B-Rep vertices (flag 3), edge interiors resampled by arc length at
        // the interaction radius (flag 2). They repel the free particles but
        // never move — sharp features are boundary conditions, not detection
        // targets. Positions fold the body placement (mirror body_brep_edges).
        let mut skeleton: Vec<[f32; 3]> = Vec::new();
        let mut skeleton_corner: Vec<bool> = Vec::new();
        // Per (body, vertex idx) → skeleton slot; per (body, edge idx) → the
        // FULL chain of skeleton slots (endpoints = corner slots for open
        // edges) — the loop assembly of the CDT stage below re-uses exactly
        // these slots, which is what makes seams watertight by construction.
        let mut corner_slot: std::collections::HashMap<(usize, usize), usize> =
            std::collections::HashMap::new();
        let mut edge_chain: std::collections::HashMap<(usize, usize), Vec<usize>> =
            std::collections::HashMap::new();
        if opts.brep_skeleton {
            let h = if opts.interaction_radius > 0.0 {
                opts.interaction_radius
            } else {
                min_size_est
            };
            for &bi in &active {
                self.bodies[bi].ensure_brep(&self.decoded, true);
                let placement = self.bodies[bi].placement;
                let Some(brep) = self.bodies[bi].brep_cached() else { continue };
                let s = placement.scale[0];
                let to_world = |p: [f64; 3]| -> [f32; 3] {
                    let scaled = [p[0] as f32 * s, p[1] as f32 * s, p[2] as f32 * s];
                    let r = crate::quat_rotate(placement.rotation, scaled);
                    [
                        r[0] + placement.translate[0],
                        r[1] + placement.translate[1],
                        r[2] + placement.translate[2],
                    ]
                };
                for (vi, v) in brep.vertices.iter().enumerate() {
                    corner_slot.insert((bi, vi), skeleton.len());
                    skeleton.push(to_world(v.position));
                    skeleton_corner.push(true);
                }
                for (ei, e) in brep.edges.iter().enumerate() {
                    let pts: Vec<[f32; 3]> = crate::brep::edge_sampling::sample_edge(&e.geometry)
                        .into_iter()
                        .map(to_world)
                        .collect();
                    let chain = crate::mesher_points::resample_polyline(&pts, h);
                    let mut slots: Vec<usize> = Vec::with_capacity(chain.len());
                    if let Some(vs) = e.start {
                        slots.push(corner_slot[&(bi, vs.0 as usize)]);
                    }
                    // Interior points only: chain endpoints coincide with the
                    // corner vertices (open edges) or wrap onto the chain
                    // itself (closed rings — drop the duplicated seam point).
                    let start = if e.start.is_some() { 1 } else { 0 };
                    let mut end = chain.len().saturating_sub(usize::from(e.end.is_some()));
                    if e.start.is_none() && e.end.is_none() && chain.len() >= 2 {
                        // Closed ring: resample keeps first == last; keep one.
                        end = end.saturating_sub(1);
                    }
                    for p in &chain[start.min(chain.len())..end] {
                        slots.push(skeleton.len());
                        skeleton.push(*p);
                        skeleton_corner.push(false);
                    }
                    if let Some(ve) = e.end {
                        slots.push(corner_slot[&(bi, ve.0 as usize)]);
                    }
                    edge_chain.insert((bi, ei), slots);
                }
            }
        }

        let (points, processed, alive) =
            crate::mesher_points::optimize_leaf_points(&leaves, &opts, &scene_sdf, &grad, &skeleton);
        // The mesh connects exactly the SHOWN points (passed the display
        // filters = processed AND survived only-on-surface): one vertex per
        // shown leaf, triangles by octree-edge adjacency (no sign-change).
        let shown: Vec<bool> = (0..leaves.len()).map(|i| processed[i] && alive[i]).collect();
        let indices = if opts.mesh_points && !opts.cdt_faces {
            crate::mesher_points::connect_octree_edges(&root, &shown)
        } else {
            Vec::new()
        };

        // "Only on surface" drops points from the SET: dead leaves are
        // omitted from the output and the mesh indices remapped onto the
        // surviving points.
        let mut remap = vec![u32::MAX; leaves.len()];
        let mut n_alive = 0u32;
        for (i, &a) in alive.iter().enumerate() {
            if a {
                remap[i] = n_alive;
                n_alive += 1;
            }
        }
        // Per point 9 floats: [x, y, z, sdf, nx, ny, nz, size, shown]. `shown`
        // (1/0) = passed the DISPLAY filters (near-surface AND sign-change) —
        // the overlay draws normals/dots only for shown points, so toggling a
        // processing filter visibly culls the cloud (tickets jt76/gjuu). The
        // MESH still uses every emitted point as a vertex (crack-free DC), so
        // `shown` is display-only; only "only-on-surface" escapees are gone
        // from the vertex set entirely (dropped above via `alive`).
        // ── CDT faces (Hybrid-PASM P3) ──
        // Per PLANAR face: assemble its boundary loops from the shared
        // skeleton chains (same slots on both adjacent faces → watertight
        // seams), attribute the free shown particles by branch, and run the
        // constrained Delaunay triangulation in the face plane. Triangles are
        // emitted in OUTPUT indices (alive-remapped particles first, then the
        // skeleton block), so they bypass the octree remap below.
        let mut cdt_tris: Vec<[u32; 3]> = Vec::new();
        // §6-F3 fallback triangles (DC patches + rim zippers of chartless
        // faces) — emitted LAST so the overlay can colour them distinctly.
        let mut fallback_tris: Vec<[u32; 3]> = Vec::new();
        // Seam Steiner vertices of periodic (unrolled) faces — appended to
        // the output point table after the skeleton block.
        let mut aux_points: Vec<[f32; 3]> = Vec::new();
        if opts.cdt_faces && opts.brep_skeleton {
            use crate::brep::SurfaceGeometry;
            use fielddraft_protocol::HalfSpace;
            // Output-space point table (mirror of the emission order below).
            let mut flat: Vec<[f32; 3]> = Vec::with_capacity(n_alive as usize + skeleton.len());
            for (i, _) in leaves.iter().enumerate() {
                if alive[i] {
                    flat.push(points[i]);
                }
            }
            flat.extend(skeleton.iter().copied());
            let skel_base = n_alive;

            // Branch attribution of the free particles (shown only): the
            // winning body's contributor set at the point names its face.
            let eps_attr = (min_size_est * 0.1).max(1e-3);
            let mut by_branch: std::collections::HashMap<(usize, u32), Vec<u32>> =
                std::collections::HashMap::new();
            // Same attribution in LEAF index space — the fallback patches
            // need the octree cells behind the particles (connectivity).
            let mut by_branch_leaf: std::collections::HashMap<(usize, u32), Vec<usize>> =
                std::collections::HashMap::new();
            for (i, _) in leaves.iter().enumerate() {
                if !(alive[i] && processed[i]) {
                    continue;
                }
                let p = points[i];
                let mut best = (f32::MAX, usize::MAX);
                for &bi in &active {
                    let d = bodies[bi].sdf(p, crate::csg::EvalMode::Logical).abs();
                    if d < best.0 {
                        best = (d, bi);
                    }
                }
                if best.1 == usize::MAX {
                    continue;
                }
                let (_, contributors) =
                    bodies[best.1].evaluate(p, eps_attr, crate::csg::EvalMode::Logical);
                for br in contributors.as_slice() {
                    by_branch
                        .entry((best.1, br.as_u32()))
                        .or_default()
                        .push(remap[i]);
                    by_branch_leaf.entry((best.1, br.as_u32())).or_default().push(i);
                }
            }

            // Seam sampling spacing (world mm) — same law as the skeleton
            // chains: the interaction radius, octree cell size as fallback.
            let h_seam = if opts.interaction_radius > 0.0 {
                opts.interaction_radius
            } else {
                min_size_est
            };
            // Faces WITHOUT a usable chart, deferred to the §6-F3 fallback
            // pass below: (body, branch, rim loops as output-index rings).
            let mut fallback_faces: Vec<(usize, u32, Vec<Vec<u32>>)> = Vec::new();
            for &bi in &active {
                let placement = self.bodies[bi].placement;
                let Some(brep) = self.bodies[bi].brep_cached() else { continue };
                for face in &brep.faces {
                    // Assemble closed loops (outer first) as OUTPUT indices —
                    // BEFORE the carrier dispatch: chartless faces need their
                    // rims for the fallback patch too.
                    let mut face_loops: Vec<Vec<u32>> = Vec::new();
                    let mut ok = true;
                    for lp in std::iter::once(&face.outer).chain(face.inner.iter()) {
                        let mut ring: Vec<u32> = Vec::new();
                        for &(eid, reversed) in &lp.edges {
                            let Some(chain) = edge_chain.get(&(bi, eid.0 as usize)) else {
                                ok = false;
                                break;
                            };
                            let seq: Vec<usize> = if reversed {
                                chain.iter().rev().copied().collect()
                            } else {
                                chain.clone()
                            };
                            for (k, slot) in seq.iter().enumerate() {
                                let out_idx = skel_base + *slot as u32;
                                if k == 0 && ring.last() == Some(&out_idx) {
                                    continue; // junction shared with the previous chain
                                }
                                ring.push(out_idx);
                            }
                        }
                        if !ok {
                            break;
                        }
                        if ring.first() == ring.last() && ring.len() > 1 {
                            ring.pop(); // the wrap point — the loop closes implicitly
                        }
                        if ring.len() >= 3 {
                            face_loops.push(ring);
                        }
                    }
                    if !ok || face_loops.is_empty() {
                        continue;
                    }
                    // Carrier dispatch: only these analytic surfaces carry a
                    // chart for the per-face CDT. Everything else — Torus /
                    // Gyroid halfspaces, tessellated gyroid sheets, future
                    // custom-SDF carriers — goes to the DC-patch fallback.
                    let chartable = match &face.surface {
                        SurfaceGeometry::Analytic { surface, transform }
                            if matches!(
                                surface,
                                HalfSpace::Plane { .. }
                                    | HalfSpace::InfiniteCylinder { .. }
                                    | HalfSpace::Cone { .. }
                                    | HalfSpace::Sphere { .. }
                                    | HalfSpace::RevolvedProfile { .. }
                            ) =>
                        {
                            Some((surface, transform))
                        }
                        _ => None,
                    };
                    let Some((surface, transform)) = chartable else {
                        if opts.fallback_patches {
                            fallback_faces.push((bi, face.branch_id.as_u32(), face_loops));
                        }
                        continue;
                    };
                    let interior = by_branch
                        .get(&(bi, face.branch_id.as_u32()))
                        .map(Vec::as_slice)
                        .unwrap_or(&[]);
                    if matches!(surface, HalfSpace::Plane { .. }) {
                        cdt_tris.extend(crate::mesher_cdt::triangulate_planar_face(
                            &flat,
                            &face_loops,
                            interior,
                            0.25 * h_seam as f64,
                        ));
                        continue;
                    }

                    // ── P4: developable/periodic carriers ──
                    // Chart failures (fold guard, pole-only loops, profile
                    // pieces missing) drop out of this labelled block with
                    // `meshed = false` and take the fallback path as well.
                    let meshed = 'chart: {
                    // Unroll the face into its (θ·s, v·s) chart (u periodic)
                    // and let the periodic CDT cut + weld the seam. The chart
                    // lives in SURFACE-LOCAL coordinates: world → body local
                    // (inverse placement) → surface local (inverse transform),
                    // folded into one composed transform.
                    let total_t = crate::compose_transforms(&placement, transform);
                    // P4b: a RevolvedProfile's chart v-axis is the ARC LENGTH
                    // along the offset-corrected profile chain. Sample the
                    // chain (ssi's pieces, continuous order) into a local
                    // (r, y) polyline with cumulative arc lengths; v of a
                    // point is the arc length of its nearest-point
                    // projection. Points ON the surface project uniquely, so
                    // the offset-fold ambiguity of far points never applies.
                    let profile: Option<(Vec<[f32; 2]>, Vec<f32>)> =
                        if matches!(surface, HalfSpace::RevolvedProfile { .. }) {
                            let mut lo = [f32::INFINITY; 3];
                            let mut hi = [f32::NEG_INFINITY; 3];
                            for ring in &face_loops {
                                for &gi in ring {
                                    let p = flat[gi as usize];
                                    for a in 0..3 {
                                        lo[a] = lo[a].min(p[a]);
                                        hi[a] = hi[a].max(p[a]);
                                    }
                                }
                            }
                            let bb = crate::csg::Aabb { min: lo, max: hi };
                            let Some(pieces) =
                                crate::brep::ssi::profile_pieces(surface, &total_t, &bb)
                            else {
                                break 'chart false;
                            };
                            let mut poly: Vec<[f32; 2]> = Vec::new();
                            for pc in &pieces {
                                let n = pc.samples.clamp(8, 128);
                                for k in 0..=n {
                                    let q = (pc.eval)(k as f32 / n as f32);
                                    if poly.last().map_or(true, |l| {
                                        let d = [q[0] - l[0], q[1] - l[1]];
                                        d[0] * d[0] + d[1] * d[1] > 1e-10
                                    }) {
                                        poly.push(q);
                                    }
                                }
                            }
                            if poly.len() < 2 {
                                break 'chart false;
                            }
                            let mut arc = Vec::with_capacity(poly.len());
                            let mut acc = 0.0f32;
                            arc.push(0.0);
                            for w in poly.windows(2) {
                                let d = [w[1][0] - w[0][0], w[1][1] - w[0][1]];
                                acc += (d[0] * d[0] + d[1] * d[1]).sqrt();
                                arc.push(acc);
                            }
                            Some((poly, arc))
                        } else {
                            None
                        };
                    let uv_raw = |p: [f32; 3]| -> Option<[f64; 2]> {
                        let l = crate::world_to_local(&total_t, p);
                        match surface {
                            HalfSpace::InfiniteCylinder { .. } | HalfSpace::Cone { .. } => {
                                Some([(l[2] as f64).atan2(l[0] as f64), l[1] as f64])
                            }
                            HalfSpace::Sphere { .. } => {
                                let (x, y, z) = (l[0] as f64, l[1] as f64, l[2] as f64);
                                let r = (x * x + y * y + z * z).sqrt();
                                if r < 1e-9 {
                                    None // a pole point has no azimuth
                                } else {
                                    Some([z.atan2(x), (y / r).clamp(-1.0, 1.0).acos()])
                                }
                            }
                            HalfSpace::RevolvedProfile { .. } => {
                                let (poly, arc) = profile.as_ref()?;
                                let rad = (l[0] * l[0] + l[2] * l[2]).sqrt();
                                let mut best = (f32::INFINITY, 0.0f32);
                                for (k, w) in poly.windows(2).enumerate() {
                                    let d = [w[1][0] - w[0][0], w[1][1] - w[0][1]];
                                    let len2 = d[0] * d[0] + d[1] * d[1];
                                    let t = if len2 > 0.0 {
                                        (((rad - w[0][0]) * d[0] + (l[1] - w[0][1]) * d[1])
                                            / len2)
                                            .clamp(0.0, 1.0)
                                    } else {
                                        0.0
                                    };
                                    let e = [
                                        rad - (w[0][0] + t * d[0]),
                                        l[1] - (w[0][1] + t * d[1]),
                                    ];
                                    let d2 = e[0] * e[0] + e[1] * e[1];
                                    if d2 < best.0 {
                                        best = (d2, arc[k] + t * len2.sqrt());
                                    }
                                }
                                Some([(l[2] as f64).atan2(l[0] as f64), best.1 as f64])
                            }
                            _ => None,
                        }
                    };
                    let mut raw_loops: Vec<Vec<(u32, [f64; 2])>> = Vec::new();
                    let mut chart_ok = true;
                    for ring in &face_loops {
                        let mut lp = Vec::with_capacity(ring.len());
                        for &gi in ring {
                            match uv_raw(flat[gi as usize]) {
                                Some(uv) => lp.push((gi, uv)),
                                None => {
                                    chart_ok = false;
                                    break;
                                }
                            }
                        }
                        if !chart_ok {
                            break;
                        }
                        raw_loops.push(lp);
                    }
                    if !chart_ok {
                        break 'chart false;
                    }
                    // Projection-fold guard (RevolvedProfile): if v jumps by
                    // more than a quarter of the chain length between
                    // neighbouring loop points, the nearest-point chart
                    // folded (closed/ambiguous profile) — leave unmeshed.
                    if let Some((_, arc)) = &profile {
                        let l_total = *arc.last().unwrap_or(&0.0) as f64;
                        let folded = raw_loops.iter().any(|lp| {
                            (0..lp.len()).any(|k| {
                                (lp[(k + 1) % lp.len()].1[1] - lp[k].1[1]).abs()
                                    > 0.25 * l_total
                            })
                        });
                        if folded {
                            break 'chart false;
                        }
                    }
                    // Scale θ (and the sphere's polar angle) to arc length so
                    // both chart axes carry comparable units — a Delaunay
                    // quality concern, not a correctness one. The cone uses
                    // the mean loop radius as its reference.
                    let (su, sv) = match surface {
                        HalfSpace::InfiniteCylinder { radius } => (*radius as f64, 1.0),
                        HalfSpace::Sphere { radius } => (*radius as f64, *radius as f64),
                        HalfSpace::Cone { half_angle } => {
                            let tan_a = (*half_angle as f64).tan().abs().max(1e-6);
                            let n_pts: usize = raw_loops.iter().map(Vec::len).sum();
                            let mean_v = raw_loops
                                .iter()
                                .flatten()
                                .map(|&(_, [_, v])| v.abs())
                                .sum::<f64>()
                                / n_pts.max(1) as f64;
                            ((mean_v * tan_a).max(1e-3), 1.0)
                        }
                        HalfSpace::RevolvedProfile { .. } => {
                            // Mean loop radius as the θ arc-length reference
                            // (v is already an arc length).
                            let (mut sum, mut n) = (0.0f64, 0usize);
                            for ring in &face_loops {
                                for &gi in ring {
                                    let l =
                                        crate::world_to_local(&total_t, flat[gi as usize]);
                                    sum += ((l[0] * l[0] + l[2] * l[2]) as f64).sqrt();
                                    n += 1;
                                }
                            }
                            ((sum / n.max(1) as f64).max(1e-3), 1.0)
                        }
                        _ => (1.0, 1.0),
                    };
                    let period = std::f64::consts::TAU * su;
                    let scaled: Vec<Vec<(u32, [f64; 2])>> = raw_loops
                        .iter()
                        .map(|lp| {
                            lp.iter().map(|&(gi, [u, v])| (gi, [u * su, v * sv])).collect()
                        })
                        .collect();
                    let scaled_interior: Vec<(u32, [f64; 2])> = interior
                        .iter()
                        .filter_map(|&gi| {
                            uv_raw(flat[gi as usize]).map(|[u, v]| (gi, [u * su, v * sv]))
                        })
                        .collect();
                    let world_of = |u: f64, v: f64| -> [f32; 3] {
                        let (th, t) = (u / su, v / sv);
                        let local = match surface {
                            HalfSpace::InfiniteCylinder { radius } => {
                                let r = *radius as f64;
                                [(r * th.cos()) as f32, t as f32, (r * th.sin()) as f32]
                            }
                            HalfSpace::Cone { half_angle } => {
                                let r = t.abs() * (*half_angle as f64).tan();
                                [(r * th.cos()) as f32, t as f32, (r * th.sin()) as f32]
                            }
                            HalfSpace::Sphere { radius } => {
                                let r = *radius as f64;
                                let sp = t.sin();
                                [
                                    (r * sp * th.cos()) as f32,
                                    (r * t.cos()) as f32,
                                    (r * sp * th.sin()) as f32,
                                ]
                            }
                            HalfSpace::RevolvedProfile { .. } => {
                                let (poly, arc) =
                                    profile.as_ref().expect("revolve chart has a profile");
                                let s_arc =
                                    (t as f32).clamp(0.0, *arc.last().unwrap_or(&0.0));
                                let k = arc
                                    .partition_point(|&a| a < s_arc)
                                    .clamp(1, arc.len() - 1);
                                let (a0, a1) = (arc[k - 1], arc[k]);
                                let f =
                                    if a1 > a0 { (s_arc - a0) / (a1 - a0) } else { 0.0 };
                                let r = (poly[k - 1][0]
                                    + f * (poly[k][0] - poly[k - 1][0]))
                                    as f64;
                                let yy = poly[k - 1][1] + f * (poly[k][1] - poly[k - 1][1]);
                                [(r * th.cos()) as f32, yy, (r * th.sin()) as f32]
                            }
                            _ => [0.0; 3],
                        };
                        let sc = [
                            local[0] * total_t.scale[0],
                            local[1] * total_t.scale[1],
                            local[2] * total_t.scale[2],
                        ];
                        let r = crate::quat_rotate(total_t.rotation, sc);
                        [
                            r[0] + total_t.translate[0],
                            r[1] + total_t.translate[1],
                            r[2] + total_t.translate[2],
                        ]
                    };
                    // Seam spacing converted from world mm to local chart mm.
                    let ms = total_t.scale[0]
                        .abs()
                        .max(total_t.scale[1].abs())
                        .max(total_t.scale[2].abs())
                        .max(1e-6);
                    let aux_base =
                        (n_alive as usize + skeleton.len() + aux_points.len()) as u32;
                    let h_chart = (h_seam / ms) as f64;
                    // Chart-v positions of the carrier's poles/apexes — where
                    // a cap face (one winding loop) closes via ring + fan.
                    let poles: Vec<f64> = match surface {
                        HalfSpace::Sphere { .. } => vec![0.0, std::f64::consts::PI * sv],
                        HalfSpace::Cone { .. } => vec![0.0],
                        HalfSpace::RevolvedProfile { .. } => {
                            let (poly, arc) =
                                profile.as_ref().expect("revolve chart has a profile");
                            poly.iter()
                                .zip(arc.iter())
                                .filter(|(q, _)| q[0].abs() < 1e-3)
                                .map(|(_, &a)| a as f64)
                                .collect()
                        }
                        _ => Vec::new(),
                    };
                    if let Some(mesh) = crate::mesher_cdt::triangulate_periodic_face(
                        &scaled,
                        &scaled_interior,
                        period,
                        h_chart,
                        0.25 * h_chart,
                        &poles,
                        aux_base,
                        &world_of,
                        None,
                    ) {
                        cdt_tris.extend(mesh.tris);
                        aux_points.extend(mesh.aux);
                        break 'chart true;
                    }
                    false
                    };
                    if !meshed && opts.fallback_patches {
                        fallback_faces.push((bi, face.branch_id.as_u32(), face_loops));
                    }
                }
            }

            // ── §6 F3: DC patch + rim zipper for the chartless faces ──
            // Interior = the octree DC mesh over the face's attributed cells
            // (same connectivity as the debug DC mesh); the rim seam reuses
            // the exact shared skeleton indices via a greedy zipper — the
            // watertightness stays structural, only the interior quality
            // drops to DC level. Plan §6, path F3.
            if !fallback_faces.is_empty() {
                use crate::mesher_patch::{
                    boundary_loops, dist_to_ring, tri_components, zip_rings,
                };
                // Single-layer guard: only cells whose corners straddle the
                // surface carry the patch — the near-surface band can be two
                // cells thick, and a second layer would fold the sheet.
                let straddles = |leaf: &crate::mesher_points::Leaf| -> bool {
                    let h2 = leaf.size * 0.5;
                    let c = leaf.center;
                    let (mut pos, mut neg) = (false, false);
                    for dx in [-h2, h2] {
                        for dy in [-h2, h2] {
                            for dz in [-h2, h2] {
                                let d = scene_sdf([c[0] + dx, c[1] + dy, c[2] + dz]);
                                pos |= d > 0.0;
                                neg |= d <= 0.0;
                            }
                        }
                    }
                    pos && neg
                };
                let clearance = 0.5 * h_seam;
                for (bi, branch, loops) in &fallback_faces {
                    let Some(leaf_ids) = by_branch_leaf.get(&(*bi, *branch)) else {
                        continue;
                    };
                    let rims: Vec<Vec<[f32; 3]>> = loops
                        .iter()
                        .map(|r| r.iter().map(|&gi| flat[gi as usize]).collect())
                        .collect();
                    let near_rim = |p: [f32; 3]| -> f32 {
                        rims.iter().map(|r| dist_to_ring(p, r)).fold(f32::MAX, f32::min)
                    };
                    let mut shown_region = vec![false; leaves.len()];
                    for &li in leaf_ids {
                        if shown_region[li] || !straddles(&leaves[li]) {
                            continue;
                        }
                        // Clearance band: cells whose particle sits closer to
                        // the rim than 0.5·h retreat — the zipper strip needs
                        // room, and a particle ON the rim would duplicate a
                        // chain vertex.
                        if near_rim(points[li]) >= clearance {
                            shown_region[li] = true;
                        }
                    }
                    let idx = crate::mesher_points::connect_octree_edges_signed(
                        &root,
                        &shown_region,
                        &scene_sdf,
                    );
                    let patch: Vec<[u32; 3]> = idx
                        .chunks(3)
                        .map(|c| {
                            [
                                remap[c[0] as usize],
                                remap[c[1] as usize],
                                remap[c[2] as usize],
                            ]
                        })
                        .collect();
                    if patch.is_empty() {
                        continue;
                    }
                    // Keep only components near THIS face's rims: one branch
                    // may carry several chartless faces (both torus bands,
                    // both gyroid sheets) — each face keeps its own sheet.
                    let comp = tri_components(&patch);
                    let n_comp = comp.iter().copied().max().map_or(0, |m| m + 1);
                    let mut comp_dist = vec![f32::MAX; n_comp];
                    for (t, tri) in patch.iter().enumerate() {
                        for &v in tri {
                            let d = near_rim(flat[v as usize]);
                            let slot = &mut comp_dist[comp[t]];
                            *slot = slot.min(d);
                        }
                    }
                    let mut kept: Vec<[u32; 3]> = Vec::new();
                    let mut kept_comp: Vec<usize> = Vec::new();
                    let mut vcomp: std::collections::HashMap<u32, usize> =
                        std::collections::HashMap::new();
                    for (t, tri) in patch.iter().enumerate() {
                        if comp_dist[comp[t]] < 2.0 * h_seam {
                            kept.push(*tri);
                            kept_comp.push(comp[t]);
                            for &v in tri {
                                vcomp.insert(v, comp[t]);
                            }
                        }
                    }
                    if kept.is_empty() {
                        continue;
                    }
                    // Zip each patch boundary loop to its best-matching rim
                    // (mean vertex→rim distance, greedy 1:1). Unmatched loops
                    // stay open — the watertightness gate reports the hole.
                    let bls = boundary_loops(&kept);
                    let mut pairs: Vec<(f32, usize, usize)> = Vec::new();
                    for (li, bl) in bls.iter().enumerate() {
                        for (ri, rim) in rims.iter().enumerate() {
                            let sum: f32 = bl
                                .iter()
                                .map(|&v| dist_to_ring(flat[v as usize], rim))
                                .sum();
                            // Length-mismatch penalty: interior holes (a few
                            // dropped sliver cells) are SHORT loops that can
                            // sit as close to a rim as the true patch border
                            // — the vertex-count ratio keeps them from
                            // stealing the rim match.
                            let ratio = bl.len() as f32 / rim.len().max(1) as f32;
                            let penalty = ratio.max(1.0 / ratio.max(1e-3));
                            pairs.push((penalty * sum / bl.len() as f32, li, ri));
                        }
                    }
                    pairs.sort_by(|a, b| {
                        a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal)
                    });
                    let pos_of = |i: u32| flat[i as usize];
                    let mut zip_tris: Vec<[u32; 3]> = Vec::new();
                    let mut zipped_comps: std::collections::HashSet<usize> =
                        std::collections::HashSet::new();
                    let mut used_bl = vec![false; bls.len()];
                    let mut used_rim = vec![false; rims.len()];
                    for (_, li, ri) in pairs {
                        if used_bl[li] || used_rim[ri] {
                            continue;
                        }
                        used_bl[li] = true;
                        used_rim[ri] = true;
                        if let Some(&c) = vcomp.get(&bls[li][0]) {
                            zipped_comps.insert(c);
                        }
                        zip_tris.extend(zip_rings(&bls[li], &loops[ri], &pos_of));
                    }
                    // Only components that actually reached a rim survive:
                    // stray straddle clusters near (but not on) the face
                    // would otherwise close into floating "pillows" and
                    // pollute the topology.
                    for (t, tri) in kept.iter().enumerate() {
                        if zipped_comps.contains(&kept_comp[t]) {
                            fallback_tris.push(*tri);
                        }
                    }
                    fallback_tris.extend(zip_tris);
                    // Remaining boundary loops ON zipped components are
                    // interior HOLES — cells the straddle test dropped
                    // (tangential slivers have no corner sign change). Close
                    // each with a fan around a NEW centroid vertex: a plain
                    // fan re-uses loop diagonals that can already exist as
                    // patch edges (the DC connectivity is that ragged),
                    // while every centroid edge is new by construction.
                    // Combinatorially watertight; DC-level geometry is this
                    // path's contract.
                    for (li, bl) in bls.iter().enumerate() {
                        if used_bl[li] || bl.len() < 3 {
                            continue;
                        }
                        if vcomp.get(&bl[0]).is_none_or(|c| !zipped_comps.contains(c)) {
                            continue;
                        }
                        let mut c = [0.0f32; 3];
                        for &v in bl {
                            let p = flat[v as usize];
                            for a in 0..3 {
                                c[a] += p[a];
                            }
                        }
                        let inv = 1.0 / bl.len() as f32;
                        for a in &mut c {
                            *a *= inv;
                        }
                        let centre =
                            (n_alive as usize + skeleton.len() + aux_points.len()) as u32;
                        aux_points.push(c);
                        for k in 0..bl.len() {
                            fallback_tris.push([centre, bl[k], bl[(k + 1) % bl.len()]]);
                        }
                    }
                }
            }
        }

        let n_total = n_alive as usize + skeleton.len() + aux_points.len();
        let mut out = Vec::with_capacity(
            3 + n_total * 9 + indices.len() + (cdt_tris.len() + fallback_tris.len()) * 3,
        );
        out.push(n_total as f32);
        out.push((indices.len() / 3 + cdt_tris.len() + fallback_tris.len()) as f32);
        // §6-F3 patch/zipper triangles — they sit LAST in the index block so
        // the overlay can colour them distinctly from the CDT wireframe.
        out.push(fallback_tris.len() as f32);
        for (i, leaf) in leaves.iter().enumerate() {
            if !alive[i] {
                continue;
            }
            let p = points[i];
            let (d, n) = if processed[i] && (opts.fit_on_surface || opts.repulsion) {
                (scene_sdf(p), grad(p))
            } else {
                (leaf.sdf, leaf.normal)
            };
            let shown = if processed[i] { 1.0 } else { 0.0 };
            out.extend_from_slice(&[p[0], p[1], p[2], d, n[0], n[1], n[2], leaf.size, shown]);
        }
        // Skeleton points: flag 2 = edge chain, 3 = corner (drawn distinctly
        // by the overlay; the octree mesh never references them, so appending
        // after the leaf points keeps every triangle index valid).
        for (k, p) in skeleton.iter().enumerate() {
            let flag = if skeleton_corner[k] { 3.0 } else { 2.0 };
            out.extend_from_slice(&[p[0], p[1], p[2], 0.0, 0.0, 0.0, 1.0, 0.0, flag]);
        }
        // Periodic-face seam Steiner vertices: chain-coloured (flag 2), only
        // referenced by the CDT triangles.
        for p in &aux_points {
            out.extend_from_slice(&[p[0], p[1], p[2], 0.0, 0.0, 0.0, 1.0, 0.0, 2.0]);
        }
        out.extend(indices.iter().map(|&i| remap[i as usize] as f32));
        for t in cdt_tris.iter().chain(fallback_tris.iter()) {
            out.extend_from_slice(&[t[0] as f32, t[1] as f32, t[2] as f32]);
        }
        out
    }

    /// World-space centre of the *visible* solids' AABB — the orbit pivot
    /// (ticket f88u). This is the union of the per-body `Body.aabb`, which the
    /// feature pipeline keeps analytically current: a boolean Remove retreats
    /// the bound past the slab it caps, an Intersect clips it to the tool, so a
    /// half-carved sphere reports the visible remainder — unlike
    /// [`Kernel::scene_aabb`], whose Difference bound stays the conservative
    /// *left operand* (the phantom whole sphere) for clip-plane framing. `[]`
    /// for an empty scene → the caller keeps the conservative centre. Cheap
    /// (unions a handful of stored boxes), so it needs no memoisation.
    #[wasm_bindgen(js_name = sceneVisibleCenter)]
    pub fn scene_visible_center(&self) -> Vec<f32> {
        let mut bb = crate::csg::Aabb::EMPTY;
        for b in &self.bodies {
            bb = crate::csg::Aabb::union(bb, b.aabb);
        }
        if bb.is_empty() {
            return Vec::new();
        }
        vec![
            0.5 * (bb.min[0] + bb.max[0]),
            0.5 * (bb.min[1] + bb.max[1]),
            0.5 * (bb.min[2] + bb.max[2]),
        ]
    }

    /// Pre-mesh estimate for the export dialog: for dense-grid methods, the
    /// finest achievable cell size (memory-guard lower bound) and a rough
    /// raw-grid triangle count at `cell_size_mm`, as JSON
    /// ([`crate::export_mesh::ExportEstimate`]). Read-only and cheap, so the
    /// dialog can call it live while the user edits the cell size / method.
    #[wasm_bindgen(js_name = exportEstimate)]
    pub fn export_estimate(&self, cell_size_mm: f32, method: String) -> String {
        let method = crate::export_mesh::MeshMethod::from_wire(&method);
        serde_json::to_string(&self.export_estimate_internal(cell_size_mm, method))
            .unwrap_or_default()
    }

    /// Mesh + validate body `i` for the running session. Returns the
    /// per-body report as JSON (`""` when no session is active).
    #[wasm_bindgen(js_name = exportMeshBody)]
    pub fn export_mesh_body(&mut self, i: usize) -> String {
        match self.export_mesh_body_internal(i) {
            Some(report) => serde_json::to_string(&report).unwrap_or_default(),
            None => String::new(),
        }
    }

    /// Binary STL of the session's export meshes.
    #[wasm_bindgen(js_name = exportMeshStl)]
    pub fn export_mesh_stl(&self) -> Vec<u8> {
        export::export_binary(&self.export_session_meshes())
    }

    /// ASCII STL of the session's export meshes (debug / diffing).
    #[wasm_bindgen(js_name = exportMeshStlAscii)]
    pub fn export_mesh_stl_ascii(&self) -> String {
        export::export_ascii(&self.export_session_meshes())
    }

    /// 3MF archive of the session's export meshes.
    #[wasm_bindgen(js_name = exportMesh3mf)]
    pub fn export_mesh_3mf(&self) -> Vec<u8> {
        export_3mf::export_3mf(&self.export_session_meshes())
    }

    /// 3MF archive from raw triangle buffers (xyz-interleaved positions +
    /// indices) — the server-meshing route (CGAL Mesh_3 returns an STL)
    /// reuses the kernel's 3MF encoder for its format conversion.
    #[wasm_bindgen(js_name = mesh3mfFromBuffers)]
    pub fn mesh_3mf_from_buffers(&self, positions: Vec<f32>, indices: Vec<u32>) -> Vec<u8> {
        let mesh = crate::dc::Mesh {
            positions,
            indices,
            ..Default::default()
        };
        export_3mf::export_3mf(&[&mesh])
    }

    /// Merged vertex positions (xyz-interleaved) of ALL session export
    /// meshes, for the in-app solid preview. Pairs with
    /// [`Kernel::export_mesh_preview_indices`] (indices are rebased per
    /// body). Flat shading derives face normals in the viewer, so no
    /// normal buffer crosses the boundary.
    #[wasm_bindgen(js_name = exportMeshPreviewPositions)]
    pub fn export_mesh_preview_positions(&self) -> Vec<f32> {
        let mut out = Vec::new();
        for m in self.export_session_meshes() {
            out.extend_from_slice(&m.positions);
        }
        out
    }

    /// Merged triangle indices of ALL session export meshes, rebased so
    /// they index into the concatenated
    /// [`Kernel::export_mesh_preview_positions`] buffer.
    #[wasm_bindgen(js_name = exportMeshPreviewIndices)]
    pub fn export_mesh_preview_indices(&self) -> Vec<u32> {
        let mut out = Vec::new();
        let mut base = 0u32;
        for m in self.export_session_meshes() {
            out.extend(m.indices.iter().map(|&idx| idx + base));
            base += (m.positions.len() / 3) as u32;
        }
        out
    }

    /// Aggregate report of the session as JSON
    /// (see [`crate::export_mesh::ExportReport`]).
    #[wasm_bindgen(js_name = exportMeshReport)]
    pub fn export_mesh_report(&self) -> String {
        match &self.export_session {
            Some(s) => serde_json::to_string(&s.report()).unwrap_or_default(),
            None => String::new(),
        }
    }

    /// Drop the session and free its meshes.
    #[wasm_bindgen(js_name = exportMeshEnd)]
    pub fn export_mesh_end(&mut self) {
        self.export_session = None;
    }

    // ── M14b: STEP classification ─────────────────────────────────

    #[wasm_bindgen(js_name = classifyForStep)]
    pub fn classify_for_step(&self) -> String {
        let report = step_classifier::classify(&self.decoded);
        report.to_json().to_string()
    }

    #[wasm_bindgen(js_name = stepCommands)]
    pub fn step_commands(&self) -> String {
        let cmds = step_commands::build_step_commands(&self.decoded);
        cmds.to_string()
    }

    /// Serialise body `i`'s feature-based B-Rep as an ISO-10303-21
    /// (STEP) AP214 advanced-B-Rep file. Reuses the same lazily-woven,
    /// cached B-Rep as [`Kernel::body_brep_counts`]; the returned
    /// string is a complete `.step` document ready to download. The
    /// product name is derived from the body's stable feature id.
    #[wasm_bindgen(js_name = bodyStepExport)]
    pub fn body_step_export(&mut self, i: usize) -> String {
        let product_name = format!("body_{}", self.bodies[i].id);
        self.bodies[i].ensure_brep(&self.decoded, self.parametric_weave);
        let placement = self.bodies[i].placement;
        let brep = self.bodies[i].brep_cached().expect("woven above");
        crate::brep::step_export::export_step_placed(brep, &placement, &product_name)
    }

    /// Serialise *every* body's feature-based B-Rep into ONE ISO-10303-21
    /// (STEP) AP214 file, each body a separate `MANIFOLD_SOLID_BREP` solid
    /// in a single shape representation (DL-53: bodies stay separate). Each
    /// body's B-Rep is lazily woven and cached exactly as in
    /// [`Kernel::body_step_export`]; the returned string is a complete
    /// `.step` document ready to download.
    #[wasm_bindgen(js_name = sceneStepExport)]
    pub fn scene_step_export(&mut self) -> String {
        // Weave every body first (mutates each body's cache), then borrow the
        // breps immutably for export — two passes so the mutable `ensure_brep`
        // borrows don't overlap the shared-slice borrow.
        for i in 0..self.bodies.len() {
            self.bodies[i].ensure_brep(&self.decoded, self.parametric_weave);
        }
        let breps: Vec<(&crate::brep::BRep, fielddraft_protocol::Transform)> = self
            .bodies
            .iter()
            .map(|b| (b.brep_cached().expect("woven above"), b.placement))
            .collect();
        crate::brep::step_export::export_step_scene_placed(&breps, "scene")
    }

}

/// Debug ▸ Mesher core (kept free of the wasm boundary so it is unit-testable):
/// build a narrow-band adaptive octree over the union of `bodies` and return
/// every leaf cell centre packed `[x, y, z, sdf, nx, ny, nz, cellSize]`.
#[cfg(test)]
pub(crate) fn octree_leaf_samples(
    bodies: &[crate::bvh::BoundedCsg],
    max_depth: u32,
    band_guard: f32,
) -> Vec<f32> {
    octree_leaves(bodies, max_depth, band_guard)
        .into_iter()
        .flat_map(|l| {
            [
                l.center[0], l.center[1], l.center[2], l.sdf, l.normal[0], l.normal[1],
                l.normal[2], l.size,
            ]
        })
        .collect()
}

/// The leaf structs behind [`octree_leaf_samples`] — the jq2t point pipeline
/// needs the cells (centre + size = clamp bounds), not just packed floats.
/// Root sizing lives here; construction is `mesher_points::build_octree`.
/// [`octree_root`] over a selected subset of `bodies` (the PASM per-body
/// export path meshes one body at a time).
pub(crate) fn octree_root_of(
    bodies: &[crate::bvh::BoundedCsg],
    active: &[usize],
) -> Option<([f32; 3], f32)> {
    octree_root_iter(active.iter().filter_map(|&i| bodies.get(i)))
}

pub(crate) fn octree_root(bodies: &[crate::bvh::BoundedCsg]) -> Option<([f32; 3], f32)> {
    octree_root_iter(bodies.iter())
}

fn octree_root_iter<'a>(
    bodies: impl Iterator<Item = &'a crate::bvh::BoundedCsg>,
) -> Option<([f32; 3], f32)> {
    let mut mn = [f32::MAX; 3];
    let mut mx = [f32::MIN; 3];
    let mut any = false;
    let mut empty = true;
    for b in bodies {
        empty = false;
        let a = b.aabb();
        if !a.is_empty() {
            any = true;
            for k in 0..3 {
                mn[k] = mn[k].min(a.min[k]);
                mx[k] = mx[k].max(a.max[k]);
            }
        }
    }
    if empty {
        return None;
    }
    if !any {
        mn = [-50.0; 3];
        mx = [50.0; 3];
    }
    let center = [
        0.5 * (mn[0] + mx[0]),
        0.5 * (mn[1] + mx[1]),
        0.5 * (mn[2] + mx[2]),
    ];
    let ext = (mx[0] - mn[0]).max(mx[1] - mn[1]).max(mx[2] - mn[2]);
    Some((center, (ext * 1.1).max(1e-3)))
}

pub(crate) fn octree_leaves(
    bodies: &[crate::bvh::BoundedCsg],
    max_depth: u32,
    band_guard: f32,
) -> Vec<crate::mesher_points::Leaf> {
    let Some((center, root_size)) = octree_root(bodies) else {
        return Vec::new();
    };
    let scene_sdf = |p: [f32; 3]| {
        let mut d = f32::MAX;
        for b in bodies {
            d = d.min(b.sdf(p, crate::csg::EvalMode::Visual));
        }
        d
    };
    let eps = (root_size * 0.005).clamp(1e-3, 1.0);
    let grad = |p: [f32; 3]| -> [f32; 3] {
        let gx = scene_sdf([p[0] + eps, p[1], p[2]]) - scene_sdf([p[0] - eps, p[1], p[2]]);
        let gy = scene_sdf([p[0], p[1] + eps, p[2]]) - scene_sdf([p[0], p[1] - eps, p[2]]);
        let gz = scene_sdf([p[0], p[1], p[2] + eps]) - scene_sdf([p[0], p[1], p[2] - eps]);
        let gl = (gx * gx + gy * gy + gz * gz).sqrt();
        if gl > 1e-12 {
            [gx / gl, gy / gl, gz / gl]
        } else {
            [0.0, 0.0, 1.0]
        }
    };
    let mut root =
        crate::mesher_points::build_octree(center, root_size, max_depth, band_guard, &scene_sdf);
    crate::mesher_points::collect_leaves(&mut root, &grad)
}

#[cfg(test)]
mod mesher_octree_tests {
    use super::octree_leaf_samples;
    use crate::bvh::BoundedCsg;
    use crate::csg::{CsgNode, DecodedFeature, PrimitiveModifiers};
    use fielddraft_protocol::{BoolOp, FeatureId, Primitive, Transform};

    fn sphere(radius: f32) -> BoundedCsg {
        let f = DecodedFeature::from_parts(
            FeatureId::from_u32(1),
            "s".into(),
            BoolOp::New,
            Primitive::Sphere {
                center: [0.0, 0.0, 0.0],
                radius,
            },
            Transform::IDENTITY,
            PrimitiveModifiers::NONE,
            vec![],
        );
        BoundedCsg::from_csg(&CsgNode::from_decoded(&[f]))
    }

    #[test]
    fn octree_is_adaptive_with_outward_normals_on_a_sphere() {
        let bodies = [sphere(10.0)];
        let out = octree_leaf_samples(&bodies, 5, 1.0);
        assert!(!out.is_empty() && out.len() % 8 == 0);
        let n = out.len() / 8;

        // Adaptive: cells near the surface are the FINEST, cells far away stay
        // coarse — so more than one distinct cell size appears.
        let mut sizes: Vec<f32> = (0..n).map(|k| out[k * 8 + 7]).collect();
        sizes.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert!(
            *sizes.last().unwrap() > *sizes.first().unwrap() * 1.5,
            "expected a range of cell sizes (adaptive), got {sizes:?}"
        );

        // The finest cells cluster on the surface; the gradient there points
        // radially outward (n · p̂ ≈ 1 for a sphere).
        let mut checked = 0;
        for k in 0..n {
            let b = k * 8;
            let (x, y, z, d) = (out[b], out[b + 1], out[b + 2], out[b + 3]);
            let (nx, ny, nz) = (out[b + 4], out[b + 5], out[b + 6]);
            let r = (x * x + y * y + z * z).sqrt();
            if d.abs() < 1.0 && r > 1e-3 {
                let dot = (nx * x + ny * y + nz * z) / r;
                assert!(dot > 0.9, "surface normal should point outward, dot={dot}");
                checked += 1;
            }
        }
        assert!(checked > 0, "expected some near-surface leaf cells");
    }

    #[test]
    fn empty_scene_yields_no_cells() {
        assert!(octree_leaf_samples(&[], 5, 1.0).is_empty());
    }

    /// Hybrid-PASM P3 (ticket nwmi): skeleton + CDT on a box must produce a
    /// CLOSED 2-manifold triangle mesh — every edge shared by exactly two
    /// triangles, Euler characteristic V − E + F = 2. That is the structural
    /// watertightness contract of the shared skeleton chains.
    #[test]
    fn mesher_points_cdt_box_is_watertight_manifold() {
        use fielddraft_protocol::{BoolOp, Primitive};
        let mut k = crate::Kernel::new();
        k.add_primitive(
            Primitive::KIND_BOX,
            BoolOp::TAG_NEW,
            0.0,
            0.0,
            0.0,
            10.0,
            10.0,
            10.0,
        )
        .unwrap();
        let out = k.mesher_points(
            r#"{"maxDepth":4,"brepSkeleton":true,"cdtFaces":true,
                "nearSurfaceOnly":true,"nearSurfaceDist":1.0,
                "fitOnSurface":true,"onlyOnSurface":true,
                "repulsion":true,"interactionRadius":3.0,"repulsionStrength":0.5,
                "pasmKernel":true,"iterations":20}"#
                .into(),
        );
        let total = out[0] as usize;
        let tris = out[1] as usize;
        assert!(tris >= 12, "expected a real triangulation, got {tris} tris");
        let idx_base = 3 + total * 9;
        // Edge → usage count over all triangles.
        let mut edge_use: std::collections::HashMap<(u32, u32), u32> =
            std::collections::HashMap::new();
        let mut used: std::collections::HashSet<u32> = std::collections::HashSet::new();
        for t in 0..tris {
            let a = out[idx_base + t * 3] as u32;
            let b = out[idx_base + t * 3 + 1] as u32;
            let c3 = out[idx_base + t * 3 + 2] as u32;
            assert!(a != b && b != c3 && c3 != a, "degenerate triangle");
            for (x, y) in [(a, b), (b, c3), (c3, a)] {
                *edge_use.entry((x.min(y), x.max(y))).or_default() += 1;
                used.insert(x);
                used.insert(y);
            }
        }
        // Closed 2-manifold: EVERY edge exactly twice.
        for (&(a, b), &n) in &edge_use {
            assert_eq!(n, 2, "edge {a}-{b} used {n}× (open seam or fan)");
        }
        // Euler characteristic of a sphere-topology solid.
        let v = used.len() as i64;
        let e = edge_use.len() as i64;
        let f = tris as i64;
        assert_eq!(v - e + f, 2, "Euler characteristic broke: V={v} E={e} F={f}");
    }

    /// Mesher-PASM preset: `targetEdgeLength` alone must stand in for
    /// `maxDepth` + `interactionRadius` (kernel-derived) and still produce
    /// the closed 2-manifold box mesh — the contract of the dedicated
    /// Debug ▸ Mesher PASM dialog, which only exposes h.
    #[test]
    fn mesher_points_pasm_target_edge_length_derives_resolution() {
        use fielddraft_protocol::{BoolOp, Primitive};
        let mut k = crate::Kernel::new();
        k.add_primitive(
            Primitive::KIND_BOX,
            BoolOp::TAG_NEW,
            0.0,
            0.0,
            0.0,
            10.0,
            10.0,
            10.0,
        )
        .unwrap();
        let out = k.mesher_points(
            r#"{"targetEdgeLength":3.0,"brepSkeleton":true,"cdtFaces":true,
                "nearSurfaceOnly":true,"nearSurfaceDist":1.0,
                "fitOnSurface":true,"onlyOnSurface":true,
                "repulsion":true,"repulsionStrength":0.5,
                "pasmKernel":true,"iterations":20}"#
                .into(),
        );
        let total = out[0] as usize;
        let tris = out[1] as usize;
        assert!(tris >= 12, "expected a real triangulation, got {tris} tris");
        let idx_base = 3 + total * 9;
        let mut edge_use: std::collections::HashMap<(u32, u32), u32> =
            std::collections::HashMap::new();
        let mut used: std::collections::HashSet<u32> = std::collections::HashSet::new();
        for t in 0..tris {
            let a = out[idx_base + t * 3] as u32;
            let b = out[idx_base + t * 3 + 1] as u32;
            let c3 = out[idx_base + t * 3 + 2] as u32;
            assert!(a != b && b != c3 && c3 != a, "degenerate triangle");
            for (x, y) in [(a, b), (b, c3), (c3, a)] {
                *edge_use.entry((x.min(y), x.max(y))).or_default() += 1;
                used.insert(x);
                used.insert(y);
            }
        }
        for (&(a, b), &n) in &edge_use {
            assert_eq!(n, 2, "edge {a}-{b} used {n}× (open seam or fan)");
        }
        let v = used.len() as i64;
        let e = edge_use.len() as i64;
        let f = tris as i64;
        assert_eq!(v - e + f, 2, "Euler characteristic broke: V={v} E={e} F={f}");
    }

    /// Hybrid-PASM P4 (ticket nwmi): a CYLINDER exercises the developable
    /// carrier path — the barrel face unrolls into its (θ·r, y) chart, gets
    /// cut open along a seam whose two copies share global vertex indices,
    /// and must close up watertight against the two planar cap discs:
    /// every edge in exactly two triangles, Euler V − E + F = 2.
    #[test]
    fn mesher_points_cdt_cylinder_is_watertight_manifold() {
        use fielddraft_protocol::{BoolOp, Primitive};
        let mut k = crate::Kernel::new();
        k.add_primitive(
            Primitive::KIND_CYLINDER,
            BoolOp::TAG_NEW,
            0.0,
            0.0,
            0.0,
            10.0,
            10.0,
            0.0,
        )
        .unwrap();
        let out = k.mesher_points(
            r#"{"maxDepth":4,"brepSkeleton":true,"cdtFaces":true,
                "nearSurfaceOnly":true,"nearSurfaceDist":1.0,
                "fitOnSurface":true,"onlyOnSurface":true,
                "repulsion":true,"interactionRadius":3.0,"repulsionStrength":0.5,
                "pasmKernel":true,"iterations":20}"#
                .into(),
        );
        let total = out[0] as usize;
        let tris = out[1] as usize;
        assert!(tris >= 12, "expected a real triangulation, got {tris} tris");
        let idx_base = 3 + total * 9;
        let mut edge_use: std::collections::HashMap<(u32, u32), u32> =
            std::collections::HashMap::new();
        let mut used: std::collections::HashSet<u32> = std::collections::HashSet::new();
        for t in 0..tris {
            let a = out[idx_base + t * 3] as u32;
            let b = out[idx_base + t * 3 + 1] as u32;
            let c3 = out[idx_base + t * 3 + 2] as u32;
            assert!(a != b && b != c3 && c3 != a, "degenerate triangle");
            for (x, y) in [(a, b), (b, c3), (c3, a)] {
                *edge_use.entry((x.min(y), x.max(y))).or_default() += 1;
                used.insert(x);
                used.insert(y);
            }
        }
        for (&(a, b), &n) in &edge_use {
            assert_eq!(n, 2, "edge {a}-{b} used {n}× (open seam or fan)");
        }
        let v = used.len() as i64;
        let e = edge_use.len() as i64;
        let f = tris as i64;
        assert_eq!(v - e + f, 2, "Euler characteristic broke: V={v} E={e} F={f}");
    }

    /// Hybrid-PASM P4b (ticket nwmi): a revolved TUBE (rectangle profile at
    /// r ∈ [5, 9] about Y, intersected with a y-slab box) exercises the
    /// RevolvedProfile chart — v is the arc length of the nearest-point
    /// projection onto the offset-corrected profile chain. Outer AND inner
    /// wall live on the SAME branch (one RevolvedProfile half-space) and
    /// must separate purely through the chart; the caps are annuli (planar
    /// path with a hole loop). Torus topology: every edge in exactly two
    /// triangles, Euler V − E + F = 0.
    #[test]
    fn mesher_points_cdt_revolve_tube_is_watertight_manifold() {
        let mut k = crate::Kernel::new();
        k.document = serde_json::from_str(
            &serde_json::json!({
                "schema_version": 2,
                "name": "revolve-tube",
                "next_feature_id": 4,
                "feature_calls": [
                    { "type": "sketch", "id": "1", "params": {
                        "plane": "qCreatedBy(origin_plane_xy)",
                        "sub_calls": [
                            { "type": "sketch_rectangle", "id": "r1", "params": {
                                "corner_a": [5.0, -10.0], "corner_b": [9.0, 10.0] } },
                            { "type": "sketch_line", "id": "axis", "params": {
                                "start": [0.0, -10.0], "end": [0.0, 10.0] } }
                        ] } },
                    { "type": "revolve", "id": "2", "params": {
                        "profiles": "qCreatedBy(1)", "axis": "qSketchLine(1, 11)" } },
                    { "type": "box", "id": "3", "params": {
                        "half_extents": [20.0, 6.0, 20.0],
                        "translate": [0.0, 0.0, 0.0],
                        "op": "intersect", "targets": "qCreatedBy(2)" } }
                ]
            })
            .to_string(),
        )
        .unwrap();
        k.bootstrap_features_from_calls().unwrap();
        k.rebuild_packed();
        let out = k.mesher_points(
            r#"{"maxDepth":4,"brepSkeleton":true,"cdtFaces":true,
                "nearSurfaceOnly":true,"nearSurfaceDist":1.0,
                "fitOnSurface":true,"onlyOnSurface":true,
                "repulsion":true,"interactionRadius":3.0,"repulsionStrength":0.5,
                "pasmKernel":true,"iterations":20}"#
                .into(),
        );
        let total = out[0] as usize;
        let tris = out[1] as usize;
        assert!(tris >= 16, "expected a real triangulation, got {tris} tris");
        let idx_base = 3 + total * 9;
        let mut edge_use: std::collections::HashMap<(u32, u32), u32> =
            std::collections::HashMap::new();
        let mut used: std::collections::HashSet<u32> = std::collections::HashSet::new();
        for t in 0..tris {
            let a = out[idx_base + t * 3] as u32;
            let b = out[idx_base + t * 3 + 1] as u32;
            let c3 = out[idx_base + t * 3 + 2] as u32;
            assert!(a != b && b != c3 && c3 != a, "degenerate triangle");
            for (x, y) in [(a, b), (b, c3), (c3, a)] {
                *edge_use.entry((x.min(y), x.max(y))).or_default() += 1;
                used.insert(x);
                used.insert(y);
            }
        }
        for (&(a, b), &n) in &edge_use {
            assert_eq!(n, 2, "edge {a}-{b} used {n}× (open seam or fan)");
        }
        let v = used.len() as i64;
        let e = edge_use.len() as i64;
        let f = tris as i64;
        assert_eq!(
            v - e + f,
            0,
            "tube Euler characteristic broke: V={v} E={e} F={f}"
        );
    }

    /// Hybrid-PASM P4c (ticket nwmi): a SPHERE CAP (sphere ∩ y-slab box
    /// enclosing the +Y pole) exercises the pole-fan path — the cap face is
    /// one winding loop around the pole and closes via pole ring + fan;
    /// together with the planar disc it must form a closed 2-manifold
    /// (every edge in exactly two triangles, Euler V − E + F = 2).
    #[test]
    fn mesher_points_cdt_sphere_cap_is_watertight_manifold() {
        let mut k = crate::Kernel::new();
        k.document = serde_json::from_str(
            &serde_json::json!({
                "schema_version": 2,
                "name": "sphere-cap",
                "next_feature_id": 3,
                "feature_calls": [
                    { "type": "sphere", "id": "1", "params": {
                        "radius": 10.0, "translate": [0.0, 0.0, 0.0] } },
                    { "type": "box", "id": "2", "params": {
                        "half_extents": [20.0, 8.0, 20.0],
                        "translate": [0.0, 12.0, 0.0],
                        "op": "intersect", "targets": "qCreatedBy(1)" } }
                ]
            })
            .to_string(),
        )
        .unwrap();
        k.bootstrap_features_from_calls().unwrap();
        k.rebuild_packed();
        let out = k.mesher_points(
            r#"{"maxDepth":4,"brepSkeleton":true,"cdtFaces":true,
                "nearSurfaceOnly":true,"nearSurfaceDist":1.0,
                "fitOnSurface":true,"onlyOnSurface":true,
                "repulsion":true,"interactionRadius":3.0,"repulsionStrength":0.5,
                "pasmKernel":true,"iterations":20}"#
                .into(),
        );
        let total = out[0] as usize;
        let tris = out[1] as usize;
        assert!(tris >= 12, "expected a real triangulation, got {tris} tris");
        let idx_base = 3 + total * 9;
        let mut edge_use: std::collections::HashMap<(u32, u32), u32> =
            std::collections::HashMap::new();
        let mut used: std::collections::HashSet<u32> = std::collections::HashSet::new();
        for t in 0..tris {
            let a = out[idx_base + t * 3] as u32;
            let b = out[idx_base + t * 3 + 1] as u32;
            let c3 = out[idx_base + t * 3 + 2] as u32;
            assert!(a != b && b != c3 && c3 != a, "degenerate triangle");
            for (x, y) in [(a, b), (b, c3), (c3, a)] {
                *edge_use.entry((x.min(y), x.max(y))).or_default() += 1;
                used.insert(x);
                used.insert(y);
            }
        }
        for (&(a, b), &n) in &edge_use {
            assert_eq!(n, 2, "edge {a}-{b} used {n}× (open seam or fan)");
        }
        let v = used.len() as i64;
        let e = edge_use.len() as i64;
        let f = tris as i64;
        assert_eq!(v - e + f, 2, "cap Euler characteristic broke: V={v} E={e} F={f}");
    }

    /// Count the closed-mesh stats of a `mesher_points` run: `(triangles,
    /// watertight, euler)`. Shared by the anisotropy test.
    #[cfg(test)]
    fn mesh_stats(out: &[f32]) -> (usize, bool, i64) {
        let total = out[0] as usize;
        let tris = out[1] as usize;
        let idx_base = 3 + total * 9;
        let mut edge_use: std::collections::HashMap<(u32, u32), u32> =
            std::collections::HashMap::new();
        let mut used: std::collections::HashSet<u32> = std::collections::HashSet::new();
        for t in 0..tris {
            let a = out[idx_base + t * 3] as u32;
            let b = out[idx_base + t * 3 + 1] as u32;
            let c = out[idx_base + t * 3 + 2] as u32;
            for (x, y) in [(a, b), (b, c), (c, a)] {
                *edge_use.entry((x.min(y), x.max(y))).or_default() += 1;
                used.insert(x);
                used.insert(y);
            }
        }
        let watertight = !edge_use.is_empty() && edge_use.values().all(|&n| n == 2);
        let euler = used.len() as i64 - edge_use.len() as i64 + tris as i64;
        (tris, watertight, euler)
    }

    /// Hybrid-PASM P4d-2 (§5.4, ticket nwmi): curvature anisotropy on a tall
    /// cylinder. At the SAME sag tolerance the anisotropic run must produce
    /// FEWER triangles than the isotropic run — the barrel stays coarse along
    /// the flat axis and only refines around the curved circumference — while
    /// keeping the mesh closed and 2-manifold (Euler 2).
    #[test]
    fn mesher_points_anisotropy_cuts_cylinder_triangles() {
        use fielddraft_protocol::{BoolOp, Primitive};
        let mesh = |opts: &str| {
            let mut k = crate::Kernel::new();
            k.add_primitive(
                Primitive::KIND_CYLINDER,
                BoolOp::TAG_NEW,
                0.0,
                0.0,
                0.0,
                10.0,
                15.0,
                0.0,
            )
            .unwrap();
            k.mesher_points(opts.into())
        };
        // Isotropic, fine enough to meet sag ≈ 0.02 on r=10 (h ≈ 1.5).
        let iso = mesh(
            r#"{"maxDepth":5,"brepSkeleton":true,"cdtFaces":true,
                "nearSurfaceOnly":true,"nearSurfaceDist":1.5,
                "fitOnSurface":true,"onlyOnSurface":true,
                "repulsion":true,"interactionRadius":1.5,"repulsionStrength":0.5,
                "pasmKernel":true,"iterations":16}"#,
        );
        // Anisotropic: coarse axial edge length h=4, same sag → circumference
        // refines, axis stays coarse.
        let aniso = mesh(
            r#"{"maxDepth":5,"brepSkeleton":true,"cdtFaces":true,
                "nearSurfaceOnly":true,"nearSurfaceDist":4.0,
                "fitOnSurface":true,"onlyOnSurface":true,
                "repulsion":true,"interactionRadius":4.0,"repulsionStrength":0.5,
                "pasmKernel":true,"iterations":16,
                "anisotropy":true,"anisoSag":0.02,"anisoSmax":8.0}"#,
        );
        let (iso_tris, iso_wt, iso_euler) = mesh_stats(&iso);
        let (an_tris, an_wt, an_euler) = mesh_stats(&aniso);
        assert!(iso_wt, "isotropic cylinder not watertight");
        assert!(an_wt, "anisotropic cylinder not watertight");
        assert_eq!(an_euler, 2, "anisotropic cylinder Euler broke: {an_euler}");
        assert_eq!(iso_euler, 2, "isotropic cylinder Euler broke: {iso_euler}");
        assert!(
            an_tris < iso_tris,
            "anisotropy should cut triangles at equal sag: aniso={an_tris} iso={iso_tris}"
        );
    }

    /// Hybrid-PASM §6 F3 (ticket nwmi): a TORUS ∩ y-slab exercises the
    /// CHARTLESS fallback — `HalfSpace::Torus` has no chart in the CDT
    /// dispatch, so both torus bands (outer + inner) are meshed as octree
    /// DC patches retreated from the rim circles and ZIPPED to the exact
    /// skeleton chains, while the two planar annuli take the CDT path. The
    /// mixed mesh must still be closed and 2-manifold; the solid is a
    /// (topological) torus, so Euler V − E + F = 0. Fallback triangles are
    /// reported in the header (`out[2]`) and sit last in the index block.
    #[test]
    fn mesher_points_fallback_torus_slab_is_watertight_manifold() {
        use fielddraft_protocol::{BoolOp, Primitive};
        let mut k = crate::Kernel::new();
        k.add_primitive(Primitive::KIND_TORUS, BoolOp::TAG_NEW, 0.0, 0.0, 0.0, 8.0, 3.0, 0.0)
            .unwrap();
        k.document = serde_json::from_str(
            &serde_json::json!({
                "schema_version": 2,
                "name": "torus-slab",
                "next_feature_id": 3,
                "feature_calls": [
                    { "type": "torus", "id": "1", "params": {
                        "major_radius": 8.0, "minor_radius": 3.0,
                        "translate": [0.0, 0.0, 0.0] } },
                    { "type": "box", "id": "2", "params": {
                        "half_extents": [20.0, 2.0, 20.0],
                        "translate": [0.0, 0.0, 0.0],
                        "op": "intersect", "targets": "qCreatedBy(1)" } }
                ]
            })
            .to_string(),
        )
        .unwrap();
        k.bootstrap_features_from_calls().unwrap();
        k.rebuild_packed();
        // F3 needs the chartless region to be a few cells WIDE at the chosen
        // resolution (patch minus clearance band): torus bands of ~4.4 mm arc
        // want h ≈ 1.2 — at h = 1.5 the inner band starves to a single cell
        // row and cannot form a patch.
        let out = k.mesher_points(
            r#"{"maxDepth":6,"brepSkeleton":true,"cdtFaces":true,"fallbackPatches":true,
                "nearSurfaceOnly":true,"nearSurfaceDist":1.0,
                "fitOnSurface":true,"onlyOnSurface":true,
                "repulsion":true,"interactionRadius":1.2,"repulsionStrength":0.5,
                "pasmKernel":true,"iterations":20}"#
                .into(),
        );
        let total = out[0] as usize;
        let tris = out[1] as usize;
        let fallback = out[2] as usize;
        assert!(tris >= 16, "expected a real triangulation, got {tris} tris");
        assert!(
            fallback > 0 && fallback < tris,
            "expected a MIX of CDT and fallback triangles, got {fallback}/{tris}"
        );
        let idx_base = 3 + total * 9;
        let mut edge_use: std::collections::HashMap<(u32, u32), u32> =
            std::collections::HashMap::new();
        let mut used: std::collections::HashSet<u32> = std::collections::HashSet::new();
        for t in 0..tris {
            let a = out[idx_base + t * 3] as u32;
            let b = out[idx_base + t * 3 + 1] as u32;
            let c3 = out[idx_base + t * 3 + 2] as u32;
            assert!(a != b && b != c3 && c3 != a, "degenerate triangle");
            for (x, y) in [(a, b), (b, c3), (c3, a)] {
                *edge_use.entry((x.min(y), x.max(y))).or_default() += 1;
                used.insert(x);
                used.insert(y);
            }
        }
        for (&(a, b), &n) in &edge_use {
            assert_eq!(n, 2, "edge {a}-{b} used {n}× (open seam or fan)");
        }
        let v = used.len() as i64;
        let e = edge_use.len() as i64;
        let f = tris as i64;
        assert_eq!(
            v - e + f,
            0,
            "torus Euler characteristic broke: V={v} E={e} F={f}"
        );
    }

    /// Hybrid-PASM P1 (ticket nwmi): with `brepSkeleton` on, a box body emits
    /// its 8 corners as pinned flag-3 particles EXACTLY at the corner
    /// positions plus arc-length-resampled flag-2 chains along the 12 edges.
    #[test]
    fn mesher_points_emits_brep_skeleton_for_a_box() {
        use fielddraft_protocol::{BoolOp, Primitive};
        let mut k = crate::Kernel::new();
        k.add_primitive(
            Primitive::KIND_BOX,
            BoolOp::TAG_NEW,
            0.0,
            0.0,
            0.0,
            10.0,
            10.0,
            10.0,
        )
        .unwrap();
        let out = k.mesher_points(
            r#"{"maxDepth":3,"brepSkeleton":true,"interactionRadius":2.0}"#.into(),
        );
        let total = out[0] as usize;
        let mut corners = Vec::new();
        let mut chain = 0usize;
        for i in 0..total {
            let b = 3 + i * 9;
            match out[b + 8] as i32 {
                3 => corners.push([out[b], out[b + 1], out[b + 2]]),
                2 => chain += 1,
                _ => {}
            }
        }
        assert_eq!(corners.len(), 8, "eight pinned box corners");
        for c in &corners {
            for a in 0..3 {
                assert!(
                    (c[a].abs() - 10.0).abs() < 1e-3,
                    "corner coordinate off ±10: {c:?}"
                );
            }
        }
        // 12 edges of length 20 at h=2 → 9 interior points each = 108.
        assert_eq!(chain, 12 * 9, "arc-length chains at the interaction radius");
    }
}
