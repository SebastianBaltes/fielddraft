//! wasm-bindgen-API des Meshers „Lichen" (`docs/Mesher-Konzept.md`).
//! Orchestriert die volle Pipeline: BRep-Feature-Seeds + Pro-Fläche-Rand-Loops
//! + Flächen-Klassifikator (harte Kanten-Wand) → Waben-Wachstum pro Fläche →
//! Pro-Fläche Constrained-Delaunay ([`crate::mesher_cdt`]). Rückgabe im flachen
//! Overlay-Format.

use std::collections::{HashMap, HashSet};

use wasm_bindgen::prelude::*;

use fielddraft_protocol::{HalfSpace, Transform};

use crate::Kernel;
use crate::brep::{EdgeId, FaceId, SurfaceGeometry};
use crate::lichen::{self, LPoint, LichenOptions};

#[wasm_bindgen]
impl Kernel {
    /// Mesher „Lichen": Seeds (BRep-Ecken/-Kanten) + Waben-Wachstum +
    /// Advancing-Front-Triangulierung. Rückgabe wie `mesherPoints`:
    /// `[nPoints, nTris, nFallbackTris, points×9, indices×3]`.
    #[wasm_bindgen(js_name = lichenMesh)]
    pub fn lichen_mesh(&mut self, options_json: String) -> Vec<f32> {
        let opts: LichenOptions = serde_json::from_str(&options_json).unwrap_or_default();
        self.lichen_mesh_impl(opts, None)
    }

    /// Kern (testbar, per-Body-fähig). `body_filter = None` = ganze Szene.
    pub(crate) fn lichen_mesh_impl(
        &mut self,
        opts: LichenOptions,
        body_filter: Option<usize>,
    ) -> Vec<f32> {
        let h = if opts.target_edge_length > 0.0 { opts.target_edge_length } else { 4.0 };

        // Welt-Felder pro Body (1:1 zu self.bodies, wie im PASM-Pfad).
        let bodies: Vec<crate::bvh::BoundedCsg> = self
            .bodies
            .iter()
            .map(|b| crate::body::build_bounded_with_primitives(&self.decoded, &b.feature_indices).0)
            .collect();
        let active: Vec<usize> = match body_filter {
            Some(i) if i < bodies.len() => vec![i],
            Some(_) => return Vec::new(),
            None => (0..bodies.len()).collect(),
        };
        if active.is_empty() {
            return vec![0.0, 0.0, 0.0];
        }

        // Szenen-Feld: (sdf, ∇f) des Body mit kleinstem Abstand (argmin).
        let fg = |p: [f32; 3]| -> (f32, [f32; 3]) {
            let mut bd = f32::MAX;
            let mut bg = [0.0f32, 0.0, 1.0];
            for &bi in &active {
                let (d, g) = crate::sdf_dual::sdf_gradient(&bodies[bi], p);
                if d < bd {
                    bd = d;
                    bg = g;
                }
            }
            (bd, bg)
        };

        // ── Seeds (§1) + Pro-Fläche-Rand-Loops + Flächen-Klassifikator ──
        // `all` = alle Punkte (parallel dazu `point_faces[i]` = die Flächen-IDs
        // (+ Normalen) von Punkt i, `local_pos[i]` = seine Body-Local-Koords).
        // Endpunkte teilen sich die Ecken-Slots → feste `chain_links` (BRep-
        // Kanten in G) + `edge_segs` (Welt-Segmente für R_BRep).
        //
        // Owned Flächendaten für den Welt→Fläche-Distanz-Klassifikator (die harte
        // Kanten-Wand): `argmin_gf |sdf(face_gf.surface, ·)|`.
        struct FaceSurf {
            surface: HalfSpace,
            transform: Transform,
            placement: Transform,
        }
        let mut all: Vec<LPoint> = Vec::new();
        let mut point_faces: Vec<Vec<(u32, [f32; 3])>> = Vec::new();
        let mut local_pos: Vec<[f32; 3]> = Vec::new();
        let mut edge_segs: Vec<([f32; 3], [f32; 3])> = Vec::new();
        let mut face_surfs: Vec<FaceSurf> = Vec::new();
        let mut face_loops: Vec<Vec<(Vec<u32>, bool)>> = Vec::new();

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

            // Ecken (flag 3): globale Slots; Normale (Mittel der Flächen) folgt.
            let mut corner_slot: HashMap<usize, u32> = HashMap::new();
            for (vi, v) in brep.vertices.iter().enumerate() {
                let p = to_world(v.position);
                let (d, _) = fg(p);
                corner_slot.insert(vi, all.len() as u32);
                all.push(LPoint { p, sdf: d, n: [0.0, 0.0, 1.0], u: h, space: h, flag: 3.0 });
                point_faces.push(Vec::new());
                local_pos.push([v.position[0] as f32, v.position[1] as f32, v.position[2] as f32]);
            }

            // Kanten (flag 2): in LOCAL resamplen (§3.2 krümmungsgesteuert);
            // geordnete globale Index-Kette je Kante merken (für die Flächen-
            // Loops); chain_links = feste Feature-Kanten in G; Welt-Segmente für
            // R_BRep.
            let h_local = h / s.max(1e-6);
            let min_spacing_local = opts.min_edge_spacing.max(0.0) / s.max(1e-6);
            let seg_len = |a: [f32; 3], b: [f32; 3]| -> f32 {
                ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2) + (b[2] - a[2]).powi(2)).sqrt()
            };
            // Kette einer Kante bauen: adaptives Resampling (mit Ecken-Rand-
            // Bedingungen fürs Grading) + 4-Punkte-Schutz für Ringe.
            let build_chain = |raw_local: &[[f32; 3]], closed: bool, bounds: (f32, f32)| {
                let mut chain = lichen::resample_edge_by_curvature(
                    raw_local,
                    h_local,
                    opts.curv_angle_deg.to_radians(),
                    min_spacing_local,
                    bounds,
                );
                // Degenerations-Schutz (Ticket 6d3f4a7f): eine geschlossene
                // Kante braucht ≥4 Punkte, egal wie klein sie ist — sonst
                // entartet der Ring. Uniform bei Viertel-Bogenlänge neu
                // abtasten (Ende = Start-Duplikat, wird unten getrimmt).
                if closed && chain.len() < 5 && raw_local.len() >= 2 {
                    let total: f32 = raw_local.windows(2).map(|w| seg_len(w[0], w[1])).sum();
                    if total > 1e-9 {
                        chain = crate::mesher_points::resample_polyline(raw_local, total / 4.0);
                    }
                }
                chain
            };

            // Pass A (Ticket cec70614): End-Abstände aller an einer Ecke
            // mündenden Ketten sammeln — das kleinste wird die Grading-Rand-
            // Bedingung der Nachbarkanten in Pass B. Ohne diese Kopplung
            // startet eine gerade Kante an der Ecke mit ~h, während U(Ecke)
            // vom dichtesten Nachbarn stammt: die U-Kreise von Ecke und
            // erstem Kettenpunkt berühren sich dann nicht mehr.
            struct PEdge {
                raw_local: Vec<[f32; 3]>,
                closed: bool,
                vs: Option<usize>,
                ve: Option<usize>,
            }
            let mut pedges: Vec<PEdge> = Vec::with_capacity(brep.edges.len());
            let mut corner_gap: HashMap<usize, f32> = HashMap::new();
            for e in brep.edges.iter() {
                let raw_local: Vec<[f32; 3]> = crate::brep::edge_sampling::sample_edge(&e.geometry)
                    .into_iter()
                    .map(|p| [p[0] as f32, p[1] as f32, p[2] as f32])
                    .collect();
                let closed = e.start.is_none() && e.end.is_none();
                let chain =
                    build_chain(&raw_local, closed, (f32::INFINITY, f32::INFINITY));
                let mut note = |v: Option<crate::brep::VertexId>, d: f32| {
                    if let Some(vid) = v {
                        let slot = corner_gap.entry(vid.0 as usize).or_insert(f32::INFINITY);
                        *slot = slot.min(d);
                    }
                };
                if chain.len() >= 2 {
                    note(e.start, seg_len(chain[0], chain[1]));
                    note(e.end, seg_len(chain[chain.len() - 2], chain[chain.len() - 1]));
                }
                pedges.push(PEdge {
                    raw_local,
                    closed,
                    vs: e.start.map(|v| v.0 as usize),
                    ve: e.end.map(|v| v.0 as usize),
                });
            }

            let mut edge_chain: HashMap<u32, Vec<u32>> = HashMap::new();
            for (eid, (e, pe)) in brep.edges.iter().zip(&pedges).enumerate() {
                let bound_of = |v: Option<usize>| -> f32 {
                    v.and_then(|vid| corner_gap.get(&vid).copied()).unwrap_or(f32::INFINITY)
                };
                let closed = pe.closed;
                let chain = build_chain(
                    &pe.raw_local,
                    closed,
                    (bound_of(pe.vs), bound_of(pe.ve)),
                );
                let mut idxs: Vec<u32> = Vec::new();
                if let Some(vs) = e.start {
                    idxs.push(corner_slot[&(vs.0 as usize)]);
                }
                let start = usize::from(e.start.is_some());
                let mut end = chain.len().saturating_sub(usize::from(e.end.is_some()));
                if e.start.is_none() && e.end.is_none() && chain.len() >= 2 {
                    end = end.saturating_sub(1);
                }
                let lo = start.min(chain.len());
                if lo < end {
                    for pl in &chain[lo..end] {
                        let p = to_world([pl[0] as f64, pl[1] as f64, pl[2] as f64]);
                        let (d, _) = fg(p);
                        idxs.push(all.len() as u32);
                        all.push(LPoint { p, sdf: d, n: [0.0, 0.0, 1.0], u: h, space: h, flag: 2.0 });
                        point_faces.push(Vec::new());
                        local_pos.push(*pl);
                    }
                }
                if let Some(ve) = e.end {
                    idxs.push(corner_slot[&(ve.0 as usize)]);
                }
                // Nachbar-Segmente: R_BRep-Distanzquelle UND U(p)-Deckel
                // (Ticket 0d94273b) — der Anzeige-Kreis eines Kantenpunkts
                // reicht höchstens bis zum nächsten Ketten-Nachbarn. Bei
                // geschlossenen Ringen zählt auch das Schluss-Segment
                // letzter→erster Punkt.
                let mut bound_pair = |ia: u32, ib: u32, all: &mut Vec<LPoint>| {
                    let (pa, pb) = (all[ia as usize].p, all[ib as usize].p);
                    let d = ((pa[0] - pb[0]).powi(2)
                        + (pa[1] - pb[1]).powi(2)
                        + (pa[2] - pb[2]).powi(2))
                    .sqrt();
                    edge_segs.push((pa, pb));
                    all[ia as usize].u = all[ia as usize].u.min(d);
                    all[ib as usize].u = all[ib as usize].u.min(d);
                };
                for w in idxs.windows(2).map(|w| [w[0], w[1]]).collect::<Vec<_>>() {
                    bound_pair(w[0], w[1], &mut all);
                }
                if closed && idxs.len() >= 2 {
                    bound_pair(idxs[idxs.len() - 1], idxs[0], &mut all);
                }
                edge_chain.insert(eid as u32, idxs);
            }

            // Flächen: gf zuweisen, Klassifikator-Daten sichern, Rand-Loops als
            // geordnete globale Index-Ringe bauen (Kanten-Ketten in Loop-Reihen-
            // folge verkettet, geteilte Gelenk-Vertices entfernt), und an jedem
            // eindeutigen Rand-Punkt die Pro-Fläche-Normale registrieren.
            let build_ring = |lp: &crate::brep::Loop| -> Vec<u32> {
                let mut ring: Vec<u32> = Vec::new();
                for &(EdgeId(ei), reversed) in &lp.edges {
                    let Some(chain) = edge_chain.get(&ei) else { continue };
                    let mut chain = chain.clone();
                    if reversed {
                        chain.reverse();
                    }
                    for gidx in chain {
                        if ring.last() == Some(&gidx) {
                            continue; // gemeinsamer Gelenk-Vertex
                        }
                        ring.push(gidx);
                    }
                }
                if ring.len() >= 2 && ring.first() == ring.last() {
                    ring.pop();
                }
                ring
            };
            for (fi, face) in brep.faces.iter().enumerate() {
                let SurfaceGeometry::Analytic { surface, transform } = &face.surface else {
                    continue; // tessellierte Flächen (Gyroid) hier ausgelassen
                };
                let gf = face_surfs.len() as u32;
                face_surfs.push(FaceSurf {
                    surface: surface.clone(),
                    transform: *transform,
                    placement,
                });
                let fid = FaceId(fi as u32);
                let mut rings: Vec<(Vec<u32>, bool)> = Vec::new();
                let outer = build_ring(&face.outer);
                if outer.len() >= 3 {
                    rings.push((outer, true));
                }
                for il in &face.inner {
                    let r = build_ring(il);
                    if r.len() >= 3 {
                        rings.push((r, false));
                    }
                }
                let mut seen: HashSet<u32> = HashSet::new();
                for (ring, _) in &rings {
                    for &gidx in ring {
                        if !seen.insert(gidx) {
                            continue;
                        }
                        let pl = local_pos[gidx as usize];
                        let nl = crate::brep::face_export::face_normal_at_local(
                            brep,
                            fid,
                            [pl[0] as f64, pl[1] as f64, pl[2] as f64],
                        );
                        let nw = lichen::unit(crate::quat_rotate(
                            placement.rotation,
                            [nl[0] as f32, nl[1] as f32, nl[2] as f32],
                        ));
                        point_faces[gidx as usize].push((gf, nw));
                    }
                }
                face_loops.push(rings);
            }
        }

        // Welt→Fläche-Klassifikator (harte Kanten-Wand): die BRep-Fläche mit
        // kleinstem |Flächen-SDF| am Punkt; `None`, wenn zu weit von allen
        // Flächen (Blend-Region) — dorthin wächst nichts.
        let face_of = |p_world: [f32; 3]| -> Option<u32> {
            let mut best = u32::MAX;
            let mut bestd = f32::MAX;
            for (gf, fs) in face_surfs.iter().enumerate() {
                let p_local = crate::world_to_local(&fs.placement, p_world);
                let p_surf = crate::world_to_local(&fs.transform, p_local);
                let d = crate::csg::sdf_halfspace(&fs.surface, p_surf).abs();
                if d < bestd {
                    bestd = d;
                    best = gf as u32;
                }
            }
            if best != u32::MAX && bestd < 0.5 * h { Some(best) } else { None }
        };

        // R_BRep (§2.2): echter Punkt-zu-Segment-Abstand zur nächsten BRep-Kante
        // (NICHT nur zum nächsten Abtastpunkt) — so bleibt der U(p)-Kreis stets
        // innerhalb der Fläche und überschreitet nie eine Kante.
        let brep_dist = |p: [f32; 3]| -> f32 {
            let mut best = f32::INFINITY;
            for &(a, b) in &edge_segs {
                let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                let ap = [p[0] - a[0], p[1] - a[1], p[2] - a[2]];
                let denom = ab[0] * ab[0] + ab[1] * ab[1] + ab[2] * ab[2];
                let t = if denom > 1e-20 {
                    ((ap[0] * ab[0] + ap[1] * ab[1] + ap[2] * ab[2]) / denom).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let q = [a[0] + ab[0] * t, a[1] + ab[1] * t, a[2] + ab[2] * t];
                let dd = ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2)).sqrt();
                if dd < best {
                    best = dd;
                }
            }
            best
        };

        // ── Wachstum (§1) pro Fläche, mit harter Kanten-Wand ──
        // Gewachsene Innen-Partikel je Fläche (für den Pro-Fläche-CDT).
        let mut face_interior: Vec<Vec<u32>> = vec![Vec::new(); face_surfs.len()];
        if opts.grow {
            let n_faces = face_surfs.len() as u32;
            let gf_normal = |pf: &[(u32, [f32; 3])], gf: u32| -> [f32; 3] {
                pf.iter().find(|&&(g, _)| g == gf).map(|&(_, nrm)| nrm).unwrap_or([0.0, 0.0, 1.0])
            };
            for gf in 0..n_faces {
                // Rand-Punkte dieser Fläche als Wachstums-Seeds (mit gf-Normale,
                // damit die Hex-Front in die Fläche zeigt, nicht diagonal).
                let mut seen: HashSet<u32> = HashSet::new();
                let mut bseeds: Vec<LPoint> = Vec::new();
                for (ring, _) in &face_loops[gf as usize] {
                    for &gidx in ring {
                        if !seen.insert(gidx) {
                            continue;
                        }
                        let src = all[gidx as usize];
                        bseeds.push(LPoint {
                            p: src.p,
                            sdf: src.sdf,
                            n: gf_normal(&point_faces[gidx as usize], gf),
                            u: h,
                            space: h,
                            flag: src.flag,
                        });
                    }
                }
                if bseeds.is_empty() {
                    continue;
                }
                let on_face = |p: [f32; 3]| face_of(p) == Some(gf);
                let grown = lichen::grow_surface(&bseeds, h, &opts, &fg, &brep_dist, &on_face);
                for g in grown {
                    let idx = all.len() as u32;
                    all.push(g);
                    point_faces.push(vec![(gf, g.n)]);
                    local_pos.push([0.0; 3]);
                    face_interior[gf as usize].push(idx);
                }
            }
        }

        // Rand-Normale (LPoint.n) = Mittel der angrenzenden Flächen-Normalen
        // (STABIL an Kanten/Ecken; z.B. Box-Ecke = ±1/√3-Diagonale); gewachsene
        // Punkte behalten ihr ∇f.
        for i in 0..all.len() {
            if all[i].flag >= 1.5 && !point_faces[i].is_empty() {
                let ns: Vec<[f32; 3]> = point_faces[i].iter().map(|&(_, nrm)| nrm).collect();
                if let Some(m) = lichen::mean_unit(&ns) {
                    all[i].n = m;
                }
            }
        }

        // ── Pro-Fläche-Triangulierung (§4/§5): robuster Constrained-Delaunay je
        // BRep-Fläche (spade, `mesher_cdt`). Rand-Ketten sind flächenübergreifend
        // geteilt → wasserdichte Nähte; Innen-Partikel werden auf die Fläche
        // gefiltert; kein Dreieck überspannt eine Kante (harte Grenze). Planare
        // Flächen (Box) im Newell-Frame; gekrümmte analytische Flächen
        // (Zylinder/Kugel/Kegel) über den PERIODISCHEN CHART (M4) — dieselbe
        // `triangulate_periodic_face`-Mechanik wie der Hybrid-PASM-Export (P4):
        // Band-Schnitt mit index-geteilter Naht, Pol-Kappen via Ring + Fächer.
        // Torus (doppelt periodisch) und Gyroid (chartlos, F2) bleiben
        // vorerst untrianguliert. ──
        let tris = if opts.triangulate && all.len() >= 3 {
            let mut t: Vec<[u32; 3]> = Vec::new();
            for gf in 0..face_surfs.len() {
                let loops: Vec<Vec<u32>> =
                    face_loops[gf].iter().map(|(r, _)| r.clone()).collect();
                if matches!(face_surfs[gf].surface, HalfSpace::Plane { .. }) {
                    let positions: Vec<[f32; 3]> = all.iter().map(|lp| lp.p).collect();
                    t.extend(crate::mesher_cdt::triangulate_planar_face(
                        &positions,
                        &loops,
                        &face_interior[gf],
                        0.25 * h as f64,
                    ));
                    continue;
                }
                let fs = &face_surfs[gf];
                // Chart-Frame: Fläche lokal → Welt = placement ∘ transform
                // (exakt die Kette des `face_of`-Klassifikators).
                let total = crate::compose_transforms(&fs.placement, &fs.transform);
                // (u, v) roh (Winkel/axial) je Trägerfläche + Bogenlängen-
                // Skalen (su für θ, sv für v) und Pol-Positionen im Chart.
                let uv_raw = |p_world: [f32; 3]| -> Option<[f64; 2]> {
                    let l = crate::world_to_local(&total, p_world);
                    match &fs.surface {
                        HalfSpace::InfiniteCylinder { .. } | HalfSpace::Cone { .. } => {
                            Some([(l[2] as f64).atan2(l[0] as f64), l[1] as f64])
                        }
                        HalfSpace::Sphere { .. } => {
                            let r = ((l[0] * l[0] + l[1] * l[1] + l[2] * l[2]) as f64)
                                .sqrt()
                                .max(1e-9);
                            Some([
                                (l[2] as f64).atan2(l[0] as f64),
                                (l[1] as f64 / r).clamp(-1.0, 1.0).acos(),
                            ])
                        }
                        _ => None,
                    }
                };
                let mut raw_loops: Vec<Vec<crate::mesher_cdt::UvPoint>> = Vec::new();
                let mut chart_ok = !loops.is_empty();
                'lp: for ring in &loops {
                    let mut lp = Vec::with_capacity(ring.len());
                    for &gi in ring {
                        match uv_raw(all[gi as usize].p) {
                            Some(uv) => lp.push((gi, uv)),
                            None => {
                                chart_ok = false;
                                break 'lp;
                            }
                        }
                    }
                    raw_loops.push(lp);
                }
                if !chart_ok {
                    continue; // Torus/Gyroid: chartlos (F2) — bleibt Punktwolke
                }
                // θ (und Kugel-Polarwinkel) auf Bogenlänge skalieren, damit
                // beide Chart-Achsen vergleichbare Einheiten tragen.
                let (su, sv) = match &fs.surface {
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
                    _ => (1.0, 1.0),
                };
                let period = std::f64::consts::TAU * su;
                let scaled: Vec<Vec<crate::mesher_cdt::UvPoint>> = raw_loops
                    .iter()
                    .map(|lp| lp.iter().map(|&(gi, [u, v])| (gi, [u * su, v * sv])).collect())
                    .collect();
                let scaled_interior: Vec<crate::mesher_cdt::UvPoint> = face_interior[gf]
                    .iter()
                    .filter_map(|&gi| {
                        uv_raw(all[gi as usize].p).map(|[u, v]| (gi, [u * su, v * sv]))
                    })
                    .collect();
                let surface = fs.surface.clone();
                let world_of = |u: f64, v: f64| -> [f32; 3] {
                    let (th, tv) = (u / su, v / sv);
                    let local = match &surface {
                        HalfSpace::InfiniteCylinder { radius } => {
                            let r = *radius as f64;
                            [(r * th.cos()) as f32, tv as f32, (r * th.sin()) as f32]
                        }
                        HalfSpace::Cone { half_angle } => {
                            let r = tv.abs() * (*half_angle as f64).tan();
                            [(r * th.cos()) as f32, tv as f32, (r * th.sin()) as f32]
                        }
                        HalfSpace::Sphere { radius } => {
                            let r = *radius as f64;
                            let sp = tv.sin();
                            [
                                (r * sp * th.cos()) as f32,
                                (r * tv.cos()) as f32,
                                (r * sp * th.sin()) as f32,
                            ]
                        }
                        _ => [0.0; 3],
                    };
                    let sc = [
                        local[0] * total.scale[0],
                        local[1] * total.scale[1],
                        local[2] * total.scale[2],
                    ];
                    let r = crate::quat_rotate(total.rotation, sc);
                    [
                        r[0] + total.translate[0],
                        r[1] + total.translate[1],
                        r[2] + total.translate[2],
                    ]
                };
                let poles: Vec<f64> = match &fs.surface {
                    HalfSpace::Sphere { .. } => vec![0.0, std::f64::consts::PI * sv],
                    HalfSpace::Cone { .. } => vec![0.0],
                    _ => Vec::new(),
                };
                // Naht-/Clearance-Spacing von Welt-mm in Chart-Einheiten.
                let ms = total.scale[0]
                    .abs()
                    .max(total.scale[1].abs())
                    .max(total.scale[2].abs())
                    .max(1e-6);
                let h_chart = (h / ms) as f64;
                if let Some(mesh) = crate::mesher_cdt::triangulate_periodic_face(
                    &scaled,
                    &scaled_interior,
                    period,
                    h_chart,
                    0.25 * h_chart,
                    &poles,
                    all.len() as u32,
                    &world_of,
                ) {
                    t.extend(mesh.tris);
                    // Naht-Steiner-/Pol-Punkte als neue Flächenpunkte anhängen
                    // (flag 1); Normale aus ∇f.
                    for p in mesh.aux {
                        let (d, g) = fg(p);
                        let n = lichen::unit(g);
                        all.push(LPoint { p, sdf: d, n, u: h, space: h, flag: 1.0 });
                        point_faces.push(vec![(gf as u32, n)]);
                        local_pos.push([0.0; 3]);
                    }
                }
            }
            t
        } else {
            Vec::new()
        };

        lichen::pack(&all, &tris, &point_faces)
    }
}

#[cfg(test)]
mod tests {
    use fielddraft_protocol::{BoolOp, Primitive};

    fn box_kernel() -> crate::Kernel {
        let mut k = crate::Kernel::new();
        k.add_primitive(Primitive::KIND_BOX, BoolOp::TAG_NEW, 0.0, 0.0, 0.0, 10.0, 10.0, 10.0)
            .unwrap();
        k
    }

    /// Full pipeline runs on a real body — seeds, growth, graph, advancing-front
    /// — and emits the box corners, edge-chain seeds, grown points and triangles.
    #[test]
    fn lichen_box_emits_seeds_growth_and_triangles() {
        let mut k = box_kernel();
        let out = k.lichen_mesh(
            r#"{"targetEdgeLength":3.0,"grow":true,"maxPoints":8000}"#.into(),
        );
        assert!(out.len() >= 3, "header present");
        let total = out[0] as usize;
        let tris = out[1] as usize;
        assert!(tris > 0, "advancing front emits real triangles");
        // Layout: header(3) + points×9 + indices×3 + nFaceRec + faceRec×4.
        let idx_end = 3 + total * 9 + tris * 3;
        let n_face = out[idx_end] as usize;
        assert_eq!(
            idx_end + 1 + n_face * 4,
            out.len(),
            "flat layout: points + index + face-normal block"
        );
        assert!(n_face >= total, "at least one circle record per point (boundary points ≥2)");
        let (mut corners, mut chain, mut surf) = (0u32, 0u32, 0u32);
        for i in 0..total {
            match out[3 + i * 9 + 8] as i32 {
                3 => corners += 1,
                2 => chain += 1,
                1 => surf += 1,
                _ => {}
            }
        }
        assert_eq!(corners, 8, "eight pinned box corners");
        assert!(chain > 0 && surf > 0, "edge chains + grown surface points");
    }

    /// The box mesh is a CLOSED, watertight 2-manifold: no edge borders more
    /// than two triangles, NO boundary edge exists at all (every edge is shared
    /// by exactly two triangles — the per-face CDT welds along the shared BRep
    /// edge chains), and the Euler characteristic of the used vertices is 2
    /// (genus-0 sphere topology, no holes). This is the core §4/§5 guarantee
    /// and the regression gate for the hole-closer.
    #[test]
    fn lichen_box_mesh_is_watertight() {
        let mut k = box_kernel();
        let out = k.lichen_mesh(
            r#"{"targetEdgeLength":3.0,"grow":true,"maxPoints":8000}"#.into(),
        );
        let total = out[0] as usize;
        let tris = out[1] as usize;
        assert!(tris > 0);
        let base = 3 + total * 9;
        let mut edge_use: std::collections::HashMap<(u32, u32), u32> =
            std::collections::HashMap::new();
        let mut used_v: std::collections::HashSet<u32> = std::collections::HashSet::new();
        for t in 0..tris {
            let a = out[base + t * 3] as u32;
            let b = out[base + t * 3 + 1] as u32;
            let c = out[base + t * 3 + 2] as u32;
            assert!(a != b && b != c && c != a, "degenerate triangle");
            used_v.insert(a);
            used_v.insert(b);
            used_v.insert(c);
            for (x, y) in [(a, b), (b, c), (c, a)] {
                *edge_use.entry((x.min(y), x.max(y))).or_default() += 1;
            }
        }
        let over = edge_use.values().filter(|&&u| u > 2).count();
        assert_eq!(over, 0, "no edge may border >2 triangles (2-manifold)");
        let interior = edge_use.values().filter(|&&u| u == 2).count();
        let boundary = edge_use.values().filter(|&&u| u == 1).count();
        assert_eq!(boundary, 0, "watertight: NO boundary/hole edges (got {boundary})");
        // Euler characteristic on the used vertices: V − E + F = 2 for a closed
        // genus-0 surface (E = interior edges, since boundary == 0).
        let euler = used_v.len() as i64 - interior as i64 + tris as i64;
        assert_eq!(euler, 2, "closed genus-0 mesh: V−E+F must be 2, got {euler}");
    }

    /// The mesher must NEVER emit a degenerate triangle: distinct vertices,
    /// non-zero area, and shape quality above the degeneracy threshold.
    #[test]
    fn lichen_box_has_no_degenerate_triangles() {
        let mut k = box_kernel();
        let out = k.lichen_mesh(
            r#"{"targetEdgeLength":3.0,"grow":true,"maxPoints":8000}"#.into(),
        );
        let total = out[0] as usize;
        let tris = out[1] as usize;
        assert!(tris > 0);
        let base = 3 + total * 9;
        let pos = |i: usize| [out[3 + i * 9], out[3 + i * 9 + 1], out[3 + i * 9 + 2]];
        let mut worst = f32::MAX;
        for t in 0..tris {
            let (ia, ib, ic) = (
                out[base + t * 3] as usize,
                out[base + t * 3 + 1] as usize,
                out[base + t * 3 + 2] as usize,
            );
            assert!(ia != ib && ib != ic && ic != ia, "coincident vertex indices");
            let (a, b, c) = (pos(ia), pos(ib), pos(ic));
            let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let cr = [
                ab[1] * ac[2] - ab[2] * ac[1],
                ab[2] * ac[0] - ab[0] * ac[2],
                ab[0] * ac[1] - ab[1] * ac[0],
            ];
            let area = 0.5 * (cr[0] * cr[0] + cr[1] * cr[1] + cr[2] * cr[2]).sqrt();
            assert!(area > 1e-6, "zero-area (degenerate) triangle {t}");
            let bc = [c[0] - b[0], c[1] - b[1], c[2] - b[2]];
            let s = ab.iter().map(|v| v * v).sum::<f32>()
                + ac.iter().map(|v| v * v).sum::<f32>()
                + bc.iter().map(|v| v * v).sum::<f32>();
            worst = worst.min(4.0 * (3.0f32).sqrt() * area / s);
        }
        assert!(worst > 0.02, "no degenerate slivers: worst shape quality {worst}");
    }

    /// Watertightness/manifold check over a packed lichen result.
    fn assert_watertight(out: &[f32]) {
        let total = out[0] as usize;
        let tris = out[1] as usize;
        assert!(tris > 0, "mesh has triangles");
        let base = 3 + total * 9;
        let mut edge_use: std::collections::HashMap<(u32, u32), u32> =
            std::collections::HashMap::new();
        let mut used_v: std::collections::HashSet<u32> = std::collections::HashSet::new();
        for t in 0..tris {
            let a = out[base + t * 3] as u32;
            let b = out[base + t * 3 + 1] as u32;
            let c = out[base + t * 3 + 2] as u32;
            assert!(a != b && b != c && c != a, "degenerate triangle");
            used_v.extend([a, b, c]);
            for (x, y) in [(a, b), (b, c), (c, a)] {
                *edge_use.entry((x.min(y), x.max(y))).or_default() += 1;
            }
        }
        let over = edge_use.values().filter(|&&u| u > 2).count();
        assert_eq!(over, 0, "no edge may border >2 triangles (2-manifold)");
        let interior = edge_use.values().filter(|&&u| u == 2).count();
        let boundary = edge_use.values().filter(|&&u| u == 1).count();
        assert_eq!(boundary, 0, "watertight: NO boundary/hole edges (got {boundary})");
        let euler = used_v.len() as i64 - interior as i64 + tris as i64;
        assert_eq!(euler, 2, "closed genus-0 mesh: V−E+F must be 2, got {euler}");
    }

    /// M4 (periodischer Chart): ein Zylinder — Mantel-Band (zwei windende
    /// Rim-Ringe, Naht-Schnitt mit geteilten Steiner-Indizes) + zwei planare
    /// Kappen — vermascht zu einem GESCHLOSSENEN 2-Manifold.
    #[test]
    fn lichen_cylinder_mesh_is_watertight() {
        let mut k = crate::Kernel::new();
        k.add_primitive(
            Primitive::KIND_CYLINDER,
            BoolOp::TAG_NEW,
            0.0,
            0.0,
            0.0,
            8.0,
            10.0,
            0.0,
        )
        .unwrap();
        let out = k.lichen_mesh(
            r#"{"targetEdgeLength":3.0,"grow":true,"maxPoints":20000}"#.into(),
        );
        assert_watertight(&out);
    }

    /// M4 Pol-Kappe: eine Halbkugel (Kugel − Box) — die Kugelfläche ist eine
    /// Kappe (EIN windender Loop) und schließt über Pol-Ring + Fächer; die
    /// Schnittebene ist eine planare Scheibe. Ergebnis: geschlossen.
    #[test]
    fn lichen_hemisphere_mesh_is_watertight() {
        let mut k = crate::Kernel::new();
        k.add_primitive(Primitive::KIND_SPHERE, BoolOp::TAG_NEW, 0.0, 0.0, 0.0, 10.0, 0.0, 0.0)
            .unwrap();
        k.add_primitive(
            Primitive::KIND_BOX,
            BoolOp::TAG_REMOVE,
            0.0,
            -20.0,
            0.0,
            25.0,
            20.0,
            25.0,
        )
        .unwrap();
        let out = k.lichen_mesh(
            r#"{"targetEdgeLength":2.5,"grow":true,"maxPoints":20000}"#.into(),
        );
        assert_watertight(&out);
    }

    /// Interior surface points must grow whether or not the reach term is on
    /// (ticket: "use reach off ⇒ no interior points" was a bug).
    #[test]
    fn lichen_grows_interior_without_reach() {
        let mut k = box_kernel();
        let out = k.lichen_mesh(
            r#"{"targetEdgeLength":10.0,"grow":true,"curvAngleDeg":30.0,"useReach":false,"maxPoints":50000}"#.into(),
        );
        let total = out[0] as usize;
        let surf = (0..total).filter(|&i| out[3 + i * 9 + 8] as i32 == 1).count();
        assert!(surf > 0, "interior points must grow without reach, got {surf}");
    }

    /// `grow=false` (seeds only): no interior growth.
    #[test]
    fn lichen_seeds_only_when_grow_off() {
        let mut k = box_kernel();
        let out = k.lichen_mesh(r#"{"targetEdgeLength":3.0,"grow":false}"#.into());
        let total = out[0] as usize;
        let surf = (0..total).filter(|&i| out[3 + i * 9 + 8] as i32 == 1).count();
        assert_eq!(surf, 0, "grow=false ⇒ no interior growth");
        assert!(total >= 8, "seeds still present");
    }

    /// The curvature angle (rad) drives the BOUNDARY subdivision (§3.2): a
    /// curved edge is denser for a smaller angle, while a straight edge only
    /// feels the h cap.
    #[test]
    fn edge_subdivision_is_curvature_driven() {
        // Halbkreis-Polyline (Radius 10).
        let arc: Vec<[f32; 3]> = (0..=64)
            .map(|k| {
                let t = k as f32 / 64.0 * std::f32::consts::PI;
                [10.0 * t.cos(), 10.0 * t.sin(), 0.0]
            })
            .collect();
        let fine = crate::lichen::resample_edge_by_curvature(&arc, 5.0, 0.05, 0.01, (f32::INFINITY, f32::INFINITY));
        let coarse = crate::lichen::resample_edge_by_curvature(&arc, 5.0, 2.0, 0.01, (f32::INFINITY, f32::INFINITY));
        assert!(
            fine.len() > coarse.len(),
            "small angle ⇒ denser on curves: {} vs {}",
            fine.len(),
            coarse.len()
        );
        // Gerade Kante: Krümmungswinkel irrelevant, nur h zählt.
        let line = vec![[0.0, 0.0, 0.0], [20.0, 0.0, 0.0]];
        let a = crate::lichen::resample_edge_by_curvature(&line, 5.0, 0.05, 0.01, (f32::INFINITY, f32::INFINITY));
        let b = crate::lichen::resample_edge_by_curvature(&line, 5.0, 2.0, 0.01, (f32::INFINITY, f32::INFINITY));
        assert_eq!(a.len(), b.len(), "straight edge: angle has no effect");
    }

    /// Ticket-Regression: eine KLEINE, stark gekrümmte Kante (Kreis r=1,
    /// Umfang ~6 mm) bei großem h (10) und kleinem Winkel muss DICHT bleiben.
    /// Das frühere uniforme Pre-Sampling mit Schritt 0.15·h kollabierte den
    /// Kreis auf ~4 Punkte, bevor der Winkel-Test greifen konnte.
    #[test]
    fn small_curved_edge_stays_dense_under_large_h() {
        let n = 128usize;
        let circle: Vec<[f32; 3]> = (0..=n)
            .map(|k| {
                let t = k as f32 / n as f32 * std::f32::consts::TAU;
                [t.cos(), t.sin(), 0.0]
            })
            .collect();
        // 2° Schwellwert: Gesamtdrehung 360° → ~180 Punkte, gedeckelt durch
        // die Eingabedichte (128). Deutlich über dem alten Kollaps (~4).
        let out = crate::lichen::resample_edge_by_curvature(
            &circle,
            10.0,
            2.0f32.to_radians(),
            0.01,
            (f32::INFINITY, f32::INFINITY),
        );
        assert!(
            out.len() >= 100,
            "r=1-Kreis bei h=10, α=2° muss dicht bleiben, got {}",
            out.len()
        );
    }

    /// Ticket 6d3f4a7f: der Mindestabstand deckelt die Winkel-Verdichtung —
    /// ein WINZIGER Kreis (r=0.05, Umfang ~0.31 mm) darf bei α=2° nicht mehr
    /// hunderte µm-dichte Punkte bekommen, sobald min_spacing=0.01 mm greift.
    #[test]
    fn min_edge_spacing_caps_tiny_circle_density() {
        let n = 256usize;
        let r = 0.05f32;
        let circle: Vec<[f32; 3]> = (0..=n)
            .map(|k| {
                let t = k as f32 / n as f32 * std::f32::consts::TAU;
                [r * t.cos(), r * t.sin(), 0.0]
            })
            .collect();
        let inf = (f32::INFINITY, f32::INFINITY);
        let unbounded =
            crate::lichen::resample_edge_by_curvature(&circle, 10.0, 2.0f32.to_radians(), 0.0, inf);
        let capped =
            crate::lichen::resample_edge_by_curvature(&circle, 10.0, 2.0f32.to_radians(), 0.01, inf);
        // Umfang 0.314 mm / 0.01 mm ⇒ höchstens ~33 Punkte (+ Enden).
        assert!(
            capped.len() <= 36,
            "min_spacing=0.01 muss den µm-Kreis deckeln, got {}",
            capped.len()
        );
        assert!(
            unbounded.len() > capped.len() * 3,
            "ohne Deckel deutlich dichter: {} vs {}",
            unbounded.len(),
            capped.len()
        );
    }

    /// Ticket cec70614: Ecken-Rand-Bedingung — mündet an einer Ecke eine
    /// dicht abgetastete Kante (Endabstand 0.2), muss die GERADE Nachbar-
    /// kante dort ebenso fein starten und geometrisch auf h wachsen; sonst
    /// klafft zwischen U(Ecke) und dem ersten Kettenpunkt eine Lücke.
    #[test]
    fn edge_grading_honours_corner_end_bounds() {
        let line = vec![[0.0f32, 0.0, 0.0], [30.0, 0.0, 0.0]];
        let out = crate::lichen::resample_edge_by_curvature(
            &line,
            10.0,
            10.0f32.to_radians(),
            0.01,
            (0.2, f32::INFINITY),
        );
        let first = out[1][0] - out[0][0];
        assert!(
            first <= 0.2 * 1.5 * 1.01,
            "erstes Segment muss an die 0.2-Ecke anschließen, got {first}"
        );
        for k in 2..out.len() {
            let d0 = (out[k - 1][0] - out[k - 2][0]).max(1e-6);
            let d1 = (out[k][0] - out[k - 1][0]).max(1e-6);
            assert!(
                d1 / d0 <= 1.6 && d0 / d1 <= 1.6,
                "Grading-Sprung: {d0:.3} → {d1:.3}"
            );
        }
    }

    /// Ticket e1a10bcd: Grading — nach einer dichten Häufung (enge Kurve)
    /// darf die Segmentlänge nicht abrupt auf h springen, sondern wächst
    /// höchstens um den Faktor 1.5 pro Segment. Damit überlappen sich die
    /// U(p)-Kreise benachbarter Kettenpunkte überall (U = Nachbarabstand ⇒
    /// U_a + U_b ≥ 2d/1.5 > d): Kanten sind vollständig überdeckt, und im
    /// U-Kreis einer Ecke liegen ihre Ketten-Nachbarn.
    #[test]
    fn edge_grading_bounds_neighbour_segment_ratio() {
        // Viertelkreis r=0.5 (eng gekrümmt → dichte Winkel-Emission), dann
        // eine 30-mm-Gerade: genau die Ecken-Häufung-neben-h-Lauf-Situation.
        let mut poly: Vec<[f32; 3]> = (0..=64)
            .map(|k| {
                let t = k as f32 / 64.0 * std::f32::consts::FRAC_PI_2;
                [0.5 * t.sin(), 0.5 * (1.0 - t.cos()), 0.0]
            })
            .collect();
        let end = *poly.last().unwrap();
        poly.push([end[0] + 30.0, end[1], 0.0]);
        let out = crate::lichen::resample_edge_by_curvature(
            &poly,
            10.0,
            10.0f32.to_radians(),
            0.01,
            (f32::INFINITY, f32::INFINITY),
        );
        assert!(out.len() >= 8, "Kurve + Grading-Übergang erwartet, got {}", out.len());
        let seg = |a: [f32; 3], b: [f32; 3]| {
            ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2) + (b[2] - a[2]).powi(2)).sqrt()
        };
        for k in 2..out.len() {
            let d0 = seg(out[k - 2], out[k - 1]).max(1e-6);
            let d1 = seg(out[k - 1], out[k]).max(1e-6);
            let ratio = (d1 / d0).max(d0 / d1);
            assert!(
                ratio <= 1.6,
                "Segment-Sprung an {k}: {d0:.4} → {d1:.4} (ratio {ratio:.2})"
            );
        }
    }

    /// Der Trailing-Face-Normalen-Block liefert PRO Randpunkt ≥2 Kreis-Records
    /// (Kante = 2 Flächen, Ecke ≥3), Flächenpunkte genau einen — die Grundlage
    /// für „2 Kreise pro Randpunkt" im Overlay.
    #[test]
    fn lichen_boundary_points_emit_two_circle_records() {
        let mut k = box_kernel();
        let out = k.lichen_mesh(r#"{"targetEdgeLength":3.0,"grow":true,"maxPoints":8000}"#.into());
        let total = out[0] as usize;
        let tris = out[1] as usize;
        let idx_end = 3 + total * 9 + tris * 3;
        let n_face = out[idx_end] as usize;
        let rec_base = idx_end + 1;
        // Records je Punkt-Index zählen.
        let mut per_point = vec![0u32; total];
        for r in 0..n_face {
            let pi = out[rec_base + r * 4] as usize;
            per_point[pi] += 1;
        }
        let mut checked_boundary = 0;
        for i in 0..total {
            let flag = out[3 + i * 9 + 8] as i32;
            match flag {
                2 | 3 => {
                    // Kanten-/Ecken-Punkt: 2 bzw. ≥3 adjazente Flächen.
                    assert!(
                        per_point[i] >= 2,
                        "boundary point {i} (flag {flag}) needs ≥2 circle records, got {}",
                        per_point[i]
                    );
                    checked_boundary += 1;
                }
                1 => assert_eq!(per_point[i], 1, "surface point {i} has exactly one record"),
                // flag 4 = beförderter Advancing-Front-Seed; kann Flächen- ODER
                // Randpunkt gewesen sein → hier nicht restriktiv prüfen.
                _ => {}
            }
        }
        assert!(checked_boundary >= 8, "at least the eight box corners are boundary points");
    }

    #[test]
    fn zz_hole_stats() {
        let mut k = box_kernel();
        let out = k.lichen_mesh(r#"{"targetEdgeLength":3.0,"grow":true,"maxPoints":8000}"#.into());
        let total = out[0] as usize;
        let tris = out[1] as usize;
        let base = 3 + total * 9;
        let mut edge_use: std::collections::HashMap<(u32, u32), u32> =
            std::collections::HashMap::new();
        for t in 0..tris {
            let a = out[base + t * 3] as u32;
            let b = out[base + t * 3 + 1] as u32;
            let c = out[base + t * 3 + 2] as u32;
            for (x, y) in [(a, b), (b, c), (c, a)] {
                *edge_use.entry((x.min(y), x.max(y))).or_default() += 1;
            }
        }
        let interior = edge_use.values().filter(|&&u| u == 2).count();
        let boundary = edge_use.values().filter(|&&u| u == 1).count();
        let over = edge_use.values().filter(|&&u| u > 2).count();
        let mut used_v = std::collections::HashSet::new();
        for t in 0..tris {
            used_v.insert(out[base + t * 3] as u32);
            used_v.insert(out[base + t * 3 + 1] as u32);
            used_v.insert(out[base + t * 3 + 2] as u32);
        }
        let euler = used_v.len() as i64 - interior as i64 + tris as i64;
        eprintln!(
            "HOLESTATS pts={total} usedV={} tris={tris} interiorEdges={interior} boundaryEdges={boundary} over={over} euler(usedV-E+F)={euler}",
            used_v.len()
        );
    }

    /// Step 1: a corner normal is now the MEAN of its adjacent face normals
    /// (stable), not the unstable ∇f. A box corner → the diagonal (±1/√3 each).
    #[test]
    fn lichen_corner_normal_is_face_mean() {
        let mut k = box_kernel();
        let out = k.lichen_mesh(r#"{"targetEdgeLength":10.0,"grow":false,"triangulate":false}"#.into());
        let total = out[0] as usize;
        let inv_sqrt3 = 1.0f32 / 3.0f32.sqrt();
        let mut checked = 0;
        for i in 0..total {
            let b = 3 + i * 9;
            if out[b + 8] as i32 != 3 {
                continue; // only corners
            }
            let n = [out[b + 4], out[b + 5], out[b + 6]];
            for c in 0..3 {
                assert!(
                    (n[c].abs() - inv_sqrt3).abs() < 0.05,
                    "corner normal is not the diagonal face-mean: {n:?}"
                );
            }
            checked += 1;
        }
        assert_eq!(checked, 8, "all eight box corners checked");
    }
}
